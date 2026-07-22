//! CompilationPlan + Executor: separating "what to do" from "doing it"
//!
//! [`CompilationPlan`] is a pure, testable representation of what the compiler will produce.
//! [`Executor`] performs the actual filesystem operations and cargo invocations.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use super::runner::{cargo_command, configure_cargo_target};

/// A file to be written as part of a compilation plan.
#[derive(Debug, Clone)]
pub struct PlannedFile {
    /// Path where the file should be written
    pub path: PathBuf,
    /// Content of the file
    pub content: String,
}

/// A directory to be created as part of a compilation plan.
#[derive(Debug, Clone)]
pub struct PlannedDirectory {
    /// Path to the directory
    pub path: PathBuf,
}

/// A cargo command to execute as part of the build.
#[derive(Debug, Clone)]
pub enum CargoCommand {
    /// `cargo build --release`
    Build,
    /// `cargo run --release`
    Run,
}

/// A pure, testable representation of what the compiler will produce.
///
/// This struct contains all the information needed to generate a Rust project without performing any side effects.
/// Use [`Executor::execute`] to actually write files and run commands.
///
/// # Design rationale
///
/// Separating planning from execution enables:
/// - Unit testing the planning logic without touching the filesystem
/// - Inspecting what would be generated before committing
/// - Future: dry-run mode, caching, reproducibility checks
#[derive(Debug, Clone)]
pub struct CompilationPlan {
    /// Project name
    pub project_name: String,
    /// Output directory for the generated project
    pub output_dir: PathBuf,
    /// Directories to create (in order)
    pub directories: Vec<PlannedDirectory>,
    /// Files to write (in order)
    pub files: Vec<PlannedFile>,
    /// Optional cargo command to run after generating files
    pub cargo_command: Option<CargoCommand>,
}

impl CompilationPlan {
    /// Create a new empty compilation plan.
    pub fn new(project_name: impl Into<String>, output_dir: impl AsRef<Path>) -> Self {
        Self {
            project_name: project_name.into(),
            output_dir: output_dir.as_ref().to_path_buf(),
            directories: Vec::new(),
            files: Vec::new(),
            cargo_command: None,
        }
    }

    /// Add a directory to create.
    pub fn add_directory(&mut self, path: impl AsRef<Path>) {
        self.directories.push(PlannedDirectory {
            path: path.as_ref().to_path_buf(),
        });
    }

    /// Add a file to write.
    pub fn add_file(&mut self, path: impl AsRef<Path>, content: impl Into<String>) {
        self.files.push(PlannedFile {
            path: path.as_ref().to_path_buf(),
            content: content.into(),
        });
    }

    /// Set the cargo command to run after generating files.
    pub fn set_cargo_command(&mut self, cmd: CargoCommand) {
        self.cargo_command = Some(cmd);
    }

    /// Get the expected path to the built binary.
    pub fn binary_path(&self) -> PathBuf {
        self.output_dir.join("target").join("release").join(&self.project_name)
    }
}

/// Executes a [`CompilationPlan`] by performing filesystem operations and running commands.
///
/// This is the only place where side effects occur in the project generation pipeline.
/// The caller owns the plan's output directory and its contained Cargo target. Incan's CLI does not use this legacy
/// executor; CLI build/run/test/library commands select the managed generated-build cache instead.
#[derive(Debug, Default)]
pub struct Executor;

impl Executor {
    /// Create a new executor.
    pub fn new() -> Self {
        Self
    }

    /// Execute a compilation plan: create directories, write files, optionally run cargo.
    ///
    /// Returns the result of the cargo command if one was specified, or `Ok(None)` if the plan only generates files.
    pub fn execute(&self, plan: &CompilationPlan) -> io::Result<Option<ExecutionResult>> {
        // ---- Create directories ----
        for dir in &plan.directories {
            fs::create_dir_all(&dir.path)?;
        }

        // ---- Write files ----
        for file in &plan.files {
            // Ensure parent directory exists
            if let Some(parent) = file.path.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(&file.path, &file.content)?;
        }

        // ---- Run cargo command if specified ----
        match &plan.cargo_command {
            Some(CargoCommand::Build) => {
                let mut command = cargo_command();
                configure_cargo_target(&mut command, &plan_cargo_target_dir(&plan.output_dir)?);
                let output = command
                    .arg("build")
                    .arg("--release")
                    // Ensure we don't inherit a broken CA bundle path from the parent env.
                    // This makes `cargo` more robust across environments (CI/sandboxes/local).
                    .env_remove("SSL_CERT_FILE")
                    .env_remove("SSL_CERT_DIR")
                    .env_remove("CURL_CA_BUNDLE")
                    .env_remove("REQUESTS_CA_BUNDLE")
                    .env_remove("CARGO_HTTP_CAINFO")
                    .current_dir(&plan.output_dir)
                    .output()?;

                Ok(Some(ExecutionResult {
                    success: output.status.success(),
                    stdout: String::from_utf8_lossy(&output.stdout).to_string(),
                    stderr: String::from_utf8_lossy(&output.stderr).to_string(),
                    exit_code: output.status.code(),
                }))
            }
            Some(CargoCommand::Run) => {
                let mut command = cargo_command();
                configure_cargo_target(&mut command, &plan_cargo_target_dir(&plan.output_dir)?);
                let mut child = command
                    .arg("run")
                    .arg("--release")
                    // Ensure we don't inherit a broken CA bundle path from the parent env.
                    .env_remove("SSL_CERT_FILE")
                    .env_remove("SSL_CERT_DIR")
                    .env_remove("CURL_CA_BUNDLE")
                    .env_remove("REQUESTS_CA_BUNDLE")
                    .env_remove("CARGO_HTTP_CAINFO")
                    .current_dir(&plan.output_dir)
                    .stdout(Stdio::inherit())
                    .stderr(Stdio::inherit())
                    .spawn()?;

                let status = child.wait()?;

                Ok(Some(ExecutionResult {
                    success: status.success(),
                    stdout: String::new(), // Output went directly to terminal
                    stderr: String::new(),
                    exit_code: status.code(),
                }))
            }
            None => Ok(None),
        }
    }
}

/// Resolve the caller-owned target absolutely so a relative output directory is not applied twice by Cargo's cwd.
fn plan_cargo_target_dir(output_dir: &Path) -> io::Result<PathBuf> {
    if output_dir.is_absolute() {
        return Ok(output_dir.join("target"));
    }
    Ok(std::env::current_dir()?.join(output_dir).join("target"))
}

/// Result of executing a cargo command.
#[derive(Debug, Clone)]
pub struct ExecutionResult {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<i32>,
}

#[cfg(test)]
mod tests {
    use super::plan_cargo_target_dir;
    use std::path::{Path, PathBuf};

    #[test]
    fn plan_target_matches_relative_output_from_process_cwd() -> Result<(), Box<dyn std::error::Error>> {
        assert_eq!(
            plan_cargo_target_dir(Path::new("generated/demo"))?,
            std::env::current_dir()?.join("generated/demo/target")
        );
        Ok(())
    }

    #[test]
    fn plan_target_matches_absolute_output() -> Result<(), Box<dyn std::error::Error>> {
        assert_eq!(
            plan_cargo_target_dir(Path::new("/private/tmp/generated/demo"))?,
            PathBuf::from("/private/tmp/generated/demo/target")
        );
        Ok(())
    }
}
