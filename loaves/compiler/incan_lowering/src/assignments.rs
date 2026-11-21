//! Lower assignments to locals, stored places, and elements, including checked integer updates.

use incan_mir_plan_native::{
    BinaryOp, Local, Place, PlanType, Projection, RvalueKind, SourceSpan, TerminatorKind, UnaryOp, Unwind,
    assign as plan_assign, place as plan_place, sized_plan_type,
};
use incan_semantics_core::body_ir::{
    AggregateKind, BinOp, BodyIrModule, Constant as SourceConstant, Operand as SourceOperand, OwnershipFact,
    Place as SourcePlace, Rvalue,
};

use crate::async_::is_future;
use crate::builtin_calls::{lower_boolean_operator, lower_numeric_helper, validate_exact_float};
use crate::carrier_operands::boundary_plan_operand;
use crate::checked_values::scalar_place;
use crate::closures::callable_copy_type;
use crate::collections::is_collection;
use crate::element_places::is_element_place;
use crate::formatting::{lower_format, lower_string_assignment};
use crate::frame::{Builder, cleanup_exit, literal, place_type, read_place, track_rvalue_moves, value_type};
use crate::list_values::list_base_place;
use crate::lists::is_list;
use crate::matches::lower_enum_match;
use crate::model_lists::structural_prefix;
use crate::numerics::{integer_carrier, numeric_carrier, numeric_zero, sized_numeric};
use crate::operators::scalar_rvalue;
use crate::options::replace_owned_destination;
use crate::pending::untranslated;
use crate::result_carriers::{
    lower_none_assignment, lower_option_construction, lower_result_construction, retained_reference_value,
};
use crate::statements::{is_nominal_type, is_owned_callable};

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
            return Err(untranslated("assignments::element_assignment"));
        }
        let base = list_base_place(builder, destination, span)?;
        let ty = builder.locals[base.local as usize].ty.clone();
        if is_collection(&ty) {
            return Err(untranslated("hashed_collections::dictionary_index_assignment"));
        }
        if is_list(&ty) {
            return Err(untranslated("list_methods::indexed_assignment"));
        }
        // A list held by a model field or tuple element is written at its list indexes after that structural prefix.
        if let Some((count, _)) = structural_prefix(builder, destination)?
            && count > 0
            && count < destination.projection.len()
        {
            return Err(untranslated("list_methods::indexed_assignment_at"));
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
    inherit_borrowed_item(builder, &target, value)
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

/// A list bound out of a snapshot iteration item is part of that snapshot, so it stays unchangeable too.
fn inherit_borrowed_item(builder: &mut Builder, target: &Place, value: &Rvalue) -> Result<(), String> {
    if target.projection != Projection::Whole || !is_list(&place_type(&builder.locals, target)) {
        return Ok(());
    }
    if let Rvalue::Use(SourceOperand::Place(read)) = value
        && !read.place.projection.is_empty()
        && let Some(local) = read.place.local_id()
        && builder
            .borrowed_items
            .contains(&(local.index() as i64 + 1 + builder.local_offset))
    {
        return Err(untranslated("list_iteration::borrow_iteration_item"));
    }
    Ok(())
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
        Rvalue::Closure { body, .. } => {
            if body.function_item || body.original_callable.is_some() {
                return Err(untranslated("closures::source_function_item"));
            }
            return Err(untranslated("closure_lifting::lower_closure"));
        }
        Rvalue::Generator { .. } => return Err(untranslated("closure_lifting::lower_generator_expression")),
        Rvalue::Use(_) => {
            let ty = place_type(&builder.locals, &target);
            if callable_copy_type(&ty) || is_owned_callable(&ty) {
                return Err(untranslated("closure_lifting::stored_callable"));
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
        PlanType::ListIntoIter(..) => return Err(untranslated("owned_lists::initialize_owned_list_iterator")),
        PlanType::ZipIteratorMutRef(..) => return Err(untranslated("tuple_iterators::initialize_zip_iterator")),
        PlanType::GeneratorMutRef(..) => return Err(untranslated("generators::generator_reference")),
        PlanType::SetRef(_) | PlanType::DictRef(..) | PlanType::ListRef(..) => {
            return Err(untranslated("list_iteration::iterator_assignment"));
        }
        PlanType::FrozenCollection(name, _) => {
            if name == "FrozenList"
                && let Rvalue::Aggregate(AggregateKind::List, _) = value
            {
                return Err(untranslated("list_values::frozen_list_assignment"));
            }
        }
        _ => {}
    }
    let ty = place_type(&builder.locals, &target);
    if is_collection(&ty) {
        return Err(untranslated("hashed_collections::collection_assignment"));
    }
    if is_list(&ty) {
        return Err(untranslated("list_values::list_assignment"));
    }
    if matches!(ty, PlanType::String | PlanType::StringRef) && builder.iteration_locals.contains(&target.local) {
        return Err(untranslated("list_iteration::initialize_text_iterator"));
    }
    if ty == PlanType::String {
        return lower_string_assignment(builder, target, value, span);
    }
    if ty == PlanType::FrozenStr || ty == PlanType::FrozenBytes {
        return Err(untranslated("frozen_plan::frozen_assignment"));
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
            BinOp::Pow => return Err(untranslated("sequence_builtins::lower_power_operator")),
            BinOp::And => return lower_boolean_operator(builder, target, left, right, "boolean_and", span),
            BinOp::Or => return lower_boolean_operator(builder, target, left, right, "boolean_or", span),
            BinOp::FloorDiv => return lower_numeric_helper(builder, target, left, right, "py_floor_div", span),
            BinOp::Mod => return lower_numeric_helper(builder, target, left, right, "py_mod", span),
            _ => {
                if compares_models(builder, left, span)? {
                    return Err(untranslated("model_values::compare_models"));
                }
            }
        },
        _ => {}
    }
    store_operation(module, builder, target, value, span)
}

/// Whether the left operand is a whole or projected model, which compares through its native or checked methods.
fn compares_models(builder: &mut Builder, left: &SourceOperand, span: &SourceSpan) -> Result<bool, String> {
    let SourceOperand::Place(read) = left else {
        return Ok(false);
    };
    if is_element_place(builder, &read.place)? {
        return Err(untranslated("element_places::element_place_type"));
    }
    if read.place.local_id().is_none() {
        return Ok(false);
    }
    let place = scalar_place(builder, &read.place, span)?;
    Ok(matches!(place_type(&builder.locals, &place), PlanType::Model(..)))
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
