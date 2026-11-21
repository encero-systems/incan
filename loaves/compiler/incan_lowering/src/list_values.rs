//! Borrow, clone, read, slice, and assign list values and their elements.

use incan_mir_plan_native::{
    Constant, ListLeaf, Operand, OperandKind, Place, PlanType, Projection, RvalueKind, SourceSpan,
    assign as plan_assign, place as plan_place,
};
use incan_semantics_core::body_ir::{
    AggregateKind, ArgumentElement, Constant as SourceConstant, Operand as SourceOperand, OwnershipFact,
    Place as SourcePlace, PlaceElem, Rvalue,
};

use crate::array_views::borrow_array_slice;
use crate::builtins::list_call;
use crate::bytes_plan::bytes_literal;
use crate::calls::instantiated_external_call;
use crate::carrier_operands::{boundary_operand, element_operand};
use crate::checked_values::{reference_value_read, scalar_operand, scalar_place, validate_callback_place};
use crate::closures::callable_copy_type;
use crate::collections::{borrow_collection, is_collection, is_owned_collection};
use crate::element_borrows::{mutable_element_place, shared_element_place};
use crate::element_places::is_element_place;
use crate::explicit_arguments::reference_contract;
use crate::formatting::display_plan_operand;
use crate::frame::{
    Builder, appended_field, dereferenced_place, drop_owned, external_call, forget_owned, literal, place_type,
    read_place, temporary, value_type,
};
use crate::frozen_plan::frozen_observation_type;
use crate::generators::generator_collect;
use crate::list_iteration::{frozen_list_snapshot_type, list_root_place};
use crate::lists::{is_owned_list, list_element, list_shape, require_list_shape};
use crate::model_lists::model_list_read;
use crate::models::field_steps;
use crate::numerics::numeric_carrier;
use crate::options::replace_owned_destination;
use crate::pending::untranslated;
use crate::statements::is_nominal_type;
use crate::strings::emit_text_helper;
use crate::text_plan::{clone_text, clone_text_reference, owned_literal};
use crate::tuple_iterators::zip_shape;
use crate::tuples::is_tuple_type;

/// Borrow an owner or reborrow a parameter; frozen bytes materialize only for shared access.
pub fn list_borrow(builder: &mut Builder, place: Place, mutable: bool, span: &SourceSpan) -> Result<Operand, String> {
    if mutable && builder.borrowed_items.contains(&place.local) {
        return Err("unsupported Body IR mutation of borrowed list iteration item".into());
    }
    let ty = place_type(&builder.locals, &place);
    if is_collection(&ty) {
        return borrow_collection(builder, &place, mutable, span);
    }
    if ty == PlanType::FrozenBytes && !mutable {
        // A read-only observation may own a copy; it cannot grant mutation authority over a constant.
        let storage = sequence_read_storage(builder, place, span)?;
        return list_borrow(builder, storage, false, span);
    }
    let (leaf, depth) = require_list_shape(&ty, "unsupported Body IR non-list borrow")?;
    let mut source = place;
    match ty {
        PlanType::ListRef(..) => {
            if mutable {
                return Err("unsupported Body IR mutable access through shared list parameter".into());
            }
            source.projection = Projection::Deref(PlanType::List(leaf.clone(), depth));
        }
        PlanType::ListMutRef(..) => source.projection = Projection::Deref(PlanType::List(leaf.clone(), depth)),
        _ => {}
    }
    let (reference_type, operation) = if mutable {
        (PlanType::ListMutRef(leaf, depth), RvalueKind::MutBorrow(source))
    } else {
        (PlanType::ListRef(leaf, depth), RvalueKind::Borrow(source))
    };
    let reference = temporary(builder, reference_type, span);
    builder.emit(plan_assign(reference.clone(), operation, span));
    Ok(read_place(reference, mutable, span))
}

/// Preserve checked exclusive-access authority before borrowing a local for an Incan parameter or method receiver.
///
/// An exclusive borrow also takes a value the caller last uses (a list literal, a temporary) in place, since nothing
/// reads it again. A copied loop variable stays refused for an exclusive borrow, because a change to it could never
/// reach the iterated list.
pub fn checked_list_argument(
    builder: &mut Builder,
    value: &SourceOperand,
    mutable: bool,
    span: &SourceSpan,
) -> Result<Operand, String> {
    let read = match value {
        SourceOperand::Place(read) => read,
        SourceOperand::Constant(constant @ SourceConstant::Bytes(_)) => {
            if mutable {
                return Err("unsupported Body IR mutable byte literal argument".into());
            }
            let literal_place = bytes_literal(builder, constant, span)?;
            return list_borrow(builder, literal_place, false, span);
        }
        SourceOperand::Constant(_) => return Err("unsupported Body IR constant list argument".into()),
    };
    if mutable && read.place.global().is_some() {
        return Err("unsupported Body IR mutable static borrow through source function".into());
    }
    let sliced = has_slice_projection(&read.place);
    if mutable && read.fact != OwnershipFact::MutBorrow && read.fact != OwnershipFact::Move && !sliced {
        return Err("unsupported Body IR list argument without mutable borrow fact".into());
    }
    if is_element_place(builder, &read.place)? {
        // A shared parameter and a fresh projected owner read through a copy that stays owned until its scope ends.
        // A slice is such an owner: its Move fact distinguishes it from stored indexed list storage.
        if !mutable || read.fact == OwnershipFact::Move || sliced {
            return Err(untranslated("list_methods::materialized_receiver"));
        }
        // An exclusive argument reaches the list stored in the container, so its change lands there.
        let stored = mutable_element_place(builder, &read.place, span)?;
        return list_borrow(builder, stored, true, span);
    }
    let root = list_root_place(builder, &read.place, span)?;
    if place_type(&builder.locals, &root) == PlanType::Generator(ListLeaf::Int, 0) {
        if read.fact != OwnershipFact::Move && !read.last_use {
            return Err("unsupported Body IR list argument without an owned range".into());
        }
        let snapshot = temporary(builder, PlanType::List(ListLeaf::Int, 1), span);
        generator_collect(builder, root, snapshot.clone(), span)?;
        builder.owned.push(snapshot.local);
        return list_borrow(builder, snapshot, mutable, span);
    }
    list_borrow(builder, root, mutable, span)
}

/// Recognize a projection that materializes a fresh sequence owner instead of naming stored element storage.
pub fn has_slice_projection(source: &SourcePlace) -> bool {
    source
        .projection
        .iter()
        .any(|projection| matches!(projection, PlaceElem::Slice { .. }))
}

/// Copy a list into a new owned temporary, which drops when its scope ends.
pub fn cloned_list(builder: &mut Builder, source: Place, span: &SourceSpan) -> Result<Place, String> {
    let (leaf, depth) = require_list_shape(
        &place_type(&builder.locals, &source),
        "unsupported Body IR non-list borrow",
    )?;
    let reference = list_borrow(builder, source, false, span)?;
    let output = temporary(builder, PlanType::List(leaf.clone(), depth), span);
    list_call(
        builder,
        "clone_list",
        list_element(&leaf, depth),
        vec![PlanType::ListRef(leaf.clone(), depth)],
        PlanType::List(leaf, depth),
        vec![reference],
        output.clone(),
        span,
    );
    Ok(output)
}

/// Materialize owned list and string values using each checked read's fact.
pub fn value_operand(builder: &mut Builder, value: &SourceOperand, span: &SourceSpan) -> Result<Operand, String> {
    let read = match value {
        SourceOperand::Constant(constant @ SourceConstant::Bytes(_)) => {
            return Ok(read_place(bytes_literal(builder, constant, span)?, true, span));
        }
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
        return Ok(read_place(cloned_list(builder, place, span)?, true, span));
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

/// Copy the selected value without changing its source storage; list field reads borrow intermediate elements.
pub fn element_read(builder: &mut Builder, source: &SourcePlace, span: &SourceSpan) -> Result<Option<Operand>, String> {
    if let Some(local) = source.local_id() {
        if let Some(PlaceElem::Field { .. }) = source.projection.last()
            && let Some(field) = shared_element_place(builder, source, span)?
        {
            let selected = materialize_element_field(builder, field, span)?;
            let owned = element_field_is_owned(builder, &selected);
            return Ok(Some(read_place(selected, owned, span)));
        }
        let root = plan_place(local.index() as i64 + 1 + builder.local_offset, span);
        let projected = !source.projection.is_empty();
        match place_type(&builder.locals, &root) {
            PlanType::String | PlanType::FrozenStr | PlanType::FrozenBytes if projected => {
                return Ok(Some(projected_element_read(builder, source, root, 0, span)?));
            }
            PlanType::StringRef | PlanType::StringMutRef if projected => {
                let text = Place {
                    local: root.local,
                    projection: Projection::Deref(PlanType::String),
                    span: span.clone(),
                };
                return Ok(Some(projected_element_read(builder, source, text, 0, span)?));
            }
            PlanType::NativeArray(..) | PlanType::FrozenCollection(..) if projected => {
                return Ok(Some(projected_element_read(builder, source, root, 0, span)?));
            }
            _ => {}
        }
    }
    if let Some(item) = model_list_read(builder, source, span)? {
        return Ok(Some(item));
    }
    if !is_element_place(builder, source)? {
        return Ok(None);
    }
    let base = list_base_place(builder, source, span)?;
    if is_collection(&builder.locals[base.local as usize].ty) {
        return Err(untranslated("hashed_collections::dictionary_index_read"));
    }
    Ok(Some(indexed_read(builder, source, span)?))
}

/// Use the retained list projection and legacy negative-index error behavior; a final slice copies a new list.
pub fn indexed_read(builder: &mut Builder, source: &SourcePlace, span: &SourceSpan) -> Result<Operand, String> {
    if source.projection.is_empty() {
        return Err("unsupported Body IR list read without index".into());
    }
    let container = list_base_place(builder, source, span)?;
    projected_element_read(builder, source, container, 0, span)
}

/// Copy the value the indexes, slices and element fields after `start` steps select out of `container`.
///
/// Each index clones its element, or the value its key selects in a dictionary, and each slice copies a new sequence,
/// so the container stays whole; a field of a copied element is read from that copy.
pub fn projected_element_read(
    builder: &mut Builder,
    source: &SourcePlace,
    container: Place,
    start: usize,
    span: &SourceSpan,
) -> Result<Operand, String> {
    let mut place = container;
    for slot in start..source.projection.len() {
        // A preceding list index or field can expose frozen storage just as the root can.
        place = sequence_read_storage(builder, place, span)?;
        let projection = &source.projection[slot];
        place = match projection {
            PlaceElem::Slice { .. } => read_sequence_slice(builder, place, projection, span)?,
            PlaceElem::Field { .. } | PlaceElem::OptionPayload { .. } | PlaceElem::UnionMember { .. } => {
                read_element_field(builder, place, projection, span)?
            }
            _ => read_indexed_element(builder, place, source, slot, span)?,
        };
    }
    let owned = element_field_is_owned(builder, &place);
    Ok(read_place(place, owned, span))
}

/// Materialize immutable frozen text and bytes through their canonical runtime observations before a sequence read.
pub fn sequence_read_storage(builder: &mut Builder, source: Place, span: &SourceSpan) -> Result<Place, String> {
    let ty = place_type(&builder.locals, &source);
    if ty == PlanType::FrozenStr {
        return display_plan_operand(builder, read_place(source, false, span), span);
    }
    if ty == PlanType::FrozenBytes {
        let output = temporary(builder, PlanType::List(ListLeaf::U8, 1), span);
        external_call(
            builder,
            "incan_native_runtime::copy_frozen_bytes",
            vec![ty],
            PlanType::List(ListLeaf::U8, 1),
            vec![read_place(source, false, span)],
            output.clone(),
            span,
        );
        builder.owned.push(output.local);
        return Ok(output);
    }
    Ok(source)
}

/// Read a checked tuple field, Unicode scalar, frozen item, or cloned collection element from the place earlier steps
/// reached.
fn read_indexed_element(
    builder: &mut Builder,
    place: Place,
    source: &SourcePlace,
    slot: usize,
    span: &SourceSpan,
) -> Result<Place, String> {
    let ty = place_type(&builder.locals, &place);
    if is_tuple_type(&ty) {
        return read_element_field(builder, place, &source.projection[slot], span);
    }
    match &ty {
        PlanType::NativeArray(elements, _) => {
            let index = list_index_operand(builder, source, slot, span)?;
            return read_native_text_array(builder, place, elements, index, span);
        }
        PlanType::FrozenCollection(name, _) => {
            if name == "FrozenDict" {
                return Err(untranslated("list_iteration::frozen_dictionary_element"));
            }
            return Err(untranslated("list_iteration::frozen_list_snapshot"));
        }
        _ => {}
    }
    if is_collection(&ty) {
        return Err(untranslated("hashed_collections::read_dictionary_element"));
    }
    let argument = list_index_operand(builder, source, slot, span)?;
    if ty == PlanType::String {
        return read_text_element(builder, place, argument, span);
    }
    read_list_element(builder, place, argument, span)
}

/// Borrow a native array of sized text references as a slice and reuse core's checked signed-index operation.
fn read_native_text_array(
    builder: &mut Builder,
    place: Place,
    elements: &[PlanType],
    index: Operand,
    span: &SourceSpan,
) -> Result<Place, String> {
    if elements != [PlanType::StrRef] {
        return Err("unsupported Body IR native array element carrier".into());
    }
    let Some(view) = borrow_array_slice(builder, &place, elements, false, &OwnershipFact::Borrow, span)? else {
        return Err("unsupported Body IR native array element read without an array carrier".into());
    };
    let result_type = PlanType::ScalarRef(vec![PlanType::StrRef]);
    let result = temporary(builder, result_type.clone(), span);
    instantiated_external_call(
        builder,
        "incan_std_primitives::collections::list_get",
        vec![PlanType::StrRef],
        vec![PlanType::SliceRef(elements.to_vec()), PlanType::Int],
        result_type,
        vec![view, index],
        result.clone(),
        span,
    )?;
    Ok(Place {
        local: result.local,
        projection: Projection::Deref(PlanType::StrRef),
        span: span.clone(),
    })
}

/// Borrow retained String storage for the canonical Unicode-scalar index helper, preserving negative indices and
/// IndexError behavior.
fn read_text_element(builder: &mut Builder, place: Place, index: Operand, span: &SourceSpan) -> Result<Place, String> {
    let reference = temporary(builder, PlanType::StringRef, span);
    builder.emit(plan_assign(reference.clone(), RvalueKind::Borrow(place), span));
    let output = temporary(builder, PlanType::String, span);
    instantiated_external_call(
        builder,
        "incan_std_primitives::strings::str_index",
        vec![PlanType::StringRef],
        vec![PlanType::StringRef, PlanType::Int],
        PlanType::String,
        vec![read_place(reference, false, span), index],
        output.clone(),
        span,
    )?;
    Ok(output)
}

/// Clone a projected element instead of moving caller or container-owned storage.
pub fn read_list_element(
    builder: &mut Builder,
    place: Place,
    argument: Operand,
    span: &SourceSpan,
) -> Result<Place, String> {
    let ty = place_type(&builder.locals, &place);
    let site = format!("{}:{}:{} in {}", builder.file, span.line, span.column, builder.name);
    let refusal = format!("unsupported Body IR indexing non-list at {site}; type {ty:?}; element read");
    let (leaf, depth) = require_list_shape(&ty, &refusal)?;
    let reference = list_borrow(builder, place, false, span)?;
    let element = list_element(&leaf, depth);
    let output = temporary(builder, element.clone(), span);
    list_call(
        builder,
        "get_list",
        element.clone(),
        vec![PlanType::ListRef(leaf, depth), PlanType::Int],
        element,
        vec![reference, argument],
        output.clone(),
        span,
    );
    Ok(output)
}

/// Read each retained slice bound once, retaining the distinction between omitted and written zero.
fn sequence_slice_bounds(
    builder: &mut Builder,
    projection: &PlaceElem,
    span: &SourceSpan,
) -> Result<Vec<Operand>, String> {
    let PlaceElem::Slice { start, end, step } = projection else {
        return Err("unsupported Body IR sequence slice or field projection".into());
    };
    // Each component travels with its presence, so an omitted bound or step keeps the slice default.
    let mut bounds = Vec::with_capacity(6);
    for component in [start, end, step] {
        match component {
            Some(value) => bounds.extend(slice_bound(builder, value, span)?),
            None => bounds.extend(absent_slice_bound(span)),
        }
    }
    Ok(bounds)
}

/// Copy a typed string or list slice through its own legacy runtime boundary.
fn read_sequence_slice(
    builder: &mut Builder,
    place: Place,
    projection: &PlaceElem,
    span: &SourceSpan,
) -> Result<Place, String> {
    if place_type(&builder.locals, &place) != PlanType::String {
        return read_list_slice(builder, place, projection, span);
    }
    let bounds = sequence_slice_bounds(builder, projection, span)?;
    let reference = temporary(builder, PlanType::StringRef, span);
    builder.emit(plan_assign(reference.clone(), RvalueKind::Borrow(place), span));
    let mut operands = vec![read_place(reference, false, span)];
    operands.extend(bounds);
    Ok(emit_text_helper(
        builder,
        "slice_text",
        PlanType::String,
        vec![
            PlanType::StringRef,
            PlanType::Bool,
            PlanType::Int,
            PlanType::Bool,
            PlanType::Int,
            PlanType::Bool,
            PlanType::Int,
        ],
        operands,
        span,
    ))
}

/// Copy the selected elements into a new owned list through legacy's Python-style slice helper.
fn read_list_slice(
    builder: &mut Builder,
    place: Place,
    projection: &PlaceElem,
    span: &SourceSpan,
) -> Result<Place, String> {
    let (leaf, depth) = require_list_shape(
        &place_type(&builder.locals, &place),
        "unsupported Body IR slicing non-list",
    )?;
    let bounds = sequence_slice_bounds(builder, projection, span)?;
    let reference = list_borrow(builder, place, false, span)?;
    let output = temporary(builder, PlanType::List(leaf.clone(), depth), span);
    let mut operands = vec![reference];
    operands.extend(bounds);
    list_call(
        builder,
        "slice_list",
        list_element(&leaf, depth),
        vec![
            PlanType::ListRef(leaf.clone(), depth),
            PlanType::Bool,
            PlanType::Int,
            PlanType::Bool,
            PlanType::Int,
            PlanType::Bool,
            PlanType::Int,
        ],
        PlanType::List(leaf, depth),
        operands,
        output.clone(),
        span,
    );
    Ok(output)
}

/// Read a list element field, copying shared references and scalar values while cloning owned values.
fn read_element_field(
    builder: &mut Builder,
    place: Place,
    projection: &PlaceElem,
    span: &SourceSpan,
) -> Result<Place, String> {
    let steps = field_steps(builder.tables(), &place_type(&builder.locals, &place), projection)?;
    let mut field = place;
    for step in steps {
        field = appended_field(&field, step, span)?;
    }
    materialize_element_field(builder, field, span)
}

/// Copy a selected scalar or clone only the selected owned field, leaving its intermediate owners untouched.
///
/// Both copied-element fallbacks and shared list projections use this boundary. Logical shared references stay
/// references; exclusive references cannot be duplicated out of a container.
fn materialize_element_field(builder: &mut Builder, field: Place, span: &SourceSpan) -> Result<Place, String> {
    let ty = place_type(&builder.locals, &field);
    if let Ok((_, writable)) = reference_contract(&ty) {
        if writable {
            return Err("unsupported Body IR exclusive reference field inside a list element".into());
        }
        let output = temporary(builder, ty, span);
        builder.emit(plan_assign(
            output.clone(),
            RvalueKind::Use(read_place(field, false, span)),
            span,
        ));
        return Ok(output);
    }
    if ty == PlanType::String {
        return clone_text(builder, field, span);
    }
    if is_nominal_type(&ty) {
        return Err(untranslated("model_construction::clone_nominal_operand"));
    }
    if is_owned_list(&ty) {
        let (leaf, depth) = require_list_shape(&ty, "invalid Body IR list element field")?;
        let reference = list_borrow(builder, field, false, span)?;
        let output = temporary(builder, ty.clone(), span);
        list_call(
            builder,
            "clone_list",
            list_element(&leaf, depth),
            vec![PlanType::ListRef(leaf, depth)],
            ty,
            vec![reference],
            output.clone(),
            span,
        );
        return Ok(output);
    }
    if is_owned_collection(&ty) {
        return Err(untranslated("hashed_collections::cloned_collection"));
    }
    // Static frozen handles copy only their shared backing pointer, never their elements or the containing record.
    if !numeric_carrier(&ty) && ty != PlanType::Bool && !callable_copy_type(&ty) && !frozen_observation_type(&ty) {
        return Err(format!(
            "unsupported Body IR list element field type {ty:?} in {}",
            builder.name
        ));
    }
    let output = temporary(builder, ty, span);
    builder.emit(plan_assign(
        output.clone(),
        RvalueKind::Use(read_place(field, false, span)),
        span,
    ));
    Ok(output)
}

/// Whether a materialized field result transfers its independent storage to the consuming expression.
fn element_field_is_owned(builder: &Builder, field: &Place) -> bool {
    let ty = place_type(&builder.locals, field);
    ty == PlanType::String || is_owned_list(&ty) || is_nominal_type(&ty) || is_owned_collection(&ty)
}

/// Pass a written slice bound or step as a present int component, presence flag first as `slice_list` takes it.
fn slice_bound(builder: &mut Builder, value: &SourceOperand, span: &SourceSpan) -> Result<Vec<Operand>, String> {
    let operand = scalar_operand(builder, value, span)?;
    if value_type(builder, &operand)? != PlanType::Int {
        return Err("unsupported Body IR non-int list slice component".into());
    }
    Ok(vec![literal(Constant::Bool(true), span), operand])
}

/// Mark an omitted slice bound or step absent, as legacy passes `None` for it.
fn absent_slice_bound(span: &SourceSpan) -> Vec<Operand> {
    vec![literal(Constant::Bool(false), span), literal(Constant::Int(0), span)]
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

/// Read one retained index after the caller has checked the projection count.
pub fn list_index_operand(
    builder: &mut Builder,
    source: &SourcePlace,
    slot: usize,
    span: &SourceSpan,
) -> Result<Operand, String> {
    match &source.projection[slot] {
        PlaceElem::Index(index) => scalar_operand(builder, index, span),
        _ => Err("unsupported Body IR list slice or field projection".into()),
    }
}

/// Construct list literals or transfer checked list values, dropping an initialized destination first.
pub fn list_assignment(
    builder: &mut Builder,
    destination: Place,
    value: &Rvalue,
    span: &SourceSpan,
) -> Result<(), String> {
    if destination.projection == Projection::Whole
        && let PlanType::ListMutRef(leaf, depth) = &builder.locals[destination.local as usize].ty
    {
        // A whole exclusive list input writes through its borrow: the caller's old list drops, then the new value takes
        // its place.
        let referent = dereferenced_place(&destination, PlanType::List(leaf.clone(), *depth), span)?;
        return list_assignment(builder, referent, value, span);
    }
    if destination.projection != Projection::Whole {
        let replacement = temporary(builder, place_type(&builder.locals, &destination), span);
        list_assignment(builder, replacement.clone(), value, span)?;
        replace_owned_destination(
            builder,
            destination,
            RvalueKind::Use(read_place(replacement, true, span)),
            span,
        );
        return Ok(());
    }
    let declared = builder.locals[destination.local as usize].ty.clone();
    let (leaf, depth) = require_list_shape(&declared, "unsupported Body IR non-list assignment")?;
    let ty = PlanType::List(leaf.clone(), depth);
    if declared != ty {
        return Err("unsupported Body IR whole borrowed list assignment".into());
    }
    let element = list_element(&leaf, depth);
    let output = temporary(builder, ty.clone(), span);
    match value {
        Rvalue::Aggregate(kind, elements) => {
            if *kind != AggregateKind::List {
                return Err("unsupported Body IR non-list Aggregate".into());
            }
            list_call(
                builder,
                "empty_list",
                element.clone(),
                Vec::new(),
                ty,
                Vec::new(),
                output.clone(),
                span,
            );
            for item in elements {
                match item.as_one() {
                    Some(value) => {
                        let argument = element_operand(builder, value, &element, span)?;
                        let argument_type = value_type(builder, &argument)?;
                        if argument_type != element {
                            return Err(format!(
                                "unsupported Body IR list element coercion from {argument_type:?} to {element:?}"
                            ));
                        }
                        append_list_value(builder, &output, argument, &leaf, depth, span)?;
                    }
                    None => splice_list_spread(builder, &output, item, &leaf, depth, span)?,
                }
            }
        }
        Rvalue::Use(value) => {
            let argument = boundary_operand(builder, value, &ty, span)?;
            // The checker types a builtin range call as a list, but its value is a lazy range only iteration consumes.
            if value_type(builder, &argument)? != ty {
                return Err("unsupported Body IR list assignment from a non-list value".into());
            }
            if let OperandKind::Move(place) = &argument.kind {
                forget_owned(builder, place.local);
            }
            builder.emit(plan_assign(output.clone(), RvalueKind::Use(argument), span));
            builder.owned.push(output.local);
        }
        _ => return Err("unsupported Body IR list assignment rvalue".into()),
    }
    drop_owned(builder, destination.local, span);
    forget_owned(builder, output.local);
    let local = destination.local;
    builder.emit(plan_assign(
        destination,
        RvalueKind::Use(read_place(output, true, span)),
        span,
    ));
    builder.owned.push(local);
    Ok(())
}

/// Append one checked element to a list literal under construction.
pub fn append_list_value(
    builder: &mut Builder,
    output: &Place,
    value: Operand,
    leaf: &ListLeaf,
    depth: i64,
    span: &SourceSpan,
) -> Result<(), String> {
    let element = list_element(leaf, depth);
    let reference = list_borrow(builder, output.clone(), true, span)?;
    let unit = temporary(builder, PlanType::Unit, span);
    list_call(
        builder,
        "append_list",
        element.clone(),
        vec![PlanType::ListMutRef(leaf.clone(), depth), element],
        PlanType::Unit,
        vec![reference, value],
        unit,
        span,
    );
    Ok(())
}

/// Freeze a list literal: build the list, then keep it for the program as the frozen list's storage.
///
/// Only a computed constant's initializer constructs a frozen list of values that are not literals, and it runs once.
pub fn frozen_list_assignment(
    builder: &mut Builder,
    target: Place,
    value: &Rvalue,
    span: &SourceSpan,
) -> Result<(), String> {
    let ty = place_type(&builder.locals, &target);
    let snapshot = frozen_list_snapshot_type(&ty, false)?;
    let values = temporary(builder, snapshot.clone(), span);
    list_assignment(builder, values.clone(), value, span)?;
    let PlanType::FrozenCollection(_, elements) = &ty else {
        return Err("unsupported Body IR frozen list assignment".into());
    };
    instantiated_external_call(
        builder,
        "incan_native_runtime::freeze_list",
        elements.clone(),
        vec![snapshot],
        ty.clone(),
        vec![read_place(values.clone(), true, span)],
        target,
        span,
    )?;
    forget_owned(builder, values.local);
    Ok(())
}

/// Append a `*source` spread to a list literal at its written position.
fn splice_list_spread(
    builder: &mut Builder,
    output: &Place,
    item: &ArgumentElement,
    leaf: &ListLeaf,
    depth: i64,
    span: &SourceSpan,
) -> Result<(), String> {
    let ArgumentElement::Spread(spread) = item else {
        return Err("unsupported Body IR named list element".into());
    };
    // A whole tuple has a fixed arity, so it splices field by field without a runtime helper.
    if tuple_spread_place(builder, &spread.source, span)?.is_some() {
        return Err(untranslated("tuples::tuple_fields"));
    }
    // A list spread's own ownership fact materializes one owner: the moved list, or a clone of a list that is read
    // again. `extend_list` copies from it and it is then dropped, as set and dictionary spreads do.
    let source = value_operand(builder, &spread.source, span)?;
    let OperandKind::Move(place) = source.kind else {
        return Err("unsupported Body IR list spread without owned source".into());
    };
    if place.projection != Projection::Whole {
        return Err("unsupported Body IR projected list spread owner".into());
    }
    if place_type(&builder.locals, &place) != PlanType::List(leaf.clone(), depth) {
        return Err("unsupported Body IR list spread source type".into());
    }
    let local = place.local;
    let target = list_borrow(builder, output.clone(), true, span)?;
    let reference = list_borrow(builder, place, false, span)?;
    let unit = temporary(builder, PlanType::Unit, span);
    list_call(
        builder,
        "extend_list",
        list_element(leaf, depth),
        vec![
            PlanType::ListMutRef(leaf.clone(), depth),
            PlanType::ListRef(leaf.clone(), depth),
        ],
        PlanType::Unit,
        vec![target, reference],
        unit,
        span,
    );
    drop_owned(builder, local, span);
    Ok(())
}

/// Select a spread source that is a whole tuple local; every other source splices as a list.
fn tuple_spread_place(
    builder: &mut Builder,
    source: &SourceOperand,
    span: &SourceSpan,
) -> Result<Option<Place>, String> {
    let SourceOperand::Place(read) = source else {
        return Ok(None);
    };
    if !read.place.projection.is_empty() || read.place.global().is_some() {
        return Ok(None);
    }
    let place = scalar_place(builder, &read.place, span)?;
    if is_tuple_type(&place_type(&builder.locals, &place)) {
        return Ok(Some(place));
    }
    Ok(None)
}
