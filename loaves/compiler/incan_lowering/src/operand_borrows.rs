//! Operands lowered for their effects or as borrows.

use incan_mir_plan_native::SourceSpan;
use incan_semantics_core::body_ir::Operand;

use crate::frame::Builder;
use crate::pending::untranslated;

/// Lower an expression statement's operand, whose value is discarded.
pub fn lower_discarded_operand(builder: &mut Builder, value: &Operand, span: &SourceSpan) -> Result<(), String> {
    let _ = (builder, value, span);
    Err(untranslated("operand_borrows::lower_discarded_operand"))
}
