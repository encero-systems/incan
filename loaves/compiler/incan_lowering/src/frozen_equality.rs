//! Frozen equality borrows the actual static wrappers and delegates to their PartialEq contract.

use incan_mir_plan_native::{Operand, OperandKind, Place, PlanType, RvalueKind, SourceSpan, assign as plan_assign};
use incan_semantics_core::body_ir::{
    ArgumentElement, BinOp, Constant as SourceConstant, Operand as SourceOperand, Place as SourcePlace, PlaceElem,
};

use crate::calls::instantiated_external_call;
use crate::checked_values::scalar_place;
use crate::element_places::{element_place_type, is_element_place};
use crate::frame::{Builder, place_type, read_place, temporary};
use crate::list_values::value_operand;
use crate::static_storage::computed_static_root;

/// Keep a frozen owner or field; a bare baked aggregate gets its existing typed temporary.
pub fn frozen_place(builder: &mut Builder, value: &SourceOperand, span: &SourceSpan) -> Result<Option<Place>, String> {
    match value {
        SourceOperand::Place(read) => {
            // Determine the selected type before emitting an accessor: unrelated comparisons must not evaluate twice.
            if is_element_place(builder, &read.place)? {
                if !matches!(
                    element_place_type(builder, &read.place)?,
                    PlanType::FrozenCollection(..)
                ) {
                    return Ok(None);
                }
                let operand = value_operand(builder, value, span)?;
                return match operand.kind {
                    OperandKind::Copy(place) | OperandKind::Move(place) => Ok(Some(place)),
                    _ => Err("unsupported Body IR frozen element storage".into()),
                };
            }
            // Paths outside the checked local-element classifier keep their existing fallback without evaluating them.
            if read
                .place
                .projection
                .iter()
                .any(|projection| matches!(projection, PlaceElem::Index(_) | PlaceElem::Slice { .. }))
            {
                return Ok(None);
            }
            let place = frozen_storage_place(builder, &read.place, span)?;
            match place_type(&builder.locals, &place) {
                PlanType::FrozenCollection(..) => Ok(Some(place)),
                _ => Ok(None),
            }
        }
        SourceOperand::Constant(SourceConstant::FrozenAggregate(..)) => {
            let operand = value_operand(builder, value, span)?;
            match operand.kind {
                OperandKind::Copy(place) | OperandKind::Move(place) => Ok(Some(place)),
                _ => Err("unsupported Body IR frozen equality literal storage".into()),
            }
        }
        _ => Ok(None),
    }
}

/// Read a computed static's field through its snapshot; local places retain their checked projection.
fn frozen_storage_place(builder: &mut Builder, value: &SourcePlace, span: &SourceSpan) -> Result<Place, String> {
    match computed_static_root(builder, value, span)? {
        Some(snapshot) => Ok(snapshot),
        None => scalar_place(builder, value, span),
    }
}

/// Keep a shared view's complete wrapper name and element storage types.
pub fn frozen_reference_type(ty: &PlanType) -> Result<PlanType, String> {
    match ty {
        PlanType::FrozenCollection(name, elements) => Ok(PlanType::FrozenCollectionRef(name.clone(), elements.clone())),
        _ => Err("unsupported Body IR frozen reference carrier".into()),
    }
}

/// Borrow one checked frozen argument without inventing a value for a spread or missing carrier.
pub fn borrow_frozen_argument(
    builder: &mut Builder,
    argument: &ArgumentElement,
    span: &SourceSpan,
) -> Result<Operand, String> {
    let Some(value) = argument.as_one() else {
        return Err("unsupported Body IR spread frozen argument".into());
    };
    match frozen_place(builder, value, span)? {
        Some(storage) => borrow_frozen(builder, storage, span),
        None => Err("unsupported Body IR frozen argument storage".into()),
    }
}

/// Create a shared view of exact baked storage, including non-Copy model elements.
pub fn borrow_frozen(builder: &mut Builder, value: Place, span: &SourceSpan) -> Result<Operand, String> {
    let PlanType::FrozenCollection(name, elements) = place_type(&builder.locals, &value) else {
        return Err("unsupported Body IR frozen equality carrier".into());
    };
    let reference = temporary(builder, PlanType::FrozenCollectionRef(name, elements), span);
    builder.emit(plan_assign(reference.clone(), RvalueKind::Borrow(value), span));
    Ok(read_place(reference, false, span))
}

/// Compare identical checked wrappers through native PartialEq; other operations retain their named refusal.
pub fn compare_frozen(
    builder: &mut Builder,
    operator: &BinOp,
    left: &SourceOperand,
    right: &SourceOperand,
    span: &SourceSpan,
) -> Result<Option<Operand>, String> {
    if *operator != BinOp::Eq && *operator != BinOp::Ne {
        return Ok(None);
    }
    let first = frozen_place(builder, left, span)?;
    let second = frozen_place(builder, right, span)?;
    match (first, second) {
        (None, None) => Ok(None),
        (Some(first_place), Some(second_place)) => {
            let ty = place_type(&builder.locals, &first_place);
            if ty != place_type(&builder.locals, &second_place) {
                return Err("unsupported Body IR frozen equality across different layouts".into());
            }
            let references = vec![
                borrow_frozen(builder, first_place, span)?,
                borrow_frozen(builder, second_place, span)?,
            ];
            let path = if *operator == BinOp::Ne {
                "incan_native_runtime::unequal_model"
            } else {
                "incan_native_runtime::equal_model"
            };
            let output = temporary(builder, PlanType::Bool, span);
            let parameters = vec![frozen_reference_type(&ty)?, frozen_reference_type(&ty)?];
            instantiated_external_call(
                builder,
                path,
                vec![ty],
                parameters,
                PlanType::Bool,
                references,
                output.clone(),
                span,
            )?;
            Ok(Some(read_place(output, false, span)))
        }
        _ => Err("unsupported Body IR mixed frozen equality".into()),
    }
}
