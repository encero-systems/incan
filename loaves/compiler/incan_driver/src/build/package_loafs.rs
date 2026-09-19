//! Package Loafs: exporting, copying, publishing, reading and validating the Loaf a public library package
//! carries beside its artifact.

use std::fs;
use std::path::{Path, PathBuf};

use crate::build::library_outputs::packaged_library_loaf_store_root;
use crate::build::output_paths::{validate_packaged_library_metadata_files, validated_project_output_relative_path};
use crate::build::source_authority::digest_baked_project_source_authority;
use crate::build::{
    CheckedPackagedProviderProfile, OVEN_PACKAGED_LIBRARY_LOAF_SCHEMA_VERSION, OvenPackagedLibraryLoafManifest,
    OvenPackagedLibraryLoafProfile,
};
use crate::error::{CliError, CliResult, oven_rustc_error};
use incan_frontend::library_manifest::published_layout::packaged_library_loaf_manifest_path;
use incan_frontend::library_manifest_index::LibraryArtifactMetadata;
use incan_lang::version::INCAN_VERSION;
use oven_model::manifest::LOAF_MANIFEST_FILENAME;
use oven_rustc::plan::{OvenDirectRustcPlanSelection, OvenPackagedLibraryLoafEntry};
use oven_rustc::rustc::select_direct_rustc_plan_for_execution;
use oven_store::digest_bytes;
use oven_store::store::{
    OvenArtifactKind, OvenArtifactMaterializedDirectory, OvenArtifactMaterializedFile, OvenArtifactPublishRequest,
    OvenStore, PublishedOvenStore,
};

/// Copy one already selected project Loaf into the public provider artifact through normal immutable-store admission.
///
/// This is intentionally an explicit-bake operation. The source store selection retains its active lease while the
/// package store validates every file and performs its atomic publication, so a package can never point to a
/// mutable cache directory or a half-copied third-party closure.
pub fn export_selected_package_loaf(
    source_store: &OvenStore,
    package_store_root: &Path,
    receipt: &oven_store::OvenReceipt,
    selection: &OvenDirectRustcPlanSelection,
) -> CliResult<Vec<OvenPackagedLibraryLoafEntry>> {
    let package_store = OvenStore::new(package_store_root, *source_store.limits());
    selection
        .package_entries(receipt)
        .into_iter()
        .map(|entry| {
            let exported = copy_receipted_oven_store_entry(
                source_store,
                &package_store,
                &entry.receipt,
                &entry.identity,
                entry.kind,
                "package export",
            )?;
            if exported.kind != entry.kind
                || exported.receipt_identity != entry.receipt.identity
                || exported.build_unit_identity != entry.receipt.build_unit_identity
                || exported.intent != entry.receipt.intent
            {
                return Err(CliError::failure(
                    "package Loaf changed its receipt-bound execution contract during immutable export",
                ));
            }
            // A shared direct plan can be byte-identical to a plan first sealed under another compatible receipt.
            // The package store must still publish that verified content under this library output's receipt, so its
            // portable entry identity is the destination publication rather than the reusable source entry.
            Ok(OvenPackagedLibraryLoafEntry {
                receipt: entry.receipt,
                identity: exported.identity,
                kind: entry.kind,
                base_loaf_identity: entry.base_loaf_identity,
            })
        })
        .collect()
}

/// Copy one selected immutable entry, or its receipt-compatible direct-plan equivalent, through destination validation.
///
/// A package Loaf is never a directory alias to another cache. The destination verifies every copied artifact and
/// calculates its own bounded admission before making the entry visible.
pub fn copy_receipted_oven_store_entry(
    source_store: &OvenStore,
    destination_store: &OvenStore,
    receipt: &oven_store::OvenReceipt,
    entry_identity: &str,
    entry_kind: OvenArtifactKind,
    operation: &str,
) -> CliResult<oven_store::store::OvenArtifactManifest> {
    if let Some(existing) = existing_provider_loaf(destination_store, receipt, entry_identity, entry_kind, operation)? {
        return Ok(existing);
    }
    let mut selected = source_store
        .select_payloads_matching_for_execution(|manifest| {
            manifest.identity == entry_identity
                && manifest.kind == entry_kind
                && manifest.build_unit_identity == receipt.build_unit_identity
                && manifest.intent == receipt.intent
        })
        .map_err(|error| CliError::failure(format!("failed to select provider Loaf for {operation}: {error}")))?;
    if selected.is_empty() && entry_kind == OvenArtifactKind::DirectRustcPlan {
        // Direct plans are reusable by their verified build unit and intent. A semantically inert source edit can
        // change the caller receipt while preserving the exact sealed Rust closure, leaving its source-store identity
        // under the original receipt. Select that compatible closure only through the ordinary receipt-aware planner,
        // then re-publish it below with the package's current receipt.
        if let Some(compatible) =
            select_direct_rustc_plan_for_execution(source_store, receipt).map_err(oven_rustc_error)?
        {
            selected.push(compatible);
        }
    }
    if selected.len() != 1 {
        return Err(CliError::failure(format!(
            "expected one receipt-selected provider Loaf `{entry_identity}` for {operation}, found {}",
            selected.len()
        )));
    }
    publish_selected_provider_loaf(
        selected.remove(0),
        destination_store,
        receipt,
        entry_identity,
        entry_kind,
        operation,
    )
}

/// Select an already admitted destination entry without rereading another store's materialized closure.
fn existing_provider_loaf(
    destination_store: &OvenStore,
    receipt: &oven_store::OvenReceipt,
    entry_identity: &str,
    entry_kind: OvenArtifactKind,
    operation: &str,
) -> CliResult<Option<oven_store::store::OvenArtifactManifest>> {
    let existing = destination_store
        .select_payloads_matching_for_execution(|manifest| {
            manifest.identity == entry_identity
                && manifest.kind == entry_kind
                && manifest.receipt_identity == receipt.identity
                && manifest.build_unit_identity == receipt.build_unit_identity
                && manifest.intent == receipt.intent
        })
        .map_err(|error| CliError::failure(format!("failed to inspect provider Loaf before {operation}: {error}")))?;
    if existing.len() > 1 {
        return Err(CliError::failure(format!(
            "expected at most one existing provider Loaf `{entry_identity}` before {operation}, found {}",
            existing.len()
        )));
    }
    Ok(existing.into_iter().next().map(|payload| payload.manifest))
}

/// Import a leased, verified source entry through the destination store's ordinary bounded publication.
fn publish_selected_provider_loaf(
    selected: oven_store::store::OvenStoreExecutionPayload,
    destination_store: &OvenStore,
    receipt: &oven_store::OvenReceipt,
    entry_identity: &str,
    entry_kind: OvenArtifactKind,
    operation: &str,
) -> CliResult<oven_store::store::OvenArtifactManifest> {
    if let Some(existing) = existing_provider_loaf(destination_store, receipt, entry_identity, entry_kind, operation)? {
        return Ok(existing);
    }
    // The record, payload bytes and witness are proven here; the file closure is proven by the publication below,
    // which reads every one of these files to describe the destination entry anyway. Verifying it twice cost a second
    // full hash of the whole provider closure for an answer the describing read already produces.
    selected.verify_admitted_record().map_err(|error| {
        CliError::failure(format!(
            "failed to verify source provider Loaf during {operation}: {error}"
        ))
    })?;
    let admitted_files = selected.admitted_materialized_files().to_vec();
    let admitted_directories = selected.admitted_materialized_directories().to_vec();
    let (manifest, artifact_root, payload, _lease) = selected.into_parts();
    if manifest.kind != entry_kind
        || manifest.build_unit_identity != receipt.build_unit_identity
        || manifest.intent != receipt.intent
        || (entry_kind != OvenArtifactKind::DirectRustcPlan && manifest.receipt_identity != receipt.identity)
    {
        return Err(CliError::failure(format!(
            "selected provider Loaf `{entry_identity}` changed its receipt-bound execution contract during {operation}"
        )));
    }
    let materialized_files = manifest
        .materialized_files
        .iter()
        .map(|file| OvenArtifactMaterializedFile {
            source_path: artifact_root.join(&file.relative_path),
            relative_path: file.relative_path.clone(),
        })
        .collect::<Vec<_>>();
    let materialized_directories = manifest
        .materialized_directories
        .iter()
        .map(|directory| OvenArtifactMaterializedDirectory {
            source_path: artifact_root.join(&directory.relative_path),
            relative_path: directory.relative_path.clone(),
        })
        .collect::<Vec<_>>();
    let exported = destination_store
        .publish_verified_import(
            &OvenArtifactPublishRequest {
                receipt: receipt.clone(),
                domain: manifest.domain.clone(),
                kind: manifest.kind,
                payload,
                materialized_files,
                materialized_directories,
            },
            &admitted_files,
            &admitted_directories,
        )
        .map_err(|error| CliError::failure(format!("failed to publish provider Loaf during {operation}: {error}")))?;
    if exported.kind != entry_kind
        || exported.receipt_identity != receipt.identity
        || exported.build_unit_identity != receipt.build_unit_identity
        || exported.intent != receipt.intent
    {
        return Err(CliError::failure(
            "provider Loaf changed its receipt-bound execution contract during immutable store copy",
        ));
    }
    Ok(exported)
}

/// Atomically publish the package-local index only after every referenced Loaf and library output exists.
pub fn write_packaged_library_loaf_manifest(
    artifact_root: &Path,
    manifest: &OvenPackagedLibraryLoafManifest,
) -> CliResult<()> {
    let path = packaged_library_loaf_manifest_path(artifact_root);
    let parent = path
        .parent()
        .ok_or_else(|| CliError::failure(format!("package Loaf manifest path has no parent: {}", path.display())))?;
    fs::create_dir_all(parent).map_err(|error| {
        CliError::failure(format!(
            "failed to create package Loaf directory {}: {error}",
            parent.display()
        ))
    })?;
    let payload = serde_json::to_vec_pretty(manifest)
        .map_err(|error| CliError::failure(format!("failed to encode package Loaf manifest: {error}")))?;
    let staged = parent.join(format!(".package-loafs-{}.tmp", std::process::id()));
    oven_store::write_receipt_staged(&payload, &staged, &path, parent).map_err(|error| {
        CliError::failure(format!(
            "failed to publish package Loaf manifest {}: {error}",
            path.display()
        ))
    })
}

/// Decode one provider's small package-Loaf index and validate its release-level facts.
pub fn decode_packaged_library_loaf_manifest(
    artifact: &LibraryArtifactMetadata,
) -> CliResult<Option<(PathBuf, String, OvenPackagedLibraryLoafManifest)>> {
    let path = packaged_library_loaf_manifest_path(&artifact.crate_root);
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(CliError::failure(format!(
                "Oven Alpha cannot read package Loaf manifest for pub::{} at {}: {error}",
                artifact.dependency_key,
                path.display()
            )));
        }
    };
    let manifest = serde_json::from_slice::<OvenPackagedLibraryLoafManifest>(&bytes).map_err(|error| {
        CliError::failure(format!(
            "Oven Alpha cannot parse package Loaf manifest for pub::{} at {}: {error}",
            artifact.dependency_key,
            path.display()
        ))
    })?;
    if manifest.schema_version != OVEN_PACKAGED_LIBRARY_LOAF_SCHEMA_VERSION {
        return Err(CliError::failure(format!(
            "Oven Alpha cannot use pub::{} package Loaf manifest at {}: schema {} is unsupported; rebake the provider with this Incan release",
            artifact.dependency_key,
            path.display(),
            manifest.schema_version
        )));
    }
    if manifest.compiler_version != INCAN_VERSION {
        return Err(CliError::failure(format!(
            "Oven Alpha cannot use pub::{} package Loaf manifest at {}: it was baked by Incan {}, but this compiler is {}; rebake the provider with this Incan release",
            artifact.dependency_key,
            path.display(),
            manifest.compiler_version,
            INCAN_VERSION
        )));
    }
    let source_authority_hex = manifest
        .source_authority_digest
        .strip_prefix("sha256:")
        .ok_or_else(|| {
            CliError::failure(format!(
                "Oven Alpha cannot use pub::{} package Loaf manifest at {}: its source authority is not a canonical SHA-256 digest",
                artifact.dependency_key,
                path.display()
            ))
        })?;
    if source_authority_hex.len() != 64
        || !source_authority_hex
            .chars()
            .all(|character| character.is_ascii_hexdigit())
    {
        return Err(CliError::failure(format!(
            "Oven Alpha cannot use pub::{} package Loaf manifest at {}: its source authority is not a canonical SHA-256 digest",
            artifact.dependency_key,
            path.display()
        )));
    }
    let canonical_path = fs::canonicalize(&path).map_err(|error| {
        CliError::failure(format!(
            "Oven Alpha cannot resolve package Loaf manifest for pub::{} at {}: {error}",
            artifact.dependency_key,
            path.display()
        ))
    })?;
    Ok(Some((canonical_path, digest_bytes(&bytes), manifest)))
}

/// Read one provider's immutable package-Loaf index and verify its release and local source authority.
///
/// Installed artifact-only providers have no authored project tree and are validated solely through their sealed
/// manifest, receipts, and output digests. A path dependency still has its source project beside `target/lib`; its
/// recursive authored digest must match so an edited provider can never be hidden behind an older package Loaf.
pub fn read_packaged_library_loaf_manifest(
    artifact: &LibraryArtifactMetadata,
) -> CliResult<Option<OvenPackagedLibraryLoafManifest>> {
    let Some((_path, _manifest_digest, manifest)) = decode_packaged_library_loaf_manifest(artifact)? else {
        return Ok(None);
    };
    validate_packaged_library_metadata_files(artifact, &manifest)?;
    if let Some(project_root) = artifact.crate_root.parent().and_then(Path::parent)
        && project_root.join(LOAF_MANIFEST_FILENAME).is_file()
    {
        let source_authority_digest = digest_baked_project_source_authority(project_root)?;
        if manifest.source_authority_digest != source_authority_digest {
            return Err(CliError::failure(format!(
                "Oven Alpha refuses pub::{} because its source project at {} changed after the package Loaf was baked; rebake that provider before baking or running a consumer",
                artifact.dependency_key,
                project_root.display()
            )));
        }
    }
    Ok(Some(manifest))
}

/// Resolve one package-owned native output without permitting a symlink escape from its artifact root.
fn validated_packaged_library_output_path(
    artifact: &LibraryArtifactMetadata,
    relative_path: &str,
    profile: &str,
) -> CliResult<PathBuf> {
    let relative = validated_project_output_relative_path(relative_path, "package-owned library output")?;
    let output = artifact.crate_root.join(relative);
    let metadata = fs::symlink_metadata(&output).map_err(|error| {
        CliError::failure(format!(
            "Oven Alpha cannot inspect package-owned `{profile}` library for pub::{} at {}: {error}",
            artifact.dependency_key,
            output.display()
        ))
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(CliError::failure(format!(
            "Oven Alpha refuses package Loaf for pub::{}: `{profile}` library must be a regular file below its artifact root",
            artifact.dependency_key
        )));
    }
    let canonical_root = fs::canonicalize(&artifact.crate_root).map_err(|error| {
        CliError::failure(format!(
            "Oven Alpha cannot resolve package artifact root for pub::{} at {}: {error}",
            artifact.dependency_key,
            artifact.crate_root.display()
        ))
    })?;
    let canonical_output = fs::canonicalize(&output).map_err(|error| {
        CliError::failure(format!(
            "Oven Alpha cannot resolve package-owned `{profile}` library for pub::{} at {}: {error}",
            artifact.dependency_key,
            output.display()
        ))
    })?;
    if !canonical_output.starts_with(&canonical_root) {
        return Err(CliError::failure(format!(
            "Oven Alpha refuses package Loaf for pub::{}: `{profile}` library escapes its artifact root through a symlink",
            artifact.dependency_key
        )));
    }
    Ok(output)
}

/// Return a package-owned profile record only when it can link with the active direct-Rustc target and toolchain.
#[cfg(test)]
fn packaged_library_loaf_profile(
    artifact: &LibraryArtifactMetadata,
    profile: &str,
    target: &str,
    toolchain: &str,
) -> CliResult<Option<OvenPackagedLibraryLoafProfile>> {
    let Some(manifest) = read_packaged_library_loaf_manifest(artifact)? else {
        return Ok(None);
    };
    validated_packaged_library_loaf_profile(artifact, &manifest, profile, target, toolchain)
}

/// Validate one profile against an already admitted provider manifest.
pub fn validated_packaged_library_loaf_profile(
    artifact: &LibraryArtifactMetadata,
    manifest: &OvenPackagedLibraryLoafManifest,
    profile: &str,
    target: &str,
    toolchain: &str,
) -> CliResult<Option<OvenPackagedLibraryLoafProfile>> {
    let Some(candidate) = manifest.profiles.get(profile) else {
        return Ok(None);
    };
    candidate.receipt.verify_identity().map_err(|error| {
        CliError::failure(format!(
            "Oven Alpha refuses package Loaf for pub::{} because its `{profile}` receipt is invalid: {error}",
            artifact.dependency_key
        ))
    })?;
    if candidate.receipt.intent.profile != profile
        || candidate.receipt.intent.target != target
        || candidate.receipt.intent.toolchain != toolchain
    {
        return Ok(None);
    }
    for entry in &candidate.entries {
        entry.receipt.verify_identity().map_err(|error| {
            CliError::failure(format!(
                "Oven Alpha refuses package Loaf for pub::{} because entry `{}` has an invalid receipt: {error}",
                artifact.dependency_key, entry.identity
            ))
        })?;
        if entry.identity.trim().is_empty() {
            return Err(CliError::failure(format!(
                "Oven Alpha refuses package Loaf for pub::{}: its `{profile}` entries must have an immutable identity",
                artifact.dependency_key
            )));
        }
        if entry.receipt.intent != candidate.receipt.intent {
            return Err(CliError::failure(format!(
                "Oven Alpha refuses package Loaf for pub::{}: entry `{}` has a different sealed intent from its library output",
                artifact.dependency_key, entry.identity
            )));
        }
    }
    let output = validated_packaged_library_output_path(artifact, &candidate.library_relative_path, profile)?;
    let actual_digest = digest_bytes(&fs::read(&output).map_err(|error| {
        CliError::failure(format!(
            "Oven Alpha cannot read package-owned `{profile}` library for pub::{} at {}: {error}",
            artifact.dependency_key,
            output.display()
        ))
    })?);
    if actual_digest != candidate.library_digest {
        return Err(CliError::failure(format!(
            "Oven Alpha refuses package Loaf for pub::{}: `{profile}` library digest {actual_digest} differs from sealed package digest {}",
            artifact.dependency_key, candidate.library_digest
        )));
    }
    Ok(Some(candidate.clone()))
}

/// Return whether a store already contains every exact immutable entry required by one package profile.
///
/// This is intentionally receipt-bound rather than an identity-only cache probe: an unrelated artifact with the
/// same digest-shaped name cannot become a substitute for a provider's sealed Rust dependency closure.
fn has_complete_packaged_library_loaf(store: &OvenStore, entries: &[OvenPackagedLibraryLoafEntry]) -> CliResult<bool> {
    for entry in entries {
        let selected = store
            .select_payloads_matching_for_execution(|stored| {
                stored.identity == entry.identity
                    && stored.kind == entry.kind
                    && stored.receipt_identity == entry.receipt.identity
                    && stored.build_unit_identity == entry.receipt.build_unit_identity
                    && stored.intent == entry.receipt.intent
            })
            .map_err(|error| {
                CliError::failure(format!(
                    "failed to inspect package Loaf `{}` before consumer import: {error}",
                    entry.identity
                ))
            })?;
        if selected.len() != 1 {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Import a compatible public provider closure into the consumer's bounded Oven store without running Cargo.
///
/// This is the explicit consumer bake boundary. A provider artifact normally carries its own portable package-store
/// export. After a local output-only restoration, that duplicate export is deliberately absent; the matching
/// receipt-bound entries remain in the primary Oven store and can be selected there without re-publishing them on a
/// normal provider build.
pub fn import_checked_packaged_library_loaf(
    consumer_store: &OvenStore,
    checked: &CheckedPackagedProviderProfile,
) -> CliResult<()> {
    let package_profile = &checked.package;
    let package_store_root = packaged_library_loaf_store_root(&checked.artifact_root);
    let package_store_exists = package_store_root.try_exists().map_err(|error| {
        CliError::failure(format!(
            "failed to inspect published provider store {}: {error}",
            package_store_root.display()
        ))
    })?;
    if package_store_exists {
        let selected = PublishedOvenStore::new(&package_store_root)
            .select_payloads_matching_for_execution(|stored| {
                package_profile.entries.iter().any(|entry| {
                    stored.identity == entry.identity
                        && stored.kind == entry.kind
                        && stored.receipt_identity == entry.receipt.identity
                        && stored.build_unit_identity == entry.receipt.build_unit_identity
                        && stored.intent == entry.receipt.intent
                })
            })
            .map_err(|error| {
                CliError::failure(format!(
                    "failed to read published package Loaf for pub::{}: {error}",
                    checked.dependency_key
                ))
            })?;
        if selected.len() == package_profile.entries.len() {
            for payload in selected {
                let identity = payload.manifest.identity.clone();
                let kind = payload.manifest.kind;
                let entry = package_profile
                    .entries
                    .iter()
                    .find(|entry| entry.identity == identity)
                    .ok_or_else(|| CliError::failure("selected package Loaf is absent from its checked profile"))?;
                let imported = publish_selected_provider_loaf(
                    payload,
                    consumer_store,
                    &entry.receipt,
                    &identity,
                    kind,
                    "consumer package-Loaf import",
                )?;
                if imported.identity != identity {
                    return Err(CliError::failure(format!(
                        "consumer package-Loaf import changed the sealed entry identity `{identity}`"
                    )));
                }
            }
            return Ok(());
        }
    }
    if !has_complete_packaged_library_loaf(consumer_store, &package_profile.entries)? {
        return Err(CliError::failure(format!(
            "Oven Alpha cannot import pub::{}: its portable package Loaf is absent and the current Oven store has no matching receipt-bound closure; run `incan oven bake --project {}`",
            checked.dependency_key,
            checked.artifact_root.display()
        )));
    }
    for entry in &package_profile.entries {
        let imported = copy_receipted_oven_store_entry(
            consumer_store,
            consumer_store,
            &entry.receipt,
            &entry.identity,
            entry.kind,
            "consumer package-Loaf import",
        )?;
        if imported.identity != entry.identity {
            return Err(CliError::failure(format!(
                "consumer package-Loaf import changed the sealed entry identity `{}`",
                entry.identity
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeMap, BTreeSet};
    use std::fs;

    use sha2::Sha256;

    use crate::build::caller_owned::caller_owned_library_receipt;
    use crate::build::library_outputs::packaged_library_loaf_store_root;
    use crate::build::output_paths::packaged_library_metadata_files;
    use crate::build::source_authority::digest_baked_project_source_authority;
    use crate::build::{
        CheckedPackagedProviderProfile, OVEN_PACKAGED_LIBRARY_LOAF_SCHEMA_VERSION, OvenPackagedLibraryLoafManifest,
        OvenPackagedLibraryLoafProfile, packaged_provider_candidates,
    };
    use incan_frontend::library_manifest::published_layout::packaged_library_loaf_manifest_path;
    use incan_frontend::library_manifest::{LibraryManifest, digest_provider_artifact};
    use incan_frontend::library_manifest_index::LibraryArtifactMetadata;
    use incan_lang::version::INCAN_VERSION;
    use oven_model::manifest::LOAF_MANIFEST_FILENAME;
    use oven_rustc::plan::composition::compose_selected_packaged_provider_plan;
    use oven_rustc::plan::selection::select_packaged_provider_plans;
    use oven_rustc::plan::test_support::{package_loaf_manifest, recapture_package_loaf_closure};
    use oven_rustc::plan::{
        OvenDirectRustcPlanSelection, OvenPackagedLibraryLoafEntry, OvenPackagedProviderExecutionPlan,
    };
    use oven_store::store::{OvenArtifactKind, OvenArtifactMaterializedFile, OvenArtifactPublishRequest, OvenStore};
    use oven_store::{OvenGeneratedProjectRequest, digest_bytes, receipt_generated_project, write_receipt};
    use sha2::Digest as _;

    #[test]
    fn direct_plan_package_loaf_composes_from_provider_into_consumer() -> Result<(), Box<dyn std::error::Error>> {
        let intent = oven_store::OvenBuildIntent {
            target: "aarch64-apple-darwin".to_string(),
            toolchain: "rustc fixture".to_string(),
            profile: "debug".to_string(),
            features: Vec::new(),
        };
        let provider = tempfile::tempdir()?;
        let provider_source = provider.path().join("src/lib.rs");
        fs::create_dir_all(provider_source.parent().ok_or("provider source has no parent")?)?;
        fs::write(&provider_source, "pub fn provider() {}\n")?;
        let provider_receipt = receipt_generated_project(
            &OvenGeneratedProjectRequest::new(
                provider.path(),
                "provider",
                "0.1.0",
                intent.target.clone(),
                intent.toolchain.clone(),
                intent.profile.clone(),
                Vec::new(),
            )
            .with_generated_source("generated-root", &provider_source),
        )?;
        let consumer = tempfile::tempdir()?;
        let consumer_source = consumer.path().join("src/main.rs");
        fs::create_dir_all(consumer_source.parent().ok_or("consumer source has no parent")?)?;
        fs::write(&consumer_source, "fn main() {}\n")?;
        let consumer_receipt = receipt_generated_project(
            &OvenGeneratedProjectRequest::new(
                consumer.path(),
                "consumer",
                "0.1.0",
                intent.target.clone(),
                intent.toolchain.clone(),
                intent.profile.clone(),
                Vec::new(),
            )
            .with_generated_source("generated-root", &consumer_source),
        )?;
        let mut artifacts = package_loaf_manifest(intent, "provider", "sha256:provider");
        let materialized_files = artifacts
            .externs
            .iter_mut()
            .enumerate()
            .map(|(index, artifact)| {
                let source = provider.path().join(format!("sealed-{index}.rlib"));
                fs::write(&source, format!("sealed {} artifact", artifact.crate_name))?;
                artifact.digest = digest_bytes(&fs::read(&source)?);
                Ok(OvenArtifactMaterializedFile {
                    source_path: source,
                    relative_path: artifact.relative_path.clone(),
                })
            })
            .collect::<Result<Vec<_>, std::io::Error>>()?;
        recapture_package_loaf_closure(&mut artifacts);
        let limits = oven_store::store::OvenStoreLimits::new(1024 * 1024, 1024 * 1024, 1024 * 1024);
        let artifact_root = provider.path().join("target/incan/provider");
        let package_store = OvenStore::new(packaged_library_loaf_store_root(&artifact_root), limits);
        let stored = package_store.publish(&OvenArtifactPublishRequest {
            receipt: provider_receipt.clone(),
            domain: "provider-package".to_string(),
            kind: OvenArtifactKind::DirectRustcPlan,
            payload: serde_json::to_vec(&artifacts)?,
            materialized_files,
            materialized_directories: Vec::new(),
        })?;
        let checked = CheckedPackagedProviderProfile {
            dependency_key: "provider".to_string(),
            artifact_root,
            profile: "debug".to_string(),
            package: OvenPackagedLibraryLoafProfile {
                receipt: provider_receipt.clone(),
                entries: vec![OvenPackagedLibraryLoafEntry {
                    receipt: provider_receipt,
                    identity: stored.identity.clone(),
                    kind: OvenArtifactKind::DirectRustcPlan,
                    base_loaf_identity: None,
                }],
                library_relative_path: "oven/debug/libprovider.rlib".to_string(),
                library_digest: "sha256:provider-library".to_string(),
            },
        };
        let consumer_store_root = tempfile::tempdir()?;
        let consumer_store = OvenStore::new(consumer_store_root.path(), limits);
        let (entry, lease) = package_store.select(&stored.identity)?;
        #[cfg(unix)]
        let entry_artifacts = entry.materialized_root();
        let entry_root = entry.path;
        drop(lease);
        fs::write(entry_root.join("last-used"), b"1\n")?;
        let published_digest = digest_provider_artifact(&checked.artifact_root)?;
        import_checked_packaged_library_loaf(&consumer_store, &checked)?;
        assert_eq!(digest_provider_artifact(&checked.artifact_root)?, published_digest);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            let leaf = stored
                .materialized_files
                .first()
                .ok_or("fixture has no materialized leaf")?;
            let source = entry_artifacts.join(&leaf.relative_path);
            let original = fs::metadata(&source)?.permissions();
            fs::set_permissions(&source, fs::Permissions::from_mode(0o0))?;
            let unreadable = fs::File::open(&source).is_err();
            let warm = import_checked_packaged_library_loaf(&consumer_store, &checked);
            fs::set_permissions(&source, original)?;
            assert!(
                unreadable,
                "the test host must enforce the source leaf's read permissions"
            );
            warm?;
        }
        import_checked_packaged_library_loaf(&consumer_store, &checked)?;
        assert_eq!(digest_provider_artifact(&checked.artifact_root)?, published_digest);
        assert_eq!(fs::read(entry_root.join("last-used"))?, b"1\n");
        let checked = [checked];
        let candidates = packaged_provider_candidates(&checked, "debug");
        let inputs = select_packaged_provider_plans(&consumer_store, &candidates)?;
        let selected = compose_selected_packaged_provider_plan(inputs, &candidates, &consumer_receipt)?
            .ok_or("consumer should select the imported direct-plan package Loaf")?;
        let OvenDirectRustcPlanSelection::PackagedProvider(packages) = selected else {
            return Err("consumer did not retain the packaged provider closure".into());
        };
        assert!(matches!(&*packages, OvenPackagedProviderExecutionPlan::Direct(_)));
        assert_eq!(
            packages
                .artifact_plan()
                .externs
                .iter()
                .map(|(crate_name, _)| crate_name.as_str())
                .collect::<BTreeSet<_>>(),
            BTreeSet::from(["incan_std_core", "provider"])
        );
        assert!(packages.report_identity().contains(&stored.identity));
        Ok(())
    }

    #[test]
    fn packaged_library_loaf_profile_requires_its_sealed_output_and_matching_cohort()
    -> Result<(), Box<dyn std::error::Error>> {
        let package = tempfile::tempdir()?;
        let artifact_root = package.path().join("target/lib");
        let authored_source = package.path().join("src/lib.incn");
        let source = artifact_root.join("src/lib.rs");
        let output = artifact_root.join("oven/debug/libprovider.rlib");
        fs::create_dir_all(authored_source.parent().ok_or("provider source has no parent")?)?;
        fs::create_dir_all(source.parent().ok_or("provider source has no parent")?)?;
        fs::create_dir_all(output.parent().ok_or("provider output has no parent")?)?;
        fs::write(
            package.path().join(LOAF_MANIFEST_FILENAME),
            "[project]\nname = \"provider\"\nversion = \"0.1.0\"\n",
        )?;
        fs::write(&authored_source, "pub def provider() -> int:\n    return 1\n")?;
        fs::write(&source, "pub fn provider() {}\n")?;
        fs::write(&output, b"sealed provider output")?;
        let sidecar = artifact_root.join("desugarers/provider.wasm");
        fs::create_dir_all(sidecar.parent().ok_or("provider sidecar has no parent")?)?;
        let sealed_sidecar = b"sealed provider desugarer";
        fs::write(&sidecar, sealed_sidecar)?;
        let mut library_manifest = LibraryManifest::new("provider", "0.1.0");
        library_manifest.vocab = Some(incan_frontend::library_manifest::VocabExports {
            crate_path: "vocab_companion".to_string(),
            package_name: "provider_vocab_companion".to_string(),
            keyword_registrations: Vec::new(),
            dsl_surfaces: Vec::new(),
            provider_manifest: incan_vocab::LibraryManifest::default(),
            desugarer_artifact: Some(incan_frontend::library_manifest::VocabDesugarerArtifact {
                artifact_kind: incan_vocab::DesugarerArtifactKind::WasmModule,
                abi_version: incan_vocab::WASM_DESUGAR_ABI_VERSION,
                relative_path: "desugarers/provider.wasm".to_string(),
                target: "wasm32-wasip1".to_string(),
                profile: "release".to_string(),
                entrypoint: "desugar_block".to_string(),
                sha256: hex::encode(Sha256::digest(sealed_sidecar)),
            }),
        });
        let library_manifest_path = artifact_root.join("provider.incnlib");
        library_manifest.write_to_path(&library_manifest_path)?;
        fs::write(
            artifact_root.join("Cargo.toml"),
            "[package]\nname = \"provider\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )?;
        let artifact = LibraryArtifactMetadata::from_crate_root("provider", "provider", &artifact_root);
        let receipt = oven_store::receipt_generated_project(
            &oven_store::OvenGeneratedProjectRequest::new(
                &artifact_root,
                "provider",
                "0.1.0",
                "aarch64-apple-darwin",
                "rustc fixture",
                "debug",
                Vec::new(),
            )
            .with_generated_source("generated-root", &source),
        )?;
        let release_receipt = oven_store::receipt_generated_project(
            &oven_store::OvenGeneratedProjectRequest::new(
                &artifact_root,
                "provider",
                "0.1.0",
                "aarch64-apple-darwin",
                "rustc fixture",
                "release",
                Vec::new(),
            )
            .with_generated_source("generated-root", &source),
        )?;
        let release_receipt_identity = release_receipt.identity.clone();
        let sealed_output_digest = digest_bytes(&fs::read(&output)?);
        let mut manifest = OvenPackagedLibraryLoafManifest {
            schema_version: OVEN_PACKAGED_LIBRARY_LOAF_SCHEMA_VERSION,
            source_authority_digest: digest_baked_project_source_authority(package.path())?,
            compiler_version: INCAN_VERSION.to_string(),
            metadata_files: packaged_library_metadata_files(&library_manifest_path, &library_manifest, &artifact_root)?,
            profiles: BTreeMap::from([
                (
                    "debug".to_string(),
                    OvenPackagedLibraryLoafProfile {
                        receipt,
                        entries: Vec::new(),
                        library_relative_path: "oven/debug/libprovider.rlib".to_string(),
                        library_digest: sealed_output_digest.clone(),
                    },
                ),
                (
                    "release".to_string(),
                    OvenPackagedLibraryLoafProfile {
                        receipt: release_receipt,
                        entries: Vec::new(),
                        library_relative_path: "oven/debug/libprovider.rlib".to_string(),
                        library_digest: sealed_output_digest.clone(),
                    },
                ),
            ]),
        };
        write_packaged_library_loaf_manifest(&artifact_root, &manifest)?;

        let executable_receipt = oven_store::receipt_generated_project(
            &oven_store::OvenGeneratedProjectRequest::new(
                &artifact_root,
                "provider_executable",
                "0.1.0",
                "aarch64-apple-darwin",
                "rustc fixture",
                "release",
                Vec::new(),
            )
            .with_generated_source("generated-root", &source),
        )?;
        write_receipt(&executable_receipt, oven_store::default_receipt_path(package.path()))?;
        let release_artifacts =
            package_loaf_manifest(executable_receipt.intent.clone(), "provider", &sealed_output_digest);
        let selected_library_receipt = caller_owned_library_receipt(&artifact, "release", &release_artifacts, None)?;
        assert_eq!(selected_library_receipt.identity, release_receipt_identity);
        assert_ne!(selected_library_receipt.identity, executable_receipt.identity);

        assert!(packaged_library_loaf_profile(&artifact, "debug", "aarch64-apple-darwin", "rustc fixture")?.is_some());
        assert!(
            packaged_library_loaf_profile(&artifact, "debug", "x86_64-unknown-linux-gnu", "rustc fixture")?.is_none()
        );

        let sealed_library_manifest = fs::read(&library_manifest_path)?;
        let mut changed_library_manifest = sealed_library_manifest.clone();
        changed_library_manifest.push(b'\n');
        fs::write(&library_manifest_path, changed_library_manifest)?;
        let metadata_error = packaged_library_loaf_profile(&artifact, "debug", "aarch64-apple-darwin", "rustc fixture")
            .err()
            .ok_or("changed checked package metadata must fail closed")?;
        assert!(metadata_error.to_string().contains("checked library metadata"));
        fs::write(&library_manifest_path, &sealed_library_manifest)?;

        fs::write(&sidecar, b"changed provider desugarer")?;
        let sidecar_error = packaged_library_loaf_profile(&artifact, "debug", "aarch64-apple-darwin", "rustc fixture")
            .err()
            .ok_or("changed manifest-declared package sidecar must fail closed")?;
        assert!(sidecar_error.to_string().contains("declared sidecars"));
        fs::write(&sidecar, sealed_sidecar)?;

        manifest.schema_version = OVEN_PACKAGED_LIBRARY_LOAF_SCHEMA_VERSION - 1;
        write_packaged_library_loaf_manifest(&artifact_root, &manifest)?;
        let schema_error = packaged_library_loaf_profile(&artifact, "debug", "aarch64-apple-darwin", "rustc fixture")
            .err()
            .ok_or("a package manifest from the previous package release-cohort schema must fail closed")?;
        assert!(schema_error.to_string().contains("schema"));
        manifest.schema_version = OVEN_PACKAGED_LIBRARY_LOAF_SCHEMA_VERSION;

        manifest.compiler_version = "0.4.0".to_string();
        write_packaged_library_loaf_manifest(&artifact_root, &manifest)?;
        let release_error = packaged_library_loaf_profile(&artifact, "debug", "aarch64-apple-darwin", "rustc fixture")
            .err()
            .ok_or("a package Loaf from another Incan release must fail closed")?;
        assert!(release_error.to_string().contains("baked by Incan"));
        manifest.compiler_version = INCAN_VERSION.to_string();

        let outside_output = package.path().join("target/outside.rlib");
        fs::write(&outside_output, b"outside provider output")?;
        let profile = manifest
            .profiles
            .get_mut("debug")
            .ok_or("fixture package manifest has no debug profile")?;
        profile.library_relative_path = "../outside.rlib".to_string();
        profile.library_digest = digest_bytes(&fs::read(&outside_output)?);
        write_packaged_library_loaf_manifest(&artifact_root, &manifest)?;
        let path_error = packaged_library_loaf_profile(&artifact, "debug", "aarch64-apple-darwin", "rustc fixture")
            .err()
            .ok_or("an escaping package library path must fail closed")?;
        assert!(
            path_error
                .to_string()
                .contains("unsafe package-owned library output path")
        );
        let profile = manifest
            .profiles
            .get_mut("debug")
            .ok_or("fixture package manifest has no debug profile")?;
        profile.library_relative_path = "oven/debug/libprovider.rlib".to_string();
        profile.library_digest = sealed_output_digest.clone();
        write_packaged_library_loaf_manifest(&artifact_root, &manifest)?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;

            fs::remove_file(&output)?;
            symlink(&outside_output, &output)?;
            let symlink_error =
                packaged_library_loaf_profile(&artifact, "debug", "aarch64-apple-darwin", "rustc fixture")
                    .err()
                    .ok_or("a package-owned library symlink must fail closed")?;
            assert!(symlink_error.to_string().contains("must be a regular file"));
            fs::remove_file(&output)?;
            fs::write(&output, b"sealed provider output")?;
        }

        fs::write(&authored_source, "pub def provider() -> int:\n    return 2\n")?;
        let source_error = packaged_library_loaf_profile(&artifact, "debug", "aarch64-apple-darwin", "rustc fixture")
            .err()
            .ok_or("an edited provider source tree must fail closed")?;
        assert!(
            source_error
                .to_string()
                .contains("changed after the package Loaf was baked")
        );
        fs::write(&authored_source, "pub def provider() -> int:\n    return 1\n")?;

        fs::write(&output, b"mutated provider output")?;
        let error = packaged_library_loaf_profile(&artifact, "debug", "aarch64-apple-darwin", "rustc fixture")
            .err()
            .ok_or("mutated packaged provider output must fail closed")?;
        assert!(error.to_string().contains("digest"));

        fs::write(&output, b"sealed provider output")?;
        let consumer = package.path().join("consumer");
        fs::create_dir_all(consumer.join("src"))?;
        fs::write(
            consumer.join(LOAF_MANIFEST_FILENAME),
            "[project]\nname = \"consumer\"\n\n[dependencies]\nprovider = { path = \"..\" }\n",
        )?;
        fs::write(consumer.join("src/main.incn"), "def main() -> None:\n    pass\n")?;
        let source_backed_consumer_authority = digest_baked_project_source_authority(&consumer)?;

        fs::remove_file(package.path().join(LOAF_MANIFEST_FILENAME))?;
        fs::remove_dir_all(package.path().join("src"))?;
        assert!(
            packaged_library_loaf_profile(&artifact, "debug", "aarch64-apple-darwin", "rustc fixture")?.is_some(),
            "a source-free installed package remains governed by its sealed release, receipt, path, and output facts"
        );
        assert_eq!(
            source_backed_consumer_authority,
            digest_baked_project_source_authority(&consumer)?,
            "relocating a sealed provider without its source tree must preserve the consumer's exact lineage"
        );

        manifest.source_authority_digest = digest_bytes(b"updated sealed provider source authority");
        write_packaged_library_loaf_manifest(&artifact_root, &manifest)?;
        assert_ne!(
            source_backed_consumer_authority,
            digest_baked_project_source_authority(&consumer)?,
            "a source-free provider edge must bind the package Loaf's sealed source authority"
        );

        manifest.source_authority_digest = "sha256:not-a-digest".to_string();
        write_packaged_library_loaf_manifest(&artifact_root, &manifest)?;
        let malformed_authority =
            packaged_library_loaf_profile(&artifact, "debug", "aarch64-apple-darwin", "rustc fixture")
                .err()
                .ok_or("a malformed package source-authority digest must fail closed")?;
        assert!(
            malformed_authority.to_string().contains("canonical SHA-256 digest"),
            "unexpected malformed source-authority diagnostic: {malformed_authority}"
        );

        fs::remove_file(packaged_library_loaf_manifest_path(&artifact_root))?;
        let missing_handoff = digest_baked_project_source_authority(&consumer)
            .err()
            .ok_or("a source-free provider without a sealed package Loaf must fail closed")?;
        assert!(missing_handoff.to_string().contains("without its sealed package Loaf"));
        Ok(())
    }

    // ---- Body IR input contract (#1166) ----
}
