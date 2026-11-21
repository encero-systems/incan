//! One bounded Crumb row for a retained plan-function product and its complete input witness.
//!
//! Transitional: the plan lane's incan_mir_plan_native takes this module over.

use crate::crumb_rows::{ModelField, model_description, required_value};
use crate::graph_crumb_fields::{decode_graph_row, encode_graph_row, graph_read_budget, items};
use crate::hex_text::hexlify;
use crate::plan_wire_rows::{PreparedWireReader, wire_field, write_wire};
use crate::plan_wire_schema::{stored_external_descriptions, stored_product_descriptions};
use incan_semantics_core::semantic_digest::semantic_digest;
use internal_models_bindings::{Description, Value, binary_message, codec_allowance, depth_allowance, settle};
use sha2::{Digest, Sha256};
use std::collections::HashMap;

const PLAN_FUNCTION_SELECTOR: &[u8] = b"plnfun\x00";
const ENTRY_SELECTOR: &[u8] = b"plnent\x00";
const DEMAND_SELECTOR: &[u8] = b"lowdem\x00";
const PRODUCT_ROOT: &[u8] = b"\x47\xc7\x7f\x24\xca\x21\x00";
const RUNTIME_FIELD: &[u8] = b"\xd9\x2c\x6a\x81\xb2\xff";
const EXTERNAL_ROOT: &[u8] = b"\x74\xaa\x5e\x89\x16\x44\x00";
const MAX_CACHE_BYTES: i64 = 67_108_864;

/// The exact lowering inputs, native function payload, instance requests and payload digest.
#[derive(Clone, PartialEq, Eq)]
pub struct PlanFunctionRecord {
    pub inputs: Vec<String>,
    pub function: Vec<u8>,
    pub requests: Vec<Vec<u8>>,
    pub digest: String,
}
crate::incan_debug::incan_debug!(PlanFunctionRecord {
    inputs,
    function,
    requests,
    digest
});

/// Encode one validated retained product as a complete Crumb envelope.
pub fn encode_plan_function_record(record: &PlanFunctionRecord) -> Result<Vec<u8>, String> {
    validate_record(record)?;
    encode_graph_row(
        PLAN_FUNCTION_SELECTOR,
        &plan_function_description(),
        vec![
            required_value(b"digest", Value::Text(record.digest.clone())),
            required_value(b"funct_", Value::Binary(record.function.clone())),
            required_value(b"inputs", Value::List(text_items(&record.inputs))),
            required_value(
                b"reques",
                Value::List(record.requests.iter().cloned().map(Value::Binary).collect()),
            ),
        ],
    )
}

/// Decode exactly one retained-product row without a legacy JSON fallback.
pub fn decode_plan_function_record(encoded: &[u8]) -> Result<PlanFunctionRecord, String> {
    let fields = decode_graph_row(encoded, PLAN_FUNCTION_SELECTOR, &plan_function_description(), 4)?;
    let record = PlanFunctionRecord {
        digest: text_value(&fields[0].value)?,
        function: binary_value(&fields[1].value)?,
        inputs: text_values(&fields[2].value)?,
        requests: items(&fields[3].value)?
            .iter()
            .map(binary_value)
            .collect::<Result<Vec<_>, _>>()?,
    };
    validate_record(&record)?;
    Ok(record)
}

/// Authenticate the exact Crumb product and ordered instance requests with the compiler's framed digest.
pub fn plan_function_digest(function: &[u8], requests: &[Vec<u8>]) -> Result<String, String> {
    let mut parts = vec![
        "incan.plan-function-product/3".to_owned(),
        format!("sha256:{}", hexlify(&Sha256::digest(function))),
    ];
    parts.extend(
        requests
            .iter()
            .map(|request| format!("sha256:{}", hexlify(&Sha256::digest(request)))),
    );
    semantic_digest(&parts).map_err(|error| error.to_string())
}

fn binary_value(value: &Value) -> Result<Vec<u8>, String> {
    match value {
        Value::Binary(data) => Ok(data.clone()),
        _ => Err("plan-function payload requires binary Crumb data".into()),
    }
}

/// Refuse incomplete witnesses and malformed semantic digests before publication or reuse.
fn validate_record(record: &PlanFunctionRecord) -> Result<(), String> {
    if record.inputs.is_empty() || record.function.is_empty() {
        return Err("incomplete plan-function record".into());
    }
    if record.digest.chars().count() != 71
        || !record.digest.starts_with("sha256:")
        || !record
            .digest
            .chars()
            .skip(7)
            .all(|digit| "0123456789abcdef".contains(digit))
    {
        return Err("invalid plan-function record digest".into());
    }
    Ok(())
}

/// Project a string list into its exact logical Crumb values.
fn text_items(values: &[String]) -> Vec<Value> {
    values.iter().cloned().map(Value::Text).collect()
}

/// Require one logical Crumb string.
fn text_value(value: &Value) -> Result<String, String> {
    match value {
        Value::Text(text) => Ok(text.clone()),
        _ => Err("plan-function Crumb field is not text".into()),
    }
}

/// Require one logical Crumb list containing only strings.
fn text_values(value: &Value) -> Result<Vec<String>, String> {
    match value {
        Value::List(items) => items.iter().map(text_value).collect(),
        _ => Err("plan-function Crumb field is not a text list".into()),
    }
}

/// Describe the retained product independently of its filesystem projection.
fn plan_function_description() -> Description {
    model_description(
        PLAN_FUNCTION_SELECTOR,
        "incan.plan-function-product/3",
        &[
            ModelField::new(b"digest", "digest", "string"),
            ModelField::new(b"funct_", "function", "binary"),
            ModelField::new(b"inputs", "inputs", "list<string>"),
            ModelField::new(b"reques", "requests", "list<binary>"),
        ],
    )
}

fn entry_description() -> Description {
    model_description(
        ENTRY_SELECTOR,
        "incan.plan-entry/2",
        &[
            ModelField::new(b"entry_", "entry", "string"),
            ModelField::new(b"inputs", "inputs", "list<string>"),
        ],
    )
}

/// Authenticate an admitted entry against its current selection, generation and complete ordered inputs.
pub fn verify_entry_record(encoded: &[u8], entry: &str, inputs: &[String]) -> Result<(), String> {
    let fields = decode_graph_row(encoded, ENTRY_SELECTOR, &entry_description(), 2)?;
    if text_value(&fields[0].value)? != entry || text_values(&fields[1].value)? != inputs {
        return Err("selected plan-function(entry) disagrees with its complete inputs".into());
    }
    Ok(())
}

/// Retain the standalone entry and its ordered, complete producer inputs.
pub fn encode_entry_record(entry: &str, inputs: &[String]) -> Result<Vec<u8>, String> {
    encode_graph_row(
        ENTRY_SELECTOR,
        &entry_description(),
        vec![
            required_value(b"entry_", Value::Text(entry.to_owned())),
            required_value(b"inputs", Value::List(text_items(inputs))),
        ],
    )
}

/// One publication's decoder and bounded cache of fully authenticated runtime signature lists. Starts empty.
#[derive(Default)]
pub struct LoweringDemands {
    pub(crate) reader: Option<PreparedWireReader>,
    pub(crate) externals: HashMap<String, Vec<Value>>,
    pub(crate) cache_bytes: i64,
}

impl LoweringDemands {
    /// A demand encoder with an empty cache.
    pub fn new() -> Self {
        Self::default()
    }

    /// Encode one lowering demand: the producer inputs and the product's runtime signatures as external rows.
    pub fn encode(&mut self, inputs: &[String], function: &[u8]) -> Result<Vec<u8>, String> {
        if self.reader.is_none() {
            self.reader = Some(PreparedWireReader::create(
                PRODUCT_ROOT,
                &stored_product_descriptions(),
                "native plan wire",
            )?);
        }
        let Some(reader) = &self.reader else {
            return Err("native plan wire reader is unavailable".into());
        };
        let mut budget = graph_read_budget()?;
        let mut slots = Vec::new();
        let row = reader.admit(function, &mut budget, &mut slots)?;
        let (available, elements) = codec_allowance(&budget);
        let cell = row.field_cell(0, RUNTIME_FIELD.to_vec(), available, elements, depth_allowance(&budget));
        settle(&cell, &mut budget).map_err(|error| binary_message(&error))?;
        if !cell.present || cell.origin < 0 || cell.length < 0 || cell.origin + cell.length > function.len() as i64 {
            return Err("native plan product has no typed runtime requirements".into());
        }
        // The source bytes and dictionary ordinals together specify every runtime signature. A cache hit still
        // requires full admission, including all fields, the exact root and the complete referenced dictionary.
        let mut key = String::new();
        if cell.length <= 65_536 {
            let (start, end) = (cell.origin as usize, (cell.origin + cell.length) as usize);
            let ordinals: Vec<String> = slots.iter().map(i64::to_string).collect();
            key = format!("{}:{}", ordinals.join(","), hexlify(&function[start..end]));
        }
        if !key.is_empty()
            && let Some(cached) = self.externals.get(&key)
        {
            return demand_row(inputs, cached.clone());
        }
        let mut externals = Vec::new();
        let mut size = key.chars().count() as i64;
        for signature in items(&wire_field(&row, RUNTIME_FIELD, &mut budget)?)? {
            let encoded = write_wire(EXTERNAL_ROOT, &stored_external_descriptions(), signature.clone())?;
            size += encoded.len() as i64;
            externals.push(Value::Binary(encoded));
        }
        if !key.is_empty() && self.cache_bytes + size <= MAX_CACHE_BYTES {
            self.externals.insert(key, externals.clone());
            self.cache_bytes += size;
        }
        demand_row(inputs, externals)
    }
}

fn demand_row(inputs: &[String], externals: Vec<Value>) -> Result<Vec<u8>, String> {
    let description = model_description(
        DEMAND_SELECTOR,
        "incan.lowering-demand/4",
        &[
            ModelField::new(b"inputs", "inputs", "list<string>"),
            ModelField::new(b"runtim", "runtime", "list<binary>"),
        ],
    );
    encode_graph_row(
        DEMAND_SELECTOR,
        &description,
        vec![
            required_value(b"inputs", Value::List(text_items(inputs))),
            required_value(b"runtim", Value::List(externals)),
        ],
    )
}

/// Encode one demand independently of other publications.
pub fn encode_lowering_demand(inputs: &[String], function: &[u8]) -> Result<Vec<u8>, String> {
    LoweringDemands::default().encode(inputs, function)
}
