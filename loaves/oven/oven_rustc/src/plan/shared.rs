//! Small consumer contracts referencing independently admitted native units, without copying their artifacts.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

use oven_store::store::{OvenStore, OvenStoreExecutionPayload, OvenStoreLimits};
use serde::{Deserialize, Serialize};

use super::{OvenPlanError, OvenPlanResult};
use crate::rustc::{
    OvenRustcArtifactManifest, OvenRustcArtifactPlan, OvenRustcSourcePathProjection, expected_artifacts,
};

/// Exact immutable native owner and its logical location in a consumer manifest.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OvenSharedNativeRoot {
    /// Store containing the admitted native unit, never an ambient artifact directory.
    pub store: PathBuf,
    /// Content address selected under an execution lease.
    pub identity: String,
    /// Receipt that authorized the native unit's outputs.
    pub receipt_identity: String,
    /// Safe logical directory below which the consumer names this owner's members.
    pub prefix: String,
}

/// Ordinary direct manifest plus exact shared owner references; older payloads have no references.
#[derive(Serialize, Deserialize)]
pub struct OvenSharedNativePlan {
    /// Single public execution contract, including source-role visibility.
    #[serde(flatten)]
    pub artifacts: OvenRustcArtifactManifest,
    /// Independently leased outputs rather than per-consumer artifact copies.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub shared_native_roots: Vec<OvenSharedNativeRoot>,
}

/// Refuse unsafe logical coordinates before joining any owner-controlled physical root.
fn safe_relative(value: &str) -> OvenPlanResult<()> {
    if value.is_empty()
        || !Path::new(value)
            .components()
            .all(|part| matches!(part, Component::Normal(_)))
    {
        return Err(OvenPlanError::selection(
            "shared native plan has an unsafe relative path",
        ));
    }
    Ok(())
}

/// Verified execution paths and their retained native owners, including the logical coordinate projection.
pub(super) type SharedNativeMaterialization = (
    OvenRustcArtifactPlan,
    Vec<OvenStoreExecutionPayload>,
    BTreeMap<String, PathBuf>,
);

/// Acquire original native receipts and bind every logical member to its verified immutable physical owner.
///
/// A reference cannot grant a new file: all member digests must agree with the owner's admitted inventory. Native
/// source snapshots remain outside the compiler's search paths. Owners stay leased in the returned execution plan.
pub(super) fn materialize(
    payload: &[u8],
    artifacts: &OvenRustcArtifactManifest,
    artifact_root: &Path,
    expected_intent: &oven_store::OvenBuildIntent,
) -> OvenPlanResult<Option<SharedNativeMaterialization>> {
    let shared: OvenSharedNativePlan = serde_json::from_slice(payload)
        .map_err(|error| OvenPlanError::selection(format!("invalid shared native plan: {error}")))?;
    if shared.shared_native_roots.is_empty() {
        return Ok(None);
    }
    artifacts.validate_shape(expected_intent)?;
    if !artifacts.vocab_auxiliary_targets.is_empty() || !artifacts.registry_sources.is_empty() {
        return Err(OvenPlanError::selection(
            "shared native plans cannot carry compatibility source or auxiliary closures",
        ));
    }
    let mut prefixes = BTreeSet::new();
    for reference in &shared.shared_native_roots {
        safe_relative(&reference.prefix)?;
        if prefixes.iter().any(|prefix: &String| {
            Path::new(prefix).starts_with(&reference.prefix) || Path::new(&reference.prefix).starts_with(prefix)
        }) {
            return Err(OvenPlanError::selection("shared native owner prefixes overlap"));
        }
        prefixes.insert(reference.prefix.clone());
    }
    let expected = expected_artifacts(artifacts)?;
    let mut locations = BTreeMap::new();
    let mut directories = BTreeMap::new();
    let mut owners = Vec::new();
    for reference in &shared.shared_native_roots {
        owners.push(bind_shared_owner(
            reference,
            artifacts,
            &expected,
            &mut locations,
            &mut directories,
        )?);
    }
    bind_local_members(&expected, artifact_root, &mut locations)?;
    for directory in artifacts
        .dependency_search_paths
        .iter()
        .chain(&artifacts.native_search_paths)
    {
        safe_relative(directory)?;
        let physical = directories
            .entry(directory.clone())
            .or_insert_with(|| artifact_root.join(directory));
        if !physical.is_dir() {
            return Err(OvenPlanError::selection(
                "shared native search directory is unavailable",
            ));
        }
    }
    let plan = execution_plan(artifacts, &locations, &directories)?;
    locations.extend(directories);
    Ok(Some((plan, owners, locations)))
}

/// Reacquire one original native unit and bind only its complete admitted output inventory.
fn bind_shared_owner(
    reference: &OvenSharedNativeRoot,
    artifacts: &OvenRustcArtifactManifest,
    expected: &BTreeMap<String, String>,
    locations: &mut BTreeMap<String, PathBuf>,
    directories: &mut BTreeMap<String, PathBuf>,
) -> OvenPlanResult<OvenStoreExecutionPayload> {
    safe_relative(&reference.prefix)?;
    let store = OvenStore::new(
        reference.store.clone(),
        OvenStoreLimits::new(u64::MAX, u64::MAX, u64::MAX),
    );
    let mut selected = store
        .select_payloads_for_execution(std::slice::from_ref(&reference.identity))
        .map_err(|error| OvenPlanError::selection(error.to_string()))?;
    let owner = selected
        .pop()
        .ok_or_else(|| OvenPlanError::selection("shared native unit is unavailable"))?;
    if owner.manifest.receipt_identity != reference.receipt_identity
        || owner.manifest.intent.target != artifacts.intent.target
        || owner.manifest.intent.toolchain != artifacts.intent.toolchain
        || !owner.manifest.domain.starts_with("sdk-source-unit-")
    {
        return Err(OvenPlanError::selection(
            "shared native unit receipt or compiler differs from consumer",
        ));
    }
    owner
        .verify_proven_native_payload()
        .map_err(|error| OvenPlanError::selection(error.to_string()))?;
    for file in owner
        .admitted_materialized_files()
        .iter()
        .filter(|file| !file.relative_path.starts_with("source/"))
    {
        let logical = format!("{}/{}", reference.prefix, file.relative_path);
        if expected.get(&logical) != Some(&file.digest)
            || locations
                .insert(logical, owner.artifact_root.join(&file.relative_path))
                .is_some()
        {
            return Err(OvenPlanError::selection(
                "shared native member is missing, duplicated, or substituted",
            ));
        }
    }
    for directory in artifacts
        .dependency_search_paths
        .iter()
        .chain(&artifacts.native_search_paths)
    {
        if directory == &reference.prefix {
            directories.insert(directory.clone(), owner.artifact_root.clone());
        } else if let Some(relative) = directory.strip_prefix(&format!("{}/", reference.prefix)) {
            safe_relative(relative)?;
            directories.insert(directory.clone(), owner.artifact_root.join(relative));
        }
    }
    Ok(owner)
}

/// Bind plan-owned facade files only after containment and exact digest validation.
fn bind_local_members(
    expected: &BTreeMap<String, String>,
    root: &Path,
    locations: &mut BTreeMap<String, PathBuf>,
) -> OvenPlanResult<()> {
    let root = root
        .canonicalize()
        .map_err(|error| OvenPlanError::selection(error.to_string()))?;
    for (relative, digest) in expected {
        if locations.contains_key(relative) {
            continue;
        }
        safe_relative(relative)?;
        let path = root
            .join(relative)
            .canonicalize()
            .map_err(|error| OvenPlanError::selection(error.to_string()))?;
        if !path.starts_with(&root)
            || oven_store::digest_bytes(
                &std::fs::read(&path).map_err(|error| OvenPlanError::selection(error.to_string()))?,
            ) != *digest
        {
            return Err(OvenPlanError::selection(
                "shared plan local member escapes its root or has changed",
            ));
        }
        locations.insert(relative.clone(), path);
    }
    Ok(())
}

/// Preserve exact publisher source-role directories across logical-to-physical owner relocation.
fn source_projection(
    artifacts: &OvenRustcArtifactManifest,
    directories: &BTreeMap<String, PathBuf>,
) -> OvenPlanResult<OvenRustcSourcePathProjection> {
    let mut roles = BTreeMap::new();
    for (role, closure) in &artifacts.entrypoint_dependency_search_paths {
        if !closure.legacy_projections.is_empty() {
            return Err(OvenPlanError::selection(
                "shared native plans require publisher-captured source roles",
            ));
        }
        let paths = closure
            .publisher_paths
            .iter()
            .map(|directory| {
                directories
                    .get(&directory.relative_path)
                    .cloned()
                    .ok_or_else(|| OvenPlanError::selection("shared source role has an undeclared directory"))
            })
            .collect::<OvenPlanResult<BTreeSet<_>>>()?;
        roles.insert(role.clone(), (closure.clone(), paths));
    }
    Ok(OvenRustcSourcePathProjection {
        declared: directories.values().cloned().collect(),
        roles,
    })
}

/// Compose public provider paths from their already verified selections rather than assuming one copied root.
///
/// Manifest composition establishes byte-compatible overlap first. The fragments retain all original unit leases;
/// this step only relocates the selected logical coordinates and preserves the merged source-role contract.
pub(super) fn compose_provider_paths(
    composed: &super::OvenDirectPackagedProviderExecutionPlan,
) -> OvenPlanResult<OvenRustcArtifactPlan> {
    let artifacts = &composed.artifacts;
    let mut locations = BTreeMap::new();
    let mut directories = BTreeMap::new();
    for fragment in &composed.fragments {
        for artifact in &fragment.supporting_artifacts {
            let path = fragment.plan.physical_path(&artifact.relative_path);
            locations.insert(artifact.relative_path.clone(), path);
        }
        for directory in fragment
            .plan
            .artifacts
            .dependency_search_paths
            .iter()
            .chain(&fragment.plan.artifacts.native_search_paths)
        {
            let path = fragment.plan.physical_path(directory);
            directories.entry(directory.clone()).or_insert(path);
        }
    }
    execution_plan(artifacts, &locations, &directories)
}

/// Construct one execution view from already admitted members and directories, preserving source-role isolation.
fn execution_plan(
    artifacts: &OvenRustcArtifactManifest,
    locations: &BTreeMap<String, PathBuf>,
    directories: &BTreeMap<String, PathBuf>,
) -> OvenPlanResult<OvenRustcArtifactPlan> {
    let externs = artifacts
        .externs
        .iter()
        .map(|external| {
            locations
                .get(&external.relative_path)
                .cloned()
                .map(|path| (external.crate_name.clone(), path))
                .ok_or_else(|| OvenPlanError::selection("composed shared provider omitted an extern"))
        })
        .collect::<OvenPlanResult<Vec<_>>>()?;
    Ok(OvenRustcArtifactPlan {
        source_path_projection: Some(source_projection(artifacts, directories)?),
        dependency_search_paths: artifacts
            .dependency_search_paths
            .iter()
            .map(|path| {
                directories
                    .get(path)
                    .cloned()
                    .ok_or_else(|| OvenPlanError::selection("shared plan omitted a search directory"))
            })
            .collect::<OvenPlanResult<Vec<_>>>()?,
        native_search_paths: artifacts
            .native_search_paths
            .iter()
            .map(|path| {
                directories
                    .get(path)
                    .cloned()
                    .ok_or_else(|| OvenPlanError::selection("shared plan omitted a search directory"))
            })
            .collect::<OvenPlanResult<Vec<_>>>()?,
        externs,
        compile_environment: artifacts.compile_environment.clone(),
        caller_owned_library_digests: BTreeMap::new(),
    })
}
