//! F-string interpolation spans: multiple interpolations, nested expressions, closures, the debug format marker, direct
//! zero-argument calls, method calls with indexing and comprehension filters.

use super::*;

/// Build a structured fixture-shape failure for fallible parser tests.
fn parser_test_error(message: &str) -> Vec<CompileError> {
    vec![CompileError::new(message.to_string(), Span::default())]
}

#[test]
fn test_parse_fstring_expr_spans_multiple_interpolations() -> Result<(), Vec<CompileError>> {
    let source = "def greet(name: str, title: str) -> str:\n  return f\"Hello {title} {name}\"\n";
    let program = parse_str(source)?;

    let function = match &program.declarations[0].node {
        Declaration::Function(f) => f,
        _ => panic!("Expected function"),
    };

    let return_expr = match &function.body[0].node {
        Statement::Return(Some(expr)) => expr,
        _ => panic!("Expected return with expression"),
    };

    let parts = match &return_expr.node {
        Expr::FString(parts) => parts,
        _ => panic!("Expected f-string expression"),
    };

    let first_expected_start = match source.find("{title}") {
        Some(start) => start,
        None => panic!("Could not find first interpolation in source"),
    };
    let second_expected_start = match source.find("{name}") {
        Some(start) => start,
        None => panic!("Could not find second interpolation in source"),
    };

    let first_expr = match &parts[1] {
        FStringPart::Expr { expr, .. } => expr,
        _ => panic!("Expected first interpolation expression"),
    };
    assert_eq!(first_expr.span.start, first_expected_start);
    assert_eq!(first_expr.span.end, first_expected_start + "{title}".len());

    let second_expr = match &parts[3] {
        FStringPart::Expr { expr, .. } => expr,
        _ => panic!("Expected second interpolation expression"),
    };
    assert_eq!(second_expr.span.start, second_expected_start);
    assert_eq!(second_expr.span.end, second_expected_start + "{name}".len());

    Ok(())
}

#[test]
fn test_parse_fstring_expr_span_nested_expression() -> Result<(), Vec<CompileError>> {
    let source = "def calc(x: int, y: int, z: int) -> str:\n  return f\"value: {x + y * z}\"\n";
    let program = parse_str(source)?;

    let function = match &program.declarations[0].node {
        Declaration::Function(f) => f,
        _ => panic!("Expected function"),
    };

    let return_expr = match &function.body[0].node {
        Statement::Return(Some(expr)) => expr,
        _ => panic!("Expected return with expression"),
    };

    let parts = match &return_expr.node {
        Expr::FString(parts) => parts,
        _ => panic!("Expected f-string expression"),
    };

    let expected_start = match source.find("{x + y * z}") {
        Some(start) => start,
        None => panic!("Could not find interpolation in source"),
    };

    let interpolation = match &parts[1] {
        FStringPart::Expr { expr, .. } => expr,
        _ => panic!("Expected interpolation expression"),
    };

    assert_eq!(interpolation.span.start, expected_start);
    assert_eq!(interpolation.span.end, expected_start + "{x + y * z}".len());
    assert!(matches!(interpolation.node, Expr::Binary(_, _, _)));

    Ok(())
}

#[test]
fn test_parse_fstring_closure_parameter_and_body_spans_use_outer_coordinates() -> Result<(), Vec<CompileError>> {
    let source = "def render() -> str:\n  return f\"{((value) => value)(1)}\"\n";
    let program = parse_str(source)?;
    let Declaration::Function(function) = &program.declarations[0].node else {
        return Err(parser_test_error("expected function"));
    };
    let Statement::Return(Some(return_expr)) = &function.body[0].node else {
        return Err(parser_test_error("expected return with expression"));
    };
    let Expr::FString(parts) = &return_expr.node else {
        return Err(parser_test_error("expected f-string expression"));
    };
    let FStringPart::Expr { expr, .. } = &parts[0] else {
        return Err(parser_test_error("expected interpolation expression"));
    };
    let Expr::Call(callee, _, _) = &expr.node else {
        return Err(parser_test_error("expected immediate closure call"));
    };
    let Expr::Paren(closure) = &callee.node else {
        return Err(parser_test_error("expected parenthesized closure callee"));
    };
    let Expr::Closure(params, body) = &closure.node else {
        return Err(parser_test_error("expected closure"));
    };

    let first = require_source_span(source, "value", 0)?.start;
    let second = require_source_span(source, "value", 1)?.start;
    assert_eq!(params[0].span, Span::new(first, first + "value".len()));
    assert_eq!(params[0].node.ty.span, params[0].span);
    assert_eq!(body.span, Span::new(second, second + "value".len()));
    Ok(())
}

#[test]
fn test_parse_fstring_debug_format_marker() -> Result<(), Vec<CompileError>> {
    let source = "def render(columns: list[str]) -> str:\n  return f\"columns: {columns:?}\"\n";
    let program = parse_str(source)?;

    let function = match &program.declarations[0].node {
        Declaration::Function(f) => f,
        _ => panic!("Expected function"),
    };

    let return_expr = match &function.body[0].node {
        Statement::Return(Some(expr)) => expr,
        _ => panic!("Expected return with expression"),
    };

    let parts = match &return_expr.node {
        Expr::FString(parts) => parts,
        _ => panic!("Expected f-string expression"),
    };

    let expected_start = match source.find("{columns:?}") {
        Some(start) => start,
        None => panic!("Could not find interpolation in source"),
    };
    let interpolation = match &parts[1] {
        FStringPart::Expr { expr, format } => {
            assert!(matches!(format, FStringFormat::Debug));
            expr
        }
        _ => panic!("Expected interpolation expression"),
    };

    assert_eq!(interpolation.span.start, expected_start);
    assert_eq!(interpolation.span.end, expected_start + "{columns:?}".len());
    assert!(matches!(interpolation.node, Expr::Ident(ref name) if name == "columns"));

    Ok(())
}

#[test]
/// Verifies that a direct zero-argument call remains an ordinary call expression inside an f-string interpolation.
fn test_parse_fstring_expr_direct_zero_arg_call() -> Result<(), Vec<CompileError>> {
    let source = "def enabled() -> bool:\n  return True\n\ndef render() -> str:\n  return f\"enabled:{enabled()}\"\n";
    let program = parse_str(source)?;
    let render = match &program.declarations[1].node {
        Declaration::Function(function) => function,
        _ => {
            return Err(vec![CompileError::new(
                "parser test internal error: expected render function".to_string(),
                program.declarations[1].span,
            )]);
        }
    };
    let interpolation = match &render.body[0].node {
        Statement::Return(Some(expr)) => match &expr.node {
            Expr::FString(parts) => match &parts[1] {
                FStringPart::Expr { expr, .. } => expr,
                _ => {
                    return Err(vec![CompileError::new(
                        "parser test internal error: expected f-string interpolation".to_string(),
                        expr.span,
                    )]);
                }
            },
            _ => {
                return Err(vec![CompileError::new(
                    "parser test internal error: expected f-string return value".to_string(),
                    expr.span,
                )]);
            }
        },
        _ => {
            return Err(vec![CompileError::new(
                "parser test internal error: expected return expression".to_string(),
                render.body[0].span,
            )]);
        }
    };

    let expected_interpolation_start = match source.rfind("{enabled()}") {
        Some(start) => start,
        None => {
            return Err(vec![CompileError::new(
                "parser test internal error: missing direct-call interpolation".to_string(),
                render.body[0].span,
            )]);
        }
    };
    let expected_callee_start = match source.rfind("enabled()") {
        Some(start) => start,
        None => {
            return Err(vec![CompileError::new(
                "parser test internal error: missing direct-call callee".to_string(),
                render.body[0].span,
            )]);
        }
    };

    assert_eq!(interpolation.span.start, expected_interpolation_start);
    assert_eq!(
        interpolation.span.end,
        expected_interpolation_start + "{enabled()}".len()
    );
    let Expr::Call(callee, type_args, args) = &interpolation.node else {
        return Err(vec![CompileError::new(
            "parser test internal error: expected direct call interpolation".to_string(),
            interpolation.span,
        )]);
    };
    assert!(type_args.is_empty());
    assert!(args.is_empty());
    assert!(matches!(&callee.node, Expr::Ident(name) if name == "enabled"));
    assert_eq!(callee.span.start, expected_callee_start);
    assert_eq!(callee.span.end, expected_callee_start + "enabled".len());
    Ok(())
}

#[test]
fn test_parse_fstring_expr_span_method_call_with_index() -> Result<(), Vec<CompileError>> {
    let source = "def render(users: List[str]) -> str:\n  return f\"user: {users[unknown_idx].upper()}\"\n";
    let program = parse_str(source)?;

    let function = match &program.declarations[0].node {
        Declaration::Function(f) => f,
        _ => panic!("Expected function"),
    };

    let return_expr = match &function.body[0].node {
        Statement::Return(Some(expr)) => expr,
        _ => panic!("Expected return with expression"),
    };

    let parts = match &return_expr.node {
        Expr::FString(parts) => parts,
        _ => panic!("Expected f-string expression"),
    };

    let expected_start = match source.find("{users[unknown_idx].upper()}") {
        Some(start) => start,
        None => panic!("Could not find interpolation in source"),
    };

    let interpolation = match &parts[1] {
        FStringPart::Expr { expr, .. } => expr,
        _ => panic!("Expected interpolation expression"),
    };
    assert_eq!(interpolation.span.start, expected_start);
    assert_eq!(
        interpolation.span.end,
        expected_start + "{users[unknown_idx].upper()}".len()
    );

    let (base, method, args) = match &interpolation.node {
        Expr::MethodCall(base, method, _, args) => (base, method, args),
        _ => panic!("Expected method call interpolation"),
    };
    assert_eq!(method, "upper");
    assert!(args.is_empty());

    let (_, index) = match &base.node {
        Expr::Index(collection, index) => {
            assert!(matches!(collection.node, Expr::Ident(ref name) if name == "users"));
            (collection, index)
        }
        _ => panic!("Expected index expression as method base"),
    };

    let expected_index_start = match source.find("unknown_idx") {
        Some(start) => start,
        None => panic!("Could not find unknown_idx in source"),
    };
    assert!(matches!(index.node, Expr::Ident(ref name) if name == "unknown_idx"));
    assert_eq!(index.span.start, expected_index_start);
    assert_eq!(index.span.end, expected_index_start + "unknown_idx".len());

    Ok(())
}

#[test]
fn test_parse_fstring_expr_span_list_comp_filter_call() -> Result<(), Vec<CompileError>> {
    let source =
        "def render(items: List[int]) -> str:\n  return f\"values: {[x for x in items if unknown_pred(x)]}\"\n";
    let program = parse_str(source)?;

    let function = match &program.declarations[0].node {
        Declaration::Function(f) => f,
        _ => panic!("Expected function"),
    };

    let return_expr = match &function.body[0].node {
        Statement::Return(Some(expr)) => expr,
        _ => panic!("Expected return with expression"),
    };

    let parts = match &return_expr.node {
        Expr::FString(parts) => parts,
        _ => panic!("Expected f-string expression"),
    };

    let expected_start = match source.find("{[x for x in items if unknown_pred(x)]}") {
        Some(start) => start,
        None => panic!("Could not find interpolation in source"),
    };

    let interpolation = match &parts[1] {
        FStringPart::Expr { expr, .. } => expr,
        _ => panic!("Expected interpolation expression"),
    };
    assert_eq!(interpolation.span.start, expected_start);
    assert_eq!(
        interpolation.span.end,
        expected_start + "{[x for x in items if unknown_pred(x)]}".len()
    );

    let comp = match &interpolation.node {
        Expr::ListComp(comp) => comp,
        _ => panic!("Expected list comprehension interpolation"),
    };
    let filter = match &comp.filter {
        Some(filter) => filter,
        None => panic!("Expected list comprehension filter"),
    };
    let callee = match &filter.node {
        Expr::Call(callee, _, _args) => callee,
        _ => panic!("Expected filter call expression"),
    };

    let expected_callee_start = match source.find("unknown_pred") {
        Some(start) => start,
        None => panic!("Could not find unknown_pred in source"),
    };
    assert!(matches!(callee.node, Expr::Ident(ref name) if name == "unknown_pred"));
    assert_eq!(callee.span.start, expected_callee_start);
    assert_eq!(callee.span.end, expected_callee_start + "unknown_pred".len());

    Ok(())
}
