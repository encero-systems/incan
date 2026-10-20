//! The library manifest (`.incnlib`): the vocabulary a checked library publishes and a consumer's frontend resolves
//! against.
//!
//! The manifest model embeds the checked exports, resolved types and source anchors the typechecker produces. A library
//! unit's publication writes it and the Loaf index reads it (DD-0007). JSON is the on-disk encoding, behind typed read
//! and write APIs.

mod checked_replay;
mod model;
pub mod public_identities;
mod type_projection;
mod type_refs;
mod validation;
pub mod wire;

use incan_vocab::{
    DslSurface, KeywordRegistration as VocabKeywordRegistration, LibraryManifest as VocabProviderManifest,
};

pub use checked_replay::CheckedExportReplay;
pub use model::*;
pub use type_projection::{
    VisitTypeRefs, contains_native_union, with_checked_type_origins, with_checked_type_routes,
    with_native_nominal_origins, with_self_owned_native_unions,
};
pub use type_refs::resolved_type_from_manifest_type_ref;
pub use type_refs::type_ref_from_resolved;

/// Stable on-disk format version for `.incnlib` manifests.
pub const LIBRARY_MANIFEST_FORMAT: u32 = 5;

/// Stable schema version for generic provider metadata embedded in `.incnlib` manifests.
pub const COMPILED_PROVIDER_METADATA_SCHEMA_VERSION: u32 = 1;

/// Stable schema version for Rust ABI metadata embedded in `.incnlib` manifests.
pub const RUST_ABI_SCHEMA_VERSION: u32 = 2;
