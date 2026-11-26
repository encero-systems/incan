//! Typed completed-read Crumb rows shared by compiler workers and the engine graph publisher.

use crate::crumb_rows::{ModelField, model_description, required_value};
use crate::graph_crumb_fields::{
    GraphRowInput, KEY_TYPE, REQUEST_TYPE, decode_graph_input, encode_graph_row, integer, items, key_value, number,
    read_key, read_request, request_value, text, tuple_items,
};
use crate::record_scope::{
    AbsentFact, DeletedFact, PreparationEvidence, READ_SCHEMA, ReadReceipt, ReadScope, ReadWitness, RecordRequest,
    RevisionEvidence, valid_request,
};
use crate::store_record_source::encode_record_key;
use internal_models_bindings::{Description, MappedView, Value};

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
    decode_receipt(GraphRowInput::Bytes(encoded))
}

/// Decode a receipt using its existing mapped backing, under the same limits and validation as byte input.
///
/// The caller supplies an immutable bounded mapping. The returned receipt owns its decoded witnesses.
pub fn decode_read_receipt_view(view: &MappedView) -> Result<ReadReceipt, String> {
    decode_receipt(GraphRowInput::Mapped(view))
}

/// Validate the completed receipt's scope, producer and typed witnesses independently of its storage placement.
fn decode_receipt(input: GraphRowInput<'_>) -> Result<ReadReceipt, String> {
    let fields = decode_graph_input(input, SELECTOR, &description(), 9)?;
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

#[cfg(test)]
mod tests {
    use super::{decode_read_receipt, decode_read_receipt_view, encode_read_receipt};
    use crate::crumb_rows::take_owned_input_copies;
    use crate::record_scope::{
        AbsentFact, DEFAULT_READ_BYTES, DeletedFact, MemberSelection, PreparationEvidence, READ_SCHEMA, ReadReceipt,
        ReadScope, ReadWitness, RecordKey, RecordRequest, RecordSelection, RevisionEvidence,
    };
    use internal_models_bindings::{MappedView, complete_view, mapped_bytes};

    /// A completed receipt containing every witness kind, with a bounded membership continuation.
    fn complete_receipt() -> ReadReceipt {
        let scope = ReadScope {
            snapshot: "snapshot".into(),
            profile: "checker".into(),
        };
        let key = RecordKey {
            family: "namespace".into(),
            identity: "src/module.incn".into(),
            identity_profile: "namespace-key".into(),
            origin: "package".into(),
            facet: "checked".into(),
            context: "interface".into(),
        };
        let request = RecordRequest {
            key: key.clone(),
            selection: RecordSelection::Field("value".into()),
            byte_limit: DEFAULT_READ_BYTES,
        };
        let members = RecordRequest {
            key: key.clone(),
            selection: RecordSelection::Members(MemberSelection {
                selector: "children".into(),
                order: "canonical".into(),
                lower: "left".into(),
                upper: "right".into(),
                continuation: "".into(),
            }),
            byte_limit: DEFAULT_READ_BYTES,
        };
        let witnesses = vec![
            ReadWitness::Found(
                RevisionEvidence {
                    scope: scope.clone(),
                    request: request.clone(),
                    revision: "frame".into(),
                    semantic_digest: "meaning".into(),
                },
                "selection".into(),
            ),
            ReadWitness::Deleted(DeletedFact {
                scope: scope.clone(),
                request: request.clone(),
                revision: "tombstone".into(),
            }),
            ReadWitness::Absent(AbsentFact {
                scope: scope.clone(),
                request,
                exclusion_digest: "exclusion".into(),
            }),
            ReadWitness::Members(scope.clone(), members, "membership".into(), "next".into()),
        ];
        ReadReceipt {
            schema: READ_SCHEMA.into(),
            scope,
            preparation: PreparationEvidence {
                output: key,
                rank_certificate: "rank".into(),
                selector_witnesses: "checked-selectors".into(),
            },
            witnesses,
            operations: 4,
            response_bytes: 120,
        }
    }

    /// Anchor a saved receipt as a mapped test input before observing the decoder.
    fn fixture_view(encoded: &[u8]) -> Result<MappedView, String> {
        let backing = mapped_bytes(&encoded.to_vec())?;
        complete_view(&backing).ok_or_else(|| "receipt fixture has no complete mapped view".into())
    }

    /// Store-backed receipt decoding should preserve every typed witness without creating owned input backing.
    #[test]
    fn mapped_receipt_preserves_all_witnesses_without_copying_input() -> Result<(), String> {
        let expected = complete_receipt();
        let encoded = encode_read_receipt(&expected)?;
        take_owned_input_copies();
        assert_eq!(decode_read_receipt(&encoded)?, expected);
        assert_eq!(take_owned_input_copies(), (1, encoded.len()));

        let view = fixture_view(&encoded)?;
        assert_eq!(decode_read_receipt_view(&view)?, expected);
        assert_eq!(
            take_owned_input_copies(),
            (0, 0),
            "mapped receipt copied its encoded row into owned input backing"
        );
        let truncated = &encoded[..encoded.len() - 1];
        let legacy_truncated = decode_read_receipt(truncated);
        assert!(legacy_truncated.is_err());
        assert_eq!(decode_read_receipt_view(&fixture_view(truncated)?), legacy_truncated);

        let mut unsupported = expected;
        unsupported.schema = "unknown-read-receipt".into();
        let unknown_schema = encode_read_receipt(&unsupported)?;
        let legacy_unknown = decode_read_receipt(&unknown_schema);
        assert_eq!(legacy_unknown, Err("unsupported compiler read receipt schema".into()));
        assert_eq!(
            decode_read_receipt_view(&fixture_view(&unknown_schema)?),
            legacy_unknown
        );
        Ok(())
    }
}
