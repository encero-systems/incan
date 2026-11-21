//! Typed completed-read Crumb rows shared by compiler workers and the engine graph publisher.

use crate::crumb_rows::{ModelField, model_description, required_value};
use crate::graph_crumb_fields::{
    KEY_TYPE, REQUEST_TYPE, decode_graph_row, encode_graph_row, integer, items, key_value, number, read_key,
    read_request, request_value, text, tuple_items,
};
use crate::record_scope::{
    AbsentFact, DeletedFact, PreparationEvidence, READ_SCHEMA, ReadReceipt, ReadScope, ReadWitness, RecordRequest,
    RevisionEvidence, valid_request,
};
use crate::store_record_source::encode_record_key;
use internal_models_bindings::{Description, Value};

pub const RECEIPT_SCHEMA: &str = "incan.engine.read-receipt/3";
const SELECTOR: &[u8] = b"readrc\x00";

/// Each receipt commits the selected scope, producing activation, exact reads and measured work.
fn description() -> Description {
    let witnesses = format!("list<tuple<string,{REQUEST_TYPE},list<string>>>");
    model_description(
        SELECTOR,
        RECEIPT_SCHEMA,
        &[
            ModelField::new(b"budget", "response_bytes", "i64"),
            ModelField::new(b"count_", "operations", "i64"),
            ModelField::new(b"output", "output", KEY_TYPE),
            ModelField::new(b"profil", "profile", "string"),
            ModelField::new(b"rank__", "rank_certificate", "string"),
            ModelField::new(b"schema", "read_schema", "string"),
            ModelField::new(b"select", "selector_witnesses", "string"),
            ModelField::new(b"snapsh", "snapshot", "string"),
            ModelField::new(b"witnes", "witnesses", &witnesses),
        ],
    )
}

/// Persist every completed witness, including a valid zero-read activation.
pub fn encode_read_receipt(receipt: &ReadReceipt) -> Result<Vec<u8>, String> {
    encode_record_key(&receipt.preparation.output)?;
    let witnesses = receipt
        .witnesses
        .iter()
        .map(|witness| witness_value(witness, &receipt.scope))
        .collect::<Result<Vec<_>, _>>()?;
    encode_graph_row(
        SELECTOR,
        &description(),
        vec![
            required_value(b"budget", integer(receipt.response_bytes)),
            required_value(b"count_", integer(receipt.operations)),
            required_value(b"output", key_value(&receipt.preparation.output)),
            required_value(b"profil", Value::Text(receipt.scope.profile.clone())),
            required_value(b"rank__", Value::Text(receipt.preparation.rank_certificate.clone())),
            required_value(b"schema", Value::Text(receipt.schema.clone())),
            required_value(b"select", Value::Text(receipt.preparation.selector_witnesses.clone())),
            required_value(b"snapsh", Value::Text(receipt.scope.snapshot.clone())),
            required_value(b"witnes", Value::List(witnesses)),
        ],
    )
}

/// Decode exactly one receipt and verify the domain constraints on its scope, counters and read requests.
pub fn decode_read_receipt(encoded: &[u8]) -> Result<ReadReceipt, String> {
    let fields = decode_graph_row(encoded, SELECTOR, &description(), 9)?;
    if text(&fields[5].value)? != READ_SCHEMA {
        return Err("unsupported compiler read receipt schema".into());
    }
    let scope = ReadScope {
        snapshot: text(&fields[7].value)?,
        profile: text(&fields[3].value)?,
    };
    let operations = number(&fields[1].value)?;
    let response_bytes = number(&fields[0].value)?;
    if scope.snapshot.is_empty() || scope.profile.is_empty() || operations < 0 || response_bytes < 0 {
        return Err("compiler read receipt has incomplete scope or counters".into());
    }
    let witnesses = items(&fields[8].value)?
        .iter()
        .map(|witness| read_witness(witness, &scope))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(ReadReceipt {
        schema: READ_SCHEMA.to_owned(),
        preparation: PreparationEvidence {
            output: read_key(&fields[2].value)?,
            rank_certificate: text(&fields[4].value)?,
            selector_witnesses: text(&fields[6].value)?,
        },
        scope,
        witnesses,
        operations,
        response_bytes,
    })
}

/// Keep each evidence component independent of its contents and bind the witness to the receipt scope.
fn witness_value(witness: &ReadWitness, scope: &ReadScope) -> Result<Value, String> {
    match witness {
        ReadWitness::Found(found, selection) => edge(
            "present",
            scope,
            &found.scope,
            &found.request,
            &[&found.revision, &found.semantic_digest, selection],
        ),
        ReadWitness::Deleted(deleted) => edge("deleted", scope, &deleted.scope, &deleted.request, &[&deleted.revision]),
        ReadWitness::Absent(absent) => edge(
            "absent",
            scope,
            &absent.scope,
            &absent.request,
            &[&absent.exclusion_digest],
        ),
        ReadWitness::Members(observed, request, membership, continuation) => {
            edge("members", scope, observed, request, &[membership, continuation])
        }
    }
}

/// A witness always belongs to the leased snapshot and profile of the completed reader.
fn edge(
    kind: &str,
    scope: &ReadScope,
    observed: &ReadScope,
    request: &RecordRequest,
    evidence: &[&String],
) -> Result<Value, String> {
    encode_record_key(&request.key)?;
    if observed != scope || !valid_request(request) || evidence[0].is_empty() {
        return Err("compiler read witness lacks matching scope, request or evidence".into());
    }
    Ok(Value::Tuple(vec![
        Value::Text(kind.to_owned()),
        request_value(request),
        Value::List(evidence.iter().map(|part| Value::Text((*part).clone())).collect()),
    ]))
}

/// Interpret each witness variant using its own exact evidence fields.
fn read_witness(value: &Value, scope: &ReadScope) -> Result<ReadWitness, String> {
    let fields = tuple_items(value, 3)?;
    let kind = text(&fields[0])?;
    let request = read_request(&fields[1])?;
    encode_record_key(&request.key)?;
    let evidence = items(&fields[2])?;
    if !valid_request(&request) || evidence.is_empty() || text(&evidence[0])?.is_empty() {
        return Err("compiler read witness has an invalid request or empty evidence".into());
    }
    let first = text(&evidence[0])?;
    match (kind.as_str(), evidence.len()) {
        ("present", 3) => Ok(ReadWitness::Found(
            RevisionEvidence {
                scope: scope.clone(),
                request,
                revision: first,
                semantic_digest: text(&evidence[1])?,
            },
            text(&evidence[2])?,
        )),
        ("deleted", 1) => Ok(ReadWitness::Deleted(DeletedFact {
            scope: scope.clone(),
            request,
            revision: first,
        })),
        ("absent", 1) => Ok(ReadWitness::Absent(AbsentFact {
            scope: scope.clone(),
            request,
            exclusion_digest: first,
        })),
        ("members", 2) => Ok(ReadWitness::Members(scope.clone(), request, first, text(&evidence[1])?)),
        _ => Err("unknown or malformed compiler read witness".into()),
    }
}
