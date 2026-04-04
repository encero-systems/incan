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
use incan_core::lang::rust_keywords;

// ============================================================================
// RFC 023: Stdlib module naming
// ============================================================================

/// Check if a module path is a stdlib module (starts with "std").
fn is_stdlib_path(path: &[String]) -> bool {
    path.first().is_some_and(|s| s == "std")
}

/// Transform stdlib module path to use `__incan_std` prefix to avoid shadowing Rust's `std`.
///
/// ## Examples
/// - `["std", "testing"]` → `["__incan_std", "testing"]`
/// - `["db", "models"]` → `["db", "models"]` (unchanged)
///
/// RFC 023: Generated stdlib modules are emitted under `__incan_std` to prevent collision with Rust's `std` crate.
/// This transformation is applied consistently across module declarations, `use` paths, and directory structures.
fn transform_stdlib_path(path: &[String]) -> Vec<String> {
    if is_stdlib_path(path) {
        let mut transformed = vec!["__incan_std".to_string()];
        transformed.extend_from_slice(&path[1..]);
        transformed
    } else {
        path.to_vec()
    }
}

/// Project generator for creating runnable Rust projects from Incan code.
pub struct ProjectGenerator {
    /// Output directory for the generated project
    pub(super) output_dir: PathBuf,
    /// Project name
    pub(super) name: String,
    /// Whether this is a binary (true) or library (false)
    pub(super) is_binary: bool,
    /// Enabled stdlib feature flags for the generated project (for example `json`, `async`, `web`).
    pub(super) stdlib_features: Vec<String>,
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
            stdlib_features: Vec::new(),
            dependencies: Vec::new(),
            dev_dependencies: Vec::new(),
            include_dev_dependencies: false,
            cargo_lock_payload: None,
            cargo_policy_flags: Vec::new(),
            rust_edition: None,
        }
    }

    /// Set the stdlib feature flags required by this generated project.
    pub fn set_stdlib_features(&mut self, features: Vec<String>) {
        let mut normalized: Vec<String> = features
            .into_iter()
            .map(|feature| feature.trim().to_string())
            .filter(|feature| !feature.is_empty())
            .collect();
        normalized.sort();
        normalized.dedup();
        self.stdlib_features = normalized;
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

    /// Ensure the generated `src/` directory exists.
    fn ensure_generated_src_dir(&self) -> io::Result<PathBuf> {
        let src_dir = self.output_dir.join("src");
        fs::create_dir_all(&src_dir)?;
        Ok(src_dir)
    }

    /// Remove a conflicting module artifact if it exists.
    ///
    /// This deliberately removes only the generated Rust file-or-directory path that conflicts with the layout we are
    /// about to emit, rather than deleting the entire `src/` tree.
    fn remove_conflicting_module_artifact(path: &Path) -> io::Result<()> {
        if path.is_dir() {
            fs::remove_dir_all(path)?;
        } else if path.exists() {
            fs::remove_file(path)?;
        }
        Ok(())
    }

    /// Generate the project structure (single-file mode).
    pub fn generate(&self, rust_code: &str) -> io::Result<()> {
        let src_dir = self.ensure_generated_src_dir()?;

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
        let src_dir = self.ensure_generated_src_dir()?;

        for module_name in modules.keys() {
            Self::remove_conflicting_module_artifact(&src_dir.join(module_name))?;
        }

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
            let mods: String = module_names
                .iter()
                .map(|m| format!("mod {};\n", rust_keywords::escape_keyword(m)))
                .collect();

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
    /// RFC 023: Stdlib modules (`std.*`) are transformed to `__incan_std.*` to avoid shadowing Rust's `std` crate.
    ///
    /// # Arguments
    /// * `main_code` - The main.rs code (without mod declarations, they will be prepended)
    /// * `modules` - HashMap of path segments to module code (e.g., ["db", "models"] -> "pub struct User { ... }")
    pub fn generate_nested(&self, main_code: &str, modules: &HashMap<Vec<String>, String>) -> io::Result<()> {
        let src_dir = self.ensure_generated_src_dir()?;

        // Write Cargo.toml
        let cargo_toml = self.generate_cargo_toml()?;
        fs::write(self.output_dir.join("Cargo.toml"), cargo_toml)?;
        self.write_cargo_lock_if_needed()?;

        // ---- RFC 023: Transform stdlib paths to __incan_std ----
        let mut transformed_modules: HashMap<Vec<String>, String> = HashMap::new();
        for (path, code) in modules {
            let transformed_path = transform_stdlib_path(path);
            transformed_modules.insert(transformed_path, code.clone());
        }

        // ---- Collect directory structure and submodules ----
        // For ["db", "models"], we need:
        //   - src/db/ directory
        //   - src/db/mod.rs with "pub mod models;"
        //   - src/db/models.rs with the code
        let mut dir_submodules: HashMap<Vec<String>, Vec<String>> = HashMap::new();
        let mut top_level_modules: std::collections::HashSet<String> = std::collections::HashSet::new();

        for path_segments in transformed_modules.keys() {
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

        // ---- Separate modules with submodules from leaf modules ----
        // Modules that have submodules need their code in mod.rs, not a separate .rs file
        let modules_with_submodules: std::collections::HashSet<Vec<String>> =
            dir_submodules.keys().filter(|path| !path.is_empty()).cloned().collect();

        // Remove only the stale Rust paths that conflict with the layout we are about to generate.
        for path_segments in transformed_modules.keys() {
            let mut module_path = src_dir.clone();
            for segment in path_segments {
                module_path = module_path.join(segment);
            }

            if modules_with_submodules.contains(path_segments) {
                Self::remove_conflicting_module_artifact(&module_path.with_extension("rs"))?;
            } else {
                Self::remove_conflicting_module_artifact(&module_path)?;
            }
        }

        // ---- Create directories and mod.rs files for modules with submodules ----
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

            // Build mod.rs content: submodule declarations + module code (if exists)
            let mut mod_rs_content = String::new();

            // Add submodule declarations
            let submod_declarations: String = submodules
                .iter()
                .map(|s| format!("pub mod {};", rust_keywords::escape_keyword(s)))
                .collect::<Vec<_>>()
                .join("\n");

            if !submod_declarations.is_empty() {
                mod_rs_content.push_str(&submod_declarations);
                mod_rs_content.push('\n');
            }

            // If this module itself has code, append it
            if let Some(module_code) = transformed_modules.get(dir_path) {
                if !mod_rs_content.is_empty() {
                    mod_rs_content.push('\n');
                }
                mod_rs_content.push_str(module_code);
            }

            let mod_rs_path = dir.join("mod.rs");
            fs::write(mod_rs_path, mod_rs_content)?;
        }

        // ---- Write leaf module code files (modules without submodules) ----
        for (path_segments, module_code) in &transformed_modules {
            // Skip modules that have submodules (already written to mod.rs above)
            if modules_with_submodules.contains(path_segments) {
                continue;
            }

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
            let mods: String = sorted_top
                .iter()
                .map(|m| format!("mod {};\n", rust_keywords::escape_keyword(m)))
                .collect();

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
    fn test_is_stdlib_path() {
        assert!(is_stdlib_path(&["std".to_string(), "testing".to_string()]));
        assert!(is_stdlib_path(&["std".to_string()]));
        assert!(!is_stdlib_path(&["db".to_string(), "models".to_string()]));
        assert!(!is_stdlib_path(&[]));
    }

    #[test]
    fn test_transform_stdlib_path() {
        // Stdlib paths get transformed
        assert_eq!(
            transform_stdlib_path(&["std".to_string(), "testing".to_string()]),
            vec!["__incan_std".to_string(), "testing".to_string()]
        );
        assert_eq!(
            transform_stdlib_path(&["std".to_string(), "derives".to_string(), "comparison".to_string()]),
            vec![
                "__incan_std".to_string(),
                "derives".to_string(),
                "comparison".to_string()
            ]
        );

        // Non-stdlib paths are unchanged
        assert_eq!(
            transform_stdlib_path(&["db".to_string(), "models".to_string()]),
            vec!["db".to_string(), "models".to_string()]
        );
        assert_eq!(transform_stdlib_path(&["api".to_string()]), vec!["api".to_string()]);
    }

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
    fn test_generate_nested_transforms_stdlib_to_incan_std() -> Result<(), Box<dyn std::error::Error>> {
        let temp_dir = std::env::temp_dir().join("incan_test_stdlib_transform");
        let _ = fs::remove_dir_all(&temp_dir);

        let generator = ProjectGenerator::new(&temp_dir, "test_stdlib", true);

        let mut modules = HashMap::new();
        // Add a stdlib module (std::testing)
        modules.insert(
            vec!["std".to_string(), "testing".to_string()],
            "pub fn assert(condition: bool) { if !condition { panic!() } }".to_string(),
        );
        // Add a regular user module
        modules.insert(
            vec!["db".to_string(), "models".to_string()],
            "pub struct User { pub name: String }".to_string(),
        );

        let main_code = "fn main() { println!(\"Hello\"); }";

        generator.generate_nested(main_code, &modules)?;

        // Check main.rs has transformed stdlib module declaration
        let main_content = fs::read_to_string(temp_dir.join("src/main.rs"))?;
        assert!(
            main_content.contains("mod __incan_std;"),
            "main.rs should declare '__incan_std' module"
        );
        assert!(main_content.contains("mod db;"), "main.rs should declare 'db' module");
        assert!(
            !main_content.contains("mod std;"),
            "main.rs should NOT have 'mod std;' (would shadow Rust std)"
        );

        // Check __incan_std directory exists (transformed from std)
        assert!(
            temp_dir.join("src/__incan_std").exists(),
            "__incan_std directory should exist"
        );
        assert!(
            temp_dir.join("src/__incan_std/mod.rs").exists(),
            "__incan_std/mod.rs should exist"
        );
        assert!(
            temp_dir.join("src/__incan_std/testing.rs").exists(),
            "__incan_std/testing.rs should exist"
        );

        // Check __incan_std/mod.rs has correct submodule declaration
        let incan_std_mod = fs::read_to_string(temp_dir.join("src/__incan_std/mod.rs"))?;
        assert!(incan_std_mod.contains("pub mod testing;"));

        // Check testing module content is preserved
        let testing_content = fs::read_to_string(temp_dir.join("src/__incan_std/testing.rs"))?;
        assert!(testing_content.contains("pub fn assert"));

        // Check regular user module is unchanged
        assert!(temp_dir.join("src/db").exists());
        assert!(temp_dir.join("src/db/models.rs").exists());

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

    #[test]
    fn test_generate_nested_removes_stale_flat_module_file() -> Result<(), Box<dyn std::error::Error>> {
        let temp_dir = std::env::temp_dir().join("incan_test_nested_cleanup");
        let _ = fs::remove_dir_all(&temp_dir);

        let generator = ProjectGenerator::new(&temp_dir, "test_cleanup", false);

        let mut flat_modules = HashMap::new();
        flat_modules.insert("dataset".to_string(), "pub trait DataSet<T> {}".to_string());
        generator.generate_multi("pub fn root() {}", &flat_modules)?;
        assert!(
            temp_dir.join("src/dataset.rs").exists(),
            "flat module should exist after flat generation"
        );

        let mut nested_modules = HashMap::new();
        nested_modules.insert(vec!["dataset".to_string()], "pub trait DataSet<T> {}".to_string());
        nested_modules.insert(
            vec!["dataset".to_string(), "ops".to_string()],
            "pub fn filter_ds<T>(ds: T) -> T { ds }".to_string(),
        );
        generator.generate_nested("pub fn root() {}", &nested_modules)?;

        assert!(
            !temp_dir.join("src/dataset.rs").exists(),
            "stale flat module file should be removed before nested generation"
        );
        assert!(
            temp_dir.join("src/dataset/mod.rs").exists(),
            "nested module entrypoint should exist"
        );
        assert!(
            temp_dir.join("src/dataset/ops.rs").exists(),
            "nested leaf module should exist"
        );

        let _ = fs::remove_dir_all(&temp_dir);
        Ok(())
    }

    #[test]
    fn test_generate_nested_preserves_unrelated_src_files() -> Result<(), Box<dyn std::error::Error>> {
        let temp_dir = std::env::temp_dir().join("incan_test_nested_preserve_unrelated");
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(temp_dir.join("src"))?;
        fs::write(temp_dir.join("src").join("manual.rs"), "pub fn keep_me() {}\n")?;

        let generator = ProjectGenerator::new(&temp_dir, "test_cleanup", false);
        let mut nested_modules = HashMap::new();
        nested_modules.insert(vec!["dataset".to_string()], "pub trait DataSet<T> {}".to_string());
        nested_modules.insert(
            vec!["dataset".to_string(), "ops".to_string()],
            "pub fn filter_ds<T>(ds: T) -> T { ds }".to_string(),
        );

        generator.generate_nested("pub fn root() {}", &nested_modules)?;

        assert!(
            temp_dir.join("src/manual.rs").exists(),
            "unrelated source files should not be removed by nested generation"
        );

        let _ = fs::remove_dir_all(&temp_dir);
        Ok(())
    }
}
