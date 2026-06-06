//! In-memory cache: one loaded workspace per manifest directory, plus per-item metadata.
//!
//! The cache is the boundary that keeps rust-analyzer/Cargo extraction out of compiler hot paths. Preparation code may
//! call `get_or_extract`; ordinary semantic/codegen consumers should use cache-only reads through `Inspector::get`.

use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
#[cfg(not(test))]
use std::sync::OnceLock;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use incan_core::interop::{
    RustFieldInfo, RustItemKind, RustItemMetadata, RustTypeInfo, RustTypeMetadataCompleteness, RustTypeShape,
    RustTypeShapePathFallback, RustVariantInfo, RustVisibility, parse_rust_type_shape_text, split_top_level_rust_args,
};
use incan_core::lang::types::collections::{self, CollectionTypeId};
use ra_ap_syntax::{
    AstNode, Edition, SourceFile, SyntaxKind, T,
    ast::{self, HasModuleItem, HasName, HasVisibility},
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
const DISK_CACHE_FORMAT: u32 = 10;
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

/// Hash the workspace files that affect rust-inspect extraction results for this generated Cargo workspace.
fn hash_workspace_fingerprint_inputs(hasher: &mut Sha256, root: &Path) -> Result<(), RustMetadataError> {
    hasher.update(fs::read(root.join("Cargo.toml"))?);
    match fs::read(root.join("Cargo.lock")) {
        Ok(lock) => hasher.update(lock),
        Err(err) if err.kind() == ErrorKind::NotFound => {}
        Err(err) => return Err(err.into()),
    }
    Ok(())
}

/// Accept either the current cache-format fingerprint or the legacy version-labeled fingerprint for one cache file.
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

/// Load valid disk-cache items into memory for one workspace and explain whether the disk state was reusable.
fn load_disk_cache_into_memory(
    inner: &mut CacheInner,
    root: &Path,
) -> Result<(Option<String>, DiskCacheLoadReport), RustMetadataError> {
    let fingerprint = workspace_fingerprint(root)?;
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
    if !disk_cache_fingerprint_matches(root, &envelope, fingerprint.as_str())? {
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
    for (canonical_path, metadata) in envelope.items {
        let mut metadata = metadata;
        metadata.canonical_path = canonical_path;
        insert_cached_item(inner, root, Arc::new(metadata));
    }
    for (canonical_path, miss) in envelope.misses {
        inner.failed_items.insert((root.to_path_buf(), canonical_path), miss);
    }
    Ok((Some(fingerprint), report))
}

/// Ensure the workspace-local disk cache has been loaded once for this process.
fn ensure_disk_cache_loaded(inner: &mut CacheInner, root: &Path) -> Result<DiskCacheLoadReport, RustMetadataError> {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CacheAccessOutcome {
    ExactHit,
    DefinitionAliasHit,
    AliasHit,
    Extracted,
}

impl CacheAccessOutcome {
    /// Return true when an access reused existing cache state rather than extracting metadata.
    pub(crate) fn reused(self) -> bool {
        matches!(self, Self::ExactHit | Self::DefinitionAliasHit | Self::AliasHit)
    }

    /// Return the stable timing-trace label for this cache access outcome.
    fn trace_label(self) -> &'static str {
        match self {
            Self::ExactHit => "hit.memory.exact",
            Self::DefinitionAliasHit => "hit.memory.definition_alias",
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

/// Normalize Cargo package and Rust crate spellings to the cache key used for dependency-route lookups.
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

/// Read and normalize a string field from a Cargo manifest table.
fn manifest_string_field(value: &toml::Value, table: &str, key: &str) -> Option<String> {
    value
        .get(table)
        .and_then(|section| section.get(key))
        .and_then(toml::Value::as_str)
        .map(normalized_crate_cache_key)
}

/// Add dependency crate names from one Cargo manifest dependency table, including renamed `package = "..."`
/// targets that appear in generated dependency workspaces.
fn manifest_dependency_crate_names(manifest: &toml::Value, table: &str, names: &mut HashSet<String>) {
    let Some(deps) = manifest.get(table).and_then(toml::Value::as_table) else {
        return;
    };
    for (key, value) in deps {
        names.insert(normalized_crate_cache_key(key));
        if let Some(package) = value.get("package").and_then(toml::Value::as_str) {
            names.insert(normalized_crate_cache_key(package));
        }
    }
}

/// Load normalized dependency crate names from the crate whose generated `OUT_DIR` Rust is being parsed.
///
/// The generated-source fallback uses this to distinguish local relative paths from external dependency paths while it
/// normalizes syntax-only field and variant metadata.
fn load_dependency_crate_names(root: &Path) -> HashSet<String> {
    let Ok(payload) = fs::read_to_string(root.join("Cargo.toml")) else {
        return HashSet::new();
    };
    let Ok(manifest) = toml::from_str::<toml::Value>(payload.as_str()) else {
        return HashSet::new();
    };
    let mut names = HashSet::new();
    for table in ["dependencies", "dev-dependencies", "build-dependencies"] {
        manifest_dependency_crate_names(&manifest, table, &mut names);
    }
    names
}

/// Load the crate names declared by the generated root workspace so root out-dir extraction only runs for root items.
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

/// Resolve the root crate's library source path from `Cargo.toml`, defaulting to `src/lib.rs`.
fn manifest_lib_source_path(root: &Path, manifest: &toml::Value) -> PathBuf {
    manifest
        .get("lib")
        .and_then(|section| section.get("path"))
        .and_then(toml::Value::as_str)
        .map_or_else(|| root.join("src").join("lib.rs"), |path| root.join(path))
}

/// Convert a rust-analyzer syntax path into ordered textual path segments.
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

/// Extract a plain `pub use crate_name` or `pub use crate_name as alias` mapping from one use tree.
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

/// Return whether a use item is exactly public at crate level, excluding restricted visibility such as `pub(crate)`.
fn use_item_is_plain_public(use_item: &ast::Use) -> bool {
    let mut significant = use_item
        .syntax()
        .descendants_with_tokens()
        .filter_map(|element| element.into_token())
        .filter(|token| !matches!(token.kind(), SyntaxKind::COMMENT | SyntaxKind::WHITESPACE));
    matches!(significant.next().map(|token| token.kind()), Some(T![pub]))
        && matches!(significant.next().map(|token| token.kind()), Some(T![use]))
}

/// Load root-level public crate re-export aliases from a dependency crate's library source.
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

/// Return the canonical target path for a dependency-owned item addressed through a public crate re-export.
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

/// Return the Cargo target directory configured for a generated workspace, falling back to the workspace-local
/// `target` directory when no `.cargo/config.toml` target override is present.
fn cargo_configured_target_dir(root: &Path) -> PathBuf {
    let config_path = root.join(".cargo").join("config.toml");
    let Ok(payload) = fs::read_to_string(config_path) else {
        return root.join("target");
    };
    let Ok(config) = toml::from_str::<toml::Value>(payload.as_str()) else {
        return root.join("target");
    };
    let Some(target_dir) = config
        .get("build")
        .and_then(|build| build.get("target-dir"))
        .and_then(toml::Value::as_str)
    else {
        return root.join("target");
    };
    let path = PathBuf::from(target_dir);
    if path.is_absolute() { path } else { root.join(path) }
}

/// Find generated Rust files under build-script `OUT_DIR` directories that may define metadata for a dependency-owned
/// item referenced through the root generated workspace.
fn generated_out_dir_candidates(root: &Path, dep_root: &Path, crate_name: &str) -> Vec<PathBuf> {
    let target_dir = cargo_configured_target_dir(root);
    let mut crate_names = load_root_crate_names(dep_root);
    crate_names.push(normalized_crate_cache_key(crate_name));
    crate_names.sort();
    crate_names.dedup();
    let mut files = Vec::new();
    for profile in ["debug", "release"] {
        let build_dir = target_dir.join(profile).join("build");
        let Ok(entries) = fs::read_dir(build_dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let file_name = entry.file_name().to_string_lossy().to_string();
            let normalized = normalized_crate_cache_key(file_name.as_str());
            if !crate_names
                .iter()
                .any(|name| normalized == *name || normalized.starts_with(format!("{name}_").as_str()))
            {
                continue;
            }
            let out_dir = entry.path().join("out");
            let Ok(out_entries) = fs::read_dir(out_dir) else {
                continue;
            };
            for out_entry in out_entries.flatten() {
                let path = out_entry.path();
                if path.extension().and_then(|ext| ext.to_str()) == Some("rs") {
                    files.push(path);
                }
            }
        }
    }
    files.sort();
    files.dedup();
    files
}

/// Collect file names from `include!(concat!(env!("OUT_DIR"), "..."))` macro text.
fn generated_include_file_names(text: &str) -> Vec<String> {
    if !text.contains("include!") || !text.contains("OUT_DIR") {
        return Vec::new();
    }
    let mut names = Vec::new();
    let mut in_string = false;
    let mut escaped = false;
    let mut current = String::new();
    for ch in text.chars() {
        if in_string {
            if escaped {
                current.push(ch);
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                if current.ends_with(".rs")
                    && let Some(file_name) = Path::new(current.as_str()).file_name().and_then(|name| name.to_str())
                {
                    names.push(file_name.to_string());
                }
                current.clear();
                in_string = false;
            } else {
                current.push(ch);
            }
            continue;
        }
        if ch == '"' {
            in_string = true;
        }
    }
    names.sort();
    names.dedup();
    names
}

/// Record generated `OUT_DIR` file owners from one module-item list.
fn collect_generated_include_owners<'a>(
    mut items: impl Iterator<Item = ast::Item> + 'a,
    module_path: &[String],
    owners: &mut HashMap<String, Vec<Vec<String>>>,
) {
    for item in items.by_ref() {
        match item {
            ast::Item::MacroCall(macro_call) => {
                for file_name in generated_include_file_names(macro_call.syntax().text().to_string().as_str()) {
                    owners.entry(file_name).or_default().push(module_path.to_vec());
                }
            }
            ast::Item::Module(module) => {
                let Some(name) = module.name() else {
                    continue;
                };
                let mut nested_path = module_path.to_vec();
                nested_path.push(generated_source_name(name.to_string().as_str()));
                let Some(item_list) = module.item_list() else {
                    continue;
                };
                for file_name in generated_include_file_names(item_list.syntax().text().to_string().as_str()) {
                    owners.entry(file_name).or_default().push(nested_path.clone());
                }
                collect_generated_include_owners(item_list.items(), &nested_path, owners);
            }
            _ => {}
        }
    }
}

/// Load generated-file owner modules from the dependency crate source that includes build-script output.
fn load_generated_include_owners(dep_root: &Path) -> HashMap<String, Vec<Vec<String>>> {
    let Ok(payload) = fs::read_to_string(dep_root.join("Cargo.toml")) else {
        return HashMap::new();
    };
    let Ok(manifest) = toml::from_str::<toml::Value>(payload.as_str()) else {
        return HashMap::new();
    };
    let Ok(source) = fs::read_to_string(manifest_lib_source_path(dep_root, &manifest)) else {
        return HashMap::new();
    };
    let parsed = SourceFile::parse(source.as_str(), Edition::CURRENT).tree();
    let mut owners = HashMap::new();
    collect_generated_include_owners(parsed.items(), &[], &mut owners);
    for paths in owners.values_mut() {
        paths.sort();
        paths.dedup();
    }
    owners
}

/// Return the path suffix inside a generated Rust file only when the requested item is owned by that include module.
fn generated_item_suffix_for_owner<'a>(item_segments: &'a [&'a str], owner_path: &[String]) -> Option<&'a [&'a str]> {
    if owner_path.is_empty() {
        return Some(item_segments);
    }
    if item_segments.len() <= owner_path.len() {
        return None;
    }
    let matches_owner = owner_path
        .iter()
        .zip(item_segments)
        .all(|(owner, segment)| owner.trim_start_matches("r#") == segment.trim_start_matches("r#"));
    matches_owner.then_some(&item_segments[owner_path.len()..])
}

/// Return whether rust-analyzer parsed a plain public visibility marker, excluding private and restricted public forms.
fn ast_visibility_is_public(vis: Option<ast::Visibility>) -> bool {
    vis.is_some_and(|visibility| {
        let text = visibility.syntax().text().to_string();
        text.trim() == "pub"
    })
}

/// Normalize raw Rust identifiers from generated source so identity comparisons use the source spelling without `r#`.
fn generated_source_name(name: &str) -> String {
    name.strip_prefix("r#").unwrap_or(name).to_string()
}

/// Return the Rust display base for std/core/alloc/prost generic containers that map onto Incan collection identities.
fn generated_known_collection_base(compact: &str) -> Option<&str> {
    let segments = compact
        .split("::")
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>();
    let tail = segments.last().copied().unwrap_or(compact);
    let id = collections::from_rust_display_base(tail)?;
    if !matches!(
        id,
        CollectionTypeId::Option | CollectionTypeId::Result | CollectionTypeId::List
    ) {
        return None;
    }
    let public_rust_namespace =
        segments.len() == 1 || matches!(segments.first().copied(), Some("core" | "std" | "alloc" | "prost"));
    public_rust_namespace.then_some(tail)
}

/// Convert a generated Rust type path into the same stable display form rust-inspect would report from HIR metadata.
fn generated_type_path_display(
    path: &str,
    crate_name: &str,
    module_path: &[String],
    external_crates: &HashSet<String>,
) -> String {
    let compact = path.trim().trim_start_matches("::").replace(' ', "");
    if compact.is_empty() {
        return compact;
    }
    if let Some(base) = generated_known_collection_base(compact.as_str()) {
        return base.to_string();
    }
    match compact.as_str() {
        "bool" | "f32" | "f64" | "i8" | "i16" | "i32" | "i64" | "i128" | "isize" | "u8" | "u16" | "u32" | "u64"
        | "u128" | "usize" | "str" | "String" | "()" | "[u8]" => return compact,
        "alloc::string::String" | "std::string::String" | "prost::alloc::string::String" => {
            return "String".to_string();
        }
        "alloc::boxed::Box" | "std::boxed::Box" | "prost::alloc::boxed::Box" | "Box" => return "Box".to_string(),
        _ => {}
    }

    let mut segments = compact
        .split("::")
        .filter(|segment| !segment.is_empty())
        .map(generated_source_name)
        .collect::<Vec<_>>();
    if segments.is_empty() {
        return compact;
    }

    if matches!(
        segments.first().map(String::as_str),
        Some("core" | "std" | "alloc" | "prost")
    ) {
        return segments.join("::");
    }
    if segments
        .first()
        .is_some_and(|segment| segment == crate_name || external_crates.contains(segment))
    {
        return segments.join("::");
    }

    if segments.first().is_some_and(|segment| segment == "crate") {
        segments[0] = crate_name.to_string();
        return segments.join("::");
    }

    let mut owner = module_path.to_vec();
    while segments.first().is_some_and(|segment| segment == "super") {
        segments.remove(0);
        owner.pop();
    }
    if segments.first().is_some_and(|segment| segment == "self") {
        segments.remove(0);
    }

    if segments.len() == 1 && segments[0].len() == 1 && segments[0].chars().all(|ch| ch.is_ascii_uppercase()) {
        return segments.remove(0);
    }

    let mut out = Vec::with_capacity(1 + owner.len() + segments.len());
    out.push(crate_name.to_string());
    out.extend(owner);
    out.extend(segments);
    out.join("::")
}

/// Convert a generated Rust type syntax fragment into a normalized type display string, preserving ownership-relevant
/// container shape while removing formatting noise from generated source.
fn generated_type_display(
    text: &str,
    crate_name: &str,
    module_path: &[String],
    external_crates: &HashSet<String>,
) -> String {
    let text = text.trim().replace(['\n', '\r', '\t', ' '], "");
    if let Some(inner) = text.strip_prefix('&') {
        let inner = inner.strip_prefix("mut").unwrap_or(inner);
        return format!(
            "&{}",
            generated_type_display(inner, crate_name, module_path, external_crates)
        );
    }
    if text.starts_with('(') && text.ends_with(')') {
        let inner = &text[1..text.len() - 1];
        if inner.is_empty() {
            return "()".to_string();
        }
        let items = split_top_level_rust_args(inner)
            .into_iter()
            .map(|arg| generated_type_display(arg, crate_name, module_path, external_crates))
            .collect::<Vec<_>>();
        return format!("({})", items.join(","));
    }
    if let Some(start) = text.find('<')
        && text.ends_with('>')
    {
        let base = generated_type_path_display(&text[..start], crate_name, module_path, external_crates);
        let inner = &text[start + 1..text.len() - 1];
        let args = split_top_level_rust_args(inner)
            .into_iter()
            .map(|arg| generated_type_display(arg, crate_name, module_path, external_crates))
            .collect::<Vec<_>>();
        return format!("{base}<{}>", args.join(", "));
    }
    generated_type_path_display(text.as_str(), crate_name, module_path, external_crates)
}

/// Extract public record-field metadata from generated Rust syntax so build-script output can feed the same field
/// lookup path as rust-inspect HIR metadata.
fn generated_field_info(
    field: ast::RecordField,
    crate_name: &str,
    module_path: &[String],
    external_crates: &HashSet<String>,
) -> Option<RustFieldInfo> {
    if !ast_visibility_is_public(field.visibility()) {
        return None;
    }
    let name = generated_source_name(field.name()?.to_string().as_str());
    let type_display = generated_type_display(
        field.ty()?.syntax().text().to_string().as_str(),
        crate_name,
        module_path,
        external_crates,
    );
    let type_shape = generated_type_shape(type_display.as_str());
    Some(RustFieldInfo {
        name,
        type_display,
        type_shape,
    })
}

/// Convert a normalized generated Rust type display into the structural shape used by boundary coercion planning.
fn generated_type_shape(text: &str) -> RustTypeShape {
    parse_rust_type_shape_text(text, |_| None, RustTypeShapePathFallback::RustPath)
}

/// Build metadata for a public generated Rust struct discovered in build-script output.
fn generated_struct_metadata(
    struct_item: ast::Struct,
    crate_name: &str,
    module_path: &[String],
    external_crates: &HashSet<String>,
) -> Option<RustTypeInfo> {
    if !ast_visibility_is_public(struct_item.visibility()) {
        return None;
    }
    let fields = match struct_item.field_list()? {
        ast::FieldList::RecordFieldList(list) => list
            .fields()
            .filter_map(|field| generated_field_info(field, crate_name, module_path, external_crates))
            .collect(),
        _ => Vec::new(),
    };
    Some(RustTypeInfo {
        alias_target: None,
        metadata_completeness: RustTypeMetadataCompleteness::FieldsAndVariantsOnly,
        methods: Vec::new(),
        implemented_traits: Vec::new(),
        fields,
        variants: Vec::new(),
    })
}

/// Remove transparent generated `Box<T>` payload wrappers from enum variant shapes because Incan pattern/coercion logic
/// cares about the semantic payload type, not prost's storage carrier.
fn normalize_generated_variant_payload_shape(shape: RustTypeShape) -> RustTypeShape {
    match shape {
        RustTypeShape::RustPath { path, args }
            if matches!(path.as_str(), "Box" | "std::boxed::Box" | "alloc::boxed::Box") =>
        {
            args.into_iter().next().unwrap_or(RustTypeShape::Unknown)
        }
        other => other,
    }
}

/// Extract tuple variant payload shapes from a generated Rust enum variant.
fn generated_variant_payload_shapes(
    variant: ast::Variant,
    crate_name: &str,
    module_path: &[String],
    external_crates: &HashSet<String>,
) -> Vec<RustTypeShape> {
    let Some(ast::FieldList::TupleFieldList(fields)) = variant.field_list() else {
        return Vec::new();
    };
    fields
        .fields()
        .filter_map(|field| field.ty())
        .map(|ty| {
            let display = generated_type_display(
                ty.syntax().text().to_string().as_str(),
                crate_name,
                module_path,
                external_crates,
            );
            normalize_generated_variant_payload_shape(generated_type_shape(display.as_str()))
        })
        .collect()
}

/// Build metadata for a public generated Rust enum discovered in build-script output.
fn generated_enum_metadata(
    enum_item: ast::Enum,
    crate_name: &str,
    module_path: &[String],
    external_crates: &HashSet<String>,
) -> Option<RustTypeInfo> {
    if !ast_visibility_is_public(enum_item.visibility()) {
        return None;
    }
    let mut variants = enum_item
        .variant_list()?
        .variants()
        .filter_map(|variant| {
            let name = variant.name()?.to_string();
            Some(RustVariantInfo {
                name,
                fields: generated_variant_payload_shapes(variant, crate_name, module_path, external_crates),
            })
        })
        .collect::<Vec<_>>();
    variants.sort_by(|a, b| a.name.cmp(&b.name));
    Some(RustTypeInfo {
        alias_target: None,
        metadata_completeness: RustTypeMetadataCompleteness::FieldsAndVariantsOnly,
        methods: Vec::new(),
        implemented_traits: Vec::new(),
        fields: Vec::new(),
        variants,
    })
}

/// Walk generated Rust syntax items along a module path and return metadata for the requested struct or enum.
fn generated_type_info_in_items<'a>(
    mut items: impl Iterator<Item = ast::Item> + 'a,
    path: &[&str],
    crate_name: &str,
    module_path: &[String],
    external_crates: &HashSet<String>,
) -> Option<RustTypeInfo> {
    let (head, tail) = path.split_first()?;
    for item in items.by_ref() {
        match item {
            ast::Item::Struct(struct_item) if tail.is_empty() => {
                let name = struct_item.name()?.to_string();
                if name.trim_start_matches("r#") == head.trim_start_matches("r#") {
                    return generated_struct_metadata(struct_item, crate_name, module_path, external_crates);
                }
            }
            ast::Item::Enum(enum_item) if tail.is_empty() => {
                let name = enum_item.name()?.to_string();
                if name.trim_start_matches("r#") == head.trim_start_matches("r#") {
                    return generated_enum_metadata(enum_item, crate_name, module_path, external_crates);
                }
            }
            ast::Item::Module(module) if !tail.is_empty() => {
                let name = module.name()?.to_string();
                if name.trim_start_matches("r#") != head.trim_start_matches("r#") {
                    continue;
                }
                let item_list = module.item_list()?;
                let mut nested_module_path = module_path.to_vec();
                nested_module_path.push(generated_source_name(name.as_str()));
                if let Some(info) = generated_type_info_in_items(
                    item_list.items(),
                    tail,
                    crate_name,
                    &nested_module_path,
                    external_crates,
                ) {
                    return Some(info);
                }
            }
            _ => {}
        }
    }
    None
}

/// Parse a generated Rust file and look up metadata for one item path within that file.
fn generated_type_info_from_source(
    source: &str,
    path: &[&str],
    crate_name: &str,
    module_path: &[String],
    external_crates: &HashSet<String>,
) -> Option<RustTypeInfo> {
    let parsed = SourceFile::parse(source, Edition::CURRENT).tree();
    generated_type_info_in_items(parsed.items(), path, crate_name, module_path, external_crates)
}

/// Resolve dependency-owned metadata directly from generated build-script Rust when rust-inspect cannot resolve the
/// item through the dependency crate's normal HIR workspace.
fn generated_out_dir_metadata(root: &Path, dep_root: &Path, canonical_path: &str) -> Option<RustItemMetadata> {
    let mut segments = canonical_path.split("::").filter(|segment| !segment.is_empty());
    let crate_name = segments.next()?;
    let item_segments = segments.collect::<Vec<_>>();
    if item_segments.is_empty() {
        return None;
    }
    let external_crates = load_dependency_crate_names(dep_root);
    let include_owners = load_generated_include_owners(dep_root);
    for generated_file in generated_out_dir_candidates(root, dep_root, crate_name) {
        let Ok(source) = fs::read_to_string(&generated_file) else {
            continue;
        };
        let file_name = generated_file
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default();
        let empty_owner_paths;
        let owner_paths = include_owners.get(file_name).map_or([].as_slice(), Vec::as_slice);
        let owner_paths: &[Vec<String>] = if owner_paths.is_empty() {
            empty_owner_paths = vec![Vec::new()];
            empty_owner_paths.as_slice()
        } else {
            owner_paths
        };
        for module_path in owner_paths {
            let Some(suffix) = generated_item_suffix_for_owner(&item_segments, module_path) else {
                continue;
            };
            if let Some(type_info) =
                generated_type_info_from_source(source.as_str(), suffix, crate_name, module_path, &external_crates)
            {
                return Some(RustItemMetadata {
                    canonical_path: canonical_path.to_string(),
                    definition_path: Some(canonical_path.to_string()),
                    visibility: RustVisibility::Public,
                    kind: RustItemKind::Type(type_info),
                });
            }
        }
    }
    None
}

/// Return whether the generated root workspace itself declares the crate segment used by a query path.
fn root_workspace_declares_crate(inner: &mut CacheInner, root: &Path, crate_name: &str) -> bool {
    let names = inner
        .root_crate_names
        .entry(root.to_path_buf())
        .or_insert_with(|| load_root_crate_names(root));
    names
        .iter()
        .any(|name| name == normalized_crate_cache_key(crate_name).as_str())
}

/// Typed rust-inspect workspace route used for one extraction attempt.
#[derive(Debug)]
enum WorkspaceExtractionRoute {
    Primary,
    DependencyOutDirs { manifest_dir: PathBuf },
    RootOutDirs,
}

impl WorkspaceExtractionRoute {
    /// Return the in-memory workspace-cache key for this route.
    fn key(&self, root: &Path) -> (PathBuf, bool) {
        match self {
            Self::Primary => (root.to_path_buf(), false),
            Self::DependencyOutDirs { manifest_dir } => (manifest_dir.clone(), true),
            Self::RootOutDirs => (root.to_path_buf(), true),
        }
    }

    /// Return the Cargo manifest directory that should be loaded for this route.
    fn manifest_dir<'a>(&'a self, root: &'a Path) -> &'a Path {
        match self {
            Self::Primary | Self::RootOutDirs => root,
            Self::DependencyOutDirs { manifest_dir } => manifest_dir.as_path(),
        }
    }

    /// Return whether build-script `OUT_DIR` output should be included while loading the route workspace.
    fn include_out_dirs(&self) -> bool {
        matches!(self, Self::DependencyOutDirs { .. } | Self::RootOutDirs)
    }

    /// Return the timing stage label for loading this route's workspace.
    fn load_stage(&self) -> &'static str {
        match self {
            Self::Primary => "workspace.load.primary",
            Self::DependencyOutDirs { .. } => "workspace.load.dependency.out_dirs",
            Self::RootOutDirs => "workspace.load.out_dirs",
        }
    }

    /// Return the timing stage label for extracting metadata through this route.
    fn extract_stage(&self) -> &'static str {
        match self {
            Self::Primary => "extract.workspace.primary",
            Self::DependencyOutDirs { .. } => "extract.workspace.dependency",
            Self::RootOutDirs => "extract.workspace.out_dirs",
        }
    }

    /// Return timing detail for a workspace load outcome.
    fn load_detail(&self, status: &str) -> String {
        match self {
            Self::DependencyOutDirs { manifest_dir } => manifest_dir.display().to_string(),
            Self::Primary | Self::RootOutDirs => {
                format!("out_dirs={} status={status}", self.include_out_dirs())
            }
        }
    }

    /// Return timing detail for a successful extraction outcome.
    fn extract_detail(&self, workspace_hit: bool) -> String {
        match self {
            Self::DependencyOutDirs { manifest_dir } => manifest_dir.display().to_string(),
            Self::Primary | Self::RootOutDirs => {
                format!("workspace_hit={workspace_hit} out_dirs={}", self.include_out_dirs())
            }
        }
    }

    /// Return timing detail for an extraction miss.
    fn extract_miss_detail(&self, workspace_hit: bool) -> String {
        match self {
            Self::DependencyOutDirs { manifest_dir } => manifest_dir.display().to_string(),
            Self::Primary | Self::RootOutDirs => {
                format!(
                    "workspace_hit={workspace_hit} out_dirs={} status=miss",
                    self.include_out_dirs()
                )
            }
        }
    }
}

/// Return whether an extraction error is a route miss that can fall through to the next route.
fn metadata_extraction_missed(err: &RustMetadataError) -> bool {
    matches!(
        err,
        RustMetadataError::CrateNotFound(_) | RustMetadataError::PathNotResolved(_)
    )
}

/// Return whether the primary route failed to load and can be deferred until fallback routes are exhausted.
fn workspace_load_failed(err: &RustMetadataError) -> bool {
    matches!(err, RustMetadataError::Io(_) | RustMetadataError::LoadWorkspace { .. })
}

/// Extract metadata through one typed workspace route.
fn extract_from_workspace_route(
    inner: &mut CacheInner,
    root: &Path,
    canonical_path: &str,
    route: WorkspaceExtractionRoute,
    progress: &(dyn Fn(String) + Sync),
    timing_enabled: bool,
) -> Result<RustItemMetadata, RustMetadataError> {
    match inner.workspaces.entry(route.key(root)) {
        Entry::Occupied(o) => {
            let started = Instant::now();
            match extract_rust_item(o.into_mut(), canonical_path) {
                Ok(meta) => {
                    log_timing_stage(
                        timing_enabled,
                        root,
                        canonical_path,
                        route.extract_stage(),
                        started.elapsed(),
                        route.extract_detail(true).as_str(),
                    );
                    return Ok(meta);
                }
                Err(err) if metadata_extraction_missed(&err) => {}
                Err(err) => return Err(err),
            }
            log_timing_stage(
                timing_enabled,
                root,
                canonical_path,
                route.extract_stage(),
                started.elapsed(),
                route.extract_miss_detail(true).as_str(),
            );
        }
        Entry::Vacant(v) => {
            let load_started = Instant::now();
            match RustWorkspace::load_with_options(route.manifest_dir(root), progress, route.include_out_dirs()) {
                Ok(workspace) => {
                    log_timing_stage(
                        timing_enabled,
                        root,
                        canonical_path,
                        route.load_stage(),
                        load_started.elapsed(),
                        route.load_detail("ok").as_str(),
                    );
                    let extract_started = Instant::now();
                    match extract_rust_item(v.insert(workspace), canonical_path) {
                        Ok(meta) => {
                            log_timing_stage(
                                timing_enabled,
                                root,
                                canonical_path,
                                route.extract_stage(),
                                extract_started.elapsed(),
                                route.extract_detail(false).as_str(),
                            );
                            return Ok(meta);
                        }
                        Err(err) if metadata_extraction_missed(&err) => {}
                        Err(err) => return Err(err),
                    }
                    log_timing_stage(
                        timing_enabled,
                        root,
                        canonical_path,
                        route.extract_stage(),
                        extract_started.elapsed(),
                        route.extract_miss_detail(false).as_str(),
                    );
                }
                Err(err) => {
                    log_timing_stage(
                        timing_enabled,
                        root,
                        canonical_path,
                        route.load_stage(),
                        load_started.elapsed(),
                        route.load_detail("error").as_str(),
                    );
                    return Err(err);
                }
            }
        }
    }

    Err(RustMetadataError::PathNotResolved(canonical_path.to_string()))
}

/// Attempt extraction through one planned route: primary workspace, dependency workspace, then root `OUT_DIR`
/// workspace.
///
/// The root `OUT_DIR` route is reached only after the primary route misses and dependency resolution either misses or
/// determines that the queried crate belongs to the generated root workspace.
fn extract_in_workspace_set(
    inner: &mut CacheInner,
    root: &Path,
    canonical_path: &str,
    registry_src_roots: Option<&[PathBuf]>,
    progress: &(dyn Fn(String) + Sync),
    timing_enabled: bool,
) -> Result<RustItemMetadata, RustMetadataError> {
    let mut deferred_load_error = None;

    match extract_from_workspace_route(
        inner,
        root,
        canonical_path,
        WorkspaceExtractionRoute::Primary,
        progress,
        timing_enabled,
    ) {
        Ok(meta) => return Ok(meta),
        Err(err) if metadata_extraction_missed(&err) => {}
        Err(err) if workspace_load_failed(&err) => deferred_load_error = Some(err),
        Err(err) => return Err(err),
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
        match extract_from_workspace_route(
            inner,
            root,
            canonical_path,
            WorkspaceExtractionRoute::DependencyOutDirs {
                manifest_dir: dep_root.clone(),
            },
            progress,
            timing_enabled,
        ) {
            Ok(meta) => return Ok(meta),
            Err(err) if metadata_extraction_missed(&err) => {
                log_timing_stage(
                    timing_enabled,
                    root,
                    canonical_path,
                    "extract.workspace.dependency",
                    std::time::Duration::ZERO,
                    "status=miss root_out_dirs_fallback=true",
                );
                if let Ok(meta) =
                    extract_from_root_out_dir_workspace(inner, root, canonical_path, progress, timing_enabled)
                {
                    return Ok(meta);
                }
                if let Some(meta) = generated_out_dir_metadata(root, &dep_root, canonical_path) {
                    return Ok(meta);
                }
                return Err(err);
            }
            Err(err) => return Err(err),
        }
    }

    if !root_workspace_declares_crate(inner, root, crate_name) {
        if let Some(err) = deferred_load_error {
            return Err(err);
        }
        return Err(RustMetadataError::CrateNotFound(crate_name.to_string()));
    }

    extract_from_root_out_dir_workspace(inner, root, canonical_path, progress, timing_enabled)
}

/// Extract metadata from the root workspace with build-script output directories enabled, preserving the same workspace
/// cache entry for repeated generated dependency lookups.
fn extract_from_root_out_dir_workspace(
    inner: &mut CacheInner,
    root: &Path,
    canonical_path: &str,
    progress: &(dyn Fn(String) + Sync),
    timing_enabled: bool,
) -> Result<RustItemMetadata, RustMetadataError> {
    extract_from_workspace_route(
        inner,
        root,
        canonical_path,
        WorkspaceExtractionRoute::RootOutDirs,
        progress,
        timing_enabled,
    )
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
            let outcome = CacheAccessOutcome::DefinitionAliasHit;
            trace.set_outcome(outcome.trace_label());
            return Ok(CacheAccess { metadata: arc, outcome });
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
