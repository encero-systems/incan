//! Receipt-bound reuse of Rust caller targets before checking or rebuilding their Incan caller facets.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::bake::ProjectRustUnit;
use crate::build::output_selection::{project_output_locator, remember_project_output};
use crate::build::source_authority::digest_baked_project_source_authority;
use crate::build::{OvenProjectBakeProfileReport, OvenProjectBakeReport};
use crate::error::{CliError, CliResult};
use incan_frontend::library_manifest::digest_provider_artifact;
use incan_lang::version::INCAN_VERSION;
use incan_provider::FeatureSelection;
use oven_model::manifest::ProjectManifest;
use oven_rustc::rustc::{resolve_active_rustc, rustc_host_target, rustc_identity};
use oven_store::store::{
    OvenArtifactKind, OvenArtifactManifest, OvenArtifactMaterializedFile, OvenArtifactPublishRequest, OvenStore,
    OvenStoreExecutionPayload,
};
use oven_store::{OvenGeneratedProjectRequest, OvenReceipt, digest_bytes, receipt_generated_project, write_receipt};

/// A command-local proof of all inputs used by both profiles of a Rust caller bake.
#[derive(PartialEq, Eq)]
pub(super) struct RustBakeReuseKey {
    digest: String,
    target: String,
    toolchain: String,
}

/// Stored outputs keep the native compilation's original receipt separate from the lookup receipt.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RustBakeOutput {
    schema: String,
    unit: String,
    plan: String,
    receipt: OvenReceipt,
}

/// Hash a regular file with filesystem-identity-bound acceleration for unchanged compiler-sized inputs.
fn file_digest(path: &Path) -> CliResult<String> {
    super::file_freshness::digest_file(path).map_err(|error| CliError::failure(error.to_string()))
}

/// Derive the portable input key from named content identities, never checkout paths or mtimes.
fn input_digest(inputs: &BTreeMap<String, String>) -> CliResult<String> {
    serde_json::to_vec(inputs)
        .map(|bytes| digest_bytes(&bytes))
        .map_err(|error| CliError::failure(error.to_string()))
}

/// Read current source, compiler, SDK and driver permission evidence before expensive caller preparation.
///
/// The source authority includes canonical lock selections and transitive provider artifact bytes. SDK artifacts
/// are independently hashed; their recorded inventory digests alone cannot authorize a changed installation.
/// Ambient compiler flags are absent because direct rustc freezes its environment from admitted plan facts.
pub(super) fn rust_bake_reuse_key(
    manifest: &ProjectManifest,
    features: &FeatureSelection,
    requested_target: Option<&str>,
) -> CliResult<Option<RustBakeReuseKey>> {
    let Some(sdk) = incan_provider::inventory::discover_active_sdk_inventory()
        .map_err(|error| CliError::failure(error.to_string()))?
    else {
        if std::env::var_os("INCAN_OVEN_TRACE_REUSE").is_some() {
            eprintln!("Rust caller reuse: no active SDK inventory");
        }
        return Ok(None);
    };
    let rustc = resolve_active_rustc().map_err(|error| CliError::failure(error.to_string()))?;
    let target = match requested_target {
        Some(target) => target.to_string(),
        None => rustc_host_target(&rustc).map_err(|error| CliError::failure(error.to_string()))?,
    };
    let toolchain = rustc_identity(&rustc).map_err(|error| CliError::failure(error.to_string()))?;
    let compiler = std::env::current_exe().map_err(|error| CliError::failure(error.to_string()))?;
    let mut inputs = BTreeMap::from([
        ("schema".into(), "rust-caller-bake/1".into()),
        (
            "sources".into(),
            digest_baked_project_source_authority(manifest.project_root())?,
        ),
        ("compiler".into(), file_digest(&compiler)?),
        ("rustc".into(), file_digest(&rustc)?),
        ("toolchain".into(), toolchain.clone()),
        ("target".into(), target.clone()),
        ("features".into(), format!("{:?}", features)),
        (
            "sdk-inventory".into(),
            file_digest(&sdk.root.join(incan_provider::SDK_INVENTORY_FILE))?,
        ),
    ]);
    for (component, entry) in &sdk.components {
        for provider in &entry.providers {
            if let Some(root) = &provider.crate_root {
                inputs.insert(
                    format!("sdk:{component}:{}", provider.name),
                    digest_provider_artifact(root).map_err(|error| CliError::failure(error.to_string()))?,
                );
            }
        }
    }
    for role in manifest.rust_binary_roles() {
        if let Some(grant) = oven_rustc::rustc::driver_grant::authorize_driver_grant(role, &rustc)
            .map_err(|error| CliError::failure(error.to_string()))?
        {
            inputs.insert(format!("driver:{}", role.name), grant);
        }
    }
    if std::env::var_os("INCAN_OVEN_TRACE_REUSE").is_some() {
        eprintln!(
            "Rust caller reuse inputs: {}",
            serde_json::to_string(&inputs).map_err(|error| CliError::failure(error.to_string()))?
        );
    }
    Ok(Some(RustBakeReuseKey {
        digest: input_digest(&inputs)?,
        target,
        toolchain,
    }))
}

/// Construct one lookup receipt whose intent distinguishes each unit and native profile.
fn lookup_receipt(
    root: &Path,
    key: &RustBakeReuseKey,
    unit: &ProjectRustUnit,
    profile: &str,
) -> CliResult<OvenReceipt> {
    receipt_generated_project(
        &OvenGeneratedProjectRequest::new(root, &unit.name, "1", &key.target, &key.toolchain, profile, Vec::new())
            .with_generated_source("project-manifest", root.join("loaf.toml"))
            .with_build_unit_input("rust-caller-bake-inputs", &key.digest)
            .with_build_unit_input("rust-caller-target", unit.report_target())
            .with_build_unit_input("rust-caller-edition", &unit.edition),
    )
    .map_err(|error| CliError::failure(error.to_string()))
}

/// Replace a stale or read-only caller output through a private staged file, preserving immutable aliases.
///
/// Matching writable regular files remain untouched. Store permissions never make the caller projection read-only;
/// the copy completes before publication, so an ordinary copy failure leaves the previous generation selected.
fn materialize_rust_caller_output(stored: &Path, output: &Path, library: bool) -> CliResult<()> {
    #[cfg(not(unix))]
    let _ = library;
    let expected = file_digest(stored)?;
    let current = fs::symlink_metadata(output).is_ok_and(|metadata| {
        metadata.is_file() && !metadata.file_type().is_symlink() && !metadata.permissions().readonly()
    }) && file_digest(output).ok().as_ref() == Some(&expected);
    if current {
        return Ok(());
    }
    let parent = output
        .parent()
        .ok_or_else(|| CliError::failure("Rust output has no parent"))?;
    fs::create_dir_all(parent).map_err(|error| CliError::failure(error.to_string()))?;
    let mut source = fs::File::open(stored).map_err(|error| CliError::failure(error.to_string()))?;
    let mut temporary =
        tempfile::NamedTempFile::new_in(parent).map_err(|error| CliError::failure(error.to_string()))?;
    std::io::copy(&mut source, temporary.as_file_mut()).map_err(|error| CliError::failure(error.to_string()))?;
    #[cfg(unix)]
    let permissions = {
        use std::os::unix::fs::PermissionsExt;
        fs::Permissions::from_mode(if library { 0o644 } else { 0o755 })
    };
    #[cfg(not(unix))]
    let permissions = {
        let mut permissions = source
            .metadata()
            .map_err(|error| CliError::failure(error.to_string()))?
            .permissions();
        permissions.set_readonly(false);
        permissions
    };
    temporary
        .as_file()
        .set_permissions(permissions)
        .map_err(|error| CliError::failure(error.to_string()))?;
    temporary
        .persist(output)
        .map_err(|error| CliError::failure(error.error.to_string()))?;
    Ok(())
}

/// Select a caller through the existing output address before searching the catalog on a missing or mismatched hint.
///
/// The hint grants no execution authority: exact Store admission retains payload integrity and the original owner
/// lease, and the current lookup receipt must still match. Damaged admitted owners refuse rather than falling back.
fn select_rust_bake_output(
    store: &OvenStore,
    receipt_path: &Path,
    lookup: &OvenReceipt,
) -> CliResult<Option<OvenStoreExecutionPayload>> {
    let matches = |header: &OvenArtifactManifest| {
        header.kind == OvenArtifactKind::ProjectOutput
            && header.receipt_identity == lookup.identity
            && header.build_unit_identity == lookup.build_unit_identity
            && header.intent == lookup.intent
    };
    if let Some(identity) = project_output_locator(receipt_path) {
        let located = store
            .try_select_payloads_for_execution(&[identity])
            .map_err(|error| CliError::failure(error.to_string()))?;
        if let Some(candidate) = located
            .and_then(|mut candidates| candidates.pop())
            .filter(|candidate| matches(candidate.manifest()))
        {
            return Ok(Some(candidate));
        }
    }
    let mut candidates = store
        .select_payloads_matching_for_execution(matches, |_, _| true)
        .map_err(|error| CliError::failure(error.to_string()))?;
    Ok((candidates.len() == 1).then(|| candidates.pop()).flatten())
}

/// Select every unit/profile before restoring any caller output; incomplete sets are ordinary cache misses.
pub(super) fn try_reuse_rust_bake(
    root: &Path,
    units: &[ProjectRustUnit],
    store: &OvenStore,
    key: &RustBakeReuseKey,
) -> CliResult<Option<OvenProjectBakeReport>> {
    let mut selected = Vec::new();
    for unit in units {
        for profile in crate::build::plan_authority::explicit_bake_profiles() {
            let lookup = lookup_receipt(root, key, unit, profile)?;
            let (_, receipt_path) = unit.output_paths(root, profile)?;
            let Some(candidate) = select_rust_bake_output(store, &receipt_path, &lookup)? else {
                return Ok(None);
            };
            let payload: RustBakeOutput =
                serde_json::from_slice(candidate.payload()).map_err(|error| CliError::failure(error.to_string()))?;
            if payload.schema != "rust-caller-output/2"
                || payload.unit != unit.report_target()
                || payload.plan.is_empty()
                || payload.receipt.verify_identity().is_err()
                || payload.receipt.intent != lookup.intent
            {
                return Ok(None);
            }
            candidate
                .verify_materialized_files()
                .map_err(|error| CliError::failure(error.to_string()))?;
            selected.push((unit, profile, candidate, payload));
        }
    }
    let mut profiles = Vec::new();
    for (unit, profile, candidate, payload) in selected {
        let (output, receipt_path) = unit.output_paths(root, profile)?;
        let stored = candidate.artifact_root().join("native-output");
        materialize_rust_caller_output(&stored, &output, unit.library)?;
        let mut expected_receipt =
            serde_json::to_vec_pretty(&payload.receipt).map_err(|error| CliError::failure(error.to_string()))?;
        expected_receipt.push(b'\n');
        if !fs::read(&receipt_path).is_ok_and(|bytes| bytes == expected_receipt) {
            write_receipt(&payload.receipt, &receipt_path).map_err(|error| CliError::failure(error.to_string()))?;
        }
        remember_project_output(&receipt_path, &candidate.manifest().identity);
        profiles.push(OvenProjectBakeProfileReport {
            project_target: unit.report_target(),
            profile: profile.into(),
            target: key.target.clone(),
            toolchain: key.toolchain.clone(),
            receipt: receipt_path,
            receipt_identity: payload.receipt.identity,
            build_unit_identity: payload.receipt.build_unit_identity,
            plan_identity: payload.plan,
            action: "reused",
        });
    }
    Ok(Some(OvenProjectBakeReport {
        project: root.into(),
        store: store.root().into(),
        profiles,
        outputs: Vec::new(),
        generated_sources: units
            .iter()
            .map(|unit| (unit.report_target(), unit.source.clone()))
            .collect(),
    }))
}

/// Admit successful native outputs into the existing bounded release domain, with original receipts intact.
pub(super) fn publish_rust_bake(
    store: &OvenStore,
    key: &RustBakeReuseKey,
    report: &OvenProjectBakeReport,
    units: &[ProjectRustUnit],
) -> CliResult<()> {
    for profile in &report.profiles {
        let unit = units
            .iter()
            .find(|unit| unit.report_target() == profile.project_target)
            .ok_or_else(|| CliError::failure("Rust bake report names an undeclared target"))?;
        let (output, expected_receipt) = unit.output_paths(&report.project, &profile.profile)?;
        if profile.receipt != expected_receipt {
            return Err(CliError::failure("Rust bake report names a different target receipt"));
        }
        let receipt: OvenReceipt =
            serde_json::from_slice(&fs::read(&profile.receipt).map_err(|error| CliError::failure(error.to_string()))?)
                .map_err(|error| CliError::failure(error.to_string()))?;
        receipt
            .verify_identity()
            .map_err(|error| CliError::failure(error.to_string()))?;
        if receipt.identity != profile.receipt_identity
            || receipt.intent.target != key.target
            || receipt.intent.toolchain != key.toolchain
        {
            return Err(CliError::failure("Rust bake output disagrees with its receipt"));
        }
        let payload = RustBakeOutput {
            schema: "rust-caller-output/2".into(),
            unit: unit.report_target(),
            plan: profile.plan_identity.clone(),
            receipt,
        };
        let published = store
            .publish(&OvenArtifactPublishRequest {
                receipt: lookup_receipt(&report.project, key, unit, &profile.profile)?,
                domain: format!("incan-release-{INCAN_VERSION}"),
                kind: OvenArtifactKind::ProjectOutput,
                payload: serde_json::to_vec(&payload).map_err(|error| CliError::failure(error.to_string()))?,
                materialized_files: vec![OvenArtifactMaterializedFile {
                    source_path: output,
                    relative_path: "native-output".into(),
                }],
                materialized_directories: Vec::new(),
            })
            .map_err(|error| CliError::failure(error.to_string()))?;
        remember_project_output(&profile.receipt, &published.identity);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A portable lookup restores every profile in a fresh directory and rejects changed semantic inputs.
    #[test]
    fn rust_bake_reuse_restores_copied_project_and_refuses_changed_key() -> Result<(), Box<dyn std::error::Error>> {
        prove_rust_bake_relocation(false)
    }

    /// A library caller retains the same relocation, exact locator and corruption guarantees as a binary.
    #[test]
    fn rust_library_bake_reuse_restores_and_refuses_changed_key() -> Result<(), Box<dyn std::error::Error>> {
        prove_rust_bake_relocation(true)
    }

    /// Exercise shared Store reuse for both native target kinds without executing fabricated fixture bytes.
    fn prove_rust_bake_relocation(library: bool) -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let original = root.path().join("original");
        let copied = root.path().join("copied");
        for project in [&original, &copied] {
            fs::create_dir_all(project)?;
            fs::write(project.join("loaf.toml"), "[project]\nname = \"caller\"\n")?;
        }
        let store = OvenStore::new(
            root.path().join("store"),
            oven_store::store::OvenStoreLimits::new(1_000_000, 1_000_000, 1_000_000),
        );
        let key = RustBakeReuseKey {
            digest: digest_bytes(b"all semantic inputs"),
            target: "aarch64-apple-darwin".into(),
            toolchain: "rustc test".into(),
        };
        let mut unit = ProjectRustUnit::binary("caller".into(), copied.join("src/main.rs"));
        unit.library = library;
        let units = vec![unit];
        let mut profiles = Vec::new();
        for profile in crate::build::plan_authority::explicit_bake_profiles() {
            let (output, receipt_path) = units[0].output_paths(&original, profile)?;
            fs::create_dir_all(output.parent().ok_or("missing output parent")?)?;
            fs::write(output, format!("native {profile}"))?;
            let receipt = lookup_receipt(&original, &key, &units[0], profile)?;
            write_receipt(&receipt, &receipt_path)?;
            profiles.push(OvenProjectBakeProfileReport {
                project_target: units[0].report_target(),
                profile: profile.into(),
                target: key.target.clone(),
                toolchain: key.toolchain.clone(),
                receipt: receipt_path,
                receipt_identity: receipt.identity,
                build_unit_identity: receipt.build_unit_identity,
                plan_identity: "plan identity".into(),
                action: "baked",
            });
        }
        let report = OvenProjectBakeReport {
            project: original,
            generated_sources: BTreeMap::new(),
            store: store.root().into(),
            profiles,
            outputs: Vec::new(),
        };
        publish_rust_bake(&store, &key, &report, &units)?;
        let reused = try_reuse_rust_bake(&copied, &units, &store, &key)?.ok_or("portable reuse missed")?;
        assert_eq!(reused.profiles.len(), report.profiles.len());
        for profile in &reused.profiles {
            assert_eq!(profile.action, "reused");
            assert_eq!(
                fs::read_to_string(units[0].output_paths(&copied, &profile.profile)?.0)?,
                format!("native {}", profile.profile)
            );
        }

        // Exact locators from publication and restored copies must avoid unrelated Store owners.
        let unrelated = store.root().join("entries").join(format!("sha256-{}", "0".repeat(64)));
        fs::create_dir(&unrelated)?;
        fs::write(unrelated.join("artifact.json"), b"not a manifest")?;
        for project in [&report.project, &copied] {
            assert!(try_reuse_rust_bake(project, &units, &store, &key)?.is_some());
        }
        fs::remove_dir_all(&unrelated)?;

        // Missing or malformed hints may cost a search, but cannot prevent portable reuse.
        for hint in [
            b"\"../../foreign\"".to_vec(),
            serde_json::to_vec(&format!("sha256:{}", "f".repeat(64)))?,
        ] {
            for profile in &reused.profiles {
                let (_, receipt) = units[0].output_paths(&copied, &profile.profile)?;
                fs::write(receipt.with_extension("output.json"), &hint)?;
            }
            assert!(try_reuse_rust_bake(&copied, &units, &store, &key)?.is_some());
        }

        // An exact locator does not authorize edited output bytes, even with a restored modification time.
        let (edited_output, _) = units[0].output_paths(&copied, "debug")?;
        let modified = fs::metadata(&edited_output)?.modified()?;
        let expected = fs::read(&edited_output)?;
        fs::write(&edited_output, vec![b'X'; expected.len()])?;
        fs::File::options()
            .write(true)
            .open(&edited_output)?
            .set_times(fs::FileTimes::new().set_modified(modified))?;
        assert!(try_reuse_rust_bake(&copied, &units, &store, &key)?.is_some());
        assert_eq!(fs::read(&edited_output)?, expected);
        let changed = RustBakeReuseKey {
            digest: digest_bytes(b"changed dependency receipt"),
            ..key
        };
        assert!(try_reuse_rust_bake(&copied, &units, &store, &changed)?.is_none());
        Ok(())
    }

    /// Every semantic input participates independently; map insertion order cannot change the key.
    #[test]
    fn rust_bake_key_binds_every_input() -> Result<(), Box<dyn std::error::Error>> {
        let inputs = BTreeMap::from_iter(
            [
                "sources",
                "dependency-receipts",
                "compiler",
                "rustc",
                "toolchain",
                "target",
                "features",
                "sdk",
                "driver",
            ]
            .map(|name| (name.into(), "unchanged".into())),
        );
        let original = input_digest(&inputs)?;
        for name in inputs.keys() {
            let mut changed = inputs.clone();
            changed.insert(name.clone(), "changed".into());
            assert_ne!(original, input_digest(&changed)?, "{name} must invalidate reuse");
        }
        assert_eq!(original, input_digest(&inputs.into_iter().rev().collect())?);
        Ok(())
    }
}
