//! `oven.lock` parsing, validation, and fingerprinting.
//!
//! The lockfile records the selected semantic graph and its dependency fingerprint for strict builds.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{self, ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::manifest::{DependencySource, DependencySpec, GitReference};
use crate::oven_interop::{InteropCSection, LockedInteropTarget, locked_interop_targets_from_section};
use crate::provider::{
    BackendImplementationRequirement, ComponentSelectionReason, PackageFeaturePlan, ProviderParticipation,
    ProviderPlan, ProviderProvenance, ProviderRecord, ResolvedSdkComponents, SdkInventory,
};

/// The generated project lockfile's filename, resolved relative to a project or workspace root.
///
/// This names the *project* lock that records the resolved dependency, provider, and interop graph. RFC 117 makes
/// `oven.lock` that file and states plainly that `incan.lock` is not read afterwards, so there is no compatibility
/// path: a lock left behind by an older toolchain is inert state, not an input.
///
/// It is distinct from the artifact-store coordination lock and from the publication sibling this module derives from
/// a lock's own filename; renaming this constant must not be taken to rename either of those.
pub const LOCK_FILENAME: &str = "oven.lock";

const LOCKFILE_FORMAT_VERSION: u32 = 4;
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

#[derive(Debug, Clone)]
pub struct IncanLock {
    pub format: u32,
    pub incan_version: String,
    pub deps_fingerprint: String,
    pub semantic: SemanticLockState,
}

/// Advisory guards retained for one canonical lockfile publication critical section.
#[derive(Debug)]
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oven: Option<LockedOvenState>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oven: Option<LockedOvenState>,
}

/// Oven requirements retained in the semantic lock without claiming that a build has resolved them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LockedOvenState {
    /// Target-specific package inputs and compatibility requirements for checked interop.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub interop: Vec<LockedInteropTarget>,
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
    /// Workspace lock publication retains this guard while collecting checked facts and replacing the canonical file,
    /// so cooperating publishers cannot interleave those operations.
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
            },
            semantic: self.semantic.clone(),
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
    pub fn new(deps_fingerprint: String) -> Self {
        Self::new_with_semantic(deps_fingerprint, SemanticLockState::default())
    }

    /// Construct a lock from the dependency fingerprint and checked semantic provider facts.
    pub fn new_with_semantic(deps_fingerprint: String, semantic: SemanticLockState) -> Self {
        Self {
            format: LOCKFILE_FORMAT_VERSION,
            incan_version: crate::version::INCAN_VERSION.to_string(),
            deps_fingerprint,
            semantic,
        }
    }
}

/// Snapshot the shared provider, SDK-component, and package-feature plans into portable canonical lock state.
pub fn semantic_lock_state(
    project_root: &Path,
    interop: Option<&InteropCSection>,
    sdk_inventory: Option<&SdkInventory>,
    sdk_components: Option<&ResolvedSdkComponents>,
    package_features: Option<&PackageFeaturePlan>,
    provider_plan: &ProviderPlan,
    sdk_path_dependencies: &[DependencySpec],
) -> Result<SemanticLockState, String> {
    semantic_lock_state_with_provider_identities(
        project_root,
        interop,
        sdk_inventory,
        sdk_components,
        package_features,
        provider_plan,
        sdk_path_dependencies,
    )
    .map(|(semantic, _)| semantic)
}

/// Retain the exact provider identity projection used to construct this semantic lock state.
///
/// Native receipt construction for the same checked provider plan and SDK requirements can reuse this map without
/// repeating semantic source traversal. These identities do not replace physical provider admission or grant native
/// inputs.
pub(crate) fn semantic_lock_state_with_provider_identities(
    project_root: &Path,
    interop: Option<&InteropCSection>,
    sdk_inventory: Option<&SdkInventory>,
    sdk_components: Option<&ResolvedSdkComponents>,
    package_features: Option<&PackageFeaturePlan>,
    provider_plan: &ProviderPlan,
    sdk_path_dependencies: &[DependencySpec],
) -> Result<(SemanticLockState, BTreeMap<String, String>), String> {
    let interop = locked_interop_targets_from_section(project_root, interop)?;
    let oven = (!interop.is_empty()).then_some(LockedOvenState { interop });
    let provider_identity_map = provider_semantic_identities(provider_plan, sdk_path_dependencies)?;
    let provider_semantic_identities = provider_plan
        .records()
        .map(|provider| {
            let key = provider.identity.stable_key();
            let identity = provider_identity_map
                .get(&key)
                .cloned()
                .ok_or_else(|| format!("semantic provider identity is missing for `{key}`"))?;
            Ok((provider, identity))
        })
        .collect::<Result<Vec<_>, String>>()?;
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
    Ok((
        SemanticLockState {
            sdk,
            packages,
            feature_edges,
            providers,
            oven,
            workspace_members: Vec::new(),
        },
        provider_identity_map,
    ))
}

/// Refuse to mint provider semantic identities until checked Oven Rust-source evidence is supplied.
///
/// An empty provider plan has no identity work to perform. Every nonempty plan must pass through the typed RFC 123
/// projection; Cargo metadata and physical artifact paths are not valid semantic substitutes.
pub(crate) fn provider_semantic_identities(
    provider_plan: &ProviderPlan,
    _sdk_path_dependencies: &[DependencySpec],
) -> Result<BTreeMap<String, String>, String> {
    if provider_plan.records().next().is_none() {
        return Ok(BTreeMap::new());
    }
    Err(
        "provider semantic identities require the checked Oven Rust source projection; Cargo-derived provider identity is no longer accepted"
            .to_string(),
    )
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
            let key = (component_id.clone(), name, version);
            let identity = identities.get(&key).ok_or_else(|| {
                format!(
                    "SDK component `{}` provider {}@{} has no checked semantic identity",
                    key.0, key.1, key.2
                )
            })?;
            provider_object.insert("digest".to_string(), serde_json::Value::String(identity.clone()));
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
                oven: semantic.oven,
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
/// render with `..` traversal through [`portable_project_path`], so sibling coordinates stay machine-independent.
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

/// Render a project-relative path so semantic lock coordinates and fingerprints survive relocation.
///
/// A coordinate inside the project root renders without traversal, as before. A coordinate outside it — most
/// commonly a sibling provider such as `../producer` — renders with `..` components the way Cargo renders path
/// dependencies, so the same checkout produces the same lock bytes on every machine (#1226). Only when no relative
/// rendering exists (different Windows path prefixes, or a path that cannot be anchored) does the coordinate stay
/// absolute, which then genuinely is machine state.
fn portable_project_path(project_root: &Path, path: &Path) -> String {
    let canonical_root = fs::canonicalize(project_root);
    let canonical_path = fs::canonicalize(path);
    let both_canonical = canonical_root.is_ok() && canonical_path.is_ok();
    let normalized_root = canonical_root.unwrap_or_else(|_| project_root.to_path_buf());
    let normalized_path = canonical_path.unwrap_or_else(|_| path.to_path_buf());
    if let Ok(contained) = normalized_path.strip_prefix(&normalized_root) {
        return contained.to_string_lossy().replace('\\', "/");
    }
    // Traversal is meaningful only when both coordinates resolved in the same real namespace. A half-canonicalized
    // pair (one side missing on disk, or split across a symlink alias such as macOS's `/var` vs `/private/var`)
    // would walk across the alias boundary and render a traversal that resolves to the wrong place.
    if both_canonical && let Some(traversal) = relative_traversal_path(&normalized_root, &normalized_path) {
        return traversal;
    }
    normalized_path.to_string_lossy().replace('\\', "/")
}

/// Render `path` relative to `root` using `..` traversal, or `None` when the two share no common anchor.
///
/// Both inputs must be absolute for the component walk to be meaningful; prefix components (Windows drives) must
/// match exactly, since no traversal crosses drives.
fn relative_traversal_path(root: &Path, path: &Path) -> Option<String> {
    use std::path::Component;

    if !root.is_absolute() || !path.is_absolute() {
        return None;
    }
    let root_components = root.components().collect::<Vec<_>>();
    let path_components = path.components().collect::<Vec<_>>();
    if matches!(root_components.first(), Some(Component::Prefix(_)))
        || matches!(path_components.first(), Some(Component::Prefix(_)))
    {
        let (Some(Component::Prefix(root_prefix)), Some(Component::Prefix(path_prefix))) =
            (root_components.first(), path_components.first())
        else {
            return None;
        };
        if root_prefix != path_prefix {
            return None;
        }
    }
    let shared = root_components
        .iter()
        .zip(&path_components)
        .take_while(|(left, right)| left == right)
        .count();
    let mut rendered = Vec::new();
    rendered.resize(root_components.len() - shared, "..".to_string());
    rendered.extend(
        path_components[shared..]
            .iter()
            .map(|component| component.as_os_str().to_string_lossy().replace('\\', "/")),
    );
    Some(rendered.join("/"))
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
/// rather than beside `oven.lock` in the project root. When an older compiler has already created the legacy sibling
/// for this lock, new compilers acquire that inode first and retain it alongside the active guard, preserving
/// mixed-version exclusion without creating or unlinking legacy project-root state on clean projects.
///
/// Since the project lock's rename that sibling cannot exist for an `oven.lock` target — see
/// [`legacy_publication_lock_path`] — so the acquisition is a no-op there and mixed-version exclusion is instead
/// provided by the rename itself: a compiler from before it publishes `incan.lock` and never contends for this file.
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
///
/// The `.incan.lock` suffix is the toolchain's generic advisory-lock sidecar convention — `std.fs` derives the same
/// `.<name>.incan.lock` identity for any protected path — and not a spelling of the project lock, so it does not
/// follow the project lock's rename. The prefix does, because it comes from the caller's own file name. That is what
/// keeps the guard correct rather than merely inherited: for `oven.lock` it resolves to a path no compiler predating
/// this guard can have created, so the guard is inert exactly where mixed-version exclusion no longer has anything to
/// exclude — a compiler from before the rename publishes `incan.lock` and never contends for this target.
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

/// Compute a stable SHA-256 fingerprint over the effective dependency specs.
///
/// ## Parameters
///
/// - `project_root`: When provided, `path:` dependency sources are relativized to this directory so the fingerprint is
///   portable across machines (RFC 013 Appendix A.2).
pub fn compute_deps_fingerprint(
    dependencies: &[DependencySpec],
    dev_dependencies: &[DependencySpec],
    project_root: Option<&Path>,
) -> String {
    compute_resolved_fingerprint(
        dependencies,
        dev_dependencies,
        project_root,
        &SemanticLockState::default(),
    )
}

/// Compute the complete dependency and semantic-provider fingerprint for canonical lock freshness.
pub fn compute_resolved_fingerprint(
    dependencies: &[DependencySpec],
    dev_dependencies: &[DependencySpec],
    project_root: Option<&Path>,
    semantic: &SemanticLockState,
) -> String {
    compute_resolved_fingerprint_with_sdk_paths(dependencies, dev_dependencies, project_root, semantic, &[])
}

/// Compute lock freshness while replacing compiler-owned SDK delivery paths with their checked semantic identities.
///
/// SDK provider roots are immutable cache coordinates, not project dependency identities. The semantic state already
/// records each provider's name, version, artifact digest, and selected feature projection, so hashing the physical
/// path as well would make an equivalent relocated provider store appear to be a dependency change.
pub fn compute_resolved_fingerprint_with_sdk_paths(
    dependencies: &[DependencySpec],
    dev_dependencies: &[DependencySpec],
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
    let input = FingerprintInput { specs, semantic };
    let json = serde_json::to_string(&input).unwrap_or_else(|_| "{}".to_string());
    let mut hasher = Sha256::new();
    hasher.update(json.as_bytes());
    let hash = hasher.finalize();
    format!("sha256:{}", hex::encode(hash))
}

/// Parse and validate one lockfile while retaining its path in all diagnostics.
fn parse_lockfile(content: &str, path: &Path) -> Result<IncanLock, LockfileError> {
    let raw: RawIncanLock = toml::from_str(content).map_err(|e| LockfileError::Parse {
        path: path.to_path_buf(),
        source: e,
    })?;

    if raw.incan.format != LOCKFILE_FORMAT_VERSION {
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
        semantic: raw.semantic,
    })
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawIncanLock {
    incan: RawIncanMeta,
    #[serde(default)]
    semantic: SemanticLockState,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawIncanMeta {
    format: u32,
    #[serde(rename = "incan-version")]
    incan_version: String,
    #[serde(rename = "deps-fingerprint")]
    deps_fingerprint: String,
}

#[derive(Debug, Serialize)]
struct FingerprintInput<'a> {
    specs: Vec<SpecFingerprint>,
    semantic: &'a SemanticLockState,
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

    #[test]
    fn portable_project_path_renders_sibling_coordinates_with_traversal() -> TestResult {
        // A consumer's sibling provider is the ordinary workspace layout (#1226). The rendered coordinate must be
        // identical on every machine, so the lock's semantic fingerprint survives relocation and repeated bakes stop
        // re-dirtying tracked example locks with checkout-absolute paths.
        let fixture = tempfile::tempdir()?;
        let consumer = fixture.path().join("examples/vocab/consumer");
        let producer = fixture.path().join("examples/vocab/producer");
        fs::create_dir_all(&consumer)?;
        fs::create_dir_all(producer.join("src"))?;

        fs::create_dir_all(consumer.join("src"))?;

        assert_eq!(portable_project_path(&consumer, &producer), "../producer");
        assert_eq!(
            portable_project_path(&consumer, &producer.join("src")),
            "../producer/src"
        );
        assert_eq!(portable_project_path(&consumer, &consumer.join("src")), "src");
        assert_eq!(portable_project_path(&consumer, &consumer), "");
        let missing = fixture.path().join("examples/vocab/not-yet-created");
        assert!(
            Path::new(&portable_project_path(&consumer, &missing)).is_absolute(),
            "a coordinate that cannot canonicalize must stay absolute rather than risk an alias-crossing traversal"
        );
        Ok(())
    }

    #[test]
    fn relative_traversal_path_requires_absolute_anchors() {
        assert_eq!(
            relative_traversal_path(Path::new("relative/root"), Path::new("/absolute/path")),
            None
        );
        assert_eq!(
            relative_traversal_path(Path::new("/absolute/root"), Path::new("relative/path")),
            None
        );
        assert_eq!(
            relative_traversal_path(Path::new("/a/b/c"), Path::new("/a/x/y")).as_deref(),
            Some("../../x/y")
        );
    }

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
        let lock_path = project.path().join("oven.lock");
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

    #[test]
    fn fingerprint_is_stable_across_feature_order() {
        let deps = vec![sample_spec("alpha", vec!["b", "a"])];
        let deps_reordered = vec![sample_spec("alpha", vec!["a", "b"])];

        let first = compute_deps_fingerprint(&deps, &[], None);
        let second = compute_deps_fingerprint(&deps_reordered, &[], None);
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
        let first_fingerprint = compute_resolved_fingerprint_with_sdk_paths(
            std::slice::from_ref(&first),
            &[],
            Some(temp.path()),
            &semantic,
            std::slice::from_ref(&first),
        );
        let second_fingerprint = compute_resolved_fingerprint_with_sdk_paths(
            std::slice::from_ref(&second),
            &[],
            Some(temp.path()),
            &semantic,
            std::slice::from_ref(&second),
        );
        assert_eq!(first_fingerprint, second_fingerprint);

        let first_lock = IncanLock::new_with_semantic(first_fingerprint, semantic.clone());
        let second_lock = IncanLock::new_with_semantic(second_fingerprint, semantic.clone());
        let first_lock_path = temp.path().join("first/oven.lock");
        let second_lock_path = temp.path().join("second/oven.lock");
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
            Some(temp.path()),
            &ambiguous_semantic,
            std::slice::from_ref(&first),
        );
        let ambiguous_second = compute_resolved_fingerprint_with_sdk_paths(
            std::slice::from_ref(&second),
            &[],
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
            Some(temp.path()),
            &changed_semantic,
            std::slice::from_ref(&second),
        );
        assert_ne!(second_lock.deps_fingerprint, changed_fingerprint);

        let ordinary_first = compute_resolved_fingerprint(&[first], &[], Some(temp.path()), &semantic);
        let ordinary_second = compute_resolved_fingerprint(&[second], &[], Some(temp.path()), &semantic);
        assert_ne!(ordinary_first, ordinary_second);
        Ok(())
    }

    #[test]
    fn provider_semantic_identity_projection_fails_closed_without_checked_oven_source() -> TestResult {
        let provider = ProviderRecord {
            identity: crate::provider::ProviderIdentity {
                name: "support_provider".to_string(),
                version: "0.5.0".to_string(),
                digest: "sha256:physical".to_string(),
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
            manifest: Some(std::sync::Arc::new(crate::library_manifest::LibraryManifest::new(
                "support_provider",
                "0.5.0",
            ))),
            artifact: None,
            implementation_facets: Vec::new(),
        };
        let plan = ProviderPlan::new(
            crate::frontend::library_manifest_index::LibraryManifestIndex::default(),
            vec![provider],
            std::iter::empty::<Vec<String>>(),
        )?;

        let error = provider_semantic_identities(&plan, &[])
            .err()
            .ok_or("provider semantics were derived without checked Oven Rust-source evidence")?;
        assert_eq!(
            error,
            "provider semantic identities require the checked Oven Rust source projection; Cargo-derived provider identity is no longer accepted"
        );
        Ok(())
    }

    /// An absent SDK stays explicitly absent through both lock projection APIs.
    #[test]
    fn semantic_lock_projection_preserves_absent_sdk() -> TestResult {
        let root = tempfile::tempdir()?;
        let plan = ProviderPlan::default();
        let ordinary = semantic_lock_state(root.path(), None, None, None, None, &plan, &[])?;
        let (retained, identities) =
            semantic_lock_state_with_provider_identities(root.path(), None, None, None, None, &plan, &[])?;
        assert_eq!(retained, ordinary);
        assert!(retained.sdk.is_none());
        assert!(retained.providers.is_empty());
        assert!(identities.is_empty());
        Ok(())
    }

    fn sdk_inventory_with_provider(root: &Path, physical_digest: &str) -> SdkInventory {
        SdkInventory {
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
        }
    }

    #[test]
    fn sdk_inventory_digest_uses_provider_semantics_not_physical_artifact_digest_issue921() -> TestResult {
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
        let first = sdk_inventory_with_provider(Path::new("/provider-home-a"), "sha256:physical-a");
        let second = sdk_inventory_with_provider(Path::new("/provider-home-b"), "sha256:physical-b");
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
    fn sdk_inventory_digest_rejects_missing_provider_semantic_identity_issue921() -> TestResult {
        let inventory = sdk_inventory_with_provider(Path::new("/provider-home"), "sha256:physical");
        let error = semantic_sdk_inventory_digest(&inventory, &[])
            .err()
            .ok_or("SDK inventory accepted a provider without checked semantic identity")?;
        assert_eq!(
            error,
            "SDK component `stdlib-data` provider incan_stdlib_data@0.5.0 has no checked semantic identity"
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
        // An out-of-workspace coordinate renders with `..` traversal (#1226) so the lock stays machine-independent.
        let expected_external =
            relative_traversal_path(&fs::canonicalize(&workspace_root)?, &fs::canonicalize(external.path())?)
                .ok_or("external coordinate should render relative to the workspace root")?;
        assert!(
            expected_external.starts_with(".."),
            "an out-of-workspace coordinate must render as traversal, got `{expected_external}`"
        );
        assert_eq!(member.packages[2].project_root, expected_external);
        assert_eq!(member.feature_edges[0].from, "consumer");
        assert_eq!(member.feature_edges[0].to, "");
        assert_eq!(member.feature_edges[1].to, expected_external);
        Ok(())
    }

    #[test]
    fn semantic_lock_state_freezes_declared_oven_interop_requirements() -> TestResult {
        let project = tempfile::tempdir()?;
        fs::create_dir_all(project.path().join("interop/include"))?;
        fs::create_dir_all(project.path().join("interop/src"))?;
        fs::create_dir_all(project.path().join("interop/lib"))?;
        fs::write(project.path().join("interop/include/bridge.h"), "int bridge(void);\n")?;
        fs::write(
            project.path().join("interop/src/bridge.c"),
            "int bridge(void) { return 7; }\n",
        )?;
        fs::write(project.path().join("interop/lib/libfixture.a"), b"fixture archive")?;
        let mut interop = InteropCSection {
            schema: crate::oven_interop::INTEROP_C_SCHEMA_VERSION,
            targets: vec![crate::oven_interop::InteropCTarget {
                target: "aarch64-apple-ios".to_string(),
                toolchain: Some(crate::oven_interop::ToolchainRequirement {
                    capability: "apple-clang".to_string(),
                    version: Some(">=17, <18".to_string()),
                }),
                sdk: Some(crate::oven_interop::ToolchainRequirement {
                    capability: "iphoneos".to_string(),
                    version: Some(">=18, <19".to_string()),
                }),
                platform: Some(crate::oven_interop::InteropTargetPlatform::Ios {
                    deployment_target: "13.0".to_string(),
                }),
                headers: vec!["interop/include/bridge.h".to_string()],
                definitions: vec!["FIXTURE=1".to_string()],
                artifacts: vec![crate::oven_interop::InteropArtifact {
                    name: "fixture".to_string(),
                    kind: crate::oven_interop::InteropArtifactKind::Static,
                    path: Some("interop/lib/libfixture.a".to_string()),
                    origin: None,
                    capability: None,
                    runtime_name: None,
                    placement: None,
                    minimum_platform: None,
                    dependencies: Vec::new(),
                }],
                bindings: Vec::new(),
                shims: vec![crate::oven_interop::InteropShim {
                    name: "fixture_bridge".to_string(),
                    language: crate::oven_interop::InteropShimLanguage::C,
                    sources: vec!["interop/src/bridge.c".to_string()],
                    headers: vec!["interop/include/bridge.h".to_string()],
                    output: "fixture_bridge".to_string(),
                }],
            }],
        };
        let first = semantic_lock_state(
            project.path(),
            Some(&interop),
            None,
            None,
            None,
            &ProviderPlan::default(),
            &[],
        )?;
        let first_interop = first.oven.as_ref().ok_or("lock state omitted Oven requirements")?;
        assert_eq!(first_interop.interop.len(), 1);
        assert_eq!(
            first_interop.interop[0].platform,
            Some(crate::oven_interop::InteropTargetPlatform::Ios {
                deployment_target: "13.0".to_string(),
            })
        );
        assert_eq!(first_interop.interop[0].headers[0].path, "interop/include/bridge.h");
        let first_fingerprint = compute_resolved_fingerprint(&[], &[], Some(project.path()), &first);

        interop.targets[0].platform = Some(crate::oven_interop::InteropTargetPlatform::Ios {
            deployment_target: "14.0".to_string(),
        });
        let changed_platform = semantic_lock_state(
            project.path(),
            Some(&interop),
            None,
            None,
            None,
            &ProviderPlan::default(),
            &[],
        )?;
        assert_ne!(first.oven, changed_platform.oven);
        assert_ne!(
            first_fingerprint,
            compute_resolved_fingerprint(&[], &[], Some(project.path()), &changed_platform)
        );
        interop.targets[0].platform = Some(crate::oven_interop::InteropTargetPlatform::Ios {
            deployment_target: "13.0".to_string(),
        });

        fs::write(
            project.path().join("interop/src/bridge.c"),
            "int bridge(void) { return 8; }\n",
        )?;
        let second = semantic_lock_state(
            project.path(),
            Some(&interop),
            None,
            None,
            None,
            &ProviderPlan::default(),
            &[],
        )?;
        assert_ne!(first.oven, second.oven);
        assert_ne!(
            first_fingerprint,
            compute_resolved_fingerprint(&[], &[], Some(project.path()), &second)
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
        assert_eq!(
            compute_resolved_fingerprint(&[], &[], None, &first),
            compute_resolved_fingerprint(&[], &[], None, &second)
        );
        Ok(())
    }

    #[test]
    fn lockfile_round_trip() -> TestResult {
        let lock = IncanLock::new("sha256:deadbeef".to_string());

        let dir = tempfile::tempdir()?;
        let path = dir.path().join("oven.lock");
        lock.write(&path)?;

        let content = std::fs::read_to_string(&path)?;
        assert!(
            !content.contains("generated ="),
            "lockfiles should not contain volatile generation timestamps"
        );
        let loaded = IncanLock::load(&path)?;
        assert_eq!(loaded.deps_fingerprint, "sha256:deadbeef");
        let encoded: toml::Value = toml::from_str(&content)?;
        assert!(encoded.get("cargo").is_none());
        Ok(())
    }

    #[test]
    fn publication_lock_lives_in_compiler_owned_target_state() -> TestResult {
        let project = tempfile::tempdir()?;
        let lock_path = project.path().join("oven.lock");

        drop(acquire_publication_lock(&lock_path)?);
        drop(acquire_publication_lock(&lock_path)?);

        assert!(
            project
                .path()
                .join("target/incan_lock/.oven.lock.publication.lock")
                .is_file()
        );
        assert!(
            !project.path().join(".oven.lock.incan.lock").exists(),
            "lock publication must not create a persistent project-root sidecar"
        );
        Ok(())
    }

    #[test]
    fn publication_lock_does_not_unlink_a_legacy_lock_inode() -> TestResult {
        let project = tempfile::tempdir()?;
        let lock_path = project.path().join("oven.lock");
        let legacy_lock_path = project.path().join(".oven.lock.incan.lock");
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
        let lock = IncanLock::new_with_semantic("sha256:semantic".to_string(), semantic.clone());

        let dir = tempfile::tempdir()?;
        let path = dir.path().join("oven.lock");
        lock.write(&path)?;

        let loaded = IncanLock::load(&path)?;
        assert_eq!(loaded.format, LOCKFILE_FORMAT_VERSION);
        assert_eq!(loaded.semantic, semantic);
        Ok(())
    }

    #[test]
    fn resolved_fingerprint_changes_with_sdk_feature_or_provider_semantics() {
        let baseline = sample_semantic_state();
        let baseline_fingerprint = compute_resolved_fingerprint(&[], &[], None, &baseline);

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
            compute_resolved_fingerprint(&[], &[], None, &sdk_changed)
        );
        assert_ne!(
            baseline_fingerprint,
            compute_resolved_fingerprint(&[], &[], None, &features_changed)
        );
        assert_ne!(
            baseline_fingerprint,
            compute_resolved_fingerprint(&[], &[], None, &provider_changed)
        );
    }

    #[test]
    fn legacy_cargo_metadata_is_rejected_on_load() -> TestResult {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("oven.lock");
        let legacy_toml = r#"
[incan]
format = 3
incan-version = "0.3.0-dev.23"
generated = "2026-04-27T13:41:45.845714Z"
deps-fingerprint = "sha256:abc"
cargo-features = []
cargo-no-default-features = false
cargo-all-features = false

"#;
        std::fs::write(&path, legacy_toml)?;

        assert!(matches!(IncanLock::load(&path), Err(LockfileError::Parse { .. })));
        Ok(())
    }

    // ---- Phase 4: fingerprint changes when deps change ----

    #[test]
    fn fingerprint_changes_when_deps_differ() {
        let deps_a = vec![sample_spec("alpha", vec!["a"])];
        let deps_b = vec![sample_spec("alpha", vec!["a", "b"])];
        let fp_a = compute_deps_fingerprint(&deps_a, &[], None);
        let fp_b = compute_deps_fingerprint(&deps_b, &[], None);
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
        assert_eq!(
            compute_deps_fingerprint(&[dep_plain], &[], Some(Path::new("."))),
            compute_deps_fingerprint(&[dep_current_dir], &[], Some(Path::new("."))),
        );
    }

    // ---- Phase 4: stale fingerprint detection ----

    #[test]
    fn stale_fingerprint_is_detectable() {
        let deps_v1 = vec![sample_spec("alpha", vec!["a"])];
        let fp_v1 = compute_deps_fingerprint(&deps_v1, &[], None);

        let lock = IncanLock::new(fp_v1.clone());

        // Simulate deps changing
        let deps_v2 = vec![sample_spec("alpha", vec!["a", "new_feature"])];
        let fp_v2 = compute_deps_fingerprint(&deps_v2, &[], None);

        assert_ne!(
            lock.deps_fingerprint, fp_v2,
            "lock fingerprint should not match updated deps"
        );
    }

    #[test]
    fn semantic_lock_refuses_legacy_cargo_authority() -> TestResult {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("oven.lock");
        let lock = IncanLock::new("sha256:semantic".to_string());
        lock.write(&path)?;
        let current = std::fs::read_to_string(&path)?;
        for format in [1, LOCKFILE_FORMAT_VERSION] {
            let mut encoded: toml::Value = toml::from_str(&current)?;
            encoded
                .get_mut("incan")
                .and_then(toml::Value::as_table_mut)
                .ok_or("serialized lock has no incan table")?
                .insert("format".to_string(), toml::Value::Integer(i64::from(format)));
            encoded.as_table_mut().ok_or("serialized lock is not a table")?.insert(
                "cargo".to_string(),
                toml::Value::Table(toml::Table::from_iter([(
                    "lock".to_string(),
                    toml::Value::String("version = 4\n".to_string()),
                )])),
            );
            std::fs::write(&path, toml::to_string(&encoded)?)?;
            assert!(matches!(IncanLock::load(&path), Err(LockfileError::Parse { .. })));
        }
        Ok(())
    }

    // ---- Phase 4: format version is validated on load ----

    #[test]
    fn lockfile_format_version_checked() -> TestResult {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("oven.lock");

        // Write a lockfile with an incompatible format version
        let bad_toml = r#"
[incan]
format = 999
incan-version = "0.1.0"
generated = "2025-01-01T00:00:00Z"
deps-fingerprint = "sha256:abc"

"#;
        std::fs::write(&path, bad_toml)?;
        let result = IncanLock::load(&path);
        assert!(
            matches!(result, Err(LockfileError::Invalid { message, .. }) if message.contains("unsupported lockfile format 999"))
        );
        Ok(())
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
            oven: None,
            workspace_members: Vec::new(),
        }
    }
}
