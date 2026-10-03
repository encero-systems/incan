//! Format and metadata gates: Rust ABI metadata and its schema versions, API metadata schema versions, compiled
//! provider metadata (features, facets, operation requirements, digests, artifact paths), soft keyword activations,
//! native union wire evidence, and the manifest format number gate.

use super::*;

#[test]
fn manifest_io_round_trip_preserves_rust_abi_metadata() -> Result<(), Box<dyn std::error::Error>> {
    use incan_lang::interop::{
        RustFunctionSig, RustItemKind, RustItemMetadata, RustParam, RustTypeInfo, RustVisibility,
    };

    let mut manifest = LibraryManifest::new("mylib", "0.1.0");
    manifest.rust_abi = LibraryRustAbi::from_items(vec![
        RustItemMetadata {
            canonical_path: "mylib_runtime::parse".to_string(),
            definition_path: Some("mylib_runtime::parse".to_string()),
            visibility: RustVisibility::Public,
            kind: RustItemKind::Function(RustFunctionSig {
                receiver_contract: None,
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
fn manifest_validation_rejects_duplicate_rust_abi_paths() -> Result<(), Box<dyn std::error::Error>> {
    use incan_lang::interop::{RustItemKind, RustItemMetadata, RustModuleInfo, RustVisibility};

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
        incan_lang::version::INCAN_VERSION,
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
            incan_lang::version::INCAN_VERSION,
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
        incan_lang::version::INCAN_VERSION,
        LIBRARY_MANIFEST_FORMAT,
        crate::api_metadata::CHECKED_API_METADATA_SCHEMA_VERSION + 1
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
        incan_lang::version::INCAN_VERSION,
        LIBRARY_MANIFEST_FORMAT,
        crate::api_metadata::CHECKED_API_METADATA_SCHEMA_VERSION,
        crate::api_metadata::CHECKED_API_METADATA_SCHEMA_VERSION + 1
    );

    let err = LibraryManifest::from_json_str(&raw);
    assert!(err.is_err(), "expected unsupported API metadata module schema to fail");
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

/// New native carriers retain the emitted wire evidence and exclude consumer-only paths; legacy unions still decode.
#[test]
fn native_union_wire_is_optional_and_excludes_checked_routes() -> Result<(), Box<dyn std::error::Error>> {
    use super::{NativeUnionExport, NativeUnionOwnerExport, TypeRef};
    let members = vec![
        TypeRef::Named {
            name: "int".into(),
            origin: None,
        },
        TypeRef::Named {
            name: "str".into(),
            origin: None,
        },
    ];
    let native = NativeUnionExport {
        owner: NativeUnionOwnerExport::ContainingArtifact,
        rust_name: "__IncanUnion0123456789abcdef".into(),
        members: members.clone(),
        local_nominals: Default::default(),
        checked_projection: Some(Box::new(super::model::NativeUnionProjection {
            dependency_root: "consumer_only".into(),
            rust_owner: "::consumer_only::pricing".into(),
            members: members.clone(),
            nominal_origins: Default::default(),
        })),
    };
    let wire = serde_json::to_string(&TypeRef::NativeUnion(native.clone()))?;
    assert!(!wire.contains("consumer_only"));
    assert!(!wire.contains("checked_projection"));
    assert!(
        !wire.contains("local_nominals"),
        "empty local binding metadata remains optional"
    );
    assert_eq!(
        serde_json::from_str::<TypeRef>(&wire)?,
        TypeRef::NativeUnion(native.for_publication())
    );
    let legacy = serde_json::json!({"Applied": {"name": "Union", "args": [{"Named": {"name": "int"}}, {"Named": {"name": "str"}}]}});
    assert_eq!(
        serde_json::from_value::<TypeRef>(legacy)?,
        TypeRef::Applied {
            name: "Union".into(),
            args: members,
            origin: None
        }
    );
    #[derive(serde::Deserialize)]
    enum LegacyTypeRef {
        Unknown,
    }
    let error = serde_json::from_str::<LegacyTypeRef>(&wire)
        .err()
        .ok_or("an older reader must reject the new variant")?;
    assert!(error.to_string().contains("unknown variant `NativeUnion`"));
    Ok(())
}

/// A manifest from a newer format is refused by format number, not by whatever field parsed first.
///
/// This is the whole reason the format gate runs before the body decode. `RawLibraryManifest` decodes every typed
/// field, so a future manifest carrying a `TypeRef` variant this build does not know would otherwise die inside
/// serde and report an opaque parse error for what is really a version mismatch. The unknown variant below stands
/// in for exactly that: without the gate the message names a type-reference field, with it the message names the
/// format.
#[test]
fn a_newer_manifest_format_is_refused_by_number_not_by_a_parse_error() -> Result<(), Box<dyn std::error::Error>> {
    let future = format!(
        r#"{{"manifest_format": {}, "incan_version": "0.6.0", "name": "future", "version": "1.0.0",
            "exports": {{}}, "some_field_this_build_has_never_seen": {{"shape": ["anything", 1, null]}}}}"#,
        LIBRARY_MANIFEST_FORMAT + 1
    );
    let error = LibraryManifest::from_json_str(&future)
        .err()
        .ok_or("expected a refusal")?;
    let message = error.to_string();
    assert!(
        message.contains("unsupported manifest_format") && message.contains(&(LIBRARY_MANIFEST_FORMAT + 1).to_string()),
        "a newer format must be refused by number, got: {message}"
    );
    Ok(())
}

/// A manifest declaring the current format still decodes through the ordinary path.
///
/// The gate must add a refusal without taking one over: a manifest at the supported format that is missing a
/// required field has to keep reaching the decoder, which names the field, rather than being short-circuited.
#[test]
fn the_format_gate_does_not_swallow_ordinary_validation() -> Result<(), Box<dyn std::error::Error>> {
    // Complete except for `soft_keywords`, so the only refusal available is the decoder's own.
    let malformed = format!(
        r#"{{"manifest_format": {LIBRARY_MANIFEST_FORMAT}, "incan_version": "1.0.0", "name": "current",
            "version": "1.0.0", "exports": {{}}}}"#
    );
    let error = LibraryManifest::from_json_str(&malformed)
        .err()
        .ok_or("expected a refusal")?;
    assert!(
        !error.to_string().contains("unsupported manifest_format"),
        "the gate must not claim a format problem for a supported format, got: {error}"
    );
    assert!(
        error.to_string().contains("failed to parse library manifest") && error.to_string().contains("soft_keywords"),
        "the decoder must name the missing field, got: {error}"
    );
    Ok(())
}
