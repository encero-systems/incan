//! Native fail-fast extraction of source-local checked newtype construction results.

use incan_mir_plan_native::{PlanType, SourceSpan};
use incan_semantics_core::body_ir::{ArgumentElement, Place as SourcePlace};

use crate::builtin_calls::fixed_arguments;
use crate::calls::instantiated_external_call;
use crate::checked_values::scalar_place;
use crate::frame::{Builder, place_type};
use crate::options::result_layout;
use crate::sequence_builtins::argument_type;

/// Validate the retained Result and dispatch the chosen fail-fast or sole-field extraction contract.
pub fn lower_newtype_hook_result(
    builder: &mut Builder,
    arguments: &[ArgumentElement],
    destination: &Option<SourcePlace>,
    aggregate_field: bool,
    span: &SourceSpan,
) -> Result<(), String> {
    if arguments.len() != 3 {
        return Err("invalid Body IR NewtypeHookExtract arity".into());
    }
    let Some(place) = destination else {
        return Err("unsupported Body IR checked-newtype extraction without a destination".into());
    };
    let output = scalar_place(builder, place, span)?;
    let target = place_type(&builder.locals, &output);
    let input = argument_type(builder, arguments, 0, span)?;
    let (_, declaration) = result_layout(builder, &input)?;
    let (Some(success), Some(failure)) = (declaration.variants.first(), declaration.variants.get(1)) else {
        return Err("unsupported Body IR checked-newtype extraction payload".into());
    };
    if success.fields.len() != 1 || success.fields[0] != target {
        return Err("unsupported Body IR checked-newtype extraction payload".into());
    }
    let [error] = failure.fields.as_slice() else {
        return Err("unsupported Body IR checked-newtype extraction error".into());
    };
    let error = error.clone();
    if argument_type(builder, arguments, 1, span)? != PlanType::String
        || argument_type(builder, arguments, 2, span)? != PlanType::String
    {
        return Err("unsupported Body IR checked-newtype extraction labels".into());
    }
    let operands = fixed_arguments(builder, arguments, span)?;
    let path = if aggregate_field {
        "incan_native_runtime::checked_newtype_field_result"
    } else {
        "incan_native_runtime::checked_newtype_result"
    };
    let type_arguments = vec![target.clone(), error];
    let parameters = vec![input, PlanType::String, PlanType::String];
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
