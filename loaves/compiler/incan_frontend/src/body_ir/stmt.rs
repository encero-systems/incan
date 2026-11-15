//! Lowering for statements and every assignment form: field, index, compound, tuple, and chained.

use super::refusals::*;
use super::*;

mod assignments;
mod blocks_and_control;
mod compound_and_multi_target;

/// Record a numeric target's type for an integer literal, negated or parenthesized or not, as the legacy backend's
/// `retype_chained_integer_literals` rewrites one evaluation of a chained value: `n = f = 1` writes `1` to the `int`
/// and `1.0` to the `float`. Any other node, and any non-numeric target, keeps the checker's recorded type.
fn chained_integer_literal_types(
    value: &ast::Spanned<ast::Expr>,
    target: &IncanType,
    overrides: &mut HashMap<crate::ast::Span, IncanType>,
) {
    if !matches!(
        target,
        IncanType::Primitive(IncanPrimitiveType::Int | IncanPrimitiveType::Float | IncanPrimitiveType::Numeric(_))
    ) {
        return;
    }
    match &value.node {
        ast::Expr::Paren(inner) => chained_integer_literal_types(inner, target, overrides),
        ast::Expr::Literal(ast::Literal::Int(_)) => {
            overrides.insert(value.span, target.clone());
        }
        ast::Expr::Unary(ast::UnaryOp::Neg, operand)
            if matches!(operand.node, ast::Expr::Literal(ast::Literal::Int(_))) =>
        {
            overrides.insert(value.span, target.clone());
            overrides.insert(operand.span, target.clone());
        }
        _ => {}
    }
}

/// Return whether two types are the same kind of container: both `Option`, `List`, `Set` or `Dict`, or tuples of one
/// length, the only nodes a chained value's per-target evaluation retypes wholesale.
fn same_container_kind(left: &IncanType, right: &IncanType) -> bool {
    if let (IncanType::Tuple(left), IncanType::Tuple(right)) = (left, right) {
        return left.len() == right.len();
    }
    match (left.collection(), right.collection()) {
        (Some((left, _)), Some((right, _))) => {
            left == right
                && matches!(
                    left,
                    CollectionTypeId::Option | CollectionTypeId::List | CollectionTypeId::Set | CollectionTypeId::Dict
                )
        }
        _ => false,
    }
}

/// Whether a block can leave the loop that encloses it before its last statement: a `break` or `continue` that no loop
/// nested in the block owns. A statement whose nested bodies this does not inspect counts as leaving, so a read is
/// only transferred on a path that is proved to return.
fn may_leave_enclosing_loop(stmts: &[ast::Spanned<ast::Statement>]) -> bool {
    stmts.iter().any(|stmt| match &stmt.node {
        ast::Statement::Break(_) | ast::Statement::Continue => true,
        ast::Statement::If(branch) => {
            may_leave_enclosing_loop(&branch.then_body)
                || branch
                    .elif_branches
                    .iter()
                    .any(|(_, body)| may_leave_enclosing_loop(body))
                || branch.else_body.as_deref().is_some_and(may_leave_enclosing_loop)
        }
        // A nested loop owns its own `break` and `continue`.
        ast::Statement::Loop(_) | ast::Statement::While(_) | ast::Statement::For(_) => false,
        ast::Statement::Expr(expr) => matches!(expr.node, ast::Expr::Match(..)),
        ast::Statement::Unsafe(_)
        | ast::Statement::Surface(_)
        | ast::Statement::VocabBlock(_)
        | ast::Statement::VocabExpressionItem(_) => true,
        _ => false,
    })
}
