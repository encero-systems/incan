//! Shared loans of list elements, reached without copying their owner.

use incan_mir_plan_native::{Place, SourceSpan};
use incan_semantics_core::body_ir::{Place as SourcePlace, PlaceElem};

use crate::frame::Builder;
use crate::pending::untranslated;

/// Reach a checked shared list element loan without copying its owner or requiring Clone.
///
/// Only a path that indexes a list is a shared element path; any other container read keeps its own
/// materialization.
pub fn shared_element_place(
    builder: &mut Builder,
    source: &SourcePlace,
    span: &SourceSpan,
) -> Result<Option<Place>, String> {
    let _ = (builder, span);
    for projection in &source.projection {
        match projection {
            PlaceElem::Index(_) => return Err(untranslated("element_borrows::shared_element_place")),
            PlaceElem::Slice { .. } => return Ok(None),
            _ => {}
        }
    }
    Ok(None)
}
