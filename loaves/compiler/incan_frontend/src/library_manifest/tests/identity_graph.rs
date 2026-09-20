//! The semantic identity graph checked exports publish: member, overload and nested-module identities survive the
//! manifest round trip, compiled aliases and re-exports keep their targets, and legacy graphs without canonical
//! metadata stay readable.

use super::*;

fn published_identity(
    package: &str,
    module_path: &[&str],
    name: &str,
    kind: incan_semantics_core::SemanticSourceTargetKind,
    start: usize,
    end: usize,
) -> incan_semantics_core::CanonicalSymbolId {
    incan_semantics_core::CanonicalSymbolId {
        namespace: incan_semantics_core::SymbolNamespace::Member,
        origin: incan_semantics_core::SymbolOrigin::Package {
            library: package.to_string(),
            module_path: module_path.iter().map(|part| (*part).to_string()).collect(),
        },
        declaration_name: name.to_string(),
        kind,
        scope_discriminant: None,
        declaration_span: incan_semantics_core::HirSourceSpan::new(start, end),
    }
}

#[test]
fn checked_exports_publish_semantic_identity_graph() -> Result<(), Box<dyn std::error::Error>> {
    let cast_identity = source_identity(
        &["helpers"],
        "cast",
        incan_semantics_core::SemanticSourceTargetKind::Function,
        10,
        20,
    );
    let callable = crate::library_exports::CheckedFunctionExport {
        name: "cast".to_string(),
        emitted_name: Some("cast_overload_abcd".to_string()),
        type_params: Vec::new(),
        params: Vec::new(),
        param_defaults: Vec::new(),
        return_type: crate::symbols::ResolvedType::Int,
        is_async: false,
    };
    let exports = vec![
        crate::library_exports::CheckedNamedExport {
            name: "cast".to_string(),
            identity: crate::library_exports::CheckedExportIdentity::direct(vec![
                "helpers".to_string(),
                "cast".to_string(),
            ])
            .with_canonical(Some(cast_identity.clone())),
            kind: crate::library_exports::CheckedExportKind::Function(callable.clone()),
        },
        crate::library_exports::CheckedNamedExport {
            name: "safe_cast".to_string(),
            identity: crate::library_exports::CheckedExportIdentity::alias(
                vec!["facade".to_string(), "safe_cast".to_string()],
                vec!["helpers".to_string(), "cast".to_string()],
            )
            .with_canonical(Some(cast_identity.clone())),
            kind: crate::library_exports::CheckedExportKind::Alias(crate::library_exports::CheckedAliasExport {
                name: "safe_cast".to_string(),
                target_path: vec!["helpers".to_string(), "cast".to_string()],
                projected_type: None,
                projected_function: Some(crate::library_exports::CheckedFunctionExport {
                    name: "safe_cast".to_string(),
                    ..callable.clone()
                }),
            }),
        },
        crate::library_exports::CheckedNamedExport {
            name: "public_cast".to_string(),
            identity: crate::library_exports::CheckedExportIdentity::reexport(
                vec!["helpers".to_string(), "cast".to_string()],
                vec!["helpers".to_string(), "cast".to_string()],
            )
            .with_canonical(Some(cast_identity)),
            kind: crate::library_exports::CheckedExportKind::Function(crate::library_exports::CheckedFunctionExport {
                name: "public_cast".to_string(),
                ..callable.clone()
            }),
        },
        crate::library_exports::CheckedNamedExport {
            name: "core_cast".to_string(),
            identity: crate::library_exports::CheckedExportIdentity::partial(
                vec!["helpers".to_string(), "core_cast".to_string()],
                vec!["helpers".to_string(), "cast".to_string()],
                crate::library_exports::CheckedPartialTargetKind::Function,
            )
            .with_canonical(Some(source_identity(
                &["helpers"],
                "core_cast",
                incan_semantics_core::SemanticSourceTargetKind::Partial,
                30,
                40,
            ))),
            kind: crate::library_exports::CheckedExportKind::Partial(crate::library_exports::CheckedPartialExport {
                name: "core_cast".to_string(),
                target_path: vec!["helpers".to_string(), "cast".to_string()],
                target_kind: crate::library_exports::CheckedPartialTargetKind::Function,
                presets: vec![crate::library_exports::CheckedPartialPreset {
                    name: "target".to_string(),
                    ty: crate::symbols::ResolvedType::Str,
                    value: crate::library_exports::CheckedPresetValue::String("core".to_string()),
                }],
                type_params: Vec::new(),
                params: Vec::new(),
                return_type: crate::symbols::ResolvedType::Int,
                is_async: false,
            }),
        },
    ];

    let manifest = LibraryManifest::from_checked_exports("mylib", "0.1.0", &exports);
    let graph = &manifest.contract_metadata.identity_graph;
    assert_eq!(graph.schema_version, LIBRARY_IDENTITY_GRAPH_SCHEMA_VERSION);

    let cast = graph.entry_for_public_name("cast").ok_or("missing cast identity")?;
    assert_eq!(cast.public_path, vec!["mylib".to_string(), "cast".to_string()]);
    assert_eq!(cast.source_path, vec!["helpers".to_string(), "cast".to_string()]);
    assert_eq!(cast.projection, ExportIdentityProjection::Direct);
    let published_cast = cast
        .canonical
        .as_ref()
        .and_then(CanonicalIdentityExport::hydrate)
        .ok_or("missing hydrated cast identity")?;

    let safe_cast = graph
        .entry_for_public_name("safe_cast")
        .ok_or("missing safe_cast identity")?;
    assert_eq!(
        safe_cast.public_path,
        vec!["mylib".to_string(), "safe_cast".to_string()]
    );
    assert_eq!(
        safe_cast.projection,
        ExportIdentityProjection::Alias {
            target_path: vec!["helpers".to_string(), "cast".to_string()]
        }
    );
    assert_eq!(
        safe_cast.canonical.as_ref().and_then(CanonicalIdentityExport::hydrate),
        Some(published_cast.clone()),
        "an alias must preserve its target declaration identity"
    );

    let public_cast = graph
        .entry_for_public_name("public_cast")
        .ok_or("missing public_cast identity")?;
    assert_eq!(
        public_cast.projection,
        ExportIdentityProjection::Reexport {
            target_path: vec!["helpers".to_string(), "cast".to_string()]
        }
    );
    assert_eq!(
        public_cast
            .canonical
            .as_ref()
            .and_then(CanonicalIdentityExport::hydrate),
        Some(published_cast),
        "a reexport must preserve its target declaration identity"
    );

    let core_cast = graph
        .entry_for_public_name("core_cast")
        .ok_or("missing core_cast identity")?;
    assert_eq!(
        core_cast.projection,
        ExportIdentityProjection::Partial {
            target_path: vec!["helpers".to_string(), "cast".to_string()],
            target_kind: PartialTargetKindExport::Function,
        }
    );

    let tmp = tempfile::tempdir()?;
    let path = tmp.path().join("identity.incnlib");
    manifest.write_to_path(&path)?;
    let loaded = LibraryManifest::read_from_path(&path)?;
    assert_eq!(
        loaded.contract_metadata.identity_graph,
        manifest.contract_metadata.identity_graph
    );

    Ok(())
}

#[test]
fn manifest_io_round_trip_preserves_member_identities() -> Result<(), Box<dyn std::error::Error>> {
    use crate::library_exports::{
        CheckedClassExport, CheckedEnumExport, CheckedEnumVariant, CheckedExportIdentity, CheckedExportKind,
        CheckedField, CheckedMethod, CheckedNamedExport, CheckedProperty,
    };
    use crate::symbols::ResolvedType;

    let field = source_member_identity(
        &["domain"],
        "label",
        incan_semantics_core::SemanticSourceTargetKind::Field,
        20,
        25,
    );
    let property = source_member_identity(
        &["domain"],
        "display",
        incan_semantics_core::SemanticSourceTargetKind::Property,
        30,
        37,
    );
    let method = source_member_identity(
        &["domain"],
        "render",
        incan_semantics_core::SemanticSourceTargetKind::Method,
        40,
        46,
    );
    let variant = source_member_identity(
        &["domain"],
        "Ready",
        incan_semantics_core::SemanticSourceTargetKind::Variant,
        80,
        85,
    );
    let exports =
        vec![
            CheckedNamedExport {
                name: "Widget".to_string(),
                identity: CheckedExportIdentity::direct(vec!["domain".to_string(), "Widget".to_string()])
                    .with_canonical(Some(source_identity(
                        &["domain"],
                        "Widget",
                        incan_semantics_core::SemanticSourceTargetKind::Class,
                        10,
                        70,
                    ))),
                kind: CheckedExportKind::Class(CheckedClassExport {
                    name: "Widget".to_string(),
                    type_params: Vec::new(),
                    extends: None,
                    traits: Vec::new(),
                    trait_adoptions: Vec::new(),
                    derives: Vec::new(),
                    fields: vec![CheckedField {
                        name: "label".to_string(),
                        canonical: Some(field),
                        ty: ResolvedType::Str,
                        surface_type_name: Some("str".to_string()),
                        visibility: crate::ast::Visibility::Public,
                        has_default: false,
                        default: None,
                        alias: None,
                        description: None,
                    }],
                    properties: vec![CheckedProperty {
                        name: "display".to_string(),
                        canonical: Some(property),
                        return_type: ResolvedType::Str,
                    }],
                    methods: vec![CheckedMethod {
                        name: "render".to_string(),
                        canonical: Some(method),
                        alias_of: None,
                        type_params: Vec::new(),
                        receiver: None,
                        params: Vec::new(),
                        param_defaults: Vec::new(),
                        return_type: ResolvedType::Str,
                        is_async: false,
                        has_body: true,
                    }],
                }),
            },
            CheckedNamedExport {
                name: "State".to_string(),
                identity: CheckedExportIdentity::direct(vec!["domain".to_string(), "State".to_string()])
                    .with_canonical(Some(source_identity(
                        &["domain"],
                        "State",
                        incan_semantics_core::SemanticSourceTargetKind::Enum,
                        75,
                        100,
                    ))),
                kind: CheckedExportKind::Enum(CheckedEnumExport {
                    name: "State".to_string(),
                    type_params: Vec::new(),
                    traits: Vec::new(),
                    trait_adoptions: Vec::new(),
                    value_type: None,
                    variants: vec![CheckedEnumVariant {
                        name: "Ready".to_string(),
                        canonical: Some(variant),
                        fields: Vec::new(),
                        value: None,
                    }],
                    variant_aliases: Vec::new(),
                    methods: Vec::new(),
                    derives: Vec::new(),
                }),
            },
        ];

    let manifest = LibraryManifest::from_checked_exports("mylib", "0.1.0", &exports);
    let tmp = tempfile::tempdir()?;
    let mut missing_member = manifest.clone();
    missing_member.exports.classes[0].fields[0].canonical = None;
    let missing_member_error = missing_member.write_to_path(&tmp.path().join("missing-member.incnlib"));
    assert!(matches!(
        missing_member_error,
        Err(LibraryManifestError::Invalid(message))
            if message.contains("class `Widget` field `label` is missing its canonical member identity")
    ));
    let mut legacy_with_member_identity = manifest.clone();
    legacy_with_member_identity
        .contract_metadata
        .identity_graph
        .schema_version = LEGACY_LIBRARY_IDENTITY_GRAPH_SCHEMA_VERSION;
    for entry in &mut legacy_with_member_identity.contract_metadata.identity_graph.exports {
        entry.canonical = None;
    }
    let legacy_member_error =
        legacy_with_member_identity.write_to_path(&tmp.path().join("legacy-member-identity.incnlib"));
    assert!(matches!(
        legacy_member_error,
        Err(LibraryManifestError::Invalid(message))
            if message.contains("cannot publish canonical member metadata in schema v1")
    ));

    let mut wrong_member_name = manifest.clone();
    wrong_member_name.exports.classes[0].fields[0]
        .canonical
        .as_mut()
        .ok_or("field identity fixture must exist")?
        .declaration_name = "other".to_string();
    let wrong_member_name_error = wrong_member_name.write_to_path(&tmp.path().join("wrong-member-name.incnlib"));
    assert!(matches!(
        wrong_member_name_error,
        Err(LibraryManifestError::Invalid(message))
            if message.contains("canonical declaration name `other` instead of `label`")
    ));

    let mut wrong_member_owner = manifest.clone();
    wrong_member_owner.exports.classes[0].fields[0]
        .canonical
        .as_mut()
        .ok_or("field identity fixture must exist")?
        .origin = CanonicalIdentityOriginExport::Package {
        library: "other_lib".to_string(),
        module_path: vec!["domain".to_string()],
    };
    let wrong_member_owner_error = wrong_member_owner.write_to_path(&tmp.path().join("wrong-member-owner.incnlib"));
    assert!(matches!(
        wrong_member_owner_error,
        Err(LibraryManifestError::Invalid(message))
            if message.contains("canonical origin different from its owner declaration")
    ));
    let mut wrong_member_span = manifest.clone();
    wrong_member_span.exports.classes[0].fields[0]
        .canonical
        .as_mut()
        .ok_or("field identity fixture must exist")?
        .declaration_span = CanonicalIdentitySpanExport { start: 71, end: 72 };
    let wrong_member_span_error = wrong_member_span.write_to_path(&tmp.path().join("wrong-member-span.incnlib"));
    assert!(matches!(
        wrong_member_span_error,
        Err(LibraryManifestError::Invalid(message))
            if message.contains("canonical declaration span outside its owner declaration")
    ));
    let mut duplicate_member = manifest.clone();
    let duplicate_field = duplicate_member.exports.classes[0].fields[0].clone();
    duplicate_member.exports.classes[0].fields.push(duplicate_field);
    let duplicate_member_error = duplicate_member.write_to_path(&tmp.path().join("duplicate-member.incnlib"));
    assert!(matches!(
        duplicate_member_error,
        Err(LibraryManifestError::Invalid(message))
            if message.contains("duplicate canonical member identity `label`")
    ));
    let path = tmp.path().join("member-identities.incnlib");
    manifest.write_to_path(&path)?;
    let loaded = LibraryManifest::read_from_path(&path)?;

    let class = loaded.exports.classes.first().ok_or("missing class export")?;
    assert_eq!(
        class.fields[0]
            .canonical
            .as_ref()
            .and_then(CanonicalIdentityExport::hydrate),
        Some(published_identity(
            "mylib",
            &["domain"],
            "label",
            incan_semantics_core::SemanticSourceTargetKind::Field,
            20,
            25,
        ))
    );
    assert_eq!(
        class.properties[0]
            .canonical
            .as_ref()
            .and_then(CanonicalIdentityExport::hydrate),
        Some(published_identity(
            "mylib",
            &["domain"],
            "display",
            incan_semantics_core::SemanticSourceTargetKind::Property,
            30,
            37,
        ))
    );
    assert_eq!(
        class.methods[0]
            .canonical
            .as_ref()
            .and_then(CanonicalIdentityExport::hydrate),
        Some(published_identity(
            "mylib",
            &["domain"],
            "render",
            incan_semantics_core::SemanticSourceTargetKind::Method,
            40,
            46,
        ))
    );
    assert_eq!(
        loaded.exports.enums[0].variants[0]
            .canonical
            .as_ref()
            .and_then(CanonicalIdentityExport::hydrate),
        Some(published_identity(
            "mylib",
            &["domain"],
            "Ready",
            incan_semantics_core::SemanticSourceTargetKind::Variant,
            80,
            85,
        ))
    );

    Ok(())
}

#[test]
fn manifest_io_round_trip_preserves_overload_identities() -> Result<(), Box<dyn std::error::Error>> {
    use crate::library_exports::{CheckedExportIdentity, CheckedExportKind, CheckedFunctionExport, CheckedNamedExport};
    use crate::symbols::ResolvedType;

    let overload = |start, end, emitted_name: &str| CheckedNamedExport {
        name: "parse".to_string(),
        identity: CheckedExportIdentity::direct(vec!["codec".to_string(), "parse".to_string()]).with_canonical(Some(
            source_identity(
                &["codec"],
                "parse",
                incan_semantics_core::SemanticSourceTargetKind::Function,
                start,
                end,
            ),
        )),
        kind: CheckedExportKind::Function(CheckedFunctionExport {
            name: "parse".to_string(),
            emitted_name: Some(emitted_name.to_string()),
            type_params: Vec::new(),
            params: Vec::new(),
            param_defaults: Vec::new(),
            return_type: ResolvedType::Int,
            is_async: false,
        }),
    };
    let manifest = LibraryManifest::from_checked_exports(
        "codec_lib",
        "0.1.0",
        &[
            overload(10, 20, "parse__incan_overload_0000000000000001"),
            overload(30, 40, "parse__incan_overload_0000000000000002"),
        ],
    );
    let tmp = tempfile::tempdir()?;
    let mut incomplete = manifest.clone();
    incomplete.contract_metadata.identity_graph.exports.pop();
    let incomplete_error = incomplete.write_to_path(&tmp.path().join("incomplete-overload-identities.incnlib"));
    assert!(matches!(
        incomplete_error,
        Err(LibraryManifestError::Invalid(message))
            if message.contains("publishes 1 root Function identities named `parse` for 2 raw declarations")
    ));

    let path = tmp.path().join("overload-identities.incnlib");
    manifest.write_to_path(&path)?;
    let loaded = LibraryManifest::read_from_path(&path)?;

    let identities = loaded
        .contract_metadata
        .identity_graph
        .function_identities_for_public_name("parse");
    assert_eq!(identities.len(), 2);
    assert_ne!(identities[0], identities[1]);
    assert_eq!(
        identities
            .iter()
            .filter_map(|identity| identity.as_ref().map(|identity| identity.declaration_span))
            .collect::<Vec<_>>(),
        vec![
            incan_semantics_core::HirSourceSpan::new(10, 20),
            incan_semantics_core::HirSourceSpan::new(30, 40),
        ]
    );

    Ok(())
}

#[test]
fn compiled_nested_module_aliases_and_reexports_preserve_identity() -> Result<(), Box<dyn std::error::Error>> {
    use crate::library_exports::{
        CheckedAliasExport, CheckedExportIdentity, CheckedExportKind, CheckedFunctionExport, CheckedNamedExport,
    };
    use crate::symbols::ResolvedType;

    let callable = CheckedFunctionExport {
        name: "compute".to_string(),
        emitted_name: None,
        type_params: Vec::new(),
        params: Vec::new(),
        param_defaults: Vec::new(),
        return_type: ResolvedType::Int,
        is_async: false,
    };
    let compute_identity = source_identity(
        &["helpers"],
        "compute",
        incan_semantics_core::SemanticSourceTargetKind::Function,
        10,
        20,
    );
    let direct = CheckedNamedExport {
        name: "compute".to_string(),
        identity: CheckedExportIdentity::direct(vec!["helpers".to_string(), "compute".to_string()])
            .with_canonical(Some(compute_identity.clone())),
        kind: CheckedExportKind::Function(callable.clone()),
    };
    let projected = |name: &str, identity: CheckedExportIdentity| CheckedNamedExport {
        name: name.to_string(),
        identity: identity.with_canonical(Some(compute_identity.clone())),
        kind: CheckedExportKind::Alias(CheckedAliasExport {
            name: name.to_string(),
            target_path: vec!["helpers".to_string(), "compute".to_string()],
            projected_type: None,
            projected_function: Some(CheckedFunctionExport {
                name: name.to_string(),
                ..callable.clone()
            }),
        }),
    };
    let alias = projected(
        "safe_compute",
        CheckedExportIdentity::alias(
            vec!["facade".to_string(), "safe_compute".to_string()],
            vec!["helpers".to_string(), "compute".to_string()],
        ),
    );
    let reexport = projected(
        "public_compute",
        CheckedExportIdentity::reexport(
            vec!["helpers".to_string(), "compute".to_string()],
            vec!["helpers".to_string(), "compute".to_string()],
        ),
    );
    let checked_modules = vec![
        (vec!["helpers".to_string()], vec![direct]),
        (vec!["facade".to_string()], vec![alias, reexport]),
    ];
    let anchor = |id: &str, start: usize, end: usize| SourceAnchor {
        id: id.to_string(),
        span: SourceSpan { start, end },
    };
    let mut modules = vec![
        CheckedApiMetadata {
            schema_version: CHECKED_API_METADATA_SCHEMA_VERSION,
            derivable_traits: Vec::new(),
            module_path: vec!["helpers".to_string()],
            declarations: vec![ApiDeclaration::Function(ApiFunction {
                name: "compute".to_string(),
                anchor: anchor("helpers.compute", 10, 20),
                docstring: None,
                docstring_sections: None,
                decorators: Vec::new(),
                type_params: Vec::new(),
                params: Vec::new(),
                return_type: TypeRef::Named {
                    origin: None,
                    name: "int".to_string(),
                },
                is_async: false,
            })],
        },
        CheckedApiMetadata {
            schema_version: CHECKED_API_METADATA_SCHEMA_VERSION,
            derivable_traits: Vec::new(),
            module_path: vec!["facade".to_string()],
            declarations: vec![
                ApiDeclaration::Alias(ApiAlias {
                    name: "safe_compute".to_string(),
                    anchor: anchor("facade.safe_compute", 30, 40),
                    target_path: vec!["helpers".to_string(), "compute".to_string()],
                    is_public: true,
                    projected_type: None,
                    projected_function: None,
                }),
                ApiDeclaration::Alias(ApiAlias {
                    name: "public_compute".to_string(),
                    anchor: anchor("facade.public_compute", 50, 60),
                    target_path: vec!["helpers".to_string(), "compute".to_string()],
                    is_public: true,
                    projected_type: None,
                    projected_function: None,
                }),
            ],
        },
    ];
    materialize_api_alias_projections(&mut modules);
    let mut api = CheckedApiMetadataPackage {
        schema_version: CHECKED_API_METADATA_SCHEMA_VERSION,
        package: None,
        modules,
        public_namespaces: Vec::new(),
    };
    materialize_checked_api_public_namespaces(&mut api)?;

    let mut manifest = LibraryManifest::from_checked_exports("nested_lib", "0.1.0", &[]);
    manifest
        .contract_metadata
        .identity_graph
        .extend_checked_api_exports("nested_lib", &api, &checked_modules)?;
    manifest.contract_metadata.api = Some(api);
    let tmp = tempfile::tempdir()?;
    let path = tmp.path().join("nested-identities.incnlib");
    manifest.write_to_path(&path)?;
    let loaded = LibraryManifest::read_from_path(&path)?;

    let expected = published_declaration_identity(
        "nested_lib",
        &["helpers"],
        "compute",
        incan_semantics_core::SemanticSourceTargetKind::Function,
        10,
        20,
    );
    for path in [
        ["nested_lib", "helpers", "compute"],
        ["nested_lib", "facade", "safe_compute"],
        ["nested_lib", "facade", "public_compute"],
    ] {
        assert_eq!(
            loaded
                .contract_metadata
                .identity_graph
                .canonical_for_public_path(&path.map(str::to_string)),
            Some(expected.clone()),
            "compiled public path `{}` must retain the provider declaration identity",
            path.join(".")
        );
    }
    let graph = &loaded.contract_metadata.identity_graph;
    assert_eq!(
        graph.canonical_for_public_name("compute"),
        None,
        "a nested declaration must not become an ambient package-root export"
    );
    let safe = graph
        .exports
        .iter()
        .find(|entry| entry.public_path == ["nested_lib", "facade", "safe_compute"].map(str::to_string))
        .ok_or("missing nested alias identity")?;
    assert!(matches!(safe.projection, ExportIdentityProjection::Alias { .. }));
    let public = graph
        .exports
        .iter()
        .find(|entry| entry.public_path == ["nested_lib", "facade", "public_compute"].map(str::to_string))
        .ok_or("missing nested reexport identity")?;
    assert!(matches!(public.projection, ExportIdentityProjection::Reexport { .. }));

    let mut fabricated = manifest.clone();
    let mut fabricated_entry = fabricated
        .contract_metadata
        .identity_graph
        .exports
        .iter()
        .find(|entry| entry.public_path == ["nested_lib", "helpers", "compute"].map(str::to_string))
        .cloned()
        .ok_or("missing nested direct identity fixture")?;
    fabricated_entry.public_name = "fabricated".to_string();
    fabricated_entry.public_path = ["nested_lib", "helpers", "fabricated"].map(str::to_string).to_vec();
    fabricated_entry.source_path = vec!["helpers".to_string(), "compute".to_string()];
    fabricated_entry.projection = ExportIdentityProjection::Reexport {
        target_path: vec!["helpers".to_string(), "compute".to_string()],
    };
    fabricated
        .contract_metadata
        .identity_graph
        .exports
        .push(fabricated_entry);
    let fabricated_error = fabricated.write_to_path(&tmp.path().join("fabricated-nested-identity.incnlib"));
    assert!(matches!(
        fabricated_error,
        Err(LibraryManifestError::Invalid(message))
            if message.contains("is not backed by a checked API namespace declaration")
    ));

    let mut fabricated_identity = manifest.clone();
    fabricated_identity
        .contract_metadata
        .identity_graph
        .exports
        .iter_mut()
        .find(|entry| entry.public_path == ["nested_lib", "helpers", "compute"].map(str::to_string))
        .and_then(|entry| entry.canonical.as_mut())
        .ok_or("missing nested canonical identity fixture")?
        .declaration_span = CanonicalIdentitySpanExport { start: 99, end: 100 };
    let fabricated_identity_error =
        fabricated_identity.write_to_path(&tmp.path().join("fabricated-nested-canonical-identity.incnlib"));
    assert!(matches!(
        fabricated_identity_error,
        Err(LibraryManifestError::Invalid(message))
            if message.contains("is not backed by a checked API namespace declaration")
    ));

    for (public_name, fixture_name) in [
        ("safe_compute", "mismatched-nested-alias-span.incnlib"),
        ("public_compute", "mismatched-nested-reexport-span.incnlib"),
    ] {
        let mut mismatched_target_identity = manifest.clone();
        mismatched_target_identity
            .contract_metadata
            .identity_graph
            .exports
            .iter_mut()
            .find(|entry| entry.public_name == public_name)
            .and_then(|entry| entry.canonical.as_mut())
            .ok_or("missing nested alias target identity fixture")?
            .declaration_span = CanonicalIdentitySpanExport { start: 98, end: 99 };
        let mismatched_target_identity_error = mismatched_target_identity.write_to_path(&tmp.path().join(fixture_name));
        assert!(matches!(
            mismatched_target_identity_error,
            Err(LibraryManifestError::Invalid(message))
                if message.contains("is not backed by a checked API namespace declaration")
        ));
    }

    let mut mismatched_target_projection = manifest.clone();
    let projected = mismatched_target_projection
        .contract_metadata
        .api
        .as_mut()
        .and_then(|api| {
            api.modules
                .iter_mut()
                .find(|module| module.module_path == ["facade".to_string()])
        })
        .and_then(|module| {
            module
                .declarations
                .iter_mut()
                .find_map(|declaration| match declaration {
                    ApiDeclaration::Alias(alias) if alias.name == "safe_compute" => alias.projected_function.as_mut(),
                    _ => None,
                })
        })
        .ok_or("missing checked API alias projection fixture")?;
    projected.source_path = vec!["helpers".to_string(), "not_compute".to_string()];
    let mismatched_target_projection_error =
        mismatched_target_projection.write_to_path(&tmp.path().join("mismatched-nested-target-projection.incnlib"));
    assert!(matches!(
        mismatched_target_projection_error,
        Err(LibraryManifestError::Invalid(message))
            if message.contains("is not backed by a checked API namespace declaration")
    ));

    let mut missing_api = manifest;
    missing_api.contract_metadata.api = None;
    let missing_api_error = missing_api.write_to_path(&tmp.path().join("nested-identity-without-api.incnlib"));
    assert!(matches!(
        missing_api_error,
        Err(LibraryManifestError::Invalid(message))
            if message.contains("has no checked API namespace backing")
    ));

    Ok(())
}

#[test]
fn legacy_identity_graph_remains_readable_without_canonical_metadata() -> Result<(), Box<dyn std::error::Error>> {
    let mut manifest = LibraryManifest::new("legacy_lib", "0.1.0");
    manifest.contract_metadata.identity_graph = LibraryIdentityGraph {
        schema_version: LEGACY_LIBRARY_IDENTITY_GRAPH_SCHEMA_VERSION,
        exports: vec![ExportIdentity {
            public_name: "parse".to_string(),
            public_path: vec!["legacy_lib".to_string(), "parse".to_string()],
            source_path: vec!["codec".to_string(), "parse".to_string()],
            kind: ExportIdentityKind::Function,
            projection: ExportIdentityProjection::Direct,
            canonical: None,
        }],
    };
    let tmp = tempfile::tempdir()?;
    let path = tmp.path().join("legacy-identity.incnlib");
    manifest.write_to_path(&path)?;
    let loaded = LibraryManifest::read_from_path(&path)?;

    assert_eq!(
        loaded.contract_metadata.identity_graph.schema_version,
        LEGACY_LIBRARY_IDENTITY_GRAPH_SCHEMA_VERSION
    );
    assert_eq!(
        loaded
            .contract_metadata
            .identity_graph
            .entry_for_public_name("parse")
            .and_then(|entry| entry.canonical.as_ref()),
        None
    );

    Ok(())
}

#[test]
fn omitted_identity_graph_and_contract_envelope_decode_as_legacy_v1() -> Result<(), Box<dyn std::error::Error>> {
    let manifest = LibraryManifest::new("legacy_lib", "0.1.0");
    let tmp = tempfile::tempdir()?;
    let current_path = tmp.path().join("current.incnlib");
    manifest.write_to_path(&current_path)?;
    let current_json = std::fs::read_to_string(&current_path)?;
    let current_value: serde_json::Value = serde_json::from_str(&current_json)?;
    assert_eq!(
        current_value["contract_metadata"]["identity_graph"]["schema_version"], LIBRARY_IDENTITY_GRAPH_SCHEMA_VERSION,
        "a current producer must serialize even an empty v2 graph explicitly"
    );

    let mut omitted_graph = current_value.clone();
    omitted_graph["contract_metadata"]
        .as_object_mut()
        .ok_or("contract metadata fixture must be an object")?
        .remove("identity_graph");
    let omitted_graph = LibraryManifest::from_json_str(&serde_json::to_string(&omitted_graph)?)?;
    assert_eq!(
        omitted_graph.contract_metadata.identity_graph.schema_version,
        LEGACY_LIBRARY_IDENTITY_GRAPH_SCHEMA_VERSION
    );
    assert!(omitted_graph.contract_metadata.identity_graph.exports.is_empty());

    let mut omitted_contract = current_value;
    omitted_contract
        .as_object_mut()
        .ok_or("manifest fixture must be an object")?
        .remove("contract_metadata");
    let omitted_contract = LibraryManifest::from_json_str(&serde_json::to_string(&omitted_contract)?)?;
    assert_eq!(
        omitted_contract.contract_metadata.identity_graph.schema_version,
        LEGACY_LIBRARY_IDENTITY_GRAPH_SCHEMA_VERSION
    );
    assert!(omitted_contract.contract_metadata.identity_graph.exports.is_empty());

    Ok(())
}

#[test]
fn checked_newtype_rewrite_uses_source_identity_for_same_leaf_names() -> Result<(), Box<dyn std::error::Error>> {
    use crate::library_exports::{CheckedExportIdentity, CheckedExportKind, CheckedNamedExport, CheckedNewtypeExport};
    use crate::symbols::ResolvedType;

    let checked_newtype = |name: &str, underlying: ResolvedType| CheckedNewtypeExport {
        name: name.to_string(),
        type_params: Vec::new(),
        traits: Vec::new(),
        trait_adoptions: Vec::new(),
        derives: Vec::new(),
        is_rusttype: false,
        underlying,
        checked_constructor: None,
        constraints: Vec::new(),
        implicit_coercion_enabled: true,
        methods: Vec::new(),
    };
    let exports = vec![
        CheckedNamedExport {
            name: "Id".to_string(),
            identity: CheckedExportIdentity::reexport(
                vec!["a".to_string(), "Id".to_string()],
                vec!["a".to_string(), "Id".to_string()],
            ),
            kind: CheckedExportKind::Newtype(checked_newtype("Id", ResolvedType::Int)),
        },
        CheckedNamedExport {
            name: "BId".to_string(),
            identity: CheckedExportIdentity::reexport(
                vec!["b".to_string(), "Id".to_string()],
                vec!["b".to_string(), "Id".to_string()],
            ),
            kind: CheckedExportKind::Newtype(checked_newtype("BId", ResolvedType::Int)),
        },
        CheckedNamedExport {
            name: "BoxedId".to_string(),
            identity: CheckedExportIdentity::reexport(
                vec!["b".to_string(), "BoxedId".to_string()],
                vec!["b".to_string(), "BoxedId".to_string()],
            ),
            kind: CheckedExportKind::Newtype(checked_newtype("BoxedId", ResolvedType::Named("Id".to_string()))),
        },
    ];

    let manifest = LibraryManifest::from_checked_exports("mylib", "0.1.0", &exports);
    let boxed = manifest
        .exports
        .newtypes
        .iter()
        .find(|newtype| newtype.name == "BoxedId")
        .ok_or("missing composed newtype export")?;
    assert_eq!(
        boxed.underlying,
        TypeRef::Named {
            origin: None,
            name: "BId".to_string()
        }
    );
    Ok(())
}
