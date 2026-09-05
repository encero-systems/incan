//! Checked identity and Body-IR coverage for Unicode-scalar string length.

use incan::frontend::ast;
use incan::frontend::body_ir::build_body_ir_module_v0;
use incan::frontend::typechecker::TypeChecker;
use incan::frontend::{lexer, parser};
use incan_core::lang::builtins::BuiltinFnId;
use incan_core::lang::surface::string_methods::StringMethodId;
use incan_semantics_core::body_ir::{BodyIrModule, CallableTarget, Callee, StatementKind};

fn checked(source: &str) -> Result<(ast::Program, TypeChecker, Vec<String>), Box<dyn std::error::Error>> {
    let tokens = lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let path = vec!["checked_string_len".to_string()];
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(path.clone()));
    checker
        .check_program(&program)
        .map_err(|errors| format!("{errors:?}"))?;
    Ok((program, checker, path))
}

fn call_span(source: &str, call: &str) -> Result<ast::Span, Box<dyn std::error::Error>> {
    let start = source.find(call).ok_or_else(|| format!("missing call `{call}`"))?;
    Ok(ast::Span::new(start, start + call.len()))
}

fn first_named_target<'module>(
    module: &'module BodyIrModule,
    body_name: &str,
) -> Result<&'module incan_semantics_core::body_ir::NamedCallableTarget, Box<dyn std::error::Error>> {
    let body = module
        .bodies
        .iter()
        .find(|body| body.name == body_name)
        .ok_or_else(|| format!("missing body `{body_name}`"))?;
    body.block
        .stmts
        .iter()
        .find_map(|statement| match &statement.kind {
            StatementKind::Call {
                callee: Callee::Function(CallableTarget::Named(target)),
                ..
            } => Some(target),
            _ => None,
        })
        .ok_or_else(|| format!("body `{body_name}` has no named call").into())
}

/// Unshadowed global length must carry compiler-owned builtin identity.
#[test]
fn global_len_str_retains_builtin_identity() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def length(value: str) -> int:\n    return len(value)\n";
    let (program, checker, path) = checked(source)?;
    let module = build_body_ir_module_v0(&program, &path, checker.type_info());
    let target = first_named_target(&module, "length")?;
    assert_eq!(target.builtin, Some(BuiltinFnId::Len));
    assert!(target.direct_call_id.is_none());
    Ok(())
}

/// A runtime string method must retain the canonical method identity and lower to one represented helper.
#[test]
fn method_len_str_retain_canonical_helper_identity() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def length(value: str) -> int:\n    return value.len()\n";
    let (program, checker, path) = checked(source)?;
    let span = call_span(source, "value.len()")?;
    assert_eq!(
        checker.type_info().resolved_string_helper_call(span),
        Some(StringMethodId::Len)
    );
    let snapshot = build_body_ir_module_v0(&program, &path, checker.type_info()).render_snapshot();
    assert!(snapshot.contains("call helper:str_len"), "{snapshot}");
    assert!(!snapshot.contains("method:len"), "{snapshot}");
    Ok(())
}

/// Ordinary `len` lexical shadowing must keep the user declaration, not acquire builtin identity.
#[test]
fn lexical_len_shadowing_remains_a_named_user_call() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
def len(value: str) -> int:
    return 99

def length(value: str) -> int:
    return len(value)
"#;
    let (program, checker, path) = checked(source)?;
    let module = build_body_ir_module_v0(&program, &path, checker.type_info());
    let target = first_named_target(&module, "length")?;
    assert_eq!(target.builtin, None);
    assert!(target.direct_call_id.is_some());
    Ok(())
}

/// The selected method is exactly zero-argument and malformed calls retain no helper identity.
#[test]
fn method_len_str_requires_zero_arguments() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def length(value: str) -> int:\n    return value.len(\"extra\")\n";
    let tokens = lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
    let program = parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
    let mut checker = TypeChecker::new();
    let errors = checker
        .check_program(&program)
        .err()
        .ok_or("string len with one argument must fail checking")?;
    assert!(
        errors
            .iter()
            .any(|error| error.message == "str.len() expects 0 argument(s), got 1"),
        "{errors:?}"
    );
    let span = call_span(source, "value.len(\"extra\")")?;
    assert!(checker.type_info().resolved_string_helper_call(span).is_none());
    Ok(())
}

/// Missing or inconsistent checked identities refuse at the original method call rather than dispatching by text.
#[test]
fn method_len_identity_gaps_refuse_at_the_call_span() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def length(value: str) -> int:\n    return value.len()\n";
    let (program, checker, path) = checked(source)?;
    let call = call_span(source, "value.len()")?;

    for type_info in {
        let mut missing = checker.type_info().clone();
        missing
            .calls
            .resolved_string_helper_calls
            .remove(&(call.start, call.end));
        let mut mismatched = checker.type_info().clone();
        mismatched
            .calls
            .resolved_string_helper_calls
            .insert((call.start, call.end), StringMethodId::Upper);
        [missing, mismatched]
    } {
        let module = build_body_ir_module_v0(&program, &path, &type_info);
        let body = module
            .bodies
            .iter()
            .find(|body| body.name == "length")
            .ok_or("missing length body")?;
        let refusal = body
            .block
            .stmts
            .iter()
            .find(|statement| matches!(statement.kind, StatementKind::Unsupported { .. }))
            .ok_or("identity gap must lower to an explicit refusal")?;
        assert_eq!((refusal.span.start, refusal.span.end), (call.start, call.end));
        let StatementKind::Unsupported { description } = &refusal.kind else {
            return Err("expected unsupported statement".into());
        };
        assert!(description.contains("checked string helper"), "{description}");
    }
    Ok(())
}
