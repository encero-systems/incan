//! RFC 041 `rust::` imports and `rusttype` declarations: crate roots, `core` / `alloc` rejection, constants, metadata
//! lookup paths, display-string resolution of Rust types and borrows, interop adapters, rusttype-backed enum matching,
//! structural coercions, and rusttype return coercions.

use super::*;

#[test]
fn metadata_free_path_new_records_a_borrowed_argument_boundary() -> Result<(), String> {
    let source = r#"
from rust::std::path import Path

def main() -> None:
  path = "example.txt"
  _ = Path.new(path)
"#;
    let ast = parse_program(source, "metadata-free Path.new borrow boundary");
    let mut checker = TypeChecker::new();
    checker
        .check_program(&ast)
        .map_err(|errors| format!("Path.new should typecheck: {errors:?}"))?;

    let params = checker
        .type_info()
        .calls
        .call_site_callable_params
        .values()
        .find(|params| params.len() == 1)
        .ok_or("Path.new call should record one callable parameter")?;
    assert!(
        matches!(params[0].ty, ResolvedType::Ref(_)),
        "Path.new should retain a borrowed Rust parameter, got {:?}",
        params[0].ty
    );
    Ok(())
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
        info.expressions.expr_types.values().any(|t| {
            matches!(
                t,
                ResolvedType::RustPath(p) if p == "std::time::Instant"
            )
        }),
        "expected RustPath(std::time::Instant) in expr types, got {:?}",
        info.expressions.expr_types
    );
    Ok(())
}

#[test]
fn test_rust_from_import_shadows_dependency_type_for_rust_display_name() -> Result<(), Box<dyn std::error::Error>> {
    let dep_source = r#"
pub model Duration:
  pub value: int
"#;
    let source = r#"
from rust::std::time import Duration

def f() -> None:
  pass
"#;
    let dep_tokens =
        lexer::lex(dep_source).map_err(|errs| std::io::Error::other(format!("lex dep failed: {errs:?}")))?;
    let dep_ast =
        parser::parse(&dep_tokens).map_err(|errs| std::io::Error::other(format!("parse dep failed: {errs:?}")))?;
    let tokens = lexer::lex(source).map_err(|errs| std::io::Error::other(format!("lex failed: {errs:?}")))?;
    let ast = parser::parse(&tokens).map_err(|errs| std::io::Error::other(format!("parse failed: {errs:?}")))?;
    let mut checker = TypeChecker::new();
    checker
        .check_with_imports(&ast, &[("dep", &dep_ast)])
        .map_err(|errs| std::io::Error::other(format!("check_program failed: {errs:?}")))?;

    let symbol = checker
        .lookup_symbol("Duration")
        .ok_or_else(|| std::io::Error::other("Duration import was not recorded"))?;
    let SymbolKind::RustItem(info) = &symbol.kind else {
        return Err(std::io::Error::other(format!("expected RustItem, got {:?}", symbol.kind)).into());
    };
    assert_eq!(info.path, "std::time::Duration");
    assert_eq!(
        checker.resolved_type_from_rust_display("Duration"),
        ResolvedType::RustPath("std::time::Duration".to_string())
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
    assert_eq!(checker.resolved_type_from_rust_display("&'h str"), ResolvedType::Str);
    assert_eq!(checker.resolved_type_from_rust_display("&'h [u8]"), ResolvedType::Bytes);
}

#[test]
fn test_resolved_type_from_structured_rust_string_shapes_is_str() {
    let checker = TypeChecker::new();
    for path in ["String", "std::string::String", "alloc::string::String"] {
        assert_eq!(
            checker.resolved_type_from_rust_shape(&RustTypeShape::RustPath {
                path: path.to_string(),
                args: vec![],
            }),
            ResolvedType::Str,
            "structured Rust metadata path {path} must retain Incan string semantics"
        );
    }

    assert_eq!(
        checker.resolved_type_from_rust_shape(&RustTypeShape::RustPath {
            path: "std::string::String".to_string(),
            args: vec![RustTypeShape::RustPath {
                path: "alloc::alloc::Global".to_string(),
                args: vec![],
            }],
        }),
        ResolvedType::Str,
        "a canonical String path must retain string semantics when rustc exposes its allocator argument"
    );
}

#[test]
fn test_resolved_type_from_generic_rust_string_display_is_str() {
    let checker = TypeChecker::new();

    assert_eq!(
        checker.resolved_type_from_rust_display("std::string::String<alloc::alloc::Global>"),
        ResolvedType::Str,
        "an implementation allocator argument must not erase canonical Rust String semantics"
    );
}

#[test]
fn test_noncanonical_rust_string_like_shapes_remain_external() {
    let checker = TypeChecker::new();

    for display in ["demo::String", "StringBuilder", "demo::String<alloc::alloc::Global>"] {
        assert_eq!(
            checker.resolved_type_from_rust_display(display),
            ResolvedType::RustPath(display.to_string()),
            "noncanonical Rust type {display} must not acquire Incan string semantics"
        );
    }
    assert_eq!(
        checker.resolved_type_from_rust_shape(&RustTypeShape::RustPath {
            path: "demo::String".to_string(),
            args: vec![],
        }),
        ResolvedType::RustPath("demo::String".to_string())
    );
}

#[test]
fn test_resolved_param_type_from_builtin_borrowed_displays_preserves_ref_payload() {
    let checker = TypeChecker::new();
    assert_eq!(
        checker.resolved_param_type_from_rust_display("&str"),
        ResolvedType::Ref(Box::new(ResolvedType::Str)),
    );
    assert_eq!(
        checker.resolved_param_type_from_rust_display("&[u8]"),
        ResolvedType::Ref(Box::new(ResolvedType::Bytes)),
    );
    assert_eq!(
        checker.resolved_param_type_from_rust_display("&'h str"),
        ResolvedType::Ref(Box::new(ResolvedType::Str)),
    );
    assert_eq!(
        checker.resolved_param_type_from_rust_display("&'h [u8]"),
        ResolvedType::Ref(Box::new(ResolvedType::Bytes)),
    );
    assert_eq!(
        checker.resolved_param_type_from_rust_display("&[demo::ColumnarValue]"),
        ResolvedType::Ref(Box::new(ResolvedType::Generic(
            "List".to_string(),
            vec![ResolvedType::RustPath("demo::ColumnarValue".to_string())]
        ))),
    );
    assert_eq!(
        checker.resolved_param_type_from_rust_display("&'h mut demo::Thing"),
        ResolvedType::RefMut(Box::new(ResolvedType::RustPath("demo::Thing".to_string()))),
    );
}

#[test]
fn test_resolved_param_type_from_rust_callable_bound_preserves_callback_borrows() {
    let checker = TypeChecker::new();
    assert_eq!(
        checker.resolved_param_type_from_rust_display(
            "impl FnMut(&mut demo::Data, &demo::OutputCallbackInfo) -> () + Send + 'static",
        ),
        ResolvedType::Function(
            vec![
                CallableParam::positional(ResolvedType::RefMut(Box::new(ResolvedType::RustPath(
                    "demo::Data".to_string()
                )))),
                CallableParam::positional(ResolvedType::Ref(Box::new(ResolvedType::RustPath(
                    "demo::OutputCallbackInfo".to_string()
                )))),
            ],
            Box::new(ResolvedType::Unit),
        )
    );
}

#[test]
fn test_rust_owner_path_expands_crate_relative_signature_displays() {
    let checker = TypeChecker::new();
    assert_eq!(
        checker.rust_display_for_owner_path(
            "Arc<dyn Fn(&[crate::ColumnarValue]) -> crate::Result<crate::ColumnarValue> + Send + Sync>",
            "demo_runtime::create_udf",
        ),
        "Arc<dyn Fn(&[demo_runtime::ColumnarValue]) -> demo_runtime::Result<demo_runtime::ColumnarValue> + Send + Sync>",
    );
    assert_eq!(
        checker.resolved_param_type_from_rust_display_for_owner_path(
            "crate::ScalarFunctionImplementation",
            "demo_runtime::create_udf",
        ),
        ResolvedType::RustPath("demo_runtime::ScalarFunctionImplementation".to_string()),
    );
    assert_eq!(
        checker.rust_display_for_owner_path(
            "impl FnMut(&mut crate::[f32], &crate::OutputCallbackInfo)",
            "demo_runtime::run",
        ),
        "impl FnMut(&mut [f32], &demo_runtime::OutputCallbackInfo)",
    );
    assert_eq!(
        checker.rust_display_for_owner_path("&mut super::Header", "demo_runtime::writer::Writer.mutate",),
        "&mut demo_runtime::Header",
    );
    assert_eq!(
        checker.rust_display_for_owner_path("self::Nested", "demo_runtime::writer::Writer.new",),
        "demo_runtime::writer::Nested",
    );
}

#[test]
fn test_rust_never_return_is_bottom_compatible_issue381() {
    let checker = TypeChecker::new();
    let signature = RustFunctionSig {
        receiver_contract: None,
        type_params: Vec::new(),
        params: Vec::new(),
        return_type: "!".to_string(),
        is_async: false,
        is_unsafe: false,
    };

    assert_eq!(checker.resolved_type_from_rust_display("!"), ResolvedType::Never);
    assert_eq!(
        checker.resolved_function_type_from_rust_sig_for_owner_path(&signature, false, "demo::errors::fail"),
        ResolvedType::Function(Vec::new(), Box::new(ResolvedType::Never)),
    );
    assert!(checker.types_compatible(&ResolvedType::Never, &ResolvedType::Numeric(NumericTypeId::I32)));
    assert!(!checker.types_compatible(&ResolvedType::Numeric(NumericTypeId::I32), &ResolvedType::Never));
}

#[test]
fn test_resolved_param_type_from_structural_borrowed_display_preserves_nested_ref_payload() {
    let checker = TypeChecker::new();
    assert_eq!(
        checker.resolved_param_type_from_rust_display("Vec<&str>"),
        ResolvedType::Generic("List".to_string(), vec![ResolvedType::Ref(Box::new(ResolvedType::Str))]),
    );
    assert_eq!(
        checker.resolved_rust_boundary_target_from_param_display("Vec<&String>"),
        ResolvedType::Generic(
            "List".to_string(),
            vec![ResolvedType::Ref(Box::new(ResolvedType::RustPath(
                "String".to_string()
            )))]
        ),
    );
}

#[test]
fn test_resolved_param_type_does_not_treat_mut_prefix_as_mutable_borrow_keyword() {
    let checker = TypeChecker::new();
    assert_eq!(
        checker.resolved_param_type_from_rust_display("&mutability::Foo"),
        ResolvedType::Ref(Box::new(ResolvedType::RustPath("mutability::Foo".to_string()))),
    );
    assert_eq!(
        checker.resolved_param_type_from_rust_display("&mut mutability::Foo"),
        ResolvedType::RefMut(Box::new(ResolvedType::RustPath("mutability::Foo".to_string()))),
    );
}

#[test]
fn test_resolved_result_display_splits_only_top_level_generic_commas() {
    let checker = TypeChecker::new();
    assert_eq!(
        checker.resolved_type_from_rust_display("Result<Vec<(i32, i32)>, String>"),
        ResolvedType::Generic(
            "Result".to_string(),
            vec![
                ResolvedType::Generic(
                    "List".to_string(),
                    vec![ResolvedType::RustPath("(i32,i32)".to_string())]
                ),
                ResolvedType::Str,
            ],
        ),
    );
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
fn test_types_compatible_function_params_require_exact_borrow_shape() {
    let checker = TypeChecker::new();
    let data = ResolvedType::RustPath("demo::Data".to_string());
    let info = ResolvedType::RustPath("demo::OutputCallbackInfo".to_string());
    let by_value_callback = ResolvedType::Function(
        vec![
            CallableParam::positional(data.clone()),
            CallableParam::positional(info.clone()),
        ],
        Box::new(ResolvedType::Unit),
    );
    let borrowed_callback = ResolvedType::Function(
        vec![
            CallableParam::positional(ResolvedType::RefMut(Box::new(data))),
            CallableParam::positional(ResolvedType::Ref(Box::new(info))),
        ],
        Box::new(ResolvedType::Unit),
    );

    assert!(
        !checker.types_compatible(&by_value_callback, &borrowed_callback),
        "by-value function parameters must not satisfy borrowed Rust callback parameters"
    );
    assert!(
        checker.types_compatible(&borrowed_callback, &borrowed_callback),
        "exact borrowed callback parameter shape should remain compatible"
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

#[test]
fn test_rust_metadata_lookup_path_strips_outer_generic_instantiation() {
    assert_eq!(
        TypeChecker::rust_metadata_lookup_path("incan_std_async::channel::SendError<T>"),
        Some("incan_std_async::channel::SendError")
    );
    assert_eq!(
        TypeChecker::rust_metadata_lookup_path("Result<(),incan_std_async::channel::SendError<T>>"),
        None
    );
}

#[test]
fn test_rust_metadata_lookup_path_rejects_unknown_placeholder() {
    assert_eq!(TypeChecker::rust_metadata_lookup_path("{unknown}"), None);
}

#[test]
fn test_rust_display_unknown_placeholder_resolves_unknown() {
    let checker = TypeChecker::new();
    assert_eq!(
        checker.resolved_type_from_rust_display("{unknown}"),
        ResolvedType::Unknown
    );
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

/// #1716: an `Fn`-family marker names the callable's parameter list. A function value whose parameters match is
/// admitted whatever it returns (the return type is the marker's free part); one whose arity or parameter types differ
/// is refused at the call site rather than by rustc. A non-function value is not judged by the marker.
#[test]
fn test_std_rust_fn_capability_bound_checks_the_parameter_list() {
    let accepted = r#"
from std.rust import Fn, FnMut, FnOnce

def run_fn[F with Fn[int]](_f: F) -> None:
  pass

def run_fn_mut[F with FnMut[int, str]](_f: F) -> None:
  pass

def run_fn_once[F with FnOnce[int]](_f: F) -> None:
  pass

def double(value: int) -> int:
  return value * 2

def label(value: int, name: str) -> str:
  return f"{name}:{value}"

def shout(value: int) -> None:
  println(value)

def main() -> None:
  run_fn(double)
  run_fn_mut(label)
  run_fn_once(shout)
"#;
    assert_check_ok(accepted);

    let wrong_arity = r#"
from std.rust import Fn

def run_fn[F with Fn[int]](_f: F) -> None:
  pass

def label(value: int, name: str) -> str:
  return f"{name}:{value}"

def main() -> None:
  run_fn(label)
"#;
    let errs = check_str_err(wrong_arity, "a two-parameter function must not satisfy Fn[int]");
    assert!(
        errs.iter()
            .any(|error| error.message.contains("violates generic bound")),
        "expected a generic-bound violation for the arity mismatch; got: {errs:?}"
    );

    let wrong_parameter_type = r#"
from std.rust import Fn

def run_fn[F with Fn[int]](_f: F) -> None:
  pass

def greet(name: str) -> str:
  return f"hello {name}"

def main() -> None:
  run_fn(greet)
"#;
    let errs = check_str_err(wrong_parameter_type, "a str-taking function must not satisfy Fn[int]");
    assert!(
        errs.iter()
            .any(|error| error.message.contains("violates generic bound")),
        "expected a generic-bound violation for the parameter type mismatch; got: {errs:?}"
    );
}

/// #1716: an `Fn`-family marker lowers to a callable bound by its parameter count, and the callable vocabulary stops
/// at two parameters. A marker naming more is refused at its declaration with `INCAN-T0106` and the limit in the
/// message, whether or not anything is ever passed to it; a marker at the limit is still accepted.
#[test]
fn test_std_rust_fn_capability_marker_refuses_more_than_two_parameters() {
    let at_the_limit = r#"
from std.rust import Fn, FnMut

def run_pair[F with Fn[int, str]](_f: F) -> None:
  pass

def run_pair_mut[F with FnMut[int, str]](_f: F) -> None:
  pass
"#;
    assert_check_ok(at_the_limit);

    let over_the_limit = r#"
from std.rust import Fn

def run_triple[F with Fn[int, int, int]](_f: F) -> None:
  pass
"#;
    let errs = check_str_err(
        over_the_limit,
        "a three-parameter Fn marker must be refused at its declaration",
    );
    let refusal = errs
        .iter()
        .find(|error| error.stable_code() == Some("INCAN-T0106"))
        .unwrap_or_else(|| panic!("expected an INCAN-T0106 refusal; got: {errs:?}"));
    assert!(
        refusal
            .message
            .contains("Callable marker 'Fn[int, int, int]' on 'F' names 3 parameters; a marker takes at most 2"),
        "the message names the marker, the parameter and the limit; got: {}",
        refusal.message
    );
    assert!(
        refusal
            .hints
            .iter()
            .any(|hint| hint.contains("gather the parameters into one model and write 'Fn[ThatModel]'")),
        "the hint says what to write instead; got: {:?}",
        refusal.hints
    );

    let method_over_the_limit = r#"
from std.rust import FnOnce

class Runner:
  count: int

  def run[F with FnOnce[int, int, int]](self, _f: F) -> None:
    pass
"#;
    let errs = check_str_err(
        method_over_the_limit,
        "a three-parameter marker on a method must be refused",
    );
    assert!(
        errs.iter().any(|error| error.stable_code() == Some("INCAN-T0106")),
        "expected an INCAN-T0106 refusal on the method's type parameter; got: {errs:?}"
    );
}

/// #1716: a marker leaves its return type to the call that passes the value, and only a function or method has such
/// a call. On a nominal declaration's type parameter the marker is refused with `INCAN-T0106`, naming the callable
/// trait that spells the return type; the same marker on a method of that declaration stays accepted.
#[test]
fn test_std_rust_fn_capability_marker_refused_on_nominal_type_parameters() {
    let nominal_owners = [
        (
            "model",
            r#"
from std.rust import Fn

model Holder[F with Fn[int]]:
  callback: F
"#,
        ),
        (
            "class",
            r#"
from std.rust import FnMut

class Runner[F with FnMut[int, str]]:
  callback: F
"#,
        ),
        (
            "enum",
            r#"
from std.rust import FnOnce

enum Step[F with FnOnce[int]]:
  Run(F)
  Skip
"#,
        ),
        (
            "trait",
            r#"
from std.rust import Fn

trait Applies[F with Fn[int]]:
  def apply(self, f: F) -> None: ...
"#,
        ),
    ];
    for (owner_kind, source) in nominal_owners {
        let errs = check_str_err(source, "a callable marker on a nominal type parameter must be refused");
        let refusal = errs
            .iter()
            .find(|error| error.stable_code() == Some("INCAN-T0106"))
            .unwrap_or_else(|| panic!("expected an INCAN-T0106 refusal on the {owner_kind}; got: {errs:?}"));
        assert!(
            refusal.message.contains("cannot bound type parameter 'F' of ") && refusal.message.contains(owner_kind),
            "the message names the declaration kind; got: {}",
            refusal.message
        );
        assert!(
            refusal
                .hints
                .iter()
                .any(|hint| hint.contains("from std.traits.callable")),
            "the hint names the callable trait to write instead; got: {:?}",
            refusal.hints
        );
    }

    let model_hint = check_str_err(nominal_owners[0].1, "the model program must be refused")
        .into_iter()
        .find(|error| error.stable_code() == Some("INCAN-T0106"))
        .map(|error| error.hints)
        .unwrap_or_default();
    assert!(
        model_hint
            .iter()
            .any(|hint| hint.contains("'Callable1[int, R]'") && hint.contains("'(int) -> R'")),
        "the hint spells the one-parameter callable trait and the function type; got: {model_hint:?}"
    );

    let method_on_nominal = r#"
from std.rust import Fn

model Holder:
  value: int

  def apply[F with Fn[int]](self, _f: F) -> None:
    pass
"#;
    assert_check_ok(method_on_nominal);
}

#[test]
fn test_structural_coercion_option_int_to_option_i64() {
    let checker = TypeChecker::new();
    let arg_ty = crate::symbols::ResolvedType::Generic("Option".to_string(), vec![crate::symbols::ResolvedType::Int]);
    assert!(
        checker.rust_arg_matches_boundary(&arg_ty, "Option<i64>"),
        "expected Option[int] to be admitted at Option<i64> Rust boundary"
    );
}

#[test]
fn test_structural_coercion_list_str_to_vec_string() {
    let checker = TypeChecker::new();
    let arg_ty = crate::symbols::ResolvedType::Generic("List".to_string(), vec![crate::symbols::ResolvedType::Str]);
    assert!(
        checker.rust_arg_matches_boundary(&arg_ty, "Vec<String>"),
        "expected List[str] to be admitted at Vec<String> Rust boundary"
    );
}

/// `maybe_record_rusttype_return_coercion` is metadata-driven; without the `rust-inspect` feature the cache is empty
/// and the helper is a no-op. The test below exercises the *non-metadata* path to assert that the coercion map stays
/// empty (no false positives).
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
    // The wrapper's parameter must be stored by the underlying Rust type (#1370 refuses an unused one), so the
    // probe wraps a generic Rust type whose only method returns a borrowed `&str`.
    let source = r#"
from rust::std::vec import Vec as RustVec

type Label[T] = rusttype RustVec[T]:
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
                canonical_path: "std::vec::Vec".to_string(),
                definition_path: Some("std::vec::Vec".to_string()),
                visibility: RustVisibility::Public,
                kind: RustItemKind::Type(RustTypeInfo {
                    type_params: vec!["T".to_string()],
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
        std::io::Error::other(format!("expected generic rusttype method call to typecheck: {errs:?}"))
    })?;
    let info = checker.type_info();
    assert!(
        info.rust
            .return_coercions
            .values()
            .any(|c| c.rust_target_type == "String" && matches!(c.target_type, ResolvedType::Str)),
        "expected rust return coercion (&str -> String) for generic rusttype method call, got {:?}",
        info.rust.return_coercions
    );
    Ok(())
}

#[test]
fn test_structural_coercion_mismatch_is_rejected() {
    let checker = TypeChecker::new();
    let arg_ty = crate::symbols::ResolvedType::Generic("List".to_string(), vec![crate::symbols::ResolvedType::Str]);
    assert!(
        !checker.rust_arg_matches_boundary(&arg_ty, "Vec<i64>"),
        "expected List[str] -> Vec<i64> structural coercion mismatch to be rejected"
    );
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
fn test_rusttype_explicit_trait_adoption_typechecks() {
    let source = r#"
from rust::ids import UserId as RustUserId

trait Labelled:
  def label(self) -> str: ...

type UserId = rusttype RustUserId with Labelled:
  def label(self) -> str:
    return "user"
"#;
    assert_check_ok(source);
}

#[test]
fn test_rusttype_bodyless_rust_trait_forwarding_requires_metadata() {
    let source = r#"
from rust::ids import UserId as RustUserId, Labelled

type UserId = rusttype RustUserId with Labelled
"#;
    let errs = check_str_err(source, "rusttype Rust trait forwarding should require metadata proof");
    assert!(
        errs.iter().any(|err| err
            .message
            .contains("Cannot forward Rust trait `ids::Labelled` for rusttype `UserId` without metadata proof")),
        "expected rusttype forwarding metadata diagnostic, got {errs:?}"
    );
}

#[test]
fn test_rusttype_awaitable_future_bridge_is_explicitly_blocked() {
    let source = r#"
from rust::async_host import JoinHandle as RustJoinHandle

trait Awaitable[T]:
  def poll(self) -> T: ...

type JoinHandle[T] = rusttype RustJoinHandle[T] with Awaitable[T]
"#;
    let errs = check_str_err(source, "rusttype Awaitable bridge should be gated");
    assert!(
        errs.iter().any(|err| err
            .message
            .contains("`Awaitable[T]` to Rust `Future` bridging is not implemented")),
        "expected Awaitable/Future bridge blocker diagnostic, got {errs:?}"
    );
}

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
