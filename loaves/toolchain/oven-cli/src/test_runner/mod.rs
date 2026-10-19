//! Test runner implementation (pytest-style).

#[cfg(test)]
use std::cell::Cell;
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::{CliError, CliResult, ExitCode};
use incan_driver::lock::policy::LockPolicy;
use incan_driver::session::CompilationSession;

mod execution;
pub(crate) mod output;
mod reporter;
mod rust;

pub use incan_driver::testing::discovery::{discover_test_files, discover_tests_and_fixtures};
pub use incan_driver::testing::types::{
    DiscoveryResult, FixtureInfo, FixtureScope, ParametrizeCall, ParametrizeCase, TestCaseResult, TestInfo,
    TestLanguages, TestMarker, TestOutputFormat, TestProgress, TestResult, TestRunConfig, TestRunReport, TestSummary,
    WorkspaceTestContext,
};
pub use reporter::{ConsoleReporter, TestReporter};

use execution::{
    OvenTestCommandContext, TestExecutionOptions, prepare_oven_test_command_context, run_file_tests_batch,
    validate_oven_test_lock_policy,
};
use incan_driver::testing::discovery::{
    CollectionEvalContext, discover_test_file_candidates, discover_test_files_with_session,
    discover_tests_and_fixtures_with_context, get_autouse_fixtures, parse_duration_literal,
};
use output::TestOutputSink;
use reporter::{print_test_result, style};

const RED: &str = "1;31";
const GREEN: &str = "1;32";
const YELLOW: &str = "33";
const BANNER_WIDTH: usize = 58;

/// Attribute one existing test stage without opening a sink or constructing fields when tracing is disabled.
fn trace_test_stage(stage: &'static str) -> tracing::span::EnteredSpan {
    tracing::trace_span!(target: "incan::invocation", "incan_cli::test_stage", stage).entered()
}

#[cfg(test)]
thread_local! {
    static TEST_COMMAND_SESSION_CONSTRUCTIONS: Cell<usize> = const { Cell::new(0) };
}

/// Construct an uncached canonical session for unit controls of collection and lock behavior.
#[cfg(test)]
fn prepare_test_command_session(
    representative_test: &Path,
    list_only: bool,
    package_features: &incan_provider::FeatureSelection,
    sdk_profile_override: Option<&str>,
) -> CliResult<Arc<CompilationSession>> {
    #[cfg(test)]
    TEST_COMMAND_SESSION_CONSTRUCTIONS.with(|count| count.set(count.get().saturating_add(1)));

    let session = if list_only {
        CompilationSession::discover_for_collection_with_selections(
            representative_test,
            package_features,
            sdk_profile_override,
        )?
    } else {
        CompilationSession::discover_for_oven(representative_test, package_features, sdk_profile_override)?
    };
    Ok(Arc::new(session))
}

/// Create a centered banner with a configurable fill character.
///
/// For example: `centered_banner("test session starts", '_')` -> `"___________________ test session starts
/// __________________"`
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
fn emit_json_result(
    test: &TestInfo,
    result: &TestResult,
    id: &str,
    workspace_context: Option<&WorkspaceTestContext>,
    output: &TestOutputSink,
) {
    let mut record = serde_json::json!({
        "schema_version": "incan.test.v1",
        "test_id": id,
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
        if let Some(workspace) = workspace_context {
            obj.insert(
                "workspace".to_string(),
                serde_json::json!({
                    "root": workspace.workspace_root.display().to_string(),
                    "scope_origin": workspace.scope_origin,
                    "member": {
                        "name": workspace.member_name,
                        "root": workspace.member_root.display().to_string(),
                    },
                }),
            );
        }
    }
    output.line(format_args!("{record}"));
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
fn write_junit_report(path: &Path, results: &[TestCaseResult], total_time: std::time::Duration) -> CliResult<()> {
    let failures = results
        .iter()
        .filter(|case| matches!(case.result, TestResult::Failed(_, _) | TestResult::XPassed(_)))
        .count();
    let skipped = results
        .iter()
        .filter(|case| matches!(case.result, TestResult::Skipped(_) | TestResult::XFailed(_, _)))
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
    for TestCaseResult { test, result, .. } in results {
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
fn print_durations(results: &[TestCaseResult], count: usize, use_color: bool, output: &TestOutputSink) {
    if count == 0 || results.is_empty() {
        return;
    }
    let mut ordered: Vec<_> = results.iter().collect();
    ordered.sort_by_key(|case| std::cmp::Reverse(result_duration_ms(&case.result)));

    output.line("");
    output.line(format_args!(
        "{}",
        style(centered_eq_banner("slowest durations"), "1", use_color)
    ));
    for case in ordered.into_iter().take(count) {
        output.line(format_args!("{:>6}ms {}", result_duration_ms(&case.result), case.id));
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

/// Retain collected tests selected by the CLI's keyword, slow-test, and marker rules.
///
/// This is deliberately pure: collection and execution remain responsible for their own I/O, while selection rules can
/// be covered without rebuilding a generated test project for every boolean marker combination.
fn filter_collected_tests(
    tests: Vec<TestInfo>,
    filter: Option<&str>,
    include_slow: bool,
    marker_tokens: Option<&[MarkerToken]>,
    stable_id_root: &Path,
) -> Vec<TestInfo> {
    tests
        .into_iter()
        .filter(|test| {
            if let Some(keyword) = filter
                && !stable_test_id(test, stable_id_root).contains(keyword)
            {
                return false;
            }
            if !include_slow && test.markers.contains(&TestMarker::Slow) {
                return false;
            }
            if let Some(tokens) = marker_tokens {
                match marker_expr_matches(test, tokens) {
                    Ok(true) => {}
                    Ok(false) | Err(_) => return false,
                }
            }
            true
        })
        .collect()
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

#[derive(Clone)]
struct CachedConftestDiscovery {
    fixtures: Vec<FixtureInfo>,
    default_marks: Vec<String>,
    known_markers: Vec<String>,
}

#[derive(Default)]
struct ConftestDiscoveryCache {
    entries: HashMap<PathBuf, Result<CachedConftestDiscovery, String>>,
    parse_count: usize,
}

impl ConftestDiscoveryCache {
    /// Parse one inherited conftest at most once per canonical path for this command.
    #[allow(clippy::too_many_arguments)]
    fn discover(
        &mut self,
        path: &Path,
        visible_fixture_names: &[String],
        inherited_marks: &[String],
        inherited_known_markers: &[String],
        eval_context: &CollectionEvalContext,
        session: &CompilationSession,
    ) -> Result<CachedConftestDiscovery, String> {
        let key = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        if let Some(result) = self.entries.get(&key) {
            return result.clone();
        }

        self.parse_count = self.parse_count.saturating_add(1);
        let result = discover_tests_and_fixtures_with_context(
            path,
            visible_fixture_names,
            inherited_marks,
            inherited_known_markers,
            eval_context,
            session,
        )
        .map(|result| CachedConftestDiscovery {
            fixtures: result.fixtures,
            default_marks: result.default_marks,
            known_markers: result.known_markers,
        });
        self.entries.insert(key, result.clone());
        result
    }
}

struct CollectedTestInventory {
    tests: Vec<TestInfo>,
    fixtures: HashMap<String, FixtureInfo>,
    known_markers: BTreeSet<String>,
}

/// Collect every test source with one shared compilation session and one canonical conftest cache.
fn collect_test_inventory(
    test_files: &[PathBuf],
    root: &Path,
    eval_context: &CollectionEvalContext,
    session: &CompilationSession,
    conftest_cache: &mut ConftestDiscoveryCache,
) -> Result<CollectedTestInventory, String> {
    let mut all_tests = Vec::new();
    let mut all_fixtures = HashMap::new();
    let mut known_markers = builtin_marker_names();
    let mut collection_errors = Vec::new();
    let mut reported_conftest_errors = BTreeSet::new();

    for file_path in test_files {
        let mut visible_fixture_names = Vec::new();
        let mut inherited_marks = Vec::new();
        let mut inherited_known_markers = Vec::new();
        let mut applicable_fixtures = HashMap::new();

        for conftest in applicable_conftest_files(file_path, root) {
            match conftest_cache.discover(
                &conftest,
                &visible_fixture_names,
                &inherited_marks,
                &inherited_known_markers,
                eval_context,
                session,
            ) {
                Ok(result) => {
                    inherited_marks = result.default_marks;
                    inherited_known_markers = result.known_markers.clone();
                    known_markers.extend(result.known_markers);
                    for fixture in result.fixtures {
                        visible_fixture_names.push(fixture.name.clone());
                        applicable_fixtures.insert(fixture.name.clone(), fixture.clone());
                        all_fixtures.insert(fixture.name.clone(), fixture);
                    }
                }
                Err(error) => {
                    let key = std::fs::canonicalize(&conftest).unwrap_or_else(|_| conftest.clone());
                    if reported_conftest_errors.insert(key) {
                        collection_errors.push(format!("Error parsing {}: {error}", conftest.display()));
                    }
                }
            }
        }

        match discover_tests_and_fixtures_with_context(
            file_path,
            &visible_fixture_names,
            &inherited_marks,
            &inherited_known_markers,
            eval_context,
            session,
        ) {
            Ok(result) => {
                known_markers.extend(result.known_markers);
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
            Err(error) => collection_errors.push(format!("Error parsing {}: {error}", file_path.display())),
        }
    }

    if collection_errors.is_empty() {
        Ok(CollectedTestInventory {
            tests: all_tests,
            fixtures: all_fixtures,
            known_markers,
        })
    } else {
        Err(collection_errors.join("\n"))
    }
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

/// Return the timeout marker or run default used for one generated native test batch.
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
    /// A shared Rust root reserves this entire global budget while it executes.
    shared_threads: Option<usize>,
}

#[derive(Debug)]
struct ActiveUnit {
    index: usize,
    file_paths: BTreeSet<PathBuf>,
    resources: BTreeSet<String>,
    serial: bool,
}

/// Results from one scheduled unit, retaining its original index for stable report ordering.
type CompletedExecutionUnit = (usize, Vec<(TestInfo, TestResult)>);

/// Run one planned execution unit through the normal Oven native harness path.
#[allow(clippy::too_many_arguments)]
fn run_execution_unit(
    unit: &ExecutionUnit,
    lock_policy: &LockPolicy,
    command_context: &Arc<OvenTestCommandContext>,
    no_capture: bool,
    verbose: bool,
    output: TestOutputSink,
) -> Vec<(TestInfo, TestResult)> {
    if unit
        .tests
        .first()
        .is_some_and(|test| test.file_path.extension().is_some_and(|extension| extension == "rs"))
    {
        return rust::execute(
            &unit.tests,
            command_context,
            unit.timeout,
            no_capture,
            &output,
            unit.shared_threads,
        );
    }
    run_file_tests_batch(
        &unit.tests,
        &unit.conftest_files_by_file,
        command_context,
        TestExecutionOptions {
            no_capture,
            timeout: unit.timeout,
            verbose,
            output,
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

/// Return whether a test can share a generated native harness batch with an existing unit.
fn unit_can_include(
    unit: &ExecutionUnit,
    test: &TestInfo,
    root: &Path,
    default_timeout: Option<Duration>,
    allow_cross_file_batches: bool,
) -> bool {
    !unit.serial
        && test.file_path.extension().is_none_or(|extension| extension != "rs")
        && unit
            .file_paths
            .iter()
            .all(|path| path.extension().is_none_or(|extension| extension != "rs"))
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

/// Convert collected tests into generated worker-batch native harness execution units.
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
                // Skipped tests never enter the native harness. Do not split the surrounding compatible tests into
                // separate Oven units: that would repeat front-end work and overwrite/rebuild otherwise reusable
                // caller-owned native outputs for the same source file.
                idx += 1;
                continue;
            }
            let provisional = ExecutionUnit {
                shared_threads: None,
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
            shared_threads: None,
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

/// Group compatible Rust cases without changing Incan fixtures, per-case deadlines or explicit ordering.
/// A shared root reserves the whole worker budget and runs exclusively in the existing scheduler. Other roots
/// cannot multiply that budget. Deadline-bearing, resource-constrained or serial cases retain individual process
/// execution.
fn share_native_execution_units(units: Vec<ExecutionUnit>, jobs: usize, isolate: bool) -> Vec<ExecutionUnit> {
    if isolate {
        return units;
    }
    let mut groups: Vec<ExecutionUnit> = Vec::new();
    for mut unit in units {
        if unit_contains_rust(&unit) && !unit.serial && unit.timeout.is_none() && unit.resources.is_empty() {
            if let Some(group) = groups.iter_mut().find(|group| {
                group.shared_threads.is_some()
                    && group.file_paths == unit.file_paths
                    && group.resources == unit.resources
            }) {
                merge_execution_unit(group, unit);
                continue;
            }
            unit.shared_threads = Some(jobs.max(1));
            unit.serial = true;
        }
        groups.push(unit);
    }
    groups
}

/// Prepared Rust cases have no generated Incan harness or process-local Incan fixtures to coalesce.
fn unit_contains_rust(unit: &ExecutionUnit) -> bool {
    unit.file_paths
        .iter()
        .any(|path| path.extension().is_some_and(|extension| extension == "rs"))
}

/// Return whether two Incan units can share one generated worker-batch harness.
fn units_share_worker_profile(left: &ExecutionUnit, right: &ExecutionUnit) -> bool {
    !left.serial
        && !right.serial
        && !unit_contains_rust(left)
        && !unit_contains_rust(right)
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
/// Each batch runs through one native libtest harness. Grouping by identical conftest and scheduling profile keeps
/// session fixtures process-local to a worker while preserving resource and serial constraints at scheduler level.
fn coalesce_worker_batches(units: Vec<ExecutionUnit>, jobs: usize) -> Vec<ExecutionUnit> {
    let mut batches: Vec<ExecutionUnit> = Vec::new();
    for unit in units {
        if unit.serial || unit_contains_rust(&unit) {
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

/// Admit independent prepared Rust cases concurrently while excluding shared generated Incan outputs.
/// Resource and serial constraints apply to both languages, regardless of whether their executable is already built.
fn active_units_allow(active: &[ActiveUnit], candidate: &ExecutionUnit) -> bool {
    if active.iter().any(|unit| {
        unit.file_paths
            .intersection(&candidate.file_paths)
            .any(|path| path.extension().is_none_or(|extension| extension != "rs"))
    }) {
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
///
/// Workers can compile Incan harnesses, so they use the same stack contract as the compiler entrypoint (#2213).
/// A failed worker startup stops new launches and drains existing workers before returning the error.
/// A worker unwind likewise stops scheduling and is resumed on the caller after the other active workers finish.
#[allow(clippy::too_many_arguments)]
fn run_scheduled_execution_units(
    units: Vec<ExecutionUnit>,
    jobs: usize,
    lock_policy: LockPolicy,
    command_context: Arc<OvenTestCommandContext>,
    stop_on_fail: bool,
    no_capture: bool,
    verbose: bool,
    output: TestOutputSink,
) -> CliResult<Vec<CompletedExecutionUnit>> {
    schedule_execution_units(units, jobs, stop_on_fail, move |unit| {
        run_execution_unit(
            unit,
            &lock_policy,
            &command_context,
            no_capture,
            verbose,
            output.clone(),
        )
    })
}

/// Schedule the existing execution callback under the shared resource and worker limits.
/// Every worker sends a terminal outcome, including an unwind; the first panic is resumed only after active work
/// drains, preserving the inline compiler panic contract without treating a partially executed request as success.
fn schedule_execution_units(
    units: Vec<ExecutionUnit>,
    jobs: usize,
    stop_on_fail: bool,
    execute: impl Fn(&ExecutionUnit) -> Vec<(TestInfo, TestResult)> + Send + Sync + 'static,
) -> CliResult<Vec<CompletedExecutionUnit>> {
    // A single compilation batch stays on the compiler-sized owner thread without another worker allocation.
    if jobs <= 1 || units.len() <= 1 {
        let mut completed = Vec::new();
        for unit in &units {
            let results = execute(unit);
            let failed = batch_has_failure(&results);
            completed.push((unit.index, results));
            if stop_on_fail && failed {
                break;
            }
        }
        return Ok(completed);
    }

    let total = units.len();
    let execute = Arc::new(execute);
    let mut pending: Vec<Option<ExecutionUnit>> = units.into_iter().map(Some).collect();
    let (sender, receiver) = mpsc::channel();
    let mut active = Vec::new();
    let mut completed = Vec::new();
    let mut launched = 0usize;
    let mut finished = 0usize;
    let mut stop_launching = false;
    let mut spawn_error = None;
    let mut worker_panic = None;
    let mut workers = Vec::new();

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
            let execute = Arc::clone(&execute);
            active.push(ActiveUnit {
                index: unit.index,
                file_paths: unit.file_paths.clone(),
                resources: unit.resources.clone(),
                serial: unit.serial,
            });
            launched += 1;
            let trace_parent = tracing::Span::current();
            let worker = thread::Builder::new()
                .name(format!("incan-test-{}", unit.index))
                .stack_size(incan_frontend::compiler_stack::compiler_stack_bytes())
                .spawn(move || {
                    let unit_index = unit.index;
                    // Catch only to report completion and drain peers. No request resumes normal execution after
                    // an unwind, so the callback's interior mutable state is never reused as a successful result.
                    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        let _parent = trace_parent.enter();
                        execute(&unit)
                    }));
                    let _ = sender.send((unit_index, outcome));
                });
            match worker {
                Ok(worker) => workers.push(worker),
                Err(error) => {
                    active.pop();
                    launched -= 1;
                    stop_launching = true;
                    spawn_error = Some(error);
                    break;
                }
            }
        }

        if active.is_empty() && launched == finished {
            break;
        }

        let Ok((unit_index, outcome)) = receiver.recv() else {
            break;
        };
        finished += 1;
        active.retain(|unit| unit.index != unit_index);
        match outcome {
            Ok(results) => {
                if stop_on_fail && batch_has_failure(&results) {
                    stop_launching = true;
                }
                completed.push((unit_index, results));
            }
            Err(payload) => {
                stop_launching = true;
                if worker_panic.is_none() {
                    worker_panic = Some(payload);
                }
            }
        }
    }

    // Completion precedes destruction of the worker's captured owners. Join before returning or resuming a panic
    // so no original lease or output callback remains active beyond this request's scheduler boundary.
    for worker in workers {
        if let Err(payload) = worker.join()
            && worker_panic.is_none()
        {
            worker_panic = Some(payload);
        }
    }
    if let Some(payload) = worker_panic {
        std::panic::resume_unwind(payload);
    }
    match spawn_error {
        Some(error) => Err(CliError::failure(format!(
            "could not start test compilation worker: {error}"
        ))),
        None => Ok(completed),
    }
}

/// Owned collection inputs carried by the existing owner transport; no compiler authority crosses the socket.
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct TestCollectionOptions {
    languages: TestLanguages,
    /// Explicit client-resolved collection path.
    pub(crate) path: String,
    verbose: bool,
    include_slow: bool,
    filter: Option<String>,
    marker_expr: Option<String>,
    strict_markers: bool,
    test_features: Vec<String>,
    package_features: incan_provider::FeatureSelection,
    sdk_profile: Option<String>,
    timeout: Option<String>,
    lock_policy: LockPolicy,
}

impl TestCollectionOptions {
    /// Own the collection subset of the existing request configuration without changing selection semantics.
    pub(crate) fn from_config(config: &TestRunConfig<'_>) -> Self {
        Self {
            languages: config.languages,
            path: config.path.to_string(),
            verbose: config.verbose,
            include_slow: config.include_slow,
            filter: config.filter.map(str::to_string),
            marker_expr: config.marker_expr.map(str::to_string),
            strict_markers: config.strict_markers,
            test_features: config.test_features.clone(),
            package_features: config.package_features.clone(),
            sdk_profile: config.sdk_profile.clone(),
            timeout: config.timeout.map(str::to_string),
            lock_policy: config.lock_policy.clone(),
        }
    }
}

/// Owned execution intent for the same runner; it carries configuration, never retained compiler authority.
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct TestExecutionRequest {
    /// Collection choices shared verbatim with list-only requests.
    pub(crate) collection: TestCollectionOptions,
    stop_on_fail: bool,
    use_color: bool,
    fail_on_empty: bool,
    report_format: TestOutputFormat,
    /// Client-resolved report destination; the owner never changes its working directory.
    pub(crate) junit_path: Option<PathBuf>,
    durations: Option<usize>,
    shuffle: bool,
    seed: Option<u64>,
    run_xfail: bool,
    jobs: usize,
    isolate: bool,
    no_capture: bool,
    workspace_context: Option<WorkspaceTestContext>,
}

impl TestExecutionRequest {
    /// Own an existing test configuration at the transport boundary without changing its semantics.
    pub(crate) fn from_config(config: &TestRunConfig<'_>) -> Self {
        Self {
            collection: TestCollectionOptions::from_config(config),
            stop_on_fail: config.stop_on_fail,
            use_color: config.use_color,
            fail_on_empty: config.fail_on_empty,
            report_format: config.report_format,
            junit_path: config.junit_path.clone(),
            durations: config.durations,
            shuffle: config.shuffle,
            seed: config.seed,
            run_xfail: config.run_xfail,
            jobs: config.jobs,
            isolate: config.isolate,
            no_capture: config.no_capture,
            workspace_context: config.workspace_context.clone(),
        }
    }

    /// Recreate the existing borrowed runner configuration only for this request's execution lifetime.
    fn as_config(&self) -> TestRunConfig<'_> {
        TestRunConfig {
            languages: self.collection.languages,
            path: &self.collection.path,
            verbose: self.collection.verbose,
            include_slow: self.collection.include_slow,
            filter: self.collection.filter.as_deref(),
            list_only: false,
            marker_expr: self.collection.marker_expr.as_deref(),
            strict_markers: self.collection.strict_markers,
            test_features: self.collection.test_features.clone(),
            package_features: self.collection.package_features.clone(),
            sdk_profile: self.collection.sdk_profile.clone(),
            timeout: self.collection.timeout.as_deref(),
            lock_policy: self.collection.lock_policy.clone(),
            stop_on_fail: self.stop_on_fail,
            use_color: self.use_color,
            fail_on_empty: self.fail_on_empty,
            report_format: self.report_format,
            junit_path: self.junit_path.clone(),
            durations: self.durations,
            shuffle: self.shuffle,
            seed: self.seed,
            run_xfail: self.run_xfail,
            jobs: self.jobs,
            isolate: self.isolate,
            no_capture: self.no_capture,
            workspace_context: self.workspace_context.clone(),
        }
    }

    /// Reject implicit working-directory authority before the owner executes or writes a report.
    pub(crate) fn validate_paths(&self) -> CliResult<()> {
        if !Path::new(&self.collection.path).is_absolute()
            || self.junit_path.as_ref().is_some_and(|path| !path.is_absolute())
            || self.workspace_context.as_ref().is_some_and(|workspace| {
                !workspace.workspace_root.is_absolute() || !workspace.member_root.is_absolute()
            })
        {
            return Err(CliError::failure("owner requires absolute test and report paths"));
        }
        Ok(())
    }
}

/// Fresh collection and selection, retaining only the session's original compiler authority inside its owner.
struct SelectedTestCollection {
    command_context: Option<Arc<OvenTestCommandContext>>,
    filtered_tests: Vec<TestInfo>,
    stable_id_root: PathBuf,
    default_timeout: Option<Duration>,
}

/// Run the single discovery/collection/selection implementation with a canonical or retained session supplier.
fn collect_selected_tests(
    options: &TestCollectionOptions,
    owner: &mut incan_driver::build::executable::ExecutableBuildOwner,
    list_only: bool,
    mut emit: impl FnMut(TestProgress),
) -> CliResult<SelectedTestCollection> {
    let TestCollectionOptions {
        languages,
        path,
        verbose,
        include_slow,
        filter,
        marker_expr,
        strict_markers,
        test_features,
        package_features,
        sdk_profile,
        timeout,
        lock_policy,
    } = options;
    let stage = trace_test_stage("discovery");
    let path = Path::new(path);
    let stable_id_root = stable_id_root(path);
    let candidates = discover_test_file_candidates(path);
    let rust_roots = if *languages != TestLanguages::Incan {
        rust::discover_roots(path)?
    } else {
        Vec::new()
    };
    if (candidates.is_empty() || *languages == TestLanguages::Rust) && rust_roots.is_empty() {
        return Err(CliError::failure(format!(
            "No test files found in '{}'\nTest files should be named test_*.incn or *_test.incn",
            path.display()
        )));
    }
    let authority_root = if candidates.is_empty() || *languages == TestLanguages::Rust {
        rust::project_root(path)?.ok_or_else(|| CliError::failure("Rust tests require an authored project"))?
    } else {
        candidates.command_authority_root().map_err(CliError::failure)?
    };
    let representative_test = if *languages == TestLanguages::Rust {
        rust_roots.first().cloned()
    } else {
        candidates
            .representative_path(&authority_root)
            .map(Path::to_path_buf)
            .or_else(|| rust_roots.first().cloned())
    };
    let representative_test =
        representative_test.ok_or_else(|| CliError::failure("test inventory lost its representative source"))?;
    drop(stage);
    let stage = trace_test_stage("session");
    let session = if list_only && *languages == TestLanguages::Incan {
        owner.test_collection_session(
            &authority_root,
            &representative_test,
            package_features,
            sdk_profile.as_deref(),
        )?
    } else {
        owner.test_execution_session(
            &authority_root,
            &representative_test,
            package_features,
            sdk_profile.as_deref(),
        )?
    };
    if let Some(manifest) = session.manifest.as_ref() {
        incan_driver::project::enforce_project_toolchain_constraint(manifest)?;
        let session_root =
            std::fs::canonicalize(manifest.project_root()).unwrap_or_else(|_| manifest.project_root().to_path_buf());
        if session_root != authority_root {
            return Err(CliError::failure(format!(
                "test inventory belongs to {}, but command session selected {}",
                authority_root.display(),
                session_root.display()
            )));
        }
    }
    drop(stage);
    let stage = trace_test_stage("candidate_selection");
    let test_files = if *languages == TestLanguages::Rust {
        Vec::new()
    } else {
        discover_test_files_with_session(&candidates, &authority_root, session.as_ref())
    };
    if test_files.is_empty() && rust_roots.is_empty() {
        return Err(CliError::failure(format!(
            "No test files found in '{}'\nTest files should be named test_*.incn or *_test.incn",
            path.display()
        )));
    }
    drop(stage);
    let stage = trace_test_stage("lock_policy");
    validate_oven_test_lock_policy(
        session.as_ref(),
        &representative_test,
        lock_policy,
        package_features,
    )?;

    drop(stage);
    let stage = trace_test_stage("collection");
    let eval_context = CollectionEvalContext::new(test_features.iter().cloned().collect());
    let mut conftest_cache = ConftestDiscoveryCache::default();
    let inventory = collect_test_inventory(&test_files, path, &eval_context, session.as_ref(), &mut conftest_cache)
        .map_err(CliError::failure)?;
    let mut all_tests = inventory.tests;
    let command_context = if !list_only || !rust_roots.is_empty() {
        let _admission = trace_test_stage("dependency_admission");
        let mut planned = test_files.clone();
        planned.extend(rust_roots.iter().cloned());
        let mut context =
            prepare_oven_test_command_context(Arc::clone(&session), &representative_test, &planned)?;
        let context_mut = Arc::get_mut(&mut context)
            .ok_or_else(|| CliError::failure("test command context escaped before collection"))?;
        for (root, prepared) in rust::prepare_roots(&rust_roots, context_mut, package_features, owner)? {
            all_tests.extend(prepared.tests.iter().cloned());
            context_mut.rust_roots.insert(root, prepared);
        }
        Some(context)
    } else {
        None
    };
    let all_fixtures = inventory.fixtures;
    let known_markers = inventory.known_markers;

    if *verbose && !all_fixtures.is_empty() {
        emit(TestProgress::FixturesDiscovered {
            fixtures: all_fixtures.into_values().collect(),
        });
    }

    drop(stage);
    let _selection = trace_test_stage("test_selection");

    // ---- Expand parametrized tests into individual variants ----
    let all_tests = expand_parametrized_tests(all_tests);

    let default_timeout = match timeout.as_deref() {
        Some(raw) => Some(parse_duration_literal(raw).ok_or_else(|| {
            CliError::failure(format!(
                "invalid --timeout value `{raw}`; use a duration like 250ms, 5s, or 2m"
            ))
        })?),
        None => None,
    };
    let marker_tokens = match marker_expr.as_deref() {
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
    validate_markers(&all_tests, &known_markers, *strict_markers, &marker_expr_names).map_err(CliError::failure)?;

    let filtered_tests = filter_collected_tests(
        all_tests,
        filter.as_deref(),
        *include_slow,
        marker_tokens.as_deref(),
        &stable_id_root,
    );

    Ok(SelectedTestCollection {
        command_context,
        filtered_tests,
        stable_id_root,
        default_timeout,
    })
}

/// Only portable display identities leave the owner; sessions, provider state and leases stay server-side.
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct TestListReport {
    /// Current selected test identities, recomputed from current source every request.
    pub(crate) ids: Vec<String>,
}

/// Collect a list through the same operation as execution, borrowing the existing owner's parser-only session.
pub(crate) fn list_tests_with_owner(
    options: &TestCollectionOptions,
    owner: &mut incan_driver::build::executable::ExecutableBuildOwner,
    emit: impl FnMut(TestProgress),
) -> CliResult<TestListReport> {
    let _invocation = tracing::trace_span!(target: "incan::invocation", "incan_cli::test", input = %options.path, list_only = true, jobs = 1).entered();
    let collected = collect_selected_tests(options, owner, true, emit)?;
    Ok(TestListReport {
        ids: collected
            .filtered_tests
            .iter()
            .map(|test| stable_test_id(test, &collected.stable_id_root))
            .collect(),
    })
}

/// Run all tests in the given path.
pub fn run_tests(config: TestRunConfig<'_>) -> CliResult<ExitCode> {
    incan_driver::project::warn_once_about_ignored_cargo_manifest(&incan_driver::project::resolve_project_root(
        Path::new(config.path),
    ));
    if config.list_only {
        return crate::project_owner::list_tests(&config);
    }
    crate::project_owner::execute_tests(&config)
}

/// Execute fresh tests through the single scheduler, borrowing canonical preparation from this request's owner.
pub(crate) fn execute_tests_with_owner(
    request: &TestExecutionRequest,
    owner: &mut incan_driver::build::executable::ExecutableBuildOwner,
    output: TestOutputSink,
) -> CliResult<TestRunReport> {
    request.validate_paths()?;
    let config = request.as_config();
    let collection_options = TestCollectionOptions::from_config(&config);
    let TestRunConfig {
        languages: _,
        path,
        verbose,
        stop_on_fail,
        include_slow: _,
        filter: _,
        use_color: _,
        fail_on_empty,
        list_only: _,
        report_format: _,
        junit_path: _,
        durations: _,
        shuffle,
        seed,
        run_xfail,
        marker_expr: _,
        strict_markers: _,
        jobs,
        isolate,
        test_features: _,
        package_features: _,
        sdk_profile: _,
        timeout: _,
        no_capture,
        lock_policy,
        workspace_context: _,
    } = config;

    let _invocation =
        tracing::trace_span!(target: "incan::invocation", "incan_cli::test", input = path, list_only = false, jobs)
            .entered();
    let start_time = Instant::now();
    let jobs = jobs.max(1);
    let path_for_execution = Path::new(path);

    let collected = collect_selected_tests(&collection_options, owner, false, |event| output.progress(event))?;
    let SelectedTestCollection {
        command_context,
        mut filtered_tests,
        stable_id_root,
        default_timeout,
    } = collected;

    if filtered_tests.is_empty() {
        output.progress(TestProgress::NoTestsCollected);
        return Ok(TestRunReport {
            results: Vec::new(),
            summary: TestSummary::default(),
            shuffle_seed: None,
            exit_code: if fail_on_empty {
                ExitCode::FAILURE.0
            } else {
                ExitCode::SUCCESS.0
            },
        });
    }

    let mut file_ids = HashMap::new();
    for test in &filtered_tests {
        file_ids.entry(test.file_path.clone()).or_insert_with(|| {
            let _identity = tracing::trace_span!(target: "incan::invocation", "incan_cli::test_file_identity", path = %test.file_path.display()).entered();
            let canonical = test.file_path.canonicalize().unwrap_or_else(|_| test.file_path.clone());
            canonical.strip_prefix(&stable_id_root).unwrap_or(&canonical).to_path_buf()
        });
    }

    let command_context =
        command_context.ok_or_else(|| CliError::failure("test execution lost its prepared command context"))?;
    let stage = trace_test_stage("scheduling");

    let shuffle_seed = if shuffle {
        let value = seed.unwrap_or_else(default_shuffle_seed);
        shuffle_tests(&mut filtered_tests, value);
        Some(value)
    } else {
        None
    };

    output.progress(TestProgress::SessionStarted {
        collected: filtered_tests.len(),
        shuffle_seed,
        jobs,
    });

    let mut results = Vec::new();
    let mut passed = 0;
    let mut failed = 0;
    let mut skipped = 0;
    let mut xfailed = 0;
    let mut xpassed = 0;

    let planning_start = Instant::now();
    let units = build_execution_units(&filtered_tests, path_for_execution, default_timeout, stop_on_fail, jobs);
    let units = share_native_execution_units(units, jobs, isolate || shuffle || stop_on_fail);
    if verbose {
        output.progress(TestProgress::UnitsPlanned {
            units: units.len(),
            elapsed: planning_start.elapsed(),
        });
    }
    if !stop_on_fail {
        for unit in &units {
            output.progress(TestProgress::UnitStarting {
                files: unit.file_paths.iter().cloned().collect(),
                tests: unit.tests.len(),
            });
        }
    }

    let mut raw_batch_results = run_scheduled_execution_units(
        units,
        jobs,
        lock_policy,
        command_context,
        stop_on_fail,
        no_capture,
        verbose,
        output.clone(),
    )?;
    if verbose {
        output.progress(TestProgress::ExecutionCompleted {
            elapsed: start_time.elapsed(),
        });
    }
    drop(stage);
    let _reporting = trace_test_stage("reporting");
    let case_key = |test: &TestInfo| (test.file_path.clone(), test_display_name(test).to_string());
    raw_batch_results.sort_by_key(|(index, _)| *index);
    let mut raw_results_by_id: HashMap<(PathBuf, String), TestResult> = HashMap::new();
    for (_, batch_results) in raw_batch_results {
        for (test, result) in batch_results {
            raw_results_by_id.insert(case_key(&test), result);
        }
    }

    for test in filtered_tests {
        let raw_result = if let Some(TestMarker::Skip(reason)) =
            test.markers.iter().find(|marker| matches!(marker, TestMarker::Skip(_)))
        {
            TestResult::Skipped(reason.clone())
        } else {
            let id = case_key(&test);
            match raw_results_by_id.remove(&id) {
                Some(result) => result,
                None if stop_on_fail => continue,
                None => TestResult::Failed(
                    Duration::from_secs(0),
                    format!("internal error: no test result was produced for `{id:?}`"),
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

        let file_id = file_ids
            .get(&test.file_path)
            .ok_or_else(|| CliError::failure("selected test lost its file identity"))?;
        let id = format!("{}::{}", file_id.to_string_lossy(), test_display_name(&test));
        results.push(TestCaseResult { id, test, result });
    }

    Ok(TestRunReport {
        summary: TestSummary {
            total: results.len(),
            passed,
            failed,
            skipped,
            xfailed,
            xpassed,
            duration: start_time.elapsed(),
        },
        results,
        shuffle_seed,
        exit_code: if failed > 0 || xpassed > 0 {
            ExitCode::FAILURE.0
        } else {
            ExitCode::SUCCESS.0
        },
    })
}

/// Format completed owner facts in the invoking CLI without reopening source paths or rerunning result policy.
/// Report-file errors belong to the client; a valid execution result is never turned into a cached test success.
pub(crate) fn render_test_report(
    report: TestRunReport,
    config: &TestRunConfig<'_>,
    output: &TestOutputSink,
) -> CliResult<ExitCode> {
    let _rendering = trace_test_stage("client_reporting");
    let TestRunReport {
        results,
        summary,
        shuffle_seed,
        exit_code,
    } = report;
    if results.is_empty() {
        return if exit_code == 0 {
            Ok(ExitCode::SUCCESS)
        } else {
            Err(CliError::with_code("", exit_code))
        };
    }
    let TestSummary {
        passed,
        failed,
        skipped,
        xfailed,
        xpassed,
        duration: total_time,
        ..
    } = summary;
    let report_format = config.report_format;
    let use_color = config.use_color;
    let workspace_context = config.workspace_context.as_ref();
    for case in &results {
        if report_format == TestOutputFormat::Console {
            print_test_result(&case.test, &case.result, config.verbose, use_color, output);
        } else {
            emit_json_result(&case.test, &case.result, &case.id, workspace_context, output);
        }
    }
    let failures: Vec<_> = results
        .iter()
        .filter(|case| matches!(case.result, TestResult::Failed(_, _) | TestResult::XPassed(_)))
        .collect();

    if report_format == TestOutputFormat::Console && !failures.is_empty() {
        output.line("");
        output.line(format_args!(
            "{}",
            style(centered_eq_banner("FAILURES"), RED, use_color)
        ));
        for TestCaseResult { test, result, .. } in failures {
            let display_name = test_display_name(test);
            output.line("");
            output.line(format_args!(
                "{}",
                style(centered_banner(display_name, '_'), "1", use_color)
            ));
            if let TestResult::Failed(_, msg) = result {
                output.line("");
                output.line(format_args!("    {}", msg));
            } else if let TestResult::XPassed(_) = result {
                output.line("");
                output.line(format_args!(
                    "    {}",
                    style("Test passed but was expected to fail (xfail)", YELLOW, use_color)
                ));
            }
            output.line("");
            output.line(format_args!("    {}::{}", test.file_path.display(), display_name));
        }
    }

    if let Some(path) = config.junit_path.as_ref() {
        write_junit_report(path, &results, total_time)?;
    }
    if report_format == TestOutputFormat::Console {
        if let Some(count) = config.durations {
            print_durations(&results, count, use_color, output);
        }
        output.line("");
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
        output.line(format_args!(
            "{}",
            style(centered_eq_banner(&summary_label), summary_color, use_color)
        ));
    } else {
        let mut record = serde_json::json!({
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
        });
        if let (Some(object), Some(workspace)) = (record.as_object_mut(), workspace_context) {
            object.insert(
                "workspace".to_string(),
                serde_json::json!({
                    "root": workspace.workspace_root.display().to_string(),
                    "scope_origin": workspace.scope_origin,
                    "member": {
                        "name": workspace.member_name,
                        "root": workspace.member_root.display().to_string(),
                    },
                }),
            );
        }
        output.line(format_args!("{record}"));
    }

    if exit_code != 0 {
        // The owner has already applied failure policy; the CLI only presents its result.
        Err(CliError::with_code("", exit_code))
    } else {
        Ok(ExitCode::SUCCESS)
    }
}

#[cfg(test)]
mod tests {
    mod envelope_paths;
    mod scheduling;

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

    /// Rust units never enter the generated Incan harness or share its fixture lifetime.
    #[test]
    fn mixed_languages_use_distinct_execution_units() {
        let incan = make_test("test_incan", Vec::new());
        let mut rust = make_test("tests::native", Vec::new());
        rust.file_path = PathBuf::from("tests/native.rs");
        let mut tests = [incan, rust];
        for _ in 0..2 {
            let units = build_execution_units(&tests, Path::new("."), None, false, 1);
            assert_eq!(units.len(), 2);
            assert!(units.iter().all(|unit| unit.tests.len() == 1));
            tests.reverse();
        }
    }

    /// Selection language survives owner transport rather than reverting to the wrapper default.
    #[test]
    fn test_language_selection_survives_owner_transport() -> Result<(), Box<dyn std::error::Error>> {
        for languages in [TestLanguages::Incan, TestLanguages::Rust, TestLanguages::All] {
            let mut config = list_test_config("/test-project");
            config.languages = languages;
            let wire = serde_json::to_vec(&TestExecutionRequest::from_config(&config))?;
            let request: TestExecutionRequest = serde_json::from_slice(&wire)?;
            assert_eq!(request.as_config().languages, languages);
        }
        Ok(())
    }

    /// Typed completion preserves resolved IDs and every status without requiring source files at render time.
    #[test]
    fn typed_completion_renders_original_identity_and_all_statuses() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let source = directory.path().join("test_original.incn");
        std::fs::write(&source, "def test_original(): pass\n")?;
        let duration = Duration::from_millis(7);
        let statuses = [
            TestResult::Passed(duration),
            TestResult::Failed(duration, "failure <details>".to_string()),
            TestResult::Skipped("skip & reason".to_string()),
            TestResult::XFailed(duration, "expected".to_string()),
            TestResult::XPassed(duration),
        ];
        let results = statuses
            .into_iter()
            .enumerate()
            .map(|(index, result)| {
                let mut test = make_test(&format!("test_{index}"), vec![TestMarker::Mark("selected".to_string())]);
                test.file_path = source.clone();
                TestCaseResult {
                    id: format!("retained/test_original.incn::test_{index}"),
                    test,
                    result,
                }
            })
            .collect();
        let report = TestRunReport {
            results,
            summary: TestSummary {
                total: 5,
                passed: 1,
                failed: 1,
                skipped: 1,
                xfailed: 1,
                xpassed: 1,
                duration,
            },
            shuffle_seed: Some(27),
            exit_code: ExitCode::FAILURE.0,
        };
        let wire = serde_json::to_vec(&report)?;
        let restored: TestRunReport = serde_json::from_slice(&wire)?;
        std::fs::remove_file(&source)?;
        let received = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let capture = std::sync::Arc::clone(&received);
        let output = TestOutputSink::new(move |event| {
            if let output::TestOutput::Stdout(text) = event
                && let Ok(mut received) = capture.lock()
            {
                received.push(text);
            }
        });
        let mut config = list_test_config("/unavailable-source-root");
        config.list_only = false;
        config.report_format = TestOutputFormat::Json;
        config.junit_path = Some(directory.path().join("report.xml"));
        assert!(render_test_report(restored, &config, &output).is_err());
        let text = received.lock().map_err(|_| "poisoned output capture")?.concat();
        let records: Vec<serde_json::Value> = text.lines().map(serde_json::from_str).collect::<Result<_, _>>()?;
        assert_eq!(records.len(), 6);
        for (index, status) in ["passed", "failed", "skipped", "xfailed", "xpassed"].iter().enumerate() {
            assert_eq!(
                records[index]["test_id"],
                format!("retained/test_original.incn::test_{index}")
            );
            assert_eq!(records[index]["status"], *status);
        }
        assert_eq!(records[5]["summary"]["shuffle_seed"], 27);
        assert_eq!(records[5]["summary"]["xpassed"], 1);
        let junit = std::fs::read_to_string(directory.path().join("report.xml"))?;
        assert!(junit.contains("failures=\"2\" skipped=\"2\""));
        assert!(junit.contains("failure &lt;details&gt;"));
        assert!(junit.contains("skip &amp; reason"));
        Ok(())
    }

    /// A successful execution cannot hide a client-side report destination failure.
    #[test]
    fn typed_completion_reports_client_junit_failure() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let occupied = directory.path().join("occupied");
        std::fs::write(&occupied, "not a directory")?;
        let report = TestRunReport {
            results: vec![TestCaseResult {
                id: "original::test_one".to_string(),
                test: make_test("test_one", Vec::new()),
                result: TestResult::Passed(Duration::ZERO),
            }],
            summary: TestSummary {
                total: 1,
                passed: 1,
                ..TestSummary::default()
            },
            shuffle_seed: None,
            exit_code: ExitCode::SUCCESS.0,
        };
        let mut config = list_test_config("/unavailable-source-root");
        config.junit_path = Some(occupied.join("report.xml"));
        assert!(render_test_report(report, &config, &TestOutputSink::new(|_| {})).is_err());
        Ok(())
    }

    fn make_case(display_id: &str, rust_args: &str) -> ParametrizeCase {
        ParametrizeCase {
            display_id: display_id.to_string(),
            rust_arguments: rust_args.split(", ").map(str::to_string).collect(),
            parameters: display_id.split('-').map(str::to_string).collect(),
            markers: Vec::new(),
        }
    }

    fn list_test_config(path: &str) -> TestRunConfig<'_> {
        TestRunConfig {
            languages: TestLanguages::Incan,
            path,
            verbose: false,
            stop_on_fail: false,
            include_slow: false,
            filter: None,
            use_color: false,
            fail_on_empty: true,
            list_only: true,
            report_format: TestOutputFormat::Console,
            junit_path: None,
            durations: None,
            shuffle: false,
            seed: None,
            run_xfail: false,
            marker_expr: None,
            strict_markers: false,
            jobs: 1,
            isolate: false,
            test_features: Vec::new(),
            package_features: incan_provider::FeatureSelection::default(),
            sdk_profile: None,
            timeout: None,
            no_capture: false,
            lock_policy: LockPolicy::default(),
            workspace_context: None,
        }
    }

    #[test]
    fn command_collection_lock_and_execution_prepare_one_session_and_parse_each_conftest_once()
    -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let tests = project.path().join("tests");
        std::fs::create_dir_all(&tests)?;
        std::fs::write(
            project.path().join("loaf.toml"),
            "[project]\nname = \"one_test_session\"\nversion = \"0.1.0\"\n",
        )?;
        std::fs::write(
            tests.join("conftest.incn"),
            "const TEST_MARKERS: FrozenList[str] = [\"shared\"]\n",
        )?;
        std::fs::write(
            tests.join("test_first.incn"),
            "def test_first() -> None:\n    assert True\n",
        )?;
        std::fs::write(
            tests.join("test_second.incn"),
            "def test_second() -> None:\n    assert True\n",
        )?;

        TEST_COMMAND_SESSION_CONSTRUCTIONS.with(|count| count.set(0));
        let candidates = discover_test_file_candidates(project.path());
        let authority_root = candidates.command_authority_root()?;
        let representative = candidates
            .representative_path(&authority_root)
            .ok_or("test candidate inventory has no representative")?;
        let package_features = incan_provider::FeatureSelection::default();
        let session = prepare_test_command_session(representative, false, &package_features, None)?;
        let test_files = discover_test_files_with_session(&candidates, &authority_root, session.as_ref());
        assert_eq!(test_files.len(), 2);

        let mut conftest_cache = ConftestDiscoveryCache::default();
        let inventory = collect_test_inventory(
            &test_files,
            project.path(),
            &CollectionEvalContext::default(),
            session.as_ref(),
            &mut conftest_cache,
        )?;
        assert_eq!(inventory.tests.len(), 2);
        assert_eq!(conftest_cache.parse_count, 1);
        assert_eq!(conftest_cache.entries.len(), 1);

        incan_driver::lock::reset_project_lock_collection_metrics();
        let Err(lock_error) = validate_oven_test_lock_policy(
            session.as_ref(),
            representative,
            &LockPolicy::explicit(false, false, true, Vec::new()),
            &package_features,
            None,
        ) else {
            return Err("strict test validation accepted a missing oven.lock".into());
        };
        assert!(lock_error.message.contains("oven.lock is missing"));
        assert_eq!(
            incan_driver::lock::project_lock_collection_counts(),
            (1, 0),
            "strict validation must collect through the command session without discovering another",
        );

        let _execution_context =
            prepare_oven_test_command_context(Arc::clone(&session), representative, &test_files, None)?;
        TEST_COMMAND_SESSION_CONSTRUCTIONS.with(|count| assert_eq!(count.get(), 1));
        Ok(())
    }

    #[test]
    fn list_only_retains_parser_session_without_preparing_execution_authority() -> Result<(), Box<dyn std::error::Error>>
    {
        let project = tempfile::tempdir()?;
        let src = project.path().join("src");
        let tests = project.path().join("tests");
        std::fs::create_dir_all(&src)?;
        std::fs::create_dir_all(&tests)?;
        std::fs::write(
            project.path().join("loaf.toml"),
            "[project]\nname = \"list_without_bake\"\nversion = \"0.1.0\"\n",
        )?;
        std::fs::write(src.join("main.incn"), "def main() -> None:\n    pass\n")?;
        std::fs::write(
            tests.join("test_list.incn"),
            "def test_list() -> None:\n    assert True\n",
        )?;

        let path = project.path().to_str().ok_or("test project path is not UTF-8")?;
        let options = TestCollectionOptions::from_config(&list_test_config(path));
        let root = std::fs::canonicalize(project.path())?;
        let entry = root.join("tests/test_list.incn");
        let mut owner = incan_driver::build::executable::ExecutableBuildOwner::default();
        let session = owner.test_collection_session(&root, &entry, &options.package_features, None)?;
        let report = list_tests_with_owner(&options, &mut owner, |_| {})?;
        assert_eq!(report.ids, ["tests/test_list.incn::test_list"]);

        std::fs::write(
            &entry,
            "def test_list() -> None:\n    assert True\ndef test_added() -> None:\n    assert True\n",
        )?;
        let report = list_tests_with_owner(&options, &mut owner, |_| {})?;
        assert_eq!(report.ids.len(), 2);
        let repeated = owner.test_collection_session(&root, &entry, &options.package_features, None)?;
        assert!(
            Arc::ptr_eq(&session, &repeated),
            "collection must retain its parser-only session"
        );
        assert!(
            !project.path().join("target").exists(),
            "listing tests must not prepare generated execution artifacts"
        );
        Ok(())
    }

    #[test]
    fn common_directory_with_sibling_projects_fails_before_selecting_a_session()
    -> Result<(), Box<dyn std::error::Error>> {
        let common = tempfile::tempdir()?;
        for name in ["alpha", "beta"] {
            let project = common.path().join(name);
            let tests = project.join("tests");
            std::fs::create_dir_all(&tests)?;
            std::fs::write(
                project.join("loaf.toml"),
                format!("[project]\nname = \"{name}\"\nversion = \"0.1.0\"\n"),
            )?;
            std::fs::write(
                tests.join(format!("test_{name}.incn")),
                format!("def test_{name}() -> None:\n    assert True\n"),
            )?;
        }

        let path = common.path().to_str().ok_or("common test path is not UTF-8")?;
        let options = TestCollectionOptions::from_config(&list_test_config(path));
        let mut owner = incan_driver::build::executable::ExecutableBuildOwner::default();
        let Err(error) = list_tests_with_owner(&options, &mut owner, |_| {}) else {
            return Err("a common directory silently selected one sibling project".into());
        };

        assert!(error.message.contains("test path spans multiple Incan projects"));
        assert!(error.message.contains("alpha"));
        assert!(error.message.contains("beta"));
        Ok(())
    }

    /// Execution refuses relative source/report paths before selecting any session or writing output.
    #[test]
    fn execution_request_requires_absolute_paths() -> Result<(), Box<dyn std::error::Error>> {
        let mut config = list_test_config("tests");
        assert!(TestExecutionRequest::from_config(&config).validate_paths().is_err());
        let directory = tempfile::tempdir()?;
        let absolute = directory.path().to_str().ok_or("test path is not UTF-8")?;
        config.path = absolute;
        config.junit_path = Some(PathBuf::from("reports/tests.xml"));
        assert!(TestExecutionRequest::from_config(&config).validate_paths().is_err());
        config.junit_path = Some(directory.path().join("reports/tests.xml"));
        let request = TestExecutionRequest::from_config(&config);
        let decoded: TestExecutionRequest = serde_json::from_value(serde_json::to_value(request)?)?;
        decoded.validate_paths()?;
        assert_eq!(decoded.as_config().junit_path, config.junit_path);
        Ok(())
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

    #[test]
    fn filtered_collection_keeps_keyword_slow_and_marker_selection_independent()
    -> Result<(), Box<dyn std::error::Error>> {
        let tests = vec![
            make_test("test_api", vec![TestMarker::Mark("api".to_string())]),
            make_test(
                "test_api_slow",
                vec![TestMarker::Mark("api".to_string()), TestMarker::Slow],
            ),
            make_test("test_db", vec![TestMarker::Mark("db".to_string())]),
        ];
        let root = Path::new(".");

        let default_selection = filter_collected_tests(tests.clone(), None, false, None, root);
        assert_eq!(
            default_selection
                .iter()
                .map(|test| test.function_name.as_str())
                .collect::<Vec<_>>(),
            vec!["test_api", "test_db"],
            "default collection must exclude only slow tests"
        );

        let keyword_selection = filter_collected_tests(tests.clone(), Some("test_api_slow"), true, None, root);
        assert_eq!(
            keyword_selection
                .iter()
                .map(|test| test.function_name.as_str())
                .collect::<Vec<_>>(),
            vec!["test_api_slow"],
            "keyword selection must remain independent of slow-test selection"
        );

        let tokens = tokenize_marker_expr("api and not slow")?;
        let marker_selection = filter_collected_tests(tests, None, true, Some(&tokens), root);
        assert_eq!(
            marker_selection
                .iter()
                .map(|test| test.function_name.as_str())
                .collect::<Vec<_>>(),
            vec!["test_api"],
            "marker selection must retain the non-slow API case only"
        );
        Ok(())
    }

    #[test]
    fn marker_expression_parser_and_strict_validation_cover_collection_errors() -> Result<(), Box<dyn std::error::Error>>
    {
        let api_test = make_test("test_api", vec![TestMarker::Mark("api".to_string())]);
        let tokens = tokenize_marker_expr("api and not slow")?;
        assert!(marker_expr_matches(&api_test, &tokens)?);

        let known_markers = std::collections::BTreeSet::from(["api".to_string()]);
        validate_markers(&[api_test], &known_markers, true, &marker_expr_names(&tokens))?;

        let unknown_tokens = tokenize_marker_expr("missing")?;
        let Err(error) = validate_markers(&[], &known_markers, true, &marker_expr_names(&unknown_tokens)) else {
            return Err("strict marker validation accepted an unknown marker".into());
        };
        assert!(error.contains("unknown marker `missing`"));

        let malformed_tokens = tokenize_marker_expr("api and (")?;
        let Err(error) = MarkerExprParser::parse(&malformed_tokens, &std::collections::BTreeSet::new()) else {
            return Err("marker parser accepted an unclosed expression".into());
        };
        assert!(error.contains("expected marker name or parenthesized expression"));
        Ok(())
    }

    #[test]
    fn stable_test_ids_are_root_relative_for_file_and_directory_inputs() -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let conventional_path = project.path().join("test_runner_surface.incn");
        std::fs::write(&conventional_path, "def test_beta() -> None:\n    pass\n")?;
        let mut conventional = make_test("test_beta", vec![]);
        conventional.file_path = conventional_path.clone();
        assert_eq!(
            stable_test_id(&conventional, &stable_id_root(&conventional_path)),
            "test_runner_surface.incn::test_beta"
        );

        let inline_path = project.path().join("src/main.incn");
        let inline_parent = inline_path.parent().ok_or("inline source path has no parent")?;
        std::fs::create_dir_all(inline_parent)?;
        std::fs::write(
            &inline_path,
            "module tests:\n    def decorated_inline_case() -> None:\n        pass\n",
        )?;
        let mut inline = make_test("decorated_inline_case", vec![TestMarker::Test]);
        inline.file_path = inline_path;
        assert_eq!(
            stable_test_id(&inline, &stable_id_root(project.path())),
            "src/main.incn::decorated_inline_case"
        );
        Ok(())
    }

    #[test]
    fn skipped_tests_do_not_split_a_compatible_oven_execution_unit() {
        let tests = vec![
            make_test("test_before", vec![]),
            make_test("test_skipped", vec![TestMarker::Skip("not ready".to_string())]),
            make_test("test_after", vec![TestMarker::XFail("known issue".to_string())]),
        ];

        let units = build_execution_units(&tests, Path::new("."), None, false, 1);

        assert_eq!(units.len(), 1);
        assert_eq!(
            units[0]
                .tests
                .iter()
                .map(|test| test.function_name.as_str())
                .collect::<Vec<_>>(),
            vec!["test_before", "test_after"],
        );
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
            shared_threads: None,
            index: 1,
            file_paths: BTreeSet::from([PathBuf::from("test_other.incn")]),
            tests: vec![make_test("test_db", vec![TestMarker::Resource("db".to_string())])],
            conftest_files_by_file: HashMap::new(),
            timeout: None,
            resources: BTreeSet::from(["db".to_string()]),
            serial: false,
        };
        let allowed = ExecutionUnit {
            shared_threads: None,
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
            shared_threads: None,
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
            shared_threads: None,
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
            shared_threads: None,
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
            "same Incan source units share a generated harness and must not overlap"
        );
    }
}
