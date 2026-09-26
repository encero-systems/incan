//! The prefix operators `not`, `-` and `~` over their operand shapes: an operator expression is handed to the emitter
//! as one grouped value in its own type, an atomic operand as written (#1763).

use super::*;

/// Return the expression the named function returns.
fn returned_expr<'a>(ir: &'a IrProgram, name: &str) -> Result<&'a TypedExpr, String> {
    let function = ir
        .declarations
        .iter()
        .find_map(|decl| match &decl.kind {
            IrDeclKind::Function(function) if function.name == name => Some(function),
            _ => None,
        })
        .ok_or_else(|| format!("missing function `{name}`"))?;
    match function.body.last() {
        Some(IrStmt {
            kind: IrStmtKind::Return(Some(expr)),
            ..
        }) => Ok(expr),
        other => Err(format!(
            "`{name}` must end in a return of the operator expression, got {other:?}"
        )),
    }
}

/// Return the prefix operator and operand of the expression the named function returns.
fn unary_parts<'a>(ir: &'a IrProgram, name: &str) -> Result<(UnaryOp, &'a TypedExpr), String> {
    match &returned_expr(ir, name)?.kind {
        IrExprKind::UnaryOp { op, operand } => Ok((*op, operand.as_ref())),
        other => Err(format!("`{name}` must lower to a prefix operator, got {other:?}")),
    }
}

/// #1763: `not (1 == 2)` negates the comparison. A prefix operator is spelled directly in front of its operand's
/// tokens, so an operator-shaped operand is handed over as one grouped value in its own type; the group wraps the
/// operator expression itself.
#[test]
fn prefix_operator_over_an_operator_expression_lowers_a_grouped_operand_issue1763() -> Result<(), String> {
    let ir = lower_source(
        r#"
def not_grouped_comparison() -> bool:
    return not (1 == 2)

def not_bare_comparison(x: int, y: int) -> bool:
    return not x == y

def negated_sum(a: int, b: int) -> int:
    return -(a + b)

def not_conjunction(a: bool, b: bool) -> bool:
    return not (a and b)

def not_truthiness(n: int) -> bool:
    return not bool(n)

def inverted_union(a: int, b: int) -> int:
    return ~(a | b)
"#,
    )
    .map_err(|errors| format!("lowering failed: {errors:?}"))?;

    for (name, op, grouped_kind) in [
        ("not_grouped_comparison", UnaryOp::Not, "BinOp"),
        ("not_bare_comparison", UnaryOp::Not, "BinOp"),
        ("negated_sum", UnaryOp::Neg, "BinOp"),
        ("not_conjunction", UnaryOp::Not, "BinOp"),
        ("not_truthiness", UnaryOp::Not, "BuiltinCall"),
        ("inverted_union", UnaryOp::Not, "BinOp"),
    ] {
        let (lowered_op, operand) = unary_parts(&ir, name)?;
        assert_eq!(lowered_op, op, "`{name}` keeps its operator");
        let IrExprKind::Block {
            stmts,
            value: Some(value),
        } = &operand.kind
        else {
            return Err(format!(
                "`{name}` must hand the emitter a grouped operand, got {operand:?}"
            ));
        };
        assert!(stmts.is_empty(), "a grouped operand carries no statements: {stmts:?}");
        assert_eq!(operand.ty, value.ty, "the group keeps the operand's type for `{name}`");
        let wrapped = match &value.kind {
            IrExprKind::BinOp { .. } => "BinOp",
            IrExprKind::BuiltinCall { .. } => "BuiltinCall",
            other => return Err(format!("`{name}` groups an unexpected shape: {other:?}")),
        };
        assert_eq!(
            wrapped, grouped_kind,
            "the group wraps the operator expression itself for `{name}`"
        );
    }
    Ok(())
}

/// A name, a call, a membership test and a nested prefix operator are already one operand: they are left as written
/// so their emission does not change.
#[test]
fn prefix_operator_over_an_atomic_operand_stays_ungrouped_issue1763() -> Result<(), String> {
    let ir = lower_source(
        r#"
def is_small(x: int) -> bool:
    return x < 10

def not_flag(flag: bool) -> bool:
    return not flag

def negated_name(x: int) -> int:
    return -x

def not_call(x: int) -> bool:
    return not is_small(x)

def not_membership(items: List[int]) -> bool:
    return not (1 in items)

def double_negation(x: int) -> int:
    return -(-x)
"#,
    )
    .map_err(|errors| format!("lowering failed: {errors:?}"))?;

    for name in [
        "not_flag",
        "negated_name",
        "not_call",
        "not_membership",
        "double_negation",
    ] {
        let (_, operand) = unary_parts(&ir, name)?;
        assert!(
            !matches!(operand.kind, IrExprKind::Block { .. }),
            "`{name}` is one operand already and must stay as written, got {operand:?}"
        );
    }
    let (_, operand) = unary_parts(&ir, "double_negation")?;
    let IrExprKind::UnaryOp { operand: inner, .. } = &operand.kind else {
        return Err(format!(
            "`double_negation` must nest the prefix operators, got {operand:?}"
        ));
    };
    assert!(
        matches!(inner.kind, IrExprKind::Var { .. }),
        "`double_negation` reaches its name through two bare prefix operators, got {inner:?}"
    );
    Ok(())
}
