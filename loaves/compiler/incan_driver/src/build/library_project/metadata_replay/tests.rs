//! Source-observation controls for the production ordinary metadata preflight (#1337/#1698).

use super::current_source_digest;
use incan_provider::{FeatureSelection, PackageFeaturePlan};
use oven_model::manifest::ProjectManifest;
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
