//! Consumer-side index of dependency library manifests (`.incnlib`).
//!
//! Phase 3 of RFC 031 resolves `pub::` imports from dependency manifests rather than reparsing library source.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

use crate::library_manifest::{LibraryManifest, LibraryManifestError};
use crate::manifest::{DependencySource, DependencySpec, ProjectManifest};
use incan_vocab::{CargoDependency, CargoDependencySource, KeywordActivation, KeywordRegistration, KeywordSpec};
use serde::Deserialize;

const LIBRARY_ARTIFACT_DIR: &str = "target/lib";
const LIBRARY_CRATE_LIB_RS: &str = "src/lib.rs";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Category for dependency manifest loading failures.
///
/// This is intended for diagnostics and test assertions rather than end-user display; user-facing
/// messaging should use [`LibraryManifestLoadFailure::message`].
pub enum LibraryManifestFailureKind {
    /// Failed to read or write the manifest file.
    ManifestRead,
    /// Failed to parse or serialize manifest content.
    ManifestParse,
    /// Manifest content is structurally invalid.
    ManifestInvalid,
    /// Expected generated artifact path is missing.
    ArtifactMissing,
    /// Artifact exists but has invalid structure/content.
    ArtifactInvalid,
    /// Artifact contract does not match expected dependency metadata.
    ArtifactMismatch,
}

#[derive(Debug, Clone)]
/// Detailed failure information when indexing a dependency manifest entry.
pub struct LibraryManifestLoadFailure {
    /// Path associated with the failure.
    pub path: PathBuf,
    /// Broad failure category for branching/reporting.
    pub kind: LibraryManifestFailureKind,
    /// Human-readable failure message.
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Contract metadata for a successfully loaded generated library artifact.
pub struct LibraryArtifactMetadata {
    /// Dependency key as written by the consumer (for example `widgets` in `pub::widgets`).
    pub dependency_key: String,
    /// Declared producer manifest name (`manifest.name`), which can differ from dependency key aliases.
    pub manifest_name: String,
    /// Resolved `.incnlib` path.
    pub manifest_path: PathBuf,
    /// Root of generated library crate (typically `target/lib`).
    pub crate_root: PathBuf,
    /// Path to generated `Cargo.toml` for `pub::` crate wiring.
    pub cargo_toml_path: PathBuf,
    /// Path to generated crate entrypoint (`src/lib.rs`).
    pub crate_lib_path: PathBuf,
}

#[derive(Debug, Clone)]
/// Indexed dependency entry, either loaded or failed.
pub enum LibraryManifestIndexEntry {
    /// Loaded manifest and validated artifact metadata.
    Loaded {
        /// Parsed dependency manifest payload.
        manifest: Box<LibraryManifest>,
        /// Validated paths and names used by downstream tooling.
        metadata: LibraryArtifactMetadata,
    },
    /// Captured failure for this dependency key.
    Failed(LibraryManifestLoadFailure),
}

#[derive(Debug, Clone, Default)]
/// Consumer-side index for dependency manifests and generated `pub::` artifacts.
pub struct LibraryManifestIndex {
    entries: HashMap<String, LibraryManifestIndexEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Error emitted when provider-declared requirements cannot be merged safely.
pub enum ProviderRequirementError {
    /// A provider declared an invalid dependency payload.
    InvalidDependency {
        /// Dependency key (`pub::<key>`) that supplied the invalid payload.
        dependency_key: String,
        /// Validation error details.
        message: String,
    },
    /// Two providers declared incompatible specs for the same crate name.
    DependencyConflict {
        /// Crate name that collided.
        crate_name: String,
        /// First provider key that introduced the crate requirement.
        existing_provider: String,
        /// Second provider key whose requirement is incompatible.
        incoming_provider: String,
    },
}

impl std::fmt::Display for ProviderRequirementError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProviderRequirementError::InvalidDependency {
                dependency_key,
                message,
            } => write!(
                f,
                "provider requirements for `pub::{dependency_key}` are invalid: {message}"
            ),
            ProviderRequirementError::DependencyConflict {
                crate_name,
                existing_provider,
                incoming_provider,
            } => write!(
                f,
                "provider dependency `{crate_name}` conflicts between `pub::{existing_provider}` and `pub::{incoming_provider}`"
            ),
        }
    }
}

impl std::error::Error for ProviderRequirementError {}

impl LibraryManifestIndex {
    /// Construct an index from precomputed entries.
    pub fn from_entries(entries: HashMap<String, LibraryManifestIndexEntry>) -> Self {
        Self { entries }
    }

    /// Load index entries for all library dependencies declared in a project manifest.
    pub fn from_project_manifest(manifest: &ProjectManifest) -> Self {
        let mut entries = HashMap::new();

        for (library_name, spec) in manifest.library_dependencies() {
            let entry = load_library_manifest_entry(library_name, &spec.path);
            entries.insert(library_name.clone(), entry);
        }

        Self { entries }
    }

    /// Return an indexed entry by dependency key.
    pub fn get(&self, library_name: &str) -> Option<&LibraryManifestIndexEntry> {
        self.entries.get(library_name)
    }

    /// Return all known dependency keys in deterministic order.
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
        for (key, registrations) in self.library_imported_vocab() {
            let mut ids = Vec::new();
            for registration in registrations {
                for keyword in registration.keywords {
                    if let Some(id) = incan_core::lang::keywords::from_str(&keyword.name)
                        && incan_core::lang::keywords::is_soft(id)
                    {
                        ids.push(id);
                    }
                }
            }
            ids.sort_by_key(|id| *id as u16);
            ids.dedup();
            if !ids.is_empty() {
                map.insert(key, ids);
            }
        }
        map
    }

    /// Return normalized imported-library keyword registrations keyed by dependency key.
    ///
    /// This is the primary consumer-side parser context for `pub::` imports and preserves the full vocab payload
    /// (`KeywordActivation`, `KeywordSpec`, placements, compound tokens, and decorator metadata).
    pub fn library_imported_vocab(&self) -> crate::frontend::parser::ImportedLibraryVocab {
        let mut map = HashMap::new();

        for (dependency_key, entry) in &self.entries {
            let LibraryManifestIndexEntry::Loaded { manifest, .. } = entry else {
                continue;
            };

            let Some(vocab) = &manifest.vocab else {
                continue;
            };

            let mut registrations = Vec::new();
            for registration in &vocab.keyword_registrations {
                if let Some(normalized) = normalize_keyword_registration(dependency_key, registration) {
                    registrations.push(normalized);
                }
            }

            if !registrations.is_empty() {
                map.insert(dependency_key.clone(), registrations);
            }
        }

        map
    }

    /// Return merged provider-required Cargo dependencies from all loaded library manifests.
    ///
    /// Requirements are merged by crate name. Equivalent specs are deduplicated; incompatible specs return
    /// [`ProviderRequirementError::DependencyConflict`].
    pub fn merged_provider_required_dependencies(&self) -> Result<Vec<DependencySpec>, ProviderRequirementError> {
        let mut merged: BTreeMap<String, (DependencySpec, String)> = BTreeMap::new();

        let mut keys: Vec<&String> = self.entries.keys().collect();
        keys.sort();

        for dependency_key in keys {
            let Some(entry) = self.entries.get(dependency_key) else {
                continue;
            };
            let LibraryManifestIndexEntry::Loaded { manifest, metadata } = entry else {
                continue;
            };
            let Some(vocab) = &manifest.vocab else {
                continue;
            };

            for dependency in &vocab.provider_manifest.required_dependencies {
                let spec = provider_dependency_to_spec(dependency, metadata).map_err(|message| {
                    ProviderRequirementError::InvalidDependency {
                        dependency_key: dependency_key.clone(),
                        message,
                    }
                })?;
                if let Some((existing, existing_provider)) = merged.get(&spec.crate_name) {
                    if existing != &spec {
                        return Err(ProviderRequirementError::DependencyConflict {
                            crate_name: spec.crate_name.clone(),
                            existing_provider: existing_provider.clone(),
                            incoming_provider: dependency_key.clone(),
                        });
                    }
                    continue;
                }
                merged.insert(spec.crate_name.clone(), (spec, dependency_key.clone()));
            }
        }

        Ok(merged.into_values().map(|(spec, _)| spec).collect())
    }

    /// Return merged provider-required stdlib feature names from all loaded library manifests.
    pub fn merged_provider_required_stdlib_features(&self) -> Vec<String> {
        let mut features = BTreeSet::new();
        let mut keys: Vec<&String> = self.entries.keys().collect();
        keys.sort();

        for dependency_key in keys {
            let Some(entry) = self.entries.get(dependency_key) else {
                continue;
            };
            let LibraryManifestIndexEntry::Loaded { manifest, .. } = entry else {
                continue;
            };
            let Some(vocab) = &manifest.vocab else {
                continue;
            };
            for feature in &vocab.provider_manifest.required_stdlib_features {
                let normalized = feature.trim();
                if !normalized.is_empty() {
                    features.insert(normalized.to_string());
                }
            }
        }

        features.into_iter().collect()
    }
}

fn provider_dependency_to_spec(
    dependency: &CargoDependency,
    metadata: &LibraryArtifactMetadata,
) -> Result<DependencySpec, String> {
    let crate_name = dependency.crate_name.trim();
    if crate_name.is_empty() {
        return Err("dependency crate_name cannot be empty".to_string());
    }

    let (source, version) = match &dependency.source {
        CargoDependencySource::Version(version) => {
            if version.trim().is_empty() {
                return Err(format!("dependency `{crate_name}` has an empty version requirement"));
            }
            (DependencySource::Registry, Some(version.clone()))
        }
        CargoDependencySource::Path(relative_or_absolute) => {
            if relative_or_absolute.trim().is_empty() {
                return Err(format!("dependency `{crate_name}` has an empty path source"));
            }
            let path_value = PathBuf::from(relative_or_absolute);
            let resolved_path = if path_value.is_absolute() {
                path_value
            } else {
                metadata.crate_root.join(path_value)
            };
            (DependencySource::Path { path: resolved_path }, None)
        }
        _ => {
            return Err(format!(
                "dependency `{crate_name}` uses unsupported source variant in this compiler version"
            ));
        }
    };

    Ok(DependencySpec {
        crate_name: crate_name.to_string(),
        version,
        features: Vec::new(),
        default_features: true,
        source,
        optional: false,
        package: None,
    }
    .normalized())
}

fn normalize_keyword_registration(
    dependency_key: &str,
    registration: &KeywordRegistration,
) -> Option<KeywordRegistration> {
    let mut keywords: Vec<KeywordSpec> = registration
        .keywords
        .iter()
        .filter(|keyword| !keyword.name.trim().is_empty())
        .cloned()
        .collect();
    if keywords.is_empty() {
        return None;
    }

    keywords.sort_by(|left, right| left.name.cmp(&right.name));
    keywords.dedup_by(|left, right| left.name == right.name && left.surface_kind == right.surface_kind);

    let activation = match &registration.activation {
        KeywordActivation::Always => KeywordActivation::Always,
        KeywordActivation::OnImport { namespace } => {
            let namespace = namespace.trim();
            if namespace.is_empty() {
                KeywordActivation::OnImport {
                    namespace: dependency_key.to_string(),
                }
            } else {
                KeywordActivation::OnImport {
                    namespace: namespace.to_string(),
                }
            }
        }
        _ => return None,
    };

    Some(KeywordRegistration {
        activation,
        keywords,
        valid_decorators: registration.valid_decorators.clone(),
    })
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
    if let Some(vocab) = &manifest.vocab
        && let Some(desugarer_artifact) = &vocab.desugarer_artifact
    {
        let artifact_path = crate_root.join(&desugarer_artifact.relative_path);
        if !artifact_path.is_file() {
            return Err(LibraryManifestLoadFailure {
                path: artifact_path,
                kind: LibraryManifestFailureKind::ArtifactMissing,
                message: "missing packaged vocab desugarer artifact".to_string(),
            });
        }
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

    #[test]
    fn exposes_imported_vocab_registrations_from_manifest_payload() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let consumer_manifest_path = tmp.path().join("incan.toml");
        let dep_root = tmp.path().join("deps").join("widgets-lib");
        let dep_artifact_root = dep_root.join("target").join("lib");
        std::fs::create_dir_all(dep_artifact_root.join("src"))?;

        let mut manifest = LibraryManifest::new("widgets_core", "0.1.0");
        manifest.vocab = Some(crate::library_manifest::VocabExports {
            crate_path: "vocab_companion".to_string(),
            package_name: "widgets_vocab_companion".to_string(),
            keyword_registrations: vec![incan_vocab::KeywordRegistration {
                activation: incan_vocab::KeywordActivation::OnImport {
                    namespace: "widgets.dsl".to_string(),
                },
                keywords: vec![incan_vocab::KeywordSpec {
                    name: "async".to_string(),
                    surface_kind: incan_vocab::KeywordSurfaceKind::FunctionDecl,
                    compound_tokens: vec!["def".to_string()],
                    placement: incan_vocab::KeywordPlacement::TopLevel,
                }],
                valid_decorators: vec!["route".to_string()],
            }],
            provider_manifest: incan_vocab::LibraryManifest::default(),
            desugarer_artifact: None,
        });
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
        let imported = index.library_imported_vocab();
        let regs = imported
            .get("widgets")
            .ok_or("missing imported vocab for dependency key")?;
        assert_eq!(regs.len(), 1);
        assert_eq!(regs[0].keywords.len(), 1);
        assert_eq!(regs[0].keywords[0].name, "async");
        assert_eq!(
            regs[0].keywords[0].surface_kind,
            incan_vocab::KeywordSurfaceKind::FunctionDecl
        );
        assert_eq!(regs[0].valid_decorators, vec!["route".to_string()]);

        Ok(())
    }

    #[test]
    fn characterization_records_failure_for_missing_packaged_vocab_desugarer_artifact()
    -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let consumer_manifest_path = tmp.path().join("incan.toml");
        let dep_root = tmp.path().join("deps").join("routes-lib");
        let dep_artifact_root = dep_root.join("target").join("lib");
        std::fs::create_dir_all(dep_artifact_root.join("src"))?;

        let mut manifest = LibraryManifest::new("routes_core", "0.1.0");
        manifest.vocab = Some(crate::library_manifest::VocabExports {
            crate_path: "vocab_companion".to_string(),
            package_name: "routes_vocab_companion".to_string(),
            keyword_registrations: Vec::new(),
            provider_manifest: incan_vocab::LibraryManifest::default(),
            desugarer_artifact: Some(crate::library_manifest::VocabDesugarerArtifact {
                artifact_kind: incan_vocab::DesugarerArtifactKind::WasmModule,
                relative_path: "desugarers/routes_desugarer.wasm".to_string(),
                target: "wasm32-wasip1".to_string(),
                profile: "release".to_string(),
                entrypoint: "desugar_block".to_string(),
                sha256: "deadbeef".to_string(),
            }),
        });
        manifest.write_to_path(&dep_artifact_root.join("routes_core.incnlib"))?;
        std::fs::write(
            dep_artifact_root.join("Cargo.toml"),
            "[package]\nname = \"routes_core\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )?;
        std::fs::write(dep_artifact_root.join("src/lib.rs"), "pub fn routes() {}\n")?;

        let manifest_content = r#"
[dependencies]
routes = { path = "deps/routes-lib" }
"#;
        std::fs::write(&consumer_manifest_path, manifest_content)?;
        let parsed = ProjectManifest::from_str(manifest_content, &consumer_manifest_path)?;

        let index = LibraryManifestIndex::from_project_manifest(&parsed);
        let entry = index.get("routes").ok_or("missing routes entry")?;
        match entry {
            LibraryManifestIndexEntry::Loaded { .. } => {
                return Err("expected failed entry for missing packaged desugarer artifact".into());
            }
            LibraryManifestIndexEntry::Failed(failure) => {
                assert_eq!(failure.kind, LibraryManifestFailureKind::ArtifactMissing);
                assert!(
                    failure.message.contains("missing packaged vocab desugarer artifact"),
                    "unexpected failure: {}",
                    failure.message
                );
            }
        }

        Ok(())
    }

    #[test]
    fn merges_provider_required_dependencies_and_stdlib_features() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let consumer_manifest_path = tmp.path().join("incan.toml");
        let dep_a_root = tmp.path().join("deps").join("widgets-lib");
        let dep_a_artifact_root = dep_a_root.join("target").join("lib");
        let dep_b_root = tmp.path().join("deps").join("analytics-lib");
        let dep_b_artifact_root = dep_b_root.join("target").join("lib");
        std::fs::create_dir_all(dep_a_artifact_root.join("src"))?;
        std::fs::create_dir_all(dep_b_artifact_root.join("src"))?;

        let mut dep_a_manifest = LibraryManifest::new("widgets_core", "0.1.0");
        dep_a_manifest.vocab = Some(crate::library_manifest::VocabExports {
            crate_path: "widgets_vocab_companion".to_string(),
            package_name: "widgets_vocab_companion".to_string(),
            keyword_registrations: Vec::new(),
            provider_manifest: incan_vocab::LibraryManifest {
                required_dependencies: vec![incan_vocab::CargoDependency {
                    crate_name: "serde_json".to_string(),
                    source: incan_vocab::CargoDependencySource::Version("1.0".to_string()),
                }],
                required_stdlib_features: vec!["json".to_string()],
                ..incan_vocab::LibraryManifest::default()
            },
            desugarer_artifact: None,
        });
        dep_a_manifest.write_to_path(&dep_a_artifact_root.join("widgets_core.incnlib"))?;
        std::fs::write(
            dep_a_artifact_root.join("Cargo.toml"),
            "[package]\nname = \"widgets_core\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )?;
        std::fs::write(dep_a_artifact_root.join("src/lib.rs"), "pub fn widgets() {}\n")?;

        let mut dep_b_manifest = LibraryManifest::new("analytics_core", "0.1.0");
        dep_b_manifest.vocab = Some(crate::library_manifest::VocabExports {
            crate_path: "analytics_vocab_companion".to_string(),
            package_name: "analytics_vocab_companion".to_string(),
            keyword_registrations: Vec::new(),
            provider_manifest: incan_vocab::LibraryManifest {
                required_dependencies: vec![incan_vocab::CargoDependency {
                    crate_name: "serde_json".to_string(),
                    source: incan_vocab::CargoDependencySource::Version("1.0".to_string()),
                }],
                required_stdlib_features: vec!["json".to_string(), "async".to_string()],
                ..incan_vocab::LibraryManifest::default()
            },
            desugarer_artifact: None,
        });
        dep_b_manifest.write_to_path(&dep_b_artifact_root.join("analytics_core.incnlib"))?;
        std::fs::write(
            dep_b_artifact_root.join("Cargo.toml"),
            "[package]\nname = \"analytics_core\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )?;
        std::fs::write(dep_b_artifact_root.join("src/lib.rs"), "pub fn analytics() {}\n")?;

        let manifest_content = r#"
[dependencies]
widgets = { path = "deps/widgets-lib" }
analytics = { path = "deps/analytics-lib" }
"#;
        std::fs::write(&consumer_manifest_path, manifest_content)?;
        let parsed = ProjectManifest::from_str(manifest_content, &consumer_manifest_path)?;
        let index = LibraryManifestIndex::from_project_manifest(&parsed);

        let deps = index.merged_provider_required_dependencies()?;
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].crate_name, "serde_json");
        assert_eq!(deps[0].version.as_deref(), Some("1.0"));

        let features = index.merged_provider_required_stdlib_features();
        assert_eq!(features, vec!["async".to_string(), "json".to_string()]);
        Ok(())
    }

    #[test]
    fn reports_provider_dependency_conflict() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let consumer_manifest_path = tmp.path().join("incan.toml");
        let dep_a_root = tmp.path().join("deps").join("widgets-lib");
        let dep_a_artifact_root = dep_a_root.join("target").join("lib");
        let dep_b_root = tmp.path().join("deps").join("analytics-lib");
        let dep_b_artifact_root = dep_b_root.join("target").join("lib");
        std::fs::create_dir_all(dep_a_artifact_root.join("src"))?;
        std::fs::create_dir_all(dep_b_artifact_root.join("src"))?;

        let mut dep_a_manifest = LibraryManifest::new("widgets_core", "0.1.0");
        dep_a_manifest.vocab = Some(crate::library_manifest::VocabExports {
            crate_path: "widgets_vocab_companion".to_string(),
            package_name: "widgets_vocab_companion".to_string(),
            keyword_registrations: Vec::new(),
            provider_manifest: incan_vocab::LibraryManifest {
                required_dependencies: vec![incan_vocab::CargoDependency {
                    crate_name: "serde_json".to_string(),
                    source: incan_vocab::CargoDependencySource::Version("1.0".to_string()),
                }],
                ..incan_vocab::LibraryManifest::default()
            },
            desugarer_artifact: None,
        });
        dep_a_manifest.write_to_path(&dep_a_artifact_root.join("widgets_core.incnlib"))?;
        std::fs::write(
            dep_a_artifact_root.join("Cargo.toml"),
            "[package]\nname = \"widgets_core\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )?;
        std::fs::write(dep_a_artifact_root.join("src/lib.rs"), "pub fn widgets() {}\n")?;

        let mut dep_b_manifest = LibraryManifest::new("analytics_core", "0.1.0");
        dep_b_manifest.vocab = Some(crate::library_manifest::VocabExports {
            crate_path: "analytics_vocab_companion".to_string(),
            package_name: "analytics_vocab_companion".to_string(),
            keyword_registrations: Vec::new(),
            provider_manifest: incan_vocab::LibraryManifest {
                required_dependencies: vec![incan_vocab::CargoDependency {
                    crate_name: "serde_json".to_string(),
                    source: incan_vocab::CargoDependencySource::Version("2.0".to_string()),
                }],
                ..incan_vocab::LibraryManifest::default()
            },
            desugarer_artifact: None,
        });
        dep_b_manifest.write_to_path(&dep_b_artifact_root.join("analytics_core.incnlib"))?;
        std::fs::write(
            dep_b_artifact_root.join("Cargo.toml"),
            "[package]\nname = \"analytics_core\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )?;
        std::fs::write(dep_b_artifact_root.join("src/lib.rs"), "pub fn analytics() {}\n")?;

        let manifest_content = r#"
[dependencies]
widgets = { path = "deps/widgets-lib" }
analytics = { path = "deps/analytics-lib" }
"#;
        std::fs::write(&consumer_manifest_path, manifest_content)?;
        let parsed = ProjectManifest::from_str(manifest_content, &consumer_manifest_path)?;
        let index = LibraryManifestIndex::from_project_manifest(&parsed);

        let error = index
            .merged_provider_required_dependencies()
            .expect_err("expected conflicting provider dependency requirements");
        assert!(
            matches!(error, ProviderRequirementError::DependencyConflict { ref crate_name, .. } if crate_name == "serde_json"),
            "unexpected error: {error}"
        );
        Ok(())
    }
}
