//! Module and import diagnostics.
//!
//! Errors related to the module system: circular imports, missing files, and visibility violations.

use crate::ast::Span;

use crate::diagnostics::CompileError;
use incan_core::lang::stdlib;

/// Circular import detected.
pub fn circular_import(path: &std::path::Path, span: Span) -> CompileError {
    CompileError::new(format!("Circular import detected: {}", path.display()), span)
}

/// Cannot read file.
pub fn cannot_read_file(path: &std::path::Path, error: &std::io::Error, span: Span) -> CompileError {
    CompileError::new(format!("Cannot read '{}': {}", path.display(), error), span)
}

/// Import path `std.<module>` does not match any known Incan stdlib module.
///
/// Rust standard library paths must use the `rust::` prefix (e.g. `from rust::std::f64::consts import PI`).
pub fn unknown_stdlib_module(module: &str, span: Span) -> CompileError {
    CompileError::new(format!("Unknown stdlib module `{module}`"), span)
        .with_hint(format!(
            "Known stdlib modules: {}",
            stdlib::known_stdlib_modules_for_hint().join(", ")
        ))
        .with_hint("To import from the Rust standard library, use: `from rust::std::... import ...`")
}

/// Soft keyword usage requires importing its stdlib namespace.
pub fn soft_keyword_requires_import(keyword: &str, namespace: &str, span: Span) -> CompileError {
    CompileError::new(
        format!("`{keyword}` is only available after importing `std.{namespace}`"),
        span,
    )
    .with_hint(format!(
        "Add `import std.{namespace}` or `from std.{namespace} import ...`"
    ))
}

/// Failed to load `std.testing` marker metadata from `stdlib/testing.incn`.
pub fn invalid_std_testing_marker_metadata(details: &str, span: Span) -> CompileError {
    CompileError::new(
        "Failed to load std.testing marker metadata from `stdlib/testing.incn`".to_string(),
        span,
    )
    .with_hint(format!("Details: {details}"))
    .with_hint("Fix the std.testing marker metadata instead of relying on fallback defaults")
}

/// Importing a private or not exported name from a module.
pub fn import_not_exported(name: &str, module_path: &str, exported_names: &[String], span: Span) -> CompileError {
    let mut names = exported_names.to_vec();
    names.sort();
    let exports = if names.is_empty() {
        "<none>".to_string()
    } else {
        names.join(", ")
    };

    CompileError::new(
        format!(
            "Cannot import `{}` from `{}`: it is private or not exported. Mark it `pub` in that module.",
            name, module_path
        ),
        span,
    )
    .with_hint(format!("Public exports from `{}`: {}", module_path, exports))
}
