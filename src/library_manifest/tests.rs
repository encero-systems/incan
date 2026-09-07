use std::collections::{BTreeMap, BTreeSet};

use crate::frontend::api_metadata::{
    ApiAlias, ApiDeclaration, ApiFunction, ApiModel, ApiNewtype, ApiTrait, CHECKED_API_METADATA_SCHEMA_VERSION,
    CheckedApiMetadata, CheckedApiMetadataPackage, SourceAnchor, SourceSpan, materialize_api_alias_projections,
    materialize_checked_api_public_namespaces,
};

use super::*;

fn legacy_manifest_fixture(name: &str, version: &str) -> LibraryManifest {
    let mut manifest = LibraryManifest::new(name, version);
    manifest.contract_metadata.identity_graph = LibraryIdentityGraph {
        schema_version: LEGACY_LIBRARY_IDENTITY_GRAPH_SCHEMA_VERSION,
        exports: Vec::new(),
    };
    manifest
}

fn source_identity(
    module_path: &[&str],
    name: &str,
    kind: incan_semantics_core::SemanticSourceTargetKind,
    start: usize,
    end: usize,
) -> incan_semantics_core::CanonicalSymbolId {
    incan_semantics_core::CanonicalSymbolId::module_declaration(
        module_path.iter().map(|part| (*part).to_string()).collect(),
        name,
        kind,
        incan_semantics_core::HirSourceSpan::new(start, end),
    )
}

fn source_member_identity(
    module_path: &[&str],
    name: &str,
    kind: incan_semantics_core::SemanticSourceTargetKind,
    start: usize,
    end: usize,
) -> incan_semantics_core::CanonicalSymbolId {
    incan_semantics_core::CanonicalSymbolId {
        namespace: incan_semantics_core::SymbolNamespace::Member,
        origin: incan_semantics_core::SymbolOrigin::Module(
            module_path.iter().map(|part| (*part).to_string()).collect(),
        ),
        declaration_name: name.to_string(),
        kind,
        scope_discriminant: None,
        declaration_span: incan_semantics_core::HirSourceSpan::new(start, end),
    }
}

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

fn published_declaration_identity(
    package: &str,
    module_path: &[&str],
    name: &str,
    kind: incan_semantics_core::SemanticSourceTargetKind,
    start: usize,
    end: usize,
) -> incan_semantics_core::CanonicalSymbolId {
    incan_semantics_core::CanonicalSymbolId {
        namespace: incan_semantics_core::SymbolNamespace::OrdinaryLexical,
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
fn manifest_io_round_trip_preserves_recursive_types_and_bounds() -> Result<(), Box<dyn std::error::Error>> {
    let mut manifest = legacy_manifest_fixture("mylib", "0.1.0");
    manifest.exports.functions.push(FunctionExport {
        name: "map_result".to_string(),
        emitted_name: None,
        type_params: vec![TypeParamExport {
            name: "T".to_string(),
            bounds: vec![TypeBoundExport {
                name: "Clone".to_string(),
                source_name: None,
                module_path: None,
                type_args: Vec::new(),
                implementation_type_params: Vec::new(),
            }],
        }],
        params: vec![ParamExport {
            name: "value".to_string(),
            ty: TypeRef::Applied {
                name: "Result".to_string(),
                args: vec![
                    TypeRef::Applied {
                        name: "Option".to_string(),
                        args: vec![TypeRef::TypeParam { name: "T".to_string() }],
                    },
                    TypeRef::Named {
                        name: "str".to_string(),
                    },
                ],
            },
            kind: ParamKindExport::Normal,
            has_default: false,
            default: None,
        }],
        return_type: TypeRef::Function {
            params: vec![TypeRef::Tuple {
                elements: vec![
                    TypeRef::TypeParam { name: "T".to_string() },
                    TypeRef::Named {
                        name: "int".to_string(),
                    },
                ],
            }],
            return_type: Box::new(TypeRef::Named {
                name: "bool".to_string(),
            }),
        },
        is_async: false,
    });

    let tmp = tempfile::tempdir()?;
    let path = tmp.path().join("mylib.incnlib");
    manifest.write_to_path(&path)?;
    let loaded = LibraryManifest::read_from_path(&path)?;

    assert_eq!(loaded, manifest);
    Ok(())
}

#[test]
fn manifest_io_preserves_private_class_field_visibility_issue883() -> Result<(), Box<dyn std::error::Error>> {
    let checked_class = crate::frontend::library_exports::CheckedClassExport {
        name: "Vault".to_string(),
        type_params: Vec::new(),
        extends: None,
        traits: Vec::new(),
        trait_adoptions: Vec::new(),
        derives: Vec::new(),
        fields: vec![
            crate::frontend::library_exports::CheckedField {
                name: "secret".to_string(),
                canonical: Some(source_member_identity(
                    &["lib"],
                    "secret",
                    incan_semantics_core::SemanticSourceTargetKind::Field,
                    20,
                    26,
                )),
                ty: crate::frontend::symbols::ResolvedType::Str,
                surface_type_name: Some("str".to_string()),
                visibility: crate::frontend::ast::Visibility::Private,
                has_default: false,
                default: None,
                alias: None,
                description: None,
            },
            crate::frontend::library_exports::CheckedField {
                name: "label".to_string(),
                canonical: Some(source_member_identity(
                    &["lib"],
                    "label",
                    incan_semantics_core::SemanticSourceTargetKind::Field,
                    30,
                    35,
                )),
                ty: crate::frontend::symbols::ResolvedType::Str,
                surface_type_name: Some("str".to_string()),
                visibility: crate::frontend::ast::Visibility::Public,
                has_default: false,
                default: None,
                alias: None,
                description: None,
            },
        ],
        properties: Vec::new(),
        methods: Vec::new(),
    };
    let manifest = LibraryManifest::from_checked_exports(
        "sealed_class_lib",
        "0.1.0",
        &[crate::frontend::library_exports::CheckedNamedExport {
            name: "Vault".to_string(),
            identity: crate::frontend::library_exports::CheckedExportIdentity::direct(vec![
                "lib".to_string(),
                "Vault".to_string(),
            ])
            .with_canonical(Some(source_identity(
                &["lib"],
                "Vault",
                incan_semantics_core::SemanticSourceTargetKind::Class,
                1,
                40,
            ))),
            kind: crate::frontend::library_exports::CheckedExportKind::Class(checked_class),
        }],
    );

    let tmp = tempfile::tempdir()?;
    let path = tmp.path().join("sealed_class_lib.incnlib");
    manifest.write_to_path(&path)?;
    let content = std::fs::read_to_string(&path)?;
    let loaded = LibraryManifest::read_from_path(&path)?;

    assert!(
        content.contains(r#""visibility": "private""#),
        "expected private visibility in manifest:\n{content}"
    );
    assert!(
        content.contains(r#""surface_type_name": "str""#),
        "expected source-level field type spelling in manifest:\n{content}"
    );
    assert!(
        !content.contains(r#""visibility": "public""#),
        "public visibility should retain the compact legacy representation:\n{content}"
    );
    assert_eq!(loaded, manifest);
    assert_eq!(
        loaded.exports.classes[0].fields[0].visibility,
        FieldVisibilityExport::Private
    );
    assert_eq!(
        loaded.exports.classes[0].fields[1].visibility,
        FieldVisibilityExport::Public
    );
    Ok(())
}

#[test]
fn legacy_manifest_fields_without_visibility_remain_public_issue883() -> Result<(), Box<dyn std::error::Error>> {
    let mut manifest = legacy_manifest_fixture("legacy_class_lib", "0.1.0");
    manifest.exports.classes.push(ClassExport {
        name: "Legacy".to_string(),
        type_params: Vec::new(),
        extends: None,
        traits: Vec::new(),
        trait_adoptions: Vec::new(),
        derives: Vec::new(),
        fields: vec![FieldExport {
            name: "value".to_string(),
            canonical: None,
            ty: TypeRef::Named {
                name: "str".to_string(),
            },
            surface_type_name: None,
            visibility: FieldVisibilityExport::Public,
            has_default: false,
            default: None,
            alias: None,
            description: None,
        }],
        properties: Vec::new(),
        methods: Vec::new(),
    });

    let tmp = tempfile::tempdir()?;
    let path = tmp.path().join("legacy_class_lib.incnlib");
    manifest.write_to_path(&path)?;
    let content = std::fs::read_to_string(&path)?;
    assert!(
        !content.contains("visibility"),
        "legacy-compatible public field should omit visibility"
    );

    let loaded = LibraryManifest::from_json_str(&content)?;
    assert_eq!(
        loaded.exports.classes[0].fields[0].visibility,
        FieldVisibilityExport::Public
    );
    Ok(())
}

#[test]
fn manifest_round_trips_private_model_field_visibility_issue884() -> Result<(), Box<dyn std::error::Error>> {
    let mut manifest = legacy_manifest_fixture("private_model_lib", "0.1.0");
    manifest.exports.models.push(ModelExport {
        name: "Record".to_string(),
        type_params: Vec::new(),
        traits: Vec::new(),
        trait_adoptions: Vec::new(),
        derives: Vec::new(),
        fields: vec![FieldExport {
            name: "secret".to_string(),
            canonical: None,
            ty: TypeRef::Named {
                name: "str".to_string(),
            },
            surface_type_name: None,
            visibility: FieldVisibilityExport::Private,
            has_default: false,
            default: None,
            alias: None,
            description: None,
        }],
        properties: Vec::new(),
        methods: Vec::new(),
    });

    let tmp = tempfile::tempdir()?;
    let path = tmp.path().join("private_model_lib.incnlib");
    manifest.write_to_path(&path)?;
    let loaded = LibraryManifest::read_from_path(&path)?;
    assert_eq!(
        loaded.exports.models[0].fields[0].visibility,
        FieldVisibilityExport::Private
    );
    Ok(())
}

fn private_api_field_issue883() -> FieldExport {
    FieldExport {
        name: "secret".to_string(),
        canonical: None,
        ty: TypeRef::Named {
            name: "str".to_string(),
        },
        surface_type_name: None,
        visibility: FieldVisibilityExport::Private,
        has_default: false,
        default: None,
        alias: None,
        description: None,
    }
}

fn api_anchor_issue883(name: &str) -> SourceAnchor {
    SourceAnchor {
        id: format!("private_api.{name}"),
        span: SourceSpan { start: 0, end: 1 },
    }
}

fn manifest_with_api_declaration_issue883(declaration: ApiDeclaration) -> LibraryManifest {
    let mut manifest = LibraryManifest::new("private_api_lib", "0.1.0");
    manifest.contract_metadata.api = Some(CheckedApiMetadataPackage {
        schema_version: CHECKED_API_METADATA_SCHEMA_VERSION,
        package: None,
        modules: vec![CheckedApiMetadata {
            schema_version: CHECKED_API_METADATA_SCHEMA_VERSION,
            module_path: vec!["private_api".to_string()],
            declarations: vec![declaration],
        }],
        public_namespaces: Vec::new(),
    });
    manifest
}

#[test]
fn manifest_round_trips_private_model_field_in_embedded_api_metadata_issue884() -> Result<(), Box<dyn std::error::Error>>
{
    let manifest = manifest_with_api_declaration_issue883(ApiDeclaration::Model(ApiModel {
        name: "Record".to_string(),
        anchor: api_anchor_issue883("Record"),
        docstring: None,
        docstring_sections: None,
        decorators: Vec::new(),
        type_params: Vec::new(),
        traits: Vec::new(),
        trait_adoptions: Vec::new(),
        derives: Vec::new(),
        fields: vec![private_api_field_issue883()],
        properties: Vec::new(),
        methods: Vec::new(),
    }));

    let tmp = tempfile::tempdir()?;
    let path = tmp.path().join("private_api_model.incnlib");
    manifest.write_to_path(&path)?;
    let loaded = LibraryManifest::read_from_path(&path)?;
    let api = loaded
        .contract_metadata
        .api
        .ok_or("expected embedded checked API metadata")?;
    let ApiDeclaration::Model(model) = &api.modules[0].declarations[0] else {
        return Err("expected embedded API model".into());
    };
    assert!(
        matches!(model.fields[0].visibility, FieldVisibilityExport::Private),
        "expected embedded private API model field to round-trip"
    );
    Ok(())
}

#[test]
fn manifest_rejects_private_trait_requirement_in_embedded_api_metadata_issue883()
-> Result<(), Box<dyn std::error::Error>> {
    let manifest = manifest_with_api_declaration_issue883(ApiDeclaration::Trait(ApiTrait {
        name: "RequiresSecret".to_string(),
        anchor: api_anchor_issue883("RequiresSecret"),
        docstring: None,
        docstring_sections: None,
        decorators: Vec::new(),
        type_params: Vec::new(),
        supertraits: Vec::new(),
        requires: vec![private_api_field_issue883()],
        methods: Vec::new(),
    }));

    let tmp = tempfile::tempdir()?;
    let error = manifest.write_to_path(&tmp.path().join("private_api_trait.incnlib"));
    assert!(
        matches!(error, Err(LibraryManifestError::Invalid(ref message)) if message.contains("API trait `RequiresSecret` required field `secret` cannot be private")),
        "expected embedded private API trait requirement to fail validation, got: {error:?}"
    );
    Ok(())
}

#[test]
fn manifest_io_round_trip_preserves_partial_exports() -> Result<(), Box<dyn std::error::Error>> {
    let mut manifest = legacy_manifest_fixture("mylib", "0.1.0");
    manifest.exports.partials.push(PartialExport {
        name: "get".to_string(),
        target_path: vec!["route".to_string()],
        target_kind: PartialTargetKindExport::Function,
        presets: vec![PartialPresetExport {
            name: "method".to_string(),
            ty: TypeRef::Named {
                name: "str".to_string(),
            },
            value: PresetValueExport::String("GET".to_string()),
        }],
        type_params: Vec::new(),
        params: vec![
            ParamExport {
                name: "method".to_string(),
                ty: TypeRef::Named {
                    name: "str".to_string(),
                },
                kind: ParamKindExport::Normal,
                has_default: true,
                default: None,
            },
            ParamExport {
                name: "path".to_string(),
                ty: TypeRef::Named {
                    name: "str".to_string(),
                },
                kind: ParamKindExport::Normal,
                has_default: false,
                default: None,
            },
        ],
        return_type: TypeRef::Named {
            name: "str".to_string(),
        },
        is_async: false,
    });

    let tmp = tempfile::tempdir()?;
    let path = tmp.path().join("partials.incnlib");
    manifest.write_to_path(&path)?;
    let loaded = LibraryManifest::read_from_path(&path)?;

    assert_eq!(loaded, manifest);
    Ok(())
}

#[test]
fn manifest_io_round_trip_preserves_parameter_defaults() -> Result<(), Box<dyn std::error::Error>> {
    let mut manifest = legacy_manifest_fixture("mylib", "0.1.0");
    manifest.exports.functions.push(FunctionExport {
        name: "with_default".to_string(),
        emitted_name: None,
        type_params: Vec::new(),
        params: vec![ParamExport {
            name: "value".to_string(),
            ty: TypeRef::Named {
                name: "int".to_string(),
            },
            kind: ParamKindExport::Normal,
            has_default: true,
            default: Some(ParamDefaultExport::Call {
                path: vec!["fallback".to_string()],
                args: vec![ParamDefaultCallArgExport {
                    name: None,
                    value: ParamDefaultExport::Int(0),
                }],
                signature: None,
            }),
        }],
        return_type: TypeRef::Named {
            name: "int".to_string(),
        },
        is_async: false,
    });

    let tmp = tempfile::tempdir()?;
    let path = tmp.path().join("defaults.incnlib");
    manifest.write_to_path(&path)?;
    let loaded = LibraryManifest::read_from_path(&path)?;

    assert_eq!(loaded, manifest);
    Ok(())
}

#[test]
fn function_export_from_checked_marks_only_materializable_defaults_as_omittable() {
    let export = super::model::function_export_from_checked(&crate::frontend::library_exports::CheckedFunctionExport {
        name: "with_default".to_string(),
        emitted_name: None,
        type_params: Vec::new(),
        params: vec![
            crate::frontend::symbols::CallableParam::named_with_default(
                "ok",
                crate::frontend::symbols::ResolvedType::Int,
                crate::frontend::ast::ParamKind::Normal,
                true,
            ),
            crate::frontend::symbols::CallableParam::named_with_default(
                "not_exportable",
                crate::frontend::symbols::ResolvedType::Int,
                crate::frontend::ast::ParamKind::Normal,
                true,
            ),
        ],
        param_defaults: vec![
            Some(crate::frontend::library_exports::CheckedParamDefault::Int(1)),
            Some(crate::frontend::library_exports::CheckedParamDefault::Unsupported),
        ],
        return_type: crate::frontend::symbols::ResolvedType::Unit,
        is_async: false,
    });

    assert!(export.params[0].has_default);
    assert_eq!(export.params[0].default, Some(ParamDefaultExport::Int(1)));
    assert!(!export.params[1].has_default);
    assert_eq!(export.params[1].default, None);
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
    let callable = crate::frontend::library_exports::CheckedFunctionExport {
        name: "cast".to_string(),
        emitted_name: Some("cast_overload_abcd".to_string()),
        type_params: Vec::new(),
        params: Vec::new(),
        param_defaults: Vec::new(),
        return_type: crate::frontend::symbols::ResolvedType::Int,
        is_async: false,
    };
    let exports = vec![
        crate::frontend::library_exports::CheckedNamedExport {
            name: "cast".to_string(),
            identity: crate::frontend::library_exports::CheckedExportIdentity::direct(vec![
                "helpers".to_string(),
                "cast".to_string(),
            ])
            .with_canonical(Some(cast_identity.clone())),
            kind: crate::frontend::library_exports::CheckedExportKind::Function(callable.clone()),
        },
        crate::frontend::library_exports::CheckedNamedExport {
            name: "safe_cast".to_string(),
            identity: crate::frontend::library_exports::CheckedExportIdentity::alias(
                vec!["facade".to_string(), "safe_cast".to_string()],
                vec!["helpers".to_string(), "cast".to_string()],
            )
            .with_canonical(Some(cast_identity.clone())),
            kind: crate::frontend::library_exports::CheckedExportKind::Alias(
                crate::frontend::library_exports::CheckedAliasExport {
                    name: "safe_cast".to_string(),
                    target_path: vec!["helpers".to_string(), "cast".to_string()],
                    projected_function: Some(crate::frontend::library_exports::CheckedFunctionExport {
                        name: "safe_cast".to_string(),
                        ..callable.clone()
                    }),
                },
            ),
        },
        crate::frontend::library_exports::CheckedNamedExport {
            name: "public_cast".to_string(),
            identity: crate::frontend::library_exports::CheckedExportIdentity::reexport(
                vec!["helpers".to_string(), "cast".to_string()],
                vec!["helpers".to_string(), "cast".to_string()],
            )
            .with_canonical(Some(cast_identity)),
            kind: crate::frontend::library_exports::CheckedExportKind::Function(
                crate::frontend::library_exports::CheckedFunctionExport {
                    name: "public_cast".to_string(),
                    ..callable.clone()
                },
            ),
        },
        crate::frontend::library_exports::CheckedNamedExport {
            name: "core_cast".to_string(),
            identity: crate::frontend::library_exports::CheckedExportIdentity::partial(
                vec!["helpers".to_string(), "core_cast".to_string()],
                vec!["helpers".to_string(), "cast".to_string()],
                crate::frontend::library_exports::CheckedPartialTargetKind::Function,
            )
            .with_canonical(Some(source_identity(
                &["helpers"],
                "core_cast",
                incan_semantics_core::SemanticSourceTargetKind::Partial,
                30,
                40,
            ))),
            kind: crate::frontend::library_exports::CheckedExportKind::Partial(
                crate::frontend::library_exports::CheckedPartialExport {
                    name: "core_cast".to_string(),
                    target_path: vec!["helpers".to_string(), "cast".to_string()],
                    target_kind: crate::frontend::library_exports::CheckedPartialTargetKind::Function,
                    presets: vec![crate::frontend::library_exports::CheckedPartialPreset {
                        name: "target".to_string(),
                        ty: crate::frontend::symbols::ResolvedType::Str,
                        value: crate::frontend::library_exports::CheckedPresetValue::String("core".to_string()),
                    }],
                    type_params: Vec::new(),
                    params: Vec::new(),
                    return_type: crate::frontend::symbols::ResolvedType::Int,
                    is_async: false,
                },
            ),
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
    use crate::frontend::library_exports::{
        CheckedClassExport, CheckedEnumExport, CheckedEnumVariant, CheckedExportIdentity, CheckedExportKind,
        CheckedField, CheckedMethod, CheckedNamedExport, CheckedProperty,
    };
    use crate::frontend::symbols::ResolvedType;

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
                        visibility: crate::frontend::ast::Visibility::Public,
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
    use crate::frontend::library_exports::{
        CheckedExportIdentity, CheckedExportKind, CheckedFunctionExport, CheckedNamedExport,
    };
    use crate::frontend::symbols::ResolvedType;

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
    use crate::frontend::library_exports::{
        CheckedAliasExport, CheckedExportIdentity, CheckedExportKind, CheckedFunctionExport, CheckedNamedExport,
    };
    use crate::frontend::symbols::ResolvedType;

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
                    name: "int".to_string(),
                },
                is_async: false,
            })],
        },
        CheckedApiMetadata {
            schema_version: CHECKED_API_METADATA_SCHEMA_VERSION,
            module_path: vec!["facade".to_string()],
            declarations: vec![
                ApiDeclaration::Alias(ApiAlias {
                    name: "safe_compute".to_string(),
                    anchor: anchor("facade.safe_compute", 30, 40),
                    target_path: vec!["helpers".to_string(), "compute".to_string()],
                    is_public: true,
                    projected_function: None,
                }),
                ApiDeclaration::Alias(ApiAlias {
                    name: "public_compute".to_string(),
                    anchor: anchor("facade.public_compute", 50, 60),
                    target_path: vec!["helpers".to_string(), "compute".to_string()],
                    is_public: true,
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
fn package_identity_path_keeps_same_named_module_and_declaration_segments() -> Result<(), Box<dyn std::error::Error>> {
    use crate::frontend::library_exports::{
        CheckedExportIdentity, CheckedExportKind, CheckedFunctionExport, CheckedNamedExport,
    };
    use crate::frontend::symbols::ResolvedType;

    let checked = CheckedNamedExport {
        name: "codec".to_string(),
        identity: CheckedExportIdentity::direct(vec!["codec".to_string(), "codec".to_string()]).with_canonical(Some(
            source_identity(
                &["codec"],
                "codec",
                incan_semantics_core::SemanticSourceTargetKind::Function,
                10,
                20,
            ),
        )),
        kind: CheckedExportKind::Function(CheckedFunctionExport {
            name: "codec".to_string(),
            emitted_name: None,
            type_params: Vec::new(),
            params: Vec::new(),
            param_defaults: Vec::new(),
            return_type: ResolvedType::Int,
            is_async: false,
        }),
    };
    let mut manifest = LibraryManifest::from_checked_exports("codec_lib", "0.1.0", &[checked]);
    manifest.contract_metadata.api = Some(CheckedApiMetadataPackage {
        schema_version: CHECKED_API_METADATA_SCHEMA_VERSION,
        package: None,
        modules: vec![CheckedApiMetadata {
            schema_version: CHECKED_API_METADATA_SCHEMA_VERSION,
            module_path: vec!["codec".to_string()],
            declarations: vec![ApiDeclaration::Function(ApiFunction {
                name: "codec".to_string(),
                anchor: SourceAnchor {
                    id: "codec.codec".to_string(),
                    span: SourceSpan { start: 10, end: 20 },
                },
                docstring: None,
                docstring_sections: None,
                decorators: Vec::new(),
                type_params: Vec::new(),
                params: Vec::new(),
                return_type: TypeRef::Named {
                    name: "int".to_string(),
                },
                is_async: false,
            })],
        }],
        public_namespaces: Vec::new(),
    });
    let tmp = tempfile::tempdir()?;
    let path = tmp.path().join("same-module-and-declaration-name.incnlib");
    manifest.write_to_path(&path)?;
    let loaded = LibraryManifest::read_from_path(&path)?;

    assert_eq!(
        loaded
            .contract_metadata
            .identity_graph
            .exports
            .first()
            .ok_or("missing package-root identity fixture")?
            .source_path,
        ["codec", "codec"].map(str::to_string)
    );

    let mut wrong_root_span = manifest;
    wrong_root_span
        .contract_metadata
        .identity_graph
        .exports
        .first_mut()
        .ok_or("missing package-root identity fixture")?
        .canonical
        .as_mut()
        .ok_or("missing package-root canonical fixture")?
        .declaration_span = CanonicalIdentitySpanExport { start: 11, end: 20 };
    let error = wrong_root_span.write_to_path(&tmp.path().join("wrong-root-span.incnlib"));
    assert!(matches!(
        error,
        Err(LibraryManifestError::Invalid(message))
            if message.contains("package-root identity graph entry `codec` is not backed")
    ));
    Ok(())
}

#[test]
fn package_root_nominal_reexport_requires_binding_and_exact_target_anchor() -> Result<(), Box<dyn std::error::Error>> {
    use crate::frontend::library_exports::{
        CheckedAliasExport, CheckedExportIdentity, CheckedExportKind, CheckedNamedExport,
    };

    let checked = CheckedNamedExport {
        name: "PublicRecord".to_string(),
        identity: CheckedExportIdentity::reexport(
            vec!["domain".to_string(), "Record".to_string()],
            vec!["domain".to_string(), "Record".to_string()],
        )
        .with_canonical(Some(source_identity(
            &["domain"],
            "Record",
            incan_semantics_core::SemanticSourceTargetKind::Model,
            10,
            20,
        ))),
        kind: CheckedExportKind::Alias(CheckedAliasExport {
            name: "PublicRecord".to_string(),
            target_path: vec!["domain".to_string(), "Record".to_string()],
            projected_function: None,
        }),
    };
    let mut manifest = LibraryManifest::from_checked_exports("records_lib", "0.1.0", &[checked]);
    manifest.contract_metadata.api = Some(CheckedApiMetadataPackage {
        schema_version: CHECKED_API_METADATA_SCHEMA_VERSION,
        package: None,
        modules: vec![
            CheckedApiMetadata {
                schema_version: CHECKED_API_METADATA_SCHEMA_VERSION,
                module_path: vec!["domain".to_string()],
                declarations: vec![ApiDeclaration::Model(ApiModel {
                    name: "Record".to_string(),
                    anchor: SourceAnchor {
                        id: "domain::Record".to_string(),
                        span: SourceSpan { start: 10, end: 20 },
                    },
                    docstring: None,
                    docstring_sections: None,
                    decorators: Vec::new(),
                    type_params: Vec::new(),
                    traits: Vec::new(),
                    trait_adoptions: Vec::new(),
                    derives: Vec::new(),
                    fields: Vec::new(),
                    properties: Vec::new(),
                    methods: Vec::new(),
                })],
            },
            CheckedApiMetadata {
                schema_version: CHECKED_API_METADATA_SCHEMA_VERSION,
                module_path: vec!["lib".to_string()],
                declarations: vec![ApiDeclaration::Alias(ApiAlias {
                    name: "PublicRecord".to_string(),
                    anchor: SourceAnchor {
                        id: "lib::PublicRecord".to_string(),
                        span: SourceSpan { start: 30, end: 40 },
                    },
                    target_path: vec!["domain".to_string(), "Record".to_string()],
                    is_public: true,
                    projected_function: None,
                })],
            },
        ],
        public_namespaces: Vec::new(),
    });
    let tmp = tempfile::tempdir()?;
    manifest.write_to_path(&tmp.path().join("root-nominal-reexport.incnlib"))?;

    let mut wrong_target_span = manifest.clone();
    wrong_target_span
        .contract_metadata
        .identity_graph
        .exports
        .iter_mut()
        .find(|entry| entry.public_name == "PublicRecord")
        .ok_or("missing nominal reexport graph fixture")?
        .canonical
        .as_mut()
        .ok_or("missing nominal reexport identity fixture")?
        .declaration_span = CanonicalIdentitySpanExport { start: 11, end: 20 };
    let error = wrong_target_span.write_to_path(&tmp.path().join("root-nominal-reexport-wrong-span.incnlib"));
    assert!(matches!(
        error,
        Err(LibraryManifestError::Invalid(message))
            if message.contains("package-root identity graph entry `PublicRecord` is not backed")
    ));

    let mut missing_binding = manifest;
    let alias = missing_binding
        .contract_metadata
        .api
        .as_mut()
        .and_then(|api| {
            api.modules
                .iter_mut()
                .find(|module| module.module_path == ["lib".to_string()])
        })
        .and_then(|module| module.declarations.first_mut())
        .ok_or("missing nominal reexport API binding fixture")?;
    let ApiDeclaration::Alias(alias) = alias else {
        return Err("nominal reexport API binding fixture has the wrong kind".into());
    };
    alias.target_path = vec!["domain".to_string(), "Other".to_string()];
    let error = missing_binding.write_to_path(&tmp.path().join("root-nominal-reexport-missing-binding.incnlib"));
    assert!(matches!(
        error,
        Err(LibraryManifestError::Invalid(message))
            if message.contains("package-root identity graph entry `PublicRecord` is not backed")
    ));
    Ok(())
}

#[test]
fn manifest_accepts_public_rusttype_identity_and_rejects_newtype_kind_disagreement()
-> Result<(), Box<dyn std::error::Error>> {
    use crate::frontend::library_exports::{
        CheckedExportIdentity, CheckedExportKind, CheckedNamedExport, CheckedNewtypeExport,
    };
    use crate::frontend::symbols::ResolvedType;

    let checked = CheckedNamedExport {
        name: "Handle".to_string(),
        identity: CheckedExportIdentity::direct(vec!["ffi".to_string(), "Handle".to_string()]).with_canonical(Some(
            source_identity(
                &["ffi"],
                "Handle",
                incan_semantics_core::SemanticSourceTargetKind::Rusttype,
                10,
                20,
            ),
        )),
        kind: CheckedExportKind::Newtype(CheckedNewtypeExport {
            name: "Handle".to_string(),
            type_params: Vec::new(),
            traits: Vec::new(),
            trait_adoptions: Vec::new(),
            derives: Vec::new(),
            is_rusttype: true,
            underlying: ResolvedType::RustPath("crate::Handle".to_string()),
            checked_constructor: None,
            constraints: Vec::new(),
            implicit_coercion_enabled: true,
            methods: Vec::new(),
        }),
    };
    let mut manifest = LibraryManifest::from_checked_exports("ffi_lib", "0.1.0", &[checked]);
    manifest.contract_metadata.api = Some(CheckedApiMetadataPackage {
        schema_version: CHECKED_API_METADATA_SCHEMA_VERSION,
        package: None,
        modules: vec![CheckedApiMetadata {
            schema_version: CHECKED_API_METADATA_SCHEMA_VERSION,
            module_path: vec!["ffi".to_string()],
            declarations: vec![ApiDeclaration::Newtype(ApiNewtype {
                name: "Handle".to_string(),
                anchor: SourceAnchor {
                    id: "ffi::Handle".to_string(),
                    span: SourceSpan { start: 10, end: 20 },
                },
                docstring: None,
                docstring_sections: None,
                decorators: Vec::new(),
                type_params: Vec::new(),
                traits: Vec::new(),
                trait_adoptions: Vec::new(),
                derives: Vec::new(),
                is_rusttype: true,
                underlying: TypeRef::RustPath {
                    path: "crate::Handle".to_string(),
                },
                checked_constructor: None,
                constraints: Vec::new(),
                implicit_coercion_enabled: true,
                methods: Vec::new(),
            })],
        }],
        public_namespaces: Vec::new(),
    });
    let tmp = tempfile::tempdir()?;
    manifest.write_to_path(&tmp.path().join("rusttype.incnlib"))?;

    let mut wrong_kind = manifest.clone();
    wrong_kind
        .contract_metadata
        .identity_graph
        .exports
        .first_mut()
        .ok_or("missing rusttype graph fixture")?
        .canonical
        .as_mut()
        .ok_or("missing rusttype canonical fixture")?
        .kind = "newtype".to_string();
    let error = wrong_kind.write_to_path(&tmp.path().join("rusttype-as-newtype.incnlib"));
    assert!(matches!(
        error,
        Err(LibraryManifestError::Invalid(message))
            if message.contains("canonical kind `newtype` instead of `rusttype`")
    ));

    let mut wrong_api_kind = manifest;
    let api_declaration = wrong_api_kind
        .contract_metadata
        .api
        .as_mut()
        .ok_or("missing rusttype API fixture")?
        .modules
        .first_mut()
        .and_then(|module| module.declarations.first_mut())
        .ok_or("missing rusttype API declaration fixture")?;
    let ApiDeclaration::Newtype(api_newtype) = api_declaration else {
        return Err("rusttype API fixture has the wrong declaration kind".into());
    };
    api_newtype.is_rusttype = false;
    let error = wrong_api_kind.write_to_path(&tmp.path().join("rusttype-api-as-newtype.incnlib"));
    assert!(matches!(
        error,
        Err(LibraryManifestError::Invalid(message))
            if message.contains("package-root identity graph entry `Handle` is not backed")
    ));
    Ok(())
}

#[test]
fn manifest_accepts_producer_callable_alias_kinds_and_rejects_non_callable_metadata()
-> Result<(), Box<dyn std::error::Error>> {
    use crate::frontend::library_exports::{CheckedExportKind, collect_checked_public_exports};
    use crate::frontend::typechecker::TypeChecker;

    let source = r#"
pub def route(method: str) -> str:
  return method

pub get = partial route(method="GET")
pub route_alias = alias route
pub fast_get = alias get
pub count_items = alias len
"#;
    let tokens = crate::frontend::lexer::lex(source)
        .map_err(|errors| std::io::Error::other(format!("callable alias fixture lex failed: {errors:?}")))?;
    let program = crate::frontend::parser::parse(&tokens)
        .map_err(|errors| std::io::Error::other(format!("callable alias fixture parse failed: {errors:?}")))?;
    let mut checker = TypeChecker::new();
    checker
        .check_program(&program)
        .map_err(|errors| std::io::Error::other(format!("callable alias fixture check failed: {errors:?}")))?;
    let exports = collect_checked_public_exports(&program, &checker);
    for (name, expected_kind) in [
        ("route_alias", incan_semantics_core::SemanticSourceTargetKind::Function),
        ("fast_get", incan_semantics_core::SemanticSourceTargetKind::Partial),
        ("count_items", incan_semantics_core::SemanticSourceTargetKind::Builtin),
    ] {
        let alias = exports
            .iter()
            .find(|export| export.name == name)
            .ok_or_else(|| std::io::Error::other(format!("missing checked callable alias `{name}`")))?;
        let CheckedExportKind::Alias(alias_export) = &alias.kind else {
            return Err(
                std::io::Error::other(format!("checked callable alias `{name}` has the wrong export kind")).into(),
            );
        };
        assert!(
            alias_export.projected_function.is_some(),
            "checked callable alias `{name}` must carry callable metadata"
        );
        assert_eq!(
            alias.identity.canonical.as_ref().map(|identity| &identity.kind),
            Some(&expected_kind)
        );
    }

    let manifest = LibraryManifest::from_checked_exports("routes_lib", "0.1.0", &exports);
    let tmp = tempfile::tempdir()?;
    manifest.write_to_path(&tmp.path().join("callable-aliases.incnlib"))?;

    let mut missing_builtin_callable = manifest.clone();
    missing_builtin_callable
        .exports
        .aliases
        .iter_mut()
        .find(|alias| alias.name == "count_items")
        .ok_or("missing builtin alias fixture")?
        .projected_function = None;
    let missing_builtin_error =
        missing_builtin_callable.write_to_path(&tmp.path().join("builtin-alias-without-callable-metadata.incnlib"));
    assert!(matches!(
        missing_builtin_error,
        Err(LibraryManifestError::Invalid(message))
            if message.contains("canonical callable target without callable metadata")
    ));

    let mut wrong_kind = manifest;
    wrong_kind
        .contract_metadata
        .identity_graph
        .exports
        .iter_mut()
        .find(|entry| entry.public_name == "fast_get")
        .ok_or("missing callable alias graph entry fixture")?
        .canonical
        .as_mut()
        .ok_or("missing callable alias identity fixture")?
        .kind = "model".to_string();
    let wrong_kind_error = wrong_kind.write_to_path(&tmp.path().join("non-callable-alias-metadata.incnlib"));
    assert!(matches!(
        wrong_kind_error,
        Err(LibraryManifestError::Invalid(message))
            if message.contains("callable metadata for non-callable canonical kind `model`")
    ));

    Ok(())
}

#[test]
fn manifest_writer_rejects_malformed_and_duplicate_canonical_identities() -> Result<(), Box<dyn std::error::Error>> {
    use crate::frontend::library_exports::{
        CheckedAliasExport, CheckedExportIdentity, CheckedExportKind, CheckedFunctionExport, CheckedNamedExport,
    };
    use crate::frontend::symbols::ResolvedType;

    let checked = CheckedNamedExport {
        name: "parse".to_string(),
        identity: CheckedExportIdentity::direct(vec!["codec".to_string(), "parse".to_string()]).with_canonical(Some(
            source_identity(
                &["codec"],
                "parse",
                incan_semantics_core::SemanticSourceTargetKind::Function,
                10,
                20,
            ),
        )),
        kind: CheckedExportKind::Function(CheckedFunctionExport {
            name: "parse".to_string(),
            emitted_name: None,
            type_params: Vec::new(),
            params: Vec::new(),
            param_defaults: Vec::new(),
            return_type: ResolvedType::Int,
            is_async: false,
        }),
    };
    let tmp = tempfile::tempdir()?;

    let mut malformed = LibraryManifest::from_checked_exports("codec_lib", "0.1.0", std::slice::from_ref(&checked));
    let malformed_identity = malformed
        .contract_metadata
        .identity_graph
        .exports
        .first_mut()
        .and_then(|entry| entry.canonical.as_mut())
        .ok_or("missing canonical identity fixture")?;
    malformed_identity.kind = "not_a_semantic_kind".to_string();
    let malformed_error = malformed.write_to_path(&tmp.path().join("malformed.incnlib"));
    assert!(matches!(
        malformed_error,
        Err(LibraryManifestError::Invalid(message))
            if message.contains("unknown canonical declaration kind `not_a_semantic_kind`")
    ));

    let mut wrong_known_kind =
        LibraryManifest::from_checked_exports("codec_lib", "0.1.0", std::slice::from_ref(&checked));
    wrong_known_kind.contract_metadata.identity_graph.exports[0]
        .canonical
        .as_mut()
        .ok_or("missing canonical identity fixture")?
        .kind = "const".to_string();
    let wrong_known_kind_error = wrong_known_kind.write_to_path(&tmp.path().join("wrong-known-kind.incnlib"));
    assert!(matches!(
        wrong_known_kind_error,
        Err(LibraryManifestError::Invalid(message))
            if message.contains("canonical kind `const` instead of `function`")
    ));

    // A source path that names a different *declaration* is still rejected. The module prefix in front of it is
    // deliberately not checked: a facade re-export, a sibling import inside a nested module, a `super`-relative
    // import, and each hop of a re-export chain all record a prefix that differs from the resolved identity's module
    // while naming the same declaration. Requiring prefix equality rejected all of those valid programs, so the
    // module a path is spelled against can no longer be validated here -- only the declaration it names.
    let mut wrong_source = LibraryManifest::from_checked_exports("codec_lib", "0.1.0", std::slice::from_ref(&checked));
    wrong_source.contract_metadata.identity_graph.exports[0].source_path =
        vec!["codec".to_string(), "not_parse".to_string()];
    let wrong_source_error = wrong_source.write_to_path(&tmp.path().join("wrong-source.incnlib"));
    assert!(
        matches!(&wrong_source_error, Err(LibraryManifestError::Invalid(message)) if
            message.contains("does not name its canonical declaration")
                || message.contains("canonical identity disagrees with its authoritative source/projection path")),
        "a source path naming a different declaration must be rejected, got: {wrong_source_error:?}"
    );

    let mut builtin_direct =
        LibraryManifest::from_checked_exports("codec_lib", "0.1.0", std::slice::from_ref(&checked));
    builtin_direct.contract_metadata.identity_graph.exports[0].source_path = vec!["parse".to_string()];
    builtin_direct.contract_metadata.identity_graph.exports[0]
        .canonical
        .as_mut()
        .ok_or("missing builtin direct identity fixture")?
        .origin = CanonicalIdentityOriginExport::Builtin;
    let builtin_direct_error = builtin_direct.write_to_path(&tmp.path().join("builtin-direct.incnlib"));
    assert!(matches!(
        builtin_direct_error,
        Err(LibraryManifestError::Invalid(message))
            if message.contains("canonical origin outside manifest package `codec_lib`")
    ));

    let mut rust_direct = LibraryManifest::from_checked_exports("codec_lib", "0.1.0", std::slice::from_ref(&checked));
    rust_direct.contract_metadata.identity_graph.exports[0].source_path =
        vec!["rust".to_string(), "codec".to_string(), "parse".to_string()];
    rust_direct.contract_metadata.identity_graph.exports[0]
        .canonical
        .as_mut()
        .ok_or("missing Rust direct identity fixture")?
        .origin = CanonicalIdentityOriginExport::RustCrate {
        path: vec!["codec".to_string()],
    };
    let rust_direct_error = rust_direct.write_to_path(&tmp.path().join("rust-direct.incnlib"));
    assert!(matches!(
        rust_direct_error,
        Err(LibraryManifestError::Invalid(message))
            if message.contains("canonical origin outside manifest package `codec_lib`")
    ));

    let mut external_direct =
        LibraryManifest::from_checked_exports("codec_lib", "0.1.0", std::slice::from_ref(&checked));
    external_direct.contract_metadata.identity_graph.exports[0].source_path = vec![
        "pub".to_string(),
        "dependency".to_string(),
        "codec".to_string(),
        "parse".to_string(),
    ];
    external_direct.contract_metadata.identity_graph.exports[0]
        .canonical
        .as_mut()
        .ok_or("missing external direct identity fixture")?
        .origin = CanonicalIdentityOriginExport::Package {
        library: "dependency".to_string(),
        module_path: vec!["codec".to_string()],
    };
    let external_direct_error = external_direct.write_to_path(&tmp.path().join("external-direct.incnlib"));
    assert!(matches!(
        external_direct_error,
        Err(LibraryManifestError::Invalid(message))
            if message.contains("canonical origin outside manifest package `codec_lib`")
    ));

    let mut missing_graph = LibraryManifest::from_checked_exports("codec_lib", "0.1.0", std::slice::from_ref(&checked));
    missing_graph.contract_metadata.identity_graph.exports.clear();
    let missing_graph_error = missing_graph.write_to_path(&tmp.path().join("missing-graph.incnlib"));
    assert!(matches!(
        missing_graph_error,
        Err(LibraryManifestError::Invalid(message))
            if message.contains("publishes 0 root Function identities named `parse` for 1 raw declarations")
    ));

    let mut duplicate = LibraryManifest::from_checked_exports("codec_lib", "0.1.0", std::slice::from_ref(&checked));
    let duplicate_entry = duplicate
        .contract_metadata
        .identity_graph
        .exports
        .first()
        .cloned()
        .ok_or("missing duplicate identity fixture")?;
    duplicate.contract_metadata.identity_graph.exports.push(duplicate_entry);
    let duplicate_error = duplicate.write_to_path(&tmp.path().join("duplicate.incnlib"));
    assert!(matches!(
        duplicate_error,
        Err(LibraryManifestError::Invalid(message))
            if message.contains("duplicate canonical export `parse`")
    ));

    let mut extra_root = LibraryManifest::from_checked_exports("codec_lib", "0.1.0", &[checked]);
    let mut extra_entry = extra_root.contract_metadata.identity_graph.exports[0].clone();
    extra_entry.public_name = "fabricated".to_string();
    extra_entry.public_path = vec!["codec_lib".to_string(), "fabricated".to_string()];
    extra_entry.source_path = vec!["codec".to_string(), "fabricated".to_string()];
    extra_entry
        .canonical
        .as_mut()
        .ok_or("missing canonical identity fixture")?
        .declaration_name = "fabricated".to_string();
    extra_root.contract_metadata.identity_graph.exports.push(extra_entry);
    let extra_root_error = extra_root.write_to_path(&tmp.path().join("extra-root.incnlib"));
    assert!(matches!(
        extra_root_error,
        Err(LibraryManifestError::Invalid(message))
            if message.contains("unbacked root Function identities named `fabricated`")
    ));

    let alias = CheckedNamedExport {
        name: "safe_parse".to_string(),
        identity: CheckedExportIdentity::alias(
            vec!["codec".to_string(), "safe_parse".to_string()],
            vec!["codec".to_string(), "parse".to_string()],
        )
        .with_canonical(Some(source_identity(
            &["codec"],
            "parse",
            incan_semantics_core::SemanticSourceTargetKind::Function,
            10,
            20,
        ))),
        kind: CheckedExportKind::Alias(CheckedAliasExport {
            name: "safe_parse".to_string(),
            target_path: vec!["codec".to_string(), "parse".to_string()],
            projected_function: Some(CheckedFunctionExport {
                name: "safe_parse".to_string(),
                emitted_name: None,
                type_params: Vec::new(),
                params: Vec::new(),
                param_defaults: Vec::new(),
                return_type: ResolvedType::Int,
                is_async: false,
            }),
        }),
    };
    let mut mismatched_callable_alias =
        LibraryManifest::from_checked_exports("codec_lib", "0.1.0", std::slice::from_ref(&alias));
    mismatched_callable_alias.exports.aliases[0]
        .projected_function
        .as_mut()
        .ok_or("missing callable alias projection fixture")?
        .name = "parse".to_string();
    let mismatched_callable_alias_error =
        mismatched_callable_alias.write_to_path(&tmp.path().join("mismatched-callable-alias.incnlib"));
    assert!(matches!(
        mismatched_callable_alias_error,
        Err(LibraryManifestError::Invalid(message))
            if message.contains("callable projection is named `parse` instead of `safe_parse`")
    ));

    let mut missing_callable_alias =
        LibraryManifest::from_checked_exports("codec_lib", "0.1.0", std::slice::from_ref(&alias));
    missing_callable_alias.exports.aliases[0].projected_function = None;
    let missing_callable_alias_error =
        missing_callable_alias.write_to_path(&tmp.path().join("missing-callable-alias.incnlib"));
    assert!(matches!(
        missing_callable_alias_error,
        Err(LibraryManifestError::Invalid(message))
            if message.contains("canonical callable target without callable metadata")
    ));

    let mut mismatched_alias = LibraryManifest::from_checked_exports("codec_lib", "0.1.0", &[alias]);
    mismatched_alias.exports.aliases[0].target_path = vec!["codec".to_string(), "other".to_string()];
    let mismatched_alias_error = mismatched_alias.write_to_path(&tmp.path().join("mismatched-alias.incnlib"));
    assert!(matches!(
        mismatched_alias_error,
        Err(LibraryManifestError::Invalid(message))
            if message.contains("projection disagrees with its raw export")
    ));

    let non_callable_alias = CheckedNamedExport {
        name: "model_parse".to_string(),
        identity: CheckedExportIdentity::alias(
            vec!["codec".to_string(), "model_parse".to_string()],
            vec!["codec".to_string(), "parse".to_string()],
        )
        .with_canonical(Some(source_identity(
            &["codec"],
            "parse",
            incan_semantics_core::SemanticSourceTargetKind::Model,
            10,
            20,
        ))),
        kind: CheckedExportKind::Alias(CheckedAliasExport {
            name: "model_parse".to_string(),
            target_path: vec!["codec".to_string(), "parse".to_string()],
            projected_function: Some(CheckedFunctionExport {
                name: "model_parse".to_string(),
                emitted_name: None,
                type_params: Vec::new(),
                params: Vec::new(),
                param_defaults: Vec::new(),
                return_type: ResolvedType::Int,
                is_async: false,
            }),
        }),
    };
    let non_callable_alias = LibraryManifest::from_checked_exports("codec_lib", "0.1.0", &[non_callable_alias]);
    let non_callable_alias_error = non_callable_alias.write_to_path(&tmp.path().join("non-callable-alias.incnlib"));
    assert!(matches!(
        non_callable_alias_error,
        Err(LibraryManifestError::Invalid(message))
            if message.contains("callable metadata for non-callable canonical kind `model`")
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
    use crate::frontend::library_exports::{
        CheckedExportIdentity, CheckedExportKind, CheckedNamedExport, CheckedNewtypeExport,
    };
    use crate::frontend::symbols::ResolvedType;

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
            name: "BId".to_string()
        }
    );
    Ok(())
}

#[test]
fn parameter_default_materializability_is_all_or_nothing() {
    let empty_call = ParamDefaultExport::Call {
        path: Vec::new(),
        args: Vec::new(),
        signature: None,
    };
    let partially_unsupported_list =
        ParamDefaultExport::List(vec![ParamDefaultExport::Int(1), ParamDefaultExport::Unsupported]);
    let partially_unsupported_dict = ParamDefaultExport::Dict(vec![ParamDefaultDictEntryExport {
        key: ParamDefaultExport::String("key".to_string()),
        value: ParamDefaultExport::Unsupported,
    }]);
    let partially_unsupported_call = ParamDefaultExport::Call {
        path: vec!["fallback".to_string()],
        args: vec![ParamDefaultCallArgExport {
            name: None,
            value: ParamDefaultExport::Unsupported,
        }],
        signature: None,
    };

    assert!(!empty_call.is_materializable());
    assert!(!partially_unsupported_list.is_materializable());
    assert!(!partially_unsupported_dict.is_materializable());
    assert!(!partially_unsupported_call.is_materializable());
}

#[test]
fn manifest_io_round_trip_preserves_rust_abi_metadata() -> Result<(), Box<dyn std::error::Error>> {
    use incan_core::interop::{
        RustFunctionSig, RustItemKind, RustItemMetadata, RustParam, RustTypeInfo, RustVisibility,
    };

    let mut manifest = LibraryManifest::new("mylib", "0.1.0");
    manifest.rust_abi = LibraryRustAbi::from_items(vec![
        RustItemMetadata {
            canonical_path: "mylib_runtime::parse".to_string(),
            definition_path: Some("mylib_runtime::parse".to_string()),
            visibility: RustVisibility::Public,
            kind: RustItemKind::Function(RustFunctionSig {
                type_params: Vec::new(),
                params: vec![RustParam {
                    name: Some("source".to_string()),
                    type_display: "&str".to_string(),
                }],
                return_type: "Result<mylib_runtime::Plan, mylib_runtime::Error>".to_string(),
                is_async: true,
                is_unsafe: false,
            }),
        },
        RustItemMetadata {
            canonical_path: "mylib_runtime::Factory".to_string(),
            definition_path: Some("mylib_runtime::Factory".to_string()),
            visibility: RustVisibility::Public,
            kind: RustItemKind::Type(RustTypeInfo {
                type_params: vec!["T".to_string()],
                type_param_defaults: Vec::new(),
                mutable_reference_type_params: Vec::new(),
                expanded_derive_traits: Vec::new(),
                has_const_params: false,
                alias_target: None,
                metadata_completeness: Default::default(),
                methods: Vec::new(),
                implemented_traits: Vec::new(),
                fields: Vec::new(),
                variants: Vec::new(),
            }),
        },
    ]);

    let tmp = tempfile::tempdir()?;
    let path = tmp.path().join("mylib.incnlib");
    manifest.write_to_path(&path)?;
    let loaded = LibraryManifest::read_from_path(&path)?;

    assert_eq!(loaded, manifest);
    let factory = loaded
        .rust_abi
        .as_ref()
        .and_then(|abi| abi.get("mylib_runtime::Factory"))
        .ok_or("expected receiver-generic Rust type metadata")?;
    let RustItemKind::Type(factory) = &factory.kind else {
        return Err("expected Rust type metadata".into());
    };
    assert_eq!(factory.type_params, ["T"]);
    Ok(())
}

#[test]
fn manifest_validation_rejects_invalid_partial_exports() -> Result<(), Box<dyn std::error::Error>> {
    let mut base = LibraryManifest::new("mylib", "0.1.0");
    base.exports.partials.push(PartialExport {
        name: "get".to_string(),
        target_path: vec!["route".to_string()],
        target_kind: PartialTargetKindExport::Function,
        presets: vec![PartialPresetExport {
            name: "method".to_string(),
            ty: TypeRef::Named {
                name: "str".to_string(),
            },
            value: PresetValueExport::String("GET".to_string()),
        }],
        type_params: Vec::new(),
        params: Vec::new(),
        return_type: TypeRef::Named {
            name: "str".to_string(),
        },
        is_async: false,
    });

    for (manifest, expected) in [
        {
            let mut manifest = base.clone();
            manifest.exports.partials[0].presets.clear();
            (manifest, "must declare at least one preset")
        },
        {
            let mut manifest = base.clone();
            let duplicate = manifest.exports.partials[0].presets[0].clone();
            manifest.exports.partials[0].presets.push(duplicate);
            (manifest, "repeats preset `method`")
        },
    ] {
        let tmp = tempfile::tempdir()?;
        let path = tmp.path().join("invalid-partials.incnlib");
        let err = manifest
            .write_to_path(&path)
            .expect_err("invalid partial manifest should fail validation");
        assert!(
            err.to_string().contains(expected),
            "expected validation error containing `{expected}`, got `{err}`"
        );
    }
    Ok(())
}

#[test]
fn manifest_validation_rejects_duplicate_rust_abi_paths() -> Result<(), Box<dyn std::error::Error>> {
    use incan_core::interop::{RustItemKind, RustItemMetadata, RustModuleInfo, RustVisibility};

    let duplicate = RustItemMetadata {
        canonical_path: "mylib_runtime::Plan".to_string(),
        definition_path: None,
        visibility: RustVisibility::Public,
        kind: RustItemKind::Module(RustModuleInfo { children: Vec::new() }),
    };
    let raw = format!(
        r#"{{
  "name": "mylib",
  "version": "0.1.0",
  "incan_version": "{}",
  "manifest_format": {},
  "exports": {{}},
  "soft_keywords": {{}},
  "rust_abi": {{
    "schema_version": {},
    "items": [{}, {}]
  }}
}}"#,
        crate::version::INCAN_VERSION,
        LIBRARY_MANIFEST_FORMAT,
        RUST_ABI_SCHEMA_VERSION,
        serde_json::to_string(&duplicate)?,
        serde_json::to_string(&duplicate)?
    );

    let err = LibraryManifest::from_json_str(&raw);
    assert!(err.is_err(), "expected duplicate Rust ABI metadata to fail");
    Ok(())
}

#[test]
fn manifest_validation_rejects_stale_and_future_rust_abi_schema_versions() {
    for unsupported in [1, RUST_ABI_SCHEMA_VERSION + 1] {
        let raw = format!(
            r#"{{
  "name": "mylib",
  "version": "0.1.0",
  "incan_version": "{}",
  "manifest_format": {},
  "exports": {{}},
  "soft_keywords": {{}},
  "rust_abi": {{
    "schema_version": {},
    "items": []
  }}
}}"#,
            crate::version::INCAN_VERSION,
            LIBRARY_MANIFEST_FORMAT,
            unsupported
        );

        let err = LibraryManifest::from_json_str(&raw);
        assert!(
            matches!(
                err,
                Err(LibraryManifestError::Invalid(ref message))
                    if message.contains(&format!("rust_abi.schema_version {unsupported} is unsupported"))
            ),
            "expected unsupported Rust ABI schema {unsupported} to fail, got {err:?}"
        );
    }
}

#[test]
fn manifest_validation_rejects_unsupported_api_metadata_package_schema_version() {
    let raw = format!(
        r#"{{
  "name": "mylib",
  "version": "0.1.0",
  "incan_version": "{}",
  "manifest_format": {},
  "exports": {{}},
  "soft_keywords": {{}},
  "contract_metadata": {{
    "api": {{
      "schema_version": {},
      "package": null,
      "modules": []
    }}
  }}
}}"#,
        crate::version::INCAN_VERSION,
        LIBRARY_MANIFEST_FORMAT,
        crate::frontend::api_metadata::CHECKED_API_METADATA_SCHEMA_VERSION + 1
    );

    let err = LibraryManifest::from_json_str(&raw);
    assert!(err.is_err(), "expected unsupported API metadata schema to fail");
}

#[test]
fn manifest_validation_rejects_unsupported_api_metadata_module_schema_version() {
    let raw = format!(
        r#"{{
  "name": "mylib",
  "version": "0.1.0",
  "incan_version": "{}",
  "manifest_format": {},
  "exports": {{}},
  "soft_keywords": {{}},
  "contract_metadata": {{
    "api": {{
      "schema_version": {},
      "package": null,
      "modules": [
        {{
          "schema_version": {},
          "module_path": ["lib"],
          "declarations": []
        }}
      ]
    }}
  }}
}}"#,
        crate::version::INCAN_VERSION,
        LIBRARY_MANIFEST_FORMAT,
        crate::frontend::api_metadata::CHECKED_API_METADATA_SCHEMA_VERSION,
        crate::frontend::api_metadata::CHECKED_API_METADATA_SCHEMA_VERSION + 1
    );

    let err = LibraryManifest::from_json_str(&raw);
    assert!(err.is_err(), "expected unsupported API metadata module schema to fail");
}

#[test]
fn manifest_io_round_trip_preserves_rest_parameter_metadata() -> Result<(), Box<dyn std::error::Error>> {
    let mut manifest = legacy_manifest_fixture("mylib", "0.1.0");
    manifest.exports.functions.push(FunctionExport {
        name: "collect".to_string(),
        emitted_name: None,
        type_params: Vec::new(),
        params: vec![
            ParamExport {
                name: "items".to_string(),
                ty: TypeRef::Named {
                    name: "int".to_string(),
                },
                kind: ParamKindExport::RestPositional,
                has_default: false,
                default: None,
            },
            ParamExport {
                name: "labels".to_string(),
                ty: TypeRef::Named {
                    name: "str".to_string(),
                },
                kind: ParamKindExport::RestKeyword,
                has_default: false,
                default: None,
            },
        ],
        return_type: TypeRef::Named {
            name: "int".to_string(),
        },
        is_async: false,
    });
    manifest.exports.classes.push(ClassExport {
        name: "Collector".to_string(),
        type_params: Vec::new(),
        extends: None,
        traits: Vec::new(),
        trait_adoptions: Vec::new(),
        derives: Vec::new(),
        fields: Vec::new(),
        properties: Vec::new(),
        methods: vec![MethodExport {
            alias_of: None,
            name: "collect".to_string(),
            canonical: None,
            type_params: Vec::new(),
            receiver: Some(ReceiverExport::Immutable),
            params: vec![ParamExport {
                name: "items".to_string(),
                ty: TypeRef::Named {
                    name: "int".to_string(),
                },
                kind: ParamKindExport::RestPositional,
                has_default: false,
                default: None,
            }],
            return_type: TypeRef::Named {
                name: "int".to_string(),
            },
            is_async: false,
            has_body: true,
        }],
    });

    let tmp = tempfile::tempdir()?;
    let path = tmp.path().join("rest_params.incnlib");
    manifest.write_to_path(&path)?;
    let loaded = LibraryManifest::read_from_path(&path)?;

    assert_eq!(loaded, manifest);
    Ok(())
}

#[test]
fn manifest_validation_rejects_invalid_rest_parameter_metadata() -> Result<(), Box<dyn std::error::Error>> {
    let mut manifest = LibraryManifest::new("mylib", "0.1.0");
    manifest.exports.functions.push(FunctionExport {
        name: "bad_collect".to_string(),
        emitted_name: None,
        type_params: Vec::new(),
        params: vec![
            ParamExport {
                name: "labels".to_string(),
                ty: TypeRef::Named {
                    name: "str".to_string(),
                },
                kind: ParamKindExport::RestKeyword,
                has_default: false,
                default: None,
            },
            ParamExport {
                name: "value".to_string(),
                ty: TypeRef::Named {
                    name: "int".to_string(),
                },
                kind: ParamKindExport::Normal,
                has_default: false,
                default: None,
            },
        ],
        return_type: TypeRef::Named {
            name: "int".to_string(),
        },
        is_async: false,
    });

    let tmp = tempfile::tempdir()?;
    let path = tmp.path().join("invalid_rest_params.incnlib");
    let err = manifest
        .write_to_path(&path)
        .expect_err("expected invalid rest parameter metadata to fail validation");
    assert!(
        err.to_string()
            .contains("cannot appear after a `**kwargs` rest parameter"),
        "unexpected validation error: {err}"
    );
    Ok(())
}

#[test]
fn manifest_io_round_trip_preserves_trait_supertraits() -> Result<(), Box<dyn std::error::Error>> {
    let mut manifest = legacy_manifest_fixture("mylib", "0.1.0");
    manifest.exports.traits.push(TraitExport {
        name: "Ord".to_string(),
        source_name: None,
        type_params: Vec::new(),
        supertraits: vec![TypeBoundExport {
            name: "Eq".to_string(),
            source_name: None,
            module_path: None,
            type_args: Vec::new(),
            implementation_type_params: Vec::new(),
        }],
        requires: Vec::new(),
        methods: Vec::new(),
    });

    let tmp = tempfile::tempdir()?;
    let path = tmp.path().join("traits.incnlib");
    manifest.write_to_path(&path)?;
    let loaded = LibraryManifest::read_from_path(&path)?;

    assert_eq!(loaded, manifest);
    Ok(())
}

#[test]
fn manifest_io_round_trip_preserves_value_enum_metadata() -> Result<(), Box<dyn std::error::Error>> {
    let mut manifest = legacy_manifest_fixture("mylib", "0.1.0");
    manifest.exports.enums.push(EnumExport {
        name: "Status".to_string(),
        type_params: Vec::new(),
        traits: Vec::new(),
        trait_adoptions: Vec::new(),
        value_type: Some(EnumValueTypeExport::Str),
        ordinal_type_identity: Some("mylib.Status".to_string()),
        variants: vec![
            EnumVariantExport {
                name: "Active".to_string(),
                canonical: None,
                fields: Vec::new(),
                value: Some(EnumValueExport::Str("active".to_string())),
            },
            EnumVariantExport {
                name: "Disabled".to_string(),
                canonical: None,
                fields: Vec::new(),
                value: Some(EnumValueExport::Str("disabled".to_string())),
            },
        ],
        variant_aliases: Vec::new(),
        methods: Vec::new(),
        derives: Vec::new(),
    });
    manifest.exports.enums.push(EnumExport {
        name: "HttpStatus".to_string(),
        type_params: Vec::new(),
        traits: Vec::new(),
        trait_adoptions: Vec::new(),
        value_type: Some(EnumValueTypeExport::Int),
        ordinal_type_identity: Some("mylib.HttpStatus".to_string()),
        variants: vec![
            EnumVariantExport {
                name: "Ok".to_string(),
                canonical: None,
                fields: Vec::new(),
                value: Some(EnumValueExport::Int(200)),
            },
            EnumVariantExport {
                name: "NotFound".to_string(),
                canonical: None,
                fields: Vec::new(),
                value: Some(EnumValueExport::Int(404)),
            },
        ],
        variant_aliases: Vec::new(),
        methods: Vec::new(),
        derives: Vec::new(),
    });

    let tmp = tempfile::tempdir()?;
    let path = tmp.path().join("value_enum.incnlib");
    manifest.write_to_path(&path)?;
    let loaded = LibraryManifest::read_from_path(&path)?;

    assert_eq!(loaded, manifest);
    Ok(())
}

#[test]
fn manifest_io_round_trip_preserves_enum_traits_and_methods() -> Result<(), Box<dyn std::error::Error>> {
    let mut manifest = legacy_manifest_fixture("mylib", "0.1.0");
    manifest.exports.enums.push(EnumExport {
        name: "Status".to_string(),
        type_params: Vec::new(),
        traits: vec!["Labelled".to_string()],
        trait_adoptions: Vec::new(),
        value_type: None,
        ordinal_type_identity: None,
        variants: vec![EnumVariantExport {
            name: "Active".to_string(),
            canonical: None,
            fields: Vec::new(),
            value: None,
        }],
        variant_aliases: Vec::new(),
        methods: vec![MethodExport {
            alias_of: None,
            name: "label".to_string(),
            canonical: None,
            type_params: Vec::new(),
            receiver: Some(ReceiverExport::Immutable),
            params: Vec::new(),
            return_type: TypeRef::Named {
                name: "str".to_string(),
            },
            is_async: false,
            has_body: true,
        }],
        derives: Vec::new(),
    });

    let tmp = tempfile::tempdir()?;
    let path = tmp.path().join("enum_methods.incnlib");
    manifest.write_to_path(&path)?;
    let loaded = LibraryManifest::read_from_path(&path)?;

    assert_eq!(loaded, manifest);
    Ok(())
}

#[test]
fn manifest_reader_rejects_incomplete_value_enum_metadata() {
    let content = format!(
        r#"{{
  "name": "mylib",
  "version": "0.1.0",
  "incan_version": "0.1.0",
  "manifest_format": {},
  "exports": {{
    "enums": [
      {{
        "name": "Status",
        "type_params": [],
        "value_type": "str",
        "variants": [
          {{ "name": "Active", "fields": [], "value": "active" }},
          {{ "name": "Disabled", "fields": [] }}
        ],
        "derives": []
      }}
    ]
  }},
  "soft_keywords": {{}}
}}"#,
        LIBRARY_MANIFEST_FORMAT
    );
    let err = LibraryManifest::from_json_str(&content);
    assert!(
        matches!(err, Err(LibraryManifestError::Invalid(ref msg)) if msg.contains("is missing a raw value")),
        "expected missing value enum metadata diagnostic, got {err:?}"
    );
}

#[test]
fn manifest_reader_rejects_mismatched_value_enum_metadata() {
    let content = format!(
        r#"{{
  "name": "mylib",
  "version": "0.1.0",
  "incan_version": "0.1.0",
  "manifest_format": {},
  "exports": {{
    "enums": [
      {{
        "name": "Status",
        "type_params": [],
        "value_type": "int",
        "variants": [
          {{ "name": "Active", "fields": [], "value": "active" }}
        ],
        "derives": []
      }}
    ]
  }},
  "soft_keywords": {{}}
}}"#,
        LIBRARY_MANIFEST_FORMAT
    );
    let err = LibraryManifest::from_json_str(&content);
    assert!(
        matches!(err, Err(LibraryManifestError::Invalid(ref msg)) if msg.contains("does not match backing type `int`")),
        "expected mismatched value enum metadata diagnostic, got {err:?}"
    );
}

#[test]
fn manifest_reader_rejects_duplicate_value_enum_metadata() {
    let content = format!(
        r#"{{
  "name": "mylib",
  "version": "0.1.0",
  "incan_version": "0.1.0",
  "manifest_format": {},
  "exports": {{
    "enums": [
      {{
        "name": "Status",
        "type_params": [],
        "value_type": "str",
        "variants": [
          {{ "name": "Active", "fields": [], "value": "active" }},
          {{ "name": "Enabled", "fields": [], "value": "active" }}
        ],
        "derives": []
      }}
    ]
  }},
  "soft_keywords": {{}}
}}"#,
        LIBRARY_MANIFEST_FORMAT
    );
    let err = LibraryManifest::from_json_str(&content);
    assert!(
        matches!(err, Err(LibraryManifestError::Invalid(ref msg)) if msg.contains("duplicate raw value `active`")),
        "expected duplicate value enum metadata diagnostic, got {err:?}"
    );
}

#[test]
fn manifest_io_round_trip_preserves_generic_method_type_params() -> Result<(), Box<dyn std::error::Error>> {
    let mut manifest = legacy_manifest_fixture("mylib", "0.1.0");
    manifest.exports.classes.push(ClassExport {
        name: "Box".to_string(),
        type_params: Vec::new(),
        extends: None,
        traits: Vec::new(),
        trait_adoptions: Vec::new(),
        derives: Vec::new(),
        fields: Vec::new(),
        properties: Vec::new(),
        methods: vec![MethodExport {
            alias_of: None,
            name: "get".to_string(),
            canonical: None,
            type_params: vec![TypeParamExport {
                name: "T".to_string(),
                bounds: vec![TypeBoundExport {
                    name: "Clone".to_string(),
                    source_name: None,
                    module_path: None,
                    type_args: Vec::new(),
                    implementation_type_params: Vec::new(),
                }],
            }],
            receiver: Some(ReceiverExport::Immutable),
            params: vec![ParamExport {
                name: "value".to_string(),
                ty: TypeRef::TypeParam { name: "T".to_string() },
                kind: ParamKindExport::Normal,
                has_default: false,
                default: None,
            }],
            return_type: TypeRef::TypeParam { name: "T".to_string() },
            is_async: false,
            has_body: true,
        }],
    });

    let tmp = tempfile::tempdir()?;
    let path = tmp.path().join("classes.incnlib");
    manifest.write_to_path(&path)?;
    let loaded = LibraryManifest::read_from_path(&path)?;

    assert_eq!(loaded, manifest);
    Ok(())
}

#[test]
fn manifest_io_round_trip_preserves_model_and_class_derives() -> Result<(), Box<dyn std::error::Error>> {
    let mut manifest = legacy_manifest_fixture("mylib", "0.1.0");
    manifest.exports.models.push(ModelExport {
        name: "Record".to_string(),
        type_params: Vec::new(),
        traits: Vec::new(),
        trait_adoptions: Vec::new(),
        derives: vec!["Clone".to_string()],
        fields: Vec::new(),
        properties: Vec::new(),
        methods: Vec::new(),
    });
    manifest.exports.classes.push(ClassExport {
        name: "Carrier".to_string(),
        type_params: Vec::new(),
        extends: None,
        traits: Vec::new(),
        trait_adoptions: Vec::new(),
        derives: vec!["Clone".to_string(), "Debug".to_string()],
        fields: Vec::new(),
        properties: Vec::new(),
        methods: Vec::new(),
    });

    let tmp = tempfile::tempdir()?;
    let path = tmp.path().join("derives.incnlib");
    manifest.write_to_path(&path)?;
    let loaded = LibraryManifest::read_from_path(&path)?;

    assert_eq!(loaded, manifest);
    Ok(())
}

#[test]
fn manifest_io_round_trip_preserves_type_trait_adoptions() -> Result<(), Box<dyn std::error::Error>> {
    let mut manifest = legacy_manifest_fixture("mylib", "0.1.0");
    let convert_int = TypeBoundExport {
        name: "Convert".to_string(),
        source_name: None,
        module_path: None,
        type_args: vec![TypeRef::Named {
            name: "int".to_string(),
        }],
        implementation_type_params: vec![ImplementationTypeParamExport {
            name: "R".to_string(),
            bounds: vec![ImplementationTraitBoundExport {
                trait_path: "Clone".to_string(),
                type_args: Vec::new(),
                associated_types: Vec::new(),
                origin: ImplementationTraitBoundOriginExport::Standard,
            }],
        }],
    };
    let convert_float = TypeBoundExport {
        name: "Convert".to_string(),
        source_name: None,
        module_path: None,
        type_args: vec![TypeRef::Named {
            name: "float".to_string(),
        }],
        implementation_type_params: Vec::new(),
    };
    manifest.exports.models.push(ModelExport {
        name: "Record".to_string(),
        type_params: Vec::new(),
        traits: vec!["Convert".to_string(), "Convert".to_string()],
        trait_adoptions: vec![convert_int.clone(), convert_float.clone()],
        derives: Vec::new(),
        fields: Vec::new(),
        properties: Vec::new(),
        methods: Vec::new(),
    });
    manifest.exports.classes.push(ClassExport {
        name: "Carrier".to_string(),
        type_params: Vec::new(),
        extends: None,
        traits: vec!["Decode".to_string()],
        trait_adoptions: vec![TypeBoundExport {
            name: "Decode".to_string(),
            source_name: None,
            module_path: None,
            type_args: vec![TypeRef::Named {
                name: "str".to_string(),
            }],
            implementation_type_params: Vec::new(),
        }],
        derives: Vec::new(),
        fields: Vec::new(),
        properties: Vec::new(),
        methods: Vec::new(),
    });
    manifest.exports.enums.push(EnumExport {
        name: "Token".to_string(),
        type_params: Vec::new(),
        traits: vec!["Convert".to_string(), "Convert".to_string()],
        trait_adoptions: vec![convert_int, convert_float],
        value_type: None,
        ordinal_type_identity: None,
        variants: vec![EnumVariantExport {
            name: "Number".to_string(),
            canonical: None,
            fields: Vec::new(),
            value: None,
        }],
        variant_aliases: Vec::new(),
        methods: Vec::new(),
        derives: Vec::new(),
    });

    let tmp = tempfile::tempdir()?;
    let path = tmp.path().join("trait_adoptions.incnlib");
    manifest.write_to_path(&path)?;
    let loaded = LibraryManifest::read_from_path(&path)?;

    assert_eq!(loaded, manifest);
    Ok(())
}

#[test]
fn manifest_reader_rejects_unknown_manifest_format() -> Result<(), Box<dyn std::error::Error>> {
    let content = r#"{
  "name": "mylib",
  "version": "0.1.0",
  "incan_version": "0.1.0",
  "manifest_format": 999,
  "exports": {},
  "soft_keywords": {}
}"#;

    let err = LibraryManifest::from_json_str(content);
    assert!(err.is_err(), "expected invalid manifest_format to fail");
    Ok(())
}

#[test]
fn compiled_provider_metadata_roundtrips_feature_and_facet_facts() -> Result<(), Box<dyn std::error::Error>> {
    let mut manifest = LibraryManifest::new("reporting", "0.5.0");
    manifest.contract_metadata.provider = CompiledProviderMetadata {
        semantic_source_digest: Some(format!("sha256:{}", "b".repeat(64))),
        namespace_claims: vec![ProviderModuleClaim {
            module_path: vec!["reports".to_string()],
            required_features: BTreeSet::new(),
        }],
        public_features: BTreeMap::from([(
            "json".to_string(),
            ProviderFeatureMetadata {
                optional_dependencies: BTreeSet::from(["serializer".to_string()]),
                ..Default::default()
            },
        )]),
        active_features: BTreeSet::from(["json".to_string()]),
        provider_dependencies: vec![ProviderDependencyMetadata {
            kind: ProviderDependencyKind::PublicPackage,
            dependency_key: "serializer".to_string(),
            provider_name: "serializer_core".to_string(),
            provider_version: "0.5.0".to_string(),
            artifact_digest: format!("sha256:{}", "a".repeat(64)),
            relative_artifact_path: "../../../serializer/target/lib".to_string(),
            requested_features: BTreeSet::from(["json".to_string()]),
            default_features: false,
            optional: true,
        }],
        fact_requirements: vec![ProviderFactRequirement {
            kind: ProviderFactKind::Export,
            identity: "reports.encode".to_string(),
            required_features: BTreeSet::from(["json".to_string()]),
        }],
        implementation_facets: vec![ProviderImplementationFacet {
            id: "json-runtime".to_string(),
            required_modules: BTreeSet::from([vec!["reports".to_string()]]),
            required_features: BTreeSet::from(["json".to_string()]),
            cargo_features: BTreeMap::from([("reporting_runtime".to_string(), BTreeSet::from(["json".to_string()]))]),
            cargo_dependencies: vec![ProviderCargoDependency {
                crate_name: "reporting_runtime".to_string(),
                package: None,
                version: Some("1".to_string()),
                features: BTreeSet::new(),
                default_features: true,
                source: ProviderCargoDependencySource::Registry,
            }],
        }],
        operation_descriptors: vec![ProviderOperationMetadata {
            operation: incan_semantics_core::CanonicalSymbolId::module_declaration(
                vec!["reports".to_string()],
                "emit",
                incan_semantics_core::SemanticSourceTargetKind::Function,
                incan_semantics_core::HirSourceSpan::new(10, 14),
            ),
            required_capability: incan_semantics_core::CanonicalSymbolId::module_declaration(
                vec!["reports".to_string()],
                "publish",
                incan_semantics_core::SemanticSourceTargetKind::Capability,
                incan_semantics_core::HirSourceSpan::new(1, 8),
            ),
            runtime_requirements: vec![incan_semantics_core::AbiV0RuntimeRequirement::HostedStd],
        }],
        ..Default::default()
    };
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("reporting.incnlib");

    manifest.write_to_path(&path)?;
    let loaded = LibraryManifest::read_from_path(&path)?;

    assert_eq!(loaded.contract_metadata.provider, manifest.contract_metadata.provider);
    Ok(())
}

#[test]
fn compiled_provider_metadata_rejects_non_capability_operation_requirements() -> Result<(), Box<dyn std::error::Error>>
{
    let mut manifest = LibraryManifest::new("reporting", "0.5.0");
    manifest
        .contract_metadata
        .provider
        .operation_descriptors
        .push(ProviderOperationMetadata {
            operation: incan_semantics_core::CanonicalSymbolId::module_declaration(
                vec!["reports".to_string()],
                "emit",
                incan_semantics_core::SemanticSourceTargetKind::Function,
                incan_semantics_core::HirSourceSpan::new(10, 14),
            ),
            required_capability: incan_semantics_core::CanonicalSymbolId::module_declaration(
                vec!["reports".to_string()],
                "not_a_capability",
                incan_semantics_core::SemanticSourceTargetKind::Function,
                incan_semantics_core::HirSourceSpan::new(1, 8),
            ),
            runtime_requirements: Vec::new(),
        });

    let dir = tempfile::tempdir()?;
    let error = manifest
        .write_to_path(&dir.path().join("reporting.incnlib"))
        .err()
        .ok_or("a non-capability provider requirement must fail manifest validation")?;
    assert!(
        matches!(error, LibraryManifestError::Invalid(ref message) if message.contains("non-capability requirement")),
        "unexpected validation error: {error}"
    );
    Ok(())
}

#[test]
fn compiled_provider_metadata_rejects_invalid_semantic_source_digest() -> Result<(), Box<dyn std::error::Error>> {
    let mut manifest = LibraryManifest::new("reporting", "0.5.0");
    manifest.contract_metadata.provider.semantic_source_digest = Some("sha256:not-a-digest".to_string());
    let dir = tempfile::tempdir()?;
    let error = manifest
        .write_to_path(&dir.path().join("reporting.incnlib"))
        .err()
        .ok_or("expected invalid provider semantic source digest to fail")?;

    assert!(matches!(error, LibraryManifestError::Invalid(message) if message.contains("provider semantic source")));
    Ok(())
}

#[test]
fn compiled_provider_metadata_rejects_unknown_active_feature() -> Result<(), Box<dyn std::error::Error>> {
    let mut manifest = LibraryManifest::new("reporting", "0.5.0");
    manifest
        .contract_metadata
        .provider
        .active_features
        .insert("missing".to_string());
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("reporting.incnlib");

    let error = manifest
        .write_to_path(&path)
        .err()
        .ok_or("expected invalid provider metadata")?;

    assert!(matches!(error, LibraryManifestError::Invalid(message) if message.contains("missing")));
    Ok(())
}

#[test]
fn compiled_provider_metadata_rejects_absolute_dependency_artifact_path() -> Result<(), Box<dyn std::error::Error>> {
    let mut manifest = LibraryManifest::new("reporting", "0.5.0");
    manifest
        .contract_metadata
        .provider
        .provider_dependencies
        .push(ProviderDependencyMetadata {
            kind: ProviderDependencyKind::PublicPackage,
            dependency_key: "serializer".to_string(),
            provider_name: "serializer_core".to_string(),
            provider_version: "0.5.0".to_string(),
            artifact_digest: format!("sha256:{}", "a".repeat(64)),
            relative_artifact_path: "/producer/serializer/target/lib".to_string(),
            requested_features: BTreeSet::new(),
            default_features: true,
            optional: false,
        });
    let dir = tempfile::tempdir()?;
    let error = manifest
        .write_to_path(&dir.path().join("reporting.incnlib"))
        .err()
        .ok_or("expected absolute provider dependency path to fail")?;

    assert!(matches!(error, LibraryManifestError::Invalid(message) if message.contains("portable relative path")));
    Ok(())
}

#[test]
fn manifest_reader_rejects_pre_checked_newtype_manifest_format() {
    let content = r#"{
  "name": "mylib",
  "version": "0.1.0",
  "incan_version": "0.4.0",
  "manifest_format": 1,
  "exports": {},
  "soft_keywords": {}
}"#;

    let err = LibraryManifest::from_json_str(content);
    assert!(
        matches!(err, Err(LibraryManifestError::Invalid(message)) if message.contains("manifest_format 1")),
        "expected pre-checked-newtype manifest format to be rejected"
    );
}

#[test]
fn manifest_reader_rejects_newer_required_compiler_version() -> Result<(), Box<dyn std::error::Error>> {
    let content = r#"{
  "name": "mylib",
  "version": "0.1.0",
  "incan_version": "999.0.0",
  "manifest_format": 2,
  "exports": {},
  "soft_keywords": {}
}"#;

    let err = LibraryManifest::from_json_str(content);
    assert!(err.is_err(), "expected newer compiler requirement to fail");
    Ok(())
}

#[test]
fn manifest_reader_rejects_invalid_soft_keyword() {
    let content = format!(
        r#"{{
  "name": "mylib",
  "version": "0.1.0",
  "incan_version": "0.1.0",
  "manifest_format": {},
  "exports": {{}},
  "soft_keywords": {{
    "activations": [
      {{ "namespace": "mylib.dsl", "keyword": "not_a_real_keyword" }}
    ]
  }}
}}"#,
        LIBRARY_MANIFEST_FORMAT
    );
    let err = LibraryManifest::from_json_str(&content);
    assert!(
        matches!(err, Err(LibraryManifestError::Invalid(msg)) if msg.contains("unknown soft keyword `not_a_real_keyword`"))
    );
}

#[test]
fn manifest_reader_rejects_hard_keyword_in_soft_keyword_activations() {
    let content = format!(
        r#"{{
  "name": "mylib",
  "version": "0.1.0",
  "incan_version": "0.1.0",
  "manifest_format": {},
  "exports": {{}},
  "soft_keywords": {{
    "activations": [
      {{ "namespace": "mylib.dsl", "keyword": "def" }}
    ]
  }}
}}"#,
        LIBRARY_MANIFEST_FORMAT
    );
    let err = LibraryManifest::from_json_str(&content);
    assert!(
        matches!(err, Err(LibraryManifestError::Invalid(msg)) if msg.contains("keyword `def` is not a soft keyword"))
    );
}

#[test]
fn manifest_io_round_trip_preserves_vocab_payload() -> Result<(), Box<dyn std::error::Error>> {
    let mut manifest = LibraryManifest::new("mylib", "0.1.0");
    manifest.vocab = Some(VocabExports {
        crate_path: "crates/mylib_vocab".to_string(),
        package_name: "mylib_vocab".to_string(),
        keyword_registrations: vec![incan_vocab::KeywordRegistration {
            activation: incan_vocab::KeywordActivation::OnImport {
                namespace: "mylib.dsl".to_string(),
            },
            keywords: vec![incan_vocab::KeywordSpec::new(
                "await",
                incan_vocab::KeywordSurfaceKind::ControlFlow,
            )],
            valid_decorators: vec!["route".to_string()],
        }],
        dsl_surfaces: Vec::new(),
        provider_manifest: incan_vocab::LibraryManifest::default(),
        desugarer_artifact: None,
    });
    manifest.soft_keywords.activations = vec![SoftKeywordActivation {
        namespace: "mylib.dsl".to_string(),
        keyword: "await".to_string(),
    }];

    let tmp = tempfile::tempdir()?;
    let path = tmp.path().join("mylib.incnlib");
    manifest.write_to_path(&path)?;
    let loaded = LibraryManifest::read_from_path(&path)?;

    assert_eq!(loaded, manifest);
    Ok(())
}

#[test]
fn manifest_io_round_trip_preserves_scoped_surface_descriptors() -> Result<(), Box<dyn std::error::Error>> {
    let mut manifest = LibraryManifest::new("mylib", "0.1.0");
    manifest.vocab = Some(VocabExports {
        crate_path: "crates/mylib_vocab".to_string(),
        package_name: "mylib_vocab".to_string(),
        keyword_registrations: Vec::new(),
        dsl_surfaces: vec![
            incan_vocab::DslSurface::on_import("mylib.query")
                .with_declaration(
                    incan_vocab::DeclarationSurface::named("query")
                        .with_clause_body()
                        .desugars_to_expression()
                        .with_clauses([
                            incan_vocab::ClauseSurface::expr("FROM").required(),
                            incan_vocab::ClauseSurface::expr_list("SELECT").required().after("FROM"),
                        ]),
                )
                .with_scoped_surfaces([
                    incan_vocab::ScopedSurfaceDescriptor::operator("query.pipe", "|>")
                        .in_clause_body("query", "SELECT")
                        .with_misuse_scope(incan_vocab::ScopedSurfaceMisuseScope::ActivatingFile)
                        .with_diagnostic(incan_vocab::ScopedSurfaceDiagnosticTemplate::new(
                            "query-pipe-outside-scope",
                            incan_vocab::ScopedSurfaceDiagnosticKind::OutsideScope,
                            "`|>` is only valid inside query SELECT clauses",
                        ))
                        .pairwise_chain(),
                    incan_vocab::ScopedSurfaceDescriptor::leading_dot_path("query.field")
                        .in_clause_body("query", "SELECT")
                        .with_receiver(incan_vocab::ScopedSurfaceReceiver::clause("FROM")),
                    incan_vocab::ScopedSurfaceDescriptor::leading_dot_path("query.arg_field")
                        .with_eligibilities([
                            incan_vocab::ScopedSurfaceEligibility::call_argument("query", "filter"),
                            incan_vocab::ScopedSurfaceEligibility::call_argument("query", "select"),
                        ])
                        .with_receiver(incan_vocab::ScopedSurfaceReceiver::custom("method-receiver")),
                ]),
        ],
        provider_manifest: incan_vocab::LibraryManifest::default(),
        desugarer_artifact: None,
    });

    let tmp = tempfile::tempdir()?;
    let path = tmp.path().join("mylib.incnlib");
    manifest.write_to_path(&path)?;
    let loaded = LibraryManifest::read_from_path(&path)?;

    let Some(loaded_vocab) = loaded.vocab.as_ref() else {
        return Err("expected vocab payload to round-trip".into());
    };
    let scoped_surfaces = &loaded_vocab.dsl_surfaces[0].scoped_surfaces;
    assert_eq!(loaded, manifest);
    assert_eq!(scoped_surfaces.len(), 3);
    assert_eq!(
        scoped_surfaces[0].format_hint.chain_mode,
        incan_vocab::ScopedSurfaceChainMode::Pairwise
    );
    assert_eq!(
        scoped_surfaces[1].receiver,
        Some(incan_vocab::ScopedSurfaceReceiver::clause("FROM"))
    );
    assert_eq!(scoped_surfaces[2].eligible_in[0].call.as_deref(), Some("filter"));
    assert_eq!(
        scoped_surfaces[2].receiver,
        Some(incan_vocab::ScopedSurfaceReceiver::custom("method-receiver"))
    );
    Ok(())
}

#[test]
fn manifest_io_round_trip_preserves_scoped_symbol_descriptors() -> Result<(), Box<dyn std::error::Error>> {
    let mut manifest = LibraryManifest::new("mylib", "0.1.0");
    manifest.vocab = Some(VocabExports {
        crate_path: "crates/mylib_vocab".to_string(),
        package_name: "mylib_vocab".to_string(),
        keyword_registrations: Vec::new(),
        dsl_surfaces: vec![
            incan_vocab::DslSurface::on_import("mylib.query")
                .with_declaration(
                    incan_vocab::DeclarationSurface::named("query")
                        .with_clause_body()
                        .desugars_to_expression()
                        .with_clauses([
                            incan_vocab::ClauseSurface::expr("FROM").required(),
                            incan_vocab::ClauseSurface::expr_list("SELECT").required().after("FROM"),
                        ]),
                )
                .with_scoped_symbols([
                    incan_vocab::ScopedSymbolDescriptor::aggregate("query.sum", "sum")
                        .in_clause_body("query", "SELECT")
                        .with_role(
                            incan_vocab::ScopedSymbolRoleMetadata::new("aggregate.sum")
                                .with_label("Sum")
                                .with_description("Sum aggregate"),
                        )
                        .with_misuse_scope(incan_vocab::ScopedSymbolMisuseScope::ActiveDsl)
                        .with_diagnostic(incan_vocab::ScopedSymbolDiagnosticTemplate::new(
                            "query-sum-outside-select",
                            incan_vocab::ScopedSymbolDiagnosticKind::OutsideEligiblePosition,
                            "`sum` is only a query aggregate inside SELECT clauses",
                        )),
                    incan_vocab::ScopedSymbolDescriptor::aggregate("query.count", "count").with_eligibilities([
                        incan_vocab::ScopedSymbolEligibility::clause_body("query", "SELECT"),
                        incan_vocab::ScopedSymbolEligibility::call_argument("query", "window"),
                    ]),
                ]),
        ],
        provider_manifest: incan_vocab::LibraryManifest::default(),
        desugarer_artifact: None,
    });

    let tmp = tempfile::tempdir()?;
    let path = tmp.path().join("mylib.incnlib");
    manifest.write_to_path(&path)?;
    let loaded = LibraryManifest::read_from_path(&path)?;

    let Some(loaded_vocab) = loaded.vocab.as_ref() else {
        return Err("expected vocab payload to round-trip".into());
    };
    let scoped_symbols = &loaded_vocab.dsl_surfaces[0].scoped_symbols;
    assert_eq!(loaded, manifest);
    assert_eq!(scoped_symbols.len(), 2);
    assert_eq!(scoped_symbols[0].symbol, "sum");
    assert_eq!(scoped_symbols[0].family, incan_vocab::ScopedSymbolFamily::AggregateLike);
    assert_eq!(
        scoped_symbols[0].role.as_ref().map(|role| role.key.as_str()),
        Some("aggregate.sum")
    );
    assert_eq!(scoped_symbols[1].eligible_in[1].call.as_deref(), Some("window"));
    assert_eq!(
        scoped_symbols[0].diagnostics[0].kind,
        incan_vocab::ScopedSymbolDiagnosticKind::OutsideEligiblePosition
    );
    Ok(())
}

#[test]
fn manifest_writer_rejects_empty_scoped_symbol_descriptor_key() -> Result<(), Box<dyn std::error::Error>> {
    let mut manifest = LibraryManifest::new("mylib", "0.1.0");
    manifest.vocab = Some(VocabExports {
        crate_path: "crates/mylib_vocab".to_string(),
        package_name: "mylib_vocab".to_string(),
        keyword_registrations: Vec::new(),
        dsl_surfaces: vec![
            incan_vocab::DslSurface::on_import("mylib.query")
                .with_declaration(
                    incan_vocab::DeclarationSurface::named("query")
                        .with_clause(incan_vocab::ClauseSurface::expr("SELECT")),
                )
                .with_scoped_symbol(
                    incan_vocab::ScopedSymbolDescriptor::aggregate("", "sum").in_clause_body("query", "SELECT"),
                ),
        ],
        provider_manifest: incan_vocab::LibraryManifest::default(),
        desugarer_artifact: None,
    });

    let tmp = tempfile::tempdir()?;
    let err = manifest.write_to_path(&tmp.path().join("mylib.incnlib"));
    assert!(matches!(
        err,
        Err(LibraryManifestError::Invalid(msg)) if msg.contains("vocab scoped symbol descriptor key cannot be empty")
    ));
    Ok(())
}

#[test]
fn manifest_writer_rejects_empty_scoped_symbol_spelling() -> Result<(), Box<dyn std::error::Error>> {
    let mut manifest = LibraryManifest::new("mylib", "0.1.0");
    manifest.vocab = Some(VocabExports {
        crate_path: "crates/mylib_vocab".to_string(),
        package_name: "mylib_vocab".to_string(),
        keyword_registrations: Vec::new(),
        dsl_surfaces: vec![
            incan_vocab::DslSurface::on_import("mylib.query")
                .with_declaration(
                    incan_vocab::DeclarationSurface::named("query")
                        .with_clause(incan_vocab::ClauseSurface::expr("SELECT")),
                )
                .with_scoped_symbol(
                    incan_vocab::ScopedSymbolDescriptor::aggregate("query.sum", "").in_clause_body("query", "SELECT"),
                ),
        ],
        provider_manifest: incan_vocab::LibraryManifest::default(),
        desugarer_artifact: None,
    });

    let tmp = tempfile::tempdir()?;
    let err = manifest.write_to_path(&tmp.path().join("mylib.incnlib"));
    assert!(matches!(
        err,
        Err(LibraryManifestError::Invalid(msg)) if msg.contains("symbol cannot be empty")
    ));
    Ok(())
}

#[test]
fn manifest_writer_rejects_hard_keyword_scoped_symbol_spelling() -> Result<(), Box<dyn std::error::Error>> {
    let mut manifest = LibraryManifest::new("mylib", "0.1.0");
    manifest.vocab = Some(VocabExports {
        crate_path: "crates/mylib_vocab".to_string(),
        package_name: "mylib_vocab".to_string(),
        keyword_registrations: Vec::new(),
        dsl_surfaces: vec![
            incan_vocab::DslSurface::on_import("mylib.query")
                .with_declaration(
                    incan_vocab::DeclarationSurface::named("query")
                        .with_clause(incan_vocab::ClauseSurface::expr("SELECT")),
                )
                .with_scoped_symbol(
                    incan_vocab::ScopedSymbolDescriptor::function("query.from", "from")
                        .in_clause_body("query", "SELECT"),
                ),
        ],
        provider_manifest: incan_vocab::LibraryManifest::default(),
        desugarer_artifact: None,
    });

    let tmp = tempfile::tempdir()?;
    let err = manifest.write_to_path(&tmp.path().join("mylib.incnlib"));
    assert!(matches!(
        err,
        Err(LibraryManifestError::Invalid(msg)) if msg.contains("cannot be a hard keyword")
    ));
    Ok(())
}

#[test]
fn manifest_writer_rejects_malformed_scoped_symbol_eligibility() -> Result<(), Box<dyn std::error::Error>> {
    let mut manifest = LibraryManifest::new("mylib", "0.1.0");
    manifest.vocab = Some(VocabExports {
        crate_path: "crates/mylib_vocab".to_string(),
        package_name: "mylib_vocab".to_string(),
        keyword_registrations: Vec::new(),
        dsl_surfaces: vec![
            incan_vocab::DslSurface::on_import("mylib.query")
                .with_declaration(
                    incan_vocab::DeclarationSurface::named("query")
                        .with_clause(incan_vocab::ClauseSurface::expr("SELECT")),
                )
                .with_scoped_symbol(
                    incan_vocab::ScopedSymbolDescriptor::aggregate("query.sum", "sum").with_eligibility(
                        incan_vocab::ScopedSymbolEligibility {
                            declaration: "query".to_string(),
                            clause: None,
                            call: None,
                            position: incan_vocab::ScopedSymbolPosition::ClauseBody,
                        },
                    ),
                ),
        ],
        provider_manifest: incan_vocab::LibraryManifest::default(),
        desugarer_artifact: None,
    });

    let tmp = tempfile::tempdir()?;
    let err = manifest.write_to_path(&tmp.path().join("mylib.incnlib"));
    assert!(matches!(
        err,
        Err(LibraryManifestError::Invalid(msg)) if msg.contains("clause-body eligibility must declare a clause")
    ));
    Ok(())
}

#[test]
fn manifest_writer_rejects_ambiguous_scoped_symbol_descriptors() -> Result<(), Box<dyn std::error::Error>> {
    let mut manifest = LibraryManifest::new("mylib", "0.1.0");
    let query_surface = incan_vocab::DslSurface::on_import("mylib.query")
        .with_declaration(
            incan_vocab::DeclarationSurface::named("query").with_clause(incan_vocab::ClauseSurface::expr("SELECT")),
        )
        .with_scoped_symbols([
            incan_vocab::ScopedSymbolDescriptor::aggregate("query.sum.primary", "sum")
                .in_clause_body("query", "SELECT"),
            incan_vocab::ScopedSymbolDescriptor::function("query.sum.secondary", "sum")
                .in_clause_body("query", "SELECT"),
        ]);
    manifest.vocab = Some(VocabExports {
        crate_path: "crates/mylib_vocab".to_string(),
        package_name: "mylib_vocab".to_string(),
        keyword_registrations: Vec::new(),
        dsl_surfaces: vec![query_surface],
        provider_manifest: incan_vocab::LibraryManifest::default(),
        desugarer_artifact: None,
    });

    let tmp = tempfile::tempdir()?;
    let err = manifest.write_to_path(&tmp.path().join("mylib.incnlib"));
    assert!(matches!(
        err,
        Err(LibraryManifestError::Invalid(msg)) if msg.contains("ambiguous scoped symbol descriptor")
    ));
    Ok(())
}

#[test]
fn manifest_writer_rejects_malformed_scoped_symbol_diagnostics() -> Result<(), Box<dyn std::error::Error>> {
    let mut manifest = LibraryManifest::new("mylib", "0.1.0");
    manifest.vocab = Some(VocabExports {
        crate_path: "crates/mylib_vocab".to_string(),
        package_name: "mylib_vocab".to_string(),
        keyword_registrations: Vec::new(),
        dsl_surfaces: vec![
            incan_vocab::DslSurface::on_import("mylib.query")
                .with_declaration(
                    incan_vocab::DeclarationSurface::named("query")
                        .with_clause(incan_vocab::ClauseSurface::expr("SELECT")),
                )
                .with_scoped_symbol(
                    incan_vocab::ScopedSymbolDescriptor::aggregate("query.sum", "sum")
                        .in_clause_body("query", "SELECT")
                        .with_diagnostic(incan_vocab::ScopedSymbolDiagnosticTemplate::new(
                            "query-sum-outside-select",
                            incan_vocab::ScopedSymbolDiagnosticKind::OutsideEligiblePosition,
                            "`sum` is only valid inside SELECT",
                        ))
                        .with_diagnostic(incan_vocab::ScopedSymbolDiagnosticTemplate::new(
                            "query-sum-outside-select",
                            incan_vocab::ScopedSymbolDiagnosticKind::AmbiguousResolution,
                            "use an explicit qualifier to disambiguate `sum`",
                        )),
                ),
        ],
        provider_manifest: incan_vocab::LibraryManifest::default(),
        desugarer_artifact: None,
    });

    let tmp = tempfile::tempdir()?;
    let err = manifest.write_to_path(&tmp.path().join("mylib.incnlib"));
    assert!(matches!(
        err,
        Err(LibraryManifestError::Invalid(msg)) if msg.contains("contains duplicate diagnostic code")
    ));
    Ok(())
}

#[test]
fn manifest_writer_rejects_ambiguous_scoped_surface_descriptors() -> Result<(), Box<dyn std::error::Error>> {
    let mut manifest = LibraryManifest::new("mylib", "0.1.0");
    let query_surface = incan_vocab::DslSurface::on_import("mylib.query")
        .with_declaration(
            incan_vocab::DeclarationSurface::named("query").with_clause(incan_vocab::ClauseSurface::expr("SELECT")),
        )
        .with_scoped_surfaces([
            incan_vocab::ScopedSurfaceDescriptor::operator("query.pipe.primary", "|>")
                .in_clause_body("query", "SELECT"),
            incan_vocab::ScopedSurfaceDescriptor::operator("query.pipe.secondary", "|>")
                .in_clause_body("query", "SELECT"),
        ]);
    manifest.vocab = Some(VocabExports {
        crate_path: "crates/mylib_vocab".to_string(),
        package_name: "mylib_vocab".to_string(),
        keyword_registrations: Vec::new(),
        dsl_surfaces: vec![query_surface],
        provider_manifest: incan_vocab::LibraryManifest::default(),
        desugarer_artifact: None,
    });

    let tmp = tempfile::tempdir()?;
    let err = manifest.write_to_path(&tmp.path().join("mylib.incnlib"));
    assert!(matches!(
        err,
        Err(LibraryManifestError::Invalid(msg)) if msg.contains("ambiguous scoped surface descriptor")
    ));
    Ok(())
}

#[test]
fn manifest_writer_rejects_expression_form_without_receiver() -> Result<(), Box<dyn std::error::Error>> {
    let mut manifest = LibraryManifest::new("mylib", "0.1.0");
    manifest.vocab = Some(VocabExports {
        crate_path: "crates/mylib_vocab".to_string(),
        package_name: "mylib_vocab".to_string(),
        keyword_registrations: Vec::new(),
        dsl_surfaces: vec![
            incan_vocab::DslSurface::on_import("mylib.query")
                .with_declaration(
                    incan_vocab::DeclarationSurface::named("query")
                        .with_clause(incan_vocab::ClauseSurface::expr("SELECT")),
                )
                .with_scoped_surface(
                    incan_vocab::ScopedSurfaceDescriptor::leading_dot_path("query.field")
                        .in_clause_body("query", "SELECT"),
                ),
        ],
        provider_manifest: incan_vocab::LibraryManifest::default(),
        desugarer_artifact: None,
    });

    let tmp = tempfile::tempdir()?;
    let err = manifest.write_to_path(&tmp.path().join("mylib.incnlib"));
    assert!(matches!(
        err,
        Err(LibraryManifestError::Invalid(msg)) if msg.contains("must declare receiver derivation")
    ));
    Ok(())
}

#[test]
fn manifest_writer_rejects_declaration_head_scoped_surface_position() -> Result<(), Box<dyn std::error::Error>> {
    let mut manifest = LibraryManifest::new("mylib", "0.1.0");
    manifest.vocab = Some(VocabExports {
        crate_path: "crates/mylib_vocab".to_string(),
        package_name: "mylib_vocab".to_string(),
        keyword_registrations: Vec::new(),
        dsl_surfaces: vec![
            incan_vocab::DslSurface::on_import("mylib.query")
                .with_declaration(incan_vocab::DeclarationSurface::named("query"))
                .with_scoped_surface(
                    incan_vocab::ScopedSurfaceDescriptor::operator("query.pipe", "|>")
                        .with_eligibility(incan_vocab::ScopedSurfaceEligibility::declaration_head("query")),
                ),
        ],
        provider_manifest: incan_vocab::LibraryManifest::default(),
        desugarer_artifact: None,
    });

    let tmp = tempfile::tempdir()?;
    let err = manifest.write_to_path(&tmp.path().join("mylib.incnlib"));
    assert!(matches!(
        err,
        Err(LibraryManifestError::Invalid(msg)) if msg.contains("declaration-head eligibility is not supported yet")
    ));
    Ok(())
}

#[test]
fn manifest_writer_rejects_helper_binding_to_unknown_export() -> Result<(), Box<dyn std::error::Error>> {
    let mut manifest = LibraryManifest::new("mylib", "0.1.0");
    manifest.vocab = Some(VocabExports {
        crate_path: "crates/mylib_vocab".to_string(),
        package_name: "mylib_vocab".to_string(),
        keyword_registrations: Vec::new(),
        dsl_surfaces: Vec::new(),
        provider_manifest: incan_vocab::LibraryManifest {
            helper_bindings: vec![incan_vocab::HelperBinding {
                key: "filter".to_string(),
                exported_name: "filter".to_string(),
            }],
            ..incan_vocab::LibraryManifest::default()
        },
        desugarer_artifact: None,
    });

    let tmp = tempfile::tempdir()?;
    let err = manifest.write_to_path(&tmp.path().join("mylib.incnlib"));
    assert!(matches!(err, Err(LibraryManifestError::Invalid(msg)) if msg.contains("unknown exported symbol `filter`")));
    Ok(())
}

#[test]
fn manifest_writer_rejects_helper_binding_to_an_uncallable_export() -> Result<(), Box<dyn std::error::Error>> {
    // The name is exported, so the unknown-symbol check passes; a helper reference is spliced into call position
    // though, and a trait cannot go there. Failing the provider's own build puts the error where its author can act
    // on it, rather than surfacing in every consumer as broken generated Rust.
    let mut manifest = legacy_manifest_fixture("mylib", "0.1.0");
    manifest.exports.traits.push(TraitExport {
        name: "Filterable".to_string(),
        source_name: None,
        type_params: Vec::new(),
        supertraits: Vec::new(),
        requires: Vec::new(),
        methods: Vec::new(),
    });
    manifest.vocab = Some(VocabExports {
        crate_path: "crates/mylib_vocab".to_string(),
        package_name: "mylib_vocab".to_string(),
        keyword_registrations: Vec::new(),
        dsl_surfaces: Vec::new(),
        provider_manifest: incan_vocab::LibraryManifest {
            helper_bindings: vec![incan_vocab::HelperBinding {
                key: "filter".to_string(),
                exported_name: "Filterable".to_string(),
            }],
            ..incan_vocab::LibraryManifest::default()
        },
        desugarer_artifact: None,
    });

    let tmp = tempfile::tempdir()?;
    let err = manifest.write_to_path(&tmp.path().join("mylib.incnlib"));
    assert!(
        matches!(&err, Err(LibraryManifestError::Invalid(msg)) if msg.contains("trait `Filterable`")),
        "error should name the kind: {err:?}"
    );
    assert!(
        matches!(&err, Err(LibraryManifestError::Invalid(msg)) if msg.contains("cannot be called")),
        "unexpected error: {err:?}"
    );
    Ok(())
}

#[test]
fn manifest_writer_rejects_a_helper_binding_through_an_alias_to_an_uncallable_target()
-> Result<(), Box<dyn std::error::Error>> {
    // The alias is exported and looks callable on its face, so a check that stops at the alias admits it. Following
    // the reexport to its target is what the consumer's frontend does, and the provider's own build has to reach the
    // same answer; otherwise this publishes cleanly and then fails in every consumer as broken generated Rust.
    let mut manifest = legacy_manifest_fixture("mylib", "0.1.0");
    manifest.exports.traits.push(TraitExport {
        name: "Filterable".to_string(),
        source_name: None,
        type_params: Vec::new(),
        supertraits: Vec::new(),
        requires: Vec::new(),
        methods: Vec::new(),
    });
    manifest.exports.aliases.push(AliasExport {
        name: "Filter".to_string(),
        target_path: vec!["mylib".to_string(), "Filterable".to_string()],
        projected_function: None,
    });
    manifest.vocab = Some(VocabExports {
        crate_path: "crates/mylib_vocab".to_string(),
        package_name: "mylib_vocab".to_string(),
        keyword_registrations: Vec::new(),
        dsl_surfaces: Vec::new(),
        provider_manifest: incan_vocab::LibraryManifest {
            helper_bindings: vec![incan_vocab::HelperBinding {
                key: "filter".to_string(),
                exported_name: "Filter".to_string(),
            }],
            ..incan_vocab::LibraryManifest::default()
        },
        desugarer_artifact: None,
    });

    let tmp = tempfile::tempdir()?;
    let err = manifest.write_to_path(&tmp.path().join("mylib.incnlib"));
    assert!(
        matches!(&err, Err(LibraryManifestError::Invalid(msg)) if msg.contains("trait `Filter`")),
        "error should name what the alias resolves to: {err:?}"
    );
    Ok(())
}

#[test]
fn manifest_writer_accepts_a_helper_binding_to_a_public_partial() -> Result<(), Box<dyn std::error::Error>> {
    // A public partial is a preset over a callable target, so it is callable in its own right and belongs on the
    // surface a companion may bind to. Omitting it from the export authority rejected a legitimate public export as
    // an unknown symbol, and the provider had no way to publish the helper at all.
    let mut manifest = legacy_manifest_fixture("mylib", "0.1.0");
    manifest.exports.partials.push(PartialExport {
        name: "filter_active".to_string(),
        target_path: vec!["mylib".to_string(), "filter_rows".to_string()],
        target_kind: PartialTargetKindExport::Function,
        presets: vec![PartialPresetExport {
            name: "status".to_string(),
            ty: TypeRef::Named {
                name: "str".to_string(),
            },
            value: PresetValueExport::String("active".to_string()),
        }],
        type_params: Vec::new(),
        params: vec![ParamExport {
            name: "status".to_string(),
            ty: TypeRef::Named {
                name: "str".to_string(),
            },
            kind: ParamKindExport::Normal,
            has_default: true,
            default: None,
        }],
        return_type: TypeRef::Named {
            name: "None".to_string(),
        },
        is_async: false,
    });
    manifest.vocab = Some(VocabExports {
        crate_path: "crates/mylib_vocab".to_string(),
        package_name: "mylib_vocab".to_string(),
        keyword_registrations: Vec::new(),
        dsl_surfaces: Vec::new(),
        provider_manifest: incan_vocab::LibraryManifest {
            helper_bindings: vec![incan_vocab::HelperBinding {
                key: "filter".to_string(),
                exported_name: "filter_active".to_string(),
            }],
            ..incan_vocab::LibraryManifest::default()
        },
        desugarer_artifact: None,
    });

    let tmp = tempfile::tempdir()?;
    manifest.write_to_path(&tmp.path().join("mylib.incnlib"))?;
    Ok(())
}

#[test]
fn manifest_writer_rejects_duplicate_helper_binding_keys() -> Result<(), Box<dyn std::error::Error>> {
    let mut manifest = legacy_manifest_fixture("mylib", "0.1.0");
    manifest.exports.functions.push(FunctionExport {
        name: "filter".to_string(),
        emitted_name: None,
        type_params: Vec::new(),
        params: Vec::new(),
        return_type: TypeRef::Unknown,
        is_async: false,
    });
    manifest.exports.functions.push(FunctionExport {
        name: "where_impl".to_string(),
        emitted_name: None,
        type_params: Vec::new(),
        params: Vec::new(),
        return_type: TypeRef::Unknown,
        is_async: false,
    });
    manifest.vocab = Some(VocabExports {
        crate_path: "crates/mylib_vocab".to_string(),
        package_name: "mylib_vocab".to_string(),
        keyword_registrations: Vec::new(),
        dsl_surfaces: Vec::new(),
        provider_manifest: incan_vocab::LibraryManifest {
            helper_bindings: vec![
                incan_vocab::HelperBinding {
                    key: "filter".to_string(),
                    exported_name: "filter".to_string(),
                },
                incan_vocab::HelperBinding {
                    key: "filter".to_string(),
                    exported_name: "where_impl".to_string(),
                },
            ],
            ..incan_vocab::LibraryManifest::default()
        },
        desugarer_artifact: None,
    });

    let tmp = tempfile::tempdir()?;
    let err = manifest.write_to_path(&tmp.path().join("mylib.incnlib"));
    assert!(matches!(err, Err(LibraryManifestError::Invalid(msg)) if msg.contains("duplicate key `filter`")));
    Ok(())
}

#[test]
fn manifest_writer_rejects_non_normalized_desugarer_relative_path() -> Result<(), Box<dyn std::error::Error>> {
    let mut manifest = LibraryManifest::new("mylib", "0.1.0");
    manifest.vocab = Some(VocabExports {
        crate_path: "crates/mylib_vocab".to_string(),
        package_name: "mylib_vocab".to_string(),
        keyword_registrations: Vec::new(),
        dsl_surfaces: Vec::new(),
        provider_manifest: incan_vocab::LibraryManifest::default(),
        desugarer_artifact: Some(VocabDesugarerArtifact {
            artifact_kind: incan_vocab::DesugarerArtifactKind::WasmModule,
            abi_version: incan_vocab::WASM_DESUGAR_ABI_VERSION,
            relative_path: "../escape.wasm".to_string(),
            target: "wasm32-wasip1".to_string(),
            profile: "release".to_string(),
            entrypoint: incan_vocab::WASM_DESUGAR_ENTRYPOINT.to_string(),
            sha256: "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".to_string(),
        }),
    });

    let tmp = tempfile::tempdir()?;
    let err = manifest.write_to_path(&tmp.path().join("mylib.incnlib"));
    assert!(
        matches!(err, Err(LibraryManifestError::Invalid(msg)) if msg.contains("must be a normalized relative path"))
    );
    Ok(())
}

#[test]
fn manifest_writer_rejects_non_hex_desugarer_sha256() -> Result<(), Box<dyn std::error::Error>> {
    let mut manifest = LibraryManifest::new("mylib", "0.1.0");
    manifest.vocab = Some(VocabExports {
        crate_path: "crates/mylib_vocab".to_string(),
        package_name: "mylib_vocab".to_string(),
        keyword_registrations: Vec::new(),
        dsl_surfaces: Vec::new(),
        provider_manifest: incan_vocab::LibraryManifest::default(),
        desugarer_artifact: Some(VocabDesugarerArtifact {
            artifact_kind: incan_vocab::DesugarerArtifactKind::WasmModule,
            abi_version: incan_vocab::WASM_DESUGAR_ABI_VERSION,
            relative_path: "desugarers/mylib.wasm".to_string(),
            target: "wasm32-wasip1".to_string(),
            profile: "release".to_string(),
            entrypoint: incan_vocab::WASM_DESUGAR_ENTRYPOINT.to_string(),
            sha256: "not-a-valid-sha256".to_string(),
        }),
    });

    let tmp = tempfile::tempdir()?;
    let err = manifest.write_to_path(&tmp.path().join("mylib.incnlib"));
    assert!(matches!(err, Err(LibraryManifestError::Invalid(msg)) if msg.contains("must be 64 hex characters")));
    Ok(())
}

/// A `pub from <module> import <Model>` re-export must survive manifest validation, exactly as the shipped
/// `examples/advanced/library_package` producer writes it.
///
/// The identity-graph validator admitted a `Reexport` projection only for `Alias` and `Function` kinds. That is not
/// a property of re-exports: the projection is orthogonal to the kind, and `CheckedExportKind` maps every
/// declaration kind onto it. `pub from pricing import LineItem, subtotal` re-exports a model beside a function, so
/// the function half satisfied the whitelist while the model half failed with "identity graph entry `LineItem` uses
/// a reexport projection for Model" — and because every existing reexport test re-exported an alias or a function,
/// nothing caught it until `check-docs-examples` failed in CI.
///
/// `LibraryReexportResolver` (the production path at `cli::commands::build`) makes the intent explicit: it resolves
/// a `pub from` item to its *target's* real kind while retaining the reexport projection, and
/// `resolve_library_reexports_*` already asserts a `TypeAlias` emerging that way. `TypeAlias` was not in the
/// whitelist either, so the validator contradicted a contract the resolver's own tests had already pinned.
#[test]
fn reexported_model_passes_identity_graph_validation() -> Result<(), Box<dyn std::error::Error>> {
    let mut manifest = LibraryManifest::from_checked_exports("pricing_core", "0.1.0", &[]);
    manifest.exports.models.push(ModelExport {
        name: "LineItem".to_string(),
        type_params: Vec::new(),
        traits: Vec::new(),
        trait_adoptions: Vec::new(),
        derives: Vec::new(),
        fields: Vec::new(),
        properties: Vec::new(),
        methods: Vec::new(),
    });

    let identity = published_declaration_identity(
        "pricing_core",
        &["pricing"],
        "LineItem",
        incan_semantics_core::SemanticSourceTargetKind::Model,
        10,
        20,
    );
    manifest.contract_metadata.identity_graph.exports.push(ExportIdentity {
        public_name: "LineItem".to_string(),
        public_path: vec!["pricing_core".to_string(), "LineItem".to_string()],
        source_path: vec!["pricing".to_string(), "LineItem".to_string()],
        kind: ExportIdentityKind::Model,
        projection: ExportIdentityProjection::Reexport {
            target_path: vec!["pricing".to_string(), "LineItem".to_string()],
        },
        canonical: CanonicalIdentityExport::from_canonical("pricing_core", &identity),
    });

    let tmp = tempfile::tempdir()?;
    let path = tmp.path().join("reexported-model.incnlib");
    manifest
        .write_to_path(&path)
        .map_err(|error| format!("a re-exported model must pass identity-graph validation, got: {error}"))?;
    Ok(())
}

/// Every declaration kind a `pub from` can republish must pass identity-graph validation under `Reexport`.
///
/// A re-export is a projection over an already-declared symbol, so its kind is the target's real kind rather than a
/// kind of its own. Building a library whose root re-exports one declaration of each form produces `Reexport`
/// entries for class, const, enum, function, model, newtype, trait, and type-alias kinds. The validator previously
/// admitted only `Alias` and `Function` there, so six of those eight were rejected and `incan build --lib` failed for
/// any library with a facade -- including the shipped `examples/advanced/library_package`.
///
/// The suite missed it because every earlier re-export fixture re-exported an alias or a function, which is exactly
/// the whitelist's blind spot. This asserts the whole set instead of one representative.
#[test]
fn every_reexportable_kind_passes_identity_graph_validation() -> Result<(), Box<dyn std::error::Error>> {
    use incan_semantics_core::SemanticSourceTargetKind;

    let named = |name: &str| TypeRef::Named { name: name.to_string() };
    let cases: Vec<(ExportIdentityKind, SemanticSourceTargetKind)> = vec![
        (ExportIdentityKind::Function, SemanticSourceTargetKind::Function),
        (ExportIdentityKind::Model, SemanticSourceTargetKind::Model),
        (ExportIdentityKind::Class, SemanticSourceTargetKind::Class),
        (ExportIdentityKind::Trait, SemanticSourceTargetKind::Trait),
        (ExportIdentityKind::Enum, SemanticSourceTargetKind::Enum),
        (ExportIdentityKind::Newtype, SemanticSourceTargetKind::Newtype),
        (ExportIdentityKind::TypeAlias, SemanticSourceTargetKind::TypeAlias),
        (ExportIdentityKind::Const, SemanticSourceTargetKind::Const),
        (ExportIdentityKind::Static, SemanticSourceTargetKind::Static),
    ];

    for (export_kind, semantic_kind) in cases {
        let name = format!("Exported{export_kind:?}");
        let mut manifest = LibraryManifest::from_checked_exports("facade_lib", "0.1.0", &[]);
        match export_kind {
            ExportIdentityKind::Function => manifest.exports.functions.push(FunctionExport {
                name: name.clone(),
                emitted_name: None,
                type_params: Vec::new(),
                params: Vec::new(),
                return_type: named("int"),
                is_async: false,
            }),
            ExportIdentityKind::Model => manifest.exports.models.push(ModelExport {
                name: name.clone(),
                type_params: Vec::new(),
                traits: Vec::new(),
                trait_adoptions: Vec::new(),
                derives: Vec::new(),
                fields: Vec::new(),
                properties: Vec::new(),
                methods: Vec::new(),
            }),
            ExportIdentityKind::Class => manifest.exports.classes.push(ClassExport {
                name: name.clone(),
                type_params: Vec::new(),
                extends: None,
                traits: Vec::new(),
                trait_adoptions: Vec::new(),
                derives: Vec::new(),
                fields: Vec::new(),
                properties: Vec::new(),
                methods: Vec::new(),
            }),
            ExportIdentityKind::Trait => manifest.exports.traits.push(TraitExport {
                name: name.clone(),
                source_name: None,
                type_params: Vec::new(),
                supertraits: Vec::new(),
                requires: Vec::new(),
                methods: Vec::new(),
            }),
            ExportIdentityKind::Enum => manifest.exports.enums.push(EnumExport {
                name: name.clone(),
                type_params: Vec::new(),
                traits: Vec::new(),
                trait_adoptions: Vec::new(),
                value_type: None,
                ordinal_type_identity: None,
                variants: Vec::new(),
                variant_aliases: Vec::new(),
                methods: Vec::new(),
                derives: Vec::new(),
            }),
            ExportIdentityKind::Newtype => manifest.exports.newtypes.push(NewtypeExport {
                name: name.clone(),
                type_params: Vec::new(),
                traits: Vec::new(),
                trait_adoptions: Vec::new(),
                derives: Vec::new(),
                is_rusttype: false,
                underlying: named("str"),
                methods: Vec::new(),
                checked_constructor: None,
                constraints: Vec::new(),
                implicit_coercion_enabled: false,
            }),
            ExportIdentityKind::TypeAlias => manifest.exports.type_aliases.push(TypeAliasExport {
                name: name.clone(),
                type_params: Vec::new(),
                target: named("int"),
            }),
            ExportIdentityKind::Const => manifest.exports.consts.push(ConstExport {
                name: name.clone(),
                ty: named("int"),
            }),
            ExportIdentityKind::Static => manifest.exports.statics.push(StaticExport {
                name: name.clone(),
                ty: named("int"),
            }),
            other => return Err(format!("unhandled export kind in fixture: {other:?}").into()),
        }

        let identity = published_declaration_identity("facade_lib", &["inner"], &name, semantic_kind, 10, 20);
        let target_path = vec!["inner".to_string(), name.clone()];
        manifest.contract_metadata.identity_graph.exports.push(ExportIdentity {
            public_name: name.clone(),
            public_path: vec!["facade_lib".to_string(), name.clone()],
            source_path: target_path.clone(),
            kind: export_kind,
            projection: ExportIdentityProjection::Reexport { target_path },
            canonical: CanonicalIdentityExport::from_canonical("facade_lib", &identity),
        });

        let tmp = tempfile::tempdir()?;
        manifest
            .write_to_path(&tmp.path().join("facade.incnlib"))
            .map_err(|error| format!("a re-exported {export_kind:?} must validate, got: {error}"))?;
    }
    Ok(())
}

/// `pub from crate.pricing import LineItem` must validate against the identity behind the `crate` qualifier.
///
/// The frontend records an export's path exactly as the source spelled it, so an absolute import arrives with a
/// leading `crate`. A canonical identity stores the resolved module path without one. Comparing the two spellings
/// verbatim rejected every export re-exported through an absolute import and took `incan build --lib` down for real
/// libraries, even though both spellings named the same declaration.
#[test]
fn crate_qualified_reexport_passes_identity_graph_validation() -> Result<(), Box<dyn std::error::Error>> {
    let mut manifest = LibraryManifest::from_checked_exports("pricing_core", "0.1.0", &[]);
    manifest.exports.models.push(ModelExport {
        name: "LineItem".to_string(),
        type_params: Vec::new(),
        traits: Vec::new(),
        trait_adoptions: Vec::new(),
        derives: Vec::new(),
        fields: Vec::new(),
        properties: Vec::new(),
        methods: Vec::new(),
    });

    let identity = published_declaration_identity(
        "pricing_core",
        &["pricing"],
        "LineItem",
        incan_semantics_core::SemanticSourceTargetKind::Model,
        10,
        20,
    );
    let crate_qualified = vec!["crate".to_string(), "pricing".to_string(), "LineItem".to_string()];
    manifest.contract_metadata.identity_graph.exports.push(ExportIdentity {
        public_name: "LineItem".to_string(),
        public_path: vec!["pricing_core".to_string(), "LineItem".to_string()],
        source_path: crate_qualified.clone(),
        kind: ExportIdentityKind::Model,
        projection: ExportIdentityProjection::Reexport {
            target_path: crate_qualified,
        },
        canonical: CanonicalIdentityExport::from_canonical("pricing_core", &identity),
    });

    let tmp = tempfile::tempdir()?;
    let path = tmp.path().join("crate-qualified-reexport.incnlib");
    manifest
        .write_to_path(&path)
        .map_err(|error| format!("a `crate`-qualified re-export must pass identity-graph validation, got: {error}"))?;
    Ok(())
}

/// A same-module alias re-exported under a new name must survive identity validation end to end.
///
/// This is the shape `pub run = alias helper` in `provider` plus `pub from provider import run as public_target` in
/// the entrypoint. It exercises three places where a spelling used to stand in for a resolved identity: the checked
/// alias records its target as the source wrote it (`["helper"]`) while the graph entry records the resolved
/// declaration (`["provider", "helper"]`); the alias's materialized callable projection carries the resolved path
/// where the alias carries the spelling; and the re-export's authoritative path ends at the alias's own public name
/// rather than at the declaration the identity names.
#[test]
fn same_module_alias_reexported_under_a_new_name_passes_identity_validation() -> Result<(), Box<dyn std::error::Error>>
{
    let anchor = |id: &str, start: usize, end: usize| SourceAnchor {
        id: id.to_string(),
        span: SourceSpan { start, end },
    };
    let mut modules = vec![
        CheckedApiMetadata {
            schema_version: CHECKED_API_METADATA_SCHEMA_VERSION,
            module_path: vec!["provider".to_string()],
            declarations: vec![
                ApiDeclaration::Function(ApiFunction {
                    name: "helper".to_string(),
                    anchor: anchor("provider.helper", 59, 116),
                    docstring: None,
                    docstring_sections: None,
                    decorators: Vec::new(),
                    type_params: Vec::new(),
                    params: Vec::new(),
                    return_type: TypeRef::Named {
                        name: "int".to_string(),
                    },
                    is_async: false,
                }),
                ApiDeclaration::Alias(ApiAlias {
                    name: "run".to_string(),
                    anchor: anchor("provider.run", 130, 150),
                    // Spelled the way the source wrote it, not the way it resolves.
                    target_path: vec!["helper".to_string()],
                    is_public: true,
                    projected_function: None,
                }),
            ],
        },
        CheckedApiMetadata {
            schema_version: CHECKED_API_METADATA_SCHEMA_VERSION,
            module_path: vec!["main".to_string()],
            declarations: vec![ApiDeclaration::Alias(ApiAlias {
                name: "public_target".to_string(),
                anchor: anchor("main.public_target", 0, 45),
                // The entrypoint spells the hop it re-exports; the projection below resolves past it.
                target_path: vec!["provider".to_string(), "run".to_string()],
                is_public: true,
                projected_function: Some(crate::frontend::api_metadata::ApiProjectedFunction {
                    source_path: vec!["provider".to_string(), "helper".to_string()],
                    callable: crate::frontend::api_metadata::ApiCallableMetadata {
                        name: "public_target".to_string(),
                        anchor: anchor("main.public_target", 0, 45),
                        type_params: Vec::new(),
                        receiver: None,
                        params: Vec::new(),
                        return_type: TypeRef::Named {
                            name: "int".to_string(),
                        },
                        is_async: false,
                    },
                    decorators: Vec::new(),
                }),
            })],
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

    let identity = published_declaration_identity(
        "aliasrepro",
        &["provider"],
        "helper",
        incan_semantics_core::SemanticSourceTargetKind::Function,
        59,
        116,
    );
    let canonical = CanonicalIdentityExport::from_canonical("aliasrepro", &identity);

    let mut manifest = LibraryManifest::from_checked_exports("aliasrepro", "0.1.0", &[]);
    manifest.exports.aliases.push(AliasExport {
        name: "public_target".to_string(),
        target_path: vec!["provider".to_string(), "helper".to_string()],
        projected_function: Some(FunctionExport {
            // The renamed re-export republishes the callable under its new public name.
            name: "public_target".to_string(),
            emitted_name: None,
            type_params: Vec::new(),
            params: Vec::new(),
            return_type: TypeRef::Named {
                name: "int".to_string(),
            },
            is_async: false,
        }),
    });
    let graph = &mut manifest.contract_metadata.identity_graph;
    graph.exports.push(ExportIdentity {
        public_name: "helper".to_string(),
        public_path: vec!["aliasrepro".to_string(), "provider".to_string(), "helper".to_string()],
        source_path: vec!["provider".to_string(), "helper".to_string()],
        kind: ExportIdentityKind::Function,
        projection: ExportIdentityProjection::Direct,
        canonical: canonical.clone(),
    });
    graph.exports.push(ExportIdentity {
        public_name: "run".to_string(),
        public_path: vec!["aliasrepro".to_string(), "provider".to_string(), "run".to_string()],
        source_path: vec!["provider".to_string(), "run".to_string()],
        kind: ExportIdentityKind::Alias,
        projection: ExportIdentityProjection::Alias {
            target_path: vec!["provider".to_string(), "helper".to_string()],
        },
        canonical: canonical.clone(),
    });
    graph.exports.push(ExportIdentity {
        public_name: "public_target".to_string(),
        public_path: vec!["aliasrepro".to_string(), "public_target".to_string()],
        source_path: vec!["provider".to_string(), "run".to_string()],
        kind: ExportIdentityKind::Alias,
        projection: ExportIdentityProjection::Reexport {
            target_path: vec!["provider".to_string(), "run".to_string()],
        },
        canonical: canonical.clone(),
    });
    manifest.contract_metadata.api = Some(api);

    let tmp = tempfile::tempdir()?;
    let path = tmp.path().join("alias-reexport.incnlib");
    manifest.write_to_path(&path).map_err(|error| {
        format!("a renamed re-export of a same-module alias must pass identity-graph validation, got: {error}")
    })?;

    let loaded = LibraryManifest::read_from_path(&path)?;
    let published = loaded
        .contract_metadata
        .identity_graph
        .canonical_for_public_path(&["aliasrepro".to_string(), "public_target".to_string()])
        .ok_or("the renamed re-export must publish a canonical identity")?;
    assert_eq!(
        published.declaration_name, "helper",
        "renaming a declaration twice must still resolve to the declaration, not to either local name"
    );
    Ok(())
}
