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

/// Existing callers and older owner messages preserve the release default.
#[test]
fn executable_profile_omission_preserves_release() -> TestResult {
    let options = crate::build::BuildCommandOptions::default();
    assert_eq!(options.executable_profile, crate::backend::project::RunProfile::Release);
    let mut wire = serde_json::to_value(&options)?;
    wire.as_object_mut()
        .ok_or("build options are not an object")?
        .remove("executable_profile");
    let decoded: crate::build::BuildCommandOptions = serde_json::from_value(wire)?;
    assert_eq!(decoded.executable_profile, crate::backend::project::RunProfile::Release);
    Ok(())
}

/// Explicit profiles survive the owner boundary without depending on the SDK selection.
#[test]
fn executable_profiles_round_trip_independently_of_sdk() -> TestResult {
    use crate::backend::project::RunProfile;
    for profile in [RunProfile::Debug, RunProfile::Release] {
        let options = crate::build::BuildCommandOptions {
            executable_profile: profile,
            sdk_profile: Some("all".to_string()),
            ..Default::default()
        };
        let wire = serde_json::to_value(&options)?;
        assert_eq!(wire["executable_profile"], profile.as_str());
        let decoded: crate::build::BuildCommandOptions = serde_json::from_value(wire)?;
        assert_eq!(decoded.executable_profile, profile);
        assert_eq!(decoded.sdk_profile.as_deref(), Some("all"));
    }
    Ok(())
}

/// Unknown or incorrectly typed profile requests fail decoding instead of silently selecting release.
#[test]
fn executable_profile_rejects_invalid_wire_values() -> TestResult {
    for invalid in [serde_json::json!("fast"), serde_json::json!(null), serde_json::json!(1)] {
        let mut wire = serde_json::to_value(crate::build::BuildCommandOptions::default())?;
        wire["executable_profile"] = invalid;
        assert!(serde_json::from_value::<crate::build::BuildCommandOptions>(wire).is_err());
    }
    Ok(())
}
