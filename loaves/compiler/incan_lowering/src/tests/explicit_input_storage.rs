//! Native input ABI follows explicit checked access without changing inferred parameter behavior.

use incan_mir_plan_native::{ListLeaf, PlanType};
use incan_semantics_core::{IncanCallableParamKind, IncanInputAccess};

use crate::closures::{packed_input, semantic_input, signature_input};

/// An explicit scalar loan keeps caller storage even though ordinary mutable scalars are local copies.
#[test]
fn explicit_scalar_inputs_keep_real_references() -> Result<(), String> {
    let inferred = IncanInputAccess::Inferred;
    assert!(signature_input(PlanType::Int, true, false, inferred)? == PlanType::Int);
    assert!(
        signature_input(PlanType::Int, false, false, IncanInputAccess::Shared)?
            == PlanType::ScalarRef(vec![PlanType::Int])
    );
    assert!(
        signature_input(PlanType::Int, false, false, IncanInputAccess::Exclusive)?
            == PlanType::ScalarMutRef(vec![PlanType::Int])
    );
    assert!(signature_input(PlanType::Int, true, false, IncanInputAccess::Owned)? == PlanType::Int);
    Ok(())
}

/// Owned input remains a value while ordinary collection input keeps the established shared/exclusive ABI.
#[test]
fn explicit_owned_collections_bypass_inferred_borrowing() -> Result<(), String> {
    let ty = PlanType::List(ListLeaf::Int, 1);
    let inferred = IncanInputAccess::Inferred;
    assert!(signature_input(ty.clone(), false, false, inferred)? == PlanType::ListRef(ListLeaf::Int, 1));
    assert!(signature_input(ty.clone(), true, false, inferred)? == PlanType::ListMutRef(ListLeaf::Int, 1));
    assert!(signature_input(ty.clone(), true, false, IncanInputAccess::Owned)? == ty);
    assert!(
        signature_input(ty.clone(), false, false, IncanInputAccess::Shared)? == PlanType::ListRef(ListLeaf::Int, 1)
    );
    assert!(signature_input(ty, false, false, IncanInputAccess::Exclusive)? == PlanType::ListMutRef(ListLeaf::Int, 1));
    Ok(())
}

/// Semantic callable types and packed frame parameters agree for text and nominal reference carriers.
#[test]
fn callable_and_frame_inputs_share_access_selection() -> Result<(), String> {
    let types = [
        PlanType::String,
        PlanType::Model(0, "Point".into()),
        PlanType::Enum(1, "Choice".into()),
    ];
    let modes = [
        IncanInputAccess::Shared,
        IncanInputAccess::Exclusive,
        IncanInputAccess::Owned,
    ];
    for ty in &types {
        for mode in modes {
            let expected = signature_input(ty.clone(), false, false, mode)?;
            assert!(packed_input(ty.clone(), IncanCallableParamKind::Normal, false, false, mode)? == expected);
            assert!(semantic_input(ty.clone(), IncanCallableParamKind::Normal, false, false, mode)? == expected);
        }
    }
    assert!(signature_input(PlanType::String, false, false, IncanInputAccess::Shared)? == PlanType::StringRef);
    assert!(
        signature_input(
            PlanType::Model(0, "Point".into()),
            false,
            false,
            IncanInputAccess::Exclusive
        )? == PlanType::ModelMutRef(0, "Point".into())
    );
    Ok(())
}

/// Native ownership and rest-pack contracts cannot silently override a checked explicit mode.
#[test]
fn explicit_inputs_refuse_conflicting_or_unrepresented_packs() {
    let ty = PlanType::List(ListLeaf::Int, 1);
    assert!(signature_input(ty.clone(), false, true, IncanInputAccess::Shared).is_err());
    assert!(
        packed_input(
            ty,
            IncanCallableParamKind::RestPositional,
            false,
            false,
            IncanInputAccess::Owned
        )
        .is_err()
    );
    assert!(
        semantic_input(
            PlanType::Int,
            IncanCallableParamKind::RestKeyword,
            false,
            false,
            IncanInputAccess::Shared
        )
        .is_err()
    );
}
