//! Computed and literal module statics planned as guarded adapter cells (RFC 052), and their reads.

use incan_mir_plan_native::{
    Callee, CalleeKind, Constant, EnumDeclaration, ExternalFunction, FieldProjection, Function, Local, Operand,
    OperandKind, Parameter, Place, PlanType, Projection, RvalueKind, SourceSpan, StaticCell, TerminatorKind, Unwind,
    assign as plan_assign, block as plan_block, place as plan_place, source_span as plan_span,
};
use incan_semantics_core::IncanType;
use incan_semantics_core::body_ir::{
    AggregateKind, ArgumentElement, Body, BodyIrModule, Constant as SourceConstant, GlobalPlace, GlobalWritePolicy,
    Operand as SourceOperand, OwnershipFact, Place as SourcePlace, PlaceElem, Rvalue, StaticInitializer,
};

use crate::assignments::lower_local_assignment;
use crate::carrier_operands::element_operand;
use crate::checked_values::{scalar_operand, target_read};
use crate::closure_lifting::synthesize;
use crate::closures::callable_copy_type;
use crate::collections::{
    begin_collection, collection_arguments, collection_name, collection_reference, finish_collection,
    insert_collection, is_collection, is_owned_collection, owned_runtime_operand,
};
use crate::computed_statics::{
    has_native_static_cell, initialize_module_statics, is_computed_static, module_initialization_externals,
    module_initializer, module_static_key, static_initializer_name, static_owner,
};
use crate::decls::{static_accessors, static_slot};
use crate::decorator_caches::DecoratorFrame;
use crate::frame::{Builder, external_call, forget_owned, literal, place_type, read_place, temporary, terminate_call};
use crate::frame_instances::{FunctionNames, append_synthesized, function_names, merge_externals, named_functions};
use crate::function_frames::lower_function;
use crate::hashed_collections::dictionary_value_read;
use crate::layout_types::{mutable_receiver_type, native_source_type};
use crate::list_values::{list_assignment, list_base_place, projected_element_read, value_operand};
use crate::lists::{is_list, is_owned_list, list_element, require_list_shape};
use crate::lowering::Inventories;
use crate::matches::read_generic_decorator;
use crate::models::{field_path, field_step_type, model_type};
use crate::numerics::numeric_carrier;
use crate::program_context::ProgramContext;
use crate::statements::is_nominal_type;
use crate::static_guards::{FieldCopy, field_copier, field_copy, uncloned_enum};
use crate::string_input::{SyntheticFrame, synthetic_builder};
use crate::type_parameters::merge_instance_requests;

/// Plan computed statics and effect-free literal lists or dictionaries as guarded adapter cells (RFC 052).
///
/// A computed initializer body returns the first value, and its module gets a declaration-order initializer. Literal
/// dictionaries construct their checked entries without entering that module initialization protocol. A continuation
/// starts at the first newly resolved declaration so preserved authored cells are never lowered twice.
#[allow(clippy::too_many_arguments)]
pub fn lower_computed_statics(
    module: &BodyIrModule,
    program: &mut ProgramContext,
    unions: &[IncanType],
    sources: &[String],
    files: &[String],
    owners: &[String],
    intrinsics: &mut Vec<EnumDeclaration>,
    inventories: &mut Inventories,
    start_slot: usize,
) -> Result<Vec<StaticCell>, String> {
    let sources = StaticSources { sources, files, owners };
    let mut cells = Vec::new();
    let mut keys: Vec<i64> = Vec::new();
    let mut names = function_names();
    for slot in start_slot..module.static_declarations.len() {
        let declaration = &module.static_declarations[slot];
        for binding in &module.decorated_bindings {
            if binding.original_body.canonical.as_ref() == Some(&declaration.canonical) {
                let owner_index = static_source_position(
                    owners,
                    &static_owner(declaration),
                    "unsupported Body IR original callable without source provenance",
                )?;
                let (mut original, runtime, pending, synthesized) = lower_function(
                    module,
                    program,
                    &binding.original_body,
                    unions,
                    &sources.sources[owner_index],
                    &sources.files[owner_index],
                    intrinsics,
                    &[],
                    DecoratorFrame::Normal,
                )?;
                original.name = format!("{}__incan_original", original.name);
                if !named_functions(&mut names, &inventories.functions, &original.name).is_empty() {
                    return Err("unsupported Body IR original callable name collision".into());
                }
                inventories.functions.push(original);
                append_synthesized(&mut inventories.functions, &mut names, synthesized)?;
                merge_externals(&mut inventories.externals, runtime)?;
                merge_instance_requests(&mut inventories.instances, &pending);
            }
        }
        match &declaration.initial {
            StaticInitializer::Computed(body) => {
                let lowered = ComputedStatic { slot, body };
                let cell = computed_static_cell(
                    module,
                    program,
                    unions,
                    &sources,
                    intrinsics,
                    &lowered,
                    &mut keys,
                    (inventories, &mut names),
                )?;
                cells.push(cell);
            }
            StaticInitializer::List(_) | StaticInitializer::Dict(_) => {
                let owner = static_owner(declaration);
                let mut file = "";
                for (index, candidate) in owners.iter().enumerate() {
                    if *candidate == owner {
                        file = &files[index];
                    }
                }
                if file.is_empty() {
                    return Err("unsupported Body IR literal collection without source provenance".into());
                }
                let (initializer, runtime) = lower_literal_static(module, program, slot, unions, intrinsics, file)?;
                cells.push(StaticCell {
                    slot: slot as i64,
                    ty: initializer.return_type.clone(),
                    initializer: initializer.name.clone(),
                    span: initializer.span.clone(),
                    module_completion: false,
                });
                inventories.functions.push(initializer);
                merge_externals(&mut inventories.externals, runtime)?;
            }
            _ => {}
        }
    }
    Ok(cells)
}

/// The unit's checked sources, files and module owners, in one order.
struct StaticSources<'a> {
    sources: &'a [String],
    files: &'a [String],
    owners: &'a [String],
}

/// One computed static: its slot and checked initializer body.
struct ComputedStatic<'a> {
    slot: usize,
    body: &'a Body,
}

/// Lower one computed static's initializer, registering its module initializer once per module.
#[allow(clippy::too_many_arguments)]
fn computed_static_cell(
    module: &BodyIrModule,
    program: &ProgramContext,
    unions: &[IncanType],
    sources: &StaticSources<'_>,
    intrinsics: &mut Vec<EnumDeclaration>,
    lowered: &ComputedStatic<'_>,
    keys: &mut Vec<i64>,
    (inventories, names): (&mut Inventories, &mut FunctionNames),
) -> Result<StaticCell, String> {
    let declaration = &module.static_declarations[lowered.slot];
    let owner = static_owner(declaration);
    let source_index = static_source_position(
        sources.owners,
        &owner,
        "unsupported Body IR computed static without source provenance",
    )?;
    let (mut initializer, runtime, pending, synthesized) = lower_function(
        module,
        program,
        lowered.body,
        unions,
        &sources.sources[source_index],
        &sources.files[source_index],
        intrinsics,
        &[],
        DecoratorFrame::Normal,
    )?;
    initializer.name = static_initializer_name(lowered.slot);
    let ty = native_source_type(
        &program.constructors,
        &module.nominal_declarations,
        &module.enum_declarations,
        unions,
        &declaration.ty,
        intrinsics,
        "",
    )?;
    if initializer.return_type != ty {
        return Err("invalid Body IR computed static initializer carrier".into());
    }
    let key = module_static_key(&program.statics, &owner);
    let cell = StaticCell {
        slot: lowered.slot as i64,
        ty,
        initializer: initializer.name.clone(),
        span: initializer.span.clone(),
        module_completion: lowered.slot as i64 == key,
    };
    if !keys.contains(&key) {
        keys.push(key);
        inventories
            .functions
            .push(module_initializer(&program.statics, key, &initializer.span));
        merge_externals(
            &mut inventories.externals,
            module_initialization_externals(&initializer.span),
        )?;
    }
    inventories.functions.push(initializer);
    append_synthesized(&mut inventories.functions, names, synthesized)?;
    merge_externals(&mut inventories.externals, runtime)?;
    merge_instance_requests(&mut inventories.instances, &pending);
    Ok(cell)
}

/// Keep the last matching checked source owner and retain the caller's precise missing-provenance refusal.
fn static_source_position(owners: &[String], owner: &str, missing: &str) -> Result<usize, String> {
    owners
        .iter()
        .rposition(|candidate| candidate == owner)
        .ok_or_else(|| missing.to_string())
}

/// Build literal list or dictionary initializers for guarded cells without runtime-managed persistent storage.
fn lower_literal_static(
    module: &BodyIrModule,
    program: &ProgramContext,
    slot: usize,
    unions: &[IncanType],
    intrinsics: &[EnumDeclaration],
    file: &str,
) -> Result<(Function, Vec<ExternalFunction>), String> {
    let declaration = &module.static_declarations[slot];
    let ty = native_source_type(
        &program.constructors,
        &module.nominal_declarations,
        &module.enum_declarations,
        unions,
        &declaration.ty,
        intrinsics,
        "",
    )?;
    let span = plan_span(file, 1, 1);
    let name = static_initializer_name(slot);
    let frame = SyntheticFrame {
        locals: vec![Local {
            ty: ty.clone(),
            span: span.clone(),
        }],
        parameter_count: 0,
        file: file.to_string(),
        module_identity: static_owner(declaration),
        unions: unions.to_vec(),
        intrinsics: intrinsics.to_vec(),
        name: name.clone(),
        owned: Vec::new(),
    };
    let mut builder = synthetic_builder(program.clone(), frame);
    builder.new_block(&span);
    let value = static_initial_operand(&mut builder, &declaration.initial, &ty, &span)?;
    builder.emit(plan_assign(plan_place(0, &span), RvalueKind::Use(value), &span));
    builder.finish(TerminatorKind::Return, &span);
    let function = Function::new(name, Vec::new(), ty, builder.locals, builder.blocks, span);
    Ok((function, builder.externals))
}

/// Whether a loaded static value owns resources its frame must drop, rather than being a plain copy.
pub fn snapshot_owns(ty: &PlanType) -> bool {
    if numeric_carrier(ty) || matches!(ty, PlanType::Bool | PlanType::Unit | PlanType::FrozenStr) {
        return false;
    }
    !callable_copy_type(ty)
}

/// Read persistent storage without consuming it; literal reads leave computed initialization untouched.
pub fn read_static(
    builder: &mut Builder,
    value: &SourcePlace,
    global: &GlobalPlace,
    fact: OwnershipFact,
    span: &SourceSpan,
) -> Result<Operand, String> {
    if let Some(pointer) = read_generic_decorator(builder, value, global, span)? {
        return Ok(pointer);
    }
    let statics = builder.program.statics.clone();
    let slot = static_slot(&statics, global)?;
    if has_native_static_cell(&statics[slot]) {
        return read_computed_static(builder, value, global, slot, fact, span);
    }
    let ty = model_type(builder.constructors(), builder.nominals(), &global.ty, "")?;
    let projected = !value.projection.is_empty();
    let owned_collection = is_owned_collection(&ty);
    if projected && !owned_collection {
        return Err("unsupported Body IR static projection".into());
    }
    if owned_collection {
        if !matches!(
            fact,
            OwnershipFact::Clone | OwnershipFact::Borrow | OwnershipFact::MutBorrow
        ) && (!projected || fact != OwnershipFact::Copy)
        {
            return Err("unsupported Body IR static collection ownership".into());
        }
    } else if ty == PlanType::String {
        if !matches!(fact, OwnershipFact::Clone | OwnershipFact::Borrow) {
            return Err("unsupported Body IR static string ownership".into());
        }
    } else if fact != OwnershipFact::Copy {
        return Err("unsupported Body IR static ownership".into());
    }
    let (read_cell, _) = static_accessors(&ty)?;
    // Keep the immutable initializer owner alive independently while materialization mutates the frame.
    let initial = static_initial_operand(builder, &statics[slot].initial, &ty, span)?;
    let target = temporary(builder, ty.clone(), span);
    external_call(
        builder,
        read_cell,
        vec![PlanType::Int, ty.clone()],
        ty.clone(),
        vec![literal(Constant::Int(slot as i64), span), initial],
        target.clone(),
        span,
    );
    if !owned_collection {
        return Ok(read_place(target, ty == PlanType::String, span));
    }
    builder.owned.push(target.local);
    // An element read selects from the snapshot, which stays owned until its scope ends.
    if !projected {
        return Ok(read_place(target, true, span));
    }
    if is_collection(&ty) {
        return dictionary_value_read(builder, value, target, span);
    }
    projected_element_read(builder, value, target, 0, span)
}

/// The checked carrier of a static's declared type in this frame.
pub fn static_carrier(builder: &Builder, global: &GlobalPlace) -> Result<PlanType, String> {
    native_source_type(
        builder.constructors(),
        builder.nominals(),
        builder.enums(),
        &builder.unions,
        &global.ty,
        &builder.intrinsics,
        "",
    )
}

/// Read a native guarded static (RFC 052), initializing its module only for a computed value.
///
/// A selected model field is copied or cloned under the actual shared guard, without copying unrelated fields, and a
/// keyed dictionary entry is cloned alone. Other supported reads use an owned snapshot. Only a collection is borrowed
/// mutably, by a method that commits its changed copy back.
fn read_computed_static(
    builder: &mut Builder,
    value: &SourcePlace,
    global: &GlobalPlace,
    slot: usize,
    fact: OwnershipFact,
    span: &SourceSpan,
) -> Result<Operand, String> {
    let ty = static_carrier(builder, global)?;
    let projected = !value.projection.is_empty();
    match fact {
        OwnershipFact::Unknown => return Err("unsupported Body IR static ownership".into()),
        OwnershipFact::MutBorrow if projected || !is_owned_collection(&ty) => {
            return Err("unsupported Body IR mutable borrow of a computed static".into());
        }
        OwnershipFact::Move if projected => {
            return Err("unsupported Body IR partial move out of a static".into());
        }
        _ => {}
    }
    if projected {
        match guarded_field(builder, value, slot, &ty, span)? {
            GuardedField::Read(field, owned) => return Ok(read_place(field, owned, span)),
            GuardedField::Uncloned => return Err("unsupported Body IR static enum field without retained Clone".into()),
            GuardedField::Unread => {}
        }
        // A keyed read borrows the stored dictionary under its guard instead of copying it into a snapshot.
        if let Some(entry) = read_guarded_dictionary_entry(builder, value, slot, &ty, span)? {
            return projected_element_read(builder, value, entry, 1, span);
        }
    }
    let snapshot = load_computed_static(builder, slot, &ty, span);
    if !projected {
        return Ok(read_place(snapshot, snapshot_owns(&ty), span));
    }
    if is_collection(&ty) {
        return dictionary_value_read(builder, value, snapshot, span);
    }
    if is_owned_collection(&ty) {
        return projected_element_read(builder, value, snapshot, 0, span);
    }
    let selected = snapshot_projection(builder, value, snapshot, span)?;
    // The copy stays whole until its scope ends, so a borrowed field is read as a clone rather than moved out of it.
    if fact == OwnershipFact::Borrow {
        return target_read(builder, selected, OwnershipFact::Clone, span);
    }
    target_read(builder, selected, fact, span)
}

/// What a guarded field read found for one projection.
enum GuardedField {
    /// The field copied out under the guard into this frame-owned local, and whether that local owns resources.
    Read(Place, bool),
    /// An enum field whose declaration retains no Clone.
    Uncloned,
    /// A projection this route does not read: a payload, an index, or a carrier it cannot copy.
    Unread,
}

/// Copy one selected field of a model static under its actual shared storage guard, never its unrelated fields: a
/// plain value is copied and an owned one cloned, into a local of this frame made before any later write.
fn guarded_field(
    builder: &mut Builder,
    value: &SourcePlace,
    slot: usize,
    owner: &PlanType,
    span: &SourceSpan,
) -> Result<GuardedField, String> {
    let PlanType::Model(index, name) = owner else {
        return Ok(GuardedField::Unread);
    };
    let receiver = PlanType::ModelRef(*index, name.clone());
    let fields = field_path(
        builder.tables(),
        owner.clone(),
        &value.projection,
        value.projection.len(),
    )?;
    let Some(last) = fields.last() else {
        return Ok(GuardedField::Unread);
    };
    if fields.iter().any(|field| field.variant >= 0) {
        return Ok(GuardedField::Unread);
    }
    let result = last.ty.clone();
    if uncloned_enum(builder, &result) {
        return Ok(GuardedField::Uncloned);
    }
    let Some(copy) = field_copy(builder, &result, span)? else {
        return Ok(GuardedField::Unread);
    };
    let owned = matches!(copy, FieldCopy::Clone { .. });
    let name = format!(
        "{}__incan_static_field_{slot}_{}",
        builder.name,
        builder.synthesized.len()
    );
    builder
        .synthesized
        .push(field_copier(name.clone(), receiver, result.clone(), fields, copy, span));
    let owner_module = static_owner(&builder.program.statics[slot]);
    initialize_module_statics(builder, &owner_module, span);
    let output = temporary(builder, result, span);
    terminate_call(
        builder,
        CalleeKind::StaticMethod(slot as i64, name),
        Vec::new(),
        output.clone(),
        span,
    );
    if owned {
        builder.owned.push(output.local);
    }
    Ok(GuardedField::Read(output, owned))
}

/// One synthesized StaticMethod accessor: its name, borrowed receiver, result and the call it makes.
struct StaticReader {
    name: String,
    receiver: PlanType,
    result: PlanType,
    callee: CalleeKind,
}

/// Clone the one value a key selects in a guarded dictionary, without copying the dictionary.
///
/// The key is evaluated before its accessor takes the shared storage guard, and only the selected value is cloned
/// under it. Text, numeric and boolean keys are read this way; any other key keeps the snapshot read and its refusals.
fn read_guarded_dictionary_entry(
    builder: &mut Builder,
    value: &SourcePlace,
    slot: usize,
    owner: &PlanType,
    span: &SourceSpan,
) -> Result<Option<Place>, String> {
    if !matches!(owner, PlanType::Dict(..)) {
        return Ok(None);
    }
    let types = collection_arguments(owner);
    if types.len() != 2 {
        return Ok(None);
    }
    if types[0] != PlanType::String && types[0] != PlanType::Bool && !numeric_carrier(&types[0]) {
        return Ok(None);
    }
    let Some(PlaceElem::Index(index)) = value.projection.first() else {
        return Ok(None);
    };
    if is_computed_static(&builder.program.statics[slot]) {
        let owner_module = static_owner(&builder.program.statics[slot]);
        initialize_module_statics(builder, &owner_module, span);
    }
    let key = value_operand(builder, index, span)?;
    let Some(admitted) = owned_runtime_operand(builder, key, &types[0], span)? else {
        return Err("unsupported Body IR collection runtime argument coercion".into());
    };
    let reference = collection_reference(owner, false);
    let path = format!("incan_native_runtime::{}", collection_name(owner, "get")?);
    builder.externals.push(ExternalFunction {
        path: path.clone(),
        type_arguments: types.clone(),
        parameters: vec![reference.clone(), types[0].clone()],
        return_type: types[1].clone(),
        span: span.clone(),
    });
    let name = format!(
        "{}__incan_static_entry_{slot}_{}",
        builder.name,
        builder.synthesized.len()
    );
    let reader = StaticReader {
        name: name.clone(),
        receiver: reference,
        result: types[1].clone(),
        callee: CalleeKind::Instantiated(path, types.clone()),
    };
    builder.synthesized.push(static_entry_reader(reader, &types[0], span));
    let output = temporary(builder, types[1].clone(), span);
    terminate_call(
        builder,
        CalleeKind::StaticMethod(slot as i64, name),
        vec![admitted],
        output.clone(),
        span,
    );
    // The cloned value is a new owner, as the value a snapshot's lookup clones is.
    let entry = &types[1];
    if *entry == PlanType::String || is_owned_collection(entry) || is_nominal_type(entry) || is_owned_list(entry) {
        builder.owned.push(output.local);
    }
    Ok(Some(output))
}

/// Look one key up in the borrowed dictionary while the enclosing StaticMethod accessor keeps its guard alive.
fn static_entry_reader(reader: StaticReader, key: &PlanType, span: &SourceSpan) -> Function {
    let invocation = TerminatorKind::Call(
        Callee {
            kind: reader.callee,
            span: span.clone(),
        },
        vec![
            read_place(plan_place(1, span), false, span),
            read_place(plan_place(2, span), snapshot_owns(key), span),
        ],
        plan_place(0, span),
        1,
        Unwind::Continue,
    );
    let local = |ty: &PlanType| Local {
        ty: ty.clone(),
        span: span.clone(),
    };
    let parameter = |name: &str, ty: &PlanType| Parameter {
        name: name.into(),
        ty: ty.clone(),
        span: span.clone(),
    };
    Function::new(
        reader.name,
        vec![
            parameter("__incan_receiver", &reader.receiver),
            parameter("__incan_key", key),
        ],
        reader.result.clone(),
        vec![local(&reader.result), local(&reader.receiver), local(key)],
        vec![
            plan_block(Vec::new(), invocation, span),
            plan_block(Vec::new(), TerminatorKind::Return, span),
        ],
        span.clone(),
    )
}

/// Load a guarded snapshot; only computed storage enters module initialization before the load.
fn load_computed_static(builder: &mut Builder, slot: usize, ty: &PlanType, span: &SourceSpan) -> Place {
    if is_computed_static(&builder.program.statics[slot]) {
        let owner_module = static_owner(&builder.program.statics[slot]);
        initialize_module_statics(builder, &owner_module, span);
    }
    let snapshot = temporary(builder, ty.clone(), span);
    terminate_call(
        builder,
        CalleeKind::StaticLoad(slot as i64),
        Vec::new(),
        snapshot.clone(),
        span,
    );
    if snapshot_owns(ty) {
        builder.owned.push(snapshot.local);
    }
    snapshot
}

/// The plan place an element read selected, for a caller that needs storage rather than a value.
fn selected_place(selected: Operand) -> Result<Place, String> {
    match selected.kind {
        OperandKind::Copy(place) | OperandKind::Move(place) => Ok(place),
        OperandKind::Literal(_) => Err("unsupported Body IR computed static element without a place".into()),
    }
}

/// Select a native guarded static's projection, or return None for other storage.
///
/// A model field is copied alone and a keyed dictionary entry read under the storage guard; any other projection is
/// selected inside a fresh snapshot.
pub fn computed_static_root(
    builder: &mut Builder,
    value: &SourcePlace,
    span: &SourceSpan,
) -> Result<Option<Place>, String> {
    let Some(global) = value.global() else {
        return Ok(None);
    };
    let slot = static_slot(&builder.program.statics, global)?;
    if !has_native_static_cell(&builder.program.statics[slot]) {
        return Ok(None);
    }
    let ty = static_carrier(builder, global)?;
    if !value.projection.is_empty()
        && let GuardedField::Read(field, _) = guarded_field(builder, value, slot, &ty, span)?
    {
        return Ok(Some(field));
    }
    if !value.projection.is_empty()
        && let Some(entry) = read_guarded_dictionary_entry(builder, value, slot, &ty, span)?
    {
        let selected = projected_element_read(builder, value, entry, 1, span)?;
        return Ok(Some(selected_place(selected)?));
    }
    let snapshot = load_computed_static(builder, slot, &ty, span);
    if value.projection.is_empty() {
        return Ok(Some(snapshot));
    }
    if is_owned_collection(&ty) {
        let selected = projected_element_read(builder, value, snapshot, 0, span)?;
        return Ok(Some(selected_place(selected)?));
    }
    Ok(Some(snapshot_projection(builder, value, snapshot, span)?))
}

/// The checked carrier a static place selects, without reading the static.
pub fn computed_static_type(builder: &Builder, value: &SourcePlace, global: &GlobalPlace) -> Result<PlanType, String> {
    let mut ty = static_carrier(builder, global)?;
    for projection in &value.projection {
        match projection {
            PlaceElem::Index(_) if is_collection(&ty) => {
                let types = collection_arguments(&ty);
                if types.len() != 2 {
                    return Err("unsupported Body IR computed static set indexing".into());
                }
                ty = types[1].clone();
                continue;
            }
            PlaceElem::Index(_) if is_list(&ty) => {
                let (leaf, depth) = require_list_shape(&ty, "unsupported Body IR computed static list indexing")?;
                ty = list_element(&leaf, depth);
                continue;
            }
            PlaceElem::Slice { .. } if is_list(&ty) => continue,
            _ => {}
        }
        ty = field_step_type(builder.tables(), &ty, projection)?;
    }
    Ok(ty)
}

/// Select a static place's checked field and payload path inside its loaded copy.
fn snapshot_projection(
    builder: &mut Builder,
    value: &SourcePlace,
    snapshot: Place,
    span: &SourceSpan,
) -> Result<Place, String> {
    let root = place_type(&builder.locals, &snapshot);
    let mut fields = field_path(builder.tables(), root, &value.projection, value.projection.len())?;
    let projection = if fields.len() == 1 && fields[0].variant >= 0 {
        let field = fields.remove(0);
        Projection::VariantField(field.variant, field.slot, field.ty)
    } else if fields.len() == 1 {
        let field = fields.remove(0);
        Projection::Field(field.slot, field.ty)
    } else {
        Projection::Fields(fields)
    };
    Ok(Place {
        local: snapshot.local,
        projection,
        span: span.clone(),
    })
}

/// Materialize the checked effect-free initializer without reading source syntax or replacing stored values.
fn static_initial_operand(
    builder: &mut Builder,
    initial: &StaticInitializer,
    ty: &PlanType,
    span: &SourceSpan,
) -> Result<Operand, String> {
    match initial {
        StaticInitializer::Literal(value) => scalar_operand(builder, &SourceOperand::Constant(value.clone()), span),
        StaticInitializer::List(values) => {
            let elements = values
                .iter()
                .map(|value| ArgumentElement::One(SourceOperand::Constant(value.clone())))
                .collect();
            let target = temporary(builder, ty.clone(), span);
            list_assignment(
                builder,
                target.clone(),
                &Rvalue::Aggregate(AggregateKind::List, elements),
                span,
            )?;
            Ok(read_place(target, true, span))
        }
        StaticInitializer::Dict(entries) => {
            let keys: Vec<SourceConstant> = entries.iter().map(|(key, _)| key.clone()).collect();
            let values: Vec<SourceConstant> = entries.iter().map(|(_, value)| value.clone()).collect();
            static_hashed_initial(builder, ty, &keys, &values, span)
        }
        StaticInitializer::Set(members) => static_hashed_initial(builder, ty, members, &[], span),
        StaticInitializer::Computed(_) => Err("invalid Body IR computed static as a literal initial value".into()),
    }
}

/// Build a dictionary from checked literal keys and their values, or a set from its members, in source order.
fn static_hashed_initial(
    builder: &mut Builder,
    ty: &PlanType,
    keys: &[SourceConstant],
    values: &[SourceConstant],
    span: &SourceSpan,
) -> Result<Operand, String> {
    let target = temporary(builder, ty.clone(), span);
    let output = begin_collection(builder, &target, span)?;
    let types = collection_arguments(ty);
    if types.len() == 2 && values.len() != keys.len() {
        return Err("invalid Body IR static dictionary entries".into());
    }
    if types.len() == 1 && !values.is_empty() {
        return Err("invalid Body IR static set members".into());
    }
    for (index, key) in keys.iter().enumerate() {
        let mut operands = vec![element_operand(
            builder,
            &SourceOperand::Constant(key.clone()),
            &types[0],
            span,
        )?];
        if types.len() == 2 {
            operands.push(element_operand(
                builder,
                &SourceOperand::Constant(values[index].clone()),
                &types[1],
                span,
            )?);
        }
        insert_collection(builder, &output, operands, span)?;
    }
    finish_collection(builder, &target, output, span);
    Ok(read_place(target, true, span))
}

/// Evaluate a write before committing it to a checker-authorized persistent cell.
pub fn write_static(
    module: &BodyIrModule,
    builder: &mut Builder,
    destination: &SourcePlace,
    global: &GlobalPlace,
    value: &Rvalue,
    span: &SourceSpan,
) -> Result<(), String> {
    if global.write_policy != GlobalWritePolicy::Rebindable {
        return Err("unsupported Body IR static write policy".into());
    }
    let slot = static_slot(&builder.program.statics, global)?;
    let ty = if has_native_static_cell(&builder.program.statics[slot]) {
        static_carrier(builder, global)?
    } else {
        model_type(builder.constructors(), builder.nominals(), &global.ty, "")?
    };
    if !destination.projection.is_empty() {
        if is_computed_static(&builder.program.statics[slot]) && matches!(ty, PlanType::Model(..)) {
            return write_guarded_static_field(module, builder, destination, global, value, ty, span);
        }
        if !is_collection(&ty) {
            return Err("unsupported Body IR static projection".into());
        }
        return write_static_entry(builder, destination, global, value, span);
    }
    let target = temporary(builder, ty, span);
    lower_local_assignment(module, builder, target.clone(), value, span)?;
    persist_static_value(builder, global, target, span)
}

/// Evaluate the replacement before locking storage, then replace only the checked field under its exclusive guard.
fn write_guarded_static_field(
    module: &BodyIrModule,
    builder: &mut Builder,
    destination: &SourcePlace,
    global: &GlobalPlace,
    value: &Rvalue,
    owner: PlanType,
    span: &SourceSpan,
) -> Result<(), String> {
    let fields = field_path(
        builder.tables(),
        owner.clone(),
        &destination.projection,
        destination.projection.len(),
    )?;
    let Some(last) = fields.last() else {
        return Err("invalid Body IR static field write without a field".into());
    };
    let selected = last.ty.clone();
    let reference = mutable_receiver_type(selected.clone())?;
    let root_reference = mutable_receiver_type(owner)?;
    let replacement = temporary(builder, selected.clone(), span);
    lower_local_assignment(module, builder, replacement.clone(), value, span)?;
    let slot = static_slot(&builder.program.statics, global)?;
    let name = format!(
        "{}__incan_static_write_{slot}_{}",
        builder.name,
        builder.synthesized.len()
    );
    let path = "core::mem::replace";
    builder.externals.push(ExternalFunction {
        path: path.into(),
        type_arguments: vec![selected.clone()],
        parameters: vec![reference.clone(), selected.clone()],
        return_type: selected.clone(),
        span: span.clone(),
    });
    let writer = static_field_writer(&name, root_reference, fields, selected, reference, span);
    synthesize(builder, writer);
    let owner_module = static_owner(&builder.program.statics[slot]);
    initialize_module_statics(builder, &owner_module, span);
    forget_owned(builder, replacement.local);
    let output = temporary(builder, PlanType::Unit, span);
    terminate_call(
        builder,
        CalleeKind::StaticMethod(slot as i64, name),
        vec![read_place(replacement, true, span)],
        output,
        span,
    );
    Ok(())
}

/// Replace before dropping the old field, so storage remains initialized even if its destructor unwinds.
fn static_field_writer(
    name: &str,
    root_reference: PlanType,
    fields: Vec<FieldProjection>,
    selected: PlanType,
    reference: PlanType,
    span: &SourceSpan,
) -> Function {
    let field = Place {
        local: 1,
        projection: Projection::DerefFields(fields),
        span: span.clone(),
    };
    let borrow = plan_assign(plan_place(3, span), RvalueKind::MutBorrow(field), span);
    let replace = TerminatorKind::Call(
        Callee {
            kind: CalleeKind::Instantiated("core::mem::replace".into(), vec![selected.clone()]),
            span: span.clone(),
        },
        vec![
            read_place(plan_place(3, span), true, span),
            read_place(plan_place(2, span), true, span),
        ],
        plan_place(4, span),
        1,
        Unwind::Continue,
    );
    let finished = plan_assign(
        plan_place(0, span),
        RvalueKind::Use(literal(Constant::Unit, span)),
        span,
    );
    let local = |ty: &PlanType| Local {
        ty: ty.clone(),
        span: span.clone(),
    };
    let parameter = |name: &str, ty: &PlanType| Parameter {
        name: name.into(),
        ty: ty.clone(),
        span: span.clone(),
    };
    Function::new(
        name.to_string(),
        vec![
            parameter("__incan_root", &root_reference),
            parameter("__incan_value", &selected),
        ],
        PlanType::Unit,
        vec![
            local(&PlanType::Unit),
            local(&root_reference),
            local(&selected),
            local(&reference),
            local(&selected),
        ],
        vec![
            plan_block(vec![borrow], replace, span),
            plan_block(
                Vec::new(),
                TerminatorKind::Drop(plan_place(4, span), 2, Unwind::Continue),
                span,
            ),
            plan_block(vec![finished], TerminatorKind::Return, span),
        ],
        span.clone(),
    )
}

/// Write one static dictionary entry: evaluate the value and then the key, insert them into the current snapshot, and
/// commit that snapshot.
///
/// The snapshot is read only after both are evaluated, so a value that itself changes the static is not overwritten by
/// a stale copy.
fn write_static_entry(
    builder: &mut Builder,
    destination: &SourcePlace,
    global: &GlobalPlace,
    value: &Rvalue,
    span: &SourceSpan,
) -> Result<(), String> {
    if destination.projection.len() != 1 {
        return Err("unsupported Body IR nested static dictionary mutation".into());
    }
    let types = collection_arguments(&static_carrier(builder, global)?);
    if types.len() != 2 {
        return Err("unsupported Body IR static set indexing assignment".into());
    }
    let Rvalue::Use(operand) = value else {
        return Err("unsupported Body IR static dictionary indexed rvalue".into());
    };
    let argument = element_operand(builder, operand, &types[1], span)?;
    let PlaceElem::Index(index) = &destination.projection[0] else {
        return Err("unsupported Body IR static dictionary mutation projection".into());
    };
    let key = element_operand(builder, index, &types[0], span)?;
    let receiver = list_base_place(builder, destination, span)?;
    insert_collection(builder, &receiver, vec![key, argument], span)?;
    persist_static_value(builder, global, receiver, span)
}

/// Commit an evaluated primitive or collection replacement to its exact canonical storage slot.
pub fn persist_static_value(
    builder: &mut Builder,
    global: &GlobalPlace,
    target: Place,
    span: &SourceSpan,
) -> Result<(), String> {
    let slot = static_slot(&builder.program.statics, global)?;
    if has_native_static_cell(&builder.program.statics[slot]) {
        return store_computed_static(builder, global, slot, target, span);
    }
    let owner_module = static_owner(&builder.program.statics[slot]);
    initialize_module_statics(builder, &owner_module, span);
    let ty = model_type(builder.constructors(), builder.nominals(), &global.ty, "")?;
    if place_type(&builder.locals, &target) != ty {
        return Err("invalid Body IR static replacement carrier".into());
    }
    let (_, write_cell) = static_accessors(&ty)?;
    let output = temporary(builder, PlanType::Unit, span);
    let moved = ty == PlanType::String || is_owned_collection(&ty);
    external_call(
        builder,
        write_cell,
        vec![PlanType::Int, ty],
        PlanType::Unit,
        vec![
            literal(Constant::Int(slot as i64), span),
            read_place(target, moved, span),
        ],
        output,
        span,
    );
    Ok(())
}

/// Replace a native guarded static's value after evaluation; only computed storage enters module initialization
/// before the write (RFC 052).
fn store_computed_static(
    builder: &mut Builder,
    global: &GlobalPlace,
    slot: usize,
    target: Place,
    span: &SourceSpan,
) -> Result<(), String> {
    let ty = static_carrier(builder, global)?;
    if place_type(&builder.locals, &target) != ty {
        return Err("unsupported Body IR computed static replacement carrier".into());
    }
    if is_computed_static(&builder.program.statics[slot]) {
        let owner_module = static_owner(&builder.program.statics[slot]);
        initialize_module_statics(builder, &owner_module, span);
    }
    let output = temporary(builder, PlanType::Unit, span);
    let moved = snapshot_owns(&ty);
    terminate_call(
        builder,
        CalleeKind::StaticStore(slot as i64),
        vec![read_place(target, moved, span)],
        output,
        span,
    );
    Ok(())
}
