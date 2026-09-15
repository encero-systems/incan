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
use oven_model::toolchain_layout::GENERATED_TOOLCHAIN_SUPPORT_CRATES;
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
    /// Required stdlib feature flags, such as `json`, `async`, and `web`.
    pub stdlib_features: Vec<String>,
    /// Required Cargo dependencies contributed by stdlib namespaces and provider manifests.
    pub dependencies: Vec<DependencySpec>,
    /// Immutable compiled-library projections that replace obsolete physical SDK cache coordinates.
    pub sdk_dependency_rebindings: Vec<SdkDependencyRebinding>,
    /// Path dependencies proven to be owned by the active SDK/toolchain rather than an ordinary project source.
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
/// This deliberately does not write `.incnlib` artifacts. A source-derived parser manifest contains vocab
/// registrations and soft-keyword activations only, because collection parsing needs syntax context but not generated
/// Rust artifacts, checked exports, Rust ABI metadata, or a packaged desugarer.
/// Load source dependency manifests without materializing their legacy library artifacts.
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
    let mut stdlib_namespaces = HashSet::new();
    if env::var_os(SDK_PROVIDER_BUILD_ENV).is_some() {
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
                    stdlib_namespaces.insert(path[1].clone());
                }
            }
        }
    }

    // The compiler-owned legacy bare `json_stringify` builtin can still be used without a provider import. Keep its
    // runtime requirement explicit until that compatibility surface is removed.
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
                merge_requirement_dependency(
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
            merge_requirement_dependency(
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
pub fn semantic_sdk_path_dependencies(requirements: &ProjectRequirements) -> Vec<DependencySpec> {
    let mut dependencies = requirements.sdk_path_dependencies.clone();
    for crate_name in GENERATED_TOOLCHAIN_SUPPORT_CRATES {
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
        default_features: true,
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

    #[test]
    fn collect_project_requirements_defers_sdk_namespace_features_to_provider_facts()
    -> Result<(), Box<dyn std::error::Error>> {
        let module = parsed_module_for_test(
            r#"
import std.async
from std.math import sqrt
"#,
        )?;

        let requirements = collect_project_requirements(&[module], &LibraryManifestIndex::default())?;
        assert!(requirements.stdlib_features.is_empty());
        assert!(requirements.dependencies.is_empty());
        Ok(())
    }

    #[test]
    fn collect_project_requirements_defers_imported_serde_runtime_to_provider_facts()
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
        assert!(requirements.stdlib_features.is_empty());
        assert!(requirements.dependencies.is_empty());
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
