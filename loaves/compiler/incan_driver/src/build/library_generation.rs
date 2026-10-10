//! Producer-sealed ordinary package generation connecting checked metadata to final authored authority.
//!
//! Raw checked source closure and portable package authority are distinct digest domains. This immutable ordinary
//! Engine payload associates both with one original metadata owner; an adjacent handoff cannot rewrite that bond.

use std::path::Path;
use std::sync::Arc;

use oven_store::store::{
    OvenArtifactKind, OvenArtifactPublishRequest, OvenStore, OvenStoreExecutionPayload, PublishedOvenStore,
};
use oven_store::{OvenReceipt, digest_bytes, receipt_with_build_unit_input};
use serde::{Deserialize, Serialize};

use super::OvenPackagedLibraryMetadataFile;
use super::library_metadata::{LibraryMetadataReference, SelectedLibraryMetadata};
use super::library_project::metadata_replay::observe_library_source_digest;
use super::source_authority::digest_baked_project_source_authority;
use crate::error::{CliError, CliResult};

/// Version of the ordinary package-generation association, independent of execution profiles.
pub const LIBRARY_GENERATION_SCHEMA_VERSION: u32 = 1;
/// Ordinary Store domain for final package-generation contracts; it has no SDK catalog dependency.
pub const LIBRARY_GENERATION_DOMAIN: &str = "incan-library-generation-v1";

/// Exact original generation owner carried by an ordinary package handoff.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LibraryGenerationReference {
    /// Supported association payload version.
    pub schema_version: u32,
    /// Reproduced receipt bound to the complete generation contract.
    pub receipt: OvenReceipt,
    /// Original immutable Store owner, never substituted by equivalent checked bytes.
    pub owner_identity: String,
}

/// Complete association of final source authority and the original checked generation.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LibraryGenerationContract {
    schema_version: u32,
    source_authority_digest: String,
    metadata_source_digest: String,
    metadata: LibraryMetadataReference,
    checked_files: Vec<OvenPackagedLibraryMetadataFile>,
    name: String,
    version: String,
    features: Vec<String>,
    target: String,
    toolchain: String,
}

/// Typed sealed transport; receipt equality is checked independently from Engine identity.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LibraryGenerationPayload {
    contract: LibraryGenerationContract,
    receipt: OvenReceipt,
}

/// Original generation and metadata leases retained through consumer use and revalidation.
pub struct SelectedLibraryGeneration {
    owner: Arc<OvenStoreExecutionPayload>,
    payload: LibraryGenerationPayload,
    metadata: Arc<SelectedLibraryMetadata>,
}

impl SelectedLibraryGeneration {
    /// Produce the portable exact reference while retaining this owner's original lease in the caller.
    pub fn reference(&self) -> LibraryGenerationReference {
        LibraryGenerationReference {
            schema_version: LIBRARY_GENERATION_SCHEMA_VERSION,
            receipt: self.payload.receipt.clone(),
            owner_identity: self.owner.manifest.identity.clone(),
        }
    }

    /// Import this exact original ordinary generation and checked owner without reminting from absent source.
    pub fn export_into(&self, destination: &OvenStore) -> CliResult<LibraryGenerationReference> {
        self.verify()?;
        self.metadata.export_into(destination)?;
        let exported = destination
            .publish_verified_import(
                &OvenArtifactPublishRequest {
                    receipt: self.payload.receipt.clone(),
                    domain: LIBRARY_GENERATION_DOMAIN.into(),
                    kind: OvenArtifactKind::Engine,
                    payload: self.owner.payload.clone(),
                    materialized_files: Vec::new(),
                    materialized_directories: Vec::new(),
                },
                self.owner.admitted_materialized_files(),
                self.owner.admitted_materialized_directories(),
            )
            .map_err(|error| invalid(error.to_string()))?;
        if exported.identity != self.owner.manifest.identity
            || exported.receipt_identity != self.payload.receipt.identity
            || exported.build_unit_identity != self.payload.receipt.build_unit_identity
            || exported.intent != self.payload.receipt.intent
        {
            return Err(invalid(
                "ordinary package generation changed during original owner export",
            ));
        }
        Ok(self.reference())
    }

    /// Verify the same admitted owner and complete checked association without rediscovery or substitution.
    pub fn verify(&self) -> CliResult<()> {
        self.owner
            .verify_admitted_payload()
            .map_err(|error| invalid(error.to_string()))?;
        validate_owner(&self.owner, &self.payload)?;
        validate_contract(
            &self.payload.contract,
            &self.metadata,
            &self.payload.contract.source_authority_digest,
            self.metadata.checked_files(),
        )
    }
}

/// Publish a final source-current ordinary association after exporting the same original checked owner.
///
/// Both digest domains are freshly observed here. Installed assembly must import the original owner and must never
/// remint an association without authored source. This conservative source observer remains narrower-JEC debt.
pub fn publish_library_generation(
    store: &OvenStore,
    project_root: &Path,
    source_authority_digest: &str,
    metadata: &Arc<SelectedLibraryMetadata>,
) -> CliResult<Arc<SelectedLibraryGeneration>> {
    metadata.verify()?;
    let project = crate::project::effective_project_manifest_for_exact_root(project_root)?;
    let identity = project
        .project
        .as_ref()
        .ok_or_else(|| invalid("ordinary package generation has no authored package identity"))?;
    if identity.name.as_deref() != Some(metadata.recipe().name.as_str())
        || identity.version.as_deref().unwrap_or("0.1.0") != metadata.recipe().version
    {
        return Err(invalid(
            "ordinary package generation checked owner differs from authored package identity",
        ));
    }
    if observe_library_source_digest(project_root, &metadata.recipe().features)? != metadata.recipe().source_digest
        || digest_baked_project_source_authority(project_root)? != source_authority_digest
    {
        return Err(invalid(
            "ordinary package generation does not match current authored checked source",
        ));
    }
    let recipe = metadata.recipe();
    let contract = LibraryGenerationContract {
        schema_version: LIBRARY_GENERATION_SCHEMA_VERSION,
        source_authority_digest: source_authority_digest.into(),
        metadata_source_digest: recipe.source_digest.clone(),
        metadata: metadata.reference(),
        checked_files: metadata.checked_files().to_vec(),
        name: recipe.name.clone(),
        version: recipe.version.clone(),
        features: recipe.features.clone(),
        target: recipe.target.clone(),
        toolchain: recipe.toolchain.clone(),
    };
    let receipt = generation_receipt(&contract)?;
    let payload = LibraryGenerationPayload {
        contract,
        receipt: receipt.clone(),
    };
    let manifest = store
        .publish(&OvenArtifactPublishRequest {
            receipt: receipt.clone(),
            domain: LIBRARY_GENERATION_DOMAIN.into(),
            kind: OvenArtifactKind::Engine,
            payload: serde_json::to_vec(&payload).map_err(|error| invalid(error.to_string()))?,
            materialized_files: Vec::new(),
            materialized_directories: Vec::new(),
        })
        .map_err(|error| invalid(error.to_string()))?;
    let reference = LibraryGenerationReference {
        schema_version: LIBRARY_GENERATION_SCHEMA_VERSION,
        receipt,
        owner_identity: manifest.identity,
    };
    select_library_generation_reference(
        store,
        &reference,
        Arc::clone(metadata),
        source_authority_digest,
        metadata.checked_files(),
    )
}

/// Admit one exact original association, binding installed source authority and original checked owner together.
pub fn select_library_generation_reference(
    store: &OvenStore,
    reference: &LibraryGenerationReference,
    metadata: Arc<SelectedLibraryMetadata>,
    source_authority_digest: &str,
    checked_files: &[OvenPackagedLibraryMetadataFile],
) -> CliResult<Arc<SelectedLibraryGeneration>> {
    validate_generation_reference(reference)?;
    let owners = store
        .select_payloads_for_execution(std::slice::from_ref(&reference.owner_identity))
        .map_err(|error| invalid(error.to_string()))?;
    admit_generation_reference(owners, reference, metadata, source_authority_digest, checked_files)
}

/// Admit an original source-free generation through existing read-only Store locks, without any publication or repair.
///
/// Exact owner identity, reproduced receipt, complete payload bytes and original checked metadata association use
/// the same validator as writable selection. The retained metadata capability also retains its dependency owners;
/// this grants no namespace authority and never remints an association from missing authored source.
pub fn select_published_library_generation_reference(
    store: &PublishedOvenStore,
    reference: &LibraryGenerationReference,
    metadata: Arc<SelectedLibraryMetadata>,
    source_authority_digest: &str,
    checked_files: &[OvenPackagedLibraryMetadataFile],
) -> CliResult<Arc<SelectedLibraryGeneration>> {
    metadata.verify_dependency_closure()?;
    validate_generation_reference(reference)?;
    let owners = store
        .select_payloads_matching_for_execution(|manifest| manifest.identity == reference.owner_identity)
        .map_err(|error| invalid(error.to_string()))?;
    admit_generation_reference(owners, reference, metadata, source_authority_digest, checked_files)
}

/// Validate the portable reference before selecting through either Store access mode.
fn validate_generation_reference(reference: &LibraryGenerationReference) -> CliResult<()> {
    if reference.schema_version != LIBRARY_GENERATION_SCHEMA_VERSION {
        return Err(invalid("unsupported ordinary package generation reference"));
    }
    reference
        .receipt
        .verify_identity()
        .map_err(|error| invalid(error.to_string()))
}

/// Share the full exact original association admission without constructing writable Store state.
fn admit_generation_reference(
    mut owners: Vec<OvenStoreExecutionPayload>,
    reference: &LibraryGenerationReference,
    metadata: Arc<SelectedLibraryMetadata>,
    source_authority_digest: &str,
    checked_files: &[OvenPackagedLibraryMetadataFile],
) -> CliResult<Arc<SelectedLibraryGeneration>> {
    if owners.len() != 1 {
        return Err(invalid(
            "ordinary package generation original owner is missing or competing",
        ));
    }
    let owner = owners.remove(0);
    let payload: LibraryGenerationPayload =
        serde_json::from_slice(&owner.payload).map_err(|error| invalid(error.to_string()))?;
    if owner.manifest.identity != reference.owner_identity || payload.receipt != reference.receipt {
        return Err(invalid("ordinary package generation differs from selected reference"));
    }
    validate_owner(&owner, &payload)?;
    validate_contract(&payload.contract, &metadata, source_authority_digest, checked_files)?;
    Ok(Arc::new(SelectedLibraryGeneration {
        owner: Arc::new(owner),
        payload,
        metadata,
    }))
}

/// Derive the association receipt from the full original metadata receipt and canonical typed contract.
fn generation_receipt(contract: &LibraryGenerationContract) -> CliResult<OvenReceipt> {
    let bytes = serde_json::to_vec(contract).map_err(|error| invalid(error.to_string()))?;
    receipt_with_build_unit_input(
        &contract.metadata.receipt,
        "ordinary-library-generation-v1",
        digest_bytes(&bytes),
    )
    .map_err(|error| invalid(error.to_string()))
}

/// Prove exact Engine coordinates and receipt binding, refusing unexpected materializations.
fn validate_owner(owner: &OvenStoreExecutionPayload, payload: &LibraryGenerationPayload) -> CliResult<()> {
    owner
        .verify_admitted_payload()
        .map_err(|error| invalid(error.to_string()))?;
    let receipt = generation_receipt(&payload.contract)?;
    if payload.receipt != receipt
        || owner.manifest.kind != OvenArtifactKind::Engine
        || owner.manifest.domain != LIBRARY_GENERATION_DOMAIN
        || owner.manifest.receipt_identity != receipt.identity
        || owner.manifest.build_unit_identity != receipt.build_unit_identity
        || owner.manifest.intent != receipt.intent
        || !owner.manifest.materialized_files.is_empty()
        || !owner.manifest.materialized_directories.is_empty()
    {
        return Err(invalid(
            "ordinary package generation receipt or original owner disagrees",
        ));
    }
    Ok(())
}

/// Require every selected package coordinate and checked intent to match the immutable producer association.
fn validate_contract(
    contract: &LibraryGenerationContract,
    metadata: &SelectedLibraryMetadata,
    source_authority_digest: &str,
    checked_files: &[OvenPackagedLibraryMetadataFile],
) -> CliResult<()> {
    metadata.verify()?;
    let reference = metadata.reference();
    let recipe = metadata.recipe();
    let expected_source = source_authority_digest.strip_prefix("sha256:").is_some_and(|hex| {
        hex.len() == 64
            && hex
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    });
    if !expected_source
        || contract.schema_version != LIBRARY_GENERATION_SCHEMA_VERSION
        || contract.source_authority_digest != source_authority_digest
        || contract.metadata_source_digest != recipe.source_digest
        || contract.metadata.schema_version != reference.schema_version
        || contract.metadata.receipt != reference.receipt
        || contract.metadata.owner_identity != reference.owner_identity
        || contract.checked_files != checked_files
        || checked_files != metadata.checked_files()
        || contract.name != recipe.name
        || contract.version != recipe.version
        || contract.features != recipe.features
        || contract.target != recipe.target
        || contract.toolchain != recipe.toolchain
    {
        return Err(invalid(
            "ordinary package generation disagrees with checked owner or final source authority",
        ));
    }
    Ok(())
}

/// Attach stable context to ordinary generation admission refusals.
fn invalid(message: impl Into<String>) -> CliError {
    CliError::failure(format!("ordinary package generation: {}", message.into()))
}
