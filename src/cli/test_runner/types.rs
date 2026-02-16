use std::path::PathBuf;
use std::time::Duration;

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
    /// For parametrized test variants: the display ID (e.g. `"test_add[1-2-3]"`) and the Rust-syntax argument list to
    /// inject into the generated `fn main()` call.
    pub parametrize_call: Option<ParametrizeCall>,
}

/// A single parametrized test variant's call information.
#[derive(Debug, Clone)]
pub struct ParametrizeCall {
    /// Display ID shown in test output (e.g. `"test_add[1-2-3]"`).
    pub display_id: String,
    /// Rust-syntax argument expressions to pass to the test function (e.g. `"1i64, 2i64, 3i64"`).
    pub rust_args: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TestMarker {
    Skip(String),
    XFail(String),
    Slow,
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
    /// Comma-separated Rust argument expressions for the injected call (e.g. `"1, 2, 3"`).
    /// String literals are wrapped with `.to_string()`.
    pub rust_args: String,
}

/// Fixture scope determines lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FixtureScope {
    #[default]
    Function,
    Module,
    Session,
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
    pub locked: bool,
    pub frozen: bool,
    pub cargo_features: Vec<String>,
    pub cargo_no_default_features: bool,
    pub cargo_all_features: bool,
}
