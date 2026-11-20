//! Record activation messages of `incan.compiler-worker/2`, separate from the client engine protocol; a `cw3-`
//! generation carries them as typed packets.

use std::collections::BTreeMap;

use internal_models_bindings::Value;
use internal_models_native::crumb_rows::required_value;
use internal_models_native::graph_crumb_fields::{integer, number, text};

use crate::json_text::{Field, object, parse_document};
use crate::worker_packets::{
    COMPILE, COMPLETE, EVALUATE, FAULT, NEED, PREFIX, PREPARE, PREPARED, PRODUCED, Packet, READY, REFUSAL, is_key,
    read_packet, value_of, write_packet,
};

pub const RECORD_SCHEMA: &str = "incan.compiler-worker/2";
const MAX_RANK: i64 = 4_294_967_295;

/// The one closed worker record-message family.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecordMessageKind {
    Prepare,
    Prepared,
    NeedPrerequisite,
    Evaluate,
    Produced,
    SemanticRefusal,
    CompilerFault,
}

impl RecordMessageKind {
    const ALL: [RecordMessageKind; 7] = [
        RecordMessageKind::Prepare,
        RecordMessageKind::Prepared,
        RecordMessageKind::NeedPrerequisite,
        RecordMessageKind::Evaluate,
        RecordMessageKind::Produced,
        RecordMessageKind::SemanticRefusal,
        RecordMessageKind::CompilerFault,
    ];

    /// The kind's wire spelling.
    pub fn value(self) -> &'static str {
        match self {
            RecordMessageKind::Prepare => "prepare",
            RecordMessageKind::Prepared => "prepared",
            RecordMessageKind::NeedPrerequisite => "need-prerequisite",
            RecordMessageKind::Evaluate => "evaluate",
            RecordMessageKind::Produced => "produced",
            RecordMessageKind::SemanticRefusal => "semantic-refusal",
            RecordMessageKind::CompilerFault => "compiler-fault",
        }
    }

    /// The kind a wire spelling names.
    pub fn from_value(value: &str) -> Option<RecordMessageKind> {
        Self::ALL.into_iter().find(|kind| kind.value() == value)
    }
}

/// Compiler-owned preparation descriptors for one producer stage.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Prepare {
    pub stage: String,
    pub snapshot: String,
    pub demand_root: String,
    pub producer_profile: String,
}

/// The immutable prepared-input envelope the engine may schedule for evaluation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Prepared {
    pub prepared_root: String,
}

/// Earlier-rank scheduling input returned without evaluating a record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NeedPrerequisite {
    pub keys_root: String,
    pub earlier_rank: i64,
}

/// Prepared record work admitted under one resource reservation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Evaluate {
    pub work_root: String,
    pub snapshot: String,
    pub prepared_root: String,
    pub reservation: String,
    pub cancellation_generation: i64,
}

/// Staged record value and evidence roots returned for atomic publication.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Produced {
    pub staged_root: String,
    pub witness_root: String,
    pub diagnostic_root: String,
    pub counter_root: String,
}

/// Compiler-owned diagnostics and the complete read evidence supporting the refusal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SemanticRefusal {
    pub diagnostic_root: String,
    pub witness_root: String,
}

/// A typed compiler-service fault staged under one immutable root.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompilerFault {
    pub fault_root: String,
}

/// Every request and reply body of the record protocol.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RecordMessage {
    Prepare(Prepare),
    Prepared(Prepared),
    NeedPrerequisite(NeedPrerequisite),
    Evaluate(Evaluate),
    Produced(Produced),
    SemanticRefusal(SemanticRefusal),
    CompilerFault(CompilerFault),
}

/// Whether one received frame is a valid record request for this worker.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RecordAdmission {
    Admitted(RecordMessage),
    NotRecordProtocol,
    Incompatible,
    ProtocolFault,
}

/// Only Prepare and Evaluate are requests.
fn request_only(message: RecordMessage) -> RecordAdmission {
    match message {
        RecordMessage::Prepare(_) | RecordMessage::Evaluate(_) => RecordAdmission::Admitted(message),
        _ => RecordAdmission::ProtocolFault,
    }
}

/// Admit only Prepare or Evaluate for this generation; replies sent as requests are protocol faults.
pub fn admit_record(payload: &[u8], generation: &str) -> RecordAdmission {
    if generation.starts_with(PREFIX) {
        return match read_packet(payload) {
            Err(_) => RecordAdmission::NotRecordProtocol,
            Ok(packet)
                if is_key(&packet.key, COMPILE) || is_key(&packet.key, READY) || is_key(&packet.key, COMPLETE) =>
            {
                RecordAdmission::NotRecordProtocol
            }
            Ok(packet) if packet.generation != generation => RecordAdmission::Incompatible,
            Ok(packet) => match from_packet(&packet) {
                Ok(message) => request_only(message),
                Err(_) => RecordAdmission::ProtocolFault,
            },
        };
    }
    let Some(document) = std::str::from_utf8(payload)
        .ok()
        .and_then(|text| parse_document(text).ok())
    else {
        return RecordAdmission::NotRecordProtocol;
    };
    if document.get("schema").and_then(serde_json::Value::as_str) != Some(RECORD_SCHEMA) {
        return RecordAdmission::NotRecordProtocol;
    }
    if document.get("generation").and_then(serde_json::Value::as_str) != Some(generation) {
        return RecordAdmission::Incompatible;
    }
    match decode_record_message(payload, generation) {
        Ok(message) => request_only(message),
        Err(_) => RecordAdmission::ProtocolFault,
    }
}

/// The kind and fields of one message's JSON document.
fn message_fields(message: &RecordMessage) -> (RecordMessageKind, Vec<(&'static str, Field)>) {
    let text = |value: &String| Field::Text(value.clone());
    match message {
        RecordMessage::Prepare(value) => (
            RecordMessageKind::Prepare,
            vec![
                ("stage", text(&value.stage)),
                ("snapshot", text(&value.snapshot)),
                ("demand_root", text(&value.demand_root)),
                ("producer_profile", text(&value.producer_profile)),
            ],
        ),
        RecordMessage::Prepared(value) => (
            RecordMessageKind::Prepared,
            vec![("prepared_root", text(&value.prepared_root))],
        ),
        RecordMessage::NeedPrerequisite(value) => (
            RecordMessageKind::NeedPrerequisite,
            vec![
                ("keys_root", text(&value.keys_root)),
                ("earlier_rank", Field::Integer(value.earlier_rank)),
            ],
        ),
        RecordMessage::Evaluate(value) => (
            RecordMessageKind::Evaluate,
            vec![
                ("work_root", text(&value.work_root)),
                ("snapshot", text(&value.snapshot)),
                ("prepared_root", text(&value.prepared_root)),
                ("reservation", text(&value.reservation)),
                ("cancellation_generation", Field::Integer(value.cancellation_generation)),
            ],
        ),
        RecordMessage::Produced(value) => (
            RecordMessageKind::Produced,
            vec![
                ("staged_root", text(&value.staged_root)),
                ("witness_root", text(&value.witness_root)),
                ("diagnostic_root", text(&value.diagnostic_root)),
                ("counter_root", text(&value.counter_root)),
            ],
        ),
        RecordMessage::SemanticRefusal(value) => (
            RecordMessageKind::SemanticRefusal,
            vec![
                ("diagnostic_root", text(&value.diagnostic_root)),
                ("witness_root", text(&value.witness_root)),
            ],
        ),
        RecordMessage::CompilerFault(value) => (
            RecordMessageKind::CompilerFault,
            vec![("fault_root", text(&value.fault_root))],
        ),
    }
}

/// Encode one record activation with its worker generation and the version-two schema.
pub fn encode_record_message(message: &RecordMessage, generation: &str) -> Vec<u8> {
    let (kind, values) = message_fields(message);
    let mut fields: BTreeMap<&str, Field> = values.into_iter().collect();
    fields.insert("schema", Field::Text(RECORD_SCHEMA.to_owned()));
    fields.insert("generation", Field::Text(generation.to_owned()));
    fields.insert("kind", Field::Text(kind.value().to_owned()));
    object(fields).into_bytes()
}

/// Read one required string field without accepting another JSON kind.
fn required_text(document: &serde_json::Value, name: &str) -> Result<String, String> {
    match document.get(name) {
        Some(value) => value
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| format!("`{name}` must be text")),
        None => Err(format!("missing `{name}`")),
    }
}

/// Read one unsigned 32-bit protocol integer through the language's integer carrier.
fn bounded_integer(document: &serde_json::Value, name: &str) -> Result<i64, String> {
    match document.get(name) {
        Some(value) => match value.as_i64() {
            Some(found) if (0..=MAX_RANK).contains(&found) => Ok(found),
            Some(_) => Err(format!("`{name}` exceeds u32")),
            None => Err(format!("`{name}` must be an integer")),
        },
        None => Err(format!("missing `{name}`")),
    }
}

/// Decode only an exact record message of this worker generation.
pub fn decode_record_message(payload: &[u8], generation: &str) -> Result<RecordMessage, String> {
    if generation.starts_with(PREFIX) {
        let packet = read_packet(payload)?;
        if packet.generation != generation {
            return Err("compiler worker generation or record protocol mismatch".to_owned());
        }
        return from_packet(&packet);
    }
    let text = std::str::from_utf8(payload).map_err(|_| "the record frame is not UTF-8".to_owned())?;
    let document = parse_document(text)?;
    if required_text(&document, "schema")? != RECORD_SCHEMA || required_text(&document, "generation")? != generation {
        return Err("compiler worker generation or record protocol mismatch".to_owned());
    }
    match RecordMessageKind::from_value(&required_text(&document, "kind")?) {
        Some(kind) => document_message(kind, &document),
        None => Err("unknown compiler worker record message".to_owned()),
    }
}

/// The message of one known kind, its fields read in declaration order.
fn document_message(kind: RecordMessageKind, document: &serde_json::Value) -> Result<RecordMessage, String> {
    let text = |name: &str| required_text(document, name);
    Ok(match kind {
        RecordMessageKind::Prepare => RecordMessage::Prepare(Prepare {
            stage: text("stage")?,
            snapshot: text("snapshot")?,
            demand_root: text("demand_root")?,
            producer_profile: text("producer_profile")?,
        }),
        RecordMessageKind::Prepared => RecordMessage::Prepared(Prepared {
            prepared_root: text("prepared_root")?,
        }),
        RecordMessageKind::NeedPrerequisite => RecordMessage::NeedPrerequisite(NeedPrerequisite {
            keys_root: text("keys_root")?,
            earlier_rank: bounded_integer(document, "earlier_rank")?,
        }),
        RecordMessageKind::Evaluate => RecordMessage::Evaluate(Evaluate {
            work_root: text("work_root")?,
            snapshot: text("snapshot")?,
            prepared_root: text("prepared_root")?,
            reservation: text("reservation")?,
            cancellation_generation: bounded_integer(document, "cancellation_generation")?,
        }),
        RecordMessageKind::Produced => RecordMessage::Produced(Produced {
            staged_root: text("staged_root")?,
            witness_root: text("witness_root")?,
            diagnostic_root: text("diagnostic_root")?,
            counter_root: text("counter_root")?,
        }),
        RecordMessageKind::SemanticRefusal => RecordMessage::SemanticRefusal(SemanticRefusal {
            diagnostic_root: text("diagnostic_root")?,
            witness_root: text("witness_root")?,
        }),
        RecordMessageKind::CompilerFault => RecordMessage::CompilerFault(CompilerFault {
            fault_root: text("fault_root")?,
        }),
    })
}

/// Accept only the closed reply set of the exact request; every other pairing is a protocol fault.
pub fn accept_record_reply(request: &RecordMessage, reply: RecordMessage) -> Result<RecordMessage, String> {
    let accepted = match request {
        RecordMessage::Prepare(_) => matches!(
            reply,
            RecordMessage::Prepared(_)
                | RecordMessage::NeedPrerequisite(_)
                | RecordMessage::SemanticRefusal(_)
                | RecordMessage::CompilerFault(_)
        ),
        RecordMessage::Evaluate(_) => matches!(
            reply,
            RecordMessage::Produced(_) | RecordMessage::SemanticRefusal(_) | RecordMessage::CompilerFault(_)
        ),
        _ => false,
    };
    if !accepted {
        return Err("compiler worker record reply does not match its request".to_owned());
    }
    Ok(reply)
}

/// Write a record activation as a typed packet, or as the JSON document of an older installed peer.
pub fn record_frame(message: &RecordMessage, generation: &str) -> Result<Vec<u8>, String> {
    if !generation.starts_with(PREFIX) {
        return Ok(encode_record_message(message, generation));
    }
    let text = |value: &String| Value::Text(value.clone());
    let (key, fields) = match message {
        RecordMessage::Prepare(value) => (
            PREPARE,
            vec![
                required_value(b"demand", text(&value.demand_root)),
                required_value(b"profil", text(&value.producer_profile)),
                required_value(b"snap__", text(&value.snapshot)),
                required_value(b"stage_", text(&value.stage)),
            ],
        ),
        RecordMessage::Prepared(value) => (PREPARED, vec![required_value(b"prepar", text(&value.prepared_root))]),
        RecordMessage::NeedPrerequisite(value) => (
            NEED,
            vec![
                required_value(b"keys__", text(&value.keys_root)),
                required_value(b"rank__", integer(value.earlier_rank)),
            ],
        ),
        RecordMessage::Evaluate(value) => (
            EVALUATE,
            vec![
                required_value(b"cancel", integer(value.cancellation_generation)),
                required_value(b"prepar", text(&value.prepared_root)),
                required_value(b"reserv", text(&value.reservation)),
                required_value(b"snap__", text(&value.snapshot)),
                required_value(b"work__", text(&value.work_root)),
            ],
        ),
        RecordMessage::Produced(value) => (
            PRODUCED,
            vec![
                required_value(b"counts", text(&value.counter_root)),
                required_value(b"diag__", text(&value.diagnostic_root)),
                required_value(b"staged", text(&value.staged_root)),
                required_value(b"witnes", text(&value.witness_root)),
            ],
        ),
        RecordMessage::SemanticRefusal(value) => (
            REFUSAL,
            vec![
                required_value(b"diag__", text(&value.diagnostic_root)),
                required_value(b"witnes", text(&value.witness_root)),
            ],
        ),
        RecordMessage::CompilerFault(value) => (FAULT, vec![required_value(b"fault_", text(&value.fault_root))]),
    };
    write_packet(key, generation, fields)
}

/// Read an unsigned 32-bit packet integer.
fn rank(value: &Value) -> Result<i64, String> {
    let selected = number(value)?;
    if !(0..=MAX_RANK).contains(&selected) {
        return Err("compiler worker integer exceeds u32".to_owned());
    }
    Ok(selected)
}

/// The record message a typed packet carries.
fn from_packet(packet: &Packet) -> Result<RecordMessage, String> {
    let field = |identity: &[u8]| value_of(packet, identity).and_then(text);
    let key = packet.key.as_slice();
    if is_key(key, PREPARE) {
        return Ok(RecordMessage::Prepare(Prepare {
            demand_root: field(b"demand")?,
            producer_profile: field(b"profil")?,
            snapshot: field(b"snap__")?,
            stage: field(b"stage_")?,
        }));
    }
    if is_key(key, PREPARED) {
        return Ok(RecordMessage::Prepared(Prepared {
            prepared_root: field(b"prepar")?,
        }));
    }
    if is_key(key, NEED) {
        return Ok(RecordMessage::NeedPrerequisite(NeedPrerequisite {
            keys_root: field(b"keys__")?,
            earlier_rank: rank(value_of(packet, b"rank__")?)?,
        }));
    }
    if is_key(key, EVALUATE) {
        return Ok(RecordMessage::Evaluate(Evaluate {
            cancellation_generation: rank(value_of(packet, b"cancel")?)?,
            prepared_root: field(b"prepar")?,
            reservation: field(b"reserv")?,
            snapshot: field(b"snap__")?,
            work_root: field(b"work__")?,
        }));
    }
    if is_key(key, PRODUCED) {
        return Ok(RecordMessage::Produced(Produced {
            counter_root: field(b"counts")?,
            diagnostic_root: field(b"diag__")?,
            staged_root: field(b"staged")?,
            witness_root: field(b"witnes")?,
        }));
    }
    if is_key(key, REFUSAL) {
        return Ok(RecordMessage::SemanticRefusal(SemanticRefusal {
            diagnostic_root: field(b"diag__")?,
            witness_root: field(b"witnes")?,
        }));
    }
    if is_key(key, FAULT) {
        return Ok(RecordMessage::CompilerFault(CompilerFault {
            fault_root: field(b"fault_")?,
        }));
    }
    Err("unexpected compiler worker record packet".to_owned())
}
