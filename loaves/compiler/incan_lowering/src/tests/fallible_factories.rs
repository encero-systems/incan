//! A published standard fallible-iterator default selects the standard primitives' factory at its carrier's checked
//! type arguments; a spelling alone, a member outside the standard trait or a foreign return selects none.

use std::collections::BTreeMap;
use std::sync::Arc;

use incan_mir_plan_native::PlanType;
use incan_semantics_core::body_ir::{
    ArgumentBinding, BodyIrModule, MethodTarget, NewtypeDeserialize, NewtypeStringConversion, NominalDeclaration,
};
use incan_semantics_core::{
    CanonicalSymbolId, CompilerNodeId, HirSourceSpan, IncanPrimitiveType, IncanType, SemanticSourceTargetKind,
    SymbolNamespace, SymbolOrigin,
};

use super::bodies::empty_body;
use crate::fallible_factories::{NativeFactory, fallible_factory};
use crate::native_type_identity::ConstructorIndex;

fn collection() -> SymbolOrigin {
    SymbolOrigin::Package {
        library: "incan_stdlib_primitives".into(),
        module_path: vec!["derives".into(), "collection".into()],
    }
}

fn symbol(
    origin: SymbolOrigin,
    name: &str,
    kind: SemanticSourceTargetKind,
    namespace: SymbolNamespace,
    span: (usize, usize),
) -> CanonicalSymbolId {
    CanonicalSymbolId {
        namespace,
        origin,
        declaration_name: name.into(),
        kind,
        scope_discriminant: None,
        declaration_span: HirSourceSpan::new(span.0, span.1),
    }
}

fn carrier(name: &str, arguments: Vec<IncanType>) -> NominalDeclaration {
    let canonical = symbol(
        collection(),
        name,
        SemanticSourceTargetKind::Model,
        SymbolNamespace::OrdinaryLexical,
        (1000, 1200),
    );
    NominalDeclaration {
        record_package_identity: None,
        trait_adoptions: Vec::new(),
        native_item: format!("incan_stdlib_primitives::derives::collection::{name}"),
        class_inheritance: None,
        native_type_arguments: arguments,
        direct_declaration_id: CompilerNodeId::declaration_span("collection", 1000, 1200),
        canonical,
        name: name.into(),
        fields: Vec::new(),
        field_identities: Vec::new(),
        field_aliases: Vec::new(),
        field_types: Vec::new(),
        owned_callable_fields: Vec::new(),
        field_public: Vec::new(),
        reflection_fields: None,
        display_method: None,
        newtype_deserialize: NewtypeDeserialize::Unselected,
        newtype_string_conversion: NewtypeStringConversion::Unselected,
        reflection_metadata: None,
        comparison_equality: None,
        public: true,
        has_field_defaults: false,
        field_default_body: None,
        validated_constructor: None,
        method_partial_defaults: Vec::new(),
        derives: Vec::new(),
        named_type_identities: Default::default(),
        type_parameters: vec!["T".into(), "E".into(), "Source".into()],
        type_parameter_count: 3,
    }
}

/// The standard FallibleIterator trait with its `take` default returning `TakeFallibleIterator`.
fn standard_module(trait_origin: SymbolOrigin, member_span: (usize, usize)) -> (BodyIrModule, MethodTarget) {
    let root = symbol(
        trait_origin,
        "FallibleIterator",
        SemanticSourceTargetKind::Trait,
        SymbolNamespace::OrdinaryLexical,
        (100, 900),
    );
    let slot = symbol(
        collection(),
        "take",
        SemanticSourceTargetKind::Method,
        SymbolNamespace::Member,
        member_span,
    );
    let declaration = carrier(
        "TakeFallibleIterator",
        vec![
            IncanType::Primitive(IncanPrimitiveType::Int),
            IncanType::Primitive(IncanPrimitiveType::Str),
            IncanType::Primitive(IncanPrimitiveType::Bool),
        ],
    );
    let mut body = empty_body("take");
    body.canonical = Some(slot.clone());
    body.return_type = IncanType::Named("TakeFallibleIterator".into());
    body.named_type_identities = Arc::new(BTreeMap::from([(
        "TakeFallibleIterator".to_owned(),
        declaration.canonical.clone(),
    )]));
    let mut module = BodyIrModule::declarations_only("main".into());
    module.trait_declarations = vec![root];
    module.nominal_declarations = vec![declaration];
    module.bodies = vec![body];
    let target = MethodTarget {
        name: "take".into(),
        intrinsic_carrier: None,
        operation: None,
        loaf_trait: None,
        trait_arguments: None,
        canonical: Some(slot),
        type_args: Vec::new(),
        native_trait_delegation: None,
        binding: ArgumentBinding::UnresolvedPositional,
    };
    (module, target)
}

fn result() -> PlanType {
    PlanType::Model(0, "TakeFallibleIterator".into())
}

#[test]
fn a_published_default_selects_the_standard_primitives_factory() -> Result<(), String> {
    let (module, target) = standard_module(collection(), (200, 300));
    let factory = fallible_factory(&ConstructorIndex::default(), &module, &target, &result(), &[])?;
    assert_eq!(
        factory,
        Some(NativeFactory {
            path: "incan_stdlib_primitives::fallible_take".into(),
            arguments: vec![PlanType::Int, PlanType::String, PlanType::Bool],
        })
    );
    Ok(())
}

#[test]
fn a_spelling_or_a_member_outside_the_standard_trait_selects_none() -> Result<(), String> {
    let constructors = ConstructorIndex::default();
    let (outside, target) = standard_module(collection(), (950, 990));
    assert_eq!(
        fallible_factory(&constructors, &outside, &target, &result(), &[])?,
        None
    );
    let (project, target) = standard_module(SymbolOrigin::Module(vec!["app".into()]), (200, 300));
    assert_eq!(
        fallible_factory(&constructors, &project, &target, &result(), &[])?,
        None
    );
    let (module, target) = standard_module(collection(), (200, 300));
    assert_eq!(
        fallible_factory(&constructors, &module, &target, &PlanType::Int, &[])?,
        None
    );
    Ok(())
}

#[test]
fn a_return_of_another_carrier_refuses_and_an_unbound_return_selects_none() -> Result<(), String> {
    let constructors = ConstructorIndex::default();
    let (mut module, target) = standard_module(collection(), (200, 300));
    module.nominal_declarations[0] = carrier("MapFallibleIterator", Vec::new());
    assert!(fallible_factory(&constructors, &module, &target, &result(), &[]).is_err());
    let (mut unbound, target) = standard_module(collection(), (200, 300));
    unbound.bodies[0].named_type_identities = Arc::new(BTreeMap::new());
    assert_eq!(
        fallible_factory(&constructors, &unbound, &target, &result(), &[])?,
        None
    );
    Ok(())
}
