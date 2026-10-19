//! The command context one `incan test` invocation shares across its execution units, and the Incan test-file path.
//!
//! Rust test roots run through `super::rust` under this context. Incan test files run on the direct route, which
//! has no test harness yet; until it does, every Incan test reports that refusal as its result.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use super::trace_test_stage;
use incan_driver::lock::OvenLockValidationRequest;
use incan_driver::lock::policy::LockPolicy;
use incan_driver::lock::resolution::validate_oven_lock_policy_with_session;
use incan_driver::testing::infer_test_project_root_without_manifest;
use incan_driver::testing::types::{TestInfo, TestResult};
use incan_driver::features::FeatureSelection;
use oven_model::lock::CargoFeatureSelection;

#[derive(Debug, Clone, Default)]
pub(super) struct TestExecutionOptions {
    pub no_capture: bool,
    pub timeout: Option<Duration>,
    pub verbose: bool,
    /// Output destination belonging to this request, including parallel workers.
    pub output: super::output::TestOutputSink,
}

/// Store and immutable source authority opened once for one `incan test` command.
pub(super) struct OvenTestCommandContext {
    /// Prepared Rust roots retain their original dependency plan leases through execution.
    pub(super) rust_roots: BTreeMap<PathBuf, super::rust::PreparedRustRoot>,
    pub(super) project_root: PathBuf,
    planned_test_files: BTreeSet<PathBuf>,
    pub(super) session: Arc<incan_driver::session::CompilationSession>,
    pub(super) store: oven_store::store::OvenStore,
}

/// Open the command's store and bind its planned test inventory before any parallel test unit starts.
pub(super) fn prepare_oven_test_command_context(
    session: Arc<incan_driver::session::CompilationSession>,
    representative_test: &Path,
    planned_test_files: &[PathBuf],
) -> crate::CliResult<Arc<OvenTestCommandContext>> {
    let project_root = absolute_project_root(
        &session
            .manifest
            .as_ref()
            .map(|manifest| manifest.project_root().to_path_buf())
            .unwrap_or_else(|| infer_test_project_root_without_manifest(representative_test)),
    );
    let stage = trace_test_stage("open_test_store");
    let store = incan_driver::oven_store::open_default_oven_store()?;
    drop(stage);
    Ok(Arc::new(OvenTestCommandContext {
        rust_roots: BTreeMap::new(),
        project_root,
        planned_test_files: planned_test_files
            .iter()
            .map(|path| canonical_path_for_cache_key(path))
            .collect(),
        session,
        store,
    }))
}

/// Return the command-owned semantic session after confirming that an execution unit came from the planned inventory.
pub(super) fn command_session_for_execution_unit<'a>(
    command_context: &'a OvenTestCommandContext,
    representative_test: &Path,
) -> Result<&'a incan_driver::session::CompilationSession, String> {
    let canonical_test = canonical_path_for_cache_key(representative_test);
    if !command_context.planned_test_files.contains(&canonical_test) {
        return Err(format!(
            "Oven test execution unit {} was not part of the command-owned collection for {}",
            canonical_test.display(),
            command_context.project_root.display(),
        ));
    }
    Ok(command_context.session.as_ref())
}

/// Validate strict Incan lock policy once before any test unit is scheduled.
///
/// `--locked` and `--frozen` are compiler-owned lock-consistency promises. Validation uses the Oven read-only
/// resolver: a missing or stale lock fails before scheduling, and a test command never publishes anything.
pub(super) fn validate_oven_test_lock_policy(
    session: &incan_driver::session::CompilationSession,
    representative_test: &Path,
    lock_policy: &LockPolicy,
    package_features: &FeatureSelection,
) -> crate::CliResult<()> {
    if !lock_policy.strict() {
        return Ok(());
    }

    let manifest = session.manifest.clone();
    let inferred_project_root = infer_test_project_root_without_manifest(representative_test);
    let project_root = manifest
        .as_ref()
        .map(|manifest| manifest.project_root().to_path_buf())
        .unwrap_or(inferred_project_root);
    let cargo_features = CargoFeatureSelection::default().normalized();
    validate_oven_lock_policy_with_session(
        OvenLockValidationRequest {
            project_root: &project_root,
            manifest: manifest.as_ref(),
            entry_file: representative_test,
            cargo_features: &cargo_features,
            lock_policy,
            package_features,
            sdk_profile_override: None,
        },
        session,
    )
}

/// Run one batch of Incan test functions.
///
/// The direct route has no test harness yet, so every test in the batch reports that refusal.
pub(super) fn run_file_tests_batch(
    tests: &[TestInfo],
    _conftest_files_by_file: &HashMap<PathBuf, Vec<PathBuf>>,
    command_context: &Arc<OvenTestCommandContext>,
    _options: TestExecutionOptions,
) -> Vec<(TestInfo, TestResult)> {
    let refusal = tests
        .first()
        .map(|test| command_session_for_execution_unit(command_context, &test.file_path))
        .and_then(Result::err)
        .unwrap_or_else(|| {
            "Incan tests do not run on the direct route yet: `incan test` gains its harness in the dev.8 fix stage (F4)"
                .to_string()
        });
    tests
        .iter()
        .map(|test| (test.clone(), TestResult::Failed(Duration::ZERO, refusal.clone())))
        .collect()
}

/// Canonicalize a path for set membership, keeping the original when it cannot be resolved.
fn canonical_path_for_cache_key(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Resolve a possibly relative project root against the current directory and canonicalize it when possible.
fn absolute_project_root(path: &Path) -> PathBuf {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else if let Ok(cwd) = std::env::current_dir() {
        cwd.join(path)
    } else {
        path.to_path_buf()
    };
    fs::canonicalize(&absolute).unwrap_or(absolute)
}
