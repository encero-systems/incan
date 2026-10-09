//! Source-observation controls for the production ordinary metadata preflight (#1337/#1698).

use super::{
    current_source_digest, metadata_output_is_observable, metadata_policy_digest, project_delivery_coordinates,
};
use crate::build::library_metadata::{LibraryMetadataRecipe, publish_library_metadata, select_library_metadata};
use incan_frontend::library_manifest::LibraryManifest;
use incan_provider::{FeatureSelection, PackageFeaturePlan};
use oven_model::manifest::ProjectManifest;
use oven_store::digest_bytes;
use oven_store::store::{OvenStore, OvenStoreLimits};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

/// Build the actual manifest feature authority used by production metadata source observation.
fn features(root: &Path) -> Result<PackageFeaturePlan, Box<dyn std::error::Error>> {
    let manifest = ProjectManifest::load(&root.join("loaf.toml"))?;
    Ok(PackageFeaturePlan::resolve(&manifest, &FeatureSelection::default())?)
}

/// Preflight observes raw bytes, including malformed Incan, without invoking frontend parsing or checking.
#[test]
fn ordinary_metadata_source_preflight_observes_raw_source_and_ignores_outputs() -> Result<(), Box<dyn std::error::Error>>
{
    for name in ["ordinary_geometry", "incan_std_core"] {
        let package = tempfile::tempdir()?;
        fs::write(
            package.path().join("loaf.toml"),
            format!("[project]\nname={name:?}\nversion='1.0.0'\n"),
        )?;
        fs::create_dir(package.path().join("src"))?;
        let source = package.path().join("src/lib.incn");
        fs::write(&source, "this is deliberately not valid Incan !!!\n")?;
        let plan = features(package.path())?;
        let initial = current_source_digest(package.path(), &plan)?;
        assert_eq!(initial, current_source_digest(package.path(), &plan)?);
        fs::create_dir_all(package.path().join("target/lib"))?;
        fs::write(package.path().join("target/lib/generated.rs"), "mutable output")?;
        assert_eq!(initial, current_source_digest(package.path(), &plan)?);
        let modified = fs::metadata(&source)?.modified()?;
        fs::write(&source, "this is deliberately not valid Incan ???\n")?;
        fs::File::options()
            .write(true)
            .open(&source)?
            .set_times(fs::FileTimes::new().set_modified(modified))?;
        assert_ne!(initial, current_source_digest(package.path(), &plan)?);
    }
    Ok(())
}

/// A sibling Rust path crate and its recursive declared leaf are source authority even outside the Incan root.
#[test]
fn ordinary_metadata_source_preflight_binds_external_rust_closure() -> Result<(), Box<dyn std::error::Error>> {
    let tree = tempfile::tempdir()?;
    let package = tree.path().join("package");
    let rust = tree.path().join("companion");
    let leaf = tree.path().join("leaf");
    for root in [&package, &rust, &leaf] {
        fs::create_dir_all(root.join("src"))?;
    }
    fs::write(
        package.join("loaf.toml"),
        "[project]\nname='ordinary'\nversion='1.0.0'\n[dependencies]\ncompanion={path='../companion',loaf='companion'}\n",
    )?;
    fs::write(
        package.join("src/lib.incn"),
        "pub def answer() -> int:\n    return 42\n",
    )?;
    fs::write(
        rust.join("loaf.toml"),
        "[project]\nname='companion'\nversion='1.0.0'\n[rust]\ntype='lib'\n[dependencies]\nleaf={path='../leaf',loaf='leaf'}\n",
    )?;
    fs::write(rust.join("src/lib.rs"), "pub fn answer() -> i64 { leaf::answer() }\n")?;
    fs::write(
        leaf.join("loaf.toml"),
        "[project]\nname='leaf'\nversion='1.0.0'\n[rust]\ntype='lib'\n",
    )?;
    let source = leaf.join("src/lib.rs");
    fs::write(&source, "pub fn answer() -> i64 { 42 }\n")?;
    let plan = features(&package)?;
    let initial = current_source_digest(&package, &plan)?;
    assert_eq!(initial, current_source_digest(&package, &plan)?);
    let modified = fs::metadata(&source)?.modified()?;
    fs::write(&source, "pub fn answer() -> i64 { 43 }\n")?;
    fs::File::options()
        .write(true)
        .open(&source)?
        .set_times(fs::FileTimes::new().set_modified(modified))?;
    let changed = current_source_digest(&package, &plan)?;
    assert_ne!(initial, changed);
    fs::write(leaf.join("src/include.txt"), "new macro input")?;
    assert_ne!(changed, current_source_digest(&package, &plan)?);
    Ok(())
}

/// New conditional declarations change authority even when a previously captured feature graph is reused.
#[test]
fn ordinary_metadata_source_preflight_binds_current_declarations() -> Result<(), Box<dyn std::error::Error>> {
    let package = tempfile::tempdir()?;
    let manifest = package.path().join("loaf.toml");
    fs::write(&manifest, "[project]\nname='ordinary'\nversion='1.0.0'\n")?;
    let plan = features(package.path())?;
    let initial = current_source_digest(package.path(), &plan)?;
    fs::write(
        &manifest,
        "[project]\nname='ordinary'\nversion='1.0.0'\n[project.features]\nextra=[]\n",
    )?;
    assert_ne!(initial, current_source_digest(package.path(), &plan)?);
    Ok(())
}

/// Delivery relationships, including renamed public edges, select distinct owners only when emitted paths differ.
#[test]
fn ordinary_metadata_delivery_projection_selects_exact_coordinate_recipe() -> Result<(), Box<dyn std::error::Error>> {
    let tree = tempfile::tempdir()?;
    let dependency = tree.path().join("dependency/target/lib");
    let first = tree.path().join("producer/target/first");
    let sibling = tree.path().join("producer/target/sibling");
    let deeper = tree.path().join("producer/target/nested/deeper");
    for root in [&dependency, &first, &sibling, &deeper] {
        fs::create_dir_all(root)?;
    }
    let roots = BTreeMap::from([("public:renamed_dependency".to_string(), dependency)]);
    let initial = project_delivery_coordinates(&first, &roots)?;
    let unchanged = project_delivery_coordinates(&sibling, &roots)?;
    let changed = project_delivery_coordinates(&deeper, &roots)?;
    assert_eq!(
        initial, unchanged,
        "a preserved relative delivery relationship can reuse"
    );
    assert_ne!(initial, changed);
    fs::write(
        tree.path().join("loaf.toml"),
        "[project]\nname='coordinates'\nversion='1.0.0'\n",
    )?;
    fs::create_dir(first.join("src"))?;
    fs::write(first.join("src/lib.rs"), "pub fn answer() -> i64 { 42 }\n")?;
    let manifest_path = first.join("coordinates.incnlib");
    LibraryManifest::new("coordinates", "1.0.0").write_to_path(&manifest_path)?;
    let recipe = LibraryMetadataRecipe {
        name: "coordinates".into(),
        version: "1.0.0".into(),
        source_digest: digest_bytes(b"source"),
        producer_digest: digest_bytes(b"producer"),
        semantic_authority_digest: digest_bytes(b"semantic"),
        dependencies: BTreeMap::new(),
        policy_digest: metadata_policy_digest(None, &initial)?,
        target: "x86_64-unknown-linux-gnu".into(),
        toolchain: "exact coordinate test compiler".into(),
        features: Vec::new(),
    };
    let store_root = tempfile::tempdir()?;
    let store = OvenStore::new(
        store_root.path(),
        OvenStoreLimits::new(16 * 1024 * 1024, 16 * 1024 * 1024, 16 * 1024 * 1024),
    );
    let receipt = recipe.receipt(tree.path())?;
    assert!(select_library_metadata(&store, &recipe, &receipt)?.is_none());
    let owner = publish_library_metadata(&store, &recipe, &receipt, &first, &manifest_path, BTreeSet::new())?;
    let mut repeat = recipe.clone();
    repeat.policy_digest = metadata_policy_digest(None, &unchanged)?;
    let selected =
        select_library_metadata(&store, &repeat, &repeat.receipt(tree.path())?)?.ok_or("preserved coordinate miss")?;
    assert_eq!(selected.reference().owner_identity, owner.reference().owner_identity);
    let mut relocated = recipe;
    relocated.policy_digest = metadata_policy_digest(None, &changed)?;
    assert!(select_library_metadata(&store, &relocated, &relocated.receipt(tree.path())?)?.is_none());
    Ok(())
}

/// Arbitrary authored-root output placement stays supported by fresh preparation instead of self-invalidating reuse.
#[test]
fn ordinary_metadata_custom_authored_output_declines_reuse() -> Result<(), Box<dyn std::error::Error>> {
    let project = tempfile::tempdir()?;
    let external = tempfile::tempdir()?;
    fs::create_dir(project.path().join("target"))?;
    assert!(metadata_output_is_observable(
        project.path(),
        &project.path().join("target/lib")
    ));
    assert!(metadata_output_is_observable(
        project.path(),
        &external.path().join("library")
    ));
    assert!(!metadata_output_is_observable(
        project.path(),
        &project.path().join("generated")
    ));
    fs::create_dir(project.path().join("generated"))?;
    assert!(!metadata_output_is_observable(
        project.path(),
        &project.path().join("generated/library")
    ));
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(project.path().join("generated"), external.path().join("alias"))?;
        assert!(!metadata_output_is_observable(
            project.path(),
            &external.path().join("alias/library")
        ));
    }
    Ok(())
}
