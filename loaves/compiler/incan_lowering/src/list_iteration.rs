//! Iterate lists, text, and frozen collections.

use incan_mir_plan_native::{
    BinaryOp, Constant, ListLeaf, Operand, OperandKind, Place, PlanType, Projection, RvalueKind, SourceSpan,
    TerminatorKind, UnaryOp, assign as plan_assign, place as plan_place, tuple_element_type,
};
use incan_semantics_core::body_ir::{
    ArgumentElement, Constant as SourceConstant, IterProtocol, Operand as SourceOperand, OwnershipFact,
    Place as SourcePlace, PlaceElem, Rvalue,
};

use crate::builtins::list_call;
use crate::calls::instantiated_external_call;
use crate::checked_values::scalar_place;
use crate::collections::{collection_arguments, is_owned_collection};
use crate::element_borrows::{mutable_element_place, shared_element_place, shared_list_element};
use crate::element_places::is_element_place;
use crate::formatting::native_format_text;
use crate::frame::{
    Builder, drop_owned, exhaust_loop, external_call, literal, place_type, read_place, temporary, value_type,
};
use crate::frozen_plan::frozen_collection_literal;
use crate::generators::{generator_collect, generator_next, generator_shape};
use crate::hashed_collections::iterable_call;
use crate::layout_types::collection_leaf;
use crate::list_methods::{argument_value, call_destination, materialized_receiver, shared_receiver_place};
use crate::list_values::{collection_read_storage, list_base_place, list_borrow, value_operand};
use crate::lists::{
    exclusive_reference, is_builtin_iteration, is_list, is_mutable_list_reference, list_element, list_shape,
    require_list_shape,
};
use crate::models::field_step_type;
use crate::operand_borrows::borrow_text_operand;
use crate::owned_lists::{is_owned_list_iterator, owned_list_next};
use crate::sequence_builtins::argument_type;
use crate::statements::{drop_loop_owners, is_nominal_type};
use crate::static_storage::computed_static_root;
use crate::tuple_iterators::{zip_next, zip_shape};
use crate::tuples::{is_tuple_type, owned_carrier, tuple_has_owners};

/// Require the checked bare list root before preparing an iterator or receiver borrow.
pub fn operand_list_place(builder: &mut Builder, value: &SourceOperand, span: &SourceSpan) -> Result<Place, String> {
    match value {
        SourceOperand::Place(read) => list_root_place(builder, &read.place, span),
        SourceOperand::Constant(_) => Err("unsupported Body IR constant list place".into()),
    }
}

/// Refuse later changes to an iteration item that is only a snapshot of storage the source program changes.
pub fn borrow_iteration_item(builder: &mut Builder, local: i64) {
    if !builder.borrowed_items.contains(&local) {
        builder.borrowed_items.push(local);
    }
}

/// Report whether a polled pair carries a list, directly or in a nested tuple.
fn pair_holds_list(ty: &PlanType) -> bool {
    match ty {
        PlanType::Tuple(elements) => elements
            .iter()
            .any(|element| pair_holds_list(&tuple_element_type(element))),
        _ => is_list(ty),
    }
}

/// A list bound out of a snapshot iteration item is part of that snapshot, so it stays unchangeable too.
pub fn inherit_borrowed_item(builder: &mut Builder, target: &Place, value: &Rvalue) {
    if target.projection != Projection::Whole || !is_list(&place_type(&builder.locals, target)) {
        return;
    }
    let Rvalue::Use(SourceOperand::Place(read)) = value else {
        return;
    };
    if read.place.projection.is_empty() {
        return;
    }
    if let Some(local) = read.place.local_id() {
        let root = local.index() as i64 + 1 + builder.local_offset;
        if builder.borrowed_items.contains(&root) {
            borrow_iteration_item(builder, target.local);
        }
    }
}

/// Materialize a text iterable's characters once, before its loop, as the one-character strings legacy's loop draws.
///
/// The checked iterator slot then holds that list and is polled through the ordinary list cursor.
pub fn initialize_text_iterator(
    builder: &mut Builder,
    target: Place,
    value: &Rvalue,
    span: &SourceSpan,
) -> Result<(), String> {
    let Rvalue::Use(operand) = value else {
        return Err("unsupported Body IR text iterator initialization".into());
    };
    let reference = borrow_text_operand(builder, operand, span)?;
    let items = PlanType::List(ListLeaf::Str, 1);
    builder.locals[target.local as usize].ty = items.clone();
    let local = target.local;
    external_call(
        builder,
        "incan_native_runtime::text_items",
        vec![PlanType::StringRef],
        items,
        vec![reference],
        target,
        span,
    );
    builder.owned.push(local);
    let cursor = temporary(builder, PlanType::Int, span);
    builder.emit(plan_assign(
        cursor.clone(),
        RvalueKind::Use(literal(Constant::Int(0), span)),
        span,
    ));
    builder.iterators.push((local, cursor.local));
    Ok(())
}

/// Read frozen snapshots or shared collection receivers, optionally materializing frozen text for membership.
pub fn shared_sequence_receiver(
    builder: &mut Builder,
    arguments: &[ArgumentElement],
    span: &SourceSpan,
    text_membership: bool,
) -> Result<Place, String> {
    match argument_type(builder, arguments, 0, span)? {
        PlanType::FrozenCollection(..) | PlanType::FrozenCollectionRef(..) => match arguments[0].as_one() {
            Some(SourceOperand::Place(_)) => {
                let receiver = shared_receiver_place(builder, arguments, 0, span)?;
                let storage = collection_read_storage(builder, receiver, span)?;
                frozen_list_snapshot(builder, read_place(storage, false, span), span, text_membership)
            }
            Some(_) => {
                let value = argument_value(builder, arguments, 0, span)?;
                frozen_list_snapshot(builder, value, span, text_membership)
            }
            None => Err("unsupported Body IR spread frozen sequence receiver".into()),
        },
        _ => shared_receiver_place(builder, arguments, 0, span),
    }
}

/// Read one baked dictionary value using its retained storage arguments, then apply legacy's owned read conversion.
pub fn frozen_dictionary_element(
    builder: &mut Builder,
    receiver: Place,
    source: &SourcePlace,
    slot: usize,
    span: &SourceSpan,
) -> Result<Place, String> {
    let PlanType::FrozenCollection(name, elements) = place_type(&builder.locals, &receiver) else {
        return Err("unsupported Body IR nonfrozen dictionary read".into());
    };
    if name != "FrozenDict" || elements.len() != 2 {
        return Err("unsupported Body IR frozen dictionary shape".into());
    }
    let PlaceElem::Index(value) = &source.projection[slot] else {
        return Err("unsupported Body IR frozen dictionary nonindex projection".into());
    };
    let key = frozen_dictionary_key(builder, &elements[0], value, span)?;
    frozen_dictionary_value(builder, receiver, elements, key, span)
}

/// Convert only the selected stored value to its source read representation; other entries stay untouched.
fn frozen_dictionary_value(
    builder: &mut Builder,
    receiver: Place,
    elements: Vec<PlanType>,
    probe: (Operand, PlanType, bool),
    span: &SourceSpan,
) -> Result<Place, String> {
    let ty = place_type(&builder.locals, &receiver);
    let (key, key_type, text) = probe;
    let path = if text {
        "incan_native_runtime::frozen_dict_text_get"
    } else {
        "incan_native_runtime::frozen_dict_get"
    };
    let value_type = elements[1].clone();
    let raw = temporary(builder, value_type.clone(), span);
    instantiated_external_call(
        builder,
        path,
        elements,
        vec![ty, key_type],
        value_type.clone(),
        vec![read_place(receiver, false, span), key],
        raw.clone(),
        span,
    )?;
    if value_type == PlanType::StaticText {
        return native_format_text(builder, read_place(raw, false, span), false, span);
    }
    if value_type == PlanType::StaticBytes {
        let output = temporary(builder, PlanType::List(ListLeaf::U8, 1), span);
        instantiated_external_call(
            builder,
            "incan_native_runtime::own_static_bytes",
            vec![PlanType::StaticBytes],
            vec![PlanType::StaticBytes],
            PlanType::List(ListLeaf::U8, 1),
            vec![read_place(raw, false, span)],
            output.clone(),
            span,
        )?;
        builder.owned.push(output.local);
        return Ok(output);
    }
    match value_type {
        PlanType::FrozenCollection(..) | PlanType::Int | PlanType::Bool | PlanType::FrozenStr => Ok(raw),
        _ => Err("unsupported Body IR frozen dictionary value read".into()),
    }
}

/// Use text probes for static text/FrozenStr keys and exact scalar probes for the admitted numeric keys.
fn frozen_dictionary_key(
    builder: &mut Builder,
    storage: &PlanType,
    value: &SourceOperand,
    span: &SourceSpan,
) -> Result<(Operand, PlanType, bool), String> {
    if matches!(storage, PlanType::StaticText | PlanType::FrozenStr) {
        let key = borrow_text_operand(builder, value, span)?;
        return Ok((key, PlanType::StringRef, true));
    }
    if !matches!(storage, PlanType::Int | PlanType::Bool) {
        return Err("unsupported Body IR frozen dictionary key storage".into());
    }
    let key = value_operand(builder, value, span)?;
    if value_type(builder, &key)? != *storage {
        return Err("unsupported Body IR frozen dictionary key type".into());
    }
    Ok((key, storage.clone(), false))
}

/// Preserve linear frozen-key membership without reading or cloning values.
pub fn frozen_dictionary_contains(
    builder: &mut Builder,
    arguments: &[ArgumentElement],
    destination: &Option<SourcePlace>,
    negated: bool,
    span: &SourceSpan,
) -> Result<(), String> {
    if arguments.len() != 2 {
        return Err("unsupported Body IR frozen dictionary membership arity".into());
    }
    let ty = argument_type(builder, arguments, 0, span)?;
    let PlanType::FrozenCollection(name, elements) = &ty else {
        return Err("unsupported Body IR nonfrozen dictionary membership".into());
    };
    if name != "FrozenDict" || elements.len() != 2 {
        return Err("unsupported Body IR frozen dictionary membership shape".into());
    }
    let Some(value) = arguments[1].as_one() else {
        return Err("unsupported Body IR spread frozen dictionary membership".into());
    };
    let (key, key_type, text) = frozen_dictionary_key(builder, &elements[0], value, span)?;
    let path = if text {
        "incan_native_runtime::frozen_dict_text_contains"
    } else {
        "incan_native_runtime::frozen_dict_contains"
    };
    let receiver = argument_value(builder, arguments, 0, span)?;
    let output = call_destination(builder, destination, PlanType::Bool, span)?;
    instantiated_external_call(
        builder,
        path,
        elements.clone(),
        vec![ty.clone(), key_type],
        PlanType::Bool,
        vec![receiver, key],
        output.clone(),
        span,
    )?;
    if negated {
        negate(builder, output, span);
    }
    Ok(())
}

/// Replace a boolean result with its negation, as the source's `not in` reads it.
fn negate(builder: &mut Builder, output: Place, span: &SourceSpan) {
    let value = read_place(output.clone(), false, span);
    builder.emit(plan_assign(output, RvalueKind::Unary(UnaryOp::Not, value), span));
}

/// Keep ordinary elements exact while allowing membership to compare owned text with frozen text.
pub fn frozen_list_snapshot_type(ty: &PlanType, text_membership: bool) -> Result<PlanType, String> {
    let PlanType::FrozenCollection(name, elements) = ty else {
        return Err("unsupported Body IR nonfrozen snapshot".into());
    };
    if !matches!(name.as_str(), "FrozenList" | "FrozenSet" | "FrozenDict") || elements.is_empty() {
        return Err("unsupported Body IR frozen collection snapshot".into());
    }
    let element = &elements[0];
    if *element == PlanType::StaticText || (text_membership && *element == PlanType::FrozenStr) {
        return Ok(PlanType::List(ListLeaf::Str, 1));
    }
    if *element == PlanType::StaticBytes {
        if name != "FrozenList" {
            return Err("unsupported Body IR frozen byte set or dictionary snapshot".into());
        }
        return Ok(PlanType::List(ListLeaf::U8, 2));
    }
    if let PlanType::Model(..) | PlanType::FrozenCollection(..) = element {
        if name != "FrozenList" {
            return Err("unsupported Body IR frozen nominal set or dictionary snapshot".into());
        }
        return Ok(PlanType::List(collection_leaf(element)?, 1));
    }
    if !matches!(
        element,
        PlanType::Int | PlanType::Float | PlanType::Bool | PlanType::FrozenStr | PlanType::FrozenBytes
    ) {
        return Err("unsupported Body IR frozen collection snapshot with nonprimitive elements".into());
    }
    Ok(PlanType::List(collection_leaf(element)?, 1))
}

/// Materialize an ordered owned snapshot, converting frozen text only for cross-text membership.
pub fn frozen_list_snapshot(
    builder: &mut Builder,
    value: Operand,
    span: &SourceSpan,
    text_membership: bool,
) -> Result<Place, String> {
    let ty = value_type(builder, &value)?;
    let snapshot = frozen_list_snapshot_type(&ty, text_membership)?;
    let PlanType::FrozenCollection(name, elements) = &ty else {
        return Err("unsupported Body IR nonfrozen snapshot".into());
    };
    let text = elements[0] == PlanType::StaticText || (text_membership && elements[0] == PlanType::FrozenStr);
    let path = match (name.as_str(), text) {
        ("FrozenSet", true) => "incan_native_runtime::frozen_set_text_items",
        ("FrozenDict", true) => "incan_native_runtime::frozen_dict_text_keys",
        (_, true) => "incan_native_runtime::frozen_list_text_items",
        _ if elements[0] == PlanType::StaticBytes => "incan_native_runtime::frozen_list_byte_items",
        ("FrozenSet", false) => "incan_native_runtime::frozen_set_items",
        ("FrozenDict", false) => "incan_native_runtime::frozen_dict_keys",
        _ => "incan_native_runtime::frozen_list_items",
    };
    let output = temporary(builder, snapshot.clone(), span);
    instantiated_external_call(
        builder,
        path,
        elements.clone(),
        vec![ty.clone()],
        snapshot,
        vec![value],
        output.clone(),
        span,
    )?;
    builder.owned.push(output.local);
    Ok(output)
}

/// Snapshot frozen source items or consume an owned generator once before constructing the canonical set.
pub fn set_constructor_source(
    builder: &mut Builder,
    source: &SourceOperand,
    span: &SourceSpan,
) -> Result<Operand, String> {
    let value = value_operand(builder, source, span)?;
    match value_type(builder, &value)? {
        PlanType::FrozenCollection(..) => {
            let snapshot = frozen_list_snapshot(builder, value, span, false)?;
            Ok(read_place(snapshot, true, span))
        }
        PlanType::Generator(leaf, depth) => {
            let OperandKind::Move(receiver) = value.kind else {
                return Err("unsupported Body IR set constructor without an owned generator".into());
            };
            let snapshot = temporary(builder, PlanType::List(leaf, depth + 1), span);
            generator_collect(builder, receiver, snapshot.clone(), span)?;
            builder.owned.push(snapshot.local);
            Ok(read_place(snapshot, true, span))
        }
        _ => Ok(value),
    }
}

/// Start a fresh cursor for a constant's immutable snapshot without inventing a mutable source place.
fn initialize_frozen_iterator(builder: &mut Builder, iterator: &Place, span: &SourceSpan) {
    let cursor = temporary(builder, PlanType::Int, span);
    builder.emit(plan_assign(
        cursor.clone(),
        RvalueKind::Use(literal(Constant::Int(0), span)),
        span,
    ));
    builder.iterators.push((iterator.local, cursor.local));
}

/// Retain cursor storage for Body IR's borrowed builtin iterable materialization, shared or exclusive.
fn initialize_list_iterator(builder: &mut Builder, iterator: &Place, source: &SourceOperand, span: &SourceSpan) {
    if let SourceOperand::Place(read) = source
        && (read.fact == OwnershipFact::Borrow || read.fact == OwnershipFact::MutBorrow)
    {
        initialize_frozen_iterator(builder, iterator, span);
    }
}

/// Poll a retained builtin collection cursor, branching to the loop exit on exhaustion.
pub fn list_next(
    builder: &mut Builder,
    destination: &SourcePlace,
    iterator: &SourceOperand,
    protocol: &IterProtocol,
    span: &SourceSpan,
) -> Result<(), String> {
    if !is_builtin_iteration(protocol) || builder.loops.is_empty() {
        return Err("unsupported Body IR non-builtin list IterNext".into());
    }
    if let SourceOperand::Place(read) = iterator {
        let candidate = scalar_place(builder, &read.place, span)?;
        if is_owned_list_iterator(&place_type(&builder.locals, &candidate)) {
            if read.fact != OwnershipFact::MutBorrow {
                return Err("unsupported Body IR consuming iterator poll ownership".into());
            }
            let target = list_root_place(builder, destination, span)?;
            return owned_list_next(builder, &candidate, &target, span);
        }
    }
    let receiver = operand_list_place(builder, iterator, span)?;
    let ty = place_type(&builder.locals, &receiver);
    if zip_shape(&ty).is_some() {
        let target = list_root_place(builder, destination, span)?;
        zip_next(builder, receiver, &target, span)?;
        // Legacy changes a zipped operand's list items in place; a pair here holds only snapshots of them.
        if pair_holds_list(&place_type(&builder.locals, &target)) {
            borrow_iteration_item(builder, target.local);
        }
        return Ok(());
    }
    if generator_shape(&ty).is_some() {
        let target = list_root_place(builder, destination, span)?;
        return generator_next(builder, receiver, &target, span);
    }
    let element = match list_shape(&ty) {
        Some((leaf, depth)) => list_element(&leaf, depth),
        None => {
            let mut arguments = collection_arguments(&ty);
            if arguments.is_empty() {
                return Err("unsupported Body IR noncollection IterNext".into());
            }
            arguments.swap_remove(0)
        }
    };
    let cursor = builder
        .iterators
        .iter()
        .rev()
        .find(|(iterator, _)| *iterator == receiver.local)
        .map(|(_, cursor)| *cursor);
    let Some(cursor) = cursor else {
        return Err("unsupported Body IR list iterator without retained materialization".into());
    };
    let position = read_place(plan_place(cursor, span), false, span);
    poll_cursor(builder, &receiver, &ty, &position, span)?;
    let target = list_root_place(builder, destination, span)?;
    bind_item(builder, receiver, &ty, target, element, position.clone(), span)?;
    // The cursor cannot overflow: a successful poll is bounded by the native collection length.
    let pair = temporary(builder, PlanType::CheckedInt, span);
    let one = literal(Constant::Int(1), span);
    builder.emit(plan_assign(
        pair.clone(),
        RvalueKind::Binary(BinaryOp::Add, position, one),
        span,
    ));
    let result = Place {
        local: pair.local,
        projection: Projection::Value,
        span: span.clone(),
    };
    builder.emit(plan_assign(
        plan_place(cursor, span),
        RvalueKind::Use(read_place(result, false, span)),
        span,
    ));
    Ok(())
}

/// Compare the cursor with the collection's length, leaving the loop on exhaustion and selecting the item block.
fn poll_cursor(
    builder: &mut Builder,
    receiver: &Place,
    ty: &PlanType,
    position: &Operand,
    span: &SourceSpan,
) -> Result<(), String> {
    let reference = list_borrow(builder, receiver.clone(), false, span)?;
    let length = temporary(builder, PlanType::Int, span);
    iterable_call(
        builder,
        ty,
        "length",
        Vec::new(),
        PlanType::Int,
        vec![reference],
        length.clone(),
        span,
    )?;
    let condition = temporary(builder, PlanType::Bool, span);
    builder.emit(plan_assign(
        condition.clone(),
        RvalueKind::Binary(BinaryOp::Less, position.clone(), read_place(length, false, span)),
        span,
    ));
    let next_block = builder.new_block(span);
    let exhausted = builder.new_block(span);
    // The item block resumes from the owners live before the exhausted branch releases the loop's owners.
    let owners = builder.owned.clone();
    builder.finish(
        TerminatorKind::SwitchBool(read_place(condition, false, span), exhausted, next_block),
        span,
    );
    builder.select(exhausted);
    drop_loop_owners(builder, span)?;
    exhaust_loop(builder, span);
    builder.select(next_block);
    builder.owned = owners;
    Ok(())
}

/// Bind the item at the cursor: in place for an exclusive list, as a shared reference, or as an owned copy.
#[allow(clippy::too_many_arguments)] // The polled receiver, its type, the item, its element and the cursor position.
fn bind_item(
    builder: &mut Builder,
    receiver: Place,
    ty: &PlanType,
    target: Place,
    element: PlanType,
    position: Operand,
    span: &SourceSpan,
) -> Result<(), String> {
    if is_list(ty) && is_mutable_list_reference(ty) {
        return in_place_item(builder, receiver, &target, element, position, span);
    }
    if is_shared_item(&place_type(&builder.locals, &target), &element) {
        return shared_item(builder, receiver, target, position, span);
    }
    let item_reference = list_borrow(builder, receiver, false, span)?;
    drop_owned(builder, target.local, span);
    if element == PlanType::String || is_list(&element) || is_nominal_type(&element) {
        borrow_iteration_item(builder, target.local);
    }
    // A bytes value's items are unsigned bytes, which legacy's loop widens to the checked `int` item.
    let widened = element == PlanType::U8 && place_type(&builder.locals, &target) == PlanType::Int;
    let item = if widened {
        temporary(builder, PlanType::U8, span)
    } else {
        target.clone()
    };
    let owned = owned_carrier(&element)
        || element == PlanType::String
        || is_owned_collection(&element)
        || tuple_has_owners(&element);
    iterable_call(
        builder,
        ty,
        "nth",
        vec![PlanType::Int],
        element,
        vec![item_reference, position],
        item.clone(),
        span,
    )?;
    if owned {
        builder.owned.push(item.local);
    }
    if widened {
        let byte = read_place(item, false, span);
        builder.emit(plan_assign(
            target,
            RvalueKind::NumericCast(byte, PlanType::U8, PlanType::Int),
            span,
        ));
    }
    Ok(())
}

/// Report whether a loop item is a shared reference to its model, source enum or text element.
fn is_shared_item(item: &PlanType, element: &PlanType) -> bool {
    match (item, element) {
        (PlanType::ModelRef(owner, _), PlanType::Model(source, _)) => owner == source,
        (PlanType::EnumRef(owner, _), PlanType::Enum(source, _)) => owner == source,
        (PlanType::StringRef, PlanType::String) => true,
        _ => false,
    }
}

/// Bind a loop item that its body only reads as a shared reference to its element (#1605).
///
/// The element is lent through the runtime's checked `list_get` at the cursor position, so no element is copied and
/// the item owns nothing.
fn shared_item(
    builder: &mut Builder,
    iterator: Place,
    item: Place,
    position: Operand,
    span: &SourceSpan,
) -> Result<(), String> {
    let element = shared_list_element(builder, iterator, position, span)?;
    let reference = Place {
        local: element.local,
        projection: Projection::Whole,
        span: span.clone(),
    };
    builder.emit(plan_assign(
        item,
        RvalueKind::Use(read_place(reference, false, span)),
        span,
    ));
    Ok(())
}

/// Bind a loop item that the body changes in place as an exclusive reference to its element, as legacy's `iter_mut`.
///
/// The element is reborrowed through the runtime's checked `borrow_list_mut` at the cursor position, so each change
/// lands in the list. Tuples keep their complete layout behind that reference; scalar elements remain refused.
fn in_place_item(
    builder: &mut Builder,
    iterator: Place,
    item: &Place,
    element: PlanType,
    position: Operand,
    span: &SourceSpan,
) -> Result<(), String> {
    let (leaf, depth) = require_list_shape(
        &place_type(&builder.locals, &iterator),
        "unsupported Body IR nonlist cursor",
    )?;
    let Some(reference_type) = exclusive_reference(&element) else {
        return Err("unsupported Body IR in-place iteration over tuple or scalar elements".into());
    };
    builder.locals[item.local as usize].ty = reference_type.clone();
    if !builder.in_place_items.contains(&item.local) {
        builder.in_place_items.push(item.local);
    }
    let exclusive_list = list_borrow(builder, iterator, true, span)?;
    list_call(
        builder,
        "borrow_list_mut",
        element,
        vec![PlanType::ListMutRef(leaf, depth), PlanType::Int],
        reference_type,
        vec![exclusive_list, position],
        item.clone(),
        span,
    );
    Ok(())
}

/// Materialize the explicit builtin iterator's live list view and initialize its cursor once.
///
/// The view is shared, unless the loop changes its items in place: then Body IR borrows the list exclusively and each
/// item becomes an exclusive reference to its element (see `in_place_item`).
pub fn iterator_assignment(
    builder: &mut Builder,
    destination: Place,
    value: &Rvalue,
    span: &SourceSpan,
) -> Result<(), String> {
    let Rvalue::Use(value) = value else {
        return Err("unsupported Body IR list iterator initialization".into());
    };
    let read = match value {
        SourceOperand::Constant(constant @ SourceConstant::FrozenAggregate(..)) => {
            let frozen = frozen_collection_literal(builder, constant, span)?;
            let snapshot = frozen_list_snapshot(builder, frozen, span, false)?;
            let reference = list_borrow(builder, snapshot, false, span)?;
            builder.emit(plan_assign(destination.clone(), RvalueKind::Use(reference), span));
            initialize_frozen_iterator(builder, &destination, span);
            return Ok(());
        }
        SourceOperand::Constant(_) => return Err("unsupported Body IR list iterator constant".into()),
        SourceOperand::Place(read) => read,
    };
    let exclusive = match read.fact {
        OwnershipFact::Borrow => false,
        OwnershipFact::MutBorrow => true,
        _ => return Err("unsupported Body IR list iterator initialization ownership".into()),
    };
    let mut root = iteration_root(builder, &read.place, exclusive, span)?;
    if let PlanType::FrozenCollection(..) = place_type(&builder.locals, &root) {
        if exclusive {
            return Err("unsupported Body IR mutable frozen iteration".into());
        }
        root = frozen_list_snapshot(builder, read_place(root, false, span), span, false)?;
    }
    let root_type = place_type(&builder.locals, &root);
    let dictionary = collection_arguments(&root_type).len() == 2;
    if exclusive && !is_list(&root_type) && !dictionary {
        return Err("unsupported Body IR in-place iteration over a set".into());
    }
    let reference = list_borrow(builder, root, exclusive, span)?;
    if exclusive {
        builder.locals[destination.local as usize].ty = value_type(builder, &reference)?;
    }
    builder.emit(plan_assign(destination.clone(), RvalueKind::Use(reference), span));
    initialize_list_iterator(builder, &destination, value, span);
    Ok(())
}

/// Select the list a loop iterates: a loop changing the items of a list held by a list element or dictionary value
/// iterates the stored list itself, reached through the container's checked reborrows.
fn iteration_root(
    builder: &mut Builder,
    value: &SourcePlace,
    exclusive: bool,
    span: &SourceSpan,
) -> Result<Place, String> {
    if is_element_place(builder, value)? {
        if exclusive {
            return mutable_element_place(builder, value, span);
        }
        if let Some(place) = shared_element_place(builder, value, span)? {
            return Ok(place);
        }
        return materialized_receiver(builder, value, span);
    }
    list_root_place(builder, value, span)
}

/// Borrow whole storage or an exact field, payload or tuple element path.
///
/// A constant index selects a tuple element exactly like a structural field, so it stays on this path. Indexing a list
/// retains its separate element-materialization route.
pub fn list_root_place(builder: &mut Builder, value: &SourcePlace, span: &SourceSpan) -> Result<Place, String> {
    if value.projection.is_empty() {
        return list_base_place(builder, value, span);
    }
    if let Some(place) = computed_static_root(builder, value, span)? {
        return Ok(place);
    }
    let Some(local) = value.local_id() else {
        return Err("unsupported Body IR global list root projection".into());
    };
    let mut ty = builder.locals[(local.index() as i64 + 1 + builder.local_offset) as usize]
        .ty
        .clone();
    // A receiver reference projects its model's fields.
    ty = match ty {
        PlanType::ModelRef(owner, name) | PlanType::ModelMutRef(owner, name) => PlanType::Model(owner, name),
        PlanType::EnumMutRef(owner, name) => PlanType::Enum(owner, name),
        other => other,
    };
    for projection in &value.projection {
        match projection {
            PlaceElem::Field { .. } | PlaceElem::OptionPayload { .. } | PlaceElem::UnionMember { .. } => {}
            PlaceElem::Index(_) => {
                if !is_tuple_type(&ty) {
                    return Err(list_projection_refusal(&builder.name, &ty, projection));
                }
            }
            _ => return Err(list_projection_refusal(&builder.name, &ty, projection)),
        }
        ty = field_step_type(builder.tables(), &ty, projection)?;
    }
    scalar_place(builder, value, span)
}

/// Name the retained carrier and projection that cannot provide structural collection storage.
fn list_projection_refusal(name: &str, ty: &PlanType, projection: &PlaceElem) -> String {
    format!("unsupported Body IR list root projection in {name}: {ty:?}; {projection:?}")
}

/// Test text membership in a frozen list or set in place: the frozen carrier compares each static element's text with
/// the borrowed needle, and no element is copied or formatted (#2944). Any other membership keeps the ordinary route.
pub fn frozen_text_membership(
    builder: &mut Builder,
    negated: bool,
    arguments: &[ArgumentElement],
    destination: &Option<SourcePlace>,
    span: &SourceSpan,
) -> Result<bool, String> {
    let ty = argument_type(builder, arguments, 0, span)?;
    let PlanType::FrozenCollection(name, elements) = &ty else {
        return Ok(false);
    };
    if elements.len() != 1 || !matches!(elements[0], PlanType::StaticText | PlanType::FrozenStr) {
        return Ok(false);
    }
    let path = match name.as_str() {
        "FrozenList" => "incan_std_primitives::frozen::frozen_list_contains_text",
        "FrozenSet" => "incan_std_primitives::frozen::frozen_set_contains_text",
        _ => return Ok(false),
    };
    let Some(value) = arguments[1].as_one() else {
        return Err("unsupported Body IR frozen membership needle spread".into());
    };
    let needle = borrow_text_operand(builder, value, span)?;
    let receiver = argument_value(builder, arguments, 0, span)?;
    let output = call_destination(builder, destination, PlanType::Bool, span)?;
    instantiated_external_call(
        builder,
        path,
        elements.clone(),
        vec![ty.clone(), PlanType::StringRef],
        PlanType::Bool,
        vec![receiver, needle],
        output.clone(),
        span,
    )?;
    if negated {
        negate(builder, output, span);
    }
    Ok(true)
}
