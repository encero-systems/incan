//! A serial native compiler worker. Oven owns its generation, startup and retirement policy; this boundary retains
//! immutable artifact readings only. Each request (a unit compilation, a check or a formatting run) runs the same
//! entry point as the short-lived command, with a fresh session and rustc context, and reports the same status.
//! A panic, or a console that cannot be restored after a request, retires the process instead of exposing possibly
//! poisoned native state to another request, and so does a physical footprint above [`RETIREMENT_FOOTPRINT`] after a
//! request: only exiting returns a heavy compile's peak heap to the system. A retiring worker withdraws its endpoint,
//! then sends its last reply with `"retired": true`.

use std::fs::{self, Permissions};
use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::json;

mod console;

const SCHEMA: &str = "incan.compiler-worker/1";
const MAX_FRAME: usize = 16 * 1024 * 1024;
/// The status Rust gives a process whose main thread panics; a request that panics reports the same status.
const PANIC_STATUS: i32 = 101;
/// The physical footprint, in bytes, above which a worker retires after replying to its request.
///
/// Measured on macOS (driver `1ced3dfbb3dc`) with one worker per dev-loop subject over five rounds of an edited unit
/// compile (`run` after an edit), an edited `check` and `fmt --check`: the peak physical footprint was 78 MB for the
/// single-module and module subjects and 80 MB for the library subject; one worker serving the whole dev-loop harness
/// (every subject and metric, 15 rounds) peaked at 182 MB. One GiB leaves more than five times the harness peak; a
/// compile of the lowering library through the worker passes 6.5 GB and retires.
const RETIREMENT_FOOTPRINT: u64 = 1 << 30;
/// A test-only setting that lowers, and never raises, [`RETIREMENT_FOOTPRINT`] for one worker generation, so a
/// contract case can retire a worker without a heavy compile.
const RETIREMENT_FOOTPRINT_SETTING: &str = "INCAN_WORKER_FOOTPRINT_BOUND";

/// Remove only the socket inode this worker created, leaving any replacement untouched.
struct SocketFile {
    path: PathBuf,
    device: u64,
    inode: u64,
}

impl Drop for SocketFile {
    /// Withdraw this worker's endpoint on normal retirement or a failed request boundary.
    fn drop(&mut self) {
        if fs::symlink_metadata(&self.path)
            .is_ok_and(|metadata| metadata.dev() == self.device && metadata.ino() == self.inode)
        {
            let _ = fs::remove_file(&self.path);
        }
    }
}

/// Bind one private endpoint, acknowledge readiness, and compile serial requests until idle, poisoned by a panic or
/// a failed console restoration, or above its footprint bound after a request.
pub(crate) fn serve(arguments: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let [socket, generation, ready, idle] = arguments else {
        return Err("usage: --resident SOCKET GENERATION READY IDLE_SECONDS".into());
    };
    let idle: u64 = idle.parse()?;
    if !(4..=3600).contains(&idle) {
        return Err("compiler worker idle seconds must be between 4 and 3600".into());
    }
    if !Path::new(socket).is_absolute() || !Path::new(ready).is_absolute() {
        return Err("compiler worker endpoints must be absolute".into());
    }
    // A worker can outlive the project that started it. Its idle directory must survive that project's deletion;
    // each request installs its caller's directory only for the ordinary compiler invocation.
    std::env::set_current_dir("/")?;
    let bound = retirement_bound();
    let listener = UnixListener::bind(socket)?;
    let metadata = fs::symlink_metadata(socket)?;
    let socket_file = SocketFile {
        path: socket.into(),
        device: metadata.dev(),
        inode: metadata.ino(),
    };
    fs::set_permissions(socket, Permissions::from_mode(0o600))?;
    let mut readiness = UnixStream::connect(ready)?;
    send(
        &mut readiness,
        &json!({"schema": SCHEMA, "generation": generation, "status": "ready", "pid": std::process::id()}),
    )?;
    while waiting(&listener, Duration::from_secs(idle))? {
        let Some((mut stream, mut reply, poisoned)) = answer(&listener, generation)? else {
            continue;
        };
        if !poisoned && !over_bound(bound) {
            // A disconnected caller cannot publish a completed-output witness. Never replay its compilation here.
            let _ = send(&mut stream, &reply);
            continue;
        }
        // The endpoint is withdrawn before the retiring reply, so no later request dials a worker that is exiting.
        drop(socket_file);
        reply["retired"] = json!(true);
        let _ = send(&mut stream, &reply);
        // Requests that connected before the withdrawal are still answered; after a panic or a failed console
        // restoration they find the worker gone.
        if !poisoned {
            drain(&listener, generation)?;
        }
        return Ok(());
    }
    Ok(())
}

/// Accept one connection and run its request, returning the stream, the reply and whether the request poisoned the
/// process (a panic or a failed console restoration). A connection whose caller already left is dropped unanswered.
fn answer(listener: &UnixListener, generation: &str) -> io::Result<Option<(UnixStream, serde_json::Value, bool)>> {
    let (mut stream, _) = listener.accept()?;
    // A caller killed while it waited in the queue leaves a connection whose options can no longer be set (EINVAL on
    // macOS). Only that connection is dropped; the requests queued behind it are still served.
    if stream.set_read_timeout(Some(Duration::from_secs(5))).is_err()
        || stream.set_write_timeout(Some(Duration::from_secs(10))).is_err()
    {
        return Ok(None);
    }
    let (reply, poisoned) = match receive(&mut stream) {
        Ok(request) => execute(request, generation),
        Err(error) => (json!({"schema": SCHEMA, "exit": 1, "reason": error.to_string()}), false),
    };
    Ok(Some((stream, reply, poisoned)))
}

/// Answer every request already queued when a retiring worker withdrew its endpoint, each reply marked retired.
fn drain(listener: &UnixListener, generation: &str) -> io::Result<()> {
    while waiting(listener, Duration::ZERO)? {
        let Some((mut stream, mut reply, poisoned)) = answer(listener, generation)? else {
            continue;
        };
        reply["retired"] = json!(true);
        let _ = send(&mut stream, &reply);
        if poisoned {
            break;
        }
    }
    Ok(())
}

/// The footprint bound this worker retires above: [`RETIREMENT_FOOTPRINT`], or a lower test-only setting.
fn retirement_bound() -> u64 {
    std::env::var(RETIREMENT_FOOTPRINT_SETTING)
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .map_or(RETIREMENT_FOOTPRINT, |bound| bound.min(RETIREMENT_FOOTPRINT))
}

/// Whether the process's physical footprint after a request is above `bound`. A footprint that cannot be read
/// keeps the worker, as before the bound existed.
fn over_bound(bound: u64) -> bool {
    physical_footprint().is_ok_and(|bytes| bytes > bound)
}

/// The process's current physical footprint in bytes, the measure macOS accounts memory pressure by.
#[cfg(target_os = "macos")]
fn physical_footprint() -> io::Result<u64> {
    let pid = libc::c_int::try_from(std::process::id()).map_err(io::Error::other)?;
    // SAFETY: `rusage_info_v4` holds only integers, so the zeroed value is valid.
    let mut info: libc::rusage_info_v4 = unsafe { std::mem::zeroed() };
    let buffer = (&raw mut info).cast::<libc::rusage_info_t>();
    // SAFETY: `buffer` addresses a writable `rusage_info_v4`, the record the V4 flavor fills, for this call only.
    if unsafe { libc::proc_pid_rusage(pid, libc::RUSAGE_INFO_V4, buffer) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(info.ri_phys_footprint)
}

/// The process's current resident memory in bytes: its resident pages in `/proc/self/statm` times the page size.
#[cfg(target_os = "linux")]
fn physical_footprint() -> io::Result<u64> {
    let statm = fs::read_to_string("/proc/self/statm")?;
    let pages: u64 = statm
        .split_whitespace()
        .nth(1)
        .ok_or_else(|| io::Error::other("/proc/self/statm has no resident field"))?
        .parse()
        .map_err(io::Error::other)?;
    // SAFETY: `sysconf` reads a configuration value and has no preconditions.
    let page = u64::try_from(unsafe { libc::sysconf(libc::_SC_PAGESIZE) }).map_err(io::Error::other)?;
    Ok(pages.saturating_mul(page))
}

/// No footprint reading on other systems, so a worker there retires only on a panic or when idle.
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn physical_footprint() -> io::Result<u64> {
    Err(io::Error::other("no physical footprint reading on this system"))
}

/// Sleep in the kernel until a client arrives or Oven's idle period elapses, without a polling thread per worker.
fn waiting(listener: &UnixListener, idle: Duration) -> io::Result<bool> {
    let timeout = i32::try_from(idle.as_millis()).map_err(io::Error::other)?;
    loop {
        let mut descriptor = libc::pollfd {
            fd: listener.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: the descriptor is initialized and remains valid for the duration of this synchronous call.
        let result = unsafe { libc::poll(&mut descriptor, 1, timeout) };
        if result >= 0 {
            return Ok(result != 0);
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
}

/// Admit a unit compilation, check or formatting request for this worker generation, then scope diagnostics around
/// it. The reported status is the one the short-lived command exits with: its own status, one for a returned error
/// (printed as `main` prints it), and Rust's panic status. The flag is true when a panic or a failed console
/// restoration poisoned the process.
fn execute(request: serde_json::Value, generation: &str) -> (serde_json::Value, bool) {
    let admitted = admit(&request, generation);
    let result = admitted.and_then(|(arguments, stdout, stderr, cwd)| {
        let console = console::Capture::open(&stdout, &stderr, &cwd).map_err(|error| error.to_string())?;
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(&arguments)));
        let (exit, retire) = match outcome {
            Ok(Ok(code)) => (code, false),
            Ok(Err(error)) => {
                eprintln!("Error: {error:?}");
                (1, false)
            }
            Err(_) => (PANIC_STATUS, true),
        };
        if let Err(error) = console.restore() {
            return Ok((
                json!({"schema": SCHEMA, "exit": 1, "reason": format!("compiler console restore failed: {error}")}),
                true,
            ));
        }
        Ok((json!({"schema": SCHEMA, "exit": exit, "reason": ""}), retire))
    });
    match result {
        Ok(result) => result,
        Err(reason) => (json!({"schema": SCHEMA, "exit": 1, "reason": reason}), false),
    }
}

/// Run one admitted request through the entry point its short-lived command uses, returning that command's status.
fn run(arguments: &[String]) -> Result<i32, Box<dyn std::error::Error>> {
    let (command, rest) = arguments.split_first().ok_or("an empty compiler worker request")?;
    match command.as_str() {
        "--unit" => crate::frontend::compile_resident(rest).map(|()| 0),
        "--check" => crate::check(rest),
        "--fmt" => crate::format(rest),
        other => Err(format!("a compiler worker does not serve {other}").into()),
    }
}

/// Decode absolute input/output coordinates without granting another command or a different generation.
fn admit(request: &serde_json::Value, generation: &str) -> Result<(Vec<String>, PathBuf, PathBuf, PathBuf), String> {
    if request["schema"] != SCHEMA || request["generation"] != generation {
        return Err("compiler worker generation or protocol mismatch".into());
    }
    let arguments: Vec<String> =
        serde_json::from_value(request["arguments"].clone()).map_err(|error| error.to_string())?;
    for &index in absolute_arguments(&arguments)? {
        if !Path::new(&arguments[index]).is_absolute() {
            return Err("compiler worker unit, output and sysroot paths must be absolute".into());
        }
    }
    let stdout = output_path(request, "stdout")?;
    let stderr = output_path(request, "stderr")?;
    let cwd = output_path(request, "cwd")?;
    Ok((arguments, stdout, stderr, cwd))
}

/// The positions of the paths a served request must name absolutely, because the worker's directory is not its
/// caller's: `--unit UNIT CRATE OUTPUT SYSROOT ...`, `--check UNIT SYSROOT ...` and `--fmt UNIT PATH ...`. A formatted
/// path resolves against the caller's directory, which each request installs. Every other command is refused.
fn absolute_arguments(arguments: &[String]) -> Result<&'static [usize], String> {
    let (minimum, absolute): (usize, &'static [usize]) = match arguments.first().map(String::as_str) {
        Some("--unit") => (5, &[1, 3, 4]),
        Some("--check") => (3, &[1, 2]),
        Some("--fmt") => (3, &[1]),
        _ => return Err("a compiler worker accepts only --unit, --check and --fmt requests".into()),
    };
    if arguments.len() < minimum {
        return Err(format!("a compiler worker {} request is incomplete", arguments[0]));
    }
    Ok(absolute)
}

/// Require an absolute diagnostic sink supplied by Oven, never a process-global cached diagnostic buffer.
fn output_path(request: &serde_json::Value, field: &str) -> Result<PathBuf, String> {
    let path = PathBuf::from(request[field].as_str().ok_or_else(|| format!("missing `{field}`"))?);
    if !path.is_absolute() {
        return Err(format!("compiler worker `{field}` path must be absolute"));
    }
    Ok(path)
}

/// Decode the existing owner transport's bounded eight-digit ASCII length frame.
fn receive(stream: &mut UnixStream) -> io::Result<serde_json::Value> {
    let mut header = [0_u8; 8];
    stream.read_exact(&mut header)?;
    if !header.iter().all(u8::is_ascii_digit) {
        return Err(io::Error::other("invalid compiler worker frame header"));
    }
    let length: usize = std::str::from_utf8(&header)
        .map_err(io::Error::other)?
        .parse()
        .map_err(io::Error::other)?;
    if length > MAX_FRAME {
        return Err(io::Error::other("compiler worker frame exceeds its bound"));
    }
    let mut bytes = vec![0; length];
    stream.read_exact(&mut bytes)?;
    serde_json::from_slice(&bytes).map_err(io::Error::other)
}

/// Write one bounded response without mixing console diagnostics into the protocol stream.
fn send(stream: &mut UnixStream, reply: &serde_json::Value) -> io::Result<()> {
    let bytes = serde_json::to_vec(reply).map_err(io::Error::other)?;
    if bytes.len() > MAX_FRAME {
        return Err(io::Error::other("compiler worker reply exceeds its bound"));
    }
    stream.write_all(format!("{:08}", bytes.len()).as_bytes())?;
    stream.write_all(&bytes)
}
