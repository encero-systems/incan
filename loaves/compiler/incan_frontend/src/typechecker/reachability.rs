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

use crate::ast::{Expr, Span, Spanned, Statement};
use crate::ast_walk::{any_expr_in_body, any_expr_in_condition, any_expr_in_expr};
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

/// Return the statements of one block that can run: every statement up to and including the block's first `return`.
fn reachable_prefix(body: &[Spanned<Statement>]) -> &[Spanned<Statement>] {
    match body.iter().position(|stmt| matches!(stmt.node, Statement::Return(_))) {
        Some(return_index) => body.get(..=return_index).unwrap_or(body),
        None => body,
    }
}

/// Whether a statement block contains a `yield` that a run of the block can reach.
///
/// RFC 006 requires a generator function to have a reachable `yield`. Reachability is the block-local rule above: a
/// statement after an unconditional `return` in its own block never runs, and every nested statement block is scanned
/// under the same rule. An expression-level block (a `match` arm body, an `if` expression) is scanned whole.
pub(super) fn body_has_reachable_yield(body: &[Spanned<Statement>]) -> bool {
    reachable_prefix(body).iter().any(statement_has_reachable_yield)
}

/// Whether one statement contains a reachable `yield`: in its own expressions, or in a reachable statement of a block
/// it holds.
fn statement_has_reachable_yield(stmt: &Spanned<Statement>) -> bool {
    let is_yield = |expr: &Expr| matches!(expr, Expr::Yield(_));
    match &stmt.node {
        Statement::If(if_stmt) => {
            any_expr_in_condition(&if_stmt.condition, is_yield)
                || body_has_reachable_yield(&if_stmt.then_body)
                || if_stmt.elif_branches.iter().any(|(condition, body)| {
                    any_expr_in_expr(&condition.node, is_yield) || body_has_reachable_yield(body)
                })
                || if_stmt.else_body.as_deref().is_some_and(body_has_reachable_yield)
        }
        Statement::While(while_stmt) => {
            any_expr_in_condition(&while_stmt.condition, is_yield) || body_has_reachable_yield(&while_stmt.body)
        }
        Statement::For(for_stmt) => {
            any_expr_in_expr(&for_stmt.iter.node, is_yield) || body_has_reachable_yield(&for_stmt.body)
        }
        Statement::Loop(loop_stmt) => body_has_reachable_yield(&loop_stmt.body),
        Statement::Unsafe(unsafe_stmt) => body_has_reachable_yield(&unsafe_stmt.body),
        _ => any_expr_in_body(std::slice::from_ref(stmt), is_yield),
    }
}

impl TypeChecker {
    /// Type-check one statement block, reporting any statements it can never reach.
    ///
    /// Every statement block in the language routes through here — function, method, and property bodies, `if` /
    /// `elif` / `else` arms, loop bodies, `unsafe` bodies, and match-arm blocks — so the reachability rule applies
    /// uniformly and nested blocks are covered without a separate traversal. The same boundary gives each block an
    /// identity for the bindings it declares, so a binding nothing reads can be reported when its own block ends
    /// (#1720): a block is where a local's readers stop being possible.
    pub fn check_statement_block(&mut self, body: &[Spanned<Statement>]) {
        let enclosing_block = self.current_statement_block;
        self.statement_block_serial += 1;
        self.current_statement_block = self.statement_block_serial;
        self.enter_item_taking_block();
        self.report_unreachable_after_return(body);
        for stmt in body {
            self.check_statement(stmt);
        }
        self.exit_item_taking_block();
        self.reject_unread_open_rust_generic_bindings_of_block(self.current_statement_block);
        self.current_statement_block = enclosing_block;
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
