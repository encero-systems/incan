//! Retain inspection metadata with the completed loaf that requires it, without retaining a second native closure.

use std::path::Path;

use oven_rustc::rustc::{
    OvenLoadedProjectInspectionAuthority, OvenProjectInspectionAuthorityPayload, OvenRustcError,
    load_project_inspection_authority, validate_project_inspection_authority_payload,
};
use oven_store::store::{
    OvenArtifactKind, OvenArtifactMaterializedDirectory, OvenArtifactMaterializedFile, OvenArtifactPublishRequest,
    OvenStore, OvenStoreError, OvenStoreExecutionPayload,
};

use super::{OvenProjectOutputPayload, OvenStoredProjectOutput};

const SNAPSHOT_PREFIX: &str = "inspection-authority";

/// Reject inconsistent snapshots before any retained file is read or Store publication is attempted.
fn snapshot_request(
    store: &OvenStore,
    output: &OvenProjectOutputPayload,
    root: &Path,
) -> Result<OvenArtifactPublishRequest, OvenRustcError> {
    let snapshot = output
        .inspection_authority_snapshot
        .as_ref()
        .ok_or_else(|| invalid_snapshot(output, "completed output has no retained inspection metadata"))?;
    let reference = output
        .inspection_authority
        .as_ref()
        .ok_or_else(|| invalid_snapshot(output, "completed output has no inspection authority reference"))?;
    let manifest = &snapshot.manifest;
    if manifest.identity != reference.identity
        || manifest.receipt_identity != reference.receipt_identity
        || manifest.build_unit_identity != reference.build_unit_identity
        || manifest.kind != OvenArtifactKind::ProjectInspectionAuthority
    {
        return Err(invalid_snapshot(
            output,
            "retained inspection manifest differs from the exact output reference",
        ));
    }
    let payload: OvenProjectInspectionAuthorityPayload = serde_json::from_str(&snapshot.payload)
        .map_err(|error| invalid_snapshot(output, &format!("invalid retained inspection metadata: {error}")))?;
    validate_project_inspection_authority_payload(&payload)?;
    if payload.project_identity != output.project_identity
        || payload.source_authority_digest != output.source_authority_digest
        || payload.compiler_version != output.compiler_version
    {
        return Err(invalid_snapshot(
            output,
            "retained inspection metadata differs from the output's source authority",
        ));
    }
    let checked_path = |relative: &str| {
        super::output_paths::validated_project_output_relative_path(relative, "retained inspection member")
            .map(|path| root.join(path))
            .map_err(|error| invalid_snapshot(output, &error.to_string()))
    };
    let request = OvenArtifactPublishRequest {
        receipt: snapshot.receipt.clone(),
        domain: manifest.domain.clone(),
        kind: manifest.kind,
        payload: snapshot.payload.as_bytes().to_vec(),
        materialized_files: manifest
            .materialized_files
            .iter()
            .map(|file| {
                Ok(OvenArtifactMaterializedFile {
                    source_path: checked_path(&file.relative_path)?,
                    relative_path: file.relative_path.clone(),
                })
            })
            .collect::<Result<_, OvenRustcError>>()?,
        materialized_directories: manifest
            .materialized_directories
            .iter()
            .map(|directory| {
                Ok(OvenArtifactMaterializedDirectory {
                    source_path: checked_path(&directory.relative_path)?,
                    relative_path: directory.relative_path.clone(),
                })
            })
            .collect::<Result<_, OvenRustcError>>()?,
    };
    if store.manifest_for_publication(&request)? != *manifest {
        return Err(invalid_snapshot(
            output,
            "retained inspection bytes do not reproduce the original manifest",
        ));
    }
    Ok(request)
}

/// Keep the exact authority owner leased while its metadata files join a completed output publication.
pub(super) fn retain_inspection_snapshot(
    store: &OvenStore,
    output: &OvenProjectOutputPayload,
    publication: &mut OvenArtifactPublishRequest,
) -> Result<Option<OvenStoreExecutionPayload>, OvenRustcError> {
    let Some(snapshot) = &output.inspection_authority_snapshot else {
        return Ok(None);
    };
    if output
        .files
        .iter()
        .any(|file| Path::new(&file.output_relative_path).starts_with(SNAPSHOT_PREFIX))
    {
        return Err(invalid_snapshot(
            output,
            "caller output overlaps the internal inspection metadata prefix",
        ));
    }
    let mut owners = store.select_payloads_for_execution(std::slice::from_ref(&snapshot.manifest.identity))?;
    let owner = owners
        .pop()
        .ok_or_else(|| invalid_snapshot(output, "inspection owner selection returned no entry"))?;
    if owner.manifest() != &snapshot.manifest || owner.payload() != snapshot.payload.as_bytes() {
        return Err(invalid_snapshot(
            output,
            "retained metadata differs from the selected inspection owner",
        ));
    }
    let request = snapshot_request(store, output, owner.artifact_root())?;
    publication
        .materialized_files
        .extend(request.materialized_files.into_iter().map(|mut file| {
            file.relative_path = format!("{SNAPSHOT_PREFIX}/{}", file.relative_path);
            file
        }));
    publication
        .materialized_directories
        .extend(request.materialized_directories.into_iter().map(|mut directory| {
            directory.relative_path = format!("{SNAPSHOT_PREFIX}/{}", directory.relative_path);
            directory
        }));
    Ok(Some(owner))
}

/// Load exact inspection authority, restoring only an absent whole metadata owner from its retained output loaf.
///
/// Existing damaged entries and missing constituents retain the canonical loader's failure. Restoration reproduces
/// the original manifest before admission and then runs that loader again, including every constituent check.
#[track_caller]
pub(super) fn load_output_inspection_authority(
    store: &OvenStore,
    output: &OvenStoredProjectOutput,
) -> Result<OvenLoadedProjectInspectionAuthority, OvenRustcError> {
    let caller = std::panic::Location::caller();
    let _invocation = tracing::trace_span!(target: "incan::invocation", "incan_driver::load_output_inspection_authority", input = %output.identity, caller_file = caller.file(), caller_line = u64::from(caller.line())).entered();
    let payload = &output.payload;
    let reference = payload
        .inspection_authority
        .as_ref()
        .ok_or_else(|| invalid_snapshot(payload, "completed output has no inspection authority reference"))?;
    let load = || {
        load_project_inspection_authority(
            store,
            reference,
            &payload.project_identity,
            &payload.source_authority_digest,
            &payload.compiler_version,
        )
    };
    let error = match load() {
        Ok(authority) => return Ok(authority),
        Err(error) => error,
    };
    if payload.inspection_authority_snapshot.is_none()
        || !matches!(&error, OvenRustcError::Store(error) if whole_entry_missing(store, error))
    {
        return Err(error);
    }
    // A missing constituent can produce the same error class. Confirm that the exact metadata owner itself is absent.
    match store.select_payloads_for_execution(std::slice::from_ref(&reference.identity)) {
        Err(error) if whole_entry_missing(store, &error) => {}
        _ => return Err(error),
    }
    let request = snapshot_request(store, payload, &output.artifact_root.join(SNAPSHOT_PREFIX))?;
    let snapshot = payload
        .inspection_authority_snapshot
        .as_ref()
        .ok_or_else(|| invalid_snapshot(payload, "completed output lost its retained inspection metadata"))?;
    let restored = store.publish_verified_import(
        &request,
        &snapshot.manifest.materialized_files,
        &snapshot.manifest.materialized_directories,
    )?;
    if restored != snapshot.manifest {
        return Err(invalid_snapshot(
            payload,
            "restored inspection authority changed its exact manifest",
        ));
    }
    load()
}

/// A missing file inside an existing entry is damage, not an evicted metadata owner.
fn whole_entry_missing(store: &OvenStore, error: &OvenStoreError) -> bool {
    matches!(error, OvenStoreError::Io { path, source }
        if source.kind() == std::io::ErrorKind::NotFound
            && path.parent() == Some(store.root().join("entries").as_path()))
}

/// Report snapshot failures against the completed output's original receipt lineage.
fn invalid_snapshot(output: &OvenProjectOutputPayload, message: &str) -> OvenRustcError {
    OvenRustcError::PlanSelection {
        receipt_identity: output.receipt_identity.clone(),
        message: message.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build::OvenProjectInspectionSnapshot;
    use crate::build::publication::publish_project_output_loaf;
    use crate::build::test_support::fixture_project_output_publication;
    use oven_rustc::rustc::{
        OVEN_PROJECT_INSPECTION_AUTHORITY_SCHEMA_VERSION, OvenProjectInspectionAuthorityRef,
        OvenProjectInspectionConstituent,
    };
    use oven_store::digest_bytes;
    use oven_store::store::OvenStoreLimits;
    use std::fs;

    type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

    /// Locate the private fixture's selected metadata owner without depending on a test-only dependency API.
    fn authority_entry(store: &OvenStore, output: &OvenStoredProjectOutput) -> TestResult<std::path::PathBuf> {
        let reference = output
            .payload
            .inspection_authority
            .as_ref()
            .ok_or("missing authority")?;
        let (_, root, _, _lease) = store.select_payload_for_execution(&reference.identity)?;
        Ok(root
            .parent()
            .ok_or("inspection artifact root has no entry parent")?
            .to_path_buf())
    }

    /// Publish a real completed loaf and a small independent metadata owner in a private Store.
    fn fixture(
        missing_constituent: bool,
    ) -> TestResult<(tempfile::TempDir, tempfile::TempDir, OvenStore, OvenStoredProjectOutput)> {
        let project = tempfile::tempdir()?;
        fs::create_dir(project.path().join("src"))?;
        fs::write(project.path().join("loaf.toml"), "[project]\nname = \"fixture\"\n")?;
        fs::write(project.path().join("src/main.incn"), "def main() -> None:\n    pass\n")?;
        let (receipt, mut output, files) = fixture_project_output_publication(project.path(), "debug", "snapshot")?;
        let root = tempfile::tempdir()?;
        let store = OvenStore::new(root.path(), OvenStoreLimits::new(1024 * 1024, 1024 * 1024, 1024 * 1024));
        let lock = project.path().join("inspection.lock");
        fs::write(&lock, "retained inspection lock")?;
        let payload = OvenProjectInspectionAuthorityPayload {
            schema_version: OVEN_PROJECT_INSPECTION_AUTHORITY_SCHEMA_VERSION,
            project_identity: output.project_identity.clone(),
            source_authority_digest: output.source_authority_digest.clone(),
            compiler_version: output.compiler_version.clone(),
            registry_lock_digest: digest_bytes(&fs::read(&lock)?),
            registry_source_dependencies: Vec::new(),
            dev_registry_source_dependencies: Vec::new(),
            test_dependency_envelope: None,
            constituents: if missing_constituent {
                vec![OvenProjectInspectionConstituent::Stored {
                    identity: digest_bytes(b"absent native constituent"),
                    artifact_kind: OvenArtifactKind::DirectRustcPlan,
                    receipt: receipt.clone(),
                    base_loaf_identity: None,
                }]
            } else {
                Vec::new()
            },
            registry_sources: Vec::new(),
            generated_out_dirs: Vec::new(),
        };
        let bytes = serde_json::to_string(&payload)?;
        let manifest = store.publish(&OvenArtifactPublishRequest {
            receipt: receipt.clone(),
            domain: "snapshot-fixture".to_string(),
            kind: OvenArtifactKind::ProjectInspectionAuthority,
            payload: bytes.as_bytes().to_vec(),
            materialized_files: vec![OvenArtifactMaterializedFile {
                source_path: lock,
                relative_path: "registry/Cargo.lock".to_string(),
            }],
            materialized_directories: Vec::new(),
        })?;
        output.inspection_authority = Some(OvenProjectInspectionAuthorityRef {
            identity: manifest.identity.clone(),
            receipt_identity: manifest.receipt_identity.clone(),
            build_unit_identity: manifest.build_unit_identity.clone(),
        });
        output.inspection_authority_snapshot = Some(OvenProjectInspectionSnapshot {
            manifest,
            receipt: receipt.clone(),
            payload: bytes,
        });
        let output = publish_project_output_loaf(&store, &receipt, &output, &files)?;
        Ok((project, root, store, output))
    }

    #[test]
    fn completed_loaf_restores_evicted_inspection_metadata_exactly() -> TestResult {
        let (_project, _root, store, output) = fixture(false)?;
        let reference = output
            .payload
            .inspection_authority
            .as_ref()
            .ok_or("missing authority")?;
        let authority_path = authority_entry(&store, &output)?;
        fs::remove_dir_all(&authority_path)?;
        assert!(
            load_project_inspection_authority(
                &store,
                reference,
                &output.payload.project_identity,
                &output.payload.source_authority_digest,
                &output.payload.compiler_version
            )
            .is_err()
        );
        let restored = load_output_inspection_authority(&store, &output)?;
        assert_eq!(restored.identity(), reference.identity);
        restored.verify()?;
        assert_eq!(
            fs::read(restored.artifact_root().join("registry/Cargo.lock"))?,
            b"retained inspection lock"
        );
        assert!(
            output
                .payload
                .files
                .iter()
                .all(|file| !Path::new(&file.output_relative_path).starts_with(SNAPSHOT_PREFIX))
        );
        assert_eq!(store.inspect()?.entries.len(), 2);
        drop(restored);
        load_output_inspection_authority(&store, &output)?.verify()?;
        Ok(())
    }

    #[test]
    fn completed_loaf_does_not_repair_damaged_inspection_owner() -> TestResult {
        let (_project, _root, store, output) = fixture(false)?;
        let reference = output
            .payload
            .inspection_authority
            .as_ref()
            .ok_or("missing authority")?;
        let (_, artifact_root, _, lease) = store.select_payload_for_execution(&reference.identity)?;
        fs::remove_file(artifact_root.join("registry/Cargo.lock"))?;
        drop(lease);
        assert!(load_output_inspection_authority(&store, &output).is_err());
        assert!(!artifact_root.join("registry/Cargo.lock").exists());
        Ok(())
    }

    #[test]
    fn completed_loaf_refuses_changed_snapshot_before_publication() -> TestResult {
        let (_project, _root, store, mut output) = fixture(false)?;
        let path = authority_entry(&store, &output)?;
        fs::remove_dir_all(&path)?;
        let snapshot = output
            .payload
            .inspection_authority_snapshot
            .as_mut()
            .ok_or("missing snapshot")?;
        snapshot.payload.push(' ');
        assert!(load_output_inspection_authority(&store, &output).is_err());
        assert!(!path.exists());
        snapshot_request(&store, &output.payload, &output.artifact_root.join(SNAPSHOT_PREFIX))
            .err()
            .ok_or("changed bytes admitted")?;
        Ok(())
    }

    #[test]
    fn completed_loaf_refuses_invalid_snapshot_files_and_lineage() -> TestResult {
        let (_project, _root, store, mut output) = fixture(false)?;
        let path = authority_entry(&store, &output)?;
        fs::remove_dir_all(&path)?;
        let original = output
            .payload
            .inspection_authority_snapshot
            .clone()
            .ok_or("missing snapshot")?;
        for corruption in 0..4 {
            let mut snapshot = original.clone();
            match corruption {
                0 => snapshot.receipt.identity = digest_bytes(b"different receipt"),
                1 => snapshot.manifest.materialized_files[0].relative_path = "../escape".to_string(),
                2 => {
                    let mut payload: OvenProjectInspectionAuthorityPayload = serde_json::from_str(&snapshot.payload)?;
                    payload.source_authority_digest = digest_bytes(b"different source");
                    snapshot.payload = serde_json::to_string(&payload)?;
                }
                _ => snapshot.manifest.identity = digest_bytes(b"different authority"),
            }
            output.payload.inspection_authority_snapshot = Some(snapshot);
            assert!(load_output_inspection_authority(&store, &output).is_err());
            assert!(!path.exists());
        }
        output.payload.inspection_authority_snapshot = Some(original);
        fs::remove_file(output.artifact_root.join(SNAPSHOT_PREFIX).join("registry/Cargo.lock"))?;
        assert!(load_output_inspection_authority(&store, &output).is_err());
        assert!(!path.exists());
        Ok(())
    }

    #[test]
    fn completed_loaf_restoration_does_not_replace_missing_constituents() -> TestResult {
        let (_project, _root, store, output) = fixture(true)?;
        fs::remove_dir_all(authority_entry(&store, &output)?)?;
        assert!(
            matches!(load_output_inspection_authority(&store, &output), Err(OvenRustcError::Store(ref error)) if whole_entry_missing(&store, error))
        );
        assert_eq!(store.inspect()?.entries.len(), 2);
        Ok(())
    }

    #[test]
    fn completed_loaf_without_snapshot_keeps_the_existing_cache_miss() -> TestResult {
        let (_project, _root, store, mut output) = fixture(false)?;
        let path = authority_entry(&store, &output)?;
        fs::remove_dir_all(&path)?;
        output.payload.inspection_authority_snapshot = None;
        assert!(
            matches!(load_output_inspection_authority(&store, &output), Err(OvenRustcError::Store(ref error)) if whole_entry_missing(&store, error))
        );
        assert!(!path.exists());
        Ok(())
    }
}
