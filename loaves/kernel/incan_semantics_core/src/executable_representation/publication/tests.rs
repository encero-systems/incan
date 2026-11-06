//! Publication authority checks for generated generic metadata, independent of native field layout planning.

use std::collections::{BTreeMap, BTreeSet};

use super::super::tests::module;
use super::{PublicationAudit, generic_metadata_belongs_to};
use crate::body_ir::{MethodTarget, ReflectionOperation};
use crate::{
    CanonicalSymbolId, HirSourceSpan, ScopeDiscriminant, SemanticSourceTargetKind, SymbolNamespace, SymbolOrigin,
};

/// Keep the generic frame distinct from the narrower source span of its parameter.
fn callable() -> CanonicalSymbolId {
    CanonicalSymbolId {
        namespace: SymbolNamespace::OrdinaryLexical,
        origin: SymbolOrigin::Module(vec!["library".to_owned()]),
        declaration_name: "inspect".to_owned(),
        kind: SemanticSourceTargetKind::Function,
        scope_discriminant: None,
        declaration_span: HirSourceSpan::new(0, 100),
    }
}

/// Build the same exact owner/member pair retained by checked generic reflection.
fn metadata_target() -> MethodTarget {
    let mut binder = callable();
    binder.declaration_name = "T".to_owned();
    binder.kind = SemanticSourceTargetKind::GenericBinder;
    binder.scope_discriminant = Some(ScopeDiscriminant(1));
    binder.declaration_span = HirSourceSpan::new(12, 13);
    let mut member = binder.clone();
    member.namespace = SymbolNamespace::Member;
    member.kind = SemanticSourceTargetKind::Method;
    member.declaration_name = "__fields__".to_owned();
    let mut target = MethodTarget::synthesized("__fields__");
    target.canonical = Some(member);
    target.operation = Some(ReflectionOperation::GenericMetadata(binder));
    target
}

/// A generated operation belongs only to the generic declaration that supplied its checked binder.
#[test]
fn metadata_requires_the_declared_enclosing_binder() {
    let target = metadata_target();
    let parameters = BTreeSet::from(["T".to_owned()]);
    assert!(generic_metadata_belongs_to(&target, &callable(), &parameters, &[]));
    assert!(!generic_metadata_belongs_to(
        &target,
        &callable(),
        &BTreeSet::new(),
        &[]
    ));
    let mut unrelated = callable();
    unrelated.origin = SymbolOrigin::Module(vec!["other".to_owned()]);
    assert!(!generic_metadata_belongs_to(&target, &unrelated, &parameters, &[]));
    unrelated = callable();
    unrelated.declaration_span = HirSourceSpan::new(200, 300);
    assert!(!generic_metadata_belongs_to(&target, &unrelated, &parameters, &[]));
}

/// A correct method spelling and scoped identity do not substitute for the checked generated operation.
#[test]
fn ordinary_or_concrete_methods_do_not_gain_generic_publication() {
    let parameters = BTreeSet::from(["T".to_owned()]);
    let mut target = metadata_target();
    target.operation = None;
    assert!(!generic_metadata_belongs_to(&target, &callable(), &parameters, &[]));
    target.operation = Some(ReflectionOperation::Metadata);
    assert!(!generic_metadata_belongs_to(&target, &callable(), &parameters, &[]));
}

/// Changing the member ancestry cannot preserve authority merely by retaining the operation's binder.
#[test]
fn metadata_rejects_a_mismatched_method_target() {
    let parameters = BTreeSet::from(["T".to_owned()]);
    let mut target = metadata_target();
    if let Some(member) = &mut target.canonical {
        member.declaration_span = HirSourceSpan::new(20, 21);
    }
    assert!(!generic_metadata_belongs_to(&target, &callable(), &parameters, &[]));
    target = metadata_target();
    if let Some(member) = &mut target.canonical {
        member.kind = SemanticSourceTargetKind::Function;
    }
    assert!(!generic_metadata_belongs_to(&target, &callable(), &parameters, &[]));
}

/// Even internally consistent metadata cannot borrow a binder from another function in the same module.
#[test]
fn metadata_rejects_an_outside_binder_owner() {
    let parameters = BTreeSet::from(["T".to_owned()]);
    let mut target = metadata_target();
    let outside = HirSourceSpan::new(220, 221);
    if let Some(ReflectionOperation::GenericMetadata(binder)) = &mut target.operation {
        binder.declaration_span = outside;
    }
    if let Some(member) = &mut target.canonical {
        member.declaration_span = outside;
    }
    assert!(target.has_valid_generic_metadata());
    assert!(!generic_metadata_belongs_to(&target, &callable(), &parameters, &[]));
}

/// Publication rebases the authenticated pair together while keeping the binder internal to its executable fragment.
#[test]
fn metadata_projection_rebases_both_identities_without_a_callable_dependency() -> Result<(), Box<dyn std::error::Error>>
{
    let module = module(Vec::new());
    let eligible = BTreeSet::new();
    let named = BTreeMap::new();
    let mut audit = PublicationAudit::new(&module, "records", &eligible, &named);
    audit.current_callable = Some(callable());
    audit.type_parameters.insert("T".to_owned());
    let mut target = metadata_target();
    assert_eq!(audit.generic_metadata_target(&mut target), Ok(true));
    let Some(ReflectionOperation::GenericMetadata(binder)) = &target.operation else {
        return Err("publication did not preserve the generic metadata operation".into());
    };
    let member = target
        .canonical
        .as_ref()
        .ok_or("publication lost the generated member")?;
    let origin = SymbolOrigin::Package {
        library: "records".to_owned(),
        module_path: vec!["library".to_owned()],
    };
    assert_eq!(binder.origin, origin);
    assert_eq!(member.origin, origin);
    assert_eq!(binder.declaration_span, member.declaration_span);
    assert_eq!(binder.scope_discriminant, member.scope_discriminant);
    assert!(target.has_valid_generic_metadata());
    assert!(audit.requirements.is_empty());
    Ok(())
}
