//! The `incan.compiler-worker/1` compile protocol: one document per frame naming the compiler command, its arguments,
//! the caller's working directory and console files; the reply's exit status; the readiness document. A `cw3-`
//! generation carries the same requests and replies as typed packets.

use std::collections::BTreeMap;

use internal_models_bindings::Value;
use internal_models_native::crumb_rows::required_value;
use internal_models_native::graph_crumb_fields::{integer, items, number, text as field_text};

pub use crate::json_text::parse_document;
use crate::json_text::{Field, object, require_text, text};
use crate::worker_packets::{
    COMPILE, COMPLETE, PREFIX, Packet, READY, flag, is_key, read_packet, value_of, write_packet,
};

pub const SCHEMA: &str = "incan.compiler-worker/1";
/// The status Rust gives a process whose main thread panics; a request whose compilation panicked reports it.
pub const PANIC_STATUS: i64 = 101;
const MAX_STATUS: i64 = 255;
pub const MISMATCH: &str = "compiler worker generation or protocol mismatch";
const UNSERVED: &str =
    "a compiler worker accepts only --unit, --check, --fmt, --source-files and --rust-imports requests";

/// The compiler commands a worker serves, each the entry point its short-lived command runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkerCommand {
    /// `--unit UNIT CRATE OUTPUT SYSROOT ...`: compile a unit.
    Unit,
    /// `--check UNIT SYSROOT ...`: check a unit.
    Check,
    /// `--fmt UNIT PATH ...`: format sources with the unit's vocabulary.
    Format,
    /// `--source-files PATH`: list the authored sources a formatting run selects.
    SourceFiles,
    /// `--rust-imports UNIT`: list the Rust crate requests of a unit's authored source graph.
    RustImports,
}

impl WorkerCommand {
    /// Whether the command only reads sources, checking and linking nothing.
    pub fn light(self) -> bool {
        !matches!(self, WorkerCommand::Unit | WorkerCommand::Check)
    }
}

/// One admitted compiler invocation, as the compiler entry runs it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompileRequest {
    pub command: WorkerCommand,
    /// The command's arguments after its command flag.
    pub arguments: Vec<String>,
    /// The files that receive the command's standard output and error, and the directory it runs in; all absolute.
    pub stdout: String,
    pub stderr: String,
    pub cwd: String,
    /// Require process teardown after this activation.
    pub retire_after: bool,
}

/// Why an activation ended without the command's own status.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Fault {
    /// The compiler panicked; its entry contained the panic and restored its own state.
    Panicked,
    /// The process could not be restored after the activation, so it cannot serve another.
    UnsafeCleanup(String),
}

/// The answer to one request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reply {
    Completed(i64),
    Refused(String),
    CompilerFault(Fault),
    HandedOff(String),
}

/// What a received document is to this worker.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Admission {
    Admitted(CompileRequest),
    Refused(String),
    /// A request of another protocol or compiler generation, which this worker never serves.
    Incompatible,
}

/// The request a frame's payload carries, admitted for this worker generation.
pub fn admit(payload: &[u8], generation: &str) -> Admission {
    if generation.starts_with(PREFIX) {
        return match read_packet(payload) {
            Err(reason) => Admission::Refused(reason),
            Ok(packet) if !is_key(&packet.key, COMPILE) || packet.generation != generation => Admission::Incompatible,
            Ok(packet) => match compile_packet(&packet) {
                Ok(request) => Admission::Admitted(request),
                Err(reason) => Admission::Refused(reason),
            },
        };
    }
    match decoded(payload) {
        Err(reason) => Admission::Refused(reason),
        Ok(found) => {
            if !text_is(&found, "schema", SCHEMA) || !text_is(&found, "generation", generation) {
                return Admission::Incompatible;
            }
            match request_of(&found) {
                Ok(request) => Admission::Admitted(request),
                Err(reason) => Admission::Refused(reason),
            }
        }
    }
}

/// One UTF-8 JSON document.
fn decoded(payload: &[u8]) -> Result<serde_json::Value, String> {
    let text = std::str::from_utf8(payload).map_err(|_| "the frame is not UTF-8".to_owned())?;
    parse_document(text)
}

/// The request a compatible document describes: the command and its arguments first, then the console files and
/// working directory, all absolute.
fn request_of(document: &serde_json::Value) -> Result<CompileRequest, String> {
    let arguments = strings(document, "arguments")?;
    let command = command_of(&arguments)?;
    Ok(CompileRequest {
        command,
        stdout: absolute_field(document, "stdout")?,
        stderr: absolute_field(document, "stderr")?,
        cwd: absolute_field(document, "cwd")?,
        retire_after: retirement_requested(document)?,
        arguments: arguments[1..].to_vec(),
    })
}

/// The optional activation-lifetime contract; older callers keep the reusable-worker policy.
fn retirement_requested(document: &serde_json::Value) -> Result<bool, String> {
    match document.get("retire_after") {
        None => Ok(false),
        Some(value) => value.as_bool().ok_or_else(|| "retire_after must be Boolean".to_owned()),
    }
}

/// The served command of an argument list whose required paths are absolute.
fn command_of(arguments: &[String]) -> Result<WorkerCommand, String> {
    let Some(flag) = arguments.first() else {
        return Err(UNSERVED.to_owned());
    };
    let (command, minimum, absolute): (WorkerCommand, usize, &[usize]) = match flag.as_str() {
        "--unit" => (WorkerCommand::Unit, 5, &[1, 3, 4]),
        "--check" => (WorkerCommand::Check, 3, &[1, 2]),
        "--fmt" => (WorkerCommand::Format, 3, &[1]),
        "--source-files" => (WorkerCommand::SourceFiles, 2, &[]),
        "--rust-imports" => (WorkerCommand::RustImports, 2, &[1]),
        _ => return Err(UNSERVED.to_owned()),
    };
    if arguments.len() < minimum {
        return Err(format!("a compiler worker {flag} request is incomplete"));
    }
    if absolute.iter().any(|index| !arguments[*index].starts_with('/')) {
        return Err("compiler worker unit, output and sysroot paths must be absolute".to_owned());
    }
    Ok(command)
}

/// A required absolute path field.
fn absolute_field(document: &serde_json::Value, name: &str) -> Result<String, String> {
    match text(document, name) {
        None => Err(format!("missing `{name}`")),
        Some(path) if !path.starts_with('/') => Err(format!("compiler worker `{name}` path must be absolute")),
        Some(path) => Ok(path),
    }
}

/// Whether the string field `name` is exactly `expected`.
fn text_is(document: &serde_json::Value, name: &str, expected: &str) -> bool {
    text(document, name).is_some_and(|found| found == expected)
}

/// The required array of strings `name`.
fn strings(document: &serde_json::Value, name: &str) -> Result<Vec<String>, String> {
    let Some(value) = document.get(name) else {
        return Err(format!("missing `{name}`"));
    };
    let invalid = || format!("`{name}` is not an array of strings");
    let items = value.as_array().ok_or_else(invalid)?;
    items
        .iter()
        .map(|item| item.as_str().map(str::to_owned).ok_or_else(invalid))
        .collect()
}

/// A reply's JSON document: the exit status, the reason it did not run, the retirement mark and any handoff.
pub fn encode_reply(reply: &Reply, retired: bool) -> Vec<u8> {
    let (exit, reason, rustc_handoff) = reply_parts(reply);
    let mut fields = BTreeMap::new();
    fields.insert("schema", Field::Text(SCHEMA.to_owned()));
    fields.insert("exit", Field::Integer(exit));
    fields.insert("reason", Field::Text(reason));
    if !rustc_handoff.is_empty() {
        fields.insert("handoff", Field::Text(rustc_handoff));
    }
    if retired {
        fields.insert("retired", Field::Boolean(true));
    }
    object(fields).into_bytes()
}

/// The exit status, reason and handoff path a reply carries.
fn reply_parts(reply: &Reply) -> (i64, String, String) {
    match reply {
        Reply::Completed(code) => (*code, String::new(), String::new()),
        Reply::Refused(why) => (1, why.clone(), String::new()),
        Reply::CompilerFault(Fault::Panicked) => (PANIC_STATUS, String::new(), String::new()),
        Reply::CompilerFault(Fault::UnsafeCleanup(why)) => (1, why.clone(), String::new()),
        Reply::HandedOff(path) => (0, String::new(), path.clone()),
    }
}

/// The readiness document a started worker sends the process that started it.
pub fn encode_ready(generation: &str, pid: i64) -> Vec<u8> {
    let mut fields = BTreeMap::new();
    fields.insert("schema", Field::Text(SCHEMA.to_owned()));
    fields.insert("generation", Field::Text(generation.to_owned()));
    fields.insert("status", Field::Text("ready".to_owned()));
    fields.insert("pid", Field::Integer(pid));
    object(fields).into_bytes()
}

/// The compiler invocation plus its owner's working directory, console sinks and, only when teardown is required,
/// its activation lifetime.
pub fn request_document(
    generation: &str,
    arguments: &[String],
    directory: &str,
    stdout: &str,
    stderr: &str,
    retire_after: bool,
) -> String {
    let mut fields = BTreeMap::new();
    fields.insert("schema", Field::Text(SCHEMA.to_owned()));
    fields.insert("generation", Field::Text(generation.to_owned()));
    fields.insert("arguments", Field::Texts(arguments.to_vec()));
    fields.insert("cwd", Field::Text(directory.to_owned()));
    fields.insert("stdout", Field::Text(stdout.to_owned()));
    fields.insert("stderr", Field::Text(stderr.to_owned()));
    if retire_after {
        fields.insert("retire_after", Field::Boolean(true));
    }
    object(fields)
}

/// The exit status a reply reports; malformed replies and transport failures authorize no cold replay.
pub fn completion(document: &serde_json::Value) -> Result<i64, String> {
    if require_text(document, "schema")? != SCHEMA {
        return Err("compiler worker completion protocol mismatch".to_owned());
    }
    if let Some(reason) = document.get("reason").and_then(serde_json::Value::as_str)
        && !reason.is_empty()
    {
        return Err(reason.to_owned());
    }
    match document.get("exit").and_then(serde_json::Value::as_i64) {
        Some(code) if (0..=MAX_STATUS).contains(&code) => Ok(code),
        _ => Err("compiler worker returned no valid completion code".to_owned()),
    }
}

/// Whether the worker exits after this reply.
pub fn retired(document: &serde_json::Value) -> bool {
    document
        .get("retired")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

/// The rustc handoff a worker over its footprint ceiling wrote instead of running rustc.
pub fn handoff(document: &serde_json::Value) -> Option<String> {
    text(document, "handoff").filter(|path| !path.is_empty())
}

/// A required integer field of a worker reply.
fn integer_field(document: &serde_json::Value, name: &str) -> Result<i64, String> {
    document
        .get(name)
        .and_then(serde_json::Value::as_i64)
        .ok_or_else(|| format!("compiler worker reply has no integer `{name}`"))
}

/// Require the started worker's protocol, generation, PID and readiness acknowledgment.
pub fn ready(document: &serde_json::Value, generation: &str, pid: i64) -> Result<(), String> {
    if require_text(document, "schema")? != SCHEMA {
        return Err("compiler worker readiness protocol mismatch".to_owned());
    }
    if require_text(document, "generation")? != generation {
        return Err("compiler worker readiness generation mismatch".to_owned());
    }
    if integer_field(document, "pid")? != pid {
        return Err("compiler worker readiness PID mismatch".to_owned());
    }
    if require_text(document, "status")? != "ready" {
        return Err("compiler worker did not acknowledge readiness".to_owned());
    }
    Ok(())
}

/// Carry one compiler invocation in the format its installed worker advertises.
pub fn compile_frame(
    generation: &str,
    arguments: &[String],
    directory: &str,
    stdout: &str,
    stderr: &str,
    retire_after: bool,
) -> Result<Vec<u8>, String> {
    if !generation.starts_with(PREFIX) {
        return Ok(request_document(generation, arguments, directory, stdout, stderr, retire_after).into_bytes());
    }
    let listed = arguments.iter().map(|argument| Value::Text(argument.clone())).collect();
    write_packet(
        COMPILE,
        generation,
        vec![
            required_value(b"argums", Value::List(listed)),
            required_value(b"cwd___", Value::Text(directory.to_owned())),
            required_value(b"retire", Value::Boolean(retire_after)),
            required_value(b"stderr", Value::Text(stderr.to_owned())),
            required_value(b"stdout", Value::Text(stdout.to_owned())),
        ],
    )
}

/// The request a typed compile packet carries.
fn compile_packet(packet: &Packet) -> Result<CompileRequest, String> {
    let arguments = items(value_of(packet, b"argums")?)?
        .iter()
        .map(field_text)
        .collect::<Result<Vec<_>, _>>()?;
    let command = command_of(&arguments)?;
    let cwd = field_text(value_of(packet, b"cwd___")?)?;
    let stdout = field_text(value_of(packet, b"stdout")?)?;
    let stderr = field_text(value_of(packet, b"stderr")?)?;
    if !cwd.starts_with('/') || !stdout.starts_with('/') || !stderr.starts_with('/') {
        return Err("compiler worker console paths and working directory must be absolute".to_owned());
    }
    Ok(CompileRequest {
        command,
        arguments: arguments[1..].to_vec(),
        cwd,
        stdout,
        stderr,
        retire_after: flag(value_of(packet, b"retire")?)?,
    })
}

/// A decoded completion of either wire generation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkerCompletion {
    pub status: i64,
    pub reason: String,
    pub handoff: String,
    pub retired: bool,
}

/// Write a reply in the generation's format.
pub fn reply_frame(reply: &Reply, retired: bool, generation: &str) -> Result<Vec<u8>, String> {
    if !generation.starts_with(PREFIX) {
        return Ok(encode_reply(reply, retired));
    }
    let (status, reason, path) = reply_parts(reply);
    write_packet(
        COMPLETE,
        generation,
        vec![
            required_value(b"exit__", integer(status)),
            required_value(b"handof", Value::Text(path)),
            required_value(b"reason", Value::Text(reason)),
            required_value(b"retire", Value::Boolean(retired)),
        ],
    )
}

/// Read a completion in the generation's format.
pub fn read_completion(payload: &[u8], generation: &str) -> Result<WorkerCompletion, String> {
    if !generation.starts_with(PREFIX) {
        let document = decoded(payload)?;
        return Ok(WorkerCompletion {
            status: completion(&document)?,
            reason: String::new(),
            handoff: handoff(&document).unwrap_or_default(),
            retired: retired(&document),
        });
    }
    let packet = read_packet(payload)?;
    if !is_key(&packet.key, COMPLETE) || packet.generation != generation {
        return Err("compiler worker completion protocol mismatch".to_owned());
    }
    let status = number(value_of(&packet, b"exit__")?)?;
    if !(0..=MAX_STATUS).contains(&status) {
        return Err("compiler worker returned no valid completion code".to_owned());
    }
    Ok(WorkerCompletion {
        status,
        reason: field_text(value_of(&packet, b"reason")?)?,
        handoff: field_text(value_of(&packet, b"handof")?)?,
        retired: flag(value_of(&packet, b"retire")?)?,
    })
}

/// Write the readiness acknowledgment in the generation's format.
pub fn ready_frame(generation: &str, pid: i64) -> Result<Vec<u8>, String> {
    if !generation.starts_with(PREFIX) {
        return Ok(encode_ready(generation, pid));
    }
    write_packet(READY, generation, vec![required_value(b"pid___", integer(pid))])
}

/// Require a readiness acknowledgment of this generation and PID.
pub fn read_ready(payload: &[u8], generation: &str, pid: i64) -> Result<(), String> {
    if !generation.starts_with(PREFIX) {
        return ready(&decoded(payload)?, generation, pid);
    }
    let packet = read_packet(payload)?;
    if !is_key(&packet.key, READY) || packet.generation != generation || number(value_of(&packet, b"pid___")?)? != pid {
        return Err("compiler worker readiness protocol, generation or PID mismatch".to_owned());
    }
    Ok(())
}
