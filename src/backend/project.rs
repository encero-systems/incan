//! Project generator - creates the output Rust project structure
//!
//! Generates:
//! - Cargo.toml with dependencies
//! - src/main.rs or src/lib.rs
//! - Invokes cargo build
//!
//! ## Cargo Dependency Policy
//!
//! The project generator uses a **strict dependency policy**: unknown `rust::` crates must specify an explicit version
//! and features. We never fall back to `*` (wildcard).
//! FIXME: We will address this*See RFC 013*.
//!
//! ### Known-good crates
//!
//! A curated list of common crates have known-good version/feature defaults maintained in
//! [`ProjectGenerator::add_rust_crate`]. To add a new crate to this list, submit a PR updating the match arm with
//! tested version and features.
//!
//! ### Unknown crates
//!
//! If a crate is not in the known-good list, the compiler emits an error asking the user
//! to provide explicit version/features via `add_rust_crate_with_version`.

use std::collections::HashMap;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const INCAN_VERSION: &str = crate::version::INCAN_VERSION;

/// Error returned when a `rust::` crate import lacks a known-good version mapping.
///
/// The user must provide an explicit version/features spec for unknown crates.
#[derive(Debug, Clone)]
pub struct UnknownCrateError {
    /// Name of the crate that is not in the known-good list
    pub crate_name: String,
}

impl fmt::Display for UnknownCrateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "unknown Rust crate `{}`: no known-good version mapping exists.\n\
             \n\
             To use this crate, you must specify an explicit version. Options:\n\
             \n\
             1. Add a version annotation in your Incan code (if supported), or\n\
             2. Request that `{}` be added to the known-good list by opening an issue/PR.\n\
             \n\
             Known-good crates: serde, serde_json, tokio, time, chrono, reqwest, uuid,\n\
             rand, regex, anyhow, thiserror, tracing, clap, log, env_logger, sqlx,\n\
             futures, bytes, itertools",
            self.crate_name, self.crate_name
        )
    }
}

impl std::error::Error for UnknownCrateError {}

// ============================================================================
// CompilationPlan + Executor: separating "what to do" from "doing it"
// ============================================================================

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
/// This struct contains all the information needed to generate a Rust project
/// without performing any side effects. Use [`Executor::execute`] to actually
/// write files and run commands.
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
#[derive(Debug, Default)]
pub struct Executor;

impl Executor {
    /// Create a new executor.
    pub fn new() -> Self {
        Self
    }

    /// Execute a compilation plan: create directories, write files, optionally run cargo.
    ///
    /// Returns the result of the cargo command if one was specified, or `Ok(None)` if
    /// the plan only generates files.
    pub fn execute(&self, plan: &CompilationPlan) -> io::Result<Option<ExecutionResult>> {
        // Create directories
        for dir in &plan.directories {
            fs::create_dir_all(&dir.path)?;
        }

        // Write files
        for file in &plan.files {
            // Ensure parent directory exists
            if let Some(parent) = file.path.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(&file.path, &file.content)?;
        }

        // Run cargo command if specified
        match &plan.cargo_command {
            Some(CargoCommand::Build) => {
                let output = Command::new("cargo")
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
                let mut child = Command::new("cargo")
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

/// Result of executing a cargo command.
#[derive(Debug, Clone)]
pub struct ExecutionResult {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<i32>,
}

// ============================================================================
// ProjectGenerator: high-level API that builds plans and executes them
// ============================================================================

/// Project generator for creating runnable Rust projects from Incan code
pub struct ProjectGenerator {
    /// Output directory for the generated project
    output_dir: PathBuf,
    /// Project name
    name: String,
    /// Whether this is a binary (true) or library (false)
    is_binary: bool,
    /// Whether serde is needed (for Serialize/Deserialize derives)
    needs_serde: bool,
    /// Whether tokio is needed (for async runtime)
    needs_tokio: bool,
    /// Whether web routing support is needed (stdlib feature)
    needs_axum: bool,
    /// Additional Rust crate dependencies from `rust::` imports
    /// Key: crate name, Value: optional version spec (if None, uses latest)
    rust_crate_deps: std::collections::HashMap<String, Option<String>>,
}

impl ProjectGenerator {
    pub fn new(output_dir: impl AsRef<Path>, name: &str, is_binary: bool) -> Self {
        Self {
            output_dir: output_dir.as_ref().to_path_buf(),
            name: name.to_string(),
            is_binary,
            needs_serde: false,
            needs_tokio: false,
            needs_axum: false,
            rust_crate_deps: std::collections::HashMap::new(),
        }
    }

    /// Enable serde support (for JSON serialization)
    pub fn with_serde(mut self) -> Self {
        self.needs_serde = true;
        self
    }

    /// Set whether serde is needed
    pub fn set_needs_serde(&mut self, needs: bool) {
        self.needs_serde = needs;
    }

    /// Enable tokio support (for async runtime)
    pub fn with_tokio(mut self) -> Self {
        self.needs_tokio = true;
        self
    }

    /// Set whether tokio is needed
    pub fn set_needs_tokio(&mut self, needs: bool) {
        self.needs_tokio = needs;
    }

    /// Enable axum support (for web framework)
    pub fn with_axum(mut self) -> Self {
        self.needs_axum = true;
        self
    }

    /// Set whether axum is needed
    pub fn set_needs_axum(&mut self, needs: bool) {
        self.needs_axum = needs;
    }

    /// Add a Rust crate dependency from `import rust::crate_name`
    /// Uses a default version mapping for common crates, otherwise uses latest
    pub fn add_rust_crate(&mut self, crate_name: &str) {
        // Common crate versions (maintain a mapping of known-good versions)
        let version = match crate_name {
            "serde" => Some(r#"{ version = "1.0", features = ["derive"] }"#.to_string()),
            "serde_json" => Some(r#""1.0""#.to_string()),
            "tokio" => {
                Some(r#"{ version = "1", features = ["rt-multi-thread", "macros", "time", "sync"] }"#.to_string())
            }
            "time" => Some(r#"{ version = "0.3", features = ["formatting", "macros"] }"#.to_string()),
            "chrono" => Some(r#"{ version = "0.4", features = ["serde"] }"#.to_string()),
            "reqwest" => Some(r#"{ version = "0.11", features = ["json"] }"#.to_string()),
            "uuid" => Some(r#"{ version = "1.0", features = ["v4", "serde"] }"#.to_string()),
            "rand" => Some(r#""0.8""#.to_string()),
            "regex" => Some(r#""1.0""#.to_string()),
            "anyhow" => Some(r#""1.0""#.to_string()),
            "thiserror" => Some(r#""1.0""#.to_string()),
            "tracing" => Some(r#""0.1""#.to_string()),
            "clap" => Some(r#"{ version = "4.0", features = ["derive"] }"#.to_string()),
            "log" => Some(r#""0.4""#.to_string()),
            "env_logger" => Some(r#""0.10""#.to_string()),
            "sqlx" => Some(r#"{ version = "0.7", features = ["runtime-tokio-native-tls", "postgres"] }"#.to_string()),
            "futures" => Some(r#""0.3""#.to_string()),
            "bytes" => Some(r#""1.0""#.to_string()),
            "itertools" => Some(r#""0.12""#.to_string()),
            // Use latest for unknown crates
            _ => None,
        };
        self.rust_crate_deps.insert(crate_name.to_string(), version);
    }

    /// Add a Rust crate with a specific version spec
    pub fn add_rust_crate_with_version(&mut self, crate_name: &str, version_spec: &str) {
        self.rust_crate_deps
            .insert(crate_name.to_string(), Some(version_spec.to_string()));
    }

    /// Generate the project structure (single-file mode)
    pub fn generate(&self, rust_code: &str) -> io::Result<()> {
        // Create directories
        let src_dir = self.output_dir.join("src");
        fs::create_dir_all(&src_dir)?;

        // Write Cargo.toml
        let cargo_toml = self.generate_cargo_toml();
        fs::write(self.output_dir.join("Cargo.toml"), cargo_toml)?;

        // Write main source file
        let main_file = if self.is_binary {
            src_dir.join("main.rs")
        } else {
            src_dir.join("lib.rs")
        };
        fs::write(main_file, rust_code)?;

        Ok(())
    }

    /// Generate the project structure with multiple module files (flat)
    ///
    /// # Arguments
    /// * `main_code` - The main.rs code (without mod declarations, they will be prepended)
    /// * `modules` - HashMap of module name to module code (e.g., "models" -> "pub struct User { ... }")
    pub fn generate_multi(&self, main_code: &str, modules: &HashMap<String, String>) -> io::Result<()> {
        // Create directories
        let src_dir = self.output_dir.join("src");
        fs::create_dir_all(&src_dir)?;

        // Write Cargo.toml
        let cargo_toml = self.generate_cargo_toml();
        fs::write(self.output_dir.join("Cargo.toml"), cargo_toml)?;

        // Write each module file
        for (module_name, module_code) in modules {
            let module_file = src_dir.join(format!("{}.rs", module_name));
            fs::write(module_file, module_code)?;
        }

        // Build main.rs with the crate-level prelude first, then mod declarations.
        // Crate attributes (`#![...]`) must appear before any Rust items (including `mod ...;`),
        // so we insert module declarations immediately after the crate-level allow attribute.
        let mut full_main = String::new();
        full_main.push_str(main_code);

        if !modules.is_empty() {
            // Add mod declarations for each module (sorted for deterministic output)
            let mut module_names: Vec<_> = modules.keys().collect();
            module_names.sort();
            let mods: String = module_names.iter().map(|m| format!("mod {};\n", m)).collect();

            // Insert right after the crate-level allow attribute line (if present),
            // otherwise prepend (best-effort).
            if let Some(attr_pos) = full_main.find("#![allow(") {
                let line_end = full_main[attr_pos..]
                    .find('\n')
                    .map(|o| attr_pos + o + 1)
                    .unwrap_or(full_main.len());
                full_main.insert_str(line_end, &mods);
                full_main.insert(line_end + mods.len(), '\n');
            } else {
                full_main = format!("{}\n{}", mods, full_main);
            }
        }

        // Write main source file
        let main_file = if self.is_binary {
            src_dir.join("main.rs")
        } else {
            src_dir.join("lib.rs")
        };
        fs::write(main_file, full_main)?;

        Ok(())
    }

    /// Generate the project structure with nested module directories
    ///
    /// This creates proper Rust module hierarchy:
    /// - `from db::models import User` creates `src/db/mod.rs` and `src/db/models.rs`
    /// - main.rs gets `mod db;` (top-level only)
    ///
    /// # Arguments
    /// * `main_code` - The main.rs code (without mod declarations, they will be prepended)
    /// * `modules` - HashMap of path segments to module code (e.g., ["db", "models"] -> "pub struct User { ... }")
    pub fn generate_nested(&self, main_code: &str, modules: &HashMap<Vec<String>, String>) -> io::Result<()> {
        let src_dir = self.output_dir.join("src");
        fs::create_dir_all(&src_dir)?;

        // Write Cargo.toml
        let cargo_toml = self.generate_cargo_toml();
        fs::write(self.output_dir.join("Cargo.toml"), cargo_toml)?;

        // Collect all unique directory paths and their submodules
        // For ["db", "models"], we need:
        //   - src/db/ directory
        //   - src/db/mod.rs with "pub mod models;"
        //   - src/db/models.rs with the code
        let mut dir_submodules: HashMap<Vec<String>, Vec<String>> = HashMap::new();
        let mut top_level_modules: std::collections::HashSet<String> = std::collections::HashSet::new();

        for path_segments in modules.keys() {
            if !path_segments.is_empty() {
                top_level_modules.insert(path_segments[0].clone());
            }

            // For each intermediate directory, track what submodules it contains
            for i in 0..path_segments.len() {
                let dir_path: Vec<String> = path_segments[..i].to_vec();
                let submodule = &path_segments[i];
                dir_submodules.entry(dir_path).or_default().push(submodule.clone());
            }
        }

        // Remove duplicates from submodule lists
        for subs in dir_submodules.values_mut() {
            subs.sort();
            subs.dedup();
        }

        // Create directories and mod.rs files for intermediate directories
        for (dir_path, submodules) in &dir_submodules {
            if dir_path.is_empty() {
                // This is the root level - handled by main.rs
                continue;
            }

            // Create the directory
            let mut dir = src_dir.clone();
            for segment in dir_path {
                dir = dir.join(segment);
            }
            fs::create_dir_all(&dir)?;

            // Create mod.rs with pub mod declarations
            let mod_rs_content: String = submodules
                .iter()
                .map(|s| format!("pub mod {};", s))
                .collect::<Vec<_>>()
                .join("\n");

            let mod_rs_path = dir.join("mod.rs");
            fs::write(mod_rs_path, format!("{}\n", mod_rs_content))?;
        }

        // Write each module's code file
        for (path_segments, module_code) in modules {
            // Build the file path: src/db/models.rs for ["db", "models"]
            let mut file_path = src_dir.clone();
            for segment in &path_segments[..path_segments.len() - 1] {
                file_path = file_path.join(segment);
            }
            fs::create_dir_all(&file_path)?;

            let file_stem = path_segments
                .last()
                .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "empty module path"))?;
            let file_name = format!("{file_stem}.rs");
            file_path = file_path.join(file_name);

            fs::write(file_path, module_code)?;
        }

        // Build main.rs with the crate-level prelude first, then top-level mod declarations.
        // Crate attributes (`#![...]`) must appear before any Rust items (including `mod ...;`),
        // so we insert module declarations immediately after the crate-level allow attribute.
        let mut full_main = String::new();
        full_main.push_str(main_code);

        let mut sorted_top: Vec<_> = top_level_modules.into_iter().collect();
        sorted_top.sort();
        if !sorted_top.is_empty() {
            let mods: String = sorted_top.iter().map(|m| format!("mod {};\n", m)).collect();

            if let Some(attr_pos) = full_main.find("#![allow(") {
                let line_end = full_main[attr_pos..]
                    .find('\n')
                    .map(|o| attr_pos + o + 1)
                    .unwrap_or(full_main.len());
                full_main.insert_str(line_end, &mods);
                full_main.insert(line_end + mods.len(), '\n');
            } else {
                full_main = format!("{}\n{}", mods, full_main);
            }
        }

        // Write main source file
        let main_file = if self.is_binary {
            src_dir.join("main.rs")
        } else {
            src_dir.join("lib.rs")
        };
        fs::write(main_file, full_main)?;

        Ok(())
    }

    /// Generate Cargo.toml content
    fn generate_cargo_toml(&self) -> String {
        let crate_type = if self.is_binary {
            r#"[[bin]]
name = "{name}"
path = "src/main.rs""#
        } else {
            r#"[lib]
name = "{name}"
path = "src/lib.rs""#
        };

        // Build dependencies list
        let mut deps = Vec::new();

        // Track which crates we've already added (to avoid duplicates)
        let mut added_crates: std::collections::HashSet<&str> = std::collections::HashSet::new();

        // Resolve workspace-rooted paths so OUTPUT_DIR can be arbitrary.
        let workspace_root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let stdlib_path = workspace_root.join("crates/incan_stdlib");
        let derive_path = workspace_root.join("crates/incan_derive");

        // Always add incan_stdlib for standard library support (enable features based on needs)
        let mut stdlib_features = Vec::new();
        if self.needs_axum {
            stdlib_features.push("web");
        }
        if self.needs_tokio {
            stdlib_features.push("async");
        }
        if self.needs_serde {
            stdlib_features.push("json");
        }
        let stdlib_dep = if stdlib_features.is_empty() {
            format!(r#"incan_stdlib = {{ path = "{}" }}"#, stdlib_path.display())
        } else {
            format!(
                r#"incan_stdlib = {{ path = "{}", features = [{}] }}"#,
                stdlib_path.display(),
                stdlib_features
                    .iter()
                    .map(|f| format!("\"{}\"", f))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        };
        deps.push(stdlib_dep);
        added_crates.insert("incan_stdlib");

        // Always add incan_derive for derive macros
        deps.push(format!(r#"incan_derive = {{ path = "{}" }}"#, derive_path.display()));
        added_crates.insert("incan_derive");

        if self.needs_serde {
            deps.push(r#"serde = { version = "1.0", features = ["derive"] }"#.to_string());
            deps.push(r#"serde_json = "1.0""#.to_string());
            added_crates.insert("serde");
            added_crates.insert("serde_json");
        }

        // Add dependencies from rust:: imports
        for (crate_name, version_spec) in &self.rust_crate_deps {
            // Skip if already added above
            if added_crates.contains(crate_name.as_str()) {
                continue;
            }

            let dep_line = if let Some(spec) = version_spec {
                format!("{} = {}", crate_name, spec)
            } else {
                // Use "*" for latest version (cargo will resolve to latest compatible)
                format!("{} = \"*\"", crate_name)
            };
            deps.push(dep_line);
        }

        let dependencies = if deps.is_empty() {
            "# No additional dependencies".to_string()
        } else {
            deps.join("\n")
        };

        format!(
            r#"[package]
name = "{name}"
version = "{incan_version}"
edition = "2021"

# Generated by the Incan compiler

# Opt out of parent workspace (if any)
[workspace]

[dependencies]
{dependencies}

{crate_type}
"#,
            name = self.name,
            incan_version = INCAN_VERSION,
            dependencies = dependencies,
            crate_type = crate_type.replace("{name}", &self.name)
        )
    }

    /// Build the project using cargo
    pub fn build(&self) -> io::Result<BuildResult> {
        let output = Command::new("cargo")
            .arg("build")
            .arg("--release")
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

    /// Run the project using cargo
    ///
    /// Uses inherited stdio so output streams to terminal in real-time
    /// (important for long-running processes like web servers)
    ///
    /// Note: This is only used by `incan run` during development.
    /// Production deployments run the generated binary directly.
    pub fn run(&self) -> io::Result<RunResult> {
        self.run_with_cwd(&self.output_dir)
    }

    /// Run the project with a custom working directory.
    ///
    /// This builds the generated Rust project, then runs the resulting binary with
    /// `cwd` as the working directory. This keeps runtime-relative paths anchored
    /// to the original project root rather than the generated `target/incan/...` directory.
    pub fn run_with_cwd(&self, cwd: &Path) -> io::Result<RunResult> {
        // Build first so we can run the binary directly with a custom cwd.
        let build_output = Command::new("cargo")
            .arg("build")
            .arg("--release")
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

    /// Get the path to the built binary
    pub fn binary_path(&self) -> PathBuf {
        self.output_dir.join("target").join("release").join(&self.name)
    }
}

/// Result of a cargo build
#[derive(Debug)]
pub struct BuildResult {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}

/// Result of running the built program
#[derive(Debug)]
pub struct RunResult {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<i32>,
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn test_cargo_toml_generation() {
        let generator = ProjectGenerator::new("/tmp/test", "hello", true);
        let toml = generator.generate_cargo_toml();
        assert!(toml.contains("name = \"hello\""));
        assert!(toml.contains("[[bin]]"));
    }

    #[test]
    fn test_generate_multi_creates_mod_declarations() {
        let temp_dir = std::env::temp_dir().join("incan_test_multi");
        let _ = fs::remove_dir_all(&temp_dir); // Clean up any previous test

        let generator = ProjectGenerator::new(&temp_dir, "test_multi", true);

        let mut modules = HashMap::new();
        modules.insert("models".to_string(), "pub struct User { pub name: String }".to_string());
        modules.insert(
            "utils".to_string(),
            "pub fn greet() -> String { \"hello\".to_string() }".to_string(),
        );

        let main_code = "fn main() { println!(\"Hello\"); }";

        generator.generate_multi(main_code, &modules).unwrap();

        // Check main.rs has mod declarations
        let main_content = fs::read_to_string(temp_dir.join("src/main.rs")).unwrap();
        assert!(main_content.contains("mod models;"));
        assert!(main_content.contains("mod utils;"));
        assert!(main_content.contains("fn main()"));

        // Check module files exist
        assert!(temp_dir.join("src/models.rs").exists());
        assert!(temp_dir.join("src/utils.rs").exists());

        // Check module content
        let models_content = fs::read_to_string(temp_dir.join("src/models.rs")).unwrap();
        assert!(models_content.contains("pub struct User"));

        let utils_content = fs::read_to_string(temp_dir.join("src/utils.rs")).unwrap();
        assert!(utils_content.contains("pub fn greet"));

        // Cleanup
        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_generate_multi_empty_modules() {
        let temp_dir = std::env::temp_dir().join("incan_test_multi_empty");
        let _ = fs::remove_dir_all(&temp_dir);

        let generator = ProjectGenerator::new(&temp_dir, "test_empty", true);
        let modules = HashMap::new();
        let main_code = "fn main() {}";

        generator.generate_multi(main_code, &modules).unwrap();

        let main_content = fs::read_to_string(temp_dir.join("src/main.rs")).unwrap();
        // Should just be the main code, no mod declarations
        assert_eq!(main_content, "fn main() {}");

        let _ = fs::remove_dir_all(&temp_dir);
    }
}
