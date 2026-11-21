//! Owned text values built through the runtime's string primitives.

use incan_mir_plan_native::{Operand, Place, PlanType, RvalueKind, SourceSpan, assign as plan_assign};

use crate::frame::{Builder, drop_owned, external_call, forget_owned, read_place, temporary, text_literal, value_type};

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

/// Call `incan_std_primitives::strings::fstring(&[&str], &[String])`, the helper the native route's f-strings use.
///
/// The literal texts and the displayed values become two fixed-size arrays, borrowed as slices for the call. The
/// value array takes ownership of the displayed strings and is dropped after the call.
pub fn format_parts(builder: &mut Builder, destination: Place, parts: &[String], values: &[Place], span: &SourceSpan) {
    let literals = temporary(builder, PlanType::StrArray(parts.len() as i64), span);
    let texts = parts.iter().map(|part| text_literal(part, span)).collect();
    builder.emit(plan_assign(literals.clone(), RvalueKind::Array(texts), span));

    let strings = temporary(builder, PlanType::StringArray(values.len() as i64), span);
    let mut operands = Vec::with_capacity(values.len());
    for value in values {
        operands.push(read_place(value.clone(), true, span));
        // Each displayed string moves into the array, so the array now owns it.
        forget_owned(builder, value.local);
    }
    builder.emit(plan_assign(strings.clone(), RvalueKind::Array(operands), span));
    builder.owned.push(strings.local);

    // `&[T; N]` coerces to `&[T]`: borrow each array, then unsize the borrow into a slice.
    let literal_reference = temporary(builder, PlanType::StrArrayRef(parts.len() as i64), span);
    let string_reference = temporary(builder, PlanType::StringArrayRef(values.len() as i64), span);
    builder.emit(plan_assign(
        literal_reference.clone(),
        RvalueKind::Borrow(literals),
        span,
    ));
    builder.emit(plan_assign(
        string_reference.clone(),
        RvalueKind::Borrow(strings.clone()),
        span,
    ));
    let literal_slice = temporary(builder, PlanType::StrSlice, span);
    let string_slice = temporary(builder, PlanType::StringSlice, span);
    builder.emit(plan_assign(
        literal_slice.clone(),
        RvalueKind::UnsizeSlice(read_place(literal_reference, false, span)),
        span,
    ));
    builder.emit(plan_assign(
        string_slice.clone(),
        RvalueKind::UnsizeSlice(read_place(string_reference, false, span)),
        span,
    ));

    let slices = vec![
        read_place(literal_slice, false, span),
        read_place(string_slice, false, span),
    ];
    let path = "incan_std_primitives::strings::fstring";
    external_call(
        builder,
        path,
        vec![PlanType::StrSlice, PlanType::StringSlice],
        PlanType::String,
        slices,
        destination,
        span,
    );
    drop_owned(builder, strings.local, span);
}

/// Materialize a retained native str view as the owned text the checked source result contract requires.
pub fn owned_slice_text(builder: &mut Builder, value: Operand, span: &SourceSpan) -> Place {
    let result = temporary(builder, PlanType::String, span);
    external_call(
        builder,
        "incan_std_primitives::strings::str_concat",
        vec![PlanType::StrRef, PlanType::StrRef],
        PlanType::String,
        vec![value, text_literal("", span)],
        result.clone(),
        span,
    );
    result
}

/// Copy a shared String through its native slice without calling the runtime's own clone wrapper.
pub fn owned_borrowed_text(builder: &mut Builder, value: Operand, span: &SourceSpan) -> Place {
    let view = temporary(builder, PlanType::StrRef, span);
    external_call(
        builder,
        "alloc::string::String::as_str",
        vec![PlanType::StringRef],
        PlanType::StrRef,
        vec![value],
        view.clone(),
        span,
    );
    owned_slice_text(builder, read_place(view, false, span), span)
}
