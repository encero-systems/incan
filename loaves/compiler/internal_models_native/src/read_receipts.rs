//! Canonical completed-read receipt bytes and strict witness restoration.

use crate::activation_envelopes::{document, encode, integer, items, read_selection, required, selection_value, text};
use crate::record_scope::{
    AbsentFact, DeletedFact, PreparationEvidence, READ_SCHEMA, ReadReceipt, ReadScope, ReadWitness, RecordKey,
    RecordRequest, RevisionEvidence, valid_key, valid_request,
};
use crate::store_record_source::encode_record_key;
use serde_json::{Value, json};
use std::borrow::Cow;
pub const RECEIPT_SCHEMA: &str = "incan.engine.read-receipt/2";

/// Encode all witnessed reads in observed order, retaining counters for failed source calls.
pub fn encode_read_receipt(receipt: &ReadReceipt) -> Result<Vec<u8>, String> {
    let witnesses = receipt.witnesses.iter().map(edge).collect::<Result<Vec<_>, _>>()?;
    encode(
        json!({"schema":RECEIPT_SCHEMA,"read_schema":receipt.schema,"snapshot":receipt.scope.snapshot,"profile":receipt.scope.profile,
        "output":encode_record_key(&receipt.preparation.output)?,"rank_certificate":receipt.preparation.rank_certificate,"selector_witnesses":receipt.preparation.selector_witnesses,
        "operations":receipt.operations,"response_bytes":receipt.response_bytes,"witnesses":witnesses}),
    )
}
/// Encode the underlying evidence before its canonical key; bare absence and deletion have different meanings.
fn edge(w: &ReadWitness) -> Result<Value, String> {
    let (kind, request, evidence): (&str, &RecordRequest, Cow<'_, str>) = match w {
        ReadWitness::Found(f, selection) => (
            "present",
            &f.request,
            format!("{}\n{}\n{}", f.revision, f.semantic_digest, selection).into(),
        ),
        ReadWitness::Absent(f) => ("absent", &f.request, f.exclusion_digest.as_str().into()),
        ReadWitness::Deleted(f) => ("deleted", &f.request, f.revision.as_str().into()),
        ReadWitness::Members(_, r, m, c) => ("members", r, format!("{m}\n{c}").into()),
    };
    if evidence.is_empty() {
        return Err("a graph edge requires witness evidence".to_owned());
    }
    let mut selection = selection_value(&request.selection);
    selection["byte_limit"] = Value::from(request.byte_limit);
    Ok(json!({"kind":kind,"target":encode_record_key(&request.key)?,"selection":selection,"evidence":evidence}))
}
/// Decode the exact six fields separated by tab characters.
fn read_key(encoded: &str) -> Result<RecordKey, String> {
    let mut parts = encoded.split('\t');
    let key = match (
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
    ) {
        (Some(family), Some(profile), Some(origin), Some(id), Some(facet), Some(context), None) => RecordKey {
            family: family.to_owned(),
            identity_profile: profile.to_owned(),
            origin: origin.to_owned(),
            identity: id.to_owned(),
            facet: facet.to_owned(),
            context: context.to_owned(),
        },
        _ => return Err("compiler read witness has a malformed target key".to_owned()),
    };
    if !valid_key(&key) {
        return Err("compiler read witness target has incomplete authority".to_owned());
    }
    Ok(key)
}
/// Restore a witness with the enclosing receipt's snapshot and profile.
fn read_witness(v: &Value, scope: &ReadScope) -> Result<ReadWitness, String> {
    let target = text(v, "target")?;
    let raw = required(v, "selection")?;
    let selection = read_selection(raw, "unknown compiler read selection")?;
    let byte_limit = integer(raw, "byte_limit")?;
    let request = RecordRequest {
        key: read_key(target)?,
        selection,
        byte_limit,
    };
    if !valid_request(&request) {
        return Err("compiler read witness has an invalid request".to_owned());
    }
    let evidence = text(v, "evidence")?;
    // The output's witnesses each own their scope independently of the enclosing receipt.
    let scope = scope.clone();
    match text(v, "kind")? {
        "present" => {
            let mut parts = evidence.split('\n');
            match (parts.next(), parts.next(), parts.next(), parts.next()) {
                (Some(rev), Some(semantic), Some(selection), None) => Ok(ReadWitness::Found(
                    RevisionEvidence {
                        scope,
                        request,
                        revision: rev.to_owned(),
                        semantic_digest: semantic.to_owned(),
                    },
                    selection.to_owned(),
                )),
                _ => Err("present witness evidence is malformed".to_owned()),
            }
        }
        "deleted" => Ok(ReadWitness::Deleted(DeletedFact {
            scope,
            request,
            revision: evidence.to_owned(),
        })),
        "absent" => Ok(ReadWitness::Absent(AbsentFact {
            scope,
            request,
            exclusion_digest: evidence.to_owned(),
        })),
        "members" => {
            let mut parts = evidence.split('\n');
            match (parts.next(), parts.next(), parts.next()) {
                (Some(digest), Some(continuation), None) => Ok(ReadWitness::Members(
                    scope,
                    request,
                    digest.to_owned(),
                    continuation.to_owned(),
                )),
                _ => Err("member witness evidence is malformed".to_owned()),
            }
        }
        _ => Err("unknown compiler read witness kind".to_owned()),
    }
}
/// Decode in the source's diagnostic order, preserving the distinction between absent evidence and missing schema
/// fields.
pub fn decode_read_receipt(bytes: &[u8]) -> Result<ReadReceipt, String> {
    let v = document(bytes, "compiler read receipt is not UTF-8")?;
    if text(&v, "schema")? != RECEIPT_SCHEMA || text(&v, "read_schema")? != READ_SCHEMA {
        return Err("unsupported compiler read receipt schema".to_owned());
    }
    let scope = ReadScope {
        snapshot: text(&v, "snapshot")?.to_owned(),
        profile: text(&v, "profile")?.to_owned(),
    };
    let output = read_key(text(&v, "output")?)?;
    let witnesses = items(&v, "witnesses")?
        .iter()
        .map(|w| read_witness(w, &scope))
        .collect::<Result<_, _>>()?;
    let operations = integer(&v, "operations")?;
    let response_bytes = integer(&v, "response_bytes")?;
    if scope.snapshot.is_empty() || scope.profile.is_empty() || operations < 0 || response_bytes < 0 {
        return Err("compiler read receipt has incomplete scope or counters".to_owned());
    }
    Ok(ReadReceipt {
        schema: READ_SCHEMA.to_owned(),
        scope,
        preparation: PreparationEvidence {
            output,
            rank_certificate: text(&v, "rank_certificate")?.to_owned(),
            selector_witnesses: text(&v, "selector_witnesses")?.to_owned(),
        },
        witnesses,
        operations,
        response_bytes,
    })
}
