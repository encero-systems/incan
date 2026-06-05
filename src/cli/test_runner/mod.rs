//! Test runner implementation (pytest-style).

use std::collections::{BTreeSet, HashMap};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::cli::commands::common::CargoPolicy;
use crate::cli::{CliError, CliResult, ExitCode};
use crate::manifest::ProjectManifest;

mod discovery;
mod execution;
mod module_graph;
mod reporter;
mod types;

pub use discovery::{discover_test_files, discover_tests_and_fixtures};
pub(crate) use module_graph::collect_source_modules_for_test;
pub use reporter::{ConsoleReporter, TestReporter};
pub use types::{
    DiscoveryResult, FixtureInfo, FixtureScope, ParametrizeCall, ParametrizeCase, TestInfo, TestMarker,
    TestOutputFormat, TestResult, TestRunConfig, TestSummary,
};

use discovery::{
    CollectionEvalContext, discover_tests_and_fixtures_with_context, get_autouse_fixtures, parse_duration_literal,
};
use execution::{TestExecutionOptions, run_file_tests_batch};
use reporter::{print_test_result, style};

const RED: &str = "1;31";
const GREEN: &str = "1;32";
const YELLOW: &str = "33";
const BANNER_WIDTH: usize = 58;

/// Create a centered banner with a configurable fill character.
///
/// For example:
/// `centered_banner("test session starts", '_')` -> `"___________________ test session starts __________________"`
fn centered_banner(label: &str, fill: char) -> String {
    let inner_width = label.len() + 2; // spaces around label

    // If the requested width is too small, keep a readable banner shape by enforcing a minimum of 3 fill characters
    // on both sides.
    const MIN_SIDE_PADDING: usize = 3;
    let min_total_width = inner_width + (MIN_SIDE_PADDING * 2);
    let effective_width = BANNER_WIDTH.max(min_total_width);

    let pad = effective_width - inner_width; // total amount of fill chars
    let right = pad / 2;
    let right_pad = std::iter::repeat_n(fill, right).collect::<String>();
    let left = pad - right; // slight left bias for odd padding widths
    let left_pad = std::iter::repeat_n(fill, left).collect::<String>();

    format!("{left_pad} {label} {right_pad}")
}

/// Create a centered `=` banner.
fn centered_eq_banner(label: &str) -> String {
    centered_banner(label, '=')
}

/// Expand parametrized tests into individual test variants.
///
/// A test decorated with `@parametrize("x, y", [(1, 2), (3, 4)])` is expanded into two separate `TestInfo` entries with
/// `parametrize_call` set. The original entry (with the `Parametrize` marker) is removed and replaced by the expanded
/// variants.
fn expand_parametrized_tests(tests: Vec<TestInfo>) -> Vec<TestInfo> {
    let mut out = Vec::with_capacity(tests.len());

    for test in tests {
        let parametrize_markers: Vec<(String, Vec<ParametrizeCase>)> = test
            .markers
            .iter()
            .filter_map(|m| {
                if let TestMarker::Parametrize(names, cases) = m {
                    Some((names.clone(), cases.clone()))
                } else {
                    None
                }
            })
            .collect();

        if !parametrize_markers.is_empty() {
            let cases = expand_parametrize_cases(&parametrize_markers);
            let argument_names = parametrize_markers
                .iter()
                .flat_map(|(names, _)| {
                    names
                        .split(',')
                        .map(str::trim)
                        .filter(|name| !name.is_empty())
                        .map(str::to_string)
                        .collect::<Vec<_>>()
                })
                .collect::<Vec<_>>();
            for case in &cases {
                let display_id = format!("{}[{}]", test.function_name, case.display_id);
                let mut non_parametrize_markers: Vec<TestMarker> = test
                    .markers
                    .iter()
                    .filter(|m| !matches!(m, TestMarker::Parametrize(_, _)))
                    .cloned()
                    .collect();
                non_parametrize_markers.extend(case.markers.clone());
                let timeout = non_parametrize_markers.iter().rev().find_map(|marker| {
                    if let TestMarker::Timeout(duration) = marker {
                        Some(*duration)
                    } else {
                        None
                    }
                });

                out.push(TestInfo {
                    file_path: test.file_path.clone(),
                    function_name: test.function_name.clone(),
                    is_async: test.is_async,
                    markers: non_parametrize_markers,
                    required_fixtures: test.required_fixtures.clone(),
                    parameter_names: test.parameter_names.clone(),
                    timeout: timeout.or(test.timeout),
                    parametrize_call: Some(ParametrizeCall {
                        display_id,
                        argument_names: argument_names.clone(),
                        rust_arguments: case.rust_arguments.clone(),
                        parameters: case.parameters.clone(),
                    }),
                });
            }
        } else {
            out.push(test);
        }
    }

    out
}

/// Expand stacked `@parametrize` decorators into cartesian-product cases.
fn expand_parametrize_cases(markers: &[(String, Vec<ParametrizeCase>)]) -> Vec<ParametrizeCase> {
    let mut expanded = vec![ParametrizeCase {
        display_id: String::new(),
        rust_arguments: Vec::new(),
        parameters: Vec::new(),
        markers: Vec::new(),
    }];

    for (_, cases) in markers {
        let mut next = Vec::new();
        for prefix in &expanded {
            for case in cases {
                let display_id = if prefix.display_id.is_empty() {
                    case.display_id.clone()
                } else {
                    format!("{}-{}", prefix.display_id, case.display_id)
                };
                let mut rust_arguments = prefix.rust_arguments.clone();
                rust_arguments.extend(case.rust_arguments.clone());
                let mut parameters = prefix.parameters.clone();
                parameters.extend(case.parameters.clone());
                let mut markers = prefix.markers.clone();
                markers.extend(case.markers.clone());
                next.push(ParametrizeCase {
                    display_id,
                    rust_arguments,
                    parameters,
                    markers,
                });
            }
        }
        expanded = next;
    }

    expanded
}

/// Return the runner-facing case name, including parameter IDs when present.
fn test_display_name(test: &TestInfo) -> &str {
    test.parametrize_call
        .as_ref()
        .map_or_else(|| test.function_name.as_str(), |p| p.display_id.as_str())
}

/// Return the root used for stable test ids.
///
/// RFC 019 defines explicit directory arguments as the preferred stable id root. File arguments use their parent
/// directory so a direct single-file run still emits ids that are portable across machines.
fn stable_id_root(path: &Path) -> PathBuf {
    let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    if canonical.is_file() {
        canonical.parent().map_or_else(|| canonical.clone(), Path::to_path_buf)
    } else {
        canonical
    }
}

/// Discover and enforce project-level toolchain constraints for a test path, when it belongs to a project.
fn enforce_test_path_toolchain_constraint(path: &Path) -> CliResult<()> {
    let start = if path.is_file() {
        path.parent().unwrap_or_else(|| Path::new("."))
    } else {
        path
    };
    if let Some(manifest) = ProjectManifest::discover(start).map_err(|error| CliError::failure(error.to_string()))? {
        crate::cli::commands::common::enforce_project_toolchain_constraint(&manifest)?;
    }
    Ok(())
}

/// Stable test identifier used by `-k`, `--list`, and machine-readable reports.
fn stable_test_id(test: &TestInfo, root: &Path) -> String {
    let file_path = test.file_path.canonicalize().unwrap_or_else(|_| test.file_path.clone());
    let path = file_path
        .strip_prefix(root)
        .unwrap_or(file_path.as_path())
        .to_path_buf();
    format!("{}::{}", path.to_string_lossy(), test_display_name(test))
}

/// Build a best-effort nondeterministic seed for `--shuffle`.
fn default_shuffle_seed() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| {
            let nanos = duration.as_nanos();
            let lower = u64::try_from(nanos & u128::from(u64::MAX)).unwrap_or(0);
            let upper = u64::try_from(nanos >> u64::BITS).unwrap_or(0);
            lower ^ upper
        })
        .unwrap_or(0)
}

/// Advance the deterministic shuffle pseudo-random state.
fn next_shuffle_state(state: &mut u64) -> u64 {
    *state = state
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    *state
}

/// Shuffle tests reproducibly with the RFC 019 seed.
fn shuffle_tests(tests: &mut [TestInfo], seed: u64) {
    let mut state = seed;
    for i in (1..tests.len()).rev() {
        let j = (next_shuffle_state(&mut state) as usize) % (i + 1);
        tests.swap(i, j);
    }
}

/// Return the machine-readable result status string.
fn result_status(result: &TestResult) -> &'static str {
    match result {
        TestResult::Passed(_) => "passed",
        TestResult::Failed(_, _) => "failed",
        TestResult::Skipped(_) => "skipped",
        TestResult::XFailed(_, _) => "xfailed",
        TestResult::XPassed(_) => "xpassed",
    }
}

/// Return a result duration in milliseconds for reports.
fn result_duration_ms(result: &TestResult) -> u128 {
    match result {
        TestResult::Passed(duration)
        | TestResult::Failed(duration, _)
        | TestResult::XFailed(duration, _)
        | TestResult::XPassed(duration) => duration.as_millis(),
        TestResult::Skipped(_) => 0,
    }
}

/// Return marker names attached to a collected test case.
fn marker_names(test: &TestInfo) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    for marker in &test.markers {
        match marker {
            TestMarker::Test => {
                names.insert("test".to_string());
            }
            TestMarker::Skip(_) => {
                names.insert("skip".to_string());
            }
            TestMarker::XFail(_) => {
                names.insert("xfail".to_string());
            }
            TestMarker::Slow => {
                names.insert("slow".to_string());
            }
            TestMarker::Mark(name) => {
                names.insert(name.clone());
            }
            TestMarker::Resource(_) => {
                names.insert("resource".to_string());
            }
            TestMarker::Serial => {
                names.insert("serial".to_string());
            }
            TestMarker::Timeout(_) => {
                names.insert("timeout".to_string());
            }
            TestMarker::Parametrize(_, _) => {}
        }
    }
    names
}

/// Emit one JSON Lines result record for a test case.
fn emit_json_result(test: &TestInfo, result: &TestResult, root: &Path) {
    let mut record = serde_json::json!({
        "schema_version": "incan.test.v1",
        "test_id": stable_test_id(test, root),
        "file": test.file_path.to_string_lossy(),
        "name": test_display_name(test),
        "status": result_status(result),
        "duration_ms": result_duration_ms(result),
    });

    if let Some(obj) = record.as_object_mut() {
        let markers: Vec<_> = marker_names(test).into_iter().collect();
        obj.insert("markers".to_string(), serde_json::json!(markers));
        if let Some(parametrize) = &test.parametrize_call {
            obj.insert("parameters".to_string(), serde_json::json!(parametrize.parameters));
        }
        match result {
            TestResult::Failed(_, message) => {
                obj.insert("message".to_string(), serde_json::Value::String(message.clone()));
            }
            TestResult::Skipped(reason) | TestResult::XFailed(_, reason) => {
                obj.insert("reason".to_string(), serde_json::Value::String(reason.clone()));
            }
            TestResult::Passed(_) | TestResult::XPassed(_) => {}
        }
    }
    println!("{record}");
}

/// Escape text for the small JUnit XML writer.
fn xml_escape(raw: &str) -> String {
    raw.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// Write a JUnit XML report for the completed run.
fn write_junit_report(
    path: &Path,
    results: &[(TestInfo, TestResult)],
    total_time: std::time::Duration,
) -> CliResult<()> {
    let failures = results
        .iter()
        .filter(|(_, result)| matches!(result, TestResult::Failed(_, _) | TestResult::XPassed(_)))
        .count();
    let skipped = results
        .iter()
        .filter(|(_, result)| matches!(result, TestResult::Skipped(_) | TestResult::XFailed(_, _)))
        .count();
    let mut body = String::new();
    body.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    body.push_str(&format!(
        "<testsuite name=\"incan\" tests=\"{}\" failures=\"{}\" skipped=\"{}\" time=\"{:.3}\">\n",
        results.len(),
        failures,
        skipped,
        total_time.as_secs_f64()
    ));
    for (test, result) in results {
        body.push_str(&format!(
            "  <testcase classname=\"{}\" name=\"{}\" time=\"{:.3}\">",
            xml_escape(&test.file_path.to_string_lossy()),
            xml_escape(test_display_name(test)),
            result_duration_ms(result) as f64 / 1000.0
        ));
        match result {
            TestResult::Failed(_, message) => {
                body.push_str(&format!("<failure>{}</failure>", xml_escape(message)));
            }
            TestResult::XPassed(_) => {
                body.push_str("<failure>XPASS: test passed but was expected to fail</failure>");
            }
            TestResult::Skipped(reason) | TestResult::XFailed(_, reason) => {
                body.push_str(&format!("<skipped>{}</skipped>", xml_escape(reason)));
            }
            TestResult::Passed(_) => {}
        }
        body.push_str("</testcase>\n");
    }
    body.push_str("</testsuite>\n");

    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)
            .map_err(|e| CliError::failure(format!("failed to create JUnit report directory: {e}")))?;
    }
    std::fs::write(path, body).map_err(|e| CliError::failure(format!("failed to write JUnit report: {e}")))
}

/// Print the slowest collected test cases.
fn print_durations(results: &[(TestInfo, TestResult)], count: usize, use_color: bool, root: &Path) {
    if count == 0 || results.is_empty() {
        return;
    }
    let mut ordered: Vec<_> = results.iter().collect();
    ordered.sort_by_key(|(_, result)| std::cmp::Reverse(result_duration_ms(result)));

    println!();
    println!("{}", style(centered_eq_banner("slowest durations"), "1", use_color));
    for (test, result) in ordered.into_iter().take(count) {
        println!("{:>6}ms {}", result_duration_ms(result), stable_test_id(test, root));
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum MarkerToken {
    Name(String),
    And,
    Or,
    Not,
    LParen,
    RParen,
}

/// Tokenize the `-m` marker expression language.
fn tokenize_marker_expr(expr: &str) -> Result<Vec<MarkerToken>, String> {
    let mut tokens = Vec::new();
    let mut chars = expr.char_indices().peekable();
    while let Some((start, ch)) = chars.peek().copied() {
        if ch.is_whitespace() {
            let _ = chars.next();
            continue;
        }
        if ch == '(' {
            tokens.push(MarkerToken::LParen);
            let _ = chars.next();
            continue;
        }
        if ch == ')' {
            tokens.push(MarkerToken::RParen);
            let _ = chars.next();
            continue;
        }
        if ch.is_ascii_alphabetic() || ch == '_' {
            let mut end = start + ch.len_utf8();
            let mut name = String::new();
            while let Some((_, c)) = chars.peek().copied() {
                if c.is_ascii_alphanumeric() || c == '_' {
                    name.push(c);
                    end += c.len_utf8();
                    let _ = chars.next();
                } else {
                    break;
                }
            }
            match name.as_str() {
                "and" => tokens.push(MarkerToken::And),
                "or" => tokens.push(MarkerToken::Or),
                "not" => tokens.push(MarkerToken::Not),
                _ => tokens.push(MarkerToken::Name(name)),
            }
            let _ = end;
            continue;
        }
        return Err(format!("invalid marker expression character `{ch}`"));
    }
    Ok(tokens)
}

struct MarkerExprParser<'a> {
    tokens: &'a [MarkerToken],
    pos: usize,
    names: &'a BTreeSet<String>,
}

impl<'a> MarkerExprParser<'a> {
    /// Parse and evaluate a marker expression against one test's marker names.
    fn parse(tokens: &'a [MarkerToken], names: &'a BTreeSet<String>) -> Result<bool, String> {
        let mut parser = Self { tokens, pos: 0, names };
        let value = parser.parse_or()?;
        if parser.pos != tokens.len() {
            return Err("unexpected trailing marker expression token".to_string());
        }
        Ok(value)
    }

    /// Parse an `or` expression.
    fn parse_or(&mut self) -> Result<bool, String> {
        let mut value = self.parse_and()?;
        while self.match_token(&MarkerToken::Or) {
            let rhs = self.parse_and()?;
            value = value || rhs;
        }
        Ok(value)
    }

    /// Parse an `and` expression.
    fn parse_and(&mut self) -> Result<bool, String> {
        let mut value = self.parse_not()?;
        while self.match_token(&MarkerToken::And) {
            let rhs = self.parse_not()?;
            value = value && rhs;
        }
        Ok(value)
    }

    /// Parse a `not` expression.
    fn parse_not(&mut self) -> Result<bool, String> {
        if self.match_token(&MarkerToken::Not) {
            Ok(!self.parse_not()?)
        } else {
            self.parse_primary()
        }
    }

    /// Parse a marker name or parenthesized expression.
    fn parse_primary(&mut self) -> Result<bool, String> {
        match self.tokens.get(self.pos) {
            Some(MarkerToken::Name(name)) => {
                self.pos += 1;
                Ok(self.names.contains(name))
            }
            Some(MarkerToken::LParen) => {
                self.pos += 1;
                let value = self.parse_or()?;
                if !self.match_token(&MarkerToken::RParen) {
                    return Err("unclosed marker expression parenthesis".to_string());
                }
                Ok(value)
            }
            _ => Err("expected marker name or parenthesized expression".to_string()),
        }
    }

    /// Consume the expected token when it is next in the stream.
    fn match_token(&mut self, expected: &MarkerToken) -> bool {
        if self.tokens.get(self.pos) == Some(expected) {
            self.pos += 1;
            true
        } else {
            false
        }
    }
}

/// Return all marker names referenced by a tokenized expression.
fn marker_expr_names(tokens: &[MarkerToken]) -> BTreeSet<String> {
    tokens
        .iter()
        .filter_map(|token| {
            if let MarkerToken::Name(name) = token {
                Some(name.clone())
            } else {
                None
            }
        })
        .collect()
}

/// Validate the runner's snake_case marker-name contract.
fn marker_name_is_valid(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first.is_ascii_lowercase() || first == '_')
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// Evaluate a marker expression for one collected test case.
fn marker_expr_matches(test: &TestInfo, tokens: &[MarkerToken]) -> Result<bool, String> {
    MarkerExprParser::parse(tokens, &marker_names(test))
}

/// Return `conftest.incn` files that apply to a conventional test file.
fn applicable_conftest_files(test_file: &Path, root: &Path) -> Vec<PathBuf> {
    if !test_file
        .ancestors()
        .any(|path| path.file_name().and_then(|name| name.to_str()) == Some("tests"))
    {
        return Vec::new();
    }

    let stop = if root.is_file() {
        root.parent().map(Path::to_path_buf)
    } else {
        Some(root.to_path_buf())
    };
    let stop = stop.and_then(|p| p.canonicalize().ok()).unwrap_or_else(|| {
        if root.is_file() {
            root.parent().map_or_else(|| root.to_path_buf(), Path::to_path_buf)
        } else {
            root.to_path_buf()
        }
    });

    let mut files = Vec::new();
    let mut cursor = test_file.parent().map(Path::to_path_buf);
    while let Some(dir) = cursor {
        let candidate = dir.join("conftest.incn");
        if candidate.is_file() {
            files.push(candidate);
        }
        let canonical = dir.canonicalize().unwrap_or_else(|_| dir.clone());
        if canonical == stop {
            break;
        }
        cursor = dir.parent().map(Path::to_path_buf);
    }
    files.reverse();
    files
}

/// Return marker names that do not need user registration.
fn builtin_marker_names() -> BTreeSet<String> {
    [
        "test", "skip", "skipif", "xfail", "xfailif", "slow", "resource", "serial", "timeout",
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}

/// Validate marker names and strict-marker registry membership.
fn validate_markers(
    tests: &[TestInfo],
    known_markers: &BTreeSet<String>,
    strict_markers: bool,
    marker_expr_names: &BTreeSet<String>,
) -> Result<(), String> {
    let builtins = builtin_marker_names();
    for test in tests {
        for name in marker_names(test) {
            if builtins.contains(&name) {
                continue;
            }
            if !marker_name_is_valid(&name) {
                return Err(format!("invalid marker name `{name}`; marker names must be snake_case"));
            }
            if strict_markers && !known_markers.contains(&name) {
                return Err(format!(
                    "unknown marker `{name}` with --strict-markers; register it in TEST_MARKERS"
                ));
            }
        }
    }
    for name in marker_expr_names {
        if builtins.contains(name) {
            continue;
        }
        if !marker_name_is_valid(name) {
            return Err(format!(
                "invalid marker expression name `{name}`; marker names must be snake_case"
            ));
        }
        if strict_markers && !known_markers.contains(name) {
            return Err(format!(
                "unknown marker `{name}` in -m expression with --strict-markers; register it in TEST_MARKERS"
            ));
        }
    }
    Ok(())
}

/// Return the timeout marker or run default used for the generated Cargo batch.
fn effective_timeout(test: &TestInfo, default_timeout: Option<Duration>) -> Option<Duration> {
    test.timeout.or(default_timeout)
}

#[derive(Debug)]
struct ExecutionUnit {
    index: usize,
    file_paths: BTreeSet<PathBuf>,
    tests: Vec<TestInfo>,
    conftest_files_by_file: HashMap<PathBuf, Vec<PathBuf>>,
    timeout: Option<Duration>,
    resources: BTreeSet<String>,
    serial: bool,
}

#[derive(Debug)]
struct ActiveUnit {
    index: usize,
    file_paths: BTreeSet<PathBuf>,
    resources: BTreeSet<String>,
    serial: bool,
}

/// Run one planned execution unit through the existing generated Cargo harness path.
#[allow(clippy::too_many_arguments)]
fn run_execution_unit(
    unit: &ExecutionUnit,
    prep_cache: &mut execution::TestPrepCache,
    cargo_policy: &CargoPolicy,
    cargo_features: &[String],
    cargo_no_default_features: bool,
    cargo_all_features: bool,
    no_capture: bool,
    verbose: bool,
    emit_progress: bool,
    jobs: usize,
) -> Vec<(TestInfo, TestResult)> {
    run_file_tests_batch(
        &unit.tests,
        &unit.conftest_files_by_file,
        prep_cache,
        cargo_policy,
        cargo_features,
        cargo_no_default_features,
        cargo_all_features,
        TestExecutionOptions {
            no_capture,
            timeout: unit.timeout,
            jobs,
            verbose,
            emit_progress,
        },
    )
}

/// Return all resource keys declared on a collected test.
fn test_resources(test: &TestInfo) -> BTreeSet<String> {
    test.markers
        .iter()
        .filter_map(|marker| {
            if let TestMarker::Resource(resource) = marker {
                Some(resource.clone())
            } else {
                None
            }
        })
        .collect()
}

/// Return whether a collected test requests exclusive serial scheduling.
fn test_is_serial(test: &TestInfo) -> bool {
    test.markers.iter().any(|marker| matches!(marker, TestMarker::Serial))
}

/// Return whether a test can share a generated Cargo harness batch with an existing unit.
fn unit_can_include(
    unit: &ExecutionUnit,
    test: &TestInfo,
    root: &Path,
    default_timeout: Option<Duration>,
    allow_cross_file_batches: bool,
) -> bool {
    !unit.serial
        && !test_is_serial(test)
        && (allow_cross_file_batches || unit.file_paths.contains(&test.file_path))
        && unit.timeout == effective_timeout(test, default_timeout)
        && unit.resources == test_resources(test)
        && unit
            .conftest_files_by_file
            .values()
            .next()
            .is_some_and(|conftests| conftests == &applicable_conftest_files(&test.file_path, root))
}

/// Convert collected tests into generated worker-batch Cargo harness execution units.
///
/// Conventional test files and inline `module tests:` files both end up here after discovery. `conftest.incn`
/// inheritance has already been resolved, so units carry the exact conftest chain they need for execution.
fn build_execution_units(
    tests: &[TestInfo],
    root: &Path,
    default_timeout: Option<Duration>,
    stop_on_fail: bool,
    jobs: usize,
) -> Vec<ExecutionUnit> {
    let allow_cross_file_batches = jobs <= 1;
    let mut units = Vec::new();
    let mut idx = 0usize;
    while idx < tests.len() {
        let first = &tests[idx];
        if first.markers.iter().any(|marker| matches!(marker, TestMarker::Skip(_))) {
            idx += 1;
            continue;
        }

        let resources = test_resources(first);
        let serial = test_is_serial(first);
        let timeout = effective_timeout(first, default_timeout);
        let file_path = first.file_path.clone();
        let conftest_files = applicable_conftest_files(&file_path, root);
        let mut batch = vec![first.clone()];
        idx += 1;
        while idx < tests.len() {
            if stop_on_fail {
                break;
            }
            let candidate = &tests[idx];
            if candidate
                .markers
                .iter()
                .any(|marker| matches!(marker, TestMarker::Skip(_)))
            {
                break;
            }
            let provisional = ExecutionUnit {
                index: units.len(),
                file_paths: BTreeSet::from([file_path.clone()]),
                tests: Vec::new(),
                conftest_files_by_file: HashMap::from([(file_path.clone(), conftest_files.clone())]),
                timeout,
                resources: resources.clone(),
                serial,
            };
            if !unit_can_include(&provisional, candidate, root, default_timeout, allow_cross_file_batches) {
                break;
            }
            batch.push(candidate.clone());
            idx += 1;
        }
        let mut file_paths = BTreeSet::new();
        let mut conftest_files_by_file = HashMap::new();
        for test in &batch {
            if file_paths.insert(test.file_path.clone()) {
                conftest_files_by_file.insert(test.file_path.clone(), applicable_conftest_files(&test.file_path, root));
            }
        }

        units.push(ExecutionUnit {
            index: units.len(),
            file_paths,
            tests: batch,
            conftest_files_by_file,
            timeout,
            resources,
            serial,
        });
    }
    if jobs > 1 && !stop_on_fail {
        coalesce_worker_batches(units, jobs)
    } else {
        units
    }
}

/// Return whether two planned units can share one generated worker-batch harness.
fn units_share_worker_profile(left: &ExecutionUnit, right: &ExecutionUnit) -> bool {
    !left.serial
        && !right.serial
        && left.timeout == right.timeout
        && left.resources == right.resources
        && left.conftest_files_by_file.values().next() == right.conftest_files_by_file.values().next()
}

/// Merge a compatible per-file unit into an existing worker-batch unit.
fn merge_execution_unit(target: &mut ExecutionUnit, source: ExecutionUnit) {
    target.file_paths.extend(source.file_paths);
    target.tests.extend(source.tests);
    target.conftest_files_by_file.extend(source.conftest_files_by_file);
}

/// Coalesce per-file units into a bounded set of worker batches for `--jobs N`.
///
/// Each batch still compiles to one Cargo/libtest harness. Grouping by identical conftest and scheduling profile keeps
/// session fixtures process-local to a worker while preserving resource and serial constraints at scheduler level.
fn coalesce_worker_batches(units: Vec<ExecutionUnit>, jobs: usize) -> Vec<ExecutionUnit> {
    let mut batches: Vec<ExecutionUnit> = Vec::new();
    for unit in units {
        if unit.serial {
            batches.push(unit);
            continue;
        }

        let matching = batches
            .iter()
            .enumerate()
            .filter(|(_, batch)| units_share_worker_profile(batch, &unit))
            .collect::<Vec<_>>();
        let target_index = if matching.len() >= jobs {
            matching
                .into_iter()
                .min_by_key(|(_, batch)| batch.tests.len())
                .map(|(index, _)| index)
        } else {
            None
        };

        if let Some(index) = target_index {
            merge_execution_unit(&mut batches[index], unit);
        } else {
            batches.push(unit);
        }
    }

    for (index, batch) in batches.iter_mut().enumerate() {
        batch.index = index;
    }
    batches
}

/// Return whether a candidate unit may start alongside the currently active units.
fn active_units_allow(active: &[ActiveUnit], candidate: &ExecutionUnit) -> bool {
    if active
        .iter()
        .any(|unit| !unit.file_paths.is_disjoint(&candidate.file_paths))
    {
        return false;
    }
    if candidate.serial {
        return active.is_empty();
    }
    if active.iter().any(|unit| unit.serial) {
        return false;
    }
    active
        .iter()
        .all(|unit| unit.resources.is_disjoint(&candidate.resources))
}

/// Return whether a completed raw batch result should trigger fail-fast scheduling.
fn batch_has_failure(results: &[(TestInfo, TestResult)]) -> bool {
    results
        .iter()
        .any(|(_, result)| matches!(result, TestResult::Failed(_, _) | TestResult::XPassed(_)))
}

/// Run planned execution units with a small resource-aware worker scheduler.
#[allow(clippy::too_many_arguments)]
fn run_scheduled_execution_units(
    units: Vec<ExecutionUnit>,
    jobs: usize,
    cargo_policy: CargoPolicy,
    cargo_features: &[String],
    cargo_no_default_features: bool,
    cargo_all_features: bool,
    stop_on_fail: bool,
    no_capture: bool,
    verbose: bool,
    emit_progress: bool,
) -> Vec<(usize, Vec<(TestInfo, TestResult)>)> {
    if jobs <= 1 {
        let mut prep_cache = execution::TestPrepCache::default();
        let mut completed = Vec::new();
        for unit in &units {
            let results = run_execution_unit(
                unit,
                &mut prep_cache,
                &cargo_policy,
                cargo_features,
                cargo_no_default_features,
                cargo_all_features,
                no_capture,
                verbose,
                emit_progress,
                jobs,
            );
            let failed = batch_has_failure(&results);
            completed.push((unit.index, results));
            if stop_on_fail && failed {
                break;
            }
        }
        return completed;
    }

    let total = units.len();
    let mut pending: Vec<Option<ExecutionUnit>> = units.into_iter().map(Some).collect();
    let (sender, receiver) = mpsc::channel();
    let mut active = Vec::new();
    let mut completed = Vec::new();
    let mut launched = 0usize;
    let mut finished = 0usize;
    let mut stop_launching = false;

    while finished < total {
        while !stop_launching && active.len() < jobs {
            let next_index = pending.iter().position(|unit| {
                unit.as_ref()
                    .is_some_and(|candidate| active_units_allow(&active, candidate))
            });
            let Some(next_index) = next_index else {
                break;
            };
            let Some(unit) = pending[next_index].take() else {
                break;
            };
            let sender = sender.clone();
            let cargo_features = cargo_features.to_vec();
            let cargo_policy = cargo_policy.clone();
            active.push(ActiveUnit {
                index: unit.index,
                file_paths: unit.file_paths.clone(),
                resources: unit.resources.clone(),
                serial: unit.serial,
            });
            launched += 1;
            thread::spawn(move || {
                let mut prep_cache = execution::TestPrepCache::default();
                let unit_index = unit.index;
                let results = run_execution_unit(
                    &unit,
                    &mut prep_cache,
                    &cargo_policy,
                    &cargo_features,
                    cargo_no_default_features,
                    cargo_all_features,
                    no_capture,
                    verbose,
                    emit_progress,
                    jobs,
                );
                let _ = sender.send((unit_index, results));
            });
        }

        if active.is_empty() && launched == finished {
            break;
        }

        let Ok((unit_index, results)) = receiver.recv() else {
            break;
        };
        finished += 1;
        active.retain(|unit| unit.index != unit_index);
        if stop_on_fail && batch_has_failure(&results) {
            stop_launching = true;
        }
        completed.push((unit_index, results));
    }

    completed
}

/// Run all tests in the given path.
pub fn run_tests(config: TestRunConfig<'_>) -> CliResult<ExitCode> {
    let TestRunConfig {
        path,
        verbose,
        stop_on_fail,
        include_slow,
        filter,
        use_color,
        fail_on_empty,
        list_only,
        report_format,
        junit_path,
        durations,
        shuffle,
        seed,
        run_xfail,
        marker_expr,
        strict_markers,
        jobs,
        test_features,
        timeout,
        no_capture,
        cargo_policy,
        cargo_features,
        cargo_no_default_features,
        cargo_all_features,
    } = config;

    let start_time = Instant::now();
    let jobs = jobs.max(1);

    let path = Path::new(path);
    enforce_test_path_toolchain_constraint(path)?;
    let stable_id_root = stable_id_root(path);
    let test_files = discover_test_files(path);
    let eval_context = CollectionEvalContext::new(test_features.into_iter().collect());

    if test_files.is_empty() {
        return Err(CliError::failure(format!(
            "No test files found in '{}'\nTest files should be named test_*.incn or *_test.incn",
            path.display()
        )));
    }

    let mut all_tests: Vec<TestInfo> = Vec::new();
    let mut all_fixtures: HashMap<String, FixtureInfo> = HashMap::new();
    let mut known_markers = builtin_marker_names();
    let mut collection_errors = Vec::new();

    for file_path in &test_files {
        let mut visible_fixture_names = Vec::new();
        let mut inherited_marks = Vec::new();
        let mut inherited_known_markers = Vec::new();
        let mut applicable_fixtures = HashMap::new();

        for conftest in applicable_conftest_files(file_path, path) {
            match discover_tests_and_fixtures_with_context(
                &conftest,
                &visible_fixture_names,
                &inherited_marks,
                &inherited_known_markers,
                &eval_context,
            ) {
                Ok(result) => {
                    inherited_marks = result.default_marks.clone();
                    inherited_known_markers = result.known_markers.clone();
                    for marker in result.known_markers {
                        known_markers.insert(marker);
                    }
                    for fixture in result.fixtures {
                        visible_fixture_names.push(fixture.name.clone());
                        applicable_fixtures.insert(fixture.name.clone(), fixture.clone());
                        all_fixtures.insert(fixture.name.clone(), fixture);
                    }
                }
                Err(e) => collection_errors.push(format!("Error parsing {}: {}", conftest.display(), e)),
            }
        }

        match discover_tests_and_fixtures_with_context(
            file_path,
            &visible_fixture_names,
            &inherited_marks,
            &inherited_known_markers,
            &eval_context,
        ) {
            Ok(result) => {
                for marker in result.known_markers {
                    known_markers.insert(marker);
                }
                for fixture in result.fixtures {
                    applicable_fixtures.insert(fixture.name.clone(), fixture.clone());
                    all_fixtures.insert(fixture.name.clone(), fixture);
                }
                let mut autouse_fixtures = get_autouse_fixtures(&applicable_fixtures, FixtureScope::Session);
                autouse_fixtures.extend(get_autouse_fixtures(&applicable_fixtures, FixtureScope::Module));
                autouse_fixtures.extend(get_autouse_fixtures(&applicable_fixtures, FixtureScope::Function));
                all_tests.extend(result.tests.into_iter().map(|mut test| {
                    for autouse in &autouse_fixtures {
                        if !test.required_fixtures.contains(autouse) {
                            test.required_fixtures.push(autouse.clone());
                        }
                    }
                    test
                }));
            }
            Err(e) => {
                collection_errors.push(format!("Error parsing {}: {}", file_path.display(), e));
            }
        }
    }

    if !collection_errors.is_empty() {
        return Err(CliError::failure(collection_errors.join("\n")));
    }

    if verbose && !all_fixtures.is_empty() {
        println!("Discovered {} fixture(s):", all_fixtures.len());
        for (name, fixture) in &all_fixtures {
            let scope_str = match fixture.scope {
                FixtureScope::Function => "function",
                FixtureScope::Module => "module",
                FixtureScope::Session => "session",
            };
            let autouse_str = if fixture.autouse { " (autouse)" } else { "" };
            let async_str = if fixture.is_async { " async" } else { "" };
            let teardown_str = if fixture.has_teardown { " (with teardown)" } else { "" };
            println!(
                "  - {}: scope={}{}{}{}",
                name, scope_str, autouse_str, async_str, teardown_str
            );
        }
        println!();
    }

    // ---- Expand parametrized tests into individual variants ----
    let all_tests = expand_parametrized_tests(all_tests);

    let default_timeout = match timeout {
        Some(raw) => Some(parse_duration_literal(raw).ok_or_else(|| {
            CliError::failure(format!(
                "invalid --timeout value `{raw}`; use a duration like 250ms, 5s, or 2m"
            ))
        })?),
        None => None,
    };
    let marker_tokens = match marker_expr {
        Some(expr) => {
            let tokens = tokenize_marker_expr(expr).map_err(CliError::failure)?;
            let _ = MarkerExprParser::parse(&tokens, &BTreeSet::new()).map_err(CliError::failure)?;
            Some(tokens)
        }
        None => None,
    };
    let marker_expr_names = marker_tokens
        .as_ref()
        .map_or_else(BTreeSet::new, |tokens| marker_expr_names(tokens));
    validate_markers(&all_tests, &known_markers, strict_markers, &marker_expr_names).map_err(CliError::failure)?;

    let mut filtered_tests: Vec<TestInfo> = all_tests
        .into_iter()
        .filter(|t| {
            if let Some(keyword) = filter
                && !stable_test_id(t, &stable_id_root).contains(keyword)
            {
                return false;
            }
            if !include_slow && t.markers.contains(&TestMarker::Slow) {
                return false;
            }
            if let Some(tokens) = marker_tokens.as_ref() {
                match marker_expr_matches(t, tokens) {
                    Ok(true) => {}
                    Ok(false) | Err(_) => return false,
                }
            }
            true
        })
        .collect();

    if filtered_tests.is_empty() {
        eprintln!("No tests collected");
        if fail_on_empty {
            return Err(CliError::new("", ExitCode::FAILURE));
        }
        return Ok(ExitCode::SUCCESS); // "no tests collected" is not a failure
    }

    if list_only {
        for test in &filtered_tests {
            println!("{}", stable_test_id(test, &stable_id_root));
        }
        return Ok(ExitCode::SUCCESS);
    }

    let shuffle_seed = if shuffle {
        let value = seed.unwrap_or_else(default_shuffle_seed);
        shuffle_tests(&mut filtered_tests, value);
        Some(value)
    } else {
        None
    };

    if report_format == TestOutputFormat::Console {
        println!("{}", style(centered_eq_banner("test session starts"), "1", use_color));
        println!("collected {} item(s)", filtered_tests.len());
        if let Some(seed) = shuffle_seed {
            println!("shuffle seed: {seed}");
        }
        if jobs > 1 {
            println!("jobs: {jobs}");
        }
        println!();
    }

    let mut results: Vec<(TestInfo, TestResult)> = Vec::new();
    let mut passed = 0;
    let mut failed = 0;
    let mut skipped = 0;
    let mut xfailed = 0;
    let mut xpassed = 0;

    let planning_start = Instant::now();
    let units = build_execution_units(&filtered_tests, path, default_timeout, stop_on_fail, jobs);
    if report_format == TestOutputFormat::Console && verbose {
        println!(
            "planned {} generated harness unit(s) in {:.2}s",
            units.len(),
            planning_start.elapsed().as_secs_f64()
        );
    }
    if report_format == TestOutputFormat::Console && !stop_on_fail {
        for unit in &units {
            let files = unit
                .file_paths
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join(", ");
            println!(
                "{}",
                style(
                    format!("running {} ({} item(s))", files, unit.tests.len()),
                    "2",
                    use_color
                )
            );
        }
        let _ = std::io::stdout().flush();
    }

    let mut raw_batch_results = run_scheduled_execution_units(
        units,
        jobs,
        cargo_policy,
        &cargo_features,
        cargo_no_default_features,
        cargo_all_features,
        stop_on_fail,
        no_capture,
        verbose,
        report_format == TestOutputFormat::Console,
    );
    if report_format == TestOutputFormat::Console && verbose {
        println!(
            "generated harness execution phase completed in {:.2}s",
            start_time.elapsed().as_secs_f64()
        );
    }
    raw_batch_results.sort_by_key(|(index, _)| *index);
    let mut raw_results_by_id: HashMap<String, TestResult> = HashMap::new();
    for (_, batch_results) in raw_batch_results {
        for (test, result) in batch_results {
            raw_results_by_id.insert(stable_test_id(&test, &stable_id_root), result);
        }
    }

    for test in filtered_tests {
        let raw_result = if let Some(TestMarker::Skip(reason)) =
            test.markers.iter().find(|marker| matches!(marker, TestMarker::Skip(_)))
        {
            TestResult::Skipped(reason.clone())
        } else {
            let id = stable_test_id(&test, &stable_id_root);
            match raw_results_by_id.remove(&id) {
                Some(result) => result,
                None if stop_on_fail => continue,
                None => TestResult::Failed(
                    Duration::from_secs(0),
                    format!("internal error: no test result was produced for `{id}`"),
                ),
            }
        };

        let is_xfail = !run_xfail && test.markers.iter().any(|marker| matches!(marker, TestMarker::XFail(_)));
        let result = if is_xfail {
            match raw_result {
                TestResult::Passed(d) => {
                    xpassed += 1;
                    TestResult::XPassed(d)
                }
                TestResult::Failed(d, _) => {
                    let reason = test
                        .markers
                        .iter()
                        .find_map(|marker| {
                            if let TestMarker::XFail(reason) = marker {
                                Some(reason.clone())
                            } else {
                                None
                            }
                        })
                        .unwrap_or_default();
                    xfailed += 1;
                    TestResult::XFailed(d, reason)
                }
                other => other,
            }
        } else {
            match &raw_result {
                TestResult::Passed(_) => passed += 1,
                TestResult::Failed(_, _) => failed += 1,
                TestResult::Skipped(_) => skipped += 1,
                TestResult::XFailed(_, _) => xfailed += 1,
                TestResult::XPassed(_) => xpassed += 1,
            }
            raw_result
        };

        if report_format == TestOutputFormat::Console {
            print_test_result(&test, &result, verbose, use_color);
        } else {
            emit_json_result(&test, &result, &stable_id_root);
        }
        results.push((test, result));
    }

    let failures: Vec<_> = results
        .iter()
        .filter(|(_, r)| matches!(r, TestResult::Failed(_, _) | TestResult::XPassed(_)))
        .collect();

    if report_format == TestOutputFormat::Console && !failures.is_empty() {
        println!();
        println!("{}", style(centered_eq_banner("FAILURES"), RED, use_color));
        for (test, result) in failures {
            let display_name = test_display_name(test);
            println!();
            println!("{}", style(centered_banner(display_name, '_'), "1", use_color));
            if let TestResult::Failed(_, msg) = result {
                println!();
                println!("    {}", msg);
            } else if let TestResult::XPassed(_) = result {
                println!();
                println!(
                    "    {}",
                    style("Test passed but was expected to fail (xfail)", YELLOW, use_color)
                );
            }
            println!();
            println!("    {}::{}", test.file_path.display(), display_name);
        }
    }

    let total_time = start_time.elapsed();
    if let Some(path) = junit_path.as_ref() {
        write_junit_report(path, &results, total_time)?;
    }
    if report_format == TestOutputFormat::Console {
        if let Some(count) = durations {
            print_durations(&results, count, use_color, &stable_id_root);
        }
        println!();
    }
    let mut parts = Vec::new();
    if passed > 0 {
        parts.push(format!("{} passed", passed));
    }
    if failed > 0 {
        parts.push(format!("{} failed", failed));
    }
    if skipped > 0 {
        parts.push(format!("{} skipped", skipped));
    }
    if xfailed > 0 {
        parts.push(format!("{} xfailed", xfailed));
    }
    if xpassed > 0 {
        parts.push(format!("{} xpassed", xpassed));
    }

    if report_format == TestOutputFormat::Console {
        let summary_label = format!("{} in {:.2}s", parts.join(", "), total_time.as_secs_f64());
        let summary_color = if failed > 0 || xpassed > 0 { RED } else { GREEN };
        println!(
            "{}",
            style(centered_eq_banner(&summary_label), summary_color, use_color)
        );
    } else {
        println!(
            "{}",
            serde_json::json!({
                "schema_version": "incan.test.v1",
                "summary": {
                    "total": results.len(),
                    "passed": passed,
                    "failed": failed,
                    "skipped": skipped,
                    "xfailed": xfailed,
                    "xpassed": xpassed,
                    "duration_ms": total_time.as_millis(),
                    "shuffle_seed": shuffle_seed,
                }
            })
        );
    }

    if failed > 0 || xpassed > 0 {
        // Tests failed - return error with empty message (summary already printed).
        Err(CliError::new("", ExitCode::FAILURE))
    } else {
        Ok(ExitCode::SUCCESS)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn make_test(name: &str, markers: Vec<TestMarker>) -> TestInfo {
        TestInfo {
            file_path: PathBuf::from("test_example.incn"),
            function_name: name.to_string(),
            is_async: false,
            markers,
            required_fixtures: Vec::new(),
            parameter_names: Vec::new(),
            timeout: None,
            parametrize_call: None,
        }
    }

    fn make_case(display_id: &str, rust_args: &str) -> ParametrizeCase {
        ParametrizeCase {
            display_id: display_id.to_string(),
            rust_arguments: rust_args.split(", ").map(str::to_string).collect(),
            parameters: display_id.split('-').map(str::to_string).collect(),
            markers: Vec::new(),
        }
    }

    // ---- expand_parametrized_tests ----

    #[test]
    fn expand_plain_test_passes_through() {
        let tests = vec![make_test("test_simple", vec![])];
        let expanded = expand_parametrized_tests(tests);

        assert_eq!(expanded.len(), 1);
        assert_eq!(expanded[0].function_name, "test_simple");
        assert!(expanded[0].parametrize_call.is_none());
    }

    #[test]
    fn expand_parametrized_creates_n_variants() {
        let cases = vec![make_case("1-2-3", "1, 2, 3"), make_case("0-0-0", "0, 0, 0")];
        let tests = vec![make_test(
            "test_add",
            vec![TestMarker::Parametrize("x, y, expected".to_string(), cases)],
        )];
        let expanded = expand_parametrized_tests(tests);

        assert_eq!(expanded.len(), 2, "should expand into 2 test variants");

        let pc0 = expanded[0].parametrize_call.as_ref();
        assert!(pc0.is_some());
        assert_eq!(pc0.map(|p| p.display_id.as_str()), Some("test_add[1-2-3]"));
        assert_eq!(pc0.map(ParametrizeCall::rust_args), Some("1, 2, 3".to_string()));

        let pc1 = expanded[1].parametrize_call.as_ref();
        assert!(pc1.is_some());
        assert_eq!(pc1.map(|p| p.display_id.as_str()), Some("test_add[0-0-0]"));
        assert_eq!(pc1.map(ParametrizeCall::rust_args), Some("0, 0, 0".to_string()));
    }

    #[test]
    fn expand_preserves_non_parametrize_markers() {
        let cases = vec![make_case("1", "1")];
        let markers = vec![TestMarker::Slow, TestMarker::Parametrize("x".to_string(), cases)];
        let tests = vec![make_test("test_slow_param", markers)];
        let expanded = expand_parametrized_tests(tests);

        assert_eq!(expanded.len(), 1);
        assert_eq!(expanded[0].markers, vec![TestMarker::Slow]);
        assert!(expanded[0].parametrize_call.is_some());
    }

    #[test]
    fn expand_mixed_plain_and_parametrized() {
        let cases = vec![make_case("a", "\"a\".to_string()"), make_case("b", "\"b\".to_string()")];
        let tests = vec![
            make_test("test_plain", vec![]),
            make_test("test_param", vec![TestMarker::Parametrize("s".to_string(), cases)]),
            make_test("test_another", vec![]),
        ];
        let expanded = expand_parametrized_tests(tests);

        assert_eq!(expanded.len(), 4, "1 plain + 2 parametrized + 1 plain");
        assert_eq!(expanded[0].function_name, "test_plain");
        assert!(expanded[0].parametrize_call.is_none());
        assert_eq!(expanded[1].function_name, "test_param");
        assert!(expanded[1].parametrize_call.is_some());
        assert_eq!(expanded[2].function_name, "test_param");
        assert!(expanded[2].parametrize_call.is_some());
        assert_eq!(expanded[3].function_name, "test_another");
        assert!(expanded[3].parametrize_call.is_none());
    }

    // ---- centered_banner ----

    #[test]
    fn banner_contains_label() {
        let b = centered_banner("test session starts", '=');
        assert!(b.contains("test session starts"));
    }

    #[test]
    fn scheduler_blocks_overlapping_resource_units() {
        let active = vec![ActiveUnit {
            index: 0,
            file_paths: BTreeSet::from([PathBuf::from("test_resource.incn")]),
            resources: BTreeSet::from(["db".to_string()]),
            serial: false,
        }];
        let blocked = ExecutionUnit {
            index: 1,
            file_paths: BTreeSet::from([PathBuf::from("test_other.incn")]),
            tests: vec![make_test("test_db", vec![TestMarker::Resource("db".to_string())])],
            conftest_files_by_file: HashMap::new(),
            timeout: None,
            resources: BTreeSet::from(["db".to_string()]),
            serial: false,
        };
        let allowed = ExecutionUnit {
            index: 2,
            file_paths: BTreeSet::from([PathBuf::from("test_cache.incn")]),
            tests: vec![make_test("test_cache", vec![TestMarker::Resource("cache".to_string())])],
            conftest_files_by_file: HashMap::new(),
            timeout: None,
            resources: BTreeSet::from(["cache".to_string()]),
            serial: false,
        };

        assert!(
            !active_units_allow(&active, &blocked),
            "same resource key should not overlap"
        );
        assert!(
            active_units_allow(&active, &allowed),
            "different resource keys may overlap"
        );
    }

    #[test]
    fn scheduler_runs_serial_units_alone() {
        let serial = ExecutionUnit {
            index: 1,
            file_paths: BTreeSet::from([PathBuf::from("test_serial.incn")]),
            tests: vec![make_test("test_serial", vec![TestMarker::Serial])],
            conftest_files_by_file: HashMap::new(),
            timeout: None,
            resources: BTreeSet::new(),
            serial: true,
        };
        assert!(
            active_units_allow(&[], &serial),
            "serial unit may start when no unit is active"
        );

        let active = vec![ActiveUnit {
            index: 0,
            file_paths: BTreeSet::from([PathBuf::from("test_regular.incn")]),
            resources: BTreeSet::new(),
            serial: false,
        }];
        assert!(
            !active_units_allow(&active, &serial),
            "serial unit must wait for active units to finish"
        );

        let active_serial = vec![ActiveUnit {
            index: 0,
            file_paths: BTreeSet::from([PathBuf::from("test_serial.incn")]),
            resources: BTreeSet::new(),
            serial: true,
        }];
        let ordinary = ExecutionUnit {
            index: 2,
            file_paths: BTreeSet::from([PathBuf::from("test_regular.incn")]),
            tests: vec![make_test("test_regular", vec![])],
            conftest_files_by_file: HashMap::new(),
            timeout: None,
            resources: BTreeSet::new(),
            serial: false,
        };
        assert!(
            !active_units_allow(&active_serial, &ordinary),
            "ordinary units must wait while a serial unit is active"
        );
    }

    #[test]
    fn scheduler_blocks_same_file_units() {
        let active = vec![ActiveUnit {
            index: 0,
            file_paths: BTreeSet::from([PathBuf::from("test_same_file.incn")]),
            resources: BTreeSet::from(["db".to_string()]),
            serial: false,
        }];
        let candidate = ExecutionUnit {
            index: 1,
            file_paths: BTreeSet::from([PathBuf::from("test_same_file.incn")]),
            tests: vec![make_test("test_cache", vec![TestMarker::Resource("cache".to_string())])],
            conftest_files_by_file: HashMap::new(),
            timeout: None,
            resources: BTreeSet::from(["cache".to_string()]),
            serial: false,
        };

        assert!(
            !active_units_allow(&active, &candidate),
            "same source file units share a generated Cargo crate and must not overlap"
        );
    }
}
