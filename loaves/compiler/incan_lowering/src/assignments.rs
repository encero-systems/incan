//! Lower assignments to locals, stored places, and elements, including checked integer updates.

use incan_mir_plan_native::{
    BinaryOp, Local, Operand, Place, PlanType, Projection, RvalueKind, SourceSpan, TerminatorKind, UnaryOp, Unwind,
    assign as plan_assign, place as plan_place, sized_plan_type,
};
use incan_semantics_core::body_ir::{
    AggregateKind, BinOp, BodyIrModule, Constant as SourceConstant, Operand as SourceOperand, OwnershipFact,
    Place as SourcePlace, PlaceElem, Rvalue,
};

use crate::async_::is_future;
use crate::builtin_calls::{lower_boolean_operator, lower_numeric_helper, validate_exact_float};
use crate::builtins::list_call;
use crate::carrier_operands::{boundary_plan_operand, element_operand, element_plan_operand};
use crate::checked_values::{scalar_operand, scalar_place};
use crate::closure_lifting::{
    ClosureExpression, assign_callable, lower_closure, lower_generator_expression, move_closure, stored_callable,
};
use crate::closures::callable_copy_type;
use crate::collections::{collection_arguments, insert_collection, is_collection, is_owned_collection};
use crate::element_borrows::{mutable_element_place, mutable_element_prefix};
use crate::element_places::{element_place_type, element_prefix_type, is_element_place};
use crate::explicit_arguments::reference_contract;
use crate::formatting::{lower_format, lower_string_assignment};
use crate::frame::{
    Builder, cleanup_exit, dereferenced_place, literal, place_type, read_place, temporary, track_rvalue_moves,
    value_type,
};
use crate::frozen_plan::frozen_bytes_literal;
use crate::generators::generator_reference;
use crate::hashed_collections::{collection_assignment, dictionary_entry_assignment, dictionary_index_assignment};
use crate::list_iteration::{
    inherit_borrowed_item, initialize_text_iterator, iterator_assignment, list_root_place, operand_list_place,
};
use crate::list_methods::{indexed_assignment, indexed_assignment_at};
use crate::list_values::{frozen_list_assignment, list_assignment, list_base_place, list_borrow};
use crate::lists::{is_exclusive_borrow, is_list, is_owned_list, list_element, require_list_shape};
use crate::matches::lower_enum_match;
use crate::model_lists::{prefix_place, structural_prefix};
use crate::model_values::compare_models;
use crate::numerics::{integer_carrier, numeric_carrier, numeric_zero, sized_numeric};
use crate::operators::scalar_rvalue;
use crate::options::replace_owned_destination;
use crate::owned_lists::initialize_owned_list_iterator;
use crate::pending::untranslated;
use crate::result_carriers::{
    frozen_operand, lower_none_assignment, lower_option_construction, lower_result_construction,
    retained_reference_value,
};
use crate::sequence_builtins::lower_power_operator;
use crate::statements::{is_nominal_type, is_owned_callable};
use crate::tuple_iterators::initialize_zip_iterator;

/// Assign a checked value to a local, routing strings, formatting and runtime-semantics operators first.
pub fn lower_assignment(
    module: &BodyIrModule,
    builder: &mut Builder,
    destination: &SourcePlace,
    value: &Rvalue,
    span: &SourceSpan,
) -> Result<(), String> {
    if destination.global().is_some() {
        return Err(untranslated("static_storage::write_static"));
    }
    if !destination.projection.is_empty() {
        if is_element_place(builder, destination)? {
            return element_assignment(module, builder, destination, value, span);
        }
        let base = list_base_place(builder, destination, span)?;
        let ty = builder.locals[base.local as usize].ty.clone();
        if is_collection(&ty) {
            return dictionary_index_assignment(builder, destination, value, span);
        }
        if is_list(&ty) {
            return indexed_assignment(builder, destination, value, span);
        }
        // A list held by a model field or tuple element is written at its list indexes after that structural prefix.
        if let Some((count, _)) = structural_prefix(builder, destination)?
            && count > 0
            && count < destination.projection.len()
        {
            let receiver = prefix_place(builder, destination, count, span)?;
            if is_collection(&place_type(&builder.locals, &receiver)) {
                return dictionary_entry_assignment(builder, destination, receiver, count, value, span);
            }
            return indexed_assignment_at(builder, destination, count, value, span);
        }
    }
    if destination.projection.is_empty()
        && let Rvalue::Use(operand) = value
    {
        let root = list_base_place(builder, destination, span)?;
        if root.local > builder.parameter_count && !builder.iteration_locals.contains(&root.local) {
            let expected = place_type(&builder.locals, &root);
            if let Some(reference) = retained_reference_value(builder, operand, &expected, span)? {
                builder.emit(plan_assign(root, RvalueKind::Use(reference), span));
                return Ok(());
            }
        }
    }
    let target = assignment_place(builder, destination, span)?;
    if let Rvalue::Match { scrutinee, arms } = value {
        return lower_enum_match(module, builder, target, scrutinee, arms, span);
    }
    lower_local_assignment(module, builder, target.clone(), value, span)?;
    inherit_borrowed_item(builder, &target, value);
    Ok(())
}

/// Replace a checked reference value's own slot; source receiver and projected writes still address their referents.
pub fn assignment_place(builder: &mut Builder, destination: &SourcePlace, span: &SourceSpan) -> Result<Place, String> {
    if destination.projection.is_empty()
        && let Some(local) = destination.local_id()
    {
        let index = local.index() as i64 + 1 + builder.local_offset;
        if builder.reference_values.contains(&index) {
            return Ok(plan_place(index, span));
        }
    }
    scalar_place(builder, destination, span)
}

/// Handle callable and iteration bindings before assigning an ordinary stored value with checked ownership.
pub fn lower_local_assignment(
    module: &BodyIrModule,
    builder: &mut Builder,
    target: Place,
    value: &Rvalue,
    span: &SourceSpan,
) -> Result<(), String> {
    // Replace the borrowed value through its checked projection, never the reference slot itself.
    if builder.locals[target.local as usize].ty == PlanType::StringMutRef
        && target.projection != Projection::Deref(PlanType::String)
    {
        return Err("unsupported Body IR assignment to a mutable text reference slot".into());
    }
    if builder.borrowed_items.contains(&target.local) {
        return Err("unsupported Body IR mutation of borrowed list iteration item".into());
    }
    if in_place_iteration_binding(builder, &target, value)? {
        return Ok(());
    }
    match value {
        Rvalue::Closure {
            params,
            captured_operands,
            body,
        } => {
            if body.function_item || body.original_callable.is_some() {
                return Err(untranslated("closures::source_function_item"));
            }
            let closure = ClosureExpression {
                parameters: params,
                captures: captured_operands,
                capture_locals: &body.capture_locals,
                statements: &body.stmts,
                result: &body.result,
                consumes_environment: body.consumes_environment,
                native_storage: &body.native_storage,
            };
            return lower_closure(module, builder, target, &closure, span);
        }
        Rvalue::Generator {
            source,
            captured_operands,
            body,
        } => return lower_generator_expression(module, builder, target, source, captured_operands, body, span),
        Rvalue::Use(operand) => {
            let ty = place_type(&builder.locals, &target);
            if callable_copy_type(&ty) || is_owned_callable(&ty) {
                let callable = stored_callable(builder, operand, span)?;
                return match value_type(builder, &callable)? {
                    PlanType::NativeClosure(signature, send, sync) => {
                        let local = target.local;
                        move_closure(builder, target, callable, signature.clone(), false, span, false)?;
                        builder.locals[local as usize].ty = PlanType::NativeClosure(signature, send, sync);
                        Ok(())
                    }
                    PlanType::Closure(signature) => {
                        move_closure(builder, target, callable, signature, false, span, false)
                    }
                    PlanType::ClosureOnce(signature) => {
                        move_closure(builder, target, callable, signature, true, span, false)
                    }
                    _ => {
                        let pointer_storage = builder.function_pointer_locals.contains(&target.local);
                        assign_callable(builder, target, callable, pointer_storage, span)
                    }
                };
            }
        }
        _ => {}
    }
    assign_stored_value(module, builder, target, value, span)
}

/// Assign an ordinary stored value after callable construction and iteration-binding admission.
fn assign_stored_value(
    module: &BodyIrModule,
    builder: &mut Builder,
    target: Place,
    value: &Rvalue,
    span: &SourceSpan,
) -> Result<(), String> {
    match &builder.locals[target.local as usize].ty {
        PlanType::ListIntoIter(..) => {
            let Rvalue::Use(SourceOperand::Place(read)) = value else {
                return Err("unsupported Body IR consuming iterator initialization".into());
            };
            if read.fact != OwnershipFact::Move {
                return Err("unsupported Body IR consuming iterator without retained taking".into());
            }
            let source = scalar_place(builder, &read.place, span)?;
            return initialize_owned_list_iterator(builder, &target, &source, span);
        }
        PlanType::ZipIteratorMutRef(..) => {
            let Rvalue::Use(SourceOperand::Place(read)) = value else {
                return Err("unsupported Body IR zip iterator initialization".into());
            };
            if read.fact != OwnershipFact::Borrow {
                return Err("unsupported Body IR zip iteration source ownership".into());
            }
            let source = list_root_place(builder, &read.place, span)?;
            return initialize_zip_iterator(builder, target, source, span);
        }
        PlanType::GeneratorMutRef(..) => {
            let Rvalue::Use(value) = value else {
                return Err("unsupported Body IR generator iterator initialization".into());
            };
            let source = operand_list_place(builder, value, span)?;
            let reference = generator_reference(builder, source, span)?;
            builder.emit(plan_assign(target, RvalueKind::Use(reference), span));
            return Ok(());
        }
        PlanType::SetRef(_) | PlanType::DictRef(..) | PlanType::ListRef(..) => {
            return iterator_assignment(builder, target, value, span);
        }
        PlanType::FrozenCollection(name, _) => {
            if name == "FrozenList"
                && let Rvalue::Aggregate(AggregateKind::List, _) = value
            {
                return frozen_list_assignment(builder, target, value, span);
            }
        }
        _ => {}
    }
    let ty = place_type(&builder.locals, &target);
    if is_collection(&ty) {
        return collection_assignment(builder, target, value, span);
    }
    if is_list(&ty) {
        return list_assignment(builder, target, value, span);
    }
    if matches!(ty, PlanType::String | PlanType::StringRef) && builder.iteration_locals.contains(&target.local) {
        return initialize_text_iterator(builder, target, value, span);
    }
    if ty == PlanType::String {
        return lower_string_assignment(builder, target, value, span);
    }
    if ty == PlanType::FrozenStr {
        let Rvalue::Use(operand) = value else {
            return Err("unsupported Body IR nonliteral FrozenStr assignment".into());
        };
        let converted = frozen_operand(builder, operand, span)?;
        builder.emit(plan_assign(target, RvalueKind::Use(converted), span));
        return Ok(());
    }
    if ty == PlanType::FrozenBytes {
        let converted = match value {
            Rvalue::Use(SourceOperand::Constant(constant)) => frozen_bytes_literal(builder, constant, span)?,
            Rvalue::Use(operand) => {
                let converted = scalar_operand(builder, operand, span)?;
                if value_type(builder, &converted)? != PlanType::FrozenBytes {
                    return Err("unsupported Body IR dynamic FrozenBytes assignment".into());
                }
                converted
            }
            _ => return Err("unsupported Body IR nonliteral FrozenBytes assignment".into()),
        };
        builder.emit(plan_assign(target, RvalueKind::Use(converted), span));
        return Ok(());
    }
    match value {
        Rvalue::ResultVariant(variant) => return lower_result_construction(builder, target, variant, span),
        Rvalue::Aggregate(AggregateKind::OptionSome, elements) => {
            return lower_option_construction(builder, target, elements, span);
        }
        Rvalue::Use(SourceOperand::Constant(SourceConstant::None)) => {
            return lower_none_assignment(builder, target, span);
        }
        Rvalue::Format(parts) => return lower_format(builder, target, parts, span),
        Rvalue::BinaryOp(operator, left, right) => match operator {
            BinOp::Pow => return lower_power_operator(builder, target, left, right, span),
            BinOp::And => return lower_boolean_operator(builder, target, left, right, "boolean_and", span),
            BinOp::Or => return lower_boolean_operator(builder, target, left, right, "boolean_or", span),
            BinOp::FloorDiv => return lower_numeric_helper(builder, target, left, right, "py_floor_div", span),
            BinOp::Mod => return lower_numeric_helper(builder, target, left, right, "py_mod", span),
            _ => {
                if compare_models(builder, target.clone(), *operator, left, right, span)? {
                    return Ok(());
                }
            }
        },
        _ => {}
    }
    store_operation(module, builder, target, value, span)
}

/// Store a checked scalar, carrier or nominal operation at the destination's exact carrier.
fn store_operation(
    module: &BodyIrModule,
    builder: &mut Builder,
    target: Place,
    value: &Rvalue,
    span: &SourceSpan,
) -> Result<(), String> {
    let ty = place_type(&builder.locals, &target);
    let mut operation = scalar_rvalue(module, builder, value, &ty, span)?;
    if let RvalueKind::Use(operand) = operation {
        if numeric_carrier(&ty) {
            let source_type = value_type(builder, &operand)?;
            operation = if source_type != ty {
                if !numeric_carrier(&source_type) {
                    return Err("unsupported Body IR nonnumeric implicit conversion".into());
                }
                RvalueKind::NumericCast(operand, source_type, ty.clone())
            } else {
                RvalueKind::Use(operand)
            };
        } else {
            operation = RvalueKind::Use(boundary_plan_operand(builder, operand, &ty, span)?);
        }
    }
    track_rvalue_moves(builder, &operation);
    if integer_carrier(&ty) {
        operation = negation_as_subtraction(operation, &ty, span)?;
        if builder.program.overflow_checks && is_checked_int_operation(&operation) {
            return lower_checked_int_assignment(builder, target, operation, span);
        }
    }
    if is_nominal_type(&ty) || is_future(&ty) {
        replace_owned_destination(builder, target, operation, span);
        return Ok(());
    }
    builder.emit(plan_assign(target.clone(), operation, span));
    validate_exact_float(builder, &target, span);
    Ok(())
}

/// Consume a checked exclusive pattern binding as a view of its polled element. Only a whole binding can change its
/// frame carrier; a later field write must keep the owner and its earlier assignments intact.
fn in_place_iteration_binding(builder: &mut Builder, destination: &Place, value: &Rvalue) -> Result<bool, String> {
    if destination.projection != Projection::Whole || builder.iteration_locals.contains(&destination.local) {
        return Ok(false);
    }
    match value {
        Rvalue::Use(SourceOperand::Place(read)) if read.fact == OwnershipFact::MutBorrow => {
            Err(untranslated("assignments::in_place_iteration_binding"))
        }
        _ => Ok(false),
    }
}

/// Rewrite integer negation with a zero of its exact carrier so the unit's subtraction policy applies.
fn negation_as_subtraction(operation: RvalueKind, ty: &PlanType, span: &SourceSpan) -> Result<RvalueKind, String> {
    match operation {
        RvalueKind::Unary(UnaryOp::Negate, operand) => Ok(RvalueKind::Binary(
            BinaryOp::Subtract,
            literal(numeric_zero(ty)?, span),
            operand,
        )),
        _ => Ok(operation),
    }
}

/// Identify addition, subtraction and multiplication, whose overflow follows the unit's selected policy.
fn is_checked_int_operation(operation: &RvalueKind) -> bool {
    matches!(
        operation,
        RvalueKind::Binary(BinaryOp::Add | BinaryOp::Subtract | BinaryOp::Multiply, _, _)
    )
}

/// Lower an overflow-checked integer operation the way rustc's own MIR does.
///
/// The operation writes a (value, overflowed) pair; an overflow assertion branches to a panic (unwinding through the
/// current owners' cleanup) or continues, and only then is the value copied into the destination.
fn lower_checked_int_assignment(
    builder: &mut Builder,
    target: Place,
    operation: RvalueKind,
    span: &SourceSpan,
) -> Result<(), String> {
    let RvalueKind::Binary(operator, left, right) = operation.clone() else {
        return Err("checked integer lowering expects a binary operation".into());
    };
    let pair = builder.locals.len() as i64;
    let ty = place_type(&builder.locals, &target);
    let (pair_type, projection) = match sized_numeric(&ty) {
        Some(kind) => {
            let value = sized_plan_type(&kind);
            (PlanType::CheckedNumeric(kind), Projection::NumericValue(value))
        }
        None => (PlanType::CheckedInt, Projection::Value),
    };
    builder.locals.push(Local {
        ty: pair_type,
        span: span.clone(),
    });
    builder.emit(plan_assign(plan_place(pair, span), operation, span));
    let next_block = builder.new_block(span);
    let flag = read_place(
        Place {
            local: pair,
            projection: Projection::Overflow,
            span: span.clone(),
        },
        false,
        span,
    );
    let cleanup = cleanup_exit(builder, span);
    let assertion = TerminatorKind::AssertOverflow(flag, operator, left, right, next_block, Unwind::Cleanup(cleanup));
    builder.finish(assertion, span);
    builder.select(next_block);
    let result = read_place(
        Place {
            local: pair,
            projection,
            span: span.clone(),
        },
        false,
        span,
    );
    builder.emit(plan_assign(target, RvalueKind::Use(result), span));
    Ok(())
}

/// Evaluate a replacement before reborrowing checked element storage, then write its last index or structural field.
fn element_assignment(
    module: &BodyIrModule,
    builder: &mut Builder,
    destination: &SourcePlace,
    value: &Rvalue,
    span: &SourceSpan,
) -> Result<(), String> {
    let ty = element_place_type(builder, destination)?;
    if is_exclusive_borrow(&ty) {
        return exclusive_view_element_assignment(module, builder, destination, value, &ty, span);
    }
    let replacement = temporary(builder, ty.clone(), span);
    lower_local_assignment(module, builder, replacement.clone(), value, span)?;
    let owned = builder.owned.contains(&replacement.local);
    let argument = element_plan_operand(builder, read_place(replacement, owned, span), &ty, span)?;
    let last = destination.projection.len() - 1;
    let container_type = element_prefix_type(builder, destination, last)?;
    if let PlaceElem::Index(index) = &destination.projection[last] {
        if is_list(&container_type) {
            let subscript = scalar_operand(builder, index, span)?;
            let container = mutable_element_prefix(builder, destination, last, span)?;
            let (leaf, depth) = require_list_shape(&container_type, "invalid Body IR list write container")?;
            let reference = list_borrow(builder, container, true, span)?;
            let output = temporary(builder, PlanType::Unit, span);
            list_call(
                builder,
                "set_list",
                list_element(&leaf, depth),
                vec![PlanType::ListMutRef(leaf, depth), PlanType::Int, ty],
                PlanType::Unit,
                vec![reference, subscript, argument],
                output,
                span,
            );
            return Ok(());
        }
        if is_collection(&container_type) {
            let types = collection_arguments(&container_type);
            if types.len() != 2 {
                return Err("unsupported Body IR set indexing assignment".into());
            }
            let subscript = element_operand(builder, index, &types[0], span)?;
            let container = mutable_element_prefix(builder, destination, last, span)?;
            return insert_collection(builder, &container, vec![subscript, argument], span);
        }
    }
    let target = mutable_element_place(builder, destination, span)?;
    write_replacement(builder, target, &ty, argument, span);
    Ok(())
}

/// Replace an owned destination's old value, or overwrite a plain one.
fn write_replacement(builder: &mut Builder, target: Place, ty: &PlanType, argument: Operand, span: &SourceSpan) {
    let operation = RvalueKind::Use(argument);
    if *ty == PlanType::String || is_owned_list(ty) || is_owned_collection(ty) || is_nominal_type(ty) {
        replace_owned_destination(builder, target, operation, span);
    } else {
        builder.emit(plan_assign(target, operation, span));
    }
}

/// Replace the referent reached through a stored exclusive view without replacing or moving the list's reference slot.
fn exclusive_view_element_assignment(
    module: &BodyIrModule,
    builder: &mut Builder,
    destination: &SourcePlace,
    value: &Rvalue,
    reference_type: &PlanType,
    span: &SourceSpan,
) -> Result<(), String> {
    let (referent, writable) = reference_contract(reference_type)?;
    if !writable {
        return Err("unsupported Body IR write through a shared stored view".into());
    }
    let replacement = temporary(builder, referent.clone(), span);
    lower_local_assignment(module, builder, replacement.clone(), value, span)?;
    let owned = builder.owned.contains(&replacement.local);
    let argument = element_plan_operand(builder, read_place(replacement, owned, span), &referent, span)?;
    let slot_reference = mutable_element_place(builder, destination, span)?;
    let stored_reference = dereferenced_place(&slot_reference, reference_type.clone(), span)?;
    let target = dereferenced_place(&stored_reference, referent.clone(), span)?;
    write_replacement(builder, target, &referent, argument, span);
    Ok(())
}
