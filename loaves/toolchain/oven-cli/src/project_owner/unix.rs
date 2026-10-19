//! Unix service lifecycle for the project owner, kept above the compiler/Store boundary.

use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io;
use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use super::protocol::{self, Reply, Request};
use super::{
    BindingInspectionIntent, BuildIntent, CheckIntent, LockIntent, NativeBuildIntent, SourceFormatIntent,
    TestExecutionIntent, TestListIntent,
};
use incan_driver::error::{CliError, CliResult};
use sha2::{Digest, Sha256};

const INTERNAL_COMMAND: &str = "--internal-project-owner";
const PROTOCOL: &str = "incan.project-owner/4";

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
        Ok(Ok(value)) if compatible_readiness(&value, child.id(), generation) => connect(path),
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

/// Require the spawned host's own protocol version; echoing a caller generation cannot establish compatibility.
fn compatible_readiness(value: &serde_json::Value, child: u32, generation: &str) -> bool {
    value["pid"].as_u64() == Some(u64::from(child))
        && value["generation"] == generation
        && value["protocol"] == PROTOCOL
}

/// Submit one build and render only this request's progress; transport failure is terminal.
pub(super) fn build(intent: BuildIntent, progress: impl FnMut(String)) -> CliResult<serde_json::Value> {
    submit(
        &std::env::current_exe().map_err(transport_error)?,
        serde_json::to_value(intent).map_err(transport_error)?,
        progress,
        None,
    )
}

/// Share the same compatible endpoint and bounded protocol for ordinary and explicit native build intents.
pub(super) fn submit(
    executable: &Path,
    intent: serde_json::Value,
    mut progress: impl FnMut(String),
    test_config: Option<&crate::test_runner::TestRunConfig<'_>>,
) -> CliResult<serde_json::Value> {
    let test_output = test_config.map(|config| {
        crate::test_runner::output::TestOutputSink::rendered(config.report_format, config.verbose, config.use_color)
    });
    // Standalone Oven joins its adjacent Incan host. Source bootstrap and test hosts keep their own executable:
    // they contain their current linked driver and must not silently delegate to an older ambient compiler.
    let compiler = if executable.file_stem().is_some_and(|name| name == "oven") {
        Some(incan_driver::build::native_runtime_inputs::compiler_for_toolchain_command(executable, None)?)
    } else {
        None
    };
    let executable = compiler.as_deref().unwrap_or(executable);
    let (path, generation) = coordinates(executable).map_err(transport_error)?;
    let mut stream = ensure_connection(executable, &path, &generation).map_err(transport_error)?;
    configure_reply_timeout(&stream, &intent).map_err(transport_error)?;
    protocol::write(&mut stream, &Request { generation, intent }).map_err(transport_error)?;
    loop {
        match protocol::read::<Reply>(&mut stream).map_err(transport_error)? {
            Reply::Progress { message } => progress(message),
            Reply::Diagnostic { message } => eprint!("{message}"),
            Reply::TestProgress { event } => {
                let output = test_output
                    .as_ref()
                    .ok_or_else(|| CliError::failure("unexpected test progress for a non-test request"))?;
                output.progress(event);
            }
            Reply::Complete { result, .. } => return Ok(result),
            Reply::Failure { message, exit_code } => return Err(CliError::with_code(message, exit_code)),
        }
    }
}

/// Leave build and test deadlines to command execution instead of limiting how long a valid owner may be quiet.
/// Readiness, lifecycle requests, writes and frame sizes retain their existing bounds; this does not retry a command
/// or authorize another execution route after a transport failure.
fn configure_reply_timeout(stream: &UnixStream, intent: &serde_json::Value) -> io::Result<()> {
    if intent.get("test_execute").is_some() || intent.get("native_build").is_some() || intent.get("file_path").is_some()
    {
        stream.set_read_timeout(None)?;
    }
    Ok(())
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
    if request.intent.get("test_execute").is_some() {
        let output_stream = Arc::new(Mutex::new(stream.try_clone()?));
        let disconnected = Arc::new(AtomicBool::new(false));
        let output_disconnected = Arc::clone(&disconnected);
        let output = crate::test_runner::output::TestOutputSink::new(move |event| {
            if output_disconnected.load(Ordering::Relaxed) {
                return;
            }
            let reply = match event {
                crate::test_runner::output::TestOutput::Stdout(message) => Reply::Progress { message },
                crate::test_runner::output::TestOutput::Stderr(message) => Reply::Diagnostic { message },
                crate::test_runner::output::TestOutput::Progress(event) => Reply::TestProgress { event },
            };
            let written = output_stream
                .lock()
                .ok()
                .is_some_and(|mut stream| protocol::write(&mut *stream, &reply).is_ok());
            if !written {
                output_disconnected.store(true, Ordering::Relaxed);
            }
        });
        let result = serde_json::from_value::<TestExecutionIntent>(request.intent)
            .map_err(transport_error)
            .and_then(|intent| crate::test_runner::execute_tests_with_owner(&intent.test_execute, owner, output));
        if disconnected.load(Ordering::Relaxed) {
            return Ok(false);
        }
        let reply = match result {
            Ok(report) => Reply::Complete {
                pid: std::process::id(),
                result: serde_json::to_value(report).map_err(io::Error::other)?,
            },
            Err(error) => Reply::Failure {
                message: error.message,
                exit_code: error.exit_code.0,
            },
        };
        protocol::write(stream, &reply)?;
        return Ok(false);
    }
    if request.intent.get("test_list").is_some() {
        let mut disconnected = false;
        let (result, warnings) = incan_driver::diagnostics::capture_warning_output(|| {
            serde_json::from_value::<TestListIntent>(request.intent)
                .map_err(transport_error)
                .and_then(|intent| {
                    if !Path::new(&intent.test_list.path).is_absolute() {
                        return Err(CliError::failure("owner requires an absolute test collection path"));
                    }
                    crate::test_runner::list_tests_with_owner(&intent.test_list, owner, |event| {
                        if !disconnected && protocol::write(stream, &Reply::TestProgress { event }).is_err() {
                            disconnected = true;
                        }
                    })
                })
                .and_then(|report| serde_json::to_value(report).map_err(transport_error))
        });
        if disconnected {
            return Ok(false);
        }
        for message in warnings {
            protocol::write(stream, &Reply::Diagnostic { message })?;
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
    if request.intent.get("source_format").is_some() {
        let (result, warnings) = incan_driver::diagnostics::capture_warning_output(|| {
            serde_json::from_value::<SourceFormatIntent>(request.intent)
                .map_err(transport_error)
                .map(|intent| super::source_format::execute_batch(&intent.source_format, owner))
                .and_then(|results| serde_json::to_value(results).map_err(transport_error))
        });
        for message in warnings {
            protocol::write(stream, &Reply::Diagnostic { message })?;
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
    if request.intent.get("binding_inspection").is_some() {
        let (result, warnings) = incan_driver::diagnostics::capture_warning_output(|| {
            serde_json::from_value::<BindingInspectionIntent>(request.intent)
                .map_err(transport_error)
                .and_then(|intent| crate::binding_inspect::execute_request(&intent.binding_inspection, owner))
                .and_then(|rendered| serde_json::to_value(rendered).map_err(transport_error))
        });
        for message in warnings {
            protocol::write(stream, &Reply::Diagnostic { message })?;
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
    if request.intent.get("lock").is_some() {
        let (result, warnings) = incan_driver::diagnostics::capture_warning_output(|| {
            serde_json::from_value::<LockIntent>(request.intent)
                .map_err(transport_error)
                .and_then(|intent| owner.prepare_lock(&intent.lock))
                .and_then(|result| serde_json::to_value(result).map_err(transport_error))
        });
        for message in warnings {
            protocol::write(stream, &Reply::Diagnostic { message })?;
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
    if request.intent.get("check").is_some() {
        let result = serde_json::from_value::<CheckIntent>(request.intent)
            .map_err(transport_error)
            .and_then(|intent| owner.check(&intent.check))
            .and_then(|report| serde_json::to_value(report).map_err(transport_error));
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
            .and_then(|intent| intent.native_build.execute(owner));
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
        &serde_json::json!({"pid":std::process::id(),"generation":generation,"protocol":PROTOCOL}),
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

    /// An older adjacent compiler must not join by echoing the new client's generation and expected process ID.
    #[test]
    fn readiness_requires_host_protocol_and_process_identity() {
        let current = serde_json::json!({"pid":7,"generation":"current","protocol":PROTOCOL});
        assert!(compatible_readiness(&current, 7, "current"));
        assert!(!compatible_readiness(&current, 8, "current"));
        assert!(!compatible_readiness(&current, 7, "changed"));
        let old = serde_json::json!({"pid":7,"generation":"current","protocol":"incan.project-owner/3"});
        assert!(!compatible_readiness(&old, 7, "current"));
        let absent = serde_json::json!({"pid":7,"generation":"current"});
        assert!(!compatible_readiness(&absent, 7, "current"));
    }

    /// Execution owns command deadlines; quiet builds and tests retain bounded writes without a reply deadline.
    #[test]
    fn test_execution_uses_runner_deadlines() -> Result<(), Box<dyn std::error::Error>> {
        // ---- Lifecycle requests keep their bounded reply wait ----
        let (client, _server) = UnixStream::pair()?;
        for operation in ["ping", "stop"] {
            client.set_read_timeout(Some(Duration::from_secs(600)))?;
            configure_reply_timeout(&client, &serde_json::json!({"operation": operation}))?;
            assert_eq!(client.read_timeout()?, Some(Duration::from_secs(600)));
        }

        // ---- Quiet execution replies outlive the former short transport deadline ----
        for intent in [
            serde_json::json!({"test_execute": {}}),
            serde_json::json!({"native_build": {}}),
            serde_json::json!({"file_path": "/project/src/main.incn"}),
        ] {
            let (mut client, mut server) = UnixStream::pair()?;
            client.set_read_timeout(Some(Duration::from_millis(25)))?;
            client.set_write_timeout(Some(Duration::from_secs(10)))?;
            configure_reply_timeout(&client, &intent)?;
            assert_eq!(client.read_timeout()?, None, "{intent}");
            assert_eq!(client.write_timeout()?, Some(Duration::from_secs(10)));
            let reply = std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(75));
                protocol::write(
                    &mut server,
                    &Reply::Complete {
                        pid: std::process::id(),
                        result: serde_json::json!({"completed": true}),
                    },
                )
            });
            let result = protocol::read::<Reply>(&mut client);
            reply.join().map_err(|_| "quiet reply writer panicked")??;
            assert!(matches!(result?, Reply::Complete { result, .. } if result["completed"] == true));
        }
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

    /// Collection warnings must reach the client on first and retained requests, then disappear after correction.
    #[test]
    fn test_collection_forwards_current_manifest_warnings() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let manifest = "[project]\nname = \"list_warnings\"\nversion = \"0.1.0\"\n";
        let manifest_path = directory.path().join("loaf.toml");
        fs::write(
            directory.path().join("test_value.incn"),
            "def test_value() -> None:\n    assert True\n",
        )?;
        let mut owner = incan_driver::build::executable::ExecutableBuildOwner::default();
        let mut previous_ids = None;
        for deprecated in [true, true, false] {
            fs::write(
                &manifest_path,
                format!("{manifest}{}", if deprecated { "[rust-dependencies]\n" } else { "" }),
            )?;
            let (mut server, mut client) = UnixStream::pair()?;
            let request = Request {
                generation: "current".to_string(),
                intent: serde_json::json!({"test_list": {
                    "languages": "Incan", "path": directory.path(), "verbose": false, "include_slow": false,
                    "filter": null, "marker_expr": null, "strict_markers": false,
                    "test_features": [], "package_features": incan_provider::FeatureSelection::default(),
                    "sdk_profile": null, "timeout": null,
                    "cargo_policy": incan_driver::cargo_policy::CargoPolicy::default(),
                }}),
            };
            assert!(!dispatch(&mut server, "current", request, &mut owner)?);
            let mut warnings = Vec::new();
            let ids = loop {
                match protocol::read::<Reply>(&mut client)? {
                    Reply::Diagnostic { message } => warnings.push(message),
                    Reply::Progress { .. } | Reply::TestProgress { .. } => {}
                    Reply::Complete { result, .. } => break result["ids"].clone(),
                    Reply::Failure { message, .. } => return Err(message.into()),
                }
            };
            assert_eq!(warnings.len(), usize::from(deprecated));
            if deprecated {
                assert!(warnings[0].contains("[rust-dependencies] is deprecated"));
            }
            assert_eq!(ids.as_array().ok_or("missing listed IDs")?.len(), 1);
            if let Some(previous) = &previous_ids {
                assert_eq!(&ids, previous);
            }
            previous_ids = Some(ids);
        }
        Ok(())
    }

    /// Formatting returns ordered successes and failures directly, without embedding JSON in progress text.
    #[test]
    fn formatting_batch_completes_with_ordered_results() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let root = directory.path().canonicalize()?;
        fs::write(
            root.join("loaf.toml"),
            "[project]\nname = \"format_reply\"\nversion = \"0.1.0\"\n",
        )?;
        let source = "def main() -> None:\n    println(42)\n";
        let mut inputs = Vec::new();
        for (name, text) in [("one.incn", source), ("bad.incn", "def main(\n"), ("two.incn", source)] {
            let path = root.join(name);
            fs::write(&path, text)?;
            inputs.push(serde_json::json!({"path": path, "display_path": path, "source": text}));
        }
        let request = Request {
            generation: "current".to_string(),
            intent: serde_json::json!({"source_format": inputs}),
        };
        let (mut server, mut client) = UnixStream::pair()?;
        let mut owner = incan_driver::build::executable::ExecutableBuildOwner::default();
        assert!(!dispatch(&mut server, "current", request, &mut owner)?);
        let Reply::Complete { result, .. } = protocol::read::<Reply>(&mut client)? else {
            return Err("format results did not arrive in one completion".into());
        };
        let results: Vec<Result<String, String>> = serde_json::from_value(result)?;
        assert_eq!(results.len(), 3);
        assert_eq!(results[0].as_deref(), Ok(source));
        assert!(results[1].is_err());
        assert_eq!(results[2].as_deref(), Ok(source));
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
        let invalid_lock = Request {
            generation: "current".to_string(),
            intent: serde_json::json!({"lock": {"unrecognized": true}}),
        };
        assert!(!dispatch(&mut server, "current", invalid_lock, &mut owner)?);
        assert!(matches!(protocol::read::<Reply>(&mut client)?, Reply::Failure { .. }));
        let relative_lock = Request {
            generation: "current".to_string(),
            intent: serde_json::json!({"lock": {
                "directory": ".", "entry": null,
                "features": incan_provider::FeatureSelection::default(),
                "sdk_profile": null, "cargo_features": oven_model::lock::CargoFeatureSelection::default(),
            }}),
        };
        assert!(!dispatch(&mut server, "current", relative_lock, &mut owner)?);
        assert!(
            matches!(protocol::read::<Reply>(&mut client)?, Reply::Failure { message, .. }
            if message.contains("absolute lock request paths"))
        );
        let invalid_collection = Request {
            generation: "current".to_string(),
            intent: serde_json::json!({"test_list": {"unrecognized": true}}),
        };
        assert!(!dispatch(&mut server, "current", invalid_collection, &mut owner)?);
        assert!(matches!(protocol::read::<Reply>(&mut client)?, Reply::Failure { .. }));
        let relative_collection = Request {
            generation: "current".to_string(),
            intent: serde_json::json!({"test_list": {
                "languages": "Incan", "path": "tests", "verbose": false, "include_slow": false,
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
        let invalid_execution = Request {
            generation: "current".to_string(),
            intent: serde_json::json!({"test_execute": {"unrecognized": true}}),
        };
        assert!(!dispatch(&mut server, "current", invalid_execution, &mut owner)?);
        assert!(matches!(protocol::read::<Reply>(&mut client)?, Reply::Failure { .. }));
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
