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

use crate::frontend::ast::{self, Declaration, ImportKind, Program};
use crate::frontend::decorator_resolution;
use incan_core::lang::decorators::{self, DecoratorId};

/// Collect import aliases from the program.
pub(super) fn collect_import_aliases(program: &Program) -> std::collections::HashMap<String, Vec<String>> {
    decorator_resolution::collect_import_aliases(program)
}

/// Resolve a decorator path to a module path.
fn resolve_decorator_path(
    dec: &ast::Decorator,
    aliases: &std::collections::HashMap<String, Vec<String>>,
) -> Vec<String> {
    decorator_resolution::resolve_decorator_path(dec, aliases)
}

/// Resolve a decorator path to a decorator id.
pub(super) fn resolve_decorator_id(
    dec: &ast::Decorator,
    aliases: &std::collections::HashMap<String, Vec<String>>,
) -> Option<DecoratorId> {
    let resolved = resolve_decorator_path(dec, aliases);
    decorators::from_segments(&resolved)
}

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
