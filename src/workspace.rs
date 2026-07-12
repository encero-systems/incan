//! RFC 077 workspace discovery and topology validation.
//!
//! This module deliberately stops at the validated graph boundary. Locking, dependency inheritance, and command
//! fan-out consume this graph in later RFC 077 stages; they must not rediscover membership independently.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use globset::Glob;
use serde::Serialize;

use crate::lockfile::IncanLock;
use crate::manifest::{
    DependencySpec, LibraryDependencySpec, MANIFEST_FILENAME, ManifestError, ProjectManifest, WorkspaceSection,
    parse_workspace_library_dependency_declarations, parse_workspace_rust_dependency_declarations,
    parse_workspace_rust_dev_dependency_declarations,
};

/// Stable schema version for the machine-readable workspace topology view.
pub const WORKSPACE_INSPECT_SCHEMA_VERSION: u32 = 1;

/// A validated workspace member in deterministic workspace order.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct WorkspaceMember {
    /// Project identity from `[project].name`.
    pub name: String,
    /// Canonical absolute member directory.
    pub path: PathBuf,
    /// Canonical absolute member manifest path.
    pub manifest_path: PathBuf,
    /// Whether this is the implicit root project member.
    pub root_member: bool,
}

/// Workspace-owned declarations that later inheritance, policy, and lock stages consume from the validated graph.
///
/// Maps are ordered here—not merely in their TOML representation—so JSON inspection and subsequent fingerprints are
/// deterministic regardless of TOML deserialization order.
#[derive(Debug, Clone, Default, Serialize)]
pub struct WorkspaceSharedConfiguration {
    pub dependencies: BTreeMap<String, toml::Value>,
    pub rust_dependencies: BTreeMap<String, toml::Value>,
    pub rust_dev_dependencies: BTreeMap<String, toml::Value>,
    pub envs: BTreeMap<String, toml::Value>,
    pub policy: Option<toml::Value>,
    pub sources: Option<toml::Value>,
    pub capabilities: Option<toml::Value>,
}

/// The root-owned lockfile location and its parse state.
///
/// Workspace topology remains inspectable when a lockfile is missing or malformed. Commands that require a current
/// lock make their own strict decision later; inspection exposes enough evidence for users and tooling to explain
/// that decision before attempting a build or mutation.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct WorkspaceLockState {
    /// Absolute canonical workspace-root lockfile location. The path is retained even when no file exists yet.
    pub path: PathBuf,
    /// Whether the root lockfile is absent, valid, or present but unparsable.
    pub status: WorkspaceLockStatus,
    /// Parsed lockfile format when the lockfile is valid.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<u32>,
    /// Compiler version recorded by a valid lockfile.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub incan_version: Option<String>,
    /// Dependency-resolution fingerprint recorded by a valid lockfile.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deps_fingerprint: Option<String>,
    /// A non-fatal parse/read error for an invalid lockfile.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Non-authoritative member-local lockfiles that should be migrated through an explicit reviewed change.
    ///
    /// Workspace inspection reports these paths but deliberately does not remove them. The workspace root lock is
    /// the only authoritative lock once `[workspace]` is declared.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub stale_member_lockfiles: Vec<PathBuf>,
}

/// Stable machine-readable classification for a root lockfile.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceLockStatus {
    /// No root `incan.lock` has been published.
    Missing,
    /// The root `incan.lock` parsed successfully.
    Valid,
    /// A root `incan.lock` exists but cannot be read or parsed.
    Invalid,
}

/// Validated RFC 077 topology, shared by all future workspace-aware command paths.
#[derive(Debug, Clone)]
pub struct WorkspaceGraph {
    root: PathBuf,
    manifest_path: PathBuf,
    members: Vec<WorkspaceMember>,
    default_members: Vec<String>,
    exclusions: Vec<String>,
    warnings: Vec<String>,
    shared: WorkspaceSharedConfiguration,
    lock: WorkspaceLockState,
    effective_library_dependencies: BTreeMap<PathBuf, BTreeMap<String, LibraryDependencySpec>>,
    effective_rust_dependencies: BTreeMap<PathBuf, BTreeMap<String, DependencySpec>>,
    effective_rust_dev_dependencies: BTreeMap<PathBuf, BTreeMap<String, DependencySpec>>,
}

/// Errors while discovering or validating one workspace graph.
#[derive(Debug, thiserror::Error)]
pub enum WorkspaceError {
    #[error(transparent)]
    Manifest(#[from] ManifestError),
    #[error("invalid workspace {manifest}: {message}")]
    Invalid { manifest: PathBuf, message: String },
    #[error("failed to inspect workspace path {path}: {source}")]
    Io { path: PathBuf, source: std::io::Error },
}

impl WorkspaceGraph {
    /// Discover the nearest workspace whose validated member graph contains `start`.
    ///
    /// A virtual workspace root is a valid context at its root even though it has no root member. Ancestor manifests
    /// that do not contain the start path have no authority over it, matching RFC 077's project-discovery rule.
    pub fn discover(start: &Path) -> Result<Option<Self>, WorkspaceError> {
        let start = canonical_start_path(start)?;
        let mut candidate = if start.is_dir() {
            start.clone()
        } else {
            start.parent().map(Path::to_path_buf).unwrap_or_else(|| start.clone())
        };

        loop {
            let manifest_path = candidate.join(MANIFEST_FILENAME);
            if manifest_path.is_file() {
                let manifest = read_manifest(&manifest_path)?;
                if manifest.workspace().is_some() {
                    let graph = Self::from_root_manifest(manifest)?;
                    if graph.contains_path(&start) || graph.root == start {
                        return Ok(Some(graph));
                    }
                }
            }
            if !candidate.pop() {
                return Ok(None);
            }
        }
    }

    /// Build and validate a graph from a manifest known to contain `[workspace]`.
    pub fn from_root_manifest(root_manifest: ProjectManifest) -> Result<Self, WorkspaceError> {
        let manifest_path = canonical_path(root_manifest.path())?;
        let root = manifest_path
            .parent()
            .map(Path::to_path_buf)
            .ok_or_else(|| WorkspaceError::Invalid {
                manifest: manifest_path.clone(),
                message: "workspace manifest has no parent directory".to_string(),
            })?;
        let workspace = root_manifest.workspace().ok_or_else(|| WorkspaceError::Invalid {
            manifest: manifest_path.clone(),
            message: "manifest does not declare [workspace]".to_string(),
        })?;

        let mut members = Vec::new();
        let mut member_manifests = BTreeMap::new();
        if let Some(project) = root_manifest.project.as_ref() {
            let name = required_project_name(project.name.as_deref(), &manifest_path)?;
            members.push(WorkspaceMember {
                name,
                path: root.clone(),
                manifest_path: manifest_path.clone(),
                root_member: true,
            });
            member_manifests.insert(root.clone(), root_manifest.clone());
        }

        let mut warnings = Vec::new();
        let excluded = expand_patterns(&root, &workspace.exclude, "exclude", &manifest_path, &mut warnings)?;
        let expanded = expand_patterns(&root, &workspace.members, "members", &manifest_path, &mut warnings)?;
        let excluded: BTreeSet<PathBuf> = excluded.into_iter().collect();
        let mut non_root = BTreeMap::new();

        for path in expanded {
            if excluded.contains(&path) {
                continue;
            }
            if path == root {
                return Err(invalid(
                    &manifest_path,
                    "[workspace].members must not include the workspace root; [project] makes it the implicit root member",
                ));
            }
            let member_manifest_path = path.join(MANIFEST_FILENAME);
            if !member_manifest_path.is_file() {
                return Err(invalid(
                    &manifest_path,
                    format!(
                        "workspace member {} does not contain {MANIFEST_FILENAME}",
                        path.display()
                    ),
                ));
            }
            let member_manifest = read_manifest(&member_manifest_path)?;
            if member_manifest.workspace().is_some() {
                return Err(invalid(
                    &manifest_path,
                    format!("workspace member {} declares a nested [workspace]", path.display()),
                ));
            }
            let name = required_project_name(
                member_manifest
                    .project
                    .as_ref()
                    .and_then(|project| project.name.as_deref()),
                &member_manifest_path,
            )?;
            non_root.insert(
                path.clone(),
                WorkspaceMember {
                    name,
                    path: path.clone(),
                    manifest_path: canonical_path(&member_manifest_path)?,
                    root_member: false,
                },
            );
            member_manifests.insert(path, member_manifest);
        }
        members.extend(non_root.into_values());

        if members.is_empty() {
            return Err(invalid(
                &manifest_path,
                "a virtual workspace must expand at least one member",
            ));
        }
        validate_unique_member_names(&members, &manifest_path)?;
        let default_members = resolve_default_members(&members, &root, workspace, &manifest_path)?;
        let shared = WorkspaceSharedConfiguration {
            dependencies: workspace
                .dependencies
                .iter()
                .map(|(name, value)| (name.clone(), value.clone()))
                .collect(),
            rust_dependencies: workspace
                .rust_dependencies
                .iter()
                .map(|(name, value)| (name.clone(), value.clone()))
                .collect(),
            rust_dev_dependencies: workspace
                .rust_dev_dependencies
                .iter()
                .map(|(name, value)| (name.clone(), value.clone()))
                .collect(),
            envs: workspace
                .envs
                .iter()
                .map(|(name, value)| (name.clone(), value.clone()))
                .collect(),
            policy: workspace.policy.clone(),
            sources: workspace.sources.clone(),
            capabilities: workspace.capabilities.clone(),
        };
        let shared_library_dependencies = parse_workspace_library_dependency_declarations(workspace, &manifest_path)?;
        let shared_rust_dependencies = parse_workspace_rust_dependency_declarations(workspace, &manifest_path)?;
        let shared_rust_dev_dependencies = parse_workspace_rust_dev_dependency_declarations(workspace, &manifest_path)?;
        let effective_library_dependencies = resolve_effective_library_dependencies(
            &members,
            &member_manifests,
            &shared_library_dependencies,
            &manifest_path,
        )?;
        let effective_rust_dependencies = resolve_effective_rust_dependencies(
            &members,
            &member_manifests,
            &shared_rust_dependencies,
            &manifest_path,
        )?;
        let effective_rust_dev_dependencies = resolve_effective_rust_dev_dependencies(
            &members,
            &member_manifests,
            &shared_rust_dev_dependencies,
            &manifest_path,
        )?;
        let lock = inspect_root_lock(&root, &members);
        warnings.extend(lock.stale_member_lockfiles.iter().map(|path| {
            format!(
                "member-local lockfile {} is non-authoritative; use the workspace root incan.lock",
                path.display()
            )
        }));

        Ok(Self {
            root,
            manifest_path,
            members,
            default_members,
            exclusions: workspace.exclude.clone(),
            warnings,
            shared,
            lock,
            effective_library_dependencies,
            effective_rust_dependencies,
            effective_rust_dev_dependencies,
        })
    }

    /// Canonical workspace root directory.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Canonical workspace root manifest path.
    pub fn manifest_path(&self) -> &Path {
        &self.manifest_path
    }

    /// Members in RFC 077 deterministic workspace order.
    pub fn members(&self) -> &[WorkspaceMember] {
        &self.members
    }

    /// Resolved member names selected from the workspace root by default.
    pub fn default_members(&self) -> &[String] {
        &self.default_members
    }

    /// Original exclusion patterns for machine-readable inspection.
    pub fn exclusions(&self) -> &[String] {
        &self.exclusions
    }

    /// Non-fatal topology warnings, including unmatched member/exclusion globs.
    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    /// Deterministic workspace-owned declarations retained from the root manifest.
    pub fn shared(&self) -> &WorkspaceSharedConfiguration {
        &self.shared
    }

    /// Root-owned lockfile state for machine-readable inspection and workspace-aware command planning.
    pub fn lock(&self) -> &WorkspaceLockState {
        &self.lock
    }

    /// Effective Incan library dependency graph for one member after explicit workspace inheritance.
    pub fn effective_library_dependencies_for(
        &self,
        member: &WorkspaceMember,
    ) -> Option<&BTreeMap<String, LibraryDependencySpec>> {
        self.effective_library_dependencies.get(&member.path)
    }

    /// Effective Rust dependency graph for one member after workspace inheritance and additive refinements.
    pub fn effective_rust_dependencies_for(
        &self,
        member: &WorkspaceMember,
    ) -> Option<&BTreeMap<String, DependencySpec>> {
        self.effective_rust_dependencies.get(&member.path)
    }

    /// Effective Rust dev-dependency graph for one member after workspace inheritance and additive refinements.
    pub fn effective_rust_dev_dependencies_for(
        &self,
        member: &WorkspaceMember,
    ) -> Option<&BTreeMap<String, DependencySpec>> {
        self.effective_rust_dev_dependencies.get(&member.path)
    }

    /// Resolve the member scope implied by `start` before any command execution.
    pub fn selected_members_for(&self, start: &Path) -> Result<Vec<&WorkspaceMember>, WorkspaceError> {
        let start = canonical_start_path(start)?;
        if start == self.root {
            if !self.default_members.is_empty() {
                return Ok(self
                    .members
                    .iter()
                    .filter(|member| self.default_members.contains(&member.name))
                    .collect());
            }
            if let Some(root_member) = self.members.iter().find(|member| member.root_member) {
                return Ok(vec![root_member]);
            }
            return Ok(self.members.iter().collect());
        }

        let mut containing = self
            .members
            .iter()
            .filter(|member| start.starts_with(&member.path))
            .collect::<Vec<_>>();
        containing.sort_by_key(|member| std::cmp::Reverse(member.path.components().count()));
        containing.into_iter().next().map_or_else(
            || {
                Err(invalid(
                    &self.manifest_path,
                    format!("{} is not contained by this workspace", start.display()),
                ))
            },
            |member| Ok(vec![member]),
        )
    }

    /// Resolve an explicit RFC 077 workspace selector, or infer the current member scope when no selector is given.
    ///
    /// Callers must invoke this before they compile, execute, lock, or mutate any member state. The graph owns name
    /// and root-relative path interpretation so individual commands cannot disagree about a selected package.
    pub fn select_members<'a>(
        &'a self,
        start: &Path,
        workspace: bool,
        selectors: &[String],
    ) -> Result<Vec<&'a WorkspaceMember>, WorkspaceError> {
        if workspace {
            return Ok(self.members.iter().collect());
        }
        if selectors.is_empty() {
            return self.selected_members_for(start);
        }

        let mut selected = BTreeSet::new();
        for selector in selectors {
            let selector = selector.trim();
            if selector.is_empty() {
                return Err(invalid(
                    &self.manifest_path,
                    "workspace member selector cannot be empty",
                ));
            }
            let by_name = self.members.iter().find(|member| member.name == selector);
            let by_path = canonical_path(&self.root.join(selector))
                .ok()
                .and_then(|path| self.members.iter().find(|member| member.path == path));
            let Some(member) = by_name.or(by_path) else {
                return Err(invalid(
                    &self.manifest_path,
                    format!("workspace member selector `{selector}` does not resolve to a member"),
                ));
            };
            selected.insert(member.path.clone());
        }

        Ok(self
            .members
            .iter()
            .filter(|member| selected.contains(&member.path))
            .collect())
    }

    fn contains_path(&self, path: &Path) -> bool {
        path == self.root || self.members.iter().any(|member| path.starts_with(&member.path))
    }
}

fn canonical_start_path(path: &Path) -> Result<PathBuf, WorkspaceError> {
    canonical_path(path)
}

fn canonical_path(path: &Path) -> Result<PathBuf, WorkspaceError> {
    fs::canonicalize(path).map_err(|source| WorkspaceError::Io {
        path: path.to_path_buf(),
        source,
    })
}

fn read_manifest(path: &Path) -> Result<ProjectManifest, WorkspaceError> {
    let content = fs::read_to_string(path).map_err(|source| WorkspaceError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    ProjectManifest::from_str(&content, path).map_err(Into::into)
}

fn inspect_root_lock(root: &Path, members: &[WorkspaceMember]) -> WorkspaceLockState {
    let path = root.join("incan.lock");
    let stale_member_lockfiles = members
        .iter()
        .filter(|member| member.path != root)
        .map(|member| member.path.join("incan.lock"))
        .filter(|path| path.is_file())
        .collect::<Vec<_>>();
    if !path.exists() {
        return WorkspaceLockState {
            path,
            status: WorkspaceLockStatus::Missing,
            format: None,
            incan_version: None,
            deps_fingerprint: None,
            error: None,
            stale_member_lockfiles,
        };
    }
    match IncanLock::load(&path) {
        Ok(lock) => WorkspaceLockState {
            path,
            status: WorkspaceLockStatus::Valid,
            format: Some(lock.format),
            incan_version: Some(lock.incan_version),
            deps_fingerprint: Some(lock.deps_fingerprint),
            error: None,
            stale_member_lockfiles,
        },
        Err(error) => WorkspaceLockState {
            path,
            status: WorkspaceLockStatus::Invalid,
            format: None,
            incan_version: None,
            deps_fingerprint: None,
            error: Some(error.to_string()),
            stale_member_lockfiles,
        },
    }
}

fn required_project_name(name: Option<&str>, manifest: &Path) -> Result<String, WorkspaceError> {
    let Some(name) = name.map(str::trim).filter(|name| !name.is_empty()) else {
        return Err(invalid(
            manifest,
            "workspace members require a non-empty [project].name",
        ));
    };
    Ok(name.to_string())
}

fn invalid(manifest: &Path, message: impl Into<String>) -> WorkspaceError {
    WorkspaceError::Invalid {
        manifest: manifest.to_path_buf(),
        message: message.into(),
    }
}

fn expand_patterns(
    root: &Path,
    patterns: &[String],
    field: &str,
    manifest: &Path,
    warnings: &mut Vec<String>,
) -> Result<Vec<PathBuf>, WorkspaceError> {
    let all_directories = discover_directories(root)?;
    let mut expanded = BTreeSet::new();
    for pattern in patterns {
        let pattern = pattern.trim();
        if pattern.is_empty() {
            return Err(invalid(
                manifest,
                format!("[workspace].{field} cannot contain an empty path"),
            ));
        }
        if pattern_is_root(pattern)? {
            return Err(invalid(
                manifest,
                format!(
                    "[workspace].{field} must not include the workspace root; use the implicit root member instead"
                ),
            ));
        }
        if contains_glob(pattern) {
            let glob = Glob::new(pattern)
                .map_err(|error| {
                    invalid(
                        manifest,
                        format!("invalid [workspace].{field} glob `{pattern}`: {error}"),
                    )
                })?
                .compile_matcher();
            let matches = all_directories
                .iter()
                .filter(|directory| {
                    directory
                        .strip_prefix(root)
                        .ok()
                        .is_some_and(|relative| glob.is_match(relative))
                })
                .cloned()
                .collect::<Vec<_>>();
            if matches.is_empty() {
                warnings.push(format!("[workspace].{field} glob `{pattern}` matched no directories"));
            }
            expanded.extend(matches);
        } else {
            let path = canonical_path(&root.join(pattern))?;
            if !path.starts_with(root) {
                return Err(invalid(
                    manifest,
                    format!("[workspace].{field} path `{pattern}` resolves outside the workspace root"),
                ));
            }
            if !path.is_dir() {
                return Err(invalid(
                    manifest,
                    format!("[workspace].{field} path `{pattern}` is not a directory"),
                ));
            }
            expanded.insert(path);
        }
    }
    Ok(expanded.into_iter().collect())
}

fn pattern_is_root(pattern: &str) -> Result<bool, WorkspaceError> {
    if contains_glob(pattern) {
        return Glob::new(pattern)
            .map(|glob| glob.compile_matcher().is_match(""))
            .map_err(|error| WorkspaceError::Invalid {
                manifest: PathBuf::from(MANIFEST_FILENAME),
                message: format!("invalid workspace glob `{pattern}`: {error}"),
            });
    }
    Ok(Path::new(pattern)
        .components()
        .all(|component| matches!(component, std::path::Component::CurDir)))
}

fn contains_glob(pattern: &str) -> bool {
    pattern
        .bytes()
        .any(|byte| matches!(byte, b'*' | b'?' | b'[' | b'{' | b'!'))
}

fn discover_directories(root: &Path) -> Result<Vec<PathBuf>, WorkspaceError> {
    let mut directories = vec![root.to_path_buf()];
    let mut index = 0;
    while let Some(directory) = directories.get(index).cloned() {
        index += 1;
        for entry in fs::read_dir(&directory).map_err(|source| WorkspaceError::Io {
            path: directory.clone(),
            source,
        })? {
            let entry = entry.map_err(|source| WorkspaceError::Io {
                path: directory.clone(),
                source,
            })?;
            let path = entry.path();
            if entry
                .file_type()
                .map_err(|source| WorkspaceError::Io {
                    path: path.clone(),
                    source,
                })?
                .is_dir()
            {
                let canonical = canonical_path(&path)?;
                if canonical.starts_with(root) && !directories.contains(&canonical) {
                    directories.push(canonical);
                }
            }
        }
    }
    directories.sort();
    Ok(directories)
}

fn validate_unique_member_names(members: &[WorkspaceMember], manifest: &Path) -> Result<(), WorkspaceError> {
    let mut paths_by_name = BTreeMap::new();
    for member in members {
        if let Some(previous) = paths_by_name.insert(member.name.as_str(), &member.path) {
            return Err(invalid(
                manifest,
                format!(
                    "workspace member name `{}` is duplicated by {} and {}",
                    member.name,
                    previous.display(),
                    member.path.display()
                ),
            ));
        }
    }
    Ok(())
}

fn resolve_default_members(
    members: &[WorkspaceMember],
    root: &Path,
    workspace: &WorkspaceSection,
    manifest: &Path,
) -> Result<Vec<String>, WorkspaceError> {
    let mut resolved = Vec::new();
    for selector in &workspace.default_members {
        let selector = selector.trim();
        let matched = members.iter().find(|member| member.name == selector).or_else(|| {
            let path = canonical_path(&root.join(selector)).ok()?;
            members.iter().find(|member| member.path == path)
        });
        let Some(member) = matched else {
            return Err(invalid(
                manifest,
                format!("[workspace].default-members entry `{selector}` does not resolve to a member"),
            ));
        };
        if !resolved.contains(&member.name) {
            resolved.push(member.name.clone());
        }
    }
    Ok(resolved)
}

/// Build the effective Incan library dependency map for every member from local declarations and explicit workspace
/// opt-ins. RFC 077 does not permit member-level refinements for these dependencies.
fn resolve_effective_library_dependencies(
    members: &[WorkspaceMember],
    manifests: &BTreeMap<PathBuf, ProjectManifest>,
    shared: &BTreeMap<String, LibraryDependencySpec>,
    workspace_manifest: &Path,
) -> Result<BTreeMap<PathBuf, BTreeMap<String, LibraryDependencySpec>>, WorkspaceError> {
    let mut effective_by_member = BTreeMap::new();
    for member in members {
        let manifest = manifests.get(&member.path).ok_or_else(|| {
            invalid(
                workspace_manifest,
                format!("missing parsed manifest for workspace member {}", member.path.display()),
            )
        })?;
        let mut effective = manifest
            .library_dependencies()
            .iter()
            .map(|(name, spec)| (name.clone(), spec.clone()))
            .collect::<BTreeMap<_, _>>();
        for name in manifest.workspace_library_dependencies().keys() {
            let baseline = shared.get(name).ok_or_else(|| {
                invalid(
                    workspace_manifest,
                    format!(
                        "workspace member `{}` inherits library dependency `{name}`, but [workspace.dependencies] does not declare it",
                        member.name
                    ),
                )
            })?;
            effective.insert(name.clone(), baseline.clone());
        }
        effective_by_member.insert(member.path.clone(), effective);
    }
    Ok(effective_by_member)
}

/// Build the effective Rust dependency map for every member from local declarations and explicit workspace opt-ins.
fn resolve_effective_rust_dependencies(
    members: &[WorkspaceMember],
    manifests: &BTreeMap<PathBuf, ProjectManifest>,
    shared: &BTreeMap<String, DependencySpec>,
    workspace_manifest: &Path,
) -> Result<BTreeMap<PathBuf, BTreeMap<String, DependencySpec>>, WorkspaceError> {
    let mut effective_by_member = BTreeMap::new();
    for member in members {
        let manifest = manifests.get(&member.path).ok_or_else(|| {
            invalid(
                workspace_manifest,
                format!("missing parsed manifest for workspace member {}", member.path.display()),
            )
        })?;
        let mut effective = manifest
            .rust_dependencies()
            .iter()
            .map(|(name, spec)| (name.clone(), spec.clone().normalized()))
            .collect::<BTreeMap<_, _>>();
        for (name, refinement) in manifest.workspace_rust_dependencies() {
            let baseline = shared.get(name).ok_or_else(|| {
                invalid(
                    workspace_manifest,
                    format!(
                        "workspace member `{}` inherits Rust dependency `{name}`, but [workspace.rust-dependencies] does not declare it",
                        member.name
                    ),
                )
            })?;
            let mut spec = baseline.clone();
            spec.features.extend(refinement.features.iter().cloned());
            spec.features.sort();
            spec.features.dedup();
            spec.optional = refinement.optional;
            effective.insert(name.clone(), spec);
        }
        effective_by_member.insert(member.path.clone(), effective);
    }
    Ok(effective_by_member)
}

fn resolve_effective_rust_dev_dependencies(
    members: &[WorkspaceMember],
    manifests: &BTreeMap<PathBuf, ProjectManifest>,
    shared: &BTreeMap<String, DependencySpec>,
    workspace_manifest: &Path,
) -> Result<BTreeMap<PathBuf, BTreeMap<String, DependencySpec>>, WorkspaceError> {
    let mut effective_by_member = BTreeMap::new();
    for member in members {
        let manifest = manifests.get(&member.path).ok_or_else(|| {
            invalid(
                workspace_manifest,
                format!("missing parsed manifest for workspace member {}", member.path.display()),
            )
        })?;
        let mut effective = manifest
            .rust_dev_dependencies()
            .iter()
            .map(|(name, spec)| (name.clone(), spec.clone().normalized()))
            .collect::<BTreeMap<_, _>>();
        for (name, refinement) in manifest.workspace_rust_dev_dependencies() {
            let baseline = shared.get(name).ok_or_else(|| {
                invalid(
                    workspace_manifest,
                    format!(
                        "workspace member `{}` inherits Rust dev dependency `{name}`, but [workspace.rust-dev-dependencies] does not declare it",
                        member.name
                    ),
                )
            })?;
            let mut spec = baseline.clone();
            spec.features.extend(refinement.features.iter().cloned());
            spec.features.sort();
            spec.features.dedup();
            spec.optional = refinement.optional;
            effective.insert(name.clone(), spec);
        }
        effective_by_member.insert(member.path.clone(), effective);
    }
    Ok(effective_by_member)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    fn write_manifest(root: &Path, relative: &str, content: &str) -> Result<(), Box<dyn std::error::Error>> {
        let path = root.join(relative);
        let parent = path.parent().ok_or("manifest path needs a parent")?;
        fs::create_dir_all(parent)?;
        fs::write(path, content)?;
        Ok(())
    }

    #[test]
    fn rooted_workspace_has_implicit_root_and_deterministic_glob_members() -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        write_manifest(
            temp.path(),
            "incan.toml",
            "[project]\nname = 'root'\n[workspace]\nmembers = ['packages/*']\nexclude = ['packages/experimental']\ndefault-members = ['storage']\n[workspace.dependencies]\nshared = { path = 'packages/shared' }\n[workspace.rust-dependencies]\nserde = { version = '1', default-features = false }\n[workspace.rust-dev-dependencies]\npretty_assertions = '1'\n[workspace.policy]\nrequire-approval = true\n[dependencies]\nshared = { workspace = true }\n[rust-dependencies]\nserde = { workspace = true, features = ['derive'] }\n[rust-dev-dependencies]\npretty_assertions = { workspace = true }\n",
        )?;
        write_manifest(
            temp.path(),
            "packages/storage/incan.toml",
            "[project]\nname = 'storage'\n[dependencies]\nshared = { workspace = true }\n[rust-dependencies]\nserde = { workspace = true, features = ['std'], optional = true }\n[rust-dev-dependencies]\npretty_assertions = { workspace = true, features = ['diff'] }\n",
        )?;
        write_manifest(
            temp.path(),
            "packages/experimental/incan.toml",
            "[project]\nname = 'experimental'\n",
        )?;

        let graph = WorkspaceGraph::discover(temp.path())?.ok_or("workspace should be discovered")?;
        assert_eq!(
            graph
                .members
                .iter()
                .map(|member| member.name.as_str())
                .collect::<Vec<_>>(),
            ["root", "storage"]
        );
        assert_eq!(graph.default_members(), ["storage"]);
        assert_eq!(graph.shared().rust_dependencies.keys().collect::<Vec<_>>(), ["serde"]);
        assert!(graph.shared().policy.is_some());
        let root_member = graph.members().first().ok_or("missing root member")?;
        let root_shared = graph
            .effective_library_dependencies_for(root_member)
            .and_then(|dependencies| dependencies.get("shared"))
            .ok_or("missing effective root shared library dependency")?;
        assert_eq!(root_shared.path, graph.root().join("packages/shared"));
        let root_serde = graph
            .effective_rust_dependencies_for(root_member)
            .and_then(|dependencies| dependencies.get("serde"))
            .ok_or("missing effective root serde dependency")?;
        assert_eq!(root_serde.features, ["derive"]);
        assert!(!root_serde.default_features);
        assert!(!root_serde.optional);
        let root_pretty_assertions = graph
            .effective_rust_dev_dependencies_for(root_member)
            .and_then(|dependencies| dependencies.get("pretty_assertions"))
            .ok_or("missing effective root pretty_assertions dev dependency")?;
        assert_eq!(root_pretty_assertions.features, Vec::<String>::new());
        let storage = graph
            .members()
            .iter()
            .find(|member| member.name == "storage")
            .ok_or("missing storage")?;
        let storage_shared = graph
            .effective_library_dependencies_for(storage)
            .and_then(|dependencies| dependencies.get("shared"))
            .ok_or("missing effective storage shared library dependency")?;
        assert_eq!(storage_shared.path, graph.root().join("packages/shared"));
        let storage_serde = graph
            .effective_rust_dependencies_for(storage)
            .and_then(|dependencies| dependencies.get("serde"))
            .ok_or("missing effective storage serde dependency")?;
        assert_eq!(storage_serde.features, ["std"]);
        assert!(storage_serde.optional);
        let storage_pretty_assertions = graph
            .effective_rust_dev_dependencies_for(storage)
            .and_then(|dependencies| dependencies.get("pretty_assertions"))
            .ok_or("missing effective storage pretty_assertions dev dependency")?;
        assert_eq!(storage_pretty_assertions.features, ["diff"]);
        assert_eq!(graph.selected_members_for(temp.path())?[0].name, "storage");
        assert_eq!(
            graph.selected_members_for(&temp.path().join("packages/storage"))?[0].name,
            "storage"
        );
        Ok(())
    }

    #[test]
    fn virtual_workspace_defaults_to_all_members_at_root() -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        write_manifest(temp.path(), "incan.toml", "[workspace]\nmembers = ['members/*']\n")?;
        write_manifest(temp.path(), "members/first/incan.toml", "[project]\nname = 'first'\n")?;
        write_manifest(temp.path(), "members/second/incan.toml", "[project]\nname = 'second'\n")?;

        let graph = WorkspaceGraph::discover(temp.path())?.ok_or("workspace should be discovered")?;
        assert!(graph.members.iter().all(|member| !member.root_member));
        assert_eq!(
            graph
                .selected_members_for(temp.path())?
                .iter()
                .map(|member| member.name.as_str())
                .collect::<Vec<_>>(),
            ["first", "second"]
        );
        Ok(())
    }

    #[test]
    fn workspace_rejects_root_repeated_in_members_and_duplicate_names() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        write_manifest(
            root.path(),
            "incan.toml",
            "[project]\nname = 'root'\n[workspace]\nmembers = ['.']\n",
        )?;
        let error = WorkspaceGraph::discover(root.path())
            .err()
            .ok_or("root member should be rejected")?;
        assert!(error.to_string().contains("implicit root member"));

        let duplicate = tempfile::tempdir()?;
        write_manifest(
            duplicate.path(),
            "incan.toml",
            "[workspace]\nmembers = ['one', 'two']\n",
        )?;
        write_manifest(duplicate.path(), "one/incan.toml", "[project]\nname = 'same'\n")?;
        write_manifest(duplicate.path(), "two/incan.toml", "[project]\nname = 'same'\n")?;
        let error = WorkspaceGraph::discover(duplicate.path())
            .err()
            .ok_or("duplicate name should be rejected")?;
        assert!(error.to_string().contains("duplicated"));
        Ok(())
    }

    #[test]
    fn workspace_warns_for_unmatched_globs_and_rejects_invalid_defaults() -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        write_manifest(
            temp.path(),
            "incan.toml",
            "[project]\nname = 'root'\n[workspace]\nmembers = ['packages/*']\ndefault-members = ['missing']\n",
        )?;
        let error = WorkspaceGraph::discover(temp.path())
            .err()
            .ok_or("invalid default should fail")?;
        assert!(error.to_string().contains("default-members"));

        write_manifest(
            temp.path(),
            "incan.toml",
            "[project]\nname = 'root'\n[workspace]\nmembers = ['packages/*']\n",
        )?;
        let graph = WorkspaceGraph::discover(temp.path())?.ok_or("workspace should be discovered")?;
        assert!(graph.warnings()[0].contains("matched no directories"));
        Ok(())
    }

    #[test]
    fn workspace_selection_uses_deterministic_graph_order_and_rejects_unknown_members()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        write_manifest(
            temp.path(),
            "incan.toml",
            "[project]\nname = 'root'\n[workspace]\nmembers = ['members/*']\n",
        )?;
        write_manifest(temp.path(), "members/alpha/incan.toml", "[project]\nname = 'alpha'\n")?;
        write_manifest(temp.path(), "members/beta/incan.toml", "[project]\nname = 'beta'\n")?;
        let graph = WorkspaceGraph::discover(temp.path())?.ok_or("workspace should be discovered")?;

        let selectors = vec!["beta".to_string(), "alpha".to_string()];
        assert_eq!(
            graph
                .select_members(temp.path(), false, &selectors)?
                .iter()
                .map(|member| member.name.as_str())
                .collect::<Vec<_>>(),
            ["alpha", "beta"]
        );
        assert_eq!(
            graph
                .select_members(temp.path(), true, &[])?
                .iter()
                .map(|member| member.name.as_str())
                .collect::<Vec<_>>(),
            ["root", "alpha", "beta"]
        );
        let error = graph
            .select_members(temp.path(), false, &["missing".to_string()])
            .err()
            .ok_or("unknown member should fail")?;
        assert!(error.to_string().contains("does not resolve"));
        Ok(())
    }

    #[test]
    fn workspace_rejects_member_inheritance_without_a_shared_declaration() -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        write_manifest(temp.path(), "incan.toml", "[workspace]\nmembers = ['member']\n")?;
        write_manifest(
            temp.path(),
            "member/incan.toml",
            "[project]\nname = 'member'\n[rust-dependencies]\nserde = { workspace = true }\n",
        )?;
        let error = WorkspaceGraph::discover(temp.path())
            .err()
            .ok_or("undeclared inheritance should fail")?;
        assert!(error.to_string().contains("does not declare it"));
        Ok(())
    }

    #[test]
    fn workspace_rejects_missing_or_refined_library_inheritance() -> Result<(), Box<dyn std::error::Error>> {
        let missing = tempfile::tempdir()?;
        write_manifest(missing.path(), "incan.toml", "[workspace]\nmembers = ['member']\n")?;
        write_manifest(
            missing.path(),
            "member/incan.toml",
            "[project]\nname = 'member'\n[dependencies]\nshared = { workspace = true }\n",
        )?;
        let error = WorkspaceGraph::discover(missing.path())
            .err()
            .ok_or("undeclared library inheritance should fail")?;
        assert!(
            error
                .to_string()
                .contains("[workspace.dependencies] does not declare it")
        );

        let refined = tempfile::tempdir()?;
        write_manifest(
            refined.path(),
            "incan.toml",
            "[workspace]\nmembers = ['member']\n[workspace.dependencies]\nshared = { path = 'shared' }\n",
        )?;
        write_manifest(
            refined.path(),
            "member/incan.toml",
            "[project]\nname = 'member'\n[dependencies]\nshared = { workspace = true, path = '../other' }\n",
        )?;
        let error = WorkspaceGraph::discover(refined.path())
            .err()
            .ok_or("refined library inheritance should fail")?;
        assert!(error.to_string().contains("may only set `workspace = true`"));
        Ok(())
    }

    #[test]
    fn workspace_inspection_reports_missing_valid_and_invalid_root_locks() -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        write_manifest(temp.path(), "incan.toml", "[workspace]\nmembers = ['member']\n")?;
        write_manifest(temp.path(), "member/incan.toml", "[project]\nname = 'member'\n")?;

        let missing = WorkspaceGraph::discover(temp.path())?.ok_or("workspace should be discovered")?;
        assert_eq!(missing.lock().status, WorkspaceLockStatus::Missing);
        assert_eq!(missing.lock().path, missing.root().join("incan.lock"));
        fs::write(temp.path().join("member/incan.lock"), "version = 4\n")?;
        let with_stale_member_lock = WorkspaceGraph::discover(temp.path())?.ok_or("workspace should be discovered")?;
        assert_eq!(
            with_stale_member_lock.lock().stale_member_lockfiles,
            [with_stale_member_lock.members()[0].path.join("incan.lock")]
        );
        assert!(
            with_stale_member_lock
                .warnings()
                .iter()
                .any(|warning| warning.contains("member-local lockfile") && warning.contains("non-authoritative"))
        );

        IncanLock::new(
            "sha256:workspace-lock".to_string(),
            crate::lockfile::CargoFeatureSelection::default(),
            "version = 4\n".to_string(),
        )
        .write(&temp.path().join("incan.lock"))?;
        let valid = WorkspaceGraph::discover(temp.path())?.ok_or("workspace should be discovered")?;
        assert_eq!(valid.lock().status, WorkspaceLockStatus::Valid);
        assert_eq!(valid.lock().deps_fingerprint.as_deref(), Some("sha256:workspace-lock"));

        fs::write(temp.path().join("incan.lock"), "[incan\n")?;
        let invalid = WorkspaceGraph::discover(temp.path())?.ok_or("workspace should be discovered")?;
        assert_eq!(invalid.lock().status, WorkspaceLockStatus::Invalid);
        assert!(invalid.lock().error.is_some());
        Ok(())
    }
}
