//! Collect the named refusals of a program's bodies and contexts, for the direct-route census.

use std::collections::BTreeMap;
use std::sync::Arc;

use incan_mir_plan_native::{EnumDeclaration, StaticCell};
use incan_semantics_core::IncanType;
use incan_semantics_core::body_ir::BodyIrModule;

use crate::classes::body_module_identity;
use crate::decorator_caches::DecoratorFrame;
use crate::frame_instances::{lower_instances, merge_externals};
use crate::function_frames::lower_function;
use crate::lowering::{Inventories, PlanContexts, initial_plan_roots, layout_tables, plan_contexts, source_file_index};
use crate::native_type_identity::{
    ConstructorIndex, constructor_index_with_packages as index_constructors, with_generic_constructor_arities,
};
use crate::nominal_instances::NominalInstance;
use crate::program_context::ProgramContext;
use crate::program_entries::{no_emitted_frames, ordinary_body};
use crate::runtime_requirements::{RefusalPlanning, RuntimeRetention};
use crate::static_storage::lower_computed_statics;
use crate::type_parameters::merge_instance_requests;

/// The checked inputs of one census request.
pub struct CensusInputs<'a> {
    pub sources: &'a [String],
    pub files: &'a [String],
    pub entry: i64,
    pub progress: bool,
    pub selection: &'a str,
    pub kind: &'a str,
    pub package_versions: &'a BTreeMap<String, String>,
    pub overflow_checks: bool,
}

/// Collect every body's first refusal rather than only the program's first, for the direct-route census.
///
/// The declaration context is prepared as `unit_plan` prepares it, and a refusal there is the only one reported,
/// because no body lowers without that context. Each ordinary body then lowers independently, so a refusal in one body
/// never hides another body's, and checked type-parameter instances follow once every body has been tried. Each
/// refusal names the body it came from. The plan itself is discarded. With `progress`, stdout names each body before
/// lowering starts, so a long-running census identifies its current boundary without process sampling. A nonempty
/// `selection` checks only ordinary bodies with that exact name; it does not compile their dependencies.
pub fn program_refusals(modules: &[BodyIrModule], inputs: &CensusInputs<'_>) -> RefusalPlanning {
    if modules.is_empty() || inputs.entry < 0 || inputs.entry as usize >= modules.len() {
        return RefusalPlanning::Complete(vec!["invalid Body IR project entry".into()]);
    }
    if inputs.sources.len() != modules.len() || inputs.files.len() != modules.len() {
        return RefusalPlanning::Complete(vec!["invalid Body IR project source provenance".into()]);
    }
    match body_refusals(modules, inputs) {
        Ok(refusals) => refusals,
        Err(error) => RefusalPlanning::Complete(vec![error]),
    }
}

/// Prepare the declaration context exactly as `unit_plan` does, refusing as it refuses, then lower each ordinary body
/// on its own and record the first refusal of every body that refuses.
///
/// The bodies lowered and the comparison bridge instances scheduled are the ones compilation selects: both read
/// `ordinary_body` and `comparison_bridge_instances`.
fn body_refusals(modules: &[BodyIrModule], inputs: &CensusInputs<'_>) -> Result<RefusalPlanning, String> {
    let entry = inputs.entry as usize;
    let source_files = source_file_index(modules, inputs.files)?;
    let empty_templates = index_constructors(&[], &source_files, inputs.package_versions);
    let PlanContexts {
        source_context,
        template_constructors,
        owners,
        context,
        nominal_instances,
        ..
    } = plan_contexts(
        modules,
        inputs.sources,
        inputs.files,
        inputs.entry,
        false,
        &source_files,
        inputs.package_versions,
        &modules[entry],
        &empty_templates,
    )?;
    let mut context = context;
    let (constructor_table, unions, initial_intrinsics) = layout_tables(
        &context,
        &inputs.files[entry],
        &source_files,
        inputs.package_versions,
        Vec::new(),
    )?;
    let constructors = Arc::new(with_generic_constructor_arities(
        constructor_table,
        &template_constructors,
    ));
    let mut intrinsics = initial_intrinsics;
    let mut inventories = Inventories::default();
    let mut program = initial_plan_roots(
        constructors,
        &context,
        &inputs.files[entry],
        &mut intrinsics,
        &nominal_instances,
        inputs.kind,
        &unions,
        inputs.overflow_checks,
        false,
        &mut inventories,
    )?;
    let mut refusals = Vec::new();
    let selected = ordinary_refusals(
        &context,
        &program,
        &owners,
        inputs,
        &unions,
        &mut intrinsics,
        &mut inventories,
        &mut refusals,
    );
    if !inputs.selection.is_empty() {
        finish_selected_census(inputs.selection, selected, inputs.progress, &mut refusals);
        return Ok(RefusalPlanning::Complete(refusals));
    }
    let lowered = ContextRefusals {
        template_constructors: &template_constructors,
        templates: &source_context,
        unions: &unions,
        sources: inputs.sources,
        files: inputs.files,
        owners: &owners,
        nominal_instances,
    };
    let retention = append_context_refusals(
        lowered,
        &mut context,
        &mut program,
        &mut intrinsics,
        &mut inventories,
        &mut refusals,
    );
    if let RuntimeRetention::NeedsRuntime(requests) = retention {
        return Ok(RefusalPlanning::NeedsRuntime(requests));
    }
    Ok(RefusalPlanning::Complete(refusals))
}

/// Lower each ordinary body of every owner module on its own, recording each refusal under its body's name, and
/// return how many bodies the selection admitted.
#[allow(clippy::too_many_arguments)]
fn ordinary_refusals(
    context: &BodyIrModule,
    program: &ProgramContext,
    owners: &[String],
    inputs: &CensusInputs<'_>,
    unions: &[IncanType],
    intrinsics: &mut Vec<EnumDeclaration>,
    inventories: &mut Inventories,
    refusals: &mut Vec<String>,
) -> usize {
    let mut selected = 0;
    let mut emitted_frames = no_emitted_frames();
    for (index, checked_owner) in owners.iter().enumerate() {
        for (position, body) in context.bodies.iter().enumerate() {
            match body_module_identity(body) {
                Ok(owner) => {
                    if owner != *checked_owner {
                        continue;
                    }
                }
                Err(error) => {
                    refusals.push(format!("{}: {error}", body.name));
                    continue;
                }
            }
            if !ordinary_body(context, body, &emitted_frames, inputs.kind) {
                continue;
            }
            // A frame reached again is skipped as compilation skips it, whether or not its first lowering refused.
            emitted_frames.record(body, position);
            if !inputs.selection.is_empty() && body.name != inputs.selection {
                continue;
            }
            selected += 1;
            if inputs.progress {
                println!("census body: {}", body.name);
            }
            let lowered = lower_function(
                context,
                program,
                body,
                unions,
                &inputs.sources[index],
                &inputs.files[index],
                intrinsics,
                &[],
                DecoratorFrame::Normal,
            );
            match lowered {
                Ok((function, runtime, pending, synthesized)) => {
                    merge_instance_requests(&mut inventories.instances, &pending);
                    inventories.functions.push(function);
                    inventories.functions.extend(synthesized);
                    if let Err(error) = merge_externals(&mut inventories.externals, runtime) {
                        refusals.push(format!("{}: {error}", body.name));
                    }
                }
                Err(error) => refusals.push(format!("{}: {error}", body.name)),
            }
        }
    }
    selected
}

/// Report a selected-body pass and diagnose a selection that matched no ordinary body.
fn finish_selected_census(selection: &str, selected: usize, progress: bool, refusals: &mut Vec<String>) {
    if selected == 0 {
        refusals.push(format!(
            "selected declaration has no ordinary function body: {selection}"
        ));
    }
    if progress {
        println!("census complete: selected ordinary bodies only; no artifact compiled");
    }
}

/// The prepared tables the context refusals read.
struct ContextRefusals<'a> {
    template_constructors: &'a ConstructorIndex,
    templates: &'a BodyIrModule,
    unions: &'a [IncanType],
    sources: &'a [String],
    files: &'a [String],
    owners: &'a [String],
    nominal_instances: Vec<NominalInstance>,
}

/// Collect static-initialization and instance refusals after ordinary bodies have contributed their requests.
fn append_context_refusals(
    tables: ContextRefusals<'_>,
    context: &mut BodyIrModule,
    program: &mut ProgramContext,
    intrinsics: &mut Vec<EnumDeclaration>,
    inventories: &mut Inventories,
    refusals: &mut Vec<String>,
) -> RuntimeRetention {
    if let Err(error) = lower_computed_statics(
        context,
        program,
        tables.unions,
        tables.sources,
        tables.files,
        tables.owners,
        intrinsics,
        inventories,
        0,
    ) {
        refusals.push(format!("computed static: {error}"));
    }
    let mut retained_nominals = tables.nominal_instances;
    let mut instance_cells: Vec<StaticCell> = Vec::new();
    let mut instances = std::mem::take(&mut inventories.instances);
    let retention = lower_instances(
        tables.template_constructors,
        context,
        tables.templates,
        program,
        &mut instances,
        tables.unions,
        tables.sources,
        tables.files,
        tables.owners,
        inventories,
        intrinsics,
        &mut retained_nominals,
        &mut instance_cells,
        None,
    );
    inventories.instances = instances;
    match retention {
        Ok(RuntimeRetention::NeedsRuntime(requests)) => RuntimeRetention::NeedsRuntime(requests),
        Ok(RuntimeRetention::Complete) => RuntimeRetention::Complete,
        Err(error) => {
            refusals.push(format!("type-parameter instance: {error}"));
            RuntimeRetention::Complete
        }
    }
}
