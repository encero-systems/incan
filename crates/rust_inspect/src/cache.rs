//! Metadata cache bound to a validated selected inspection projection.
//!
//! Preparation binds validated inputs; cache hits do not require a database. Semantic misses load it on demand.
//! A path or source catalog cannot establish selection, and no cache miss triggers ambient source discovery.

use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
#[cfg(not(test))]
use std::sync::OnceLock;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use incan_core::interop::{RustItemKind, RustItemMetadata, RustTypeInfo, RustTypeMetadataCompleteness};
use serde::{Deserialize, Serialize};

use crate::ValidatedInspectionProject;
use crate::cache_resolve::crate_name_for_path;
use crate::cache_timing::{CallTrace, log_timing_stage, rust_inspect_timing_enabled};
use crate::error::RustMetadataError;
use crate::extractor::{extract_rust_item, rust_type_implements_trait};
use crate::loader::RustWorkspace;

/// Cache for [`RustWorkspace`] instances and extracted [`RustItemMetadata`].
///
/// The workspace is loaded at most once per canonical manifest directory; item metadata is stored per `(workspace_root,
/// canonical_path)` and reused without re-querying salsa.
///
/// This type is internal plumbing for the toolchain-locked inspection subsystem. Its persistence format and negative
/// lookup behavior are implementation details unless promoted through the crate-level API.
///
/// The entire cache is protected by one mutex so `RustWorkspace` (which is not `Sync` because of the retained `Vfs`)
/// never has to live inside `Arc` for cross-thread sharing.
pub struct RustMetadataCache {
    inner: Arc<Mutex<CacheInner>>,
}

#[derive(Default)]
struct CacheInner {
    selections: HashMap<PathBuf, InspectionContext>,
    workspaces: HashMap<PathBuf, RustWorkspace>,
    items: HashMap<(PathBuf, String), Arc<RustItemMetadata>>,
    fast_failed_items: HashSet<(PathBuf, String)>,
    /// Items whose record was produced by complete semantic extraction rather than a source-only fallback.
    complete_items: HashSet<(PathBuf, String)>,
    failed_items: HashMap<(PathBuf, String), NegativeLookup>,
    disk_cache_state: HashMap<PathBuf, DiskCacheState>,
}

/// Validated selection and an explicit output location; neither requires a loaded analysis database.
struct InspectionContext {
    projection: ValidatedInspectionProject,
    temporary_root: PathBuf,
}

#[derive(Default)]
struct DiskCacheState {
    loaded: bool,
    workspace_fingerprint: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DiskCacheLoadReport {
    reason: &'static str,
    items: usize,
    misses: usize,
}

impl DiskCacheLoadReport {
    /// Return a compact timing-detail string for `INCAN_RUST_INSPECT_TIMING` output.
    fn detail(self) -> String {
        format!(
            "reason={} cached_items={} cached_misses={}",
            self.reason, self.items, self.misses
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
enum NegativeLookup {
    CrateNotFound(String),
    PathNotResolved(String),
    UnsupportedMacro(String),
}

impl NegativeLookup {
    /// Only stable item misses may enter the negative cache; selection and operational errors remain terminal.
    fn from_error(err: &RustMetadataError) -> Option<Self> {
        match err {
            RustMetadataError::CrateNotFound(path) => Some(Self::CrateNotFound(path.clone())),
            RustMetadataError::PathNotResolved(path) => Some(Self::PathNotResolved(path.clone())),
            RustMetadataError::UnsupportedMacro(path) => Some(Self::UnsupportedMacro(path.clone())),
            RustMetadataError::Io(_)
            | RustMetadataError::LoadWorkspace { .. }
            | RustMetadataError::SelectedInputUnavailable { .. }
            | RustMetadataError::InvalidSelectedInput { .. }
            | RustMetadataError::UnsupportedSelectedOperation { .. } => None,
        }
    }

    fn to_error(&self) -> RustMetadataError {
        match self {
            Self::CrateNotFound(path) => RustMetadataError::CrateNotFound(path.clone()),
            Self::PathNotResolved(path) => RustMetadataError::PathNotResolved(path.clone()),
            Self::UnsupportedMacro(path) => RustMetadataError::UnsupportedMacro(path.clone()),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DiskCacheEnvelope {
    cache_format: u32,
    #[serde(alias = "incan_version")]
    inspector_version: String,
    workspace_fingerprint: String,
    items: HashMap<String, RustItemMetadata>,
    /// Non-type records verified by complete semantic extraction.
    ///
    /// Type records carry their own completeness marker. Functions, traits, constants, and supported modules do not,
    /// so this set prevents a source-only record from satisfying a complete lookup after process restart.
    #[serde(default)]
    complete_items: HashSet<String>,
    #[serde(default)]
    misses: HashMap<String, NegativeLookup>,
}

// Bump when extracted metadata semantics change in a way that makes previously persisted items unsafe to reuse.
const DISK_CACHE_FORMAT: u32 = 39;
const DISK_CACHE_FILE: &str = ".incan_rust_inspect_cache.json";
// Backward-compatibility read path for caches written before the crate/module rename.
const LEGACY_DISK_CACHE_FILE: &str = ".incan_rust_metadata_cache.json";

/// Canonical on-disk cache path for a generated lock workspace.
fn disk_cache_path(root: &Path) -> PathBuf {
    root.join(DISK_CACHE_FILE)
}

/// Legacy on-disk cache path kept for backward-compatible reads.
fn legacy_disk_cache_path(root: &Path) -> PathBuf {
    root.join(LEGACY_DISK_CACHE_FILE)
}

/// Require validated selected bindings before cache reuse, without constructing an analysis database.
fn workspace_fingerprint(inner: &CacheInner, root: &Path) -> Result<String, RustMetadataError> {
    inner
        .selections
        .get(root)
        .map(|selection| selection.projection.fingerprint().to_string())
        .ok_or_else(|| RustMetadataError::SelectedInputUnavailable {
            path: root.to_path_buf(),
        })
}

/// Read a cache envelope without treating absent/corrupt cache bytes as selection authority.
fn read_json_cache(path: &Path) -> Result<Option<DiskCacheEnvelope>, RustMetadataError> {
    let payload = match fs::read_to_string(path) {
        Ok(payload) => payload,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err.into()),
    };
    match serde_json::from_str::<DiskCacheEnvelope>(&payload) {
        Ok(envelope) => Ok(Some(envelope)),
        Err(err) => {
            tracing::warn!(
                path = %path.display(),
                error = %err,
                "ignoring unreadable rust-inspect disk cache (treated as cache miss)"
            );
            if rust_inspect_timing_enabled() {
                eprintln!(
                    "[rust-inspect-timing] disk_cache.parse_error path={} err={err}",
                    path.display()
                );
            }
            Ok(None)
        }
    }
}

/// Load the current disk cache file, then transparently fall back to the legacy filename.
fn read_disk_cache(root: &Path) -> Result<Option<DiskCacheEnvelope>, RustMetadataError> {
    let cache_path = disk_cache_path(root);
    if let Some(envelope) = read_json_cache(&cache_path)? {
        return Ok(Some(envelope));
    }
    read_json_cache(&legacy_disk_cache_path(root))
}

/// Atomically write one cache envelope to disk.
fn write_json_cache(path: &Path, envelope: &DiskCacheEnvelope) -> Result<(), RustMetadataError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp_path = path.with_extension("tmp");
    let payload = serde_json::to_vec_pretty(envelope).map_err(|err| RustMetadataError::LoadWorkspace {
        path: path.to_path_buf(),
        message: format!("failed to serialize rust-inspect disk cache: {err}"),
    })?;
    fs::write(&tmp_path, payload)?;
    fs::rename(tmp_path, path)?;
    Ok(())
}

/// Persist the current workspace cache snapshot to disk.
fn write_disk_cache(root: &Path, envelope: &DiskCacheEnvelope) -> Result<(), RustMetadataError> {
    let cache_path = disk_cache_path(root);
    write_json_cache(&cache_path, envelope)
}

/// Load valid disk-cache items into memory for one workspace and explain whether the disk state was reusable.
fn load_disk_cache_into_memory(
    inner: &mut CacheInner,
    root: &Path,
) -> Result<(Option<String>, DiskCacheLoadReport), RustMetadataError> {
    let fingerprint = workspace_fingerprint(inner, root)?;
    let Some(envelope) = read_disk_cache(root)? else {
        return Ok((
            Some(fingerprint),
            DiskCacheLoadReport {
                reason: "miss.cache_file_absent",
                items: 0,
                misses: 0,
            },
        ));
    };
    if envelope.cache_format != DISK_CACHE_FORMAT {
        return Ok((
            Some(fingerprint),
            DiskCacheLoadReport {
                reason: "miss.cache_format_changed",
                items: envelope.items.len(),
                misses: envelope.misses.len(),
            },
        ));
    }
    if envelope.workspace_fingerprint != fingerprint {
        return Ok((
            Some(fingerprint),
            DiskCacheLoadReport {
                reason: "miss.workspace_fingerprint_changed",
                items: envelope.items.len(),
                misses: envelope.misses.len(),
            },
        ));
    }
    let report = DiskCacheLoadReport {
        reason: "hit.disk",
        items: envelope.items.len(),
        misses: envelope.misses.len(),
    };
    let complete_items = envelope.complete_items;
    for (canonical_path, metadata) in envelope.items {
        let mut metadata = metadata;
        metadata.canonical_path = canonical_path;
        insert_cached_item(inner, root, Arc::new(metadata));
    }
    for canonical_path in complete_items {
        let key = (root.to_path_buf(), canonical_path);
        if inner.items.contains_key(&key) {
            inner.complete_items.insert(key);
        }
    }
    for (canonical_path, miss) in envelope.misses {
        inner.failed_items.insert((root.to_path_buf(), canonical_path), miss);
    }
    Ok((Some(fingerprint), report))
}

/// Ensure the workspace-local disk cache has been loaded once for this process.
fn ensure_disk_cache_loaded(inner: &mut CacheInner, root: &Path) -> Result<DiskCacheLoadReport, RustMetadataError> {
    workspace_fingerprint(inner, root)?;
    if inner.disk_cache_state.get(root).is_some_and(|state| state.loaded) {
        let items = inner
            .items
            .keys()
            .filter(|(workspace_root, _)| workspace_root == root)
            .count();
        let misses = inner
            .failed_items
            .keys()
            .filter(|(workspace_root, _)| workspace_root == root)
            .count();
        return Ok(DiskCacheLoadReport {
            reason: "hit.process_loaded",
            items,
            misses,
        });
    }
    let (fingerprint, report) = load_disk_cache_into_memory(inner, root)?;
    let state = inner.disk_cache_state.entry(root.to_path_buf()).or_default();
    state.workspace_fingerprint = fingerprint;
    state.loaded = true;
    Ok(report)
}

/// Build the current workspace-local disk cache snapshot.
///
/// Reuses the loaded projection's complete physical binding. A former Cargo workspace fingerprint can never
/// authorize this cache format; the validated selected projection must be bound before persistence or lookup.
fn disk_cache_envelope(inner: &CacheInner, root: &Path) -> Result<DiskCacheEnvelope, RustMetadataError> {
    let fingerprint = match inner
        .disk_cache_state
        .get(root)
        .and_then(|state| state.workspace_fingerprint.clone())
    {
        Some(fingerprint) => fingerprint,
        None => workspace_fingerprint(inner, root)?,
    };
    let mut items = HashMap::new();
    let mut misses = HashMap::new();
    for ((item_root, canonical_path), cached) in &inner.items {
        if item_root == root {
            items.insert(canonical_path.clone(), (*cached.as_ref()).clone());
        }
    }
    let complete_items = inner
        .complete_items
        .iter()
        .filter(|(item_root, _)| item_root == root)
        .map(|(_, canonical_path)| canonical_path.clone())
        .collect();
    for ((item_root, canonical_path), miss) in &inner.failed_items {
        if item_root == root {
            misses.insert(canonical_path.clone(), miss.clone());
        }
    }
    Ok(DiskCacheEnvelope {
        cache_format: DISK_CACHE_FORMAT,
        inspector_version: format!("cache-format-{DISK_CACHE_FORMAT}"),
        workspace_fingerprint: fingerprint,
        items,
        complete_items,
        misses,
    })
}

/// Persist the complete workspace-local disk cache snapshot.
fn persist_manifest_dir_to_disk_cache(inner: &CacheInner, root: &Path) -> Result<(), RustMetadataError> {
    let envelope = disk_cache_envelope(inner, root)?;
    write_disk_cache(root, &envelope)
}

/// Persist the workspace-local disk cache snapshot after an item update.
fn persist_item_to_disk_cache(inner: &CacheInner, root: &Path) -> Result<(), RustMetadataError> {
    persist_manifest_dir_to_disk_cache(inner, root)
}

/// Persist the workspace-local disk cache snapshot after a stable miss.
fn persist_negative_to_disk_cache(inner: &CacheInner, root: &Path) -> Result<(), RustMetadataError> {
    persist_manifest_dir_to_disk_cache(inner, root)
}

#[derive(Debug, Clone)]
pub struct CacheLookupHit {
    pub metadata: Arc<RustItemMetadata>,
    pub alias_used: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CacheAccessOutcome {
    ExactHit,
    AliasHit,
    Extracted,
}

impl CacheAccessOutcome {
    /// Return true when an access reused existing cache state rather than extracting metadata.
    pub(crate) fn reused(self) -> bool {
        matches!(self, Self::ExactHit | Self::AliasHit)
    }

    /// Return the stable timing-trace label for this cache access outcome.
    fn trace_label(self) -> &'static str {
        match self {
            Self::ExactHit => "hit.memory.exact",
            Self::AliasHit => "hit.memory.alias",
            Self::Extracted => "hit.extracted",
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct CacheAccess {
    pub metadata: Arc<RustItemMetadata>,
    pub outcome: CacheAccessOutcome,
}

/// Normalize raw identifier spelling within one selected query binding.
///
/// Crate aliases, public reexports and std/core/alloc identities come from the selected database. In particular,
/// a missing std::collections::HashMap must never fall through to a different selected hashbrown declaration.
fn canonical_path_aliases(canonical_path: &str) -> Vec<String> {
    let normalized = canonical_path
        .split("::")
        .map(|segment| segment.strip_prefix("r#").unwrap_or(segment))
        .collect::<Vec<_>>()
        .join("::");
    if normalized != canonical_path {
        vec![normalized]
    } else {
        Vec::new()
    }
}

/// Build lookup candidates in preferred order for extraction and cache hits.
fn canonical_path_candidates(canonical_path: &str) -> Vec<String> {
    std::iter::once(canonical_path.to_string())
        .chain(canonical_path_aliases(canonical_path))
        .collect()
}

/// Insert or replace metadata only under the exact selected query that produced it.
fn insert_cached_item(inner: &mut CacheInner, root: &Path, metadata: Arc<RustItemMetadata>) {
    inner
        .complete_items
        .remove(&(root.to_path_buf(), metadata.canonical_path.clone()));
    inner
        .items
        .insert((root.to_path_buf(), metadata.canonical_path.clone()), metadata);
}

/// Insert a record produced by complete semantic extraction.
fn insert_complete_cached_item(inner: &mut CacheInner, root: &Path, metadata: Arc<RustItemMetadata>) {
    let key = (root.to_path_buf(), metadata.canonical_path.clone());
    insert_cached_item(inner, root, metadata);
    inner.complete_items.insert(key);
}

/// Return whether cached metadata can satisfy a caller that requires a complete Rust type surface.
///
/// Syntax-only source fallbacks intentionally retain a `FieldsAndVariantsOnly` marker, so they must still be
/// upgraded through semantic extraction. A complete type record already contains the methods and trait
/// implementations such a caller requires and can be reused without reopening its workspace.
fn is_complete_type_metadata(metadata: &RustItemMetadata) -> bool {
    matches!(
        &metadata.kind,
        RustItemKind::Type(type_info) if type_info.metadata_completeness.is_complete()
    )
}

/// Return whether the record itself proves it came from semantic expansion.
///
/// Source-only extraction cannot manufacture derive output. A trait or macro record carrying that output therefore
/// has the same complete semantic authority after a process restart as the expansion that produced it.
fn is_intrinsically_complete_metadata(metadata: &RustItemMetadata) -> bool {
    is_complete_type_metadata(metadata)
        || matches!(&metadata.kind, RustItemKind::Trait(info) if info.derive_macro.is_some())
        || matches!(&metadata.kind, RustItemKind::Macro(info) if !info.expanded_traits.is_empty())
}

/// Return whether one cache entry can satisfy a semantic-complete lookup.
///
/// Types carry an intrinsic completeness marker because source-derived type records may omit methods and trait
/// implementations. A trait or macro carrying derive-probe output is also intrinsically semantic because source
/// extraction cannot manufacture proc-macro output. Other item kinds are complete only when recorded by the complete
/// extraction path; that distinction survives the disk cache through `complete_items`.
fn cached_metadata_satisfies_complete_lookup(
    inner: &CacheInner,
    key: &(PathBuf, String),
    metadata: &RustItemMetadata,
) -> bool {
    is_intrinsically_complete_metadata(metadata) || inner.complete_items.contains(key)
}

/// Re-key a cached item for a query path while preserving the extracted Rust metadata.
fn insert_aliased_item(
    inner: &mut CacheInner,
    root: &Path,
    canonical_path: &str,
    hit: &Arc<RustItemMetadata>,
) -> Arc<RustItemMetadata> {
    let source_key = (root.to_path_buf(), hit.canonical_path.clone());
    let source_was_complete = inner.complete_items.contains(&source_key);
    let mut aliased = (*hit.as_ref()).clone();
    aliased.canonical_path = canonical_path.to_owned();
    let arc = Arc::new(aliased);
    let key_item = (root.to_path_buf(), canonical_path.to_owned());
    inner.failed_items.remove(&key_item);
    insert_cached_item(inner, root, Arc::clone(&arc));
    if source_was_complete {
        inner.complete_items.insert(key_item);
    }
    arc
}

/// Return whether an extraction error is a stable miss in this same selected database.
fn metadata_extraction_missed(err: &RustMetadataError) -> bool {
    matches!(
        err,
        RustMetadataError::CrateNotFound(_) | RustMetadataError::PathNotResolved(_)
    )
}

/// Extract through the one already selected database; no alternative graph or source route exists.
fn extract_in_workspace_set(
    inner: &mut CacheInner,
    root: &Path,
    canonical_path: &str,
    progress: &(dyn Fn(String) + Sync),
    timing_enabled: bool,
) -> Result<RustItemMetadata, RustMetadataError> {
    let selection = inner
        .selections
        .get(root)
        .ok_or_else(|| RustMetadataError::SelectedInputUnavailable {
            path: root.to_path_buf(),
        })?;
    let workspace = match inner.workspaces.entry(root.to_path_buf()) {
        Entry::Occupied(entry) => entry.into_mut(),
        Entry::Vacant(entry) => {
            let started = Instant::now();
            let workspace = RustWorkspace::load_selected(&selection.projection, &selection.temporary_root, progress)?;
            log_timing_stage(
                timing_enabled,
                root,
                canonical_path,
                "workspace.load.selected",
                started.elapsed(),
                "status=ok",
            );
            entry.insert(workspace)
        }
    };
    if workspace.selection_fingerprint != selection.projection.fingerprint() {
        return Err(RustMetadataError::InvalidSelectedInput {
            path: root.to_path_buf(),
            message: "loaded inspection database does not match its selected context".to_string(),
        });
    }
    progress(format!("extracting selected Rust item {canonical_path}"));
    let started = Instant::now();
    let result = extract_rust_item(workspace, canonical_path);
    log_timing_stage(
        timing_enabled,
        root,
        canonical_path,
        "extract.workspace.selected",
        started.elapsed(),
        if result.is_ok() { "status=ok" } else { "status=error" },
    );
    result
}

/// Clear all bookkeeping for one context while holding the cache mutex.
fn clear_context(inner: &mut CacheInner, root: &Path) {
    inner.selections.remove(root);
    inner.workspaces.remove(root);
    inner.items.retain(|(workspace_root, _), _| workspace_root != root);
    inner
        .fast_failed_items
        .retain(|(workspace_root, _)| workspace_root != root);
    inner
        .failed_items
        .retain(|(workspace_root, _), _| workspace_root != root);
    inner
        .complete_items
        .retain(|(workspace_root, _)| workspace_root != root);
    inner.disk_cache_state.remove(root);
}

impl RustMetadataCache {
    #[cfg(not(test))]
    fn shared_inner() -> Arc<Mutex<CacheInner>> {
        static SHARED_INNER: OnceLock<Arc<Mutex<CacheInner>>> = OnceLock::new();
        Arc::clone(SHARED_INNER.get_or_init(|| Arc::new(Mutex::new(CacheInner::default()))))
    }

    #[cfg(test)]
    fn shared_inner() -> Arc<Mutex<CacheInner>> {
        // Keep unit tests isolated by default so assertions remain deterministic.
        Arc::new(Mutex::new(CacheInner::default()))
    }

    /// Create an empty cache.
    pub fn new() -> Self {
        Self {
            inner: Self::shared_inner(),
        }
    }

    /// Bind validated selection to a caller-owned cache context without loading rust-analyzer.
    ///
    /// The caller retains admitted input leases while this cache uses the projection. A matching persisted record
    /// needs only this binding; semantic extraction lazily loads the selected database on a miss. Changing the
    /// fingerprint atomically discards all previous metadata, misses and the database for this context.
    pub fn bind_selected_project(
        &self,
        context: &Path,
        projection: ValidatedInspectionProject,
        temporary_root: &Path,
    ) -> Result<(), RustMetadataError> {
        let root = context.canonicalize()?;
        // Checking the allocated directory does not create the per-load projection or read selected source trees.
        let temporary_root = temporary_root.canonicalize()?;
        if !temporary_root.is_dir() {
            return Err(RustMetadataError::InvalidSelectedInput {
                path: temporary_root,
                message: "inspection temporary root must be an allocated directory".to_string(),
            });
        }
        if projection.contains_input_path(&temporary_root) || projection.contains_input_path(&root) {
            return Err(RustMetadataError::InvalidSelectedInput {
                path: root,
                message: "metadata cache and temporary output must be outside selected input roots".to_string(),
            });
        }
        let mut inner = self.inner.lock().map_err(|error| RustMetadataError::LoadWorkspace {
            path: root.clone(),
            message: format!("metadata cache lock poisoned: {error}"),
        })?;
        if inner
            .selections
            .get(&root)
            .is_none_or(|selected| selected.projection.fingerprint() != projection.fingerprint())
        {
            clear_context(&mut inner, &root);
        }
        inner.selections.insert(
            root,
            InspectionContext {
                projection,
                temporary_root,
            },
        );
        Ok(())
    }

    /// Attach an already loaded database only when it matches the context's validated selection.
    ///
    /// This is optional preparation reuse. A database cannot stand in for selection authority or replace a different
    /// binding. Ordinary disk-cache reads do not call this method.
    pub fn bind_selected_workspace(&self, context: &Path, workspace: RustWorkspace) -> Result<(), RustMetadataError> {
        let root = context.canonicalize()?;
        let mut inner = self.inner.lock().map_err(|error| RustMetadataError::LoadWorkspace {
            path: root.clone(),
            message: format!("metadata cache lock poisoned: {error}"),
        })?;
        if workspace_fingerprint(&inner, &root)? != workspace.selection_fingerprint {
            return Err(RustMetadataError::InvalidSelectedInput {
                path: root,
                message: "loaded inspection database does not match its selected context".to_string(),
            });
        }
        inner.workspaces.insert(root, workspace);
        Ok(())
    }

    /// Return metadata for `canonical_path`, loading/extracting on cache miss.
    ///
    /// Lookup uses in-memory exact/raw-identifier spelling aliases, then this selected database, then persistence.
    /// A missing selection and operational failures are terminal; only stable item misses try spelling aliases.
    fn get_or_extract_inner(
        &self,
        manifest_dir: &Path,
        canonical_path: &str,
        _registry_src_roots: Option<&[PathBuf]>,
        progress: &(dyn Fn(String) + Sync),
        persist_immediately: bool,
    ) -> Result<CacheAccess, RustMetadataError> {
        let root = manifest_dir.canonicalize()?;
        let timing_enabled = rust_inspect_timing_enabled();
        let mut trace = CallTrace::new(timing_enabled, &root, canonical_path);
        let key_item = (root.clone(), canonical_path.to_owned());

        let mut inner = self.inner.lock().map_err(|e| RustMetadataError::LoadWorkspace {
            path: root.clone(),
            message: format!("metadata cache lock poisoned: {e}"),
        })?;

        let disk_load_started = Instant::now();
        let disk_report = ensure_disk_cache_loaded(&mut inner, &root)?;
        log_timing_stage(
            timing_enabled,
            &root,
            canonical_path,
            "disk_cache.ensure_loaded",
            disk_load_started.elapsed(),
            disk_report.detail().as_str(),
        );

        if let Some(hit) = inner.items.get(&key_item) {
            let outcome = CacheAccessOutcome::ExactHit;
            trace.set_outcome(outcome.trace_label());
            return Ok(CacheAccess {
                metadata: Arc::clone(hit),
                outcome,
            });
        }
        if let Some(miss) = inner.failed_items.get(&key_item) {
            trace.set_outcome("hit.memory.negative");
            return Err(miss.to_error());
        }

        let mut last_err = None;
        let mut meta = None;
        for candidate in canonical_path_candidates(canonical_path) {
            let candidate_key = (root.clone(), candidate.clone());
            if let Some(hit) = inner.items.get(&candidate_key).cloned() {
                let arc = insert_aliased_item(&mut inner, &root, canonical_path, &hit);
                let persist_started = Instant::now();
                if persist_immediately
                    && let Err(err) = persist_item_to_disk_cache(&inner, &root)
                    && timing_enabled
                {
                    eprintln!(
                        "[rust-inspect-timing] root={} query={} stage=disk_cache.persist.alias_hit status=error err={err}",
                        root.display(),
                        canonical_path
                    );
                }
                log_timing_stage(
                    timing_enabled,
                    &root,
                    canonical_path,
                    "disk_cache.persist.alias_hit",
                    persist_started.elapsed(),
                    if persist_immediately { "" } else { "deferred=true" },
                );
                let outcome = CacheAccessOutcome::AliasHit;
                trace.set_outcome(outcome.trace_label());
                return Ok(CacheAccess { metadata: arc, outcome });
            }
            if let Some(miss) = inner.failed_items.get(&candidate_key) {
                last_err = Some(miss.to_error());
                continue;
            }
            match extract_in_workspace_set(&mut inner, &root, candidate.as_str(), progress, timing_enabled) {
                Ok(found) => {
                    meta = Some(found);
                    break;
                }
                Err(err) => {
                    let Some(negative) = NegativeLookup::from_error(&err) else {
                        return Err(err);
                    };
                    inner.failed_items.insert(candidate_key, negative);
                    last_err = Some(err);
                }
            }
        }
        let mut meta = match meta {
            Some(meta) => meta,
            None => {
                let err = last_err.unwrap_or_else(|| {
                    RustMetadataError::CrateNotFound(crate_name_for_path(canonical_path).to_string())
                });
                if let Some(negative) = NegativeLookup::from_error(&err) {
                    inner
                        .failed_items
                        .insert((root.clone(), canonical_path.to_owned()), negative.clone());
                    if persist_immediately
                        && let Err(persist_err) = persist_negative_to_disk_cache(&inner, &root)
                        && timing_enabled
                    {
                        eprintln!(
                            "[rust-inspect-timing] root={} query={} stage=disk_cache.persist.negative status=error err={persist_err}",
                            root.display(),
                            canonical_path
                        );
                    }
                }
                trace.set_outcome("miss.cached.negative");
                return Err(err);
            }
        };
        inner.failed_items.remove(&(root.clone(), canonical_path.to_owned()));
        meta.canonical_path = canonical_path.to_owned();
        let arc = Arc::new(meta);
        insert_cached_item(&mut inner, &root, Arc::clone(&arc));
        let persist_started = Instant::now();
        if persist_immediately
            && let Err(err) = persist_item_to_disk_cache(&inner, &root)
            && timing_enabled
        {
            eprintln!(
                "[rust-inspect-timing] root={} query={} stage=disk_cache.persist.extracted status=error err={err}",
                root.display(),
                canonical_path
            );
        }
        log_timing_stage(
            timing_enabled,
            &root,
            canonical_path,
            "disk_cache.persist.extracted",
            persist_started.elapsed(),
            if persist_immediately { "" } else { "deferred=true" },
        );
        let outcome = CacheAccessOutcome::Extracted;
        trace.set_outcome(outcome.trace_label());
        Ok(CacheAccess { metadata: arc, outcome })
    }

    /// Return metadata for a canonical Rust path, extracting from the workspace and persisting cache misses.
    pub fn get_or_extract(
        &self,
        manifest_dir: &Path,
        canonical_path: &str,
        progress: &(dyn Fn(String) + Sync),
    ) -> Result<Arc<RustItemMetadata>, RustMetadataError> {
        self.get_or_extract_inner(manifest_dir, canonical_path, None, progress, true)
            .map(|access| access.metadata)
    }

    /// Return complete semantic metadata from the selected database, refreshing an incomplete cached record.
    pub fn get_or_extract_complete(
        &self,
        manifest_dir: &Path,
        canonical_path: &str,
        progress: &(dyn Fn(String) + Sync),
    ) -> Result<Arc<RustItemMetadata>, RustMetadataError> {
        self.get_or_extract_complete_inner(manifest_dir, canonical_path, progress, true)
            .map(|access| access.metadata)
    }

    /// Return complete metadata while deferring persistence until the owning compiler phase flushes once.
    ///
    /// A typechecking pass can prepare hundreds of complete metadata records. Rewriting the full
    /// growing cache snapshot after each promotion is quadratic in the number of discovered records, so the compiler
    /// uses this method and calls [`Self::persist_manifest_dir`] after its final lookup.
    pub fn get_or_extract_complete_deferred_persist(
        &self,
        manifest_dir: &Path,
        canonical_path: &str,
        progress: &(dyn Fn(String) + Sync),
    ) -> Result<Arc<RustItemMetadata>, RustMetadataError> {
        self.get_or_extract_complete_inner(manifest_dir, canonical_path, progress, false)
            .map(|access| access.metadata)
    }

    /// Query a concrete Rust obligation only while the owning preparation phase still retains its workspace.
    ///
    /// Semantic consumers must use persisted [`RustTypeInfo::implemented_traits`] instead of reopening source
    /// inspection. This method remains available to preparation/tests that already own the loaded workspace.
    pub fn type_implements_trait(
        &self,
        manifest_dir: &Path,
        type_path: &str,
        trait_path: &str,
        mutable_reference: bool,
    ) -> Result<bool, RustMetadataError> {
        let root = manifest_dir.canonicalize()?;
        let mut inner = self.inner.lock().map_err(|error| RustMetadataError::LoadWorkspace {
            path: root.clone(),
            message: format!("metadata cache lock poisoned: {error}"),
        })?;
        let _ = ensure_disk_cache_loaded(&mut inner, &root)?;
        let workspace_key = root.clone();
        let workspace = inner
            .workspaces
            .get(&workspace_key)
            .ok_or_else(|| RustMetadataError::LoadWorkspace {
                path: root,
                message: format!(
                    "trait-solver workspace is not retained for `{type_path}`; consume persisted implementation metadata"
                ),
            })?;
        rust_type_implements_trait(workspace, type_path, trait_path, mutable_reference)
    }

    /// Run complete semantic extraction and replace metadata for this one canonical path.
    fn get_or_extract_complete_inner(
        &self,
        manifest_dir: &Path,
        canonical_path: &str,
        progress: &(dyn Fn(String) + Sync),
        persist_immediately: bool,
    ) -> Result<CacheAccess, RustMetadataError> {
        let root = manifest_dir.canonicalize()?;
        let timing_enabled = rust_inspect_timing_enabled();
        let mut trace = CallTrace::new(timing_enabled, &root, canonical_path);
        let key_item = (root.clone(), canonical_path.to_owned());
        let mut inner = self.inner.lock().map_err(|e| RustMetadataError::LoadWorkspace {
            path: root.clone(),
            message: format!("metadata cache lock poisoned: {e}"),
        })?;
        let disk_load_started = Instant::now();
        let disk_report = ensure_disk_cache_loaded(&mut inner, &root)?;
        log_timing_stage(
            timing_enabled,
            &root,
            canonical_path,
            "disk_cache.ensure_loaded",
            disk_load_started.elapsed(),
            disk_report.detail().as_str(),
        );
        if let Some(hit) = inner.items.get(&key_item).cloned()
            && cached_metadata_satisfies_complete_lookup(&inner, &key_item, hit.as_ref())
        {
            let outcome = CacheAccessOutcome::ExactHit;
            trace.set_outcome(outcome.trace_label());
            return Ok(CacheAccess { metadata: hit, outcome });
        }
        if let Some(miss) = inner.failed_items.get(&key_item) {
            trace.set_outcome("hit.memory.negative");
            return Err(miss.to_error());
        }
        let mut last_err = None;
        for candidate in canonical_path_candidates(canonical_path) {
            let candidate_key = (root.clone(), candidate.clone());
            if let Some(miss) = inner.failed_items.get(&candidate_key) {
                last_err = Some(miss.to_error());
                continue;
            }
            match extract_in_workspace_set(&mut inner, &root, candidate.as_str(), progress, timing_enabled) {
                Ok(mut metadata) => {
                    metadata.canonical_path = canonical_path.to_owned();
                    let metadata = Arc::new(metadata);
                    inner.failed_items.remove(&key_item);
                    insert_complete_cached_item(&mut inner, &root, Arc::clone(&metadata));
                    if persist_immediately && let Err(err) = persist_item_to_disk_cache(&inner, &root) {
                        tracing::warn!(
                            root = %root.display(),
                            query = %canonical_path,
                            error = %err,
                            "failed to persist rust-inspect disk cache after complete extraction"
                        );
                    }
                    trace.set_outcome(CacheAccessOutcome::Extracted.trace_label());
                    return Ok(CacheAccess {
                        metadata,
                        outcome: CacheAccessOutcome::Extracted,
                    });
                }
                Err(err) => {
                    let Some(negative) = NegativeLookup::from_error(&err) else {
                        return Err(err);
                    };
                    inner.failed_items.insert(candidate_key, negative);
                    last_err = Some(err);
                }
            }
        }
        let err = last_err
            .unwrap_or_else(|| RustMetadataError::CrateNotFound(crate_name_for_path(canonical_path).to_string()));
        if let Some(negative) = NegativeLookup::from_error(&err) {
            inner.failed_items.insert(key_item, negative);
            if persist_immediately && let Err(persist_err) = persist_negative_to_disk_cache(&inner, &root) {
                tracing::warn!(
                    root = %root.display(),
                    query = %canonical_path,
                    error = %persist_err,
                    "failed to persist rust-inspect disk cache after complete extraction miss"
                );
            }
        }
        trace.set_outcome("miss.cached.negative");
        Err(err)
    }

    /// Return metadata for a canonical Rust path while deferring disk-cache persistence to the caller.
    ///
    /// Prewarm batches extract many items and flush the manifest cache once instead of rewriting it after every item.
    pub(crate) fn get_or_extract_deferred_persist(
        &self,
        manifest_dir: &Path,
        canonical_path: &str,
        progress: &(dyn Fn(String) + Sync),
    ) -> Result<CacheAccess, RustMetadataError> {
        self.get_or_extract_inner(manifest_dir, canonical_path, None, progress, false)
    }

    /// Persist the in-memory cache snapshot for one manifest root.
    ///
    /// Prewarm uses deferred extraction so callers can batch writes until every requested item has been visited.
    pub fn persist_manifest_dir(&self, manifest_dir: &Path) -> Result<(), RustMetadataError> {
        let root = manifest_dir.canonicalize()?;
        let mut inner = self.inner.lock().map_err(|e| RustMetadataError::LoadWorkspace {
            path: root.clone(),
            message: format!("metadata cache lock poisoned: {e}"),
        })?;
        ensure_disk_cache_loaded(&mut inner, &root)?;
        persist_manifest_dir_to_disk_cache(&inner, &root)
    }

    /// Return metadata from memory/disk cache only.
    ///
    /// This does not trigger rust-analyzer workspace loading or extraction.
    pub fn get_cached(
        &self,
        manifest_dir: &Path,
        canonical_path: &str,
    ) -> Result<Option<CacheLookupHit>, RustMetadataError> {
        let root = manifest_dir.canonicalize()?;
        let key_item = (root.clone(), canonical_path.to_owned());
        let mut inner = self.inner.lock().map_err(|e| RustMetadataError::LoadWorkspace {
            path: root.clone(),
            message: format!("metadata cache lock poisoned: {e}"),
        })?;
        ensure_disk_cache_loaded(&mut inner, &root)?;

        if let Some(hit) = inner.items.get(&key_item) {
            return Ok(Some(CacheLookupHit {
                metadata: Arc::clone(hit),
                alias_used: false,
            }));
        }

        for candidate in canonical_path_candidates(canonical_path) {
            let candidate_key = (root.clone(), candidate.clone());
            if let Some(hit) = inner.items.get(&candidate_key).cloned() {
                let arc = insert_aliased_item(&mut inner, &root, canonical_path, &hit);
                if let Err(err) = persist_item_to_disk_cache(&inner, &root) {
                    tracing::warn!(
                        root = %root.display(),
                        query = %canonical_path,
                        error = %err,
                        "failed to persist rust-inspect disk cache after alias hit"
                    );
                    if rust_inspect_timing_enabled() {
                        eprintln!(
                            "[rust-inspect-timing] root={} query={} stage=disk_cache.persist.cached_alias status=error err={err}",
                            root.display(),
                            canonical_path
                        );
                    }
                }
                return Ok(Some(CacheLookupHit {
                    metadata: arc,
                    alias_used: true,
                }));
            }
        }
        Ok(None)
    }

    /// Return metadata from the bound database or its cache without loading another workspace.
    ///
    /// Stable misses are remembered in memory. This compatibility entrypoint no longer performs syntax-only source
    /// substitution; callers must bind the selected database during preparation.
    pub fn get_cached_or_extract_fast(
        &self,
        manifest_dir: &Path,
        canonical_path: &str,
    ) -> Result<Option<CacheLookupHit>, RustMetadataError> {
        self.get_cached_or_extract_fast_with_search_roots(manifest_dir, canonical_path, None)
    }

    /// Query the bound database while retaining the former caller signature during migration.
    ///
    /// Registry roots cannot supply a selected graph; this method never searches them or derives dependency edges.
    pub fn get_cached_or_extract_fast_with_registry_src_roots(
        &self,
        manifest_dir: &Path,
        canonical_path: &str,
        registry_src_roots: &[PathBuf],
    ) -> Result<Option<CacheLookupHit>, RustMetadataError> {
        self.get_cached_or_extract_fast_with_search_roots(manifest_dir, canonical_path, Some(registry_src_roots))
    }

    /// Share the bound-database query path; the old source catalog is not selection authority.
    fn get_cached_or_extract_fast_with_search_roots(
        &self,
        manifest_dir: &Path,
        canonical_path: &str,
        _registry_src_roots: Option<&[PathBuf]>,
    ) -> Result<Option<CacheLookupHit>, RustMetadataError> {
        let root = manifest_dir.canonicalize()?;
        let timing_enabled = rust_inspect_timing_enabled();
        let key_item = (root.clone(), canonical_path.to_owned());
        let mut inner = self.inner.lock().map_err(|e| RustMetadataError::LoadWorkspace {
            path: root.clone(),
            message: format!("metadata cache lock poisoned: {e}"),
        })?;
        ensure_disk_cache_loaded(&mut inner, &root)?;

        if let Some(hit) = inner.items.get(&key_item) {
            return Ok(Some(CacheLookupHit {
                metadata: Arc::clone(hit),
                alias_used: false,
            }));
        }
        if inner.fast_failed_items.contains(&key_item) {
            return Ok(None);
        }

        for candidate in canonical_path_candidates(canonical_path) {
            let candidate_key = (root.clone(), candidate.clone());
            if let Some(hit) = inner.items.get(&candidate_key).cloned() {
                let arc = insert_aliased_item(&mut inner, &root, canonical_path, &hit);
                return Ok(Some(CacheLookupHit {
                    metadata: arc,
                    alias_used: true,
                }));
            }
            if inner.fast_failed_items.contains(&candidate_key) {
                continue;
            }
            match extract_in_workspace_set(&mut inner, &root, candidate.as_str(), &|_| (), timing_enabled) {
                Ok(mut meta) => {
                    inner.fast_failed_items.remove(&candidate_key);
                    inner.fast_failed_items.remove(&key_item);
                    meta.canonical_path = canonical_path.to_owned();
                    let arc = Arc::new(meta);
                    insert_cached_item(&mut inner, &root, Arc::clone(&arc));
                    // The typechecker can request hundreds of metadata records in one pass. Each
                    // snapshot contains all prior records, so rewriting it here turns a linear walk into repeated
                    // growing JSON serializations. The owning preparation phase flushes this shared cache once after
                    // typechecking, just as `Inspector::prewarm` does for its explicit query batch.
                    return Ok(Some(CacheLookupHit {
                        metadata: arc,
                        alias_used: candidate != canonical_path,
                    }));
                }
                Err(err) if metadata_extraction_missed(&err) => {
                    inner.fast_failed_items.insert(candidate_key);
                }
                Err(err) => return Err(err),
            }
        }

        inner.fast_failed_items.insert(key_item);
        Ok(None)
    }

    /// Drop all in-memory and disk-cache bookkeeping for one manifest root.
    ///
    /// Use this after filesystem or dependency changes so the next lookup rebuilds fresh alias indexes.
    pub fn invalidate_manifest_dir(&self, manifest_dir: &Path) -> Result<(), RustMetadataError> {
        let root = manifest_dir.canonicalize()?;
        let mut inner = self.inner.lock().map_err(|e| RustMetadataError::LoadWorkspace {
            path: root.clone(),
            message: format!("metadata cache lock poisoned: {e}"),
        })?;
        clear_context(&mut inner, &root);
        Ok(())
    }

    /// Query the selected database with the former source-catalog entrypoint signature.
    ///
    /// Kept for source compatibility while callers migrate to `bind_selected_project`. A source catalog alone
    /// cannot select the graph; this method uses only the already bound database.
    pub fn get_or_extract_with_registry_src_roots(
        &self,
        manifest_dir: &Path,
        canonical_path: &str,
        registry_src_roots: &[PathBuf],
        progress: &(dyn Fn(String) + Sync),
    ) -> Result<Arc<RustItemMetadata>, RustMetadataError> {
        self.get_or_extract_inner(manifest_dir, canonical_path, Some(registry_src_roots), progress, true)
            .map(|access| access.metadata)
    }

    /// Seed metadata directly for tests without invoking rust-analyzer extraction.
    #[doc(hidden)]
    pub fn insert_test_item(&self, manifest_dir: &Path, metadata: RustItemMetadata) -> Result<(), RustMetadataError> {
        let root = manifest_dir.canonicalize()?;
        let mut inner = self.inner.lock().map_err(|e| RustMetadataError::LoadWorkspace {
            path: manifest_dir.to_path_buf(),
            message: format!("metadata cache lock poisoned: {e}"),
        })?;
        inner
            .failed_items
            .remove(&(root.clone(), metadata.canonical_path.clone()));
        insert_cached_item(&mut inner, &root, Arc::new(metadata));
        Ok(())
    }
}

impl Default for RustMetadataCache {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    include!("cache_tests.rs");
}
