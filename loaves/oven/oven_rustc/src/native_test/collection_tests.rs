//! Request-owned native inventory preserves physical identity, invocation context and fresh execution.

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::{
    OvenNativeTestCollection, OvenNativeTestError, OvenNativeTestRequest, inventory_native_tests_for_collection,
    run_native_tests_exact_for_request,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// A supervised libtest protocol fixture records each listing and execution independently.
struct Fixture {
    _directory: tempfile::TempDir,
    root: PathBuf,
    executable: PathBuf,
    environment: BTreeMap<String, String>,
}

impl Fixture {
    /// Use canonical physical paths so retention depends on the same object observations as production.
    fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let root = fs::canonicalize(directory.path())?;
        let executable = root.join("harness");
        write_harness(&executable, "selected")?;
        let environment = BTreeMap::from([("FIXTURE_ROOT".to_string(), root.display().to_string())]);
        Ok(Self {
            _directory: directory,
            root,
            executable,
            environment,
        })
    }

    /// Collect names through the real supervisor before any selected case can execute.
    fn collect(&self) -> Result<OvenNativeTestCollection, OvenNativeTestError> {
        inventory_native_tests_for_collection(
            &self.executable,
            &self.environment,
            &self.root,
            Some(Duration::from_secs(5)),
        )
    }

    /// Keep normal per-case process isolation and deadlines while choosing an exact case.
    fn request(&self, name: &str) -> OvenNativeTestRequest {
        OvenNativeTestRequest {
            executable: self.executable.clone(),
            exact_names: vec![name.to_string()],
            environment: self.environment.clone(),
            timeout: Some(Duration::from_secs(5)),
        }
    }

    /// Count actual inventory child launches, including the separate ignored-case listing.
    fn listings(&self) -> Result<usize, std::io::Error> {
        Ok(fs::read(self.root.join("listings"))?.len())
    }
}

/// Speak the same private result-file protocol as libtest; stdout alone never grants a passing outcome.
fn write_harness(path: &Path, name: &str) -> Result<(), std::io::Error> {
    fs::write(
        path,
        format!(
            r#"#!/bin/sh
if [ "$1" = "--list" ]; then
  printf x >> "$FIXTURE_ROOT/listings"
  if [ "$4" != "--ignored" ]; then printf '%s\n' '{name}: test'; fi
  exit 0
fi
printf x >> "$FIXTURE_ROOT/executions"
if [ -f "$FIXTURE_ROOT/fail" ]; then
  printf '%s\n' 'failed {name} <0.000s>' > "${{11}}"
  exit 1
fi
printf '%s\n' 'ok {name} <0.000s>' > "${{11}}"
"#
        ),
    )?;
    let mut permissions = fs::metadata(path)?.permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions)
}

/// Repeated selected cases share only the collected names; their runtime outcome is always fresh.
#[test]
fn retained_inventory_avoids_relisting_and_executes_fresh_outcomes() -> TestResult {
    let fixture = Fixture::new()?;
    let collection = fixture.collect()?;
    assert_eq!(collection.inventory().names, ["selected"]);
    assert!(collection.ignored().names.is_empty());
    assert_eq!(fixture.listings()?, 2);
    let request = fixture.request("selected");
    let first = run_native_tests_exact_for_request(&request, &fixture.root, false, Some(&collection))?;
    assert!(first.success, "{}", first.output);
    fs::write(fixture.root.join("fail"), "runtime data changed")?;
    let second = run_native_tests_exact_for_request(&request, &fixture.root, false, Some(&collection))?;
    assert!(!second.success);
    assert_eq!(fixture.listings()?, 2);
    assert_eq!(fs::read(fixture.root.join("executions"))?, b"xx");
    Ok(())
}

/// A retained collection never converts a missing exact name into a zero-test success.
#[test]
fn retained_inventory_rejects_missing_case_before_execution() -> TestResult {
    let fixture = Fixture::new()?;
    let collection = fixture.collect()?;
    let result =
        run_native_tests_exact_for_request(&fixture.request("missing"), &fixture.root, false, Some(&collection));
    assert!(matches!(result, Err(OvenNativeTestError::MissingExactTest { .. })));
    assert_eq!(fixture.listings()?, 2);
    assert!(!fixture.root.join("executions").exists());
    Ok(())
}

/// Replacing the executable invalidates the collected names even when the path is identical.
#[test]
fn replaced_executable_reconciles_inventory_before_execution() -> TestResult {
    let fixture = Fixture::new()?;
    let collection = fixture.collect()?;
    let replacement = fixture.root.join("replacement");
    write_harness(&replacement, "replacement")?;
    fs::rename(replacement, &fixture.executable)?;
    let result =
        run_native_tests_exact_for_request(&fixture.request("selected"), &fixture.root, false, Some(&collection));
    assert!(matches!(result, Err(OvenNativeTestError::MissingExactTest { .. })));
    assert_eq!(fixture.listings()?, 3);
    assert!(!fixture.root.join("executions").exists());
    Ok(())
}

/// Changed explicit environment and working directory each require canonical inventory again.
#[test]
fn changed_invocation_context_recollects_inventory() -> TestResult {
    let fixture = Fixture::new()?;
    let collection = fixture.collect()?;
    let mut request = fixture.request("selected");
    request.environment.insert("NEW_CONTEXT".into(), "value".into());
    assert!(run_native_tests_exact_for_request(&request, &fixture.root, false, Some(&collection))?.success);
    assert_eq!(fixture.listings()?, 3);
    let directory = fixture.root.join("other");
    fs::create_dir(&directory)?;
    assert!(
        run_native_tests_exact_for_request(&fixture.request("selected"), &directory, false, Some(&collection))?.success
    );
    assert_eq!(fixture.listings()?, 4);
    Ok(())
}

/// Retained names do not weaken the aggregate deadline before a case can launch.
#[test]
fn retained_inventory_preserves_expired_deadline() -> TestResult {
    let fixture = Fixture::new()?;
    let collection = fixture.collect()?;
    let mut request = fixture.request("selected");
    request.timeout = Some(Duration::ZERO);
    assert!(run_native_tests_exact_for_request(&request, &fixture.root, false, Some(&collection)).is_err());
    assert_eq!(fixture.listings()?, 2);
    assert!(!fixture.root.join("executions").exists());
    Ok(())
}

/// A collection cannot retain names observed while its executable is being modified.
#[test]
fn collection_refuses_executable_changes_during_listing() -> TestResult {
    let fixture = Fixture::new()?;
    fs::write(
        &fixture.executable,
        "#!/bin/sh\nprintf '# changed\\n' >> \"$0\"\nprintf 'selected: test\\n'\n",
    )?;
    assert!(matches!(
        fixture.collect(),
        Err(OvenNativeTestError::InventoryFailed { .. })
    ));
    Ok(())
}
