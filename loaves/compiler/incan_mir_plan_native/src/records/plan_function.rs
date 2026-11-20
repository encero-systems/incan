//! One bounded Crumb row for a retained plan-function product and its complete input witness.

use incan_semantics_core::semantic_digest::semantic_digest;
use sha2::{Digest, Sha256};

use crate::wire::{Schema, Shape, ShapeBody, ShapeField, Wire, decode_graph_row, encode_graph_row, items, required};

/// The root selector of a plan-function record.
const PLAN_FUNCTION_ROOT: &[u8] = b"plnfun\x00";

/// The retained product independently of its filesystem projection.
static PLAN_FUNCTION: Shape = Shape {
    selector: PLAN_FUNCTION_ROOT,
    path: "incan.plan-function-product/3",
    body: ShapeBody::Record(&[
        ShapeField {
            identity: b"digest",
            name: "digest",
            storage: "string",
        },
        ShapeField {
            identity: b"funct_",
            name: "function",
            storage: "binary",
        },
        ShapeField {
            identity: b"inputs",
            name: "inputs",
            storage: "list<string>",
        },
        ShapeField {
            identity: b"reques",
            name: "requests",
            storage: "list<binary>",
        },
    ]),
};
static PLAN_FUNCTION_SCHEMA: Schema = Schema::new(&[&PLAN_FUNCTION]);

/// The exact lowering inputs, native function payload, instance requests and payload digest.
#[derive(Clone, Debug, PartialEq)]
pub struct PlanFunctionRecord {
    pub inputs: Vec<String>,
    pub function: Vec<u8>,
    pub requests: Vec<Vec<u8>>,
    pub digest: String,
}

/// Encode one validated retained product as a complete Crumb envelope.
pub fn encode_plan_function_record(record: &PlanFunctionRecord) -> Result<Vec<u8>, String> {
    validate_record(record)?;
    encode_graph_row(
        PLAN_FUNCTION_ROOT,
        &PLAN_FUNCTION_SCHEMA,
        vec![
            required(b"digest", Wire::Text(record.digest.clone())),
            required(b"funct_", Wire::Binary(record.function.clone())),
            required(
                b"inputs",
                Wire::List(record.inputs.iter().map(|input| Wire::Text(input.clone())).collect()),
            ),
            required(
                b"reques",
                Wire::List(record.requests.iter().map(|row| Wire::Binary(row.clone())).collect()),
            ),
        ],
    )
}

/// Decode exactly one retained-product row without a legacy JSON fallback.
pub fn decode_plan_function_record(encoded: &[u8]) -> Result<PlanFunctionRecord, String> {
    let fields = decode_graph_row(encoded, PLAN_FUNCTION_ROOT, &PLAN_FUNCTION_SCHEMA, 4)?;
    let record = PlanFunctionRecord {
        digest: text_value(&fields[0].value)?,
        function: binary_value(&fields[1].value)?,
        inputs: text_values(&fields[2].value)?,
        requests: items(&fields[3].value)?
            .iter()
            .map(binary_value)
            .collect::<Result<_, _>>()?,
    };
    validate_record(&record)?;
    Ok(record)
}

/// Authenticate the exact Crumb product and ordered instance requests with the compiler's framed digest.
pub fn plan_function_digest(function: &[u8], requests: &[Vec<u8>]) -> Result<String, String> {
    let mut parts = vec![
        "incan.plan-function-product/3".to_owned(),
        format!("sha256:{}", hex(&Sha256::digest(function))),
    ];
    parts.extend(
        requests
            .iter()
            .map(|request| format!("sha256:{}", hex(&Sha256::digest(request)))),
    );
    semantic_digest(&parts).map_err(|error| error.to_string())
}

/// Lowercase hexadecimal digits of a digest.
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Require binary Crumb data.
fn binary_value(value: &Wire) -> Result<Vec<u8>, String> {
    match value {
        Wire::Binary(data) => Ok(data.clone()),
        _ => Err("plan-function payload requires binary Crumb data".to_owned()),
    }
}

/// Refuse incomplete witnesses and malformed semantic digests before publication or reuse.
fn validate_record(record: &PlanFunctionRecord) -> Result<(), String> {
    if record.inputs.is_empty() || record.function.is_empty() {
        return Err("incomplete plan-function record".to_owned());
    }
    let digits = record.digest.strip_prefix("sha256:");
    let valid = record.digest.len() == 71
        && digits.is_some_and(|digits| digits.bytes().all(|digit| matches!(digit, b'0'..=b'9' | b'a'..=b'f')));
    if !valid {
        return Err("invalid plan-function record digest".to_owned());
    }
    Ok(())
}

/// Require one logical Crumb string.
fn text_value(value: &Wire) -> Result<String, String> {
    match value {
        Wire::Text(text) => Ok(text.clone()),
        _ => Err("plan-function Crumb field is not text".to_owned()),
    }
}

/// Require one logical Crumb list containing only strings.
fn text_values(value: &Wire) -> Result<Vec<String>, String> {
    match value {
        Wire::List(items) => items.iter().map(text_value).collect(),
        _ => Err("plan-function Crumb field is not a text list".to_owned()),
    }
}
