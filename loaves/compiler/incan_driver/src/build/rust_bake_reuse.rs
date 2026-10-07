//! Receipt-bound reuse of Rust caller binaries before checking or rebuilding their Incan caller facets.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::build::source_authority::digest_baked_project_source_authority;
use crate::build::{OvenProjectBakeProfileReport, OvenProjectBakeReport};
use crate::error::{CliError, CliResult};
use incan_frontend::library_manifest::digest_provider_artifact;
use incan_lang::version::INCAN_VERSION;
use incan_provider::FeatureSelection;
use oven_model::manifest::ProjectManifest;
use oven_rustc::rustc::{resolve_active_rustc, rustc_host_target, rustc_identity};
use oven_store::store::{OvenArtifactKind, OvenArtifactMaterializedFile, OvenArtifactPublishRequest, OvenStore};
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
    Ok(Some(RustBakeReuseKey {
        digest: input_digest(&inputs)?,
        target,
        toolchain,
    }))
}

/// Construct one lookup receipt whose intent distinguishes each unit and native profile.
fn lookup_receipt(root: &Path, key: &RustBakeReuseKey, unit: &str, profile: &str) -> CliResult<OvenReceipt> {
    receipt_generated_project(
        &OvenGeneratedProjectRequest::new(root, unit, "1", &key.target, &key.toolchain, profile, Vec::new())
            .with_generated_source("project-manifest", root.join("loaf.toml"))
            .with_build_unit_input("rust-caller-bake-inputs", &key.digest),
    )
    .map_err(|error| CliError::failure(error.to_string()))
}

/// Validate a manifest unit name before using it as a caller-relative artifact path.
fn output_paths(root: &Path, unit: &str, profile: &str) -> CliResult<(PathBuf, PathBuf)> {
    let relative = crate::build::output_paths::validated_project_output_relative_path(unit, "Rust unit")?;
    if relative.components().count() != 1 {
        return Err(CliError::failure("Rust unit name must be a single path component"));
    }
    Ok((
        root.join("target/rust").join(profile).join(unit),
        root.join("target/rust/receipts").join(format!("{unit}-{profile}.json")),
    ))
}

/// Select every unit/profile before restoring any caller output; incomplete sets are ordinary cache misses.
pub(super) fn try_reuse_rust_bake(
    root: &Path,
    units: &[(String, PathBuf)],
    store: &OvenStore,
    key: &RustBakeReuseKey,
) -> CliResult<Option<OvenProjectBakeReport>> {
    let mut selected = Vec::new();
    for (unit, _) in units {
        for profile in crate::build::plan_authority::explicit_bake_profiles() {
            let lookup = lookup_receipt(root, key, unit, profile)?;
            let candidates = store
                .select_payloads_matching_for_execution(|header| {
                    header.kind == OvenArtifactKind::ProjectOutput
                        && header.receipt_identity == lookup.identity
                        && header.build_unit_identity == lookup.build_unit_identity
                        && header.intent == lookup.intent
                })
                .map_err(|error| CliError::failure(error.to_string()))?;
            if candidates.len() != 1 {
                return Ok(None);
            }
            let Some(candidate) = candidates.into_iter().next() else {
                return Ok(None);
            };
            let payload: RustBakeOutput =
                serde_json::from_slice(&candidate.payload).map_err(|error| CliError::failure(error.to_string()))?;
            if payload.schema != "rust-caller-output/1"
                || payload.unit != *unit
                || payload.plan.is_empty()
                || payload.receipt.verify_identity().is_err()
                || payload.receipt.intent != lookup.intent
            {
                return Ok(None);
            }
            candidate
                .verify_materialized_files()
                .map_err(|error| CliError::failure(error.to_string()))?;
            selected.push((unit.clone(), profile, candidate, payload));
        }
    }
    let mut profiles = Vec::new();
    for (unit, profile, candidate, payload) in selected {
        let (output, receipt_path) = output_paths(root, &unit, profile)?;
        let stored = candidate.artifact_root.join("binary");
        let expected = file_digest(&stored)?;
        if file_digest(&output).ok().as_ref() != Some(&expected) {
            fs::create_dir_all(
                output
                    .parent()
                    .ok_or_else(|| CliError::failure("Rust output has no parent"))?,
            )
            .map_err(|error| CliError::failure(error.to_string()))?;
            fs::copy(&stored, &output).map_err(|error| CliError::failure(error.to_string()))?;
        }
        write_receipt(&payload.receipt, &receipt_path).map_err(|error| CliError::failure(error.to_string()))?;
        profiles.push(OvenProjectBakeProfileReport {
            project_target: format!("rust:{unit}"),
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
            .map(|(unit, source)| (format!("rust:{unit}"), source.clone()))
            .collect(),
    }))
}

/// Admit successful native outputs into the existing bounded release domain, with original receipts intact.
pub(super) fn publish_rust_bake(
    store: &OvenStore,
    key: &RustBakeReuseKey,
    report: &OvenProjectBakeReport,
) -> CliResult<()> {
    for profile in &report.profiles {
        let unit = profile
            .project_target
            .strip_prefix("rust:")
            .ok_or_else(|| CliError::failure("expected Rust target"))?;
        let (output, _) = output_paths(&report.project, unit, &profile.profile)?;
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
            schema: "rust-caller-output/1".into(),
            unit: unit.into(),
            plan: profile.plan_identity.clone(),
            receipt,
        };
        store
            .publish(&OvenArtifactPublishRequest {
                receipt: lookup_receipt(&report.project, key, unit, &profile.profile)?,
                domain: format!("incan-release-{INCAN_VERSION}"),
                kind: OvenArtifactKind::ProjectOutput,
                payload: serde_json::to_vec(&payload).map_err(|error| CliError::failure(error.to_string()))?,
                materialized_files: vec![OvenArtifactMaterializedFile {
                    source_path: output,
                    relative_path: "binary".into(),
                }],
                materialized_directories: Vec::new(),
            })
            .map_err(|error| CliError::failure(error.to_string()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A portable lookup restores every profile in a fresh directory and rejects changed semantic inputs.
    #[test]
    fn rust_bake_reuse_restores_copied_project_and_refuses_changed_key() -> Result<(), Box<dyn std::error::Error>> {
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
        let mut profiles = Vec::new();
        for profile in crate::build::plan_authority::explicit_bake_profiles() {
            let (output, receipt_path) = output_paths(&original, "caller", profile)?;
            fs::create_dir_all(output.parent().ok_or("missing output parent")?)?;
            fs::write(output, format!("native {profile}"))?;
            let receipt = lookup_receipt(&original, &key, "caller", profile)?;
            write_receipt(&receipt, &receipt_path)?;
            profiles.push(OvenProjectBakeProfileReport {
                project_target: "rust:caller".into(),
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
        publish_rust_bake(&store, &key, &report)?;
        let units = vec![("caller".into(), copied.join("src/main.rs"))];
        let reused = try_reuse_rust_bake(&copied, &units, &store, &key)?.ok_or("portable reuse missed")?;
        assert_eq!(reused.profiles.len(), report.profiles.len());
        for profile in &reused.profiles {
            assert_eq!(profile.action, "reused");
            assert_eq!(
                fs::read_to_string(output_paths(&copied, "caller", &profile.profile)?.0)?,
                format!("native {}", profile.profile)
            );
        }
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
