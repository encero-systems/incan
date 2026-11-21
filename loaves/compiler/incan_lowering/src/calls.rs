//! Calls: checked direct, Loaf, delegated and instantiated external calls.

use incan_mir_plan_native::SourceSpan;
use incan_semantics_core::body_ir::{BodyIrModule, Statement};

use crate::frame::Builder;
use crate::pending::untranslated;

/// Lower one checked call statement.
pub fn lower_call(
    module: &BodyIrModule,
    builder: &mut Builder,
    statement: &Statement,
    span: &SourceSpan,
) -> Result<(), String> {
    let _ = (module, builder, statement, span);
    Err(untranslated("calls::lower_call"))
}
