//! Checked C ABI diagnostics (RFC 116).

use crate::ast::Span;
use crate::diagnostics::CompileError;

/// A public Incan function directly contains a checked raw C call.
///
/// This remains a warning because low-level binding packages are supported. Packages that promise an ordinary safe
/// API should keep the raw call in a private bridge and expose an Incan-facing facade instead.
pub fn public_checked_c_call_requires_private_bridge(function: &str, span: Span) -> CompileError {
    CompileError::warning(
        format!("public function `{function}` directly calls a checked C symbol"),
        span,
    )
    .with_hint("Move the `unsafe:` C call into a private bridge and expose an ordinary public Incan facade")
    .with_note("This is advisory: intentionally low-level checked binding packages remain supported")
}
