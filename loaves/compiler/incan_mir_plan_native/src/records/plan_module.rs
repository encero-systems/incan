//! A module's plan record: the plan function of every body of one module, as Bakery pages it for admission.

use std::collections::HashSet;

use internal_models_bindings::{MappedView, view_len};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::records::lowering_events::{AdmittedDefinition, projected_definition};
use crate::wire::{
    MAX_RECORD_BYTES, Schema, Shape, ShapeBody, ShapeField, Wire, WireField, admit_root_view, boolean_value,
    decode_graph_row, encode_graph_row, field, items, number, required, text, tuple_items,
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
    if function.event.is_empty() {
        return Err("malformed plan-module record".into());
    }
    member_order(
        &function.identity,
        &function.symbol,
        &function.function,
        &function.key,
        previous,
    )
}

/// Give owned and projected Plan members the same canonical owner and function order.
fn member_order(identity: &str, symbol: &str, function: &str, key: &str, previous: &str) -> Result<String, String> {
    let order = format!("{identity}\n{function}");
    if [identity, symbol, function, key].iter().any(|field| field.is_empty()) || order.as_str() <= previous {
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

/// Project the native definition inputs from one authenticated, positioned module page.
pub fn read_positioned_plan_members(
    view: &MappedView,
    selected: &PlanSelectedModule,
) -> Result<Vec<AdmittedDefinition>, String> {
    validate_selected_modules(std::slice::from_ref(selected))?;
    if !selected.positioned || view_len(view) > MAX_RECORD_BYTES {
        return Err("selected Plan module lacks a bounded positioned page".into());
    }
    if format!("{:x}", Sha256::digest(view.0.as_bytes())) != selected.digest {
        return Err("positioned Plan bytes do not match the selected page digest".into());
    }
    let mut page = admit_root_view(
        view,
        PLAN_MODULE_SELECTOR,
        &PLAN_MODULE_SCHEMA,
        "positioned Plan module",
    )?;
    let module = text(&page.row.read_field(b"module")?)?;
    if module != selected.module {
        return Err("positioned Plan page differs from its selected module name".into());
    }
    let functions = page.row.read_field(b"funcs_")?;
    let fields = items(&functions)?.iter().map(text).collect::<Result<Vec<_>, _>>()?;
    if fields.len() != selected.count as usize * FUNCTION_FIELDS {
        return Err("positioned Plan declarations differ from the selected count".into());
    }
    let events = page.binary_list_views(b"events", selected.count)?;
    let mut names = HashSet::with_capacity(events.len());
    let mut definitions = Vec::with_capacity(events.len());
    let mut previous = String::new();
    for (member, event) in fields.chunks_exact(FUNCTION_FIELDS).zip(events) {
        let [identity, symbol, function, key] = member else {
            return Err("malformed positioned Plan module member".into());
        };
        previous = member_order(identity, symbol, function, key, &previous)?;
        if !canonical_digest(key) || !names.insert(function) {
            return Err("positioned Plan page repeats a function or has a malformed member key".into());
        }
        definitions.push(projected_definition(&event, &module, function, symbol, key)?);
    }
    Ok(definitions)
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

/// The Crumb reference selector declared by a complete positioned module note.
pub const WHOLE_ADMISSION_SELECTOR: &[u8] = b"admwhl\x00";
pub(crate) static WHOLE_ADMISSION: Shape = Shape {
    selector: WHOLE_ADMISSION_SELECTOR,
    path: "incan.module-admission-whole/1",
    body: ShapeBody::Record(&[
        field(b"count_", "count", "i64"),
        field(b"digest", "digest", "string"),
        field(b"module", "module", "string"),
        field(b"output", "output", "string"),
        field(b"select", "selection", "string"),
    ]),
};
static WHOLE_ADMISSION_SCHEMA: Schema = Schema::new(&[&WHOLE_ADMISSION]);

/// A lowering outcome that names the entire current positioned module selected by this compile request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WholeModuleAdmission {
    pub module: String,
    pub output: String,
    pub digest: String,
    pub count: i64,
    pub selection: String,
}

impl WholeModuleAdmission {
    /// Check the wire fields before joining this outcome to its independent selection and staged page.
    fn validate(&self) -> Result<(), String> {
        if self.module.is_empty()
            || self.output.is_empty()
            || !canonical_digest(&self.digest)
            || !canonical_digest(&self.selection)
            || !(1..=MAX_PLAN_MODULE_FUNCTIONS as i64).contains(&self.count)
        {
            return Err("malformed whole-module Plan admission".into());
        }
        Ok(())
    }
}

/// Encode one versioned complete admission, without repeating function identities or placement.
pub fn encode_whole_module_admission(note: &WholeModuleAdmission) -> Result<Vec<u8>, String> {
    note.validate()?;
    encode_graph_row(
        WHOLE_ADMISSION_SELECTOR,
        &WHOLE_ADMISSION_SCHEMA,
        vec![
            required(b"count_", Wire::Signed(i128::from(note.count))),
            required(b"digest", Wire::Text(note.digest.clone())),
            required(b"module", Wire::Text(note.module.clone())),
            required(b"output", Wire::Text(note.output.clone())),
            required(b"select", Wire::Text(note.selection.clone())),
        ],
    )
}

/// Decode the exact whole-admission row. A caller must still bind its selection and positioned module page.
pub fn decode_whole_module_admission(encoded: &[u8]) -> Result<WholeModuleAdmission, String> {
    let fields = decode_graph_row(encoded, WHOLE_ADMISSION_SELECTOR, &WHOLE_ADMISSION_SCHEMA, 5)?;
    whole_admission_fields(&fields)
}

/// Interpret the shared whole-admission fields once the exact versioned root has been admitted.
pub(crate) fn whole_admission_fields(fields: &[WireField]) -> Result<WholeModuleAdmission, String> {
    let [count, digest, module, output, selection] = fields else {
        return Err("whole-module Plan admission has the wrong field count".into());
    };
    let note = WholeModuleAdmission {
        count: number(&count.value)?,
        digest: text(&digest.value)?,
        module: text(&module.value)?,
        output: text(&output.value)?,
        selection: text(&selection.value)?,
    };
    note.validate()?;
    Ok(note)
}

/// The exact set of compact admission notes published by a request, carried through native handoff.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompactAdmissionPin {
    pub count: u64,
    pub digest: [u8; 32],
}

/// Pin the exact emitted notes in strictly ascending UTF-8 module order, without reopening their encoded bytes.
pub fn compact_admission_pin<'a>(
    notes: impl IntoIterator<Item = (&'a str, [u8; 32])>,
) -> Result<Option<CompactAdmissionPin>, String> {
    let mut hash = Sha256::new();
    hash.update(b"incan.plan-compact-admissions/1\x00");
    let mut count = 0u64;
    let mut previous = "";
    for (module, note_digest) in notes {
        if module <= previous {
            return Err("compact admissions repeat a module or differ from canonical name order".into());
        }
        hash.update((module.len() as u64).to_be_bytes());
        hash.update(module.as_bytes());
        hash.update(note_digest);
        count += 1;
        previous = module;
    }
    if count == 0 {
        return Ok(None);
    }
    hash.update(count.to_be_bytes());
    Ok(Some(CompactAdmissionPin {
        count,
        digest: hash.finalize().into(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::records::lowering_events::{
        AdmittedFunction, AdmittedModule, LoweringEvent, decode_lowering_event, decode_module_note,
        encode_lowering_event, encode_module_note,
    };
    use crate::wire::take_materialized_binary_bytes;
    use crate::{
        Function, Local, PlanFunctionRecord, PlanType, RvalueKind, TerminatorKind, assign, block,
        decode_plan_function_record, encode_plan_function_record, encode_product, integer, place, plan_function_digest,
        source_span,
    };
    use internal_models_bindings::{
        binary_list_field_views, binary_message, complete_view, create_budget, crumb_message, mapped_bytes,
        view_request,
    };

    /// Build a real positioned event whose owned decode would materialize its complete native function product.
    fn projected_event() -> Result<PlanModuleFunction, String> {
        let span = source_span("probe.incn", 11, 5);
        let body = block(
            vec![assign(place(0, &span), RvalueKind::Use(integer(10, &span)), &span)],
            TerminatorKind::Return,
            &span,
        );
        let function = Function::new(
            "probe_run".into(),
            Vec::new(),
            PlanType::Int,
            vec![Local {
                ty: PlanType::Int,
                span: span.clone(),
            }],
            vec![body],
            span,
        );
        let encoded = encode_product(&function, &[], &[], &[])?;
        let product = encode_plan_function_record(&PlanFunctionRecord {
            inputs: vec!["probe".into()],
            digest: plan_function_digest(&encoded, &[])?,
            function: encoded,
            requests: Vec::new(),
        })?;
        let key = "2a".repeat(32);
        let symbol = "pub::probe::run";
        let event = encode_lowering_event(&LoweringEvent {
            function: "probe_run".into(),
            module: "probe".into(),
            identity: symbol.into(),
            product,
            key: key.clone(),
            reused: true,
            ..LoweringEvent::default()
        })?;
        Ok(PlanModuleFunction {
            identity: "function:probe::run|signature".into(),
            symbol: symbol.into(),
            function: "probe_run".into(),
            key,
            event,
        })
    }

    /// One admitted module field keeps its actual event bytes at the existing anchor, with work and range refusals.
    #[test]
    fn admitted_plan_events_keep_their_immutable_backing() -> Result<(), String> {
        let function = projected_event()?;
        let event = function.event.clone();
        let encoded = encode_plan_module(&PlanModuleValue {
            module: "probe".into(),
            functions: vec![function],
        })?;
        let _ = take_materialized_binary_bytes();
        let decoded = decode_plan_module(&encoded)?;
        assert_eq!(decoded.functions[0].event, event);
        let old_event = decode_lowering_event(&decoded.functions[0].event)?;
        let old_product = decode_plan_function_record(&old_event.product)?;
        let body_bytes = old_product.function.len();
        assert!(body_bytes > 0);
        assert_eq!(
            take_materialized_binary_bytes(),
            event.len() + old_event.product.len() + body_bytes
        );

        let owner = mapped_bytes(&encoded)?;
        let backing = complete_view(&owner).ok_or("fixture has no bounded module view")?;
        let mut request = view_request(&backing, false, true);
        for description in PLAN_MODULE_SCHEMA.descriptions() {
            request.supply(description.clone());
        }
        request.allow(1_048_576, 1_048_576, 128);
        let admitted = request.admit();
        assert!(admitted.admitted(), "{}", admitted.category);
        let budget = |bytes| create_budget(bytes, 1_048_576, 128).map_err(|error| crumb_message(&error));
        let field = b"events".to_vec();
        let views = binary_list_field_views(&admitted, &backing, &field, 1, &mut budget(1_048_576)?)
            .map_err(|error| binary_message(&error));
        assert!(views.is_ok(), "the admitted module's exact event was not projected");
        let [view]: [_; 1] = views?.try_into().map_err(|_| "module projection changed event count")?;
        let source = backing.0.as_bytes();
        let kept = view.0.as_bytes();
        let offset = kept
            .as_ptr()
            .addr()
            .checked_sub(source.as_ptr().addr())
            .ok_or("shared view left parent")?;
        assert_eq!(source.get(offset..offset + kept.len()), Some(kept));
        assert_eq!(kept, event);
        assert_eq!(take_materialized_binary_bytes(), 0);

        for (count, bytes, reason) in [(2, 1_048_576, "arity_mismatch"), (1, 1, "limit_exceeded")] {
            let refused = binary_list_field_views(&admitted, &backing, &field, count, &mut budget(bytes)?)
                .map_err(|error| binary_message(&error));
            assert!(matches!(refused, Err(message) if message.contains(reason)));
        }
        let clipped_owner = mapped_bytes(&source[..4].to_vec())?;
        let clipped = complete_view(&clipped_owner).ok_or("fixture clip refused")?;
        let refused = binary_list_field_views(&admitted, &clipped, &field, 1, &mut budget(1_048_576)?);
        assert!(matches!(refused, Err(error) if binary_message(&error).contains("invalid_offsets")));
        Ok(())
    }

    /// Member projection validates the selected page digest and returns only its exact native fingerprints.
    #[test]
    fn selected_positioned_members_need_the_exact_admitted_backing() -> Result<(), String> {
        let function = projected_event()?;
        let prior = decode_lowering_event(&function.event)?;
        let old = decode_plan_function_record(&prior.product)?;
        let expected = AdmittedDefinition {
            function: function.function.clone(),
            identity: function.symbol.clone(),
            key: function.key.clone(),
            digest: old.digest,
        };
        assert_ne!(function.identity, expected.identity);
        let mut page = PlanModuleValue {
            module: "probe".into(),
            functions: vec![function],
        };
        let encoded = encode_plan_module(&page)?;
        let selected = PlanSelectedModule {
            module: page.module.clone(),
            output: "probe-positioned-module".into(),
            digest: format!("{:x}", Sha256::digest(&encoded)),
            count: 1,
            positioned: true,
            rank: 1,
            certificate: "compiled-check".into(),
        };
        let owner = mapped_bytes(&encoded)?;
        let view = complete_view(&owner).ok_or("no mapped fixture page")?;
        let _ = take_materialized_binary_bytes();
        let projected = read_positioned_plan_members(&view, &selected);
        assert!(
            projected.is_ok(),
            "the exact positioned selection did not resolve its native member"
        );
        assert_eq!(projected?, [expected]);
        assert_eq!(take_materialized_binary_bytes(), 0);

        let wrong_arity = PlanSelectedModule {
            count: 2,
            ..selected.clone()
        };
        assert!(read_positioned_plan_members(&view, &wrong_arity).is_err());
        let repositioned = PlanSelectedModule {
            positioned: false,
            ..selected.clone()
        };
        assert!(read_positioned_plan_members(&view, &repositioned).is_err());

        let mut damaged = encoded.clone();
        let last = damaged.last_mut().ok_or("fixture page unexpectedly empty")?;
        *last ^= 1;
        assert_eq!(damaged.len(), encoded.len());
        page.module = "foreign".into();
        let foreign = encode_plan_module(&page)?;
        assert_eq!(decode_plan_module(&foreign)?.module, page.module);
        for offered in [damaged, foreign.clone()] {
            let owner = mapped_bytes(&offered)?;
            let offered = complete_view(&owner).ok_or("fixture backing missing")?;
            assert!(
                matches!(read_positioned_plan_members(&offered, &selected),
                         Err(reason) if reason.contains("selected page digest")),
                "same-size damaged or coherent foreign bytes were admitted under the old SHA"
            );
        }
        let foreign_selected = PlanSelectedModule {
            digest: format!("{:x}", Sha256::digest(&foreign)),
            ..selected
        };
        let foreign_owner = mapped_bytes(&foreign)?;
        let foreign_view = complete_view(&foreign_owner).ok_or("foreign fixture backing missing")?;
        assert!(matches!(read_positioned_plan_members(&foreign_view, &foreign_selected),
                     Err(reason) if reason.contains("selected module name")));
        Ok(())
    }

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
        let empty = CompactAdmissionPin {
            count: 0,
            digest: [
                149, 238, 125, 89, 242, 99, 68, 174, 175, 86, 3, 135, 30, 209, 61, 32, 251, 208, 86, 56, 223, 193, 156,
                128, 75, 36, 208, 42, 186, 249, 13, 152,
            ],
        };
        assert_eq!(
            compact_admission_pin([])?,
            Some(empty),
            "selected empty compact publication has no independently settled fingerprint"
        );
        assert!(compact_admission_pin([first, first]).is_err());
        assert!(compact_admission_pin([second, first]).is_err());
        assert!(compact_admission_pin([("", first.1)]).is_err());
        Ok(())
    }
}
