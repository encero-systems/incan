//! Lower a program's ordinary bodies, entrypoints, async entries, tests, and Loaf exports.

use std::collections::BTreeMap;

use incan_mir_plan_native::{
    AdmittedFunction, AdmittedPlacement, Carrier, EnumDeclaration, Function, GenericEnumDeclaration,
    GenericModelDeclaration, ModelDeclaration, NominalReference, PlanExport, PlanType, UnitNominals, resolve_nominals,
};
use incan_semantics_core::body_ir::{Body, BodyIrModule};
use incan_semantics_core::{
    CanonicalSymbolId, CompilerNodeId, IncanType, SymbolOrigin, canonical_module_identity, encode_incan_symbol_identity,
};

use crate::async_::{async_entry, async_test_name, boxed_future_constructor, evaluator_name};
use crate::body_positions::indexed_body_positions;
use crate::checked_values::{indexed_location, line_starts, location};
use crate::classes::{body_module_identity, function_name, is_program_entry};
use crate::decorator_caches::DecoratorFrame;
use crate::frame_instances::{
    append_synthesized, external_index, function_names, merge_externals, merge_indexed, named_functions,
};
use crate::function_frames::{LoweredFrame, lower_function};
use crate::generators::append_generator_constructor;
use crate::instance_products::{encode_requests, restore_requests};
use crate::lowering::Inventories;
use crate::nominal_records::{product_nominal_table, retain_named_scalar, sync_carriers};
use crate::plan_modules::{AdmittedPage, admitted_page, report_admitted};
use crate::plan_product_inputs::{ordinary_product_inputs, producer_generation};
use crate::program_context::ProgramContext;
use crate::scalar_store::{
    ProductEvent, admitted_function, admitted_payload, event_nodes, event_reference, report_scalar,
    report_scalar_refusal, retained_scalar,
};
use crate::trace_channel::{emit, frame_product};
use crate::traits::is_lazy_method_body;
use crate::type_parameters::{Instance, merge_instance_requests, request_instance};

/// Emitted frames in the immutable checked body inventory, indexed without retaining executable snapshots.
#[derive(Clone, Debug, Default)]
pub struct EmittedFrames {
    pub body_positions: Vec<usize>,
    pub positions: BTreeMap<CanonicalSymbolId, Vec<usize>>,
}

impl EmittedFrames {
    /// Reuse an identical canonical frame revisited through module context.
    pub fn contains(&self, context: &BodyIrModule, candidate: &Body) -> bool {
        let Some(identity) = &candidate.canonical else {
            return false;
        };
        match self.positions.get(identity) {
            Some(indices) => indices.iter().any(|index| context.bodies[*index] == *candidate),
            None => false,
        }
    }

    /// Record a checked body position under its canonical identity, preserving emission order for exports.
    pub fn record(&mut self, body: &Body, position: usize) {
        if let Some(identity) = &body.canonical {
            self.positions.entry(identity.clone()).or_default().push(position);
        }
        self.body_positions.push(position);
    }
}

pub fn no_emitted_frames() -> EmittedFrames {
    EmittedFrames::default()
}

/// Lower selected source frames in module order, retaining synthesized callees and checked instance requests.
///
/// The reference lowers contiguous module ranges in parallel and merges them in module order, falling back to this
/// order whenever a worker admitted its own carrier; both produce this serial result.
#[allow(clippy::too_many_arguments)]
pub fn lower_ordinary_bodies(
    context: &BodyIrModule,
    source_context: &BodyIrModule,
    source_files: &BTreeMap<String, String>,
    package_versions: &BTreeMap<String, String>,
    program: &mut ProgramContext,
    owners: &[String],
    sources: &[String],
    files: &[String],
    entry: usize,
    kind: &str,
    tests: &[String],
    intrinsics: &mut Vec<EnumDeclaration>,
    inventories: &mut Inventories,
    emitted_frames: &mut EmittedFrames,
    unions: &[IncanType],
    trace: bool,
    scalar_products: Option<String>,
    scalar_bodies: &BTreeMap<CompilerNodeId, String>,
    overflow_checks: bool,
    record_report: Option<String>,
) -> Result<(), String> {
    let _ = (source_context, source_files, package_versions, overflow_checks);
    let directory = scalar_products.as_deref();
    let mut keyed = KeyedProducts {
        directory,
        admitted: scalar_bodies,
        report: record_report.as_deref(),
        generation: producer_generation(directory),
        nominals: product_nominal_table(program, intrinsics, directory),
    };
    let mut module_bodies: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (position, body) in context.bodies.iter().enumerate() {
        module_bodies
            .entry(body_module_identity(body)?)
            .or_default()
            .push(position);
    }
    let mut names = function_names();
    let mut signatures = external_index();
    for (index, owner) in owners.iter().enumerate() {
        let positions = module_bodies.get(owner).cloned().unwrap_or_default();
        // The module's admitted plan record and the functions restored from it; without a page every body takes the
        // keyed route.
        let page = admitted_page(keyed.directory, owner, keyed.generation.as_deref());
        let placement_lines = if !page.events.is_empty() && page.inspect_placement {
            line_starts(&sources[index])
        } else {
            Vec::new()
        };
        let mut admission = ModuleAdmission {
            page: &page,
            placement_lines: &placement_lines,
            restored: Vec::new(),
        };
        let mut ordinary = 0;
        for position in positions {
            let body = &context.bodies[position];
            if !ordinary_body(context, body, emitted_frames, kind) {
                continue;
            }
            ordinary += 1;
            let diagnostic_owner = format!(" in {} at {}", body.name, files[index]);
            let frame = FrameSource {
                source: &sources[index],
                file: &files[index],
            };
            let restored_before = admission.restored.len();
            let (function, runtime, pending, synthesized) = lower_ordinary_frame(
                context,
                program,
                body,
                unions,
                &frame,
                intrinsics,
                &mut keyed,
                &mut admission,
            )
            .map_err(|reason| reason + &diagnostic_owner)?;
            // Only a body that took the keyed route was lowered or looked up; a restored body is not traced.
            if trace && admission.restored.len() == restored_before {
                emit(
                    "lowering",
                    "INCAN_TRACE_LOWERING",
                    &format!("lowering: body {owner} {} {}", body.name, body.span.start),
                );
            }
            merge_instance_requests(&mut inventories.instances, &pending);
            if !named_functions(&mut names, &inventories.functions, &function.name).is_empty() {
                return Err(format!("unsupported Body IR overloaded function {}", body.name));
            }
            let parameters: Vec<PlanType> = function
                .parameters
                .iter()
                .map(|parameter| parameter.ty.clone())
                .collect();
            let (planned, return_type, span) = (
                function.name.clone(),
                function.return_type.clone(),
                function.span.clone(),
            );
            inventories.functions.push(function.clone());
            if kind == "library" && body.is_async {
                inventories.functions.push(boxed_future_constructor(
                    function_name(context, body),
                    incan_mir_plan_native::CalleeKind::Planned(planned),
                    parameters,
                    return_type,
                    span,
                )?);
            }
            emitted_frames.record(body, position);
            append_synthesized(&mut inventories.functions, &mut names, synthesized)?;
            append_async_entry(context, body, &function, index == entry, tests, intrinsics, inventories)?;
            append_generator_constructor(
                &program.constructors,
                &mut inventories.functions,
                context,
                body,
                &sources[index],
                &files[index],
                intrinsics,
            )?;
            merge_indexed(&mut inventories.externals, &mut signatures, runtime)?;
        }
        report_admitted(
            keyed.report,
            owner,
            &admission.restored,
            ordinary,
            page.inspect_placement,
            keyed.directory,
        )?;
    }
    Ok(())
}

/// The keyed-product state one unit's ordinary frames share: the product directory, the checker's witnesses by body,
/// the records report, the producer profile and the unit's described declaration tables.
pub struct KeyedProducts<'a> {
    pub directory: Option<&'a str>,
    pub admitted: &'a BTreeMap<CompilerNodeId, String>,
    pub report: Option<&'a str>,
    pub generation: Option<String>,
    pub nominals: UnitNominals,
}

/// The source text and file an ordinary frame lowers in.
pub struct FrameSource<'a> {
    pub source: &'a str,
    pub file: &'a str,
}

/// One module's admitted plan record as its ordinary frames read it: the page, the line index an inspected placement
/// borrows, and the functions restored from the page so far.
pub struct ModuleAdmission<'a> {
    pub page: &'a AdmittedPage,
    pub placement_lines: &'a [i64],
    pub restored: Vec<AdmittedFunction>,
}

/// Read only producer-admitted products, or lower normally and retain an exact witnessed result.
///
/// A body of an admitted module whose final event restores is neither serialized, keyed nor lowered, and reports no
/// event: `admission.restored` receives it for the module's note. Every other body takes the keyed route. A product
/// names the unit's models and enums by declaration identity; `keyed.nominals` describes the current tables. A reused
/// product returns the helper frames and generic instance requests its lowering once made, as lowering would; a request
/// whose generic is gone makes the product a miss. A fresh product carries the synthesized helper frames and the
/// generic instance requests this lowering made; a request only a source position names keeps the frame without a
/// product. A records request reports each keyed frame's event, or why it has none.
#[allow(clippy::too_many_arguments)]
fn lower_ordinary_frame(
    context: &BodyIrModule,
    program: &ProgramContext,
    body: &Body,
    unions: &[IncanType],
    frame: &FrameSource<'_>,
    intrinsics: &mut Vec<EnumDeclaration>,
    keyed: &mut KeyedProducts<'_>,
    admission: &mut ModuleAdmission<'_>,
) -> Result<LoweredFrame, String> {
    if let Some(restored) = restore_admitted(context, program, body, frame, intrinsics, keyed, admission) {
        return Ok(restored);
    }
    let records = keyed.report.is_some();
    let inputs = ordinary_product_inputs(
        context,
        body,
        frame.source,
        frame.file,
        program.overflow_checks,
        keyed.directory,
        keyed.admitted,
        records,
        keyed.generation.as_deref(),
        None,
    )?;
    let owner = RecordOwner::of(context, body, keyed.report, inputs.is_some())?;
    if let (Some(directory), Some((witness, positions))) = (keyed.directory, &inputs) {
        sync_carriers(&mut keyed.nominals, intrinsics);
        let current = location(&body.span, frame.source, frame.file);
        if let Some((function, runtime, references, frames, encoded)) =
            retained_scalar(directory, witness, &keyed.nominals, Some(&current), positions)
            && let Some(requests) = restore_requests(&encoded, context)
        {
            report_scalar(
                keyed.report,
                &owner.event(
                    directory,
                    witness,
                    &function.name,
                    true,
                    runtime.len(),
                    &references,
                    positions,
                ),
            )?;
            frame_product("reused", "ordinary", &frame_label(body));
            return Ok((function, runtime, requests, frames));
        }
    }
    let lowered = lower_function(
        context,
        program,
        body,
        unions,
        frame.source,
        frame.file,
        intrinsics,
        &[],
        DecoratorFrame::Normal,
    )?;
    let (function, runtime, pending, synthesized) = &lowered;
    let mut kept = false;
    match encode_requests(pending) {
        Ok(requests) => {
            if let (Some(directory), Some((witness, positions))) = (keyed.directory, &inputs) {
                sync_carriers(&mut keyed.nominals, intrinsics);
                // The ordinary compiler already has its complete lowered function. Record the optional product's
                // refusal without changing that ordinary verdict.
                match retain_named_scalar(
                    directory,
                    witness,
                    function,
                    runtime,
                    &keyed.nominals,
                    positions,
                    synthesized,
                    &requests,
                ) {
                    Ok(references) => {
                        kept = true;
                        let event = owner.event(
                            directory,
                            witness,
                            &function.name,
                            false,
                            runtime.len(),
                            &references,
                            positions,
                        );
                        report_scalar(keyed.report, &event)?;
                    }
                    Err(reason) => report_scalar_refusal(keyed.report, &owner.identity, &function.name, &reason)?,
                }
            }
        }
        Err(reason) => {
            if inputs.is_some() {
                report_scalar_refusal(keyed.report, &owner.identity, &function.name, &reason)?;
            }
        }
    }
    if inputs.is_some() {
        frame_product(if kept { "lowered" } else { "unkept" }, "ordinary", &frame_label(body));
    }
    Ok(lowered)
}

/// The owner a records request reports an ordinary frame's event under: its stable declaration identity and declaring
/// module, so Bakery can fold a module's events into one record. Without a records request both stay empty.
struct RecordOwner {
    identity: String,
    module: String,
}

impl RecordOwner {
    /// Name the frame's owner, reporting a body without a stable identity or without a complete checker witness.
    fn of(context: &BodyIrModule, body: &Body, report: Option<&str>, witnessed: bool) -> Result<Self, String> {
        let mut owner = RecordOwner {
            identity: String::new(),
            module: String::new(),
        };
        if report.is_none() {
            return Ok(owner);
        }
        match &body.canonical {
            None => report_scalar_refusal(
                report,
                &owner.identity,
                &evaluator_name(context, body),
                "body has no stable declaration identity",
            )?,
            Some(identity) => {
                owner.module = body_module_identity(body)?;
                owner.identity = encode_incan_symbol_identity(identity);
            }
        }
        if !witnessed {
            report_scalar_refusal(
                report,
                &owner.identity,
                &evaluator_name(context, body),
                "checker supplied no complete scalar body witness",
            )?;
        }
        Ok(owner)
    }

    /// The event of one keyed product of this owner.
    #[allow(clippy::too_many_arguments)]
    fn event<'a>(
        &'a self,
        products: &'a str,
        inputs: &'a [String],
        function: &'a str,
        reused: bool,
        runtime: usize,
        references: &'a [NominalReference],
        positions: &'a [(i64, i64)],
    ) -> ProductEvent<'a> {
        ProductEvent {
            products,
            inputs,
            identity: &self.identity,
            function,
            reused,
            runtime,
            references,
            positions,
            module: &self.module,
        }
    }
}

/// Restore a body of an admitted module from its final event in the module's plan record.
///
/// Its function, helper frames and generic instance requests come from the event. An event that does not restore, as
/// when the owner moved, the witness changed or a named declaration is gone, leaves the body on the keyed route.
fn restore_admitted(
    context: &BodyIrModule,
    program: &ProgramContext,
    body: &Body,
    frame: &FrameSource<'_>,
    intrinsics: &[EnumDeclaration],
    keyed: &mut KeyedProducts<'_>,
    admission: &mut ModuleAdmission<'_>,
) -> Option<LoweredFrame> {
    if admission.page.events.is_empty() {
        return None;
    }
    let name = evaluator_name(context, body);
    let selected = admission.page.event(&name)?;
    let current_identity = body
        .canonical
        .as_ref()
        .map(encode_incan_symbol_identity)
        .unwrap_or_default();
    // A physical-symbol fallback has no independent stable owner to carry across a rename or move.
    if selected.identity == selected.symbol && selected.symbol != current_identity {
        return None;
    }
    let reads = keyed.admitted.get(&body.direct_call_id)?;
    let lines = admission.placement_lines;
    let span = if lines.is_empty() {
        location(&body.span, frame.source, frame.file)
    } else {
        indexed_location(lines, &body.span, frame.file)
    };
    let current = (!lines.is_empty()).then(|| indexed_body_positions(body, lines, frame.file, &span));
    sync_carriers(&mut keyed.nominals, intrinsics);
    let generation = keyed.generation.as_deref().unwrap_or("");
    let (payload, identities, encoded, previous) = admitted_payload(
        &selected.event,
        &span,
        &name,
        program.overflow_checks,
        reads,
        generation,
        current.clone(),
    )?;
    if previous.key != selected.key || previous.function != selected.function || previous.identity != selected.symbol {
        return None;
    }
    let resolved = resolve_nominals(&identities, &keyed.nominals)?;
    let (function, runtime, frames) = admitted_function(&payload, &resolved)?;
    let requests = restore_requests(&encoded, context)?;
    let references: Vec<_> = resolved.iter().map(event_reference).collect();
    let nodes = match &current {
        Some(positions) => event_nodes(positions),
        None => previous.positions.clone(),
    };
    let moved =
        current_identity != previous.identity || references != previous.references || nodes != previous.positions;
    admission.restored.push(AdmittedFunction {
        function: function.name.clone(),
        identity: current_identity,
        references,
        placement: moved.then(|| AdmittedPlacement {
            key: previous.key.clone(),
            positions: nodes,
        }),
    });
    Some((function, runtime, requests, frames))
}

/// A traced frame's module and source name, as the reuse proof matches bodies across captures.
fn frame_label(body: &Body) -> String {
    format!("{}::{}", body_module_identity(body).unwrap_or_default(), body.name)
}

/// Apply standalone entry naming against the current modules or their already validated retained bodies.
pub fn validate_program_entry_selection(
    modules: &[BodyIrModule],
    entry: usize,
    retained: &BodyIrModule,
) -> Result<(), String> {
    for identity in &modules[entry].program_entrypoints {
        let Some(owner) = canonical_module_identity(identity) else {
            return Err("selected program entry has no canonical module".into());
        };
        let mut found = false;
        if let Some(module) = modules.iter().find(|module| module.module_id.path() == owner) {
            found = validate_selected_entry_body(module, identity)?;
        }
        if !found {
            found = validate_retained_entry_body(retained, identity)?;
        }
        if !found {
            return Err("selected program entry has no owning module body".into());
        }
    }
    Ok(())
}

fn validate_selected_entry_body(module: &BodyIrModule, identity: &CanonicalSymbolId) -> Result<bool, String> {
    for body in &module.bodies {
        if body.canonical.as_ref() == Some(identity) {
            if function_name(module, body) != "main" {
                return Err("selected program entry is not a standalone main".into());
            }
            return Ok(true);
        }
    }
    Ok(false)
}

fn validate_retained_entry_body(context: &BodyIrModule, identity: &CanonicalSymbolId) -> Result<bool, String> {
    for body in &context.bodies {
        if body.canonical.as_ref() == Some(identity) {
            if body.name != "main" {
                return Err("selected program entry is not a standalone main".into());
            }
            return Ok(true);
        }
    }
    Ok(false)
}

/// Keep the unit's checked selector order while resolving canonical entries to their native wrappers.
pub fn selected_program_entries(context: &BodyIrModule) -> Result<Vec<String>, String> {
    let mut entries = Vec::new();
    for identity in &context.program_entrypoints {
        match context
            .bodies
            .iter()
            .find(|body| body.canonical.as_ref() == Some(identity))
        {
            Some(body) => entries.push(function_name(context, body)),
            None => return Err("selected program entry has no retained body".into()),
        }
    }
    Ok(entries)
}

/// The native function that runs an async body under the executor: `main`, or a collected test's wrapper.
fn async_entry_name(context: &BodyIrModule, body: &Body, in_entry: bool, tests: &[String]) -> Option<String> {
    if !body.is_async {
        return None;
    }
    let native = function_name(context, body);
    if is_program_entry(context, body) {
        return Some(native);
    }
    if in_entry && tests.contains(&body.name) {
        return Some(async_test_name(&native));
    }
    None
}

/// Add the native function that runs an async `main` or collected async test, when the body is one.
fn append_async_entry(
    context: &BodyIrModule,
    body: &Body,
    function: &Function,
    in_entry: bool,
    tests: &[String],
    intrinsics: &mut Vec<EnumDeclaration>,
    inventories: &mut Inventories,
) -> Result<(), String> {
    if let Some(entry_name) = async_entry_name(context, body, in_entry, tests) {
        let (entry_function, entry_constructor, entry_runtime) =
            async_entry(context, function, intrinsics, &entry_name)?;
        inventories.functions.push(entry_function);
        inventories.functions.push(entry_constructor);
        merge_externals(&mut inventories.externals, vec![entry_runtime])?;
    }
    Ok(())
}

/// Resolve zero-argument tests returning None or Result[None, E]; rustc's test harness reports Err as failure.
pub fn collected_test_entries(
    context: &BodyIrModule,
    entry_owner: &str,
    tests: &[String],
    functions: &[Function],
    enums: &[EnumDeclaration],
) -> Result<Vec<String>, String> {
    let mut entries = Vec::new();
    for name in tests {
        let mut found = false;
        for body in &context.bodies {
            if body_module_identity(body)? != entry_owner || body.name != *name {
                continue;
            }
            let native = function_name(context, body);
            let evaluator = evaluator_name(context, body);
            for function in functions {
                if function.name != evaluator {
                    continue;
                }
                if !function.parameters.is_empty() || !test_return_type(&function.return_type, enums) {
                    return Err(format!(
                        "unsupported test signature for {name}: requires () -> None or Result[None, E]"
                    ));
                }
                if function.is_async {
                    entries.push(async_test_name(&native));
                } else {
                    entries.push(native.clone());
                }
                found = true;
            }
        }
        if !found {
            return Err(format!("unsupported collected test body {name}"));
        }
    }
    Ok(entries)
}

/// Admit only unit success values, preserving a standard Result's native failure and Debug contract.
fn test_return_type(ty: &PlanType, enums: &[EnumDeclaration]) -> bool {
    if *ty == PlanType::Unit {
        return true;
    }
    let PlanType::Enum(owner, _) = ty else {
        return false;
    };
    if *owner < 0 || *owner as usize >= enums.len() {
        return false;
    }
    let declaration = &enums[*owner as usize];
    if declaration.carrier != Carrier::Result || declaration.variants.len() != 2 {
        return false;
    }
    declaration.variants[0].fields == [PlanType::Unit] && declaration.variants[1].fields.len() == 1
}

/// Export every body and type a Loaf compiles at its crate root, under the encoding of its canonical identity.
///
/// A body emitted in several frames under one identity has no single item, so none is exported for it. A generic
/// body emits no item. Generic nominal exports name their retained templates.
pub fn loaf_exports(
    context: &BodyIrModule,
    emitted: &[usize],
    declarations: &BodyIrModule,
    templates: &[GenericModelDeclaration],
    enum_templates: &[GenericEnumDeclaration],
) -> Vec<PlanExport> {
    let mut exports: Vec<PlanExport> = Vec::new();
    let mut ambiguous: Vec<String> = Vec::new();
    for position in emitted {
        let body = &context.bodies[*position];
        if let Some(identity) = &body.canonical
            && is_package_identity(identity)
        {
            let name = encode_incan_symbol_identity(identity);
            if exports.iter().any(|export| export.name == name) {
                ambiguous.push(name);
            } else {
                exports.push(PlanExport {
                    name,
                    path: format!("crate::{}", function_name(context, body)),
                });
            }
        }
    }
    for declaration in &declarations.nominal_declarations {
        if !declaration.type_parameters.is_empty()
            && !templates.iter().any(|template| template.name == declaration.name)
        {
            continue;
        }
        if declaration.native_item.is_empty() && is_package_identity(&declaration.canonical) {
            exports.push(PlanExport {
                name: encode_incan_symbol_identity(&declaration.canonical),
                path: format!("crate::{}", declaration.name),
            });
        }
    }
    for declaration in &declarations.enum_declarations {
        if !declaration.type_parameters.is_empty()
            && !enum_templates
                .iter()
                .any(|template| template.name == declaration.carrier_name())
        {
            continue;
        }
        if declaration.native_item.is_empty() && is_package_identity(&declaration.canonical) {
            exports.push(PlanExport {
                name: encode_incan_symbol_identity(&declaration.canonical),
                path: format!("crate::{}", declaration.name),
            });
        }
    }
    exports
        .into_iter()
        .filter(|export| !ambiguous.contains(&export.name))
        .collect()
}

/// Whether a package declares `identity`, so a Loaf compiles it.
fn is_package_identity(identity: &CanonicalSymbolId) -> bool {
    matches!(identity.origin, SymbolOrigin::Package { .. })
}

/// Whether a checked module's body lowers as an ordinary function, for compilation and the census alike.
pub fn ordinary_body(context: &BodyIrModule, body: &Body, emitted_frames: &EmittedFrames, kind: &str) -> bool {
    if !body.type_parameters.is_empty() || is_lazy_method_body(context, body) || emitted_frames.contains(context, body)
    {
        return false;
    }
    if kind == "test" && function_name(context, body) == "main" {
        return false;
    }
    context.program_entrypoints.is_empty() || function_name(context, body) != "main"
}

/// Schedule the lazy method bodies that native comparison bridges call.
pub fn comparison_bridge_instances(
    context: &BodyIrModule,
    models: &[ModelDeclaration],
    enums: &[EnumDeclaration],
) -> Vec<Instance> {
    let mut bridge_bodies: Vec<String> = Vec::new();
    for declaration in models {
        bridge_bodies.extend(declaration.comparison_methods.iter().cloned());
    }
    for declaration in enums {
        bridge_bodies.extend(declaration.comparison_methods.iter().cloned());
    }
    let mut instances = Vec::new();
    for body in &context.bodies {
        if is_lazy_method_body(context, body) && bridge_bodies.contains(&function_name(context, body)) {
            request_instance(&mut instances, body, Vec::new(), Vec::new(), false);
        }
    }
    instances
}
