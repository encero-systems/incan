//! The compiler worker host: it listens on the worker's private socket, admits one request at a time, hands it to the
//! compiler entry, answers it with the entry's outcome and applies the worker's lifecycle.
//!
//! A retiring worker withdraws its socket before it sends its last reply, so no later request dials it; it then answers
//! every request that connected before the withdrawal, each reply marked retired, unless the process is unsafe or its
//! caller required teardown after one activation. Between requests the worker polls its socket and decides for itself:
//! it retires once it has been idle for its idle period, or once nothing can reach it any more.

use std::thread::sleep;
use std::time::{Duration, Instant};

use incan_worker_host_bindings::{
    Clock, Connection, Emitter, Listener, SocketFile, bound_reads, bound_writes, close_connection,
    connection_descriptor, dial, emit_activation, listen, open_emitter, peer_uid, poll_listener, reachable,
    receive_frame, send_frame, socket_file, socket_owner, trace_begin, withdraw,
};

use crate::endpoint::CallerWatch;
use crate::lifecycle::{
    ActivationOutcome, Bounds, FOOTPRINT_CEILING, FOOTPRINT_MARGIN, IDLE_SECONDS, Lifecycle, Retirement, lifecycle,
};
use crate::protocol::{Admission, CompileRequest, MISMATCH, Reply, WorkerCommand, admit, ready_frame, reply_frame};
use crate::record_protocol::{RecordAdmission, RecordMessage, accept_record_reply, admit_record, record_frame};

/// The seconds a connected caller has to send its request, and to take the reply.
const READ_SECONDS: u64 = 5;
const WRITE_SECONDS: u64 = 10;
/// A test-only setting that lowers, and never raises, the footprint ceiling.
const FOOTPRINT_SETTING: &str = "INCAN_WORKER_FOOTPRINT_BOUND";
/// How long a waiting worker sleeps before it looks for a caller again.
const POLL_MILLIS: u64 = 2;
/// Wait more quietly after ordinary idleness while an editor still owns the warm state.
const RETAINED_POLL_MILLIS: u64 = 32;
const REACHABILITY_MILLIS: u128 = 1000;

/// What a worker host starts with.
#[derive(Clone, Debug)]
pub struct HostSettings {
    pub socket: String,
    pub generation: String,
    /// The socket of the process that started the worker, told when it listens; empty tells nobody.
    pub ready: String,
    /// The private directory of the home the worker serves; once it is gone, the worker retires.
    pub home: String,
    pub bounds: Bounds,
    /// The only user served; `None` serves the user that owns the socket, which is the worker's own.
    pub owner: Option<u32>,
    /// The descriptor directory authorizing record reads; socket lifecycle continues to use home.
    pub snapshot_authority: String,
}

/// Where a worker is in its life.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Serving,
    /// The socket is withdrawn; requests that connected before are answered, each reply marked retired.
    Draining,
    Ended,
}

/// One admitted compile command or record request, never both protocols for one activation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WorkerActivation {
    Compile(CompileRequest),
    Record(RecordMessage),
}

/// One worker's socket, its policy state and the activation it is running, if any.
pub struct WorkerHost {
    pub generation: String,
    pub phase: Phase,
    home: String,
    socket: SocketFile,
    listener: Listener,
    owner: u32,
    policy: Lifecycle,
    active: Option<Connection>,
    active_record: Option<RecordMessage>,
    retire_after: bool,
    /// Whether the open activation only reads sources, which the footprint policy holds only to its ceiling.
    active_light: bool,
    active_editor: bool,
    active_record_build: bool,
    observer: Option<Emitter>,
    activation_clock: Option<Clock>,
}

impl WorkerHost {
    /// The explicit home this resident worker was started to serve.
    pub fn selected_home(&self) -> String {
        self.home.clone()
    }

    /// The next request for the compiler entry, or `None` once the worker has retired. An activation the entry did
    /// not complete leaves the process in an unknown state, which ends the worker.
    pub fn next(&mut self) -> Option<WorkerActivation> {
        if self.active.is_some() {
            self.settle(
                ActivationOutcome::UnsafeCleanup("the compiler entry did not complete its activation".into()),
                -1,
                -1,
            );
            return None;
        }
        while self.phase != Phase::Ended {
            match self.connection() {
                None => self.phase = Phase::Ended,
                Some(caller) => {
                    if let Some(request) = self.receive(caller) {
                        return Some(request);
                    }
                }
            }
        }
        None
    }

    /// Answer an open compile activation; using this completion for a record request is a protocol fault.
    pub fn complete(&mut self, outcome: ActivationOutcome, footprint: i64, live: i64) {
        if self.active.is_none() {
            return;
        }
        if self.active_record.is_some() {
            self.protocol_fault();
            return;
        }
        self.settle(outcome, footprint, live);
    }

    /// Answer one record request only with a member of that request's closed reply set.
    pub fn complete_record(&mut self, reply: RecordMessage, footprint: i64, live: i64) {
        if self.active.is_none() {
            return;
        }
        let accepted = match &self.active_record {
            None => None,
            Some(request) => accept_record_reply(request, reply).ok(),
        };
        match accepted {
            None => self.protocol_fault(),
            Some(accepted) => self.settle_record(accepted, footprint, live),
        }
    }

    /// Answer the open activation: decide the reply and any retirement, which withdraws the socket before the reply is
    /// sent, then send the reply and close the activation.
    fn settle(&mut self, outcome: ActivationOutcome, footprint: i64, live: i64) {
        self.finish_observation();
        if (self.active_editor || self.active_record_build)
            && let ActivationOutcome::Exited(code) = outcome
            && (self.active_editor || code == 0)
        {
            self.policy.note_retained_activity(code == 0);
        }
        let settled = self.policy.settle(outcome, footprint, self.active_light, live);
        if let Some(reason) = settled.retirement {
            self.retire(reason);
        }
        if self.retire_after {
            self.retire(Retirement::ActivationComplete);
        }
        if let Some(caller) = self.active.take() {
            self.answer(caller, &settled.reply);
        }
        self.active_record = None;
    }

    /// Apply ordinary worker lifetime policy, then send the already validated record reply.
    fn settle_record(&mut self, reply: RecordMessage, footprint: i64, live: i64) {
        self.finish_observation();
        let settled = self.policy.settle(ActivationOutcome::Exited(0), footprint, false, live);
        if let Some(reason) = settled.retirement {
            self.retire(reason);
        }
        if let Some(caller) = self.active.take() {
            self.answer_record(caller, &reply);
        }
        self.active_record = None;
    }

    /// End the unsafe worker and close its caller without inventing a reply from another request's set.
    pub fn protocol_fault(&mut self) {
        self.finish_observation();
        self.retire(Retirement::Unsafe);
        if let Some(caller) = self.active.take() {
            hang_up(&caller);
        }
        self.active_record = None;
    }

    /// Flush one activation before its reply, so the caller's request span encloses the worker record.
    fn finish_observation(&mut self) {
        if let (Some(emitter), Some(clock)) = (&self.observer, &self.activation_clock) {
            emit_activation(emitter, clock);
        }
        self.activation_clock = None;
    }

    /// Whether the open compile, at this footprint after lowering, hands rustc to its caller instead of running it.
    pub fn hands_rustc_off(&self, footprint: i64) -> bool {
        self.policy.hands_rustc_off(footprint)
    }

    /// What the compiler entry watches while the open activation compiles, if one is open.
    pub fn caller_watch(&self) -> Option<CallerWatch> {
        self.active.as_ref().map(|caller| CallerWatch {
            descriptor: connection_descriptor(caller),
            socket: self.socket.clone(),
        })
    }

    /// The next caller: wait for one while serving, retiring once idle or unreachable; take only those already queued
    /// while draining.
    fn connection(&mut self) -> Option<Connection> {
        let mut reachability = Instant::now();
        while self.phase == Phase::Serving {
            match poll_listener(&mut self.listener) {
                Ok(Some(found)) => return Some(found),
                Ok(None) => {}
                Err(_) => {
                    // A listener that cannot accept serves nobody; retiring lets the next request start a worker.
                    self.retire(Retirement::Unsafe);
                    return None;
                }
            }
            let pause = if self.policy.past_ordinary_idle() {
                RETAINED_POLL_MILLIS
            } else {
                POLL_MILLIS
            };
            sleep(Duration::from_millis(pause));
            if self.policy.idle() {
                self.retire(Retirement::Idle);
            } else if reachability.elapsed().as_millis() >= REACHABILITY_MILLIS {
                reachability = Instant::now();
                if !reachable(&self.socket) {
                    self.retire(Retirement::Unreachable);
                }
            }
        }
        if self.phase == Phase::Draining {
            return poll_listener(&mut self.listener).ok().flatten();
        }
        None
    }

    /// The request one caller sends, admitted, or `None` when the caller was refused or answered. Only the worker's
    /// own user is served; a frame that is not a request is refused; a request of another protocol or generation
    /// retires the worker; an admitted request opens the activation.
    fn receive(&mut self, connection: Connection) -> Option<WorkerActivation> {
        let mut caller = connection;
        bound_reads(&mut caller, READ_SECONDS);
        bound_writes(&mut caller, WRITE_SECONDS);
        if !self.served_peer(&caller) {
            hang_up(&caller);
            return None;
        }
        let payload = match receive_frame(&mut caller) {
            Err(reason) => {
                self.answer(caller, &Reply::Refused(reason));
                return None;
            }
            Ok(payload) => payload,
        };
        match admit_record(&payload, &self.generation) {
            RecordAdmission::Admitted(request) => {
                self.open(caller, false, false);
                self.active_record = Some(request.clone());
                Some(WorkerActivation::Record(request))
            }
            RecordAdmission::Incompatible => {
                self.retire(Retirement::Incompatible);
                hang_up(&caller);
                None
            }
            RecordAdmission::ProtocolFault => {
                self.retire(Retirement::Unsafe);
                hang_up(&caller);
                None
            }
            RecordAdmission::NotRecordProtocol => self.receive_compile(caller, &payload),
        }
    }

    /// Admit a compile request, or refuse or retire for it.
    fn receive_compile(&mut self, caller: Connection, payload: &[u8]) -> Option<WorkerActivation> {
        match admit(payload, &self.generation) {
            Admission::Refused(reason) => {
                self.answer(caller, &Reply::Refused(reason));
                None
            }
            Admission::Incompatible => {
                // The socket is withdrawn before the refusal, so its caller starts a fresh worker.
                self.retire(Retirement::Incompatible);
                self.answer(caller, &Reply::Refused(MISMATCH.to_owned()));
                None
            }
            Admission::Admitted(request) => {
                self.open(caller, request.retire_after, request.command.light());
                self.active_editor = request.command == WorkerCommand::Check
                    && request
                        .arguments
                        .iter()
                        .any(|argument| argument == "--retain-checked-modules");
                self.active_record_build = request.command == WorkerCommand::Unit
                    && request
                        .arguments
                        .iter()
                        .any(|argument| argument == "--graph-body-output")
                    && request
                        .arguments
                        .iter()
                        .any(|argument| argument == "--plan-record-products");
                Some(WorkerActivation::Compile(request))
            }
        }
    }

    /// Open an activation for this caller: the caller stays connected until its activation is answered.
    fn open(&mut self, caller: Connection, retire_after: bool, light: bool) {
        self.activation_clock = trace_begin(self.observer.is_some());
        self.active = Some(caller);
        self.retire_after = retire_after;
        self.active_light = light;
    }

    /// Whether the OS reports the caller as the user this worker serves.
    fn served_peer(&self, caller: &Connection) -> bool {
        peer_uid(caller).is_ok_and(|uid| uid == self.owner)
    }

    /// Send one reply, marked retired once the socket is withdrawn, then close the connection.
    fn answer(&self, caller: Connection, reply: &Reply) {
        let mut answered = caller;
        if let Ok(frame) = reply_frame(reply, self.phase != Phase::Serving, &self.generation) {
            // A reply whose caller has gone needs no delivery: it already left.
            let _ = send_frame(&mut answered, &frame);
        }
        hang_up(&answered);
    }

    /// Send one record reply and close its activation; lifecycle never adds fields to this schema.
    fn answer_record(&self, caller: Connection, reply: &RecordMessage) {
        let mut answered = caller;
        if let Ok(frame) = record_frame(reply, &self.generation) {
            let _ = send_frame(&mut answered, &frame);
        }
        hang_up(&answered);
    }

    /// Withdraw the socket; unsafe and one-activation workers end, while other retirements drain queued requests.
    fn retire(&mut self, reason: Retirement) {
        if self.phase == Phase::Serving {
            withdraw(&self.socket);
        }
        if matches!(reason, Retirement::Unsafe | Retirement::ActivationComplete) {
            self.phase = Phase::Ended;
            return;
        }
        self.phase = Phase::Draining;
    }
}

/// Start the worker host of `--resident SOCKET GENERATION READY HOME [SNAPSHOT_AUTHORITY]`. The endpoints and the
/// home are absolute, because the worker leaves its caller's directory; its own directory is the root.
pub fn start(arguments: &[String]) -> Result<WorkerHost, String> {
    if !(4..=5).contains(&arguments.len()) {
        return Err("usage: --resident SOCKET GENERATION READY SOCKET_DIRECTORY [SNAPSHOT_AUTHORITY]".to_owned());
    }
    if [0, 2, 3].iter().any(|index| !arguments[*index].starts_with('/')) {
        return Err("compiler worker endpoints and home must be absolute".to_owned());
    }
    let mut authority = arguments[3].clone();
    if let Some(selected) = arguments.get(4) {
        if !selected.starts_with('/') {
            return Err("compiler snapshot authority must be absolute".to_owned());
        }
        authority = selected.clone();
    }
    std::env::set_current_dir("/").map_err(|error| format!("cannot leave the starting directory: {error}"))?;
    host_for(&HostSettings {
        socket: arguments[0].clone(),
        generation: arguments[1].clone(),
        ready: arguments[2].clone(),
        home: arguments[3].clone(),
        snapshot_authority: authority,
        bounds: Bounds::new(IDLE_SECONDS * 1000, FOOTPRINT_MARGIN, footprint_ceiling()),
        owner: None,
    })
}

/// The resident worker's explicit home, for its native executable entry.
pub fn selected_home(host: &WorkerHost) -> String {
    host.selected_home()
}

/// A listening worker host: bind the private socket, polled, remember the exact file it created and the home it
/// belongs to, and tell the starting process the worker listens.
pub fn host_for(settings: &HostSettings) -> Result<WorkerHost, String> {
    let listener = listen(&settings.socket, false)?;
    let bound = socket_file(settings.socket.clone(), settings.home.clone())?;
    let home = if settings.snapshot_authority.is_empty() {
        settings.home.clone()
    } else {
        settings.snapshot_authority.clone()
    };
    let owner = settings.owner.unwrap_or_else(|| socket_owner(&bound));
    let host = WorkerHost {
        generation: settings.generation.clone(),
        phase: Phase::Serving,
        home,
        socket: bound,
        listener,
        owner,
        policy: lifecycle(settings.bounds),
        active: None,
        active_record: None,
        retire_after: false,
        active_light: false,
        active_editor: false,
        active_record_build: false,
        observer: worker_observer(),
        activation_clock: None,
    };
    // A worker that cannot announce itself is never sent a request.
    if !settings.ready.is_empty() {
        let mut announcing = dial(&settings.ready)?;
        send_frame(
            &mut announcing,
            &ready_frame(&settings.generation, i64::from(std::process::id()))?,
        )?;
        hang_up(&announcing);
    }
    Ok(host)
}

/// The next compile request for the compiler entry, or `None` once the worker has retired; a record request here is
/// a protocol fault.
pub fn next_request(host: &mut WorkerHost) -> Option<CompileRequest> {
    match host.next() {
        Some(WorkerActivation::Compile(request)) => Some(request),
        Some(WorkerActivation::Record(_)) => {
            host.protocol_fault();
            None
        }
        None => None,
    }
}

/// The next compile or record activation for the compiler entry, or `None` once the worker retires.
pub fn next_activation(host: &mut WorkerHost) -> Option<WorkerActivation> {
    host.next()
}

/// Answer the open activation with the entry's outcome, given the physical footprint and live heap after it (each
/// negative when unknown), and apply the lifecycle.
pub fn complete(host: &mut WorkerHost, outcome: &ActivationOutcome, footprint: i64, live: i64) {
    host.complete(outcome.clone(), footprint, live);
}

/// Validate and answer the open record activation, applying the worker's ordinary lifetime policy.
pub fn complete_record(host: &mut WorkerHost, reply: RecordMessage, footprint: i64, live: i64) {
    host.complete_record(reply, footprint, live);
}

/// What the compiler entry watches while the open activation compiles; `None` when no activation is open.
pub fn caller_watch(host: &mut WorkerHost) -> Option<CallerWatch> {
    host.caller_watch()
}

/// Whether the open compile, at this physical footprint after checking and lowering, hands rustc to its caller.
pub fn hands_rustc_off(host: &WorkerHost, footprint: i64) -> bool {
    host.hands_rustc_off(footprint)
}

/// Open the requested canonical worker trace channel once; absent tracing starts no clock or writer.
fn worker_observer() -> Option<Emitter> {
    let directory = environment_or("INCAN_ENGINE_TRACE");
    if directory.is_empty() {
        return None;
    }
    open_emitter(
        &format!("{directory}/engine-compiler-{}.jsonl", std::process::id()),
        "compiler-worker".to_owned(),
        environment_or("INCAN_ENGINE_RUN"),
        environment_or("INCAN_ENGINE_REQUEST"),
    )
}

/// The footprint ceiling, lowered by the test-only setting when it is a smaller decimal number of bytes.
fn footprint_ceiling() -> i64 {
    let text = environment_or(FOOTPRINT_SETTING);
    if !text.is_empty()
        && text.bytes().all(|byte| byte.is_ascii_digit())
        && let Ok(bound) = text.parse::<i64>()
        && bound < FOOTPRINT_CEILING
    {
        return bound;
    }
    FOOTPRINT_CEILING
}

/// An environment value, or empty when it is unset or not text.
fn environment_or(key: &str) -> String {
    std::env::var(key).unwrap_or_default()
}

/// Close a connection; one its peer already closed needs nothing more.
fn hang_up(connection: &Connection) {
    let _ = close_connection(connection);
}
