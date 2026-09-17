//! Process-tree containment and bounded output capture shared by Oven's child-process paths.

use std::io;
use std::process::{Child, Command, ExitStatus};
use std::sync::atomic::AtomicBool;
use std::time::Duration;

#[cfg(unix)]
use std::io::Read;
#[cfg(unix)]
use std::process::Stdio;
#[cfg(unix)]
use std::sync::Arc;
#[cfg(unix)]
use std::sync::atomic::Ordering;
#[cfg(unix)]
use std::thread;
#[cfg(unix)]
use std::time::Instant;

#[cfg(unix)]
const PROCESS_POLL_INTERVAL: Duration = Duration::from_millis(10);
#[cfg(unix)]
const STREAM_READ_BUFFER_BYTES: usize = 8 * 1024;

/// The reason a bounded child process stopped.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BoundedProcessTermination {
    /// The direct child exited before any configured refusal condition.
    Completed,
    /// The caller's cancellation flag was set while the child was running.
    Cancelled,
    /// The configured wall-clock timeout elapsed while the child was running.
    TimedOut,
    /// Captured standard output exceeded its configured byte limit.
    StdoutLimitExceeded,
    /// Captured standard error exceeded its configured byte limit.
    StderrLimitExceeded,
}

/// Limits that keep a child process and its retained diagnostics physically bounded.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BoundedProcessLimits {
    /// The maximum number of stdout bytes retained before the process is refused.
    pub stdout_bytes: usize,
    /// The maximum number of stderr bytes retained before the process is refused.
    pub stderr_bytes: usize,
    /// The maximum wall-clock duration for the process, or `None` to disable a timeout.
    pub timeout: Option<Duration>,
}

/// The direct-child status and bounded diagnostics retained from a subprocess attempt.
///
/// A result is accepted only when [`Self::termination`] is [`BoundedProcessTermination::Completed`]. A completed
/// result can still carry an unsuccessful program exit status; the adapter that supplied the command decides how to
/// interpret that program-specific status.
#[derive(Debug)]
pub struct BoundedProcessOutput {
    /// The reaped direct child's exit status.
    pub status: ExitStatus,
    /// At most [`BoundedProcessLimits::stdout_bytes`] bytes, captured while the process ran.
    pub stdout: Vec<u8>,
    /// At most [`BoundedProcessLimits::stderr_bytes`] bytes, captured while the process ran.
    pub stderr: Vec<u8>,
    /// Whether the process completed or was refused by a physical execution limit.
    pub termination: BoundedProcessTermination,
}

/// Put a child and all normally spawned descendants in an isolated process group.
pub fn isolate_process_group(command: &mut Command) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;

        command.process_group(0);
    }
    #[cfg(not(unix))]
    let _command = command;
}

/// Run a command with concurrently drained, separately bounded stdout and stderr.
///
/// The command receives no stdin and an isolated process group. The runner retains at most the requested number of
/// bytes from each stream, polls the optional cancellation flag and timeout until both readers reach EOF, closes the
/// isolated process group, and returns only after it has reaped the direct child. On Unix, ordinary compiler and
/// linker descendants cannot keep an inherited pipe open or survive either completion or refusal. Callers must accept
/// only a [`BoundedProcessTermination::Completed`] result and must construct the command from already admitted facts;
/// this primitive performs no command discovery or policy selection. On a non-Unix host it returns
/// `io::ErrorKind::Unsupported` before it mutates or spawns the command because no descendant-containment primitive
/// is available. On Unix, a cancellation flag already set at call time returns `io::ErrorKind::Interrupted` before
/// spawn.
pub fn run_bounded_process(
    command: &mut Command,
    limits: BoundedProcessLimits,
    cancellation: Option<&AtomicBool>,
) -> io::Result<BoundedProcessOutput> {
    #[cfg(unix)]
    {
        run_bounded_process_unix(command, limits, cancellation)
    }
    #[cfg(not(unix))]
    {
        let _command = command;
        let _limits = limits;
        let _cancellation = cancellation;
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "bounded subprocess execution requires Unix process-group containment",
        ))
    }
}

/// Execute the Unix process-group implementation after its platform capability has been admitted.
#[cfg(unix)]
fn run_bounded_process_unix(
    command: &mut Command,
    limits: BoundedProcessLimits,
    cancellation: Option<&AtomicBool>,
) -> io::Result<BoundedProcessOutput> {
    if cancellation.is_some_and(|flag| flag.load(Ordering::Acquire)) {
        return Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "bounded process was cancelled before spawn",
        ));
    }

    // ---- Child setup ----
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    isolate_process_group(command);

    let mut child = command.spawn()?;
    let process_id = child.id();
    let stdout = match child.stdout.take() {
        Some(stdout) => stdout,
        None => {
            terminate_process_group(&mut child)?;
            return Err(io::Error::other("bounded process did not provide piped stdout"));
        }
    };
    let stderr = match child.stderr.take() {
        Some(stderr) => stderr,
        None => {
            terminate_process_group(&mut child)?;
            return Err(io::Error::other("bounded process did not provide piped stderr"));
        }
    };
    let stdout_limit_exceeded = Arc::new(AtomicBool::new(false));
    let stderr_limit_exceeded = Arc::new(AtomicBool::new(false));
    let stream_reader_failed = Arc::new(AtomicBool::new(false));
    let stdout_reader_finished = Arc::new(AtomicBool::new(false));
    let stderr_reader_finished = Arc::new(AtomicBool::new(false));
    let stdout_reader = match spawn_limited_reader(
        stdout,
        limits.stdout_bytes,
        Arc::clone(&stdout_limit_exceeded),
        Arc::clone(&stream_reader_failed),
        Arc::clone(&stdout_reader_finished),
    ) {
        Ok(reader) => reader,
        Err(error) => {
            terminate_process_group(&mut child)?;
            return Err(error);
        }
    };
    let stderr_reader = match spawn_limited_reader(
        stderr,
        limits.stderr_bytes,
        Arc::clone(&stderr_limit_exceeded),
        Arc::clone(&stream_reader_failed),
        Arc::clone(&stderr_reader_finished),
    ) {
        Ok(reader) => reader,
        Err(error) => {
            terminate_process_group(&mut child)?;
            let _stdout = join_limited_reader(stdout_reader)?;
            return Err(error);
        }
    };

    // ---- Direct-child containment ----
    let started_at = Instant::now();
    let mut direct_child_status = None;
    let mut requested_termination = None;
    loop {
        if stream_reader_failed.load(Ordering::Acquire) {
            refuse_process_group(&mut child, process_id, &mut direct_child_status)?;
            wait_for_stream_readers(&stdout_reader_finished, &stderr_reader_finished);
            join_stream_reader_errors(stdout_reader, stderr_reader)?;
            return Err(io::Error::other(
                "bounded process stream reader failed without an I/O error",
            ));
        }

        if requested_termination.is_none() {
            let termination = if stdout_limit_exceeded.load(Ordering::Acquire) {
                Some(BoundedProcessTermination::StdoutLimitExceeded)
            } else if stderr_limit_exceeded.load(Ordering::Acquire) {
                Some(BoundedProcessTermination::StderrLimitExceeded)
            } else if cancellation.is_some_and(|flag| flag.load(Ordering::Acquire)) {
                Some(BoundedProcessTermination::Cancelled)
            } else if limits.timeout.is_some_and(|timeout| started_at.elapsed() >= timeout) {
                Some(BoundedProcessTermination::TimedOut)
            } else {
                None
            };

            if let Some(termination) = termination {
                refuse_process_group(&mut child, process_id, &mut direct_child_status)?;
                requested_termination = Some(termination);
            }
        }

        if direct_child_status.is_none() {
            match child.try_wait() {
                Ok(Some(status)) => direct_child_status = Some(status),
                Ok(None) => {}
                Err(error) => {
                    refuse_process_group(&mut child, process_id, &mut direct_child_status)?;
                    wait_for_stream_readers(&stdout_reader_finished, &stderr_reader_finished);
                    let _reader_result = join_stream_reader_errors(stdout_reader, stderr_reader);
                    return Err(error);
                }
            }
        }

        if direct_child_status.is_some() && stream_readers_finished(&stdout_reader_finished, &stderr_reader_finished) {
            break;
        }
        thread::sleep(PROCESS_POLL_INTERVAL);
    }

    // ---- Retained diagnostics ----
    terminate_remaining_process_group(process_id)?;
    let stdout = join_limited_reader(stdout_reader);
    let stderr = join_limited_reader(stderr_reader);
    let (stdout, stderr) = match (stdout, stderr) {
        (Ok(stdout), Ok(stderr)) => (stdout, stderr),
        (Err(error), _) | (_, Err(error)) => {
            terminate_remaining_process_group(process_id)?;
            return Err(error);
        }
    };
    let observed_termination = stream_limit_termination(
        stdout.limit_exceeded || stdout_limit_exceeded.load(Ordering::Acquire),
        stderr.limit_exceeded || stderr_limit_exceeded.load(Ordering::Acquire),
    );
    let termination = observed_termination
        .or(requested_termination)
        .unwrap_or(BoundedProcessTermination::Completed);
    let status = direct_child_status.ok_or_else(|| io::Error::other("bounded process exited without a status"))?;

    Ok(BoundedProcessOutput {
        status,
        stdout: stdout.bytes,
        stderr: stderr.bytes,
        termination,
    })
}

/// Terminate and reap an isolated child process group.
///
/// Cargo, Rustdoc, libtest, build scripts, compilers, and linkers inherit the isolated group unless they explicitly
/// create a new session, which none of the supported Oven child paths permit or require. GNU `kill` needs `--`
/// before a negative process-group ID, while the BSD implementation shipped by macOS rejects that separator.
pub fn terminate_process_group(child: &mut Child) -> io::Result<ExitStatus> {
    #[cfg(unix)]
    {
        let terminated = terminate_process_group_id(child.id())?;
        match child.try_wait() {
            Ok(Some(status)) => Ok(status),
            Ok(None) if !terminated => Err(io::Error::other(format!(
                "failed to terminate process group -{}",
                child.id()
            ))),
            Ok(None) | Err(_) => child.wait(),
        }
    }
    #[cfg(not(unix))]
    {
        child.kill()?;
        child.wait()
    }
}

/// Start an independent reader so either child stream can make progress while the other is busy.
#[cfg(unix)]
fn spawn_limited_reader<R>(
    reader: R,
    limit: usize,
    limit_exceeded: Arc<AtomicBool>,
    reader_failed: Arc<AtomicBool>,
    reader_finished: Arc<AtomicBool>,
) -> io::Result<thread::JoinHandle<io::Result<LimitedStream>>>
where
    R: Read + Send + 'static,
{
    thread::Builder::new()
        .name("oven-bounded-process-stream".to_string())
        .spawn(move || {
            let result = read_limited_stream(reader, limit, limit_exceeded, reader_failed);
            reader_finished.store(true, Ordering::Release);
            result
        })
}

/// Retain a prefix of one stream while continuing to drain it after the limit is observed.
#[cfg(unix)]
fn read_limited_stream<R>(
    mut reader: R,
    limit: usize,
    limit_exceeded: Arc<AtomicBool>,
    reader_failed: Arc<AtomicBool>,
) -> io::Result<LimitedStream>
where
    R: Read,
{
    let mut bytes = Vec::with_capacity(limit.min(STREAM_READ_BUFFER_BYTES));
    let mut buffer = [0_u8; STREAM_READ_BUFFER_BYTES];
    let mut exceeded = false;

    loop {
        let read = match reader.read(&mut buffer) {
            Ok(read) => read,
            Err(error) => {
                reader_failed.store(true, Ordering::Release);
                return Err(error);
            }
        };
        if read == 0 {
            break;
        }

        let remaining = limit.saturating_sub(bytes.len());
        let retained = remaining.min(read);
        bytes.extend_from_slice(&buffer[..retained]);
        if retained < read {
            exceeded = true;
            limit_exceeded.store(true, Ordering::Release);
        }
    }

    Ok(LimitedStream {
        bytes,
        limit_exceeded: exceeded,
    })
}

/// Collect a reader's retained stream and surface a reader panic as an I/O failure.
#[cfg(unix)]
fn join_limited_reader(handle: thread::JoinHandle<io::Result<LimitedStream>>) -> io::Result<LimitedStream> {
    handle
        .join()
        .map_err(|_| io::Error::other("bounded process stream reader panicked"))?
}

/// Join both failed reader tasks before surfacing their first I/O error to the caller.
#[cfg(unix)]
fn join_stream_reader_errors(
    stdout_reader: thread::JoinHandle<io::Result<LimitedStream>>,
    stderr_reader: thread::JoinHandle<io::Result<LimitedStream>>,
) -> io::Result<()> {
    let stdout = join_limited_reader(stdout_reader);
    let stderr = join_limited_reader(stderr_reader);
    match (stdout, stderr) {
        (Err(error), _) | (_, Err(error)) => Err(error),
        (Ok(_), Ok(_)) => Ok(()),
    }
}

/// Terminate a running direct child or its remaining Unix process group after direct-child exit.
#[cfg(unix)]
fn refuse_process_group(
    child: &mut Child,
    process_id: u32,
    direct_child_status: &mut Option<ExitStatus>,
) -> io::Result<()> {
    if direct_child_status.is_none() {
        *direct_child_status = Some(terminate_process_group(child)?);
    } else {
        terminate_remaining_process_group(process_id)?;
    }
    Ok(())
}

/// Return whether both concurrent stream readers have reached EOF or reported an I/O failure.
#[cfg(unix)]
fn stream_readers_finished(stdout_finished: &AtomicBool, stderr_finished: &AtomicBool) -> bool {
    stdout_finished.load(Ordering::Acquire) && stderr_finished.load(Ordering::Acquire)
}

/// Poll reader completion after process-group refusal so joining cannot wait on an inherited pipe indefinitely.
#[cfg(unix)]
fn wait_for_stream_readers(stdout_finished: &AtomicBool, stderr_finished: &AtomicBool) {
    while !stream_readers_finished(stdout_finished, stderr_finished) {
        thread::sleep(PROCESS_POLL_INTERVAL);
    }
}

/// Prefer stdout when both streams cross their limits before the polling loop observes either one.
#[cfg(unix)]
fn stream_limit_termination(stdout_exceeded: bool, stderr_exceeded: bool) -> Option<BoundedProcessTermination> {
    if stdout_exceeded {
        Some(BoundedProcessTermination::StdoutLimitExceeded)
    } else if stderr_exceeded {
        Some(BoundedProcessTermination::StderrLimitExceeded)
    } else {
        None
    }
}

/// The retained prefix and refusal observation from a single stream reader.
#[cfg(unix)]
struct LimitedStream {
    bytes: Vec<u8>,
    limit_exceeded: bool,
}

/// Kill a Unix process group and report whether the operating system accepted the signal.
#[cfg(unix)]
fn terminate_process_group_id(process_id: u32) -> io::Result<bool> {
    let process_group = format!("-{process_id}");
    let mut command = Command::new("/bin/kill");
    command.arg("-KILL");
    #[cfg(target_os = "linux")]
    command.arg("--");
    Ok(command
        .arg(process_group)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?
        .success())
}

/// Ensure an output-limit observation made after direct-child exit cannot leave a Unix descendant alive.
#[cfg(unix)]
fn terminate_remaining_process_group(process_id: u32) -> io::Result<()> {
    let _was_terminated = terminate_process_group_id(process_id)?;
    Ok(())
}

#[cfg(all(any(test, feature = "test_support"), unix))]
/// Return whether a process is still running rather than an unreaped zombie.
///
/// Terminating a process group necessarily reaps the direct child, but its descendants become children of the host
/// reaper. A normal host init reaps those immediately; a test container may run a non-reaping PID 1, leaving an
/// inert zombie that still answers `kill -0`. Capacity containment cares about executable descendants and their disk
/// activity, so a zombie is correctly considered stopped.
pub fn process_is_running(pid: u32) -> io::Result<bool> {
    let exists = Command::new("/bin/kill")
        .args(["-0", &pid.to_string()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?
        .success();
    if !exists {
        return Ok(false);
    }

    let status = Command::new("/bin/ps")
        .args(["-o", "stat=", "-p", &pid.to_string()])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()?;
    if !status.status.success() {
        return Ok(false);
    }
    Ok(!String::from_utf8_lossy(&status.stdout).trim_start().starts_with('Z'))
}

#[cfg(all(test, unix))]
mod tests {
    use std::error::Error;
    use std::fs;
    use std::io;
    use std::process::Command;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::thread;
    use std::time::Duration;

    use tempfile::tempdir;

    use super::{BoundedProcessLimits, BoundedProcessTermination, process_is_running, run_bounded_process};

    const SHORT_TIMEOUT: Duration = Duration::from_secs(1);

    #[test]
    /// Preserve separate stdout and stderr from a successful bounded command.
    fn captures_both_streams_without_refusal() -> Result<(), Box<dyn Error>> {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "printf stdout; printf stderr >&2"]);

        let output = run_bounded_process(
            &mut command,
            BoundedProcessLimits {
                stdout_bytes: 16,
                stderr_bytes: 16,
                timeout: Some(SHORT_TIMEOUT),
            },
            None,
        )?;

        assert_eq!(output.termination, BoundedProcessTermination::Completed);
        assert!(output.status.success());
        assert_eq!(output.stdout, b"stdout");
        assert_eq!(output.stderr, b"stderr");
        Ok(())
    }

    #[test]
    /// Refuse oversized stdout while retaining only the configured diagnostic prefix.
    fn stdout_limit_refuses_and_retains_only_the_configured_prefix() -> Result<(), Box<dyn Error>> {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "printf 123456"]);

        let output = run_bounded_process(
            &mut command,
            BoundedProcessLimits {
                stdout_bytes: 3,
                stderr_bytes: 16,
                timeout: Some(SHORT_TIMEOUT),
            },
            None,
        )?;

        assert_eq!(output.termination, BoundedProcessTermination::StdoutLimitExceeded);
        assert_eq!(output.stdout, b"123");
        Ok(())
    }

    #[test]
    /// Refuse oversized stderr independently of the stdout limit.
    fn stderr_limit_refuses_and_retains_only_the_configured_prefix() -> Result<(), Box<dyn Error>> {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "printf 123456 >&2"]);

        let output = run_bounded_process(
            &mut command,
            BoundedProcessLimits {
                stdout_bytes: 16,
                stderr_bytes: 3,
                timeout: Some(SHORT_TIMEOUT),
            },
            None,
        )?;

        assert_eq!(output.termination, BoundedProcessTermination::StderrLimitExceeded);
        assert_eq!(output.stderr, b"123");
        Ok(())
    }

    #[test]
    /// Stop and reap a running command after the caller requests cancellation.
    fn cancellation_refuses_a_running_child() -> Result<(), Box<dyn Error>> {
        let cancellation = Arc::new(AtomicBool::new(false));
        let cancellation_for_thread = Arc::clone(&cancellation);
        let setter = thread::spawn(move || {
            thread::sleep(Duration::from_millis(50));
            cancellation_for_thread.store(true, Ordering::Release);
        });
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "sleep 30"]);

        let output = run_bounded_process(
            &mut command,
            BoundedProcessLimits {
                stdout_bytes: 16,
                stderr_bytes: 16,
                timeout: Some(SHORT_TIMEOUT),
            },
            Some(cancellation.as_ref()),
        )?;
        setter.join().map_err(|_| "cancellation setter panicked")?;

        assert_eq!(output.termination, BoundedProcessTermination::Cancelled);
        Ok(())
    }

    #[test]
    /// Prove an already cancelled request cannot launch command side effects.
    fn pre_cancelled_request_does_not_spawn_the_command() -> Result<(), Box<dyn Error>> {
        let directory = tempdir()?;
        let marker = directory.path().join("command-started");
        let cancellation = AtomicBool::new(true);
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "printf started > \"$1\"", "bounded-process"]);
        command.arg(&marker);

        let error = match run_bounded_process(
            &mut command,
            BoundedProcessLimits {
                stdout_bytes: 16,
                stderr_bytes: 16,
                timeout: Some(SHORT_TIMEOUT),
            },
            Some(&cancellation),
        ) {
            Ok(_) => return Err("pre-cancelled command unexpectedly ran".into()),
            Err(error) => error,
        };

        assert_eq!(error.kind(), io::ErrorKind::Interrupted);
        assert!(!marker.exists());
        Ok(())
    }

    #[test]
    /// Verify timeout also terminates a descendant in the owned process group.
    fn timeout_reaps_a_descendant_in_the_isolated_group() -> Result<(), Box<dyn Error>> {
        let directory = tempdir()?;
        let descendant_pid = directory.path().join("descendant-pid");
        let mut command = Command::new("/bin/sh");
        command.args([
            "-c",
            "sleep 30 & child=$!; printf '%s' \"$child\" > \"$1\"; wait",
            "bounded-process",
        ]);
        command.arg(&descendant_pid);

        let output = run_bounded_process(
            &mut command,
            BoundedProcessLimits {
                stdout_bytes: 16,
                stderr_bytes: 16,
                timeout: Some(Duration::from_millis(250)),
            },
            None,
        )?;

        let pid = fs::read_to_string(&descendant_pid)?.trim().parse::<u32>()?;
        assert_eq!(output.termination, BoundedProcessTermination::TimedOut);
        assert!(!process_is_running(pid)?);
        Ok(())
    }

    #[test]
    /// Keep the timeout active while inherited pipes remain open after direct-child exit.
    fn timeout_survives_direct_child_exit_while_a_quiet_descendant_holds_the_pipes() -> Result<(), Box<dyn Error>> {
        let directory = tempdir()?;
        let descendant_pid = directory.path().join("descendant-pid");
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "sleep 30 & printf '%s' \"$!\" > \"$1\"", "bounded-process"]);
        command.arg(&descendant_pid);

        let started_at = std::time::Instant::now();
        let output = run_bounded_process(
            &mut command,
            BoundedProcessLimits {
                stdout_bytes: 16,
                stderr_bytes: 16,
                timeout: Some(Duration::from_millis(150)),
            },
            None,
        )?;

        let pid = fs::read_to_string(&descendant_pid)?.trim().parse::<u32>()?;
        assert_eq!(output.termination, BoundedProcessTermination::TimedOut);
        assert!(started_at.elapsed() < Duration::from_secs(2));
        assert!(!process_is_running(pid)?);
        Ok(())
    }

    #[test]
    /// Prevent successful completion from leaving a background descendant alive.
    fn completion_reaps_a_descendant_that_closed_all_inherited_pipes() -> Result<(), Box<dyn Error>> {
        let directory = tempdir()?;
        let descendant_pid = directory.path().join("descendant-pid");
        let mut command = Command::new("/bin/sh");
        command.args([
            "-c",
            "sleep 30 </dev/null >/dev/null 2>/dev/null & printf '%s' \"$!\" > \"$1\"",
            "bounded-process",
        ]);
        command.arg(&descendant_pid);

        let output = run_bounded_process(
            &mut command,
            BoundedProcessLimits {
                stdout_bytes: 16,
                stderr_bytes: 16,
                timeout: Some(SHORT_TIMEOUT),
            },
            None,
        )?;

        let pid = fs::read_to_string(&descendant_pid)?.trim().parse::<u32>()?;
        assert_eq!(output.termination, BoundedProcessTermination::Completed);
        assert!(output.status.success());
        assert!(!process_is_running(pid)?);
        Ok(())
    }

    #[test]
    /// Enforce the output cap while a descendant writes after the direct child exits.
    fn output_limit_survives_direct_child_exit_while_a_descendant_floods_stdout() -> Result<(), Box<dyn Error>> {
        let directory = tempdir()?;
        let descendant_pid = directory.path().join("descendant-pid");
        let mut command = Command::new("/bin/sh");
        command.args([
            "-c",
            "(while :; do printf 0123456789; done) & printf '%s' \"$!\" > \"$1\"",
            "bounded-process",
        ]);
        command.arg(&descendant_pid);

        let started_at = std::time::Instant::now();
        let output = run_bounded_process(
            &mut command,
            BoundedProcessLimits {
                stdout_bytes: 16,
                stderr_bytes: 16,
                timeout: Some(SHORT_TIMEOUT),
            },
            None,
        )?;

        let pid = fs::read_to_string(&descendant_pid)?.trim().parse::<u32>()?;
        assert_eq!(output.termination, BoundedProcessTermination::StdoutLimitExceeded);
        assert_eq!(output.stdout.len(), 16);
        assert!(started_at.elapsed() < Duration::from_secs(2));
        assert!(!process_is_running(pid)?);
        Ok(())
    }
}

#[cfg(all(test, not(unix)))]
mod unsupported_platform_tests {
    use std::error::Error;
    use std::io;
    use std::process::Command;

    use super::{BoundedProcessLimits, run_bounded_process};

    #[test]
    /// Refuse unsupported hosts before attempting to launch even an invalid executable.
    fn bounded_execution_is_refused_before_command_spawn() -> Result<(), Box<dyn Error>> {
        let mut command = Command::new("command-that-must-not-spawn");
        let error = match run_bounded_process(
            &mut command,
            BoundedProcessLimits {
                stdout_bytes: 16,
                stderr_bytes: 16,
                timeout: None,
            },
            None,
        ) {
            Ok(_) => return Err("unsupported platform unexpectedly ran the command".into()),
            Err(error) => error,
        };

        assert_eq!(error.kind(), io::ErrorKind::Unsupported);
        Ok(())
    }
}
