//! Web feature detection.
//!
//! Activation is import-driven (RFC 022): any import from `std.web` signals that the  web framework (axum) is required.
//! Users of `@route` must import from `std.web`, so an explicit decorator scan is unnecessary.

use crate::frontend::ast::Program;

use super::decorators::has_stdlib_import;

/// Detect web framework usage (axum/tokio/serde implied).
pub fn detect_web_usage(program: &Program) -> bool {
    has_stdlib_import(program, "web")
}
