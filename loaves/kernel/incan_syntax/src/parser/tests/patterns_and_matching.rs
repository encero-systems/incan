//! Patterns and matching: named-key and qualified unit patterns, alternation and grouping, `if let` / `while let`
//! conditions, match arm guards, fat-arrow arm bodies and their blank-line handling.

use super::*;

/// Return the exact span of the last fixture occurrence without introducing a panic path.
fn require_last_source_span(source: &str, needle: &str) -> Result<Span, Vec<CompileError>> {
    source
        .rmatch_indices(needle)
        .next()
        .map(|(start, matched)| Span::new(start, start + matched.len()))
        .ok_or_else(|| {
            vec![CompileError::new(
                format!("parser test internal error: `{needle}` not found"),
                Span::default(),
            )]
        })
}

#[test]
fn test_parse_pattern_named_key_keyword() -> Result<(), Vec<CompileError>> {
    let source = r#"
def f(a: Foo) -> int:
  match a:
    Foo(type=x) => return x
"#;
    let program = parse_str(source)?;
    let func = match &program.declarations[0].node {
        Declaration::Function(func) => func,
        _ => panic!("Expected function"),
    };
    let match_expr = match &func.body[0].node {
        Statement::Expr(expr) => expr,
        _ => panic!("Expected match expression statement"),
    };
    let arms = match &match_expr.node {
        Expr::Match(_, arms) => arms,
        _ => panic!("Expected match expression"),
    };
    let arm = &arms[0].node;
    match &arm.pattern.node {
        Pattern::Constructor(name, args) => {
            assert_eq!(name.node, "Foo");
            let constructor_start = require_last_source_span(source, "Foo(type")?.start;
            assert_eq!(name.span, Span::new(constructor_start, constructor_start + 3));
            assert!(matches!(
                &args[0],
                PatternArg::Named(field, pat)
                    if field.node == "type" && matches!(&pat.node, Pattern::Binding(b) if b == "x")
            ));
            let PatternArg::Named(field, _) = &args[0] else {
                unreachable!("named pattern shape was asserted above")
            };
            let field_start = require_source_span(source, "type=x", 0)?.start;
            assert_eq!(field.span, Span::new(field_start, field_start + "type".len()));
        }
        _ => panic!("Expected constructor pattern"),
    }
    Ok(())
}

/// Qualified unit variant patterns parse as `Type::Variant` in the AST for Rust lowering; surface syntax uses `.`.
#[test]
fn test_parse_qualified_unit_pattern_stores_double_colon_in_ast() -> Result<(), Vec<CompileError>> {
    let source = r#"
def f(x: int) -> int:
  match x:
    Kind.Read =>
      return 1
"#;
    let program = parse_str(source)?;
    let func = match &program.declarations[0].node {
        Declaration::Function(func) => func,
        _ => panic!("Expected function"),
    };
    let match_expr = match &func.body[0].node {
        Statement::Expr(expr) => expr,
        _ => panic!("Expected match expression statement"),
    };
    let arms = match &match_expr.node {
        Expr::Match(_, arms) => arms,
        _ => panic!("Expected match expression"),
    };
    let arm = &arms[0].node;
    match &arm.pattern.node {
        Pattern::Constructor(name, args) => {
            assert_eq!(name.node, "Kind::Read");
            let start = require_source_span(source, "Kind.Read", 0)?.start;
            assert_eq!(name.span, Span::new(start, start + "Kind.Read".len()));
            assert!(args.is_empty());
        }
        _ => panic!("Expected constructor pattern"),
    }
    Ok(())
}

#[test]
fn test_parse_match_pattern_alternation() -> Result<(), Vec<CompileError>> {
    let source = r#"
def f(kind: Kind) -> int:
  match kind:
    Kind.Read | Kind.Scan | _ => return 1
"#;
    let program = parse_str(source)?;
    let func = match &program.declarations[0].node {
        Declaration::Function(func) => func,
        _ => panic!("Expected function"),
    };
    let match_expr = match &func.body[0].node {
        Statement::Expr(expr) => expr,
        _ => panic!("Expected match expression statement"),
    };
    let arms = match &match_expr.node {
        Expr::Match(_, arms) => arms,
        _ => panic!("Expected match expression"),
    };
    match &arms[0].node.pattern.node {
        Pattern::Or(patterns) => {
            assert_eq!(patterns.len(), 3);
            assert!(
                matches!(&patterns[0].node, Pattern::Constructor(name, args) if name.node == "Kind::Read" && args.is_empty())
            );
            assert!(
                matches!(&patterns[1].node, Pattern::Constructor(name, args) if name.node == "Kind::Scan" && args.is_empty())
            );
            assert!(matches!(&patterns[2].node, Pattern::Wildcard));
        }
        _ => panic!("Expected pattern alternation"),
    }
    Ok(())
}

#[test]
fn test_parse_grouped_pattern_alternation() -> Result<(), Vec<CompileError>> {
    let source = r#"
def f(kind: Kind) -> int:
  match kind:
    (
      Kind.Read
      | Kind.Scan
      | _
    ) => return 1
"#;
    let program = parse_str(source)?;
    let func = require_function_decl(&program.declarations[0])?;
    let Statement::Expr(match_expr) = &func.body[0].node else {
        panic!("Expected match expression statement");
    };
    let Expr::Match(_, arms) = &match_expr.node else {
        panic!("Expected match expression");
    };
    match &arms[0].node.pattern.node {
        Pattern::Group(inner) => {
            assert!(matches!(&inner.node, Pattern::Or(patterns) if patterns.len() == 3));
        }
        _ => panic!("Expected grouped pattern alternation"),
    }
    Ok(())
}

#[test]
fn test_parse_if_let_condition() -> Result<(), Vec<CompileError>> {
    let source = r#"
def f(opt: Option[int]) -> int:
  if let Some(value) = opt:
    return value
  return 0
"#;
    let program = parse_str(source)?;
    let func = require_function_decl(&program.declarations[0])?;
    let stmt = &func.body[0].node;
    let Statement::If(if_stmt) = stmt else {
        panic!("Expected if statement");
    };
    match &if_stmt.condition {
        Condition::Let { pattern, value } => {
            assert!(matches!(&value.node, Expr::Ident(name) if name == "opt"));
            match &pattern.node {
                Pattern::Constructor(name, args) => {
                    assert_eq!(name.node, "Some");
                    assert!(matches!(
                        &args[0],
                        PatternArg::Positional(pat)
                            if matches!(&pat.node, Pattern::Binding(binding) if binding == "value")
                    ));
                }
                _ => panic!("Expected constructor pattern"),
            }
        }
        Condition::Expr(_) => panic!("Expected let condition"),
    }
    Ok(())
}

#[test]
fn test_parse_if_let_pattern_alternation() -> Result<(), Vec<CompileError>> {
    let source = r#"
def f(result: Result[int, int]) -> int:
  if let Ok(value) | Err(value) = result:
    return value
  return 0
"#;
    let program = parse_str(source)?;
    let func = require_function_decl(&program.declarations[0])?;
    let Statement::If(if_stmt) = &func.body[0].node else {
        panic!("Expected if statement");
    };
    match &if_stmt.condition {
        Condition::Let { pattern, value } => {
            let Expr::Ident(value_name) = &value.node else {
                panic!("Expected identifier value");
            };
            assert_eq!(value_name, "result");
            match &pattern.node {
                Pattern::Or(patterns) => {
                    assert_eq!(patterns.len(), 2);
                    assert!(matches!(
                        &patterns[0].node,
                        Pattern::Constructor(name, args)
                        if name.node == "Ok"
                                && matches!(&args[0], PatternArg::Positional(pat) if matches!(&pat.node, Pattern::Binding(binding) if binding == "value"))
                    ));
                    assert!(matches!(
                        &patterns[1].node,
                        Pattern::Constructor(name, args)
                        if name.node == "Err"
                                && matches!(&args[0], PatternArg::Positional(pat) if matches!(&pat.node, Pattern::Binding(binding) if binding == "value"))
                    ));
                }
                _ => panic!("Expected pattern alternation"),
            }
        }
        Condition::Expr(_) => panic!("Expected let condition"),
    }
    Ok(())
}

#[test]
fn test_parse_while_let_condition() -> Result<(), Vec<CompileError>> {
    let source = r#"
def drain(current: Option[int]) -> int:
  while let Some(value) = current:
    return value
  return 0
"#;
    let program = parse_str(source)?;
    let func = require_function_decl(&program.declarations[0])?;
    let stmt = &func.body[0].node;
    let Statement::While(while_stmt) = stmt else {
        panic!("Expected while statement");
    };
    match &while_stmt.condition {
        Condition::Let { pattern, value } => {
            assert!(matches!(&value.node, Expr::Ident(name) if name == "current"));
            assert!(matches!(
                &pattern.node,
                Pattern::Constructor(name, args)
                    if name.node == "Some"
                        && matches!(
                            &args[0],
                            PatternArg::Positional(pat)
                                if matches!(&pat.node, Pattern::Binding(binding) if binding == "value")
                        )
            ));
        }
        Condition::Expr(_) => panic!("Expected let condition"),
    }
    Ok(())
}

#[test]
fn test_parse_while_let_rejects_pattern_alternation() {
    let source = r#"
def drain(current: Result[int, int]) -> int:
  while let Ok(value) | Err(value) = current:
    return value
  return 0
"#;
    let errors = parse_str_err(source, "`while let` pattern alternation should fail");
    assert!(
        errors.iter().any(|err| err
            .message
            .contains("Pattern alternation is only supported in match arms and if let patterns")),
        "expected `while let` pattern alternation rejection, got: {errors:?}"
    );
}

#[test]
fn test_parse_if_let_rejects_else_branch() {
    let source = r#"
def f(opt: Option[int]) -> int:
  if let Some(value) = opt:
    return value
  else:
    return 0
"#;
    let errors = parse_str_err(source, "`if let` with else should fail");
    assert!(
        errors
            .iter()
            .any(|err| err.message.contains("`if let` does not support `else` branches")),
        "expected `if let` else rejection, got: {errors:?}"
    );
}

#[test]
fn test_parse_match() -> Result<(), Vec<CompileError>> {
    let source = r#"
def handle(opt: Option[int]) -> int:
  match opt:
    case Some(x):
      return x
    case None:
      return 0
"#;
    let program = parse_str(source)?;
    assert_eq!(program.declarations.len(), 1);
    Ok(())
}

/// Return the arms of the sole match expression in a single-function program.
fn sole_match_arms(program: &Program) -> Result<&Vec<Spanned<MatchArm>>, String> {
    let Declaration::Function(func) = &program.declarations[0].node else {
        return Err("expected a function declaration".to_string());
    };
    let statement = &func.body[0].node;
    let expr = match statement {
        Statement::Expr(expr) => expr,
        Statement::Return(Some(expr)) => expr,
        other => return Err(format!("expected a match-bearing statement, got {other:?}")),
    };
    match &expr.node {
        Expr::Match(_, arms) => Ok(arms),
        other => Err(format!("expected a match expression, got {other:?}")),
    }
}

#[test]
fn both_arm_spellings_accept_the_same_guard() -> Result<(), String> {
    // `case Pattern:` and `Pattern =>` are two spellings of one arm grammar. The arrow form used to reject a
    // guard the `case` form accepted -- `Expected '=>' after pattern` -- which is a divergence the language
    // states nowhere. Parsing the guard once for both is what keeps them from drifting again (#1401).
    let case_form = r#"
def classify(n: int) -> int:
  match n:
    case x if x < 0:
      return 0
    case _:
      return 1
"#;
    let arrow_form = r#"
def classify(n: int) -> int:
  match n:
    x if x < 0 => return 0
    _ => return 1
"#;
    let case_program = parse_str(case_form).map_err(|errors| format!("case form: {errors:?}"))?;
    let arrow_program = parse_str(arrow_form).map_err(|errors| format!("arrow form: {errors:?}"))?;

    let case_arms = sole_match_arms(&case_program)?;
    let arrow_arms = sole_match_arms(&arrow_program)?;

    let case_guard = case_arms[0]
        .node
        .guard
        .as_ref()
        .ok_or_else(|| "the case form lost its guard".to_string())?;
    let arrow_guard = arrow_arms[0]
        .node
        .guard
        .as_ref()
        .ok_or_else(|| "the arrow form did not record a guard".to_string())?;

    // Compared by source text rather than by AST: every node carries its own span, and the two spellings put
    // the same guard at different offsets, so a structural comparison would only be asserting that the two
    // fixtures are the same length.
    assert_eq!(
        &case_form[case_guard.span.start..case_guard.span.end],
        &arrow_form[arrow_guard.span.start..arrow_guard.span.end],
        "the two spellings parsed different guard expressions"
    );
    let case_pattern = &case_arms[0].node.pattern.span;
    let arrow_pattern = &arrow_arms[0].node.pattern.span;
    assert_eq!(
        &case_form[case_pattern.start..case_pattern.end],
        &arrow_form[arrow_pattern.start..arrow_pattern.end],
        "the two spellings parsed different patterns"
    );
    assert!(
        arrow_arms[1].node.guard.is_none(),
        "an unguarded arrow arm must not acquire a guard"
    );
    Ok(())
}

#[test]
fn an_arrow_arm_guard_stops_at_the_arrow() -> Result<(), String> {
    // The guard is parsed with the ordinary expression grammar, so the risk is that it swallows the `=>` that
    // terminates the arm. A comparison guard is the shape most likely to expose that.
    let source = r#"
def pick(n: int) -> str:
  return match n:
    x if x >= 10 => "big"
    _ => "small"
"#;
    let program = parse_str(source).map_err(|errors| format!("{errors:?}"))?;
    let arms = sole_match_arms(&program)?;
    assert_eq!(arms.len(), 2, "the guard swallowed the arm separator");
    assert!(arms[0].node.guard.is_some(), "the guarded arm lost its guard");
    match &arms[0].node.body {
        MatchBody::Expr(_) => Ok(()),
        other => Err(format!("expected an expression body after `=>`, got {other:?}")),
    }
}

#[test]
fn test_parse_match_fat_arrow_inline_return() -> Result<(), Vec<CompileError>> {
    let source = r#"
def f() -> int:
  match Ok(1):
    Ok(x) => return x
    Err(_) => return 0
"#;
    let program = parse_str(source)?;
    assert_eq!(program.declarations.len(), 1);
    let func = match &program.declarations[0].node {
        Declaration::Function(func) => func,
        _ => panic!("Expected function declaration"),
    };
    assert_eq!(func.body.len(), 1);
    let match_expr = match &func.body[0].node {
        Statement::Expr(expr) => expr,
        _ => panic!("Expected match expression statement"),
    };
    let arms = match &match_expr.node {
        Expr::Match(_, arms) => arms,
        _ => panic!("Expected match expression"),
    };
    assert_eq!(arms.len(), 2);
    for arm in arms {
        match &arm.node.body {
            MatchBody::Block(stmts) => {
                assert_eq!(stmts.len(), 1);
                assert!(matches!(stmts[0].node, Statement::Return(_)));
            }
            MatchBody::Expr(_) => panic!("Expected inline return to parse as statement block"),
        }
    }
    Ok(())
}

#[test]
fn test_parse_match_fat_arrow_inline_compound_assignment() -> Result<(), Vec<CompileError>> {
    let source = r#"
def f() -> str:
  mut out = ""
  match 1:
    1 => out += "a"
    _ => out += "b"
  return out
"#;
    let program = parse_str(source)?;
    let func = match &program.declarations[0].node {
        Declaration::Function(func) => func,
        _ => panic!("Expected function declaration"),
    };
    assert_eq!(func.body.len(), 3);
    let match_expr = match &func.body[1].node {
        Statement::Expr(expr) => expr,
        _ => panic!("Expected match expression statement"),
    };
    let arms = match &match_expr.node {
        Expr::Match(_, arms) => arms,
        _ => panic!("Expected match expression"),
    };
    assert_eq!(arms.len(), 2);
    for arm in arms {
        match &arm.node.body {
            MatchBody::Block(stmts) => {
                assert_eq!(stmts.len(), 1);
                assert!(matches!(stmts[0].node, Statement::CompoundAssignment(_)));
            }
            MatchBody::Expr(_) => panic!("Expected inline compound assignment to parse as statement block"),
        }
    }
    Ok(())
}

#[test]
fn test_parse_match_fat_arrow_block_allows_blank_before_body() -> Result<(), Vec<CompileError>> {
    let source = r#"
def f() -> int:
  match Err("bad"):
    Ok(x) =>
      return x
    Err(err) =>

      return 0
"#;
    let program = parse_str(source)?;
    let func = match &program.declarations[0].node {
        Declaration::Function(func) => func,
        _ => panic!("Expected function declaration"),
    };
    let match_expr = match &func.body[0].node {
        Statement::Expr(expr) => expr,
        _ => panic!("Expected match expression statement"),
    };
    let arms = match &match_expr.node {
        Expr::Match(_, arms) => arms,
        _ => panic!("Expected match expression"),
    };
    assert_eq!(arms.len(), 2);
    assert!(matches!(arms[1].node.body, MatchBody::Block(ref stmts) if stmts.len() == 1));
    Ok(())
}

#[test]
fn test_parse_match_arm_suite_does_not_inherit_outer_blank_line_intent() -> Result<(), Vec<CompileError>> {
    let source = r#"
def f(result: Result[int, str]) -> int:
  match result:
    Ok(value) => match value:
      Ready(x) => return x

      Failed(err) => return 0

    Err(err) =>
      return 1
"#;
    let program = parse_str(source)?;
    let func = match &program.declarations[0].node {
        Declaration::Function(func) => func,
        _ => panic!("Expected function declaration"),
    };
    let match_expr = match &func.body[0].node {
        Statement::Expr(expr) => expr,
        _ => panic!("Expected match expression statement"),
    };
    let arms = match &match_expr.node {
        Expr::Match(_, arms) => arms,
        _ => panic!("Expected match expression"),
    };
    let err_body = match &arms[1].node.body {
        MatchBody::Block(stmts) => stmts,
        _ => panic!("Expected block match body"),
    };
    assert_eq!(err_body.len(), 1);
    assert_eq!(
        err_body[0].leading_blank_lines, 0,
        "outer Err arm body should not inherit the preserved gap from the nested Ok arm"
    );
    Ok(())
}
