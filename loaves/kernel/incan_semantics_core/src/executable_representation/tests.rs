//! Tests for module encoding, surface coverage and published declaration contexts.

use std::collections::{BTreeMap, BTreeSet};

use super::{
    CoverageReason, DeclarationCoverage, EXECUTABLE_REPRESENTATION_VERSION, ExecutableDeclaration,
    ExecutableRepresentationError, MAX_SURFACE_INDEX_BYTES, OLDEST_SUPPORTED_VERSION, SurfaceIndex, SurfaceReader,
    build_surface, decode_module, decode_owner, encode_module, encode_owner, publication, require_supported_version,
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

#[test]
fn executable_owner_frame_round_trips_the_native_body_and_provenance() -> Result<(), Box<dyn std::error::Error>> {
    let owner = body("retained", 4);
    let module = CompilerNodeId::module("probe::lib");
    let encoded = encode_owner(&module, "src/lib.incn", "function:probe::lib::retained", &owner)?;
    let decoded = decode_owner(&encoded)?;

    assert_eq!(decoded.version, EXECUTABLE_REPRESENTATION_VERSION);
    assert_eq!(decoded.module, module);
    assert_eq!(decoded.source, "src/lib.incn");
    assert_eq!(decoded.identity, "function:probe::lib::retained");
    assert_eq!(decoded.body, owner);
    assert_eq!(
        encode_owner(&decoded.module, &decoded.source, &decoded.identity, &decoded.body)?,
        encoded
    );
    Ok(())
}

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
        record_package_identity: None,
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
        validated_constructor: None,
        method_partial_defaults: Vec::new(),
        derives: Vec::new(),
        named_type_identities: Default::default(),
        type_parameters: vec!["T".into()],
        type_parameter_count: 1,
    }
}

/// Current Body IR retains record metadata byte for byte and validates its identity from the earliest name.
#[test]
fn field_record_metadata_round_trips_and_is_revalidated() -> Result<(), Box<dyn std::error::Error>> {
    let mut owner = identity("Holder", 2);
    owner.origin = SymbolOrigin::Module(vec!["lib".into()]);
    owner.kind = SemanticSourceTargetKind::Model;
    owner.declaration_span = HirSourceSpan::new(200, 499);
    let mut field = identity("value", 3);
    field.origin = SymbolOrigin::Module(vec!["lib".into()]);
    field.kind = SemanticSourceTargetKind::Field;
    field.namespace = SymbolNamespace::Member;
    let mut declaration = generic_holder_declaration(owner, field);
    declaration.record_package_identity = Some(crate::record_identity::RecordPackageIdentity {
        registry_id: "sha256:3d7781b11f6b80f950bab17238cffa7c1f77b5e1f0e0dbbaa656149d9ffbffb0".to_owned(),
        scoped_name: "example/catalog".to_owned(),
    });
    declaration.reflection_fields = Some(vec![0]);
    declaration.reflection_metadata = Some(vec![crate::body_ir::FieldMetadata {
        position: 0,
        name: "value".into(),
        record_identity: crate::record_identity::record_member_identity("old_value").to_vec(),
        record_canonical: false,
        renamed_from: vec!["old_value".into()],
        renamed_from_lifecycle: Some(crate::body_ir::LifecycleMetadata {
            since: Some("0.6".into()),
            retire_in: None,
            message: Some("".into()),
            by: None,
            issue: Some("migration".into()),
        }),
        default_is_empty: false,
        alias: Some("wireValue".into()),
        description: Some("Retained value".into()),
        type_name: "T".into(),
        has_default: false,
    }]);
    let mut module = module(Vec::new());
    module.nominal_declarations.push(declaration.clone());
    assert!(module.is_well_formed_nominal_declaration(&declaration));
    let decoded = decode_module(&encode_module(&module)?)?;
    assert_eq!(decoded, module);

    let mut invalid_namespace = declaration.clone();
    if let Some(package) = &mut invalid_namespace.record_package_identity {
        package.registry_id = "sha256:bad".to_owned();
    }
    assert!(!module.is_well_formed_nominal_declaration(&invalid_namespace));
    invalid_namespace = declaration.clone();
    invalid_namespace.canonical.origin = SymbolOrigin::Package {
        library: "consumer/alias".to_owned(),
        module_path: vec!["lib".to_owned()],
    };
    assert!(!module.is_well_formed_nominal_declaration(&invalid_namespace));

    for invalid_kind in 0..3 {
        let mut invalid = declaration.clone();
        let record = invalid
            .reflection_metadata
            .as_mut()
            .and_then(|records| records.first_mut())
            .ok_or("missing reflection record")?;
        match invalid_kind {
            0 => record.position = 1,
            1 => record.default_is_empty = true,
            _ => {
                record.renamed_from.clear();
                record.record_identity = crate::record_identity::record_member_identity("value").to_vec();
            }
        }
        assert!(!module.is_well_formed_nominal_declaration(&invalid));
    }

    let record = module
        .nominal_declarations
        .first_mut()
        .and_then(|declaration| declaration.reflection_metadata.as_mut())
        .and_then(|metadata| metadata.first_mut())
        .ok_or("missing reflection record")?;
    record.record_identity[0] ^= 255;
    let invalid = module
        .nominal_declarations
        .first()
        .cloned()
        .ok_or("missing nominal declaration")?;
    assert!(!module.is_well_formed_nominal_declaration(&invalid));
    Ok(())
}

/// Validated construction retains exact generated and source identities through the executable representation.
#[test]
fn validated_constructor_round_trips_and_rejects_another_generated_owner() -> Result<(), Box<dyn std::error::Error>> {
    let mut owner = identity("Checked", 2);
    owner.origin = SymbolOrigin::Module(vec!["lib".into()]);
    owner.kind = SemanticSourceTargetKind::Model;
    owner.declaration_span = HirSourceSpan::new(200, 499);
    let mut field = identity("value", 3);
    field.origin = owner.origin.clone();
    field.namespace = SymbolNamespace::Member;
    field.kind = SemanticSourceTargetKind::Field;
    field.declaration_span = HirSourceSpan::new(240, 260);
    let mut validator = identity("validate", 4);
    validator.origin = owner.origin.clone();
    validator.namespace = SymbolNamespace::Member;
    validator.kind = SemanticSourceTargetKind::Method;
    validator.declaration_span = HirSourceSpan::new(300, 399);
    let mut declaration = generic_holder_declaration(owner.clone(), field);
    declaration.name = "Checked".into();
    declaration.fields = vec!["value".into()];
    declaration.derives = vec![incan_lang::lang::derives::as_str(incan_lang::lang::derives::DeriveId::Validate).into()];
    declaration.validated_constructor = Some(crate::body_ir::ValidatedConstructor {
        constructor: owner.generated_member(
            incan_lang::lang::conventions::NEW_METHOD,
            SemanticSourceTargetKind::Method,
        ),
        validator,
        required_fields: vec![0],
    });
    let mut module = module(Vec::new());
    module.nominal_declarations.push(declaration.clone());
    assert!(module.is_well_formed_nominal_declaration(&declaration));
    assert_eq!(decode_module(&encode_module(&module)?)?, module);

    let contract = declaration
        .validated_constructor
        .as_mut()
        .ok_or("missing validated constructor")?;
    contract.constructor.declaration_span = HirSourceSpan::new(500, 599);
    assert!(!module.is_well_formed_nominal_declaration(&declaration));
    Ok(())
}
