//! Consumer-side index of dependency library manifests (`.incnlib`).
//!
//! Phase 3 of RFC 031 resolves `pub::` imports from dependency manifests rather than reparsing library source.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::library_manifest::{LibraryManifest, LibraryManifestError};
use crate::manifest::ProjectManifest;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LibraryManifestFailureKind {
    Read,
    Parse,
    Invalid,
}

#[derive(Debug, Clone)]
pub struct LibraryManifestLoadFailure {
    pub path: PathBuf,
    pub kind: LibraryManifestFailureKind,
    pub message: String,
}

#[derive(Debug, Clone)]
pub enum LibraryManifestIndexEntry {
    Loaded { path: PathBuf, manifest: Box<LibraryManifest> },
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
            let manifest_path = dependency_manifest_path(&spec.path, library_name);
            let entry = match LibraryManifest::read_from_path(&manifest_path) {
                Ok(loaded) => LibraryManifestIndexEntry::Loaded {
                    path: manifest_path,
                    manifest: Box::new(loaded),
                },
                Err(error) => {
                    let failure = LibraryManifestLoadFailure::from_error(manifest_path, error);
                    LibraryManifestIndexEntry::Failed(failure)
                }
            };
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
}

fn dependency_manifest_path(dependency_root: &Path, library_name: &str) -> PathBuf {
    dependency_root
        .join("target")
        .join("lib")
        .join(format!("{library_name}.incnlib"))
}

impl LibraryManifestLoadFailure {
    fn from_error(path: PathBuf, error: LibraryManifestError) -> Self {
        let kind = match &error {
            LibraryManifestError::Read { .. } | LibraryManifestError::Write { .. } => LibraryManifestFailureKind::Read,
            LibraryManifestError::Parse(_) | LibraryManifestError::Serialize(_) => LibraryManifestFailureKind::Parse,
            LibraryManifestError::Invalid(_) => LibraryManifestFailureKind::Invalid,
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
        let dep_manifest_path = dep_root.join("target").join("lib").join("mylib.incnlib");

        std::fs::create_dir_all(dep_manifest_path.parent().ok_or("missing dep manifest parent path")?)?;
        let manifest = LibraryManifest::new("mylib", "0.1.0");
        manifest.write_to_path(&dep_manifest_path)?;

        let manifest_content = r#"
[dependencies]
mylib = { path = "deps/mylib" }
"#;
        std::fs::write(&consumer_manifest_path, manifest_content)?;
        let parsed = ProjectManifest::from_str(manifest_content, &consumer_manifest_path)?;

        let index = LibraryManifestIndex::from_project_manifest(&parsed);
        let entry = index.get("mylib").ok_or("missing mylib index entry")?;
        match entry {
            LibraryManifestIndexEntry::Loaded { manifest, .. } => {
                assert_eq!(manifest.name, "mylib");
                assert_eq!(manifest.version, "0.1.0");
            }
            LibraryManifestIndexEntry::Failed(failure) => {
                return Err(format!("expected loaded manifest, got failure: {}", failure.message).into());
            }
        }

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
                assert_eq!(failure.kind, LibraryManifestFailureKind::Read);
                assert!(
                    failure.message.contains("failed to read"),
                    "unexpected failure: {}",
                    failure.message
                );
            }
        }

        Ok(())
    }
}
