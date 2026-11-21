//! Owned callable storage at generic value parameters.

use incan_mir_plan_native::PlanType;
use incan_semantics_core::IncanType;
use incan_semantics_core::body_ir::{ArgumentBinding, ArgumentElement, Body, BoundArgument, Operand};

use crate::frame::Builder;
use crate::layout_types::native_source_type;

/// Select storage from retained generic parameters and evaluated closure facts; method bindings exclude their receiver
/// slot.
pub fn owned_callable_slots(
    builder: &Builder,
    original: &Body,
    closed: &Body,
    arguments: &[ArgumentElement],
    binding: &ArgumentBinding,
    receiver_slots: usize,
) -> Result<Vec<i64>, String> {
    match binding {
        ArgumentBinding::Resolved {
            arguments: bindings, ..
        } => owned_slots_for_bindings(builder, (original, closed), arguments, bindings, receiver_slots),
        ArgumentBinding::ResolvedDeclarationOrder {
            arguments: bindings, ..
        } => {
            // A checked named free call keeps frozen declaration order; its records still name each argument's slot.
            for index in 1..bindings.len() {
                if bindings[index].slot <= bindings[index - 1].slot {
                    return Err("unsupported Body IR binding outside declaration order".into());
                }
            }
            owned_slots_for_bindings(builder, (original, closed), arguments, bindings, receiver_slots)
        }
        _ => Err("unsupported Body IR generic callable without resolved slot binding".into()),
    }
}

/// Collect the generic value slots whose bound argument is an owned closure, whatever order the call evaluated in.
fn owned_slots_for_bindings(
    builder: &Builder,
    (original, closed): (&Body, &Body),
    arguments: &[ArgumentElement],
    bindings: &[BoundArgument],
    receiver_slots: usize,
) -> Result<Vec<i64>, String> {
    if receiver_slots > 1 || bindings.len() + receiver_slots != arguments.len() {
        return Err("invalid Body IR generic callable slot binding".into());
    }
    let mut owned = Vec::new();
    for (slot, parameter) in original.params.iter().enumerate() {
        if !matches!(parameter.ty, IncanType::TypeVar(_)) {
            continue;
        }
        for (index, bound) in bindings.iter().enumerate() {
            if bound.slot + receiver_slots != slot {
                continue;
            }
            match argument_storage(builder, &arguments[index + receiver_slots]) {
                Some(PlanType::Closure(signature)) => {
                    if parameter.mutable {
                        return Err("unsupported Body IR mutable generic callable storage".into());
                    }
                    let expected = native_source_type(
                        builder.constructors(),
                        builder.nominals(),
                        builder.enums(),
                        &builder.unions,
                        &closed.params[slot].ty,
                        &builder.intrinsics,
                        "",
                    )?;
                    if expected != PlanType::FunctionPointer(signature.clone())
                        && expected != PlanType::Closure(signature)
                    {
                        return Err("unsupported Body IR generic callable signature mismatch".into());
                    }
                    owned.push(slot as i64);
                }
                Some(PlanType::ClosureRef(_)) => {
                    return Err("unsupported Body IR borrowed generic callable storage".into());
                }
                _ => {}
            }
        }
    }
    validate_uniform_storage(original, &owned)?;
    Ok(owned)
}

/// Refuse one generic type variable used at incompatible callable ABIs or an unevaluated default.
fn validate_uniform_storage(original: &Body, owned: &[i64]) -> Result<(), String> {
    for owner in owned {
        let owner_type = &original.params[*owner as usize].ty;
        for (slot, parameter) in original.params.iter().enumerate() {
            if parameter.ty == *owner_type && !owned.contains(&(slot as i64)) {
                return Err("unsupported Body IR mixed generic callable storage or default".into());
            }
        }
    }
    Ok(())
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
