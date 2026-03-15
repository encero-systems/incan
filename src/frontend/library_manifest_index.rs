//! Consumer-side index of dependency library manifests (`.incnlib`).
//!
//! Phase 3 of RFC 031 resolves `pub::` imports from dependency manifests rather than reparsing library source.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::library_manifest::{LibraryManifest, LibraryManifestError};
use crate::manifest::{DependencySource, DependencySpec, ProjectManifest};
use serde::Deserialize;

const LIBRARY_ARTIFACT_DIR: &str = "target/lib";
const LIBRARY_CRATE_LIB_RS: &str = "src/lib.rs";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LibraryManifestFailureKind {
    ManifestRead,
    ManifestParse,
    ManifestInvalid,
    ArtifactMissing,
    ArtifactInvalid,
    ArtifactMismatch,
}

#[derive(Debug, Clone)]
pub struct LibraryManifestLoadFailure {
    pub path: PathBuf,
    pub kind: LibraryManifestFailureKind,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibraryArtifactMetadata {
    pub dependency_key: String,
    pub manifest_name: String,
    pub manifest_path: PathBuf,
    pub crate_root: PathBuf,
    pub cargo_toml_path: PathBuf,
    pub crate_lib_path: PathBuf,
}

#[derive(Debug, Clone)]
pub enum LibraryManifestIndexEntry {
    Loaded {
        manifest: Box<LibraryManifest>,
        metadata: LibraryArtifactMetadata,
    },
    Failed(LibraryManifestLoadFailure),
}

#[derive(Debug, Clone, Default)]
pub struct LibraryManifestIndex {
    entries: HashMap<String, LibraryManifestIndexEntry>,
}

impl LibraryManifestIndex {
    pub fn from_entries(entries: HashMap<String, LibraryManifestIndexEntry>) -> Self {
        Self { entries }
    }

    pub fn from_project_manifest(manifest: &ProjectManifest) -> Self {
        let mut entries = HashMap::new();

        for (library_name, spec) in manifest.library_dependencies() {
            let entry = load_library_manifest_entry(library_name, &spec.path);
            entries.insert(library_name.clone(), entry);
        }

        Self { entries }
    }

    pub fn get(&self, library_name: &str) -> Option<&LibraryManifestIndexEntry> {
        self.entries.get(library_name)
    }

    pub fn known_libraries(&self) -> Vec<String> {
        let mut names: Vec<String> = self.entries.keys().cloned().collect();
        names.sort();
        names
    }

    /// Return metadata for a loaded dependency artifact.
    pub fn loaded_artifact(&self, dependency_key: &str) -> Option<&LibraryArtifactMetadata> {
        let entry = self.get(dependency_key)?;
        match entry {
            LibraryManifestIndexEntry::Loaded { metadata, .. } => Some(metadata),
            LibraryManifestIndexEntry::Failed(_) => None,
        }
    }

    /// Build path-based Cargo dependencies for all successfully loaded library artifacts.
    pub fn cargo_path_dependencies(&self) -> Vec<DependencySpec> {
        let mut dependencies = Vec::new();
        let mut keys: Vec<_> = self.entries.keys().cloned().collect();
        keys.sort();
        for key in keys {
            let Some(entry) = self.entries.get(&key) else {
                continue;
            };
            let LibraryManifestIndexEntry::Loaded { metadata, .. } = entry else {
                continue;
            };
            dependencies.push(metadata.to_dependency_spec());
        }
        dependencies
    }

    /// Return the mapped soft keywords for all successfully loaded library artifacts.
    /// The keys are the `dependency_key` (alias), making them ready for parser use.
    pub fn library_soft_keywords(&self) -> HashMap<String, Vec<incan_core::lang::keywords::KeywordId>> {
        let mut map = HashMap::new();
        for (key, entry) in &self.entries {
            if let LibraryManifestIndexEntry::Loaded { manifest, .. } = entry {
                let mut ids = Vec::new();
                for activation in &manifest.soft_keywords.activations {
                    if let Some(id) = incan_core::lang::keywords::from_str(&activation.keyword)
                        && incan_core::lang::keywords::is_soft(id)
                    {
                        ids.push(id);
                    }
                }
                if !ids.is_empty() {
                    map.insert(key.clone(), ids);
                }
            }
        }
        map
    }
}

fn load_library_manifest_entry(dependency_key: &str, dependency_root: &Path) -> LibraryManifestIndexEntry {
    let crate_root = dependency_crate_root(dependency_root);
    let manifest_path = match resolve_manifest_path(&crate_root, dependency_key) {
        Ok(path) => path,
        Err(failure) => return LibraryManifestIndexEntry::Failed(failure),
    };

    let manifest = match LibraryManifest::read_from_path(&manifest_path) {
        Ok(loaded) => loaded,
        Err(error) => {
            let failure = LibraryManifestLoadFailure::from_manifest_error(manifest_path, error);
            return LibraryManifestIndexEntry::Failed(failure);
        }
    };

    let metadata = match validate_artifact_contract(dependency_key, &manifest, &manifest_path, &crate_root) {
        Ok(metadata) => metadata,
        Err(failure) => return LibraryManifestIndexEntry::Failed(failure),
    };

    LibraryManifestIndexEntry::Loaded {
        manifest: Box::new(manifest),
        metadata,
    }
}

fn dependency_crate_root(dependency_root: &Path) -> PathBuf {
    dependency_root.join(LIBRARY_ARTIFACT_DIR)
}

fn resolve_manifest_path(crate_root: &Path, dependency_key: &str) -> Result<PathBuf, LibraryManifestLoadFailure> {
    if !crate_root.is_dir() {
        return Err(LibraryManifestLoadFailure {
            path: crate_root.to_path_buf(),
            kind: LibraryManifestFailureKind::ArtifactMissing,
            message: format!("missing generated library artifacts at `{}`", crate_root.display()),
        });
    }

    let expected = crate_root.join(format!("{dependency_key}.incnlib"));
    if expected.is_file() {
        return Ok(expected);
    }

    let mut candidates = Vec::new();
    let read_dir = fs::read_dir(crate_root).map_err(|error| LibraryManifestLoadFailure {
        path: crate_root.to_path_buf(),
        kind: LibraryManifestFailureKind::ArtifactInvalid,
        message: format!("failed to inspect `{}`: {error}", crate_root.display()),
    })?;
    for entry in read_dir {
        let entry = entry.map_err(|error| LibraryManifestLoadFailure {
            path: crate_root.to_path_buf(),
            kind: LibraryManifestFailureKind::ArtifactInvalid,
            message: format!("failed to inspect `{}`: {error}", crate_root.display()),
        })?;
        let path = entry.path();
        if path.extension().is_some_and(|ext| ext == "incnlib") {
            candidates.push(path);
        }
    }

    candidates.sort();
    if candidates.is_empty() {
        return Err(LibraryManifestLoadFailure {
            path: expected,
            kind: LibraryManifestFailureKind::ArtifactMissing,
            message: format!(
                "missing library manifest `{}` (run `incan build --lib` in the dependency project)",
                dependency_key
            ),
        });
    }
    if candidates.len() > 1 {
        let names: Vec<String> = candidates
            .iter()
            .map(|candidate| {
                candidate
                    .file_name()
                    .map(|name| name.to_string_lossy().to_string())
                    .unwrap_or_else(|| candidate.display().to_string())
            })
            .collect();
        return Err(LibraryManifestLoadFailure {
            path: crate_root.to_path_buf(),
            kind: LibraryManifestFailureKind::ArtifactMismatch,
            message: format!(
                "multiple manifests found for `pub::{dependency_key}`: {}",
                names.join(", ")
            ),
        });
    }

    // Alias case: dependency key differs from producer package/manifest name.
    Ok(candidates.remove(0))
}

fn validate_artifact_contract(
    dependency_key: &str,
    manifest: &LibraryManifest,
    manifest_path: &Path,
    crate_root: &Path,
) -> Result<LibraryArtifactMetadata, LibraryManifestLoadFailure> {
    let cargo_toml_path = crate_root.join("Cargo.toml");
    if !cargo_toml_path.is_file() {
        return Err(LibraryManifestLoadFailure {
            path: cargo_toml_path,
            kind: LibraryManifestFailureKind::ArtifactMissing,
            message: "missing generated Cargo.toml".to_string(),
        });
    }

    let crate_lib_path = crate_root.join(LIBRARY_CRATE_LIB_RS);
    if !crate_lib_path.is_file() {
        return Err(LibraryManifestLoadFailure {
            path: crate_lib_path,
            kind: LibraryManifestFailureKind::ArtifactMissing,
            message: format!("missing generated `{LIBRARY_CRATE_LIB_RS}`"),
        });
    }

    let cargo_contract = parse_cargo_contract(&cargo_toml_path)?;
    if cargo_contract.package_name != manifest.name {
        return Err(LibraryManifestLoadFailure {
            path: cargo_toml_path,
            kind: LibraryManifestFailureKind::ArtifactMismatch,
            message: format!(
                "manifest name `{}` does not match Cargo package `{}`",
                manifest.name, cargo_contract.package_name
            ),
        });
    }
    if !cargo_contract.uses_default_lib_target {
        return Err(LibraryManifestLoadFailure {
            path: cargo_toml_path,
            kind: LibraryManifestFailureKind::ArtifactInvalid,
            message: format!("library crate target must use `{LIBRARY_CRATE_LIB_RS}` for `pub::{dependency_key}`"),
        });
    }

    let expected_manifest_file = format!("{}.incnlib", manifest.name);
    let actual_manifest_file = manifest_path
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_default();
    if actual_manifest_file != expected_manifest_file {
        return Err(LibraryManifestLoadFailure {
            path: manifest_path.to_path_buf(),
            kind: LibraryManifestFailureKind::ArtifactMismatch,
            message: format!(
                "manifest filename `{actual_manifest_file}` does not match manifest name `{}`",
                manifest.name
            ),
        });
    }

    Ok(LibraryArtifactMetadata::from_manifest_path(
        dependency_key,
        manifest.name.clone(),
        manifest_path.to_path_buf(),
        crate_root.to_path_buf(),
    ))
}

#[derive(Debug, Deserialize)]
struct CargoContractToml {
    package: Option<CargoContractPackage>,
    lib: Option<CargoContractLib>,
}

#[derive(Debug, Deserialize)]
struct CargoContractPackage {
    name: String,
}

#[derive(Debug, Deserialize)]
struct CargoContractLib {
    path: Option<String>,
}

struct ParsedCargoContract {
    package_name: String,
    uses_default_lib_target: bool,
}

fn parse_cargo_contract(path: &Path) -> Result<ParsedCargoContract, LibraryManifestLoadFailure> {
    let content = fs::read_to_string(path).map_err(|error| LibraryManifestLoadFailure {
        path: path.to_path_buf(),
        kind: LibraryManifestFailureKind::ArtifactInvalid,
        message: format!("failed to read Cargo.toml: {error}"),
    })?;

    let parsed: CargoContractToml = toml::from_str(&content).map_err(|error| LibraryManifestLoadFailure {
        path: path.to_path_buf(),
        kind: LibraryManifestFailureKind::ArtifactInvalid,
        message: format!("failed to parse Cargo.toml: {error}"),
    })?;

    let package_name = parsed
        .package
        .map(|package| package.name)
        .filter(|name| !name.trim().is_empty())
        .ok_or_else(|| LibraryManifestLoadFailure {
            path: path.to_path_buf(),
            kind: LibraryManifestFailureKind::ArtifactInvalid,
            message: "Cargo.toml is missing `[package].name`".to_string(),
        })?;
    let uses_default_lib_target = match parsed.lib.as_ref().and_then(|lib| lib.path.as_ref()) {
        Some(path) => path.trim().replace('\\', "/") == LIBRARY_CRATE_LIB_RS,
        None => true,
    };

    Ok(ParsedCargoContract {
        package_name,
        uses_default_lib_target,
    })
}

impl LibraryArtifactMetadata {
    /// Build artifact metadata when both the resolved `.incnlib` path and crate root are known.
    pub fn from_manifest_path(
        dependency_key: impl Into<String>,
        manifest_name: impl Into<String>,
        manifest_path: PathBuf,
        crate_root: PathBuf,
    ) -> Self {
        let dependency_key = dependency_key.into();
        let manifest_name = manifest_name.into();
        Self {
            dependency_key,
            manifest_name,
            manifest_path,
            cargo_toml_path: crate_root.join("Cargo.toml"),
            crate_lib_path: crate_root.join(LIBRARY_CRATE_LIB_RS),
            crate_root,
        }
    }

    /// Build artifact metadata from a crate root using the conventional `<manifest_name>.incnlib` file name.
    pub fn from_crate_root(
        dependency_key: impl Into<String>,
        manifest_name: impl Into<String>,
        crate_root: impl Into<PathBuf>,
    ) -> Self {
        let crate_root = crate_root.into();
        let manifest_name = manifest_name.into();
        let manifest_path = crate_root.join(format!("{manifest_name}.incnlib"));
        Self::from_manifest_path(dependency_key, manifest_name, manifest_path, crate_root)
    }

    fn to_dependency_spec(&self) -> DependencySpec {
        DependencySpec {
            crate_name: self.dependency_key.clone(),
            version: None,
            features: Vec::new(),
            default_features: true,
            source: DependencySource::Path {
                path: self.crate_root.clone(),
            },
            optional: false,
            package: if self.dependency_key == self.manifest_name {
                None
            } else {
                Some(self.manifest_name.clone())
            },
        }
        .normalized()
    }
}

impl LibraryManifestLoadFailure {
    fn from_manifest_error(path: PathBuf, error: LibraryManifestError) -> Self {
        let kind = match &error {
            LibraryManifestError::Read { .. } | LibraryManifestError::Write { .. } => {
                LibraryManifestFailureKind::ManifestRead
            }
            LibraryManifestError::Parse(_) | LibraryManifestError::Serialize(_) => {
                LibraryManifestFailureKind::ManifestParse
            }
            LibraryManifestError::Invalid(_) => LibraryManifestFailureKind::ManifestInvalid,
        };
        Self {
            path,
            kind,
            message: error.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_dependency_manifest_into_index() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let consumer_manifest_path = tmp.path().join("incan.toml");
        let dep_root = tmp.path().join("deps").join("mylib");
        let dep_artifact_root = dep_root.join("target").join("lib");
        let dep_manifest_path = dep_artifact_root.join("mylib.incnlib");

        std::fs::create_dir_all(dep_artifact_root.join("src"))?;
        let manifest = LibraryManifest::new("mylib", "0.1.0");
        manifest.write_to_path(&dep_manifest_path)?;
        std::fs::write(
            dep_artifact_root.join("Cargo.toml"),
            "[package]\nname = \"mylib\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )?;
        std::fs::write(dep_artifact_root.join("src/lib.rs"), "pub fn ready() {}\n")?;

        let manifest_content = r#"
[dependencies]
mylib = { path = "deps/mylib" }
"#;
        std::fs::write(&consumer_manifest_path, manifest_content)?;
        let parsed = ProjectManifest::from_str(manifest_content, &consumer_manifest_path)?;

        let index = LibraryManifestIndex::from_project_manifest(&parsed);
        let entry = index.get("mylib").ok_or("missing mylib index entry")?;
        match entry {
            LibraryManifestIndexEntry::Loaded { manifest, metadata } => {
                assert_eq!(manifest.name, "mylib");
                assert_eq!(manifest.version, "0.1.0");
                assert_eq!(metadata.dependency_key, "mylib");
                assert_eq!(metadata.manifest_name, "mylib");
                assert_eq!(metadata.crate_root, dep_artifact_root);
            }
            LibraryManifestIndexEntry::Failed(failure) => {
                return Err(format!("expected loaded manifest, got failure: {}", failure.message).into());
            }
        }

        let specs = index.cargo_path_dependencies();
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].crate_name, "mylib");
        assert!(matches!(specs[0].source, DependencySource::Path { .. }));
        assert_eq!(specs[0].package, None);

        Ok(())
    }

    #[test]
    fn records_failed_entry_for_missing_dependency_manifest() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let consumer_manifest_path = tmp.path().join("incan.toml");
        let dep_root = tmp.path().join("deps").join("missinglib");
        std::fs::create_dir_all(&dep_root)?;

        let manifest_content = r#"
[dependencies]
missinglib = { path = "deps/missinglib" }
"#;
        std::fs::write(&consumer_manifest_path, manifest_content)?;
        let parsed = ProjectManifest::from_str(manifest_content, &consumer_manifest_path)?;

        let index = LibraryManifestIndex::from_project_manifest(&parsed);
        let entry = index.get("missinglib").ok_or("missing dependency index entry")?;
        match entry {
            LibraryManifestIndexEntry::Loaded { .. } => {
                return Err("expected failed manifest entry for missing file".into());
            }
            LibraryManifestIndexEntry::Failed(failure) => {
                assert_eq!(failure.kind, LibraryManifestFailureKind::ArtifactMissing);
                assert!(
                    failure.message.contains("missing generated library artifacts"),
                    "unexpected failure: {}",
                    failure.message
                );
            }
        }

        Ok(())
    }

    #[test]
    fn supports_dependency_key_alias_to_manifest_name() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let consumer_manifest_path = tmp.path().join("incan.toml");
        let dep_root = tmp.path().join("deps").join("widgets-lib");
        let dep_artifact_root = dep_root.join("target").join("lib");
        std::fs::create_dir_all(dep_artifact_root.join("src"))?;

        let manifest = LibraryManifest::new("widgets_core", "0.1.0");
        manifest.write_to_path(&dep_artifact_root.join("widgets_core.incnlib"))?;
        std::fs::write(
            dep_artifact_root.join("Cargo.toml"),
            "[package]\nname = \"widgets_core\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )?;
        std::fs::write(dep_artifact_root.join("src/lib.rs"), "pub fn widgets() {}\n")?;

        let manifest_content = r#"
[dependencies]
widgets = { path = "deps/widgets-lib" }
"#;
        std::fs::write(&consumer_manifest_path, manifest_content)?;
        let parsed = ProjectManifest::from_str(manifest_content, &consumer_manifest_path)?;

        let index = LibraryManifestIndex::from_project_manifest(&parsed);
        let entry = index.get("widgets").ok_or("missing widgets entry")?;
        match entry {
            LibraryManifestIndexEntry::Loaded { metadata, .. } => {
                assert_eq!(metadata.dependency_key, "widgets");
                assert_eq!(metadata.manifest_name, "widgets_core");
            }
            LibraryManifestIndexEntry::Failed(failure) => {
                return Err(format!("expected loaded entry, got: {failure:?}").into());
            }
        }

        let specs = index.cargo_path_dependencies();
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].crate_name, "widgets");
        assert_eq!(specs[0].package.as_deref(), Some("widgets_core"));

        Ok(())
    }

    #[test]
    fn records_failure_for_manifest_and_cargo_name_mismatch() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let consumer_manifest_path = tmp.path().join("incan.toml");
        let dep_root = tmp.path().join("deps").join("broken");
        let dep_artifact_root = dep_root.join("target").join("lib");
        std::fs::create_dir_all(dep_artifact_root.join("src"))?;

        let manifest = LibraryManifest::new("widgets_core", "0.1.0");
        manifest.write_to_path(&dep_artifact_root.join("widgets_core.incnlib"))?;
        std::fs::write(
            dep_artifact_root.join("Cargo.toml"),
            "[package]\nname = \"totally_different\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )?;
        std::fs::write(dep_artifact_root.join("src/lib.rs"), "pub fn broken() {}\n")?;

        let manifest_content = r#"
[dependencies]
widgets = { path = "deps/broken" }
"#;
        std::fs::write(&consumer_manifest_path, manifest_content)?;
        let parsed = ProjectManifest::from_str(manifest_content, &consumer_manifest_path)?;

        let index = LibraryManifestIndex::from_project_manifest(&parsed);
        let entry = index.get("widgets").ok_or("missing widgets entry")?;
        match entry {
            LibraryManifestIndexEntry::Loaded { .. } => {
                return Err("expected failed entry for name mismatch".into());
            }
            LibraryManifestIndexEntry::Failed(failure) => {
                assert_eq!(failure.kind, LibraryManifestFailureKind::ArtifactMismatch);
                assert!(failure.message.contains("does not match Cargo package"));
            }
        }

        Ok(())
    }
}
