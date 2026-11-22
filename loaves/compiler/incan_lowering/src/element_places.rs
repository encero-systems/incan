//! Recognize and type checked places that select a collection element or a copied text slice.
//!
//! No native place projection can name such an element directly: the runtime reaches it through a checked accessor.
//! Shared list access preserves intermediate elements for a field read; owned element reads materialize a copy.
//! Code that only needs the selected type computes it here without emitting either operation.

use incan_mir_plan_native::{ListLeaf, PlanType};
use incan_semantics_core::body_ir::{Place as SourcePlace, PlaceElem};

use crate::collections::{collection_arguments, is_collection};
use crate::frame::Builder;
use crate::lists::{is_list, list_element, list_shape};
use crate::model_lists::structural_prefix;
use crate::models::field_step_type;

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

/// Follow an element place's projections to the type of the value its read materializes, emitting nothing.
pub fn element_place_type(builder: &Builder, source: &SourcePlace) -> Result<PlanType, String> {
    element_prefix_type(builder, source, source.projection.len())
}

/// Resolve a checked element prefix once, without reading or borrowing its storage.
pub fn element_prefix_type(builder: &Builder, source: &SourcePlace, count: usize) -> Result<PlanType, String> {
    if count > source.projection.len() {
        return Err("invalid Body IR element type prefix length".into());
    }
    let Some(local) = source.local_id() else {
        return Err("unsupported Body IR global element place".into());
    };
    // A receiver reference projects its model's fields.
    let mut ty = match &builder.locals[(local.index() as i64 + 1 + builder.local_offset) as usize].ty {
        PlanType::ModelRef(owner, name) | PlanType::ModelMutRef(owner, name) => PlanType::Model(*owner, name.clone()),
        PlanType::TupleMutRef(elements) | PlanType::TupleRef(elements) => PlanType::Tuple(elements.clone()),
        PlanType::StringRef | PlanType::StringMutRef => PlanType::String,
        root => root.clone(),
    };
    for projection in &source.projection[..count] {
        ty = projected_type(builder, ty, projection)?;
    }
    Ok(ty)
}

/// Select one step's carrier: an indexed Unicode scalar string, collection element, slice, field or tuple element.
fn projected_type(builder: &Builder, ty: PlanType, projection: &PlaceElem) -> Result<PlanType, String> {
    let source = match ty {
        PlanType::FrozenCollectionRef(name, elements) => PlanType::FrozenCollection(name, elements),
        other => other,
    };
    match projection {
        // RFC 164: a whole reference-valued local read or written through its reference names its referent.
        PlaceElem::Deref => {
            return Ok(crate::explicit_arguments::reference_contract(&source).map_or(source, |(referent, _)| referent));
        }
        PlaceElem::Index(_) => {
            if matches!(source, PlanType::String | PlanType::FrozenStr) {
                return Ok(PlanType::String);
            }
            if source == PlanType::FrozenBytes {
                return Ok(PlanType::U8);
            }
            match &source {
                PlanType::NativeArray(elements, _) if elements.len() == 1 => return Ok(elements[0].clone()),
                PlanType::FrozenCollection(name, elements) => {
                    if name == "FrozenDict" && elements.len() == 2 {
                        return Ok(frozen_read_type(&elements[1]));
                    }
                    if name == "FrozenList" && elements.len() == 1 {
                        return Ok(frozen_read_type(&elements[0]));
                    }
                    return Err("unsupported Body IR frozen collection index type".into());
                }
                _ => {}
            }
            if let Some((leaf, depth)) = list_shape(&source) {
                return Ok(list_element(&leaf, depth));
            }
            if is_collection(&source) {
                return dictionary_value_type(&source);
            }
        }
        PlaceElem::Slice { .. } => {
            if matches!(source, PlanType::String | PlanType::FrozenStr) {
                return Ok(PlanType::String);
            }
            if source == PlanType::FrozenBytes {
                return Ok(PlanType::List(ListLeaf::U8, 1));
            }
            return match list_shape(&source) {
                Some((leaf, depth)) => Ok(PlanType::List(leaf, depth)),
                None => Err("unsupported Body IR slicing non-list".into()),
            };
        }
        _ => {}
    }
    field_step_type(builder.tables(), &source, projection)
}

/// Observe ordinary static text and bytes as owned values; explicitly frozen wrappers keep their exact identity.
fn frozen_read_type(storage: &PlanType) -> PlanType {
    match storage {
        PlanType::StaticText => PlanType::String,
        PlanType::StaticBytes => PlanType::List(ListLeaf::U8, 1),
        other => other.clone(),
    }
}

/// A dictionary index selects its value type; a set has no indexable elements.
fn dictionary_value_type(ty: &PlanType) -> Result<PlanType, String> {
    let mut arguments = collection_arguments(ty);
    if arguments.len() != 2 {
        return Err("unsupported Body IR set indexing".into());
    }
    Ok(arguments.swap_remove(1))
}
