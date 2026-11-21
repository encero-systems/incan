//! Borrow checked list or dictionary element storage through the runtime's bounds-checked accessors.
//!
//! Owned reads materialize elements. A checked shared list loan preserves the stored value without requiring Clone;
//! a `mut self` method or a field write changes the stored element. Field projections follow the resulting loan.

use incan_mir_plan_native::{
    Operand, Place, PlanType, Projection, RvalueKind, SourceSpan, assign as plan_assign, place as plan_place,
};
use incan_semantics_core::body_ir::{
    ArgumentElement, BodyIrModule, Operand as SourceOperand, OwnershipFact, Place as SourcePlace, PlaceElem,
};

use crate::builtins::list_call;
use crate::calls::instantiated_external_call;
use crate::checked_values::scalar_operand;
use crate::classes::frame_type;
use crate::collections::{
    borrow_collection, collection_arguments, collection_call, collection_owned, collection_reference, is_collection,
};
use crate::element_places::{element_place_type, element_prefix_type, is_element_place};
use crate::frame::{Builder, appended_field, place_type, temporary};
use crate::layout_types::shared_receiver_type;
use crate::list_methods::materialized_receiver;
use crate::list_values::{list_borrow, value_operand};
use crate::lists::{is_exclusive_borrow, is_list, list_element, require_list_shape};
use crate::model_lists::{prefix_place, structural_prefix};
use crate::models::field_steps;
use crate::operand_borrows::list_slice_view;
use crate::tuples::is_tuple_type;

/// Reach a checked shared list element loan without copying its owner or requiring Clone.
///
/// 1. Admit a list indexing path before evaluating any index.
/// 2. Borrow each indexed element and project any structural fields through that loan.
pub fn shared_element_place(
    builder: &mut Builder,
    source: &SourcePlace,
    span: &SourceSpan,
) -> Result<Option<Place>, String> {
    // 1. Other container reads retain their existing materialization path.
    if !is_shared_list_path(builder, source)? {
        return Ok(None);
    }
    let start = element_start(builder, source)?;
    let mut place = local_root(builder, source, span)?;
    if start > 0 {
        place = prefix_place(builder, source, start, span)?;
    }
    // 2. Each index is evaluated once; the runtime owns negative-index and bounds checks.
    for projection in &source.projection[start..] {
        place = match projection {
            PlaceElem::Index(index) if !is_tuple_type(&place_type(&builder.locals, &place)) => {
                let argument = scalar_operand(builder, index, span)?;
                shared_list_element(builder, place, argument, span)?
            }
            _ => projected_field(builder, place, projection, span)?,
        };
    }
    Ok(Some(place))
}

/// Admit only list indices and structural fields, without evaluating a partial path that might be retried.
fn is_shared_list_path(builder: &Builder, source: &SourcePlace) -> Result<bool, String> {
    let mut indexed = false;
    for (position, projection) in source.projection.iter().enumerate() {
        match projection {
            PlaceElem::Index(_) => {
                let ty = element_prefix_type(builder, source, position)?;
                if !is_list(&ty) && !is_tuple_type(&ty) {
                    return Ok(false);
                }
                indexed = indexed || is_list(&ty);
            }
            PlaceElem::Slice { .. } => return Ok(false),
            _ => {}
        }
    }
    Ok(indexed)
}

/// Call the checked runtime accessor returning a shared element reference, with no owned element read.
pub fn shared_list_element(
    builder: &mut Builder,
    container: Place,
    index: Operand,
    span: &SourceSpan,
) -> Result<Place, String> {
    let (leaf, depth) = require_list_shape(&place_type(&builder.locals, &container), "invalid shared list loan")?;
    let element = list_element(&leaf, depth);
    let reference = shared_receiver_type(element.clone())?;
    let output = temporary(builder, reference.clone(), span);
    let loan = list_borrow(builder, container, false, span)?;
    let borrowed = list_slice_view(builder, loan, std::slice::from_ref(&element), false, span)?;
    instantiated_external_call(
        builder,
        "incan_std_primitives::collections::list_get",
        vec![element.clone()],
        vec![PlanType::SliceRef(vec![element.clone()]), PlanType::Int],
        reference,
        vec![borrowed, index],
        output.clone(),
        span,
    )?;
    Ok(Place {
        local: output.local,
        projection: Projection::Deref(element),
        span: span.clone(),
    })
}

/// The model or source enum owner of a list or dictionary element receiver; `None` when a native place names it.
pub fn element_method_owner(builder: &Builder, argument: &ArgumentElement) -> Result<Option<PlanType>, String> {
    let Some(SourceOperand::Place(read)) = argument.as_one() else {
        return Ok(None);
    };
    if !is_element_place(builder, &read.place)? {
        return Ok(None);
    }
    match element_place_type(builder, &read.place)? {
        PlanType::Model(owner, name) => return Ok(Some(PlanType::Model(owner, name))),
        PlanType::Enum(owner, name) if owner < builder.enums().len() as i64 => {
            return Ok(Some(PlanType::Enum(owner, name)));
        }
        _ => {}
    }
    Err("unsupported Body IR Method on a non-nominal collection element".into())
}

/// Borrow a list or dictionary element as the selected method's receiver: in place when `mut self` changes it.
pub fn element_method_receiver(
    module: &BodyIrModule,
    builder: &mut Builder,
    index: usize,
    argument: &ArgumentElement,
    span: &SourceSpan,
) -> Result<Place, String> {
    let receiver_type = frame_type(
        builder.constructors(),
        builder.nominals(),
        builder.enums(),
        &module.bodies[index].locals[0],
        &builder.intrinsics,
        "",
    )?;
    let Some(SourceOperand::Place(read)) = argument.as_one() else {
        return Err("unsupported Body IR Method nonplace or spread receiver".into());
    };
    // Body IR borrows every source receiver; the selected body's `mut self` decides the borrow's exclusivity.
    if read.fact != OwnershipFact::Borrow && read.fact != OwnershipFact::MutBorrow {
        return Err("unsupported Body IR Method receiver ownership".into());
    }
    match receiver_type {
        PlanType::ModelMutRef(..) => mutable_element_reference(builder, &read.place, span),
        PlanType::ModelRef(..) | PlanType::EnumRef(..) => {
            shared_element_receiver(builder, &read.place, receiver_type, span)
        }
        _ => Err("invalid Body IR Method receiver type".into()),
    }
}

/// Borrow the selected element in place when list storage can provide a checked shared loan.
fn shared_element_receiver(
    builder: &mut Builder,
    source: &SourcePlace,
    receiver_type: PlanType,
    span: &SourceSpan,
) -> Result<Place, String> {
    if let Some(selected) = shared_element_place(builder, source, span)? {
        let reference = temporary(builder, receiver_type, span);
        builder.emit(plan_assign(reference.clone(), RvalueKind::Borrow(selected), span));
        return Ok(reference);
    }
    let copied = materialized_receiver(builder, source, span)?;
    let reference = temporary(builder, receiver_type, span);
    builder.emit(plan_assign(reference.clone(), RvalueKind::Borrow(copied), span));
    Ok(reference)
}

/// Reborrow the nominal element, or a nominal field inside it, that a list or dictionary place selects.
pub fn mutable_element_reference(
    builder: &mut Builder,
    source: &SourcePlace,
    span: &SourceSpan,
) -> Result<Place, String> {
    let place = mutable_element_place(builder, source, span)?;
    model_reference(builder, place, span)
}

/// Reach the storage a list or dictionary place selects, so a change through it lands in the container.
///
/// Each index reborrows its element or value through the runtime's checked accessor, and each field after it projects
/// through the resulting reference. The result is that reference, or a field place behind it.
pub fn mutable_element_place(builder: &mut Builder, source: &SourcePlace, span: &SourceSpan) -> Result<Place, String> {
    mutable_element_prefix(builder, source, source.projection.len(), span)
}

/// Reborrow checked element storage up to `count` projections, retaining the container before an assignment's final
/// index.
pub fn mutable_element_prefix(
    builder: &mut Builder,
    source: &SourcePlace,
    count: usize,
    span: &SourceSpan,
) -> Result<Place, String> {
    if count > source.projection.len() {
        return Err("invalid Body IR element prefix length".into());
    }
    let start = element_start(builder, source)?;
    let mut place = local_root(builder, source, span)?;
    if start > 0 {
        place = prefix_place(builder, source, start, span)?;
    }
    for position in start..count {
        let projection = &source.projection[position];
        place = match projection {
            PlaceElem::Index(index) => {
                if is_tuple_type(&place_type(&builder.locals, &place)) {
                    projected_field(builder, place, projection, span)?
                } else {
                    borrow_element(builder, place, index, span)?
                }
            }
            PlaceElem::Slice { .. } => return Err("unsupported Body IR mutable borrow through a list slice".into()),
            _ => projected_field(builder, place, projection, span)?,
        };
    }
    Ok(place)
}

/// Project a checked model or constant tuple field through its retained element reference.
fn projected_field(
    builder: &mut Builder,
    place: Place,
    projection: &PlaceElem,
    span: &SourceSpan,
) -> Result<Place, String> {
    let owner = dereferenced(builder, place);
    let steps = field_steps(builder.tables(), &place_type(&builder.locals, &owner), projection)?;
    let mut projected = owner;
    for step in steps {
        projected = appended_field(&projected, step, span)?;
    }
    Ok(projected)
}

/// Reborrow one nested list or nominal list element through the legacy negative-index and bounds checks.
pub fn borrow_element_mut(
    builder: &mut Builder,
    receiver: Place,
    index: Operand,
    span: &SourceSpan,
) -> Result<Place, String> {
    let ty = place_type(&builder.locals, &receiver);
    let site = format!("{}:{}:{} in {}", builder.file, span.line, span.column, builder.name);
    let refusal = format!("unsupported Body IR indexing non-list at {site}; type {ty:?}; exclusive element borrow");
    let (leaf, depth) = require_list_shape(&ty, &refusal)?;
    let element = list_element(&leaf, depth);
    let result = mutable_reference_type(&element)?;
    let reference = list_borrow(builder, receiver, true, span)?;
    let output = temporary(builder, result.clone(), span);
    list_call(
        builder,
        "borrow_list_mut",
        element,
        vec![PlanType::ListMutRef(leaf, depth), PlanType::Int],
        result,
        vec![reference, index],
        output.clone(),
        span,
    );
    Ok(output)
}

/// Count the structural steps before the first element index; a list or dictionary root has none.
fn element_start(builder: &Builder, source: &SourcePlace) -> Result<usize, String> {
    let Some(local) = source.local_id() else {
        return Err("unsupported Body IR mutable borrow of a static element".into());
    };
    let root = &builder.locals[(local.index() as i64 + 1 + builder.local_offset) as usize].ty;
    if is_list(root) || is_collection(root) {
        return Ok(0);
    }
    if let Some((count, _)) = structural_prefix(builder, source)?
        && count > 0
        && count < source.projection.len()
    {
        return Ok(count);
    }
    Err("unsupported Body IR mutable borrow of a non-element place".into())
}

/// Map the place's frame local; static storage is a snapshot, so a borrow of it could never change the static.
fn local_root(builder: &Builder, source: &SourcePlace, span: &SourceSpan) -> Result<Place, String> {
    match source.local_id() {
        Some(local) => Ok(plan_place(local.index() as i64 + 1 + builder.local_offset, span)),
        None => Err("unsupported Body IR mutable borrow of a static element".into()),
    }
}

/// Evaluate the index or key first, then reborrow the selected list element or dictionary value.
fn borrow_element(
    builder: &mut Builder,
    container: Place,
    index: &SourceOperand,
    span: &SourceSpan,
) -> Result<Place, String> {
    if is_collection(&place_type(&builder.locals, &container)) {
        return borrow_dictionary_value(builder, container, index, span);
    }
    let argument = scalar_operand(builder, index, span)?;
    borrow_element_mut(builder, container, argument, span)
}

/// Reborrow a stored dictionary value through legacy's missing-key diagnostic.
///
/// The dictionary is a whole local or reference, or an owned dictionary field such as a model's; a reference reached
/// through a projection keeps its refusal, because the reborrow dereferences only a whole reference.
fn borrow_dictionary_value(
    builder: &mut Builder,
    container: Place,
    index: &SourceOperand,
    span: &SourceSpan,
) -> Result<Place, String> {
    let ty = place_type(&builder.locals, &container);
    if container.projection != Projection::Whole && collection_owned(&ty) != ty {
        return Err("unsupported Body IR mutable borrow of a projected dictionary value".into());
    }
    let types = collection_arguments(&ty);
    if types.len() != 2 {
        return Err("unsupported Body IR set indexing".into());
    }
    // Legacy dictionary indexing requires Display for its missing-key diagnostic; tuples and unit lack that bound.
    match types[0] {
        PlanType::Tuple(_) => {
            return Err("tuple dictionary keys cannot satisfy the lookup's Display requirement".into());
        }
        PlanType::Unit => return Err("unit dictionary keys cannot satisfy the lookup's Display requirement".into()),
        _ => {}
    }
    let result = mutable_reference_type(&types[1])?;
    let key = value_operand(builder, index, span)?;
    let reference = borrow_collection(builder, &container, true, span)?;
    let output = temporary(builder, result.clone(), span);
    collection_call(
        builder,
        &ty,
        "borrow_mut",
        vec![collection_reference(&ty, true), types[0].clone()],
        result,
        vec![reference, key],
        output.clone(),
        span,
    )?;
    Ok(output)
}

/// The exclusive reference a reborrowed element takes; scalar elements are written by index instead.
fn mutable_reference_type(element: &PlanType) -> Result<PlanType, String> {
    if is_exclusive_borrow(element) {
        return Ok(PlanType::ScalarMutRef(vec![element.clone()]));
    }
    match element {
        PlanType::List(leaf, depth) => Ok(PlanType::ListMutRef(leaf.clone(), *depth)),
        PlanType::Model(owner, name) => Ok(PlanType::ModelMutRef(*owner, name.clone())),
        PlanType::Dict(key, value) => Ok(PlanType::DictMutRef(key.clone(), value.clone())),
        PlanType::Set(leaf) => Ok(PlanType::SetMutRef(leaf.clone())),
        PlanType::Tuple(elements) => Ok(PlanType::TupleMutRef(elements.clone())),
        _ => Err("unsupported Body IR mutable borrow of a scalar collection element".into()),
    }
}

/// Project through a whole nominal reference so its fields append to the referent rather than the reference.
fn dereferenced(builder: &Builder, place: Place) -> Place {
    if place.projection != Projection::Whole {
        return place;
    }
    let referent = match &builder.locals[place.local as usize].ty {
        PlanType::ModelMutRef(owner, name) => PlanType::Model(*owner, name.clone()),
        PlanType::TupleMutRef(elements) => PlanType::Tuple(elements.clone()),
        _ => return place,
    };
    Place {
        local: place.local,
        projection: Projection::Deref(referent),
        span: place.span,
    }
}

/// Return the element's exclusive reference, reborrowing a field path inside it as a reference of its own.
fn model_reference(builder: &mut Builder, place: Place, span: &SourceSpan) -> Result<Place, String> {
    match place_type(&builder.locals, &place) {
        PlanType::ModelMutRef(..) if place.projection == Projection::Whole => Ok(place),
        PlanType::Model(owner, name) => {
            let reference = temporary(builder, PlanType::ModelMutRef(owner, name), span);
            builder.emit(plan_assign(reference.clone(), RvalueKind::MutBorrow(place), span));
            Ok(reference)
        }
        _ => Err("unsupported Body IR mutable Method receiver on a non-nominal element".into()),
    }
}
