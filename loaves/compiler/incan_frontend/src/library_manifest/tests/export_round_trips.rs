//! Round trips and validation of export shapes: recursive types and bounds, private field visibility (#883, #884),
//! partial exports, parameter defaults and their materializability, rest parameters, trait supertraits, value enums,
//! enum traits and methods, generic method type parameters, derives, and type trait adoptions.

use super::*;

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
                inferred: false,
            }],
        }],
        params: vec![ParamExport {
            name: "value".to_string(),
            ty: TypeRef::Applied {
                origin: None,
                name: "Result".to_string(),
                args: vec![
                    TypeRef::Applied {
                        origin: None,
                        name: "Option".to_string(),
                        args: vec![TypeRef::TypeParam { name: "T".to_string() }],
                    },
                    TypeRef::Named {
                        origin: None,
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
                        origin: None,
                        name: "int".to_string(),
                    },
                ],
            }],
            return_type: Box::new(TypeRef::Named {
                origin: None,
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
    let checked_class = crate::library_exports::CheckedClassExport {
        name: "Vault".to_string(),
        type_params: Vec::new(),
        extends: None,
        traits: Vec::new(),
        trait_adoptions: Vec::new(),
        derives: Vec::new(),
        fields: vec![
            crate::library_exports::CheckedField {
                name: "secret".to_string(),
                canonical: Some(source_member_identity(
                    &["lib"],
                    "secret",
                    incan_semantics_core::SemanticSourceTargetKind::Field,
                    20,
                    26,
                )),
                ty: crate::symbols::ResolvedType::Str,
                surface_type_name: Some("str".to_string()),
                visibility: crate::ast::Visibility::Private,
                has_default: false,
                default: None,
                alias: None,
                description: None,
            },
            crate::library_exports::CheckedField {
                name: "label".to_string(),
                canonical: Some(source_member_identity(
                    &["lib"],
                    "label",
                    incan_semantics_core::SemanticSourceTargetKind::Field,
                    30,
                    35,
                )),
                ty: crate::symbols::ResolvedType::Str,
                surface_type_name: Some("str".to_string()),
                visibility: crate::ast::Visibility::Public,
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
        &[crate::library_exports::CheckedNamedExport {
            name: "Vault".to_string(),
            identity: crate::library_exports::CheckedExportIdentity::direct(vec![
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
            kind: crate::library_exports::CheckedExportKind::Class(checked_class),
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
                origin: None,
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
                origin: None,
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
            origin: None,
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
            derivable_traits: Vec::new(),
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
                origin: None,
                name: "str".to_string(),
            },
            value: PresetValueExport::String("GET".to_string()),
        }],
        type_params: Vec::new(),
        params: vec![
            ParamExport {
                name: "method".to_string(),
                ty: TypeRef::Named {
                    origin: None,
                    name: "str".to_string(),
                },
                kind: ParamKindExport::Normal,
                has_default: true,
                default: None,
            },
            ParamExport {
                name: "path".to_string(),
                ty: TypeRef::Named {
                    origin: None,
                    name: "str".to_string(),
                },
                kind: ParamKindExport::Normal,
                has_default: false,
                default: None,
            },
        ],
        return_type: TypeRef::Named {
            origin: None,
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
                origin: None,
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
            origin: None,
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
    let export = super::model::function_export_from_checked(&crate::library_exports::CheckedFunctionExport {
        name: "with_default".to_string(),
        emitted_name: None,
        type_params: Vec::new(),
        params: vec![
            crate::symbols::CallableParam::named_with_default(
                "ok",
                crate::symbols::ResolvedType::Int,
                crate::ast::ParamKind::Normal,
                true,
            ),
            crate::symbols::CallableParam::named_with_default(
                "not_exportable",
                crate::symbols::ResolvedType::Int,
                crate::ast::ParamKind::Normal,
                true,
            ),
        ],
        param_defaults: vec![
            Some(crate::library_exports::CheckedParamDefault::Int(1)),
            Some(crate::library_exports::CheckedParamDefault::Unsupported),
        ],
        return_type: crate::symbols::ResolvedType::Unit,
        is_async: false,
    });

    assert!(export.params[0].has_default);
    assert_eq!(export.params[0].default, Some(ParamDefaultExport::Int(1)));
    assert!(!export.params[1].has_default);
    assert_eq!(export.params[1].default, None);
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
fn manifest_validation_rejects_invalid_partial_exports() -> Result<(), Box<dyn std::error::Error>> {
    let mut base = LibraryManifest::new("mylib", "0.1.0");
    base.exports.partials.push(PartialExport {
        name: "get".to_string(),
        target_path: vec!["route".to_string()],
        target_kind: PartialTargetKindExport::Function,
        presets: vec![PartialPresetExport {
            name: "method".to_string(),
            ty: TypeRef::Named {
                origin: None,
                name: "str".to_string(),
            },
            value: PresetValueExport::String("GET".to_string()),
        }],
        type_params: Vec::new(),
        params: Vec::new(),
        return_type: TypeRef::Named {
            origin: None,
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
                    origin: None,
                    name: "int".to_string(),
                },
                kind: ParamKindExport::RestPositional,
                has_default: false,
                default: None,
            },
            ParamExport {
                name: "labels".to_string(),
                ty: TypeRef::Named {
                    origin: None,
                    name: "str".to_string(),
                },
                kind: ParamKindExport::RestKeyword,
                has_default: false,
                default: None,
            },
        ],
        return_type: TypeRef::Named {
            origin: None,
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
                    origin: None,
                    name: "int".to_string(),
                },
                kind: ParamKindExport::RestPositional,
                has_default: false,
                default: None,
            }],
            return_type: TypeRef::Named {
                origin: None,
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
                    origin: None,
                    name: "str".to_string(),
                },
                kind: ParamKindExport::RestKeyword,
                has_default: false,
                default: None,
            },
            ParamExport {
                name: "value".to_string(),
                ty: TypeRef::Named {
                    origin: None,
                    name: "int".to_string(),
                },
                kind: ParamKindExport::Normal,
                has_default: false,
                default: None,
            },
        ],
        return_type: TypeRef::Named {
            origin: None,
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
            inferred: false,
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
        traits: vec!["Labeled".to_string()],
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
                origin: None,
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
                    inferred: false,
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
            origin: None,
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
        inferred: false,
    };
    let convert_float = TypeBoundExport {
        name: "Convert".to_string(),
        source_name: None,
        module_path: None,
        type_args: vec![TypeRef::Named {
            origin: None,
            name: "float".to_string(),
        }],
        implementation_type_params: Vec::new(),
        inferred: false,
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
                origin: None,
                name: "str".to_string(),
            }],
            implementation_type_params: Vec::new(),
            inferred: false,
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
