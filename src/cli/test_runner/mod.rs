//! Test runner implementation (pytest-style).

use std::collections::HashMap;
use std::io::Write;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use crate::cli::{CliError, CliResult, ExitCode};

mod discovery;
mod execution;
mod module_graph;
mod reporter;
mod types;

pub use discovery::{discover_test_files, discover_tests_and_fixtures};
pub use reporter::{ConsoleReporter, TestReporter};
pub use types::{
    DiscoveryResult, FixtureInfo, FixtureScope, ParametrizeCall, ParametrizeCase, TestInfo, TestMarker, TestResult,
    TestRunConfig, TestSummary,
};

use discovery::get_autouse_fixtures;
use execution::run_file_tests_batch;
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
        let parametrize_marker = test
            .markers
            .iter()
            .find(|m| matches!(m, TestMarker::Parametrize(_, _)))
            .cloned();

        if let Some(TestMarker::Parametrize(_, cases)) = parametrize_marker {
            for case in &cases {
                let display_id = format!("{}[{}]", test.function_name, case.display_id);
                let non_parametrize_markers: Vec<TestMarker> = test
                    .markers
                    .iter()
                    .filter(|m| !matches!(m, TestMarker::Parametrize(_, _)))
                    .cloned()
                    .collect();

                out.push(TestInfo {
                    file_path: test.file_path.clone(),
                    function_name: test.function_name.clone(),
                    markers: non_parametrize_markers,
                    required_fixtures: test.required_fixtures.clone(),
                    parametrize_call: Some(ParametrizeCall {
                        display_id,
                        rust_args: case.rust_args.clone(),
                    }),
                });
            }
        } else {
            out.push(test);
        }
    }

    out
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
        locked,
        frozen,
        cargo_features,
        cargo_no_default_features,
        cargo_all_features,
    } = config;

    let start_time = Instant::now();

    let test_files = discover_test_files(Path::new(path));

    if test_files.is_empty() {
        return Err(CliError::failure(format!(
            "No test files found in '{}'\nTest files should be named test_*.incn or *_test.incn",
            path
        )));
    }

    let mut all_tests: Vec<TestInfo> = Vec::new();
    let mut all_fixtures: HashMap<String, FixtureInfo> = HashMap::new();

    for file_path in &test_files {
        match discover_tests_and_fixtures(file_path) {
            Ok(result) => {
                all_tests.extend(result.tests);
                for fixture in result.fixtures {
                    all_fixtures.insert(fixture.name.clone(), fixture);
                }
            }
            Err(e) => {
                eprintln!("Error parsing {}: {}", file_path.display(), e);
            }
        }
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

    let autouse_fixtures = get_autouse_fixtures(&all_fixtures, FixtureScope::Function);

    let all_tests: Vec<TestInfo> = all_tests
        .into_iter()
        .map(|mut test| {
            for autouse in &autouse_fixtures {
                if !test.required_fixtures.contains(autouse) {
                    test.required_fixtures.push(autouse.clone());
                }
            }
            test
        })
        .collect();

    // ---- Expand parametrized tests into individual variants ----
    let all_tests = expand_parametrized_tests(all_tests);

    let filtered_tests: Vec<TestInfo> = all_tests
        .into_iter()
        .filter(|t| {
            if let Some(keyword) = filter
                && !t.function_name.contains(keyword)
            {
                return false;
            }
            if !include_slow && t.markers.contains(&TestMarker::Slow) {
                return false;
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

    println!("{}", style(centered_eq_banner("test session starts"), "1", use_color));
    println!("collected {} item(s)", filtered_tests.len());
    println!();

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
            print_test_result(test, &result, verbose, use_color);
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
            idx += 1;
        }

        let batch = &filtered_tests[batch_start..idx];
        println!(
            "{}",
            style(
                format!("running {} ({} item(s))", file_path.display(), batch.len()),
                "2",
                use_color
            )
        );
        let _ = std::io::stdout().flush();
        let batch_results = run_file_tests_batch(
            batch,
            &mut prep_cache,
            locked,
            frozen,
            &cargo_features,
            cargo_no_default_features,
            cargo_all_features,
            stop_on_fail,
        );

        let mut stop_after_batch = false;
        for (test, result) in batch_results {
            let is_xfail = test.markers.iter().any(|m| matches!(m, TestMarker::XFail(_)));

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

            print_test_result(&test, &result, verbose, use_color);
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

    if !failures.is_empty() {
        println!();
        println!("{}", style(centered_eq_banner("FAILURES"), RED, use_color));
        for (test, result) in failures {
            let display_name = test
                .parametrize_call
                .as_ref()
                .map_or_else(|| test.function_name.as_str(), |p| p.display_id.as_str());
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
    println!();
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

    let summary_label = format!("{} in {:.2}s", parts.join(", "), total_time.as_secs_f64());
    let summary_color = if failed > 0 || xpassed > 0 { RED } else { GREEN };
    println!(
        "{}",
        style(centered_eq_banner(&summary_label), summary_color, use_color)
    );

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
            parametrize_call: None,
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
        let cases = vec![
            ParametrizeCase {
                display_id: "1-2-3".to_string(),
                rust_args: "1, 2, 3".to_string(),
            },
            ParametrizeCase {
                display_id: "0-0-0".to_string(),
                rust_args: "0, 0, 0".to_string(),
            },
        ];
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
        let cases = vec![ParametrizeCase {
            display_id: "1".to_string(),
            rust_args: "1".to_string(),
        }];
        let markers = vec![TestMarker::Slow, TestMarker::Parametrize("x".to_string(), cases)];
        let tests = vec![make_test("test_slow_param", markers)];
        let expanded = expand_parametrized_tests(tests);

        assert_eq!(expanded.len(), 1);
        assert_eq!(expanded[0].markers, vec![TestMarker::Slow]);
        assert!(expanded[0].parametrize_call.is_some());
    }

    #[test]
    fn expand_mixed_plain_and_parametrized() {
        let cases = vec![
            ParametrizeCase {
                display_id: "a".to_string(),
                rust_args: "\"a\".to_string()".to_string(),
            },
            ParametrizeCase {
                display_id: "b".to_string(),
                rust_args: "\"b\".to_string()".to_string(),
            },
        ];
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
