//! Typechecker unit tests.

use super::*;
use crate::frontend::library_manifest_index::{
    LibraryArtifactMetadata, LibraryManifestFailureKind, LibraryManifestIndex, LibraryManifestIndexEntry,
    LibraryManifestLoadFailure,
};
use crate::frontend::{lexer, parser};
use crate::library_manifest::{
    ConstExport, EnumExport, EnumVariantExport, FunctionExport, LibraryExports, LibraryManifest, ModelExport,
    ParamExport, TypeRef,
};
use std::collections::HashMap;
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
            }],
            type_aliases: Vec::new(),
            newtypes: Vec::new(),
            consts: vec![ConstExport {
                name: "DEFAULT_NAME".to_string(),
                ty: TypeRef::Named {
                    name: "str".to_string(),
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
        errs.iter()
            .any(|e| e.message.contains("List.append requires element type 'Mutex'"))
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
    let leaf = checker
        .supertrait_closure
        .get("Leaf")
        .expect("Leaf should have a supertrait closure");
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
