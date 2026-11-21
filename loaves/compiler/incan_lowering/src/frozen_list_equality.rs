//! Compare frozen and owned list elements through shared content views, without changing storage lifetimes.

use incan_mir_plan_native::{
    BinaryOp, Constant, ListLeaf, Operand, Place, PlanType, RvalueKind, SourceSpan, TerminatorKind, UnaryOp,
    assign as plan_assign,
};

use crate::builtins::list_call;
use crate::calls::instantiated_external_call;
use crate::element_borrows::shared_list_element;
use crate::frame::{Builder, literal, place_type, read_place, temporary};
use crate::list_values::list_borrow;
use crate::lists::{list_element, list_shape, require_list_shape};
use crate::optional_text_equality::text_argument;
use crate::pending::untranslated;

/// Recognize equal-depth lists whose terminal values are the same text or byte family.
pub fn supports_frozen_list_equality(left: &PlanType, right: &PlanType) -> bool {
    if left == right {
        return false;
    }
    let (Some((first, first_depth)), Some((second, second_depth))) = (list_shape(left), list_shape(right)) else {
        return false;
    };
    if first_depth == second_depth && first_depth > 0 {
        return matches!(
            (&first, &second),
            (ListLeaf::FrozenStr, ListLeaf::Str) | (ListLeaf::Str, ListLeaf::FrozenStr)
        );
    }
    (first == ListLeaf::FrozenBytes && second == ListLeaf::U8 && second_depth == first_depth + 1)
        || (first == ListLeaf::U8 && second == ListLeaf::FrozenBytes && first_depth == second_depth + 1)
}

/// Compare lengths and borrowed elements, stopping at the first mismatch and retaining both owners.
pub fn frozen_list_equality(
    builder: &mut Builder,
    left: Place,
    right: Place,
    negate: bool,
    span: &SourceSpan,
) -> Result<Operand, String> {
    let output = compare_lists(builder, left, right, span)?;
    if !negate {
        return Ok(output);
    }
    let negated = temporary(builder, PlanType::Bool, span);
    builder.emit(plan_assign(
        negated.clone(),
        RvalueKind::Unary(UnaryOp::Not, output),
        span,
    ));
    Ok(read_place(negated, false, span))
}

/// Emit a length-checked loop; recursive list shapes repeat this borrowed comparison at each depth.
fn compare_lists(builder: &mut Builder, left: Place, right: Place, span: &SourceSpan) -> Result<Operand, String> {
    let output = temporary(builder, PlanType::Bool, span);
    let index = temporary(builder, PlanType::Int, span);
    builder.emit(plan_assign(
        output.clone(),
        RvalueKind::Use(literal(Constant::Bool(false), span)),
        span,
    ));
    builder.emit(plan_assign(
        index.clone(),
        RvalueKind::Use(literal(Constant::Int(0), span)),
        span,
    ));
    let left_length = list_length(builder, left.clone(), span)?;
    let right_length = list_length(builder, right.clone(), span)?;
    let condition = temporary(builder, PlanType::Bool, span);
    builder.emit(plan_assign(
        condition.clone(),
        RvalueKind::Binary(BinaryOp::Equal, left_length.clone(), right_length),
        span,
    ));
    let head = builder.new_block(span);
    let body = builder.new_block(span);
    let advance = builder.new_block(span);
    let equal = builder.new_block(span);
    let joined = builder.new_block(span);
    builder.finish(
        TerminatorKind::SwitchBool(read_place(condition.clone(), false, span), joined, head),
        span,
    );
    builder.select(head);
    builder.emit(plan_assign(
        condition.clone(),
        RvalueKind::Binary(BinaryOp::Equal, read_place(index.clone(), false, span), left_length),
        span,
    ));
    builder.finish(
        TerminatorKind::SwitchBool(read_place(condition, false, span), body, equal),
        span,
    );
    builder.select(body);
    let first = shared_list_element(builder, left, read_place(index.clone(), false, span), span)?;
    let second = shared_list_element(builder, right, read_place(index.clone(), false, span), span)?;
    let same = compare_elements(builder, first, second, span)?;
    builder.finish(TerminatorKind::SwitchBool(same, joined, advance), span);
    builder.select(advance);
    builder.emit(plan_assign(
        index.clone(),
        RvalueKind::Binary(
            BinaryOp::Add,
            read_place(index, false, span),
            literal(Constant::Int(1), span),
        ),
        span,
    ));
    builder.finish(TerminatorKind::Goto(head), span);
    builder.select(equal);
    builder.emit(plan_assign(
        output.clone(),
        RvalueKind::Use(literal(Constant::Bool(true), span)),
        span,
    ));
    builder.finish(TerminatorKind::Goto(joined), span);
    builder.select(joined);
    Ok(read_place(output, false, span))
}

/// Read an admitted list's length through its shared runtime loan.
fn list_length(builder: &mut Builder, value: Place, span: &SourceSpan) -> Result<Operand, String> {
    let (leaf, depth) = require_list_shape(
        &place_type(&builder.locals, &value),
        "invalid frozen list equality owner",
    )?;
    let reference = list_borrow(builder, value, false, span)?;
    let output = temporary(builder, PlanType::Int, span);
    list_call(
        builder,
        "length_list",
        list_element(&leaf, depth),
        vec![PlanType::ListRef(leaf, depth)],
        PlanType::Int,
        vec![reference],
        output.clone(),
        span,
    );
    Ok(read_place(output, false, span))
}

/// Compare a terminal text/bytes pair, or recurse while both sides still contain lists.
fn compare_elements(builder: &mut Builder, left: Place, right: Place, span: &SourceSpan) -> Result<Operand, String> {
    let left_type = place_type(&builder.locals, &left);
    let right_type = place_type(&builder.locals, &right);
    if supports_frozen_list_equality(&left_type, &right_type) {
        return compare_lists(builder, left, right, span);
    }
    let output = temporary(builder, PlanType::Bool, span);
    let text = |ty: &PlanType| matches!(ty, PlanType::String | PlanType::FrozenStr);
    if text(&left_type) && text(&right_type) {
        let (first, first_type) = text_argument(builder, left, &left_type, span);
        let (second, second_type) = text_argument(builder, right, &right_type, span);
        instantiated_external_call(
            builder,
            "incan_std_primitives::strings::str_eq",
            vec![first_type.clone(), second_type.clone()],
            vec![first_type, second_type],
            PlanType::Bool,
            vec![first, second],
            output.clone(),
            span,
        )?;
    } else {
        let first = bytes_view(builder, left, &left_type, span)?;
        let second = bytes_view(builder, right, &right_type, span)?;
        let slice_type = PlanType::NativeSlice(vec![PlanType::U8]);
        let reference_type = PlanType::SliceRef(vec![PlanType::U8]);
        instantiated_external_call(
            builder,
            "core::cmp::PartialEq::eq",
            vec![slice_type.clone(), slice_type],
            vec![reference_type.clone(), reference_type],
            PlanType::Bool,
            vec![first, second],
            output.clone(),
            span,
        )?;
    }
    Ok(read_place(output, false, span))
}

/// Lend either a static frozen byte payload or owned byte vector as the same unsigned byte slice.
///
/// An owned byte vector's slice view is `operand_borrows::list_slice_view`, which refuses by name until it is in Rust.
fn bytes_view(builder: &mut Builder, value: Place, ty: &PlanType, span: &SourceSpan) -> Result<Operand, String> {
    if *ty == PlanType::FrozenBytes {
        let reference_type = PlanType::ScalarRef(vec![ty.clone()]);
        let reference = temporary(builder, reference_type.clone(), span);
        builder.emit(plan_assign(reference.clone(), RvalueKind::Borrow(value), span));
        let output = temporary(builder, PlanType::SliceRef(vec![PlanType::U8]), span);
        instantiated_external_call(
            builder,
            "incan_std_primitives::frozen::FrozenBytes::as_slice",
            Vec::new(),
            vec![reference_type],
            PlanType::SliceRef(vec![PlanType::U8]),
            vec![read_place(reference, false, span)],
            output.clone(),
            span,
        )?;
        return Ok(read_place(output, false, span));
    }
    if list_shape(ty) != Some((ListLeaf::U8, 1)) {
        return Err("invalid frozen list equality byte payload".into());
    }
    list_borrow(builder, value, false, span)?;
    Err(untranslated("operand_borrows::list_slice_view"))
}
