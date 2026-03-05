//! Web usage detection scanner.
//!
//! RFC 023 feature activation is import-driven: importing from `std.web` (or submodules) enables web support in the
//! generated project.
//!
//! TODO: This entire module is technical debt. When the compiler gains a manifest-driven dependency
//! model (where each imported module declares its own required Cargo features), the `needs_web` flag
//! and this AST scan become unnecessary — `std.web` will simply advertise its feature requirements
//! in `STDLIB_NAMESPACES`, and the build pipeline will collect them without scanning. Delete this
//! file and its callers at that point.

use crate::frontend::ast::Program;

use super::decorators::has_stdlib_import;

/// Detect whether web support is required for this program.
///
/// Returns `true` if any import in the program references `std.web` (or a submodule like `std.web.routing`).
/// When true, the build pipeline enables the `web` stdlib feature and adds framework dependencies (`axum`,
/// `incan_web_macros`, `inventory`) to the generated `Cargo.toml`.
pub fn detect_web_usage(program: &Program) -> bool {
    has_stdlib_import(program, "web")
}
