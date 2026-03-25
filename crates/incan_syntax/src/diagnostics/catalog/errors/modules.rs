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

/// A crate-root `import rust::crate_name` binding was used in type position.
///
/// RFC 041: crate-root imports name the crate as a namespace, not a concrete Rust type. Authors should import a
/// specific item with `from rust::crate_name import TypeName` (or a longer rooted `import rust::...` path).
pub fn rust_crate_root_used_as_type(local_name: &str, crate_path: &str, span: Span) -> CompileError {
    CompileError::new(
        format!("`{local_name}` is a crate-root `rust::` import (`{crate_path}`) and cannot be used as a type"),
        span,
    )
    .with_hint(format!(
        "Import a concrete item, e.g. `from rust::{crate_path} import YourType`"
    ))
    .with_note("Crate-root `import rust::...` binds the crate namespace, not a single Rust type")
}

/// Rust item exists but is not publicly visible from the importing module.
pub fn rust_item_not_public(local_name: &str, canonical_path: &str, span: Span) -> CompileError {
    CompileError::new(
        format!("Rust item `{local_name}` (`{canonical_path}`) is not public"),
        span,
    )
    .with_hint("Import a public item or expose it from the Rust crate with `pub`")
    .with_note("RFC 041 requires visibility-aware Rust metadata for `rust::` imports")
}

/// Rust `core`/`alloc` imports are reserved for future no_std/target work.
pub fn unsupported_rust_core_alloc(crate_name: &str, span: Span) -> CompileError {
    CompileError::new(
        format!("`rust::{crate_name}` is not supported yet (reserved for future no_std/target support)"),
        span,
    )
    .with_hint("Use `rust::std::...` for standard-library interop in this release")
    .with_note("Support for `rust::core` / `rust::alloc` will be introduced in a future RFC")
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

/// `src/lib.incn` public exports must use `pub from ... import ...`.
pub fn library_pub_reexport_requires_from(span: Span) -> CompileError {
    CompileError::new("Library exports must use `pub from ... import ...`".to_string(), span)
        .with_hint("Use `pub from module import Name` in `src/lib.incn`")
}

/// A `pub from ... import ...` statement in `src/lib.incn` references an unknown module.
pub fn library_reexport_unknown_module(module_path: &str, known_modules: &[String], span: Span) -> CompileError {
    let mut modules = known_modules.to_vec();
    modules.sort();
    let known = if modules.is_empty() {
        "<none>".to_string()
    } else {
        modules.join(", ")
    };

    CompileError::new(
        format!("Cannot re-export from `{module_path}`: module not found in this library build"),
        span,
    )
    .with_hint(format!("Known modules: {}", known))
}

/// Duplicate exported name in `src/lib.incn`.
pub fn duplicate_library_export(name: &str, span: Span) -> CompileError {
    CompileError::new(format!("Duplicate library export `{name}` in `src/lib.incn`"), span)
        .with_hint("Rename one of the exports with `as`, or remove the duplicate")
}

/// `from pub::... import ...` references a library not declared in `incan.toml [dependencies]`.
pub fn unknown_pub_library(library: &str, known_libraries: &[String], span: Span) -> CompileError {
    let mut known = known_libraries.to_vec();
    known.sort();
    let known = if known.is_empty() {
        "<none>".to_string()
    } else {
        known.join(", ")
    };

    CompileError::new(format!("Unknown `pub::` library `{library}`"), span)
        .with_hint("Declare it in `incan.toml [dependencies]` and build the dependency library first")
        .with_hint(format!("Known libraries: {known}"))
}

/// Loading a dependency `.incnlib` failed for a `pub::` import.
pub fn pub_library_manifest_load_failed(library: &str, manifest_path: &str, details: &str, span: Span) -> CompileError {
    CompileError::new(
        format!("Failed to load manifest for `pub::{library}` from `{manifest_path}`"),
        span,
    )
    .with_hint(details.to_string())
    .with_hint("Run `incan build --lib` in the dependency project to regenerate its `.incnlib` manifest")
}

/// A `pub::` dependency is missing generated crate artifacts under `target/lib`.
pub fn pub_library_artifact_missing(library: &str, artifact_path: &str, details: &str, span: Span) -> CompileError {
    CompileError::new(
        format!("Missing generated crate artifacts for `pub::{library}` at `{artifact_path}`"),
        span,
    )
    .with_hint(details.to_string())
    .with_hint("Run `incan build --lib` in the dependency project")
}

/// A `pub::` dependency has an invalid generated crate layout under `target/lib`.
pub fn pub_library_artifact_invalid(library: &str, artifact_path: &str, details: &str, span: Span) -> CompileError {
    CompileError::new(
        format!("Invalid generated crate artifacts for `pub::{library}` at `{artifact_path}`"),
        span,
    )
    .with_hint(details.to_string())
    .with_hint("Rebuild the dependency with `incan build --lib` and verify `target/lib/Cargo.toml` + `src/lib.rs`")
}

/// A `pub::` dependency has mismatched naming between dependency key and produced crate metadata.
pub fn pub_library_artifact_mismatch(library: &str, artifact_path: &str, details: &str, span: Span) -> CompileError {
    CompileError::new(
        format!("Generated crate metadata mismatch for `pub::{library}` at `{artifact_path}`"),
        span,
    )
    .with_hint(details.to_string())
    .with_hint("Ensure `.incnlib` name, manifest `name`, and Cargo `[package].name` are consistent")
}

/// A symbol was requested from a known `pub::` library but is not part of its manifest exports.
pub fn pub_library_symbol_not_exported(
    symbol: &str,
    library: &str,
    exported_names: &[String],
    span: Span,
) -> CompileError {
    let mut names = exported_names.to_vec();
    names.sort();
    let exports = if names.is_empty() {
        "<none>".to_string()
    } else {
        names.join(", ")
    };

    CompileError::new(format!("`{symbol}` is not exported by `pub::{library}`"), span)
        .with_hint(format!("Available exports from `pub::{library}`: {exports}"))
}

/// A `pub::` import binding collides with an already-defined local/imported symbol.
pub fn pub_library_import_name_collision(name: &str, existing_kind: &str, span: Span) -> CompileError {
    CompileError::new(
        format!("Cannot import `{name}` from `pub::`: a {existing_kind} named `{name}` is already in scope"),
        span,
    )
    .with_hint(format!(
        "Use an alias to avoid the collision, e.g. `from pub::mylib import {name} as {name}FromLib`"
    ))
}
