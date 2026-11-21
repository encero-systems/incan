//! Calls: checked direct, Loaf, delegated and instantiated external calls.

use incan_mir_plan_native::{Operand, Place, PlanType, SourceSpan};
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

/// Declare one selected external signature and preserve owned-result replacement and cleanup.
#[allow(clippy::too_many_arguments)] // The Incan helper's eight parts of one external call.
pub fn instantiated_external_call(
    builder: &mut Builder,
    path: &str,
    type_arguments: Vec<PlanType>,
    parameters: Vec<PlanType>,
    result: PlanType,
    operands: Vec<Operand>,
    output: Place,
    span: &SourceSpan,
) -> Result<(), String> {
    let _ = (
        builder,
        path,
        type_arguments,
        parameters,
        result,
        operands,
        output,
        span,
    );
    Err(untranslated("calls::instantiated_external_call"))
}
