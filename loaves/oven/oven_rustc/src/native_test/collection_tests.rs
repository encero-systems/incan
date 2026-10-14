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
    let first = run_native_tests_exact_for_request(&request, &fixture.root, false, Some(&collection), None)?;
    assert!(first.success, "{}", first.output);
    fs::write(fixture.root.join("fail"), "runtime data changed")?;
    let second = run_native_tests_exact_for_request(&request, &fixture.root, false, Some(&collection), None)?;
    assert!(!second.success);
    assert_eq!(fixture.listings()?, 2);
    assert_eq!(fs::read(fixture.root.join("executions"))?, b"xx");
    assert!(std::sync::Arc::ptr_eq(&first.inventory, &collection.inventory));
    assert!(std::sync::Arc::ptr_eq(&first.inventory, &second.inventory));
    assert_eq!(
        serde_json::to_value(&first)?["inventory"]["names"],
        serde_json::json!(["selected"])
    );
    Ok(())
}

/// A retained collection never converts a missing exact name into a zero-test success.
#[test]
fn retained_inventory_rejects_missing_case_before_execution() -> TestResult {
    let fixture = Fixture::new()?;
    let collection = fixture.collect()?;
    let result = run_native_tests_exact_for_request(
        &fixture.request("missing"),
        &fixture.root,
        false,
        Some(&collection),
        None,
    );
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
    let result = run_native_tests_exact_for_request(
        &fixture.request("selected"),
        &fixture.root,
        false,
        Some(&collection),
        None,
    );
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
    assert!(run_native_tests_exact_for_request(&request, &fixture.root, false, Some(&collection), None)?.success);
    assert_eq!(fixture.listings()?, 3);
    let directory = fixture.root.join("other");
    fs::create_dir(&directory)?;
    assert!(
        run_native_tests_exact_for_request(&fixture.request("selected"), &directory, false, Some(&collection), None)?
            .success
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
    assert!(run_native_tests_exact_for_request(&request, &fixture.root, false, Some(&collection), None).is_err());
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
        run_native_tests_exact_for_request(&fixture.request("selected"), &fixture.root, false, Some(&second), None)?
            .success
    );
    fs::write(fixture.root.join("fail"), "fresh data")?;
    let third = collect(Some(second))?;
    assert!(
        !run_native_tests_exact_for_request(&fixture.request("selected"), &fixture.root, false, Some(&third), None)?
            .success
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
        let report = run_native_tests_exact_for_request(&request, &fixture.root, false, Some(&collection), None)?;
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

/// Speak the existing private result protocol while accepting several exact names in a single child.
fn shared_fixture() -> Result<Fixture, Box<dyn std::error::Error>> {
    let fixture = Fixture::new()?;
    fs::write(
        &fixture.executable,
        r#"#!/bin/sh
if [ "$1" = "--list" ]; then
  printf x >> "$FIXTURE_ROOT/listings"
  if [ "$4" != "--ignored" ]; then printf '%s\n' 'alpha: test' 'beta: test' 'unselected: test'; fi
  exit 0
fi
printf x >> "$FIXTURE_ROOT/executions"
names=''
log=''
while [ "$#" -gt 0 ]; do
  case "$1" in
    alpha|beta|unselected) names="$names $1" ;;
    --logfile) shift; log="$1" ;;
    --test-threads=*) printf '%s\n' "$1" >> "$FIXTURE_ROOT/threads" ;;
  esac
  shift
done
code=0
for name in $names; do
  if [ "$name" = beta ] && [ -f "$FIXTURE_ROOT/partial" ]; then exit 1; fi
  if [ "$name" = beta ] && [ -f "$FIXTURE_ROOT/hang" ]; then sleep 60; fi
  if [ "$name" = beta ] && [ -f "$FIXTURE_ROOT/fail" ]; then
    printf 'failed %s <0.001s>\n' "$name" >> "$log"; code=1
  else
    printf 'ok %s <0.001s>\n' "$name" >> "$log"
  fi
done
exit "$code"
"#,
    )?;
    Ok(fixture)
}

/// Shared exact selections use one child, preserve the budget and execute fresh runtime outcomes.
#[test]
fn shared_selection_retains_inventory_and_executes_fresh_outcomes() -> TestResult {
    let fixture = shared_fixture()?;
    let collection = fixture.collect()?;
    let mut request = fixture.request("alpha");
    request.exact_names.push("beta".into());
    let first = run_native_tests_exact_for_request(&request, &fixture.root, false, Some(&collection), Some(2))?;
    assert!(first.success, "{}", first.output);
    assert_eq!(first.case_counts.as_ref().ok_or("missing counts")?.passed, 2);
    assert_eq!(first.case_timings.len(), 2);
    assert_eq!(fs::read(fixture.root.join("executions"))?, b"x");
    assert_eq!(fs::read_to_string(fixture.root.join("threads"))?, "--test-threads=2\n");
    fs::write(fixture.root.join("fail"), "changed runtime data")?;
    let second = run_native_tests_exact_for_request(&request, &fixture.root, false, Some(&collection), Some(2))?;
    assert!(!second.success);
    let counts = second.case_counts.as_ref().ok_or("missing failure counts")?;
    assert_eq!((counts.passed, counts.failed), (1, 1));
    assert_eq!(fs::read(fixture.root.join("executions"))?, b"xx");
    assert_eq!(fixture.listings()?, 2);
    assert!(std::sync::Arc::ptr_eq(&first.inventory, &second.inventory));
    Ok(())
}

/// Isolation remains an explicit execution policy over the same selected inventory and supervisor.
#[test]
fn shared_selection_can_preserve_per_case_isolation() -> TestResult {
    let fixture = shared_fixture()?;
    let collection = fixture.collect()?;
    let mut request = fixture.request("alpha");
    request.exact_names.push("beta".into());
    let report = run_native_tests_exact_for_request(&request, &fixture.root, false, Some(&collection), None)?;
    assert!(report.success, "{}", report.output);
    assert_eq!(fs::read(fixture.root.join("executions"))?, b"xx");
    assert_eq!(fixture.listings()?, 2);
    Ok(())
}

/// Invalid budgets or selections cannot broaden to every test or launch an unverified child.
#[test]
fn shared_selection_refuses_invalid_budget_and_names() -> TestResult {
    let fixture = shared_fixture()?;
    let collection = fixture.collect()?;
    let request = fixture.request("alpha");
    assert!(run_native_tests_exact_for_request(&request, &fixture.root, false, Some(&collection), Some(0)).is_err());
    for names in [
        Vec::new(),
        vec!["missing".to_string()],
        vec!["alpha".to_string(), "alpha".to_string()],
    ] {
        let request = OvenNativeTestRequest {
            exact_names: names,
            ..fixture.request("alpha")
        };
        assert!(
            run_native_tests_exact_for_request(&request, &fixture.root, false, Some(&collection), Some(2)).is_err()
        );
    }
    assert!(!fixture.root.join("executions").exists());
    assert_eq!(fixture.listings()?, 2);
    Ok(())
}

/// A shared child that exits before finishing cannot manufacture outcomes for unexecuted cases.
#[test]
fn shared_selection_refuses_partial_results_and_preserves_deadline() -> TestResult {
    let fixture = shared_fixture()?;
    let collection = fixture.collect()?;
    let mut request = fixture.request("alpha");
    request.exact_names.push("beta".into());
    fs::write(fixture.root.join("partial"), "stop after alpha")?;
    let partial = run_native_tests_exact_for_request(&request, &fixture.root, false, Some(&collection), Some(2))?;
    assert!(!partial.success);
    assert!(partial.case_counts.is_none());
    assert_eq!(partial.case_timings.len(), 1);
    fs::remove_file(fixture.root.join("partial"))?;
    fs::write(fixture.root.join("hang"), "wait after alpha")?;
    request.timeout = Some(Duration::from_millis(100));
    let timeout = run_native_tests_exact_for_request(&request, &fixture.root, false, Some(&collection), Some(2))?;
    assert!(!timeout.success);
    assert!(timeout.timed_out);
    assert!(timeout.case_counts.is_none());
    assert_eq!(fixture.listings()?, 2);
    Ok(())
}
