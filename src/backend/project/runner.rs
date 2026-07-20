//! Build and run logic for generated Rust projects
//!
//! Provides [`ProjectGenerator::build`], [`ProjectGenerator::run`], and [`ProjectGenerator::run_with_cwd`] along with
//! their result types.

use std::collections::BTreeSet;
use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
#[cfg(test)]
use std::sync::{LazyLock, Mutex};

use super::generator::{ProjectGenerator, RunProfile};

const CARGO_MANIFEST_FILENAME: &str = "Cargo.toml";

#[cfg(test)]
static TEST_PROJECTION_CARGO_POLICIES: LazyLock<Mutex<std::collections::BTreeMap<PathBuf, Vec<String>>>> =
    LazyLock::new(|| Mutex::new(std::collections::BTreeMap::new()));

#[cfg(test)]
/// Return the Cargo policy flags observed by projection for a generated test project.
pub(crate) fn test_projection_cargo_policy(output_dir: &Path) -> Option<Vec<String>> {
    TEST_PROJECTION_CARGO_POLICIES.lock().ok()?.get(output_dir).cloned()
}

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

impl ProjectGenerator {
    /// Ask Cargo to project a fresh canonical seed onto this caller-local manifest and prove two-pass convergence.
    pub(crate) fn materialize_cargo_lock_projection(&self) -> io::Result<bool> {
        let Some(projection) = self.cargo_lock_projection()? else {
            return Ok(false);
        };
        #[cfg(test)]
        if let Ok(mut policies) = TEST_PROJECTION_CARGO_POLICIES.lock() {
            policies.insert(self.output_dir.clone(), self.cargo_policy_flags.clone());
        }
        let lock_path = self.output_dir.join("Cargo.lock");
        let seed = fs::read_to_string(&lock_path)?;
        run_cargo_lock_projection(
            &self.output_dir,
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

    /// Whether `incan run` must invoke Cargo before executing the generated binary.
    fn should_build_before_run(&self, project_changed: bool) -> bool {
        project_changed || !self.run_binary_path().is_file()
    }

    /// Return extra Cargo CLI args required to build with the configured run profile.
    fn run_profile_build_args(&self) -> &'static [&'static str] {
        match self.run_profile {
            RunProfile::Debug => &[],
            RunProfile::Release => &["--release"],
        }
    }

    /// Return the Cargo target subdirectory that contains binaries for the configured run profile.
    fn run_profile_binary_dir(&self) -> &'static str {
        match self.run_profile {
            RunProfile::Debug => "debug",
            RunProfile::Release => "release",
        }
    }

    /// Return a human-readable label for the configured run profile.
    fn run_profile_label(&self) -> &'static str {
        match self.run_profile {
            RunProfile::Debug => "debug",
            RunProfile::Release => "release",
        }
    }

    /// Shared Cargo target directory for generated projects under the same parent folder.
    ///
    /// Generated projects like `target/incan/<name>` and `target/incan_tests/<case>` otherwise each get their own
    /// nested `target/` directory, which forces Cargo to rebuild dependencies repeatedly across examples, smoke
    /// tests, and benchmark checks. Sharing a parent-scoped target dir lets those generated crates reuse compiled
    /// dependencies.
    pub(crate) fn cargo_target_dir(&self) -> PathBuf {
        if let Some(target_dir) = self.cargo_target_dir_override() {
            return target_dir;
        }

        let base_dir = self.output_dir.parent().unwrap_or(self.output_dir.as_path());
        let target_dir = base_dir.join(".cargo-target");

        Self::resolve_target_dir(target_dir)
    }

    /// Build the project using cargo.
    pub fn build(&self) -> io::Result<BuildResult> {
        self.materialize_cargo_lock_projection()?;
        let cargo_target_dir = self.cargo_target_dir();
        let mut command = Command::new("cargo");
        sanitize_cargo_environment(&mut command);
        command.arg("build").arg("--release");
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
            .env("CARGO_TARGET_DIR", &cargo_target_dir)
            .current_dir(&self.output_dir)
            .output()?;

        Ok(BuildResult {
            success: output.status.success(),
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        })
    }

    /// Run the project using cargo.
    ///
    /// Uses inherited stdio so output streams to terminal in real-time (important for long-running processes like web
    /// servers).
    ///
    /// Note: This is only used by `incan run` during dev. By default `incan run` uses Cargo's debug profile for fast
    /// iteration and supports `--release` as an opt-in.
    /// Production deployments run the generated binary directly.
    pub fn run(&self) -> io::Result<RunResult> {
        self.run_with_cwd(&self.output_dir, true)
    }

    /// Run the project with a custom working directory.
    ///
    /// This builds the generated Rust project, then runs the resulting binary with `cwd` as the working directory.
    /// This keeps runtime-relative paths anchored to the original project root rather than the generated
    /// `target/incan/...` directory.
    ///
    /// Cargo build output is streamed directly to the terminal so incremental compilation progress remains visible on
    /// slow first runs and long rebuilds.
    pub fn run_with_cwd(&self, cwd: &Path, project_changed: bool) -> io::Result<RunResult> {
        // Generation validates and preserves an existing projection before reporting `project_changed = false`.
        // Avoid a redundant two-pass Cargo projection in that fast path. A changed/direct run must still materialize
        // its canonical seed before Cargo is allowed to build it.
        let project_changed = if project_changed {
            self.materialize_cargo_lock_projection()?;
            true
        } else {
            false
        };
        if self.should_build_before_run(project_changed) {
            // ---- Context: build generated crate with selected run profile ----
            let cargo_target_dir = self.cargo_target_dir();
            eprintln!(
                "Building generated project with cargo ({}) profile...",
                self.run_profile_label()
            );
            let mut build_command = Command::new("cargo");
            sanitize_cargo_environment(&mut build_command);
            build_command.arg("build");
            for arg in self.run_profile_build_args() {
                build_command.arg(arg);
            }
            for flag in &self.cargo_policy_flags {
                build_command.arg(flag);
            }
            let build_status = build_command
                // Ensure we don't inherit a broken CA bundle path from the parent env.
                .env_remove("SSL_CERT_FILE")
                .env_remove("SSL_CERT_DIR")
                .env_remove("CURL_CA_BUNDLE")
                .env_remove("REQUESTS_CA_BUNDLE")
                .env_remove("CARGO_HTTP_CAINFO")
                .env("CARGO_TARGET_DIR", &cargo_target_dir)
                .current_dir(&self.output_dir)
                .stdout(Stdio::inherit())
                .stderr(Stdio::inherit())
                .status()?;
            if !build_status.success() {
                return Ok(RunResult {
                    success: false,
                    stdout: String::new(),
                    stderr: String::new(),
                    exit_code: build_status.code(),
                });
            }
        } else {
            eprintln!(
                "Generated project unchanged; reusing existing cargo ({}) binary.",
                self.run_profile_label()
            );
        }

        // ---- Context: execute built binary with caller-provided cwd ----
        eprintln!("Build finished. Running generated binary...");
        let mut child = Command::new(self.run_binary_path())
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

    /// Get the path to the built binary.
    pub fn binary_path(&self) -> PathBuf {
        self.cargo_target_dir().join("release").join(self.cargo_target_name())
    }

    /// Get the path to the binary produced for `incan run`.
    pub fn run_binary_path(&self) -> PathBuf {
        self.cargo_target_dir()
            .join(self.run_profile_binary_dir())
            .join(self.cargo_target_name())
    }
}

/// Run one Cargo-owned projection pass against an already rendered generated manifest.
fn run_cargo_lock_projection(
    output_dir: &Path,
    projection: &super::lock_projection::CargoLockProjection,
    generated_package_name: &str,
    generated_package_version: &str,
    network: CargoLockProjectionNetwork,
    cargo_home: Option<&Path>,
) -> io::Result<()> {
    let cargo = env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let mut command = Command::new(&cargo);
    sanitize_cargo_environment(&mut command);
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

/// Result of a cargo build.
#[derive(Debug)]
pub struct BuildResult {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}

/// Result of running the built program.
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
    use crate::backend::project::lock_projection::{CargoLockProjection, CargoLockUpdate};
    use std::collections::HashMap;
    use std::fs;

    fn successful_command(mut command: Command, label: &str) -> Result<(), Box<dyn std::error::Error>> {
        let output = command.output()?;
        if output.status.success() {
            return Ok(());
        }
        Err(format!(
            "{label} failed:\n{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
        .into())
    }

    fn initialize_git_fixture(repository: &Path) -> Result<(), Box<dyn std::error::Error>> {
        let mut init = Command::new("git");
        init.args(["init", "-q"]).current_dir(repository);
        successful_command(init, "git init")?;
        for (key, value) in [("user.email", "incan@example.invalid"), ("user.name", "Incan Test")] {
            let mut config = Command::new("git");
            config.args(["config", key, value]).current_dir(repository);
            successful_command(config, "git config")?;
        }
        Ok(())
    }

    fn commit_git_fixture(repository: &Path, message: &str) -> Result<(), Box<dyn std::error::Error>> {
        let mut add = Command::new("git");
        add.args(["add", "."]).current_dir(repository);
        successful_command(add, "git add")?;
        let mut commit = Command::new("git");
        commit.args(["commit", "-q", "-m", message]).current_dir(repository);
        successful_command(commit, "git commit")?;
        Ok(())
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

        let result = generator.run_with_cwd(tmp.path(), false)?;
        assert!(result.success);
        Ok(())
    }

    #[test]
    fn shared_target_safe_name_distinguishes_same_project_name_by_output_dir() -> Result<(), Box<dyn std::error::Error>>
    {
        let tmp = tempfile::tempdir()?;
        let first = ProjectGenerator::shared_target_safe_name("demo-app", &tmp.path().join("one"));
        let second = ProjectGenerator::shared_target_safe_name("demo-app", &tmp.path().join("two"));

        assert_ne!(first, second);
        assert!(first.starts_with("demo_app_"), "unexpected target name: {first}");
        assert!(
            first.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '_'),
            "target name should be Rust-identifier safe: {first}"
        );
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

        let cargo = env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
        let mut seed_command = Command::new(cargo);
        seed_command
            .arg("generate-lockfile")
            .arg("--offline")
            .arg("--manifest-path")
            .arg(tmp.path().join("Cargo.toml"));
        successful_command(seed_command, "initial caller lock generation")?;
        let caller_lock = fs::read_to_string(tmp.path().join("Cargo.lock"))?;
        let canonical_lock =
            caller_lock.replacen("name = \"issue921_projection_caller\"", "name = \"incan_workspace\"", 1);
        assert_ne!(caller_lock, canonical_lock, "fixture must rename the canonical root");

        fs::remove_file(tmp.path().join("Cargo.lock"))?;
        generator.set_cargo_lock_payload(Some(canonical_lock));
        generator.set_cargo_lock_projection_root(Some("incan_workspace".to_string()));
        generator.set_cargo_policy_flags(vec!["--offline".to_string()]);

        assert!(generator.generate_multi("fn main() {}", &HashMap::new())?);
        assert!(generator.materialize_cargo_lock_projection()?);
        let first = fs::read(tmp.path().join("Cargo.lock"))?;

        assert!(!generator.generate_multi("fn main() {}", &HashMap::new())?);
        assert!(!generator.materialize_cargo_lock_projection()?);
        assert_eq!(first, fs::read(tmp.path().join("Cargo.lock"))?);
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

        let cargo = env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
        let mut seed_command = Command::new(cargo);
        seed_command
            .arg("generate-lockfile")
            .arg("--offline")
            .arg("--manifest-path")
            .arg(output_dir.join(CARGO_MANIFEST_FILENAME));
        successful_command(seed_command, "relative caller lock generation")?;
        let caller_lock = fs::read_to_string(output_dir.join("Cargo.lock"))?;
        let canonical_lock =
            caller_lock.replacen("name = \"issue921_relative_caller\"", "name = \"incan_workspace\"", 1);
        assert_ne!(caller_lock, canonical_lock, "fixture must rename the canonical root");

        fs::remove_file(output_dir.join("Cargo.lock"))?;
        generator.set_cargo_lock_payload(Some(canonical_lock));
        generator.set_cargo_lock_projection_root(Some("incan_workspace".to_string()));
        generator.set_cargo_policy_flags(vec!["--offline".to_string()]);
        assert!(generator.generate_multi("fn main() {}", &HashMap::new())?);
        assert!(generator.materialize_cargo_lock_projection()?);

        let projected = fs::read_to_string(output_dir.join("Cargo.lock"))?;
        assert!(projected.contains("name = \"issue921_relative_caller\""));
        assert!(!projected.contains("name = \"incan_workspace\""));
        Ok(())
    }

    #[test]
    fn online_locked_projection_can_populate_a_fresh_cargo_home_issue923() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let dependency = tmp.path().join("dependency");
        fs::create_dir_all(dependency.join("src"))?;
        fs::write(
            dependency.join("Cargo.toml"),
            "[package]\nname = \"issue923_local_dep\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )?;
        fs::write(dependency.join("src/lib.rs"), "pub fn value() -> u8 { 1 }\n")?;
        initialize_git_fixture(&dependency)?;
        commit_git_fixture(&dependency, "fixture")?;

        let dependency_url = format!("file://{}", dependency.display());
        let manifest = |name: &str| {
            format!(
                "[package]\nname = \"{name}\"\nversion = \"{}\"\nedition = \"2024\"\n\n[dependencies]\nissue923_local_dep = {{ git = \"{dependency_url}\" }}\n",
                crate::version::INCAN_VERSION
            )
        };
        let canonical = tmp.path().join("canonical");
        fs::create_dir_all(canonical.join("src"))?;
        fs::write(canonical.join("Cargo.toml"), manifest("incan_workspace"))?;
        fs::write(canonical.join("src/lib.rs"), "pub fn canonical() {}\n")?;
        let canonical_home = tmp.path().join("canonical-cargo-home");
        fs::create_dir_all(&canonical_home)?;
        let cargo = env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
        let mut canonical_command = Command::new(&cargo);
        canonical_command
            .arg("generate-lockfile")
            .arg("--manifest-path")
            .arg(canonical.join("Cargo.toml"))
            .env("CARGO_HOME", &canonical_home);
        successful_command(canonical_command, "canonical lock generation")?;
        let canonical_payload = fs::read_to_string(canonical.join("Cargo.lock"))?;

        let caller = tmp.path().join("caller");
        fs::create_dir_all(caller.join("src"))?;
        fs::write(caller.join("Cargo.toml"), manifest("caller"))?;
        fs::write(caller.join("src/lib.rs"), "pub fn caller() {}\n")?;
        fs::write(caller.join("Cargo.lock"), &canonical_payload)?;
        let projection = CargoLockProjection::new(canonical_payload, "incan_workspace".to_string())?;
        let fresh_home = tmp.path().join("fresh-cargo-home");
        fs::create_dir_all(&fresh_home)?;

        run_cargo_lock_projection(
            &caller,
            &projection,
            "caller",
            crate::version::INCAN_VERSION,
            CargoLockProjectionNetwork::from_cargo_flags(&["--locked".to_string()]),
            Some(&fresh_home),
        )?;
        let projected = fs::read_to_string(caller.join("Cargo.lock"))?;
        projection.validate_projected(&projected, "caller", crate::version::INCAN_VERSION)?;
        assert!(
            fresh_home.join("git").is_dir(),
            "fresh Cargo home should receive the git source"
        );
        Ok(())
    }
}
