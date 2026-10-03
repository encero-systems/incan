//! Canonical identity validation at the manifest boundary: package identity paths, package-root nominal re-export
//! anchors, public rusttype and callable-alias identity kinds, the writer's rejection of malformed and duplicate
//! identities, and identity-graph validation of `pub from` re-exports of every kind.

use super::*;

#[test]
fn package_identity_path_keeps_same_named_module_and_declaration_segments() -> Result<(), Box<dyn std::error::Error>> {
    use crate::library_exports::{CheckedExportIdentity, CheckedExportKind, CheckedFunctionExport, CheckedNamedExport};
    use crate::symbols::ResolvedType;

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
            derivable_traits: Vec::new(),
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
                    origin: None,
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
    use crate::library_exports::{CheckedAliasExport, CheckedExportIdentity, CheckedExportKind, CheckedNamedExport};

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
            projected_type: None,
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
                derivable_traits: Vec::new(),
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
                derivable_traits: Vec::new(),
                module_path: vec!["lib".to_string()],
                declarations: vec![ApiDeclaration::Alias(ApiAlias {
                    name: "PublicRecord".to_string(),
                    anchor: SourceAnchor {
                        id: "lib::PublicRecord".to_string(),
                        span: SourceSpan { start: 30, end: 40 },
                    },
                    target_path: vec!["domain".to_string(), "Record".to_string()],
                    is_public: true,
                    projected_type: None,
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
    use crate::library_exports::{CheckedExportIdentity, CheckedExportKind, CheckedNamedExport, CheckedNewtypeExport};
    use crate::symbols::ResolvedType;

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
            derivable_traits: Vec::new(),
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
    use crate::library_exports::{CheckedExportKind, collect_checked_public_exports};
    use crate::typechecker::TypeChecker;

    let source = r#"
pub def route(method: str) -> str:
  return method

pub get = partial route(method="GET")
pub route_alias = alias route
pub fast_get = alias get
pub count_items = alias len
"#;
    let tokens = crate::lexer::lex(source)
        .map_err(|errors| std::io::Error::other(format!("callable alias fixture lex failed: {errors:?}")))?;
    let program = crate::parser::parse(&tokens)
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
    use crate::library_exports::{
        CheckedAliasExport, CheckedExportIdentity, CheckedExportKind, CheckedFunctionExport, CheckedNamedExport,
    };
    use crate::symbols::ResolvedType;

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
            projected_type: None,
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
            projected_type: None,
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

    let named = |name: &str| TypeRef::Named {
        origin: None,
        name: name.to_string(),
    };
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
            derivable_traits: Vec::new(),
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
                        origin: None,
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
                    projected_type: None,
                    projected_function: None,
                }),
            ],
        },
        CheckedApiMetadata {
            schema_version: CHECKED_API_METADATA_SCHEMA_VERSION,
            derivable_traits: Vec::new(),
            module_path: vec!["main".to_string()],
            declarations: vec![ApiDeclaration::Alias(ApiAlias {
                name: "public_target".to_string(),
                anchor: anchor("main.public_target", 0, 45),
                // The entrypoint spells the hop it re-exports; the projection below resolves past it.
                target_path: vec!["provider".to_string(), "run".to_string()],
                is_public: true,
                projected_type: None,
                projected_function: Some(crate::api_metadata::ApiProjectedFunction {
                    source_path: vec!["provider".to_string(), "helper".to_string()],
                    callable: crate::api_metadata::ApiCallableMetadata {
                        name: "public_target".to_string(),
                        anchor: anchor("main.public_target", 0, 45),
                        type_params: Vec::new(),
                        receiver: None,
                        params: Vec::new(),
                        return_type: TypeRef::Named {
                            origin: None,
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
        projected_type: None,
        projected_function: Some(FunctionExport {
            // The renamed re-export republishes the callable under its new public name.
            name: "public_target".to_string(),
            emitted_name: None,
            type_params: Vec::new(),
            params: Vec::new(),
            return_type: TypeRef::Named {
                origin: None,
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
