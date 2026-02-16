use std::fmt;

use super::types::{TestInfo, TestResult, TestSummary};

/// Trait for reporting test execution results.
///
/// Implement this trait to customize test output format (JSON, TAP, etc.)
pub trait TestReporter {
    /// Called when test discovery begins.
    fn on_discovery_start(&mut self, _path: &str) {}

    /// Called when a test file is discovered.
    fn on_file_discovered(&mut self, _path: &std::path::Path) {}

    /// Called when test collection is complete.
    fn on_collection_complete(&mut self, test_count: usize);

    /// Called when a test run begins.
    fn on_test_start(&mut self, test: &TestInfo);

    /// Called when a test completes.
    fn on_test_complete(&mut self, test: &TestInfo, result: &TestResult);

    /// Called when all tests have completed.
    fn on_run_complete(&mut self, summary: &TestSummary);
}

/// Default console reporter (pytest-style).
#[derive(Default)]
pub struct ConsoleReporter {
    pub verbose: bool,
    pub use_color: bool,
}

impl ConsoleReporter {
    pub fn new(verbose: bool, use_color: bool) -> Self {
        Self { verbose, use_color }
    }
}

pub(crate) fn style<T: fmt::Display>(text: T, code: &str, use_color: bool) -> String {
    if use_color {
        format!("\x1b[{}m{}\x1b[0m", code, text)
    } else {
        text.to_string()
    }
}

impl TestReporter for ConsoleReporter {
    fn on_collection_complete(&mut self, test_count: usize) {
        if test_count == 0 {
            eprintln!("No tests collected");
        }
    }

    fn on_test_start(&mut self, test: &TestInfo) {
        if self.verbose {
            eprint!("{} ... ", test.function_name);
        }
    }

    fn on_test_complete(&mut self, test: &TestInfo, result: &TestResult) {
        let use_color = self.use_color;
        let status = match result {
            TestResult::Passed(d) => {
                if self.verbose {
                    format!("{} ({:.0}ms)", style("PASSED", "32", use_color), d.as_millis())
                } else {
                    style(".", "32", use_color)
                }
            }
            TestResult::Failed(d, _) => {
                if self.verbose {
                    format!("{} ({:.0}ms)", style("FAILED", "31", use_color), d.as_millis())
                } else {
                    style("F", "31", use_color)
                }
            }
            TestResult::Skipped(reason) => {
                if reason.is_empty() {
                    style("SKIPPED", "33", use_color)
                } else {
                    format!("{} ({})", style("SKIPPED", "33", use_color), reason)
                }
            }
            TestResult::XFailed(_, reason) => {
                format!("{} ({})", style("XFAIL", "33", use_color), reason)
            }
            TestResult::XPassed(_) => style("XPASS", "31", use_color),
        };

        if self.verbose {
            eprintln!("{}", status);
        } else {
            eprint!("{}", status);
        }

        // Print failure details.
        if let TestResult::Failed(_, error) = result {
            eprintln!("\n{}", style(&test.function_name, "31", use_color));
            eprintln!("{}", error);
        }
    }

    fn on_run_complete(&mut self, summary: &TestSummary) {
        let use_color = self.use_color;
        if !self.verbose {
            eprintln!();
        }
        eprintln!();

        let mut parts = Vec::new();
        if summary.passed > 0 {
            parts.push(style(format!("{} passed", summary.passed), "32", use_color));
        }
        if summary.failed > 0 {
            parts.push(style(format!("{} failed", summary.failed), "31", use_color));
        }
        if summary.skipped > 0 {
            parts.push(style(format!("{} skipped", summary.skipped), "33", use_color));
        }
        if summary.xfailed > 0 {
            parts.push(style(format!("{} xfailed", summary.xfailed), "33", use_color));
        }

        eprintln!(
            "====== {} in {:.2}s ======",
            parts.join(", "),
            summary.duration.as_secs_f64()
        );
    }
}

pub(crate) fn print_test_result(test: &TestInfo, result: &TestResult, verbose: bool, use_color: bool) {
    let file_stem = test.file_path.file_name().and_then(|n| n.to_str()).unwrap_or("unknown");

    let status = match result {
        TestResult::Passed(d) => {
            if verbose {
                format!("{} ({:.0}ms)", style("PASSED", "32", use_color), d.as_millis())
            } else {
                style("PASSED", "32", use_color)
            }
        }
        TestResult::Failed(d, _) => {
            if verbose {
                format!("{} ({:.0}ms)", style("FAILED", "31", use_color), d.as_millis())
            } else {
                style("FAILED", "31", use_color)
            }
        }
        TestResult::Skipped(reason) => {
            if reason.is_empty() {
                style("SKIPPED", "33", use_color)
            } else {
                format!("{} ({})", style("SKIPPED", "33", use_color), reason)
            }
        }
        TestResult::XFailed(_, reason) => {
            if reason.is_empty() {
                style("XFAIL", "33", use_color)
            } else {
                format!("{} ({})", style("XFAIL", "33", use_color), reason)
            }
        }
        TestResult::XPassed(_) => style("XPASS", "31", use_color),
    };

    let display_name = test
        .parametrize_call
        .as_ref()
        .map_or_else(|| test.function_name.as_str(), |p| p.display_id.as_str());
    println!("{}::{} {}", file_stem, display_name, status);
}
