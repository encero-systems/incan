//! Owned text values built through the runtime's string primitives.

use incan_mir_plan_native::{Operand, Place, PlanType, RvalueKind, SourceSpan, assign as plan_assign};

use crate::frame::{Builder, external_call, read_place, temporary, text_literal, value_type};

/// Allocate an owned string from literal text, through the same runtime concatenation the legacy route uses.
pub fn owned_literal(builder: &mut Builder, text: &str, span: &SourceSpan) -> Place {
    let result = temporary(builder, PlanType::String, span);
    let arguments = vec![text_literal(text, span), text_literal("", span)];
    let path = "incan_std_primitives::strings::str_concat";
    external_call(
        builder,
        path,
        vec![PlanType::StrRef, PlanType::StrRef],
        PlanType::String,
        arguments,
        result.clone(),
        span,
    );
    result
}

/// Clone a string local into a new owned string through the runtime, borrowing the original.
pub fn clone_text(builder: &mut Builder, place: Place, span: &SourceSpan) -> Result<Place, String> {
    let reference = temporary(builder, PlanType::StringRef, span);
    builder.emit(plan_assign(reference.clone(), RvalueKind::Borrow(place), span));
    clone_text_reference(builder, read_place(reference, false, span), span)
}

/// Materialize owned display text from an existing shared string reference, retaining the original owner.
pub fn clone_text_reference(builder: &mut Builder, reference: Operand, span: &SourceSpan) -> Result<Place, String> {
    if value_type(builder, &reference)? != PlanType::StringRef {
        return Err("unsupported Body IR text clone without a shared string reference".into());
    }
    let result = temporary(builder, PlanType::String, span);
    external_call(
        builder,
        "incan_native_runtime::clone_text",
        vec![PlanType::StringRef],
        PlanType::String,
        vec![reference],
        result.clone(),
        span,
    );
    Ok(result)
}
