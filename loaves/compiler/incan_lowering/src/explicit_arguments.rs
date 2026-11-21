//! Explicit input access: returned views and the places explicit arguments lend.

use incan_mir_plan_native::{
    Local, Operand, Place, PlanType, Projection, RvalueKind, SourceSpan, assign as plan_assign,
};
use incan_semantics_core::IncanInputAccess;
use incan_semantics_core::body_ir::{Body, BodyIrModule, CallableParam, OwnershipFact};

use crate::frame::{Builder, dereferenced_place, place_type, read_place, temporary};
use crate::numerics::numeric_carrier;

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

/// Emit a checked move or a fresh loan of the exact selected storage; never call a cloning/materialization helper.
pub fn explicit_place_argument(
    builder: &mut Builder,
    source: Place,
    fact: OwnershipFact,
    expected: &PlanType,
    access: IncanInputAccess,
    span: &SourceSpan,
) -> Result<Operand, String> {
    let operation = explicit_input_operation(&builder.locals, source, fact, expected, access, span)?;
    if let RvalueKind::Use(operand) = operation {
        return Ok(operand);
    }
    let reference = temporary(builder, expected.clone(), span);
    builder.emit(plan_assign(reference.clone(), operation, span));
    Ok(read_place(reference, access == IncanInputAccess::Exclusive, span))
}

/// Require both the checked access fact and the actual carrier before selecting an operation.
///
/// An explicit owned input may consume a whole owner or copy a primitive Copy value. Moving a field remains refused
/// until partial-move cleanup is represented. Borrowed inputs retain structural field paths and reborrow existing
/// references; a shared path can never supply an exclusive loan. No conversion or snapshot is implicit.
pub fn explicit_input_operation(
    locals: &[Local],
    source: Place,
    fact: OwnershipFact,
    expected: &PlanType,
    access: IncanInputAccess,
    span: &SourceSpan,
) -> Result<RvalueKind, String> {
    if source.local < 0 || source.local as usize >= locals.len() {
        return Err("invalid Body IR explicit argument local".into());
    }
    let actual = place_type(locals, &source);
    if access == IncanInputAccess::Owned {
        if actual != *expected {
            return Err("unsupported Body IR explicit owned argument carrier mismatch".into());
        }
        if fact == OwnershipFact::Copy
            && (numeric_carrier(&actual) || matches!(actual, PlanType::Bool | PlanType::Unit))
        {
            return Ok(RvalueKind::Use(read_place(source, false, span)));
        }
        if fact != OwnershipFact::Move || source.projection != Projection::Whole {
            return Err("unsupported Body IR explicit owned argument without a whole moved owner".into());
        }
        return Ok(RvalueKind::Use(read_place(source, true, span)));
    }
    let exclusive = access == IncanInputAccess::Exclusive;
    if !matches!(access, IncanInputAccess::Shared | IncanInputAccess::InferredShared) && !exclusive {
        return Err("invalid Body IR explicit argument access".into());
    }
    if (exclusive && fact != OwnershipFact::MutBorrow) || (!exclusive && fact != OwnershipFact::Borrow) {
        return Err("unsupported Body IR explicit argument without its checked borrow fact".into());
    }
    let (referent, writable) = reference_contract(expected)?;
    if writable != exclusive {
        return Err("invalid Body IR explicit input reference contract".into());
    }
    let mut borrowed = source.clone();
    if actual != referent {
        let (held, held_writable) = reference_contract(&actual)?;
        if held != referent || (exclusive && !held_writable) {
            return Err("unsupported Body IR explicit argument reborrow contract".into());
        }
        borrowed = dereferenced_place(&source, referent, span)?;
    }
    if exclusive {
        require_writable_path(locals, &borrowed)?;
        return Ok(RvalueKind::MutBorrow(borrowed));
    }
    Ok(RvalueKind::Borrow(borrowed))
}

/// Check each dereferenced layer, including a shared reference stored inside an otherwise writable model.
fn require_writable_path(locals: &[Local], source: &Place) -> Result<(), String> {
    let mut current = locals[source.local as usize].ty.clone();
    let fields = match &source.projection {
        Projection::Deref(_) | Projection::DerefField(..) => {
            writable_referent(&current)?;
            return Ok(());
        }
        Projection::DerefFields(path) => {
            current = writable_referent(&current)?;
            path
        }
        Projection::Fields(path) => path,
        _ => return Ok(()),
    };
    for field in fields {
        for _ in 0..field.dereferences {
            current = writable_referent(&current)?;
        }
        current = field.ty.clone();
    }
    Ok(())
}

/// Refuse a mutable projection through any shared reference even when later fields are themselves mutable.
fn writable_referent(ty: &PlanType) -> Result<PlanType, String> {
    let (referent, writable) = reference_contract(ty)?;
    if !writable {
        return Err("unsupported Body IR exclusive argument through shared storage".into());
    }
    Ok(referent)
}
