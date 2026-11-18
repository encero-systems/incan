//! Graph-local typed Crumb rows built through the existing native Crumb implementation.

#[path = "../../../../stdlib/primitives/rust/src/crumb/semantic_blocks.rs"]
mod blocks;
#[path = "../../../../stdlib/primitives/rust/src/crumb/semantic_columns.rs"]
mod columns;
#[path = "../../../../stdlib/primitives/rust/src/crumb/encoders.rs"]
mod encoders;
#[path = "../../../../stdlib/primitives/rust/src/crumb/semantic_envelope.rs"]
mod envelope;
#[path = "../../../../stdlib/primitives/rust/src/crumb/semantic_hash.rs"]
mod hash;
#[path = "../../../../stdlib/primitives/rust/src/crumb/semantic_row_cells.rs"]
mod row_cells;
#[path = "../../../../stdlib/primitives/rust/src/crumb/semantic_row_cursor.rs"]
mod row_cursor;
#[path = "../../../../stdlib/primitives/rust/src/crumb/semantic_row_write.rs"]
mod row_write;
#[path = "../../../../stdlib/primitives/rust/src/crumb/semantic_rows.rs"]
mod rows;

use columns::Budget;
use hash::Value;
use rows::{Meta, ReadInputs, Record, Rows, admit, get, read_rows};

pub use encoders::{Encoded, encode_integer_unsigned, encode_native_signed, encode_native_unsigned, frame};

const MAX_PAYLOAD_BYTES: usize = 67_108_864;
const PAYLOAD_BUDGET_BYTES: u64 = 268_435_456;

fn encode_payload_row_native(
    selector: Vec<u8>,
    model: String,
    kind: String,
    id: String,
    payload: Vec<u8>,
) -> Option<Vec<u8>> {
    if payload.len() > MAX_PAYLOAD_BYTES {
        return None;
    }
    let mut budget = Budget::new(PAYLOAD_BUDGET_BYTES, 1_048_576, 128);
    let description = admit(payload_description(selector, model), true, &[], &mut budget).ok()?;
    let value = Value::Record(vec![
        hash::Field {
            identity: b"kind__".to_vec(),
            canonical: true,
            absent: 'r',
            value: Value::Text(kind),
        },
        hash::Field {
            identity: b"id____".to_vec(),
            canonical: true,
            absent: 'r',
            value: Value::Text(id),
        },
        hash::Field {
            identity: b"value_".to_vec(),
            canonical: true,
            absent: 'r',
            value: Value::Binary(payload),
        },
    ]);
    row_write::write_rows(&Rows {
        dictionary: vec![description].into(),
        records: vec![Record {
            slot: 0,
            value,
            origin: 0,
            length: 0,
        }],
        sealed: false,
        consumed: 0,
    })
}

fn decode_payload_row_native(data: &[u8]) -> Option<(Vec<u8>, String, String, String, Vec<u8>)> {
    if data.len() > MAX_PAYLOAD_BYTES + 4096 {
        return None;
    }
    let mut budget = Budget::new(PAYLOAD_BUDGET_BYTES, 1_048_576, 128);
    let rows = read_rows(
        data,
        &ReadInputs {
            encoded_descriptions: None,
            supplied: &[],
            kinds: &[],
            require_sealed: false,
            exact: true,
        },
        &mut budget,
    )
    .ok()?;
    if rows.dictionary.len() != 1 || rows.records.len() != 1 {
        return None;
    }
    let selector = rows.dictionary[0].key.clone();
    let Meta::Text(model) = get(&rows.dictionary[0].metadata, "path")? else {
        return None;
    };
    let Value::Record(fields) = &rows.records[0].value else {
        return None;
    };
    let kind = payload_field(fields, b"kind__")?;
    let id = payload_field(fields, b"id____")?;
    let payload = payload_field(fields, b"value_")?;
    match (kind, id, payload) {
        (Value::Text(kind), Value::Text(id), Value::Binary(payload)) => {
            Some((selector, model.clone(), kind.clone(), id.clone(), payload.clone()))
        }
        _ => None,
    }
}

fn payload_field<'a>(fields: &'a [hash::Field], identity: &[u8]) -> Option<&'a Value> {
    fields
        .iter()
        .find(|field| field.identity == identity)
        .map(|field| &field.value)
}

fn payload_description(selector: Vec<u8>, model: String) -> Meta {
    Meta::Map(vec![
        ("kind".to_string(), Meta::Text("Record".to_string())),
        ("nominal".to_string(), Meta::Binary(selector)),
        ("path".to_string(), Meta::Text(model)),
        ("properties".to_string(), Meta::Map(Vec::new())),
        (
            "fields".to_string(),
            Meta::List(vec![
                payload_field_description(b"kind__", "kind", "string"),
                payload_field_description(b"id____", "id", "string"),
                payload_field_description(b"value_", "payload", "binary"),
            ]),
        ),
    ])
}

fn payload_field_description(identity: &[u8], name: &str, storage: &str) -> Meta {
    Meta::Map(vec![
        ("id".to_string(), Meta::Binary(identity.to_vec())),
        ("name".to_string(), Meta::Text(name.to_string())),
        ("type".to_string(), Meta::Text(storage.to_string())),
        ("canonical".to_string(), Meta::Boolean(true)),
        ("absent".to_string(), Meta::Text("required".to_string())),
        ("properties".to_string(), Meta::Map(Vec::new())),
    ])
}

pub(super) struct PayloadRow {
    pub(super) selector: Vec<u8>,
    pub(super) model: String,
    pub(super) kind: String,
    pub(super) id: String,
    pub(super) payload: Vec<u8>,
}

pub(super) fn encode_payload_row(
    selector: Vec<u8>,
    model: String,
    kind: String,
    id: String,
    payload: Vec<u8>,
) -> Option<Vec<u8>> {
    encode_payload_row_native(selector, model, kind, id, payload)
}

pub(super) fn decode_payload_row(data: &[u8]) -> Option<PayloadRow> {
    let (selector, model, kind, id, payload) = decode_payload_row_native(data)?;
    Some(PayloadRow {
        selector,
        model,
        kind,
        id,
        payload,
    })
}
