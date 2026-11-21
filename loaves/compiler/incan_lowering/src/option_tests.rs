//! `isinstance` over an Option carrier, as legacy matches it: true only for a present payload of the target type.
//!
//! A payload that is itself a union is tested by its member tag after the Option's own tag proves it present, so the
//! union payload is never inspected through an absent Option.

use incan_mir_plan_native::{
    Carrier, Constant, Place, PlanType, RvalueKind, SourceSpan, assign as plan_assign, carrier_position,
};

use crate::frame::{Builder, literal, read_place, temporary};
use crate::pending::untranslated;

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
///
/// The member tag tests are `unions::union_test_slots` and `unions::test_union_tag`, which refuse by name until they
/// are in Rust.
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
        return Err(untranslated("unions::union_test_slots"));
    }
    if !same_isinstance_type(&payload, member) {
        return Ok(RvalueKind::Use(literal(Constant::Bool(false), span)));
    }
    enum_tag(builder, source, span);
    Err(untranslated("unions::test_union_tag"))
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
