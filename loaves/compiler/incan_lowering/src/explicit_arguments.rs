//! Explicit input access: returned views and the places explicit arguments lend.

use incan_mir_plan_native::PlanType;
use incan_semantics_core::IncanInputAccess;
use incan_semantics_core::body_ir::Body;

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
