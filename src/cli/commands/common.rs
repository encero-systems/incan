//! Shared utilities used across multiple CLI command pipelines.
//!
//! This module contains functions for source file reading, module collection, project root resolution,
//! dependency helpers, and Cargo flag construction.

use std::collections::{BTreeSet, HashMap, HashSet};
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
use crate::frontend::library_manifest_index::LibraryManifestIndex;
use crate::frontend::module::{
    SourceModuleImportResolution, canonicalize_source_module_segments, resolve_program_source_imports,
};
use crate::frontend::{diagnostics, lexer, parser, vocab_desugar_pass};
use crate::lockfile::CargoFeatureSelection;
use crate::manifest::ProjectManifest;
use crate::manifest::{DependencySource, DependencySpec};
#[cfg(feature = "rust_inspect")]
use crate::rust_inspect::{Inspector, InspectorConfig};
use incan_core::lang::stdlib::{self, StdlibExtraCrateSource};
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

    // Legacy serde-driven surfaces (`@derive(Serialize/Deserialize)`, `to_json`, `json_stringify`) can still be used
    // without importing `std.serde.*`. Keep this as an explicit compatibility fallback, but treat import/provider
    // manifests as the primary source of dependency and feature requirements.
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
            let spec = match dep.source {
                StdlibExtraCrateSource::Version(version) => DependencySpec {
                    crate_name: dep.crate_name.to_string(),
                    version: Some(version.to_string()),
                    features: vec![],
                    default_features: true,
                    source: DependencySource::Registry,
                    optional: false,
                    package: None,
                },
                StdlibExtraCrateSource::Path(relative_path) => DependencySpec {
                    crate_name: dep.crate_name.to_string(),
                    version: None,
                    features: vec![],
                    default_features: true,
                    source: DependencySource::Path {
                        path: workspace_root.join(relative_path),
                    },
                    optional: false,
                    package: None,
                },
            }
            .normalized();

            merge_requirement_dependency(
                &mut requirements.dependencies,
                spec,
                format!("stdlib namespace `std.{namespace_name}`"),
            )?;
        }
    }

    if needs_legacy_serde_runtime {
        let serde = DependencySpec {
            crate_name: "serde".to_string(),
            version: Some("1.0".to_string()),
            features: vec!["derive".to_string()],
            default_features: true,
            source: DependencySource::Registry,
            optional: false,
            package: None,
        }
        .normalized();
        merge_requirement_dependency(
            &mut requirements.dependencies,
            serde,
            "legacy serde usage in source".to_string(),
        )?;

        let serde_json = DependencySpec {
            crate_name: "serde_json".to_string(),
            version: Some("1.0".to_string()),
            features: vec![],
            default_features: true,
            source: DependencySource::Registry,
            optional: false,
            package: None,
        }
        .normalized();
        merge_requirement_dependency(
            &mut requirements.dependencies,
            serde_json,
            "legacy serde usage in source".to_string(),
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

#[cfg(feature = "rust_inspect")]
const RUST_INSPECT_WORKSPACE_FINGERPRINT_FILE: &str = ".incan_rust_inspect_fingerprint";

#[cfg(feature = "rust_inspect")]
const RUST_INSPECT_WORKSPACE_FINGERPRINT_PREFIX: &str = "v1:";

/// Counts how many times the rust-inspect stub workspace is fully regenerated (not skipped via fingerprint).
/// Used by unit tests in this module; serialized with [`RUST_INSPECT_WORKSPACE_TEST_LOCK`].
#[cfg(all(test, feature = "rust_inspect"))]
pub(crate) static TEST_RUST_INSPECT_WORKSPACE_GENERATIONS: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

#[cfg(all(test, feature = "rust_inspect"))]
static RUST_INSPECT_WORKSPACE_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

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

/// Stable fingerprint for inputs that define the generated rust-inspect Cargo workspace under `target/incan_lock`.
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

/// Generate the rust-inspect workspace that semantic Rust extraction should query for this project.
///
/// The generated workspace intentionally uses the Rust import spelling for dependency keys, while preserving the
/// published Cargo package name separately when the two differ.
///
/// When the same inputs are seen again (for example across multiple `incan test` cases in one package), regeneration is
/// skipped if `target/incan_lock/.incan_rust_inspect_fingerprint` matches the computed digest and expected artifacts
/// exist.
#[cfg(feature = "rust_inspect")]
pub(crate) fn ensure_rust_inspect_workspace(
    project_root: &Path,
    project_name: &str,
    rust_edition: Option<String>,
    resolved: &ResolvedDependencies,
    project_requirements: &ProjectRequirements,
    cargo_lock_payload: Option<String>,
) -> CliResult<PathBuf> {
    let rust_inspect_manifest_dir = project_root.join("target").join("incan_lock");
    let fingerprint_path = rust_inspect_manifest_dir.join(RUST_INSPECT_WORKSPACE_FINGERPRINT_FILE);
    let cargo_toml_path = rust_inspect_manifest_dir.join("Cargo.toml");
    let main_rs_path = rust_inspect_manifest_dir.join("src").join("main.rs");

    let fingerprint = rust_inspect_workspace_fingerprint(
        project_name,
        rust_edition.as_deref(),
        resolved,
        &project_requirements.stdlib_features,
        cargo_lock_payload.as_deref(),
    );

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
    TEST_RUST_INSPECT_WORKSPACE_GENERATIONS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);

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

    fn should_prewarm_item(item_name: &str) -> bool {
        let stripped = item_name.trim_start_matches("r#");
        if matches!(
            stripped,
            "bool"
                | "char"
                | "str"
                | "f32"
                | "f64"
                | "i8"
                | "i16"
                | "i32"
                | "i64"
                | "i128"
                | "isize"
                | "u8"
                | "u16"
                | "u32"
                | "u64"
                | "u128"
                | "usize"
        ) {
            return false;
        }
        stripped
            .chars()
            .next()
            .is_some_and(|ch| ch.is_ascii_lowercase() || ch == '_')
    }

    // Default policy: prewarm likely callable *user* imports only. This avoids eager extraction of heavyweight
    // type/module imports (especially `incan_stdlib::*`) that can force expensive rust-analyzer def-map walks during
    // `incan test`.
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
                        if !primitive_ns && (prewarm_all || should_prewarm_item(&item.name)) {
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

/// Eagerly load rust-inspect metadata before typechecking/codegen hot paths.
#[cfg(feature = "rust_inspect")]
pub(crate) fn prewarm_rust_inspect_workspace(manifest_dir: &Path, query_paths: &[String]) -> CliResult<()> {
    let prewarm_enabled = std::env::var_os("INCAN_RUST_INSPECT_PREWARM").is_some_and(|value| {
        let value = value.to_string_lossy();
        matches!(value.as_ref(), "1" | "true" | "TRUE" | "on" | "ON")
    });
    if !prewarm_enabled {
        return Ok(());
    }
    if query_paths.is_empty() {
        return Ok(());
    }
    let inspector = Inspector::new(InspectorConfig::new(manifest_dir.to_path_buf()));
    inspector.prewarm(query_paths.iter().cloned(), &|_| ()).map_err(|err| {
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

/// Collect and parse the entry file and all its dependencies.
///
/// # Note on Prelude
///
/// The stdlib prelude (`stdlib/prelude.incn`) exists but is not currently wired into the compilation pipeline.
/// Prelude traits like `Debug`, `Display`, `Clone` are recognized by codegen heuristics rather than actual trait
/// definitions.
///
/// Future work: integrate prelude ASTs into typechecking so trait bounds are validated and derives work through actual
/// trait implementations.
pub fn collect_modules(entry_path: &str) -> CliResult<Vec<ParsedModule>> {
    let path = if Path::new(entry_path).is_absolute() {
        PathBuf::from(entry_path)
    } else {
        std::env::current_dir()
            .map_err(|e| CliError::failure(format!("failed to determine current directory: {e}")))?
            .join(entry_path)
    };
    let base_dir = path.parent().unwrap_or(Path::new("."));

    let inferred_project_root = resolve_project_root(&path);
    let manifest = ProjectManifest::discover(&inferred_project_root).map_err(|e| CliError::failure(e.to_string()))?;
    let project_root = manifest
        .as_ref()
        .map(|manifest| manifest.project_root().to_path_buf())
        .unwrap_or(inferred_project_root);
    let source_root = resolve_source_root(&project_root, manifest.as_ref());
    let library_manifest_index = manifest
        .as_ref()
        .and_then(|manifest| {
            (!manifest.library_dependencies().is_empty()).then(|| LibraryManifestIndex::from_project_manifest(manifest))
        })
        .unwrap_or_default();
    let library_imported_vocab = library_manifest_index.library_imported_vocab();
    let library_imported_dsl_surfaces = library_manifest_index.library_imported_dsl_surfaces();

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
        let tokens = match lexer::lex(&source) {
            Ok(t) => t,
            Err(errs) => {
                let mut msg = String::new();
                for err in &errs {
                    msg.push_str(&diagnostics::format_error(&file_path, &source, err));
                    msg.push('\n');
                }
                return Err(CliError::failure(msg.trim_end()));
            }
        };

        let mut ast = match parser::parse_with_context_and_surfaces(
            &tokens,
            Some(&file_path),
            Some(&library_imported_vocab),
            Some(&library_imported_dsl_surfaces),
        ) {
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
        if let Err(errs) =
            vocab_desugar_pass::desugar_program_vocab_blocks(&mut ast, Some(&file_path), &library_manifest_index)
        {
            let mut msg = String::new();
            for err in &errs {
                msg.push_str(&diagnostics::format_error(&file_path, &source, err));
                msg.push('\n');
            }
            return Err(CliError::failure(msg.trim_end()));
        }

        let current_base = Path::new(&file_path).parent().unwrap_or(base_dir);
        for resolved in resolve_program_source_imports(&ast, current_base, Some(&source_root)) {
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
fn topologically_sort_modules(
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

/// Build Cargo policy flags (`--locked` / `--frozen`).
pub(crate) fn cargo_policy_flags(locked: bool, frozen: bool) -> Vec<String> {
    if frozen {
        vec!["--frozen".to_string()]
    } else if locked {
        vec!["--locked".to_string()]
    } else {
        Vec::new()
    }
}

/// Build Cargo command flags (policy flags + feature flags).
pub(crate) fn cargo_command_flags(locked: bool, frozen: bool, cargo_features: &CargoFeatureSelection) -> Vec<String> {
    let mut flags = cargo_policy_flags(locked, frozen);
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

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frontend::typechecker;
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
@derive(Serialize)
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
                .any(|dep| dep.crate_name == "serde_json"),
            "serde usage should inject serde_json dependency"
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
    fn ensure_rust_inspect_workspace_uses_rust_safe_dependency_keys() -> Result<(), Box<dyn std::error::Error>> {
        use std::sync::atomic::Ordering;

        let _serial = super::RUST_INSPECT_WORKSPACE_TEST_LOCK
            .lock()
            .map_err(|e| format!("rust-inspect workspace test lock poisoned: {e}"))?;
        super::TEST_RUST_INSPECT_WORKSPACE_GENERATIONS.store(0, Ordering::SeqCst);

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
            super::TEST_RUST_INSPECT_WORKSPACE_GENERATIONS.load(Ordering::SeqCst),
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
        use std::sync::atomic::Ordering;

        let _serial = super::RUST_INSPECT_WORKSPACE_TEST_LOCK
            .lock()
            .map_err(|e| format!("rust-inspect workspace test lock poisoned: {e}"))?;
        super::TEST_RUST_INSPECT_WORKSPACE_GENERATIONS.store(0, Ordering::SeqCst);

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

        ensure_rust_inspect_workspace(
            tmp.path(),
            "skip_probe",
            Some("2021".to_string()),
            &resolved,
            &requirements,
            lock.clone(),
        )?;
        assert_eq!(
            super::TEST_RUST_INSPECT_WORKSPACE_GENERATIONS.load(Ordering::SeqCst),
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
            super::TEST_RUST_INSPECT_WORKSPACE_GENERATIONS.load(Ordering::SeqCst),
            1,
            "second call with identical inputs should skip regeneration"
        );

        Ok(())
    }
}
