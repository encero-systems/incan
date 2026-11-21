//! Retain the scalar plan's entry selection and complete native interface as record producer outputs.
//!
//! A records request publishes, beside its report's product events, the whole-program plan function of every keyed
//! declaration, an entry binding for the ordinary `main`, and the native interface Bakery admits the unit by.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use incan_mir_plan_native::{
    ADMITTED_DEFINITIONS, AdmittedDefinition, Function, INTERFACE_REPORT, LoweringEvent, NATIVE_DEFINITIONS,
    NativeInterface, NominalKind, NominalReference, Plan, PlanType, REPOSITION_MODULE_PAGE, carrier_position,
    decode_lowering_event, decode_lowering_refusal, decode_plan_function_record, decode_plan_module,
    encode_admitted_definitions, encode_entry_record, encode_lowering_event, encode_native_interface,
    encode_record_plan_products, is_carrier_index, placed_admission, product_frame_names, verify_entry_record,
};
use incan_semantics_core::semantic_digest::semantic_digest;

use crate::plan_modules::{admitted_frames, module_page, read_admitted};
use crate::producer_trace::{enter_producer, finish_producer};
use crate::record_positions::positioned_product;
use crate::scalar_store::{event_positions, event_references, finalized_plan_product, report_scalar_refusal};

/// Publish loss-checked plan output and a reusable ordinary-entry binding before native compilation.
pub fn publish_plan_records(plan: &Plan, products: &str, report: &str) -> Result<(), String> {
    let report_path = Path::new(report);
    let mut events = scalar_events(report_path)?;
    let (restored_frames, restored_digests) = append_admitted_events(&mut events, products, report_path)?;
    // Every keyed declaration retains its whole-plan function, including in a unit that publishes no interface. Only a
    // unit whose every function has an event can publish one; otherwise no placed product is kept for it.
    let publishable = events.len() == plan.functions.len();
    let admitted = if publishable {
        AdmittedProducts::Placed
    } else {
        AdmittedProducts::Framed(&restored_frames)
    };
    let mut references = Vec::new();
    let mut carried = Vec::new();
    let keyed = keyed_plan_products(
        plan,
        products,
        report_path,
        &events,
        admitted,
        &mut references,
        &mut carried,
    )?;
    publish_native_definitions(plan, report_path, &events, &restored_frames, &restored_digests)?;
    if !publishable {
        return refuse_interface(plan, report, &events, &carried);
    }
    if keyed.len() != plan.functions.len() {
        return Err("native-interface has an unwitnessed plan-function member".into());
    }
    let interface_attempt = enter_producer("compiler", "native-interface", report, "encode");
    let encoded = encode_record_plan_products(plan, keyed, references)?;
    finish_producer(interface_attempt, "produced");
    let main = ordinary_entry(plan)?;
    let matches: Vec<&LoweringEvent> = events.iter().filter(|event| event.function == main).collect();
    let [selected] = matches.as_slice() else {
        return Err("plan-function(entry) has no unique witnessed main function".into());
    };
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
    )
}

/// Name a native input for every plan function no event names, so native incremental reuse can track every definition
/// in a unit that publishes no whole-unit interface: a helper frame by the key of the product that carries it, any
/// other function by the digest of its own plan. Each also names the unit's external declarations, which a native body
/// reads beside its own record. A unit whose every function has an event names none.
fn publish_native_definitions(
    plan: &Plan,
    report: &Path,
    events: &[LoweringEvent],
    restored_frames: &HashMap<String, Vec<String>>,
    restored_digests: &HashMap<String, String>,
) -> Result<(), String> {
    let named: HashSet<&str> = events.iter().map(|event| event.function.as_str()).collect();
    // Each helper frame by the key and product digest of the function that carries it.
    let mut carriers: HashMap<String, (String, String)> = HashMap::new();
    for event in events {
        let (frames, digest) = match (
            restored_frames.get(&event.function),
            restored_digests.get(&event.function),
        ) {
            (Some(frames), Some(digest)) if event.admitted => (frames.clone(), digest.clone()),
            _ if event.role == "entry" => continue,
            _ => {
                let record = decode_plan_function_record(&event.product)?;
                (product_frame_names(&record.function)?, record.digest)
            }
        };
        for frame in frames {
            carriers
                .entry(frame)
                .or_insert_with(|| (event.key.clone(), digest.clone()));
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
fn refuse_interface(plan: &Plan, report: &str, events: &[LoweringEvent], carried: &[String]) -> Result<(), String> {
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
    let reported: HashSet<&str> = events.iter().map(|event| event.function.as_str()).collect();
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

/// How plan publication treats the products restored from admitted module pages.
#[derive(Clone, Copy)]
enum AdmittedProducts<'a> {
    /// The unit publishes its native interface: each admitted product is placed at its declaration's positions.
    Placed,
    /// The interface is refused: only the helper frames each admitted function carries are named, as lowering
    /// recorded them beside the module's note.
    Framed(&'a HashMap<String, Vec<String>>),
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
    events: &[LoweringEvent],
    admitted: AdmittedProducts<'_>,
    references: &mut Vec<Vec<NominalReference>>,
    carried: &mut Vec<String>,
) -> Result<Vec<Vec<u8>>, String> {
    let mut event_index: HashMap<&str, Option<usize>> = HashMap::new();
    for (index, event) in events.iter().enumerate() {
        event_index
            .entry(event.function.as_str())
            .and_modify(|slot| *slot = None)
            .or_insert(Some(index));
    }
    let mut function_indexes: HashMap<&str, usize> = HashMap::new();
    for (index, function) in plan.functions.iter().enumerate() {
        function_indexes.entry(function.name.as_str()).or_insert(index);
    }
    let base = carrier_base(plan);
    let mut keyed = Vec::new();
    for function in &plan.functions {
        let Some(slot) = event_index.get(function.name.as_str()) else {
            continue;
        };
        let Some(index) = slot else {
            return Err(format!(
                "native-interface has an ambiguous plan-function member {}",
                function.name
            ));
        };
        let reported = &events[*index];
        let resolved = plan_positions(plan, base, &event_references(reported)?)?;
        let nodes = event_positions(reported);
        let file = function.span.file.to_string();
        if reported.admitted {
            let mut names = Vec::new();
            let positioned = match admitted {
                AdmittedProducts::Placed => {
                    let product = decode_plan_function_record(&reported.product)?.function;
                    Some(positioned_product(
                        &product,
                        &nodes,
                        &file,
                        function.span.line,
                        &mut names,
                    )?)
                }
                AdmittedProducts::Framed(frames) => {
                    let Some(listed) = frames.get(&reported.function) else {
                        return Err(format!(
                            "admitted plan function {} has no helper frame list",
                            reported.function
                        ));
                    };
                    names.clone_from(listed);
                    None
                }
            };
            for name in names {
                if !function_indexes.contains_key(name.as_str()) {
                    return Err(format!(
                        "plan-function frame {name} is missing from the whole-program plan"
                    ));
                }
                carried.push(name);
            }
            if let Some(positioned) = positioned {
                keyed.push(positioned);
                references.push(resolved);
            }
            continue;
        }
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
        frames.push(plan.functions[*index].clone());
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
        if reference.kind != NominalKind::Enum || !is_carrier_index(reference.index) {
            placed.push(reference.clone());
            continue;
        }
        let index = base as i64 + carrier_position(reference.index);
        let declared = usize::try_from(index).ok().and_then(|at| plan.enums.get(at));
        let Some(declaration) = declared else {
            return Err(format!(
                "plan names no carrier {} at the place a product reported",
                reference.name
            ));
        };
        if !reference.name.is_empty() && declaration.name != reference.name {
            return Err(format!(
                "plan names no carrier {} at the place a product reported",
                reference.name
            ));
        }
        placed.push(NominalReference {
            kind: reference.kind.clone(),
            index,
            name: reference.name.clone(),
        });
    }
    Ok(placed)
}

/// Add the final event of every function this request restored from an admitted module's plan record.
///
/// Such a function reported no event. Its event comes from the module's page, marked `admitted`, with the positions its
/// declarations resolved to in this request in place of the ones the page recorded. A function that also took the
/// keyed route keeps the event it reported. The native adapter fingerprints each definition by its product key and
/// digest; it reads them from one list. Returns, by native function name, the helper frames each added function's
/// product carries, as lowering recorded them beside the module's note.
fn append_admitted_events(
    events: &mut Vec<LoweringEvent>,
    products: &str,
    report: &Path,
) -> Result<(HashMap<String, Vec<String>>, HashMap<String, String>), String> {
    let mut reported: HashSet<String> = events.iter().map(|event| event.function.clone()).collect();
    let mut definitions = Vec::new();
    let mut digests = HashMap::new();
    let mut carried = HashMap::new();
    let admissions = Path::new(products).join("admissions");
    for note in read_admitted(&admissions)? {
        let mut frames = admitted_frames(&admissions, &note)?;
        let path = module_page(products, &note.module);
        if path.to_string_lossy().ends_with(REPOSITION_MODULE_PAGE) && !note.inspected {
            return Err("inspected module lacks its placement report".into());
        }
        let encoded = std::fs::read(&path).map_err(|error| format!("cannot read admitted plan module {error}"))?;
        let page = decode_plan_module(&encoded).map_err(|error| format!("admitted plan module refused: {error:?}"))?;
        let stored: HashMap<&str, &[u8]> = page
            .functions
            .iter()
            .map(|function| (function.function.as_str(), function.event.as_slice()))
            .collect();
        for function in &note.functions {
            if reported.contains(&function.function) {
                continue;
            }
            let Some(event) = stored.get(function.function.as_str()) else {
                return Err(format!(
                    "admitted plan module {} holds no event for {}",
                    note.module, function.function
                ));
            };
            let mut event = placed_admission(&decode_lowering_event(event)?, &note.module, function)?;
            let record = decode_plan_function_record(&event.product)?;
            event.reused = true;
            event.admitted = true;
            definitions.push(AdmittedDefinition {
                function: function.function.clone(),
                identity: function.identity.clone(),
                key: event.key.clone(),
                digest: record.digest.clone(),
            });
            digests.insert(function.function.clone(), record.digest);
            events.push(event);
            reported.insert(function.function.clone());
            carried.insert(
                function.function.clone(),
                frames.remove(&function.function).unwrap_or_default(),
            );
        }
    }
    let listed = report.join(ADMITTED_DEFINITIONS);
    if !definitions.is_empty() {
        write(&listed, &encode_admitted_definitions(&definitions)?)?;
    } else if listed.is_file() {
        std::fs::remove_file(&listed).map_err(|error| error.to_string())?;
    }
    Ok((carried, digests))
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
