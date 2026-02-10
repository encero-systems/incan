//! Project manifest (`incan.toml`) discovery and parsing.
//!
//! Implements the `incan.toml` schema from RFC 013 (Rust crate dependencies) and RFC 015 (project discovery).
//! This module is responsible for locating the manifest and parsing dependency tables into structured specs that the
//! dependency resolver can validate.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The canonical manifest filename that the compiler searches for.
pub const MANIFEST_FILENAME: &str = "incan.toml";

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
    #[error("failed to parse {path}: {source}")]
    Parse { path: PathBuf, source: toml::de::Error },

    /// The file was parsed but contains invalid configuration.
    #[error("invalid manifest {path}: {message}")]
    Invalid { path: PathBuf, message: String },
}

// ============================================================================
// Dependency specification types
// ============================================================================

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DependencySource {
    Registry,
    Git { url: String, reference: GitReference },
    Path { path: PathBuf },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GitReference {
    Branch(String),
    Tag(String),
    Rev(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
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
    pub fn normalized(mut self) -> Self {
        self.features.sort();
        self.features.dedup();
        self
    }
}

// ============================================================================
// Project manifest
// ============================================================================

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProjectSection {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub authors: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub readme: Option<String>,
    #[serde(rename = "requires-incan", skip_serializing_if = "Option::is_none")]
    pub requires_incan: Option<String>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub scripts: HashMap<String, String>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub features: HashMap<String, Vec<String>>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BuildSection {
    #[serde(rename = "rust-edition", skip_serializing_if = "Option::is_none")]
    pub rust_edition: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    /// Explicit source root directory (relative to project root).
    ///
    /// When set, the compiler and test runner resolve user module imports against this directory.
    /// If omitted, `src/` is used by convention when it exists, otherwise the project root itself.
    #[serde(rename = "source-root", skip_serializing_if = "Option::is_none")]
    pub source_root: Option<String>,
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
    /// `[dependencies]` (Rust crate dependencies).
    dependencies: HashMap<String, DependencySpec>,
    /// `[dev-dependencies]` (dev-only Rust crates).
    dev_dependencies: HashMap<String, DependencySpec>,
}

impl ProjectManifest {
    /// Discover and parse an `incan.toml` manifest by walking upward from `start_dir`.
    ///
    /// Returns `Ok(None)` if no `incan.toml` is found (e.g., single-file mode).
    /// Returns `Err` if a manifest is found but cannot be read or parsed.
    pub fn discover(start_dir: &Path) -> Result<Option<Self>, ManifestError> {
        let manifest_path = match find_manifest(start_dir) {
            Some(path) => path,
            None => return Ok(None),
        };

        let content = std::fs::read_to_string(&manifest_path).map_err(|e| ManifestError::Read {
            path: manifest_path.clone(),
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

    /// The set of crate names declared in `[dependencies]` (normal deps only).
    pub fn declared_crate_names(&self) -> HashSet<String> {
        self.dependencies.keys().cloned().collect()
    }

    /// The set of crate names declared in `[dev-dependencies]` only.
    pub fn declared_dev_crate_names(&self) -> HashSet<String> {
        self.dev_dependencies.keys().cloned().collect()
    }

    /// Normal Rust dependencies from the manifest.
    pub fn dependencies(&self) -> &HashMap<String, DependencySpec> {
        &self.dependencies
    }

    /// Dev-only Rust dependencies from the manifest.
    pub fn dev_dependencies(&self) -> &HashMap<String, DependencySpec> {
        &self.dev_dependencies
    }

    /// Path to the `incan.toml` file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The project root directory (parent of `incan.toml`).
    pub fn project_root(&self) -> &Path {
        self.path.parent().unwrap_or_else(|| Path::new("."))
    }
}

// ============================================================================
// Internal helpers
// ============================================================================

#[derive(Debug, Default, Deserialize)]
struct RawManifest {
    #[serde(default)]
    project: Option<ProjectSection>,
    #[serde(default)]
    build: Option<BuildSection>,
    #[serde(default)]
    dependencies: Option<DependencyTable>,
    #[serde(rename = "dev-dependencies", default)]
    dev_dependencies: Option<DependencyTable>,
    #[serde(default)]
    rust: Option<RustTables>,
}

#[derive(Debug, Default, Deserialize)]
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
}

fn parse_manifest_content(content: &str, path: &Path) -> Result<ProjectManifest, ManifestError> {
    let raw: RawManifest = toml::from_str(content).map_err(|e| ManifestError::Parse {
        path: path.to_path_buf(),
        source: e,
    })?;

    let (deps_table, dev_deps_table) = resolve_dependency_tables(&raw, path)?;
    let dependencies = deps_table
        .map(|table| parse_dependency_table(&table, path))
        .transpose()?
        .unwrap_or_default();
    let dev_dependencies = dev_deps_table
        .map(|table| parse_dependency_table(&table, path))
        .transpose()?
        .unwrap_or_default();

    validate_package_collisions(&dependencies, &dev_dependencies, path)?;

    Ok(ProjectManifest {
        path: path.to_path_buf(),
        project: raw.project,
        build: raw.build,
        dependencies,
        dev_dependencies,
    })
}

fn resolve_dependency_tables(
    raw: &RawManifest,
    path: &Path,
) -> Result<(Option<DependencyTable>, Option<DependencyTable>), ManifestError> {
    let rust_tables = raw.rust.as_ref();
    let deps = raw.dependencies.clone();
    let rust_deps = rust_tables.and_then(|r| r.dependencies.clone());
    let dev_deps = raw.dev_dependencies.clone();
    let rust_dev_deps = rust_tables.and_then(|r| r.dev_dependencies.clone());

    if deps.is_some() && rust_deps.is_some() {
        return Err(ManifestError::Invalid {
            path: path.to_path_buf(),
            message: "cannot specify both [dependencies] and [rust.dependencies]".to_string(),
        });
    }
    if dev_deps.is_some() && rust_dev_deps.is_some() {
        return Err(ManifestError::Invalid {
            path: path.to_path_buf(),
            message: "cannot specify both [dev-dependencies] and [rust.dev-dependencies]".to_string(),
        });
    }

    Ok((deps.or(rust_deps), dev_deps.or(rust_dev_deps)))
}

fn parse_dependency_table(
    table: &DependencyTable,
    path: &Path,
) -> Result<HashMap<String, DependencySpec>, ManifestError> {
    let mut result = HashMap::new();

    for (name, entry) in &table.entries {
        if table.optional.contains_key(name) {
            return Err(ManifestError::Invalid {
                path: path.to_path_buf(),
                message: format!(
                    "dependency `{}` appears in both [dependencies] and [dependencies.optional]",
                    name
                ),
            });
        }
        let spec = dependency_from_entry(name, entry, false, path)?;
        result.insert(name.clone(), spec);
    }

    for (name, entry) in &table.optional {
        let spec = dependency_from_entry(name, entry, true, path)?;
        result.insert(name.clone(), spec);
    }

    Ok(result)
}

fn dependency_from_entry(
    name: &str,
    entry: &DependencyEntry,
    optional_override: bool,
    path: &Path,
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
            let (source, version) = parse_dependency_source(table, path)?;
            let mut optional = table.optional.unwrap_or(false);
            if optional_override {
                optional = true;
            }
            let default_features = table.default_features.unwrap_or(true);
            let features = table.features.clone().unwrap_or_default();

            let package = table.package.clone().filter(|p| !p.trim().is_empty());
            if table.package.as_ref().is_some_and(|p| p.trim().is_empty()) {
                return Err(ManifestError::Invalid {
                    path: path.to_path_buf(),
                    message: format!("dependency `{}` has an empty package rename", name),
                });
            }

            (version, features, default_features, source, optional, package)
        }
    };

    if matches!(source, DependencySource::Registry) && version.is_none() {
        return Err(ManifestError::Invalid {
            path: path.to_path_buf(),
            message: format!("dependency `{}` is missing a version requirement", name),
        });
    }

    if let Some(version) = &version {
        if version.trim().is_empty() {
            return Err(ManifestError::Invalid {
                path: path.to_path_buf(),
                message: format!("dependency `{}` has an empty version requirement", name),
            });
        }

        if let Err(msg) = crate::dependency_resolver::validate_cargo_version_req(version) {
            return Err(ManifestError::Invalid {
                path: path.to_path_buf(),
                message: format!("dependency `{name}`: {msg}"),
            });
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

fn parse_dependency_source(
    table: &DependencyEntryTable,
    path: &Path,
) -> Result<(DependencySource, Option<String>), ManifestError> {
    let has_git = table.git.is_some();
    let has_path = table.path.is_some();
    if has_git && has_path {
        return Err(ManifestError::Invalid {
            path: path.to_path_buf(),
            message: "dependency cannot specify both `git` and `path`".to_string(),
        });
    }

    if let Some(git) = &table.git {
        let reference = match (&table.branch, &table.tag, &table.rev) {
            (Some(branch), None, None) => GitReference::Branch(branch.clone()),
            (None, Some(tag), None) => GitReference::Tag(tag.clone()),
            (None, None, Some(rev)) => GitReference::Rev(rev.clone()),
            (None, None, None) => {
                return Err(ManifestError::Invalid {
                    path: path.to_path_buf(),
                    message: "git dependency must specify exactly one of branch, tag, or rev".to_string(),
                });
            }
            _ => {
                return Err(ManifestError::Invalid {
                    path: path.to_path_buf(),
                    message: "git dependency must specify exactly one of branch, tag, or rev".to_string(),
                });
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

fn validate_package_collisions(
    deps: &HashMap<String, DependencySpec>,
    dev_deps: &HashMap<String, DependencySpec>,
    path: &Path,
) -> Result<(), ManifestError> {
    let mut seen: HashMap<(String, String), String> = HashMap::new();

    let mut check = |spec: &DependencySpec| -> Result<(), ManifestError> {
        let package_name = spec.package.as_ref().unwrap_or(&spec.crate_name).to_string();
        let source_key = dependency_source_key(&spec.source);
        let key = (source_key, package_name.clone());

        if let Some(existing) = seen.get(&key) {
            if existing != &spec.crate_name {
                return Err(ManifestError::Invalid {
                    path: path.to_path_buf(),
                    message: format!(
                        "dependency keys collide: `{}` and `{}` resolve to the same package `{}`",
                        existing, spec.crate_name, package_name
                    ),
                });
            }
        } else {
            seen.insert(key, spec.crate_name.clone());
        }

        Ok(())
    };

    for spec in deps.values() {
        check(spec)?;
    }
    for spec in dev_deps.values() {
        check(spec)?;
    }

    Ok(())
}

fn dependency_source_key(source: &DependencySource) -> String {
    match source {
        DependencySource::Registry => "registry".to_string(),
        DependencySource::Git { url, reference } => format!("git:{url}:{:?}", reference),
        DependencySource::Path { path } => format!("path:{}", path.display()),
    }
}

/// Walk upward from `start_dir` to find an `incan.toml` file.
fn find_manifest(start_dir: &Path) -> Option<PathBuf> {
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
        assert!(manifest.dependencies().is_empty());
        assert!(manifest.dev_dependencies().is_empty());
        Ok(())
    }

    #[test]
    fn parse_manifest_string_version_dependencies() -> Result<(), ManifestError> {
        let content = r#"
[dependencies]
tokio = "1.0"
serde = "1.0"
"#;
        let manifest = ProjectManifest::from_str(content, Path::new("incan.toml"))?;
        assert_eq!(manifest.dependencies().len(), 2);
        assert!(manifest.dependencies().contains_key("tokio"));
        assert!(manifest.dependencies().contains_key("serde"));
        Ok(())
    }

    #[test]
    fn parse_manifest_table_dependencies() -> TestResult {
        let content = r#"
[dependencies]
tokio = { version = "1.0", features = ["full"] }
my_crate = { path = "../my_crate" }
"#;
        let manifest = ProjectManifest::from_str(content, Path::new("incan.toml"))?;
        let tokio = manifest.dependencies().get("tokio").ok_or("missing tokio dep")?;
        assert_eq!(tokio.version.as_deref(), Some("1.0"));
        assert_eq!(tokio.features, vec!["full".to_string()]);
        assert!(matches!(tokio.source, DependencySource::Registry));
        Ok(())
    }

    #[test]
    fn parse_manifest_optional_dependencies() -> TestResult {
        let content = r#"
[dependencies.optional]
fancy = { version = "0.3" }
"#;
        let manifest = ProjectManifest::from_str(content, Path::new("incan.toml"))?;
        let fancy = manifest.dependencies().get("fancy").ok_or("missing fancy dep")?;
        assert!(fancy.optional);
        Ok(())
    }

    #[test]
    fn parse_manifest_dependency_rename() -> TestResult {
        let content = r#"
[dependencies]
serde_json = { package = "serde-json", version = "1.0" }
"#;
        let manifest = ProjectManifest::from_str(content, Path::new("incan.toml"))?;
        let dep = manifest
            .dependencies()
            .get("serde_json")
            .ok_or("missing serde_json dep")?;
        assert_eq!(dep.package.as_deref(), Some("serde-json"));
        Ok(())
    }

    #[test]
    fn alias_tables_conflict() {
        let content = r#"
[dependencies]
serde = "1.0"

[rust.dependencies]
tokio = "1.0"
"#;
        let err = ProjectManifest::from_str(content, Path::new("incan.toml"));
        assert!(matches!(err, Err(ManifestError::Invalid { .. })));
    }

    #[test]
    fn invalid_git_source_errors() {
        let content = r#"
[dependencies]
my_crate = { git = "https://example.com/repo", branch = "main", tag = "v1" }
"#;
        let err = ProjectManifest::from_str(content, Path::new("incan.toml"));
        assert!(matches!(err, Err(ManifestError::Invalid { .. })));
    }

    #[test]
    fn discover_finds_manifest_in_parent_directory() -> TestResult {
        let dir = tempdir_with_manifest(
            r#"
[dependencies]
parent_crate = "2.0"
"#,
        )?;
        let subdir = dir.path().join("src").join("nested");
        fs::create_dir_all(&subdir)?;

        let manifest = ProjectManifest::discover(&subdir)?.ok_or("should find manifest in parent")?;
        assert!(manifest.dependencies().contains_key("parent_crate"));
        Ok(())
    }

    fn tempdir_with_manifest(content: &str) -> Result<tempfile::TempDir, Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        fs::write(dir.path().join(MANIFEST_FILENAME), content)?;
        Ok(dir)
    }
}
