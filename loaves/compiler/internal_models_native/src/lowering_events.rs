//! Request-local transport for named lowering products, explicit refusals and the complete native interface.

use crate::crumb_rows::{ModelField, model_description, required_value};
use crate::graph_crumb_fields::{decode_graph_row, encode_graph_row, integer, items, number, text, tuple_items};
use internal_models_bindings::{
    Description, Path, Value, path_is_file, path_read, path_suffixed, path_unlink, path_write,
};
use std::collections::{HashMap, HashSet};

const EVENT: &[u8] = b"lowevt\x00";
const REFUSAL: &[u8] = b"lowref\x00";
const INTERFACE: &[u8] = b"lowint\x00";
const DEFINITIONS: &[u8] = b"admdef\x00";
const NOTE: &[u8] = b"admnot\x00";
const PLACEMENT: &[u8] = b"admgeo\x00";
pub const INTERFACE_REPORT: &str = "native-interface.crumb";
pub const ADMITTED_DEFINITIONS: &str = "admitted-definitions.crumb";

/// A named ordinal's position in the current model or enum table.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct EventReference {
    pub kind: String,
    pub index: i64,
    pub name: String,
}
crate::incan_debug::incan_debug!(EventReference { kind, index, name });

/// A Body IR node's current source coordinates.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct EventPosition {
    pub line: i64,
    pub column: i64,
}
crate::incan_debug::incan_debug!(EventPosition { line, column });

/// The exact published product and its use in this request.
///
/// Defaults: `admitted` false, `module`, `role` and `demand` empty, `runtime_requirements` zero, `references` and
/// `positions` empty.
#[derive(Clone, PartialEq, Eq)]
pub struct LoweringEvent {
    pub identity: String,
    pub key: String,
    pub function: String,
    pub product: Vec<u8>,
    pub reused: bool,
    pub admitted: bool,
    pub module: String,
    pub role: String,
    pub demand: String,
    pub runtime_requirements: i64,
    pub references: Vec<EventReference>,
    pub positions: Vec<EventPosition>,
}
crate::incan_debug::incan_debug!(LoweringEvent {
    identity,
    key,
    function,
    product,
    reused,
    admitted,
    module,
    role,
    demand,
    runtime_requirements,
    references,
    positions
});

/// An explicit per-function reason lowering could not publish a reusable product.
#[derive(Clone, PartialEq, Eq)]
pub struct LoweringRefusal {
    pub identity: String,
    pub function: String,
    pub refusal: String,
}
crate::incan_debug::incan_debug!(LoweringRefusal {
    identity,
    function,
    refusal
});

/// The standalone entry key and its complete loss-checked native plan.
#[derive(Clone, PartialEq, Eq)]
pub struct NativeInterface {
    pub entry: String,
    pub plan: Vec<u8>,
}
crate::incan_debug::incan_debug!(NativeInterface { entry, plan });

/// The filename shared by the lowering publisher and native reader.
pub fn native_interface_filename() -> String {
    INTERFACE_REPORT.to_owned()
}

fn reference_value(reference: &EventReference) -> Value {
    Value::Tuple(vec![
        Value::Text(reference.kind.clone()),
        integer(reference.index),
        Value::Text(reference.name.clone()),
    ])
}

fn position_value(position: &EventPosition) -> Value {
    Value::Tuple(vec![integer(position.line), integer(position.column)])
}

fn read_position(item: &Value) -> Result<EventPosition, String> {
    let pair = tuple_items(item, 2)?;
    Ok(EventPosition {
        line: number(&pair[0])?,
        column: number(&pair[1])?,
    })
}

fn flag(value: &Value, missing: &str) -> Result<bool, String> {
    match value {
        Value::Boolean(found) => Ok(*found),
        _ => Err(missing.to_owned()),
    }
}

fn binary(value: &Value, missing: &str) -> Result<Vec<u8>, String> {
    match value {
        Value::Binary(found) => Ok(found.clone()),
        _ => Err(missing.to_owned()),
    }
}

/// The binary product field retains exactly the bytes admitted under the graph slot.
fn event_description() -> Description {
    model_description(
        EVENT,
        "incan.lowering-event/3",
        &[
            ModelField::new(b"admitt", "admitted", "boolean"),
            ModelField::new(b"demand", "demand", "string"),
            ModelField::new(b"funct_", "function", "string"),
            ModelField::new(b"identi", "identity", "string"),
            ModelField::new(b"key___", "key", "string"),
            ModelField::new(b"module", "module", "string"),
            ModelField::new(b"positi", "positions", "list<tuple<i64,i64>>"),
            ModelField::new(b"produc", "product", "binary"),
            ModelField::new(b"refere", "references", "list<tuple<string,i64,string>>"),
            ModelField::new(b"reused", "reused", "boolean"),
            ModelField::new(b"role__", "role", "string"),
            ModelField::new(b"runtim", "runtime_requirements", "i64"),
        ],
    )
}

/// Transport the authenticated plan-function record without reconstructing or transcoding it.
pub fn encode_lowering_event(event: &LoweringEvent) -> Result<Vec<u8>, String> {
    let text = |value: &String| Value::Text(value.clone());
    encode_graph_row(
        EVENT,
        &event_description(),
        vec![
            required_value(b"admitt", Value::Boolean(event.admitted)),
            required_value(b"demand", text(&event.demand)),
            required_value(b"funct_", text(&event.function)),
            required_value(b"identi", text(&event.identity)),
            required_value(b"key___", text(&event.key)),
            required_value(b"module", text(&event.module)),
            required_value(
                b"positi",
                Value::List(event.positions.iter().map(position_value).collect()),
            ),
            required_value(b"produc", Value::Binary(event.product.clone())),
            required_value(
                b"refere",
                Value::List(event.references.iter().map(reference_value).collect()),
            ),
            required_value(b"reused", Value::Boolean(event.reused)),
            required_value(b"role__", text(&event.role)),
            required_value(b"runtim", integer(event.runtime_requirements)),
        ],
    )
}

/// Read one nominal reference, refusing a kind outside the model and enum tables.
fn read_reference(item: &Value, invalid: &str) -> Result<EventReference, String> {
    let parts = tuple_items(item, 3)?;
    let kind = text(&parts[0])?;
    if kind != "model" && kind != "enum" {
        return Err(invalid.to_owned());
    }
    Ok(EventReference {
        kind,
        index: number(&parts[1])?,
        name: text(&parts[2])?,
    })
}

/// Validate the complete event, including ordinal table, node positions and entry role.
pub fn decode_lowering_event(encoded: &[u8]) -> Result<LoweringEvent, String> {
    let fields = decode_graph_row(encoded, EVENT, &event_description(), 12)?;
    let admitted = flag(&fields[0].value, "lowering event has no admission flag")?;
    let references = items(&fields[8].value)?
        .iter()
        .map(|item| read_reference(item, "invalid lowering event nominal kind"))
        .collect::<Result<Vec<_>, _>>()?;
    let positions = items(&fields[6].value)?
        .iter()
        .map(read_position)
        .collect::<Result<Vec<_>, _>>()?;
    let product = binary(&fields[7].value, "lowering event has no binary product")?;
    let reused = flag(&fields[9].value, "lowering event has no reuse flag")?;
    let role = text(&fields[10].value)?;
    let demand = text(&fields[1].value)?;
    if !(role.is_empty() || role == "entry") || (role == "entry") != !demand.is_empty() {
        return Err("invalid lowering event role or entry demand".into());
    }
    Ok(LoweringEvent {
        admitted,
        demand,
        function: text(&fields[2].value)?,
        identity: text(&fields[3].value)?,
        key: text(&fields[4].value)?,
        module: text(&fields[5].value)?,
        positions,
        product,
        references,
        reused,
        role,
        runtime_requirements: number(&fields[11].value)?,
    })
}

/// The refusal identifies both its declaration and the exact lowering reason.
fn refusal_description() -> Description {
    model_description(
        REFUSAL,
        "incan.lowering-refusal/3",
        &[
            ModelField::new(b"funct_", "function", "string"),
            ModelField::new(b"identi", "identity", "string"),
            ModelField::new(b"reason", "refusal", "string"),
        ],
    )
}

/// Encode one explicit skipped producer.
pub fn encode_lowering_refusal(value: &LoweringRefusal) -> Result<Vec<u8>, String> {
    encode_graph_row(
        REFUSAL,
        &refusal_description(),
        vec![
            required_value(b"funct_", Value::Text(value.function.clone())),
            required_value(b"identi", Value::Text(value.identity.clone())),
            required_value(b"reason", Value::Text(value.refusal.clone())),
        ],
    )
}

/// Authenticate all three fields before accepting an omitted native interface.
pub fn decode_lowering_refusal(encoded: &[u8]) -> Result<LoweringRefusal, String> {
    let fields = decode_graph_row(encoded, REFUSAL, &refusal_description(), 3)?;
    Ok(LoweringRefusal {
        function: text(&fields[0].value)?,
        identity: text(&fields[1].value)?,
        refusal: text(&fields[2].value)?,
    })
}

/// A complete unit interface has one selected entry.
fn interface_description() -> Description {
    model_description(
        INTERFACE,
        "incan.native-interface/3",
        &[
            ModelField::new(b"entry_", "entry", "string"),
            ModelField::new(b"plan__", "plan", "binary"),
        ],
    )
}

/// Bind the keyed entry to the native plan verified by the producer.
pub fn encode_native_interface(value: &NativeInterface) -> Result<Vec<u8>, String> {
    encode_graph_row(
        INTERFACE,
        &interface_description(),
        vec![
            required_value(b"entry_", Value::Text(value.entry.clone())),
            required_value(b"plan__", Value::Binary(value.plan.clone())),
        ],
    )
}

/// Read the complete producer report from its versioned Crumb row.
pub fn decode_native_interface(encoded: &[u8]) -> Result<NativeInterface, String> {
    let fields = decode_graph_row(encoded, INTERFACE, &interface_description(), 2)?;
    let plan = binary(&fields[1].value, "native interface has no binary plan")?;
    Ok(NativeInterface {
        entry: text(&fields[0].value)?,
        plan,
    })
}

/// The filename for definitions already admitted by lowering.
pub fn admitted_definitions_filename() -> String {
    ADMITTED_DEFINITIONS.to_owned()
}

/// A restored function's authenticated native definition inputs.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct AdmittedDefinition {
    pub function: String,
    pub identity: String,
    pub key: String,
    pub digest: String,
}
crate::incan_debug::incan_debug!(AdmittedDefinition {
    function,
    identity,
    key,
    digest
});

/// Each entry includes the same fingerprint inputs as a function that reported a fresh event.
fn definitions_description() -> Description {
    model_description(
        DEFINITIONS,
        "incan.admitted-definitions/1",
        &[ModelField::new(
            b"define",
            "definitions",
            "list<tuple<string,string,string,string>>",
        )],
    )
}

/// Persist the named, restored definitions without duplicating their full products.
pub fn encode_admitted_definitions(definitions: &[AdmittedDefinition]) -> Result<Vec<u8>, String> {
    let values = definitions
        .iter()
        .map(|definition| {
            Value::Tuple(vec![
                Value::Text(definition.function.clone()),
                Value::Text(definition.identity.clone()),
                Value::Text(definition.key.clone()),
                Value::Text(definition.digest.clone()),
            ])
        })
        .collect();
    encode_graph_row(
        DEFINITIONS,
        &definitions_description(),
        vec![required_value(b"define", Value::List(values))],
    )
}

/// Read the exact function identity, key and digest used to populate the native query inputs.
pub fn decode_admitted_definitions(encoded: &[u8]) -> Result<Vec<AdmittedDefinition>, String> {
    let fields = decode_graph_row(encoded, DEFINITIONS, &definitions_description(), 1)?;
    items(&fields[0].value)?
        .iter()
        .map(|value| {
            let parts = tuple_items(value, 4)?;
            Ok(AdmittedDefinition {
                function: text(&parts[0])?,
                identity: text(&parts[1])?,
                key: text(&parts[2])?,
                digest: text(&parts[3])?,
            })
        })
        .collect()
}

/// Current Body IR node coordinates for an admitted scalar in a physically edited file.
#[derive(Clone, PartialEq, Eq)]
pub struct AdmittedPlacement {
    pub key: String,
    pub positions: Vec<EventPosition>,
}
crate::incan_debug::incan_debug!(AdmittedPlacement { key, positions });

/// A restored function's identity and native declaration positions in this request. `placement` defaults to `None`.
#[derive(Clone, PartialEq, Eq)]
pub struct AdmittedFunction {
    pub function: String,
    pub identity: String,
    pub references: Vec<EventReference>,
    pub placement: Option<AdmittedPlacement>,
}
crate::incan_debug::incan_debug!(AdmittedFunction {
    function,
    identity,
    references,
    placement
});

/// Actual restored functions of a named module, including whether every ordinary body was restored. `inspected`
/// defaults to false.
#[derive(Clone, PartialEq, Eq)]
pub struct AdmittedModule {
    pub module: String,
    pub complete: bool,
    pub functions: Vec<AdmittedFunction>,
    pub inspected: bool,
}
crate::incan_debug::incan_debug!(AdmittedModule {
    module,
    complete,
    functions,
    inspected
});

/// The note keeps each function's references grouped with its canonical identity.
fn note_description() -> Description {
    model_description(
        NOTE,
        "incan.module-admission/2",
        &[
            ModelField::new(b"all___", "complete", "boolean"),
            ModelField::new(
                b"funct_",
                "functions",
                "list<tuple<string,string,list<tuple<string,i64,string>>>>",
            ),
            ModelField::new(b"module", "module", "string"),
        ],
    )
}

/// Publish a complete, versioned worker note for native assembly and reuse counting.
pub fn encode_module_note(value: &AdmittedModule) -> Result<Vec<u8>, String> {
    let functions = value
        .functions
        .iter()
        .map(|function| {
            Value::Tuple(vec![
                Value::Text(function.function.clone()),
                Value::Text(function.identity.clone()),
                Value::List(function.references.iter().map(reference_value).collect()),
            ])
        })
        .collect();
    encode_graph_row(
        NOTE,
        &note_description(),
        vec![
            required_value(b"all___", Value::Boolean(value.complete)),
            required_value(b"funct_", Value::List(functions)),
            required_value(b"module", Value::Text(value.module.clone())),
        ],
    )
}

/// Admit one produced note and every restored function before counting the module or restoring its inputs.
pub fn decode_module_note(encoded: &[u8]) -> Result<AdmittedModule, String> {
    let fields = decode_graph_row(encoded, NOTE, &note_description(), 3)?;
    let complete = flag(&fields[0].value, "invalid module admission status")?;
    let mut functions = Vec::new();
    let mut names = HashSet::new();
    for value in items(&fields[1].value)? {
        let parts = tuple_items(value, 3)?;
        let name = text(&parts[0])?;
        if name.is_empty() || !names.insert(name.clone()) {
            return Err("module admission repeated or omitted a function name".into());
        }
        let references = items(&parts[2])?
            .iter()
            .map(|cell| read_reference(cell, "invalid module admission reference kind"))
            .collect::<Result<Vec<_>, _>>()?;
        functions.push(AdmittedFunction {
            function: name,
            identity: text(&parts[1])?,
            references,
            placement: None,
        });
    }
    let module = text(&fields[2].value)?;
    if module.is_empty() || functions.is_empty() {
        return Err("module admission lacks its module or restored function".into());
    }
    Ok(AdmittedModule {
        module,
        complete,
        functions,
        inspected: false,
    })
}

/// Supplement a module admission without changing the existing admission or plan-module formats.
fn placement_description() -> Description {
    model_description(
        PLACEMENT,
        "incan.module-admission-placement/1",
        &[
            ModelField::new(
                b"funct_",
                "functions",
                "list<tuple<string,string,string,list<tuple<i64,i64>>>>",
            ),
            ModelField::new(b"module", "module", "string"),
        ],
    )
}

/// Store only changed placements. Refuse a stale companion if it cannot be removed.
pub fn write_module_placement(path: &Path, note: &AdmittedModule) -> Result<(), String> {
    let functions: Vec<Value> = note
        .functions
        .iter()
        .filter_map(|function| {
            let placement = function.placement.as_ref()?;
            Some(Value::Tuple(vec![
                Value::Text(function.function.clone()),
                Value::Text(function.identity.clone()),
                Value::Text(placement.key.clone()),
                Value::List(placement.positions.iter().map(position_value).collect()),
            ]))
        })
        .collect();
    if functions.is_empty() && !note.inspected {
        if path_is_file(path) {
            path_unlink(path)?;
        }
        return Ok(());
    }
    let encoded = encode_graph_row(
        PLACEMENT,
        &placement_description(),
        vec![
            required_value(b"funct_", Value::List(functions)),
            required_value(b"module", Value::Text(note.module.clone())),
        ],
    )?;
    path_write(path, &encoded)
}

/// Bind a sparse coordinates companion to the exact functions and identities in this admission.
pub fn read_module_admission(path: &Path) -> Result<AdmittedModule, String> {
    let mut note = decode_module_note(&path_read(path)?)?;
    let companion = path_suffixed(path, &".placement".to_owned());
    if !path_is_file(&companion) {
        return Ok(note);
    }
    let encoded = path_read(&companion)?;
    let fields = decode_graph_row(&encoded, PLACEMENT, &placement_description(), 2)?;
    if text(&fields[1].value)? != note.module {
        return Err("placement belongs to a different admitted module".into());
    }
    note.inspected = true;
    let index: HashMap<String, usize> = note
        .functions
        .iter()
        .enumerate()
        .map(|(i, function)| (function.function.clone(), i))
        .collect();
    for value in items(&fields[0].value)? {
        let parts = tuple_items(value, 4)?;
        let name = text(&parts[0])?;
        let Some(&i) = index.get(&name) else {
            return Err("placement names an unreported function".into());
        };
        if note.functions[i].placement.is_some() || note.functions[i].identity != text(&parts[1])? {
            return Err("placement repeats or changes an admitted function".into());
        }
        let nodes = items(&parts[3])?
            .iter()
            .map(read_position)
            .collect::<Result<Vec<_>, _>>()?;
        note.functions[i].placement = Some(AdmittedPlacement {
            key: text(&parts[2])?,
            positions: nodes,
        });
    }
    Ok(note)
}

/// Move only physical placement fields of a product already admitted under its original scalar key.
pub fn placed_admission(
    original: &LoweringEvent,
    module: &str,
    function: &AdmittedFunction,
) -> Result<LoweringEvent, String> {
    let mut event = original.clone();
    if event.function != function.function || event.module != module {
        return Err("admitted lowering event names a different function or module".into());
    }
    if let Some(placement) = &function.placement {
        if placement.key != event.key || placement.positions.len() != event.positions.len() {
            return Err("admitted position map disagrees with its normalized scalar".into());
        }
        event.positions = placement.positions.clone();
    }
    event.identity = function.identity.clone();
    event.references = function.references.clone();
    Ok(event)
}
