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

use crate::manifest::{DependencySource, DependencySpec, GitReference};
use crate::provider::{
    BackendImplementationRequirement, ComponentSelectionReason, PackageFeaturePlan, ProviderParticipation,
    ProviderPlan, ResolvedSdkComponents, SdkInventory,
};

const LOCKFILE_FORMAT_VERSION: u32 = 2;
const LEGACY_LOCKFILE_FORMAT_VERSION: u32 = 1;

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

    /// Render and crash-safely publish this lockfile at `path` using the RFC 112 sibling-lock protocol.
    ///
    /// The method stages complete contents beside the destination, synchronizes them, atomically replaces the target,
    /// then requests parent-directory synchronization. Concurrent cooperative publishers serialize on a stable sibling
    /// lock entry, so callers either observe the prior complete lockfile or the new complete lockfile.
    pub fn write(&self, path: &Path) -> Result<(), LockfileError> {
        let publication_lock = acquire_publication_lock(path).map_err(|e| LockfileError::Write {
            path: path.to_path_buf(),
            source: e,
        })?;
        self.write_while_locked(path, &publication_lock)
    }

    /// Publish this lockfile while the caller retains the matching RFC 112 sibling lock.
    ///
    /// Workspace lock generation holds this guard across resolution and Cargo lockfile generation as well as the final
    /// publish, preventing concurrent commands from sharing the generated-project staging directory.
    pub(crate) fn write_while_locked(&self, path: &Path, publication_lock: &File) -> Result<(), LockfileError> {
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
) -> Result<SemanticLockState, String> {
    let sdk = match (sdk_inventory, sdk_components) {
        (Some(inventory), Some(components)) => {
            let payload = inventory.to_json().map_err(|error| error.to_string())?;
            let inventory_digest = digest_bytes(payload.as_bytes());
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
    let providers = provider_plan
        .records()
        .filter(|provider| provider.enabled)
        .map(|provider| LockedProvider {
            identity: provider.identity.stable_key(),
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

/// Publish a complete lockfile while holding the stable sibling lock identity specified by RFC 112.
///
/// This is compiler-host infrastructure: `incan lock` runs before any user program exists, so it cannot invoke the
/// generated Incan `std.fs` library directly. The operation intentionally mirrors its public recipe—exclusive sibling
/// lock, same-directory exclusive staging, content synchronization, atomic replacement, then parent synchronization.
fn publish_lockfile(path: &Path, content: &[u8], _publication_lock: &File) -> io::Result<()> {
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

/// Retain the sibling lock descriptor for the entire publication critical section.
pub(crate) fn acquire_publication_lock(path: &Path) -> io::Result<File> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let file_name = path.file_name().ok_or_else(|| {
        io::Error::new(
            ErrorKind::InvalidInput,
            "lockfile publication requires a target path with a final component",
        )
    })?;
    let lock_path = parent.join(format!(".{}.incan.lock", file_name.to_string_lossy()));
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(lock_path)?;
    file.lock()?;
    Ok(file)
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
    let mut specs = Vec::new();

    for spec in dependencies {
        specs.push(SpecFingerprint::from_spec(spec, "normal", project_root));
    }
    for spec in dev_dependencies {
        specs.push(SpecFingerprint::from_spec(spec, "dev", project_root));
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
    fn from_spec(spec: &DependencySpec, kind: &str, project_root: Option<&Path>) -> Self {
        let mut features = spec.features.clone();
        features.sort();
        features.dedup();

        Self {
            crate_name: spec.crate_name.clone(),
            kind: kind.to_string(),
            source: source_fingerprint(&spec.source, project_root),
            version_req: spec.version.as_deref().map(normalize_version_req),
            default_features: spec.default_features,
            features,
            optional: spec.optional,
            package: spec.package.clone(),
        }
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
