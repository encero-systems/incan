//! Library manifest (`.incnlib`) semantic model and stable IO boundary.
//!
//! The semantic model in this module is intentionally transport-agnostic. JSON is the current on-disk encoding, but
//! callers interact with typed read/write APIs only.

mod model;
#[cfg(test)]
mod tests;
mod type_refs;
mod validation;
mod wire;

use incan_vocab::{
    DslSurface, KeywordRegistration as VocabKeywordRegistration, LibraryManifest as VocabProviderManifest,
};

pub use model::*;
pub use type_refs::resolved_type_from_manifest_type_ref;
pub(crate) use type_refs::type_ref_from_resolved;

/// Stable on-disk format version for `.incnlib` manifests.
pub const LIBRARY_MANIFEST_FORMAT: u32 = 1;

/// Stable schema version for Rust ABI metadata embedded in `.incnlib` manifests.
pub const RUST_ABI_SCHEMA_VERSION: u32 = 1;
