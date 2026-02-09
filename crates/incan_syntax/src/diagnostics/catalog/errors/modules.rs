//! Module and import diagnostics.
//!
//! Errors related to the module system: circular imports, missing files,
//! and visibility violations.

use crate::ast::Span;

use crate::diagnostics::CompileError;

pub fn circular_import(path: &std::path::Path, span: Span) -> CompileError {
    CompileError::new(format!("Circular import detected: {}", path.display()), span)
}

pub fn cannot_read_file(path: &std::path::Path, error: &std::io::Error, span: Span) -> CompileError {
    CompileError::new(format!("Cannot read '{}': {}", path.display(), error), span)
}

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
