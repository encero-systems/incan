//! Block-local reachability for statements that follow an unconditional `return` (#1117).
//!
//! This is the single place the typechecker decides that a statement can never run. It sits at the statement-block
//! boundary rather than inside [`TypeChecker::check_statement`] because reachability is a property of a *sequence*
//! of statements, not of any statement on its own.
//!
//! ## What this deliberately does not do
//!
//! The rule follows a `return` statement within one block and stops there. It does not propagate divergence out of
//! `if`/`else` arms that all return, out of `match`, or through `break`, `continue`, or a diverging call — those
//! need a real control-flow graph, and guessing at them produces false "unreachable" reports on code that runs.
//! Nested blocks are still covered, because every block reaches this boundary and is scanned on its own.

use crate::ast::{Span, Spanned, Statement};
use crate::diagnostics::lints;

use super::TypeChecker;

/// The dead tail of one statement block.
struct UnreachableTail {
    /// The `return` that exits the block before the tail can run.
    return_span: Span,
    /// One span covering every statement after that `return`.
    unreachable_span: Span,
}

/// Locate the statements one block can never reach, if it has any.
///
/// Reports the tail as one region rather than one diagnostic per statement, so deleting a long dead block is a
/// single fix with a single warning attached to it.
fn unreachable_tail(body: &[Spanned<Statement>]) -> Option<UnreachableTail> {
    let return_index = body.iter().position(|stmt| matches!(stmt.node, Statement::Return(_)))?;
    let return_span = body.get(return_index)?.span;
    let (first, rest) = body.get(return_index.checked_add(1)?..)?.split_first()?;

    Some(UnreachableTail {
        return_span,
        unreachable_span: rest.iter().fold(first.span, |region, stmt| region.merge(stmt.span)),
    })
}

impl TypeChecker {
    /// Type-check one statement block, reporting any statements it can never reach.
    ///
    /// Every statement block in the language routes through here — function, method, and property bodies, `if` /
    /// `elif` / `else` arms, loop bodies, `unsafe` bodies, and match-arm blocks — so the reachability rule applies
    /// uniformly and nested blocks are covered without a separate traversal.
    pub fn check_statement_block(&mut self, body: &[Spanned<Statement>]) {
        self.report_unreachable_after_return(body);
        for stmt in body {
            self.check_statement(stmt);
        }
    }

    /// Warn about the block's unreachable tail without checking its statements.
    ///
    /// Separate from [`TypeChecker::check_statement_block`] for the one block form that cannot use it: a block whose
    /// trailing statement doubles as the block's value expression, and so must be checked differently from the rest.
    pub fn report_unreachable_after_return(&mut self, body: &[Spanned<Statement>]) {
        if let Some(tail) = unreachable_tail(body) {
            self.warnings.push(lints::unreachable_code_after_return(
                tail.unreachable_span,
                tail.return_span,
            ));
        }
    }
}
