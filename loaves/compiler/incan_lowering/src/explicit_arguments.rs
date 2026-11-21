//! Explicit input access: returned views and the places explicit arguments lend.

use incan_mir_plan_native::PlanType;
use incan_semantics_core::IncanInputAccess;
use incan_semantics_core::body_ir::{Body, BodyIrModule, CallableParam};

use crate::frame::Builder;

/// The access of a body's direct returned view; a carried view returns an owned carrier and has none.
pub fn direct_view_access(body: &Body) -> Option<IncanInputAccess> {
    let view = body.returned_view.as_ref()?;
    if view.access != IncanInputAccess::Owned && view.component.is_empty() {
        return Some(view.access);
    }
    None
}

/// Recover the exact referent and write authority from supported native reference carriers.
pub fn reference_contract(ty: &PlanType) -> Result<(PlanType, bool), String> {
    let contract = match ty {
        PlanType::StringRef => (PlanType::String, false),
        PlanType::StringMutRef => (PlanType::String, true),
        PlanType::ModelRef(owner, name) => (PlanType::Model(*owner, name.clone()), false),
        PlanType::ModelMutRef(owner, name) => (PlanType::Model(*owner, name.clone()), true),
        PlanType::EnumRef(owner, name) => (PlanType::Enum(*owner, name.clone()), false),
        PlanType::EnumMutRef(owner, name) => (PlanType::Enum(*owner, name.clone()), true),
        PlanType::TupleRef(elements) => (PlanType::Tuple(elements.clone()), false),
        PlanType::TupleMutRef(elements) => (PlanType::Tuple(elements.clone()), true),
        PlanType::ListRef(leaf, depth) => (PlanType::List(leaf.clone(), *depth), false),
        PlanType::ListMutRef(leaf, depth) => (PlanType::List(leaf.clone(), *depth), true),
        PlanType::SetRef(leaf) => (PlanType::Set(leaf.clone()), false),
        PlanType::SetMutRef(leaf) => (PlanType::Set(leaf.clone()), true),
        PlanType::DictRef(key, value) => (PlanType::Dict(key.clone(), value.clone()), false),
        PlanType::DictMutRef(key, value) => (PlanType::Dict(key.clone(), value.clone()), true),
        PlanType::ScalarRef(elements) if elements.len() == 1 => (elements[0].clone(), false),
        PlanType::ScalarMutRef(elements) if elements.len() == 1 => (elements[0].clone(), true),
        PlanType::FrozenCollectionRef(name, elements) => {
            (PlanType::FrozenCollection(name.clone(), elements.clone()), false)
        }
        PlanType::NativeArrayRef(elements, length) => (PlanType::NativeArray(elements.clone(), *length), false),
        PlanType::NativeArrayMutRef(elements, length) => (PlanType::NativeArray(elements.clone(), *length), true),
        PlanType::SliceRef(elements) => (PlanType::NativeSlice(elements.clone()), false),
        PlanType::SliceMutRef(elements) => (PlanType::NativeSlice(elements.clone()), true),
        PlanType::ValidationErrorRef => (PlanType::ValidationError, false),
        _ => return Err(format!("unsupported Body IR explicit reference carrier {ty:?}")),
    };
    Ok(contract)
}

/// Keep declaration-order access slots aligned when a method's separately lowered receiver is omitted.
pub fn parameter_accesses(parameters: &[CallableParam], offset: usize) -> Result<Vec<IncanInputAccess>, String> {
    if offset > parameters.len() {
        return Err("invalid Body IR input access parameter offset".into());
    }
    Ok(parameters[offset..]
        .iter()
        .map(|parameter| parameter.input_access)
        .collect())
}

/// An absent source contract denotes the ordinary/native route; a supplied contract must cover every slot.
pub fn argument_access(accesses: &[IncanInputAccess], count: usize, slot: usize) -> Result<IncanInputAccess, String> {
    if slot >= count || (!accesses.is_empty() && accesses.len() != count) {
        return Err("invalid Body IR argument input access slots".into());
    }
    Ok(accesses.get(slot).copied().unwrap_or(IncanInputAccess::Inferred))
}

/// Authenticate source-authored input modes; compiler-inferred shared access needs no source permission.
pub fn validate_source_input_call(module: &BodyIrModule, builder: &Builder, callee: &Body) -> Result<(), String> {
    // A returned view obliges its caller to alias the substituted place under a checked profile.
    let explicit = callee.returned_view.is_some()
        || callee.params.iter().any(|parameter| {
            matches!(
                parameter.input_access,
                IncanInputAccess::Shared | IncanInputAccess::Exclusive | IncanInputAccess::Owned
            )
        });
    if !explicit {
        return Ok(());
    }
    let Some(identity) = &builder.callable_identity else {
        return Err("unsupported Body IR explicit source call without a declared caller".into());
    };
    let Some(index) = builder.program.body_indices.unique(identity) else {
        return Err("unsupported Body IR explicit source call without a unique checked caller".into());
    };
    let caller = &module.bodies[index];
    if caller.canonical.as_ref() != Some(identity) {
        return Err("unsupported Body IR explicit source caller identity mismatch".into());
    }
    match caller.input_call_profile_gap(callee) {
        Some(reason) => Err(format!("unsupported Body IR {reason}")),
        None => Ok(()),
    }
}
