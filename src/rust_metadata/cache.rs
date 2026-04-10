//! In-memory cache: one loaded workspace per manifest directory, plus per-item metadata.

use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::Command;
#[cfg(test)]
use std::sync::OnceLock;
use std::sync::{Arc, Mutex};

use incan_core::interop::RustItemMetadata;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::error::RustMetadataError;
use super::extractor::extract_rust_item;
use super::loader::RustWorkspace;
use crate::version::INCAN_VERSION;

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
    disk_cache_state: HashMap<PathBuf, DiskCacheState>,
}

#[derive(Default)]
struct DiskCacheState {
    loaded: bool,
    workspace_fingerprint: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DiskCacheEnvelope {
    cache_format: u32,
    incan_version: String,
    workspace_fingerprint: String,
    items: HashMap<String, RustItemMetadata>,
}

const DISK_CACHE_FORMAT: u32 = 1;
const DISK_CACHE_FILE: &str = ".incan_rust_metadata_cache.json";

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

#[derive(Deserialize)]
struct CargoLock {
    package: Vec<CargoLockPackage>,
}

#[derive(Deserialize)]
struct CargoLockPackage {
    name: String,
    version: String,
    source: Option<String>,
}

fn normalize_crate_name(name: &str) -> String {
    name.replace('-', "_")
}

fn disk_cache_path(root: &Path) -> PathBuf {
    root.join(DISK_CACHE_FILE)
}

#[cfg(test)]
static TEST_SHARED_CACHE_ROOT: OnceLock<Mutex<Option<PathBuf>>> = OnceLock::new();

fn shared_cache_root() -> Option<PathBuf> {
    #[cfg(test)]
    {
        // Never silently skip the override: a poisoned mutex must not fall through to the real user cache dir.
        let root = TEST_SHARED_CACHE_ROOT
            .get_or_init(|| Mutex::new(None))
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        if let Some(path) = root {
            return Some(path);
        }
    }

    if let Some(path) = std::env::var_os("INCAN_RUST_METADATA_SHARED_CACHE_DIR") {
        return Some(PathBuf::from(path));
    }
    #[cfg(target_os = "macos")]
    {
        let home = std::env::var_os("HOME")?;
        Some(
            PathBuf::from(home)
                .join("Library")
                .join("Caches")
                .join("incan")
                .join("rust_metadata"),
        )
    }
    #[cfg(not(target_os = "macos"))]
    {
        if let Some(xdg) = std::env::var_os("XDG_CACHE_HOME") {
            return Some(PathBuf::from(xdg).join("incan").join("rust_metadata"));
        }
        let home = std::env::var_os("HOME")?;
        Some(PathBuf::from(home).join(".cache").join("incan").join("rust_metadata"))
    }
}

fn shared_disk_cache_path(fingerprint: &str) -> Option<PathBuf> {
    Some(shared_cache_root()?.join(format!("{fingerprint}.json")))
}

fn workspace_fingerprint(root: &Path) -> Result<String, RustMetadataError> {
    let mut hasher = Sha256::new();
    hasher.update(format!("cache_format:{DISK_CACHE_FORMAT}\n").as_bytes());
    hasher.update(format!("incan_version:{INCAN_VERSION}\n").as_bytes());
    hasher.update(fs::read(root.join("Cargo.toml"))?);
    match fs::read(root.join("Cargo.lock")) {
        Ok(lock) => hasher.update(lock),
        Err(err) if err.kind() == ErrorKind::NotFound => {}
        Err(err) => return Err(err.into()),
    }
    Ok(hex::encode(hasher.finalize()))
}

fn read_json_cache(path: &Path) -> Result<Option<DiskCacheEnvelope>, RustMetadataError> {
    let payload = match fs::read_to_string(path) {
        Ok(payload) => payload,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err.into()),
    };
    match serde_json::from_str::<DiskCacheEnvelope>(&payload) {
        Ok(envelope) => Ok(Some(envelope)),
        Err(_) => Ok(None),
    }
}

fn read_disk_cache(root: &Path, fingerprint: &str) -> Result<Option<DiskCacheEnvelope>, RustMetadataError> {
    let cache_path = disk_cache_path(root);
    if cache_path.is_file() {
        // Local cache file exists: it is authoritative. Malformed JSON is a hard miss for this workspace — do **not**
        // fall back to the shared fingerprint cache, or identical probe `Cargo.toml` fingerprints could repopulate
        // entries from `~/Library/Caches/...` / `$XDG_CACHE_HOME` after a corrupted local file (flaky tests, confusing
        // IDE behavior).
        return read_json_cache(&cache_path);
    }
    let Some(shared_path) = shared_disk_cache_path(fingerprint) else {
        return Ok(None);
    };
    read_json_cache(&shared_path)
}

fn write_json_cache(path: &Path, envelope: &DiskCacheEnvelope) -> Result<(), RustMetadataError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp_path = path.with_extension("tmp");
    let payload = serde_json::to_vec_pretty(envelope).map_err(|err| RustMetadataError::LoadWorkspace {
        path: path.to_path_buf(),
        message: format!("failed to serialize rust-metadata disk cache: {err}"),
    })?;
    fs::write(&tmp_path, payload)?;
    fs::rename(tmp_path, path)?;
    Ok(())
}

fn write_disk_cache(root: &Path, envelope: &DiskCacheEnvelope) -> Result<(), RustMetadataError> {
    let cache_path = disk_cache_path(root);
    write_json_cache(&cache_path, envelope)?;
    if let Some(shared_path) = shared_disk_cache_path(envelope.workspace_fingerprint.as_str()) {
        let _ = write_json_cache(&shared_path, envelope);
    }
    Ok(())
}

/// Resolve the manifest directory for a dependency crate reachable from `root`.
///
/// Cargo package names may use `-` while Rust import paths use `_`, so lookup normalizes both the package name and
/// the target/lib names before matching.
fn dependency_manifest_dir_from_cargo_metadata(root: &Path, crate_name: &str) -> Option<PathBuf> {
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

fn cargo_registry_src_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();

    if let Some(cargo_home) = std::env::var_os("CARGO_HOME") {
        roots.push(PathBuf::from(cargo_home).join("registry").join("src"));
    }

    if let Some(home) = std::env::var_os("HOME") {
        let default_root = PathBuf::from(home).join(".cargo").join("registry").join("src");
        if !roots.contains(&default_root) {
            roots.push(default_root);
        }
    }

    roots
}

/// Resolve a registry package source directory from a `Cargo.lock` entry.
///
/// Generated rust-metadata lock workspaces may know the exact locked package version without having a fully populated
/// rust-analyzer crate graph. This fallback bridges from Cargo's package identity (`foo-bar`) back to the local
/// downloaded crate source so extractor queries can still load metadata for Rust import paths like `foo_bar::...`.
fn dependency_manifest_dir_from_lock_with_search_roots(
    root: &Path,
    crate_name: &str,
    registry_src_roots: &[PathBuf],
) -> Option<PathBuf> {
    let lock_path = root.join("Cargo.lock");
    let lock: CargoLock = toml::from_str(fs::read_to_string(lock_path).ok()?.as_str()).ok()?;
    let normalized = normalize_crate_name(crate_name);

    for pkg in lock.package {
        if normalize_crate_name(pkg.name.as_str()) != normalized {
            continue;
        }
        if !pkg
            .source
            .as_deref()
            .is_some_and(|source| source.starts_with("registry+"))
        {
            continue;
        }
        let dir_name = format!("{}-{}", pkg.name, pkg.version);
        for root in registry_src_roots {
            let Ok(entries) = fs::read_dir(root) else {
                continue;
            };
            for entry in entries.flatten() {
                let candidate = entry.path().join(dir_name.as_str());
                if candidate.join("Cargo.toml").is_file() {
                    return Some(candidate);
                }
            }
        }
    }

    None
}

fn dependency_manifest_dir_from_lock(
    root: &Path,
    crate_name: &str,
    registry_src_roots: Option<&[PathBuf]>,
) -> Option<PathBuf> {
    let owned_roots;
    let search_roots = if let Some(roots) = registry_src_roots {
        roots
    } else {
        owned_roots = cargo_registry_src_roots();
        &owned_roots
    };
    dependency_manifest_dir_from_lock_with_search_roots(root, crate_name, search_roots)
}

fn dependency_manifest_dir_for_crate(
    root: &Path,
    crate_name: &str,
    registry_src_roots: Option<&[PathBuf]>,
) -> Option<PathBuf> {
    dependency_manifest_dir_from_cargo_metadata(root, crate_name)
        .or_else(|| dependency_manifest_dir_from_lock(root, crate_name, registry_src_roots))
}

fn load_disk_cache_into_memory(inner: &mut CacheInner, root: &Path) -> Result<Option<String>, RustMetadataError> {
    let fingerprint = workspace_fingerprint(root)?;
    let Some(envelope) = read_disk_cache(root, fingerprint.as_str())? else {
        return Ok(Some(fingerprint));
    };
    if envelope.cache_format != DISK_CACHE_FORMAT
        || envelope.incan_version != INCAN_VERSION
        || envelope.workspace_fingerprint != fingerprint
    {
        return Ok(Some(fingerprint));
    }
    for (canonical_path, metadata) in envelope.items {
        inner
            .items
            .insert((root.to_path_buf(), canonical_path), Arc::new(metadata));
    }
    Ok(Some(fingerprint))
}

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

fn persist_item_to_disk_cache(
    inner: &CacheInner,
    root: &Path,
    metadata: &RustItemMetadata,
) -> Result<(), RustMetadataError> {
    let fingerprint = inner
        .disk_cache_state
        .get(root)
        .and_then(|state| state.workspace_fingerprint.clone())
        .unwrap_or(workspace_fingerprint(root)?);
    let mut items = HashMap::new();
    for ((item_root, canonical_path), cached) in &inner.items {
        if item_root == root {
            items.insert(canonical_path.clone(), (*cached.as_ref()).clone());
        }
    }
    items.insert(metadata.canonical_path.clone(), metadata.clone());
    let envelope = DiskCacheEnvelope {
        cache_format: DISK_CACHE_FORMAT,
        incan_version: INCAN_VERSION.to_string(),
        workspace_fingerprint: fingerprint,
        items,
    };
    write_disk_cache(root, &envelope)
}

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
    registry_src_roots: Option<&[PathBuf]>,
    progress: &(dyn Fn(String) + Sync),
) -> Result<RustItemMetadata, RustMetadataError> {
    let mut deferred_load_error = None;

    match inner.workspaces.entry((root.to_path_buf(), false)) {
        Entry::Occupied(o) => match extract_rust_item(o.into_mut().db(), canonical_path) {
            Ok(meta) => return Ok(meta),
            Err(RustMetadataError::CrateNotFound(_)) | Err(RustMetadataError::PathNotResolved(_)) => {}
            Err(err) => return Err(err),
        },
        Entry::Vacant(v) => match RustWorkspace::load(root, progress) {
            Ok(workspace) => match extract_rust_item(v.insert(workspace).db(), canonical_path) {
                Ok(meta) => return Ok(meta),
                Err(RustMetadataError::CrateNotFound(_)) | Err(RustMetadataError::PathNotResolved(_)) => {}
                Err(err) => return Err(err),
            },
            Err(err) => deferred_load_error = Some(err),
        },
    }

    match inner.workspaces.entry((root.to_path_buf(), true)) {
        Entry::Occupied(o) => match extract_rust_item(o.into_mut().db(), canonical_path) {
            Ok(meta) => return Ok(meta),
            Err(RustMetadataError::CrateNotFound(_)) | Err(RustMetadataError::PathNotResolved(_)) => {}
            Err(err) => return Err(err),
        },
        Entry::Vacant(v) => match RustWorkspace::load_with_options(root, progress, true) {
            Ok(workspace) => match extract_rust_item(v.insert(workspace).db(), canonical_path) {
                Ok(meta) => return Ok(meta),
                Err(RustMetadataError::CrateNotFound(_)) | Err(RustMetadataError::PathNotResolved(_)) => {}
                Err(err) => return Err(err),
            },
            Err(err) => {
                if deferred_load_error.is_none() {
                    deferred_load_error = Some(err);
                }
            }
        },
    }

    let crate_name = crate_name_for_path(canonical_path);
    if let Some(dep_root) = dependency_manifest_dir_for_crate(root, crate_name, registry_src_roots) {
        let dep_workspace = match inner.workspaces.entry((dep_root.clone(), true)) {
            Entry::Occupied(o) => o.into_mut(),
            Entry::Vacant(v) => v.insert(RustWorkspace::load_with_options(&dep_root, progress, true)?),
        };
        return extract_rust_item(dep_workspace.db(), canonical_path);
    }

    if let Some(err) = deferred_load_error {
        return Err(err);
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
    fn get_or_extract_inner(
        &self,
        manifest_dir: &Path,
        canonical_path: &str,
        registry_src_roots: Option<&[PathBuf]>,
        progress: &(dyn Fn(String) + Sync),
    ) -> Result<Arc<RustItemMetadata>, RustMetadataError> {
        let root = manifest_dir.canonicalize()?;
        let key_item = (root.clone(), canonical_path.to_owned());

        let mut inner = self.inner.lock().map_err(|e| RustMetadataError::LoadWorkspace {
            path: root.clone(),
            message: format!("metadata cache lock poisoned: {e}"),
        })?;

        ensure_disk_cache_loaded(&mut inner, &root)?;

        if let Some(hit) = inner.items.get(&key_item) {
            return Ok(Arc::clone(hit));
        }

        let mut last_err = None;
        let mut meta = None;
        for candidate in canonical_path_candidates(canonical_path) {
            let candidate_key = (root.clone(), candidate.clone());
            if let Some(hit) = inner.items.get(&candidate_key) {
                let mut aliased = (*hit.as_ref()).clone();
                aliased.canonical_path = canonical_path.to_owned();
                let arc = Arc::new(aliased);
                inner.items.insert(key_item.clone(), Arc::clone(&arc));
                let _ = persist_item_to_disk_cache(&inner, &root, arc.as_ref());
                return Ok(arc);
            }
            match extract_in_workspace_set(&mut inner, &root, candidate.as_str(), registry_src_roots, progress) {
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
        let _ = persist_item_to_disk_cache(&inner, &root, arc.as_ref());
        Ok(arc)
    }

    pub fn get_or_extract(
        &self,
        manifest_dir: &Path,
        canonical_path: &str,
        progress: &(dyn Fn(String) + Sync),
    ) -> Result<Arc<RustItemMetadata>, RustMetadataError> {
        self.get_or_extract_inner(manifest_dir, canonical_path, None, progress)
    }

    #[cfg(test)]
    pub(crate) fn get_or_extract_with_registry_src_roots(
        &self,
        manifest_dir: &Path,
        canonical_path: &str,
        registry_src_roots: &[PathBuf],
        progress: &(dyn Fn(String) + Sync),
    ) -> Result<Arc<RustItemMetadata>, RustMetadataError> {
        self.get_or_extract_inner(manifest_dir, canonical_path, Some(registry_src_roots), progress)
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

#[cfg(test)]
mod tests {
    use super::*;
    use incan_core::interop::{RustItemKind, RustTypeInfo, RustVisibility};
    use std::sync::MutexGuard;

    static TEST_SHARED_CACHE_SERIAL: OnceLock<Mutex<()>> = OnceLock::new();

    fn set_test_shared_cache_root(path: Option<PathBuf>) {
        let mut guard = TEST_SHARED_CACHE_ROOT
            .get_or_init(|| Mutex::new(None))
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *guard = path;
    }

    struct TestSharedCacheRootGuard<'a> {
        _serial: MutexGuard<'a, ()>,
    }

    impl<'a> TestSharedCacheRootGuard<'a> {
        fn new(path: Option<PathBuf>) -> Self {
            let serial = TEST_SHARED_CACHE_SERIAL
                .get_or_init(|| Mutex::new(()))
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            set_test_shared_cache_root(path);
            Self { _serial: serial }
        }
    }

    impl Drop for TestSharedCacheRootGuard<'_> {
        fn drop(&mut self) {
            set_test_shared_cache_root(None);
        }
    }

    fn dummy_type_metadata(path: &str) -> RustItemMetadata {
        RustItemMetadata {
            canonical_path: path.to_string(),
            definition_path: None,
            visibility: RustVisibility::Public,
            kind: RustItemKind::Type(RustTypeInfo {
                methods: Vec::new(),
                fields: Vec::new(),
                variants: Vec::new(),
            }),
        }
    }

    #[test]
    fn lockfile_registry_fallback_resolves_hyphenated_package_for_underscored_crate_name()
    -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let root = tmp.path().join("generated_lock");
        fs::create_dir_all(root.join("src"))?;
        fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"probe\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )?;
        fs::write(
            root.join("Cargo.lock"),
            r#"version = 3

[[package]]
name = "foo-bar"
version = "0.1.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
"#,
        )?;

        let registry_src_root = tmp.path().join("cargo-home").join("registry").join("src");
        let dep_dir = registry_src_root.join("index.crates.io-test").join("foo-bar-0.1.0");
        fs::create_dir_all(dep_dir.join("src"))?;
        fs::write(
            dep_dir.join("Cargo.toml"),
            r#"[package]
name = "foo-bar"
version = "0.1.0"
edition = "2021"

[lib]
name = "foo_bar"
"#,
        )?;
        fs::write(dep_dir.join("src/lib.rs"), "pub fn consume() {}\n")?;

        let resolved = dependency_manifest_dir_from_lock_with_search_roots(&root, "foo_bar", &[registry_src_root])
            .ok_or_else(|| std::io::Error::other("expected Cargo.lock fallback to resolve foo-bar source dir"))?;
        assert_eq!(resolved, dep_dir);
        Ok(())
    }

    #[test]
    fn disk_cache_round_trips_inserted_items() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        fs::write(
            tmp.path().join("Cargo.toml"),
            "[package]\nname = \"probe\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )?;
        let cache = RustMetadataCache::new();
        cache.insert_test_item(tmp.path(), dummy_type_metadata("demo::Thing"))?;
        {
            let inner = cache
                .inner
                .lock()
                .map_err(|_| std::io::Error::other("poisoned cache"))?;
            persist_item_to_disk_cache(&inner, tmp.path(), &dummy_type_metadata("demo::Thing"))?;
        }

        let payload = fs::read_to_string(disk_cache_path(tmp.path()))?;
        assert!(payload.contains("\"demo::Thing\""));

        let cache = RustMetadataCache::new();
        let meta = cache.get_or_extract(tmp.path(), "demo::Thing", &|_| ())?;
        assert_eq!(meta.canonical_path, "demo::Thing");
        Ok(())
    }

    #[test]
    fn disk_cache_invalidates_when_workspace_fingerprint_changes() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        fs::write(
            tmp.path().join("Cargo.toml"),
            "[package]\nname = \"probe\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )?;
        let fingerprint = workspace_fingerprint(tmp.path())?;
        write_disk_cache(
            tmp.path(),
            &DiskCacheEnvelope {
                cache_format: DISK_CACHE_FORMAT,
                incan_version: INCAN_VERSION.to_string(),
                workspace_fingerprint: fingerprint,
                items: HashMap::from([("demo::Thing".to_string(), dummy_type_metadata("demo::Thing"))]),
            },
        )?;

        fs::write(
            tmp.path().join("Cargo.toml"),
            "[package]\nname = \"probe_changed\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )?;

        let mut inner = CacheInner::default();
        ensure_disk_cache_loaded(&mut inner, tmp.path())?;
        assert!(
            !inner
                .items
                .contains_key(&(tmp.path().canonicalize()?, "demo::Thing".to_string()))
        );
        Ok(())
    }

    #[test]
    fn malformed_disk_cache_is_treated_as_miss() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let isolated_shared = tempfile::tempdir()?;
        let _shared_root = TestSharedCacheRootGuard::new(Some(isolated_shared.path().to_path_buf()));
        fs::write(
            tmp.path().join("Cargo.toml"),
            "[package]\nname = \"incan_test_malformed_rust_metadata_disk_cache\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )?;
        fs::write(disk_cache_path(tmp.path()), "{ definitely not json")?;
        let mut inner = CacheInner::default();
        ensure_disk_cache_loaded(&mut inner, tmp.path())?;
        assert!(inner.items.is_empty());
        Ok(())
    }

    #[test]
    fn raw_identifier_alias_hits_existing_cached_item() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        fs::write(
            tmp.path().join("Cargo.toml"),
            "[package]\nname = \"probe\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )?;

        let cache = RustMetadataCache::new();
        cache.insert_test_item(
            tmp.path(),
            RustItemMetadata {
                canonical_path: "incan_stdlib::async::sync::RawSemaphore".to_string(),
                definition_path: Some("incan_stdlib::r#async::sync::Semaphore".to_string()),
                visibility: RustVisibility::Public,
                kind: RustItemKind::Type(RustTypeInfo {
                    methods: Vec::new(),
                    fields: Vec::new(),
                    variants: Vec::new(),
                }),
            },
        )?;

        let hit = cache.get_or_extract(tmp.path(), "incan_stdlib::r#async::sync::RawSemaphore", &|_| ())?;
        assert_eq!(hit.canonical_path, "incan_stdlib::r#async::sync::RawSemaphore");
        Ok(())
    }

    #[test]
    fn shared_fingerprint_cache_reuses_metadata_across_equivalent_workspaces() -> Result<(), Box<dyn std::error::Error>>
    {
        let shared = tempfile::tempdir()?;
        let _shared_root = TestSharedCacheRootGuard::new(Some(shared.path().to_path_buf()));

        let root_a = tempfile::tempdir()?;
        let root_b = tempfile::tempdir()?;
        let manifest = "[package]\nname = \"probe\"\nversion = \"0.1.0\"\nedition = \"2021\"\n";
        fs::write(root_a.path().join("Cargo.toml"), manifest)?;
        fs::write(root_b.path().join("Cargo.toml"), manifest)?;

        let cache_a = RustMetadataCache::new();
        let seeded = RustItemMetadata {
            canonical_path: "demo::Thing".to_string(),
            definition_path: None,
            visibility: RustVisibility::Public,
            kind: RustItemKind::Type(RustTypeInfo {
                methods: Vec::new(),
                fields: Vec::new(),
                variants: Vec::new(),
            }),
        };
        cache_a.insert_test_item(root_a.path(), seeded.clone())?;
        {
            let mut inner = cache_a
                .inner
                .lock()
                .map_err(|_| std::io::Error::other("poisoned cache"))?;
            ensure_disk_cache_loaded(&mut inner, root_a.path())?;
            persist_item_to_disk_cache(&inner, root_a.path(), &seeded)?;
        }

        let cache_b = RustMetadataCache::new();
        let hit = cache_b.get_or_extract(root_b.path(), "demo::Thing", &|_| ())?;
        assert_eq!(hit.canonical_path, "demo::Thing");
        Ok(())
    }
}
