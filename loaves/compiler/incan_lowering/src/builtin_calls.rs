//! Builtin function calls and their scalar boundary checks.

use incan_mir_plan_native::{Operand, Place, PlanType, RvalueKind, SourceSpan, assign as plan_assign};

use crate::frame::{Builder, external_call, place_type, read_place, temporary, value_type};
use crate::numerics::numeric_carrier;

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
