//! Project requirements: the dependency set a command compiles against, assembled from the manifest, the selected
//! SDK providers, and the library artifacts those dependencies publish.
//!
//! Library dependency preparation lives here too, because a requirement is not satisfied until its artifact
//! exists and its profile receipts are verified.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{LazyLock, Mutex};
use std::{env, fs};

use incan_core::lang::stdlib;
use incan_core::lang::stdlib::{StdlibExtraCrateDep, StdlibExtraCrateSource};

use crate::dependency_resolver::ResolvedDependencies;
use crate::error::{ProviderError, ProviderResult};
use crate::vocab_extraction::collect_library_vocab_metadata_for_parser;
use crate::{PackageFeaturePlan, SDK_PROVIDER_BUILD_ENV, SdkArtifactProjection, SdkDependencyRebinding};
use incan_frontend::ast::ImportKind;
use incan_frontend::library_manifest::LibraryManifest;
use incan_frontend::library_manifest::published_layout::oven_library_dependency_declares_package_loaf;
use incan_frontend::library_manifest_index::{
    LibraryArtifactMetadata, LibraryManifestFailureKind, LibraryManifestIndex, LibraryManifestIndexEntry,
};
use incan_frontend::parsed_module::ParsedModule;
use incan_frontend::serde_usage::detect_serde_non_import_usage;
use oven_model::manifest::{
    DependencySource, DependencySpec, INTERNAL_MANIFEST_OVERRIDE_ENV, INTERNAL_PROJECT_ROOT_OVERRIDE_ENV,
    LOAF_MANIFEST_FILENAME, ProjectManifest,
};
use oven_model::toolchain_layout::GENERATED_CARGO_TARGET_DIR_ENV;
static PREPARED_LIBRARY_DEPENDENCIES: LazyLock<Mutex<HashMap<PathBuf, BTreeSet<String>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

pub const INTERNAL_LIBRARY_ARTIFACT_ONLY_ENV: &str = "INCAN_INTERNAL_LIBRARY_ARTIFACT_ONLY";

/// Internal marker for a nested `pub::` dependency library build.
///
/// Unlike artifact-only mode, an Oven direct-rustc dependency build must emit caller-owned rlibs. It still targets
/// exactly the dependency project selected by the parent, even if that project is the root of a larger workspace.
pub const INTERNAL_LIBRARY_DEPENDENCY_PREPARATION_ENV: &str = "INCAN_INTERNAL_LIBRARY_DEPENDENCY_PREPARATION";

/// Unified project requirements collected from parsed modules and loaded provider manifests.
#[derive(Debug, Clone, Default)]
pub struct ProjectRequirements {
    /// The standard library facets the program links beyond the mandatory `incan_std_core`, such as
    /// `incan_std_data` or `incan_std_web`, sorted.
    pub stdlib_facets: Vec<String>,
    /// Required Cargo dependencies contributed by stdlib namespaces and provider manifests.
    pub dependencies: Vec<DependencySpec>,
    /// Immutable compiled-library projections that replace obsolete physical SDK cache coordinates.
    pub sdk_dependency_rebindings: Vec<SdkDependencyRebinding>,
    /// Trusted active SDK/toolchain coordinates, including dependencies of unselected implementation facets in
    /// compiled providers. Feature flags are neutral here; selected links remain in `dependencies`.
    pub sdk_path_dependencies: Vec<DependencySpec>,
    /// Complete compiled-artifact closure whose transitive coordinates must be projected together.
    pub sdk_artifact_projections: Vec<SdkArtifactProjection>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DependencyManifestMode {
    /// Prepare a legacy source-compatible dependency manifest without baking a native library.
    FullArtifacts,
    /// Materialize direct-Rustc caller-owned libraries for a normal Oven consumer.
    OvenArtifacts,
    ParserOnly,
}

impl DependencyManifestMode {
    /// Return the caller-owned library artifact policy for this dependency preparation mode.
    pub fn library_dependency_preparation(self) -> Option<LibraryDependencyPreparation> {
        match self {
            Self::FullArtifacts => Some(LibraryDependencyPreparation::LegacyManifestOnly),
            Self::OvenArtifacts => Some(LibraryDependencyPreparation::OvenDirectRustc),
            Self::ParserOnly => None,
        }
    }

    /// Return whether this mode needs the checked library index after preparation.
    pub fn uses_materialized_library_index(self) -> bool {
        matches!(self, Self::FullArtifacts | Self::OvenArtifacts)
    }
}

/// Specify which owned artifact a local `pub::` dependency preparation must provide to its caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LibraryDependencyPreparation {
    /// Preserve the legacy preparation behavior used by commands that only require dependency metadata.
    LegacyManifestOnly,
    /// Produce metadata and profile-specific caller-owned rlibs through normal Oven direct-rustc library execution.
    OvenDirectRustc,
}

/// Decide whether session construction is inside the explicitly named legacy-Cargo provider publisher.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SdkInventorySource {
    /// Existing compatibility behavior for commands that still explicitly own legacy artifact preparation.
    PrepareLegacyCargoIfAbsent,
    /// Oven consumer mode: read an installed/prepared inventory only and never create Cargo state on a cache miss.
    DiscoverOnly,
}

/// Build a parser-only dependency manifest index for formatting and other collection-only entrypoints.
///
/// This deliberately does not write `.incnlib` artifacts. A source-derived parser manifest contains vocab registrations
/// and soft-keyword activations only, because collection parsing needs syntax context but not generated Rust artifacts,
/// checked exports, Rust ABI metadata, or a packaged desugarer. Load source dependency manifests without materializing
/// their legacy library artifacts.
///
/// This is intentionally available to Oven's lock validator so `--locked` and `--frozen` can retain canonical
/// freshness semantics without starting Cargo merely to inspect dependency metadata.
pub fn parser_only_library_manifest_index(
    manifest: &ProjectManifest,
    active_dependencies: &BTreeSet<String>,
) -> ProviderResult<LibraryManifestIndex> {
    let existing_index = LibraryManifestIndex::from_project_manifest_dependencies(
        manifest,
        active_dependencies.iter().map(String::as_str),
    );
    let mut entries = HashMap::new();

    for dependency_key in active_dependencies {
        let Some(dependency) = manifest.library_dependencies().get(dependency_key) else {
            continue;
        };
        match existing_index.get(dependency_key) {
            Some(LibraryManifestIndexEntry::Loaded { .. }) => {
                let Some(entry) = existing_index.get(dependency_key) else {
                    continue;
                };
                entries.insert(dependency_key.clone(), entry.clone());
            }
            Some(LibraryManifestIndexEntry::Failed(failure))
                if failure.kind == LibraryManifestFailureKind::ArtifactMissing
                    && dependency.path.join(LOAF_MANIFEST_FILENAME).is_file() =>
            {
                entries.insert(
                    dependency_key.clone(),
                    parser_only_library_manifest_entry(dependency_key, &dependency.path)?,
                );
            }
            Some(entry) => {
                entries.insert(dependency_key.clone(), entry.clone());
            }
            None => {}
        }
    }

    Ok(LibraryManifestIndex::from_entries(entries))
}

/// Derive the parser-visible portion of one source dependency's library manifest without writing package artifacts.
fn parser_only_library_manifest_entry(
    dependency_key: &str,
    dependency_root: &Path,
) -> ProviderResult<LibraryManifestIndexEntry> {
    let dependency_root = fs::canonicalize(dependency_root).unwrap_or_else(|_| dependency_root.to_path_buf());
    let manifest_path = dependency_root.join(LOAF_MANIFEST_FILENAME);
    let manifest_content = fs::read_to_string(&manifest_path)
        .map_err(|error| ProviderError::failure(format!("failed to read {}: {error}", manifest_path.display())))?;
    let dependency_manifest = ProjectManifest::from_str(&manifest_content, &manifest_path)
        .map_err(|error| ProviderError::failure(error.to_string()))?;
    let project_root = dependency_manifest.project_root().to_path_buf();
    let project_name = dependency_manifest
        .project
        .as_ref()
        .and_then(|project| project.name.clone())
        .or_else(|| {
            project_root
                .file_name()
                .and_then(|name| name.to_str())
                .map(str::to_owned)
        })
        .unwrap_or_else(|| dependency_key.to_string());
    let project_version = dependency_manifest
        .project
        .as_ref()
        .and_then(|project| project.version.clone())
        .unwrap_or_else(|| "0.1.0".to_string());
    let mut manifest = LibraryManifest::new(project_name.clone(), project_version);

    let generated_cargo_target_dir = env::var_os(GENERATED_CARGO_TARGET_DIR_ENV)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from);
    if let Some(vocab_extraction) = collect_library_vocab_metadata_for_parser(
        &dependency_manifest,
        &project_root,
        generated_cargo_target_dir.as_deref(),
    )? {
        manifest.vocab = Some(vocab_extraction.payload);
        manifest.soft_keywords.activations = vocab_extraction.compatibility_activations;
    }

    let metadata =
        LibraryArtifactMetadata::for_parser_source(dependency_key.to_string(), project_name, dependency_root);
    Ok(LibraryManifestIndexEntry::Loaded {
        manifest: Box::new(manifest),
        metadata,
    })
}

/// Ensure clean check/format/test entrypoints see the same public dependency manifests as warmed worktrees.
pub fn prepare_library_dependency_artifacts(
    manifest: &ProjectManifest,
    feature_plan: Option<&PackageFeaturePlan>,
    active_dependencies: &BTreeSet<String>,
    preparation: LibraryDependencyPreparation,
) -> ProviderResult<()> {
    if active_dependencies.is_empty() {
        return Ok(());
    }

    let initial_index = LibraryManifestIndex::from_project_manifest_dependencies(
        manifest,
        active_dependencies.iter().map(String::as_str),
    );
    let mut required = Vec::new();
    for dependency_key in active_dependencies {
        let Some(dependency) = manifest.library_dependencies().get(dependency_key) else {
            continue;
        };
        let expected_features = feature_plan
            .and_then(|plan| plan.package(&dependency.path))
            .map(|package| package.features.active_features.clone())
            .unwrap_or_default();
        let has_source_manifest = dependency.path.join(LOAF_MANIFEST_FILENAME).is_file();
        let needs_build = match initial_index.get(dependency_key) {
            Some(LibraryManifestIndexEntry::Loaded {
                manifest: artifact_manifest,
                metadata,
            }) => {
                let actual_features = &artifact_manifest.contract_metadata.provider.active_features;
                if actual_features != &expected_features && !has_source_manifest {
                    return Err(ProviderError::failure(format!(
                        "compiled dependency `pub::{dependency_key}` at {} was built with package features [{}], but this consumer requires [{}]; the producer source manifest is unavailable, so install or publish an artifact with the exact requested feature projection",
                        metadata.manifest_path.display(),
                        actual_features.iter().cloned().collect::<Vec<_>>().join(", "),
                        expected_features.iter().cloned().collect::<Vec<_>>().join(", "),
                    )));
                }
                actual_features != &expected_features
                    || matches!(preparation, LibraryDependencyPreparation::OvenDirectRustc)
                        && has_source_manifest
                        && !oven_library_dependency_has_verified_profile_receipts(&dependency.path)
                        && !oven_library_dependency_declares_package_loaf(&dependency.path)
            }
            Some(LibraryManifestIndexEntry::Failed(failure)) => {
                failure.kind == LibraryManifestFailureKind::ArtifactMissing
            }
            None => false,
        };
        if needs_build && has_source_manifest {
            required.push((dependency_key.clone(), dependency.path.clone(), expected_features));
        }
    }

    for (dependency_key, dependency_root, active_features) in required {
        if matches!(preparation, LibraryDependencyPreparation::OvenDirectRustc) {
            return Err(ProviderError::failure(format!(
                "Oven Alpha requires a baked package Loaf for pub::{dependency_key} at {}; run `incan oven bake --project {}` in that provider before preparing this consumer. Normal build, run, test, lock, and consumer bake will not compile the provider or invoke Cargo on its behalf",
                dependency_root.display(),
                dependency_root.display()
            )));
        }
        prepare_library_dependency_artifact(&dependency_key, &dependency_root, &active_features, preparation)?;
    }

    Ok(())
}

/// Return whether a materialized local `pub::` library is authorized for both normal Oven profiles.
///
/// A legacy generated artifact is not enough: consumers re-materialize the provider source through their selected
/// direct-Rustc cohort, which requires an identity-verified producer receipt for the debug and release profiles.
/// A local source manifest permits the existing nested Oven build to refresh either missing or malformed receipt.
fn oven_library_dependency_has_verified_profile_receipts(dependency_root: &Path) -> bool {
    let release = oven_store::default_receipt_path(dependency_root);
    let debug = release.with_file_name("library-debug-receipt.json");
    [release, debug].iter().all(|path| {
        fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<oven_store::OvenReceipt>(&bytes).ok())
            .is_some_and(|receipt| receipt.verify_identity().is_ok())
    })
}

/// Prepare one missing `pub::` dependency artifact through the existing library-mode compiler path.
fn prepare_library_dependency_artifact(
    dependency_key: &str,
    dependency_root: &Path,
    active_features: &BTreeSet<String>,
    preparation: LibraryDependencyPreparation,
) -> ProviderResult<()> {
    let canonical_root = fs::canonicalize(dependency_root).unwrap_or_else(|_| dependency_root.to_path_buf());
    {
        let prepared = PREPARED_LIBRARY_DEPENDENCIES
            .lock()
            .map_err(|_| ProviderError::failure("failed to lock prepared library dependency set"))?;
        if prepared.get(&canonical_root) == Some(active_features) {
            return Ok(());
        }
    }

    let preparation_label = match preparation {
        LibraryDependencyPreparation::LegacyManifestOnly => "metadata artifact",
        LibraryDependencyPreparation::OvenDirectRustc => "Oven direct-rustc library",
    };
    // A parent `--report json` reserves stdout for exactly one machine-readable document. The internal compiler
    // child inherits no report options, so route its progress through stderr before it can corrupt the parent's
    // aggregate report. The normal human command retains the existing concise progress line below.
    eprintln!(
        "Preparing missing pub::{dependency_key} {preparation_label} with `incan build --lib` in {}",
        dependency_root.display()
    );
    let current_exe = env::current_exe()
        .map_err(|error| ProviderError::failure(format!("failed to resolve current incan executable: {error}")))?;
    let mut command = Command::new(current_exe);
    command
        .args(["build", "--lib", "--no-default-features"])
        .current_dir(dependency_root)
        .env_remove(INTERNAL_MANIFEST_OVERRIDE_ENV)
        .env_remove(INTERNAL_PROJECT_ROOT_OVERRIDE_ENV)
        .env(INTERNAL_LIBRARY_DEPENDENCY_PREPARATION_ENV, "1")
        .stdout(Stdio::null());
    match preparation {
        LibraryDependencyPreparation::LegacyManifestOnly => {
            command.env(INTERNAL_LIBRARY_ARTIFACT_ONLY_ENV, "1");
        }
        LibraryDependencyPreparation::OvenDirectRustc => {
            // An inherited preparation flag would silently turn this child back into metadata-only output, leaving
            // the normal Oven consumer without a caller-owned rlib. Clear only that internal flag: the child keeps
            // the suite's sealed SDK inventory and direct-rustc selection context.
            command.env_remove(INTERNAL_LIBRARY_ARTIFACT_ONLY_ENV);
        }
    }
    if !active_features.is_empty() {
        command
            .arg("--features")
            .arg(active_features.iter().cloned().collect::<Vec<_>>().join(","));
    }
    let status = command.status().map_err(|error| {
        ProviderError::failure(format!(
            "failed to run `incan build --lib` for pub::{dependency_key} dependency at {}: {error}",
            dependency_root.display()
        ))
    })?;

    if !status.success() {
        return Err(ProviderError::failure(format!(
            "failed to prepare pub::{dependency_key} dependency artifact at {}",
            dependency_root.display()
        )));
    }

    let mut prepared = PREPARED_LIBRARY_DEPENDENCIES
        .lock()
        .map_err(|_| ProviderError::failure("failed to lock prepared library dependency set"))?;
    prepared.insert(canonical_root, active_features.clone());
    Ok(())
}

/// Collect a unified set of project requirements from source imports and loaded provider manifests.
pub fn collect_project_requirements(
    modules: &[ParsedModule],
    library_manifest_index: &LibraryManifestIndex,
) -> ProviderResult<ProjectRequirements> {
    // ---- Every `std.<namespace>` the collected modules import, the program's own and any stdlib source compiled in
    // ----
    let mut imported_stdlib_namespaces = HashSet::new();
    for module in modules {
        for decl in &module.ast.declarations {
            let incan_frontend::ast::Declaration::Import(import) = &decl.node else {
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
            if path.len() >= 2 && path[0] == stdlib::STDLIB_ROOT {
                imported_stdlib_namespaces.insert(path[1].clone());
            }
        }
    }
    // A namespace's extra crates are a provider fact: a consumer of a compiled provider inherits them through its
    // metadata, so only a provider build derives them from its own imports here.
    let mut stdlib_namespaces = if env::var_os(SDK_PROVIDER_BUILD_ENV).is_some() {
        imported_stdlib_namespaces.clone()
    } else {
        HashSet::new()
    };

    // The compiler-owned legacy bare `json_stringify` builtin can still be used without a provider import. Keep its
    // runtime requirement explicit until that compatibility surface is removed.
    let needs_legacy_serde_runtime = modules.iter().any(|module| detect_serde_non_import_usage(&module.ast));
    if needs_legacy_serde_runtime {
        stdlib_namespaces.insert("serde".to_string());
    }

    // ---- The facets: one per imported namespace by the registry's fact, then whatever a module's own Rust names ----
    // A namespace's facet is not a provider fact the consumer can defer: the emitter reaches the facet from the
    // namespace's compiled source (`std.collections` is Incan-authored and names no Rust, yet its ordinal-key bridges
    // are spelled through `incan_std_data`), so the program that imports the namespace links the facet. A compiled
    // provider records the same facet in its metadata, and the generator keeps one copy.
    if needs_legacy_serde_runtime {
        imported_stdlib_namespaces.insert("serde".to_string());
    }
    let mut stdlib_facets: BTreeSet<String> = BTreeSet::new();
    for namespace_name in &imported_stdlib_namespaces {
        let Some(namespace) = stdlib::find_namespace(namespace_name) else {
            continue;
        };
        if let Some(facet) = namespace.facet {
            stdlib_facets.insert(facet.to_string());
        }
    }
    // A module's own Rust names facets too: a component's sources reach their facet through
    // `rust.module("incan_std_<facet>")` and `from rust::incan_std_<facet>::…` without importing the namespace the
    // facet serves — the testing component is `std.testing`, it does not import it — so every facet a module's Rust
    // spells is linked. The dependency resolver deliberately drops these imports as toolchain-supplied; this is where
    // that supply is recorded.
    for module in modules {
        if let Some(directive) = &module.ast.rust_module_path
            && let Some(facet) = directive
                .node
                .split("::")
                .next()
                .filter(|first| stdlib::facets::is_facet(first))
        {
            stdlib_facets.insert(facet.to_string());
        }
        for decl in &module.ast.declarations {
            let incan_frontend::ast::Declaration::Import(import) = &decl.node else {
                continue;
            };
            let crate_name = match &import.kind {
                ImportKind::RustCrate { crate_name, .. } | ImportKind::RustFrom { crate_name, .. } => crate_name,
                _ => continue,
            };
            if stdlib::facets::is_facet(crate_name) {
                stdlib_facets.insert(crate_name.clone());
            }
        }
    }
    // A vocab manifest spells its runtime requirements in the vocabulary the contract had before the facets existed;
    // the registry says which facet serves each name.
    for requirement in library_manifest_index.merged_provider_required_stdlib_features() {
        let Some(facet) = stdlib::facets::for_requirement(&requirement) else {
            return Err(ProviderError::failure(format!(
                "a provider manifest requires the unknown standard library runtime `{requirement}`"
            )));
        };
        stdlib_facets.insert(facet.to_string());
    }

    let mut requirements = ProjectRequirements {
        stdlib_facets: stdlib_facets.into_iter().collect(),
        dependencies: Vec::new(),
        sdk_dependency_rebindings: Vec::new(),
        sdk_path_dependencies: Vec::new(),
        sdk_artifact_projections: Vec::new(),
    };
    for namespace_name in &stdlib_namespaces {
        let Some(namespace) = stdlib::find_namespace(namespace_name) else {
            continue;
        };
        for dep in namespace.extra_crate_deps {
            let spec = dependency_spec_from_stdlib_dep(dep);
            if matches!(spec.source, DependencySource::Path { .. }) {
                merge_sdk_path_dependency(
                    &mut requirements.sdk_path_dependencies,
                    spec.clone(),
                    format!("stdlib namespace `std.{namespace_name}` toolchain path"),
                )?;
            }
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
        if matches!(serde.source, DependencySource::Path { .. }) {
            merge_sdk_path_dependency(
                &mut requirements.sdk_path_dependencies,
                serde.clone(),
                "std.serde toolchain path".to_string(),
            )?;
        }
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
        .map_err(|err| ProviderError::failure(format!("failed to merge provider requirements: {err}")))?
    {
        merge_requirement_dependency(
            &mut requirements.dependencies,
            spec,
            "provider manifest requirement".to_string(),
        )?;
    }

    Ok(requirements)
}

/// Return the exact compiler-owned path catalog used only for semantic generated-artifact identity.
///
/// The catalog includes mandatory support crates, runtime facets reached by the program, and trusted active SDK
/// artifact coordinates. Those artifacts retain toolchain dependencies from unselected implementation facets too, so
/// their complete manifests have the same semantic identity for every consumer. Catalog entries do not activate links
/// or features.
pub fn semantic_sdk_path_dependencies(requirements: &ProjectRequirements) -> Vec<DependencySpec> {
    let mut dependencies = requirements.sdk_path_dependencies.clone();
    let toolchain_crates = incan_core::lang::generated_support::SUPPORT_CRATES_EVERY_PROGRAM_LINKS
        .into_iter()
        .chain(requirements.stdlib_facets.iter().map(String::as_str));
    for crate_name in toolchain_crates {
        if dependencies
            .iter()
            .any(|dependency| dependency.crate_name == crate_name)
        {
            continue;
        }
        dependencies.push(compiler_support_dependency_spec(crate_name));
    }
    dependencies.sort_by(|left, right| {
        (&left.crate_name, left.package.as_deref()).cmp(&(&right.crate_name, right.package.as_deref()))
    });
    dependencies
}

/// Describe one support crate emitted into every generated Cargo project as an exact compiler-owned path.
fn compiler_support_dependency_spec(crate_name: &str) -> DependencySpec {
    DependencySpec {
        crate_name: crate_name.to_string(),
        version: None,
        features: Vec::new(),
        default_features: false,
        source: DependencySource::Path {
            path: oven_model::toolchain_layout::resolve_toolchain_crate_path(crate_name),
        },
        optional: false,
        package: None,
    }
}

/// Build a dependency specification from a stdlib extra crate requirement.
fn dependency_spec_from_stdlib_extra_crate(crate_name: &str) -> ProviderResult<DependencySpec> {
    let dep = stdlib::find_extra_crate_dep(crate_name).ok_or_else(|| {
        ProviderError::failure(format!(
            "stdlib dependency metadata for `{crate_name}` is missing from the registry"
        ))
    })?;
    Ok(dependency_spec_from_stdlib_dep(dep))
}

/// Build a dependency specification from a stdlib dependency requirement.
fn dependency_spec_from_stdlib_dep(dep: &StdlibExtraCrateDep) -> DependencySpec {
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
                path: oven_model::toolchain_layout::resolve_toolchain_relative_path(Path::new(relative_path)),
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
pub fn merge_requirement_dependency(
    merged: &mut Vec<DependencySpec>,
    candidate: DependencySpec,
    source_label: String,
) -> ProviderResult<()> {
    if let Some(existing) = merged.iter().find(|dep| dep.crate_name == candidate.crate_name) {
        if !dependency_specs_match(existing, &candidate) {
            return Err(ProviderError::failure(format!(
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

/// Add a trusted SDK coordinate without treating a consumer's link features as catalog identity.
///
/// The catalog authorizes source paths and semantic artifact normalization; actual selected links are merged
/// separately. Facets can request different features of the same package without naming different trusted sources.
/// Package, version, optionality and canonical source conflicts remain errors.
pub(crate) fn merge_sdk_path_dependency(
    merged: &mut Vec<DependencySpec>,
    mut candidate: DependencySpec,
    source_label: String,
) -> ProviderResult<()> {
    candidate.features.clear();
    candidate.default_features = false;
    merge_requirement_dependency(merged, candidate, source_label)
}

/// Compare dependency specs while treating equivalent path spellings as the same dependency.
pub fn dependency_specs_match(left: &DependencySpec, right: &DependencySpec) -> bool {
    if left == right {
        return true;
    }
    let mut left = left.clone();
    let mut right = right.clone();
    for spec in [&mut left, &mut right] {
        if let DependencySource::Path { path } = &mut spec.source {
            *path = fs::canonicalize(&*path).unwrap_or_else(|_| path.clone());
        }
    }
    left == right
}

/// Merge collected requirement dependencies into resolved dependency sets.
///
/// Existing entries with the same crate name must be compatible.
pub fn merge_project_requirement_dependencies(
    resolved: &mut ResolvedDependencies,
    requirements: &ProjectRequirements,
) -> ProviderResult<()> {
    for required in &requirements.dependencies {
        let already_in_dependencies = resolved
            .dependencies
            .iter()
            .find(|spec| spec.crate_name == required.crate_name);
        if let Some(existing) = already_in_dependencies {
            if !dependency_specs_match(existing, required) {
                return Err(ProviderError::failure(format!(
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
                return Err(ProviderError::failure(format!(
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FeatureSelection;
    use crate::test_support::parsed_module_for_test;
    use incan_frontend::library_manifest::ProviderFeatureMetadata;
    use std::collections::BTreeMap;

    /// Link flags do not select another source, but every retained coordinate field still constrains trust.
    #[test]
    fn sdk_path_catalog_normalizes_link_flags_and_rejects_coordinate_conflicts()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let source = root.path().join("runtime");
        fs::create_dir_all(&source)?;
        let candidate = DependencySpec {
            crate_name: "runtime".to_string(),
            package: Some("runtime-package".to_string()),
            version: Some("1.0".to_string()),
            features: vec!["first".to_string()],
            default_features: true,
            optional: false,
            source: DependencySource::Path { path: source.clone() },
        };
        let mut catalog = Vec::new();
        merge_sdk_path_dependency(&mut catalog, candidate.clone(), "first".to_string())?;
        let mut equivalent = candidate.clone();
        equivalent.features = vec!["other".to_string()];
        equivalent.default_features = false;
        equivalent.source = DependencySource::Path { path: source.join(".") };
        merge_sdk_path_dependency(&mut catalog, equivalent, "equivalent source".to_string())?;
        assert_eq!(catalog.len(), 1);
        assert!(catalog[0].features.is_empty());
        assert!(!catalog[0].default_features);
        let mut changed_path = candidate.clone();
        changed_path.source = DependencySource::Path {
            path: root.path().join("other"),
        };
        let mut changed_version = candidate.clone();
        changed_version.version = Some("2.0".to_string());
        let mut changed_package = candidate.clone();
        changed_package.package = Some("other-package".to_string());
        let mut changed_optional = candidate;
        changed_optional.optional = true;
        for conflict in [changed_path, changed_version, changed_package, changed_optional] {
            let previous = catalog.clone();
            assert!(merge_sdk_path_dependency(&mut catalog, conflict, "conflict".to_string()).is_err());
            assert_eq!(catalog, previous, "a rejected coordinate must not change the catalog");
        }
        Ok(())
    }

    #[test]
    fn an_imported_namespace_links_its_facet_while_its_crates_stay_provider_facts()
    -> Result<(), Box<dyn std::error::Error>> {
        let module = parsed_module_for_test(
            r#"
import std.async
from std.math import sqrt
"#,
        )?;

        let requirements = collect_project_requirements(&[module], &LibraryManifestIndex::default())?;
        assert_eq!(requirements.stdlib_facets, ["incan_std_async"]);
        assert!(requirements.dependencies.is_empty());
        Ok(())
    }

    #[test]
    fn an_imported_serde_namespace_links_the_data_facet_and_defers_its_runtime_to_provider_facts()
    -> Result<(), Box<dyn std::error::Error>> {
        let module = parsed_module_for_test(
            r#"
from std.serde import json

@derive(json)
model User:
    name: str
"#,
        )?;

        let requirements = collect_project_requirements(&[module], &LibraryManifestIndex::default())?;
        assert_eq!(requirements.stdlib_facets, ["incan_std_data"]);
        assert!(requirements.dependencies.is_empty());
        Ok(())
    }

    #[test]
    fn an_incan_authored_namespace_links_the_facet_the_emitter_reaches_for_it() -> Result<(), Box<dyn std::error::Error>>
    {
        // `std.collections` names no Rust of its own; the emitter spells its ordinal-key bridges through the data
        // facet, so importing the namespace is what links the crate.
        let module = parsed_module_for_test(
            r#"
from std.collections import OrdinalMap

def main() -> None:
    pass
"#,
        )?;

        let requirements = collect_project_requirements(&[module], &LibraryManifestIndex::default())?;
        assert_eq!(requirements.stdlib_facets, ["incan_std_data"]);
        Ok(())
    }

    #[test]
    fn a_program_importing_no_namespace_links_no_facet() -> Result<(), Box<dyn std::error::Error>> {
        let module = parsed_module_for_test(
            r#"
def main() -> None:
    println("hi")
"#,
        )?;

        let requirements = collect_project_requirements(&[module], &LibraryManifestIndex::default())?;
        assert!(requirements.stdlib_facets.is_empty());
        Ok(())
    }

    #[test]
    fn a_module_links_every_facet_its_own_rust_names() -> Result<(), Box<dyn std::error::Error>> {
        // A component's sources reach their facet without importing the namespace it serves.
        let own_facet = parsed_module_for_test(
            r#"
rust.module("incan_std_testing")

def main() -> None:
    pass
"#,
        )?;
        let inline = parsed_module_for_test(
            r#"
from rust::incan_std_data::json import JsonValue
from rust::serde_json import Value

def main() -> None:
    pass
"#,
        )?;

        let requirements = collect_project_requirements(&[own_facet, inline], &LibraryManifestIndex::default())?;
        assert_eq!(requirements.stdlib_facets, ["incan_std_data", "incan_std_testing"]);
        Ok(())
    }

    #[test]
    fn source_requirements_do_not_rediscover_math_provider_dependencies() -> Result<(), Box<dyn std::error::Error>> {
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

        assert!(resolved.dependencies.is_empty());
        Ok(())
    }

    #[test]
    fn source_requirements_do_not_rediscover_io_provider_dependencies() -> Result<(), Box<dyn std::error::Error>> {
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

        assert!(resolved.dependencies.is_empty());
        Ok(())
    }

    #[test]
    fn artifact_only_dependency_rejects_a_stale_feature_projection() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let dependency_root = tmp.path().join("feature_library");
        let artifact_root = dependency_root.join("target/lib");
        fs::create_dir_all(artifact_root.join("src"))?;
        fs::write(
            artifact_root.join("Cargo.toml"),
            "[package]\nname = \"feature_library\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )?;
        fs::write(artifact_root.join("src/lib.rs"), "pub fn alpha() {}\n")?;
        let mut artifact = LibraryManifest::new("feature_library", "0.1.0");
        artifact.contract_metadata.provider.public_features = BTreeMap::from([
            ("alpha".to_string(), ProviderFeatureMetadata::default()),
            ("beta".to_string(), ProviderFeatureMetadata::default()),
        ]);
        artifact.contract_metadata.provider.active_features = BTreeSet::from(["alpha".to_string()]);
        artifact.write_to_path(&artifact_root.join("feature_library.incnlib"))?;

        let consumer_root = tmp.path().join("consumer");
        fs::create_dir_all(&consumer_root)?;
        fs::write(
            consumer_root.join(LOAF_MANIFEST_FILENAME),
            "[project]\nname = \"consumer\"\n\n[dependencies]\nfeature_library = { path = \"../feature_library\", features = [\"beta\"], default-features = false }\n",
        )?;
        let consumer = ProjectManifest::discover(&consumer_root)?.ok_or("missing consumer manifest")?;
        let error = PackageFeaturePlan::resolve(&consumer, &FeatureSelection::default())
            .err()
            .ok_or("stale artifact-only feature projection should fail")?;
        let message = error.to_string();

        assert!(message.contains("was built with package features [alpha]"));
        assert!(message.contains("requires [beta]"));
        Ok(())
    }

    #[test]
    fn oven_dependency_discovery_materializes_while_legacy_preparation_remains_metadata_only() {
        assert_eq!(
            DependencyManifestMode::FullArtifacts.library_dependency_preparation(),
            Some(LibraryDependencyPreparation::LegacyManifestOnly)
        );
        assert_eq!(
            DependencyManifestMode::OvenArtifacts.library_dependency_preparation(),
            Some(LibraryDependencyPreparation::OvenDirectRustc)
        );
        assert_eq!(
            DependencyManifestMode::ParserOnly.library_dependency_preparation(),
            None
        );
        assert!(DependencyManifestMode::FullArtifacts.uses_materialized_library_index());
        assert!(DependencyManifestMode::OvenArtifacts.uses_materialized_library_index());
        assert!(!DependencyManifestMode::ParserOnly.uses_materialized_library_index());
    }

    #[test]
    fn oven_library_dependency_requires_verified_debug_and_release_receipts() -> Result<(), Box<dyn std::error::Error>>
    {
        let temp_dir = tempfile::tempdir()?;
        let project_root = temp_dir.path();
        let generated_source = project_root.join("target/lib/src/lib.rs");
        fs::create_dir_all(generated_source.parent().ok_or("generated source has no parent")?)?;
        fs::write(&generated_source, "pub fn provider() {}\n")?;

        let receipt = |profile| {
            oven_store::receipt_generated_project(
                &oven_store::OvenGeneratedProjectRequest::new(
                    project_root,
                    "provider",
                    "0.1.0",
                    "aarch64-apple-darwin",
                    "rustc test",
                    profile,
                    Vec::new(),
                )
                .with_generated_source("generated-root", &generated_source),
            )
        };
        let release_path = oven_store::default_receipt_path(project_root);
        oven_store::write_receipt(&receipt("release")?, &release_path)?;
        assert!(!oven_library_dependency_has_verified_profile_receipts(project_root));

        let debug_path = release_path.with_file_name("library-debug-receipt.json");
        oven_store::write_receipt(&receipt("debug")?, &debug_path)?;
        assert!(oven_library_dependency_has_verified_profile_receipts(project_root));

        fs::write(&debug_path, "not a receipt")?;
        assert!(!oven_library_dependency_has_verified_profile_receipts(project_root));
        Ok(())
    }
}
