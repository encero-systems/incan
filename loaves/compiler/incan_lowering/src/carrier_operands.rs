//! Values at typed boundaries: carrier construction, native aliases and payload conversion.

use incan_lang::lang::types::collections::{CollectionTypeId, from_str as collection_type};
use incan_mir_plan_native::{
    Carrier, Constant, FieldProjection, ListLeaf, Operand, OperandKind, Place, PlanType, Projection, RvalueKind,
    SourceSpan, TerminatorKind, assign as plan_assign, carrier_position, place as plan_place,
};
use incan_semantics_core::body_ir::{Operand as SourceOperand, OwnershipFact, Pattern};
use incan_semantics_core::{IncanType, SemanticSourceTargetKind};

use crate::builtin_calls::{clone_enum_reference, clone_model_reference, coerce_numeric_operand};
use crate::callable_calls::carrier_callable_operand;
use crate::callable_values::owned_callable_value;
use crate::checked_values::{reference_read_fact, scalar_place};
use crate::closures::function_assignment;
use crate::compiler_record_adapters::semantic_key_matches_native;
use crate::decls::library_enum_item;
use crate::enums::enum_owner;
use crate::formatting::display_plan_operand;
use crate::frame::{
    Builder, appended_field, bind_read_value, dereferenced_place, forget_moved_arguments, forget_owned, literal,
    place_type, read_place, temporary, track_rvalue_moves, value_type,
};
use crate::frozen_plan::frozen_bytes_literal;
use crate::generators::generator_shape;
use crate::layout_types::shared_receiver_type;
use crate::layout_types::{native_alias_boundary, native_source_type};
use crate::list_values::sequence_read_storage;
use crate::lists::exclusive_reference;
use crate::lists::is_exclusive_borrow;
use crate::lists::is_list;
use crate::matches::{bind_enum_payload, bind_structural_pattern, enum_payload_read};
use crate::numerics::numeric_carrier;
use crate::operators::enum_comparison_tag;
use crate::option_tests::{is_option_carrier, option_isinstance};
use crate::options::{assign_carrier, variant_condition};
use crate::patterns::{carrier_pattern_slot, carrier_payload_types, pattern_binds};
use crate::result_carriers::{frozen_operand, typed_carrier_operand};
use crate::static_storage::snapshot_owns;
use crate::text_plan::{clone_text_reference, owned_slice_text};
use crate::unions::{carrier_injection, test_union_tag, union_test_slots};

/// Preserve checked static literals and materialize carrier construction at a typed value boundary.
pub fn boundary_operand(
    builder: &mut Builder,
    value: &SourceOperand,
    target: &PlanType,
    span: &SourceSpan,
) -> Result<Operand, String> {
    if *target == PlanType::FrozenStr {
        return frozen_operand(builder, value, span);
    }
    if *target == PlanType::FrozenBytes
        && let SourceOperand::Constant(constant) = value
    {
        return frozen_bytes_literal(builder, constant, span);
    }
    let operand = typed_carrier_operand(builder, value, target, span)?;
    if generator_shape(&value_type(builder, &operand)?).is_some() && is_list(target) {
        return Err("unsupported Body IR generator value at a list boundary".into());
    }
    boundary_plan_operand(builder, operand, target, span)
}

/// Evaluate a collection element at the element's checked carrier.
///
/// The checker types each element against the collection, so a frozen `str` constant becomes owned text and a
/// narrower numeric carrier widens to the element's carrier, exactly as at any other typed boundary.
pub fn element_operand(
    builder: &mut Builder,
    value: &SourceOperand,
    element: &PlanType,
    span: &SourceSpan,
) -> Result<Operand, String> {
    let operand = boundary_operand(builder, value, element, span)?;
    coerce_numeric_operand(builder, operand, element, span)
}

/// Convert an already evaluated collection element to the element's checked carrier, as `element_operand` does.
pub fn element_plan_operand(
    builder: &mut Builder,
    value: Operand,
    element: &PlanType,
    span: &SourceSpan,
) -> Result<Operand, String> {
    let operand = boundary_plan_operand(builder, value, element, span)?;
    coerce_numeric_operand(builder, operand, element, span)
}

/// Convert an evaluated value to its checked boundary carrier, retaining exact native aliases, tuple layouts and
/// ownership without repeating source effects.
pub fn boundary_plan_operand(
    builder: &mut Builder,
    value: Operand,
    target: &PlanType,
    span: &SourceSpan,
) -> Result<Operand, String> {
    let source = value_type(builder, &value)?;
    if let Some(copied) = copied_native_scalar_referent(&value, &source, target, span)? {
        return Ok(copied);
    }
    if source == PlanType::FrozenBytes && *target == PlanType::List(ListLeaf::U8, 1) {
        let frozen = temporary(builder, source, span);
        builder.emit(plan_assign(frozen.clone(), RvalueKind::Use(value), span));
        let storage = sequence_read_storage(builder, frozen, span)?;
        return Ok(read_place(storage, true, span));
    }
    if source == PlanType::StrRef && *target == PlanType::String {
        let owned = owned_slice_text(builder, value, span);
        return Ok(read_place(owned, true, span));
    }
    // Text an inferred-shared parameter lends reaching a destination that owns text is that destination's copy.
    if source == PlanType::StringRef && *target == PlanType::String {
        return Ok(read_place(clone_text_reference(builder, value, span)?, true, span));
    }
    // A `const` str reaching a destination that owns text, such as an `Err` payload or a list element, becomes owned
    // text there, as at a `str` parameter.
    if source == PlanType::FrozenStr && *target == PlanType::String {
        let text = display_plan_operand(builder, value, span)?;
        return Ok(read_place(text, true, span));
    }
    // A borrowed source model that reaches a destination owning one was admitted by the checker there, so it is cloned.
    match (&source, target) {
        (PlanType::Tuple(_), PlanType::Tuple(_)) => {
            if source != *target {
                return Err("unsupported Body IR tuple value with differing checked field carriers".into());
            }
        }
        (PlanType::ModelRef(owner, name), PlanType::Model(target_owner, _)) if owner == target_owner => {
            return Ok(clone_model_reference(builder, value, *owner, name, span));
        }
        (PlanType::EnumRef(owner, name), PlanType::Enum(target_owner, _)) if owner == target_owner => {
            return Ok(clone_enum_reference(builder, value, *owner, name, span));
        }
        _ => {}
    }
    if converted_carrier(builder, &source, target) {
        return converted_carrier_operand(builder, value, &source, target, span);
    }
    match target {
        PlanType::Closure(signature) => return owned_callable_value(builder, value, signature, span),
        PlanType::FunctionPointer(_) => {
            let source = value_type(builder, &value)?;
            let context = format!("value boundary in {}", builder.name);
            let (ty, operation) = function_assignment(target, &source, value, true, &context)?;
            let pointer = temporary(builder, ty, span);
            builder.emit(plan_assign(pointer.clone(), operation, span));
            return Ok(read_place(pointer, false, span));
        }
        _ => {}
    }
    let mut operand = value;
    if source == PlanType::FrozenStr && frozen_text_union_member(builder, target)? {
        let text = display_plan_operand(builder, operand, span)?;
        operand = read_place(text, true, span);
    }
    operand = owned_union_member(builder, operand, target, span)?;
    operand = carrier_callable_operand(builder, operand, target, span)?;
    operand = option_payload_operand(builder, operand, target, span)?;
    let actual_source = value_type(builder, &operand)?;
    let operation = carrier_injection(builder, operand, &actual_source, target)?;
    match operation {
        RvalueKind::Use(result) => {
            if actual_source == *target || !native_alias_boundary(builder.nominals(), &actual_source, target) {
                return Ok(result);
            }
            let alias = temporary(builder, target.clone(), span);
            forget_moved_arguments(builder, std::slice::from_ref(&result));
            let alias_operation = RvalueKind::NativeAlias(result, actual_source, target.clone());
            builder.emit(plan_assign(alias.clone(), alias_operation, span));
            let owns = snapshot_owns(target);
            if owns {
                builder.owned.push(alias.local);
            }
            Ok(read_place(alias, owns, span))
        }
        _ => {
            let output = temporary(builder, target.clone(), span);
            track_rvalue_moves(builder, &operation);
            builder.emit(plan_assign(output.clone(), operation, span));
            builder.owned.push(output.local);
            Ok(read_place(output, true, span))
        }
    }
}

/// A `const` str reaching a declared union that holds owned text but no frozen text converts to that owned text.
fn frozen_text_union_member(builder: &Builder, target: &PlanType) -> Result<bool, String> {
    let PlanType::Enum(owner, _) = target else {
        return Ok(false);
    };
    if *owner < builder.enums().len() as i64 {
        return Ok(false);
    }
    let declaration = &builder.intrinsics[carrier_position(*owner) as usize];
    if declaration.carrier != Carrier::Declared {
        return Ok(false);
    }
    for union in &builder.unions {
        if !semantic_key_matches_native(&declaration.semantic_key, union)? {
            continue;
        }
        let members: Vec<&PlanType> = declaration
            .variants
            .iter()
            .filter_map(|variant| variant.fields.first())
            .collect();
        return Ok(!members.contains(&&PlanType::FrozenStr) && members.contains(&&PlanType::String));
    }
    Ok(false)
}

/// A union payload owns its member, so a nominal a shared parameter lends becomes the payload's copy at the union.
///
/// A field of a parameter the body only reads arrives as a shared reference to the field; its declared union member is
/// the owned nominal, which is cloned from that reference. Every other operand is returned unchanged.
fn owned_union_member(
    builder: &mut Builder,
    value: Operand,
    target: &PlanType,
    span: &SourceSpan,
) -> Result<Operand, String> {
    let PlanType::Enum(owner, _) = target else {
        return Ok(value);
    };
    let index = carrier_position(*owner);
    if index < 0
        || index as usize >= builder.intrinsics.len()
        || builder.intrinsics[index as usize].carrier != Carrier::Declared
    {
        return Ok(value);
    }
    match value_type(builder, &value)? {
        PlanType::ModelRef(nominal, name) => Ok(clone_model_reference(builder, value, nominal, &name, span)),
        PlanType::EnumRef(nominal, name) => Ok(clone_enum_reference(builder, value, nominal, &name, span)),
        _ => Ok(value),
    }
}

/// Copy a signature-borrowed scalar through its native reference when the checked boundary expects its referent.
fn copied_native_scalar_referent(
    value: &Operand,
    source: &PlanType,
    target: &PlanType,
    span: &SourceSpan,
) -> Result<Option<Operand>, String> {
    let (PlanType::ScalarRef(elements) | PlanType::ScalarMutRef(elements)) = source else {
        return Ok(None);
    };
    if elements.len() != 1 || elements[0] != *target {
        return Ok(None);
    }
    match &value.kind {
        OperandKind::Copy(place) | OperandKind::Move(place) => {
            let referent = dereferenced_place(place, target.clone(), span)?;
            Ok(Some(read_place(referent, false, span)))
        }
        _ => Err("invalid Body IR native scalar reference operand".into()),
    }
}

/// Whether checked Option/Result payload carriers convert without changing the active variant; exact Option payloads
/// stay on the wrapping boundary.
fn converted_carrier(builder: &Builder, source: &PlanType, target: &PlanType) -> bool {
    if source == target {
        return false;
    }
    let (PlanType::Enum(source_owner, _), PlanType::Enum(target_owner, _)) = (source, target) else {
        return false;
    };
    let enums = builder.enums().len() as i64;
    if *source_owner < enums || *target_owner < enums {
        return false;
    }
    let before = &builder.intrinsics[carrier_position(*source_owner) as usize];
    let after = &builder.intrinsics[carrier_position(*target_owner) as usize];
    if before.carrier == Carrier::Declared || before.carrier != after.carrier {
        return false;
    }
    // An existing optional payload lifts as a whole, including None inside Some(None).
    !(after.carrier == Carrier::Option && after.variants[1].fields.len() == 1 && after.variants[1].fields[0] == *source)
}

/// Convert checked Option/Result payload carriers without changing the active variant or duplicating owners; exact
/// Option payloads stay on the wrapping boundary. `converted_carrier` has already selected the conversion.
fn converted_carrier_operand(
    builder: &mut Builder,
    value: Operand,
    source_type: &PlanType,
    target: &PlanType,
    span: &SourceSpan,
) -> Result<Operand, String> {
    let (PlanType::Enum(source_owner, _), PlanType::Enum(target_owner, _)) = (source_type, target) else {
        return Err("invalid Body IR carrier conversion".into());
    };
    let before = builder.intrinsics[carrier_position(*source_owner) as usize].clone();
    let after = builder.intrinsics[carrier_position(*target_owner) as usize].clone();
    let OperandKind::Move(source) = value.kind else {
        return Err("unsupported Body IR carrier conversion without an owned value".into());
    };
    let output = temporary(builder, target.clone(), span);
    let selected = builder.new_block(span);
    let other = builder.new_block(span);
    let joined = builder.new_block(span);
    let condition = variant_condition(builder, &source, 0, span);
    builder.finish(TerminatorKind::SwitchBool(condition, other, selected), span);
    forget_owned(builder, source.local);
    let owners = builder.owned.clone();
    for slot in 0..2usize {
        builder.select(if slot == 1 { other } else { selected });
        builder.owned = owners.clone();
        let (from, to) = (&before.variants[slot].fields, &after.variants[slot].fields);
        if from.len() != to.len() {
            return Err("unsupported Body IR carrier conversion variant layout".into());
        }
        let mut fields = Vec::with_capacity(from.len());
        for (index, payload_type) in from.iter().enumerate() {
            let payload = Place {
                local: source.local,
                projection: Projection::VariantField(slot as i64, index as i64, payload_type.clone()),
                span: span.clone(),
            };
            fields.push(boundary_plan_operand(
                builder,
                read_place(payload, true, span),
                &to[index],
                span,
            )?);
        }
        assign_carrier(builder, output.clone(), *target_owner, slot as i64, fields, span);
        builder.finish(TerminatorKind::Goto(joined), span);
    }
    builder.select(joined);
    builder.owned = owners;
    builder.owned.push(output.local);
    Ok(read_place(output, true, span))
}

/// Bring a value written into an `Option` to the type of that `Option`'s payload, which then wraps it in `Some`.
///
/// A value written into an `Option` of an `Option` or union is first injected into that inner carrier, so each layer
/// wraps it, and a number written into an `Option` of another numeric type converts first, as legacy writes it.
/// Every other value is returned unchanged.
fn option_payload_operand(
    builder: &mut Builder,
    value: Operand,
    target: &PlanType,
    span: &SourceSpan,
) -> Result<Operand, String> {
    let PlanType::Enum(owner, _) = target else {
        return Ok(value);
    };
    if *owner < builder.enums().len() as i64 {
        return Ok(value);
    }
    let declaration = &builder.intrinsics[carrier_position(*owner) as usize];
    if declaration.carrier != Carrier::Option {
        return Ok(value);
    }
    let payload = declaration.variants[1].fields[0].clone();
    let source = value_type(builder, &value)?;
    if source == *target || source == payload {
        return Ok(value);
    }
    if numeric_carrier(&source) && numeric_carrier(&payload) {
        return coerce_numeric_operand(builder, value, &payload, span);
    }
    match payload {
        PlanType::Enum(..) => boundary_plan_operand(builder, value, &payload, span),
        _ => Ok(value),
    }
}

/// Preserve a checker-retained shared reference binding: copy a reference payload or borrow its owned nominal field.
/// Match declaration slots rather than diagnostic names, and never materialize owned text or move an owner's payload
/// into a reference slot.
fn bind_shared_enum_payload(
    builder: &mut Builder,
    local: i64,
    payload: &Place,
    fact: OwnershipFact,
    span: &SourceSpan,
) -> Result<bool, String> {
    let target = builder.locals[local as usize].ty.clone();
    if !builder.reference_values.contains(&local) || is_exclusive_borrow(&target) {
        return Ok(false);
    }
    if !reference_read_fact(fact) {
        return Err(format!(
            "unsupported Body IR shared enum payload reference read {fact:?} in {}",
            builder.name
        ));
    }
    let source = place_type(&builder.locals, payload);
    if source == target {
        let value = read_place(payload.clone(), false, span);
        builder.emit(plan_assign(plan_place(local, span), RvalueKind::Use(value), span));
        return Ok(true);
    }
    match (&target, &source) {
        (PlanType::ModelRef(owner, _), PlanType::Model(held, _)) => {
            if owner != held {
                return Err("unsupported Body IR shared model payload owner mismatch".into());
            }
        }
        (PlanType::EnumRef(owner, _), PlanType::Enum(held, _)) => {
            if owner != held {
                return Err("unsupported Body IR shared enum payload owner mismatch".into());
            }
        }
        _ => return Ok(false),
    }
    builder.emit(plan_assign(
        plan_place(local, span),
        RvalueKind::Borrow(payload.clone()),
        span,
    ));
    Ok(true)
}

/// Bind checked enum payload slots through the same downcast and ownership path for source enums and unions.
pub fn bind_enum_fields(
    builder: &mut Builder,
    source: &Place,
    variant: i64,
    fields: &[Pattern],
    span: &SourceSpan,
) -> Result<(), String> {
    let owner = enum_owner(&place_type(&builder.locals, source))?;
    let payload_types = carrier_payload_types(builder, owner, variant)?;
    for (slot, field) in fields.iter().enumerate() {
        let Pattern::Var(binding) = field else {
            if pattern_binds(field) {
                bind_nested_payload(builder, source, variant, slot, field, span)?;
            }
            continue;
        };
        let local = binding.local.index() as i64 + 1 + builder.local_offset;
        let ty = builder.locals[local as usize].ty.clone();
        let mut projection = FieldProjection::new(slot as i64, payload_types[slot].clone());
        projection.variant = variant;
        let projected = appended_field(source, projection, span)?;
        if ty != payload_types[slot] {
            // A reference binding borrows its exact payload. Other representation changes, such as native borrowed text
            // becoming owned source text, keep their ordinary materialization path below.
            if let Ok(reference) = shared_receiver_type(payload_types[slot].clone())
                && ty == reference
            {
                builder.emit(plan_assign(
                    plan_place(local, span),
                    RvalueKind::Borrow(projected),
                    span,
                ));
                continue;
            }
            // An exclusive subject binds each payload as an exclusive reference to it.
            if let Some(exclusive) = exclusive_reference(&payload_types[slot])
                && ty == exclusive
            {
                builder.emit(plan_assign(
                    plan_place(local, span),
                    RvalueKind::MutBorrow(projected),
                    span,
                ));
                continue;
            }
        }
        if binding.fact == OwnershipFact::MutBorrow {
            bind_payload_in_place(builder, local, projected, span)?;
            continue;
        }
        if bind_shared_enum_payload(builder, local, &projected, binding.fact, span)? {
            continue;
        }
        let payload = moved_native_enum_payload(builder, source, projected, fields, binding.fact, span)?;
        let value = boundary_plan_operand(builder, payload, &ty, span)?;
        bind_read_value(builder, local, value, span);
    }
    Ok(())
}

/// Transfer a checked payload move, retiring its carrier's drop entry when present.
///
/// Intrinsic call temporaries can be drop-free; checked Body IR supplies their move authority. A source enum the frame
/// owns gives its single payload up the same way; any other source carrier keeps its payload, and the binding takes a
/// clone that it owns.
fn moved_native_enum_payload(
    builder: &mut Builder,
    source: &Place,
    payload: Place,
    fields: &[Pattern],
    fact: OwnershipFact,
    span: &SourceSpan,
) -> Result<Operand, String> {
    let owner = enum_owner(&place_type(&builder.locals, source))?;
    if fact != OwnershipFact::Move {
        return enum_payload_read(builder, payload, fact, span);
    }
    let source_enum = owner < builder.enums().len() as i64;
    if source_enum {
        if library_enum_item(&builder.enums()[owner as usize]).is_empty()
            && (fields.len() != 1 || source.projection != Projection::Whole || !builder.owned.contains(&source.local))
        {
            return enum_payload_read(builder, payload, fact, span);
        }
    } else if builder.intrinsics[carrier_position(owner) as usize].carrier == Carrier::Declared {
        return enum_payload_read(builder, payload, fact, span);
    }
    if fields.len() != 1 || source.projection != Projection::Whole {
        return Err("unsupported Body IR native enum payload move outside a whole owned single-field carrier".into());
    }
    if source_enum && !builder.owned.contains(&source.local) {
        return Err("unsupported Body IR native enum payload move without its carrier's drop owner".into());
    }
    forget_owned(builder, source.local);
    Ok(read_place(payload, true, span))
}

/// Bind the names a nested carrier or tuple payload pattern names, from the payload its tests already matched.
fn bind_nested_payload(
    builder: &mut Builder,
    source: &Place,
    variant: i64,
    slot: usize,
    pattern: &Pattern,
    span: &SourceSpan,
) -> Result<(), String> {
    let PlanType::Enum(owner, _) = place_type(&builder.locals, source) else {
        return Err("invalid Body IR nested payload carrier".into());
    };
    let types = carrier_payload_types(builder, owner, variant)?;
    let mut projection = FieldProjection::new(slot as i64, types[slot].clone());
    projection.variant = variant;
    let payload = appended_field(source, projection, span)?;
    match place_type(&builder.locals, &payload) {
        PlanType::Enum(nested_owner, _) => {
            let nested = carrier_pattern_slot(builder, nested_owner, pattern)?;
            bind_enum_payload(builder, &payload, nested, pattern, span)
        }
        _ => bind_structural_pattern(builder, &payload, pattern, span),
    }
}

/// Bind a name its arm changes as an exclusive reference to the payload it names, as legacy matches `&mut`.
///
/// The change then lands in the scrutinee itself. The reference owns nothing, so the binding never enters the drop
/// ledger. Tuple payloads retain their complete layout behind that reference. A scalar payload reaches here only for a
/// name written `mut`, which changes the scalar where it is stored, so it binds a scalar reference; every read and
/// assignment of the name then goes through that reference.
pub fn bind_payload_in_place(
    builder: &mut Builder,
    local: i64,
    payload: Place,
    span: &SourceSpan,
) -> Result<(), String> {
    let payload_type = place_type(&builder.locals, &payload);
    let reference = match exclusive_reference(&payload_type) {
        Some(exclusive) => exclusive,
        None => {
            if !numeric_carrier(&payload_type) && payload_type != PlanType::Bool {
                return Err("unsupported Body IR in-place pattern payload without an exclusive reference".into());
            }
            PlanType::ScalarMutRef(vec![payload_type])
        }
    };
    builder.locals[local as usize].ty = reference;
    builder.emit(plan_assign(
        plan_place(local, span),
        RvalueKind::MutBorrow(payload),
        span,
    ));
    Ok(())
}

/// Inspect carrier tags or a statically known nominal identity without consuming storage; distinct class tests remain
/// refused.
pub fn lower_union_isinstance(
    builder: &mut Builder,
    value: &SourceOperand,
    ty: &IncanType,
    target: &IncanType,
    span: &SourceSpan,
) -> Result<RvalueKind, String> {
    let source_ty = isinstance_source_type(builder, value, ty, span)?;
    let member = native_source_type(
        builder.constructors(),
        builder.nominals(),
        builder.enums(),
        &builder.unions,
        target,
        &builder.intrinsics,
        "",
    )?;
    let constant = |flag: bool| RvalueKind::Use(literal(Constant::Bool(flag), span));
    let text = |ty: &PlanType| matches!(ty, PlanType::String | PlanType::FrozenStr);
    match &source_ty {
        PlanType::Int | PlanType::Float | PlanType::Bool => return Ok(constant(source_ty == member)),
        PlanType::String | PlanType::FrozenStr => return Ok(constant(text(&member))),
        PlanType::Model(owner, _) => {
            if source_ty == member {
                return Ok(constant(true));
            }
            let newtype = usize::try_from(*owner)
                .ok()
                .and_then(|slot| builder.nominals().get(slot))
                .is_some_and(|declaration| declaration.canonical.kind == SemanticSourceTargetKind::Newtype);
            if !newtype {
                return Err("unsupported Body IR isinstance over a non-newtype nominal".into());
            }
            return Ok(constant(source_ty == member));
        }
        _ => {}
    }
    let owner = enum_owner(&source_ty)?;
    if is_option_carrier(builder, owner) {
        let option = isinstance_place(builder, value, span)?;
        return option_isinstance(builder, option, owner, &member, span);
    }
    let slots = union_test_slots(&builder.unions, owner, &member, &builder.intrinsics)?;
    if slots.is_empty() {
        return Ok(constant(false));
    }
    let source = isinstance_place(builder, value, span)?;
    // The same discriminant read as `option_tests::enum_tag`.
    let tag = enum_comparison_tag(builder, &source, span);
    Ok(test_union_tag(builder, &tag, &slots, span))
}

/// Use retained place storage for carrier views, preserving full union indexes after a checker-proven narrowing.
fn isinstance_source_type(
    builder: &mut Builder,
    value: &SourceOperand,
    checked: &IncanType,
    span: &SourceSpan,
) -> Result<PlanType, String> {
    if let IncanType::Generic { base, .. } = checked
        && (checked.is_union() || collection_type(base) == Some(CollectionTypeId::Option))
    {
        let source = isinstance_place(builder, value, span)?;
        return Ok(place_type(&builder.locals, &source));
    }
    native_source_type(
        builder.constructors(),
        builder.nominals(),
        builder.enums(),
        &builder.unions,
        checked,
        &builder.intrinsics,
        "",
    )
}

/// The storage whose tag `isinstance` reads in place.
fn isinstance_place(builder: &mut Builder, value: &SourceOperand, span: &SourceSpan) -> Result<Place, String> {
    match value {
        SourceOperand::Place(read) => scalar_place(builder, &read.place, span),
        SourceOperand::Constant(_) => Err("unsupported Body IR nonplace union isinstance".into()),
    }
}
