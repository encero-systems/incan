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

fn canonical_path_aliases(canonical_path: &str) -> Vec<String> {
    let mut aliases = Vec::new();

    for (prefix, replacement) in [
        ("std::option::", "core::option::"),
        ("std::result::", "core::result::"),
        ("std::string::", "alloc::string::"),
        ("std::vec::", "alloc::vec::"),
        ("std::boxed::", "alloc::boxed::"),
    ] {
        if let Some(rest) = canonical_path.strip_prefix(prefix) {
            aliases.push(format!("{replacement}{rest}"));
        }
    }

    if canonical_path == "std::collections::HashMap" {
        aliases.push("hashbrown::HashMap".to_string());
    } else if let Some(rest) = canonical_path.strip_prefix("std::collections::HashMap::") {
        aliases.push(format!("hashbrown::HashMap::{rest}"));
    }

    aliases
}

fn canonical_path_candidates(canonical_path: &str) -> Vec<String> {
    let aliases = canonical_path_aliases(canonical_path);
    if canonical_path.starts_with("std::") && !aliases.is_empty() {
        aliases
            .into_iter()
            .chain(std::iter::once(canonical_path.to_string()))
            .collect()
    } else {
        std::iter::once(canonical_path.to_string()).chain(aliases).collect()
    }
}

fn crate_name_for_path(canonical_path: &str) -> &str {
    canonical_path.split("::").next().unwrap_or(canonical_path)
}

fn extract_in_workspace_set(
    inner: &mut CacheInner,
    root: &Path,
    canonical_path: &str,
    progress: &(dyn Fn(String) + Sync),
) -> Result<RustItemMetadata, RustMetadataError> {
    let workspace = match inner.workspaces.entry((root.to_path_buf(), false)) {
        Entry::Occupied(o) => o.into_mut(),
        Entry::Vacant(v) => v.insert(RustWorkspace::load(root, progress)?),
    };
    match extract_rust_item(workspace.db(), canonical_path) {
        Ok(meta) => return Ok(meta),
        Err(RustMetadataError::CrateNotFound(_)) | Err(RustMetadataError::PathNotResolved(_)) => {}
        Err(err) => return Err(err),
    }

    let root_outdir_workspace = match inner.workspaces.entry((root.to_path_buf(), true)) {
        Entry::Occupied(o) => o.into_mut(),
        Entry::Vacant(v) => v.insert(RustWorkspace::load_with_options(root, progress, true)?),
    };
    match extract_rust_item(root_outdir_workspace.db(), canonical_path) {
        Ok(meta) => return Ok(meta),
        Err(RustMetadataError::CrateNotFound(_)) | Err(RustMetadataError::PathNotResolved(_)) => {}
        Err(err) => return Err(err),
    }

    let crate_name = crate_name_for_path(canonical_path);
    if let Some(dep_root) = dependency_manifest_dir_for_crate(root, crate_name) {
        let dep_workspace = match inner.workspaces.entry((dep_root.clone(), true)) {
            Entry::Occupied(o) => o.into_mut(),
            Entry::Vacant(v) => v.insert(RustWorkspace::load_with_options(&dep_root, progress, true)?),
        };
        return extract_rust_item(dep_workspace.db(), canonical_path);
    }

    Err(RustMetadataError::CrateNotFound(crate_name.to_string()))
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

        let mut last_err = None;
        let mut meta = None;
        for candidate in canonical_path_candidates(canonical_path) {
            match extract_in_workspace_set(&mut inner, &root, candidate.as_str(), progress) {
                Ok(found) => {
                    meta = Some(found);
                    break;
                }
                Err(err) => last_err = Some(err),
            }
        }
        let mut meta = meta.ok_or_else(|| {
            last_err
                .unwrap_or_else(|| RustMetadataError::CrateNotFound(crate_name_for_path(canonical_path).to_string()))
        })?;
        meta.canonical_path = canonical_path.to_owned();
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
