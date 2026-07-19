//! Diagnostics and error reporting for Incan
//!
//! Provides Python-friendly error messages with source highlighting.
//!
//! ## miette Integration
//!
//! This module provides `IncanDiagnostic` which implements miette's `Diagnostic` trait for rich error output with
//! source context, hints, and related errors.

mod base;
mod catalog;
mod miette;
mod stable;

pub use base::{CompileError, ErrorKind, RelatedSpan, format_error, print_error};
pub use catalog::{errors, lints};
pub use miette::{IncanDiagnostic, format_error_smart, render_miette};
pub use stable::{
    DIAGNOSTIC_SCHEMA_VERSION, DiagnosticCatalogEntry, DiagnosticOrigin, DiagnosticPhase, DiagnosticRelatedSpan,
    StableDiagnostic, catalog_entries, code_for_error, explain, phase_for_typecheck_span, stable_diagnostic,
};
