//! Values at typed boundaries: carrier construction, native aliases and payload conversion.

use incan_mir_plan_native::{
    Carrier, ListLeaf, Operand, OperandKind, PlanType, RvalueKind, SourceSpan, assign as plan_assign, carrier_position,
};
use incan_semantics_core::body_ir::Operand as SourceOperand;

use crate::builtin_calls::coerce_numeric_operand;
use crate::callable_calls::carrier_callable_operand;
use crate::compiler_record_adapters::semantic_key_matches_native;
use crate::formatting::display_plan_operand;
use crate::frame::{
    Builder, dereferenced_place, forget_moved_arguments, read_place, temporary, track_rvalue_moves, value_type,
};
use crate::frozen_plan::frozen_bytes_literal;
use crate::generators::generator_shape;
use crate::layout_types::native_alias_boundary;
use crate::list_values::sequence_read_storage;
use crate::lists::is_list;
use crate::numerics::numeric_carrier;
use crate::pending::untranslated;
use crate::result_carriers::{frozen_operand, typed_carrier_operand};
use crate::static_storage::snapshot_owns;
use crate::text_plan::{clone_text_reference, owned_slice_text};
use crate::unions::carrier_injection;

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
        (PlanType::ModelRef(owner, _), PlanType::Model(target_owner, _)) if owner == target_owner => {
            return Err(untranslated("model_construction::clone_model_reference"));
        }
        (PlanType::EnumRef(owner, _), PlanType::Enum(target_owner, _)) if owner == target_owner => {
            return Err(untranslated("model_construction::clone_enum_reference"));
        }
        _ => {}
    }
    if converted_carrier(builder, &source, target) {
        return Err(untranslated("carrier_operands::converted_carrier_operand"));
    }
    match target {
        PlanType::Closure(_) => return Err(untranslated("callable_calls::owned_callable_value")),
        PlanType::FunctionPointer(_) => return Err(untranslated("closures::function_assignment")),
        _ => {}
    }
    let mut operand = value;
    if source == PlanType::FrozenStr && frozen_text_union_member(builder, target)? {
        let text = display_plan_operand(builder, operand, span)?;
        operand = read_place(text, true, span);
    }
    operand = owned_union_member(builder, operand, target)?;
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
fn owned_union_member(builder: &Builder, value: Operand, target: &PlanType) -> Result<Operand, String> {
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
        PlanType::ModelRef(..) => Err(untranslated("model_construction::clone_model_reference")),
        PlanType::EnumRef(..) => Err(untranslated("model_construction::clone_enum_reference")),
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
