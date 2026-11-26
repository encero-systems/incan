//! Explicit call inputs retain actual storage and never select the ordinary repair-copy route.

use incan_mir_plan_native::{
    FieldProjection, ListLeaf, Local, Place, PlanType, Projection, RvalueKind, SourceSpan, place, source_span,
};
use incan_semantics_core::IncanInputAccess;
use incan_semantics_core::body_ir::OwnershipFact;

use crate::explicit_arguments::{argument_access, explicit_input_operation};
use crate::frame::read_place;

fn span() -> SourceSpan {
    source_span("explicit_arguments.incn", 1, 1)
}

fn local(ty: PlanType) -> Local {
    Local { ty, span: span() }
}

/// Scalar, collection and text loans name the caller's storage, including a writable existing loan.
#[test]
fn explicit_inputs_borrow_existing_storage_and_reborrow_exclusively() -> Result<(), String> {
    let span = span();
    let owner = place(0, &span);
    let types = [PlanType::Int, PlanType::List(ListLeaf::Int, 1), PlanType::String];
    let shared = [
        PlanType::ScalarRef(vec![PlanType::Int]),
        PlanType::ListRef(ListLeaf::Int, 1),
        PlanType::StringRef,
    ];
    let exclusive = [
        PlanType::ScalarMutRef(vec![PlanType::Int]),
        PlanType::ListMutRef(ListLeaf::Int, 1),
        PlanType::StringMutRef,
    ];
    for index in 0..types.len() {
        let locals = vec![local(types[index].clone())];
        let borrowed = explicit_input_operation(
            &locals,
            owner.clone(),
            OwnershipFact::Borrow,
            &shared[index],
            IncanInputAccess::Shared,
            &span,
        )?;
        assert!(borrowed == RvalueKind::Borrow(owner.clone()));
        let written = explicit_input_operation(
            &locals,
            owner.clone(),
            OwnershipFact::MutBorrow,
            &exclusive[index],
            IncanInputAccess::Exclusive,
            &span,
        )?;
        assert!(written == RvalueKind::MutBorrow(owner.clone()));
        let reread = explicit_input_operation(
            &[local(shared[index].clone())],
            owner.clone(),
            OwnershipFact::Borrow,
            &shared[index],
            IncanInputAccess::Shared,
            &span,
        )?;
        let dereferenced = Place {
            local: 0,
            projection: Projection::Deref(types[index].clone()),
            span: span.clone(),
        };
        assert!(reread == RvalueKind::Borrow(dereferenced));
        let reborrow = explicit_input_operation(
            &[local(exclusive[index].clone())],
            owner.clone(),
            OwnershipFact::MutBorrow,
            &exclusive[index],
            IncanInputAccess::Exclusive,
            &span,
        )?;
        let RvalueKind::MutBorrow(storage) = reborrow else {
            return Err("an exclusive loan of existing exclusive storage is a reborrow".into());
        };
        assert_eq!(storage.local, owner.local);
        assert!(storage.projection == Projection::Deref(types[index].clone()));
    }
    Ok(())
}

/// Owned lists move only from their real owner; Clone, Borrow and a moved ListRef cannot create a snapshot.
#[test]
fn explicit_owned_inputs_never_clone_borrowed_storage() -> Result<(), String> {
    let span = span();
    let owner = place(0, &span);
    let ty = PlanType::List(ListLeaf::Int, 1);
    let locals = vec![local(ty.clone())];
    let moved = explicit_input_operation(
        &locals,
        owner.clone(),
        OwnershipFact::Move,
        &ty,
        IncanInputAccess::Owned,
        &span,
    )?;
    assert!(moved == RvalueKind::Use(read_place(owner.clone(), true, &span)));
    for fact in [
        OwnershipFact::Clone,
        OwnershipFact::Borrow,
        OwnershipFact::Copy,
        OwnershipFact::MutBorrow,
    ] {
        assert!(explicit_input_operation(&locals, owner.clone(), fact, &ty, IncanInputAccess::Owned, &span).is_err());
    }
    let borrowed = vec![local(PlanType::ListRef(ListLeaf::Int, 1))];
    assert!(
        explicit_input_operation(
            &borrowed,
            owner,
            OwnershipFact::Move,
            &ty,
            IncanInputAccess::Owned,
            &span
        )
        .is_err()
    );
    Ok(())
}

/// A writable field remains the same place, and a shared reference midway through a path removes write authority.
#[test]
fn explicit_nested_loans_keep_paths_and_refuse_shared_ancestors() -> Result<(), String> {
    let span = span();
    let locals = vec![local(PlanType::Model(0, "Customer".into()))];
    let at = |fields: Vec<FieldProjection>| Place {
        local: 0,
        projection: Projection::Fields(fields),
        span: span.clone(),
    };
    let nested = at(vec![
        FieldProjection::new(1, PlanType::Model(1, "Address".into())),
        FieldProjection::new(2, PlanType::String),
    ]);
    let written = explicit_input_operation(
        &locals,
        nested.clone(),
        OwnershipFact::MutBorrow,
        &PlanType::StringMutRef,
        IncanInputAccess::Exclusive,
        &span,
    )?;
    assert!(written == RvalueKind::MutBorrow(nested.clone()));
    assert!(
        explicit_input_operation(
            &locals,
            nested,
            OwnershipFact::Move,
            &PlanType::String,
            IncanInputAccess::Owned,
            &span
        )
        .is_err()
    );
    let stored_reference = at(vec![FieldProjection::new(2, PlanType::StringRef)]);
    let mut dereference = FieldProjection::new(-1, PlanType::String);
    dereference.dereferences = 1;
    let expected_reference = at(vec![FieldProjection::new(2, PlanType::StringRef), dereference]);
    let shared = explicit_input_operation(
        &locals,
        stored_reference.clone(),
        OwnershipFact::Borrow,
        &PlanType::StringRef,
        IncanInputAccess::Shared,
        &span,
    )?;
    assert!(shared == RvalueKind::Borrow(expected_reference));
    let mismatched = explicit_input_operation(
        &locals,
        stored_reference,
        OwnershipFact::Borrow,
        &PlanType::ScalarRef(vec![PlanType::Int]),
        IncanInputAccess::Shared,
        &span,
    );
    assert!(
        mismatched.is_err(),
        "a stored text reference cannot supply an integer referent"
    );
    let mut through = FieldProjection::new(2, PlanType::String);
    through.dereferences = 1;
    let through_shared = at(vec![
        FieldProjection::new(1, PlanType::ModelRef(1, "Address".into())),
        through,
    ]);
    assert!(
        explicit_input_operation(
            &locals,
            through_shared.clone(),
            OwnershipFact::MutBorrow,
            &PlanType::StringMutRef,
            IncanInputAccess::Exclusive,
            &span,
        )
        .is_err()
    );
    let reread = explicit_input_operation(
        &locals,
        through_shared.clone(),
        OwnershipFact::Borrow,
        &PlanType::StringRef,
        IncanInputAccess::Shared,
        &span,
    )?;
    assert!(reread == RvalueKind::Borrow(through_shared));
    Ok(())
}

/// A mismatched slot vector or checked fact is refused instead of defaulting to ordinary call semantics.
#[test]
fn explicit_input_slots_and_facts_cannot_fall_back() -> Result<(), String> {
    let accesses = [
        IncanInputAccess::Shared,
        IncanInputAccess::Owned,
        IncanInputAccess::Exclusive,
    ];
    assert!(argument_access(&accesses, 3, 2)? == IncanInputAccess::Exclusive);
    assert!(argument_access(&accesses, 3, 0)? == IncanInputAccess::Shared);
    assert!(argument_access(&accesses, 2, 0).is_err());
    assert!(argument_access(&accesses, 3, 3).is_err());
    assert!(argument_access(&[], 1, 0)? == IncanInputAccess::Inferred);
    let span = span();
    let owner = place(0, &span);
    let locals = vec![local(PlanType::Int)];
    let exclusive = PlanType::ScalarMutRef(vec![PlanType::Int]);
    for fact in [
        OwnershipFact::Move,
        OwnershipFact::Clone,
        OwnershipFact::Copy,
        OwnershipFact::Unknown,
    ] {
        let operation = explicit_input_operation(
            &locals,
            owner.clone(),
            fact,
            &exclusive,
            IncanInputAccess::Exclusive,
            &span,
        );
        assert!(operation.is_err());
    }
    let shared_fact = explicit_input_operation(
        &locals,
        owner,
        OwnershipFact::Borrow,
        &exclusive,
        IncanInputAccess::Shared,
        &span,
    );
    assert!(shared_fact.is_err());
    Ok(())
}
