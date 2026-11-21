//! `isinstance` over an Option carrier, as legacy matches it: true only for a present payload of the target type.
//!
//! A payload that is itself a union is tested by its member tag after the Option's own tag proves it present, so the
//! union payload is never inspected through an absent Option.

use incan_mir_plan_native::{
    BinaryOp, Carrier, Constant, FieldProjection, Place, PlanType, RvalueKind, SourceSpan, TerminatorKind,
    assign as plan_assign, carrier_position,
};

use crate::frame::{Builder, appended_field, literal, read_place, temporary};
use crate::unions::{test_union_tag, union_test_slots};

/// Whether a retained enum owner is the intrinsic Option carrier rather than a source enum or a union.
pub fn is_option_carrier(builder: &Builder, owner: i64) -> bool {
    intrinsic_carrier(builder, owner) == Some(Carrier::Option)
}

/// Whether a retained enum owner is an anonymous union, the intrinsic carrier without a standard name.
pub fn is_union_carrier(builder: &Builder, owner: i64) -> bool {
    intrinsic_carrier(builder, owner) == Some(Carrier::Declared)
}

/// The carrier kind of an intrinsic enum owner, or nothing for a source enum.
fn intrinsic_carrier(builder: &Builder, owner: i64) -> Option<Carrier> {
    let index = carrier_position(owner);
    if index < 0 {
        return None;
    }
    builder
        .intrinsics
        .get(index as usize)
        .map(|declaration| declaration.carrier.clone())
}

/// Test a present Option payload against the checked target type without consuming the Option.
pub fn option_isinstance(
    builder: &mut Builder,
    source: Place,
    owner: i64,
    member: &PlanType,
    span: &SourceSpan,
) -> Result<RvalueKind, String> {
    let payload = builder.intrinsics[carrier_position(owner) as usize].variants[1].fields[0].clone();
    if let PlanType::Enum(inner, _) = &payload
        && is_union_carrier(builder, *inner)
    {
        return union_payload_test(builder, source, &payload, *inner, member, span);
    }
    if !same_isinstance_type(&payload, member) {
        return Ok(RvalueKind::Use(literal(Constant::Bool(false), span)));
    }
    let tag = enum_tag(builder, source, span);
    // Option's Some is variant 1, so a present payload of the target type is exactly tag 1.
    Ok(test_union_tag(builder, &tag, &[1], span))
}

/// Test the union member tag only on the branch where the Option's tag proved its payload present.
fn union_payload_test(
    builder: &mut Builder,
    source: Place,
    payload: &PlanType,
    inner: i64,
    member: &PlanType,
    span: &SourceSpan,
) -> Result<RvalueKind, String> {
    let slots = union_test_slots(&builder.unions, inner, member, &builder.intrinsics)?;
    if slots.is_empty() {
        return Ok(RvalueKind::Use(literal(Constant::Bool(false), span)));
    }
    let output = temporary(builder, PlanType::Bool, span);
    builder.emit(plan_assign(
        output.clone(),
        RvalueKind::Use(literal(Constant::Bool(false), span)),
        span,
    ));
    let tag = enum_tag(builder, source.clone(), span);
    let present = temporary(builder, PlanType::Bool, span);
    let compared = RvalueKind::Binary(
        BinaryOp::Equal,
        read_place(tag, false, span),
        literal(Constant::Int(1), span),
    );
    builder.emit(plan_assign(present.clone(), compared, span));
    let inspected = builder.new_block(span);
    let joined = builder.new_block(span);
    builder.finish(
        TerminatorKind::SwitchBool(read_place(present, false, span), joined, inspected),
        span,
    );
    builder.select(inspected);
    let mut projection = FieldProjection::new(0, payload.clone());
    projection.variant = 1;
    let member_place = appended_field(&source, projection, span)?;
    let member_tag = enum_tag(builder, member_place, span);
    let matched = test_union_tag(builder, &member_tag, &slots, span);
    builder.emit(plan_assign(output.clone(), matched, span));
    builder.finish(TerminatorKind::Goto(joined), span);
    builder.select(joined);
    Ok(RvalueKind::Use(read_place(output, false, span)))
}

/// Read an enum's variant index as an int without moving or copying its payload.
pub fn enum_tag(builder: &mut Builder, source: Place, span: &SourceSpan) -> Place {
    let raw_tag = temporary(builder, PlanType::EnumTag, span);
    let tag = temporary(builder, PlanType::Int, span);
    builder.emit(plan_assign(raw_tag.clone(), RvalueKind::Discriminant(source), span));
    builder.emit(plan_assign(
        tag.clone(),
        RvalueKind::TagToInt(read_place(raw_tag, false, span)),
        span,
    ));
    tag
}

/// Match the target exactly, treating owned and frozen text as the one `str` identity as union tests do.
fn same_isinstance_type(payload: &PlanType, member: &PlanType) -> bool {
    let text = |ty: &PlanType| matches!(ty, PlanType::String | PlanType::FrozenStr);
    payload == member || (text(payload) && text(member))
}
