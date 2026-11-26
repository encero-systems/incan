//! Compare optional owned and frozen text without changing either payload's storage or lifetime.

use incan_mir_plan_native::{
    BinaryOp, Carrier, FieldProjection, Operand, Place, PlanType, RvalueKind, SourceSpan, TerminatorKind, UnaryOp,
    assign as plan_assign, carrier_position,
};

use crate::calls::instantiated_external_call;
use crate::frame::{Builder, appended_field, place_type, read_place, temporary};
use crate::options::variant_condition;

/// Compare checked Option text carriers by tag and then by their borrowed text contents.
///
/// Only differing owned/frozen text layouts enter here. Matching carriers retain their native PartialEq path. Neither
/// payload is moved, copied into owned text, or converted into program-lifetime frozen storage.
pub fn optional_text_equality(
    builder: &mut Builder,
    left: &Place,
    right: &Place,
    negate: bool,
    span: &SourceSpan,
) -> Result<Option<Operand>, String> {
    let left_type = place_type(&builder.locals, left);
    let right_type = place_type(&builder.locals, right);
    let left_payload = optional_text_payload(builder, &left_type);
    let right_payload = optional_text_payload(builder, &right_type);
    if left_type == right_type || left_payload.is_none() || right_payload.is_none() {
        return Ok(None);
    }
    let (Some(first), Some(second)) = (left_payload, right_payload) else {
        return Err("invalid Body IR optional text payload classification".into());
    };
    // Inspect both variants before projecting either payload; None never exposes a field.
    let output = temporary(builder, PlanType::Bool, span);
    let left_present = variant_condition(builder, left, 1, span);
    let right_present = variant_condition(builder, right, 1, span);
    builder.emit(plan_assign(
        output.clone(),
        RvalueKind::Binary(BinaryOp::Equal, left_present.clone(), right_present.clone()),
        span,
    ));
    let check_payload = builder.new_block(span);
    let compare_payload = builder.new_block(span);
    let joined = builder.new_block(span);
    builder.finish(TerminatorKind::SwitchBool(left_present, joined, check_payload), span);
    builder.select(check_payload);
    builder.finish(TerminatorKind::SwitchBool(right_present, joined, compare_payload), span);
    builder.select(compare_payload);
    // Existing generic string equality accepts both a copyable frozen wrapper and a borrowed String.
    let mut first_projection = FieldProjection::new(0, first.clone());
    first_projection.variant = 1;
    let mut second_projection = FieldProjection::new(0, second.clone());
    second_projection.variant = 1;
    let first_field = appended_field(left, first_projection, span)?;
    let second_field = appended_field(right, second_projection, span)?;
    let (first_argument, first_type) = text_argument(builder, first_field, &first, span);
    let (second_argument, second_type) = text_argument(builder, second_field, &second, span);
    instantiated_external_call(
        builder,
        "incan_std_primitives::strings::str_eq",
        vec![first_type.clone(), second_type.clone()],
        vec![first_type, second_type],
        PlanType::Bool,
        vec![first_argument, second_argument],
        output.clone(),
        span,
    )?;
    builder.finish(TerminatorKind::Goto(joined), span);
    builder.select(joined);
    if negate {
        let negated = temporary(builder, PlanType::Bool, span);
        builder.emit(plan_assign(
            negated.clone(),
            RvalueKind::Unary(UnaryOp::Not, read_place(output, false, span)),
            span,
        ));
        return Ok(Some(read_place(negated, false, span)));
    }
    Ok(Some(read_place(output, false, span)))
}

/// Admit only a retained intrinsic Option with one owned or frozen text payload.
fn optional_text_payload(builder: &Builder, ty: &PlanType) -> Option<PlanType> {
    let PlanType::Enum(owner, _) = ty else {
        return None;
    };
    if *owner < builder.enums().len() as i64 {
        return None;
    }
    let declaration = &builder.intrinsics[carrier_position(*owner) as usize];
    if declaration.carrier != Carrier::Option || declaration.variants[1].fields.len() != 1 {
        return None;
    }
    let payload = &declaration.variants[1].fields[0];
    if matches!(payload, PlanType::String | PlanType::FrozenStr) {
        return Some(payload.clone());
    }
    None
}

/// Copy a frozen wrapper or lend the original owned String, retaining the complete field projection.
pub fn text_argument(builder: &mut Builder, field: Place, ty: &PlanType, span: &SourceSpan) -> (Operand, PlanType) {
    if *ty == PlanType::FrozenStr {
        return (read_place(field, false, span), ty.clone());
    }
    let reference = temporary(builder, PlanType::StringRef, span);
    builder.emit(plan_assign(reference.clone(), RvalueKind::Borrow(field), span));
    (read_place(reference, false, span), PlanType::StringRef)
}
