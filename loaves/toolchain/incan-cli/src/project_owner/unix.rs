//! Unix service lifecycle for the project owner, kept above the compiler/Store boundary.

use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io;
use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use super::protocol::{self, Reply, Request};
use super::{BuildIntent, NativeBuildIntent, TestListIntent};
use incan_driver::error::{CliError, CliResult};
use sha2::{Digest, Sha256};

const INTERNAL_COMMAND: &str = "--internal-project-owner";
const PROTOCOL: &str = "incan.project-owner/2";

/// Convert a transport failure into the existing command failure contract.
fn transport_error(error: impl std::fmt::Display) -> CliError {
    CliError::failure(format!("project owner: {error}"))
}

/// Locate a compatible owner without hashing the compiler image on each CLI launch.
///
/// The coordinate includes replacement-sensitive executable metadata, working directory and inherited configuration.
/// It is only transport compatibility, not artifact authority: the driver's existing compiler/source checks still
/// decide reuse. Environment values are hashed in memory and never written to endpoint names or logs.
fn coordinates(executable: &Path) -> io::Result<(PathBuf, String)> {
    if !executable.is_absolute() {
        return Err(io::Error::other("owner executable must be absolute"));
    }
    let metadata = fs::metadata(executable)?;
    let mut digest = Sha256::new();
    digest.update(PROTOCOL.as_bytes());
    let working_directory = std::env::current_dir()?;
    for value in [executable.as_os_str(), working_directory.as_os_str()] {
        let bytes = value.as_encoded_bytes();
        digest.update(bytes.len().to_le_bytes());
        digest.update(bytes);
    }
    for value in [metadata.dev(), metadata.ino(), metadata.len()] {
        digest.update(value.to_le_bytes());
    }
    for value in [
        metadata.mtime(),
        metadata.mtime_nsec(),
        metadata.ctime(),
        metadata.ctime_nsec(),
    ] {
        digest.update(value.to_le_bytes());
    }
    let variables: BTreeMap<_, _> = std::env::vars_os()
        .filter(|(key, _)| key != "_" && key != "SHLVL")
        .collect();
    for (key, value) in variables {
        for part in [key, value] {
            let bytes = part.as_encoded_bytes();
            digest.update(bytes.len().to_le_bytes());
            digest.update(bytes);
        }
    }
    let generation = format!("{PROTOCOL}:{:x}", digest.finalize());
    let root = match std::env::var_os("INCAN_OWNER_DIRECTORY") {
        Some(root) => PathBuf::from(root),
        None => PathBuf::from(std::env::var_os("HOME").ok_or_else(|| io::Error::other("HOME is unavailable"))?)
            .join(".incan/owners"),
    };
    fs::create_dir_all(&root)?;
    let metadata = fs::symlink_metadata(&root)?;
    if !metadata.is_dir()
        || metadata.file_type().is_symlink()
        || metadata.uid()
            != fs::metadata(std::env::var_os("HOME").ok_or_else(|| io::Error::other("HOME is unavailable"))?)?.uid()
    {
        return Err(io::Error::other(
            "owner directory must be a private, owned regular directory",
        ));
    }
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700))?;
    let key = generation
        .rsplit(':')
        .next()
        .ok_or_else(|| io::Error::other("missing owner generation"))?;
    Ok((root.join(format!("{}.sock", &key[..24])), generation))
}

/// Connect without treating a slow or inaccessible live service as a stale socket.
fn connect(path: &Path) -> io::Result<UnixStream> {
    let stream = UnixStream::connect(path)?;
    stream.set_read_timeout(Some(Duration::from_secs(600)))?;
    stream.set_write_timeout(Some(Duration::from_secs(10)))?;
    Ok(stream)
}

/// Distinguish absent/refused endpoints from failures that must not authorize replacement.
fn missing_endpoint(error: &io::Error) -> bool {
    matches!(error.kind(), io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused)
}

/// Read the bounded acknowledgment; the spawning thread owns the deadline and kills the sole writer on timeout.
fn read_readiness(listener: &UnixListener) -> io::Result<serde_json::Value> {
    let (mut stream, _) = listener.accept()?;
    // macOS rejects timeout setup after peer close, even with a complete acknowledgment still buffered. On a failed
    // startup the parent kills the child, closing this accepted stream, or wakes accept if no child connected yet.
    protocol::read(&mut stream)
}

/// Serialize startup, then await an explicit child readiness connection rather than sleeping for an assumed delay.
fn ensure_connection(executable: &Path, path: &Path, generation: &str) -> io::Result<UnixStream> {
    match connect(path) {
        Ok(stream) => return Ok(stream),
        Err(error) if missing_endpoint(&error) => {}
        Err(error) => return Err(error),
    }
    let lock_path = path.with_extension("start.lock");
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(lock_path)?;
    lock.lock()?;
    match connect(path) {
        Ok(stream) => return Ok(stream),
        Err(error) if missing_endpoint(&error) => {}
        Err(error) => return Err(error),
    }
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_socket() => fs::remove_file(path)?,
        Ok(_) => return Err(io::Error::other("endpoint is not a socket; refusing replacement")),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }

    // ---- Readiness: a separate socket keeps driver stdout and stderr out of the protocol ----
    let ready_dir = tempfile::Builder::new().prefix("ready-").tempdir_in(
        path.parent()
            .ok_or_else(|| io::Error::other("owner endpoint has no parent"))?,
    )?;
    let ready_path = ready_dir.path().join("r");
    let listener = UnixListener::bind(&ready_path)?;
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path.with_extension("log"))?;
    let mut child = Command::new(executable)
        .arg(INTERNAL_COMMAND)
        .arg(path)
        .arg(generation)
        .arg(&ready_path)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log.try_clone()?))
        .stderr(Stdio::from(log))
        .spawn()?;
    let (sender, receiver) = mpsc::channel();
    let readiness = std::thread::spawn(move || {
        let result = read_readiness(&listener);
        let _delivered = sender.send(result);
    });
    let result = match receiver.recv_timeout(Duration::from_secs(15)) {
        Ok(Ok(value)) if value["pid"].as_u64() == Some(u64::from(child.id())) && value["generation"] == generation => {
            connect(path)
        }
        Ok(Ok(_)) => Err(io::Error::other("incompatible owner readiness acknowledgment")),
        Ok(Err(error)) => Err(error),
        Err(error) => Err(io::Error::other(format!("owner readiness failed: {error}"))),
    };
    if result.is_err() {
        let _killed = child.kill();
        let _reaped = child.wait();
        // Wake an abandoned accept so failed startup does not retain a helper thread.
        let _wake = UnixStream::connect(&ready_path);
    }
    let _joined = readiness.join();
    drop(lock);
    result
}

/// Submit one build and render only this request's progress; transport failure is terminal.
pub(super) fn build(intent: BuildIntent, progress: impl FnMut(String)) -> CliResult<serde_json::Value> {
    submit(
        &std::env::current_exe().map_err(transport_error)?,
        serde_json::to_value(intent).map_err(transport_error)?,
        progress,
    )
}

/// Share the same compatible endpoint and bounded protocol for ordinary and explicit native build intents.
pub(super) fn submit(
    executable: &Path,
    intent: serde_json::Value,
    mut progress: impl FnMut(String),
) -> CliResult<serde_json::Value> {
    let (path, generation) = coordinates(executable).map_err(transport_error)?;
    let mut stream = ensure_connection(executable, &path, &generation).map_err(transport_error)?;
    protocol::write(&mut stream, &Request { generation, intent }).map_err(transport_error)?;
    loop {
        match protocol::read::<Reply>(&mut stream).map_err(transport_error)? {
            Reply::Progress { message } => progress(message),
            Reply::Complete { result, .. } => return Ok(result),
            Reply::Failure { message, exit_code } => return Err(CliError::with_code(message, exit_code)),
        }
    }
}

/// Execute a request only after generation validation, streaming driver progress on its own connection.
fn dispatch(
    stream: &mut UnixStream,
    generation: &str,
    request: Request,
    owner: &mut incan_driver::build::executable::ExecutableBuildOwner,
) -> io::Result<bool> {
    if request.generation != generation {
        protocol::write(
            stream,
            &Reply::Failure {
                message: "project owner generation mismatch".to_string(),
                exit_code: 1,
            },
        )?;
        return Ok(false);
    }
    let stop = request.intent.get("operation").and_then(serde_json::Value::as_str) == Some("stop");
    if stop || request.intent.get("operation").and_then(serde_json::Value::as_str) == Some("ping") {
        protocol::write(
            stream,
            &Reply::Complete {
                pid: std::process::id(),
                result: serde_json::json!({"generation":generation}),
            },
        )?;
        return Ok(stop);
    }
    if request.intent.get("test_list").is_some() {
        let mut disconnected = false;
        let result = serde_json::from_value::<TestListIntent>(request.intent)
            .map_err(transport_error)
            .and_then(|intent| {
                if !Path::new(&intent.test_list.path).is_absolute() {
                    return Err(CliError::failure("owner requires an absolute test collection path"));
                }
                crate::test_runner::list_tests_with_owner(&intent.test_list, owner, |message| {
                    if !disconnected && protocol::write(stream, &Reply::Progress { message }).is_err() {
                        disconnected = true;
                    }
                })
            })
            .and_then(|report| serde_json::to_value(report).map_err(transport_error));
        if disconnected {
            return Ok(false);
        }
        let reply = match result {
            Ok(result) => Reply::Complete {
                pid: std::process::id(),
                result,
            },
            Err(error) => Reply::Failure {
                message: error.message,
                exit_code: error.exit_code.0,
            },
        };
        protocol::write(stream, &reply)?;
        return Ok(false);
    }
    if request.intent.get("native_build").is_some() {
        let result = serde_json::from_value::<NativeBuildIntent>(request.intent)
            .map_err(transport_error)
            .and_then(|intent| owner.build_native(&intent.native_build));
        let reply = match result {
            Ok(result) => Reply::Complete {
                pid: std::process::id(),
                result,
            },
            Err(error) => Reply::Failure {
                message: error.message,
                exit_code: error.exit_code.0,
            },
        };
        protocol::write(stream, &reply)?;
        return Ok(false);
    }
    let intent: BuildIntent = match serde_json::from_value(request.intent) {
        Ok(intent) => intent,
        Err(error) => {
            protocol::write(
                stream,
                &Reply::Failure {
                    message: format!("invalid build intent: {error}"),
                    exit_code: 1,
                },
            )?;
            return Ok(false);
        }
    };
    if !Path::new(&intent.file_path).is_absolute()
        || intent
            .output_dir
            .as_ref()
            .is_some_and(|path| !Path::new(path).is_absolute())
    {
        protocol::write(
            stream,
            &Reply::Failure {
                message: "owner requires absolute build paths".to_string(),
                exit_code: 1,
            },
        )?;
        return Ok(false);
    }
    let _invocation =
        tracing::trace_span!(target: "incan::invocation", "incan_owner::build", input = %intent.file_path).entered();
    let mut disconnected = false;
    let result = owner.build(
        &intent.file_path,
        intent.output_dir.as_ref(),
        intent.options,
        |message| {
            if protocol::write(stream, &Reply::Progress { message }).is_err() {
                disconnected = true;
            }
        },
    );
    // A disconnected client cannot observe a partial result. The existing driver transaction still owns publication.
    if disconnected {
        return Ok(false);
    }
    let reply = match result {
        Ok(result) => Reply::Complete {
            pid: std::process::id(),
            result,
        },
        Err(error) => Reply::Failure {
            message: error.message,
            exit_code: error.exit_code.0,
        },
    };
    protocol::write(stream, &reply)?;
    Ok(false)
}

/// Host one serial owner. Failed or incomplete clients lose only their request, not subsequent service readiness.
fn serve(path: &Path, generation: &str, ready_path: &Path) -> io::Result<()> {
    let listener = UnixListener::bind(path)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    let mut ready = UnixStream::connect(ready_path)?;
    protocol::write(
        &mut ready,
        &serde_json::json!({"pid":std::process::id(),"generation":generation}),
    )?;
    drop(ready);
    let mut owner = incan_driver::build::executable::ExecutableBuildOwner::default();
    for incoming in listener.incoming() {
        let mut stream = incoming?;
        let result = (|| {
            stream.set_read_timeout(Some(Duration::from_secs(5)))?;
            stream.set_write_timeout(Some(Duration::from_secs(10)))?;
            let request = protocol::read::<Request>(&mut stream)?;
            dispatch(&mut stream, generation, request, &mut owner)
        })();
        match result {
            Ok(true) => break,
            Ok(false) => {}
            Err(error) => {
                let _reply = protocol::write(
                    &mut stream,
                    &Reply::Failure {
                        message: error.to_string(),
                        exit_code: 1,
                    },
                );
            }
        }
    }
    fs::remove_file(path)?;
    Ok(())
}

/// Parse the private host mode without exposing service management as an ordinary user command.
pub(super) fn run_internal() -> CliResult<bool> {
    let mut arguments = std::env::args_os().skip(1);
    if arguments.next().as_deref() != Some(std::ffi::OsStr::new(INTERNAL_COMMAND)) {
        return Ok(false);
    }
    let path = arguments.next().ok_or_else(|| transport_error("missing endpoint"))?;
    let generation = arguments
        .next()
        .and_then(|value| value.into_string().ok())
        .ok_or_else(|| transport_error("missing generation"))?;
    let ready = arguments
        .next()
        .ok_or_else(|| transport_error("missing readiness endpoint"))?;
    if arguments.next().is_some() {
        return Err(transport_error("unexpected service arguments"));
    }
    serve(Path::new(&path), &generation, Path::new(&ready)).map_err(transport_error)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A child may finish its acknowledgment and close before the readiness reader is scheduled.
    #[test]
    fn closed_readiness_peer_keeps_its_buffered_acknowledgment() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let path = root.path().join("ready");
        let listener = UnixListener::bind(&path)?;
        let acknowledgment = serde_json::json!({"pid": 7, "generation": "current"});
        let mut child = UnixStream::connect(&path)?;
        protocol::write(&mut child, &acknowledgment)?;
        drop(child);
        assert_eq!(read_readiness(&listener)?, acknowledgment);
        Ok(())
    }

    /// A stale owner's generation must refuse before an invalid intent could enter the compiler.
    #[test]
    fn incompatible_generation_refuses_before_dispatch() -> Result<(), Box<dyn std::error::Error>> {
        let (mut server, mut client) = UnixStream::pair()?;
        let request = Request {
            generation: "old".to_string(),
            intent: serde_json::Value::Null,
        };
        assert!(!dispatch(
            &mut server,
            "current",
            request,
            &mut incan_driver::build::executable::ExecutableBuildOwner::default()
        )?);
        assert!(
            matches!(protocol::read::<Reply>(&mut client)?, Reply::Failure { message, .. } if message.contains("generation mismatch"))
        );
        Ok(())
    }

    /// Client working directories cannot leak through relative build paths into the retained process.
    #[test]
    fn relative_intent_refuses_before_compilation() -> Result<(), Box<dyn std::error::Error>> {
        let (mut server, mut client) = UnixStream::pair()?;
        let intent = BuildIntent {
            file_path: "src/main.incn".to_string(),
            output_dir: None,
            options: Default::default(),
        };
        let request = Request {
            generation: "current".to_string(),
            intent: serde_json::to_value(intent)?,
        };
        assert!(!dispatch(
            &mut server,
            "current",
            request,
            &mut incan_driver::build::executable::ExecutableBuildOwner::default()
        )?);
        assert!(
            matches!(protocol::read::<Reply>(&mut client)?, Reply::Failure { message, .. } if message.contains("absolute build paths"))
        );
        Ok(())
    }

    /// A failed request must leave the same owner able to answer its next compatible request.
    #[test]
    fn invalid_request_does_not_poison_owner() -> Result<(), Box<dyn std::error::Error>> {
        let (mut server, mut client) = UnixStream::pair()?;
        let mut owner = incan_driver::build::executable::ExecutableBuildOwner::default();
        let invalid = Request {
            generation: "current".to_string(),
            intent: serde_json::Value::Null,
        };
        assert!(!dispatch(&mut server, "current", invalid, &mut owner)?);
        assert!(matches!(protocol::read::<Reply>(&mut client)?, Reply::Failure { .. }));
        let invalid_native = Request {
            generation: "current".to_string(),
            intent: serde_json::json!({"native_build": {"unrecognized": true}}),
        };
        assert!(!dispatch(&mut server, "current", invalid_native, &mut owner)?);
        assert!(matches!(protocol::read::<Reply>(&mut client)?, Reply::Failure { .. }));
        let invalid_collection = Request {
            generation: "current".to_string(),
            intent: serde_json::json!({"test_list": {"unrecognized": true}}),
        };
        assert!(!dispatch(&mut server, "current", invalid_collection, &mut owner)?);
        assert!(matches!(protocol::read::<Reply>(&mut client)?, Reply::Failure { .. }));
        let relative_collection = Request {
            generation: "current".to_string(),
            intent: serde_json::json!({"test_list": {
                "path": "tests", "verbose": false, "include_slow": false,
                "filter": null, "marker_expr": null, "strict_markers": false,
                "test_features": [], "package_features": incan_provider::FeatureSelection::default(),
                "sdk_profile": null, "timeout": null,
                "cargo_policy": incan_driver::cargo_policy::CargoPolicy::default(),
            }}),
        };
        assert!(!dispatch(&mut server, "current", relative_collection, &mut owner)?);
        assert!(
            matches!(protocol::read::<Reply>(&mut client)?, Reply::Failure { message, .. }
            if message.contains("absolute test collection path"))
        );
        let ping = Request {
            generation: "current".to_string(),
            intent: serde_json::json!({"operation":"ping"}),
        };
        assert!(!dispatch(&mut server, "current", ping, &mut owner)?);
        assert!(
            matches!(protocol::read::<Reply>(&mut client)?, Reply::Complete { pid, .. } if pid == std::process::id())
        );
        Ok(())
    }
}
