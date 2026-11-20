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

/// Write the bounded codegraph payload as one synthetic Crumb row.
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
            value: Value::Binary(payload.into()),
        },
    ]);
    row_write::write_rows(&Rows {
        dictionary: vec![description].into(),
        records: vec![Record {
            slot: 0,
            read_allowance: None,
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
        None,
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
    let record = rows.records.into_iter().next()?;
    let Value::Record(fields) = record.value else {
        return None;
    };
    let mut kind = None;
    let mut id = None;
    let mut payload = None;
    for field in fields {
        match (field.identity.as_slice(), field.value) {
            (b"kind__", Value::Text(value)) => kind = Some(value),
            (b"id____", Value::Text(value)) => id = Some(value),
            (b"value_", Value::Binary(hash::Binary::Owned(value))) => payload = Some(value),
            _ => return None,
        }
    }
    Some((selector, model.clone(), kind?, id?, payload?))
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
