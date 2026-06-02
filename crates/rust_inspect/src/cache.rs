//! In-memory cache: one loaded workspace per manifest directory, plus per-item metadata.
//!
//! The cache is the boundary that keeps rust-analyzer/Cargo extraction out of compiler hot paths. Preparation code may
//! call `get_or_extract`; ordinary semantic/codegen consumers should use cache-only reads through `Inspector::get`.

use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
#[cfg(not(test))]
use std::sync::OnceLock;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use incan_core::interop::RustItemMetadata;
use ra_ap_syntax::{
    AstNode, Edition, SourceFile, SyntaxKind, T,
    ast::{self, HasModuleItem, HasName},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::cache_resolve::{crate_name_for_path, dependency_manifest_dir_for_crate};
use crate::cache_timing::{CallTrace, log_timing_stage, rust_inspect_timing_enabled};
use crate::error::RustMetadataError;
use crate::extractor::extract_rust_item;
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
    workspaces: HashMap<(PathBuf, bool), RustWorkspace>,
    items: HashMap<(PathBuf, String), Arc<RustItemMetadata>>,
    definition_aliases: HashMap<(PathBuf, String), String>,
    dependency_manifest_dirs: HashMap<(PathBuf, String), Option<PathBuf>>,
    root_crate_names: HashMap<PathBuf, Vec<String>>,
    crate_reexport_aliases: HashMap<PathBuf, HashMap<String, String>>,
    failed_items: HashMap<(PathBuf, String), NegativeLookup>,
    disk_cache_state: HashMap<PathBuf, DiskCacheState>,
}

#[derive(Default)]
struct DiskCacheState {
    loaded: bool,
    workspace_fingerprint: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
enum NegativeLookup {
    CrateNotFound(String),
    PathNotResolved(String),
    UnsupportedMacro(String),
}

impl NegativeLookup {
    fn from_error(err: &RustMetadataError) -> Option<Self> {
        match err {
            RustMetadataError::CrateNotFound(path) => Some(Self::CrateNotFound(path.clone())),
            RustMetadataError::PathNotResolved(path) => Some(Self::PathNotResolved(path.clone())),
            RustMetadataError::UnsupportedMacro(path) => Some(Self::UnsupportedMacro(path.clone())),
            RustMetadataError::Io(_) | RustMetadataError::LoadWorkspace { .. } => None,
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
    #[serde(default)]
    misses: HashMap<String, NegativeLookup>,
}

// Bump when extracted metadata semantics change in a way that makes previously persisted items unsafe to reuse.
const DISK_CACHE_FORMAT: u32 = 7;
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

/// Hash lock-workspace inputs so stale cache files can be ignored cheaply.
fn workspace_fingerprint(root: &Path) -> Result<String, RustMetadataError> {
    let mut hasher = Sha256::new();
    hasher.update(format!("cache_format:{DISK_CACHE_FORMAT}\n").as_bytes());
    hash_workspace_fingerprint_inputs(&mut hasher, root)?;
    Ok(hex::encode(hasher.finalize()))
}

/// Historical fingerprint used before cache compatibility was decoupled from the package version.
fn legacy_versioned_workspace_fingerprint(root: &Path, inspector_version: &str) -> Result<String, RustMetadataError> {
    let mut hasher = Sha256::new();
    hasher.update(format!("cache_format:{DISK_CACHE_FORMAT}\n").as_bytes());
    hasher.update(format!("inspector_version:{inspector_version}\n").as_bytes());
    hash_workspace_fingerprint_inputs(&mut hasher, root)?;
    Ok(hex::encode(hasher.finalize()))
}

fn hash_workspace_fingerprint_inputs(hasher: &mut Sha256, root: &Path) -> Result<(), RustMetadataError> {
    hasher.update(fs::read(root.join("Cargo.toml"))?);
    match fs::read(root.join("Cargo.lock")) {
        Ok(lock) => hasher.update(lock),
        Err(err) if err.kind() == ErrorKind::NotFound => {}
        Err(err) => return Err(err.into()),
    }
    Ok(())
}

fn disk_cache_fingerprint_matches(
    root: &Path,
    envelope: &DiskCacheEnvelope,
    current_fingerprint: &str,
) -> Result<bool, RustMetadataError> {
    if envelope.workspace_fingerprint == current_fingerprint {
        return Ok(true);
    }
    let legacy_fingerprint = legacy_versioned_workspace_fingerprint(root, envelope.inspector_version.as_str())?;
    Ok(envelope.workspace_fingerprint == legacy_fingerprint)
}

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

/// Load valid disk-cache items into memory for one workspace.
fn load_disk_cache_into_memory(inner: &mut CacheInner, root: &Path) -> Result<Option<String>, RustMetadataError> {
    let fingerprint = workspace_fingerprint(root)?;
    let Some(envelope) = read_disk_cache(root)? else {
        return Ok(Some(fingerprint));
    };
    if envelope.cache_format != DISK_CACHE_FORMAT
        || !disk_cache_fingerprint_matches(root, &envelope, fingerprint.as_str())?
    {
        return Ok(Some(fingerprint));
    }
    for (canonical_path, metadata) in envelope.items {
        let mut metadata = metadata;
        metadata.canonical_path = canonical_path;
        insert_cached_item(inner, root, Arc::new(metadata));
    }
    for (canonical_path, miss) in envelope.misses {
        inner.failed_items.insert((root.to_path_buf(), canonical_path), miss);
    }
    Ok(Some(fingerprint))
}

/// Ensure the workspace-local disk cache has been loaded once for this process.
fn ensure_disk_cache_loaded(inner: &mut CacheInner, root: &Path) -> Result<(), RustMetadataError> {
    if inner.disk_cache_state.get(root).is_some_and(|state| state.loaded) {
        return Ok(());
    }
    let fingerprint = load_disk_cache_into_memory(inner, root)?;
    let state = inner.disk_cache_state.entry(root.to_path_buf()).or_default();
    state.workspace_fingerprint = fingerprint;
    state.loaded = true;
    Ok(())
}

/// Build the current workspace-local disk cache snapshot.
fn disk_cache_envelope(inner: &CacheInner, root: &Path) -> Result<DiskCacheEnvelope, RustMetadataError> {
    let fingerprint = inner
        .disk_cache_state
        .get(root)
        .and_then(|state| state.workspace_fingerprint.clone())
        .unwrap_or(workspace_fingerprint(root)?);
    let mut items = HashMap::new();
    let mut misses = HashMap::new();
    for ((item_root, canonical_path), cached) in &inner.items {
        if item_root == root {
            items.insert(canonical_path.clone(), (*cached.as_ref()).clone());
        }
    }
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

/// Generate canonical-path aliases that account for Rust/Cargo naming and std/core/alloc spellings.
fn canonical_path_aliases(canonical_path: &str) -> Vec<String> {
    let mut aliases = Vec::new();

    let stripped_raw_idents = canonical_path
        .split("::")
        .map(|segment| segment.strip_prefix("r#").unwrap_or(segment))
        .collect::<Vec<_>>()
        .join("::");
    if stripped_raw_idents != canonical_path {
        aliases.push(stripped_raw_idents);
    }

    if let Some((crate_name, rest)) = canonical_path.split_once("::") {
        if crate_name.contains('_') {
            aliases.push(format!("{}::{rest}", crate_name.replace('_', "-")));
        }
        if crate_name.contains('-') {
            aliases.push(format!("{}::{rest}", crate_name.replace('-', "_")));
        }
    }

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

/// Build lookup candidates in preferred order for extraction and cache hits.
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

/// Remove definition-path aliases owned by the currently cached item at `canonical_path`.
fn remove_cached_item_definition_aliases(inner: &mut CacheInner, root: &Path, canonical_path: &str) {
    let key_item = (root.to_path_buf(), canonical_path.to_owned());
    let Some(existing) = inner.items.get(&key_item) else {
        return;
    };
    let Some(definition_path) = existing.definition_path.as_deref() else {
        return;
    };
    for candidate in canonical_path_candidates(definition_path) {
        let key = (root.to_path_buf(), candidate);
        if inner
            .definition_aliases
            .get(&key)
            .is_some_and(|indexed_path| indexed_path == canonical_path)
        {
            inner.definition_aliases.remove(&key);
        }
    }
}

/// Index one cached item by its resolved Rust definition path and supported spelling aliases.
fn index_cached_item_definition_aliases(inner: &mut CacheInner, root: &Path, metadata: &RustItemMetadata) {
    let Some(definition_path) = metadata.definition_path.as_deref() else {
        return;
    };
    for candidate in canonical_path_candidates(definition_path) {
        inner
            .definition_aliases
            .insert((root.to_path_buf(), candidate), metadata.canonical_path.clone());
    }
}

/// Insert or replace cached metadata while keeping the definition-path alias index in sync.
fn insert_cached_item(inner: &mut CacheInner, root: &Path, metadata: Arc<RustItemMetadata>) {
    remove_cached_item_definition_aliases(inner, root, metadata.canonical_path.as_str());
    index_cached_item_definition_aliases(inner, root, metadata.as_ref());
    inner
        .items
        .insert((root.to_path_buf(), metadata.canonical_path.clone()), metadata);
}

/// Re-key a cached item for a query path while preserving the extracted Rust metadata.
fn insert_aliased_item(
    inner: &mut CacheInner,
    root: &Path,
    canonical_path: &str,
    hit: &Arc<RustItemMetadata>,
) -> Arc<RustItemMetadata> {
    let mut aliased = (*hit.as_ref()).clone();
    aliased.canonical_path = canonical_path.to_owned();
    let arc = Arc::new(aliased);
    let key_item = (root.to_path_buf(), canonical_path.to_owned());
    inner.failed_items.remove(&key_item);
    insert_cached_item(inner, root, Arc::clone(&arc));
    arc
}

/// Look up cached public aliases whose recorded definition path matches the requested path.
fn cached_definition_alias(inner: &CacheInner, root: &Path, canonical_path: &str) -> Option<Arc<RustItemMetadata>> {
    for candidate in canonical_path_candidates(canonical_path) {
        let alias_key = (root.to_path_buf(), candidate);
        if let Some(canonical_path) = inner.definition_aliases.get(&alias_key) {
            let item_key = (root.to_path_buf(), canonical_path.clone());
            if let Some(cached) = inner.items.get(&item_key) {
                return Some(Arc::clone(cached));
            }
        }
    }
    None
}

fn normalized_crate_cache_key(crate_name: &str) -> String {
    crate_name.replace('-', "_")
}

/// Resolve a dependency manifest directory once per generated lock workspace and crate spelling.
fn resolve_dependency_manifest_dir(
    inner: &mut CacheInner,
    root: &Path,
    crate_name: &str,
    registry_src_roots: Option<&[PathBuf]>,
) -> Option<PathBuf> {
    let key = (root.to_path_buf(), normalized_crate_cache_key(crate_name));
    if let Some(cached) = inner.dependency_manifest_dirs.get(&key) {
        return cached.clone();
    }
    let resolved = dependency_manifest_dir_for_crate(root, crate_name, registry_src_roots);
    inner.dependency_manifest_dirs.insert(key, resolved.clone());
    resolved
}

fn manifest_string_field(value: &toml::Value, table: &str, key: &str) -> Option<String> {
    value
        .get(table)
        .and_then(|section| section.get(key))
        .and_then(toml::Value::as_str)
        .map(normalized_crate_cache_key)
}

fn load_root_crate_names(root: &Path) -> Vec<String> {
    let Ok(payload) = fs::read_to_string(root.join("Cargo.toml")) else {
        return Vec::new();
    };
    let Ok(manifest) = toml::from_str::<toml::Value>(payload.as_str()) else {
        return Vec::new();
    };
    let mut names = Vec::new();
    if let Some(name) = manifest_string_field(&manifest, "package", "name") {
        names.push(name);
    }
    if let Some(name) = manifest_string_field(&manifest, "lib", "name") {
        names.push(name);
    }
    if let Some(bins) = manifest.get("bin").and_then(toml::Value::as_array) {
        for bin in bins {
            if let Some(name) = bin.get("name").and_then(toml::Value::as_str) {
                names.push(normalized_crate_cache_key(name));
            }
        }
    }
    names.sort();
    names.dedup();
    names
}

fn manifest_lib_source_path(root: &Path, manifest: &toml::Value) -> PathBuf {
    manifest
        .get("lib")
        .and_then(|section| section.get("path"))
        .and_then(toml::Value::as_str)
        .map_or_else(|| root.join("src").join("lib.rs"), |path| root.join(path))
}

fn rust_path_segments(path: &ast::Path) -> Vec<String> {
    let mut segments = Vec::new();
    let mut current = Some(path.clone());
    while let Some(path) = current {
        if let Some(segment) = path.segment().and_then(|segment| segment.name_ref()) {
            segments.push(segment.to_string());
        }
        current = path.qualifier();
    }
    segments.reverse();
    segments
}

fn crate_reexport_alias_from_use_tree(tree: &ast::UseTree) -> Option<(String, String)> {
    if tree.star_token().is_some() || tree.use_tree_list().is_some() {
        return None;
    }
    let segments = rust_path_segments(&tree.path()?);
    if segments.len() != 1 {
        return None;
    }
    let target = normalized_crate_cache_key(segments[0].trim_start_matches("r#"));
    let alias = tree
        .rename()
        .and_then(|rename| rename.name())
        .map(|name| normalized_crate_cache_key(name.to_string().trim_start_matches("r#")))
        .unwrap_or_else(|| target.clone());
    Some((alias, target))
}

fn use_item_is_plain_public(use_item: &ast::Use) -> bool {
    let mut significant = use_item
        .syntax()
        .descendants_with_tokens()
        .filter_map(|element| element.into_token())
        .filter(|token| !matches!(token.kind(), SyntaxKind::COMMENT | SyntaxKind::WHITESPACE));
    matches!(significant.next().map(|token| token.kind()), Some(T![pub]))
        && matches!(significant.next().map(|token| token.kind()), Some(T![use]))
}

fn load_crate_reexport_aliases(root: &Path) -> HashMap<String, String> {
    let Ok(payload) = fs::read_to_string(root.join("Cargo.toml")) else {
        return HashMap::new();
    };
    let Ok(manifest) = toml::from_str::<toml::Value>(payload.as_str()) else {
        return HashMap::new();
    };
    let source_path = manifest_lib_source_path(root, &manifest);
    let Ok(source) = fs::read_to_string(source_path) else {
        return HashMap::new();
    };
    let parsed = SourceFile::parse(source.as_str(), Edition::CURRENT).tree();
    let mut aliases = HashMap::new();
    for item in parsed.items() {
        let ast::Item::Use(use_item) = item else {
            continue;
        };
        if !use_item_is_plain_public(&use_item) {
            continue;
        }
        let Some(tree) = use_item.use_tree() else {
            continue;
        };
        if let Some((alias, target)) = crate_reexport_alias_from_use_tree(&tree) {
            aliases.insert(alias, target);
        }
    }
    aliases
}

fn dependency_reexport_alias_candidate(
    inner: &mut CacheInner,
    dep_root: &Path,
    canonical_path: &str,
) -> Option<String> {
    let mut segments = canonical_path.split("::").collect::<Vec<_>>();
    if segments.len() < 3 {
        return None;
    }
    let reexport_alias = normalized_crate_cache_key(segments[1].trim_start_matches("r#"));
    let aliases = inner
        .crate_reexport_aliases
        .entry(dep_root.to_path_buf())
        .or_insert_with(|| load_crate_reexport_aliases(dep_root));
    let target = aliases.get(&reexport_alias)?;
    segments[1] = target.as_str();
    let candidate = segments[1..].join("::");
    if candidate == canonical_path {
        None
    } else {
        Some(candidate)
    }
}

fn root_workspace_declares_crate(inner: &mut CacheInner, root: &Path, crate_name: &str) -> bool {
    let names = inner
        .root_crate_names
        .entry(root.to_path_buf())
        .or_insert_with(|| load_root_crate_names(root));
    names
        .iter()
        .any(|name| name == normalized_crate_cache_key(crate_name).as_str())
}

/// Attempt extraction through one planned route: primary workspace, dependency workspace, then root out-dir workspace
/// only for root-package items.
fn extract_in_workspace_set(
    inner: &mut CacheInner,
    root: &Path,
    canonical_path: &str,
    registry_src_roots: Option<&[PathBuf]>,
    progress: &(dyn Fn(String) + Sync),
    timing_enabled: bool,
) -> Result<RustItemMetadata, RustMetadataError> {
    let mut deferred_load_error = None;

    match inner.workspaces.entry((root.to_path_buf(), false)) {
        Entry::Occupied(o) => {
            let started = Instant::now();
            match extract_rust_item(o.into_mut(), canonical_path) {
                Ok(meta) => {
                    log_timing_stage(
                        timing_enabled,
                        root,
                        canonical_path,
                        "extract.workspace.primary",
                        started.elapsed(),
                        "workspace_hit=true out_dirs=false",
                    );
                    return Ok(meta);
                }
                Err(RustMetadataError::CrateNotFound(_)) | Err(RustMetadataError::PathNotResolved(_)) => {}
                Err(err) => return Err(err),
            }
            log_timing_stage(
                timing_enabled,
                root,
                canonical_path,
                "extract.workspace.primary",
                started.elapsed(),
                "workspace_hit=true out_dirs=false status=miss",
            );
        }
        Entry::Vacant(v) => {
            let load_started = Instant::now();
            match RustWorkspace::load(root, progress) {
                Ok(workspace) => {
                    log_timing_stage(
                        timing_enabled,
                        root,
                        canonical_path,
                        "workspace.load.primary",
                        load_started.elapsed(),
                        "out_dirs=false status=ok",
                    );
                    let extract_started = Instant::now();
                    match extract_rust_item(v.insert(workspace), canonical_path) {
                        Ok(meta) => {
                            log_timing_stage(
                                timing_enabled,
                                root,
                                canonical_path,
                                "extract.workspace.primary",
                                extract_started.elapsed(),
                                "workspace_hit=false out_dirs=false",
                            );
                            return Ok(meta);
                        }
                        Err(RustMetadataError::CrateNotFound(_)) | Err(RustMetadataError::PathNotResolved(_)) => {}
                        Err(err) => return Err(err),
                    }
                    log_timing_stage(
                        timing_enabled,
                        root,
                        canonical_path,
                        "extract.workspace.primary",
                        extract_started.elapsed(),
                        "workspace_hit=false out_dirs=false status=miss",
                    );
                }
                Err(err) => {
                    log_timing_stage(
                        timing_enabled,
                        root,
                        canonical_path,
                        "workspace.load.primary",
                        load_started.elapsed(),
                        "out_dirs=false status=error",
                    );
                    deferred_load_error = Some(err);
                }
            }
        }
    }

    let crate_name = crate_name_for_path(canonical_path);
    let dep_resolve_started = Instant::now();
    let dep_root = resolve_dependency_manifest_dir(inner, root, crate_name, registry_src_roots);
    log_timing_stage(
        timing_enabled,
        root,
        canonical_path,
        "dependency.resolve_manifest_dir",
        dep_resolve_started.elapsed(),
        crate_name,
    );
    if let Some(dep_root) = dep_root {
        if let Some(reexported_path) = dependency_reexport_alias_candidate(inner, &dep_root, canonical_path) {
            let alias_started = Instant::now();
            match extract_in_workspace_set(
                inner,
                root,
                reexported_path.as_str(),
                registry_src_roots,
                progress,
                timing_enabled,
            ) {
                Ok(meta) => {
                    log_timing_stage(
                        timing_enabled,
                        root,
                        canonical_path,
                        "extract.dependency.reexport_alias",
                        alias_started.elapsed(),
                        reexported_path.as_str(),
                    );
                    return Ok(meta);
                }
                Err(
                    err @ (RustMetadataError::CrateNotFound(_)
                    | RustMetadataError::PathNotResolved(_)
                    | RustMetadataError::UnsupportedMacro(_)),
                ) => {
                    log_timing_stage(
                        timing_enabled,
                        root,
                        canonical_path,
                        "extract.dependency.reexport_alias",
                        alias_started.elapsed(),
                        "status=miss short_circuit=true",
                    );
                    // A public crate re-export is an identity path. If the target crate misses, extracting
                    // the wrapper dependency cannot discover a different item for the same path; it only
                    // repeats the expensive semantic lookup through another surface.
                    return Err(err);
                }
                Err(err) => return Err(err),
            }
        }
        let dep_root_display = dep_root.display().to_string();
        let dep_workspace = match inner.workspaces.entry((dep_root.clone(), true)) {
            Entry::Occupied(o) => o.into_mut(),
            Entry::Vacant(v) => {
                let load_started = Instant::now();
                let workspace = RustWorkspace::load_with_options(&dep_root, progress, true)?;
                log_timing_stage(
                    timing_enabled,
                    root,
                    canonical_path,
                    "workspace.load.dependency.out_dirs",
                    load_started.elapsed(),
                    dep_root_display.as_str(),
                );
                v.insert(workspace)
            }
        };
        let extract_started = Instant::now();
        let meta = extract_rust_item(dep_workspace, canonical_path);
        log_timing_stage(
            timing_enabled,
            root,
            canonical_path,
            "extract.workspace.dependency",
            extract_started.elapsed(),
            dep_root_display.as_str(),
        );
        return meta;
    }

    if !root_workspace_declares_crate(inner, root, crate_name) {
        if let Some(err) = deferred_load_error {
            return Err(err);
        }
        return Err(RustMetadataError::CrateNotFound(crate_name.to_string()));
    }

    match inner.workspaces.entry((root.to_path_buf(), true)) {
        Entry::Occupied(o) => {
            let started = Instant::now();
            match extract_rust_item(o.into_mut(), canonical_path) {
                Ok(meta) => {
                    log_timing_stage(
                        timing_enabled,
                        root,
                        canonical_path,
                        "extract.workspace.out_dirs",
                        started.elapsed(),
                        "workspace_hit=true out_dirs=true",
                    );
                    return Ok(meta);
                }
                Err(RustMetadataError::CrateNotFound(_)) | Err(RustMetadataError::PathNotResolved(_)) => {}
                Err(err) => return Err(err),
            }
            log_timing_stage(
                timing_enabled,
                root,
                canonical_path,
                "extract.workspace.out_dirs",
                started.elapsed(),
                "workspace_hit=true out_dirs=true status=miss",
            );
        }
        Entry::Vacant(v) => {
            let load_started = Instant::now();
            match RustWorkspace::load_with_options(root, progress, true) {
                Ok(workspace) => {
                    log_timing_stage(
                        timing_enabled,
                        root,
                        canonical_path,
                        "workspace.load.out_dirs",
                        load_started.elapsed(),
                        "out_dirs=true status=ok",
                    );
                    let extract_started = Instant::now();
                    match extract_rust_item(v.insert(workspace), canonical_path) {
                        Ok(meta) => {
                            log_timing_stage(
                                timing_enabled,
                                root,
                                canonical_path,
                                "extract.workspace.out_dirs",
                                extract_started.elapsed(),
                                "workspace_hit=false out_dirs=true",
                            );
                            return Ok(meta);
                        }
                        Err(RustMetadataError::CrateNotFound(_)) | Err(RustMetadataError::PathNotResolved(_)) => {}
                        Err(err) => return Err(err),
                    }
                    log_timing_stage(
                        timing_enabled,
                        root,
                        canonical_path,
                        "extract.workspace.out_dirs",
                        extract_started.elapsed(),
                        "workspace_hit=false out_dirs=true status=miss",
                    );
                }
                Err(err) => {
                    log_timing_stage(
                        timing_enabled,
                        root,
                        canonical_path,
                        "workspace.load.out_dirs",
                        load_started.elapsed(),
                        "out_dirs=true status=error",
                    );
                    if deferred_load_error.is_none() {
                        deferred_load_error = Some(err);
                    }
                }
            }
        }
    }

    if let Some(err) = deferred_load_error {
        return Err(err);
    }

    Err(RustMetadataError::CrateNotFound(crate_name.to_string()))
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

    /// Return metadata for `canonical_path`, loading/extracting on cache miss.
    ///
    /// Lookup order is:
    /// 1. in-memory exact, definition-path, and spelling-alias hits
    /// 2. workspace extraction using canonical-path candidates
    /// 3. dependency-workspace extraction fallback
    /// 4. persisted disk-cache update for future sessions
    fn get_or_extract_inner(
        &self,
        manifest_dir: &Path,
        canonical_path: &str,
        registry_src_roots: Option<&[PathBuf]>,
        progress: &(dyn Fn(String) + Sync),
        persist_immediately: bool,
    ) -> Result<Arc<RustItemMetadata>, RustMetadataError> {
        let root = manifest_dir.canonicalize()?;
        let timing_enabled = rust_inspect_timing_enabled();
        let mut trace = CallTrace::new(timing_enabled, &root, canonical_path);
        let key_item = (root.clone(), canonical_path.to_owned());

        let mut inner = self.inner.lock().map_err(|e| RustMetadataError::LoadWorkspace {
            path: root.clone(),
            message: format!("metadata cache lock poisoned: {e}"),
        })?;

        let disk_load_started = Instant::now();
        ensure_disk_cache_loaded(&mut inner, &root)?;
        log_timing_stage(
            timing_enabled,
            &root,
            canonical_path,
            "disk_cache.ensure_loaded",
            disk_load_started.elapsed(),
            "",
        );

        if let Some(hit) = inner.items.get(&key_item) {
            trace.set_outcome("hit.memory.exact");
            return Ok(Arc::clone(hit));
        }
        if let Some(hit) = cached_definition_alias(&inner, &root, canonical_path) {
            let arc = insert_aliased_item(&mut inner, &root, canonical_path, &hit);
            let persist_started = Instant::now();
            if persist_immediately
                && let Err(err) = persist_item_to_disk_cache(&inner, &root)
                && timing_enabled
            {
                eprintln!(
                    "[rust-inspect-timing] root={} query={} stage=disk_cache.persist.definition_alias_hit status=error err={err}",
                    root.display(),
                    canonical_path
                );
            }
            log_timing_stage(
                timing_enabled,
                &root,
                canonical_path,
                "disk_cache.persist.definition_alias_hit",
                persist_started.elapsed(),
                if persist_immediately { "" } else { "deferred=true" },
            );
            trace.set_outcome("hit.memory.definition_alias");
            return Ok(arc);
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
                trace.set_outcome("hit.memory.alias");
                return Ok(arc);
            }
            if let Some(miss) = inner.failed_items.get(&candidate_key) {
                last_err = Some(miss.to_error());
                continue;
            }
            match extract_in_workspace_set(
                &mut inner,
                &root,
                candidate.as_str(),
                registry_src_roots,
                progress,
                timing_enabled,
            ) {
                Ok(found) => {
                    meta = Some(found);
                    break;
                }
                Err(err) => {
                    if let Some(negative) = NegativeLookup::from_error(&err) {
                        inner.failed_items.insert(candidate_key, negative);
                    }
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
        trace.set_outcome("hit.extracted");
        Ok(arc)
    }

    /// Return metadata for a canonical Rust path, extracting from the workspace and persisting cache misses.
    pub fn get_or_extract(
        &self,
        manifest_dir: &Path,
        canonical_path: &str,
        progress: &(dyn Fn(String) + Sync),
    ) -> Result<Arc<RustItemMetadata>, RustMetadataError> {
        self.get_or_extract_inner(manifest_dir, canonical_path, None, progress, true)
    }

    /// Return metadata for a canonical Rust path while deferring disk-cache persistence to the caller.
    ///
    /// Prewarm batches extract many items and flush the manifest cache once instead of rewriting it after every item.
    pub(crate) fn get_or_extract_deferred_persist(
        &self,
        manifest_dir: &Path,
        canonical_path: &str,
        progress: &(dyn Fn(String) + Sync),
    ) -> Result<Arc<RustItemMetadata>, RustMetadataError> {
        self.get_or_extract_inner(manifest_dir, canonical_path, None, progress, false)
    }

    /// Persist the in-memory cache snapshot for one manifest root.
    ///
    /// Prewarm uses deferred extraction so callers can batch writes until every requested item has been visited.
    pub(crate) fn persist_manifest_dir(&self, manifest_dir: &Path) -> Result<(), RustMetadataError> {
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

        if let Some(hit) = cached_definition_alias(&inner, &root, canonical_path) {
            let arc = insert_aliased_item(&mut inner, &root, canonical_path, &hit);
            if let Err(err) = persist_item_to_disk_cache(&inner, &root) {
                tracing::warn!(
                    root = %root.display(),
                    query = %canonical_path,
                    error = %err,
                    "failed to persist rust-inspect disk cache after definition alias hit"
                );
                if rust_inspect_timing_enabled() {
                    eprintln!(
                        "[rust-inspect-timing] root={} query={} stage=disk_cache.persist.cached_definition_alias status=error err={err}",
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

    /// Drop all in-memory and disk-cache bookkeeping for one manifest root.
    ///
    /// Use this after filesystem or dependency changes so the next lookup rebuilds fresh alias indexes.
    pub fn invalidate_manifest_dir(&self, manifest_dir: &Path) -> Result<(), RustMetadataError> {
        let root = manifest_dir.canonicalize()?;
        let mut inner = self.inner.lock().map_err(|e| RustMetadataError::LoadWorkspace {
            path: root.clone(),
            message: format!("metadata cache lock poisoned: {e}"),
        })?;
        inner
            .workspaces
            .retain(|(workspace_root, _), _| workspace_root != &root);
        inner.items.retain(|(workspace_root, _), _| workspace_root != &root);
        inner
            .definition_aliases
            .retain(|(workspace_root, _), _| workspace_root != &root);
        inner
            .dependency_manifest_dirs
            .retain(|(workspace_root, _), _| workspace_root != &root);
        inner
            .root_crate_names
            .retain(|workspace_root, _| workspace_root != &root);
        inner
            .crate_reexport_aliases
            .retain(|workspace_root, _| workspace_root != &root);
        inner
            .failed_items
            .retain(|(workspace_root, _), _| workspace_root != &root);
        inner.disk_cache_state.remove(&root);
        Ok(())
    }

    /// Return metadata for tests that need custom registry source roots.
    ///
    /// Production callers should use `get_or_extract`; this hook lets tests use synthetic cargo registry directories.
    #[doc(hidden)]
    pub fn get_or_extract_with_registry_src_roots(
        &self,
        manifest_dir: &Path,
        canonical_path: &str,
        registry_src_roots: &[PathBuf],
        progress: &(dyn Fn(String) + Sync),
    ) -> Result<Arc<RustItemMetadata>, RustMetadataError> {
        self.get_or_extract_inner(manifest_dir, canonical_path, Some(registry_src_roots), progress, true)
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
