//! Implicit closure parameters retain validated call requirements without changing legacy signature snapshots.

use super::{IncanType, TypeChecker, bir, body_named, build, build_body_ir_module_v0, lexer, parser};
use crate::symbols::ResolvedType;

/// The complete original closes both its locally stored and contextually passed closure parameter frames.
#[test]
fn complete_local_function_closures_retain_closed_parameters() -> Result<(), Box<dyn std::error::Error>> {
    let source = include_str!(
        "../../../../incan_test_support/fixtures/behavior/codegen_functions_and_aliases/closure_shapes_call_local_functions.incn"
    );
    let module = build(source, &["m", "complete_implicit_parameters"])?;
    for name in ["run_local", "run_method"] {
        let body = body_named(&module, name)?;
        let locals = serde_json::to_string(&body.locals)?;
        assert!(
            !locals.contains("\"Unknown\"") && !locals.contains("\"TypeVar\""),
            "{}",
            body.render_snapshot()
        );
    }
    Ok(())
}

/// The Body IR callable contract closes the parameter while the ordinary checked expression keeps its legacy hole.
#[test]
fn validated_parameter_requirement_preserves_legacy_snapshot() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def describe(code: int) -> str:\n    return f\"code {code}\"\n\ndef main() -> None:\n    f = (code) => describe(code)\n    println(f(3))\n";
    let tokens = lexer::lex(source).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let program = parser::parse(&tokens).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let path = vec!["m".to_owned(), "implicit_parameter_snapshot".to_owned()];
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(path.clone()));
    checker
        .check_program(&program)
        .map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let start = source.find("(code) => describe(code)").ok_or("missing closure")?;
    let end = start + "(code) => describe(code)".len();
    let facts = checker.type_info();
    let Some(ResolvedType::Function(parameters, _)) = facts.expressions.expr_types.get(&(start, end)) else {
        return Err("missing ordinary closure signature".into());
    };
    assert!(matches!(
        &parameters[0].ty,
        ResolvedType::TypeVar(_) | ResolvedType::Unknown
    ));
    let Some(ResolvedType::Function(parameters, _)) = facts.expressions.body_ir_expr_types.get(&(start, end)) else {
        return Err("missing executable closure signature".into());
    };
    assert_eq!(parameters[0].ty, ResolvedType::Int);
    let module = build_body_ir_module_v0(&program, &path, facts);
    assert!(!serde_json::to_string(&body_named(&module, "main")?.locals)?.contains("\"TypeVar\""));
    Ok(())
}

/// A closure with no validated requirement retains its unknown parameter instead of guessing from a later invocation.
#[test]
fn absent_parameter_requirement_keeps_parameter_open() -> Result<(), Box<dyn std::error::Error>> {
    assert_open_parameter(
        "def main() -> None:\n    held = (value) => value\n    println(held(1))\n",
        "main",
    )
}

/// Inconsistent validated int and str requirements cannot grant a single concrete closure parameter.
#[test]
fn conflicting_parameter_requirements_keep_parameter_open() -> Result<(), Box<dyn std::error::Error>> {
    assert_open_parameter(
        "def consume_int(value: int) -> None:\n    println(value)\n\ndef consume_str(value: str) -> None:\n    println(value)\n\ndef main() -> None:\n    held = (value) => (consume_int(value), consume_str(value))\n",
        "main",
    )
}

/// A caller-owned generic requirement remains open in the closure's own parameter contract.
#[test]
fn caller_owned_parameter_requirement_keeps_parameter_open() -> Result<(), Box<dyn std::error::Error>> {
    assert_open_parameter(
        "def echo[T](value: T) -> T:\n    return value\n\ndef outer[T](other: T) -> None:\n    held = (value) => echo[T](value)\n",
        "outer",
    )
}

/// Parameter facts use exact lexical identities, preserving a same-spelled outer text binding.
#[test]
fn shadowed_parameter_requirement_preserves_outer_binding() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def describe(code: int) -> str:\n    return f\"code {code}\"\n\ndef main() -> None:\n    code = \"outer\"\n    held = (code) => describe(code)\n    println(held(3))\n    println(code)\n";
    let module = build(source, &["m", "shadowed_implicit_parameter"])?;
    let main = body_named(&module, "main")?;
    assert!(
        !serde_json::to_string(&main.locals)?.contains("\"TypeVar\""),
        "{}",
        main.render_snapshot()
    );
    assert!(
        main.locals
            .iter()
            .any(|local| local.name.as_deref() == Some("code") && local.ty.to_string() == "str")
    );
    Ok(())
}

/// Require at least one still-open callable frame in a normally checked source program.
fn assert_open_parameter(source: &str, body_name: &str) -> Result<(), Box<dyn std::error::Error>> {
    let module = build(source, &["m", "open_implicit_parameter"])?;
    let body = body_named(&module, body_name)?;
    let mut parameters = body
        .block
        .stmts
        .iter()
        .filter_map(|statement| match &statement.kind {
            bir::StatementKind::Assign {
                rvalue: bir::Rvalue::Closure { params, .. },
                ..
            } => Some(params.as_slice()),
            _ => None,
        })
        .flatten();
    assert!(
        parameters.any(|parameter| matches!(&parameter.ty, IncanType::Unknown | IncanType::TypeVar(_))),
        "{}",
        body.render_snapshot()
    );
    Ok(())
}

/// A call rejected for its extra argument cannot authorize a private requirement for the implicit parameter it reads.
#[test]
fn failed_call_validation_keeps_parameter_requirement_absent() -> Result<(), Box<dyn std::error::Error>> {
    let source = "def describe(code: int) -> str:\n    return f\"code {code}\"\n\ndef main() -> None:\n    held = (code) => describe(code, 0)\n";
    let tokens = lexer::lex(source).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let program = parser::parse(&tokens).map_err(|errors| std::io::Error::other(format!("{errors:?}")))?;
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(vec!["m".to_owned(), "invalid_parameter_requirement".to_owned()]));
    let errors = checker
        .check_program(&program)
        .err()
        .ok_or("expected an invalid call diagnostic")?;
    assert!(!errors.is_empty());
    let start = source.find("(code)").ok_or("missing implicit parameter")? + 1;
    let span = crate::ast::Span {
        start,
        end: start + "code".len(),
    };
    let facts = checker.type_info();
    let identity = facts
        .resolved_write_identity(span, "code")
        .ok_or("missing parameter identity")?;
    assert!(!facts.expressions.body_ir_parameter_requirements.contains_key(identity));
    Ok(())
}
