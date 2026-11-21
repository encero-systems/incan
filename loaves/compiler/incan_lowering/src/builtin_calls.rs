//! Builtin function calls and their scalar boundary checks.

use incan_mir_plan_native::{Operand, Place, PlanType, RvalueKind, SourceSpan, assign as plan_assign};

use crate::builtins::emit_boolean_operator;
use crate::checked_values::scalar_operand;
use crate::frame::{Builder, external_call, place_type, read_place, temporary, value_type};
use crate::list_values::value_operand;
use crate::numerics::{has_sized_numeric, numeric_carrier, numeric_name};
use crate::operators::scalar_operator_operand;
use incan_semantics_core::body_ir::Operand as SourceOperand;

/// Keep exact f32 values finite at assignment and parameter boundaries, as legacy does.
pub fn validate_exact_float(builder: &mut Builder, target: &Place, span: &SourceSpan) {
    if place_type(&builder.locals, target) == PlanType::F32 {
        let operand = read_place(target.clone(), false, span);
        external_call(
            builder,
            "incan_std_primitives::num::require_finite_f32",
            vec![PlanType::F32],
            PlanType::F32,
            vec![operand],
            target.clone(),
            span,
        );
    }
}

/// Materialize a checker-admitted destination carrier at typed boundaries, never guess binary promotions.
pub fn coerce_numeric_operand(
    builder: &mut Builder,
    operand: Operand,
    target: &PlanType,
    span: &SourceSpan,
) -> Result<Operand, String> {
    if !numeric_carrier(target) {
        return Ok(operand);
    }
    let source = value_type(builder, &operand)?;
    if source == *target {
        return Ok(operand);
    }
    if !numeric_carrier(&source) {
        return Err("unsupported Body IR nonnumeric implicit conversion".into());
    }
    let converted = temporary(builder, target.clone(), span);
    builder.emit(plan_assign(
        converted.clone(),
        RvalueKind::NumericCast(operand, source, target.clone()),
        span,
    ));
    validate_exact_float(builder, &converted, span);
    Ok(read_place(converted, false, span))
}

/// Combine pure Body IR operands; source short-circuiting is retained by frontend branches.
pub fn lower_boolean_operator(
    builder: &mut Builder,
    destination: Place,
    left: &SourceOperand,
    right: &SourceOperand,
    name: &str,
    span: &SourceSpan,
) -> Result<(), String> {
    let operands = vec![
        scalar_operator_operand(builder, left, span)?,
        scalar_operator_operand(builder, right, span)?,
    ];
    emit_boolean_operator(builder, destination, operands, name, span)
}

/// Call the runtime's Python-semantics `//` or `%`, which handle negative operands and division by zero.
pub fn lower_numeric_helper(
    builder: &mut Builder,
    destination: Place,
    left: &SourceOperand,
    right: &SourceOperand,
    operation: &str,
    span: &SourceSpan,
) -> Result<(), String> {
    let ty = place_type(&builder.locals, &destination);
    if has_sized_numeric(&ty) {
        let arguments = vec![
            scalar_operand(builder, left, span)?,
            scalar_operand(builder, right, span)?,
        ];
        if value_type(builder, &arguments[0])? != ty || value_type(builder, &arguments[1])? != ty {
            return Err("unsupported Body IR mixed numeric division or modulo".into());
        }
        external_call(
            builder,
            &format!("incan_native_runtime::{operation}_{}", numeric_name(&ty)),
            vec![ty.clone(), ty.clone()],
            ty,
            arguments,
            destination.clone(),
            span,
        );
        validate_exact_float(builder, &destination, span);
        return Ok(());
    }
    let suffix = match ty {
        PlanType::Int => "i64",
        PlanType::Float => "f64",
        _ => return Err("unsupported Body IR division or modulo type".into()),
    };
    let path = format!("incan_std_primitives::num::{operation}_{suffix}");
    let arguments = vec![
        value_operand(builder, left, span)?,
        value_operand(builder, right, span)?,
    ];
    external_call(
        builder,
        &path,
        vec![ty.clone(), ty.clone()],
        ty,
        arguments,
        destination,
        span,
    );
    Ok(())
}
