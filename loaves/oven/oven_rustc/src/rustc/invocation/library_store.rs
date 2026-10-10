//! Receipt-bound generated native outputs retained across caller-owned directories.

use super::{
    OvenDirectRustcBake, OvenDirectRustcOutputKind, OvenDirectRustcOutputReceipt, OvenDirectRustcOutputRecord,
    OvenRustcError, OvenTrustedDirectRustcTargetRequest, bake_trusted_direct_rustc_library,
    bake_trusted_direct_rustc_run, bake_trusted_direct_rustc_test, caller_output_path, caller_output_receipt_path,
    caller_output_reusable_digest, digest_regular_file, direct_rustc_output_recipe, fs, write_caller_output_record,
};
use oven_store::store::{OvenArtifactKind, OvenArtifactMaterializedFile, OvenArtifactPublishRequest, OvenStore};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Native output evidence plus the declared environment that accompanies its selected physical plan.
#[derive(Serialize, Deserialize)]
struct StoredNativeOutput {
    /// Existing input/output binding, including exact caller-owned dependency bytes.
    record: OvenDirectRustcOutputRecord,
    /// Portable declaration values; source-relative tokens retain the executor's relocation contract.
    environment: BTreeMap<String, String>,
}

/// Reuse or compile one admitted generated library in the caller-selected bounded Oven store.
///
/// The native recipe binds generated source bytes, the selected manifest, features, target, toolchain, crate identity,
/// and caller-owned dependency digests. Store selection requires the same enclosing receipt, build unit, and intent,
/// and retains the immutable output lease while projecting its verified bytes into the new caller directory.
pub fn bake_trusted_direct_rustc_library_in_store(
    request: &OvenTrustedDirectRustcTargetRequest<'_>,
    store: &OvenStore,
) -> Result<OvenDirectRustcBake, OvenRustcError> {
    bake_native_in_store(request, store, OvenDirectRustcOutputKind::Library, false)
}

/// Reuse or compile one admitted executable in the bounded store, retaining its lease through execution.
///
/// Uses the ordinary direct executor's recipe, including its pinned linker closure and caller-owned library bytes.
/// Projections are writable executables; compatible callers share immutable bytes without sharing mutable outputs.
pub fn bake_trusted_direct_rustc_run_in_store(
    request: &OvenTrustedDirectRustcTargetRequest<'_>,
    store: &OvenStore,
) -> Result<OvenDirectRustcBake, OvenRustcError> {
    bake_native_in_store(request, store, OvenDirectRustcOutputKind::Binary, false)
}

/// Retain receipt-qualified libtest binaries across edits, restored sources and compatible caller projections.
///
/// Harness identity is part of the ordinary executor recipe and a separate store domain. A normal executable with
/// identical source and enclosing receipt cannot be substituted for the native test runner.
pub fn bake_trusted_direct_rustc_test_in_store(
    request: &OvenTrustedDirectRustcTargetRequest<'_>,
    store: &OvenStore,
) -> Result<OvenDirectRustcBake, OvenRustcError> {
    bake_native_in_store(request, store, OvenDirectRustcOutputKind::Binary, true)
}

/// Keep publication, selection and projection identical for libraries and executables.
fn bake_native_in_store(
    request: &OvenTrustedDirectRustcTargetRequest<'_>,
    store: &OvenStore,
    kind: OvenDirectRustcOutputKind,
    test_harness: bool,
) -> Result<OvenDirectRustcBake, OvenRustcError> {
    let (domain, filename) = match (kind, test_harness) {
        (OvenDirectRustcOutputKind::Library, false) => ("generated-rust-library-v2", "library.rlib"),
        (OvenDirectRustcOutputKind::Binary, false) => ("generated-rust-binary-v2", "program"),
        (OvenDirectRustcOutputKind::Binary, true) => ("generated-rust-test-v2", "native-libtest"),
        _ => return Err(recipe_error("unsupported shared native output kind")),
    };
    let expected = direct_rustc_output_recipe(request, request.source_evidence_key, test_harness, kind)?;
    let environment = request
        .artifact_plan
        .map_or(&request.artifacts.compile_environment, |plan| &plan.compile_environment);
    if let Some(bake) = reuse_native(request, store, &expected, environment, domain, filename)? {
        return Ok(bake);
    }
    if std::env::var_os("INCAN_TEST_REQUIRE_STORED_NATIVE_REUSE").is_some() {
        return Err(recipe_error(
            "required stored native output reuse missed before compilation",
        ));
    }
    let output = caller_output_path(request.output, request.artifact_root)?;
    let parent = output
        .parent()
        .ok_or_else(|| recipe_error("native output has no parent"))?;
    fs::create_dir_all(parent).map_err(|source| OvenRustcError::Io {
        path: parent.to_path_buf(),
        source,
    })?;
    let temporary = tempfile::Builder::new()
        .prefix("oven-native-")
        .tempdir_in(parent)
        .map_err(|source| OvenRustcError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    let staged = temporary.path().join(filename);
    // A caller-local sidecar predates environment-qualified store reuse. On a store miss, compile into a fresh
    // projection so that weaker evidence cannot accidentally turn an environment change into a warm native hit.
    let staged_request = OvenTrustedDirectRustcTargetRequest {
        output: &staged,
        ..*request
    };
    let mut bake = match (kind, test_harness) {
        (OvenDirectRustcOutputKind::Library, false) => bake_trusted_direct_rustc_library(&staged_request)?,
        (OvenDirectRustcOutputKind::Binary, false) => bake_trusted_direct_rustc_run(&staged_request)?,
        (OvenDirectRustcOutputKind::Binary, true) => bake_trusted_direct_rustc_test(&staged_request)?,
        _ => return Err(recipe_error("unsupported shared native output kind")),
    };
    let published = store.publish(&OvenArtifactPublishRequest {
        receipt: request.receipt.clone(),
        domain: domain.to_string(),
        kind: OvenArtifactKind::Engine,
        payload: serde_json::to_vec(&StoredNativeOutput {
            record: OvenDirectRustcOutputRecord {
                inputs: expected,
                output_digest: bake.output_digest.clone(),
            },
            environment: environment.clone(),
        })
        .map_err(recipe_error)?,
        materialized_files: vec![OvenArtifactMaterializedFile {
            source_path: bake.output.clone(),
            relative_path: filename.to_string(),
        }],
        materialized_directories: Vec::new(),
    })?;
    let staged_sidecar = caller_output_receipt_path(&bake.output)?;
    let sidecar = caller_output_receipt_path(&output)?;
    fs::rename(&bake.output, &output).map_err(|source| OvenRustcError::Io {
        path: output.clone(),
        source,
    })?;
    fs::rename(staged_sidecar, &sidecar).map_err(|source| OvenRustcError::Io { path: sidecar, source })?;
    bake.output = output;
    let (_, _, _, lease) = store.select_payload_for_execution(&published.identity)?;
    bake.lease = Some(lease);
    Ok(bake)
}

/// Select matching compiled bytes under lease and write a caller-local sidecar with the current receipt identity.
fn reuse_native(
    request: &OvenTrustedDirectRustcTargetRequest<'_>,
    store: &OvenStore,
    expected: &OvenDirectRustcOutputReceipt,
    environment: &BTreeMap<String, String>,
    domain: &str,
    filename: &str,
) -> Result<Option<OvenDirectRustcBake>, OvenRustcError> {
    let candidates = store.select_payloads_matching_for_execution(|manifest| {
        manifest.kind == OvenArtifactKind::Engine
            && manifest.domain == domain
            && manifest.receipt_identity == request.receipt.identity
            && manifest.build_unit_identity == request.receipt.build_unit_identity
            && manifest.intent == request.receipt.intent
    })?;
    for candidate in candidates {
        let stored: StoredNativeOutput = serde_json::from_slice(&candidate.payload).map_err(recipe_error)?;
        if stored.record.inputs != *expected || stored.environment != *environment {
            continue;
        }
        let record = stored.record;
        let (_, root, _, lease) = candidate.into_parts();
        let source = root.join(filename);
        if digest_regular_file(&source, "stored native output")? != record.output_digest {
            return Err(recipe_error(
                "stored generated native output differs from its native recipe",
            ));
        }
        let output = caller_output_path(request.output, request.artifact_root)?;
        let parent = output
            .parent()
            .ok_or_else(|| recipe_error("native output has no parent"))?;
        fs::create_dir_all(parent).map_err(|source| OvenRustcError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
        let permissions = writable_projection_permissions(&source, expected.output_kind == "binary")?;
        let current = fs::symlink_metadata(&output).is_ok_and(|metadata| {
            metadata.is_file()
                && !metadata.file_type().is_symlink()
                && !metadata.permissions().readonly()
                && executable_projection(&metadata, expected.output_kind == "binary")
        }) && caller_output_reusable_digest(&output, expected).as_deref()
            == Some(record.output_digest.as_str());
        if !current {
            let mut source_file = fs::File::open(&source).map_err(|error| OvenRustcError::Io {
                path: source.clone(),
                source: error,
            })?;
            let mut temporary = tempfile::NamedTempFile::new_in(parent).map_err(|source| OvenRustcError::Io {
                path: parent.to_path_buf(),
                source,
            })?;
            // Store files remain immutable; replace caller projections atomically without inheriting read-only state.
            std::io::copy(&mut source_file, temporary.as_file_mut()).map_err(|source| OvenRustcError::Io {
                path: output.clone(),
                source,
            })?;
            temporary
                .as_file()
                .set_permissions(permissions)
                .map_err(|source| OvenRustcError::Io {
                    path: output.clone(),
                    source,
                })?;
            temporary.persist(&output).map_err(|error| OvenRustcError::Io {
                path: output.clone(),
                source: error.error,
            })?;
            write_caller_output_record(
                &output,
                &OvenDirectRustcOutputRecord {
                    inputs: expected.clone(),
                    output_digest: record.output_digest.clone(),
                },
            )?;
        }
        return Ok(Some(OvenDirectRustcBake {
            source_digest: expected.source_digest.clone(),
            output,
            output_digest: record.output_digest,
            cargo_process_started: false,
            reused: true,
            lease: Some(lease),
        }));
    }
    Ok(None)
}

/// Copy native executable bits while granting only the caller projection owner write access.
fn writable_projection_permissions(
    source: &std::path::Path,
    executable: bool,
) -> Result<fs::Permissions, OvenRustcError> {
    let metadata = fs::metadata(source).map_err(|error| OvenRustcError::Io {
        path: source.to_path_buf(),
        source: error,
    })?;
    if !executable_projection(&metadata, executable) {
        return Err(recipe_error("stored native executable has no execute permission"));
    }
    let mut permissions = metadata.permissions();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        permissions.set_mode(permissions.mode() | 0o200);
    }
    #[cfg(not(unix))]
    permissions.set_readonly(false);
    Ok(permissions)
}

/// Native Unix executables must remain executable; other platforms use their native process admission rules.
fn executable_projection(metadata: &fs::Metadata, executable: bool) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        !executable || metadata.permissions().mode() & 0o100 != 0
    }
    #[cfg(not(unix))]
    {
        let _ = (metadata, executable);
        true
    }
}

/// Keep native recipe and stored-payload failures at the existing executor input-error boundary.
fn recipe_error(error: impl std::fmt::Display) -> OvenRustcError {
    OvenRustcError::InvalidInput {
        field: "generated native recipe",
        message: error.to_string(),
    }
}
