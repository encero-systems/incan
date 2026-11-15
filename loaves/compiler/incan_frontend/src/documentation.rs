//! Which statements of a block are documentation.
//!
//! Documentation is a string-literal statement that a later statement of its block, other than `pass`, follows. It has
//! no effect, so the checker checks nothing in it, Body IR leaves it out, and an owner's syntax inventory leaves out
//! the documentation that leads a function's statements. A block's last statement can be its value (#2025), and a
//! docstring before only `pass` marks a documented trait placeholder, so both stay. The records engine's source
//! projection draws the same line.

use crate::ast::{Expr, Literal, Spanned, Statement};

/// Whether `stmt` is documentation, given the statements `rest` that follow it in its block.
pub(crate) fn is_documentation(stmt: &Spanned<Statement>, rest: &[Spanned<Statement>]) -> bool {
    matches!(&stmt.node, Statement::Expr(expr) if matches!(&expr.node, Expr::Literal(Literal::String(_))))
        && rest.iter().any(|later| !matches!(later.node, Statement::Pass))
}

/// How many statements at the start of `block` are documentation.
pub(crate) fn leading_documentation(block: &[Spanned<Statement>]) -> usize {
    (0..block.len())
        .take_while(|&index| is_documentation(&block[index], &block[index + 1..]))
        .count()
}
