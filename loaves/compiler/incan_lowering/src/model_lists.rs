//! Lists held by model fields and tuple elements, read after their structural prefix.

use incan_mir_plan_native::{Operand, Place, PlanType, SourceSpan};
use incan_semantics_core::body_ir::{Place as SourcePlace, PlaceElem};

use crate::frame::Builder;
use crate::list_values::projected_element_read;
use crate::models::{field_step_type, model_prefix};
use crate::tuples::is_tuple_type;

/// Read an index or slice after a retained nominal field or tuple element prefix, cloning its result instead of moving
/// the parent.
pub fn model_list_read(
    builder: &mut Builder,
    source: &SourcePlace,
    span: &SourceSpan,
) -> Result<Option<Operand>, String> {
    match structural_prefix(builder, source)? {
        None => Ok(None),
        Some((count, _)) => {
            if count == source.projection.len() {
                return Ok(None);
            }
            if count == 0 {
                return Err("unsupported Body IR model indexing without a field".into());
            }
            Ok(Some(prefixed_list_read(builder, source, count, span)?))
        }
    }
}

/// Count the leading field, payload and tuple-element steps of a place rooted at a model, enum or tuple.
///
/// The walk tracks each step's type, so a constant index into a tuple stays part of the prefix like a field, while the
/// first index into a list ends it. The flag reports whether the prefix passed through a tuple. A place rooted at
/// anything else has no structural prefix.
pub fn structural_prefix(builder: &Builder, source: &SourcePlace) -> Result<Option<(usize, bool)>, String> {
    let Some(local) = source.local_id() else {
        return Ok(None);
    };
    let root = &builder.locals[(local.index() as i64 + 1 + builder.local_offset) as usize].ty;
    let mut ty = match root {
        PlanType::Model(..) | PlanType::Enum(..) | PlanType::Tuple(_) => root.clone(),
        PlanType::TupleMutRef(elements) | PlanType::TupleRef(elements) => PlanType::Tuple(elements.clone()),
        PlanType::ModelRef(owner, name) | PlanType::ModelMutRef(owner, name) => PlanType::Model(*owner, name.clone()),
        _ => return Ok(None),
    };
    let mut through_tuple = false;
    let mut count = 0;
    for projection in &source.projection {
        if is_tuple_type(&ty) {
            through_tuple = true;
        }
        match projection {
            // RFC 164: a reference root's dereference reaches the same referent its fields are projected from.
            PlaceElem::Deref
                if count == 0 && !matches!(root, PlanType::Model(..) | PlanType::Enum(..) | PlanType::Tuple(_)) =>
            {
                count += 1;
                continue;
            }
            PlaceElem::Field { .. } | PlaceElem::OptionPayload { .. } | PlaceElem::UnionMember { .. } => {}
            PlaceElem::Index(_) => {
                if !is_tuple_type(&ty) {
                    break;
                }
            }
            _ => break,
        }
        ty = field_step_type(builder.tables(), &ty, projection)?;
        count += 1;
    }
    Ok(Some((count, through_tuple)))
}

/// Project the first `count` structural steps of a place without reading or copying anything.
pub fn prefix_place(builder: &Builder, source: &SourcePlace, count: usize, span: &SourceSpan) -> Result<Place, String> {
    let mut place = model_prefix(
        builder.tables(),
        &builder.locals,
        source,
        count,
        span,
        builder.local_offset,
    )?;
    place.local += builder.local_offset;
    Ok(place)
}

/// Read the list indexes, slices and element fields that follow a structural prefix, cloning an owned result.
fn prefixed_list_read(
    builder: &mut Builder,
    source: &SourcePlace,
    count: usize,
    span: &SourceSpan,
) -> Result<Operand, String> {
    let container = prefix_place(builder, source, count, span)?;
    projected_element_read(builder, source, container, count, span)
}
