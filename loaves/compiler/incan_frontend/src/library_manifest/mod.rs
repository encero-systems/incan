//! The library manifest (`.incnlib`): the vocabulary a checked library publishes and a consumer's frontend resolves
//! against.
//!
//! This is the frontend's own contract, not the provider's: the manifest model embeds checked exports, resolved types
//! and source anchors the typechecker produces, and the typechecker reads dependency manifests through the same types.
//! The provider ring (`incan_provider`) loads, digests and publishes manifests; it depends on this module and never the
//! reverse. The model is transport-agnostic — JSON is the on-disk encoding, behind typed read and write APIs.

mod artifact;
mod model;
pub mod published_layout;
#[cfg(test)]
mod tests;
mod type_projection;
mod type_refs;
mod validation;
pub mod wire;

use incan_vocab::{
    DslSurface, KeywordRegistration as VocabKeywordRegistration, LibraryManifest as VocabProviderManifest,
};

pub use artifact::{
    ProviderArtifactDigestError, ProviderSemanticToolchainDependency, digest_cargo_path_source_tree_with_cache,
    digest_provider_artifact, digest_provider_semantic_artifact_with_context_and_cache, digest_provider_source_inputs,
    digest_toolchain_source_tree_with_cache,
};
pub use model::*;
pub use type_projection::{
    VisitTypeRefs, contains_native_union, with_checked_native_unions, with_checked_type_origins,
    with_checked_type_routes, with_native_nominal_origins, with_self_owned_native_unions,
};
pub use type_refs::resolved_type_from_manifest_type_ref;
pub use type_refs::type_ref_from_resolved;

/// Stable on-disk format version for `.incnlib` manifests.
pub const LIBRARY_MANIFEST_FORMAT: u32 = 5;

/// Stable schema version for generic provider metadata embedded in `.incnlib` manifests.
pub const COMPILED_PROVIDER_METADATA_SCHEMA_VERSION: u32 = 1;

/// Stable schema version for Rust ABI metadata embedded in `.incnlib` manifests.
pub const RUST_ABI_SCHEMA_VERSION: u32 = 2;
