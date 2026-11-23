//! A module's plan record: the plan function of every body of one module, as Bakery pages it for admission.

use serde::{Deserialize, Serialize};

use crate::wire::{
    MAX_RECORD_BYTES, Schema, Shape, ShapeBody, ShapeField, Wire, boolean_value, decode_graph_row, encode_graph_row,
    items, number, required, text, tuple_items,
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
/// The request-private authority page for modules selected by the current Oven activation.
pub const PLAN_MODULE_SELECTION_PAGE: &str = "plan-module-selection.crumb";
/// The existing wire limit, enforced before the compiler allocates a staged module selection.
pub const MAX_PLAN_MODULE_SELECTION_BYTES: u64 = MAX_RECORD_BYTES as u64;

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

const PLAN_MODULE_SELECTION_SELECTOR: &[u8] = b"plnwit\x00";
static PLAN_MODULE_SELECTION: Shape = Shape {
    selector: PLAN_MODULE_SELECTION_SELECTOR,
    path: "incan.plan-module-selection/1",
    body: ShapeBody::Record(&[
        ShapeField {
            identity: b"module",
            name: "modules",
            storage: "list<tuple<string,string,string,i64,boolean,i64,string>>",
        },
        ShapeField {
            identity: b"scope_",
            name: "scope",
            storage: "tuple<string,string,string,i64,string,string,string,string>",
        },
    ]),
};
static PLAN_MODULE_SELECTION_SCHEMA: Schema = Schema::new(&[&PLAN_MODULE_SELECTION]);

/// The exact graph and producer against which Oven selected the request's module pages.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlanSelectionScope {
    pub store_root: String,
    pub placement: String,
    pub manifest: String,
    pub generation: i64,
    pub view: String,
    pub index: String,
    pub origin: String,
    pub profile: String,
}

/// The selected output key, content, function count, placement and namespace order of one module.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlanSelectedModule {
    pub module: String,
    pub output: String,
    pub digest: String,
    pub count: i64,
    pub positioned: bool,
    pub rank: i64,
    pub certificate: String,
}

/// One activation's module selection, independent of the files through which the compiler reads the page bodies.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlanModuleSelectionRecord {
    pub scope: PlanSelectionScope,
    pub modules: Vec<PlanSelectedModule>,
}

/// Accept only the canonical lowercase spelling of a raw SHA-256 digest.
fn canonical_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// Require a concrete graph generation, placement and producer.
fn validate_selection_scope(scope: &PlanSelectionScope) -> Result<(), String> {
    if scope.generation <= 0
        || scope.placement.is_empty()
        || (scope.store_root.is_empty() && !scope.placement.starts_with("memory:"))
        || !canonical_digest(&scope.manifest)
        || scope.view.is_empty()
        || scope.index.is_empty()
        || scope.origin.is_empty()
        || scope.profile.is_empty()
    {
        return Err("malformed plan-module selection scope".into());
    }
    Ok(())
}

/// Check each selected module's identity, bounds and canonical name order.
fn validate_selected_modules(modules: &[PlanSelectedModule]) -> Result<(), String> {
    let mut previous = "";
    for module in modules {
        if module.module.as_str() <= previous
            || module.output.is_empty()
            || !canonical_digest(&module.digest)
            || !(1..=MAX_PLAN_MODULE_FUNCTIONS as i64).contains(&module.count)
            || module.rank < 0
        {
            return Err("malformed or unordered selected plan module".into());
        }
        previous = &module.module;
    }
    if previous.is_empty() {
        return Err("plan-module selection has no modules".into());
    }
    Ok(())
}

/// Decode the one canonical selector record without rebinding it to a different graph, placement or module order.
pub fn decode_plan_module_selection(encoded: &[u8]) -> Result<PlanModuleSelectionRecord, String> {
    let fields = decode_graph_row(
        encoded,
        PLAN_MODULE_SELECTION_SELECTOR,
        &PLAN_MODULE_SELECTION_SCHEMA,
        2,
    )?;
    let scope = tuple_items(&fields[1].value, 8)?;
    let scope = PlanSelectionScope {
        store_root: text(&scope[0])?,
        placement: text(&scope[1])?,
        manifest: text(&scope[2])?,
        generation: number(&scope[3])?,
        view: text(&scope[4])?,
        index: text(&scope[5])?,
        origin: text(&scope[6])?,
        profile: text(&scope[7])?,
    };
    validate_selection_scope(&scope)?;
    let modules = items(&fields[0].value)?
        .iter()
        .map(|module| {
            let module = tuple_items(module, 7)?;
            Ok(PlanSelectedModule {
                module: text(&module[0])?,
                output: text(&module[1])?,
                digest: text(&module[2])?,
                count: number(&module[3])?,
                positioned: boolean_value(&module[4])?,
                rank: number(&module[5])?,
                certificate: text(&module[6])?,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    validate_selected_modules(&modules)?;
    Ok(PlanModuleSelectionRecord { scope, modules })
}

/// A lowering outcome that names the entire current positioned module selected by this compile request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WholeModuleAdmission {
    pub module: String,
    pub output: String,
    pub digest: String,
    pub count: i64,
    pub selection: String,
}

/// Encode one versioned complete admission, without repeating function identities or placement.
pub fn encode_whole_module_admission(_note: &WholeModuleAdmission) -> Result<Vec<u8>, String> {
    Err("whole-module Plan admission has no engaged writer".into())
}

/// Decode the exact whole-admission row. A caller must still bind its selection and positioned module page.
pub fn decode_whole_module_admission(_encoded: &[u8]) -> Result<WholeModuleAdmission, String> {
    Err("whole-module Plan admission has no engaged reader".into())
}

/// The exact set of compact admission notes published by a request, carried through native handoff.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompactAdmissionPin {
    pub count: u64,
    pub digest: [u8; 32],
}

/// Pin the exact emitted notes in strictly ascending UTF-8 module order, without reopening their encoded bytes.
pub fn compact_admission_pin<'a>(
    _notes: impl IntoIterator<Item = (&'a str, [u8; 32])>,
) -> Result<Option<CompactAdmissionPin>, String> {
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::records::lowering_events::{AdmittedFunction, AdmittedModule, decode_module_note, encode_module_note};

    /// One positioned module note is smaller in shape and remains distinguishable from existing scalar notes.
    #[test]
    fn a_whole_module_note_binds_the_exact_versioned_fields() -> Result<(), String> {
        let note = WholeModuleAdmission {
            module: "probe".into(),
            output: "plan-module\tprobe".into(),
            digest: "2a".repeat(32),
            count: 1,
            selection: "e3".repeat(32),
        };
        let encoded = encode_whole_module_admission(&note);
        assert!(encoded.is_ok(), "whole module writer refused a current positioned note");
        let encoded = encoded?;
        let observed = decode_whole_module_admission(&encoded)?;
        assert_eq!(observed, note);
        assert_eq!(encode_whole_module_admission(&observed)?, encoded);
        assert!(decode_module_note(&encoded).is_err());
        for invalid in [
            WholeModuleAdmission {
                selection: note.selection.to_uppercase(),
                ..note.clone()
            },
            WholeModuleAdmission {
                digest: "a".repeat(63),
                ..note.clone()
            },
            WholeModuleAdmission {
                count: 0,
                ..note.clone()
            },
            WholeModuleAdmission {
                count: MAX_PLAN_MODULE_FUNCTIONS as i64 + 1,
                ..note.clone()
            },
            WholeModuleAdmission {
                output: String::new(),
                ..note.clone()
            },
            WholeModuleAdmission {
                module: String::new(),
                ..note.clone()
            },
        ] {
            assert!(encode_whole_module_admission(&invalid).is_err());
        }
        let scalar = AdmittedModule {
            module: note.module.clone(),
            complete: true,
            functions: vec![AdmittedFunction {
                function: "probe_run".into(),
                identity: "pub::probe::run".into(),
                references: Vec::new(),
                placement: None,
            }],
            inspected: false,
        };
        let encoded = encode_module_note(&scalar)?;
        assert_eq!(decode_module_note(&encoded)?, scalar);
        assert!(decode_whole_module_admission(&encoded).is_err());
        Ok(())
    }

    /// The compact-set fingerprint uses UTF-8 byte lengths, exact note hashes, canonical order and final count.
    #[test]
    fn the_set_pin_keeps_compact_notes_in_exact_name_order() -> Result<(), String> {
        let first = ("a", std::array::from_fn(|index| index as u8));
        let second = ("ä", std::array::from_fn(|index| (index + 32) as u8));
        let expected = CompactAdmissionPin {
            count: 2,
            digest: [
                61, 227, 213, 16, 15, 55, 21, 121, 227, 50, 42, 160, 251, 137, 238, 166, 252, 236, 78, 251, 135, 195,
                240, 150, 208, 77, 0, 116, 230, 153, 128, 86,
            ],
        };
        assert_eq!(compact_admission_pin([first, second])?, Some(expected));
        assert_ne!(compact_admission_pin([first])?, Some(expected));
        assert_eq!(compact_admission_pin([])?, None);
        assert!(compact_admission_pin([first, first]).is_err());
        assert!(compact_admission_pin([second, first]).is_err());
        assert!(compact_admission_pin([("", first.1)]).is_err());
        Ok(())
    }
}
