//! Diagnostics for the Rust interop forms retired by RFC 171.

use crate::ast::Span;
use crate::diagnostics::CompileError;

/// Refuse the retired `rust.module(...)` directive with its direct-import replacement.
pub fn retired_rust_module(span: Span) -> CompileError {
    CompileError::syntax(
        "`rust.module(...)` has been removed; import each Rust item through `rust::` instead".to_string(),
        span,
    )
    .with_hint("Replace the module facade with `from rust::crate::module import item` imports")
}

/// Refuse the retired `@rust.extern` decorator with its direct-import replacement.
pub fn retired_rust_extern(span: Span) -> CompileError {
    CompileError::type_error(
        "`@rust.extern` has been removed; import the Rust function through `rust::` instead".to_string(),
        span,
    )
    .with_hint("Use `from rust::crate::module import function`, or call that import from an ordinary Incan wrapper")
}

/// Refuse the retired no-op `@rust.allow(...)` decorator.
pub fn retired_rust_allow(span: Span) -> CompileError {
    CompileError::type_error(
        "`@rust.allow(...)` has been removed because native compilation generates no Rust source to carry it"
            .to_string(),
        span,
    )
    .with_hint("Delete the decorator; there is no replacement Rust lint suppression")
}
