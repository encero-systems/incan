//! Owned callable storage at generic value parameters.

use incan_mir_plan_native::PlanType;
use incan_semantics_core::IncanType;
use incan_semantics_core::body_ir::{ArgumentBinding, ArgumentElement, Body, BoundArgument, Operand};

use crate::frame::Builder;
use crate::pending::untranslated;

/// Select storage from retained generic parameters and evaluated closure facts; method bindings exclude their receiver
/// slot.
pub fn owned_callable_slots(
    builder: &Builder,
    original: &Body,
    arguments: &[ArgumentElement],
    binding: &ArgumentBinding,
    receiver_slots: usize,
) -> Result<Vec<i64>, String> {
    match binding {
        ArgumentBinding::Resolved {
            arguments: bindings, ..
        } => owned_slots_for_bindings(builder, original, arguments, bindings, receiver_slots),
        ArgumentBinding::ResolvedDeclarationOrder {
            arguments: bindings, ..
        } => {
            // A checked named free call keeps frozen declaration order; its records still name each argument's slot.
            for index in 1..bindings.len() {
                if bindings[index].slot <= bindings[index - 1].slot {
                    return Err("unsupported Body IR binding outside declaration order".into());
                }
            }
            owned_slots_for_bindings(builder, original, arguments, bindings, receiver_slots)
        }
        _ => Err("unsupported Body IR generic callable without resolved slot binding".into()),
    }
}

/// Collect the generic value slots whose bound argument is an owned closure, whatever order the call evaluated in.
fn owned_slots_for_bindings(
    builder: &Builder,
    original: &Body,
    arguments: &[ArgumentElement],
    bindings: &[BoundArgument],
    receiver_slots: usize,
) -> Result<Vec<i64>, String> {
    if receiver_slots > 1 || bindings.len() + receiver_slots != arguments.len() {
        return Err("invalid Body IR generic callable slot binding".into());
    }
    for (slot, parameter) in original.params.iter().enumerate() {
        if !matches!(parameter.ty, IncanType::TypeVar(_)) {
            continue;
        }
        for (index, bound) in bindings.iter().enumerate() {
            if bound.slot + receiver_slots != slot {
                continue;
            }
            match argument_storage(builder, &arguments[index + receiver_slots]) {
                Some(PlanType::Closure(_)) => return Err(untranslated("generic_callables::owned_slots_for_bindings")),
                Some(PlanType::ClosureRef(_)) => {
                    return Err("unsupported Body IR borrowed generic callable storage".into());
                }
                _ => {}
            }
        }
    }
    Ok(Vec::new())
}

/// Inspect a proven frame local without reading it, executing an index, or manufacturing a projected field ABI.
pub fn argument_storage(builder: &Builder, argument: &ArgumentElement) -> Option<PlanType> {
    let Some(Operand::Place(read)) = argument.as_one() else {
        return None;
    };
    if !read.place.projection.is_empty() || read.place.global().is_some() {
        return None;
    }
    let local = read.place.local_id()?;
    Some(
        builder.locals[(local.index() as i64 + builder.local_offset + 1) as usize]
            .ty
            .clone(),
    )
}
