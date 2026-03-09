//! `rust.module()` and `@rust.extern` diagnostics (RFC 023).
//!
//! Errors and warnings related to the `rust.module()` directive and `@rust.extern` decorator validation, including path
//! sanitization and crate resolution.

use crate::ast::Span;
use crate::diagnostics::{CompileError, ErrorKind};

// -- rust.module() directive --------------------------------------------------

/// Duplicate `rust.module()` directive in the same file.
pub fn duplicate_rust_module(span: Span) -> CompileError {
    CompileError::new(
        "Duplicate `rust.module()` directive — only one is allowed per file".to_string(),
        span,
    )
    .with_hint("Remove the duplicate directive; each file can declare at most one Rust backing module")
}

/// `rust.module()` directive appears after other declarations (must be at top of file).
pub fn rust_module_not_at_top(span: Span) -> CompileError {
    CompileError::new(
        "`rust.module()` directive must appear at the top of the file (before any declarations)".to_string(),
        span,
    )
    .with_hint("Move `rust.module(\"...\")` to the top of the file (module docstring is allowed before it)")
}

/// `rust.module()` path contains invalid characters (not a well-formed Rust module path).
pub fn invalid_rust_module_path(path: &str, span: Span) -> CompileError {
    CompileError::new(
        format!("`rust.module()` path contains invalid characters: \"{}\"", path),
        span,
    )
    .with_hint("Use only identifier segments separated by `::` (e.g. \"my_crate::my_module\")")
    .with_note("Path must match: identifier (`::` identifier)* — no whitespace, semicolons, or special characters")
}

/// `rust.module()` path references an unknown crate (not `incan_stdlib` and not in `incan.toml`).
pub fn unresolved_rust_module_crate(crate_name: &str, path: &str, span: Span) -> CompileError {
    CompileError::new(
        format!("`rust.module(\"{}\")` references unknown crate `{}`", path, crate_name),
        span,
    )
    .with_hint(
        "The first segment of the path must be `incan_stdlib` or a crate declared in `incan.toml [dependencies]`",
    )
    .with_note("Add the crate to your project's `incan.toml` under `[dependencies]`")
}

/// `rust.module()` directive with no `@rust.extern` items in the module (warning).
pub fn unused_rust_module(span: Span) -> CompileError {
    CompileError {
        message: "`rust.module()` directive has no effect — no `@rust.extern` items found".to_string(),
        span,
        kind: ErrorKind::Warning,
        notes: Vec::new(),
        hints: vec![
            "Remove it if this module is pure Incan, or add `@rust.extern` to Rust-backed functions".to_string(),
        ],
    }
}

// -- @rust.extern decorator ---------------------------------------------------

/// `@rust.extern` function exists but the module has no `rust.module()` directive.
pub fn rust_extern_missing_rust_module(func_name: &str, span: Span) -> CompileError {
    CompileError::new(
        format!("`@rust.extern` function `{}` has no Rust backing path", func_name),
        span,
    )
    .with_note("This function's body is marked as runtime-provided by Rust")
    .with_hint("Add `rust.module(\"path::to::rust::module\")` to the top of this file")
}

/// `@rust.extern` function has a non-trivial body (anything other than `...` or `pass`).
pub fn rust_extern_non_trivial_body(func_name: &str, span: Span) -> CompileError {
    CompileError::new(
        format!(
            "`@rust.extern` function `{}` must have a `...` body — the implementation is provided by Rust",
            func_name
        ),
        span,
    )
    .with_hint("Remove the body and use `...` instead, or remove `@rust.extern` if this is a pure Incan function")
}

/// `@rust.extern` on an instance method (has `self` receiver).
pub fn rust_extern_on_instance_method(method_name: &str, span: Span) -> CompileError {
    CompileError::new(
        format!("`@rust.extern` is not allowed on instance method `{}`", method_name),
        span,
    )
    .with_note("Instance methods cannot be runtime-provided by Rust")
    .with_hint("Extract a free function (e.g. `run_server(app, ...)`) and delegate to it from the method")
}

/// Downstream Rust build failed because the backing item/module for `@rust.extern` could not be resolved.
pub fn rust_extern_unresolved_backing_item(item_name: &str, rust_module_path: &str, span: Span) -> CompileError {
    CompileError::new(
        format!(
            "Rust backing item for `@rust.extern` declaration `{}` could not be resolved under `{}`",
            item_name, rust_module_path
        ),
        span,
    )
    .with_note("The `.incn` declaration is treated as the contract for the Rust-backed implementation")
    .with_hint(format!(
        "Ensure `{rust_module_path}::{item_name}` exists and is exported by the target Rust crate/module"
    ))
}

/// Downstream Rust build failed with a likely signature mismatch for an `@rust.extern` item.
pub fn rust_extern_signature_mismatch(item_name: &str, rust_module_path: &str, span: Span) -> CompileError {
    CompileError::new(
        format!(
            "Likely signature mismatch for `@rust.extern` declaration `{}` backed by `{}`",
            item_name, rust_module_path
        ),
        span,
    )
    .with_note("The Incan signature and the Rust function signature must match after Incan-to-Rust type mapping")
    .with_hint("Compare parameter types/return type between the `.incn` declaration and the Rust implementation")
}

/// Downstream Rust build failed with a likely feature-gated backing path for an `@rust.extern` item.
pub fn rust_extern_feature_gated_backing_path(item_name: &str, rust_module_path: &str, span: Span) -> CompileError {
    CompileError::new(
        format!(
            "Rust backing path for `@rust.extern` declaration `{}` appears to be feature-gated (`{}`)",
            item_name, rust_module_path
        ),
        span,
    )
    .with_note("Cargo reported that the referenced Rust item/module is configured out")
    .with_hint(
        "Enable the required Cargo feature for the backing crate, or point `rust.module()` to an always-enabled path",
    )
}
