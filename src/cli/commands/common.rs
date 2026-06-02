//! Shared utilities used across multiple CLI command pipelines.
//!
//! This module contains functions for source file reading, module collection, project root resolution,
//! dependency helpers, and Cargo flag construction.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

#[cfg(feature = "rust_inspect")]
use crate::backend::ProjectGenerator;
use crate::backend::ir::detect_serde_non_import_usage;
use crate::cli::prelude::ParsedModule;
use crate::cli::{CliError, CliResult};
use crate::dependency_resolver::ResolvedDependencies;
use crate::dependency_resolver::{DependencyError, InlineRustImport};
use crate::frontend::ast::{ImportKind, Program, Span};
use crate::frontend::contract_metadata::{
    CanonicalModelBundle, materialize_contract_models, read_project_model_bundles,
};
use crate::frontend::library_manifest_index::LibraryManifestIndex;
use crate::frontend::module::{
    SourceModuleImportResolution, canonicalize_source_module_segments, resolve_program_source_imports,
};
use crate::frontend::{ast_walk, diagnostics, lexer, parser, typechecker, vocab_desugar_pass};
use crate::lockfile::CargoFeatureSelection;
use crate::manifest::ProjectManifest;
use crate::manifest::{DependencySource, DependencySpec};
use crate::project_lifecycle::toolchain::ToolchainConstraintSet;
#[cfg(feature = "rust_inspect")]
use crate::rust_inspect::{Inspector, InspectorConfig};
use incan_core::lang::{
    stdlib::{self, StdlibExtraCrateDep, StdlibExtraCrateSource},
    surface::result_methods,
};
#[cfg(feature = "rust_inspect")]
use sha2::{Digest, Sha256};

/// Maximum source file size (100 MB)
///
/// Files larger than this are rejected to prevent out-of-memory conditions during compilation.
const MAX_SOURCE_SIZE: u64 = 100 * 1024 * 1024;

/// Unified project requirements collected from parsed modules and loaded provider manifests.
#[derive(Debug, Clone, Default)]
pub(crate) struct ProjectRequirements {
    /// Required stdlib feature flags, such as `json`, `async`, and `web`.
    pub stdlib_features: Vec<String>,
    /// Required Cargo dependencies contributed by stdlib namespaces and provider manifests.
    pub dependencies: Vec<DependencySpec>,
}

/// Cargo execution policy resolved from CLI inputs and environment defaults.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CargoPolicy {
    pub(crate) offline: bool,
    pub(crate) locked: bool,
    pub(crate) frozen: bool,
    pub(crate) extra_args: Vec<String>,
}

/// CLI policy flags, including explicit disables for environment defaults.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct CargoPolicyCliFlags {
    pub offline: bool,
    pub no_offline: bool,
    pub locked: bool,
    pub no_locked: bool,
    pub frozen: bool,
    pub no_frozen: bool,
}

impl CargoPolicy {
    /// Resolve policy for a user-facing build/run/test command.
    pub(crate) fn from_cli_and_env(
        cli_flags: CargoPolicyCliFlags,
        cli_cargo_args: Vec<String>,
        cli_passthrough_args: Vec<String>,
    ) -> Self {
        Self::from_sources(cli_flags, cli_cargo_args, cli_passthrough_args, |name| {
            env::var(name).ok()
        })
    }

    /// Build an explicit policy for internal Cargo invocations that should not read RFC 020 env defaults.
    pub(crate) fn explicit(offline: bool, locked: bool, frozen: bool, extra_args: Vec<String>) -> Self {
        let mut policy = Self {
            offline,
            locked,
            frozen,
            extra_args,
        };
        policy.normalize();
        policy
    }

    /// Resolve policy from injected sources; used by tests to avoid mutating process env.
    fn from_sources<F>(
        cli_flags: CargoPolicyCliFlags,
        mut cli_cargo_args: Vec<String>,
        cli_passthrough_args: Vec<String>,
        env_value: F,
    ) -> Self
    where
        F: Fn(&str) -> Option<String>,
    {
        let env_frozen = env_flag_value(env_value("INCAN_FROZEN").as_deref());
        let env_offline = env_flag_value(env_value("INCAN_OFFLINE").as_deref());
        let env_locked = env_flag_value(env_value("INCAN_LOCKED").as_deref());

        cli_cargo_args.extend(cli_passthrough_args);
        let extra_args = if cli_cargo_args.is_empty() {
            split_env_cargo_args(env_value("INCAN_CARGO_ARGS").as_deref())
        } else {
            cli_cargo_args
        };

        Self::explicit(
            resolve_cli_env_flag(env_offline, cli_flags.offline, cli_flags.no_offline),
            resolve_cli_env_flag(env_locked, cli_flags.locked, cli_flags.no_locked),
            resolve_cli_env_flag(env_frozen, cli_flags.frozen, cli_flags.no_frozen),
            extra_args,
        )
    }

    /// Apply derived policy semantics after raw source resolution.
    fn normalize(&mut self) {
        if self.frozen {
            self.offline = true;
            self.locked = true;
        }
    }
}

/// Enforce the project-level `requires-incan` constraint for a project-aware command.
pub(crate) fn enforce_project_toolchain_constraint(manifest: &ProjectManifest) -> CliResult<()> {
    enforce_toolchain_constraints(&ToolchainConstraintSet::from_project_manifest(manifest))
}

/// Enforce an already-resolved effective `requires-incan` constraint set.
pub(crate) fn enforce_toolchain_constraints(constraints: &ToolchainConstraintSet) -> CliResult<()> {
    constraints
        .enforce_current()
        .map_err(|error| CliError::failure(error.to_string()))
}

/// Resolve one boolean policy input with CLI enable/disable flags over env defaults.
fn resolve_cli_env_flag(env_default: bool, cli_enable: bool, cli_disable: bool) -> bool {
    if cli_enable {
        true
    } else if cli_disable {
        false
    } else {
        env_default
    }
}

/// Parse a boolean RFC 020 environment flag value.
fn env_flag_value(value: Option<&str>) -> bool {
    value.is_some_and(|value| matches!(value, "1" | "true" | "TRUE" | "on" | "ON"))
}

/// Split `INCAN_CARGO_ARGS` using the RFC 020 whitespace-only rule.
fn split_env_cargo_args(value: Option<&str>) -> Vec<String> {
    value
        .into_iter()
        .flat_map(str::split_whitespace)
        .map(str::to_string)
        .collect()
}

/// Shared source-analysis context for CLI commands and the LSP.
///
/// This owns the project-level inputs that affect context-sensitive parsing and typechecking so entrypoints do not
/// independently rediscover manifests, library vocabulary, provider surfaces, or checked contract metadata.
#[derive(Debug, Clone)]
pub(crate) struct CompilationSession {
    #[cfg(feature = "lsp")]
    pub manifest: Option<ProjectManifest>,
    pub source_root: PathBuf,
    pub library_manifest_index: LibraryManifestIndex,
    pub library_imported_vocab: parser::ImportedLibraryVocab,
    pub library_imported_dsl_surfaces: parser::ImportedLibraryDslSurfaces,
    pub contract_model_bundles: Vec<CanonicalModelBundle>,
}

impl CompilationSession {
    /// Discover project-level compilation context for an entry source path.
    pub(crate) fn discover(entry_path: &Path) -> CliResult<Self> {
        let inferred_project_root = resolve_project_root(entry_path);
        let manifest =
            ProjectManifest::discover(&inferred_project_root).map_err(|error| CliError::failure(error.to_string()))?;
        let project_root = manifest
            .as_ref()
            .map(|manifest| manifest.project_root().to_path_buf())
            .unwrap_or(inferred_project_root);
        let source_root = resolve_source_root(&project_root, manifest.as_ref());
        let library_manifest_index = manifest
            .as_ref()
            .and_then(|manifest| {
                (!manifest.library_dependencies().is_empty())
                    .then(|| LibraryManifestIndex::from_project_manifest(manifest))
            })
            .unwrap_or_default();
        let library_imported_vocab = library_manifest_index.library_imported_vocab();
        let library_imported_dsl_surfaces = library_manifest_index.library_imported_dsl_surfaces();
        let contract_model_bundles = manifest
            .as_ref()
            .map(|manifest| read_project_model_bundles(&project_root, &manifest.contract_model_bundle_paths()))
            .transpose()
            .map_err(|error| CliError::failure(error.to_string()))?
            .unwrap_or_default();

        Ok(Self {
            #[cfg(feature = "lsp")]
            manifest,
            source_root,
            library_manifest_index,
            library_imported_vocab,
            library_imported_dsl_surfaces,
            contract_model_bundles,
        })
    }

    /// Return the Rust crate names declared by the project manifest, or an empty set outside a project.
    #[cfg(feature = "lsp")]
    pub(crate) fn declared_crate_names(&self) -> HashSet<String> {
        self.manifest
            .as_ref()
            .map(ProjectManifest::declared_rust_crate_names)
            .unwrap_or_default()
    }

    /// Lex and parse one source file using the project-aware vocabulary surfaces, without running desugarers or
    /// compile-time materialization passes.
    pub(crate) fn parse_source_for_collection(
        &self,
        file_path: &Path,
        source: &str,
    ) -> Result<Program, Vec<diagnostics::CompileError>> {
        let tokens = lexer::lex(source)?;
        let file_path_display = file_path.to_string_lossy();
        parser::parse_with_context_and_surfaces(
            &tokens,
            Some(file_path_display.as_ref()),
            Some(&self.library_imported_vocab),
            Some(&self.library_imported_dsl_surfaces),
        )
    }

    /// Lex, parse, vocab-desugar, and optionally materialize checked contract models for one source file.
    pub(crate) fn parse_source(
        &self,
        file_path: &Path,
        source: &str,
        materialize_models: bool,
    ) -> Result<Program, Vec<diagnostics::CompileError>> {
        let mut ast = self.parse_source_for_collection(file_path, source)?;
        let file_path_display = file_path.to_string_lossy();
        vocab_desugar_pass::desugar_program_vocab_blocks(
            &mut ast,
            Some(file_path_display.as_ref()),
            &self.library_manifest_index,
        )?;
        if materialize_models && let Err(error) = materialize_contract_models(&mut ast, &self.contract_model_bundles) {
            return Err(vec![diagnostics::CompileError::new(
                format!("Invalid checked contract metadata: {error}"),
                Span::default(),
            )]);
        }
        Ok(ast)
    }
}

/// Collect a unified set of project requirements from source imports and loaded provider manifests.
pub(crate) fn collect_project_requirements(
    modules: &[ParsedModule],
    library_manifest_index: &LibraryManifestIndex,
) -> CliResult<ProjectRequirements> {
    let mut stdlib_namespaces = HashSet::new();
    for module in modules {
        for decl in &module.ast.declarations {
            let crate::frontend::ast::Declaration::Import(import) = &decl.node else {
                continue;
            };
            let path = match &import.kind {
                ImportKind::From { module, .. } => {
                    if module.parent_levels > 0 || module.is_absolute {
                        continue;
                    }
                    &module.segments
                }
                ImportKind::Module(path) => {
                    if path.parent_levels > 0 || path.is_absolute {
                        continue;
                    }
                    &path.segments
                }
                _ => continue,
            };

            if path.len() < 2 || path[0] != stdlib::STDLIB_ROOT {
                continue;
            }
            stdlib_namespaces.insert(path[1].clone());
        }
    }

    // The legacy bare `json_stringify` builtin can still be used without importing `std.serde.*`. Keep this as an
    // explicit compatibility fallback, but treat import/provider manifests as the primary source of dependency and
    // feature requirements.
    let needs_legacy_serde_runtime = modules.iter().any(|module| detect_serde_non_import_usage(&module.ast));
    if needs_legacy_serde_runtime {
        stdlib_namespaces.insert("serde".to_string());
    }

    let mut stdlib_features: BTreeSet<String> = BTreeSet::new();
    for namespace_name in &stdlib_namespaces {
        let Some(namespace) = stdlib::find_namespace(namespace_name) else {
            continue;
        };
        if let Some(feature) = namespace.feature {
            stdlib_features.insert(feature.to_string());
        }
    }
    for feature in library_manifest_index.merged_provider_required_stdlib_features() {
        stdlib_features.insert(feature);
    }

    let mut requirements = ProjectRequirements {
        stdlib_features: stdlib_features.into_iter().collect(),
        dependencies: Vec::new(),
    };
    let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    for namespace_name in &stdlib_namespaces {
        let Some(namespace) = stdlib::find_namespace(namespace_name) else {
            continue;
        };
        for dep in namespace.extra_crate_deps {
            let spec = dependency_spec_from_stdlib_dep(dep, &workspace_root);
            merge_requirement_dependency(
                &mut requirements.dependencies,
                spec,
                format!("stdlib namespace `std.{namespace_name}`"),
            )?;
        }
    }

    let needs_serde_runtime = needs_legacy_serde_runtime || stdlib_namespaces.contains("serde");
    if needs_serde_runtime {
        let serde = dependency_spec_from_stdlib_extra_crate("serde")?;
        merge_requirement_dependency(
            &mut requirements.dependencies,
            serde,
            "std.serde usage in source".to_string(),
        )?;
    }

    for spec in library_manifest_index.cargo_path_dependencies() {
        merge_requirement_dependency(
            &mut requirements.dependencies,
            spec,
            "pub:: dependency artifact".to_string(),
        )?;
    }
    for spec in library_manifest_index
        .merged_provider_required_dependencies()
        .map_err(|err| CliError::failure(format!("failed to merge provider requirements: {err}")))?
    {
        merge_requirement_dependency(
            &mut requirements.dependencies,
            spec,
            "provider manifest requirement".to_string(),
        )?;
    }

    Ok(requirements)
}

/// Build a dependency specification from a stdlib extra crate requirement.
fn dependency_spec_from_stdlib_extra_crate(crate_name: &str) -> CliResult<DependencySpec> {
    let dep = stdlib::find_extra_crate_dep(crate_name).ok_or_else(|| {
        CliError::failure(format!(
            "stdlib dependency metadata for `{crate_name}` is missing from the registry"
        ))
    })?;
    let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    Ok(dependency_spec_from_stdlib_dep(dep, &workspace_root))
}

/// Build a dependency specification from a stdlib dependency requirement.
fn dependency_spec_from_stdlib_dep(dep: &StdlibExtraCrateDep, workspace_root: &Path) -> DependencySpec {
    match dep.source {
        StdlibExtraCrateSource::Version(version) => DependencySpec {
            crate_name: dep.crate_name.to_string(),
            version: Some(version.to_string()),
            features: dep.features.iter().map(|feature| (*feature).to_string()).collect(),
            default_features: true,
            source: DependencySource::Registry,
            optional: false,
            package: stdlib::extra_crate_package_alias(dep.crate_name).map(str::to_string),
        },
        StdlibExtraCrateSource::Path(relative_path) => DependencySpec {
            crate_name: dep.crate_name.to_string(),
            version: None,
            features: dep.features.iter().map(|feature| (*feature).to_string()).collect(),
            default_features: true,
            source: DependencySource::Path {
                path: workspace_root.join(relative_path),
            },
            optional: false,
            package: None,
        },
    }
    .normalized()
}

/// Merge a dependency requirement into a collection of requirements.
///
/// Existing entries with the same crate name must be compatible.
fn merge_requirement_dependency(
    merged: &mut Vec<DependencySpec>,
    candidate: DependencySpec,
    source_label: String,
) -> CliResult<()> {
    if let Some(existing) = merged.iter().find(|dep| dep.crate_name == candidate.crate_name) {
        if existing != &candidate {
            return Err(CliError::failure(format!(
                "dependency requirement `{}` conflicts with existing collected requirements ({source_label})",
                candidate.crate_name
            )));
        }
        return Ok(());
    }
    merged.push(candidate);
    merged.sort_by(|left, right| left.crate_name.cmp(&right.crate_name));
    Ok(())
}

/// Merge collected requirement dependencies into resolved dependency sets.
///
/// Existing entries with the same crate name must be compatible.
pub(crate) fn merge_project_requirement_dependencies(
    resolved: &mut ResolvedDependencies,
    requirements: &ProjectRequirements,
) -> CliResult<()> {
    for required in &requirements.dependencies {
        let already_in_dependencies = resolved
            .dependencies
            .iter()
            .find(|spec| spec.crate_name == required.crate_name);
        if let Some(existing) = already_in_dependencies {
            if existing != required {
                return Err(CliError::failure(format!(
                    "dependency `{}` conflicts between resolved imports and collected project requirements",
                    required.crate_name
                )));
            }
            continue;
        }
        let already_in_dev = resolved
            .dev_dependencies
            .iter()
            .find(|spec| spec.crate_name == required.crate_name);
        if let Some(existing) = already_in_dev {
            if existing != required {
                return Err(CliError::failure(format!(
                    "dependency `{}` conflicts between dev dependencies and collected project requirements",
                    required.crate_name
                )));
            }
            continue;
        }
        resolved.dependencies.push(required.clone());
    }
    resolved
        .dependencies
        .sort_by(|left, right| left.crate_name.cmp(&right.crate_name));
    Ok(())
}

/// Merge project-level dependency requirements into the resolved dependency set.
pub(crate) fn merge_project_requirements(
    current: &ProjectRequirements,
    extra: &ProjectRequirements,
) -> CliResult<ProjectRequirements> {
    let stdlib_features = current
        .stdlib_features
        .iter()
        .chain(extra.stdlib_features.iter())
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();

    let mut dependencies = current.dependencies.clone();
    for candidate in &extra.dependencies {
        if let Some(existing) = dependencies.iter().find(|dep| dep.crate_name == candidate.crate_name) {
            if existing != candidate {
                return Err(CliError::failure(format!(
                    "dependency requirement `{}` conflicts between project requirement contexts",
                    candidate.crate_name
                )));
            }
            continue;
        }
        dependencies.push(candidate.clone());
    }
    dependencies.sort_by(|left, right| left.crate_name.cmp(&right.crate_name));

    Ok(ProjectRequirements {
        stdlib_features,
        dependencies,
    })
}

/// Merge resolved dependency requirements from multiple sources.
pub(crate) fn merge_resolved_dependencies(
    current: &ResolvedDependencies,
    extra: &ResolvedDependencies,
) -> CliResult<ResolvedDependencies> {
    let mut merged = current.clone();
    for candidate in &extra.dependencies {
        merge_resolved_dependency(&mut merged.dependencies, &mut merged.dev_dependencies, candidate, false)?;
    }
    for candidate in &extra.dev_dependencies {
        merge_resolved_dependency(&mut merged.dependencies, &mut merged.dev_dependencies, candidate, true)?;
    }
    merged
        .dependencies
        .sort_by(|left, right| left.crate_name.cmp(&right.crate_name));
    merged
        .dev_dependencies
        .sort_by(|left, right| left.crate_name.cmp(&right.crate_name));
    Ok(merged)
}

/// Merge one resolved dependency requirement into the dependency map.
fn merge_resolved_dependency(
    dependencies: &mut Vec<DependencySpec>,
    dev_dependencies: &mut Vec<DependencySpec>,
    candidate: &DependencySpec,
    dev_only: bool,
) -> CliResult<()> {
    if let Some(existing) = dependencies.iter().find(|dep| dep.crate_name == candidate.crate_name) {
        if existing != candidate {
            return Err(CliError::failure(format!(
                "dependency `{}` conflicts between resolved dependency contexts",
                candidate.crate_name
            )));
        }
        return Ok(());
    }

    if dev_only {
        if let Some(existing) = dev_dependencies
            .iter()
            .find(|dep| dep.crate_name == candidate.crate_name)
        {
            if existing != candidate {
                return Err(CliError::failure(format!(
                    "dev dependency `{}` conflicts between resolved dependency contexts",
                    candidate.crate_name
                )));
            }
            return Ok(());
        }
        dev_dependencies.push(candidate.clone());
        return Ok(());
    }

    if let Some(existing_idx) = dev_dependencies
        .iter()
        .position(|dep| dep.crate_name == candidate.crate_name)
    {
        if dev_dependencies[existing_idx] != *candidate {
            return Err(CliError::failure(format!(
                "dependency `{}` conflicts between dependency and dev-dependency contexts",
                candidate.crate_name
            )));
        }
        dev_dependencies.remove(existing_idx);
    }
    dependencies.push(candidate.clone());
    Ok(())
}

#[cfg(feature = "rust_inspect")]
const RUST_INSPECT_WORKSPACE_FINGERPRINT_FILE: &str = ".incan_rust_inspect_fingerprint";

#[cfg(feature = "rust_inspect")]
const RUST_INSPECT_WORKSPACE_FINGERPRINT_PREFIX: &str = "v1:";

/// Counts how many times each rust-inspect stub workspace is fully regenerated instead of skipped via fingerprint.
///
/// Full lib tests run in parallel and other tests can legitimately create unrelated rust-inspect workspaces, so this
/// instrumentation is keyed by generated workspace path instead of using one process-wide counter.
#[cfg(all(test, feature = "rust_inspect"))]
static TEST_RUST_INSPECT_WORKSPACE_GENERATIONS: std::sync::LazyLock<
    std::sync::Mutex<std::collections::BTreeMap<PathBuf, u64>>,
> = std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::BTreeMap::new()));

/// Records a full rust-inspect workspace regeneration for the generated workspace path under test.
#[cfg(all(test, feature = "rust_inspect"))]
fn record_test_rust_inspect_workspace_generation(workspace_dir: &Path) {
    let mut counts = TEST_RUST_INSPECT_WORKSPACE_GENERATIONS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    *counts.entry(workspace_dir.to_path_buf()).or_default() += 1;
}

/// Returns the number of full rust-inspect workspace regenerations recorded for a generated workspace path.
#[cfg(all(test, feature = "rust_inspect"))]
fn test_rust_inspect_workspace_generations(workspace_dir: &Path) -> u64 {
    let counts = TEST_RUST_INSPECT_WORKSPACE_GENERATIONS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    counts.get(workspace_dir).copied().unwrap_or(0)
}

#[cfg(feature = "rust_inspect")]
fn normalized_stdlib_features_for_rust_inspect_fingerprint(features: &[String]) -> Vec<String> {
    let mut normalized: Vec<String> = features
        .iter()
        .map(|feature| feature.trim().to_string())
        .filter(|feature| !feature.is_empty())
        .collect();
    normalized.sort();
    normalized.dedup();
    normalized
}

#[cfg(feature = "rust_inspect")]
fn hash_dependency_spec_for_rust_inspect(hasher: &mut Sha256, spec: &DependencySpec) {
    use crate::manifest::GitReference;

    hasher.update(spec.crate_name.as_bytes());
    hasher.update(b"\0");
    match &spec.version {
        Some(v) => {
            hasher.update(b"ver\0");
            hasher.update(v.as_bytes());
            hasher.update(b"\0");
        }
        None => hasher.update(b"nover\0"),
    }
    let mut feats = spec.features.clone();
    feats.sort();
    for f in feats {
        hasher.update(f.as_bytes());
        hasher.update(b"\0");
    }
    hasher.update([if spec.default_features { 1 } else { 0 }]);
    hasher.update([if spec.optional { 1 } else { 0 }]);
    match &spec.package {
        Some(p) => {
            hasher.update(b"pkg\0");
            hasher.update(p.as_bytes());
            hasher.update(b"\0");
        }
        None => hasher.update(b"nopkg\0"),
    }
    match &spec.source {
        DependencySource::Registry => hasher.update(b"src_reg\0"),
        DependencySource::Git { url, reference } => {
            hasher.update(b"src_git\0");
            hasher.update(url.as_bytes());
            hasher.update(b"\0");
            match reference {
                GitReference::Branch(s) => {
                    hasher.update(b"git_br\0");
                    hasher.update(s.as_bytes());
                    hasher.update(b"\0");
                }
                GitReference::Tag(s) => {
                    hasher.update(b"git_tag\0");
                    hasher.update(s.as_bytes());
                    hasher.update(b"\0");
                }
                GitReference::Rev(s) => {
                    hasher.update(b"git_rev\0");
                    hasher.update(s.as_bytes());
                    hasher.update(b"\0");
                }
            }
        }
        DependencySource::Path { path } => {
            hasher.update(b"src_path\0");
            hasher.update(path.as_os_str().as_encoded_bytes());
            hasher.update(b"\0");
        }
    }
    hasher.update(b"|dep|\0");
}

/// Stable fingerprint for inputs that define one generated rust-inspect Cargo workspace.
#[cfg(feature = "rust_inspect")]
fn rust_inspect_workspace_fingerprint(
    project_name: &str,
    rust_edition: Option<&str>,
    resolved: &ResolvedDependencies,
    stdlib_features: &[String],
    cargo_lock_payload: Option<&str>,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"incan_rust_inspect_workspace/1\0");
    hasher.update(project_name.as_bytes());
    hasher.update(b"\0");
    match rust_edition {
        Some(e) => {
            hasher.update(b"ed\0");
            hasher.update(e.as_bytes());
            hasher.update(b"\0");
        }
        None => hasher.update(b"noed\0"),
    }
    // Matches `ProjectGenerator::new(..., is_binary: true)` + `set_include_dev_dependencies(true)` for this workspace.
    hasher.update(b"layout_bin_devdeps\0");

    let stdlib = normalized_stdlib_features_for_rust_inspect_fingerprint(stdlib_features);
    for f in &stdlib {
        hasher.update(f.as_bytes());
        hasher.update(b"\0");
    }
    hasher.update(b"|\0");

    let mut deps = resolved.dependencies.clone();
    deps.sort_by(|a, b| a.crate_name.cmp(&b.crate_name));
    for dep in &mut deps {
        *dep = dep.clone().normalized();
    }
    hasher.update(b"deps\0");
    for dep in &deps {
        hash_dependency_spec_for_rust_inspect(&mut hasher, dep);
    }
    hasher.update(b"|\0");

    let mut dev_deps = resolved.dev_dependencies.clone();
    dev_deps.sort_by(|a, b| a.crate_name.cmp(&b.crate_name));
    for dep in &mut dev_deps {
        *dep = dep.clone().normalized();
    }
    hasher.update(b"dev_deps\0");
    for dep in &dev_deps {
        hash_dependency_spec_for_rust_inspect(&mut hasher, dep);
    }
    hasher.update(b"|\0");

    match cargo_lock_payload {
        Some(lock) => {
            hasher.update(b"lock\0");
            hasher.update(lock.as_bytes());
        }
        None => hasher.update(b"nolock\0"),
    }

    format!(
        "{}{}",
        RUST_INSPECT_WORKSPACE_FINGERPRINT_PREFIX,
        hex::encode(hasher.finalize())
    )
}

/// Return the workspace directory used for Rust inspection metadata.
#[cfg(feature = "rust_inspect")]
fn rust_inspect_workspace_dir(project_root: &Path, project_name: &str, fingerprint: &str) -> PathBuf {
    let mut safe_name = project_name
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();
    if safe_name.is_empty() {
        safe_name.push_str("project");
    }
    let suffix = fingerprint
        .rsplit_once(':')
        .map(|(_, hash)| hash)
        .unwrap_or(fingerprint)
        .chars()
        .take(16)
        .collect::<String>();
    project_root
        .join("target")
        .join("incan_lock")
        .join("rust_inspect")
        .join(format!("{safe_name}-{suffix}"))
}

/// Generate the rust-inspect workspace that semantic Rust extraction should query for this project.
///
/// The generated workspace intentionally uses the Rust import spelling for dependency keys, while preserving the
/// published Cargo package name separately when the two differ.
///
/// When the same inputs are seen again (for example across multiple `incan test` cases in one package), regeneration is
/// skipped if the namespaced workspace fingerprint matches the computed digest and expected artifacts exist.
#[cfg(feature = "rust_inspect")]
pub(crate) fn ensure_rust_inspect_workspace(
    project_root: &Path,
    project_name: &str,
    rust_edition: Option<String>,
    resolved: &ResolvedDependencies,
    project_requirements: &ProjectRequirements,
    cargo_lock_payload: Option<String>,
) -> CliResult<PathBuf> {
    let fingerprint = rust_inspect_workspace_fingerprint(
        project_name,
        rust_edition.as_deref(),
        resolved,
        &project_requirements.stdlib_features,
        cargo_lock_payload.as_deref(),
    );
    let rust_inspect_manifest_dir = rust_inspect_workspace_dir(project_root, project_name, &fingerprint);
    let fingerprint_path = rust_inspect_manifest_dir.join(RUST_INSPECT_WORKSPACE_FINGERPRINT_FILE);
    let cargo_toml_path = rust_inspect_manifest_dir.join("Cargo.toml");
    let main_rs_path = rust_inspect_manifest_dir.join("src").join("main.rs");

    let fingerprint_matches = match fs::read_to_string(&fingerprint_path) {
        Ok(existing) => existing.trim() == fingerprint.as_str(),
        Err(_) => false,
    };

    if cargo_toml_path.is_file() && main_rs_path.is_file() && fingerprint_matches {
        return Ok(rust_inspect_manifest_dir);
    }

    let mut generator = ProjectGenerator::new(&rust_inspect_manifest_dir, project_name, true);
    generator.set_dependencies(resolved.dependencies.clone());
    generator.set_dev_dependencies(resolved.dev_dependencies.clone());
    generator.set_include_dev_dependencies(true);
    generator.set_stdlib_features(project_requirements.stdlib_features.clone());
    generator.set_rust_edition(rust_edition);
    generator.set_cargo_lock_payload(cargo_lock_payload);
    let mut referenced_crates = std::collections::BTreeSet::new();
    for dep in resolved.dependencies.iter().chain(resolved.dev_dependencies.iter()) {
        referenced_crates.insert(dep.crate_name.replace('-', "_"));
    }
    let mut rust_inspect_stub = String::new();
    for crate_name in referenced_crates {
        rust_inspect_stub.push_str(format!("use {crate_name} as _;\n").as_str());
    }
    rust_inspect_stub.push_str("fn main() {}");

    #[cfg(all(test, feature = "rust_inspect"))]
    record_test_rust_inspect_workspace_generation(&rust_inspect_manifest_dir);

    generator.generate(rust_inspect_stub.as_str()).map_err(|e| {
        CliError::failure(format!(
            "Failed to generate rust-inspect lock project at {}: {e}",
            rust_inspect_manifest_dir.display()
        ))
    })?;

    if let Err(err) = fs::write(&fingerprint_path, &fingerprint) {
        return Err(CliError::failure(format!(
            "Failed to write rust-inspect workspace fingerprint {}: {err}",
            fingerprint_path.display()
        )));
    }

    Ok(rust_inspect_manifest_dir)
}

/// Collect canonical rust-inspect query paths from parsed `rust::` imports.
#[cfg(feature = "rust_inspect")]
pub(crate) fn collect_rust_inspect_query_paths(modules: &[ParsedModule]) -> Vec<String> {
    fn env_flag_enabled(name: &str) -> bool {
        std::env::var_os(name).is_some_and(|value| {
            let value = value.to_string_lossy();
            matches!(value.as_ref(), "1" | "true" | "TRUE" | "on" | "ON")
        })
    }

    // Default policy: prewarm explicit non-stdlib `from rust::... import Item` imports. These are the exact paths
    // semantic/codegen hot paths may query later, including Rust types with uppercase names.
    //
    // We still avoid crate/module imports and `incan_stdlib::*` by default. Full eager prewarm can force broad
    // rust-analyzer walks and persist negative module lookups that are not safe metadata items.
    // Set `INCAN_RUST_INSPECT_PREWARM_ALL=1` to restore full eager prewarm for debugging/regressions.
    let prewarm_all = env_flag_enabled("INCAN_RUST_INSPECT_PREWARM_ALL");
    let mut paths: BTreeSet<String> = BTreeSet::new();
    for module in modules {
        for decl in &module.ast.declarations {
            let crate::frontend::ast::Declaration::Import(import) = &decl.node else {
                continue;
            };
            match &import.kind {
                ImportKind::RustCrate { crate_name, path, .. } if prewarm_all => {
                    let mut segments = Vec::with_capacity(path.len() + 1);
                    segments.push(crate_name.replace('-', "_"));
                    segments.extend(path.iter().cloned());
                    if !segments.is_empty() {
                        paths.insert(segments.join("::"));
                    }
                }
                ImportKind::RustCrate { .. } => {}
                ImportKind::RustFrom {
                    crate_name,
                    path,
                    items,
                    ..
                } => {
                    let mut base = Vec::with_capacity(path.len() + 1);
                    base.push(crate_name.replace('-', "_"));
                    base.extend(path.iter().cloned());
                    let base = base.join("::");
                    if base.is_empty() {
                        continue;
                    }
                    if !prewarm_all && base.starts_with("incan_stdlib::") {
                        continue;
                    }
                    let primitive_ns = matches!(base.as_str(), "std::primitive" | "core::primitive");
                    for item in items {
                        if !primitive_ns {
                            paths.insert(format!("{base}::{}", item.name));
                        }
                    }
                }
                _ => {}
            }
        }
    }
    paths.into_iter().collect()
}

/// Return whether rust-inspect prewarm should run for the supplied environment value.
#[cfg(feature = "rust_inspect")]
fn parse_rust_inspect_prewarm_env(raw: Option<&str>) -> bool {
    let Some(raw) = raw else {
        return true;
    };
    !matches!(raw.trim(), "0" | "false" | "FALSE" | "off" | "OFF" | "no" | "NO")
}

/// Return whether Rust inspection prewarming is enabled.
#[cfg(feature = "rust_inspect")]
fn rust_inspect_prewarm_enabled() -> bool {
    parse_rust_inspect_prewarm_env(std::env::var("INCAN_RUST_INSPECT_PREWARM").ok().as_deref())
}

/// Surface rust-inspect preparation progress from explicit CLI prewarm phases.
#[cfg(feature = "rust_inspect")]
fn print_rust_inspect_prewarm_progress(message: String) {
    if message.starts_with("rust-inspect prewarm") {
        eprintln!("{message}");
    }
}

/// Eagerly load rust-inspect metadata before typechecking/codegen hot paths.
///
/// Prewarm defaults to enabled because lazy rust-analyzer extraction can dominate warm CLI runs.
/// Set `INCAN_RUST_INSPECT_PREWARM=0` to disable it for troubleshooting.
#[cfg(feature = "rust_inspect")]
pub(crate) fn prewarm_rust_inspect_workspace(manifest_dir: &Path, query_paths: &[String]) -> CliResult<()> {
    if !rust_inspect_prewarm_enabled() {
        return Ok(());
    }
    if query_paths.is_empty() {
        return Ok(());
    }
    let inspector = Inspector::new(InspectorConfig::new(manifest_dir.to_path_buf()));
    inspector
        .prewarm(query_paths.iter().cloned(), &print_rust_inspect_prewarm_progress)
        .map_err(|err| {
            CliError::failure(format!(
                "failed to prewarm rust-inspect cache from {}: {err}",
                manifest_dir.display()
            ))
        })
}

/// Resolve the source path for a stdlib module path (e.g. `["std", "testing"]`).
pub(crate) fn resolve_stdlib_module_source_path(module_path: &[String]) -> CliResult<PathBuf> {
    let Some(relative_stub_path) = stdlib::stdlib_stub_path(module_path) else {
        return Err(CliError::failure(format!(
            "Cannot resolve source for non-stdlib module path '{}'.",
            module_path.join(".")
        )));
    };

    let stdlib_relative = relative_stub_path
        .strip_prefix("stdlib/")
        .unwrap_or(relative_stub_path.as_str());
    let mut candidates: Vec<PathBuf> = Vec::new();

    if let Some(stdlib_dir) = crate::cli::prelude::find_stdlib_dir() {
        candidates.push(stdlib_dir.join(stdlib_relative));
    }
    candidates.push(PathBuf::from(&relative_stub_path));
    candidates.push(PathBuf::from("crates/incan_stdlib").join(&relative_stub_path));

    for candidate in candidates {
        if candidate.exists() {
            return Ok(candidate);
        }
    }

    Err(CliError::failure(format!(
        "Cannot resolve source file for '{}'; expected '{}' under stdlib search roots.",
        module_path.join("."),
        relative_stub_path
    )))
}

/// Read source file contents.
///
/// ## Errors
///
/// Returns an error if:
/// - The file cannot be read (I/O error)
/// - The file exceeds `MAX_SOURCE_SIZE` (100 MB)
pub fn read_source(file_path: &str) -> CliResult<String> {
    // Check file size before reading
    let metadata =
        fs::metadata(file_path).map_err(|e| CliError::failure(format!("Cannot access file '{}': {}", file_path, e)))?;

    if metadata.len() > MAX_SOURCE_SIZE {
        return Err(CliError::failure(format!(
            "Source file '{}' is too large ({} bytes, max {} bytes)",
            file_path,
            metadata.len(),
            MAX_SOURCE_SIZE
        )));
    }

    fs::read_to_string(file_path).map_err(|e| CliError::failure(format!("Error reading file '{}': {}", file_path, e)))
}

/// Return whether a parsed module uses RFC 088 iterator surface methods that require stdlib adapter modules.
pub(crate) fn uses_iterator_adapter_surface(program: &Program) -> bool {
    ast_walk::any_expr_in_program(program, |expr| match expr {
        crate::frontend::ast::Expr::MethodCall(_, method, _, _) => matches!(
            method.as_str(),
            "iter"
                | "map"
                | "filter"
                | "enumerate"
                | "zip"
                | "take"
                | "skip"
                | "take_while"
                | "skip_while"
                | "chain"
                | "flat_map"
                | "batch"
                | "collect"
                | "count"
                | "reduce"
                | "fold"
                | "any"
                | "all"
                | "find"
                | "for_each"
                | "sum"
        ),
        _ => false,
    })
}

/// Return whether a parsed module uses RFC 070 Result combinators backed by std.result helpers.
pub(crate) fn uses_result_combinator_surface(program: &Program) -> bool {
    ast_walk::any_expr_in_program(program, |expr| match expr {
        crate::frontend::ast::Expr::MethodCall(_, method, _, _) => result_methods::from_str(method).is_some(),
        _ => false,
    })
}

/// Collect and parse the entry file and all its dependencies.
///
/// # Note on Prelude
///
/// The stdlib root prelude (`stdlib/prelude.incn`) exists, but it is not auto-imported into every compilation unit.
/// Source-backed stdlib trait modules and builtin fallback traits are still discovered explicitly when the parsed AST
/// needs them.
pub fn collect_modules(entry_path: &str) -> CliResult<Vec<ParsedModule>> {
    let path = if Path::new(entry_path).is_absolute() {
        PathBuf::from(entry_path)
    } else {
        std::env::current_dir()
            .map_err(|e| CliError::failure(format!("failed to determine current directory: {e}")))?
            .join(entry_path)
    };
    let base_dir = path.parent().unwrap_or(Path::new("."));

    let session = CompilationSession::discover(&path)?;

    let mut modules = Vec::new();
    let mut processed = HashSet::new();
    let mut dependency_edges: HashMap<String, HashSet<String>> = HashMap::new();
    let mut incan_source_stdlib_module_paths: HashMap<String, PathBuf> = HashMap::new();
    // (file_path, module_name, path_segments)
    let mut to_process: Vec<(String, String, Vec<String>)> = vec![(
        path.to_string_lossy().to_string(),
        "main".to_string(),
        vec!["main".to_string()],
    )];

    while let Some((file_path, module_name, path_segments)) = to_process.pop() {
        if processed.contains(&file_path) {
            continue;
        }
        processed.insert(file_path.clone());
        dependency_edges.entry(file_path.clone()).or_default();

        let source = read_source(&file_path)?;
        let file_path_obj = Path::new(&file_path);
        let is_incan_source_stdlib_module = path_segments
            .first()
            .is_some_and(|segment| segment == stdlib::INCAN_STD_NAMESPACE);
        let ast = match session.parse_source(file_path_obj, &source, !is_incan_source_stdlib_module) {
            Ok(a) => {
                // Surface any non-fatal parser warnings (e.g. RFC 005 dot-notation nudges) immediately,
                // so they reach the user regardless of which build/run/debug command was invoked.
                for warn in &a.warnings {
                    eprint!("{}", diagnostics::format_error(&file_path, &source, warn));
                }
                a
            }
            Err(errs) => {
                let mut msg = String::new();
                for err in &errs {
                    msg.push_str(&diagnostics::format_error(&file_path, &source, err));
                    msg.push('\n');
                }
                return Err(CliError::failure(msg.trim_end()));
            }
        };

        let current_base = file_path_obj.parent().unwrap_or(base_dir);
        if uses_iterator_adapter_surface(&ast) {
            let module_path = vec![
                stdlib::STDLIB_ROOT.to_string(),
                "derives".to_string(),
                "collection".to_string(),
            ];
            let source_path = resolve_stdlib_module_source_path(&module_path)?;
            let mut module_segments = vec![stdlib::INCAN_STD_NAMESPACE.to_string()];
            module_segments.extend(module_path.iter().skip(1).cloned());
            let module_name = module_segments.join("_");
            let dep_path_str = source_path.to_string_lossy().to_string();
            if !processed.contains(&dep_path_str) {
                to_process.push((dep_path_str.clone(), module_name, module_segments));
            }
            dependency_edges
                .entry(file_path.clone())
                .or_default()
                .insert(dep_path_str);
        }
        if uses_result_combinator_surface(&ast) {
            let module_path = vec![stdlib::STDLIB_ROOT.to_string(), "result".to_string()];
            let source_path = resolve_stdlib_module_source_path(&module_path)?;
            let mut module_segments = vec![stdlib::INCAN_STD_NAMESPACE.to_string()];
            module_segments.extend(module_path.iter().skip(1).cloned());
            let module_name = module_segments.join("_");
            let dep_path_str = source_path.to_string_lossy().to_string();
            if !processed.contains(&dep_path_str) {
                to_process.push((dep_path_str.clone(), module_name, module_segments));
            }
            dependency_edges
                .entry(file_path.clone())
                .or_default()
                .insert(dep_path_str);
        }
        for resolved in resolve_program_source_imports(&ast, current_base, Some(&session.source_root)) {
            match resolved.resolution {
                SourceModuleImportResolution::Stdlib { module_path } => {
                    if stdlib::stdlib_stub_path(&module_path).is_none() {
                        continue;
                    }
                    let stdlib_key = module_path.join(".");
                    let source_path = if let Some(cached_path) = incan_source_stdlib_module_paths.get(&stdlib_key) {
                        cached_path.clone()
                    } else {
                        let resolved = resolve_stdlib_module_source_path(&module_path)?;
                        incan_source_stdlib_module_paths.insert(stdlib_key, resolved.clone());
                        resolved
                    };

                    let mut module_segments = vec![stdlib::INCAN_STD_NAMESPACE.to_string()];
                    module_segments.extend(module_path.iter().skip(1).cloned());
                    let module_name = module_segments.join("_");
                    let dep_path_str = source_path.to_string_lossy().to_string();
                    if !processed.contains(&dep_path_str) {
                        to_process.push((dep_path_str.clone(), module_name, module_segments));
                    }
                    dependency_edges
                        .entry(file_path.clone())
                        .or_default()
                        .insert(dep_path_str);
                }
                SourceModuleImportResolution::Local(module_ref) => {
                    let dep_path_str = module_ref.file_path.to_string_lossy().to_string();
                    if !processed.contains(&dep_path_str) {
                        to_process.push((dep_path_str.clone(), module_ref.module_name, module_ref.path_segments));
                    }
                    dependency_edges
                        .entry(file_path.clone())
                        .or_default()
                        .insert(dep_path_str);
                }
                SourceModuleImportResolution::External => {}
            }
        }

        modules.push(ParsedModule {
            name: module_name,
            path_segments,
            file_path: PathBuf::from(&file_path),
            source,
            ast,
        });
    }

    topologically_sort_modules(modules, &dependency_edges)
}

/// Return modules in stable topological order (dependencies first).
///
/// Discovery traversal uses a stack, which is not guaranteed to produce dependency-safe ordering for siblings.
/// This explicit sort guarantees each module appears only after its direct and transitive dependencies for acyclic
/// portions of the graph. For cyclic components (for example stdlib prelude re-export loops), we keep deterministic
/// fallback ordering rather than hard-failing in collection.
pub(crate) fn topologically_sort_modules(
    modules: Vec<ParsedModule>,
    dependency_edges: &HashMap<String, HashSet<String>>,
) -> CliResult<Vec<ParsedModule>> {
    if modules.is_empty() {
        return Ok(modules);
    }

    let mut module_by_path: HashMap<String, ParsedModule> = HashMap::new();
    let mut order_index: HashMap<String, usize> = HashMap::new();
    for (idx, module) in modules.into_iter().enumerate() {
        let key = module.file_path.to_string_lossy().to_string();
        order_index.insert(key.clone(), idx);
        module_by_path.insert(key, module);
    }

    let mut indegree: HashMap<String, usize> = module_by_path.keys().cloned().map(|key| (key, 0usize)).collect();
    let mut reverse_adj: HashMap<String, Vec<String>> = HashMap::new();

    for (module_path, deps) in dependency_edges {
        if !module_by_path.contains_key(module_path) {
            continue;
        }
        for dep in deps {
            if !module_by_path.contains_key(dep) {
                continue;
            }
            if let Some(value) = indegree.get_mut(module_path) {
                *value += 1;
            }
            reverse_adj.entry(dep.clone()).or_default().push(module_path.clone());
        }
    }

    let mut ready: BTreeSet<(usize, String)> = indegree
        .iter()
        .filter_map(|(path, &degree)| {
            (degree == 0).then_some((order_index.get(path).copied().unwrap_or(usize::MAX), path.clone()))
        })
        .collect();

    let mut sorted = Vec::new();
    while let Some((_, next)) = ready.pop_first() {
        let Some(module) = module_by_path.remove(&next) else {
            continue;
        };
        sorted.push(module);

        if let Some(dependents) = reverse_adj.get(&next) {
            for dependent in dependents {
                if let Some(value) = indegree.get_mut(dependent)
                    && *value > 0
                {
                    *value -= 1;
                    if *value == 0 {
                        ready.insert((
                            order_index.get(dependent).copied().unwrap_or(usize::MAX),
                            dependent.clone(),
                        ));
                    }
                }
            }
        }
    }

    if !module_by_path.is_empty() {
        // Kahn's algorithm leaves cycle members (and dependents blocked by them) unresolved.
        // Preserve deterministic behavior by appending unresolved modules in reverse discovery order, which matches the
        // previous `modules.reverse()` shape that existing stdlib integration tests rely on.
        let mut unresolved: Vec<(usize, ParsedModule)> = module_by_path
            .into_iter()
            .map(|(path, module)| (order_index.get(&path).copied().unwrap_or(usize::MAX), module))
            .collect();
        unresolved.sort_by_key(|(idx, _)| std::cmp::Reverse(*idx));
        sorted.extend(unresolved.into_iter().map(|(_, module)| module));
    }

    Ok(sorted)
}

/// Resolve the project root from a source file path.
///
/// If the file is inside a `src/` directory (e.g. `src/main.incn` or `projects/foo/src/main.incn`), the project root
/// is the parent of `src/`. Otherwise, the project root is the file's parent directory.
///
/// Returns `"."` when the computed root would be empty (which happens for relative paths like `src/main.incn` where
/// the parent of `"src"` is `""`).
pub(crate) fn resolve_project_root(file_path: &Path) -> PathBuf {
    file_path
        .parent()
        .and_then(|p| {
            if p.file_name().is_some_and(|name| name == "src") {
                p.parent()
            } else {
                Some(p)
            }
        })
        .map(|p| {
            if p.as_os_str().is_empty() {
                PathBuf::from(".")
            } else {
                p.to_path_buf()
            }
        })
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Resolve the source root directory for a project.
///
/// The source root is where user module imports are resolved from. Resolution order:
///
/// 1. Explicit `[build] source-root` in the manifest (e.g. `source-root = "lib"`)
/// 2. Convention: `src/` directory exists relative to project root
/// 3. Fallback: project root itself (flat layout)
///
/// This is used by both the build pipeline and the test runner so that `from greet import greet` resolves to the same
/// file everywhere.
pub(crate) fn resolve_source_root(project_root: &Path, manifest: Option<&ProjectManifest>) -> PathBuf {
    // ---- Explicit configuration ----
    if let Some(source_root) = manifest
        .and_then(|m| m.build.as_ref())
        .and_then(|b| b.source_root.as_deref())
    {
        return project_root.join(source_root);
    }

    // ---- Convention: src/ directory ----
    let src_dir = project_root.join("src");
    if src_dir.is_dir() {
        return src_dir;
    }

    // ---- Fallback: project root (flat layout) ----
    project_root.to_path_buf()
}

/// Validate the output directory to prevent path traversal attacks.
///
/// This function ensures:
/// - The path doesn't contain `..` components
/// - The path doesn't start with `/` (absolute path outside workspace) unless it starts with a known safe prefix
pub(crate) fn validate_output_dir(out_dir: &str) -> CliResult<()> {
    let path = Path::new(out_dir);

    // Check for path traversal attempts
    for component in path.components() {
        if let std::path::Component::ParentDir = component {
            return Err(CliError::failure(format!(
                "Output directory '{}' contains path traversal (..)",
                out_dir
            )));
        }
    }

    // Warn about absolute paths (but allow them for flexibility)
    if path.is_absolute() {
        tracing::warn!(
            "Using absolute output path: {}. Consider using a relative path.",
            out_dir
        );
    }

    Ok(())
}

/// Format a Rust import base path like `rust::serde_json` or `rust::chrono::naive::date`.
pub(crate) fn format_rust_import_base_path(crate_name: &str, path: &[String]) -> String {
    if path.is_empty() {
        format!("rust::{}", crate_name)
    } else {
        format!("rust::{}::{}", crate_name, path.join("::"))
    }
}

/// Format a Rust from-import path like `from rust::serde_json import from_str, to_string`.
pub(crate) fn format_rust_from_import_path(crate_name: &str, path: &[String], imported: &[String]) -> String {
    format!(
        "from {} import {}",
        format_rust_import_base_path(crate_name, path),
        imported.join(", ")
    )
}

/// Build an inline Rust import record for dependency resolution.
pub(crate) fn build_inline_rust_import(
    crate_name: &str,
    import_path: String,
    version: &Option<String>,
    features: &[String],
    span: Span,
    file_path: &Path,
    is_test_context: bool,
) -> InlineRustImport {
    InlineRustImport {
        crate_name: crate_name.to_string(),
        import_path,
        version: version.clone(),
        features: features.to_vec(),
        span,
        file_path: file_path.to_path_buf(),
        is_test_context,
    }
}

/// Extract inline Rust crate imports from a parsed module.
pub(crate) fn collect_inline_rust_imports(module: &ParsedModule, is_test_context: bool) -> Vec<InlineRustImport> {
    let mut imports = Vec::new();

    for decl in &module.ast.declarations {
        let crate::frontend::ast::Declaration::Import(import) = &decl.node else {
            continue;
        };

        match &import.kind {
            ImportKind::RustCrate {
                crate_name,
                path,
                version,
                features,
                ..
            } => {
                let import_path = format_rust_import_base_path(crate_name, path);
                imports.push(build_inline_rust_import(
                    crate_name,
                    import_path,
                    version,
                    features,
                    decl.span,
                    &module.file_path,
                    is_test_context,
                ));
            }
            ImportKind::RustFrom {
                crate_name,
                path,
                items,
                version,
                features,
                ..
            } => {
                let imported = items.iter().map(|item| item.name.clone()).collect::<Vec<_>>();
                let import_path = format_rust_from_import_path(crate_name, path, &imported);
                imports.push(build_inline_rust_import(
                    crate_name,
                    import_path,
                    version,
                    features,
                    decl.span,
                    &module.file_path,
                    is_test_context,
                ));
            }
            _ => {}
        }
    }

    imports
}

/// Extract all Rust dependency uses from a parsed module.
pub(crate) fn collect_rust_dependency_uses(module: &ParsedModule, is_test_context: bool) -> Vec<InlineRustImport> {
    let mut imports = collect_inline_rust_imports(module, is_test_context);
    let Some(rust_module_path) = &module.ast.rust_module_path else {
        return imports;
    };
    let Some(crate_name) = rust_module_path.node.split("::").next().filter(|name| !name.is_empty()) else {
        return imports;
    };
    if crate_name == stdlib::STDLIB_ROOT || stdlib::is_path_extra_crate_dep(crate_name) {
        return imports;
    }

    imports.push(build_inline_rust_import(
        crate_name,
        format!("rust.module(\"{}\")", rust_module_path.node),
        &None,
        &[],
        rust_module_path.span,
        &module.file_path,
        is_test_context,
    ));
    imports
}

/// Build a map of file paths to source contents for error reporting.
pub(crate) fn build_source_map(modules: &[ParsedModule]) -> HashMap<PathBuf, String> {
    let mut sources = HashMap::new();
    for module in modules {
        sources.insert(module.file_path.clone(), module.source.clone());
    }
    sources
}

/// Format a dependency resolution error with source-file context.
pub(crate) fn format_dependency_error(error: &DependencyError, sources: &HashMap<PathBuf, String>) -> String {
    let file_path = error.file_path.to_string_lossy();
    if let Some(source) = sources.get(&error.file_path) {
        return diagnostics::format_error(&file_path, source, &error.error);
    }
    if let Ok(source) = fs::read_to_string(&error.file_path) {
        return diagnostics::format_error(&file_path, &source, &error.error);
    }

    format!("error: {}\n  --> {}\n", error.error.message, error.file_path.display())
}

/// Build Cargo policy flags (`--offline` / `--locked` / `--frozen`).
pub(crate) fn cargo_policy_flags(policy: &CargoPolicy) -> Vec<String> {
    if policy.frozen {
        return vec!["--frozen".to_string()];
    }

    let mut flags = Vec::new();
    if policy.offline {
        flags.push("--offline".to_string());
    }
    if policy.locked {
        flags.push("--locked".to_string());
    }
    flags
}

/// Build Cargo feature-selection flags without policy or arbitrary extra args.
fn cargo_feature_flags(cargo_features: &CargoFeatureSelection) -> Vec<String> {
    let mut flags = Vec::new();
    if cargo_features.cargo_all_features {
        flags.push("--all-features".to_string());
    }
    if cargo_features.cargo_no_default_features {
        flags.push("--no-default-features".to_string());
    }
    if !cargo_features.cargo_features.is_empty() {
        flags.push("--features".to_string());
        flags.push(cargo_features.cargo_features.join(","));
    }
    flags
}

/// Build flags for lockfile-oriented Cargo commands.
pub(crate) fn cargo_lockfile_flags(policy: &CargoPolicy, cargo_features: &CargoFeatureSelection) -> Vec<String> {
    let mut flags = cargo_policy_flags(policy);
    flags.extend(cargo_feature_flags(cargo_features));
    flags
}

/// Build Cargo command flags (policy flags + feature flags + extra Cargo args).
pub(crate) fn cargo_command_flags(policy: &CargoPolicy, cargo_features: &CargoFeatureSelection) -> Vec<String> {
    let mut flags = cargo_lockfile_flags(policy, cargo_features);
    flags.extend(policy.extra_args.clone());
    flags
}

/// Build a lookup map from canonical module key (`a_b_c`) to module index in `collect_modules` output.
pub(crate) fn module_key_index(modules: &[ParsedModule]) -> HashMap<String, usize> {
    let mut module_idx_by_key: HashMap<String, usize> = HashMap::new();
    for (idx, module) in modules.iter().enumerate() {
        let key = canonicalize_source_module_segments(&module.path_segments).join("_");
        module_idx_by_key.insert(key, idx);
    }
    module_idx_by_key
}

/// Resolve imported source-module dependencies for one collected module using a precomputed module key index.
///
/// Public signatures in a directly imported module may reference types from that module's own imports, so the
/// typechecker needs the transitive source-module dependency closure rather than just the immediate import list.
/// This helper preserves stable module ordering by returning dependencies in collected-module index order.
///
/// Use this variant inside per-module loops to avoid rebuilding the module key map on every iteration.
pub(crate) fn imported_module_deps_for_with_index<'m>(
    modules: &'m [ParsedModule],
    module_index: usize,
    module_idx_by_key: &HashMap<String, usize>,
) -> Vec<(&'m str, &'m Program)> {
    // ---- Context: bounds and setup ----
    if module_index >= modules.len() {
        return Vec::new();
    }

    // ---- Context: walk the transitive local source-module import closure ----
    fn direct_local_dep_indexes(
        modules: &[ParsedModule],
        module_index: usize,
        module_idx_by_key: &HashMap<String, usize>,
    ) -> BTreeSet<usize> {
        let mut dep_indexes: BTreeSet<usize> = BTreeSet::new();
        for decl in &modules[module_index].ast.declarations {
            let crate::frontend::ast::Declaration::Import(import) = &decl.node else {
                continue;
            };
            match &import.kind {
                ImportKind::From { module, .. } => {
                    if module.parent_levels > 0 || module.is_absolute || module.segments.is_empty() {
                        continue;
                    }
                    let key = canonicalize_source_module_segments(&module.segments).join("_");
                    if let Some(dep_idx) = module_idx_by_key.get(&key).copied()
                        && dep_idx != module_index
                    {
                        dep_indexes.insert(dep_idx);
                    }
                }
                ImportKind::Module(path) => {
                    if path.parent_levels > 0 || path.is_absolute || path.segments.is_empty() {
                        continue;
                    }
                    let full_key = canonicalize_source_module_segments(&path.segments).join("_");
                    if let Some(dep_idx) = module_idx_by_key.get(&full_key).copied()
                        && dep_idx != module_index
                    {
                        dep_indexes.insert(dep_idx);
                    }
                    if path.segments.len() > 1 {
                        let parent_key =
                            canonicalize_source_module_segments(&path.segments[..path.segments.len() - 1]).join("_");
                        if let Some(dep_idx) = module_idx_by_key.get(&parent_key).copied()
                            && dep_idx != module_index
                        {
                            dep_indexes.insert(dep_idx);
                        }
                    }
                }
                _ => {}
            }
        }
        dep_indexes
    }

    let mut dep_indexes: BTreeSet<usize> = BTreeSet::new();
    let mut pending: Vec<usize> = direct_local_dep_indexes(modules, module_index, module_idx_by_key)
        .into_iter()
        .collect();
    while let Some(dep_idx) = pending.pop() {
        if dep_idx == module_index || !dep_indexes.insert(dep_idx) {
            continue;
        }
        pending.extend(direct_local_dep_indexes(modules, dep_idx, module_idx_by_key));
    }

    // ---- Context: materialize dependency pairs for typechecker.check_with_imports ----
    dep_indexes
        .into_iter()
        .map(|idx| (modules[idx].name.as_str(), &modules[idx].ast))
        .collect()
}

/// Typecheck all collected modules in dependency-safe order using shared CLI diagnostics formatting.
///
/// This helper centralizes the per-module checker setup used by `build` and `check` paths so warning/error rendering
/// stays consistent across command flows.
pub(crate) fn typecheck_modules_with_import_graph(
    modules: &[ParsedModule],
    manifest: Option<&ProjectManifest>,
    library_manifest_index: &LibraryManifestIndex,
    #[cfg(feature = "rust_inspect")] rust_inspect_manifest_dir: Option<&Path>,
) -> CliResult<()> {
    let declared = manifest.map(|m| m.declared_rust_crate_names());
    let module_idx_by_key = module_key_index(modules);
    let mut all_errors = String::new();

    for (idx, module) in modules.iter().enumerate() {
        let deps_for_module = imported_module_deps_for_with_index(modules, idx, &module_idx_by_key);

        let mut checker = typechecker::TypeChecker::new();
        if let Some(names) = declared.clone() {
            checker.set_declared_crate_names(names);
        }
        checker.set_library_manifest_index(library_manifest_index.clone());
        #[cfg(feature = "rust_inspect")]
        if let Some(rust_inspect_manifest_dir) = rust_inspect_manifest_dir {
            checker.set_rust_inspect_manifest_dir(rust_inspect_manifest_dir.to_path_buf());
        }

        match checker.check_with_imports(&module.ast, &deps_for_module) {
            Ok(()) => {
                for warn in checker.warnings() {
                    eprint!(
                        "{}",
                        diagnostics::format_error(module.file_path.to_string_lossy().as_ref(), &module.source, warn)
                    );
                }
            }
            Err(errs) => {
                for err in &errs {
                    all_errors.push_str(&diagnostics::format_error(
                        module.file_path.to_string_lossy().as_ref(),
                        &module.source,
                        err,
                    ));
                }
            }
        }
    }

    if all_errors.is_empty() {
        Ok(())
    } else {
        Err(CliError::failure(all_errors.trim_end()))
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frontend::typechecker;
    use crate::library_manifest::{LibraryManifest, VocabExports};
    use std::path::Path;

    fn parsed_module_for_test(source: &str) -> Result<ParsedModule, Box<dyn std::error::Error>> {
        let tokens = lexer::lex(source).map_err(|errs| format!("lex failed: {errs:?}"))?;
        let ast = parser::parse(&tokens).map_err(|errs| format!("parse failed: {errs:?}"))?;
        Ok(ParsedModule {
            name: "main".to_string(),
            path_segments: vec!["main".to_string()],
            file_path: PathBuf::from("main.incn"),
            source: source.to_string(),
            ast,
        })
    }

    fn registry_dependency(crate_name: &str, version: &str) -> DependencySpec {
        DependencySpec {
            crate_name: crate_name.to_string(),
            version: Some(version.to_string()),
            features: Vec::new(),
            default_features: true,
            source: DependencySource::Registry,
            optional: false,
            package: None,
        }
    }

    fn write_minimal_library_artifact(
        root: &Path,
        dependency_key: &str,
        manifest_name: &str,
        manifest: &LibraryManifest,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let artifact_root = root.join("deps").join(dependency_key).join("target").join("lib");
        std::fs::create_dir_all(artifact_root.join("src"))?;
        std::fs::write(
            artifact_root.join("Cargo.toml"),
            format!("[package]\nname = \"{manifest_name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n"),
        )?;
        std::fs::write(artifact_root.join("src/lib.rs"), "")?;
        manifest.write_to_path(&artifact_root.join(format!("{manifest_name}.incnlib")))?;
        Ok(())
    }

    #[test]
    fn collect_rust_dependency_uses_includes_rust_module_root() -> Result<(), Box<dyn std::error::Error>> {
        let module = parsed_module_for_test("rust.module(\"datafusion::prelude\")\n\ndef main() -> None:\n  pass\n")?;

        let imports = collect_rust_dependency_uses(&module, false);

        assert!(
            imports.iter().any(|import| import.crate_name == "datafusion"
                && import.import_path == "rust.module(\"datafusion::prelude\")"),
            "rust.module roots should participate in dependency resolution: {imports:?}"
        );
        Ok(())
    }

    #[test]
    fn collect_rust_dependency_uses_skips_stdlib_path_extra_crate_roots() -> Result<(), Box<dyn std::error::Error>> {
        let module = parsed_module_for_test("rust.module(\"incan_web_macros\")\n\ndef main() -> None:\n  pass\n")?;

        let imports = collect_rust_dependency_uses(&module, false);

        assert!(
            imports.iter().all(|import| import.crate_name != "incan_web_macros"),
            "stdlib-managed path crates should come from project requirements, not rust.module dependency uses: {imports:?}"
        );
        Ok(())
    }

    #[test]
    fn merge_resolved_dependencies_unions_dependency_contexts() -> Result<(), Box<dyn std::error::Error>> {
        let current = ResolvedDependencies {
            dependencies: vec![registry_dependency("serde", "1")],
            dev_dependencies: vec![registry_dependency("tokio", "1")],
        };
        let extra = ResolvedDependencies {
            dependencies: vec![
                registry_dependency("tokio", "1"),
                registry_dependency("datafusion", "53"),
            ],
            dev_dependencies: Vec::new(),
        };

        let merged = merge_resolved_dependencies(&current, &extra)?;

        assert_eq!(
            merged
                .dependencies
                .iter()
                .map(|dependency| dependency.crate_name.as_str())
                .collect::<Vec<_>>(),
            vec!["datafusion", "serde", "tokio"]
        );
        assert!(merged.dev_dependencies.is_empty());
        Ok(())
    }

    #[test]
    fn merge_resolved_dependencies_rejects_conflicting_contexts() {
        let current = ResolvedDependencies {
            dependencies: vec![registry_dependency("serde", "1")],
            dev_dependencies: Vec::new(),
        };
        let extra = ResolvedDependencies {
            dependencies: vec![registry_dependency("serde", "2")],
            dev_dependencies: Vec::new(),
        };

        let error = match merge_resolved_dependencies(&current, &extra) {
            Ok(merged) => panic!("expected conflict, got merged dependencies: {merged:?}"),
            Err(error) => error,
        };
        assert!(error.message.contains("serde"));
        assert!(error.message.contains("conflicts"));
    }

    #[test]
    fn compilation_session_parses_with_imported_library_vocab() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let project_root = tmp.path();
        std::fs::create_dir_all(project_root.join("src"))?;
        std::fs::write(
            project_root.join("incan.toml"),
            "[project]\nname = \"consumer\"\n\n[dependencies]\nwidgets = { path = \"deps/widgets\" }\n",
        )?;

        let mut manifest = LibraryManifest::new("widgets_core", "0.1.0");
        manifest.vocab = Some(VocabExports {
            crate_path: "widgets_vocab_companion".to_string(),
            package_name: "widgets_vocab_companion".to_string(),
            keyword_registrations: vec![incan_vocab::KeywordRegistration {
                activation: incan_vocab::KeywordActivation::OnImport {
                    namespace: "widgets.dsl".to_string(),
                },
                keywords: vec![incan_vocab::KeywordSpec::new(
                    "assert",
                    incan_vocab::KeywordSurfaceKind::ControlFlow,
                )],
                valid_decorators: Vec::new(),
            }],
            dsl_surfaces: Vec::new(),
            provider_manifest: incan_vocab::LibraryManifest::default(),
            desugarer_artifact: None,
        });
        write_minimal_library_artifact(project_root, "widgets", "widgets_core", &manifest)?;

        let main_path = project_root.join("src/main.incn");
        let source = "import pub::widgets\n\ndef main() -> None:\n  assert true\n";
        std::fs::write(&main_path, source)?;

        let session = CompilationSession::discover(&main_path)?;
        session
            .parse_source(&main_path, source, false)
            .map_err(|errors| format!("expected session parse to use imported vocab: {errors:?}"))?;

        Ok(())
    }

    // ---- resolve_project_root ----

    #[test]
    fn project_root_from_relative_src_is_dot_not_empty() {
        // Regression: `src/main.incn` used to yield "" instead of ".", causing
        // `Command::current_dir("")` to fail with ENOENT.
        let root = resolve_project_root(Path::new("src/main.incn"));
        assert_eq!(root, PathBuf::from("."));
    }

    #[test]
    fn project_root_from_nested_src_path() {
        let root = resolve_project_root(Path::new("projects/greeter/src/main.incn"));
        assert_eq!(root, PathBuf::from("projects/greeter"));
    }

    #[test]
    fn project_root_from_absolute_src_path() {
        let root = resolve_project_root(Path::new("/home/user/project/src/main.incn"));
        assert_eq!(root, PathBuf::from("/home/user/project"));
    }

    #[test]
    fn cargo_policy_resolves_env_defaults_and_frozen_implication() {
        let policy = CargoPolicy::from_sources(
            CargoPolicyCliFlags::default(),
            Vec::new(),
            Vec::new(),
            |name| match name {
                "INCAN_FROZEN" => Some("1".to_string()),
                "INCAN_CARGO_ARGS" => Some("--timings --verbose".to_string()),
                _ => None,
            },
        );

        assert!(policy.frozen);
        assert!(policy.offline);
        assert!(policy.locked);
        assert_eq!(policy.extra_args, vec!["--timings", "--verbose"]);
    }

    #[cfg(feature = "rust_inspect")]
    #[test]
    fn rust_inspect_prewarm_env_defaults_to_enabled() {
        assert!(parse_rust_inspect_prewarm_env(None));
        assert!(parse_rust_inspect_prewarm_env(Some("")));
        assert!(parse_rust_inspect_prewarm_env(Some("1")));
        assert!(parse_rust_inspect_prewarm_env(Some("true")));
        assert!(parse_rust_inspect_prewarm_env(Some("on")));
        assert!(parse_rust_inspect_prewarm_env(Some("unexpected")));
        assert!(!parse_rust_inspect_prewarm_env(Some("0")));
        assert!(!parse_rust_inspect_prewarm_env(Some("false")));
        assert!(!parse_rust_inspect_prewarm_env(Some(" OFF ")));
        assert!(!parse_rust_inspect_prewarm_env(Some("no")));
    }

    #[cfg(feature = "rust_inspect")]
    #[test]
    fn rust_inspect_query_paths_include_explicit_non_stdlib_rust_types() -> Result<(), Box<dyn std::error::Error>> {
        let module = parsed_module_for_test(
            r#"
from rust::datafusion::execution::context import SessionContext
from rust::datafusion::prelude import CsvReadOptions, read_csv
from rust::incan_stdlib::async::runtime import block_on
from rust::std::primitive import i64 as RustI64
"#,
        )?;

        let paths = collect_rust_inspect_query_paths(&[module]);

        assert_eq!(
            paths,
            vec![
                "datafusion::execution::context::SessionContext".to_string(),
                "datafusion::prelude::CsvReadOptions".to_string(),
                "datafusion::prelude::read_csv".to_string(),
            ]
        );
        Ok(())
    }

    #[test]
    fn cargo_policy_uses_cli_extra_args_before_env_extra_args() {
        let policy = CargoPolicy::from_sources(
            CargoPolicyCliFlags {
                offline: true,
                ..CargoPolicyCliFlags::default()
            },
            vec!["--features".to_string(), "cli".to_string()],
            vec!["--no-default-features".to_string()],
            |name| match name {
                "INCAN_CARGO_ARGS" => Some("--features env".to_string()),
                _ => None,
            },
        );

        assert!(policy.offline);
        assert_eq!(policy.extra_args, vec!["--features", "cli", "--no-default-features"]);
    }

    #[test]
    fn cargo_policy_cli_disable_flags_override_env_defaults() {
        let policy = CargoPolicy::from_sources(
            CargoPolicyCliFlags {
                no_offline: true,
                no_locked: true,
                no_frozen: true,
                ..CargoPolicyCliFlags::default()
            },
            Vec::new(),
            Vec::new(),
            |name| match name {
                "INCAN_OFFLINE" | "INCAN_LOCKED" | "INCAN_FROZEN" => Some("1".to_string()),
                _ => None,
            },
        );

        assert!(!policy.offline);
        assert!(!policy.locked);
        assert!(!policy.frozen);
    }

    #[test]
    fn cargo_command_flags_order_policy_features_then_extra_args() {
        let policy = CargoPolicy::explicit(
            true,
            true,
            false,
            vec!["--timings".to_string(), "--color=always".to_string()],
        );
        let features = CargoFeatureSelection {
            cargo_features: vec!["json".to_string(), "web".to_string()],
            cargo_no_default_features: true,
            cargo_all_features: false,
        };

        assert_eq!(
            cargo_command_flags(&policy, &features),
            vec![
                "--offline",
                "--locked",
                "--no-default-features",
                "--features",
                "json,web",
                "--timings",
                "--color=always"
            ]
        );
    }

    #[test]
    fn project_root_when_file_is_not_in_src() {
        // File directly in a directory, not in src/
        let root = resolve_project_root(Path::new("main.incn"));
        assert_eq!(root, PathBuf::from("."));
    }

    #[test]
    fn project_root_from_non_src_subdirectory() {
        let root = resolve_project_root(Path::new("lib/utils.incn"));
        assert_eq!(root, PathBuf::from("lib"));
    }

    // ---- resolve_source_root ----

    #[test]
    fn source_root_uses_src_convention() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let project = tmp.path().join("myproject");
        fs::create_dir_all(project.join("src"))?;

        let root = resolve_source_root(&project, None);
        assert_eq!(root, project.join("src"));
        Ok(())
    }

    #[test]
    fn source_root_falls_back_to_project_root_when_no_src() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let project = tmp.path().join("flat_project");
        fs::create_dir_all(&project)?;

        let root = resolve_source_root(&project, None);
        assert_eq!(root, project);
        Ok(())
    }

    #[test]
    fn source_root_respects_explicit_manifest_config() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let project = tmp.path().join("custom_src");
        fs::create_dir_all(project.join("src"))?; // src/ exists but should be overridden

        let manifest_content = r#"
[build]
source-root = "lib"
"#;
        let manifest = ProjectManifest::from_str(manifest_content, &project.join("incan.toml"))?;

        let root = resolve_source_root(&project, Some(&manifest));
        assert_eq!(root, project.join("lib"));
        Ok(())
    }

    #[test]
    fn collect_project_requirements_tracks_async_namespace_features() -> Result<(), Box<dyn std::error::Error>> {
        let module = parsed_module_for_test(
            r#"
import std.async
from std.math import sqrt
"#,
        )?;

        let requirements = collect_project_requirements(&[module], &LibraryManifestIndex::default())?;
        assert!(
            requirements.stdlib_features.iter().any(|feature| feature == "async"),
            "std.async should enable async stdlib feature"
        );
        assert!(
            requirements.stdlib_features.iter().any(|f| f == "async"),
            "expected async feature"
        );
        Ok(())
    }

    #[test]
    fn collect_project_requirements_adds_serde_runtime_deps_from_derives() -> Result<(), Box<dyn std::error::Error>> {
        let module = parsed_module_for_test(
            r#"
from std.serde import json

@derive(json)
model User:
    name: str
"#,
        )?;

        let requirements = collect_project_requirements(&[module], &LibraryManifestIndex::default())?;
        assert!(
            requirements.stdlib_features.iter().any(|feature| feature == "json"),
            "serde usage should enable the json stdlib feature"
        );
        assert!(
            requirements.dependencies.iter().any(|dep| dep.crate_name == "serde"),
            "serde usage should inject serde dependency"
        );
        assert!(
            requirements
                .dependencies
                .iter()
                .all(|dep| dep.crate_name != "serde_json"),
            "serde_json should stay behind incan_stdlib's json feature"
        );
        Ok(())
    }

    #[test]
    fn collect_modules_canonicalizes_directory_entrypoints() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let project_root = tmp.path();
        std::fs::write(
            project_root.join("incan.toml"),
            "[project]\nname = \"demo\"\nversion = \"0.1.0\"\n",
        )?;

        let src_dir = project_root.join("src");
        std::fs::create_dir_all(src_dir.join("dataset"))?;
        std::fs::write(
            src_dir.join("lib.incn"),
            "from dataset.mod import DataSet\nfrom dataset.ops import filter_ds\n",
        )?;
        std::fs::write(
            src_dir.join("dataset").join("mod.incn"),
            "pub trait DataSet[T]:\n    pass\n",
        )?;
        std::fs::write(
            src_dir.join("dataset").join("ops.incn"),
            "from dataset.mod import DataSet\npub def filter_ds[T](ds: DataSet[T]) -> DataSet[T]:\n    return ds\n",
        )?;

        let entry = src_dir.join("lib.incn");
        let entry_str = entry
            .to_str()
            .ok_or("entry path should be valid utf-8 for collect_modules test")?;
        let modules = collect_modules(entry_str)?;

        let dataset_mod = modules
            .iter()
            .find(|module| module.file_path.ends_with(Path::new("dataset").join("mod.incn")))
            .ok_or("expected dataset/mod.incn to be collected")?;
        assert_eq!(dataset_mod.path_segments, vec!["dataset".to_string()]);
        assert_ne!(
            dataset_mod.path_segments,
            vec!["dataset".to_string(), "mod".to_string()]
        );

        let dataset_ops = modules
            .iter()
            .find(|module| module.file_path.ends_with(Path::new("dataset").join("ops.incn")))
            .ok_or("expected dataset/ops.incn to be collected")?;
        assert_eq!(
            dataset_ops.path_segments,
            vec!["dataset".to_string(), "ops".to_string()]
        );

        Ok(())
    }

    #[test]
    fn collect_modules_supports_init_directory_entrypoints() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let project_root = tmp.path();
        std::fs::write(
            project_root.join("incan.toml"),
            "[project]\nname = \"demo\"\nversion = \"0.1.0\"\n",
        )?;

        let src_dir = project_root.join("src");
        std::fs::create_dir_all(src_dir.join("dataset"))?;
        std::fs::write(src_dir.join("lib.incn"), "from dataset import DataSet\n")?;
        std::fs::write(
            src_dir.join("dataset").join("__init__.incn"),
            "pub trait DataSet[T]:\n    pass\n",
        )?;

        let entry = src_dir.join("lib.incn");
        let entry_str = entry
            .to_str()
            .ok_or("entry path should be valid utf-8 for collect_modules test")?;
        let modules = collect_modules(entry_str)?;

        let dataset_init = modules
            .iter()
            .find(|module| module.file_path.ends_with(Path::new("dataset").join("__init__.incn")))
            .ok_or("expected dataset/__init__.incn to be collected")?;
        assert_eq!(dataset_init.path_segments, vec!["dataset".to_string()]);

        Ok(())
    }

    #[test]
    fn merge_project_requirement_dependencies_adds_math_runtime_crate() -> Result<(), Box<dyn std::error::Error>> {
        let module = parsed_module_for_test(
            r#"
from std.math import sqrt
"#,
        )?;
        let requirements = collect_project_requirements(&[module], &LibraryManifestIndex::default())?;
        let mut resolved = ResolvedDependencies {
            dependencies: Vec::new(),
            dev_dependencies: Vec::new(),
        };

        merge_project_requirement_dependencies(&mut resolved, &requirements)?;

        assert!(
            resolved.dependencies.iter().any(|dep| dep.crate_name == "libm"),
            "std.math should inject libm for generated projects"
        );
        Ok(())
    }

    #[test]
    fn merge_project_requirement_dependencies_adds_io_runtime_crate() -> Result<(), Box<dyn std::error::Error>> {
        let module = parsed_module_for_test(
            r#"
from std.io import BytesIO
"#,
        )?;
        let requirements = collect_project_requirements(&[module], &LibraryManifestIndex::default())?;
        let mut resolved = ResolvedDependencies {
            dependencies: Vec::new(),
            dev_dependencies: Vec::new(),
        };

        merge_project_requirement_dependencies(&mut resolved, &requirements)?;

        assert!(
            resolved.dependencies.iter().any(|dep| dep.crate_name == "byteorder"),
            "std.io should inject byteorder for generated projects"
        );
        Ok(())
    }

    #[test]
    fn collect_modules_skips_unknown_stdlib_source_resolution() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let src_dir = tmp.path().join("src");
        std::fs::create_dir_all(&src_dir)?;
        let entry = src_dir.join("main.incn");
        std::fs::write(&entry, "from std.unknown_module import thing\n")?;

        let modules = collect_modules(entry.to_string_lossy().as_ref())?;
        assert_eq!(modules.len(), 1, "unknown std.* imports should not queue source stubs");
        Ok(())
    }

    #[test]
    fn collect_modules_resolves_source_root_for_examples_entrypoints() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let project_root = tmp.path();
        std::fs::write(
            project_root.join("incan.toml"),
            r#"[project]
name = "demo"
version = "0.1.0"
"#,
        )?;
        let src_dir = project_root.join("src");
        let examples_dir = project_root.join("examples");
        std::fs::create_dir_all(&src_dir)?;
        std::fs::create_dir_all(&examples_dir)?;

        std::fs::write(
            src_dir.join("dataset.incn"),
            r#"pub trait DataSet[T]:
    pass
"#,
        )?;
        let entry = examples_dir.join("trait_hierarchy.incn");
        std::fs::write(
            &entry,
            r#"from dataset import DataSet

def main() -> None:
    pass
"#,
        )?;

        let modules = collect_modules(entry.to_string_lossy().as_ref())?;
        assert_eq!(modules.len(), 2, "example entrypoint should pull source-root imports");
        assert!(
            modules.iter().any(|m| m.file_path.ends_with("src/dataset.incn")),
            "expected dataset module to resolve from source root"
        );
        Ok(())
    }

    #[test]
    fn collect_modules_orders_dependencies_before_dependents() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let project_root = tmp.path();
        std::fs::write(
            project_root.join("incan.toml"),
            r#"[project]
name = "dep_order_demo"
version = "0.1.0"
"#,
        )?;
        let src_dir = project_root.join("src");
        std::fs::create_dir_all(&src_dir)?;

        std::fs::write(
            src_dir.join("substrait_model.incn"),
            r#"pub model SubstraitPlan:
    rels: list[str]
"#,
        )?;
        std::fs::write(
            src_dir.join("substrait_builder.incn"),
            r#"from substrait_model import SubstraitPlan

pub def plan_from_named_table(name: str) -> SubstraitPlan:
    _ = name
    return SubstraitPlan(rels=[])
"#,
        )?;
        let entry = src_dir.join("lib.incn");
        std::fs::write(
            &entry,
            r#"from substrait_builder import plan_from_named_table
from substrait_model import SubstraitPlan

pub def probe() -> SubstraitPlan:
    return plan_from_named_table(str("orders"))
"#,
        )?;

        let modules = collect_modules(entry.to_string_lossy().as_ref())?;
        let mut model_idx = None;
        let mut builder_idx = None;
        let mut entry_idx = None;
        for (idx, module) in modules.iter().enumerate() {
            if module.file_path.ends_with("src/substrait_model.incn") {
                model_idx = Some(idx);
            } else if module.file_path.ends_with("src/substrait_builder.incn") {
                builder_idx = Some(idx);
            } else if module.file_path.ends_with("src/lib.incn") {
                entry_idx = Some(idx);
            }
        }

        let Some(model_idx) = model_idx else {
            panic!("expected substrait_model module");
        };
        let Some(builder_idx) = builder_idx else {
            panic!("expected substrait_builder module");
        };
        let Some(entry_idx) = entry_idx else {
            panic!("expected entry module");
        };

        assert!(
            model_idx < builder_idx,
            "dependency module must be ordered before dependent module"
        );
        assert!(
            builder_idx < entry_idx,
            "entry module must be ordered after imported modules"
        );
        Ok(())
    }

    #[test]
    fn collect_modules_order_keeps_imported_types_resolved_during_typecheck() -> Result<(), Box<dyn std::error::Error>>
    {
        let tmp = tempfile::tempdir()?;
        let project_root = tmp.path();
        std::fs::write(
            project_root.join("incan.toml"),
            r#"[project]
name = "dep_check_demo"
version = "0.1.0"
"#,
        )?;
        let src_dir = project_root.join("src");
        std::fs::create_dir_all(&src_dir)?;

        std::fs::write(
            src_dir.join("substrait_model.incn"),
            r#"@derive(Clone)
pub model SubstraitRelNode:
    rel_id: str

@derive(Clone)
pub model SubstraitPlan:
    plan_id: str
    root_rel_id: str
    rels: list[SubstraitRelNode]
    profile_tags: list[str]

pub def empty_substrait_plan() -> SubstraitPlan:
    return SubstraitPlan(plan_id=str("p"), root_rel_id=str(""), rels=[], profile_tags=[])
"#,
        )?;
        std::fs::write(
            src_dir.join("substrait_builder.incn"),
            r#"from substrait_model import SubstraitPlan, SubstraitRelNode, empty_substrait_plan

pub def build_one() -> SubstraitPlan:
    plan = empty_substrait_plan()
    mut rels = plan.rels
    rel = SubstraitRelNode(rel_id=str("r1"))
    rels.append(rel)
    return SubstraitPlan(plan_id=plan.plan_id, root_rel_id=rel.rel_id, rels=rels, profile_tags=plan.profile_tags)
"#,
        )?;
        let entry = src_dir.join("lib.incn");
        std::fs::write(
            &entry,
            r#"from substrait_builder import build_one
from substrait_model import SubstraitPlan

pub def probe() -> SubstraitPlan:
    return build_one()
"#,
        )?;

        let modules = collect_modules(entry.to_string_lossy().as_ref())?;
        let module_idx_by_key = module_key_index(&modules);
        for (idx, module) in modules.iter().enumerate() {
            let deps = imported_module_deps_for_with_index(&modules, idx, &module_idx_by_key);
            let mut checker = typechecker::TypeChecker::new();
            if let Err(errs) = checker.check_with_imports(&module.ast, &deps) {
                return Err(format!(
                    "typecheck failed for module {}: {:?}",
                    module.file_path.display(),
                    errs.iter().map(|e| e.message.clone()).collect::<Vec<_>>()
                )
                .into());
            }
        }
        Ok(())
    }

    #[test]
    fn imported_module_deps_for_includes_forward_edge_in_cycle() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let project_root = tmp.path();
        std::fs::write(
            project_root.join("incan.toml"),
            r#"[project]
name = "cycle_dep_resolver_demo"
version = "0.1.0"
"#,
        )?;
        let src_dir = project_root.join("src");
        std::fs::create_dir_all(&src_dir)?;
        std::fs::write(
            src_dir.join("a.incn"),
            r#"from b import pong

pub def ping() -> int:
    return pong()
"#,
        )?;
        std::fs::write(
            src_dir.join("b.incn"),
            r#"from a import ping

pub def pong() -> int:
    return 1
"#,
        )?;
        let entry = src_dir.join("main.incn");
        std::fs::write(
            &entry,
            r#"from a import ping

pub def main() -> int:
    return ping()
"#,
        )?;

        let modules = collect_modules(entry.to_string_lossy().as_ref())?;
        let Some(b_index) = modules
            .iter()
            .position(|module| module.file_path.ends_with("src/b.incn"))
        else {
            panic!("expected src/b.incn module");
        };
        let module_idx_by_key = module_key_index(&modules);
        let deps = imported_module_deps_for_with_index(&modules, b_index, &module_idx_by_key);
        assert!(
            deps.iter().any(|(name, _)| *name == "a"),
            "expected cyclic forward dependency `b -> a` to be resolved, got: {:?}",
            deps.iter().map(|(name, _)| (*name).to_string()).collect::<Vec<_>>()
        );
        Ok(())
    }

    #[test]
    fn imported_module_deps_for_includes_transitive_signature_dependencies() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let project_root = tmp.path();
        std::fs::write(
            project_root.join("incan.toml"),
            r#"[project]
name = "transitive_signature_dep_demo"
version = "0.1.0"
"#,
        )?;
        let src_dir = project_root.join("src");
        std::fs::create_dir_all(&src_dir)?;
        std::fs::write(
            src_dir.join("dataset.incn"),
            r#"pub class LazyFrame[T]:
    def clone(self) -> Self:
        return self
"#,
        )?;
        std::fs::write(
            src_dir.join("session.incn"),
            r#"from dataset import LazyFrame

pub class Session:
    def read_csv[T](self) -> Result[LazyFrame[T], str]:
        return Err(str("not implemented"))
"#,
        )?;
        let entry = src_dir.join("main.incn");
        std::fs::write(
            &entry,
            r#"from session import Session

def main() -> Result[None, str]:
    session = Session()
    lines = session.read_csv[int]()?
    lines.clone()
    return Ok(None)
"#,
        )?;

        let modules = collect_modules(entry.to_string_lossy().as_ref())?;
        let Some(main_index) = modules
            .iter()
            .position(|module| module.file_path.ends_with("src/main.incn"))
        else {
            return Err("expected src/main.incn module".into());
        };
        let module_idx_by_key = module_key_index(&modules);
        let deps = imported_module_deps_for_with_index(&modules, main_index, &module_idx_by_key);
        assert!(
            deps.iter().any(|(name, _)| *name == "dataset"),
            "expected transitive dependency `dataset` to be included for imported signature resolution, got: {:?}",
            deps.iter().map(|(name, _)| (*name).to_string()).collect::<Vec<_>>()
        );

        let mut checker = typechecker::TypeChecker::new();
        if let Err(errs) = checker.check_with_imports(&modules[main_index].ast, &deps) {
            return Err(format!(
                "typecheck failed: {:?}",
                errs.iter().map(|e| e.message.clone()).collect::<Vec<_>>()
            )
            .into());
        }
        Ok(())
    }

    #[test]
    fn collect_modules_supports_example_entry_with_cyclic_src_interfaces() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let project_root = tmp.path();
        std::fs::write(
            project_root.join("incan.toml"),
            r#"[project]
name = "example_cycle_demo"
version = "0.1.0"
"#,
        )?;
        let src_dir = project_root.join("src");
        let examples_dir = project_root.join("examples");
        std::fs::create_dir_all(&src_dir)?;
        std::fs::create_dir_all(&examples_dir)?;
        std::fs::write(
            src_dir.join("functions.incn"),
            r#"from dataset import DataFrame, DataSet

pub def display[T](data: DataSet[T]) -> None:
    pass

pub def sink[T](data: DataFrame[T]) -> None:
    pass
"#,
        )?;
        std::fs::write(
            src_dir.join("session.incn"),
            r#"from dataset import DataFrame, LazyFrame

pub model SessionError:
    pub message: str

pub class Session:
    @staticmethod
    def default() -> Session:
        return Session()

    def read_csv[T](self, _logical_name: str, _uri: str) -> Result[LazyFrame[T], SessionError]:
        return Err(SessionError(message=str("not implemented")))

    def activate(self) -> None:
        pass

pub def collect_with_active_session[T](data: LazyFrame[T]) -> Result[DataFrame[T], SessionError]:
    return Err(SessionError(message=str("not implemented")))
"#,
        )?;
        std::fs::write(
            src_dir.join("dataset.incn"),
            r#"from session import SessionError, collect_with_active_session

pub trait DataSet[T]:
    pass

pub class DataFrame[T] with DataSet:
    def clone(self) -> Self:
        return self

pub class LazyFrame[T] with DataSet:
    def clone(self) -> Self:
        return self

    def collect(self) -> Result[DataFrame[T], SessionError]:
        return collect_with_active_session[T](self.clone())
"#,
        )?;
        let entry = examples_dir.join("main.incn");
        std::fs::write(
            &entry,
            r#"from functions import display
from session import Session, SessionError

def main() -> Result[None, SessionError]:
    mut session = Session.default()
    lines = session.read_csv[int](str("orders"), str("input.csv"))?
    transformed = lines.clone()
    session.activate()
    df = transformed.clone().collect()?
    display(df)
    return Ok(None)
"#,
        )?;

        let modules = collect_modules(entry.to_string_lossy().as_ref())?;
        let module_idx_by_key = module_key_index(&modules);
        for (idx, module) in modules.iter().enumerate() {
            let deps = imported_module_deps_for_with_index(&modules, idx, &module_idx_by_key);
            let mut checker = typechecker::TypeChecker::new();
            if let Err(errs) = checker.check_with_imports(&module.ast, &deps) {
                return Err(format!(
                    "typecheck failed for module {}: {:?}",
                    module.file_path.display(),
                    errs.iter().map(|e| e.message.clone()).collect::<Vec<_>>()
                )
                .into());
            }
        }
        Ok(())
    }

    #[test]
    fn collect_modules_supports_directory_module_cycles_from_example_entry() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let project_root = tmp.path();
        std::fs::write(
            project_root.join("incan.toml"),
            r#"[project]
name = "example_directory_cycle_demo"
version = "0.1.0"
"#,
        )?;
        let src_dir = project_root.join("src");
        let dataset_dir = src_dir.join("dataset");
        let examples_dir = project_root.join("examples");
        std::fs::create_dir_all(&dataset_dir)?;
        std::fs::create_dir_all(&examples_dir)?;
        std::fs::write(
            src_dir.join("session.incn"),
            r#"from dataset import DataFrame, LazyFrame

pub model SessionError:
    pub message: str

pub class Session:
    @staticmethod
    def default() -> Session:
        return Session()

    def read_csv[T with Clone](self, _logical_name: str, _uri: str) -> Result[LazyFrame[T], SessionError]:
        return Err(SessionError(message=str("not implemented")))

pub def collect_with_active_session[T with Clone](data: LazyFrame[T]) -> Result[DataFrame[T], SessionError]:
    return Err(SessionError(message=str("not implemented")))
"#,
        )?;
        std::fs::write(
            dataset_dir.join("mod.incn"),
            r#"from session import SessionError, collect_with_active_session

pub trait DataSet[T with Clone]:
    pass

pub class DataFrame[T with Clone] with DataSet:
    def clone(self) -> Self:
        return self

pub class LazyFrame[T with Clone] with DataSet:
    def clone(self) -> Self:
        return self

    def collect(self) -> Result[DataFrame[T], SessionError]:
        return collect_with_active_session[T](self.clone())
"#,
        )?;
        let entry = examples_dir.join("main.incn");
        std::fs::write(
            &entry,
            r#"from session import Session, SessionError

@derive(Clone)
pub model OrderLine:
    pub sku: str

def main() -> Result[None, SessionError]:
    session = Session.default()
    lines = session.read_csv[OrderLine](str("orders"), str("input.csv"))?
    df = lines.clone().collect()?
    df.clone()
    return Ok(None)
"#,
        )?;

        let modules = collect_modules(entry.to_string_lossy().as_ref())?;
        let module_idx_by_key = module_key_index(&modules);
        for (idx, module) in modules.iter().enumerate() {
            let deps = imported_module_deps_for_with_index(&modules, idx, &module_idx_by_key);
            let mut checker = typechecker::TypeChecker::new();
            if let Err(errs) = checker.check_with_imports(&module.ast, &deps) {
                return Err(format!(
                    "typecheck failed for module {}: {:?}",
                    module.file_path.display(),
                    errs.iter().map(|e| e.message.clone()).collect::<Vec<_>>()
                )
                .into());
            }
        }
        Ok(())
    }

    #[test]
    fn collect_modules_cycle_falls_back_to_deterministic_order() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let project_root = tmp.path();
        std::fs::write(
            project_root.join("incan.toml"),
            r#"[project]
name = "cycle_demo"
version = "0.1.0"
"#,
        )?;
        let src_dir = project_root.join("src");
        std::fs::create_dir_all(&src_dir)?;

        std::fs::write(
            src_dir.join("a.incn"),
            r#"from b import pong

pub def ping() -> int:
    return pong()
"#,
        )?;
        std::fs::write(
            src_dir.join("b.incn"),
            r#"from a import ping

pub def pong() -> int:
    return 1
"#,
        )?;
        let entry = src_dir.join("main.incn");
        std::fs::write(
            &entry,
            r#"from a import ping

pub def main() -> int:
    return ping()
"#,
        )?;

        let modules = collect_modules(entry.to_string_lossy().as_ref())?;
        assert_eq!(modules.len(), 3, "expected all modules to be collected even with cycle");
        assert!(modules[0].file_path.ends_with("src/b.incn"));
        assert!(modules[1].file_path.ends_with("src/a.incn"));
        assert!(modules[2].file_path.ends_with("src/main.incn"));
        Ok(())
    }

    #[cfg(feature = "rust_inspect")]
    #[test]
    fn rust_inspect_workspace_fingerprint_is_deterministic() {
        let requirements = ProjectRequirements::default();
        let resolved = ResolvedDependencies {
            dependencies: vec![DependencySpec {
                crate_name: "serde".to_string(),
                version: Some("1".to_string()),
                features: vec!["derive".to_string()],
                default_features: true,
                source: DependencySource::Registry,
                optional: false,
                package: None,
            }],
            dev_dependencies: Vec::new(),
        };
        let fp_a = super::rust_inspect_workspace_fingerprint(
            "probe",
            Some("2021"),
            &resolved,
            &requirements.stdlib_features,
            Some("lock-bytes"),
        );
        let fp_b = super::rust_inspect_workspace_fingerprint(
            "probe",
            Some("2021"),
            &resolved,
            &requirements.stdlib_features,
            Some("lock-bytes"),
        );
        assert_eq!(fp_a, fp_b);
        assert!(fp_a.starts_with(super::RUST_INSPECT_WORKSPACE_FINGERPRINT_PREFIX));
    }

    #[cfg(feature = "rust_inspect")]
    #[test]
    fn rust_inspect_workspace_fingerprint_changes_when_lock_payload_changes() {
        let requirements = ProjectRequirements::default();
        let resolved = ResolvedDependencies {
            dependencies: Vec::new(),
            dev_dependencies: Vec::new(),
        };
        let fp_one = super::rust_inspect_workspace_fingerprint(
            "p",
            None,
            &resolved,
            &requirements.stdlib_features,
            Some("lock-a"),
        );
        let fp_two = super::rust_inspect_workspace_fingerprint(
            "p",
            None,
            &resolved,
            &requirements.stdlib_features,
            Some("lock-b"),
        );
        assert_ne!(fp_one, fp_two);
    }

    #[cfg(feature = "rust_inspect")]
    #[test]
    fn rust_inspect_workspace_dir_is_namespaced_by_input_fingerprint() {
        let root = Path::new("/workspace");
        let first = super::rust_inspect_workspace_dir(root, "demo", "v1:aaaaaaaaaaaaaaaaaaaaaaaa");
        let second = super::rust_inspect_workspace_dir(root, "demo", "v1:bbbbbbbbbbbbbbbbbbbbbbbb");

        assert_ne!(first, second);
        assert!(first.ends_with(Path::new("target/incan_lock/rust_inspect/demo-aaaaaaaaaaaaaaaa")));
        assert!(second.ends_with(Path::new("target/incan_lock/rust_inspect/demo-bbbbbbbbbbbbbbbb")));
    }

    #[cfg(feature = "rust_inspect")]
    #[test]
    fn ensure_rust_inspect_workspace_uses_rust_safe_dependency_keys() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let requirements = ProjectRequirements::default();
        let resolved = ResolvedDependencies {
            dependencies: vec![DependencySpec {
                crate_name: "datafusion-substrait".to_string(),
                version: Some("53".to_string()),
                features: vec!["protoc".to_string()],
                default_features: true,
                source: DependencySource::Registry,
                optional: false,
                package: None,
            }],
            dev_dependencies: Vec::new(),
        };

        let out_dir = ensure_rust_inspect_workspace(
            tmp.path(),
            "metadata_probe",
            Some("2021".to_string()),
            &resolved,
            &requirements,
            Some("[[package]]\nname = \"metadata_probe\"\n".to_string()),
        )?;
        assert_eq!(
            super::test_rust_inspect_workspace_generations(&out_dir),
            1,
            "expected one rust-inspect workspace generation"
        );

        let cargo_toml = fs::read_to_string(out_dir.join("Cargo.toml"))?;
        let cargo_lock = fs::read_to_string(out_dir.join("Cargo.lock"))?;
        let main_rs = fs::read_to_string(out_dir.join("src").join("main.rs"))?;

        assert!(
            cargo_toml.contains("[dependencies.datafusion_substrait]"),
            "expected rust-safe dependency key in generated rust-inspect workspace, got:\n{cargo_toml}"
        );
        assert!(
            cargo_toml.contains("package = \"datafusion-substrait\""),
            "expected original package name preserved in generated rust-inspect workspace, got:\n{cargo_toml}"
        );
        assert!(
            cargo_lock.contains("metadata_probe"),
            "expected rust-inspect workspace to write the provided Cargo.lock payload"
        );
        assert!(
            main_rs.contains("use datafusion_substrait as _;"),
            "expected rust-inspect workspace stub to reference the aliased dependency crate, got:\n{main_rs}"
        );
        Ok(())
    }

    #[cfg(feature = "rust_inspect")]
    #[test]
    fn ensure_rust_inspect_workspace_skips_regeneration_when_unchanged() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let requirements = ProjectRequirements::default();
        let resolved = ResolvedDependencies {
            dependencies: vec![DependencySpec {
                crate_name: "serde".to_string(),
                version: Some("1".to_string()),
                features: Vec::new(),
                default_features: true,
                source: DependencySource::Registry,
                optional: false,
                package: None,
            }],
            dev_dependencies: Vec::new(),
        };
        let lock = Some("[[package]]\nname = \"skip_probe\"\n".to_string());

        let out_dir = ensure_rust_inspect_workspace(
            tmp.path(),
            "skip_probe",
            Some("2021".to_string()),
            &resolved,
            &requirements,
            lock.clone(),
        )?;
        assert_eq!(
            super::test_rust_inspect_workspace_generations(&out_dir),
            1,
            "first call should generate the workspace"
        );

        ensure_rust_inspect_workspace(
            tmp.path(),
            "skip_probe",
            Some("2021".to_string()),
            &resolved,
            &requirements,
            lock,
        )?;
        assert_eq!(
            super::test_rust_inspect_workspace_generations(&out_dir),
            1,
            "second call with identical inputs should skip regeneration"
        );

        Ok(())
    }

    #[test]
    fn typecheck_modules_with_import_graph_accepts_valid_program() -> Result<(), Box<dyn std::error::Error>> {
        let module = parsed_module_for_test(
            r#"
def main() -> None:
    pass
"#,
        )?;

        typecheck_modules_with_import_graph(
            &[module],
            None,
            &LibraryManifestIndex::default(),
            #[cfg(feature = "rust_inspect")]
            None,
        )?;

        Ok(())
    }

    #[test]
    fn typecheck_modules_with_import_graph_reports_errors() -> Result<(), Box<dyn std::error::Error>> {
        let module = parsed_module_for_test(
            r#"
def main() -> None:
    missing_symbol()
"#,
        )?;

        let result = typecheck_modules_with_import_graph(
            &[module],
            None,
            &LibraryManifestIndex::default(),
            #[cfg(feature = "rust_inspect")]
            None,
        );
        assert!(result.is_err(), "expected unresolved symbol to fail typecheck");

        Ok(())
    }
}
