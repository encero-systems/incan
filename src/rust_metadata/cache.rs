//! In-memory cache: one loaded workspace per manifest directory, plus per-item metadata.

use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};

use incan_core::interop::RustItemMetadata;
use serde::Deserialize;

use super::error::RustMetadataError;
use super::extractor::extract_rust_item;
use super::loader::RustWorkspace;

/// Cache for [`RustWorkspace`] instances and extracted [`RustItemMetadata`].
///
/// The workspace is loaded at most once per canonical manifest directory; item metadata is stored per `(workspace_root,
/// canonical_path)` and reused without re-querying salsa.
///
/// The entire cache is protected by one mutex so `RustWorkspace` (which is not `Sync` because of the retained `Vfs`)
/// never has to live inside `Arc` for cross-thread sharing.
pub struct RustMetadataCache {
    inner: Mutex<CacheInner>,
}

#[derive(Default)]
struct CacheInner {
    workspaces: HashMap<(PathBuf, bool), RustWorkspace>,
    items: HashMap<(PathBuf, String), Arc<RustItemMetadata>>,
}

#[derive(Deserialize)]
struct CargoMetadata {
    packages: Vec<CargoPackage>,
}

#[derive(Deserialize)]
struct CargoPackage {
    name: String,
    manifest_path: PathBuf,
    targets: Vec<CargoTarget>,
}

#[derive(Deserialize)]
struct CargoTarget {
    name: String,
}

fn normalize_crate_name(name: &str) -> String {
    name.replace('-', "_")
}

fn dependency_manifest_dir_for_crate(root: &Path, crate_name: &str) -> Option<PathBuf> {
    let manifest_path = root.join("Cargo.toml");
    let output = Command::new("cargo")
        .arg("metadata")
        .arg("--offline")
        .arg("--manifest-path")
        .arg(manifest_path.as_os_str())
        .arg("--format-version")
        .arg("1")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let parsed: CargoMetadata = serde_json::from_slice(&output.stdout).ok()?;
    let normalized = normalize_crate_name(crate_name);
    parsed
        .packages
        .into_iter()
        .find(|pkg| {
            normalize_crate_name(pkg.name.as_str()) == normalized
                || pkg
                    .targets
                    .iter()
                    .any(|target| normalize_crate_name(target.name.as_str()) == normalized)
        })
        .and_then(|pkg| pkg.manifest_path.parent().map(Path::to_path_buf))
}

impl RustMetadataCache {
    /// Create an empty cache.
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(CacheInner::default()),
        }
    }

    /// Return metadata for `canonical_path`, loading `manifest_dir` on first use.
    pub fn get_or_extract(
        &self,
        manifest_dir: &Path,
        canonical_path: &str,
        progress: &(dyn Fn(String) + Sync),
    ) -> Result<Arc<RustItemMetadata>, RustMetadataError> {
        let root = manifest_dir.canonicalize()?;
        let key_item = (root.clone(), canonical_path.to_owned());

        let mut inner = self.inner.lock().map_err(|e| RustMetadataError::LoadWorkspace {
            path: root.clone(),
            message: format!("metadata cache lock poisoned: {e}"),
        })?;

        if let Some(hit) = inner.items.get(&key_item) {
            return Ok(Arc::clone(hit));
        }

        let workspace = match inner.workspaces.entry((root.clone(), false)) {
            Entry::Occupied(o) => o.into_mut(),
            Entry::Vacant(v) => v.insert(RustWorkspace::load(&root, progress)?),
        };

        let meta = match extract_rust_item(workspace.db(), canonical_path) {
            Ok(meta) => meta,
            Err(RustMetadataError::CrateNotFound(crate_name)) => {
                let root_outdir_workspace = match inner.workspaces.entry((root.clone(), true)) {
                    Entry::Occupied(o) => o.into_mut(),
                    Entry::Vacant(v) => v.insert(RustWorkspace::load_with_options(&root, progress, true)?),
                };
                if let Ok(meta) = extract_rust_item(root_outdir_workspace.db(), canonical_path) {
                    meta
                } else {
                    let Some(dep_root) = dependency_manifest_dir_for_crate(&root, crate_name.as_str()) else {
                        return Err(RustMetadataError::CrateNotFound(crate_name));
                    };
                    let dep_workspace = match inner.workspaces.entry((dep_root.clone(), true)) {
                        Entry::Occupied(o) => o.into_mut(),
                        Entry::Vacant(v) => v.insert(RustWorkspace::load_with_options(&dep_root, progress, true)?),
                    };
                    extract_rust_item(dep_workspace.db(), canonical_path)?
                }
            }
            Err(err) => return Err(err),
        };
        let arc = Arc::new(meta);
        inner.items.insert(key_item, Arc::clone(&arc));
        Ok(arc)
    }

    /// Seed metadata directly for tests without invoking rust-analyzer extraction.
    #[cfg(test)]
    pub(crate) fn insert_test_item(
        &self,
        manifest_dir: &Path,
        metadata: RustItemMetadata,
    ) -> Result<(), RustMetadataError> {
        let root = manifest_dir.canonicalize()?;
        let key_item = (root, metadata.canonical_path.clone());
        let mut inner = self.inner.lock().map_err(|e| RustMetadataError::LoadWorkspace {
            path: manifest_dir.to_path_buf(),
            message: format!("metadata cache lock poisoned: {e}"),
        })?;
        inner.items.insert(key_item, Arc::new(metadata));
        Ok(())
    }
}

impl Default for RustMetadataCache {
    fn default() -> Self {
        Self::new()
    }
}
