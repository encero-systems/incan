//! List values: element reads, indexed reads and writes, and list construction.

use incan_mir_plan_native::{Operand, Place, PlanType, SourceSpan, place as plan_place};
use incan_semantics_core::body_ir::{
    Constant as SourceConstant, Operand as SourceOperand, OwnershipFact, Place as SourcePlace, PlaceElem,
};

use crate::checked_values::{reference_value_read, scalar_operand, scalar_place, validate_callback_place};
use crate::collections::is_collection;
use crate::element_borrows::shared_element_place;
use crate::element_places::is_element_place;
use crate::frame::{Builder, dereferenced_place, place_type, read_place};
use crate::lists::list_shape;
use crate::model_lists::model_list_read;
use crate::pending::untranslated;
use crate::text_plan::{clone_text, clone_text_reference, owned_literal};
use crate::tuple_iterators::zip_shape;

/// Copy the selected value without changing its source storage; list field reads borrow intermediate elements.
pub fn element_read(builder: &mut Builder, source: &SourcePlace, span: &SourceSpan) -> Result<Option<Operand>, String> {
    if let Some(local) = source.local_id() {
        if let Some(PlaceElem::Field { .. }) = source.projection.last()
            && shared_element_place(builder, source, span)?.is_some()
        {
            return Err(untranslated("list_values::materialize_element_field"));
        }
        let root_type = &builder.locals[(local.index() as i64 + 1 + builder.local_offset) as usize].ty;
        let text = matches!(
            root_type,
            PlanType::String
                | PlanType::StringRef
                | PlanType::StringMutRef
                | PlanType::FrozenStr
                | PlanType::FrozenBytes
        );
        let projected = matches!(root_type, PlanType::NativeArray(..) | PlanType::FrozenCollection(..));
        if (text || projected) && !source.projection.is_empty() {
            return Err(untranslated("list_values::projected_element_read"));
        }
    }
    if let Some(item) = model_list_read(builder, source, span)? {
        return Ok(Some(item));
    }
    if !is_element_place(builder, source)? {
        return Ok(None);
    }
    Err(untranslated("list_values::indexed_read"))
}

/// Materialize owned list and string values using each checked read's fact.
pub fn value_operand(builder: &mut Builder, value: &SourceOperand, span: &SourceSpan) -> Result<Operand, String> {
    let read = match value {
        SourceOperand::Constant(SourceConstant::Bytes(_)) => return Err(untranslated("bytes_plan::bytes_literal")),
        SourceOperand::Constant(SourceConstant::Str(text)) => {
            let text_place = owned_literal(builder, text, span);
            return Ok(read_place(text_place, true, span));
        }
        SourceOperand::Constant(_) => return scalar_operand(builder, value, span),
        SourceOperand::Place(read) => read,
    };
    validate_callback_place(builder, &read.place)?;
    if let Some(reference) = reference_value_read(builder, &read.place, read.fact, span)? {
        return Ok(reference);
    }
    if read.place.global().is_some() {
        return Err(untranslated("static_storage::read_static"));
    }
    if let Some(item) = element_read(builder, &read.place, span)? {
        return Ok(item);
    }
    let base = list_base_place(builder, &read.place, span)?;
    if is_collection(&builder.locals[base.local as usize].ty) {
        return Err(untranslated("hashed_collections::collection_value"));
    }
    let place = scalar_place(builder, &read.place, span)?;
    let ty = place_type(&builder.locals, &place);
    if is_collection(&ty) {
        return Err(untranslated("hashed_collections::collection_place_value"));
    }
    if zip_shape(&ty).is_some() {
        if read.fact == OwnershipFact::Move {
            return Ok(read_place(place, true, span));
        }
        if read.fact == OwnershipFact::Clone || read.fact == OwnershipFact::Borrow {
            return Err(untranslated("tuple_iterators::clone_zip"));
        }
        return Err("unsupported Body IR zip value ownership".into());
    }
    if let Some((leaf, depth)) = list_shape(&ty) {
        if read.fact == OwnershipFact::Move && ty == PlanType::List(leaf.clone(), depth) {
            return Ok(read_place(place, true, span));
        }
        // A list parameter is a view of the caller's list, so moving it out hands over a copy, which is the value
        // legacy's by-value parameter would carry.
        let shared = read.fact == OwnershipFact::Move && ty == PlanType::ListRef(leaf, depth);
        if read.fact != OwnershipFact::Clone && read.fact != OwnershipFact::Borrow && !shared {
            return Err("unsupported Body IR list value ownership".into());
        }
        return Err(untranslated("list_values::cloned_list"));
    }
    if ty == PlanType::String {
        if read.fact == OwnershipFact::Move {
            return Ok(read_place(place, true, span));
        }
        if read.fact == OwnershipFact::Clone || read.fact == OwnershipFact::Borrow {
            let text_place = clone_text(builder, place, span)?;
            return Ok(read_place(text_place, true, span));
        }
        return Err("unsupported Body IR string list element ownership".into());
    }
    // Text an inferred-shared parameter lends is a view of the caller's text, so its owned value is a copy, the value
    // a by-value parameter would carry.
    if ty == PlanType::StringRef {
        if !matches!(
            read.fact,
            OwnershipFact::Move | OwnershipFact::Clone | OwnershipFact::Borrow
        ) {
            return Err("unsupported Body IR shared text value ownership".into());
        }
        let text_place = clone_text_reference(builder, read_place(place, false, span), span)?;
        return Ok(read_place(text_place, true, span));
    }
    scalar_operand(builder, value, span)
}

/// Map one collection root, materializing immutable frozen wrappers into the owned list used by every read path.
pub fn list_base_place(builder: &mut Builder, source: &SourcePlace, span: &SourceSpan) -> Result<Place, String> {
    if source.global().is_some() {
        return Err(untranslated("static_storage::read_static"));
    }
    match source.local_id() {
        Some(local) => {
            let place = plan_place(local.index() as i64 + 1 + builder.local_offset, span);
            collection_read_storage(builder, place, span)
        }
        None => Err("unsupported Body IR global list place".into()),
    }
}

/// Normalize a borrowed frozen wrapper to its immutable referent while leaving every owned root unchanged.
pub fn collection_read_storage(builder: &Builder, place: Place, span: &SourceSpan) -> Result<Place, String> {
    match place_type(&builder.locals, &place) {
        PlanType::FrozenCollectionRef(name, elements) => {
            dereferenced_place(&place, PlanType::FrozenCollection(name, elements), span)
        }
        _ => Ok(place),
    }
}
