//! List methods, indexed assignment and call destinations.

use incan_mir_plan_native::{Place, PlanType, SourceSpan};
use incan_semantics_core::body_ir::Place as SourcePlace;

use crate::checked_values::scalar_place;
use crate::frame::{Builder, place_type, temporary};
use crate::list_iteration::list_root_place;

/// Select the checked destination or a typed temporary for a discarded call.
pub fn call_destination(
    builder: &mut Builder,
    destination: &Option<SourcePlace>,
    ty: PlanType,
    span: &SourceSpan,
) -> Result<Place, String> {
    match destination {
        Some(place) => {
            let root = list_root_place(builder, place, span)?;
            if place_type(&builder.locals, &root) == ty {
                return Ok(root);
            }
            scalar_place(builder, place, span)
        }
        None => Ok(temporary(builder, ty, span)),
    }
}
