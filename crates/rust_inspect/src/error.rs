//! Errors produced while validating selected inspection inputs or extracting Rust metadata.

use std::path::PathBuf;

/// Failure modes for the rust-analyzer-backed metadata layer (RFC 041 Phase 1).
#[derive(Debug, thiserror::Error)]
pub enum RustMetadataError {
    /// Local filesystem error (creating temp projects, canonical paths, …).
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// The neutral rust-analyzer loader failed to build a database for the selected projection.
    #[error("failed to load selected Rust workspace at {path}: {message}")]
    LoadWorkspace { path: PathBuf, message: String },
    /// No explicit selected projection is bound to this inspection context.
    #[error(
        "selected Rust inspection inputs are unavailable at {path}; Oven must supply the admitted projection (#991, #1037)"
    )]
    SelectedInputUnavailable { path: PathBuf },
    /// A supplied projection or physical input does not match its declared binding.
    #[error("invalid selected Rust inspection input at {path}: {message}")]
    InvalidSelectedInput { path: PathBuf, message: String },
    /// The bounded host has no implementation for a requested physical operation.
    #[error("selected Rust inspection operation is unavailable: {operation}")]
    UnsupportedSelectedOperation { operation: &'static str },
    /// No crate in the resolved graph matches the first `rust::` path segment.
    #[error("Rust crate `{0}` not found in loaded workspace")]
    CrateNotFound(String),
    /// Path segments after the crate name did not resolve to a single `hir::ModuleDef`.
    #[error("could not resolve Rust path `{0}`")]
    PathNotResolved(String),
    /// Resolution produced only macro definitions (no `ModuleDef`).
    #[error("Rust path `{0}` resolved to macros only; metadata extraction is not implemented for this item")]
    UnsupportedMacro(String),
}
