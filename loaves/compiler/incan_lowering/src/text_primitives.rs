//! Calls into primitive-owned UTF-8 conversion selected by compiler-introduced operations.

use incan_mir_plan_native::{ListLeaf, Operand, Place, PlanType, SourceSpan, TupleElement};

use crate::frame::{Builder, external_call, temporary};

/// Copy a borrowed or frozen text value through the authenticated primitive provider.
pub fn encode_utf8(builder: &mut Builder, value: Operand, frozen: bool, span: &SourceSpan) -> Place {
    let bytes = PlanType::List(ListLeaf::U8, 1);
    let output = temporary(builder, bytes.clone(), span);
    let (path, parameter) = if frozen {
        ("incan_stdlib_primitives::encode_frozen_utf8", PlanType::FrozenStr)
    } else {
        ("incan_stdlib_primitives::encode_utf8", PlanType::StringRef)
    };
    external_call(builder, path, vec![parameter], bytes, vec![value], output.clone(), span);
    builder.owned.push(output.local);
    output
}

/// Copy frozen byte storage through the authenticated primitive provider.
pub fn copy_frozen_bytes(builder: &mut Builder, value: Operand, span: &SourceSpan) -> Place {
    let ty = PlanType::List(ListLeaf::U8, 1);
    let output = temporary(builder, ty.clone(), span);
    external_call(
        builder,
        "incan_stdlib_primitives::copy_frozen_bytes",
        vec![PlanType::FrozenBytes],
        ty,
        vec![value],
        output.clone(),
        span,
    );
    builder.owned.push(output.local);
    output
}

/// Return the primitive decoder's explicit success flag and owned text-or-detail payload.
pub fn decode_utf8(
    builder: &mut Builder,
    value: Operand,
    value_type: PlanType,
    replace_invalid: Operand,
    span: &SourceSpan,
) -> Place {
    let result = PlanType::Tuple(vec![TupleElement::Bool, TupleElement::String]);
    let output = temporary(builder, result.clone(), span);
    external_call(
        builder,
        "incan_stdlib_primitives::decode_utf8",
        vec![value_type, PlanType::Bool],
        result,
        vec![value, replace_invalid],
        output.clone(),
        span,
    );
    builder.owned.push(output.local);
    output
}
