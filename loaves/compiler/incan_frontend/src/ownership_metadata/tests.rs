//! Canonical input projection keeps access independent from source spelling and generic instantiation.

use super::{
    AccessMode, CheckedOwnershipContract, OwnershipContract, TypeComponent, direct_input_access, input_value_type,
};
use crate::symbols::ResolvedType;
use incan_lang::lang::types::ownership::OwnershipMode;
use incan_semantics_core::IncanInputAccess;

/// Open declaration types intentionally differ from the closed call parameter used by the tests.
fn contract(mode: OwnershipMode) -> CheckedOwnershipContract {
    OwnershipContract {
        schema_version: 1,
        modes: vec![AccessMode {
            mode,
            component: TypeComponent {
                parameter: Some(1),
                children: Vec::new(),
            },
            underlying: ResolvedType::TypeVar("T".into()),
        }],
        result_source: None,
    }
}

/// Only canonical metadata authorizes unwrapping; an instantiated value is never replaced with the open binder.
#[test]
fn canonical_input_projection_preserves_closed_values() -> Result<(), Box<dyn std::error::Error>> {
    let wrapped = ResolvedType::Generic("ImportedReadAlias".into(), vec![ResolvedType::Int]);
    let ordinary = ResolvedType::Generic("Borrowed".into(), vec![ResolvedType::Int]);
    assert_eq!(input_value_type(None, 1, &ordinary)?, &ordinary);
    for (mode, access) in [
        (OwnershipMode::Borrowed, IncanInputAccess::Shared),
        (OwnershipMode::BorrowedMut, IncanInputAccess::Exclusive),
        (OwnershipMode::Owned, IncanInputAccess::Owned),
    ] {
        let contract = contract(mode);
        assert_eq!(direct_input_access(Some(&contract), 1)?, access);
        assert_eq!(input_value_type(Some(&contract), 1, &wrapped)?, &ResolvedType::Int);
        assert_eq!(input_value_type(Some(&contract), 0, &ordinary)?, &ordinary);
    }
    Ok(())
}

/// Missing structure, nested requests and unsupported schemas cannot authorize incidental wrapper layouts.
#[test]
fn malformed_input_projection_refuses() {
    let wrapped = ResolvedType::Generic("Read".into(), vec![ResolvedType::Int]);
    let mut contract = contract(OwnershipMode::Borrowed);
    assert!(input_value_type(Some(&contract), 1, &ResolvedType::Int).is_err());
    assert!(input_value_type(Some(&contract), 1, &ResolvedType::Generic("Read".into(), Vec::new())).is_err());
    contract.modes[0].component.children.push(0);
    assert!(input_value_type(Some(&contract), 1, &wrapped).is_err());
    contract.modes[0].component.children.clear();
    contract.modes.push(contract.modes[0].clone());
    assert!(input_value_type(Some(&contract), 1, &wrapped).is_err());
    contract.modes.truncate(1);
    contract.modes[0].mode = OwnershipMode::Reference;
    assert!(input_value_type(Some(&contract), 1, &wrapped).is_err());
    contract.schema_version = 2;
    assert!(input_value_type(Some(&contract), 0, &wrapped).is_err());
}

/// Concrete drift and contradictory repeated binders are malformed metadata, while matching closures keep T.
#[test]
fn payload_instantiation_requires_fixed_parts_and_consistent_binders() -> Result<(), Box<dyn std::error::Error>> {
    let mut contract = contract(OwnershipMode::Borrowed);
    let wrap = |ty| ResolvedType::Generic("ReadAlias".into(), vec![ty]);
    contract.modes[0].underlying = ResolvedType::Int;
    assert!(input_value_type(Some(&contract), 1, &wrap(ResolvedType::Str)).is_err());
    contract.modes[0].underlying = ResolvedType::Tuple(vec![ResolvedType::TypeVar("T".into()), ResolvedType::Int]);
    assert!(
        input_value_type(
            Some(&contract),
            1,
            &wrap(ResolvedType::Tuple(vec![ResolvedType::Str, ResolvedType::Str]))
        )
        .is_err()
    );
    let valid = wrap(ResolvedType::Tuple(vec![ResolvedType::Str, ResolvedType::Int]));
    assert_eq!(
        input_value_type(Some(&contract), 1, &valid)?,
        &ResolvedType::Tuple(vec![ResolvedType::Str, ResolvedType::Int])
    );
    contract.modes[0].underlying = ResolvedType::Tuple(vec![
        ResolvedType::TypeVar("T".into()),
        ResolvedType::TypeVar("T".into()),
    ]);
    assert!(input_value_type(Some(&contract), 1, &valid).is_err());
    Ok(())
}
