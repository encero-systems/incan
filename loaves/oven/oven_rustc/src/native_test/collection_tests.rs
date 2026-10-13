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

/// Separate owner requests reuse one collection but continue to observe fresh test outcomes.
#[test]
fn owner_reconciliation_retains_names_and_executes_fresh_results() -> TestResult {
    let fixture = Fixture::new()?;
    let collect = |previous| {
        OvenNativeTestCollection::reconcile(
            previous,
            &fixture.executable,
            &fixture.environment,
            &fixture.root,
            Some(Duration::from_secs(5)),
        )
    };
    let first = collect(None)?;
    let second = collect(Some(std::sync::Arc::clone(&first)))?;
    assert!(std::sync::Arc::ptr_eq(&first, &second));
    assert_eq!(fixture.listings()?, 2);
    assert!(
        run_native_tests_exact_for_request(&fixture.request("selected"), &fixture.root, false, Some(&second))?.success
    );
    fs::write(fixture.root.join("fail"), "fresh data")?;
    let third = collect(Some(second))?;
    assert!(
        !run_native_tests_exact_for_request(&fixture.request("selected"), &fixture.root, false, Some(&third))?.success
    );
    assert_eq!(fixture.listings()?, 2);
    assert_eq!(fs::read(fixture.root.join("executions"))?, b"xx");
    Ok(())
}

/// Replacement and same-size executable edits invalidate names, even with restored modification time.
#[test]
fn owner_reconciliation_observes_executable_edits_and_replacement() -> TestResult {
    let fixture = Fixture::new()?;
    let collect = |previous| {
        OvenNativeTestCollection::reconcile(
            previous,
            &fixture.executable,
            &fixture.environment,
            &fixture.root,
            Some(Duration::from_secs(5)),
        )
    };
    let first = collect(None)?;
    let before = fs::metadata(&fixture.executable)?;
    write_harness(&fixture.executable, "modified")?;
    fs::File::options()
        .write(true)
        .open(&fixture.executable)?
        .set_times(fs::FileTimes::new().set_modified(before.modified()?))?;
    let edited = collect(Some(first))?;
    assert_eq!(edited.inventory().names, ["modified"]);
    let replacement = fixture.root.join("replacement");
    write_harness(&replacement, "replaced")?;
    fs::rename(replacement, &fixture.executable)?;
    let replaced = collect(Some(edited))?;
    assert_eq!(replaced.inventory().names, ["replaced"]);
    assert_eq!(fixture.listings()?, 6);
    Ok(())
}

/// Invocation context changes recollect both inventories, and a missing executable cannot reuse names.
#[test]
fn owner_reconciliation_observes_context_and_missing_executable() -> TestResult {
    let fixture = Fixture::new()?;
    let first =
        OvenNativeTestCollection::reconcile(None, &fixture.executable, &fixture.environment, &fixture.root, None)?;
    let mut environment = fixture.environment.clone();
    environment.insert("CHANGED_CONTEXT".into(), "yes".into());
    let second =
        OvenNativeTestCollection::reconcile(Some(first), &fixture.executable, &environment, &fixture.root, None)?;
    let other = fixture.root.join("other");
    fs::create_dir(&other)?;
    let third = OvenNativeTestCollection::reconcile(Some(second), &fixture.executable, &environment, &other, None)?;
    assert_eq!(fixture.listings()?, 6);
    fs::remove_file(&fixture.executable)?;
    assert!(OvenNativeTestCollection::reconcile(Some(third), &fixture.executable, &environment, &other, None).is_err());
    Ok(())
}

/// Independent test groups cannot borrow a nested owner that another group's cleanup will terminate.
/// Commands within one group still inherit the same identity, and retained inventory remains reusable.
#[test]
fn native_groups_scope_nested_owners_without_recollecting() -> TestResult {
    let mut fixture = Fixture::new()?;
    let harness = fs::read_to_string(&fixture.executable)?;
    let recording = r#"printf '%s\n' "$INCAN_INTERNAL_TEST_OWNER_SCOPE" >> "$FIXTURE_ROOT/scopes"
sh -c 'printf "%s\n" "$INCAN_INTERNAL_TEST_OWNER_SCOPE"' >> "$FIXTURE_ROOT/scopes"
"#;
    fs::write(
        &fixture.executable,
        harness.replace(
            "printf x >> \"$FIXTURE_ROOT/executions\"",
            &format!("{recording}printf x >> \"$FIXTURE_ROOT/executions\""),
        ),
    )?;
    fixture
        .environment
        .insert("INCAN_INTERNAL_TEST_OWNER_SCOPE".into(), "parent-scope".into());
    let collection = fixture.collect()?;
    let request = fixture.request("selected");
    for _ in 0..2 {
        let report = run_native_tests_exact_for_request(&request, &fixture.root, false, Some(&collection))?;
        assert!(report.success, "{}", report.output);
    }
    let scopes = fs::read_to_string(fixture.root.join("scopes"))?;
    let scopes = scopes.lines().collect::<Vec<_>>();
    assert_eq!(
        scopes.len(),
        4,
        "each case and its nested command must record their scope"
    );
    assert_eq!(
        scopes[0], scopes[1],
        "nested commands share their supervising case's owner"
    );
    assert_eq!(
        scopes[2], scopes[3],
        "nested commands share their supervising case's owner"
    );
    assert_ne!(
        scopes[0], scopes[2],
        "independently reaped groups cannot share an owner"
    );
    for scope in scopes {
        assert!(!scope.is_empty());
        assert_ne!(scope, "parent-scope", "nested runner must own its lifetime");
        assert!(
            !Path::new(scope).exists(),
            "completed groups release their lifetime token"
        );
    }
    assert_eq!(
        fixture.listings()?,
        2,
        "runtime lifetime does not invalidate retained inventory"
    );
    Ok(())
}
