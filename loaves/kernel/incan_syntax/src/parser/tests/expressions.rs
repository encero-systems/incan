//! Expression forms: keyword-named arguments and member access, rest parameters and call unpacking, collection literal
//! spreads, decimal literals, explicit and inferred call type arguments, leading-dot fluent chains, generator
//! expressions, `is not None`, comprehension tuple bindings, `loop` values, `race_for` and RFC 028 operator spellings.

use super::*;

#[test]
fn test_parse_keyword_named_args_and_member_access() -> Result<(), Vec<CompileError>> {
    let source = r#"
def f(a: Foo) -> int:
  let x = Foo(type=1, class=2)
  return a.type
"#;
    let program = parse_str(source)?;
    let func = match &program.declarations[0].node {
        Declaration::Function(func) => func,
        _ => panic!("Expected function"),
    };
    let call_expr = match &func.body[0].node {
        Statement::Assignment(stmt) => match &stmt.value.node {
            Expr::Call(_, _, args) => args,
            _ => panic!("Expected call expression"),
        },
        _ => panic!("Expected assignment statement"),
    };
    assert!(matches!(call_expr[0], CallArg::Named(ref name, _) if name.node == "type"));
    assert!(matches!(call_expr[1], CallArg::Named(ref name, _) if name.node == "class"));
    let type_start = require_source_span(source, "type=1", 0)?.start;
    let class_start = require_source_span(source, "class=2", 0)?.start;
    let CallArg::Named(type_label, _) = &call_expr[0] else {
        unreachable!("first argument shape was asserted above")
    };
    let CallArg::Named(class_label, _) = &call_expr[1] else {
        unreachable!("second argument shape was asserted above")
    };
    assert_eq!(type_label.span, Span::new(type_start, type_start + "type".len()));
    assert_eq!(class_label.span, Span::new(class_start, class_start + "class".len()));
    let return_expr = match &func.body[1].node {
        Statement::Return(Some(expr)) => expr,
        _ => panic!("Expected return"),
    };
    assert!(matches!(&return_expr.node, Expr::Field(_, name) if name == "type"));
    Ok(())
}

#[test]
fn test_parse_rest_params_and_call_unpacking() -> Result<(), Vec<CompileError>> {
    let source = r#"
def collect(prefix: str, *items: int, **labels: str) -> int:
  return 0

def use(xs: list[int], kw: dict[str, str]) -> int:
  return collect("x", 1, *xs, name="demo", **kw)
"#;
    let program = parse_str(source)?;
    let collect = require_function_decl(&program.declarations[0])?;
    assert_eq!(collect.params[0].node.kind, ParamKind::Normal);
    assert_eq!(collect.params[1].node.kind, ParamKind::RestPositional);
    assert_eq!(collect.params[1].node.name, "items");
    assert_eq!(collect.params[2].node.kind, ParamKind::RestKeyword);
    assert_eq!(collect.params[2].node.name, "labels");

    let use_fn = require_function_decl(&program.declarations[1])?;
    let call_args = match &use_fn.body[0].node {
        Statement::Return(Some(expr)) => match &expr.node {
            Expr::Call(_, _, args) => args,
            _ => panic!("expected call expression"),
        },
        _ => panic!("expected return statement"),
    };
    assert!(matches!(call_args[0], CallArg::Positional(_)));
    assert!(matches!(call_args[1], CallArg::Positional(_)));
    assert!(matches!(call_args[2], CallArg::PositionalUnpack(_)));
    assert!(matches!(call_args[3], CallArg::Named(ref name, _) if name.node == "name"));
    assert!(matches!(call_args[4], CallArg::KeywordUnpack(_)));
    Ok(())
}

#[test]
fn test_parse_list_and_dict_literal_spread_entries() -> Result<(), Vec<CompileError>> {
    let source = r#"
def use(xs: list[int], headers: dict[str, str]) -> None:
  values = [1, *xs, 4]
  merged = {"accept": "json", **headers}
"#;
    let program = parse_str(source)?;
    let use_fn = require_function_decl(&program.declarations[0])?;

    let list_entries = match &use_fn.body[0].node {
        Statement::Assignment(stmt) => match &stmt.value.node {
            Expr::List(entries) => entries,
            _ => panic!("expected list literal"),
        },
        _ => panic!("expected assignment statement"),
    };
    assert!(matches!(list_entries[0], ListEntry::Element(_)));
    assert!(matches!(list_entries[1], ListEntry::Spread(_)));
    assert!(matches!(list_entries[2], ListEntry::Element(_)));

    let dict_entries = match &use_fn.body[1].node {
        Statement::Assignment(stmt) => match &stmt.value.node {
            Expr::Dict(entries) => entries,
            _ => panic!("expected dict literal"),
        },
        _ => panic!("expected assignment statement"),
    };
    assert!(matches!(dict_entries[0], DictEntry::Pair(_, _)));
    assert!(matches!(dict_entries[1], DictEntry::Spread(_)));
    Ok(())
}

#[test]
fn test_parse_collection_literal_spread_invalid_markers() {
    let list_errs = parse_str_err(
        "def f(xs: list[int]) -> None:\n  values = [**xs]\n",
        "list literal should reject dictionary spread marker",
    );
    assert!(
        list_errs
            .iter()
            .any(|err| err.message.contains("Invalid list spread marker `**`")),
        "expected invalid list spread marker diagnostic, got: {list_errs:?}"
    );

    let dict_errs = parse_str_err(
        "def f(xs: list[int]) -> None:\n  values = {*xs}\n",
        "dict literal should reject list spread marker",
    );
    assert!(
        dict_errs
            .iter()
            .any(|err| err.message.contains("Invalid dictionary spread marker `*`")),
        "expected invalid dictionary spread marker diagnostic, got: {dict_errs:?}"
    );
}

#[test]
fn test_parse_decimal_literal() -> Result<(), Vec<CompileError>> {
    let source = r#"
const PRICE = 19.99d
"#;
    let program = parse_str(source)?;
    let Declaration::Const(c) = &program.declarations[0].node else {
        panic!("Expected const");
    };
    let Expr::Literal(Literal::Decimal(value)) = &c.value.node else {
        panic!("Expected decimal literal");
    };
    assert_eq!(value.body, "19.99");
    assert_eq!(value.repr, "19.99d");
    Ok(())
}

#[test]
fn test_parse_function_call_with_explicit_type_args() -> Result<(), Vec<CompileError>> {
    let source = "def run() -> int:\n  return id[int](1)\n";
    let program = parse_str(source)?;
    let function = match &program.declarations[0].node {
        Declaration::Function(f) => f,
        _ => panic!("Expected function"),
    };
    let return_expr = match &function.body[0].node {
        Statement::Return(Some(expr)) => expr,
        _ => panic!("Expected return expression"),
    };
    match &return_expr.node {
        Expr::Call(callee, type_args, args) => {
            assert!(matches!(callee.node, Expr::Ident(ref name) if name == "id"));
            assert_eq!(type_args.len(), 1);
            assert!(matches!(type_args[0].node, Type::Simple(ref name) if name == "int"));
            assert_eq!(args.len(), 1);
        }
        other => panic!("Expected explicit-generic call, got {other:?}"),
    }
    Ok(())
}

#[test]
fn test_parse_method_call_with_explicit_type_args() -> Result<(), Vec<CompileError>> {
    let source = "def run(box: Boxed[int]) -> int:\n  return box.get[int]()\n";
    let program = parse_str(source)?;
    let function = match &program.declarations[0].node {
        Declaration::Function(f) => f,
        _ => panic!("Expected function"),
    };
    let return_expr = match &function.body[0].node {
        Statement::Return(Some(expr)) => expr,
        _ => panic!("Expected return expression"),
    };
    match &return_expr.node {
        Expr::MethodCall(base, method, type_args, args) => {
            assert!(matches!(base.node, Expr::Ident(ref name) if name == "box"));
            assert_eq!(method, "get");
            assert_eq!(type_args.len(), 1);
            assert!(matches!(type_args[0].node, Type::Simple(ref name) if name == "int"));
            assert!(args.is_empty());
        }
        other => panic!("Expected explicit-generic method call, got {other:?}"),
    }
    Ok(())
}

#[test]
fn test_parse_indented_leading_dot_fluent_method_chain() -> Result<(), Vec<CompileError>> {
    let source = r#"def run(orders: DataFrame) -> DataFrame:
  enriched = orders
    .with_column("region_norm", col("region"))
    .with_column("status_norm", col("status"))

  paid = enriched.filter(eq(col("status_norm"), "paid"))
  return paid
"#;
    let program = parse_str(source)?;
    let function = require_function_decl(&program.declarations[0])?;
    let assignment = match &function.body[0].node {
        Statement::Assignment(assign) => assign,
        other => panic!("Expected assignment, got {other:?}"),
    };

    let Expr::MethodCall(first_call, second_method, _, second_args) = &assignment.value.node else {
        panic!("Expected outer fluent method call, got {:?}", assignment.value.node);
    };
    assert_eq!(second_method, "with_column");
    assert_eq!(second_args.len(), 2);

    let Expr::MethodCall(root, first_method, _, first_args) = &first_call.node else {
        panic!("Expected nested fluent method call, got {:?}", first_call.node);
    };
    assert_eq!(first_method, "with_column");
    assert_eq!(first_args.len(), 2);
    assert!(matches!(&root.node, Expr::Ident(name) if name == "orders"));

    match &function.body[1].node {
        Statement::Assignment(assign) => assert_eq!(assign.name, "paid"),
        other => panic!("Expected assignment after fluent chain, got {other:?}"),
    }

    match &function.body[2].node {
        Statement::Return(Some(expr)) => assert!(matches!(&expr.node, Expr::Ident(name) if name == "paid")),
        other => panic!("Expected return after fluent chain, got {other:?}"),
    }
    Ok(())
}

#[test]
fn test_parse_indented_leading_dot_fluent_method_chain_allows_comment_lines() -> Result<(), Vec<CompileError>> {
    let source = r#"def run(orders: DataFrame) -> DataFrame:
  enriched = orders
    # a valid placement for a comment
    .with_column("region_norm", col("region"))
    .with_column("status_norm", col("status"))
  return enriched
"#;
    let program = parse_str(source)?;
    let function = require_function_decl(&program.declarations[0])?;
    let assignment = match &function.body[0].node {
        Statement::Assignment(assign) => assign,
        other => panic!("Expected assignment, got {other:?}"),
    };

    let Expr::MethodCall(first_call, second_method, _, _) = &assignment.value.node else {
        panic!("Expected outer fluent method call, got {:?}", assignment.value.node);
    };
    assert_eq!(second_method, "with_column");

    let Expr::MethodCall(root, first_method, _, _) = &first_call.node else {
        panic!("Expected nested fluent method call, got {:?}", first_call.node);
    };
    assert_eq!(first_method, "with_column");
    assert!(matches!(&root.node, Expr::Ident(name) if name == "orders"));
    Ok(())
}

#[test]
fn test_parse_function_call_with_infer_type_arg_placeholder() -> Result<(), Vec<CompileError>> {
    let source = "def run() -> int:\n  return pair_map[int, _](1, 2)\n";
    let program = parse_str(source)?;
    let function = match &program.declarations[0].node {
        Declaration::Function(f) => f,
        _ => panic!("Expected function"),
    };
    let return_expr = match &function.body[0].node {
        Statement::Return(Some(expr)) => expr,
        _ => panic!("Expected return expression"),
    };
    match &return_expr.node {
        Expr::Call(callee, type_args, args) => {
            assert!(matches!(callee.node, Expr::Ident(ref name) if name == "pair_map"));
            assert_eq!(type_args.len(), 2);
            assert!(matches!(type_args[0].node, Type::Simple(ref name) if name == "int"));
            assert!(matches!(type_args[1].node, Type::Infer));
            assert_eq!(args.len(), 2);
        }
        other => panic!("Expected explicit-generic call with infer, got {other:?}"),
    }
    Ok(())
}

#[test]
fn test_parse_method_call_with_infer_type_arg_placeholder() -> Result<(), Vec<CompileError>> {
    let source = "def run(box: Boxed[int]) -> int:\n  return box.unwrap[int, _](0)\n";
    let program = parse_str(source)?;
    let function = match &program.declarations[0].node {
        Declaration::Function(f) => f,
        _ => panic!("Expected function"),
    };
    let return_expr = match &function.body[0].node {
        Statement::Return(Some(expr)) => expr,
        _ => panic!("Expected return expression"),
    };
    match &return_expr.node {
        Expr::MethodCall(base, method, type_args, args) => {
            assert!(matches!(base.node, Expr::Ident(ref name) if name == "box"));
            assert_eq!(method, "unwrap");
            assert_eq!(type_args.len(), 2);
            assert!(matches!(type_args[0].node, Type::Simple(ref name) if name == "int"));
            assert!(matches!(type_args[1].node, Type::Infer));
            assert_eq!(args.len(), 1);
        }
        other => panic!("Expected explicit-generic method call with infer, got {other:?}"),
    }
    Ok(())
}

#[test]
fn test_parse_generator_expression_full_clause_shape() -> Result<(), Vec<CompileError>> {
    let source = "def run(xs: list[int], ys: list[int]) -> Generator[int]:\n  return (x * y for x in xs if x > 0 for y in ys if y > x)\n";
    let program = parse_str(source)?;
    let function = require_function_decl(&program.declarations[0])?;
    let Statement::Return(Some(expr)) = &function.body[0].node else {
        return Err(vec![CompileError::new(
            "parser test internal error: expected return statement".to_string(),
            function.body[0].span,
        )]);
    };
    let Expr::Generator(generator) = &expr.node else {
        return Err(vec![CompileError::new(
            "parser test internal error: expected generator expression".to_string(),
            expr.span,
        )]);
    };

    assert_eq!(generator.clauses.len(), 4);
    let ComprehensionClause::For { pattern, iter } = &generator.clauses[0] else {
        return Err(vec![CompileError::new(
            "parser test internal error: expected first for clause".to_string(),
            expr.span,
        )]);
    };
    assert_eq!(pattern.node, Pattern::Binding("x".to_string()));
    assert!(matches!(iter.node, Expr::Ident(ref name) if name == "xs"));
    assert!(matches!(generator.clauses[1], ComprehensionClause::If(_)));
    let ComprehensionClause::For { pattern, iter } = &generator.clauses[2] else {
        return Err(vec![CompileError::new(
            "parser test internal error: expected second for clause".to_string(),
            expr.span,
        )]);
    };
    assert_eq!(pattern.node, Pattern::Binding("y".to_string()));
    assert!(matches!(iter.node, Expr::Ident(ref name) if name == "ys"));
    assert!(matches!(generator.clauses[3], ComprehensionClause::If(_)));
    Ok(())
}

#[test]
fn test_parse_generator_expression_tuple_unpack_binding() -> Result<(), Vec<CompileError>> {
    let source = "def names(xs: list[str]) -> Generator[str]:\n  return (name for idx, name in enumerate(xs))\n";
    let program = parse_str(source)?;
    let function = require_function_decl(&program.declarations[0])?;
    let Statement::Return(Some(expr)) = &function.body[0].node else {
        return Err(vec![CompileError::new(
            "parser test internal error: expected return statement".to_string(),
            function.body[0].span,
        )]);
    };
    let Expr::Generator(generator) = &expr.node else {
        return Err(vec![CompileError::new(
            "parser test internal error: expected generator expression".to_string(),
            expr.span,
        )]);
    };

    let ComprehensionClause::For { pattern, .. } = &generator.clauses[0] else {
        return Err(vec![CompileError::new(
            "parser test internal error: expected for clause".to_string(),
            expr.span,
        )]);
    };
    let Pattern::Tuple(items) = &pattern.node else {
        return Err(vec![CompileError::new(
            "parser test internal error: expected tuple binding pattern".to_string(),
            pattern.span,
        )]);
    };
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].node, Pattern::Binding("idx".to_string()));
    assert_eq!(items[1].node, Pattern::Binding("name".to_string()));
    Ok(())
}

#[test]
fn test_parse_generator_function_yield_compatibility() -> Result<(), Vec<CompileError>> {
    let source = "def count() -> Generator[int]:\n  yield 1\n";
    let program = parse_str(source)?;
    let function = require_function_decl(&program.declarations[0])?;
    if !matches!(
        &function.body[0].node,
        Statement::Expr(expr) if matches!(expr.node, Expr::Yield(Some(_)))
    ) {
        return Err(vec![CompileError::new(
            "parser test internal error: expected yield expression statement".to_string(),
            function.body[0].span,
        )]);
    };
    Ok(())
}

#[test]
fn test_is_not_none_parses_as_identity_negation() -> Result<(), Vec<CompileError>> {
    let source = r#"
def has_name(name: str | None) -> bool:
  if name is not None:
    return true
  return false
"#;
    let program = parse_str(source)?;
    let function = require_function_decl(&program.declarations[0])?;
    let Statement::If(if_stmt) = &function.body[0].node else {
        panic!("Expected first statement to be if, got: {:?}", function.body[0].node);
    };
    let Condition::Expr(condition) = &if_stmt.condition else {
        panic!("Expected expression condition");
    };
    match &condition.node {
        Expr::Binary(left, BinaryOp::IsNot, right) => {
            assert!(matches!(left.node, Expr::Ident(ref name) if name == "name"));
            assert!(matches!(right.node, Expr::Literal(Literal::None)));
        }
        other => panic!("Expected `is not None` binary expression, got: {other:?}"),
    }
    Ok(())
}

#[test]
fn test_parse_list_comprehension_tuple_unpack_binding() {
    let source = "def names(xs: list[str]) -> list[str]:\n  return [name for idx, name in enumerate(xs)]\n";
    let program = match parse_str(source) {
        Ok(program) => program,
        Err(errs) => panic!("list comprehension tuple-unpack binding should parse: {errs:?}"),
    };
    let Declaration::Function(function) = &program.declarations[0].node else {
        panic!("expected function declaration");
    };
    let Statement::Return(Some(expr)) = &function.body[0].node else {
        panic!("expected return statement");
    };
    let Expr::ListComp(comp) = &expr.node else {
        panic!("expected list comprehension");
    };
    let Pattern::Tuple(items) = &comp.pattern.node else {
        panic!("expected tuple binding pattern");
    };

    assert_eq!(items.len(), 2);
    assert_eq!(items[0].node, Pattern::Binding("idx".to_string()));
    assert_eq!(items[1].node, Pattern::Binding("name".to_string()));
}

#[test]
fn test_parse_loop_expression_with_break_value() -> Result<(), Vec<CompileError>> {
    let source = r#"
def run() -> int:
  return loop:
    break 1
"#;
    let program = parse_str(source)?;
    let function = require_function_decl(&program.declarations[0])?;
    let Statement::Return(Some(expr)) = &function.body[0].node else {
        return Err(vec![CompileError::new(
            "expected return statement with loop expression".to_string(),
            function.body[0].span,
        )]);
    };
    let Expr::Loop(loop_expr) = &expr.node else {
        return Err(vec![CompileError::new(
            "expected loop expression".to_string(),
            expr.span,
        )]);
    };
    let Statement::Break(Some(value)) = &loop_expr.body[0].node else {
        return Err(vec![CompileError::new(
            "expected break with value inside loop expression".to_string(),
            loop_expr.body[0].span,
        )]);
    };
    assert!(matches!(value.node, Expr::Literal(Literal::Int(_))));
    Ok(())
}

#[test]
fn test_parse_race_for_expression_requires_std_async_activation() -> Result<(), Vec<CompileError>> {
    let source = r#"
def run() -> int:
  race = 1
  return race
"#;
    let program = parse_str(source)?;
    let function = require_function_decl(&program.declarations[0])?;
    let Statement::Assignment(assign) = &function.body[0].node else {
        return Err(vec![CompileError::new(
            "parser test internal error: expected assignment".to_string(),
            function.body[0].span,
        )]);
    };
    assert_eq!(assign.name, "race");
    let Statement::Return(Some(expr)) = &function.body[1].node else {
        return Err(vec![CompileError::new(
            "parser test internal error: expected return statement".to_string(),
            function.body[1].span,
        )]);
    };
    assert!(matches!(&expr.node, Expr::Ident(name) if name == "race"));
    Ok(())
}

#[test]
fn test_parse_active_race_for_expression_surface_shape() -> Result<(), Vec<CompileError>> {
    let source = r#"
import std.async

async def run() -> int:
  return race for value:
    await fast() => value
    await slow() =>
      return value
"#;
    let program = parse_str(source)?;
    let function = require_function_decl(&program.declarations[1])?;
    let Statement::Return(Some(expr)) = &function.body[0].node else {
        return Err(vec![CompileError::new(
            "parser test internal error: expected return statement".to_string(),
            function.body[0].span,
        )]);
    };
    let Expr::Surface(surface) = &expr.node else {
        return Err(vec![CompileError::new(
            "parser test internal error: expected surface expression".to_string(),
            expr.span,
        )]);
    };
    let SurfaceExprPayload::RaceFor(race) = &surface.payload else {
        return Err(vec![CompileError::new(
            "parser test internal error: expected race-for payload".to_string(),
            expr.span,
        )]);
    };

    assert_eq!(race.binding.node, "value");
    let binding_start = require_source_span(source, "race for value", 0)?.start + "race for ".len();
    assert_eq!(
        race.binding.span,
        Span::new(binding_start, binding_start + "value".len())
    );
    assert_eq!(race.arms.len(), 2);
    assert!(
        matches!(&race.arms[0].awaitable.node, Expr::Call(callee, _, _) if matches!(&callee.node, Expr::Ident(name) if name == "fast"))
    );
    assert!(
        matches!(&race.arms[0].body, RaceForBody::Expr(body) if matches!(&body.node, Expr::Ident(name) if name == "value"))
    );
    assert!(
        matches!(&race.arms[1].awaitable.node, Expr::Call(callee, _, _) if matches!(&callee.node, Expr::Ident(name) if name == "slow"))
    );
    assert!(
        matches!(&race.arms[1].body, RaceForBody::Block(stmts) if matches!(&stmts[0].node, Statement::Return(Some(value)) if matches!(&value.node, Expr::Ident(name) if name == "value")))
    );
    Ok(())
}

#[test]
fn test_parse_race_for_rejects_pattern_binding_header() {
    let source = r#"
import std.async

async def run() -> int:
  return race for (value):
    await fast() => value
"#;
    let errors = parse_str(source).expect_err("pattern race header should fail");
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("Pattern-binding race headers are not supported")),
        "expected pattern-binding diagnostic, got: {errors:?}"
    );
}

#[test]
fn test_parse_race_for_rejects_default_arm() {
    let source = r#"
import std.async

async def run() -> int:
  return race for value:
    default => value
"#;
    let errors = parse_str(source).expect_err("default race arm should fail");
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("Default race arms are not supported")),
        "expected default-arm diagnostic, got: {errors:?}"
    );
}

#[test]
fn test_parse_race_for_rejects_guard_arm() {
    let source = r#"
import std.async

async def run() -> int:
  return race for value:
    await fast() if value > 0 => value
"#;
    let errors = parse_str(source).expect_err("guarded race arm should fail");
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("Race arm guards are not supported")),
        "expected guard diagnostic, got: {errors:?}"
    );
}

#[test]
fn test_parse_race_for_rejects_fairness_control() {
    let source = r#"
import std.async

async def run() -> int:
  return race for value:
    fair await fast() => value
"#;
    let errors = parse_str(source).expect_err("fairness-controlled race arm should fail");
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("Race fairness controls are not supported")),
        "expected fairness diagnostic, got: {errors:?}"
    );
}

#[test]
fn test_parse_rfc028_operator_spellings() -> Result<(), Vec<CompileError>> {
    let source = r#"
def ops(a: Any, b: Any, c: Any) -> None:
  mat = a @ b
  piped = a |> b <| c
  bits = a & b | c ^ a << b >> c
  inv = ~a
"#;
    let program = parse_str(source)?;
    let function = require_function_decl(&program.declarations[0])?;

    let Statement::Assignment(mat) = &function.body[0].node else {
        return Err(vec![CompileError::new(
            "parser test internal error: expected mat assignment".to_string(),
            function.body[0].span,
        )]);
    };
    assert!(matches!(mat.value.node, Expr::Binary(_, BinaryOp::MatMul, _)));

    let Statement::Assignment(piped) = &function.body[1].node else {
        return Err(vec![CompileError::new(
            "parser test internal error: expected piped assignment".to_string(),
            function.body[1].span,
        )]);
    };
    let Expr::Binary(left, BinaryOp::PipeBackward, _) = &piped.value.node else {
        return Err(vec![CompileError::new(
            "parser test internal error: expected pipe-backward expression".to_string(),
            piped.value.span,
        )]);
    };
    assert!(matches!(left.node, Expr::Binary(_, BinaryOp::PipeForward, _)));

    let Statement::Assignment(bits) = &function.body[2].node else {
        return Err(vec![CompileError::new(
            "parser test internal error: expected bits assignment".to_string(),
            function.body[2].span,
        )]);
    };
    let Expr::Binary(_, BinaryOp::BitOr, right) = &bits.value.node else {
        return Err(vec![CompileError::new(
            "parser test internal error: expected bit-or expression".to_string(),
            bits.value.span,
        )]);
    };
    assert!(matches!(right.node, Expr::Binary(_, BinaryOp::BitXor, _)));

    let Statement::Assignment(inv) = &function.body[3].node else {
        return Err(vec![CompileError::new(
            "parser test internal error: expected inv assignment".to_string(),
            function.body[3].span,
        )]);
    };
    assert!(matches!(inv.value.node, Expr::Unary(UnaryOp::Invert, _)));
    Ok(())
}

#[test]
fn test_parse_rfc028_compound_assignment_spellings() -> Result<(), Vec<CompileError>> {
    let source = r#"
def update(x: Any, y: Any) -> None:
  x @= y
  x &= y
  x |= y
  x ^= y
  x <<= y
  x >>= y
"#;
    let program = parse_str(source)?;
    let function = require_function_decl(&program.declarations[0])?;
    let expected = [
        CompoundOp::MatMul,
        CompoundOp::BitAnd,
        CompoundOp::BitOr,
        CompoundOp::BitXor,
        CompoundOp::Shl,
        CompoundOp::Shr,
    ];
    for (stmt, op) in function.body.iter().zip(expected) {
        assert!(
            matches!(&stmt.node, Statement::CompoundAssignment(assign) if assign.op == op),
            "expected compound assignment {op:?}, got {:?}",
            stmt.node
        );
    }
    Ok(())
}

#[test]
fn test_parse_matmul_preserves_decorator_and_rust_import_at() -> Result<(), Vec<CompileError>> {
    let source = r#"
from rust::libm @ "0.2" import sqrt

@derive(Clone)
class Tensor:
  def apply(self, other: Tensor) -> Tensor:
    return self @ other
"#;
    let program = parse_str(source)?;
    let class = require_class_decl(&program.declarations[1])?;
    assert_eq!(class.decorators.len(), 1);
    let Some(body) = class.methods[0].node.body.as_ref() else {
        return Err(vec![CompileError::new(
            "parser test internal error: expected concrete method body".to_string(),
            class.methods[0].span,
        )]);
    };
    let Statement::Return(Some(expr)) = &body[0].node else {
        return Err(vec![CompileError::new(
            "parser test internal error: expected return statement".to_string(),
            body[0].span,
        )]);
    };
    assert!(matches!(expr.node, Expr::Binary(_, BinaryOp::MatMul, _)));
    Ok(())
}
