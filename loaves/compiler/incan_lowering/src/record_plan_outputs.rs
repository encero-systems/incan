//! Retain the scalar plan's entry selection and complete native interface as record producer outputs.
//!
//! A records request publishes, beside its report's product events, the whole-program plan function of every keyed
//! declaration, an entry binding for the ordinary `main`, and the native interface Bakery admits the unit by.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::path::Path;

use incan_mir_plan_native::{
    ADMITTED_DEFINITIONS, AdmittedDefinition, AdmittedFunction, CompactAdmissionPin, EventReference, Function,
    INTERFACE_REPORT, LoweringEvent, NATIVE_DEFINITIONS, NativeInterface, NominalKind, NominalReference, Plan,
    PlanType, REPOSITION_MODULE_PAGE, carrier_position, compact_admission_pin, decode_lowering_event,
    decode_lowering_refusal, decode_plan_function_record, decode_plan_module, encode_admitted_definitions,
    encode_entry_record, encode_lowering_event, encode_native_interface, encode_record_plan_products, is_carrier_index,
    place_admitted_event, product_frame_names, verify_entry_record,
};
use incan_semantics_core::semantic_digest::semantic_digest;

use crate::plan_modules::{
    AdmittedProduct, PlanAdmissions, SettledModule, admitted_plan_page, module_page, publish_settled_module_note,
};
use crate::producer_trace::{enter_producer, finish_producer};
use crate::record_positions::positioned_product;
use crate::scalar_store::{
    event_positions, event_reference_kind, event_references, finalized_plan_product, report_scalar_refusal,
};

/// The identity a refusal of the whole-unit native interface is reported under.
const INTERFACE_IDENTITY: &str = "native-interface";

#[cfg(test)]
thread_local! {
    static MATERIALIZED_SETTLED_EVENTS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Count admitted report members represented as ordinary events with no encoded product.
#[cfg(test)]
pub(crate) fn materialized_settled_events() -> usize {
    MATERIALIZED_SETTLED_EVENTS.with(std::cell::Cell::get)
}

/// Publish loss-checked plan output and a reusable ordinary-entry binding before native compilation.
pub fn publish_plan_records(
    plan: &Plan,
    products: &str,
    report: &str,
    admissions: &PlanAdmissions,
) -> Result<Option<CompactAdmissionPin>, String> {
    let report_path = Path::new(report);
    let scalar = scalar_events(report_path)?;
    let publication = append_admitted_events(scalar, products, report_path, plan.functions.len(), admissions)?;
    #[cfg(test)]
    MATERIALIZED_SETTLED_EVENTS.with(|count| {
        count.set(
            publication
                .members()
                .filter_map(PublicationMember::event)
                .filter(|event| event.admitted && event.product.is_empty())
                .count(),
        );
    });
    // Every keyed declaration retains its whole-plan function, including in a unit that publishes no interface. Only a
    // unit whose every function has an event can publish one; otherwise no placed product is kept for it.
    let publishable = publication.scalar.len() + publication.settled.len() == plan.functions.len();
    let mut references = Vec::new();
    let mut carried = Vec::new();
    let keyed = keyed_plan_products(
        plan,
        products,
        report_path,
        &publication,
        publishable,
        &mut references,
        &mut carried,
    )?;
    publish_native_definitions(plan, report_path, &publication)?;
    if !publishable {
        refuse_interface(plan, report, &publication, &carried)?;
        return Ok(publication.compact);
    }
    if keyed.len() != plan.functions.len() {
        return Err("native-interface has an unwitnessed plan-function member".into());
    }
    // The interface is an optional product beside the unit's ordinary compile, like each declaration's product: an
    // entry its record cannot bind, or a plan its codec refuses, publishes no interface and records why, and the unit
    // compiles exactly as it would have.
    let main = match ordinary_entry(plan) {
        Ok(main) => main,
        Err(refusal) => {
            report_scalar_refusal(Some(report), INTERFACE_IDENTITY, "main", &refusal)?;
            return Ok(publication.compact);
        }
    };
    let interface_attempt = enter_producer("compiler", "native-interface", report, "encode");
    let encoded = match encode_record_plan_products(plan, keyed, references) {
        Ok(encoded) => encoded,
        Err(refusal) => {
            finish_producer(interface_attempt, "refused");
            report_scalar_refusal(Some(report), INTERFACE_IDENTITY, INTERFACE_IDENTITY, &refusal)?;
            return Ok(publication.compact);
        }
    };
    finish_producer(interface_attempt, "produced");
    let matches: Vec<_> = publication
        .members()
        .filter(|member| member.function() == main)
        .collect();
    let [selected] = matches.as_slice() else {
        return Err("plan-function(entry) has no unique witnessed main function".into());
    };
    let selected = selected
        .event()
        .ok_or("plan-function(entry) has no unique witnessed main function")?;
    let generation =
        std::fs::read_to_string(Path::new(products).join("producer-profile")).map_err(|error| error.to_string())?;
    let selection = if plan.programs.is_empty() {
        "implicit-main"
    } else {
        "selected-main"
    };
    let inputs: Vec<String> = ["incan.plan-entry/2", &generation, &main, selection, &selected.key]
        .iter()
        .map(|input| (*input).to_owned())
        .collect();
    let key = semantic_digest(&inputs)
        .map_err(|error| error.to_string())?
        .replace("sha256:", "");
    let path = Path::new(products).join(format!("{key}.incnentry"));
    let reused = path.is_file();
    let value = if reused {
        let value = std::fs::read(&path).map_err(|error| error.to_string())?;
        verify_entry_record(&value, &main, &inputs)?;
        value
    } else {
        let entry_attempt = enter_producer("compiler", "plan-function", &selected.identity, "entry");
        let value = encode_entry_record(&main, &inputs)?;
        finish_producer(entry_attempt, "produced");
        std::fs::write(&path, &value).map_err(|error| error.to_string())?;
        value
    };
    let entry = LoweringEvent {
        identity: selected.identity.clone(),
        key: key.clone(),
        function: "entry".into(),
        role: "entry".into(),
        module: selected.module.clone(),
        demand: selected.key.clone(),
        product: value,
        reused,
        ..LoweringEvent::default()
    };
    write(
        &report_path.join(format!("{key}.event")),
        &encode_lowering_event(&entry)?,
    )?;
    let interface = NativeInterface {
        entry: key,
        plan: encoded,
    };
    write(
        &report_path.join(INTERFACE_REPORT),
        &encode_native_interface(&interface)?,
    )?;
    Ok(publication.compact)
}

/// Name a native input for every plan function no event names, so native incremental reuse can track every definition
/// in a unit that publishes no whole-unit interface: a helper frame by the key of the product that carries it, any
/// other function by the digest of its own plan. Each also names the unit's external declarations, which a native body
/// reads beside its own record. A unit whose every function has an event names none.
fn publish_native_definitions(plan: &Plan, report: &Path, publication: &AdmittedPublication<'_>) -> Result<(), String> {
    let named: HashSet<&str> = publication.members().map(PublicationMember::function).collect();
    // Each helper frame by the key and product digest of the function that carries it.
    let mut carriers: HashMap<String, (String, String)> = HashMap::new();
    for member in publication.members() {
        let (key, frames, digest): (&str, Cow<'_, [String]>, Cow<'_, str>) = match member {
            PublicationMember::Admitted { product, .. } => (
                &product.key,
                Cow::Borrowed(&product.helpers),
                Cow::Borrowed(&product.digest),
            ),
            PublicationMember::Scalar(event) if event.role == "entry" => continue,
            PublicationMember::Scalar(event) => {
                let record = decode_plan_function_record(&event.product)?;
                (
                    &event.key,
                    Cow::Owned(product_frame_names(&record.function)?),
                    Cow::Owned(record.digest),
                )
            }
        };
        for frame in frames.iter() {
            carriers
                .entry(frame.clone())
                .or_insert_with(|| (key.to_owned(), digest.to_string()));
        }
    }
    let externals = semantic_digest(&format!("{:?}", plan.externals)).map_err(|error| error.to_string())?;
    let mut definitions = Vec::new();
    for function in &plan.functions {
        if named.contains(function.name.as_str()) {
            continue;
        }
        let (key, content) = match carriers.get(&function.name) {
            Some((carrier, digest)) => (
                carrier.clone(),
                vec![
                    "carried-frame".to_owned(),
                    carrier.clone(),
                    digest.clone(),
                    function.name.clone(),
                ],
            ),
            None => (
                "content".to_owned(),
                vec!["plan-function".to_owned(), format!("{function:?}")],
            ),
        };
        let digest =
            semantic_digest(&[content, vec![externals.clone()]].concat()).map_err(|error| error.to_string())?;
        definitions.push(AdmittedDefinition {
            function: function.name.clone(),
            identity: function.name.clone(),
            key,
            digest,
        });
    }
    let listed = report.join(NATIVE_DEFINITIONS);
    if !definitions.is_empty() {
        write(&listed, &encode_admitted_definitions(&definitions)?)?;
    } else if listed.is_file() {
        std::fs::remove_file(&listed).map_err(|error| error.to_string())?;
    }
    Ok(())
}

/// Publish no whole-unit interface for a plan with functions no product witnesses, and say why.
///
/// Helper frames travel inside their caller's product, so a unit with any publishes no whole-unit interface: Bakery
/// accepts an omitted interface only beside an explicit lowering refusal. Unit-level runtime resolution adds forwarders
/// after every product was captured, so none witnesses them.
fn refuse_interface(
    plan: &Plan,
    report: &str,
    publication: &AdmittedPublication<'_>,
    carried: &[String],
) -> Result<(), String> {
    for frame in carried {
        report_scalar_refusal(
            Some(report),
            frame,
            frame,
            "native-interface refuses a helper frame carried inside its caller's product",
        )?;
    }
    if !carried.is_empty() || has_scalar_refusal(Path::new(report))? {
        return Ok(());
    }
    let reported: HashSet<&str> = publication.members().map(PublicationMember::function).collect();
    let mut unwitnessed = 0;
    for function in &plan.functions {
        if !reported.contains(function.name.as_str()) {
            report_scalar_refusal(
                Some(report),
                &function.name,
                &function.name,
                "native-interface refuses a generated function without a graph body witness",
            )?;
            unwitnessed += 1;
        }
    }
    if unwitnessed != 0 {
        return Ok(());
    }
    Err("native-interface refuses generated functions without graph body witnesses".into())
}

/// Make each keyed product byte-identical to its whole-plan function, relocated to the declaration's position.
///
/// Lowering captures a product before unit-level runtime resolution rewrites its calls. A fresh product therefore
/// retains the final function under the same inputs, and its event carries the replaced bytes to Bakery admission. A
/// reused product must already be final. A declaration without a product stays ordinary. `references` receives, per
/// keyed product, the positions its declaration ordinals take in this unit, and `carried` the names of the helper
/// frames those products carry.
///
/// An admitted product is final already; it is placed at its declaration's positions only for an interface the unit
/// can publish, the one consumer of those bytes. Otherwise only the helper frames it carries are named.
fn keyed_plan_products(
    plan: &Plan,
    products: &str,
    report: &Path,
    publication: &AdmittedPublication<'_>,
    publishable: bool,
    references: &mut Vec<Vec<NominalReference>>,
    carried: &mut Vec<String>,
) -> Result<Vec<Vec<u8>>, String> {
    let mut event_index = HashMap::new();
    for member in publication.members() {
        event_index
            .entry(member.function())
            .and_modify(|slot| *slot = None)
            .or_insert(Some(member));
    }
    let mut function_indexes: HashMap<&str, usize> = HashMap::new();
    for (index, function) in plan.functions.iter().enumerate() {
        function_indexes.entry(function.name.as_str()).or_insert(index);
    }
    let base = carrier_base(plan);
    let mut keyed = Vec::new();
    for function in &plan.functions {
        let Some(slot) = event_index.get(function.name.as_str()).copied() else {
            continue;
        };
        let Some(member) = slot else {
            return Err(format!(
                "native-interface has an ambiguous plan-function member {}",
                function.name
            ));
        };
        let reported = match member {
            PublicationMember::Admitted {
                function: row,
                product,
                event,
            } => {
                let mut decoded_names = Vec::new();
                let names = if publishable {
                    let event = event.ok_or("complete native interface has no selected product event")?;
                    let resolved = plan_positions(plan, base, &event_references(event)?)?;
                    let record = decode_plan_function_record(&event.product)?;
                    if record.digest != product.digest {
                        return Err("admitted plan function digest differs from its settled product".into());
                    }
                    let positioned = positioned_product(
                        &record.function,
                        &event_positions(event),
                        function.span.file.as_str(),
                        function.span.line,
                        &mut decoded_names,
                    )?;
                    keyed.push(positioned);
                    references.push(resolved);
                    &decoded_names
                } else {
                    check_borrowed_positions(plan, base, &row.references)?;
                    &product.helpers
                };
                for name in names {
                    if !function_indexes.contains_key(name.as_str()) {
                        return Err(format!(
                            "plan-function frame {name} is missing from the whole-program plan"
                        ));
                    }
                    carried.push(name.clone());
                }
                continue;
            }
            PublicationMember::Scalar(event) => event,
        };
        if reported.admitted {
            return Err(format!(
                "admitted plan function {} has no product evidence",
                reported.function
            ));
        }
        let resolved = plan_positions(plan, base, &event_references(reported)?)?;
        let nodes = event_positions(reported);
        let mut event = reported.clone();
        let frames = carried_frames(plan, &function_indexes, &event.product, carried)?;
        let attempt = enter_producer("compiler", "plan-function-finalize", &event.identity, &event.key);
        let (last, positions, positioned) = finalized_plan_product(
            products,
            &event.key,
            function,
            event.reused,
            resolved,
            &nodes,
            &frames,
            &event.product,
        )?;
        finish_producer(attempt, "produced");
        if last != event.product {
            event.product = last;
            write(
                &report.join(format!("{}.event", event.key)),
                &encode_lowering_event(&event)?,
            )?;
        }
        keyed.push(positioned);
        references.push(positions);
    }
    Ok(keyed)
}

/// Require incomplete native-interface publication to carry an explicit scalar-record refusal.
fn has_scalar_refusal(report: &Path) -> Result<bool, String> {
    for entry in std::fs::read_dir(report).map_err(|error| error.to_string())? {
        let path = entry.map_err(|error| error.to_string())?.path();
        if path.is_file() && path.to_string_lossy().ends_with(".refusal") {
            decode_lowering_refusal(&std::fs::read(&path).map_err(|error| error.to_string())?)?;
            return Ok(true);
        }
    }
    Ok(false)
}

/// Select the same standalone synchronous unit main admitted by existing lowering; other entry roles refuse.
fn ordinary_entry(plan: &Plan) -> Result<String, String> {
    if !plan.programs.is_empty() && plan.programs != ["main"] {
        return Err("plan-function(entry) refuses a non-standalone program selection".into());
    }
    let Some(function) = plan.functions.iter().find(|function| function.name == "main") else {
        return Err("plan-function(entry) has no standalone main".into());
    };
    if !function.parameters.is_empty() || function.return_type != PlanType::Unit || function.is_async {
        return Err("plan-function(entry) currently requires a synchronous unit main".into());
    }
    Ok(function.name.clone())
}

/// Read each actual scalar producer output in deterministic key order.
fn scalar_events(report: &Path) -> Result<Vec<LoweringEvent>, String> {
    let mut paths = Vec::new();
    for entry in std::fs::read_dir(report).map_err(|error| error.to_string())? {
        paths.push(entry.map_err(|error| error.to_string())?.path());
    }
    paths.sort_by_key(|path| path.to_string_lossy().into_owned());
    let mut events = Vec::new();
    for path in paths {
        if path.to_string_lossy().ends_with(".event") {
            events.push(decode_lowering_event(
                &std::fs::read(&path).map_err(|error| error.to_string())?,
            )?);
        }
    }
    Ok(events)
}

/// Persist the producer's complete typed record without a character encoding step.
fn write(path: &Path, value: &[u8]) -> Result<(), String> {
    std::fs::write(path, value).map_err(|error| error.to_string())
}

/// Select, in a product's order, the whole-plan helper frames it carries, and record their names.
fn carried_frames(
    plan: &Plan,
    function_indexes: &HashMap<&str, usize>,
    record: &[u8],
    carried: &mut Vec<String>,
) -> Result<Vec<Function>, String> {
    let payload = decode_plan_function_record(record)?.function;
    let mut frames = Vec::new();
    for name in product_frame_names(&payload)? {
        let Some(index) = function_indexes.get(name.as_str()) else {
            return Err(format!(
                "plan-function frame {name} is missing from the whole-program plan"
            ));
        };
        frames.push((*plan.functions[*index]).clone());
        carried.push(name);
    }
    Ok(frames)
}

/// Name each carrier a product reported at its lowering position by its place in the published plan.
///
/// Publication appended the carriers, in their lowering order, after the source enums; only a carrier records the
/// checked type it lays out, so the first enum that does is the first carrier.
fn carrier_base(plan: &Plan) -> usize {
    plan.enums
        .iter()
        .position(|declaration| declaration.semantic_key.is_some())
        .unwrap_or(plan.enums.len())
}

/// Resolve product-local carrier ordinals against the whole unit's declared table.
fn plan_positions(plan: &Plan, base: usize, references: &[NominalReference]) -> Result<Vec<NominalReference>, String> {
    let mut placed = Vec::with_capacity(references.len());
    for reference in references {
        let index = plan_position(plan, base, reference.kind, reference.index, &reference.name)?;
        placed.push(NominalReference {
            kind: reference.kind,
            index,
            name: reference.name.clone(),
        });
    }
    Ok(placed)
}

/// Validate settled references in the original order. Every kind is checked before any position can refuse.
fn check_borrowed_positions(plan: &Plan, base: usize, references: &[EventReference]) -> Result<(), String> {
    for reference in references {
        event_reference_kind(&reference.kind)?;
    }
    for reference in references {
        if reference.kind == NominalKind::Enum.value() {
            plan_position(plan, base, NominalKind::Enum, reference.index, &reference.name)?;
        }
    }
    Ok(())
}

/// Resolve one carrier reference against the current whole-plan enum and verify its name.
fn plan_position(plan: &Plan, base: usize, kind: NominalKind, index: i64, name: &str) -> Result<i64, String> {
    if kind != NominalKind::Enum || !is_carrier_index(index) {
        return Ok(index);
    }
    let position = base as i64 + carrier_position(index);
    let declared = usize::try_from(position).ok().and_then(|at| plan.enums.get(at));
    if declared.is_some_and(|declaration| name.is_empty() || declaration.name == name) {
        return Ok(position);
    }
    Err(format!("plan names no carrier {name} at the place a product reported"))
}

/// One settled function's existing evidence and the optional real product event read for a complete interface.
struct SettledEvent<'a> {
    function: &'a AdmittedFunction,
    product: &'a AdmittedProduct,
    page_event: Option<usize>,
}

/// A changed scalar report row or a settled definition with the same ordered publication membership.
#[derive(Clone, Copy)]
enum PublicationMember<'a> {
    Scalar(&'a LoweringEvent),
    Admitted {
        function: &'a AdmittedFunction,
        product: &'a AdmittedProduct,
        event: Option<&'a LoweringEvent>,
    },
}

impl<'a> PublicationMember<'a> {
    /// The whole-program function this member witnesses.
    fn function(self) -> &'a str {
        match self {
            Self::Scalar(event) => &event.function,
            Self::Admitted { function, .. } => &function.function,
        }
    }

    /// The encoded product, when publication read a real scalar or selected module-page event.
    fn event(self) -> Option<&'a LoweringEvent> {
        match self {
            Self::Scalar(event) => Some(event),
            Self::Admitted { event, .. } => event,
        }
    }
}

/// Ordered scalar and admitted members and the independently hashed compact set from the same publication pass.
struct AdmittedPublication<'a> {
    scalar: Vec<LoweringEvent>,
    settled: Vec<SettledEvent<'a>>,
    page_events: Vec<LoweringEvent>,
    compact: Option<CompactAdmissionPin>,
}

impl AdmittedPublication<'_> {
    /// The sorted scalar files precede members in their existing module and note order.
    fn members(&self) -> impl Iterator<Item = PublicationMember<'_>> {
        self.scalar
            .iter()
            .map(PublicationMember::Scalar)
            .chain(self.settled.iter().map(|settled| PublicationMember::Admitted {
                function: settled.function,
                product: settled.product,
                event: settled.page_event.map(|index| &self.page_events[index]),
            }))
    }
}

/// Add settled definitions for functions restored through a module page. When the plan contains functions with no
/// witnessed product, the native publisher needs only the already checked keys, digests and helper names. A possible
/// complete interface still reads the authoritative products and applies their current positions.
fn append_admitted_events<'a>(
    scalar: Vec<LoweringEvent>,
    products: &str,
    report: &Path,
    functions_in_plan: usize,
    admissions: &'a PlanAdmissions,
) -> Result<AdmittedPublication<'a>, String> {
    let mut reported: HashSet<String> = scalar.iter().map(|event| event.function.clone()).collect();
    let mut definitions = Vec::new();
    let mut restored = Vec::new();
    let mut page_events = Vec::new();
    let mut compact_notes = Vec::new();
    let modules = admissions.modules();
    let candidates = modules
        .values()
        .map(|module| module.note.functions.len())
        .sum::<usize>();
    let needs_products = functions_in_plan <= scalar.len() + candidates;
    for module in modules.values() {
        let SettledModule {
            note,
            products: settled,
            ..
        } = module;
        let path = module_page(products, &note.module);
        if path.to_string_lossy().ends_with(REPOSITION_MODULE_PAGE) && !note.inspected {
            return Err("inspected module lacks its placement report".into());
        }
        let page = needs_products
            .then(|| admitted_plan_page(&path))
            .transpose()
            .map_err(|error| format!("cannot read admitted plan module: {error}"))?;
        let stored = page.as_ref().map(|page| {
            page.functions
                .iter()
                .map(|function| (function.function.as_str(), function.event.as_slice()))
                .collect::<HashMap<_, _>>()
        });
        let compact = publish_settled_module_note(products, report, module, &reported)?;
        if let Some(digest) = compact {
            compact_notes.push((note.module.as_str(), digest));
        }
        for (function, product) in note.functions.iter().zip(settled) {
            if reported.contains(&function.function) {
                continue;
            }
            let page_event = if let Some(stored) = &stored {
                let encoded = stored.get(function.function.as_str()).ok_or_else(|| {
                    format!(
                        "admitted plan module {} holds no event for {}",
                        note.module, function.function
                    )
                })?;
                let mut event = decode_lowering_event(encoded)?;
                if event.key != product.key {
                    return Err("admitted plan function key differs from its settled product".into());
                }
                place_admitted_event(&mut event, &note.module, function)?;
                event.reused = true;
                event.admitted = true;
                let index = page_events.len();
                page_events.push(event);
                Some(index)
            } else {
                None
            };
            if compact.is_none() {
                definitions.push(AdmittedDefinition {
                    function: function.function.clone(),
                    identity: function.identity.clone(),
                    key: product.key.clone(),
                    digest: product.digest.clone(),
                });
            }
            reported.insert(function.function.clone());
            restored.push(SettledEvent {
                function,
                product,
                page_event,
            });
        }
    }
    let listed = report.join(ADMITTED_DEFINITIONS);
    if !definitions.is_empty() {
        write(&listed, &encode_admitted_definitions(&definitions)?)?;
    } else if listed.is_file() {
        std::fs::remove_file(&listed).map_err(|error| error.to_string())?;
    }
    if crate::trace_channel::tracing_products() {
        let counts = serde_json::json!({
            "admitted_functions": restored.len(),
            "compact_modules": compact_notes.len(),
            "full_page_selections": if needs_products { modules.len() } else { 0 },
        });
        crate::trace_channel::emit("admitted_plan_publication", "", &counts.to_string());
    }
    Ok(AdmittedPublication {
        scalar,
        settled: restored,
        page_events,
        compact: admissions
            .selection()
            .map(|_| compact_admission_pin(compact_notes))
            .transpose()?,
    })
}

#[cfg(test)]
mod tests {
    use incan_mir_plan_native::{
        EnumDeclaration, Function, NominalKind, NominalReference, Parameter, Plan, PlanType, carrier_index, source_span,
    };

    use super::{ordinary_entry, plan_positions};

    fn function(name: &str, parameters: Vec<Parameter>, return_type: PlanType) -> Function {
        Function::new(
            name.into(),
            parameters,
            return_type,
            Vec::new(),
            Vec::new(),
            source_span("main.incn", 1, 1),
        )
    }

    fn plan(functions: Vec<Function>) -> Plan {
        let span = source_span("main.incn", 1, 1);
        let enums = ["Color", "Option_i64"]
            .iter()
            .map(|name| EnumDeclaration::new((*name).into(), false, Vec::new(), Vec::new(), span.clone()))
            .collect();
        Plan::new(enums, Vec::new(), functions, Vec::new(), span)
    }

    #[test]
    fn only_a_synchronous_unit_main_is_the_ordinary_entry() {
        let main = || function("main", Vec::new(), PlanType::Unit);
        assert_eq!(
            ordinary_entry(&plan(vec![function("helper", Vec::new(), PlanType::Int), main()])),
            Ok("main".into())
        );
        let mut selected = plan(vec![main()]);
        selected.programs = vec!["main".into()];
        assert!(ordinary_entry(&selected).is_ok());
        selected.programs = vec!["serve".into()];
        assert!(ordinary_entry(&selected).is_err());
        assert!(ordinary_entry(&plan(vec![function("main", Vec::new(), PlanType::Int)])).is_err());
        let mut asynchronous = main();
        asynchronous.is_async = true;
        assert!(ordinary_entry(&plan(vec![asynchronous])).is_err());
        assert!(ordinary_entry(&plan(vec![function("helper", Vec::new(), PlanType::Unit)])).is_err());
    }

    #[test]
    fn a_product_carrier_takes_its_place_after_the_source_enums() {
        let whole = plan(Vec::new());
        let reference = |kind: NominalKind, index: i64, name: &str| NominalReference {
            kind,
            index,
            name: name.into(),
        };
        let placed = plan_positions(
            &whole,
            1,
            &[
                reference(NominalKind::Enum, carrier_index(0), "Option_i64"),
                reference(NominalKind::Enum, 0, "Color"),
                reference(NominalKind::Model, carrier_index(0), "Point"),
            ],
        );
        assert_eq!(
            placed,
            Ok(vec![
                reference(NominalKind::Enum, 1, "Option_i64"),
                reference(NominalKind::Enum, 0, "Color"),
                reference(NominalKind::Model, carrier_index(0), "Point"),
            ])
        );
        assert!(
            plan_positions(
                &whole,
                1,
                &[reference(NominalKind::Enum, carrier_index(0), "Option_str")]
            )
            .is_err()
        );
        assert!(plan_positions(&whole, 1, &[reference(NominalKind::Enum, carrier_index(1), "")]).is_err());
        assert_eq!(
            plan_positions(&whole, 1, &[reference(NominalKind::Enum, carrier_index(0), "")]),
            Ok(vec![reference(NominalKind::Enum, 1, "")])
        );
    }
}
