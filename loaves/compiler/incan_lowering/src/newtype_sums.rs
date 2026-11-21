//! Execute retained numeric-newtype item projections and post-exhaustion hook-result extraction.

use incan_mir_plan_native::{BinaryOp, PlanType, RvalueKind, SourceSpan, assign as plan_assign};
use incan_semantics_core::body_ir::{ArgumentElement, Place as SourcePlace};

use crate::builtin_calls::fixed_arguments;
use crate::calls::instantiated_external_call;
use crate::checked_values::scalar_place;
use crate::frame::{Builder, external_call, place_type};
use crate::options::result_layout;
use crate::sequence_builtins::argument_type;

/// Add already projected integer or float payloads through the selected primitive operation.
pub fn lower_newtype_sum_add(
    builder: &mut Builder,
    arguments: &[ArgumentElement],
    destination: &Option<SourcePlace>,
    span: &SourceSpan,
) -> Result<(), String> {
    if arguments.len() != 2 {
        return Err("unsupported Body IR NewtypeSumAdd arity".into());
    }
    let Some(place) = destination else {
        return Err("unsupported Body IR newtype sum addition without a destination".into());
    };
    let output = scalar_place(builder, place, span)?;
    let numeric = place_type(&builder.locals, &output);
    for index in 0..2 {
        if argument_type(builder, arguments, index, span)? != numeric {
            return Err("unsupported Body IR newtype sum addition operand".into());
        }
    }
    let operands = fixed_arguments(builder, arguments, span)?;
    match numeric {
        PlanType::Int => {
            let path = "incan_native_runtime::add_newtype_sum";
            external_call(
                builder,
                path,
                vec![PlanType::Int, PlanType::Int],
                PlanType::Int,
                operands,
                output,
                span,
            );
            Ok(())
        }
        PlanType::Float => {
            let mut operands = operands.into_iter();
            let (Some(left), Some(right)) = (operands.next(), operands.next()) else {
                return Err("unsupported Body IR NewtypeSumAdd arity".into());
            };
            builder.emit(plan_assign(
                output,
                RvalueKind::Binary(BinaryOp::Add, left, right),
                span,
            ));
            Ok(())
        }
        _ => Err("unsupported Body IR newtype sum accumulator type".into()),
    }
}

/// Extract the already selected final hook's Result with the retained frozen sum failure message.
pub fn lower_newtype_sum_extract(
    builder: &mut Builder,
    arguments: &[ArgumentElement],
    destination: &Option<SourcePlace>,
    span: &SourceSpan,
) -> Result<(), String> {
    if arguments.len() != 2 {
        return Err("unsupported Body IR NewtypeSumExtract arity".into());
    }
    let Some(place) = destination else {
        return Err("unsupported Body IR newtype sum extraction without a destination".into());
    };
    let output = scalar_place(builder, place, span)?;
    let target = place_type(&builder.locals, &output);
    let input = argument_type(builder, arguments, 0, span)?;
    let (_, declaration) = result_layout(builder, &input)?;
    let (Some(success), Some(failure)) = (declaration.variants.first(), declaration.variants.get(1)) else {
        return Err("unsupported Body IR newtype sum extraction payload".into());
    };
    if success.fields.len() != 1 || success.fields[0] != target {
        return Err("unsupported Body IR newtype sum extraction payload".into());
    }
    if failure.fields.len() != 1 || argument_type(builder, arguments, 1, span)? != PlanType::String {
        return Err("unsupported Body IR newtype sum extraction error or message".into());
    }
    let error = failure.fields[0].clone();
    let operands = fixed_arguments(builder, arguments, span)?;
    let type_arguments = vec![target.clone(), error];
    let parameters = vec![input, PlanType::String];
    let path = "incan_native_runtime::checked_sum_result";
    instantiated_external_call(
        builder,
        path,
        type_arguments,
        parameters,
        target,
        operands,
        output,
        span,
    )
}
