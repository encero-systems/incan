//! Native publication staging lifetime controls using actual Store publication and data-only native outputs.

use super::{Error, NativePublicationFailure, PreparedUnit, publish_staged_unit, tests};
use oven_store::store::{OvenStore, OvenStoreLimits};
use oven_store::{OvenGeneratedProjectRequest, receipt_generated_project};
use std::path::Path;

/// Construct a source-owned receipt and ordinary prepared metadata without invoking a compiler.
fn source(root: &Path) -> Result<(PreparedUnit, oven_store::OvenReceipt), Error> {
    let source = root.join("source");
    std::fs::create_dir_all(source.join("src"))?;
    std::fs::write(source.join("src/lib.rs"), "pub fn fixture() {}\n")?;
    let declaration = "[project]\nname='fixture'\nversion='1.0.0'\n[rust]\nname='fixture'\nedition='2024'\n";
    std::fs::write(source.join("loaf.toml"), declaration)?;
    let mut unit = tests::unit("fixture", "target", declaration, &[])?;
    unit.root = source;
    unit.binding.archive_digest = oven_store::digest_source_tree(&unit.root)?;
    let receipt = receipt_generated_project(
        &OvenGeneratedProjectRequest::new(
            &unit.root,
            "fixture",
            "1.0.0",
            "fixture-target",
            "fixture-toolchain",
            "debug",
            Vec::new(),
        )
        .with_generated_source("sdk-root", unit.root.join("src/lib.rs")),
    )?;
    Ok((unit, receipt))
}

/// Independent simultaneous staging coordinates disappear only after each durable publication succeeds.
#[test]
fn dev7_native_publication_success_removes_private_staging() -> Result<(), Error> {
    let root = tempfile::tempdir()?;
    let (unit, receipt) = source(root.path())?;
    let store = OvenStore::new(
        root.path().join("store"),
        OvenStoreLimits::new(1024 * 1024, 1024 * 1024, 1024 * 1024),
    );
    let first = tempfile::Builder::new()
        .prefix("native-unit-")
        .tempdir_in(root.path())?;
    let second = tempfile::Builder::new()
        .prefix("native-unit-")
        .tempdir_in(root.path())?;
    let first_path = first.path().to_path_buf();
    let second_path = second.path().to_path_buf();
    assert_ne!(first_path, second_path);
    std::fs::write(first_path.join("libfixture.rlib"), b"native data")?;
    std::fs::write(second_path.join("libfixture.rlib"), b"native data")?;
    let selected = publish_staged_unit(
        &unit,
        &store,
        &receipt,
        "fixture-native".to_string(),
        first_path.join("libfixture.rlib"),
        "libfixture.rlib",
        first,
    )?;
    assert!(!first_path.exists());
    assert!(second_path.exists());
    let repeated = publish_staged_unit(
        &unit,
        &store,
        &receipt,
        "fixture-native".to_string(),
        second_path.join("libfixture.rlib"),
        "libfixture.rlib",
        second,
    )?;
    assert!(!second_path.exists());
    assert_eq!(selected.manifest.identity, repeated.manifest.identity);
    assert_eq!(
        std::fs::read(selected.artifact_root.join("libfixture.rlib"))?,
        b"native data"
    );
    assert!(!root.path().join(receipt.identity.replace(':', "-")).exists());
    Ok(())
}

/// Failed durable publication preserves private bytes, the exact receipt and the original failure source.
#[test]
fn dev7_native_publication_failure_retains_explicit_evidence() -> Result<(), Error> {
    let root = tempfile::tempdir()?;
    let (unit, receipt) = source(root.path())?;
    let blocked = root.path().join("blocked-store");
    std::fs::write(&blocked, b"not a directory")?;
    let store = OvenStore::new(blocked, OvenStoreLimits::new(1024 * 1024, 1024 * 1024, 1024 * 1024));
    let staging = tempfile::Builder::new()
        .prefix("native-unit-")
        .tempdir_in(root.path())?;
    let path = staging.path().to_path_buf();
    std::fs::write(path.join("libfixture.rlib"), b"failed native data")?;
    let error = publish_staged_unit(
        &unit,
        &store,
        &receipt,
        "fixture-native".to_string(),
        path.join("libfixture.rlib"),
        "libfixture.rlib",
        staging,
    )
    .err()
    .ok_or("blocked Store publication succeeded")?;
    let failure = error
        .downcast_ref::<NativePublicationFailure>()
        .ok_or("publication failure lost its source wrapper")?;
    assert_eq!(failure.path, path);
    assert!(std::error::Error::source(failure).is_some());
    assert_eq!(std::fs::read(path.join("libfixture.rlib"))?, b"failed native data");
    let evidence: serde_json::Value = serde_json::from_slice(&std::fs::read(path.join("publication-failure.json"))?)?;
    assert_eq!(evidence["receipt"]["identity"], receipt.identity);
    assert_eq!(evidence["state"], "failed-durable-native-publication");
    Ok(())
}
