//! Carrier keys, result identities, global write policies, and checked input and call profiles.

use super::*;

/// Only an authenticated SDK native bridge gains a source carrier key; equal names remain distinct owners.
#[test]
fn sdk_native_carrier_key_preserves_source_identity() -> Result<(), String> {
    let mut sdk = crate::CanonicalSymbolId::module_declaration(
        vec!["reflection".into()],
        "FieldInfo",
        crate::SemanticSourceTargetKind::Class,
        crate::HirSourceSpan::new(52, 423),
    );
    sdk.origin = crate::SymbolOrigin::Package {
        library: "incan_stdlib_core".into(),
        module_path: vec!["reflection".into()],
    };
    let key = super::nominal_carrier_key(&sdk).ok_or("missing SDK source carrier")?;
    assert!(!key.contains("::"));
    assert_eq!(
        super::native_nominal_item(&sdk).as_deref(),
        Some("incan_std_primitives::reflection::FieldInfo")
    );
    let mut primitive = sdk.clone();
    primitive.origin = crate::SymbolOrigin::Package {
        library: "incan_stdlib_primitives".into(),
        module_path: vec!["reflection".into()],
    };
    assert_eq!(
        super::native_nominal_item(&primitive).as_deref(),
        Some("incan_std_primitives::reflection::FieldInfo")
    );
    let mut local = sdk.clone();
    local.origin = crate::SymbolOrigin::Package {
        library: "application".into(),
        module_path: vec!["reflection".into()],
    };
    assert_ne!(super::nominal_carrier_key(&local), Some(key));
    assert!(super::native_nominal_item(&local).is_none());
    let mut ordinary = sdk;
    ordinary.declaration_name = "OtherRecord".into();
    assert!(super::nominal_carrier_key(&ordinary).is_none());
    assert!(super::native_nominal_item(&ordinary).is_none());
    Ok(())
}

/// Carrier bytes remain identical to the original formatting, including UTF-8 owners and expanded declarations.
#[test]
fn nominal_carrier_encoding_preserves_identity_bytes() -> Result<(), String> {
    for (module, name) in [("types", "Pair"), ("módulo.类型", "Élément"), ("", "_")] {
        for (kind, spelling) in [
            (crate::SemanticSourceTargetKind::Model, "model"),
            (crate::SemanticSourceTargetKind::Class, "class"),
            (crate::SemanticSourceTargetKind::Newtype, "newtype"),
            (crate::SemanticSourceTargetKind::Enum, "enum"),
        ] {
            let mut identity = crate::CanonicalSymbolId::module_declaration(
                vec![module.into()],
                name,
                kind,
                crate::HirSourceSpan::new(17, 103),
            );
            for (start, end, expansion) in [
                (0, 0, None),
                (17, 103, None),
                (17, 103, Some([0xab; 32])),
                (usize::MAX, usize::MAX, Some([0xab; 32])),
            ] {
                identity.declaration_span.start = start;
                identity.declaration_span.end = end;
                identity.declaration_span.expansion = expansion;
                let owner = crate::canonical_module_identity(&identity).ok_or("missing owner")?;
                let encode = |value: &str| value.bytes().map(|byte| format!("{byte:02x}")).collect::<String>();
                let suffix = expansion
                    .map(|value| format!("_expansion_{}", hex::encode(value)))
                    .unwrap_or_default();
                let expected = format!(
                    "__incan_carrier_{spelling}_{}_{}_{}_{}_{start}_{end}{suffix}",
                    owner.len(),
                    encode(&owner),
                    name.len(),
                    encode(name)
                );
                let actual = super::nominal_carrier_key(&identity).ok_or("missing carrier")?;
                assert_eq!(actual, expected);
                assert_eq!(
                    actual.capacity(),
                    actual.len(),
                    "carrier keys must not retain spare capacity"
                );
            }
        }
    }
    Ok(())
}

/// Named leaves keep structural identity paths while aliases remain diagnostic spelling.
#[test]
fn result_type_identity_paths_are_complete_and_reject_extra_or_missing_leaves() {
    let canonical = crate::CanonicalSymbolId::module_declaration(
        vec!["types".into()],
        "Pair",
        crate::SemanticSourceTargetKind::Model,
        crate::HirSourceSpan::new(0, 20),
    );
    let mut variant = super::ResultVariant {
        kind: super::ResultVariantKind::Ok,
        payload: super::Operand::Constant(super::Constant::Unit),
        ok_type: crate::IncanType::Generic {
            base: "list".into(),
            args: vec![crate::IncanType::Tuple(vec![crate::IncanType::Named("Alias".into())])],
        },
        error_type: crate::IncanType::Primitive(crate::IncanPrimitiveType::Str),
        canonical_types: std::collections::BTreeMap::from([(vec![0, 0, 0], canonical.clone())]),
    };
    assert!(variant.has_complete_canonical_types());
    variant.canonical_types.insert(vec![1], canonical);
    assert!(!variant.has_complete_canonical_types());
    variant.canonical_types.clear();
    assert!(!variant.has_complete_canonical_types());
}

/// Checked newtypes are nominal Result payloads; aliases, value bindings and unproven origins are not.
#[test]
fn result_newtype_identity_requires_its_declaration_authority() {
    let canonical = crate::CanonicalSymbolId::module_declaration(
        vec!["types".into()],
        "Kind",
        crate::SemanticSourceTargetKind::Newtype,
        crate::HirSourceSpan::new(0, 20),
    );
    let mut variant = super::ResultVariant {
        kind: super::ResultVariantKind::Err,
        payload: super::Operand::Constant(super::Constant::Unit),
        ok_type: crate::IncanType::Primitive(crate::IncanPrimitiveType::Int),
        error_type: crate::IncanType::Named("Kind".into()),
        canonical_types: std::collections::BTreeMap::from([(vec![1], canonical.clone())]),
    };
    assert!(variant.has_complete_canonical_types());
    for kind in [
        crate::SemanticSourceTargetKind::TypeAlias,
        crate::SemanticSourceTargetKind::Local,
        crate::SemanticSourceTargetKind::Function,
    ] {
        let mut invalid = canonical.clone();
        invalid.kind = kind;
        variant.canonical_types.insert(vec![1], invalid);
        assert!(!variant.has_complete_canonical_types());
    }
    let mut invalid = canonical.clone();
    invalid.origin = crate::SymbolOrigin::Builtin;
    variant.canonical_types.insert(vec![1], invalid);
    assert!(!variant.has_complete_canonical_types());
    let mut invalid = canonical.clone();
    invalid.scope_discriminant = Some(crate::ScopeDiscriminant(0));
    variant.canonical_types.insert(vec![1], invalid);
    assert!(!variant.has_complete_canonical_types());
    let mut published = canonical;
    published.origin = crate::SymbolOrigin::Package {
        library: "types".into(),
        module_path: vec!["types".into()],
    };
    variant.canonical_types.insert(vec![1], published);
    assert!(variant.has_complete_canonical_types());
}

#[test]
fn global_write_policies_distinguish_rebinding_from_projection_mutation() {
    let local = Place::from_local(LocalId(0));
    let read_only = sample_global_place(SemanticSourceTargetKind::Const, GlobalWritePolicy::ReadOnly);
    let mut read_only_projection = read_only.clone();
    read_only_projection
        .projection
        .push(PlaceElem::structural_field("member"));
    let projection_only = sample_global_place(SemanticSourceTargetKind::Static, GlobalWritePolicy::ProjectionOnly);
    let mut mutable_projection = projection_only.clone();
    mutable_projection
        .projection
        .push(PlaceElem::structural_field("member"));
    let rebindable = sample_global_place(SemanticSourceTargetKind::Static, GlobalWritePolicy::Rebindable);

    assert!(local.permits_write());
    assert!(!read_only.permits_write());
    assert!(!read_only_projection.permits_write());
    assert!(!projection_only.permits_write());
    assert!(mutable_projection.permits_write());
    assert!(rebindable.permits_write());
}

#[test]
fn body_snapshot_is_deterministic() {
    let body = sample_body();
    assert_eq!(body.render_snapshot(), body.render_snapshot());
}

/// A parameter mode cannot grant execution by itself; owner, version and complete slot inventory are required.
#[test]
fn explicit_inputs_require_matching_body_profile() -> Result<(), Box<dyn std::error::Error>> {
    let mut body = sample_body();
    body.canonical = Some(CanonicalSymbolId::module_declaration(
        vec!["main".into()],
        "add",
        SemanticSourceTargetKind::Function,
        body.span,
    ));
    body.params[0].input_access = crate::IncanInputAccess::Shared;
    assert!(body.first_representation_gap().is_some());
    let profile = super::CheckedInputProfile {
        version: 1,
        owner: body.canonical.clone().ok_or("missing owner")?,
        owned_initializers: Vec::new(),
        parameters: body
            .params
            .iter()
            .enumerate()
            .map(|(slot, param)| super::CheckedInputParameter {
                slot,
                access: param.input_access,
            })
            .collect(),
    };
    body.checked_input_profile = Some(profile.clone());
    assert!(body.first_representation_gap().is_none());
    let encoded = postcard::to_allocvec(&body)?;
    let decoded: Body = postcard::from_bytes(&encoded)?;
    assert_eq!(decoded.checked_input_profile, body.checked_input_profile);
    assert!(decoded.first_representation_gap().is_none());
    let mut old_profile = serde_json::to_value(&profile)?;
    old_profile
        .as_object_mut()
        .ok_or("profile is not an object")?
        .remove("owned_initializers");
    assert!(serde_json::from_value::<super::CheckedInputProfile>(old_profile).is_err());
    let assignment = HirSourceSpan::new(10, 30).with_expansion(Some([7; 32]));
    let initializer = super::CheckedOwnedInitializer {
        source: CanonicalSymbolId::module_declaration(
            vec!["main".into()],
            "source",
            SemanticSourceTargetKind::Parameter,
            HirSourceSpan::new(1, 8),
        ),
        destination: CanonicalSymbolId::module_declaration(
            vec!["main".into()],
            "destination",
            SemanticSourceTargetKind::Local,
            HirSourceSpan::new(14, 25),
        ),
        assignment,
    };
    // Initializer transfers travel with both profiles; only the conditional profile may execute them.
    for version in [1, 2] {
        body.checked_input_profile = Some(super::CheckedInputProfile {
            version,
            owned_initializers: vec![initializer.clone()],
            ..profile.clone()
        });
        let decoded: Body = postcard::from_bytes(&postcard::to_allocvec(&body)?)?;
        assert_eq!(decoded.checked_input_profile, body.checked_input_profile);
        assert_eq!(
            decoded.first_representation_gap().is_some(),
            version == 1,
            "profile {version} with an initializer transfer"
        );
    }
    body.checked_input_profile = Some(profile.clone());
    let mut named = serde_json::to_value(&body)?;
    named
        .as_object_mut()
        .ok_or("body is not an object")?
        .remove("checked_input_profile");
    let legacy: Body = serde_json::from_value(named)?;
    assert!(legacy.first_representation_gap().is_some());
    for invalid in [
        super::CheckedInputProfile {
            version: 3,
            ..profile.clone()
        },
        super::CheckedInputProfile {
            parameters: Vec::new(),
            ..profile.clone()
        },
        super::CheckedInputProfile {
            owner: CanonicalSymbolId::module_declaration(
                vec!["other".into()],
                "add",
                SemanticSourceTargetKind::Function,
                body.span,
            ),
            ..profile.clone()
        },
        super::CheckedInputProfile {
            parameters: vec![
                super::CheckedInputParameter {
                    slot: 0,
                    access: crate::IncanInputAccess::Owned,
                },
                profile.parameters[1].clone(),
            ],
            ..profile.clone()
        },
    ] {
        body.checked_input_profile = Some(invalid);
        assert!(body.first_representation_gap().is_some());
    }
    body.checked_input_profile = Some(profile);
    body.params[0].native_owned = true;
    assert!(body.first_representation_gap().is_some());
    Ok(())
}

/// Removing a caller profile from named persisted data must not grant an explicit source call merely because
/// all caller parameters use ordinary types. Positional transport preserves the same independent obligation.
#[test]
fn explicit_calls_require_persisted_caller_authority() -> Result<(), Box<dyn std::error::Error>> {
    let mut caller = sample_body();
    let owner = CanonicalSymbolId::module_declaration(
        vec!["main".into()],
        "caller",
        SemanticSourceTargetKind::Function,
        caller.span,
    );
    caller.canonical = Some(owner.clone());
    caller.checked_input_profile = Some(super::CheckedInputProfile {
        version: 1,
        owner,
        owned_initializers: Vec::new(),
        parameters: caller
            .params
            .iter()
            .enumerate()
            .map(|(slot, parameter)| super::CheckedInputParameter {
                slot,
                access: parameter.input_access,
            })
            .collect(),
    });
    let mut callee = caller.clone();
    let callee_owner = CanonicalSymbolId::module_declaration(
        vec!["main".into()],
        "callee",
        SemanticSourceTargetKind::Function,
        callee.span,
    );
    callee.canonical = Some(callee_owner.clone());
    callee.params[0].input_access = crate::IncanInputAccess::Shared;
    let profile = callee.checked_input_profile.as_mut().ok_or("missing callee profile")?;
    profile.owner = callee_owner;
    profile.parameters[0].access = crate::IncanInputAccess::Shared;
    assert!(caller.input_call_profile_gap(&callee).is_none());
    let decoded: Body = postcard::from_bytes(&postcard::to_allocvec(&caller)?)?;
    assert!(decoded.input_call_profile_gap(&callee).is_none());

    let mut named = serde_json::to_value(&caller)?;
    named
        .as_object_mut()
        .ok_or("caller is not an object")?
        .remove("checked_input_profile");
    let stripped: Body = serde_json::from_value(named)?;
    assert!(stripped.input_profile_gap().is_none());
    assert_eq!(
        stripped.input_call_profile_gap(&callee),
        Some("explicit source call lacks a checked caller body profile")
    );
    caller
        .checked_input_profile
        .as_mut()
        .ok_or("missing caller profile")?
        .owner = callee.canonical.clone().ok_or("missing callee identity")?;
    assert!(caller.input_call_profile_gap(&callee).is_some());
    callee.checked_input_profile = None;
    assert!(decoded.input_call_profile_gap(&callee).is_some());
    callee.params[0].input_access = crate::IncanInputAccess::Inferred;
    assert!(stripped.input_call_profile_gap(&callee).is_none());
    Ok(())
}
