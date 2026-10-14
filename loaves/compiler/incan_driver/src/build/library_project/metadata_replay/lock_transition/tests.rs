//! Actual writer-proof/source policy and real checked-owner re-sealing controls.
//!
//! Native authority observation remains in production capture/finalize and the parent command gate. The real lock
//! writer fixture supplies empty resolved test requirements; it does not claim compiler/native execution equivalence.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use super::{canonical_lock_coordinate, validate_source_transition};
use crate::build::library_generation::publish_library_generation;
use crate::build::library_metadata::{LibraryMetadataRecipe, publish_library_metadata, select_library_metadata};
use crate::build::library_project::metadata_replay::current_source_snapshot;
use crate::build::source_authority::digest_baked_project_source_authority;
use crate::lock::resolution::publish_lock_evidence;
use incan_frontend::library_manifest::LibraryManifest;
use incan_provider::{FeatureSelection, PackageFeaturePlan};
use oven_model::manifest::ProjectManifest;
use oven_store::digest_bytes;
use oven_store::store::{OvenStore, OvenStoreLimits};

/// Create ordinary authored inputs and the exact current resolver graph without loading Incan source.
fn source_project(root: &Path) -> Result<PackageFeaturePlan, Box<dyn std::error::Error>> {
    fs::create_dir_all(root.join("src"))?;
    fs::write(root.join("loaf.toml"), "[project]\nname='ordinary'\nversion='1.0.0'\n")?;
    fs::write(root.join("src/lib.incn"), "pub def answer() -> int:\n    return 42\n")?;
    features(root)
}

/// Resolve current actual manifest feature inputs for each observation.
fn features(root: &Path) -> Result<PackageFeaturePlan, Box<dyn std::error::Error>> {
    let project = ProjectManifest::load(&root.join("loaf.toml"))?;
    Ok(PackageFeaturePlan::resolve(&project, &FeatureSelection::default())?)
}

/// Exact first lock publication changes raw identity, then genuine unchanged checked output gets a reusable owner.
#[test]
fn ordinary_metadata_first_lock_reseals_same_checked_output() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let plan = source_project(root.path())?;
    let original = current_source_snapshot(root.path(), &plan)?;
    let lock = canonical_lock_coordinate(root.path())?;
    assert!(!lock.exists());
    let output = root.path().join("target/lib");
    fs::create_dir_all(output.join("src"))?;
    fs::write(output.join("src/lib.rs"), "pub fn answer() -> i64 { 42 }\n")?;
    let manifest = output.join("ordinary.incnlib");
    LibraryManifest::new("ordinary", "1.0.0").write_to_path(&manifest)?;
    let store = OvenStore::new(
        root.path().join("target/owners"),
        OvenStoreLimits::new(16 * 1024 * 1024, 16 * 1024 * 1024, 16 * 1024 * 1024),
    );
    let mut recipe = LibraryMetadataRecipe {
        name: "ordinary".into(),
        version: "1.0.0".into(),
        source_digest: original.digest()?,
        producer_digest: digest_bytes(b"test checked metadata producer"),
        semantic_authority_digest: digest_bytes(b"unchanged test native/provider contract"),
        dependencies: BTreeMap::new(),
        policy_digest: digest_bytes(b"test publication policy"),
        target: "aarch64-apple-darwin".into(),
        toolchain: "test compiler".into(),
        features: Vec::new(),
    };
    let before = publish_library_metadata(
        &store,
        &recipe,
        &recipe.receipt(root.path())?,
        &output,
        &manifest,
        BTreeSet::new(),
    )?;
    let published = publish_lock_evidence(root.path())?;
    let current = current_source_snapshot(root.path(), &features(root.path())?)?;
    assert_ne!(original.digest()?, current.digest()?);
    validate_source_transition(&original, &current, &lock, &published)?;
    recipe.source_digest = current.digest()?;
    let finalized = publish_library_metadata(
        &store,
        &recipe,
        &recipe.receipt(root.path())?,
        &output,
        &manifest,
        BTreeSet::new(),
    )?;
    before.verify_same_checked_output(&finalized)?;
    assert_ne!(before.reference().owner_identity, finalized.reference().owner_identity);
    let repeat =
        select_library_metadata(&store, &recipe, &recipe.receipt(root.path())?)?.ok_or("finalized owner miss")?;
    assert_eq!(repeat.reference().owner_identity, finalized.reference().owner_identity);
    let source = digest_baked_project_source_authority(root.path())?;
    assert!(publish_library_generation(&store, root.path(), &source, &before).is_err());
    let generation = publish_library_generation(&store, root.path(), &source, &finalized)?;
    generation.verify()?;
    Ok(())
}

/// The exact writer capability does not authorize another lock, changed bytes or preserved-mtime source edits.
#[test]
fn ordinary_metadata_lock_transition_refuses_wrong_writer_and_other_source_change()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let plan = source_project(root.path())?;
    let original = current_source_snapshot(root.path(), &plan)?;
    let lock = canonical_lock_coordinate(root.path())?;
    let published = publish_lock_evidence(root.path())?;
    let current = current_source_snapshot(root.path(), &features(root.path())?)?;
    validate_source_transition(&original, &current, &lock, &published)?;
    let other = tempfile::tempdir()?;
    let wrong = publish_lock_evidence(other.path())?;
    assert!(validate_source_transition(&original, &current, &lock, &wrong).is_err());
    let source = root.path().join("src/lib.incn");
    let modified = fs::metadata(&source)?.modified()?;
    fs::write(&source, "pub def answer() -> int:\n    return 43\n")?;
    fs::File::options().write(true).open(&source)?.set_modified(modified)?;
    let changed = current_source_snapshot(root.path(), &features(root.path())?)?;
    assert!(validate_source_transition(&original, &changed, &lock, &published).is_err());
    let bytes = fs::read(&lock)?;
    let modified = fs::metadata(&lock)?.modified()?;
    let mut changed_bytes = bytes.clone();
    *changed_bytes.first_mut().ok_or("real lock empty")? ^= 1;
    fs::write(&lock, changed_bytes)?;
    fs::File::options().write(true).open(&lock)?.set_modified(modified)?;
    assert!(validate_source_transition(&original, &current, &lock, &published).is_err());
    Ok(())
}

/// A separately declared Rust source closure remains authoritative across the one permitted lock publication.
#[test]
fn ordinary_metadata_lock_transition_refuses_rust_and_feature_change() -> Result<(), Box<dyn std::error::Error>> {
    let tree = tempfile::tempdir()?;
    let root = tree.path().join("ordinary");
    let rust = tree.path().join("companion");
    fs::create_dir_all(&root)?;
    source_project(&root)?;
    fs::create_dir_all(rust.join("src"))?;
    fs::write(
        rust.join("loaf.toml"),
        "[project]\nname='companion'\nversion='1.0.0'\n[rust]\ntype='lib'\n",
    )?;
    fs::write(rust.join("src/lib.rs"), "pub fn answer() -> i64 { 42 }\n")?;
    fs::write(
        root.join("loaf.toml"),
        "[project]\nname='ordinary'\nversion='1.0.0'\n[project.features]\nextra=[]\n[dependencies]\ncompanion={path='../companion',loaf='companion'}\n",
    )?;
    let original = current_source_snapshot(&root, &features(&root)?)?;
    let lock = canonical_lock_coordinate(&root)?;
    let published = publish_lock_evidence(&root)?;
    let current = current_source_snapshot(&root, &features(&root)?)?;
    validate_source_transition(&original, &current, &lock, &published)?;
    fs::write(rust.join("src/lib.rs"), "pub fn answer() -> i64 { 43 }\n")?;
    let changed = current_source_snapshot(&root, &features(&root)?)?;
    assert!(validate_source_transition(&original, &changed, &lock, &published).is_err());
    fs::write(rust.join("src/lib.rs"), "pub fn answer() -> i64 { 42 }\n")?;
    let project = ProjectManifest::load(&root.join("loaf.toml"))?;
    let selected = PackageFeaturePlan::resolve(&project, &FeatureSelection::new(["extra"]))?;
    let changed = current_source_snapshot(&root, &selected)?;
    assert!(validate_source_transition(&original, &changed, &lock, &published).is_err());
    Ok(())
}

/// Identical public metadata does not permit replacing generated code while assigning a new source generation.
#[test]
fn ordinary_metadata_reseal_refuses_changed_generated_output() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    source_project(root.path())?;
    let output = root.path().join("target/lib");
    fs::create_dir_all(output.join("src"))?;
    fs::write(output.join("src/lib.rs"), "pub fn answer() -> i64 { 42 }\n")?;
    let manifest = output.join("ordinary.incnlib");
    LibraryManifest::new("ordinary", "1.0.0").write_to_path(&manifest)?;
    let store = OvenStore::new(
        root.path().join("target/owners"),
        OvenStoreLimits::new(16 * 1024 * 1024, 16 * 1024 * 1024, 16 * 1024 * 1024),
    );
    let mut recipe = LibraryMetadataRecipe {
        name: "ordinary".into(),
        version: "1.0.0".into(),
        source_digest: current_source_snapshot(root.path(), &features(root.path())?)?.digest()?,
        producer_digest: digest_bytes(b"test checked metadata producer"),
        semantic_authority_digest: digest_bytes(b"unchanged test native/provider contract"),
        dependencies: BTreeMap::new(),
        policy_digest: digest_bytes(b"test publication policy"),
        target: "aarch64-apple-darwin".into(),
        toolchain: "test compiler".into(),
        features: Vec::new(),
    };
    let before = publish_library_metadata(
        &store,
        &recipe,
        &recipe.receipt(root.path())?,
        &output,
        &manifest,
        BTreeSet::new(),
    )?;
    publish_lock_evidence(root.path())?;
    recipe.source_digest = current_source_snapshot(root.path(), &features(root.path())?)?.digest()?;
    fs::write(output.join("src/lib.rs"), "pub fn answer() -> i64 { 43 }\n")?;
    let changed = publish_library_metadata(
        &store,
        &recipe,
        &recipe.receipt(root.path())?,
        &output,
        &manifest,
        BTreeSet::new(),
    )?;
    assert_eq!(before.checked_files(), changed.checked_files());
    assert!(before.verify_same_checked_output(&changed).is_err());
    Ok(())
}

/// A member binds its exact external workspace lock, while unrelated sibling sources remain outside its closure.
#[test]
fn ordinary_metadata_workspace_lock_is_observed_without_sibling_sources() -> Result<(), Box<dyn std::error::Error>> {
    let workspace = tempfile::tempdir()?;
    let member = workspace.path().join("packages/member");
    fs::create_dir_all(&member)?;
    fs::write(
        workspace.path().join("loaf.toml"),
        "[project]\nname='workspace-root'\n[workspace]\nmembers=['packages/member']\n",
    )?;
    source_project(&member)?;
    let original = current_source_snapshot(&member, &features(&member)?)?;
    let lock = canonical_lock_coordinate(&member)?;
    assert_eq!(lock, fs::canonicalize(workspace.path())?.join("oven.lock"));
    assert_eq!(original.external_locks.get(&lock), Some(&None));
    let published = publish_lock_evidence(workspace.path())?;
    let current = current_source_snapshot(&member, &features(&member)?)?;
    assert_ne!(original.digest()?, current.digest()?);
    assert_eq!(
        current.external_locks.get(&lock).and_then(Option::as_deref),
        Some(published.published_content_digest()),
    );
    validate_source_transition(&original, &current, &lock, &published)?;
    let unrelated = workspace.path().join("packages/unselected/src");
    fs::create_dir_all(&unrelated)?;
    fs::write(unrelated.join("lib.incn"), "pub def ignored() -> int:\n    return 1\n")?;
    let sibling = current_source_snapshot(&member, &features(&member)?)?;
    assert_eq!(current.digest()?, sibling.digest()?);
    validate_source_transition(&original, &sibling, &lock, &published)?;
    let bytes = fs::read(&lock)?;
    let modified = fs::metadata(&lock)?.modified()?;
    let mut changed = bytes.clone();
    *changed.first_mut().ok_or("workspace lock empty")? ^= 1;
    fs::write(&lock, changed)?;
    fs::File::options().write(true).open(&lock)?.set_modified(modified)?;
    let edited = current_source_snapshot(&member, &features(&member)?)?;
    assert_ne!(current.digest()?, edited.digest()?);
    assert!(validate_source_transition(&original, &edited, &lock, &published).is_err());
    fs::write(&lock, &bytes)?;
    let restored = current_source_snapshot(&member, &features(&member)?)?;
    assert_eq!(current.digest()?, restored.digest()?);
    fs::remove_file(&lock)?;
    let absent = current_source_snapshot(&member, &features(&member)?)?;
    assert_eq!(original.digest()?, absent.digest()?);
    assert!(validate_source_transition(&original, &absent, &lock, &published).is_err());
    Ok(())
}

/// A same-byte symlink cannot substitute for the external canonical workspace lock.
#[cfg(unix)]
#[test]
fn ordinary_metadata_workspace_lock_refuses_symlink() -> Result<(), Box<dyn std::error::Error>> {
    let workspace = tempfile::tempdir()?;
    let member = workspace.path().join("member");
    fs::create_dir_all(&member)?;
    fs::write(workspace.path().join("loaf.toml"), "[workspace]\nmembers=['member']\n")?;
    source_project(&member)?;
    let published = publish_lock_evidence(workspace.path())?;
    let lock = canonical_lock_coordinate(&member)?;
    let target = workspace.path().join("same-lock-bytes");
    fs::write(&target, fs::read(&lock)?)?;
    fs::remove_file(&lock)?;
    std::os::unix::fs::symlink(&target, &lock)?;
    assert!(current_source_snapshot(&member, &features(&member)?).is_err());
    assert!(published.verify_published_file().is_err());
    Ok(())
}
