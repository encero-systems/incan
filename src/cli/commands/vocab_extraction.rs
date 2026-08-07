//! Producer-side vocab companion crate extraction for `incan build --lib`.

use std::collections::{BTreeMap, HashSet};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::cli::{CliError, CliResult};
use crate::library_manifest::{SoftKeywordActivation, VocabDesugarerArtifact, VocabExports};
use crate::manifest::ProjectManifest;
use crate::oven::rustc::{
    OvenRustcArtifactManifest, OvenRustcArtifactPlan, OvenRustcAuxiliaryTargetPlan, clear_inherited_cargo_environment,
};
use crate::version::INCAN_VERSION;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use wasmtime::{Config, Engine, ExternType, Module, ValType};

const VOCAB_COMPANION_CACHE_FORMAT: u32 = 1;
const VOCAB_COMPANION_CACHE_DIR_ENV: &str = "INCAN_VOCAB_COMPANION_CACHE_DIR";
const VOCAB_COMPANION_CACHE_FILE: &str = "metadata.json";
const OVEN_COMPILER_SUITE_RUSTC_ENV: &str = "INCAN_OVEN_COMPILER_SUITE_RUSTC";
const OVEN_COMPILER_SUITE_VOCAB_RUSTC_ENV: &str = "INCAN_OVEN_COMPILER_SUITE_VOCAB_RUSTC";
const OVEN_COMPILER_SUITE_VOCAB_DEPENDENCY_PATH_COUNT_ENV: &str =
    "INCAN_OVEN_COMPILER_SUITE_VOCAB_DEPENDENCY_PATH_COUNT";
const OVEN_COMPILER_SUITE_VOCAB_EXTERN_COUNT_ENV: &str = "INCAN_OVEN_COMPILER_SUITE_VOCAB_EXTERN_COUNT";

/// A receipt-bound direct-Rustc closure that can compile the bounded vocabulary helper.
///
/// The compiler-suite scheduler exports this through its child environment, while a normal Oven library build derives
/// it directly from its selected plan and retains that plan's lease through extraction. A partial or malformed
/// capability is an error: it must never reopen a Cargo fallback merely because the child was launched outside the
/// compiler process.
#[derive(Debug)]
pub(crate) struct OvenVocabDirectRustcContext {
    rustc: PathBuf,
    dependency_search_paths: Vec<PathBuf>,
    externs: BTreeMap<String, PathBuf>,
    auxiliary_targets: BTreeMap<String, OvenVocabAuxiliaryTargetContext>,
}

/// One target-specific compiler-owned vocabulary support closure.
#[derive(Debug)]
struct OvenVocabAuxiliaryTargetContext {
    dependency_search_paths: Vec<PathBuf>,
    externs: BTreeMap<String, PathBuf>,
}

pub(crate) struct LibraryVocabExtraction {
    pub(crate) payload: VocabExports,
    pub(crate) compatibility_activations: Vec<SoftKeywordActivation>,
    pub(crate) pending_desugarer_artifact: Option<PendingDesugarerArtifact>,
}

#[derive(Debug, Clone)]
pub(crate) struct PendingDesugarerArtifact {
    pub(crate) metadata: VocabDesugarerArtifact,
    pub(crate) source_path: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VocabExtractionMode {
    PackageArtifacts,
    ParserOnly,
}

#[derive(Debug, Clone)]
struct VocabCompanionCacheContext {
    fingerprint: String,
    cache_dir: PathBuf,
}

#[derive(Debug, Clone)]
struct CachedVocabCompanion {
    metadata: incan_vocab::VocabMetadata,
    pending_desugarer_artifact: Option<PendingDesugarerArtifact>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct VocabCompanionCacheEnvelope {
    cache_format: u32,
    compiler_version: String,
    vocab_metadata_version: u32,
    fingerprint: String,
    metadata: incan_vocab::VocabMetadata,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    desugarer_artifact: Option<CachedDesugarerArtifact>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CachedDesugarerArtifact {
    metadata: VocabDesugarerArtifact,
    file_name: String,
}

/// Collect full vocab companion metadata for packaging a library artifact.
pub(crate) fn collect_library_vocab_metadata(
    manifest: &ProjectManifest,
    project_root: &Path,
    generated_cargo_target_dir: Option<&Path>,
    direct_rustc: Option<&OvenVocabDirectRustcContext>,
) -> CliResult<Option<LibraryVocabExtraction>> {
    collect_library_vocab_metadata_with_mode(
        manifest,
        project_root,
        generated_cargo_target_dir,
        VocabExtractionMode::PackageArtifacts,
        direct_rustc,
    )
}

/// Collect parser-only vocab metadata for source collection without preparing persistent library artifacts.
pub(crate) fn collect_library_vocab_metadata_for_parser(
    manifest: &ProjectManifest,
    project_root: &Path,
    generated_cargo_target_dir: Option<&Path>,
) -> CliResult<Option<LibraryVocabExtraction>> {
    collect_library_vocab_metadata_with_mode(
        manifest,
        project_root,
        generated_cargo_target_dir,
        VocabExtractionMode::ParserOnly,
        None,
    )
}

/// Collect vocab companion metadata using either full package artifacts or parser-only source metadata.
fn collect_library_vocab_metadata_with_mode(
    manifest: &ProjectManifest,
    project_root: &Path,
    generated_cargo_target_dir: Option<&Path>,
    mode: VocabExtractionMode,
    direct_rustc: Option<&OvenVocabDirectRustcContext>,
) -> CliResult<Option<LibraryVocabExtraction>> {
    let Some(vocab) = manifest.vocab() else {
        return Ok(None);
    };

    let declared_crate_path = vocab
        .crate_path
        .clone()
        .ok_or_else(|| CliError::failure("`[vocab]` section requires a `crate` field in incan.toml".to_string()))?;
    let declared_crate_path = declared_crate_path.trim().to_string();
    if declared_crate_path.is_empty() {
        return Err(CliError::failure("`[vocab].crate` cannot be empty".to_string()));
    }

    let companion_crate_root = resolve_companion_crate_root(project_root, &declared_crate_path);
    validate_companion_crate_root(&companion_crate_root)?;
    let cargo_manifest_path = companion_crate_root.join("Cargo.toml");
    let package_name = read_companion_package_name(&cargo_manifest_path)?;
    let cache_context = vocab_companion_cache_context(
        project_root,
        &companion_crate_root,
        &package_name,
        generated_cargo_target_dir,
    )?;
    let cached = read_cached_vocab_companion(&cache_context)?;
    let cache_hit = cached.is_some();
    let cached_had_desugarer_artifact = cached
        .as_ref()
        .and_then(|cached| cached.pending_desugarer_artifact.as_ref())
        .is_some();

    let metadata = if let Some(cached) = cached.as_ref() {
        cached.metadata.clone()
    } else {
        let extraction_target_dir = cache_context.cache_dir.join("target");
        extract_vocab_metadata_from_library_entrypoint(
            &companion_crate_root,
            &package_name,
            &extraction_target_dir,
            direct_rustc,
        )?
    };
    ensure_supported_vocab_metadata_version(&metadata, &companion_crate_root)?;
    let mut pending_desugarer_artifact = cached
        .as_ref()
        .and_then(|cached| cached.pending_desugarer_artifact.clone());
    if mode == VocabExtractionMode::PackageArtifacts
        && let Some(desugarer) = metadata.desugarer.as_ref()
        && pending_desugarer_artifact.is_none()
    {
        if let Some(context) = direct_rustc {
            let desugarer_target_dir = cache_context.cache_dir.join("desugarer-target");
            pending_desugarer_artifact = build_pending_desugarer_artifact_with_direct_rustc(
                context,
                &companion_crate_root,
                &cargo_manifest_path,
                &desugarer_target_dir,
                &package_name,
                desugarer,
            )?;
        } else {
            ensure_companion_supports_cdylib(&cargo_manifest_path)?;
            ensure_rust_target_installed(&desugarer.target)?;
            let desugarer_target_dir = cache_context.cache_dir.join("desugarer-target");
            run_cargo_build_for_target(
                &cargo_manifest_path,
                &desugarer_target_dir,
                &desugarer.target,
                &desugarer.profile,
            )?;
            pending_desugarer_artifact =
                build_pending_desugarer_artifact(&desugarer_target_dir, &package_name, metadata.desugarer.as_ref())?;
        }
    }
    if !cache_hit
        || (mode == VocabExtractionMode::PackageArtifacts
            && pending_desugarer_artifact.is_some()
            && !cached_had_desugarer_artifact)
    {
        write_cached_vocab_companion(&cache_context, &metadata, pending_desugarer_artifact.as_ref())?;
    }
    let compatibility_activations = project_soft_keyword_activations(&metadata.keyword_registrations);
    let pending_desugarer_artifact = match mode {
        VocabExtractionMode::PackageArtifacts => pending_desugarer_artifact,
        VocabExtractionMode::ParserOnly => None,
    };

    Ok(Some(LibraryVocabExtraction {
        payload: VocabExports {
            crate_path: declared_crate_path,
            package_name,
            keyword_registrations: metadata.keyword_registrations,
            dsl_surfaces: metadata.dsl_surfaces,
            provider_manifest: metadata.library_manifest,
            desugarer_artifact: pending_desugarer_artifact
                .as_ref()
                .map(|artifact| artifact.metadata.clone()),
        },
        compatibility_activations,
        pending_desugarer_artifact,
    }))
}

/// Build the cache identity and directory for one vocab companion crate.
fn vocab_companion_cache_context(
    project_root: &Path,
    companion_crate_root: &Path,
    package_name: &str,
    generated_cargo_target_dir: Option<&Path>,
) -> CliResult<VocabCompanionCacheContext> {
    let fingerprint = vocab_companion_fingerprint(companion_crate_root, package_name)?;
    let cache_base = vocab_companion_cache_base(project_root, generated_cargo_target_dir);
    Ok(VocabCompanionCacheContext {
        cache_dir: cache_base.join(&fingerprint),
        fingerprint,
    })
}

/// Return the root directory that stores vocab companion cache entries for this invocation.
fn vocab_companion_cache_base(project_root: &Path, generated_cargo_target_dir: Option<&Path>) -> PathBuf {
    if let Some(raw) = env::var_os(VOCAB_COMPANION_CACHE_DIR_ENV).filter(|raw| !raw.is_empty()) {
        return resolve_cache_path(project_root, Path::new(&raw));
    }

    if let Some(target_dir) = generated_cargo_target_dir {
        return resolve_cache_path(project_root, target_dir).join("incan-vocab-cache");
    }

    project_root.join("target").join(".incan-vocab-cache")
}

/// Resolve a user-provided cache path using the current directory, falling back to the project root if needed.
fn resolve_cache_path(project_root: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        return path.to_path_buf();
    }

    env::current_dir()
        .unwrap_or_else(|_| project_root.to_path_buf())
        .join(path)
}

/// Compute a stable content fingerprint for the companion inputs that affect extracted metadata or artifacts.
fn vocab_companion_fingerprint(companion_crate_root: &Path, package_name: &str) -> CliResult<String> {
    let mut hasher = Sha256::new();
    hasher.update(b"incan-vocab-companion-cache\0");
    hasher.update(VOCAB_COMPANION_CACHE_FORMAT.to_le_bytes());
    hasher.update(INCAN_VERSION.as_bytes());
    hasher.update(b"\0");
    hasher.update(incan_vocab::VOCAB_METADATA_VERSION.to_le_bytes());
    hasher.update(package_name.as_bytes());
    hasher.update(b"\0");

    for file in vocab_companion_fingerprint_files(companion_crate_root)? {
        let relative_path = normalized_relative_path(companion_crate_root, &file);
        let bytes = fs::read(&file).map_err(|err| {
            CliError::failure(format!(
                "failed to read vocab companion cache input {}: {err}",
                file.display()
            ))
        })?;
        hasher.update(relative_path.as_bytes());
        hasher.update(b"\0");
        hasher.update((bytes.len() as u64).to_le_bytes());
        hasher.update(b"\0");
        hasher.update(&bytes);
        hasher.update(b"\0");
    }

    Ok(hex::encode(hasher.finalize()))
}

/// Collect companion files that participate in the cache fingerprint.
fn vocab_companion_fingerprint_files(companion_crate_root: &Path) -> CliResult<Vec<PathBuf>> {
    let mut files = Vec::new();
    collect_vocab_companion_fingerprint_files(companion_crate_root, &mut files)?;
    files.sort();
    Ok(files)
}

/// Recursively append fingerprint input files while skipping Cargo output and VCS directories.
fn collect_vocab_companion_fingerprint_files(dir: &Path, files: &mut Vec<PathBuf>) -> CliResult<()> {
    let mut entries = fs::read_dir(dir)
        .map_err(|err| {
            CliError::failure(format!(
                "failed to read vocab companion directory {}: {err}",
                dir.display()
            ))
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|err| {
            CliError::failure(format!(
                "failed to read vocab companion directory {}: {err}",
                dir.display()
            ))
        })?;
    entries.sort_by_key(std::fs::DirEntry::path);

    for entry in entries {
        let path = entry.path();
        let file_name = entry.file_name();
        if path.is_dir() {
            if matches!(file_name.to_str(), Some("target" | ".git")) {
                continue;
            }
            collect_vocab_companion_fingerprint_files(&path, files)?;
        } else if path.is_file() {
            files.push(path);
        }
    }

    Ok(())
}

/// Convert a fingerprint input path to a platform-independent relative path label.
fn normalized_relative_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

/// Read a valid vocab companion cache entry, returning `None` when the cache is absent, stale, or corrupt.
fn read_cached_vocab_companion(context: &VocabCompanionCacheContext) -> CliResult<Option<CachedVocabCompanion>> {
    let cache_file = context.cache_dir.join(VOCAB_COMPANION_CACHE_FILE);
    let bytes = match fs::read(&cache_file) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Ok(None),
    };
    let envelope = match serde_json::from_slice::<VocabCompanionCacheEnvelope>(&bytes) {
        Ok(envelope) => envelope,
        Err(_) => return Ok(None),
    };
    if envelope.cache_format != VOCAB_COMPANION_CACHE_FORMAT
        || envelope.compiler_version != INCAN_VERSION
        || envelope.vocab_metadata_version != incan_vocab::VOCAB_METADATA_VERSION
        || envelope.fingerprint != context.fingerprint
    {
        return Ok(None);
    }
    let pending_desugarer_artifact = match envelope.desugarer_artifact {
        Some(cached) => cached_pending_desugarer_artifact(context, cached)?,
        None => None,
    };

    Ok(Some(CachedVocabCompanion {
        metadata: envelope.metadata,
        pending_desugarer_artifact,
    }))
}

/// Rehydrate a cached desugarer artifact when its stored bytes still match the recorded digest.
fn cached_pending_desugarer_artifact(
    context: &VocabCompanionCacheContext,
    cached: CachedDesugarerArtifact,
) -> CliResult<Option<PendingDesugarerArtifact>> {
    let source_path = context.cache_dir.join("desugarers").join(&cached.file_name);
    let bytes = match fs::read(&source_path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Ok(None),
    };
    let sha256 = hex::encode(Sha256::digest(&bytes));
    if sha256 != cached.metadata.sha256 {
        return Ok(None);
    }
    Ok(Some(PendingDesugarerArtifact {
        metadata: cached.metadata,
        source_path,
    }))
}

/// Persist extracted companion metadata and any packaged desugarer artifact under the fingerprinted cache directory.
fn write_cached_vocab_companion(
    context: &VocabCompanionCacheContext,
    metadata: &incan_vocab::VocabMetadata,
    pending_desugarer_artifact: Option<&PendingDesugarerArtifact>,
) -> CliResult<()> {
    fs::create_dir_all(&context.cache_dir).map_err(|err| {
        CliError::failure(format!(
            "failed to create vocab companion cache directory {}: {err}",
            context.cache_dir.display()
        ))
    })?;
    let desugarer_artifact = pending_desugarer_artifact
        .map(|artifact| cache_desugarer_artifact(context, artifact))
        .transpose()?;
    let envelope = VocabCompanionCacheEnvelope {
        cache_format: VOCAB_COMPANION_CACHE_FORMAT,
        compiler_version: INCAN_VERSION.to_string(),
        vocab_metadata_version: incan_vocab::VOCAB_METADATA_VERSION,
        fingerprint: context.fingerprint.clone(),
        metadata: metadata.clone(),
        desugarer_artifact,
    };
    let payload = serde_json::to_vec_pretty(&envelope)
        .map_err(|err| CliError::failure(format!("failed to encode vocab companion cache metadata: {err}")))?;
    let cache_file = context.cache_dir.join(VOCAB_COMPANION_CACHE_FILE);
    fs::write(&cache_file, payload).map_err(|err| {
        CliError::failure(format!(
            "failed to write vocab companion cache {}: {err}",
            cache_file.display()
        ))
    })
}

/// Copy a validated desugarer artifact into the cache and return its cache-local metadata.
fn cache_desugarer_artifact(
    context: &VocabCompanionCacheContext,
    artifact: &PendingDesugarerArtifact,
) -> CliResult<CachedDesugarerArtifact> {
    let file_name = artifact_cache_file_name(&artifact.metadata)?;
    let destination_dir = context.cache_dir.join("desugarers");
    fs::create_dir_all(&destination_dir).map_err(|err| {
        CliError::failure(format!(
            "failed to create vocab desugarer cache directory {}: {err}",
            destination_dir.display()
        ))
    })?;
    let destination = destination_dir.join(&file_name);
    fs::copy(&artifact.source_path, &destination).map_err(|err| {
        CliError::failure(format!(
            "failed to cache vocab desugarer artifact {} -> {}: {err}",
            artifact.source_path.display(),
            destination.display()
        ))
    })?;

    Ok(CachedDesugarerArtifact {
        metadata: artifact.metadata.clone(),
        file_name,
    })
}

/// Derive the cache-local artifact filename from the packaged desugarer metadata.
fn artifact_cache_file_name(metadata: &VocabDesugarerArtifact) -> CliResult<String> {
    Path::new(&metadata.relative_path)
        .file_name()
        .and_then(|name| name.to_str())
        .map(str::to_string)
        .ok_or_else(|| {
            CliError::failure(format!(
                "invalid vocab desugarer relative path for cache: {}",
                metadata.relative_path
            ))
        })
}

fn resolve_companion_crate_root(project_root: &Path, declared_crate_path: &str) -> PathBuf {
    let crate_path = PathBuf::from(declared_crate_path);
    if crate_path.is_absolute() {
        crate_path
    } else {
        project_root.join(crate_path)
    }
}

fn validate_companion_crate_root(crate_root: &Path) -> CliResult<()> {
    if !crate_root.exists() {
        return Err(CliError::failure(format!(
            "`[vocab].crate` does not exist: {}",
            crate_root.display()
        )));
    }
    if !crate_root.is_dir() {
        return Err(CliError::failure(format!(
            "`[vocab].crate` must point to a directory: {}",
            crate_root.display()
        )));
    }

    let cargo_toml = crate_root.join("Cargo.toml");
    if !cargo_toml.is_file() {
        return Err(CliError::failure(format!(
            "vocab companion crate is missing Cargo.toml: {}",
            cargo_toml.display()
        )));
    }

    let lib_rs = crate_root.join("src").join("lib.rs");
    if !lib_rs.is_file() {
        return Err(CliError::failure(format!(
            "vocab companion crate is missing src/lib.rs: {}",
            lib_rs.display()
        )));
    }

    Ok(())
}

fn read_companion_package_name(cargo_manifest_path: &Path) -> CliResult<String> {
    let content = std::fs::read_to_string(cargo_manifest_path)
        .map_err(|err| CliError::failure(format!("failed to read {}: {err}", cargo_manifest_path.display())))?;
    let cargo_toml = toml::from_str::<toml::Value>(&content)
        .map_err(|err| CliError::failure(format!("failed to parse {}: {err}", cargo_manifest_path.display())))?;

    let package_name = cargo_toml
        .get("package")
        .and_then(toml::Value::as_table)
        .and_then(|pkg| pkg.get("name"))
        .and_then(toml::Value::as_str)
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .ok_or_else(|| {
            CliError::failure(format!(
                "vocab companion crate {} is missing [package].name",
                cargo_manifest_path.display()
            ))
        })?;

    Ok(package_name.to_string())
}

/// Build one generated vocab companion for the requested Rust target and profile.
fn run_cargo_build_for_target(
    cargo_manifest_path: &Path,
    target_dir: &Path,
    target: &str,
    profile: &str,
) -> CliResult<()> {
    let mut command = crate::backend::project::runner::cargo_command();
    crate::backend::project::runner::configure_cargo_target(&mut command, target_dir);
    command.arg("build").arg("--manifest-path").arg(cargo_manifest_path);
    if profile == "release" {
        command.arg("--release");
    }
    command.arg("--target").arg(target).arg("--quiet");

    let output = command
        .output()
        .map_err(|err| CliError::failure(format!("failed to run cargo build for vocab desugarer target: {err}")))?;
    if output.status.success() {
        return Ok(());
    }

    let stderr = String::from_utf8_lossy(&output.stderr);
    Err(CliError::failure(format!(
        "vocab companion crate failed to build desugarer target `{target}` profile `{profile}` ({}):\n{}",
        cargo_manifest_path.display(),
        stderr.trim()
    )))
}

fn ensure_companion_supports_cdylib(cargo_manifest_path: &Path) -> CliResult<()> {
    let content = fs::read_to_string(cargo_manifest_path)
        .map_err(|err| CliError::failure(format!("failed to read {}: {err}", cargo_manifest_path.display())))?;
    let cargo_toml = toml::from_str::<toml::Value>(&content)
        .map_err(|err| CliError::failure(format!("failed to parse {}: {err}", cargo_manifest_path.display())))?;
    let has_cdylib = cargo_toml
        .get("lib")
        .and_then(toml::Value::as_table)
        .and_then(|lib| lib.get("crate-type"))
        .and_then(toml::Value::as_array)
        .map(|crate_types| {
            crate_types
                .iter()
                .filter_map(toml::Value::as_str)
                .any(|crate_type| crate_type == "cdylib")
        })
        .unwrap_or(false);
    if has_cdylib {
        Ok(())
    } else {
        Err(CliError::failure(format!(
            "vocab companion crate `{}` must declare `[lib].crate-type` including `cdylib` to package a desugarer (example: `crate-type = [\"rlib\", \"cdylib\"]`)",
            cargo_manifest_path.display()
        )))
    }
}

fn ensure_rust_target_installed(target: &str) -> CliResult<()> {
    let output = Command::new("rustup")
        .arg("target")
        .arg("list")
        .arg("--installed")
        .output()
        .map_err(|err| {
            CliError::failure(format!(
                "failed to check installed Rust targets for vocab desugarer build: {err}"
            ))
        })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(CliError::failure(format!(
            "failed to list installed Rust targets for vocab desugarer build:\n{}",
            stderr.trim()
        )));
    }
    let installed = parse_installed_rust_targets(&String::from_utf8_lossy(&output.stdout));
    if installed.contains(target) {
        return Ok(());
    }
    Err(CliError::failure(format!(
        "vocab desugarer target `{target}` is not installed in the Rust toolchain. Install it with `rustup target add {target}`."
    )))
}

fn parse_installed_rust_targets(stdout: &str) -> HashSet<String> {
    stdout
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(std::string::ToString::to_string)
        .collect()
}

/// Run the vocab extraction helper against a companion crate entrypoint.
fn extract_vocab_metadata_from_library_entrypoint(
    companion_crate_root: &Path,
    package_name: &str,
    target_dir: &Path,
    direct_rustc: Option<&OvenVocabDirectRustcContext>,
) -> CliResult<incan_vocab::VocabMetadata> {
    if let Some(context) = direct_rustc {
        return extract_vocab_metadata_with_direct_rustc(context, companion_crate_root, package_name);
    }
    if env::var_os(OVEN_COMPILER_SUITE_RUSTC_ENV).is_some() {
        let context = oven_compiler_suite_rustc_context()?.ok_or_else(|| {
            CliError::failure(
                "stored Oven compiler suite has no direct-Rustc vocab companion capability; Cargo fallback is forbidden"
                    .to_string(),
            )
        })?;
        return extract_vocab_metadata_with_direct_rustc(&context, companion_crate_root, package_name);
    }

    let extraction_dir = create_extraction_workspace_dir()?;
    let helper_root = extraction_dir.join("runner");
    fs::create_dir_all(helper_root.join("src")).map_err(|err| {
        CliError::failure(format!(
            "failed to create vocab extraction workspace {}: {err}",
            helper_root.display()
        ))
    })?;
    write_extraction_runner_manifest(&helper_root, companion_crate_root, package_name)?;
    write_extraction_runner_source(&helper_root)?;

    let mut command = crate::backend::project::runner::cargo_command();
    crate::backend::project::runner::configure_cargo_target(&mut command, target_dir);
    let output = command
        .arg("run")
        .arg("--quiet")
        .arg("--manifest-path")
        .arg(helper_root.join("Cargo.toml"))
        .output()
        .map_err(|err| CliError::failure(format!("failed to run vocab extraction helper: {err}")))?;

    let metadata_result = if output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        serde_json::from_str::<incan_vocab::VocabMetadata>(stdout.trim()).map_err(|err| {
            CliError::failure(format!(
                "failed to parse metadata extracted from `library_vocab()` in {}: {err}",
                companion_crate_root.display()
            ))
        })
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(CliError::failure(format!(
            "failed to extract vocab metadata from companion crate via `library_vocab()` ({}):\n{}",
            companion_crate_root.display(),
            stderr.trim()
        )))
    };

    let _ = fs::remove_dir_all(&extraction_dir);
    metadata_result
}

/// Load the direct-Rustc capability exported by an active Oven compiler-suite scheduler.
///
/// A stored-suite child receives individual paths instead of a path list because Oven identities contain `:`. Every
/// supplied path must be an absolute regular file or directory, and any malformed capability fails closed instead of
/// using Cargo as a recovery path.
fn oven_compiler_suite_rustc_context() -> CliResult<Option<OvenVocabDirectRustcContext>> {
    let Some(rustc) = env::var_os(OVEN_COMPILER_SUITE_VOCAB_RUSTC_ENV).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    let rustc = PathBuf::from(rustc);
    if !rustc.is_absolute() || !rustc.is_file() {
        return Err(CliError::failure(format!(
            "stored Oven compiler suite provided invalid Rust compiler {}",
            rustc.display()
        )));
    }
    let dependency_path_count = oven_compiler_suite_path_count(OVEN_COMPILER_SUITE_VOCAB_DEPENDENCY_PATH_COUNT_ENV)?;
    let extern_count = oven_compiler_suite_path_count(OVEN_COMPILER_SUITE_VOCAB_EXTERN_COUNT_ENV)?;
    let mut dependency_search_paths = Vec::with_capacity(dependency_path_count);
    for index in 0..dependency_path_count {
        let variable = format!("INCAN_OVEN_COMPILER_SUITE_VOCAB_DEPENDENCY_PATH_{index}");
        let path = oven_compiler_suite_absolute_path(&variable, false)?;
        dependency_search_paths.push(path);
    }
    let mut externs = BTreeMap::new();
    for index in 0..extern_count {
        let name_variable = format!("INCAN_OVEN_COMPILER_SUITE_VOCAB_EXTERN_{index}_NAME");
        let path_variable = format!("INCAN_OVEN_COMPILER_SUITE_VOCAB_EXTERN_{index}_PATH");
        let crate_name = env::var(&name_variable).map_err(|_| {
            CliError::failure(format!(
                "stored Oven compiler suite did not provide direct-Rustc extern name {index}"
            ))
        })?;
        if crate_name.is_empty()
            || !crate_name
                .chars()
                .all(|character| character == '_' || character.is_ascii_alphanumeric())
        {
            return Err(CliError::failure(format!(
                "stored Oven compiler suite provided invalid direct-Rustc extern name `{crate_name}`"
            )));
        }
        let path = oven_compiler_suite_absolute_path(&path_variable, true)?;
        if externs.insert(crate_name.clone(), path).is_some() {
            return Err(CliError::failure(format!(
                "stored Oven compiler suite repeated direct-Rustc extern `{crate_name}`"
            )));
        }
    }
    for required in ["incan_vocab", "serde_json"] {
        if !externs.contains_key(required) {
            return Err(CliError::failure(format!(
                "stored Oven compiler suite direct-Rustc closure lacks required `{required}` for vocab extraction"
            )));
        }
    }
    Ok(Some(OvenVocabDirectRustcContext {
        rustc,
        dependency_search_paths,
        externs,
        auxiliary_targets: BTreeMap::new(),
    }))
}

/// Derive the vocabulary-helper capability from an already materialized normal Oven plan.
///
/// The caller retains the matching store lease for the complete extraction. This deliberately accepts only the
/// compiler-owned helper roots baked into the selected immutable closure, so a normal library build cannot use an
/// arbitrary companion dependency or recover by invoking Cargo.
pub(crate) fn oven_vocab_direct_rustc_context_from_plan(
    rustc: &Path,
    plan: &OvenRustcArtifactPlan,
    artifacts: &OvenRustcArtifactManifest,
    artifact_root: &Path,
) -> CliResult<OvenVocabDirectRustcContext> {
    if !rustc.is_absolute() || !rustc.is_file() {
        return Err(CliError::failure(format!(
            "selected Oven vocabulary Rust compiler is invalid: {}",
            rustc.display()
        )));
    }
    for path in &plan.dependency_search_paths {
        if !path.is_absolute() || !path.is_dir() {
            return Err(CliError::failure(format!(
                "selected Oven vocabulary dependency search path is invalid: {}",
                path.display()
            )));
        }
    }
    let mut externs = BTreeMap::new();
    for (crate_name, path) in &plan.externs {
        if crate_name.is_empty()
            || !crate_name
                .chars()
                .all(|character| character == '_' || character.is_ascii_alphanumeric())
            || !path.is_absolute()
            || !path.is_file()
        {
            return Err(CliError::failure(format!(
                "selected Oven vocabulary extern `{crate_name}` is invalid: {}",
                path.display()
            )));
        }
        if externs.insert(crate_name.clone(), path.clone()).is_some() {
            return Err(CliError::failure(format!(
                "selected Oven vocabulary closure repeats direct-Rustc extern `{crate_name}`"
            )));
        }
    }
    for required in ["incan_vocab", "serde_json"] {
        if !externs.contains_key(required) {
            return Err(CliError::failure(format!(
                "selected Oven vocabulary closure lacks required `{required}`; normal library builds will not invoke Cargo"
            )));
        }
    }
    let mut auxiliary_targets = BTreeMap::new();
    for auxiliary in &artifacts.vocab_auxiliary_targets {
        let auxiliary_plan = artifacts
            .materialize_trusted_vocab_auxiliary_target(artifact_root, &auxiliary.target)
            .map_err(|error| CliError::failure(error.to_string()))?
            .ok_or_else(|| {
                CliError::failure(format!(
                    "selected Oven vocabulary closure omitted declared auxiliary target `{}`",
                    auxiliary.target
                ))
            })?;
        let target_context = oven_vocab_auxiliary_target_context(&auxiliary.target, auxiliary_plan)?;
        if auxiliary_targets
            .insert(auxiliary.target.clone(), target_context)
            .is_some()
        {
            return Err(CliError::failure(format!(
                "selected Oven vocabulary closure repeats auxiliary target `{}`",
                auxiliary.target
            )));
        }
    }
    Ok(OvenVocabDirectRustcContext {
        rustc: rustc.to_path_buf(),
        dependency_search_paths: plan.dependency_search_paths.clone(),
        externs,
        auxiliary_targets,
    })
}

/// Verify the exact target-specific roots needed to compile a vocabulary companion without Cargo.
fn oven_vocab_auxiliary_target_context(
    target: &str,
    plan: OvenRustcAuxiliaryTargetPlan,
) -> CliResult<OvenVocabAuxiliaryTargetContext> {
    let mut externs = BTreeMap::new();
    for (crate_name, path) in plan.externs {
        if crate_name.is_empty()
            || !crate_name
                .chars()
                .all(|character| character == '_' || character.is_ascii_alphanumeric())
            || !path.is_absolute()
            || !path.is_file()
        {
            return Err(CliError::failure(format!(
                "selected Oven vocabulary auxiliary target `{target}` has invalid extern `{crate_name}`: {}",
                path.display()
            )));
        }
        if externs.insert(crate_name.clone(), path).is_some() {
            return Err(CliError::failure(format!(
                "selected Oven vocabulary auxiliary target `{target}` repeats extern `{crate_name}`"
            )));
        }
    }
    if !externs.contains_key("incan_vocab") {
        return Err(CliError::failure(format!(
            "selected Oven vocabulary auxiliary target `{target}` lacks required `incan_vocab`"
        )));
    }
    Ok(OvenVocabAuxiliaryTargetContext {
        dependency_search_paths: plan.dependency_search_paths,
        externs,
    })
}

/// Read one bounded count from the scheduler-provided direct-Rustc environment.
fn oven_compiler_suite_path_count(variable: &str) -> CliResult<usize> {
    const MAX_COMPILER_SUITE_DIRECT_RUSTC_INPUTS: usize = 1024;
    let value = env::var(variable)
        .map_err(|_| CliError::failure(format!("stored Oven compiler suite did not provide `{variable}`")))?;
    let count = value
        .parse::<usize>()
        .map_err(|error| CliError::failure(format!("stored Oven compiler suite has invalid `{variable}`: {error}")))?;
    if count > MAX_COMPILER_SUITE_DIRECT_RUSTC_INPUTS {
        return Err(CliError::failure(format!(
            "stored Oven compiler suite `{variable}` exceeds the direct-Rustc input limit {MAX_COMPILER_SUITE_DIRECT_RUSTC_INPUTS}"
        )));
    }
    Ok(count)
}

/// Read one verified-looking direct-Rustc capability path from the compiler-suite environment.
fn oven_compiler_suite_absolute_path(variable: &str, regular_file: bool) -> CliResult<PathBuf> {
    let path = env::var_os(variable)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| CliError::failure(format!("stored Oven compiler suite did not provide `{variable}`")))?;
    let valid_kind = if regular_file { path.is_file() } else { path.is_dir() };
    if !path.is_absolute() || !valid_kind {
        return Err(CliError::failure(format!(
            "stored Oven compiler suite provided invalid `{variable}` path {}",
            path.display()
        )));
    }
    Ok(path)
}

/// Extract a companion's registration through the suite's receipt-bound direct-Rustc closure.
///
/// This intentionally supports only dependencies already named by that closure. A companion that needs an arbitrary
/// Cargo package fails as an unsupported direct compilation; the normal command never resolves or launches Cargo.
fn extract_vocab_metadata_with_direct_rustc(
    context: &OvenVocabDirectRustcContext,
    companion_crate_root: &Path,
    package_name: &str,
) -> CliResult<incan_vocab::VocabMetadata> {
    let extraction_dir = create_extraction_workspace_dir()?;
    let result = (|| {
        let (companion_source, edition, version) = vocab_companion_rustc_inputs(companion_crate_root)?;
        let companion_output = extraction_dir.join("libcompanion.rlib");
        run_vocab_direct_rustc(
            context,
            companion_crate_root,
            &companion_source,
            "companion",
            "rlib",
            &edition,
            package_name,
            &version,
            &companion_output,
            None,
            None,
            "compile vocab companion",
        )?;

        let helper_root = extraction_dir.join("runner");
        fs::create_dir_all(helper_root.join("src")).map_err(|err| {
            CliError::failure(format!(
                "failed to create direct vocab extraction workspace {}: {err}",
                helper_root.display()
            ))
        })?;
        write_extraction_runner_source(&helper_root)?;
        let helper_output = helper_root.join("vocab-extraction-runner");
        run_vocab_direct_rustc(
            context,
            &helper_root,
            &helper_root.join("src/main.rs"),
            "incan_vocab_extraction_runner",
            "bin",
            "2021",
            "incan_vocab_extraction_runner",
            "0.1.0",
            &helper_output,
            Some(("companion", companion_output.as_path())),
            None,
            "compile vocab extraction helper",
        )?;
        let mut command = Command::new(&helper_output);
        command.current_dir(companion_crate_root);
        clear_inherited_cargo_environment(&mut command);
        let output = command.output().map_err(|err| {
            CliError::failure(format!(
                "failed to run direct vocab extraction helper {}: {err}",
                helper_output.display()
            ))
        })?;
        if !output.status.success() {
            return Err(CliError::failure(format!(
                "failed to extract vocab metadata from companion crate via direct Rustc ({})\n{}",
                companion_crate_root.display(),
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        serde_json::from_str::<incan_vocab::VocabMetadata>(stdout.trim()).map_err(|err| {
            CliError::failure(format!(
                "failed to parse metadata extracted from `library_vocab()` in {}: {err}",
                companion_crate_root.display()
            ))
        })
    })();
    let _ = fs::remove_dir_all(&extraction_dir);
    result
}

/// Read the direct-Rustc-relevant companion manifest fields without asking Cargo to interpret the package.
fn vocab_companion_rustc_inputs(companion_crate_root: &Path) -> CliResult<(PathBuf, String, String)> {
    let manifest_path = companion_crate_root.join("Cargo.toml");
    let content = fs::read_to_string(&manifest_path)
        .map_err(|err| CliError::failure(format!("failed to read {}: {err}", manifest_path.display())))?;
    let manifest = toml::from_str::<toml::Value>(&content)
        .map_err(|err| CliError::failure(format!("failed to parse {}: {err}", manifest_path.display())))?;
    let package = manifest.get("package").and_then(toml::Value::as_table).ok_or_else(|| {
        CliError::failure(format!(
            "vocab companion crate {} is missing [package]",
            manifest_path.display()
        ))
    })?;
    let edition = package
        .get("edition")
        .and_then(toml::Value::as_str)
        .unwrap_or("2015")
        .to_string();
    let version = package
        .get("version")
        .and_then(toml::Value::as_str)
        .unwrap_or("0.0.0")
        .to_string();
    let source = manifest
        .get("lib")
        .and_then(toml::Value::as_table)
        .and_then(|library| library.get("path"))
        .and_then(toml::Value::as_str)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("src/lib.rs"));
    let source = companion_crate_root.join(source);
    if !source.is_file() {
        return Err(CliError::failure(format!(
            "vocab companion crate {} has no direct-Rustc library source {}",
            companion_crate_root.display(),
            source.display()
        )));
    }
    Ok((source, edition, version))
}

/// Compile one temporary vocab companion/helper source through only the already selected Oven closure.
#[allow(clippy::too_many_arguments)]
fn run_vocab_direct_rustc(
    context: &OvenVocabDirectRustcContext,
    current_dir: &Path,
    source: &Path,
    crate_name: &str,
    crate_type: &str,
    edition: &str,
    package_name: &str,
    package_version: &str,
    output: &Path,
    additional_extern: Option<(&str, &Path)>,
    target: Option<&str>,
    action: &str,
) -> CliResult<()> {
    let (dependency_search_paths, externs) = if let Some(target) = target {
        let auxiliary = context.auxiliary_targets.get(target).ok_or_else(|| {
            CliError::failure(format!(
                "Oven Alpha has no sealed direct-Rustc vocabulary closure for desugarer target `{target}`; normal library builds will not invoke Cargo"
            ))
        })?;
        (&auxiliary.dependency_search_paths, &auxiliary.externs)
    } else {
        (&context.dependency_search_paths, &context.externs)
    };
    if let Some((name, _)) = additional_extern
        && externs.contains_key(name)
    {
        return Err(CliError::failure(format!(
            "stored Oven compiler suite direct-Rustc closure conflicts with helper extern `{name}`"
        )));
    }
    let parent = output.parent().ok_or_else(|| {
        CliError::failure(format!(
            "direct vocab output {} has no parent directory",
            output.display()
        ))
    })?;
    fs::create_dir_all(parent).map_err(|err| {
        CliError::failure(format!(
            "failed to create direct vocab output directory {}: {err}",
            parent.display()
        ))
    })?;
    let mut command = Command::new(&context.rustc);
    command
        .current_dir(current_dir)
        .arg(source)
        .arg("--crate-name")
        .arg(crate_name)
        .arg("--crate-type")
        .arg(crate_type)
        .arg("--edition")
        .arg(edition)
        .arg("-o")
        .arg(output);
    if let Some(target) = target {
        command.arg("--target").arg(target);
    }
    for path in dependency_search_paths {
        command.arg("-L").arg(format!("dependency={}", path.display()));
    }
    for (name, path) in externs {
        command.arg("--extern").arg(format!("{name}={}", path.display()));
    }
    if let Some((name, path)) = additional_extern {
        command.arg("--extern").arg(format!("{name}={}", path.display()));
    }
    clear_inherited_cargo_environment(&mut command);
    command
        .env("CARGO_MANIFEST_DIR", current_dir)
        .env("CARGO_PKG_NAME", package_name)
        .env("CARGO_PKG_VERSION", package_version);
    let output_result = command
        .output()
        .map_err(|err| CliError::failure(format!("failed to {action} with direct Rustc: {err}")))?;
    if output_result.status.success() {
        return Ok(());
    }
    Err(CliError::failure(format!(
        "failed to {action} with the stored Oven direct-Rustc closure:\n{}",
        String::from_utf8_lossy(&output_result.stderr).trim()
    )))
}

/// Build a declared Wasm desugarer through the receipt-selected cross-target Oven closure.
fn build_pending_desugarer_artifact_with_direct_rustc(
    context: &OvenVocabDirectRustcContext,
    companion_crate_root: &Path,
    cargo_manifest_path: &Path,
    target_dir: &Path,
    package_name: &str,
    desugarer: &incan_vocab::DesugarerMetadata,
) -> CliResult<Option<PendingDesugarerArtifact>> {
    if !matches!(desugarer.artifact_kind, incan_vocab::DesugarerArtifactKind::WasmModule) {
        return Err(CliError::failure(
            "unsupported vocab desugarer artifact kind (expected WasmModule)".to_string(),
        ));
    }
    ensure_companion_supports_cdylib(cargo_manifest_path)?;
    ensure_rust_target_installed(&desugarer.target)?;
    let (source, edition, version) = vocab_companion_rustc_inputs(companion_crate_root)?;
    let artifact_file_name = desugarer
        .file_name
        .clone()
        .unwrap_or_else(|| format!("{}.wasm", package_name.replace('-', "_")));
    let output = target_dir
        .join(&desugarer.target)
        .join(&desugarer.profile)
        .join(&artifact_file_name);
    run_vocab_direct_rustc(
        context,
        companion_crate_root,
        &source,
        &package_name.replace('-', "_"),
        "cdylib",
        &edition,
        package_name,
        &version,
        &output,
        None,
        Some(&desugarer.target),
        "compile vocab Wasm desugarer",
    )?;
    build_pending_desugarer_artifact(target_dir, package_name, Some(desugarer))
}

fn ensure_supported_vocab_metadata_version(
    metadata: &incan_vocab::VocabMetadata,
    companion_crate_root: &Path,
) -> CliResult<()> {
    if metadata.metadata_version == 0 {
        return Err(CliError::failure(format!(
            "companion crate `{}` produced invalid vocab metadata version 0",
            companion_crate_root.display()
        )));
    }
    if metadata.metadata_version > incan_vocab::VOCAB_METADATA_VERSION {
        return Err(CliError::failure(format!(
            "companion crate `{}` produced vocab metadata version {} but this compiler supports up to {}",
            companion_crate_root.display(),
            metadata.metadata_version,
            incan_vocab::VOCAB_METADATA_VERSION
        )));
    }
    Ok(())
}

fn create_extraction_workspace_dir() -> CliResult<PathBuf> {
    static EXTRACTION_COUNTER: AtomicU64 = AtomicU64::new(0);
    let nonce = format!(
        "{}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|err| CliError::failure(format!("failed to compute extraction workspace timestamp: {err}")))?
            .as_nanos(),
        EXTRACTION_COUNTER.fetch_add(1, Ordering::Relaxed)
    );
    let dir = env::temp_dir().join(format!("incan_vocab_extract_{nonce}"));
    fs::create_dir_all(&dir).map_err(|err| {
        CliError::failure(format!(
            "failed to create temporary vocab extraction directory {}: {err}",
            dir.display()
        ))
    })?;
    Ok(dir)
}

/// Write the temporary Cargo package that calls the companion crate's `library_vocab()` entrypoint.
fn write_extraction_runner_manifest(
    helper_root: &Path,
    companion_crate_root: &Path,
    package_name: &str,
) -> CliResult<()> {
    let helper_manifest = helper_root.join("Cargo.toml");
    let escaped_companion_path = escape_cargo_toml_string(companion_crate_root);
    let escaped_package_name = package_name.replace('\\', "\\\\").replace('"', "\\\"");
    let manifest = format!(
        "[package]\nname = \"incan_vocab_extraction_runner\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[workspace]\n\n[dependencies]\ncompanion = {{ package = \"{escaped_package_name}\", path = \"{escaped_companion_path}\" }}\nserde_json = \"1.0\"\n"
    );
    fs::write(&helper_manifest, manifest).map_err(|err| {
        CliError::failure(format!(
            "failed to write vocab extraction helper manifest {}: {err}",
            helper_manifest.display()
        ))
    })?;
    copy_workspace_lockfile_to_extraction_runner(helper_root)
}

/// Seed the temporary helper with the repo lockfile so path-only vocab tests do not re-resolve crates.io.
fn copy_workspace_lockfile_to_extraction_runner(helper_root: &Path) -> CliResult<()> {
    let workspace_lockfile = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.lock");
    if !workspace_lockfile.is_file() {
        return Ok(());
    }

    let helper_lockfile = helper_root.join("Cargo.lock");
    fs::copy(&workspace_lockfile, &helper_lockfile).map_err(|err| {
        CliError::failure(format!(
            "failed to copy workspace lockfile {} to vocab extraction helper {}: {err}",
            workspace_lockfile.display(),
            helper_lockfile.display()
        ))
    })?;
    Ok(())
}

/// Write the Rust entrypoint for the temporary Cargo package that prints serialized vocab metadata.
fn write_extraction_runner_source(helper_root: &Path) -> CliResult<()> {
    let source_path = helper_root.join("src").join("main.rs");
    let source = "fn main() {\n    let registration = companion::library_vocab();\n    let metadata = registration.metadata();\n    let text = match serde_json::to_string_pretty(&metadata) {\n        Ok(text) => text,\n        Err(err) => {\n            eprintln!(\"failed to serialize registration metadata: {err}\");\n            std::process::exit(1);\n        }\n    };\n    print!(\"{text}\");\n}\n";
    fs::write(&source_path, source).map_err(|err| {
        CliError::failure(format!(
            "failed to write vocab extraction helper source {}: {err}",
            source_path.display()
        ))
    })
}

fn escape_cargo_toml_string(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "\\\\").replace('"', "\\\"")
}

/// Materialize a declared vocab desugarer into the selected generated target.
fn build_pending_desugarer_artifact(
    target_dir: &Path,
    package_name: &str,
    desugarer: Option<&incan_vocab::DesugarerMetadata>,
) -> CliResult<Option<PendingDesugarerArtifact>> {
    let Some(desugarer) = desugarer else {
        return Ok(None);
    };

    let artifact_kind = desugarer.artifact_kind;
    if !matches!(artifact_kind, incan_vocab::DesugarerArtifactKind::WasmModule) {
        return Err(CliError::failure(
            "unsupported vocab desugarer artifact kind (expected WasmModule)".to_string(),
        ));
    }

    let artifact_file_name = desugarer
        .file_name
        .clone()
        .unwrap_or_else(|| format!("{}.wasm", package_name.replace('-', "_")));
    let source_path = target_dir
        .join(&desugarer.target)
        .join(&desugarer.profile)
        .join(&artifact_file_name);

    if !source_path.is_file() {
        return Err(CliError::failure(format!(
            "vocab desugarer artifact not found at {} (build companion crate for target `{}` profile `{}` first)",
            source_path.display(),
            desugarer.target,
            desugarer.profile
        )));
    }

    let bytes = fs::read(&source_path).map_err(|err| {
        CliError::failure(format!(
            "failed to read vocab desugarer artifact at {}: {err}",
            source_path.display()
        ))
    })?;
    validate_wasm_desugarer_entrypoint(&source_path, &bytes, &desugarer.entrypoint)?;
    let sha256 = hex::encode(Sha256::digest(&bytes));

    Ok(Some(PendingDesugarerArtifact {
        metadata: VocabDesugarerArtifact {
            artifact_kind,
            abi_version: desugarer.abi_version,
            relative_path: format!("desugarers/{artifact_file_name}"),
            target: desugarer.target.clone(),
            profile: desugarer.profile.clone(),
            entrypoint: desugarer.entrypoint.clone(),
            sha256,
        },
        source_path,
    }))
}

fn validate_wasm_desugarer_entrypoint(path: &Path, bytes: &[u8], entrypoint: &str) -> CliResult<()> {
    let mut config = Config::new();
    config.consume_fuel(true);
    let engine = Engine::new(&config)
        .map_err(|err| CliError::failure(format!("failed to initialize wasm validation engine: {err}")))?;
    let module = Module::new(&engine, bytes).map_err(|err| {
        CliError::failure(format!(
            "failed to compile vocab desugarer artifact `{}` as wasm: {err}",
            path.display()
        ))
    })?;
    validate_wasm_memory_export(&module, path)?;
    validate_wasm_func_export(&module, path, entrypoint, Some(ValType::I32))?;
    validate_wasm_func_export(&module, path, incan_vocab::WASM_DESUGAR_INIT_ENTRYPOINT, None)?;
    for &global_name in incan_vocab::WASM_DESUGAR_REQUIRED_I32_GLOBAL_EXPORTS {
        validate_wasm_i32_global_export(&module, path, global_name)?;
    }
    Ok(())
}

fn validate_wasm_memory_export(module: &Module, path: &Path) -> CliResult<()> {
    let Some(export) = module.get_export(incan_vocab::WASM_DESUGAR_MEMORY_EXPORT) else {
        return Err(CliError::failure(format!(
            "vocab desugarer artifact `{}` is missing exported memory `{}`",
            path.display(),
            incan_vocab::WASM_DESUGAR_MEMORY_EXPORT
        )));
    };
    if matches!(export, ExternType::Memory(_)) {
        Ok(())
    } else {
        Err(CliError::failure(format!(
            "vocab desugarer export `{}` in `{}` is not a memory export",
            incan_vocab::WASM_DESUGAR_MEMORY_EXPORT,
            path.display()
        )))
    }
}

fn validate_wasm_func_export(
    module: &Module,
    path: &Path,
    export_name: &str,
    expected_result: Option<ValType>,
) -> CliResult<()> {
    let Some(export) = module.get_export(export_name) else {
        return Err(CliError::failure(format!(
            "vocab desugarer artifact `{}` is missing exported function `{export_name}`",
            path.display()
        )));
    };
    let ExternType::Func(func_ty) = export else {
        return Err(CliError::failure(format!(
            "vocab desugarer export `{export_name}` in `{}` is not a function",
            path.display()
        )));
    };
    let params_ok = func_ty.params().next().is_none();
    let mut results = func_ty.results();
    let result_ok = match expected_result {
        Some(ValType::I32) => matches!(results.next(), Some(ValType::I32)) && results.next().is_none(),
        None => results.next().is_none(),
        Some(_) => false,
    };
    if params_ok && result_ok {
        Ok(())
    } else {
        Err(CliError::failure(format!(
            "vocab desugarer export `{export_name}` in `{}` has an invalid function signature",
            path.display()
        )))
    }
}

fn validate_wasm_i32_global_export(module: &Module, path: &Path, export_name: &str) -> CliResult<()> {
    let Some(export) = module.get_export(export_name) else {
        return Err(CliError::failure(format!(
            "vocab desugarer artifact `{}` is missing exported global `{export_name}`",
            path.display()
        )));
    };
    let ExternType::Global(global_ty) = export else {
        return Err(CliError::failure(format!(
            "vocab desugarer export `{export_name}` in `{}` is not a global",
            path.display()
        )));
    };
    if matches!(global_ty.content(), ValType::I32) {
        Ok(())
    } else {
        Err(CliError::failure(format!(
            "vocab desugarer global `{export_name}` in `{}` must have type `i32`",
            path.display()
        )))
    }
}

fn project_soft_keyword_activations(registrations: &[incan_vocab::KeywordRegistration]) -> Vec<SoftKeywordActivation> {
    let mut dedup = HashSet::new();
    let mut projected = Vec::new();

    for registration in registrations {
        let incan_vocab::KeywordActivation::OnImport { namespace } = &registration.activation else {
            continue;
        };
        for keyword in &registration.keywords {
            let Some(id) = incan_core::lang::keywords::from_str(&keyword.name) else {
                continue;
            };
            if !incan_core::lang::keywords::is_soft(id) {
                continue;
            }

            let key = (namespace.clone(), keyword.name.clone());
            if dedup.insert(key.clone()) {
                projected.push(SoftKeywordActivation {
                    namespace: key.0,
                    keyword: key.1,
                });
            }
        }
    }

    projected.sort_by(|left, right| {
        left.namespace
            .cmp(&right.namespace)
            .then(left.keyword.cmp(&right.keyword))
    });
    projected
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::ProjectManifest;
    use std::fs;

    fn write_vocab_companion_crate(
        project_root: &Path,
        crate_dir: &str,
        package_name: &str,
    ) -> Result<PathBuf, Box<dyn std::error::Error>> {
        let crate_root = project_root.join(crate_dir);
        fs::create_dir_all(crate_root.join("src"))?;
        fs::write(
            crate_root.join("Cargo.toml"),
            format!(
                "[package]\nname = \"{package_name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nincan_vocab = {{ path = \"{}\" }}\n\n[lib]\npath = \"src/lib.rs\"\n",
                PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("crates")
                    .join("incan_vocab")
                    .display()
            ),
        )?;
        fs::write(
            crate_root.join("src/lib.rs"),
            "pub fn library_vocab() -> incan_vocab::VocabRegistration {\n    incan_vocab::VocabRegistration::new().with_keyword_registration(\n        incan_vocab::KeywordRegistration {\n            activation: incan_vocab::KeywordActivation::OnImport {\n                namespace: \"widgets.dsl\".to_string(),\n            },\n            keywords: vec![incan_vocab::KeywordSpec::new(\n                \"await\",\n                incan_vocab::KeywordSurfaceKind::ControlFlow,\n            )],\n            valid_decorators: Vec::new(),\n        }\n    )\n}\n",
        )?;
        Ok(crate_root)
    }

    #[test]
    fn projects_import_activated_soft_keywords() {
        let registrations = vec![
            incan_vocab::KeywordRegistration {
                activation: incan_vocab::KeywordActivation::OnImport {
                    namespace: "mylib.dsl".to_string(),
                },
                keywords: vec![
                    incan_vocab::KeywordSpec::new("await", incan_vocab::KeywordSurfaceKind::ControlFlow),
                    incan_vocab::KeywordSpec::new("def", incan_vocab::KeywordSurfaceKind::FunctionDecl),
                ],
                valid_decorators: Vec::new(),
            },
            incan_vocab::KeywordRegistration {
                activation: incan_vocab::KeywordActivation::Always,
                keywords: vec![incan_vocab::KeywordSpec::new(
                    "await",
                    incan_vocab::KeywordSurfaceKind::ControlFlow,
                )],
                valid_decorators: Vec::new(),
            },
        ];

        let projected = project_soft_keyword_activations(&registrations);
        assert_eq!(
            projected,
            vec![SoftKeywordActivation {
                namespace: "mylib.dsl".to_string(),
                keyword: "await".to_string(),
            }]
        );
    }

    #[test]
    fn resolve_companion_crate_root_uses_project_root_for_relative_paths() {
        let project_root = PathBuf::from("/tmp/incan_project");
        let resolved = resolve_companion_crate_root(&project_root, "crates/mylib_vocab");
        assert_eq!(resolved, project_root.join("crates/mylib_vocab"));
    }

    #[test]
    fn validate_companion_crate_root_rejects_missing_src_lib() -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let crate_root = temp.path().join("vocab_companion");
        fs::create_dir_all(&crate_root)?;
        fs::write(
            crate_root.join("Cargo.toml"),
            "[package]\nname = \"vocab_companion\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )?;

        let err = validate_companion_crate_root(&crate_root)
            .err()
            .ok_or("expected validation failure")?;
        assert!(err.to_string().contains("missing src/lib.rs"));
        Ok(())
    }

    #[test]
    fn read_companion_package_name_reads_package_name() -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let cargo_toml = temp.path().join("Cargo.toml");
        fs::write(
            &cargo_toml,
            "[package]\nname = \"widgets_vocab_companion\"\nversion = \"0.1.0\"\n",
        )?;

        let package_name = read_companion_package_name(&cargo_toml)?;
        assert_eq!(package_name, "widgets_vocab_companion");
        Ok(())
    }

    #[test]
    fn ensure_companion_supports_cdylib_accepts_cdylib_crate_type() -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let cargo_toml = temp.path().join("Cargo.toml");
        fs::write(
            &cargo_toml,
            "[package]\nname = \"widgets_vocab_companion\"\nversion = \"0.1.0\"\n\n[lib]\ncrate-type = [\"rlib\", \"cdylib\"]\n",
        )?;
        ensure_companion_supports_cdylib(&cargo_toml)?;
        Ok(())
    }

    #[test]
    fn ensure_companion_supports_cdylib_rejects_missing_cdylib_crate_type() -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let cargo_toml = temp.path().join("Cargo.toml");
        fs::write(
            &cargo_toml,
            "[package]\nname = \"widgets_vocab_companion\"\nversion = \"0.1.0\"\n\n[lib]\npath = \"src/lib.rs\"\n",
        )?;
        let err = match ensure_companion_supports_cdylib(&cargo_toml) {
            Ok(()) => return Err("expected missing cdylib to fail".into()),
            Err(err) => err,
        };
        assert!(err.to_string().contains("cdylib"));
        Ok(())
    }

    #[test]
    fn extract_vocab_metadata_from_library_entrypoint_parses_valid_payload() -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let crate_root = write_vocab_companion_crate(temp.path(), "vocab_companion", "widgets_vocab_companion")?;
        let parsed = extract_vocab_metadata_from_library_entrypoint(
            &crate_root,
            "widgets_vocab_companion",
            &temp.path().join("extraction-target"),
            None,
        )?;
        assert_eq!(parsed.keyword_registrations.len(), 1);
        assert_eq!(
            parsed.keyword_registrations[0].activation,
            incan_vocab::KeywordActivation::OnImport {
                namespace: "widgets.dsl".to_string()
            }
        );
        Ok(())
    }

    #[test]
    fn extraction_runner_manifest_reuses_workspace_lockfile() -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let helper_root = temp.path().join("runner");
        let companion_root = temp.path().join("vocab_companion");
        fs::create_dir_all(helper_root.join("src"))?;
        fs::create_dir_all(&companion_root)?;

        write_extraction_runner_manifest(&helper_root, &companion_root, "widgets_vocab_companion")?;

        let manifest = fs::read_to_string(helper_root.join("Cargo.toml"))?;
        assert!(manifest.contains("serde_json = \"1.0\""));
        assert!(
            manifest.contains("\n[workspace]\n"),
            "the temporary runner must opt out of an enclosing checkout workspace"
        );
        assert!(
            helper_root.join("Cargo.lock").is_file(),
            "helper runner should inherit the workspace lockfile"
        );
        Ok(())
    }

    #[test]
    fn collect_library_vocab_metadata_requires_library_vocab_entrypoint() -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let project_root = temp.path().join("project");
        fs::create_dir_all(&project_root)?;
        let crate_root = project_root.join("vocab_companion");
        fs::create_dir_all(crate_root.join("src"))?;
        fs::write(
            crate_root.join("Cargo.toml"),
            "[package]\nname = \"widgets_vocab_companion\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[lib]\npath = \"src/lib.rs\"\n",
        )?;
        fs::write(crate_root.join("src/lib.rs"), "pub fn register_vocab() {}\n")?;

        let manifest_path = project_root.join("incan.toml");
        fs::write(
            &manifest_path,
            "[project]\nname = \"widgets\"\nversion = \"0.1.0\"\n\n[vocab]\ncrate = \"vocab_companion\"\n",
        )?;
        let manifest = ProjectManifest::from_str(&fs::read_to_string(&manifest_path)?, &manifest_path)?;

        let err = collect_library_vocab_metadata(&manifest, &project_root, None, None)
            .err()
            .ok_or("expected vocab metadata extraction to fail without library_vocab entrypoint")?;
        let message = err.to_string();
        assert!(message.contains("library_vocab"), "unexpected error: {message}");
        Ok(())
    }

    #[test]
    fn collect_library_vocab_metadata_extracts_payload_and_projection() -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let project_root = temp.path().join("project");
        fs::create_dir_all(&project_root)?;
        write_vocab_companion_crate(&project_root, "vocab_companion", "widgets_vocab_companion")?;

        let manifest_path = project_root.join("incan.toml");
        fs::write(
            &manifest_path,
            "[project]\nname = \"widgets\"\nversion = \"0.1.0\"\n\n[vocab]\ncrate = \"vocab_companion\"\n",
        )?;
        let manifest = ProjectManifest::from_str(&fs::read_to_string(&manifest_path)?, &manifest_path)?;

        let extraction = collect_library_vocab_metadata(&manifest, &project_root, None, None)?
            .ok_or("expected vocab metadata extraction to return payload")?;
        assert_eq!(extraction.payload.crate_path, "vocab_companion");
        assert_eq!(extraction.payload.package_name, "widgets_vocab_companion");
        assert_eq!(extraction.payload.keyword_registrations.len(), 1);
        assert_eq!(
            extraction.compatibility_activations,
            vec![SoftKeywordActivation {
                namespace: "widgets.dsl".to_string(),
                keyword: "await".to_string(),
            }]
        );
        Ok(())
    }

    #[test]
    fn vocab_companion_fingerprint_changes_when_source_changes() -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let crate_root = write_vocab_companion_crate(temp.path(), "vocab_companion", "widgets_vocab_companion")?;
        let first = vocab_companion_fingerprint(&crate_root, "widgets_vocab_companion")?;
        fs::write(
            crate_root.join("src/lib.rs"),
            "pub fn library_vocab() -> incan_vocab::VocabRegistration {\n    incan_vocab::VocabRegistration::new()\n}\n",
        )?;
        let second = vocab_companion_fingerprint(&crate_root, "widgets_vocab_companion")?;
        assert_ne!(first, second);
        Ok(())
    }

    #[test]
    fn vocab_companion_cache_context_uses_generated_target_cache_base() -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let project_root = temp.path().join("project");
        let generated_target = temp.path().join("generated-target");
        fs::create_dir_all(&project_root)?;
        let crate_root = write_vocab_companion_crate(&project_root, "vocab_companion", "widgets_vocab_companion")?;

        let fingerprint = vocab_companion_fingerprint(&crate_root, "widgets_vocab_companion")?;
        let cache_context = vocab_companion_cache_context(
            &project_root,
            &crate_root,
            "widgets_vocab_companion",
            Some(&generated_target),
        )?;
        assert_eq!(
            cache_context.cache_dir,
            generated_target.join("incan-vocab-cache").join(fingerprint)
        );
        assert!(
            !cache_context.cache_dir.exists(),
            "computing a cache context must not create a Cargo target or cache entry"
        );
        assert!(
            cache_context
                .cache_dir
                .starts_with(generated_target.join("incan-vocab-cache"))
        );
        Ok(())
    }

    #[test]
    fn parse_installed_rust_targets_ignores_empty_lines() {
        let parsed = parse_installed_rust_targets("wasm32-wasip1\n\nx86_64-apple-darwin\n");
        assert!(parsed.contains("wasm32-wasip1"));
        assert!(parsed.contains("x86_64-apple-darwin"));
        assert_eq!(parsed.len(), 2);
    }

    #[test]
    fn ensure_supported_vocab_metadata_version_rejects_newer_version() {
        let metadata = incan_vocab::VocabMetadata {
            metadata_version: incan_vocab::VOCAB_METADATA_VERSION + 1,
            ..incan_vocab::VocabMetadata::default()
        };
        let err = match ensure_supported_vocab_metadata_version(&metadata, Path::new("/tmp/companion")) {
            Ok(()) => panic!("expected metadata version mismatch"),
            Err(err) => err,
        };
        assert!(err.to_string().contains("metadata version"));
    }
}
