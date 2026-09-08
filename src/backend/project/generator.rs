//! ProjectGenerator: high-level API that renders compiler-owned Rust source projections
//!
//! This is the primary struct for generating inspectable Rust projections from Incan code.
//! Its responsibilities are split across sibling modules:
//!
//! - **This module** — struct definition, setters, and `generate*()` methods

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
#[cfg(feature = "cli")]
use std::sync::RwLock;

use crate::compiled_sdk::CompiledSdkModules;
use crate::frontend::library_manifest_index::LibraryArtifactMetadata;
#[cfg(feature = "cli")]
use crate::generated_cache::GeneratedCacheLease;
use crate::library_manifest::{LibraryManifest, ProviderDependencyKind, digest_provider_artifact};
use crate::manifest::{DependencySource, DependencySpec};
use crate::provider::{ProviderPlan, SDK_PROVIDER_BUILD_ENV, SdkArtifactProjection, SdkDependencyRebinding};
use incan_core::lang::{rust_keywords, stdlib};
use sha2::{Digest as _, Sha256};

const MOD_INSERT_MARKER: &str = "// __INCAN_INSERT_MODS__";

/// One checked dependency edge and its effective projected artifact root.
struct ProjectedArtifactEdge {
    dependency_key: String,
    provider_name: String,
    source_root: PathBuf,
    target_root: PathBuf,
    kind: ProviderDependencyKind,
    default_features: bool,
    optional: bool,
}

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
    std::env::var_os(SDK_PROVIDER_BUILD_ENV).is_some()
}

/// Normalize an artifact coordinate through its nearest existing ancestor so absent cache tails remain comparable.
fn normalize_artifact_path(path: &Path) -> PathBuf {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|current| current.join(path))
            .unwrap_or_else(|_| path.to_path_buf())
    };
    let mut cursor = absolute.as_path();
    let mut tail = Vec::new();
    loop {
        if let Ok(mut canonical) = fs::canonicalize(cursor) {
            for component in tail.iter().rev() {
                canonical.push(component);
            }
            return canonical;
        }
        let Some(name) = cursor.file_name() else {
            return absolute;
        };
        tail.push(name.to_os_string());
        let Some(parent) = cursor.parent() else {
            return absolute;
        };
        cursor = parent;
    }
}

/// Render a filesystem-safe prefix for one deterministic rebound artifact directory.
fn sanitize_artifact_name(name: &str) -> String {
    let normalized = name
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '-'
            }
        })
        .collect::<String>();
    if normalized.is_empty() {
        "compiled-library".to_string()
    } else {
        normalized
    }
}

/// Render a path-independent dependency identity for generated root-artifact naming.
fn dependency_spec_identity(dependency: &DependencySpec) -> String {
    let mut features = dependency.features.clone();
    features.sort();
    features.dedup();
    let source = match &dependency.source {
        DependencySource::Registry => "registry".to_string(),
        DependencySource::Git { url, reference } => format!("git:{url}:{reference:?}"),
        // Cargo fingerprints the path crate and rebuilds the shared root in place when its contents differ. Hashing
        // the physical path here would only multiply top-level root identities across compatible worktrees.
        DependencySource::Path { .. } => "path".to_string(),
    };
    format!(
        "{}\0{}\0{}\0{}\0{}\0{}\0{}",
        dependency.crate_name,
        dependency.version.as_deref().unwrap_or_default(),
        features.join("\u{1f}"),
        dependency.default_features,
        source,
        dependency.optional,
        dependency.package.as_deref().unwrap_or_default(),
    )
}

/// Hash sorted logical records under one domain label.
fn hash_logical_records(hasher: &mut Sha256, label: &[u8], mut records: Vec<String>) {
    records.sort();
    hasher.update(label);
    for record in records {
        hasher.update(record.as_bytes());
        hasher.update(b"\0");
    }
}

/// Render compiled-artifact metadata without its checkout- or cache-root paths.
fn artifact_metadata_identity(metadata: &LibraryArtifactMetadata) -> String {
    format!(
        "{}\0{}\0{:?}",
        metadata.dependency_key, metadata.manifest_name, metadata.kind
    )
}

/// Project generator for creating runnable Rust projects from Incan code.
pub struct ProjectGenerator {
    /// Output directory for the generated project
    pub(super) output_dir: PathBuf,
    /// Project name
    pub(super) name: String,
    /// Authored package name when it differs from the generated Rust target name.
    pub(super) package_name: Option<String>,
    /// Authored package version exposed to generated native code.
    pub(super) package_version: Option<String>,
    /// Optional authored SPDX license identifier or expression.
    pub(super) package_license: Option<String>,
    /// Whether this is a binary (true) or library (false)
    pub(super) is_binary: bool,
    /// Whether this binary's generated Cargo manifest also needs a publisher-only library target at `src/main.rs`.
    pub(super) companion_library_target: bool,
    /// Enabled stdlib feature flags for the generated project, including compiler-required runtime support.
    pub(super) stdlib_features: Vec<String>,
    /// Resolved Rust crate dependencies.
    pub(super) dependencies: Vec<DependencySpec>,
    /// Resolved dev-only Rust dependencies.
    pub(super) dev_dependencies: Vec<DependencySpec>,
    /// Whether dev dependencies should be emitted.
    pub(super) include_dev_dependencies: bool,
    /// Active-use lease for the generated native output domain.
    #[cfg(feature = "cli")]
    pub(super) generated_cache_lease: RwLock<Option<GeneratedCacheLease>>,
    /// Compatibility-domain identity used to validate project-local run publications.
    #[cfg(feature = "cli")]
    pub(super) generated_cache_identity: Option<String>,
    /// Optional Rust edition override.
    pub(super) rust_edition: Option<String>,
    /// Profile used when building the generated crate for `incan run`.
    pub(super) run_profile: RunProfile,
    /// Modules supplied by linked compiled SDK providers.
    pub(super) compiled_sdk_modules: CompiledSdkModules,
    /// Top-level `std.*` modules grouped by the generated Rust crate that supplies each compiled SDK provider.
    pub(super) compiled_provider_modules: BTreeMap<String, BTreeSet<String>>,
    /// Equivalent active SDK paths used to project immutable compiled-library Cargo artifacts into this build.
    pub(super) sdk_dependency_rebindings: Vec<SdkDependencyRebinding>,
    /// Path-backed dependencies proven to come from the active SDK/toolchain rather than an ordinary project source.
    pub(super) sdk_path_dependencies: Vec<DependencySpec>,
    /// Complete compiled-artifact closure that must be copied so projected child paths propagate to every ancestor.
    pub(super) sdk_artifact_projections: Vec<SdkArtifactProjection>,
    /// Checked public namespace facades keyed by their source-module path.
    pub(super) public_namespace_facades: Option<BTreeMap<Vec<String>, BTreeSet<String>>>,
}

/// Runtime features required by compiler-emitted Rust independently of user-selected namespace features.
///
/// The compiler writes the checked `std.async` and `std.json` facades into every generated crate. Their Rust
/// imports require the matching runtime modules even when the project's authored imports do not name those
/// namespaces. It can also synthesize `OrdinalKey` bridges whose helpers are cfg-gated behind `ordinal`.
///
/// Keep this baseline narrower than the full-stdlib Loaf envelope: project output does not emit the `std.web`
/// facade unless the project explicitly requires it.
const GENERATED_PROJECT_RUNTIME_FEATURES: &[&str] = &["async", "json", "ordinal"];

/// Native optimization profile used for `incan run`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunProfile {
    /// Development output with debug information.
    Debug,
    /// Optimized release output.
    Release,
}

impl ProjectGenerator {
    /// Create a project generator for an Incan build target.
    pub fn new(output_dir: impl AsRef<Path>, name: &str, is_binary: bool) -> Self {
        Self {
            output_dir: output_dir.as_ref().to_path_buf(),
            name: name.to_string(),
            package_name: None,
            package_version: None,
            package_license: None,
            is_binary,
            companion_library_target: false,
            stdlib_features: GENERATED_PROJECT_RUNTIME_FEATURES
                .iter()
                .map(|feature| (*feature).to_string())
                .collect(),
            dependencies: Vec::new(),
            dev_dependencies: Vec::new(),
            include_dev_dependencies: false,
            #[cfg(feature = "cli")]
            generated_cache_lease: RwLock::new(None),
            #[cfg(feature = "cli")]
            generated_cache_identity: None,
            rust_edition: None,
            run_profile: RunProfile::Debug,
            compiled_sdk_modules: CompiledSdkModules::default(),
            compiled_provider_modules: BTreeMap::new(),
            sdk_dependency_rebindings: Vec::new(),
            sdk_path_dependencies: Vec::new(),
            sdk_artifact_projections: Vec::new(),
            public_namespace_facades: None,
        }
    }

    /// Select the producer-validated namespace graph that owns generated library facade re-exports.
    pub fn set_public_namespace_facades(&mut self, api: &crate::frontend::api_metadata::CheckedApiMetadataPackage) {
        self.public_namespace_facades = Some(
            api.public_namespaces
                .iter()
                .map(|namespace| {
                    let parent_names = api
                        .modules
                        .iter()
                        .filter(|module| module.module_path == namespace.path)
                        .flat_map(|module| &module.declarations)
                        .filter_map(crate::frontend::api_metadata::api_declaration_public_name)
                        .collect::<BTreeSet<_>>();
                    let children = namespace
                        .child_modules
                        .iter()
                        .filter(|child| {
                            let mut child_path = namespace.path.clone();
                            child_path.push((*child).clone());
                            api.modules
                                .iter()
                                .filter(|module| module.module_path == child_path)
                                .flat_map(|module| &module.declarations)
                                .filter(|declaration| {
                                    crate::frontend::api_metadata::checked_api_declaration_is_public_namespace_member(
                                        declaration,
                                    )
                                })
                                .filter_map(crate::frontend::api_metadata::api_declaration_public_name)
                                .any(|name| !parent_names.contains(name))
                        })
                        .cloned()
                        .collect();
                    (namespace.path.clone(), children)
                })
                .collect(),
        );
    }

    /// Set the stdlib feature flags required by this generated project.
    pub fn set_stdlib_features(&mut self, features: Vec<String>) {
        let mut normalized: Vec<String> = features
            .into_iter()
            .map(|feature| feature.trim().to_string())
            .filter(|feature| !feature.is_empty())
            .collect();
        normalized.extend(
            GENERATED_PROJECT_RUNTIME_FEATURES
                .iter()
                .map(|feature| (*feature).to_string()),
        );
        normalized.sort();
        normalized.dedup();
        self.stdlib_features = normalized;
    }

    /// Set the authored package name while preserving the generated Rust target name.
    pub fn set_package_name(&mut self, package_name: Option<String>) {
        self.package_name = package_name;
    }

    /// Set authored package metadata used by native emission.
    pub fn set_package_metadata(&mut self, version: Option<String>, license: Option<String>) {
        self.package_version = version;
        self.package_license = license;
    }

    /// Project retained package metadata into the existing native compile-time environment (#1037).
    ///
    /// Package coordinates come from this generator's caller, without reading a generated Cargo manifest. The
    /// executor resolves the portable source token against this generator's `src/main.rs` or `src/lib.rs` root and
    /// removes inherited Cargo environment first. These are Rust compatibility values, not a dependency selection.
    pub(crate) fn native_compile_environment(&self) -> BTreeMap<String, String> {
        BTreeMap::from([
            ("CARGO_MANIFEST_DIR".to_string(), "@oven-source-ancestor:2".to_string()),
            (
                "CARGO_PKG_NAME".to_string(),
                self.package_name.as_ref().unwrap_or(&self.name).clone(),
            ),
            (
                "CARGO_PKG_VERSION".to_string(),
                self.package_version
                    .as_deref()
                    .unwrap_or(crate::version::INCAN_VERSION)
                    .to_string(),
            ),
        ])
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

    /// Emit `src/main.rs` as a companion Cargo library target for one explicit compatibility publisher.
    ///
    /// Normal generated executables remain binary-only. The companion target exists only while Oven prepares a
    /// Rust-only interop bootstrap, where Cargo must compile the root without linking the not-yet-sealed native
    /// artifact.
    pub(crate) fn enable_companion_library_target(&mut self) {
        self.companion_library_target = true;
    }

    /// Retain the active-use lease for this generator's selected output domain.
    #[cfg(feature = "cli")]
    pub(crate) fn set_generated_cache_context(&mut self, lease: Option<GeneratedCacheLease>, identity: Option<String>) {
        *self
            .generated_cache_lease
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = lease;
        self.generated_cache_identity = identity;
    }

    /// Release a managed output lease once native work and local publication are complete.
    #[cfg(feature = "cli")]
    pub(super) fn finish_generated_cache_lease(&self) -> io::Result<()> {
        let lease = self
            .generated_cache_lease
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        if let Some(lease) = lease {
            lease.finish()?;
        }
        Ok(())
    }

    /// Keep non-CLI library builds independent from cache-management implementation details.
    #[cfg(not(feature = "cli"))]
    pub(super) fn finish_generated_cache_lease(&self) -> io::Result<()> {
        Ok(())
    }

    /// Return the managed compatibility identity, when the CLI selected one.
    #[cfg(test)]
    pub(super) fn generated_cache_identity(&self) -> Option<&str> {
        #[cfg(feature = "cli")]
        {
            self.generated_cache_identity.as_deref()
        }
        #[cfg(not(feature = "cli"))]
        {
            None
        }
    }

    /// Select the Rust edition for emitted native source.
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
        self.sdk_dependency_rebindings = plan.sdk_dependency_rebindings().to_vec();
        self.sdk_artifact_projections = plan.sdk_artifact_projections().to_vec();
        self.set_sdk_path_dependencies(
            plan.active_sdk_records()
                .filter_map(|provider| {
                    provider
                        .artifact
                        .as_ref()
                        .map(LibraryArtifactMetadata::to_dependency_spec)
                })
                .collect(),
        );
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

    /// Configure immutable compiled-library SDK projections for helper Cargo workspaces that do not retain a plan.
    pub(crate) fn set_sdk_dependency_rebindings(&mut self, rebindings: Vec<SdkDependencyRebinding>) {
        self.sdk_dependency_rebindings = rebindings;
    }

    /// Configure active path-backed SDK/toolchain dependencies for helper workspaces that do not retain a plan.
    pub(crate) fn set_sdk_path_dependencies(&mut self, dependencies: Vec<DependencySpec>) {
        self.sdk_path_dependencies.extend(
            dependencies
                .into_iter()
                .filter(|dependency| matches!(dependency.source, DependencySource::Path { .. }))
                .map(DependencySpec::normalized),
        );
        self.sdk_path_dependencies.sort_by(|left, right| {
            (
                &left.crate_name,
                left.package.as_deref(),
                match &left.source {
                    DependencySource::Path { path } => Some(path),
                    DependencySource::Registry | DependencySource::Git { .. } => None,
                },
            )
                .cmp(&(
                    &right.crate_name,
                    right.package.as_deref(),
                    match &right.source {
                        DependencySource::Path { path } => Some(path),
                        DependencySource::Registry | DependencySource::Git { .. } => None,
                    },
                ))
        });
        self.sdk_path_dependencies.dedup();
    }

    /// Configure the complete compiled-artifact closure for helper workspaces that do not retain a provider plan.
    pub(crate) fn set_sdk_artifact_projections(&mut self, projections: Vec<SdkArtifactProjection>) {
        self.sdk_artifact_projections = projections;
    }

    /// Return the generated Rust project directory.
    pub fn output_dir(&self) -> &Path {
        &self.output_dir
    }

    /// Return the generated Rust crate root file.
    pub fn crate_root_path(&self) -> PathBuf {
        if self.is_binary {
            self.output_dir.join("src").join("main.rs")
        } else {
            self.output_dir.join("src").join("lib.rs")
        }
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

    /// Normalize one generated Rust target name without changing the public Cargo package identity.
    ///
    /// Cargo packages may retain distribution spellings such as `hyphenated-library`, while `[lib].name` and direct
    /// `rustc --crate-name` require an identifier-compatible spelling. Every generated-library boundary must use this
    /// one normalization rule so a consumer sees the same crate identity as the generated manifest.
    pub(crate) fn rust_target_name(name: &str) -> String {
        let mut target_name = name
            .chars()
            .map(|character| {
                if character.is_ascii_alphanumeric() || character == '_' {
                    character
                } else {
                    '_'
                }
            })
            .collect::<String>();
        if target_name.is_empty()
            || target_name
                .chars()
                .next()
                .is_some_and(|character| character.is_ascii_digit())
        {
            target_name.insert(0, '_');
        }
        target_name
    }

    /// Return a filesystem-safe name for a shared cargo target directory.
    pub(super) fn shared_target_safe_name(name: &str, root_identity: &str) -> String {
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

        let mut hasher = Sha256::new();
        hasher.update(name.as_bytes());
        hasher.update(b"\0");
        hasher.update(root_identity.as_bytes());
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

    /// Keep Rust implementation lints from leaking through compiled Incan provider crates.
    ///
    /// Incan owns declaration reachability and identifier style for provider source. Private metadata declarations and
    /// snake-case constants are valid Incan even though the corresponding generated Rust triggers `dead_code` and
    /// `non_upper_case_globals`. Suppress only those representation-level lints, and only at the provider crate root.
    fn add_sdk_provider_crate_lints(&self, crate_root: &mut String, sdk_provider_build: bool) {
        if self.is_binary || !sdk_provider_build {
            return;
        }

        const PROVIDER_LINTS: &str = "#![allow(dead_code, non_upper_case_globals)]\n";
        if crate_root.contains(PROVIDER_LINTS) {
            return;
        }
        let insertion = crate_root.find(MOD_INSERT_MARKER).unwrap_or(0);
        crate_root.insert_str(insertion, PROVIDER_LINTS);
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

        // Single-file consumers need the same artifact-backed compatibility namespace as nested projects. Compiler
        // bridges still use `crate::__incan_std` while they are migrated to canonical artifact paths; re-exporting
        // the artifact's facade keeps those bridges out of a regenerated source stdlib tree.
        let mut full_main = rust_code.to_string();
        self.add_sdk_provider_crate_lints(&mut full_main, is_sdk_provider_build());
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
        self.add_sdk_provider_crate_lints(&mut full_main, is_sdk_provider_build());

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

            // Published Incan directory modules are namespace facades: public declarations from their immediate source
            // files remain reachable through the directory name without requiring hand-authored `pub from` plumbing.
            // Rust visibility still enforces the checked declaration boundary, so private implementation items do not
            // become public merely because their source file participates in the namespace.
            if !self.is_binary
                && let Some(public_submodules) = self
                    .public_namespace_facades
                    .as_ref()
                    .and_then(|facades| facades.get(dir_path))
            {
                for submodule in submodules
                    .iter()
                    .filter(|submodule| public_submodules.contains(*submodule))
                {
                    let escaped = rust_keywords::escape_keyword(submodule);
                    mod_rs_content.push_str(&format!("pub use {escaped}::*;\n"));
                }
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
        self.add_sdk_provider_crate_lints(&mut full_main, is_sdk_provider_build());

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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frontend::library_manifest_index::{LibraryArtifactMetadata, LibraryManifestIndex};
    use crate::library_manifest::ProviderDependencyMetadata;
    use crate::manifest::DependencySource;
    use crate::provider::{
        NamespaceAuthority, ProviderIdentity, ProviderPlanError, ProviderProvenance, ProviderRecord,
    };
    use std::collections::HashMap;
    use std::process::Command;

    #[test]
    fn native_package_metadata_survives_relocation_without_cargo_inputs() -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let poison = project.path().join("Cargo.toml");
        fs::write(&poison, "not a manifest; must remain irrelevant\n")?;
        let mut environments = Vec::new();
        for binary in [true, false] {
            let output = project.path().join(if binary { "first" } else { "relocated" });
            let mut generator = ProjectGenerator::new(&output, "rust_target", binary);
            generator.set_package_name(Some("authored-package".to_string()));
            generator.set_package_metadata(Some("0.1.2".to_string()), None);
            environments.push(generator.native_compile_environment());
            assert!(
                !output.exists(),
                "reading retained metadata must not create a projection"
            );
            assert_eq!(
                generator.crate_root_path().parent().and_then(Path::parent),
                Some(output.as_path()),
                "the portable token must resolve from either generated crate root",
            );
        }
        assert_eq!(environments[0], environments[1]);
        assert_eq!(
            environments[0].get("CARGO_PKG_NAME").map(String::as_str),
            Some("authored-package")
        );
        assert_eq!(
            environments[0].get("CARGO_PKG_VERSION").map(String::as_str),
            Some("0.1.2")
        );
        assert_eq!(fs::read_to_string(&poison)?, "not a manifest; must remain irrelevant\n");
        Ok(())
    }

    #[test]
    fn native_package_metadata_preserves_default_and_explicit_version_inputs() {
        let mut generator = ProjectGenerator::new("absent-native-metadata-output", "target_name", true);
        let defaults = generator.native_compile_environment();
        assert_eq!(defaults.get("CARGO_PKG_NAME").map(String::as_str), Some("target_name"));
        assert_eq!(
            defaults.get("CARGO_PKG_VERSION").map(String::as_str),
            Some(crate::version::INCAN_VERSION)
        );
        assert_eq!(
            defaults.get("CARGO_MANIFEST_DIR").map(String::as_str),
            Some("@oven-source-ancestor:2")
        );
        generator.set_package_metadata(Some("9.8.7".to_string()), None);
        let explicit = generator.native_compile_environment();
        assert_ne!(
            defaults, explicit,
            "an embedded version remains an effective native input"
        );
        assert_eq!(explicit.get("CARGO_PKG_VERSION").map(String::as_str), Some("9.8.7"));
        assert_eq!(explicit.len(), 3);
    }

    /// Compile one small rebinding fixture with direct Rustc rather than making the generated Cargo graph the test
    /// executor. The frozen Oven suite has no Cargo consumer, and the relevant invariant is that the rebound source
    /// graph links from its declared artifact roots.
    fn compile_rebound_fixture_with_rustc(
        source: &Path,
        crate_name: &str,
        crate_type: &str,
        output: &Path,
        externs: &[(&str, &Path)],
    ) -> Result<(), Box<dyn std::error::Error>> {
        let rustc = std::env::var_os(crate::oven::compiler_suite_env::OVEN_COMPILER_SUITE_RUSTC_ENV)
            .or_else(|| std::env::var_os("RUSTC"))
            .unwrap_or_else(|| "rustc".into());
        let mut command = Command::new(rustc);
        command
            .arg("--edition")
            .arg("2024")
            .arg("--crate-name")
            .arg(crate_name)
            .arg("--crate-type")
            .arg(crate_type)
            .arg(source)
            .arg("-o")
            .arg(output)
            .env_remove("CARGO")
            .env_remove("CARGO_MANIFEST_DIR")
            .env_remove("CARGO_MANIFEST_PATH");
        for (name, artifact) in externs {
            let parent = artifact.parent().ok_or_else(|| {
                format!(
                    "direct Rustc fixture artifact for `{name}` has no dependency search directory: {}",
                    artifact.display()
                )
            })?;
            command
                .arg("-L")
                .arg(format!("dependency={}", parent.display()))
                .arg("--extern")
                .arg(format!("{name}={}", artifact.display()));
        }
        let result = command.output()?;
        if result.status.success() {
            return Ok(());
        }
        Err(format!(
            "direct Rustc fixture compilation for `{crate_name}` failed:\n{}{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        )
        .into())
    }

    /// Write immutable checked fixture inputs; these records are not a production native unit plan.
    fn sdk_rebinding_fixture_record(
        root: &Path,
        name: &str,
        source: &str,
        sdk: bool,
        dependencies: Vec<ProviderDependencyMetadata>,
    ) -> Result<ProviderRecord, Box<dyn std::error::Error>> {
        fs::create_dir_all(root.join("src"))?;
        fs::write(root.join("src/lib.rs"), source)?;
        let mut manifest = LibraryManifest::new(name, "0.1.0");
        manifest.contract_metadata.provider.provider_dependencies = dependencies;
        let manifest_path = root.join(format!("{name}.incnlib"));
        manifest.write_to_path(&manifest_path)?;
        Ok(ProviderRecord {
            identity: ProviderIdentity {
                name: name.to_string(),
                version: "0.1.0".to_string(),
                digest: digest_provider_artifact(root)?,
                feature_projection: BTreeSet::new(),
            },
            provenance: if sdk {
                ProviderProvenance::Sdk {
                    sdk_identity: "checked-fixture-sdk".to_string(),
                    component_id: name.to_string(),
                    inventory_path: None,
                }
            } else {
                ProviderProvenance::ProjectDependency {
                    dependency_key: name.to_string(),
                    manifest_path: manifest_path.clone(),
                }
            },
            authority: if sdk {
                NamespaceAuthority::SdkReserved
            } else {
                NamespaceAuthority::ProjectDependency {
                    dependency_key: name.to_string(),
                }
            },
            namespace_claims: BTreeSet::new(),
            available: true,
            enabled: true,
            manifest: Some(std::sync::Arc::new(manifest)),
            artifact: Some(LibraryArtifactMetadata::from_manifest_path(
                name,
                name,
                manifest_path,
                root.to_path_buf(),
            )),
            implementation_facets: Vec::new(),
        })
    }

    /// Retain the exact declared identity and coordinate of a fixture dependency without resolving it again.
    fn sdk_rebinding_fixture_edge(
        record: &ProviderRecord,
        relative_path: &str,
        private: bool,
    ) -> ProviderDependencyMetadata {
        ProviderDependencyMetadata {
            kind: if private {
                ProviderDependencyKind::PrivateImplementation
            } else {
                ProviderDependencyKind::PublicPackage
            },
            dependency_key: record.identity.name.clone(),
            provider_name: record.identity.name.clone(),
            provider_version: record.identity.version.clone(),
            artifact_digest: record.identity.digest.clone(),
            relative_artifact_path: relative_path.to_string(),
            requested_features: record.identity.feature_projection.clone(),
            default_features: false,
            optional: false,
        }
    }

    /// Snapshot complete fixture source inventories before physical compilation writes elsewhere.
    fn sdk_rebinding_fixture_inventory(
        roots: &[&Path],
    ) -> Result<BTreeMap<PathBuf, String>, Box<dyn std::error::Error>> {
        roots
            .iter()
            .map(|root| Ok((root.to_path_buf(), digest_provider_artifact(root)?)))
            .collect()
    }

    #[test]
    fn generated_consumer_rebinds_absent_private_sdk_cache_root_issue911() -> Result<(), Box<dyn std::error::Error>> {
        let workspace = tempfile::tempdir()?;
        let library = workspace.path().join("root");
        let active = workspace.path().join("active");
        let absent = workspace.path().join("historical-sdk");
        let external = workspace.path().join("external");
        let decoy = workspace.path().join("sdk-external-decoy");
        let support = workspace.path().join("support");
        let dev_support = workspace.path().join("dev-support");
        let generated = workspace.path().join("generated");
        let mut records = Vec::new();
        let mut edges = Vec::new();
        for (name, value) in [
            ("incan_issue911_codecs", 7),
            ("incan_issue911_core", 8),
            ("incan_issue911_testing", 9),
        ] {
            let record = sdk_rebinding_fixture_record(
                &active.join(name),
                name,
                &format!("pub fn value() -> u8 {{ {value} }}\n"),
                true,
                Vec::new(),
            )?;
            edges.push(sdk_rebinding_fixture_edge(
                &record,
                &format!("../historical-sdk/{name}"),
                true,
            ));
            records.push(record);
        }
        let ordinary = sdk_rebinding_fixture_record(
            &external,
            "incan_issue911_external",
            "pub fn value() -> u8 { 10 }\n",
            false,
            Vec::new(),
        )?;
        edges.push(sdk_rebinding_fixture_edge(&ordinary, "../external", false));
        records.push(sdk_rebinding_fixture_record(
            &decoy,
            "incan_issue911_external",
            "pub fn value() -> u8 { 99 }\n",
            true,
            Vec::new(),
        )?);
        // Rust-only support is an explicit fixture input, not a claim of general native dependency selection.
        sdk_rebinding_fixture_record(
            &support,
            "incan_issue911_support",
            "pub fn value() -> u8 { 5 }\n",
            false,
            Vec::new(),
        )?;
        sdk_rebinding_fixture_record(
            &dev_support,
            "incan_issue911_dev_support",
            "pub fn value() -> u8 { 6 }\n",
            false,
            Vec::new(),
        )?;
        records.push(sdk_rebinding_fixture_record(&library, "root_lib", "pub fn root_value() -> u8 { incan_issue911_codecs::value() + incan_issue911_core::value() + incan_issue911_testing::value() + incan_issue911_external::value() + incan_issue911_support::value() }\n", false, edges)?);
        let roots = [&library, &active, &external, &decoy, &support, &dev_support];
        let roots = roots.iter().map(|root| root.as_path()).collect::<Vec<_>>();
        let before = sdk_rebinding_fixture_inventory(&roots)?;
        assert!(!absent.exists());
        let plan = ProviderPlan::new(LibraryManifestIndex::default(), records, [])?;
        assert_eq!(plan.sdk_dependency_rebindings().len(), 3);
        assert_eq!(plan.sdk_artifact_projections().len(), 1);
        let mut selected = Vec::new();
        for binding in plan.sdk_dependency_rebindings() {
            assert_eq!(binding.containing_artifact.crate_root, library);
            assert_eq!(
                binding.active_crate_root,
                fs::canonicalize(active.join(&binding.provider_name))?
            );
            assert_eq!(
                binding.source_crate_root,
                library.join(format!("../historical-sdk/{}", binding.provider_name))
            );
            assert_ne!(binding.dependency_key, "incan_issue911_external");
            assert!(!binding.active_crate_root.starts_with(&support));
            assert!(!binding.active_crate_root.starts_with(&dev_support));
            selected.push((binding.dependency_key.clone(), binding.active_crate_root.clone()));
        }
        let external_artifact = plan
            .public_artifacts()
            .find(|artifact| artifact.identity == ordinary.identity)
            .ok_or("checked ordinary dependency absent")?;
        assert_eq!(external_artifact.artifact.crate_root, fs::canonicalize(&external)?);
        selected.push((
            "incan_issue911_external".to_string(),
            external_artifact.artifact.crate_root.clone(),
        ));
        selected.push(("incan_issue911_support".to_string(), support.clone()));
        let mut generator = ProjectGenerator::new(&generated, "issue911_consumer", true);
        generator.set_provider_plan(&plan);
        generator.generate("fn main() { assert_eq!(root_lib::root_value(), 39); }")?;
        assert!(!absent.exists());
        let direct = workspace.path().join("direct");
        fs::create_dir_all(&direct)?;
        let mut compiled = Vec::new();
        for (name, root) in selected {
            let output = direct.join(format!("lib{name}.rlib"));
            compile_rebound_fixture_with_rustc(&root.join("src/lib.rs"), &name, "lib", &output, &[])?;
            compiled.push((name, output));
        }
        let externs = compiled
            .iter()
            .map(|(name, path)| (name.as_str(), path.as_path()))
            .collect::<Vec<_>>();
        let root_output = direct.join("libroot_lib.rlib");
        compile_rebound_fixture_with_rustc(&library.join("src/lib.rs"), "root_lib", "lib", &root_output, &externs)?;
        let consumer = direct.join("consumer");
        let mut consumer_externs = externs;
        consumer_externs.push(("root_lib", root_output.as_path()));
        compile_rebound_fixture_with_rustc(
            &generated.join("src/main.rs"),
            "issue911_consumer",
            "bin",
            &consumer,
            &consumer_externs,
        )?;
        let execution = Command::new(&consumer).output()?;
        if !execution.status.success() {
            return Err(format!(
                "native39 fixture failed: {}",
                String::from_utf8_lossy(&execution.stderr)
            )
            .into());
        }
        assert!(!absent.exists());
        assert_eq!(sdk_rebinding_fixture_inventory(&roots)?, before);
        Ok(())
    }

    #[test]
    fn nested_compiled_artifacts_propagate_sdk_projection_issue911() -> Result<(), Box<dyn std::error::Error>> {
        let workspace = tempfile::tempdir()?;
        let active = workspace.path().join("active");
        let absent = workspace.path().join("historical-sdk");
        let child = workspace.path().join("child");
        let root = workspace.path().join("root");
        let generated = workspace.path().join("generated");
        let runtime = sdk_rebinding_fixture_record(
            &active,
            "incan_issue911_runtime",
            "pub fn value() -> u8 { 11 }\n",
            true,
            Vec::new(),
        )?;
        let child_record = sdk_rebinding_fixture_record(
            &child,
            "issue911_child",
            "pub fn child_value() -> u8 { incan_issue911_runtime::value() }\n",
            false,
            vec![sdk_rebinding_fixture_edge(&runtime, "../historical-sdk", true)],
        )?;
        let mut public_edge = sdk_rebinding_fixture_edge(&child_record, "../child", false);
        public_edge.default_features = true;
        public_edge.optional = true;
        let root_record = sdk_rebinding_fixture_record(
            &root,
            "issue911_root",
            "pub fn root_value() -> u8 { issue911_child::child_value() }\n",
            false,
            vec![public_edge.clone()],
        )?;
        let before = sdk_rebinding_fixture_inventory(&[&active, &child, &root])?;
        let plan = ProviderPlan::new(LibraryManifestIndex::default(), vec![root_record, runtime], [])?;
        assert_eq!(plan.sdk_dependency_rebindings().len(), 1);
        assert_eq!(plan.sdk_artifact_projections().len(), 2);
        let projected_roots = plan
            .sdk_artifact_projections()
            .iter()
            .map(|projection| fs::canonicalize(&projection.artifact.crate_root))
            .collect::<Result<BTreeSet<_>, _>>()?;
        for expected in [&root, &child] {
            assert!(projected_roots.contains(&fs::canonicalize(expected)?));
        }
        let binding = &plan.sdk_dependency_rebindings()[0];
        assert_eq!(binding.containing_artifact.crate_root, fs::canonicalize(&child)?);
        assert_eq!(binding.active_crate_root, fs::canonicalize(&active)?);
        let admitted_child = plan
            .public_artifacts()
            .find(|artifact| artifact.identity == child_record.identity)
            .ok_or("checked child absent")?;
        assert_eq!(admitted_child.artifact.crate_root, fs::canonicalize(&child)?);
        let root_manifest = LibraryManifest::read_from_path(&root.join("issue911_root.incnlib"))?;
        assert_eq!(
            root_manifest.contract_metadata.provider.provider_dependencies,
            vec![public_edge]
        );
        let mut generator = ProjectGenerator::new(&generated, "issue911_nested_consumer", true);
        generator.set_provider_plan(&plan);
        generator.generate("fn main() { assert_eq!(issue911_root::root_value(), 11); }")?;
        let direct = workspace.path().join("direct");
        fs::create_dir_all(&direct)?;
        let runtime_output = direct.join("libincan_issue911_runtime.rlib");
        let child_output = direct.join("libissue911_child.rlib");
        let root_output = direct.join("libissue911_root.rlib");
        let consumer = direct.join("consumer");
        compile_rebound_fixture_with_rustc(
            &binding.active_crate_root.join("src/lib.rs"),
            &binding.dependency_key,
            "lib",
            &runtime_output,
            &[],
        )?;
        compile_rebound_fixture_with_rustc(
            &admitted_child.artifact.crate_root.join("src/lib.rs"),
            "issue911_child",
            "lib",
            &child_output,
            &[(binding.dependency_key.as_str(), runtime_output.as_path())],
        )?;
        compile_rebound_fixture_with_rustc(
            &root.join("src/lib.rs"),
            "issue911_root",
            "lib",
            &root_output,
            &[("issue911_child", child_output.as_path())],
        )?;
        compile_rebound_fixture_with_rustc(
            &generated.join("src/main.rs"),
            "issue911_nested_consumer",
            "bin",
            &consumer,
            &[
                ("issue911_root", root_output.as_path()),
                ("issue911_child", child_output.as_path()),
                ("incan_issue911_runtime", runtime_output.as_path()),
            ],
        )?;
        let execution = Command::new(&consumer).output()?;
        if !execution.status.success() {
            return Err(format!(
                "native11 fixture failed: {}",
                String::from_utf8_lossy(&execution.stderr)
            )
            .into());
        }
        assert!(!absent.exists());
        assert_eq!(sdk_rebinding_fixture_inventory(&[&active, &child, &root])?, before);
        Ok(())
    }

    #[test]
    fn sdk_projection_rejects_checked_identity_and_feature_mismatch_before_generation_issue911()
    -> Result<(), Box<dyn std::error::Error>> {
        let workspace = tempfile::tempdir()?;
        let active = workspace.path().join("active");
        let artifact = workspace.path().join("library");
        let absent = workspace.path().join("historical-sdk");
        let generated = workspace.path().join("generated");
        let runtime = sdk_rebinding_fixture_record(
            &active,
            "incan_issue911_runtime",
            "pub fn value() {}\n",
            true,
            Vec::new(),
        )?;
        let edge = sdk_rebinding_fixture_edge(&runtime, "../historical-sdk", true);
        let record =
            sdk_rebinding_fixture_record(&artifact, "issue911_root", "pub fn value() {}\n", false, vec![edge])?;
        let before = sdk_rebinding_fixture_inventory(&[&active, &artifact])?;
        for dimension in ["version", "digest", "features", "default-features", "optional"] {
            let mut changed = record.clone();
            let manifest = std::sync::Arc::make_mut(changed.manifest.as_mut().ok_or("fixture manifest absent")?);
            let dependency = &mut manifest.contract_metadata.provider.provider_dependencies[0];
            match dimension {
                "version" => dependency.provider_version = "9.9.9".to_string(),
                "digest" => dependency.artifact_digest = "sha256:wrong".to_string(),
                "features" => {
                    dependency.requested_features.insert("unexpected".to_string());
                }
                "default-features" => dependency.default_features = true,
                _ => dependency.optional = true,
            }
            let error = ProviderPlan::new(LibraryManifestIndex::default(), vec![changed, runtime.clone()], [])
                .err()
                .ok_or("checked mismatch must refuse")?;
            assert!(
                matches!(error, ProviderPlanError::IncompatibleCompiledSdkDependency { .. }),
                "{dimension}: {error}"
            );
            assert!(!generated.exists());
            assert!(!absent.exists());
        }
        let plan = ProviderPlan::new(LibraryManifestIndex::default(), vec![record, runtime], [])?;
        let mut generator = ProjectGenerator::new(&generated, "consumer", true);
        generator.set_provider_plan(&plan);
        generator.generate("fn main() {}")?;
        assert_eq!(plan.sdk_dependency_rebindings().len(), 1);
        assert!(!absent.exists());
        assert_eq!(sdk_rebinding_fixture_inventory(&[&active, &artifact])?, before);
        Ok(())
    }

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
    fn sdk_provider_crate_lints_suppress_only_generated_representation_warnings() {
        let library = ProjectGenerator::new("target/test-provider-lints", "provider", false);
        let mut provider_root = format!("// Generated\n\n{MOD_INSERT_MARKER}\n");
        library.add_sdk_provider_crate_lints(&mut provider_root, true);
        assert_eq!(
            provider_root,
            format!("// Generated\n\n#![allow(dead_code, non_upper_case_globals)]\n{MOD_INSERT_MARKER}\n")
        );

        let mut ordinary_library_root = MOD_INSERT_MARKER.to_string();
        library.add_sdk_provider_crate_lints(&mut ordinary_library_root, false);
        assert_eq!(ordinary_library_root, MOD_INSERT_MARKER);

        let binary = ProjectGenerator::new("target/test-provider-lints", "provider", true);
        let mut binary_root = MOD_INSERT_MARKER.to_string();
        binary.add_sdk_provider_crate_lints(&mut binary_root, true);
        assert_eq!(binary_root, MOD_INSERT_MARKER);
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

        let mut generator = ProjectGenerator::new(&temp_dir, "test_nested_keyword_modules", false);
        generator.public_namespace_facades = Some(BTreeMap::from([
            (vec!["api".to_string()], BTreeSet::from(["async".to_string()])),
            (vec!["type".to_string()], BTreeSet::from(["helpers".to_string()])),
        ]));

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
        assert!(mod_rs_content.contains("pub use r#async::*;"));

        let async_content = fs::read_to_string(temp_dir.join("src/api/async.rs"))?;
        assert!(async_content.contains("pub fn launch"));

        let type_mod_rs_content = fs::read_to_string(temp_dir.join("src/type/mod.rs"))?;
        assert!(type_mod_rs_content.contains("pub mod helpers;"));

        let _ = fs::remove_dir_all(&temp_dir);
        Ok(())
    }

    #[test]
    fn checked_namespace_facades_skip_children_fully_bound_by_the_parent_issue948() {
        use crate::frontend::api_metadata::{
            ApiAlias, ApiDeclaration, CHECKED_API_METADATA_SCHEMA_VERSION, CheckedApiMetadata,
            CheckedApiMetadataPackage, CheckedApiPublicNamespace, SourceAnchor, SourceSpan,
        };

        let alias = |name: &str, target: &[&str], is_public| {
            ApiDeclaration::Alias(ApiAlias {
                name: name.to_string(),
                anchor: SourceAnchor {
                    id: format!("traits::{name}"),
                    span: SourceSpan { start: 0, end: 0 },
                },
                target_path: target.iter().map(|segment| (*segment).to_string()).collect(),
                is_public,
                projected_type: None,
                projected_function: None,
            })
        };
        let api = CheckedApiMetadataPackage {
            schema_version: CHECKED_API_METADATA_SCHEMA_VERSION,
            package: None,
            modules: vec![
                CheckedApiMetadata {
                    schema_version: CHECKED_API_METADATA_SCHEMA_VERSION,
                    derivable_traits: Vec::new(),
                    module_path: vec!["traits".to_string()],
                    declarations: vec![alias("SharedItem", &["traits", "covered", "SharedItem"], false)],
                },
                CheckedApiMetadata {
                    schema_version: CHECKED_API_METADATA_SCHEMA_VERSION,
                    derivable_traits: Vec::new(),
                    module_path: vec!["traits".to_string(), "covered".to_string()],
                    declarations: vec![alias("SharedItem", &["traits", "covered", "SharedItem"], true)],
                },
                CheckedApiMetadata {
                    schema_version: CHECKED_API_METADATA_SCHEMA_VERSION,
                    derivable_traits: Vec::new(),
                    module_path: vec!["traits".to_string(), "partial".to_string()],
                    declarations: vec![
                        alias("SharedItem", &["traits", "partial", "SharedItem"], true),
                        alias("Extra", &["traits", "partial", "Extra"], true),
                    ],
                },
            ],
            public_namespaces: vec![CheckedApiPublicNamespace {
                path: vec!["traits".to_string()],
                members: Vec::new(),
                child_modules: vec!["covered".to_string(), "partial".to_string()],
            }],
        };
        let mut generator = ProjectGenerator::new("target/test-checked-namespace-facades", "provider", false);

        generator.set_public_namespace_facades(&api);

        assert_eq!(
            generator.public_namespace_facades,
            Some(BTreeMap::from([(
                vec!["traits".to_string()],
                BTreeSet::from(["partial".to_string()])
            )]))
        );
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

        let mut generator = ProjectGenerator::new(&temp_dir, "test_cleanup", false);
        generator.public_namespace_facades = Some(BTreeMap::from([(
            vec!["dataset".to_string()],
            BTreeSet::from(["ops".to_string()]),
        )]));

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
        let dataset_facade = fs::read_to_string(temp_dir.join("src/dataset/mod.rs"))?;
        assert!(
            dataset_facade.contains("pub use ops::*;"),
            "published directory modules should expose public declarations from immediate source-file children"
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
