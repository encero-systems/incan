//! Retain owned callable storage at an SDK method's checked generic parameter boundary.

use incan_mir_plan_native::PlanType;
use incan_semantics_core::IncanType;
use incan_semantics_core::body_ir::{ArgumentBinding, ArgumentElement, LoafCallable, Operand as SourceOperand};

use crate::frame::Builder;
use crate::generic_callables::argument_storage;
use crate::layout_types::native_source_type;
use crate::type_parameters::substitute;

/// An SDK generic value binder's name and the evaluated environment storage its arguments keep.
pub type OwnedBinder = (String, PlanType);

/// The cloneable SDK callable a fallible iterator's error mapper must receive.
const CLONEABLE_CALLABLE_PATH: &str = "incan_stdlib_primitives::derives::collection::FallibleIterator::map_err";

/// Keep an SDK generic value binder's evaluated environment ABI, checking its declared callable signature first.
pub fn sdk_owned_callable_bindings(
    builder: &Builder,
    delegation: &LoafCallable,
    binding: &ArgumentBinding,
    arguments: &[ArgumentElement],
    types: &[PlanType],
) -> Result<Vec<OwnedBinder>, String> {
    let ArgumentBinding::Resolved {
        arguments: bindings,
        defaulted_slots,
    } = binding
    else {
        return Ok(Vec::new());
    };
    if !defaulted_slots.is_empty() {
        return Ok(Vec::new());
    }
    if bindings.len() != types.len() || arguments.len() != types.len() + 1 {
        return Err("standard library generic callable parameter binding is unsupported".into());
    }
    let mut owned: Vec<OwnedBinder> = Vec::new();
    let mut slots: Vec<usize> = Vec::new();
    for (index, bound) in bindings.iter().enumerate() {
        let slot = bound.slot;
        if slot >= types.len() {
            return Err("the standard library generic callable parameter is invalid".into());
        }
        let Some(IncanType::TypeVar(name)) = delegation.parameters.get(slot + 1) else {
            continue;
        };
        let argument = &arguments[index + 1];
        match argument_storage(builder, argument) {
            Some(PlanType::Closure(signature)) => {
                if types[slot] != PlanType::FunctionPointer(signature.clone()) {
                    return Err("the standard library generic callable signatures differ".into());
                }
                require_plain_sdk_callable(builder, argument)?;
                let storage = if delegation.rust_path().as_deref() == Some(CLONEABLE_CALLABLE_PATH) {
                    cloneable_storage(builder, argument, signature)?
                } else {
                    PlanType::Closure(signature)
                };
                owned.push((name.to_string(), storage));
                slots.push(slot);
            }
            Some(PlanType::ClosureRef(_)) => {
                return Err("borrowed standard library generic callable storage is unsupported".into());
            }
            _ => {}
        }
    }
    for slot in &slots {
        for other in 0..types.len() {
            if delegation.parameters[other + 1] == delegation.parameters[slot + 1] && !slots.contains(&other) {
                return Err("mixed standard library generic callable storage is unsupported".into());
            }
        }
    }
    Ok(owned)
}

/// The shared callable an error mapper keeps: only a proven scalar-capture closure local may be shared.
fn cloneable_storage(
    builder: &Builder,
    argument: &ArgumentElement,
    signature: Vec<PlanType>,
) -> Result<PlanType, String> {
    if signature.len() != 2 {
        return Err("the standard library cloneable callable argument count is unsupported".into());
    }
    let Some(SourceOperand::Place(read)) = argument.as_one() else {
        return Err("the standard library cloneable callable origin is unsupported".into());
    };
    let Some(local) = read.place.local_id() else {
        return Err("the standard library cloneable callable origin is unsupported".into());
    };
    let held = local.index() as i64 + builder.local_offset + 1;
    if !builder.scalar_closures.contains(&held) || builder.unproven_closures.contains(&held) {
        return Err("the standard library cloneable callable capture shape is unsupported".into());
    }
    Ok(PlanType::SharedCallable(signature))
}

/// Refuse a partial callable whose extra supplied flags are absent from the SDK's checked input signature.
fn require_plain_sdk_callable(builder: &Builder, argument: &ArgumentElement) -> Result<(), String> {
    if let Some(SourceOperand::Place(read)) = argument.as_one()
        && let Some(local) = read.place.local_id()
    {
        let index = local.index() as i64 + builder.local_offset + 1;
        if builder
            .partial_callables
            .iter()
            .any(|entry| entry.0 == index && entry.1.contains(&true))
        {
            return Err("standard library generic callables with default parameters are unsupported".into());
        }
    }
    Ok(())
}

/// Use one declaration-owned generic value ABI at both its ordinary parameter and native type-argument slots.
pub fn sdk_callable_type(
    builder: &Builder,
    original: &IncanType,
    names: &[String],
    closed: &[IncanType],
    owned: &[OwnedBinder],
) -> Result<PlanType, String> {
    if let IncanType::TypeVar(name) = original
        && let Some((_, storage)) = owned.iter().find(|(binder, _)| binder == name)
    {
        return Ok(storage.clone());
    }
    let concrete = substitute(original, names, closed)?;
    native_type(builder, &concrete)
}

/// Retain explicit native argument order, or close declaration binders without constructing substitute Rust types.
pub fn sdk_callable_type_arguments(
    builder: &Builder,
    delegation: &LoafCallable,
    names: &[String],
    closed: &[IncanType],
    owned: &[OwnedBinder],
) -> Result<Vec<PlanType>, String> {
    if !delegation.native_type_arguments.is_empty() {
        let mut result = Vec::with_capacity(delegation.native_type_arguments.len());
        for argument in &delegation.native_type_arguments {
            result.push(sdk_callable_type(builder, argument, names, closed, owned)?);
        }
        return Ok(result);
    }
    let mut result = Vec::with_capacity(closed.len());
    for (index, argument) in closed.iter().enumerate() {
        let mut ty = native_type(builder, argument)?;
        for (binder, storage) in owned {
            if names.get(index) == Some(binder) {
                ty = storage.clone();
            }
        }
        result.push(ty);
    }
    Ok(result)
}

/// The native carrier of a closed SDK type.
fn native_type(builder: &Builder, ty: &IncanType) -> Result<PlanType, String> {
    native_source_type(
        builder.constructors(),
        builder.nominals(),
        builder.enums(),
        &builder.unions,
        ty,
        &builder.intrinsics,
        "",
    )
}
