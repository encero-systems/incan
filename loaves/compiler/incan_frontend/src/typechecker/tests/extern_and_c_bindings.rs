//! Foreign function boundaries: RFC 023 `rust.module()` and `@rust.extern`, `@rust.allow` lint attributes, and checked
//! C bindings (scalar and resource descriptors, `unsafe` acknowledgement, raw-call bridges, ownership transfers).

use super::*;

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
fn test_rust_allow_accepts_targeted_lints() {
    let source = r#"
@rust.allow("dead_code", "clippy::too_many_arguments")
model RustAllowed:
  value: int

@rust.allow("non_snake_case")
def MixedName() -> int:
  return 1

@rust.allow("non_camel_case_types")
type rust_allowed_newtype = newtype int
"#;
    assert_check_ok(source);
}

#[test]
fn test_rust_allow_rejects_invalid_arguments() {
    let cases = [
        (
            r#"
@rust.allow()
def missing() -> None:
  pass
"#,
            "@rust.allow requires one or more positional string literal arguments",
        ),
        (
            r#"
@rust.allow(name = "dead_code")
def named() -> None:
  pass
"#,
            "@rust.allow does not accept named argument 'name'",
        ),
        (
            r#"
@rust.allow(dead_code)
def non_string() -> None:
  pass
"#,
            "@rust.allow requires one or more positional string literal arguments",
        ),
        (
            r#"
@rust.allow("")
def empty() -> None:
  pass
"#,
            "Invalid Rust lint name ''",
        ),
        (
            r#"
@rust.allow(" dead_code")
def padded() -> None:
  pass
"#,
            "Invalid Rust lint name ' dead_code'",
        ),
        (
            r#"
@rust.allow("dead_code", "dead_code")
def duplicate() -> None:
  pass
"#,
            "Duplicate Rust lint 'dead_code' in @rust.allow",
        ),
        (
            r#"
@rust.allow("warnings")
def broad_warnings() -> None:
  pass
"#,
            "Broad Rust lint group 'warnings' is not allowed in @rust.allow",
        ),
        (
            r#"
@rust.allow("unused")
def broad_unused() -> None:
  pass
"#,
            "Broad Rust lint group 'unused' is not allowed in @rust.allow",
        ),
        (
            r#"
@rust.allow("clippy::all")
def broad_clippy_all() -> None:
  pass
"#,
            "Broad Rust lint group 'clippy::all' is not allowed in @rust.allow",
        ),
        (
            r#"
@rust.allow("clippy::pedantic")
def broad_clippy_pedantic() -> None:
  pass
"#,
            "Broad Rust lint group 'clippy::pedantic' is not allowed in @rust.allow",
        ),
        (
            r#"
@rust.allow("clippy::nursery")
def broad_clippy_nursery() -> None:
  pass
"#,
            "Broad Rust lint group 'clippy::nursery' is not allowed in @rust.allow",
        ),
        (
            r#"
@rust.allow("clippy::restriction")
def broad_clippy_restriction() -> None:
  pass
"#,
            "Broad Rust lint group 'clippy::restriction' is not allowed in @rust.allow",
        ),
        (
            r#"
@rust.allow("clippy::cargo")
def broad_clippy_cargo() -> None:
  pass
"#,
            "Broad Rust lint group 'clippy::cargo' is not allowed in @rust.allow",
        ),
        (
            r#"
@rust.allow("dead_code")
trait NotConcrete:
  def value(self) -> int: ...
"#,
            "@rust.allow cannot be used on trait declarations",
        ),
    ];

    for (source, expected) in cases {
        let errors = check_str_err(source, "invalid @rust.allow should fail typechecking");
        assert!(
            errors.iter().any(|err| err.message.contains(expected)),
            "expected diagnostic containing {expected:?}, got {errors:?}"
        );
    }
}

#[test]
fn test_rust_module_with_rust_extern_ok() {
    let source = r#"
rust.module("incan_std_testing")

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
rust.module("incan_std_testing")

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
rust.module("incan_std_testing")

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
rust.module("incan_std_web")

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
rust.module("incan_std_core::utils")

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
    // incan_std_core is always allowed even without a manifest.
    let source = r#"
rust.module("incan_std_testing")

@rust.extern
def fail(msg: str) -> None:
    ...
"#;
    let tokens = lexer::lex(source)?;
    let ast = parser::parse(&tokens)?;
    let mut tc = TypeChecker::new();
    tc.set_declared_crate_names(std::collections::HashSet::new());
    let result = tc.check_program(&ast);
    assert!(result.is_ok(), "incan_std_core should always be allowed");
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

fn scalar_c_binding_descriptor() -> CBindingDescriptor {
    CBindingDescriptor {
        span: Span::default(),
        class_name: "Fixture".to_string(),
        header: "fixture.h".to_string(),
        system_library: "fixture".to_string(),
        link_capability: incan_lang::lang::c_abi::LinkCapabilityId::SystemLibrary,
        resources: Vec::new(),
        symbols: vec![CBindingSymbol {
            name: "add".to_string(),
            native: "fixture_add".to_string(),
            parameters: vec![CBindingParameter {
                name: "value".to_string(),
                ty: CBindingType::Scalar(ScalarTypeId::I32),
            }],
            return_type: CBindingType::Scalar(ScalarTypeId::I32),
            buffers: Vec::new(),
            outcomes: Vec::new(),
        }],
        enums: Vec::new(),
        structs: Vec::new(),
    }
}

#[test]
fn checked_c_binding_identity_is_relocation_stable_and_contract_sensitive() {
    let module_path = vec!["bindings".to_string(), "fixture".to_string()];
    let descriptor = scalar_c_binding_descriptor();
    let identity = c_binding_descriptor_identity(&module_path, &descriptor);

    let mut relocated = descriptor.clone();
    relocated.span = Span::new(4_096, 8_192);
    assert_eq!(
        c_binding_descriptor_identity(&module_path, &relocated),
        identity,
        "source spans and their machine-local locations must not affect a checked binding identity"
    );

    let mut changed_contract = descriptor;
    changed_contract.symbols[0].native = "fixture_add_v2".to_string();
    assert_ne!(
        c_binding_descriptor_identity(&module_path, &changed_contract),
        identity,
        "the descriptor identity must change when its native ABI contract changes"
    );
}

fn scalar_c_binding_call(span: Span) -> Spanned<Expr> {
    let binding = Spanned::new(Expr::Ident("Fixture".to_string()), span);
    Spanned::new(
        Expr::MethodCall(
            Box::new(binding),
            "add".to_string(),
            Vec::new(),
            vec![CallArg::Positional(Spanned::new(
                Expr::Literal(Literal::Int(IntLiteral::synthetic(1))),
                span,
            ))],
        ),
        span,
    )
}

#[test]
fn checked_c_scalar_call_requires_unsafe_and_records_the_descriptor_call() {
    let span = Span::new(1, 20);
    let mut checker = TypeChecker::new();
    let descriptor = scalar_c_binding_descriptor();
    checker
        .type_info
        .c_abi
        .bindings
        .insert(descriptor.class_name.clone(), descriptor);

    let result = checker.check_expr(&scalar_c_binding_call(span));
    assert_eq!(result, ResolvedType::Unknown);
    assert!(checker.errors.iter().any(|error| {
        error
            .message
            .contains("requires an enclosing `unsafe:` acknowledgement")
    }));
    assert!(checker.type_info.c_abi.raw_calls.is_empty());

    checker.errors.clear();
    checker.unsafe_depth = 1;
    let result = checker.check_expr(&scalar_c_binding_call(span));
    assert_eq!(result, ResolvedType::Numeric(NumericTypeId::I32));
    assert!(checker.errors.is_empty(), "unexpected errors: {:?}", checker.errors);
    assert_eq!(
        checker.type_info.c_abi.raw_calls,
        vec![super::type_info::CBindingRawCall {
            span,
            owner: None,
            binding: "Fixture".to_string(),
            symbol: "add".to_string(),
        }]
    );
}

#[test]
fn checked_c_raw_call_records_its_owning_private_bridge() -> Result<(), String> {
    let mut checker = TypeChecker::new();
    let descriptor = scalar_c_binding_descriptor();
    checker
        .type_info
        .c_abi
        .bindings
        .insert(descriptor.class_name.clone(), descriptor);
    checker.unsafe_depth = 1;
    checker.current_c_abi_raw_call_owner = Some(super::type_info::CBindingRawCallOwner {
        name: "increment_bridge".to_string(),
        visibility: crate::ast::Visibility::Private,
        declaration_span: Span::new(100, 200),
    });
    let _ = checker.check_expr(&scalar_c_binding_call(Span::new(120, 140)));

    let raw_call = checker
        .type_info()
        .c_abi
        .raw_calls
        .first()
        .ok_or_else(|| "expected one checked C raw call".to_string())?;
    let owner = raw_call
        .owner
        .as_ref()
        .ok_or_else(|| "expected the direct native call to retain its owning bridge".to_string())?;
    assert_eq!(owner.name, "increment_bridge");
    assert_eq!(owner.visibility, crate::ast::Visibility::Private);
    Ok(())
}

#[test]
fn checked_c_public_raw_call_warns_once_and_preserves_low_level_support() -> Result<(), String> {
    let mut checker = TypeChecker::new();
    let descriptor = scalar_c_binding_descriptor();
    checker
        .type_info
        .c_abi
        .bindings
        .insert(descriptor.class_name.clone(), descriptor);
    checker.unsafe_depth = 1;
    checker.current_c_abi_raw_call_owner = Some(super::type_info::CBindingRawCallOwner {
        name: "public_raw_api".to_string(),
        visibility: crate::ast::Visibility::Public,
        declaration_span: Span::new(100, 200),
    });

    let _ = checker.check_expr(&scalar_c_binding_call(Span::new(120, 140)));
    let _ = checker.check_expr(&scalar_c_binding_call(Span::new(150, 170)));

    assert_eq!(
        checker
            .warnings()
            .iter()
            .filter(|warning| warning.message.contains("public function `public_raw_api`"))
            .count(),
        1,
        "a public raw bridge should receive one actionable advisory without rejecting low-level packages"
    );
    assert!(
        checker.errors.is_empty(),
        "the advisory must not make checked C calls fail: {:?}",
        checker.errors
    );
    Ok(())
}

#[test]
fn checked_c_facades_require_a_same_module_checked_bridge() {
    let private_bridge = super::type_info::CBindingRawCallOwner {
        name: "bridge".to_string(),
        visibility: crate::ast::Visibility::Private,
        declaration_span: Span::new(10, 30),
    };
    let public_facade = super::type_info::CBindingRawCallOwner {
        name: "facade".to_string(),
        visibility: crate::ast::Visibility::Public,
        declaration_span: Span::new(40, 60),
    };
    let imported_lookalike = super::type_info::CBindingRawCallOwner {
        name: "external_facade".to_string(),
        visibility: crate::ast::Visibility::Public,
        declaration_span: Span::new(70, 90),
    };
    let mut artifacts = super::type_info::CAbiInteropArtifacts::default();
    artifacts.raw_calls.push(super::type_info::CBindingRawCall {
        span: Span::new(15, 25),
        owner: Some(private_bridge.clone()),
        binding: "Fixture".to_string(),
        symbol: "open".to_string(),
    });
    artifacts.function_calls.push(super::type_info::CBindingFunctionCall {
        span: Span::new(45, 55),
        caller: public_facade.clone(),
        target: super::type_info::SourceTargetInfo {
            module_path: vec!["app".to_string()],
            name: "bridge".to_string(),
            kind: "function".to_string(),
        },
    });
    artifacts.function_calls.push(super::type_info::CBindingFunctionCall {
        span: Span::new(75, 85),
        caller: imported_lookalike,
        target: super::type_info::SourceTargetInfo {
            module_path: vec!["dependency".to_string()],
            name: "bridge".to_string(),
            kind: "function".to_string(),
        },
    });

    artifacts.resolve_checked_facades(Some(&["app".to_string()]));

    assert_eq!(
        artifacts.facades,
        vec![super::type_info::CBindingFacade {
            facade: public_facade,
            bridge: private_bridge,
            call_span: Span::new(45, 55),
        }],
        "only the compiler-proven same-module bridge may become a facade relation"
    );
}

#[test]
fn checked_c_string_pointer_requires_unsafe_acknowledgement() {
    let errors = check_str_err(
        r#"
from std.interop import c

def pointer_without_unsafe(value: str) -> Result[None, str]:
  text = c.cstr(value)?
  text.as_const_ptr()
  return Ok(None)
"#,
        "checked C string pointer without unsafe acknowledgement",
    );
    assert!(
        errors.iter().any(|error| {
            error
                .message
                .contains("extracting a checked C string pointer requires an enclosing `unsafe:` acknowledgement")
        }),
        "expected checked C string pointer acknowledgement diagnostic, got {errors:?}"
    );
}

#[test]
fn checked_c_string_view_copy_requires_unsafe_and_a_named_bound() {
    let span = Span::new(1, 20);
    let mut checker = TypeChecker::new();
    checker.symbols.define(crate::symbols::Symbol {
        name: "view".to_string(),
        kind: crate::symbols::SymbolKind::Variable(crate::symbols::VariableInfo {
            ty: ResolvedType::Named(incan_lang::lang::c_abi::SCOPED_C_STRING_VIEW_TYPE_ID.to_string()),
            is_mutable: false,
            is_used: false,
        }),
        span,
        scope: 0,
    });
    let view = Spanned::new(Expr::Ident("view".to_string()), span);
    let copy = Spanned::new(
        Expr::MethodCall(
            Box::new(view.clone()),
            "copy_utf8".to_string(),
            Vec::new(),
            vec![CallArg::Named(
                Spanned::new("max_bytes".to_string(), span),
                Spanned::new(Expr::Literal(Literal::Int(IntLiteral::synthetic(64))), span),
            )],
        ),
        span,
    );

    assert_eq!(checker.check_expr(&copy), ResolvedType::Unknown);
    assert!(
        checker.errors.iter().any(|error| error
            .message
            .contains("copying a scoped C string view requires an enclosing `unsafe:`")),
        "expected scoped-view unsafe diagnostic, got {:?}",
        checker.errors
    );

    checker.errors.clear();
    checker.unsafe_depth = 1;
    assert_eq!(
        checker.check_expr(&copy),
        ResolvedType::Generic("Result".to_string(), vec![ResolvedType::Str, ResolvedType::Str])
    );
    assert!(
        checker.errors.is_empty(),
        "unexpected scoped-view copy errors: {:?}",
        checker.errors
    );
    assert!(checker.type_info.c_abi.uses_scoped_c_string_views);

    let missing_name = Spanned::new(
        Expr::MethodCall(
            Box::new(view),
            "copy_utf8".to_string(),
            Vec::new(),
            vec![CallArg::Positional(Spanned::new(
                Expr::Literal(Literal::Int(IntLiteral::synthetic(64))),
                span,
            ))],
        ),
        span,
    );
    assert_eq!(checker.check_expr(&missing_name), ResolvedType::Unknown);
    assert!(
        checker
            .errors
            .iter()
            .any(|error| error.message.contains("copy_utf8(max_bytes=<positive int>)")),
        "expected explicit-bound diagnostic, got {:?}",
        checker.errors
    );
}

#[test]
fn checked_c_binding_rejects_general_pointer_calls() {
    let span = Span::new(1, 20);
    let mut checker = TypeChecker::new();
    let mut descriptor = scalar_c_binding_descriptor();
    descriptor.symbols[0].parameters[0].ty = CBindingType::Pointer {
        mutable: false,
        pointee: Box::new(CBindingType::Scalar(ScalarTypeId::I32)),
    };
    checker
        .type_info
        .c_abi
        .bindings
        .insert(descriptor.class_name.clone(), descriptor);
    checker.unsafe_depth = 1;

    assert_eq!(checker.check_expr(&scalar_c_binding_call(span)), ResolvedType::Unknown);
    assert!(
        checker.errors.iter().any(|error| error
            .message
            .contains("requires native ownership or ABI emission that is not implemented yet")),
        "expected general-pointer rejection, got {:?}",
        checker.errors
    );
    assert!(checker.type_info.c_abi.raw_calls.is_empty());
}

fn resource_c_binding_descriptor() -> CBindingDescriptor {
    CBindingDescriptor {
        span: Span::default(),
        class_name: "Fixture".to_string(),
        header: "fixture.h".to_string(),
        system_library: "fixture".to_string(),
        link_capability: incan_lang::lang::c_abi::LinkCapabilityId::SystemLibrary,
        resources: vec![CBindingResource {
            span: Span::default(),
            name: "Handle".to_string(),
            native: "fixture_handle".to_string(),
            release: "close".to_string(),
        }],
        symbols: vec![
            CBindingSymbol {
                name: "close".to_string(),
                native: "fixture_close".to_string(),
                parameters: vec![CBindingParameter {
                    name: "handle".to_string(),
                    ty: CBindingType::Resource {
                        access: CResourceAccess::Owned,
                        resource: "Handle".to_string(),
                    },
                }],
                return_type: CBindingType::Void,
                buffers: Vec::new(),
                outcomes: Vec::new(),
            },
            CBindingSymbol {
                name: "mutate".to_string(),
                native: "fixture_mutate".to_string(),
                parameters: vec![CBindingParameter {
                    name: "handle".to_string(),
                    ty: CBindingType::Resource {
                        access: CResourceAccess::BorrowedMut,
                        resource: "Handle".to_string(),
                    },
                }],
                return_type: CBindingType::Void,
                buffers: Vec::new(),
                outcomes: Vec::new(),
            },
        ],
        enums: Vec::new(),
        structs: Vec::new(),
    }
}

fn resource_c_binding_call(member: &str, span: Span) -> Spanned<Expr> {
    let binding = Spanned::new(Expr::Ident("Fixture".to_string()), span);
    Spanned::new(
        Expr::MethodCall(
            Box::new(binding),
            member.to_string(),
            Vec::new(),
            vec![CallArg::Positional(Spanned::new(
                Expr::Ident("handle".to_string()),
                span,
            ))],
        ),
        span,
    )
}

fn resource_c_binding_checker(is_mutable: bool) -> TypeChecker {
    let span = Span::new(1, 20);
    let mut checker = TypeChecker::new();
    let descriptor = resource_c_binding_descriptor();
    checker
        .type_info
        .c_abi
        .bindings
        .insert(descriptor.class_name.clone(), descriptor);
    checker.symbols.define(crate::symbols::Symbol {
        name: "handle".to_string(),
        kind: crate::symbols::SymbolKind::Variable(crate::symbols::VariableInfo {
            ty: ResolvedType::Named("__incan_c_resource::Fixture::Handle".to_string()),
            is_mutable,
            is_used: false,
        }),
        span,
        scope: 0,
    });
    if is_mutable {
        checker.mutable_bindings.insert("handle".to_string());
    }
    checker.unsafe_depth = 1;
    checker
}

#[test]
fn checked_c_resources_record_ownership_transfers_and_require_mutable_borrows() {
    let span = Span::new(1, 20);
    let mut moved = resource_c_binding_checker(false);
    assert_eq!(
        moved.check_expr(&resource_c_binding_call("close", span)),
        ResolvedType::Unit
    );
    assert!(moved.errors.is_empty(), "unexpected close errors: {:?}", moved.errors);
    let _ = moved.check_expr(&Spanned::new(Expr::Ident("handle".to_string()), span));
    assert!(
        moved
            .errors
            .iter()
            .any(|error| error.message.contains("was transferred to native code"))
    );

    let mut immutable_borrow = resource_c_binding_checker(false);
    assert_eq!(
        immutable_borrow.check_expr(&resource_c_binding_call("mutate", span)),
        ResolvedType::Unit
    );
    assert!(
        immutable_borrow
            .errors
            .iter()
            .any(|error| error.message.contains("requires a mutable borrow"))
    );

    let mut mutable_borrow = resource_c_binding_checker(true);
    assert_eq!(
        mutable_borrow.check_expr(&resource_c_binding_call("mutate", span)),
        ResolvedType::Unit
    );
    assert!(
        mutable_borrow.errors.is_empty(),
        "unexpected mutable-borrow errors: {:?}",
        mutable_borrow.errors
    );
}
