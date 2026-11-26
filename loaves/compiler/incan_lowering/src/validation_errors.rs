//! Plan construction and borrowed observation of the checked builtin validation error carrier.

use incan_mir_plan_native::{
    CalleeKind, ExternalFunction, Operand, Place, PlanType, RvalueKind, SourceSpan, assign as plan_assign,
};
use incan_semantics_core::body_ir::{ArgumentElement, Place as SourcePlace};

use crate::builtin_calls::fixed_arguments;
use crate::callable_calls::lower_callable_result;
use crate::checked_values::scalar_place;
use crate::frame::{Builder, external_call, place_type, read_place, temporary};
use crate::sequence_builtins::argument_type;

/// Use checked owned String arguments and the actual runtime constructor, retaining normal and unwind ownership.
pub fn lower_validation_error(
    builder: &mut Builder,
    arguments: &[ArgumentElement],
    destination: &Option<SourcePlace>,
    coded: bool,
    span: &SourceSpan,
) -> Result<(), String> {
    let (count, path) = if coded {
        (2, "incan_native_runtime::make_coded_validation_error")
    } else {
        (1, "incan_native_runtime::make_validation_error")
    };
    if arguments.len() != count {
        return Err("invalid Body IR validation error constructor arity".into());
    }
    let mut parameters = Vec::with_capacity(count);
    for index in 0..count {
        if argument_type(builder, arguments, index, span)? != PlanType::String {
            return Err("unsupported Body IR validation error constructor input".into());
        }
        parameters.push(PlanType::String);
    }
    let Some(place) = destination else {
        return Err("unsupported Body IR validation error constructor without destination".into());
    };
    let output = scalar_place(builder, place, span)?;
    if place_type(&builder.locals, &output) != PlanType::ValidationError {
        return Err("unsupported Body IR validation error constructor destination".into());
    }
    let operands = fixed_arguments(builder, arguments, span)?;
    builder.externals.push(ExternalFunction {
        path: path.into(),
        type_arguments: Vec::new(),
        parameters,
        return_type: PlanType::ValidationError,
        span: span.clone(),
    });
    lower_callable_result(
        builder,
        CalleeKind::External(path.into()),
        operands,
        output,
        PlanType::ValidationError,
        span,
    )
}

/// Borrow an owned native validation error or preserve its existing shared reference without cloning its payload.
pub fn validation_error_reference(builder: &mut Builder, source: Place, span: &SourceSpan) -> Result<Operand, String> {
    let reference = match place_type(&builder.locals, &source) {
        PlanType::ValidationError => {
            let reference = temporary(builder, PlanType::ValidationErrorRef, span);
            builder.emit(plan_assign(reference.clone(), RvalueKind::Borrow(source), span));
            reference
        }
        PlanType::ValidationErrorRef => source,
        _ => return Err("unsupported Body IR validation error display source".into()),
    };
    Ok(read_place(reference, false, span))
}

/// Borrow dependency-owned Display without consuming either a local error or an active Result payload.
pub fn display_validation_error(builder: &mut Builder, source: Place, span: &SourceSpan) -> Result<Place, String> {
    let reference = validation_error_reference(builder, source, span)?;
    let output = temporary(builder, PlanType::String, span);
    external_call(
        builder,
        "incan_native_runtime::format_validation_error",
        vec![PlanType::ValidationErrorRef],
        PlanType::String,
        vec![reference],
        output.clone(),
        span,
    );
    Ok(output)
}
