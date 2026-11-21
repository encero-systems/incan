//! Typed carrier operands at call, return and collection boundaries, and retained reference values.

use incan_mir_plan_native::{
    Carrier, Operand, Place, PlanType, RvalueKind, SourceSpan, assign as plan_assign, carrier_position,
    place as plan_place,
};
use incan_semantics_core::body_ir::{
    ArgumentElement, Constant as SourceConstant, Operand as SourceOperand, OwnershipFact, Place as SourcePlace,
    ResultVariant, ResultVariantKind, TryErrorRouting,
};

use crate::assignments::assignment_place;
use crate::checked_values::{scalar_place, target_read};
use crate::element_places::is_element_place;
use crate::enums::enum_owner;
use crate::explicit_arguments::reference_contract;
use crate::formatting::display_plan_operand;
use crate::frame::{Builder, dereferenced_place, place_type, read_place, temporary, track_rvalue_moves, value_type};
use crate::frozen_plan::frozen_literal;
use crate::layout_types::shared_receiver_type;
use crate::list_values::value_operand;
use crate::lists::is_exclusive_borrow;
use crate::model_lists::structural_prefix;
use crate::options::{none_operand, propagate_result, replace_owned_destination};
use crate::statements::{carrier_payload, is_nominal_type};

/// Materialize checked carrier and text conversions at a concrete call, return, or collection boundary.
///
/// A checked native Copy read can dereference an explicit reference slot only for its matching owned carrier. Other
/// reference values retain the early reference-read boundary, and the adapter verifies the native Copy capability.
/// Indexed fields use the collection read path, which materializes their selected value exactly once.
pub fn typed_carrier_operand(
    builder: &mut Builder,
    operand: &SourceOperand,
    ty: &PlanType,
    span: &SourceSpan,
) -> Result<Operand, String> {
    if *ty == PlanType::FrozenStr {
        return frozen_operand(builder, operand, span);
    }
    if let Some(reference) = retained_reference_value(builder, operand, ty, span)? {
        return Ok(reference);
    }
    if let SourceOperand::Constant(SourceConstant::None) = operand {
        return none_operand(builder, ty, span);
    }
    if let SourceOperand::Place(read) = operand
        && read.fact == OwnershipFact::Copy
        && native_copy_type(builder, ty)
        && !is_element_place(builder, &read.place)?
    {
        let source = scalar_place(builder, &read.place, span)?;
        if place_type(&builder.locals, &source) == *ty {
            return target_read(builder, source, read.fact, span);
        }
    }
    let value = value_operand(builder, operand, span)?;
    if *ty == PlanType::String && value_type(builder, &value)? == PlanType::FrozenStr {
        let text = display_plan_operand(builder, value, span)?;
        return Ok(read_place(text, true, span));
    }
    Ok(value)
}

/// Keep only checked static literals or existing frozen values; dynamic text remains named-refused.
pub fn frozen_operand(builder: &mut Builder, value: &SourceOperand, span: &SourceSpan) -> Result<Operand, String> {
    match value {
        SourceOperand::Constant(constant) => frozen_literal(builder, constant, span),
        SourceOperand::Place(_) => {
            let operand = value_operand(builder, value, span)?;
            if value_type(builder, &operand)? != PlanType::FrozenStr {
                return Err("unsupported Body IR dynamic text to FrozenStr conversion".into());
            }
            Ok(operand)
        }
    }
}

/// Transfer or reborrow an exact exclusive reference, copy a shared reference, or borrow a checked local or structural
/// field.
pub fn retained_reference_value(
    builder: &mut Builder,
    value: &SourceOperand,
    expected: &PlanType,
    span: &SourceSpan,
) -> Result<Option<Operand>, String> {
    let exclusive = is_exclusive_borrow(expected);
    if !exclusive
        && !matches!(
            expected,
            PlanType::ModelRef(..)
                | PlanType::EnumRef(..)
                | PlanType::TupleRef(_)
                | PlanType::FrozenCollectionRef(..)
                | PlanType::StringRef
                | PlanType::ScalarRef(_)
                | PlanType::SliceRef(_)
        )
    {
        return Ok(None);
    }
    let SourceOperand::Place(read) = value else {
        return Ok(None);
    };
    let Some(place) = retained_reference_place(builder, &read.place, span)? else {
        return Ok(None);
    };
    let actual = place_type(&builder.locals, &place);
    if exclusive {
        if read.fact == OwnershipFact::Move && actual == *expected {
            return Ok(Some(read_place(place, true, span)));
        }
        if read.fact != OwnershipFact::MutBorrow {
            return Ok(None);
        }
        let (referent, writable) = reference_contract(expected)?;
        if !writable {
            return Err("unsupported Body IR stored exclusive view without writable reference storage".into());
        }
        let borrowed = if actual == *expected {
            dereferenced_place(&place, referent, span)?
        } else if actual != referent {
            return Ok(None);
        } else {
            place
        };
        let reference = temporary(builder, expected.clone(), span);
        builder.emit(plan_assign(reference.clone(), RvalueKind::MutBorrow(borrowed), span));
        return Ok(Some(read_place(reference, true, span)));
    }
    if read.fact == OwnershipFact::Unknown || read.fact == OwnershipFact::MutBorrow {
        return Err("unsupported Body IR shared-reference value ownership".into());
    }
    if actual != *expected {
        if read.fact != OwnershipFact::Borrow {
            return Ok(None);
        }
        match shared_receiver_type(actual) {
            Ok(reference_type) => {
                if reference_type != *expected {
                    return Ok(None);
                }
            }
            Err(_) => return Ok(None),
        }
        let reference = temporary(builder, expected.clone(), span);
        builder.emit(plan_assign(reference.clone(), RvalueKind::Borrow(place), span));
        return Ok(Some(read_place(reference, false, span)));
    }
    Ok(Some(read_place(place, read.fact == OwnershipFact::Move, span)))
}

/// Keep local and structural field storage addressable; collection indexing retains its own borrow path.
fn retained_reference_place(
    builder: &mut Builder,
    source: &SourcePlace,
    span: &SourceSpan,
) -> Result<Option<Place>, String> {
    let Some(local) = source.local_id() else {
        return Ok(None);
    };
    if source.projection.is_empty() {
        return Ok(Some(plan_place(local.index() as i64 + 1 + builder.local_offset, span)));
    }
    match structural_prefix(builder, source)? {
        Some((count, _)) => {
            if count != source.projection.len() {
                return Ok(None);
            }
        }
        None => return Ok(None),
    }
    Ok(Some(scalar_place(builder, source, span)?))
}

/// Require retained native Copy capability for the exact owned carrier requested by a typed boundary.
fn native_copy_type(builder: &Builder, ty: &PlanType) -> bool {
    match ty {
        PlanType::Model(owner, _) => match builder.nominals().get(*owner as usize) {
            Some(declaration) => {
                *owner >= 0
                    && !declaration.native_item.is_empty()
                    && declaration.derives.iter().any(|derive| derive == "Copy")
            }
            None => false,
        },
        PlanType::Enum(owner, _) => match builder.enums().get(*owner as usize) {
            Some(declaration) => {
                *owner >= 0
                    && !declaration.native_item.is_empty()
                    && declaration.derives.iter().any(|derive| derive == "Copy")
            }
            None => false,
        },
        _ => false,
    }
}

/// Materialize the checked Result read before consuming its selected payload on either propagation edge. The only
/// error conversion the propagation performs is the owned text a `const` str error becomes; it decides that from the
/// concrete carriers.
pub fn lower_try_propagation(
    builder: &mut Builder,
    destination: &SourcePlace,
    operand: &SourceOperand,
    routing: &TryErrorRouting,
    span: &SourceSpan,
) -> Result<(), String> {
    if !matches!(
        routing,
        TryErrorRouting::SameType { .. } | TryErrorRouting::ConversionRequired { .. }
    ) {
        return Err("unsupported Body IR unresolved TryPropagate error routing".into());
    }
    let value = value_operand(builder, operand, span)?;
    let ty = value_type(builder, &value)?;
    let source = temporary(builder, ty, span);
    track_rvalue_moves(builder, &RvalueKind::Use(value.clone()));
    builder.emit(plan_assign(source.clone(), RvalueKind::Use(value), span));
    builder.owned.push(source.local);
    let target = assignment_place(builder, destination, span)?;
    propagate_result(builder, source, target, span)
}

/// Construct the checker-selected Result variant in its concrete standard carrier.
pub fn lower_result_construction(
    builder: &mut Builder,
    target: Place,
    variant: &ResultVariant,
    span: &SourceSpan,
) -> Result<(), String> {
    let owner = enum_owner(&place_type(&builder.locals, &target))?;
    if owner < builder.enums().len() as i64 {
        return Err("invalid Body IR Result destination".into());
    }
    let declaration = builder.intrinsics[carrier_position(owner) as usize].clone();
    if declaration.carrier != Carrier::Result || !variant.has_complete_canonical_types() {
        return Err("invalid Body IR Result payload identity".into());
    }
    let slot = if variant.kind == ResultVariantKind::Err { 1 } else { 0 };
    let operand = carrier_payload(builder, &variant.payload, &declaration.variants[slot].fields[0], span)?;
    replace_owned_destination(
        builder,
        target,
        RvalueKind::Enum(owner, slot as i64, vec![operand]),
        span,
    );
    Ok(())
}

/// Build only an explicitly retained intrinsic Some node in the checked Option destination.
pub fn lower_option_construction(
    builder: &mut Builder,
    target: Place,
    elements: &[ArgumentElement],
    span: &SourceSpan,
) -> Result<(), String> {
    let owner = enum_owner(&place_type(&builder.locals, &target))?;
    if owner < builder.enums().len() as i64
        || builder.intrinsics[carrier_position(owner) as usize].carrier != Carrier::Option
        || elements.len() != 1
    {
        return Err("invalid Body IR Some destination or payload".into());
    }
    let Some(payload) = elements[0].as_one() else {
        return Err("unsupported Body IR Some spread payload".into());
    };
    let ty = builder.intrinsics[carrier_position(owner) as usize].variants[1].fields[0].clone();
    let value = carrier_payload(builder, payload, &ty, span)?;
    replace_owned_destination(builder, target, RvalueKind::Enum(owner, 1, vec![value]), span);
    Ok(())
}

/// Assign the typed None value through the standard carrier ownership path.
pub fn lower_none_assignment(builder: &mut Builder, target: Place, span: &SourceSpan) -> Result<(), String> {
    let ty = place_type(&builder.locals, &target);
    let value = none_operand(builder, &ty, span)?;
    if is_nominal_type(&ty) {
        replace_owned_destination(builder, target, RvalueKind::Use(value), span);
    } else {
        track_rvalue_moves(builder, &RvalueKind::Use(value.clone()));
        builder.emit(plan_assign(target, RvalueKind::Use(value), span));
    }
    Ok(())
}
