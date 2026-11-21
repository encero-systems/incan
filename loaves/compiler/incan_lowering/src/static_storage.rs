//! Computed and literal module statics planned as guarded adapter cells (RFC 052), and their reads.

use incan_mir_plan_native::{
    Callee, CalleeKind, Constant, EnumDeclaration, ExternalFunction, FieldProjection, Function, Local, Operand,
    OperandKind, Parameter, Place, PlanType, Projection, RvalueKind, SourceSpan, StaticCell, TerminatorKind, Unwind,
    assign as plan_assign, block as plan_block, place as plan_place,
};
use incan_semantics_core::IncanType;
use incan_semantics_core::body_ir::{
    AggregateKind, ArgumentElement, BodyIrModule, Constant as SourceConstant, GlobalPlace, Operand as SourceOperand,
    OwnershipFact, Place as SourcePlace, PlaceElem, Rvalue, StaticInitializer,
};

use crate::carrier_operands::element_operand;
use crate::checked_values::{scalar_operand, target_read};
use crate::closures::callable_copy_type;
use crate::collections::{
    begin_collection, collection_arguments, collection_name, collection_reference, finish_collection,
    insert_collection, is_collection, is_owned_collection, owned_runtime_operand,
};
use crate::computed_statics::{has_native_static_cell, initialize_module_statics, is_computed_static, static_owner};
use crate::decls::{static_accessors, static_slot};
use crate::frame::{Builder, external_call, literal, place_type, read_place, temporary, terminate_call};
use crate::hashed_collections::dictionary_value_read;
use crate::layout_types::native_source_type;
use crate::list_values::{list_assignment, projected_element_read, value_operand};
use crate::lists::{is_list, is_owned_list, list_element, require_list_shape};
use crate::lowering::Inventories;
use crate::matches::read_generic_decorator;
use crate::models::{field_path, field_step_type, model_type};
use crate::numerics::numeric_carrier;
use crate::pending::untranslated;
use crate::program_context::ProgramContext;
use crate::statements::is_nominal_type;

/// Plan computed statics and effect-free literal lists or dictionaries as guarded adapter cells.
///
/// A continuation starts at the first newly resolved declaration so preserved authored cells are never lowered twice.
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
    let _ = (program, unions, sources, files, owners, intrinsics, inventories);
    let cells = Vec::new();
    for declaration in module.static_declarations.iter().skip(start_slot) {
        if module
            .decorated_bindings
            .iter()
            .any(|binding| binding.original_body.canonical.as_ref() == Some(&declaration.canonical))
        {
            return Err(untranslated("static_storage::lower_computed_statics original callable"));
        }
        match declaration.initial {
            StaticInitializer::Computed(_) => {
                return Err(untranslated(
                    "static_storage::lower_computed_statics computed initializer",
                ));
            }
            StaticInitializer::List(_) | StaticInitializer::Dict(_) => {
                return Err(untranslated("static_storage::lower_literal_static"));
            }
            _ => {}
        }
    }
    Ok(cells)
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
fn static_carrier(builder: &Builder, global: &GlobalPlace) -> Result<PlanType, String> {
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
/// Selected enum and text fields are cloned under the actual shared guard, without cloning unrelated fields. Other
/// supported reads use an owned snapshot. Only a collection is borrowed mutably, by a method that commits its changed
/// copy back.
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
        if let Some(operand) = read_guarded_static_field(builder, value, slot, &ty, span)? {
            return Ok(operand);
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

/// Clone a selected enum or text field under the actual storage guard, without cloning unrelated object fields.
fn read_guarded_static_field(
    builder: &mut Builder,
    value: &SourcePlace,
    slot: usize,
    owner: &PlanType,
    span: &SourceSpan,
) -> Result<Option<Operand>, String> {
    let PlanType::Model(index, name) = owner else {
        return Ok(None);
    };
    let receiver = PlanType::ModelRef(*index, name.clone());
    let fields = field_path(
        builder.tables(),
        owner.clone(),
        &value.projection,
        value.projection.len(),
    )?;
    let Some(last) = fields.last() else {
        return Ok(None);
    };
    if fields.iter().any(|field| field.variant >= 0) {
        return Ok(None);
    }
    let result = last.ty.clone();
    let (reference, callee) = match &result {
        PlanType::String => {
            let path = "incan_native_runtime::clone_text";
            builder.externals.push(ExternalFunction {
                path: path.into(),
                type_arguments: Vec::new(),
                parameters: vec![PlanType::StringRef],
                return_type: result.clone(),
                span: span.clone(),
            });
            (PlanType::StringRef, CalleeKind::External(path.into()))
        }
        PlanType::Enum(index, name) => {
            let cloned = usize::try_from(*index)
                .ok()
                .and_then(|slot| builder.enums().get(slot))
                .is_some_and(|declaration| declaration.derives.iter().any(|derive| derive == "Clone"));
            if !cloned {
                return Err("unsupported Body IR static enum field without retained Clone".into());
            }
            let reference = PlanType::EnumRef(*index, name.clone());
            let path = "core::clone::Clone::clone";
            builder.externals.push(ExternalFunction {
                path: path.into(),
                type_arguments: vec![result.clone()],
                parameters: vec![reference.clone()],
                return_type: result.clone(),
                span: span.clone(),
            });
            (reference, CalleeKind::Instantiated(path.into(), vec![result.clone()]))
        }
        _ => return Ok(None),
    };
    let name = format!(
        "{}__incan_static_field_{slot}_{}",
        builder.name,
        builder.synthesized.len()
    );
    let reader = StaticReader {
        name: name.clone(),
        receiver,
        result: result.clone(),
        callee,
    };
    builder
        .synthesized
        .push(static_field_reader(reader, fields, reference, span));
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
    builder.owned.push(output.local);
    Ok(Some(read_place(output, true, span)))
}

/// One synthesized StaticMethod accessor: its name, borrowed receiver, result and the call it makes.
struct StaticReader {
    name: String,
    receiver: PlanType,
    result: PlanType,
    callee: CalleeKind,
}

/// Borrow the exact checked field path while the enclosing StaticMethod accessor keeps its shared guard alive.
fn static_field_reader(
    reader: StaticReader,
    fields: Vec<FieldProjection>,
    reference: PlanType,
    span: &SourceSpan,
) -> Function {
    let field = Place {
        local: 1,
        projection: Projection::DerefFields(fields),
        span: span.clone(),
    };
    let borrowed = plan_assign(plan_place(2, span), RvalueKind::Borrow(field), span);
    let invocation = TerminatorKind::Call(
        Callee {
            kind: reader.callee,
            span: span.clone(),
        },
        vec![read_place(plan_place(2, span), false, span)],
        plan_place(0, span),
        1,
        Unwind::Continue,
    );
    let locals = vec![
        Local {
            ty: reader.result.clone(),
            span: span.clone(),
        },
        Local {
            ty: reader.receiver.clone(),
            span: span.clone(),
        },
        Local {
            ty: reference,
            span: span.clone(),
        },
    ];
    Function::new(
        reader.name,
        vec![Parameter {
            name: "__incan_receiver".into(),
            ty: reader.receiver,
            span: span.clone(),
        }],
        reader.result,
        locals,
        vec![
            plan_block(vec![borrowed], invocation, span),
            plan_block(Vec::new(), TerminatorKind::Return, span),
        ],
        span.clone(),
    )
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
/// A keyed dictionary entry is read under the storage guard; any other projection is selected inside a fresh snapshot.
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
