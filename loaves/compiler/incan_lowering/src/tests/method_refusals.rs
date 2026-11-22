//! A method call whose selected implementation has no matching receiver frame refuses with the reference's text.

use std::sync::Arc;

use incan_mir_plan_native::PlanType;
use incan_semantics_core::body_ir::{
    ArgumentBinding, BodyIrModule, LocalDecl, LocalId, LocalOrigin, MethodTarget, NewtypeDeserialize,
    NewtypeStringConversion, NominalDeclaration, ScopeId,
};
use incan_semantics_core::{
    CanonicalSymbolId, CompilerNodeId, HirSourceSpan, IncanPrimitiveType, IncanType, SemanticSourceTargetKind,
    SymbolNamespace, SymbolOrigin,
};

use super::bodies::empty_body;
use crate::body_index::body_index;
use crate::method_calls::owner_method_index;
use crate::native_type_identity::ConstructorIndex;
use crate::program_context::synthetic_program_context;
use crate::string_input::{SyntheticFrame, synthetic_builder};

fn app_symbol(
    name: &str,
    kind: SemanticSourceTargetKind,
    namespace: SymbolNamespace,
    span: (usize, usize),
) -> CanonicalSymbolId {
    CanonicalSymbolId {
        namespace,
        origin: SymbolOrigin::Module(vec!["app".into()]),
        declaration_name: name.into(),
        kind,
        scope_discriminant: None,
        declaration_span: HirSourceSpan::new(span.0, span.1),
    }
}

fn point(canonical: CanonicalSymbolId) -> NominalDeclaration {
    NominalDeclaration {
        record_package_identity: None,
        trait_adoptions: Vec::new(),
        native_item: String::new(),
        class_inheritance: None,
        native_type_arguments: Vec::new(),
        direct_declaration_id: CompilerNodeId::declaration_span("app", 0, 100),
        canonical,
        name: "Point".into(),
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
        type_parameters: Vec::new(),
        type_parameter_count: 0,
    }
}

/// A body named `render` under the implementation identity, with the given locals and no parameters.
fn candidate(implementation: &CanonicalSymbolId, locals: Vec<LocalDecl>) -> incan_semantics_core::body_ir::Body {
    let mut body = empty_body("render");
    body.canonical = Some(implementation.clone());
    body.locals = locals;
    body
}

#[test]
fn a_method_without_a_receiver_frame_names_its_candidates() {
    let owner_identity = app_symbol(
        "Point",
        SemanticSourceTargetKind::Model,
        SymbolNamespace::OrdinaryLexical,
        (0, 100),
    );
    let implementation = app_symbol(
        "render",
        SemanticSourceTargetKind::Method,
        SymbolNamespace::Member,
        (10, 20),
    );
    let integer = LocalDecl {
        id: LocalId(0),
        name: Some("value".into()),
        identity: None,
        ty: IncanType::Primitive(IncanPrimitiveType::Int),
        origin: LocalOrigin::UserBinding,
        scope: ScopeId(0),
        span: HirSourceSpan::new(12, 14),
    };
    let mut module = BodyIrModule::declarations_only("main".into());
    module.nominal_declarations = vec![point(owner_identity.clone())];
    module.bodies = vec![
        candidate(&implementation, Vec::new()),
        candidate(&implementation, vec![integer]),
        empty_body("other"),
    ];
    let mut program = synthetic_program_context(&module, Arc::new(ConstructorIndex::default()));
    program.body_indices = body_index(&module);
    let frame = SyntheticFrame {
        locals: Vec::new(),
        parameter_count: 0,
        file: "main.incn".into(),
        module_identity: "app".into(),
        unions: Vec::new(),
        intrinsics: Vec::new(),
        name: "main".into(),
        owned: Vec::new(),
    };
    let builder = synthetic_builder(program, frame);
    let target = MethodTarget {
        name: "render".into(),
        intrinsic_carrier: None,
        operation: None,
        loaf_trait: None,
        trait_arguments: None,
        canonical: Some(implementation.clone()),
        type_args: Vec::new(),
        native_trait_delegation: None,
        binding: ArgumentBinding::UnresolvedPositional,
    };
    let owner = PlanType::Model(0, "Point".into());
    let refusal = owner_method_index(&module, &target, &owner, &builder).expect_err("no candidate has a receiver");
    let expected = format!(
        "unsupported Body IR nonlocal or trait-object Method; target={target:?}; owner=Model(0, \"Point\"); owner_identity={owner_identity:?}; implementation={implementation:?}; canonical_candidates=2; candidates_limit=6; candidates=[render(params=0, receiver=<no locals>); render(params=0, raw_receiver=int, frame=Int); ]",
        target = Some(&implementation),
        implementation = Some(&implementation),
    );
    assert_eq!(refusal, expected);
}
