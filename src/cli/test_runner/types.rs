use std::path::PathBuf;
use std::time::Duration;

use crate::cli::commands::common::CargoPolicy;

/// Output format for `incan test` results.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum TestOutputFormat {
    /// Human-oriented pytest-style console output.
    Console,
    /// JSON Lines result output with a final summary record.
    Json,
}

/// Summary of test run
pub struct TestSummary {
    pub total: usize,
    pub passed: usize,
    pub failed: usize,
    pub skipped: usize,
    pub xfailed: usize,
    pub duration: Duration,
}

/// Information about a discovered test
#[derive(Debug, Clone)]
pub struct TestInfo {
    pub file_path: PathBuf,
    pub function_name: String,
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
#[derive(Debug, Clone)]
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

#[derive(Debug, Clone, PartialEq)]
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
#[derive(Debug, Clone, PartialEq)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
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
#[derive(Debug, Clone)]
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
#[derive(Debug)]
pub enum TestResult {
    Passed(Duration),
    Failed(Duration, String),
    Skipped(String),
    XFailed(Duration, String),
    #[allow(dead_code)]
    XPassed(Duration),
}

/// Result of discovering tests and fixtures in a file.
pub struct DiscoveryResult {
    pub tests: Vec<TestInfo>,
    pub fixtures: Vec<FixtureInfo>,
    pub default_marks: Vec<String>,
    pub known_markers: Vec<String>,
}

/// Configuration for a test run, grouping the many options that `run_tests` needs.
pub struct TestRunConfig<'a> {
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
    pub jobs: usize,
    pub test_features: Vec<String>,
    pub timeout: Option<&'a str>,
    pub no_capture: bool,
    pub cargo_policy: CargoPolicy,
    pub cargo_features: Vec<String>,
    pub cargo_no_default_features: bool,
    pub cargo_all_features: bool,
}
