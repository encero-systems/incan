//! Small consumer contracts referencing independently admitted native units, without copying their artifacts.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

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

/// Command-owned index of native owners already admitted by the canonical store selector.
///
/// This shares original execution leases, not manifests copied from an ambient catalog. Each physical handoff
/// revalidates the held payload and witness before using its paths. No dependency graph or provider facts enter Oven.
#[derive(Default)]
pub struct OvenSharedNativeOwners {
    owners: BTreeMap<(PathBuf, String), Arc<OvenStoreExecutionPayload>>,
}

impl OvenSharedNativeOwners {
    /// Index already retained native owners by their exact canonical store coordinate.
    pub fn from_selected(owners: &[Arc<OvenStoreExecutionPayload>]) -> OvenPlanResult<Self> {
        let mut indexed = BTreeMap::new();
        for owner in owners {
            owner
                .verify_proven_native_payload()
                .map_err(|error| OvenPlanError::selection(error.to_string()))?;
            let store = owner
                .artifact_root
                .parent()
                .and_then(Path::parent)
                .and_then(Path::parent)
                .ok_or_else(|| OvenPlanError::selection("retained native owner has no store root"))?
                .to_path_buf();
            indexed
                .entry((store, owner.manifest.identity.clone()))
                .or_insert_with(|| Arc::clone(owner));
        }
        Ok(Self { owners: indexed })
    }

    /// Borrow only the explicitly supplied owner set, refusing missing coordinates rather than reacquiring them.
    fn select(
        &self,
        references: &[OvenSharedNativeRoot],
    ) -> OvenPlanResult<BTreeMap<(PathBuf, String), Arc<OvenStoreExecutionPayload>>> {
        let mut selected = BTreeMap::new();
        for reference in references {
            let key = (reference.store.clone(), reference.identity.clone());
            if selected.contains_key(&key) {
                continue;
            }
            let owner = self
                .owners
                .get(&key)
                .ok_or_else(|| OvenPlanError::selection("shared native owner was not admitted by this command"))?;
            owner
                .verify_proven_native_payload()
                .map_err(|error| OvenPlanError::selection(error.to_string()))?;
            selected.insert(key, Arc::clone(owner));
        }
        Ok(selected)
    }
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
    Vec<Arc<OvenStoreExecutionPayload>>,
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
    materialize_with_owners(payload, artifacts, artifact_root, expected_intent, None)
}

/// Materialize the canonical shared plan using either independently acquired or command-retained native owners.
pub(super) fn materialize_with_owners(
    payload: &[u8],
    artifacts: &OvenRustcArtifactManifest,
    artifact_root: &Path,
    expected_intent: &oven_store::OvenBuildIntent,
    admitted: Option<&OvenSharedNativeOwners>,
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
    let owners = match admitted {
        Some(admitted) => admitted.select(&shared.shared_native_roots)?,
        None => select_shared_owners(&shared.shared_native_roots)?,
    };
    for reference in &shared.shared_native_roots {
        let owner = owners
            .get(&(reference.store.clone(), reference.identity.clone()))
            .ok_or_else(|| OvenPlanError::selection("shared native unit is unavailable"))?;
        bind_shared_owner(reference, owner, artifacts, &expected, &mut locations, &mut directories)?;
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
    Ok(Some((plan, owners.into_values().collect(), locations)))
}

/// Acquire each store's complete requested set atomically, retaining one lease per distinct native owner.
///
/// Logical aliases may name one owner more than once. Grouping them avoids repeatedly opening the store and
/// reclaiming staging under its manager lock; receipt and member validation still run for every logical reference.
fn select_shared_owners(
    references: &[OvenSharedNativeRoot],
) -> OvenPlanResult<BTreeMap<(PathBuf, String), Arc<OvenStoreExecutionPayload>>> {
    let mut identities_by_store: BTreeMap<PathBuf, BTreeSet<String>> = BTreeMap::new();
    for reference in references {
        identities_by_store
            .entry(reference.store.clone())
            .or_default()
            .insert(reference.identity.clone());
    }
    let mut owners = BTreeMap::new();
    for (root, identities) in identities_by_store {
        let store = OvenStore::new(root.clone(), OvenStoreLimits::new(u64::MAX, u64::MAX, u64::MAX));
        let identities = identities.into_iter().collect::<Vec<_>>();
        let selected = store
            .select_payloads_for_execution(&identities)
            .map_err(|error| OvenPlanError::selection(error.to_string()))?;
        for (identity, owner) in identities.into_iter().zip(selected) {
            owner
                .verify_proven_native_payload()
                .map_err(|error| OvenPlanError::selection(error.to_string()))?;
            owners.insert((root.clone(), identity), Arc::new(owner));
        }
    }
    Ok(owners)
}

/// Bind one logical reference to its already leased owner and complete admitted output inventory.
fn bind_shared_owner(
    reference: &OvenSharedNativeRoot,
    owner: &OvenStoreExecutionPayload,
    artifacts: &OvenRustcArtifactManifest,
    expected: &BTreeMap<String, String>,
    locations: &mut BTreeMap<String, PathBuf>,
    directories: &mut BTreeMap<String, PathBuf>,
) -> OvenPlanResult<()> {
    safe_relative(&reference.prefix)?;
    if owner.manifest.receipt_identity != reference.receipt_identity
        || owner.manifest.intent.target != artifacts.intent.target
        || owner.manifest.intent.toolchain != artifacts.intent.toolchain
        || !owner.manifest.domain.starts_with("sdk-source-unit-")
    {
        return Err(OvenPlanError::selection(
            "shared native unit receipt or compiler differs from consumer",
        ));
    }
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
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::{OvenSharedNativeOwners, OvenSharedNativePlan, OvenSharedNativeRoot, materialize_with_owners};
    use crate::rustc::{
        OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION, OvenRustcArtifactExtern, OvenRustcArtifactManifest,
        OvenRustcSourceSearchClosure,
    };
    use oven_store::store::{
        OvenArtifactKind, OvenArtifactMaterializedFile, OvenArtifactPublishRequest, OvenStore,
        OvenStoreExecutionPayload, OvenStoreLimits,
    };
    use oven_store::{OvenGeneratedProjectRequest, OvenReceipt, receipt_generated_project};
    use std::collections::BTreeMap;
    use std::fs;
    use std::sync::Arc;

    /// Opaque artifact bytes exercise store admission and shared ownership without launching a native compiler.
    struct Fixture {
        root: tempfile::TempDir,
        store: OvenStore,
        owner: Arc<OvenStoreExecutionPayload>,
        receipt: OvenReceipt,
        artifacts: OvenRustcArtifactManifest,
        payload: Vec<u8>,
    }

    /// Publish a genuine store owner and a role-bearing consumer contract for the same admitted member.
    fn fixture() -> Result<Fixture, Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let source = root.path().join("main.rs");
        fs::write(&source, "fn main() {}\n")?;
        let receipt = receipt_generated_project(
            &OvenGeneratedProjectRequest::new(
                root.path(),
                "shared_owner_control",
                "1.0.0",
                "fixture-target",
                "fixture-toolchain",
                "debug",
                Vec::new(),
            )
            .with_generated_source("generated-root", &source),
        )?;
        let output = root.path().join("libfixture.rlib");
        fs::write(&output, b"opaque native control bytes")?;
        let store = OvenStore::new(
            root.path().join("store"),
            OvenStoreLimits::new(1_000_000, 1_000_000, 1_000_000),
        );
        let published = store.publish(&OvenArtifactPublishRequest {
            receipt: receipt.clone(),
            domain: "sdk-source-unit-target".to_string(),
            kind: OvenArtifactKind::DirectRustcPlan,
            payload: b"opaque selected payload".to_vec(),
            materialized_files: vec![OvenArtifactMaterializedFile {
                source_path: output,
                relative_path: "libfixture.rlib".to_string(),
            }],
            materialized_directories: Vec::new(),
        })?;
        let owner = Arc::new(
            store
                .select_payloads_for_execution(&[published.identity])?
                .pop()
                .ok_or("fixture owner missing")?,
        );
        let relative_path = "units/0/libfixture.rlib".to_string();
        let digest = oven_store::digest_bytes(b"opaque native control bytes");
        let paths = vec!["units/0".to_string()];
        let declared = BTreeMap::from([(relative_path.clone(), digest.clone())]);
        let artifacts = OvenRustcArtifactManifest {
            schema_version: OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION,
            intent: receipt.intent.clone(),
            dependency_search_paths: paths.clone(),
            native_search_paths: Vec::new(),
            externs: vec![OvenRustcArtifactExtern {
                crate_name: "fixture".to_string(),
                relative_path,
                digest,
            }],
            entrypoint_externs: BTreeMap::from([("generated-root".to_string(), vec!["fixture".to_string()])]),
            entrypoint_dependency_search_paths: BTreeMap::from([(
                "generated-root".to_string(),
                OvenRustcSourceSearchClosure::publisher_selected(paths, &declared),
            )]),
            registry_leaves: Vec::new(),
            registry_sources: Vec::new(),
            compile_environment: BTreeMap::new(),
            vocab_auxiliary_targets: Vec::new(),
            supporting_artifacts: Vec::new(),
        };
        let payload = serde_json::to_vec(&OvenSharedNativePlan {
            artifacts: artifacts.clone(),
            shared_native_roots: vec![OvenSharedNativeRoot {
                store: store.root().to_path_buf(),
                identity: owner.manifest.identity.clone(),
                receipt_identity: owner.manifest.receipt_identity.clone(),
                prefix: "units/0".to_string(),
            }],
        })?;
        Ok(Fixture {
            root,
            store,
            owner,
            receipt,
            artifacts,
            payload,
        })
    }

    /// A consumer shares the original owner and retains its lease after its command index has been dropped.
    #[test]
    fn dev7_native_admission_retains_original_owner_through_pruning() -> Result<(), Box<dyn std::error::Error>> {
        let Fixture {
            root,
            store,
            owner,
            receipt,
            artifacts,
            payload,
        } = fixture()?;
        let admitted = OvenSharedNativeOwners::from_selected(&[Arc::clone(&owner)])?;
        let (_, held, _) =
            materialize_with_owners(&payload, &artifacts, root.path(), &receipt.intent, Some(&admitted))?
                .ok_or("shared materialization missing")?;
        assert!(Arc::ptr_eq(&owner, held.first().ok_or("retained owner missing")?));
        let path = owner.artifact_root.join("libfixture.rlib");
        drop(owner);
        drop(admitted);
        let bounded = OvenStore::new(store.root(), OvenStoreLimits::new(1, 1, 1));
        bounded.prune()?;
        assert!(path.is_file());
        drop(held);
        bounded.prune()?;
        assert!(!path.exists());
        Ok(())
    }

    /// Exact supplied owners cannot authorize absent references, substituted receipts or changed members.
    #[test]
    fn dev7_native_admission_refuses_missing_and_substituted_owners() -> Result<(), Box<dyn std::error::Error>> {
        let f = fixture()?;
        let admitted = OvenSharedNativeOwners::from_selected(&[Arc::clone(&f.owner)])?;
        assert!(
            materialize_with_owners(
                &f.payload,
                &f.artifacts,
                f.root.path(),
                &f.receipt.intent,
                Some(&OvenSharedNativeOwners::default())
            )
            .is_err(),
            "missing owners must refuse"
        );
        for field in ["identity", "receipt_identity", "store", "prefix"] {
            let mut value: serde_json::Value = serde_json::from_slice(&f.payload)?;
            value["shared_native_roots"][0][field] = serde_json::json!("unadmitted-coordinate");
            let changed: OvenSharedNativePlan = serde_json::from_value(value)?;
            assert!(
                materialize_with_owners(
                    &serde_json::to_vec(&changed)?,
                    &changed.artifacts,
                    f.root.path(),
                    &f.receipt.intent,
                    Some(&admitted)
                )
                .is_err(),
                "{field} substitution accepted"
            );
        }
        let mut changed: OvenSharedNativePlan = serde_json::from_slice(&f.payload)?;
        changed.artifacts.externs[0].digest = oven_store::digest_bytes(b"different native output");
        assert!(
            materialize_with_owners(
                &serde_json::to_vec(&changed)?,
                &changed.artifacts,
                f.root.path(),
                &f.receipt.intent,
                Some(&admitted)
            )
            .is_err(),
            "member substitution accepted"
        );
        Ok(())
    }

    /// Domain-scoped reuse refuses missing admitted SDK owners while preserving canonical caller-owned selection.
    #[test]
    fn dev7_native_admission_preserves_other_plan_domains() -> Result<(), Box<dyn std::error::Error>> {
        let f = fixture()?;
        let consumer_receipt = oven_store::receipt_with_build_unit_input(&f.receipt, "consumer", "control")?;
        let empty = OvenSharedNativeOwners::default();
        for domain in ["caller-owned-plan", "sdk-native-consumer-plan"] {
            f.store.publish(&OvenArtifactPublishRequest {
                receipt: consumer_receipt.clone(),
                domain: domain.to_string(),
                kind: OvenArtifactKind::DirectRustcPlan,
                payload: f.payload.clone(),
                materialized_files: Vec::new(),
                materialized_directories: Vec::new(),
            })?;
            let selected =
                crate::plan::selection::select_receipt_direct_rustc_execution_plan_with_native_owners_for_domain(
                    &f.store,
                    &consumer_receipt,
                    &empty,
                    domain,
                );
            assert!(selected.is_err(), "missing owners accepted for {domain}");
            let ordinary =
                crate::plan::selection::select_receipt_direct_rustc_execution_plan_with_native_owners_for_domain(
                    &f.store,
                    &consumer_receipt,
                    &empty,
                    "other-publisher-domain",
                )?;
            assert!(ordinary.is_some(), "canonical domain fallback missing");
        }
        Ok(())
    }

    /// Sharing a live lease does not turn the original native-receipt witness into cached authorization.
    #[test]
    fn dev7_native_admission_revalidates_held_witness() -> Result<(), Box<dyn std::error::Error>> {
        let f = fixture()?;
        let admitted = OvenSharedNativeOwners::from_selected(&[Arc::clone(&f.owner)])?;
        let witness = f
            .owner
            .artifact_root
            .parent()
            .ok_or("fixture entry missing")?
            .join("native-receipt.json");
        let original = fs::read(&witness)?;
        let mut changed = original.clone();
        changed.push(b' ');
        let replacement = witness.with_extension("replacement");
        fs::write(&replacement, changed)?;
        fs::rename(&replacement, &witness)?;
        assert!(
            materialize_with_owners(
                &f.payload,
                &f.artifacts,
                f.root.path(),
                &f.receipt.intent,
                Some(&admitted)
            )
            .is_err()
        );
        fs::write(&replacement, &original)?;
        fs::rename(&replacement, &witness)?;
        assert!(
            materialize_with_owners(
                &f.payload,
                &f.artifacts,
                f.root.path(),
                &f.receipt.intent,
                Some(&admitted)
            )?
            .is_some()
        );
        fs::remove_file(&witness)?;
        assert!(
            materialize_with_owners(
                &f.payload,
                &f.artifacts,
                f.root.path(),
                &f.receipt.intent,
                Some(&admitted)
            )
            .is_err()
        );
        Ok(())
    }
}
