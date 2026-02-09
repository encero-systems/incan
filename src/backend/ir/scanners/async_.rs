//! Async/Tokio feature detection.
//!
//! Activation is primarily import-driven (RFC 022): importing from `std.async` signals
//! that the async runtime (tokio) is required. We also check for `async fn` / `async def`
//! declarations, since `async/await` is a language keyword that doesn't require an import.
//!
//! Surface functions like `sleep_ms`, `spawn`, `channel`, etc. live in `std.async` and
//! are covered by the import check — no AST walk for individual function names is needed.

use crate::frontend::ast::{Declaration, Program};

use super::decorators::has_stdlib_import;

/// Detect whether async runtime is required.
pub fn detect_async_usage(program: &Program) -> bool {
    // Fast path: explicit `import std.async` or `from std.async import ...`
    if has_stdlib_import(program, "async") {
        return true;
    }

    // Check for `async def` on functions / methods.
    program.declarations.iter().any(|decl| match &decl.node {
        Declaration::Function(f) => f.is_async,
        Declaration::Model(m) => m.methods.iter().any(|method| method.node.is_async),
        Declaration::Class(c) => c.methods.iter().any(|method| method.node.is_async),
        _ => false,
    })
}
