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

pub use base::{CompileError, ErrorKind, format_error, print_error};
pub use catalog::{errors, lints};
pub use miette::{IncanDiagnostic, format_error_smart, render_miette};
