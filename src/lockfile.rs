//! `incan.lock` parsing, validation, and fingerprinting.
//!
//! The lockfile embeds a Cargo.lock payload and records a dependency fingerprint for strict `--locked` / `--frozen`
//! builds.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{self, ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::library_manifest::{
    ProviderSemanticToolchainDependency, digest_provider_semantic_artifact_with_context_and_cache,
    digest_toolchain_source_tree_with_cache,
};
use crate::manifest::{DependencySource, DependencySpec, GitReference};
use crate::provider::{
    BackendImplementationRequirement, ComponentSelectionReason, PackageFeaturePlan, ProviderParticipation,
    ProviderPlan, ProviderProvenance, ProviderRecord, ResolvedSdkComponents, SdkInventory,
};

const LOCKFILE_FORMAT_VERSION: u32 = 2;
const LEGACY_LOCKFILE_FORMAT_VERSION: u32 = 1;
/// Synthetic Cargo package used to resolve one canonical lock across all workspace members.
pub(crate) const WORKSPACE_LOCK_CARGO_PACKAGE_NAME: &str = "incan_workspace";

#[derive(Debug, thiserror::Error)]
pub enum LockfileError {
    #[error("failed to read {path}: {source}")]
    Read { path: PathBuf, source: std::io::Error },
    #[error("failed to write {path}: {source}")]
    Write { path: PathBuf, source: std::io::Error },
    #[error("failed to parse {path}: {source}")]
    Parse { path: PathBuf, source: toml::de::Error },
    #[error("failed to serialize lockfile: {0}")]
    Serialize(String),
    #[error("invalid lockfile {path}: {message}")]
    Invalid { path: PathBuf, message: String },
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CargoFeatureSelection {
    #[serde(rename = "cargo-features", default)]
    pub cargo_features: Vec<String>,
    #[serde(rename = "cargo-no-default-features", default)]
    pub cargo_no_default_features: bool,
    #[serde(rename = "cargo-all-features", default)]
    pub cargo_all_features: bool,
}

impl CargoFeatureSelection {
    pub fn normalized(mut self) -> Self {
        self.cargo_features.sort();
        self.cargo_features.dedup();
        self
    }
}

#[derive(Debug, Clone)]
pub struct IncanLock {
    pub format: u32,
    pub incan_version: String,
    pub deps_fingerprint: String,
    pub cargo_features: CargoFeatureSelection,
    pub semantic: SemanticLockState,
    pub cargo_lock_payload: String,
}

/// Advisory guards retained for one canonical lockfile publication critical section.
pub(crate) struct PublicationLock {
    _legacy: Option<File>,
    _active: File,
}

/// Backend-neutral graph inputs whose resolution can change checking or generated output.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SemanticLockState {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sdk: Option<LockedSdkState>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub packages: Vec<LockedPackageFeatures>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub feature_edges: Vec<LockedFeatureEdge>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub providers: Vec<LockedProvider>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub workspace_members: Vec<LockedWorkspaceMember>,
}

/// One workspace member's independently resolved semantic graph inside the canonical root lock.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LockedWorkspaceMember {
    pub member_root: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sdk: Option<LockedSdkState>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub packages: Vec<LockedPackageFeatures>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub feature_edges: Vec<LockedFeatureEdge>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub providers: Vec<LockedProvider>,
}

/// Exact SDK inventory and expanded component selection recorded by the lock.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LockedSdkState {
    pub identity: String,
    pub inventory_digest: String,
    pub profile: String,
    pub components: Vec<LockedSdkComponent>,
}

/// One selected SDK component and its stable activation reason.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LockedSdkComponent {
    pub id: String,
    pub version: String,
    pub reason: String,
}

/// Public feature and optional-dependency closure for one concrete package root.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LockedPackageFeatures {
    pub package: String,
    pub project_root: String,
    pub active_features: BTreeSet<String>,
    pub active_optional_dependencies: BTreeSet<String>,
    pub dependency_features: BTreeMap<String, BTreeSet<String>>,
    pub required_sdk_components: BTreeSet<String>,
}

/// One active package dependency edge and its unified public feature request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LockedFeatureEdge {
    pub from: String,
    pub dependency_key: String,
    pub to: String,
    pub requested_features: BTreeSet<String>,
    pub default_features: bool,
    pub optional: bool,
}

/// Exact provider identity, semantic participation, used modules, and private implementation closure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LockedProvider {
    pub identity: String,
    pub participation: String,
    pub namespace_claims: BTreeSet<Vec<String>>,
    pub used_modules: BTreeSet<Vec<String>>,
    pub implementation_facets: Vec<String>,
    pub backend_requirements: BTreeSet<String>,
}

impl IncanLock {
    pub fn load(path: &Path) -> Result<Self, LockfileError> {
        let content = fs::read_to_string(path).map_err(|e| LockfileError::Read {
            path: path.to_path_buf(),
            source: e,
        })?;
        parse_lockfile(&content, path)
    }

    /// Render and crash-safely publish this lockfile at `path` using RFC 112's ordered publication sequence.
    ///
    /// The method stages complete contents beside the destination, synchronizes them, atomically replaces the target,
    /// then requests parent-directory synchronization. Concurrent cooperative publishers serialize on a stable lock in
    /// compiler-owned target state, so callers either observe the prior complete lockfile or the new complete lockfile.
    pub fn write(&self, path: &Path) -> Result<(), LockfileError> {
        let publication_lock = acquire_publication_lock(path).map_err(|e| LockfileError::Write {
            path: path.to_path_buf(),
            source: e,
        })?;
        self.write_while_locked(path, &publication_lock)
    }

    /// Publish this lockfile while the caller retains the matching compiler-private publication lock.
    ///
    /// Workspace lock generation holds this guard across resolution and Cargo lockfile generation as well as the final
    /// publish, preventing concurrent commands from sharing the generated-project staging directory.
    pub(crate) fn write_while_locked(
        &self,
        path: &Path,
        publication_lock: &PublicationLock,
    ) -> Result<(), LockfileError> {
        let raw = RawIncanLock {
            incan: RawIncanMeta {
                format: self.format,
                incan_version: self.incan_version.clone(),
                deps_fingerprint: self.deps_fingerprint.clone(),
                cargo_features: self.cargo_features.cargo_features.clone(),
                cargo_no_default_features: self.cargo_features.cargo_no_default_features,
                cargo_all_features: self.cargo_features.cargo_all_features,
            },
            semantic: self.semantic.clone(),
            cargo: RawCargoLock {
                lock: self.cargo_lock_payload.clone(),
            },
        };

        let body = toml::to_string(&raw).map_err(|e| LockfileError::Serialize(e.to_string()))?;
        let content =
            format!("# Auto-generated by Incan - do not edit manually\n# Regenerate with: incan lock\n\n{body}");
        publish_lockfile(path, content.as_bytes(), publication_lock).map_err(|e| LockfileError::Write {
            path: path.to_path_buf(),
            source: e,
        })?;
        Ok(())
    }

    /// Construct a lock for callers that have no provider or package-feature semantic state.
    pub fn new(deps_fingerprint: String, cargo_features: CargoFeatureSelection, cargo_lock_payload: String) -> Self {
        Self::new_with_semantic(
            deps_fingerprint,
            cargo_features,
            SemanticLockState::default(),
            cargo_lock_payload,
        )
    }

    /// Construct a lock containing both the backend dependency payload and the resolved semantic provider graph.
    pub fn new_with_semantic(
        deps_fingerprint: String,
        cargo_features: CargoFeatureSelection,
        semantic: SemanticLockState,
        cargo_lock_payload: String,
    ) -> Self {
        Self {
            format: LOCKFILE_FORMAT_VERSION,
            incan_version: crate::version::INCAN_VERSION.to_string(),
            deps_fingerprint,
            cargo_features: cargo_features.normalized(),
            semantic,
            cargo_lock_payload: normalize_cargo_lock_payload(&cargo_lock_payload),
        }
    }
}

/// Snapshot the shared provider, SDK-component, and package-feature plans into portable canonical lock state.
pub fn semantic_lock_state(
    project_root: &Path,
    sdk_inventory: Option<&SdkInventory>,
    sdk_components: Option<&ResolvedSdkComponents>,
    package_features: Option<&PackageFeaturePlan>,
    provider_plan: &ProviderPlan,
    sdk_path_dependencies: &[DependencySpec],
) -> Result<SemanticLockState, String> {
    let semantic_toolchain_dependencies = semantic_toolchain_dependencies(sdk_path_dependencies)?;
    let dependency_semantic_digests =
        provider_dependency_semantic_digests(provider_plan, &semantic_toolchain_dependencies)?;
    let mut provider_digest_cache = BTreeMap::new();
    let mut provider_semantic_identities = Vec::new();
    for provider in provider_plan.records() {
        let identity = locked_provider_semantic_identity(
            provider,
            &dependency_semantic_digests,
            &semantic_toolchain_dependencies,
            &mut provider_digest_cache,
        )?;
        provider_semantic_identities.push((provider, identity));
    }
    let sdk = match (sdk_inventory, sdk_components) {
        (Some(inventory), Some(components)) => {
            let inventory_digest = semantic_sdk_inventory_digest(inventory, &provider_semantic_identities)?;
            let selected = components
                .enabled
                .iter()
                .filter_map(|id| {
                    let component = inventory.components.get(id)?;
                    let reason = components
                        .reasons
                        .get(id)
                        .map(component_reason)
                        .unwrap_or_else(|| "selected".into());
                    Some(LockedSdkComponent {
                        id: id.clone(),
                        version: component.version.clone(),
                        reason,
                    })
                })
                .collect();
            Some(LockedSdkState {
                identity: inventory.identity(),
                inventory_digest,
                profile: components.profile.clone(),
                components: selected,
            })
        }
        (None, None) => None,
        _ => return Err("SDK inventory and resolved component state must be recorded together".to_string()),
    };

    let packages = package_features
        .iter()
        .flat_map(|plan| plan.packages())
        .map(|package| LockedPackageFeatures {
            package: package.package_name.clone(),
            project_root: portable_project_path(project_root, &package.project_root),
            active_features: package.features.active_features.clone(),
            active_optional_dependencies: package.features.active_optional_dependencies.clone(),
            dependency_features: package.features.dependency_features.clone(),
            required_sdk_components: package.features.required_sdk_components.clone(),
        })
        .collect();
    let feature_edges = package_features
        .iter()
        .flat_map(|plan| plan.edges())
        .map(|edge| LockedFeatureEdge {
            from: portable_project_path(project_root, &edge.from),
            dependency_key: edge.dependency_key.clone(),
            to: portable_project_path(project_root, &edge.to),
            requested_features: edge.requested_features.clone(),
            default_features: edge.default_features,
            optional: edge.optional,
        })
        .collect();
    let providers = provider_semantic_identities
        .into_iter()
        .filter(|(provider, _)| provider.enabled)
        .map(|(provider, identity)| LockedProvider {
            identity,
            participation: participation_name(provider_plan.participation(provider)).to_string(),
            namespace_claims: provider.namespace_claims.clone(),
            used_modules: provider_plan.used_modules(provider),
            implementation_facets: provider_plan
                .selected_implementation_facets(provider)
                .into_iter()
                .map(|facet| facet.id.clone())
                .collect(),
            backend_requirements: provider_plan
                .selected_backend_requirements(provider)
                .iter()
                .map(backend_requirement_name)
                .collect(),
        })
        .collect();
    Ok(SemanticLockState {
        sdk,
        packages,
        feature_edges,
        providers,
        workspace_members: Vec::new(),
    })
}

/// Project a physical provider record into its path-independent semantic lock identity.
///
/// Runtime catalog matching keeps the byte-exact provider digest. Only the lock projection substitutes a digest that
/// removes checked delivery paths from generated metadata while retaining source, API, dependency-content, and
/// feature changes.
fn locked_provider_semantic_identity(
    provider: &ProviderRecord,
    dependency_semantic_digests: &BTreeMap<String, String>,
    semantic_toolchain_dependencies: &[ProviderSemanticToolchainDependency],
    resolved_artifacts: &mut BTreeMap<PathBuf, String>,
) -> Result<String, String> {
    let digest = match (provider.manifest.as_deref(), provider.artifact.as_ref()) {
        (Some(manifest), Some(artifact)) => digest_provider_semantic_artifact_with_context_and_cache(
            &artifact.crate_root,
            &artifact.manifest_path,
            &artifact.cargo_toml_path,
            manifest,
            dependency_semantic_digests,
            semantic_toolchain_dependencies,
            resolved_artifacts,
        )
        .map_err(|error| error.to_string())?,
        _ if matches!(provider.provenance, ProviderProvenance::Sdk { .. }) && !provider.available => {
            "unavailable".to_string()
        }
        _ => provider.identity.digest.clone(),
    };
    let features = provider
        .identity
        .feature_projection
        .iter()
        .cloned()
        .collect::<Vec<_>>()
        .join(",");
    Ok(format!(
        "{}@{}#{}[{}]",
        provider.identity.name, provider.identity.version, digest, features
    ))
}

/// Resolve exact compiler-owned SDK support roots into path-independent content identities.
fn semantic_toolchain_dependencies(
    sdk_path_dependencies: &[DependencySpec],
) -> Result<Vec<ProviderSemanticToolchainDependency>, String> {
    let mut resolved_packages = BTreeMap::new();
    sdk_path_dependencies
        .iter()
        .filter_map(|dependency| {
            let DependencySource::Path { path } = &dependency.source else {
                return None;
            };
            let package_name = dependency
                .package
                .clone()
                .unwrap_or_else(|| dependency.crate_name.clone());
            // Generated provider artifacts carry their checked `.incnlib` identity and are hashed by the provider
            // graph below. Only compiler support Cargo packages need the separate recursive source closure.
            if path.join(format!("{package_name}.incnlib")).is_file() {
                return None;
            }
            Some(
                digest_toolchain_source_tree_with_cache(path, &mut resolved_packages)
                    .map(|content_digest| ProviderSemanticToolchainDependency {
                        crate_name: dependency.crate_name.clone(),
                        package_name,
                        artifact_root: path.clone(),
                        content_digest,
                    })
                    .map_err(|error| error.to_string()),
            )
        })
        .collect()
}

/// Precompute path-independent identities for every locally available physical provider digest.
fn provider_dependency_semantic_digests(
    provider_plan: &ProviderPlan,
    semantic_toolchain_dependencies: &[ProviderSemanticToolchainDependency],
) -> Result<BTreeMap<String, String>, String> {
    let mut candidates = BTreeMap::<String, BTreeSet<String>>::new();
    let mut resolved_artifacts = BTreeMap::new();
    for provider in provider_plan.records() {
        let (Some(manifest), Some(artifact)) = (provider.manifest.as_deref(), provider.artifact.as_ref()) else {
            continue;
        };
        let semantic_digest = digest_provider_semantic_artifact_with_context_and_cache(
            &artifact.crate_root,
            &artifact.manifest_path,
            &artifact.cargo_toml_path,
            manifest,
            &BTreeMap::new(),
            semantic_toolchain_dependencies,
            &mut resolved_artifacts,
        )
        .map_err(|error| error.to_string())?;
        candidates
            .entry(provider.identity.digest.clone())
            .or_default()
            .insert(semantic_digest);
    }
    Ok(candidates
        .into_iter()
        .filter_map(|(physical, semantic)| {
            (semantic.len() == 1).then(|| semantic.into_iter().next().map(|semantic| (physical, semantic)))?
        })
        .collect())
}

/// Hash the relocatable SDK inventory after replacing physical provider digests with semantic lock identities.
fn semantic_sdk_inventory_digest(
    inventory: &SdkInventory,
    provider_semantic_identities: &[(&ProviderRecord, String)],
) -> Result<String, String> {
    let mut identities = BTreeMap::<(String, String, String), String>::new();
    for (provider, identity) in provider_semantic_identities {
        let ProviderProvenance::Sdk { component_id, .. } = &provider.provenance else {
            continue;
        };
        let key = (
            component_id.clone(),
            provider.identity.name.clone(),
            provider.identity.version.clone(),
        );
        if identities.insert(key.clone(), identity.clone()).is_some() {
            return Err(format!(
                "SDK component `{}` contains duplicate provider {}@{} while computing semantic inventory identity",
                key.0, key.1, key.2
            ));
        }
    }

    let payload = inventory.to_json().map_err(|error| error.to_string())?;
    let mut value: serde_json::Value = serde_json::from_str(&payload).map_err(|error| error.to_string())?;
    let components = value
        .get_mut("components")
        .and_then(serde_json::Value::as_object_mut)
        .ok_or_else(|| "serialized SDK inventory has no component map".to_string())?;
    for (component_id, component) in components {
        let Some(providers) = component.get_mut("providers").and_then(serde_json::Value::as_array_mut) else {
            continue;
        };
        for provider in providers {
            let Some(provider_object) = provider.as_object_mut() else {
                continue;
            };
            let Some(name) = provider_object
                .get("name")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
            else {
                continue;
            };
            let Some(version) = provider_object
                .get("version")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
            else {
                continue;
            };
            if let Some(identity) = identities.get(&(component_id.clone(), name, version)) {
                provider_object.insert("digest".to_string(), serde_json::Value::String(identity.clone()));
            }
        }
    }
    let normalized = serde_json::to_vec(&value).map_err(|error| error.to_string())?;
    Ok(digest_bytes(&normalized))
}

/// Assemble independently resolved member graphs into the one canonical workspace semantic lock state.
///
/// Member semantic paths are recorded relative to their owning member when possible. This boundary rebases every
/// nested package and feature-edge coordinate into workspace-root coordinates so the canonical lock remains portable
/// when the workspace is relocated.
pub fn workspace_semantic_lock_state(
    workspace_root: &Path,
    members: impl IntoIterator<Item = (PathBuf, SemanticLockState)>,
) -> Result<SemanticLockState, String> {
    let mut workspace_members = members
        .into_iter()
        .map(|(member_root, semantic)| {
            if !semantic.workspace_members.is_empty() {
                return Err(format!(
                    "workspace member {} contains a nested workspace semantic graph",
                    member_root.display()
                ));
            }
            let packages = semantic
                .packages
                .into_iter()
                .map(|mut package| {
                    package.project_root =
                        rebase_member_semantic_path(workspace_root, &member_root, &package.project_root);
                    package
                })
                .collect();
            let feature_edges = semantic
                .feature_edges
                .into_iter()
                .map(|mut edge| {
                    edge.from = rebase_member_semantic_path(workspace_root, &member_root, &edge.from);
                    edge.to = rebase_member_semantic_path(workspace_root, &member_root, &edge.to);
                    edge
                })
                .collect();
            Ok(LockedWorkspaceMember {
                member_root: portable_project_path(workspace_root, &member_root),
                sdk: semantic.sdk,
                packages,
                feature_edges,
                providers: semantic.providers,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    workspace_members.sort_by(|left, right| left.member_root.cmp(&right.member_root));
    Ok(SemanticLockState {
        workspace_members,
        ..SemanticLockState::default()
    })
}

/// Translate one member-local semantic coordinate into the canonical workspace coordinate space.
///
/// Absolute coordinates are retained as their original targets before portability is applied. Relative coordinates,
/// including the empty member-root marker, are first resolved against the member root. Targets outside the workspace
/// remain absolute because [`portable_project_path`] only strips paths contained by its project root.
fn rebase_member_semantic_path(workspace_root: &Path, member_root: &Path, path: &str) -> String {
    let member_path = Path::new(path);
    let resolved = if member_path.is_absolute() {
        member_path.to_path_buf()
    } else {
        member_root.join(member_path)
    };
    portable_project_path(workspace_root, &resolved)
}

/// Render one component-selection edge in the stable lockfile vocabulary.
fn component_reason(reason: &ComponentSelectionReason) -> String {
    match reason {
        ComponentSelectionReason::Mandatory => "mandatory".to_string(),
        ComponentSelectionReason::Profile { profile } => format!("profile:{profile}"),
        ComponentSelectionReason::Explicit => "explicit".to_string(),
        ComponentSelectionReason::Dependency { required_by } => format!("dependency:{required_by}"),
    }
}

/// Render provider availability, enablement, and use in the stable lockfile vocabulary.
fn participation_name(participation: ProviderParticipation) -> &'static str {
    match participation {
        ProviderParticipation::Unavailable => "unavailable",
        ProviderParticipation::Disabled => "disabled",
        ProviderParticipation::Enabled => "enabled",
        ProviderParticipation::Used => "used",
    }
}

/// Render one private provider implementation requirement in the stable lockfile vocabulary.
fn backend_requirement_name(requirement: &BackendImplementationRequirement) -> String {
    match requirement {
        BackendImplementationRequirement::CargoFeature { crate_name, feature } => {
            format!("cargo-feature:{crate_name}/{feature}")
        }
        BackendImplementationRequirement::CargoDependency { dependency } => {
            format!("cargo-dependency:{}", dependency.crate_name)
        }
    }
}

/// Render a project-relative path when possible so semantic lock fingerprints survive relocation.
fn portable_project_path(project_root: &Path, path: &Path) -> String {
    let normalized_root = fs::canonicalize(project_root).unwrap_or_else(|_| project_root.to_path_buf());
    let normalized_path = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    normalized_path
        .strip_prefix(&normalized_root)
        .unwrap_or(&normalized_path)
        .to_string_lossy()
        .replace('\\', "/")
}

/// Return the canonical SHA-256 identity for one serialized semantic lock payload.
fn digest_bytes(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("sha256:{}", hex::encode(hasher.finalize()))
}

/// Publish a complete lockfile while holding the compiler-private stable publication-lock identity.
///
/// This is compiler-host infrastructure: `incan lock` runs before any user program exists, so it cannot invoke the
/// generated Incan `std.fs` library directly. The operation intentionally mirrors its public recipe—exclusive stable
/// lock, same-directory exclusive staging, content synchronization, atomic replacement, then parent synchronization.
fn publish_lockfile(path: &Path, content: &[u8], _publication_lock: &PublicationLock) -> io::Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let (staged_path, mut staged_file) = create_staged_lockfile(path)?;

    let result = (|| {
        staged_file.write_all(content)?;
        staged_file.sync_all()?;
        fs::rename(&staged_path, path)?;
        File::open(parent)?.sync_all()?;
        Ok(())
    })();
    drop(staged_file);
    if result.is_err() && staged_path.exists() {
        // A failed stage remains private to this invocation; never remove the published target to retry a failure.
        let _ = fs::remove_file(&staged_path);
    }
    result
}

/// Return the compiler-owned project state root used for canonical lock generation and related metadata.
pub(crate) fn compiler_lock_state_dir(project_root: &Path) -> PathBuf {
    project_root.join("target").join("incan_lock")
}

/// Retain the compiler-owned lock descriptor for the entire publication critical section.
///
/// The persistent advisory-lock file lives below `target/incan_lock`, which is already compiler-owned ignored state,
/// rather than beside `incan.lock` in the project root. When an older compiler has already created the legacy sibling,
/// new compilers acquire that inode first and retain it alongside the active guard. This preserves mixed-version
/// exclusion without creating or unlinking legacy project-root state on clean projects. A project with no legacy inode
/// is an intentional protocol cutover: an older compiler started later cannot discover the new hidden guard.
pub(crate) fn acquire_publication_lock(path: &Path) -> io::Result<PublicationLock> {
    let legacy = acquire_legacy_publication_lock_if_present(path)?;
    let lock_path = publication_lock_path(path)?;
    let lock_parent = lock_path.parent().ok_or_else(|| {
        io::Error::new(
            ErrorKind::InvalidInput,
            "lockfile publication state requires a parent directory",
        )
    })?;
    fs::create_dir_all(lock_parent)?;
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(lock_path)?;
    file.lock()?;
    Ok(PublicationLock {
        _legacy: legacy,
        _active: file,
    })
}

/// Resolve the stable compiler-owned advisory-lock path for one published lockfile.
fn publication_lock_path(path: &Path) -> io::Result<PathBuf> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let file_name = path.file_name().ok_or_else(|| {
        io::Error::new(
            ErrorKind::InvalidInput,
            "lockfile publication requires a target path with a final component",
        )
    })?;
    Ok(compiler_lock_state_dir(parent).join(format!(".{}.publication.lock", file_name.to_string_lossy())))
}

/// Acquire the old project-root guard when it already exists, without creating or unlinking that inode.
fn acquire_legacy_publication_lock_if_present(path: &Path) -> io::Result<Option<File>> {
    let legacy_path = legacy_publication_lock_path(path)?;
    let file = match OpenOptions::new().read(true).write(true).open(legacy_path) {
        Ok(file) => file,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    file.lock()?;
    Ok(Some(file))
}

/// Resolve the sibling advisory-lock path used by compilers predating issue #912.
fn legacy_publication_lock_path(path: &Path) -> io::Result<PathBuf> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let file_name = path.file_name().ok_or_else(|| {
        io::Error::new(
            ErrorKind::InvalidInput,
            "lockfile publication requires a target path with a final component",
        )
    })?;
    Ok(parent.join(format!(".{}.incan.lock", file_name.to_string_lossy())))
}

/// Create one unique same-directory staging file, guaranteeing that rename uses the target filesystem.
fn create_staged_lockfile(path: &Path) -> io::Result<(PathBuf, File)> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let file_name = path.file_name().ok_or_else(|| {
        io::Error::new(
            ErrorKind::InvalidInput,
            "lockfile publication requires a target path with a final component",
        )
    })?;
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| io::Error::other(format!("system clock predates Unix epoch: {error}")))?
        .as_nanos();
    for attempt in 0..128 {
        let staged_path = parent.join(format!(
            ".{}.incan-stage-{}-{}-{attempt}",
            file_name.to_string_lossy(),
            std::process::id(),
            timestamp
        ));
        match OpenOptions::new().write(true).create_new(true).open(&staged_path) {
            Ok(file) => return Ok((staged_path, file)),
            Err(error) if error.kind() == ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(
        ErrorKind::AlreadyExists,
        "failed to allocate a unique lockfile staging path",
    ))
}

/// Compute a stable SHA-256 fingerprint over the effective dependency specs and Cargo feature selection.
///
/// ## Parameters
///
/// - `project_root`: When provided, `path:` dependency sources are relativized to this directory so the fingerprint is
///   portable across machines (RFC 013 Appendix A.2).
pub fn compute_deps_fingerprint(
    dependencies: &[DependencySpec],
    dev_dependencies: &[DependencySpec],
    cargo_features: &CargoFeatureSelection,
    project_root: Option<&Path>,
) -> String {
    compute_resolved_fingerprint(
        dependencies,
        dev_dependencies,
        cargo_features,
        project_root,
        &SemanticLockState::default(),
    )
}

/// Compute the complete dependency and semantic-provider fingerprint for canonical lock freshness.
pub fn compute_resolved_fingerprint(
    dependencies: &[DependencySpec],
    dev_dependencies: &[DependencySpec],
    cargo_features: &CargoFeatureSelection,
    project_root: Option<&Path>,
    semantic: &SemanticLockState,
) -> String {
    compute_resolved_fingerprint_with_sdk_paths(
        dependencies,
        dev_dependencies,
        cargo_features,
        project_root,
        semantic,
        &[],
    )
}

/// Compute lock freshness while replacing compiler-owned SDK delivery paths with their checked semantic identities.
///
/// SDK provider roots are immutable cache coordinates, not project dependency identities. The semantic state already
/// records each provider's name, version, artifact digest, and selected feature projection, so hashing the physical
/// path as well would make an equivalent relocated provider store appear to be a dependency change.
pub fn compute_resolved_fingerprint_with_sdk_paths(
    dependencies: &[DependencySpec],
    dev_dependencies: &[DependencySpec],
    cargo_features: &CargoFeatureSelection,
    project_root: Option<&Path>,
    semantic: &SemanticLockState,
    sdk_path_dependencies: &[DependencySpec],
) -> String {
    let mut specs = Vec::new();

    for spec in dependencies {
        specs.push(SpecFingerprint::from_spec(
            spec,
            "normal",
            project_root,
            sdk_path_dependencies,
        ));
    }
    for spec in dev_dependencies {
        specs.push(SpecFingerprint::from_spec(
            spec,
            "dev",
            project_root,
            sdk_path_dependencies,
        ));
    }

    specs.sort_by(|a, b| (a.kind.as_str(), a.crate_name.as_str()).cmp(&(b.kind.as_str(), b.crate_name.as_str())));
    let input = FingerprintInput {
        cargo_feature_selection: CargoFeatureSelectionFingerprint::from_selection(cargo_features),
        specs,
        semantic,
    };
    let json = serde_json::to_string(&input).unwrap_or_else(|_| "{}".to_string());
    let mut hasher = Sha256::new();
    hasher.update(json.as_bytes());
    let hash = hasher.finalize();
    format!("sha256:{}", hex::encode(hash))
}

pub fn normalize_cargo_lock_payload(payload: &str) -> String {
    let mut out = payload.replace("\r\n", "\n");
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out
}

/// Parse and validate one lockfile while retaining its path in all diagnostics.
fn parse_lockfile(content: &str, path: &Path) -> Result<IncanLock, LockfileError> {
    let raw: RawIncanLock = toml::from_str(content).map_err(|e| LockfileError::Parse {
        path: path.to_path_buf(),
        source: e,
    })?;

    if raw.incan.format != LOCKFILE_FORMAT_VERSION && raw.incan.format != LEGACY_LOCKFILE_FORMAT_VERSION {
        return Err(LockfileError::Invalid {
            path: path.to_path_buf(),
            message: format!(
                "unsupported lockfile format {} (expected {})",
                raw.incan.format, LOCKFILE_FORMAT_VERSION
            ),
        });
    }

    Ok(IncanLock {
        format: raw.incan.format,
        incan_version: raw.incan.incan_version,
        deps_fingerprint: raw.incan.deps_fingerprint,
        cargo_features: CargoFeatureSelection {
            cargo_features: raw.incan.cargo_features,
            cargo_no_default_features: raw.incan.cargo_no_default_features,
            cargo_all_features: raw.incan.cargo_all_features,
        }
        .normalized(),
        semantic: raw.semantic,
        cargo_lock_payload: normalize_cargo_lock_payload(&raw.cargo.lock),
    })
}

#[derive(Debug, Serialize, Deserialize)]
struct RawIncanLock {
    incan: RawIncanMeta,
    #[serde(default)]
    semantic: SemanticLockState,
    cargo: RawCargoLock,
}

#[derive(Debug, Serialize, Deserialize)]
struct RawIncanMeta {
    format: u32,
    #[serde(rename = "incan-version")]
    incan_version: String,
    #[serde(rename = "deps-fingerprint")]
    deps_fingerprint: String,
    #[serde(rename = "cargo-features", default)]
    cargo_features: Vec<String>,
    #[serde(rename = "cargo-no-default-features", default)]
    cargo_no_default_features: bool,
    #[serde(rename = "cargo-all-features", default)]
    cargo_all_features: bool,
}

#[derive(Debug, Serialize, Deserialize)]
struct RawCargoLock {
    lock: String,
}

#[derive(Debug, Serialize)]
struct FingerprintInput<'a> {
    cargo_feature_selection: CargoFeatureSelectionFingerprint,
    specs: Vec<SpecFingerprint>,
    semantic: &'a SemanticLockState,
}

#[derive(Debug, Serialize)]
struct CargoFeatureSelectionFingerprint {
    cargo_all_features: bool,
    cargo_no_default_features: bool,
    cargo_features: Vec<String>,
}

impl CargoFeatureSelectionFingerprint {
    fn from_selection(selection: &CargoFeatureSelection) -> Self {
        let mut features = selection.cargo_features.clone();
        features.sort();
        features.dedup();
        Self {
            cargo_all_features: selection.cargo_all_features,
            cargo_no_default_features: selection.cargo_no_default_features,
            cargo_features: features,
        }
    }
}

#[derive(Debug, Serialize)]
struct SpecFingerprint {
    crate_name: String,
    kind: String,
    source: String,
    version_req: Option<String>,
    default_features: bool,
    features: Vec<String>,
    optional: bool,
    package: Option<String>,
}

impl SpecFingerprint {
    /// Snapshot one Cargo dependency while distinguishing semantic SDK ownership from ordinary project paths.
    fn from_spec(
        spec: &DependencySpec,
        kind: &str,
        project_root: Option<&Path>,
        sdk_path_dependencies: &[DependencySpec],
    ) -> Self {
        let mut features = spec.features.clone();
        features.sort();
        features.dedup();

        Self {
            crate_name: spec.crate_name.clone(),
            kind: kind.to_string(),
            source: sdk_dependency_source_fingerprint(spec, sdk_path_dependencies)
                .unwrap_or_else(|| source_fingerprint(&spec.source, project_root)),
            version_req: spec.version.as_deref().map(normalize_version_req),
            default_features: spec.default_features,
            features,
            optional: spec.optional,
            package: spec.package.clone(),
        }
    }
}

/// Return a stable source coordinate for one dependency proven to come from the exact active SDK path catalog.
///
/// Provider and toolchain content identity already lives in the semantic lock state hashed beside this spec. Keeping
/// the source projection tied only to the typed catalog record avoids both physical cache paths and ambiguous
/// provider-name lookups when workspace members select different feature projections of the same provider.
fn sdk_dependency_source_fingerprint(
    spec: &DependencySpec,
    sdk_path_dependencies: &[DependencySpec],
) -> Option<String> {
    let DependencySource::Path { path } = &spec.source else {
        return None;
    };
    let owned_by_sdk = sdk_path_dependencies.iter().any(|candidate| {
        candidate.crate_name == spec.crate_name
            && candidate.package == spec.package
            && matches!(&candidate.source, DependencySource::Path { path: candidate_path } if dependency_paths_match(path, candidate_path))
    });
    if !owned_by_sdk {
        return None;
    }
    Some(format!(
        "sdk-path:{}?package={}",
        spec.crate_name,
        spec.package.as_deref().unwrap_or(&spec.crate_name)
    ))
}

/// Compare SDK delivery paths without requiring an artifact to remain present after cache relocation.
fn dependency_paths_match(left: &Path, right: &Path) -> bool {
    if left == right {
        return true;
    }
    match (fs::canonicalize(left), fs::canonicalize(right)) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

/// Produce a stable, portable string for a dependency source.
///
/// For `Path` sources, strips `project_root` when available so the fingerprint doesn't change across machines with
/// different absolute paths (RFC 013 Appendix A.2).
fn source_fingerprint(source: &DependencySource, project_root: Option<&Path>) -> String {
    match source {
        DependencySource::Registry => "registry".to_string(),
        DependencySource::Git { url, reference } => match reference {
            GitReference::Branch(branch) => format!("git:{url}#branch:{branch}"),
            GitReference::Tag(tag) => format!("git:{url}#tag:{tag}"),
            GitReference::Rev(rev) => format!("git:{url}#rev:{rev}"),
        },
        DependencySource::Path { path } => {
            let relative = project_root
                .and_then(|root| path.strip_prefix(root).ok())
                .unwrap_or(path);
            let normalized = normalize_relative_path_for_fingerprint(relative);
            format!("path:{}", normalized.to_string_lossy().replace('\\', "/"))
        }
    }
}

/// Normalize a relative path before adding it to a lockfile fingerprint.
fn normalize_relative_path_for_fingerprint(path: &Path) -> PathBuf {
    if path.is_absolute() {
        return path.to_path_buf();
    }

    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::Normal(segment) => normalized.push(segment),
            std::path::Component::ParentDir => normalized.push(".."),
            std::path::Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            std::path::Component::RootDir => normalized.push(component.as_os_str()),
        }
    }
    normalized
}

fn normalize_version_req(input: &str) -> String {
    input.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    const PUBLICATION_LOCK_HELPER_MODE_ENV: &str = "INCAN_TEST_PUBLICATION_LOCK_HELPER_MODE";
    const PUBLICATION_LOCK_HELPER_PATH_ENV: &str = "INCAN_TEST_PUBLICATION_LOCK_HELPER_PATH";
    const PUBLICATION_LOCK_HELPER_PROBE_ENV: &str = "INCAN_TEST_PUBLICATION_LOCK_HELPER_PROBE";
    const PUBLICATION_LOCK_HELPER_READY_ENV: &str = "INCAN_TEST_PUBLICATION_LOCK_HELPER_READY";
    const PUBLICATION_LOCK_HELPER_RELEASE_ENV: &str = "INCAN_TEST_PUBLICATION_LOCK_HELPER_RELEASE";
    const PUBLICATION_LOCK_PROBE_CONTENDED: &str = "would-block";

    /// Process roles used to prove active and legacy publication-lock contention.
    #[derive(Debug, Clone, Copy)]
    enum PublicationLockHelperMode {
        ActiveHolder,
        LegacyHolder,
        ActiveContender,
        ActiveViaLegacyContender,
        LegacyContender,
    }

    impl PublicationLockHelperMode {
        /// Parse one helper role received through the child-process environment.
        fn parse(value: &str) -> Result<Self, Box<dyn std::error::Error>> {
            match value {
                "active-holder" => Ok(Self::ActiveHolder),
                "legacy-holder" => Ok(Self::LegacyHolder),
                "active-contender" => Ok(Self::ActiveContender),
                "active-via-legacy-contender" => Ok(Self::ActiveViaLegacyContender),
                "legacy-contender" => Ok(Self::LegacyContender),
                _ => Err(format!("unknown publication-lock helper mode `{value}`").into()),
            }
        }

        /// Return the stable child-process representation of this helper role.
        fn as_str(self) -> &'static str {
            match self {
                Self::ActiveHolder => "active-holder",
                Self::LegacyHolder => "legacy-holder",
                Self::ActiveContender => "active-contender",
                Self::ActiveViaLegacyContender => "active-via-legacy-contender",
                Self::LegacyContender => "legacy-contender",
            }
        }

        /// Return whether this helper owns its selected guard until the release marker appears.
        fn is_holder(self) -> bool {
            matches!(self, Self::ActiveHolder | Self::LegacyHolder)
        }

        /// Return whether this helper operates on the legacy sibling identity.
        fn uses_legacy_identity(self) -> bool {
            matches!(
                self,
                Self::LegacyHolder | Self::ActiveViaLegacyContender | Self::LegacyContender
            )
        }

        /// Return whether this helper acquires the complete active protocol after any contention probe.
        fn uses_active_protocol(self) -> bool {
            matches!(
                self,
                Self::ActiveHolder | Self::ActiveContender | Self::ActiveViaLegacyContender
            )
        }
    }

    /// Child process that is always released, terminated when necessary, and reaped when its guard leaves scope.
    struct PublicationLockHelperProcess {
        child: std::process::Child,
        release_path: PathBuf,
    }

    impl Drop for PublicationLockHelperProcess {
        fn drop(&mut self) {
            let _ = fs::write(&self.release_path, b"release");
            match self.child.try_wait() {
                Ok(Some(_)) => {}
                Ok(None) | Err(_) => {
                    let _ = self.child.kill();
                    let _ = self.child.wait();
                }
            }
        }
    }

    fn sample_spec(name: &str, features: Vec<&str>) -> DependencySpec {
        DependencySpec {
            crate_name: name.to_string(),
            version: Some("1.0".to_string()),
            features: features.into_iter().map(|f| f.to_string()).collect(),
            default_features: true,
            source: DependencySource::Registry,
            optional: false,
            package: None,
        }
    }

    /// Run the child side of deterministic cross-process publication-lock tests.
    #[test]
    fn publication_lock_process_helper() -> TestResult {
        let Some(raw_mode) = std::env::var_os(PUBLICATION_LOCK_HELPER_MODE_ENV) else {
            return Ok(());
        };
        let mode = PublicationLockHelperMode::parse(&raw_mode.to_string_lossy())?;
        let lock_path = required_helper_path(PUBLICATION_LOCK_HELPER_PATH_ENV)?;
        let probe_path = required_helper_path(PUBLICATION_LOCK_HELPER_PROBE_ENV)?;
        let ready_path = required_helper_path(PUBLICATION_LOCK_HELPER_READY_ENV)?;
        let release_path = required_helper_path(PUBLICATION_LOCK_HELPER_RELEASE_ENV)?;

        if mode.is_holder() {
            let (_legacy_guard, _active_guard) = acquire_publication_lock_helper_guard(mode, &lock_path)?;
            fs::write(&ready_path, b"ready")?;
            wait_for_helper_path(&release_path, std::time::Duration::from_secs(10))?;
            return Ok(());
        }

        let identity_path = publication_lock_helper_identity_path(mode, &lock_path)?;
        let probe = OpenOptions::new().read(true).write(true).open(identity_path)?;
        let probe_result = match probe.try_lock() {
            Ok(()) => "acquired".to_string(),
            Err(std::fs::TryLockError::WouldBlock) => PUBLICATION_LOCK_PROBE_CONTENDED.to_string(),
            Err(std::fs::TryLockError::Error(error)) => format!("error:{error}"),
        };
        publish_publication_lock_helper_result(&probe_path, probe_result.as_bytes())?;
        if probe_result != PUBLICATION_LOCK_PROBE_CONTENDED {
            return Err(format!("publication-lock contention probe unexpectedly reported `{probe_result}`").into());
        }
        drop(probe);

        wait_for_helper_path(&release_path, std::time::Duration::from_secs(10))?;
        let (_legacy_guard, _active_guard) = acquire_publication_lock_helper_guard(mode, &lock_path)?;
        fs::write(&ready_path, b"ready")?;
        Ok(())
    }

    /// Acquire the identity selected for one holder process and retain the resulting guard shape.
    fn acquire_publication_lock_helper_guard(
        mode: PublicationLockHelperMode,
        lock_path: &Path,
    ) -> Result<(Option<File>, Option<PublicationLock>), Box<dyn std::error::Error>> {
        if !mode.uses_active_protocol() {
            let file = OpenOptions::new()
                .read(true)
                .write(true)
                .open(legacy_publication_lock_path(lock_path)?)?;
            file.lock()?;
            Ok((Some(file), None))
        } else {
            Ok((None, Some(acquire_publication_lock(lock_path)?)))
        }
    }

    /// Return the exact active or legacy inode that a contender must probe.
    fn publication_lock_helper_identity_path(mode: PublicationLockHelperMode, lock_path: &Path) -> io::Result<PathBuf> {
        if mode.uses_legacy_identity() {
            legacy_publication_lock_path(lock_path)
        } else {
            publication_lock_path(lock_path)
        }
    }

    /// Read one required helper path from the child-process environment.
    fn required_helper_path(key: &str) -> Result<PathBuf, Box<dyn std::error::Error>> {
        std::env::var_os(key)
            .map(PathBuf::from)
            .ok_or_else(|| format!("missing required publication-lock helper environment variable {key}").into())
    }

    /// Publish a helper result atomically so path existence also proves that its complete contents are readable.
    fn publish_publication_lock_helper_result(path: &Path, contents: &[u8]) -> io::Result<()> {
        let staging_path = path.with_extension("partial");
        fs::write(&staging_path, contents)?;
        fs::rename(staging_path, path)
    }

    /// Wait for one helper-process synchronization path with a bounded timeout.
    fn wait_for_helper_path(path: &Path, timeout: std::time::Duration) -> TestResult {
        let started = std::time::Instant::now();
        while !path.exists() {
            if started.elapsed() >= timeout {
                return Err(format!("timed out waiting for publication-lock helper path {}", path.display()).into());
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        Ok(())
    }

    /// Spawn this unit-test binary as one OS-process publication-lock helper.
    fn spawn_publication_lock_helper(
        mode: PublicationLockHelperMode,
        lock_path: &Path,
        probe_path: &Path,
        ready_path: &Path,
        release_path: &Path,
    ) -> Result<PublicationLockHelperProcess, Box<dyn std::error::Error>> {
        let child = std::process::Command::new(std::env::current_exe()?)
            .args([
                "--exact",
                "lockfile::tests::publication_lock_process_helper",
                "--nocapture",
            ])
            .env(PUBLICATION_LOCK_HELPER_MODE_ENV, mode.as_str())
            .env(PUBLICATION_LOCK_HELPER_PATH_ENV, lock_path)
            .env(PUBLICATION_LOCK_HELPER_PROBE_ENV, probe_path)
            .env(PUBLICATION_LOCK_HELPER_READY_ENV, ready_path)
            .env(PUBLICATION_LOCK_HELPER_RELEASE_ENV, release_path)
            .spawn()?;
        Ok(PublicationLockHelperProcess {
            child,
            release_path: release_path.to_path_buf(),
        })
    }

    /// Require one child helper to exit successfully within a bounded interval.
    fn wait_for_helper_success(
        process: &mut PublicationLockHelperProcess,
        timeout: std::time::Duration,
        context: &str,
    ) -> TestResult {
        let started = std::time::Instant::now();
        loop {
            if let Some(status) = process.child.try_wait()? {
                return if status.success() {
                    Ok(())
                } else {
                    Err(format!("{context} failed with status {status}").into())
                };
            }
            if started.elapsed() >= timeout {
                let _ = process.child.kill();
                let _ = process.child.wait();
                return Err(format!("{context} did not exit within {timeout:?}").into());
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    /// Prove one holder and contender coordinate on the selected active or legacy identity without timing inference.
    fn assert_publication_lock_process_contention(
        holder_mode: PublicationLockHelperMode,
        contender_mode: PublicationLockHelperMode,
        create_legacy_identity: bool,
    ) -> TestResult {
        let project = tempfile::tempdir()?;
        let lock_path = project.path().join("incan.lock");
        if create_legacy_identity {
            fs::write(legacy_publication_lock_path(&lock_path)?, [])?;
        }

        let holder_probe = project.path().join("holder.probe");
        let holder_ready = project.path().join("holder.ready");
        let contender_probe = project.path().join("contender.probe");
        let contender_ready = project.path().join("contender.ready");
        let release = project.path().join("release");

        let mut holder =
            spawn_publication_lock_helper(holder_mode, &lock_path, &holder_probe, &holder_ready, &release)?;
        wait_for_helper_path(&holder_ready, std::time::Duration::from_secs(10))?;

        let mut contender =
            spawn_publication_lock_helper(contender_mode, &lock_path, &contender_probe, &contender_ready, &release)?;
        wait_for_helper_path(&contender_probe, std::time::Duration::from_secs(10))?;
        let observed_probe = fs::read_to_string(&contender_probe)?;
        fs::write(&release, b"release")?;

        let holder_result = wait_for_helper_success(
            &mut holder,
            std::time::Duration::from_secs(10),
            "publication-lock holder",
        );
        let contender_result = wait_for_helper_success(
            &mut contender,
            std::time::Duration::from_secs(10),
            "publication-lock contender",
        );
        holder_result?;
        contender_result?;
        if observed_probe != PUBLICATION_LOCK_PROBE_CONTENDED {
            return Err(format!("expected a would-block contention probe, found `{observed_probe}`").into());
        }
        if !contender_ready.is_file() {
            return Err("the contender did not acquire the complete publication guard after release".into());
        }
        if !publication_lock_path(&lock_path)?.is_file() {
            return Err("the active compiler-owned publication guard was not created".into());
        }
        Ok(())
    }

    /// Prove two new compiler processes contend on the same hidden advisory-lock inode.
    #[test]
    fn publication_lock_blocks_a_second_process_until_release() -> TestResult {
        assert_publication_lock_process_contention(
            PublicationLockHelperMode::ActiveHolder,
            PublicationLockHelperMode::ActiveContender,
            false,
        )
    }

    /// Prove a new compiler waits on a pre-existing lock held through the legacy protocol.
    #[test]
    fn publication_lock_coordinates_with_an_existing_legacy_process() -> TestResult {
        assert_publication_lock_process_contention(
            PublicationLockHelperMode::LegacyHolder,
            PublicationLockHelperMode::ActiveViaLegacyContender,
            true,
        )
    }

    /// Prove a new compiler retains the legacy descriptor until its active critical section finishes.
    #[test]
    fn publication_lock_retains_legacy_guard_against_an_older_contender() -> TestResult {
        assert_publication_lock_process_contention(
            PublicationLockHelperMode::ActiveHolder,
            PublicationLockHelperMode::LegacyContender,
            true,
        )
    }

    fn sdk_path_spec(name: &str, path: &Path) -> DependencySpec {
        DependencySpec {
            crate_name: name.to_string(),
            version: None,
            features: Vec::new(),
            default_features: false,
            source: DependencySource::Path {
                path: path.to_path_buf(),
            },
            optional: false,
            package: None,
        }
    }

    /// Inputs that define one source-checkout-independent SDK semantic state.
    struct ProductionToolchainSemanticFixture {
        specs: Vec<DependencySpec>,
        provider_plan: ProviderPlan,
        inventory: SdkInventory,
        components: ResolvedSdkComponents,
    }

    fn production_toolchain_semantic_fixture(
        checkout: &Path,
    ) -> Result<ProductionToolchainSemanticFixture, Box<dyn std::error::Error>> {
        production_toolchain_semantic_fixture_with_native_output(checkout, "pub fn support() {}\n", false, 'a')
    }

    /// Build one SDK fixture whose physical provider output can vary independently of its authored semantic input.
    fn production_toolchain_semantic_fixture_with_native_output(
        checkout: &Path,
        generated_source: &str,
        host_abi: bool,
        source_digest_digit: char,
    ) -> Result<ProductionToolchainSemanticFixture, Box<dyn std::error::Error>> {
        let derive_root = checkout.join("crates/incan_derive");
        fs::create_dir_all(derive_root.join("src"))?;
        fs::write(
            derive_root.join("Cargo.toml"),
            "[package]\nname = \"incan_derive\"\nversion = \"0.5.0\"\n",
        )?;
        fs::write(derive_root.join("src/lib.rs"), "pub fn derive_marker() {}\n")?;

        let provider_root = checkout.join("sdk/components/support-provider");
        fs::create_dir_all(provider_root.join("src"))?;
        fs::write(
            provider_root.join("Cargo.toml"),
            format!(
                "[package]\nname = \"support_provider\"\nversion = \"0.5.0\"\n\n[dependencies]\nincan_derive = {{ path = \"{}\" }}\n",
                derive_root.display()
            ),
        )?;
        fs::write(provider_root.join("src/lib.rs"), generated_source)?;
        let manifest_path = provider_root.join("support_provider.incnlib");
        let mut manifest = crate::library_manifest::LibraryManifest::new("support_provider", "0.5.0");
        manifest.contract_metadata.provider.semantic_source_digest =
            Some(format!("sha256:{}", source_digest_digit.to_string().repeat(64)));
        if host_abi {
            manifest.rust_abi = Some(crate::library_manifest::LibraryRustAbi {
                schema_version: crate::library_manifest::RUST_ABI_SCHEMA_VERSION,
                items: Vec::new(),
            });
        }
        manifest.contract_metadata.provider.implementation_facets.push(
            crate::library_manifest::ProviderImplementationFacet {
                id: "derive-support".to_string(),
                required_modules: BTreeSet::new(),
                required_features: BTreeSet::new(),
                cargo_features: BTreeMap::new(),
                cargo_dependencies: vec![crate::library_manifest::ProviderCargoDependency {
                    crate_name: "incan_derive".to_string(),
                    package: None,
                    version: None,
                    features: BTreeSet::new(),
                    default_features: false,
                    source: crate::library_manifest::ProviderCargoDependencySource::Toolchain {
                        relative_path: "crates/incan_derive".to_string(),
                    },
                }],
            },
        );
        manifest.write_to_path(&manifest_path)?;
        let physical_digest = crate::library_manifest::digest_provider_artifact(&provider_root)?;
        let artifact = crate::frontend::library_manifest_index::LibraryArtifactMetadata::from_manifest_path(
            "support_provider",
            "support_provider",
            manifest_path.clone(),
            provider_root.clone(),
        );
        let provider = ProviderRecord {
            identity: crate::provider::ProviderIdentity {
                name: "support_provider".to_string(),
                version: "0.5.0".to_string(),
                digest: physical_digest.clone(),
                feature_projection: BTreeSet::new(),
            },
            provenance: ProviderProvenance::Sdk {
                sdk_identity: "incan@0.5.0".to_string(),
                component_id: "support".to_string(),
                inventory_path: None,
            },
            authority: crate::provider::NamespaceAuthority::SdkReserved,
            namespace_claims: BTreeSet::new(),
            available: true,
            enabled: true,
            manifest: Some(std::sync::Arc::new(manifest)),
            artifact: Some(artifact),
            implementation_facets: Vec::new(),
        };
        let provider_plan = ProviderPlan::new(
            crate::frontend::library_manifest_index::LibraryManifestIndex::default(),
            vec![provider],
            std::iter::empty::<Vec<String>>(),
        )?;
        let inventory = SdkInventory {
            root: checkout.join("sdk"),
            sdk_id: "incan".to_string(),
            sdk_version: "0.5.0".to_string(),
            compiler_requirement: "^0.5".to_string(),
            provider_codegen_revision: crate::version::SDK_PROVIDER_CODEGEN_REVISION,
            components: BTreeMap::from([(
                "support".to_string(),
                crate::provider::SdkComponent {
                    id: "support".to_string(),
                    version: "0.5.0".to_string(),
                    mandatory: false,
                    available: true,
                    dependencies: BTreeSet::new(),
                    providers: vec![crate::provider::SdkProviderDescriptor {
                        name: "support_provider".to_string(),
                        version: "0.5.0".to_string(),
                        digest: physical_digest,
                        namespace_claims: BTreeSet::new(),
                        manifest_path: Some(manifest_path),
                        crate_root: Some(provider_root),
                    }],
                },
            )]),
            profiles: BTreeMap::from([("default".to_string(), BTreeSet::from(["support".to_string()]))]),
        };
        let components = ResolvedSdkComponents {
            sdk_identity: "incan@0.5.0".to_string(),
            profile: "default".to_string(),
            enabled: BTreeSet::from(["support".to_string()]),
            unavailable: BTreeSet::new(),
            reasons: BTreeMap::from([(
                "support".to_string(),
                ComponentSelectionReason::Profile {
                    profile: "default".to_string(),
                },
            )]),
        };
        Ok(ProductionToolchainSemanticFixture {
            specs: vec![sdk_path_spec("incan_derive", &derive_root)],
            provider_plan,
            inventory,
            components,
        })
    }

    #[test]
    fn fingerprint_is_stable_across_feature_order() {
        let deps = vec![sample_spec("alpha", vec!["b", "a"])];
        let deps_reordered = vec![sample_spec("alpha", vec!["a", "b"])];
        let selection = CargoFeatureSelection::default();

        let first = compute_deps_fingerprint(&deps, &[], &selection, None);
        let second = compute_deps_fingerprint(&deps_reordered, &[], &selection, None);
        assert_eq!(first, second);
    }

    #[test]
    fn sdk_provider_fingerprint_uses_semantic_identity_across_relocated_stores_issue921() -> TestResult {
        let temp = tempfile::tempdir()?;
        let first_path = temp.path().join("provider-home-a/components/stdlib-core");
        let second_path = temp.path().join("provider-home-b/components/stdlib-core");
        let first = sdk_path_spec("incan_stdlib_core", &first_path);
        let second = sdk_path_spec("incan_stdlib_core", &second_path);
        let semantic = SemanticLockState {
            providers: vec![LockedProvider {
                identity: "incan_stdlib_core@0.5.0#sha256:stable[]".to_string(),
                participation: "used".to_string(),
                namespace_claims: BTreeSet::new(),
                used_modules: BTreeSet::new(),
                implementation_facets: Vec::new(),
                backend_requirements: BTreeSet::new(),
            }],
            ..SemanticLockState::default()
        };
        let selection = CargoFeatureSelection::default();

        let first_fingerprint = compute_resolved_fingerprint_with_sdk_paths(
            std::slice::from_ref(&first),
            &[],
            &selection,
            Some(temp.path()),
            &semantic,
            std::slice::from_ref(&first),
        );
        let second_fingerprint = compute_resolved_fingerprint_with_sdk_paths(
            std::slice::from_ref(&second),
            &[],
            &selection,
            Some(temp.path()),
            &semantic,
            std::slice::from_ref(&second),
        );
        assert_eq!(first_fingerprint, second_fingerprint);

        let first_lock = IncanLock::new_with_semantic(
            first_fingerprint,
            selection.clone(),
            semantic.clone(),
            "version = 4\n".to_string(),
        );
        let second_lock = IncanLock::new_with_semantic(
            second_fingerprint,
            selection.clone(),
            semantic.clone(),
            "version = 4\n".to_string(),
        );
        let first_lock_path = temp.path().join("first/incan.lock");
        let second_lock_path = temp.path().join("second/incan.lock");
        fs::create_dir_all(first_lock_path.parent().ok_or("first lock path has no parent")?)?;
        fs::create_dir_all(second_lock_path.parent().ok_or("second lock path has no parent")?)?;
        first_lock.write(&first_lock_path)?;
        second_lock.write(&second_lock_path)?;
        assert_eq!(fs::read(first_lock_path)?, fs::read(second_lock_path)?);

        let ambiguous_semantic = SemanticLockState {
            providers: vec![
                LockedProvider {
                    identity: "incan_stdlib_core@0.5.0#sha256:stable[feature-a]".to_string(),
                    participation: "used".to_string(),
                    namespace_claims: BTreeSet::new(),
                    used_modules: BTreeSet::new(),
                    implementation_facets: Vec::new(),
                    backend_requirements: BTreeSet::new(),
                },
                LockedProvider {
                    identity: "incan_stdlib_core@0.5.0#sha256:stable[feature-b]".to_string(),
                    participation: "used".to_string(),
                    namespace_claims: BTreeSet::new(),
                    used_modules: BTreeSet::new(),
                    implementation_facets: Vec::new(),
                    backend_requirements: BTreeSet::new(),
                },
            ],
            ..SemanticLockState::default()
        };
        let ambiguous_first = compute_resolved_fingerprint_with_sdk_paths(
            std::slice::from_ref(&first),
            &[],
            &selection,
            Some(temp.path()),
            &ambiguous_semantic,
            std::slice::from_ref(&first),
        );
        let ambiguous_second = compute_resolved_fingerprint_with_sdk_paths(
            std::slice::from_ref(&second),
            &[],
            &selection,
            Some(temp.path()),
            &ambiguous_semantic,
            std::slice::from_ref(&second),
        );
        assert_eq!(ambiguous_first, ambiguous_second);
        let mut changed_ambiguous = ambiguous_semantic.clone();
        changed_ambiguous.providers[1].identity = "incan_stdlib_core@0.5.0#sha256:changed[feature-b]".to_string();
        assert_ne!(
            ambiguous_second,
            compute_resolved_fingerprint_with_sdk_paths(
                std::slice::from_ref(&second),
                &[],
                &selection,
                Some(temp.path()),
                &changed_ambiguous,
                std::slice::from_ref(&second),
            )
        );

        let changed_semantic = SemanticLockState {
            providers: vec![LockedProvider {
                identity: "incan_stdlib_core@0.5.0#sha256:changed[]".to_string(),
                participation: "used".to_string(),
                namespace_claims: BTreeSet::new(),
                used_modules: BTreeSet::new(),
                implementation_facets: Vec::new(),
                backend_requirements: BTreeSet::new(),
            }],
            ..SemanticLockState::default()
        };
        let changed_fingerprint = compute_resolved_fingerprint_with_sdk_paths(
            std::slice::from_ref(&second),
            &[],
            &selection,
            Some(temp.path()),
            &changed_semantic,
            std::slice::from_ref(&second),
        );
        assert_ne!(second_lock.deps_fingerprint, changed_fingerprint);

        let ordinary_first = compute_resolved_fingerprint(&[first], &[], &selection, Some(temp.path()), &semantic);
        let ordinary_second = compute_resolved_fingerprint(&[second], &[], &selection, Some(temp.path()), &semantic);
        assert_ne!(ordinary_first, ordinary_second);
        Ok(())
    }

    #[test]
    fn sdk_toolchain_fingerprint_tracks_content_not_source_checkout_path_issue921() -> TestResult {
        let temp = tempfile::tempdir()?;
        let first_checkout = temp.path().join("source-checkout-a");
        let second_checkout = temp.path().join("source-checkout-b");
        let ProductionToolchainSemanticFixture {
            specs: first_specs,
            provider_plan: first_plan,
            inventory: first_inventory,
            components: first_components,
        } = production_toolchain_semantic_fixture(&first_checkout)?;
        let ProductionToolchainSemanticFixture {
            specs: second_specs,
            provider_plan: second_plan,
            inventory: second_inventory,
            components: second_components,
        } = production_toolchain_semantic_fixture(&second_checkout)?;
        let first_semantic = semantic_lock_state(
            &first_checkout,
            Some(&first_inventory),
            Some(&first_components),
            None,
            &first_plan,
            &first_specs,
        )?;
        let second_semantic = semantic_lock_state(
            &second_checkout,
            Some(&second_inventory),
            Some(&second_components),
            None,
            &second_plan,
            &second_specs,
        )?;
        assert_eq!(first_semantic, second_semantic);
        let selection = CargoFeatureSelection::default();
        let first_fingerprint = compute_resolved_fingerprint_with_sdk_paths(
            &first_specs,
            &[],
            &selection,
            Some(&first_checkout),
            &first_semantic,
            &first_specs,
        );
        let second_fingerprint = compute_resolved_fingerprint_with_sdk_paths(
            &second_specs,
            &[],
            &selection,
            Some(&second_checkout),
            &second_semantic,
            &second_specs,
        );
        assert_eq!(first_fingerprint, second_fingerprint);

        fs::write(
            second_checkout.join("crates/incan_derive/src/lib.rs"),
            "pub fn derive_marker() { changed(); }\n",
        )?;
        let changed_semantic = semantic_lock_state(
            &second_checkout,
            Some(&second_inventory),
            Some(&second_components),
            None,
            &second_plan,
            &second_specs,
        )?;
        assert_ne!(second_semantic, changed_semantic);
        let changed_fingerprint = compute_resolved_fingerprint_with_sdk_paths(
            &second_specs,
            &[],
            &selection,
            Some(&second_checkout),
            &changed_semantic,
            &second_specs,
        );
        assert_ne!(second_fingerprint, changed_fingerprint);
        Ok(())
    }

    #[test]
    fn sdk_semantic_state_and_fingerprint_ignore_native_provider_outputs_issue931() -> TestResult {
        let temp = tempfile::tempdir()?;
        let first_checkout = temp.path().join("macos-source");
        let second_checkout = temp.path().join("linux-source");
        let first = production_toolchain_semantic_fixture_with_native_output(
            &first_checkout,
            "pub fn host_marker() -> &'static str { \"macos\" }\n",
            false,
            'a',
        )?;
        let second = production_toolchain_semantic_fixture_with_native_output(
            &second_checkout,
            "pub fn host_marker() -> &'static str { \"linux\" }\n",
            true,
            'a',
        )?;
        let first_physical = first
            .inventory
            .components
            .get("support")
            .and_then(|component| component.providers.first())
            .ok_or("first fixture did not publish the support provider")?;
        let second_physical = second
            .inventory
            .components
            .get("support")
            .and_then(|component| component.providers.first())
            .ok_or("second fixture did not publish the support provider")?;
        assert_ne!(
            first_physical.digest, second_physical.digest,
            "the regression requires distinct physical native artifacts"
        );

        let first_semantic = semantic_lock_state(
            &first_checkout,
            Some(&first.inventory),
            Some(&first.components),
            None,
            &first.provider_plan,
            &first.specs,
        )?;
        let second_semantic = semantic_lock_state(
            &second_checkout,
            Some(&second.inventory),
            Some(&second.components),
            None,
            &second.provider_plan,
            &second.specs,
        )?;
        assert_eq!(first_semantic, second_semantic);
        assert_eq!(first_semantic.sdk, second_semantic.sdk);

        let selection = CargoFeatureSelection::default();
        let first_fingerprint = compute_resolved_fingerprint_with_sdk_paths(
            &first.specs,
            &[],
            &selection,
            Some(&first_checkout),
            &first_semantic,
            &first.specs,
        );
        let second_fingerprint = compute_resolved_fingerprint_with_sdk_paths(
            &second.specs,
            &[],
            &selection,
            Some(&second_checkout),
            &second_semantic,
            &second.specs,
        );
        assert_eq!(first_fingerprint, second_fingerprint);

        let changed = production_toolchain_semantic_fixture_with_native_output(
            &temp.path().join("changed-source"),
            "pub fn host_marker() -> &'static str { \"linux\" }\n",
            true,
            'b',
        )?;
        let changed_semantic = semantic_lock_state(
            &temp.path().join("changed-source"),
            Some(&changed.inventory),
            Some(&changed.components),
            None,
            &changed.provider_plan,
            &changed.specs,
        )?;
        assert_ne!(second_semantic, changed_semantic);
        assert_ne!(
            second_fingerprint,
            compute_resolved_fingerprint_with_sdk_paths(
                &changed.specs,
                &[],
                &selection,
                Some(&temp.path().join("changed-source")),
                &changed_semantic,
                &changed.specs,
            )
        );
        Ok(())
    }

    #[test]
    fn sdk_inventory_digest_uses_provider_semantics_not_physical_artifact_digest_issue921() -> TestResult {
        let inventory = |root: &Path, physical_digest: &str| SdkInventory {
            root: root.to_path_buf(),
            sdk_id: "incan".to_string(),
            sdk_version: "0.5.0".to_string(),
            compiler_requirement: "^0.5".to_string(),
            provider_codegen_revision: crate::version::SDK_PROVIDER_CODEGEN_REVISION,
            components: BTreeMap::from([(
                "stdlib-data".to_string(),
                crate::provider::SdkComponent {
                    id: "stdlib-data".to_string(),
                    version: "0.5.0".to_string(),
                    mandatory: false,
                    available: true,
                    dependencies: BTreeSet::new(),
                    providers: vec![crate::provider::SdkProviderDescriptor {
                        name: "incan_stdlib_data".to_string(),
                        version: "0.5.0".to_string(),
                        digest: physical_digest.to_string(),
                        namespace_claims: BTreeSet::from([vec!["std".to_string(), "regex".to_string()]]),
                        manifest_path: Some(root.join("components/stdlib-data/incan_stdlib_data.incnlib")),
                        crate_root: Some(root.join("components/stdlib-data")),
                    }],
                },
            )]),
            profiles: BTreeMap::from([("default".to_string(), BTreeSet::from(["stdlib-data".to_string()]))]),
        };
        let provider = ProviderRecord {
            identity: crate::provider::ProviderIdentity {
                name: "incan_stdlib_data".to_string(),
                version: "0.5.0".to_string(),
                digest: "sha256:physical-a".to_string(),
                feature_projection: BTreeSet::new(),
            },
            provenance: ProviderProvenance::Sdk {
                sdk_identity: "incan@0.5.0".to_string(),
                component_id: "stdlib-data".to_string(),
                inventory_path: None,
            },
            authority: crate::provider::NamespaceAuthority::SdkReserved,
            namespace_claims: BTreeSet::new(),
            available: true,
            enabled: true,
            manifest: None,
            artifact: None,
            implementation_facets: Vec::new(),
        };
        let semantic_identity = "incan_stdlib_data@0.5.0#sha256:semantic[]".to_string();
        let first = inventory(Path::new("/provider-home-a"), "sha256:physical-a");
        let second = inventory(Path::new("/provider-home-b"), "sha256:physical-b");
        assert_eq!(
            semantic_sdk_inventory_digest(&first, &[(&provider, semantic_identity.clone())])?,
            semantic_sdk_inventory_digest(&second, &[(&provider, semantic_identity.clone())])?
        );
        assert_ne!(
            semantic_sdk_inventory_digest(&second, &[(&provider, semantic_identity)])?,
            semantic_sdk_inventory_digest(
                &second,
                &[(&provider, "incan_stdlib_data@0.5.0#sha256:changed[]".to_string())],
            )?
        );
        Ok(())
    }

    #[test]
    fn portable_project_path_normalizes_relative_and_absolute_roots() -> TestResult {
        let current_dir = std::env::current_dir()?;

        assert_eq!(portable_project_path(Path::new("."), &current_dir), "");
        Ok(())
    }

    #[test]
    fn workspace_semantic_lock_state_rebases_member_paths_to_workspace_root() -> TestResult {
        let fixture = tempfile::tempdir()?;
        let workspace_root = fixture.path().join("root_lib");
        let consumer_root = workspace_root.join("consumer");
        let external = tempfile::tempdir()?;
        fs::create_dir_all(&consumer_root)?;

        let member_semantic = SemanticLockState {
            packages: vec![
                locked_package("consumer", ""),
                locked_package("root_lib", &workspace_root.to_string_lossy()),
                locked_package("external", &external.path().to_string_lossy()),
            ],
            feature_edges: vec![
                locked_feature_edge("", "root_lib", &workspace_root.to_string_lossy()),
                locked_feature_edge("", "external", &external.path().to_string_lossy()),
            ],
            ..SemanticLockState::default()
        };

        let state = workspace_semantic_lock_state(&workspace_root, [(consumer_root, member_semantic)])?;
        assert_eq!(state.workspace_members.len(), 1);
        let member = &state.workspace_members[0];
        assert_eq!(member.member_root, "consumer");
        assert_eq!(member.packages[0].project_root, "consumer");
        assert_eq!(member.packages[1].project_root, "");
        assert_eq!(
            member.packages[2].project_root,
            fs::canonicalize(external.path())?.to_string_lossy().replace('\\', "/")
        );
        assert_eq!(member.feature_edges[0].from, "consumer");
        assert_eq!(member.feature_edges[0].to, "");
        assert_eq!(
            member.feature_edges[1].to,
            fs::canonicalize(external.path())?.to_string_lossy().replace('\\', "/")
        );
        Ok(())
    }

    #[test]
    fn workspace_semantic_fingerprint_is_stable_after_relocation() -> TestResult {
        let first_fixture = tempfile::tempdir()?;
        let second_fixture = tempfile::tempdir()?;
        let first = relocated_workspace_semantic(first_fixture.path())?;
        let second = relocated_workspace_semantic(second_fixture.path())?;

        assert_eq!(first, second);
        let selection = CargoFeatureSelection::default();
        assert_eq!(
            compute_resolved_fingerprint(&[], &[], &selection, None, &first),
            compute_resolved_fingerprint(&[], &[], &selection, None, &second)
        );
        Ok(())
    }

    #[test]
    fn lockfile_round_trip() -> TestResult {
        let selection = CargoFeatureSelection {
            cargo_features: vec!["alpha".to_string()],
            cargo_no_default_features: false,
            cargo_all_features: false,
        };
        let lock = IncanLock::new(
            "sha256:deadbeef".to_string(),
            selection,
            "[[package]]\nname = \"x\"\n".to_string(),
        );

        let dir = tempfile::tempdir()?;
        let path = dir.path().join("incan.lock");
        lock.write(&path)?;

        let content = std::fs::read_to_string(&path)?;
        assert!(
            !content.contains("generated ="),
            "lockfiles should not contain volatile generation timestamps"
        );
        let loaded = IncanLock::load(&path)?;
        assert_eq!(loaded.deps_fingerprint, "sha256:deadbeef");
        assert_eq!(loaded.cargo_features.cargo_features, vec!["alpha".to_string()]);
        assert!(loaded.cargo_lock_payload.contains("package"));
        Ok(())
    }

    #[test]
    fn publication_lock_lives_in_compiler_owned_target_state() -> TestResult {
        let project = tempfile::tempdir()?;
        let lock_path = project.path().join("incan.lock");

        drop(acquire_publication_lock(&lock_path)?);
        drop(acquire_publication_lock(&lock_path)?);

        assert!(
            project
                .path()
                .join("target/incan_lock/.incan.lock.publication.lock")
                .is_file()
        );
        assert!(
            !project.path().join(".incan.lock.incan.lock").exists(),
            "lock publication must not create a persistent project-root sidecar"
        );
        Ok(())
    }

    #[test]
    fn publication_lock_does_not_unlink_a_legacy_lock_inode() -> TestResult {
        let project = tempfile::tempdir()?;
        let lock_path = project.path().join("incan.lock");
        let legacy_lock_path = project.path().join(".incan.lock.incan.lock");
        fs::write(&legacy_lock_path, [])?;

        drop(acquire_publication_lock(&lock_path)?);

        assert!(
            legacy_lock_path.is_file(),
            "a new compiler must not unlink an inode that an older compiler may still hold"
        );
        Ok(())
    }

    #[test]
    fn semantic_lock_state_round_trip_preserves_sdk_features_and_providers() -> TestResult {
        let semantic = sample_semantic_state();
        let lock = IncanLock::new_with_semantic(
            "sha256:semantic".to_string(),
            CargoFeatureSelection::default(),
            semantic.clone(),
            "payload".to_string(),
        );

        let dir = tempfile::tempdir()?;
        let path = dir.path().join("incan.lock");
        lock.write(&path)?;

        let loaded = IncanLock::load(&path)?;
        assert_eq!(loaded.format, LOCKFILE_FORMAT_VERSION);
        assert_eq!(loaded.semantic, semantic);
        Ok(())
    }

    #[test]
    fn resolved_fingerprint_changes_with_sdk_feature_or_provider_semantics() {
        let selection = CargoFeatureSelection::default();
        let baseline = sample_semantic_state();
        let baseline_fingerprint = compute_resolved_fingerprint(&[], &[], &selection, None, &baseline);

        let mut sdk_changed = baseline.clone();
        if let Some(sdk) = &mut sdk_changed.sdk {
            sdk.profile = "full".to_string();
            sdk.components.push(LockedSdkComponent {
                id: "stdlib-web".to_string(),
                version: "0.5.0".to_string(),
                reason: "profile:full".to_string(),
            });
        }

        let mut features_changed = baseline.clone();
        features_changed.packages[0].active_features.insert("tls".to_string());

        let mut provider_changed = baseline.clone();
        provider_changed.providers[0].identity = "stdlib-data@0.5.0#sha256:changed[]".to_string();
        provider_changed.providers[0]
            .implementation_facets
            .push("json-serde".to_string());
        provider_changed.providers[0]
            .backend_requirements
            .insert("cargo-feature:incan_stdlib/serde".to_string());

        assert_ne!(
            baseline_fingerprint,
            compute_resolved_fingerprint(&[], &[], &selection, None, &sdk_changed)
        );
        assert_ne!(
            baseline_fingerprint,
            compute_resolved_fingerprint(&[], &[], &selection, None, &features_changed)
        );
        assert_ne!(
            baseline_fingerprint,
            compute_resolved_fingerprint(&[], &[], &selection, None, &provider_changed)
        );
    }

    #[test]
    fn legacy_generated_timestamp_is_accepted_on_load() -> TestResult {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("incan.lock");
        let legacy_toml = r#"
[incan]
format = 1
incan-version = "0.3.0-dev.23"
generated = "2026-04-27T13:41:45.845714Z"
deps-fingerprint = "sha256:abc"
cargo-features = []
cargo-no-default-features = false
cargo-all-features = false

[cargo]
lock = "payload"
"#;
        std::fs::write(&path, legacy_toml)?;

        let lock = IncanLock::load(&path)?;
        assert_eq!(lock.deps_fingerprint, "sha256:abc");
        assert_eq!(lock.cargo_lock_payload, "payload\n");
        Ok(())
    }

    // ---- Phase 4: fingerprint changes when deps change ----

    #[test]
    fn fingerprint_changes_when_deps_differ() {
        let deps_a = vec![sample_spec("alpha", vec!["a"])];
        let deps_b = vec![sample_spec("alpha", vec!["a", "b"])];
        let selection = CargoFeatureSelection::default();

        let fp_a = compute_deps_fingerprint(&deps_a, &[], &selection, None);
        let fp_b = compute_deps_fingerprint(&deps_b, &[], &selection, None);
        assert_ne!(fp_a, fp_b, "fingerprints should differ when features differ");
    }

    #[test]
    fn path_dependency_fingerprint_normalizes_current_dir_segments() {
        let mut dep_plain = sample_spec("tiny_helper", vec![]);
        dep_plain.source = DependencySource::Path {
            path: PathBuf::from("rust/tiny_helper"),
        };
        let mut dep_current_dir = dep_plain.clone();
        dep_current_dir.source = DependencySource::Path {
            path: PathBuf::from("./rust/tiny_helper"),
        };
        let selection = CargoFeatureSelection::default();

        assert_eq!(
            compute_deps_fingerprint(&[dep_plain], &[], &selection, Some(Path::new("."))),
            compute_deps_fingerprint(&[dep_current_dir], &[], &selection, Some(Path::new("."))),
        );
    }

    // ---- Phase 4: stale fingerprint detection ----

    #[test]
    fn stale_fingerprint_is_detectable() {
        let selection = CargoFeatureSelection::default();
        let deps_v1 = vec![sample_spec("alpha", vec!["a"])];
        let fp_v1 = compute_deps_fingerprint(&deps_v1, &[], &selection, None);

        let lock = IncanLock::new(fp_v1.clone(), selection.clone(), "payload".to_string());

        // Simulate deps changing
        let deps_v2 = vec![sample_spec("alpha", vec!["a", "new_feature"])];
        let fp_v2 = compute_deps_fingerprint(&deps_v2, &[], &selection, None);

        assert_ne!(
            lock.deps_fingerprint, fp_v2,
            "lock fingerprint should not match updated deps"
        );
    }

    // ---- Phase 4: Cargo.lock materialization via ProjectGenerator ----

    #[test]
    fn cargo_lock_payload_materializes_in_project() -> TestResult {
        use std::fs;

        let temp_dir = tempfile::tempdir()?;
        let project_dir = temp_dir.path().join("test_lock_project");

        let mut generator = crate::backend::ProjectGenerator::new(&project_dir, "test_lock", true);
        generator.set_cargo_lock_payload(Some("[[package]]\nname = \"hello\"\nversion = \"0.1.0\"\n".to_string()));

        generator.generate("fn main() {}")?;

        let cargo_lock_path = project_dir.join("Cargo.lock");
        assert!(cargo_lock_path.exists(), "Cargo.lock should be written to project dir");
        let content = fs::read_to_string(&cargo_lock_path)?;
        assert!(
            content.contains("hello"),
            "Cargo.lock should contain the payload, got:\n{content}"
        );
        Ok(())
    }

    // ---- Phase 4: format version is validated on load ----

    #[test]
    fn lockfile_format_version_checked() -> TestResult {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("incan.lock");

        // Write a lockfile with an incompatible format version
        let bad_toml = r#"
[incan]
format = 999
incan-version = "0.1.0"
generated = "2025-01-01T00:00:00Z"
deps-fingerprint = "sha256:abc"
cargo-features = []
cargo-no-default-features = false
cargo-all-features = false

[cargo]
lock = "payload"
"#;
        std::fs::write(&path, bad_toml)?;
        let result = IncanLock::load(&path);
        assert!(result.is_err(), "loading a future format version should fail");
        Ok(())
    }

    // ---- CargoFeatureSelection normalization ----

    #[test]
    fn cargo_feature_selection_normalized() {
        let sel = CargoFeatureSelection {
            cargo_features: vec!["b".to_string(), "a".to_string(), "a".to_string()],
            cargo_no_default_features: false,
            cargo_all_features: false,
        };
        let norm = sel.normalized();
        assert_eq!(norm.cargo_features, vec!["a".to_string(), "b".to_string()]);
    }

    /// Build one minimal package-feature snapshot using the supplied member-local coordinate.
    fn locked_package(package: &str, project_root: &str) -> LockedPackageFeatures {
        LockedPackageFeatures {
            package: package.to_string(),
            project_root: project_root.to_string(),
            active_features: BTreeSet::new(),
            active_optional_dependencies: BTreeSet::new(),
            dependency_features: BTreeMap::new(),
            required_sdk_components: BTreeSet::new(),
        }
    }

    /// Build one minimal feature edge using member-local source and target coordinates.
    fn locked_feature_edge(from: &str, dependency_key: &str, to: &str) -> LockedFeatureEdge {
        LockedFeatureEdge {
            from: from.to_string(),
            dependency_key: dependency_key.to_string(),
            to: to.to_string(),
            requested_features: BTreeSet::new(),
            default_features: true,
            optional: false,
        }
    }

    /// Create the same rooted workspace semantic graph under an arbitrary filesystem location.
    fn relocated_workspace_semantic(fixture_root: &Path) -> Result<SemanticLockState, Box<dyn std::error::Error>> {
        let workspace_root = fixture_root.join("root_lib");
        let consumer_root = workspace_root.join("consumer");
        fs::create_dir_all(&consumer_root)?;
        let member_semantic = SemanticLockState {
            packages: vec![
                locked_package("consumer", ""),
                locked_package("root_lib", &workspace_root.to_string_lossy()),
            ],
            feature_edges: vec![locked_feature_edge("", "root_lib", &workspace_root.to_string_lossy())],
            ..SemanticLockState::default()
        };

        workspace_semantic_lock_state(&workspace_root, [(consumer_root, member_semantic)]).map_err(|error| error.into())
    }

    fn sample_semantic_state() -> SemanticLockState {
        SemanticLockState {
            sdk: Some(LockedSdkState {
                identity: "incan@0.5.0".to_string(),
                inventory_digest: "sha256:inventory".to_string(),
                profile: "default".to_string(),
                components: vec![LockedSdkComponent {
                    id: "stdlib-core".to_string(),
                    version: "0.5.0".to_string(),
                    reason: "mandatory".to_string(),
                }],
            }),
            packages: vec![LockedPackageFeatures {
                package: "consumer".to_string(),
                project_root: "".to_string(),
                active_features: BTreeSet::from(["json".to_string()]),
                active_optional_dependencies: BTreeSet::from(["codec".to_string()]),
                dependency_features: BTreeMap::from([("codec".to_string(), BTreeSet::from(["derive".to_string()]))]),
                required_sdk_components: BTreeSet::from(["stdlib-data".to_string()]),
            }],
            feature_edges: vec![LockedFeatureEdge {
                from: "".to_string(),
                dependency_key: "codec".to_string(),
                to: "../codec".to_string(),
                requested_features: BTreeSet::from(["derive".to_string()]),
                default_features: false,
                optional: true,
            }],
            providers: vec![LockedProvider {
                identity: "stdlib-data@0.5.0#sha256:data[]".to_string(),
                participation: "used".to_string(),
                namespace_claims: BTreeSet::from([vec!["std".to_string(), "json".to_string()]]),
                used_modules: BTreeSet::from([vec!["std".to_string(), "json".to_string()]]),
                implementation_facets: vec!["json-core".to_string()],
                backend_requirements: BTreeSet::from(["cargo-dependency:serde_json".to_string()]),
            }],
            workspace_members: Vec::new(),
        }
    }
}
