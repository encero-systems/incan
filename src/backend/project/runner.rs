//! Cargo-lock projection support for the explicit publisher boundary.
//!
//! Normal Incan execution is receipt-bound Oven direct-`rustc`. Historical generated-project Cargo execution remains
//! compiled only for isolated unit fixtures; it is not an available product backend.

use std::collections::BTreeSet;
use std::env;
use std::ffi::OsString;
use std::fs;
#[cfg(test)]
use std::fs::{File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
#[cfg(test)]
use std::process::Stdio;

use super::generator::ProjectGenerator;
#[cfg(test)]
use super::generator::{RunProfile, cargo_config_identity};
#[cfg(test)]
use sha2::{Digest, Sha256};

const CARGO_MANIFEST_FILENAME: &str = "Cargo.toml";

/// Network policy for Cargo-owned lock projection. `--locked` constrains mutation but does not imply offline mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CargoLockProjectionNetwork {
    Online,
    Offline,
}

impl CargoLockProjectionNetwork {
    /// Derive projection network access from the caller's Cargo policy flags.
    fn from_cargo_flags(flags: &[String]) -> Self {
        if flags.iter().any(|flag| flag == "--offline" || flag == "--frozen") {
            Self::Offline
        } else {
            Self::Online
        }
    }

    /// Apply an offline projection policy to a Cargo command when required.
    fn apply(self, command: &mut Command) {
        if self == Self::Offline {
            command.arg("--offline");
        }
    }
}

/// Remove process-environment entries that Cargo and Rust build scripts cannot represent as Unicode.
///
/// Incan programs may intentionally inspect a non-Unicode environment value through `std.environ`. Cargo's build
/// script support, however, exposes the environment through Unicode-only APIs and can panic before compilation when
/// such a value is inherited. Generated binaries retain their original environment; only compiler-owned Cargo child
/// processes receive this sanitized view.
pub(crate) fn sanitize_cargo_environment(command: &mut Command) {
    for (key, value) in env::vars_os() {
        if key.to_str().is_none() || value.to_str().is_none() {
            command.env_remove(key);
        }
    }
}

/// Resolve the Cargo executable selected for compiler-owned generated-project commands.
pub(crate) fn cargo_executable() -> OsString {
    env::var_os("CARGO")
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "cargo".into())
}

/// Create one compiler-owned Cargo command from the canonical executable selection.
pub(crate) fn cargo_command() -> Command {
    Command::new(cargo_executable())
}

/// Keep Cargo's target and unstable build-directory outputs inside the lifecycle-owned target root.
pub(crate) fn configure_cargo_target(command: &mut Command, target_dir: &Path) {
    command.env("CARGO_TARGET_DIR", target_dir);
    command.env("CARGO_BUILD_BUILD_DIR", target_dir);
}

impl ProjectGenerator {
    /// Ask Cargo to project a fresh canonical seed onto this caller-local manifest and prove two-pass convergence.
    pub(crate) fn materialize_cargo_lock_projection(&self) -> io::Result<bool> {
        let Some(projection) = self.cargo_lock_projection()? else {
            return Ok(false);
        };
        let lock_path = self.output_dir.join("Cargo.lock");
        let seed = fs::read_to_string(&lock_path)?;
        run_cargo_lock_projection(
            &self.output_dir,
            &self.cargo_target_dir(),
            &projection,
            self.cargo_package_name(),
            self.cargo_package_version(),
            CargoLockProjectionNetwork::from_cargo_flags(&self.cargo_policy_flags),
            None,
        )?;
        let first = fs::read_to_string(&lock_path)?;
        projection.validate_projected(&first, self.cargo_package_name(), self.cargo_package_version())?;

        fs::write(&lock_path, projection.seed_payload())?;
        run_cargo_lock_projection(
            &self.output_dir,
            &self.cargo_target_dir(),
            &projection,
            self.cargo_package_name(),
            self.cargo_package_version(),
            CargoLockProjectionNetwork::from_cargo_flags(&self.cargo_policy_flags),
            None,
        )?;
        let second = fs::read_to_string(&lock_path)?;
        projection.validate_convergence(&first, &second)?;
        Ok(seed != first)
    }

    /// Whether an isolated Cargo-runner fixture must compile before executing its generated binary.
    #[cfg(test)]
    fn should_build_before_run(&self, project_changed: bool) -> bool {
        project_changed || !self.run_binary_path().is_file() || !self.run_publication_fingerprint_matches()
    }

    /// Return extra Cargo CLI args required by an isolated runner fixture.
    #[cfg(test)]
    fn run_profile_build_args(&self) -> &'static [&'static str] {
        match self.run_profile {
            RunProfile::Debug => &[],
            RunProfile::Release => &["--release"],
        }
    }

    /// Return the Cargo target subdirectory used by an isolated runner fixture.
    #[cfg(test)]
    fn run_profile_binary_dir(&self) -> &'static str {
        match self.run_profile {
            RunProfile::Debug => "debug",
            RunProfile::Release => "release",
        }
    }

    /// Return a human-readable label for an isolated runner fixture profile.
    #[cfg(test)]
    fn run_profile_label(&self) -> &'static str {
        match self.run_profile {
            RunProfile::Debug => "debug",
            RunProfile::Release => "release",
        }
    }

    /// Return the selected Cargo target for this generated project.
    ///
    /// CLI build/test/library paths set an explicit managed or caller-owned target before Cargo runs. The parent-scoped
    /// fallback remains only for internal or non-CLI generators that do not install that policy.
    pub(crate) fn cargo_target_dir(&self) -> PathBuf {
        if let Some(target_dir) = self.cargo_target_dir_override() {
            return target_dir;
        }

        let base_dir = self.output_dir.parent().unwrap_or(self.output_dir.as_path());
        let target_dir = base_dir.join(".cargo-target");

        Self::resolve_target_dir(target_dir)
    }

    /// Build an isolated generated-project fixture using Cargo.
    #[cfg(test)]
    pub fn build(&self) -> io::Result<BuildResult> {
        self.materialize_cargo_lock_projection()?;
        let _root_artifact_guard = self.acquire_root_artifact_lock()?;
        let cargo_target_dir = self.cargo_target_dir();
        let mut command = cargo_command();
        sanitize_cargo_environment(&mut command);
        configure_cargo_target(&mut command, &cargo_target_dir);
        command
            .arg("build")
            .arg("--release")
            .arg("--message-format=json-render-diagnostics");
        for flag in &self.cargo_policy_flags {
            command.arg(flag);
        }
        let output = command
            // Ensure we don't inherit a broken CA bundle path from the parent env.
            .env_remove("SSL_CERT_FILE")
            .env_remove("SSL_CERT_DIR")
            .env_remove("CURL_CA_BUNDLE")
            .env_remove("REQUESTS_CA_BUNDLE")
            .env_remove("CARGO_HTTP_CAINFO")
            .current_dir(&self.output_dir)
            .output()?;

        let cargo_messages = parse_cargo_json_build_output(&output.stdout, &self.cargo_target_name());
        let mut stderr = String::from_utf8_lossy(&output.stderr).to_string();
        stderr.push_str(&cargo_messages.rendered);
        let result = BuildResult {
            success: output.status.success(),
            stdout: String::new(),
            stderr,
        };
        if result.success && self.is_binary {
            let executable = cargo_messages.executable.ok_or_else(|| {
                io::Error::other(format!(
                    "Cargo reported a successful build without an executable artifact for target `{}`",
                    self.cargo_target_name()
                ))
            })?;
            self.publish_cargo_binary(&executable, &self.binary_path())?;
        }
        self.finish_generated_cache_lease()?;
        Ok(result)
    }

    /// Run an isolated generated-project fixture using Cargo.
    ///
    /// Uses inherited stdio so fixture output streams to the terminal in real-time.
    ///
    /// This test-only helper preserves historical generated-project coverage. `incan run` uses Oven direct-rustc and
    /// never calls this method in a production compiler build.
    #[cfg(test)]
    pub fn run(&self) -> io::Result<RunResult> {
        self.run_with_cwd(&self.output_dir, true)
    }

    /// Run an isolated generated-project fixture with a custom working directory.
    ///
    /// This builds the generated Rust project, then runs the resulting binary with `cwd` as the working directory.
    /// This keeps runtime-relative paths anchored to the original project root rather than the generated
    /// `target/incan/...` directory.
    ///
    /// Cargo build output is streamed directly to the terminal so incremental compilation progress remains visible on
    /// slow first runs and long rebuilds.
    #[cfg(test)]
    pub fn run_with_cwd(&self, cwd: &Path, project_changed: bool) -> io::Result<RunResult> {
        // Generation validates and preserves an existing projection before reporting `project_changed = false`.
        // The isolated fixture avoids a redundant two-pass projection in that fast path.
        let project_changed = if project_changed {
            self.materialize_cargo_lock_projection()?;
            true
        } else {
            false
        };
        let root_artifact_guard = self.acquire_root_artifact_lock()?;
        if self.should_build_before_run(project_changed) {
            // ---- Context: build generated crate with selected run profile ----
            let cargo_target_dir = self.cargo_target_dir();
            eprintln!(
                "Building generated project with cargo ({}) profile...",
                self.run_profile_label()
            );
            let mut build_command = cargo_command();
            sanitize_cargo_environment(&mut build_command);
            configure_cargo_target(&mut build_command, &cargo_target_dir);
            build_command.arg("build");
            build_command.arg("--message-format=json-render-diagnostics");
            for arg in self.run_profile_build_args() {
                build_command.arg(arg);
            }
            for flag in &self.cargo_policy_flags {
                build_command.arg(flag);
            }
            let build_output = build_command
                // Ensure we don't inherit a broken CA bundle path from the parent env.
                .env_remove("SSL_CERT_FILE")
                .env_remove("SSL_CERT_DIR")
                .env_remove("CURL_CA_BUNDLE")
                .env_remove("REQUESTS_CA_BUNDLE")
                .env_remove("CARGO_HTTP_CAINFO")
                .current_dir(&self.output_dir)
                .stdout(Stdio::piped())
                .stderr(Stdio::inherit())
                .output()?;
            let cargo_messages = parse_cargo_json_build_output(&build_output.stdout, &self.cargo_target_name());
            if !cargo_messages.rendered.is_empty() {
                eprint!("{}", cargo_messages.rendered);
            }
            if !build_output.status.success() {
                self.finish_generated_cache_lease()?;
                return Ok(RunResult {
                    success: false,
                    stdout: String::new(),
                    stderr: cargo_messages.rendered,
                    exit_code: build_output.status.code(),
                });
            }
            let executable = cargo_messages.executable.ok_or_else(|| {
                io::Error::other(format!(
                    "Cargo reported a successful build without an executable artifact for target `{}`",
                    self.cargo_target_name()
                ))
            })?;
            self.publish_cargo_binary(&executable, &self.run_binary_path())?;
            self.write_run_publication_fingerprint()?;
        } else {
            eprintln!(
                "Generated project unchanged; reusing existing cargo ({}) binary.",
                self.run_profile_label()
            );
        }

        // Cargo has completed and its root artifact is now project-local. Holding a managed-cache lease while the
        // generated program runs would make a long-lived server unevictable even though it no longer uses Cargo.
        self.finish_generated_cache_lease()?;

        // The project-local publication is isolated from later compatible builds that reuse the shared Cargo target.
        drop(root_artifact_guard);

        // ---- Context: execute built binary with caller-provided cwd ----
        eprintln!("Build finished. Running generated binary...");
        // The default generated output can be relative to the invocation directory while `cwd` is the discovered
        // project root. Resolve the published artifact before changing directory or the child would look for the
        // relative `target/incan/...` path beneath the project root instead of beneath the invocation directory.
        let run_binary = fs::canonicalize(self.run_binary_path())?;
        let mut child = Command::new(run_binary)
            .current_dir(cwd)
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .spawn()?;

        let status = child.wait()?;

        Ok(RunResult {
            success: status.success(),
            stdout: String::new(), // Output went directly to terminal
            stderr: String::new(),
            exit_code: status.code(),
        })
    }

    /// Atomically copy one fixture Cargo root artifact into project-local generated output.
    #[cfg(test)]
    fn publish_cargo_binary(&self, source: &Path, destination: &Path) -> io::Result<()> {
        if source == destination {
            return Ok(());
        }
        let parent = destination
            .parent()
            .ok_or_else(|| io::Error::other(format!("binary output has no parent: {}", destination.display())))?;
        fs::create_dir_all(parent)?;
        let staged = parent.join(format!(".incan-binary-{}.tmp", std::process::id()));
        if let Err(error) = fs::copy(source, &staged) {
            let _ = fs::remove_file(&staged);
            return Err(error);
        }
        if let Err(error) = fs::rename(&staged, destination) {
            let _ = fs::remove_file(&staged);
            return Err(error);
        }
        Ok(())
    }

    /// Serialize fixture Cargo root-artifact production and publication for one deterministic target name.
    #[cfg(test)]
    fn acquire_root_artifact_lock(&self) -> io::Result<File> {
        let lock_dir = self.cargo_target_dir().join(".incan-root-locks");
        fs::create_dir_all(&lock_dir)?;
        let lock = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(lock_dir.join(format!("{}.lock", self.cargo_target_name())))?;
        lock.lock()?;
        Ok(lock)
    }

    /// Identity of the exact fixture Cargo publication used by the local run binary.
    #[cfg(test)]
    fn run_publication_fingerprint(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(b"incan-run-publication-v1\0");
        hasher.update(self.generated_cache_identity().unwrap_or_default().as_bytes());
        hasher.update(b"\0target-dir\0");
        hasher.update(self.cargo_target_dir().as_os_str().as_encoded_bytes());
        hasher.update(b"\0root\0");
        hasher.update(self.cargo_target_name().as_bytes());
        hasher.update(b"\0profile\0");
        hasher.update(self.run_profile_label().as_bytes());
        hasher.update(b"\0policy\0");
        for flag in &self.cargo_policy_flags {
            hasher.update(flag.as_bytes());
            hasher.update(b"\0");
        }
        hasher.update(b"cargo-config=");
        hasher.update(cargo_config_identity(&self.output_dir).as_bytes());
        hasher.update(b"\0");
        hasher.update(b"rust-backend=");
        match crate::generated_cache::rust_backend_identity(&self.output_dir) {
            Ok(identity) => hasher.update(identity.as_bytes()),
            Err(error) => hasher.update(format!("unavailable:{error}").as_bytes()),
        }
        hasher.update(b"\0");
        for selector in [
            "RUSTC",
            "RUSTC_WRAPPER",
            "RUSTC_WORKSPACE_WRAPPER",
            "RUSTUP_TOOLCHAIN",
            "CARGO_BUILD_TARGET",
            "RUSTFLAGS",
            "CARGO_ENCODED_RUSTFLAGS",
        ] {
            hasher.update(selector.as_bytes());
            hasher.update(b"=");
            let value = std::env::var_os(selector).unwrap_or_default();
            hasher.update(value.as_os_str().as_encoded_bytes());
            hasher.update(b"\0");
        }
        hex::encode(hasher.finalize())
    }

    /// Sidecar that proves a fixture run binary still matches the selected managed domain.
    #[cfg(test)]
    fn run_publication_fingerprint_path(&self) -> PathBuf {
        self.run_binary_path()
            .parent()
            .unwrap_or(self.output_dir.as_path())
            .join(format!(".{}.incan-run-fingerprint", self.name))
    }

    /// Return whether the fixture run binary still belongs to the selected generated-build domain.
    #[cfg(test)]
    fn run_publication_fingerprint_matches(&self) -> bool {
        fs::read_to_string(self.run_publication_fingerprint_path())
            .is_ok_and(|fingerprint| fingerprint.trim() == self.run_publication_fingerprint())
    }

    /// Atomically record the fixture Cargo domain that supplied the published run binary.
    #[cfg(test)]
    fn write_run_publication_fingerprint(&self) -> io::Result<()> {
        let path = self.run_publication_fingerprint_path();
        let parent = path
            .parent()
            .ok_or_else(|| io::Error::other(format!("run fingerprint has no parent: {}", path.display())))?;
        fs::create_dir_all(parent)?;
        let staged = parent.join(format!(".incan-run-fingerprint-{}.tmp", std::process::id()));
        fs::write(&staged, self.run_publication_fingerprint())?;
        if let Err(error) = fs::rename(&staged, &path) {
            let _ = fs::remove_file(&staged);
            return Err(error);
        }
        Ok(())
    }

    /// Get the project-local path to an isolated fixture build artifact.
    #[cfg(test)]
    pub fn binary_path(&self) -> PathBuf {
        self.output_dir.join("target").join("release").join(&self.name)
    }

    /// Get the path to the binary produced by an isolated runner fixture.
    #[cfg(test)]
    pub fn run_binary_path(&self) -> PathBuf {
        self.output_dir
            .join("target")
            .join(self.run_profile_binary_dir())
            .join(&self.name)
    }
}

/// Executable and rendered diagnostics selected from a fixture Cargo JSON message stream.
#[cfg(test)]
#[derive(Default)]
struct CargoJsonBuildOutput {
    executable: Option<PathBuf>,
    rendered: String,
}

/// Parse fixture Cargo artifact locations instead of reconstructing profile/target-triple paths.
#[cfg(test)]
fn parse_cargo_json_build_output(stdout: &[u8], expected_target_name: &str) -> CargoJsonBuildOutput {
    let mut parsed = CargoJsonBuildOutput::default();
    for line in stdout.split(|byte| *byte == b'\n').filter(|line| !line.is_empty()) {
        let Ok(message) = serde_json::from_slice::<serde_json::Value>(line) else {
            parsed.rendered.push_str(&String::from_utf8_lossy(line));
            parsed.rendered.push('\n');
            continue;
        };
        match message.get("reason").and_then(serde_json::Value::as_str) {
            Some("compiler-artifact")
                if message.pointer("/target/name").and_then(serde_json::Value::as_str)
                    == Some(expected_target_name) =>
            {
                if let Some(executable) = message.get("executable").and_then(serde_json::Value::as_str) {
                    parsed.executable = Some(PathBuf::from(executable));
                }
            }
            Some("compiler-message") => {
                if let Some(rendered) = message.pointer("/message/rendered").and_then(serde_json::Value::as_str) {
                    parsed.rendered.push_str(rendered);
                }
            }
            _ => {}
        }
    }
    parsed
}

/// Run one Cargo-owned projection pass against an already rendered generated manifest.
fn run_cargo_lock_projection(
    output_dir: &Path,
    target_dir: &Path,
    projection: &super::lock_projection::CargoLockProjection,
    generated_package_name: &str,
    generated_package_version: &str,
    network: CargoLockProjectionNetwork,
    cargo_home: Option<&Path>,
) -> io::Result<()> {
    let cargo = env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let mut command = Command::new(&cargo);
    sanitize_cargo_environment(&mut command);
    configure_cargo_target(&mut command, target_dir);
    if let Some(cargo_home) = cargo_home {
        command.env("CARGO_HOME", cargo_home);
    }
    command.arg("generate-lockfile");
    network.apply(&mut command);
    let output = command
        .arg("--manifest-path")
        .arg(CARGO_MANIFEST_FILENAME)
        .env_remove("CARGO_MANIFEST_DIR")
        .env_remove("CARGO_MANIFEST_PATH")
        .current_dir(output_dir)
        .output()?;
    if !output.status.success() {
        return Err(io::Error::other(format!(
            "Cargo could not derive a lock projection from the canonical Incan lock:\n{}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }

    let lock_path = output_dir.join("Cargo.lock");
    let initial = fs::read_to_string(&lock_path)?;
    let pass_limit = projection.reconciliation_pass_limit(&initial)?;
    let mut seen_targets = BTreeSet::new();
    for _ in 0..pass_limit {
        let payload = fs::read_to_string(&lock_path)?;
        let Some(candidates) =
            projection.next_update_candidates(&payload, generated_package_name, generated_package_version)?
        else {
            return Ok(());
        };
        record_reconciliation_target(&mut seen_targets, &candidates)?;
        let mut errors = Vec::new();
        let mut updated = false;
        for candidate in candidates {
            let mut command = Command::new(&cargo);
            sanitize_cargo_environment(&mut command);
            configure_cargo_target(&mut command, target_dir);
            if let Some(cargo_home) = cargo_home {
                command.env("CARGO_HOME", cargo_home);
            }
            command.arg("update");
            network.apply(&mut command);
            let output = command
                .arg("--manifest-path")
                .arg(CARGO_MANIFEST_FILENAME)
                .arg("--package")
                .arg(&candidate.package_spec)
                .arg("--precise")
                .arg(&candidate.precise)
                .env_remove("CARGO_MANIFEST_DIR")
                .env_remove("CARGO_MANIFEST_PATH")
                .current_dir(output_dir)
                .output()?;
            if output.status.success() {
                updated = true;
                break;
            }
            errors.push(String::from_utf8_lossy(&output.stderr).trim().to_string());
        }
        if !updated {
            return Err(io::Error::other(format!(
                "Cargo could not reconcile a generated dependency with the canonical Incan lock:\n{}",
                errors.join("\n")
            )));
        }
    }
    Err(io::Error::other(format!(
        "Cargo lock projection exceeded its graph-derived bound of {pass_limit} canonical reconciliation passes"
    )))
}

/// Record one reconciliation target and reject Cargo output that returns to an already attempted state.
fn record_reconciliation_target(
    seen_targets: &mut BTreeSet<Vec<super::lock_projection::CargoLockUpdate>>,
    candidates: &[super::lock_projection::CargoLockUpdate],
) -> io::Result<()> {
    if seen_targets.insert(candidates.to_vec()) {
        return Ok(());
    }
    Err(io::Error::other(format!(
        "Cargo lock projection made no monotonic reconciliation progress; repeated update target `{}`",
        candidates
            .first()
            .map_or("<empty>", |candidate| candidate.package_spec.as_str())
    )))
}

/// Result of an isolated Cargo-runner fixture build.
#[cfg(test)]
#[derive(Debug)]
pub struct BuildResult {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}

/// Result of running an isolated Cargo-runner fixture program.
#[cfg(test)]
#[derive(Debug)]
pub struct RunResult {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<i32>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::project::lock_projection::CargoLockUpdate;
    use crate::manifest::{DependencySource, DependencySpec};
    use std::collections::{BTreeMap, HashMap};
    use std::fs;

    /// Render the smallest valid lock payload for the pure root-name projection contract.
    ///
    /// The normal Oven path never asks Cargo to generate or reconcile generated-project locks. These tests retain
    /// coverage for preserving a caller-local projection without treating Cargo's resolver or cache population as
    /// product behavior.
    fn minimal_projected_cargo_lock(root_name: &str) -> String {
        format!(
            "version = 4\n\n[[package]]\nname = \"{root_name}\"\nversion = \"{}\"\n",
            crate::version::INCAN_VERSION
        )
    }

    #[test]
    fn run_profile_debug_uses_default_cargo_build_args_and_binary_dir() {
        let generator = ProjectGenerator::new("/tmp/incan_runner_debug", "demo", true);
        assert!(generator.run_profile_build_args().is_empty());
        assert_eq!(generator.run_profile_binary_dir(), "debug");
        let binary_path = generator.run_binary_path();
        let binary_path_str = binary_path.to_string_lossy();
        assert!(
            binary_path_str.contains("/debug/demo"),
            "expected debug binary path, got: {}",
            binary_path_str
        );
    }

    #[test]
    fn run_profile_release_uses_release_args_and_binary_dir() {
        let mut generator = ProjectGenerator::new("/tmp/incan_runner_release", "demo", true);
        generator.set_run_profile(RunProfile::Release);
        assert_eq!(generator.run_profile_build_args(), &["--release"]);
        assert_eq!(generator.run_profile_binary_dir(), "release");
        let binary_path = generator.run_binary_path();
        let binary_path_str = binary_path.to_string_lossy();
        assert!(
            binary_path_str.contains("/release/demo"),
            "expected release binary path, got: {}",
            binary_path_str
        );
    }

    #[test]
    fn unchanged_project_with_existing_binary_skips_cargo_build() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let generator = ProjectGenerator::new(tmp.path(), "demo", true);
        let binary_path = generator.run_binary_path();
        let parent = binary_path.parent().ok_or("missing binary parent")?;
        fs::create_dir_all(parent)?;
        fs::write(&binary_path, "")?;
        generator.write_run_publication_fingerprint()?;
        assert!(
            !generator.should_build_before_run(false),
            "existing unchanged binary should skip cargo build"
        );
        Ok(())
    }

    #[test]
    fn changed_project_still_rebuilds_even_when_binary_exists() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let generator = ProjectGenerator::new(tmp.path(), "demo", true);
        let binary_path = generator.run_binary_path();
        let parent = binary_path.parent().ok_or("missing binary parent")?;
        fs::create_dir_all(parent)?;
        fs::write(&binary_path, "")?;
        assert!(
            generator.should_build_before_run(true),
            "changed generated inputs must rebuild even with an existing binary"
        );
        Ok(())
    }

    #[cfg(feature = "cli")]
    #[test]
    fn changed_managed_domain_invalidates_project_local_run_binary() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let mut generator = ProjectGenerator::new(tmp.path(), "demo", true);
        generator.set_cargo_target_dir_override(Some(tmp.path().join("managed-target")));
        generator.set_generated_cache_context(None, Some("domain-a".to_string()));
        let binary = generator.run_binary_path();
        fs::create_dir_all(binary.parent().ok_or("missing binary parent")?)?;
        fs::write(&binary, "")?;
        generator.write_run_publication_fingerprint()?;
        assert!(!generator.should_build_before_run(false));

        generator.set_generated_cache_context(None, Some("domain-b".to_string()));
        assert!(generator.should_build_before_run(false));
        Ok(())
    }

    #[test]
    fn shared_cargo_target_publishes_binary_to_project_local_output() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let output_dir = tmp.path().join("generated");
        let shared_target = tmp.path().join("shared-target");
        let mut generator = ProjectGenerator::new(&output_dir, "demo", true);
        generator.set_cargo_target_dir_override(Some(shared_target.clone()));
        let cargo_binary = shared_target
            .join("custom-triple")
            .join("release")
            .join("cargo-reported-demo");
        fs::create_dir_all(cargo_binary.parent().ok_or("missing Cargo binary parent")?)?;
        fs::write(&cargo_binary, b"compiled")?;

        generator.publish_cargo_binary(&cargo_binary, &generator.binary_path())?;

        assert_eq!(fs::read(generator.binary_path())?, b"compiled");
        assert!(
            cargo_binary.exists(),
            "publication must not remove the shared Cargo artifact"
        );
        assert!(generator.binary_path().starts_with(&output_dir));
        assert!(!generator.binary_path().starts_with(generator.cargo_target_dir()));
        Ok(())
    }

    #[test]
    fn cargo_json_selects_exact_target_triple_executable() {
        let payload = br#"{"reason":"compiler-artifact","target":{"name":"dependency"},"executable":null}
{"reason":"compiler-message","message":{"rendered":"warning: probe\n"}}
{"reason":"compiler-artifact","target":{"name":"demo_abcd"},"executable":"/cache/target/aarch64-unknown-linux-gnu/release/demo_abcd"}
"#;
        let parsed = parse_cargo_json_build_output(payload, "demo_abcd");
        assert_eq!(
            parsed.executable,
            Some(PathBuf::from(
                "/cache/target/aarch64-unknown-linux-gnu/release/demo_abcd"
            ))
        );
        assert_eq!(parsed.rendered, "warning: probe\n");
    }

    #[test]
    fn cargo_config_identity_tracks_project_build_target() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let cargo_dir = tmp.path().join(".cargo");
        fs::create_dir_all(&cargo_dir)?;
        fs::write(
            cargo_dir.join("config.toml"),
            "[build]\ntarget = \"aarch64-apple-darwin\"\n",
        )?;
        let shallow = tmp.path().join("project");
        let deep = tmp.path().join("nested/project");
        fs::create_dir_all(&shallow)?;
        fs::create_dir_all(&deep)?;
        let first = cargo_config_identity(&shallow);
        assert_eq!(first, cargo_config_identity(&deep));
        fs::write(
            cargo_dir.join("config.toml"),
            "[build]\ntarget = \"x86_64-unknown-linux-gnu\"\n",
        )?;
        let second = cargo_config_identity(&shallow);
        assert_ne!(first, second);
        Ok(())
    }

    #[test]
    fn cargo_config_identity_distinguishes_generated_output_hierarchies() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let first = tmp.path().join("first/generated");
        let second = tmp.path().join("second/generated");
        fs::create_dir_all(first.join(".cargo"))?;
        fs::create_dir_all(second.join(".cargo"))?;
        fs::write(first.join(".cargo/config.toml"), "[profile.release]\nopt-level = 2\n")?;
        fs::write(second.join(".cargo/config.toml"), "[profile.release]\nopt-level = 3\n")?;

        assert_ne!(cargo_config_identity(&first), cargo_config_identity(&second));
        Ok(())
    }

    #[test]
    fn generated_cargo_target_contains_inherited_build_directory() {
        let target = Path::new("/managed/generated-target");
        let mut command = Command::new("cargo");
        command.env("CARGO_BUILD_BUILD_DIR", "/outside");
        configure_cargo_target(&mut command, target);

        let environment = command
            .get_envs()
            .filter_map(|(name, value)| value.map(|value| (name, value)))
            .collect::<BTreeMap<_, _>>();
        assert_eq!(
            environment.get(std::ffi::OsStr::new("CARGO_TARGET_DIR")),
            Some(&target.as_os_str())
        );
        assert_eq!(
            environment.get(std::ffi::OsStr::new("CARGO_BUILD_BUILD_DIR")),
            Some(&target.as_os_str())
        );
    }

    #[cfg(unix)]
    #[test]
    fn unchanged_run_skips_cargo_projection_and_build_subprocesses_issue921() -> Result<(), Box<dyn std::error::Error>>
    {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::tempdir()?;
        let mut generator = ProjectGenerator::new(tmp.path(), "caller", true);
        let canonical = format!(
            "version = 4\n\n[[package]]\nname = \"incan_workspace\"\nversion = \"{}\"\n",
            crate::version::INCAN_VERSION
        );
        let projected = format!(
            "version = 4\n\n[[package]]\nname = \"caller\"\nversion = \"{}\"\n",
            crate::version::INCAN_VERSION
        );
        generator.set_cargo_lock_payload(Some(canonical));
        generator.set_cargo_lock_projection_root(Some("incan_workspace".to_string()));
        // Deliberately omit Cargo.toml: entering projection or build would fail. The valid prepared projection and
        // executable prove the unchanged fast path invokes only the generated binary.
        fs::write(tmp.path().join("Cargo.lock"), projected)?;
        let binary = generator.run_binary_path();
        fs::create_dir_all(binary.parent().ok_or("missing binary parent")?)?;
        fs::write(&binary, "#!/bin/sh\nexit 0\n")?;
        let mut permissions = fs::metadata(&binary)?.permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&binary, permissions)?;
        generator.write_run_publication_fingerprint()?;

        let result = generator.run_with_cwd(tmp.path(), false)?;
        assert!(result.success);
        Ok(())
    }

    #[test]
    fn shared_target_safe_name_distinguishes_root_identities() {
        let first = ProjectGenerator::shared_target_safe_name("demo-app", "root-one");
        let second = ProjectGenerator::shared_target_safe_name("demo-app", "root-two");

        assert_ne!(first, second);
        assert!(first.starts_with("demo_app_"), "unexpected target name: {first}");
        assert!(
            first.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '_'),
            "target name should be Rust-identifier safe: {first}"
        );
    }

    #[test]
    fn generated_source_identity_is_stable_across_output_directories() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let shared_target = tmp.path().join("shared");
        let mut first = ProjectGenerator::new(tmp.path().join("first"), "demo", true);
        first.set_cargo_target_dir_override(Some(shared_target.clone()));
        first.generate("fn main() { println!(\"same\"); }")?;
        let mut second = ProjectGenerator::new(tmp.path().join("second"), "demo", true);
        second.set_cargo_target_dir_override(Some(shared_target.clone()));
        second.generate("fn main() { println!(\"same\"); }")?;
        let mut changed = ProjectGenerator::new(tmp.path().join("changed"), "demo", true);
        changed.set_cargo_target_dir_override(Some(shared_target.clone()));
        changed.generate("fn main() { println!(\"changed\"); }")?;
        let mut changed_manifest = ProjectGenerator::new(tmp.path().join("changed-manifest"), "demo", true);
        changed_manifest.set_cargo_target_dir_override(Some(shared_target));
        changed_manifest.set_package_metadata(Some("9.9.9".to_string()), None);
        changed_manifest.generate("fn main() { println!(\"same\"); }")?;
        let mut changed_sdk = ProjectGenerator::new(tmp.path().join("changed-sdk"), "demo", true);
        changed_sdk.set_cargo_target_dir_override(Some(tmp.path().join("shared")));
        changed_sdk.set_sdk_path_dependencies(vec![DependencySpec {
            crate_name: "provider".to_string(),
            version: Some("1.0.0".to_string()),
            features: Vec::new(),
            default_features: true,
            source: DependencySource::Path {
                path: tmp.path().join("provider-a"),
            },
            optional: false,
            package: None,
        }]);
        changed_sdk.generate("fn main() { println!(\"same\"); }")?;
        let mut relocated_sdk = ProjectGenerator::new(tmp.path().join("relocated-sdk"), "demo", true);
        relocated_sdk.set_cargo_target_dir_override(Some(tmp.path().join("shared")));
        relocated_sdk.set_sdk_path_dependencies(vec![DependencySpec {
            crate_name: "provider".to_string(),
            version: Some("1.0.0".to_string()),
            features: Vec::new(),
            default_features: true,
            source: DependencySource::Path {
                path: tmp.path().join("provider-b"),
            },
            optional: false,
            package: None,
        }]);
        relocated_sdk.generate("fn main() { println!(\"same\"); }")?;

        assert_eq!(first.cargo_target_name(), second.cargo_target_name());
        assert_ne!(first.cargo_target_name(), changed.cargo_target_name());
        assert_ne!(first.cargo_target_name(), changed_manifest.cargo_target_name());
        assert_ne!(first.cargo_target_name(), changed_sdk.cargo_target_name());
        assert_eq!(changed_sdk.cargo_target_name(), relocated_sdk.cargo_target_name());
        Ok(())
    }

    #[test]
    fn root_artifact_lock_serializes_compatible_publishers() -> Result<(), Box<dyn std::error::Error>> {
        use std::sync::mpsc::{self, RecvTimeoutError};
        use std::time::Duration;

        let tmp = tempfile::tempdir()?;
        let shared_target = tmp.path().join("shared");
        let mut first = ProjectGenerator::new(tmp.path().join("first"), "demo", true);
        first.set_cargo_target_dir_override(Some(shared_target.clone()));
        first.generate("fn main() {}")?;
        let mut second = ProjectGenerator::new(tmp.path().join("second"), "demo", true);
        second.set_cargo_target_dir_override(Some(shared_target));
        second.generate("fn main() {}")?;
        assert_eq!(first.cargo_target_name(), second.cargo_target_name());

        let first_guard = first.acquire_root_artifact_lock()?;
        let (acquired_tx, acquired_rx) = mpsc::channel();
        let waiter = std::thread::spawn(move || -> io::Result<()> {
            let second_guard = second.acquire_root_artifact_lock()?;
            acquired_tx
                .send(())
                .map_err(|error| io::Error::other(format!("failed to report acquired lock: {error}")))?;
            drop(second_guard);
            Ok(())
        });

        assert!(matches!(
            acquired_rx.recv_timeout(Duration::from_millis(100)),
            Err(RecvTimeoutError::Timeout)
        ));
        drop(first_guard);
        acquired_rx.recv_timeout(Duration::from_secs(2))?;
        match waiter.join() {
            Ok(result) => result?,
            Err(_) => return Err("root-artifact lock waiter panicked".into()),
        }
        Ok(())
    }

    #[test]
    fn relative_target_dirs_resolve_against_current_working_dir() -> Result<(), Box<dyn std::error::Error>> {
        let cwd = std::env::current_dir()?;
        let target_dir = ProjectGenerator::resolve_target_dir(PathBuf::from("target/shared-generated"));
        assert_eq!(target_dir, cwd.join("target/shared-generated"));
        Ok(())
    }

    #[test]
    fn projection_network_policy_does_not_treat_locked_as_offline() {
        assert_eq!(
            CargoLockProjectionNetwork::from_cargo_flags(&[]),
            CargoLockProjectionNetwork::Online
        );
        assert_eq!(
            CargoLockProjectionNetwork::from_cargo_flags(&["--locked".to_string()]),
            CargoLockProjectionNetwork::Online
        );
        assert_eq!(
            CargoLockProjectionNetwork::from_cargo_flags(&["--offline".to_string()]),
            CargoLockProjectionNetwork::Offline
        );
        assert_eq!(
            CargoLockProjectionNetwork::from_cargo_flags(&["--frozen".to_string()]),
            CargoLockProjectionNetwork::Offline
        );

        let mut online = Command::new("cargo");
        CargoLockProjectionNetwork::Online.apply(&mut online);
        assert!(online.get_args().next().is_none());
        let mut offline = Command::new("cargo");
        CargoLockProjectionNetwork::Offline.apply(&mut offline);
        assert_eq!(offline.get_args().collect::<Vec<_>>(), ["--offline"]);
    }

    #[test]
    fn repeated_reconciliation_target_is_rejected_as_non_monotonic() -> Result<(), Box<dyn std::error::Error>> {
        let candidates = vec![CargoLockUpdate {
            package_spec: "registry+https://example.invalid/index#dep@2.0.0".to_string(),
            precise: "1.0.0".to_string(),
        }];
        let mut seen = BTreeSet::new();
        record_reconciliation_target(&mut seen, &candidates)?;
        let error = match record_reconciliation_target(&mut seen, &candidates) {
            Ok(()) => return Err("a repeated reconciliation state was accepted".into()),
            Err(error) => error,
        };
        assert!(error.to_string().contains("no monotonic reconciliation progress"));
        Ok(())
    }

    #[test]
    fn identical_generation_preserves_a_valid_projected_cargo_lock_issue921() -> Result<(), Box<dyn std::error::Error>>
    {
        let tmp = tempfile::tempdir()?;
        let mut generator = ProjectGenerator::new(tmp.path(), "issue921_projection_caller", true);
        generator.generate_multi("fn main() {}", &HashMap::new())?;

        let caller_lock = minimal_projected_cargo_lock("issue921_projection_caller");
        let canonical_lock = minimal_projected_cargo_lock("incan_workspace");
        assert_ne!(caller_lock, canonical_lock, "fixture must rename the canonical root");
        fs::write(tmp.path().join("Cargo.lock"), &caller_lock)?;
        generator.set_cargo_lock_payload(Some(canonical_lock));
        generator.set_cargo_lock_projection_root(Some("incan_workspace".to_string()));
        generator.set_cargo_policy_flags(vec!["--offline".to_string()]);

        let cargo_toml = fs::read_to_string(tmp.path().join("Cargo.toml"))?;
        let main = fs::read_to_string(tmp.path().join("src/main.rs"))?;
        let changed = generator.generate_multi("fn main() {}", &HashMap::new())?;
        assert_eq!(caller_lock, fs::read_to_string(tmp.path().join("Cargo.lock"))?);
        assert_eq!(cargo_toml, fs::read_to_string(tmp.path().join("Cargo.toml"))?);
        assert_eq!(main, fs::read_to_string(tmp.path().join("src/main.rs"))?);
        assert!(!changed);
        assert!(!generator.generate_multi("fn main() {}", &HashMap::new())?);
        Ok(())
    }

    #[test]
    fn relative_output_dir_projects_without_duplicating_manifest_path_issue921()
    -> Result<(), Box<dyn std::error::Error>> {
        let fixture_parent = Path::new("target").join("issue921-relative-projection");
        fs::create_dir_all(&fixture_parent)?;
        let tmp = tempfile::Builder::new()
            .prefix("generated-")
            .tempdir_in(&fixture_parent)?;
        let cwd = env::current_dir()?;
        let output_dir = tmp.path().strip_prefix(&cwd)?.to_path_buf();
        assert!(
            !output_dir.is_absolute(),
            "fixture must exercise a relative generated output directory"
        );

        let mut generator = ProjectGenerator::new(&output_dir, "issue921_relative_caller", true);
        generator.generate_multi("fn main() {}", &HashMap::new())?;

        let caller_lock = minimal_projected_cargo_lock("issue921_relative_caller");
        let canonical_lock = minimal_projected_cargo_lock("incan_workspace");
        assert_ne!(caller_lock, canonical_lock, "fixture must rename the canonical root");
        fs::write(output_dir.join("Cargo.lock"), &caller_lock)?;
        generator.set_cargo_lock_payload(Some(canonical_lock));
        generator.set_cargo_lock_projection_root(Some("incan_workspace".to_string()));
        generator.set_cargo_policy_flags(vec!["--offline".to_string()]);
        assert!(!generator.generate_multi("fn main() {}", &HashMap::new())?);

        let projected = fs::read_to_string(output_dir.join("Cargo.lock"))?;
        assert!(projected.contains("name = \"issue921_relative_caller\""));
        assert!(!projected.contains("name = \"incan_workspace\""));
        Ok(())
    }
}
