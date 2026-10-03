//! Inspected Rust metadata: shipped-library ABI precedence, type compatibility with cached definitions, method
//! resolution on Rust receivers, async Rust methods, rusttype aliases over Rust methods, nested match bindings on Rust
//! fields, prost `oneof` payloads, and the no-inspector fallbacks.

use super::*;

#[test]
fn rust_item_metadata_prefers_shipped_library_abi() {
    let manifest_metadata = RustItemMetadata {
        canonical_path: "demo_runtime::parse".to_string(),
        definition_path: Some("demo_runtime::parse".to_string()),
        visibility: RustVisibility::Public,
        kind: RustItemKind::Function(RustFunctionSig {
            receiver_contract: None,
            type_params: Vec::new(),
            params: vec![RustParam {
                name: Some("source".to_string()),
                type_display: "&str".to_string(),
            }],
            return_type: "demo_runtime::Plan".to_string(),
            is_async: false,
            is_unsafe: false,
        }),
    };

    let mut checker = TypeChecker::new();
    checker.set_library_manifest_index(library_index_with_rust_abi_item(
        "demo_runtime_parse",
        manifest_metadata.clone(),
    ));

    let Some(actual) = checker.rust_item_metadata_for_path("rust::demo_runtime::parse") else {
        panic!("expected shipped Rust ABI metadata");
    };
    assert_eq!(actual, manifest_metadata);
}

#[cfg(feature = "rust_inspect")]
#[test]
fn test_rust_inspect_unavailable_stays_permissive_for_method_calls() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
from rust::regex import Regex

def f() -> None:
  _ = Regex.no_such_method("x")
"#;
    let tokens = lexer::lex(source).map_err(|errs| std::io::Error::other(format!("lex failed: {errs:?}")))?;
    let ast = parser::parse(&tokens).map_err(|errs| std::io::Error::other(format!("parse failed: {errs:?}")))?;
    let mut checker = TypeChecker::new();
    // Leave rust-inspect disabled for this checker: no manifest dir means cache-only permissive fallback.
    let result = checker.check_program(&ast);
    assert!(
        result.is_ok(),
        "expected permissive fallback when metadata is unavailable, got {result:?}"
    );
    Ok(())
}

#[cfg(feature = "rust_inspect")]
#[test]
fn compiler_owned_function_contract_overrides_stale_warm_metadata() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
from rust::incan_std_core::strings import str_slice_byte_range

def slice(text: str) -> str:
  return str_slice_byte_range(text, 0, 1)
"#;
    let tokens = lexer::lex(source).map_err(|errors| std::io::Error::other(format!("lex failed: {errors:?}")))?;
    let ast = parser::parse(&tokens).map_err(|errors| std::io::Error::other(format!("parse failed: {errors:?}")))?;
    let mut checker = TypeChecker::new();
    let tmp = seeded_rust_inspect_workspace()?;
    let manifest_dir = tmp.path().to_path_buf();
    checker.set_rust_inspect_manifest_dir(manifest_dir.clone());
    checker
        .rust_inspect_cache
        .insert_test_item(
            &manifest_dir,
            RustItemMetadata {
                canonical_path: "incan_std_core::strings::str_slice_byte_range".to_string(),
                definition_path: Some("incan_std_core::strings::str_slice_byte_range".to_string()),
                visibility: RustVisibility::Public,
                kind: RustItemKind::Function(RustFunctionSig {
                    receiver_contract: None,
                    type_params: Vec::new(),
                    params: vec![
                        RustParam {
                            name: Some("s".to_string()),
                            type_display: "&str".to_string(),
                        },
                        RustParam {
                            name: Some("start".to_string()),
                            type_display: "i64".to_string(),
                        },
                        RustParam {
                            name: Some("end".to_string()),
                            type_display: "i64".to_string(),
                        },
                    ],
                    return_type: "i64".to_string(),
                    is_async: false,
                    is_unsafe: false,
                }),
            },
        )
        .map_err(|error| std::io::Error::other(format!("seed stale rust-inspect function: {error}")))?;
    let Some(RustItemMetadata {
        kind: RustItemKind::Function(stale_signature),
        ..
    }) = checker.rust_item_metadata_for_path("incan_std_core::strings::str_slice_byte_range")
    else {
        return Err(std::io::Error::other("expected stale warm helper metadata to be visible").into());
    };
    assert_eq!(
        stale_signature.return_type, "i64",
        "the regression must exercise a warm metadata result that disagrees with the compiler-owned contract"
    );

    checker.check_program(&ast).map_err(|errors| {
        std::io::Error::other(format!(
            "compiler-owned String return must win over stale warm metadata: {errors:?}"
        ))
    })?;
    Ok(())
}

#[cfg(feature = "rust_inspect")]
#[test]
fn test_rust_inspect_function_signature_preserves_borrowed_rust_path_param() -> Result<(), Box<dyn std::error::Error>> {
    let mut checker = TypeChecker::new();
    let tmp = seeded_rust_inspect_workspace()?;
    let manifest_dir = tmp.path().to_path_buf();
    checker.set_rust_inspect_manifest_dir(manifest_dir.clone());
    checker
        .rust_inspect_cache
        .insert_test_item(
            &manifest_dir,
            RustItemMetadata {
                canonical_path: "demo::takes_ref".to_string(),
                definition_path: Some("demo::takes_ref".to_string()),
                visibility: RustVisibility::Public,
                kind: RustItemKind::Function(RustFunctionSig {
                    receiver_contract: None,
                    type_params: Vec::new(),
                    params: vec![RustParam {
                        name: Some("value".to_string()),
                        type_display: "&demo::Thing".to_string(),
                    }],
                    return_type: "()".to_string(),
                    is_async: false,
                    is_unsafe: false,
                }),
            },
        )
        .map_err(|e| std::io::Error::other(format!("seed rust-inspect function: {e}")))?;
    let Some(RustItemMetadata {
        kind: RustItemKind::Function(sig),
        ..
    }) = checker.rust_item_metadata_for_path("demo::takes_ref")
    else {
        return Err(std::io::Error::other("expected rust-inspect function entry").into());
    };
    assert_eq!(
        checker.resolved_function_type_from_rust_sig_for_owner_path(&sig, false, "demo::takes_ref"),
        ResolvedType::Function(
            vec![CallableParam::positional(ResolvedType::Ref(Box::new(
                ResolvedType::RustPath("demo::Thing".to_string())
            )))],
            Box::new(ResolvedType::Unit),
        )
    );
    Ok(())
}

#[cfg(feature = "rust_inspect")]
#[test]
fn test_rust_item_metadata_lookup_reuses_cached_nominal_item_for_instantiated_rust_path()
-> Result<(), Box<dyn std::error::Error>> {
    let mut checker = TypeChecker::new();
    let tmp = seeded_rust_inspect_workspace()?;
    let manifest_dir = tmp.path().to_path_buf();
    checker.set_rust_inspect_manifest_dir(manifest_dir.clone());
    checker
        .rust_inspect_cache
        .insert_test_item(
            &manifest_dir,
            RustItemMetadata {
                canonical_path: "demo::SendError".to_string(),
                definition_path: Some("demo::SendError".to_string()),
                visibility: RustVisibility::Public,
                kind: RustItemKind::Type(RustTypeInfo {
                    type_params: Vec::new(),
                    type_param_defaults: Vec::new(),
                    mutable_reference_type_params: Vec::new(),
                    expanded_derive_traits: Vec::new(),
                    has_const_params: false,
                    alias_target: None,
                    metadata_completeness: Default::default(),
                    fields: Vec::new(),
                    methods: Vec::new(),
                    implemented_traits: Vec::new(),
                    variants: Vec::new(),
                }),
            },
        )
        .map_err(|e| std::io::Error::other(format!("seed rust-inspect type: {e}")))?;

    let Some(meta) = checker.rust_item_metadata_for_path("demo::SendError<T>") else {
        return Err(std::io::Error::other("expected nominal rust-inspect hit").into());
    };
    assert_eq!(meta.canonical_path, "demo::SendError");
    Ok(())
}

#[cfg(feature = "rust_inspect")]
#[test]
fn test_rust_constant_identifier_records_value_kind_for_lowering() {
    let mut checker = TypeChecker::new();
    let span = Span::new(0, "UNIX_EPOCH".len());
    checker.symbols.define(Symbol {
        name: "UNIX_EPOCH".to_string(),
        kind: SymbolKind::RustItem(RustItemInfo {
            crate_name: "std".to_string(),
            path: "std::time::UNIX_EPOCH".to_string(),
            binding: RustImportBindingKind::FromImport,
            metadata: Some(RustItemMetadata {
                canonical_path: "std::time::UNIX_EPOCH".to_string(),
                definition_path: Some("std::time::UNIX_EPOCH".to_string()),
                visibility: RustVisibility::Public,
                kind: RustItemKind::Constant {
                    type_display: "std::time::SystemTime".to_string(),
                },
            }),
        }),
        span,
        scope: 0,
    });

    let expr = Spanned::new(Expr::Ident("UNIX_EPOCH".to_string()), span);
    let ty = checker.check_expr(&expr);

    assert_eq!(
        checker.type_info().ident_kind(span),
        Some(IdentKind::RustValue),
        "Rust constants must lower as values so `UNIX_EPOCH.method()` emits `UNIX_EPOCH.method()`"
    );
    assert_eq!(ty, ResolvedType::RustPath("std::time::SystemTime".to_string()));
}

#[test]
fn test_rust_constant_identifier_without_metadata_uses_const_name_fallback() {
    let mut checker = TypeChecker::new();
    let span = Span::new(0, "UNIX_EPOCH".len());
    checker.symbols.define(Symbol {
        name: "UNIX_EPOCH".to_string(),
        kind: SymbolKind::RustItem(RustItemInfo {
            crate_name: "std".to_string(),
            path: "std::time::UNIX_EPOCH".to_string(),
            binding: RustImportBindingKind::FromImport,
            metadata: None,
        }),
        span,
        scope: 0,
    });

    let expr = Spanned::new(Expr::Ident("UNIX_EPOCH".to_string()), span);
    let _ = checker.check_expr(&expr);

    assert_eq!(
        checker.type_info().ident_kind(span),
        Some(IdentKind::RustValue),
        "metadata-free Rust constants should still lower as values when the imported Rust item uses const naming"
    );
}

#[cfg(feature = "rust_inspect")]
#[test]
fn test_types_compatible_accepts_rust_alias_definition_without_metadata_lookup() {
    let mut checker = TypeChecker::new();
    checker.symbols.define(Symbol {
        name: "RawSender".to_string(),
        kind: SymbolKind::RustItem(RustItemInfo {
            crate_name: "incan_std_core".to_string(),
            path: "incan_std_async::channel::RawSender".to_string(),
            binding: RustImportBindingKind::FromImport,
            metadata: Some(RustItemMetadata {
                canonical_path: "incan_std_async::channel::RawSender".to_string(),
                definition_path: Some("incan_std_async::channel::Sender".to_string()),
                visibility: RustVisibility::Public,
                kind: RustItemKind::Type(RustTypeInfo {
                    type_params: Vec::new(),
                    type_param_defaults: Vec::new(),
                    mutable_reference_type_params: Vec::new(),
                    expanded_derive_traits: Vec::new(),
                    has_const_params: false,
                    alias_target: None,
                    metadata_completeness: Default::default(),
                    fields: Vec::new(),
                    methods: Vec::new(),
                    implemented_traits: Vec::new(),
                    variants: Vec::new(),
                }),
            }),
        }),
        span: Span::default(),
        scope: 0,
    });

    let actual = ResolvedType::Generic("RawSender".to_string(), vec![ResolvedType::Numeric(NumericTypeId::I32)]);
    let expected = ResolvedType::Ref(Box::new(ResolvedType::RustPath(
        "incan_std_async::channel::Sender<i32>".to_string(),
    )));

    assert!(
        checker.types_compatible(&actual, &expected),
        "Rust alias should satisfy borrowed underlying Rust path without forcing fresh metadata extraction"
    );
}

#[cfg(feature = "rust_inspect")]
#[test]
fn test_types_compatible_accepts_rust_path_alias_with_attached_definition_metadata() {
    let mut checker = TypeChecker::new();
    checker.symbols.define(Symbol {
        name: "RawSemaphore".to_string(),
        kind: SymbolKind::RustItem(RustItemInfo {
            crate_name: "incan_std_core".to_string(),
            path: "incan_std_async::sync::RawSemaphore".to_string(),
            binding: RustImportBindingKind::FromImport,
            metadata: Some(RustItemMetadata {
                canonical_path: "incan_std_async::sync::RawSemaphore".to_string(),
                definition_path: Some("incan_std_async::sync::Semaphore".to_string()),
                visibility: RustVisibility::Public,
                kind: RustItemKind::Type(RustTypeInfo {
                    type_params: Vec::new(),
                    type_param_defaults: Vec::new(),
                    mutable_reference_type_params: Vec::new(),
                    expanded_derive_traits: Vec::new(),
                    has_const_params: false,
                    alias_target: None,
                    metadata_completeness: Default::default(),
                    fields: Vec::new(),
                    methods: Vec::new(),
                    implemented_traits: Vec::new(),
                    variants: Vec::new(),
                }),
            }),
        }),
        span: Span::default(),
        scope: 0,
    });

    let actual = ResolvedType::RustPath("incan_std_async::sync::RawSemaphore".to_string());
    let expected = ResolvedType::Ref(Box::new(ResolvedType::RustPath(
        "incan_std_async::sync::Semaphore".to_string(),
    )));

    assert!(
        checker.types_compatible(&actual, &expected),
        "RustPath aliases should reuse attached import metadata instead of forcing external metadata lookup"
    );
}

#[cfg(feature = "rust_inspect")]
#[test]
fn test_types_compatible_accepts_reexported_rust_types_with_expanded_default_arguments() {
    let mut checker = TypeChecker::new();
    let mut define_rust_type = |name: &str,
                                path: &str,
                                definition_path: &str,
                                type_params: Vec<String>,
                                type_param_defaults: Vec<Option<String>>| {
        checker.symbols.define(Symbol {
            name: name.to_string(),
            kind: SymbolKind::RustItem(RustItemInfo {
                crate_name: path.split("::").next().unwrap_or_default().to_string(),
                path: path.to_string(),
                binding: RustImportBindingKind::FromImport,
                metadata: Some(RustItemMetadata {
                    canonical_path: path.to_string(),
                    definition_path: Some(definition_path.to_string()),
                    visibility: RustVisibility::Public,
                    kind: RustItemKind::Type(RustTypeInfo {
                        type_params,
                        type_param_defaults,
                        mutable_reference_type_params: Vec::new(),
                        expanded_derive_traits: Vec::new(),
                        has_const_params: false,
                        alias_target: None,
                        metadata_completeness: Default::default(),
                        fields: Vec::new(),
                        methods: Vec::new(),
                        implemented_traits: Vec::new(),
                        variants: Vec::new(),
                    }),
                }),
            }),
            span: Span::default(),
            scope: 0,
        });
    };
    define_rust_type(
        "RustNamedTemporaryFile",
        "tempfile::NamedTempFile",
        "tempfile::file::NamedTempFile",
        vec!["F".to_string()],
        vec![Some("std::fs::File".to_string())],
    );
    define_rust_type(
        "RustTemporaryDirectory",
        "tempfile::TempDir",
        "tempfile::dir::TempDir",
        Vec::new(),
        Vec::new(),
    );
    define_rust_type(
        "RustIoError",
        "std::io::Error",
        "core::io::error::Error",
        Vec::new(),
        Vec::new(),
    );

    let expected_file = ResolvedType::Generic(
        collection_name(CollectionTypeId::Result).to_string(),
        vec![
            ResolvedType::Named("RustNamedTemporaryFile".to_string()),
            ResolvedType::Named("RustIoError".to_string()),
        ],
    );
    let actual_file = checker.resolved_type_from_rust_display(
        "Result<tempfile::file::NamedTempFile<std::fs::File>, core::io::error::Error>",
    );
    let file_matches = checker.types_compatible(&actual_file, &expected_file);

    let expected_directory = ResolvedType::Generic(
        collection_name(CollectionTypeId::Result).to_string(),
        vec![
            ResolvedType::Named("RustTemporaryDirectory".to_string()),
            ResolvedType::Named("RustIoError".to_string()),
        ],
    );
    let actual_directory =
        checker.resolved_type_from_rust_display("Result<tempfile::dir::TempDir, core::io::error::Error>");
    let directory_matches = checker.types_compatible(&actual_directory, &expected_directory);
    assert!(
        file_matches,
        "a public Rust type with an omitted default parameter must match its expanded defining type; non-generic re-export match = {directory_matches}"
    );
    assert!(
        directory_matches,
        "non-generic public Rust re-exports must match the same defining types"
    );
}

#[cfg(feature = "rust_inspect")]
#[test]
fn test_types_compatible_keeps_rust_paths_permissive_without_definition_metadata() {
    let checker = TypeChecker::new();
    let actual = ResolvedType::RustPath("rust::datafusion_substrait::substrait::proto::Plan".to_string());
    let expected = ResolvedType::Ref(Box::new(ResolvedType::RustPath("substrait::proto::Plan".to_string())));
    assert!(
        checker.types_compatible(&actual, &expected),
        "Rust path compatibility should stay permissive when definition metadata is unavailable"
    );
}

#[test]
fn test_types_compatible_elides_lifetime_only_rust_generic_args() {
    let checker = TypeChecker::new();
    let actual = ResolvedType::RustPath("datafusion::execution::options::ArrowReadOptions<i64>".to_string());
    let expected = checker.resolved_type_from_rust_display(
        "datafusion::execution::options::ArrowReadOptions<datafusion::execution::context::'_, i64>",
    );
    let mismatched = checker.resolved_type_from_rust_display(
        "datafusion::execution::options::ArrowReadOptions<datafusion::execution::context::'_, String>",
    );

    assert!(
        checker.types_compatible(&actual, &expected),
        "lifetime-only Rust generic arguments must not become Incan type arguments"
    );
    assert!(
        !checker.types_compatible(&actual, &mismatched),
        "eliding a lifetime must not hide a remaining concrete generic-argument mismatch"
    );
}

#[test]
fn test_rust_method_parameter_paths_resolve_from_receiver_not_synthetic_callable() {
    let checker = TypeChecker::new();
    let owner =
        TypeChecker::rust_method_owner_path("rust::datafusion::execution::context::SessionContext.register_parquet");
    let parameter = checker.rust_display_for_owner_path(
        "datafusion::execution::context::super::options::ParquetReadOptions<datafusion::execution::context::parquet::'_>",
        owner,
    );

    assert_eq!(owner, "datafusion::execution::context::SessionContext");
    assert_eq!(
        parameter,
        "datafusion::execution::options::ParquetReadOptions<datafusion::execution::context::parquet::'_>"
    );
    assert!(checker.rust_arg_matches_boundary(
        &ResolvedType::RustPath("datafusion::execution::options::ParquetReadOptions".to_string()),
        parameter.as_str(),
    ));
}

#[test]
fn test_hashset_lookup_records_preserved_arg_shape_for_imported_generic_receiver()
-> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
from rust::std::collections import HashSet

def f(words: HashSet[str]) -> None:
  _ = words.contains("the")
"#;
    let tokens = lexer::lex(source).map_err(|errs| std::io::Error::other(format!("lex failed: {errs:?}")))?;
    let ast = parser::parse(&tokens).map_err(|errs| std::io::Error::other(format!("parse failed: {errs:?}")))?;
    let mut checker = TypeChecker::new();
    checker
        .check_program(&ast)
        .map_err(|errs| std::io::Error::other(format!("expected imported HashMap lookup to typecheck: {errs:?}")))?;
    let info = checker.type_info();
    assert!(
        info.rust
            .regular_method_arg_shape_preserving_calls
            .iter()
            .any(|(_, _, method)| method == "contains"),
        "expected HashSet.contains lookup to record preserved method arg shape, got {:?}",
        info.rust.regular_method_arg_shape_preserving_calls
    );
    Ok(())
}

/// Field access on a Rust type that isn't a known inherent associated function should be permissive (metadata only
/// covers inherent methods, not consts, type aliases, or trait-provided items).
#[test]
fn test_rust_path_field_access_permissive_when_not_module() {
    let source = r#"
from rust::std::time import Instant

def f() -> None:
  _ = Instant.SOME_UNKNOWN_CONST
"#;
    assert_check_ok(source);
}

/// Method calls on Rust types where the specific method isn't in inherent metadata should be permissive (trait-provided
/// or extension methods aren't extracted yet).
#[test]
fn test_rust_path_method_call_permissive_for_unextracted_methods() {
    let source = r#"
from rust::std::time import Instant

def f() -> None:
  t = Instant.now()
  _ = t.some_trait_method()
"#;
    assert_check_ok(source);
}

#[cfg(feature = "rust_inspect")]
#[test]
fn test_typechecker_defaults_to_no_rust_inspect_workspace() {
    let checker = TypeChecker::new();
    assert!(
        checker.rust_inspect_manifest_dir.is_none(),
        "plain typechecker construction should not eagerly bind a rust-inspect workspace"
    );
}

/// Default builds omit `rust-inspect`; Rust receivers stay permissive (no method index).
#[cfg(not(feature = "rust_inspect"))]
#[test]
fn test_without_rust_inspect_missing_rust_method_is_not_an_error() {
    let source = r#"
from rust::regex import Regex

def f() -> None:
  _ = Regex.no_such_method("x")
"#;
    assert_check_ok(source);
}

#[cfg(feature = "rust_inspect")]
fn seed_async_rust_method_probe(
    checker: &mut TypeChecker,
    manifest_dir: &std::path::Path,
) -> Result<(), Box<dyn std::error::Error>> {
    seed_async_rust_method_probe_with_options_param(checker, manifest_dir, "demo::CsvReadOptions")
}

#[cfg(feature = "rust_inspect")]
fn seed_async_rust_method_probe_with_options_param(
    checker: &mut TypeChecker,
    manifest_dir: &std::path::Path,
    options_param_type: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    checker.rust_inspect_cache.insert_test_item(
        manifest_dir,
        RustItemMetadata {
            canonical_path: "demo::SessionContext".to_string(),
            definition_path: Some("demo::SessionContext".to_string()),
            visibility: RustVisibility::Public,
            kind: RustItemKind::Type(RustTypeInfo {
                type_params: Vec::new(),
                type_param_defaults: Vec::new(),
                mutable_reference_type_params: Vec::new(),
                expanded_derive_traits: Vec::new(),
                has_const_params: false,
                alias_target: None,
                metadata_completeness: Default::default(),
                methods: vec![
                    RustMethodSig {
                        name: "new".to_string(),
                        signature: RustFunctionSig {
                            receiver_contract: None,
                            type_params: Vec::new(),
                            params: Vec::new(),
                            return_type: "demo::SessionContext".to_string(),
                            is_async: false,
                            is_unsafe: false,
                        },
                    },
                    RustMethodSig {
                        name: "register_csv".to_string(),
                        signature: RustFunctionSig {
                            receiver_contract: None,
                            type_params: Vec::new(),
                            params: vec![
                                RustParam {
                                    name: Some("self".to_string()),
                                    type_display: "&self".to_string(),
                                },
                                RustParam {
                                    name: Some("name".to_string()),
                                    type_display: "&str".to_string(),
                                },
                                RustParam {
                                    name: Some("path".to_string()),
                                    type_display: "&str".to_string(),
                                },
                                RustParam {
                                    name: Some("options".to_string()),
                                    type_display: options_param_type.to_string(),
                                },
                            ],
                            return_type: "Result<(), demo::DataFusionError>".to_string(),
                            is_async: true,
                            is_unsafe: false,
                        },
                    },
                ],
                implemented_traits: Vec::new(),
                fields: vec![],
                variants: vec![],
            }),
        },
    )?;
    checker.rust_inspect_cache.insert_test_item(
        manifest_dir,
        RustItemMetadata {
            canonical_path: "demo::CsvReadOptions".to_string(),
            definition_path: Some("demo::CsvReadOptions".to_string()),
            visibility: RustVisibility::Public,
            kind: RustItemKind::Type(RustTypeInfo {
                type_params: Vec::new(),
                type_param_defaults: Vec::new(),
                mutable_reference_type_params: Vec::new(),
                expanded_derive_traits: Vec::new(),
                has_const_params: false,
                alias_target: None,
                metadata_completeness: Default::default(),
                methods: vec![RustMethodSig {
                    name: "new".to_string(),
                    signature: RustFunctionSig {
                        receiver_contract: None,
                        type_params: Vec::new(),
                        params: Vec::new(),
                        return_type: "demo::CsvReadOptions".to_string(),
                        is_async: false,
                        is_unsafe: false,
                    },
                }],
                implemented_traits: Vec::new(),
                fields: vec![],
                variants: vec![],
            }),
        },
    )?;
    checker.rust_inspect_cache.insert_test_item(
        manifest_dir,
        RustItemMetadata {
            canonical_path: "demo::make_context".to_string(),
            definition_path: Some("demo::make_context".to_string()),
            visibility: RustVisibility::Public,
            kind: RustItemKind::Function(RustFunctionSig {
                receiver_contract: None,
                type_params: Vec::new(),
                params: Vec::new(),
                return_type: "demo::SessionContext".to_string(),
                is_async: false,
                is_unsafe: false,
            }),
        },
    )?;
    checker.rust_inspect_cache.insert_test_item(
        manifest_dir,
        RustItemMetadata {
            canonical_path: "demo::make_options".to_string(),
            definition_path: Some("demo::make_options".to_string()),
            visibility: RustVisibility::Public,
            kind: RustItemKind::Function(RustFunctionSig {
                receiver_contract: None,
                type_params: Vec::new(),
                params: Vec::new(),
                return_type: "demo::CsvReadOptions".to_string(),
                is_async: false,
                is_unsafe: false,
            }),
        },
    )?;
    Ok(())
}

#[cfg(feature = "rust_inspect")]
#[test]
fn test_rust_async_method_call_can_be_awaited() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
import std.async
from rust::demo import SessionContext
from rust::demo import CsvReadOptions
from rust::demo import make_context
from rust::demo import make_options

pub async def register_csv_with_await() -> None:
  ctx = make_context()
  opts = make_options()
  match await ctx.register_csv("orders", "orders.csv", opts):
    Ok(_) => pass
    Err(_) => pass
"#;
    let tokens = lexer::lex(source).map_err(|errs| std::io::Error::other(format!("lex failed: {errs:?}")))?;
    let ast = parser::parse(&tokens).map_err(|errs| std::io::Error::other(format!("parse failed: {errs:?}")))?;
    let mut checker = TypeChecker::new();
    let tmp = seeded_rust_inspect_workspace()?;
    checker.set_rust_inspect_manifest_dir(tmp.path().to_path_buf());
    seed_async_rust_method_probe(&mut checker, tmp.path())?;
    checker.check_program(&ast).map_err(|errs| {
        std::io::Error::other(format!(
            "expected awaited Rust async method call to typecheck: {errs:?}"
        ))
    })?;
    Ok(())
}

#[cfg(feature = "rust_inspect")]
#[test]
fn test_rust_async_method_call_accepts_imported_type_with_unknown_generic_metadata()
-> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
import std.async
from rust::demo import SessionContext
from rust::demo import CsvReadOptions
from rust::demo import make_context
from rust::demo import make_options

pub async def register_csv_with_unknown_options_metadata() -> None:
  ctx = make_context()
  opts = make_options()
  match await ctx.register_csv("orders", "orders.csv", opts):
    Ok(_) => pass
    Err(_) => pass
"#;
    let tokens = lexer::lex(source).map_err(|errs| std::io::Error::other(format!("lex failed: {errs:?}")))?;
    let ast = parser::parse(&tokens).map_err(|errs| std::io::Error::other(format!("parse failed: {errs:?}")))?;
    let mut checker = TypeChecker::new();
    let tmp = seeded_rust_inspect_workspace()?;
    checker.set_rust_inspect_manifest_dir(tmp.path().to_path_buf());
    seed_async_rust_method_probe_with_options_param(&mut checker, tmp.path(), "demo::CsvReadOptions<?>")?;
    checker.check_program(&ast).map_err(|errs| {
        std::io::Error::other(format!(
            "expected Rust async method to accept an imported Rust type when metadata has only unknown generic args: {errs:?}"
        ))
    })?;
    Ok(())
}

#[cfg(feature = "rust_inspect")]
#[test]
fn test_rust_async_method_call_without_await_is_rejected() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
import std.async
from rust::demo import SessionContext
from rust::demo import CsvReadOptions
from rust::demo import make_context
from rust::demo import make_options

pub async def register_csv_without_await() -> None:
  ctx = make_context()
  opts = make_options()
  match ctx.register_csv("orders", "orders.csv", opts):
    Ok(_) => pass
    Err(_) => pass
"#;
    let tokens = lexer::lex(source).map_err(|errs| std::io::Error::other(format!("lex failed: {errs:?}")))?;
    let ast = parser::parse(&tokens).map_err(|errs| std::io::Error::other(format!("parse failed: {errs:?}")))?;
    let mut checker = TypeChecker::new();
    let tmp = seeded_rust_inspect_workspace()?;
    checker.set_rust_inspect_manifest_dir(tmp.path().to_path_buf());
    seed_async_rust_method_probe(&mut checker, tmp.path())?;
    let Err(errs) = checker.check_program(&ast) else {
        return Err(std::io::Error::other("expected un-awaited Rust async method call to fail").into());
    };
    assert!(
        errs.iter()
            .any(|err| err.message.contains("Awaitable[Result") && err.message.contains("does not resolve")),
        "expected un-awaited Rust async method call to expose an Awaitable Result before matching, got {errs:?}"
    );
    Ok(())
}

#[cfg(feature = "rust_inspect")]
#[test]
fn test_rusttype_alias_resolves_underlying_rust_methods() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
from rust::std::string import String as RustString

type Label = rusttype RustString

def render(value: Label) -> str:
    return value.as_str()
"#;
    let tokens = lexer::lex(source).map_err(|errs| std::io::Error::other(format!("lex failed: {errs:?}")))?;
    let ast = parser::parse(&tokens).map_err(|errs| std::io::Error::other(format!("parse failed: {errs:?}")))?;
    let mut checker = TypeChecker::new();
    let tmp = seeded_rust_inspect_workspace()?;
    let manifest_dir = tmp.path().to_path_buf();
    checker.set_rust_inspect_manifest_dir(manifest_dir.clone());
    checker
        .rust_inspect_cache
        .insert_test_item(
            &manifest_dir,
            RustItemMetadata {
                canonical_path: "std::string::String".to_string(),
                definition_path: Some("std::string::String".to_string()),
                visibility: RustVisibility::Public,
                kind: RustItemKind::Type(RustTypeInfo {
                    type_params: Vec::new(),
                    type_param_defaults: Vec::new(),
                    mutable_reference_type_params: Vec::new(),
                    expanded_derive_traits: Vec::new(),
                    has_const_params: false,
                    alias_target: None,
                    metadata_completeness: Default::default(),
                    methods: vec![RustMethodSig {
                        name: "as_str".to_string(),
                        signature: RustFunctionSig {
                            receiver_contract: None,
                            type_params: Vec::new(),
                            params: vec![RustParam {
                                name: Some("self".to_string()),
                                type_display: "&self".to_string(),
                            }],
                            return_type: "&str".to_string(),
                            is_async: false,
                            is_unsafe: false,
                        },
                    }],
                    implemented_traits: Vec::new(),
                    fields: vec![],
                    variants: vec![],
                }),
            },
        )
        .map_err(|e| std::io::Error::other(format!("seed rust-inspect: {e}")))?;
    checker.check_program(&ast).map_err(|errs| {
        std::io::Error::other(format!(
            "expected rusttype alias receiver to expose underlying Rust methods: {errs:?}"
        ))
    })?;
    let info = checker.type_info();
    assert!(
        info.expressions
            .expr_types
            .values()
            .any(|ty| matches!(ty, ResolvedType::Str)),
        "expected underlying rusttype method call to resolve to str, got {:?}",
        info.expressions.expr_types
    );
    assert!(
        info.rust
            .return_coercions
            .values()
            .any(|c| c.rust_target_type == "String" && matches!(c.target_type, ResolvedType::Str)),
        "expected borrowed Rust method return to be owned as Incan str, got {:?}",
        info.rust.return_coercions
    );
    Ok(())
}

#[cfg(feature = "rust_inspect")]
#[test]
fn test_rust_field_access_preserves_type_for_nested_match_binding() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
def id[T](x: T) -> T:
  return x

from rust::demo import Envelope as RustEnvelope
from rust::demo import Kind as RustKind

type Envelope = rusttype RustEnvelope:
  def noop(self) -> None:
    ...

type Kind = rusttype RustKind:
  def noop(self) -> None:
    ...

def f(x: Envelope) -> None:
  match x.kind:
    Some(Kind.A(inner)) =>
      _ = id(inner)
    None =>
      _ = 0
"#;
    let tokens = lexer::lex(source).map_err(|errs| std::io::Error::other(format!("lex failed: {errs:?}")))?;
    let ast = parser::parse(&tokens).map_err(|errs| std::io::Error::other(format!("parse failed: {errs:?}")))?;
    let mut checker = TypeChecker::new();
    let tmp = seeded_rust_inspect_workspace()?;
    let manifest_dir = tmp.path().to_path_buf();
    checker.set_rust_inspect_manifest_dir(manifest_dir.clone());
    checker
        .rust_inspect_cache
        .insert_test_item(
            &manifest_dir,
            RustItemMetadata {
                canonical_path: "demo::Envelope".to_string(),
                definition_path: Some("demo::Envelope".to_string()),
                visibility: RustVisibility::Public,
                kind: RustItemKind::Type(RustTypeInfo {
                    type_params: Vec::new(),
                    type_param_defaults: Vec::new(),
                    mutable_reference_type_params: Vec::new(),
                    expanded_derive_traits: Vec::new(),
                    has_const_params: false,
                    alias_target: None,
                    metadata_completeness: Default::default(),
                    methods: vec![],
                    implemented_traits: Vec::new(),
                    fields: vec![RustFieldInfo {
                        name: "kind".to_string(),
                        type_display: "Option<demo::Kind>".to_string(),
                        type_shape: RustTypeShape::Option(Box::new(RustTypeShape::RustPath {
                            path: "demo::Kind".to_string(),
                            args: vec![],
                        })),
                    }],
                    variants: vec![],
                }),
            },
        )
        .map_err(|e| std::io::Error::other(format!("seed rust-inspect envelope: {e}")))?;
    checker
        .rust_inspect_cache
        .insert_test_item(
            &manifest_dir,
            RustItemMetadata {
                canonical_path: "demo::Kind".to_string(),
                definition_path: Some("demo::Kind".to_string()),
                visibility: RustVisibility::Public,
                kind: RustItemKind::Type(RustTypeInfo {
                    type_params: Vec::new(),
                    type_param_defaults: Vec::new(),
                    mutable_reference_type_params: Vec::new(),
                    expanded_derive_traits: Vec::new(),
                    has_const_params: false,
                    alias_target: None,
                    metadata_completeness: Default::default(),
                    methods: vec![],
                    implemented_traits: Vec::new(),
                    fields: vec![],
                    variants: vec![],
                }),
            },
        )
        .map_err(|e| std::io::Error::other(format!("seed rust-inspect kind: {e}")))?;
    checker.check_program(&ast).map_err(|errs| {
        std::io::Error::other(format!(
            "expected rust field access + nested match binding to typecheck: {errs:?}"
        ))
    })?;
    Ok(())
}

#[cfg(feature = "rust_inspect")]
#[test]
fn test_rust_path_field_access_preserves_type_for_nested_match_binding() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
def id[T](x: T) -> T:
  return x

from rust::demo import Envelope
from rust::demo import Kind as KindPath

def f(x: Envelope) -> None:
  match x.kind:
    Some(KindPath.A(inner)) =>
      _ = id(inner)
    None =>
      _ = 0
"#;
    let tokens = lexer::lex(source).map_err(|errs| std::io::Error::other(format!("lex failed: {errs:?}")))?;
    let ast = parser::parse(&tokens).map_err(|errs| std::io::Error::other(format!("parse failed: {errs:?}")))?;
    let mut checker = TypeChecker::new();
    let tmp = seeded_rust_inspect_workspace()?;
    let manifest_dir = tmp.path().to_path_buf();
    checker.set_rust_inspect_manifest_dir(manifest_dir.clone());
    checker
        .rust_inspect_cache
        .insert_test_item(
            &manifest_dir,
            RustItemMetadata {
                canonical_path: "demo::Envelope".to_string(),
                definition_path: Some("demo::Envelope".to_string()),
                visibility: RustVisibility::Public,
                kind: RustItemKind::Type(RustTypeInfo {
                    type_params: Vec::new(),
                    type_param_defaults: Vec::new(),
                    mutable_reference_type_params: Vec::new(),
                    expanded_derive_traits: Vec::new(),
                    has_const_params: false,
                    alias_target: None,
                    metadata_completeness: Default::default(),
                    methods: vec![],
                    implemented_traits: Vec::new(),
                    fields: vec![RustFieldInfo {
                        name: "kind".to_string(),
                        type_display: "Option<demo::Kind>".to_string(),
                        type_shape: RustTypeShape::Option(Box::new(RustTypeShape::RustPath {
                            path: "demo::Kind".to_string(),
                            args: vec![],
                        })),
                    }],
                    variants: vec![],
                }),
            },
        )
        .map_err(|e| std::io::Error::other(format!("seed rust-inspect envelope: {e}")))?;
    checker
        .rust_inspect_cache
        .insert_test_item(
            &manifest_dir,
            RustItemMetadata {
                canonical_path: "demo::Kind".to_string(),
                definition_path: Some("demo::Kind".to_string()),
                visibility: RustVisibility::Public,
                kind: RustItemKind::Type(RustTypeInfo {
                    type_params: Vec::new(),
                    type_param_defaults: Vec::new(),
                    mutable_reference_type_params: Vec::new(),
                    expanded_derive_traits: Vec::new(),
                    has_const_params: false,
                    alias_target: None,
                    metadata_completeness: Default::default(),
                    methods: vec![],
                    implemented_traits: Vec::new(),
                    fields: vec![],
                    variants: vec![],
                }),
            },
        )
        .map_err(|e| std::io::Error::other(format!("seed rust-inspect kind: {e}")))?;
    checker.check_program(&ast).map_err(|errs| {
        std::io::Error::other(format!(
            "expected rust path field access + nested match binding to typecheck: {errs:?}"
        ))
    })?;
    Ok(())
}

#[cfg(feature = "rust_inspect")]
#[test]
fn test_imported_prost_oneof_field_match_uses_concrete_variant_payload_types() -> Result<(), Box<dyn std::error::Error>>
{
    let source = r#"
from rust::demo import Rel
from rust::demo::rel import RelType
from rust::demo::read_rel import ReadType

def inspect(rel: Rel) -> None:
  match rel.rel_type:
    Some(RelType.Read(read)) =>
      match read.read_type:
        Some(ReadType.NamedTable(_)) =>
          _ = 0
        _ =>
          _ = 1
    _ =>
      _ = 2
"#;
    let tokens = lexer::lex(source).map_err(|errs| std::io::Error::other(format!("lex failed: {errs:?}")))?;
    let ast = parser::parse(&tokens).map_err(|errs| std::io::Error::other(format!("parse failed: {errs:?}")))?;
    let mut checker = TypeChecker::new();
    let tmp = seeded_rust_inspect_workspace()?;
    let manifest_dir = tmp.path().to_path_buf();
    checker.set_rust_inspect_manifest_dir(manifest_dir.clone());
    checker
        .rust_inspect_cache
        .insert_test_item(
            &manifest_dir,
            RustItemMetadata {
                canonical_path: "demo::Rel".to_string(),
                definition_path: Some("demo::Rel".to_string()),
                visibility: RustVisibility::Public,
                kind: RustItemKind::Type(RustTypeInfo {
                    type_params: Vec::new(),
                    type_param_defaults: Vec::new(),
                    mutable_reference_type_params: Vec::new(),
                    expanded_derive_traits: Vec::new(),
                    has_const_params: false,
                    alias_target: None,
                    metadata_completeness: Default::default(),
                    methods: vec![],
                    implemented_traits: Vec::new(),
                    fields: vec![RustFieldInfo {
                        name: "rel_type".to_string(),
                        type_display: "Option<demo::rel::RelType>".to_string(),
                        type_shape: RustTypeShape::Option(Box::new(RustTypeShape::RustPath {
                            path: "demo::rel::RelType".to_string(),
                            args: vec![],
                        })),
                    }],
                    variants: vec![],
                }),
            },
        )
        .map_err(|e| std::io::Error::other(format!("seed rust-inspect rel: {e}")))?;
    checker
        .rust_inspect_cache
        .insert_test_item(
            &manifest_dir,
            RustItemMetadata {
                canonical_path: "demo::rel::RelType".to_string(),
                definition_path: Some("demo::rel::RelType".to_string()),
                visibility: RustVisibility::Public,
                kind: RustItemKind::Type(RustTypeInfo {
                    type_params: Vec::new(),
                    type_param_defaults: Vec::new(),
                    mutable_reference_type_params: Vec::new(),
                    expanded_derive_traits: Vec::new(),
                    has_const_params: false,
                    alias_target: None,
                    metadata_completeness: Default::default(),
                    methods: vec![],
                    implemented_traits: Vec::new(),
                    fields: vec![],
                    variants: vec![RustVariantInfo {
                        name: "Read".to_string(),
                        fields: vec![RustTypeShape::RustPath {
                            path: "demo::ReadRel".to_string(),
                            args: vec![],
                        }],
                        field_carriers: Vec::new(),
                    }],
                }),
            },
        )
        .map_err(|e| std::io::Error::other(format!("seed rust-inspect rel type: {e}")))?;
    checker
        .rust_inspect_cache
        .insert_test_item(
            &manifest_dir,
            RustItemMetadata {
                canonical_path: "demo::ReadRel".to_string(),
                definition_path: Some("demo::ReadRel".to_string()),
                visibility: RustVisibility::Public,
                kind: RustItemKind::Type(RustTypeInfo {
                    type_params: Vec::new(),
                    type_param_defaults: Vec::new(),
                    mutable_reference_type_params: Vec::new(),
                    expanded_derive_traits: Vec::new(),
                    has_const_params: false,
                    alias_target: None,
                    metadata_completeness: Default::default(),
                    methods: vec![],
                    implemented_traits: Vec::new(),
                    fields: vec![RustFieldInfo {
                        name: "read_type".to_string(),
                        type_display: "Option<demo::read_rel::ReadType>".to_string(),
                        type_shape: RustTypeShape::Option(Box::new(RustTypeShape::RustPath {
                            path: "demo::read_rel::ReadType".to_string(),
                            args: vec![],
                        })),
                    }],
                    variants: vec![],
                }),
            },
        )
        .map_err(|e| std::io::Error::other(format!("seed rust-inspect read rel: {e}")))?;
    checker
        .rust_inspect_cache
        .insert_test_item(
            &manifest_dir,
            RustItemMetadata {
                canonical_path: "demo::read_rel::ReadType".to_string(),
                definition_path: Some("demo::read_rel::ReadType".to_string()),
                visibility: RustVisibility::Public,
                kind: RustItemKind::Type(RustTypeInfo {
                    type_params: Vec::new(),
                    type_param_defaults: Vec::new(),
                    mutable_reference_type_params: Vec::new(),
                    expanded_derive_traits: Vec::new(),
                    has_const_params: false,
                    alias_target: None,
                    metadata_completeness: Default::default(),
                    methods: vec![],
                    implemented_traits: Vec::new(),
                    fields: vec![],
                    variants: vec![RustVariantInfo {
                        name: "NamedTable".to_string(),
                        fields: vec![RustTypeShape::RustPath {
                            path: "demo::NamedTable".to_string(),
                            args: vec![],
                        }],
                        field_carriers: Vec::new(),
                    }],
                }),
            },
        )
        .map_err(|e| std::io::Error::other(format!("seed rust-inspect read type: {e}")))?;
    checker.check_program(&ast).map_err(|errs| {
        std::io::Error::other(format!(
            "expected imported prost oneof field match to typecheck: {errs:?}"
        ))
    })?;
    Ok(())
}

#[cfg(feature = "rust_inspect")]
#[test]
fn test_real_rust_inspect_allows_imported_prost_oneof_field_match() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    write_substrait_probe_crate(tmp.path())?;
    let source = r#"
from rust::substrait::proto import Rel
from rust::substrait::proto::rel import RelType
from rust::substrait::proto::read_rel import ReadType

def inspect(rel: Rel) -> None:
  match rel.rel_type:
    Some(RelType.Read(read)) =>
      match read.read_type:
        Some(ReadType.NamedTable(_)) =>
          _ = 0
        _ =>
          _ = 1
    _ =>
      _ = 2
"#;
    let tokens = lexer::lex(source).map_err(|errs| std::io::Error::other(format!("lex failed: {errs:?}")))?;
    let ast = parser::parse(&tokens).map_err(|errs| std::io::Error::other(format!("parse failed: {errs:?}")))?;
    let mut checker = TypeChecker::new();
    checker.set_rust_inspect_manifest_dir(tmp.path().to_path_buf());
    checker.check_program(&ast).map_err(|errs| {
        std::io::Error::other(format!(
            "expected extracted prost oneof field metadata to typecheck end-to-end: {errs:?}"
        ))
    })?;
    Ok(())
}

#[cfg(feature = "rust_inspect")]
#[test]
fn test_real_rust_inspect_preserves_concrete_borrowed_param_pointees() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    write_borrowed_param_probe_crate(tmp.path())?;
    let inspector = Inspector::new(InspectorConfig::new(tmp.path().to_path_buf()));
    let query = "ra_borrowed_param_probe::logical_plan::consumer::consume".to_string();
    inspector.prewarm([query.clone()], &|_| {})?;
    let hit = inspector.get(query.as_str())?;
    let RustItemKind::Function(sig) = &hit.metadata.kind else {
        return Err(std::io::Error::other("expected function metadata from borrowed-param probe").into());
    };
    let displays: Vec<&str> = sig.params.iter().map(|param| param.type_display.as_str()).collect();
    assert_eq!(
        displays,
        vec![
            "&ra_borrowed_param_probe::execution::session_state::SessionState",
            "&substrait::proto::Plan",
        ],
        "borrowed rust-inspect params must preserve concrete pointees"
    );
    assert!(
        sig.is_async,
        "expected async metadata for borrowed-param probe function"
    );
    Ok(())
}
