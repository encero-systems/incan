//! Preserve verified foreign declarations and checked arguments through the native call boundary.

use incan_mir_plan_native::{CalleeKind, ForeignFunction, Operand, PlanType, SourceSpan};
use incan_semantics_core::body_ir::{ArgumentElement, CheckedForeignCall, Place as SourcePlace};
use incan_semantics_core::{IncanType, encode_incan_symbol_identity};

use crate::builtin_calls::coerce_numeric_operand;
use crate::callable_calls::lower_callable_result;
use crate::checked_values::{scalar_operand, scalar_place};
use crate::frame::{Builder, temporary};
use crate::layout_types::native_source_type;

/// Call one verified C declaration without reconstructing a native contract.
///
/// 1. Retain the exact native carriers and source-selected symbol.
/// 2. Read checked arguments and select the result place.
/// 3. Use the ordinary call-result ownership boundary.
pub fn lower_foreign_call(
    builder: &mut Builder,
    target: &CheckedForeignCall,
    arguments: &[ArgumentElement],
    destination: &Option<SourcePlace>,
    span: &SourceSpan,
) -> Result<(), String> {
    // 1. Physical names and carriers come only from the accepting frontend.
    let mut parameters = Vec::with_capacity(target.parameters.len());
    for parameter in &target.parameters {
        parameters.push(builder_native_type(builder, parameter)?);
    }
    let output = builder_native_type(builder, &target.return_type)?;
    let declared = ForeignFunction {
        name: format!("{}__c_{}", encode_incan_symbol_identity(&target.binding), target.member),
        symbol: target.symbol.clone(),
        library: target.library.clone(),
        clove: target.clove.clone(),
        configuration: target.configuration.clone(),
        framework: target.framework,
        parameters: parameters.clone(),
        return_type: output.clone(),
    };
    // 2. Source evaluation order was retained in Body IR; no foreign argument is reclassified here.
    let operands = foreign_arguments(builder, arguments, &parameters, span)?;
    let mut result = temporary(builder, output.clone(), span);
    if let Some(place) = destination {
        result = scalar_place(builder, place, span)?;
    }
    // 3. Return replacement and cleanup use the same rule as other native calls.
    lower_callable_result(builder, CalleeKind::Foreign(declared), operands, result, output, span)
}

/// The native carrier of one checked foreign parameter or result type.
fn builder_native_type(builder: &Builder, ty: &IncanType) -> Result<PlanType, String> {
    native_source_type(
        builder.constructors(),
        builder.nominals(),
        builder.enums(),
        &builder.unions,
        ty,
        &builder.intrinsics,
        "",
    )
}

/// Read checked arguments and materialize the descriptor's already accepted exact numeric carriers.
fn foreign_arguments(
    builder: &mut Builder,
    arguments: &[ArgumentElement],
    parameters: &[PlanType],
    span: &SourceSpan,
) -> Result<Vec<Operand>, String> {
    let mut operands = Vec::with_capacity(parameters.len());
    for argument in arguments {
        let ArgumentElement::One(value) = argument else {
            return Err("invalid Body IR spread at a checked foreign call".into());
        };
        let Some(parameter) = parameters.get(operands.len()) else {
            return Err("invalid Body IR foreign call arity".into());
        };
        let operand = scalar_operand(builder, value, span)?;
        operands.push(coerce_numeric_operand(builder, operand, parameter, span)?);
    }
    if operands.len() != parameters.len() {
        return Err("invalid Body IR foreign call arity".into());
    }
    Ok(operands)
}
