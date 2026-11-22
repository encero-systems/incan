//! Typed IPC packets for a compiler worker generation with the version-three capability: one referenced Crumb row
//! per frame, its first field the worker generation.

use internal_models_bindings::{
    DescriptionMode, RowRecord, Value, ValueField, binary_message, read_record_at, row_count, row_descriptions,
};
use internal_models_native::crumb_rows::{
    ModelField, admit_rows, encode_rows, model_description, operation_budget, required_value,
};
use internal_models_native::graph_crumb_fields::text;

/// The worker generation prefix of the typed packet scheme.
pub const PREFIX: &str = "cw3-";
pub const COMPILE: &[u8] = b"wkcprq\x00";
pub const READY: &[u8] = b"wkrdy_\x00";
pub const COMPLETE: &[u8] = b"wkdone\x00";
pub const PREPARE: &[u8] = b"wkprep\x00";
pub const PREPARED: &[u8] = b"wkpred\x00";
pub const NEED: &[u8] = b"wkneed\x00";
pub const EVALUATE: &[u8] = b"wkeval\x00";
pub const PRODUCED: &[u8] = b"wkprod\x00";
pub const REFUSAL: &[u8] = b"wkrefu\x00";
pub const FAULT: &[u8] = b"wkfalt\x00";
/// The platform IPC frame limit.
pub const MAX_FRAME: i64 = 16_777_216;
const FAMILIES: [&[u8]; 10] = [
    COMPILE, READY, COMPLETE, PREPARE, PREPARED, NEED, EVALUATE, PRODUCED, REFUSAL, FAULT,
];

/// One admitted packet: its family key, its generation and its remaining fields in declaration order.
#[derive(Clone)]
pub struct Packet {
    pub key: Vec<u8>,
    pub generation: String,
    pub fields: Vec<ValueField>,
}

/// Whether a packet key is the expected family.
pub fn is_key(key: &[u8], expected: &[u8]) -> bool {
    key == expected
}

/// The name and fields of one packet family after its generation field, in declaration order.
fn family(key: &[u8]) -> Option<(&'static str, &'static [(&'static [u8], &'static str, &'static str)])> {
    let selected: (&str, &[(&[u8], &str, &str)]) = match key {
        COMPILE => (
            "compile",
            &[
                (b"argums", "arguments", "list<string>"),
                (b"cwd___", "cwd", "string"),
                (b"retire", "retire_after", "boolean"),
                (b"stderr", "stderr", "string"),
                (b"stdout", "stdout", "string"),
            ],
        ),
        READY => ("ready", &[(b"pid___", "pid", "i64")]),
        COMPLETE => (
            "complete",
            &[
                (b"exit__", "exit", "i64"),
                (b"handof", "handoff", "string"),
                (b"reason", "reason", "string"),
                (b"retire", "retired", "boolean"),
            ],
        ),
        PREPARE => (
            "prepare",
            &[
                (b"demand", "demand_root", "string"),
                (b"profil", "producer_profile", "string"),
                (b"snap__", "snapshot", "string"),
                (b"stage_", "stage", "string"),
            ],
        ),
        PREPARED => ("prepared", &[(b"prepar", "prepared_root", "string")]),
        NEED => (
            "need",
            &[(b"keys__", "keys_root", "string"), (b"rank__", "earlier_rank", "i64")],
        ),
        EVALUATE => (
            "evaluate",
            &[
                (b"cancel", "cancellation_generation", "i64"),
                (b"prepar", "prepared_root", "string"),
                (b"reserv", "reservation", "string"),
                (b"snap__", "snapshot", "string"),
                (b"work__", "work_root", "string"),
            ],
        ),
        PRODUCED => (
            "produced",
            &[
                (b"counts", "counter_root", "string"),
                (b"diag__", "diagnostic_root", "string"),
                (b"staged", "staged_root", "string"),
                (b"witnes", "witness_root", "string"),
            ],
        ),
        REFUSAL => (
            "refusal",
            &[
                (b"diag__", "diagnostic_root", "string"),
                (b"witnes", "witness_root", "string"),
            ],
        ),
        FAULT => ("fault", &[(b"fault_", "fault_root", "string")]),
        _ => return None,
    };
    Some(selected)
}

/// The referenced model description of one packet family.
fn description(key: &[u8]) -> Result<internal_models_bindings::Description, String> {
    let (name, declared) = family(key).ok_or("unknown compiler worker Crumb family")?;
    let mut fields = vec![ModelField::new(b"000gen", "generation", "string")];
    fields.extend(
        declared
            .iter()
            .map(|(identity, field, storage)| ModelField::new(identity, field, storage)),
    );
    Ok(model_description(
        key,
        &format!("incan.compiler-worker/3/{name}"),
        &fields,
    ))
}

/// Encode one packet of a version-three generation.
pub fn write_packet(key: &[u8], generation: &str, fields: Vec<ValueField>) -> Result<Vec<u8>, String> {
    if !generation.starts_with(PREFIX) {
        return Err("unsupported compiler worker wire generation".to_owned());
    }
    let mut values = vec![required_value(b"000gen", Value::Text(generation.to_owned()))];
    values.extend(fields);
    let row = RowRecord {
        key: key.to_vec(),
        value: Value::Record(values),
    };
    encode_rows(&description(key)?, vec![row], MAX_FRAME)
}

/// Admit one frame as exactly one referenced row of a known family and split off its generation.
pub fn read_packet(encoded: &[u8]) -> Result<Packet, String> {
    if encoded.len() as i64 > MAX_FRAME {
        return Err("compiler worker frame exceeds transport limit".to_owned());
    }
    let mut budget = operation_budget(MAX_FRAME * 4)?;
    let choices = FAMILIES
        .iter()
        .map(|key| description(key))
        .collect::<Result<Vec<_>, _>>()?;
    let rows = admit_rows(encoded, &choices, MAX_FRAME, &mut budget)?;
    let dictionary = row_descriptions(&rows, &mut budget).map_err(|error| binary_message(&error))?;
    if row_count(&rows) != 1 || dictionary.len() != 1 {
        return Err("compiler worker frame requires one referenced schema and row".to_owned());
    }
    if let DescriptionMode::Inline = dictionary[0].mode {
        return Err("inline compiler worker frame is unsupported".to_owned());
    }
    let row = read_record_at(&rows, 0, &mut budget).map_err(|error| binary_message(&error))?;
    match row.value {
        Value::Record(mut fields) => {
            if fields.is_empty() || fields[0].identity != b"000gen" {
                return Err("compiler worker packet has no generation".to_owned());
            }
            let generation = text(&fields[0].value)?;
            fields.remove(0);
            Ok(Packet {
                key: row.key,
                generation,
                fields,
            })
        }
        _ => Err("expected compiler worker record".to_owned()),
    }
}

/// The value of one named packet field.
pub fn value_of<'packet>(packet: &'packet Packet, identity: &[u8]) -> Result<&'packet Value, String> {
    packet
        .fields
        .iter()
        .find(|field| field.identity == identity)
        .map(|field| &field.value)
        .ok_or_else(|| "missing compiler worker packet field".to_owned())
}

/// Read one Boolean field.
pub fn flag(value: &Value) -> Result<bool, String> {
    match value {
        Value::Boolean(selected) => Ok(*selected),
        _ => Err("invalid compiler worker Boolean".to_owned()),
    }
}
