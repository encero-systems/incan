//! Actual ordinary library publication using original command admissions (#1337/#1698).

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;
use std::sync::Arc;

use incan_frontend::library_manifest::published_layout::packaged_library_loaf_manifest_path;
use incan_provider::FeatureSelection;
use oven_rustc::rustc::{resolve_active_rustc, rustc_host_target, rustc_identity};
use oven_store::store::OvenStore;

use super::bake_admitted_library;
use crate::build::library_dependencies::PreparedLibraryDependencies;
use crate::build::library_generation::select_library_generation_reference;
use crate::build::library_metadata::{SelectedLibraryMetadata, select_library_metadata_reference};
use crate::build::library_outputs::packaged_library_loaf_store_root;
use crate::build::library_project::{
    AdmittedLibraryPreparation, ordinary_library_preparation_branches, reset_ordinary_library_preparation_branches,
};
use crate::build::{NativeSdkCommandContext, OvenPackagedLibraryLoafManifest, OvenProjectBakeReport};
use crate::lock::{project_lock_collection_counts, reset_project_lock_collection_metrics};
use crate::oven_store::open_default_oven_store;
use crate::session::CompilationSession;

/// Verify real debug/release outputs and original checked/generation owners produced by the shared finalizer.
fn published_metadata(
    root: &Path,
    report: &OvenProjectBakeReport,
) -> Result<Arc<SelectedLibraryMetadata>, Box<dyn std::error::Error>> {
    let artifact = root.join("target/lib");
    let package: OvenPackagedLibraryLoafManifest =
        serde_json::from_slice(&fs::read(packaged_library_loaf_manifest_path(&artifact))?)?;
    assert_eq!(
        package.schema_version,
        crate::build::OVEN_PACKAGED_LIBRARY_LOAF_SCHEMA_VERSION
    );
    assert_eq!(
        package.profiles.keys().map(String::as_str).collect::<BTreeSet<_>>(),
        ["debug", "release"].into()
    );
    assert_eq!(
        report
            .profiles
            .iter()
            .map(|profile| profile.profile.as_str())
            .collect::<BTreeSet<_>>(),
        ["debug", "release"].into()
    );
    assert_eq!(report.outputs.len(), 2);
    for (profile, published) in &package.profiles {
        let output = artifact.join(&published.library_relative_path);
        assert_eq!(oven_store::digest_bytes(&fs::read(&output)?), published.library_digest);
        let reported = report
            .profiles
            .iter()
            .find(|entry| entry.profile == *profile)
            .ok_or("profile missing from actual bake report")?;
        let receipt: oven_store::OvenReceipt = serde_json::from_slice(&fs::read(&reported.receipt)?)?;
        receipt.verify_identity()?;
        assert_eq!(receipt, published.receipt);
        assert!(!published.entries.is_empty());
    }
    let store = OvenStore::with_release(
        packaged_library_loaf_store_root(&artifact),
        *open_default_oven_store()?.limits(),
        &incan_oven_facet::compiler_identity(),
    );
    let metadata = select_library_metadata_reference(
        &store,
        package
            .checked_metadata
            .as_ref()
            .ok_or("missing checked metadata reference")?,
    )?;
    metadata.verify_materialization(&artifact)?;
    let generation = select_library_generation_reference(
        &store,
        package
            .checked_generation
            .as_ref()
            .ok_or("missing checked generation reference")?,
        Arc::clone(&metadata),
        &package.source_authority_digest,
        &package.metadata_files,
    )?;
    generation.verify()?;
    Ok(metadata)
}

/// Exercise the actual preparation, both native profiles, canonical writer and finalizer on fresh/replay/source edit.
/// The outer test admission is genuine; repeats pass the same original capability to every production boundary.
#[test]
#[ignore = "requires a published native SDK; exercises the real ordinary publisher"]
fn admitted_ordinary_library_publication_first_replay_and_source_edit() -> Result<(), Box<dyn std::error::Error>> {
    let native = NativeSdkCommandContext::discover()?.ok_or("missing published native command authority")?;
    let rustc = resolve_active_rustc()?;
    let limits = *open_default_oven_store()?.limits();
    let root = tempfile::tempdir()?;
    fs::create_dir_all(root.path().join("src"))?;
    fs::write(
        root.path().join("loaf.toml"),
        "[project]\nname='ordinary_publication'\nversion='1.0.0'\n",
    )?;
    let entry = root.path().join("src/lib.incn");
    fs::write(
        &entry,
        "pub def answer() -> int:\n    \"\"\"Return an ordinary checked answer.\"\"\"\n    return 42\n",
    )?;
    let dependencies = Arc::new(PreparedLibraryDependencies::admit(
        &[],
        &rustc_host_target(&rustc)?,
        &rustc_identity(&rustc)?,
        limits,
    )?);
    let features = FeatureSelection::default();
    let session = CompilationSession::discover_with_admitted_library_dependencies(&entry, &features, dependencies)?;
    let input = AdmittedLibraryPreparation::new(session, Arc::clone(&native))?;
    reset_ordinary_library_preparation_branches();
    let mut first = None;
    for iteration in 0..2 {
        reset_project_lock_collection_metrics();
        let report = bake_admitted_library(&input, &features, None)?;
        let metadata = published_metadata(root.path(), &report)?;
        assert_eq!(project_lock_collection_counts(), (1, 0));
        assert!(Arc::ptr_eq(&native, input.temporary_native_sdk_context()));
        assert_eq!(ordinary_library_preparation_branches(), (1, iteration));
        if let Some(original) = &first {
            let original: &Arc<SelectedLibraryMetadata> = original;
            assert_eq!(original.reference().owner_identity, metadata.reference().owner_identity);
        } else {
            first = Some(metadata);
        }
    }
    fs::write(
        &entry,
        "pub def answer() -> int:\n    \"\"\"Return the edited ordinary answer.\"\"\"\n    return 43\n",
    )?;
    reset_project_lock_collection_metrics();
    let edited = bake_admitted_library(&input, &features, None)?;
    let metadata = published_metadata(root.path(), &edited)?;
    assert_ne!(
        first.ok_or("missing original metadata")?.reference().owner_identity,
        metadata.reference().owner_identity
    );
    assert_eq!(ordinary_library_preparation_branches(), (2, 1));
    assert_eq!(project_lock_collection_counts(), (1, 0));
    native.verify()?;
    Ok(())
}
