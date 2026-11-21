//! Lower one function: its body builder, frame parameters, locals, scopes, and entrypoint effects.

use incan_mir_plan_native::{
    Constant, EnumDeclaration, ExternalFunction, Function, Local, Parameter, PlanType, RvalueKind, SourceSpan,
    TerminatorKind, assign as plan_assign, place as plan_place,
};
use incan_semantics_core::body_ir::{
    Body, BodyIrModule, CallableParam, CallableParamDefault, EntrypointEffect,
    EnumDeclaration as SourceEnumDeclaration, LocalDecl, NominalDeclaration,
};
use incan_semantics_core::{CanonicalSymbolId, IncanInputAccess, IncanType};

use crate::async_::{evaluator_name, future_type, integer_entry, is_future};
use crate::attributes::native_attributes;
use crate::builtin_calls::validate_exact_float;
use crate::call_arguments::{lacks_exclusive_parameter, owned_collection, stored_parameter_type};
use crate::checked_values::{builder_location, indexed_location, line_starts, location};
use crate::classes::{body_module_identity, body_type_bindings, frame_type, function_name, is_program_entry};
use crate::closures::{
    function_pointer_storage, local_representation, parameter_representation, return_representation,
    validate_callable_representation,
};
use crate::computed_statics::{enters_module_initialization, initialize_module_statics};
use crate::decorator_caches::{DecoratorFrame, callable_name_profile, frame_name, validate_binding_policy};
use crate::defaults::default_type;
use crate::explicit_arguments::direct_view_access;
use crate::frame::{Builder, drop_all_owned, external_call, literal, read_place, temporary};
use crate::frame_instances::entry_result;
use crate::generators::{generator_shape, producer_frame, producer_name};
use crate::layout_types::native_source_type;
use crate::list_iteration::frozen_list_snapshot_type;
use crate::lists::{list_iterator_locals, list_parameter_type, taking_iterator_locals};
use crate::native_type_identity::ConstructorIndex;
use crate::pair_generators::{pair_generator_locals, pair_generator_type};
use crate::program_context::ProgramContext;
use crate::redo_counts::record_lowering;
use crate::result_requirements::validate_result_requirements;
use crate::signature_lifetimes::signature_lifetimes;
use crate::statements::{is_nominal_type, is_owned_callable, lower_block};
use crate::text_plan::owned_literal;
use crate::traits::append_default_type_bindings;
use crate::tuple_iterators::zip_shape;
use crate::type_parameters::{Instance, substitute};
use crate::unions::retain_body_unions;

/// A lowered frame with the runtime externals it calls, the instances it requests and the functions it lifted.
pub type LoweredFrame = (Function, Vec<ExternalFunction>, Vec<Instance>, Vec<Function>);

/// The checked frame storage a body's builder starts from.
struct FrameStart<'a> {
    parameters: &'a [Parameter],
    return_type: PlanType,
    selected_name: String,
    cached_decorator: i64,
    initializing_decorator: Option<CanonicalSymbolId>,
    span: SourceSpan,
}

/// Build a concrete frame and retain its instance requests and lifted functions.
///
/// 1. Validate its decorator role and declared ABI.
/// 2. Initialize storage and lower the checked executable body.
/// 3. Retain producer and program entry wrappers with the completed frame.
#[allow(clippy::too_many_arguments)]
pub fn lower_function(
    module: &BodyIrModule,
    program: &ProgramContext,
    body: &Body,
    unions: &[IncanType],
    source: &str,
    file: &str,
    intrinsics: &mut Vec<EnumDeclaration>,
    type_arguments: &[IncanType],
    role: DecoratorFrame,
) -> Result<LoweredFrame, String> {
    // 1. Validate the source role and declared ABI before creating executable storage.
    let constructors: &ConstructorIndex = &program.constructors;
    let nominals = &module.nominal_declarations;
    let enums = &module.enum_declarations;
    let span = location(&body.span, source, file);
    let selected_name = frame_name(evaluator_name(module, body), role);
    record_lowering(program.observe_work, body, &selected_name, type_arguments, file);
    let (cached_decorator, initializing_decorator) = frame_decorator_role(module, body, role)?;
    validate_callable_representation(body)?;
    validate_result_requirements(body, nominals, enums)?;
    let declared_carrier = match native_source_type(
        constructors,
        nominals,
        enums,
        unions,
        &body.return_type,
        intrinsics,
        file,
    ) {
        Ok(carrier) => carrier,
        Err(reason) => {
            let mut frame = body.name.clone();
            if let Some(parameter) = body.params.first() {
                frame += &format!(" on {}", body.locals[parameter.local.index()].ty);
            }
            return Err(format!("{reason} in {frame} return ABI"));
        }
    };
    let declared_type = return_representation(body, declared_carrier)?;
    let return_type = producer_return_type(constructors, module, body, declared_type.clone(), intrinsics)?;
    if is_program_entry(module, body) && return_type != PlanType::Unit {
        if (body.is_async || return_type != PlanType::Int) && !entry_result(module, &return_type, intrinsics) {
            return Err("unsupported Body IR non-unit native main".into());
        }
        if !body.params.is_empty() {
            return Err("unsupported Body IR non-unit native main parameters".into());
        }
    }
    if body.is_async && is_program_entry(module, body) && !body.params.is_empty() {
        return Err("unsupported Body IR async entry signature".into());
    }
    let parameters = frame_parameters(constructors, module, body, source, file, intrinsics)?;
    // 2. Initialize storage and lower the checked executable body.
    let start = FrameStart {
        parameters: &parameters,
        return_type: return_type.clone(),
        selected_name,
        cached_decorator,
        initializing_decorator,
        span: span.clone(),
    };
    let mut builder = body_builder(
        module,
        program,
        body,
        unions,
        source,
        file,
        intrinsics,
        type_arguments,
        start,
    )?;
    if let Some(access) = direct_view_access(body) {
        if body.is_generator() {
            return Err("unsupported Body IR returned view in a generator frame".into());
        }
        // A carried view's carrier is an ordinary owned result whose carrying payload already holds the borrow.
        builder.returned_view = access;
    }
    if let Some(view) = &body.returned_view {
        builder.carries_view =
            view.access != IncanInputAccess::Owned && (view.carried.is_some() || !view.component.is_empty());
    }
    initialize_body_frame(module, body, &mut builder, &parameters, &declared_type, &span)?;
    lower_block(module, &mut builder, &body.block)?;
    if !builder.closed[builder.current] {
        if return_type != PlanType::Unit {
            return Err("Body IR non-unit body has no retained Return".into());
        }
        builder.emit(plan_assign(
            plan_place(0, &span),
            RvalueKind::Use(literal(Constant::Unit, &span)),
            &span,
        ));
        drop_all_owned(&mut builder, &span);
        builder.finish(TerminatorKind::Return, &span);
    }
    // 3. Retain the completed frame and any producer or program entry wrappers.
    let mut function = Function::new(
        builder.name.clone(),
        parameters,
        return_type.clone(),
        std::mem::take(&mut builder.locals),
        std::mem::take(&mut builder.blocks),
        span,
    );
    function.is_async = body.is_async;
    function.attributes = native_attributes(body);
    function.retained_result_source = retained_result_source(body);
    // A returned view's result shares its source parameter's loan; instances recompute the same relation.
    if let Some(view) = &body.returned_view
        && view.access != IncanInputAccess::Owned
    {
        function.signature_lifetimes = signature_lifetimes(body, type_arguments)?;
    }
    if body.is_generator() {
        let Some((leaf, depth)) = generator_shape(&declared_type) else {
            return Err("unsupported Body IR generator return type".into());
        };
        producer_frame(&mut function, builder.generator_handle, &leaf, depth);
        function.name = producer_name(&function.name);
    }
    if is_program_entry(module, body) && return_type == PlanType::Int {
        let entry = integer_entry(&function, function_name(module, body));
        builder.synthesized.push(entry);
    }
    Ok((function, builder.externals, builder.instances, builder.synthesized))
}

/// Retain the checked parameter slot for an owned or aggregate result whose native storage carries its loan.
fn retained_result_source(body: &Body) -> i64 {
    match &body.returned_view {
        Some(source) if source.access == IncanInputAccess::Owned || !source.component.is_empty() => source.slot as i64,
        _ => -1,
    }
}

/// Validate matching decorator bindings and select the cached wrapper or initializer for this source role.
fn frame_decorator_role(
    module: &BodyIrModule,
    body: &Body,
    role: DecoratorFrame,
) -> Result<(i64, Option<CanonicalSymbolId>), String> {
    let mut cached_decorator = -1;
    let mut initializing_decorator = None;
    for (slot, binding) in module.decorated_bindings.iter().enumerate() {
        if binding.original_body.canonical != body.canonical {
            continue;
        }
        validate_binding_policy(binding)?;
        if binding.initializer.is_some() {
            if role == DecoratorFrame::Normal {
                cached_decorator = slot as i64;
            } else if role == DecoratorFrame::Initializer {
                initializing_decorator = body.canonical.clone();
            }
        }
    }
    Ok((cached_decorator, initializing_decorator))
}

/// Construct frame storage and its checked lookup context before any executable block is emitted.
#[allow(clippy::too_many_arguments)]
fn body_builder(
    module: &BodyIrModule,
    program: &ProgramContext,
    body: &Body,
    unions: &[IncanType],
    source: &str,
    file: &str,
    intrinsics: &[EnumDeclaration],
    type_arguments: &[IncanType],
    start: FrameStart<'_>,
) -> Result<Builder, String> {
    let (mut names, mut bindings) = body_type_bindings(module, body, type_arguments)?;
    if let Some(first) = start.parameters.first() {
        append_default_type_bindings(module, body, &first.ty, &mut names, &mut bindings)?;
    }
    let mut selected_unions = unions.to_vec();
    retain_body_unions(&mut selected_unions, &program.union_types, body);
    let (scoped_locals, scope_parents) = frame_scopes(body);
    let locals = frame_locals(
        &program.constructors,
        module,
        body,
        &start.return_type,
        &start.span,
        source,
        file,
        intrinsics,
    )?;
    let module_identity = body_module_identity(body)?;
    let reference_values = reference_value_storage(body, &names, &bindings)?;
    let callable_name_dynamic_profile = callable_name_profile(module, &module_identity)?;
    Ok(Builder {
        program: program.clone(),
        locals,
        parameter_count: start.parameters.len() as i64,
        blocks: Vec::new(),
        current: 0,
        source: source.to_string(),
        file: file.to_string(),
        line_starts: line_starts(source),
        module_identity,
        callable_identity: body.canonical.clone(),
        loops: Vec::new(),
        loop_owners: Vec::new(),
        externals: Vec::new(),
        owned: Vec::new(),
        cleanup_chains: Vec::new(),
        closed: Vec::new(),
        iterators: Vec::new(),
        borrowed_items: Vec::new(),
        in_place_items: Vec::new(),
        partial_callables: Vec::new(),
        scalar_closures: Vec::new(),
        unproven_closures: Vec::new(),
        unions: selected_unions,
        local_offset: 0,
        consuming_closure_frame: false,
        intrinsics: intrinsics.to_vec(),
        generator_handle: -1,
        instances: Vec::new(),
        type_parameters: names,
        type_arguments: bindings,
        callable_type_arguments: type_arguments.to_vec(),
        trait_environment: body.trait_default_instantiation.clone(),
        function_pointer_locals: function_pointer_storage(body),
        name: start.selected_name,
        synthesized: Vec::new(),
        materialized_closures: Vec::new(),
        step_element: None,
        anchor: None,
        iteration_locals: list_iterator_locals(&body.block),
        step_bindings: Vec::new(),
        step_clauses: Vec::new(),
        scoped_locals,
        scope_parents,
        loop_scopes: Vec::new(),
        loop_exit_blocks: Vec::new(),
        loop_exit_owners: Vec::new(),
        forbidden_reads: Vec::new(),
        moved_tuple_fields: Vec::new(),
        body_identity: body.canonical.clone(),
        checked_local_types: body.locals.iter().map(|local| local.ty.clone()).collect(),
        initializing_decorator: start.initializing_decorator,
        cached_decorator: start.cached_decorator,
        callable_name_dynamic_profile,
        reference_values,
        returned_view: IncanInputAccess::Inferred,
        carries_view: false,
        invocation_storage: Vec::new(),
        released: Vec::new(),
    })
}

/// Initialize producer storage, parameter ownership and entry effects before lowering the checked body.
fn initialize_body_frame(
    module: &BodyIrModule,
    body: &Body,
    builder: &mut Builder,
    parameters: &[Parameter],
    declared_type: &PlanType,
    span: &SourceSpan,
) -> Result<(), String> {
    builder.new_block(span);
    if body.is_generator() {
        match generator_shape(declared_type) {
            Some((leaf, depth)) => {
                let handle = temporary(builder, PlanType::GeneratorYield(leaf, depth), span);
                builder.generator_handle = handle.local;
                builder.owned.push(handle.local);
            }
            None => return Err("unsupported Body IR generator return type".into()),
        }
    }
    for (index, parameter) in parameters.iter().enumerate() {
        let ty = &parameter.ty;
        if *ty == PlanType::String
            || is_nominal_type(ty)
            || is_owned_callable(ty)
            || is_future(ty)
            || owned_collection(ty)
        {
            builder.owned.push(index as i64 + 1);
        }
    }
    // Computed statics initialize on module entry (RFC 052), before a program entry installs the panic hook.
    if enters_module_initialization(body) {
        initialize_module_statics(builder, &body_module_identity(body)?, span);
    }
    // Only the checked program entry requests panic initialization; other functions named main remain ordinary calls.
    let entry = is_program_entry(module, body);
    if entry {
        let initialized = temporary(builder, PlanType::Unit, span);
        external_call(
            builder,
            "incan_native_runtime::initialize_panic_hook",
            Vec::new(),
            PlanType::Unit,
            Vec::new(),
            initialized,
            span,
        );
    }
    for index in 0..parameters.len() {
        validate_exact_float(builder, &plan_place(index as i64 + 1, span), span);
    }
    if entry {
        if !module.entrypoint_effects.is_empty() && (body.is_async || body.is_generator()) {
            return Err("unsupported Body IR producer entrypoint setup effects".into());
        }
        lower_entrypoint_effects(module, builder);
    }
    Ok(())
}

/// Print the entry module's retained setup before its body; project assembly does not merge dependency effects.
fn lower_entrypoint_effects(module: &BodyIrModule, builder: &mut Builder) {
    for effect in &module.entrypoint_effects {
        let EntrypointEffect::PrintText {
            text,
            span: source_span,
        } = effect;
        let span = builder_location(builder, source_span);
        let value = owned_literal(builder, text, &span);
        let output = temporary(builder, PlanType::Unit, &span);
        external_call(
            builder,
            "incan_native_runtime::println_text",
            vec![PlanType::String],
            PlanType::Unit,
            vec![read_place(value, true, &span)],
            output,
            &span,
        );
    }
}

/// A generator producer returns unit and holds only owned parameters; ordinary functions retain their declared result.
fn producer_return_type(
    constructors: &ConstructorIndex,
    module: &BodyIrModule,
    body: &Body,
    declared: PlanType,
    intrinsics: &[EnumDeclaration],
) -> Result<PlanType, String> {
    if !body.is_generator() {
        return Ok(declared);
    }
    for parameter in &body.params {
        let capture = frame_type(
            constructors,
            &module.nominal_declarations,
            &module.enum_declarations,
            &body.locals[parameter.local.index()],
            intrinsics,
            "",
        )?;
        if stored_parameter_type(body, parameter, capture.clone())? != capture {
            return Err(format!(
                "unsupported Body IR borrowed generator parameter in {}",
                body.name
            ));
        }
        if let PlanType::ModelRef(..) | PlanType::ModelMutRef(..) = capture {
            return Err("unsupported Body IR borrowed generator receiver".into());
        }
    }
    Ok(PlanType::Unit)
}

/// Retain the checked reference-value slots after ordinary type substitution. Receiver origins and borrowed
/// parameter representations do not change a source value's logical type and therefore add no slot here.
fn reference_value_storage(body: &Body, names: &[String], arguments: &[IncanType]) -> Result<Vec<i64>, String> {
    let mut slots = Vec::new();
    for local in &body.locals {
        let closed = substitute(&local.ty, names, arguments)
            .map_err(|reason| format!("{reason} in reference storage of {}", body.name))?;
        if let IncanType::Ref(_) | IncanType::RefMut(_) = closed {
            slots.push(local.id.index() as i64 + 1);
        }
    }
    Ok(slots)
}

/// Retain the checked scope IDs and parent links in Copy tables; a negative parent denotes the root scope.
#[allow(clippy::type_complexity)]
fn frame_scopes(body: &Body) -> (Vec<(i64, i64)>, Vec<(i64, i64)>) {
    let scoped_locals = body
        .locals
        .iter()
        .map(|local| (local.id.index() as i64 + 1, local.scope.0 as i64))
        .collect();
    let scope_parents = body
        .scopes
        .iter()
        .map(|scope| (scope.id.0 as i64, scope.parent.map_or(-1, |parent| parent.0 as i64)))
        .collect();
    (scoped_locals, scope_parents)
}

/// The frame's locals: the return slot first, then Body IR's locals in `LocalId` order.
#[allow(clippy::too_many_arguments)]
fn frame_locals(
    constructors: &ConstructorIndex,
    module: &BodyIrModule,
    body: &Body,
    return_type: &PlanType,
    span: &SourceSpan,
    source: &str,
    file: &str,
    intrinsics: &[EnumDeclaration],
) -> Result<Vec<Local>, String> {
    let iterators = list_iterator_locals(&body.block);
    let taking = taking_iterator_locals(&body.block);
    let pairs = pair_generator_locals(body);
    let starts = line_starts(source);
    let mut locals = vec![Local {
        ty: return_type.clone(),
        span: span.clone(),
    }];
    // Borrow each local and parameter: every frame types every local, so copied declarations cost per frame.
    for local in &body.locals {
        if local.id.index() + 1 != locals.len() {
            return Err("Body IR local order disagrees with LocalId".into());
        }
        let index = local.id.index() as i64 + 1;
        let checked = match future_type(constructors, module, &local.ty, intrinsics)? {
            Some(carrier) => carrier,
            None => body_frame_type(
                constructors,
                &module.nominal_declarations,
                &module.enum_declarations,
                body,
                local,
                intrinsics,
                file,
            )?,
        };
        let mut ty = local_representation(body, local.id.index(), checked);
        for parameter in &body.params {
            if parameter.local == local.id
                && parameter.input_access != IncanInputAccess::Inferred
                && (taking.contains(&index) || iterators.contains(&index) || pairs.contains(&index))
            {
                return Err("unsupported Body IR explicit input iteration storage".into());
            }
        }
        if pairs.contains(&index) {
            ty = pair_generator_type(ty);
        }
        if taking.contains(&index) {
            // A checked move preserves an admitted generator or zip carrier's existing ownership. Only an owned list
            // needs the consuming list cursor representation; borrowed or unsupported carriers cannot supply it.
            ty = match ty {
                PlanType::List(leaf, depth) => PlanType::ListIntoIter(leaf, depth),
                PlanType::Generator(..) | PlanType::ZipIterator(..) => ty,
                _ => return Err("unsupported Body IR consuming iteration outside an owned list".into()),
            };
        } else if iterators.contains(&index) {
            ty = iterator_storage(ty)?;
        }
        for parameter in &body.params {
            if parameter.local == local.id {
                let represented = parameter_representation(body, local.id.index(), ty)?;
                ty = stored_parameter_type(body, parameter, represented)?;
            }
        }
        locals.push(Local {
            ty,
            span: indexed_location(&starts, &local.span, file),
        });
    }
    Ok(locals)
}

/// A builtin iterator's cursor storage: a frozen snapshot, a zip or generator poll reference, or a list view.
fn iterator_storage(ty: PlanType) -> Result<PlanType, String> {
    let mut ty = ty;
    if let PlanType::FrozenCollection(..) = ty {
        ty = frozen_list_snapshot_type(&ty, false)?;
    }
    if let Some((left, right)) = zip_shape(&ty) {
        ty = PlanType::ZipIteratorMutRef(left, right);
    }
    match generator_shape(&ty) {
        Some((leaf, depth)) => Ok(PlanType::GeneratorMutRef(leaf, depth)),
        None => Ok(list_parameter_type(&ty, false)),
    }
}

/// Resolve a frame local, retaining its location and Body IR's own unsupported-construct explanation on failure.
fn body_frame_type(
    constructors: &ConstructorIndex,
    nominals: &[NominalDeclaration],
    enums: &[SourceEnumDeclaration],
    body: &Body,
    local: &LocalDecl,
    intrinsics: &[EnumDeclaration],
    file: &str,
) -> Result<PlanType, String> {
    match frame_type(constructors, nominals, enums, local, intrinsics, file) {
        Ok(ty) => Ok(ty),
        Err(reason) => {
            let mut context = format!("{reason} in {} frame local {} in {file}", body.name, local.id.index());
            if let Some(gap) = body.first_representation_gap() {
                context = format!("{context}; unsupported source construct: {gap}");
            }
            Err(format!(
                "{context} at source bytes {}..{}",
                local.span.start, local.span.end
            ))
        }
    }
}

/// Read a declared frame slot's checked representation for its definition and generated runtime forwarders.
pub fn frame_parameter_type(
    constructors: &ConstructorIndex,
    nominals: &[NominalDeclaration],
    enums: &[SourceEnumDeclaration],
    body: &Body,
    parameter: &CallableParam,
    intrinsics: &[EnumDeclaration],
    file: &str,
) -> Result<PlanType, String> {
    let local = &body.locals[parameter.local.index()];
    let ty = parameter_representation(
        body,
        parameter.local.index(),
        body_frame_type(constructors, nominals, enums, body, local, intrinsics, file)?,
    )?;
    if parameter.input_access == IncanInputAccess::Inferred && parameter.mutable && lacks_exclusive_parameter(&ty) {
        return Err("unsupported Body IR mutable tuple or iterator parameter".into());
    }
    if parameter.input_access == IncanInputAccess::Inferred && ty == PlanType::FrozenStr && parameter.mutable {
        return Err("unsupported Body IR mutable FrozenStr parameter: mutable scalar parameter representation".into());
    }
    stored_parameter_type(body, parameter, ty)
}

/// Build declaration-ordered parameters from their checked frame storage and source locations.
pub fn frame_parameters(
    constructors: &ConstructorIndex,
    module: &BodyIrModule,
    body: &Body,
    source: &str,
    file: &str,
    intrinsics: &[EnumDeclaration],
) -> Result<Vec<Parameter>, String> {
    let mut parameters: Vec<Parameter> = Vec::new();
    let starts = line_starts(source);
    let nominals = &module.nominal_declarations;
    let enums = &module.enum_declarations;
    for parameter in &body.params {
        let ty = frame_parameter_type(constructors, nominals, enums, body, parameter, intrinsics, file)?;
        if parameter.local.index() != parameters.len() {
            return Err("Body IR parameter order disagrees with scalar frame".into());
        }
        if !matches!(parameter.default, CallableParamDefault::Required) {
            let local = &body.locals[parameter.local.index()];
            let declared = body_frame_type(constructors, nominals, enums, body, local, intrinsics, file)?;
            default_type(&parameter_representation(body, parameter.local.index(), declared)?)?;
        }
        let name = if parameter.name == "self" {
            "__incan_self".to_string()
        } else {
            parameter.name.clone()
        };
        parameters.push(Parameter {
            name,
            ty,
            span: indexed_location(&starts, &parameter.span, file),
        });
    }
    Ok(parameters)
}
