//! Worker host contracts, ported from `loaves/compiler/incan_worker_host/tests/test_worker_host.incn`: what a caller
//! of a compiler worker observes on its socket, and the lifecycle policy behind it.
//!
//! Each case connects its callers and sends their frames before the host reads them, so one thread plays both sides.

use std::os::unix::fs::MetadataExt;
use std::thread::sleep;
use std::time::{Duration, Instant};

use incan_worker_host_bindings::{Connection, bound_reads, dial, listen, receive_frame, send_frame, write_raw};

use crate::host::{
    HostSettings, WorkerActivation, WorkerHost, complete, complete_record, hands_rustc_off, host_for, next_activation,
    next_request,
};
use crate::lifecycle::{ActivationOutcome, Bounds, FOOTPRINT_CEILING, FOOTPRINT_MARGIN, Retirement, lifecycle};
use crate::protocol::{
    CompileRequest, MISMATCH, PANIC_STATUS, SCHEMA, WorkerCommand, compile_frame, handoff, parse_document,
    read_completion, retired,
};
use crate::record_protocol::{
    Evaluate, Prepare, Prepared, Produced, RecordMessage, decode_record_message, encode_record_message, record_frame,
};

const GENERATION: &str = "0123456789abcdef";

type Outcome = Result<(), String>;

/// A fresh socket path for one case, short enough to bind.
fn socket_path(name: &str) -> String {
    let path = std::env::temp_dir()
        .join(format!("iwhn-{}-{name}.sock", std::process::id()))
        .to_string_lossy()
        .into_owned();
    discard(&path);
    path
}

fn discard(path: &str) {
    let _ = std::fs::remove_file(path);
}

/// The user running the cases, read from a file it creates.
fn own_uid() -> Result<u32, String> {
    let path = socket_path("uid");
    std::fs::File::create(&path).map_err(|error| format!("cannot create {path}: {error}"))?;
    let uid = std::fs::metadata(&path)
        .map_err(|error| format!("cannot read {path}: {error}"))?
        .uid();
    discard(&path);
    Ok(uid)
}

/// A host that announces itself to nobody.
fn settings(socket: &str, idle_millis: i64, margin: i64, owner: Option<u32>) -> HostSettings {
    HostSettings {
        socket: socket.to_owned(),
        generation: GENERATION.to_owned(),
        ready: String::new(),
        home: std::env::temp_dir().to_string_lossy().into_owned(),
        bounds: Bounds::new(idle_millis, margin, FOOTPRINT_CEILING),
        owner,
        snapshot_authority: String::new(),
    }
}

const SINKS: &str = "\"cwd\":\"/p\",\"stdout\":\"/p/r.out\",\"stderr\":\"/p/r.err\"";

/// A request document for a formatting run, as Oven sends it.
fn request_text(generation: &str, command: &str, retire_after: bool) -> String {
    let arguments = format!("[\"{command}\",\"/p/unit.json\",\"src/main.incn\",\"--check\"]");
    let policy = if retire_after { ",\"retire_after\":true" } else { "" };
    format!("{{\"schema\":\"{SCHEMA}\",\"generation\":\"{generation}\",\"arguments\":{arguments},{SINKS}{policy}}}")
}

/// A request document checking a unit against its sysroot, as Oven sends it.
fn check_text(generation: &str) -> String {
    let arguments = "[\"--check\",\"/p/unit.json\",\"/p/sysroot\"]";
    format!("{{\"schema\":\"{SCHEMA}\",\"generation\":\"{generation}\",\"arguments\":{arguments},{SINKS}}}")
}

/// A request document compiling a unit against its sysroot, as Oven sends it.
fn unit_text(generation: &str) -> String {
    let arguments = "[\"--unit\",\"/p/unit.json\",\"unit\",\"/p/out/libunit.rlib\",\"/p/sysroot\"]";
    format!("{{\"schema\":\"{SCHEMA}\",\"generation\":\"{generation}\",\"arguments\":{arguments},{SINKS}}}")
}

/// A caller that sent one framed payload and waits at most five seconds for its reply.
fn caller(socket: &str, payload: &str) -> Result<Connection, String> {
    let mut connected = dial(&socket.to_owned())?;
    bound_reads(&mut connected, 5);
    send_frame(&mut connected, &payload.as_bytes().to_vec())?;
    Ok(connected)
}

/// A caller that wrote exactly these bytes, as a broken client does.
fn raw_caller(socket: &str, sent: &[u8]) -> Result<Connection, String> {
    let mut connected = dial(&socket.to_owned())?;
    bound_reads(&mut connected, 5);
    write_raw(&mut connected, &sent.to_vec())?;
    Ok(connected)
}

/// A record caller that sent one framed request and waits for its reply.
fn record_caller(socket: &str, request: &RecordMessage) -> Result<Connection, String> {
    let mut connected = dial(&socket.to_owned())?;
    bound_reads(&mut connected, 5);
    send_frame(&mut connected, &encode_record_message(request, GENERATION))?;
    Ok(connected)
}

/// The reply document a caller received.
fn reply(connected: &mut Connection) -> Result<serde_json::Value, String> {
    let payload = receive_frame(connected)?;
    parse_document(std::str::from_utf8(&payload).map_err(|_| "the reply is not UTF-8".to_owned())?)
}

fn exit_of(document: &serde_json::Value) -> i64 {
    document.get("exit").and_then(serde_json::Value::as_i64).unwrap_or(-1)
}

fn reason_of(document: &serde_json::Value) -> String {
    document
        .get("reason")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

fn withdrawn(socket: &str) -> bool {
    std::fs::metadata(socket).is_err()
}

fn served(host: &mut WorkerHost) -> Result<CompileRequest, String> {
    next_request(host).ok_or_else(|| "the host retired instead of serving a request".to_owned())
}

fn served_record(host: &mut WorkerHost) -> Result<RecordMessage, String> {
    match next_activation(host) {
        Some(WorkerActivation::Record(request)) => Ok(request),
        Some(WorkerActivation::Compile(_)) => Err("the host substituted a compile request".to_owned()),
        None => Err("the host retired instead of serving a record request".to_owned()),
    }
}

fn record_reply(connected: &mut Connection) -> Result<RecordMessage, String> {
    decode_record_message(&receive_frame(connected)?, GENERATION)
}

fn ended(host: &mut WorkerHost) -> bool {
    next_request(host).is_none()
}

fn ended_within(host: &mut WorkerHost, millis: u128) -> bool {
    let started = Instant::now();
    ended(host) && started.elapsed().as_millis() < millis
}

fn check_is(request: &CompileRequest, command: WorkerCommand) -> Outcome {
    if request.command == command {
        return Ok(());
    }
    Err(format!("expected {command:?}, served {:?}", request.command))
}

#[test]
fn record_requests_cross_the_host_with_their_closed_replies() -> Outcome {
    let socket = socket_path("records");
    let mut selected = settings(&socket, 10000, 16_777_216, None);
    selected.snapshot_authority = format!("{}/independent-snapshot-authority", selected.home);
    let mut host = host_for(&selected)?;
    assert_eq!(host.selected_home(), selected.snapshot_authority);
    let prepare = RecordMessage::Prepare(Prepare {
        stage: "local".into(),
        snapshot: "s".into(),
        demand_root: "d".into(),
        producer_profile: "p".into(),
    });
    let mut preparing = record_caller(&socket, &prepare)?;
    assert!(matches!(served_record(&mut host)?, RecordMessage::Prepare(value) if value.stage == "local"));
    complete_record(
        &mut host,
        RecordMessage::Prepared(Prepared {
            prepared_root: "prepared".into(),
        }),
        -1,
        -1,
    );
    assert!(
        matches!(record_reply(&mut preparing)?, RecordMessage::Prepared(value) if value.prepared_root == "prepared")
    );
    let evaluate = RecordMessage::Evaluate(Evaluate {
        work_root: "work".into(),
        snapshot: "s".into(),
        prepared_root: "prepared".into(),
        reservation: "reservation".into(),
        cancellation_generation: 0,
    });
    let produced = RecordMessage::Produced(Produced {
        staged_root: "staged".into(),
        witness_root: "witnesses".into(),
        diagnostic_root: "diagnostics".into(),
        counter_root: "c".into(),
    });
    let mut evaluating = record_caller(&socket, &evaluate)?;
    assert!(matches!(served_record(&mut host)?, RecordMessage::Evaluate(value) if value.work_root == "work"));
    complete_record(&mut host, produced.clone(), -1, -1);
    assert!(matches!(record_reply(&mut evaluating)?, RecordMessage::Produced(value) if value.staged_root == "staged"));
    let mut faulty = record_caller(&socket, &prepare)?;
    let _ = served_record(&mut host)?;
    complete_record(&mut host, produced, -1, -1);
    assert!(record_reply(&mut faulty).is_err_and(|reason| !reason.is_empty()));
    assert!(ended(&mut host));
    Ok(())
}

#[test]
fn malformed_frames_are_refused_and_the_next_request_is_served() -> Outcome {
    let socket = socket_path("frames");
    let mut host = host_for(&settings(&socket, 10000, 16_777_216, None))?;
    let mut not_a_length = raw_caller(&socket, b"12ab5678")?;
    let mut over_bound = raw_caller(&socket, b"99999999")?;
    let mut not_json = caller(&socket, "{not json")?;
    let mut unserved = caller(&socket, &request_text(GENERATION, "--run", false))?;
    let invalid_policy =
        request_text(GENERATION, "--fmt", false).replace("\"cwd\"", "\"retire_after\":\"yes\",\"cwd\"");
    let mut invalid_lifetime = caller(&socket, &invalid_policy)?;
    let imports = format!(
        "{{\"schema\":\"{SCHEMA}\",\"generation\":\"{GENERATION}\",\"arguments\":[\"--rust-imports\",\"/p/unit.json\"],{SINKS}}}"
    );
    let mut import_discovery = caller(&socket, &imports)?;
    let mut valid = caller(&socket, &request_text(GENERATION, "--fmt", false))?;
    let listing = format!(
        "{{\"schema\":\"{SCHEMA}\",\"generation\":\"{GENERATION}\",\"arguments\":[\"--source-files\",\"src\"],{SINKS}}}"
    );
    let mut discovery = caller(&socket, &listing)?;
    let request = served(&mut host)?;
    check_is(&request, WorkerCommand::RustImports)?;
    assert_eq!(request.arguments, vec!["/p/unit.json"]);
    complete(&mut host, &ActivationOutcome::Exited(0), -1, -1);
    assert_eq!(exit_of(&reply(&mut import_discovery)?), 0);
    let request = served(&mut host)?;
    check_is(&request, WorkerCommand::Format)?;
    assert_eq!(request.arguments, vec!["/p/unit.json", "src/main.incn", "--check"]);
    assert!(request.cwd == "/p" && request.stdout == "/p/r.out" && request.stderr == "/p/r.err");
    for refused in [
        &mut not_a_length,
        &mut over_bound,
        &mut not_json,
        &mut unserved,
        &mut invalid_lifetime,
    ] {
        let document = reply(refused)?;
        assert_eq!(exit_of(&document), 1);
        assert!(!reason_of(&document).is_empty());
        assert!(!retired(&document));
    }
    complete(&mut host, &ActivationOutcome::Exited(0), -1, -1);
    let answered = reply(&mut valid)?;
    assert!(exit_of(&answered) == 0 && reason_of(&answered).is_empty() && !retired(&answered));
    let listed = served(&mut host)?;
    check_is(&listed, WorkerCommand::SourceFiles)?;
    assert_eq!(listed.arguments, vec!["src"]);
    complete(&mut host, &ActivationOutcome::Exited(0), -1, -1);
    assert_eq!(exit_of(&reply(&mut discovery)?), 0);
    assert!(!withdrawn(&socket));
    discard(&socket);
    Ok(())
}

#[test]
fn a_peer_of_another_user_is_closed_unanswered() -> Outcome {
    let socket = socket_path("peer");
    let other = own_uid()?.wrapping_add(1);
    let mut host = host_for(&settings(&socket, 300, 16_777_216, Some(other)))?;
    let mut foreign = caller(&socket, &request_text(GENERATION, "--fmt", false))?;
    assert!(ended(&mut host));
    assert!(reply(&mut foreign).is_err(), "a foreign peer was answered");
    assert!(withdrawn(&socket));
    Ok(())
}

#[test]
fn an_idle_worker_retires_and_withdraws_its_socket() -> Outcome {
    let socket = socket_path("idle");
    let started = Instant::now();
    let mut host = host_for(&settings(&socket, 300, 16_777_216, None))?;
    assert!(ended(&mut host));
    assert!(started.elapsed().as_millis() >= 300);
    assert!(withdrawn(&socket));
    Ok(())
}

#[test]
fn editor_check_keeps_the_worker_through_ordinary_idleness() -> Outcome {
    let socket = socket_path("editor-idle");
    let mut configuration = settings(&socket, 40, 16_777_216, None);
    configuration.bounds = Bounds {
        retained_idle_millis: 300,
        ..Bounds::new(40, 16_777_216, FOOTPRINT_CEILING)
    };
    let mut host = host_for(&configuration)?;
    let editor = check_text(GENERATION).replace("\"/p/sysroot\"]", "\"/p/sysroot\",\"--retain-checked-modules\"]");
    let mut initial = caller(&socket, &editor)?;
    check_is(&served(&mut host)?, WorkerCommand::Check)?;
    complete(&mut host, &ActivationOutcome::Exited(0), 1000, -1);
    assert!(!retired(&reply(&mut initial)?));
    sleep(Duration::from_millis(90));
    let mut renewed = caller(&socket, &editor)?;
    check_is(&served(&mut host)?, WorkerCommand::Check)?;
    let started = Instant::now();
    complete(&mut host, &ActivationOutcome::Exited(0), 1000, -1);
    assert!(!retired(&reply(&mut renewed)?));
    sleep(Duration::from_millis(90));
    let mut next = caller(&socket, &check_text(GENERATION))?;
    check_is(&served(&mut host)?, WorkerCommand::Check)?;
    complete(&mut host, &ActivationOutcome::Exited(0), 1000, -1);
    assert!(!retired(&reply(&mut next)?));
    assert!(ended(&mut host));
    assert!(started.elapsed().as_millis() >= 300);
    assert!(withdrawn(&socket));
    Ok(())
}

#[test]
fn saved_build_keeps_the_worker_through_ordinary_idleness() -> Outcome {
    let socket = socket_path("saved-build-idle");
    let mut configuration = settings(&socket, 40, FOOTPRINT_MARGIN, None);
    configuration.bounds = Bounds {
        retained_idle_millis: 300,
        ..Bounds::new(40, FOOTPRINT_MARGIN, FOOTPRINT_CEILING)
    };
    let mut host = host_for(&configuration)?;
    let saved = unit_text(GENERATION).replace(
        "\"/p/sysroot\"]",
        "\"/p/sysroot\",\"--graph-body-output\",\"/p/bodies\",\"--plan-record-products\",\"/p/products\"]",
    );
    let mut initial = caller(&socket, &saved)?;
    check_is(&served(&mut host)?, WorkerCommand::Unit)?;
    complete(&mut host, &ActivationOutcome::Exited(0), 1000, -1);
    assert!(!retired(&reply(&mut initial)?));
    sleep(Duration::from_millis(90));
    let mut renewed = caller(&socket, &saved)?;
    check_is(&served(&mut host)?, WorkerCommand::Unit)?;
    let started = Instant::now();
    complete(&mut host, &ActivationOutcome::Exited(0), 1000, -1);
    assert!(!retired(&reply(&mut renewed)?));
    assert!(ended(&mut host));
    assert!(started.elapsed().as_millis() >= 300);
    Ok(())
}

#[test]
fn editor_checks_measure_growth_from_the_first_successful_project() {
    let bounds = Bounds::new(10000, 100, 5000);
    let mut policy = lifecycle(bounds);
    assert_eq!(
        policy.settle(ActivationOutcome::Exited(0), 1000, false, 800).retirement,
        None
    );
    policy.note_retained_activity(false);
    assert_eq!(
        policy.settle(ActivationOutcome::Exited(1), 1000, false, 800).retirement,
        None
    );
    for _version in 0..3 {
        policy.note_retained_activity(true);
        assert_eq!(
            policy
                .settle(ActivationOutcome::Exited(0), 3200, false, 2400)
                .retirement,
            None
        );
    }
    policy.note_retained_activity(true);
    assert_eq!(
        policy
            .settle(ActivationOutcome::Exited(0), 3350, false, 2520)
            .retirement,
        None
    );
    policy.note_retained_activity(true);
    assert_eq!(
        policy
            .settle(ActivationOutcome::Exited(0), 3350, false, 2520)
            .retirement,
        Some(Retirement::Footprint)
    );
    let mut ceiling = lifecycle(bounds);
    ceiling.note_retained_activity(true);
    assert_eq!(
        ceiling
            .settle(ActivationOutcome::Exited(0), 5001, false, 2400)
            .retirement,
        Some(Retirement::Footprint)
    );
}

#[test]
fn a_worker_retires_once_its_socket_or_home_is_gone() -> Outcome {
    let socket = socket_path("socketless");
    let mut socketless = host_for(&settings(&socket, 60000, 16_777_216, None))?;
    discard(&socket);
    assert!(ended_within(&mut socketless, 5000));
    let replaced = socket_path("replaced");
    let mut displaced = host_for(&settings(&replaced, 60000, 16_777_216, None))?;
    discard(&replaced);
    let _replacement = listen(&replaced, true)?;
    assert!(ended_within(&mut displaced, 5000));
    assert!(!withdrawn(&replaced));
    discard(&replaced);
    let home = std::env::temp_dir()
        .join(format!("iwhn-{}-home", std::process::id()))
        .to_string_lossy()
        .into_owned();
    std::fs::create_dir_all(&home).map_err(|error| format!("cannot create {home}: {error}"))?;
    let homeless_socket = socket_path("homeless");
    let mut homeless_settings = settings(&homeless_socket, 60000, 16_777_216, None);
    homeless_settings.home = home.clone();
    let mut homeless = host_for(&homeless_settings)?;
    std::fs::remove_dir_all(&home).map_err(|error| format!("cannot remove {home}: {error}"))?;
    assert!(ended_within(&mut homeless, 5000));
    assert!(withdrawn(&homeless_socket));
    Ok(())
}

#[test]
fn a_worker_still_retires_when_idle_after_an_activation_longer_than_its_queue() -> Outcome {
    let socket = socket_path("long");
    let mut host = host_for(&settings(&socket, 40, 16_777_216, None))?;
    let mut first = caller(&socket, &request_text(GENERATION, "--fmt", false))?;
    check_is(&served(&mut host)?, WorkerCommand::Format)?;
    sleep(Duration::from_millis(2000));
    complete(&mut host, &ActivationOutcome::Exited(0), -1, -1);
    assert_eq!(exit_of(&reply(&mut first)?), 0);
    let started = Instant::now();
    assert!(ended(&mut host));
    assert!(started.elapsed().as_millis() < 2000);
    assert!(withdrawn(&socket));
    Ok(())
}

#[test]
fn a_footprint_over_its_margin_for_two_activations_retires_the_worker() -> Outcome {
    let socket = socket_path("footprint");
    let mut host = host_for(&settings(&socket, 10000, 100, None))?;
    let mut warming = caller(&socket, &check_text(GENERATION))?;
    check_is(&served(&mut host)?, WorkerCommand::Check)?;
    complete(&mut host, &ActivationOutcome::Exited(0), 1000, -1);
    assert!(!retired(&reply(&mut warming)?));
    let mut first = caller(&socket, &check_text(GENERATION))?;
    check_is(&served(&mut host)?, WorkerCommand::Check)?;
    complete(&mut host, &ActivationOutcome::Exited(0), 1101, -1);
    assert!(!retired(&reply(&mut first)?));
    let mut light = caller(&socket, &request_text(GENERATION, "--fmt", false))?;
    check_is(&served(&mut host)?, WorkerCommand::Format)?;
    complete(&mut host, &ActivationOutcome::Exited(0), 1200, -1);
    assert!(!retired(&reply(&mut light)?));
    let mut second = caller(&socket, &check_text(GENERATION))?;
    let mut queued = caller(&socket, &request_text(GENERATION, "--fmt", false))?;
    check_is(&served(&mut host)?, WorkerCommand::Check)?;
    complete(&mut host, &ActivationOutcome::Exited(0), 1200, -1);
    let second_reply = reply(&mut second)?;
    assert!(exit_of(&second_reply) == 0 && retired(&second_reply));
    assert!(withdrawn(&socket));
    check_is(&served(&mut host)?, WorkerCommand::Format)?;
    complete(&mut host, &ActivationOutcome::Exited(0), 1200, -1);
    assert!(retired(&reply(&mut queued)?));
    assert!(ended(&mut host));
    Ok(())
}

#[test]
fn a_footprint_back_within_its_margin_keeps_the_worker() {
    let mut policy = lifecycle(Bounds::new(10000, 100, FOOTPRINT_CEILING));
    for footprint in [1000, 1101, 1050, 1101, -1, 1101] {
        assert_eq!(
            policy
                .settle(ActivationOutcome::Exited(0), footprint, false, -1)
                .retirement,
            None
        );
    }
    assert_eq!(
        policy.settle(ActivationOutcome::Exited(0), 1101, false, -1).retirement,
        Some(Retirement::Footprint)
    );
}

#[test]
fn freed_heap_an_allocator_keeps_does_not_retire_the_worker() {
    let mut policy = lifecycle(Bounds::new(10000, 100, 5000));
    for _round in 0..4 {
        assert_eq!(
            policy.settle(ActivationOutcome::Exited(0), 1500, false, 550).retirement,
            None
        );
    }
    assert_eq!(
        policy.settle(ActivationOutcome::Exited(0), 1500, false, 700).retirement,
        None
    );
    assert_eq!(
        policy.settle(ActivationOutcome::Exited(0), 1500, false, 700).retirement,
        Some(Retirement::Footprint)
    );
    let mut ceiling = lifecycle(Bounds::new(10000, 100, 5000));
    assert_eq!(
        ceiling
            .settle(ActivationOutcome::Exited(0), 5001, false, 550)
            .retirement,
        Some(Retirement::Footprint)
    );
}

#[test]
fn the_heap_a_worker_keeps_from_one_compile_to_the_next_does_not_retire_it() {
    let mut policy = lifecycle(Bounds::new(10000, FOOTPRINT_MARGIN, FOOTPRINT_CEILING));
    for _round in 0..8 {
        assert_eq!(
            policy
                .settle(ActivationOutcome::Exited(0), 24_822_408, false, 19_549_776)
                .retirement,
            None
        );
    }
    let grown = 19_549_776 + FOOTPRINT_MARGIN + 1;
    let footprint = 24_822_408 + FOOTPRINT_MARGIN;
    assert_eq!(
        policy
            .settle(ActivationOutcome::Exited(0), footprint, false, grown)
            .retirement,
        None
    );
    assert_eq!(
        policy
            .settle(ActivationOutcome::Exited(0), footprint, false, grown)
            .retirement,
        Some(Retirement::Footprint)
    );
}

#[test]
fn a_light_activation_neither_adds_to_nor_ends_a_run_of_excess() {
    let mut policy = lifecycle(Bounds::new(10000, 100, 5000));
    assert_eq!(
        policy.settle(ActivationOutcome::Exited(0), 1000, false, -1).retirement,
        None
    );
    assert_eq!(
        policy.settle(ActivationOutcome::Exited(0), 1101, false, -1).retirement,
        None
    );
    for footprint in [1101, 1050, -1] {
        assert_eq!(
            policy
                .settle(ActivationOutcome::Exited(0), footprint, true, -1)
                .retirement,
            None
        );
    }
    assert_eq!(
        policy.settle(ActivationOutcome::Exited(0), 1101, false, -1).retirement,
        Some(Retirement::Footprint)
    );
    let mut ceiling = lifecycle(Bounds::new(10000, 100, 5000));
    assert_eq!(
        ceiling.settle(ActivationOutcome::Exited(0), 5001, true, -1).retirement,
        Some(Retirement::Footprint)
    );
}

#[test]
fn a_footprint_over_the_ceiling_retires_the_worker_at_once() -> Outcome {
    let mut policy = lifecycle(Bounds::new(10000, FOOTPRINT_MARGIN, 5000));
    assert_eq!(
        policy.settle(ActivationOutcome::Exited(0), 5000, false, -1).retirement,
        None
    );
    assert_eq!(
        policy.settle(ActivationOutcome::Panicked, 5001, false, -1).retirement,
        Some(Retirement::Footprint)
    );
    let socket = socket_path("one-activation");
    let mut host = host_for(&settings(&socket, 10000, FOOTPRINT_MARGIN, None))?;
    let mut client = caller(&socket, &request_text(GENERATION, "--fmt", true))?;
    check_is(&served(&mut host)?, WorkerCommand::Format)?;
    complete(&mut host, &ActivationOutcome::Exited(0), -1, -1);
    let completed = reply(&mut client)?;
    assert!(exit_of(&completed) == 0 && retired(&completed));
    assert!(withdrawn(&socket));
    assert!(ended(&mut host));
    Ok(())
}

#[test]
fn a_footprint_over_the_ceiling_hands_rustc_to_the_host_and_retires() -> Outcome {
    let mut policy = lifecycle(Bounds::new(10000, FOOTPRINT_MARGIN, 5000));
    assert!(policy.hands_rustc_off(5001));
    let settled = policy.settle(
        ActivationOutcome::HandedOff("/p/out/libunit.rustc-handoff".into()),
        5001,
        false,
        -1,
    );
    assert_eq!(settled.retirement, Some(Retirement::Footprint));
    let socket = socket_path("handoff");
    let mut host = host_for(&settings(&socket, 10000, FOOTPRINT_MARGIN, None))?;
    let mut client = caller(&socket, &unit_text(GENERATION))?;
    let _ = served(&mut host)?;
    assert!(hands_rustc_off(&host, FOOTPRINT_CEILING + 1));
    complete(
        &mut host,
        &ActivationOutcome::HandedOff("/p/out/libunit.rustc-handoff".into()),
        FOOTPRINT_CEILING + 1,
        -1,
    );
    let answered = reply(&mut client)?;
    assert_eq!(handoff(&answered).as_deref(), Some("/p/out/libunit.rustc-handoff"));
    assert!(retired(&answered));
    assert!(withdrawn(&socket));
    assert!(ended(&mut host));
    Ok(())
}

#[test]
fn a_footprint_within_the_ceiling_keeps_rustc_in_the_warm_worker() -> Outcome {
    let policy = lifecycle(Bounds::new(10000, FOOTPRINT_MARGIN, 5000));
    assert!(!policy.hands_rustc_off(5000) && !policy.hands_rustc_off(-1));
    let socket = socket_path("in-worker");
    let mut host = host_for(&settings(&socket, 10000, FOOTPRINT_MARGIN, None))?;
    let mut client = caller(&socket, &unit_text(GENERATION))?;
    let _ = served(&mut host)?;
    assert!(!hands_rustc_off(&host, FOOTPRINT_CEILING));
    complete(&mut host, &ActivationOutcome::Exited(0), FOOTPRINT_CEILING, -1);
    let answered = reply(&mut client)?;
    assert!(exit_of(&answered) == 0 && !retired(&answered));
    assert_eq!(handoff(&answered), None);
    assert!(!withdrawn(&socket));
    discard(&socket);
    Ok(())
}

#[test]
fn a_compiler_panic_is_a_fault_and_the_next_request_is_served() -> Outcome {
    let socket = socket_path("panic");
    let mut host = host_for(&settings(&socket, 10000, 16_777_216, None))?;
    let mut panicked = caller(&socket, &request_text(GENERATION, "--fmt", false))?;
    let mut next = caller(&socket, &request_text(GENERATION, "--fmt", false))?;
    check_is(&served(&mut host)?, WorkerCommand::Format)?;
    complete(&mut host, &ActivationOutcome::Panicked, -1, -1);
    let fault = reply(&mut panicked)?;
    assert!(exit_of(&fault) == PANIC_STATUS && reason_of(&fault).is_empty() && !retired(&fault));
    check_is(&served(&mut host)?, WorkerCommand::Format)?;
    complete(&mut host, &ActivationOutcome::Exited(3), -1, -1);
    assert_eq!(exit_of(&reply(&mut next)?), 3);
    assert!(!withdrawn(&socket));
    discard(&socket);
    Ok(())
}

#[test]
fn a_request_of_another_generation_ends_the_worker() -> Outcome {
    let socket = socket_path("generation");
    let mut host = host_for(&settings(&socket, 10000, 16_777_216, None))?;
    let mut stranger = caller(&socket, &request_text("fedcba9876543210", "--fmt", false))?;
    assert!(ended(&mut host));
    let refusal = reply(&mut stranger)?;
    assert!(exit_of(&refusal) == 1 && reason_of(&refusal) == MISMATCH && retired(&refusal));
    assert!(withdrawn(&socket));
    Ok(())
}

#[test]
fn an_unrestored_process_serves_nothing_more() -> Outcome {
    let socket = socket_path("unsafe");
    let mut host = host_for(&settings(&socket, 10000, 16_777_216, None))?;
    let mut first = caller(&socket, &request_text(GENERATION, "--fmt", false))?;
    check_is(&served(&mut host)?, WorkerCommand::Format)?;
    complete(
        &mut host,
        &ActivationOutcome::UnsafeCleanup("compiler console restore failed: closed".into()),
        -1,
        -1,
    );
    let failed = reply(&mut first)?;
    assert!(exit_of(&failed) == 1 && reason_of(&failed) == "compiler console restore failed: closed");
    assert!(retired(&failed));
    assert!(withdrawn(&socket));
    assert!(ended(&mut host));
    let again = socket_path("unsafe-again");
    let mut second_host = host_for(&settings(&again, 10000, 16_777_216, None))?;
    let mut open = caller(&again, &request_text(GENERATION, "--fmt", false))?;
    check_is(&served(&mut second_host)?, WorkerCommand::Format)?;
    assert!(ended(&mut second_host));
    assert!(retired(&reply(&mut open)?));
    Ok(())
}

#[test]
fn current_compiler_and_record_activations_share_typed_socket_frames() -> Outcome {
    let generation = "cw3-0123456789abcdef";
    let socket = socket_path("typed");
    let mut configured = settings(&socket, 200, 128, None);
    configured.generation = generation.to_owned();
    let mut host = host_for(&configured)?;
    let mut compiling = dial(&socket)?;
    bound_reads(&mut compiling, 5);
    let arguments = vec!["--check".to_owned(), "/p/unit.json".to_owned(), "/p/sysroot".to_owned()];
    let compile_request = compile_frame(generation, &arguments, "/p", "/p/out", "/p/err", false)?;
    assert_eq!(&compile_request[..4], b"CRMB");
    send_frame(&mut compiling, &compile_request)?;
    let command = served(&mut host)?;
    assert!(command.cwd == "/p" && command.stdout == "/p/out");
    assert_eq!(command.arguments, vec!["/p/unit.json", "/p/sysroot"]);
    complete(&mut host, &ActivationOutcome::Exited(3), -1, -1);
    let result = receive_frame(&mut compiling)?;
    assert_eq!(&result[..4], b"CRMB");
    assert_eq!(read_completion(&result, generation)?.status, 3);
    assert!(read_completion(&result, "cw3-another").is_err());
    let mut preparing = dial(&socket)?;
    bound_reads(&mut preparing, 5);
    let request = RecordMessage::Prepare(Prepare {
        stage: "final".into(),
        snapshot: "s".into(),
        demand_root: "d".into(),
        producer_profile: "p".into(),
    });
    let packet = record_frame(&request, generation)?;
    assert_eq!(&packet[..4], b"CRMB");
    send_frame(&mut preparing, &packet)?;
    assert!(
        matches!(served_record(&mut host)?, RecordMessage::Prepare(value) if value.stage == "final" && value.snapshot == "s")
    );
    complete_record(
        &mut host,
        RecordMessage::Prepared(Prepared {
            prepared_root: "prepared".into(),
        }),
        -1,
        -1,
    );
    let answer = receive_frame(&mut preparing)?;
    assert!(
        matches!(decode_record_message(&answer, generation)?, RecordMessage::Prepared(value) if value.prepared_root == "prepared")
    );
    assert!(decode_record_message(&answer, "cw3-another").is_err());
    assert!(decode_record_message(&answer[..12], generation).is_err());
    discard(&socket);
    Ok(())
}
