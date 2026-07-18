use std::collections::{BTreeMap, BTreeSet};

use crate::frontend::api_metadata::{
    ApiDeclaration, ApiModel, ApiTrait, CHECKED_API_METADATA_SCHEMA_VERSION, CheckedApiMetadata,
    CheckedApiMetadataPackage, SourceAnchor, SourceSpan,
};

use super::*;

#[test]
fn manifest_io_round_trip_preserves_recursive_types_and_bounds() -> Result<(), Box<dyn std::error::Error>> {
    let mut manifest = LibraryManifest::new("mylib", "0.1.0");
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
                ty: crate::frontend::symbols::ResolvedType::Str,
                visibility: crate::frontend::ast::Visibility::Private,
                has_default: false,
                default: None,
                alias: None,
                description: None,
            },
            crate::frontend::library_exports::CheckedField {
                name: "label".to_string(),
                ty: crate::frontend::symbols::ResolvedType::Str,
                visibility: crate::frontend::ast::Visibility::Public,
                has_default: false,
                default: None,
                alias: None,
                description: None,
            },
        ],
        methods: Vec::new(),
    };
    let manifest = LibraryManifest::from_checked_exports(
        "sealed_class_lib",
        "0.1.0",
        &[crate::frontend::library_exports::CheckedNamedExport {
            name: "Vault".to_string(),
            identity: crate::frontend::library_exports::CheckedExportIdentity::direct(vec!["Vault".to_string()]),
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
    let mut manifest = LibraryManifest::new("legacy_class_lib", "0.1.0");
    manifest.exports.classes.push(ClassExport {
        name: "Legacy".to_string(),
        type_params: Vec::new(),
        extends: None,
        traits: Vec::new(),
        trait_adoptions: Vec::new(),
        derives: Vec::new(),
        fields: vec![FieldExport {
            name: "value".to_string(),
            ty: TypeRef::Named {
                name: "str".to_string(),
            },
            visibility: FieldVisibilityExport::Public,
            has_default: false,
            default: None,
            alias: None,
            description: None,
        }],
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
fn manifest_rejects_unsupported_private_model_field_visibility_issue883() -> Result<(), Box<dyn std::error::Error>> {
    let mut manifest = LibraryManifest::new("private_model_lib", "0.1.0");
    manifest.exports.models.push(ModelExport {
        name: "Record".to_string(),
        type_params: Vec::new(),
        traits: Vec::new(),
        trait_adoptions: Vec::new(),
        derives: Vec::new(),
        fields: vec![FieldExport {
            name: "secret".to_string(),
            ty: TypeRef::Named {
                name: "str".to_string(),
            },
            visibility: FieldVisibilityExport::Private,
            has_default: false,
            default: None,
            alias: None,
            description: None,
        }],
        methods: Vec::new(),
    });

    let tmp = tempfile::tempdir()?;
    let error = manifest.write_to_path(&tmp.path().join("private_model_lib.incnlib"));
    assert!(
        matches!(error, Err(LibraryManifestError::Invalid(ref message)) if message.contains("model `Record` field `secret` cannot be private")),
        "expected unsupported private model field visibility to fail validation, got: {error:?}"
    );
    Ok(())
}

fn private_api_field_issue883() -> FieldExport {
    FieldExport {
        name: "secret".to_string(),
        ty: TypeRef::Named {
            name: "str".to_string(),
        },
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
    });
    manifest
}

#[test]
fn manifest_rejects_private_model_field_in_embedded_api_metadata_issue883() -> Result<(), Box<dyn std::error::Error>> {
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
        methods: Vec::new(),
    }));

    let tmp = tempfile::tempdir()?;
    let error = manifest.write_to_path(&tmp.path().join("private_api_model.incnlib"));
    assert!(
        matches!(error, Err(LibraryManifestError::Invalid(ref message)) if message.contains("API model `Record` field `secret` cannot be private")),
        "expected embedded private API model field to fail validation, got: {error:?}"
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
    let mut manifest = LibraryManifest::new("mylib", "0.1.0");
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
    let mut manifest = LibraryManifest::new("mylib", "0.1.0");
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
            ]),
            kind: crate::frontend::library_exports::CheckedExportKind::Function(callable.clone()),
        },
        crate::frontend::library_exports::CheckedNamedExport {
            name: "safe_cast".to_string(),
            identity: crate::frontend::library_exports::CheckedExportIdentity::alias(
                vec!["facade".to_string(), "safe_cast".to_string()],
                vec!["helpers".to_string(), "cast".to_string()],
            ),
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
            ),
            kind: crate::frontend::library_exports::CheckedExportKind::Function(callable.clone()),
        },
        crate::frontend::library_exports::CheckedNamedExport {
            name: "core_cast".to_string(),
            identity: crate::frontend::library_exports::CheckedExportIdentity::partial(
                vec!["helpers".to_string(), "core_cast".to_string()],
                vec!["helpers".to_string(), "cast".to_string()],
                crate::frontend::library_exports::CheckedPartialTargetKind::Function,
            ),
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

    let public_cast = graph
        .entry_for_public_name("public_cast")
        .ok_or("missing public_cast identity")?;
    assert_eq!(
        public_cast.projection,
        ExportIdentityProjection::Reexport {
            target_path: vec!["helpers".to_string(), "cast".to_string()]
        }
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
    use incan_core::interop::{RustFunctionSig, RustItemKind, RustItemMetadata, RustParam, RustVisibility};

    let mut manifest = LibraryManifest::new("mylib", "0.1.0");
    manifest.rust_abi = LibraryRustAbi::from_items(vec![RustItemMetadata {
        canonical_path: "mylib_runtime::parse".to_string(),
        definition_path: Some("mylib_runtime::parse".to_string()),
        visibility: RustVisibility::Public,
        kind: RustItemKind::Function(RustFunctionSig {
            params: vec![RustParam {
                name: Some("source".to_string()),
                type_display: "&str".to_string(),
            }],
            return_type: "Result<mylib_runtime::Plan, mylib_runtime::Error>".to_string(),
            is_async: true,
            is_unsafe: false,
        }),
    }]);

    let tmp = tempfile::tempdir()?;
    let path = tmp.path().join("mylib.incnlib");
    manifest.write_to_path(&path)?;
    let loaded = LibraryManifest::read_from_path(&path)?;

    assert_eq!(loaded, manifest);
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
fn manifest_validation_rejects_unsupported_rust_abi_schema_version() {
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
        RUST_ABI_SCHEMA_VERSION + 1
    );

    let err = LibraryManifest::from_json_str(&raw);
    assert!(err.is_err(), "expected unsupported Rust ABI schema to fail");
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
    let mut manifest = LibraryManifest::new("mylib", "0.1.0");
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
        methods: vec![MethodExport {
            alias_of: None,
            name: "collect".to_string(),
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
    let mut manifest = LibraryManifest::new("mylib", "0.1.0");
    manifest.exports.traits.push(TraitExport {
        name: "Ord".to_string(),
        source_name: None,
        type_params: Vec::new(),
        supertraits: vec![TypeBoundExport {
            name: "Eq".to_string(),
            source_name: None,
            module_path: None,
            type_args: Vec::new(),
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
    let mut manifest = LibraryManifest::new("mylib", "0.1.0");
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
                fields: Vec::new(),
                value: Some(EnumValueExport::Str("active".to_string())),
            },
            EnumVariantExport {
                name: "Disabled".to_string(),
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
                fields: Vec::new(),
                value: Some(EnumValueExport::Int(200)),
            },
            EnumVariantExport {
                name: "NotFound".to_string(),
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
    let mut manifest = LibraryManifest::new("mylib", "0.1.0");
    manifest.exports.enums.push(EnumExport {
        name: "Status".to_string(),
        type_params: Vec::new(),
        traits: vec!["Labelled".to_string()],
        trait_adoptions: Vec::new(),
        value_type: None,
        ordinal_type_identity: None,
        variants: vec![EnumVariantExport {
            name: "Active".to_string(),
            fields: Vec::new(),
            value: None,
        }],
        variant_aliases: Vec::new(),
        methods: vec![MethodExport {
            alias_of: None,
            name: "label".to_string(),
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
    let mut manifest = LibraryManifest::new("mylib", "0.1.0");
    manifest.exports.classes.push(ClassExport {
        name: "Box".to_string(),
        type_params: Vec::new(),
        extends: None,
        traits: Vec::new(),
        trait_adoptions: Vec::new(),
        derives: Vec::new(),
        fields: Vec::new(),
        methods: vec![MethodExport {
            alias_of: None,
            name: "get".to_string(),
            type_params: vec![TypeParamExport {
                name: "T".to_string(),
                bounds: vec![TypeBoundExport {
                    name: "Clone".to_string(),
                    source_name: None,
                    module_path: None,
                    type_args: Vec::new(),
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
    let mut manifest = LibraryManifest::new("mylib", "0.1.0");
    manifest.exports.models.push(ModelExport {
        name: "Record".to_string(),
        type_params: Vec::new(),
        traits: Vec::new(),
        trait_adoptions: Vec::new(),
        derives: vec!["Clone".to_string()],
        fields: Vec::new(),
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
    let mut manifest = LibraryManifest::new("mylib", "0.1.0");
    let convert_int = TypeBoundExport {
        name: "Convert".to_string(),
        source_name: None,
        module_path: None,
        type_args: vec![TypeRef::Named {
            name: "int".to_string(),
        }],
    };
    let convert_float = TypeBoundExport {
        name: "Convert".to_string(),
        source_name: None,
        module_path: None,
        type_args: vec![TypeRef::Named {
            name: "float".to_string(),
        }],
    };
    manifest.exports.models.push(ModelExport {
        name: "Record".to_string(),
        type_params: Vec::new(),
        traits: vec!["Convert".to_string(), "Convert".to_string()],
        trait_adoptions: vec![convert_int.clone(), convert_float.clone()],
        derives: Vec::new(),
        fields: Vec::new(),
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
        }],
        derives: Vec::new(),
        fields: Vec::new(),
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
fn manifest_writer_rejects_duplicate_helper_binding_keys() -> Result<(), Box<dyn std::error::Error>> {
    let mut manifest = LibraryManifest::new("mylib", "0.1.0");
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
