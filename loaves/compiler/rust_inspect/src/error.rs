//! Errors produced while loading Cargo workspaces or extracting Rust item metadata.

use std::path::PathBuf;

/// Failure modes for the rust-analyzer-backed metadata layer (RFC 041 Phase 1).
#[derive(Debug, thiserror::Error)]
pub enum RustMetadataError {
    /// Local filesystem error (creating temp projects, canonical paths, …).
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// `ra_ap_load_cargo` failed to build a `RootDatabase` for the manifest.
    #[error("failed to load Cargo workspace at {path}: {message}")]
    LoadWorkspace { path: PathBuf, message: String },
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
