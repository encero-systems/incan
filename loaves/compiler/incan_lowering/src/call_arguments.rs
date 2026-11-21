//! Call argument storage: the parameter representation each frame slot holds and each call materializes.

use incan_mir_plan_native::{Constant, Operand, Place, PlanType, RvalueKind, SourceSpan, assign as plan_assign};
use incan_semantics_core::IncanInputAccess;
use incan_semantics_core::body_ir::{
    ArgumentBinding, ArgumentElement, Body, BoundArgument, CallableParam, Constant as SourceConstant,
    Operand as SourceOperand, OwnershipFact,
};

use crate::builtin_calls::coerce_numeric_operand;
use crate::callable_calls::callable_argument;
use crate::carrier_operands::boundary_operand;
use crate::checked_values::{scalar_constant, scalar_place};
use crate::closures::{function_assignment, packed_input, parameter_representation};
use crate::collections::is_collection;
use crate::element_borrows::shared_element_place;
use crate::element_places::element_place_type;
use crate::element_places::is_element_place;
use crate::explicit_arguments::{argument_access, explicit_place_argument, reference_contract};
use crate::formatting::display_operand;
use crate::frame::{
    Builder, forget_moved_arguments, lend_invocation_storage, literal, place_type, read_place, temporary, value_type,
};
use crate::frozen_equality::{borrow_frozen, frozen_place, frozen_reference_type};
use crate::generators::is_generator_type;
use crate::generators::{generator_reference, is_generator_reference};
use crate::layout_types::native_source_type;
use crate::list_iteration::list_root_place;
use crate::list_methods::materialized_receiver;
use crate::list_values::value_operand;
use crate::list_values::{checked_list_argument, cloned_list, list_borrow};
use crate::lists::exclusive_reference;
use crate::lists::{is_list, is_mutable_list_reference};
use crate::mutable_parameters::mutable_argument;
use crate::numerics::numeric_carrier;
use crate::operand_borrows::{borrow_scalar_operand, borrow_slice_operand, borrow_str_operand, borrow_text_operand};
use crate::pending::untranslated;
use crate::statements::is_nominal_type;
use crate::text_plan::owned_literal;

/// A deferred producer owns its value parameters; ordinary frames retain the shared collection ABI.
///
/// Call materialization, frame locals and constructor signatures use this same storage decision so an escaping
/// generator never captures a borrow of a caller's list snapshot. Explicit inputs select their checked access first;
/// deferred producers still refuse that contract until retained dependencies are represented. Inferred shared access
/// arrives at a type with a shared executable form for a private callable, and at any read-only type for a published
/// callable, as on the base; lowering lends both through the same carriers.
pub fn stored_parameter_type(body: &Body, parameter: &CallableParam, ty: PlanType) -> Result<PlanType, String> {
    if parameter.input_access != IncanInputAccess::Inferred {
        if body.is_generator() {
            return Err("unsupported Body IR explicit input access in a deferred producer".into());
        }
        return packed_input(
            ty,
            parameter.kind,
            parameter.mutable,
            parameter.native_owned,
            parameter.input_access,
        );
    }
    if body.is_generator() && !parameter.mutable {
        return Ok(ty);
    }
    packed_input(
        ty,
        parameter.kind,
        parameter.mutable,
        parameter.native_owned,
        parameter.input_access,
    )
}

/// Report whether a value that owns storage has no exclusive reference a `mut` parameter could receive.
pub fn lacks_exclusive_parameter(ty: &PlanType) -> bool {
    if !is_nominal_type(ty) || is_generator_type(ty) {
        return false;
    }
    exclusive_reference(ty).is_none()
}

/// Whether a parameter takes a list, set or dictionary by value rather than through a borrow.
pub fn owned_collection(ty: &PlanType) -> bool {
    matches!(ty, PlanType::List(..) | PlanType::Set(_) | PlanType::Dict(..))
}

/// The representation a call passes for one declared parameter, shared by functions and methods.
///
/// A list, set or dictionary parameter takes a shared or, when `mut`, an exclusive reference to the caller's storage;
/// `mut` text, models and enums take an exclusive reference to the caller's value; a `mut` scalar is the callee's own
/// copy. Rest parameters consume their checked owned packs. A `mut` tuple borrows its complete layout; an iterator
/// without retained exclusive storage stays refused.
pub fn call_parameter_type(builder: &Builder, body: &Body, parameter: &CallableParam) -> Result<PlanType, String> {
    let declared = native_source_type(
        builder.constructors(),
        builder.nominals(),
        builder.enums(),
        &builder.unions,
        &parameter.ty,
        &builder.intrinsics,
        &builder.file,
    )?;
    let ty = parameter_representation(body, parameter.local.index(), declared)?;
    if parameter.input_access == IncanInputAccess::Inferred && parameter.mutable && lacks_exclusive_parameter(&ty) {
        return Err("unsupported Body IR mutable tuple or iterator parameter".into());
    }
    stored_parameter_type(body, parameter, ty)
}

/// Report whether a resolved call leaves at least one declared slot to its default.
pub fn has_defaulted_slots(binding: &ArgumentBinding) -> bool {
    match binding {
        ArgumentBinding::Resolved { defaulted_slots, .. }
        | ArgumentBinding::ResolvedDeclarationOrder { defaulted_slots, .. } => !defaulted_slots.is_empty(),
        _ => false,
    }
}

/// How a call fills its declared slots: an offset past a separately lowered receiver, owned collection slots, a
/// positional reading of an unresolved binding, and the callee's source input accesses.
#[derive(Clone, Debug, Default)]
pub struct SlotPolicy {
    pub offset: usize,
    pub owned: bool,
    pub positional: bool,
    pub accesses: Vec<IncanInputAccess>,
}

/// Place the written arguments into the callee's declared parameter slots.
///
/// Body IR has already evaluated arguments in the binding's recorded order. Fixed named free calls and imported SDK
/// factories can retain declaration order; written-order operations keep their explicit policy. The binding says which
/// slot each materialized value fills. Defaults, missing slots, named or spread elements are refused. An offset of one
/// skips a method receiver, whose borrow is lowered separately and whose slot is absent from the ordinary argument
/// binding. With `owned`, a collection slot receives an owned value, as an SDK crate's compiled signature takes it.
///
/// With `positional`, an unresolved binding whose elements are exactly one plain value per declared slot fills the
/// slots in written order: without a name, a spread or an omitted slot, position is the only binding. An SDK crate
/// call to an overloaded function arrives this way, since the checker records no slots for an overload set.
pub fn ordered_slot_operands(
    builder: &mut Builder,
    binding: &ArgumentBinding,
    elements: &[ArgumentElement],
    types: &[PlanType],
    span: &SourceSpan,
    policy: &SlotPolicy,
) -> Result<Vec<Operand>, String> {
    if policy.offset > elements.len() {
        return Err("invalid Body IR argument offset".into());
    }
    let count = types.len();
    if !policy.accesses.is_empty() && policy.accesses.len() != count {
        return Err("invalid Body IR argument input access count".into());
    }
    match binding {
        ArgumentBinding::Resolved {
            arguments,
            defaulted_slots,
        } => resolved_slot_operands(builder, arguments, defaulted_slots.len(), elements, types, span, policy),
        ArgumentBinding::ResolvedDeclarationOrder {
            arguments,
            defaulted_slots,
        } => {
            for index in 1..arguments.len() {
                if arguments[index].slot <= arguments[index - 1].slot {
                    return Err("unsupported Body IR binding outside declaration order".into());
                }
            }
            resolved_slot_operands(builder, arguments, defaulted_slots.len(), elements, types, span, policy)
        }
        ArgumentBinding::UnresolvedPositional => {
            if !policy.positional || elements.len() - policy.offset != count {
                return Err("unsupported Body IR unresolved ArgumentBinding".into());
            }
            let mut operands = Vec::with_capacity(count);
            for index in 0..count {
                match &elements[index + policy.offset] {
                    ArgumentElement::One(value) => {
                        let access = argument_access(&policy.accesses, count, index)?;
                        operands.push(call_argument_operand(
                            builder,
                            value,
                            &types[index],
                            span,
                            policy.owned,
                            access,
                        )?);
                    }
                    _ => return Err("unsupported Body IR named or spread ArgumentElement".into()),
                }
            }
            Ok(operands)
        }
    }
}

/// Pass already evaluated resolved values by slot, preserving the ownership order their binding variant records.
fn resolved_slot_operands(
    builder: &mut Builder,
    bindings: &[BoundArgument],
    default_count: usize,
    elements: &[ArgumentElement],
    types: &[PlanType],
    span: &SourceSpan,
    policy: &SlotPolicy,
) -> Result<Vec<Operand>, String> {
    let count = types.len();
    if default_count != 0 || bindings.len() != elements.len() - policy.offset || bindings.len() != count {
        return Err("unsupported Body IR defaulted or incomplete ArgumentBinding".into());
    }
    let mut operands: Vec<Operand> = (0..count).map(|_| literal(Constant::Unit, span)).collect();
    let mut occupied: Vec<usize> = Vec::new();
    for (index, bound) in bindings.iter().enumerate() {
        let slot = bound.slot;
        if slot >= count || occupied.contains(&slot) {
            return Err("invalid Body IR argument slot map".into());
        }
        occupied.push(slot);
        match elements[index + policy.offset].as_one() {
            Some(value) => {
                let access = argument_access(&policy.accesses, count, slot)?;
                operands[slot] = call_argument_operand(builder, value, &types[slot], span, policy.owned, access)?;
            }
            None => return Err("unsupported Body IR named or spread ArgumentElement".into()),
        }
    }
    Ok(operands)
}

/// Honor explicit input authority before ordinary collection borrowing, callable adaptation, or boundary coercion.
pub fn call_argument_operand(
    builder: &mut Builder,
    value: &SourceOperand,
    ty: &PlanType,
    span: &SourceSpan,
    owned: bool,
    input_access: IncanInputAccess,
) -> Result<Operand, String> {
    let _ = owned;
    if input_access != IncanInputAccess::Inferred {
        return explicit_argument_operand(builder, value, ty, span, input_access);
    }
    match ty {
        PlanType::ClosureRef(signature) => return callable_argument(builder, value, signature, span, false),
        PlanType::BorrowingClosure(signature) | PlanType::BorrowingClosureRef(signature) => {
            return callable_argument(builder, value, signature, span, true);
        }
        PlanType::StringRef => return borrow_text_operand(builder, value, span),
        PlanType::StrRef => return borrow_str_operand(builder, value, span),
        PlanType::ValidationErrorRef => return Err(untranslated("validation_errors::validation_error_reference")),
        PlanType::SliceRef(_) | PlanType::SliceMutRef(_) => {
            return borrow_slice_operand(
                builder,
                value,
                slice_elements(ty),
                matches!(ty, PlanType::SliceMutRef(_)),
                span,
            );
        }
        PlanType::ScalarRef(_) | PlanType::ScalarMutRef(_) => {
            return borrow_scalar_operand(
                builder,
                value,
                slice_elements(ty),
                matches!(ty, PlanType::ScalarMutRef(_)),
                span,
            );
        }
        PlanType::FrozenCollectionRef(..) => return frozen_reference_argument(builder, value, ty, span),
        _ => {}
    }
    if owned_collection(ty) {
        // The checked ABI distinguishes owned packs and SDK values from ordinary collection references.
        return value_operand(builder, value, span);
    }
    if is_list(ty) || is_collection(ty) {
        if let Some(lent) = stored_element_list(builder, value, ty, span)? {
            return Ok(lent);
        }
        if is_mutable_list_reference(ty)
            && let Some(copy) = loop_variable_copy(builder, value, span)?
        {
            return list_borrow(builder, copy, true, span);
        }
        return checked_list_argument(builder, value, is_mutable_list_reference(ty), span);
    }
    if is_generator_reference(ty) {
        let SourceOperand::Place(read) = value else {
            return Err("unsupported Body IR constant generator argument".into());
        };
        if read.fact != OwnershipFact::MutBorrow {
            return Err("unsupported Body IR generator argument without mutable borrow fact".into());
        }
        let root = list_root_place(builder, &read.place, span)?;
        return generator_reference(builder, root, span);
    }
    if *ty == PlanType::String {
        let text = display_operand(builder, value, span)?;
        return Ok(read_place(text, true, span));
    }
    match ty {
        PlanType::ModelRef(..) | PlanType::EnumRef(..) | PlanType::TupleRef(_) => {
            return shared_nominal_argument(builder, value, ty, span);
        }
        PlanType::StringMutRef | PlanType::ModelMutRef(..) | PlanType::EnumMutRef(..) | PlanType::TupleMutRef(_) => {
            return mutable_argument(builder, value, ty, span);
        }
        _ => {}
    }
    let operand = boundary_operand(builder, value, ty, span)?;
    let coerced = coerce_numeric_operand(builder, operand, ty, span)?;
    function_argument(builder, ty, coerced, span)
}

/// Reify an item passed into a function-pointer signature without changing its inferred binding type.
pub fn function_argument(
    builder: &mut Builder,
    ty: &PlanType,
    value: Operand,
    span: &SourceSpan,
) -> Result<Operand, String> {
    let PlanType::FunctionPointer(_) = ty else {
        return Ok(value);
    };
    let source = value_type(builder, &value)?;
    let context = format!("call argument in {}", builder.name);
    let (_, operation) = function_assignment(ty, &source, value.clone(), true, &context)?;
    if let RvalueKind::ReifyFunction(_) = operation {
        let result = temporary(builder, ty.clone(), span);
        builder.emit(plan_assign(result.clone(), operation, span));
        return Ok(read_place(result, false, span));
    }
    Ok(value)
}

/// Borrow a checked nominal or enum, preserving shared list element loans and existing frame references.
pub fn shared_nominal_argument(
    builder: &mut Builder,
    value: &SourceOperand,
    ty: &PlanType,
    span: &SourceSpan,
) -> Result<Operand, String> {
    let SourceOperand::Place(read) = value else {
        return Err("unsupported Body IR shared receiver without a place".into());
    };
    let receiver = if is_element_place(builder, &read.place)? {
        let shared = if read.fact == OwnershipFact::Borrow {
            shared_element_place(builder, &read.place, span)?
        } else {
            None
        };
        match shared {
            Some(place) => place,
            None => materialized_receiver(builder, &read.place, span)?,
        }
    } else {
        scalar_place(builder, &read.place, span)?
    };
    let source = place_type(&builder.locals, &receiver);
    if source == *ty {
        return Ok(read_place(receiver, false, span));
    }
    match &source {
        PlanType::Enum(owner, name) => {
            if *ty != PlanType::EnumRef(*owner, name.clone()) {
                return Err(format!("unsupported Body IR shared enum owner mismatch for {name}"));
            }
        }
        PlanType::Tuple(elements) => {
            if *ty != PlanType::TupleRef(elements.clone()) {
                return Err("unsupported Body IR shared tuple owner mismatch".into());
            }
        }
        PlanType::Model(owner, name) => {
            if *ty != PlanType::ModelRef(*owner, name.clone()) {
                return Err(format!(
                    "unsupported Body IR shared receiver owner mismatch: {source:?} requires {ty:?} in {} at {}:{}",
                    builder.name, span.file, span.line
                ));
            }
        }
        _ => return Err("unsupported Body IR shared receiver outside its nominal frame".into()),
    }
    let reference = temporary(builder, ty.clone(), span);
    builder.emit(plan_assign(reference.clone(), RvalueKind::Borrow(receiver), span));
    Ok(read_place(reference, false, span))
}

/// Consume checked storage or materialize fresh inferred-shared and owned literals for this invocation.
fn explicit_argument_operand(
    builder: &mut Builder,
    value: &SourceOperand,
    ty: &PlanType,
    span: &SourceSpan,
    access: IncanInputAccess,
) -> Result<Operand, String> {
    if access == IncanInputAccess::InferredShared {
        return inferred_shared_argument(builder, value, ty, span);
    }
    match value {
        SourceOperand::Place(read) => {
            if read.place.global().is_some() {
                return Err("unsupported Body IR explicit argument from static storage".into());
            }
            if is_element_place(builder, &read.place)? {
                return Err(untranslated("call_arguments::explicit indexed argument"));
            }
            let source = scalar_place(builder, &read.place, span)?;
            explicit_place_argument(builder, source, read.fact, ty, access, span)
        }
        SourceOperand::Constant(constant) => explicit_constant_argument(builder, constant, ty, access, span),
    }
}

/// Lend a plain input whose callee published shared access.
///
/// That access already decided to lend, so the read's ownership fact, which describes a by-value call, decides
/// nothing here. Only this frame's plan types are exact after instantiation.
///
/// 1. Lend a stored owner of the selected type, a reference to it, or a shared list element directly.
/// 2. Use the ordinary lending route of a carrier that has one; it applies conversions such as a range becoming a list.
/// 3. Apply any other checked conversion to fresh storage and lend that.
fn inferred_shared_argument(
    builder: &mut Builder,
    value: &SourceOperand,
    expected: &PlanType,
    span: &SourceSpan,
) -> Result<Operand, String> {
    let Ok((referent, writable)) = reference_contract(expected) else {
        return call_argument_operand(builder, value, expected, span, false, IncanInputAccess::Inferred);
    };
    if writable {
        return Err("invalid Body IR inferred shared reference carrier".into());
    }
    // 1. Exact storage needs no copy and no conversion.
    if let Some(source) = exact_shared_storage(builder, value, &referent, span)? {
        return explicit_place_argument(
            builder,
            source,
            OwnershipFact::Borrow,
            expected,
            IncanInputAccess::InferredShared,
            span,
        );
    }
    // 2. These carriers already lend through the route an ordinary parameter of that carrier takes.
    if has_ordinary_lending_route(expected) {
        return call_argument_operand(builder, value, expected, span, false, IncanInputAccess::Inferred);
    }
    // 3. A union member becoming its union, for example, has no storage of the selected type yet.
    materialized_shared_argument(builder, value, expected, referent, span)
}

/// Reach storage that already holds the selected type, or a reference to it, without copying or converting.
fn exact_shared_storage(
    builder: &mut Builder,
    value: &SourceOperand,
    referent: &PlanType,
    span: &SourceSpan,
) -> Result<Option<Place>, String> {
    let SourceOperand::Place(read) = value else {
        return Ok(None);
    };
    if read.place.global().is_some() {
        return Ok(None);
    }
    if is_element_place(builder, &read.place)? {
        if let Ok(element) = element_place_type(builder, &read.place)
            && element == *referent
        {
            return shared_element_place(builder, &read.place, span);
        }
        return Ok(None);
    }
    let source = scalar_place(builder, &read.place, span)?;
    let actual = place_type(&builder.locals, &source);
    if actual == *referent {
        return Ok(Some(source));
    }
    if let Ok((held, _)) = reference_contract(&actual)
        && held == *referent
    {
        return Ok(Some(source));
    }
    Ok(None)
}

/// Whether `call_argument_operand` lends this reference carrier itself, including its call-site conversions.
fn has_ordinary_lending_route(ty: &PlanType) -> bool {
    is_list(ty)
        || is_collection(ty)
        || matches!(
            ty,
            PlanType::ValidationErrorRef | PlanType::SliceRef(_) | PlanType::ScalarRef(_)
        )
}

/// Apply a checked conversion to fresh owned storage, then lend that exact storage for this invocation.
///
/// The frame owns that storage only for the call: it is dropped when the call returns, and by the call's unwind
/// cleanup. An inferred-shared input is never the source of a returned view, so nothing reads it afterward.
fn materialized_shared_argument(
    builder: &mut Builder,
    value: &SourceOperand,
    expected: &PlanType,
    referent: PlanType,
    span: &SourceSpan,
) -> Result<Operand, String> {
    let materialized = call_argument_operand(builder, value, &referent, span, false, IncanInputAccess::Inferred)?;
    if value_type(builder, &materialized)? != referent {
        return Err("unsupported Body IR inferred shared materialization mismatch".into());
    }
    let storage = temporary(builder, referent, span);
    forget_moved_arguments(builder, std::slice::from_ref(&materialized));
    builder.emit(plan_assign(storage.clone(), RvalueKind::Use(materialized), span));
    let lent = explicit_place_argument(
        builder,
        storage.clone(),
        OwnershipFact::Borrow,
        expected,
        IncanInputAccess::InferredShared,
        span,
    )?;
    lend_invocation_storage(builder, &storage, &lent);
    Ok(lent)
}

/// Give an inferred-shared fresh literal invocation storage; explicit loans still require authored storage.
fn explicit_constant_argument(
    builder: &mut Builder,
    constant: &SourceConstant,
    expected: &PlanType,
    access: IncanInputAccess,
    span: &SourceSpan,
) -> Result<Operand, String> {
    if access != IncanInputAccess::Owned {
        return Err("unsupported Body IR borrowed constant without retained local storage".into());
    }
    if let SourceConstant::Str(text) = constant {
        if *expected != PlanType::String {
            return Err("unsupported Body IR owned text literal carrier mismatch".into());
        }
        return Ok(read_place(owned_literal(builder, text, span), true, span));
    }
    if !numeric_carrier(expected) && !matches!(expected, PlanType::Bool | PlanType::Unit) {
        return Err("unsupported Body IR explicit owned constant carrier".into());
    }
    let literal = scalar_constant(constant, span)?;
    let converted = coerce_numeric_operand(builder, literal, expected, span)?;
    if value_type(builder, &converted)? != *expected {
        return Err("unsupported Body IR explicit owned constant carrier mismatch".into());
    }
    Ok(converted)
}

/// Lend an existing frozen view, or borrow the exact frozen storage a checked argument names.
fn frozen_reference_argument(
    builder: &mut Builder,
    value: &SourceOperand,
    ty: &PlanType,
    span: &SourceSpan,
) -> Result<Operand, String> {
    if let SourceOperand::Place(read) = value
        && !is_element_place(builder, &read.place)?
    {
        let existing = scalar_place(builder, &read.place, span)?;
        if place_type(&builder.locals, &existing) == *ty {
            return Ok(read_place(existing, false, span));
        }
    }
    match frozen_place(builder, value, span)? {
        Some(storage) => {
            if frozen_reference_type(&place_type(&builder.locals, &storage))? != *ty {
                return Err("unsupported Body IR frozen argument owner mismatch".into());
            }
            borrow_frozen(builder, storage, span)
        }
        None => Err("unsupported Body IR native frozen argument without exact storage".into()),
    }
}

/// Lend a list stored inside a list element, such as `stored.variants[position].labels`, to a shared list parameter
/// through the element's own loan, so the call copies nothing.
///
/// Only a shared parameter of exactly the stored list's type qualifies. A slice, a moved projection, a mutable
/// parameter and a list of another shape keep the copy `checked_list_argument` makes for them.
fn stored_element_list(
    builder: &mut Builder,
    value: &SourceOperand,
    ty: &PlanType,
    span: &SourceSpan,
) -> Result<Option<Operand>, String> {
    let SourceOperand::Place(read) = value else {
        return Ok(None);
    };
    if read.fact != OwnershipFact::Borrow || !is_element_place(builder, &read.place)? {
        return Ok(None);
    }
    let Ok((referent, writable)) = reference_contract(ty) else {
        return Ok(None);
    };
    if writable || !is_list(&referent) {
        return Ok(None);
    }
    match element_place_type(builder, &read.place) {
        Ok(element) if element == referent => {}
        _ => return Ok(None),
    }
    match shared_element_place(builder, &read.place, span)? {
        Some(stored) => Ok(Some(list_borrow(builder, stored, false, span)?)),
        None => Ok(None),
    }
}

/// Copy a list loop variable, or a list field of one, that a function's `mut` parameter receives.
///
/// Legacy passes such a parameter `&mut item.clone()`, whether the loop copies its items or iterates them in place, so
/// the callee's changes never reach the iterated list. A method receiver is not an argument and changes the item.
fn loop_variable_copy(
    builder: &mut Builder,
    value: &SourceOperand,
    span: &SourceSpan,
) -> Result<Option<Place>, String> {
    let SourceOperand::Place(read) = value else {
        return Ok(None);
    };
    if read.place.global().is_some() {
        return Ok(None);
    }
    // A slice or indexed projection is an argument value, not the binding of a copied loop item. Its ordinary
    // projected-argument path materializes the fresh owner or reaches the checked element storage.
    if is_element_place(builder, &read.place)? {
        return Ok(None);
    }
    let root = list_root_place(builder, &read.place, span)?;
    if !is_list(&place_type(&builder.locals, &root)) {
        return Ok(None);
    }
    if builder.borrowed_items.contains(&root.local) || builder.in_place_items.contains(&root.local) {
        return Ok(Some(cloned_list(builder, root, span)?));
    }
    Ok(None)
}

/// The element types a slice or scalar reference carrier names.
fn slice_elements(ty: &PlanType) -> &[PlanType] {
    match ty {
        PlanType::SliceRef(elements)
        | PlanType::SliceMutRef(elements)
        | PlanType::ScalarRef(elements)
        | PlanType::ScalarMutRef(elements) => elements,
        _ => &[],
    }
}
