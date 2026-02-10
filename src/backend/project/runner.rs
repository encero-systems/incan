//! Build and run logic for generated Rust projects
//!
//! Provides [`ProjectGenerator::build`], [`ProjectGenerator::run`], and [`ProjectGenerator::run_with_cwd`] along with
//! their result types.

use std::io;
use std::path::Path;
use std::process::{Command, Stdio};

use super::generator::ProjectGenerator;

impl ProjectGenerator {
    /// Build the project using cargo.
    pub fn build(&self) -> io::Result<BuildResult> {
        let mut command = Command::new("cargo");
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
    /// Uses inherited stdio so output streams to terminal in real-time (important for long-running processes like
    /// web servers).
    ///
    /// Note: This is only used by `incan run` during dev. Production deployments run the generated binary directly.
    pub fn run(&self) -> io::Result<RunResult> {
        self.run_with_cwd(&self.output_dir)
    }

    /// Run the project with a custom working directory.
    ///
    /// This builds the generated Rust project, then runs the resulting binary with `cwd` as the working directory.
    /// This keeps runtime-relative paths anchored to the original project root rather than the generated
    /// `target/incan/...` directory.
    pub fn run_with_cwd(&self, cwd: &Path) -> io::Result<RunResult> {
        // Build first so we can run the binary directly with a custom cwd.
        let mut build_command = Command::new("cargo");
        build_command.arg("build").arg("--release");
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
            .output()?;
        if !build_output.status.success() {
            return Ok(RunResult {
                success: false,
                stdout: String::from_utf8_lossy(&build_output.stdout).to_string(),
                stderr: String::from_utf8_lossy(&build_output.stderr).to_string(),
                exit_code: build_output.status.code(),
            });
        }

        let binary_path = self.binary_path();
        let binary_path = if binary_path.is_absolute() {
            binary_path
        } else {
            std::env::current_dir()?.join(binary_path)
        };

        let mut child = Command::new(binary_path)
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
    pub fn binary_path(&self) -> std::path::PathBuf {
        self.output_dir.join("target").join("release").join(&self.name)
    }
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
