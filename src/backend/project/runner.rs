//! Build and run logic for generated Rust projects
//!
//! Provides [`ProjectGenerator::build`], [`ProjectGenerator::run`], and [`ProjectGenerator::run_with_cwd`] along with
//! their result types.

use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use super::generator::{ProjectGenerator, RunProfile};

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
        let lock_path = self.output_dir.join("Cargo.lock");
        let seed = fs::read_to_string(&lock_path)?;
        run_cargo_lock_projection(
            &self.output_dir,
            &projection,
            self.cargo_package_name(),
            self.cargo_package_version(),
        )?;
        let first = fs::read_to_string(&lock_path)?;
        projection.validate_projected(&first, self.cargo_package_name(), self.cargo_package_version())?;

        fs::write(&lock_path, projection.seed_payload())?;
        run_cargo_lock_projection(
            &self.output_dir,
            &projection,
            self.cargo_package_name(),
            self.cargo_package_version(),
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
        let project_changed = project_changed || self.materialize_cargo_lock_projection()?;
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

/// Run one offline Cargo-owned projection pass against an already rendered generated manifest.
fn run_cargo_lock_projection(
    output_dir: &Path,
    projection: &super::lock_projection::CargoLockProjection,
    generated_package_name: &str,
    generated_package_version: &str,
) -> io::Result<()> {
    let mut command = Command::new("cargo");
    sanitize_cargo_environment(&mut command);
    let output = command
        .arg("generate-lockfile")
        .arg("--offline")
        .arg("--manifest-path")
        .arg(output_dir.join("Cargo.toml"))
        .env_remove("CARGO_MANIFEST_DIR")
        .env_remove("CARGO_MANIFEST_PATH")
        .current_dir(output_dir)
        .output()?;
    if !output.status.success() {
        return Err(io::Error::other(format!(
            "Cargo could not derive an offline lock projection from the canonical Incan lock:\n{}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }

    let lock_path = output_dir.join("Cargo.lock");
    for _ in 0..1024 {
        let payload = fs::read_to_string(&lock_path)?;
        let Some(candidates) =
            projection.next_update_candidates(&payload, generated_package_name, generated_package_version)?
        else {
            return Ok(());
        };
        let mut errors = Vec::new();
        let mut updated = false;
        for candidate in candidates {
            let mut command = Command::new("cargo");
            sanitize_cargo_environment(&mut command);
            let output = command
                .arg("update")
                .arg("--offline")
                .arg("--manifest-path")
                .arg(output_dir.join("Cargo.toml"))
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
    Err(io::Error::other(
        "Cargo lock projection exceeded its bounded canonical reconciliation passes",
    ))
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
    use std::fs;

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
}
