//! Advisory lint warnings (non-fatal).
//!
//! These diagnostics use `ErrorKind::Lint` and are displayed as style
//! suggestions rather than hard errors.

use crate::ast::Span;

use super::super::{CompileError, ErrorKind};

pub fn unused_variable(name: &str, span: Span) -> CompileError {
    CompileError {
        message: format!("Unused variable '{}'", name),
        span,
        kind: ErrorKind::Lint,
        notes: vec![],
        hints: vec!["Prefix with underscore to silence: _{}".to_string() + name],
    }
}

pub fn unused_import(name: &str, span: Span) -> CompileError {
    CompileError {
        message: format!("Unused import '{}'", name),
        span,
        kind: ErrorKind::Lint,
        notes: vec![],
        hints: vec!["Remove the import or use it".to_string()],
    }
}

pub fn wildcard_match(span: Span) -> CompileError {
    CompileError {
        message: "Using wildcard '_' in match - consider handling all cases explicitly".to_string(),
        span,
        kind: ErrorKind::Lint,
        notes: vec![],
        hints: vec![],
    }
}
