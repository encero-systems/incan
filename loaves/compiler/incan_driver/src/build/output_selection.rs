//! Selecting the baked outputs a command may reuse, and keeping their reports portable across roots.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::build::bake::discover_oven_bake_project_targets;
use crate::build::output_paths::{
    native_project_output_target, project_relative_entrypoint, validated_project_output_relative_path,
};
use crate::build::publication::stored_project_output_from_parts;
use crate::build::source_authority::{
    baked_project_lock_dependencies_fingerprint, digest_baked_project_source_authority, project_bake_receipt_path,
};
use crate::build::{
    CurrentDebugProjectOutputExpectation, OVEN_PROJECT_OUTPUT_PAYLOAD_SCHEMA_VERSION,
    OVEN_PROJECT_OUTPUT_REPORT_EXTERNAL_ROOT, OVEN_PROJECT_OUTPUT_REPORT_PATH_TAG,
    OVEN_PROJECT_OUTPUT_REPORT_SCHEMA_VERSION, OvenBakeProjectTarget, OvenProjectOutputPayload,
    OvenProjectOutputReportSnapshot, OvenStoredProjectOutput, oven_bake_project_target_identity,
};
use crate::build_report::BuildReport;
use crate::error::{CliError, CliResult};
use incan_lang::version::INCAN_VERSION;
use oven_model::manifest::{LOAF_MANIFEST_FILENAME, ProjectManifest};
use oven_rustc::rustc::{
    OvenLoadedProjectInspectionAuthority, resolve_active_rustc, rustc_host_target, rustc_identity,
};
use oven_store::digest_bytes;
use oven_store::store::{OvenArtifactKind, OvenStore, OvenStoreExecutionRequest, OvenStorePayloadIndex};

/// Profile, native target and optional explicit target/toolchain constraint for one output lookup partition.
type ProjectOutputIndexPartition = (String, String, Option<(String, String)>);

/// The Store retains this payload projection alongside its existing parsed manifests, without retaining leases.
static PROJECT_OUTPUT_INDEX: OvenStorePayloadIndex = OvenStorePayloadIndex {
    namespace: "incan.project-output/1",
    matches_partition: project_output_index_partition_matches,
    key: project_output_payload_index_key,
};

/// Select exactly the manifest partition used by ordinary output lookup, preserving explicit toolchain constraints.
fn project_output_index_partition_matches(partition: &str, manifest: &oven_store::store::OvenArtifactManifest) -> bool {
    let Ok((profile, target, required_toolchain)) = serde_json::from_str::<ProjectOutputIndexPartition>(partition)
    else {
        return false;
    };
    manifest.kind == OvenArtifactKind::ProjectOutput
        && manifest.intent.profile == profile
        && manifest.intent.target == target
        && required_toolchain.is_none_or(|(target, toolchain)| {
            manifest.intent.target == target && manifest.intent.toolchain == toolchain
        })
}

/// Encode the source and preparation facts that identify reusable output, independently of replaceable lock hints.
fn project_output_request_key(
    project_target: &str,
    target_identity: &str,
    source_authority: &str,
    compiler_version: &str,
    entrypoint: &str,
    preparation_identity: Option<&str>,
) -> CliResult<String> {
    serde_json::to_string(&(
        project_target,
        target_identity,
        source_authority,
        compiler_version,
        entrypoint,
        preparation_identity,
    ))
    .map_err(|error| CliError::failure(format!("failed to encode Oven project-output lookup key: {error}")))
}

/// Project only structurally understood, receipt-bound payloads; final selection still verifies their exact owners.
fn project_output_payload_index_key(
    manifest: &oven_store::store::OvenArtifactManifest,
    bytes: &[u8],
) -> Option<String> {
    let payload = serde_json::from_slice::<OvenProjectOutputPayload>(bytes).ok()?;
    if payload.schema_version != OVEN_PROJECT_OUTPUT_PAYLOAD_SCHEMA_VERSION
        || payload.receipt_identity != manifest.receipt_identity
        || payload.build_unit_identity != manifest.build_unit_identity
        || payload.plan_identity.trim().is_empty()
    {
        return None;
    }
    project_output_request_key(
        &payload.project_target,
        &payload.target_identity,
        &payload.source_authority_digest,
        &payload.compiler_version,
        &payload.entrypoint_relative_path,
        payload.preparation_identity_digest.as_deref(),
    )
    .ok()
}

/// Read the existing replaceable output address; only Store admission can turn this hint into authority.
pub(super) fn project_output_locator(receipt_path: &Path) -> Option<String> {
    fs::read(receipt_path.with_extension("output.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<String>(&bytes).ok())
        .filter(|identity| {
            identity
                .strip_prefix("sha256:")
                .is_some_and(|hex| hex.len() == 64 && hex.bytes().all(|byte| byte.is_ascii_hexdigit()))
        })
}

/// Resolve an optional local locator through the store's verified payload and active-lease boundary.
///
/// The locator is only a search hint. Explicit bake reuse requires its local receipt lineage; ordinary selection
/// supplies its current source, preparation and intent predicates instead. Both retain Store integrity and leases.
/// Missing, malformed, reclaimed or mismatched hints fall back to normal selection and never authorize execution alone.
#[track_caller]
pub(super) fn located_project_output(
    store: &OvenStore,
    receipt_path: &Path,
    receipt: Option<&oven_store::OvenReceipt>,
    payload_matches: impl Fn(&oven_store::store::OvenArtifactManifest, &[u8]) -> bool,
) -> CliResult<Option<OvenStoredProjectOutput>> {
    let caller = std::panic::Location::caller();
    let _invocation = tracing::trace_span!(target: "incan::invocation", "incan_driver::located_project_output", input = %receipt_path.display(), caller_file = caller.file(), caller_line = u64::from(caller.line())).entered();
    let Some(identity) = project_output_locator(receipt_path) else {
        return Ok(None);
    };
    let Ok((manifest, root, bytes, lease)) = store.select_payload_for_execution(&identity) else {
        return Ok(None);
    };
    if manifest.kind != OvenArtifactKind::ProjectOutput
        || receipt.is_some_and(|receipt| {
            manifest.receipt_identity != receipt.identity
                || manifest.build_unit_identity != receipt.build_unit_identity
                || manifest.intent != receipt.intent
        })
        || !payload_matches(&manifest, &bytes)
    {
        return Ok(None);
    }
    let Ok(payload) = serde_json::from_slice::<super::OvenProjectOutputPayload>(&bytes) else {
        return Ok(None);
    };
    if payload.schema_version != super::OVEN_PROJECT_OUTPUT_PAYLOAD_SCHEMA_VERSION {
        return Ok(None);
    }
    crate::build::publication::stored_project_output_from_parts(manifest, root, payload, lease).map(Some)
}

/// Atomically retain a verified output's address as a replaceable local selection hint.
pub(super) fn remember_project_output(receipt_path: &Path, identity: &str) {
    let locator = receipt_path.with_extension("output.json");
    if fs::read(&locator)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<String>(&bytes).ok())
        .as_deref()
        == Some(identity)
    {
        return;
    }
    let Some(parent) = locator.parent() else {
        return;
    };
    if fs::create_dir_all(parent).is_err() {
        return;
    }
    let staged = locator.with_extension(format!("{}.tmp", std::process::id()));
    let result = serde_json::to_vec(identity)
        .map_err(std::io::Error::other)
        .and_then(|bytes| fs::write(&staged, bytes))
        .and_then(|()| fs::rename(&staged, &locator));
    if result.is_err() {
        let _removed = fs::remove_file(staged);
    }
}

/// Match the selected preparation route without allowing legacy output to satisfy a required ordinary identity.
/// Legacy explicit outputs remain selectable when no ordinary library route was requested.
pub(super) fn project_output_preparation_matches(
    payload: &OvenProjectOutputPayload,
    preparation_identity: Option<&str>,
    required: bool,
) -> bool {
    match payload.preparation_identity_digest.as_deref() {
        Some(identity) => Some(identity) == preparation_identity,
        None => !required,
    }
}

/// Select a completed executable output before frontend work on an exact project match.
///
/// This intentionally consults no ambient `rustc`: the stored native output is already bound to its publisher receipt
/// and target. Fresh compilation remains responsible for resolving a compatible Rust compiler when this fast path
/// misses.
#[cfg(test)]
pub fn select_baked_project_output(
    store: &OvenStore,
    project_root: &Path,
    entrypoint: &Path,
    target: OvenBakeProjectTarget,
    profile: &str,
) -> CliResult<Option<OvenStoredProjectOutput>> {
    let source_authority_digest = digest_baked_project_source_authority(project_root)?;
    select_baked_project_output_with_source_authority(
        store,
        project_root,
        entrypoint,
        target,
        profile,
        &source_authority_digest,
        None,
    )
}

/// Return a stable, portable owner identity for a manifest-backed project.
///
/// Project Loafs must remain reusable from an identical clean worktree, so an absolute path cannot be part of this
/// identity. The project distribution name is the durable coordinate; the target and entrypoint remain separate payload
/// facts checked by the selector. Exact source authority prevents a same-named unrelated project from reusing a
/// different completed output; project-local receipts separately govern stale-output diagnostics.
pub fn baked_project_owner_identity(project_root: &Path) -> CliResult<String> {
    let manifest_path = project_root.join(LOAF_MANIFEST_FILENAME);
    let manifest = ProjectManifest::load(&manifest_path).map_err(|error| CliError::failure(error.to_string()))?;
    let project_name = manifest
        .project
        .as_ref()
        .and_then(|project| project.name.as_deref())
        .unwrap_or("unnamed-incan-project");
    Ok(baked_project_owner_identity_for_name(project_name))
}

/// Derive the portable completed-output owner for a declared project name.
///
/// Publishers use the same derivation when checking a retained output without reopening its source checkout.
pub fn baked_project_owner_identity_for_name(project_name: &str) -> String {
    digest_bytes(format!("incan_oven_project_output_owner/1\\0{project_name}").as_bytes())
}

/// Whether this local project has a previously baked completed output whose source authority is no longer exact.
///
/// This is deliberately distinct from ordinary selection. It never authorizes an output. The project-local receipt is
/// the lineage proof: scanning the global store by package name alone would let an unrelated same-named project trigger
/// a false stale-output refusal. Exact output selection remains portable and path-independent; only this diagnostic
/// requires evidence that this checkout crossed the explicit bake boundary.
pub fn has_stale_baked_project_output(
    store: &OvenStore,
    project_root: &Path,
    entrypoint: &Path,
    target: OvenBakeProjectTarget,
    profile: &str,
) -> CliResult<bool> {
    let Some(entrypoint_relative_path) = project_relative_entrypoint(project_root, entrypoint) else {
        return Ok(false);
    };
    let target_identity = oven_bake_project_target_identity(project_root, target, entrypoint)?;
    let Some(native_target) = native_project_output_target() else {
        return Ok(false);
    };
    let receipt_path = project_bake_receipt_path(project_root, target, entrypoint, profile)?;
    let receipt = match fs::read(&receipt_path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<oven_store::OvenReceipt>(&bytes).ok())
    {
        Some(receipt)
            if receipt.verify_identity().is_ok()
                && receipt.intent.profile == profile
                && receipt.intent.target == native_target =>
        {
            receipt
        }
        Some(_) | None => return Ok(false),
    };
    let candidates = store
        .select_payloads_matching_for_execution(
            |manifest| {
                manifest.kind == OvenArtifactKind::ProjectOutput
                    && manifest.intent.profile == profile
                    && manifest.intent.target == native_target
                    && manifest.receipt_identity == receipt.identity
                    && manifest.build_unit_identity == receipt.build_unit_identity
            },
            |_, _| true,
        )
        .map_err(|error| CliError::failure(format!("failed to inspect Oven project-output Loafs: {error}")))?;
    for candidate in candidates {
        let Ok(payload) = serde_json::from_slice::<OvenProjectOutputPayload>(candidate.payload()) else {
            continue;
        };
        if payload.schema_version == OVEN_PROJECT_OUTPUT_PAYLOAD_SCHEMA_VERSION
            && payload.project_target == target.as_str()
            && payload.target_identity == target_identity
            && payload.compiler_version == INCAN_VERSION
            && payload.entrypoint_relative_path == entrypoint_relative_path
            && payload.receipt_identity == receipt.identity
            && payload.build_unit_identity == receipt.build_unit_identity
        {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Select a completed output after a caller has already computed the exact build-authority digest.
pub fn select_baked_project_output_with_source_authority(
    store: &OvenStore,
    project_root: &Path,
    entrypoint: &Path,
    target: OvenBakeProjectTarget,
    profile: &str,
    source_authority_digest: &str,
    required_target_toolchain: Option<(&str, &str)>,
) -> CliResult<Option<OvenStoredProjectOutput>> {
    Ok(project_outputs_with_source_authority(
        store,
        project_root,
        entrypoint,
        target,
        profile,
        source_authority_digest,
        required_target_toolchain,
        true,
        None,
        None,
    )?
    .into_iter()
    .next())
}

/// Select with a canonical lock fingerprint already reconciled by the request owner, sharing all admission checks.
#[allow(clippy::too_many_arguments)]
pub(super) fn select_baked_project_output_with_fingerprint(
    store: &OvenStore,
    project_root: &Path,
    entrypoint: &Path,
    target: OvenBakeProjectTarget,
    profile: &str,
    source_authority_digest: &str,
    required_target_toolchain: Option<(&str, &str)>,
    fingerprint: &Option<String>,
) -> CliResult<Option<OvenStoredProjectOutput>> {
    Ok(project_outputs_with_source_authority(
        store,
        project_root,
        entrypoint,
        target,
        profile,
        source_authority_digest,
        required_target_toolchain,
        true,
        Some(fingerprint),
        None,
    )?
    .into_iter()
    .next())
}

/// Collect verified source-current outputs, preferring the local explicit-bake receipt and then canonical lock.
///
/// Keep older candidates available for coherent multi-profile selection and non-strict stale-lock warnings.
pub fn matching_baked_project_outputs_with_source_authority(
    store: &OvenStore,
    project_root: &Path,
    entrypoint: &Path,
    target: OvenBakeProjectTarget,
    profile: &str,
    source_authority_digest: &str,
    required_target_toolchain: Option<(&str, &str)>,
) -> CliResult<Vec<OvenStoredProjectOutput>> {
    project_outputs_with_source_authority(
        store,
        project_root,
        entrypoint,
        target,
        profile,
        source_authority_digest,
        required_target_toolchain,
        false,
        None,
        None,
    )
}

/// Apply the same intent, payload and lineage checks to an exact locator and the fallback collection.
///
/// Multi-profile consumers retain the complete ordered candidate set. A single-output consumer may avoid the scan
/// only when its verified hint has the preferred receipt lineage and current lock fingerprint. Compiler tooling may
/// additionally require exact publisher-compiler and dependency authority; those predicates apply identically to the
/// locator and fallback collection, so rejecting a stale locator cannot hide another valid candidate.
#[allow(clippy::too_many_arguments)]
pub(super) fn project_outputs_with_source_authority(
    store: &OvenStore,
    project_root: &Path,
    entrypoint: &Path,
    target: OvenBakeProjectTarget,
    profile: &str,
    source_authority_digest: &str,
    required_target_toolchain: Option<(&str, &str)>,
    first_only: bool,
    observed_fingerprint: Option<&Option<String>>,
    required_authority: Option<(&str, &str)>,
) -> CliResult<Vec<OvenStoredProjectOutput>> {
    let Some(entrypoint_relative_path) = project_relative_entrypoint(project_root, entrypoint) else {
        return Ok(Vec::new());
    };
    let target_identity = oven_bake_project_target_identity(project_root, target, entrypoint)?;
    if !project_root.join(LOAF_MANIFEST_FILENAME).is_file() {
        return Ok(Vec::new());
    }
    let Some(native_target) = native_project_output_target() else {
        return Ok(Vec::new());
    };
    let ordinary_library = target == OvenBakeProjectTarget::Library
        && super::native_loaf_plan::ConfiguredNativeLoafInputs::discover()?.is_some();
    let preparation_identity = if ordinary_library {
        Some(super::source_authority::ordinary_library_preparation_identity_digest(
            None,
        )?)
    } else {
        super::source_authority::ordinary_preparation_identity_digest().ok()
    };
    let manifest_matches = |manifest: &oven_store::store::OvenArtifactManifest| {
        manifest.kind == OvenArtifactKind::ProjectOutput
            && manifest.intent.profile == profile
            && manifest.intent.target == native_target
            && required_target_toolchain.is_none_or(|(target, toolchain)| {
                manifest.intent.target == target && manifest.intent.toolchain == toolchain
            })
    };
    let payload_matches = |manifest: &oven_store::store::OvenArtifactManifest, bytes: &[u8]| {
        let Ok(payload) = serde_json::from_slice::<OvenProjectOutputPayload>(bytes) else {
            return false;
        };
        payload.schema_version == OVEN_PROJECT_OUTPUT_PAYLOAD_SCHEMA_VERSION
            && payload.project_target == target.as_str()
            && payload.target_identity == target_identity
            && payload.source_authority_digest == source_authority_digest
            && required_authority.is_none_or(|(compiler, dependencies)| {
                payload.compiler_identity_digest.as_deref() == Some(compiler)
                    && payload.dependency_authority_digest.as_deref() == Some(dependencies)
            })
            && project_output_preparation_matches(&payload, preparation_identity.as_deref(), ordinary_library)
            && payload.compiler_version == INCAN_VERSION
            && payload.entrypoint_relative_path == entrypoint_relative_path
            && payload.receipt_identity == manifest.receipt_identity
            && payload.build_unit_identity == manifest.build_unit_identity
            && !payload.plan_identity.trim().is_empty()
    };
    let current_receipt = current_project_output_receipt(project_root, target, entrypoint, profile)?;
    let current_fingerprint = match observed_fingerprint {
        Some(fingerprint) => fingerprint.clone(),
        None => baked_project_lock_dependencies_fingerprint(project_root)?,
    };
    let receipt_path = project_bake_receipt_path(project_root, target, entrypoint, profile)?;
    if first_only
        && let Some(located) =
            located_project_output(store, &receipt_path, current_receipt.as_ref(), |manifest, bytes| {
                manifest_matches(manifest) && payload_matches(manifest, bytes)
            })?
        && located.payload.lock_dependencies_fingerprint == current_fingerprint
    {
        tracing::trace!(target: "incan::invocation", decision = "reuse_project_output_locator", input = %located.identity);
        return Ok(vec![located]);
    }
    let partition = serde_json::to_string(&(profile, &native_target, required_target_toolchain)).map_err(|error| {
        CliError::failure(format!(
            "failed to encode Oven project-output lookup partition: {error}"
        ))
    })?;
    let mut keys = Vec::with_capacity(2);
    if let Some(preparation) = preparation_identity.as_deref() {
        keys.push(project_output_request_key(
            target.as_str(),
            &target_identity,
            source_authority_digest,
            INCAN_VERSION,
            &entrypoint_relative_path,
            Some(preparation),
        )?);
    }
    if !ordinary_library {
        keys.push(project_output_request_key(
            target.as_str(),
            &target_identity,
            source_authority_digest,
            INCAN_VERSION,
            &entrypoint_relative_path,
            None,
        )?);
    }
    let selected = store
        .select_payloads_for_execution(OvenStoreExecutionRequest::Indexed {
            index: &PROJECT_OUTPUT_INDEX,
            partition: &partition,
            keys: &keys,
        })
        .map_err(|error| CliError::failure(format!("failed to select Oven project-output Loaf: {error}")))?;
    let mut matches = Vec::new();
    for selected in selected {
        if !manifest_matches(selected.manifest()) || !payload_matches(selected.manifest(), selected.payload()) {
            continue;
        }
        let Ok(payload) = serde_json::from_slice::<OvenProjectOutputPayload>(selected.payload()) else {
            // Selection already checked these immutable payload bytes before acquiring the execution lease.
            continue;
        };
        let (manifest, artifact_root, _payload, lease) = selected.into_parts();
        matches.push(stored_project_output_from_parts(
            manifest,
            artifact_root,
            payload,
            lease,
        )?);
    }
    // Prefer the explicit bake's verified lineage before the derived lock fingerprint: a non-strict stale lock
    // must not make an older SDK output outrank the current bake. Missing local receipts still permit store reuse.
    matches.sort_by_key(|output| {
        let current_lineage = current_receipt.as_ref().is_some_and(|receipt| {
            output.payload.receipt_identity == receipt.identity
                && output.payload.build_unit_identity == receipt.build_unit_identity
                && output.intent == receipt.intent
        });
        (
            !current_lineage,
            output.payload.lock_dependencies_fingerprint != current_fingerprint,
            output.identity.clone(),
        )
    });
    if first_only && let Some(selected) = matches.first() {
        remember_project_output(&receipt_path, &selected.identity);
    }
    Ok(matches)
}

/// Read a verified local bake lineage when available without requiring a mutable caller projection for reuse.
pub fn current_project_output_receipt(
    project_root: &Path,
    target: OvenBakeProjectTarget,
    entrypoint: &Path,
    profile: &str,
) -> CliResult<Option<oven_store::OvenReceipt>> {
    let path = project_bake_receipt_path(project_root, target, entrypoint, profile)?;
    Ok(fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<oven_store::OvenReceipt>(&bytes).ok())
        .filter(|receipt| receipt.verify_identity().is_ok()))
}

/// Select every source-current baked debug output through one leased ProjectOutput candidate scan.
///
/// Local explicit-bake receipts identify the exact target lineages before any payload is decoded. A malformed payload
/// from another project is therefore irrelevant, while malformed bytes under an expected receipt fail closed.
#[cfg(feature = "rust_inspect")]
pub fn select_current_debug_project_outputs(
    store: &OvenStore,
    project_root: &Path,
    targets: &[(OvenBakeProjectTarget, PathBuf)],
    source_authority_digest: &str,
    native_target: &str,
    toolchain: &str,
) -> CliResult<Option<Vec<(OvenBakeProjectTarget, OvenStoredProjectOutput)>>> {
    select_debug_project_outputs(
        store,
        project_root,
        targets,
        source_authority_digest,
        native_target,
        toolchain,
        None,
    )
}

/// Read verified local bake receipts before inspecting candidate payloads, retaining exact target lineages.
#[cfg(feature = "rust_inspect")]
fn debug_project_output_expectations(
    project_root: &Path,
    targets: &[(OvenBakeProjectTarget, PathBuf)],
    native_target: &str,
    toolchain: &str,
) -> CliResult<Option<Vec<CurrentDebugProjectOutputExpectation>>> {
    let mut expected = Vec::with_capacity(targets.len());
    for (target, entrypoint) in targets {
        let Some(entrypoint_relative_path) = project_relative_entrypoint(project_root, entrypoint) else {
            return Ok(None);
        };
        let target_identity = oven_bake_project_target_identity(project_root, *target, entrypoint)?;
        let receipt_path = project_bake_receipt_path(project_root, *target, entrypoint, "debug")?;
        let receipt = match fs::read(&receipt_path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<oven_store::OvenReceipt>(&bytes).ok())
        {
            Some(receipt)
                if receipt.verify_identity().is_ok()
                    && receipt.intent.profile == "debug"
                    && receipt.intent.target == native_target
                    && receipt.intent.toolchain == toolchain =>
            {
                receipt
            }
            Some(_) | None => return Ok(None),
        };
        expected.push(CurrentDebugProjectOutputExpectation {
            target: *target,
            target_identity,
            entrypoint_relative_path,
            receipt,
        });
    }
    Ok(Some(expected))
}

/// Select local bake lineages for exact replay or fresh development compilation against unchanged dependencies.
#[cfg(feature = "rust_inspect")]
fn select_debug_project_outputs(
    store: &OvenStore,
    project_root: &Path,
    targets: &[(OvenBakeProjectTarget, PathBuf)],
    source_authority_digest: &str,
    native_target: &str,
    toolchain: &str,
    dependency_authority_digest: Option<&str>,
) -> CliResult<Option<Vec<(OvenBakeProjectTarget, OvenStoredProjectOutput)>>> {
    let project_identity = baked_project_owner_identity(project_root)?;
    let Some(expected) = debug_project_output_expectations(project_root, targets, native_target, toolchain)? else {
        return Ok(None);
    };
    let candidates = store
        .select_payloads_matching_for_execution(
            |manifest| {
                manifest.kind == OvenArtifactKind::ProjectOutput
                    && manifest.intent.profile == "debug"
                    && manifest.intent.target == native_target
                    && manifest.intent.toolchain == toolchain
                    && expected.iter().any(|expected| {
                        manifest.receipt_identity == expected.receipt.identity
                            && manifest.build_unit_identity == expected.receipt.build_unit_identity
                            && manifest.intent == expected.receipt.intent
                    })
            },
            |_, _| true,
        )
        .map_err(|error| CliError::failure(format!("failed to select Oven project-output Loafs: {error}")))?;
    let mut grouped = (0..expected.len()).map(|_| Vec::new()).collect::<Vec<_>>();
    for candidate in candidates {
        let matching = expected
            .iter()
            .enumerate()
            .filter(|(_, expected)| {
                candidate.manifest().receipt_identity == expected.receipt.identity
                    && candidate.manifest().build_unit_identity == expected.receipt.build_unit_identity
                    && candidate.manifest().intent == expected.receipt.intent
            })
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        match matching.as_slice() {
            [] => {
                // Do not decode unrelated output payloads. Their leases are dropped after this one in-memory scan.
            }
            [index] => grouped[*index].push(candidate),
            _ => {
                return Err(CliError::failure(
                    "baked debug targets unexpectedly share one project-output receipt lineage",
                ));
            }
        }
    }

    let mut groups = Vec::with_capacity(expected.len());
    for (expected, mut candidates) in expected.into_iter().zip(grouped) {
        if candidates.is_empty() {
            return Ok(None);
        }
        candidates.sort_by(|left, right| left.manifest().identity.cmp(&right.manifest().identity));
        let mut exact = Vec::with_capacity(candidates.len());
        let mut rejected = Vec::new();
        for candidate in candidates {
            let payload = match serde_json::from_slice::<OvenProjectOutputPayload>(candidate.payload()) {
                Ok(payload) => payload,
                Err(error) => {
                    rejected.push(CliError::failure(format!(
                        "exact debug {} project-output Loaf `{}` has an invalid payload: {error}",
                        expected.target.as_str(),
                        candidate.manifest().identity
                    )));
                    continue;
                }
            };
            if payload.project_target != expected.target.as_str()
                || payload.target_identity != expected.target_identity
                || payload.project_identity != project_identity
                || (payload.source_authority_digest != source_authority_digest
                    && !dependency_authority_digest
                        .is_some_and(|digest| payload.dependency_authority_digest.as_deref() == Some(digest)))
                || payload.compiler_version != INCAN_VERSION
                || payload.entrypoint_relative_path != expected.entrypoint_relative_path
                || payload.receipt_identity != expected.receipt.identity
                || payload.build_unit_identity != expected.receipt.build_unit_identity
            {
                rejected.push(CliError::failure(format!(
                    "exact debug {} project-output Loaf `{}` disagrees with its source-current lineage; rerun `incan oven bake --project .`",
                    expected.target.as_str(),
                    candidate.manifest().identity
                )));
                continue;
            }
            let (manifest, artifact_root, _payload, lease) = candidate.into_parts();
            match stored_project_output_from_parts(manifest, artifact_root, payload, lease) {
                Ok(output) => exact.push(output),
                Err(error) => rejected.push(error),
            }
        }
        exact.sort_by(|left, right| left.identity.cmp(&right.identity));
        if exact.is_empty() {
            return Err(rejected.into_iter().next().unwrap_or_else(|| {
                CliError::failure("exact debug project-output lineage disappeared during in-memory selection")
            }));
        }
        groups.push((expected.target, exact));
    }
    select_coherent_project_outputs(groups).map(Some)
}

/// Choose one source-current output per target so that every target names the same project inspection authority.
///
/// A bake that re-seals the project inspection authority for unchanged sources (for example after the store came
/// back from a cache and the generated build-script outputs were discovered again) leaves two exact generations of
/// every output behind. Each generation is coherent on its own, but the identity-smallest output per target can
/// belong to different generations, and `incan test` then refuses the mixed set as a disagreement. Prefer an
/// authority that every target's exact outputs share; when there is none, keep the identity-smallest output per
/// target so the caller reports the disagreement exactly as before.
fn select_coherent_project_outputs(
    groups: Vec<(OvenBakeProjectTarget, Vec<OvenStoredProjectOutput>)>,
) -> CliResult<Vec<(OvenBakeProjectTarget, OvenStoredProjectOutput)>> {
    let shared_authority = groups.first().and_then(|(_, outputs)| {
        outputs
            .iter()
            .filter_map(|output| output.payload.inspection_authority.as_ref())
            .find(|authority| {
                groups.iter().all(|(_, candidates)| {
                    candidates
                        .iter()
                        .any(|candidate| candidate.payload.inspection_authority.as_ref() == Some(*authority))
                })
            })
            .cloned()
    });
    groups
        .into_iter()
        .map(|(target, outputs)| {
            // Each group is sorted by identity and nonempty; the shared authority, when there is one, names the
            // generation to take from every group.
            let position = shared_authority
                .as_ref()
                .and_then(|authority| {
                    outputs
                        .iter()
                        .position(|output| output.payload.inspection_authority.as_ref() == Some(authority))
                })
                .unwrap_or(0);
            outputs
                .into_iter()
                .nth(position)
                .map(|selected| (target, selected))
                .ok_or_else(|| {
                    CliError::failure("exact debug project-output lineage disappeared during in-memory selection")
                })
        })
        .collect()
}

/// Load immutable Rust-inspection lineage from local baked debug outputs for development compilation.
///
/// This is command-scoped authority preparation for check, run and test. Own-source edits may reuse dependency
/// authority recorded by the same local bake; native output replay still requires exact source authority. The caller
/// retains both completed-output and inspection-entry leases across all scheduled harness batches.
/// Missing target output is a cache miss; nonempty malformed or absent lineage is an explicit rebake error.
#[cfg(feature = "rust_inspect")]
pub fn load_current_project_registry_source_authorities(
    store: &OvenStore,
    project_root: &Path,
) -> CliResult<Option<OvenLoadedProjectInspectionAuthority>> {
    let targets = discover_oven_bake_project_targets(project_root)?;
    let source_authority_digest = digest_baked_project_source_authority(project_root)?;
    let rustc = resolve_active_rustc().map_err(|error| CliError::failure(error.to_string()))?;
    let target = rustc_host_target(&rustc).map_err(|error| CliError::failure(error.to_string()))?;
    let toolchain = rustc_identity(&rustc).map_err(|error| CliError::failure(error.to_string()))?;
    let dependency_authority_digest = super::source_authority::digest_project_development_dependencies(project_root)?;
    let Some(outputs) = select_debug_project_outputs(
        store,
        project_root,
        &targets,
        &source_authority_digest,
        &target,
        &toolchain,
        Some(&dependency_authority_digest),
    )?
    else {
        return Ok(None);
    };
    let preferred = outputs
        .iter()
        .find(|(project_target, _)| *project_target == OvenBakeProjectTarget::Library)
        .or_else(|| outputs.first())
        .ok_or_else(|| CliError::failure("Oven project target discovery returned no baked target"))?;
    let authority_ref = preferred.1.payload.inspection_authority.clone().ok_or_else(|| {
        CliError::failure(
            "source-current Oven project output has no project inspection authority; rerun `incan oven bake --project .`",
        )
    })?;
    if outputs.iter().any(|(_, output)| {
        output.payload.inspection_authority.as_ref() != Some(&authority_ref)
            || output.payload.project_identity != preferred.1.payload.project_identity
            || output.payload.source_authority_digest != preferred.1.payload.source_authority_digest
            || output.payload.compiler_version != preferred.1.payload.compiler_version
    }) {
        return Err(CliError::failure(
            "source-current debug project outputs disagree on their singular Rust inspection authority; rerun `incan oven bake --project .`",
        ));
    }
    let mut authority = super::inspection_snapshot::load_output_inspection_authority(store, &preferred.1)
        .map_err(|error| CliError::failure(error.to_string()))?;
    let output_leases = outputs.into_iter().map(|(_, output)| output._lease).collect();
    authority.retain_lineage_leases(output_leases);
    Ok(Some(authority))
}

/// Visit only compiler-owned filesystem fields in a serialized build report.
///
/// Semantic strings such as package names, features, imports, and notes deliberately never enter this traversal. A
/// tagged portable path therefore cannot collide with authored text that happens to resemble an internal token.
fn transform_project_output_report_paths(
    report: &mut serde_json::Value,
    mut transform: impl FnMut(&mut serde_json::Value) -> CliResult<()>,
) -> CliResult<()> {
    for pointer in [
        "/project/project_root",
        "/entrypoint",
        "/library_root",
        "/generated/project_path",
        "/generated/manifest_path",
        "/generated/crate_root",
        "/generated/cargo_target_dir",
        "/generated/oven_output_dir",
    ] {
        if let Some(value) = report.pointer_mut(pointer) {
            transform(value)?;
        }
    }

    for (pointer, fields) in [
        ("/source_files", &["path"][..]),
        ("/artifacts", &["path"][..]),
        ("/dependencies/incan", &["path"][..]),
        ("/semantic/packages", &["project_root"][..]),
        ("/semantic/feature_edges", &["from", "to"][..]),
    ] {
        let Some(values) = report.pointer_mut(pointer) else {
            continue;
        };
        let values = values.as_array_mut().ok_or_else(|| {
            CliError::failure(format!("completed Oven build report field `{pointer}` is not an array"))
        })?;
        for value in values {
            let object = value.as_object_mut().ok_or_else(|| {
                CliError::failure(format!(
                    "completed Oven build report field `{pointer}` contains a non-object row"
                ))
            })?;
            for field in fields {
                if let Some(path) = object.get_mut(*field) {
                    transform(path)?;
                }
            }
        }
    }

    for pointer in ["/dependencies/rust", "/dependencies/rust_dev"] {
        let Some(values) = report.pointer_mut(pointer) else {
            continue;
        };
        let values = values.as_array_mut().ok_or_else(|| {
            CliError::failure(format!("completed Oven build report field `{pointer}` is not an array"))
        })?;
        for value in values {
            let object = value.as_object_mut().ok_or_else(|| {
                CliError::failure(format!(
                    "completed Oven build report field `{pointer}` contains a non-object row"
                ))
            })?;
            if object.get("source").and_then(serde_json::Value::as_str) == Some("path")
                && let Some(path) = object.get_mut("source_detail")
            {
                transform(path)?;
            }
        }
    }

    if let Some(values) = report.pointer_mut("/semantic/providers") {
        let values = values.as_array_mut().ok_or_else(|| {
            CliError::failure("completed Oven build report field `/semantic/providers` is not an array")
        })?;
        for value in values {
            let object = value.as_object_mut().ok_or_else(|| {
                CliError::failure("completed Oven build report field `/semantic/providers` contains a non-object row")
            })?;
            if let Some(path) = object.get_mut("manifest_path") {
                transform(path)?;
            }
            if let Some(provenance) = object.get_mut("provenance") {
                let provenance = provenance.as_object_mut().ok_or_else(|| {
                    CliError::failure("completed Oven build report provider provenance is not an object")
                })?;
                for field in ["manifest_path", "inventory_path"] {
                    if let Some(path) = provenance.get_mut(field) {
                        transform(path)?;
                    }
                }
            }
        }
    }
    Ok(())
}

/// Replace one absolute report path with a tagged portable project path or opaque external-authority slot.
fn seal_project_output_report_path(
    value: &mut serde_json::Value,
    project_root: &Path,
    external_paths: &mut BTreeMap<String, u64>,
) -> CliResult<()> {
    if value.is_null() {
        return Ok(());
    }
    let text = value
        .as_str()
        .ok_or_else(|| CliError::failure("completed Oven build report path field is not a string"))?;
    let path = Path::new(text);
    if !path.is_absolute() {
        return Ok(());
    }
    if let Ok(relative) = path.strip_prefix(project_root) {
        let relative = relative.to_string_lossy().replace('\\', "/");
        if relative.is_empty() || validated_project_output_relative_path(&relative, "build-report projection").is_ok() {
            *value = serde_json::json!({
                (OVEN_PROJECT_OUTPUT_REPORT_PATH_TAG): {
                    "root": "project",
                    "relative": relative,
                }
            });
            return Ok(());
        }
    }
    let next_slot = u64::try_from(external_paths.len())
        .map_err(|_| CliError::failure("completed Oven build report has too many external path authorities"))?;
    let slot = *external_paths.entry(text.to_string()).or_insert(next_slot);
    *value = serde_json::json!({
        (OVEN_PROJECT_OUTPUT_REPORT_PATH_TAG): {
            "root": "external",
            "slot": slot,
        }
    });
    Ok(())
}

/// Restore one tagged report path after exact completed-output selection.
fn restore_project_output_report_path(value: &mut serde_json::Value, project_root: &Path) -> CliResult<()> {
    if value.is_null() || value.is_string() {
        return Ok(());
    }
    let tagged = value
        .as_object()
        .filter(|object| object.len() == 1)
        .and_then(|object| object.get(OVEN_PROJECT_OUTPUT_REPORT_PATH_TAG))
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| CliError::failure("completed Oven build report path field has an invalid portable tag"))?;
    match tagged.get("root").and_then(serde_json::Value::as_str) {
        Some("project") => {
            let relative = tagged
                .get("relative")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| CliError::failure("completed Oven build report project path has no relative value"))?;
            let restored = if relative.is_empty() {
                project_root.to_path_buf()
            } else {
                project_root.join(validated_project_output_relative_path(
                    relative,
                    "build-report projection",
                )?)
            };
            *value = serde_json::Value::String(restored.to_string_lossy().to_string());
        }
        Some("external") => {
            let slot = tagged
                .get("slot")
                .and_then(serde_json::Value::as_u64)
                .ok_or_else(|| CliError::failure("completed Oven build report external path has no authority slot"))?;
            *value = serde_json::Value::String(format!("{OVEN_PROJECT_OUTPUT_REPORT_EXTERNAL_ROOT}/{slot}"));
        }
        Some(_) | None => {
            return Err(CliError::failure(
                "completed Oven build report path has an unknown portable authority",
            ));
        }
    }
    Ok(())
}

/// Reject an unclassified absolute string before a portable report can be sealed.
fn reject_unclassified_absolute_report_strings(value: &serde_json::Value) -> CliResult<()> {
    match value {
        serde_json::Value::Array(values) => {
            for value in values {
                reject_unclassified_absolute_report_strings(value)?;
            }
        }
        serde_json::Value::Object(values) => {
            for value in values.values() {
                reject_unclassified_absolute_report_strings(value)?;
            }
        }
        serde_json::Value::String(text) if Path::new(text).is_absolute() => {
            return Err(CliError::failure(
                "completed Oven build report contains an unclassified absolute path",
            ));
        }
        serde_json::Value::Null
        | serde_json::Value::Bool(_)
        | serde_json::Value::Number(_)
        | serde_json::Value::String(_) => {}
    }
    Ok(())
}

/// Replace filesystem fields in a sealed build report with collision-proof portable path tags.
pub fn make_project_output_report_portable(value: &mut serde_json::Value, project_root: &Path) -> CliResult<()> {
    let mut external_paths = BTreeMap::new();
    transform_project_output_report_paths(value, |path| {
        seal_project_output_report_path(path, project_root, &mut external_paths)
    })?;
    reject_unclassified_absolute_report_strings(value)
}

/// Restore only tagged project paths after an exact output has been selected; external paths remain logical tokens.
pub fn restore_project_output_report_paths(value: &mut serde_json::Value, project_root: &Path) -> CliResult<()> {
    transform_project_output_report_paths(value, |path| restore_project_output_report_path(path, project_root))
}

/// Seal one complete executable report while keeping caller-owned project paths relocatable.
pub fn project_output_report_snapshot(
    project_root: &Path,
    report: &BuildReport,
) -> CliResult<OvenProjectOutputReportSnapshot> {
    let mut report = serde_json::to_value(report)
        .map_err(|error| CliError::failure(format!("failed to serialize completed Oven build report: {error}")))?;
    make_project_output_report_portable(&mut report, project_root)?;
    Ok(OvenProjectOutputReportSnapshot {
        schema_version: OVEN_PROJECT_OUTPUT_REPORT_SCHEMA_VERSION,
        report,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    use crate::backend::selection::{BackendKind, finalize_receipt, select_backend};
    use crate::build::output_materialization::{
        materialize_project_output, select_current_sealed_project_output, verify_stored_project_output_native,
    };
    use crate::build::publication::{project_output_payload_for_bake, publish_project_output_loaf};
    use crate::build::source_authority::{
        baked_project_lock_dependencies_fingerprint, digest_baked_project_source_authority, project_bake_receipt_path,
    };
    use crate::build::{
        OVEN_PROJECT_OUTPUT_ARTIFACT_PATH, OVEN_PROJECT_OUTPUT_PAYLOAD_SCHEMA_VERSION,
        OVEN_PROJECT_OUTPUT_REPORT_SCHEMA_VERSION, OvenBakeProjectTarget, OvenProjectOutputBakeFile,
        OvenProjectOutputBakeRequest, OvenProjectOutputFile, OvenProjectOutputPayload, OvenProjectOutputReportSnapshot,
    };
    use crate::build_report::BUILD_REPORT_SCHEMA_VERSION;
    use incan_frontend::diagnostics;
    use incan_lang::version::INCAN_VERSION;
    use oven_rustc::rustc::{
        OvenProjectInspectionAuthorityRef, resolve_active_rustc, rustc_host_target, rustc_identity,
    };
    use oven_store::store::{OvenArtifactKind, OvenArtifactMaterializedFile, OvenArtifactPublishRequest, OvenStore};
    use oven_store::{OvenGeneratedProjectRequest, digest_bytes, receipt_generated_project, write_receipt};
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    /// Count real catalog queries in one controlled selection without production counters or another selector.
    #[derive(Clone, Default)]
    struct CatalogQueries {
        calls: std::sync::Arc<std::sync::atomic::AtomicUsize>,
        sequence: std::sync::Arc<std::sync::atomic::AtomicU64>,
    }

    impl tracing::Subscriber for CatalogQueries {
        /// Enable the existing invocation spans in this thread's controlled dispatch.
        fn enabled(&self, _metadata: &tracing::Metadata<'_>) -> bool {
            true
        }

        /// Count actual index lookups, assigning a unique ID to every observed span.
        fn new_span(&self, attributes: &tracing::span::Attributes<'_>) -> tracing::span::Id {
            use std::sync::atomic::Ordering;
            if attributes.metadata().name() == "oven_store::lookup_payload_index" {
                self.calls.fetch_add(1, Ordering::Relaxed);
            }
            tracing::span::Id::from_u64(self.sequence.fetch_add(1, Ordering::Relaxed) + 1)
        }

        /// Field updates do not represent another catalog query.
        fn record(&self, _span: &tracing::span::Id, _values: &tracing::span::Record<'_>) {}
        /// Causal links do not represent catalog queries.
        fn record_follows_from(&self, _span: &tracing::span::Id, _follows: &tracing::span::Id) {}
        /// Events are excluded from invocation counts.
        fn event(&self, _event: &tracing::Event<'_>) {}
        /// Entering an existing span is not a new invocation.
        fn enter(&self, _span: &tracing::span::Id) {}
        /// Exiting an existing span does not change invocation counts.
        fn exit(&self, _span: &tracing::span::Id) {}
    }

    /// Both ordinary and compiler-client selection use an exact locator without querying unrelated owners.
    #[test]
    fn ordinary_project_output_locator_avoids_unrelated_manifest_walk() -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        fs::create_dir(project.path().join("src"))?;
        fs::write(project.path().join("loaf.toml"), "[project]\nname = 'fixture'\n")?;
        let entrypoint = project.path().join("src/main.incn");
        fs::write(&entrypoint, "def main() -> None:\n    pass\n")?;
        let (receipt, mut payload, files) =
            crate::build::test_support::fixture_project_output_publication(project.path(), "debug", "located")?;
        payload.inspection_authority = None;
        payload.preparation_identity_digest =
            Some(super::super::source_authority::ordinary_preparation_identity_digest()?);
        let compiler_identity = digest_bytes(b"fixture compiler");
        let dependency_identity = digest_bytes(b"fixture dependencies");
        payload.compiler_identity_digest = Some(compiler_identity.clone());
        payload.dependency_authority_digest = Some(dependency_identity.clone());
        let store_root = tempfile::tempdir()?;
        let store = OvenStore::new(
            store_root.path(),
            oven_store::store::OvenStoreLimits::new(1024 * 1024, 1024 * 1024, 1024 * 1024),
        );
        let output = publish_project_output_loaf(&store, &receipt, &payload, &files)?;
        let receipt_path =
            project_bake_receipt_path(project.path(), OvenBakeProjectTarget::Executable, &entrypoint, "debug")?;
        fs::create_dir_all(receipt_path.parent().ok_or("receipt has no parent")?)?;
        fs::write(
            receipt_path.with_extension("output.json"),
            serde_json::to_vec(&output.identity)?,
        )?;

        // A full Store walk would encounter this damaged, unrelated entry. Exact admission must never read it.
        let unrelated = store.root().join("entries").join(format!("sha256-{}", "0".repeat(64)));
        fs::create_dir(&unrelated)?;
        fs::write(unrelated.join("artifact.json"), b"not a manifest")?;
        let selected = select_baked_project_output(
            &store,
            project.path(),
            &entrypoint,
            OvenBakeProjectTarget::Executable,
            "debug",
        )?
        .ok_or("exact ordinary output locator missed")?;
        assert_eq!(selected.identity, output.identity);
        let select = |compiler: &str, dependencies: &str, target: &str, toolchain: &str| {
            super::super::native_runtime_inputs::select_project_output_for_incan(
                &store,
                project.path(),
                &entrypoint,
                OvenBakeProjectTarget::Executable,
                "debug",
                &payload.source_authority_digest,
                target,
                toolchain,
                compiler,
                dependencies,
            )
        };
        let queries = CatalogQueries::default();
        let selected = tracing::subscriber::with_default(queries.clone(), || {
            select(
                &compiler_identity,
                &dependency_identity,
                &receipt.intent.target,
                &receipt.intent.toolchain,
            )
        })?
        .ok_or("exact compiler-client output locator missed")?;
        assert_eq!(selected.identity, output.identity);
        assert_eq!(
            queries.calls.load(std::sync::atomic::Ordering::Relaxed),
            0,
            "a compiler client with an exact existing output locator must not query the Store catalog"
        );
        fs::remove_dir_all(&unrelated)?;

        // ---- Current authority: every bound dimension rejects a stale candidate ----
        let other = digest_bytes(b"other generation");
        for (compiler, dependencies, target, toolchain) in [
            (
                other.as_str(),
                dependency_identity.as_str(),
                receipt.intent.target.as_str(),
                receipt.intent.toolchain.as_str(),
            ),
            (
                compiler_identity.as_str(),
                other.as_str(),
                receipt.intent.target.as_str(),
                receipt.intent.toolchain.as_str(),
            ),
            (
                compiler_identity.as_str(),
                dependency_identity.as_str(),
                "other-target",
                receipt.intent.toolchain.as_str(),
            ),
            (
                compiler_identity.as_str(),
                dependency_identity.as_str(),
                receipt.intent.target.as_str(),
                "other-toolchain",
            ),
        ] {
            assert!(select(compiler, dependencies, target, toolchain)?.is_none());
        }

        // ---- Fallback: a wrong generation at the locator must not hide an exact published candidate ----
        for stale_compiler in [true, false] {
            let mut stale = payload.clone();
            if stale_compiler {
                stale.compiler_identity_digest = Some(other.clone());
            } else {
                stale.dependency_authority_digest = Some(other.clone());
            }
            let stale = publish_project_output_loaf(&store, &receipt, &stale, &files)?;
            fs::write(
                receipt_path.with_extension("output.json"),
                serde_json::to_vec(&stale.identity)?,
            )?;
            assert_eq!(
                select(
                    &compiler_identity,
                    &dependency_identity,
                    &receipt.intent.target,
                    &receipt.intent.toolchain
                )?
                .ok_or("stale locator hid the exact compiler/dependency output")?
                .identity,
                output.identity
            );
        }
        fs::remove_file(receipt_path.with_extension("output.json"))?;
        assert_eq!(
            select(
                &compiler_identity,
                &dependency_identity,
                &receipt.intent.target,
                &receipt.intent.toolchain
            )?
            .ok_or("missing locator prevented exact fallback")?
            .identity,
            output.identity
        );
        fs::write(receipt_path.with_extension("output.json"), b"\"../../foreign\"")?;
        assert_eq!(
            select(
                &compiler_identity,
                &dependency_identity,
                &receipt.intent.target,
                &receipt.intent.toolchain
            )?
            .ok_or("malformed locator prevented exact fallback")?
            .identity,
            output.identity
        );
        assert_eq!(
            select_baked_project_output(
                &store,
                project.path(),
                &entrypoint,
                OvenBakeProjectTarget::Executable,
                "debug",
            )?
            .ok_or("malformed hint prevented fallback")?
            .identity,
            output.identity
        );
        Ok(())
    }

    /// Considering historical compiler binaries and other source generations must not make them hot cache entries.
    #[test]
    fn project_output_lookup_does_not_refresh_rejected_payloads() -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        fs::create_dir(project.path().join("src"))?;
        fs::write(project.path().join("loaf.toml"), "[project]\nname = 'fixture'\n")?;
        fs::write(project.path().join("src/main.incn"), "def main() -> None:\n    pass\n")?;
        let (receipt, payload, files) =
            crate::build::test_support::fixture_project_output_publication(project.path(), "debug", "lru")?;
        let store_root = tempfile::tempdir()?;
        let store = OvenStore::new(
            store_root.path(),
            oven_store::store::OvenStoreLimits::new(1024 * 1024, 1024 * 1024, 1024 * 1024),
        );
        let current = publish_project_output_loaf(&store, &receipt, &payload, &files)?;
        let mut stale_payload = payload.clone();
        stale_payload.source_authority_digest = digest_bytes(b"older source generation");
        let stale = publish_project_output_loaf(&store, &receipt, &stale_payload, &files)?;
        let compiler = store.publish(&OvenArtifactPublishRequest {
            receipt,
            domain: format!("incan-release-{INCAN_VERSION}"),
            kind: OvenArtifactKind::ProjectOutput,
            payload: br#"{"schema":"incan.native-compiler-output/1","unit":"historical-compiler"}"#.to_vec(),
            materialized_files: Vec::new(),
            materialized_directories: Vec::new(),
        })?;
        let (_, compiler_root, _, compiler_lease) = store.select_payload_for_execution(&compiler.identity)?;
        let access = |root: &Path| -> Result<PathBuf, Box<dyn std::error::Error>> {
            Ok(root.parent().ok_or("artifact root has no parent")?.join("last-used"))
        };
        let current_access = access(&current.artifact_root)?;
        let stale_access = access(&stale.artifact_root)?;
        let compiler_access = access(&compiler_root)?;
        for path in [&current_access, &stale_access, &compiler_access] {
            fs::write(path, b"7\n")?;
        }
        let selected = matching_baked_project_outputs_with_source_authority(
            &store,
            project.path(),
            &project.path().join("src/main.incn"),
            OvenBakeProjectTarget::Executable,
            "debug",
            &payload.source_authority_digest,
            None,
        )?;
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].identity, current.identity);
        assert_ne!(fs::read(&current_access)?, b"7\n");
        assert_eq!(
            fs::read(&compiler_access)?,
            b"7\n",
            "rejected historical compiler binary became hot"
        );
        assert_eq!(
            fs::read(&stale_access)?,
            b"7\n",
            "rejected source generation became hot"
        );
        drop(compiler_lease);
        Ok(())
    }

    /// Own-source edits reuse only the inspection lineage; changed manifest authority remains refused.
    #[test]
    fn development_selection_preserves_dependency_authority() -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        fs::create_dir(project.path().join("src"))?;
        fs::write(project.path().join("loaf.toml"), "[project]\nname = 'fixture'\n")?;
        fs::write(project.path().join("src/main.incn"), "def main() -> None:\n    pass\n")?;
        let (receipt, payload, files) =
            crate::build::test_support::fixture_project_output_publication(project.path(), "debug", "development")?;
        oven_store::write_receipt(
            &receipt,
            project_bake_receipt_path(
                project.path(),
                OvenBakeProjectTarget::Executable,
                &project.path().join("src/main.incn"),
                "debug",
            )?,
        )?;
        let store_root = tempfile::tempdir()?;
        let store = OvenStore::new(
            store_root.path(),
            oven_store::store::OvenStoreLimits::new(1024 * 1024, 1024 * 1024, 1024 * 1024),
        );
        crate::build::publication::publish_project_output_loaf(&store, &receipt, &payload, &files)?;
        let dependencies = super::super::source_authority::digest_project_development_dependencies(project.path())?;
        fs::write(
            project.path().join("src/main.incn"),
            "def added() -> int:\n    return 42\n\ndef main() -> None:\n    pass\n",
        )?;
        let current = digest_baked_project_source_authority(project.path())?;
        assert_ne!(current, payload.source_authority_digest);
        assert_eq!(
            dependencies,
            super::super::source_authority::digest_project_development_dependencies(project.path())?
        );
        let targets = discover_oven_bake_project_targets(project.path())?;
        assert!(
            select_debug_project_outputs(
                &store,
                project.path(),
                &targets,
                &current,
                &receipt.intent.target,
                &receipt.intent.toolchain,
                Some(&dependencies)
            )?
            .is_some()
        );
        assert!(
            select_current_debug_project_outputs(
                &store,
                project.path(),
                &targets,
                &current,
                &receipt.intent.target,
                &receipt.intent.toolchain
            )
            .is_err()
        );
        fs::write(
            project.path().join("loaf.toml"),
            "[project]\nname = 'fixture'\nversion = '2.0.0'\n",
        )?;
        let changed = super::super::source_authority::digest_project_development_dependencies(project.path())?;
        assert_ne!(dependencies, changed);
        assert!(
            select_debug_project_outputs(
                &store,
                project.path(),
                &targets,
                &current,
                &receipt.intent.target,
                &receipt.intent.toolchain,
                Some(&changed)
            )
            .is_err()
        );
        Ok(())
    }

    /// A requested ordinary library cannot reuse an SDK-era output with no preparation identity.
    #[test]
    fn ordinary_library_selection_requires_its_preparation_route() -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        fs::create_dir(project.path().join("src"))?;
        fs::write(project.path().join("loaf.toml"), "[project]\nname = \"fixture\"\n")?;
        fs::write(project.path().join("src/main.incn"), "def main() -> None:\n    pass\n")?;
        let (_, mut payload, _) =
            crate::build::test_support::fixture_project_output_publication(project.path(), "debug", "route")?;
        payload.preparation_identity_digest = None;
        assert!(project_output_preparation_matches(&payload, Some("ordinary"), false));
        assert!(!project_output_preparation_matches(&payload, Some("ordinary"), true));
        payload.preparation_identity_digest = Some("ordinary".into());
        assert!(project_output_preparation_matches(&payload, Some("ordinary"), true));
        assert!(!project_output_preparation_matches(&payload, Some("other route"), true));
        assert!(!project_output_preparation_matches(&payload, None, true));
        Ok(())
    }

    /// An ordinary output shares the completed-loaf selector but cannot reuse another preparation configuration.
    #[test]
    fn ordinary_project_output_requires_exact_preparation_identity() -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        fs::create_dir(project.path().join("src"))?;
        fs::write(project.path().join("loaf.toml"), "[project]\nname = \"fixture\"\n")?;
        fs::write(project.path().join("src/main.incn"), "def main() -> None:\n    pass\n")?;
        let (receipt, mut payload, files) =
            crate::build::test_support::fixture_project_output_publication(project.path(), "release", "ordinary")?;
        let store_root = tempfile::tempdir()?;
        let store = OvenStore::new(
            store_root.path(),
            oven_store::store::OvenStoreLimits::new(1024 * 1024, 1024 * 1024, 1024 * 1024),
        );
        payload.inspection_authority = None;
        assert!(publish_project_output_loaf(&store, &receipt, &payload, &files).is_err());
        payload.preparation_identity_digest = Some("sha256:other-preparation".into());
        publish_project_output_loaf(&store, &receipt, &payload, &files)?;
        let entrypoint = project.path().join("src/main.incn");
        assert!(
            select_baked_project_output(
                &store,
                project.path(),
                &entrypoint,
                OvenBakeProjectTarget::Executable,
                "release"
            )?
            .is_none()
        );
        payload.preparation_identity_digest =
            Some(super::super::source_authority::ordinary_preparation_identity_digest()?);
        publish_project_output_loaf(&store, &receipt, &payload, &files)?;
        assert!(
            select_baked_project_output(
                &store,
                project.path(),
                &entrypoint,
                OvenBakeProjectTarget::Executable,
                "release"
            )?
            .is_some()
        );
        fs::write(&entrypoint, "def main() -> None:\n    println(1)\n")?;
        assert!(
            select_baked_project_output(
                &store,
                project.path(),
                &entrypoint,
                OvenBakeProjectTarget::Executable,
                "release"
            )?
            .is_none()
        );
        Ok(())
    }

    #[test]
    fn project_output_loaf_selects_only_the_exact_authored_project_before_frontend_work()
    -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let entrypoint = project.path().join("src/main.incn");
        let generated_source = project.path().join("generated/main.rs");
        let native_output = project.path().join("publisher/fixture");
        fs::create_dir_all(entrypoint.parent().ok_or("entrypoint has no parent")?)?;
        fs::create_dir_all(generated_source.parent().ok_or("generated source has no parent")?)?;
        fs::create_dir_all(native_output.parent().ok_or("native output has no parent")?)?;
        fs::write(project.path().join("loaf.toml"), "[project]\nname = \"fixture\"\n")?;
        fs::write(&entrypoint, "def main() -> None:\n    pass\n")?;
        fs::write(&generated_source, "fn main() {}\n")?;
        fs::write(&native_output, "fixture native output")?;

        let rustc = resolve_active_rustc()?;
        let receipt = receipt_generated_project(
            &OvenGeneratedProjectRequest::new(
                project.path(),
                "fixture",
                "0.1.0",
                rustc_host_target(&rustc)?,
                rustc_identity(&rustc)?,
                "release",
                Vec::new(),
            )
            .with_generated_source("generated-root", &generated_source)
            .with_build_unit_input("compiler-version", INCAN_VERSION),
        )?;
        let store_root = tempfile::tempdir()?;
        let store = OvenStore::new(
            store_root.path(),
            oven_store::store::OvenStoreLimits::new(1024 * 1024, 1024 * 1024, 1024 * 1024),
        );
        let payload = OvenProjectOutputPayload {
            schema_version: OVEN_PROJECT_OUTPUT_PAYLOAD_SCHEMA_VERSION,
            project_target: OvenBakeProjectTarget::Executable.as_str().to_string(),
            target_identity: OvenBakeProjectTarget::Executable.as_str().to_string(),
            project_identity: baked_project_owner_identity(project.path())?,
            source_authority_digest: digest_baked_project_source_authority(project.path())?,
            dependency_authority_digest: None,
            lock_dependencies_fingerprint: baked_project_lock_dependencies_fingerprint(project.path())?,
            compiler_identity_digest: None,
            preparation_identity_digest: None,
            compiler_version: INCAN_VERSION.to_string(),
            entrypoint_relative_path: "src/main.incn".to_string(),
            build_unit_identity: receipt.build_unit_identity.clone(),
            receipt_identity: receipt.identity.clone(),
            plan_identity: "fixture-plan".to_string(),
            inspection_authority: Some(OvenProjectInspectionAuthorityRef {
                identity: "fixture-inspection-authority".to_string(),
                receipt_identity: receipt.identity.clone(),
                build_unit_identity: receipt.build_unit_identity.clone(),
            }),
            inspection_authority_snapshot: None,
            files: vec![
                OvenProjectOutputFile {
                    caller_relative_path: "generated/main.rs".to_string(),
                    output_relative_path: "generated/main.rs".to_string(),
                    digest: digest_bytes(&fs::read(&generated_source)?),
                    logical_bytes: fs::metadata(&generated_source)?.len(),
                },
                OvenProjectOutputFile {
                    caller_relative_path: "target/incan/fixture/oven/release/fixture".to_string(),
                    output_relative_path: OVEN_PROJECT_OUTPUT_ARTIFACT_PATH.to_string(),
                    digest: digest_bytes(&fs::read(&native_output)?),
                    logical_bytes: fs::metadata(&native_output)?.len(),
                },
            ],
            required_project_loafs: Vec::new(),
            package_loaf_store_relative_path: None,
            backend_receipt: finalize_receipt(
                &select_backend(BackendKind::Legacy, "sha256:fixture-source"),
                "sha256:fixture-output",
                diagnostics::DIAGNOSTIC_SCHEMA_VERSION,
            )?,
            build_report: None,
        };
        let files = vec![
            OvenProjectOutputBakeFile {
                source_path: generated_source.clone(),
                caller_relative_path: "generated/main.rs".to_string(),
                output_relative_path: "generated/main.rs".to_string(),
            },
            OvenProjectOutputBakeFile {
                source_path: native_output.clone(),
                caller_relative_path: "target/incan/fixture/oven/release/fixture".to_string(),
                output_relative_path: OVEN_PROJECT_OUTPUT_ARTIFACT_PATH.to_string(),
            },
        ];
        let inconsistent = project_output_payload_for_bake(OvenProjectOutputBakeRequest {
            project_root: project.path(),
            entrypoint: &entrypoint,
            target: OvenBakeProjectTarget::Executable,
            receipt: &receipt,
            plan_identity: "fixture-plan".to_string(),
            profile: "release",
            source_authority_digest: &payload.source_authority_digest,
            lock_dependencies_fingerprint: payload.lock_dependencies_fingerprint.clone(),
            files: files.clone(),
            inspection_authority: Some(OvenProjectInspectionAuthorityRef {
                identity: String::new(),
                receipt_identity: receipt.identity.clone(),
                build_unit_identity: receipt.build_unit_identity.clone(),
            }),
            required_project_loafs: Vec::new(),
            package_loaf_store_relative_path: None,
            backend_receipt: payload.backend_receipt.clone(),
            build_report: None,
        });
        let Err(inconsistent) = inconsistent else {
            return Err("empty project inspection authority was accepted".into());
        };
        assert!(inconsistent.message.contains("one exact project inspection authority"));

        let mut current_payload = payload.clone();
        current_payload.lock_dependencies_fingerprint = Some("sha256:fixture-lock-fingerprint".to_string());
        current_payload.build_report = Some(OvenProjectOutputReportSnapshot {
            schema_version: OVEN_PROJECT_OUTPUT_REPORT_SCHEMA_VERSION,
            report: serde_json::json!({ "schema_version": BUILD_REPORT_SCHEMA_VERSION }),
        });
        let current_roundtrip =
            serde_json::from_slice::<OvenProjectOutputPayload>(&serde_json::to_vec(&current_payload)?)?;
        assert_eq!(current_roundtrip.target_identity, current_payload.target_identity);
        assert_eq!(
            current_roundtrip.lock_dependencies_fingerprint,
            current_payload.lock_dependencies_fingerprint
        );
        assert_eq!(current_roundtrip.build_report, current_payload.build_report);
        assert_eq!(
            current_roundtrip.inspection_authority,
            current_payload.inspection_authority
        );

        let mut stale_payload = payload.clone();
        stale_payload.schema_version = OVEN_PROJECT_OUTPUT_PAYLOAD_SCHEMA_VERSION - 1;
        let mut stale_payload_value = serde_json::to_value(&stale_payload)?;
        let stale_payload_object = stale_payload_value
            .as_object_mut()
            .ok_or("serialized project output payload was not an object")?;
        stale_payload_object.remove("target_identity");
        stale_payload_object.remove("lock_dependencies_fingerprint");
        stale_payload_object.remove("build_report");
        let _stale = store.publish(&OvenArtifactPublishRequest {
            receipt: receipt.clone(),
            domain: format!("incan-release-{INCAN_VERSION}"),
            kind: OvenArtifactKind::ProjectOutput,
            payload: serde_json::to_vec(&stale_payload_value)?,
            materialized_files: files
                .iter()
                .map(|file| OvenArtifactMaterializedFile {
                    source_path: file.source_path.clone(),
                    relative_path: file.output_relative_path.clone(),
                })
                .collect(),
            materialized_directories: Vec::new(),
        })?;
        assert!(
            select_baked_project_output(
                &store,
                project.path(),
                &entrypoint,
                OvenBakeProjectTarget::Executable,
                "release",
            )?
            .is_none(),
            "a completed output from the previous project-output release-cohort schema must not be replayed"
        );
        publish_project_output_loaf(&store, &receipt, &payload, &files)?;

        // A second explicit publication can arise after an interrupted caller projection. Its byte-level native output
        // need not share the first content address, but it is governed by the same complete source and direct-rustc
        // authority, so normal selection must remain usable.
        let duplicate_native_output = project.path().join("publisher/fixture-republished");
        fs::write(&duplicate_native_output, "republished fixture native output")?;
        let mut duplicate_payload = payload.clone();
        let duplicate_native = duplicate_payload
            .files
            .iter_mut()
            .find(|file| file.output_relative_path == OVEN_PROJECT_OUTPUT_ARTIFACT_PATH)
            .ok_or("duplicate project output should retain a native artifact")?;
        duplicate_native.digest = digest_bytes(&fs::read(&duplicate_native_output)?);
        duplicate_native.logical_bytes = fs::metadata(&duplicate_native_output)?.len();
        let duplicate_files = vec![
            OvenProjectOutputBakeFile {
                source_path: generated_source.clone(),
                caller_relative_path: "generated/main.rs".to_string(),
                output_relative_path: "generated/main.rs".to_string(),
            },
            OvenProjectOutputBakeFile {
                source_path: duplicate_native_output,
                caller_relative_path: "target/incan/fixture/oven/release/fixture".to_string(),
                output_relative_path: OVEN_PROJECT_OUTPUT_ARTIFACT_PATH.to_string(),
            },
        ];
        publish_project_output_loaf(&store, &receipt, &duplicate_payload, &duplicate_files)?;

        let source_authority_digest = digest_baked_project_source_authority(project.path())?;
        assert!(
            select_baked_project_output_with_source_authority(
                &store,
                project.path(),
                &entrypoint,
                OvenBakeProjectTarget::Executable,
                "release",
                &source_authority_digest,
                Some((&receipt.intent.target, "rustc incompatible fixture")),
            )?
            .is_none(),
            "a completed output from another Rust toolchain must not be replayed"
        );
        assert!(
            select_baked_project_output_with_source_authority(
                &store,
                project.path(),
                &entrypoint,
                OvenBakeProjectTarget::Executable,
                "release",
                &source_authority_digest,
                Some((&receipt.intent.target, &receipt.intent.toolchain)),
            )?
            .is_some(),
            "the exact target/toolchain output should remain selectable"
        );

        let selected = select_baked_project_output(
            &store,
            project.path(),
            &entrypoint,
            OvenBakeProjectTarget::Executable,
            "release",
        )?
        .ok_or("exact project output Loaf should be selected")?;
        assert!(selected.payload == payload || selected.payload == duplicate_payload);
        let selected_native = fs::read(&selected.native_output)?;
        assert!(selected_native == b"fixture native output" || selected_native == b"republished fixture native output");
        let original_permissions = fs::metadata(&selected.native_output)?.permissions();
        #[cfg(unix)]
        fs::set_permissions(&selected.native_output, fs::Permissions::from_mode(0o644))?;
        #[cfg(not(unix))]
        {
            let mut writable = original_permissions.clone();
            writable.set_readonly(false);
            fs::set_permissions(&selected.native_output, writable)?;
        }
        let mut corrupted_native = selected_native.clone();
        let Some(first) = corrupted_native.first_mut() else {
            return Err("fixture native output must not be empty".into());
        };
        *first ^= 0xff;
        fs::write(&selected.native_output, &corrupted_native)?;
        let Err(error) = verify_stored_project_output_native(&selected) else {
            return Err("normal run accepted same-length corruption of its store-owned executable".into());
        };
        assert!(error.message.contains("digest differs"));
        fs::write(&selected.native_output, &selected_native)?;
        fs::set_permissions(&selected.native_output, original_permissions)?;
        verify_stored_project_output_native(&selected)?;
        fs::remove_file(&generated_source)?;
        materialize_project_output(project.path(), &selected)?;
        assert_eq!(fs::read(&generated_source)?, b"fn main() {}\n");

        fs::write(&entrypoint, "def main() -> None:\n    return\n")?;
        assert!(
            select_baked_project_output(
                &store,
                project.path(),
                &entrypoint,
                OvenBakeProjectTarget::Executable,
                "release",
            )?
            .is_none()
        );
        let receipt_path = project_bake_receipt_path(
            project.path(),
            OvenBakeProjectTarget::Executable,
            &entrypoint,
            "release",
        )?;
        write_receipt(&receipt, &receipt_path)?;
        assert!(has_stale_baked_project_output(
            &store,
            project.path(),
            &entrypoint,
            OvenBakeProjectTarget::Executable,
            "release",
        )?);
        // The stale sealed output is a fact to mention, never a reason to refuse: the edited project takes the
        // source-aware route against the dependency closure the same bake published.
        assert!(
            select_current_sealed_project_output(
                &store,
                project.path(),
                &entrypoint,
                OvenBakeProjectTarget::Executable,
                "release",
            )?
            .is_none(),
            "an edited project with a stale sealed output must fall through to the source route"
        );

        let unrelated = tempfile::tempdir()?;
        let unrelated_entrypoint = unrelated.path().join("src/main.incn");
        fs::create_dir_all(
            unrelated_entrypoint
                .parent()
                .ok_or("unrelated entrypoint has no parent")?,
        )?;
        fs::write(unrelated.path().join("loaf.toml"), "[project]\nname = \"fixture\"\n")?;
        fs::write(&unrelated_entrypoint, "def main() -> None:\n    return\n")?;
        assert!(
            !has_stale_baked_project_output(
                &store,
                unrelated.path(),
                &unrelated_entrypoint,
                OvenBakeProjectTarget::Executable,
                "release",
            )?,
            "a same-named unrelated project without this bake's local receipt must not inherit its stale marker"
        );
        Ok(())
    }
}
