//! ProjectGenerator: high-level API that builds compilation plans and executes them
//!
//! This is the primary struct for generating runnable Rust projects from Incan code.
//! Its responsibilities are split across sibling modules:
//!
//! - **This module** — struct definition, setters, and `generate*()` methods
//! - [`super::cargo_toml`] — `Cargo.toml` rendering (`generate_cargo_toml`, `format_dependency_spec`)
//! - [`super::runner`] — `build()`, `run()`, `run_with_cwd()` and result types

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::manifest::DependencySpec;

/// Project generator for creating runnable Rust projects from Incan code.
pub struct ProjectGenerator {
    /// Output directory for the generated project
    pub(super) output_dir: PathBuf,
    /// Project name
    pub(super) name: String,
    /// Whether this is a binary (true) or library (false)
    pub(super) is_binary: bool,
    /// Whether serde is needed (for Serialize/Deserialize derives)
    pub(super) needs_serde: bool,
    /// Whether tokio is needed (for async runtime)
    pub(super) needs_tokio: bool,
    /// Whether web routing support is needed (stdlib feature)
    pub(super) needs_axum: bool,
    /// Resolved Rust crate dependencies.
    pub(super) dependencies: Vec<DependencySpec>,
    /// Resolved dev-only Rust dependencies.
    pub(super) dev_dependencies: Vec<DependencySpec>,
    /// Whether dev dependencies should be emitted.
    pub(super) include_dev_dependencies: bool,
    /// Optional Cargo.lock payload to materialize.
    pub(super) cargo_lock_payload: Option<String>,
    /// Extra cargo policy flags (e.g. --locked, --frozen).
    pub(super) cargo_policy_flags: Vec<String>,
    /// Optional Rust edition override.
    pub(super) rust_edition: Option<String>,
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
            dependencies: Vec::new(),
            dev_dependencies: Vec::new(),
            include_dev_dependencies: false,
            cargo_lock_payload: None,
            cargo_policy_flags: Vec::new(),
            rust_edition: None,
        }
    }

    /// Enable serde support (for JSON serialization).
    pub fn with_serde(mut self) -> Self {
        self.needs_serde = true;
        self
    }

    /// Set whether serde is needed.
    pub fn set_needs_serde(&mut self, needs: bool) {
        self.needs_serde = needs;
    }

    /// Enable tokio support (for async runtime).
    pub fn with_tokio(mut self) -> Self {
        self.needs_tokio = true;
        self
    }

    /// Set whether tokio is needed.
    pub fn set_needs_tokio(&mut self, needs: bool) {
        self.needs_tokio = needs;
    }

    /// Enable axum support (for web framework).
    pub fn with_axum(mut self) -> Self {
        self.needs_axum = true;
        self
    }

    /// Set whether axum is needed.
    pub fn set_needs_axum(&mut self, needs: bool) {
        self.needs_axum = needs;
    }

    /// Set resolved Rust dependencies.
    pub fn set_dependencies(&mut self, dependencies: Vec<DependencySpec>) {
        self.dependencies = dependencies;
    }

    /// Set resolved dev-only Rust dependencies.
    pub fn set_dev_dependencies(&mut self, dependencies: Vec<DependencySpec>) {
        self.dev_dependencies = dependencies;
    }

    /// Control whether dev dependencies should be emitted.
    pub fn set_include_dev_dependencies(&mut self, include: bool) {
        self.include_dev_dependencies = include;
    }

    /// Provide a Cargo.lock payload to write alongside Cargo.toml.
    pub fn set_cargo_lock_payload(&mut self, payload: Option<String>) {
        self.cargo_lock_payload = payload;
    }

    /// Set additional cargo policy flags (e.g. --locked, --frozen).
    pub fn set_cargo_policy_flags(&mut self, flags: Vec<String>) {
        self.cargo_policy_flags = flags;
    }

    /// Override the Rust edition used in Cargo.toml.
    pub fn set_rust_edition(&mut self, edition: Option<String>) {
        self.rust_edition = edition;
    }

    /// Generate the project structure (single-file mode).
    pub fn generate(&self, rust_code: &str) -> io::Result<()> {
        let src_dir = self.output_dir.join("src");
        fs::create_dir_all(&src_dir)?;

        // Write Cargo.toml
        let cargo_toml = self.generate_cargo_toml()?;
        fs::write(self.output_dir.join("Cargo.toml"), cargo_toml)?;
        self.write_cargo_lock_if_needed()?;

        // Write main source file
        let main_file = if self.is_binary {
            src_dir.join("main.rs")
        } else {
            src_dir.join("lib.rs")
        };
        fs::write(main_file, rust_code)?;

        Ok(())
    }

    /// Generate the project structure with multiple module files (flat).
    ///
    /// # Arguments
    /// * `main_code` - The main.rs code (without mod declarations, they will be prepended)
    /// * `modules` - HashMap of module name to module code (e.g., "models" -> "pub struct User { ... }")
    pub fn generate_multi(&self, main_code: &str, modules: &HashMap<String, String>) -> io::Result<()> {
        let src_dir = self.output_dir.join("src");
        fs::create_dir_all(&src_dir)?;

        // Write Cargo.toml
        let cargo_toml = self.generate_cargo_toml()?;
        fs::write(self.output_dir.join("Cargo.toml"), cargo_toml)?;
        self.write_cargo_lock_if_needed()?;

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

    /// Generate the project structure with nested module directories.
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
        let cargo_toml = self.generate_cargo_toml()?;
        fs::write(self.output_dir.join("Cargo.toml"), cargo_toml)?;
        self.write_cargo_lock_if_needed()?;

        // ---- Collect directory structure and submodules ----
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

        // ---- Create directories and mod.rs files for intermediate directories ----
        for (dir_path, submodules) in &dir_submodules {
            if dir_path.is_empty() {
                // This is the root level — handled by main.rs
                continue;
            }

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

        // ---- Write each module's code file ----
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

        // ---- Build main.rs with crate-level prelude + top-level mod declarations ----
        // Crate attributes (`#![...]`) must appear before any Rust items (including `mod ...;`), so we insert module
        // declarations immediately after the crate-level allow attribute.
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

    /// Write a Cargo.lock file if a lock payload was provided.
    fn write_cargo_lock_if_needed(&self) -> io::Result<()> {
        let Some(payload) = &self.cargo_lock_payload else {
            return Ok(());
        };
        fs::write(self.output_dir.join("Cargo.lock"), payload)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn test_generate_multi_creates_mod_declarations() -> Result<(), Box<dyn std::error::Error>> {
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

        generator.generate_multi(main_code, &modules)?;

        // Check main.rs has mod declarations
        let main_content = fs::read_to_string(temp_dir.join("src/main.rs"))?;
        assert!(main_content.contains("mod models;"));
        assert!(main_content.contains("mod utils;"));
        assert!(main_content.contains("fn main()"));

        // Check module files exist
        assert!(temp_dir.join("src/models.rs").exists());
        assert!(temp_dir.join("src/utils.rs").exists());

        // Check module content
        let models_content = fs::read_to_string(temp_dir.join("src/models.rs"))?;
        assert!(models_content.contains("pub struct User"));

        let utils_content = fs::read_to_string(temp_dir.join("src/utils.rs"))?;
        assert!(utils_content.contains("pub fn greet"));

        // Cleanup
        let _ = fs::remove_dir_all(&temp_dir);
        Ok(())
    }

    #[test]
    fn test_generate_multi_empty_modules() -> Result<(), Box<dyn std::error::Error>> {
        let temp_dir = std::env::temp_dir().join("incan_test_multi_empty");
        let _ = fs::remove_dir_all(&temp_dir);

        let generator = ProjectGenerator::new(&temp_dir, "test_empty", true);
        let modules = HashMap::new();
        let main_code = "fn main() {}";

        generator.generate_multi(main_code, &modules)?;

        let main_content = fs::read_to_string(temp_dir.join("src/main.rs"))?;
        // Should just be the main code, no mod declarations
        assert_eq!(main_content, "fn main() {}");

        let _ = fs::remove_dir_all(&temp_dir);
        Ok(())
    }
}
