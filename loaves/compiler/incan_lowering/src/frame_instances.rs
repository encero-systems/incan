//! Closed generic instances: their frames, native layouts and the plan inventories they extend.

use std::collections::BTreeMap;

use incan_mir_plan_native::{
    Carrier, EnumDeclaration, ExternalFunction, Function, GenericLoan, PlanType, StaticCell, carrier_position,
};
use incan_semantics_core::IncanType;
use incan_semantics_core::body_ir::{Body, BodyIrModule, DecoratedInitialization};

use crate::async_::evaluator_name;
use crate::checked_values::location;
use crate::classes::body_module_identity;
use crate::decls::{library_enum_item, library_item};
use crate::decorator_caches::{CachedBinding, cached_binding_frames, frame_signature};
use crate::decorator_caches::{DecoratorFrame, validate_binding_policy};
use crate::function_frames::lower_function;
use crate::generators::append_generator_constructor;
use crate::instance_products::InstanceProducts;
use crate::layout_types::native_source_type;
use crate::lowering::Inventories;
use crate::native_type_identity::ConstructorIndex;
use crate::nominal_instances::NominalInstance;
use crate::nominal_instances::retain_frame_nominals;
use crate::options::retain_body_carriers;
use crate::program_context::ProgramContext;
use crate::program_context::retain_nominal_tables;
use crate::runtime_calls::{
    GovernedFrames, RuntimeInventories, redirect_runtime_calls, retain_runtime_instances, runtime_redirects,
};
use crate::runtime_requirements::RuntimeRetention;
use crate::signature_lifetimes::{generic_loans, signature_lifetimes};
use crate::type_parameters::merge_instance_requests;
use crate::type_parameters::{Instance, callable_storage_arguments, enqueue_instances, instance_queue, instantiate};
use crate::unions::collect_unions;

/// Drain checked instances through an explicit mutable loan and append native layouts in place.
#[allow(clippy::too_many_arguments)]
pub fn lower_instances(
    template_constructors: &ConstructorIndex,
    context: &mut BodyIrModule,
    templates: &BodyIrModule,
    program: &mut ProgramContext,
    instances: &mut Vec<Instance>,
    unions: &[IncanType],
    sources: &[String],
    files: &[String],
    owners: &[String],
    inventories: &mut Inventories,
    intrinsics: &mut Vec<EnumDeclaration>,
    nominal_instances: &mut Vec<NominalInstance>,
    cells: &mut Vec<StaticCell>,
    products: Option<&InstanceProducts>,
) -> Result<RuntimeRetention, String> {
    let mut queue = instance_queue();
    let mut runtime_unions = unions.to_vec();
    let mut redirects = runtime_redirects();
    let mut names = function_names();
    let governed_runtime = !context.runtime_callable_bindings.is_empty()
        || !context.runtime_binding_scopes.is_empty()
        || governed_runtime_module(owners);
    if governed_runtime {
        let frames = GovernedFrames {
            template_constructors,
            templates,
            program,
            nominal_instances,
            instances,
        };
        let runtime = RuntimeInventories {
            functions: &mut inventories.functions,
            externals: &mut inventories.externals,
            intrinsics,
            unions: &mut runtime_unions,
        };
        retain_runtime_instances(context, frames, runtime, &mut redirects)?;
    }
    let mut source_indexes: BTreeMap<String, usize> = BTreeMap::new();
    for (index, owner) in owners.iter().enumerate() {
        source_indexes.insert(owner.clone(), index);
    }
    enqueue_instances(&mut queue, instances);
    while let Some(request) = queue.pending.pop_front() {
        // Keyed instance products, reused or retained under a records request, are not yet produced here; a records
        // request lowers each closed instance afresh, as an empty product directory would, and the replay counts the
        // missing product files separately from the plans.
        let _ = products;
        let body = instantiate(&request.body, &request.arguments, &request.owned_callable_slots)?;
        let storage_arguments =
            callable_storage_arguments(&request.body, &request.arguments, &request.owned_callable_slots)?;
        let lifetimes = signature_lifetimes(&request.body, &request.arguments)?;
        let declarations = retain_frame_nominals(template_constructors, context, templates, &body, nominal_instances)?;
        let added = !declarations.is_empty();
        context.nominal_declarations.extend(declarations);
        if added {
            runtime_unions = collect_unions(context);
        }
        let (role, source_index) = instance_frame_selection(&body, request.original_callable, &source_indexes)?;
        retain_nominal_tables(program, context)?;
        let constructors = program.constructors.clone();
        retain_body_carriers(&constructors, context, intrinsics, &body, &files[source_index])?;
        let decorated = DecoratedInstance {
            body: &body,
            arguments: &request.arguments,
            owned_callable_slots: &request.owned_callable_slots,
            source: &sources[source_index],
            file: &files[source_index],
        };
        let queues = (&mut names, &mut *instances, &mut *cells);
        retain_generic_decorator_frames(context, program, &decorated, unions, intrinsics, inventories, queues)?;
        enqueue_instances(&mut queue, instances);
        let frame = InstanceFrame {
            request: &request,
            body: &body,
            role,
            storage_arguments: &storage_arguments,
            lifetimes,
            source: &sources[source_index],
            file: &files[source_index],
        };
        let mut scheduled = lower_instance_frame(
            context,
            program,
            &frame,
            unions,
            &runtime_unions,
            intrinsics,
            inventories,
            &mut names,
        )?;
        enqueue_instances(&mut queue, &mut scheduled);
        if governed_runtime {
            let frames = GovernedFrames {
                template_constructors,
                templates,
                program,
                nominal_instances,
                instances,
            };
            let runtime = RuntimeInventories {
                functions: &mut inventories.functions,
                externals: &mut inventories.externals,
                intrinsics,
                unions: &mut runtime_unions,
            };
            retain_runtime_instances(context, frames, runtime, &mut redirects)?;
            enqueue_instances(&mut queue, instances);
        }
    }
    // Runtime declarations are preflighted before plan construction. Retain this check as a fail-closed assertion
    // against a future lowering operation bypassing that authenticated facade inventory.
    if !redirects.missing.is_empty() {
        return Ok(RuntimeRetention::NeedsRuntime(redirects.missing));
    }
    redirect_runtime_calls(&mut inventories.functions, &redirects)?;
    Ok(RuntimeRetention::Complete)
}

/// One closed instance and the source frame it lowers in.
struct InstanceFrame<'a> {
    request: &'a Instance,
    body: &'a Body,
    role: DecoratorFrame,
    storage_arguments: &'a [IncanType],
    lifetimes: Vec<Vec<String>>,
    source: &'a str,
    file: &'a str,
}

/// Lower one closed instance frame, retain it with the functions it synthesizes, and return its own requests.
#[allow(clippy::too_many_arguments)]
fn lower_instance_frame(
    context: &BodyIrModule,
    program: &ProgramContext,
    frame: &InstanceFrame<'_>,
    unions: &[IncanType],
    runtime_unions: &[IncanType],
    intrinsics: &mut Vec<EnumDeclaration>,
    inventories: &mut Inventories,
    names: &mut FunctionNames,
) -> Result<Vec<Instance>, String> {
    let diagnostic_context = format!("{} at {}", frame.body.name, frame.file);
    let (function, runtime, pending, synthesized) = lower_function(
        context,
        program,
        frame.body,
        unions,
        frame.source,
        frame.file,
        intrinsics,
        frame.storage_arguments,
        frame.role,
    )
    .map_err(|reason| format!("{reason} in {diagnostic_context}"))?;
    let mut retained_function = function;
    retained_function.signature_lifetimes = frame.lifetimes.clone();
    retained_function.generic_loans = instance_loans(program, context, frame.request, runtime_unions, intrinsics)?;
    retain_instance(&mut inventories.functions, names, retained_function)?;
    append_synthesized(&mut inventories.functions, names, synthesized)?;
    append_generator_constructor(
        &program.constructors,
        &mut inventories.functions,
        context,
        frame.body,
        frame.source,
        frame.file,
        intrinsics,
    )?;
    merge_externals(&mut inventories.externals, runtime)?;
    Ok(pending)
}

/// Retain the same canonical argument storage used by native instantiation for signature-loan verification.
fn instance_loans(
    program: &ProgramContext,
    context: &BodyIrModule,
    request: &Instance,
    unions: &[IncanType],
    intrinsics: &[EnumDeclaration],
) -> Result<Vec<GenericLoan>, String> {
    let mut retained = Vec::with_capacity(request.arguments.len());
    for argument in &request.arguments {
        retained.push(native_source_type(
            &program.constructors,
            &context.nominal_declarations,
            &context.enum_declarations,
            unions,
            argument,
            intrinsics,
            "",
        )?);
    }
    Ok(generic_loans(&request.body, &retained))
}

/// Keep the checked execution role and last matching source owner, refusing an instance without provenance.
fn instance_frame_selection(
    body: &Body,
    original_callable: bool,
    source_indexes: &BTreeMap<String, usize>,
) -> Result<(DecoratorFrame, usize), String> {
    let role = if original_callable {
        DecoratorFrame::Original
    } else {
        DecoratorFrame::Normal
    };
    let owner = body_module_identity(body)?;
    match source_indexes.get(&owner) {
        Some(index) => Ok((role, *index)),
        None => Err("unsupported Body IR instance without source provenance".into()),
    }
}

/// One closed instance a generic decorated wrapper may own, with the source frame it lowers in.
struct DecoratedInstance<'a> {
    body: &'a Body,
    arguments: &'a [IncanType],
    owned_callable_slots: &'a [i64],
    source: &'a str,
    file: &'a str,
}

/// Close a retained original and its sole initializer with the wrapper's authenticated ordered arguments.
///
/// The closed instance's storage cell takes the next slot after the unit's source statics and earlier instance cells.
#[allow(clippy::too_many_arguments)]
fn retain_generic_decorator_frames(
    module: &BodyIrModule,
    program: &ProgramContext,
    instance: &DecoratedInstance<'_>,
    unions: &[IncanType],
    intrinsics: &mut Vec<EnumDeclaration>,
    inventories: &mut Inventories,
    (names, instances, cells): (&mut FunctionNames, &mut Vec<Instance>, &mut Vec<StaticCell>),
) -> Result<(), String> {
    let body = instance.body;
    for binding in &module.decorated_bindings {
        if binding.original_body.canonical != body.canonical {
            continue;
        }
        validate_binding_policy(binding)?;
        if binding.initialization != DecoratedInitialization::PerInstantiation {
            continue;
        }
        let Some(template) = &binding.initializer else {
            continue;
        };
        if !instance.owned_callable_slots.is_empty() {
            return Err("unsupported Body IR generic decorated wrapper with owned closure parameters".into());
        }
        let arguments = instance.arguments;
        let original = instantiate(&binding.original_body, arguments, &[])?;
        let (function, runtime, pending, synthesized) = lower_function(
            module,
            program,
            &original,
            unions,
            instance.source,
            instance.file,
            intrinsics,
            arguments,
            DecoratorFrame::Original,
        )?;
        let signature = frame_signature(&function);
        let original_name = function.name.clone();
        retain_instance(&mut inventories.functions, names, function)?;
        append_synthesized(&mut inventories.functions, names, synthesized)?;
        merge_externals(&mut inventories.externals, runtime)?;
        merge_instance_requests(instances, &pending);
        let initializer = instantiate(template, arguments, &[])?;
        let (init_function, init_runtime, init_pending, init_synthesized) = lower_function(
            module,
            program,
            &initializer,
            unions,
            instance.source,
            instance.file,
            intrinsics,
            arguments,
            DecoratorFrame::Initializer,
        )?;
        // Decorators generic over the whole callable return the original's exact closed signature.
        if init_function.return_type != PlanType::FunctionPointer(signature.clone()) {
            return Err("unsupported Body IR generic decorator initializer outside its original's signature".into());
        }
        let initializer_name = init_function.name.clone();
        retain_instance(&mut inventories.functions, names, init_function)?;
        append_synthesized(&mut inventories.functions, names, init_synthesized)?;
        merge_externals(&mut inventories.externals, init_runtime)?;
        merge_instance_requests(instances, &init_pending);
        let cached = CachedBinding {
            base: evaluator_name(module, body),
            original: original_name,
            initializer: initializer_name,
            signature,
            slot: (program.statics.len() + cells.len()) as i64,
            unions,
            intrinsics,
        };
        let span = location(&body.span, instance.source, instance.file);
        let (cell, frames, cache_runtime) = cached_binding_frames(program, binding, &cached, &span)?;
        cells.push(cell);
        for frame in frames {
            retain_instance(&mut inventories.functions, names, frame)?;
        }
        merge_externals(&mut inventories.externals, cache_runtime)?;
    }
    Ok(())
}

/// Refuse a native instance symbol that collides with another retained source declaration.
fn retain_instance(functions: &mut Vec<Function>, names: &mut FunctionNames, function: Function) -> Result<(), String> {
    if !named_functions(names, functions, &function.name).is_empty() {
        return Err(format!(
            "unsupported Body IR type parameter instance symbol collision {}",
            function.name
        ));
    }
    functions.push(function);
    Ok(())
}

/// Whether the unit lowers the runtime itself, which governs its own instances.
fn governed_runtime_module(owners: &[String]) -> bool {
    owners
        .iter()
        .any(|owner| owner.starts_with("pub::incan_native_runtime::"))
}

/// Positions of one plan function list by symbol; each lookup first indexes the rows appended since the last.
#[derive(Default)]
pub struct FunctionNames {
    pub positions: BTreeMap<String, Vec<usize>>,
    pub indexed: usize,
}

pub fn function_names() -> FunctionNames {
    FunctionNames::default()
}

/// Index the rows appended since the last lookup, then return this symbol's positions in list order.
pub fn named_functions(names: &mut FunctionNames, functions: &[Function], name: &str) -> Vec<usize> {
    while names.indexed < functions.len() {
        names
            .positions
            .entry(functions[names.indexed].name.clone())
            .or_default()
            .push(names.indexed);
        names.indexed += 1;
    }
    names.positions.get(name).cloned().unwrap_or_default()
}

/// Add closure bodies and constructors once; two functions sharing one item's forwarding environment agree on it.
pub fn append_synthesized(
    functions: &mut Vec<Function>,
    names: &mut FunctionNames,
    additions: Vec<Function>,
) -> Result<(), String> {
    for addition in additions {
        let mut present = false;
        for position in named_functions(names, functions, &addition.name) {
            if functions[position] != addition {
                return Err(format!(
                    "invalid Body IR synthesized function collision {}",
                    addition.name
                ));
            }
            present = true;
        }
        if !present {
            functions.push(addition);
        }
    }
    Ok(())
}

/// Retain one exact signature per native path and instantiation; report both checked signatures on conflict.
pub fn merge_externals(externals: &mut Vec<ExternalFunction>, additions: Vec<ExternalFunction>) -> Result<(), String> {
    for external in additions {
        let mut found = false;
        for retained in externals.iter() {
            if retained.path == external.path && retained.type_arguments == external.type_arguments {
                if retained.parameters != external.parameters || retained.return_type != external.return_type {
                    return Err(conflicting_external_signatures(retained, &external));
                }
                found = true;
            }
        }
        if !found {
            externals.push(external);
        }
    }
    Ok(())
}

/// Positions of each native path's signatures in an externals list that only grows while the index is in use.
#[derive(Default)]
pub struct ExternalIndex {
    pub positions: BTreeMap<String, Vec<usize>>,
    pub indexed: usize,
}

pub fn external_index() -> ExternalIndex {
    ExternalIndex::default()
}

/// Merge as `merge_externals` does, finding a path's retained signatures by key instead of scanning every entry.
pub fn merge_indexed(
    externals: &mut Vec<ExternalFunction>,
    index: &mut ExternalIndex,
    additions: Vec<ExternalFunction>,
) -> Result<(), String> {
    for external in additions {
        while index.indexed < externals.len() {
            index
                .positions
                .entry(externals[index.indexed].path.clone())
                .or_default()
                .push(index.indexed);
            index.indexed += 1;
        }
        let mut found = false;
        if let Some(positions) = index.positions.get(&external.path) {
            for position in positions {
                let retained = &externals[*position];
                if retained.type_arguments == external.type_arguments {
                    if retained.parameters != external.parameters || retained.return_type != external.return_type {
                        return Err(conflicting_external_signatures(retained, &external));
                    }
                    found = true;
                }
            }
        }
        if !found {
            externals.push(external);
        }
    }
    Ok(())
}

/// Keep each conflicting checked signature and source location in the native merge refusal.
fn conflicting_external_signatures(retained: &ExternalFunction, external: &ExternalFunction) -> String {
    format!(
        "unsupported Body IR conflicting native call signatures for {} with {:?}: ({:?}) -> {:?} at {}:{}:{}; ({:?}) -> {:?} at {}:{}:{}",
        external.path,
        external.type_arguments,
        retained.parameters,
        retained.return_type,
        retained.span.file,
        retained.span.line,
        retained.span.column,
        external.parameters,
        external.return_type,
        external.span.file,
        external.span.line,
        external.span.column
    )
}

/// Report whether a native main's result is a `Result` with a unit success, which Rust's entry point reports itself.
///
/// The error side must be text or a type an SDK crate owns: legacy's entry point reports an error through its Debug,
/// which both have.
pub fn entry_result(module: &BodyIrModule, ty: &PlanType, intrinsics: &[EnumDeclaration]) -> bool {
    let PlanType::Enum(owner, _) = ty else {
        return false;
    };
    let index = carrier_position(*owner);
    if index < 0 || index as usize >= intrinsics.len() {
        return false;
    }
    let declaration = &intrinsics[index as usize];
    if declaration.carrier != Carrier::Result || declaration.variants.len() != 2 {
        return false;
    }
    let success = &declaration.variants[0].fields;
    let failure = &declaration.variants[1].fields;
    if failure.len() != 1 || (!success.is_empty() && success.as_slice() != [PlanType::Unit]) {
        return false;
    }
    entry_error(module, &failure[0])
}

/// Report whether an entry result's error is text or an SDK crate's model or enum.
fn entry_error(module: &BodyIrModule, ty: &PlanType) -> bool {
    match ty {
        PlanType::String => true,
        PlanType::Model(index, _) => module
            .nominal_declarations
            .get(*index as usize)
            .is_some_and(|declaration| !library_item(declaration).is_empty()),
        PlanType::Enum(index, _) => module
            .enum_declarations
            .get(*index as usize)
            .is_some_and(|declaration| !library_enum_item(declaration).is_empty()),
        _ => false,
    }
}
