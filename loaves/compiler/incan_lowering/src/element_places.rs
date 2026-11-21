//! Places that select a collection element or text slice rather than storage a native place can name.

use incan_mir_plan_native::PlanType;
use incan_semantics_core::body_ir::Place as SourcePlace;

use crate::collections::is_collection;
use crate::frame::Builder;
use crate::lists::is_list;
use crate::model_lists::structural_prefix;

/// Whether a local place selects a collection element or text slice rather than storage a native place can name.
///
/// Such a place is rooted at a list, dictionary or text value, or indexes a sequence after a structural field, payload
/// or tuple element prefix. A global place keeps its static read path and is never an element place here.
pub fn is_element_place(builder: &Builder, source: &SourcePlace) -> Result<bool, String> {
    if source.projection.is_empty() {
        return Ok(false);
    }
    let Some(local) = source.local_id() else {
        return Ok(false);
    };
    let root = &builder.locals[(local.index() as i64 + 1 + builder.local_offset) as usize].ty;
    if is_list(root)
        || is_collection(root)
        || matches!(
            root,
            PlanType::String
                | PlanType::StringRef
                | PlanType::StringMutRef
                | PlanType::FrozenStr
                | PlanType::FrozenBytes
                | PlanType::NativeArray(..)
                | PlanType::FrozenCollection(..)
                | PlanType::FrozenCollectionRef(..)
        )
    {
        return Ok(true);
    }
    match structural_prefix(builder, source)? {
        Some((count, _)) => Ok(count < source.projection.len()),
        None => Ok(false),
    }
}
