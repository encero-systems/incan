//! Lower the frontend's canonical checked Body IR into a native plan.
//!
//! The Body IR is read through the real `incan_semantics_core` types, borrowed, never a serialized or mirrored copy.
//! Every module is validated before any lowering reads its identities, and every construct outside the supported
//! slice is refused by name rather than lowered approximately. This module owns the boundary types, project
//! validation, and plan assembly; program entries, function frames and each construct's lowering live in their own
//! modules.

use std::collections::BTreeMap;
use std::sync::Arc;

use incan_mir_plan_native::{
    Callee, EnumDeclaration, ExternalFunction, Function, GenericEnumDeclaration, GenericModelDeclaration,
    ModelDeclaration, Plan, PlanExport, StaticCell, ZipCarrier, fold_carrier_positions, fold_inventories, source_span,
};
use incan_semantics_core::body_ir::{BodyIrModule, LinkedStaticAccessors, NativeUnionDeclaration};
use incan_semantics_core::{CanonicalSymbolId, CompilerNodeId, IncanType};

use crate::async_::validate_futures;
use crate::body_index::body_index;
use crate::checked_json::lower_checked_deserialize;
use crate::classes::{body_module_identity, is_program_entry};
use crate::comparison_methods::{attach_enum_comparisons, comparison_functions};
use crate::continuation_context::{continuation_inputs, extend_continuation_context, refresh_entry_context};
use crate::decls::{lower_model_declarations, publish_local_layouts};
use crate::decorator_caches::validate_instance_binding;
use crate::display_methods::display_functions;
use crate::enum_instances::{specialize_enums, specialized_enum_layouts};
use crate::enums::lower_enum_declarations;
use crate::frame_instances::{lower_instances, merge_externals};
use crate::generic_enums::generic_enums;
use crate::generic_layouts::{generic_models, referenced_templates};
use crate::instance_products::{InstanceProducts, instance_transport};
use crate::native_type_identity::{
    ConstructorIndex, constructor_index_with_packages as index_constructors, extend_constructor_authority,
    extend_constructor_index, extend_generic_constructor_arities, with_generic_constructor_arities,
};
use crate::nominal_instances::{NominalInstance, specialize_nominals_with_instances};
use crate::options::intrinsic_declarations;
use crate::program_context::{ProgramContext, extend_program_context};
use crate::program_entries::{
    EmittedFrames, collected_test_entries, comparison_bridge_instances, loaf_exports, lower_ordinary_bodies,
    no_emitted_frames, selected_program_entries, validate_program_entry_selection,
};
use crate::project::{project_context, project_context_parallel};
use crate::runtime_calls::selected_runtime_requirements;
use crate::runtime_layouts::runtime_zip_carriers;
use crate::runtime_requirements::{RuntimeRequirement, RuntimeRetention};
use crate::static_link_exports::publish_static_accessors;
use crate::static_storage::lower_computed_statics;
use crate::string_conversion::lower_string_conversions;
use crate::string_conversion_contracts::validate_conversion_placement;
use crate::trace_channel::{emit, traced};
use crate::trait_links::{has_open_trait_arguments, validate_project_implementation};
use crate::type_parameters::{Instance, merge_instance_requests};
use crate::unions::{collect_unions, extend_unions, publish_unions, union_index};

/// Ordinary-frame work retained while the frontend resolves only the runtime declarations that work selected.
pub struct UnitContinuation {
    pub source_context: Arc<BodyIrModule>,
    pub template_constructors: Arc<ConstructorIndex>,
    pub owners: Vec<String>,
    pub sources: Vec<String>,
    pub files: Vec<String>,
    pub entry_owner: String,
    pub entry_file: String,
    pub context: Arc<BodyIrModule>,
    pub template_references: Vec<String>,
    pub constructors: Arc<ConstructorIndex>,
    pub program: ProgramContext,
    pub unions: Vec<IncanType>,
    pub nominal_instances: Vec<NominalInstance>,
    pub intrinsic_layouts: Vec<EnumDeclaration>,
    pub models: Vec<ModelDeclaration>,
    pub enums: Vec<EnumDeclaration>,
    pub functions: Vec<Function>,
    pub externals: Vec<ExternalFunction>,
    pub instances: Vec<Instance>,
    pub emitted_frames: EmittedFrames,
    pub statics: Vec<StaticCell>,
    /// The keyed-product transport its closed instances read and retain in a records request.
    pub instance_products: Option<InstanceProducts>,
}

/// A complete native plan, or preserved ordinary-frame work awaiting exact runtime declarations.
#[allow(clippy::large_enum_variant)]
pub enum UnitPlanning {
    Complete(Option<Plan>),
    NeedsRuntime(UnitContinuation, Vec<RuntimeRequirement>),
}

/// Validate one module, resolving trait implementations against its current slice and retained project authority.
fn validate_project_module(
    module: &BodyIrModule,
    modules: &[BodyIrModule],
    retained: &BodyIrModule,
) -> Result<usize, String> {
    let count = validate_declarations_and_bodies(module, modules)?;
    for implementation in &module.trait_implementations {
        if let Err(reason) = validate_project_implementation(module, implementation, modules, retained) {
            if has_open_trait_arguments(implementation) {
                return Err("unsupported Body IR open trait default type arguments".into());
            }
            return Err(format!(
                "{reason}; owner={:?}; slot={:?}; implementation={:?}",
                implementation.owner, implementation.method, implementation.implementation
            ));
        }
    }
    Ok(count)
}

/// Validate a module's retained declarations and bodies; trait implementations are validated by the caller.
///
/// A generic decorator initializer is admitted only for a synchronous per-instantiation binding. Its closed instances
/// publish their own storage cells; they never enter ordinary module-static initialization or fall through to the
/// undecorated original.
fn validate_declarations_and_bodies(module: &BodyIrModule, modules: &[BodyIrModule]) -> Result<usize, String> {
    for binding in &module.decorated_bindings {
        if !module.is_well_formed_decorated_binding_in_project(binding, modules) {
            return Err("invalid Body IR decorated binding identity".into());
        }
        if binding.initializer.is_some() {
            validate_instance_binding(binding)?;
        }
    }
    for declaration in &module.static_declarations {
        if !module.is_well_formed_static_declaration_in_project(declaration, modules) {
            return Err("invalid Body IR static declaration".into());
        }
    }
    for declaration in &module.enum_declarations {
        if !module.is_well_formed_native_enum_declaration(declaration) {
            return Err("invalid Body IR normal enum declaration identity".into());
        }
    }
    for declaration in &module.nominal_declarations {
        if !module.is_well_formed_nominal_declaration(declaration) {
            return Err(format!(
                "invalid Body IR nominal declaration identity {} in {}",
                declaration.name,
                module.module_id.path()
            ));
        }
    }
    for declaration in &module.fieldless_enum_declarations {
        if !module.is_well_formed_fieldless_enum_declaration(declaration) {
            return Err("invalid Body IR fieldless enum declaration identity".into());
        }
    }
    for declaration in &module.value_enum_declarations {
        if !module.is_well_formed_value_enum_declaration(declaration) {
            return Err("invalid Body IR value enum declaration identity".into());
        }
    }
    for body in &module.bodies {
        if !module.body_has_canonical_direct_call_id(body) {
            return Err(format!(
                "unsupported Body IR body identity {}::{}",
                module.module_id.path(),
                body.name
            ));
        }
    }
    Ok(module.bodies.len())
}

/// The unit inputs the lowering reads besides its modules, aligned with them.
pub struct UnitInputs<'a> {
    pub sources: &'a [String],
    pub files: &'a [String],
    pub entry: i64,
    pub kind: &'a str,
    pub tests: &'a [String],
    pub package_versions: &'a BTreeMap<String, String>,
    pub overflow_checks: bool,
    pub scalar_products: Option<String>,
    pub scalar_bodies: &'a [(CompilerNodeId, String)],
    pub record_report: Option<String>,
    /// Producer-owned native exports chosen before lowering the Loaf.
    pub linked_static_exports: &'a BTreeMap<CanonicalSymbolId, LinkedStaticAccessors>,
}

/// Lower one checked unit by kind; test entries come from session collection, never from naming heuristics here.
pub fn unit_plan(
    modules: &[BodyIrModule],
    inputs: &UnitInputs<'_>,
    published: &mut Vec<NativeUnionDeclaration>,
) -> Result<UnitPlanning, String> {
    let kind = inputs.kind;
    if kind != "program" && kind != "library" && kind != "test" {
        return Err(format!("unsupported unit kind {kind}"));
    }
    let mut scalar_index: BTreeMap<CompilerNodeId, String> = BTreeMap::new();
    for (identity, witness) in inputs.scalar_bodies {
        scalar_index.insert(identity.clone(), witness.clone());
    }
    lowered_plan(modules, inputs, scalar_index, published)
}

/// Freeze program dispatch mappings after layout admission, copying storage once from the still-borrowed module.
pub fn program_context(
    constructors: Arc<ConstructorIndex>,
    module: &BodyIrModule,
    intrinsics: &mut Vec<EnumDeclaration>,
    unions: &[IncanType],
    overflow_checks: bool,
) -> Result<ProgramContext, String> {
    let constructor_view = constructors.as_ref();
    let display = display_functions(constructor_view, module, intrinsics)?;
    let comparisons = comparison_functions(constructor_view, module, intrinsics)?;
    Ok(ProgramContext {
        body_indices: body_index(module),
        display_functions: Arc::new(display),
        statics: Arc::new(module.static_declarations.clone()),
        comparison_functions: Arc::new(comparisons),
        union_types: union_index(unions),
        nominals: Arc::new(module.nominal_declarations.clone()),
        constructors: constructors.clone(),
        enums: Arc::new(module.enum_declarations.clone()),
        value_enums: Arc::new(module.value_enum_declarations.clone()),
        decorated_bindings: Arc::new(module.decorated_bindings.clone()),
        overflow_checks,
        observe_work: std::env::var_os("INCAN_NATIVE_TIMING").is_some(),
        described: Arc::default(),
    })
}

/// The plan inventories one attempt fills.
#[derive(Default)]
pub struct Inventories {
    pub models: Vec<ModelDeclaration>,
    pub enums: Vec<EnumDeclaration>,
    pub functions: Vec<Function>,
    pub externals: Vec<ExternalFunction>,
    pub instances: Vec<Instance>,
}

/// Lower a checked unit once, preserving ordinary frames when exact runtime declarations need resolution.
fn lowered_plan(
    modules: &[BodyIrModule],
    inputs: &UnitInputs<'_>,
    scalar_bodies: BTreeMap<CompilerNodeId, String>,
    published: &mut Vec<NativeUnionDeclaration>,
) -> Result<UnitPlanning, String> {
    let trace = traced("INCAN_TRACE_LOWERING");
    let sources = inputs.sources;
    let files = inputs.files;
    // 1. Validate the checked inputs and prepare their shared layouts.
    let source_files = source_file_index(modules, files)?;
    let entry = checked_entry(modules, inputs.entry)?;
    let empty_templates = index_constructors(&[], &source_files, inputs.package_versions);
    let contexts = plan_contexts(
        modules,
        sources,
        files,
        inputs.entry,
        trace,
        &source_files,
        inputs.package_versions,
        &modules[entry],
        &empty_templates,
    )?;
    let PlanContexts {
        source_context,
        template_constructors,
        owners,
        context,
        nominal_instances,
        template_references,
    } = contexts;
    let shared_source_context = Arc::new(source_context);
    let context = Arc::new(context);
    let (constructor_table, unions, initial_intrinsics) = layout_tables(
        &context,
        &files[entry],
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
        constructors.clone(),
        &context,
        &files[entry],
        &mut intrinsics,
        &nominal_instances,
        inputs.kind,
        &unions,
        inputs.overflow_checks,
        trace,
        &mut inventories,
    )?;
    let mut emitted_frames = no_emitted_frames();
    // 2. Lower ordinary frames and static initializers exactly once.
    let instance_products = instance_transport(
        &context,
        inputs.scalar_products.clone(),
        &scalar_bodies,
        inputs.record_report.clone(),
        inputs.overflow_checks,
    );
    lower_ordinary_bodies(
        &context,
        &shared_source_context,
        &source_files,
        inputs.package_versions,
        &mut program,
        &owners,
        sources,
        files,
        entry,
        inputs.kind,
        inputs.tests,
        &mut intrinsics,
        &mut inventories,
        &mut emitted_frames,
        &unions,
        trace,
        inputs.scalar_products.clone(),
        &scalar_bodies,
        inputs.overflow_checks,
        inputs.record_report.clone(),
    )?;
    let statics = lower_computed_statics(
        &context,
        &mut program,
        &unions,
        sources,
        files,
        &owners,
        &mut intrinsics,
        &mut inventories,
        0,
    )?;
    let requirements = selected_runtime_requirements(&context, &inventories.externals, &inventories.functions)?;
    if !requirements.is_empty() {
        return Ok(UnitPlanning::NeedsRuntime(
            UnitContinuation {
                source_context: shared_source_context,
                template_constructors: Arc::new(template_constructors),
                entry_owner: owners[entry].clone(),
                entry_file: files[entry].clone(),
                owners,
                sources: sources.to_vec(),
                files: files.to_vec(),
                context,
                template_references,
                constructors,
                program,
                unions,
                nominal_instances,
                intrinsic_layouts: intrinsics,
                models: inventories.models,
                enums: inventories.enums,
                functions: inventories.functions,
                externals: inventories.externals,
                instances: inventories.instances,
                emitted_frames,
                statics,
                instance_products,
            },
            requirements,
        ));
    }
    // 3. Complete instances and publish the plan when no declaration resolution is needed.
    let entry_owner = owners[entry].clone();
    let entry_file = files[entry].clone();
    finish_unit_plan(
        Finishing {
            template_constructors: Arc::new(template_constructors),
            constructors,
            source_context: shared_source_context,
            template_references,
            context,
            program,
            owners,
            sources: sources.to_vec(),
            files: files.to_vec(),
            entry_owner,
            entry_file,
            intrinsics,
            inventories,
            emitted_frames,
            statics,
            unions,
            retained_nominals: nominal_instances,
            products: instance_products,
        },
        inputs.kind,
        inputs.tests,
        inputs.linked_static_exports,
        published,
        trace,
    )
}

/// The entry position, refused before any table is indexed by it.
fn checked_entry(modules: &[BodyIrModule], entry: i64) -> Result<usize, String> {
    if modules.is_empty() || entry < 0 || entry as usize >= modules.len() {
        return Err("invalid Body IR project entry".into());
    }
    Ok(entry as usize)
}

/// Resume preserved ordinary frames against the exact runtime declaration closure the frontend resolved.
pub fn resume_unit_plan(
    continuation: UnitContinuation,
    modules: &[BodyIrModule],
    inputs: &UnitInputs<'_>,
    published: &mut Vec<NativeUnionDeclaration>,
) -> Result<UnitPlanning, String> {
    let trace = traced("INCAN_TRACE_LOWERING");
    validate_plan_inputs(
        modules,
        inputs.sources,
        inputs.files,
        inputs.entry,
        &continuation.context,
    )?;
    let entry = checked_entry(modules, inputs.entry)?;
    let source_files = source_file_index(modules, inputs.files)?;
    let UnitContinuation {
        mut source_context,
        mut template_constructors,
        mut owners,
        sources: mut retained_sources,
        files: mut retained_files,
        entry_owner,
        entry_file,
        mut context,
        mut template_references,
        mut constructors,
        mut program,
        mut unions,
        nominal_instances: mut retained_nominals,
        intrinsic_layouts: mut intrinsics,
        models,
        enums,
        functions,
        externals,
        instances,
        emitted_frames,
        mut statics,
        instance_products,
    } = continuation;
    let mut inventories = Inventories {
        models,
        enums,
        functions,
        externals,
        instances,
    };
    let previous_nominal_count = context.nominal_declarations.len();
    let previous_model_count = inventories.models.len();
    let previous_static_count = context.static_declarations.len();
    refresh_entry_context(Arc::make_mut(&mut context), &modules[entry]);
    let additions = continuation_inputs(
        &source_context,
        &context,
        &owners,
        modules,
        inputs.sources,
        inputs.files,
        entry,
    )?;
    if !additions.modules.is_empty() {
        let addition_source_files = source_file_index(&additions.modules, &additions.files)?;
        let added = plan_contexts_parallel(
            additions.modules.clone(),
            &additions.sources,
            &additions.files,
            additions.entry as i64,
            false,
            &addition_source_files,
            inputs.package_versions,
            &context,
            &template_constructors,
        )?;
        extend_continuation_context(
            Arc::make_mut(&mut source_context),
            Arc::make_mut(&mut context),
            Arc::make_mut(&mut template_constructors),
            &mut owners,
            &mut retained_sources,
            &mut retained_files,
            &mut template_references,
            &mut retained_nominals,
            &additions,
            &added.source_context,
            &added.context,
            &added.template_references,
            &added.nominal_instances,
        )?;
    }
    extend_constructor_authority(
        Arc::make_mut(&mut template_constructors),
        &source_files,
        inputs.package_versions,
    );
    extend_constructor_index(Arc::make_mut(&mut constructors), &context.nominal_declarations);
    extend_generic_constructor_arities(Arc::make_mut(&mut constructors), &template_constructors);
    extend_constructor_authority(Arc::make_mut(&mut constructors), &source_files, inputs.package_versions);
    extend_unions(&mut unions, &context);
    intrinsics = intrinsic_declarations(&constructors, &context, &entry_file, intrinsics)?;
    resume_plan_roots(
        &mut program,
        constructors.clone(),
        &context,
        &entry_file,
        &mut intrinsics,
        &retained_nominals,
        inputs.kind,
        &unions,
        previous_nominal_count,
        previous_model_count,
        &mut inventories,
    )?;
    let resolved_statics = lower_computed_statics(
        &context,
        &mut program,
        &unions,
        &retained_sources,
        &retained_files,
        &owners,
        &mut intrinsics,
        &mut inventories,
        previous_static_count,
    )?;
    let emitted_frames = rebase_emitted_frames(&context, &emitted_frames);
    statics.extend(resolved_statics);
    let requirements = selected_runtime_requirements(&context, &inventories.externals, &inventories.functions)?;
    if !requirements.is_empty() {
        return Ok(UnitPlanning::NeedsRuntime(
            UnitContinuation {
                source_context,
                template_constructors,
                owners,
                sources: retained_sources,
                files: retained_files,
                entry_owner,
                entry_file,
                context,
                template_references,
                constructors,
                program,
                unions,
                nominal_instances: retained_nominals,
                intrinsic_layouts: intrinsics,
                models: inventories.models,
                enums: inventories.enums,
                functions: inventories.functions,
                externals: inventories.externals,
                instances: inventories.instances,
                emitted_frames,
                statics,
                instance_products,
            },
            requirements,
        ));
    }
    finish_unit_plan(
        Finishing {
            template_constructors,
            constructors,
            source_context,
            template_references,
            context,
            program,
            owners,
            sources: retained_sources,
            files: retained_files,
            entry_owner,
            entry_file,
            intrinsics,
            inventories,
            emitted_frames,
            statics,
            unions,
            retained_nominals,
            products: instance_products,
        },
        inputs.kind,
        inputs.tests,
        inputs.linked_static_exports,
        published,
        trace,
    )
}

/// Rebase ordinary-frame body positions after resolved dependency modules enter before authored modules.
fn rebase_emitted_frames(context: &BodyIrModule, previous: &EmittedFrames) -> EmittedFrames {
    let mut emitted = no_emitted_frames();
    for (position, body) in context.bodies.iter().enumerate() {
        if let Some(identity) = &body.canonical
            && previous.positions.contains_key(identity)
        {
            emitted.record(body, position);
        }
    }
    emitted
}

/// Everything one attempt carries into its completion.
struct Finishing {
    template_constructors: Arc<ConstructorIndex>,
    constructors: Arc<ConstructorIndex>,
    source_context: Arc<BodyIrModule>,
    template_references: Vec<String>,
    context: Arc<BodyIrModule>,
    program: ProgramContext,
    owners: Vec<String>,
    sources: Vec<String>,
    files: Vec<String>,
    entry_owner: String,
    entry_file: String,
    intrinsics: Vec<EnumDeclaration>,
    inventories: Inventories,
    emitted_frames: EmittedFrames,
    statics: Vec<StaticCell>,
    unions: Vec<IncanType>,
    retained_nominals: Vec<NominalInstance>,
    products: Option<InstanceProducts>,
}

/// Drain instances from preserved ordinary frames, finalize their layouts and publish one complete plan.
fn finish_unit_plan(
    finishing: Finishing,
    kind: &str,
    tests: &[String],
    linked_static_exports: &BTreeMap<CanonicalSymbolId, LinkedStaticAccessors>,
    published: &mut Vec<NativeUnionDeclaration>,
    trace: bool,
) -> Result<UnitPlanning, String> {
    let Finishing {
        template_constructors,
        constructors,
        source_context,
        template_references,
        mut context,
        program,
        owners,
        sources,
        files,
        entry_owner,
        entry_file,
        mut intrinsics,
        mut inventories,
        emitted_frames,
        mut statics,
        unions,
        mut retained_nominals,
        products,
    } = finishing;
    let mut retained_program = program;
    let original_nominal_count = context.nominal_declarations.len();
    trace_lowering(trace, "generic instances");
    let retry_intrinsic_layouts = intrinsics.clone();
    let original_functions = inventories.functions.len();
    // Prior to runtime closure, lowering only appends frames and binds deferred future callees. Preserve that small
    // mutable surface; the native blocks of existing functions are rewritten only on success.
    let future_callees: Vec<(usize, Callee)> = inventories.functions[..original_functions]
        .iter()
        .enumerate()
        .filter_map(|(index, function)| function.future_callee.clone().map(|callee| (index, callee)))
        .collect();
    let retry_externals = inventories.externals.clone();
    let retry_instances = inventories.instances.clone();
    let mut instance_cells: Vec<StaticCell> = Vec::new();
    let mut instances = std::mem::take(&mut inventories.instances);
    let retention = lower_instances(
        &template_constructors,
        Arc::make_mut(&mut context),
        &source_context,
        &mut retained_program,
        &mut instances,
        &unions,
        &sources,
        &files,
        &owners,
        &mut inventories,
        &mut intrinsics,
        &mut retained_nominals,
        &mut instance_cells,
        products.as_ref(),
    )?;
    inventories.instances = instances;
    match retention {
        RuntimeRetention::NeedsRuntime(requests) => {
            inventories.functions.truncate(original_functions);
            for (index, callee) in future_callees {
                inventories.functions[index].future_callee = Some(callee);
            }
            let retry = UnitContinuation {
                source_context,
                template_constructors,
                owners,
                sources,
                files,
                entry_owner,
                entry_file,
                context,
                template_references,
                constructors,
                program: retained_program,
                unions,
                nominal_instances: retained_nominals,
                intrinsic_layouts: retry_intrinsic_layouts,
                models: inventories.models,
                enums: inventories.enums,
                functions: inventories.functions,
                externals: retry_externals,
                instances: retry_instances,
                emitted_frames,
                statics,
                instance_products: products,
            };
            return Ok(UnitPlanning::NeedsRuntime(retry, requests));
        }
        RuntimeRetention::Complete => {
            // A completed attempt releases the retry-only snapshot before it assembles the published plan.
            drop(retry_intrinsic_layouts);
            drop(retry_externals);
            drop(retry_instances);
            statics.extend(instance_cells);
        }
    }
    let retained_constructors = retained_program.constructors.clone();
    let mut completed_models = std::mem::take(&mut inventories.models);
    if context.nominal_declarations.len() != original_nominal_count {
        completed_models = lower_model_declarations(
            &retained_constructors,
            &context,
            &entry_file,
            &mut intrinsics,
            &retained_nominals,
            kind == "library",
            0,
        )?;
    }
    inventories.models = completed_models;
    let zip_carriers = finish_instance_layouts(
        &constructors,
        &retained_constructors,
        Arc::make_mut(&mut context),
        original_nominal_count,
        &mut inventories,
        &mut intrinsics,
        kind,
    )?;
    // ---- Completed publication ----
    let mut exports: Vec<PlanExport> = Vec::new();
    let mut templates: Vec<GenericModelDeclaration> = Vec::new();
    let mut enum_templates: Vec<GenericEnumDeclaration> = Vec::new();
    if kind == "library" {
        templates = generic_models(
            &template_constructors,
            &source_context,
            &template_references,
            &retained_nominals,
            &entry_file,
        )?;
        enum_templates = generic_enums(
            &template_constructors,
            &source_context,
            &context,
            &mut inventories.enums,
            &intrinsics,
            &entry_file,
        )?;
        exports = loaf_exports(
            &context,
            &emitted_frames.body_positions,
            &source_context,
            &templates,
            &enum_templates,
        );
        publish_static_accessors(
            &retained_program,
            linked_static_exports,
            &entry_file,
            &mut inventories,
            &mut exports,
        )?;
        publish_unions(&constructors, &context, &intrinsics, published)?;
    }
    let sources_count = inventories.enums.len() - intrinsics.len();
    let planned = completed_plan(
        &context,
        &entry_owner,
        kind,
        tests,
        &entry_file,
        inventories,
        statics,
        exports,
        templates,
        enum_templates,
        zip_carriers,
        sources_count,
    )?;
    Ok(UnitPlanning::Complete(planned))
}

/// Fill empty planning inventories with generated functions and declaration roots for either entry point.
///
/// 1. Retain checked deserialization and string conversion roots in declaration order.
/// 2. Admit model and enum layouts with the selected unit kind and profile policy.
/// 3. Merge their runtime and instance requests through the ordinary admission rules.
#[allow(clippy::too_many_arguments)]
pub fn initial_plan_roots(
    constructors: Arc<ConstructorIndex>,
    context: &BodyIrModule,
    file: &str,
    intrinsics: &mut Vec<EnumDeclaration>,
    nominal_instances: &[NominalInstance],
    kind: &str,
    unions: &[IncanType],
    overflow_checks: bool,
    trace: bool,
    inventories: &mut Inventories,
) -> Result<ProgramContext, String> {
    trace_lowering(trace, "checked deserialize");
    let (checked_functions, checked_runtime, checked_instances) =
        lower_checked_deserialize(&constructors, context, nominal_instances, file, intrinsics, 0)?;
    trace_lowering(trace, "string conversions");
    let (string_functions, conversion_runtime) = lower_string_conversions(&constructors, context, file, intrinsics, 0)?;
    let (prepared_models, prepared_enums, program) = initial_plan_layouts(
        constructors,
        context,
        file,
        intrinsics,
        nominal_instances,
        kind,
        unions,
        overflow_checks,
        trace,
    )?;
    inventories.models.extend(prepared_models);
    inventories.enums.extend(prepared_enums);
    inventories.functions.extend(checked_functions);
    inventories.functions.extend(string_functions);
    inventories.externals.extend(checked_runtime);
    merge_externals(&mut inventories.externals, conversion_runtime)?;
    merge_instance_requests(&mut inventories.instances, &checked_instances);
    let bridges = comparison_bridge_instances(context, &inventories.models, &inventories.enums);
    merge_instance_requests(&mut inventories.instances, &bridges);
    Ok(program)
}

/// Append roots introduced since the preserved attempt and extend the retained shared lookup generations.
///
/// `previous_nominal_count` separates newly resolved runtime declarations from layouts an earlier generic-instance
/// pass appended. `previous_model_count` also admits those already-closed layouts without rebuilding older models.
#[allow(clippy::too_many_arguments)]
fn resume_plan_roots(
    program: &mut ProgramContext,
    constructors: Arc<ConstructorIndex>,
    context: &BodyIrModule,
    file: &str,
    intrinsics: &mut Vec<EnumDeclaration>,
    nominal_instances: &[NominalInstance],
    kind: &str,
    unions: &[IncanType],
    previous_nominal_count: usize,
    previous_model_count: usize,
    inventories: &mut Inventories,
) -> Result<(), String> {
    let (checked_functions, checked_runtime, checked_instances) = lower_checked_deserialize(
        &constructors,
        context,
        nominal_instances,
        file,
        intrinsics,
        previous_nominal_count,
    )?;
    let (string_functions, conversion_runtime) =
        lower_string_conversions(&constructors, context, file, intrinsics, previous_nominal_count)?;
    let new_models = lower_model_declarations(
        &constructors,
        context,
        file,
        intrinsics,
        nominal_instances,
        kind == "library",
        previous_model_count,
    )?;
    extend_program_context(program, context, constructors, unions)?;
    inventories.functions.extend(checked_functions);
    inventories.functions.extend(string_functions);
    inventories.externals.extend(checked_runtime);
    merge_externals(&mut inventories.externals, conversion_runtime)?;
    merge_instance_requests(&mut inventories.instances, &checked_instances);
    let bridges = comparison_bridge_instances(context, &new_models, &[]);
    merge_instance_requests(&mut inventories.instances, &bridges);
    inventories.models.extend(new_models);
    Ok(())
}

/// Admit model and enum layouts in order, then freeze their matching program lookup context.
#[allow(clippy::too_many_arguments)]
fn initial_plan_layouts(
    constructors: Arc<ConstructorIndex>,
    context: &BodyIrModule,
    file: &str,
    intrinsics: &mut Vec<EnumDeclaration>,
    nominal_instances: &[NominalInstance],
    kind: &str,
    unions: &[IncanType],
    overflow_checks: bool,
    trace: bool,
) -> Result<(Vec<ModelDeclaration>, Vec<EnumDeclaration>, ProgramContext), String> {
    trace_lowering(trace, "model declarations");
    let models = lower_model_declarations(
        &constructors,
        context,
        file,
        intrinsics,
        nominal_instances,
        kind == "library",
        0,
    )?;
    trace_lowering(trace, "enum declarations");
    let mut enums = lower_enum_declarations(&constructors, context, file, intrinsics)?;
    attach_enum_comparisons(&constructors, context, &mut enums, intrinsics)?;
    let program = program_context(constructors, context, intrinsics, unions, overflow_checks)?;
    Ok((models, enums, program))
}

/// Matched constructor, union and intrinsic tables for either planning entry point.
pub fn layout_tables(
    context: &BodyIrModule,
    file: &str,
    source_files: &BTreeMap<String, String>,
    package_versions: &BTreeMap<String, String>,
    retained_intrinsics: Vec<EnumDeclaration>,
) -> Result<(ConstructorIndex, Vec<IncanType>, Vec<EnumDeclaration>), String> {
    let constructors = index_constructors(&context.nominal_declarations, source_files, package_versions);
    let unions = collect_unions(context);
    let intrinsics = intrinsic_declarations(&constructors, context, file, retained_intrinsics)?;
    Ok((constructors, unions, intrinsics))
}

/// Admit the completed unit's test and program entries, then transfer its owned native inventories into the plan.
#[allow(clippy::too_many_arguments)]
fn completed_plan(
    context: &BodyIrModule,
    entry_owner: &str,
    kind: &str,
    tests: &[String],
    file: &str,
    inventories: Inventories,
    statics: Vec<StaticCell>,
    exports: Vec<PlanExport>,
    templates: Vec<GenericModelDeclaration>,
    enum_templates: Vec<GenericEnumDeclaration>,
    zip_carriers: Vec<ZipCarrier>,
    sources: usize,
) -> Result<Option<Plan>, String> {
    let (has_main, declares_main) = main_selection(context, entry_owner);
    let test_entries = collected_test_entries(context, entry_owner, tests, &inventories.functions, &inventories.enums)?;
    let mut program_entries: Vec<String> = Vec::new();
    if kind == "program" && !has_main {
        if declares_main {
            return Err("unsupported Body IR selected main representation".into());
        }
        return Ok(None);
    }
    if kind == "program" {
        program_entries = selected_program_entries(context)?;
    }
    let planned = Plan {
        enums: inventories.enums,
        models: inventories.models,
        functions: inventories.functions,
        externals: inventories.externals,
        span: source_span(file, 1, 1),
        statics,
        exports,
        tests: test_entries,
        zip_carriers,
        programs: program_entries,
        generic_models: templates,
        generic_enums: enum_templates,
    };
    // The inventories were folded when the last carrier was appended; storage and templates are folded here.
    Ok(Some(fold_carrier_positions(planned, sources as i64)?))
}

/// Retain closed storage, restore the source inventory, then finalize with its paired index.
fn finish_instance_layouts(
    original_constructors: &ConstructorIndex,
    retained_constructors: &ConstructorIndex,
    context: &mut BodyIrModule,
    original_nominal_count: usize,
    inventories: &mut Inventories,
    intrinsics: &mut Vec<EnumDeclaration>,
    kind: &str,
) -> Result<Vec<ZipCarrier>, String> {
    let zip_carriers = runtime_zip_carriers(retained_constructors, context, intrinsics)?;
    // Closed frames keep their native layouts; source exports retain the original declaration inventory.
    context.nominal_declarations.truncate(original_nominal_count);
    finalize_plan_layouts(
        original_constructors,
        context,
        inventories,
        intrinsics,
        kind == "library",
    )?;
    Ok(zip_carriers)
}

/// Publish completed layouts and validate callable and coroutine storage after every instance has contributed.
fn finalize_plan_layouts(
    constructors: &ConstructorIndex,
    context: &BodyIrModule,
    inventories: &mut Inventories,
    intrinsics: &mut Vec<EnumDeclaration>,
    publish: bool,
) -> Result<(), String> {
    // Instance lowering can append carriers needed by a callable or coroutine capture.
    let sources = inventories.enums.len();
    inventories.enums.extend(intrinsics.iter().cloned());
    // No carrier is appended after this point, so each carrier position takes its place in the plan's enum table.
    fold_inventories(
        &mut inventories.functions,
        &mut inventories.models,
        &mut inventories.enums,
        &mut inventories.externals,
        sources as i64,
    )?;
    if publish {
        publish_local_layouts(&mut inventories.models, &mut inventories.enums);
    }
    validate_futures(constructors, context, intrinsics)?;
    Ok(())
}

/// The immutable source context with its matching constructor index, module provenance and specialized layouts.
pub struct PlanContexts {
    pub source_context: BodyIrModule,
    pub template_constructors: ConstructorIndex,
    pub owners: Vec<String>,
    pub context: BodyIrModule,
    pub nominal_instances: Vec<NominalInstance>,
    pub template_references: Vec<String>,
}

/// Validate against retained authority and prepare only the current slice's compact specialized contexts.
#[allow(clippy::too_many_arguments)]
pub fn plan_contexts(
    modules: &[BodyIrModule],
    sources: &[String],
    files: &[String],
    entry: i64,
    trace: bool,
    source_files: &BTreeMap<String, String>,
    package_versions: &BTreeMap<String, String>,
    validation_context: &BodyIrModule,
    retained_templates: &ConstructorIndex,
) -> Result<PlanContexts, String> {
    let footprint_trace = traced("INCAN_TRACE_LOWERING_FOOTPRINT");
    trace_lowering(trace, "validate modules");
    validate_plan_inputs(modules, sources, files, entry, validation_context)?;
    trace_lowering_footprint(footprint_trace, "validated modules");
    trace_lowering(trace, "project context");
    let (mut context, owners) = project_context(modules, entry as usize)?;
    let (source_context, template_constructors, nominal_instances, template_references) = specialize_plan_context(
        &mut context,
        source_files,
        package_versions,
        retained_templates,
        footprint_trace,
        trace,
    )?;
    Ok(PlanContexts {
        source_context,
        template_constructors,
        owners,
        context,
        nominal_instances,
        template_references,
    })
}

/// Validate a resumed runtime slice and project its body-weighted module ranges concurrently.
#[allow(clippy::too_many_arguments)]
fn plan_contexts_parallel(
    modules: Arc<Vec<BodyIrModule>>,
    sources: &[String],
    files: &[String],
    entry: i64,
    trace: bool,
    source_files: &BTreeMap<String, String>,
    package_versions: &BTreeMap<String, String>,
    validation_context: &BodyIrModule,
    retained_templates: &ConstructorIndex,
) -> Result<PlanContexts, String> {
    let footprint_trace = traced("INCAN_TRACE_LOWERING_FOOTPRINT");
    trace_lowering(trace, "validate resumed modules");
    validate_plan_inputs(&modules, sources, files, entry, validation_context)?;
    trace_lowering_footprint(footprint_trace, "validated resumed modules");
    trace_lowering(trace, "parallel resumed project context");
    let (mut context, owners) = project_context_parallel(modules, entry as usize)?;
    let (source_context, template_constructors, nominal_instances, template_references) = specialize_plan_context(
        &mut context,
        source_files,
        package_versions,
        retained_templates,
        footprint_trace,
        trace,
    )?;
    Ok(PlanContexts {
        source_context,
        template_constructors,
        owners,
        context,
        nominal_instances,
        template_references,
    })
}

/// Specialize one serially or concurrently projected module context under the same deterministic layout rules.
fn specialize_plan_context(
    context: &mut BodyIrModule,
    source_files: &BTreeMap<String, String>,
    package_versions: &BTreeMap<String, String>,
    retained_templates: &ConstructorIndex,
    footprint_trace: bool,
    trace: bool,
) -> Result<(BodyIrModule, ConstructorIndex, Vec<NominalInstance>, Vec<String>), String> {
    trace_lowering_footprint(footprint_trace, "joined project");
    // Later generic-instance and publication work reads only source nominal and enum declarations. Retain that
    // declaration authority without keeping a second copy of executable bodies or the other joined module tables.
    let mut source_context = BodyIrModule::declarations_only(context.module_id.path().to_owned());
    source_context.nominal_declarations = context.nominal_declarations.clone();
    source_context.enum_declarations = context.enum_declarations.clone();
    let constructors = with_generic_constructor_arities(
        index_constructors(&source_context.nominal_declarations, source_files, package_versions),
        retained_templates,
    );
    let template_references = referenced_templates(context);
    trace_lowering_footprint(footprint_trace, "retained source templates");
    // A closed enum payload can name a nominal instance no frame uses, and a closed nominal field or method frame can
    // name an enum instance no source frame uses, so enum layouts are discovered before nominal specialization and
    // installed again from the same joined context afterwards.
    trace_lowering(trace, "discover enum payload dependencies");
    let enum_layouts = specialized_enum_layouts(context)?;
    trace_lowering(trace, "specialize nominals");
    let nominal_instances = specialize_nominals_with_instances(&constructors, context, &enum_layouts)?;
    trace_lowering_footprint(footprint_trace, "specialized nominals");
    trace_lowering(trace, "specialize enums");
    specialize_enums(context)?;
    trace_lowering_footprint(footprint_trace, "specialized enums");
    Ok((source_context, constructors, nominal_instances, template_references))
}

/// Bind every checked module identity to its aligned source input without inferring files from reference sites.
pub fn source_file_index(modules: &[BodyIrModule], files: &[String]) -> Result<BTreeMap<String, String>, String> {
    if modules.len() != files.len() {
        return Err("invalid Body IR project source provenance".into());
    }
    let mut indexed: BTreeMap<String, String> = BTreeMap::new();
    for (module, file) in modules.iter().zip(files) {
        let path = module.module_id.path().to_owned();
        if indexed.contains_key(&path) {
            return Err(format!("duplicate Body IR source provenance for module {path}"));
        }
        indexed.insert(path, file.clone());
    }
    Ok(indexed)
}

fn trace_lowering(enabled: bool, stage: &str) {
    if enabled {
        emit("lowering", "INCAN_TRACE_LOWERING", &format!("lowering: {stage}"));
    }
}

/// Footprint tracing reads the platform probes; this lowering reports the boundary only.
fn trace_lowering_footprint(enabled: bool, boundary: &str) {
    if enabled {
        emit(
            "lowering_footprint",
            "INCAN_TRACE_LOWERING_FOOTPRINT",
            &format!("lowering footprint: {boundary}: unavailable: not measured by the Rust lowering"),
        );
    }
}

/// Validate source provenance and checked authority, including retained continuation facts, before indexing.
pub fn validate_plan_inputs(
    modules: &[BodyIrModule],
    sources: &[String],
    files: &[String],
    entry: i64,
    retained: &BodyIrModule,
) -> Result<(), String> {
    let entry = checked_entry(modules, entry)?;
    if sources.len() != modules.len() || files.len() != modules.len() {
        return Err("invalid Body IR project source provenance".into());
    }
    validate_program_entry_selection(modules, entry, retained)?;
    validate_conversion_placement(modules, entry)?;
    for module in modules {
        validate_project_module(module, modules, retained)?;
    }
    Ok(())
}

/// Separate the checked program entry from an ordinary declaration merely spelled main.
fn main_selection(context: &BodyIrModule, entry_owner: &str) -> (bool, bool) {
    let mut selected = false;
    let mut declared = false;
    for body in &context.bodies {
        match body_module_identity(body) {
            Ok(owner) if owner == entry_owner => {}
            _ => continue,
        }
        if body.name == "main" {
            declared = true;
        }
        if is_program_entry(context, body) {
            selected = true;
        }
    }
    (selected, declared)
}
