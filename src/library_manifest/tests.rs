use super::*;

#[test]
fn manifest_io_round_trip_preserves_recursive_types_and_bounds() -> Result<(), Box<dyn std::error::Error>> {
    let mut manifest = LibraryManifest::new("mylib", "0.1.0");
    manifest.exports.functions.push(FunctionExport {
        name: "map_result".to_string(),
        type_params: vec![TypeParamExport {
            name: "T".to_string(),
            bounds: vec![TypeBoundExport {
                name: "Clone".to_string(),
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
fn manifest_io_round_trip_preserves_rest_parameter_metadata() -> Result<(), Box<dyn std::error::Error>> {
    let mut manifest = LibraryManifest::new("mylib", "0.1.0");
    manifest.exports.functions.push(FunctionExport {
        name: "collect".to_string(),
        type_params: Vec::new(),
        params: vec![
            ParamExport {
                name: "items".to_string(),
                ty: TypeRef::Named {
                    name: "int".to_string(),
                },
                kind: ParamKindExport::RestPositional,
                has_default: false,
            },
            ParamExport {
                name: "labels".to_string(),
                ty: TypeRef::Named {
                    name: "str".to_string(),
                },
                kind: ParamKindExport::RestKeyword,
                has_default: false,
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
        type_params: Vec::new(),
        params: vec![
            ParamExport {
                name: "labels".to_string(),
                ty: TypeRef::Named {
                    name: "str".to_string(),
                },
                kind: ParamKindExport::RestKeyword,
                has_default: false,
            },
            ParamExport {
                name: "value".to_string(),
                ty: TypeRef::Named {
                    name: "int".to_string(),
                },
                kind: ParamKindExport::Normal,
                has_default: false,
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
        type_params: Vec::new(),
        supertraits: vec![TypeBoundExport {
            name: "Eq".to_string(),
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
        methods: Vec::new(),
        derives: Vec::new(),
    });
    manifest.exports.enums.push(EnumExport {
        name: "HttpStatus".to_string(),
        type_params: Vec::new(),
        traits: Vec::new(),
        trait_adoptions: Vec::new(),
        value_type: Some(EnumValueTypeExport::Int),
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
        variants: vec![EnumVariantExport {
            name: "Active".to_string(),
            fields: Vec::new(),
            value: None,
        }],
        methods: vec![MethodExport {
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
            name: "get".to_string(),
            type_params: vec![TypeParamExport {
                name: "T".to_string(),
                bounds: vec![TypeBoundExport {
                    name: "Clone".to_string(),
                    type_args: Vec::new(),
                }],
            }],
            receiver: Some(ReceiverExport::Immutable),
            params: vec![ParamExport {
                name: "value".to_string(),
                ty: TypeRef::TypeParam { name: "T".to_string() },
                kind: ParamKindExport::Normal,
                has_default: false,
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
        type_args: vec![TypeRef::Named {
            name: "int".to_string(),
        }],
    };
    let convert_float = TypeBoundExport {
        name: "Convert".to_string(),
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
        variants: vec![EnumVariantExport {
            name: "Number".to_string(),
            fields: Vec::new(),
            value: None,
        }],
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
fn manifest_reader_rejects_newer_required_compiler_version() -> Result<(), Box<dyn std::error::Error>> {
    let content = r#"{
  "name": "mylib",
  "version": "0.1.0",
  "incan_version": "999.0.0",
  "manifest_format": 1,
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

    let scoped_surfaces = &loaded.vocab.as_ref().unwrap().dsl_surfaces[0].scoped_surfaces;
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
        type_params: Vec::new(),
        params: Vec::new(),
        return_type: TypeRef::Unknown,
        is_async: false,
    });
    manifest.exports.functions.push(FunctionExport {
        name: "where_impl".to_string(),
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
