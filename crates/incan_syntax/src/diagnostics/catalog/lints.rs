//! Advisory lint warnings (non-fatal).
//!
//! These diagnostics use `ErrorKind::Lint` and are displayed as style
//! suggestions rather than hard errors.

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
