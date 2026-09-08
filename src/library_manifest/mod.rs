//! Library manifest (`.incnlib`) semantic model and stable IO boundary.
//!
//! The semantic model in this module is intentionally transport-agnostic. JSON is the current on-disk encoding, but
//! callers interact with typed read/write APIs only.

mod artifact;
mod model;
mod native_source;
pub mod published_layout;
#[cfg(test)]
mod tests;
mod type_projection;
mod type_refs;
mod validation;
mod wire;

use incan_vocab::{
    DslSurface, KeywordRegistration as VocabKeywordRegistration, LibraryManifest as VocabProviderManifest,
};

pub use artifact::{ProviderArtifactDigestError, digest_provider_artifact};
pub(crate) use artifact::{
    ProviderSemanticToolchainDependency, digest_cargo_path_source_tree_with_cache,
    digest_provider_semantic_artifact_with_context_and_cache, digest_provider_source_inputs,
    digest_toolchain_source_tree_with_cache,
};
pub use model::*;
pub use native_source::{
    NATIVE_SOURCE_UNIT_PATH, NATIVE_SOURCE_UNIT_SCHEMA_VERSION, NativeGitReference, NativePathAnchor,
    NativeRequirementRole, NativeRequirementSource, NativeSourceCrateKind, NativeSourceDefinitionError,
    NativeSourceInput, NativeSourcePackage, NativeSourceRequirement, NativeSourceUnitDefinition,
    NativeUnboundPathReason,
};
pub(crate) use type_projection::{
    VisitTypeRefs, with_checked_native_unions, with_checked_type_origins, with_checked_type_routes,
    with_native_nominal_origins,
};
pub use type_refs::resolved_type_from_manifest_type_ref;
pub(crate) use type_refs::type_ref_from_resolved;

/// Stable on-disk format version for `.incnlib` manifests.
pub const LIBRARY_MANIFEST_FORMAT: u32 = 3;

/// Stable schema version for generic provider metadata embedded in `.incnlib` manifests.
pub const COMPILED_PROVIDER_METADATA_SCHEMA_VERSION: u32 = 1;

/// Stable schema version for Rust ABI metadata embedded in `.incnlib` manifests.
pub const RUST_ABI_SCHEMA_VERSION: u32 = 2;
