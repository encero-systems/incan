//! Native test inventory and exact execution for Oven Alpha's direct-rustc consumers.
//!
//! Oven executes the libtest binary it built itself. It obtains a real inventory first and rejects a requested exact
//! test absent from that inventory, so a zero-match filter can never become a misleading success. Neither collection
//! nor execution launches Cargo or inherits Cargo process state.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{self, BufRead, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use serde::Serialize;

use super::process::{isolate_process_group, terminate_process_group};
use super::rustc::clear_inherited_cargo_environment;

/// Inventory returned by one exact native libtest binary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OvenNativeTestInventory {
    /// Complete deterministic set of test names reported by the binary.
    pub names: Vec<String>,
}

/// Exact native test execution request.
#[derive(Debug, Clone)]
pub struct OvenNativeTestRequest {
    /// Caller-owned direct-rustc libtest binary.
    pub executable: PathBuf,
    /// Names that must occur in the real binary inventory before execution begins.
    pub exact_names: Vec<String>,
    /// Compiler-owned environment replacements for the test process.
    ///
    /// These are applied after inherited Cargo variables are removed. They let a receipt-bound suite pin paths such
    /// as its source checkout without making ambient shell configuration part of test correctness.
    pub environment: BTreeMap<String, String>,
    /// Maximum wall-clock duration for one generated native execution group.
    ///
    /// The generated group remains one libtest process so session-scoped fixture behaviour is preserved. When this
    /// deadline expires Oven terminates that child and returns its captured partial transcript plus a timeout record.
    pub timeout: Option<Duration>,
}

/// Successful native-test execution record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OvenNativeTestReport {
    /// Complete inventory consulted before exact selection.
    pub inventory: OvenNativeTestInventory,
    /// Exact test names successfully executed by the native binary.
    pub passed: Vec<String>,
}

/// Terminal libtest case counts reported by one native batch.
///
/// These are parsed from the batch process output already captured for diagnostics. Oven does not launch another
/// process or scan retained caller files merely to report green coverage.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct OvenNativeTestCaseCounts {
    /// Cases that completed successfully.
    pub passed: usize,
    /// Cases that completed with an assertion or harness failure.
    pub failed: usize,
    /// Cases intentionally ignored by libtest.
    pub ignored: usize,
}

/// One elapsed time reported by the libtest process that Oven already executes.
///
/// Case timings are diagnostic-only and are collected only when the caller opts into libtest's unstable JSON
/// `--report-time` output. They never cause Oven to launch an additional test process.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OvenNativeTestCaseTiming {
    /// Exact test name from the verified native-test inventory.
    pub name: String,
    /// Elapsed wall-clock time reported by libtest, rounded to its millisecond precision.
    pub elapsed_ms: u64,
}

/// One nested Incan command duration emitted by an explicitly instrumented integration test.
///
/// The native runner only parses these diagnostic records from the libtest transcript it already captured. It never
/// wraps, schedules, or reruns the nested command itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OvenNativeTestCommandTiming {
    /// Libtest case that started the nested command.
    pub test_name: String,
    /// Stable command label supplied by the integration-test helper.
    pub command: String,
    /// Nested command wall-clock duration in milliseconds.
    pub elapsed_ms: u64,
    /// Opt-in command-internal timing phases emitted by a JSON build report.
    ///
    /// Empty means the nested command did not produce a build report; it is not a zero-duration claim.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub phase_timings_ms: BTreeMap<String, u64>,
}

/// One verified all-in-one native libtest execution used when fixture scope requires a shared process.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OvenNativeTestBatchReport {
    /// Complete inventory consulted before execution.
    pub inventory: OvenNativeTestInventory,
    /// Whether libtest reported an all-green result.
    pub success: bool,
    /// Whether Oven terminated the native execution group after its configured deadline.
    pub timed_out: bool,
    /// Case counts from libtest's final summary when it emitted one.
    ///
    /// A native test executable may exit before libtest can produce a summary, so absence is represented explicitly
    /// rather than fabricating green counts from its inventory.
    pub case_counts: Option<OvenNativeTestCaseCounts>,
    /// Per-case libtest timings when `INCAN_OVEN_NATIVE_TEST_CASE_TIMINGS` enabled their collection.
    ///
    /// An empty vector means timing diagnostics were not requested or the libtest process did not emit a terminal
    /// timing line for an inventory case. It does not mean that the case took zero time.
    pub case_timings: Vec<OvenNativeTestCaseTiming>,
    /// Opt-in nested command timings emitted by existing integration-test helpers.
    pub command_timings: Vec<OvenNativeTestCommandTiming>,
    /// Wall-clock timing for the two native-test subprocess phases already required by this execution.
    ///
    /// The values are observational only: Oven does not launch another process or rerun a test merely to populate
    /// them. They let compiler-suite evidence distinguish a slow inventory from slow test execution.
    pub timing: OvenNativeTestBatchTiming,
    /// Combined libtest transcript retained for per-test result mapping by the caller.
    pub output: String,
}

/// Wall-clock timings for one all-in-one native libtest batch.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct OvenNativeTestBatchTiming {
    /// Time spent obtaining the binary's exact libtest inventory.
    pub inventory_elapsed_ms: u64,
    /// Time spent executing the verified native libtest process.
    pub execution_elapsed_ms: u64,
}

/// Opt into libtest's per-case elapsed-time diagnostics for one Oven invocation.
///
/// This is deliberately opt-in because libtest requires its unstable reporting switch. Oven retains its normal
/// output and execution behaviour by default.
pub const OVEN_NATIVE_TEST_CASE_TIMINGS_ENV: &str = "INCAN_OVEN_NATIVE_TEST_CASE_TIMINGS";

/// Error while obtaining native test inventory or executing an exact test.
#[derive(Debug, thiserror::Error)]
pub enum OvenNativeTestError {
    /// The caller supplied an invalid executable path or duplicate/empty exact test name.
    #[error("invalid Oven native-test {field}: {message}")]
    InvalidInput { field: &'static str, message: String },
    /// Starting or reading a native-test process failed.
    #[error("Oven native-test I/O failed at {path}: {source}")]
    Io { path: PathBuf, source: io::Error },
    /// The native binary could not produce a valid libtest inventory.
    #[error("Oven native-test inventory failed: {output}")]
    InventoryFailed { output: String },
    /// The requested exact test did not occur in the binary's verified inventory.
    #[error("Oven native-test exact selection `{name}` is absent from the binary inventory")]
    MissingExactTest { name: String },
    /// An exact test ran but reported a failure.
    #[error("Oven native-test `{name}` failed: {output}")]
    TestFailed { name: String, output: String },
}

/// Obtain a complete deterministic native libtest inventory without a Cargo process.
pub fn inventory_native_tests(executable: &Path) -> Result<OvenNativeTestInventory, OvenNativeTestError> {
    inventory_native_tests_with_environment(executable, &BTreeMap::new(), None, false)
}

/// Inventory a native libtest binary after applying its explicit, Cargo-free process environment.
///
/// This is necessary for test roots such as proc-macro crates, whose direct-rustc binary links the receipt-selected
/// toolchain dynamic standard library. The environment is never inherited from Cargo.
fn inventory_native_tests_with_environment(
    executable: &Path,
    environment: &BTreeMap<String, String>,
    working_directory: Option<&Path>,
    allow_empty: bool,
) -> Result<OvenNativeTestInventory, OvenNativeTestError> {
    inventory_native_tests_with_environment_and_timeout(executable, environment, working_directory, allow_empty, None)
}

/// Inventory one native libtest binary under the same process-tree supervisor as its selected test cases.
fn inventory_native_tests_with_environment_and_timeout(
    executable: &Path,
    environment: &BTreeMap<String, String>,
    working_directory: Option<&Path>,
    allow_empty: bool,
    timeout: Option<Duration>,
) -> Result<OvenNativeTestInventory, OvenNativeTestError> {
    let executable = verified_executable(executable)?;
    let mut command = Command::new(&executable);
    command.args(["--list", "--format", "terse"]);
    if let Some(working_directory) = working_directory {
        command.current_dir(working_directory);
    }
    clear_inherited_cargo_environment(&mut command);
    command.envs(environment);
    // No reporter: this lists cases, it does not run them, so there is no progress to render.
    let (output, timed_out) = run_native_batch_child(command, &executable, timeout, None)?;
    let mut transcript = combined_output(&output.stdout, &output.stderr);
    if timed_out {
        if !transcript.ends_with('\n') && !transcript.is_empty() {
            transcript.push('\n');
        }
        if let Some(timeout) = timeout {
            transcript.push_str(&format!(
                "Oven native test inventory timed out after {} (executable: {})\n",
                format_timeout(timeout),
                executable.display(),
            ));
        }
    }
    if !output.status.success() || timed_out {
        return Err(OvenNativeTestError::InventoryFailed { output: transcript });
    }
    let names = parse_inventory(&output.stdout, allow_empty)?;
    Ok(OvenNativeTestInventory { names })
}

/// Run exact native tests only after every requested name occurs in the verified binary inventory.
pub fn run_native_tests(request: &OvenNativeTestRequest) -> Result<OvenNativeTestReport, OvenNativeTestError> {
    let inventory = inventory_native_tests_with_environment(&request.executable, &request.environment, None, false)?;
    let requested = normalized_exact_names(&request.exact_names)?;
    let available = inventory.names.iter().collect::<BTreeSet<_>>();
    for name in &requested {
        if !available.contains(name) {
            return Err(OvenNativeTestError::MissingExactTest { name: name.clone() });
        }
    }

    for name in &requested {
        let mut command = Command::new(&request.executable);
        command.args(["--exact", name, "--nocapture"]);
        clear_inherited_cargo_environment(&mut command);
        command.envs(&request.environment);
        let output = command.output().map_err(|source| OvenNativeTestError::Io {
            path: request.executable.clone(),
            source,
        })?;
        if !output.status.success() {
            return Err(OvenNativeTestError::TestFailed {
                name: name.clone(),
                output: combined_output(&output.stdout, &output.stderr),
            });
        }
    }
    Ok(OvenNativeTestReport {
        inventory,
        passed: requested,
    })
}

/// Run one generated batch in a single native libtest process after verifying its exact expected inventory.
///
/// This preserves session-scoped fixture behaviour. Generated Incan file batches can share registration and fixture
/// initialization between their native Rust `#[test]` functions, so the batch itself runs one test at a time while
/// the outer scheduler remains free to run independent files in parallel. The caller may parse the returned libtest
/// transcript into its own richer test-reporting format; a test assertion failure is represented as `success: false`,
/// not as a transport error that would hide results for later tests in the same batch.
pub fn run_native_test_batch(
    request: &OvenNativeTestRequest,
) -> Result<OvenNativeTestBatchReport, OvenNativeTestError> {
    let inventory_started = Instant::now();
    let inventory = inventory_native_tests_with_environment(&request.executable, &request.environment, None, false)?;
    let inventory_elapsed_ms = duration_millis(inventory_started.elapsed());
    let requested = normalized_exact_names(&request.exact_names)?;
    let available = inventory.names.iter().collect::<BTreeSet<_>>();
    for name in &requested {
        if !available.contains(name) {
            return Err(OvenNativeTestError::MissingExactTest { name: name.clone() });
        }
    }
    let executable = verified_executable(&request.executable)?;
    let mut command = Command::new(&executable);
    command.args(["--test-threads=1", "--nocapture"]);
    clear_inherited_cargo_environment(&mut command);
    command.envs(&request.environment);
    let structured_events = add_case_timing_diagnostics(&mut command);
    let execution_started = Instant::now();
    let (output, timed_out) = run_native_batch_child(
        command,
        &executable,
        request.timeout,
        structured_events.then(NativeTestProgressReporter::new),
    )?;
    let execution_elapsed_ms = duration_millis(execution_started.elapsed());
    let mut transcript = combined_output(&output.stdout, &output.stderr);
    if let Some(timeout) = timed_out.then_some(request.timeout).flatten() {
        if !transcript.ends_with('\n') && !transcript.is_empty() {
            transcript.push('\n');
        }
        transcript.push_str(&format!(
            "Oven native test execution group timed out after {}\n",
            format_timeout(timeout)
        ));
    }
    let case_timings = parse_libtest_case_timings_if_requested(&transcript, &inventory);
    let command_timings = parse_native_test_command_timings(&transcript);
    Ok(OvenNativeTestBatchReport {
        inventory,
        success: output.status.success() && !timed_out,
        timed_out,
        case_counts: parse_libtest_case_counts(&transcript),
        case_timings,
        command_timings,
        timing: OvenNativeTestBatchTiming {
            inventory_elapsed_ms,
            execution_elapsed_ms,
        },
        output: transcript,
    })
}

/// Execute receipt-selected native tests after verifying one complete inventory, with the target's authored working
/// directory and the compiler-suite timeout supervisor.
///
/// This is the narrow diagnostic counterpart to the complete-root runner. Every requested name must occur in the
/// same verified inventory before any case starts. The cases then run sequentially from the same executable while
/// retaining normal Cargo sanitization, output capture, and process-group deadlines. Their terminal summaries and
/// timings are aggregated without manufacturing one root per selected case.
pub fn run_native_tests_exact_in_directory_with_timeout(
    executable: &Path,
    exact_names: &[String],
    environment: &BTreeMap<String, String>,
    working_directory: Option<&Path>,
    timeout: Option<Duration>,
) -> Result<OvenNativeTestBatchReport, OvenNativeTestError> {
    let selection_started = Instant::now();
    let requested = normalized_exact_names(exact_names)?;
    let inventory_started = Instant::now();
    let inventory_timeout = remaining_native_test_timeout(&selection_started, timeout).map_err(|timeout| {
        OvenNativeTestError::InventoryFailed {
            output: format!(
                "Oven native exact selection exhausted its aggregate {} deadline before inventory started",
                format_timeout(timeout)
            ),
        }
    })?;
    let inventory = inventory_native_tests_with_environment_and_timeout(
        executable,
        environment,
        working_directory,
        false,
        inventory_timeout,
    )?;
    let inventory_elapsed_ms = duration_millis(inventory_started.elapsed());
    let available = inventory.names.iter().collect::<BTreeSet<_>>();
    for exact_name in &requested {
        if !available.contains(exact_name) {
            return Err(OvenNativeTestError::MissingExactTest {
                name: exact_name.clone(),
            });
        }
    }
    let executable = verified_executable(executable)?;
    let mut report = OvenNativeTestBatchReport {
        inventory: inventory.clone(),
        success: true,
        timed_out: false,
        case_counts: Some(OvenNativeTestCaseCounts::default()),
        case_timings: Vec::new(),
        command_timings: Vec::new(),
        timing: OvenNativeTestBatchTiming {
            inventory_elapsed_ms,
            execution_elapsed_ms: 0,
        },
        output: String::new(),
    };
    for exact_name in requested {
        let process_timeout = match remaining_native_test_timeout(&selection_started, timeout) {
            Ok(timeout) => timeout,
            Err(timeout) => {
                report.success = false;
                report.timed_out = true;
                report.case_counts = None;
                if !report.output.is_empty() && !report.output.ends_with('\n') {
                    report.output.push('\n');
                }
                report
                    .output
                    .push_str(&format!("--- Oven exact test `{exact_name}` ---\n"));
                report.output.push_str(&format!(
                    "Oven native exact selection exhausted its aggregate {} deadline before this case could start; no later selected case was launched\n",
                    format_timeout(timeout)
                ));
                break;
            }
        };
        let mut command = Command::new(&executable);
        command.args(["--exact", &exact_name, "--nocapture"]);
        if let Some(working_directory) = working_directory {
            command.current_dir(working_directory);
        }
        clear_inherited_cargo_environment(&mut command);
        command.envs(environment);
        let structured_events = add_case_timing_diagnostics(&mut command);
        let execution_started = Instant::now();
        let (output, timed_out) = run_native_batch_child(
            command,
            &executable,
            process_timeout,
            structured_events.then(NativeTestProgressReporter::new),
        )?;
        report.timing.execution_elapsed_ms = report
            .timing
            .execution_elapsed_ms
            .saturating_add(duration_millis(execution_started.elapsed()));
        let mut transcript = combined_output(&output.stdout, &output.stderr);
        if timed_out {
            if !transcript.ends_with('\n') && !transcript.is_empty() {
                transcript.push('\n');
            }
            if let Some(timeout) = timeout {
                let working_directory = working_directory
                    .map(|path| path.display().to_string())
                    .unwrap_or_else(|| "inherited".to_string());
                transcript.push_str(&format!(
                    "Oven native exact selection exhausted its aggregate {} deadline while executing `{exact_name}` (executable: {}; working directory: {}); no later selected case was launched\n",
                    format_timeout(timeout),
                    executable.display(),
                    working_directory,
                ));
            }
        }
        report.success &= output.status.success() && !timed_out;
        report.timed_out |= timed_out;
        match (&mut report.case_counts, parse_libtest_case_counts(&transcript)) {
            (Some(total), Some(counts)) => {
                total.passed = total.passed.saturating_add(counts.passed);
                total.failed = total.failed.saturating_add(counts.failed);
                total.ignored = total.ignored.saturating_add(counts.ignored);
            }
            (counts, None) => *counts = None,
            (None, Some(_)) => {}
        }
        report
            .case_timings
            .extend(parse_libtest_case_timings_if_requested(&transcript, &inventory));
        report
            .command_timings
            .extend(parse_native_test_command_timings(&transcript));
        if !report.output.is_empty() && !report.output.ends_with('\n') {
            report.output.push('\n');
        }
        report
            .output
            .push_str(&format!("--- Oven exact test `{exact_name}` ---\n"));
        report.output.push_str(&transcript);
        if timed_out {
            break;
        }
    }
    Ok(report)
}

/// Return the remaining wall-clock allowance for one exact selection, including inventory and every selected case.
fn remaining_native_test_timeout(started: &Instant, timeout: Option<Duration>) -> Result<Option<Duration>, Duration> {
    let Some(timeout) = timeout else {
        return Ok(None);
    };
    timeout
        .checked_sub(started.elapsed())
        .filter(|remaining| !remaining.is_zero())
        .map(Some)
        .ok_or(timeout)
}

/// Execute one receipt-selected native test through the multi-exact supervisor.
pub fn run_native_test_exact_in_directory_with_timeout(
    executable: &Path,
    exact_name: &str,
    environment: &BTreeMap<String, String>,
    working_directory: Option<&Path>,
    timeout: Option<Duration>,
) -> Result<OvenNativeTestBatchReport, OvenNativeTestError> {
    run_native_tests_exact_in_directory_with_timeout(
        executable,
        &[exact_name.to_string()],
        environment,
        working_directory,
        timeout,
    )
}

/// Inventory and execute every test in one native libtest binary, accepting a valid zero-test target.
///
/// Cargo accepts a compiled test root with no `#[test]` functions; Oven must do the same for workspace proc-macro
/// roots. The binary is still inventoried and launched with Cargo state removed, so an empty inventory is not treated
/// as an unverified success.
pub fn run_native_test_batch_all(
    executable: &Path,
    environment: &BTreeMap<String, String>,
) -> Result<OvenNativeTestBatchReport, OvenNativeTestError> {
    run_native_test_batch_all_in_directory(executable, environment, None)
}

/// Inventory and execute every test from one verified caller-selected package directory.
///
/// Cargo launches each test target from its package manifest directory. Stored direct-rustc test binaries must retain
/// that authored working-directory contract: snapshot and fixture tests commonly use paths relative to the package
/// root, while Oven's executable output remains caller-owned and must not become an implicit source root.
pub fn run_native_test_batch_all_in_directory(
    executable: &Path,
    environment: &BTreeMap<String, String>,
    working_directory: Option<&Path>,
) -> Result<OvenNativeTestBatchReport, OvenNativeTestError> {
    run_native_test_batch_all_in_directory_with_timeout(executable, environment, working_directory, None)
}

/// Inventory and execute every test from one verified caller-selected package directory with an optional deadline.
///
/// The compiler-suite scheduler uses the deadline-bearing form so a platform-specific child cannot hold the whole
/// bounded worker pool indefinitely. Ordinary callers retain the historical no-deadline wrapper above.
pub fn run_native_test_batch_all_in_directory_with_timeout(
    executable: &Path,
    environment: &BTreeMap<String, String>,
    working_directory: Option<&Path>,
    timeout: Option<Duration>,
) -> Result<OvenNativeTestBatchReport, OvenNativeTestError> {
    run_native_test_batch_all_in_directory_with_options(executable, environment, working_directory, timeout, None)
}

/// Inventory and execute every test from one verified caller-selected package directory with a fixed libtest budget.
///
/// The compiler-suite scheduler uses this form after splitting its host CPU budget between independent root workers.
/// A zero budget is rejected before launching a child, so a caller cannot turn a scheduling error into libtest's
/// less actionable command-line diagnostic.
pub fn run_native_test_batch_all_in_directory_with_timeout_and_threads(
    executable: &Path,
    environment: &BTreeMap<String, String>,
    working_directory: Option<&Path>,
    timeout: Option<Duration>,
    test_threads: usize,
) -> Result<OvenNativeTestBatchReport, OvenNativeTestError> {
    if test_threads == 0 {
        return Err(OvenNativeTestError::InvalidInput {
            field: "native test thread budget",
            message: "must be greater than zero".to_string(),
        });
    }
    run_native_test_batch_all_in_directory_with_options(
        executable,
        environment,
        working_directory,
        timeout,
        Some(test_threads),
    )
}

/// Shared executor for ordinary and scheduler-budgeted complete native-test roots.
fn run_native_test_batch_all_in_directory_with_options(
    executable: &Path,
    environment: &BTreeMap<String, String>,
    working_directory: Option<&Path>,
    timeout: Option<Duration>,
    test_threads: Option<usize>,
) -> Result<OvenNativeTestBatchReport, OvenNativeTestError> {
    let inventory_started = Instant::now();
    let inventory = inventory_native_tests_with_environment(executable, environment, working_directory, true)?;
    let inventory_elapsed_ms = duration_millis(inventory_started.elapsed());
    let executable = verified_executable(executable)?;
    let mut command = Command::new(&executable);
    if let Some(test_threads) = test_threads {
        command.arg(format!("--test-threads={test_threads}"));
    }
    command.arg("--nocapture");
    if let Some(working_directory) = working_directory {
        command.current_dir(working_directory);
    }
    clear_inherited_cargo_environment(&mut command);
    command.envs(environment);
    let structured_events = add_case_timing_diagnostics(&mut command);
    // Progress rendering depends on the structured event stream, so it follows the same switch. Without it libtest
    // emits its own human transcript, which this must not duplicate or reorder.
    let reporter = structured_events.then(NativeTestProgressReporter::new);
    let execution_started = Instant::now();
    let (output, timed_out) = run_native_batch_child(command, &executable, timeout, reporter)?;
    let execution_elapsed_ms = duration_millis(execution_started.elapsed());
    let transcript = combined_output(&output.stdout, &output.stderr);
    let mut transcript = transcript;
    if timed_out {
        if !transcript.ends_with('\n') && !transcript.is_empty() {
            transcript.push('\n');
        }
        if let Some(timeout) = timeout {
            let working_directory = working_directory
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "inherited".to_string());
            transcript.push_str(&format!(
                "Oven native test execution group timed out after {} (executable: {}; working directory: {})\n",
                format_timeout(timeout),
                executable.display(),
                working_directory,
            ));
        }
    }
    let case_timings = parse_libtest_case_timings_if_requested(&transcript, &inventory);
    let command_timings = parse_native_test_command_timings(&transcript);
    Ok(OvenNativeTestBatchReport {
        inventory,
        success: output.status.success() && !timed_out,
        timed_out,
        case_counts: parse_libtest_case_counts(&transcript),
        case_timings,
        command_timings,
        timing: OvenNativeTestBatchTiming {
            inventory_elapsed_ms,
            execution_elapsed_ms,
        },
        output: transcript,
    })
}

/// Convert a measured duration to reportable milliseconds without a lossy platform-width conversion.
fn duration_millis(duration: Duration) -> u64 {
    duration
        .as_secs()
        .saturating_mul(1_000)
        .saturating_add(u64::from(duration.subsec_millis()))
}

/// Add libtest's report-time switches only for an explicit diagnostic invocation.
///
/// `--report-time` and JSON output are still unstable in libtest. The opt-in marker itself is removed from the child
/// environment so nested normal Incan commands cannot recursively opt into timing output. The bootstrap
/// compatibility environment applies to the direct native test process and its ordinary process descendants only for
/// this diagnostic run.
fn add_case_timing_diagnostics(command: &mut Command) -> bool {
    let requested = std::env::var_os(OVEN_NATIVE_TEST_CASE_TIMINGS_ENV).is_some();
    command.env_remove(OVEN_NATIVE_TEST_CASE_TIMINGS_ENV);
    if requested {
        command.args(["-Z", "unstable-options", "--format", "json", "--report-time"]);
        command.env("RUSTC_BOOTSTRAP", "1");
    }
    requested
}

/// Parse JSON per-case libtest elapsed times for the inventory that Oven itself verified.
///
/// The outer diagnostic runner writes JSON only because it received the explicit `--format json` argument. Nested
/// programs retain normal text output because the opt-in marker is removed before their parent test process starts.
/// Restricting parsed events to the outer binary's exact inventory is a second, independent safeguard.
fn parse_libtest_case_timings(output: &str, inventory: &OvenNativeTestInventory) -> Vec<OvenNativeTestCaseTiming> {
    let inventory_names = inventory.names.iter().map(String::as_str).collect::<BTreeSet<_>>();
    let mut timings = BTreeMap::new();
    for line in output.lines() {
        let Ok(event) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if event.get("type").and_then(serde_json::Value::as_str) != Some("test")
            || !matches!(
                event.get("event").and_then(serde_json::Value::as_str),
                Some("ok" | "failed")
            )
        {
            continue;
        }
        let Some(name) = event.get("name").and_then(serde_json::Value::as_str) else {
            continue;
        };
        if !inventory_names.contains(name) {
            continue;
        }
        let Some(seconds) = event.get("exec_time").and_then(serde_json::Value::as_f64) else {
            continue;
        };
        let elapsed_ms = seconds_to_rounded_millis(seconds);
        let Some(elapsed_ms) = elapsed_ms else {
            continue;
        };
        timings.insert(name.to_string(), elapsed_ms);
    }
    let mut timings = timings
        .into_iter()
        .map(|(name, elapsed_ms)| OvenNativeTestCaseTiming { name, elapsed_ms })
        .collect::<Vec<_>>();
    timings.sort_by(|left, right| {
        right
            .elapsed_ms
            .cmp(&left.elapsed_ms)
            .then_with(|| left.name.cmp(&right.name))
    });
    timings
}

/// Convert libtest's JSON floating-point seconds to a rounded millisecond report value.
fn seconds_to_rounded_millis(seconds: f64) -> Option<u64> {
    let milliseconds = seconds * 1_000.0;
    (seconds.is_finite() && seconds >= 0.0 && milliseconds <= u64::MAX as f64).then(|| milliseconds.round() as u64)
}

/// Return no timings unless the caller explicitly asked the existing libtest invocation to report them.
fn parse_libtest_case_timings_if_requested(
    output: &str,
    inventory: &OvenNativeTestInventory,
) -> Vec<OvenNativeTestCaseTiming> {
    if std::env::var_os(OVEN_NATIVE_TEST_CASE_TIMINGS_ENV).is_some() {
        parse_libtest_case_timings(output, inventory)
    } else {
        Vec::new()
    }
}

/// Parse explicit integration-test command timing records from an already-captured libtest transcript.
///
/// Records are JSON so test names and temporary paths cannot make the diagnostic format ambiguous. Malformed or
/// unrelated diagnostic output is ignored; timing is observational and must never make a passing test fail.
fn parse_native_test_command_timings(output: &str) -> Vec<OvenNativeTestCommandTiming> {
    const COMMAND_PREFIX: &str = "incan-test-command-timing ";
    const BUILD_PHASE_PREFIX: &str = "incan-test-build-phase-timing ";
    let mut timings = Vec::new();
    for line in output.lines() {
        if let Some(payload) = line.strip_prefix(COMMAND_PREFIX) {
            let Ok(record) = serde_json::from_str::<serde_json::Value>(payload) else {
                continue;
            };
            let Some(test_name) = record.get("test_name").and_then(serde_json::Value::as_str) else {
                continue;
            };
            let Some(command) = record.get("command").and_then(serde_json::Value::as_str) else {
                continue;
            };
            let Some(elapsed_ms) = record.get("elapsed_ms").and_then(serde_json::Value::as_u64) else {
                continue;
            };
            if test_name.is_empty() || command.is_empty() {
                continue;
            }
            timings.push(OvenNativeTestCommandTiming {
                test_name: test_name.to_string(),
                command: command.to_string(),
                elapsed_ms,
                phase_timings_ms: BTreeMap::new(),
            });
            continue;
        }
        let Some(payload) = line.strip_prefix(BUILD_PHASE_PREFIX) else {
            continue;
        };
        let Ok(record) = serde_json::from_str::<serde_json::Value>(payload) else {
            continue;
        };
        let Some(test_name) = record.get("test_name").and_then(serde_json::Value::as_str) else {
            continue;
        };
        let Some(command) = record.get("command").and_then(serde_json::Value::as_str) else {
            continue;
        };
        let Some(phase_timings_ms) = record.get("phase_timings_ms").and_then(serde_json::Value::as_object) else {
            continue;
        };
        let phase_timings_ms = phase_timings_ms
            .iter()
            .filter_map(|(phase, elapsed_ms)| elapsed_ms.as_u64().map(|elapsed_ms| (phase.clone(), elapsed_ms)))
            .collect::<BTreeMap<_, _>>();
        if phase_timings_ms.is_empty() {
            continue;
        }
        if let Some(timing) = timings
            .iter_mut()
            .rev()
            .find(|timing| timing.test_name == test_name && timing.command == command)
        {
            timing.phase_timings_ms = phase_timings_ms;
        }
    }
    timings
}

/// Render libtest's structured events as human progress while a root is still running.
///
/// Oven asks libtest for JSON events so it can recover per-case durations, which means the child's stdout is no
/// longer readable as a transcript. This turns that stream back into something a person can watch: one line per case
/// as it finishes, carrying the time it took, and non-event lines passed through unchanged because `--nocapture`
/// interleaves the tests' own output with the event stream.
///
/// Reporting is observational. A malformed or unrecognized line is passed through rather than diagnosed, because
/// progress output must never be able to fail a run that would otherwise pass.
struct NativeTestProgressReporter {
    /// Cases libtest has started and not yet reported a terminal event for.
    outstanding: BTreeSet<String>,
    /// Terminal events seen so far, used only to render a running count against the suite total.
    completed: usize,
    /// Total the suite announced up front, absent until its `started` event arrives.
    total: Option<usize>,
}

impl NativeTestProgressReporter {
    /// Start a reporter with nothing outstanding and no announced total.
    fn new() -> Self {
        Self {
            outstanding: BTreeSet::new(),
            completed: 0,
            total: None,
        }
    }

    /// Consume one transcript line, rendering it as progress when it is a libtest event.
    fn observe(&mut self, line: &str) {
        let Some(event) = serde_json::from_str::<serde_json::Value>(line)
            .ok()
            .filter(serde_json::Value::is_object)
        else {
            // Test output under `--nocapture`, not an event. Pass it through so a `println!` in a failing test still
            // reaches the person watching.
            println!("{line}");
            return;
        };
        let kind = event
            .get("type")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        let outcome = event
            .get("event")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        match (kind, outcome) {
            ("suite", "started") => {
                self.total = event
                    .get("test_count")
                    .and_then(serde_json::Value::as_u64)
                    .and_then(|count| usize::try_from(count).ok());
            }
            ("test", "started") => {
                if let Some(name) = event.get("name").and_then(serde_json::Value::as_str) {
                    self.outstanding.insert(name.to_string());
                }
            }
            ("test", _) if !outcome.is_empty() => self.report_terminal_event(&event, outcome),
            _ => {}
        }
    }

    /// Render one case's terminal event, and drop it from the outstanding set.
    fn report_terminal_event(&mut self, event: &serde_json::Value, outcome: &str) {
        let Some(name) = event.get("name").and_then(serde_json::Value::as_str) else {
            return;
        };
        self.outstanding.remove(name);
        self.completed = self.completed.saturating_add(1);
        let position = match self.total {
            Some(total) => format!("{:>5}/{total}", self.completed),
            None => format!("{:>5}", self.completed),
        };
        let elapsed = event
            .get("exec_time")
            .and_then(serde_json::Value::as_f64)
            .map(|seconds| format!(" {:>8.3}s", seconds))
            .unwrap_or_default();
        println!("{position} {outcome:<8}{elapsed}  {name}");
    }

    /// Name anything libtest started and never finished, which is what a killed or hung root leaves behind.
    fn finish(&mut self) {
        if self.outstanding.is_empty() {
            return;
        }
        println!(
            "{} case(s) started and never reported a result:",
            self.outstanding.len()
        );
        for name in &self.outstanding {
            println!("  {name}");
        }
    }
}

/// Spawn one captured native libtest child and enforce an optional execution-group deadline.
///
/// `Command::output` cannot supervise a running child. Keeping this small polling loop here ensures the same
/// Cargo-free environment and output capture apply to a terminated child as to a normally completed libtest process.
fn run_native_batch_child(
    mut command: Command,
    executable: &Path,
    timeout: Option<Duration>,
    reporter: Option<NativeTestProgressReporter>,
) -> Result<(std::process::Output, bool), OvenNativeTestError> {
    // Both the deadline and the deadline-free case now take the piped path. `Command::output` cannot supervise a
    // running child, and it cannot stream one either, so keeping it for the deadline-free case would have meant a
    // root reports progress only when someone gave it a budget.
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    // Nested Incan commands and fixture children inherit this group, allowing a timeout to close every inherited
    // stdout/stderr writer before reader threads are joined.
    isolate_process_group(&mut command);
    let mut child = command.spawn().map_err(|source| OvenNativeTestError::Io {
        path: executable.to_path_buf(),
        source,
    })?;
    let mut stdout = child.stdout.take().ok_or_else(|| OvenNativeTestError::Io {
        path: executable.to_path_buf(),
        source: io::Error::other("native test child stdout was not piped"),
    })?;
    let mut stderr = child.stderr.take().ok_or_else(|| OvenNativeTestError::Io {
        path: executable.to_path_buf(),
        source: io::Error::other("native test child stderr was not piped"),
    })?;
    let mut progress = reporter;
    let stdout_reader = thread::spawn(move || {
        // Read by line rather than to end. `read_to_end` is why a long root is silent: it yields nothing until the
        // child exits, so a slow suite and a hung one look identical for as long as they run. The accumulated bytes
        // stay byte-for-byte what they were, because every downstream consumer -- the retained transcript, the
        // libtest timing parse, and the caller's per-test result mapping -- reads that buffer rather than this loop.
        let mut reader = io::BufReader::new(stdout);
        let mut bytes = Vec::new();
        let mut line = Vec::new();
        loop {
            line.clear();
            let read = reader.read_until(b'\n', &mut line)?;
            if read == 0 {
                break;
            }
            bytes.extend_from_slice(&line);
            if let Some(progress) = progress.as_mut() {
                progress.observe(String::from_utf8_lossy(&line).trim_end_matches(['\r', '\n']));
            }
        }
        if let Some(progress) = progress.as_mut() {
            progress.finish();
        }
        Ok::<_, io::Error>(bytes)
    });
    let stderr_reader = thread::spawn(move || {
        let mut bytes = Vec::new();
        stderr.read_to_end(&mut bytes)?;
        Ok::<_, io::Error>(bytes)
    });
    let deadline = timeout.map(|timeout| Instant::now() + timeout);
    let mut timed_out = false;
    let status = loop {
        match child.try_wait().map_err(|source| OvenNativeTestError::Io {
            path: executable.to_path_buf(),
            source,
        })? {
            Some(status) => break status,
            None if deadline.is_some_and(|deadline| Instant::now() >= deadline) => {
                timed_out = true;
                break terminate_native_batch_child(&mut child, executable)?;
            }
            None => thread::sleep(Duration::from_millis(1)),
        }
    };
    let stdout = join_output_reader(stdout_reader, executable, "stdout")?;
    let stderr = join_output_reader(stderr_reader, executable, "stderr")?;
    let output = std::process::Output { status, stdout, stderr };
    Ok((output, timed_out))
}

/// Terminate and reap one timed-out root together with descendants that inherited its process group.
fn terminate_native_batch_child(
    child: &mut std::process::Child,
    executable: &Path,
) -> Result<std::process::ExitStatus, OvenNativeTestError> {
    terminate_process_group(child).map_err(|source| OvenNativeTestError::Io {
        path: executable.to_path_buf(),
        source,
    })
}

/// Join one concurrent pipe reader and preserve its failure as an ordinary native-runner error.
fn join_output_reader(
    reader: thread::JoinHandle<Result<Vec<u8>, io::Error>>,
    executable: &Path,
    stream: &str,
) -> Result<Vec<u8>, OvenNativeTestError> {
    reader
        .join()
        .map_err(|_| OvenNativeTestError::Io {
            path: executable.to_path_buf(),
            source: io::Error::other(format!("native test {stream} reader panicked")),
        })?
        .map_err(|source| OvenNativeTestError::Io {
            path: executable.to_path_buf(),
            source,
        })
}

/// Use a compact, stable diagnostic spelling while preserving sub-millisecond values when supplied by the API.
fn format_timeout(timeout: Duration) -> String {
    let nanos = timeout.as_nanos();
    if timeout.as_secs() > 0 && nanos.is_multiple_of(1_000_000_000) {
        format!("{}s", timeout.as_secs())
    } else if timeout.as_millis() > 0 && nanos.is_multiple_of(1_000_000) {
        format!("{}ms", timeout.as_millis())
    } else if timeout.as_micros() > 0 && nanos.is_multiple_of(1_000) {
        format!("{}us", timeout.as_micros())
    } else {
        format!("{nanos}ns")
    }
}

/// Reject symlink and non-file execution paths before creating a child process.
fn verified_executable(executable: &Path) -> Result<PathBuf, OvenNativeTestError> {
    if executable.as_os_str().is_empty() {
        return Err(OvenNativeTestError::InvalidInput {
            field: "executable",
            message: "must not be empty".to_string(),
        });
    }
    let metadata = fs::symlink_metadata(executable).map_err(|source| OvenNativeTestError::Io {
        path: executable.to_path_buf(),
        source,
    })?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(OvenNativeTestError::InvalidInput {
            field: "executable",
            message: "must be a non-symlink regular file".to_string(),
        });
    }
    Ok(executable.to_path_buf())
}

/// Normalize the exact test selection and make duplicate execution an input error.
fn normalized_exact_names(names: &[String]) -> Result<Vec<String>, OvenNativeTestError> {
    if names.is_empty() {
        return Err(OvenNativeTestError::InvalidInput {
            field: "exact test selection",
            message: "must name at least one collected test".to_string(),
        });
    }
    let mut unique = BTreeSet::new();
    for name in names {
        let normalized = name.trim();
        if normalized.is_empty() {
            return Err(OvenNativeTestError::InvalidInput {
                field: "exact test selection",
                message: "must not contain an empty name".to_string(),
            });
        }
        if !unique.insert(normalized.to_string()) {
            return Err(OvenNativeTestError::InvalidInput {
                field: "exact test selection",
                message: format!("contains duplicate `{normalized}`"),
            });
        }
    }
    Ok(unique.into_iter().collect())
}

/// Parse the stable `<name>: test` libtest terse inventory lines and reject unexplained non-empty output.
fn parse_inventory(stdout: &[u8], allow_empty: bool) -> Result<Vec<String>, OvenNativeTestError> {
    let text = String::from_utf8_lossy(stdout);
    let mut names = BTreeSet::new();
    for line in text.lines().map(str::trim).filter(|line| !line.is_empty()) {
        let Some(name) = line.strip_suffix(": test") else {
            return Err(OvenNativeTestError::InventoryFailed {
                output: format!("unexpected libtest inventory line `{line}`"),
            });
        };
        if name.is_empty() || !names.insert(name.to_string()) {
            return Err(OvenNativeTestError::InventoryFailed {
                output: format!("invalid or duplicate libtest test name `{name}`"),
            });
        }
    }
    if names.is_empty() && !allow_empty {
        return Err(OvenNativeTestError::InventoryFailed {
            output: "libtest inventory contained no test cases".to_string(),
        });
    }
    Ok(names.into_iter().collect())
}

/// Preserve both child streams in deterministic diagnostic order.
fn combined_output(stdout: &[u8], stderr: &[u8]) -> String {
    format!("{}{}", String::from_utf8_lossy(stdout), String::from_utf8_lossy(stderr))
}

/// Parse libtest's final `test result` line without treating diagnostic text as a result.
///
/// Test bodies can run nested programs that also print libtest summaries. The outer native batch always emits its
/// own summary last, so scan backwards and preserve `None` when a process dies before doing so.
fn parse_libtest_case_counts(output: &str) -> Option<OvenNativeTestCaseCounts> {
    parse_libtest_json_case_counts(output).or_else(|| parse_libtest_text_case_counts(output))
}

/// Parse libtest's terminal JSON suite event emitted by the opt-in case-timing diagnostic mode.
fn parse_libtest_json_case_counts(output: &str) -> Option<OvenNativeTestCaseCounts> {
    let event = output.lines().rev().find_map(|line| {
        let value = serde_json::from_str::<serde_json::Value>(line).ok()?;
        (value.get("type").and_then(serde_json::Value::as_str) == Some("suite")).then_some(value)
    })?;
    let passed = usize::try_from(event.get("passed")?.as_u64()?).ok()?;
    let failed = usize::try_from(event.get("failed")?.as_u64()?).ok()?;
    let ignored = usize::try_from(event.get("ignored")?.as_u64()?).ok()?;
    Some(OvenNativeTestCaseCounts {
        passed,
        failed,
        ignored,
    })
}

/// Parse libtest's final text `test result` line without treating diagnostic text as a result.
fn parse_libtest_text_case_counts(output: &str) -> Option<OvenNativeTestCaseCounts> {
    let summary = output
        .lines()
        .rev()
        .map(str::trim)
        .find_map(|line| line.strip_prefix("test result: "))?;
    let mut counts = OvenNativeTestCaseCounts::default();
    let mut saw_passed = false;
    let mut saw_failed = false;
    let mut saw_ignored = false;
    for segment in summary.split(';').map(str::trim) {
        if let Some(value) = segment.strip_suffix(" passed") {
            counts.passed = value.split_whitespace().last()?.parse().ok()?;
            saw_passed = true;
        } else if let Some(value) = segment.strip_suffix(" failed") {
            counts.failed = value.split_whitespace().last()?.parse().ok()?;
            saw_failed = true;
        } else if let Some(value) = segment.strip_suffix(" ignored") {
            counts.ignored = value.split_whitespace().last()?.parse().ok()?;
            saw_ignored = true;
        }
    }
    (saw_passed && saw_failed && saw_ignored).then_some(counts)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;
    use std::path::PathBuf;
    use std::process::Command;

    use super::{
        OvenNativeTestCaseCounts, OvenNativeTestCommandTiming, OvenNativeTestError, OvenNativeTestInventory,
        OvenNativeTestRequest, parse_libtest_case_counts, parse_libtest_case_timings,
        parse_native_test_command_timings, run_native_test_batch, run_native_test_batch_all,
        run_native_test_batch_all_in_directory_with_timeout,
        run_native_test_batch_all_in_directory_with_timeout_and_threads,
        run_native_test_exact_in_directory_with_timeout, run_native_tests,
        run_native_tests_exact_in_directory_with_timeout,
    };

    #[test]
    fn terminal_case_counts_use_the_outermost_libtest_summary() {
        let counts = parse_libtest_case_counts(
            "nested test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s\n\
             test result: FAILED. 7 passed; 2 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.02s\n",
        );
        assert_eq!(
            counts,
            Some(OvenNativeTestCaseCounts {
                passed: 7,
                failed: 2,
                ignored: 1,
            })
        );
        assert_eq!(parse_libtest_case_counts("process aborted\n"), None);
        assert_eq!(
            parse_libtest_case_counts(
                "{ \"type\": \"test\", \"event\": \"ok\", \"name\": \"selected\", \"exec_time\": 0.01 }\n\
                 { \"type\": \"suite\", \"event\": \"ok\", \"passed\": 3, \"failed\": 0, \"ignored\": 2 }\n",
            ),
            Some(OvenNativeTestCaseCounts {
                passed: 3,
                failed: 0,
                ignored: 2,
            })
        );
    }

    #[test]
    fn case_timings_keep_only_inventory_cases_and_last_terminal_result() {
        let inventory = OvenNativeTestInventory {
            names: vec!["outer::fast".to_string(), "outer::slow".to_string()],
        };
        let timings = parse_libtest_case_timings(
            "nested test output that is not JSON\n\
             { \"type\": \"test\", \"name\": \"nested::slow\", \"event\": \"ok\", \"exec_time\": 98.5 }\n\
             { \"type\": \"test\", \"name\": \"outer::slow\", \"event\": \"ok\", \"exec_time\": 0.1234 }\n\
             { \"type\": \"test\", \"name\": \"outer::fast\", \"event\": \"ok\", \"exec_time\": 0.0005 }\n\
             { \"type\": \"test\", \"name\": \"outer::slow\", \"event\": \"ok\", \"exec_time\": 1.234 }\n",
            &inventory,
        );

        assert_eq!(
            timings
                .iter()
                .map(|timing| (timing.name.as_str(), timing.elapsed_ms))
                .collect::<Vec<_>>(),
            vec![("outer::slow", 1_234), ("outer::fast", 1)]
        );
    }

    #[test]
    fn command_timings_preserve_valid_transcript_order_and_ignore_unrelated_output() {
        let timings = parse_native_test_command_timings(
            "normal output\n\
             incan-test-command-timing {\"test_name\":\"outer::first\",\"command\":\"incan lock\",\"elapsed_ms\":12}\n\
             incan-test-command-timing not-json\n\
             incan-test-command-timing {\"test_name\":\"outer::second\",\"command\":\"incan build --lib\",\"elapsed_ms\":345}\n\
             incan-test-build-phase-timing {\"test_name\":\"outer::second\",\"command\":\"incan build --lib\",\"phase_timings_ms\":{\"prepare\":100,\"oven_build\":200,\"total\":300}}\n\
             incan-test-build-phase-timing {\"test_name\":\"unknown\",\"command\":\"incan build\",\"phase_timings_ms\":{\"total\":1}}\n",
        );
        assert_eq!(
            timings,
            vec![
                OvenNativeTestCommandTiming {
                    test_name: "outer::first".to_string(),
                    command: "incan lock".to_string(),
                    elapsed_ms: 12,
                    phase_timings_ms: BTreeMap::new(),
                },
                OvenNativeTestCommandTiming {
                    test_name: "outer::second".to_string(),
                    command: "incan build --lib".to_string(),
                    elapsed_ms: 345,
                    phase_timings_ms: BTreeMap::from([
                        ("oven_build".to_string(), 200),
                        ("prepare".to_string(), 100),
                        ("total".to_string(), 300),
                    ]),
                },
            ]
        );
    }
    use crate::oven::rustc::{
        OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION, OvenRustcArtifactManifest, OvenStoredDirectRustcTestRequest,
        bake_stored_direct_rustc_test, rustc_host_target,
    };
    use crate::oven::store::{OvenArtifactKind, OvenArtifactPublishRequest, OvenStore, OvenStoreLimits};
    use crate::oven::{OvenImportRequest, digest_bytes, import_frozen_project};

    #[test]
    fn native_runner_rejects_missing_exact_test_and_runs_verified_test_without_cargo()
    -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let output = tempfile::tempdir()?;
        let store_root = tempfile::tempdir()?;
        write_project(project.path())?;
        let source = output.path().join("native-tests.rs");
        fs::write(
            &source,
            "#[test]\nfn selected() { assert!(std::env::var_os(\"CARGO\").is_none()); assert!(std::env::var_os(\"CARGO_PKG_NAME\").is_none()); }\n#[test]\nfn other() {}\n",
        )?;
        let rustc = rustc_path()?;
        let receipt = import_frozen_project(
            &OvenImportRequest::new(
                project.path(),
                rustc_host_target(&rustc)?,
                rustc_identity(&rustc)?,
                "release",
                Vec::new(),
            )
            .with_supplemental_source_digest("direct-rustc-source", digest_bytes(&fs::read(&source)?)),
        )?;
        let plan = OvenRustcArtifactManifest {
            schema_version: OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION,
            intent: receipt.intent.clone(),
            dependency_search_paths: Vec::new(),
            native_search_paths: Vec::new(),
            externs: Vec::new(),
            entrypoint_externs: BTreeMap::new(),
            registry_leaves: Vec::new(),
            registry_sources: Vec::new(),
            compile_environment: BTreeMap::new(),
            vocab_auxiliary_targets: Vec::new(),
            supporting_artifacts: Vec::new(),
        };
        let store = OvenStore::new(
            store_root.path(),
            OvenStoreLimits::new(128 * 1024, 128 * 1024, 64 * 1024),
        );
        let stored = store.publish(&OvenArtifactPublishRequest {
            receipt: receipt.clone(),
            domain: "native-tests".to_string(),
            kind: OvenArtifactKind::DirectRustcPlan,
            payload: serde_json::to_vec(&plan)?,
            materialized_files: Vec::new(),
        })?;
        let bake = bake_stored_direct_rustc_test(&OvenStoredDirectRustcTestRequest {
            store: &store,
            plan_identity: stored.identity,
            receipt,
            rustc,
            source,
            output: output.path().join("native-tests"),
            crate_name: "oven_native_tests".to_string(),
            edition: "2024".to_string(),
            source_evidence_key: "direct-rustc-source".to_string(),
        })?;

        let missing = run_native_tests(&OvenNativeTestRequest {
            executable: bake.output.clone(),
            exact_names: vec!["absent".to_string()],
            environment: BTreeMap::new(),
            timeout: None,
        });
        assert!(matches!(missing, Err(OvenNativeTestError::MissingExactTest { .. })));
        let report = run_native_tests(&OvenNativeTestRequest {
            executable: bake.output,
            exact_names: vec!["selected".to_string()],
            environment: BTreeMap::new(),
            timeout: None,
        })?;
        assert_eq!(report.inventory.names, ["other", "selected"]);
        assert_eq!(report.passed, ["selected"]);
        Ok(())
    }

    #[test]
    fn all_batch_executes_a_valid_zero_test_target() -> Result<(), Box<dyn std::error::Error>> {
        let output = tempfile::tempdir()?;
        let source = output.path().join("zero-tests.rs");
        let executable = output.path().join("zero-tests");
        fs::write(&source, "fn helper() {}\n")?;
        let rustc = rustc_path()?;
        let status = Command::new(rustc)
            .arg("--test")
            .arg(&source)
            .arg("-o")
            .arg(&executable)
            .status()?;
        assert!(status.success());

        let report = run_native_test_batch_all(&executable, &BTreeMap::new())?;
        assert!(report.success);
        assert!(report.inventory.names.is_empty());
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn generated_batch_forces_single_inner_libtest_thread() -> Result<(), Box<dyn std::error::Error>> {
        use std::os::unix::fs::PermissionsExt;

        let output = tempfile::tempdir()?;
        let executable = output.path().join("native-test-argument-check");
        fs::write(
            &executable,
            "#!/bin/sh\n\
             if [ \"$1\" = \"--list\" ] && [ \"$2\" = \"--format\" ] && [ \"$3\" = \"terse\" ]; then\n\
               printf '%s\\n' 'generated::case: test'\n\
               exit 0\n\
             fi\n\
             if [ \"$1\" = \"--test-threads=1\" ] && [ \"$2\" = \"--nocapture\" ] && [ \"$#\" -eq 2 ]; then\n\
               exit 0\n\
             fi\n\
             printf 'unexpected native test arguments: %s\\n' \"$*\" >&2\n\
             exit 62\n",
        )?;
        let mut permissions = fs::metadata(&executable)?.permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&executable, permissions)?;

        let report = run_native_test_batch(&OvenNativeTestRequest {
            executable,
            exact_names: vec!["generated::case".to_string()],
            environment: BTreeMap::new(),
            timeout: None,
        })?;
        assert!(report.success, "{report:#?}");
        assert_eq!(report.inventory.names, ["generated::case"]);
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn exact_batch_runs_one_verified_case_from_the_requested_directory() -> Result<(), Box<dyn std::error::Error>> {
        use std::os::unix::fs::PermissionsExt;
        use std::time::Duration;

        let output = tempfile::tempdir()?;
        let executable = output.path().join("exact-native-test-argument-check");
        let working_directory = output.path().join("working-directory");
        fs::create_dir_all(&working_directory)?;
        let working_directory_marker = output.path().join("working-directory.txt");
        fs::write(
            &executable,
            "#!/bin/sh\n\
             if [ \"$1\" = \"--list\" ] && [ \"$2\" = \"--format\" ] && [ \"$3\" = \"terse\" ]; then\n\
               printf '%s\\n' 'exact::selected: test' 'exact::other: test'\n\
               exit 0\n\
             fi\n\
             if [ \"$1\" = \"--exact\" ] && [ \"$2\" = \"exact::selected\" ] && [ \"$3\" = \"--nocapture\" ] && [ \"$#\" -eq 3 ]; then\n\
               pwd > \"$INCAN_TEST_EXACT_WORKING_DIRECTORY_MARKER\"\n\
               printf '%s\\n' 'test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 1 filtered out; finished in 0.00s'\n\
               exit 0\n\
             fi\n\
             printf 'unexpected native test arguments: %s\\n' \"$*\" >&2\n\
             exit 62\n",
        )?;
        let mut permissions = fs::metadata(&executable)?.permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&executable, permissions)?;
        let environment = BTreeMap::from([(
            "INCAN_TEST_EXACT_WORKING_DIRECTORY_MARKER".to_string(),
            working_directory_marker.display().to_string(),
        )]);

        let report = run_native_test_exact_in_directory_with_timeout(
            &executable,
            "exact::selected",
            &environment,
            Some(&working_directory),
            Some(Duration::from_secs(5)),
        )?;

        assert!(report.success, "{report:#?}");
        assert_eq!(report.inventory.names, ["exact::other", "exact::selected"]);
        assert_eq!(
            report.case_counts,
            Some(OvenNativeTestCaseCounts {
                passed: 1,
                failed: 0,
                ignored: 0,
            })
        );
        assert_eq!(
            fs::read_to_string(working_directory_marker)?.trim(),
            fs::canonicalize(&working_directory)?.display().to_string(),
            "the exact diagnostic must retain the package working directory"
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn multi_exact_batch_inventories_once_validates_first_and_aggregates_terminal_outcomes()
    -> Result<(), Box<dyn std::error::Error>> {
        use std::os::unix::fs::PermissionsExt;
        use std::time::Duration;

        let output = tempfile::tempdir()?;
        let executable = output.path().join("multi-exact-native-test");
        let inventory_marker = output.path().join("inventory.txt");
        let execution_log = output.path().join("executions.txt");
        let cargo_marker = output.path().join("cargo-environment.txt");
        fs::write(
            &executable,
            "#!/bin/sh\n\
             if [ \"$1\" = \"--list\" ] && [ \"$2\" = \"--format\" ] && [ \"$3\" = \"terse\" ]; then\n\
               printf x >> \"$INCAN_TEST_INVENTORY_MARKER\"\n\
               printf '%s\\n' 'exact::failed: test' 'exact::green: test' 'exact::ignored: test'\n\
               exit 0\n\
             fi\n\
             if [ -n \"${CARGO:-}\" ] || [ -n \"${CARGO_PKG_NAME:-}\" ]; then\n\
               printf cargo > \"$INCAN_TEST_CARGO_MARKER\"\n\
               exit 97\n\
             fi\n\
             if [ \"$1\" != \"--exact\" ] || [ \"$3\" != \"--nocapture\" ] || [ \"$#\" -ne 3 ]; then\n\
               printf 'unexpected native test arguments: %s\\n' \"$*\" >&2\n\
               exit 62\n\
             fi\n\
             printf '%s\\n' \"$2\" >> \"$INCAN_TEST_EXECUTION_LOG\"\n\
             case \"$2\" in\n\
               exact::green)\n\
                 printf '%s\\n' 'incan-test-command-timing {\"test_name\":\"exact::green\",\"command\":\"incan build\",\"elapsed_ms\":9}'\n\
                 printf '%s\\n' 'test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 2 filtered out; finished in 0.00s'\n\
                 exit 0;;\n\
               exact::failed)\n\
                 printf '%s\\n' 'test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 2 filtered out; finished in 0.00s'\n\
                 exit 1;;\n\
               exact::ignored)\n\
                 printf '%s\\n' 'test result: ok. 0 passed; 0 failed; 1 ignored; 0 measured; 2 filtered out; finished in 0.00s'\n\
                 exit 0;;\n\
             esac\n\
             exit 63\n",
        )?;
        let mut permissions = fs::metadata(&executable)?.permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&executable, permissions)?;
        let environment = BTreeMap::from([
            (
                "INCAN_TEST_INVENTORY_MARKER".to_string(),
                inventory_marker.display().to_string(),
            ),
            (
                "INCAN_TEST_EXECUTION_LOG".to_string(),
                execution_log.display().to_string(),
            ),
            (
                "INCAN_TEST_CARGO_MARKER".to_string(),
                cargo_marker.display().to_string(),
            ),
        ]);

        let missing = run_native_tests_exact_in_directory_with_timeout(
            &executable,
            &["exact::green".to_string(), "exact::missing".to_string()],
            &environment,
            Some(output.path()),
            Some(Duration::from_secs(5)),
        );
        assert!(matches!(missing, Err(OvenNativeTestError::MissingExactTest { .. })));
        assert!(
            !execution_log.exists(),
            "no selected case may start before the complete exact selection is valid"
        );
        assert_eq!(fs::read_to_string(&inventory_marker)?, "x");
        fs::write(&inventory_marker, "")?;

        let report = run_native_tests_exact_in_directory_with_timeout(
            &executable,
            &[
                "exact::ignored".to_string(),
                "exact::green".to_string(),
                "exact::failed".to_string(),
            ],
            &environment,
            Some(output.path()),
            Some(Duration::from_secs(5)),
        )?;

        assert!(!report.success, "{report:#?}");
        assert!(!report.timed_out, "{report:#?}");
        assert_eq!(
            fs::read_to_string(inventory_marker)?,
            "x",
            "one multi-exact run must inventory exactly once"
        );
        assert_eq!(
            fs::read_to_string(execution_log)?,
            "exact::failed\nexact::green\nexact::ignored\n"
        );
        assert!(
            !cargo_marker.exists(),
            "multi-exact children inherited Cargo process state"
        );
        assert_eq!(
            report.case_counts,
            Some(OvenNativeTestCaseCounts {
                passed: 1,
                failed: 1,
                ignored: 1
            })
        );
        assert_eq!(report.command_timings.len(), 1);
        assert_eq!(report.command_timings[0].test_name, "exact::green");
        assert_eq!(report.command_timings[0].elapsed_ms, 9);
        assert!(report.output.contains("--- Oven exact test `exact::failed` ---"));
        assert!(report.output.contains("--- Oven exact test `exact::green` ---"));
        assert!(report.output.contains("--- Oven exact test `exact::ignored` ---"));
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn multi_exact_batch_contains_a_timed_out_process_tree_and_stops_launching_later_cases()
    -> Result<(), Box<dyn std::error::Error>> {
        use std::os::unix::fs::PermissionsExt;
        use std::time::{Duration, Instant};

        let output = tempfile::tempdir()?;
        let executable = output.path().join("multi-exact-timeout");
        let execution_log = output.path().join("executions.txt");
        fs::write(
            &executable,
            "#!/bin/sh\n\
             if [ \"$1\" = \"--list\" ] && [ \"$2\" = \"--format\" ] && [ \"$3\" = \"terse\" ]; then\n\
               printf '%s\\n' 'exact::a_slow: test' 'exact::z_after: test'\n\
               exit 0\n\
             fi\n\
             printf '%s\\n' \"$2\" >> \"$INCAN_TEST_EXECUTION_LOG\"\n\
             if [ \"$2\" = \"exact::a_slow\" ]; then sleep 30; fi\n\
             printf '%s\\n' 'test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 1 filtered out; finished in 0.00s'\n",
        )?;
        let mut permissions = fs::metadata(&executable)?.permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&executable, permissions)?;
        let environment = BTreeMap::from([(
            "INCAN_TEST_EXECUTION_LOG".to_string(),
            execution_log.display().to_string(),
        )]);

        let started = Instant::now();
        let report = run_native_tests_exact_in_directory_with_timeout(
            &executable,
            &["exact::a_slow".to_string(), "exact::z_after".to_string()],
            &environment,
            Some(output.path()),
            Some(Duration::from_secs(5)),
        )?;

        assert!(!report.success, "{report:#?}");
        assert!(report.timed_out, "{report:#?}");
        assert_eq!(report.case_counts, None);
        assert!(report.output.contains("aggregate 5s deadline"), "{report:#?}");
        assert_eq!(fs::read_to_string(execution_log)?, "exact::a_slow\n");
        assert!(
            started.elapsed() < Duration::from_secs(15),
            "the timed-out exact process tree retained its pipes"
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn multi_exact_batch_contains_inventory_in_the_aggregate_deadline_before_any_case_starts()
    -> Result<(), Box<dyn std::error::Error>> {
        use std::os::unix::fs::PermissionsExt;
        use std::time::{Duration, Instant};

        let output = tempfile::tempdir()?;
        let executable = output.path().join("multi-exact-inventory-timeout");
        let execution_marker = output.path().join("execution.txt");
        fs::write(
            &executable,
            "#!/bin/sh\n\
             if [ \"$1\" = \"--list\" ]; then sleep 30; fi\n\
             printf executed > \"$INCAN_TEST_EXECUTION_MARKER\"\n",
        )?;
        let mut permissions = fs::metadata(&executable)?.permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&executable, permissions)?;
        let environment = BTreeMap::from([(
            "INCAN_TEST_EXECUTION_MARKER".to_string(),
            execution_marker.display().to_string(),
        )]);

        let started = Instant::now();
        let result = run_native_tests_exact_in_directory_with_timeout(
            &executable,
            &["exact::never_started".to_string()],
            &environment,
            Some(output.path()),
            Some(Duration::from_millis(10)),
        );

        assert!(
            matches!(result, Err(OvenNativeTestError::InventoryFailed { output }) if output.contains("inventory timed out"))
        );
        assert!(!execution_marker.exists());
        assert!(
            started.elapsed() < Duration::from_secs(15),
            "inventory descendants retained pipes beyond the aggregate timeout"
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn scheduled_all_batch_honors_its_explicit_libtest_thread_budget() -> Result<(), Box<dyn std::error::Error>> {
        use std::os::unix::fs::PermissionsExt;
        use std::time::Duration;

        let output = tempfile::tempdir()?;
        let executable = output.path().join("scheduled-native-test-argument-check");
        fs::write(
            &executable,
            "#!/bin/sh\n\
             if [ \"$1\" = \"--list\" ] && [ \"$2\" = \"--format\" ] && [ \"$3\" = \"terse\" ]; then\n\
               printf '%s\\n' 'scheduled::case: test'\n\
               exit 0\n\
             fi\n\
             if [ \"$1\" = \"--test-threads=1\" ] && [ \"$2\" = \"--nocapture\" ] && [ \"$#\" -eq 2 ]; then\n\
               exit 0\n\
             fi\n\
             printf 'unexpected native test arguments: %s\\n' \"$*\" >&2\n\
             exit 62\n",
        )?;
        let mut permissions = fs::metadata(&executable)?.permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&executable, permissions)?;

        let report = run_native_test_batch_all_in_directory_with_timeout_and_threads(
            &executable,
            &BTreeMap::new(),
            Some(output.path()),
            Some(Duration::from_secs(5)),
            1,
        )?;
        assert!(report.success, "{report:#?}");
        assert_eq!(report.inventory.names, ["scheduled::case"]);
        Ok(())
    }

    #[test]
    fn generated_batch_timeout_terminates_a_native_test_child() -> Result<(), Box<dyn std::error::Error>> {
        use std::time::Duration;

        let output = tempfile::tempdir()?;
        let source = output.path().join("slow-native-test.rs");
        let executable = output.path().join("slow-native-test");
        fs::write(
            &source,
            "#[test]\nfn generated_case() { std::thread::sleep(std::time::Duration::from_secs(1)); }\n",
        )?;
        let status = Command::new(rustc_path()?)
            .arg("--test")
            .arg(&source)
            .arg("-o")
            .arg(&executable)
            .status()?;
        assert!(status.success());

        let report = run_native_test_batch(&OvenNativeTestRequest {
            executable,
            exact_names: vec!["generated_case".to_string()],
            environment: BTreeMap::new(),
            timeout: Some(Duration::from_millis(10)),
        })?;
        assert!(!report.success, "{report:#?}");
        assert!(report.timed_out, "{report:#?}");
        assert!(report.output.contains("timed out after 10ms"), "{report:#?}");
        assert_eq!(report.case_counts, None);
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn all_batch_timeout_terminates_a_stalled_native_test_child() -> Result<(), Box<dyn std::error::Error>> {
        use std::os::unix::fs::PermissionsExt;
        use std::time::{Duration, Instant};

        let output = tempfile::tempdir()?;
        let executable = output.path().join("stalled-native-test");
        fs::write(
            &executable,
            "#!/bin/sh\n\
             if [ \"$1\" = \"--list\" ] && [ \"$2\" = \"--format\" ] && [ \"$3\" = \"terse\" ]; then\n\
               printf '%s\\n' 'stalled_case: test'\n\
               exit 0\n\
             fi\n\
             sleep 30\n",
        )?;
        let mut permissions = fs::metadata(&executable)?.permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&executable, permissions)?;

        let started = Instant::now();
        let report = run_native_test_batch_all_in_directory_with_timeout(
            &executable,
            &BTreeMap::new(),
            Some(output.path()),
            Some(Duration::from_millis(10)),
        )?;
        let executable_display = executable.display().to_string();
        assert!(!report.success, "{report:#?}");
        assert!(report.timed_out, "{report:#?}");
        assert!(report.output.contains("timed out after 10ms"), "{report:#?}");
        assert!(report.output.contains(&executable_display), "{report:#?}");
        assert!(
            started.elapsed() < Duration::from_secs(15),
            "descendant-held pipes outlived the process-group timeout"
        );
        Ok(())
    }

    #[test]
    fn timeout_supervisor_drains_large_child_output_while_it_runs() -> Result<(), Box<dyn std::error::Error>> {
        use std::time::Duration;

        let output = tempfile::tempdir()?;
        let source = output.path().join("large-output-native-test.rs");
        let executable = output.path().join("large-output-native-test");
        fs::write(
            &source,
            "#[test]\n\
             fn large_output() -> Result<(), Box<dyn std::error::Error>> {\n\
                 use std::io::Write;\n\
                 let bytes = vec![b'x'; 1024 * 1024];\n\
                 std::io::stdout().write_all(&bytes)?;\n\
                 std::io::stderr().write_all(&bytes)?;\n\
                 Ok(())\n\
             }\n",
        )?;
        let status = Command::new(rustc_path()?)
            .arg("--test")
            .arg(&source)
            .arg("-o")
            .arg(&executable)
            .status()?;
        assert!(status.success());

        let report = run_native_test_batch_all_in_directory_with_timeout(
            &executable,
            &BTreeMap::new(),
            Some(output.path()),
            Some(Duration::from_secs(5)),
        )?;
        assert!(report.success, "{report:#?}");
        assert!(!report.timed_out, "{report:#?}");
        assert_eq!(
            report.case_counts,
            Some(OvenNativeTestCaseCounts {
                passed: 1,
                failed: 0,
                ignored: 0
            })
        );
        assert!(report.output.len() >= 2 * 1024 * 1024, "{}", report.output.len());
        Ok(())
    }

    fn write_project(path: &std::path::Path) -> Result<(), std::io::Error> {
        fs::write(
            path.join("Cargo.toml"),
            "[package]\nname = \"oven-native-tests\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )?;
        fs::write(
            path.join("Cargo.lock"),
            "# This file is automatically @generated by Cargo.\nversion = 4\n",
        )
    }

    fn rustc_path() -> Result<PathBuf, Box<dyn std::error::Error>> {
        let output = Command::new("rustup").args(["which", "rustc"]).output()?;
        if !output.status.success() {
            return Err("rustup could not locate rustc".into());
        }
        let path = PathBuf::from(String::from_utf8(output.stdout)?.trim());
        if !path.is_file() {
            return Err(format!("rustup returned a non-file rustc path: {}", path.display()).into());
        }
        Ok(path)
    }

    fn rustc_identity(rustc: &std::path::Path) -> Result<String, Box<dyn std::error::Error>> {
        let output = Command::new(rustc).arg("--version").output()?;
        if !output.status.success() {
            return Err(format!("rustc could not report its version: {}", rustc.display()).into());
        }
        Ok(String::from_utf8(output.stdout)?.trim().to_string())
    }
}
