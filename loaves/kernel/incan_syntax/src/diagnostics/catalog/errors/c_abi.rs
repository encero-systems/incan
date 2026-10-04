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

/// A scoped C text view is used as an ordinary value (RFC 116).
///
/// The result of a symbol that returns `c.ConstPtr[c.c_char]` points into memory the C library owns, so it lives only
/// until the next call into that library. It may be bound to a local and then only be the receiver of
/// `copy_utf8(max_bytes=...)`; returning it, storing it, capturing it, passing it on or reading it any other way would
/// let it outlive that memory.
pub fn scoped_c_string_view_escapes(span: Span) -> CompileError {
    CompileError::type_error(
        "a scoped C text view cannot be returned, stored, captured or passed on; copy it with copy_utf8(max_bytes=...)"
            .to_string(),
        span,
    )
    .with_hint(
        "Bind the symbol's result to a local and call copy_utf8(max_bytes=n) on that local in the same `unsafe:` block",
    )
    .with_note("The view points into memory the C library owns; copy_utf8 is its only owning conversion")
}
