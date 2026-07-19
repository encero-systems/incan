//! RFC 077 workspace manifest discovery and graph construction.
//!
//! This module owns the validated, filesystem-backed topology shared by later workspace selection, lock, and
//! inspection work. It deliberately has no CLI, dependency-resolution, or lockfile behavior.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path, PathBuf};

use globset::{GlobBuilder, GlobMatcher};

use crate::manifest::{
    DependencySpec, EnvSection, LibraryDependencySpec, MANIFEST_FILENAME, ManifestError, ProjectManifest,
    WorkspaceRustDependencyRequest, WorkspaceSection, WorkspaceSharedDependencies,
};

/// One warning preserved while building an otherwise valid workspace graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkspaceWarning {
    /// A member or exclusion glob expanded to no project manifests.
    UnmatchedGlob {
        /// The manifest declaration that did not match a project.
        pattern: String,
    },
}

/// A validated project member in an RFC 077 workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceMember {
    name: String,
    root: PathBuf,
    manifest_path: PathBuf,
    is_root_member: bool,
}

/// Whether an effective member dependency was declared locally or inherited from the workspace root.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceDependencyOrigin {
    /// The member owns this dependency declaration directly.
    Member,
    /// The workspace root owns identity and this member explicitly inherited it.
    Workspace,
}

/// An effective Incan library dependency with its declaration provenance.
#[derive(Debug, Clone)]
pub struct EffectiveLibraryDependency {
    spec: LibraryDependencySpec,
    origin: WorkspaceDependencyOrigin,
}

impl EffectiveLibraryDependency {
    /// Fully resolved library dependency specification.
    pub fn spec(&self) -> &LibraryDependencySpec {
        &self.spec
    }

    /// Whether the member or the workspace root supplied the declaration.
    pub fn origin(&self) -> WorkspaceDependencyOrigin {
        self.origin
    }
}

/// An effective Rust dependency with workspace feature provenance.
#[derive(Debug, Clone)]
pub struct EffectiveRustDependency {
    spec: DependencySpec,
    origin: WorkspaceDependencyOrigin,
    workspace_features: Vec<String>,
    member_features: Vec<String>,
}

impl EffectiveRustDependency {
    /// Fully resolved dependency specification used by this member.
    pub fn spec(&self) -> &DependencySpec {
        &self.spec
    }

    /// Whether the member or the workspace root supplied the declaration identity.
    pub fn origin(&self) -> WorkspaceDependencyOrigin {
        self.origin
    }

    /// Features requested by the workspace root before member refinements.
    pub fn workspace_features(&self) -> &[String] {
        &self.workspace_features
    }

    /// Additional features requested by this member.
    pub fn member_features(&self) -> &[String] {
        &self.member_features
    }
}

/// Complete effective dependency view for one selected workspace member.
#[derive(Debug, Clone, Default)]
pub struct WorkspaceMemberDependencies {
    library_dependencies: BTreeMap<String, EffectiveLibraryDependency>,
    rust_dependencies: BTreeMap<String, EffectiveRustDependency>,
    rust_dev_dependencies: BTreeMap<String, EffectiveRustDependency>,
}

impl WorkspaceMemberDependencies {
    /// Effective Incan library dependencies keyed by manifest name.
    pub fn library_dependencies(&self) -> &BTreeMap<String, EffectiveLibraryDependency> {
        &self.library_dependencies
    }

    /// Effective Rust dependencies keyed by manifest name.
    pub fn rust_dependencies(&self) -> &BTreeMap<String, EffectiveRustDependency> {
        &self.rust_dependencies
    }

    /// Effective Rust development dependencies keyed by manifest name.
    pub fn rust_dev_dependencies(&self) -> &BTreeMap<String, EffectiveRustDependency> {
        &self.rust_dev_dependencies
    }
}

impl WorkspaceMember {
    /// The unique `[project].name` identifying this workspace member.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Canonical project directory for this member.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Canonical `incan.toml` path for this member.
    pub fn manifest_path(&self) -> &Path {
        &self.manifest_path
    }

    /// Whether this member is the implicit project colocated with the workspace declaration.
    pub fn is_root_member(&self) -> bool {
        self.is_root_member
    }
}

/// The rule that produced one resolved workspace command scope.
///
/// This is deliberately part of the compiler-facing model rather than a CLI detail: diagnostics, JSON inspection,
/// and future command reports must be able to explain why a member set was selected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceScopeOrigin {
    /// The invocation started inside one workspace member.
    CurrentMember,
    /// The invocation started at the root and selected explicit `default-members`.
    DefaultMembers,
    /// The invocation started at a rooted workspace root without `default-members`.
    ImplicitRootMember,
    /// The caller explicitly requested every workspace member.
    Workspace,
    /// The caller explicitly named one or more members.
    ExplicitMembers,
}

impl WorkspaceScopeOrigin {
    /// Stable machine-readable name for the rule that selected a workspace scope.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::CurrentMember => "current_member",
            Self::DefaultMembers => "default_members",
            Self::ImplicitRootMember => "implicit_root_member",
            Self::Workspace => "workspace",
            Self::ExplicitMembers => "explicit_members",
        }
    }
}

/// Raw command-selection inputs supplied by a workspace-aware caller.
///
/// `current_dir` is retained even for explicit selectors so reports can carry both the initiating directory and the
/// resolved scope. `select_workspace` corresponds to `--workspace`; `member_selectors` corresponds to repeatable
/// `--member` flags.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceScopeRequest {
    current_dir: PathBuf,
    select_workspace: bool,
    member_selectors: Vec<String>,
}

impl WorkspaceScopeRequest {
    /// Create an unqualified scope request from the process current directory.
    pub fn from_current_dir(current_dir: impl Into<PathBuf>) -> Self {
        Self {
            current_dir: current_dir.into(),
            select_workspace: false,
            member_selectors: Vec::new(),
        }
    }

    /// Create a scope request with the standard RFC 077 selectors.
    pub fn new(
        current_dir: impl Into<PathBuf>,
        select_workspace: bool,
        member_selectors: impl IntoIterator<Item = impl AsRef<str>>,
    ) -> Self {
        Self {
            current_dir: current_dir.into(),
            select_workspace,
            member_selectors: member_selectors
                .into_iter()
                .map(|selector| selector.as_ref().to_string())
                .collect(),
        }
    }

    /// Invocation directory before workspace canonicalization.
    pub fn current_dir(&self) -> &Path {
        &self.current_dir
    }

    /// Whether the caller requested every workspace member.
    pub fn select_workspace(&self) -> bool {
        self.select_workspace
    }

    /// Repeatable explicit member selectors in caller order.
    pub fn member_selectors(&self) -> impl Iterator<Item = &str> {
        self.member_selectors.iter().map(String::as_str)
    }
}

/// One validated, deterministically ordered workspace member scope.
#[derive(Debug, Clone)]
pub struct WorkspaceSelection<'graph> {
    workspace_root: &'graph Path,
    origin: WorkspaceScopeOrigin,
    members: Vec<&'graph WorkspaceMember>,
}

/// An owned, compiler-produced description of a resolved workspace command scope.
///
/// CLI commands, reports, and tooling projections use this after graph selection so they can retain the exact
/// workspace root, selection rule, and deterministic member order without independently rediscovering topology.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedWorkspaceScope {
    workspace_root: PathBuf,
    origin: WorkspaceScopeOrigin,
    members: Vec<WorkspaceMember>,
}

impl ResolvedWorkspaceScope {
    /// Canonical workspace root that owns the selected member set.
    pub fn workspace_root(&self) -> &Path {
        &self.workspace_root
    }

    /// The selection rule applied to the invocation.
    pub fn origin(&self) -> WorkspaceScopeOrigin {
        self.origin
    }

    /// Selected members in deterministic workspace order.
    pub fn members(&self) -> impl Iterator<Item = &WorkspaceMember> {
        self.members.iter()
    }

    /// Whether this scope contains exactly one member.
    pub fn is_single_member(&self) -> bool {
        self.members.len() == 1
    }

    /// Require one selected member for commands whose semantics cannot fan out safely.
    pub fn require_single_member(&self, command_name: &str) -> Result<&WorkspaceMember, WorkspaceError> {
        match self.members.as_slice() {
            [member] => Ok(member),
            _ => Err(WorkspaceError::Invalid {
                root: self.workspace_root.clone(),
                message: format!(
                    "{command_name} requires exactly one workspace member; select one with --member <name-or-path>"
                ),
            }),
        }
    }
}

impl<'graph> WorkspaceSelection<'graph> {
    /// The policy that selected this scope.
    pub fn origin(&self) -> WorkspaceScopeOrigin {
        self.origin
    }

    /// Selected members in deterministic workspace order.
    pub fn members(&self) -> impl Iterator<Item = &'graph WorkspaceMember> + '_ {
        self.members.iter().copied()
    }

    /// Selected member names in deterministic workspace order.
    pub fn member_names(&self) -> Vec<&'graph str> {
        self.members.iter().map(|member| member.name()).collect()
    }

    /// Preserve this resolved scope after the borrowed workspace graph is no longer needed.
    pub fn to_owned_scope(&self) -> ResolvedWorkspaceScope {
        ResolvedWorkspaceScope {
            workspace_root: self.workspace_root.to_path_buf(),
            origin: self.origin,
            members: self.members.iter().map(|member| (*member).clone()).collect(),
        }
    }

    /// Require exactly one selected member for commands such as `run` and `version`.
    pub fn require_single_member(&self, command_name: &str) -> Result<&'graph WorkspaceMember, WorkspaceError> {
        match self.members.as_slice() {
            [member] => Ok(*member),
            _ => Err(WorkspaceError::Invalid {
                root: self.workspace_root.to_path_buf(),
                message: format!(
                    "{command_name} requires exactly one workspace member; select one with --member <name-or-path>"
                ),
            }),
        }
    }
}

/// The deterministic, validated topology declared by one `[workspace]` root manifest.
#[derive(Debug, Clone)]
pub struct WorkspaceGraph {
    root: PathBuf,
    manifest_path: PathBuf,
    members: Vec<WorkspaceMember>,
    member_manifests: BTreeMap<PathBuf, ProjectManifest>,
    default_member_indices: Vec<usize>,
    exclusions: Vec<String>,
    warnings: Vec<WorkspaceWarning>,
}

impl WorkspaceGraph {
    /// Load the workspace declared by `root/incan.toml` and construct its complete member graph.
    ///
    /// This API validates topology only. Call [`Self::resolve_scope`] before any workspace-aware command reads or
    /// mutates member state.
    pub fn load_from_root(root: &Path) -> Result<Self, WorkspaceError> {
        let root = canonical_directory(root)?;
        let manifest_path = root.join(MANIFEST_FILENAME);
        let manifest = ProjectManifest::load(&manifest_path)?;
        let Some(workspace) = manifest.workspace().cloned() else {
            return Err(WorkspaceError::NotWorkspace { manifest_path });
        };
        Self::from_root_manifest(root, manifest, &workspace)
    }

    /// Discover the nearest ancestor workspace whose graph contains the nearest discovered project.
    ///
    /// An ancestor `[workspace]` that does not list the current project has no authority over it and is ignored. This
    /// preserves RFC 015 nearest-project discovery for projects that happen to live below unrelated workspace roots.
    pub fn discover(start_dir: &Path) -> Result<Option<Self>, WorkspaceError> {
        let Some(project) = ProjectManifest::discover(start_dir)? else {
            return Ok(None);
        };
        let project_root = canonical_directory(project.project_root())?;
        let starts_at_virtual_workspace_root = project.project.is_none()
            && project.workspace().is_some()
            && canonical_directory(start_dir).is_ok_and(|start| start == project_root);
        let mut candidate_root = project_root.clone();

        loop {
            let manifest_path = candidate_root.join(MANIFEST_FILENAME);
            if manifest_path.is_file() {
                let manifest = ProjectManifest::load(&manifest_path)?;
                if let Some(workspace) = manifest.workspace().cloned() {
                    let has_authority = starts_at_virtual_workspace_root && candidate_root == project_root
                        || workspace_might_contain_project(&candidate_root, &manifest, &workspace, &project_root);
                    match Self::from_root_manifest(candidate_root.clone(), manifest, &workspace) {
                        Ok(graph) if starts_at_virtual_workspace_root && candidate_root == project_root => {
                            return Ok(Some(graph));
                        }
                        Ok(graph) if graph.member_for_root(&project_root).is_some() => {
                            return Ok(Some(graph));
                        }
                        Ok(_) => {}
                        Err(error) if has_authority => return Err(error),
                        Err(_) => {}
                    }
                }
            }

            if !candidate_root.pop() {
                return Ok(None);
            }
        }
    }

    /// Canonical workspace root directory.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Canonical path to the root workspace manifest.
    pub fn manifest_path(&self) -> &Path {
        &self.manifest_path
    }

    /// Members in RFC 077 deterministic order: the implicit root first, then canonical non-root paths.
    pub fn members(&self) -> impl Iterator<Item = &WorkspaceMember> {
        self.members.iter()
    }

    /// Explicitly configured default members in their declaration order.
    ///
    /// An absent `default-members` declaration remains empty here; later command selection owns RFC 077's implicit
    /// root/every-member fallback instead of smuggling selection policy into this topology model.
    pub fn default_members(&self) -> impl Iterator<Item = &WorkspaceMember> {
        self.default_member_indices.iter().map(|index| &self.members[*index])
    }

    /// Raw exclusion declarations preserved for future inspection output.
    pub fn exclusions(&self) -> impl Iterator<Item = &str> {
        self.exclusions.iter().map(String::as_str)
    }

    /// Non-fatal topology warnings, such as a temporary unmatched member glob.
    pub fn warnings(&self) -> impl Iterator<Item = &WorkspaceWarning> {
        self.warnings.iter()
    }

    /// Return the member whose canonical root exactly matches `root`.
    pub fn member_for_root(&self, root: &Path) -> Option<&WorkspaceMember> {
        self.members.iter().find(|member| member.root == root)
    }

    /// Return the workspace member that owns `path`, preferring the deepest matching member root.
    ///
    /// Rooted workspaces can contain a non-root member below the root project. In that topology, both member roots
    /// are path prefixes, so callers must use the deepest match rather than allowing the root project to claim a
    /// descendant member's entrypoint or test file.
    pub(crate) fn member_containing_path(&self, path: &Path) -> Option<&WorkspaceMember> {
        self.member_containing_index(path).map(|index| &self.members[index])
    }

    /// Return the fully parsed manifest for one validated workspace member.
    pub fn member_manifest(&self, member: &WorkspaceMember) -> Option<&ProjectManifest> {
        self.member_manifests.get(member.root())
    }

    /// Return the parsed workspace-root manifest, including virtual roots without a project identity.
    pub fn workspace_manifest(&self) -> Result<&ProjectManifest, WorkspaceError> {
        self.member_manifests.get(&self.root).ok_or_else(|| {
            invalid_workspace(
                &self.root,
                "workspace root manifest was not retained during graph construction",
            )
        })
    }

    /// Return direct dependency identities declared by the workspace root.
    pub fn shared_dependencies(&self) -> Result<WorkspaceSharedDependencies, WorkspaceError> {
        self.workspace_manifest()?
            .workspace_shared_dependencies()
            .map_err(WorkspaceError::from)
    }

    /// Return named environment fragments owned by this workspace root.
    pub fn workspace_env_sections(&self) -> Result<BTreeMap<String, EnvSection>, WorkspaceError> {
        Ok(self
            .workspace_manifest()?
            .workspace()
            .map(|workspace| workspace.envs.clone())
            .unwrap_or_default())
    }

    /// Resolve one member's direct and explicitly inherited dependencies with RFC 077 provenance.
    ///
    /// This is intentionally separate from command execution: lock generation, inspection, and every workspace-aware
    /// lifecycle command consume the same effective dependency view rather than reimplementing `{ workspace = true }`.
    pub fn resolve_member_dependencies(
        &self,
        member: &WorkspaceMember,
    ) -> Result<WorkspaceMemberDependencies, WorkspaceError> {
        let manifest = self.member_manifest(member).ok_or_else(|| {
            invalid_workspace(
                &self.root,
                format!("workspace member {} has no parsed manifest", member.name()),
            )
        })?;
        let shared = self.shared_dependencies()?;

        let mut dependencies = WorkspaceMemberDependencies {
            library_dependencies: manifest
                .library_dependencies()
                .iter()
                .map(|(name, spec)| {
                    (
                        name.clone(),
                        EffectiveLibraryDependency {
                            spec: spec.clone(),
                            origin: WorkspaceDependencyOrigin::Member,
                        },
                    )
                })
                .collect(),
            rust_dependencies: direct_rust_dependencies(manifest.rust_dependencies()),
            rust_dev_dependencies: direct_rust_dependencies(manifest.rust_dev_dependencies()),
        };

        for (name, request) in manifest.workspace_library_dependencies() {
            let spec = shared.library_dependencies.get(name).ok_or_else(|| {
                invalid_workspace(
                    &self.root,
                    format!(
                        "member {} inherits library dependency `{name}`, but [workspace.dependencies] does not declare it",
                        member.name()
                    ),
                )
            })?;
            dependencies.library_dependencies.insert(
                name.clone(),
                EffectiveLibraryDependency {
                    spec: spec.clone(),
                    origin: WorkspaceDependencyOrigin::Workspace,
                },
            );
            debug_assert_eq!(request.library_name, *name);
        }

        resolve_inherited_rust_dependencies(
            &self.root,
            member.name(),
            &shared.rust_dependencies,
            manifest.workspace_rust_dependencies(),
            &mut dependencies.rust_dependencies,
            "[workspace.rust-dependencies]",
        )?;
        resolve_inherited_rust_dependencies(
            &self.root,
            member.name(),
            &shared.rust_dev_dependencies,
            manifest.workspace_rust_dev_dependencies(),
            &mut dependencies.rust_dev_dependencies,
            "[workspace.rust-dev-dependencies]",
        )?;
        Ok(dependencies)
    }

    /// Materialize one member manifest with all validated workspace inheritance applied.
    ///
    /// Existing compiler stages deliberately continue to receive an ordinary project manifest. Workspace orchestration
    /// owns this conversion once, before invoking those stages, so inherited identity cannot be lost on one route.
    pub fn effective_member_manifest(&self, member: &WorkspaceMember) -> Result<ProjectManifest, WorkspaceError> {
        let manifest = self.member_manifest(member).ok_or_else(|| {
            invalid_workspace(
                &self.root,
                format!("workspace member {} has no parsed manifest", member.name()),
            )
        })?;
        let dependencies = self.resolve_member_dependencies(member)?;
        Ok(manifest.with_effective_dependencies(
            dependencies
                .library_dependencies()
                .iter()
                .map(|(name, dependency)| (name.clone(), dependency.spec().clone())),
            dependencies
                .rust_dependencies()
                .iter()
                .map(|(name, dependency)| (name.clone(), dependency.spec().clone())),
            dependencies
                .rust_dev_dependencies()
                .iter()
                .map(|(name, dependency)| (name.clone(), dependency.spec().clone())),
        ))
    }

    /// Resolve RFC 077 command scope before compilation, execution, locking, or mutation.
    ///
    /// Explicit selectors win over current-directory inference. All successful selections are returned in workspace
    /// order even when `default-members` or repeatable `--member` flags were supplied in another order.
    pub fn resolve_scope(&self, request: WorkspaceScopeRequest) -> Result<WorkspaceSelection<'_>, WorkspaceError> {
        if request.select_workspace && !request.member_selectors.is_empty() {
            return Err(invalid_workspace(
                &self.root,
                "--workspace cannot be combined with one or more --member selectors",
            ));
        }

        if request.select_workspace {
            return Ok(self.selection(WorkspaceScopeOrigin::Workspace, 0..self.members.len()));
        }

        if !request.member_selectors.is_empty() {
            let mut selected = BTreeSet::new();
            for selector in &request.member_selectors {
                selected.insert(self.resolve_member_selector(selector)?);
            }
            return Ok(self.selection(WorkspaceScopeOrigin::ExplicitMembers, selected));
        }

        let current_dir = canonical_directory(&request.current_dir)?;
        if current_dir == self.root {
            return Ok(self.root_scope());
        }
        if let Some(index) = self.member_containing_index(&current_dir) {
            return Ok(self.selection(WorkspaceScopeOrigin::CurrentMember, [index]));
        }

        if self.root_member_index().is_some() && current_dir.starts_with(&self.root) {
            return Ok(self.selection(WorkspaceScopeOrigin::CurrentMember, [0]));
        }

        Err(invalid_workspace(
            &self.root,
            format!(
                "current directory {} is not inside a workspace member; choose one with --member <name-or-path>",
                current_dir.display()
            ),
        ))
    }

    /// Build a selection from validated member indices, normalizing to workspace declaration order.
    fn selection(
        &self,
        origin: WorkspaceScopeOrigin,
        indices: impl IntoIterator<Item = usize>,
    ) -> WorkspaceSelection<'_> {
        let selected = indices.into_iter().collect::<BTreeSet<_>>();
        WorkspaceSelection {
            workspace_root: &self.root,
            origin,
            members: self
                .members
                .iter()
                .enumerate()
                .filter_map(|(index, member)| selected.contains(&index).then_some(member))
                .collect(),
        }
    }

    /// Resolve the workspace-root fallback prescribed by RFC 077.
    fn root_scope(&self) -> WorkspaceSelection<'_> {
        if !self.default_member_indices.is_empty() {
            return self.selection(
                WorkspaceScopeOrigin::DefaultMembers,
                self.default_member_indices.iter().copied(),
            );
        }
        if let Some(index) = self.root_member_index() {
            return self.selection(WorkspaceScopeOrigin::ImplicitRootMember, [index]);
        }
        self.selection(WorkspaceScopeOrigin::Workspace, 0..self.members.len())
    }

    /// Return the member containing a canonical invocation directory, preferring the deepest match defensively.
    fn member_containing_index(&self, directory: &Path) -> Option<usize> {
        self.members
            .iter()
            .enumerate()
            .filter(|(_, member)| directory.starts_with(member.root()))
            .max_by_key(|(_, member)| member.root().components().count())
            .map(|(index, _)| index)
    }

    /// Return the implicit root member index when the root carries `[project]`.
    fn root_member_index(&self) -> Option<usize> {
        self.members.iter().position(WorkspaceMember::is_root_member)
    }

    /// Resolve one explicit `--member` selector by unique name or a root-relative path.
    fn resolve_member_selector(&self, selector: &str) -> Result<usize, WorkspaceError> {
        let selector = selector.trim();
        if selector.is_empty() {
            return Err(invalid_workspace(
                &self.root,
                "--member must name a workspace member or root-relative path",
            ));
        }

        let by_name = self.members.iter().position(|member| member.name() == selector);
        let by_path = self.member_index_for_selector_path(selector)?;
        match (by_name, by_path) {
            (Some(name), Some(path)) if name != path => Err(invalid_workspace(
                &self.root,
                format!(
                    "--member selector {selector:?} is ambiguous: it names {} but points to {}",
                    self.members[name].name(),
                    self.members[path].name()
                ),
            )),
            (Some(index), _) | (_, Some(index)) => Ok(index),
            (None, None) => Err(invalid_workspace(
                &self.root,
                format!("--member selector {selector:?} does not identify a workspace member"),
            )),
        }
    }

    /// Resolve the path interpretation of an explicit selector without treating a missing path as a filesystem error.
    fn member_index_for_selector_path(&self, selector: &str) -> Result<Option<usize>, WorkspaceError> {
        if selector == "." {
            return Ok(self.root_member_index());
        }
        let path = Path::new(selector);
        if path.is_absolute() {
            return Err(invalid_workspace(
                &self.root,
                format!("--member path {selector:?} must be relative to the workspace root"),
            ));
        }
        let Some(relative) = normalized_relative_declaration(selector) else {
            return Err(invalid_workspace(
                &self.root,
                format!("--member path {selector:?} must stay beneath the workspace root"),
            ));
        };
        let candidate = self.root.join(relative);
        let canonical = match fs::canonicalize(&candidate) {
            Ok(path) => path,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(source) => {
                return Err(WorkspaceError::Canonicalize {
                    path: candidate,
                    source,
                });
            }
        };
        if !canonical.starts_with(&self.root) {
            return Err(invalid_workspace(
                &self.root,
                format!("--member path {selector:?} resolves outside the workspace root"),
            ));
        }
        Ok(self.members.iter().position(|member| member.root() == canonical))
    }

    /// Build one graph after the root manifest has already been parsed as an RFC 077 workspace declaration.
    fn from_root_manifest(
        root: PathBuf,
        manifest: ProjectManifest,
        workspace: &WorkspaceSection,
    ) -> Result<Self, WorkspaceError> {
        let manifest_path = root.join(MANIFEST_FILENAME);
        // Validate workspace-owned dependency identity at graph construction time so no command can observe a valid
        // topology while silently carrying malformed shared configuration.
        let _shared_dependencies = manifest.workspace_shared_dependencies()?;
        let discovered = discover_project_manifests(&root)?;
        let mut warnings = Vec::new();
        let exclusions = compile_exclusions(&root, &workspace.exclude)?;
        let member_paths = expand_member_paths(&root, workspace, &discovered, &mut warnings)?;
        let member_paths = member_paths
            .into_iter()
            .filter(|path| !exclusions.iter().any(|exclude| exclude.matches(path)))
            .collect::<BTreeSet<_>>();

        let mut members = Vec::new();
        let mut member_manifests = BTreeMap::new();
        member_manifests.insert(root.clone(), manifest.clone());
        if let Some(project) = &manifest.project {
            members.push(root_member(&root, project.name.as_deref())?);
        }

        for member_root in member_paths {
            let (member, member_manifest) = load_non_root_member(&root, &member_root)?;
            member_manifests.insert(member.root().to_path_buf(), member_manifest);
            members.push(member);
        }

        if manifest.project.is_none() && members.is_empty() {
            return Err(invalid_workspace(
                &root,
                "a virtual workspace must expand at least one non-root member",
            ));
        }
        validate_member_names(&root, &members)?;
        let default_member_indices = resolve_default_members(&root, workspace, &members)?;

        Ok(Self {
            root,
            manifest_path,
            members,
            member_manifests,
            default_member_indices,
            exclusions: workspace.exclude.clone(),
            warnings,
        })
    }
}

/// Convert member-owned Rust dependencies into the effective view without workspace provenance.
fn direct_rust_dependencies(
    dependencies: &std::collections::HashMap<String, DependencySpec>,
) -> BTreeMap<String, EffectiveRustDependency> {
    dependencies
        .iter()
        .map(|(name, spec)| {
            (
                name.clone(),
                EffectiveRustDependency {
                    spec: spec.clone(),
                    origin: WorkspaceDependencyOrigin::Member,
                    workspace_features: Vec::new(),
                    member_features: spec.features.clone(),
                },
            )
        })
        .collect()
}

/// Apply explicit Rust workspace inheritance while preserving every source of feature activation.
fn resolve_inherited_rust_dependencies(
    root: &Path,
    member_name: &str,
    shared: &BTreeMap<String, DependencySpec>,
    requests: &BTreeMap<String, WorkspaceRustDependencyRequest>,
    effective: &mut BTreeMap<String, EffectiveRustDependency>,
    shared_table_name: &str,
) -> Result<(), WorkspaceError> {
    for (name, request) in requests {
        let shared_spec = shared.get(name).ok_or_else(|| {
            invalid_workspace(
                root,
                format!(
                    "member {member_name} inherits Rust dependency `{name}`, but {shared_table_name} does not declare it"
                ),
            )
        })?;
        let mut spec = shared_spec.clone();
        let workspace_features = spec.features.clone();
        let mut member_features = request.features.clone();
        member_features.sort();
        member_features.dedup();
        spec.features.extend(member_features.iter().cloned());
        spec.features.sort();
        spec.features.dedup();
        spec.optional = request.optional;
        effective.insert(
            name.clone(),
            EffectiveRustDependency {
                spec,
                origin: WorkspaceDependencyOrigin::Workspace,
                workspace_features,
                member_features,
            },
        );
    }
    Ok(())
}

/// Determine whether an ancestor declaration can govern a discovered project before fully validating its graph.
///
/// A malformed workspace that does not select the current project must not break RFC 015 project discovery: RFC 077
/// gives it no authority. This intentionally small preflight considers only whether the project's canonical path is
/// structurally included and not excluded; full topology validation remains owned by [`WorkspaceGraph`].
fn workspace_might_contain_project(
    root: &Path,
    manifest: &ProjectManifest,
    workspace: &WorkspaceSection,
    project_root: &Path,
) -> bool {
    if root == project_root {
        return manifest.project.is_some();
    }
    if !project_root.starts_with(root) {
        return false;
    }
    let member_selected = workspace
        .members
        .iter()
        .any(|declaration| declaration_matches_project(root, declaration, project_root));
    member_selected
        && !workspace
            .exclude
            .iter()
            .any(|declaration| declaration_matches_project(root, declaration, project_root))
}

/// Return whether one member or exclusion declaration can match a project's canonical root.
fn declaration_matches_project(root: &Path, declaration: &str, project_root: &Path) -> bool {
    if has_glob_magic(declaration) {
        return project_root.strip_prefix(root).is_ok_and(|relative| {
            GlobBuilder::new(declaration)
                .literal_separator(true)
                .build()
                .is_ok_and(|glob| glob.compile_matcher().is_match(relative))
        });
    }
    let Some(relative) = normalized_relative_declaration(declaration) else {
        return false;
    };
    let candidate = root.join(relative);
    fs::canonicalize(&candidate).map_or(candidate, |path| path) == project_root
}

/// Errors raised while parsing or validating the filesystem-backed workspace topology.
#[derive(Debug, thiserror::Error)]
pub enum WorkspaceError {
    /// The root manifest did not declare a `[workspace]` table.
    #[error("{manifest_path} does not declare a [workspace] table")]
    NotWorkspace {
        /// The manifest requested as a workspace root.
        manifest_path: PathBuf,
    },
    /// A manifest failed RFC 015/RFC 077 parsing before graph construction could continue.
    #[error(transparent)]
    Manifest(#[from] ManifestError),
    /// A filesystem operation needed for deterministic discovery failed.
    #[error("failed to read workspace directory {path}: {source}")]
    ReadDirectory {
        /// The directory that could not be listed.
        path: PathBuf,
        /// The underlying filesystem error.
        source: std::io::Error,
    },
    /// Canonicalizing a workspace path failed.
    #[error("failed to canonicalize workspace path {path}: {source}")]
    Canonicalize {
        /// The path whose canonical form was required.
        path: PathBuf,
        /// The underlying filesystem error.
        source: std::io::Error,
    },
    /// A literal non-root member did not identify a project manifest.
    #[error("workspace {root} declares member {member}, but it does not contain {manifest_filename}")]
    MissingMember {
        /// Canonical workspace root.
        root: PathBuf,
        /// Root-relative member declaration.
        member: String,
        /// The required manifest filename.
        manifest_filename: &'static str,
    },
    /// A workspace declaration violated RFC 077 topology rules.
    #[error("invalid workspace {root}: {message}")]
    Invalid {
        /// Canonical workspace root.
        root: PathBuf,
        /// Actionable validation detail.
        message: String,
    },
}

/// A candidate non-root project manifest discovered below a workspace root.
#[derive(Debug)]
struct DiscoveredProject {
    root: PathBuf,
    relative_root: PathBuf,
}

/// One compiled exclusion declaration used while filtering expanded members.
enum ExclusionMatcher {
    Literal(PathBuf),
    Glob { root: PathBuf, matcher: GlobMatcher },
}

impl ExclusionMatcher {
    /// Return whether this exclusion removes the supplied canonical member root.
    fn matches(&self, member_root: &Path) -> bool {
        match self {
            Self::Literal(path) => path == member_root,
            Self::Glob { root, matcher } => member_root
                .strip_prefix(root)
                .is_ok_and(|relative| matcher.is_match(relative)),
        }
    }
}

/// Build the implicit root member from the colocated `[project]` section.
fn root_member(root: &Path, name: Option<&str>) -> Result<WorkspaceMember, WorkspaceError> {
    let name = required_project_name(root, name, "the implicit root member")?;
    Ok(WorkspaceMember {
        name,
        root: root.to_path_buf(),
        manifest_path: root.join(MANIFEST_FILENAME),
        is_root_member: true,
    })
}

/// Parse and validate one explicitly listed non-root workspace member.
fn load_non_root_member(root: &Path, member_root: &Path) -> Result<(WorkspaceMember, ProjectManifest), WorkspaceError> {
    let manifest_path = member_root.join(MANIFEST_FILENAME);
    let manifest = ProjectManifest::load(&manifest_path)?;
    if manifest.workspace().is_some() {
        return Err(invalid_workspace(
            root,
            format!(
                "nested workspace member {} is not supported",
                display_workspace_relative(root, member_root)
            ),
        ));
    }
    let name = required_project_name(
        root,
        manifest.project.as_ref().and_then(|project| project.name.as_deref()),
        "a member",
    )?;
    Ok((
        WorkspaceMember {
            name,
            root: member_root.to_path_buf(),
            manifest_path,
            is_root_member: false,
        },
        manifest,
    ))
}

/// Reject missing or whitespace-only `[project].name` fields at the workspace membership boundary.
fn required_project_name(root: &Path, name: Option<&str>, owner: &str) -> Result<String, WorkspaceError> {
    let Some(name) = name.map(str::trim).filter(|name| !name.is_empty()) else {
        return Err(invalid_workspace(
            root,
            format!("{owner} must declare a non-empty [project].name"),
        ));
    };
    Ok(name.to_string())
}

/// Discover every project manifest below `root` without following symlinked directories.
fn discover_project_manifests(root: &Path) -> Result<Vec<DiscoveredProject>, WorkspaceError> {
    let mut directories = vec![root.to_path_buf()];
    let mut projects = Vec::new();

    while let Some(directory) = directories.pop() {
        let entries = fs::read_dir(&directory).map_err(|source| WorkspaceError::ReadDirectory {
            path: directory.clone(),
            source,
        })?;
        for entry in entries {
            let entry = entry.map_err(|source| WorkspaceError::ReadDirectory {
                path: directory.clone(),
                source,
            })?;
            let path = entry.path();
            let file_type = entry.file_type().map_err(|source| WorkspaceError::ReadDirectory {
                path: path.clone(),
                source,
            })?;
            if file_type.is_dir() {
                directories.push(path);
            } else if file_type.is_file() && entry.file_name() == MANIFEST_FILENAME {
                let Some(member_root) = path.parent() else {
                    continue;
                };
                let member_root = canonical_directory(member_root)?;
                if member_root == root {
                    continue;
                }
                ensure_contained(root, &member_root, "discovered member")?;
                let relative_root = member_root.strip_prefix(root).map(Path::to_path_buf).map_err(|_| {
                    invalid_workspace(
                        root,
                        format!(
                            "discovered member {} is outside the workspace root",
                            member_root.display()
                        ),
                    )
                })?;
                projects.push(DiscoveredProject {
                    root: member_root,
                    relative_root,
                });
            }
        }
    }

    projects.sort_by(|left, right| left.root.cmp(&right.root));
    projects.dedup_by(|left, right| left.root == right.root);
    Ok(projects)
}

/// Expand member declarations before exclusions are applied.
fn expand_member_paths(
    root: &Path,
    workspace: &WorkspaceSection,
    discovered: &[DiscoveredProject],
    warnings: &mut Vec<WorkspaceWarning>,
) -> Result<BTreeSet<PathBuf>, WorkspaceError> {
    let mut members = BTreeSet::new();
    for declaration in &workspace.members {
        if has_glob_magic(declaration) {
            let matcher = compile_glob(root, declaration, "member")?;
            let matches = discovered
                .iter()
                .filter(|project| matcher.is_match(&project.relative_root))
                .map(|project| project.root.clone())
                .collect::<Vec<_>>();
            if matches.is_empty() {
                warnings.push(WorkspaceWarning::UnmatchedGlob {
                    pattern: declaration.clone(),
                });
            }
            members.extend(matches);
        } else {
            let member_root = resolve_literal_workspace_path(root, declaration, "member")?;
            let manifest_path = member_root.join(MANIFEST_FILENAME);
            if !manifest_path.is_file() {
                return Err(WorkspaceError::MissingMember {
                    root: root.to_path_buf(),
                    member: declaration.clone(),
                    manifest_filename: MANIFEST_FILENAME,
                });
            }
            members.insert(member_root);
        }
    }
    Ok(members)
}

/// Compile exclusions into canonical literal and glob matchers before member filtering.
fn compile_exclusions(root: &Path, declarations: &[String]) -> Result<Vec<ExclusionMatcher>, WorkspaceError> {
    let mut exclusions = Vec::new();
    for declaration in declarations {
        if has_glob_magic(declaration) {
            exclusions.push(ExclusionMatcher::Glob {
                root: root.to_path_buf(),
                matcher: compile_glob(root, declaration, "exclude")?,
            });
        } else {
            exclusions.push(ExclusionMatcher::Literal(resolve_literal_exclusion_path(
                root,
                declaration,
            )?));
        }
    }
    Ok(exclusions)
}

/// Build a separator-aware root-relative glob matcher.
fn compile_glob(root: &Path, declaration: &str, kind: &str) -> Result<GlobMatcher, WorkspaceError> {
    validate_relative_declaration(root, declaration, kind)?;
    GlobBuilder::new(declaration)
        .literal_separator(true)
        .build()
        .map(|glob| glob.compile_matcher())
        .map_err(|error| invalid_workspace(root, format!("{kind} glob `{declaration}` is invalid: {error}")))
}

/// Resolve one literal root-relative declaration while preventing root repetition and path escape.
fn resolve_literal_workspace_path(root: &Path, declaration: &str, kind: &str) -> Result<PathBuf, WorkspaceError> {
    let relative = validate_relative_declaration(root, declaration, kind)?;
    let candidate = root.join(relative);
    let canonical = match fs::canonicalize(&candidate) {
        Ok(path) => path,
        Err(_) if kind == "member" => {
            return Err(WorkspaceError::MissingMember {
                root: root.to_path_buf(),
                member: declaration.to_string(),
                manifest_filename: MANIFEST_FILENAME,
            });
        }
        Err(source) => {
            return Err(WorkspaceError::Canonicalize {
                path: candidate,
                source,
            });
        }
    };
    if canonical == root {
        return Err(invalid_workspace(
            root,
            format!("{kind} declaration `{declaration}` repeats root membership already established by [project]"),
        ));
    }
    ensure_contained(root, &canonical, kind)?;
    Ok(canonical)
}

/// Resolve one exclusion literal without requiring it to exist yet.
///
/// Exclusions deliberately tolerate temporary non-existent paths so repository transitions remain inspectable. If the
/// path does exist, canonicalization still prevents a symbolic link from escaping the workspace boundary.
fn resolve_literal_exclusion_path(root: &Path, declaration: &str) -> Result<PathBuf, WorkspaceError> {
    let relative = validate_relative_declaration(root, declaration, "exclude")?;
    let candidate = root.join(relative);
    if !candidate.exists() {
        return Ok(candidate);
    }
    let canonical = fs::canonicalize(&candidate).map_err(|source| WorkspaceError::Canonicalize {
        path: candidate,
        source,
    })?;
    if canonical == root {
        return Err(invalid_workspace(
            root,
            format!("exclude declaration `{declaration}` repeats root membership already established by [project]"),
        ));
    }
    ensure_contained(root, &canonical, "exclude")?;
    Ok(canonical)
}

/// Validate that a declaration is a non-root path below the workspace before filesystem resolution.
fn validate_relative_declaration(root: &Path, declaration: &str, kind: &str) -> Result<PathBuf, WorkspaceError> {
    let Some(normalized) = normalized_relative_declaration(declaration) else {
        return Err(invalid_workspace(
            root,
            format!("{kind} declaration `{declaration}` must be a non-empty root-relative path or glob"),
        ));
    };
    if normalized.as_os_str().is_empty() {
        return Err(invalid_workspace(
            root,
            format!("{kind} declaration `{declaration}` repeats root membership already established by [project]"),
        ));
    }
    Ok(normalized)
}

/// Normalize one root-relative declaration without attaching a workspace-specific diagnostic.
fn normalized_relative_declaration(declaration: &str) -> Option<PathBuf> {
    let path = Path::new(declaration);
    if declaration.trim().is_empty() || path.is_absolute() {
        return None;
    }
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(segment) => normalized.push(segment),
            Component::ParentDir => {
                if !normalized.pop() {
                    return None;
                }
            }
            Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    Some(normalized)
}

/// Ensure one canonical path stays strictly beneath the canonical workspace root.
fn ensure_contained(root: &Path, path: &Path, kind: &str) -> Result<(), WorkspaceError> {
    if path == root || !path.starts_with(root) {
        return Err(invalid_workspace(
            root,
            format!(
                "{kind} path {} must remain beneath the canonical workspace root",
                path.display()
            ),
        ));
    }
    Ok(())
}

/// Reject duplicate project names because command selection must never silently pick one by path order.
fn validate_member_names(root: &Path, members: &[WorkspaceMember]) -> Result<(), WorkspaceError> {
    let mut names = BTreeMap::new();
    for member in members {
        if let Some(existing) = names.insert(member.name.as_str(), member.root()) {
            return Err(invalid_workspace(
                root,
                format!(
                    "duplicate member name `{}` for {} and {}",
                    member.name,
                    display_workspace_relative(root, existing),
                    display_workspace_relative(root, member.root())
                ),
            ));
        }
    }
    Ok(())
}

/// Resolve explicit defaults to their unique member indices without implementing command-scope fallback policy.
fn resolve_default_members(
    root: &Path,
    workspace: &WorkspaceSection,
    members: &[WorkspaceMember],
) -> Result<Vec<usize>, WorkspaceError> {
    let mut defaults = Vec::new();
    for declaration in &workspace.default_members {
        let member_path = default_member_path(root, declaration)?;
        let matches = members
            .iter()
            .enumerate()
            .filter(|(_, member)| {
                member.name == *declaration || member_path.as_ref().is_some_and(|path| member.root == *path)
            })
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        match matches.as_slice() {
            [index] => {
                if !defaults.contains(index) {
                    defaults.push(*index);
                }
            }
            [] => {
                return Err(invalid_workspace(
                    root,
                    format!("default member `{declaration}` does not resolve to a workspace member"),
                ));
            }
            _ => {
                return Err(invalid_workspace(
                    root,
                    format!("default member `{declaration}` is ambiguous"),
                ));
            }
        }
    }
    Ok(defaults)
}

/// Resolve the path interpretation of one default declaration without discarding its simultaneous name interpretation.
///
/// RFC 077 permits `default-members` entries to name either a member or a root-relative path. A simple segment can be
/// both, so callers compare this canonical path alongside the name and reject any resulting ambiguity.
fn default_member_path(root: &Path, declaration: &str) -> Result<Option<PathBuf>, WorkspaceError> {
    if declaration == "." {
        return Ok(Some(root.to_path_buf()));
    }
    let path = Path::new(declaration);
    if path.is_absolute() {
        return Err(invalid_workspace(
            root,
            format!("default member `{declaration}` must be a member name or root-relative path"),
        ));
    }
    let relative = validate_relative_declaration(root, declaration, "default member")?;
    let candidate = root.join(relative);
    if !candidate.exists() {
        return Ok(Some(candidate));
    }
    let canonical = fs::canonicalize(&candidate).map_err(|source| WorkspaceError::Canonicalize {
        path: candidate,
        source,
    })?;
    if canonical != root {
        ensure_contained(root, &canonical, "default member")?;
    }
    Ok(Some(canonical))
}

/// Return a stable slash-separated path for diagnostics relative to the workspace root.
fn display_workspace_relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .map(|relative| relative.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|_| path.display().to_string())
}

/// Return whether a declaration requires glob expansion rather than literal member validation.
fn has_glob_magic(value: &str) -> bool {
    value.contains(['*', '?', '[', '{'])
}

/// Canonicalize one directory before it becomes part of the graph identity.
fn canonical_directory(path: &Path) -> Result<PathBuf, WorkspaceError> {
    fs::canonicalize(path).map_err(|source| WorkspaceError::Canonicalize {
        path: path.to_path_buf(),
        source,
    })
}

/// Construct one topology validation error at the canonical workspace boundary.
fn invalid_workspace(root: &Path, message: impl Into<String>) -> WorkspaceError {
    WorkspaceError::Invalid {
        root: root.to_path_buf(),
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use super::{WorkspaceDependencyOrigin, WorkspaceGraph, WorkspaceScopeOrigin, WorkspaceScopeRequest};
    use crate::manifest::MANIFEST_FILENAME;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    #[test]
    fn rooted_workspace_orders_root_then_canonical_members() -> TestResult {
        let root = tempfile::tempdir()?;
        write_manifest(
            root.path(),
            r#"
[project]
name = "root"

[workspace]
members = ["packages/*"]
"#,
        )?;
        write_project(root.path().join("packages/zebra"), "zebra")?;
        write_project(root.path().join("packages/alpha"), "alpha")?;

        let graph = WorkspaceGraph::load_from_root(root.path())?;
        let names = graph.members().map(|member| member.name()).collect::<Vec<_>>();

        assert_eq!(names, vec!["root", "alpha", "zebra"]);
        assert!(graph.members().next().is_some_and(|member| member.is_root_member()));
        Ok(())
    }

    #[test]
    fn virtual_workspace_requires_and_loads_non_root_members() -> TestResult {
        let root = tempfile::tempdir()?;
        write_manifest(
            root.path(),
            r#"
[workspace]
members = ["packages/*"]
default-members = ["beta"]
"#,
        )?;
        write_project(root.path().join("packages/beta"), "beta")?;

        let graph = WorkspaceGraph::load_from_root(root.path())?;

        assert_eq!(graph.members().count(), 1);
        assert_eq!(
            graph.default_members().map(|member| member.name()).collect::<Vec<_>>(),
            vec!["beta"]
        );
        Ok(())
    }

    #[test]
    fn root_membership_and_missing_literal_members_are_rejected() -> TestResult {
        let root = tempfile::tempdir()?;
        write_manifest(
            root.path(),
            r#"
[project]
name = "root"

[workspace]
members = [".", "packages/missing"]
"#,
        )?;

        let error = WorkspaceGraph::load_from_root(root.path())
            .err()
            .ok_or("workspace should be invalid")?;
        assert!(
            error.to_string().contains("root membership"),
            "unexpected error: {error}"
        );
        Ok(())
    }

    #[test]
    fn missing_literal_members_are_rejected_after_root_membership_validation() -> TestResult {
        let root = tempfile::tempdir()?;
        write_manifest(
            root.path(),
            r#"
[project]
name = "root"

[workspace]
members = ["packages/missing"]
"#,
        )?;

        let error = WorkspaceGraph::load_from_root(root.path())
            .err()
            .ok_or("workspace should be invalid")?;
        assert!(
            error.to_string().contains("does not contain incan.toml"),
            "unexpected error: {error}"
        );
        Ok(())
    }

    #[test]
    fn exclusions_apply_after_glob_expansion_and_unmatched_globs_warn() -> TestResult {
        let root = tempfile::tempdir()?;
        write_manifest(
            root.path(),
            r#"
[project]
name = "root"

[workspace]
members = ["packages/*", "examples/*"]
exclude = ["packages/i*"]
"#,
        )?;
        write_project(root.path().join("packages/kept"), "kept")?;
        write_project(root.path().join("packages/ignored"), "ignored")?;

        let graph = WorkspaceGraph::load_from_root(root.path())?;
        let names = graph.members().map(|member| member.name()).collect::<Vec<_>>();

        assert_eq!(names, vec!["root", "kept"]);
        assert_eq!(graph.warnings().count(), 1);
        Ok(())
    }

    #[test]
    fn discovery_skips_ancestor_workspaces_that_do_not_contain_the_project() -> TestResult {
        let outer = tempfile::tempdir()?;
        write_manifest(
            outer.path(),
            r#"
[project]
name = "outer"

[workspace]
members = ["."]
"#,
        )?;
        write_project(outer.path().join("unmanaged/project"), "unmanaged")?;

        let graph = WorkspaceGraph::discover(&outer.path().join("unmanaged/project/src"))?;

        assert!(graph.is_none());
        Ok(())
    }

    #[test]
    fn discovery_accepts_a_virtual_workspace_started_at_its_root() -> TestResult {
        let root = tempfile::tempdir()?;
        write_manifest(
            root.path(),
            r#"
[workspace]
members = ["packages/member"]
"#,
        )?;
        write_project(root.path().join("packages/member"), "member")?;

        let graph = WorkspaceGraph::discover(root.path())?.ok_or("workspace missing")?;

        assert_eq!(graph.root(), root.path().canonicalize()?.as_path());
        Ok(())
    }

    #[test]
    fn discovery_returns_the_nearest_workspace_that_contains_the_project() -> TestResult {
        let root = tempfile::tempdir()?;
        write_manifest(
            root.path(),
            r#"
[workspace]
members = ["packages/member"]
"#,
        )?;
        write_project(root.path().join("packages/member"), "member")?;

        let graph = WorkspaceGraph::discover(&root.path().join("packages/member/src"))?.ok_or("workspace missing")?;

        assert_eq!(graph.root(), root.path().canonicalize()?.as_path());
        assert_eq!(
            graph.members().map(|member| member.name()).collect::<Vec<_>>(),
            vec!["member"]
        );
        Ok(())
    }

    #[test]
    fn duplicate_member_names_and_invalid_defaults_are_rejected() -> TestResult {
        let root = tempfile::tempdir()?;
        write_manifest(
            root.path(),
            r#"
[workspace]
members = ["packages/*"]
default-members = ["missing"]
"#,
        )?;
        write_project(root.path().join("packages/one"), "duplicate")?;
        write_project(root.path().join("packages/two"), "duplicate")?;

        let error = WorkspaceGraph::load_from_root(root.path())
            .err()
            .ok_or("workspace should be invalid")?;
        assert!(
            error.to_string().contains("duplicate member name"),
            "unexpected error: {error}"
        );
        Ok(())
    }

    #[test]
    fn default_member_names_and_paths_cannot_resolve_to_different_members() -> TestResult {
        let root = tempfile::tempdir()?;
        write_manifest(
            root.path(),
            r#"
[workspace]
members = ["named", "other"]
default-members = ["named"]
"#,
        )?;
        write_project(root.path().join("named"), "different")?;
        write_project(root.path().join("other"), "named")?;

        let error = WorkspaceGraph::load_from_root(root.path())
            .err()
            .ok_or("workspace should be invalid")?;

        assert!(error.to_string().contains("is ambiguous"), "unexpected error: {error}");
        Ok(())
    }

    #[test]
    fn virtual_workspace_cannot_expand_to_zero_members() -> TestResult {
        let root = tempfile::tempdir()?;
        write_manifest(
            root.path(),
            r#"
[workspace]
members = ["packages/*"]
"#,
        )?;

        let error = WorkspaceGraph::load_from_root(root.path())
            .err()
            .ok_or("workspace should be invalid")?;

        assert!(
            error.to_string().contains("virtual workspace must expand"),
            "unexpected error: {error}"
        );
        Ok(())
    }

    #[test]
    fn nested_workspaces_are_rejected_as_members() -> TestResult {
        let root = tempfile::tempdir()?;
        write_manifest(
            root.path(),
            r#"
[workspace]
members = ["packages/nested"]
"#,
        )?;
        write_manifest(
            root.path().join("packages/nested"),
            r#"
[project]
name = "nested"

[workspace]
members = []
"#,
        )?;

        let error = WorkspaceGraph::load_from_root(root.path())
            .err()
            .ok_or("workspace should be invalid")?;

        assert!(
            error.to_string().contains("nested workspace member"),
            "unexpected error: {error}"
        );
        Ok(())
    }

    #[test]
    fn manifest_parses_workspace_tables_reserved_for_later_rfc_phases() -> TestResult {
        let manifest = crate::manifest::ProjectManifest::from_str(
            r#"
[workspace]
members = ["packages/*"]

[workspace.dependencies]
model = { path = "packages/model" }

[workspace.rust-dependencies]
serde = { version = "1", default-features = false }

[workspace.envs.ci]
requires-incan = ">=0.5,<0.6"

[workspace.policy.release]
approval = "required"

[workspace.sources.internal]
url = "https://packages.example.test"
"#,
            Path::new("incan.toml"),
        )?;
        let workspace = manifest.workspace().ok_or("workspace declaration missing")?;

        assert_eq!(workspace.members, vec!["packages/*"]);
        assert!(workspace.dependencies.contains_key("model"));
        assert!(workspace.rust_dependencies.contains_key("serde"));
        assert!(workspace.envs.contains_key("ci"));
        assert!(workspace.policy.contains_key("release"));
        assert!(workspace.sources.contains_key("internal"));
        Ok(())
    }

    #[test]
    fn scope_selection_respects_current_directory_defaults_and_explicit_selectors() -> TestResult {
        let root = tempfile::tempdir()?;
        write_manifest(
            root.path(),
            r#"
[project]
name = "root"

[workspace]
members = ["packages/*"]
default-members = ["zebra", "packages/alpha"]
"#,
        )?;
        write_project(root.path().join("packages/alpha"), "alpha")?;
        write_project(root.path().join("packages/zebra"), "zebra")?;
        fs::create_dir_all(root.path().join("packages/zebra/src"))?;

        let graph = WorkspaceGraph::load_from_root(root.path())?;

        let at_root = graph.resolve_scope(WorkspaceScopeRequest::from_current_dir(root.path()))?;
        assert_eq!(at_root.origin(), WorkspaceScopeOrigin::DefaultMembers);
        assert_eq!(at_root.member_names(), ["alpha", "zebra"]);

        let in_member = graph.resolve_scope(WorkspaceScopeRequest::from_current_dir(
            root.path().join("packages/zebra/src"),
        ))?;
        assert_eq!(in_member.origin(), WorkspaceScopeOrigin::CurrentMember);
        assert_eq!(in_member.member_names(), ["zebra"]);

        let explicit = graph.resolve_scope(WorkspaceScopeRequest::new(
            root.path(),
            false,
            ["zebra", ".", "packages/alpha", "zebra"],
        ))?;
        assert_eq!(explicit.origin(), WorkspaceScopeOrigin::ExplicitMembers);
        assert_eq!(explicit.member_names(), ["root", "alpha", "zebra"]);

        let all = graph.resolve_scope(WorkspaceScopeRequest::new(
            root.path(),
            true,
            std::iter::empty::<&str>(),
        ))?;
        assert_eq!(all.origin(), WorkspaceScopeOrigin::Workspace);
        assert_eq!(all.member_names(), ["root", "alpha", "zebra"]);
        Ok(())
    }

    #[test]
    fn member_path_ownership_prefers_the_deepest_rooted_workspace_member() -> TestResult {
        let root = tempfile::tempdir()?;
        write_manifest(
            root.path(),
            r#"
[project]
name = "root"

[workspace]
members = ["packages/consumer"]
"#,
        )?;
        write_project(root.path().join("packages/consumer"), "consumer")?;
        let graph = WorkspaceGraph::load_from_root(root.path())?;

        let root_owner = graph
            .member_containing_path(&graph.root().join("tests/test_root.incn"))
            .ok_or("root-owned path should resolve to a member")?;
        let consumer_owner = graph
            .member_containing_path(&graph.root().join("packages/consumer/tests/test_consumer.incn"))
            .ok_or("consumer-owned path should resolve to a member")?;

        assert_eq!(root_owner.name(), "root");
        assert_eq!(consumer_owner.name(), "consumer");
        Ok(())
    }

    #[test]
    fn scope_selection_rejects_conflicting_and_unknown_explicit_selectors() -> TestResult {
        let root = tempfile::tempdir()?;
        write_manifest(
            root.path(),
            r#"
[project]
name = "root"

[workspace]
members = ["packages/member"]
"#,
        )?;
        write_project(root.path().join("packages/member"), "member")?;
        let graph = WorkspaceGraph::load_from_root(root.path())?;

        let conflict = graph
            .resolve_scope(WorkspaceScopeRequest::new(root.path(), true, ["member"]))
            .err()
            .ok_or("conflicting selectors should fail")?;
        assert!(conflict.to_string().contains("cannot be combined"));

        let unknown = graph
            .resolve_scope(WorkspaceScopeRequest::new(root.path(), false, ["missing"]))
            .err()
            .ok_or("unknown selector should fail")?;
        assert!(unknown.to_string().contains("does not identify a workspace member"));
        Ok(())
    }

    #[test]
    fn effective_member_dependencies_preserve_workspace_identity_and_feature_provenance() -> TestResult {
        let root = tempfile::tempdir()?;
        write_manifest(
            root.path(),
            r#"
[project]
name = "root"

[workspace]
members = ["packages/member"]

[workspace.dependencies]
domain = { path = "libraries/domain" }

[workspace.rust-dependencies]
serde = { version = "1", features = ["alloc"], default-features = false }

[workspace.rust-dev-dependencies]
proptest = "1"
"#,
        )?;
        write_manifest(
            root.path().join("packages/member"),
            r#"
[project]
name = "member"

[dependencies]
domain = { workspace = true }

[rust-dependencies]
serde = { workspace = true, features = ["derive"], optional = true }

[rust-dev-dependencies]
proptest = { workspace = true, features = ["std"] }
"#,
        )?;

        let graph = WorkspaceGraph::load_from_root(root.path())?;
        let member = graph
            .members()
            .find(|member| member.name() == "member")
            .ok_or("member missing")?;
        let dependencies = graph.resolve_member_dependencies(member)?;
        let domain = dependencies
            .library_dependencies()
            .get("domain")
            .ok_or("missing effective library dependency")?;
        assert_eq!(domain.origin(), WorkspaceDependencyOrigin::Workspace);
        assert!(domain.spec().path.ends_with("libraries/domain"));

        let serde = dependencies
            .rust_dependencies()
            .get("serde")
            .ok_or("missing effective serde dependency")?;
        assert_eq!(serde.origin(), WorkspaceDependencyOrigin::Workspace);
        assert_eq!(serde.workspace_features(), ["alloc"]);
        assert_eq!(serde.member_features(), ["derive"]);
        assert_eq!(serde.spec().features, vec!["alloc", "derive"]);
        assert!(!serde.spec().default_features);
        assert!(serde.spec().optional);

        let proptest = dependencies
            .rust_dev_dependencies()
            .get("proptest")
            .ok_or("missing effective proptest dependency")?;
        assert_eq!(proptest.member_features(), ["std"]);

        let effective_manifest = graph.effective_member_manifest(member)?;
        assert!(!effective_manifest.has_workspace_inherited_dependencies());
        assert_eq!(
            effective_manifest
                .rust_dependencies()
                .get("serde")
                .map(|dependency| dependency.features.as_slice()),
            Some(["alloc".to_string(), "derive".to_string()].as_slice())
        );
        Ok(())
    }

    /// Write an `incan.toml` at one directory, creating its parent first.
    fn write_manifest(directory: impl AsRef<Path>, content: &str) -> Result<(), std::io::Error> {
        let directory = directory.as_ref();
        fs::create_dir_all(directory)?;
        fs::write(directory.join(MANIFEST_FILENAME), content)
    }

    /// Write the smallest RFC 015 project manifest used by workspace topology tests.
    fn write_project(directory: impl AsRef<Path>, name: &str) -> Result<(), std::io::Error> {
        write_manifest(directory, format!("[project]\nname = \"{name}\"\n").as_str())
    }
}
