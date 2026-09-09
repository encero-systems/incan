//! Private #991 host transport for an explicitly permitted, originally admitted Engine.
//!
//! Selection remains authored in Incan. This module neither issues permission nor interprets response policy. Its
//! filesystem scope and process group are not an OS sandbox: a same-user adversary or a child that creates a new
//! session is outside this bootstrap containment contract. Ordinary issuer/installation source is connected by the
//! parent module; compiled and native acceptance remain pending.

#![allow(
    dead_code,
    reason = "Pending #991: the complete hosted-module lifecycle and receipt replay are not connected"
)]

use std::fs;
#[cfg(unix)]
use std::fs::{File, OpenOptions};
#[cfg(unix)]
use std::io::Write;
use std::io::{self, Read};
#[cfg(unix)]
use std::path::Path;
use std::path::PathBuf;
use std::process::{Child, ExitStatus};
#[cfg(unix)]
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use super::EngineBootstrapPermit;
use super::engine::{AdmittedEngineArtifact, EngineModuleContract};
use crate::generated_source::digest_bytes;
#[cfg(unix)]
use crate::oven::process::isolate_process_group;
use crate::oven::process::terminate_process_group;

const FILE_LIMIT: usize = 1024 * 1024;
const DIAGNOSTIC_LIMIT: usize = 64 * 1024;
static SCOPE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Actual kernel result, without asserting source-language operation authority or receipt completion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ExchangeOutcome {
    Completed,
    Refused,
    Failed,
    Cancelled,
    TimedOut,
}

/// Host phases are caller-owned and distinct from the child adapter's read/write operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ExchangePhase {
    Admission,
    FileScope,
    RequestFile,
    Spawn,
    Supervision,
    ProcessCleanup,
    ResponseRead,
    FileCleanup,
}

/// One phase is recorded before execution and completed or failed only from its observed result.
#[derive(Debug)]
pub(crate) struct PhaseObservation {
    pub(crate) phase: ExchangePhase,
    pub(crate) outcome: Option<ExchangeOutcome>,
    pub(crate) detail: Option<String>,
}

/// Bounded diagnostic bytes; an overflow digest covers the retained prefix only, never the unseen full stream.
#[derive(Debug, Default)]
pub(crate) struct DiagnosticCapture {
    pub(crate) bytes: Vec<u8>,
    pub(crate) exceeded: bool,
    pub(crate) eof: bool,
}

/// Internal, non-serialized kernel observations, separate from `OperationReceipt` and the caller-issued permit.
///
/// Versioned kernel receipt encoding, identity sealing and persistence remain unwired; this report does not complete
/// that receipt contract.
#[derive(Debug)]
pub(crate) struct EngineExchangeReport {
    pub(crate) permit_id: String,
    pub(crate) invocation_id: String,
    pub(crate) command_receipt_identity: String,
    pub(crate) engine_identity: String,
    pub(crate) output_identity: String,
    pub(crate) source_identity: String,
    pub(crate) compiler_binary_digest: String,
    pub(crate) native_digest: String,
    pub(crate) contract: EngineModuleContract,
    pub(crate) operation: String,
    pub(crate) request_schema: String,
    pub(crate) native_file_exchange_abi: u32,
    pub(crate) host_target: String,
    pub(crate) request_limit: usize,
    pub(crate) response_limit: usize,
    pub(crate) stdout_limit: usize,
    pub(crate) stderr_limit: usize,
    pub(crate) deadline_remaining_at_start: Duration,
    pub(crate) permitted_request_digest: String,
    pub(crate) request_digest: Option<String>,
    pub(crate) response_digest: Option<String>,
    pub(crate) response: Option<Vec<u8>>,
    pub(crate) stdout: DiagnosticCapture,
    pub(crate) stderr: DiagnosticCapture,
    pub(crate) stdout_prefix_digest: String,
    pub(crate) stderr_prefix_digest: String,
    pub(crate) phases: Vec<PhaseObservation>,
    pub(crate) outcome: ExchangeOutcome,
    pub(crate) detail: Option<String>,
    pub(crate) child_id: Option<u32>,
    pub(crate) child_status: Option<ExitStatus>,
    pub(crate) scratch_path: Option<PathBuf>,
    pub(crate) elapsed: Duration,
    pub(crate) deadline_exceeded: bool,
}

/// Internal failure category carried through cleanup without turning a partial exchange into success.
struct Failure {
    outcome: ExchangeOutcome,
    message: String,
}

impl Failure {
    /// Preserve the phase's physical error without synthesizing a source operation diagnostic.
    fn io(error: impl std::fmt::Display) -> Self {
        Self {
            outcome: ExchangeOutcome::Failed,
            message: error.to_string(),
        }
    }

    /// Refuse unsupported or mismatching admission before any process is started.
    fn refuse(message: impl Into<String>) -> Self {
        Self {
            outcome: ExchangeOutcome::Refused,
            message: message.into(),
        }
    }
}

/// Execute exactly one caller-issued exchange; the permit is consumed and is never inferred from child data.
///
/// The absolute deadline covers admission, execution and cleanup. File-size limits are acceptance bounds monitored
/// while the child runs, not OS disk quotas. A cleanup overrun is reported and cannot produce an accepted response.
pub(crate) fn exchange(
    admitted: &AdmittedEngineArtifact<'_>,
    permit: EngineBootstrapPermit<'_>,
    request: &[u8],
    cancelled: &AtomicBool,
) -> EngineExchangeReport {
    let started = Instant::now();
    let (engine, output) = admitted.owner_identities();
    let mut report = EngineExchangeReport {
        permit_id: permit.permit_id.clone(),
        invocation_id: permit.invocation_id.clone(),
        command_receipt_identity: permit.command_receipt.identity.clone(),
        engine_identity: engine.to_string(),
        output_identity: output.to_string(),
        source_identity: admitted.descriptor().source_authority_digest().to_string(),
        compiler_binary_digest: admitted.descriptor().compiler_binary_identity().digest().to_string(),
        native_digest: admitted.native_digest().to_string(),
        contract: admitted.descriptor().module_contract(),
        operation: permit.operation.invocation_name().to_string(),
        request_schema: permit.request_schema.clone(),
        native_file_exchange_abi: admitted.descriptor().native_file_exchange_abi(),
        host_target: permit.host_target.clone(),
        request_limit: permit.request_limit,
        response_limit: permit.response_limit,
        stdout_limit: permit.stdout_limit,
        stderr_limit: permit.stderr_limit,
        deadline_remaining_at_start: permit.deadline.saturating_duration_since(started),
        permitted_request_digest: permit.request_digest.clone(),
        request_digest: (request.len() <= FILE_LIMIT).then(|| digest_bytes(request)),
        response_digest: None,
        response: None,
        stdout: DiagnosticCapture::default(),
        stderr: DiagnosticCapture::default(),
        stdout_prefix_digest: String::new(),
        stderr_prefix_digest: String::new(),
        phases: Vec::new(),
        outcome: ExchangeOutcome::Failed,
        detail: None,
        child_id: None,
        child_status: None,
        scratch_path: None,
        elapsed: Duration::ZERO,
        deadline_exceeded: false,
    };
    let result = execute(admitted, &permit, request, cancelled, &mut report);
    report.deadline_exceeded = Instant::now() >= permit.deadline;
    let result = result.and_then(|response| {
        check_interrupt(&permit, cancelled)?;
        Ok(response)
    });
    match result {
        Ok(response) => {
            report.outcome = ExchangeOutcome::Completed;
            report.response_digest = Some(digest_bytes(&response));
            report.response = Some(response);
        }
        Err(error) => {
            report.outcome = error.outcome;
            report.detail = Some(error.message);
        }
    }
    report.stdout_prefix_digest = digest_bytes(&report.stdout.bytes);
    report.stderr_prefix_digest = digest_bytes(&report.stderr.bytes);
    report.elapsed = started.elapsed();
    report
}

/// Run one physical phase and retain both the attempt and its actual outcome.
fn phase<T>(
    report: &mut EngineExchangeReport,
    phase: ExchangePhase,
    operation: impl FnOnce(&mut EngineExchangeReport) -> Result<T, Failure>,
) -> Result<T, Failure> {
    let index = report.phases.len();
    report.phases.push(PhaseObservation {
        phase,
        outcome: None,
        detail: None,
    });
    let result = operation(report);
    report.phases[index].outcome = Some(
        result
            .as_ref()
            .map_or_else(|error| error.outcome, |_| ExchangeOutcome::Completed),
    );
    report.phases[index].detail = result.as_ref().err().map(|error| error.message.clone());
    result
}

/// Validate receipt provenance and explicit permit coordinates before creating the private file scope.
fn admit(
    admitted: &AdmittedEngineArtifact<'_>,
    permit: &EngineBootstrapPermit<'_>,
    request: &[u8],
    cancelled: &AtomicBool,
) -> Result<(), Failure> {
    check_interrupt(permit, cancelled)?;
    permit.command_receipt.verify_identity().map_err(Failure::io)?;
    let request_document = serde_json::from_slice::<serde_json::Value>(request).map_err(Failure::io)?;
    let request_schema = request_document
        .get("schema")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| Failure::refuse("Engine request has no exact schema"))?;
    let (engine, output) = admitted.owner_identities();
    if permit.permit_id.trim().is_empty()
        || permit.invocation_id.trim().is_empty()
        || permit.engine_identity != engine
        || permit.output_identity != output
        || permit.contract != admitted.descriptor().module_contract()
        || permit.request_schema != permit.operation.request_schema()
        || request_schema != permit.request_schema
        || !admitted.descriptor().supports_request_schema(request_schema)
        || permit.host_target != admitted.descriptor().receipt().intent.target
        || admitted.descriptor().native_file_exchange_abi() != 1
        || [
            (permit.request_limit, FILE_LIMIT),
            (permit.response_limit, FILE_LIMIT),
            (permit.stdout_limit, DIAGNOSTIC_LIMIT),
            (permit.stderr_limit, DIAGNOSTIC_LIMIT),
        ]
        .into_iter()
        .any(|(limit, ceiling)| limit == 0 || limit > ceiling)
        || request.len() > permit.request_limit
        || permit.request_digest != digest_bytes(request)
    {
        return Err(Failure::refuse(
            "Engine permit owner, contract, request or budget mismatch",
        ));
    }
    std::str::from_utf8(request).map_err(Failure::io)?;
    let metadata = fs::symlink_metadata(&permit.scratch_parent).map_err(Failure::io)?;
    if !metadata.is_dir() || permit.scratch_parent.canonicalize().map_err(Failure::io)? != permit.scratch_parent {
        return Err(Failure::refuse(
            "Engine scratch parent must be an existing canonical directory",
        ));
    }
    check_interrupt(permit, cancelled)
}

/// Return interruption before beginning any subsequent host phase.
fn check_interrupt(permit: &EngineBootstrapPermit<'_>, cancelled: &AtomicBool) -> Result<(), Failure> {
    if cancelled.load(Ordering::Acquire) {
        return Err(Failure {
            outcome: ExchangeOutcome::Cancelled,
            message: "Engine exchange cancelled by caller".into(),
        });
    }
    if Instant::now() >= permit.deadline {
        return Err(Failure {
            outcome: ExchangeOutcome::TimedOut,
            message: "Engine exchange absolute deadline expired".into(),
        });
    }
    Ok(())
}

/// Refuse where the existing native-mode and process-group contract is unavailable.
#[cfg(not(unix))]
fn execute(
    _admitted: &AdmittedEngineArtifact<'_>,
    _permit: &EngineBootstrapPermit<'_>,
    _request: &[u8],
    _cancelled: &AtomicBool,
    report: &mut EngineExchangeReport,
) -> Result<Vec<u8>, Failure> {
    phase(report, ExchangePhase::Admission, |_| {
        Err(Failure::refuse(
            "Engine exchange requires Unix process-group containment",
        ))
    })
}

/// Keep both borrowed owners alive through launch, process cleanup, response reading and file cleanup.
#[cfg(unix)]
fn execute(
    admitted: &AdmittedEngineArtifact<'_>,
    permit: &EngineBootstrapPermit<'_>,
    request: &[u8],
    cancelled: &AtomicBool,
    report: &mut EngineExchangeReport,
) -> Result<Vec<u8>, Failure> {
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};

    phase(report, ExchangePhase::Admission, |_| {
        admit(admitted, permit, request, cancelled)
    })?;
    let directory = permit.scratch_parent.join(format!(
        "engine-{}-{}",
        std::process::id(),
        SCOPE_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    let scope = phase(report, ExchangePhase::FileScope, |report| {
        check_interrupt(permit, cancelled)?;
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&directory)
            .map_err(Failure::io)?;
        report.scratch_path = Some(directory.clone());
        Ok(FileScope { directory })
    })?;
    let result = (|| {
        phase(report, ExchangePhase::RequestFile, |_| {
            check_interrupt(permit, cancelled)?;
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(scope.request())
                .map_err(Failure::io)?;
            file.write_all(request).map_err(Failure::io)
        })?;
        let mut child = phase(report, ExchangePhase::Spawn, |report| {
            check_interrupt(permit, cancelled)?;
            // Only the admitted original executable and the two fixed protocol paths reach Command.
            admitted.verify_native_for_execution().map_err(Failure::io)?;
            check_interrupt(permit, cancelled)?;
            let mut command = Command::new(admitted.native_output());
            command
                .args([scope.request(), scope.response()])
                .current_dir(&scope.directory)
                .env_clear()
                .env("HOME", &scope.directory)
                .env("TMPDIR", &scope.directory)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            isolate_process_group(&mut command);
            let child = command.spawn().map_err(Failure::io)?;
            report.child_id = Some(child.id());
            Ok(ChildGroup { child, reaped: false })
        })?;
        let result = phase(report, ExchangePhase::Supervision, |report| {
            supervise(&mut child.child, &scope, permit, cancelled, report)
        });
        let cleanup = phase(report, ExchangePhase::ProcessCleanup, |report| {
            let status = child.finish().map_err(Failure::io)?;
            report.child_status = Some(status);
            Ok(())
        });
        // Cleanup failure takes precedence; both phase outcomes remain in the report.
        cleanup?;
        result?;
        phase(report, ExchangePhase::ResponseRead, |_| {
            check_interrupt(permit, cancelled)?;
            if read_regular(&scope.request(), permit.request_limit)? != request {
                return Err(Failure::io("Engine child changed the original request file"));
            }
            let response = read_regular(&scope.response(), permit.response_limit)?;
            std::str::from_utf8(&response).map_err(Failure::io)?;
            Ok(response)
        })
    })();
    let cleanup = phase(report, ExchangePhase::FileCleanup, |_| {
        scope.close().map_err(Failure::io)
    });
    cleanup?;
    result
}

/// Private, exclusively created directory; cleanup removes only the two known protocol files, never a discovered tree.
struct FileScope {
    directory: PathBuf,
}

impl FileScope {
    /// Return the fixed request coordinate within this invocation's scope.
    fn request(&self) -> PathBuf {
        self.directory.join("request.json")
    }

    /// Return the fixed response coordinate, absent when the child starts.
    fn response(&self) -> PathBuf {
        self.directory.join("response.json")
    }

    /// Retain an unexpected child-created tree instead of recursively removing ungranted paths.
    fn close(&self) -> io::Result<()> {
        for path in [self.request(), self.response()] {
            match fs::remove_file(path) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
        }
        fs::remove_dir(&self.directory)
    }
}

/// Process cleanup fallback for unexpected unwinding; ordinary paths record the explicit cleanup result.
struct ChildGroup {
    child: Child,
    reaped: bool,
}

impl ChildGroup {
    /// Kill normal group descendants even if the direct child already exited, then reap the direct child.
    fn finish(&mut self) -> io::Result<ExitStatus> {
        let status = terminate_process_group(&mut self.child)?;
        self.reaped = true;
        Ok(status)
    }
}

impl Drop for ChildGroup {
    /// Avoid leaving an active process group on an unexpected early unwind.
    fn drop(&mut self) {
        if !self.reaped {
            let _ = self.finish();
        }
    }
}

/// Drain bounded nonblocking pipes while observing the caller's deadline and response-size ceiling.
#[cfg(unix)]
fn supervise(
    child: &mut Child,
    scope: &FileScope,
    permit: &EngineBootstrapPermit<'_>,
    cancelled: &AtomicBool,
    report: &mut EngineExchangeReport,
) -> Result<(), Failure> {
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| Failure::io("Engine stdout pipe missing"))?;
    let mut stderr = child
        .stderr
        .take()
        .ok_or_else(|| Failure::io("Engine stderr pipe missing"))?;
    nonblocking(&stdout)?;
    nonblocking(&stderr)?;
    loop {
        let interrupt = check_interrupt(permit, cancelled);
        // Retain an already available bounded diagnostic prefix even when cancellation precedes this poll.
        let stdout_result = drain(&mut stdout, &mut report.stdout, permit.stdout_limit);
        let stderr_result = drain(&mut stderr, &mut report.stderr, permit.stderr_limit);
        interrupt?;
        stdout_result?;
        stderr_result?;
        match fs::symlink_metadata(scope.response()) {
            Ok(metadata) if !metadata.is_file() || metadata.len() > permit.response_limit as u64 => {
                return Err(Failure::io(
                    "Engine response is indirect, nonregular or exceeds its byte limit",
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(Failure::io(error)),
        }
        if let Some(status) = child.try_wait().map_err(Failure::io)? {
            report.child_status = Some(status);
            // Retain currently available diagnostics. Never await EOF from a surviving descendant.
            drain(&mut stdout, &mut report.stdout, permit.stdout_limit)?;
            drain(&mut stderr, &mut report.stderr, permit.stderr_limit)?;
            if !status.success() {
                return Err(Failure::io(format!("Engine child exited with {status}")));
            }
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(5).min(permit.deadline.saturating_duration_since(Instant::now())));
    }
}

/// Use the existing rustix filesystem dependency to avoid blocking pipe reads or additional reader processes.
#[cfg(unix)]
fn nonblocking(pipe: &impl std::os::fd::AsFd) -> Result<(), Failure> {
    let flags = rustix::fs::fcntl_getfl(pipe).map_err(Failure::io)?;
    rustix::fs::fcntl_setfl(pipe, flags | rustix::fs::OFlags::NONBLOCK).map_err(Failure::io)
}

/// Capture at most the authorized prefix and stop on overflow, EOF or a currently empty pipe.
fn drain(reader: &mut impl Read, capture: &mut DiagnosticCapture, limit: usize) -> Result<(), Failure> {
    let mut chunk = [0_u8; 4096];
    loop {
        let capacity = (limit.saturating_sub(capture.bytes.len()) + 1).min(chunk.len());
        match reader.read(&mut chunk[..capacity]) {
            Ok(0) => {
                capture.eof = true;
                return Ok(());
            }
            Ok(count) => {
                let keep = count.min(limit.saturating_sub(capture.bytes.len()));
                capture.bytes.extend_from_slice(&chunk[..keep]);
                if keep != count {
                    capture.exceeded = true;
                    return Err(Failure::io("Engine diagnostic byte limit exceeded"));
                }
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(()),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => return Ok(()),
            Err(error) => return Err(Failure::io(error)),
        }
    }
}

/// Read a single bounded direct file without following symlinks, blocking on FIFOs or accepting external hardlinks.
#[cfg(unix)]
fn read_regular(path: &Path, limit: usize) -> Result<Vec<u8>, Failure> {
    use std::os::unix::fs::MetadataExt;
    let fd = rustix::fs::open(
        path,
        rustix::fs::OFlags::RDONLY | rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::NONBLOCK,
        rustix::fs::Mode::empty(),
    )
    .map_err(Failure::io)?;
    let file = File::from(fd);
    let metadata = file.metadata().map_err(Failure::io)?;
    if !metadata.is_file() || metadata.nlink() != 1 || metadata.len() > limit as u64 {
        return Err(Failure::io(
            "Engine exchange file is not a bounded private regular file",
        ));
    }
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(Failure::io)?;
    if bytes.len() > limit {
        return Err(Failure::io(
            "Engine exchange file exceeded its byte limit while reading",
        ));
    }
    Ok(bytes)
}
