//! Passive emitted-file descriptions; native execution is supplied separately.

use std::path::{Path, PathBuf};

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

/// A pure, testable representation of what the compiler will produce.
///
/// This struct contains all the information needed to generate a Rust project without performing any side effects.
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
}

impl CompilationPlan {
    /// Create a new empty compilation plan.
    pub fn new(project_name: impl Into<String>, output_dir: impl AsRef<Path>) -> Self {
        Self {
            project_name: project_name.into(),
            output_dir: output_dir.as_ref().to_path_buf(),
            directories: Vec::new(),
            files: Vec::new(),
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
}

/// Captured outcome of a native command.
#[derive(Debug, Clone)]
pub struct ExecutionResult {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<i32>,
}
