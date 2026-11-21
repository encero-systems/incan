//! Typed carrier operands at call, return and collection boundaries, and retained reference values.

use incan_mir_plan_native::{
    Operand, Place, PlanType, RvalueKind, SourceSpan, assign as plan_assign, place as plan_place,
};
use incan_semantics_core::body_ir::{
    Constant as SourceConstant, Operand as SourceOperand, OwnershipFact, Place as SourcePlace,
};

use crate::checked_values::{scalar_place, target_read};
use crate::element_places::is_element_place;
use crate::explicit_arguments::reference_contract;
use crate::formatting::display_plan_operand;
use crate::frame::{Builder, dereferenced_place, place_type, read_place, temporary, value_type};
use crate::layout_types::shared_receiver_type;
use crate::list_values::value_operand;
use crate::lists::is_exclusive_borrow;
use crate::model_lists::structural_prefix;
use crate::pending::untranslated;

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
        return Err(untranslated("options::none_operand"));
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
        SourceOperand::Constant(_) => Err(untranslated("frozen_plan::frozen_literal")),
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
