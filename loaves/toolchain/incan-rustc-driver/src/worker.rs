//! A serial native compiler worker. Oven owns its generation, startup and retirement policy; this boundary retains
//! immutable artifact readings only. Each request runs the ordinary compiler with a fresh session and rustc context.
//! A panic retires the process instead of exposing possibly poisoned native state to another request.

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

/// Bind one private endpoint, acknowledge readiness, and compile serial requests until idle or poisoned.
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
    let listener = UnixListener::bind(socket)?;
    let metadata = fs::symlink_metadata(socket)?;
    let _socket = SocketFile {
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
        let (mut stream, _) = listener.accept()?;
        stream.set_read_timeout(Some(Duration::from_secs(5)))?;
        stream.set_write_timeout(Some(Duration::from_secs(10)))?;
        let (reply, retire) = match receive(&mut stream) {
            Ok(request) => execute(request, generation),
            Err(error) => (json!({"schema": SCHEMA, "exit": 1, "reason": error.to_string()}), false),
        };
        // A disconnected caller cannot publish a completed-output witness. Never replay its compilation here.
        let _ = send(&mut stream, &reply);
        if retire {
            return Ok(());
        }
    }
    Ok(())
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

/// Admit only ordinary unit compilation for this worker generation, then scope diagnostics around the request.
fn execute(request: serde_json::Value, generation: &str) -> (serde_json::Value, bool) {
    let admitted = admit(&request, generation);
    let result = admitted.and_then(|(arguments, stdout, stderr, cwd)| {
        let _console = console::Capture::open(&stdout, &stderr, &cwd).map_err(|error| error.to_string())?;
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            crate::frontend::compile_resident(&arguments)
        }));
        match outcome {
            Ok(Ok(())) => Ok((0, false)),
            Ok(Err(error)) => {
                eprintln!("Error: {error:?}");
                Ok((1, false))
            }
            Err(_) => Ok((1, true)),
        }
    });
    match result {
        Ok((exit, retire)) => (json!({"schema": SCHEMA, "exit": exit, "reason": ""}), retire),
        Err(reason) => (json!({"schema": SCHEMA, "exit": 1, "reason": reason}), false),
    }
}

/// Decode absolute input/output coordinates without granting another command or a different generation.
fn admit(request: &serde_json::Value, generation: &str) -> Result<(Vec<String>, PathBuf, PathBuf, PathBuf), String> {
    if request["schema"] != SCHEMA || request["generation"] != generation {
        return Err("compiler worker generation or protocol mismatch".into());
    }
    let arguments: Vec<String> =
        serde_json::from_value(request["arguments"].clone()).map_err(|error| error.to_string())?;
    if arguments.len() < 5 || arguments.first().map(String::as_str) != Some("--unit") {
        return Err("a compiler worker accepts only --unit requests".into());
    }
    for index in [1, 3, 4] {
        if !Path::new(&arguments[index]).is_absolute() {
            return Err("compiler worker unit, output and sysroot paths must be absolute".into());
        }
    }
    let stdout = output_path(request, "stdout")?;
    let stderr = output_path(request, "stderr")?;
    let cwd = output_path(request, "cwd")?;
    Ok((arguments.into_iter().skip(1).collect(), stdout, stderr, cwd))
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
