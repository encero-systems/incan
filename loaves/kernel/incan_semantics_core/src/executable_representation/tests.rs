//! Tests for module encoding, surface coverage and published declaration contexts.

use std::collections::{BTreeMap, BTreeSet};

use super::{
    CoverageReason, DeclarationCoverage, EXECUTABLE_REPRESENTATION_VERSION, ExecutableDeclaration,
    ExecutableRepresentationError, MAX_SURFACE_INDEX_BYTES, OLDEST_SUPPORTED_VERSION, SurfaceIndex, SurfaceReader,
    build_surface, decode_module, publication, require_supported_version,
};
use crate::body_ir::{
    ArgumentBinding, Block, Body, BodyIrModule, CallableParam, CallableParamDefault, CallableTarget, Callee,
    NamedCallableTarget, ScopeId, Statement, StatementKind,
};
use crate::{
    CanonicalSymbolId, CompilerNodeId, HirSourceSpan, IncanPrimitiveType, IncanType, SemanticSourceTargetKind,
    SymbolNamespace, SymbolOrigin,
};

mod coverage;
mod public_contexts;

/// Distinct package identities remain stable even when input bodies are reordered.
fn identity(name: &str, ordinal: usize) -> CanonicalSymbolId {
    CanonicalSymbolId {
        namespace: SymbolNamespace::OrdinaryLexical,
        origin: SymbolOrigin::Package {
            library: "probe".into(),
            module_path: vec!["lib".into()],
        },
        declaration_name: name.into(),
        kind: SemanticSourceTargetKind::Function,
        scope_discriminant: None,
        declaration_span: HirSourceSpan::new(ordinal * 100, ordinal * 100 + 99),
    }
}

/// Build a governed template with a stable identity so codec and body-audit tests retain executable statements.
/// Tests of compiled contracts explicitly clear this fixture binder.
fn body(name: &str, ordinal: usize) -> Body {
    let identity = identity(name, ordinal);
    Body {
        nominal_instantiation: Vec::new(),
        trait_default_instantiation: None,
        type_parameters: vec!["Fixture".into()],
        decl_id: CompilerNodeId::declaration_span(
            "lib",
            identity.declaration_span.start,
            identity.declaration_span.end,
        ),
        direct_call_id: CompilerNodeId::declaration_span(
            "lib",
            identity.declaration_span.start,
            identity.declaration_span.end,
        ),
        canonical: Some(identity.clone()),
        name: name.into(),
        span: identity.declaration_span,
        return_type: IncanType::Primitive(IncanPrimitiveType::Unit),
        checked_input_profile: None,
        explicit_input_bindings: Vec::new(),
        returned_view: None,
        callable_representation: None,
        named_type_identities: Default::default(),
        locals: Vec::new(),
        params: Vec::new(),
        param_locals: Vec::new(),
        scopes: Vec::new(),
        block: Block {
            scope: ScopeId(0),
            stmts: Vec::new(),
        },
        runtime_requirements: Vec::new(),
        panic_facts: Vec::new(),
        is_async: false,
        retired_rust_facade_wire: None,
        native_attributes: Vec::new(),
    }
}

/// Synthetic module context used only for codec and public-closure invariants.
pub(super) fn module(bodies: Vec<Body>) -> BodyIrModule {
    BodyIrModule {
        runtime_callable_bindings: Vec::new(),
        decorated_bindings: Vec::new(),
        entrypoint_effects: Vec::new(),
        program_entrypoints: Vec::new(),
        string_conversion_contract: crate::body_ir::StringConversionContract::Unselected,
        constant_declarations: Vec::new(),
        type_alias_declarations: Vec::new(),
        trait_declarations: Vec::new(),
        trait_implementations: Vec::new(),
        module_id: CompilerNodeId::module("lib"),
        loaf_callables: Vec::new(),
        native_union_declarations: Vec::new(),
        enum_declarations: Vec::new(),
        static_declarations: Vec::new(),
        nominal_declarations: Vec::new(),
        fieldless_enum_declarations: Vec::new(),
        value_enum_declarations: Vec::new(),
        bodies,
    }
}

/// Build with an explicit public set, as the finalized manifest supplies at the real producer seam.
fn publish(
    module: &BodyIrModule,
    public: &BTreeSet<CanonicalSymbolId>,
) -> Result<Vec<u8>, ExecutableRepresentationError> {
    build_surface(std::slice::from_ref(module), "probe", "1.2.3", public, &BTreeSet::new())
}

/// A canonical call participates in closure even before any branch is executed.
fn call(target: CanonicalSymbolId) -> Statement {
    Statement {
        span: target.declaration_span,
        kind: StatementKind::Call {
            destination: None,
            callee: Callee::Function(CallableTarget::Named(NamedCallableTarget {
                receiver_type: None,
                name: "arbitrary_alias".into(),
                direct_call_id: None,
                builtin: None,
                type_args: Vec::new(),
                binding: ArgumentBinding::UnresolvedPositional,
                canonical: Some(target),
            })),
            args: Vec::new(),
            may_panic: false,
        },
    }
}

/// Build the generic Holder layout used to test parameter scope and lazy protocol dependencies together.
fn generic_holder_declaration(
    owner: CanonicalSymbolId,
    field: CanonicalSymbolId,
) -> crate::body_ir::NominalDeclaration {
    crate::body_ir::NominalDeclaration {
        trait_adoptions: Vec::new(),
        native_item: String::new(),
        class_inheritance: None,
        native_type_arguments: Vec::new(),
        direct_declaration_id: CompilerNodeId::declaration_span("lib", 200, 499),
        canonical: owner,
        name: "Holder".into(),
        fields: vec!["value".into()],
        field_identities: vec![field],
        field_aliases: Vec::new(),
        field_types: vec![IncanType::TypeVar("T".into())],
        owned_callable_fields: Vec::new(),
        field_public: vec![true],
        reflection_fields: None,
        display_method: None,
        newtype_deserialize: crate::body_ir::NewtypeDeserialize::Unselected,
        newtype_string_conversion: crate::body_ir::NewtypeStringConversion::Unselected,
        reflection_metadata: None,
        comparison_equality: None,
        public: true,
        has_field_defaults: false,
        field_default_body: None,
        method_partial_defaults: Vec::new(),
        derives: Vec::new(),
        named_type_identities: Default::default(),
        type_parameters: vec!["T".into()],
        type_parameter_count: 1,
    }
}
