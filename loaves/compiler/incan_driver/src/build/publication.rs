//! Publishing a project's completed output Loaf into the store.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::build::output_paths::{project_relative_entrypoint, validated_project_output_relative_path};
use crate::build::output_selection::baked_project_owner_identity;
use crate::build::{
    OVEN_PROJECT_OUTPUT_ARTIFACT_PATH, OVEN_PROJECT_OUTPUT_PAYLOAD_SCHEMA_VERSION, OVEN_PROJECT_OUTPUT_PUBLICATION_RETRY,
    OVEN_PROJECT_OUTPUT_PUBLICATION_WAIT, OVEN_PROJECT_OUTPUT_REPORT_SCHEMA_VERSION, OvenProjectOutputBakeFile,
    OvenProjectOutputBakeRequest, OvenProjectOutputFile, OvenProjectOutputPayload, OvenStoredProjectOutput,
    PublishedProjectInspectionAuthority, oven_bake_project_target_identity,
};
use crate::error::{CliError, CliResult, oven_rustc_error};
use incan_lang::version::INCAN_VERSION;
use oven_model::manifest::DependencySpec;
use oven_rustc::loaf::resolve_compiler_owned_loaf_by_identity;
use oven_rustc::plan::OvenDirectRustcPlanSelection;
use oven_rustc::plan::selection::{
    select_packaged_direct_rustc_execution_plan, select_receipt_project_extension_execution_plan,
};
use oven_rustc::rustc::{
    OVEN_PROJECT_INSPECTION_AUTHORITY_SCHEMA_VERSION, OVEN_RUSTC_REGISTRY_LOCK_RELATIVE_PATH,
    OvenProjectInspectionAuthorityPayload, OvenProjectInspectionAuthorityRef, OvenProjectInspectionConstituent,
    OvenProjectInspectionGeneratedOutDir, OvenProjectInspectionSource, OvenProjectInspectionSourceOwner,
    OvenProjectInspectionTestDependencyEnvelope, OvenProjectInspectionTestProviderConstituent,
    validate_project_inspection_authority_payload,
};
use oven_store::digest_bytes;
use oven_store::store::{
    OvenArtifactKind, OvenArtifactMaterializedFile, OvenArtifactPublishRequest, OvenStore, OvenStoreError,
    OvenStoreLease,
};

/// Publish one completed project-native output at the explicit project bake boundary.
///
/// The project-output Loaf shares the release compatibility domain with its selected direct-rustc inputs, so ordinary
/// bounded retention can reclaim an inactive completed output without creating a separate unbounded cache class.
pub fn publish_project_output_loaf(
    store: &OvenStore,
    receipt: &oven_store::OvenReceipt,
    payload: &OvenProjectOutputPayload,
    files: &[OvenProjectOutputBakeFile],
) -> CliResult<OvenStoredProjectOutput> {
    receipt
        .verify_identity()
        .map_err(|error| CliError::failure(format!("cannot publish invalid Oven project-output receipt: {error}")))?;
    if payload.schema_version != OVEN_PROJECT_OUTPUT_PAYLOAD_SCHEMA_VERSION
        || payload.compiler_version != INCAN_VERSION
        || payload.target_identity.trim().is_empty()
        || payload.receipt_identity != receipt.identity
        || payload.build_unit_identity != receipt.build_unit_identity
        || payload.plan_identity.trim().is_empty()
        || (payload.inspection_authority.is_none() && payload.preparation_identity_digest.is_none())
        || payload.files.is_empty()
        || payload
            .build_report
            .as_ref()
            .is_some_and(|snapshot| snapshot.schema_version != OVEN_PROJECT_OUTPUT_REPORT_SCHEMA_VERSION)
    {
        return Err(CliError::failure(
            "cannot publish project-output Loaf with inconsistent receipt or output authority",
        ));
    }
    let sources = files
        .iter()
        .map(|file| (file.output_relative_path.as_str(), file))
        .collect::<BTreeMap<_, _>>();
    if sources.len() != files.len() || sources.len() != payload.files.len() {
        return Err(CliError::failure(
            "cannot publish Oven project-output Loaf with duplicate or missing output files",
        ));
    }
    for file in &payload.files {
        let Some(source) = sources.get(file.output_relative_path.as_str()) else {
            return Err(CliError::failure(
                "cannot publish Oven project-output Loaf whose payload omits a source file",
            ));
        };
        let (logical_bytes, digest) =
            super::output_materialization::digest_project_output_projection_file(&source.source_path)?;
        if source.caller_relative_path != file.caller_relative_path
            || logical_bytes != file.logical_bytes
            || digest != file.digest
        {
            return Err(CliError::failure(
                "cannot publish Oven project output whose source differs from its payload",
            ));
        }
    }
    let payload_bytes = serde_json::to_vec(payload)
        .map_err(|error| CliError::failure(format!("failed to serialize Oven project-output Loaf: {error}")))?;
    let mut request = OvenArtifactPublishRequest {
        receipt: receipt.clone(),
        domain: format!("incan-release-{INCAN_VERSION}"),
        kind: OvenArtifactKind::ProjectOutput,
        payload: payload_bytes,
        materialized_files: files
            .iter()
            .map(|file| OvenArtifactMaterializedFile {
                source_path: file.source_path.clone(),
                relative_path: file.output_relative_path.clone(),
            })
            .collect(),
        materialized_directories: Vec::new(),
    };
    let _inspection_owner = super::inspection_snapshot::retain_inspection_snapshot(store, payload, &mut request)
        .map_err(oven_rustc_error)?;
    let deadline = Instant::now() + OVEN_PROJECT_OUTPUT_PUBLICATION_WAIT;
    let manifest = loop {
        match store.publish(&request) {
            Ok(manifest) => break manifest,
            Err(OvenStoreError::LegacyPublisherStagingActive { .. }) if Instant::now() < deadline => {
                // The active named publisher owns the remaining physical staging capacity. Waiting preserves that hard
                // bound while allowing independent explicit project bakes to converge safely.
                std::thread::sleep(OVEN_PROJECT_OUTPUT_PUBLICATION_RETRY);
            }
            Err(error) => {
                return Err(CliError::failure(format!(
                    "failed to publish Oven project-output Loaf: {error}"
                )));
            }
        }
    };
    let selected = store
        .select_payload_for_execution(&manifest.identity)
        .map_err(|error| {
            CliError::failure(format!(
                "failed to acquire the newly published Oven project-output lease: {error}"
            ))
        })?;
    if selected.0 != manifest {
        return Err(CliError::failure(
            "published Oven project output changed its immutable manifest during exact lease acquisition",
        ));
    }
    let selected_payload = serde_json::from_slice::<OvenProjectOutputPayload>(&selected.2).map_err(|error| {
        CliError::failure(format!(
            "newly published Oven project output has an invalid payload: {error}"
        ))
    })?;
    if &selected_payload != payload {
        return Err(CliError::failure(
            "newly published Oven project output differs from the completed target payload",
        ));
    }
    stored_project_output_from_parts(selected.0, selected.1, selected_payload, selected.3)
}

/// Validate one already leased project-output payload without rehashing its immutable artifact bytes.
pub fn stored_project_output_from_parts(
    manifest: oven_store::store::OvenArtifactManifest,
    artifact_root: PathBuf,
    payload: OvenProjectOutputPayload,
    lease: OvenStoreLease,
) -> CliResult<OvenStoredProjectOutput> {
    if manifest.kind != OvenArtifactKind::ProjectOutput
        || payload.schema_version != OVEN_PROJECT_OUTPUT_PAYLOAD_SCHEMA_VERSION
        || payload.compiler_version != INCAN_VERSION
        || payload.target_identity.trim().is_empty()
        || payload.receipt_identity != manifest.receipt_identity
        || payload.build_unit_identity != manifest.build_unit_identity
        || payload.plan_identity.trim().is_empty()
        || payload.files.is_empty()
        || payload
            .build_report
            .as_ref()
            .is_some_and(|snapshot| snapshot.schema_version != OVEN_PROJECT_OUTPUT_REPORT_SCHEMA_VERSION)
    {
        return Err(CliError::failure(format!(
            "selected Oven project-output Loaf `{}` has inconsistent schema, receipt, compiler, or output authority",
            manifest.identity
        )));
    }
    match payload.inspection_authority.as_ref() {
        Some(authority)
            if authority.identity.trim().is_empty()
                || authority.receipt_identity.trim().is_empty()
                || authority.build_unit_identity.trim().is_empty() =>
        {
            return Err(CliError::failure(format!(
                "selected Oven project-output Loaf `{}` has an incomplete project inspection authority",
                manifest.identity
            )));
        }
        None if payload
            .preparation_identity_digest
            .as_ref()
            .is_none_or(|identity| identity.trim().is_empty()) =>
        {
            return Err(CliError::failure(format!(
                "selected Oven project-output Loaf `{}` has neither inspection nor ordinary preparation authority",
                manifest.identity
            )));
        }
        _ => {}
    }
    let native_outputs = payload
        .files
        .iter()
        .filter(|file| file.output_relative_path == OVEN_PROJECT_OUTPUT_ARTIFACT_PATH)
        .collect::<Vec<_>>();
    if native_outputs.len() != 1 {
        return Err(CliError::failure(format!(
            "selected Oven project-output Loaf `{}` must contain exactly one native output",
            manifest.identity
        )));
    }
    let mut stored_paths = BTreeSet::new();
    let mut caller_paths = BTreeSet::new();
    for file in &payload.files {
        let _ = validated_project_output_relative_path(&file.output_relative_path, "stored output")?;
        let _ = validated_project_output_relative_path(&file.caller_relative_path, "caller output")?;
        if !stored_paths.insert(file.output_relative_path.as_str())
            || !caller_paths.insert(file.caller_relative_path.as_str())
        {
            return Err(CliError::failure(
                "selected Oven project-output Loaf contains duplicate output paths",
            ));
        }
    }
    if let Some(package_loaf_store_relative_path) = payload.package_loaf_store_relative_path.as_deref() {
        let _ = validated_project_output_relative_path(package_loaf_store_relative_path, "package Loaf store")?;
    } else if !payload.required_project_loafs.is_empty() {
        return Err(CliError::failure(
            "selected executable Oven project-output Loaf unexpectedly carries library package Loafs",
        ));
    }
    let native = native_outputs[0];
    let native_output = artifact_root.join(validated_project_output_relative_path(
        &native.output_relative_path,
        "stored output",
    )?);
    let metadata = fs::metadata(&native_output).map_err(|error| {
        CliError::failure(format!(
            "selected Oven project-output Loaf is missing its sealed native output {}: {error}",
            native_output.display()
        ))
    })?;
    if !metadata.is_file() || metadata.len() != native.logical_bytes {
        return Err(CliError::failure(format!(
            "selected Oven project-output Loaf native output length differs at {}",
            native_output.display()
        )));
    }
    let profile = manifest.intent.profile.clone();
    let intent = manifest.intent.clone();
    Ok(OvenStoredProjectOutput {
        identity: manifest.identity,
        profile,
        intent,
        payload,
        artifact_root,
        native_output,
        _lease: lease,
    })
}

/// Derive the completed-output authority from the project state that the explicit baker has already validated and
/// compiled.
pub fn project_output_payload_for_bake(
    request: OvenProjectOutputBakeRequest<'_>,
) -> CliResult<OvenProjectOutputPayload> {
    request.backend_receipt.verify_identity().map_err(|error| {
        CliError::failure(format!(
            "completed Oven project output has an invalid backend receipt: {error}"
        ))
    })?;
    let entrypoint_relative_path =
        project_relative_entrypoint(request.project_root, request.entrypoint).ok_or_else(|| {
            CliError::failure(format!(
                "explicit Oven project bake entrypoint {} is outside its project root {}",
                request.entrypoint.display(),
                request.project_root.display()
            ))
        })?;
    let target_identity = oven_bake_project_target_identity(request.project_root, request.target, request.entrypoint)?;
    if request.receipt.intent.profile != request.profile {
        return Err(CliError::failure(
            "explicit Oven project bake profile differs from its prepared receipt",
        ));
    }
    let mut files = request
        .files
        .into_iter()
        .map(|file| {
            let (logical_bytes, digest) =
                super::output_materialization::digest_project_output_projection_file(&file.source_path)?;
            Ok(OvenProjectOutputFile {
                caller_relative_path: file.caller_relative_path,
                output_relative_path: file.output_relative_path,
                digest,
                logical_bytes,
            })
        })
        .collect::<CliResult<Vec<_>>>()?;
    files.sort_by(|left, right| left.output_relative_path.cmp(&right.output_relative_path));
    let native_outputs = files
        .iter()
        .filter(|file| file.output_relative_path == OVEN_PROJECT_OUTPUT_ARTIFACT_PATH)
        .count();
    if native_outputs != 1 {
        return Err(CliError::failure(
            "completed Oven project-output Loaf must contain exactly one native output",
        ));
    }
    if request.inspection_authority.as_ref().is_some_and(|authority| {
        authority.identity.trim().is_empty()
            || authority.receipt_identity.trim().is_empty()
            || authority.build_unit_identity.trim().is_empty()
    }) {
        return Err(CliError::failure(
            "completed Oven project output must name one exact project inspection authority",
        ));
    }
    let mut required_project_loafs = request.required_project_loafs;
    required_project_loafs.sort_by(|left, right| {
        (&left.identity, &left.receipt.identity).cmp(&(&right.identity, &right.receipt.identity))
    });
    required_project_loafs.dedup_by(|left, right| {
        left.identity == right.identity && left.receipt.identity == right.receipt.identity && left.kind == right.kind
    });
    if request.package_loaf_store_relative_path.is_none() && !required_project_loafs.is_empty() {
        return Err(CliError::failure(
            "completed executable Oven project output cannot carry library package Loafs",
        ));
    }
    if let Some(path) = request.package_loaf_store_relative_path.as_deref() {
        let _ = validated_project_output_relative_path(path, "package Loaf store")?;
    }
    if request
        .lock_dependencies_fingerprint
        .as_deref()
        .is_some_and(|fingerprint| fingerprint.trim().is_empty())
    {
        return Err(CliError::failure(
            "completed Oven project output cannot carry an empty lock dependency fingerprint",
        ));
    }
    if request.build_report.as_ref().is_some_and(|snapshot| {
        snapshot.schema_version != OVEN_PROJECT_OUTPUT_REPORT_SCHEMA_VERSION || !snapshot.report.is_object()
    }) {
        return Err(CliError::failure(
            "completed Oven project output cannot carry an invalid build-report snapshot",
        ));
    }
    Ok(OvenProjectOutputPayload {
        schema_version: OVEN_PROJECT_OUTPUT_PAYLOAD_SCHEMA_VERSION,
        project_target: request.target.as_str().to_string(),
        target_identity,
        project_identity: baked_project_owner_identity(request.project_root)?,
        source_authority_digest: request.source_authority_digest.to_string(),
        dependency_authority_digest: Some(super::source_authority::digest_project_development_dependencies(
            request.project_root,
        )?),
        lock_dependencies_fingerprint: request.lock_dependencies_fingerprint,
        compiler_identity_digest: Some(super::source_authority::current_compiler_identity_digest()?),
        preparation_identity_digest: None,
        compiler_version: INCAN_VERSION.to_string(),
        entrypoint_relative_path,
        build_unit_identity: request.receipt.build_unit_identity.clone(),
        receipt_identity: request.receipt.identity.clone(),
        plan_identity: request.plan_identity,
        backend_receipt: request.backend_receipt,
        inspection_authority: request.inspection_authority,
        inspection_authority_snapshot: None,
        files,
        required_project_loafs,
        package_loaf_store_relative_path: request.package_loaf_store_relative_path,
        build_report: request.build_report,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build::test_support::fixture_project_output_publication;
    use std::collections::BTreeSet;
    use std::fs;

    use oven_store::store::OvenStore;

    /// Publication must reject changed bytes and inconsistent lengths even after payload generation warmed the
    /// observer.
    #[test]
    fn publication_rechecks_changed_sources_and_payload_lengths() -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        fs::create_dir(project.path().join("src"))?;
        fs::write(project.path().join("loaf.toml"), "[project]\nname='fixture'\n")?;
        fs::write(project.path().join("src/main.incn"), "def main() -> None:\n    pass\n")?;
        let (receipt, payload, files) = fixture_project_output_publication(project.path(), "debug", "observed")?;
        let source = &files.first().ok_or("fixture output missing")?.source_path;
        let original = fs::read(source)?;
        let modified = fs::metadata(source)?.modified()?;
        let mut changed = original.clone();
        let first = changed.first_mut().ok_or("fixture output is empty")?;
        *first ^= 1;
        fs::write(source, changed)?;
        fs::File::options()
            .write(true)
            .open(source)?
            .set_times(fs::FileTimes::new().set_modified(modified))?;
        let root = tempfile::tempdir()?;
        let store = OvenStore::new(
            root.path(),
            oven_store::store::OvenStoreLimits::new(1024 * 1024, 1024 * 1024, 1024 * 1024),
        );
        assert!(publish_project_output_loaf(&store, &receipt, &payload, &files).is_err());
        fs::write(source, original)?;
        let mut wrong_length = payload.clone();
        wrong_length
            .files
            .first_mut()
            .ok_or("fixture payload missing")?
            .logical_bytes += 1;
        assert!(publish_project_output_loaf(&store, &receipt, &wrong_length, &files).is_err());
        let selected = publish_project_output_loaf(&store, &receipt, &payload, &files)?;
        assert_eq!(selected.payload.files, payload.files);
        Ok(())
    }

    /// Store budgets sweep idle entries; bounded publication preserves every leased project sibling above the target.
    #[test]
    fn project_output_publication_preserves_leased_siblings_above_retention_target()
    -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        fs::create_dir(project.path().join("src"))?;
        fs::write(project.path().join("loaf.toml"), "[project]\nname = \"fixture\"\n")?;
        fs::write(project.path().join("src/main.incn"), "def main() -> None:\n    pass\n")?;
        let debug = fixture_project_output_publication(project.path(), "debug", "debug")?;
        let release = fixture_project_output_publication(project.path(), "release", "release")?;

        let prototype_root = tempfile::tempdir()?;
        let prototype = OvenStore::new(
            prototype_root.path(),
            oven_store::store::OvenStoreLimits::new(1024 * 1024, 1024 * 1024, 1024 * 1024),
        );
        let prototype_debug = publish_project_output_loaf(&prototype, &debug.0, &debug.1, &debug.2)?;
        let prototype_release = publish_project_output_loaf(&prototype, &release.0, &release.1, &release.2)?;
        let measured = prototype.inspect()?;
        drop((prototype_debug, prototype_release));

        let tight_root = tempfile::tempdir()?;
        let tight = OvenStore::new(
            tight_root.path(),
            oven_store::store::OvenStoreLimits::new(
                measured.physical_bytes,
                measured.physical_bytes,
                measured.logical_bytes,
            ),
        );
        let retained_debug = publish_project_output_loaf(&tight, &debug.0, &debug.1, &debug.2)?;
        let retained_release = publish_project_output_loaf(&tight, &release.0, &release.1, &release.2)?;
        let complete = tight.inspect()?;
        assert_eq!(complete.entries.len(), 2);
        assert_eq!(complete.active_lease_physical_bytes, complete.physical_bytes);
        assert_eq!(complete.reclaimable_physical_bytes, 0);

        let overflow = fixture_project_output_publication(project.path(), "debug", "overflow")?;
        let retained_overflow = publish_project_output_loaf(&tight, &overflow.0, &overflow.1, &overflow.2)?;
        let after = tight.inspect()?;
        assert_eq!(after.entries.len(), 3);
        assert_eq!(after.active_lease_physical_bytes, after.physical_bytes);
        assert_eq!(after.reclaimable_physical_bytes, 0);
        let identities = after
            .entries
            .iter()
            .map(|entry| entry.manifest.identity.as_str())
            .collect::<BTreeSet<_>>();
        assert!(identities.contains(retained_debug.identity.as_str()));
        assert!(identities.contains(retained_release.identity.as_str()));
        assert!(identities.contains(retained_overflow.identity.as_str()));
        Ok(())
    }
}
