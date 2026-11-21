//! Request-local transport for named lowering products, explicit refusals, the complete native interface and the
//! notes of admitted modules: one Crumb graph row each.

use std::collections::{BTreeSet, HashMap};
use std::path::Path;

use crate::wire::{
    Schema, Shape, ShapeBody, ShapeField, Wire, WireField, boolean_value, decode_graph_row, encode_graph_row, items,
    number, required, text, tuple_items,
};

const EVENT: &[u8] = b"lowevt\x00";
const REFUSAL: &[u8] = b"lowref\x00";
const INTERFACE: &[u8] = b"lowint\x00";
const DEFINITIONS: &[u8] = b"admdef\x00";
const NOTE: &[u8] = b"admnot\x00";
const PLACEMENT: &[u8] = b"admgeo\x00";

/// The filename of the native interface report, shared by the lowering publisher and native reader.
pub const INTERFACE_REPORT: &str = "native-interface.crumb";
/// The filename for definitions already admitted by lowering.
pub const ADMITTED_DEFINITIONS: &str = "admitted-definitions.crumb";

/// One field of a record shape.
const fn field(identity: &'static [u8], name: &'static str, storage: &'static str) -> ShapeField {
    ShapeField {
        identity,
        name,
        storage,
    }
}

static EVENT_SHAPE: Shape = Shape {
    selector: EVENT,
    path: "incan.lowering-event/3",
    body: ShapeBody::Record(&[
        field(b"admitt", "admitted", "boolean"),
        field(b"demand", "demand", "string"),
        field(b"funct_", "function", "string"),
        field(b"identi", "identity", "string"),
        field(b"key___", "key", "string"),
        field(b"module", "module", "string"),
        field(b"positi", "positions", "list<tuple<i64,i64>>"),
        field(b"produc", "product", "binary"),
        field(b"refere", "references", "list<tuple<string,i64,string>>"),
        field(b"reused", "reused", "boolean"),
        field(b"role__", "role", "string"),
        field(b"runtim", "runtime_requirements", "i64"),
    ]),
};
static EVENT_SCHEMA: Schema = Schema::new(&[&EVENT_SHAPE]);

static REFUSAL_SHAPE: Shape = Shape {
    selector: REFUSAL,
    path: "incan.lowering-refusal/3",
    body: ShapeBody::Record(&[
        field(b"funct_", "function", "string"),
        field(b"identi", "identity", "string"),
        field(b"reason", "refusal", "string"),
    ]),
};
static REFUSAL_SCHEMA: Schema = Schema::new(&[&REFUSAL_SHAPE]);

static INTERFACE_SHAPE: Shape = Shape {
    selector: INTERFACE,
    path: "incan.native-interface/3",
    body: ShapeBody::Record(&[field(b"entry_", "entry", "string"), field(b"plan__", "plan", "binary")]),
};
static INTERFACE_SCHEMA: Schema = Schema::new(&[&INTERFACE_SHAPE]);

static DEFINITIONS_SHAPE: Shape = Shape {
    selector: DEFINITIONS,
    path: "incan.admitted-definitions/1",
    body: ShapeBody::Record(&[field(
        b"define",
        "definitions",
        "list<tuple<string,string,string,string>>",
    )]),
};
static DEFINITIONS_SCHEMA: Schema = Schema::new(&[&DEFINITIONS_SHAPE]);

static NOTE_SHAPE: Shape = Shape {
    selector: NOTE,
    path: "incan.module-admission/2",
    body: ShapeBody::Record(&[
        field(b"all___", "complete", "boolean"),
        field(
            b"funct_",
            "functions",
            "list<tuple<string,string,list<tuple<string,i64,string>>>>",
        ),
        field(b"module", "module", "string"),
    ]),
};
static NOTE_SCHEMA: Schema = Schema::new(&[&NOTE_SHAPE]);

static PLACEMENT_SHAPE: Shape = Shape {
    selector: PLACEMENT,
    path: "incan.module-admission-placement/1",
    body: ShapeBody::Record(&[
        field(
            b"funct_",
            "functions",
            "list<tuple<string,string,string,list<tuple<i64,i64>>>>",
        ),
        field(b"module", "module", "string"),
    ]),
};
static PLACEMENT_SCHEMA: Schema = Schema::new(&[&PLACEMENT_SHAPE]);

/// A named ordinal's position in the current model or enum table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EventReference {
    pub kind: String,
    pub index: i64,
    pub name: String,
}

/// A Body IR node's current source coordinates.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EventPosition {
    pub line: i64,
    pub column: i64,
}

/// The exact published product and its use in this request.
#[derive(Clone, Debug, Default, PartialEq)]
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

/// An explicit per-function reason lowering could not publish a reusable product.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoweringRefusal {
    pub identity: String,
    pub function: String,
    pub refusal: String,
}

/// The standalone entry key and its complete loss-checked native plan.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NativeInterface {
    pub entry: String,
    pub plan: Vec<u8>,
}

/// A restored function's authenticated native definition inputs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AdmittedDefinition {
    pub function: String,
    pub identity: String,
    pub key: String,
    pub digest: String,
}

/// Current Body IR node coordinates for an admitted scalar in a physically edited file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AdmittedPlacement {
    pub key: String,
    pub positions: Vec<EventPosition>,
}

/// A restored function's identity and native declaration positions in this request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AdmittedFunction {
    pub function: String,
    pub identity: String,
    pub references: Vec<EventReference>,
    pub placement: Option<AdmittedPlacement>,
}

/// Actual restored functions of a named module, including whether every ordinary body was restored.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AdmittedModule {
    pub module: String,
    pub complete: bool,
    pub functions: Vec<AdmittedFunction>,
    pub inspected: bool,
}

/// A reference as its Crumb tuple: kind, index, native name.
fn reference_value(reference: &EventReference) -> Wire {
    Wire::Tuple(vec![
        Wire::Text(reference.kind.clone()),
        Wire::integer(reference.index),
        Wire::Text(reference.name.clone()),
    ])
}

/// A position as its Crumb tuple: line, column.
fn position_value(position: &EventPosition) -> Wire {
    Wire::Tuple(vec![Wire::integer(position.line), Wire::integer(position.column)])
}

/// Read one reference tuple, refusing a kind other than a model or enum.
fn read_reference(item: &Wire, refusal: &str) -> Result<EventReference, String> {
    let parts = tuple_items(item, 3)?;
    let kind = text(&parts[0])?;
    if kind != "model" && kind != "enum" {
        return Err(refusal.to_owned());
    }
    Ok(EventReference {
        kind,
        index: number(&parts[1])?,
        name: text(&parts[2])?,
    })
}

/// Read a list of position tuples.
fn read_positions(value: &Wire) -> Result<Vec<EventPosition>, String> {
    items(value)?
        .iter()
        .map(|item| {
            let pair = tuple_items(item, 2)?;
            Ok(EventPosition {
                line: number(&pair[0])?,
                column: number(&pair[1])?,
            })
        })
        .collect()
}

/// Require a binary field.
fn binary(value: &Wire, refusal: &str) -> Result<Vec<u8>, String> {
    match value {
        Wire::Binary(data) => Ok(data.clone()),
        _ => Err(refusal.to_owned()),
    }
}

/// Transport the authenticated plan-function record without reconstructing or transcoding it.
pub fn encode_lowering_event(event: &LoweringEvent) -> Result<Vec<u8>, String> {
    encode_graph_row(
        EVENT,
        &EVENT_SCHEMA,
        vec![
            required(b"admitt", Wire::Boolean(event.admitted)),
            required(b"demand", Wire::Text(event.demand.clone())),
            required(b"funct_", Wire::Text(event.function.clone())),
            required(b"identi", Wire::Text(event.identity.clone())),
            required(b"key___", Wire::Text(event.key.clone())),
            required(b"module", Wire::Text(event.module.clone())),
            required(
                b"positi",
                Wire::List(event.positions.iter().map(position_value).collect()),
            ),
            required(b"produc", Wire::Binary(event.product.clone())),
            required(
                b"refere",
                Wire::List(event.references.iter().map(reference_value).collect()),
            ),
            required(b"reused", Wire::Boolean(event.reused)),
            required(b"role__", Wire::Text(event.role.clone())),
            required(b"runtim", Wire::integer(event.runtime_requirements)),
        ],
    )
}

/// Validate the complete event, including ordinal table, node positions and entry role.
pub fn decode_lowering_event(encoded: &[u8]) -> Result<LoweringEvent, String> {
    let fields = decode_graph_row(encoded, EVENT, &EVENT_SCHEMA, 12)?;
    let admitted = boolean_value(&fields[0].value).map_err(|_| "lowering event has no admission flag".to_owned())?;
    let references = items(&fields[8].value)?
        .iter()
        .map(|item| read_reference(item, "invalid lowering event nominal kind"))
        .collect::<Result<Vec<_>, _>>()?;
    let positions = read_positions(&fields[6].value)?;
    let product = binary(&fields[7].value, "lowering event has no binary product")?;
    let reused = boolean_value(&fields[9].value).map_err(|_| "lowering event has no reuse flag".to_owned())?;
    let role = text(&fields[10].value)?;
    let demand = text(&fields[1].value)?;
    if (role != "" && role != "entry") || (role == "entry") != !demand.is_empty() {
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

/// Encode one explicit skipped producer.
pub fn encode_lowering_refusal(value: &LoweringRefusal) -> Result<Vec<u8>, String> {
    encode_graph_row(
        REFUSAL,
        &REFUSAL_SCHEMA,
        vec![
            required(b"funct_", Wire::Text(value.function.clone())),
            required(b"identi", Wire::Text(value.identity.clone())),
            required(b"reason", Wire::Text(value.refusal.clone())),
        ],
    )
}

/// Authenticate all three fields before accepting an omitted native interface.
pub fn decode_lowering_refusal(encoded: &[u8]) -> Result<LoweringRefusal, String> {
    let fields = decode_graph_row(encoded, REFUSAL, &REFUSAL_SCHEMA, 3)?;
    Ok(LoweringRefusal {
        function: text(&fields[0].value)?,
        identity: text(&fields[1].value)?,
        refusal: text(&fields[2].value)?,
    })
}

/// Bind the keyed entry to the native plan verified by the producer.
pub fn encode_native_interface(value: &NativeInterface) -> Result<Vec<u8>, String> {
    encode_graph_row(
        INTERFACE,
        &INTERFACE_SCHEMA,
        vec![
            required(b"entry_", Wire::Text(value.entry.clone())),
            required(b"plan__", Wire::Binary(value.plan.clone())),
        ],
    )
}

/// Read the complete producer report from its versioned Crumb row.
pub fn decode_native_interface(encoded: &[u8]) -> Result<NativeInterface, String> {
    let fields = decode_graph_row(encoded, INTERFACE, &INTERFACE_SCHEMA, 2)?;
    Ok(NativeInterface {
        entry: text(&fields[0].value)?,
        plan: binary(&fields[1].value, "native interface has no binary plan")?,
    })
}

/// Persist the named, restored definitions without duplicating their full products.
pub fn encode_admitted_definitions(definitions: &[AdmittedDefinition]) -> Result<Vec<u8>, String> {
    let values = definitions
        .iter()
        .map(|definition| {
            Wire::Tuple(vec![
                Wire::Text(definition.function.clone()),
                Wire::Text(definition.identity.clone()),
                Wire::Text(definition.key.clone()),
                Wire::Text(definition.digest.clone()),
            ])
        })
        .collect();
    encode_graph_row(
        DEFINITIONS,
        &DEFINITIONS_SCHEMA,
        vec![required(b"define", Wire::List(values))],
    )
}

/// Read the exact function identity, key and digest used to populate the native query inputs.
pub fn decode_admitted_definitions(encoded: &[u8]) -> Result<Vec<AdmittedDefinition>, String> {
    let fields = decode_graph_row(encoded, DEFINITIONS, &DEFINITIONS_SCHEMA, 1)?;
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

/// Publish a complete, versioned worker note for native assembly and reuse counting.
pub fn encode_module_note(value: &AdmittedModule) -> Result<Vec<u8>, String> {
    let functions = value
        .functions
        .iter()
        .map(|function| {
            Wire::Tuple(vec![
                Wire::Text(function.function.clone()),
                Wire::Text(function.identity.clone()),
                Wire::List(function.references.iter().map(reference_value).collect()),
            ])
        })
        .collect();
    encode_graph_row(
        NOTE,
        &NOTE_SCHEMA,
        vec![
            required(b"all___", Wire::Boolean(value.complete)),
            required(b"funct_", Wire::List(functions)),
            required(b"module", Wire::Text(value.module.clone())),
        ],
    )
}

/// Admit one produced note and every restored function before counting the module or restoring its inputs.
pub fn decode_module_note(encoded: &[u8]) -> Result<AdmittedModule, String> {
    let fields = decode_graph_row(encoded, NOTE, &NOTE_SCHEMA, 3)?;
    let complete = boolean_value(&fields[0].value).map_err(|_| "invalid module admission status".to_owned())?;
    let mut functions = Vec::new();
    let mut names = BTreeSet::new();
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

/// Store only changed placements. Refuse a stale companion if it cannot be removed.
pub fn write_module_placement(path: &Path, note: &AdmittedModule) -> Result<(), String> {
    let mut functions: Vec<Wire> = Vec::new();
    for function in &note.functions {
        if let Some(placement) = &function.placement {
            functions.push(Wire::Tuple(vec![
                Wire::Text(function.function.clone()),
                Wire::Text(function.identity.clone()),
                Wire::Text(placement.key.clone()),
                Wire::List(placement.positions.iter().map(position_value).collect()),
            ]));
        }
    }
    if functions.is_empty() && !note.inspected {
        if path.is_file() {
            std::fs::remove_file(path).map_err(|error| error.to_string())?;
        }
        return Ok(());
    }
    let fields: Vec<WireField> = vec![
        required(b"funct_", Wire::List(functions)),
        required(b"module", Wire::Text(note.module.clone())),
    ];
    let encoded = encode_graph_row(PLACEMENT, &PLACEMENT_SCHEMA, fields)?;
    std::fs::write(path, encoded).map_err(|error| error.to_string())
}

/// Bind a sparse coordinates companion to the exact functions and identities in this admission.
pub fn read_module_admission(path: &Path) -> Result<AdmittedModule, String> {
    let mut note = decode_module_note(&std::fs::read(path).map_err(|error| error.to_string())?)?;
    let companion = Path::new(&format!("{}.placement", path.display())).to_path_buf();
    if !companion.is_file() {
        return Ok(note);
    }
    let encoded = std::fs::read(&companion).map_err(|error| error.to_string())?;
    let fields = decode_graph_row(&encoded, PLACEMENT, &PLACEMENT_SCHEMA, 2)?;
    if text(&fields[1].value)? != note.module {
        return Err("placement belongs to a different admitted module".into());
    }
    note.inspected = true;
    let index: HashMap<String, usize> = note
        .functions
        .iter()
        .enumerate()
        .map(|(position, function)| (function.function.clone(), position))
        .collect();
    for value in items(&fields[0].value)? {
        let parts = tuple_items(value, 4)?;
        let name = text(&parts[0])?;
        let Some(position) = index.get(&name).copied() else {
            return Err("placement names an unreported function".into());
        };
        let function = &mut note.functions[position];
        if function.placement.is_some() || function.identity != text(&parts[1])? {
            return Err("placement repeats or changes an admitted function".into());
        }
        function.placement = Some(AdmittedPlacement {
            key: text(&parts[2])?,
            positions: read_positions(&parts[3])?,
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
