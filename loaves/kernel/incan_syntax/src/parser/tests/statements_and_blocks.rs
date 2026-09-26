//! Statements and suite layout: assert statement forms, the single top-level indent error, blank lines at the start of
//! and between suites, `if` / `elif` / `else` suite bodies, and `for` tuple bindings.

use super::*;

#[test]
fn test_assert_keyword_lexes_as_identifier() -> Result<(), Vec<CompileError>> {
    let tokens = lexer::lex("assert value\n").map_err(|_| {
        vec![CompileError::new(
            "parser test internal error: lex failed".to_string(),
            Span::default(),
        )]
    })?;

    assert!(matches!(&tokens[0].kind, TokenKind::Ident(name) if name == "assert"));
    Ok(())
}

#[test]
fn test_parse_assert_statement_without_testing_import() -> Result<(), Vec<CompileError>> {
    let source = r#"
def check(value: int) -> None:
  assert value > 0
"#;
    let program = parse_str(source)?;
    let func = require_function_decl(&program.declarations[0])?;
    let Statement::Assert(assert_stmt) = &func.body[0].node else {
        return Err(vec![CompileError::new(
            "parser test internal error: expected assert statement".to_string(),
            func.body[0].span,
        )]);
    };

    assert!(matches!(assert_stmt.kind, AssertKind::Condition(_)));
    assert!(assert_stmt.message.is_none());
    Ok(())
}

#[test]
fn test_parse_assert_statement_with_message() -> Result<(), Vec<CompileError>> {
    let source = r#"
def check(value: int) -> None:
  assert value > 0, "positive required"
"#;
    let program = parse_str(source)?;
    let func = require_function_decl(&program.declarations[0])?;
    let Statement::Assert(assert_stmt) = &func.body[0].node else {
        return Err(vec![CompileError::new(
            "parser test internal error: expected assert statement".to_string(),
            func.body[0].span,
        )]);
    };

    assert!(matches!(assert_stmt.kind, AssertKind::Condition(_)));
    assert!(matches!(
        assert_stmt.message.as_ref().map(|msg| &msg.node),
        Some(Expr::Literal(Literal::String(msg))) if msg == "positive required"
    ));
    Ok(())
}

#[test]
fn test_parse_assert_raises_statement() -> Result<(), Vec<CompileError>> {
    let source = r#"
def check() -> None:
  assert explode() raises AssertionError, "boom"
"#;
    let program = parse_str(source)?;
    let func = require_function_decl(&program.declarations[0])?;
    let Statement::Assert(assert_stmt) = &func.body[0].node else {
        return Err(vec![CompileError::new(
            "parser test internal error: expected assert statement".to_string(),
            func.body[0].span,
        )]);
    };

    let AssertKind::Raises { call, error_type } = &assert_stmt.kind else {
        return Err(vec![CompileError::new(
            "parser test internal error: expected raises assert".to_string(),
            func.body[0].span,
        )]);
    };
    assert!(matches!(call.node, Expr::Call(_, _, _)));
    assert!(matches!(error_type.node, Type::Simple(ref name) if name == "AssertionError"));
    assert!(assert_stmt.message.is_some());
    Ok(())
}

#[test]
fn test_parse_assert_identity_bool_literals_as_condition() -> Result<(), Vec<CompileError>> {
    let source = r#"
def check(ready: bool, done: bool) -> None:
  assert ready is true, "ready should be true"
  assert done is false
"#;
    let program = parse_str(source)?;
    let func = require_function_decl(&program.declarations[0])?;

    let Statement::Assert(true_assert) = &func.body[0].node else {
        return Err(vec![CompileError::new(
            "parser test internal error: expected true assert statement".to_string(),
            func.body[0].span,
        )]);
    };
    assert!(matches!(true_assert.kind, AssertKind::Condition(_)));
    assert!(true_assert.message.is_some());

    let Statement::Assert(false_assert) = &func.body[1].node else {
        return Err(vec![CompileError::new(
            "parser test internal error: expected false assert statement".to_string(),
            func.body[1].span,
        )]);
    };
    assert!(matches!(false_assert.kind, AssertKind::Condition(_)));
    assert!(false_assert.message.is_none());
    Ok(())
}

#[test]
fn test_parse_assert_is_some_pattern_statement() -> Result<(), Vec<CompileError>> {
    let source = r#"
def check(user: Option[str]) -> None:
  assert user is Some(value), "user required"
"#;
    let program = parse_str(source)?;
    let func = require_function_decl(&program.declarations[0])?;
    let Statement::Assert(assert_stmt) = &func.body[0].node else {
        return Err(vec![CompileError::new(
            "parser test internal error: expected assert statement".to_string(),
            func.body[0].span,
        )]);
    };

    let AssertKind::IsPattern { value, pattern } = &assert_stmt.kind else {
        return Err(vec![CompileError::new(
            "parser test internal error: expected pattern assert".to_string(),
            func.body[0].span,
        )]);
    };
    assert!(matches!(value.node, Expr::Ident(ref name) if name == "user"));
    assert!(matches!(
        &pattern.node,
        Pattern::Constructor(name, args)
            if name.node == "Some"
                && matches!(args.first(), Some(PatternArg::Positional(arg)) if matches!(&arg.node, Pattern::Binding(binding) if binding == "value"))
    ));
    assert!(assert_stmt.message.is_some());
    Ok(())
}

#[test]
fn test_parse_assert_is_none_pattern_statement() -> Result<(), Vec<CompileError>> {
    let source = r#"
def check(user: Option[str]) -> None:
  assert user is None
"#;
    let program = parse_str(source)?;
    let func = require_function_decl(&program.declarations[0])?;
    let Statement::Assert(assert_stmt) = &func.body[0].node else {
        return Err(vec![CompileError::new(
            "parser test internal error: expected assert statement".to_string(),
            func.body[0].span,
        )]);
    };

    assert!(matches!(
        &assert_stmt.kind,
        AssertKind::IsPattern { pattern, .. }
            if matches!(&pattern.node, Pattern::Constructor(name, args) if name.node == "None" && args.is_empty())
    ));
    Ok(())
}

#[test]
fn test_parse_assert_is_ok_and_err_pattern_statements() -> Result<(), Vec<CompileError>> {
    let source = r#"
def check(result: Result[int, str]) -> None:
  assert result is Ok(value)
  assert result is Err(_), "error required"
"#;
    let program = parse_str(source)?;
    let func = require_function_decl(&program.declarations[0])?;

    let Statement::Assert(ok_assert) = &func.body[0].node else {
        return Err(vec![CompileError::new(
            "parser test internal error: expected Ok assert statement".to_string(),
            func.body[0].span,
        )]);
    };
    assert!(matches!(
        &ok_assert.kind,
        AssertKind::IsPattern { pattern, .. }
            if matches!(
                &pattern.node,
                Pattern::Constructor(name, args)
                    if name.node == "Ok"
                        && matches!(args.first(), Some(PatternArg::Positional(arg)) if matches!(&arg.node, Pattern::Binding(binding) if binding == "value"))
            )
    ));

    let Statement::Assert(err_assert) = &func.body[1].node else {
        return Err(vec![CompileError::new(
            "parser test internal error: expected Err assert statement".to_string(),
            func.body[1].span,
        )]);
    };
    assert!(matches!(
        &err_assert.kind,
        AssertKind::IsPattern { pattern, .. }
            if matches!(
                &pattern.node,
                Pattern::Constructor(name, args)
                    if name.node == "Err"
                        && matches!(args.first(), Some(PatternArg::Positional(arg)) if matches!(arg.node, Pattern::Wildcard))
            )
    ));
    assert!(err_assert.message.is_some());
    Ok(())
}

#[test]
fn test_unexpected_indent_at_toplevel_is_single_clear_error() {
    // We intentionally allow the lexer to emit INDENT/DEDENT tokens at the top-level.
    // The parser should produce a single clear error and avoid cascading failures.
    let source = "  x = 1\n";
    let Err(err) = parse_str(source) else {
        panic!("Top-level indentation should be rejected by the parser");
    };
    assert_eq!(err.len(), 1, "Parser should return exactly one error (no cascade)");
    assert!(
        err[0].message.contains("Expected declaration") && err[0].message.contains("Indent"),
        "Error message should clearly indicate the unexpected INDENT token; got: {}",
        err[0].message
    );
}

#[test]
fn test_parse_block_leading_blank_lines_single_empty_line() -> Result<(), Vec<CompileError>> {
    let source = r#"def f() -> int:
    a = 1

    b = 2
"#;
    let program = parse_str(source)?;
    let func = require_function_decl(&program.declarations[0])?;
    assert_eq!(func.body.len(), 2, "expected two statements");
    assert_eq!(func.body[0].leading_blank_lines, 0);
    assert_eq!(func.body[1].leading_blank_lines, 1);
    Ok(())
}

#[test]
fn test_parse_block_leading_blank_lines_collapses_multiple_empty_lines() -> Result<(), Vec<CompileError>> {
    let source = r#"def f() -> int:
    a = 1



    b = 2
"#;
    let program = parse_str(source)?;
    let func = require_function_decl(&program.declarations[0])?;
    assert_eq!(func.body.len(), 2);
    assert_eq!(func.body[0].leading_blank_lines, 0);
    assert_eq!(func.body[1].leading_blank_lines, 1);
    Ok(())
}

#[test]
fn test_parse_block_preserves_blank_line_after_nested_suite() -> Result<(), Vec<CompileError>> {
    let source = r#"def f(items: list[int]) -> int:
    for item in items:
        value = item

    result = 1
    return result
"#;
    let program = parse_str(source)?;
    let func = require_function_decl(&program.declarations[0])?;
    assert_eq!(func.body.len(), 3);
    assert_eq!(func.body[0].leading_blank_lines, 0);
    assert_eq!(func.body[1].leading_blank_lines, 1);
    assert_eq!(func.body[2].leading_blank_lines, 0);
    Ok(())
}

#[test]
fn test_parse_block_preserves_single_blank_line_between_sibling_if_statements() -> Result<(), Vec<CompileError>> {
    let source = r#"def f(a: bool, b: bool) -> None:
    if a:
        x = 1

    if b:
        y = 2

    z = 3
"#;
    let program = parse_str(source)?;
    let func = require_function_decl(&program.declarations[0])?;
    assert_eq!(func.body.len(), 3);
    assert_eq!(func.body[0].leading_blank_lines, 0);
    assert_eq!(func.body[1].leading_blank_lines, 1);
    assert_eq!(func.body[2].leading_blank_lines, 1);
    Ok(())
}

#[test]
fn test_parse_block_does_not_invent_blank_line_between_sibling_if_statements() -> Result<(), Vec<CompileError>> {
    let source = r#"def f(a: bool, b: bool) -> None:
    if a:
        x = 1
    if b:
        y = 2
    z = 3
"#;
    let program = parse_str(source)?;
    let func = require_function_decl(&program.declarations[0])?;
    assert_eq!(func.body.len(), 3);
    assert_eq!(func.body[0].leading_blank_lines, 0);
    assert_eq!(func.body[1].leading_blank_lines, 0);
    assert_eq!(func.body[2].leading_blank_lines, 0);
    Ok(())
}

#[test]
fn test_parse_block_preserves_single_blank_line_between_if_blocks_ending_in_match() -> Result<(), Vec<CompileError>> {
    let source = r#"def f(a: bool, b: bool, result: Result[int, str]) -> None:
    if a:
        match result:
            Ok(_) => return
            Err(err) => return

    if b:
        match result:
            Ok(_) => return
            Err(err) => return

    z = 3
"#;
    let program = parse_str(source)?;
    let func = require_function_decl(&program.declarations[0])?;
    assert_eq!(func.body.len(), 3);
    assert_eq!(func.body[0].leading_blank_lines, 0);
    assert_eq!(func.body[1].leading_blank_lines, 1);
    assert_eq!(func.body[2].leading_blank_lines, 1);
    Ok(())
}

#[test]
fn test_parse_assert_without_std_testing_import_ok() -> Result<(), Vec<CompileError>> {
    let source = r#"
def f(x: int) -> None:
  assert x > 0
"#;
    let program = parse_str(source)?;
    let func = require_function_decl(&program.declarations[0])?;
    assert!(matches!(&func.body[0].node, Statement::Assert(_)));
    Ok(())
}

#[test]
fn test_parse_assert_with_std_testing_import_still_uses_core_assert_ast() -> Result<(), Vec<CompileError>> {
    let source = r#"
import std.testing

def f(x: int) -> None:
  assert x > 0, "x must be positive"
"#;
    let program = parse_str(source)?;
    let func = require_function_decl(&program.declarations[1])?;
    let Statement::Assert(assert_stmt) = &func.body[0].node else {
        return Err(vec![CompileError::new(
            "parser test internal error: expected assert statement".to_string(),
            func.body[0].span,
        )]);
    };
    assert!(matches!(assert_stmt.kind, AssertKind::Condition(_)));
    assert!(assert_stmt.message.is_some());
    Ok(())
}

#[test]
fn test_parse_if_elif_else_allows_blank_before_suite_body() -> Result<(), Vec<CompileError>> {
    let source = r#"
def f(kind: str) -> int:
  if kind == "a":
    return 1
  elif kind == "b":

    return 2
  else:

    return 3
"#;
    let program = parse_str(source)?;
    let func = match &program.declarations[0].node {
        Declaration::Function(func) => func,
        _ => panic!("Expected function declaration"),
    };
    assert_eq!(func.body.len(), 1);
    assert!(matches!(func.body[0].node, Statement::If(_)));
    Ok(())
}

#[test]
fn test_parse_for_tuple_unpack_binding() {
    let source = "def bind(xs: list[str]) -> list[str]:\n  mut out: list[str] = []\n  for idx, name in enumerate(xs):\n    out.append(name)\n  return out\n";
    let program = match parse_str(source) {
        Ok(program) => program,
        Err(errs) => panic!("for tuple-unpack binding should parse: {errs:?}"),
    };
    let Declaration::Function(function) = &program.declarations[0].node else {
        panic!("expected function declaration");
    };
    let Statement::For(for_stmt) = &function.body[1].node else {
        panic!("expected for statement");
    };
    let Pattern::Tuple(items) = &for_stmt.pattern.node else {
        panic!("expected tuple binding pattern");
    };

    assert_eq!(items.len(), 2);
    assert_eq!(items[0].node, Pattern::Binding("idx".to_string()));
    assert_eq!(items[1].node, Pattern::Binding("name".to_string()));
}

#[test]
fn test_parse_refuses_an_annotated_chained_assignment_issue1806() -> Result<(), Vec<CompileError>> {
    // An annotation declares one binding, so `x: Option[int] = y = 5` is refused rather than dropping the annotation.
    let Err(errors) = parse_str("def f() -> None:\n  x: Option[int] = y = 5\n") else {
        return Err(vec![CompileError::new(
            "parser test internal error: an annotated chain must not parse".to_string(),
            Span::default(),
        )]);
    };
    assert!(
        errors.iter().any(|error| error
            .message
            .contains("A type annotation applies to one assignment target")),
        "expected the annotated-chain refusal, got {errors:?}"
    );

    // Without the annotation the chain parses, and an annotated single assignment still parses.
    parse_str("def f() -> None:\n  x = y = 5\n  z: int = 5\n")?;
    Ok(())
}
