//! Immutable checked metadata for ordinary library Loaves (#1337/#1698).
//!
//! Metadata ownership is independent of profile execution plans. A selected owner retains its original Store lease;
//! consumers must derive current execution plans from checked requirements rather than replaying a native selection.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;
use std::sync::Arc;

use incan_frontend::library_manifest::LibraryManifest;
use incan_lang::interop::metadata::RustItemKind;
use oven_store::store::{
    OvenArtifactKind, OvenArtifactMaterializedFile, OvenArtifactPublishRequest, OvenStore, OvenStoreExecutionPayload,
};
use oven_store::{OvenGeneratedProjectRequest, OvenReceipt, digest_bytes, receipt_generated_project};
use serde::{Deserialize, Serialize};

use super::OvenPackagedLibraryMetadataFile;
use super::output_paths::{packaged_library_metadata_files, validated_project_output_relative_path};
use crate::error::{CliError, CliResult};

/// Ordinary checked-library payload schema; absent older authority is a preparation miss.
pub const LIBRARY_METADATA_SCHEMA_VERSION: u32 = 1;
/// Shared Store domain for checked ordinary package contracts, including installed standard packages.
pub const LIBRARY_METADATA_DOMAIN: &str = "incan-library-metadata-v1";

/// Source-current input contract selecting checked output, without aggregate publication paths or native profiles.
///
/// The producer contract initially includes the real compiler digest and complete semantic preparation authority.
/// Replacing those conservative inputs requires an authenticated narrower producer/semantic closure, not a version
/// label.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LibraryMetadataRecipe {
    /// Ordinary package identity.
    pub name: String,
    /// Exact package version.
    pub version: String,
    /// Current authored source and declaration closure.
    pub source_digest: String,
    /// Actual selected metadata-producing implementation.
    pub producer_digest: String,
    /// Complete selected semantic/tool/macro authority until a narrower closure is proven.
    pub semantic_authority_digest: String,
    /// Current admitted checked dependency contracts, keyed by exact import alias.
    pub dependencies: BTreeMap<String, LibraryMetadataDependency>,
    /// Explicit preparation policy, including namespace grants and generated-output contract.
    pub policy_digest: String,
    /// Target on which checked ABI/layout facts depend.
    pub target: String,
    /// Exact toolchain selecting Rust facts.
    pub toolchain: String,
    /// Sorted package features, independent of output profile.
    pub features: Vec<String>,
}

/// One exact checked dependency owner; equal file bytes do not authorize substituting another package.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LibraryMetadataDependency {
    /// Canonical package name.
    pub name: String,
    /// Canonical package version.
    pub version: String,
    /// Original receipt selecting the metadata owner.
    pub receipt_identity: String,
    /// Original immutable Store entry, retained by the preparing caller.
    pub owner_identity: String,
    /// Complete checked contract digest, including Rust ABI and declared sidecars.
    pub checked_digest: String,
}

/// Portable metadata entry carried by the ordinary package Loaf index.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LibraryMetadataReference {
    /// Versioned ordinary metadata payload contract.
    pub schema_version: u32,
    /// Reproduced source-current recipe; Engine entries do not carry an original-receipt witness.
    pub receipt: OvenReceipt,
    /// Exact immutable Engine owner in the package's normal Loaf Store.
    pub owner_identity: String,
}

/// Checked output contract sealed as the Engine payload, never trusted as an adjacent cache file.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct LibraryMetadataPayload {
    schema_version: u32,
    recipe: LibraryMetadataRecipe,
    receipt: OvenReceipt,
    manifest_relative_path: String,
    metadata_files: Vec<OvenPackagedLibraryMetadataFile>,
    generated_files: Vec<OvenPackagedLibraryMetadataFile>,
    required_rust_abi: BTreeSet<String>,
}

/// Admitted checked output with the original immutable Store coordinate and lease.
pub struct SelectedLibraryMetadata {
    owner: OvenStoreExecutionPayload,
    payload: LibraryMetadataPayload,
    manifest: LibraryManifest,
}

impl LibraryMetadataRecipe {
    /// Validate complete canonical inputs before selection or publication.
    pub fn validate(&self) -> CliResult<()> {
        if self.name.trim().is_empty()
            || self.version.trim().is_empty()
            || self.target.trim().is_empty()
            || self.toolchain.trim().is_empty()
        {
            return Err(CliError::failure(
                "checked library recipe has missing package or tool intent",
            ));
        }
        for digest in [
            &self.source_digest,
            &self.producer_digest,
            &self.semantic_authority_digest,
            &self.policy_digest,
        ] {
            validate_digest(digest)?;
        }
        if self.features.iter().any(|feature| feature.trim().is_empty())
            || self.features.windows(2).any(|pair| pair[0] >= pair[1])
        {
            return Err(CliError::failure("checked library features are not canonical"));
        }
        for (alias, dependency) in &self.dependencies {
            if alias.trim().is_empty() || dependency.name.trim().is_empty() || dependency.version.trim().is_empty() {
                return Err(CliError::failure("checked library dependency identity is missing"));
            }
            for digest in [
                &dependency.receipt_identity,
                &dependency.owner_identity,
                &dependency.checked_digest,
            ] {
                validate_digest(digest)?;
            }
        }
        Ok(())
    }

    /// Reproduce a canonical receipt from current source selection without parsing Incan or consulting native plans.
    pub fn receipt(&self, project_root: &Path) -> CliResult<OvenReceipt> {
        self.validate()?;
        let bytes = serde_json::to_vec(self).map_err(|error| CliError::failure(error.to_string()))?;
        receipt_generated_project(
            &OvenGeneratedProjectRequest::new(
                project_root,
                &self.name,
                &self.version,
                &self.target,
                &self.toolchain,
                "checked-metadata",
                self.features.clone(),
            )
            .with_generated_source("library-declaration", project_root.join("loaf.toml"))
            .with_build_unit_input(
                "ordinary-library-metadata-schema",
                LIBRARY_METADATA_SCHEMA_VERSION.to_string(),
            )
            .with_build_unit_input("ordinary-library-metadata-recipe", digest_bytes(&bytes)),
        )
        .map_err(|error| CliError::failure(error.to_string()))
    }
}

impl SelectedLibraryMetadata {
    /// Borrow the validated full manifest, including complete published Rust ABI.
    pub fn manifest(&self) -> &LibraryManifest {
        &self.manifest
    }

    /// Return a portable reference while retaining this owner's original lease.
    pub fn reference(&self) -> LibraryMetadataReference {
        LibraryMetadataReference {
            schema_version: LIBRARY_METADATA_SCHEMA_VERSION,
            receipt: self.payload.receipt.clone(),
            owner_identity: self.owner.manifest.identity.clone(),
        }
    }

    /// Revalidate the held owner and copy only its exact checked/generated closure to a publication destination.
    ///
    /// No frontend work occurs here. The caller retains this selection through aggregate/package publication and
    /// subsequent use; materialization itself does not transfer pruning protection to a mutable output directory.
    pub fn replay(&self, destination: &Path) -> CliResult<()> {
        self.owner
            .verify_admitted_payload()
            .map_err(|error| CliError::failure(error.to_string()))?;
        validate_output_contract(&self.owner.artifact_root, &self.payload)?;
        fs::create_dir_all(destination).map_err(|error| CliError::failure(error.to_string()))?;
        require_directory(destination)?;
        for file in self.payload.metadata_files.iter().chain(&self.payload.generated_files) {
            let relative = validated_project_output_relative_path(&file.relative_path, "checked library replay")?;
            let output = destination.join(relative);
            create_safe_parents(destination, &output)?;
            match fs::symlink_metadata(&output) {
                Ok(metadata) if !metadata.is_file() || metadata.file_type().is_symlink() => {
                    return Err(CliError::failure(
                        "checked library replay destination is not a regular file",
                    ));
                }
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(CliError::failure(error.to_string())),
            }
            let bytes = fs::read(self.owner.artifact_root.join(&file.relative_path))
                .map_err(|error| CliError::failure(error.to_string()))?;
            if digest_bytes(&bytes) != file.digest {
                return Err(CliError::failure("checked library replay input changed while copying"));
            }
            fs::write(&output, bytes).map_err(|error| CliError::failure(error.to_string()))?;
        }
        validate_output_contract(destination, &self.payload)?;
        Ok(())
    }
}

/// Select only a receipt-exact ordinary metadata owner; absence is a miss and malformed claimed authority refuses.
pub fn select_library_metadata(
    store: &OvenStore,
    recipe: &LibraryMetadataRecipe,
    receipt: &OvenReceipt,
) -> CliResult<Option<Arc<SelectedLibraryMetadata>>> {
    recipe.validate()?;
    receipt
        .verify_identity()
        .map_err(|error| CliError::failure(error.to_string()))?;
    validate_recipe_receipt(recipe, receipt)?;
    let mut candidates = store
        .select_payloads_matching_for_execution(|manifest| {
            manifest.kind == OvenArtifactKind::Engine
                && manifest.domain == LIBRARY_METADATA_DOMAIN
                && manifest.receipt_identity == receipt.identity
                && manifest.build_unit_identity == receipt.build_unit_identity
                && manifest.intent == receipt.intent
        })
        .map_err(|error| CliError::failure(error.to_string()))?;
    if candidates.is_empty() {
        return Ok(None);
    }
    if candidates.len() != 1 {
        return Err(CliError::failure(
            "competing checked library owners for one exact recipe",
        ));
    }
    admit(candidates.remove(0), recipe, receipt).map(|selected| Some(Arc::new(selected)))
}

/// Publish finalized checked metadata and generated facade files through ordinary immutable Engine admission.
///
/// The caller must recapture its source-current recipe immediately before this call and retain current dependency
/// selections. This function proves output closure, canonical manifest and complete promised ABI, not source discovery.
pub fn publish_library_metadata(
    store: &OvenStore,
    recipe: &LibraryMetadataRecipe,
    receipt: &OvenReceipt,
    artifact_root: &Path,
    manifest_path: &Path,
    required_rust_abi: BTreeSet<String>,
) -> CliResult<Arc<SelectedLibraryMetadata>> {
    recipe.validate()?;
    receipt
        .verify_identity()
        .map_err(|error| CliError::failure(error.to_string()))?;
    validate_recipe_receipt(recipe, receipt)?;
    let manifest =
        LibraryManifest::read_from_path(manifest_path).map_err(|error| CliError::failure(error.to_string()))?;
    let manifest_relative_path = manifest_path
        .strip_prefix(artifact_root)
        .map_err(|_| CliError::failure("checked manifest escapes artifact root"))?
        .to_string_lossy()
        .replace('\\', "/");
    let payload = LibraryMetadataPayload {
        schema_version: LIBRARY_METADATA_SCHEMA_VERSION,
        recipe: recipe.clone(),
        receipt: receipt.clone(),
        manifest_relative_path,
        metadata_files: packaged_library_metadata_files(manifest_path, &manifest, artifact_root)?,
        generated_files: generated_files(artifact_root)?,
        required_rust_abi,
    };
    validate_output_contract(artifact_root, &payload)?;
    let materialized_files = payload
        .metadata_files
        .iter()
        .chain(&payload.generated_files)
        .map(|file| OvenArtifactMaterializedFile {
            source_path: artifact_root.join(&file.relative_path),
            relative_path: file.relative_path.clone(),
        })
        .collect();
    store
        .publish(&OvenArtifactPublishRequest {
            receipt: receipt.clone(),
            domain: LIBRARY_METADATA_DOMAIN.to_string(),
            kind: OvenArtifactKind::Engine,
            payload: serde_json::to_vec(&payload).map_err(|error| CliError::failure(error.to_string()))?,
            materialized_files,
            materialized_directories: Vec::new(),
        })
        .map_err(|error| CliError::failure(error.to_string()))?;
    select_library_metadata(store, recipe, receipt)?
        .ok_or_else(|| CliError::failure("published checked library owner disappeared"))
}

/// Bind a canonical receipt to every typed recipe field without relying on an Engine witness.
fn validate_recipe_receipt(recipe: &LibraryMetadataRecipe, receipt: &OvenReceipt) -> CliResult<()> {
    let bytes = serde_json::to_vec(recipe).map_err(|error| CliError::failure(error.to_string()))?;
    if receipt.project.name != recipe.name
        || receipt.project.version != recipe.version
        || receipt.intent.target != recipe.target
        || receipt.intent.toolchain != recipe.toolchain
        || receipt.intent.profile != "checked-metadata"
        || receipt.intent.features != recipe.features
        || receipt.compatibility.kind != oven_store::OvenCompatibilityKind::GeneratedIncanProject
        || receipt.compatibility.cargo_input_only
        || receipt.sources.cargo_manifest_digest.is_some()
        || receipt.sources.cargo_lock_digest.is_some()
        || receipt.sources.incan_manifest_digest.is_some()
        || receipt.sources.build_unit_inputs.len() != 2
        || receipt.sources.supplemental_digests.len() != 1
        || !receipt.sources.supplemental_digests.contains_key("library-declaration")
        || receipt
            .sources
            .build_unit_inputs
            .get("ordinary-library-metadata-recipe")
            != Some(&digest_bytes(&bytes))
        || receipt
            .sources
            .build_unit_inputs
            .get("ordinary-library-metadata-schema")
            != Some(&LIBRARY_METADATA_SCHEMA_VERSION.to_string())
    {
        return Err(CliError::failure(
            "checked library receipt disagrees with canonical recipe",
        ));
    }
    Ok(())
}

/// Decode only the selected typed contract and prove its exact original coordinate and closure.
fn admit(
    owner: OvenStoreExecutionPayload,
    recipe: &LibraryMetadataRecipe,
    receipt: &OvenReceipt,
) -> CliResult<SelectedLibraryMetadata> {
    owner
        .verify_admitted_payload()
        .map_err(|error| CliError::failure(error.to_string()))?;
    let payload: LibraryMetadataPayload = serde_json::from_slice(&owner.payload)
        .map_err(|error| CliError::failure(format!("invalid checked library payload: {error}")))?;
    if payload.schema_version != LIBRARY_METADATA_SCHEMA_VERSION
        || payload.recipe != *recipe
        || payload.receipt != *receipt
    {
        return Err(CliError::failure(
            "checked library payload disagrees with selected source-current recipe",
        ));
    }
    validate_recipe_receipt(recipe, receipt)?;
    let expected: BTreeMap<_, _> = payload
        .metadata_files
        .iter()
        .chain(&payload.generated_files)
        .map(|file| (file.relative_path.as_str(), file.digest.as_str()))
        .collect();
    if expected.len() != payload.metadata_files.len() + payload.generated_files.len()
        || owner.manifest.materialized_files.len() != expected.len()
        || !owner.manifest.materialized_directories.is_empty()
        || owner
            .manifest
            .materialized_files
            .iter()
            .any(|file| expected.get(file.relative_path.as_str()) != Some(&file.digest.as_str()))
    {
        return Err(CliError::failure(
            "checked library owner has a different complete file closure",
        ));
    }
    let manifest = validate_output_contract(&owner.artifact_root, &payload)?;
    Ok(SelectedLibraryMetadata {
        owner,
        payload,
        manifest,
    })
}

/// Validate the full checked handoff and generated sources; semantic identity alone excludes Rust ABI and is
/// insufficient.
fn validate_output_contract(root: &Path, payload: &LibraryMetadataPayload) -> CliResult<LibraryManifest> {
    let relative = validated_project_output_relative_path(&payload.manifest_relative_path, "checked library manifest")?;
    let path = root.join(relative);
    let manifest = LibraryManifest::read_from_path(&path).map_err(|error| CliError::failure(error.to_string()))?;
    if manifest.name != payload.recipe.name
        || manifest.version != payload.recipe.version
        || packaged_library_metadata_files(&path, &manifest, root)? != payload.metadata_files
        || generated_files(root)? != payload.generated_files
    {
        return Err(CliError::failure("checked library manifest or file closure changed"));
    }
    if let Some(surface) = manifest.contract_metadata.executable_representation.as_ref() {
        let expected = format!("sha256:{}", surface.content_digest);
        let surface_path =
            incan_frontend::library_manifest::published_layout::executable_surface_path(&path, &manifest)
                .ok_or_else(|| CliError::failure("checked library has invalid executable surface path"))?;
        let relative = surface_path
            .strip_prefix(root)
            .map_err(|error| CliError::failure(error.to_string()))?
            .to_string_lossy()
            .replace('\\', "/");
        if !payload
            .metadata_files
            .iter()
            .any(|file| file.relative_path == relative && file.digest == expected)
        {
            return Err(CliError::failure(
                "checked library executable bytes disagree with manifest digest",
            ));
        }
    }
    if let Some(desugarer) = manifest
        .vocab
        .as_ref()
        .and_then(|vocab| vocab.desugarer_artifact.as_ref())
    {
        let expected = format!("sha256:{}", desugarer.sha256);
        if !payload
            .metadata_files
            .iter()
            .any(|file| file.relative_path == desugarer.relative_path && file.digest == expected)
        {
            return Err(CliError::failure(
                "checked library vocabulary bytes disagree with manifest digest",
            ));
        }
    }
    if let Some(abi) = manifest.rust_abi.as_ref() {
        let mut facts = BTreeMap::new();
        for item in &abi.items {
            for path in std::iter::once(&item.canonical_path).chain(item.definition_path.as_ref()) {
                if let Some(previous) = facts.insert(path, &item.kind)
                    && previous != &item.kind
                {
                    return Err(CliError::failure(format!(
                        "checked library ABI has competing facts for `{path}`"
                    )));
                }
            }
        }
    }
    for query in &payload.required_rust_abi {
        if query.trim().is_empty() {
            return Err(CliError::failure("checked library ABI requirement is empty"));
        }
        let item = manifest
            .rust_abi
            .as_ref()
            .and_then(|abi| abi.get(query))
            .ok_or_else(|| CliError::failure(format!("checked library ABI is missing required item `{query}`")))?;
        if let RustItemKind::Type(ty) = &item.kind
            && (!ty.metadata_completeness.has_trait_impls() || !ty.metadata_completeness.has_methods())
        {
            return Err(CliError::failure(format!(
                "checked library ABI has incomplete type facts for `{query}`"
            )));
        }
    }
    Ok(manifest)
}

/// Inventory every regular generated source file using safe relative paths and exact content digests.
fn generated_files(root: &Path) -> CliResult<Vec<OvenPackagedLibraryMetadataFile>> {
    let mut files = Vec::new();
    collect_generated_files(root, &root.join("src"), &mut files)?;
    files.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
    if !files.iter().any(|file| file.relative_path == "src/lib.rs") {
        return Err(CliError::failure(
            "checked library replay has no generated library root",
        ));
    }
    Ok(files)
}

/// Walk generated source directories without following symlinks or accepting special files.
fn collect_generated_files(
    root: &Path,
    directory: &Path,
    files: &mut Vec<OvenPackagedLibraryMetadataFile>,
) -> CliResult<()> {
    require_directory(directory)?;
    for entry in fs::read_dir(directory).map_err(|error| CliError::failure(error.to_string()))? {
        let path = entry.map_err(|error| CliError::failure(error.to_string()))?.path();
        let metadata = fs::symlink_metadata(&path).map_err(|error| CliError::failure(error.to_string()))?;
        if metadata.is_dir() && !metadata.file_type().is_symlink() {
            collect_generated_files(root, &path, files)?;
        } else if metadata.is_file() && !metadata.file_type().is_symlink() {
            let relative = path
                .strip_prefix(root)
                .map_err(|error| CliError::failure(error.to_string()))?
                .to_string_lossy()
                .replace('\\', "/");
            validated_project_output_relative_path(&relative, "checked generated source")?;
            let digest =
                super::file_freshness::digest_file(&path).map_err(|error| CliError::failure(error.to_string()))?;
            files.push(OvenPackagedLibraryMetadataFile {
                relative_path: relative,
                digest,
            });
        } else {
            return Err(CliError::failure(
                "checked generated source is not a regular file or directory",
            ));
        }
    }
    Ok(())
}

/// Reject absent, redirected or special directories before output traversal.
fn require_directory(path: &Path) -> CliResult<()> {
    let metadata = fs::symlink_metadata(path).map_err(|error| CliError::failure(error.to_string()))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(CliError::failure("checked library directory is not regular"));
    }
    Ok(())
}

/// Create each destination directory separately, refusing pre-existing symlink parents.
fn create_safe_parents(root: &Path, output: &Path) -> CliResult<()> {
    let parent = output
        .parent()
        .ok_or_else(|| CliError::failure("checked replay file has no parent"))?;
    let relative = parent
        .strip_prefix(root)
        .map_err(|error| CliError::failure(error.to_string()))?;
    let mut current = root.to_path_buf();
    for component in relative.components() {
        current.push(component);
        match fs::create_dir(&current) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(CliError::failure(error.to_string())),
        }
        require_directory(&current)?;
    }
    Ok(())
}

/// Require lowercase canonical SHA-256 evidence rather than accepting paths or informal version labels.
fn validate_digest(value: &str) -> CliResult<()> {
    let hex = value
        .strip_prefix("sha256:")
        .ok_or_else(|| CliError::failure("checked library input is not a canonical digest"))?;
    if hex.len() != 64
        || !hex
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(CliError::failure("checked library input is not a canonical digest"));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
