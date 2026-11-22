//! Read, assign, and call methods on sets and dictionaries.

use incan_lang::lang::surface::dict_methods::DictMethodId;
use incan_lang::lang::surface::set_methods::SetMethodId;
use incan_mir_plan_native::{
    Carrier, Constant, Operand, Place, PlanType, Projection, RvalueKind, SourceSpan, UnaryOp, assign as plan_assign,
};
use incan_semantics_core::body_ir::{
    AggregateKind, ArgumentElement, DictEntry, OwnershipFact, Place as SourcePlace, PlaceElem, PlaceOperand, Rvalue,
};

use crate::builtins::list_call;
use crate::carrier_operands::{element_operand, element_plan_operand};
use crate::collections::{
    begin_collection, borrow_collection, collection_arguments, collection_call, collection_owned, collection_reference,
    dictionary_method_signature, extend_collection, finish_collection, insert_collection, is_collection,
    is_owned_collection, require_mutable_receiver, transfer_collection,
};
use crate::frame::{Builder, drop_owned, literal, place_type, read_place, temporary};
use crate::list_iteration::{frozen_dictionary_contains, list_root_place, set_constructor_source};
use crate::list_methods::{
    argument_place, argument_value, call_destination, lower_list_helper, shared_receiver_place,
    static_collection_receiver,
};
use crate::list_values::{list_base_place, projected_element_read, value_operand};
use crate::lists::{is_owned_list, list_element, require_list_shape};
use crate::options::carrier_layout;
use crate::options::replace_owned_destination;
use crate::sequence_builtins::argument_type;
use crate::statements::is_nominal_type;
use crate::static_guards::{GuardedCollection, GuardedOperation, guarded_collection, guarded_collection_call};
use crate::static_storage::persist_static_value;

/// Materialize a checked collection read while preserving caller-owned bindings.
///
/// Moving a value parameter that uses a shared collection ABI copies its stored value, just as a list parameter does;
/// the caller still owns the borrowed storage.
pub fn collection_value(builder: &mut Builder, read: &PlaceOperand, span: &SourceSpan) -> Result<Operand, String> {
    if !read.place.projection.is_empty() {
        return dictionary_index_read(builder, &read.place, span);
    }
    let place = list_root_place(builder, &read.place, span)?;
    collection_place_value(builder, place, read.fact, span)
}

/// Materialize whole or structurally projected dictionary/set storage using the same checked read policy; projected
/// moves gain no authority over their containing owner.
pub fn collection_place_value(
    builder: &mut Builder,
    place: Place,
    fact: OwnershipFact,
    span: &SourceSpan,
) -> Result<Operand, String> {
    let ty = place_type(&builder.locals, &place);
    let owned = collection_owned(&ty);
    if fact == OwnershipFact::Move && ty == owned {
        return Ok(read_place(place, true, span));
    }
    let shared_move = fact == OwnershipFact::Move && ty != owned;
    if fact != OwnershipFact::Clone && fact != OwnershipFact::Borrow && !shared_move {
        let reason = format!("unsupported Body IR collection value ownership {fact:?} for {ty:?}");
        return Err(format!("{reason} in {} at {}:{}", builder.name, span.line, span.column));
    }
    cloned_collection(builder, place, span)
}

/// Copy a set or dictionary into a new owned temporary that drops when its scope ends.
///
/// A whole local is borrowed, or reborrowed when it is a parameter view; a projected one, such as an enum payload, is
/// owned where it lives and is borrowed in place.
pub fn cloned_collection(builder: &mut Builder, place: Place, span: &SourceSpan) -> Result<Operand, String> {
    let ty = place_type(&builder.locals, &place);
    let owned = collection_owned(&ty);
    let reference = if place.projection == Projection::Whole {
        borrow_collection(builder, &place, false, span)?
    } else {
        if ty != owned {
            return Err("unsupported Body IR projected collection view".into());
        }
        let view = temporary(builder, collection_reference(&ty, false), span);
        builder.emit(plan_assign(view.clone(), RvalueKind::Borrow(place), span));
        read_place(view, false, span)
    };
    let output = temporary(builder, owned.clone(), span);
    collection_call(
        builder,
        &ty,
        "clone",
        vec![collection_reference(&ty, false)],
        owned,
        vec![reference],
        output.clone(),
        span,
    )?;
    Ok(read_place(output, true, span))
}

/// Read checked literal operands before the collection module constructs their exact layout.
pub fn collection_assignment(
    builder: &mut Builder,
    destination: Place,
    value: &Rvalue,
    span: &SourceSpan,
) -> Result<(), String> {
    if destination.projection != Projection::Whole {
        let replacement = temporary(builder, place_type(&builder.locals, &destination), span);
        collection_assignment(builder, replacement.clone(), value, span)?;
        let operation = RvalueKind::Use(read_place(replacement, true, span));
        replace_owned_destination(builder, destination, operation, span);
        return Ok(());
    }
    match value {
        Rvalue::Use(value) => {
            let argument = value_operand(builder, value, span)?;
            transfer_collection(builder, &destination, argument, span)
        }
        Rvalue::Aggregate(kind, elements) => {
            if *kind != AggregateKind::Set {
                return Err("unsupported Body IR non-set collection Aggregate".into());
            }
            let output = begin_collection(builder, &destination, span)?;
            let types = collection_arguments(&builder.locals[output.local as usize].ty);
            for element in elements {
                match element.as_one() {
                    Some(value) => {
                        let argument = element_operand(builder, value, &types[0], span)?;
                        insert_collection(builder, &output, vec![argument], span)?;
                    }
                    None => {
                        let ArgumentElement::Spread(spread) = element else {
                            return Err("unsupported Body IR set spread".into());
                        };
                        let source = set_constructor_source(builder, &spread.source, span)?;
                        extend_collection(builder, &output, &source, span)?;
                    }
                }
            }
            finish_collection(builder, &destination, output, span);
            Ok(())
        }
        Rvalue::Dict(entries) => {
            let output = begin_collection(builder, &destination, span)?;
            let types = collection_arguments(&builder.locals[output.local as usize].ty);
            for entry in entries {
                match entry {
                    DictEntry::Pair(key, value) => {
                        let key_operand = element_operand(builder, key, &types[0], span)?;
                        let item_operand = element_operand(builder, value, &types[1], span)?;
                        insert_collection(builder, &output, vec![key_operand, item_operand], span)?;
                    }
                    DictEntry::Spread(spread) => {
                        let source = value_operand(builder, &spread.source, span)?;
                        extend_collection(builder, &output, &source, span)?;
                    }
                }
            }
            finish_collection(builder, &destination, output, span);
            Ok(())
        }
        _ => Err("unsupported Body IR collection assignment rvalue".into()),
    }
}

/// Lower canonical len without transferring container ownership.
pub fn collection_length(
    builder: &mut Builder,
    receiver: Place,
    destination: &Option<SourcePlace>,
    span: &SourceSpan,
) -> Result<(), String> {
    let ty = place_type(&builder.locals, &receiver);
    let reference = borrow_collection(builder, &receiver, false, span)?;
    let output = call_destination(builder, destination, PlanType::Int, span)?;
    collection_call(
        builder,
        &ty,
        "length",
        vec![collection_reference(&ty, false)],
        PlanType::Int,
        vec![reference],
        output,
        span,
    )
}

/// Preserve haystack-first membership, copying an indexed collection receiver before its shared read.
pub fn collection_contains(
    builder: &mut Builder,
    negated: bool,
    arguments: &[ArgumentElement],
    destination: &Option<SourcePlace>,
    span: &SourceSpan,
) -> Result<(), String> {
    if arguments.len() != 2 {
        return Err("unsupported Body IR collection membership arity".into());
    }
    if let PlanType::FrozenCollection(name, _) = argument_type(builder, arguments, 0, span)? {
        if name == "FrozenDict" {
            return frozen_dictionary_contains(builder, arguments, destination, negated, span);
        }
        return lower_list_helper(builder, negated, arguments, destination, span);
    }
    if let Some(guarded) = guarded_collection(builder, arguments)? {
        return guarded_contains(builder, &guarded, negated, arguments, destination, span);
    }
    let receiver = shared_receiver_place(builder, arguments, 0, span)?;
    let ty = place_type(&builder.locals, &receiver);
    let types = collection_arguments(&ty);
    if types.is_empty() {
        return Err("unsupported Body IR collection membership on noncollection".into());
    }
    let argument = argument_value(builder, arguments, 1, span)?;
    let reference = borrow_collection(builder, &receiver, false, span)?;
    let output = call_destination(builder, destination, PlanType::Bool, span)?;
    collection_call(
        builder,
        &ty,
        "contains",
        vec![collection_reference(&ty, false), types[0].clone()],
        PlanType::Bool,
        vec![reference, argument],
        output.clone(),
        span,
    )?;
    if negated {
        negate(builder, output, span);
    }
    Ok(())
}

/// Answer membership in a static's set or dictionary under its guard, after evaluating the needle.
fn guarded_contains(
    builder: &mut Builder,
    guarded: &GuardedCollection,
    negated: bool,
    arguments: &[ArgumentElement],
    destination: &Option<SourcePlace>,
    span: &SourceSpan,
) -> Result<(), String> {
    let types = collection_arguments(&guarded.collection);
    if types.is_empty() {
        return Err("unsupported Body IR collection membership on noncollection".into());
    }
    let argument = argument_value(builder, arguments, 1, span)?;
    let output = call_destination(builder, destination, PlanType::Bool, span)?;
    let request = GuardedOperation {
        operation: "contains",
        parameters: vec![types[0].clone()],
        result: PlanType::Bool,
        operands: vec![argument],
    };
    guarded_collection_call(builder, guarded, request, output.clone(), span)?;
    if negated {
        negate(builder, output, span);
    }
    Ok(())
}

/// Replace a boolean answer with its negation.
fn negate(builder: &mut Builder, output: Place, span: &SourceSpan) {
    let value = read_place(output.clone(), false, span);
    builder.emit(plan_assign(output, RvalueKind::Unary(UnaryOp::Not, value), span));
}

/// Clone a checked dictionary element through legacy indexing and error behavior, then read what follows from it.
pub fn dictionary_index_read(
    builder: &mut Builder,
    source: &SourcePlace,
    span: &SourceSpan,
) -> Result<Operand, String> {
    if source.projection.is_empty() {
        return Err("unsupported Body IR dictionary read without key".into());
    }
    let receiver = list_base_place(builder, source, span)?;
    dictionary_value_read(builder, source, receiver, span)
}

/// Look up the place's first key in an already selected dictionary, such as a static's snapshot, then read on.
pub fn dictionary_value_read(
    builder: &mut Builder,
    source: &SourcePlace,
    receiver: Place,
    span: &SourceSpan,
) -> Result<Operand, String> {
    let output = read_dictionary_element(builder, receiver, source, 0, span)?;
    projected_element_read(builder, source, output, 1, span)
}

/// Clone the value the key at projection `slot` selects, through legacy indexing and its missing-key error.
pub fn read_dictionary_element(
    builder: &mut Builder,
    receiver: Place,
    source: &SourcePlace,
    slot: usize,
    span: &SourceSpan,
) -> Result<Place, String> {
    let ty = place_type(&builder.locals, &receiver);
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
    let PlaceElem::Index(index) = &source.projection[slot] else {
        return Err("unsupported Body IR dictionary slice or field projection".into());
    };
    let key = value_operand(builder, index, span)?;
    let reference = borrow_collection(builder, &receiver, false, span)?;
    let output = temporary(builder, types[1].clone(), span);
    collection_call(
        builder,
        &ty,
        "get",
        vec![collection_reference(&ty, false), types[0].clone()],
        types[1].clone(),
        vec![reference, key],
        output.clone(),
        span,
    )?;
    // A cloned value that owns storage is a new owner, like a cloned list element; the final read moves it out.
    if is_nominal_type(&types[1]) || is_owned_list(&types[1]) || is_owned_collection(&types[1]) {
        builder.owned.push(output.local);
    }
    Ok(output)
}

/// Replace dictionary entries after evaluating the source value and retained key.
pub fn dictionary_index_assignment(
    builder: &mut Builder,
    destination: &SourcePlace,
    value: &Rvalue,
    span: &SourceSpan,
) -> Result<(), String> {
    let receiver = list_base_place(builder, destination, span)?;
    dictionary_entry_assignment(builder, destination, receiver, 0, value, span)
}

/// Insert an evaluated entry into its retained dictionary, including a dictionary reached through a structural field.
pub fn dictionary_entry_assignment(
    builder: &mut Builder,
    destination: &SourcePlace,
    receiver: Place,
    start: usize,
    value: &Rvalue,
    span: &SourceSpan,
) -> Result<(), String> {
    if destination.projection.len() != start + 1 {
        return Err("unsupported Body IR nested dictionary mutation".into());
    }
    let types = collection_arguments(&place_type(&builder.locals, &receiver));
    if types.len() != 2 {
        return Err("unsupported Body IR set indexing assignment".into());
    }
    let Rvalue::Use(value) = value else {
        return Err("unsupported Body IR dictionary indexed rvalue".into());
    };
    let argument = element_operand(builder, value, &types[1], span)?;
    let PlaceElem::Index(index) = &destination.projection[start] else {
        return Err("unsupported Body IR dictionary mutation projection".into());
    };
    let key = element_operand(builder, index, &types[0], span)?;
    insert_collection(builder, &receiver, vec![key, argument], span)
}

/// Lower one checker-selected set operation without interpreting canonical text.
pub fn set_method(
    builder: &mut Builder,
    method: SetMethodId,
    arguments: &[ArgumentElement],
    destination: &Option<SourcePlace>,
    span: &SourceSpan,
) -> Result<(), String> {
    match method {
        SetMethodId::Contains => collection_contains(builder, false, arguments, destination, span),
        SetMethodId::Add => collection_insert_method(builder, "Set.add", arguments, destination, span),
    }
}

/// Lower one checker-selected dictionary operation without interpreting canonical text.
pub fn dictionary_method(
    builder: &mut Builder,
    method: DictMethodId,
    arguments: &[ArgumentElement],
    destination: &Option<SourcePlace>,
    span: &SourceSpan,
) -> Result<(), String> {
    if method == DictMethodId::ContainsKey {
        return collection_contains(builder, false, arguments, destination, span);
    }
    if method == DictMethodId::Insert {
        return collection_insert_method(builder, "Dict.insert", arguments, destination, span);
    }
    // A static's dictionary is read under its guard, so the call copies neither the static nor the dictionary.
    if let Some(guarded) = guarded_collection(builder, arguments)? {
        return guarded_dictionary_method(builder, &guarded, method, arguments, destination, span);
    }
    let receiver = shared_receiver_place(builder, arguments, 0, span)?;
    let ty = place_type(&builder.locals, &receiver);
    if method == DictMethodId::Get && arguments.len() == 2 {
        return optional_dictionary_get(builder, receiver, arguments, destination, span);
    }
    let (operation, parameters, result) = dictionary_method_signature(&ty, method, arguments.len() as i64)?;
    let mut operands = Vec::with_capacity(parameters.len());
    for slot in 0..parameters.len() {
        operands.push(argument_value(builder, arguments, slot + 1, span)?);
    }
    let reference = borrow_collection(builder, &receiver, false, span)?;
    let output = call_destination(builder, destination, result.clone(), span)?;
    let mut all_parameters = vec![collection_reference(&ty, false)];
    all_parameters.extend(parameters);
    let mut all_operands = vec![reference];
    all_operands.extend(operands);
    collection_call(
        builder,
        &ty,
        &operation,
        all_parameters,
        result,
        all_operands,
        output,
        span,
    )
}

/// Lower one read-only dictionary operation on a static's dictionary under its guard, after evaluating the operands.
fn guarded_dictionary_method(
    builder: &mut Builder,
    guarded: &GuardedCollection,
    method: DictMethodId,
    arguments: &[ArgumentElement],
    destination: &Option<SourcePlace>,
    span: &SourceSpan,
) -> Result<(), String> {
    let types = collection_arguments(&guarded.collection);
    if method == DictMethodId::Get && arguments.len() == 2 {
        let Some(place) = destination else {
            return Err("unsupported Body IR discarded optional dictionary get".into());
        };
        let output = list_root_place(builder, place, span)?;
        let result = optional_get_result(builder, &types, &output)?;
        let key = argument_value(builder, arguments, 1, span)?;
        let found = temporary(builder, result.clone(), span);
        let request = GuardedOperation {
            operation: "optional",
            parameters: vec![types[0].clone()],
            result,
            operands: vec![key],
        };
        guarded_collection_call(builder, guarded, request, found.clone(), span)?;
        replace_optional(builder, output, found, span);
        return Ok(());
    }
    let (operation, parameters, result) =
        dictionary_method_signature(&guarded.collection, method, arguments.len() as i64)?;
    let mut operands = Vec::with_capacity(parameters.len());
    for slot in 0..parameters.len() {
        operands.push(argument_value(builder, arguments, slot + 1, span)?);
    }
    let output = call_destination(builder, destination, result.clone(), span)?;
    let request = GuardedOperation {
        operation: &operation,
        parameters,
        result,
        operands,
    };
    guarded_collection_call(builder, guarded, request, output, span)
}

/// Insert the evaluated arguments into a set or dictionary receiver.
///
/// A static receiver's snapshot is read only after the arguments and committed after the insertion, as a static list
/// method's is. A call retained as an expression initializes its checked Unit destination after the mutation; an
/// argument that propagated an error has already returned before this call is lowered.
fn collection_insert_method(
    builder: &mut Builder,
    member: &str,
    arguments: &[ArgumentElement],
    destination: &Option<SourcePlace>,
    span: &SourceSpan,
) -> Result<(), String> {
    let types = collection_arguments(&argument_type(builder, arguments, 0, span)?);
    if arguments.len() != types.len() + 1 {
        return Err(format!("unsupported Body IR collection method arity {member}"));
    }
    // Use the shared argument check before creating the short mutable runtime borrow.
    require_mutable_receiver(arguments)?;
    let mut operands = Vec::with_capacity(types.len());
    for (slot, ty) in types.iter().enumerate() {
        let argument = argument_value(builder, arguments, slot + 1, span)?;
        operands.push(element_plan_operand(builder, argument, ty, span)?);
    }
    let output = call_destination(builder, destination, PlanType::Unit, span)?;
    match static_collection_receiver(builder, arguments, true, span)? {
        Some((global, receiver)) => {
            insert_collection(builder, &receiver, operands, span)?;
            persist_static_value(builder, &global, receiver, span)?;
        }
        None => {
            let receiver = argument_place(builder, arguments, 0, span)?;
            insert_collection(builder, &receiver, operands, span)?;
        }
    }
    builder.emit(plan_assign(
        output,
        RvalueKind::Use(literal(Constant::Unit, span)),
        span,
    ));
    Ok(())
}

/// Share cursor calls while each collection retains its actual runtime representation.
#[allow(clippy::too_many_arguments)] // The Incan helper's eight parts of one cursor call.
pub fn iterable_call(
    builder: &mut Builder,
    ty: &PlanType,
    operation: &str,
    tail: Vec<PlanType>,
    result: PlanType,
    operands: Vec<Operand>,
    destination: Place,
    span: &SourceSpan,
) -> Result<(), String> {
    if is_collection(ty) {
        let mut parameters = vec![collection_reference(ty, false)];
        parameters.extend(tail);
        return collection_call(builder, ty, operation, parameters, result, operands, destination, span);
    }
    let (leaf, depth) = require_list_shape(ty, "unsupported Body IR nonlist cursor")?;
    let name = if operation == "nth" { "get_list" } else { "length_list" };
    let mut parameters = vec![PlanType::ListRef(leaf.clone(), depth)];
    parameters.extend(tail);
    list_call(
        builder,
        name,
        list_element(&leaf, depth),
        parameters,
        result,
        operands,
        destination,
        span,
    );
    Ok(())
}

/// Copy a present value into the checked Option destination, replacing that destination's earlier value.
fn optional_dictionary_get(
    builder: &mut Builder,
    receiver: Place,
    arguments: &[ArgumentElement],
    destination: &Option<SourcePlace>,
    span: &SourceSpan,
) -> Result<(), String> {
    let Some(place) = destination else {
        return Err("unsupported Body IR discarded optional dictionary get".into());
    };
    let output = list_root_place(builder, place, span)?;
    let ty = place_type(&builder.locals, &receiver);
    let types = collection_arguments(&ty);
    let result = optional_get_result(builder, &types, &output)?;
    let key = argument_value(builder, arguments, 1, span)?;
    let reference = borrow_collection(builder, &receiver, false, span)?;
    let found = temporary(builder, result.clone(), span);
    collection_call(
        builder,
        &ty,
        "optional",
        vec![collection_reference(&ty, false), types[0].clone()],
        result,
        vec![reference, key],
        found.clone(),
        span,
    )?;
    replace_optional(builder, output, found, span);
    Ok(())
}

/// The checked Option destination's carrier, required to hold exactly the dictionary's value type.
fn optional_get_result(builder: &Builder, types: &[PlanType], output: &Place) -> Result<PlanType, String> {
    let result = builder.locals[output.local as usize].ty.clone();
    let (_, declaration) = carrier_layout(builder, &result)?;
    let carrier = "unsupported Body IR optional dictionary get carrier";
    let Some(value_type) = types.get(1) else {
        return Err(carrier.into());
    };
    if declaration.carrier != Carrier::Option || declaration.variants[1].fields != [value_type.clone()] {
        return Err(carrier.into());
    }
    Ok(result)
}

/// Move the found Option into its destination, dropping the destination's earlier value first.
fn replace_optional(builder: &mut Builder, output: Place, found: Place, span: &SourceSpan) {
    drop_owned(builder, output.local, span);
    let local = output.local;
    builder.emit(plan_assign(
        output,
        RvalueKind::Use(read_place(found, true, span)),
        span,
    ));
    builder.owned.push(local);
}
