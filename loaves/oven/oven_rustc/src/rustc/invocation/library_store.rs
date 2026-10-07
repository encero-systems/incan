//! Receipt-bound generated libraries retained across caller-owned output directories.

use super::{
    Command, OVEN_DIRECT_RUSTC_OUTPUT_RECEIPT_SCHEMA_VERSION, OvenDirectRustcBake, OvenDirectRustcOutputReceipt,
    OvenDirectRustcOutputRecord, OvenRustcError, OvenTrustedDirectRustcTargetRequest,
    bake_trusted_direct_rustc_library, caller_output_path, caller_output_receipt_path, digest_bytes,
    digest_regular_file, fs, validate_edition, validate_rust_identifier, verify_rustc_identity,
    write_caller_output_record,
};
use oven_store::store::{OvenArtifactKind, OvenArtifactMaterializedFile, OvenArtifactPublishRequest, OvenStore};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

const DOMAIN: &str = "generated-rust-library-v2";
const OUTPUT: &str = "library.rlib";

/// Native output evidence plus the declared environment that accompanies its selected physical plan.
#[derive(Serialize, Deserialize)]
struct StoredLibraryOutput {
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
    let expected = library_recipe(request)?;
    let environment = request
        .artifact_plan
        .map_or(&request.artifacts.compile_environment, |plan| &plan.compile_environment);
    if let Some(bake) = reuse_library(request, store, &expected, environment)? {
        return Ok(bake);
    }
    let output = caller_output_path(request.output, request.artifact_root)?;
    let parent = output
        .parent()
        .ok_or_else(|| recipe_error("library output has no parent"))?;
    fs::create_dir_all(parent).map_err(|source| OvenRustcError::Io {
        path: parent.to_path_buf(),
        source,
    })?;
    let temporary = tempfile::Builder::new()
        .prefix("oven-library-")
        .tempdir_in(parent)
        .map_err(|source| OvenRustcError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    let staged = temporary.path().join(OUTPUT);
    // A caller-local sidecar predates environment-qualified store reuse. On a store miss, compile into a fresh
    // projection so that weaker evidence cannot accidentally turn an environment change into a warm native hit.
    let staged_request = OvenTrustedDirectRustcTargetRequest {
        output: &staged,
        ..*request
    };
    let mut bake = bake_trusted_direct_rustc_library(&staged_request)?;
    let published = store.publish(&OvenArtifactPublishRequest {
        receipt: request.receipt.clone(),
        domain: DOMAIN.to_string(),
        kind: OvenArtifactKind::Engine,
        payload: serde_json::to_vec(&StoredLibraryOutput {
            record: OvenDirectRustcOutputRecord {
                inputs: expected,
                output_digest: bake.output_digest.clone(),
            },
            environment: environment.clone(),
        })
        .map_err(recipe_error)?,
        materialized_files: vec![OvenArtifactMaterializedFile {
            source_path: bake.output.clone(),
            relative_path: OUTPUT.to_string(),
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

/// Validate the source and compiler before projecting the existing native sidecar contract into a portable recipe.
fn library_recipe(
    request: &OvenTrustedDirectRustcTargetRequest<'_>,
) -> Result<OvenDirectRustcOutputReceipt, OvenRustcError> {
    request.receipt.verify_identity().map_err(recipe_error)?;
    verify_rustc_identity(request.rustc, &request.receipt.intent.toolchain)?;
    validate_rust_identifier(request.crate_name)?;
    validate_edition(request.edition)?;
    super::super::driver_grant::apply_driver_grant(
        &mut Command::new(request.rustc),
        request.receipt,
        request.crate_name,
    )?;
    let digest = digest_regular_file(request.source, "source")?;
    let expected = request
        .receipt
        .sources
        .supplemental_digests
        .get(request.source_evidence_key)
        .ok_or_else(|| recipe_error("receipt has no generated library source evidence"))?;
    if *expected != digest {
        return Err(OvenRustcError::SourceEvidenceMismatch {
            key: request.source_evidence_key.to_string(),
            expected: expected.clone(),
            actual: digest,
        });
    }
    let artifacts = request.artifacts.for_source_evidence(request.source_evidence_key)?;
    Ok(OvenDirectRustcOutputReceipt {
        schema_version: OVEN_DIRECT_RUSTC_OUTPUT_RECEIPT_SCHEMA_VERSION,
        receipt_identity: request.receipt.identity.clone(),
        link_closure_identity: None,
        artifact_manifest_digest: digest_bytes(&serde_json::to_vec(&artifacts).map_err(recipe_error)?),
        source_digest: digest,
        crate_name: request.crate_name.to_string(),
        edition: request.edition.to_string(),
        features: request.features.to_vec(),
        test_harness: false,
        prefer_dynamic: request.prefer_dynamic,
        output_kind: "library".to_string(),
        caller_owned_library_digests: request
            .artifact_plan
            .map(|plan| plan.caller_owned_library_digests.clone())
            .unwrap_or_default(),
    })
}

/// Select matching compiled bytes under lease and write a caller-local sidecar with the current receipt identity.
fn reuse_library(
    request: &OvenTrustedDirectRustcTargetRequest<'_>,
    store: &OvenStore,
    expected: &OvenDirectRustcOutputReceipt,
    environment: &BTreeMap<String, String>,
) -> Result<Option<OvenDirectRustcBake>, OvenRustcError> {
    let candidates = store.select_payloads_matching_for_execution(|manifest| {
        manifest.kind == OvenArtifactKind::Engine
            && manifest.domain == DOMAIN
            && manifest.receipt_identity == request.receipt.identity
            && manifest.build_unit_identity == request.receipt.build_unit_identity
            && manifest.intent == request.receipt.intent
    })?;
    for candidate in candidates {
        let stored: StoredLibraryOutput = serde_json::from_slice(&candidate.payload).map_err(recipe_error)?;
        if stored.record.inputs != *expected || stored.environment != *environment {
            continue;
        }
        let record = stored.record;
        let (_, root, _, lease) = candidate.into_parts();
        let source = root.join(OUTPUT);
        if digest_regular_file(&source, "stored library")? != record.output_digest {
            return Err(recipe_error("stored generated library differs from its native recipe"));
        }
        let output = caller_output_path(request.output, request.artifact_root)?;
        let parent = output
            .parent()
            .ok_or_else(|| recipe_error("library output has no parent"))?;
        fs::create_dir_all(parent).map_err(|source| OvenRustcError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
        let mut source_file = fs::File::open(&source).map_err(|source_error| OvenRustcError::Io {
            path: source.clone(),
            source: source_error,
        })?;
        let mut temporary = tempfile::NamedTempFile::new_in(parent).map_err(|source| OvenRustcError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
        // Store files remain immutable; caller projections remain writable and are replaced atomically.
        std::io::copy(&mut source_file, temporary.as_file_mut()).map_err(|source| OvenRustcError::Io {
            path: output.clone(),
            source,
        })?;
        temporary.persist(&output).map_err(|error| OvenRustcError::Io {
            path: output.clone(),
            source: error.error,
        })?;
        let mut inputs = expected.clone();
        inputs.receipt_identity = request.receipt.identity.clone();
        write_caller_output_record(
            &output,
            &OvenDirectRustcOutputRecord {
                inputs,
                output_digest: record.output_digest.clone(),
            },
        )?;
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

/// Keep native recipe and stored-payload failures at the existing executor input-error boundary.
fn recipe_error(error: impl std::fmt::Display) -> OvenRustcError {
    OvenRustcError::InvalidInput {
        field: "generated library recipe",
        message: error.to_string(),
    }
}
