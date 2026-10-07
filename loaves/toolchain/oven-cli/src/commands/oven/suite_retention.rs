//! Retention of compiler-suite reports, transcripts, and wall-time evidence.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use rustix::time::{ClockId, clock_gettime};
use serde::Serialize;

use super::{CliError, CliResult, ExitCode};

const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(30);
const TIMING_SCOPE: &str = "wrapper clock through retention snapshot; compiler interval includes invocation setup and trap entry; excludes final timing-file publication and process exit";

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
struct TranscriptEvidence {
    inventory_complete: bool,
    file_count: usize,
    input_bytes: u64,
    archive_bytes: Option<u64>,
}

#[derive(Debug, Serialize)]
struct RetentionTiming<'a> {
    schema_version: u8,
    complete: bool,
    exit_status: i32,
    replay_exit_status: i32,
    phases: &'a BTreeMap<String, u128>,
    transcripts: &'a TranscriptEvidence,
    wrapper_setup_elapsed_ms: Option<u128>,
    compiler_command_elapsed_ms: Option<u128>,
    retention_elapsed_ms: u128,
    wrapper_elapsed_ms: Option<u128>,
    scope: &'static str,
}

#[derive(Debug)]
struct RetentionRequest {
    output: PathBuf,
    scratch: PathBuf,
    succeeded: bool,
    report: Option<PathBuf>,
    replay_status: i32,
    wrapper_started: Option<u128>,
    command_started: Option<u128>,
}

/// Print a wrapper clock or retain one replay's evidence and return its final status.
pub fn oven_retain_suite_output(clock: bool, arguments: &[String]) -> CliResult<ExitCode> {
    if clock {
        println!("{}", monotonic_ns()?);
        return Ok(ExitCode::SUCCESS);
    }
    let request = parse_request(arguments)?;
    retain_suite_output(&request, HEARTBEAT_INTERVAL).map(ExitCode)
}

/// Parse the stable positional contract used by Make's exit traps.
fn parse_request(arguments: &[String]) -> CliResult<RetentionRequest> {
    if arguments.len() != 5 && arguments.len() != 7 {
        return Err(CliError::failure(
            "usage: incan oven retain-suite-output OUTPUT TMP SUCCEEDED REPORT EXIT_STATUS [WRAPPER_START COMMAND_START]",
        ));
    }
    Ok(RetentionRequest {
        output: PathBuf::from(&arguments[0]),
        scratch: PathBuf::from(&arguments[1]),
        succeeded: arguments[2] == "true",
        report: (!arguments[3].is_empty()).then(|| PathBuf::from(&arguments[3])),
        replay_status: parse_number(&arguments[4], "exit status")?,
        wrapper_started: arguments
            .get(5)
            .map(|value| parse_number(value, "wrapper clock"))
            .transpose()?,
        command_started: arguments
            .get(6)
            .map(|value| parse_number(value, "command clock"))
            .transpose()?,
    })
}

/// Parse one numeric command-contract value with a useful diagnostic.
fn parse_number<T>(value: &str, label: &str) -> CliResult<T>
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    value
        .parse()
        .map_err(|error| CliError::failure(format!("invalid {label}: {error}")))
}

/// Execute retention while preserving the replay status separately from retention failures.
fn retain_suite_output(request: &RetentionRequest, heartbeat_interval: Duration) -> CliResult<i32> {
    let retention_started = monotonic_ns()?;
    let mut phases = BTreeMap::new();
    let mut transcripts = TranscriptEvidence::default();
    let mut status = request.replay_status;
    let mut failed = false;

    if let Err(error) = measured_phase("wrapper_scratch_cleanup", &mut phases, heartbeat_interval, || {
        remove_path(&request.scratch)
    }) {
        eprintln!("Oven scratch cleanup failed: {error}");
        failed = true;
    }

    let timing_path = request.report.as_ref().map(|path| timing_path_for(path));
    if let Some(report) = &request.report {
        if let Err(error) = prepare_evidence_paths(&request.output, report) {
            eprintln!("{error}; Oven suite output retained at {}", request.output.display());
            return Ok(1);
        }
        if let Err(error) = publish_report(
            &request.output,
            report,
            request.succeeded,
            &mut phases,
            heartbeat_interval,
        ) {
            eprintln!("Oven report publication failed; retaining complete caller output: {error}");
            failed = true;
        }
        if let Err(error) = inventory_transcripts(&request.output, &mut transcripts, &mut phases, heartbeat_interval) {
            eprintln!("Oven transcript inventory failed; retaining complete caller output: {error}");
            failed = true;
        }
        if transcripts.inventory_complete
            && let Err(error) = publish_transcript_archive(
                &request.output,
                report,
                &mut transcripts,
                &mut phases,
                heartbeat_interval,
            )
        {
            eprintln!("Oven transcript archive failed; retaining complete caller output: {error}");
            failed = true;
        }
    }

    if failed {
        status = 1;
    }
    if let Some(path) = &timing_path
        && let Err(error) =
            write_timing_snapshot(path, false, status, request, &phases, &transcripts, retention_started)
    {
        eprintln!("Oven timing publication failed; retaining caller output: {error}");
        status = 1;
    }

    if request.succeeded && status == 0 {
        if let Err(error) = measured_phase("caller_output_cleanup", &mut phases, heartbeat_interval, || {
            remove_path(&request.output)
        }) {
            eprintln!("Oven caller output cleanup failed: {error}");
            status = 1;
        }
    } else {
        eprintln!("Oven suite output retained at {}", request.output.display());
    }

    if let Some(path) = &timing_path
        && let Err(error) = write_timing_snapshot(path, true, status, request, &phases, &transcripts, retention_started)
    {
        eprintln!("Oven final timing publication failed: {error}");
        status = 1;
    }
    let elapsed = monotonic_ns()?.saturating_sub(request.wrapper_started.unwrap_or(retention_started));
    announce(
        if status == 0 { "DONE" } else { "FAILED" },
        if request.wrapper_started.is_some() {
            "suite wrapper"
        } else {
            "suite retention"
        },
        &format!("{:.3}s, exit {status}", elapsed as f64 / 1_000_000_000.0),
    );
    Ok(status)
}

/// Remove stale retained artifacts without accepting a directory in place of an artifact file.
fn prepare_evidence_paths(output: &Path, report: &Path) -> io::Result<()> {
    let report_parent = report.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(report_parent)?;
    let output = output.canonicalize()?;
    let report_parent = report_parent.canonicalize()?;
    let report_name = report
        .file_name()
        .ok_or_else(|| io::Error::other("Oven retained report needs a filename"))?;
    let resolved_report = report_parent.join(report_name);
    if resolved_report.starts_with(&output) {
        return Err(io::Error::other(
            "Oven retained report must differ from the disposable source report directory",
        ));
    }
    let source = output.join("compiler-suite-report.json");
    if source.exists() && resolved_report.exists() && source.canonicalize()? == resolved_report.canonicalize()? {
        return Err(io::Error::other(
            "Oven retained report must differ from the disposable source report",
        ));
    }
    for path in [
        resolved_report.clone(),
        archive_path_for(&resolved_report),
        timing_path_for(&resolved_report),
    ] {
        remove_artifact_file(&path)?;
    }
    Ok(())
}

/// Atomically copy the current report, or reject a green replay that did not produce it.
fn publish_report(
    output: &Path,
    report: &Path,
    succeeded: bool,
    phases: &mut BTreeMap<String, u128>,
    heartbeat_interval: Duration,
) -> io::Result<()> {
    measured_phase("report_publication", phases, heartbeat_interval, || {
        let source = output.join("compiler-suite-report.json");
        if source.is_file() && source.metadata()?.len() > 0 {
            atomic_copy(&source, report)
        } else if succeeded {
            Err(io::Error::other(
                "Oven replay succeeded without its requested JSON report",
            ))
        } else {
            Ok(())
        }
    })
}

/// Inventory regular transcript files without following directory or file symlinks.
fn inventory_transcripts(
    output: &Path,
    evidence: &mut TranscriptEvidence,
    phases: &mut BTreeMap<String, u128>,
    heartbeat_interval: Duration,
) -> io::Result<()> {
    measured_phase("transcript_inventory", phases, heartbeat_interval, || {
        let mut paths = Vec::new();
        walk_transcripts(output, output, &mut paths, evidence)?;
        paths.sort();
        evidence.inventory_complete = true;
        Ok(())
    })
}

/// Atomically publish a gzip-compressed tar archive when transcripts exist.
fn publish_transcript_archive(
    output: &Path,
    report: &Path,
    evidence: &mut TranscriptEvidence,
    phases: &mut BTreeMap<String, u128>,
    heartbeat_interval: Duration,
) -> io::Result<()> {
    if evidence.file_count == 0 {
        return Ok(());
    }
    measured_phase("transcript_archive", phases, heartbeat_interval, || {
        let mut paths = Vec::new();
        collect_transcript_paths(output, output, &mut paths)?;
        paths.sort();
        let parent = report.parent().unwrap_or_else(|| Path::new("."));
        let staged = tempfile::NamedTempFile::new_in(parent)?;
        let staged_path = staged.path().canonicalize()?;
        let mut child = Command::new("tar")
            .args(["--null", "-T", "-", "-czf"])
            .arg(&staged_path)
            .current_dir(output)
            .env("COPYFILE_DISABLE", "1")
            .stdin(Stdio::piped())
            .spawn()?;
        {
            let mut stdin = child
                .stdin
                .take()
                .ok_or_else(|| io::Error::other("tar stdin was unavailable"))?;
            for relative in paths {
                stdin.write_all(b"./")?;
                stdin.write_all(&path_bytes(&relative))?;
                stdin.write_all(&[0])?;
            }
        }
        let status = child.wait()?;
        if !status.success() {
            return Err(io::Error::other(format!("tar exited with {status}")));
        }
        let archive_bytes = staged.as_file().metadata()?.len();
        staged.persist(archive_path_for(report)).map_err(|error| error.error)?;
        evidence.archive_bytes = Some(archive_bytes);
        Ok(())
    })
}

/// Recursively count transcript files and retain their byte sizes.
fn walk_transcripts(
    root: &Path,
    directory: &Path,
    paths: &mut Vec<PathBuf>,
    evidence: &mut TranscriptEvidence,
) -> io::Result<()> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            walk_transcripts(root, &entry.path(), paths, evidence)?;
        } else if file_type.is_file() && entry.file_name().to_string_lossy().ends_with(".libtest-output.txt") {
            paths.push(entry.path().strip_prefix(root).map_err(io::Error::other)?.to_path_buf());
            evidence.file_count += 1;
            evidence.input_bytes += entry.metadata()?.len();
        }
    }
    Ok(())
}

/// Recursively recover the same transcript paths after a completed inventory.
fn collect_transcript_paths(root: &Path, directory: &Path, paths: &mut Vec<PathBuf>) -> io::Result<()> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            collect_transcript_paths(root, &entry.path(), paths)?;
        } else if file_type.is_file() && entry.file_name().to_string_lossy().ends_with(".libtest-output.txt") {
            paths.push(entry.path().strip_prefix(root).map_err(io::Error::other)?.to_path_buf());
        }
    }
    Ok(())
}

/// Measure one filesystem phase and emit periodic liveness until the operation returns.
fn measured_phase<T>(
    name: &str,
    phases: &mut BTreeMap<String, u128>,
    heartbeat_interval: Duration,
    operation: impl FnOnce() -> io::Result<T>,
) -> io::Result<T> {
    let started = monotonic_ns().map_err(io::Error::other)?;
    announce("START", name, "suite retention");
    let (stop_tx, stop_rx) = mpsc::channel();
    let heartbeat_name = name.to_owned();
    let heartbeat = thread::spawn(move || {
        while stop_rx.recv_timeout(heartbeat_interval).is_err() {
            let elapsed = monotonic_ns().map_or(0, |now| now.saturating_sub(started));
            announce(
                "WAIT",
                &heartbeat_name,
                &format!("{:.1}s elapsed", elapsed as f64 / 1_000_000_000.0),
            );
        }
    });
    let result = operation();
    let _ = stop_tx.send(());
    let _ = heartbeat.join();
    let elapsed = monotonic_ns().map_err(io::Error::other)?.saturating_sub(started) / 1_000_000;
    phases.insert(name.to_owned(), elapsed);
    announce(
        if result.is_ok() { "DONE" } else { "FAILED" },
        name,
        &format!("{:.3}s", elapsed as f64 / 1_000.0),
    );
    result
}

/// Publish one timing snapshot atomically so an interrupted write cannot become evidence.
fn write_timing_snapshot(
    path: &Path,
    complete: bool,
    status: i32,
    request: &RetentionRequest,
    phases: &BTreeMap<String, u128>,
    transcripts: &TranscriptEvidence,
    retention_started: u128,
) -> io::Result<()> {
    let current = monotonic_ns().map_err(io::Error::other)?;
    let value = RetentionTiming {
        schema_version: 1,
        complete,
        exit_status: status,
        replay_exit_status: request.replay_status,
        phases,
        transcripts,
        wrapper_setup_elapsed_ms: request.wrapper_started.map(|wrapper| {
            request
                .command_started
                .unwrap_or(retention_started)
                .saturating_sub(wrapper)
                / 1_000_000
        }),
        compiler_command_elapsed_ms: request
            .command_started
            .map(|command| retention_started.saturating_sub(command) / 1_000_000),
        retention_elapsed_ms: current.saturating_sub(retention_started) / 1_000_000,
        wrapper_elapsed_ms: request
            .wrapper_started
            .map(|wrapper| current.saturating_sub(wrapper) / 1_000_000),
        scope: TIMING_SCOPE,
    };
    atomic_json(path, &value)
}

/// Serialize one JSON value through a sibling temporary file and atomic rename.
fn atomic_json(path: &Path, value: &impl Serialize) -> io::Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    serde_json::to_writer_pretty(&mut temporary, value).map_err(io::Error::other)?;
    writeln!(temporary)?;
    temporary.as_file().sync_all()?;
    temporary.persist(path).map_err(|error| error.error)?;
    Ok(())
}

/// Copy a file through a sibling temporary file and atomic rename.
fn atomic_copy(source: &Path, destination: &Path) -> io::Result<()> {
    let parent = destination.parent().unwrap_or_else(|| Path::new("."));
    let mut input = File::open(source)?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    io::copy(&mut input, &mut temporary)?;
    temporary.as_file().sync_all()?;
    temporary.persist(destination).map_err(|error| error.error)?;
    Ok(())
}

/// Remove a disposable file or directory if it exists.
fn remove_path(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() => fs::remove_dir_all(path),
        Ok(_) => fs::remove_file(path),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

/// Remove one retained artifact while rejecting a directory at that location.
fn remove_artifact_file(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

/// Return the timing sidecar path for a retained report.
fn timing_path_for(report: &Path) -> PathBuf {
    PathBuf::from(format!("{}.wall-time.json", report.display()))
}

/// Return the transcript archive path for a retained report.
fn archive_path_for(report: &Path) -> PathBuf {
    PathBuf::from(format!("{}.transcripts.tar.gz", report.display()))
}

/// Preserve path bytes when sending the null-delimited transcript list to `tar`.
#[cfg(unix)]
fn path_bytes(path: &Path) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;

    path.as_os_str().as_bytes().to_vec()
}

/// Encode platform paths for the archive helper on non-Unix hosts.
#[cfg(not(unix))]
fn path_bytes(path: &Path) -> Vec<u8> {
    path.to_string_lossy().into_owned().into_bytes()
}

/// Return a nanosecond token from the host monotonic clock shared across processes.
fn monotonic_ns() -> CliResult<u128> {
    let time = clock_gettime(ClockId::Monotonic);
    let seconds =
        u128::try_from(time.tv_sec).map_err(|error| CliError::failure(format!("invalid monotonic clock: {error}")))?;
    let nanoseconds =
        u128::try_from(time.tv_nsec).map_err(|error| CliError::failure(format!("invalid monotonic clock: {error}")))?;
    Ok(seconds.saturating_mul(1_000_000_000).saturating_add(nanoseconds))
}

/// Emit one aligned phase or wrapper progress record.
fn announce(state: &str, subject: &str, detail: &str) {
    eprintln!("{state:<10} {subject} ({detail})");
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build one representative replay output and request.
    fn fixture(succeeded: bool, report_exists: bool) -> io::Result<(tempfile::TempDir, RetentionRequest)> {
        let root = tempfile::tempdir()?;
        let output = root.path().join("oven-compiler-suite-output.probe");
        let scratch = root.path().join("incan-oven-suite.probe");
        let transcripts = output.join("shards/path with spaces");
        fs::create_dir_all(&transcripts)?;
        fs::create_dir(&scratch)?;
        fs::write(
            transcripts.join("probe.libtest-output.txt"),
            b"complete captured diagnostic\n",
        )?;
        fs::write(output.join("caller-binary"), b"not an artifact")?;
        if report_exists {
            fs::write(output.join("compiler-suite-report.json"), b"{\"probe\": true}\n")?;
        }
        let now = monotonic_ns().map_err(io::Error::other)?;
        let request = RetentionRequest {
            output,
            scratch,
            succeeded,
            report: Some(root.path().join("retained/report.json")),
            replay_status: if succeeded { 0 } else { 9 },
            wrapper_started: Some(now.saturating_sub(700_000_000)),
            command_started: Some(now.saturating_sub(500_000_000)),
        };
        Ok((root, request))
    }

    /// Decode one timing sidecar into an owned JSON value.
    fn timing(request: &RetentionRequest) -> io::Result<serde_json::Value> {
        let report = request
            .report
            .as_deref()
            .ok_or_else(|| io::Error::other("fixture needs report"))?;
        let bytes = fs::read(timing_path_for(report))?;
        serde_json::from_slice(&bytes).map_err(io::Error::other)
    }

    /// List archive paths and read the first archived transcript.
    fn archive(request: &RetentionRequest) -> io::Result<(Vec<PathBuf>, Vec<u8>)> {
        let report = request
            .report
            .as_deref()
            .ok_or_else(|| io::Error::other("fixture needs report"))?;
        let archive = archive_path_for(report);
        let listing = Command::new("tar").args(["-tzf"]).arg(&archive).output()?;
        if !listing.status.success() {
            return Err(io::Error::other("could not list transcript archive"));
        }
        let paths = String::from_utf8(listing.stdout)
            .map_err(io::Error::other)?
            .lines()
            .map(|path| PathBuf::from(path.strip_prefix("./").unwrap_or(path)))
            .collect();
        let contents = Command::new("tar").args(["-xOzf"]).arg(&archive).output()?;
        if !contents.status.success() {
            return Err(io::Error::other("could not read transcript archive"));
        }
        Ok((paths, contents.stdout))
    }

    #[test]
    fn success_retains_report_and_transcripts_before_reclaiming_output() -> Result<(), Box<dyn std::error::Error>> {
        let (_root, request) = fixture(true, true)?;
        assert_eq!(retain_suite_output(&request, Duration::from_millis(5))?, 0);
        assert!(!request.output.exists());
        let report = request.report.as_deref().ok_or("fixture needs report")?;
        assert_eq!(fs::read_to_string(report)?, "{\"probe\": true}\n");
        let (paths, bytes) = archive(&request)?;
        assert_eq!(
            paths,
            vec![PathBuf::from("shards/path with spaces/probe.libtest-output.txt")]
        );
        assert_eq!(bytes, b"complete captured diagnostic\n");
        let timing = timing(&request)?;
        assert_eq!(timing["exit_status"], 0);
        assert!(timing["complete"].as_bool().ok_or("complete must be bool")?);
        assert_eq!(timing["transcripts"]["file_count"], 1);
        assert!(timing["phases"]["caller_output_cleanup"].is_number());
        assert!(
            timing["wrapper_setup_elapsed_ms"]
                .as_u64()
                .ok_or("wrapper setup must be integer")?
                >= 200
        );
        Ok(())
    }

    #[test]
    fn failed_replay_without_report_archives_diagnostics_and_keeps_output() -> Result<(), Box<dyn std::error::Error>> {
        let (_root, request) = fixture(false, false)?;
        assert_eq!(retain_suite_output(&request, Duration::from_millis(5))?, 9);
        assert!(request.output.exists());
        let report = request.report.as_deref().ok_or("fixture needs report")?;
        assert!(!report.exists());
        assert_eq!(archive(&request)?.1, b"complete captured diagnostic\n");
        let timing = timing(&request)?;
        assert_eq!(timing["exit_status"], 9);
        assert!(timing["phases"].get("caller_output_cleanup").is_none());
        Ok(())
    }

    #[test]
    fn green_replay_without_report_fails_and_keeps_output() -> Result<(), Box<dyn std::error::Error>> {
        let (_root, request) = fixture(true, false)?;
        assert_eq!(retain_suite_output(&request, Duration::from_millis(5))?, 1);
        assert!(request.output.exists());
        assert_eq!(archive(&request)?.1, b"complete captured diagnostic\n");
        Ok(())
    }

    #[test]
    fn previous_evidence_is_removed_before_failed_replay_publication() -> Result<(), Box<dyn std::error::Error>> {
        let (_root, request) = fixture(false, false)?;
        let report = request.report.as_deref().ok_or("fixture needs report")?;
        fs::create_dir_all(report.parent().ok_or("report needs parent")?)?;
        fs::write(report, b"old report")?;
        fs::write(archive_path_for(report), b"old archive")?;
        fs::write(timing_path_for(report), b"old timing")?;
        assert_eq!(retain_suite_output(&request, Duration::from_millis(5))?, 9);
        assert!(!report.exists());
        assert_ne!(fs::read(archive_path_for(report))?, b"old archive");
        assert!(!fs::read_to_string(timing_path_for(report))?.contains("old timing"));
        Ok(())
    }

    #[test]
    fn unpublishable_timing_artifact_preserves_output() -> Result<(), Box<dyn std::error::Error>> {
        let (_root, request) = fixture(true, true)?;
        let report = request.report.as_deref().ok_or("fixture needs report")?;
        fs::create_dir_all(report.parent().ok_or("report needs parent")?)?;
        fs::create_dir(timing_path_for(report))?;
        assert_eq!(retain_suite_output(&request, Duration::from_millis(5))?, 1);
        assert!(request.output.exists());
        Ok(())
    }

    #[test]
    fn report_inside_disposable_output_is_rejected() -> Result<(), Box<dyn std::error::Error>> {
        let (_root, mut request) = fixture(true, true)?;
        request.report = Some(request.output.join("compiler-suite-report.json"));
        assert_eq!(retain_suite_output(&request, Duration::from_millis(5))?, 1);
        assert!(request.output.exists());
        assert_eq!(
            fs::read_to_string(request.output.join("compiler-suite-report.json"))?,
            "{\"probe\": true}\n"
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn transcript_inventory_does_not_follow_symlinks() -> Result<(), Box<dyn std::error::Error>> {
        use std::os::unix::fs::symlink;

        let (_root, request) = fixture(true, true)?;
        let outside = request
            .output
            .parent()
            .ok_or("output needs parent")?
            .join("outside.libtest-output.txt");
        fs::write(&outside, b"outside")?;
        symlink(&outside, request.output.join("linked.libtest-output.txt"))?;
        symlink(
            outside.parent().ok_or("outside needs parent")?,
            request.output.join("linked-directory"),
        )?;
        assert_eq!(retain_suite_output(&request, Duration::from_millis(5))?, 0);
        let timing = timing(&request)?;
        assert_eq!(timing["transcripts"]["file_count"], 1);
        Ok(())
    }

    #[test]
    fn malformed_contract_values_are_rejected() {
        let arguments = vec!["out", "tmp", "true", "report", "not-a-status"]
            .into_iter()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        assert!(parse_request(&arguments).is_err());
        assert!(parse_request(&[]).is_err());
    }
}
