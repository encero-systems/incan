//! Operands lowered for their effects or as borrows: discarded values, text views and shared slices.

use incan_mir_plan_native::{Operand, PlanType, RvalueKind, SourceSpan, assign as plan_assign};
use incan_semantics_core::body_ir::{Constant as SourceConstant, Operand as SourceOperand, OwnershipFact};

use crate::checked_values::scalar_place;
use crate::element_places::is_element_place;
use crate::formatting::{display_operand, display_plan_operand};
use crate::frame::{Builder, drop_owned, external_call, place_type, read_place, temporary, text_literal};
use crate::list_values::value_operand;
use crate::pending::untranslated;
use crate::text_plan::{owned_literal, owned_slice_text};

/// Evaluate discarded text through retained ownership facts and drop its materialized owner.
pub fn lower_discarded_operand(builder: &mut Builder, value: &SourceOperand, span: &SourceSpan) -> Result<(), String> {
    let is_text = match value {
        // A discarded string constant, such as a docstring, has no effect: materializing it would allocate and free
        // its text on every call.
        SourceOperand::Constant(SourceConstant::Str(_)) => return Ok(()),
        SourceOperand::Constant(_) => false,
        SourceOperand::Place(read) => {
            let place = scalar_place(builder, &read.place, span)?;
            place_type(&builder.locals, &place) == PlanType::String
        }
    };
    if is_text {
        let text = display_operand(builder, value, span)?;
        drop_owned(builder, text.local, span);
    } else {
        value_operand(builder, value, span)?;
    }
    Ok(())
}

/// Pass literal slice text directly or borrow owned text through String's own as_str method.
pub fn borrow_str_operand(builder: &mut Builder, value: &SourceOperand, span: &SourceSpan) -> Result<Operand, String> {
    match value {
        SourceOperand::Constant(SourceConstant::Str(text)) => return Ok(text_literal(text, span)),
        SourceOperand::Place(read) => {
            let place = scalar_place(builder, &read.place, span)?;
            if place_type(&builder.locals, &place) == PlanType::StrRef {
                return Ok(read_place(place, false, span));
            }
        }
        _ => {}
    }
    let reference = borrow_text_operand(builder, value, span)?;
    let output = temporary(builder, PlanType::StrRef, span);
    external_call(
        builder,
        "alloc::string::String::as_str",
        vec![PlanType::StringRef],
        PlanType::StrRef,
        vec![reference],
        output.clone(),
        span,
    );
    Ok(read_place(output, false, span))
}

/// Borrow owned text or reuse a checked shared String reference without transferring its owner or adding a reference
/// layer.
pub fn borrow_text_operand(builder: &mut Builder, value: &SourceOperand, span: &SourceSpan) -> Result<Operand, String> {
    let read = match value {
        SourceOperand::Place(read) => read,
        SourceOperand::Constant(constant) => {
            let text = match constant {
                SourceConstant::Str(text) | SourceConstant::FrozenStr(text) => text,
                _ => return Err("unsupported Body IR non-string text constant".into()),
            };
            let owner = owned_literal(builder, text, span);
            return Ok(borrow_string(builder, owner, span));
        }
    };
    if read.place.global().is_some() {
        return Err(untranslated("static_storage::read_static"));
    }
    if is_element_place(builder, &read.place)? {
        return Err(untranslated("operand_borrows::borrow_element_text"));
    }
    let place = scalar_place(builder, &read.place, span)?;
    match read.fact {
        OwnershipFact::MutBorrow => return Err("unsupported Body IR mutable String borrow".into()),
        OwnershipFact::Unknown => return Err("unsupported Body IR unknown String ownership".into()),
        _ => {}
    }
    match place_type(&builder.locals, &place) {
        PlanType::StringRef => Ok(read_place(place, false, span)),
        PlanType::StrRef => {
            let owner = owned_slice_text(builder, read_place(place, false, span), span);
            builder.owned.push(owner.local);
            Ok(borrow_string(builder, owner, span))
        }
        PlanType::FrozenStr => {
            let owner = display_plan_operand(builder, read_place(place, false, span), span)?;
            Ok(borrow_string(builder, owner, span))
        }
        PlanType::String => Ok(borrow_string(builder, place, span)),
        ty => {
            let site = format!("{}:{}:{} in {}", builder.file, span.line, span.column, builder.name);
            Err(format!(
                "unsupported Body IR non-string text operand at {site}; type {ty:?}"
            ))
        }
    }
}

/// A shared reference to one owned string place.
fn borrow_string(builder: &mut Builder, owner: incan_mir_plan_native::Place, span: &SourceSpan) -> Operand {
    let reference = temporary(builder, PlanType::StringRef, span);
    builder.emit(plan_assign(reference.clone(), RvalueKind::Borrow(owner), span));
    read_place(reference, false, span)
}
