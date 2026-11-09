//! Published alias, constant, trait, enum, nominal and type-context round trips.

use super::*;

/// A public scalar union alias retains its expanded semantic type without introducing a runtime body.
#[test]
fn public_union_alias_context_round_trips() -> Result<(), Box<dyn std::error::Error>> {
    let mut identity = identity("Answer", 2);
    identity.kind = SemanticSourceTargetKind::TypeAlias;
    let ty = IncanType::Generic {
        base: incan_lang::lang::types::UNION_TYPE_NAME.into(),
        args: vec![
            IncanType::Primitive(IncanPrimitiveType::Int),
            IncanType::Primitive(IncanPrimitiveType::Str),
        ],
    };
    let mut module = module(Vec::new());
    module
        .type_alias_declarations
        .push(crate::body_ir::TypeAliasDeclaration {
            canonical: identity.clone(),
            ty: ty.clone(),
            named_type_identities: BTreeMap::new(),
        });
    let bytes = publish(&module, &BTreeSet::from([identity.clone()]))?;
    let reader = SurfaceReader::open(&bytes)?;
    assert!(matches!(reader.fragment(&identity)?, ExecutableDeclaration::TypeAlias(value) if value.ty == ty));
    assert!(reader.declaration(&identity).is_err());
    Ok(())
}

/// Public constants carry their checked immutable value and do not disclose a private sibling value.
#[test]
fn public_constant_fragments_preserve_checked_values() -> Result<(), Box<dyn std::error::Error>> {
    let mut exported = identity("BASE", 2);
    exported.kind = SemanticSourceTargetKind::Const;
    let mut private = identity("SECRET", 3);
    private.kind = SemanticSourceTargetKind::Const;
    let mut module = module(Vec::new());
    module.constant_declarations = vec![
        crate::body_ir::ConstantDeclaration {
            canonical: exported.clone(),
            value: crate::body_ir::Constant::Int(42),
        },
        crate::body_ir::ConstantDeclaration {
            canonical: private,
            value: crate::body_ir::Constant::Str("private-value".into()),
        },
    ];
    let bytes = publish(&module, &BTreeSet::from([exported.clone()]))?;
    let reader = SurfaceReader::open(&bytes)?;
    assert!(
        matches!(reader.fragment(&exported)?, ExecutableDeclaration::Constant(value) if value.value == crate::body_ir::Constant::Int(42))
    );
    assert!(
        !bytes
            .windows(b"private-value".len())
            .any(|part| part == b"private-value")
    );
    Ok(())
}

/// Abstract trait slots resolve through their public declaring trait without inventing callable bodies.
#[test]
fn public_abstract_trait_slot_resolves_to_trait_context() -> Result<(), Box<dyn std::error::Error>> {
    let mut owner = identity("Reading", 2);
    owner.kind = SemanticSourceTargetKind::Trait;
    let mut slot = identity("get", 3);
    slot.kind = SemanticSourceTargetKind::Method;
    slot.namespace = SymbolNamespace::Member;
    slot.declaration_span = HirSourceSpan::new(210, 225);
    let mut module = module(Vec::new());
    module.trait_declarations.push(owner.clone());
    let bytes = publish(&module, &BTreeSet::from([owner.clone(), slot.clone()]))?;
    let reader = SurfaceReader::open(&bytes)?;
    assert!(matches!(reader.fragment(&owner)?, ExecutableDeclaration::Trait(value) if value == owner));
    assert!(
        matches!(reader.index().coverage(&slot)?, DeclarationCoverage::TypeContext { owner: selected } if selected == &owner)
    );
    assert!(reader.declaration(&slot).is_err());
    Ok(())
}

/// An executable trait method retains its public declaring owner even when its body names no other slot.
#[test]
fn public_default_trait_body_requires_its_declaring_context() -> Result<(), Box<dyn std::error::Error>> {
    let mut owner = identity("Reading", 2);
    owner.kind = SemanticSourceTargetKind::Trait;
    let mut slot = identity("touch", 3);
    slot.kind = SemanticSourceTargetKind::Method;
    slot.namespace = SymbolNamespace::Member;
    slot.declaration_span = HirSourceSpan::new(210, 225);
    let mut default = body("touch", 3);
    default.canonical = Some(slot.clone());
    default.span = slot.declaration_span;
    default.return_type = IncanType::SelfType;
    let mut source = module(vec![default]);
    source.trait_declarations.push(owner.clone());
    let bytes = publish(&source, &BTreeSet::from([owner.clone(), slot.clone()]))?;
    let reader = SurfaceReader::open(&bytes)?;
    let DeclarationCoverage::Covered { requirements, .. } = reader.index().coverage(&slot)? else {
        return Err("default method has no executable coverage".into());
    };
    assert_eq!(requirements, &[owner.clone()]);
    assert!(matches!(reader.fragment(&owner)?, ExecutableDeclaration::Trait(value) if value == owner));
    let private_owner = publish(&source, &BTreeSet::from([slot.clone()]))?;
    let private_reader = SurfaceReader::open(&private_owner)?;
    assert!(private_reader.covers(&slot));
    assert!(private_reader.index().implementation.contains_key(&owner));
    assert!(private_reader.fragment(&owner).is_err());
    Ok(())
}

/// Normal enum payload layouts round-trip through public fragments, and their variants resolve to the owner.
#[test]
fn public_normal_enum_payload_context_round_trips() -> Result<(), Box<dyn std::error::Error>> {
    let mut owner = identity("Signal", 2);
    owner.kind = SemanticSourceTargetKind::Enum;
    let mut variant = identity("Ready", 3);
    variant.kind = SemanticSourceTargetKind::Variant;
    variant.namespace = SymbolNamespace::Member;
    let value = crate::body_ir::EnumDeclaration {
        native_item: String::new(),
        native_type_arguments: Vec::new(),
        direct_declaration_id: CompilerNodeId::declaration_span("lib", 200, 299),
        canonical: owner.clone(),
        name: "Signal".into(),
        public: true,
        type_parameters: Vec::new(),
        variants: vec![crate::body_ir::EnumVariantDeclaration {
            direct_declaration_id: CompilerNodeId::declaration_span("lib", 300, 399),
            canonical: variant.clone(),
            name: "Ready".into(),
            fields: vec![IncanType::Primitive(IncanPrimitiveType::Int)],
        }],
        derives: Vec::new(),
        named_type_identities: BTreeMap::new(),
    };
    let mut module = module(Vec::new());
    module.enum_declarations.push(value);
    let bytes = publish(&module, &BTreeSet::from([owner.clone(), variant.clone()]))?;
    let reader = SurfaceReader::open(&bytes)?;
    assert!(
        matches!(reader.fragment(&owner)?, ExecutableDeclaration::Enum(value) if value.variants[0].fields == vec![IncanType::Primitive(IncanPrimitiveType::Int)])
    );
    assert!(
        matches!(reader.index().coverage(&variant)?, DeclarationCoverage::TypeContext { owner: selected } if selected == &owner)
    );
    let bytes = publish(&module, &BTreeSet::from([owner.clone()]))?;
    let reader = SurfaceReader::open(&bytes)?;
    assert!(reader.covers(&owner));
    assert!(reader.index().coverage(&variant).is_err());
    assert!(reader.fragment(&variant).is_err());
    Ok(())
}

/// Declared function binders remain portable; an undeclared variable cannot enter a published body.
#[test]
fn published_functions_cover_only_declared_type_parameters() -> Result<(), Box<dyn std::error::Error>> {
    let mut declared = body("identity", 1);
    declared.type_parameters = vec!["T".into()];
    declared.return_type = IncanType::TypeVar("T".into());
    let mut undeclared = body("unbound", 2);
    undeclared.return_type = IncanType::TypeVar("T".into());
    let bytes = publish(
        &module(vec![declared, undeclared]),
        &BTreeSet::from([identity("identity", 1), identity("unbound", 2)]),
    )?;
    let reader = SurfaceReader::open(&bytes)?;
    assert!(reader.covers(&identity("identity", 1)));
    assert!(matches!(
        reader.index().coverage(&identity("unbound", 2))?,
        DeclarationCoverage::Uncovered(CoverageReason::UnresolvedReference)
    ));
    Ok(())
}

/// A generic nominal layout and its receiver method share the owner's binders, with canonical use requirements.
#[test]
fn published_nominals_cover_their_declared_parameters() -> Result<(), Box<dyn std::error::Error>> {
    let mut owner = identity("Holder", 2);
    owner.kind = SemanticSourceTargetKind::Model;
    owner.declaration_span = HirSourceSpan::new(200, 499);
    let mut field = identity("value", 3);
    field.kind = SemanticSourceTargetKind::Field;
    field.namespace = SymbolNamespace::Member;
    let mut method = body("get", 4);
    let mut method_id = identity("get", 4);
    method_id.kind = SemanticSourceTargetKind::Method;
    method_id.namespace = SymbolNamespace::Member;
    method.canonical = Some(method_id.clone());
    method.return_type = IncanType::TypeVar("T".into());
    let mut user = body("use_holder", 1);
    user.return_type = IncanType::Generic {
        base: "Holder".into(),
        args: vec![IncanType::Primitive(IncanPrimitiveType::Int)],
    };
    let mut module = module(vec![method, user]);
    module
        .nominal_declarations
        .push(generic_holder_declaration(owner.clone(), field.clone()));
    let public = BTreeSet::from([owner.clone(), field, method_id.clone(), identity("use_holder", 1)]);
    let bytes = publish(&module, &public)?;
    let reader = SurfaceReader::open(&bytes)?;
    assert!(reader.covers(&owner));
    assert!(reader.covers(&method_id));
    let DeclarationCoverage::Covered { requirements, .. } = reader.index().coverage(&identity("use_holder", 1))? else {
        return Err("generic nominal use is uncovered".into());
    };
    assert!(requirements.contains(&owner));
    // An uncalled lazy protocol default contributes a dispatch relation, not an executable layout dependency.
    let mut protocol = identity("UnusedProtocol", 6);
    protocol.kind = SemanticSourceTargetKind::Trait;
    let mut slot = identity("unavailable_default", 7);
    slot.kind = SemanticSourceTargetKind::Method;
    slot.namespace = SymbolNamespace::Member;
    slot.declaration_span = HirSourceSpan::new(610, 625);
    let mut unavailable = body("unavailable_default", 7);
    unavailable.canonical = Some(slot.clone());
    unavailable.span = slot.declaration_span;
    unavailable.block.stmts.push(Statement {
        kind: StatementKind::Unsupported {
            description: "unavailable default".into(),
        },
        span: slot.declaration_span,
    });
    module.trait_declarations.push(protocol.clone());
    module.bodies.push(unavailable);
    module.nominal_declarations[0]
        .trait_adoptions
        .push(crate::body_ir::TraitAdoption {
            trait_identity: protocol.clone(),
            arguments: Vec::new(),
            methods: Some(vec![slot.clone()]),
            primitive_adapter: None,
            fields: Vec::new(),
        });
    module.trait_implementations.push(crate::body_ir::TraitImplementation {
        owner: owner.clone(),
        method: slot.clone(),
        implementation: slot.clone(),
        slot_arguments: Vec::new(),
        type_arguments: Vec::new(),
    });
    let mut public = public;
    public.extend([protocol, slot.clone()]);
    let bytes = publish(&module, &public)?;
    let reader = SurfaceReader::open(&bytes)?;
    assert!(
        reader.covers(&owner),
        "unused default must not invalidate its adopter layout"
    );
    assert!(matches!(
        reader.index().coverage(&slot)?,
        DeclarationCoverage::Uncovered(CoverageReason::UnsupportedConstruct)
    ));
    let mut caller = body("requests_default", 8);
    caller.block.stmts.push(call(slot.clone()));
    module.bodies.push(caller);
    public.insert(identity("requests_default", 8));
    let bytes = publish(&module, &public)?;
    let reader = SurfaceReader::open(&bytes)?;
    assert!(
        !reader.covers(&identity("requests_default", 8)),
        "calling the unavailable default must still refuse"
    );
    let mut missing = module.clone();
    missing.trait_implementations[0].implementation = identity("missing_implementation", 9);
    let bytes = publish(&missing, &public)?;
    assert!(
        !SurfaceReader::open(&bytes)?.covers(&owner),
        "an actual missing implementation still invalidates its layout"
    );
    Ok(())
}

/// Class fields resolve through a covered class; a field cannot use an unrelated declaration kind as its owner.
#[test]
fn public_class_field_context_is_admitted() -> Result<(), Box<dyn std::error::Error>> {
    let mut owner = identity("Counter", 2);
    owner.kind = SemanticSourceTargetKind::Class;
    let mut field = identity("value", 3);
    field.kind = SemanticSourceTargetKind::Field;
    let index = super::SurfaceIndex {
        library: "probe".into(),
        package_version: "0.1.0".into(),
        implementation: BTreeMap::new(),
        declarations: BTreeMap::from([
            (
                owner.clone(),
                DeclarationCoverage::Covered {
                    offset: 0,
                    length: 1,
                    requirements: Vec::new(),
                },
            ),
            (field, DeclarationCoverage::TypeContext { owner }),
        ]),
    };
    index.validate(1)?;
    Ok(())
}

/// Complete private fields support execution while absent checked layouts keep public callers uncovered.
#[test]
fn public_type_context_retains_private_fields_and_requires_checked_layouts() -> Result<(), Box<dyn std::error::Error>> {
    let mut type_id = identity("Record", 2);
    type_id.kind = SemanticSourceTargetKind::Model;
    let mut field = identity("private_field", 3);
    field.kind = SemanticSourceTargetKind::Field;
    field.namespace = SymbolNamespace::Member;
    let mut exported = body("exported", 1);
    exported.return_type = IncanType::Named("Record".into());
    let mut module = module(vec![exported]);
    module.nominal_declarations.push(crate::body_ir::NominalDeclaration {
        trait_adoptions: Vec::new(),
        native_item: String::new(),
        class_inheritance: None,
        native_type_arguments: Vec::new(),
        direct_declaration_id: CompilerNodeId::declaration_span("lib", 200, 299),
        canonical: type_id.clone(),
        name: "Record".into(),
        fields: vec!["private_field".into()],
        field_identities: vec![field.clone()],
        field_types: vec![IncanType::Primitive(IncanPrimitiveType::Int)],
        owned_callable_fields: Vec::new(),
        field_public: vec![false],
        field_aliases: Vec::new(),
        reflection_fields: None,
        reflection_metadata: None,
        comparison_equality: None,
        public: false,
        has_field_defaults: false,
        field_default_body: None,
        method_partial_defaults: Vec::new(),
        display_method: None,
        newtype_deserialize: crate::body_ir::NewtypeDeserialize::Unselected,
        newtype_string_conversion: crate::body_ir::NewtypeStringConversion::Unselected,
        derives: vec![],
        named_type_identities: Default::default(),
        type_parameters: Vec::new(),
        type_parameter_count: 0,
    });
    let public = BTreeSet::from([identity("exported", 1), type_id.clone()]);
    let bytes = publish(&module, &public)?;
    let reader = SurfaceReader::open(&bytes)?;
    assert!(reader.covers(&identity("exported", 1)));
    assert!(reader.covers(&type_id));
    assert!(reader.index().coverage(&field).is_err());
    assert!(reader.fragment(&field).is_err());
    let mut complete_public = public;
    complete_public.insert(field.clone());
    let bytes = publish(&module, &complete_public)?;
    let reader = SurfaceReader::open(&bytes)?;
    assert!(reader.covers(&identity("exported", 1)));
    assert!(reader.covers(&type_id));
    assert!(
        matches!(reader.index().coverage(&field)?, DeclarationCoverage::TypeContext { owner } if owner == &type_id)
    );
    let nominal = module.nominal_declarations.first_mut().ok_or("public model absent")?;
    nominal.field_types[0] = IncanType::Named("private_layout_secret".into());
    let mut private_type = identity("private_layout_secret", 4);
    private_type.kind = SemanticSourceTargetKind::Model;
    nominal
        .named_type_identities
        .insert("private_layout_secret".into(), private_type);
    let bytes = publish(&module, &complete_public)?;
    assert!(!SurfaceReader::open(&bytes)?.covers(&type_id));
    assert!(
        !bytes
            .windows(b"private_layout_secret".len())
            .any(|part| part == b"private_layout_secret")
    );
    Ok(())
}

/// Foreign type bindings retain distinct declared origins, while unused private checker bindings stay absent.
#[test]
fn checked_nominal_type_bindings_are_pruned_and_contribute_public_requirements()
-> Result<(), Box<dyn std::error::Error>> {
    let mut first = identity("Product", 2);
    first.kind = SemanticSourceTargetKind::Model;
    first.origin = SymbolOrigin::Package {
        library: "catalog".into(),
        module_path: vec!["lib".into()],
    };
    let mut second = first.clone();
    second.origin = SymbolOrigin::Package {
        library: "other".into(),
        module_path: vec!["lib".into()],
    };
    let mut exported = body("exported", 1);
    exported.return_type = IncanType::Tuple(vec![
        IncanType::Named("First".into()),
        IncanType::Named("Second".into()),
    ]);
    exported.params.push(CallableParam {
        mutable: false,
        input_access: crate::IncanInputAccess::Inferred,
        native_owned: false,
        kind: crate::IncanCallableParamKind::Normal,
        local: crate::body_ir::LocalId(0),
        name: "product".into(),
        ty: IncanType::Named("First".into()),
        span: exported.span,
        default: CallableParamDefault::Required,
    });
    exported.named_type_identities = std::collections::BTreeMap::from([
        ("First".into(), first.clone()),
        ("Second".into(), second.clone()),
        ("unused_private_secret".into(), identity("unused_private_secret", 3)),
    ])
    .into();
    let bytes = publish(&module(vec![exported]), &BTreeSet::from([identity("exported", 1)]))?;
    let reader = SurfaceReader::open(&bytes)?;
    let decoded = reader.declaration(&identity("exported", 1))?;
    assert_eq!(
        decoded.named_type_identities,
        std::collections::BTreeMap::from([("First".into(), first.clone()), ("Second".into(), second.clone())]).into()
    );
    let DeclarationCoverage::Covered { requirements, .. } = reader.index().coverage(&identity("exported", 1))? else {
        return Err("foreign typed body uncovered".into());
    };
    assert_eq!(
        requirements.iter().cloned().collect::<BTreeSet<_>>(),
        BTreeSet::from([first, second])
    );
    assert!(
        !bytes
            .windows(b"unused_private_secret".len())
            .any(|part| part == b"unused_private_secret")
    );
    Ok(())
}

/// Every retained Result type identity is public-audited and rebased to the declaring package.
#[test]
fn result_type_identities_are_rebased_and_private_identity_payloads_are_not_published()
-> Result<(), Box<dyn std::error::Error>> {
    use crate::body_ir::{Constant, LocalId, Operand, Place, ResultVariant, ResultVariantKind, Rvalue};
    let mut published_type = identity("Record", 2);
    published_type.kind = SemanticSourceTargetKind::Model;
    let mut source_type = published_type.clone();
    source_type.origin = SymbolOrigin::Module(vec!["lib".into()]);
    let mut exported = body("exported", 1);
    exported.block.stmts.push(Statement {
        span: exported.span,
        kind: StatementKind::Assign {
            place: Place::from_local(LocalId(0)),
            rvalue: Rvalue::ResultVariant(ResultVariant {
                kind: ResultVariantKind::Ok,
                payload: Operand::Constant(Constant::Unit),
                ok_type: IncanType::Named("Record".into()),
                error_type: IncanType::Primitive(IncanPrimitiveType::Unit),
                canonical_types: std::collections::BTreeMap::from([(vec![0], source_type.clone())]),
            }),
        },
    });
    let mut input = module(vec![exported]);
    input.nominal_declarations.push(crate::body_ir::NominalDeclaration {
        trait_adoptions: Vec::new(),
        native_item: String::new(),
        class_inheritance: None,
        native_type_arguments: Vec::new(),
        direct_declaration_id: CompilerNodeId::declaration_span("lib", 200, 299),
        canonical: published_type.clone(),
        name: "Record".into(),
        fields: vec![],
        field_identities: vec![],
        field_types: vec![],
        owned_callable_fields: Vec::new(),
        field_public: vec![],
        field_aliases: Vec::new(),
        reflection_fields: None,
        reflection_metadata: None,
        comparison_equality: None,
        public: false,
        has_field_defaults: false,
        field_default_body: None,
        method_partial_defaults: Vec::new(),
        display_method: None,
        newtype_deserialize: crate::body_ir::NewtypeDeserialize::Unselected,
        newtype_string_conversion: crate::body_ir::NewtypeStringConversion::Unselected,
        derives: vec![],
        named_type_identities: Default::default(),
        type_parameters: Vec::new(),
        type_parameter_count: 0,
    });
    let public = BTreeSet::from([identity("exported", 1), published_type.clone()]);
    let bytes = publish(&input, &public)?;
    let decoded = SurfaceReader::open(&bytes)?.declaration(&identity("exported", 1))?;
    let Some(Statement {
        kind: StatementKind::Assign {
            rvalue: Rvalue::ResultVariant(variant),
            ..
        },
        ..
    }) = decoded.block.stmts.first()
    else {
        return Err("Result type context disappeared".into());
    };
    assert_eq!(variant.canonical_types.get(&vec![0]), Some(&published_type));
    let Some(Statement {
        kind: StatementKind::Assign {
            rvalue: Rvalue::ResultVariant(variant),
            ..
        },
        ..
    }) = input.bodies.first_mut().and_then(|body| body.block.stmts.first_mut())
    else {
        return Err("source Result disappeared".into());
    };
    let mut private = identity("private_type_secret", 3);
    private.kind = SemanticSourceTargetKind::Model;
    variant.canonical_types.insert(vec![0], private);
    let bytes = publish(&input, &public)?;
    assert!(!SurfaceReader::open(&bytes)?.covers(&identity("exported", 1)));
    assert!(
        !bytes
            .windows(b"private_type_secret".len())
            .any(|bytes| bytes == b"private_type_secret")
    );
    Ok(())
}

/// Local binding identity is checker-session metadata; fragment-local slots retain execution identity instead.
#[test]
fn publication_removes_session_local_binding_discriminants() -> Result<(), Box<dyn std::error::Error>> {
    let mut exported = body("exported", 1);
    let mut local_identity = identity("local", 2);
    local_identity.scope_discriminant = Some(crate::ScopeDiscriminant(917));
    exported.locals.push(crate::body_ir::LocalDecl {
        id: crate::body_ir::LocalId(0),
        name: Some("local".into()),
        identity: Some(Box::new(local_identity)),
        ty: IncanType::Primitive(IncanPrimitiveType::Int),
        origin: crate::body_ir::LocalOrigin::UserBinding,
        scope: ScopeId(0),
        span: exported.span,
    });
    let bytes = publish(&module(vec![exported]), &BTreeSet::from([identity("exported", 1)]))?;
    let decoded = SurfaceReader::open(&bytes)?.declaration(&identity("exported", 1))?;
    assert_eq!(decoded.locals.first().ok_or("local slot disappeared")?.identity, None);
    assert_eq!(
        decoded.locals.first().ok_or("local slot disappeared")?.id,
        crate::body_ir::LocalId(0)
    );
    Ok(())
}

/// The frame's own layout is pinned, because a second reader parses it by hand.
///
/// `SurfaceReader` reads the frame from a slice with postcard; the compiler's streaming reader cannot, and
/// reimplements postcard's varint framing to find where the version ends before it seeks. Nothing else
/// connects those two implementations, so this asserts the shape both rely on: a postcard `u32` version whose
/// encoding uses the 0x80 continuation bit, then an 8-byte little-endian index length, then the index, then
/// the payload. If postcard ever changes its integer encoding, this test fails here rather than on a real
/// artifact in the one reader that has no test of its own.
#[test]
fn the_frame_layout_the_streaming_reader_reimplements_is_pinned() -> Result<(), Box<dyn std::error::Error>> {
    let module = module(vec![body("exported", 1)]);
    let bytes = publish(&module, &BTreeSet::from([identity("exported", 1)]))?;

    // The hand-rolled reader consumes version bytes while the continuation bit is set, at most five.
    let version_length = bytes
        .iter()
        .take(5)
        .position(|byte| byte & 128 == 0)
        .ok_or("version prefix is not postcard varint framed")?
        + 1;
    let (decoded_version, remainder) = postcard::take_from_bytes::<u32>(&bytes)?;
    assert_eq!(
        decoded_version,
        super::EXECUTABLE_REPRESENTATION_VERSION,
        "the frame must open with the representation version"
    );
    assert_eq!(
        bytes.len() - remainder.len(),
        version_length,
        "the hand-rolled varint scan and postcard must agree on where the version ends"
    );

    // Then eight little-endian bytes of index length, and the index must land inside the file.
    let length_bytes: [u8; 8] = remainder.get(..8).ok_or("missing index length")?.try_into()?;
    let index_length = u64::from_le_bytes(length_bytes);
    assert!(index_length > 0, "a published surface carries an index");
    assert!(
        index_length <= super::MAX_SURFACE_INDEX_BYTES,
        "a published index must fall inside the bound both readers enforce"
    );
    let payload_offset = u64::try_from(version_length)? + 8 + index_length;
    assert!(
        payload_offset <= u64::try_from(bytes.len())?,
        "the streaming reader seeks to this offset; it must be inside the file"
    );

    // And both readers must agree on what the index says.
    let reader = SurfaceReader::open(&bytes)?;
    let decoded = super::SurfaceIndex::decode(
        remainder
            .get(8..usize::try_from(8 + index_length)?)
            .ok_or("index truncated")?,
        u64::try_from(bytes.len())? - payload_offset,
    )?;
    assert_eq!(
        decoded.declarations.len(),
        reader.index().declarations.len(),
        "parsing the frame by offset and through the reader must yield the same index"
    );
    Ok(())
}
