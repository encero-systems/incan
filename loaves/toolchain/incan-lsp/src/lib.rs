//! Incan Language Server Protocol (LSP) implementation
//!
//! Provides IDE features:
//! - Real-time diagnostics (errors, warnings, lints)
//! - Hover information (types, signatures)
//! - Go-to-definition
//! - Completions (future)
//! - Semantic highlighting, including embedded-fragment ownership (RFC 081)

pub mod backend;
mod call_site_type_args;
pub mod diagnostics;
pub mod semantic_tokens;

pub use backend::IncanLanguageServer;
