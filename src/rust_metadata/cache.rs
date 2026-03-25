//! In-memory cache: one loaded workspace per manifest directory, plus per-item metadata.

use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use incan_core::interop::RustItemMetadata;

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

struct CacheInner {
    workspaces: HashMap<PathBuf, RustWorkspace>,
    items: HashMap<(PathBuf, String), Arc<RustItemMetadata>>,
}

impl Default for CacheInner {
    fn default() -> Self {
        Self {
            workspaces: HashMap::new(),
            items: HashMap::new(),
        }
    }
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

        let workspace = match inner.workspaces.entry(root.clone()) {
            Entry::Occupied(o) => o.into_mut(),
            Entry::Vacant(v) => v.insert(RustWorkspace::load(&root, progress)?),
        };

        let meta = extract_rust_item(workspace.db(), canonical_path)?;
        let arc = Arc::new(meta);
        inner.items.insert(key_item, Arc::clone(&arc));
        Ok(arc)
    }
}
