//! One bounded Crumb row for a retained plan-function product and its complete input witness.

use std::collections::HashMap;

use incan_semantics_core::semantic_digest::semantic_digest;
use sha2::{Digest, Sha256};

use crate::records::wire_schema::{STORED_EXTERNAL_SCHEMA, STORED_PRODUCT_SCHEMA};
use crate::wire::{
    Schema, Shape, ShapeBody, ShapeField, Wire, admit_root, decode_graph_row, encode_graph_row, items, required,
    write_wire,
};

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

/// The root selector of a keyed function product, whose runtime field a lowering demand reads.
const PRODUCT_ROOT: &[u8] = b"G\xc7\x7f$\xca!\x00";
/// The product's runtime field identity.
const RUNTIME_FIELD: &[u8] = b"\xd9,j\x81\xb2\xff";
/// The root selector of one runtime dependency signature.
const EXTERNAL_ROOT: &[u8] = b"t\xaa^\x89\x16D\x00";
/// The root selector of a lowering demand.
const DEMAND_ROOT: &[u8] = b"lowdem\x00";
/// The largest runtime field a demand cache keys by its exact bytes.
const CACHED_FIELD_BYTES: i64 = 65_536;
/// The bound on one publication's cached signature bytes.
const CACHE_BYTES: usize = 67_108_864;

/// One function's lowering demand: its complete producer inputs and the runtime signatures its product requires.
static LOWERING_DEMAND: Shape = Shape {
    selector: DEMAND_ROOT,
    path: "incan.lowering-demand/4",
    body: ShapeBody::Record(&[
        ShapeField {
            identity: b"inputs",
            name: "inputs",
            storage: "list<string>",
        },
        ShapeField {
            identity: b"runtim",
            name: "runtime",
            storage: "list<binary>",
        },
    ]),
};
static LOWERING_DEMAND_SCHEMA: Schema = Schema::new(&[&LOWERING_DEMAND]);

/// The standalone entry and its ordered, complete producer inputs.
static PLAN_ENTRY: Shape = Shape {
    selector: b"plnent\x00",
    path: "incan.plan-entry/2",
    body: ShapeBody::Record(&[
        ShapeField {
            identity: b"entry_",
            name: "entry",
            storage: "string",
        },
        ShapeField {
            identity: b"inputs",
            name: "inputs",
            storage: "list<string>",
        },
    ]),
};
static PLAN_ENTRY_SCHEMA: Schema = Schema::new(&[&PLAN_ENTRY]);

/// Retain the standalone entry and its ordered, complete producer inputs.
pub fn encode_entry_record(entry: &str, inputs: &[String]) -> Result<Vec<u8>, String> {
    encode_graph_row(
        b"plnent\x00",
        &PLAN_ENTRY_SCHEMA,
        vec![
            required(b"entry_", Wire::Text(entry.to_owned())),
            required(
                b"inputs",
                Wire::List(inputs.iter().map(|input| Wire::Text(input.clone())).collect()),
            ),
        ],
    )
}

/// Authenticate an admitted entry against its current selection, generation and complete ordered inputs.
pub fn verify_entry_record(encoded: &[u8], entry: &str, inputs: &[String]) -> Result<(), String> {
    let fields = decode_graph_row(encoded, b"plnent\x00", &PLAN_ENTRY_SCHEMA, 2)?;
    if text_value(&fields[0].value)? != entry || text_values(&fields[1].value)? != inputs {
        return Err("selected plan-function(entry) disagrees with its complete inputs".to_owned());
    }
    Ok(())
}

/// One publication's decoder and bounded cache of fully authenticated runtime signature lists.
#[derive(Debug, Default)]
pub struct LoweringDemands {
    externals: HashMap<String, Vec<Vec<u8>>>,
    cache_bytes: usize,
}

impl LoweringDemands {
    /// Encode the demand of one admitted product: its producer inputs and each runtime signature as its own row.
    ///
    /// The source bytes and dictionary ordinals together specify every runtime signature. A cache hit still requires
    /// full admission, including all fields, the exact root and the complete referenced dictionary.
    pub fn encode(&mut self, inputs: &[String], function: &[u8]) -> Result<Vec<u8>, String> {
        let (mut row, slots) = admit_root(function, PRODUCT_ROOT, &STORED_PRODUCT_SCHEMA, "native plan wire")?;
        let cell = row.field_cell(RUNTIME_FIELD)?;
        let end = cell.origin.checked_add(cell.length);
        let within = end.is_some_and(|end| end <= function.len() as i64);
        if !cell.present || cell.origin < 0 || cell.length < 0 || !within {
            return Err("native plan product has no typed runtime requirements".to_owned());
        }
        let mut key = String::new();
        if cell.length <= CACHED_FIELD_BYTES {
            let start = cell.origin as usize;
            let bytes = &function[start..start + cell.length as usize];
            let ordinals: Vec<String> = slots.iter().map(i64::to_string).collect();
            key = format!("{}:{}", ordinals.join(","), hex(bytes));
        }
        if let Some(externals) = self.externals.get(&key).filter(|_| !key.is_empty()) {
            return demand_row(inputs, externals.clone());
        }
        let mut externals = Vec::new();
        let mut size = key.len();
        for signature in items(&row.read_field(RUNTIME_FIELD)?)? {
            let encoded = write_wire(EXTERNAL_ROOT, &STORED_EXTERNAL_SCHEMA, signature.clone())?;
            size += encoded.len();
            externals.push(encoded);
        }
        if !key.is_empty() && self.cache_bytes + size <= CACHE_BYTES {
            self.externals.insert(key, externals.clone());
            self.cache_bytes += size;
        }
        demand_row(inputs, externals)
    }
}

/// Encode one demand independently of other publications.
pub fn encode_lowering_demand(inputs: &[String], function: &[u8]) -> Result<Vec<u8>, String> {
    LoweringDemands::default().encode(inputs, function)
}

/// The demand row: inputs, then the runtime signature rows in product order.
fn demand_row(inputs: &[String], externals: Vec<Vec<u8>>) -> Result<Vec<u8>, String> {
    encode_graph_row(
        DEMAND_ROOT,
        &LOWERING_DEMAND_SCHEMA,
        vec![
            required(
                b"inputs",
                Wire::List(inputs.iter().map(|input| Wire::Text(input.clone())).collect()),
            ),
            required(b"runtim", Wire::List(externals.into_iter().map(Wire::Binary).collect())),
        ],
    )
}
