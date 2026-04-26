//! Test runner implementation (pytest-style).

use std::collections::{BTreeSet, HashMap};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::cli::{CliError, CliResult, ExitCode};

mod discovery;
mod execution;
mod module_graph;
mod reporter;
mod types;

pub use discovery::{discover_test_files, discover_tests_and_fixtures};
pub use reporter::{ConsoleReporter, TestReporter};
pub use types::{
    DiscoveryResult, FixtureInfo, FixtureScope, ParametrizeCall, ParametrizeCase, TestInfo, TestMarker,
    TestOutputFormat, TestResult, TestRunConfig, TestSummary,
};

use discovery::{discover_tests_and_fixtures_with_context, get_autouse_fixtures, parse_duration_literal};
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
                    markers: non_parametrize_markers,
                    required_fixtures: test.required_fixtures.clone(),
                    parameter_names: test.parameter_names.clone(),
                    timeout: timeout.or(test.timeout),
                    parametrize_call: Some(ParametrizeCall {
                        display_id,
                        rust_args: case.rust_args.clone(),
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

fn expand_parametrize_cases(markers: &[(String, Vec<ParametrizeCase>)]) -> Vec<ParametrizeCase> {
    let mut expanded = vec![ParametrizeCase {
        display_id: String::new(),
        rust_args: String::new(),
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
                let rust_args = if prefix.rust_args.is_empty() {
                    case.rust_args.clone()
                } else {
                    format!("{}, {}", prefix.rust_args, case.rust_args)
                };
                let mut rust_arguments = prefix.rust_arguments.clone();
                rust_arguments.extend(case.rust_arguments.clone());
                let mut parameters = prefix.parameters.clone();
                parameters.extend(case.parameters.clone());
                let mut markers = prefix.markers.clone();
                markers.extend(case.markers.clone());
                next.push(ParametrizeCase {
                    display_id,
                    rust_args,
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

/// Stable test identifier used by `-k`, `--list`, and machine-readable reports.
fn stable_test_id(test: &TestInfo, root: &Path) -> String {
    let file_path = test.file_path.canonicalize().unwrap_or_else(|_| test.file_path.clone());
    let path = file_path
        .strip_prefix(root)
        .unwrap_or(file_path.as_path())
        .to_path_buf();
    format!("{}::{}", path.to_string_lossy(), test_display_name(test))
}

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

fn next_shuffle_state(state: &mut u64) -> u64 {
    *state = state
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    *state
}

fn shuffle_tests(tests: &mut [TestInfo], seed: u64) {
    let mut state = seed;
    for i in (1..tests.len()).rev() {
        let j = (next_shuffle_state(&mut state) as usize) % (i + 1);
        tests.swap(i, j);
    }
}

fn result_status(result: &TestResult) -> &'static str {
    match result {
        TestResult::Passed(_) => "passed",
        TestResult::Failed(_, _) => "failed",
        TestResult::Skipped(_) => "skipped",
        TestResult::XFailed(_, _) => "xfailed",
        TestResult::XPassed(_) => "xpassed",
    }
}

fn result_duration_ms(result: &TestResult) -> u128 {
    match result {
        TestResult::Passed(duration)
        | TestResult::Failed(duration, _)
        | TestResult::XFailed(duration, _)
        | TestResult::XPassed(duration) => duration.as_millis(),
        TestResult::Skipped(_) => 0,
    }
}

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

fn xml_escape(raw: &str) -> String {
    raw.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

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
    fn parse(tokens: &'a [MarkerToken], names: &'a BTreeSet<String>) -> Result<bool, String> {
        let mut parser = Self { tokens, pos: 0, names };
        let value = parser.parse_or()?;
        if parser.pos != tokens.len() {
            return Err("unexpected trailing marker expression token".to_string());
        }
        Ok(value)
    }

    fn parse_or(&mut self) -> Result<bool, String> {
        let mut value = self.parse_and()?;
        while self.match_token(&MarkerToken::Or) {
            value = value || self.parse_and()?;
        }
        Ok(value)
    }

    fn parse_and(&mut self) -> Result<bool, String> {
        let mut value = self.parse_not()?;
        while self.match_token(&MarkerToken::And) {
            value = value && self.parse_not()?;
        }
        Ok(value)
    }

    fn parse_not(&mut self) -> Result<bool, String> {
        if self.match_token(&MarkerToken::Not) {
            Ok(!self.parse_not()?)
        } else {
            self.parse_primary()
        }
    }

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

    fn match_token(&mut self, expected: &MarkerToken) -> bool {
        if self.tokens.get(self.pos) == Some(expected) {
            self.pos += 1;
            true
        } else {
            false
        }
    }
}

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

fn marker_name_is_valid(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first.is_ascii_lowercase() || first == '_')
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

fn marker_expr_matches(test: &TestInfo, tokens: &[MarkerToken]) -> Result<bool, String> {
    MarkerExprParser::parse(tokens, &marker_names(test))
}

fn applicable_conftest_files(test_file: &Path, root: &Path) -> Vec<PathBuf> {
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

fn builtin_marker_names() -> BTreeSet<String> {
    ["test", "skip", "xfail", "slow", "resource", "serial", "timeout"]
        .into_iter()
        .map(str::to_string)
        .collect()
}

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

fn effective_timeout(test: &TestInfo, default_timeout: Option<Duration>) -> Option<Duration> {
    test.timeout.or(default_timeout)
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
        timeout,
        no_capture,
        locked,
        frozen,
        cargo_features,
        cargo_no_default_features,
        cargo_all_features,
    } = config;

    let start_time = Instant::now();
    let jobs = jobs.max(1);

    let path = Path::new(path);
    let stable_id_root = stable_id_root(path);
    let test_files = discover_test_files(path);

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
        ) {
            Ok(result) => {
                for marker in result.known_markers {
                    known_markers.insert(marker);
                }
                for fixture in result.fixtures {
                    applicable_fixtures.insert(fixture.name.clone(), fixture.clone());
                    all_fixtures.insert(fixture.name.clone(), fixture);
                }
                let autouse_fixtures = get_autouse_fixtures(&applicable_fixtures, FixtureScope::Function);
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
        Some(expr) => Some(tokenize_marker_expr(expr).map_err(CliError::failure)?),
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

    let mut prep_cache: HashMap<String, Arc<execution::PreparedTestFile>> = HashMap::new();

    let mut results: Vec<(TestInfo, TestResult)> = Vec::new();
    let mut passed = 0;
    let mut failed = 0;
    let mut skipped = 0;
    let mut xfailed = 0;
    let mut xpassed = 0;

    let mut idx = 0usize;
    while idx < filtered_tests.len() {
        let test = &filtered_tests[idx];

        if let Some(TestMarker::Skip(reason)) = test.markers.iter().find(|m| matches!(m, TestMarker::Skip(_))) {
            let result = TestResult::Skipped(reason.clone());
            if report_format == TestOutputFormat::Console {
                print_test_result(test, &result, verbose, use_color);
            } else {
                emit_json_result(test, &result, &stable_id_root);
            }
            skipped += 1;
            results.push((test.clone(), result));
            idx += 1;
            continue;
        }

        let file_path = test.file_path.clone();
        let batch_start = idx;
        idx += 1;
        while idx < filtered_tests.len() {
            let t = &filtered_tests[idx];
            if t.markers.iter().any(|m| matches!(m, TestMarker::Skip(_))) {
                break;
            }
            if t.file_path != file_path {
                break;
            }
            if effective_timeout(t, default_timeout) != effective_timeout(test, default_timeout) {
                break;
            }
            idx += 1;
        }

        let batch = &filtered_tests[batch_start..idx];
        let batch_timeout = batch
            .iter()
            .filter_map(|test| effective_timeout(test, default_timeout))
            .min();
        if report_format == TestOutputFormat::Console {
            println!(
                "{}",
                style(
                    format!("running {} ({} item(s))", file_path.display(), batch.len()),
                    "2",
                    use_color
                )
            );
        }
        let _ = std::io::stdout().flush();
        let batch_results = run_file_tests_batch(
            batch,
            &mut prep_cache,
            locked,
            frozen,
            &cargo_features,
            cargo_no_default_features,
            cargo_all_features,
            TestExecutionOptions {
                fail_fast: stop_on_fail,
                no_capture,
                timeout: batch_timeout,
                jobs,
            },
        );

        let mut stop_after_batch = false;
        for (test, result) in batch_results {
            let is_xfail = !run_xfail && test.markers.iter().any(|m| matches!(m, TestMarker::XFail(_)));

            let result = if is_xfail {
                match result {
                    TestResult::Passed(d) => {
                        xpassed += 1;
                        TestResult::XPassed(d)
                    }
                    TestResult::Failed(d, _) => {
                        let reason = test
                            .markers
                            .iter()
                            .find_map(|m| {
                                if let TestMarker::XFail(r) = m {
                                    Some(r.clone())
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
                match &result {
                    TestResult::Passed(_) => passed += 1,
                    TestResult::Failed(_, _) => failed += 1,
                    _ => {}
                }
                result
            };

            if report_format == TestOutputFormat::Console {
                print_test_result(&test, &result, verbose, use_color);
            } else {
                emit_json_result(&test, &result, &stable_id_root);
            }
            results.push((test, result));

            if stop_on_fail
                && let Some((_, r)) = results.last()
                && matches!(r, TestResult::Failed(_, _))
            {
                stop_after_batch = true;
            }
        }

        if stop_after_batch {
            break;
        }
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
            rust_args: rust_args.to_string(),
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
        assert_eq!(pc0.map(|p| p.rust_args.as_str()), Some("1, 2, 3"));

        let pc1 = expanded[1].parametrize_call.as_ref();
        assert!(pc1.is_some());
        assert_eq!(pc1.map(|p| p.display_id.as_str()), Some("test_add[0-0-0]"));
        assert_eq!(pc1.map(|p| p.rust_args.as_str()), Some("0, 0, 0"));
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
}
