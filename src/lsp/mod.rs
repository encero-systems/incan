//! Incan Language Server Protocol (LSP) implementation
//!
//! Provides IDE features:
//! - Real-time diagnostics (errors, warnings, lints)
//! - Hover information (types, signatures)
//! - Go-to-definition
//! - Completions (future)

pub mod backend;
mod call_site_type_args;
pub mod diagnostics;

pub use backend::IncanLanguageServer;
