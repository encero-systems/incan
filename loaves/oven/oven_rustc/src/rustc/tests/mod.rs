//! Shared fixtures for direct-rustc planning, materialization, and execution regression tests.

mod compiler_environment;
mod direct_execution;
mod foundation_composition;
mod native_inputs;
mod path_library_execution;
mod receipts_and_inspection;
mod registry_and_toolchain;
mod source_roles;

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use super::{
    OVEN_PROJECT_INSPECTION_AUTHORITY_SCHEMA_VERSION, OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION,
    OvenCallerOwnedRustcLibrary, OvenClosureProof, OvenDirectRustcTestRequest, OvenProjectInspectionAuthorityPayload,
    OvenProjectInspectionAuthorityRef, OvenProjectInspectionConstituent, OvenProjectInspectionRootDependency,
    OvenProjectInspectionSource, OvenProjectInspectionSourceOwner, OvenProjectInspectionTestDependencyEnvelope,
    OvenProjectInspectionTestDependencyRoot, OvenProjectInspectionTestProviderConstituent, OvenRegistryLeafAuthority,
    OvenRustcArtifactExtern, OvenRustcArtifactManifest, OvenRustcArtifactPlan, OvenRustcAuxiliaryTarget,
    OvenRustcError, OvenRustcRegistryLeaf, OvenRustcRegistryLeafDomain, OvenRustcRegistryLeafKind,
    OvenRustcRegistrySource, OvenRustcRegistrySourcePackage, OvenRustcSupportingArtifact,
    OvenSelectedPathRustcAuthority, OvenStoredDirectRustcRunRequest, OvenStoredDirectRustcTestRequest,
    OvenTrustedDirectRustcTargetRequest, OvenTrustedRustcArtifactRoot, OvenTrustedRustdocTestRequest,
    apply_oven_profile, attach_caller_owned_rustc_libraries, bake_direct_rustc_test, bake_stored_direct_rustc_run,
    bake_stored_direct_rustc_test, bake_trusted_direct_rustc_dylib, bake_trusted_direct_rustc_library,
    bake_trusted_direct_rustc_proc_macro, bake_trusted_direct_rustc_run, bake_trusted_direct_rustc_test,
    combined_process_output, is_host_native_unix_target, load_project_inspection_authority,
    materialize_declared_rust_libraries, materialize_declared_rust_libraries_with_selected_path_authority,
    project_inspection_authority_supports_dependencies, project_inspection_constituent_matches_receipt,
    project_inspection_test_dependency_envelope_mismatch,
    project_inspection_test_dependency_envelope_supports_dependencies, resolve_sealed_registry_leaf,
    run_trusted_rustdoc_test, rustc_dynamic_library_environment, rustc_host_target, select_direct_rustc_plan_identity,
    validate_project_extension_payload_against_base, validate_project_inspection_authority_payload,
};
use crate::native_contract::{
    OVEN_PROJECT_EXTENSION_PAYLOAD_SCHEMA_VERSION, OvenProjectExtensionPayload, OvenProjectRegistrySourceDependency,
};
use crate::native_test::run_native_test_batch_all;
use oven_model::manifest::{DependencySource, DependencySpec};
use oven_store::store::{
    OvenArtifactKind, OvenArtifactMaterializedFile, OvenArtifactPublishRequest, OvenStore, OvenStoreLimits,
};
use oven_store::{
    OVEN_COMPILER_TEST_PROFILE, OvenGeneratedProjectRequest, OvenImportRequest, digest_bytes, import_frozen_project,
    receipt_generated_project,
};

/// A real store-selected payload with explicit role, registry and transitive physical records.
struct NativeInputFixture {
    owner: oven_store::store::OvenStoreExecutionPayload,
    receipt: oven_store::OvenReceipt,
    store: OvenStore,
    root: tempfile::TempDir,
}

/// Publish fixture bytes through the normal store without invoking a compiler or interpreting dependency requests.
fn native_input_fixture(profile: &str) -> Result<NativeInputFixture, Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    write_project(root.path())?;
    let receipt = receipt_generated_project(
        &OvenGeneratedProjectRequest::new(
            root.path(),
            "native_input_fixture",
            "0.1.0",
            "aarch64-apple-darwin",
            "rustc fixture",
            profile,
            Vec::new(),
        )
        .with_generated_source("generated-root", root.path().join("fixture.rs")),
    )?;
    let source = root.path().join("publisher");
    let files = [
        ("release/deps/libruntime.rlib", "runtime bytes"),
        ("release/deps/libserde_fixture.rlib", "registry bytes"),
        ("release/deps/libtransitive.rlib", "transitive bytes"),
        ("helper/libprivate.rlib", "private helper bytes"),
        ("native/libsupport.a", "native support bytes"),
        (
            "registry-sources/fixture/Cargo.toml",
            "[package]\nname='serde_fixture'\nversion='1.2.3'\n",
        ),
        (super::OVEN_RUSTC_REGISTRY_LOCK_RELATIVE_PATH, "version = 4\n"),
    ];
    for (relative, bytes) in files {
        let path = source.join(relative);
        fs::create_dir_all(path.parent().ok_or("fixture path missing parent")?)?;
        fs::write(path, bytes)?;
    }
    let mut artifacts = empty_manifest(&receipt);
    artifacts.dependency_search_paths = vec!["release/deps".to_string(), "helper".to_string()];
    artifacts.native_search_paths = vec!["native".to_string()];
    artifacts
        .compile_environment
        .insert("CARGO_PKG_NAME".to_string(), "native_input_fixture".to_string());
    artifacts.externs = vec![
        OvenRustcArtifactExtern {
            crate_name: "runtime".to_string(),
            relative_path: files[0].0.to_string(),
            digest: digest_bytes(files[0].1.as_bytes()),
        },
        OvenRustcArtifactExtern {
            crate_name: "private".to_string(),
            relative_path: files[3].0.to_string(),
            digest: digest_bytes(files[3].1.as_bytes()),
        },
    ];
    artifacts.entrypoint_externs = BTreeMap::from([
        ("generated-root".to_string(), vec!["runtime".to_string()]),
        ("compiler-helper".to_string(), vec!["private".to_string()]),
    ]);
    artifacts.supporting_artifacts = files
        .iter()
        .enumerate()
        .filter(|(index, _)| *index != 0 && *index != 3)
        .map(|(_, (relative, bytes))| OvenRustcSupportingArtifact {
            relative_path: (*relative).to_string(),
            digest: digest_bytes(bytes.as_bytes()),
        })
        .collect();
    let registry_source = fixture_registry_source();
    artifacts.registry_sources = vec![OvenRustcRegistrySourcePackage {
        package: "serde_fixture".to_string(),
        version: "1.2.3".to_string(),
        features: vec!["derive".to_string()],
        source: registry_source.clone(),
    }];
    artifacts.registry_leaves = vec![OvenRustcRegistryLeaf {
        domain: Default::default(),
        crate_kind: Default::default(),
        selected_unit_identity: Some("sha256:fixture-portable-unit".to_string()),
        package: "serde_fixture".to_string(),
        version: "1.2.3".to_string(),
        crate_name: "serde_fixture".to_string(),
        features: vec!["derive".to_string()],
        source: registry_source,
        artifact: OvenRustcArtifactExtern {
            crate_name: "serde_fixture".to_string(),
            relative_path: files[1].0.to_string(),
            digest: digest_bytes(files[1].1.as_bytes()),
        },
    }];
    // Every declared source role carries a publisher-selected search closure. Each role gets only the
    // directories one of its own direct roots actually lives under, so the helper role keeps no claim on the
    // generated root's `release/deps` -- the exclusion `for_source_evidence` used to derive is now recorded.
    artifacts.entrypoint_dependency_search_paths = artifacts
        .entrypoint_externs
        .iter()
        .map(|(role, names)| {
            let paths = artifacts
                .dependency_search_paths
                .iter()
                .filter(|search_path| {
                    artifacts.externs.iter().any(|artifact| {
                        names.contains(&artifact.crate_name)
                            && super::artifact_is_below_search_path(&artifact.relative_path, search_path)
                    })
                })
                .cloned()
                .collect::<Vec<_>>();
            artifacts
                .capture_source_search_closure(&paths)
                .map(|closure| (role.clone(), closure))
        })
        .collect::<Result<BTreeMap<_, _>, _>>()?;
    let materialized = artifacts
        .materialized_artifacts(&source, &receipt.intent)?
        .into_iter()
        .map(|artifact| oven_store::store::OvenArtifactMaterializedFile {
            source_path: artifact.source_path,
            relative_path: artifact.relative_path,
        })
        .collect();
    let store = OvenStore::new(
        root.path().join("store"),
        OvenStoreLimits::new(4 * 1024 * 1024, 4 * 1024 * 1024, 4 * 1024 * 1024),
    );
    let published = store.publish(&OvenArtifactPublishRequest {
        receipt: receipt.clone(),
        domain: "native-input-view-fixture".to_string(),
        kind: OvenArtifactKind::DirectRustcPlan,
        payload: serde_json::to_vec(&artifacts)?,
        materialized_files: materialized,
        materialized_directories: Vec::new(),
    })?;
    let owner = store
        .select_payloads_for_execution(&[published.identity])?
        .pop()
        .ok_or("selected fixture absent")?;
    Ok(NativeInputFixture {
        owner,
        receipt,
        store,
        root,
    })
}

/// Round-trip a host-issued named-member ID while retaining its actual publisher, role and bytes.
fn fixture_registry_source() -> OvenRustcRegistrySource {
    OvenRustcRegistrySource {
        registry: "registry+https://example.invalid/index".to_string(),
        checksum: "fixture-checksum".to_string(),
        relative_root: "registry-sources/fixture".to_string(),
        digest: digest_bytes(b"fixture registry source"),
    }
}

/// A package compiled for the host beside its target build, and a procedural macro, are sealed as distinct
/// leaves; a consumer's registry dependency still selects the target library and nothing else.
/// Build a Rustup-shaped home holding the named channels, each carrying a `rustc` and `cargo`.
fn provisioned_rust_root(root: &Path, channels: &[&str]) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let rust_root = root.join("rust");
    for channel in channels {
        let bin = rust_root.join("toolchains").join(channel).join("bin");
        fs::create_dir_all(&bin)?;
        fs::write(bin.join("rustc"), b"")?;
        fs::write(bin.join("cargo"), b"")?;
    }
    Ok(rust_root)
}

/// Assert the invariant `bind_source_search_roles` later enforces: a role that claims a directory claims every
/// artifact the manifest declares in it.
fn assert_role_closures_cover_declared_artifacts(
    composed: &OvenRustcArtifactManifest,
) -> Result<(), Box<dyn std::error::Error>> {
    let declared = super::expected_artifacts(composed)?;
    for (key, closure) in &composed.entrypoint_dependency_search_paths {
        let claimed_paths = closure.paths().cloned().collect::<BTreeSet<_>>();
        let claimed = closure
            .directories()
            .flat_map(|directory| &directory.artifacts)
            .map(|artifact| artifact.relative_path.as_str())
            .collect::<BTreeSet<_>>();
        for relative in declared.keys() {
            let owning = claimed_paths
                .iter()
                .any(|path| super::artifact_is_below_search_path(relative, path));
            assert!(
                !owning || claimed.contains(relative.as_str()),
                "role `{key}` claims the directory holding `{relative}` but not the artifact itself; \
                 `bind_source_search_roles` refuses such a directory"
            );
        }
    }
    Ok(())
}

/// The first proven materialization walks every file and writes the proof; the next one is served on the
/// proof alone, and a manifest declaring a different file set under the same identity misses it.
/// A real native library rebuilds changed output or invalid evidence and preserves an unchanged warm result.
fn intent(project: &Path) -> Result<oven_store::OvenReceipt, Box<dyn std::error::Error>> {
    write_project(project)?;
    Ok(receipt_generated_project(
        &OvenGeneratedProjectRequest::new(
            project,
            "rustc_fixture",
            "0.1.0",
            "aarch64-apple-darwin",
            "rustc 1.96.0",
            "release",
            Vec::new(),
        )
        .with_generated_source("fixture-source", project.join("fixture.rs")),
    )?)
}

fn empty_manifest(receipt: &oven_store::OvenReceipt) -> OvenRustcArtifactManifest {
    OvenRustcArtifactManifest {
        schema_version: OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION,
        intent: receipt.intent.clone(),
        dependency_search_paths: Vec::new(),
        native_search_paths: Vec::new(),
        externs: Vec::new(),
        entrypoint_dependency_search_paths: Default::default(),
        entrypoint_externs: BTreeMap::new(),
        registry_leaves: Vec::new(),
        registry_sources: Vec::new(),
        compile_environment: BTreeMap::new(),
        vocab_auxiliary_targets: Vec::new(),
        supporting_artifacts: Vec::new(),
    }
}

/// A repeated registry source record is a duplicate declaration, and only a differing source is a second identity.
///
/// The manifest names each registry package version from one registry once. Declaring the same record twice is
/// refused as a repeat, while the same package version whose checksum, source root, or tree digest differs is
/// refused as a second source identity.
/// Build a manifest whose normal closure includes a host macro but excludes an unrelated helper cohort.
fn source_search_fixture(receipt: &oven_store::OvenReceipt) -> Result<OvenRustcArtifactManifest, OvenRustcError> {
    let mut artifacts = empty_manifest(receipt);
    artifacts.dependency_search_paths = vec!["target".to_string(), "host".to_string(), "helper".to_string()];
    artifacts.externs = [
        ("runtime", "target/libruntime.rlib", b"runtime".as_slice()),
        ("derive", "host/libderive.rlib", b"macro".as_slice()),
        ("private_helper", "helper/libprivate_helper.rlib", b"helper".as_slice()),
    ]
    .into_iter()
    .map(|(name, path, bytes)| OvenRustcArtifactExtern {
        crate_name: name.to_string(),
        relative_path: path.to_string(),
        digest: digest_bytes(bytes),
    })
    .collect();
    artifacts
        .entrypoint_externs
        .insert("generated-root".to_string(), vec!["runtime".to_string()]);
    artifacts.entrypoint_dependency_search_paths.insert(
        "generated-root".to_string(),
        artifacts.capture_source_search_closure(&["target".to_string(), "host".to_string()])?,
    );
    Ok(artifacts)
}

/// Compile a tiny publisher unit with the selected Rustc, retaining complete diagnostics on failure.
fn compile_source_search_probe(command: &mut Command) -> Result<(), Box<dyn std::error::Error>> {
    let output = command.output()?;
    if !output.status.success() {
        return Err(format!(
            "source-search publisher failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    Ok(())
}

fn rustc_path() -> Result<PathBuf, Box<dyn std::error::Error>> {
    let output = Command::new("rustup").args(["which", "rustc"]).output()?;
    if !output.status.success() {
        return Err("rustup could not locate rustc".into());
    }
    let path = String::from_utf8(output.stdout)?;
    let path = PathBuf::from(path.trim());
    if !path.is_file() {
        return Err(format!("rustup returned a non-file rustc path: {}", path.display()).into());
    }
    Ok(path)
}

fn rustc_identity(rustc: &Path) -> Result<String, Box<dyn std::error::Error>> {
    let output = Command::new(rustc).arg("--version").output()?;
    if !output.status.success() {
        return Err(format!("rustc could not report its version: {}", rustc.display()).into());
    }
    Ok(String::from_utf8(output.stdout)?.trim().to_string())
}

fn write_project(root: &Path) -> Result<(), std::io::Error> {
    fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"rustc_fixture\"\nversion = \"0.1.0\"\n",
    )?;
    fs::write(root.join("Cargo.lock"), "version = 4\n")?;
    fs::write(root.join("fixture.rs"), "pub fn fixture() {}\n")
}
