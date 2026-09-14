//! Loading, digesting and publishing library manifests (`.incnlib`) — the provider side of the contract.
//!
//! The manifest model itself is the frontend's (`crate::frontend::library_manifest`), because the typechecker
//! reads dependency manifests through the same types the publisher writes. This module owns everything that
//! touches an artifact on disk: reading a manifest back, digesting a provider's outputs and source inputs, and
//! the toolchain-dependency records those digests carry. It re-exports the model so callers name one path.

mod artifact;

pub use crate::frontend::library_manifest::published_layout;
pub use crate::frontend::library_manifest::*;
pub use artifact::{ProviderArtifactDigestError, digest_provider_artifact, digest_provider_source_inputs};
pub(crate) use artifact::{
    ProviderSemanticToolchainDependency, digest_cargo_path_source_tree_with_cache,
    digest_provider_semantic_artifact_with_context_and_cache, digest_toolchain_source_tree_with_cache,
};
