//! Validation policy for raw `.incnlib` payloads.
//!
//! This module stays on the transport-facing side of the manifest boundary: it validates decoded `RawLibraryManifest`
//! values before the rest of the compiler treats them as trustworthy semantic data. The checks here intentionally fail
//! early on producer mistakes such as unsupported manifest versions, malformed vocab artifacts or invalid soft-keyword
//! activations.

use std::collections::{BTreeSet, HashSet};
use std::path::{Component, Path};

use semver::Version;

use super::wire::{RawLibraryExports, RawLibraryManifest};
use super::{
    COMPILED_PROVIDER_METADATA_SCHEMA_VERSION, CanonicalIdentityExport, CanonicalIdentityNamespaceExport,
    CanonicalIdentityOriginExport, CompiledProviderMetadata, EnumExport, EnumValueExport, EnumValueTypeExport,
    ExportIdentityKind, ExportIdentityProjection, LEGACY_LIBRARY_IDENTITY_GRAPH_SCHEMA_VERSION,
    LIBRARY_IDENTITY_GRAPH_SCHEMA_VERSION, LIBRARY_MANIFEST_FORMAT, LibraryManifestError, MemberVisibilityExport,
    ParamExport, ParamKindExport, PartialExport, RUST_ABI_SCHEMA_VERSION,
};
use crate::api_metadata::{
    ApiDeclaration, ApiProjectedFunction, CHECKED_API_METADATA_SCHEMA_VERSION, resolve_api_alias_target,
    validate_checked_api_public_namespaces,
};
use crate::contract_metadata::CONTRACT_METADATA_SCHEMA_VERSION;
use crate::registry_metadata::CHECKED_REGISTRY_METADATA_SCHEMA_VERSION;
use incan_semantics_core::SemanticSourceTargetKind;

use super::{
    ExportIdentity, FieldExport, MethodExport, OWNERSHIP_MANIFEST_FORMAT, PartialTargetKindExport, PropertyExport,
    ownership,
};

mod api_identity;
mod export_rules;
mod identity_graph;
mod provider_metadata;
mod vocab_payload;

use api_identity::*;
use export_rules::*;
use identity_graph::*;
pub(crate) use provider_metadata::*;
use vocab_payload::*;

/// Validate one raw manifest payload before it is written or decoded into the semantic model.
pub(super) fn validate_raw_manifest(raw: &RawLibraryManifest) -> Result<(), LibraryManifestError> {
    validate_manifest_version(raw)?;
    validate_field_visibilities(raw)?;
    validate_callable_param_exports(&raw.exports)?;
    validate_ownership_exports(&raw.exports, raw.manifest_format)?;
    super::ownership::validate_api(raw.contract_metadata.api.as_ref(), raw.manifest_format)?;
    validate_value_enum_exports(&raw.exports)?;
    validate_identity_graph(raw)?;
    validate_contract_metadata(raw)?;
    validate_rust_abi(raw)?;
    validate_vocab_payload(raw)?;
    validate_soft_keyword_activations(raw)?;
    Ok(())
}

#[cfg(test)]
mod tests;
