//! Assignments: checked values stored into locals, fields, elements and statics.

use incan_mir_plan_native::SourceSpan;
use incan_semantics_core::body_ir::{BodyIrModule, Place as SourcePlace, Rvalue};

use crate::frame::Builder;
use crate::pending::untranslated;

/// Assign a checked value to a local, routing strings, formatting and runtime-semantics operators first.
pub fn lower_assignment(
    module: &BodyIrModule,
    builder: &mut Builder,
    destination: &SourcePlace,
    value: &Rvalue,
    span: &SourceSpan,
) -> Result<(), String> {
    let _ = (module, builder, destination, value, span);
    Err(untranslated("assignments::lower_assignment"))
}
