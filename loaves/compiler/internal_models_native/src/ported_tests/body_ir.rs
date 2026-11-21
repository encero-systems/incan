//! Ported from internal_models/tests/test_body_ir_records.incn: complete Body IR coverage and every mandatory
//! skipped-field disposition.

use incan_semantics_core::body_ir::{
    NativeAttribute, NewtypeDeserialize, NewtypeStringConversion, RuntimeCallableBinding, StringConversionContract,
};
use incan_semantics_core::{
    CanonicalSymbolId as NativeSymbolId, CompilerNodeId, HirSourceSpan as NativeSpan, SemanticSourceTargetKind,
};

use crate::body_ir_declarations::{
    CompilerBody, CompilerBodyCoverage, CompilerBodyIrModule, CompilerDeclarationSet, CompilerLoafCallable,
    CompilerNominalDeclaration, CompilerTraitAdoption, CompilerValidatedConstructor, NamedTypeIdentity,
    TraitDefaultInstantiation, TraitFieldWitness, TraitTypeArgument, covered_body_count,
    invalidate_owned_callable_fields, valid_compiler_body, valid_invocation_view, valid_loaf_callable,
    valid_nominal_declaration, valid_published_expanded_body,
};
use crate::body_ir_values::CompilerBlock;
use crate::checked_types::{CheckedPrimitiveType, CheckedType};
use crate::compiler_records::{CanonicalSymbolId, DeclarationKind, HirSourceSpan, SymbolNamespace, SymbolOrigin};

/// One authored module declaration identity with complete source provenance.
fn identity(name: &str, kind: DeclarationKind) -> CanonicalSymbolId {
    CanonicalSymbolId {
        namespace: SymbolNamespace::OrdinaryLexical,
        origin: SymbolOrigin::Module(vec!["app".into()]),
        declaration_name: name.into(),
        kind,
        scope_discriminant: None,
        declaration_span: span(),
    }
}

fn span() -> HirSourceSpan {
    HirSourceSpan {
        file: "src/main.incn".into(),
        start: 0,
        end: 10,
        expansion: None,
    }
}

fn unit() -> CheckedType {
    CheckedType::Primitive(CheckedPrimitiveType::Unit)
}

/// One complete scalar authored body for focused record-policy controls.
fn empty_body(name: &str) -> CompilerBody {
    CompilerBody {
        nominal_instantiation: Vec::new(),
        trait_default_instantiation: None,
        type_parameters: Vec::new(),
        declaration_id: CompilerNodeId::declaration("app", name),
        direct_call_id: CompilerNodeId::declaration_span("app", 0, 10),
        canonical: Some(identity(name, DeclarationKind::Function)),
        name: name.into(),
        span: span(),
        return_type: unit(),
        callable_representation: None,
        checked_input_profile: None,
        explicit_input_bindings: Vec::new(),
        returned_view: None,
        named_type_identities: vec![NamedTypeIdentity {
            name: "Packet".into(),
            identity: identity("Packet", DeclarationKind::Model),
        }],
        locals: Vec::new(),
        params: Vec::new(),
        param_locals: Vec::new(),
        scopes: Vec::new(),
        block: CompilerBlock {
            scope: 0,
            statements: Vec::new(),
        },
        runtime_requirements: Vec::new(),
        panic_facts: Vec::new(),
        is_async: false,
        retired_rust_facade_wire: None,
        native_attributes: Vec::new(),
    }
}

/// One private single-field nominal with optional nonempty default evidence.
fn nominal(
    canonical: CanonicalSymbolId,
    adoptions: Vec<CompilerTraitAdoption>,
    with_default: bool,
) -> CompilerNominalDeclaration {
    CompilerNominalDeclaration {
        trait_adoptions: adoptions,
        native_item: String::new(),
        class_inheritance: None,
        native_type_arguments: Vec::new(),
        direct_declaration_id: CompilerNodeId::declaration_span("app", 0, 10),
        canonical,
        name: "Packet".into(),
        fields: vec!["payload".into()],
        field_identities: vec![identity("payload", DeclarationKind::Field)],
        field_types: vec![CheckedType::Primitive(CheckedPrimitiveType::Int)],
        owned_callable_fields: Vec::new(),
        field_public: vec![false],
        field_aliases: vec![None],
        reflection_fields: None,
        reflection_metadata: None,
        comparison_equality: None,
        public: false,
        has_field_defaults: with_default,
        field_default_body: if with_default {
            vec![empty_body("field_default")]
        } else {
            Vec::new()
        },
        validated_constructor: None,
        method_partial_defaults: Vec::new(),
        display_method: None,
        newtype_deserialize: NewtypeDeserialize::Unselected,
        newtype_string_conversion: NewtypeStringConversion::Unselected,
        derives: Vec::new(),
        named_type_identities: Vec::new(),
        type_parameters: Vec::new(),
        type_parameter_count: 0,
        record_package_identity: None,
    }
}

/// One marker-trait adoption with an explicit coverage state and required-field witness list.
fn adoption(
    adopter: CanonicalSymbolId,
    methods: Option<Vec<CanonicalSymbolId>>,
    fields: Vec<TraitFieldWitness>,
) -> CompilerTraitAdoption {
    CompilerTraitAdoption {
        adopter,
        trait_identity: identity("Readable", DeclarationKind::Trait),
        arguments: Vec::new(),
        methods,
        fields,
        primitive_adapter: None,
    }
}

/// One complete module record with no invocation selection or declarations.
fn empty_module() -> CompilerBodyIrModule {
    CompilerBodyIrModule {
        runtime_callable_bindings: Vec::new(),
        decorated_bindings: Vec::new(),
        entrypoint_effects: Vec::new(),
        program_entrypoints: Vec::new(),
        string_conversion_contract: StringConversionContract::Unselected,
        constant_declarations: Vec::new(),
        type_alias_declarations: Vec::new(),
        enum_declarations: Vec::new(),
        loaf_callables: Vec::new(),
        native_union_declarations: Vec::new(),
        static_declarations: Vec::new(),
        module_id: CompilerNodeId::module("app"),
        nominal_declarations: Vec::new(),
        fieldless_enum_declarations: Vec::new(),
        value_enum_declarations: Vec::new(),
        trait_declarations: Vec::new(),
        trait_implementations: Vec::new(),
        bodies: Vec::new(),
        runtime_binding_scopes: Vec::new(),
    }
}

/// A loaf callable whose declaration frame is `frame`.
fn callable(canonical: CanonicalSymbolId, frame: CompilerBody) -> CompilerLoafCallable {
    CompilerLoafCallable {
        canonical,
        item: "packet::render".into(),
        parameters: Vec::new(),
        return_type: unit(),
        type_parameters: Vec::new(),
        native_type_arguments: Vec::new(),
        native_callable_parameters: Vec::new(),
        parameter_defaults: Vec::new(),
        declaration_frame: vec![frame],
    }
}

/// An authenticated declaration with zero bodies stays distinct from one whose coverage is unknown.
#[test]
fn covered_empty_is_not_uncovered() {
    assert_eq!(covered_body_count(&CompilerBodyCoverage::Covered(Vec::new())), Some(0));
    assert_eq!(
        covered_body_count(&CompilerBodyCoverage::Uncovered("not retained".into())),
        None
    );
}

/// Missing method metadata stays distinct from an authenticated marker trait with no method slots.
#[test]
fn trait_method_coverage_keeps_none_and_some_empty_distinct() {
    let owner = identity("Packet", DeclarationKind::Model);
    assert!(adoption(owner.clone(), None, Vec::new()).methods.is_none());
    assert_eq!(
        adoption(owner, Some(Vec::new()), Vec::new())
            .methods
            .map(|methods| methods.len()),
        Some(0)
    );
}

/// Runtime bindings and selected mains mark an invocation view; an ordinary dependency module stays unselected.
#[test]
fn runtime_bindings_and_program_entrypoints_are_invocation_only() {
    let mut module = empty_module();
    assert!(!valid_invocation_view(&module));
    module.runtime_callable_bindings = vec![RuntimeCallableBinding {
        path: "runtime::print".into(),
        canonical: NativeSymbolId::module_declaration(
            vec!["app".into()],
            "print",
            SemanticSourceTargetKind::Function,
            NativeSpan::new(0, 10),
        ),
    }];
    assert!(valid_invocation_view(&module));
    let mut module = empty_module();
    module.program_entrypoints = vec![identity("main", DeclarationKind::Function)];
    assert!(valid_invocation_view(&module));
    module.program_entrypoints.clear();
    assert!(!valid_invocation_view(&module));
}

/// A closed owner's ordered substitutions and the selected adoption's binders are retained as given.
#[test]
fn nominal_and_trait_default_instantiations_keep_their_binders() {
    let mut body = empty_body("render");
    body.nominal_instantiation = vec![
        TraitTypeArgument {
            parameter: "T".into(),
            argument: CheckedType::Named("Invoice".into()),
        },
        TraitTypeArgument {
            parameter: "U".into(),
            argument: CheckedType::Named("Line".into()),
        },
    ];
    assert_eq!(body.nominal_instantiation[0].parameter, "T");
    assert_eq!(
        body.nominal_instantiation[1].argument,
        CheckedType::Named("Line".into())
    );
    body.type_parameters = vec!["T".into()];
    body.trait_default_instantiation = Some(TraitDefaultInstantiation {
        receiver_type: CheckedType::Named("Invoice".into()),
        arguments: vec![TraitTypeArgument {
            parameter: "T".into(),
            argument: CheckedType::Named("Money".into()),
        }],
    });
    let selected = body
        .trait_default_instantiation
        .as_ref()
        .map(|selected| &selected.arguments[0]);
    assert_eq!(selected.map(|argument| argument.parameter.as_str()), Some("T"));
    assert_eq!(
        selected.map(|argument| &argument.argument),
        Some(&CheckedType::Named("Money".into()))
    );
}

/// Macro inputs are permitted before expansion and refused in an already-expanded published body; a retired facade
/// payload is refused.
#[test]
fn native_attributes_and_retired_facades_are_refused_in_published_bodies() {
    let mut body = empty_body("route");
    body.native_attributes = vec![NativeAttribute {
        path: "web::route".into(),
        arguments: Vec::new(),
    }];
    assert!(!valid_published_expanded_body(&body));
    body.native_attributes.clear();
    assert!(valid_published_expanded_body(&body));
    body.retired_rust_facade_wire = Some(("provider".into(), "method".into()));
    assert!(!valid_compiler_body(&body));
}

/// Required and implementing field identities belong to their adopter; a forged enclosing owner is refused.
#[test]
fn trait_field_witnesses_belong_to_the_adoption() {
    let owner = identity("Packet", DeclarationKind::Model);
    let witness = TraitFieldWitness {
        requirement: identity("value", DeclarationKind::Field),
        implementation: identity("payload", DeclarationKind::Field),
    };
    let checked = adoption(owner.clone(), Some(Vec::new()), vec![witness]);
    assert!(valid_nominal_declaration(&nominal(owner.clone(), vec![checked], false)));
    let forged = nominal(
        identity("Other", DeclarationKind::Model),
        vec![adoption(owner, Some(Vec::new()), Vec::new())],
        false,
    );
    assert!(!valid_nominal_declaration(&forged));
}

/// Stale callable and validation slots are rejected before a field-type rewrite can publish invalid indexes.
#[test]
fn owned_callable_fields_are_invalidated_before_rewrite() {
    let mut declaration = nominal(identity("Packet", DeclarationKind::Model), Vec::new(), false);
    declaration.owned_callable_fields = vec![0];
    assert!(
        invalidate_owned_callable_fields(&mut declaration.clone())
            .owned_callable_fields
            .is_empty()
    );
    declaration.owned_callable_fields = vec![1];
    assert!(!valid_nominal_declaration(&declaration));
    let constructor = |required_fields: Vec<i64>| CompilerValidatedConstructor {
        constructor: identity("new", DeclarationKind::Method),
        validator: identity("validate", DeclarationKind::Method),
        required_fields,
    };
    let mut validation = nominal(identity("Packet", DeclarationKind::Model), Vec::new(), false);
    validation.validated_constructor = Some(constructor(vec![0]));
    assert!(valid_nominal_declaration(&validation));
    validation.validated_constructor = Some(constructor(vec![0, 0]));
    assert!(!valid_nominal_declaration(&validation));
}

/// One empty frame from the exact callable body is accepted; a differently owned frame is refused.
#[test]
fn declaration_frame_is_authenticated_and_declaration_only() {
    let callable_identity = identity("render", DeclarationKind::Function);
    let mut frame = empty_body("render");
    frame.canonical = Some(callable_identity.clone());
    assert!(valid_loaf_callable(&callable(callable_identity.clone(), frame.clone())));
    frame.canonical = Some(identity("other", DeclarationKind::Function));
    assert!(!valid_loaf_callable(&callable(callable_identity, frame)));
}

/// Async declaration state, a nonempty field-default frame and private implementations stay explicit.
#[test]
fn async_defaults_and_private_implementations_remain_explicit() {
    let mut implementation = empty_body("private_render");
    implementation.is_async = true;
    let private = CompilerDeclarationSet {
        public_roots: Vec::new(),
        private_implementations: vec![implementation],
    };
    assert!(private.public_roots.is_empty());
    assert!(private.private_implementations[0].is_async);
    let declaration = nominal(identity("Packet", DeclarationKind::Model), Vec::new(), true);
    assert!(declaration.has_field_defaults);
    assert_eq!(declaration.field_default_body.len(), 1);
    assert!(valid_nominal_declaration(&declaration));
}
