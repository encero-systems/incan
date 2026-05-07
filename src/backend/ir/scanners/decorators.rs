//! Decorator resolution helpers for scanner passes.
//!
//! Scanners work on the parsed AST and need to recognize stdlib decorators
//! (e.g. `@std.web.route`) even when referenced through local aliases:
//!
//! - `import std.web as web` → `@web.route(...)`
//! - `from std.web import route` → `@route(...)`
//!
//! This module collects import aliases and resolves decorator paths via the same
//! “segments + alias prefix” approach as the frontend.

use crate::frontend::ast::{Declaration, ImportKind, Program};

/// Check if the program imports from `std.<module>` (or any submodule).
///
/// This is the import-driven feature activation mechanism prescribed by RFC 022:
/// when the compiler resolves an import from a `std.*` module, it activates the
/// corresponding feature.
pub(super) fn has_stdlib_import(program: &Program, module: &str) -> bool {
    use incan_core::lang::stdlib::STDLIB_ROOT;
    program.declarations.iter().any(|decl| {
        let Declaration::Import(import) = &decl.node else {
            return false;
        };
        match &import.kind {
            ImportKind::Module(path) => {
                path.segments.len() >= 2 && path.segments[0] == STDLIB_ROOT && path.segments[1] == module
            }
            ImportKind::From { module: m, .. } => {
                m.segments.len() >= 2 && m.segments[0] == STDLIB_ROOT && m.segments[1] == module
            }
            _ => false,
        }
    })
}
