//! Project manifest (`incan.toml`) discovery and parsing.
//!
//! Implements the `incan.toml` schema from RFC 013 (Rust crate dependencies), RFC 015 (project discovery), and
//! RFC 031 Phase 1 (Incan library dependency table split). This module is responsible for locating the manifest and
//! parsing dependency tables into structured specs that the dependency resolver and future library resolver can
//! validate.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use semver::VersionReq;
use serde::{Deserialize, Serialize};
use toml_edit::{Array as EditArray, Document, DocumentMut, Item, Table, Value as EditValue};

/// The canonical manifest filename that the compiler searches for.
pub const MANIFEST_FILENAME: &str = "incan.toml";
/// Internal manifest-path override used for nested `incan` subprocesses launched via `incan env run`.
pub const INTERNAL_MANIFEST_OVERRIDE_ENV: &str = "INCAN_INTERNAL_MANIFEST_OVERRIDE";
/// Internal project-root override used for nested `incan` subprocesses launched via `incan env run`.
pub const INTERNAL_PROJECT_ROOT_OVERRIDE_ENV: &str = "INCAN_INTERNAL_PROJECT_ROOT";

// ============================================================================
// Error types
// ============================================================================

/// Errors that can occur when reading or parsing an `incan.toml` manifest.
#[derive(Debug, thiserror::Error)]
pub enum ManifestError {
    /// The file exists but could not be read.
    #[error("failed to read {path}: {source}")]
    Read { path: PathBuf, source: std::io::Error },

    /// The file was read but contains invalid TOML or an unexpected structure.
    #[error("failed to parse {path}{location}: {message}")]
    Parse {
        path: PathBuf,
        location: ManifestLocationDisplay,
        message: String,
    },

    /// The file was parsed but contains invalid configuration.
    #[error("invalid manifest {path}{location}: {message}")]
    Invalid {
        path: PathBuf,
        location: ManifestLocationDisplay,
        message: String,
    },
}

/// 1-based line/column location within a manifest file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ManifestLocation {
    line: usize,
    column: usize,
}

/// Optional display wrapper for manifest locations in diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ManifestLocationDisplay(Option<ManifestLocation>);

impl ManifestLocationDisplay {
    /// Construct a display wrapper with no concrete line/column location.
    fn none() -> Self {
        Self(None)
    }

    /// Construct a display wrapper for one concrete manifest location.
    fn some(location: ManifestLocation) -> Self {
        Self(Some(location))
    }
}

impl std::fmt::Display for ManifestLocationDisplay {
    /// Render the optional `:line:column` suffix used in manifest diagnostics.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(location) = self.0 {
            write!(f, ":{}:{}", location.line, location.column)
        } else {
            Ok(())
        }
    }
}

// ============================================================================
// Dependency specification types
// ============================================================================

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum DependencySource {
    Registry,
    Git { url: String, reference: GitReference },
    Path { path: PathBuf },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum GitReference {
    Branch(String),
    Tag(String),
    Rev(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DependencySpec {
    pub crate_name: String,
    pub version: Option<String>,
    pub features: Vec<String>,
    pub default_features: bool,
    pub source: DependencySource,
    pub optional: bool,
    pub package: Option<String>,
}

impl DependencySpec {
    /// Return a dependency spec with deterministically ordered and deduplicated features.
    pub fn normalized(mut self) -> Self {
        self.features.sort();
        self.features.dedup();
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LibraryDependencySpec {
    pub library_name: String,
    pub path: PathBuf,
}

/// A member-local opt-in to a workspace-owned Incan library dependency declaration.
///
/// RFC 077 deliberately permits no member-level refinements for Incan library dependencies: the workspace owns the
/// complete declaration and the member either opts in or declares an independent dependency.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceLibraryDependencyRef;

/// A member-local opt-in to a workspace-owned Rust dependency declaration.
///
/// The workspace owns source, version, package identity, and default-feature policy. A member may add features and
/// choose whether its use is optional; resolution of the effective dependency happens only after the workspace graph
/// has been validated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceRustDependencyRef {
    pub features: Vec<String>,
    pub optional: bool,
}

// ============================================================================
// Project manifest
// ============================================================================

/// `[project]` metadata from `incan.toml`.
///
/// RFC 015 makes this table the canonical project identity and lifecycle metadata source. Fields are optional in the
/// Rust representation because the compiler supports legacy manifests and commands validate only the fields they
/// require.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectSection {
    /// Project distribution name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Project version as a complete SemVer string.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// Short human-readable project summary.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Project authors, usually formatted as `Name <email>`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub authors: Option<Vec<String>>,
    /// Current project maintainers, usually formatted as `Name <email>`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub maintainers: Option<Vec<String>>,
    /// SPDX license identifier or expression.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
    /// License file paths relative to the project root.
    #[serde(rename = "license-files", skip_serializing_if = "Option::is_none")]
    pub license_files: Option<Vec<String>>,
    /// README path relative to the project root.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub readme: Option<String>,
    /// Project homepage URL.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub homepage: Option<String>,
    /// Source repository URL.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repository: Option<String>,
    /// Published documentation URL.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub documentation: Option<String>,
    /// Issue tracker URL.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub issues: Option<String>,
    /// Search/discovery keywords for future package tooling.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub keywords: Option<Vec<String>>,
    /// Trove-like classifiers for future package tooling.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub classifiers: Option<Vec<String>>,
    /// Required Incan toolchain version requirement.
    #[serde(rename = "requires-incan", skip_serializing_if = "Option::is_none")]
    pub requires_incan: Option<String>,
    /// Whether future publish tooling must refuse to publish this project.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub private: Option<bool>,
    /// Named Incan entry points such as `main = "src/main.incn"`.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub scripts: HashMap<String, String>,
    /// Named project feature groups.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub features: HashMap<String, Vec<String>>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuildSection {
    #[serde(rename = "rust-edition", skip_serializing_if = "Option::is_none")]
    pub rust_edition: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    /// Explicit source root directory (relative to project root).
    ///
    /// When set, the compiler and test runner resolve user module imports against this directory. If omitted, `src/`
    /// is used by convention when it exists; otherwise the project root itself is used.
    #[serde(rename = "source-root", skip_serializing_if = "Option::is_none")]
    pub source_root: Option<String>,
}

/// `[workspace]` coordination metadata from an Incan workspace root manifest.
///
/// RFC 077 deliberately keeps membership separate from dependency inheritance: this section declares the topology,
/// while member manifests opt into any shared declaration explicitly. The graph builder owns filesystem expansion and
/// cross-manifest validation because those checks cannot be performed from one TOML document alone.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceSection {
    /// Explicit root-relative paths or glob patterns for non-root members.
    #[serde(default)]
    pub members: Vec<String>,
    /// Members selected from the workspace root when no selector is provided.
    #[serde(rename = "default-members", default)]
    pub default_members: Vec<String>,
    /// Root-relative paths or globs removed after member expansion.
    #[serde(default)]
    pub exclude: Vec<String>,
    /// Shared Incan dependency declarations. Resolution is implemented after topology validation.
    #[serde(default)]
    pub dependencies: HashMap<String, toml::Value>,
    /// Shared Rust dependency declarations. Resolution is implemented after topology validation.
    #[serde(rename = "rust-dependencies", default)]
    pub rust_dependencies: HashMap<String, toml::Value>,
    /// Shared Rust development dependency declarations. Resolution is implemented after topology validation.
    #[serde(rename = "rust-dev-dependencies", default)]
    pub rust_dev_dependencies: HashMap<String, toml::Value>,
    /// Reserved workspace-owned environment declarations.
    #[serde(default)]
    pub envs: HashMap<String, toml::Value>,
    /// Reserved workspace policy configuration, retained for validation and inspection before policy evaluation lands.
    #[serde(default)]
    pub policy: Option<toml::Value>,
    /// Reserved workspace source/catalog configuration, retained for validation and inspection before resolution
    /// lands.
    #[serde(default)]
    pub sources: Option<toml::Value>,
    /// Reserved workspace capability configuration. Capability mutation remains member-local until RFC 077 is
    /// complete.
    #[serde(default)]
    pub capabilities: Option<toml::Value>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VocabSection {
    #[serde(rename = "crate")]
    pub crate_path: Option<String>,
}

/// Tool-owned configuration namespace from `[tool]`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ToolSection {
    /// Incan-specific tool configuration from `[tool.incan]`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub incan: Option<IncanToolSection>,
}

/// Incan-owned tool configuration from `[tool.incan]`.
///
/// Unknown future keys are intentionally tolerated at this layer so older compilers can still load manifests that
/// include newer tool sections. Concrete feature parsers, such as the env parser, remain free to reject typos inside
/// the subsection they own.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct IncanToolSection {
    /// Named RFC 015 environment definitions.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub envs: HashMap<String, EnvSection>,
    /// RFC 048 checked metadata configuration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<MetadataToolSection>,
}

/// RFC 048 metadata inputs owned by the Incan toolchain.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct MetadataToolSection {
    /// Manifest-relative JSON files containing canonical model bundles or RFC 048 metadata packages.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub model_bundles: Vec<String>,
}

/// Serializable shape for `[tool.incan.envs.<name>]`.
///
/// This canonical manifest-side type owns the RFC 015 env schema. Unknown keys inside an env definition are rejected
/// here through `deny_unknown_fields`, and downstream lifecycle helpers resolve overlays from this parsed shape instead
/// of maintaining a second env-schema model.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct EnvSection {
    /// Env names to apply before this env.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extends: Vec<String>,
    /// Required Incan toolchain version requirement for this env.
    #[serde(rename = "requires-incan", skip_serializing_if = "Option::is_none")]
    pub requires_incan: Option<String>,
    /// Skip implicit `default` inclusion when resolving this env.
    #[serde(default, skip_serializing_if = "is_false")]
    pub detached: bool,
    /// Script working directory, relative to the project root unless absolute.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// Environment variables injected into scripts.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub env_vars: HashMap<String, String>,
    /// Script commands represented as argv arrays.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub scripts: HashMap<String, Vec<String>>,
    /// Rust dependency overlays for this env.
    ///
    /// Current manifests spell this table `rust-dependencies`; the older RFC 015 text used `dependencies`.
    #[serde(
        rename = "rust-dependencies",
        alias = "dependencies",
        default,
        skip_serializing_if = "HashMap::is_empty"
    )]
    pub dependencies: HashMap<String, toml::Value>,
    /// Rust dev-dependency overlays for this env.
    ///
    /// Current manifests spell this table `rust-dev-dependencies`; the older RFC 015 text used `dev-dependencies`.
    #[serde(
        rename = "rust-dev-dependencies",
        alias = "dev-dependencies",
        default,
        skip_serializing_if = "HashMap::is_empty"
    )]
    pub dev_dependencies: HashMap<String, toml::Value>,
}

/// Helper for serde `skip_serializing_if` on boolean flags.
fn is_false(value: &bool) -> bool {
    !*value
}

/// A manifest that can be serialized to TOML.
///
/// Used by `incan init` and any future code that needs to write `incan.toml`.
/// The canonical field definitions live in [`ProjectSection`] and [`BuildSection`], keeping read and write in sync.
#[derive(Debug, Clone, Default, Serialize)]
pub struct WritableManifest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project: Option<ProjectSection>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub build: Option<BuildSection>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vocab: Option<VocabSection>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool: Option<ToolSection>,
}

impl WritableManifest {
    /// Serialize to TOML string.
    pub fn to_toml(&self) -> Result<String, toml::ser::Error> {
        toml::to_string(self)
    }
}

/// A parsed project manifest (`incan.toml`).
#[derive(Debug, Clone)]
pub struct ProjectManifest {
    /// Absolute (or as-discovered) path to the `incan.toml` file.
    path: PathBuf,
    /// `[project]` metadata (optional).
    pub project: Option<ProjectSection>,
    /// `[build]` configuration (optional).
    pub build: Option<BuildSection>,
    /// `[vocab]` configuration (optional).
    pub vocab: Option<VocabSection>,
    /// `[tool]` configuration (optional).
    pub tool: Option<ToolSection>,
    /// `[workspace]` configuration when this manifest is a workspace root.
    pub workspace: Option<WorkspaceSection>,
    /// `[dependencies]` (Incan library dependencies).
    library_dependencies: HashMap<String, LibraryDependencySpec>,
    /// Incan library dependency entries that explicitly inherit their full declaration from
    /// `[workspace.dependencies]`.
    workspace_library_dependencies: HashMap<String, WorkspaceLibraryDependencyRef>,
    /// `[rust-dependencies]` (Rust crate dependencies).
    rust_dependencies: HashMap<String, DependencySpec>,
    /// `[rust-dev-dependencies]` (dev-only Rust crates).
    rust_dev_dependencies: HashMap<String, DependencySpec>,
    /// Rust dependency entries that explicitly inherit their identity from `[workspace.rust-dependencies]`.
    workspace_rust_dependencies: HashMap<String, WorkspaceRustDependencyRef>,
    /// Dev-only Rust dependency entries that explicitly inherit their identity from
    /// `[workspace.rust-dev-dependencies]`.
    workspace_rust_dev_dependencies: HashMap<String, WorkspaceRustDependencyRef>,
}

impl ProjectManifest {
    /// Discover and parse an `incan.toml` manifest by walking upward from `start_dir`.
    ///
    /// Returns `Ok(None)` if no `incan.toml` is found (e.g., single-file mode).
    /// Returns `Err` if a manifest is found but cannot be read or parsed.
    pub fn discover(start_dir: &Path) -> Result<Option<Self>, ManifestError> {
        let manifest_path = match internal_project_root_override()
            .map(|root| root.join(MANIFEST_FILENAME))
            .or_else(|| find_manifest(start_dir))
        {
            Some(path) => path,
            None => return Ok(None),
        };

        let content_path = internal_manifest_override_path().unwrap_or_else(|| manifest_path.clone());
        let content = std::fs::read_to_string(&content_path).map_err(|e| ManifestError::Read {
            path: content_path.clone(),
            source: e,
        })?;

        let manifest = parse_manifest_content(&content, &manifest_path)?;
        Ok(Some(manifest))
    }

    /// Parse an `incan.toml` from raw string content.
    ///
    /// Useful for testing without touching the filesystem.
    pub fn from_str(content: &str, path: &Path) -> Result<Self, ManifestError> {
        parse_manifest_content(content, path)
    }

    /// The set of crate names declared in `[rust-dependencies]` (normal deps only).
    pub fn declared_rust_crate_names(&self) -> HashSet<String> {
        crate_name_alias_set(self.rust_dependencies.keys())
    }

    /// The set of crate names declared in `[rust-dev-dependencies]` only.
    pub fn declared_rust_dev_crate_names(&self) -> HashSet<String> {
        crate_name_alias_set(self.rust_dev_dependencies.keys())
    }

    /// Incan library dependencies from the manifest.
    pub fn library_dependencies(&self) -> &HashMap<String, LibraryDependencySpec> {
        &self.library_dependencies
    }

    /// Explicit member opt-ins to workspace-owned Incan library dependencies.
    pub fn workspace_library_dependencies(&self) -> &HashMap<String, WorkspaceLibraryDependencyRef> {
        &self.workspace_library_dependencies
    }

    /// Normal Rust dependencies from the manifest.
    pub fn rust_dependencies(&self) -> &HashMap<String, DependencySpec> {
        &self.rust_dependencies
    }

    /// Dev-only Rust dependencies from the manifest.
    pub fn rust_dev_dependencies(&self) -> &HashMap<String, DependencySpec> {
        &self.rust_dev_dependencies
    }

    /// Explicit member opt-ins to workspace-owned Rust dependencies.
    pub fn workspace_rust_dependencies(&self) -> &HashMap<String, WorkspaceRustDependencyRef> {
        &self.workspace_rust_dependencies
    }

    /// Explicit member opt-ins to workspace-owned Rust dev dependencies.
    pub fn workspace_rust_dev_dependencies(&self) -> &HashMap<String, WorkspaceRustDependencyRef> {
        &self.workspace_rust_dev_dependencies
    }

    /// Path to the `incan.toml` file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The project root directory (parent of `incan.toml`).
    pub fn project_root(&self) -> &Path {
        let parent = self.path.parent().unwrap_or_else(|| Path::new("."));
        if parent.as_os_str().is_empty() {
            Path::new(".")
        } else {
            parent
        }
    }

    /// Optional vocab configuration.
    pub fn vocab(&self) -> Option<&VocabSection> {
        self.vocab.as_ref()
    }

    /// Optional Incan-owned tool configuration.
    pub fn incan_tool(&self) -> Option<&IncanToolSection> {
        self.tool.as_ref().and_then(|tool| tool.incan.as_ref())
    }

    /// Workspace configuration when this manifest declares a workspace root.
    pub fn workspace(&self) -> Option<&WorkspaceSection> {
        self.workspace.as_ref()
    }

    /// Return a copy whose dependency tables have been resolved by a validated workspace graph.
    ///
    /// Dependency inheritance is intentionally resolved outside this parser because it needs the workspace root and
    /// every member manifest. Consumers that need one member's effective build or lock inputs must use this view,
    /// rather than treating `{ workspace = true }` as an empty local declaration.
    pub fn with_effective_workspace_dependencies(
        &self,
        library_dependencies: BTreeMap<String, LibraryDependencySpec>,
        rust_dependencies: BTreeMap<String, DependencySpec>,
        rust_dev_dependencies: BTreeMap<String, DependencySpec>,
    ) -> Self {
        let mut resolved = self.clone();
        resolved.library_dependencies = library_dependencies.into_iter().collect();
        resolved.rust_dependencies = rust_dependencies.into_iter().collect();
        resolved.rust_dev_dependencies = rust_dev_dependencies.into_iter().collect();
        resolved.workspace_library_dependencies.clear();
        resolved.workspace_rust_dependencies.clear();
        resolved.workspace_rust_dev_dependencies.clear();
        resolved
    }

    /// RFC 048 model bundle JSON paths declared under `[tool.incan.metadata]`.
    pub fn contract_model_bundle_paths(&self) -> Vec<String> {
        self.incan_tool()
            .and_then(|tool| tool.metadata.as_ref())
            .map(|metadata| metadata.model_bundles.clone())
            .unwrap_or_default()
    }

    /// Canonical env definitions as parsed from `[tool.incan.envs]`.
    pub fn env_sections(&self) -> BTreeMap<String, EnvSection> {
        self.incan_tool()
            .map(|tool| {
                tool.envs
                    .iter()
                    .map(|(name, env)| (name.clone(), env.clone()))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Canonical project-level Rust dependency overlay used as the base for env resolution.
    pub fn env_base_dependency_overlay(&self) -> BTreeMap<String, toml::Value> {
        self.rust_dependencies
            .iter()
            .map(|(name, spec)| (name.clone(), dependency_spec_to_toml_value(spec)))
            .collect()
    }

    /// Canonical project-level Rust dev-dependency overlay used as the base for env resolution.
    pub fn env_base_dev_dependency_overlay(&self) -> BTreeMap<String, toml::Value> {
        self.rust_dev_dependencies
            .iter()
            .map(|(name, spec)| (name.clone(), dependency_spec_to_toml_value(spec)))
            .collect()
    }
}

/// Parse `[workspace.rust-dependencies]` declarations into the same canonical dependency representation used by
/// project manifests. Paths resolve relative to the workspace root manifest.
pub fn parse_workspace_rust_dependency_declarations(
    workspace: &WorkspaceSection,
    manifest_path: &Path,
) -> Result<BTreeMap<String, DependencySpec>, ManifestError> {
    parse_workspace_rust_dependency_table(&workspace.rust_dependencies, "rust-dependencies", manifest_path)
}

/// Parse `[workspace.dependencies]` declarations into the canonical Incan library dependency representation.
///
/// Paths resolve relative to the workspace root manifest. Unlike Rust dependencies, library members cannot refine a
/// shared declaration; they opt in with exactly `{ workspace = true }`.
pub fn parse_workspace_library_dependency_declarations(
    workspace: &WorkspaceSection,
    manifest_path: &Path,
) -> Result<BTreeMap<String, LibraryDependencySpec>, ManifestError> {
    let mut dependencies = BTreeMap::new();
    for (name, value) in &workspace.dependencies {
        let entry = DependencyEntry::deserialize(value.clone()).map_err(|error| {
            manifest_invalid(
                manifest_path,
                None,
                format!("invalid [workspace.dependencies] entry `{name}`: {error}"),
            )
        })?;
        if matches!(&entry, DependencyEntry::Table(table) if table.workspace.is_some()) {
            return Err(manifest_invalid(
                manifest_path,
                None,
                format!("workspace library dependency `{name}` must declare its own path and cannot use `workspace`"),
            ));
        }
        dependencies.insert(
            name.clone(),
            library_dependency_from_entry(name, &entry, manifest_path, None)?,
        );
    }
    Ok(dependencies)
}

/// Parse `[workspace.rust-dev-dependencies]` declarations into the canonical dependency representation.
///
/// Dev dependencies obey the same inheritance constraints as normal Rust dependencies. They remain a distinct map
/// so workspace inspection and later shared-lock resolution preserve Cargo's normal-versus-dev boundary.
pub fn parse_workspace_rust_dev_dependency_declarations(
    workspace: &WorkspaceSection,
    manifest_path: &Path,
) -> Result<BTreeMap<String, DependencySpec>, ManifestError> {
    parse_workspace_rust_dependency_table(&workspace.rust_dev_dependencies, "rust-dev-dependencies", manifest_path)
}

/// Parse one workspace Rust dependency table and reject member-style inheritance or workspace-level optionality.
fn parse_workspace_rust_dependency_table(
    entries: &HashMap<String, toml::Value>,
    section: &str,
    manifest_path: &Path,
) -> Result<BTreeMap<String, DependencySpec>, ManifestError> {
    let mut dependencies = BTreeMap::new();
    for (name, value) in entries {
        let entry = DependencyEntry::deserialize(value.clone()).map_err(|error| {
            manifest_invalid(
                manifest_path,
                None,
                format!("invalid [workspace.{section}] entry `{name}`: {error}"),
            )
        })?;
        if matches!(&entry, DependencyEntry::Table(table) if table.workspace.is_some()) {
            return Err(manifest_invalid(
                manifest_path,
                None,
                format!(
                    "workspace {section} dependency `{name}` must declare its own identity and cannot use `workspace`"
                ),
            ));
        }
        let mut spec = dependency_from_entry(name, &entry, false, manifest_path, None)?;
        if spec.optional {
            return Err(manifest_invalid(
                manifest_path,
                None,
                format!(
                    "workspace {section} dependency `{name}` must not set `optional`; optionality belongs to each member"
                ),
            ));
        }
        spec.features.sort();
        spec.features.dedup();
        dependencies.insert(name.clone(), spec);
    }
    Ok(dependencies)
}

/// Walk upward from `start_dir` to find an `incan.toml` file.
pub fn find_manifest(start_dir: &Path) -> Option<PathBuf> {
    let mut current = start_dir.to_path_buf();
    loop {
        let candidate = current.join(MANIFEST_FILENAME);
        if candidate.is_file() {
            return Some(candidate);
        }
        if !current.pop() {
            return None;
        }
    }
}

/// Return the active internal manifest override path, if nested execution provided one.
fn internal_manifest_override_path() -> Option<PathBuf> {
    std::env::var_os(INTERNAL_MANIFEST_OVERRIDE_ENV).map(PathBuf::from)
}

/// Return the active internal project-root override, if nested execution provided one.
fn internal_project_root_override() -> Option<PathBuf> {
    std::env::var_os(INTERNAL_PROJECT_ROOT_OVERRIDE_ENV).map(PathBuf::from)
}

/// Render a temporary manifest view whose top-level Rust dependency tables match the provided resolved overlay.
pub fn render_dependency_overlay_manifest(
    content: &str,
    manifest_path: &Path,
    dependencies: &BTreeMap<String, toml::Value>,
    dev_dependencies: &BTreeMap<String, toml::Value>,
) -> Result<String, ManifestError> {
    let mut document = content
        .parse::<DocumentMut>()
        .map_err(|error| manifest_parse_error(manifest_path, content, error))?;
    remove_dependency_alias_tables(&mut document);
    replace_dependency_table(&mut document, "rust-dependencies", dependencies, manifest_path)?;
    replace_dependency_table(&mut document, "rust-dev-dependencies", dev_dependencies, manifest_path)?;
    Ok(document.to_string())
}

/// Build a lookup set for **Rust crate names** as written in `incan.toml` and common spellings used in source.
///
/// Cargo package names often use **hyphens** (`serde-json`), while `use` paths and `rust::` imports typically use
/// **underscores** (`serde_json`). For each manifest key we insert the key as-is plus both single-character-style
/// substitutions so [`ProjectManifest::declared_rust_crate_names`] and dev-deps peers accept either form when
/// validating the first segment of a `rust::…` path.
fn crate_name_alias_set<'a>(names: impl Iterator<Item = &'a String>) -> HashSet<String> {
    let mut out = HashSet::new();
    for name in names {
        out.insert(name.clone());
        out.insert(name.replace('-', "_"));
        out.insert(name.replace('_', "-"));
    }
    out
}

// ============================================================================
// Internal helpers
// ============================================================================

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawManifest {
    #[serde(default)]
    project: Option<ProjectSection>,
    #[serde(default)]
    build: Option<BuildSection>,
    #[serde(default)]
    vocab: Option<VocabSection>,
    #[serde(default)]
    tool: Option<ToolSection>,
    #[serde(default)]
    workspace: Option<WorkspaceSection>,
    #[serde(default)]
    dependencies: Option<DependencyTable>,
    #[serde(rename = "rust-dependencies", default)]
    rust_dependencies: Option<DependencyTable>,
    #[serde(rename = "rust-dev-dependencies", default)]
    rust_dev_dependencies: Option<DependencyTable>,
    #[serde(rename = "dev-dependencies", default)]
    legacy_dev_dependencies: Option<DependencyTable>,
    #[serde(default)]
    rust: Option<RustTables>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RustTables {
    #[serde(default)]
    dependencies: Option<DependencyTable>,
    #[serde(rename = "dev-dependencies", default)]
    dev_dependencies: Option<DependencyTable>,
}

#[derive(Debug, Default, Clone, Deserialize)]
struct DependencyTable {
    #[serde(default)]
    optional: HashMap<String, DependencyEntry>,
    #[serde(flatten)]
    entries: HashMap<String, DependencyEntry>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum DependencyEntry {
    Version(String),
    Table(DependencyEntryTable),
}

#[derive(Debug, Default, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct DependencyEntryTable {
    version: Option<String>,
    features: Option<Vec<String>>,
    git: Option<String>,
    branch: Option<String>,
    tag: Option<String>,
    rev: Option<String>,
    path: Option<String>,
    optional: Option<bool>,
    package: Option<String>,
    #[serde(rename = "default-features")]
    default_features: Option<bool>,
    workspace: Option<bool>,
}

struct ManifestSpans<'a> {
    content: &'a str,
    document: &'a Document<String>,
}

trait TomlSpanError {
    /// Return the parse/deserialize error message.
    fn message(&self) -> &str;
    /// Return the byte-span location associated with the error, if available.
    fn span(&self) -> Option<std::ops::Range<usize>>;
}

impl TomlSpanError for toml_edit::TomlError {
    /// Return the parse error message.
    fn message(&self) -> &str {
        self.message()
    }

    /// Return the parse error span.
    fn span(&self) -> Option<std::ops::Range<usize>> {
        self.span()
    }
}

impl TomlSpanError for toml_edit::de::Error {
    /// Return the deserialize error message.
    fn message(&self) -> &str {
        self.message()
    }

    /// Return the deserialize error span.
    fn span(&self) -> Option<std::ops::Range<usize>> {
        self.span()
    }
}

impl<'a> ManifestSpans<'a> {
    /// Create span lookup helpers for one parsed manifest document.
    fn new(content: &'a str, document: &'a Document<String>) -> Self {
        Self { content, document }
    }

    /// Locate one table header in the source manifest.
    fn table_location(&self, table_path: &[&str]) -> Option<ManifestLocation> {
        self.item_at_path(table_path)
            .and_then(|item| item.span())
            .and_then(|span| manifest_location_from_span(self.content, span))
    }

    /// Locate one table entry in the source manifest.
    fn entry_location(&self, table_path: &[&str], entry: &str) -> Option<ManifestLocation> {
        self.item_at_path(table_path)
            .and_then(|item| item.get(entry))
            .and_then(|item| item.span())
            .and_then(|span| manifest_location_from_span(self.content, span))
    }

    /// Resolve one nested TOML item by path.
    fn item_at_path(&self, path: &[&str]) -> Option<&Item> {
        let mut item = self.document.as_item();
        for segment in path {
            item = item.get(segment)?;
        }
        Some(item)
    }

    /// Locate one already-resolved TOML item in the source manifest.
    fn item_location(&self, item: &Item) -> Option<ManifestLocation> {
        item.span()
            .and_then(|span| manifest_location_from_span(self.content, span))
    }
}

/// Convert a TOML parser/deserializer error into the manifest error type with source location.
fn manifest_parse_error<E: TomlSpanError>(path: &Path, content: &str, error: E) -> ManifestError {
    ManifestError::Parse {
        path: path.to_path_buf(),
        location: ManifestLocationDisplay::from_span(content, error.span()),
        message: error.message().to_string(),
    }
}

/// Construct one semantic manifest error with an optional source location.
fn manifest_invalid(path: &Path, location: Option<ManifestLocation>, message: impl Into<String>) -> ManifestError {
    ManifestError::Invalid {
        path: path.to_path_buf(),
        location: match location {
            Some(location) => ManifestLocationDisplay::some(location),
            None => ManifestLocationDisplay::none(),
        },
        message: message.into(),
    }
}

impl ManifestLocationDisplay {
    /// Convert an optional parser byte span into a display-ready location wrapper.
    fn from_span(content: &str, span: Option<std::ops::Range<usize>>) -> Self {
        span.and_then(|span| manifest_location_from_span(content, span))
            .map_or_else(Self::none, Self::some)
    }
}

/// Convert one byte span in TOML source into a 1-based line/column location.
fn manifest_location_from_span(content: &str, span: std::ops::Range<usize>) -> Option<ManifestLocation> {
    let (line, column) = byte_offset_to_line_col(content, span.start);
    Some(ManifestLocation {
        line: line + 1,
        column: column + 1,
    })
}

/// Convert one byte offset into zero-based line and column indices.
fn byte_offset_to_line_col(content: &str, index: usize) -> (usize, usize) {
    if content.is_empty() {
        return (0, index);
    }

    let bytes = content.as_bytes();
    let safe_index = index.min(bytes.len().saturating_sub(1));
    let column_offset = index.saturating_sub(safe_index);

    let nl = bytes[0..safe_index]
        .iter()
        .rev()
        .enumerate()
        .find(|(_, byte)| **byte == b'\n')
        .map(|(offset, _)| safe_index - offset - 1);
    let line_start = match nl {
        Some(line_start) => line_start + 1,
        None => 0,
    };
    let line = bytes[0..line_start].iter().filter(|byte| **byte == b'\n').count();

    let column = core::str::from_utf8(&bytes[line_start..=safe_index])
        .map(|s| s.chars().count().saturating_sub(1))
        .unwrap_or_else(|_| safe_index - line_start);
    (line, column + column_offset)
}

/// Parse and validate one complete `incan.toml` document.
fn parse_manifest_content(content: &str, path: &Path) -> Result<ProjectManifest, ManifestError> {
    let document: Document<String> = content
        .parse()
        .map_err(|error| manifest_parse_error(path, content, error))?;
    let spans = ManifestSpans::new(content, &document);
    validate_dependency_entry_shapes(&spans, path)?;
    let raw: RawManifest =
        toml_edit::de::from_document(document.clone()).map_err(|error| manifest_parse_error(path, content, error))?;

    let library_dependencies = raw
        .dependencies
        .as_ref()
        .map(|table| parse_library_dependency_table(table, &spans, path))
        .transpose()?
        .unwrap_or_default();

    let (rust_deps_table, rust_dev_deps_table) = resolve_rust_dependency_tables(&raw, &spans, path)?;
    let rust_dependencies = rust_deps_table
        .map(|table| parse_dependency_table(&table, &spans, path, "[rust-dependencies]", &["rust-dependencies"]))
        .transpose()?
        .unwrap_or_default();
    let rust_dev_dependencies = rust_dev_deps_table
        .map(|table| {
            parse_dependency_table(
                &table,
                &spans,
                path,
                "[rust-dev-dependencies]",
                &["rust-dev-dependencies"],
            )
        })
        .transpose()?
        .unwrap_or_default();

    validate_package_collisions(&rust_dependencies, &rust_dev_dependencies, path)?;
    validate_requires_incan_constraints(&raw, &spans, path)?;

    if let Some(vocab) = &raw.vocab {
        if let Some(crate_path) = &vocab.crate_path {
            if crate_path.trim().is_empty() {
                return Err(manifest_invalid(
                    path,
                    spans.entry_location(&["vocab"], "crate"),
                    "[vocab].crate cannot be empty",
                ));
            }
        } else {
            return Err(manifest_invalid(
                path,
                spans.table_location(&["vocab"]),
                "[vocab] section requires a `crate` field",
            ));
        }
    }

    let workspace_library_dependencies = library_dependencies.workspace_refs;
    let workspace_rust_dependencies = rust_dependencies.workspace_refs;
    let workspace_rust_dev_dependencies = rust_dev_dependencies.workspace_refs;

    Ok(ProjectManifest {
        path: path.to_path_buf(),
        project: raw.project,
        build: raw.build,
        vocab: raw.vocab,
        tool: raw.tool,
        workspace: raw.workspace,
        library_dependencies: library_dependencies.specs,
        workspace_library_dependencies,
        rust_dependencies: rust_dependencies.specs,
        rust_dev_dependencies: rust_dev_dependencies.specs,
        workspace_rust_dependencies,
        workspace_rust_dev_dependencies,
    })
}

/// Validate every `requires-incan` string in the manifest before command-specific policy checks run.
fn validate_requires_incan_constraints(
    raw: &RawManifest,
    spans: &ManifestSpans<'_>,
    path: &Path,
) -> Result<(), ManifestError> {
    if let Some(requirement) = raw
        .project
        .as_ref()
        .and_then(|project| project.requires_incan.as_deref())
    {
        validate_requires_incan_requirement(requirement, path, spans.entry_location(&["project"], "requires-incan"))?;
    }

    if let Some(envs) = raw
        .tool
        .as_ref()
        .and_then(|tool| tool.incan.as_ref())
        .map(|incan| &incan.envs)
    {
        for (env_name, env) in envs {
            if let Some(requirement) = env.requires_incan.as_deref() {
                validate_requires_incan_requirement(
                    requirement,
                    path,
                    spans.entry_location(&["tool", "incan", "envs", env_name.as_str()], "requires-incan"),
                )?;
            }
        }
    }

    Ok(())
}

/// Validate one SemVer requirement string from a `requires-incan` field.
fn validate_requires_incan_requirement(
    requirement: &str,
    path: &Path,
    location: Option<ManifestLocation>,
) -> Result<(), ManifestError> {
    VersionReq::parse(requirement).map_err(|error| {
        manifest_invalid(
            path,
            location,
            format!("invalid requires-incan constraint `{requirement}`: {error}"),
        )
    })?;
    Ok(())
}

/// Serialize one resolved Rust dependency spec back into the canonical TOML table shape.
///
/// This is used when lifecycle tooling materializes a temporary manifest view after env overlays have been resolved.
/// The output intentionally uses the current canonical field names rather than preserving legacy alias spellings.
fn dependency_spec_to_toml_value(spec: &DependencySpec) -> toml::Value {
    let mut table = toml::map::Map::new();

    if let Some(version) = &spec.version {
        table.insert("version".to_string(), toml::Value::String(version.clone()));
    }
    if !spec.features.is_empty() {
        table.insert(
            "features".to_string(),
            toml::Value::Array(spec.features.iter().cloned().map(toml::Value::String).collect()),
        );
    }
    if !spec.default_features {
        table.insert("default-features".to_string(), toml::Value::Boolean(false));
    }
    if spec.optional {
        table.insert("optional".to_string(), toml::Value::Boolean(true));
    }
    if let Some(package) = &spec.package {
        table.insert("package".to_string(), toml::Value::String(package.clone()));
    }

    match &spec.source {
        DependencySource::Registry => {}
        DependencySource::Git { url, reference } => {
            table.insert("git".to_string(), toml::Value::String(url.clone()));
            match reference {
                GitReference::Branch(branch) => {
                    table.insert("branch".to_string(), toml::Value::String(branch.clone()));
                }
                GitReference::Tag(tag) => {
                    table.insert("tag".to_string(), toml::Value::String(tag.clone()));
                }
                GitReference::Rev(rev) => {
                    table.insert("rev".to_string(), toml::Value::String(rev.clone()));
                }
            }
        }
        DependencySource::Path { path } => {
            table.insert(
                "path".to_string(),
                toml::Value::String(path.to_string_lossy().into_owned()),
            );
        }
    }

    toml::Value::Table(table)
}

/// Remove legacy dependency-table aliases before writing canonical dependency tables back into the manifest view.
///
/// RFC 013 and RFC 031 standardized top-level `rust-dependencies` / `rust-dev-dependencies`. Older manifests may still
/// carry top-level `dev-dependencies` or nested `[rust.dependencies]` / `[rust.dev-dependencies]` aliases. Once a
/// temporary manifest view is rewritten, those aliases should disappear so one logical dependency set is not expressed
/// twice. If the nested `[rust]` table becomes empty after alias removal, it is dropped entirely.
fn remove_dependency_alias_tables(document: &mut DocumentMut) {
    document.remove("dev-dependencies");
    if let Some(rust) = document.get_mut("rust").and_then(Item::as_table_like_mut) {
        rust.remove("dependencies");
        rust.remove("dev-dependencies");
        if rust.is_empty() {
            document.remove("rust");
        }
    }
}

/// Replace one canonical dependency table in the editable manifest view.
///
/// An empty map removes the table entirely. Non-empty values are converted through the same TOML-edit bridge used for
/// env overlays so lifecycle-generated manifests keep one canonical shape for dependency entries.
fn replace_dependency_table(
    document: &mut DocumentMut,
    table_name: &str,
    values: &BTreeMap<String, toml::Value>,
    manifest_path: &Path,
) -> Result<(), ManifestError> {
    if values.is_empty() {
        document.remove(table_name);
        return Ok(());
    }

    let mut table = Table::new();
    for (name, value) in values {
        table.insert(
            name,
            toml_value_to_edit_item(value, manifest_path)
                .map_err(|message| manifest_invalid(manifest_path, None, message))?,
        );
    }
    document.insert(table_name, Item::Table(table));
    Ok(())
}

/// Convert a serde-style TOML value into a `toml_edit` item for manifest rewriting.
///
/// Tables recurse into nested editable tables, while scalar and array values are delegated to
/// [`toml_value_to_edit_value`]. The conversion stays intentionally small because dependency overlays do not support
/// arbitrary TOML constructs.
fn toml_value_to_edit_item(value: &toml::Value, manifest_path: &Path) -> Result<Item, String> {
    match value {
        toml::Value::Table(table) => {
            let mut edit_table = Table::new();
            for (key, value) in table {
                edit_table.insert(key, toml_value_to_edit_item(value, manifest_path)?);
            }
            Ok(Item::Table(edit_table))
        }
        _ => toml_value_to_edit_value(value, manifest_path).map(Item::Value),
    }
}

/// Convert a serde-style TOML scalar or array into a `toml_edit` value for dependency overlay rewrites.
///
/// Nested table values are rejected here because dependency overlay entries only support the canonical dependency-spec
/// shape; they are not a general-purpose TOML merge surface.
fn toml_value_to_edit_value(value: &toml::Value, manifest_path: &Path) -> Result<EditValue, String> {
    match value {
        toml::Value::String(value) => Ok(EditValue::from(value.clone())),
        toml::Value::Integer(value) => Ok(EditValue::from(*value)),
        toml::Value::Float(value) => Ok(EditValue::from(*value)),
        toml::Value::Boolean(value) => Ok(EditValue::from(*value)),
        toml::Value::Datetime(value) => value.to_string().parse::<EditValue>().map_err(|error| {
            format!(
                "invalid datetime in dependency overlay for '{}': {error}",
                manifest_path.display()
            )
        }),
        toml::Value::Array(values) => {
            let mut array = EditArray::new();
            for value in values {
                array.push(toml_value_to_edit_value(value, manifest_path)?);
            }
            Ok(EditValue::Array(array))
        }
        toml::Value::Table(_) => Err(format!(
            "nested inline table values are not supported in dependency overlay for '{}'",
            manifest_path.display()
        )),
    }
}

const DEPENDENCY_ENTRY_KEYS: &[&str] = &[
    "version",
    "features",
    "git",
    "branch",
    "tag",
    "rev",
    "path",
    "optional",
    "package",
    "default-features",
    "workspace",
];

/// Validate the raw TOML shapes of all dependency tables before serde decoding proceeds.
fn validate_dependency_entry_shapes(spans: &ManifestSpans<'_>, path: &Path) -> Result<(), ManifestError> {
    for table_path in [
        &["dependencies"][..],
        &["rust-dependencies"][..],
        &["rust-dev-dependencies"][..],
        &["rust", "dependencies"][..],
        &["rust", "dev-dependencies"][..],
    ] {
        validate_dependency_table_items(spans, path, table_path)?;
    }
    Ok(())
}

/// Validate the item shapes inside one dependency table.
fn validate_dependency_table_items(
    spans: &ManifestSpans<'_>,
    path: &Path,
    table_path: &[&str],
) -> Result<(), ManifestError> {
    let Some(table_item) = spans.item_at_path(table_path) else {
        return Ok(());
    };
    let Some(table) = table_item.as_table_like() else {
        return Ok(());
    };

    for (entry_name, entry_item) in table.iter() {
        if entry_name == "optional" {
            if let Some(optional_table) = entry_item.as_table_like() {
                for (optional_name, optional_item) in optional_table.iter() {
                    validate_dependency_entry_item(spans, path, table_path, optional_name, optional_item)?;
                }
            }
            continue;
        }
        validate_dependency_entry_item(spans, path, table_path, entry_name, entry_item)?;
    }

    Ok(())
}

/// Validate one dependency entry item against the supported dependency-entry schema.
fn validate_dependency_entry_item(
    spans: &ManifestSpans<'_>,
    path: &Path,
    table_path: &[&str],
    entry_name: &str,
    entry_item: &Item,
) -> Result<(), ManifestError> {
    if entry_item.is_str() {
        return Ok(());
    }

    let Some(entry_table) = entry_item.as_table_like() else {
        return Err(manifest_invalid(
            path,
            spans
                .item_location(entry_item)
                .or_else(|| spans.entry_location(table_path, entry_name)),
            format!("dependency `{entry_name}` must be a version string or a table with known dependency keys"),
        ));
    };

    for (key, value) in entry_table.iter() {
        if !DEPENDENCY_ENTRY_KEYS.contains(&key) {
            return Err(manifest_invalid(
                path,
                spans.item_location(value).or_else(|| spans.item_location(entry_item)),
                format!(
                    "unknown field `{key}` in dependency `{entry_name}`; expected one of {}",
                    DEPENDENCY_ENTRY_KEYS.join(", ")
                ),
            ));
        }

        let location = spans.item_location(value).or_else(|| spans.item_location(entry_item));
        match key {
            "version" | "git" | "branch" | "tag" | "rev" | "path" | "package" if !value.is_str() => {
                return Err(manifest_invalid(
                    path,
                    location,
                    format!("dependency `{entry_name}` field `{key}` must be a string"),
                ));
            }
            "optional" | "default-features" | "workspace" if !value.is_bool() => {
                return Err(manifest_invalid(
                    path,
                    location,
                    format!("dependency `{entry_name}` field `{key}` must be a boolean"),
                ));
            }
            "features" => {
                let Some(array) = value.as_array() else {
                    return Err(manifest_invalid(
                        path,
                        location,
                        format!("dependency `{entry_name}` field `features` must be an array of strings"),
                    ));
                };
                if array.iter().any(|item| !item.is_str()) {
                    return Err(manifest_invalid(
                        path,
                        location,
                        format!("dependency `{entry_name}` field `features` must be an array of strings"),
                    ));
                }
            }
            _ => {}
        }
    }

    Ok(())
}

/// Resolve canonical and legacy Rust dependency table spellings into one pair of parsed tables.
fn resolve_rust_dependency_tables(
    raw: &RawManifest,
    spans: &ManifestSpans<'_>,
    path: &Path,
) -> Result<(Option<DependencyTable>, Option<DependencyTable>), ManifestError> {
    let rust_tables = raw.rust.as_ref();
    let rust_deps = raw.rust_dependencies.clone();
    let legacy_rust_deps = rust_tables.and_then(|r| r.dependencies.clone());
    let explicit_rust_dev_deps = raw.rust_dev_dependencies.clone();
    let legacy_dev_deps = raw.legacy_dev_dependencies.clone();
    let legacy_rust_dev_deps = rust_tables.and_then(|r| r.dev_dependencies.clone());

    if rust_deps.is_some() && legacy_rust_deps.is_some() {
        return Err(manifest_invalid(
            path,
            spans
                .table_location(&["rust", "dependencies"])
                .or_else(|| spans.table_location(&["rust-dependencies"])),
            "cannot specify both [rust-dependencies] and [rust.dependencies]",
        ));
    }

    if legacy_dev_deps.is_some() {
        return Err(manifest_invalid(
            path,
            spans.table_location(&["dev-dependencies"]),
            "table [dev-dependencies] has been renamed to [rust-dev-dependencies]",
        ));
    }

    if explicit_rust_dev_deps.is_some() && legacy_rust_dev_deps.is_some() {
        return Err(manifest_invalid(
            path,
            spans
                .table_location(&["rust", "dev-dependencies"])
                .or_else(|| spans.table_location(&["rust-dev-dependencies"])),
            "cannot specify both [rust-dev-dependencies] and [rust.dev-dependencies]",
        ));
    }

    Ok((
        rust_deps.or(legacy_rust_deps),
        explicit_rust_dev_deps.or(legacy_rust_dev_deps),
    ))
}

/// Parse the Incan library dependency table from `[dependencies]`.
fn parse_library_dependency_table(
    table: &DependencyTable,
    spans: &ManifestSpans<'_>,
    path: &Path,
) -> Result<ParsedLibraryDependencyTable, ManifestError> {
    if !table.optional.is_empty() {
        return Err(manifest_invalid(
            path,
            spans.table_location(&["dependencies", "optional"]),
            "table [dependencies.optional] is no longer valid; move Rust optional crates to [rust-dependencies]",
        ));
    }

    let mut result = ParsedLibraryDependencyTable::default();
    for (name, entry) in &table.entries {
        let location = spans.entry_location(&["dependencies"], name);
        if workspace_library_dependency_ref_from_entry(name, entry, path, location)? {
            result
                .workspace_refs
                .insert(name.clone(), WorkspaceLibraryDependencyRef);
            continue;
        }
        let spec = library_dependency_from_entry(name, entry, path, location)?;
        result.specs.insert(name.clone(), spec);
    }
    Ok(result)
}

/// Parse a `{ workspace = true }` Incan library opt-in without assigning any member-local path or identity.
fn workspace_library_dependency_ref_from_entry(
    name: &str,
    entry: &DependencyEntry,
    path: &Path,
    location: Option<ManifestLocation>,
) -> Result<bool, ManifestError> {
    let DependencyEntry::Table(table) = entry else {
        return Ok(false);
    };
    let Some(workspace) = table.workspace else {
        return Ok(false);
    };
    if !workspace {
        return Err(manifest_invalid(
            path,
            location,
            format!("library dependency `{name}` sets `workspace = false`; omit the key for a member-local dependency"),
        ));
    }
    if table.version.is_some()
        || table.features.is_some()
        || table.git.is_some()
        || table.branch.is_some()
        || table.tag.is_some()
        || table.rev.is_some()
        || table.path.is_some()
        || table.optional.is_some()
        || table.package.is_some()
        || table.default_features.is_some()
    {
        return Err(manifest_invalid(
            path,
            location,
            format!(
                "workspace library dependency `{name}` may only set `workspace = true`; its complete declaration belongs to [workspace.dependencies]"
            ),
        ));
    }
    Ok(true)
}

/// Parse one library dependency entry from `[dependencies]`.
fn library_dependency_from_entry(
    name: &str,
    entry: &DependencyEntry,
    path: &Path,
    location: Option<ManifestLocation>,
) -> Result<LibraryDependencySpec, ManifestError> {
    let table = match entry {
        DependencyEntry::Version(_) => {
            return Err(manifest_invalid(
                path,
                location,
                format!(
                    "dependency `{name}` in [dependencies] uses legacy Rust crate syntax. Move Rust crates to [rust-dependencies]."
                ),
            ));
        }
        DependencyEntry::Table(table) => table,
    };

    if looks_like_legacy_rust_dependency(entry) {
        return Err(manifest_invalid(
            path,
            location,
            format!(
                "dependency `{name}` in [dependencies] looks like a Rust crate dependency. Move it to [rust-dependencies]."
            ),
        ));
    }

    if table.path.is_none() {
        return Err(manifest_invalid(
            path,
            location,
            format!("library dependency `{name}` is missing `path`. Use `{name} = {{ path = \"../{name}\" }}`."),
        ));
    }

    let raw_path = table.path.clone().unwrap_or_default();
    if raw_path.trim().is_empty() {
        return Err(manifest_invalid(
            path,
            location,
            format!("library dependency `{name}` has an empty `path`"),
        ));
    }
    let manifest_dir = path.parent().unwrap_or_else(|| Path::new("."));
    let raw_path_buf = PathBuf::from(raw_path);
    let resolved_path = if raw_path_buf.is_relative() {
        manifest_dir.join(raw_path_buf)
    } else {
        raw_path_buf
    };

    Ok(LibraryDependencySpec {
        library_name: name.to_string(),
        path: resolved_path,
    })
}

/// Return whether an entry in `[dependencies]` looks like legacy Rust dependency syntax.
fn looks_like_legacy_rust_dependency(entry: &DependencyEntry) -> bool {
    match entry {
        DependencyEntry::Version(_) => true,
        DependencyEntry::Table(table) => {
            table.version.is_some()
                || table.features.is_some()
                || table.git.is_some()
                || table.branch.is_some()
                || table.tag.is_some()
                || table.rev.is_some()
                || table.optional.is_some()
                || table.package.is_some()
                || table.default_features.is_some()
        }
    }
}

/// Parse one Rust dependency table, including the optional-subtable overlay form.
fn parse_dependency_table(
    table: &DependencyTable,
    spans: &ManifestSpans<'_>,
    path: &Path,
    table_name: &str,
    table_path: &[&str],
) -> Result<ParsedDependencyTable, ManifestError> {
    let mut result = ParsedDependencyTable::default();

    for (name, entry) in &table.entries {
        if table.optional.contains_key(name) {
            return Err(manifest_invalid(
                path,
                spans.entry_location(table_path, name),
                format!("dependency `{name}` appears in both {table_name} and {table_name}.optional"),
            ));
        }
        let location = spans.entry_location(table_path, name);
        if let Some(workspace_ref) = workspace_dependency_ref_from_entry(name, entry, false, path, location)? {
            result.workspace_refs.insert(name.clone(), workspace_ref);
            continue;
        }
        let spec = dependency_from_entry(name, entry, false, path, location)?;
        result.specs.insert(name.clone(), spec);
        if let Some(location) = location {
            result.locations.insert(name.clone(), location);
        }
    }

    for (name, entry) in &table.optional {
        let location = spans.entry_location(table_path, name);
        if let Some(workspace_ref) = workspace_dependency_ref_from_entry(name, entry, true, path, location)? {
            result.workspace_refs.insert(name.clone(), workspace_ref);
            continue;
        }
        let spec = dependency_from_entry(name, entry, true, path, location)?;
        result.specs.insert(name.clone(), spec);
        if let Some(location) = location {
            result.locations.insert(name.clone(), location);
        }
    }

    Ok(result)
}

/// Parse a `{ workspace = true }` dependency opt-in without assigning a member-local source identity.
fn workspace_dependency_ref_from_entry(
    name: &str,
    entry: &DependencyEntry,
    optional_override: bool,
    path: &Path,
    location: Option<ManifestLocation>,
) -> Result<Option<WorkspaceRustDependencyRef>, ManifestError> {
    let DependencyEntry::Table(table) = entry else {
        return Ok(None);
    };
    let Some(workspace) = table.workspace else {
        return Ok(None);
    };
    if !workspace {
        return Err(manifest_invalid(
            path,
            location,
            format!("dependency `{name}` sets `workspace = false`; omit the key for a member-local dependency"),
        ));
    }
    if table.version.is_some()
        || table.git.is_some()
        || table.branch.is_some()
        || table.tag.is_some()
        || table.rev.is_some()
        || table.path.is_some()
        || table.package.is_some()
        || table.default_features.is_some()
    {
        return Err(manifest_invalid(
            path,
            location,
            format!(
                "workspace dependency `{name}` may refine only `features` and `optional`; source, version, package, and default-feature policy belong to the workspace"
            ),
        ));
    }
    let mut features = table.features.clone().unwrap_or_default();
    features.sort();
    features.dedup();
    Ok(Some(WorkspaceRustDependencyRef {
        features,
        optional: optional_override || table.optional.unwrap_or(false),
    }))
}

/// Parse one Rust dependency entry into the canonical dependency model.
fn dependency_from_entry(
    name: &str,
    entry: &DependencyEntry,
    optional_override: bool,
    path: &Path,
    location: Option<ManifestLocation>,
) -> Result<DependencySpec, ManifestError> {
    let (version, features, default_features, source, optional, package) = match entry {
        DependencyEntry::Version(version) => (
            Some(version.clone()),
            Vec::new(),
            true,
            DependencySource::Registry,
            optional_override,
            None,
        ),
        DependencyEntry::Table(table) => {
            let (source, version) = parse_dependency_source(table, path, location)?;
            let mut optional = table.optional.unwrap_or(false);
            if optional_override {
                optional = true;
            }
            let default_features = table.default_features.unwrap_or(true);
            let features = table.features.clone().unwrap_or_default();

            let package = table.package.clone().filter(|p| !p.trim().is_empty());
            if table.package.as_ref().is_some_and(|p| p.trim().is_empty()) {
                return Err(manifest_invalid(
                    path,
                    location,
                    format!("dependency `{}` has an empty package rename", name),
                ));
            }

            (version, features, default_features, source, optional, package)
        }
    };

    if matches!(source, DependencySource::Registry) && version.is_none() {
        return Err(manifest_invalid(
            path,
            location,
            format!("dependency `{}` is missing a version requirement", name),
        ));
    }

    if let Some(version) = &version {
        if version.trim().is_empty() {
            return Err(manifest_invalid(
                path,
                location,
                format!("dependency `{}` has an empty version requirement", name),
            ));
        }

        if let Err(msg) = crate::dependency_resolver::validate_cargo_version_req(version) {
            return Err(manifest_invalid(path, location, format!("dependency `{name}`: {msg}")));
        }
    }

    Ok(DependencySpec {
        crate_name: name.to_string(),
        version,
        features,
        default_features,
        source,
        optional,
        package,
    })
}

/// Parse the source selector for one Rust dependency entry.
fn parse_dependency_source(
    table: &DependencyEntryTable,
    path: &Path,
    location: Option<ManifestLocation>,
) -> Result<(DependencySource, Option<String>), ManifestError> {
    let has_git = table.git.is_some();
    let has_path = table.path.is_some();
    if has_git && has_path {
        return Err(manifest_invalid(
            path,
            location,
            "dependency cannot specify both `git` and `path`",
        ));
    }

    if let Some(git) = &table.git {
        let reference = match (&table.branch, &table.tag, &table.rev) {
            (Some(branch), None, None) => GitReference::Branch(branch.clone()),
            (None, Some(tag), None) => GitReference::Tag(tag.clone()),
            (None, None, Some(rev)) => GitReference::Rev(rev.clone()),
            (None, None, None) => {
                return Err(manifest_invalid(
                    path,
                    location,
                    "git dependency must specify exactly one of branch, tag, or rev",
                ));
            }
            _ => {
                return Err(manifest_invalid(
                    path,
                    location,
                    "git dependency must specify exactly one of branch, tag, or rev",
                ));
            }
        };
        return Ok((
            DependencySource::Git {
                url: git.clone(),
                reference,
            },
            table.version.clone(),
        ));
    }

    if let Some(path_value) = &table.path {
        let manifest_dir = path.parent().unwrap_or_else(|| Path::new("."));
        let raw_path = PathBuf::from(path_value);
        let resolved_path = if raw_path.is_relative() {
            manifest_dir.join(raw_path)
        } else {
            raw_path
        };
        return Ok((DependencySource::Path { path: resolved_path }, table.version.clone()));
    }

    Ok((DependencySource::Registry, table.version.clone()))
}

/// Reject duplicate package identities across normal and dev Rust dependency tables.
fn validate_package_collisions(
    deps: &ParsedDependencyTable,
    dev_deps: &ParsedDependencyTable,
    path: &Path,
) -> Result<(), ManifestError> {
    let mut seen: HashMap<(String, String), String> = HashMap::new();

    let mut check = |spec: &DependencySpec| -> Result<(), ManifestError> {
        let package_name = spec.package.as_ref().unwrap_or(&spec.crate_name).to_string();
        let source_key = dependency_source_key(&spec.source);
        let key = (source_key, package_name.clone());
        let location = deps
            .locations
            .get(&spec.crate_name)
            .copied()
            .or_else(|| dev_deps.locations.get(&spec.crate_name).copied());

        if let Some(existing) = seen.get(&key) {
            if existing != &spec.crate_name {
                return Err(manifest_invalid(
                    path,
                    location,
                    format!(
                        "dependency keys collide: `{}` and `{}` resolve to the same package `{}`",
                        existing, spec.crate_name, package_name
                    ),
                ));
            }
        } else {
            seen.insert(key, spec.crate_name.clone());
        }

        Ok(())
    };

    for spec in deps.specs.values() {
        check(spec)?;
    }
    for spec in dev_deps.specs.values() {
        check(spec)?;
    }

    Ok(())
}

#[derive(Debug, Default)]
struct ParsedLibraryDependencyTable {
    specs: HashMap<String, LibraryDependencySpec>,
    workspace_refs: HashMap<String, WorkspaceLibraryDependencyRef>,
}

#[derive(Debug, Default)]
struct ParsedDependencyTable {
    specs: HashMap<String, DependencySpec>,
    locations: HashMap<String, ManifestLocation>,
    workspace_refs: HashMap<String, WorkspaceRustDependencyRef>,
}

/// Return a stable identity key for one dependency source.
fn dependency_source_key(source: &DependencySource) -> String {
    match source {
        DependencySource::Registry => "registry".to_string(),
        DependencySource::Git { url, reference } => format!("git:{url}:{:?}", reference),
        DependencySource::Path { path } => format!("path:{}", path.display()),
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    #[test]
    fn parse_empty_manifest() -> Result<(), ManifestError> {
        let manifest = ProjectManifest::from_str("", Path::new("incan.toml"))?;
        assert!(manifest.library_dependencies().is_empty());
        assert!(manifest.rust_dependencies().is_empty());
        assert!(manifest.rust_dev_dependencies().is_empty());
        Ok(())
    }

    #[test]
    fn malformed_manifest_reports_location() {
        let content = "[dependencies\nserde = \"1.0\"\n";
        let err = match ProjectManifest::from_str(content, Path::new("incan.toml")) {
            Err(err) => err,
            Ok(_) => panic!("expected malformed TOML to fail"),
        };
        let rendered = err.to_string();
        assert!(
            rendered.contains("incan.toml:1"),
            "expected line-aware parse error, got: {rendered}"
        );
    }

    #[test]
    fn parse_manifest_renamed_rust_dependency_tables() -> Result<(), ManifestError> {
        let content = r#"
[rust-dependencies]
tokio = "1.0"
serde = "1.0"

[rust-dev-dependencies]
pretty_assertions = "1.4"
"#;
        let manifest = ProjectManifest::from_str(content, Path::new("incan.toml"))?;
        assert_eq!(manifest.rust_dependencies().len(), 2);
        assert!(manifest.rust_dependencies().contains_key("tokio"));
        assert!(manifest.rust_dependencies().contains_key("serde"));
        assert!(manifest.rust_dev_dependencies().contains_key("pretty_assertions"));
        Ok(())
    }

    #[test]
    fn parses_workspace_rust_dependency_refs_without_assigning_member_local_identity() -> Result<(), ManifestError> {
        let content = r#"
[rust-dependencies]
serde = { workspace = true, features = ["derive"], optional = true }

[rust-dev-dependencies]
pretty_assertions = { workspace = true }
"#;
        let manifest = ProjectManifest::from_str(content, Path::new("incan.toml"))?;
        assert!(manifest.rust_dependencies().is_empty());
        assert!(manifest.rust_dev_dependencies().is_empty());
        assert_eq!(
            manifest.workspace_rust_dependencies().get("serde"),
            Some(&WorkspaceRustDependencyRef {
                features: vec!["derive".to_string()],
                optional: true,
            })
        );
        assert_eq!(
            manifest.workspace_rust_dev_dependencies().get("pretty_assertions"),
            Some(&WorkspaceRustDependencyRef {
                features: Vec::new(),
                optional: false,
            })
        );
        Ok(())
    }

    #[test]
    fn rejects_workspace_rust_dependency_identity_refinements() {
        let content = r#"
[rust-dependencies]
serde = { workspace = true, version = "1" }
"#;
        let error = ProjectManifest::from_str(content, Path::new("incan.toml"))
            .expect_err("workspace identity refinement must fail");
        assert!(error.to_string().contains("may refine only"));
    }

    #[test]
    fn parse_env_rust_dependency_overlay_tables() -> TestResult {
        let content = r#"
[tool.incan.envs.unit.rust-dependencies.serde]
version = "1.0"

[tool.incan.envs.unit.rust-dev-dependencies.proptest]
version = "1"
"#;
        let manifest = ProjectManifest::from_str(content, Path::new("incan.toml"))?;
        let tool = manifest.incan_tool().ok_or("missing [tool.incan]")?;
        let unit = tool.envs.get("unit").ok_or("missing unit env")?;

        assert!(unit.dependencies.contains_key("serde"));
        assert!(unit.dev_dependencies.contains_key("proptest"));
        Ok(())
    }

    #[test]
    fn parse_env_legacy_dependency_overlay_aliases() -> TestResult {
        let content = r#"
[tool.incan.envs.unit.dependencies.serde]
version = "1.0"

[tool.incan.envs.unit.dev-dependencies.proptest]
version = "1"
"#;
        let manifest = ProjectManifest::from_str(content, Path::new("incan.toml"))?;
        let tool = manifest.incan_tool().ok_or("missing [tool.incan]")?;
        let unit = tool.envs.get("unit").ok_or("missing unit env")?;

        assert!(unit.dependencies.contains_key("serde"));
        assert!(unit.dev_dependencies.contains_key("proptest"));
        Ok(())
    }

    #[test]
    fn parse_contract_model_bundle_paths() -> TestResult {
        let content = r#"
[project]
name = "contract-demo"

[project.scripts]
main = "src/main.incn"

[tool.incan.metadata]
model-bundles = ["contracts/order_summary.json"]
"#;
        let manifest = ProjectManifest::from_str(content, Path::new("incan.toml"))?;
        assert_eq!(
            manifest.contract_model_bundle_paths(),
            vec!["contracts/order_summary.json".to_string()]
        );
        Ok(())
    }

    #[test]
    fn manifest_owns_env_schema_and_base_overlay_translation() -> TestResult {
        let content = r#"
[rust-dependencies.serde]
version = "1.0"
features = ["derive"]

[rust-dev-dependencies.proptest]
version = "1"

[tool.incan.envs.unit]
extends = ["default"]
env-vars = { INCAN_NO_BANNER = "1" }
"#;
        let manifest = ProjectManifest::from_str(content, Path::new("incan.toml"))?;
        let envs = manifest.env_sections();
        let unit = envs.get("unit").ok_or("missing unit env")?;

        assert_eq!(unit.extends, vec!["default".to_string()]);
        assert_eq!(unit.env_vars.get("INCAN_NO_BANNER").map(String::as_str), Some("1"));
        assert_eq!(
            manifest
                .env_base_dependency_overlay()
                .get("serde")
                .and_then(toml::Value::as_table),
            Some(&toml::map::Map::from_iter([
                ("version".to_string(), toml::Value::String("1.0".to_string())),
                (
                    "features".to_string(),
                    toml::Value::Array(vec![toml::Value::String("derive".to_string())]),
                ),
            ]))
        );
        assert!(manifest.env_base_dev_dependency_overlay().contains_key("proptest"));
        Ok(())
    }

    #[test]
    fn parse_manifest_library_dependencies() -> TestResult {
        let content = r#"
[dependencies]
mylib = { path = "../mylib" }
"#;
        let manifest = ProjectManifest::from_str(content, Path::new("incan.toml"))?;
        let mylib = manifest
            .library_dependencies()
            .get("mylib")
            .ok_or("missing mylib library dependency")?;
        assert_eq!(mylib.library_name, "mylib");
        assert!(
            mylib.path.ends_with("mylib"),
            "expected path to end with mylib, got {}",
            mylib.path.display()
        );
        Ok(())
    }

    #[test]
    fn dependencies_with_rust_version_syntax_emits_migration_error() {
        let content = r#"
[dependencies]
serde = "1.0"
"#;
        let err = ProjectManifest::from_str(content, Path::new("incan.toml"));
        assert!(matches!(err, Err(ManifestError::Invalid { .. })));
    }

    #[test]
    fn dependencies_optional_subtable_emits_migration_error() {
        let content = r#"
[dependencies.optional]
fancy = { version = "0.3" }
"#;
        let err = ProjectManifest::from_str(content, Path::new("incan.toml"));
        assert!(matches!(err, Err(ManifestError::Invalid { .. })));
    }

    #[test]
    fn parse_renamed_rust_dependency_with_package_alias() -> TestResult {
        let content = r#"
[rust-dependencies]
serde_json = { package = "serde-json", version = "1.0" }
"#;
        let manifest = ProjectManifest::from_str(content, Path::new("incan.toml"))?;
        let dep = manifest
            .rust_dependencies()
            .get("serde_json")
            .ok_or("missing serde_json rust dep")?;
        assert_eq!(dep.package.as_deref(), Some("serde-json"));
        Ok(())
    }

    #[test]
    fn rust_alias_tables_conflict() {
        let content = r#"
[rust-dependencies]
serde = "1.0"

[rust.dependencies]
tokio = "1.0"
"#;
        let err = ProjectManifest::from_str(content, Path::new("incan.toml"));
        assert!(matches!(err, Err(ManifestError::Invalid { .. })));
    }

    #[test]
    fn rust_alias_tables_conflict_reports_location() {
        let content = "[rust-dependencies]\nserde = \"1.0\"\n\n[rust.dependencies]\ntokio = \"1.0\"\n";
        let err = match ProjectManifest::from_str(content, Path::new("incan.toml")) {
            Err(err) => err,
            Ok(_) => panic!("expected conflicting rust dependency tables to fail"),
        };
        let rendered = err.to_string();
        assert!(
            rendered.contains("incan.toml:4:1"),
            "expected line+column manifest error, got: {rendered}"
        );
    }

    #[test]
    fn unknown_project_field_reports_location() {
        let content = "[project]\nname = \"x\"\nversion = \"0.1.0\"\nunknown = true\n";
        let rendered = match ProjectManifest::from_str(content, Path::new("incan.toml")) {
            Err(err) => err.to_string(),
            Ok(_) => panic!("expected unknown project field to fail"),
        };
        assert!(
            rendered.contains("incan.toml:4:1"),
            "expected line+column manifest error, got: {rendered}"
        );
        assert!(
            rendered.contains("unknown field"),
            "expected unknown-field wording, got: {rendered}"
        );
    }

    #[test]
    fn unknown_dependency_option_reports_location() {
        let content = "[rust-dependencies]\nserde = { version = \"1.0\", feat = [\"derive\"] }\n";
        let rendered = match ProjectManifest::from_str(content, Path::new("incan.toml")) {
            Err(err) => err.to_string(),
            Ok(_) => panic!("expected unknown dependency option to fail"),
        };
        assert!(
            rendered.contains("incan.toml:2:35"),
            "expected line+column manifest error, got: {rendered}"
        );
        assert!(
            rendered.contains("unknown field"),
            "expected unknown-field wording, got: {rendered}"
        );
    }

    #[test]
    fn legacy_dev_dependencies_table_is_rejected() {
        let content = r#"
[dev-dependencies]
pretty_assertions = "1.4"
"#;
        let err = ProjectManifest::from_str(content, Path::new("incan.toml"));
        assert!(matches!(err, Err(ManifestError::Invalid { .. })));
    }

    #[test]
    fn invalid_git_source_errors() {
        let content = r#"
[rust-dependencies]
my_crate = { git = "https://example.com/repo", branch = "main", tag = "v1" }
"#;
        let err = ProjectManifest::from_str(content, Path::new("incan.toml"));
        assert!(matches!(err, Err(ManifestError::Invalid { .. })));
    }

    #[test]
    fn discover_finds_manifest_in_parent_directory() -> TestResult {
        let dir = tempdir_with_manifest(
            r#"
[rust-dependencies]
parent_crate = "2.0"
"#,
        )?;
        let subdir = dir.path().join("src").join("nested");
        fs::create_dir_all(&subdir)?;

        let manifest = ProjectManifest::discover(&subdir)?.ok_or("should find manifest in parent")?;
        assert!(manifest.rust_dependencies().contains_key("parent_crate"));
        Ok(())
    }

    #[test]
    fn project_root_normalizes_empty_parent_to_dot() -> Result<(), ManifestError> {
        let manifest = ProjectManifest::from_str("", Path::new("incan.toml"))?;
        assert_eq!(manifest.project_root(), Path::new("."));
        Ok(())
    }

    #[test]
    fn parse_vocab_section() -> TestResult {
        let content = r#"
[vocab]
crate = "crates/mylib_vocab"
"#;
        let manifest = ProjectManifest::from_str(content, Path::new("incan.toml"))?;
        let vocab = manifest.vocab().ok_or("missing vocab section")?;
        assert_eq!(vocab.crate_path.as_deref(), Some("crates/mylib_vocab"));
        Ok(())
    }

    #[test]
    fn parse_vocab_section_rejects_empty_crate() {
        let content = r#"
[vocab]
crate = "   "
"#;
        let rendered = match ProjectManifest::from_str(content, Path::new("incan.toml")) {
            Err(err) => err.to_string(),
            Ok(_) => panic!("expected empty crate field to fail"),
        };
        assert!(
            rendered.contains("incan.toml:3:9"),
            "expected line+column manifest error, got: {rendered}"
        );
    }

    #[test]
    fn parse_vocab_section_rejects_missing_crate() {
        let content = r#"
[vocab]
some_other_field = "value"
"#;
        let rendered = match ProjectManifest::from_str(content, Path::new("incan.toml")) {
            Err(err) => err.to_string(),
            Ok(_) => panic!("expected missing crate field to fail"),
        };
        assert!(
            rendered.contains("incan.toml:3:1"),
            "expected line+column manifest error, got: {rendered}"
        );
        assert!(
            rendered.contains("unknown field `some_other_field`"),
            "expected unknown-field wording, got: {rendered}"
        );
    }

    fn tempdir_with_manifest(content: &str) -> Result<tempfile::TempDir, Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        fs::write(dir.path().join(MANIFEST_FILENAME), content)?;
        Ok(dir)
    }
}
