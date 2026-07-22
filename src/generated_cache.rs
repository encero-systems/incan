//! Managed storage for Cargo artifacts produced by generated Rust projects.
//!
//! Generated source remains project-local. This module only shares Cargo's rebuildable `target` data across compatible
//! Incan projects and worktrees, then bounds that shared data with lease-aware pruning.

use std::collections::BTreeMap;
use std::env;
use std::fs::{self, File, OpenOptions, TryLockError};
use std::io::{self, ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::backend::project::generator::{GENERATED_CARGO_TARGET_DIR_ENV, cargo_config_identity};
use crate::backend::project::runner::cargo_executable;
use crate::lockfile::CargoFeatureSelection;

pub(crate) const GENERATED_CACHE_MAX_BYTES_ENV: &str = "INCAN_GENERATED_CACHE_MAX_BYTES";
pub(crate) const GENERATED_CACHE_MAX_ENTRY_BYTES_ENV: &str = "INCAN_GENERATED_CACHE_MAX_ENTRY_BYTES";
pub(crate) const GENERATED_CACHE_ENABLED_ENV: &str = "INCAN_GENERATED_CACHE";
const DEFAULT_GENERATED_CACHE_MAX_BYTES: u64 = 20 * 1024 * 1024 * 1024;
const DEFAULT_GENERATED_CACHE_MAX_ENTRY_BYTES: u64 = DEFAULT_GENERATED_CACHE_MAX_BYTES;
const CACHE_LAYOUT_VERSION: &str = "v1";
const CACHE_METADATA_FILE: &str = "entry.json";
const CACHE_ACTIVE_LOCK_FILE: &str = ".active.lock";
const CACHE_MANAGER_LOCK_FILE: &str = ".manager.lock";
const CACHE_MEASUREMENT_INTERVAL_SECONDS: u64 = 10;
const CACHE_CATEGORY: &str = "generated-cargo";
const CACHE_REPORT_SCHEMA_VERSION: u32 = 1;
const RUST_BACKEND_IDENTITY_ENV: &[&str] = &[
    "RUSTC",
    "RUSTC_WRAPPER",
    "RUSTC_WORKSPACE_WRAPPER",
    "RUSTUP_TOOLCHAIN",
    "CARGO",
    "CARGO_BUILD_TARGET",
    "RUSTFLAGS",
    "CARGO_ENCODED_RUSTFLAGS",
];

/// One resolved generated-project Cargo target and its optional active-use lease.
pub(crate) struct GeneratedCargoTarget {
    path: PathBuf,
    lease: Option<GeneratedCacheLease>,
    identity: Option<String>,
}

impl GeneratedCargoTarget {
    /// Split the selected path from the lease that must remain alive while Cargo uses it.
    pub(crate) fn into_parts(self) -> (PathBuf, Option<GeneratedCacheLease>, Option<String>) {
        (self.path, self.lease, self.identity)
    }
}

/// Shared advisory lock proving that one managed cache entry is in active use.
pub(crate) struct GeneratedCacheLease {
    file: Option<File>,
    entry_root: PathBuf,
    max_entry_bytes: u64,
    finalized: bool,
}

impl Drop for GeneratedCacheLease {
    fn drop(&mut self) {
        if self.finalized {
            return;
        }
        self.release_activity_lock();
        let _ = self.refresh_size_if_idle(false);
    }
}

impl GeneratedCacheLease {
    /// Finish one compiler-owned Cargo operation before user code may continue outside the cache lease.
    pub(crate) fn finish(mut self) -> io::Result<()> {
        self.release_activity_lock();
        let result = self.refresh_size_if_idle(true);
        self.finalized = true;
        result
    }

    /// Release this process's shared activity descriptor before testing whether the domain is now idle.
    fn release_activity_lock(&mut self) {
        if let Some(file) = self.file.take() {
            let _ = file.unlock();
            drop(file);
        }
    }

    /// Refresh metadata only when no other process is actively using this compatibility domain.
    fn refresh_size_if_idle(&self, force: bool) -> io::Result<()> {
        let Ok(active_file) = open_active_lock(&self.entry_root) else {
            return Ok(());
        };
        match active_file.try_lock() {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => return Ok(()),
            Err(TryLockError::Error(error)) => return Err(error),
        }
        let metadata_path = self.entry_root.join(CACHE_METADATA_FILE);
        let Ok(payload) = fs::read(&metadata_path) else {
            return Ok(());
        };
        let Ok(existing) = serde_json::from_slice::<CacheEntryMetadata>(&payload) else {
            return Ok(());
        };
        let measured_at = now_unix_seconds();
        if !force
            && measured_at.saturating_sub(existing.last_measured_unix_seconds) < CACHE_MEASUREMENT_INTERVAL_SECONDS
        {
            return Ok(());
        }
        let logical_bytes = measure_and_enforce_entry_bound(&self.entry_root, self.max_entry_bytes)?;
        let payload = fs::read(metadata_path)?;
        let mut metadata = serde_json::from_slice::<CacheEntryMetadata>(&payload).map_err(io::Error::other)?;
        metadata.logical_bytes = logical_bytes;
        metadata.last_used_unix_seconds = measured_at;
        metadata.last_measured_unix_seconds = measured_at;
        write_metadata(&self.entry_root, &metadata)?;
        Ok(())
    }
}

/// Stable metadata recorded beside one compatibility-domain target directory.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct CacheEntryMetadata {
    identity: String,
    incan_version: String,
    rust_backend_identity: String,
    profile: String,
    lock_digest: String,
    cargo_features: CargoFeatureSelection,
    #[serde(default)]
    cargo_flags: Vec<String>,
    last_used_unix_seconds: u64,
    #[serde(default)]
    logical_bytes: u64,
    #[serde(default)]
    last_measured_unix_seconds: u64,
}

/// One cache entry exposed by inspection and pruning commands.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct GeneratedCacheEntry {
    pub(crate) category: String,
    pub(crate) identity: String,
    pub(crate) path: PathBuf,
    pub(crate) bytes: u64,
    pub(crate) last_used_unix_seconds: u64,
    pub(crate) profile: String,
    pub(crate) active: bool,
}

/// Snapshot of the managed generated-build cache.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct GeneratedCacheInspection {
    pub(crate) schema_version: u32,
    pub(crate) root: PathBuf,
    pub(crate) max_bytes: u64,
    pub(crate) total_bytes: u64,
    pub(crate) entries: Vec<GeneratedCacheEntry>,
}

/// Result of a generated-cache prune operation.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct GeneratedCachePruneReport {
    pub(crate) schema_version: u32,
    pub(crate) root: PathBuf,
    pub(crate) max_bytes: u64,
    pub(crate) before_bytes: u64,
    pub(crate) after_bytes: u64,
    pub(crate) removed_logical_bytes: u64,
    pub(crate) removed_entries: Vec<String>,
    pub(crate) skipped_active_entries: Vec<String>,
    pub(crate) requested_identities: Vec<String>,
    pub(crate) not_found_identities: Vec<String>,
    pub(crate) dry_run: bool,
}

/// Resolve an explicit target override or acquire the default managed compatibility-domain target.
pub(crate) fn resolve_generated_cargo_target(
    explicit_override: Option<&Path>,
    project_root: &Path,
    cargo_working_dir: &Path,
    generated_package_name: &str,
    profile: &str,
    lock_payload: Option<&str>,
    cargo_features: &CargoFeatureSelection,
    cargo_flags: &[String],
) -> io::Result<GeneratedCargoTarget> {
    let environment_override = env::var_os(GENERATED_CARGO_TARGET_DIR_ENV)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from);
    if let Some(path) = explicit_override.or(environment_override.as_deref()) {
        return Ok(GeneratedCargoTarget {
            path: resolve_path(path),
            lease: None,
            identity: None,
        });
    }
    if !generated_cache_enabled(env::var(GENERATED_CACHE_ENABLED_ENV).ok().as_deref()) {
        return Ok(GeneratedCargoTarget {
            path: absolute_project_root(project_root).join("target"),
            lease: None,
            identity: None,
        });
    }

    let Some(cache_root) = default_generated_cache_root(env::var_os("INCAN_HOME"), user_home()) else {
        return Ok(GeneratedCargoTarget {
            path: absolute_project_root(project_root).join("target"),
            lease: None,
            identity: None,
        });
    };
    let max_bytes = configured_max_bytes(env::var(GENERATED_CACHE_MAX_BYTES_ENV).ok().as_deref())?;
    let max_entry_bytes = configured_max_entry_bytes(env::var(GENERATED_CACHE_MAX_ENTRY_BYTES_ENV).ok().as_deref())?;
    validate_managed_cargo_flags(cargo_flags)?;
    let rust_backend_identity = rust_backend_identity(cargo_working_dir)?;
    let metadata = cache_entry_metadata(
        generated_package_name,
        profile,
        lock_payload,
        cargo_features,
        cargo_flags,
        &rust_backend_identity,
    )?;
    acquire_managed_target(&cache_root, max_bytes, max_entry_bytes, metadata)
}

/// Inspect the default managed cache without mutating it.
pub(crate) fn inspect_default_cache() -> io::Result<GeneratedCacheInspection> {
    let cache_root = require_default_cache_root()?;
    let max_bytes = configured_max_bytes(env::var(GENERATED_CACHE_MAX_BYTES_ENV).ok().as_deref())?;
    inspect_cache_root(&cache_root, max_bytes)
}

/// Prune the default managed cache to the configured or caller-supplied byte limit.
pub(crate) fn prune_default_cache(
    max_bytes: Option<u64>,
    dry_run: bool,
    identities: &[String],
) -> io::Result<GeneratedCachePruneReport> {
    let cache_root = require_default_cache_root()?;
    let max_bytes = match max_bytes {
        Some(max_bytes) => max_bytes,
        None => configured_max_bytes(env::var(GENERATED_CACHE_MAX_BYTES_ENV).ok().as_deref())?,
    };
    prune_cache_root(&cache_root, max_bytes, dry_run, identities)
}

/// Resolve the user-shared generated-cache root below `INCAN_HOME`.
fn default_generated_cache_root(
    incan_home: Option<std::ffi::OsString>,
    user_home: Option<std::ffi::OsString>,
) -> Option<PathBuf> {
    incan_home
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            user_home
                .filter(|path| !path.is_empty())
                .map(|path| PathBuf::from(path).join(".incan"))
        })
        .map(|root| root.join("cache").join("generated-cargo").join(CACHE_LAYOUT_VERSION))
}

/// Return the platform home environment used by installed Incan binaries.
fn user_home() -> Option<std::ffi::OsString> {
    env::var_os("HOME").or_else(|| env::var_os("USERPROFILE"))
}

/// Require a resolvable default cache root for an explicit cache-management command.
fn require_default_cache_root() -> io::Result<PathBuf> {
    default_generated_cache_root(env::var_os("INCAN_HOME"), user_home()).ok_or_else(|| {
        io::Error::new(
            ErrorKind::NotFound,
            "cannot resolve the generated cache root; set INCAN_HOME or HOME",
        )
    })
}

/// Parse the cache enable switch while keeping managed reuse on by default.
fn generated_cache_enabled(raw: Option<&str>) -> bool {
    !raw.is_some_and(|value| matches!(value.trim().to_ascii_lowercase().as_str(), "0" | "false" | "no" | "off"))
}

/// Parse the cache size limit in bytes.
fn configured_max_bytes(raw: Option<&str>) -> io::Result<u64> {
    match raw {
        None | Some("") => Ok(DEFAULT_GENERATED_CACHE_MAX_BYTES),
        Some(raw) => raw.parse::<u64>().map_err(|error| {
            io::Error::new(
                ErrorKind::InvalidInput,
                format!("invalid {GENERATED_CACHE_MAX_BYTES_ENV} value `{raw}`: {error}"),
            )
        }),
    }
}

/// Parse the retained per-domain bound enforced whenever a compatibility domain becomes idle.
fn configured_max_entry_bytes(raw: Option<&str>) -> io::Result<u64> {
    match raw {
        None | Some("") => Ok(DEFAULT_GENERATED_CACHE_MAX_ENTRY_BYTES),
        Some(raw) => raw.parse::<u64>().map_err(|error| {
            io::Error::new(
                ErrorKind::InvalidInput,
                format!("invalid {GENERATED_CACHE_MAX_ENTRY_BYTES_ENV} value `{raw}`: {error}"),
            )
        }),
    }
}

/// Resolve one path relative to the current process directory.
fn resolve_path(path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else if let Ok(cwd) = env::current_dir() {
        cwd.join(path)
    } else {
        path.to_path_buf()
    }
}

/// Resolve a project root before choosing its legacy project-local fallback target.
fn absolute_project_root(project_root: &Path) -> PathBuf {
    resolve_path(project_root)
}

/// Query the Rust backend command and selectors inherited by Cargo subprocesses.
pub(crate) fn rust_backend_identity(cargo_working_dir: &Path) -> io::Result<String> {
    let command_working_dir = nearest_existing_directory(cargo_working_dir);
    let rustc = env::var_os("RUSTC")
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "rustc".into());
    let output = Command::new(&rustc)
        .arg("-vV")
        .current_dir(&command_working_dir)
        .output()?;
    if !output.status.success() {
        return Err(io::Error::other(format!(
            "{} -vV failed with status {}",
            rustc.to_string_lossy(),
            output.status
        )));
    }
    let verbose_version = String::from_utf8(output.stdout).map_err(|error| {
        io::Error::new(
            ErrorKind::InvalidData,
            format!("{} -vV returned invalid UTF-8: {error}", rustc.to_string_lossy()),
        )
    })?;
    let cargo = cargo_executable();
    let cargo_output = Command::new(&cargo)
        .arg("-vV")
        .current_dir(&command_working_dir)
        .output()?;
    if !cargo_output.status.success() {
        return Err(io::Error::other(format!(
            "{} -vV failed with status {}",
            cargo.to_string_lossy(),
            cargo_output.status
        )));
    }
    let cargo_verbose_version = String::from_utf8(cargo_output.stdout).map_err(|error| {
        io::Error::new(
            ErrorKind::InvalidData,
            format!("{} -vV returned invalid UTF-8: {error}", cargo.to_string_lossy()),
        )
    })?;
    let mut identity = format_rust_backend_identity(
        &rustc.to_string_lossy(),
        verbose_version.trim(),
        rust_backend_identity_selectors(),
    );
    identity.push_str("cargo_command=");
    identity.push_str(&cargo.to_string_lossy());
    identity.push('\n');
    identity.push_str("cargo_verbose_version=\n");
    identity.push_str(cargo_verbose_version.trim());
    identity.push('\n');
    identity.push_str("cargo_config_identity=");
    identity.push_str(&cargo_config_identity(cargo_working_dir));
    Ok(identity)
}

/// Resolve the directory context for toolchain probes before a generated output directory necessarily exists.
fn nearest_existing_directory(path: &Path) -> PathBuf {
    let absolute = resolve_path(path);
    absolute
        .ancestors()
        .find(|ancestor| ancestor.is_dir())
        .map(Path::to_path_buf)
        .or_else(|| env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Collect every inherited selector that can alter Cargo's emitted artifacts without recording process-local paths.
fn rust_backend_identity_selectors() -> BTreeMap<String, String> {
    let mut selectors = BTreeMap::new();
    for name in RUST_BACKEND_IDENTITY_ENV {
        selectors.insert(
            (*name).to_string(),
            env::var_os(name).unwrap_or_default().to_string_lossy().into_owned(),
        );
    }
    for (name, value) in env::vars_os() {
        let Some(name) = name.to_str() else {
            continue;
        };
        if name.starts_with("CARGO_PROFILE_") || name.starts_with("CARGO_TARGET_") {
            selectors.insert(name.to_string(), value.to_string_lossy().into_owned());
        }
    }
    selectors
}

/// Format backend identity inputs deterministically for hashing and metadata.
fn format_rust_backend_identity(
    rustc_command: &str,
    verbose_version: &str,
    selectors: impl IntoIterator<Item = (String, String)>,
) -> String {
    let mut identity = format!("rustc_command={rustc_command}\nrustc_verbose_version=\n{verbose_version}\n");
    for (name, value) in selectors {
        identity.push_str(&name);
        identity.push('=');
        identity.push_str(&value);
        identity.push('\n');
    }
    identity
}

/// Derive the complete deterministic metadata and digest for one compatibility domain.
fn cache_entry_metadata(
    generated_package_name: &str,
    profile: &str,
    lock_payload: Option<&str>,
    cargo_features: &CargoFeatureSelection,
    cargo_flags: &[String],
    rust_backend_identity: &str,
) -> io::Result<CacheEntryMetadata> {
    let cargo_flags = cargo_flags_identity(cargo_flags);
    let lock_digest = normalized_lock_digest(lock_payload, generated_package_name)?;
    let mut identity_hasher = Sha256::new();
    identity_hasher.update(b"incan-generated-cargo-cache-v1\0");
    identity_hasher.update(crate::version::INCAN_VERSION.as_bytes());
    identity_hasher.update(b"\0rust-backend\0");
    identity_hasher.update(rust_backend_identity.as_bytes());
    identity_hasher.update(b"\0profile\0");
    identity_hasher.update(profile.as_bytes());
    identity_hasher.update(b"\0lock\0");
    identity_hasher.update(lock_digest.as_bytes());
    identity_hasher.update(b"\0features\0");
    for feature in &cargo_features.cargo_features {
        identity_hasher.update(feature.as_bytes());
        identity_hasher.update(b"\0");
    }
    identity_hasher.update([u8::from(cargo_features.cargo_no_default_features)]);
    identity_hasher.update([u8::from(cargo_features.cargo_all_features)]);
    identity_hasher.update(b"\0cargo-flags\0");
    for flag in &cargo_flags {
        identity_hasher.update(flag.as_bytes());
        identity_hasher.update(b"\0");
    }
    Ok(CacheEntryMetadata {
        identity: hex::encode(identity_hasher.finalize()),
        incan_version: crate::version::INCAN_VERSION.to_string(),
        rust_backend_identity: rust_backend_identity.to_string(),
        profile: profile.to_string(),
        lock_digest,
        cargo_features: cargo_features.clone().normalized(),
        cargo_flags,
        last_used_unix_seconds: now_unix_seconds(),
        logical_bytes: 0,
        last_measured_unix_seconds: 0,
    })
}

/// Retain Cargo arguments that can change compiled artifacts while discarding execution and presentation policy.
fn cargo_flags_identity(cargo_flags: &[String]) -> Vec<String> {
    let mut identity = Vec::new();
    let mut flags = cargo_flags.iter();
    while let Some(flag) = flags.next() {
        if matches!(
            flag.as_str(),
            "--offline" | "--locked" | "--frozen" | "--verbose" | "-v" | "--quiet" | "-q" | "--timings"
        ) || flag.starts_with("--timings=")
        {
            continue;
        }
        if matches!(flag.as_str(), "--color" | "--message-format") {
            let _ = flags.next();
            continue;
        }
        if flag.starts_with("--color=") || flag.starts_with("--message-format=") {
            continue;
        }
        identity.push(flag.clone());
    }
    identity
}

/// Reject Cargo passthrough that could escape the target directory protected and reported by the managed cache.
fn validate_managed_cargo_flags(cargo_flags: &[String]) -> io::Result<()> {
    let mut flags = cargo_flags.iter();
    while let Some(flag) = flags.next() {
        if flag == "--target-dir" || flag.starts_with("--target-dir=") {
            return Err(unsupported_cargo_target_dir_error());
        }
        let config = if flag == "--config" {
            let Some(value) = flags.next() else {
                continue;
            };
            value.as_str()
        } else if let Some(value) = flag.strip_prefix("--config=") {
            value
        } else {
            continue;
        };
        if !cargo_config_is_inline_and_target_safe(config) {
            return Err(unsupported_cargo_target_dir_error());
        }
    }
    Ok(())
}

/// Accept only inline Cargo config that cannot redirect managed artifacts through either supported Cargo build path.
fn cargo_config_is_inline_and_target_safe(config: &str) -> bool {
    let Ok(parsed) = toml::from_str::<toml::Value>(config) else {
        return false;
    };
    let Some(build) = parsed.get("build").and_then(toml::Value::as_table) else {
        return config.contains('=');
    };
    !build.contains_key("target-dir") && !build.contains_key("build-dir")
}

/// Explain the supported target-directory override without exposing Cargo outside Incan's cache lease.
fn unsupported_cargo_target_dir_error() -> io::Error {
    io::Error::new(
        ErrorKind::InvalidInput,
        "Cargo --target-dir and target/build directory --config passthrough are incompatible with Incan-managed storage; use \
         --generated-cargo-target-dir or INCAN_GENERATED_CARGO_TARGET_DIR instead",
    )
}

/// Hash a Cargo lock after replacing the generated root package's project-specific coordinates.
fn normalized_lock_digest(lock_payload: Option<&str>, generated_package_name: &str) -> io::Result<String> {
    let Some(lock_payload) = lock_payload else {
        return Ok(hex::encode(Sha256::digest([])));
    };
    let mut lock = toml::from_str::<toml::Value>(lock_payload).map_err(|error| {
        io::Error::new(
            ErrorKind::InvalidData,
            format!("failed to parse generated Cargo lock for cache identity: {error}"),
        )
    })?;
    if let Some(packages) = lock.get_mut("package").and_then(toml::Value::as_array_mut) {
        for package in packages {
            let is_generated_root = package
                .get("name")
                .and_then(toml::Value::as_str)
                .is_some_and(|name| name == generated_package_name);
            if is_generated_root && let Some(table) = package.as_table_mut() {
                table.insert(
                    "name".to_string(),
                    toml::Value::String("__incan_generated_root".to_string()),
                );
                table.insert("version".to_string(), toml::Value::String("0.0.0".to_string()));
            }
        }
    }
    let normalized = toml::to_string(&lock).map_err(|error| {
        io::Error::new(
            ErrorKind::InvalidData,
            format!("failed to serialize normalized Cargo lock for cache identity: {error}"),
        )
    })?;
    Ok(hex::encode(Sha256::digest(normalized.as_bytes())))
}

/// Acquire the root manager guard, prune old domains, publish metadata, and retain a shared activity lease.
fn acquire_managed_target(
    cache_root: &Path,
    max_bytes: u64,
    max_entry_bytes: u64,
    mut metadata: CacheEntryMetadata,
) -> io::Result<GeneratedCargoTarget> {
    fs::create_dir_all(cache_root)?;
    let _manager = acquire_manager_lock(cache_root)?;
    let entry_root = cache_root.join(&metadata.identity);
    let identity = metadata.identity.clone();
    fs::create_dir_all(&entry_root)?;
    let active_file = open_active_lock(&entry_root)?;
    match active_file.try_lock() {
        Ok(()) => {
            if let Ok(payload) = fs::read(entry_root.join(CACHE_METADATA_FILE))
                && let Ok(existing) = serde_json::from_slice::<CacheEntryMetadata>(&payload)
            {
                metadata.logical_bytes = existing.logical_bytes;
                metadata.last_measured_unix_seconds = existing.last_measured_unix_seconds;
            }
            if metadata.last_measured_unix_seconds == 0 || metadata.logical_bytes > max_entry_bytes {
                metadata.logical_bytes = measure_and_enforce_entry_bound(&entry_root, max_entry_bytes)?;
                metadata.last_measured_unix_seconds = now_unix_seconds();
            }
            active_file.unlock()?;
        }
        Err(TryLockError::WouldBlock) => {
            if let Ok(payload) = fs::read(entry_root.join(CACHE_METADATA_FILE))
                && let Ok(existing) = serde_json::from_slice::<CacheEntryMetadata>(&payload)
            {
                metadata.logical_bytes = existing.logical_bytes;
            }
        }
        Err(TryLockError::Error(error)) => return Err(error),
    }
    active_file.lock_shared()?;
    metadata.last_measured_unix_seconds = 0;
    // Protect the requested domain before pruning. This preserves warm artifacts even when the domain itself is older
    // than the remaining entries or larger than the configured soft limit.
    prune_entries_while_locked(cache_root, max_bytes, false, &[], false)?;
    write_metadata(&entry_root, &metadata)?;
    Ok(GeneratedCargoTarget {
        path: entry_root.join("target"),
        lease: Some(GeneratedCacheLease {
            file: Some(active_file),
            entry_root,
            max_entry_bytes,
            finalized: false,
        }),
        identity: Some(identity),
    })
}

/// Measure an idle domain and discard only its rebuildable Cargo target when it exceeds the retained safety bound.
fn measure_and_enforce_entry_bound(entry_root: &Path, max_entry_bytes: u64) -> io::Result<u64> {
    let logical_bytes = directory_size(entry_root)?;
    if logical_bytes <= max_entry_bytes {
        return Ok(logical_bytes);
    }
    let target = entry_root.join("target");
    match fs::remove_dir_all(&target) {
        Ok(()) => {}
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    directory_size(entry_root)
}

/// Open and exclusively lock the cache-wide manager descriptor.
fn acquire_manager_lock(cache_root: &Path) -> io::Result<File> {
    fs::create_dir_all(cache_root)?;
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(cache_root.join(CACHE_MANAGER_LOCK_FILE))?;
    file.lock()?;
    Ok(file)
}

/// Open the stable per-domain activity descriptor.
fn open_active_lock(entry_root: &Path) -> io::Result<File> {
    OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(entry_root.join(CACHE_ACTIVE_LOCK_FILE))
}

/// Publish entry metadata atomically inside its compatibility domain.
fn write_metadata(entry_root: &Path, metadata: &CacheEntryMetadata) -> io::Result<()> {
    let payload = serde_json::to_vec_pretty(metadata).map_err(|error| {
        io::Error::new(
            ErrorKind::InvalidData,
            format!("failed to serialize cache metadata: {error}"),
        )
    })?;
    let staged_path = entry_root.join(format!(".{CACHE_METADATA_FILE}.tmp-{}", std::process::id()));
    let final_path = entry_root.join(CACHE_METADATA_FILE);
    let mut staged = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&staged_path)?;
    staged.write_all(&payload)?;
    staged.sync_all()?;
    fs::rename(staged_path, final_path)
}

/// Return a read-only cache snapshot with recursive logical file-byte counts.
fn inspect_cache_root(cache_root: &Path, max_bytes: u64) -> io::Result<GeneratedCacheInspection> {
    if !cache_root.exists() {
        return Ok(GeneratedCacheInspection {
            schema_version: CACHE_REPORT_SCHEMA_VERSION,
            root: cache_root.to_path_buf(),
            max_bytes,
            total_bytes: 0,
            entries: Vec::new(),
        });
    }
    let entries = collect_entries(cache_root, true)?;
    let total_bytes = entries.iter().map(|entry| entry.bytes).sum();
    Ok(GeneratedCacheInspection {
        schema_version: CACHE_REPORT_SCHEMA_VERSION,
        root: cache_root.to_path_buf(),
        max_bytes,
        total_bytes,
        entries,
    })
}

/// Serialize one prune transaction against entry acquisition and other cleanup commands.
fn prune_cache_root(
    cache_root: &Path,
    max_bytes: u64,
    dry_run: bool,
    identities: &[String],
) -> io::Result<GeneratedCachePruneReport> {
    if !cache_root.exists() {
        return Ok(GeneratedCachePruneReport {
            schema_version: CACHE_REPORT_SCHEMA_VERSION,
            root: cache_root.to_path_buf(),
            max_bytes,
            before_bytes: 0,
            after_bytes: 0,
            removed_logical_bytes: 0,
            removed_entries: Vec::new(),
            skipped_active_entries: Vec::new(),
            requested_identities: identities.to_vec(),
            not_found_identities: identities.to_vec(),
            dry_run,
        });
    }
    let _manager = acquire_manager_lock(cache_root)?;
    prune_entries_while_locked(cache_root, max_bytes, dry_run, identities, true)
}

/// Remove least-recently-used inactive domains until the requested limit is satisfied.
fn prune_entries_while_locked(
    cache_root: &Path,
    max_bytes: u64,
    dry_run: bool,
    identities: &[String],
    refresh_sizes: bool,
) -> io::Result<GeneratedCachePruneReport> {
    let mut entries = collect_entries(cache_root, refresh_sizes)?;
    entries.sort_by_key(|entry| entry.last_used_unix_seconds);
    let before_bytes = entries.iter().map(|entry| entry.bytes).sum::<u64>();
    let mut projected_bytes = before_bytes;
    let mut removed_entries = Vec::new();
    let mut removed_logical_bytes = 0_u64;
    let mut skipped_active_entries = Vec::new();
    let selective = !identities.is_empty();
    let mut found_identities = Vec::new();
    for entry in entries {
        if selective {
            if !identities.iter().any(|identity| identity == &entry.identity) {
                continue;
            }
            found_identities.push(entry.identity.clone());
        } else if projected_bytes <= max_bytes {
            break;
        }
        let active_file = if dry_run {
            match OpenOptions::new()
                .read(true)
                .write(true)
                .open(entry.path.join(CACHE_ACTIVE_LOCK_FILE))
            {
                Ok(file) => Some(file),
                Err(error) if error.kind() == ErrorKind::NotFound => None,
                Err(error) => return Err(error),
            }
        } else {
            Some(open_active_lock(&entry.path)?)
        };
        match active_file {
            None => {
                projected_bytes = projected_bytes.saturating_sub(entry.bytes);
                removed_logical_bytes = removed_logical_bytes.saturating_add(entry.bytes);
                removed_entries.push(entry.identity.clone());
            }
            Some(active_file) => match active_file.try_lock() {
                Ok(()) => {
                    projected_bytes = projected_bytes.saturating_sub(entry.bytes);
                    removed_logical_bytes = removed_logical_bytes.saturating_add(entry.bytes);
                    removed_entries.push(entry.identity.clone());
                    if !dry_run {
                        fs::remove_dir_all(&entry.path)?;
                    }
                }
                Err(TryLockError::WouldBlock) => skipped_active_entries.push(entry.identity),
                Err(TryLockError::Error(error)) => return Err(error),
            },
        }
    }
    let after_bytes = if dry_run {
        projected_bytes
    } else {
        collect_entries(cache_root, refresh_sizes)?
            .iter()
            .map(|entry| entry.bytes)
            .sum()
    };
    let not_found_identities = identities
        .iter()
        .filter(|identity| !found_identities.contains(identity))
        .cloned()
        .collect();
    Ok(GeneratedCachePruneReport {
        schema_version: CACHE_REPORT_SCHEMA_VERSION,
        root: cache_root.to_path_buf(),
        max_bytes,
        before_bytes,
        after_bytes,
        removed_logical_bytes,
        removed_entries,
        skipped_active_entries,
        requested_identities: identities.to_vec(),
        not_found_identities,
        dry_run,
    })
}

/// Enumerate managed compatibility domains and their recursive file sizes.
fn collect_entries(cache_root: &Path, refresh_sizes: bool) -> io::Result<Vec<GeneratedCacheEntry>> {
    let mut entries = Vec::new();
    for entry in fs::read_dir(cache_root)? {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) if error.kind() == ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(error) if error.kind() == ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        if !file_type.is_dir() {
            continue;
        }
        let entry_root = entry.path();
        let metadata_path = entry_root.join(CACHE_METADATA_FILE);
        let metadata = fs::read(&metadata_path)
            .ok()
            .and_then(|payload| serde_json::from_slice::<CacheEntryMetadata>(&payload).ok());
        let active = match OpenOptions::new()
            .read(true)
            .write(true)
            .open(entry_root.join(CACHE_ACTIVE_LOCK_FILE))
        {
            Ok(active_file) => match active_file.try_lock() {
                Ok(()) => false,
                Err(TryLockError::WouldBlock) => true,
                Err(TryLockError::Error(error)) => return Err(error),
            },
            Err(error) if error.kind() == ErrorKind::NotFound => false,
            Err(error) => return Err(error),
        };
        // A zero measurement timestamp means the producing process never completed its lease cleanup. Recover that
        // interrupted domain during the next ordinary acquisition so a valid-but-dirty metadata file cannot hide a
        // multi-gigabyte partial build from automatic pruning indefinitely.
        let measurement_unknown = metadata
            .as_ref()
            .is_some_and(|metadata| metadata.last_measured_unix_seconds == 0);
        let bytes = if refresh_sizes || (!active && (measurement_unknown || metadata.is_none())) {
            directory_size(&entry_root)?
        } else {
            metadata.as_ref().map_or(0, |metadata| metadata.logical_bytes)
        };
        entries.push(GeneratedCacheEntry {
            category: CACHE_CATEGORY.to_string(),
            identity: metadata
                .as_ref()
                .map(|metadata| metadata.identity.clone())
                .unwrap_or_else(|| entry.file_name().to_string_lossy().to_string()),
            path: entry_root.clone(),
            bytes,
            last_used_unix_seconds: metadata
                .as_ref()
                .map(|metadata| metadata.last_used_unix_seconds)
                .unwrap_or_else(|| entry_modified_unix_seconds(&entry)),
            profile: metadata
                .map(|metadata| metadata.profile)
                .unwrap_or_else(|| "unknown".to_string()),
            active,
        });
    }
    entries.sort_by(|left, right| left.identity.cmp(&right.identity));
    Ok(entries)
}

/// Return one cache directory's modification timestamp for orphaned or unreadable domains.
fn entry_modified_unix_seconds(entry: &fs::DirEntry) -> u64 {
    entry
        .metadata()
        .and_then(|metadata| metadata.modified())
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).map_err(io::Error::other))
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

/// Sum regular-file sizes below a managed cache domain without following symlinks.
fn directory_size(path: &Path) -> io::Result<u64> {
    let mut total = 0_u64;
    let entries = match fs::read_dir(path) {
        Ok(entries) => entries,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(error),
    };
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) if error.kind() == ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(error) if error.kind() == ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        if file_type.is_dir() {
            total = total.saturating_add(directory_size(&entry.path())?);
        } else if file_type.is_file() {
            match entry.metadata() {
                Ok(metadata) => total = total.saturating_add(metadata.len()),
                Err(error) if error.kind() == ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
        }
    }
    Ok(total)
}

/// Return the current Unix timestamp, using zero only on an invalid pre-epoch system clock.
fn now_unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compatibility_identity_changes_for_every_declared_domain_input() -> io::Result<()> {
        let features = CargoFeatureSelection::default();
        let baseline = cache_entry_metadata("root", "release", None, &features, &[], "rustc-a")?;
        let profile = cache_entry_metadata("root", "debug", None, &features, &[], "rustc-a")?;
        let lock = cache_entry_metadata("root", "release", Some("version = 4"), &features, &[], "rustc-a")?;
        let rustc = cache_entry_metadata("root", "release", None, &features, &[], "rustc-b")?;
        let selected_features = CargoFeatureSelection {
            cargo_features: vec!["serde".to_string()],
            ..CargoFeatureSelection::default()
        };
        let feature = cache_entry_metadata("root", "release", None, &selected_features, &[], "rustc-a")?;
        let cargo_flag = cache_entry_metadata(
            "root",
            "release",
            None,
            &features,
            &["--target".to_string(), "wasm32-wasip1".to_string()],
            "rustc-a",
        )?;
        let execution_policy = cache_entry_metadata(
            "root",
            "release",
            None,
            &features,
            &["--offline".to_string(), "--locked".to_string(), "--timings".to_string()],
            "rustc-a",
        )?;

        assert_ne!(baseline.identity, profile.identity);
        assert_ne!(baseline.identity, lock.identity);
        assert_ne!(baseline.identity, rustc.identity);
        assert_ne!(baseline.identity, feature.identity);
        assert_ne!(baseline.identity, cargo_flag.identity);
        assert_eq!(baseline.identity, execution_policy.identity);
        Ok(())
    }

    #[test]
    fn compatibility_identity_ignores_generated_root_coordinates() -> io::Result<()> {
        let first_lock = "version = 4\n\n[[package]]\nname = \"first\"\nversion = \"1.2.3\"\ndependencies = [\"serde\"]\n\n[[package]]\nname = \"serde\"\nversion = \"1.0.0\"\n";
        let second_lock = "version = 4\n\n[[package]]\nname = \"second\"\nversion = \"9.8.7\"\ndependencies = [\"serde\"]\n\n[[package]]\nname = \"serde\"\nversion = \"1.0.0\"\n";
        let features = CargoFeatureSelection::default();
        let first = cache_entry_metadata("first", "release", Some(first_lock), &features, &[], "rustc")?;
        let second = cache_entry_metadata("second", "release", Some(second_lock), &features, &[], "rustc")?;
        assert_eq!(first.identity, second.identity);
        Ok(())
    }

    #[test]
    fn default_root_follows_incan_home_before_user_home() {
        assert_eq!(
            default_generated_cache_root(Some("/incan".into()), Some("/user".into())),
            Some(PathBuf::from("/incan/cache/generated-cargo/v1"))
        );
        assert_eq!(
            default_generated_cache_root(None, Some("/user".into())),
            Some(PathBuf::from("/user/.incan/cache/generated-cargo/v1"))
        );
    }

    #[test]
    fn pruning_removes_oldest_inactive_domain_and_preserves_active_domain() -> io::Result<()> {
        let temp = tempfile::tempdir()?;
        let cache_root = temp.path().join("cache");
        fs::create_dir_all(&cache_root)?;
        let features = CargoFeatureSelection::default();
        let mut old = cache_entry_metadata("root", "release", None, &features, &[], "rustc")?;
        old.identity = "old".to_string();
        old.last_used_unix_seconds = 1;
        let old_root = cache_root.join(&old.identity);
        fs::create_dir_all(old_root.join("target"))?;
        write_metadata(&old_root, &old)?;
        fs::write(old_root.join("target/artifact"), [0_u8; 16])?;

        let mut active = cache_entry_metadata("root", "release", None, &features, &[], "rustc")?;
        active.identity = "active".to_string();
        active.last_used_unix_seconds = 2;
        let active_root = cache_root.join(&active.identity);
        fs::create_dir_all(active_root.join("target"))?;
        write_metadata(&active_root, &active)?;
        fs::write(active_root.join("target/artifact"), [0_u8; 16])?;
        let active_file = open_active_lock(&active_root)?;
        active_file.lock_shared()?;

        let report = prune_cache_root(&cache_root, 0, false, &[])?;
        assert!(!old_root.exists());
        assert!(active_root.exists());
        assert_eq!(report.removed_entries, vec![old.identity]);
        assert!(report.removed_logical_bytes >= 16);
        assert_eq!(report.skipped_active_entries, vec![active.identity]);
        Ok(())
    }

    #[test]
    fn inspection_and_pruning_include_orphaned_domains() -> io::Result<()> {
        let temp = tempfile::tempdir()?;
        let cache_root = temp.path().join("cache");
        let orphan_root = cache_root.join("partial");
        fs::create_dir_all(&orphan_root)?;
        fs::write(orphan_root.join("artifact"), [0_u8; 8])?;

        let inspection = inspect_cache_root(&cache_root, 0)?;
        assert_eq!(inspection.entries.len(), 1);
        assert_eq!(inspection.entries[0].identity, "partial");
        let report = prune_cache_root(&cache_root, 0, false, &[])?;
        assert_eq!(report.removed_entries, ["partial"]);
        assert!(!orphan_root.exists());
        Ok(())
    }

    #[test]
    fn rust_backend_identity_includes_compiler_and_cargo_selectors() {
        let identity = format_rust_backend_identity(
            "/toolchains/nightly/bin/rustc",
            "rustc 1.99.0\nhost: aarch64-apple-darwin",
            [
                ("RUSTUP_TOOLCHAIN".to_string(), "nightly".to_string()),
                ("CARGO_BUILD_TARGET".to_string(), "x86_64-unknown-linux-gnu".to_string()),
            ],
        );
        assert!(identity.contains("rustc_command=/toolchains/nightly/bin/rustc"));
        assert!(identity.contains("host: aarch64-apple-darwin"));
        assert!(identity.contains("RUSTUP_TOOLCHAIN=nightly"));
        assert!(identity.contains("CARGO_BUILD_TARGET=x86_64-unknown-linux-gnu"));
    }

    #[test]
    fn rejects_cargo_passthrough_that_can_escape_managed_target() {
        for flags in [
            vec!["--target-dir".to_string(), "/tmp/other".to_string()],
            vec!["--target-dir=/tmp/other".to_string()],
            vec!["--config".to_string(), "build.target-dir='/tmp/other'".to_string()],
            vec!["--config".to_string(), "build.'target-dir'='/tmp/other'".to_string()],
            vec![
                "--config".to_string(),
                "build = { build-dir = '/tmp/other' }".to_string(),
            ],
            vec!["--config=other.toml".to_string()],
        ] {
            assert!(validate_managed_cargo_flags(&flags).is_err());
        }
        assert!(validate_managed_cargo_flags(&["--config".to_string(), "net.retry=3".to_string()]).is_ok());
    }

    #[test]
    fn explicit_target_override_bypasses_managed_cargo_flag_validation() -> io::Result<()> {
        let temp = tempfile::tempdir()?;
        let target = temp.path().join("caller-owned-target");
        let resolved = resolve_generated_cargo_target(
            Some(&target),
            temp.path(),
            temp.path(),
            "root",
            "release",
            None,
            &CargoFeatureSelection::default(),
            &["--target-dir=/tmp/caller-owned".to_string()],
        )?;

        assert_eq!(resolved.path, target);
        assert!(resolved.lease.is_none());
        assert!(resolved.identity.is_none());
        Ok(())
    }

    #[test]
    fn acquisition_protects_requested_domain_while_pruning() -> io::Result<()> {
        let temp = tempfile::tempdir()?;
        let cache_root = temp.path().join("cache");
        fs::create_dir_all(&cache_root)?;
        let features = CargoFeatureSelection::default();
        let mut requested = cache_entry_metadata("root", "release", None, &features, &[], "rustc")?;
        requested.identity = "requested".to_string();
        requested.last_used_unix_seconds = 1;
        let requested_root = cache_root.join(&requested.identity);
        fs::create_dir_all(requested_root.join("target"))?;
        write_metadata(&requested_root, &requested)?;
        fs::write(requested_root.join("target/artifact"), [0_u8; 16])?;

        let mut other = requested.clone();
        other.identity = "other".to_string();
        other.last_used_unix_seconds = 2;
        let other_root = cache_root.join(&other.identity);
        fs::create_dir_all(other_root.join("target"))?;
        write_metadata(&other_root, &other)?;
        fs::write(other_root.join("target/artifact"), [0_u8; 16])?;

        let acquired = acquire_managed_target(&cache_root, 0, u64::MAX, requested)?;
        assert!(requested_root.join("target/artifact").exists());
        assert!(!other_root.exists());
        drop(acquired);
        Ok(())
    }

    #[test]
    fn acquisition_marks_domain_dirty_and_recovers_it_after_interruption() -> io::Result<()> {
        let temp = tempfile::tempdir()?;
        let cache_root = temp.path().join("cache");
        fs::create_dir_all(&cache_root)?;
        let features = CargoFeatureSelection::default();

        let mut interrupted = cache_entry_metadata("root", "release", None, &features, &[], "rustc")?;
        interrupted.identity = "interrupted".to_string();
        interrupted.last_used_unix_seconds = 1;
        interrupted.logical_bytes = 8;
        interrupted.last_measured_unix_seconds = 1;
        let interrupted_root = cache_root.join(&interrupted.identity);
        fs::create_dir_all(interrupted_root.join("target"))?;
        write_metadata(&interrupted_root, &interrupted)?;

        let acquired = acquire_managed_target(&cache_root, u64::MAX, u64::MAX, interrupted.clone())?;
        let GeneratedCargoTarget {
            lease: Some(mut interrupted_lease),
            ..
        } = acquired
        else {
            return Err(io::Error::other("managed acquisition omitted its activity lease"));
        };
        let dirty_payload = fs::read(interrupted_root.join(CACHE_METADATA_FILE))?;
        let dirty = serde_json::from_slice::<CacheEntryMetadata>(&dirty_payload).map_err(io::Error::other)?;
        assert_eq!(dirty.logical_bytes, 8);
        assert_eq!(dirty.last_measured_unix_seconds, 0);

        // Simulate process termination: the operating system releases the activity descriptor but `Drop` never gets
        // an opportunity to measure the newly written partial output.
        let Some(active_file) = interrupted_lease.file.take() else {
            return Err(io::Error::other("managed lease omitted its activity descriptor"));
        };
        active_file.unlock()?;
        drop(active_file);
        std::mem::forget(interrupted_lease);
        fs::write(interrupted_root.join("target/partial-artifact"), [0_u8; 64])?;

        let mut requested = interrupted.clone();
        requested.identity = "requested".to_string();
        requested.last_used_unix_seconds = 2;
        let acquired = acquire_managed_target(&cache_root, 0, u64::MAX, requested)?;

        assert!(!interrupted_root.exists());
        assert!(cache_root.join("requested").exists());
        drop(acquired);
        Ok(())
    }

    #[test]
    fn reacquiring_interrupted_domain_discards_oversized_rebuildable_target() -> io::Result<()> {
        let temp = tempfile::tempdir()?;
        let cache_root = temp.path().join("cache");
        let features = CargoFeatureSelection::default();
        let mut metadata = cache_entry_metadata("root", "release", None, &features, &[], "rustc")?;
        metadata.identity = "interrupted".to_string();
        let entry_root = cache_root.join(&metadata.identity);

        let acquired = acquire_managed_target(&cache_root, u64::MAX, 32, metadata.clone())?;
        let GeneratedCargoTarget {
            lease: Some(mut interrupted_lease),
            ..
        } = acquired
        else {
            return Err(io::Error::other("managed acquisition omitted its activity lease"));
        };
        fs::create_dir_all(entry_root.join("target"))?;
        fs::write(entry_root.join("target/partial-artifact"), [0_u8; 64])?;

        let Some(active_file) = interrupted_lease.file.take() else {
            return Err(io::Error::other("managed lease omitted its activity descriptor"));
        };
        active_file.unlock()?;
        drop(active_file);
        std::mem::forget(interrupted_lease);

        let reacquired = acquire_managed_target(&cache_root, u64::MAX, 32, metadata)?;
        assert!(!entry_root.join("target").exists());
        drop(reacquired);
        Ok(())
    }

    #[test]
    fn selective_prune_removes_only_requested_identity() -> io::Result<()> {
        let temp = tempfile::tempdir()?;
        let cache_root = temp.path().join("cache");
        fs::create_dir_all(cache_root.join("selected"))?;
        fs::create_dir_all(cache_root.join("preserved"))?;
        fs::write(cache_root.join("selected/artifact"), [0_u8; 8])?;
        fs::write(cache_root.join("preserved/artifact"), [0_u8; 8])?;

        let identities = vec!["selected".to_string(), "missing".to_string()];
        let report = prune_cache_root(&cache_root, u64::MAX, false, &identities)?;
        assert_eq!(report.removed_entries, ["selected"]);
        assert_eq!(report.not_found_identities, ["missing"]);
        assert!(!cache_root.join("selected").exists());
        assert!(cache_root.join("preserved").exists());
        Ok(())
    }

    #[test]
    fn inspection_does_not_create_a_manager_lock() -> io::Result<()> {
        let temp = tempfile::tempdir()?;
        let cache_root = temp.path().join("cache");
        fs::create_dir_all(cache_root.join("domain"))?;
        fs::write(cache_root.join("domain/artifact"), [0_u8; 8])?;

        let inspection = inspect_cache_root(&cache_root, u64::MAX)?;

        assert_eq!(inspection.entries.len(), 1);
        assert!(!cache_root.join(CACHE_MANAGER_LOCK_FILE).exists());
        Ok(())
    }

    #[test]
    fn acquisition_discards_known_oversized_rebuildable_target() -> io::Result<()> {
        let temp = tempfile::tempdir()?;
        let cache_root = temp.path().join("cache");
        let features = CargoFeatureSelection::default();
        let mut metadata = cache_entry_metadata("root", "release", None, &features, &[], "rustc")?;
        metadata.identity = "oversized".to_string();
        metadata.logical_bytes = 9;
        let entry_root = cache_root.join(&metadata.identity);
        fs::create_dir_all(entry_root.join("target"))?;
        write_metadata(&entry_root, &metadata)?;
        fs::write(entry_root.join("target/artifact"), [0_u8; 9])?;

        let acquired = acquire_managed_target(&cache_root, u64::MAX, 8, metadata)?;
        assert!(entry_root.exists());
        assert!(!entry_root.join("target").exists());
        drop(acquired);
        Ok(())
    }

    #[test]
    fn completed_lease_releases_domain_and_discards_oversized_rebuildable_target() -> io::Result<()> {
        let temp = tempfile::tempdir()?;
        let cache_root = temp.path().join("cache");
        let features = CargoFeatureSelection::default();
        let mut metadata = cache_entry_metadata("root", "release", None, &features, &[], "rustc")?;
        metadata.identity = "completed".to_string();
        let entry_root = cache_root.join(&metadata.identity);

        let Ok(GeneratedCargoTarget { lease: Some(lease), .. }) =
            acquire_managed_target(&cache_root, u64::MAX, 8, metadata)
        else {
            return Err(io::Error::other("managed acquisition omitted its activity lease"));
        };
        fs::create_dir_all(entry_root.join("target"))?;
        fs::write(entry_root.join("target/artifact"), [0_u8; 9])?;

        lease.finish()?;
        assert!(entry_root.exists());
        assert!(!entry_root.join("target").exists());
        Ok(())
    }
}
