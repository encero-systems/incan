use std::path::PathBuf;
use std::time::Duration;

use incan_driver::features::FeatureSelection;

use crate::lock::policy::LockPolicy;

/// Output format for `incan test` results.
///
/// The `clap` derive rides the `cli` feature: the runner owns the choice, the command line is one way to make it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "cli", derive(clap::ValueEnum))]
pub enum TestOutputFormat {
    /// Human-oriented pytest-style console output.
    Console,
    /// JSON Lines result output with a final summary record.
    Json,
}

/// Authoritative counts and elapsed time from one fresh test run.
#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct TestSummary {
    pub total: usize,
    pub passed: usize,
    pub failed: usize,
    pub skipped: usize,
    pub xfailed: usize,
    /// Unexpected passes contribute to a failed run.
    pub xpassed: usize,
    pub duration: Duration,
}

/// Information about a discovered test
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct TestInfo {
    pub file_path: PathBuf,
    pub function_name: String,
    /// Whether the collected test function is declared with `async def`.
    pub is_async: bool,
    pub markers: Vec<TestMarker>,
    pub required_fixtures: Vec<String>,
    /// Ordered callable parameter names used to interleave parametrized arguments with fixture injections.
    pub parameter_names: Vec<String>,
    /// Effective `@timeout` marker value for this collected test case, before CLI defaults are applied.
    pub timeout: Option<Duration>,
    /// For parametrized test variants: the display ID and generated Rust argument expressions for the file harness.
    pub parametrize_call: Option<ParametrizeCall>,
}

/// A single parametrized test variant's call information.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ParametrizeCall {
    /// Display ID shown in test output (e.g. `"test_add[1-2-3]"`).
    pub display_id: String,
    /// Ordered parameter names from the `@parametrize` argnames string.
    pub argument_names: Vec<String>,
    /// Rust expressions emitted into the generated harness call for this variant.
    pub rust_arguments: Vec<String>,
    /// Per-argument display values for machine-readable reports.
    pub parameters: Vec<String>,
}

impl ParametrizeCall {
    /// Render the stored Rust argument expressions for call-site generation.
    pub fn rust_args(&self) -> String {
        self.rust_arguments.join(", ")
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum TestMarker {
    /// Explicit discovery marker from `@std.testing.test`.
    Test,
    Skip(String),
    XFail(String),
    Slow,
    Mark(String),
    Resource(String),
    Serial,
    Timeout(Duration),
    /// `@parametrize("x, y", [(1, 2), (3, 4)])`.
    ///
    /// Carries the argnames string plus one [`ParametrizeCase`] per value-tuple, holding both a display ID for test
    /// output and a Rust argument list for code generation.
    Parametrize(String, Vec<ParametrizeCase>),
}

/// One parameter set inside a `@parametrize` decorator.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ParametrizeCase {
    /// Dash-separated display ID used in test names (e.g. `"1-2-3"`).
    pub display_id: String,
    /// Rust expressions emitted when this case is expanded into a concrete test call.
    pub rust_arguments: Vec<String>,
    /// Per-argument display values included in machine-readable reports.
    pub parameters: Vec<String>,
    /// Per-case marks from `param_case(...)`.
    pub markers: Vec<TestMarker>,
}

/// Fixture scope determines lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum FixtureScope {
    #[default]
    Function,
    Module,
    Session,
}

impl FixtureScope {
    /// Return the spelling accepted by `@fixture(scope=...)`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Function => "function",
            Self::Module => "module",
            Self::Session => "session",
        }
    }
}

/// Information about a discovered fixture.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FixtureInfo {
    pub name: String,
    #[allow(dead_code)]
    pub file_path: PathBuf,
    pub scope: FixtureScope,
    pub autouse: bool,
    #[allow(dead_code)]
    pub dependencies: Vec<String>,
    pub has_teardown: bool,
    pub is_async: bool,
}

/// Result of running a single test.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub enum TestResult {
    Passed(Duration),
    Failed(Duration, String),
    Skipped(String),
    XFailed(Duration, String),
    #[allow(dead_code)]
    XPassed(Duration),
}

/// One completed case with its already resolved portable identity and original typed result.
/// Clients format these facts without consulting source files or reinterpreting execution outcomes.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct TestCaseResult {
    /// Stable root-relative identifier established by the executing request.
    pub id: String,
    /// Original collection facts, including display parameters and markers.
    pub test: TestInfo,
    /// Fresh execution result after skip and expected-failure policy.
    pub result: TestResult,
}

/// Completed test facts crossing the owner/client boundary, independent of the requested display format.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct TestRunReport {
    /// Results in selected execution/report order; passing results are never retained for another invocation.
    pub results: Vec<TestCaseResult>,
    /// Counts and duration calculated by the single runner.
    pub summary: TestSummary,
    /// Actual shuffle seed, if shuffling was requested.
    pub shuffle_seed: Option<u64>,
    /// Command outcome, including fail-on-empty policy.
    pub exit_code: i32,
}

/// Request-local test progress facts. Presentation and terminal styling belong to the invoking client.
/// Raw child output remains a separate byte-preserving stream rather than being interpreted as progress.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub enum TestProgress {
    /// Current fixture metadata requested by verbose collection, in discovery order.
    FixturesDiscovered { fixtures: Vec<FixtureInfo> },
    /// Selection completed without any cases; failure policy remains in the completion result.
    NoTestsCollected,
    /// Selected inventory and execution policy, emitted before any test body runs.
    SessionStarted {
        collected: usize,
        shuffle_seed: Option<u64>,
        jobs: usize,
    },
    /// Existing scheduler finished grouping selected cases into execution units.
    UnitsPlanned { units: usize, elapsed: Duration },
    /// One selected unit is queued for execution; filenames retain sorted source order.
    UnitStarting { files: Vec<PathBuf>, tests: usize },
    /// Fresh execution completed, before result policy and final report assembly.
    ExecutionCompleted { elapsed: Duration },
    /// Detailed timing facts for an Incan harness; no display strings are produced by the worker.
    HarnessCompleted {
        file: PathBuf,
        frontend: Duration,
        generation: Duration,
        receipt: Duration,
        selection: Duration,
        native: Duration,
        execution: Duration,
        native_reused: bool,
    },
}

/// Result of discovering tests and fixtures in a file.
pub struct DiscoveryResult {
    pub tests: Vec<TestInfo>,
    pub fixtures: Vec<FixtureInfo>,
    pub default_marks: Vec<String>,
    pub known_markers: Vec<String>,
}

/// Source languages admitted to the common test inventory and scheduler.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum TestLanguages {
    /// Collect Incan tests and preserve their fixture scopes.
    #[default]
    Incan,
    /// Collect native libtest cases from authored Rust crate roots.
    Rust,
    /// Combine both adapters in the same selection and reporting lifecycle.
    All,
}

/// Configuration for a test run, grouping the many options that `run_tests` needs.
pub struct TestRunConfig<'a> {
    /// Source adapters selected by the command; Incan's wrapper always selects only Incan.
    pub languages: TestLanguages,
    pub path: &'a str,
    pub verbose: bool,
    pub stop_on_fail: bool,
    pub include_slow: bool,
    pub filter: Option<&'a str>,
    pub use_color: bool,
    pub fail_on_empty: bool,
    pub list_only: bool,
    pub report_format: TestOutputFormat,
    pub junit_path: Option<PathBuf>,
    pub durations: Option<usize>,
    pub shuffle: bool,
    pub seed: Option<u64>,
    pub run_xfail: bool,
    pub marker_expr: Option<&'a str>,
    pub strict_markers: bool,
    /// Global native thread / execution-unit budget.
    pub jobs: usize,
    /// Preserve individual Rust test processes rather than sharing one native root.
    pub isolate: bool,
    pub test_features: Vec<String>,
    pub package_features: FeatureSelection,
    /// The build profile whose Loaf artifacts the run reads; the default profile when absent.
    pub profile: Option<String>,
    pub timeout: Option<&'a str>,
    pub no_capture: bool,
    pub lock_policy: LockPolicy,
    /// Compiler-owned workspace identity attached to every observable test result for a fan-out invocation.
    pub workspace_context: Option<WorkspaceTestContext>,
}

/// Member identity supplied by RFC 077 orchestration to a single test-batch run.
///
/// The runner does not discover workspaces itself: it only projects this context into JSON Lines test records and
/// summaries after the CLI has validated scope selection.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceTestContext {
    pub workspace_root: PathBuf,
    pub scope_origin: String,
    pub member_name: String,
    pub member_root: PathBuf,
}
