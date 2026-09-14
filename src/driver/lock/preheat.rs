//! Preheating a generated library's Cargo dependencies once per fingerprint so the compatibility path does not
//! resolve the same graph for every consumer.
//!
//! Test-only since the direct-rustc path became the normal one: the module rides `cfg(test)` as a whole rather
//! than gating each item, and its callers are the lock tests.
#![allow(
    dead_code,
    reason = "the preheat path is retained for the Cargo-compatibility baker while the direct-rustc path is the normal one"
)]

use std::fs;
use std::path::Path;

use crate::driver::cargo_policy::cargo_command_flags;
use crate::driver::error::{CliError, CliResult};
use std::fs::OpenOptions;
use std::io::{self, Write};
use std::process::Command;
use std::process::Stdio;
use std::thread;
use std::time::Duration;
use std::time::Instant;
use std::time::SystemTime;

use sha2::{Digest, Sha256};

use crate::backend::ProjectGenerator;
use crate::driver::lock::{DependencyPreheatContext, GeneratedLibraryDependencyPreheatRequest};
use crate::driver::lock::{
    LIBRARY_DEPENDENCY_PREHEAT_FINGERPRINT_FILE, LIBRARY_DEPENDENCY_PREHEAT_LOCK_FILE,
    LOCK_DEPENDENCY_PREHEAT_STALE_LOCK_SECS, LockDependencyPreheatGuard,
};
use crate::oven::legacy_cargo::cargo_process::cargo_command;
use crate::oven::legacy_cargo::cargo_process::configure_cargo_target;
use crate::oven::legacy_cargo::cargo_process::sanitize_cargo_environment;

/// Return whether lock-generation dependency preheat should run for the supplied environment value.
fn parse_lock_dependency_preheat_env(raw: Option<&str>) -> bool {
    !matches!(raw.map(str::trim), Some("0" | "false" | "no" | "off"))
}

/// Return whether dependency preheat is enabled for this process.
fn lock_dependency_preheat_enabled() -> bool {
    parse_lock_dependency_preheat_env(std::env::var("INCAN_LOCK_PREHEAT").ok().as_deref())
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
    crate_root_file: &str,
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
    hash_lock_dependency_preheat_file(&mut hasher, lock_dir, &lock_dir.join("src").join(crate_root_file))?;
    Ok(format!("{}{}", fingerprint_file, hex::encode(hasher.finalize())))
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
        "lib.rs",
    )
}

/// Compile the lock workspace dependency graph into the generated-library target/profile domain when stale.
pub(crate) fn run_generated_library_dependency_preheat(
    request: GeneratedLibraryDependencyPreheatRequest<'_>,
) -> CliResult<()> {
    let GeneratedLibraryDependencyPreheatRequest {
        cargo_working_dir,
        lock_dir,
        project_name,
        rust_edition,
        resolved,
        project_requirements,
        cargo_features,
        cargo_policy,
        target_dir,
        cargo_lock_payload,
        cargo_lock_projection_root,
    } = request;
    if !lock_dependency_preheat_enabled() {
        eprintln!("generated library dependency preheat: disabled by INCAN_LOCK_PREHEAT");
        return Ok(());
    }

    let cargo_flags = cargo_command_flags(cargo_policy, cargo_features);
    let preheat_context = DependencyPreheatContext {
        project_name,
        rust_edition: rust_edition.as_deref(),
        resolved,
        project_requirements,
        cargo_policy_flags: &cargo_flags,
    };
    materialize_dependency_preheat_workspace(
        lock_dir,
        &preheat_context,
        cargo_lock_payload,
        cargo_lock_projection_root,
    )?;

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
    let mut command = cargo_command();
    sanitize_cargo_environment(&mut command);
    configure_cargo_target(&mut command, target_dir);
    command.arg("build");
    command.arg("--release");
    command.arg("--manifest-path");
    command.arg(lock_dir.join("Cargo.toml"));
    for flag in &cargo_flags {
        command.arg(flag);
    }
    command.current_dir(cargo_working_dir);

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

/// Materialize the dependency-only generated lock workspace from the current dependency graph and committed lock
/// payload.
fn materialize_dependency_preheat_workspace(
    lock_dir: &Path,
    context: &DependencyPreheatContext<'_>,
    cargo_lock_payload: &str,
    cargo_lock_projection_root: Option<&str>,
) -> CliResult<()> {
    let mut generator = ProjectGenerator::new(lock_dir, context.project_name, false);
    generator.set_dependencies(context.resolved.dependencies.clone());
    generator.set_dev_dependencies(context.resolved.dev_dependencies.clone());
    generator.set_include_dev_dependencies(true);
    generator.set_rust_edition(context.rust_edition.map(ToOwned::to_owned));
    generator.set_stdlib_features(context.project_requirements.stdlib_features.clone());
    generator.set_sdk_dependency_rebindings(context.project_requirements.sdk_dependency_rebindings.clone());
    generator.set_sdk_path_dependencies(context.project_requirements.sdk_path_dependencies.clone());
    generator.set_sdk_artifact_projections(context.project_requirements.sdk_artifact_projections.clone());
    generator.set_cargo_lock_payload(Some(cargo_lock_payload.to_string()));
    generator.set_cargo_lock_projection_root(cargo_lock_projection_root.map(ToOwned::to_owned));
    generator.set_cargo_policy_flags(context.cargo_policy_flags.to_vec());
    generator
        .generate("pub fn __incan_dependency_preheat() {}")
        .map_err(|err| CliError::failure(format!("Failed to generate dependency preheat project: {err}")))?;
    generator.materialize_cargo_lock_projection().map_err(|error| {
        CliError::failure(format!(
            "Failed to project generated dependency preheat Cargo.lock: {error}"
        ))
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
        fs::write(temp_dir.join("src").join("lib.rs"), "pub fn library() {}\n")?;

        let target_dir = temp_dir.join("target").join(".cargo-target");
        let library_preheat = compute_library_dependency_preheat_fingerprint(&temp_dir, &[], &target_dir)?;
        assert!(library_preheat.starts_with(LIBRARY_DEPENDENCY_PREHEAT_FINGERPRINT_FILE));
        fs::write(temp_dir.join("src").join("lib.rs"), "pub fn library_changed() {}\n")?;
        let changed_library_preheat = compute_library_dependency_preheat_fingerprint(&temp_dir, &[], &target_dir)?;
        assert_ne!(
            library_preheat, changed_library_preheat,
            "generated-library preheat fingerprint must track src/lib.rs, not src/main.rs"
        );
        let _ = fs::remove_dir_all(&temp_dir);
        Ok(())
    }
}
