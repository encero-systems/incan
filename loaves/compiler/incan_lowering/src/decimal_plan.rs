//! Decimal values retain the frontend's exact coefficient and written scale.

use incan_mir_plan_native::{Constant, Operand, PlanType, SourceSpan};
use incan_semantics_core::body_ir::{BinOp, TypedNumericConstant};

use crate::frame::{Builder, external_call, literal, read_place, temporary};

/// Materialize the existing legacy carrier from the checker-owned exact payload.
pub fn decimal_literal(
    builder: &mut Builder,
    value: &TypedNumericConstant,
    span: &SourceSpan,
) -> Result<Operand, String> {
    let TypedNumericConstant::Decimal {
        coefficient,
        literal_scale,
        ..
    } = value
    else {
        return Err("unsupported Body IR nondecimal literal at decimal boundary".into());
    };
    let output = temporary(builder, PlanType::Decimal, span);
    external_call(
        builder,
        "incan_native_runtime::decimal_from_parts",
        vec![PlanType::I128, PlanType::U8],
        PlanType::Decimal,
        vec![
            literal(Constant::Numeric(coefficient.to_string(), PlanType::I128), span),
            literal(Constant::Numeric(literal_scale.to_string(), PlanType::U8), span),
        ],
        output.clone(),
        span,
    );
    Ok(read_place(output, false, span))
}

/// Use the canonical Decimal128 value comparison, never rustc scalar arithmetic.
pub fn decimal_comparison(
    builder: &mut Builder,
    operator: BinOp,
    left: Operand,
    right: Operand,
    span: &SourceSpan,
) -> Result<Operand, String> {
    let name = match operator {
        BinOp::Eq => "equal_decimal",
        BinOp::Ne => "unequal_decimal",
        BinOp::Lt => "less_decimal",
        BinOp::Le => "less_equal_decimal",
        BinOp::Gt => "greater_decimal",
        BinOp::Ge => "greater_equal_decimal",
        _ => return Err("decimal values support comparison here; decimal arithmetic is unsupported".into()),
    };
    let output = temporary(builder, PlanType::Bool, span);
    external_call(
        builder,
        &format!("incan_native_runtime::{name}"),
        vec![PlanType::Decimal, PlanType::Decimal],
        PlanType::Bool,
        vec![left, right],
        output.clone(),
        span,
    );
    Ok(read_place(output, false, span))
}
