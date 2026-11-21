//! Builtin runtime operations shared by calls and operators.

use incan_mir_plan_native::{Operand, Place, PlanType, SourceSpan};

use crate::frame::{Builder, external_call, value_type};

/// Combine pure Body IR operands; source short-circuiting is retained by frontend branches.
pub fn emit_boolean_operator(
    builder: &mut Builder,
    destination: Place,
    operands: Vec<Operand>,
    name: &str,
    span: &SourceSpan,
) -> Result<(), String> {
    if operands.len() != 2
        || value_type(builder, &operands[0])? != PlanType::Bool
        || value_type(builder, &operands[1])? != PlanType::Bool
    {
        return Err("unsupported Body IR non-boolean logical operands".into());
    }
    external_call(
        builder,
        &format!("incan_native_runtime::{name}"),
        vec![PlanType::Bool, PlanType::Bool],
        PlanType::Bool,
        operands,
        destination,
        span,
    );
    Ok(())
}
