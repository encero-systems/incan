//! Lock file generation and resolution for Incan projects.
//!
//! Handles creating and validating `incan.lock` files that pin dependency versions for reproducible builds.
//! Used by both `incan lock` and the build pipeline.

use std::collections::BTreeSet;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use sha2::{Digest, Sha256};

use crate::backend::ProjectGenerator;
use crate::cli::prelude::ParsedModule;
use crate::cli::{CliError, CliResult, ExitCode};
use crate::dependency_resolver::{InlineRustImport, ResolvedDependencies, resolve_reachable_dependencies};
use crate::frontend::library_manifest_index::LibraryManifestIndex;
use crate::frontend::{diagnostics, lexer, parser};
use crate::lockfile::{CargoFeatureSelection, IncanLock, compute_deps_fingerprint};
use crate::manifest::ProjectManifest;

use super::common::{
    CargoPolicy, ProjectRequirements, build_source_map, cargo_command_flags, cargo_lockfile_flags, collect_modules,
    collect_project_requirements, collect_rust_dependency_uses, enforce_project_toolchain_constraint,
    format_dependency_error, merge_project_requirement_dependencies, merge_project_requirements,
    merge_resolved_dependencies,
};
#[cfg(feature = "rust_inspect")]
use super::common::{collect_rust_inspect_query_paths, ensure_rust_inspect_workspace, prewarm_rust_inspect_workspace};

const LOCK_DEPENDENCY_PREHEAT_FINGERPRINT_FILE: &str = ".incan_dependency_preheat_fingerprint";
const LOCK_DEPENDENCY_PREHEAT_LOCK_FILE: &str = ".incan_dependency_preheat.lock";
const LOCK_DEPENDENCY_PREHEAT_STALE_LOCK_SECS: u64 = 30 * 60;
const LIBRARY_DEPENDENCY_PREHEAT_FINGERPRINT_FILE: &str = ".incan_library_dependency_preheat_fingerprint";
const LIBRARY_DEPENDENCY_PREHEAT_LOCK_FILE: &str = ".incan_library_dependency_preheat.lock";

/// Generate or update incan.lock for a project.
pub fn lock_project(
    entry_file: Option<&PathBuf>,
    cargo_features: Vec<String>,
    cargo_no_default_features: bool,
    cargo_all_features: bool,
) -> CliResult<ExitCode> {
    let start_dir = entry_file
        .and_then(|p| p.parent().map(|p| p.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."));
    let manifest = ProjectManifest::discover(&start_dir)
        .map_err(|e| CliError::failure(e.to_string()))?
        .ok_or_else(|| CliError::failure("No incan.toml found (run `incan init`)"))?;
    enforce_project_toolchain_constraint(&manifest)?;

    let cargo_features = CargoFeatureSelection {
        cargo_features,
        cargo_no_default_features,
        cargo_all_features,
    }
    .normalized();
    let context = collect_project_lock_context(&manifest, entry_file.map(PathBuf::as_path), &cargo_features)?
        .ok_or_else(|| {
            CliError::failure("incan lock requires a FILE argument or at least one [project.scripts] entry")
        })?;

    let project_name = manifest
        .project
        .as_ref()
        .and_then(|p| p.name.clone())
        .or_else(|| {
            context
                .modules
                .first()
                .and_then(|module| module.file_path.file_stem())
                .and_then(|s| s.to_str())
                .map(|s| s.to_string())
        })
        .unwrap_or_else(|| "incan_project".to_string());
    let rust_edition = manifest.build.as_ref().and_then(|b| b.rust_edition.clone());
    let cargo_policy = CargoPolicy::explicit(false, false, false, Vec::new());
    generate_lockfile(
        manifest.project_root(),
        &project_name,
        rust_edition,
        &context.resolved,
        &context.project_requirements,
        &cargo_features,
        &cargo_policy,
        #[cfg(feature = "rust_inspect")]
        &context.rust_inspect_query_paths,
    )?;

    Ok(ExitCode::SUCCESS)
}

/// Resolve the lock payload for a project build.
///
/// Returns `None` if no manifest is present (standalone file compilation).
/// Otherwise, loads the lock file and returns the Cargo.lock payload. Non-strict commands still
/// generate `incan.lock` on first use, but they do not rewrite an existing stale lockfile as a
/// side effect of ordinary build or test verification.
pub(crate) struct LockResolutionRequest<'a> {
    pub project_root: &'a Path,
    pub project_name: &'a str,
    pub manifest: Option<&'a ProjectManifest>,
    pub resolved: &'a ResolvedDependencies,
    pub project_requirements: &'a ProjectRequirements,
    pub cargo_features: &'a CargoFeatureSelection,
    pub cargo_policy: &'a CargoPolicy,
    #[cfg(feature = "rust_inspect")]
    pub rust_inspect_query_paths: &'a [String],
}

/// Resolve the embedded Cargo lock payload that generated Cargo projects should reuse.
///
/// Manifest-less single-file builds have no project lockfile, so they return no payload. Project
/// builds generate `incan.lock` only when it is missing in default mode; stale existing lockfiles
/// are reused with a warning unless `--locked` or `--frozen` requires a hard failure.
pub(crate) fn resolve_lock_payload(request: LockResolutionRequest<'_>) -> CliResult<Option<String>> {
    let LockResolutionRequest {
        project_root,
        project_name,
        manifest,
        resolved,
        project_requirements,
        cargo_features,
        cargo_policy,
        #[cfg(feature = "rust_inspect")]
        rust_inspect_query_paths,
    } = request;

    if manifest.is_none() {
        return Ok(None);
    }

    let project_context = if let Some(manifest) = manifest {
        collect_project_lock_context(manifest, None, cargo_features)?
    } else {
        None
    };
    let lock_inputs = if let Some(context) = project_context.as_ref() {
        Some((
            merge_resolved_dependencies(resolved, &context.resolved)?,
            merge_project_requirements(project_requirements, &context.project_requirements)?,
        ))
    } else {
        None
    };
    let (resolved, project_requirements) = lock_inputs
        .as_ref()
        .map(|(resolved, requirements)| (resolved, requirements))
        .unwrap_or((resolved, project_requirements));
    #[cfg(feature = "rust_inspect")]
    let rust_inspect_query_paths = project_context
        .as_ref()
        .map(|context| context.rust_inspect_query_paths.as_slice())
        .unwrap_or(rust_inspect_query_paths);

    let lock_path = project_root.join("incan.lock");
    let rust_edition = manifest.and_then(|m| m.build.as_ref().and_then(|b| b.rust_edition.clone()));
    let mut resolved_with_requirements = resolved.clone();
    merge_project_requirement_dependencies(&mut resolved_with_requirements, project_requirements)?;
    let fingerprint = compute_deps_fingerprint(
        &resolved_with_requirements.dependencies,
        &resolved_with_requirements.dev_dependencies,
        cargo_features,
        Some(project_root),
    );

    let strict = cargo_policy.locked || cargo_policy.frozen;
    if strict && let Some(message) = strict_git_source_error(&resolved_with_requirements) {
        return Err(CliError::failure(message));
    }
    if lock_path.exists() {
        let lock = IncanLock::load(&lock_path).map_err(|e| CliError::failure(e.to_string()))?;
        if lock.deps_fingerprint != fingerprint {
            if strict {
                return Err(CliError::failure(format!(
                    "incan.lock is out of date\n\n\
                     \x20 expected deps-fingerprint: {fingerprint}\n\
                     \x20   actual deps-fingerprint: {actual}\n\n\
                     This usually means your dependency inputs changed since the lock was generated:\n\n\
                     \x20 - incan.toml dependency entries changed, and/or\n\
                     \x20 - inline rust::... annotations changed, and/or\n\
                     \x20 - toolchain known-good defaults changed (if you rely on defaults)\n\
                     \x20 - Cargo feature selection changed\n\n\
                     Fix:\n\n\
                     \x20   incan lock\n\n\
                     Tip: Pin crate versions/features explicitly in incan.toml for stability \
                     across toolchain upgrades.",
                    actual = lock.deps_fingerprint,
                )));
            }
            eprintln!(
                "warning: incan.lock is out of date; using the existing lock payload without rewriting it. \
                 Run `incan lock` to refresh it."
            );
            return Ok(Some(lock.cargo_lock_payload));
        }
        return Ok(Some(lock.cargo_lock_payload));
    }

    if strict {
        return Err(CliError::failure("incan.lock is missing; run `incan lock`".to_string()));
    }

    let lock = generate_lockfile(
        project_root,
        project_name,
        rust_edition,
        &resolved_with_requirements,
        project_requirements,
        cargo_features,
        cargo_policy,
        #[cfg(feature = "rust_inspect")]
        rust_inspect_query_paths,
    )?;
    Ok(Some(lock.cargo_lock_payload))
}

/// Fully collected dependency inputs that define a manifest project's lock freshness surface.
struct ProjectLockContext {
    modules: Vec<ParsedModule>,
    resolved: ResolvedDependencies,
    project_requirements: ProjectRequirements,
    #[cfg(feature = "rust_inspect")]
    rust_inspect_query_paths: Vec<String>,
}

/// Test-file dependency inputs that must participate in the same project lock fingerprint as normal scripts.
struct TestLockInputs {
    inline_imports: Vec<InlineRustImport>,
    project_requirement_modules: Vec<ParsedModule>,
}

/// Return sorted manifest script entry paths plus an optional explicitly requested entry file.
fn project_lock_entry_paths(manifest: &ProjectManifest, explicit_entry_file: Option<&Path>) -> Vec<PathBuf> {
    let mut paths = BTreeSet::new();
    if let Some(project) = &manifest.project {
        for script in project.scripts.values() {
            paths.insert(manifest.project_root().join(script));
        }
    }
    if let Some(file) = explicit_entry_file {
        paths.insert(file.to_path_buf());
    }
    paths.into_iter().collect()
}

/// Collect the project-wide script and test dependency inputs used for both lock generation and freshness checks.
fn collect_project_lock_context(
    manifest: &ProjectManifest,
    explicit_entry_file: Option<&Path>,
    cargo_features: &CargoFeatureSelection,
) -> CliResult<Option<ProjectLockContext>> {
    let entry_paths = project_lock_entry_paths(manifest, explicit_entry_file);
    if entry_paths.is_empty() {
        return Ok(None);
    }

    let library_manifest_index = LibraryManifestIndex::from_project_manifest(manifest);
    let library_imported_vocab = library_manifest_index.library_imported_vocab();
    let library_imported_dsl_surfaces = library_manifest_index.library_imported_dsl_surfaces();
    let mut modules = Vec::new();
    for entry_path in entry_paths {
        modules.extend(collect_modules(&entry_path.to_string_lossy())?);
    }

    let test_inputs = collect_test_lock_inputs(
        manifest.project_root(),
        Some(&library_imported_vocab),
        Some(&library_imported_dsl_surfaces),
        Some(&library_manifest_index),
    )?;

    let mut project_requirement_modules = modules.clone();
    project_requirement_modules.extend(test_inputs.project_requirement_modules);
    let project_requirements = collect_project_requirements(&project_requirement_modules, &library_manifest_index)?;

    let mut inline_imports = Vec::new();
    for module in &modules {
        inline_imports.extend(collect_rust_dependency_uses(module, false));
    }
    inline_imports.extend(test_inputs.inline_imports);

    let mut resolved =
        resolve_reachable_dependencies(Some(manifest), &inline_imports, true, cargo_features).map_err(|errors| {
            let mut msg = String::new();
            let sources = build_source_map(&project_requirement_modules);
            for err in errors {
                msg.push_str(&format_dependency_error(&err, &sources));
            }
            CliError::failure(msg.trim_end())
        })?;
    merge_project_requirement_dependencies(&mut resolved, &project_requirements)?;
    #[cfg(feature = "rust_inspect")]
    let rust_inspect_query_paths = collect_rust_inspect_query_paths(&project_requirement_modules);

    Ok(Some(ProjectLockContext {
        modules,
        resolved,
        project_requirements,
        #[cfg(feature = "rust_inspect")]
        rust_inspect_query_paths,
    }))
}

struct LockDependencyPreheatGuard {
    path: PathBuf,
}

impl Drop for LockDependencyPreheatGuard {
    /// Remove the cooperative dependency-preheat lock file when the writer exits.
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

/// Return whether lock-generation dependency preheat should run for the supplied environment value.
fn parse_lock_dependency_preheat_env(raw: Option<&str>) -> bool {
    !matches!(raw.map(str::trim), Some("0" | "false" | "no" | "off"))
}

/// Return whether dependency preheat is enabled for this process.
fn lock_dependency_preheat_enabled() -> bool {
    parse_lock_dependency_preheat_env(std::env::var("INCAN_LOCK_PREHEAT").ok().as_deref())
}

/// Return whether the lock-generation inputs are worth compiling ahead of the test harness.
fn should_preheat_lockfile_dependencies(
    resolved: &ResolvedDependencies,
    project_requirements: &ProjectRequirements,
) -> bool {
    !resolved.dependencies.is_empty()
        || !resolved.dev_dependencies.is_empty()
        || !project_requirements.stdlib_features.is_empty()
}

/// Return whether generated tests should use the isolated target-domain override.
fn parse_isolated_test_target_env(raw: Option<&str>) -> bool {
    matches!(raw.map(str::trim), Some("1" | "true" | "yes" | "on"))
}

/// Return the target directory that lock dependency preheat should populate for generated tests.
fn lock_dependency_preheat_target_dir(project_root: &Path) -> PathBuf {
    let absolute_project_root = if project_root.is_absolute() {
        project_root.to_path_buf()
    } else if let Ok(cwd) = std::env::current_dir() {
        cwd.join(project_root)
    } else {
        project_root.to_path_buf()
    };

    if parse_isolated_test_target_env(std::env::var("INCAN_TEST_ISOLATED_TARGET_DIR").ok().as_deref()) {
        absolute_project_root.join("target").join("incan_test_runner")
    } else {
        absolute_project_root.join("target")
    }
}

/// Return the age after which an abandoned dependency-preheat lock may be reclaimed.
fn stale_lock_dependency_preheat_after() -> Duration {
    std::env::var("INCAN_LOCK_PREHEAT_STALE_LOCK_SECS")
        .ok()
        .and_then(|raw| raw.parse::<u64>().ok())
        .map(Duration::from_secs)
        .unwrap_or_else(|| Duration::from_secs(LOCK_DEPENDENCY_PREHEAT_STALE_LOCK_SECS))
}

/// Try to become the single dependency-preheat writer for one lock workspace.
fn try_acquire_lock_dependency_preheat(lock_path: &Path) -> io::Result<Option<LockDependencyPreheatGuard>> {
    if let Some(parent) = lock_path.parent() {
        fs::create_dir_all(parent)?;
    }
    match OpenOptions::new().write(true).create_new(true).open(lock_path) {
        Ok(mut file) => {
            let _ = writeln!(file, "pid={}", std::process::id());
            Ok(Some(LockDependencyPreheatGuard {
                path: lock_path.to_path_buf(),
            }))
        }
        Err(err) if err.kind() == io::ErrorKind::AlreadyExists => Ok(None),
        Err(err) => Err(err),
    }
}

/// Return whether an existing cooperative dependency-preheat lock is old enough to discard.
fn lock_dependency_preheat_is_stale(lock_path: &Path, stale_after: Duration) -> bool {
    let Ok(metadata) = fs::metadata(lock_path) else {
        return false;
    };
    let Ok(modified) = metadata.modified() else {
        return false;
    };
    SystemTime::now()
        .duration_since(modified)
        .is_ok_and(|age| age >= stale_after)
}

/// Return whether the recorded dependency-preheat fingerprint matches the current lock workspace.
fn lock_dependency_preheat_stamp_matches(stamp_path: &Path, fingerprint: &str) -> bool {
    fs::read_to_string(stamp_path)
        .map(|existing| existing.trim() == fingerprint)
        .unwrap_or(false)
}

/// Run a Cargo preheat command with inherited output so long dependency builds remain visible.
fn run_streamed_cargo_preheat(mut command: Command, context: &str) -> CliResult<()> {
    command.stdout(Stdio::inherit());
    command.stderr(Stdio::inherit());
    let status = command
        .status()
        .map_err(|err| CliError::failure(format!("Failed to run {context}: {err}")))?;
    if !status.success() {
        return Err(CliError::failure(format!(
            "{context} failed with status {status}; Cargo output was streamed above"
        )));
    }
    Ok(())
}

/// Add one lock-workspace input file to the dependency-preheat fingerprint.
fn hash_lock_dependency_preheat_file(hasher: &mut Sha256, base: &Path, path: &Path) -> io::Result<()> {
    let relative = path.strip_prefix(base).unwrap_or(path);
    hasher.update(relative.to_string_lossy().as_bytes());
    hasher.update(b"\0");
    hasher.update(fs::read(path)?);
    hasher.update(b"\0");
    Ok(())
}

/// Compute the fingerprint that decides whether a dependency preheat can be reused.
fn compute_dependency_preheat_fingerprint(
    lock_dir: &Path,
    cargo_flags: &[String],
    target_dir: &Path,
    namespace: &[u8],
    command_label: &str,
    fingerprint_file: &str,
) -> io::Result<String> {
    let mut hasher = Sha256::new();
    hasher.update(namespace);
    hasher.update(command_label.as_bytes());
    hasher.update(b"\0");
    hasher.update(target_dir.to_string_lossy().as_bytes());
    hasher.update(b"\0");
    for flag in cargo_flags {
        hasher.update(flag.as_bytes());
        hasher.update(b"\0");
    }
    hash_lock_dependency_preheat_file(&mut hasher, lock_dir, &lock_dir.join("Cargo.toml"))?;
    hash_lock_dependency_preheat_file(&mut hasher, lock_dir, &lock_dir.join("Cargo.lock"))?;
    hash_lock_dependency_preheat_file(&mut hasher, lock_dir, &lock_dir.join("src").join("main.rs"))?;
    Ok(format!("{}{}", fingerprint_file, hex::encode(hasher.finalize())))
}

/// Compute the fingerprint that decides whether lock dependency preheat can be reused.
fn compute_lock_dependency_preheat_fingerprint(
    lock_dir: &Path,
    cargo_flags: &[String],
    target_dir: &Path,
) -> io::Result<String> {
    compute_dependency_preheat_fingerprint(
        lock_dir,
        cargo_flags,
        target_dir,
        b"incan_lock_dependency_preheat/1\0",
        "cargo test --no-run",
        LOCK_DEPENDENCY_PREHEAT_FINGERPRINT_FILE,
    )
}

/// Compute the fingerprint that decides whether generated-library dependency preheat can be reused.
fn compute_library_dependency_preheat_fingerprint(
    lock_dir: &Path,
    cargo_flags: &[String],
    target_dir: &Path,
) -> io::Result<String> {
    compute_dependency_preheat_fingerprint(
        lock_dir,
        cargo_flags,
        target_dir,
        b"incan_library_dependency_preheat/1\0",
        "cargo build --release",
        LIBRARY_DEPENDENCY_PREHEAT_FINGERPRINT_FILE,
    )
}

/// Compile the lock workspace dependency graph into the generated-test target domain when stale.
fn run_lock_dependency_preheat(
    project_root: &Path,
    lock_dir: &Path,
    cargo_features: &CargoFeatureSelection,
    cargo_policy: &CargoPolicy,
) -> CliResult<()> {
    if !lock_dependency_preheat_enabled() {
        return Ok(());
    }

    let cargo_flags = cargo_command_flags(cargo_policy, cargo_features);
    let target_dir = lock_dependency_preheat_target_dir(project_root);
    let fingerprint = compute_lock_dependency_preheat_fingerprint(lock_dir, &cargo_flags, &target_dir)
        .map_err(|err| CliError::failure(format!("Failed to fingerprint lock dependency preheat: {err}")))?;
    let stamp_path = lock_dir.join(LOCK_DEPENDENCY_PREHEAT_FINGERPRINT_FILE);
    if lock_dependency_preheat_stamp_matches(&stamp_path, &fingerprint) {
        return Ok(());
    }

    eprintln!(
        "preheating Cargo dependencies for generated test harnesses into {}",
        target_dir.display()
    );
    let _ = io::stderr().flush();

    let lock_path = lock_dir.join(LOCK_DEPENDENCY_PREHEAT_LOCK_FILE);
    let stale_after = stale_lock_dependency_preheat_after();
    let wait_start = Instant::now();
    let mut announced_wait = false;
    let guard = loop {
        if lock_dependency_preheat_stamp_matches(&stamp_path, &fingerprint) {
            return Ok(());
        }
        match try_acquire_lock_dependency_preheat(&lock_path) {
            Ok(Some(guard)) => break guard,
            Ok(None) => {
                if lock_dependency_preheat_is_stale(&lock_path, stale_after) {
                    let _ = fs::remove_file(&lock_path);
                    continue;
                }
                if !announced_wait && wait_start.elapsed() >= Duration::from_secs(1) {
                    eprintln!("waiting for another incan dependency preheat to finish");
                    let _ = io::stderr().flush();
                    announced_wait = true;
                }
                thread::sleep(Duration::from_millis(100));
            }
            Err(err) => {
                return Err(CliError::failure(format!(
                    "Failed to acquire dependency preheat lock {}: {err}",
                    lock_path.display()
                )));
            }
        }
    };

    if lock_dependency_preheat_stamp_matches(&stamp_path, &fingerprint) {
        drop(guard);
        return Ok(());
    }

    let mut command = Command::new("cargo");
    command.arg("test");
    command.arg("--no-run");
    command.arg("--manifest-path");
    command.arg(lock_dir.join("Cargo.toml"));
    for flag in &cargo_flags {
        command.arg(flag);
    }
    command.env("CARGO_TARGET_DIR", &target_dir);
    command.current_dir(project_root);

    run_streamed_cargo_preheat(command, "cargo test --no-run for dependency preheat")?;

    fs::write(&stamp_path, &fingerprint).map_err(|err| {
        CliError::failure(format!(
            "Failed to write dependency preheat fingerprint {}: {err}",
            stamp_path.display()
        ))
    })?;
    drop(guard);
    Ok(())
}

/// Compile the lock workspace dependency graph into the generated-library target/profile domain when stale.
pub(crate) fn run_generated_library_dependency_preheat(
    project_root: &Path,
    lock_dir: &Path,
    cargo_features: &CargoFeatureSelection,
    cargo_policy: &CargoPolicy,
    target_dir: &Path,
) -> CliResult<()> {
    if !lock_dependency_preheat_enabled() {
        eprintln!("generated library dependency preheat: disabled by INCAN_LOCK_PREHEAT");
        return Ok(());
    }

    let cargo_flags = cargo_command_flags(cargo_policy, cargo_features);
    let fingerprint =
        compute_library_dependency_preheat_fingerprint(lock_dir, &cargo_flags, target_dir).map_err(|err| {
            CliError::failure(format!(
                "Failed to fingerprint generated library dependency preheat: {err}"
            ))
        })?;
    let stamp_path = lock_dir.join(LIBRARY_DEPENDENCY_PREHEAT_FINGERPRINT_FILE);
    if lock_dependency_preheat_stamp_matches(&stamp_path, &fingerprint) {
        eprintln!(
            "generated library dependency preheat: up-to-date (target {}, profile release)",
            target_dir.display()
        );
        return Ok(());
    }

    eprintln!(
        "preheating Cargo dependencies for generated library builds into {} (profile release)",
        target_dir.display()
    );
    let _ = io::stderr().flush();

    let lock_path = lock_dir.join(LIBRARY_DEPENDENCY_PREHEAT_LOCK_FILE);
    let stale_after = stale_lock_dependency_preheat_after();
    let wait_start = Instant::now();
    let mut announced_wait = false;
    let guard = loop {
        if lock_dependency_preheat_stamp_matches(&stamp_path, &fingerprint) {
            eprintln!(
                "generated library dependency preheat: reused after waiting {:.2}s",
                wait_start.elapsed().as_secs_f64()
            );
            return Ok(());
        }
        match try_acquire_lock_dependency_preheat(&lock_path) {
            Ok(Some(guard)) => break guard,
            Ok(None) => {
                if lock_dependency_preheat_is_stale(&lock_path, stale_after) {
                    let _ = fs::remove_file(&lock_path);
                    continue;
                }
                if !announced_wait && wait_start.elapsed() >= Duration::from_secs(1) {
                    eprintln!("waiting for another generated library dependency preheat to finish");
                    let _ = io::stderr().flush();
                    announced_wait = true;
                }
                thread::sleep(Duration::from_millis(100));
            }
            Err(err) => {
                return Err(CliError::failure(format!(
                    "Failed to acquire generated library dependency preheat lock {}: {err}",
                    lock_path.display()
                )));
            }
        }
    };

    if lock_dependency_preheat_stamp_matches(&stamp_path, &fingerprint) {
        drop(guard);
        eprintln!("generated library dependency preheat: up-to-date after lock acquisition");
        return Ok(());
    }

    let start = Instant::now();
    let mut command = Command::new("cargo");
    command.arg("build");
    command.arg("--release");
    command.arg("--manifest-path");
    command.arg(lock_dir.join("Cargo.toml"));
    for flag in &cargo_flags {
        command.arg(flag);
    }
    command.env("CARGO_TARGET_DIR", target_dir);
    command.current_dir(project_root);

    run_streamed_cargo_preheat(
        command,
        "cargo build --release for generated library dependency preheat",
    )?;

    fs::write(&stamp_path, &fingerprint).map_err(|err| {
        CliError::failure(format!(
            "Failed to write generated library dependency preheat fingerprint {}: {err}",
            stamp_path.display()
        ))
    })?;
    drop(guard);
    eprintln!(
        "generated library dependency preheat: ran in {:.2}s",
        start.elapsed().as_secs_f64()
    );
    Ok(())
}

/// Prewarm rust-inspect metadata into the lock workspace cache when lock generation knows the query set.
#[cfg(feature = "rust_inspect")]
fn run_lock_rust_inspect_prewarm(
    project_root: &Path,
    project_name: &str,
    rust_edition: Option<String>,
    resolved: &ResolvedDependencies,
    project_requirements: &ProjectRequirements,
    lock: &IncanLock,
    query_paths: &[String],
) -> CliResult<()> {
    if query_paths.is_empty() {
        return Ok(());
    }

    let rust_inspect_manifest_dir = ensure_rust_inspect_workspace(
        project_root,
        project_name,
        rust_edition,
        resolved,
        project_requirements,
        Some(lock.cargo_lock_payload.clone()),
    )?;
    prewarm_rust_inspect_workspace(&rust_inspect_manifest_dir, query_paths)
}

/// Generate an `incan.lock` file by creating a temporary Cargo project and resolving dependencies.
#[allow(clippy::too_many_arguments)]
pub(crate) fn generate_lockfile(
    project_root: &Path,
    project_name: &str,
    rust_edition: Option<String>,
    resolved: &ResolvedDependencies,
    project_requirements: &ProjectRequirements,
    cargo_features: &CargoFeatureSelection,
    cargo_policy: &CargoPolicy,
    #[cfg(feature = "rust_inspect")] rust_inspect_query_paths: &[String],
) -> CliResult<IncanLock> {
    let lock_dir = project_root.join("target").join("incan_lock");
    let mut generator = ProjectGenerator::new(&lock_dir, project_name, true);
    #[cfg(feature = "rust_inspect")]
    let rust_edition_for_prewarm = rust_edition.clone();
    generator.set_dependencies(resolved.dependencies.clone());
    generator.set_dev_dependencies(resolved.dev_dependencies.clone());
    generator.set_include_dev_dependencies(true);
    generator.set_rust_edition(rust_edition);
    generator.set_stdlib_features(project_requirements.stdlib_features.clone());

    let rust_code = "fn main() {}";
    generator
        .generate(rust_code)
        .map_err(|e| CliError::failure(format!("Failed to generate lock project: {}", e)))?;

    let mut command = Command::new("cargo");
    command.arg("generate-lockfile");
    for flag in cargo_lockfile_flags(cargo_policy, cargo_features) {
        command.arg(flag);
    }
    let status = command
        .current_dir(&lock_dir)
        .output()
        .map_err(|e| CliError::failure(format!("Failed to run cargo generate-lockfile: {}", e)))?;

    if !status.status.success() {
        let stderr = String::from_utf8_lossy(&status.stderr);
        return Err(CliError::failure(format!(
            "cargo generate-lockfile failed:\n{}",
            stderr
        )));
    }

    let cargo_lock = fs::read_to_string(lock_dir.join("Cargo.lock"))
        .map_err(|e| CliError::failure(format!("Failed to read Cargo.lock: {}", e)))?;
    let fingerprint = compute_deps_fingerprint(
        &resolved.dependencies,
        &resolved.dev_dependencies,
        cargo_features,
        Some(project_root),
    );
    let lock = IncanLock::new(fingerprint, cargo_features.clone(), cargo_lock);

    if should_preheat_lockfile_dependencies(resolved, project_requirements) {
        run_lock_dependency_preheat(project_root, &lock_dir, cargo_features, cargo_policy)?;
    }
    #[cfg(feature = "rust_inspect")]
    run_lock_rust_inspect_prewarm(
        project_root,
        project_name,
        rust_edition_for_prewarm,
        resolved,
        project_requirements,
        &lock,
        rust_inspect_query_paths,
    )?;

    let lock_path = project_root.join("incan.lock");
    lock.write(&lock_path)
        .map_err(|e| CliError::failure(format!("Failed to write incan.lock: {}", e)))?;

    Ok(lock)
}

/// Collect inline Rust crate imports and stdlib/provider requirements from test files for lock resolution.
fn collect_test_lock_inputs(
    project_root: &Path,
    library_imported_vocab: Option<&parser::ImportedLibraryVocab>,
    library_imported_dsl_surfaces: Option<&parser::ImportedLibraryDslSurfaces>,
    library_manifest_index: Option<&LibraryManifestIndex>,
) -> CliResult<TestLockInputs> {
    let mut inline_imports = Vec::new();
    let mut project_requirement_modules = Vec::new();
    let test_files = crate::cli::test_runner::discover_test_files(project_root);
    let source_root = project_root.join("src");

    for file_path in test_files {
        let source = fs::read_to_string(&file_path)
            .map_err(|e| CliError::failure(format!("Failed to read test file '{}': {}", file_path.display(), e)))?;
        let tokens = lexer::lex(&source).map_err(|errs| {
            let mut msg = String::new();
            for err in &errs {
                msg.push_str(&diagnostics::format_error(&file_path.to_string_lossy(), &source, err));
            }
            CliError::failure(msg.trim_end())
        })?;
        let path_display = file_path.to_string_lossy();
        let ast = parser::parse_with_context_and_surfaces(
            &tokens,
            Some(path_display.as_ref()),
            library_imported_vocab,
            library_imported_dsl_surfaces,
        )
        .map_err(|errs| {
            let mut msg = String::new();
            for err in &errs {
                msg.push_str(&diagnostics::format_error(&file_path.to_string_lossy(), &source, err));
            }
            CliError::failure(msg.trim_end())
        })?;

        let test_module = ParsedModule {
            name: file_path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .unwrap_or("test")
                .to_string(),
            path_segments: vec!["test".to_string()],
            file_path: file_path.clone(),
            source: source.clone(),
            ast: ast.clone(),
        };
        inline_imports.extend(collect_rust_dependency_uses(&test_module, true));
        project_requirement_modules.push(test_module);

        let source_modules = crate::cli::test_runner::collect_source_modules_for_test(
            &ast,
            &source_root,
            library_imported_vocab,
            library_imported_dsl_surfaces,
            library_manifest_index,
        )
        .map_err(CliError::failure)?;
        for module in &source_modules {
            inline_imports.extend(collect_rust_dependency_uses(module, false));
        }
        project_requirement_modules.extend(source_modules);
    }

    Ok(TestLockInputs {
        inline_imports,
        project_requirement_modules,
    })
}

/// Check whether any resolved dependency uses a git branch source, which is forbidden in strict
/// (`--locked` / `--frozen`) mode.
fn strict_git_source_error(resolved: &ResolvedDependencies) -> Option<String> {
    for spec in resolved.dependencies.iter().chain(resolved.dev_dependencies.iter()) {
        if let crate::manifest::DependencySource::Git { reference, .. } = &spec.source
            && matches!(reference, crate::manifest::GitReference::Branch(_))
        {
            return Some(format!(
                "strict mode forbids git branch dependencies (crate `{}`); use tag or rev",
                spec.crate_name
            ));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::{DependencySource, DependencySpec};

    fn empty_resolved() -> ResolvedDependencies {
        ResolvedDependencies {
            dependencies: Vec::new(),
            dev_dependencies: Vec::new(),
        }
    }

    fn empty_project_requirements() -> ProjectRequirements {
        ProjectRequirements {
            stdlib_features: Vec::new(),
            dependencies: Vec::new(),
        }
    }

    fn registry_dependency(crate_name: &str) -> DependencySpec {
        DependencySpec {
            crate_name: crate_name.to_string(),
            version: Some("1".to_string()),
            features: Vec::new(),
            default_features: true,
            source: DependencySource::Registry,
            optional: false,
            package: None,
        }
    }

    #[test]
    fn parse_lock_dependency_preheat_env_defaults_to_enabled() {
        assert!(parse_lock_dependency_preheat_env(None));
        assert!(parse_lock_dependency_preheat_env(Some("1")));
        assert!(parse_lock_dependency_preheat_env(Some("true")));
        assert!(!parse_lock_dependency_preheat_env(Some("0")));
        assert!(!parse_lock_dependency_preheat_env(Some("false")));
        assert!(!parse_lock_dependency_preheat_env(Some(" off ")));
    }

    #[test]
    fn lock_dependency_preheat_is_skipped_without_dependency_inputs() {
        assert!(!should_preheat_lockfile_dependencies(
            &empty_resolved(),
            &empty_project_requirements()
        ));

        let mut resolved = empty_resolved();
        resolved.dependencies.push(registry_dependency("serde"));
        assert!(should_preheat_lockfile_dependencies(
            &resolved,
            &empty_project_requirements()
        ));

        let mut requirements = empty_project_requirements();
        requirements.stdlib_features.push("json".to_string());
        assert!(should_preheat_lockfile_dependencies(&empty_resolved(), &requirements));
    }

    #[test]
    fn lock_collects_test_imported_source_modules_as_normal_deps() -> Result<(), Box<dyn std::error::Error>> {
        let temp_dir = tempfile::tempdir()?;
        let project_root = temp_dir.path();
        fs::create_dir_all(project_root.join("src"))?;
        fs::create_dir_all(project_root.join("tests"))?;
        fs::write(
            project_root.join("src").join("internal.incn"),
            "from rust::datafusion @ \"53\" import SessionContext\n",
        )?;
        fs::write(
            project_root.join("tests").join("test_internal.incn"),
            "from internal import SessionContext\nfrom rust::tokio @ \"1\" import spawn\n",
        )?;

        let inputs = collect_test_lock_inputs(project_root, None, None, None)?;
        let imports = inputs.inline_imports;
        let tokio = imports
            .iter()
            .find(|import| import.crate_name == "tokio")
            .ok_or("expected direct test tokio import")?;
        let datafusion = imports
            .iter()
            .find(|import| import.crate_name == "datafusion")
            .ok_or("expected test-imported source module datafusion import")?;

        assert!(tokio.is_test_context);
        assert!(!datafusion.is_test_context);
        Ok(())
    }

    #[test]
    fn lock_dependency_preheat_fingerprint_changes_when_cargo_lock_changes() -> Result<(), Box<dyn std::error::Error>> {
        let temp_dir = std::env::temp_dir().join(format!("incan_lock_preheat_fingerprint_{}", std::process::id()));
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(temp_dir.join("src"))?;
        fs::write(
            temp_dir.join("Cargo.toml"),
            "[package]\nname = \"lock_preheat\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )?;
        fs::write(
            temp_dir.join("Cargo.lock"),
            "# This file is automatically @generated by Cargo.\nversion = 4\n",
        )?;
        fs::write(temp_dir.join("src").join("main.rs"), "fn main() {}\n")?;

        let target_dir = temp_dir.join("target");
        let first = compute_lock_dependency_preheat_fingerprint(&temp_dir, &[], &target_dir)?;
        fs::write(
            temp_dir.join("Cargo.lock"),
            "# This file is automatically @generated by Cargo.\nversion = 4\n\n[[package]]\nname = \"serde\"\nversion = \"1.0.0\"\n",
        )?;
        let second = compute_lock_dependency_preheat_fingerprint(&temp_dir, &[], &target_dir)?;

        assert_ne!(first, second);
        let _ = fs::remove_dir_all(&temp_dir);
        Ok(())
    }

    #[test]
    fn library_dependency_preheat_fingerprint_uses_separate_profile_domain() -> Result<(), Box<dyn std::error::Error>> {
        let temp_dir = std::env::temp_dir().join(format!("incan_library_preheat_fingerprint_{}", std::process::id()));
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(temp_dir.join("src"))?;
        fs::write(
            temp_dir.join("Cargo.toml"),
            "[package]\nname = \"library_preheat\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )?;
        fs::write(
            temp_dir.join("Cargo.lock"),
            "# This file is automatically @generated by Cargo.\nversion = 4\n",
        )?;
        fs::write(temp_dir.join("src").join("main.rs"), "fn main() {}\n")?;

        let target_dir = temp_dir.join("target").join(".cargo-target");
        let test_preheat = compute_lock_dependency_preheat_fingerprint(&temp_dir, &[], &target_dir)?;
        let library_preheat = compute_library_dependency_preheat_fingerprint(&temp_dir, &[], &target_dir)?;

        assert_ne!(
            test_preheat, library_preheat,
            "test-harness and generated-library preheats must not share stale stamps"
        );
        assert!(library_preheat.starts_with(LIBRARY_DEPENDENCY_PREHEAT_FINGERPRINT_FILE));
        let _ = fs::remove_dir_all(&temp_dir);
        Ok(())
    }
}
