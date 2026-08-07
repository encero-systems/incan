//! Native test inventory and exact execution for Oven Alpha's direct-rustc consumers.
//!
//! Oven executes the libtest binary it built itself. It obtains a real inventory first and rejects a requested exact
//! test absent from that inventory, so a zero-match filter can never become a misleading success. Neither collection
//! nor execution launches Cargo or inherits Cargo process state.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{self, Read};
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
    /// Combined libtest transcript retained for per-test result mapping by the caller.
    pub output: String,
}

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
    let executable = verified_executable(executable)?;
    let mut command = Command::new(&executable);
    command.args(["--list", "--format", "terse"]);
    if let Some(working_directory) = working_directory {
        command.current_dir(working_directory);
    }
    clear_inherited_cargo_environment(&mut command);
    command.envs(environment);
    let output = command.output().map_err(|source| OvenNativeTestError::Io {
        path: executable.clone(),
        source,
    })?;
    let transcript = combined_output(&output.stdout, &output.stderr);
    if !output.status.success() {
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
    let inventory = inventory_native_tests_with_environment(&request.executable, &request.environment, None, false)?;
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
    let (output, timed_out) = run_native_batch_child(command, &executable, request.timeout)?;
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
    Ok(OvenNativeTestBatchReport {
        inventory,
        success: output.status.success() && !timed_out,
        timed_out,
        case_counts: parse_libtest_case_counts(&transcript),
        output: transcript,
    })
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
    let inventory = inventory_native_tests_with_environment(executable, environment, working_directory, true)?;
    let executable = verified_executable(executable)?;
    let mut command = Command::new(&executable);
    command.arg("--nocapture");
    if let Some(working_directory) = working_directory {
        command.current_dir(working_directory);
    }
    clear_inherited_cargo_environment(&mut command);
    command.envs(environment);
    let (output, timed_out) = run_native_batch_child(command, &executable, timeout)?;
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
    Ok(OvenNativeTestBatchReport {
        inventory,
        success: output.status.success() && !timed_out,
        timed_out,
        case_counts: parse_libtest_case_counts(&transcript),
        output: transcript,
    })
}

/// Spawn one captured native libtest child and enforce an optional execution-group deadline.
///
/// `Command::output` cannot supervise a running child. Keeping this small polling loop here ensures the same
/// Cargo-free environment and output capture apply to a terminated child as to a normally completed libtest process.
fn run_native_batch_child(
    mut command: Command,
    executable: &Path,
    timeout: Option<Duration>,
) -> Result<(std::process::Output, bool), OvenNativeTestError> {
    let timeout = match timeout {
        Some(timeout) => timeout,
        None => {
            let output = command.output().map_err(|source| OvenNativeTestError::Io {
                path: executable.to_path_buf(),
                source,
            })?;
            return Ok((output, false));
        }
    };

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
    let stdout_reader = thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout.read_to_end(&mut bytes)?;
        Ok::<_, io::Error>(bytes)
    });
    let stderr_reader = thread::spawn(move || {
        let mut bytes = Vec::new();
        stderr.read_to_end(&mut bytes)?;
        Ok::<_, io::Error>(bytes)
    });
    let deadline = Instant::now() + timeout;
    let mut timed_out = false;
    let status = loop {
        match child.try_wait().map_err(|source| OvenNativeTestError::Io {
            path: executable.to_path_buf(),
            source,
        })? {
            Some(status) => break status,
            None if Instant::now() >= deadline => {
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
        OvenNativeTestCaseCounts, OvenNativeTestError, OvenNativeTestRequest, parse_libtest_case_counts,
        run_native_test_batch, run_native_test_batch_all, run_native_test_batch_all_in_directory_with_timeout,
        run_native_tests,
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
