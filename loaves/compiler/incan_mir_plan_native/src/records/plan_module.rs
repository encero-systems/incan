//! A module's plan record: the plan function of every body of one module, as Bakery pages it for admission.

use crate::wire::{
    Schema, Shape, ShapeBody, ShapeField, Wire, decode_graph_row, encode_graph_row, items, required, text,
};

/// The family of a module's plan record.
pub const PLAN_MODULE_FAMILY: &str = "plan-module";
/// The suffix of a module's plan record page.
pub const PLAN_MODULE_PAGE: &str = ".incnmodule";
/// The suffix of a page whose recorded node positions are the current source's.
pub const POSITIONED_MODULE_PAGE: &str = ".incnmodule.positioned";
/// The suffix of a page whose bodies must be placed at the current source's node positions.
pub const REPOSITION_MODULE_PAGE: &str = ".incnmodule.reposition";
/// The suffix of the note of a module whose every ordinary body was restored.
pub const ADMITTED_MODULE_NOTE: &str = ".admitted";
/// The suffix of the note of a module some of whose ordinary bodies were restored.
pub const PARTIAL_MODULE_NOTE: &str = ".partially-admitted";

const PLAN_MODULE_SELECTOR: &[u8] = b"plnmod\x00";
const MAX_PLAN_MODULE_FUNCTIONS: usize = 65_536;
const FUNCTION_FIELDS: usize = 4;

static PLAN_MODULE: Shape = Shape {
    selector: PLAN_MODULE_SELECTOR,
    path: "incan.plan-module/2",
    body: ShapeBody::Record(&[
        ShapeField {
            identity: b"events",
            name: "events",
            storage: "list<binary>",
        },
        ShapeField {
            identity: b"funcs_",
            name: "functions",
            storage: "list<string>",
        },
        ShapeField {
            identity: b"module",
            name: "module",
            storage: "string",
        },
    ]),
};
static PLAN_MODULE_SCHEMA: Schema = Schema::new(&[&PLAN_MODULE]);

/// One body of a module with a plan product: the owner its plan-function record is keyed by, the canonical identity of
/// the body, its native function name, the key of its product, and the final lowering event with the product.
#[derive(Clone, PartialEq, Eq)]
pub struct PlanModuleFunction {
    pub identity: String,
    pub symbol: String,
    pub function: String,
    pub key: String,
    pub event: Vec<u8>,
}

/// A module, named as lowering names it, with the plan function of every one of its bodies.
#[derive(Clone, PartialEq, Eq)]
pub struct PlanModuleValue {
    pub module: String,
    pub functions: Vec<PlanModuleFunction>,
}

// Oven prints these records in the reference models' `Name(field=value, ...)` text.
crate::structural_debug!(
    PlanModuleFunction,
    "PlanModuleFunction",
    identity,
    symbol,
    function,
    key,
    event
);
crate::structural_debug!(PlanModuleValue, "PlanModuleValue", module, functions);

/// Refuse an empty field, an empty event or a function out of strictly ascending owner and name order.
fn ordered(function: &PlanModuleFunction, previous: &str) -> Result<String, String> {
    let order = format!("{}\n{}", function.identity, function.function);
    let empty = [&function.identity, &function.symbol, &function.function, &function.key]
        .iter()
        .any(|field| field.is_empty());
    if empty || function.event.is_empty() || order.as_str() <= previous {
        return Err("malformed plan-module record".into());
    }
    Ok(order)
}

/// Encode one module's plan functions as one Crumb row, in strictly ascending order of owner and function.
pub fn encode_plan_module(value: &PlanModuleValue) -> Result<Vec<u8>, String> {
    if value.module.is_empty() || value.functions.is_empty() || value.functions.len() > MAX_PLAN_MODULE_FUNCTIONS {
        return Err("malformed plan-module record".into());
    }
    let mut flattened = Vec::with_capacity(value.functions.len() * FUNCTION_FIELDS);
    let mut events = Vec::with_capacity(value.functions.len());
    let mut previous = String::new();
    for function in &value.functions {
        previous = ordered(function, &previous)?;
        flattened.extend([
            Wire::Text(function.identity.clone()),
            Wire::Text(function.symbol.clone()),
            Wire::Text(function.function.clone()),
            Wire::Text(function.key.clone()),
        ]);
        events.push(Wire::Binary(function.event.clone()));
    }
    encode_graph_row(
        PLAN_MODULE_SELECTOR,
        &PLAN_MODULE_SCHEMA,
        vec![
            required(b"events", Wire::List(events)),
            required(b"funcs_", Wire::List(flattened)),
            required(b"module", Wire::Text(value.module.clone())),
        ],
    )
}

/// Decode one complete module row and refuse unordered, empty or trailing function fields.
pub fn decode_plan_module(encoded: &[u8]) -> Result<PlanModuleValue, String> {
    let fields = decode_graph_row(encoded, PLAN_MODULE_SELECTOR, &PLAN_MODULE_SCHEMA, 3)?;
    let events = items(&fields[0].value)?;
    let flattened = items(&fields[1].value)?
        .iter()
        .map(text)
        .collect::<Result<Vec<_>, _>>()?;
    let module = text(&fields[2].value)?;
    if module.is_empty()
        || events.is_empty()
        || events.len() > MAX_PLAN_MODULE_FUNCTIONS
        || flattened.len() != events.len() * FUNCTION_FIELDS
    {
        return Err("malformed plan-module record".into());
    }
    let mut functions = Vec::with_capacity(events.len());
    let mut previous = String::new();
    for (index, item) in events.iter().enumerate() {
        let Wire::Binary(event) = item else {
            return Err("malformed plan-module record".into());
        };
        let offset = index * FUNCTION_FIELDS;
        let function = PlanModuleFunction {
            identity: flattened[offset].clone(),
            symbol: flattened[offset + 1].clone(),
            function: flattened[offset + 2].clone(),
            key: flattened[offset + 3].clone(),
            event: event.clone(),
        };
        previous = ordered(&function, &previous)?;
        functions.push(function);
    }
    Ok(PlanModuleValue { module, functions })
}
