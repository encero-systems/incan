//! Version refusal, public membership, dependency coverage and fragment index tests.

use super::*;

/// Publication rebases the profile's proven owner together with the body, without repairing a wrong owner.
#[test]
fn input_profile_publication_retains_canonical_owner() -> Result<(), Box<dyn std::error::Error>> {
    let mut source = body("input", 1);
    source.type_parameters.clear();
    source.canonical = Some(CanonicalSymbolId::module_declaration(
        vec!["lib".into()],
        "input",
        SemanticSourceTargetKind::Function,
        source.span,
    ));
    let owner = source.canonical.clone().ok_or("missing source owner")?;
    source.checked_input_profile = Some(crate::body_ir::CheckedInputProfile {
        result_requirements: Vec::new(),
        version: 1,
        owner: owner.clone(),
        parameters: Vec::new(),
        owned_initializers: Vec::new(),
    });
    source
        .explicit_input_bindings
        .push(crate::body_ir::ExplicitInputBinding {
            declaration: source.span,
            identity: Some(identity("view", 3)),
            access: crate::IncanInputAccess::Shared,
        });
    let context = module(vec![source.clone()]);
    let eligible = BTreeSet::from([identity("input", 1)]);
    let projected =
        publication::project_body(&source, &context, "probe", &eligible).map_err(|error| format!("{error:?}"))?;
    // The obligation is published; the session-local binding identity is not.
    assert!(matches!(
        projected.body.explicit_input_bindings.as_slice(),
        [crate::body_ir::ExplicitInputBinding { identity: None, .. }]
    ));
    assert_eq!(
        projected
            .body
            .checked_input_profile
            .as_ref()
            .map(|profile| &profile.owner),
        projected.body.canonical.as_ref()
    );
    let (frame, _) = publication::project_compiled_frame(&source, &context, "probe", &eligible)
        .map_err(|error| format!("{error:?}"))?;
    let ExecutableDeclaration::CompiledCallableFrame { frame, .. } = frame else {
        return Err("expected compiled frame".into());
    };
    assert_eq!(
        frame.checked_input_profile.as_ref().map(|profile| &profile.owner),
        frame.canonical.as_ref()
    );
    assert!(matches!(
        frame.explicit_input_bindings.as_slice(),
        [crate::body_ir::ExplicitInputBinding { identity: None, .. }]
    ));
    source.checked_input_profile.as_mut().ok_or("missing profile")?.owner = identity("other", 2);
    assert!(publication::project_body(&source, &context, "probe", &eligible).is_err());
    assert!(publication::project_compiled_frame(&source, &context, "probe", &eligible).is_err());
    for obsolete in [36_u32, 37, 38, 39, 40, 41, 42, 43, 44] {
        assert!(require_supported_version(&postcard::to_allocvec(&obsolete)?).is_err());
    }
    Ok(())
}

/// Generic straight-line publication keeps symbolic result requirements; conditional ownership remains refused.
#[test]
fn generic_input_requirements_survive_publication() -> Result<(), Box<dyn std::error::Error>> {
    let mut source = body("input", 1);
    source.type_parameters = vec!["E".into()];
    source.canonical = Some(CanonicalSymbolId::module_declaration(
        vec!["lib".into()],
        "input",
        SemanticSourceTargetKind::Function,
        source.span,
    ));
    let owner = source.canonical.clone().ok_or("missing source owner")?;
    let required = crate::IncanType::Generic {
        base: "Result".into(),
        args: vec![
            crate::IncanType::Primitive(crate::IncanPrimitiveType::Unit),
            crate::IncanType::TypeVar("E".into()),
        ],
    };
    source.checked_input_profile = Some(crate::body_ir::CheckedInputProfile {
        result_requirements: vec![(source.span, required.clone())],
        version: 3,
        owner,
        parameters: Vec::new(),
        owned_initializers: Vec::new(),
    });
    let context = module(vec![source.clone()]);
    let eligible = BTreeSet::from([identity("input", 1)]);
    let projected =
        publication::project_body(&source, &context, "probe", &eligible).map_err(|error| format!("{error:?}"))?;
    let profile = projected
        .body
        .checked_input_profile
        .as_ref()
        .ok_or("missing published requirements")?;
    assert_eq!(profile.version, 3);
    assert_eq!(profile.result_requirements, vec![(source.span, required)]);
    source.checked_input_profile.as_mut().ok_or("missing profile")?.version = 4;
    assert!(publication::project_body(&source, &context, "probe", &eligible).is_err());
    Ok(())
}

/// Prior omitted-field layouts and future versions refuse before either reader attempts positional decoding.
#[test]
fn version_refusal_precedes_incompatible_header_or_module_decode() -> Result<(), Box<dyn std::error::Error>> {
    for version in [
        37,
        38,
        51,
        super::OLDEST_SUPPORTED_VERSION - 1,
        EXECUTABLE_REPRESENTATION_VERSION + 1,
    ] {
        let bytes = postcard::to_allocvec(&version)?;
        assert!(matches!(
            SurfaceReader::open(&bytes),
            Err(ExecutableRepresentationError::UnsupportedVersion { found, supported })
                if found == version && supported == EXECUTABLE_REPRESENTATION_VERSION
        ));
        assert!(matches!(
            decode_module(&bytes),
            Err(ExecutableRepresentationError::UnsupportedVersion { found, supported })
                if found == version && supported == EXECUTABLE_REPRESENTATION_VERSION
        ));
    }
    assert!(matches!(
        SurfaceReader::open(&[255; 12]),
        Err(ExecutableRepresentationError::Malformed { .. })
    ));
    Ok(())
}

/// A missing source field identity must not be mistaken for a compiler-proven structural projection.
#[test]
fn unresolved_named_fields_are_uncovered_and_synthesized_projections_remain_covered()
-> Result<(), Box<dyn std::error::Error>> {
    use crate::body_ir::{LocalId, Operand, OwnershipFact, Place, PlaceElem};
    let mut unresolved = body("unresolved", 1);
    let mut place = Place::from_local(LocalId(0));
    place.projection.push(PlaceElem::field("private_secret", None));
    unresolved.block.stmts.push(Statement {
        span: unresolved.span,
        kind: StatementKind::Return {
            value: Some(Operand::place(place.clone(), OwnershipFact::Copy, false)),
        },
    });
    let mut structural = body("structural", 2);
    place.projection = vec![PlaceElem::structural_field("0")];
    structural.block.stmts.push(Statement {
        span: structural.span,
        kind: StatementKind::Return {
            value: Some(Operand::place(place, OwnershipFact::Copy, false)),
        },
    });
    let bytes = publish(
        &module(vec![unresolved, structural]),
        &BTreeSet::from([identity("unresolved", 1), identity("structural", 2)]),
    )?;
    let reader = SurfaceReader::open(&bytes)?;
    assert!(matches!(
        reader.index().coverage(&identity("unresolved", 1))?,
        DeclarationCoverage::Uncovered(CoverageReason::UnresolvedReference)
    ));
    assert!(reader.covers(&identity("structural", 2)));
    assert!(
        !bytes
            .windows(b"private_secret".len())
            .any(|part| part == b"private_secret")
    );
    Ok(())
}

#[test]
fn public_membership_and_explicit_empty_body_coverage() -> Result<(), Box<dyn std::error::Error>> {
    let module = module(vec![body("public", 1), body("private_secret", 2)]);
    let public = BTreeSet::from([identity("public", 1), identity("unlowered", 3)]);
    let bytes = publish(&module, &public)?;
    let reader = SurfaceReader::open(&bytes)?;
    assert!(reader.covers(&identity("public", 1)));
    assert!(matches!(
        reader.index().coverage(&identity("unlowered", 3))?,
        DeclarationCoverage::Uncovered(CoverageReason::NoExecutableDeclaration)
    ));
    assert!(
        !bytes
            .windows(b"private_secret".len())
            .any(|part| part == b"private_secret")
    );
    assert_eq!(
        reader.declaration(&identity("public", 1))?.direct_call_id.path(),
        "pub::probe::lib#decl.100..199"
    );
    Ok(())
}

/// Native implementations stay outside compiled frames; governed calls require a retained checked native selection.
#[test]
fn compiled_contract_excludes_native_implementation() -> Result<(), Box<dyn std::error::Error>> {
    let mut native = identity("serialize", 9);
    native.origin = SymbolOrigin::RustCrate(vec!["serde_json".into()]);
    let mut compiled = body("compiled", 1);
    compiled.type_parameters.clear();
    compiled.block.stmts.push(call(native.clone()));
    let mut governed = body("governed", 2);
    governed.block.stmts.push(call(native));
    let bytes = publish(
        &module(vec![compiled, governed]),
        &BTreeSet::from([identity("compiled", 1), identity("governed", 2)]),
    )?;
    let reader = SurfaceReader::open(&bytes)?;
    let ExecutableDeclaration::CompiledCallableFrame { frame, .. } = reader.fragment(&identity("compiled", 1))? else {
        return Err("compiled callable contract missing".into());
    };
    assert!(frame.block.stmts.is_empty());
    assert!(frame.locals.is_empty());
    assert!(reader.declaration(&identity("compiled", 1)).is_err());
    assert!(matches!(
        reader.index().coverage(&identity("governed", 2))?,
        DeclarationCoverage::Uncovered(CoverageReason::UnresolvedReference)
    ));
    Ok(())
}

/// Missing and unsupported callees propagate uncovered status through dependent executable declarations.
#[test]
fn missing_and_unsupported_dependencies_are_uncovered_transitively() -> Result<(), Box<dyn std::error::Error>> {
    let mut caller = body("caller", 1);
    caller.block.stmts.push(call(identity("private_secret", 2)));
    let mut dependent = body("dependent", 3);
    dependent.block.stmts.push(call(identity("caller", 1)));
    let mut unsupported = body("unsupported", 4);
    unsupported.block.stmts.push(Statement {
        span: unsupported.span,
        kind: StatementKind::Unsupported {
            description: "never serialized".into(),
        },
    });
    let module = module(vec![caller, dependent, unsupported]);
    let public = BTreeSet::from([
        identity("caller", 1),
        identity("dependent", 3),
        identity("unsupported", 4),
    ]);
    let bytes = publish(&module, &public)?;
    let reader = SurfaceReader::open(&bytes)?;
    assert_eq!(reader.covered_identities().count(), 0);
    // The reason has to be asserted, not just the absence of coverage. `UnsupportedConstruct` is the one
    // reason whose whole purpose is to reach a consumer distinctly, and without this a mutant mapping it to
    // `UnresolvedReference` or `PrivateDependency` passes the test named after it.
    assert!(matches!(
        reader.index().coverage(&identity("unsupported", 4))?,
        DeclarationCoverage::Uncovered(CoverageReason::UnsupportedConstruct)
    ));
    assert!(matches!(
        reader.index().coverage(&identity("caller", 1))?,
        DeclarationCoverage::Uncovered(CoverageReason::PrivateDependency)
    ));
    assert!(matches!(
        reader.index().coverage(&identity("dependent", 3))?,
        DeclarationCoverage::Uncovered(CoverageReason::RequiredDeclarationUnavailable)
    ));
    assert!(
        !bytes
            .windows(b"private_secret".len())
            .any(|part| part == b"private_secret")
    );
    // A compiled export's checked signature survives an unavailable implementation without becoming a body.
    let mut compiled = module;
    compiled.bodies[2].type_parameters.clear();
    compiled.bodies[2].callable_representation = Some(crate::body_ir::CallableRepresentation {
        function_pointer_locals: Vec::new(),
        closure_holding_locals: Vec::new(),
        closure_holding_parameters: Vec::new(),
        closure_holding_return: false,
    });
    let bytes = publish(&compiled, &public)?;
    let reader = SurfaceReader::open(&bytes)?;
    assert!(matches!(
        reader.fragment(&identity("unsupported", 4))?,
        ExecutableDeclaration::CompiledCallableFrame { frame, .. } if frame.block.stmts.is_empty()
    ));
    assert!(reader.declaration(&identity("unsupported", 4)).is_err());
    Ok(())
}

#[test]
fn recursive_public_closure_is_order_independent() -> Result<(), Box<dyn std::error::Error>> {
    let mut alpha = body("alpha", 1);
    let mut beta = body("beta", 2);
    alpha.block.stmts.push(call(identity("beta", 2)));
    beta.block.stmts.push(call(identity("alpha", 1)));
    let public = BTreeSet::from([identity("alpha", 1), identity("beta", 2)]);
    let forward = publish(&module(vec![alpha.clone(), beta.clone()]), &public)?;
    let reverse = publish(&module(vec![beta, alpha]), &public)?;
    assert_eq!(forward, reverse);
    assert_eq!(SurfaceReader::open(&forward)?.covered_identities().count(), 2);
    Ok(())
}

/// Internal recursive fragments retain deterministic ranges without becoming selectable public roots.
#[test]
fn recursive_private_closure_keeps_disjoint_ranges_and_public_roots() -> Result<(), Box<dyn std::error::Error>> {
    let mut alpha = body("alpha", 1);
    let mut beta = body("beta", 2);
    alpha.block.stmts.push(call(identity("beta", 2)));
    beta.block.stmts.push(call(identity("alpha", 1)));
    let public = BTreeSet::from([identity("alpha", 1)]);
    let forward = publish(&module(vec![alpha.clone(), beta.clone()]), &public)?;
    let reverse = publish(&module(vec![beta, alpha]), &public)?;
    assert_eq!(forward, reverse);
    let reader = SurfaceReader::open(&forward)?;
    let payload_length = u64::try_from(reader.payload.len())?;
    assert_eq!(reader.covered_identities().count(), 1);
    assert!(reader.index().implementation.contains_key(&identity("beta", 2)));
    assert!(reader.fragment(&identity("beta", 2)).is_err());
    let mut overlap = reader.index().clone();
    let coverage = overlap.coverage(&identity("alpha", 1))?.clone();
    overlap.implementation.insert(identity("alpha", 1), coverage);
    assert!(matches!(
        overlap.validate(payload_length),
        Err(ExecutableRepresentationError::Malformed { .. })
    ));
    let mut ranges = reader.index().clone();
    let Some(DeclarationCoverage::Covered { offset, .. }) = ranges.implementation.get_mut(&identity("beta", 2)) else {
        return Err("private recursive body lacks a payload range".into());
    };
    *offset = 0;
    assert!(matches!(
        ranges.validate(payload_length),
        Err(ExecutableRepresentationError::Malformed { .. })
    ));
    Ok(())
}

#[test]
fn private_default_dependency_is_not_hidden_by_an_empty_main_block() -> Result<(), Box<dyn std::error::Error>> {
    let mut exported = body("exported", 1);
    exported.params.push(CallableParam {
        mutable: false,
        input_access: crate::IncanInputAccess::Inferred,
        native_owned: false,
        kind: crate::IncanCallableParamKind::Normal,
        local: crate::body_ir::LocalId(0),
        name: "x".into(),
        ty: IncanType::Primitive(IncanPrimitiveType::Int),
        span: exported.span,
        default: CallableParamDefault::Source(Box::new(crate::body_ir::DefaultComputation {
            span: exported.span,
            stmts: vec![call(identity("secret", 2))],
            result: crate::body_ir::Operand::Constant(crate::body_ir::Constant::Int(1)),
        })),
    });
    let bytes = publish(
        &module(vec![exported, body("secret", 2)]),
        &BTreeSet::from([identity("exported", 1)]),
    )?;
    let reader = SurfaceReader::open(&bytes)?;
    assert!(reader.covers(&identity("exported", 1)));
    assert!(reader.index().implementation.contains_key(&identity("secret", 2)));
    assert!(reader.index().coverage(&identity("secret", 2)).is_err());
    Ok(())
}

#[test]
fn three_selected_fragments_ignore_unrelated_corrupt_payloads() -> Result<(), Box<dyn std::error::Error>> {
    let bodies = (0..400)
        .map(|index| body(&format!("f{index:03}"), index))
        .collect::<Vec<_>>();
    let public = bodies.iter().filter_map(|body| body.canonical.clone()).collect();
    let mut bytes = publish(&module(bodies), &public)?;
    let last = bytes.len().checked_sub(1).ok_or("surface cannot be empty")?;
    bytes[last] ^= 255;
    let reader = SurfaceReader::open(&bytes)?;
    assert_eq!(reader.covered_identities().count(), 400);
    for index in [1, 2, 3] {
        assert_eq!(
            reader.declaration(&identity(&format!("f{index:03}"), index))?.name,
            format!("f{index:03}")
        );
    }
    assert!(reader.declaration(&identity("f399", 399)).is_err());
    Ok(())
}

/// Public indexes and generic decorator frames refuse overlapping addresses and substituted callable identities.
#[test]
fn index_refuses_overlaps_and_payload_identity_substitution() -> Result<(), Box<dyn std::error::Error>> {
    let public = BTreeSet::from([identity("alpha", 1), identity("beta", 2)]);
    let bytes = publish(&module(vec![body("alpha", 1), body("beta", 2)]), &public)?;
    let reader = SurfaceReader::open(&bytes)?;
    let mut index = reader.index().clone();
    let Some(DeclarationCoverage::Covered { offset, .. }) = index.declarations.get_mut(&identity("beta", 2)) else {
        return Err("covered beta missing".into());
    };
    *offset = 0;
    assert!(index.validate(u64::try_from(reader.payload.len())?).is_err());
    let wrong = postcard::to_allocvec(&ExecutableDeclaration::Body(Box::new(
        reader.declaration(&identity("beta", 2))?,
    )))?;
    assert!(
        reader
            .index()
            .decode_declaration(&identity("alpha", 1), &wrong)
            .is_err()
    );

    let wrapper = body("cached", 3);
    let mut initializer = wrapper.clone();
    initializer.return_type = IncanType::Function {
        params: Vec::new(),
        return_type: Box::new(wrapper.return_type.clone()),
    };
    let mut generic = module(vec![wrapper.clone()]);
    generic.decorated_bindings.push(crate::body_ir::DecoratedBinding {
        original_body: wrapper,
        initializer: Some(Box::new(initializer)),
        initialization: crate::body_ir::DecoratedInitialization::PerInstantiation,
    });
    let generic_public = BTreeSet::from([identity("cached", 3)]);
    let generic_bytes = publish(&generic, &generic_public)?;
    let generic_reader = SurfaceReader::open(&generic_bytes)?;
    let fragment = generic_reader.fragment(&identity("cached", 3))?;
    assert_eq!(fragment.identity(), Some(&identity("cached", 3)));
    let mut substituted = fragment.clone();
    let ExecutableDeclaration::GenericDecoratedBody { body, .. } = &mut substituted else {
        return Err("generic decorator fragment missing".into());
    };
    body.direct_call_id = CompilerNodeId::declaration_span("foreign", 300, 399);
    assert!(substituted.identity().is_none());
    let mut asynchronous = fragment;
    let ExecutableDeclaration::GenericDecoratedBody { body, .. } = &mut asynchronous else {
        return Err("generic decorator fragment missing".into());
    };
    body.is_async = true;
    assert!(asynchronous.identity().is_none());
    Ok(())
}
