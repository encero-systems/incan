//! Advisory diagnostics (non-fatal).
//!
//! Nothing here stops compilation. Style suggestions use `ErrorKind::Lint` and render as hints; diagnostics that
//! report genuinely wrong-but-compilable code use `ErrorKind::Warning` so tooling shows them as warnings rather
//! than downgrading them to a hint.

use crate::ast::Span;

use super::super::CompileError;

/// Build the lint warning emitted for an unused local binding.
pub fn unused_variable(name: &str, span: Span) -> CompileError {
    CompileError::lint(format!("Unused variable '{}'", name), span)
        .with_hint("Prefix with underscore to silence: _".to_string() + name)
}

/// Build the lint warning emitted for an unused import.
pub fn unused_import(name: &str, span: Span) -> CompileError {
    CompileError::lint(format!("Unused import '{}'", name), span).with_hint("Remove the import or use it")
}

/// Build the lint warning emitted for a wildcard match arm.
pub fn wildcard_match(span: Span) -> CompileError {
    CompileError::lint(
        "Using wildcard '_' in match - consider handling all cases explicitly".to_string(),
        span,
    )
}

/// Build the warning emitted for statements that can never run because their block already returned.
///
/// `unreachable` covers the whole dead tail of one block so a long dead region reports once instead of once per
/// statement, and `return_span` points at the `return` that ended the block. This is deliberately block-local: it
/// follows a `return` statement within a single block and does not model divergence through `if`/`else`, `match`,
/// loops, or `break`, which would need a real control-flow graph and risks false positives on reachable code.
pub fn unreachable_code_after_return(unreachable: Span, return_span: Span) -> CompileError {
    CompileError::warning("Unreachable code after `return`".to_string(), unreachable)
        .with_stable_code("INCAN-T0101")
        .with_related_span(return_span, "This `return` always exits the block first")
        .with_hint("Remove the unreachable statements, or move them above the `return`")
}
