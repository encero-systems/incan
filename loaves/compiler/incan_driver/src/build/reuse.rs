//! Reusing an already baked project: restoring a reused library package and the bake authority context.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::build::library_outputs::{library_publication_receipts, packaged_library_loaf_store_root};
use crate::build::output_materialization::{
    caller_project_output_path, materialize_project_output, project_output_projection_is_current,
};
use crate::build::output_paths::{validate_packaged_library_metadata_files, validated_project_output_relative_path};
use crate::build::output_selection::{baked_project_owner_identity, select_baked_project_output_with_source_authority};
use crate::build::package_loafs::{
    copy_receipted_oven_store_entry, decode_packaged_library_loaf_manifest, validated_packaged_library_loaf_profile,
};
use crate::build::plan_authority::explicit_bake_profiles;
use crate::build::source_authority::{
    baked_project_lock_dependencies_fingerprint, digest_baked_project_source_authority, project_bake_receipt_path,
};
use crate::build::{
    MemoizedPackagedProviderAuthority, OVEN_PACKAGED_LIBRARY_LOAF_SCHEMA_VERSION, OVEN_PROJECT_OUTPUT_ARTIFACT_PATH,
    OvenBakeProjectTarget, OvenPackagedLibraryLoafManifest, OvenPackagedLibraryLoafProfile,
    OvenProjectBakeAuthorityContext, OvenProjectBakeOutputReport, OvenProjectBakeProfileReport, OvenProjectBakeReport,
    OvenStoredProjectOutput, ProjectSourceAuthorityDigester, library_publication,
};
use crate::error::{CliError, CliResult};
use incan_frontend::library_manifest::published_layout::packaged_library_loaf_manifest_path;
use incan_frontend::library_manifest_index::LibraryArtifactMetadata;
use incan_lang::version::INCAN_VERSION;
use incan_provider::FeatureSelection;
use oven_model::manifest::LOAF_MANIFEST_FILENAME;
use oven_rustc::loaf::{
    resolve_compiler_owned_loaf_by_identity, resolve_compiler_owned_loaf_for_registry_dependencies,
};
use oven_rustc::rustc::{
    OvenLoadedProjectInspectionAuthority, OvenProjectInspectionConstituent, load_project_inspection_authority,
    resolve_active_rustc, rustc_host_target, rustc_identity,
};
use oven_store::store::{OvenArtifactKind, OvenStore};

/// Restore and validate the portable package handoff carried by reused library outputs.
fn restore_reused_library_package(
    project_root: &Path,
    store: &OvenStore,
    source_authority_digest: &str,
    outputs: &[&OvenStoredProjectOutput],
) -> CliResult<bool> {
    if outputs.is_empty() {
        return Ok(true);
    }
    let artifact_root = project_root.join("target/lib");
    let manifest_path = packaged_library_loaf_manifest_path(&artifact_root);
    let bytes = match fs::read(&manifest_path) {
        Ok(bytes) => bytes,
        Err(_) => return Ok(false),
    };
    let manifest = match serde_json::from_slice::<OvenPackagedLibraryLoafManifest>(&bytes) {
        Ok(manifest)
            if manifest.schema_version == OVEN_PACKAGED_LIBRARY_LOAF_SCHEMA_VERSION
                && manifest.source_authority_digest == source_authority_digest
                && manifest.compiler_version == INCAN_VERSION =>
        {
            manifest
        }
        Ok(_) | Err(_) => return Ok(false),
    };
    if manifest.profiles.len() != outputs.len() {
        return Ok(false);
    }
    let package_store = OvenStore::new(packaged_library_loaf_store_root(&artifact_root), *store.limits());
    for output in outputs {
        let Some(candidate) = manifest.profiles.get(&output.profile) else {
            return Ok(false);
        };
        if candidate.receipt.verify_identity().is_err()
            || candidate.receipt.identity != output.payload.receipt_identity
            || candidate.receipt.build_unit_identity != output.payload.build_unit_identity
            || candidate.receipt.intent != output.intent
            || candidate
                .receipt
                .sources
                .build_unit_inputs
                .get("compiler-version")
                .is_none_or(|version| version != INCAN_VERSION)
        {
            return Ok(false);
        }
        let Some(native) = output
            .payload
            .files
            .iter()
            .find(|file| file.output_relative_path == OVEN_PROJECT_OUTPUT_ARTIFACT_PATH)
        else {
            return Ok(false);
        };
        let expected_library_relative_path = match Path::new(&native.caller_relative_path).strip_prefix("target/lib") {
            Ok(path) => path.to_string_lossy().replace('\\', "/"),
            Err(_) => return Ok(false),
        };
        if validated_project_output_relative_path(&candidate.library_relative_path, "package library").is_err()
            || candidate.library_relative_path != expected_library_relative_path
            || candidate.library_digest != native.digest
            || output.payload.package_loaf_store_relative_path.as_deref() != Some("target/lib/oven/loafs")
        {
            return Ok(false);
        }
        let mut candidate_entries = candidate.entries.clone();
        candidate_entries.sort_by(|left, right| {
            (&left.identity, &left.receipt.identity).cmp(&(&right.identity, &right.receipt.identity))
        });
        let mut output_entries = output.payload.required_project_loafs.clone();
        output_entries.sort_by(|left, right| {
            (&left.identity, &left.receipt.identity).cmp(&(&right.identity, &right.receipt.identity))
        });
        if candidate_entries != output_entries {
            return Ok(false);
        }
        if candidate.entries.is_empty()
            && resolve_compiler_owned_loaf_for_registry_dependencies(&candidate.receipt, &[])
                .map_or(true, |selected| selected.is_none())
        {
            return Ok(false);
        }
        for entry in &candidate.entries {
            if entry.receipt.verify_identity().is_err() || entry.receipt.intent != candidate.receipt.intent {
                return Ok(false);
            }
            if let Some(base_loaf_identity) = entry.base_loaf_identity.as_deref()
                && resolve_compiler_owned_loaf_by_identity(&candidate.receipt, base_loaf_identity)
                    .map_or(true, |selected| selected.is_none())
            {
                return Ok(false);
            }
            let packaged = package_store.select_payloads_matching_for_execution(|stored| {
                stored.identity == entry.identity
                    && stored.kind == entry.kind
                    && stored.receipt_identity == entry.receipt.identity
                    && stored.build_unit_identity == entry.receipt.build_unit_identity
                    && stored.intent == entry.receipt.intent
            });
            match packaged {
                Ok(selected) if selected.len() == 1 => {}
                Ok(selected) if selected.is_empty() => {
                    let source = store.select_payloads_matching_for_execution(|stored| {
                        stored.identity == entry.identity
                            && stored.kind == entry.kind
                            && stored.receipt_identity == entry.receipt.identity
                            && stored.build_unit_identity == entry.receipt.build_unit_identity
                            && stored.intent == entry.receipt.intent
                    });
                    if source.map_or(true, |selected| selected.len() != 1) {
                        return Ok(false);
                    }
                    let copied = copy_receipted_oven_store_entry(
                        store,
                        &package_store,
                        &entry.receipt,
                        &entry.identity,
                        entry.kind,
                        "project bake package export",
                    )?;
                    if copied.identity != entry.identity || copied.kind != entry.kind {
                        return Err(CliError::failure(
                            "completed Oven library output changed identity while restoring its package Loaf",
                        ));
                    }
                }
                Ok(_) | Err(_) => return Ok(false),
            }
        }
    }
    Ok(true)
}

/// Return whether every compiler-shipped release Loaf a project inspection authority names is provided by the
/// active toolchain.
///
/// Only availability is decided here. A Loaf that exists but disagrees with the recorded build unit or intent is
/// left for [`crate::lock::registry_sources::prepare_project_registry_source_authorities`] to reject, so a
/// switched toolchain family reads as a cache miss while a tampered authority still fails closed.
fn project_authority_release_loafs_available(authority: &OvenLoadedProjectInspectionAuthority) -> CliResult<bool> {
    release_loaf_constituents_available(&authority.payload.constituents)
}

/// Decide [`project_authority_release_loafs_available`] over the bare constituent list, so the rule is testable
/// without a loaded authority.
fn release_loaf_constituents_available(constituents: &[OvenProjectInspectionConstituent]) -> CliResult<bool> {
    for constituent in constituents {
        if let OvenProjectInspectionConstituent::ReleaseLoaf {
            loaf_identity, receipt, ..
        } = constituent
            && resolve_compiler_owned_loaf_by_identity(receipt, loaf_identity)
                .map_err(|error| CliError::failure(error.to_string()))?
                .is_none()
        {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Return a previously baked project report only when every discovered target/profile remains exact.
///
/// Any stale, absent, or malformed evidence returns a cache miss so the explicit baker can repair it. Selection
/// completes before any caller projection is restored, and a full hit returns before frontend, codegen, or Rustc.
pub fn try_reuse_baked_project(
    project_root: &Path,
    targets: &[(OvenBakeProjectTarget, PathBuf)],
    store: &OvenStore,
    package_features: &FeatureSelection,
    authority_context: &mut OvenProjectBakeAuthorityContext,
) -> CliResult<Option<OvenProjectBakeReport>> {
    // A completed project-output payload is selected only for the default command projection. Feature-qualified project
    // outputs remain explicit bake results until their selection facts are part of the public normal command payload,
    // so never reuse the default package export for one.
    if package_features != &FeatureSelection::default() {
        return Ok(None);
    }
    let rustc = resolve_active_rustc().map_err(|error| CliError::failure(error.to_string()))?;
    let target = rustc_host_target(&rustc).map_err(|error| CliError::failure(error.to_string()))?;
    let toolchain = rustc_identity(&rustc).map_err(|error| CliError::failure(error.to_string()))?;
    let lock_dependencies_fingerprint = baked_project_lock_dependencies_fingerprint(project_root)?;
    let mut expected_outputs = Vec::new();
    for (project_target, entrypoint) in targets {
        for profile in explicit_bake_profiles() {
            let receipt_path = project_bake_receipt_path(project_root, *project_target, entrypoint, profile)?;
            let receipt = match fs::read(&receipt_path)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<oven_store::OvenReceipt>(&bytes).ok())
            {
                Some(receipt)
                    if receipt.verify_identity().is_ok()
                        && receipt.intent.profile == profile
                        && receipt.intent.target == target
                        && receipt.intent.toolchain == toolchain =>
                {
                    receipt
                }
                Some(_) | None => return Ok(None),
            };
            expected_outputs.push((
                *project_target,
                entrypoint.clone(),
                profile.to_string(),
                receipt_path,
                receipt,
            ));
        }
    }
    let headers = store
        .manifests_for_selection()
        .map_err(|error| CliError::failure(format!("failed to inspect Oven project-output headers: {error}")))?;
    if expected_outputs.iter().any(|(_, _, _, _, receipt)| {
        !headers.iter().any(|manifest| {
            manifest.kind == OvenArtifactKind::ProjectOutput
                && manifest.receipt_identity == receipt.identity
                && manifest.build_unit_identity == receipt.build_unit_identity
                && manifest.intent == receipt.intent
        })
    }) {
        return Ok(None);
    }

    // Only an exact local receipt set with matching immutable store headers earns the recursive authored-source scan.
    // Headers are a reject-only optimization; exact payload selection below remains the execution authority.
    // A cache candidate is only tentative until every receipt, payload, and inspection authority validates below.
    // Do not preserve this pre-refresh lock projection as this command's publication authority: an explicit bake
    // may refresh an old lock after the cache probe misses, and that compiler-owned refresh is not an authored edit.
    let source_authority_digest = authority_context.cache_probe_source_authority(project_root)?;
    let mut selected_outputs = Vec::new();
    for (project_target, entrypoint, profile, receipt_path, receipt) in expected_outputs {
        let Some(output) = select_baked_project_output_with_source_authority(
            store,
            project_root,
            &entrypoint,
            project_target,
            &profile,
            &source_authority_digest,
            Some((&target, &toolchain)),
        )?
        else {
            return Ok(None);
        };
        if output.payload.lock_dependencies_fingerprint != lock_dependencies_fingerprint {
            return Ok(None);
        }
        match project_target {
            OvenBakeProjectTarget::Library
                if output.payload.package_loaf_store_relative_path.as_deref() == Some("target/lib/oven/loafs") => {}
            OvenBakeProjectTarget::Library => return Ok(None),
            OvenBakeProjectTarget::Executable
                if output.payload.package_loaf_store_relative_path.is_none()
                    && output.payload.required_project_loafs.is_empty() => {}
            OvenBakeProjectTarget::Executable => return Ok(None),
        }
        if receipt.identity != output.payload.receipt_identity
            || receipt.build_unit_identity != output.payload.build_unit_identity
            || receipt.intent != output.intent
        {
            return Ok(None);
        }
        selected_outputs.push((project_target, output, receipt_path, receipt));
    }
    let authority_ref = selected_outputs
        .first()
        .and_then(|(_, output, _, _)| output.payload.inspection_authority.as_ref())
        .cloned()
        .ok_or_else(|| CliError::failure("completed Oven project outputs have no inspection authority"))?;
    if selected_outputs
        .iter()
        .any(|(_, output, _, _)| output.payload.inspection_authority.as_ref() != Some(&authority_ref))
    {
        return Ok(None);
    }
    let authority = load_project_inspection_authority(
        store,
        &authority_ref,
        &baked_project_owner_identity(project_root)?,
        &source_authority_digest,
        INCAN_VERSION,
    )
    .map_err(|error| CliError::failure(error.to_string()))?;
    // A cache candidate whose release Loaf the active toolchain no longer ships is a miss, not a fault: the
    // installed family changed underneath a still-valid local receipt (#1444), and an explicit bake exists to
    // refresh exactly that. Corrupt or mismatched authority still fails below, where the candidate is validated.
    if !project_authority_release_loafs_available(&authority)? {
        return Ok(None);
    }
    let _validated_authority = crate::lock::registry_sources::prepare_project_registry_source_authorities(authority)?;

    let mut generated_sources = BTreeMap::new();
    let mut profiles = Vec::new();
    for (project_target, output, receipt_path, _) in &selected_outputs {
        let generated_relative_path = match project_target {
            OvenBakeProjectTarget::Library => "generated/src/lib.rs",
            OvenBakeProjectTarget::Executable => "generated/src/main.rs",
        };
        let Some(generated) = output
            .payload
            .files
            .iter()
            .find(|file| file.output_relative_path == generated_relative_path)
        else {
            return Ok(None);
        };
        let generated_path = caller_project_output_path(project_root, &generated.caller_relative_path)?;
        match generated_sources.entry(output.payload.target_identity.clone()) {
            std::collections::btree_map::Entry::Vacant(entry) => {
                entry.insert(generated_path);
            }
            std::collections::btree_map::Entry::Occupied(entry) if entry.get() == &generated_path => {}
            std::collections::btree_map::Entry::Occupied(_) => return Ok(None),
        }
        profiles.push(OvenProjectBakeProfileReport {
            project_target: output.payload.target_identity.clone(),
            profile: output.profile.clone(),
            target: output.intent.target.clone(),
            toolchain: output.intent.toolchain.clone(),
            receipt: receipt_path.clone(),
            receipt_identity: output.payload.receipt_identity.clone(),
            build_unit_identity: output.payload.build_unit_identity.clone(),
            plan_identity: output.payload.plan_identity.clone(),
            action: "reused",
        });
    }
    let outputs = selected_outputs
        .iter()
        .map(|(_, output, _, _)| OvenProjectBakeOutputReport::from(output))
        .collect();
    let report = OvenProjectBakeReport {
        project: project_root.to_path_buf(),
        generated_sources,
        store: store.root().to_path_buf(),
        profiles,
        outputs,
    };
    let library_outputs = selected_outputs
        .iter()
        .filter_map(|(project_target, output, _, _)| {
            (*project_target == OvenBakeProjectTarget::Library).then_some(output)
        })
        .collect::<Vec<_>>();
    let library_current = library_outputs.iter().try_fold(true, |current, output| {
        Ok::<_, CliError>(project_output_projection_is_current(project_root, output)? && current)
    })?;
    // A verified warm hit leaves the artifact in place. Repairing a stale projection captures the whole prior
    // library before any profile is copied; a later handoff cache miss must roll back before starting a fresh bake.
    let publication = if library_current {
        None
    } else {
        Some(library_publication::LibraryPublication::begin(
            project_root,
            &project_root.join("target/lib"),
            library_publication_receipts(project_root)?,
        )?)
    };
    let result = (|| {
        if !library_current {
            for output in &library_outputs {
                materialize_project_output(project_root, output)?;
            }
        }
        if !restore_reused_library_package(project_root, store, &source_authority_digest, &library_outputs)? {
            return Ok(None);
        }
        for (project_target, output, _, _) in &selected_outputs {
            if *project_target == OvenBakeProjectTarget::Executable {
                materialize_project_output(project_root, output)?;
            }
        }
        Ok(Some(report))
    })();
    match publication {
        Some(publication) => publication.finish_reuse(result),
        None => result,
    }
}

impl OvenProjectBakeAuthorityContext {
    /// Validate requested provider profiles before spending a deep authored-tree scan, then memoize that one scan.
    pub fn checked_packaged_library_loaf_profiles(
        &mut self,
        artifact: &LibraryArtifactMetadata,
        profiles: &[&str],
        target: &str,
        toolchain: &str,
    ) -> CliResult<Option<Vec<OvenPackagedLibraryLoafProfile>>> {
        let canonical_artifact_root = fs::canonicalize(&artifact.crate_root).map_err(|error| {
            CliError::failure(format!(
                "Oven Alpha cannot resolve package artifact root for pub::{} at {}: {error}",
                artifact.dependency_key,
                artifact.crate_root.display()
            ))
        })?;
        let Some((manifest_path, manifest_digest, decoded_manifest)) = decode_packaged_library_loaf_manifest(artifact)?
        else {
            if self.providers.contains_key(&canonical_artifact_root) {
                return Err(CliError::failure(format!(
                    "Oven Alpha refuses pub::{} because its package Loaf manifest disappeared during this explicit bake",
                    artifact.dependency_key
                )));
            }
            return Ok(None);
        };

        let (manifest, source_project_root, source_authority_verified) = if let Some(memoized) =
            self.providers.get(&canonical_artifact_root)
        {
            if memoized.manifest_path != manifest_path || memoized.manifest_digest != manifest_digest {
                return Err(CliError::failure(format!(
                    "Oven Alpha refuses pub::{} because its package Loaf manifest changed during this explicit bake",
                    artifact.dependency_key
                )));
            }
            (
                Arc::clone(&memoized.manifest),
                memoized.source_project_root.clone(),
                memoized.source_authority_verified,
            )
        } else {
            let source_project_root = artifact
                .crate_root
                .parent()
                .and_then(Path::parent)
                .filter(|root| root.join(LOAF_MANIFEST_FILENAME).is_file())
                .map(|root| {
                    fs::canonicalize(root).map_err(|error| {
                        CliError::failure(format!(
                            "Oven Alpha cannot resolve source project for pub::{} at {}: {error}",
                            artifact.dependency_key,
                            root.display()
                        ))
                    })
                })
                .transpose()?;
            let manifest = Arc::new(decoded_manifest);
            self.providers.insert(
                canonical_artifact_root.clone(),
                MemoizedPackagedProviderAuthority {
                    artifact: artifact.clone(),
                    manifest_path,
                    manifest_digest,
                    manifest: Arc::clone(&manifest),
                    source_project_root: source_project_root.clone(),
                    source_authority_verified: false,
                    admitted_profiles: BTreeMap::new(),
                },
            );
            (manifest, source_project_root, false)
        };

        // Profile receipts, target/toolchain intent, entry receipts, and the native output digest are all cheaper than
        // recursively hashing a provider source tree. A missing release handoff therefore fails before the first deep
        // scan even when debug was the first target/profile requested by the caller.
        let mut selected = Vec::with_capacity(profiles.len());
        for profile in profiles {
            let Some(candidate) =
                validated_packaged_library_loaf_profile(artifact, &manifest, profile, target, toolchain)?
            else {
                return Ok(None);
            };
            selected.push(candidate);
        }
        validate_packaged_library_metadata_files(artifact, &manifest)?;

        if !source_authority_verified {
            if let Some(source_project_root) = source_project_root.as_deref() {
                let actual = self.source_digester.digest(source_project_root)?;
                if actual != manifest.source_authority_digest {
                    return Err(CliError::failure(format!(
                        "Oven Alpha refuses pub::{} because its source project at {} changed after the package Loaf was baked; rebake that provider before baking or running a consumer",
                        artifact.dependency_key,
                        source_project_root.display()
                    )));
                }
            }
            let memoized = self.providers.get_mut(&canonical_artifact_root).ok_or_else(|| {
                CliError::failure(format!(
                    "Oven Alpha lost the command-local provider authority for pub::{} at {} before source validation completed",
                    artifact.dependency_key,
                    canonical_artifact_root.display()
                ))
            })?;
            memoized.source_authority_verified = true;
        }
        let memoized = self.providers.get_mut(&canonical_artifact_root).ok_or_else(|| {
            CliError::failure(format!(
                "Oven Alpha lost the command-local provider authority for pub::{} at {} before profile admission completed",
                artifact.dependency_key,
                canonical_artifact_root.display()
            ))
        })?;
        for profile in profiles {
            memoized
                .admitted_profiles
                .insert((*profile).to_string(), (target.to_string(), toolchain.to_string()));
        }
        Ok(Some(selected))
    }

    /// Scan a tentative cache candidate without committing its current lock projection as publication authority.
    ///
    /// A cache miss must leave this context unbound. In particular, an old completed-output receipt can require an
    /// authority scan before the explicit baker refreshes a legacy lock; binding that old projection would make the
    /// final publication check reject the baker's own lock refresh.
    pub fn cache_probe_source_authority(&self, project_root: &Path) -> CliResult<String> {
        digest_baked_project_source_authority(project_root)
    }

    /// Forget memoized project-tree digests after this bake published the canonical lock.
    ///
    /// The root authority is first bound right after that publication, from the same digester that already scanned
    /// this project's providers while preparing them. In a workspace the published lock is a root file that every
    /// member's build inputs include, so a provider digest memoized before the write is stale, and binding it would
    /// make final publication reject the lock this bake just wrote (#1414).
    pub fn lock_published(&mut self) {
        self.source_digester.forget_project_tree_digests();
    }

    /// Return the memoized root authority used while preparing this command's targets.
    pub fn project_source_authority(&mut self, project_root: &Path) -> CliResult<String> {
        let digest = self.source_digester.digest(project_root)?;
        match self.initial_project_source_authority.as_deref() {
            Some(initial) if initial != digest => Err(CliError::failure(
                "explicit Oven project source authority changed during command-local preparation",
            )),
            Some(_) => Ok(digest),
            None => {
                self.initial_project_source_authority = Some(digest.clone());
                Ok(digest)
            }
        }
    }

    /// Recheck cheap mutable provider facts, then perform the fresh deep scan that gates final publication.
    pub fn final_project_source_authority(&self, project_root: &Path) -> CliResult<String> {
        for memoized in self.providers.values() {
            let Some((manifest_path, manifest_digest, _decoded_manifest)) =
                decode_packaged_library_loaf_manifest(&memoized.artifact)?
            else {
                return Err(CliError::failure(format!(
                    "Oven Alpha refuses pub::{} because its package Loaf manifest disappeared before final publication",
                    memoized.artifact.dependency_key
                )));
            };
            if manifest_path != memoized.manifest_path || manifest_digest != memoized.manifest_digest {
                return Err(CliError::failure(format!(
                    "Oven Alpha refuses pub::{} because its package Loaf manifest changed before final publication",
                    memoized.artifact.dependency_key
                )));
            }
            for (profile, (target, toolchain)) in &memoized.admitted_profiles {
                if validated_packaged_library_loaf_profile(
                    &memoized.artifact,
                    &memoized.manifest,
                    profile,
                    target,
                    toolchain,
                )?
                .is_none()
                {
                    return Err(CliError::failure(format!(
                        "Oven Alpha refuses pub::{} because its `{profile}` package Loaf became incompatible before final publication",
                        memoized.artifact.dependency_key
                    )));
                }
            }
            validate_packaged_library_metadata_files(&memoized.artifact, &memoized.manifest)?;
        }

        let mut fresh = ProjectSourceAuthorityDigester::default();
        let final_project_authority = fresh.digest(project_root)?;
        if self
            .initial_project_source_authority
            .as_deref()
            .is_some_and(|initial| initial != final_project_authority)
        {
            return Err(CliError::failure(
                "Oven Alpha refuses to publish this explicit bake because the project source authority changed during preparation",
            ));
        }
        for memoized in self.providers.values() {
            let Some(source_project_root) = memoized.source_project_root.as_deref() else {
                continue;
            };
            let actual = fresh.digest(source_project_root)?;
            if actual != memoized.manifest.source_authority_digest {
                return Err(CliError::failure(format!(
                    "Oven Alpha refuses pub::{} because its source project at {} changed before final publication; rebake that provider before baking this consumer",
                    memoized.artifact.dependency_key,
                    source_project_root.display()
                )));
            }
        }
        Ok(final_project_authority)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oven_rustc::rustc::OvenProjectInspectionConstituent;
    use oven_store::store::OvenArtifactKind;

    /// A release Loaf the active toolchain does not provide reads as unavailable, while an authority made only of
    /// stored outputs has no release Loaf to be unavailable in the first place.
    #[test]
    fn an_unshipped_release_loaf_is_a_cache_miss_not_a_fault_issue1444() -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        oven_store::test_support::write_project(project.path())?;
        let receipt = oven_store::test_support::request(project.path(), "owner", b"payload")?.receipt;
        let unshipped = OvenProjectInspectionConstituent::ReleaseLoaf {
            loaf_identity: format!("sha256:{}", "0".repeat(64)),
            build_unit_identity: receipt.build_unit_identity.clone(),
            receipt: receipt.clone(),
        };
        assert!(
            !release_loaf_constituents_available(std::slice::from_ref(&unshipped))?,
            "a release Loaf the active toolchain does not provide is a miss"
        );
        let stored_only = OvenProjectInspectionConstituent::Stored {
            identity: "sha256:stored".to_string(),
            artifact_kind: OvenArtifactKind::ProjectOutput,
            receipt,
            base_loaf_identity: None,
        };
        assert!(
            release_loaf_constituents_available(std::slice::from_ref(&stored_only))?,
            "an authority without release Loafs has nothing to be unavailable"
        );
        Ok(())
    }
}
