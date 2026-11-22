//! Preserve checked callback environments in their exact native generic argument positions.

use incan_mir_plan_native::PlanType;
use incan_semantics_core::body_ir::{ArgumentBinding, ArgumentElement, BoundArgument, LoafCallable};

use crate::frame::Builder;
use crate::generic_callables::argument_storage;

/// Keep the environment representation chosen by checking at each native callback boundary.
///
/// 1. Read each callback's already evaluated storage from its checked slot.
/// 2. Replace only the generic position retained for that parameter, preserving the checked signature.
pub fn native_callable_storage(
    builder: &Builder,
    declaration: &LoafCallable,
    arguments: &[ArgumentElement],
    binding: &ArgumentBinding,
    parameters: Vec<PlanType>,
    type_arguments: Vec<PlanType>,
) -> Result<(Vec<PlanType>, Vec<PlanType>), String> {
    let mut inputs = parameters;
    let mut generics = type_arguments;
    for positions in &declaration.native_callable_parameters {
        let slot = positions.parameter_slot;
        let generic = positions.type_argument_slot;
        if slot >= inputs.len() || generic >= generics.len() {
            return Err("invalid Body IR native callback generic position".into());
        }
        // 1. Read the callback's already evaluated storage from its checked slot.
        let Some(held) = callback_storage(builder, arguments, binding, slot)? else {
            continue;
        };
        if !owns_callback(&held) {
            continue;
        }
        // 2. Refine only the retained generic position; no callee or bound is inferred here.
        let expected = pointer_signature(&held)?;
        if inputs[slot] != expected || (generics[generic] != expected && generics[generic] != held) {
            return Err("invalid Body IR native callback signature or mixed storage".into());
        }
        inputs[slot] = held.clone();
        generics[generic] = held;
    }
    Ok((inputs, generics))
}

/// Select owned environments, leaving copied function items and function pointers to ordinary coercion.
fn owns_callback(ty: &PlanType) -> bool {
    matches!(ty, PlanType::Closure(_) | PlanType::NativeClosure(..))
}

/// Read the unchanged call signature from a checked owned environment.
fn pointer_signature(ty: &PlanType) -> Result<PlanType, String> {
    match ty {
        PlanType::Closure(signature) | PlanType::NativeClosure(signature, ..) => {
            Ok(PlanType::FunctionPointer(signature.clone()))
        }
        _ => Err("invalid Body IR native callback without owned storage".into()),
    }
}

/// Use the checked slot map rather than source argument order, including named calls.
fn callback_storage(
    builder: &Builder,
    arguments: &[ArgumentElement],
    binding: &ArgumentBinding,
    slot: usize,
) -> Result<Option<PlanType>, String> {
    match binding {
        ArgumentBinding::Resolved { arguments: bound, .. }
        | ArgumentBinding::ResolvedDeclarationOrder { arguments: bound, .. } => {
            bound_callback_storage(builder, arguments, bound, slot)
        }
        ArgumentBinding::UnresolvedPositional => match arguments.get(slot) {
            Some(argument) => Ok(argument_storage(builder, argument)),
            None => Err("invalid Body IR native callback argument position".into()),
        },
    }
}

/// Locate the materialized value belonging to one declaration slot without evaluating it again.
fn bound_callback_storage(
    builder: &Builder,
    arguments: &[ArgumentElement],
    bound: &[BoundArgument],
    slot: usize,
) -> Result<Option<PlanType>, String> {
    let Some(index) = bound.iter().position(|argument| argument.slot == slot) else {
        return Err("invalid Body IR missing native callback argument".into());
    };
    match arguments.get(index) {
        Some(argument) => Ok(argument_storage(builder, argument)),
        None => Err("invalid Body IR native callback binding position".into()),
    }
}
