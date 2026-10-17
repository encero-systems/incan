//! Shared compiler-source identity excludes only registered canonical locks; publication recipes retain all locks.

use std::fs;
use std::path::Path;

use incan_lang::lang::standard_packages::STANDARD_PACKAGE_NAMESPACE_POLICIES;
use incan_provider::{FeatureSelection, PackageFeaturePlan};
use oven_model::manifest::ProjectManifest;
use oven_store::project_source_tree_evidence;

use super::source_identity;
use crate::build::library_project::metadata_replay::current_source_digest;

/// Materialize original registered declarations without building or substituting a native owner.
fn standard_source_fixture() -> Result<tempfile::TempDir, Box<dyn std::error::Error>> {
    let fixture = tempfile::tempdir()?;
    for policy in STANDARD_PACKAGE_NAMESPACE_POLICIES {
        let package = fixture.path().join("standard").join(policy.source_directory);
        fs::create_dir_all(package.join("src"))?;
        fs::write(package.join("loaf.toml"), policy.declaration)?;
        fs::write(package.join("src/lib.incn"), "def answer() -> int:\n    return 1\n")?;
    }
    for relative in [
        "standard/web/native",
        "standard/web/derive/incan_web_macros/src",
        "kernel/src",
    ] {
        fs::create_dir_all(fixture.path().join(relative))?;
        fs::write(fixture.path().join(relative).join("input.incn"), "original input\n")?;
    }
    Ok(fixture)
}

/// Sibling publication may create its exact canonical lock without renaming shared compiler-source authority.
#[test]
fn registered_standard_locks_do_not_change_shared_source_identity() -> Result<(), Box<dyn std::error::Error>> {
    // ---- Original source family before any package lock exists ----
    let fixture = standard_source_fixture()?;
    let standard = fixture.path().join("standard");
    let kernel = fixture.path().join("kernel");
    let original = source_identity(&standard, &kernel)?;

    // ---- Every registered canonical lock has an independent package owner ----
    for policy in STANDARD_PACKAGE_NAMESPACE_POLICIES {
        let lock = standard.join(policy.source_directory).join("oven.lock");
        fs::write(&lock, "first publication\n")?;
        assert_eq!(
            original,
            source_identity(&standard, &kernel)?,
            "registered sibling canonical lock must not rename shared source authority: {}",
            policy.source_directory,
        );
        fs::write(&lock, "later publication\n")?;
        assert_eq!(original, source_identity(&standard, &kernel)?);
    }

    // ---- Unregistered, nested, authored, native and macro inputs remain exact ----
    for relative in ["notes/oven.lock", "web/notes/oven.lock", "web/oven.lock.copy"] {
        let path = standard.join(relative);
        fs::create_dir_all(path.parent().ok_or("fixture file has no parent")?)?;
        fs::write(&path, "not a canonical package lock\n")?;
        assert_ne!(original, source_identity(&standard, &kernel)?, "{relative}");
        fs::remove_file(path)?;
    }
    for relative in [
        "src/lib.incn",
        "native/input.incn",
        "derive/incan_web_macros/src/input.incn",
    ] {
        let path = standard.join("web").join(relative);
        let bytes = fs::read(&path)?;
        let modified = fs::metadata(&path)?.modified()?;
        fs::write(&path, "changed authored bytes\n")?;
        fs::File::options().write(true).open(&path)?.set_modified(modified)?;
        assert_ne!(original, source_identity(&standard, &kernel)?, "{relative}");
        fs::write(path, bytes)?;
    }
    assert_eq!(original, source_identity(&standard, &kernel)?);
    let declaration = standard.join("web/loaf.toml");
    fs::write(
        &declaration,
        format!("{}\n# changed\n", fs::read_to_string(&declaration)?),
    )?;
    assert!(!source_identity(&standard, &kernel).is_ok_and(|current| current == original));
    Ok(())
}

/// The full selected publisher recipe still binds its own canonical lock's absence and every byte change.
#[test]
fn full_publication_source_identity_retains_own_lock() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::create_dir(root.path().join("src"))?;
    fs::write(root.path().join("loaf.toml"), "[project]\nname='lock_owner'\n")?;
    fs::write(root.path().join("src/lib.incn"), "def answer() -> int:\n    return 1\n")?;
    let project = ProjectManifest::load(&root.path().join("loaf.toml"))?;
    let features = PackageFeaturePlan::resolve(&project, &FeatureSelection::default())?;
    let absent = current_source_digest(root.path(), &features)?;
    let lock = crate::build::source_authority::canonical_baked_project_lock_path(root.path())?;
    fs::write(&lock, "first publication\n")?;
    let created = current_source_digest(root.path(), &features)?;
    assert_ne!(absent, created);
    fs::write(lock, "changed publication\n")?;
    assert_ne!(created, current_source_digest(root.path(), &features)?);
    Ok(())
}

/// Exact-coordinate projection rejects aliases that could conceal unrelated authored files.
#[test]
fn shared_source_projection_refuses_unsafe_coordinates() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = standard_source_fixture()?;
    let tree = project_source_tree_evidence(&fixture.path().join("standard"))?;
    for relative in [
        "",
        "../web/oven.lock",
        "/web/oven.lock",
        "./web/oven.lock",
        "web//oven.lock",
    ] {
        assert!(
            tree.digest_except_exact_files(&[Path::new(relative).to_path_buf()])
                .is_err(),
            "{relative}"
        );
    }
    Ok(())
}

/// Excluded canonical lock coordinates never admit a symlink to another owner's file.
#[cfg(unix)]
#[test]
fn shared_source_identity_refuses_symlink_lock() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = standard_source_fixture()?;
    let standard = fixture.path().join("standard");
    let outside = fixture.path().join("outside.lock");
    fs::write(&outside, "foreign lock\n")?;
    std::os::unix::fs::symlink(outside, standard.join("web/oven.lock"))?;
    assert!(source_identity(&standard, &fixture.path().join("kernel")).is_err());
    Ok(())
}
