//! Constructors and fields of imported Rust types: associated functions, unit and named constructors, boxed payloads,
//! field assignment and indexing through metadata, and ancestral re-export paths.

use super::*;

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
fn prewarm_metadata(manifest_dir: &std::path::Path, paths: &[&str]) -> Result<(), Box<dyn std::error::Error>> {
    let inspector = Inspector::new(InspectorConfig::new(manifest_dir.to_path_buf()));
    inspector.prewarm(
        paths.iter().map(|path| (*path).to_string()).collect::<Vec<_>>(),
        &|_| (),
    )?;
    Ok(())
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
        info.expressions.expr_types.values().any(|t| matches!(
            t,
            ResolvedType::Function(params, ret)
                if params.is_empty()
                    && matches!(ret.as_ref(), ResolvedType::RustPath(path) if path == "demo::Builder")
        )),
        "expected associated function field access to resolve to a callable type returning demo::Builder, got {:?}",
        info.expressions.expr_types
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

#[test]
fn test_imported_rust_type_like_constructor_without_metadata_is_rejected() {
    let source = r#"
from rust::std::ops import Range

def f() -> None:
  _ = Range(1, 3)
"#;
    let errs = check_str_err(source, "expected Rust constructor metadata diagnostic");
    assert!(
        errs.iter()
            .any(|e| e.message.contains("Cannot construct imported Rust item") && e.message.contains("std::ops::Range")),
        "expected Rust constructor metadata diagnostic, got {errs:?}"
    );
}

#[cfg(feature = "rust_inspect")]
#[test]
fn test_imported_rust_unit_variant_and_zero_field_constructor_typecheck() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
from rust::demo import Empty, Kind, accept_empty, accept_kind

def f() -> None:
  accept_kind(Kind.Unit)
  accept_empty(Empty())
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
                    variants: vec![
                        RustVariantInfo {
                            name: "Unit".to_string(),
                            fields: vec![],
                            field_carriers: Vec::new(),
                        },
                        RustVariantInfo {
                            name: "Tuple".to_string(),
                            fields: vec![RustTypeShape::Int],
                            field_carriers: Vec::new(),
                        },
                    ],
                }),
            },
        )
        .map_err(|e| std::io::Error::other(format!("seed rust-inspect kind: {e}")))?;
    checker
        .rust_inspect_cache
        .insert_test_item(
            &manifest_dir,
            RustItemMetadata {
                canonical_path: "demo::Empty".to_string(),
                definition_path: Some("demo::Empty".to_string()),
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
        .map_err(|e| std::io::Error::other(format!("seed rust-inspect empty: {e}")))?;
    checker
        .rust_inspect_cache
        .insert_test_item(
            &manifest_dir,
            RustItemMetadata {
                canonical_path: "demo::accept_kind".to_string(),
                definition_path: Some("demo::accept_kind".to_string()),
                visibility: RustVisibility::Public,
                kind: RustItemKind::Function(RustFunctionSig {
                    receiver_contract: None,
                    type_params: Vec::new(),
                    params: vec![RustParam {
                        name: Some("value".to_string()),
                        type_display: "demo::Kind".to_string(),
                    }],
                    return_type: "()".to_string(),
                    is_async: false,
                    is_unsafe: false,
                }),
            },
        )
        .map_err(|e| std::io::Error::other(format!("seed rust-inspect accept_kind: {e}")))?;
    checker
        .rust_inspect_cache
        .insert_test_item(
            &manifest_dir,
            RustItemMetadata {
                canonical_path: "demo::accept_empty".to_string(),
                definition_path: Some("demo::accept_empty".to_string()),
                visibility: RustVisibility::Public,
                kind: RustItemKind::Function(RustFunctionSig {
                    receiver_contract: None,
                    type_params: Vec::new(),
                    params: vec![RustParam {
                        name: Some("value".to_string()),
                        type_display: "demo::Empty".to_string(),
                    }],
                    return_type: "()".to_string(),
                    is_async: false,
                    is_unsafe: false,
                }),
            },
        )
        .map_err(|e| std::io::Error::other(format!("seed rust-inspect accept_empty: {e}")))?;
    checker.check_program(&ast).map_err(|errs| {
        std::io::Error::other(format!(
            "expected imported Rust unit variants and zero-field constructors to typecheck: {errs:?}"
        ))
    })?;
    Ok(())
}

#[cfg(feature = "rust_inspect")]
#[test]
fn test_imported_rust_boxed_variant_payload_records_box_payload_coercion() -> Result<(), Box<dyn std::error::Error>> {
    // prost stores a recursive `oneof` payload as `Box<T>`; Incan records the payload as `T` and remembers the
    // carrier, so the source passes `T` and lowering must wrap it. The coercion recorded on the argument is that
    // contract (#1229 follow-up).
    let source = r#"
from rust::demo import Kind, accept_kind

def f() -> None:
  accept_kind(Kind.Tuple(1))
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
                    variants: vec![RustVariantInfo {
                        name: "Tuple".to_string(),
                        fields: vec![RustTypeShape::Int],
                        field_carriers: vec![incan_lang::interop::RustPayloadCarrier::Boxed],
                    }],
                }),
            },
        )
        .map_err(|e| std::io::Error::other(format!("seed rust-inspect kind: {e}")))?;
    checker
        .rust_inspect_cache
        .insert_test_item(
            &manifest_dir,
            RustItemMetadata {
                canonical_path: "demo::accept_kind".to_string(),
                definition_path: Some("demo::accept_kind".to_string()),
                visibility: RustVisibility::Public,
                kind: RustItemKind::Function(RustFunctionSig {
                    receiver_contract: None,
                    type_params: Vec::new(),
                    params: vec![RustParam {
                        name: Some("value".to_string()),
                        type_display: "demo::Kind".to_string(),
                    }],
                    return_type: "()".to_string(),
                    is_async: false,
                    is_unsafe: false,
                }),
            },
        )
        .map_err(|e| std::io::Error::other(format!("seed rust-inspect accept_kind: {e}")))?;
    checker
        .check_program(&ast)
        .map_err(|errs| std::io::Error::other(format!("expected the semantic payload to typecheck: {errs:?}")))?;
    let payload_start = source.find("Kind.Tuple(1)").ok_or("constructor missing from source")? + "Kind.Tuple(".len();
    let coercion = checker
        .type_info
        .rust
        .arg_coercions
        .get(&(payload_start, payload_start + 1))
        .ok_or("expected a coercion recorded on the boxed payload argument")?;
    assert_eq!(
        coercion.kind,
        crate::typechecker::type_info::RustArgCoercionKind::BoxPayload,
        "boxed variant payload must record the carrier lowering restores"
    );
    assert_eq!(coercion.target_type, ResolvedType::Int);
    Ok(())
}

#[cfg(feature = "rust_inspect")]
#[test]
fn test_imported_rust_field_assignment_resolves_through_rust_metadata() -> Result<(), Box<dyn std::error::Error>> {
    // `mut plan = empty_plan()` followed by `plan.relations = [...]` is how a test mutates a prost struct. The value
    // is typed by its Rust path, so the assignment must resolve the field through the same metadata a read uses,
    // reject a field the struct does not have, and hold the value to the field's type.
    let source = r#"
from rust::demo import Holder, Item
def f(holder: Holder, item: Item) -> None:
  mut current = holder
  current.items = [item]

def g(holder: Holder, item: Item) -> None:
  mut current = holder
  current.missing = [item]

def h(holder: Holder) -> None:
  mut current = holder
  current.items = 1
"#;
    let tokens = lexer::lex(source).map_err(|errs| std::io::Error::other(format!("lex failed: {errs:?}")))?;
    let ast = parser::parse(&tokens).map_err(|errs| std::io::Error::other(format!("parse failed: {errs:?}")))?;
    let mut checker = TypeChecker::new();
    let tmp = seeded_rust_inspect_workspace()?;
    let manifest_dir = tmp.path().to_path_buf();
    checker.set_rust_inspect_manifest_dir(manifest_dir.clone());
    for (path, fields) in [
        (
            "demo::Holder",
            vec![RustFieldInfo {
                name: "items".to_string(),
                type_display: "Vec<demo::Item>".to_string(),
                type_shape: RustTypeShape::RustPath {
                    path: "Vec".to_string(),
                    args: vec![RustTypeShape::RustPath {
                        path: "demo::Item".to_string(),
                        args: vec![],
                    }],
                },
            }],
        ),
        ("demo::Item", vec![]),
    ] {
        checker
            .rust_inspect_cache
            .insert_test_item(
                &manifest_dir,
                RustItemMetadata {
                    canonical_path: path.to_string(),
                    definition_path: Some(path.to_string()),
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
                        fields,
                        variants: vec![],
                    }),
                },
            )
            .map_err(|e| std::io::Error::other(format!("seed rust-inspect {path}: {e}")))?;
    }
    let messages = checker
        .check_program(&ast)
        .err()
        .unwrap_or_default()
        .iter()
        .map(|error| error.message.clone())
        .collect::<Vec<_>>();
    assert!(
        !messages.iter().any(|message| message.contains("has no field 'items'")),
        "assigning an existing Rust field must typecheck: {messages:?}"
    );
    assert!(
        messages
            .iter()
            .any(|message| message.contains("has no field 'missing'")),
        "assigning a field the Rust struct lacks must be rejected: {messages:?}"
    );
    assert!(
        messages
            .iter()
            .any(|message| message.contains("Cannot assign 'int' to field 'items' of type 'List[rust::demo::Item]'")),
        "assigning an int to a Vec field must be rejected with the field's type: {messages:?}"
    );
    assert_eq!(
        messages.len(),
        2,
        "only the two invalid assignments may be reported: {messages:?}"
    );
    Ok(())
}

#[test]
fn test_imported_rust_vec_field_indexes_with_element_type() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
from rust::demo import Holder, accept_item

def f(holder: Holder) -> None:
  accept_item(holder.items[0])
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
                canonical_path: "demo::Holder".to_string(),
                definition_path: Some("demo::Holder".to_string()),
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
                        name: "items".to_string(),
                        type_display: "Vec<demo::Item>".to_string(),
                        type_shape: RustTypeShape::RustPath {
                            path: "Vec".to_string(),
                            args: vec![RustTypeShape::RustPath {
                                path: "demo::Item".to_string(),
                                args: vec![],
                            }],
                        },
                    }],
                    variants: vec![],
                }),
            },
        )
        .map_err(|e| std::io::Error::other(format!("seed rust-inspect holder: {e}")))?;
    checker
        .rust_inspect_cache
        .insert_test_item(
            &manifest_dir,
            RustItemMetadata {
                canonical_path: "demo::Item".to_string(),
                definition_path: Some("demo::Item".to_string()),
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
        .map_err(|e| std::io::Error::other(format!("seed rust-inspect item: {e}")))?;
    checker
        .rust_inspect_cache
        .insert_test_item(
            &manifest_dir,
            RustItemMetadata {
                canonical_path: "demo::accept_item".to_string(),
                definition_path: Some("demo::accept_item".to_string()),
                visibility: RustVisibility::Public,
                kind: RustItemKind::Function(RustFunctionSig {
                    receiver_contract: None,
                    type_params: Vec::new(),
                    params: vec![RustParam {
                        name: Some("value".to_string()),
                        type_display: "demo::Item".to_string(),
                    }],
                    return_type: "()".to_string(),
                    is_async: false,
                    is_unsafe: false,
                }),
            },
        )
        .map_err(|e| std::io::Error::other(format!("seed rust-inspect accept_item: {e}")))?;
    checker.check_program(&ast).map_err(|errs| {
        std::io::Error::other(format!(
            "expected imported Rust Vec field indexing to preserve element type: {errs:?}"
        ))
    })?;
    Ok(())
}

#[test]
fn test_imported_rust_function_without_metadata_stays_permissive() {
    let source = r#"
from rust::std::fs import read_to_string

def f() -> None:
  _ = read_to_string("input.csv")
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_imported_rust_named_constructor_without_metadata_stays_permissive() {
    let source = r#"
from rust::std::ops import Range

def f() -> None:
  _ = Range(start=1, end=3)
"#;
    assert!(check_str(source).is_ok());
}

#[test]
fn test_imported_rust_named_constructor_records_field_boundary_coercion() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
from rust::demo import FunctionOption

const OPTION_NAME: str = "name"

def f() -> FunctionOption:
  return FunctionOption(name=OPTION_NAME)
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
                canonical_path: "demo::FunctionOption".to_string(),
                definition_path: Some("demo::FunctionOption".to_string()),
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
                        name: "name".to_string(),
                        type_display: "String".to_string(),
                        type_shape: RustTypeShape::Str,
                    }],
                    variants: vec![],
                }),
            },
        )
        .map_err(|err| std::io::Error::other(format!("seed rust-inspect FunctionOption: {err}")))?;

    checker
        .check_program(&ast)
        .map_err(|errs| std::io::Error::other(format!("check failed: {errs:?}")))?;
    let coercions: Vec<_> = checker.type_info().rust.arg_coercions.values().collect();
    assert_eq!(
        coercions.len(),
        1,
        "expected one field boundary coercion, got {coercions:?}"
    );
    assert_eq!(coercions[0].rust_target_type, "String");
    assert!(matches!(
        coercions[0].kind,
        RustArgCoercionKind::Builtin(CoercionPolicy::Exact)
    ));
    Ok(())
}

#[cfg(feature = "rust_inspect")]
#[test]
fn imported_rust_named_constructor_fills_omitted_fields_only_with_proven_default()
-> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
from rust::demo import DefaultRecord, RequiredRecord

def accepted() -> DefaultRecord:
  return DefaultRecord(name="kept")

def rejected() -> RequiredRecord:
  return RequiredRecord(name="kept")
"#;
    let tokens = lexer::lex(source).map_err(|errs| std::io::Error::other(format!("lex failed: {errs:?}")))?;
    let ast = parser::parse(&tokens).map_err(|errs| std::io::Error::other(format!("parse failed: {errs:?}")))?;
    let mut checker = TypeChecker::new();
    let tmp = seeded_rust_inspect_workspace()?;
    let manifest_dir = tmp.path().to_path_buf();
    checker.set_rust_inspect_manifest_dir(manifest_dir.clone());
    for (name, has_default) in [("DefaultRecord", true), ("RequiredRecord", false)] {
        checker.rust_inspect_cache.insert_test_item(
            &manifest_dir,
            RustItemMetadata {
                canonical_path: format!("demo::{name}"),
                definition_path: Some(format!("demo::{name}")),
                visibility: RustVisibility::Public,
                kind: RustItemKind::Type(RustTypeInfo {
                    type_params: Vec::new(),
                    type_param_defaults: Vec::new(),
                    mutable_reference_type_params: Vec::new(),
                    expanded_derive_traits: Vec::new(),
                    has_const_params: false,
                    alias_target: None,
                    metadata_completeness: Default::default(),
                    methods: Vec::new(),
                    implemented_traits: has_default
                        .then(|| RustImplementedTrait {
                            path: "core::default::Default".to_string(),
                            mutable_reference: false,
                        })
                        .into_iter()
                        .collect(),
                    fields: vec![
                        RustFieldInfo {
                            name: "name".to_string(),
                            type_display: "String".to_string(),
                            type_shape: RustTypeShape::Str,
                        },
                        RustFieldInfo {
                            name: "count".to_string(),
                            type_display: "u32".to_string(),
                            type_shape: RustTypeShape::Int,
                        },
                    ],
                    variants: Vec::new(),
                }),
            },
        )?;
    }

    let errors = checker
        .check_program(&ast)
        .expect_err("the non-Default Rust record must still reject an omitted field");
    assert_eq!(
        errors
            .iter()
            .filter(|error| error.message.contains("Missing required field 'count'"))
            .count(),
        1,
        "only the non-Default record should reject its omitted field: {errors:?}"
    );
    let call_start = source
        .find("DefaultRecord(name=\"kept\")")
        .ok_or_else(|| std::io::Error::other("expected DefaultRecord call"))?;
    let call_span = Span::new(call_start, call_start + "DefaultRecord(name=\"kept\")".len());
    assert!(
        checker
            .type_info()
            .rust_named_field_constructor_fills_defaults(call_span),
        "the metadata-proven Default constructor must retain its fill decision for lowering"
    );
    Ok(())
}

#[test]
fn test_imported_rust_named_constructor_resolves_bare_field_display_through_alias()
-> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
from rust::demo::outer import Container
from rust::demo::defs import Payload as DemoPayload

def f(payload: DemoPayload) -> Container:
  return Container(item=payload)
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
                canonical_path: "demo::outer::Container".to_string(),
                definition_path: Some("demo::outer::Container".to_string()),
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
                        name: "item".to_string(),
                        type_display: "Payload".to_string(),
                        type_shape: RustTypeShape::RustPath {
                            path: "Payload".to_string(),
                            args: vec![],
                        },
                    }],
                    variants: vec![],
                }),
            },
        )
        .map_err(|err| std::io::Error::other(format!("seed rust-inspect Container: {err}")))?;
    checker
        .rust_inspect_cache
        .insert_test_item(
            &manifest_dir,
            RustItemMetadata {
                canonical_path: "demo::defs::Payload".to_string(),
                definition_path: Some("demo::defs::Payload".to_string()),
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
        .map_err(|err| std::io::Error::other(format!("seed rust-inspect Payload: {err}")))?;

    checker
        .check_program(&ast)
        .map_err(|errs| std::io::Error::other(format!("check failed: {errs:?}")))?;
    Ok(())
}

/// A dependency that re-exports the `alloc` crate lets generated code spell `::carrier::alloc::boxed::Box<T>`.
///
/// That path is absolute. Joining it onto the owning module recorded a field type no consumer could name
/// (`demo::proto::nested::carrier::alloc::boxed::Box`), which is the `Box` mismatch of incan#1229 and the reason
/// rust-inspect prewarm had to stay disabled. This harness runs the source-level extractor, so it proves the property
/// that matters — an absolute path is never joined onto the owning module — but not the HIR-only steps (following
/// `carrier::alloc` to the `alloc` crate, stripping the `Box` carrier from a variant payload). Those are exercised by
/// baking a real project against the semantic extractor.
#[cfg(feature = "rust_inspect")]
#[test]
fn rust_inspect_records_absolute_reexport_paths_in_the_ancestral_namespace() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let root = tmp.path();
    for dir in ["src", "demo/src", "carrier/src"] {
        fs::create_dir_all(root.join(dir))?;
    }
    fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"ra_reexport_probe\"\nversion = \"0.1.0\"\nedition = \"2021\"\n[dependencies]\ndemo = { path = \"demo\" }\n",
    )?;
    fs::write(
        root.join("src/lib.rs"),
        "pub fn touch() { let _ = demo::proto::Inner; }\n",
    )?;
    fs::write(
        root.join("demo/Cargo.toml"),
        "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n[dependencies]\ncarrier = { path = \"../carrier\" }\n",
    )?;
    fs::write(
        root.join("demo/src/lib.rs"),
        "extern crate alloc;\npub mod proto {\n    pub struct Inner;\n    pub mod nested {\n        pub enum Wrap {\n            Boxed(::carrier::alloc::boxed::Box<super::Inner>),\n            Direct(::alloc::boxed::Box<super::Inner>),\n            Text(::std::string::String),\n        }\n    }\n}\n",
    )?;
    fs::write(
        root.join("carrier/Cargo.toml"),
        "[package]\nname = \"carrier\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )?;
    fs::write(root.join("carrier/src/lib.rs"), "pub extern crate alloc;\n")?;
    prewarm_metadata(root, &["demo::proto::nested::Wrap"])?;
    let inspector = Inspector::new(InspectorConfig::new(root.to_path_buf()));
    let result = inspector.get("demo::proto::nested::Wrap")?;
    let incan_lang::interop::RustItemKind::Type(info) = &result.metadata.kind else {
        return Err("expected enum type metadata for demo::proto::nested::Wrap".into());
    };
    let rendered = |name: &str| -> Result<Vec<String>, Box<dyn std::error::Error>> {
        Ok(info
            .variants
            .iter()
            .find(|variant| variant.name == name)
            .ok_or_else(|| format!("missing {name} variant"))?
            .fields
            .iter()
            .map(incan_lang::interop::render_rust_type_shape)
            .collect())
    };
    let (boxed, direct, text) = (rendered("Boxed")?, rendered("Direct")?, rendered("Text")?);
    eprintln!("PROBE Boxed={boxed:?} Direct={direct:?} Text={text:?}");
    for (name, fields) in [("Boxed", &boxed), ("Direct", &direct), ("Text", &text)] {
        assert!(
            fields.iter().all(|field| !field.contains("nested::")),
            "{name}: an absolute path must never be joined onto the owning module, got {fields:?}"
        );
    }
    // Both boxed spellings are the one `alloc::boxed::Box` item: the payload is recorded as its semantic type and
    // the carrier beside it, whether the source reached `Box` directly or through another crate's re-export.
    let carriers = |name: &str| -> Result<Vec<incan_lang::interop::RustPayloadCarrier>, Box<dyn std::error::Error>> {
        Ok(info
            .variants
            .iter()
            .find(|variant| variant.name == name)
            .ok_or_else(|| format!("missing {name} variant"))?
            .field_carriers
            .clone())
    };
    assert_eq!(
        direct,
        vec!["demo::proto::Inner".to_string()],
        "a direct absolute std path records the semantic payload in the ancestral namespace"
    );
    assert_eq!(
        boxed,
        vec!["demo::proto::Inner".to_string()],
        "a re-exported absolute path is the same carrier"
    );
    assert_eq!(
        carriers("Direct")?,
        vec![incan_lang::interop::RustPayloadCarrier::Boxed]
    );
    assert_eq!(carriers("Boxed")?, vec![incan_lang::interop::RustPayloadCarrier::Boxed]);
    assert_eq!(text, vec!["String".to_string()]);
    assert_eq!(carriers("Text")?, vec![incan_lang::interop::RustPayloadCarrier::Direct]);
    Ok(())
}

/// `Some(Box.new(v))` against a Rust field typed `Option<Box<T>>` must type-check.
///
/// Metadata records the field in the ancestral namespace (`core::option::Option<alloc::boxed::Box<T>>`), the source
/// imports `std::boxed::Box`, and `Box::new` returns `Self`. Reconciling that `Self` with `Box<T>` needs the expected
/// inner type to reach the call inside `Some(...)`, and the owner match to happen in the ancestral namespace.
#[cfg(feature = "rust_inspect")]
#[test]
fn rust_struct_field_option_box_accepts_boxed_some_argument() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
from rust::demo import Holder, Item
from rust::std::boxed import Box
def f() -> None:
  _ = Holder(inner=Some(Box.new(Item.new())))
"#;
    let tokens = lexer::lex(source).map_err(|errs| std::io::Error::other(format!("lex failed: {errs:?}")))?;
    let ast = parser::parse(&tokens).map_err(|errs| std::io::Error::other(format!("parse failed: {errs:?}")))?;
    let tmp = tempfile::tempdir()?;
    let root = tmp.path();
    fs::create_dir_all(root.join("src"))?;
    fs::create_dir_all(root.join("demo/src"))?;
    fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"ra_option_box_probe\"\nversion = \"0.1.0\"\nedition = \"2021\"\n[dependencies]\ndemo = { path = \"demo\" }\n",
    )?;
    fs::write(
        root.join("src/lib.rs"),
        "pub fn touch() { let _ = demo::Item::new(); }\n",
    )?;
    fs::write(
        root.join("demo/Cargo.toml"),
        "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )?;
    fs::write(
        root.join("demo/src/lib.rs"),
        "extern crate alloc;\npub struct Item;\nimpl Item {\n    pub fn new() -> Self {\n        Item\n    }\n}\npub struct Holder {\n    pub inner: ::core::option::Option<::alloc::boxed::Box<Item>>,\n}\n",
    )?;
    let mut checker = TypeChecker::new();
    checker.set_rust_inspect_manifest_dir(root.to_path_buf());
    prewarm_metadata(
        root,
        &[
            "demo::Holder",
            "demo::Item",
            "demo::Item::new",
            "std::boxed::Box",
            "std::boxed::Box::new",
        ],
    )?;
    if let Err(errs) = checker.check_program(&ast) {
        panic!("expected Some(Box.new(..)) to satisfy Option<Box<T>>, got {errs:?}");
    }
    Ok(())
}
