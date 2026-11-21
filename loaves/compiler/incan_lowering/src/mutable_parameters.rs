//! The exclusive references `mut` text, model, enum and tuple parameters receive, built the way legacy passes each
//! argument.

use incan_mir_plan_native::{
    Operand, OperandKind, Place, PlanType, Projection, RvalueKind, SourceSpan, assign as plan_assign,
};
use incan_semantics_core::body_ir::{Operand as SourceOperand, OwnershipFact};

use crate::checked_values::{clone_nominal_operand, scalar_place};
use crate::formatting::display_operand;
use crate::frame::{Builder, place_type, read_place, temporary};
use crate::lists::parameter_storage_type;
use crate::options::clone_tuple_operand;
use crate::text_plan::clone_text;
use crate::tuples::is_tuple_type;

/// Borrow the value a `mut` text, model, enum or tuple parameter receives, as an exclusive reference of type
/// `reference`.
///
/// The caller's own value, or one it last uses, is borrowed in place, so the callee's changes reach it. A text literal,
/// a loop variable, or a value the checker reads by copy reaches the callee as a fresh owned copy, as legacy passes
/// `&mut value.clone()`; the copy drops when its scope ends.
pub fn mutable_argument(
    builder: &mut Builder,
    value: &SourceOperand,
    reference: &PlanType,
    span: &SourceSpan,
) -> Result<Operand, String> {
    let source = argument_source(builder, value, &exclusive_referent(reference)?, span)?;
    let exclusive = temporary(builder, reference.clone(), span);
    builder.emit(plan_assign(exclusive.clone(), RvalueKind::MutBorrow(source), span));
    Ok(read_place(exclusive, true, span))
}

/// The value type an exclusive parameter reference names.
pub fn exclusive_referent(reference: &PlanType) -> Result<PlanType, String> {
    match reference {
        PlanType::StringMutRef => Ok(PlanType::String),
        PlanType::ModelMutRef(owner, name) => Ok(PlanType::Model(*owner, name.clone())),
        PlanType::EnumMutRef(owner, name) => Ok(PlanType::Enum(*owner, name.clone())),
        PlanType::TupleMutRef(elements) => Ok(PlanType::Tuple(elements.clone())),
        _ => Err("invalid Body IR mutable parameter reference".into()),
    }
}

/// Select the value to borrow: the argument itself when the caller hands it over, otherwise an owned copy.
fn argument_source(
    builder: &mut Builder,
    value: &SourceOperand,
    ty: &PlanType,
    span: &SourceSpan,
) -> Result<Place, String> {
    match value {
        SourceOperand::Place(read) => {
            if read.place.global().is_some() {
                return Err("unsupported Body IR mutable argument from a static".into());
            }
            let place = scalar_place(builder, &read.place, span)?;
            // A `mut` parameter handed on to another one is reborrowed, so the changes still reach its own caller.
            let exclusive = parameter_storage_type(ty, true);
            if place.projection == Projection::Whole && builder.locals[place.local as usize].ty == exclusive {
                let handed_on = Place {
                    local: place.local,
                    projection: Projection::Deref(ty.clone()),
                    span: span.clone(),
                };
                return checked_source(builder, handed_on, read.fact, span);
            }
            if place_type(&builder.locals, &place) != *ty {
                return Err("unsupported Body IR mutable argument of another type".into());
            }
            // A loop variable is a copy of its element or a view of it; legacy hands the callee a further copy.
            if builder.borrowed_items.contains(&place.local) || builder.in_place_items.contains(&place.local) {
                return copied_value(builder, place, span);
            }
            checked_source(builder, place, read.fact, span)
        }
        SourceOperand::Constant(_) => {
            if *ty != PlanType::String {
                return Err("unsupported Body IR mutable argument constant".into());
            }
            display_operand(builder, value, span)
        }
    }
}

/// Borrow a value the checker hands over exclusively in place, and copy one it only reads.
fn checked_source(
    builder: &mut Builder,
    place: Place,
    fact: OwnershipFact,
    span: &SourceSpan,
) -> Result<Place, String> {
    if let PlanType::ModelRef(..) = builder.locals[place.local as usize].ty {
        let site = format!(
            "{} frame local {} at {}:{}",
            builder.name, place.local, span.file, span.line
        );
        return Err(format!(
            "unsupported Body IR mutable argument through a shared receiver in {site} ({fact:?})"
        ));
    }
    match fact {
        OwnershipFact::MutBorrow | OwnershipFact::Move => Ok(place),
        OwnershipFact::Clone | OwnershipFact::Borrow => copied_value(builder, place, span),
        _ => Err("unsupported Body IR mutable argument ownership".into()),
    }
}

/// Clone text, a tuple, a model or an enum into a new owned temporary.
fn copied_value(builder: &mut Builder, place: Place, span: &SourceSpan) -> Result<Place, String> {
    let ty = place_type(&builder.locals, &place);
    if ty == PlanType::String {
        return clone_text(builder, place, span);
    }
    let copy = if is_tuple_type(&ty) {
        clone_tuple_operand(builder, place, span)?
    } else {
        clone_nominal_operand(builder, place, span)?
    };
    match copy.kind {
        OperandKind::Move(copy) => Ok(copy),
        _ => Err(if is_tuple_type(&ty) {
            "invalid Body IR cloned tuple argument".into()
        } else {
            "invalid Body IR cloned argument".into()
        }),
    }
}
