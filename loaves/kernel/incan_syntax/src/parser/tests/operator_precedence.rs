//! Operator precedence: how the parser groups mixed operators, checked against the precedence and associativity the
//! operator registry documents (`incan_lang::lang::operators`). The parser's ladder does not read the registry, so
//! these tests are what keep the generated reference table from drifting away from what the parser does (#1786).

use super::*;
use incan_lang::lang::operators::{self, Associativity};

/// Which operand of a grouped expression holds the operator being checked.
#[derive(Debug, Clone, Copy)]
enum Side {
    Left,
    Right,
    Operand,
}

/// Parse `x = <expression>` inside a function and return the assigned expression.
fn parse_value(expression: &str) -> Result<Spanned<Expr>, Vec<CompileError>> {
    let source = format!("def f(a: Any, b: Any, c: Any) -> None:\n  x = {expression}\n");
    let program = parse_str(&source)?;
    let func = require_function_decl(&program.declarations[0])?;
    match func.body.first().map(|stmt| &stmt.node) {
        Some(Statement::Assignment(assignment)) => Ok(assignment.value.clone()),
        _ => Err(vec![CompileError::new(
            format!("parser test internal error: expected `x = {expression}` to parse as one assignment"),
            Span::default(),
        )]),
    }
}

/// Name the registry operator at the root of `expr`, or `None` when the registry has no row for it (prefix `-`, a
/// primary expression).
fn root_operator(expr: &Expr) -> Option<OperatorId> {
    match expr {
        Expr::Binary(_, op, _) => Some(match op {
            BinaryOp::Or => OperatorId::Or,
            BinaryOp::And => OperatorId::And,
            BinaryOp::Eq => OperatorId::EqEq,
            BinaryOp::NotEq => OperatorId::NotEq,
            BinaryOp::Lt => OperatorId::Lt,
            BinaryOp::LtEq => OperatorId::LtEq,
            BinaryOp::Gt => OperatorId::Gt,
            BinaryOp::GtEq => OperatorId::GtEq,
            BinaryOp::In | BinaryOp::NotIn => OperatorId::In,
            BinaryOp::Is | BinaryOp::IsNot => OperatorId::Is,
            BinaryOp::PipeForward => OperatorId::PipeForward,
            BinaryOp::PipeBackward => OperatorId::PipeBackward,
            BinaryOp::BitOr => OperatorId::Pipe,
            BinaryOp::BitXor => OperatorId::Caret,
            BinaryOp::BitAnd => OperatorId::Amp,
            BinaryOp::Shl => OperatorId::Shl,
            BinaryOp::Shr => OperatorId::Shr,
            BinaryOp::Add => OperatorId::Plus,
            BinaryOp::Sub => OperatorId::Minus,
            BinaryOp::Mul => OperatorId::Star,
            BinaryOp::Div => OperatorId::Slash,
            BinaryOp::FloorDiv => OperatorId::SlashSlash,
            BinaryOp::Mod => OperatorId::Percent,
            BinaryOp::MatMul => OperatorId::MatMul,
            BinaryOp::Pow => OperatorId::StarStar,
        }),
        Expr::Unary(UnaryOp::Not, _) => Some(OperatorId::Not),
        Expr::Unary(UnaryOp::Invert, _) => Some(OperatorId::Tilde),
        Expr::Range { inclusive: true, .. } => Some(OperatorId::DotDotEq),
        Expr::Range { inclusive: false, .. } => Some(OperatorId::DotDot),
        _ => None,
    }
}

/// Return one operand of a binary, range or prefix expression.
fn operand(expr: &Expr, side: Side) -> Option<&Expr> {
    match (expr, side) {
        (Expr::Binary(left, _, _), Side::Left) => Some(&left.node),
        (Expr::Binary(_, _, right), Side::Right) => Some(&right.node),
        (Expr::Range { start, .. }, Side::Left) => Some(&start.node),
        (Expr::Range { end, .. }, Side::Right) => Some(&end.node),
        (Expr::Unary(_, inner), Side::Operand) => Some(&inner.node),
        _ => None,
    }
}

#[test]
fn test_each_precedence_level_groups_tighter_than_the_one_below_it_issue1786() -> Result<(), Vec<CompileError>> {
    // One case per step of the ladder, loosest first: the root operator, the side that holds the tighter operator,
    // and that operator. The registry must rank the tighter operator strictly higher.
    let cases = [
        ("a or b and c", OperatorId::Or, Side::Right, OperatorId::And),
        ("not a and b", OperatorId::And, Side::Left, OperatorId::Not),
        ("not a == b", OperatorId::Not, Side::Operand, OperatorId::EqEq),
        ("a in b and c", OperatorId::And, Side::Left, OperatorId::In),
        ("a is b or c", OperatorId::Or, Side::Left, OperatorId::Is),
        ("a..b == c", OperatorId::EqEq, Side::Left, OperatorId::DotDot),
        ("a | b..c", OperatorId::DotDot, Side::Left, OperatorId::Pipe),
        ("a | b ^ c", OperatorId::Pipe, Side::Right, OperatorId::Caret),
        ("a ^ b & c", OperatorId::Caret, Side::Right, OperatorId::Amp),
        ("a & b << c", OperatorId::Amp, Side::Right, OperatorId::Shl),
        ("a << b + c", OperatorId::Shl, Side::Right, OperatorId::Plus),
        ("a + b * c", OperatorId::Plus, Side::Right, OperatorId::Star),
        ("a * b ** c", OperatorId::Star, Side::Right, OperatorId::StarStar),
        ("~a * b", OperatorId::Star, Side::Left, OperatorId::Tilde),
        ("~a ** b", OperatorId::Tilde, Side::Operand, OperatorId::StarStar),
    ];
    for (expression, root, side, tighter) in cases {
        let value = parse_value(expression)?;
        assert_eq!(
            root_operator(&value.node),
            Some(root),
            "`{expression}` groups under {root:?}"
        );
        assert_eq!(
            operand(&value.node, side).and_then(root_operator),
            Some(tighter),
            "`{expression}` holds {tighter:?} on its {side:?} side"
        );
        let root_precedence = operators::info_for(root).precedence;
        let tighter_precedence = operators::info_for(tighter).precedence;
        assert!(
            root_precedence < tighter_precedence,
            "the registry ranks {root:?} at {root_precedence} and {tighter:?} at {tighter_precedence}, but the parser \
             groups `{expression}` with {tighter:?} binding tighter"
        );
    }
    Ok(())
}

#[test]
fn test_comparison_level_operators_share_one_precedence_and_group_left_issue1786() -> Result<(), Vec<CompileError>> {
    // `in`, `is` and the pipes are read at the comparison level, so a chain of them groups left to right.
    let cases = [
        ("a in b == c", OperatorId::EqEq, OperatorId::In),
        ("a == b in c", OperatorId::In, OperatorId::EqEq),
        ("a is b != c", OperatorId::NotEq, OperatorId::Is),
        ("a == b |> c", OperatorId::PipeForward, OperatorId::EqEq),
    ];
    for (expression, root, left) in cases {
        let value = parse_value(expression)?;
        assert_eq!(
            root_operator(&value.node),
            Some(root),
            "`{expression}` groups under {root:?}"
        );
        assert_eq!(
            operand(&value.node, Side::Left).and_then(root_operator),
            Some(left),
            "`{expression}` holds {left:?} on its left side"
        );
        assert_eq!(
            operators::info_for(root).precedence,
            operators::info_for(left).precedence,
            "the registry ranks {root:?} and {left:?} apart, but the parser reads them at one level"
        );
        assert_eq!(operators::info_for(root).associativity, Associativity::Left);
    }
    Ok(())
}

/// Return the operand of a prefix `-` at the root of `value`.
fn negated_operand<'e>(value: &'e Spanned<Expr>, what: &str) -> Result<&'e Expr, Vec<CompileError>> {
    match &value.node {
        Expr::Unary(UnaryOp::Neg, operand) => Ok(&operand.node),
        _ => Err(vec![CompileError::new(
            format!("parser test internal error: expected {what} to group under a prefix `-`"),
            value.span,
        )]),
    }
}

#[test]
fn test_power_binds_tighter_than_a_prefix_operator_on_its_left_and_looser_on_its_right_issue1786()
-> Result<(), Vec<CompileError>> {
    // `-a ** b` is `-(a ** b)` and `~a ** b` is `~(a ** b)`; the exponent is read at the prefix level, so
    // `a ** -b` is `a ** (-b)`; `**` stays right-associative.
    let negated = parse_value("-a ** b")?;
    assert!(matches!(
        negated_operand(&negated, "`-a ** b`")?,
        Expr::Binary(_, BinaryOp::Pow, _)
    ));

    let inverted = parse_value("~a ** b")?;
    assert!(matches!(
        &inverted.node,
        Expr::Unary(UnaryOp::Invert, operand) if matches!(operand.node, Expr::Binary(_, BinaryOp::Pow, _))
    ));

    let negative_exponent = parse_value("a ** -b")?;
    assert!(matches!(
        &negative_exponent.node,
        Expr::Binary(_, BinaryOp::Pow, exponent) if matches!(exponent.node, Expr::Unary(UnaryOp::Neg, _))
    ));

    let right_associative = parse_value("a ** b ** c")?;
    assert!(matches!(
        &right_associative.node,
        Expr::Binary(_, BinaryOp::Pow, exponent) if matches!(exponent.node, Expr::Binary(_, BinaryOp::Pow, _))
    ));

    // A prefix operator still binds tighter than `*`, and parentheses keep a negated base.
    let product = parse_value("-a * b")?;
    assert!(matches!(
        &product.node,
        Expr::Binary(left, BinaryOp::Mul, _) if matches!(left.node, Expr::Unary(UnaryOp::Neg, _))
    ));
    let negated_base = parse_value("(-a) ** b")?;
    assert!(matches!(
        &negated_base.node,
        Expr::Binary(base, BinaryOp::Pow, _) if !matches!(base.node, Expr::Binary(..))
    ));
    Ok(())
}

/// Return the operand of an `await` at the root of `expr`.
fn awaited_operand(expr: &Expr) -> Option<&Expr> {
    match expr {
        Expr::Surface(surface) => match &surface.payload {
            SurfaceExprPayload::PrefixUnary(operand) => Some(&operand.node),
            _ => None,
        },
        _ => None,
    }
}

#[test]
fn test_power_groups_with_await_and_a_prefix_exponent_issue1786() -> Result<(), Vec<CompileError>> {
    // `await` binds tighter than `**`; a prefix operator after `await` reads a whole prefix expression, and so does a
    // prefix operator in an exponent.
    let source = r#"
from std.async.time import sleep

async def f(x: Any) -> None:
  a = await x ** 2
  b = await -x ** 2
  c = 2 ** -x ** 2
"#;
    let program = parse_str(source)?;
    let func = require_function_decl(&program.declarations[1])?;
    let values = func
        .body
        .iter()
        .map(|stmt| match &stmt.node {
            Statement::Assignment(assignment) => Ok(&assignment.value.node),
            other => Err(vec![CompileError::new(
                format!("parser test internal error: expected an assignment, got {other:?}"),
                stmt.span,
            )]),
        })
        .collect::<Result<Vec<_>, _>>()?;
    let [awaited_power, awaited_negation, negated_exponent] = values.as_slice() else {
        return Err(vec![CompileError::new(
            "parser test internal error: expected three assignments".to_string(),
            Span::default(),
        )]);
    };

    // `await x ** 2` is `(await x) ** 2`.
    assert!(matches!(
        awaited_power,
        Expr::Binary(base, BinaryOp::Pow, _) if awaited_operand(&base.node).is_some()
    ));
    // `await -x ** 2` is `await (-(x ** 2))`.
    assert!(matches!(
        awaited_operand(awaited_negation),
        Some(Expr::Unary(UnaryOp::Neg, operand)) if matches!(operand.node, Expr::Binary(_, BinaryOp::Pow, _))
    ));
    // `2 ** -x ** 2` is `2 ** (-(x ** 2))`.
    assert!(matches!(
        negated_exponent,
        Expr::Binary(_, BinaryOp::Pow, exponent)
            if matches!(&exponent.node, Expr::Unary(UnaryOp::Neg, operand)
                if matches!(operand.node, Expr::Binary(_, BinaryOp::Pow, _)))
    ));
    Ok(())
}

#[test]
fn test_prefix_operators_nest_to_the_right_issue1786() -> Result<(), Vec<CompileError>> {
    // `not not a` is `not (not a)`, the associativity the registry records for every prefix operator.
    let value = parse_value("not not a")?;
    assert!(matches!(
        &value.node,
        Expr::Unary(UnaryOp::Not, operand) if matches!(operand.node, Expr::Unary(UnaryOp::Not, _))
    ));
    for id in [OperatorId::Not, OperatorId::Tilde] {
        assert_eq!(operators::info_for(id).associativity, Associativity::Right);
    }
    Ok(())
}

#[test]
fn test_ranges_do_not_chain_issue1786() -> Result<(), Vec<CompileError>> {
    // The registry records `..` and `..=` as non-associative: the parser reads one range per operand position.
    if parse_str("def f(a: int, b: int, c: int) -> None:\n  x = a..b..c\n").is_ok() {
        return Err(vec![CompileError::new(
            "parser test internal error: a chained range must not parse".to_string(),
            Span::default(),
        )]);
    }
    for id in [OperatorId::DotDot, OperatorId::DotDotEq] {
        assert_eq!(operators::info_for(id).associativity, Associativity::None);
    }
    Ok(())
}
