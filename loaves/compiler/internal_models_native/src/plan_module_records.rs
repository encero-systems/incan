//! The module-level plan record: one Crumb row naming a module's plan functions, their final events kept opaque.

use crate::checker_record_models::CodecFault;
use crate::crumb_rows::{ModelField, decode_rows, encode_rows, model_description, required_value};
use internal_models_bindings::{Description, RowRecord, Value};

pub const PLAN_MODULE_FAMILY: &str = "plan-module";
pub const PLAN_MODULE_PAGE: &str = ".incnmodule";
pub const POSITIONED_MODULE_PAGE: &str = ".incnmodule.positioned";
pub const REPOSITION_MODULE_PAGE: &str = ".incnmodule.reposition";
pub const ADMITTED_MODULE_NOTE: &str = ".admitted";
pub const PARTIAL_MODULE_NOTE: &str = ".partially-admitted";
const PLAN_MODULE_SELECTOR: &[u8] = b"plnmod\x00";
const MAX_PLAN_MODULE_BYTES: i64 = 67_108_864;
const MAX_PLAN_MODULE_FUNCTIONS: usize = 65_536;
const FUNCTION_FIELDS: usize = 4;

/// One body of a module with a plan product: the owner its plan-function record is keyed by, the canonical identity
/// of the body, its native function name, the key of its product, and the final lowering event with the product.
#[derive(Clone, PartialEq, Eq)]
pub struct PlanModuleFunction {
    pub identity: String,
    pub symbol: String,
    pub function: String,
    pub key: String,
    pub event: Vec<u8>,
}
crate::incan_debug::incan_debug!(PlanModuleFunction {
    identity,
    symbol,
    function,
    key,
    event
});

/// A module, named as lowering names it, with the plan function of every one of its bodies.
#[derive(Clone, PartialEq, Eq)]
pub struct PlanModuleValue {
    pub module: String,
    pub functions: Vec<PlanModuleFunction>,
}
crate::incan_debug::incan_debug!(PlanModuleValue { module, functions });

/// Describe one module's plan functions without interpreting their events.
fn plan_module_description() -> Description {
    model_description(
        PLAN_MODULE_SELECTOR,
        "incan.plan-module/2",
        &[
            ModelField::new(b"events", "events", "list<binary>"),
            ModelField::new(b"funcs_", "functions", "list<string>"),
            ModelField::new(b"module", "module", "string"),
        ],
    )
}

/// Refuse an empty field or event, and a function out of strictly ascending owner and function order.
fn admit_function(function: &PlanModuleFunction, previous: &mut String) -> Result<(), CodecFault> {
    let order = format!("{}\n{}", function.identity, function.function);
    if [&function.identity, &function.symbol, &function.function, &function.key]
        .iter()
        .any(|part| part.is_empty())
        || function.event.is_empty()
        || order <= *previous
    {
        return Err(CodecFault::Malformed);
    }
    *previous = order;
    Ok(())
}

/// Encode one module's plan functions as one Crumb row, in strictly ascending order of owner and function.
pub fn encode_plan_module(value: &PlanModuleValue) -> Result<Vec<u8>, CodecFault> {
    if value.module.is_empty() || value.functions.is_empty() || value.functions.len() > MAX_PLAN_MODULE_FUNCTIONS {
        return Err(CodecFault::Malformed);
    }
    let mut flattened = Vec::with_capacity(value.functions.len() * FUNCTION_FIELDS);
    let mut events = Vec::with_capacity(value.functions.len());
    let mut previous = String::new();
    for function in &value.functions {
        admit_function(function, &mut previous)?;
        flattened.extend([
            Value::Text(function.identity.clone()),
            Value::Text(function.symbol.clone()),
            Value::Text(function.function.clone()),
            Value::Text(function.key.clone()),
        ]);
        events.push(Value::Binary(function.event.clone()));
    }
    let record = RowRecord {
        key: PLAN_MODULE_SELECTOR.to_vec(),
        value: Value::Record(vec![
            required_value(b"events", Value::List(events)),
            required_value(b"funcs_", Value::List(flattened)),
            required_value(b"module", Value::Text(value.module.clone())),
        ]),
    };
    encode_rows(&plan_module_description(), vec![record], MAX_PLAN_MODULE_BYTES).map_err(|_| CodecFault::Limit)
}

/// Decode one complete module row and refuse unordered, empty or trailing function fields.
pub fn decode_plan_module(encoded: &[u8]) -> Result<PlanModuleValue, CodecFault> {
    let rows = decode_rows(encoded, &vec![plan_module_description()], MAX_PLAN_MODULE_BYTES)
        .map_err(|_| CodecFault::Malformed)?;
    let [row] = rows.as_slice() else {
        return Err(CodecFault::Malformed);
    };
    if row.key != PLAN_MODULE_SELECTOR {
        return Err(CodecFault::Malformed);
    }
    let Value::Record(fields) = &row.value else {
        return Err(CodecFault::Malformed);
    };
    if fields.len() != 3 {
        return Err(CodecFault::Malformed);
    }
    let Value::List(events) = &fields[0].value else {
        return Err(CodecFault::Malformed);
    };
    let flattened = text_values(&fields[1].value)?;
    let module = text_value(&fields[2].value)?;
    if module.is_empty()
        || events.is_empty()
        || events.len() > MAX_PLAN_MODULE_FUNCTIONS
        || flattened.len() != events.len() * FUNCTION_FIELDS
    {
        return Err(CodecFault::Malformed);
    }
    let mut functions = Vec::with_capacity(events.len());
    let mut previous = String::new();
    for (item, fields) in events.iter().zip(flattened.chunks_exact(FUNCTION_FIELDS)) {
        let Value::Binary(event) = item else {
            return Err(CodecFault::Malformed);
        };
        let function = PlanModuleFunction {
            identity: fields[0].clone(),
            symbol: fields[1].clone(),
            function: fields[2].clone(),
            key: fields[3].clone(),
            event: event.clone(),
        };
        admit_function(&function, &mut previous)?;
        functions.push(function);
    }
    Ok(PlanModuleValue { module, functions })
}

/// Require one logical Crumb string value.
fn text_value(value: &Value) -> Result<String, CodecFault> {
    match value {
        Value::Text(text) => Ok(text.clone()),
        _ => Err(CodecFault::Malformed),
    }
}

/// Require one logical Crumb string list without accepting nested or alternate values.
fn text_values(value: &Value) -> Result<Vec<String>, CodecFault> {
    match value {
        Value::List(items) => items.iter().map(text_value).collect(),
        _ => Err(CodecFault::Malformed),
    }
}
