//! ProjectGenerator: high-level API that builds compilation plans and executes them
//!
//! This is the primary struct for generating runnable Rust projects from Incan code.
//! Its responsibilities are split across sibling modules:
//!
//! - **This module** — struct definition, setters, and `generate*()` methods
//! - [`super::cargo_toml`] — `Cargo.toml` rendering (`generate_cargo_toml`, `format_dependency_spec`)
//! - [`super::runner`] — `build()`, `run()`, `run_with_cwd()` and result types

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::compiled_sdk::CompiledSdkModules;
use crate::manifest::DependencySpec;
use crate::provider::ProviderPlan;
use incan_core::lang::{rust_keywords, stdlib};
use sha2::{Digest as _, Sha256};

const MOD_INSERT_MARKER: &str = "// __INCAN_INSERT_MODS__";
pub(crate) const GENERATED_CARGO_TARGET_DIR_ENV: &str = "INCAN_GENERATED_CARGO_TARGET_DIR";

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

/// Return whether this process is compiling an SDK provider into its own library artifact.
///
/// The generated artifact exposes source modules directly (`crate::fs`, `crate::traits`, …), while existing compiler
/// bridges address those same modules through `crate::__incan_std`. Keep that compatibility namespace confined to the
/// artifact build so ordinary user libraries do not acquire a synthetic module.
pub(super) fn is_sdk_provider_build() -> bool {
    std::env::var_os("INCAN_INTERNAL_SDK_PROVIDER_BUILD").is_some()
}

/// Project generator for creating runnable Rust projects from Incan code.
pub struct ProjectGenerator {
    /// Output directory for the generated project
    pub(super) output_dir: PathBuf,
    /// Project name
    pub(super) name: String,
    /// Optional Cargo package name when it should differ from the generated target name.
    pub(super) package_name: Option<String>,
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
    /// Optional shared Cargo target directory for generated Rust projects.
    pub(super) cargo_target_dir_override: Option<PathBuf>,
    /// Optional Rust edition override.
    pub(super) rust_edition: Option<String>,
    /// Profile used when building the generated crate for `incan run`.
    pub(super) run_profile: RunProfile,
    /// Modules supplied by linked compiled SDK providers.
    pub(super) compiled_sdk_modules: CompiledSdkModules,
    /// Top-level `std.*` modules grouped by the generated Rust crate that supplies each compiled SDK provider.
    pub(super) compiled_provider_modules: BTreeMap<String, BTreeSet<String>>,
}

/// Cargo profile used for `incan run`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunProfile {
    /// `cargo build` (debug profile).
    Debug,
    /// `cargo build --release` (optimized profile).
    Release,
}

impl ProjectGenerator {
    /// Create a project generator for an Incan build target.
    pub fn new(output_dir: impl AsRef<Path>, name: &str, is_binary: bool) -> Self {
        Self {
            output_dir: output_dir.as_ref().to_path_buf(),
            name: name.to_string(),
            package_name: None,
            is_binary,
            stdlib_features: Vec::new(),
            dependencies: Vec::new(),
            dev_dependencies: Vec::new(),
            include_dev_dependencies: false,
            cargo_lock_payload: None,
            cargo_policy_flags: Vec::new(),
            cargo_target_dir_override: None,
            rust_edition: None,
            run_profile: RunProfile::Debug,
            compiled_sdk_modules: CompiledSdkModules::default(),
            compiled_provider_modules: BTreeMap::new(),
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

    /// Override the Cargo package name while preserving the generated Rust target name.
    pub fn set_package_name(&mut self, package_name: Option<String>) {
        self.package_name = package_name;
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

    /// Set the Cargo target directory used by generated Rust projects.
    pub fn set_cargo_target_dir_override(&mut self, target_dir: Option<PathBuf>) {
        self.cargo_target_dir_override = target_dir;
    }

    /// Override the Rust edition used in Cargo.toml.
    pub fn set_rust_edition(&mut self, edition: Option<String>) {
        self.rust_edition = edition;
    }

    /// Set the cargo profile used for `incan run`.
    pub fn set_run_profile(&mut self, profile: RunProfile) {
        self.run_profile = profile;
    }

    /// Configure one compiled SDK provider directly for focused generator tests.
    #[cfg(test)]
    fn set_compiled_provider_modules(&mut self, crate_name: &str, modules: CompiledSdkModules) {
        let top_level_modules = modules
            .relative_paths()
            .filter_map(|path| path.first().cloned())
            .collect::<BTreeSet<_>>();
        if top_level_modules.is_empty() {
            self.compiled_provider_modules.remove(crate_name);
        } else {
            self.compiled_provider_modules
                .insert(crate_name.to_string(), top_level_modules);
        }
        self.compiled_sdk_modules = modules;
    }

    /// Configure generated Rust facade links from the shared compiler provider plan.
    pub(crate) fn set_provider_plan(&mut self, plan: &ProviderPlan) {
        self.compiled_provider_modules.clear();
        self.compiled_sdk_modules = CompiledSdkModules::from_provider_plan(plan);
        for provider in plan.sdk_link_roots() {
            let Some(artifact) = provider.artifact.as_ref() else {
                continue;
            };
            let modules = self
                .compiled_provider_modules
                .entry(artifact.dependency_key.clone())
                .or_default();
            for claim in &provider.namespace_claims {
                if claim.first().map(String::as_str) == Some(stdlib::STDLIB_ROOT)
                    && let Some(module) = claim.get(1)
                {
                    modules.insert(module.clone());
                }
            }
        }
    }

    /// Return the generated Rust project directory.
    pub fn output_dir(&self) -> &Path {
        &self.output_dir
    }

    /// Return the generated Cargo manifest path.
    pub fn cargo_manifest_path(&self) -> PathBuf {
        self.output_dir.join("Cargo.toml")
    }

    /// Return the generated Rust crate root file.
    pub fn crate_root_path(&self) -> PathBuf {
        if self.is_binary {
            self.output_dir.join("src").join("main.rs")
        } else {
            self.output_dir.join("src").join("lib.rs")
        }
    }

    /// Resolve the optional generated-project Cargo target override.
    ///
    /// This is primarily used by integration tests and smoke gates that compile many generated Rust projects from one
    /// parent workspace. It lets those projects share dependency artifacts while keeping ordinary user invocations on
    /// the parent-scoped default target directory.
    pub(super) fn generated_cargo_target_dir_override() -> Option<PathBuf> {
        let raw = std::env::var_os(GENERATED_CARGO_TARGET_DIR_ENV)?;
        let raw = PathBuf::from(raw);
        if raw.as_os_str().is_empty() {
            return None;
        }
        Some(Self::resolve_target_dir(raw))
    }

    /// Return the explicit target override, falling back to the legacy environment variable.
    pub(super) fn cargo_target_dir_override(&self) -> Option<PathBuf> {
        self.cargo_target_dir_override
            .clone()
            .map(Self::resolve_target_dir)
            .or_else(Self::generated_cargo_target_dir_override)
    }

    /// Resolve the cargo target directory for a generated project.
    pub(super) fn resolve_target_dir(target_dir: PathBuf) -> PathBuf {
        if target_dir.is_absolute() {
            target_dir
        } else if let Ok(cwd) = std::env::current_dir() {
            cwd.join(target_dir)
        } else {
            target_dir
        }
    }

    /// Cargo target name used for the generated binary or library target.
    ///
    /// When a caller opts into a broad shared target directory, multiple unrelated generated projects can have the same
    /// user-facing project name (`main`, `consumer`, etc.). Cargo writes root binaries and libraries at
    /// `target/<profile>/<target-name>`, so shared target dirs need a unique target name to avoid stale binary reuse
    /// and parallel build collisions. Library target names stay stable because native Rust consumers import them as
    /// crate names from generated library artifacts.
    pub(super) fn cargo_target_name(&self) -> String {
        if self.is_binary && self.cargo_target_dir_override().is_some() {
            Self::shared_target_safe_name(&self.name, &self.output_dir)
        } else {
            self.name.clone()
        }
    }

    /// Return a filesystem-safe name for a shared cargo target directory.
    pub(super) fn shared_target_safe_name(name: &str, output_dir: &Path) -> String {
        let mut normalized = name
            .chars()
            .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '_' })
            .collect::<String>();
        if normalized.is_empty() {
            normalized.push_str("incan_project");
        }
        if !normalized
            .as_bytes()
            .first()
            .is_some_and(|byte| byte.is_ascii_alphabetic() || *byte == b'_')
        {
            normalized.insert(0, '_');
        }

        let absolute_output_dir = if output_dir.is_absolute() {
            output_dir.to_path_buf()
        } else if let Ok(cwd) = std::env::current_dir() {
            cwd.join(output_dir)
        } else {
            output_dir.to_path_buf()
        };

        let mut hasher = Sha256::new();
        hasher.update(name.as_bytes());
        hasher.update(b"\0");
        hasher.update(absolute_output_dir.to_string_lossy().as_bytes());
        let digest_bytes = hasher.finalize();
        let digest = hex::encode(&digest_bytes[..8]);

        format!("{normalized}_{digest}")
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
    fn remove_conflicting_module_artifact(path: &Path) -> io::Result<bool> {
        if path.is_dir() {
            fs::remove_dir_all(path)?;
            return Ok(true);
        } else if path.exists() {
            fs::remove_file(path)?;
            return Ok(true);
        }
        Ok(false)
    }

    /// Write `content` to `path` only when the file contents actually changed.
    fn write_file_if_changed(path: &Path, content: &str) -> io::Result<bool> {
        match fs::read_to_string(path) {
            Ok(existing) if existing == content => Ok(false),
            Ok(_) | Err(_) => {
                fs::write(path, content)?;
                Ok(true)
            }
        }
    }

    /// Return the generated filename for a top-level Rust module leaf.
    ///
    /// Cargo treats `src/main.rs` and `src/lib.rs` as crate roots. Generated library projects can still have source
    /// modules named `main` or `lib`, so those module leaves use explicit `#[path]` declarations and non-root
    /// filenames.
    fn top_level_leaf_module_file_name(module_name: &str) -> String {
        match module_name {
            "main" | "lib" => format!("__incan_mod_{module_name}.rs"),
            _ => format!("{module_name}.rs"),
        }
    }

    /// Return whether a top-level generated module name would otherwise create a Cargo crate-root file.
    fn is_special_top_level_leaf_module(module_name: &str) -> bool {
        matches!(module_name, "main" | "lib")
    }

    /// Return the path used in a top-level module declaration for a generated leaf module.
    fn top_level_leaf_module_relative_path(module_name: &str) -> String {
        Self::top_level_leaf_module_file_name(module_name)
    }

    /// Render a Rust module declaration for a generated module file or directory.
    ///
    /// Keyword-named modules use raw identifiers in Rust (`r#type`) while keeping the on-disk layout clean
    /// (`type.rs`, `type/mod.rs`). The explicit `#[path = "..."]` keeps that mapping obvious in emitted code and
    /// matches the RFC 023 closeout contract for keyword-named module paths.
    fn render_module_decl(name: &str, relative_path: &str, visibility: &str) -> String {
        let escaped_name = rust_keywords::escape_keyword(name);
        let default_leaf_path = format!("{name}.rs");
        let default_dir_path = format!("{name}/mod.rs");
        if rust_keywords::is_keyword(name) || (relative_path != default_leaf_path && relative_path != default_dir_path)
        {
            return format!("#[path = \"{relative_path}\"]\n{visibility}mod {escaped_name};");
        }
        format!("{visibility}mod {escaped_name};")
    }

    /// Render the compatibility namespace used by generated compiler bridges inside the compiled stdlib artifact.
    ///
    /// The artifact owns its source modules at crate root so consumers can depend on normal Rust paths, but generated
    /// source still refers to `__incan_std` while the migration is in flight. Re-export each concrete root module
    /// rather than glob-re-exporting the crate: the latter would recursively re-export the facade itself.
    fn compiled_provider_facade(&self, local_top_level_modules: &[String]) -> String {
        let mut facade = String::from("pub mod __incan_std {\n");
        for module in local_top_level_modules {
            let escaped_module = rust_keywords::escape_keyword(module);
            facade.push_str(&format!("    pub use crate::{escaped_module};\n"));
        }
        for crate_name in self.compiled_provider_modules.keys() {
            let escaped_crate = rust_keywords::escape_keyword(crate_name);
            // A provider's compatibility facade includes the provider modules of its component dependencies. Reuse
            // that checked artifact projection so compiler-generated paths such as `crate::__incan_std::traits` keep
            // working without making transitive component crates direct Cargo dependencies of the consumer.
            facade.push_str(&format!("    pub use {escaped_crate}::__incan_std::*;\n"));
        }
        facade.push_str("}\n");
        facade
    }

    /// Return whether this generated project links at least one compiled SDK provider.
    ///
    /// The artifact preserves a narrow `__incan_std` facade for compiler-generated compatibility paths. Consumers
    /// re-export that facade instead of regenerating any stdlib source; the bridge can be removed once every emitted
    /// compiler path is artifact-qualified.
    fn links_compiled_sdk_provider(&self) -> bool {
        !self.compiled_provider_modules.is_empty()
    }

    /// Generate the project structure (single-file mode).
    pub fn generate(&self, rust_code: &str) -> io::Result<bool> {
        let src_dir = self.ensure_generated_src_dir()?;
        let mut changed = false;

        // Write Cargo.toml
        let cargo_toml = self.generate_cargo_toml()?;
        changed |= Self::write_file_if_changed(&self.output_dir.join("Cargo.toml"), &cargo_toml)?;
        changed |= self.write_cargo_lock_if_needed()?;

        // Single-file consumers need the same artifact-backed compatibility namespace as nested projects. Compiler
        // bridges still use `crate::__incan_std` while they are migrated to canonical artifact paths; re-exporting
        // the artifact's facade keeps those bridges out of a regenerated source stdlib tree.
        let mut full_main = rust_code.to_string();
        if self.links_compiled_sdk_provider() && !is_sdk_provider_build() && !full_main.contains("mod __incan_std") {
            let facade = self.compiled_provider_facade(&[]);
            if let Some(marker_pos) = full_main.find(MOD_INSERT_MARKER) {
                let line_end = full_main[marker_pos..]
                    .find('\n')
                    .map(|offset| marker_pos + offset + 1)
                    .unwrap_or(full_main.len());
                full_main.replace_range(marker_pos..line_end, &facade);
            } else if let Some(attr_pos) = full_main.find("#![") {
                let line_end = full_main[attr_pos..]
                    .find('\n')
                    .map(|offset| attr_pos + offset + 1)
                    .unwrap_or(full_main.len());
                full_main.insert_str(line_end, &facade);
            } else {
                full_main = format!("{facade}\n{full_main}");
            }
        }

        // Write main source file
        let main_file = if self.is_binary {
            src_dir.join("main.rs")
        } else {
            src_dir.join("lib.rs")
        };
        changed |= Self::write_file_if_changed(&main_file, &full_main)?;

        Ok(changed)
    }

    /// Generate the project structure with multiple module files (flat).
    ///
    /// # Arguments
    /// * `main_code` - The main.rs code (without mod declarations, they will be prepended)
    /// * `modules` - HashMap of module name to module code (e.g., "models" -> "pub struct User { ... }")
    pub fn generate_multi(&self, main_code: &str, modules: &HashMap<String, String>) -> io::Result<bool> {
        let src_dir = self.ensure_generated_src_dir()?;
        let mut changed = false;

        for module_name in modules.keys() {
            changed |= Self::remove_conflicting_module_artifact(&src_dir.join(module_name))?;
            if Self::is_special_top_level_leaf_module(module_name) {
                changed |= Self::remove_conflicting_module_artifact(&src_dir.join(format!("{module_name}.rs")))?;
            }
        }

        // Write Cargo.toml
        let cargo_toml = self.generate_cargo_toml()?;
        changed |= Self::write_file_if_changed(&self.output_dir.join("Cargo.toml"), &cargo_toml)?;
        changed |= self.write_cargo_lock_if_needed()?;

        // Write each module file
        for (module_name, module_code) in modules {
            let module_file = src_dir.join(Self::top_level_leaf_module_file_name(module_name));
            changed |= Self::write_file_if_changed(&module_file, module_code)?;
        }

        // Build main.rs with the generated header first, then mod declarations.
        // Crate attributes (`#![...]`) must appear before any Rust items (including `mod ...;`),
        // so we insert module declarations at the backend marker after any crate attributes.
        let mut full_main = String::new();
        full_main.push_str(main_code);

        if !modules.is_empty() {
            // Add mod declarations for each module (sorted for deterministic output)
            let mut module_names: Vec<_> = modules.keys().collect();
            module_names.sort();
            let visibility = if self.is_binary { "" } else { "pub " };
            let mods: String = module_names
                .iter()
                .map(|m| Self::render_module_decl(m, &Self::top_level_leaf_module_relative_path(m), visibility))
                .collect::<Vec<_>>()
                .join("\n")
                + "\n";

            // Insert at the backend marker when present. Older generated code may not have the marker, so fall back to
            // the crate-attribute position before prepending.
            if let Some(marker_pos) = full_main.find(MOD_INSERT_MARKER) {
                let line_end = full_main[marker_pos..]
                    .find('\n')
                    .map(|o| marker_pos + o + 1)
                    .unwrap_or(full_main.len());
                full_main.replace_range(marker_pos..line_end, &mods);
                full_main.insert(marker_pos + mods.len(), '\n');
            } else if let Some(attr_pos) = full_main.find("#![") {
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
        changed |= Self::write_file_if_changed(&main_file, &full_main)?;

        Ok(changed)
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
    pub fn generate_nested(&self, main_code: &str, modules: &HashMap<Vec<String>, String>) -> io::Result<bool> {
        let src_dir = self.ensure_generated_src_dir()?;
        let mut changed = false;

        // Write Cargo.toml
        let cargo_toml = self.generate_cargo_toml()?;
        changed |= Self::write_file_if_changed(&self.output_dir.join("Cargo.toml"), &cargo_toml)?;
        changed |= self.write_cargo_lock_if_needed()?;

        // ---- RFC 023: Transform stdlib paths to __incan_std ----
        let mut transformed_modules: HashMap<Vec<String>, String> = HashMap::new();
        for (path, code) in modules {
            let transformed_path = transform_stdlib_path(path);
            transformed_modules.insert(transformed_path, code.clone());
        }

        // Remove only migrated artifact modules from a reused generated project. Other source-backed stdlib modules
        // must remain available until their own migration is complete.
        for relative_path in self.compiled_sdk_modules.relative_paths() {
            let mut emitted_path = Vec::with_capacity(relative_path.len() + 1);
            emitted_path.push(stdlib::INCAN_STD_NAMESPACE.to_string());
            emitted_path.extend(relative_path.iter().cloned());
            let Some(last_segment) = emitted_path.last() else {
                continue;
            };
            let mut leaf = src_dir.clone();
            for segment in &emitted_path[..emitted_path.len() - 1] {
                leaf.push(segment);
            }
            leaf.push(last_segment);
            changed |= Self::remove_conflicting_module_artifact(&leaf.with_extension("rs"))?;
            for depth in (1..emitted_path.len()).rev() {
                let parent = &emitted_path[..depth];
                if transformed_modules.keys().any(|path| path.starts_with(parent)) {
                    break;
                }
                let parent_dir = parent.iter().fold(src_dir.clone(), |dir, segment| dir.join(segment));
                changed |= Self::remove_conflicting_module_artifact(&parent_dir)?;
            }
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
                changed |= Self::remove_conflicting_module_artifact(&module_path.with_extension("rs"))?;
            } else {
                changed |= Self::remove_conflicting_module_artifact(&module_path)?;
                if path_segments.len() == 1
                    && let Some(module_name) = path_segments.first()
                    && Self::is_special_top_level_leaf_module(module_name)
                {
                    changed |= Self::remove_conflicting_module_artifact(&module_path.with_extension("rs"))?;
                }
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
                .map(|s| {
                    let mut child_path = dir_path.clone();
                    child_path.push(s.clone());
                    let relative_path = if modules_with_submodules.contains(&child_path) {
                        format!("{s}/mod.rs")
                    } else {
                        format!("{s}.rs")
                    };
                    Self::render_module_decl(s, &relative_path, "pub ")
                })
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
            changed |= Self::write_file_if_changed(&mod_rs_path, &mod_rs_content)?;
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
            let file_name = if path_segments.len() == 1 {
                Self::top_level_leaf_module_file_name(file_stem)
            } else {
                format!("{file_stem}.rs")
            };
            file_path = file_path.join(file_name);

            changed |= Self::write_file_if_changed(&file_path, module_code)?;
        }

        // ---- Build main.rs with generated header + top-level mod declarations ----
        // Crate attributes (`#![...]`) must appear before any Rust items (including `mod ...;`), so we insert module
        // declarations at the backend marker after any crate attributes.
        let mut full_main = String::new();
        full_main.push_str(main_code);

        let mut sorted_top: Vec<_> = top_level_modules.into_iter().collect();
        sorted_top.sort();
        let consumer_stdlib_facade = self.links_compiled_sdk_provider()
            && !is_sdk_provider_build()
            && !sorted_top.iter().any(|module| module == "__incan_std");
        if !sorted_top.is_empty() || consumer_stdlib_facade {
            let visibility = if self.is_binary { "" } else { "pub " };
            let mut mods = sorted_top
                .iter()
                .map(|m| {
                    let top_level_path = vec![(*m).clone()];
                    let relative_path = if modules_with_submodules.contains(&top_level_path) {
                        format!("{m}/mod.rs")
                    } else {
                        Self::top_level_leaf_module_relative_path(m)
                    };
                    Self::render_module_decl(m, &relative_path, visibility)
                })
                .collect::<Vec<_>>()
                .join("\n");
            if !mods.is_empty() {
                mods.push('\n');
            }

            if !self.is_binary && is_sdk_provider_build() {
                mods.push('\n');
                mods.push_str(&self.compiled_provider_facade(&sorted_top));
            } else if consumer_stdlib_facade {
                mods.push_str(&self.compiled_provider_facade(&[]));
            }

            if let Some(marker_pos) = full_main.find(MOD_INSERT_MARKER) {
                let line_end = full_main[marker_pos..]
                    .find('\n')
                    .map(|o| marker_pos + o + 1)
                    .unwrap_or(full_main.len());
                full_main.replace_range(marker_pos..line_end, &mods);
                full_main.insert(marker_pos + mods.len(), '\n');
            } else if let Some(attr_pos) = full_main.find("#![") {
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
        changed |= Self::write_file_if_changed(&main_file, &full_main)?;

        Ok(changed)
    }

    /// Write a Cargo.lock file if a lock payload was provided.
    fn write_cargo_lock_if_needed(&self) -> io::Result<bool> {
        let Some(payload) = &self.cargo_lock_payload else {
            return Ok(false);
        };
        Self::write_file_if_changed(&self.output_dir.join("Cargo.lock"), payload)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::DependencySource;
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
    fn test_compiled_sdk_provider_facade_reexports_direct_modules() {
        let generator = ProjectGenerator::new("target/test-provider-facade", "provider", false);
        let facade = generator.compiled_provider_facade(&["async".to_string(), "fs".to_string(), "traits".to_string()]);

        assert_eq!(
            facade,
            "pub mod __incan_std {\n    pub use crate::r#async;\n    pub use crate::fs;\n    pub use crate::traits;\n}\n"
        );
    }

    #[test]
    fn test_generate_nested_consumer_reexports_compiled_stdlib_compatibility_facade()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let mut generator = ProjectGenerator::new(temp.path(), "consumer", true);
        generator.set_dependencies(vec![DependencySpec {
            crate_name: "test_sdk_provider".to_string(),
            version: None,
            features: Vec::new(),
            default_features: true,
            source: DependencySource::Path {
                path: temp.path().join("artifact"),
            },
            optional: false,
            package: None,
        }]);
        generator.set_compiled_provider_modules(
            "test_sdk_provider",
            CompiledSdkModules::from_relative_paths([vec!["math".to_string()]]),
        );

        generator.generate_nested("fn main() {}\n", &HashMap::new())?;
        let generated = fs::read_to_string(temp.path().join("src/main.rs"))?;
        assert!(
            generated.contains("pub use test_sdk_provider::__incan_std::*;"),
            "compiled-provider consumers must reuse the artifact compatibility facade:\n{generated}"
        );
        Ok(())
    }

    #[test]
    fn test_generate_consumer_reexports_compiled_stdlib_compatibility_facade() -> Result<(), Box<dyn std::error::Error>>
    {
        let temp = tempfile::tempdir()?;
        let mut generator = ProjectGenerator::new(temp.path(), "consumer", true);
        generator.set_dependencies(vec![DependencySpec {
            crate_name: "test_sdk_provider".to_string(),
            version: None,
            features: Vec::new(),
            default_features: true,
            source: DependencySource::Path {
                path: temp.path().join("artifact"),
            },
            optional: false,
            package: None,
        }]);
        generator.set_compiled_provider_modules(
            "test_sdk_provider",
            CompiledSdkModules::from_relative_paths([vec!["math".to_string()]]),
        );

        generator.generate("// __INCAN_INSERT_MODS__\nfn main() {}\n")?;
        let generated = fs::read_to_string(temp.path().join("src/main.rs"))?;
        assert!(
            generated.contains("pub use test_sdk_provider::__incan_std::*;"),
            "single-file compiled-provider consumers must reuse the artifact compatibility facade:\n{generated}"
        );
        assert!(!generated.contains("__INCAN_INSERT_MODS__"));
        Ok(())
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
    fn test_generate_multi_escapes_keyword_module_names() -> Result<(), Box<dyn std::error::Error>> {
        let temp_dir = std::env::temp_dir().join("incan_test_keyword_modules");
        let _ = fs::remove_dir_all(&temp_dir);

        let generator = ProjectGenerator::new(&temp_dir, "test_keyword_modules", true);

        let mut modules = HashMap::new();
        modules.insert("async".to_string(), "pub fn launch() {}".to_string());
        modules.insert("type".to_string(), "pub fn marker() {}".to_string());

        generator.generate_multi("fn main() {}", &modules)?;

        let main_content = fs::read_to_string(temp_dir.join("src/main.rs"))?;
        assert!(main_content.contains("#[path = \"async.rs\"]\nmod r#async;"));
        assert!(main_content.contains("#[path = \"type.rs\"]\nmod r#type;"));
        assert!(temp_dir.join("src/async.rs").exists());
        assert!(temp_dir.join("src/type.rs").exists());

        let async_content = fs::read_to_string(temp_dir.join("src/async.rs"))?;
        assert!(async_content.contains("pub fn launch"));

        let type_content = fs::read_to_string(temp_dir.join("src/type.rs"))?;
        assert!(type_content.contains("pub fn marker"));

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
    fn test_generate_nested_escapes_keyword_submodule_names() -> Result<(), Box<dyn std::error::Error>> {
        let temp_dir = std::env::temp_dir().join("incan_test_nested_keyword_modules");
        let _ = fs::remove_dir_all(&temp_dir);

        let generator = ProjectGenerator::new(&temp_dir, "test_nested_keyword_modules", false);

        let mut modules = HashMap::new();
        modules.insert(
            vec!["api".to_string(), "async".to_string()],
            "pub fn launch() {}".to_string(),
        );
        modules.insert(
            vec!["type".to_string(), "helpers".to_string()],
            "pub fn marker() {}".to_string(),
        );

        generator.generate_nested("pub fn root() {}", &modules)?;

        assert!(temp_dir.join("src/api").exists());
        assert!(temp_dir.join("src/api/mod.rs").exists());
        assert!(temp_dir.join("src/api/async.rs").exists());
        assert!(temp_dir.join("src/type").exists());
        assert!(temp_dir.join("src/type/mod.rs").exists());
        assert!(temp_dir.join("src/type/helpers.rs").exists());

        let main_content = fs::read_to_string(temp_dir.join("src/lib.rs"))?;
        assert!(main_content.contains("#[path = \"type/mod.rs\"]\npub mod r#type;"));

        let mod_rs_content = fs::read_to_string(temp_dir.join("src/api/mod.rs"))?;
        assert!(mod_rs_content.contains("#[path = \"async.rs\"]\npub mod r#async;"));

        let async_content = fs::read_to_string(temp_dir.join("src/api/async.rs"))?;
        assert!(async_content.contains("pub fn launch"));

        let type_mod_rs_content = fs::read_to_string(temp_dir.join("src/type/mod.rs"))?;
        assert!(type_mod_rs_content.contains("pub mod helpers;"));

        let _ = fs::remove_dir_all(&temp_dir);
        Ok(())
    }

    #[test]
    fn test_generate_nested_avoids_cargo_root_filenames_for_top_level_modules() -> Result<(), Box<dyn std::error::Error>>
    {
        let temp_dir = std::env::temp_dir().join("incan_test_special_top_modules");
        let _ = fs::remove_dir_all(&temp_dir);

        let generator = ProjectGenerator::new(&temp_dir, "test_special_top_modules", false);

        let mut modules = HashMap::new();
        modules.insert(vec!["main".to_string()], "pub fn from_main() {}".to_string());
        modules.insert(vec!["lib".to_string()], "pub fn from_lib() {}".to_string());
        generator.generate_nested("pub fn root() {}", &modules)?;

        let lib_rs = fs::read_to_string(temp_dir.join("src/lib.rs"))?;
        assert!(lib_rs.contains("#[path = \"__incan_mod_main.rs\"]\npub mod main;"));
        assert!(lib_rs.contains("#[path = \"__incan_mod_lib.rs\"]\npub mod lib;"));
        assert!(temp_dir.join("src/__incan_mod_main.rs").exists());
        assert!(temp_dir.join("src/__incan_mod_lib.rs").exists());
        assert!(
            !temp_dir.join("src/main.rs").exists(),
            "top-level generated module must not create a Cargo binary root"
        );

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
    fn test_generate_is_unchanged_when_contents_match() -> Result<(), Box<dyn std::error::Error>> {
        let temp_dir = std::env::temp_dir().join("incan_test_generate_unchanged");
        let _ = fs::remove_dir_all(&temp_dir);

        let generator = ProjectGenerator::new(&temp_dir, "test_unchanged", true);
        let first = generator.generate("fn main() {}\n")?;
        let second = generator.generate("fn main() {}\n")?;

        assert!(first, "initial generation should report changes");
        assert!(!second, "identical regeneration should not rewrite files");

        let _ = fs::remove_dir_all(&temp_dir);
        Ok(())
    }

    #[test]
    fn test_generate_nested_is_unchanged_when_contents_match() -> Result<(), Box<dyn std::error::Error>> {
        let temp_dir = std::env::temp_dir().join("incan_test_generate_nested_unchanged");
        let _ = fs::remove_dir_all(&temp_dir);

        let generator = ProjectGenerator::new(&temp_dir, "test_nested_unchanged", true);
        let mut modules = HashMap::new();
        modules.insert(
            vec!["dataset".to_string(), "ops".to_string()],
            "pub fn filter_ds<T>(ds: T) -> T { ds }".to_string(),
        );

        let first = generator.generate_nested("fn main() {}\n", &modules)?;
        let second = generator.generate_nested("fn main() {}\n", &modules)?;

        assert!(first, "initial nested generation should report changes");
        assert!(!second, "identical nested regeneration should not rewrite files");

        let _ = fs::remove_dir_all(&temp_dir);
        Ok(())
    }

    #[test]
    fn test_generate_nested_removes_manifest_owned_stdlib_source() -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let stale_module = temp.path().join("src/__incan_std/fs/locking.rs");
        fs::create_dir_all(stale_module.parent().ok_or("stale module had no parent")?)?;
        fs::write(&stale_module, "pub fn stale() {}\n")?;

        let mut generator = ProjectGenerator::new(temp.path(), "consumer", true);
        generator.set_compiled_provider_modules(
            "test_sdk_provider",
            CompiledSdkModules::from_relative_paths([vec!["fs".to_string(), "locking".to_string()]]),
        );
        generator.generate_nested("fn main() {}\n", &HashMap::new())?;

        assert!(
            !stale_module.exists(),
            "artifact-owned modules discovered from the manifest must be removed from reused consumer projects"
        );
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
