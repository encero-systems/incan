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
