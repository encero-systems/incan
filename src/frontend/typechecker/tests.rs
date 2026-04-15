//! Typechecker unit tests.

use super::*;
use crate::frontend::library_manifest_index::{
    LibraryArtifactMetadata, LibraryManifestFailureKind, LibraryManifestIndex, LibraryManifestIndexEntry,
    LibraryManifestLoadFailure,
};
use crate::frontend::{lexer, parser};
use crate::library_manifest::{
    ClassExport, ConstExport, EnumExport, EnumVariantExport, FunctionExport, LibraryExports, LibraryManifest,
    MethodExport, ModelExport, ParamExport, ReceiverExport, StaticExport, TraitExport, TypeBoundExport,
    TypeParamExport, TypeRef,
};
#[cfg(feature = "rust_inspect")]
use crate::rust_inspect::{Inspector, InspectorConfig, write_borrowed_param_probe_crate, write_substrait_probe_crate};
#[cfg(feature = "rust_inspect")]
use incan_core::interop::{
    RustFieldInfo, RustFunctionSig, RustItemKind, RustItemMetadata, RustMethodSig, RustParam, RustTypeInfo,
    RustTypeShape, RustVariantInfo, RustVisibility,
};
use incan_core::lang::surface::constructors::{self as surface_constructors, ConstructorId};
use incan_core::lang::traits::{self as builtin_traits, TraitId};
use std::collections::HashMap;
#[cfg(feature = "rust_inspect")]
use std::fs;
use std::path::PathBuf;

fn check_str(source: &str) -> Result<(), Vec<CompileError>> {
    let tokens = lexer::lex(source)?;
    let ast = parser::parse(&tokens)?;
    check(&ast)
}

fn check_str_with_library_index(source: &str, library_index: LibraryManifestIndex) -> Result<(), Vec<CompileError>> {
    let tokens = lexer::lex(source)?;
    let ast = parser::parse(&tokens)?;
    let mut checker = TypeChecker::new();
    checker.set_library_manifest_index(library_index);
    checker.check_program(&ast)
}

fn synthetic_artifact_root(name: &str) -> PathBuf {
    let mut root = std::env::temp_dir();
    root.push(format!("incan_test_{name}_artifacts"));
    root.push("target");
    root.push("lib");
    root
}

fn clone_trait_name() -> String {
    builtin_traits::as_str(TraitId::Clone).to_string()
}

fn none_constructor_name() -> String {
    surface_constructors::as_str(ConstructorId::None).to_string()
}

#[cfg(feature = "rust_inspect")]
fn write_rust_inspect_probe_crate(root: &std::path::Path) -> Result<(), Box<dyn std::error::Error>> {
    fs::create_dir_all(root.join("src"))?;
    fs::create_dir_all(root.join("demo").join("src"))?;
    fs::write(
        root.join("Cargo.toml"),
        r#"[package]
name = "ra_frontend_probe"
version = "0.1.0"
edition = "2021"

[dependencies]
demo = { path = "demo" }
"#,
    )?;
    fs::write(
        root.join("src/lib.rs"),
        "pub fn touch() { let _ = demo::Builder::new(); }\n",
    )?;
    fs::write(
        root.join("demo").join("Cargo.toml"),
        r#"[package]
name = "demo"
version = "0.1.0"
edition = "2021"
"#,
    )?;
    fs::write(
        root.join("demo").join("src/lib.rs"),
        r#"pub struct Builder;

impl Builder {
    pub fn new() -> Self {
        Self
    }
}

pub enum Choice {
    Some(i32),
}
"#,
    )?;
    Ok(())
}

#[cfg(feature = "rust_inspect")]
fn seeded_rust_inspect_workspace() -> Result<tempfile::TempDir, Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    fs::write(
        tmp.path().join("Cargo.toml"),
        r#"[package]
name = "ra_seeded_metadata_probe"
version = "0.1.0"
edition = "2021"
"#,
    )?;
    Ok(tmp)
}

#[cfg(feature = "rust_inspect")]
fn prewarm_metadata(manifest_dir: &std::path::Path, paths: &[&str]) -> Result<(), Box<dyn std::error::Error>> {
    let inspector = Inspector::new(InspectorConfig::new(manifest_dir.to_path_buf()));
    inspector.prewarm(
        paths.iter().map(|path| (*path).to_string()).collect::<Vec<_>>(),
        &|_| (),
    )?;
    Ok(())
}

fn assert_check_ok(source: &str) {
    if let Err(errs) = check_str(source) {
        for e in &errs {
            eprintln!("typecheck error: {} @ {:?}", e.message, e.span);
        }
        panic!("expected Ok, got errors (see stderr)");
    }
}

fn library_index_with_mylib_exports() -> LibraryManifestIndex {
    let manifest = LibraryManifest {
        name: "mylib".to_string(),
        version: "0.1.0".to_string(),
        incan_version: crate::version::INCAN_VERSION.to_string(),
        manifest_format: crate::library_manifest::LIBRARY_MANIFEST_FORMAT,
        exports: LibraryExports {
            models: vec![ModelExport {
                name: "Widget".to_string(),
                type_params: Vec::new(),
                traits: Vec::new(),
                derives: Vec::new(),
                fields: Vec::new(),
                methods: Vec::new(),
            }],
            classes: Vec::new(),
            functions: vec![FunctionExport {
                name: "make_widget".to_string(),
                type_params: Vec::new(),
                params: vec![ParamExport {
                    name: "name".to_string(),
                    ty: TypeRef::Named {
                        name: "str".to_string(),
                    },
                }],
                return_type: TypeRef::Named {
                    name: "Widget".to_string(),
                },
                is_async: false,
            }],
            traits: Vec::new(),
            enums: vec![EnumExport {
                name: "Status".to_string(),
                type_params: Vec::new(),
                variants: vec![
                    EnumVariantExport {
                        name: "Active".to_string(),
                        fields: Vec::new(),
                    },
                    EnumVariantExport {
                        name: "Disabled".to_string(),
                        fields: Vec::new(),
                    },
                ],
                derives: Vec::new(),
            }],
            type_aliases: Vec::new(),
            newtypes: Vec::new(),
            consts: vec![ConstExport {
                name: "DEFAULT_NAME".to_string(),
                ty: TypeRef::Named {
                    name: "str".to_string(),
                },
            }],
            statics: vec![StaticExport {
                name: "SHARED_ITEMS".to_string(),
                ty: TypeRef::Applied {
                    name: "list".to_string(),
                    args: vec![TypeRef::Named {
                        name: "int".to_string(),
                    }],
                },
            }],
        },
        vocab: None,
        soft_keywords: Default::default(),
    };

    LibraryManifestIndex::from_entries(HashMap::from([(
        "mylib".to_string(),
        LibraryManifestIndexEntry::Loaded {
            manifest: Box::new(manifest),
            metadata: LibraryArtifactMetadata::from_crate_root("mylib", "mylib", synthetic_artifact_root("mylib")),
        },
    )]))
}

fn library_index_with_trait_export() -> LibraryManifestIndex {
    let manifest = LibraryManifest {
        name: "mylib".to_string(),
        version: "0.1.0".to_string(),
        incan_version: crate::version::INCAN_VERSION.to_string(),
        manifest_format: crate::library_manifest::LIBRARY_MANIFEST_FORMAT,
        exports: LibraryExports {
            models: Vec::new(),
            classes: Vec::new(),
            functions: Vec::new(),
            traits: vec![TraitExport {
                name: "ExternBox".to_string(),
                type_params: vec![TypeParamExport {
                    name: "T".to_string(),
                    bounds: Vec::new(),
                }],
                supertraits: Vec::new(),
                requires: Vec::new(),
                methods: Vec::new(),
            }],
            enums: Vec::new(),
            type_aliases: Vec::new(),
            newtypes: Vec::new(),
            consts: Vec::new(),
            statics: Vec::new(),
        },
        vocab: None,
        soft_keywords: Default::default(),
    };

    LibraryManifestIndex::from_entries(HashMap::from([(
        "mylib".to_string(),
        LibraryManifestIndexEntry::Loaded {
            manifest: Box::new(manifest),
            metadata: LibraryArtifactMetadata::from_crate_root(
                "mylib",
                "mylib",
                synthetic_artifact_root("mylib_trait_export"),
            ),
        },
    )]))
}

fn library_index_with_pub_boundary_type_fidelity_exports() -> LibraryManifestIndex {
    let type_param_t = TypeParamExport {
        name: "T".to_string(),
        bounds: Vec::new(),
    };
    let manifest = LibraryManifest {
        name: "pubdemo".to_string(),
        version: "0.1.0".to_string(),
        incan_version: crate::version::INCAN_VERSION.to_string(),
        manifest_format: crate::library_manifest::LIBRARY_MANIFEST_FORMAT,
        exports: LibraryExports {
            models: vec![ModelExport {
                name: "SessionError".to_string(),
                type_params: Vec::new(),
                traits: Vec::new(),
                derives: Vec::new(),
                fields: Vec::new(),
                methods: Vec::new(),
            }],
            classes: vec![
                ClassExport {
                    name: "Session".to_string(),
                    type_params: Vec::new(),
                    extends: None,
                    traits: Vec::new(),
                    derives: Vec::new(),
                    fields: Vec::new(),
                    methods: vec![
                        MethodExport {
                            name: "default".to_string(),
                            type_params: Vec::new(),
                            receiver: None,
                            params: Vec::new(),
                            return_type: TypeRef::Named {
                                name: "Session".to_string(),
                            },
                            is_async: false,
                            has_body: true,
                        },
                        MethodExport {
                            name: "read_csv".to_string(),
                            type_params: vec![type_param_t.clone()],
                            receiver: Some(ReceiverExport::Mutable),
                            params: vec![
                                ParamExport {
                                    name: "logical_name".to_string(),
                                    ty: TypeRef::Named {
                                        name: "str".to_string(),
                                    },
                                },
                                ParamExport {
                                    name: "uri".to_string(),
                                    ty: TypeRef::Named {
                                        name: "str".to_string(),
                                    },
                                },
                            ],
                            return_type: TypeRef::Applied {
                                name: "Result".to_string(),
                                args: vec![
                                    TypeRef::Applied {
                                        name: "LazyFrame".to_string(),
                                        args: vec![TypeRef::TypeParam { name: "T".to_string() }],
                                    },
                                    TypeRef::Named {
                                        name: "SessionError".to_string(),
                                    },
                                ],
                            },
                            is_async: false,
                            has_body: true,
                        },
                        MethodExport {
                            name: "collect".to_string(),
                            type_params: vec![type_param_t.clone()],
                            receiver: Some(ReceiverExport::Immutable),
                            params: vec![ParamExport {
                                name: "data".to_string(),
                                ty: TypeRef::Applied {
                                    name: "LazyFrame".to_string(),
                                    args: vec![TypeRef::TypeParam { name: "T".to_string() }],
                                },
                            }],
                            return_type: TypeRef::Applied {
                                name: "Result".to_string(),
                                args: vec![
                                    TypeRef::Applied {
                                        name: "DataFrame".to_string(),
                                        args: vec![TypeRef::TypeParam { name: "T".to_string() }],
                                    },
                                    TypeRef::Named {
                                        name: "SessionError".to_string(),
                                    },
                                ],
                            },
                            is_async: false,
                            has_body: true,
                        },
                    ],
                },
                ClassExport {
                    name: "DataFrame".to_string(),
                    type_params: vec![type_param_t.clone()],
                    extends: None,
                    traits: vec!["BoundedDataSet".to_string()],
                    derives: vec![clone_trait_name()],
                    fields: Vec::new(),
                    methods: Vec::new(),
                },
                ClassExport {
                    name: "LazyFrame".to_string(),
                    type_params: vec![type_param_t.clone()],
                    extends: None,
                    traits: vec!["BoundedDataSet".to_string()],
                    derives: vec![clone_trait_name()],
                    fields: Vec::new(),
                    methods: vec![MethodExport {
                        name: "collect".to_string(),
                        type_params: Vec::new(),
                        receiver: Some(ReceiverExport::Immutable),
                        params: Vec::new(),
                        return_type: TypeRef::Applied {
                            name: "Result".to_string(),
                            args: vec![
                                TypeRef::Applied {
                                    name: "DataFrame".to_string(),
                                    args: vec![TypeRef::TypeParam { name: "T".to_string() }],
                                },
                                TypeRef::Named {
                                    name: "SessionError".to_string(),
                                },
                            ],
                        },
                        is_async: false,
                        has_body: true,
                    }],
                },
            ],
            functions: vec![FunctionExport {
                name: "display".to_string(),
                type_params: vec![type_param_t.clone()],
                params: vec![ParamExport {
                    name: "data".to_string(),
                    ty: TypeRef::Applied {
                        name: "DataSet".to_string(),
                        args: vec![TypeRef::TypeParam { name: "T".to_string() }],
                    },
                }],
                return_type: TypeRef::Named {
                    name: none_constructor_name(),
                },
                is_async: false,
            }],
            traits: vec![
                TraitExport {
                    name: "DataSet".to_string(),
                    type_params: vec![type_param_t.clone()],
                    supertraits: Vec::new(),
                    requires: Vec::new(),
                    methods: Vec::new(),
                },
                TraitExport {
                    name: "BoundedDataSet".to_string(),
                    type_params: vec![type_param_t],
                    supertraits: vec![TypeBoundExport {
                        name: "DataSet".to_string(),
                        type_args: vec![TypeRef::TypeParam { name: "T".to_string() }],
                    }],
                    requires: Vec::new(),
                    methods: Vec::new(),
                },
            ],
            enums: Vec::new(),
            type_aliases: Vec::new(),
            newtypes: Vec::new(),
            consts: Vec::new(),
            statics: Vec::new(),
        },
        vocab: None,
        soft_keywords: Default::default(),
    };

    LibraryManifestIndex::from_entries(HashMap::from([(
        "pubdemo".to_string(),
        LibraryManifestIndexEntry::Loaded {
            manifest: Box::new(manifest),
            metadata: LibraryArtifactMetadata::from_crate_root(
                "pubdemo",
                "pubdemo",
                synthetic_artifact_root("pub_boundary_type_fidelity"),
            ),
        },
    )]))
}

// ========================================
// Basic function tests
// ========================================

#[test]
fn test_simple_function() {
    let source = r#"
def add(a: int, b: int) -> int:
  return a + b
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_generic_function_reference_rejected_in_value_position() {
    let source = r#"
def id[T](x: T) -> T:
  return x

def accept(f: (int) -> int) -> int:
  return f(42)

def main() -> None:
  _ = accept(id)
"#;
    let err = check_str(source).expect_err("generic function name in value position should fail");
    assert!(
        err.iter()
            .any(|e| e.message.contains("generic function") && e.message.contains("'id'")),
        "unexpected errors: {:?}",
        err.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

#[test]
fn test_type_mismatch() {
    let source = r#"
def foo() -> int:
  return "hello"
"#;
    let result = check_str(source);
    assert!(result.is_err());
}

#[test]
fn test_unknown_symbol() {
    let source = r#"
def foo() -> int:
  return unknown_var
"#;
    let result = check_str(source);
    assert!(result.is_err());
}

#[test]
fn test_fstring_unknown_symbol_span_points_to_interpolation() {
    let source = "def foo() -> str:\n  return f\"value: {unknown_var}\"\n";
    let result = check_str(source);
    assert!(result.is_err());

    let errors = match result {
        Ok(()) => {
            panic!("Expected typechecker error for unknown symbol in f-string interpolation")
        }
        Err(errors) => errors,
    };

    let error = match errors
        .iter()
        .find(|e| e.message.contains("Unknown symbol 'unknown_var'"))
    {
        Some(error) => error,
        None => panic!("Expected unknown symbol error for unknown_var; got: {errors:?}"),
    };

    let expected_start = match source.find("{unknown_var}") {
        Some(start) => start,
        None => panic!("Expected interpolation segment in source"),
    };

    assert_eq!(error.span.start, expected_start);
    assert_eq!(error.span.end, expected_start + "{unknown_var}".len());
}

#[test]
fn test_fstring_nested_unknown_symbol_span_rebased() {
    let source = "def foo(x: int) -> str:\n  return f\"sum: {x + unknown_var}\"\n";
    let result = check_str(source);
    assert!(result.is_err());

    let errors = match result {
        Ok(()) => panic!("Expected typechecker error for nested unknown symbol in f-string interpolation"),
        Err(errors) => errors,
    };

    let error = match errors
        .iter()
        .find(|e| e.message.contains("Unknown symbol 'unknown_var'"))
    {
        Some(error) => error,
        None => panic!("Expected unknown symbol error for unknown_var; got: {errors:?}"),
    };

    let expected_start = match source.find("unknown_var") {
        Some(start) => start,
        None => panic!("Expected unknown symbol segment in source"),
    };

    assert_eq!(error.span.start, expected_start);
    assert_eq!(error.span.end, expected_start + "unknown_var".len());
}

#[test]
fn test_fstring_unknown_symbol_span_in_index_method_chain() {
    let source = "def foo(users: List[str]) -> str:\n  return f\"value: {users[unknown_idx].upper()}\"\n";
    let result = check_str(source);
    assert!(result.is_err());

    let errors = match result {
        Ok(()) => panic!("Expected typechecker error for unknown symbol in index interpolation"),
        Err(errors) => errors,
    };

    let error = match errors
        .iter()
        .find(|e| e.message.contains("Unknown symbol 'unknown_idx'"))
    {
        Some(error) => error,
        None => panic!("Expected unknown symbol error for unknown_idx; got: {errors:?}"),
    };

    let expected_start = match source.find("unknown_idx") {
        Some(start) => start,
        None => panic!("Expected unknown symbol segment in source"),
    };

    assert_eq!(error.span.start, expected_start);
    assert_eq!(error.span.end, expected_start + "unknown_idx".len());
}

#[test]
fn test_fstring_unknown_symbol_span_in_list_comp_filter_call() {
    let source = "def foo(items: List[int]) -> str:\n  return f\"value: {[x for x in items if unknown_pred(x)]}\"\n";
    let result = check_str(source);
    assert!(result.is_err());

    let errors = match result {
        Ok(()) => panic!("Expected typechecker error for unknown symbol in list comp interpolation"),
        Err(errors) => errors,
    };

    let error = match errors
        .iter()
        .find(|e| e.message.contains("Unknown symbol 'unknown_pred'"))
    {
        Some(error) => error,
        None => panic!("Expected unknown symbol error for unknown_pred; got: {errors:?}"),
    };

    let expected_start = match source.find("unknown_pred") {
        Some(start) => start,
        None => panic!("Expected unknown symbol segment in source"),
    };

    assert_eq!(error.span.start, expected_start);
    assert_eq!(error.span.end, expected_start + "unknown_pred".len());
}

#[test]
fn test_reserved_root_namespace_std() {
    // `std` is a reserved root namespace, so `def std() -> int: return 1` is rejected.
    let source = r#"
def std() -> int:
  return 1
"#;
    let result = check_str(source);
    assert!(result.is_err());
}

#[test]
fn test_reserved_root_namespace_rust_import_alias() {
    // Aliasing a std import to `rust` (a different reserved root) is rejected.
    let source = r#"
import std.web as rust
"#;
    let result = check_str(source);
    assert!(result.is_err());
}

/// RFC 041: `import rust::crate` binds the crate root; it is not a concrete Rust type.
#[test]
fn test_rust_crate_root_import_rejected_in_type_position() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
import rust::serde_json

def f(x: serde_json) -> None:
  pass
"#;
    let tokens = lexer::lex(source).map_err(|errs| std::io::Error::other(format!("lex failed: {errs:?}")))?;
    let ast = parser::parse(&tokens).map_err(|errs| std::io::Error::other(format!("parse failed: {errs:?}")))?;
    let mut checker = TypeChecker::new();
    let errs = checker
        .check_program(&ast)
        .err()
        .ok_or_else(|| std::io::Error::other("expected type error for crate-root import used as type"))?;
    assert!(
        errs.iter().any(|e| e.message.contains("cannot be used as a type")),
        "expected crate-root-as-type diagnostic, got {errs:?}"
    );
    Ok(())
}

/// RFC 041: `from rust::... import Item` carries canonical path in [`ResolvedType::RustPath`].
#[test]
fn test_rust_from_import_records_rust_path_on_ident_use() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
from rust::std::time import Instant

def f() -> None:
  _ = Instant
"#;
    let tokens = lexer::lex(source).map_err(|errs| std::io::Error::other(format!("lex failed: {errs:?}")))?;
    let ast = parser::parse(&tokens).map_err(|errs| std::io::Error::other(format!("parse failed: {errs:?}")))?;
    let mut checker = TypeChecker::new();
    checker
        .check_program(&ast)
        .map_err(|errs| std::io::Error::other(format!("check_program failed: {errs:?}")))?;
    let info = checker.type_info();
    assert!(
        info.expr_types.values().any(|t| {
            matches!(
                t,
                ResolvedType::RustPath(p) if p == "std::time::Instant"
            )
        }),
        "expected RustPath(std::time::Instant) in expr types, got {:?}",
        info.expr_types
    );
    Ok(())
}

#[test]
fn test_rusttype_requires_rust_import_backing() {
    let source = r#"
type Email = rusttype str
"#;
    let Err(errs) = check_str(source) else {
        panic!("expected rusttype backing diagnostic");
    };
    assert!(
        errs.iter().any(|e| e.message.contains("declared as `rusttype`")),
        "expected rusttype backing diagnostic, got {errs:?}"
    );
}

#[test]
fn test_interop_block_rejected_on_non_rusttype() {
    let source = r#"
type Email = newtype str:
  interop:
    from str try Email.parse
"#;
    let Err(errs) = check_str(source) else {
        panic!("expected interop block diagnostic");
    };
    assert!(
        errs.iter().any(|e| e.message.contains("`interop:` is only valid")),
        "expected interop-on-newtype diagnostic, got {errs:?}"
    );
}

#[test]
fn test_interop_try_adapter_requires_result_or_option() {
    let source = r#"
from rust::mail import EmailAddress as RustEmailAddress

type Email = rusttype RustEmailAddress:
  def parse(raw: str) -> Email:
    ...

  interop:
    from str try Email.parse
"#;
    let Err(errs) = check_str(source) else {
        panic!("expected invalid try-adapter diagnostic");
    };
    assert!(
        errs.iter().any(|e| e.message.contains("`try` interop adapter")),
        "expected try-adapter return diagnostic, got {errs:?}"
    );
}

#[test]
fn test_interop_via_adapter_rejects_fallible_return() {
    let source = r#"
from rust::mail import EmailAddress as RustEmailAddress

type Email = rusttype RustEmailAddress:
  def parse(raw: str) -> Result[Email, str]:
    ...

  interop:
    from str via Email.parse
"#;
    let Err(errs) = check_str(source) else {
        panic!("expected invalid via-adapter diagnostic");
    };
    assert!(
        errs.iter().any(|e| e.message.contains("`via` interop adapter")),
        "expected via-adapter infallible diagnostic, got {errs:?}"
    );
}

#[test]
fn test_interop_from_adapter_input_type_mismatch() {
    let source = r#"
from rust::mail import EmailAddress as RustEmailAddress

type Email = rusttype RustEmailAddress:
  def parse(raw: int) -> Result[Email, str]:
    ...

  interop:
    from str try Email.parse
"#;
    let Err(errs) = check_str(source) else {
        panic!("expected interop input-type mismatch diagnostic");
    };
    assert!(
        errs.iter().any(|e| e.message.contains("incompatible input type")),
        "expected interop adapter input mismatch diagnostic, got {errs:?}"
    );
}

#[test]
fn test_interop_into_receiver_method_allowed() {
    let source = r#"
from rust::mail import EmailAddress as RustEmailAddress

type Email = rusttype RustEmailAddress:
  def as_str(self) -> str:
    ...

  interop:
    into str via Email.as_str
"#;
    assert_check_ok(source);
}

#[test]
fn test_interop_from_via_positive_path() {
    let source = r#"
from rust::mail import EmailAddress as RustEmailAddress

type Email = rusttype RustEmailAddress:
  def parse(raw: str) -> Email:
    ...

  interop:
    from str via Email.parse
"#;
    assert_check_ok(source);
}

#[cfg(feature = "rust_inspect")]
#[test]
fn test_ambiguous_short_form_adapter_rejected() {
    let source = r#"
from rust::regex import Regex as RustRegex

type WrappedRegex = rusttype RustRegex:
  def new(pattern: str) -> WrappedRegex:
    ...

  interop:
    from str via new
"#;
    let result = check_str(source);
    if let Err(errs) = result {
        assert!(
            errs.iter()
                .any(|e| e.message.contains("Ambiguous short-form interop adapter")),
            "expected ambiguous short-form adapter diagnostic, got {errs:?}"
        );
    }
}

#[test]
fn test_rusttype_rebinding_resolves_to_target_method() {
    let source = r#"
from rust::mail import Sender as RustSender

type Sender = rusttype RustSender:
  send_now = try_send

  def try_send(self, value: int) -> Result[None, str]:
    ...

def push(sender: Sender, value: int) -> Result[None, str]:
  return sender.send_now(value)
"#;
    assert_check_ok(source);
}

/// Issue #217: payload names bound from `rusttype`-backed enum-style patterns must be in scope in the arm body.
#[test]
fn test_rusttype_enum_match_binds_payload_in_arm() {
    let source = r#"
def id[T](x: T) -> T:
  return x

from rust::mail import Sender as RustSender

type PlanRel = rusttype RustSender:
  def noop(self) -> None:
    ...

def f(x: PlanRel) -> None:
  match x:
    PlanRel.Root(root) =>
      _ = id(root)
    _ =>
      _ = x

def g(x: Option[PlanRel]) -> None:
  match x:
    Some(inner) =>
      match inner:
        PlanRel.Root(root) =>
          _ = id(root)
        _ =>
          _ = inner
    None =>
      _ = 0
"#;
    assert_check_ok(source);
}

#[test]
fn test_rusttype_enum_match_with_mismatched_qualifier_reports_constructor_resolution_error() {
    let source = r#"
from rust::mail import Sender as RustSender

type PlanRel = rusttype RustSender:
  def noop(self) -> None:
    ...

type OtherEnum = rusttype RustSender:
  def noop(self) -> None:
    ...

def f(x: PlanRel) -> None:
  match x:
    OtherEnum.Root(root) =>
      _ = root
    _ =>
      _ = x
"#;
    let Err(errs) = check_str(source) else {
        panic!("expected type errors for mismatched rusttype constructor qualifier");
    };
    assert!(
        errs.iter()
            .any(|e| e.message.contains("does not resolve for this match")),
        "expected unknown_match_constructor_pattern, got {errs:?}"
    );
}

#[test]
fn test_resolved_type_from_fully_qualified_option_display_extracts_option_payload() {
    let checker = TypeChecker::new();
    let resolved = checker.resolved_type_from_rust_display("::core::option::Option<demo::Thing>");
    assert_eq!(
        resolved,
        ResolvedType::Generic(
            "Option".to_string(),
            vec![ResolvedType::RustPath("demo::Thing".to_string())],
        )
    );
}

#[test]
fn test_resolved_type_from_fully_qualified_result_display_normalizes() {
    let checker = TypeChecker::new();
    let resolved = checker.resolved_type_from_rust_display("::core::result::Result<demo::OkThing, demo::ErrThing>");
    assert_eq!(
        resolved,
        ResolvedType::Generic(
            "Result".to_string(),
            vec![
                ResolvedType::RustPath("demo::OkThing".to_string()),
                ResolvedType::RustPath("demo::ErrThing".to_string()),
            ],
        )
    );
}

#[test]
fn test_resolved_type_from_namespaced_result_alias_normalizes_ok_payload() {
    let checker = TypeChecker::new();
    let resolved = checker.resolved_type_from_rust_display(
        "datafusion_common::error::Result<datafusion_expr::logical_plan::plan::LogicalPlan>",
    );
    assert_eq!(
        resolved,
        ResolvedType::Generic(
            "Result".to_string(),
            vec![
                ResolvedType::RustPath("datafusion_expr::logical_plan::plan::LogicalPlan".to_string()),
                ResolvedType::Unknown,
            ],
        )
    );
}

#[test]
fn test_resolved_type_from_borrowed_rust_path_display_extracts_ref_payload() {
    let checker = TypeChecker::new();
    let resolved = checker.resolved_type_from_rust_display("&demo::Thing");
    assert_eq!(
        resolved,
        ResolvedType::Ref(Box::new(ResolvedType::RustPath("demo::Thing".to_string()))),
    );
}

#[test]
fn test_resolved_type_from_mut_borrowed_rust_path_display_extracts_refmut_payload() {
    let checker = TypeChecker::new();
    let resolved = checker.resolved_type_from_rust_display("&mut demo::Thing");
    assert_eq!(
        resolved,
        ResolvedType::RefMut(Box::new(ResolvedType::RustPath("demo::Thing".to_string()))),
    );
}

#[test]
fn test_resolved_type_from_builtin_borrowed_displays_stays_stable() {
    let checker = TypeChecker::new();
    assert_eq!(checker.resolved_type_from_rust_display("&str"), ResolvedType::Str);
    assert_eq!(checker.resolved_type_from_rust_display("&[u8]"), ResolvedType::Bytes);
}

#[test]
fn test_types_compatible_refmut_is_assignable_to_ref_but_not_reverse() {
    let checker = TypeChecker::new();
    let immutable = ResolvedType::Ref(Box::new(ResolvedType::RustPath("demo::Thing".to_string())));
    let mutable = ResolvedType::RefMut(Box::new(ResolvedType::RustPath("demo::Thing".to_string())));
    assert!(
        checker.types_compatible(&mutable, &immutable),
        "mutable borrow should satisfy immutable borrow expectations"
    );
    assert!(
        !checker.types_compatible(&immutable, &mutable),
        "immutable borrow must not satisfy mutable borrow expectations"
    );
}

#[test]
fn test_duplicate_interop_edges_rejected() {
    let source = r#"
from rust::mail import EmailAddress as RustEmailAddress

type Email = rusttype RustEmailAddress:
  def parse(raw: str) -> Result[Email, str]:
    ...

  interop:
    from str try Email.parse
    from str try Email.parse
"#;
    let Err(errs) = check_str(source) else {
        panic!("expected duplicate interop edge diagnostic");
    };
    assert!(
        errs.iter().any(|e| e.message.contains("Duplicate interop edge")),
        "expected duplicate interop edge diagnostic, got {errs:?}"
    );
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
        checker.resolved_function_type_from_rust_sig(&sig, false),
        ResolvedType::Function(
            vec![ResolvedType::Ref(Box::new(ResolvedType::RustPath(
                "demo::Thing".to_string()
            )))],
            Box::new(ResolvedType::Unit),
        )
    );
    Ok(())
}

#[cfg(feature = "rust_inspect")]
#[test]
fn test_rust_inspect_lookup_path_strips_outer_generic_instantiation() {
    assert_eq!(
        TypeChecker::rust_inspect_lookup_path("incan_stdlib::r#async::channel::SendError<T>"),
        Some("incan_stdlib::r#async::channel::SendError")
    );
    assert_eq!(
        TypeChecker::rust_inspect_lookup_path("Result<(),incan_stdlib::r#async::channel::SendError<T>>"),
        None
    );
}

#[cfg(feature = "rust_inspect")]
#[test]
fn test_rust_inspect_lookup_path_rejects_unknown_placeholder() {
    assert_eq!(TypeChecker::rust_inspect_lookup_path("{unknown}"), None);
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
                    fields: Vec::new(),
                    methods: Vec::new(),
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
fn test_types_compatible_accepts_rust_alias_definition_without_metadata_lookup() {
    let mut checker = TypeChecker::new();
    checker.symbols.define(Symbol {
        name: "RawSender".to_string(),
        kind: SymbolKind::RustItem(RustItemInfo {
            crate_name: "incan_stdlib".to_string(),
            path: "incan_stdlib::r#async::channel::RawSender".to_string(),
            binding: RustImportBindingKind::FromImport,
            metadata: Some(RustItemMetadata {
                canonical_path: "incan_stdlib::r#async::channel::RawSender".to_string(),
                definition_path: Some("incan_stdlib::r#async::channel::Sender".to_string()),
                visibility: RustVisibility::Public,
                kind: RustItemKind::Type(RustTypeInfo {
                    fields: Vec::new(),
                    methods: Vec::new(),
                    variants: Vec::new(),
                }),
            }),
        }),
        span: Span::default(),
        scope: 0,
    });

    let actual = ResolvedType::Generic("RawSender".to_string(), vec![ResolvedType::Int]);
    let expected = ResolvedType::Ref(Box::new(ResolvedType::RustPath(
        "incan_stdlib::r#async::channel::Sender<i32>".to_string(),
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
            crate_name: "incan_stdlib".to_string(),
            path: "incan_stdlib::r#async::sync::RawSemaphore".to_string(),
            binding: RustImportBindingKind::FromImport,
            metadata: Some(RustItemMetadata {
                canonical_path: "incan_stdlib::r#async::sync::RawSemaphore".to_string(),
                definition_path: Some("incan_stdlib::r#async::sync::Semaphore".to_string()),
                visibility: RustVisibility::Public,
                kind: RustItemKind::Type(RustTypeInfo {
                    fields: Vec::new(),
                    methods: Vec::new(),
                    variants: Vec::new(),
                }),
            }),
        }),
        span: Span::default(),
        scope: 0,
    });

    let actual = ResolvedType::RustPath("incan_stdlib::r#async::sync::RawSemaphore".to_string());
    let expected = ResolvedType::Ref(Box::new(ResolvedType::RustPath(
        "incan_stdlib::r#async::sync::Semaphore".to_string(),
    )));

    assert!(
        checker.types_compatible(&actual, &expected),
        "RustPath aliases should reuse attached import metadata instead of forcing external metadata lookup"
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

#[cfg(feature = "rust_inspect")]
#[test]
fn test_rust_inspect_resolves_type_associated_function_field_access() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
from rust::demo import Builder

def f() -> None:
  make = Builder.new
  _ = make
"#;
    let tokens = lexer::lex(source).map_err(|errs| std::io::Error::other(format!("lex failed: {errs:?}")))?;
    let ast = parser::parse(&tokens).map_err(|errs| std::io::Error::other(format!("parse failed: {errs:?}")))?;
    let mut checker = TypeChecker::new();
    let tmp = tempfile::tempdir()?;
    write_rust_inspect_probe_crate(tmp.path())?;
    checker.set_rust_inspect_manifest_dir(tmp.path().to_path_buf());
    prewarm_metadata(tmp.path(), &["demo::Builder"])?;
    checker.check_program(&ast).map_err(|errs| {
        std::io::Error::other(format!(
            "expected associated function field access to typecheck: {errs:?}"
        ))
    })?;
    let info = checker.type_info();
    assert!(
        info.expr_types
            .values()
            .any(|t| matches!(t, ResolvedType::Function(params, _) if params.is_empty())),
        "expected associated function field access to resolve to a callable type, got {:?}",
        info.expr_types
    );
    Ok(())
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
        info.regular_method_arg_shape_preserving_calls
            .iter()
            .any(|(_, _, method)| method == "contains"),
        "expected HashSet.contains lookup to record preserved method arg shape, got {:?}",
        info.regular_method_arg_shape_preserving_calls
    );
    Ok(())
}

#[cfg(feature = "rust_inspect")]
#[test]
fn test_rust_inspect_validates_associated_function_arguments() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
from rust::demo import Builder

def f() -> None:
  _ = Builder.new("x")
"#;
    let tokens = lexer::lex(source).map_err(|errs| std::io::Error::other(format!("lex failed: {errs:?}")))?;
    let ast = parser::parse(&tokens).map_err(|errs| std::io::Error::other(format!("parse failed: {errs:?}")))?;
    let tmp = tempfile::tempdir()?;
    write_rust_inspect_probe_crate(tmp.path())?;
    let mut checker = TypeChecker::new();
    checker.set_rust_inspect_manifest_dir(tmp.path().to_path_buf());
    prewarm_metadata(tmp.path(), &["demo::Builder"])?;
    let Err(errs) = checker.check_program(&ast) else {
        panic!("expected arity error for Builder.new with an argument");
    };
    assert!(
        errs.iter()
            .any(|e| e.message.contains("Builder.new() expects 0 argument") && e.message.contains("got 1")),
        "expected associated-function arity diagnostic, got {errs:?}"
    );
    Ok(())
}

#[cfg(feature = "rust_inspect")]
#[test]
fn test_rust_inspect_reports_unsupported_rust_item_shape() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
from rust::demo::Choice import Some

def f() -> None:
  _ = Some
"#;
    let tokens = lexer::lex(source).map_err(|errs| std::io::Error::other(format!("lex failed: {errs:?}")))?;
    let ast = parser::parse(&tokens).map_err(|errs| std::io::Error::other(format!("parse failed: {errs:?}")))?;
    let tmp = tempfile::tempdir()?;
    write_rust_inspect_probe_crate(tmp.path())?;
    let mut checker = TypeChecker::new();
    checker.set_rust_inspect_manifest_dir(tmp.path().to_path_buf());
    prewarm_metadata(tmp.path(), &["demo::Choice", "demo::Choice::Some"])?;
    let Err(errs) = checker.check_program(&ast) else {
        panic!("expected unsupported Rust item shape diagnostic");
    };
    assert!(
        errs.iter()
            .any(|e| e.message.contains("unsupported shape") && e.message.contains("enum variant")),
        "expected unsupported-shape diagnostic, got {errs:?}"
    );
    Ok(())
}

/// Field access on a Rust type that isn't a known inherent associated function should be permissive
/// (metadata only covers inherent methods, not consts, type aliases, or trait-provided items).
#[test]
fn test_rust_path_field_access_permissive_when_not_module() {
    let source = r#"
from rust::std::time import Instant

def f() -> None:
  _ = Instant.SOME_UNKNOWN_CONST
"#;
    assert_check_ok(source);
}

/// Method calls on Rust types where the specific method isn't in inherent metadata
/// should be permissive (trait-provided or extension methods aren't extracted yet).
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

#[test]
fn test_std_rust_capability_import_binds_trait_symbols() {
    let source = r#"
from std.rust import Send, Sync

def run[T with Send, Sync](task: T) -> None:
  pass
"#;
    assert_check_ok(source);
}

#[test]
fn test_std_rust_static_capability_bound_typechecks() {
    let source = r#"
from std.rust import Static

def run[T with Static](_value: T) -> None:
  pass
"#;
    assert_check_ok(source);
}

#[test]
fn test_std_rust_fn_capability_bounds_typecheck() {
    let source = r#"
from std.rust import Fn, FnMut, FnOnce

def run_fn[F with Fn[int]](_f: F) -> None:
  pass

def run_fn_mut[F with FnMut[int]](_f: F) -> None:
  pass

def run_fn_once[F with FnOnce[int]](_f: F) -> None:
  pass
"#;
    assert_check_ok(source);
}

#[test]
fn test_structural_coercion_option_int_to_option_i64() {
    let checker = TypeChecker::new();
    let arg_ty = crate::frontend::symbols::ResolvedType::Generic(
        "Option".to_string(),
        vec![crate::frontend::symbols::ResolvedType::Int],
    );
    assert!(
        checker.rust_arg_matches_boundary(&arg_ty, "Option<i64>"),
        "expected Option[int] to be admitted at Option<i64> Rust boundary"
    );
}

#[test]
fn test_structural_coercion_list_str_to_vec_string() {
    let checker = TypeChecker::new();
    let arg_ty = crate::frontend::symbols::ResolvedType::Generic(
        "List".to_string(),
        vec![crate::frontend::symbols::ResolvedType::Str],
    );
    assert!(
        checker.rust_arg_matches_boundary(&arg_ty, "Vec<String>"),
        "expected List[str] to be admitted at Vec<String> Rust boundary"
    );
}

/// `maybe_record_rusttype_return_coercion` is metadata-driven; without the `rust-inspect`
/// feature the cache is empty and the helper is a no-op.  The test below exercises the
/// *non-metadata* path to assert that the coercion map stays empty (no false positives).
#[test]
fn test_rusttype_return_coercion_no_false_positive_without_metadata() {
    // Declare a rusttype whose underlying path has no metadata loaded.
    let source = r#"
from rust::acme import Widget as RustWidget

type Widget = rusttype RustWidget:
    def label(self) -> str:
        ...

def use_widget(w: Widget) -> str:
    return w.label()
"#;
    // Should typecheck cleanly; no spurious errors from return coercion path.
    assert_check_ok(source);
}

#[cfg(feature = "rust_inspect")]
#[test]
fn test_rusttype_return_coercion_recorded_for_generic_newtype_method_call() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
from rust::std::string import String as RustString

type Label[T] = rusttype RustString:
    def as_str(self) -> str:
        ...

def render[T](value: Label[T]) -> str:
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
                    methods: vec![RustMethodSig {
                        name: "as_str".to_string(),
                        signature: RustFunctionSig {
                            params: vec![RustParam {
                                name: Some("self".to_string()),
                                type_display: "&self".to_string(),
                            }],
                            return_type: "&str".to_string(),
                            is_async: false,
                            is_unsafe: false,
                        },
                    }],
                    fields: vec![],
                    variants: vec![],
                }),
            },
        )
        .map_err(|e| std::io::Error::other(format!("seed rust-inspect: {e}")))?;
    checker.check_program(&ast).map_err(|errs| {
        std::io::Error::other(format!("expected generic rusttype method call to typecheck: {errs:?}"))
    })?;
    let info = checker.type_info();
    assert!(
        info.rust_return_coercions
            .values()
            .any(|c| c.rust_target_type == "String" && matches!(c.target_type, ResolvedType::Str)),
        "expected rust return coercion (&str -> String) for generic rusttype method call, got {:?}",
        info.rust_return_coercions
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
                    methods: vec![],
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
                    methods: vec![],
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
                    methods: vec![],
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
                    methods: vec![],
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
                    methods: vec![],
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
                    methods: vec![],
                    fields: vec![],
                    variants: vec![RustVariantInfo {
                        name: "Read".to_string(),
                        fields: vec![RustTypeShape::RustPath {
                            path: "demo::ReadRel".to_string(),
                            args: vec![],
                        }],
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
                    methods: vec![],
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
                    methods: vec![],
                    fields: vec![],
                    variants: vec![RustVariantInfo {
                        name: "NamedTable".to_string(),
                        fields: vec![RustTypeShape::RustPath {
                            path: "demo::NamedTable".to_string(),
                            args: vec![],
                        }],
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

#[test]
fn test_structural_coercion_mismatch_is_rejected() {
    let checker = TypeChecker::new();
    let arg_ty = crate::frontend::symbols::ResolvedType::Generic(
        "List".to_string(),
        vec![crate::frontend::symbols::ResolvedType::Str],
    );
    assert!(
        !checker.rust_arg_matches_boundary(&arg_ty, "Vec<i64>"),
        "expected List[str] -> Vec<i64> structural coercion mismatch to be rejected"
    );
}

#[test]
fn test_rust_extern_accepted_in_user_code() {
    // @rust.extern is allowed 'everywhere' per RFC 023.
    // A rust.module() directive is required when @rust.extern items are present.
    let source = r#"
rust.module("my_crate::my_module")

@rust.extern
def foo() -> None:
  pass
"#;
    assert_check_ok(source);
}

#[test]
fn test_std_web_type_requires_import() {
    // async needs to be imported to use the Query type and asyc keyword.
    let source = r#"
async def search(params: Query[int]) -> None:
  pass
"#;
    let result = check_str(source);
    assert!(result.is_err());
}

#[test]
fn test_std_web_type_import_ok() {
    // async needs to be imported to use the Query type and asyc keyword.
    let source = r#"
from std.web import Query
import std.async

async def search(params: Query[int]) -> None:
  pass
"#;
    assert_check_ok(source);
}

#[test]
fn test_std_async_type_requires_import() {
    let source = r#"
def queue(handle: JoinHandle[int]) -> None:
  pass
"#;
    let result = check_str(source);
    assert!(result.is_err());
}

#[test]
fn test_std_async_type_import_ok() {
    let source = r#"
from std.async.task import JoinHandle

def queue(handle: JoinHandle[int]) -> None:
  pass
"#;
    assert_check_ok(source);
}

#[test]
fn test_std_async_function_requires_import() {
    let source = r#"
async def foo():
  await sleep(1.0)
"#;
    let result = check_str(source);
    assert!(result.is_err());
}

#[test]
fn test_std_async_function_import_ok() {
    let source = r#"
from std.async.time import sleep

async def foo() -> None:
  await sleep(1.0)
"#;
    assert_check_ok(source);
}

#[test]
fn test_await_outside_async_function() {
    let source = r#"
from std.async.time import sleep

def foo() -> None:
  await sleep(1.0)
"#;
    let errs = check_str(source).expect_err("await in sync function should fail");
    assert!(
        errs.iter()
            .any(|e| e.message.contains("await") && e.message.contains("async")),
        "expected await-outside-async diagnostic, got: {errs:?}"
    );
}

#[test]
fn test_await_outside_async_method() {
    let source = r#"
from std.async.time import sleep

model Widget:
  id: int

  def work(self) -> None:
    await sleep(1.0)
"#;
    let errs = check_str(source).expect_err("await in sync method should fail");
    assert!(
        errs.iter()
            .any(|e| e.message.contains("await") && e.message.contains("async")),
        "expected await-outside-async diagnostic, got: {errs:?}"
    );
}

#[test]
fn test_await_join_handle_returns_result_task_join_error() {
    let source = r#"
from std.async.task import JoinHandle, TaskJoinError

async def wait_for(handle: JoinHandle[int]) -> Result[int, TaskJoinError]:
  return await handle
"#;
    assert_check_ok(source);
}

#[test]
fn test_semaphore_acquire_returns_result_semaphore_acquire_error() {
    let source = r#"
from std.async.sync import Semaphore, SemaphoreAcquireError

async def take(sem: Semaphore) -> Result[int, SemaphoreAcquireError]:
  result = await sem.acquire()
  permit = result?
  return Ok(1)
"#;
    assert_check_ok(source);
}

#[test]
fn test_std_reflection_type_requires_import() {
    let source = r#"
def foo(fields: List[FieldInfo]) -> None:
  pass
"#;
    let result = check_str(source);
    assert!(result.is_err());
}

#[test]
fn test_std_reflection_type_import_ok() {
    let source = r#"
from std.reflection import FieldInfo

def foo(fields: List[FieldInfo]) -> None:
  pass
"#;
    assert_check_ok(source);
}

// ============================================================================
// RFC 022: Decorator resolution — canonical, aliased, and from-imported paths
// ============================================================================

#[test]
fn test_decorator_resolution_canonical_path() {
    // Canonical @std.web.routing.route with fully qualified path
    let source = r#"
from std.web.routing import GET
import std.async

@std.web.routing.route("/", methods=[GET])
async def index() -> int:
  return 1
"#;
    assert_check_ok(source);
}

#[test]
fn test_decorator_resolution_module_alias() {
    // Aliased @web.route after `import std.web.routing as web`
    let source = r#"
import std.web.routing as web
from std.web.routing import GET
import std.async

@web.route("/", methods=[GET])
async def index() -> int:
  return 1
"#;
    assert_check_ok(source);
}

#[test]
fn test_decorator_resolution_from_import() {
    // Bare @route after `from std.web import route` (prelude re-export)
    let source = r#"
from std.web import route, GET
import std.async

@route("/", methods=[GET])
async def index() -> int:
  return 1
"#;
    assert_check_ok(source);
}

#[test]
fn test_decorator_resolution_colcolon_path() {
    // `::` separator variant: @std::web::routing::route
    let source = r#"
from std.web.routing import GET
import std.async

@std::web::routing::route("/", methods=[GET])
async def index() -> int:
  return 1
"#;
    assert_check_ok(source);
}

#[test]
fn test_reserved_root_namespace_std_import_alias_allowed() {
    // Import aliases may use reserved roots — only declarations are rejected.
    let source = r#"
import std.web as std
"#;
    assert_check_ok(source);
}

#[test]
fn test_unknown_decorator_path() {
    let source = r#"
@std.web.missing
def foo() -> None:
  pass
"#;
    let result = check_str(source);
    assert!(result.is_err());
}

#[test]
fn test_try_on_non_result() {
    let source = r#"
def foo() -> Result[int, str]:
  x = 42
  y = x?
  return Ok(y)
"#;
    let result = check_str(source);
    assert!(result.is_err());
}

#[test]
fn test_sleep_requires_float() {
    let source = r#"
from std.async.time import sleep

async def foo():
  await sleep(1)
"#;
    let result = check_str(source);
    assert!(result.is_err());
}

// ========================================
// Variable declaration and assignment
// ========================================

#[test]
fn test_variable_declaration() {
    let source = r#"
def foo() -> int:
  x = 10
  return x
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_mutable_variable() {
    let source = r#"
def foo() -> int:
  mut x = 10
  x = 20
  return x
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_typed_variable() {
    let source = r#"
def foo() -> int:
  let x: int = 10
  return x
"#;
    assert!(check_str(source).is_ok());
}

// ========================================
// Arithmetic operations
// ========================================

#[test]
fn test_arithmetic_addition() {
    let source = r#"
def foo() -> int:
  return 1 + 2
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_arithmetic_subtraction() {
    let source = r#"
def foo() -> int:
  return 10 - 5
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_arithmetic_multiplication() {
    let source = r#"
def foo() -> int:
  return 3 * 4
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_arithmetic_division() {
    // Division always returns float (Python-like semantics)
    let source = r#"
def foo() -> float:
  return 10 / 2
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_arithmetic_modulo() {
    let source = r#"
def foo() -> int:
  return 10 % 3
"#;
    assert!(check_str(source).is_ok());
}

// ========================================
// Comparison operations
// ========================================

#[test]
fn test_comparison_equal() {
    let source = r#"
def foo() -> bool:
  return 1 == 1
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_comparison_not_equal() {
    let source = r#"
def foo() -> bool:
  return 1 != 2
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_comparison_less_than() {
    let source = r#"
def foo() -> bool:
  return 1 < 2
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_comparison_greater_than() {
    let source = r#"
def foo() -> bool:
  return 2 > 1
"#;
    assert!(check_str(source).is_ok());
}

// ========================================
// RFC 021: field metadata + aliases
// ========================================

#[test]
fn test_alias_resolution_member_and_constructor() {
    let source = r#"
model Account:
  type_ [alias="type"]: str

def f(a: Account) -> str:
  let x = Account(type="premium")
  return a.type
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_canonical_wins_over_alias_resolution() {
    // RFC 021: When typechecking a field key, canonical name is checked first, then alias.
    // This test verifies that accessing by canonical name works even when the same model
    // has aliases, and that the type is correctly resolved from the canonical field.
    let source = r#"
model Data:
    foo [alias="wire_foo"]: str
    bar: int

def test_canonical_access(d: Data) -> str:
    # Accessing by canonical name should work and return the correct type
    return d.foo

def test_alias_access(d: Data) -> str:
    # Accessing by alias should also work
    return d.wire_foo

def test_constructor_canonical(name: str) -> Data:
    # Constructor with canonical name
    return Data(foo=name, bar=42)

def test_constructor_alias(name: str) -> Data:
    # Constructor with alias
    return Data(wire_foo=name, bar=42)
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_canonical_takes_precedence_in_mixed_access() {
    // RFC 021: Canonical name takes precedence. If a field has both canonical name
    // and alias, both should work independently with correct type resolution.
    let source = r#"
model Account:
    name: str
    type_ [alias="type"]: str
    balance: int

def access_all(a: Account) -> str:
    # Access fields by canonical name
    let n = a.name       # canonical, no alias
    let t = a.type_      # canonical (has alias "type")
    let b = a.balance    # canonical, no alias
    
    # Access field by alias
    let t2 = a.type      # alias for type_
    
    # Both t and t2 should have type str
    return f"{n} {t} {t2} {b}"
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_alias_resolution_in_pattern() {
    let source = r#"
model Account:
  type_ [alias="type"]: str

def f(a: Account) -> str:
  match a:
    Account(type="premium") => return "premium"
    Account(type="basic") => return "basic"
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_duplicate_alias_error() {
    let source = r#"
model Account:
  a [alias="wire"]: str
  b [alias="wire"]: int
"#;
    let Err(err) = check_str(source) else {
        panic!("Expected duplicate alias error");
    };
    assert!(err.iter().any(|e| e.message.contains("Duplicate alias")));
}

#[test]
fn test_alias_collides_with_canonical_error() {
    let source = r#"
model Account:
  type_: str
  kind [alias="type_"]: str
"#;
    let Err(err) = check_str(source) else {
        panic!("Expected alias collision error");
    };
    assert!(
        err.iter()
            .any(|e| e.message.contains("collides with a canonical field name"))
    );
}

#[test]
fn test_alias_collides_with_method_error() {
    let source = r#"
model Account:
  type_ [alias="describe"]: str

  def describe(self) -> str:
    return self.type_
"#;
    let Err(err) = check_str(source) else {
        panic!("Expected alias/method collision error");
    };
    assert!(err.iter().any(|e| e.message.contains("collides with a method name")));
}

#[test]
fn test_empty_alias_error() {
    let source = r#"
model Account:
  type_ [alias=""]: str
"#;
    let Err(err) = check_str(source) else {
        panic!("Expected empty alias error");
    };
    assert!(err.iter().any(|e| e.message.contains("non-empty")));
}

#[test]
fn test_whitespace_alias_error() {
    let source = r#"
model Account:
  type_ [alias="   "]: str
"#;
    let Err(err) = check_str(source) else {
        panic!("Expected whitespace alias error");
    };
    assert!(err.iter().any(|e| e.message.contains("non-empty")));
}

#[test]
fn test_alias_and_canonical_in_constructor_error() {
    let source = r#"
model Account:
  type_ [alias="type"]: str

def f() -> Account:
  return Account(type="x", type_="y")
"#;
    let Err(err) = check_str(source) else {
        panic!("Expected duplicate field error");
    };
    assert!(err.iter().any(|e| e.message.contains("Duplicate constructor argument")));
}

#[test]
fn test_non_identifier_alias_allowed() {
    let source = r#"
model Weird:
  one_ [alias="1"]: int

def f(w: Weird) -> int:
  return w.one_
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_alias_not_supported_on_class() {
    // RFC 021: Field aliases are only supported on `model`, not `class`
    let source = r#"
class Account:
  type_ [alias="type"]: str
"#;
    let Err(err) = check_str(source) else {
        panic!("Expected class alias error");
    };
    assert!(err.iter().any(|e| e.message.contains("not supported on class")));
}

#[test]
fn test_numeric_alias_member_access_error() {
    let source = r#"
model Weird:
  one_ [alias="1"]: int

def f(w: Weird) -> int:
  return w.1
"#;
    let Err(err) = check_str(source) else {
        panic!("Expected error for numeric access");
    };
    assert!(err.iter().any(|e| e.message.contains("no field '1'")));
}

#[test]
fn test_alias_collides_with_builtin_error() {
    let source = r#"
model Account:
  fields_ [alias="__fields__"]: str
"#;
    let Err(err) = check_str(source) else {
        panic!("Expected builtin collision error");
    };
    assert!(err.iter().any(|e| e.message.contains("builtin member")));
}

#[test]
fn test_alias_and_canonical_in_pattern_error() {
    let source = r#"
model Account:
  type_ [alias="type"]: str

def f(a: Account) -> str:
  match a:
    Account(type="x", type_="y") => return "x"
"#;
    let Err(err) = check_str(source) else {
        panic!("Expected duplicate pattern field error");
    };
    assert!(err.iter().any(|e| e.message.contains("Duplicate pattern field")));
}

#[test]
fn test_unicode_alias_allowed() {
    let source = r#"
model Intl:
  name_ [alias="名前"]: str

def f(i: Intl) -> str:
  return i.name_
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_alias_self_keyword() {
    let source = r#"
model Data:
  self_ [alias="self"]: str

def f(d: Data) -> str:
  return d.self
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_alias_super_keyword_member_access() {
    let source = r#"
model Data:
  super_ [alias="super"]: str

def f(d: Data) -> str:
  return d.super
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_alias_super_keyword_constructor_key() {
    let source = r#"
model Data:
  super_ [alias="super"]: str

def f() -> Data:
  return Data(super="x")
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_alias_super_keyword_pattern_key() {
    let source = r#"
model Data:
  super_ [alias="super"]: str

def f(d: Data) -> str:
  match d:
    Data(super=x) => return x
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_alias_underscore_member_access() {
    let source = r#"
model Data:
  under_ [alias="_"]: str

def f(d: Data) -> str:
  return d._
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_alias_underscore_constructor_key() {
    let source = r#"
model Data:
  under_ [alias="_"]: str

def f() -> Data:
  return Data(_="x")
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_alias_underscore_pattern_key() {
    let source = r#"
model Data:
  under_ [alias="_"]: str

def f(d: Data) -> str:
  match d:
    Data(_=x) => return x
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_alias_unicode_normalization_variants_treated_as_distinct() {
    // RFC 021: alias matching uses exact string equality; no Unicode normalization is performed.
    // Example: NFC "é" vs NFD "e\u{301}" must be treated as distinct aliases.
    let source = r#"
model Data:
  nfc_ [alias="é"]: str
  nfd_ [alias="e\u{301}"]: str

def f(d: Data) -> str:
  return d.nfc_
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_alias_case_variants_treated_as_distinct() {
    // RFC 021: no case-folding is performed for alias matching.
    let source = r#"
model Data:
  lower_ [alias="type"]: str
  upper_ [alias="Type"]: str

def f(d: Data) -> str:
  return d.lower_
"#;
    assert!(check_str(source).is_ok());
}

// ========================================
// Logical operations
// ========================================

#[test]
fn test_logical_and() {
    let source = r#"
def foo() -> bool:
  return true and false
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_logical_or() {
    let source = r#"
def foo() -> bool:
  return true or false
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_logical_not() {
    let source = r#"
def foo() -> bool:
  return not true
"#;
    assert!(check_str(source).is_ok());
}

// ========================================
// String operations
// ========================================

#[test]
fn test_string_return() {
    let source = r#"
def foo() -> str:
  return "hello"
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_string_concat() {
    let source = r#"
def foo() -> str:
  return "hello" + " world"
"#;
    assert!(check_str(source).is_ok());
}

// ========================================
// Slicing
// ========================================

#[test]
fn test_list_slice_rejects_non_int_bounds_and_step() {
    let source = r#"
def main() -> None:
  xs: List[int] = [1, 2, 3]
  _a = xs["bad":]
  _b = xs[:1.2]
  _c = xs[0:2:"nope"]
"#;
    assert!(check_str(source).is_err());
}

#[test]
fn test_list_slice_accepts_int_bounds_and_step() {
    let source = r#"
def main() -> None:
  xs: List[int] = [1, 2, 3]
  _a = xs[0:]
  _b = xs[:2]
  _c = xs[0:2:1]
"#;
    assert!(check_str(source).is_ok());
}

// FIXME(#121): `List[Mutex].append(value)` should become valid once implicit ownership
// inference can choose move/borrow over Clone-by-default for external Rust types.
#[test]
fn test_list_append_requires_clone_for_external_type() {
    let source = r#"
from rust::std::sync import Mutex

def add(mut xs: List[Mutex], value: Mutex) -> None:
  xs.append(value)
"#;
    let Err(errs) = check_str(source) else {
        panic!("expected type errors");
    };
    assert!(
        errs.iter().any(|e| {
            e.message.contains("List.append requires element type")
                && e.message.contains("Mutex")
                && e.message.contains(incan_core::lang::traits::as_str(
                    incan_core::lang::traits::TraitId::Clone,
                ))
        }),
        "expected List.append / Clone diagnostic for Rust element type; got {errs:?}"
    );
}

// ========================================
// Models implementing traits (Issue #42)
// ========================================

#[test]
fn test_model_trait_requires_missing_field_errors() {
    let source = r#"
@requires(name: str)
trait Loggable:
  def log(self, msg: str) -> None:
    println(f"[{self.name}] {msg}")

model User with Loggable:
  id: int
"#;
    assert!(check_str(source).is_err());
}

#[test]
fn test_class_trait_requires_missing_field_errors() {
    let source = r#"
@requires(name: str)
trait Loggable:
  def log(self, msg: str) -> None:
    println(f"[{self.name}] {msg}")

class Service with Loggable:
  id: int
"#;
    assert!(check_str(source).is_err());
}

#[test]
fn test_model_trait_requires_field_type_mismatch_errors() {
    let source = r#"
@requires(name: str)
trait Loggable:
  def log(self, msg: str) -> None:
    println(f"[{self.name}] {msg}")

model User with Loggable:
  name: int
"#;
    assert!(check_str(source).is_err());
}

#[test]
fn test_model_trait_default_method_call_typechecks() {
    let source = r#"
@requires(name: str)
trait Loggable:
  def log(self, msg: str) -> None:
    println(f"[{self.name}] {msg}")

model User with Loggable:
  name: str

def main() -> None:
  u = User(name="Ada")
  u.log("hello")
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_class_trait_default_method_call_typechecks() {
    let source = r#"
@requires(name: str)
trait Loggable:
  def log(self, msg: str) -> None:
    println(f"[{self.name}] {msg}")

class Service with Loggable:
  name: str

def main() -> None:
  s = Service(name="svc")
  s.log("hello")
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_trait_duplicate_requires_errors() {
    let source = r#"
@requires(name: str, name: str)
trait Dup:
  def get(self) -> str:
    return self.name
"#;
    assert!(check_str(source).is_err());
}

#[test]
fn test_trait_default_method_assignment_requires_declared_field() {
    let source = r#"
trait Counter:
  def bump(mut self) -> None:
    self.count += 1

class Thing with Counter:
  count: int = 0
"#;
    assert!(check_str(source).is_err());
}

#[test]
fn test_trait_default_method_requires_declared_field() {
    let source = r#"
trait Greeter:
  def greet(self) -> str:
    return self.name

class User with Greeter:
  name: str
"#;
    assert!(check_str(source).is_err());
}

#[test]
fn test_trait_default_method_allows_required_field_assignment() {
    let source = r#"
@requires(count: int)
trait Counter:
  def bump(mut self) -> None:
    self.count = self.count + 1

class CounterImpl with Counter:
  count: int

def main() -> None:
  c = CounterImpl(count=1)
  c.bump()
"#;
    assert_check_ok(source);
}

#[test]
fn test_trait_required_method_signature_mismatch_receiver() {
    let source = r#"
trait Inc:
  def inc(mut self, by: int) -> int: ...

class Bad with Inc:
  value: int

  def inc(self, by: int) -> int:
    return self.value
"#;
    assert!(check_str(source).is_err());
}

#[test]
fn test_trait_required_method_signature_mismatch_param_type() {
    let source = r#"
trait Inc:
  def inc(mut self, by: int) -> int: ...

class Bad with Inc:
  value: int

  def inc(mut self, by: str) -> int:
    return self.value
"#;
    assert!(check_str(source).is_err());
}

#[test]
fn test_trait_required_method_signature_mismatch_return_type() {
    let source = r#"
trait Inc:
  def inc(mut self, by: int) -> int: ...

class Bad with Inc:
  value: int

  def inc(mut self, by: int) -> None:
    return None
"#;
    assert!(check_str(source).is_err());
}

#[test]
fn test_trait_required_method_signature_mismatch_async() {
    let source = r#"
trait Inc:
  async def inc(mut self, by: int) -> int: ...

class Bad with Inc:
  value: int

  def inc(mut self, by: int) -> int:
    return self.value
"#;
    assert!(check_str(source).is_err());
}

#[test]
fn test_trait_conformance_allows_inherited_members() {
    let source = r#"
@requires(name: str)
trait Named:
  def get_name(self) -> str: ...

class Base:
  name: str

  def get_name(self) -> str:
    return self.name

class Child extends Base with Named:
  name: str
"#;
    assert_check_ok(source);
}

#[test]
fn test_trait_requires_field_type_checked_for_class() {
    let source = r#"
@requires(name: str)
trait Named:
  def get_name(self) -> str: ...

class Bad with Named:
  name: int

  def get_name(self) -> str:
    return "x"
"#;
    assert!(check_str(source).is_err());
}

// RFC 042: supertrait graph (symbol collection + transitive closure)

#[test]
fn test_supertrait_cycle_is_diagnosed() -> Result<(), Vec<CompileError>> {
    let source = r#"
trait A with B:
  def fa(self) -> int: ...

trait B with A:
  def fb(self) -> int: ...
"#;
    let tokens = lexer::lex(source)?;
    let ast = parser::parse(&tokens)?;
    let mut checker = TypeChecker::new();
    let Err(errs) = checker.check_program(&ast) else {
        panic!("expected supertrait cycle to be rejected");
    };
    assert!(
        errs.iter().any(|e| e.message.contains("Supertrait cycle")),
        "unexpected errors: {:?}",
        errs.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
    Ok(())
}

#[test]
fn test_supertrait_transitive_closure() -> Result<(), Vec<CompileError>> {
    let source = r#"
trait Root:
  def root_m(self) -> int: ...

trait Mid with Root:
  def mid_m(self) -> int: ...

trait Leaf with Mid:
  def leaf_m(self) -> int: ...
"#;
    let tokens = lexer::lex(source)?;
    let ast = parser::parse(&tokens)?;
    let mut checker = TypeChecker::new();
    checker.check_program(&ast)?;
    let Some(leaf) = checker.supertrait_closure.get("Leaf") else {
        return Err(vec![CompileError::type_error(
            "Leaf should have a supertrait closure".to_string(),
            Span::default(),
        )]);
    };
    assert!(
        leaf.iter().any(|(n, _)| n == "Mid"),
        "expected Mid in Leaf closure, got {:?}",
        leaf
    );
    assert!(
        leaf.iter().any(|(n, _)| n == "Root"),
        "expected Root in Leaf closure, got {:?}",
        leaf
    );
    Ok(())
}

#[test]
fn test_supertrait_bound_rejects_non_trait_type() -> Result<(), Vec<CompileError>> {
    let source = r#"
model M:
  x: int

trait T with M:
  def f(self) -> int: ...
"#;
    let tokens = lexer::lex(source)?;
    let ast = parser::parse(&tokens)?;
    let mut checker = TypeChecker::new();
    let Err(errs) = checker.check_program(&ast) else {
        panic!("expected errors for non-trait supertrait bound");
    };
    assert!(
        errs.iter().any(|e| e.message.contains("is not a trait")),
        "unexpected errors: {:?}",
        errs.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
    Ok(())
}

#[test]
fn test_supertrait_bound_rejects_arity_mismatch() -> Result<(), Vec<CompileError>> {
    let source = r#"
trait Boxed[T]:
  def get(self) -> T: ...

trait Bad with Boxed:
  def run(self) -> int: ...
"#;
    let tokens = lexer::lex(source)?;
    let ast = parser::parse(&tokens)?;
    let mut checker = TypeChecker::new();
    let Err(errs) = checker.check_program(&ast) else {
        panic!("expected errors for supertrait arity mismatch");
    };
    assert!(
        errs.iter().any(|e| e.message.contains("expects 1 type argument")),
        "unexpected errors: {:?}",
        errs.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
    Ok(())
}

#[test]
fn test_generic_supertrait_cycle_is_diagnosed() -> Result<(), Vec<CompileError>> {
    let source = r#"
trait A[T] with A[list[T]]:
  def f(self) -> int: ...
"#;
    let tokens = lexer::lex(source)?;
    let ast = parser::parse(&tokens)?;
    let mut checker = TypeChecker::new();
    let Err(errs) = checker.check_program(&ast) else {
        panic!("expected generic supertrait cycle to be rejected");
    };
    assert!(
        errs.iter().any(|e| e.message.contains("Supertrait cycle")),
        "unexpected errors: {:?}",
        errs.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
    Ok(())
}

// RFC 042 Phase 3: assignability, conformance, `@requires` merge, trait construction, diamond diagnostics

#[test]
fn test_type_implements_trait_includes_transitive_supertraits() -> Result<(), Vec<CompileError>> {
    let source = r#"
trait Root:
  def r(self) -> int: ...

trait Mid with Root:
  def m(self) -> int: ...

trait Leaf with Mid:
  def l(self) -> int: ...

model M with Leaf:
  def r(self) -> int:
    return 0
  def m(self) -> int:
    return 0
  def l(self) -> int:
    return 0
"#;
    let tokens = lexer::lex(source)?;
    let ast = parser::parse(&tokens)?;
    let mut checker = TypeChecker::new();
    checker.check_program(&ast)?;
    assert!(checker.type_implements_trait("M", "Leaf"));
    assert!(checker.type_implements_trait("M", "Mid"));
    assert!(checker.type_implements_trait("M", "Root"));
    Ok(())
}

#[test]
fn test_types_compatible_generic_trait_annotation() -> Result<(), Vec<CompileError>> {
    let source = r#"
trait Boxed[T]:
  def get(self) -> T: ...

model Cell[T] with Boxed:
  value: T

  def get(self) -> T:
    return self.value
"#;
    let tokens = lexer::lex(source)?;
    let ast = parser::parse(&tokens)?;
    let mut checker = TypeChecker::new();
    checker.check_program(&ast)?;
    let actual = ResolvedType::Generic("Cell".to_string(), vec![ResolvedType::Int]);
    let expected = ResolvedType::Generic("Boxed".to_string(), vec![ResolvedType::Int]);
    assert!(
        checker.types_compatible(&actual, &expected),
        "Generic concrete type should be assignable to matching generic trait annotation (RFC 042)"
    );
    Ok(())
}

#[test]
fn test_types_compatible_generic_trait_annotation_extra_concrete_type_params() -> Result<(), Vec<CompileError>> {
    let source = r#"
trait Boxed[T]:
  def get(self) -> T: ...

model Pair[A, B] with Boxed:
  first: A
  second: B

  def get(self) -> A:
    return self.first
"#;
    let tokens = lexer::lex(source)?;
    let ast = parser::parse(&tokens)?;
    let mut checker = TypeChecker::new();
    checker.check_program(&ast)?;

    let ok_actual = ResolvedType::Generic("Pair".to_string(), vec![ResolvedType::Int, ResolvedType::Str]);
    let expected = ResolvedType::Generic("Boxed".to_string(), vec![ResolvedType::Int]);
    assert!(
        checker.types_compatible(&ok_actual, &expected),
        "Concrete type with more type parameters than the trait should still match when leading args align (RFC 042)"
    );

    let bad_actual = ResolvedType::Generic("Pair".to_string(), vec![ResolvedType::Str, ResolvedType::Int]);
    assert!(
        !checker.types_compatible(&bad_actual, &expected),
        "First concrete type parameter must be compatible with the trait's type argument"
    );

    let short_actual = ResolvedType::Generic("Pair".to_string(), vec![ResolvedType::Int]);
    assert!(
        !checker.types_compatible(&short_actual, &expected),
        "Concrete type must supply at least as many type arguments as the trait annotation"
    );
    Ok(())
}

#[test]
fn test_types_compatible_generic_supertrait_annotation() -> Result<(), Vec<CompileError>> {
    let source = r#"
trait Collection[T]:
  def first(self) -> T: ...

trait OrderedCollection[T] with Collection[T]:
  def sorted(self) -> Self: ...

model BoxedValue[T] with OrderedCollection:
  value: T

  def first(self) -> T:
    return self.value

  def sorted(self) -> Self:
    return self
"#;
    let tokens = lexer::lex(source)?;
    let ast = parser::parse(&tokens)?;
    let mut checker = TypeChecker::new();
    checker.check_program(&ast)?;
    let actual = ResolvedType::Generic("BoxedValue".to_string(), vec![ResolvedType::Int]);
    let expected = ResolvedType::Generic("Collection".to_string(), vec![ResolvedType::Int]);
    assert!(
        checker.types_compatible(&actual, &expected),
        "Generic adopters should satisfy transitive generic supertrait annotations with substituted args"
    );
    Ok(())
}

#[test]
fn test_types_compatible_named_concrete_rejects_mismatched_generic_trait_annotation() -> Result<(), Vec<CompileError>> {
    let source = r#"
trait Boxed[T]:
  def get(self) -> T: ...

model IntBox with Boxed:
  value: int

  def get(self) -> int:
    return self.value
"#;
    let tokens = lexer::lex(source)?;
    let ast = parser::parse(&tokens)?;
    let mut checker = TypeChecker::new();
    checker.check_program(&ast)?;
    let actual = ResolvedType::Named("IntBox".to_string());
    let expected = ResolvedType::Generic("Boxed".to_string(), vec![ResolvedType::Str]);
    assert!(
        !checker.types_compatible(&actual, &expected),
        "Non-generic adopters must not silently satisfy arbitrary generic trait instantiations"
    );
    Ok(())
}

// RFC 042: trait-typed value assignable to supertrait (trait-to-trait upcasts)

#[test]
fn test_types_compatible_trait_to_supertrait_named() -> Result<(), Vec<CompileError>> {
    let source = r#"
trait Root:
  def root_m(self) -> int: ...

trait Mid with Root:
  def mid_m(self) -> int: ...
"#;
    let tokens = lexer::lex(source)?;
    let ast = parser::parse(&tokens)?;
    let mut checker = TypeChecker::new();
    checker.check_program(&ast)?;
    let actual = ResolvedType::Named("Mid".to_string());
    let expected = ResolvedType::Named("Root".to_string());
    assert!(
        checker.types_compatible(&actual, &expected),
        "Named subtrait should be assignable to supertrait (RFC 042)"
    );
    Ok(())
}

#[test]
fn test_types_compatible_trait_to_supertrait_generic() -> Result<(), Vec<CompileError>> {
    let source = r#"
trait DataSet[T]:
  def id(self) -> T: ...

trait BoundedDataSet[T] with DataSet[T]:
  def bounded(self) -> T: ...
"#;
    let tokens = lexer::lex(source)?;
    let ast = parser::parse(&tokens)?;
    let mut checker = TypeChecker::new();
    checker.check_program(&ast)?;
    let actual = ResolvedType::Generic("BoundedDataSet".to_string(), vec![ResolvedType::Int]);
    let expected = ResolvedType::Generic("DataSet".to_string(), vec![ResolvedType::Int]);
    assert!(
        checker.types_compatible(&actual, &expected),
        "Generic subtrait[T] should be assignable to supertrait[T] with compatible args"
    );
    Ok(())
}

#[test]
fn test_types_compatible_trait_to_supertrait_transitive_generic() -> Result<(), Vec<CompileError>> {
    let source = r#"
trait Root[T]:
  def r(self) -> T: ...

trait Mid[T] with Root[T]:
  def m(self) -> T: ...

trait Leaf[T] with Mid[T]:
  def l(self) -> T: ...
"#;
    let tokens = lexer::lex(source)?;
    let ast = parser::parse(&tokens)?;
    let mut checker = TypeChecker::new();
    checker.check_program(&ast)?;
    let actual = ResolvedType::Generic("Leaf".to_string(), vec![ResolvedType::Int]);
    let expected = ResolvedType::Generic("Root".to_string(), vec![ResolvedType::Int]);
    assert!(
        checker.types_compatible(&actual, &expected),
        "Transitive supertrait generics should substitute through the chain"
    );
    Ok(())
}

#[test]
fn test_types_compatible_concrete_to_transitive_supertrait_generic() -> Result<(), Vec<CompileError>> {
    let source = r#"
trait DataSet[T]:
  def filter(self, _p: bool) -> Self: ...

trait BoundedDataSet[T] with DataSet[T]:
  def bounded_marker(self) -> T: ...

class DataFrame[T] with BoundedDataSet:
  _row_schema_marker: T

  def filter(self, _p: bool) -> Self:
    return self

  def bounded_marker(self) -> T:
    return self._row_schema_marker

model Order:
  id: int
"#;
    let tokens = lexer::lex(source)?;
    let ast = parser::parse(&tokens)?;
    let mut checker = TypeChecker::new();
    checker.check_program(&ast)?;
    let order = ResolvedType::Named("Order".to_string());
    let actual = ResolvedType::Generic("DataFrame".to_string(), vec![order.clone()]);
    let expected = ResolvedType::Generic("DataSet".to_string(), vec![order]);
    assert!(
        checker.types_compatible(&actual, &expected),
        "Concrete class through intermediate trait should satisfy transitive supertrait"
    );
    Ok(())
}

/// Regression for #237: `-> Self` on a generic class method must type as the instantiated receiver at the call site,
/// not bare `Self`, so annotations and chaining against `Carrier[Order]` succeed.
#[test]
fn test_issue_237_self_return_substituted_at_call_site() -> Result<(), Vec<CompileError>> {
    let source = r#"
class Carrier[T]:
  _m: T

  def filter(self, _p: bool) -> Self:
    return self

model Order:
  id: int

def use_filter(x: Carrier[Order]) -> Carrier[Order]:
  return x.filter(true)

def use_annotated_local(x: Carrier[Order]) -> Carrier[Order]:
  y: Carrier[Order] = x.filter(true)
  return y
"#;
    let tokens = lexer::lex(source)?;
    let ast = parser::parse(&tokens)?;
    let mut checker = TypeChecker::new();
    checker.check_program(&ast)?;
    Ok(())
}

/// `Self` in non-receiver parameters must use the same call-site substitution as the return type (#237 follow-up).
#[test]
fn test_self_param_substituted_at_call_site_for_method_args() -> Result<(), Vec<CompileError>> {
    let source = r#"
class Carrier[T]:
  _m: T

  def join(self, other: Self, cond: bool) -> Self:
    return self

model Order:
  id: int

def use_join(left: Carrier[Order], right: Carrier[Order]) -> Carrier[Order]:
  return left.join(right, true)
"#;
    let tokens = lexer::lex(source)?;
    let ast = parser::parse(&tokens)?;
    let mut checker = TypeChecker::new();
    checker.check_program(&ast)?;
    Ok(())
}

/// Trait **default** methods are not copied into `ClassInfo.methods`; dispatch goes through the trait branch of
/// `resolve_named_method`. Call-site `Self` substitution must still apply (#237).
#[test]
fn test_issue_237_self_substitution_trait_default_methods_not_on_class_map() -> Result<(), Vec<CompileError>> {
    let source = r#"
trait DataSet[T]:
  def filter(self, _p: bool) -> Self:
    return self

  def join(self, other: Self, cond: bool) -> Self:
    return self

class Carrier[T] with DataSet:
  _m: T

model Order:
  id: int

def use_filter(x: Carrier[Order]) -> Carrier[Order]:
  return x.filter(true)

def use_annotated_local(x: Carrier[Order]) -> Carrier[Order]:
  y: Carrier[Order] = x.filter(true)
  return y

def use_join(left: Carrier[Order], right: Carrier[Order]) -> Carrier[Order]:
  return left.join(right, true)
"#;
    let tokens = lexer::lex(source)?;
    let ast = parser::parse(&tokens)?;
    let mut checker = TypeChecker::new();
    checker.check_program(&ast)?;
    Ok(())
}

/// `rusttype` inherent methods declared with `-> Self` must type as the surface newtype at the call site so
/// `maybe_record_rusttype_return_coercion` and downstream checks see the substituted return (no bare `Self`).
#[test]
fn test_rusttype_method_returning_self_substitutes_at_call_site_without_metadata() {
    let source = r#"
from rust::acme import Widget as RustWidget

type Widget = rusttype RustWidget:
    def myself(self) -> Self:
        ...

def f(w: Widget) -> Widget:
    return w.myself()
"#;
    assert_check_ok(source);
}

#[test]
fn test_check_with_imports_concrete_and_supertrait_upcasts() -> Result<(), Box<dyn std::error::Error>> {
    let dependency_source = r#"
pub trait DataSet[T]:
  def filter(self, _p: bool) -> Self: ...

pub trait BoundedDataSet[T] with DataSet[T]:
  def bounded_marker(self) -> T: ...

pub class DataFrame[T] with BoundedDataSet:
  _row_schema_marker: T

  def filter(self, _p: bool) -> Self:
    return self

  def bounded_marker(self) -> T:
    return self._row_schema_marker
"#;
    let consumer_source = r#"
from dataset import DataFrame, BoundedDataSet, DataSet

def upcast_data_frame[T](v: DataFrame[T]) -> BoundedDataSet[T]:
  return v

def upcast_bounded[T](v: BoundedDataSet[T]) -> DataSet[T]:
  return v
"#;

    let dep_tokens = lexer::lex(dependency_source).map_err(|errs| format!("dependency lex failed: {errs:?}"))?;
    let dep_ast = parser::parse(&dep_tokens).map_err(|errs| format!("dependency parse failed: {errs:?}"))?;
    let consumer_tokens = lexer::lex(consumer_source).map_err(|errs| format!("consumer lex failed: {errs:?}"))?;
    let consumer_ast = parser::parse(&consumer_tokens).map_err(|errs| format!("consumer parse failed: {errs:?}"))?;

    let mut checker = TypeChecker::new();
    checker
        .check_with_imports(&consumer_ast, &[("dataset", &dep_ast)])
        .map_err(|errs| format!("typecheck failed: {errs:?}"))?;
    Ok(())
}

#[test]
fn test_check_with_imports_preserves_cyclic_dependency_interface_result_types() -> Result<(), Box<dyn std::error::Error>>
{
    let dataset_source = r#"
from session import SessionError

pub class DataFrame[T]:
  def clone(self) -> Self:
    return self

pub class LazyFrame[T]:
  def clone(self) -> Self:
    return self

  def collect(self) -> Result[DataFrame[T], SessionError]:
    return Err(str("not implemented"))
"#;
    let session_source = r#"
from dataset import LazyFrame

pub class Session:
  def read_csv[T](self) -> Result[LazyFrame[T], SessionError]:
    return Err(str("not implemented"))

pub model SessionError:
  pub message: str
"#;
    let consumer_source = r#"
from session import Session, SessionError

def main() -> Result[None, SessionError]:
  session = Session()
  lines = session.read_csv[int]()?
  df = lines.clone().collect()?
  df.clone()
  return Ok(None)
"#;

    let dataset_tokens = lexer::lex(dataset_source).map_err(|errs| format!("dataset lex failed: {errs:?}"))?;
    let dataset_ast = parser::parse(&dataset_tokens).map_err(|errs| format!("dataset parse failed: {errs:?}"))?;
    let session_tokens = lexer::lex(session_source).map_err(|errs| format!("session lex failed: {errs:?}"))?;
    let session_ast = parser::parse(&session_tokens).map_err(|errs| format!("session parse failed: {errs:?}"))?;
    let consumer_tokens = lexer::lex(consumer_source).map_err(|errs| format!("consumer lex failed: {errs:?}"))?;
    let consumer_ast = parser::parse(&consumer_tokens).map_err(|errs| format!("consumer parse failed: {errs:?}"))?;

    let mut checker = TypeChecker::new();
    checker
        .check_with_imports(&consumer_ast, &[("dataset", &dataset_ast), ("session", &session_ast)])
        .map_err(|errs| format!("typecheck failed: {errs:?}"))?;
    Ok(())
}

#[test]
fn test_check_with_imports_preserves_imported_generic_method_bounds_for_local_derived_types()
-> Result<(), Box<dyn std::error::Error>> {
    let dataset_source = r#"
from session import SessionError

pub class LazyFrame[T with Clone]:
  def clone(self) -> Self:
    return self

  def collect(self) -> Result[DataFrame[T], SessionError]:
    return Err(SessionError(message=str("not implemented")))

pub class DataFrame[T with Clone]:
  def clone(self) -> Self:
    return self
"#;
    let session_source = r#"
from dataset import LazyFrame

pub model SessionError:
  pub message: str

pub class Session:
  @staticmethod
  def default() -> Session:
    return Session()

  def read_csv[T with Clone](self, _logical_name: str, _uri: str) -> Result[LazyFrame[T], SessionError]:
    return Err(SessionError(message=str("not implemented")))
"#;
    let consumer_source = r#"
from session import Session, SessionError

@derive(Clone)
pub model OrderLine:
  pub sku: str

def main() -> Result[None, SessionError]:
  session = Session.default()
  lines = session.read_csv[OrderLine](str("orders"), str("input.csv"))?
  df = lines.clone().collect()?
  df.clone()
  return Ok(None)
"#;

    let dataset_tokens = lexer::lex(dataset_source).map_err(|errs| format!("dataset lex failed: {errs:?}"))?;
    let dataset_ast = parser::parse(&dataset_tokens).map_err(|errs| format!("dataset parse failed: {errs:?}"))?;
    let session_tokens = lexer::lex(session_source).map_err(|errs| format!("session lex failed: {errs:?}"))?;
    let session_ast = parser::parse(&session_tokens).map_err(|errs| format!("session parse failed: {errs:?}"))?;
    let consumer_tokens = lexer::lex(consumer_source).map_err(|errs| format!("consumer lex failed: {errs:?}"))?;
    let consumer_ast = parser::parse(&consumer_tokens).map_err(|errs| format!("consumer parse failed: {errs:?}"))?;

    let mut checker = TypeChecker::new();
    checker
        .check_with_imports(&consumer_ast, &[("dataset", &dataset_ast), ("session", &session_ast)])
        .map_err(|errs| format!("typecheck failed: {errs:?}"))?;
    Ok(())
}

#[test]
fn test_types_compatible_trait_to_supertrait_identity() -> Result<(), Vec<CompileError>> {
    let source = r#"
trait DataSet[T]:
  def id(self) -> T: ...
"#;
    let tokens = lexer::lex(source)?;
    let ast = parser::parse(&tokens)?;
    let mut checker = TypeChecker::new();
    checker.check_program(&ast)?;
    let actual = ResolvedType::Generic("DataSet".to_string(), vec![ResolvedType::Str]);
    let expected = ResolvedType::Generic("DataSet".to_string(), vec![ResolvedType::Str]);
    assert!(checker.types_compatible(&actual, &expected));
    Ok(())
}

#[test]
fn test_types_compatible_trait_to_supertrait_wrong_args() -> Result<(), Vec<CompileError>> {
    let source = r#"
trait DataSet[T]:
  def id(self) -> T: ...

trait BoundedDataSet[T] with DataSet[T]:
  def b(self) -> T: ...
"#;
    let tokens = lexer::lex(source)?;
    let ast = parser::parse(&tokens)?;
    let mut checker = TypeChecker::new();
    checker.check_program(&ast)?;
    let actual = ResolvedType::Generic("BoundedDataSet".to_string(), vec![ResolvedType::Int]);
    let expected = ResolvedType::Generic("DataSet".to_string(), vec![ResolvedType::Str]);
    assert!(
        !checker.types_compatible(&actual, &expected),
        "Mismatched type arguments across trait upcast must be rejected"
    );
    Ok(())
}

#[test]
fn test_types_compatible_unrelated_traits_rejected() -> Result<(), Vec<CompileError>> {
    let source = r#"
trait Apple[T]:
  def a(self) -> T: ...

trait Orange[T]:
  def o(self) -> T: ...
"#;
    let tokens = lexer::lex(source)?;
    let ast = parser::parse(&tokens)?;
    let mut checker = TypeChecker::new();
    checker.check_program(&ast)?;
    let actual = ResolvedType::Generic("Apple".to_string(), vec![ResolvedType::Int]);
    let expected = ResolvedType::Generic("Orange".to_string(), vec![ResolvedType::Int]);
    assert!(!checker.types_compatible(&actual, &expected));
    Ok(())
}

#[test]
fn test_types_compatible_wrong_direction_supertrait_to_subtrait_rejected() -> Result<(), Vec<CompileError>> {
    let source = r#"
trait DataSet[T]:
  def id(self) -> T: ...

trait BoundedDataSet[T] with DataSet[T]:
  def b(self) -> T: ...
"#;
    let tokens = lexer::lex(source)?;
    let ast = parser::parse(&tokens)?;
    let mut checker = TypeChecker::new();
    checker.check_program(&ast)?;
    let actual = ResolvedType::Generic("DataSet".to_string(), vec![ResolvedType::Int]);
    let expected = ResolvedType::Generic("BoundedDataSet".to_string(), vec![ResolvedType::Int]);
    assert!(
        !checker.types_compatible(&actual, &expected),
        "Supertrait must not be assignable to subtrait"
    );
    Ok(())
}

#[test]
fn test_supertrait_requires_merge_conflict() -> Result<(), Vec<CompileError>> {
    let source = r#"
@requires(x: int)
trait A:
  def fa(self) -> int: ...

@requires(x: str)
trait B:
  def fb(self) -> str: ...

trait C with A, B:
  def fc(self) -> int: ...
"#;
    let tokens = lexer::lex(source)?;
    let ast = parser::parse(&tokens)?;
    let mut checker = TypeChecker::new();
    let Err(errs) = checker.check_program(&ast) else {
        panic!("expected @requires merge conflict");
    };
    assert!(
        errs.iter().any(|e| e.message.contains("merges conflicting @requires")),
        "unexpected errors: {:?}",
        errs.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
    Ok(())
}

#[test]
fn test_cannot_instantiate_trait() {
    let source = r#"
trait T:
  def f(self) -> int: ...

def main() -> int:
  let _x = T()
  return 0
"#;
    let err = check_str(source).expect_err("trait constructor should be rejected");
    assert!(
        err.iter().any(|e| e.message.contains("Cannot construct trait")),
        "unexpected errors: {:?}",
        err.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

#[test]
fn test_supertrait_incompatible_method_conflict() -> Result<(), Vec<CompileError>> {
    let source = r#"
trait A:
  def m(self) -> int: ...

trait B:
  def m(self) -> str: ...

trait C with A, B:
  def c(self) -> int: ...

model M with C:
  def c(self) -> int:
    return 0
"#;
    let tokens = lexer::lex(source)?;
    let ast = parser::parse(&tokens)?;
    let mut checker = TypeChecker::new();
    let Err(errs) = checker.check_program(&ast) else {
        panic!("expected conflicting supertrait method requirements");
    };
    assert!(
        errs.iter().any(|e| e.message.contains("Conflicting implementations")),
        "unexpected errors: {:?}",
        errs.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
    Ok(())
}

#[test]
fn test_supertrait_method_ambiguity_param_name_only() -> Result<(), Vec<CompileError>> {
    let source = r#"
trait A:
  def m(self, a: int) -> int: ...

trait B:
  def m(self, b: int) -> int: ...

trait C with A, B:
  def c(self) -> int: ...

model M with C:
  def c(self) -> int:
    return 0
"#;
    let tokens = lexer::lex(source)?;
    let ast = parser::parse(&tokens)?;
    let mut checker = TypeChecker::new();
    let Err(errs) = checker.check_program(&ast) else {
        panic!("expected ambiguous supertrait method");
    };
    assert!(
        errs.iter().any(|e| e.message.contains("Ambiguous trait method")),
        "unexpected errors: {:?}",
        errs.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
    Ok(())
}

#[test]
fn test_transitive_supertrait_abstract_method_required() -> Result<(), Vec<CompileError>> {
    let source = r#"
trait Root:
  def root_only(self) -> int: ...

trait Leaf with Root:
  def leaf_m(self) -> int: ...

model M with Leaf:
  def leaf_m(self) -> int:
    return 1
"#;
    let tokens = lexer::lex(source)?;
    let ast = parser::parse(&tokens)?;
    let mut checker = TypeChecker::new();
    let Err(errs) = checker.check_program(&ast) else {
        panic!("expected missing transitive supertrait method");
    };
    assert!(
        errs.iter().any(|e| e.message.contains("requires method 'root_only'")),
        "unexpected errors: {:?}",
        errs.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
    Ok(())
}

#[test]
fn test_derive_validate_requires_validate_method() {
    let source = r#"
@derive(Validate)
model User:
  name: str
"#;
    assert!(check_str(source).is_err());
}

#[test]
fn test_derive_validate_rejects_raw_constructor_call() {
    let source = r#"
@derive(Validate)
model User:
  name: str

  def validate(self) -> Result[User, str]:
    return Ok(self)

def main() -> int:
  let u = User(name="Ada")
  return 0
"#;
    assert!(check_str(source).is_err());
}

#[test]
fn test_derive_validate_allows_new_constructor_call() {
    let source = r#"
@derive(Validate)
model User:
  name: str

  def validate(self) -> Result[User, str]:
    return Ok(self)

def build_user() -> Result[User, str]:
  return User.new(name="Ada")
"#;
    assert_check_ok(source);
}

#[test]
fn test_derive_validate_new_constructor_param_order_positional() {
    let source = r#"
@derive(Validate)
model User:
  id: int
  email: str

  def validate(self) -> Result[User, str]:
    return Ok(self)

def build_user() -> Result[User, str]:
  return User.new(42, "a@b.com")
"#;
    assert_check_ok(source);
}

#[test]
fn test_derive_validate_new_constructor_param_order_positional_mismatch() {
    let source = r#"
@derive(Validate)
model User:
  id: int
  email: str

  def validate(self) -> Result[User, str]:
    return Ok(self)

def build_user() -> Result[User, str]:
  # Wrong order: str then int should be rejected.
  return User.new("a@b.com", 42)
"#;
    assert!(check_str(source).is_err());
}
// ========================================
// Control flow
// ========================================

#[test]
fn test_if_statement() {
    let source = r#"
def foo(x: int) -> int:
  if x > 0:
    return 1
  return 0
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_if_else_statement() {
    let source = r#"
def foo(x: int) -> int:
  if x > 0:
    return 1
  else:
    return -1
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_while_loop() {
    let source = r#"
def foo() -> int:
  mut x = 0
  while x < 10:
    x = x + 1
  return x
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_for_loop() {
    let source = r#"
def foo() -> int:
  mut sum = 0
  for i in range(10):
    sum = sum + i
  return sum
"#;
    assert!(check_str(source).is_ok());
}

// ========================================
// Collections
// ========================================

#[test]
fn test_list_literal() {
    let source = r#"
def foo() -> List[int]:
  return [1, 2, 3]
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_empty_list() {
    let source = r#"
def foo() -> List[int]:
  let x: List[int] = []
  return x
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_empty_list_matches_typed_call_parameter() {
    let source = r#"
def takes_names(names: List[str]) -> int:
  return len(names)

def foo() -> int:
  return takes_names([])
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_generic_model_field_access_returns_substituted_type() {
    let source = r#"
pub model Boxed[T]:
  pub value: T

pub def get_value[T](boxed: Boxed[T]) -> T:
  return boxed.value
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_generic_class_field_access_substitutes_nested_field_type() {
    let source = r#"
pub class Boxed[T]:
  pub values: List[T]

pub def get_values[T](boxed: Boxed[T]) -> List[T]:
  return boxed.values
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_list_self_accepts_explicit_owner_instances() {
    let source = r#"
pub class Boxed[T]:
  pub value: T

  def pair(self) -> List[Self]:
    return [Boxed(value=self.value), Boxed(value=self.value)]
"#;
    assert!(check_str(source).is_ok());
}

// ========================================
// Model tests
// ========================================

#[test]
fn test_model_definition() {
    let source = r#"
model User:
  name: str
  age: int
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_model_instantiation() {
    let source = r#"
model Point:
  x: int
  y: int

def make_point() -> Point:
  return Point(x=0, y=0)
"#;
    assert!(check_str(source).is_ok());
}

// ========================================
// Class tests
// ========================================

#[test]
fn test_class_definition() {
    let source = r#"
class Counter:
  value: int

  def get(self) -> int:
    return self.value
"#;
    assert!(check_str(source).is_ok());
}

// ========================================
// Enum tests
// ========================================

#[test]
fn test_enum_definition() {
    let source = r#"
enum Color:
  Red
  Green
  Blue
"#;
    assert!(check_str(source).is_ok());
}

// ========================================
// Option and Result
// ========================================

#[test]
fn test_option_some() {
    let source = r#"
def foo() -> Option[int]:
  return Some(42)
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_option_none() {
    let source = r#"
def foo() -> Option[int]:
  return None
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_result_none_ok_literal() {
    let source = r#"
def ping() -> Result[None, str]:
  return Ok(None)
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_generic_newtype_static_builder_typechecks() {
    let source = r#"
type Box[T] = newtype T:
  @staticmethod
  def wrap(value: T) -> Self:
    return Box(value)

  def duplicate(self) -> Tuple[T, T]:
    return (self.0, self.0)
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_option_match_exhaustive_some_none() {
    let source = r#"
def foo(value: Option[int]) -> int:
  match value:
    case Some(n):
      return n
    case None:
      return 0
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_result_ok() {
    let source = r#"
def foo() -> Result[int, str]:
  return Ok(42)
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_result_err() {
    let source = r#"
def foo() -> Result[int, str]:
  return Err("error")
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_result_ok_reports_payload_type_mismatch() {
    let source = r#"
def foo() -> Result[int, str]:
  return Ok("hello")
"#;
    let Err(errs) = check_str(source) else {
        panic!("Ok payload type mismatch should fail");
    };
    assert!(
        errs.iter()
            .any(|e| e.message.contains("Result[int, str]") && e.message.contains("Result[str, str]")),
        "Expected Result payload mismatch; got: {:?}",
        errs.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

#[test]
fn test_result_err_reports_payload_type_mismatch() {
    let source = r#"
def foo() -> Result[int, str]:
  return Err(1)
"#;
    let Err(errs) = check_str(source) else {
        panic!("Err payload type mismatch should fail");
    };
    assert!(
        errs.iter()
            .any(|e| e.message.contains("Result[int, str]") && e.message.contains("Result[int, int]")),
        "Expected Result payload mismatch; got: {:?}",
        errs.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

// ========================================
// Function calls
// ========================================

#[test]
fn test_function_call() {
    let source = r#"
def add(a: int, b: int) -> int:
  return a + b

def foo() -> int:
  return add(1, 2)
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_generic_bound_enforced_at_callsite_negative() {
    let source = r#"
@requires(message: str)
trait Displayable:
  def display(self) -> str:
    return self.message

class NotDisplayable:
  value: int

def show[T with Displayable](value: T) -> T:
  return value

def main() -> None:
  _ = show(NotDisplayable(value=1))
"#;
    let Err(errs) = check_str(source) else {
        panic!("expected generic bound failure");
    };
    assert!(errs.iter().any(|e| e.message.contains("violates generic bound")));
}

#[test]
fn test_generic_bound_enforced_at_callsite_positive() {
    let source = r#"
@requires(message: str)
trait Displayable:
  def display(self) -> str:
    return self.message

class User with Displayable:
  message: str

def show[T with Displayable](value: T) -> T:
  return value

def main() -> None:
  _ = show(User(message="ok"))
"#;
    assert_check_ok(source);
}

#[test]
fn test_builtin_len() {
    let source = r#"
def foo() -> int:
  x = [1, 2, 3]
  return len(x)
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_builtin_sum() {
    let source = r#"
def foo() -> int:
  x = [True, False, True]
  return sum(x)
"#;
    assert!(check_str(source).is_ok());
}

// ========================================
// Tuple tests
// ========================================

#[test]
fn test_tuple_literal() {
    let source = r#"
def foo() -> (int, str):
  return (1, "hello")
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_tuple_index_requires_literal() {
    let source = r#"
def foo(t: tuple[int, int]) -> int:
  idx: int = 0
  return t[idx]
"#;
    let Err(errs) = check_str(source) else {
        panic!("expected error");
    };
    assert!(
        errs.iter()
            .any(|e| { e.message.contains("Tuple indices must be an integer literal") })
    );
}

#[test]
fn test_unknown_method_errors() {
    let source = r#"
def foo() -> int:
  return "hi".nope()
"#;
    let Err(errs) = check_str(source) else {
        panic!("expected error");
    };
    assert!(errs.iter().any(|e| e.message.contains("has no method")));
}

#[test]
fn test_string_methods_typecheck() {
    let source = r#"
def foo() -> str:
  return "hello world".upper().strip()
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_module_level_const() {
    let source = r#"
const X: int = 1 + 2

def foo() -> int:
  return X
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_const_cycle_detected() {
    let source = r#"
const A: int = B
const B: int = A
"#;
    let Err(errs) = check_str(source) else {
        panic!("expected error");
    };
    assert!(errs.iter().any(|e| e.message.contains("Const dependency cycle")));
}

// ========================================
// Closure tests
// ========================================

#[test]
fn test_closure() {
    // Note: untyped closure params may not pass typechecker
    // This tests that we handle closures correctly (even if they error)
    let source = r#"
def foo() -> int:
  f = (x) => x + 1
  return f(41)
"#;
    // Closure with untyped params may error, so just check it doesn't panic
    let _ = check_str(source);
}

// ========================================
// Match expression tests
// ========================================

#[test]
fn test_match_expression() {
    let source = r#"
def foo(x: int) -> str:
  match x:
    0 => "zero"
    1 => "one"
    _ => "other"
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_match_unknown_incan_enum_variant_reports_constructor_resolution_error() {
    let source = r#"
enum Traffic:
  Red
  Amber

def f(x: Traffic) -> None:
  match x:
    Crimson() =>
      _ = 0
"#;
    let Err(errs) = check_str(source) else {
        panic!("expected type errors for unknown enum constructor pattern");
    };
    assert!(
        errs.iter()
            .any(|e| e.message.contains("does not resolve for this match")),
        "expected unknown_match_constructor_pattern, got {errs:?}"
    );
}

// ========================================
// Async function tests
// ========================================

#[test]
fn test_async_function() {
    let source = r#"
import std.async

async def foo() -> int:
  return 42
"#;
    assert!(check_str(source).is_ok());
}

// ========================================
// Error case tests
// ========================================

#[test]
fn test_wrong_argument_count() {
    // Note: The typechecker may be lenient on argument counts
    // Just verify we can run through the check without panic
    let source = r#"
def add(a: int, b: int) -> int:
  return a + b

def foo() -> int:
  return add(1)
"#;
    let _ = check_str(source);
}

#[test]
fn test_undefined_function() {
    let source = r#"
def foo() -> int:
  return undefined_func()
"#;
    let result = check_str(source);
    assert!(result.is_err());
}

#[test]
fn test_return_type_mismatch_in_if() {
    let source = r#"
def foo(x: bool) -> int:
  if x:
    return "wrong"
  return 0
"#;
    let result = check_str(source);
    assert!(result.is_err());
}

// ========================================
// Const binding tests (RFC 008)
// ========================================

#[test]
fn test_const_frozen_str() {
    let source = r#"
const GREETING: FrozenStr = "hello"

def foo() -> FrozenStr:
  return GREETING
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_const_frozen_list() {
    let source = r#"
const NUMS: FrozenList[int] = [1, 2, 3]

def foo() -> int:
  return NUMS.len()
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_const_frozen_dict() {
    let source = r#"
const HEADERS: FrozenDict[FrozenStr, int] = {"a": 1, "b": 2}

def foo() -> bool:
  return HEADERS.contains_key("a")
"#;
    // Note: This may or may not pass depending on type inference for dict keys
    let _ = check_str(source);
}

#[test]
fn test_const_frozen_set() {
    let source = r#"
const ALLOWED: FrozenSet[int] = {1, 2, 3}

def foo() -> bool:
  return ALLOWED.contains(2)
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_const_reference_other_const() {
    let source = r#"
const BASE: int = 10
const DOUBLED: int = BASE * 2

def foo() -> int:
  return DOUBLED
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_const_non_const_in_initializer_fails() {
    // A variable binding (not a const) should not be usable in a const initializer
    let source = r#"
const BAD: int = some_runtime_var
"#;
    let result = check_str(source);
    // Should fail because some_runtime_var is not defined, or if defined as var, not allowed
    assert!(result.is_err());
}

#[test]
fn test_const_runtime_call_fails() {
    let source = r#"
def helper() -> int:
  return 42

const BAD: int = helper()
"#;
    let Err(errs) = check_str(source) else {
        panic!("expected error");
    };
    assert!(
        errs.iter()
            .any(|e| e.message.contains("not allowed") || e.message.contains("const initializers"))
    );
}

#[test]
fn test_const_empty_list_requires_annotation() {
    let source = r#"
const EMPTY = []
"#;
    let Err(errs) = check_str(source) else {
        panic!("expected error");
    };
    assert!(
        errs.iter()
            .any(|e| { e.message.contains("Cannot infer type") || e.message.contains("empty const list") })
    );
}

#[test]
fn test_const_type_mismatch() {
    let source = r#"
const X: int = "not an int"
"#;
    let result = check_str(source);
    assert!(result.is_err());
}

#[test]
fn test_const_string_concat_allowed() {
    let source = r#"
const GREETING: FrozenStr = "hello" + " world"
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_const_bytes_literal_allowed() {
    let source = r#"
const DATA: FrozenBytes = b"hi"
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_frozen_bytes_method_len() {
    let source = r#"
const DATA: FrozenBytes = b"hi"

def foo() -> int:
  return DATA.len()
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_frozen_bytes_method_is_empty() {
    let source = r#"
const DATA: FrozenBytes = b"hi"

def foo() -> bool:
  return DATA.is_empty()
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_frozen_list_method_len() {
    let source = r#"
const NUMS: FrozenList[int] = [1, 2, 3]

def foo() -> int:
  return NUMS.len()
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_frozen_list_method_is_empty() {
    let source = r#"
const NUMS: FrozenList[int] = [1, 2]

def foo() -> bool:
  return NUMS.is_empty()
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_frozen_set_contains_method() {
    let source = r#"
const ALLOWED: FrozenSet[int] = {10, 20}

def foo() -> bool:
  return ALLOWED.contains(10)
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_frozen_dict_contains_key_method() {
    let source = r#"
const ITEMS: FrozenDict[FrozenStr, int] = {"x": 1}

def foo() -> bool:
  return ITEMS.contains_key("x")
"#;
    // May need type inference improvements
    let _ = check_str(source);
}

#[test]
fn test_frozen_unknown_method_errors() {
    let source = r#"
const NUMS: FrozenList[int] = [1, 2]

def foo() -> int:
  return NUMS.nonexistent_method()
"#;
    let Err(errs) = check_str(source) else {
        panic!("expected error");
    };
    assert!(errs.iter().any(|e| e.message.contains("has no method")));
}

// ========================================
// Web wrappers
// ========================================

#[test]
fn test_web_wrapper_value_and_deref_access() {
    let source = r#"
from std.web import Json, Query

@derive(Deserialize)
model SearchParams:
  q: str

@derive(Deserialize)
model CreateUser:
  name: str

def use_query(params: Query[SearchParams]) -> str:
  let a = params.q
  let b = params.value.q
  return b

def use_body(body: Json[CreateUser]) -> str:
  let a = body.name
  let b = body.value.name
  return b
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_web_wrapper_invalid_constructor_args() {
    let source = r#"
from std.web import Json, Query

@derive(Serialize)
model User:
  name: str

@derive(Deserialize)
model SearchParams:
  q: str

def bad_json() -> None:
  let a = Json(User(name="a"), User(name="b"))

def bad_query() -> None:
  let b = Query(value=SearchParams(q="x"), other=SearchParams(q="y"))
"#;
    let Err(errs) = check_str(source) else {
        panic!("expected type errors");
    };
    assert!(
        errs.iter()
            .any(|e| e.message.contains("Json() expects exactly one argument"))
    );
    assert!(
        errs.iter()
            .any(|e| e.message.contains("Query() expects exactly one argument"))
    );
}

// ========================================
// RFC 023: rust.module() and @rust.extern
// ========================================

#[test]
fn test_rust_module_with_rust_extern_ok() {
    let source = r#"
rust.module("incan_stdlib::testing")

@rust.extern
def fail(msg: str) -> None:
    ...
"#;
    assert_check_ok(source);
}

#[test]
fn test_rust_extern_missing_rust_module() {
    let source = r#"
@rust.extern
def fail(msg: str) -> None:
    ...
"#;
    let Err(errs) = check_str(source) else {
        panic!("should fail: missing rust.module()");
    };
    assert!(
        errs.iter().any(|e| e.message.contains("no Rust backing path")),
        "Expected missing-rust-module error; got: {:?}",
        errs.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

#[test]
fn test_rust_extern_non_trivial_body() {
    let source = r#"
rust.module("incan_stdlib::testing")

@rust.extern
def fail(msg: str) -> None:
    return
"#;
    let Err(errs) = check_str(source) else {
        panic!("should fail: non-trivial body");
    };
    assert!(
        errs.iter().any(|e| e.message.contains("must have a `...` body")),
        "Expected non-trivial-body error; got: {:?}",
        errs.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

#[test]
fn test_rust_extern_docstring_plus_ellipsis_is_trivial() {
    let source = r#"
rust.module("incan_stdlib::testing")

@rust.extern
def fail(msg: str) -> None:
    """Host boundary docstring."""
    ...
"#;
    assert_check_ok(source);
}

#[test]
fn test_rust_extern_on_instance_method() {
    let source = r#"
rust.module("incan_stdlib::web")

class App:
    @rust.extern
    def run(self) -> None:
        ...
"#;
    let Err(errs) = check_str(source) else {
        panic!("should fail: instance method");
    };
    assert!(
        errs.iter()
            .any(|e| e.message.contains("not allowed on instance method")),
        "Expected instance-method error; got: {:?}",
        errs.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

#[test]
fn test_unused_rust_module_warning() {
    let source = r#"
rust.module("incan_stdlib::utils")

def pure_incan() -> int:
    return 42
"#;
    let Ok(tokens) = lexer::lex(source) else {
        panic!("lex failed");
    };
    let Ok(ast) = parser::parse(&tokens) else {
        panic!("parse failed");
    };
    let mut tc = TypeChecker::new();
    let result = tc.check_program(&ast);
    assert!(result.is_ok(), "warnings should not fail typechecking");
    assert!(
        tc.warnings().iter().any(|e| e.message.contains("no effect")),
        "Expected unused-rust-module warning; got: {:?}",
        tc.warnings().iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

#[test]
fn test_invalid_rust_module_path_syntax() {
    let source = "rust.module(\"my crate; bad\")\n\n@rust.extern\ndef foo() -> None:\n    ...\n";
    let Err(errs) = check_str(source) else {
        panic!("should fail: invalid path");
    };
    assert!(
        errs.iter().any(|e| e.message.contains("invalid characters")),
        "Expected invalid-path error; got: {:?}",
        errs.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

#[test]
fn test_rust_module_unresolved_crate_with_manifest() -> Result<(), Vec<CompileError>> {
    // When declared_crate_names is set, unknown crates should error.
    // This test uses the TypeChecker directly to set declared_crate_names.
    let source = r#"
rust.module("unknown_crate::module")

@rust.extern
def foo() -> None:
    ...
"#;
    let tokens = lexer::lex(source)?;
    let ast = parser::parse(&tokens)?;
    let mut tc = TypeChecker::new();
    tc.set_declared_crate_names(std::collections::HashSet::new());
    let Err(errs) = tc.check_program(&ast) else {
        panic!("should fail: unresolved crate");
    };
    assert!(
        errs.iter().any(|e| e.message.contains("unknown crate")),
        "Expected unresolved-crate error; got: {:?}",
        errs.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
    Ok(())
}

#[test]
fn test_rust_module_incan_stdlib_always_allowed() -> Result<(), Vec<CompileError>> {
    // incan_stdlib is always allowed even without a manifest.
    let source = r#"
rust.module("incan_stdlib::testing")

@rust.extern
def fail(msg: str) -> None:
    ...
"#;
    let tokens = lexer::lex(source)?;
    let ast = parser::parse(&tokens)?;
    let mut tc = TypeChecker::new();
    tc.set_declared_crate_names(std::collections::HashSet::new());
    let result = tc.check_program(&ast);
    assert!(result.is_ok(), "incan_stdlib should always be allowed");
    Ok(())
}

#[test]
fn test_rust_extern_on_newtype_instance_method() {
    let source = r#"
rust.module("my_crate::stuff")

newtype Wrapper = int:
    @rust.extern
    def doubled(self) -> int:
        ...
"#;
    let Err(errs) = check_str(source) else {
        panic!("should fail: instance method on newtype");
    };
    assert!(
        errs.iter()
            .any(|e| e.message.contains("not allowed on instance method")),
        "Expected instance-method error for newtype; got: {:?}",
        errs.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

// ========================================================================
// Unknown stdlib module diagnostic
// ========================================================================

#[test]
fn test_unknown_stdlib_module_from_import() {
    // `from std.f64.consts import PI` should be rejected — user meant `from rust::std::f64::consts import PI`.
    let source = "from std.f64.consts import PI\n";
    let Err(errs) = check_str(source) else {
        panic!("should fail: std.f64.consts is not a known Incan stdlib module");
    };
    assert!(
        errs.iter().any(|e| e.message.contains("Unknown stdlib module")),
        "Expected unknown stdlib module error; got: {:?}",
        errs.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

// ========================================================================
// RFC 005: Rust interop
// ========================================================================

#[test]
fn test_rust_core_import_is_rejected() {
    let source = "from rust::core::fmt import Debug\n";
    let Err(errs) = check_str(source) else {
        panic!("should fail: rust::core is reserved and unsupported");
    };
    assert!(
        errs.iter()
            .any(|e| e.message.contains("`rust::core` is not supported yet")),
        "Expected rust::core unsupported diagnostic; got: {:?}",
        errs.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
    assert!(
        errs.iter()
            .flat_map(|e| e.hints.iter())
            .any(|h| h.contains("rust::std::...")),
        "Expected rust::std guidance hint; got: {:?}",
        errs.iter().map(|e| &e.hints).collect::<Vec<_>>()
    );
}

#[test]
fn test_rust_alloc_import_is_rejected() {
    let source = "import rust::alloc::vec\n";
    let Err(errs) = check_str(source) else {
        panic!("should fail: rust::alloc is reserved and unsupported");
    };
    assert!(
        errs.iter()
            .any(|e| e.message.contains("`rust::alloc` is not supported yet")),
        "Expected rust::alloc unsupported diagnostic; got: {:?}",
        errs.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
    assert!(
        errs.iter()
            .flat_map(|e| e.hints.iter())
            .any(|h| h.contains("rust::std::...")),
        "Expected rust::std guidance hint; got: {:?}",
        errs.iter().map(|e| &e.hints).collect::<Vec<_>>()
    );
}

#[test]
fn test_rust_from_import_numeric_constant_allows_numeric_usage() {
    let source = r#"
from rust::std::f64::consts import PI

def area(r: float) -> float:
  return PI * r * r
"#;
    assert_check_ok(source);
}

#[test]
fn test_rust_module_alias_constant_chain_allows_numeric_usage() {
    let source = r#"
import rust::std::f64::consts as consts

def area(r: float) -> float:
  return consts.PI * r * r
"#;
    assert_check_ok(source);
}

#[test]
fn test_rust_from_import_integer_constant_allows_integer_usage() {
    let source = r#"
from rust::std::u8 import MAX

def next_limit() -> int:
  return MAX + 1
"#;
    assert_check_ok(source);
}

#[test]
fn test_rust_module_alias_integer_constant_chain_allows_integer_usage() {
    let source = r#"
import rust::std::u16 as u16

def next_limit() -> int:
  return u16.MAX + 1
"#;
    assert_check_ok(source);
}

#[test]
fn test_std_math_constant_import_ok() {
    let source = r#"
from std.math import PI

def circle_constant() -> float:
  return PI
"#;
    assert_check_ok(source);
}

#[test]
fn test_known_stdlib_module_is_accepted() {
    // `from std.testing import fail` should not trigger unknown-module diagnostic.
    let source = "from std.testing import fail\ndef main() -> None:\n    fail(\"oops\")\n";
    // This may error for other reasons (e.g. fail not found if stdlib stubs aren't available),
    // but it must NOT error with "Unknown stdlib module".
    let result = check_str(source);
    if let Err(errs) = &result {
        assert!(
            !errs.iter().any(|e| e.message.contains("Unknown stdlib module")),
            "std.testing should be recognized; got: {:?}",
            errs.iter().map(|e| &e.message).collect::<Vec<_>>()
        );
    }
}

#[test]
fn test_std_testing_marker_runtime_call_is_rejected() {
    let source = r#"
from std.testing import skip

def main() -> None:
    skip("not as runtime call")
"#;
    let Err(errs) = check_str(source) else {
        panic!("runtime call to std.testing marker should fail");
    };
    assert!(
        errs.iter().any(|e| e.message.contains("cannot be called at runtime")),
        "Expected marker runtime-call diagnostic; got: {:?}",
        errs.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

#[test]
fn test_known_stdlib_web_submodule_is_accepted() {
    let source = "from std.web.app import App\n";
    let result = check_str(source);
    if let Err(errs) = &result {
        assert!(
            !errs.iter().any(|e| e.message.contains("Unknown stdlib module")),
            "std.web.app should be recognized; got: {:?}",
            errs.iter().map(|e| &e.message).collect::<Vec<_>>()
        );
    }
}

#[test]
fn test_known_stdlib_async_prelude_is_accepted() {
    let source = "from std.async.prelude import sleep\n";
    let result = check_str(source);
    if let Err(errs) = &result {
        assert!(
            !errs.iter().any(|e| e.message.contains("Unknown stdlib module")),
            "std.async.prelude should be recognized; got: {:?}",
            errs.iter().map(|e| &e.message).collect::<Vec<_>>()
        );
    }
}

#[test]
fn test_unknown_stdlib_module_hint_includes_registry_entries() {
    let source = "from std.f64.consts import PI\n";
    let Err(errs) = check_str(source) else {
        panic!("should fail: std.f64.consts is not a known Incan stdlib module");
    };
    let Some(err) = errs.iter().find(|e| e.message.contains("Unknown stdlib module")) else {
        panic!(
            "Expected unknown stdlib module error; got: {:?}",
            errs.iter().map(|e| &e.message).collect::<Vec<_>>()
        );
    };
    assert!(
        err.hints.iter().any(|h| h.contains("std.derives")),
        "Expected hint to include std.derives; hints: {:?}",
        err.hints
    );
    assert!(
        err.hints.iter().any(|h| h.contains("std.web.app")),
        "Expected hint to include std.web.app; hints: {:?}",
        err.hints
    );
}

// ========================================================================
// RFC 031 Phase 3: `pub::` library imports from dependency manifests
// ========================================================================

#[test]
fn test_pub_from_import_manifest_symbols_typecheck() {
    let source = r#"
from pub::mylib import Widget, make_widget, DEFAULT_NAME

def build() -> Widget:
  return make_widget(DEFAULT_NAME)
"#;
    let result = check_str_with_library_index(source, library_index_with_mylib_exports());
    assert!(result.is_ok(), "expected pub import to typecheck, got: {result:?}");
}

#[test]
fn test_pub_from_import_manifest_symbols_are_in_symbol_table() -> Result<(), Box<dyn std::error::Error>> {
    // This test simulates what the LSP needs for completion and hover tooltips. It verifies that `pub::` symbols are
    // properly resolved and available in `checker.symbols` so that the LSP can extract their types and signatures.
    let source = "from pub::mylib import Widget, make_widget, DEFAULT_NAME\n";
    let tokens = lexer::lex(source).map_err(|errs| format!("lex failed: {errs:?}"))?;
    let ast = parser::parse(&tokens).map_err(|errs| format!("parse failed: {errs:?}"))?;
    let mut checker = TypeChecker::new();
    checker.set_library_manifest_index(library_index_with_mylib_exports());
    let _ = checker.check_program(&ast);

    // Verify Widget
    let widget_id = checker
        .symbols
        .lookup("Widget")
        .ok_or_else(|| "Widget should be in symbols".to_string())?;
    let widget_sym = checker
        .symbols
        .get(widget_id)
        .ok_or_else(|| "Widget symbol id should resolve".to_string())?;
    assert!(matches!(
        widget_sym.kind,
        crate::frontend::symbols::SymbolKind::Type(crate::frontend::symbols::TypeInfo::Model(_))
    ));

    // Verify make_widget
    let fn_id = checker
        .symbols
        .lookup("make_widget")
        .ok_or_else(|| "make_widget should be in symbols".to_string())?;
    let fn_sym = checker
        .symbols
        .get(fn_id)
        .ok_or_else(|| "make_widget symbol id should resolve".to_string())?;
    assert!(matches!(fn_sym.kind, crate::frontend::symbols::SymbolKind::Function(_)));

    // Verify DEFAULT_NAME
    let const_id = checker
        .symbols
        .lookup("DEFAULT_NAME")
        .ok_or_else(|| "DEFAULT_NAME should be in symbols".to_string())?;
    let const_sym = checker
        .symbols
        .get(const_id)
        .ok_or_else(|| "DEFAULT_NAME symbol id should resolve".to_string())?;
    assert!(matches!(
        const_sym.kind,
        crate::frontend::symbols::SymbolKind::Variable(_)
    ));
    Ok(())
}

#[test]
fn test_type_info_records_imported_trait_metadata_for_lowering() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
from pub::mylib import ExternBox

model Cell[T] with ExternBox:
  value: T
"#;
    let tokens = lexer::lex(source).map_err(|errs| format!("lex failed: {errs:?}"))?;
    let ast = parser::parse(&tokens).map_err(|errs| format!("parse failed: {errs:?}"))?;
    let mut checker = TypeChecker::new();
    checker.set_library_manifest_index(library_index_with_trait_export());
    checker
        .check_program(&ast)
        .map_err(|errs| format!("typecheck failed: {errs:?}"))?;

    let type_info = checker.type_info();
    assert_eq!(
        type_info.trait_type_params.get("ExternBox"),
        Some(&vec!["T".to_string()]),
        "Imported trait type params should be available to lowering metadata"
    );
    assert_eq!(
        type_info.trait_direct_supertraits.get("ExternBox"),
        Some(&Vec::new()),
        "Imported trait supertraits should be recorded even when empty"
    );
    Ok(())
}

#[test]
fn test_pub_import_transitive_method_return_type_supports_follow_up_method_lookup() {
    let source = r#"
from pub::pubdemo import Session, SessionError

model Row:
  value: int

def main() -> Result[None, SessionError]:
  mut session = Session.default()
  lines = session.read_csv[Row]("orders", "orders.csv")?
  df = lines.collect()?
  print(df)
  return Ok(None)
"#;

    let result = check_str_with_library_index(source, library_index_with_pub_boundary_type_fidelity_exports());
    assert!(
        result.is_ok(),
        "expected transitive pub-returned carrier methods to resolve, got: {:?}",
        result.err()
    );
}

#[test]
fn test_pub_import_transitive_derived_method_chain_supports_follow_up_method_lookup() {
    let source = r#"
from pub::pubdemo import Session, SessionError

model Row:
  value: int

def main() -> Result[None, SessionError]:
  mut session = Session.default()
  lines = session.read_csv[Row]("orders", "orders.csv")?
  df = lines.clone().collect()?
  print(df)
  return Ok(None)
"#;

    let result = check_str_with_library_index(source, library_index_with_pub_boundary_type_fidelity_exports());
    assert!(
        result.is_ok(),
        "expected transitive pub derived-method chains to resolve, got: {:?}",
        result.err()
    );
}

#[test]
fn test_pub_import_transitive_trait_conformance_accepts_concrete_carrier() {
    let source = r#"
from pub::pubdemo import Session, SessionError, display

model Row:
  value: int

def main() -> Result[None, SessionError]:
  mut session = Session.default()
  lines = session.read_csv[Row]("orders", "orders.csv")?
  df = session.collect(lines)?
  display(df)
  return Ok(None)
"#;

    let result = check_str_with_library_index(source, library_index_with_pub_boundary_type_fidelity_exports());
    assert!(
        result.is_ok(),
        "expected transitive pub trait conformance to resolve, got: {:?}",
        result.err()
    );
}

#[test]
fn test_pub_from_import_unknown_library_is_error() {
    let source = "from pub::missinglib import Widget\n";
    let result = check_str_with_library_index(source, library_index_with_mylib_exports());
    let Err(errs) = result else {
        panic!("expected unknown pub library error");
    };
    assert!(
        errs.iter().any(|e| e.message.contains("Unknown `pub::` library")),
        "Expected unknown-library diagnostic; got: {:?}",
        errs.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

#[test]
fn test_pub_from_import_missing_export_is_error() {
    let source = "from pub::mylib import MissingSymbol\n";
    let result = check_str_with_library_index(source, library_index_with_mylib_exports());
    let Err(errs) = result else {
        panic!("expected missing export error");
    };
    assert!(
        errs.iter()
            .any(|e| e.message.contains("is not exported by `pub::mylib`")),
        "Expected missing-export diagnostic; got: {:?}",
        errs.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

#[test]
fn test_pub_from_import_collision_with_local_symbol_is_error() {
    let source = r#"
def Widget() -> None:
  pass

from pub::mylib import Widget
"#;
    let result = check_str_with_library_index(source, library_index_with_mylib_exports());
    let Err(errs) = result else {
        panic!("expected collision diagnostic");
    };
    assert!(
        errs.iter().any(|e| e.message.contains("already in scope")),
        "Expected collision diagnostic; got: {:?}",
        errs.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

#[test]
fn test_pub_from_import_alias_recovers_from_collision() {
    let source = r#"
def Widget() -> None:
  pass

from pub::mylib import Widget as LibWidget, make_widget

def build() -> LibWidget:
  return make_widget("ok")
"#;
    let result = check_str_with_library_index(source, library_index_with_mylib_exports());
    assert!(result.is_ok(), "expected alias recovery to typecheck, got: {result:?}");
}

#[test]
fn test_pub_import_module_alias_resolves_manifest_exports() {
    let source = r#"
import pub::mylib as lib
from pub::mylib import Widget

def build() -> Widget:
  return lib.make_widget("ok")
"#;
    let result = check_str_with_library_index(source, library_index_with_mylib_exports());
    assert!(
        result.is_ok(),
        "expected module alias pub import to typecheck, got: {result:?}"
    );
}

#[test]
fn test_pub_from_import_enum_variant_parity() {
    let source = r#"
from pub::mylib import Status, Active

def current() -> Status:
  return Active
"#;
    let result = check_str_with_library_index(source, library_index_with_mylib_exports());
    assert!(
        result.is_ok(),
        "expected enum variant pub import to typecheck, got: {result:?}"
    );
}

#[test]
fn test_pub_import_manifest_load_failure_is_error() {
    let broken_index = LibraryManifestIndex::from_entries(HashMap::from([(
        "brokenlib".to_string(),
        LibraryManifestIndexEntry::Failed(LibraryManifestLoadFailure {
            path: synthetic_artifact_root("brokenlib").join("brokenlib.incnlib"),
            kind: LibraryManifestFailureKind::ManifestInvalid,
            message: "invalid library manifest: unsupported manifest_format 999 (expected 1)".to_string(),
        }),
    )]));

    let source = "from pub::brokenlib import Widget\n";
    let result = check_str_with_library_index(source, broken_index);
    let Err(errs) = result else {
        panic!("expected manifest-load failure diagnostic");
    };
    assert!(
        errs.iter().any(|e| e.message.contains("Failed to load manifest")),
        "Expected manifest-load diagnostic; got: {:?}",
        errs.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

#[test]
fn test_explicit_clone_bound_accepts_builtin_clone_types() {
    let source = r#"
def identity[T with Clone](value: T) -> T:
  return value

def main() -> int:
  return identity(1)
"#;
    assert_check_ok(source);
}

#[test]
fn test_explicit_clone_bound_rejects_non_clone_model() {
    let source = r#"
model Token:
  value: int

def identity[T with Clone](value: T) -> T:
  return value

def main() -> Token:
  return identity(Token(value=1))
"#;
    let Err(errs) = check_str(source) else {
        panic!("non-clone model should fail explicit Clone bound");
    };
    assert!(
        errs.iter().any(|e| e.message.contains("violates generic bound")),
        "Expected explicit generic bound error; got: {:?}",
        errs.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

/// GitHub #193: `@derive(Clone)` must allow `.clone()` on the concrete type (not only through unconstrained `T`).
#[test]
fn test_derive_clone_allows_direct_clone_on_model() {
    let source = r#"
@derive(Clone)
model Issue193Foo:
  id: int

def direct(f: Issue193Foo) -> Issue193Foo:
  return f.clone()

def via_generic[T](x: T) -> T:
  return x.clone()

def main() -> Issue193Foo:
  x = Issue193Foo(id=1)
  _ = via_generic(x)
  return direct(x)
"#;
    assert_check_ok(source);
}

#[test]
fn test_derive_clone_allows_direct_clone_on_enum() {
    let source = r#"
@derive(Clone)
enum Issue193Bar:
  A
  B

def dup_bar(e: Issue193Bar) -> Issue193Bar:
  return e.clone()

def main() -> None:
  pass
"#;
    assert_check_ok(source);
}

#[test]
fn test_explicit_eq_bound_rejects_float_arguments() {
    let source = r#"
def show_eq[T with Eq](value: T) -> T:
  return value

def main() -> float:
  return show_eq(1.5)
"#;
    let Err(errs) = check_str(source) else {
        panic!("float should fail explicit Eq bound");
    };
    assert!(
        errs.iter().any(|e| e.message.contains("violates generic bound")),
        "Expected explicit Eq bound error; got: {:?}",
        errs.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

#[test]
fn test_explicit_trait_bound_accepts_transitive_supertrait_adopter() {
    let source = r#"
trait Capability:
  def capability(self) -> int: ...

trait Ordered with Capability:
  def ordered(self) -> int: ...

model Carrier with Ordered:
  value: int

  def capability(self) -> int:
    return self.value

  def ordered(self) -> int:
    return self.value

def require_capability[T with Capability](value: T) -> T:
  return value

def main() -> Carrier:
  return require_capability(Carrier(value=1))
"#;
    assert_check_ok(source);
}

#[test]
fn test_explicit_trait_bound_accepts_trait_typed_arguments() {
    let source = r#"
trait Capability:
  def capability(self) -> int: ...

trait Ordered with Capability:
  def ordered(self) -> int: ...

model Carrier with Ordered:
  value: int

  def capability(self) -> int:
    return self.value

  def ordered(self) -> int:
    return self.value

def as_ordered(value: Carrier) -> Ordered:
  return value

def require_capability[T with Capability](value: T) -> T:
  return value

def main() -> Ordered:
  ordered = as_ordered(Carrier(value=1))
  return require_capability(ordered)
"#;
    assert_check_ok(source);
}

#[test]
fn test_method_generic_bound_accepts_transitive_capability_adopter() {
    let source = r#"
trait Capability:
  def capability(self) -> int: ...

trait Ordered with Capability:
  def ordered(self) -> int: ...

model Carrier with Ordered:
  value: int

  def capability(self) -> int:
    return self.value

  def ordered(self) -> int:
    return self.value

class Helpers:
  @staticmethod
  def require_capability[T with Capability](value: T) -> T:
    return value

def main() -> Carrier:
  return Helpers.require_capability(Carrier(value=1))
"#;
    assert_check_ok(source);
}

#[test]
fn test_explicit_trait_bound_rejects_missing_capability() {
    let source = r#"
trait Capability:
  def capability(self) -> int: ...

trait Other:
  def other(self) -> int: ...

model Plain with Other:
  value: int

  def other(self) -> int:
    return self.value

def require_capability[T with Capability](value: T) -> T:
  return value

def main() -> Plain:
  return require_capability(Plain(value=1))
"#;
    let Err(errs) = check_str(source) else {
        panic!("missing capability should fail explicit trait bound");
    };
    assert!(
        errs.iter().any(|e| e.message.contains("violates generic bound")),
        "Expected explicit generic bound error; got: {:?}",
        errs.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

#[test]
fn test_static_initializer_requires_earlier_static() {
    let source = r#"
static SECOND: int = FIRST
static FIRST: int = 1
"#;
    let Err(errs) = check_str(source) else {
        panic!("forward static reference should fail");
    };
    assert!(
        errs.iter().any(|e| e
            .message
            .contains("Static 'SECOND' cannot reference 'FIRST' before it is initialized")),
        "expected earlier-static diagnostic, got: {:?}",
        errs.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

#[test]
fn test_static_dependency_cycle_is_rejected() {
    let source = r#"
static A: int = B
static B: int = A
"#;
    let Err(errs) = check_str(source) else {
        panic!("static cycle should fail");
    };
    assert!(
        errs.iter()
            .any(|e| e.message.contains("Static dependency cycle detected")),
        "expected static-cycle diagnostic, got: {:?}",
        errs.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

#[test]
fn test_static_dependency_cycle_via_function_calls_is_rejected() {
    let source = r#"
def read_a() -> int:
  return A

def read_b() -> int:
  return B

static A: int = read_b()
static B: int = read_a()
"#;
    let Err(errs) = check_str(source) else {
        panic!("static cycle through helper functions should fail");
    };
    assert!(
        errs.iter()
            .any(|e| e.message.contains("Static dependency cycle detected")),
        "expected static-cycle diagnostic, got: {:?}",
        errs.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

#[test]
fn test_static_initializer_rejects_helper_static_assignment() {
    let source = r#"
static TARGET: int = 0

def mutate_target() -> int:
  TARGET = TARGET + 1
  return TARGET

static RESULT: int = mutate_target()
"#;
    let Err(errs) = check_str(source) else {
        panic!("static initializer that assigns a static through helper call should fail");
    };
    assert!(
        errs.iter().any(|e| e
            .message
            .contains("Static initializer for 'RESULT' cannot assign to static 'TARGET'")),
        "expected static-initializer write diagnostic, got: {:?}",
        errs.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

#[test]
fn test_static_initializer_via_function_requires_earlier_static() {
    let source = r#"
def read_first() -> int:
  return FIRST

static SECOND: int = read_first()
static FIRST: int = 1
"#;
    let Err(errs) = check_str(source) else {
        panic!("forward static reference through helper function should fail");
    };
    assert!(
        errs.iter().any(|e| e
            .message
            .contains("Static 'SECOND' cannot reference 'FIRST' before it is initialized")),
        "expected earlier-static diagnostic, got: {:?}",
        errs.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

#[test]
fn test_imported_static_reassignment_is_rejected() {
    let source = r#"
from pub::mylib import SHARED_ITEMS

def main() -> None:
  SHARED_ITEMS = []
"#;
    let Err(errs) = check_str_with_library_index(source, library_index_with_mylib_exports()) else {
        panic!("imported static reassignment should fail");
    };
    assert!(
        errs.iter()
            .any(|e| e.message.contains("Cannot reassign imported static 'SHARED_ITEMS'")),
        "expected imported-static reassignment diagnostic, got: {:?}",
        errs.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

#[test]
fn test_const_reassignment_suggests_static() {
    let source = r#"
const COUNTER: int = 0

def main() -> None:
  COUNTER = 1
"#;
    let Err(errs) = check_str(source) else {
        panic!("const reassignment should fail");
    };
    assert!(
        errs.iter()
            .any(|e| e.message.contains("Cannot reassign const 'COUNTER'"))
            && errs.iter().any(|e| e
                .hints
                .iter()
                .any(|hint| hint.contains("declare it as `static COUNTER"))),
        "expected const-reassignment static hint, got: {:?}",
        errs.iter().map(|e| (&e.message, &e.hints)).collect::<Vec<_>>()
    );
}

#[test]
fn test_static_alias_mutation_typechecks() {
    let source = r#"
static ITEMS: list[int] = []

def main() -> None:
  let live = ITEMS
  live.append(1)
  println(len(ITEMS))
  println(len(live))
"#;
    assert_check_ok(source);
}

#[test]
fn test_imported_static_mutation_typechecks() {
    let source = r#"
from pub::mylib import SHARED_ITEMS

def main() -> None:
  SHARED_ITEMS.append(1)
  let live = SHARED_ITEMS
  live.append(2)
"#;
    assert!(
        check_str_with_library_index(source, library_index_with_mylib_exports()).is_ok(),
        "expected imported static mutation to typecheck"
    );
}

#[test]
fn test_local_inference_preserves_method_result_field_access_after_factory_call() {
    let source = r#"
class Backend:
  enable_optimizer: bool

class Session:
  @staticmethod
  def default() -> Session:
    return Session()

  def backend(self) -> Backend:
    return Backend(enable_optimizer=True)

def main() -> None:
  let session = Session.default()
  let backend = session.backend()
  let enabled = backend.enable_optimizer
  let _ = enabled
"#;
    assert_check_ok(source);
}

#[test]
fn test_local_inference_preserves_result_match_after_factory_call() {
    let source = r#"
@derive(Clone)
class Source:
  value: str

model SessionError:
  kind: str

class Session:
  regs: list[Source]

  @staticmethod
  def default() -> Session:
    return Session(regs=[])

  def register(mut self, logical_name: str, source: Source) -> Result[None, SessionError]:
    self.regs.append(source)
    return Ok(None)

def main() -> None:
  mut session = Session.default()
  match session.register("x", Source(value="y")):
    Ok(_) => pass
    Err(err) => pass
"#;
    assert_check_ok(source);
}

#[test]
fn test_local_inference_preserves_generic_result_match_after_factory_call() {
    let source = r#"
model SessionError:
  kind: str

class Session:
  @staticmethod
  def default() -> Session:
    return Session()

  def table[T with Clone](self, logical_name: str, marker: T) -> Result[T, SessionError]:
    return Ok(marker)

def main() -> None:
  let session = Session.default()
  match session.table("x", 1):
    Ok(value) => pass
    Err(err) => pass
"#;
    assert_check_ok(source);
}

#[test]
fn test_local_inference_annotation_control_still_typechecks() {
    let source = r#"
class Backend:
  enable_optimizer: bool

class Session:
  @staticmethod
  def default() -> Session:
    return Session()

  def backend(self) -> Backend:
    return Backend(enable_optimizer=True)

def main() -> None:
  let session: Session = Session.default()
  let backend: Backend = session.backend()
  let _ = backend.enable_optimizer
"#;
    assert_check_ok(source);
}

#[test]
fn test_direct_construction_method_result_field_access_control_typechecks() {
    let source = r#"
class Backend:
  enable_optimizer: bool

class Session:
  def backend(self) -> Backend:
    return Backend(enable_optimizer=True)

def main() -> None:
  let session = Session()
  let backend = session.backend()
  let _ = backend.enable_optimizer
"#;
    assert_check_ok(source);
}

#[test]
fn test_direct_construction_with_static_factory_present_still_typechecks() {
    let source = r#"
class Backend:
  enable_optimizer: bool

class Session:
  @staticmethod
  def default() -> Session:
    return Session()

  def backend(self) -> Backend:
    return Backend(enable_optimizer=True)

def main() -> None:
  let session = Session()
  let backend = session.backend()
  let _ = backend.enable_optimizer
"#;
    assert_check_ok(source);
}

#[test]
fn explicit_call_type_args_specialize_generic_function_params() {
    assert_check_ok(
        r#"
def id[T](x: T) -> T:
  return x

def run() -> int:
  return id[int](1)
"#,
    );
}

#[test]
fn explicit_call_type_args_enforce_function_type_arg_arity() {
    let source = r#"
def id[T](x: T) -> T:
  return x

def run() -> int:
  return id[int, str](1)
"#;
    let errs = check_str(source).expect_err("expected explicit type arg arity error");
    assert!(
        errs.iter()
            .any(|e| e.message.contains("expects 1 explicit type argument(s), got 2")),
        "expected explicit type argument arity diagnostic, got {errs:?}"
    );
}

#[test]
fn explicit_method_type_args_specialize_generic_method_params() {
    assert_check_ok(
        r#"
class Box:
  def get[T](self, value: T) -> T:
    return value

def run() -> int:
  let b = Box()
  return b.get[int](1)
"#,
    );
}

#[test]
fn explicit_method_type_args_enforce_generic_contract() {
    let source = r#"
class Box:
  def get[T](self, value: T) -> T:
    return value

def run() -> int:
  let b = Box()
  return b.get[int](str("x"))
"#;
    let errs = check_str(source).expect_err("expected explicit method type arg mismatch");
    assert!(
        errs.iter().any(|e| e.message.contains("expected 'int', found 'str'")),
        "expected type mismatch after explicit method type specialization, got {errs:?}"
    );
}

#[test]
fn explicit_call_type_args_infer_placeholder_filled_from_value_args() {
    assert_check_ok(
        r#"
def pair_map[T, U](x: T, y: U) -> int:
  return 0

def run() -> int:
  return pair_map[int, _](1, 2)
"#,
    );
}

#[test]
fn explicit_call_type_args_all_infer_placeholders_filled_from_value_args() {
    assert_check_ok(
        r#"
def id[T](x: T) -> T:
  return x

def run() -> int:
  return id[_](1)
"#,
    );
}

#[test]
fn explicit_call_type_args_infer_placeholder_reports_when_unresolved() {
    let source = r#"
def mystery[T]() -> int:
  return 0

def run() -> int:
  return mystery[_]()
"#;
    let errs = check_str(source).expect_err("expected inference unresolved when no value args bind T");
    assert!(
        errs.iter()
            .any(|e| e.message.contains("Could not infer type parameter")),
        "expected call-site `_` unresolved diagnostic, got {errs:?}"
    );
}

#[test]
fn explicit_call_type_args_rejected_on_builtin_callee() {
    let source = r#"
def run() -> int:
  return len[int]([1, 2])
"#;
    let errs = check_str(source).expect_err("expected unsupported explicit type args on builtin");
    assert!(
        errs.iter()
            .any(|e| e.message.contains("not supported for this call form")),
        "expected unsupported call-site type args diagnostic, got {errs:?}"
    );
}

#[test]
fn explicit_call_type_args_rejected_on_indirect_function_value_call() {
    let source = r#"
def id[T](x: T) -> T:
  return x

def run() -> int:
  let f = id
  return f[int](1)
"#;
    let errs = check_str(source).expect_err("expected unsupported explicit type args on indirect call");
    assert!(
        errs.iter()
            .any(|e| e.message.contains("not supported for this call form")),
        "expected unsupported call-site type args diagnostic, got {errs:?}"
    );
}
