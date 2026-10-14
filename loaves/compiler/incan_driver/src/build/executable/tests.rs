//! Output receipt publication shares the existing observer and repairs changed generations independently of plans.

use std::fs;
use std::path::Path;

use oven_store::{OvenGeneratedProjectRequest, OvenReceipt, receipt_generated_project};

use super::RetainedOutputReceipt;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

/// Build a genuine generated-source receipt without invoking a compiler or importing Cargo metadata.
fn generated_receipt(root: &Path, profile: &str) -> TestResult<OvenReceipt> {
    let source = root.join("main.rs");
    receipt_generated_project(
        &OvenGeneratedProjectRequest::new(
            root,
            "receipt-fixture",
            "1.0.0",
            "test-target",
            "test-compiler",
            profile,
            Vec::new(),
        )
        .with_generated_source("compiler-main", &source),
    )
    .map_err(Into::into)
}

/// Same publication preserves file identity; source/profile changes and damaged output still publish current bytes.
#[test]
fn native_receipt_reuse_preserves_output_and_repairs_changes() -> TestResult {
    let fixture = tempfile::tempdir()?;
    let root = fixture.path().canonicalize()?;
    fs::write(root.join("main.rs"), "fn main() {}\n")?;
    let path = root.join("output.receipt.json");
    let receipt = generated_receipt(&root, "debug")?;
    let mut retained = None;
    assert!(!RetainedOutputReceipt::publish(&mut retained, &receipt, &path)?);

    // ---- Unchanged publication ----
    #[cfg(unix)]
    let original = fs::metadata(&path)?;
    assert_eq!(
        RetainedOutputReceipt::publish(&mut retained, &receipt, &path)?,
        cfg!(unix)
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let current = fs::metadata(&path)?;
        assert_eq!(current.ino(), original.ino());
        assert_eq!(current.modified()?, original.modified()?);
    }

    // ---- Profile and source identities ----
    let release = generated_receipt(&root, "release")?;
    assert!(!RetainedOutputReceipt::publish(&mut retained, &release, &path)?);
    assert_eq!(serde_json::from_slice::<OvenReceipt>(&fs::read(&path)?)?, release);
    fs::write(root.join("main.rs"), "fn main() { let _value = 42; }\n")?;
    let changed = generated_receipt(&root, "release")?;
    assert_ne!(changed, release);
    assert!(!RetainedOutputReceipt::publish(&mut retained, &changed, &path)?);

    // ---- Preserved-mtime corruption and missing receipt ----
    let stamp = fs::metadata(&path)?.modified()?;
    let mut damaged = fs::read(&path)?;
    *damaged.first_mut().ok_or("receipt is empty")? = b'!';
    fs::write(&path, damaged)?;
    fs::File::open(&path)?.set_modified(stamp)?;
    assert!(!RetainedOutputReceipt::publish(&mut retained, &changed, &path)?);
    assert_eq!(serde_json::from_slice::<OvenReceipt>(&fs::read(&path)?)?, changed);
    fs::remove_file(&path)?;
    assert!(!RetainedOutputReceipt::publish(&mut retained, &changed, &path)?);
    assert_eq!(serde_json::from_slice::<OvenReceipt>(&fs::read(&path)?)?, changed);
    Ok(())
}

/// Failed publication consumes old state and a later valid request recovers through the canonical writer.
#[test]
fn native_receipt_failed_publication_discards_previous_state() -> TestResult {
    let fixture = tempfile::tempdir()?;
    let root = fixture.path().canonicalize()?;
    fs::write(root.join("main.rs"), "fn main() {}\n")?;
    let receipt = generated_receipt(&root, "debug")?;
    let mut retained = None;
    let original = root.join("original.receipt.json");
    assert!(!RetainedOutputReceipt::publish(&mut retained, &receipt, &original)?);
    fs::write(root.join("blocked"), "not a directory")?;
    assert!(RetainedOutputReceipt::publish(&mut retained, &receipt, &root.join("blocked/receipt.json")).is_err());
    assert!(retained.is_none());
    assert!(!RetainedOutputReceipt::publish(&mut retained, &receipt, &original)?);
    assert_eq!(serde_json::from_slice::<OvenReceipt>(&fs::read(&original)?)?, receipt);
    Ok(())
}

/// Replacement and directory topology changes cannot reuse a stale observation; linked files are replaced safely.
#[cfg(unix)]
#[test]
fn native_receipt_replacement_and_links_do_not_retarget_publication() -> TestResult {
    use std::os::unix::fs::symlink;

    let fixture = tempfile::tempdir()?;
    let root = fixture.path().canonicalize()?;
    fs::write(root.join("main.rs"), "fn main() {}\n")?;
    let receipt = generated_receipt(&root, "debug")?;
    let output = root.join("output");
    let path = output.join("receipt.json");
    let mut retained = None;
    assert!(!RetainedOutputReceipt::publish(&mut retained, &receipt, &path)?);
    let bytes = fs::read(&path)?;

    // ---- Atomic file and ancestor replacement ----
    fs::write(root.join("replacement"), &bytes)?;
    fs::rename(root.join("replacement"), &path)?;
    assert!(!RetainedOutputReceipt::publish(&mut retained, &receipt, &path)?);
    fs::rename(&output, root.join("previous-output"))?;
    fs::create_dir(&output)?;
    fs::write(&path, &bytes)?;
    assert!(!RetainedOutputReceipt::publish(&mut retained, &receipt, &path)?);

    // ---- Linked destination preserves the linked target ----
    let outside = root.join("outside.json");
    fs::write(&outside, b"outside stays unchanged")?;
    fs::remove_file(&path)?;
    symlink(&outside, &path)?;
    assert!(!RetainedOutputReceipt::publish(&mut retained, &receipt, &path)?);
    assert!(!fs::symlink_metadata(&path)?.file_type().is_symlink());
    assert_eq!(fs::read(&outside)?, b"outside stays unchanged");
    assert_eq!(serde_json::from_slice::<OvenReceipt>(&fs::read(&path)?)?, receipt);
    assert!(RetainedOutputReceipt::publish(&mut retained, &receipt, &path)?);
    Ok(())
}
