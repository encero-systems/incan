//! Shared utilities used across multiple CLI command pipelines.
//!
//! This module contains functions for source file reading, module collection, project root resolution,
//! dependency helpers, and SDK provider input evidence.

#[cfg(test)]
use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};

use crate::backend::c_abi::{CAbiVerificationPlan, ClangToolchain, verify_checked_c_binding};
use crate::backend::ir::detect_serde_non_import_usage;
use crate::backend::project::{GENERATED_TOOLCHAIN_SUPPORT_CRATES, INCAN_STDLIB_CRATE_NAME};
use crate::cli::prelude::ParsedModule;
use crate::cli::{CliError, CliResult};
use crate::dependency_resolver::ResolvedDependencies;
use crate::dependency_resolver::{DependencyError, InlineRustImport};
use crate::frontend::ast::{ImportKind, ImportPath, Program, Span};
use crate::frontend::contract_metadata::{
    CanonicalModelBundle, materialize_contract_models, read_project_model_bundles,
};
use crate::frontend::hir::build_semantic_module_snapshot_v0;
use crate::frontend::library_manifest_index::{
    LibraryArtifactMetadata, LibraryManifestFailureKind, LibraryManifestIndex, LibraryManifestIndexEntry,
};
use crate::frontend::module::{
    SourceModuleImportResolution, canonicalize_source_module_segments, logical_module_segments_from_file,
    logical_source_import_candidates, resolve_program_source_imports, self_import_diagnostic_message,
};
use crate::frontend::testing_markers::{
    TestingMarkerSemantics, load_testing_marker_semantics, testing_marker_semantics_from_manifest,
};
use crate::frontend::typechecker::stdlib_loader::StdlibAstCache;
use crate::frontend::typechecker::{CBindingDescriptor, TypeCheckInfo};
use crate::frontend::{ast_walk, diagnostics, lexer, parser, typechecker, vocab_desugar_pass};
use crate::library_manifest::{LibraryManifest, ProviderCargoDependency, ProviderCargoDependencySource};
use crate::manifest::{
    CARGO_MANIFEST_FILENAME, DiscoveredManifest, LOAF_MANIFEST_FILENAME, ManifestError, ProjectManifest,
    discovered_manifest_kind,
};
use crate::manifest::{DependencySource, DependencySpec};
use crate::project_lifecycle::toolchain::ToolchainConstraintSet;
use crate::provider::{
    BackendImplementationRequirement, FeatureSelection, PackageFeatureGraph, PackageFeaturePlan,
    ProviderModuleResolution, ProviderPlan, ProviderProvenance, ResolvedSdkComponents, SDK_INVENTORY_FILE,
    SDK_PROVIDER_BUILD_ENV, SDK_SOURCE_CATALOG_FILE, SdkArtifactProjection, SdkComponentSelection,
    SdkDependencyRebinding, SdkInventory, SdkResolutionError, SdkSourceCatalog,
};
use crate::workspace::WorkspaceGraph;
use incan_core::lang::{
    stdlib::{self, StdlibExtraCrateDep, StdlibExtraCrateSource},
    surface::result_methods,
};

use super::vocab_extraction::collect_library_vocab_metadata_for_parser;

/// Maximum source file size (100 MB)
///
/// Files larger than this are rejected to prevent out-of-memory conditions during compilation.
const MAX_SOURCE_SIZE: u64 = 100 * 1024 * 1024;
/// Project roots already told that their `Cargo.toml` is ignored, so one command warns once.
///
/// Oven prepares a project more than once per command — once per selected profile, and again for a caller-owned
/// library graph — so emitting at the preparation boundary without this would repeat the same line several times for
/// a single `incan build`. Keyed by project root rather than a global flag, so a workspace build still reports each
/// member that has one.
static IGNORED_CARGO_MANIFESTS_REPORTED: LazyLock<Mutex<HashSet<PathBuf>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));
/// Shared immutable provider projections indexed by the canonical modules an invocation uses.
type ProviderPlanCache = Arc<Mutex<BTreeMap<BTreeSet<Vec<String>>, Arc<ProviderPlan>>>>;
pub(crate) const INTERNAL_LIBRARY_ARTIFACT_ONLY_ENV: &str = "INCAN_INTERNAL_LIBRARY_ARTIFACT_ONLY";
/// Explicit active SDK inventory override used by toolchain selection and SDK publication.
pub(crate) const SDK_INVENTORY_OVERRIDE_ENV: &str = "INCAN_SDK_INVENTORY";

#[cfg(test)]
thread_local! {
    static COMPILATION_SESSION_ANALYSIS_INVOCATIONS: Cell<Option<usize>> = const { Cell::new(None) };
}

/// Scoped, current-thread instrumentation for calls to [`CompilationSession::analyze_modules`].
///
/// The counter is inactive unless this scope is constructed, and it restores any enclosing scope when dropped. This
/// keeps structural command-path assertions isolated from Rust's parallel test execution while leaving production
/// analysis behavior unchanged.
#[cfg(test)]
#[must_use = "keep the scope alive while observing compilation-session analysis invocations"]
pub(crate) struct CompilationSessionAnalysisInvocationScope {
    previous_count: Option<usize>,
}

/// Count compilation-session analysis invocations within a scope on the current test thread.
#[cfg(test)]
pub(crate) fn scoped_compilation_session_analysis_invocations() -> CompilationSessionAnalysisInvocationScope {
    let previous_count = COMPILATION_SESSION_ANALYSIS_INVOCATIONS.with(|count| count.replace(Some(0)));
    CompilationSessionAnalysisInvocationScope { previous_count }
}

#[cfg(test)]
impl CompilationSessionAnalysisInvocationScope {
    /// Return the analysis calls observed since this scope was constructed.
    #[cfg(test)]
    pub(crate) fn invocation_count(&self) -> usize {
        COMPILATION_SESSION_ANALYSIS_INVOCATIONS.with(|count| count.get().unwrap_or_default())
    }
}

#[cfg(test)]
impl Drop for CompilationSessionAnalysisInvocationScope {
    fn drop(&mut self) {
        COMPILATION_SESSION_ANALYSIS_INVOCATIONS.with(|count| count.set(self.previous_count));
    }
}

/// Record one invocation when the current test thread has explicitly enabled the analysis counter.
#[cfg(test)]
fn record_compilation_session_analysis_invocation() {
    COMPILATION_SESSION_ANALYSIS_INVOCATIONS.with(|counter| {
        let Some(count) = counter.get() else {
            return;
        };
        counter.set(Some(count.saturating_add(1)));
    });
}

/// One compiler diagnostic with enough source context for either human or machine-readable rendering.
#[derive(Debug, Clone)]
pub(crate) struct CliDiagnostic {
    pub file_path: String,
    pub source: String,
    pub error: diagnostics::CompileError,
    pub phase: diagnostics::DiagnosticPhase,
}

/// Structured failure produced by shared CLI collection/typechecking helpers.
#[derive(Debug, Clone)]
pub(crate) struct CliDiagnosticFailure {
    pub diagnostics: Vec<CliDiagnostic>,
    /// Non-fatal diagnostics gathered before the failure, reported but never re-rendered.
    ///
    /// Kept apart from `diagnostics` because these have already been printed to stderr as they were produced.
    /// Folding them in would print them twice for a human, while dropping them would make the machine-readable
    /// report inconsistent: a file with both a warning and an error would report only the error, even though the
    /// same file reports the warning fine when it compiles.
    pub warnings: Vec<CliDiagnostic>,
}

impl CliDiagnosticFailure {
    /// Build one structured diagnostic failure while preserving the source text needed for JSON span projection.
    pub(crate) fn single(
        file_path: impl Into<String>,
        source: impl Into<String>,
        error: diagnostics::CompileError,
        phase: diagnostics::DiagnosticPhase,
    ) -> Self {
        Self {
            diagnostics: vec![CliDiagnostic {
                file_path: file_path.into(),
                source: source.into(),
                error,
                phase,
            }],
            warnings: Vec::new(),
        }
    }

    /// Build one structured failure from parser or typechecker errors that all belong to the same source file.
    pub(crate) fn from_errors(
        file_path: impl Into<String>,
        source: impl Into<String>,
        errors: Vec<diagnostics::CompileError>,
        phase: diagnostics::DiagnosticPhase,
    ) -> Self {
        let file_path = file_path.into();
        let source = source.into();
        Self {
            diagnostics: errors
                .into_iter()
                .map(|error| CliDiagnostic {
                    file_path: file_path.clone(),
                    source: source.clone(),
                    error,
                    phase,
                })
                .collect(),
            warnings: Vec::new(),
        }
    }

    /// Render the failing diagnostics through the existing source-highlighted human diagnostic formatter.
    ///
    /// Warnings are deliberately excluded: they were already shown when they were produced.
    pub(crate) fn render_human(&self) -> String {
        let mut rendered = String::new();
        for diagnostic in &self.diagnostics {
            rendered.push_str(&diagnostics::format_error(
                &diagnostic.file_path,
                &diagnostic.source,
                &diagnostic.error,
            ));
            rendered.push('\n');
        }
        rendered.trim_end().to_string()
    }
}

impl From<CliError> for CliDiagnosticFailure {
    fn from(error: CliError) -> Self {
        Self::single(
            "<command>",
            "",
            diagnostics::CompileError::new(error.message, Span::default()),
            diagnostics::DiagnosticPhase::Tooling,
        )
    }
}

#[derive(Debug, Clone)]
struct SourceReadFailure {
    message: String,
}

/// Unified project requirements collected from parsed modules and loaded provider manifests.
#[derive(Debug, Clone, Default)]
pub(crate) struct ProjectRequirements {
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

/// Discover the active component-aware SDK relative to the selected toolchain or an explicit override.
/// Discover the installed SDK inventory without publishing source-checkout providers.
///
/// Oven consumers use this narrow read-only path. A normal command must treat an absent inventory as an explicit
/// preparation requirement, never as authority to invoke the legacy Cargo publisher.
pub(crate) fn discover_active_sdk_inventory() -> CliResult<Option<Arc<SdkInventory>>> {
    let explicit = env::var_os(SDK_INVENTORY_OVERRIDE_ENV)
        .filter(|path| !path.is_empty())
        .map(PathBuf::from);
    let inventory_path = if let Some(path) = explicit.as_ref() {
        if !path.is_file() {
            return Err(CliError::failure(format!(
                "{SDK_INVENTORY_OVERRIDE_ENV} points to missing SDK inventory {}",
                path.display()
            )));
        }
        Some(path.clone())
    } else {
        crate::toolchain_layout::current_executable_search_bases()
            .into_iter()
            .flat_map(|base| {
                [
                    base.join(SDK_INVENTORY_FILE),
                    base.join("share").join("incan").join(SDK_INVENTORY_FILE),
                    base.join("share").join("incan").join("sdk").join(SDK_INVENTORY_FILE),
                ]
            })
            .find(|path| path.is_file())
    };
    let Some(path) = inventory_path else {
        return Ok(None);
    };
    let inventory = SdkInventory::read_from_path(&path).map_err(|error| CliError::failure(error.to_string()))?;
    inventory
        .validate_compiler_compatibility(
            crate::version::INCAN_VERSION,
            crate::version::SDK_PROVIDER_CODEGEN_REVISION,
        )
        .map_err(|error| CliError::failure(error.to_string()))?;
    Ok(Some(Arc::new(inventory)))
}

/// Reject explicit component-aware selection when the active toolchain exposes only the legacy monolithic SDK.
fn validate_component_inventory_selection(
    manifest: Option<&ProjectManifest>,
    sdk_profile_override: Option<&str>,
    inventory: Option<&SdkInventory>,
) -> CliResult<()> {
    let explicit_selection = manifest.and_then(ProjectManifest::sdk).is_some() || sdk_profile_override.is_some();
    if inventory.is_some() || !explicit_selection {
        return Ok(());
    }
    let message = concat!(
        "the active Incan SDK has no component inventory, so explicit `[sdk]` selection is unavailable; ",
        "use a component-aware v0.5 SDK or remove the explicit SDK selection",
    );
    let message = manifest
        .and_then(|manifest| sdk_manifest_value_location(manifest, &[]))
        .map_or_else(|| message.to_string(), |location| format!("{location}: {message}"));
    Err(CliError::failure(message))
}

/// Resolve one SDK component selection and retain the manifest or command provenance of configuration failures.
pub(crate) fn resolve_sdk_component_selection(
    inventory: &SdkInventory,
    selection: &SdkComponentSelection,
    manifest: Option<&ProjectManifest>,
    sdk_profile_override: Option<&str>,
    require_available: bool,
) -> CliResult<ResolvedSdkComponents> {
    let result = if require_available {
        inventory.resolve(selection)
    } else {
        inventory.resolve_catalog(selection)
    };
    result.map_err(|error| {
        CliError::failure(format_sdk_selection_error(
            &error,
            selection,
            manifest,
            sdk_profile_override,
        ))
    })
}

/// Attach exact `[sdk]` source provenance to one component-resolution failure when it came from the manifest.
fn format_sdk_selection_error(
    error: &SdkResolutionError,
    selection: &SdkComponentSelection,
    manifest: Option<&ProjectManifest>,
    sdk_profile_override: Option<&str>,
) -> String {
    if matches!(error, SdkResolutionError::UnknownProfile { profile, .. } if sdk_profile_override == Some(profile)) {
        return format!("{error} (selected by the current command's `--sdk-profile` override)");
    }
    let candidates = match error {
        SdkResolutionError::UnknownProfile { profile, .. } => vec![profile.clone()],
        SdkResolutionError::UnknownComponent { component, .. }
        | SdkResolutionError::MandatoryComponentExcluded { component, .. }
        | SdkResolutionError::SelectedComponentExcluded { component }
        | SdkResolutionError::ExcludedRequiredComponent { component, .. }
        | SdkResolutionError::EnabledComponentUnavailable { component, .. } => {
            vec![component.clone(), selection.profile.clone()]
        }
    };
    manifest
        .and_then(|manifest| sdk_manifest_value_location(manifest, &candidates))
        .map_or_else(|| error.to_string(), |location| format!("{location}: {error}"))
}

/// Locate an authored SDK selection value, falling back to the `[sdk]` table header for derived failures.
fn sdk_manifest_value_location(manifest: &ProjectManifest, candidates: &[String]) -> Option<String> {
    let content = fs::read_to_string(manifest.path()).ok()?;
    let mut in_sdk = false;
    let mut sdk_header = None;
    let mut sdk_lines = Vec::new();
    for (line_index, line) in content.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            in_sdk = trimmed == "[sdk]";
            if in_sdk {
                sdk_header = Some(format!("{}:{}:1", manifest.path().display(), line_index + 1));
            }
            continue;
        }
        if !in_sdk {
            continue;
        }
        sdk_lines.push((line_index, line));
    }
    for candidate in candidates {
        for (line_index, line) in &sdk_lines {
            let quoted = format!("\"{candidate}\"");
            if let Some(column) = line.find(&quoted) {
                return Some(format!(
                    "{}:{}:{}",
                    manifest.path().display(),
                    line_index + 1,
                    column + 1
                ));
            }
        }
    }
    sdk_header
}

/// Add linked generated crates selected by active compiled providers to the current backend requirements.
pub(crate) fn extend_requirements_with_provider_plan(
    requirements: &mut ProjectRequirements,
    provider_plan: &ProviderPlan,
) -> CliResult<()> {
    let sdk_providers = provider_plan
        .sdk_link_roots()
        .into_iter()
        .map(|provider| provider.identity.stable_key())
        .collect::<BTreeSet<_>>();
    extend_requirements_with_selected_sdk_providers(requirements, provider_plan, &sdk_providers)
}

/// Add ordinary providers and the selected direct SDK provider set to backend requirements.
fn extend_requirements_with_selected_sdk_providers(
    requirements: &mut ProjectRequirements,
    provider_plan: &ProviderPlan,
    sdk_providers: &BTreeSet<String>,
) -> CliResult<()> {
    // Projection helpers do not retain the ProviderPlan. Preserve the complete active SDK path catalog separately
    // from the minimal set of providers linked directly into this generated crate: a copied compiled artifact can
    // still carry a non-descriptor Cargo edge to an active provider supplied transitively or unused by this consumer.
    for provider in provider_plan.active_sdk_records() {
        if let Some(artifact) = provider.artifact.as_ref() {
            merge_requirement_dependency(
                &mut requirements.sdk_path_dependencies,
                artifact.to_dependency_spec(),
                format!("active SDK provider `{}`", provider.identity.name),
            )?;
        }
        for requirement in provider_plan.selected_backend_requirements(provider) {
            let BackendImplementationRequirement::CargoDependency { dependency } = requirement else {
                continue;
            };
            if matches!(dependency.source, ProviderCargoDependencySource::Toolchain { .. }) {
                merge_requirement_dependency(
                    &mut requirements.sdk_path_dependencies,
                    provider_cargo_dependency_spec(&dependency),
                    format!("active SDK provider `{}` toolchain dependency", provider.identity.name),
                )?;
            }
        }
    }
    for provider in provider_plan.active_records() {
        if matches!(provider.authority, crate::provider::NamespaceAuthority::SdkReserved)
            && !sdk_providers.contains(&provider.identity.stable_key())
        {
            continue;
        }
        let Some(artifact) = provider.artifact.as_ref() else {
            continue;
        };
        let mut provider_dependency = artifact.to_dependency_spec();
        if matches!(provider.authority, crate::provider::NamespaceAuthority::SdkReserved) {
            // Checked private SDK edges freeze an exact feature projection and never inherit the provider crate's
            // conventional Cargo defaults. Emit that contract explicitly so future artifacts need no legacy repair.
            provider_dependency.default_features = false;
        }
        merge_requirement_dependency(
            &mut requirements.dependencies,
            provider_dependency,
            format!("compiled provider `{}`", provider.identity.name),
        )?;
        for requirement in provider_plan.selected_backend_requirements(provider) {
            if let BackendImplementationRequirement::CargoDependency { dependency } = requirement {
                let dependency_spec = provider_cargo_dependency_spec(&dependency);
                if matches!(dependency.source, ProviderCargoDependencySource::Toolchain { .. }) {
                    merge_requirement_dependency(
                        &mut requirements.sdk_path_dependencies,
                        dependency_spec.clone(),
                        format!("compiled provider `{}` toolchain dependency", provider.identity.name),
                    )?;
                }
                merge_requirement_dependency(
                    &mut requirements.dependencies,
                    dependency_spec,
                    format!("compiled provider `{}` implementation facet", provider.identity.name),
                )?;
            }
        }
        for requirement in provider_plan.selected_backend_requirements(provider) {
            let BackendImplementationRequirement::CargoFeature { crate_name, feature } = requirement else {
                continue;
            };
            if crate_name == INCAN_STDLIB_CRATE_NAME {
                requirements.stdlib_features.push(feature);
                continue;
            }
            let Some(dependency) = requirements
                .dependencies
                .iter_mut()
                .find(|dependency| dependency.crate_name == crate_name)
            else {
                return Err(CliError::failure(format!(
                    "provider `{}` implementation facet selects Cargo feature `{crate_name}/{feature}` without declaring that dependency",
                    provider.identity.name
                )));
            };
            dependency.features.push(feature);
            dependency.features.sort();
            dependency.features.dedup();
        }
    }
    requirements
        .sdk_dependency_rebindings
        .extend_from_slice(provider_plan.sdk_dependency_rebindings());
    normalize_sdk_dependency_rebindings(&mut requirements.sdk_dependency_rebindings);
    requirements
        .sdk_artifact_projections
        .extend_from_slice(provider_plan.sdk_artifact_projections());
    normalize_sdk_artifact_projections(&mut requirements.sdk_artifact_projections);
    requirements.stdlib_features.sort();
    requirements.stdlib_features.dedup();
    Ok(())
}

/// Sort and de-duplicate physical SDK projections independently of module-group traversal order.
fn normalize_sdk_dependency_rebindings(rebindings: &mut Vec<SdkDependencyRebinding>) {
    rebindings.sort_by(|left, right| {
        (
            &left.containing_artifact.crate_root,
            &left.provider_name,
            &left.dependency_key,
            &left.source_crate_root,
            &left.active_crate_root,
        )
            .cmp(&(
                &right.containing_artifact.crate_root,
                &right.provider_name,
                &right.dependency_key,
                &right.source_crate_root,
                &right.active_crate_root,
            ))
    });
    rebindings.dedup();
}

/// Sort and de-duplicate projected artifacts by their immutable compiled crate root.
fn normalize_sdk_artifact_projections(projections: &mut Vec<SdkArtifactProjection>) {
    projections.sort_by(|left, right| left.artifact.crate_root.cmp(&right.artifact.crate_root));
    projections.dedup_by(|left, right| left.artifact.crate_root == right.artifact.crate_root);
}

/// Translate relocatable provider-owned Cargo metadata at the final Rust-backend boundary.
fn provider_cargo_dependency_spec(dependency: &ProviderCargoDependency) -> DependencySpec {
    let source = match &dependency.source {
        ProviderCargoDependencySource::Registry => DependencySource::Registry,
        ProviderCargoDependencySource::Toolchain { relative_path } => DependencySource::Path {
            path: crate::toolchain_layout::resolve_toolchain_relative_path(Path::new(relative_path)),
        },
    };
    DependencySpec {
        crate_name: dependency.crate_name.clone(),
        version: dependency.version.clone(),
        features: dependency.features.iter().cloned().collect(),
        default_features: dependency.default_features,
        source,
        optional: false,
        package: dependency.package.clone(),
    }
    .normalized()
}

/// Collect canonical provider module use from resolved source modules and authored import edges.
pub(crate) fn provider_used_module_paths(modules: &[ParsedModule]) -> BTreeSet<Vec<String>> {
    let mut used = BTreeSet::new();
    if !modules.is_empty() && env::var_os(SDK_PROVIDER_BUILD_ENV).is_none() {
        // Every ordinary compilation consumes the implicit language prelude. Recording that compiler requirement
        // keeps the mandatory core provider linked even when generated support such as iterator adapters is the only
        // emitted path into `std.derives.*`.
        used.insert(vec![stdlib::STDLIB_ROOT.to_string(), "prelude".to_string()]);
    }
    for module in modules {
        if module.path_segments.first().map(String::as_str) == Some(stdlib::INCAN_STD_NAMESPACE) {
            let mut canonical = vec![stdlib::STDLIB_ROOT.to_string()];
            canonical.extend(module.path_segments.iter().skip(1).cloned());
            used.insert(canonical);
        }
        for declaration in &module.ast.declarations {
            let crate::frontend::ast::Declaration::Import(import) = &declaration.node else {
                continue;
            };
            // Root imports name their provider module in each imported item, not in the `std` path itself.
            if let ImportKind::From { module: path, items } = &import.kind
                && path.parent_levels == 0
                && !path.is_absolute
                && path.segments.as_slice() == [stdlib::STDLIB_ROOT]
            {
                used.extend(
                    items
                        .iter()
                        .map(|item| vec![stdlib::STDLIB_ROOT.to_string(), item.name.clone()]),
                );
                continue;
            }
            let path = match &import.kind {
                ImportKind::Module(path) | ImportKind::From { module: path, .. }
                    if path.parent_levels == 0
                        && !path.is_absolute
                        && path.segments.first().map(String::as_str) == Some(stdlib::STDLIB_ROOT) =>
                {
                    Some(path.segments.clone())
                }
                _ => None,
            };
            used.extend(path);
        }
    }
    used
}

/// Resolve the reserved namespace roots granted to the SDK component currently being compiled from source.
pub(crate) fn sdk_provider_bootstrap_namespace_roots(project_root: &Path) -> CliResult<BTreeSet<String>> {
    let Some(component_marker) = env::var_os(SDK_PROVIDER_BUILD_ENV).filter(|value| !value.is_empty()) else {
        return Ok(BTreeSet::new());
    };
    let stdlib_root = crate::cli::prelude::find_stdlib_dir()
        .ok_or_else(|| CliError::failure("cannot locate the SDK source catalog while compiling an SDK provider"))?;
    let catalog = SdkSourceCatalog::read_from_path(&stdlib_root.join(SDK_SOURCE_CATALOG_FILE))
        .map_err(|error| CliError::failure(error.to_string()))?;
    let component_marker = component_marker.to_string_lossy();
    let canonical_project_root = fs::canonicalize(project_root).unwrap_or_else(|_| project_root.to_path_buf());
    let component = catalog.components.get(component_marker.as_ref()).or_else(|| {
        catalog.components.values().find(|component| {
            fs::canonicalize(&component.project_root).unwrap_or_else(|_| component.project_root.clone())
                == canonical_project_root
        })
    });
    let component = component.ok_or_else(|| {
        CliError::failure(format!(
            "SDK provider bootstrap marker `{component_marker}` does not match a component in {}",
            stdlib_root.join(SDK_SOURCE_CATALOG_FILE).display()
        ))
    })?;
    Ok(component.namespace_roots.clone())
}

/// Discover a project manifest and materialize explicit RFC 077 inheritance before compiler stages consume it.
///
/// This is the single-project compatibility boundary: projects outside a workspace retain their parsed manifest, while
/// a member receives the graph-owned effective manifest. A dangling `{ workspace = true }` request is always an error
/// instead of being silently treated as an absent local dependency.
pub(crate) fn discover_effective_project_manifest(start_dir: &Path) -> CliResult<Option<ProjectManifest>> {
    let Some(manifest) = ProjectManifest::discover(start_dir).map_err(|error| CliError::failure(error.to_string()))?
    else {
        return Ok(None);
    };
    effective_project_manifest(manifest).map(Some)
}

/// Load one exact project root and materialize RFC 077 inheritance without applying command-discovery overrides.
///
/// Recursive source-authority traversal already owns the exact dependency root it is inspecting. Rediscovering from
/// that root could honor a nested command override and silently bind a different project, so this boundary loads the
/// named manifest directly while sharing the same workspace resolution as ordinary command discovery.
pub(crate) fn effective_project_manifest_for_exact_root(project_root: &Path) -> CliResult<ProjectManifest> {
    let manifest = ProjectManifest::load(&project_root.join(LOAF_MANIFEST_FILENAME))
        .map_err(|error| CliError::failure(error.to_string()))?;
    effective_project_manifest(manifest)
}

/// Resolve one parsed manifest against its active RFC 077 workspace, if any.
fn effective_project_manifest(manifest: ProjectManifest) -> CliResult<ProjectManifest> {
    let workspace =
        WorkspaceGraph::discover(manifest.project_root()).map_err(|error| CliError::failure(error.to_string()))?;
    let Some(workspace) = workspace else {
        if manifest.has_workspace_inherited_dependencies() {
            return Err(CliError::failure(format!(
                "{} declares {{ workspace = true }} dependencies but is not a member of an active workspace",
                manifest.path().display()
            )));
        }
        return Ok(manifest);
    };
    let canonical_root = std::fs::canonicalize(manifest.project_root()).map_err(|error| {
        CliError::failure(format!(
            "failed to canonicalize project root {}: {error}",
            manifest.project_root().display()
        ))
    })?;
    let member = workspace.member_for_root(&canonical_root).ok_or_else(|| {
        CliError::failure(format!(
            "project {} is not a member of the active workspace at {}",
            manifest.path().display(),
            workspace.root().display()
        ))
    })?;
    workspace
        .effective_member_manifest(member)
        .map_err(|error| CliError::failure(error.to_string()))
}

/// Checked products produced by one [`CompilationSession`] analysis pass.
///
/// The current Rust-source backend still lowers from [`TypeCheckInfo`], while compiler-facing consumers use the
/// portable snapshot. Keeping both products in one analysis result prevents a CLI command from independently checking
/// the same sources and then treating its second result as authoritative.
///
/// The `TypeCheckInfo` half is a transition bridge. Remove it when Body IR owns every lowering query tracked by #225.
#[derive(Debug, Clone)]
pub(crate) struct CompilationAnalysis {
    type_info_by_path: BTreeMap<PathBuf, TypeCheckInfo>,
    type_info_by_module_path: BTreeMap<Vec<String>, TypeCheckInfo>,
    semantic_snapshots_by_path: BTreeMap<PathBuf, incan_semantics_core::SemanticModuleSnapshot>,
    stdlib_cache: StdlibAstCache,
}

/// One module's checked lowering bridge and portable semantic snapshot from one [`CompilationAnalysis`].
///
/// This bundle prevents a consumer from treating an independently derived `TypeCheckInfo` as the authority for a
/// semantic snapshot. `TypeCheckInfo` remains only because Body IR has not yet moved all of its lowering queries to
/// portable semantic facts (#225).
pub(crate) struct CompilationModuleAnalysis<'analysis> {
    type_info: &'analysis TypeCheckInfo,
    semantic_snapshot: &'analysis incan_semantics_core::SemanticModuleSnapshot,
}

impl CompilationModuleAnalysis<'_> {
    /// Return the session-owned transition bridge Body IR currently requires for lowering.
    pub(crate) fn type_info(&self) -> &TypeCheckInfo {
        self.type_info
    }

    /// Return the portable semantic module produced beside this lowering bridge.
    pub(crate) fn semantic_snapshot(&self) -> &incan_semantics_core::SemanticModuleSnapshot {
        self.semantic_snapshot
    }
}

impl CompilationAnalysis {
    /// Return the paired checked products for one collected source file.
    ///
    /// A caller must consume this bundle when it needs both Body-IR lowering and semantic provenance. Looking up the
    /// halves separately would make it too easy to join facts from different analysis products at the CLI boundary.
    pub(crate) fn module_analysis_for_path(&self, path: &Path) -> Option<CompilationModuleAnalysis<'_>> {
        Some(CompilationModuleAnalysis {
            type_info: self.type_info_by_path.get(path)?,
            semantic_snapshot: self.semantic_snapshots_by_path.get(path)?,
        })
    }

    /// Return the lowering input for one collected source file.
    pub(crate) fn type_info_for_path(&self, path: &Path) -> Option<&TypeCheckInfo> {
        self.type_info_by_path.get(path)
    }

    /// Return the lowering input for one compiler module identity.
    ///
    /// Identity is distinct from a source file path because the test runner may create multiple compiler modules rooted
    /// at the same file.
    pub(crate) fn type_info_for_module_path(&self, path: &[String]) -> Option<&TypeCheckInfo> {
        self.type_info_by_module_path.get(path)
    }

    /// Return portable HIR and semantic fact snapshots keyed by source path.
    pub(crate) fn semantic_snapshots(&self) -> &BTreeMap<PathBuf, incan_semantics_core::SemanticModuleSnapshot> {
        &self.semantic_snapshots_by_path
    }

    /// Return the source-backed stdlib metadata accumulated by this analysis.
    ///
    /// Lowering currently queries this cache for source-defined trait and type metadata. It stays part of the session
    /// result until those queries move to portable semantic facts and Body IR (#225).
    pub(crate) fn stdlib_cache(&self) -> &StdlibAstCache {
        &self.stdlib_cache
    }
}

/// Index import-activated vocabulary packaged by enabled standard-library providers.
///
/// Standard providers remain owned by the SDK plan for module resolution and generated-Rust dependencies. This index
/// contributes only companion metadata and WASM desugarers to the generic vocabulary pipeline.
fn add_selected_standard_vocab_providers(
    library_manifest_index: &mut LibraryManifestIndex,
    sdk_inventory: Option<&SdkInventory>,
    sdk_components: Option<&ResolvedSdkComponents>,
) -> CliResult<()> {
    let (Some(sdk_inventory), Some(sdk_components)) = (sdk_inventory, sdk_components) else {
        return Ok(());
    };

    for component_id in &sdk_components.enabled {
        let component = sdk_inventory.components.get(component_id).ok_or_else(|| {
            CliError::failure(format!(
                "selected SDK component `{component_id}` is absent from {}",
                sdk_components.sdk_identity
            ))
        })?;
        for provider in &component.providers {
            let (Some(manifest_path), Some(crate_root)) = (&provider.manifest_path, &provider.crate_root) else {
                continue;
            };
            library_manifest_index
                .add_standard_vocab_provider(manifest_path, crate_root)
                .map_err(|error| {
                    CliError::failure(format!(
                        "failed to index standard vocabulary from SDK component `{component_id}` provider `{}`: {error}",
                        provider.name
                    ))
                })?;
        }
    }
    Ok(())
}

/// Shared source-analysis context for CLI commands and the LSP.
///
/// This owns the project-level inputs that affect context-sensitive parsing and typechecking so entrypoints do not
/// independently rediscover manifests, library vocabulary, provider surfaces, or checked contract metadata.
#[derive(Debug, Clone)]
pub(crate) struct CompilationSession {
    pub manifest: Option<ProjectManifest>,
    pub source_root: PathBuf,
    pub library_manifest_index: LibraryManifestIndex,
    /// Immutable provider projection shared by compiler stages for ordinary package dependencies and SDK providers.
    pub provider_plan: Arc<ProviderPlan>,
    /// Module-usage projections already derived from the immutable session inputs.
    ///
    /// Collection, requirement discovery, and semantic analysis all need the same projection. Rebuilding it makes
    /// every command rehash provider source roots and, worse, lets a mutable local cache dominate the warm path.
    provider_plans_by_modules: ProviderPlanCache,
    /// Integrity-checked active SDK catalog, when this toolchain is component-aware.
    pub sdk_inventory: Option<Arc<SdkInventory>>,
    /// Project-selected SDK component closure, when an inventory is active.
    pub sdk_components: Option<ResolvedSdkComponents>,
    /// Typed additive feature closure for the root package and active path dependencies.
    pub package_feature_plan: Option<PackageFeaturePlan>,
    /// Active root-package features used to project compilation-unit `when feature(...)` declarations.
    pub active_features: BTreeSet<String>,
    /// Declared root-package features used for source diagnostics before projection.
    pub declared_features: BTreeSet<String>,
    pub library_imported_vocab: parser::ImportedLibraryVocab,
    pub library_imported_dsl_surfaces: parser::ImportedLibraryDslSurfaces,
    pub contract_model_bundles: Vec<CanonicalModelBundle>,
}

impl CompilationSession {
    /// Discover project-level compilation context for an explicit Incan package-feature selection.
    pub(crate) fn discover_with_feature_selection(
        entry_path: &Path,
        feature_selection: &FeatureSelection,
    ) -> CliResult<Self> {
        Self::discover_with_selections(entry_path, feature_selection, None)
    }

    /// Discover project context for explicit package-feature and transient SDK-profile selections.
    pub(crate) fn discover_with_selections(
        entry_path: &Path,
        feature_selection: &FeatureSelection,
        sdk_profile_override: Option<&str>,
    ) -> CliResult<Self> {
        Self::discover_with_dependency_mode(
            entry_path,
            DependencyManifestMode::OvenArtifacts,
            feature_selection,
            sdk_profile_override,
        )
    }

    /// Discover project-level parsing context without preparing full dependency artifacts.
    pub(crate) fn discover_for_collection(entry_path: &Path) -> CliResult<Self> {
        Self::discover_for_collection_with_feature_selection(entry_path, &FeatureSelection::default())
    }

    /// Discover parser-only project context for an explicit Incan package-feature selection.
    pub(crate) fn discover_for_collection_with_feature_selection(
        entry_path: &Path,
        feature_selection: &FeatureSelection,
    ) -> CliResult<Self> {
        Self::discover_for_collection_with_selections(entry_path, feature_selection, None)
    }

    /// Discover parser-only project context for explicit package-feature and transient SDK-profile selections.
    ///
    /// Test collection is part of the normal Oven execution path, so a missing SDK inventory is an explicit
    /// preparation error rather than authorization to rebuild it through the former Cargo backend.
    pub(crate) fn discover_for_collection_with_selections(
        entry_path: &Path,
        feature_selection: &FeatureSelection,
        sdk_profile_override: Option<&str>,
    ) -> CliResult<Self> {
        Self::discover_with_dependency_mode(
            entry_path,
            DependencyManifestMode::ParserOnly,
            feature_selection,
            sdk_profile_override,
        )
    }

    /// Discover the semantic context for an Oven consumer without triggering any legacy Cargo publication.
    ///
    /// This permits an already installed or Oven-prepared SDK inventory, but a normal command must never respond to a
    /// cache miss by launching the old provider builder. The explicit `legacy_cargo` publisher owns that transition.
    pub(crate) fn discover_for_oven(
        entry_path: &Path,
        feature_selection: &FeatureSelection,
        sdk_profile_override: Option<&str>,
    ) -> CliResult<Self> {
        Self::discover_with_dependency_mode(
            entry_path,
            DependencyManifestMode::OvenArtifacts,
            feature_selection,
            sdk_profile_override,
        )
    }

    /// Discover project context with either admitted Oven artifacts or parser-only dependency metadata.
    fn discover_with_dependency_mode(
        entry_path: &Path,
        dependency_mode: DependencyManifestMode,
        feature_selection: &FeatureSelection,
        sdk_profile_override: Option<&str>,
    ) -> CliResult<Self> {
        let inferred_project_root = resolve_project_root(entry_path);
        let manifest = discover_effective_project_manifest(&inferred_project_root)?;
        let project_root = manifest
            .as_ref()
            .map(|manifest| manifest.project_root().to_path_buf())
            .unwrap_or(inferred_project_root);
        let source_root = resolve_source_root(&project_root, manifest.as_ref());
        let sdk_inventory = discover_active_sdk_inventory()?;
        let package_feature_plan = manifest
            .as_ref()
            .map(|manifest| {
                PackageFeaturePlan::resolve_with_sdk_inventory(manifest, feature_selection, sdk_inventory.as_deref())
            })
            .transpose()
            .map_err(|error| CliError::failure(error.to_string()))?;
        let root_feature_state = package_feature_plan
            .as_ref()
            .and_then(|plan| plan.package(&project_root));
        let active_dependencies = root_feature_state
            .map(|state| state.active_dependencies.clone())
            .unwrap_or_default();
        let active_features = root_feature_state
            .map(|state| state.features.active_features.clone())
            .unwrap_or_default();
        let declared_features = manifest
            .as_ref()
            .map(PackageFeatureGraph::from_manifest)
            .transpose()
            .map_err(|error| CliError::failure(error.to_string()))?
            .map(|graph| graph.declared_features().map(str::to_string).collect())
            .unwrap_or_default();
        if let Some(manifest) = manifest.as_ref()
            && dependency_mode.uses_materialized_library_index()
        {
            require_library_dependency_artifacts(manifest, package_feature_plan.as_ref(), &active_dependencies)?;
        }
        let mut library_manifest_index = match (manifest.as_ref(), dependency_mode) {
            (Some(manifest), mode) if mode.uses_materialized_library_index() && !active_dependencies.is_empty() => {
                LibraryManifestIndex::from_project_manifest_dependencies(
                    manifest,
                    active_dependencies.iter().map(String::as_str),
                )
            }
            (Some(manifest), DependencyManifestMode::ParserOnly) if !active_dependencies.is_empty() => {
                parser_only_library_manifest_index(manifest, &active_dependencies)?
            }
            _ => LibraryManifestIndex::default(),
        };
        let contract_model_bundles = manifest
            .as_ref()
            .map(|manifest| read_project_model_bundles(&project_root, &manifest.contract_model_bundle_paths()))
            .transpose()
            .map_err(|error| CliError::failure(error.to_string()))?
            .unwrap_or_default();
        // Collection and execution must resolve the same SDK catalog. In a source checkout there is no installed
        // inventory to discover, so a parser-only collection that skipped publication silently fell back to the
        // legacy monolithic stdlib. Besides losing component-aware diagnostics, that made a transient SDK profile
        // fail during collection and allowed the lock projection to drift before execution prepared the artifacts.
        // Publication is content-addressed and reused; parser-only mode still avoids preparing ordinary dependencies.
        validate_component_inventory_selection(manifest.as_ref(), sdk_profile_override, sdk_inventory.as_deref())?;
        let sdk_selection =
            SdkComponentSelection::from_manifest_with_profile_override(manifest.as_ref(), sdk_profile_override);
        let sdk_components = sdk_inventory
            .as_ref()
            .map(|inventory| {
                resolve_sdk_component_selection(
                    inventory,
                    &sdk_selection,
                    manifest.as_ref(),
                    sdk_profile_override,
                    false,
                )
            })
            .transpose()?;
        add_selected_standard_vocab_providers(
            &mut library_manifest_index,
            sdk_inventory.as_deref(),
            sdk_components.as_ref(),
        )?;
        let library_imported_vocab = library_manifest_index.library_imported_vocab();
        let library_imported_dsl_surfaces = library_manifest_index.library_imported_dsl_surfaces();
        let bootstrap_sdk_namespace_roots = sdk_provider_bootstrap_namespace_roots(&project_root)?;
        let provider_plan = Arc::new(
            ProviderPlan::from_resolved_inputs(
                library_manifest_index.clone(),
                package_feature_plan.as_ref(),
                sdk_inventory.as_deref(),
                sdk_components.as_ref(),
                std::iter::empty(),
            )
            .map_err(|error| CliError::failure(error.to_string()))?
            .with_bootstrap_sdk_namespace_roots(bootstrap_sdk_namespace_roots),
        );
        let provider_plans_by_modules = Arc::new(Mutex::new(BTreeMap::from([(
            BTreeSet::new(),
            Arc::clone(&provider_plan),
        )])));

        Ok(Self {
            manifest,
            source_root,
            library_manifest_index,
            provider_plan,
            provider_plans_by_modules,
            sdk_inventory,
            sdk_components,
            package_feature_plan,
            active_features,
            declared_features,
            library_imported_vocab,
            library_imported_dsl_surfaces,
            contract_model_bundles,
        })
    }

    /// Resolve module participation from this session's immutable provider, feature, and SDK inputs.
    pub(crate) fn provider_plan_for_modules(&self, modules: &[ParsedModule]) -> CliResult<Arc<ProviderPlan>> {
        self.provider_plan_for_used_module_paths(provider_used_module_paths(modules))
    }

    /// Resolve one provider projection from canonical module paths while retaining the session's authority snapshot.
    ///
    /// Callers with nested source constructs can derive their complete module-use set once, then enter the same cache
    /// as ordinary compilation without rebuilding SDK, package-feature, or library-manifest inputs.
    pub(crate) fn provider_plan_for_used_module_paths(
        &self,
        used_module_paths: BTreeSet<Vec<String>>,
    ) -> CliResult<Arc<ProviderPlan>> {
        if let Some(plan) = self
            .provider_plans_by_modules
            .lock()
            .map_err(|_| CliError::failure("provider-plan cache lock was poisoned"))?
            .get(&used_module_paths)
            .cloned()
        {
            return Ok(plan);
        }
        let plan = ProviderPlan::from_resolved_inputs(
            self.provider_plan.library_manifest_index().clone(),
            self.package_feature_plan.as_ref(),
            self.sdk_inventory.as_deref(),
            self.sdk_components.as_ref(),
            used_module_paths.clone(),
        )
        .map(|plan| {
            plan.with_bootstrap_sdk_namespace_roots(self.provider_plan.bootstrap_sdk_namespace_roots().cloned())
        })
        .map(Arc::new)
        .map_err(|error| CliError::failure(error.to_string()))?;
        let mut cached = self
            .provider_plans_by_modules
            .lock()
            .map_err(|_| CliError::failure("provider-plan cache lock was poisoned"))?;
        Ok(cached.entry(used_module_paths).or_insert(plan).clone())
    }

    /// Return the number of provider projections cached by this compilation session.
    #[cfg(test)]
    pub(crate) fn provider_plan_cache_entry_count(&self) -> CliResult<usize> {
        self.provider_plans_by_modules
            .lock()
            .map(|cached| cached.len())
            .map_err(|_| CliError::failure("provider-plan cache lock was poisoned"))
    }

    /// Analyze one collected module graph exactly once.
    ///
    /// This is the v0.5 session bridge: code generation receives the checked lowering inputs it currently needs, and
    /// codegraph/LSP-facing callers receive compiler-owned semantic facts from the same pass. No command may re-run
    /// typechecking merely to derive its own authority.
    pub(crate) fn analyze_modules(
        &self,
        modules: &[ParsedModule],
        #[cfg(feature = "rust_inspect")] rust_inspect_manifest_dir: Option<&Path>,
    ) -> Result<CompilationAnalysis, CliDiagnosticFailure> {
        #[cfg(test)]
        record_compilation_session_analysis_invocation();
        let provider_plan = self.provider_plan_for_modules(modules).map_err(|error| {
            let module = modules.last();
            CliDiagnosticFailure::single(
                module
                    .map(|module| module.file_path.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                module.map(|module| module.source.clone()).unwrap_or_default(),
                diagnostics::CompileError::new(error.message, Span::default()),
                diagnostics::DiagnosticPhase::Import,
            )
        })?;
        let typecheck_artifacts = typecheck_modules_with_import_graph_artifacts(
            modules,
            self.manifest.as_ref(),
            &provider_plan,
            None,
            #[cfg(feature = "rust_inspect")]
            rust_inspect_manifest_dir,
        )?;
        let mut type_info_by_path = BTreeMap::new();
        let mut type_info_by_module_path = BTreeMap::new();
        let mut semantic_snapshots_by_path = BTreeMap::new();

        for (module, type_info) in modules.iter().zip(typecheck_artifacts.type_infos) {
            let snapshot = build_semantic_module_snapshot_v0(&module.ast, &module.path_segments, &type_info);
            type_info_by_path.insert(module.file_path.clone(), type_info.clone());
            type_info_by_module_path.insert(module.path_segments.clone(), type_info);
            semantic_snapshots_by_path.insert(module.file_path.clone(), snapshot);
        }

        Ok(CompilationAnalysis {
            type_info_by_path,
            type_info_by_module_path,
            semantic_snapshots_by_path,
            stdlib_cache: typecheck_artifacts.stdlib_cache,
        })
    }

    /// Resolve the test runner's marker contract from the same checked provider plan used by compilation.
    ///
    /// A source fallback remains only for legacy/development sessions without an SDK catalog. Component-aware SDKs
    /// must supply this contract through the compiled `std.testing` provider manifest.
    pub(crate) fn testing_marker_semantics(&self) -> CliResult<Option<TestingMarkerSemantics>> {
        let module = [stdlib::STDLIB_ROOT.to_string(), "testing".to_string()];
        match self.provider_plan.resolve_module(&module) {
            ProviderModuleResolution::Active(provider) => {
                let manifest = provider.manifest.as_deref().ok_or_else(|| {
                    CliError::failure(format!(
                        "active std.testing provider `{}` has no checked manifest",
                        provider.identity.name
                    ))
                })?;
                testing_marker_semantics_from_manifest(manifest).map_err(|error| CliError::failure(error.to_string()))
            }
            ProviderModuleResolution::Disabled(_) | ProviderModuleResolution::Unavailable(_) => Ok(None),
            ProviderModuleResolution::Unknown if self.provider_plan.has_sdk_catalog() => Ok(None),
            ProviderModuleResolution::Unknown => load_testing_marker_semantics()
                .map(Some)
                .map_err(|error| CliError::failure(error.to_string())),
        }
    }

    /// Require `std.testing` marker semantics and retain the component-specific remedy in the failure.
    pub(crate) fn require_testing_marker_semantics(&self) -> CliResult<TestingMarkerSemantics> {
        if let Some(semantics) = self.testing_marker_semantics()? {
            return Ok(semantics);
        }

        let module = [stdlib::STDLIB_ROOT.to_string(), "testing".to_string()];
        let message = match self.provider_plan.resolve_module(&module) {
            ProviderModuleResolution::Disabled(provider) => match &provider.provenance {
                ProviderProvenance::Sdk { component_id, .. } => format!(
                    "std.testing marker syntax requires disabled SDK component `{component_id}`; enable it under [sdk] components"
                ),
                _ => "std.testing marker syntax requires a disabled provider".to_string(),
            },
            ProviderModuleResolution::Unavailable(provider) => match &provider.provenance {
                ProviderProvenance::Sdk { component_id, .. } => format!(
                    "std.testing marker syntax requires SDK component `{component_id}`, but its artifact is not installed"
                ),
                _ => "std.testing marker syntax requires an unavailable provider artifact".to_string(),
            },
            _ => "std.testing marker syntax requires the compiled std.testing provider".to_string(),
        };
        Err(CliError::failure(message))
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
    ///
    /// Always parses with the original `source` text available (RFC 081, `#1023`), so a descriptor-gated embedded
    /// fragment (`crates/incan_syntax/src/parser/embedded/`) can claim eligible positions in every real
    /// compilation, not only in the parser's own unit tests. The ordinary strict lexer (`lexer::lex`) still runs
    /// first, unchanged, for the overwhelming majority of files that tokenize cleanly. It only fails outright for
    /// source containing bytes that are not valid ordinary-Incan token starts at all (`;`, `` ` ``, `$`, and
    /// similar) -- exactly the kind of content realistic embedded-fragment submodes (style rules, template
    /// literals, ...) routinely contain. Only in that fallback case does this retry with the tolerant lexer
    /// (`lexer::lex_tolerant`), which never discards its token stream on error, and hand its collected lex errors
    /// to the parser for reconciliation (`parser::parse_with_source_and_lex_errors`) rather than dropping or
    /// unconditionally surfacing them: an error inside a fragment this parse actually claims is expected noise
    /// from the ordinary lexer's honest confusion about foreign submode syntax, while every other error is a real
    /// mistake and still reaches the caller.
    pub(crate) fn parse_source_for_collection(
        &self,
        file_path: &Path,
        source: &str,
    ) -> Result<Program, Vec<diagnostics::CompileError>> {
        let file_path_display = file_path.to_string_lossy();
        match lexer::lex(source) {
            Ok(tokens) => parser::parse_with_source(
                &tokens,
                Some(file_path_display.as_ref()),
                Some(&self.library_imported_vocab),
                Some(&self.library_imported_dsl_surfaces),
                source,
            ),
            Err(_) => {
                let (tokens, lex_errors) = lexer::lex_tolerant(source);
                parser::parse_with_source_and_lex_errors(
                    &tokens,
                    Some(file_path_display.as_ref()),
                    Some(&self.library_imported_vocab),
                    Some(&self.library_imported_dsl_surfaces),
                    source,
                    lex_errors,
                )
            }
        }
    }

    /// Lex, parse, vocab-desugar, and optionally materialize checked contract models for one source file.
    pub(crate) fn parse_source(
        &self,
        file_path: &Path,
        source: &str,
        materialize_models: bool,
    ) -> Result<Program, Vec<diagnostics::CompileError>> {
        let parsed = self.parse_source_unprojected(file_path, source, materialize_models)?;
        self.project_parsed_program(parsed)
    }

    /// Parse and desugar one source file while retaining inactive compile-time feature declarations for tooling.
    pub(crate) fn parse_source_unprojected(
        &self,
        file_path: &Path,
        source: &str,
        materialize_models: bool,
    ) -> Result<Program, Vec<diagnostics::CompileError>> {
        let parsed = self.parse_source_for_collection(file_path, source)?;
        let mut ast = parsed;
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

    /// Validate and project one already-parsed source program through this session's active package features.
    pub(crate) fn project_parsed_program(&self, parsed: Program) -> Result<Program, Vec<diagnostics::CompileError>> {
        self.validate_parsed_program_features(&parsed)?;
        Ok(parsed.projected_for_features(&self.active_features))
    }

    /// Validate compile-time feature names while retaining the complete unprojected source program.
    pub(crate) fn validate_parsed_program_features(
        &self,
        parsed: &Program,
    ) -> Result<(), Vec<diagnostics::CompileError>> {
        let mut feature_errors = Vec::new();
        for declaration in &parsed.declarations {
            self.validate_declaration_feature_requirements(declaration, &mut feature_errors);
        }
        if !feature_errors.is_empty() {
            return Err(feature_errors);
        }
        Ok(())
    }

    /// Validate one declaration and any inline-test declarations nested inside it against the package feature graph.
    fn validate_declaration_feature_requirements(
        &self,
        declaration: &crate::frontend::ast::Spanned<crate::frontend::ast::Declaration>,
        errors: &mut Vec<diagnostics::CompileError>,
    ) {
        for feature in &declaration.required_features {
            if !self.declared_features.contains(feature) {
                errors.push(diagnostics::CompileError::new(
                    format!("Unknown package feature `{feature}` in compile-time condition"),
                    declaration.span,
                ));
            }
        }
        if let crate::frontend::ast::Declaration::TestModule(module) = &declaration.node {
            for nested in &module.body {
                self.validate_declaration_feature_requirements(nested, errors);
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DependencyManifestMode {
    /// Materialize direct-Rustc caller-owned libraries for a normal Oven consumer.
    OvenArtifacts,
    ParserOnly,
}

impl DependencyManifestMode {
    /// Return whether this mode needs the checked library index after preparation.
    fn uses_materialized_library_index(self) -> bool {
        matches!(self, Self::OvenArtifacts)
    }
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
pub(crate) fn parser_only_library_manifest_index(
    manifest: &ProjectManifest,
    active_dependencies: &BTreeSet<String>,
) -> CliResult<LibraryManifestIndex> {
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
) -> CliResult<LibraryManifestIndexEntry> {
    let dependency_root = fs::canonicalize(dependency_root).unwrap_or_else(|_| dependency_root.to_path_buf());
    let manifest_path = dependency_root.join(LOAF_MANIFEST_FILENAME);
    let manifest_content = fs::read_to_string(&manifest_path)
        .map_err(|error| CliError::failure(format!("failed to read {}: {error}", manifest_path.display())))?;
    let dependency_manifest = ProjectManifest::from_str(&manifest_content, &manifest_path)
        .map_err(|error| CliError::failure(error.to_string()))?;
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

    if let Some(vocab_extraction) = collect_library_vocab_metadata_for_parser(&dependency_manifest, &project_root)? {
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

/// Require every selected local library dependency to have an admitted Oven artifact.
///
/// Session discovery is read-only. A missing or stale dependency artifact is a producer obligation and never
/// authorizes the compiler to spawn itself, synthesize a Cargo project, or mutate the dependency checkout.
fn require_library_dependency_artifacts(
    manifest: &ProjectManifest,
    feature_plan: Option<&PackageFeaturePlan>,
    active_dependencies: &BTreeSet<String>,
) -> CliResult<()> {
    if active_dependencies.is_empty() {
        return Ok(());
    }

    let initial_index = LibraryManifestIndex::from_project_manifest_dependencies(
        manifest,
        active_dependencies.iter().map(String::as_str),
    );
    for dependency_key in active_dependencies {
        let Some(dependency) = manifest.library_dependencies().get(dependency_key) else {
            continue;
        };
        let expected_features = feature_plan
            .and_then(|plan| plan.package(&dependency.path))
            .map(|package| package.features.active_features.clone())
            .unwrap_or_default();
        let has_source_manifest = dependency.path.join(LOAF_MANIFEST_FILENAME).is_file();
        let unavailable = match initial_index.get(dependency_key) {
            Some(LibraryManifestIndexEntry::Loaded {
                manifest: artifact_manifest,
                metadata,
            }) => {
                let actual_features = &artifact_manifest.contract_metadata.provider.active_features;
                if actual_features != &expected_features && !has_source_manifest {
                    return Err(CliError::failure(format!(
                        "compiled dependency `pub::{dependency_key}` at {} was built with package features [{}], but this consumer requires [{}]; the producer source manifest is unavailable, so install or publish an artifact with the exact requested feature projection",
                        metadata.manifest_path.display(),
                        actual_features.iter().cloned().collect::<Vec<_>>().join(", "),
                        expected_features.iter().cloned().collect::<Vec<_>>().join(", "),
                    )));
                }
                actual_features != &expected_features
                    || has_source_manifest
                        && !oven_library_dependency_has_verified_profile_receipts(&dependency.path)
                        && !super::build::oven_library_dependency_declares_package_loaf(&dependency.path)
            }
            Some(LibraryManifestIndexEntry::Failed(failure)) => {
                failure.kind == LibraryManifestFailureKind::ArtifactMissing
            }
            None => false,
        };
        if unavailable && has_source_manifest {
            return Err(CliError::failure(format!(
                "Oven requires a baked package Loaf for pub::{dependency_key} at {}; run `incan oven bake --project {}` in that provider before preparing this consumer. Session discovery will not compile or mutate a dependency on the consumer's behalf",
                dependency.path.display(),
                dependency.path.display()
            )));
        }
    }

    Ok(())
}

/// Return whether a materialized local `pub::` library is authorized for both normal Oven profiles.
///
/// A legacy generated artifact is not enough: consumers re-materialize the provider source through their selected
/// direct-Rustc cohort, which requires an identity-verified producer receipt for the debug and release profiles.
/// A local source manifest identifies the producer project but does not authorize a consumer to rebuild it.
fn oven_library_dependency_has_verified_profile_receipts(dependency_root: &Path) -> bool {
    let release = crate::oven::default_receipt_path(dependency_root);
    let debug = release.with_file_name("library-debug-receipt.json");
    [release, debug].iter().all(|path| {
        fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<crate::oven::OvenReceipt>(&bytes).ok())
            .is_some_and(|receipt| receipt.verify_identity().is_ok())
    })
}

/// Collect a unified set of project requirements from source imports and loaded provider manifests.
pub(crate) fn collect_project_requirements(
    modules: &[ParsedModule],
    library_manifest_index: &LibraryManifestIndex,
) -> CliResult<ProjectRequirements> {
    let mut stdlib_namespaces = HashSet::new();
    if env::var_os(SDK_PROVIDER_BUILD_ENV).is_some() {
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

/// Return the exact compiler-owned path catalog used only for semantic generated-artifact identity.
pub(crate) fn semantic_sdk_path_dependencies(requirements: &ProjectRequirements) -> Vec<DependencySpec> {
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
            path: crate::toolchain_layout::resolve_toolchain_crate_path(crate_name),
        },
        optional: false,
        package: None,
    }
}

/// Build a dependency specification from a stdlib extra crate requirement.
fn dependency_spec_from_stdlib_extra_crate(crate_name: &str) -> CliResult<DependencySpec> {
    let dep = stdlib::find_extra_crate_dep(crate_name).ok_or_else(|| {
        CliError::failure(format!(
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
                path: crate::toolchain_layout::resolve_toolchain_relative_path(Path::new(relative_path)),
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
        if !dependency_specs_match(existing, &candidate) {
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

/// Compare dependency specs while treating equivalent path spellings as the same dependency.
pub(crate) fn dependency_specs_match(left: &DependencySpec, right: &DependencySpec) -> bool {
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
            if !dependency_specs_match(existing, required) {
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

/// Collect canonical rust-inspect query paths from parsed `rust::` imports.
#[cfg(feature = "rust_inspect")]
pub(crate) fn collect_rust_inspect_query_paths(modules: &[ParsedModule]) -> Vec<String> {
    collect_rust_inspect_query_paths_from_programs(modules.iter().map(|module| &module.ast))
}

/// Collect canonical rust-inspect query paths from parsed programs.
///
/// Loaf publication also parses compiler-owned provider source that is metadata-only in the consumer module
/// graph. Keeping the import walk program-based lets that explicit publisher preserve Rust ownership signatures
/// without making normal Oven consumers re-emit provider source.
#[cfg(feature = "rust_inspect")]
pub(crate) fn collect_rust_inspect_query_paths_from_programs<'a>(
    programs: impl IntoIterator<Item = &'a Program>,
) -> Vec<String> {
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
    for program in programs {
        for decl in &program.declarations {
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

/// Collect exact Rust derive paths used by concrete Incan type declarations.
///
/// The explicit Cargo-backed inspection bootstrap emits one synthetic Rust type per path. Expanding that type records
/// a candidate trait and associated-type contract for ABI selection; native rustc remains authoritative for the real
/// Incan-authored declaration. This collector admits direct `rust::` item imports used by `@derive(...)` or
/// `@rust.derive(...)` plus syntactically valid explicit Rust macro paths. The preparation boundary later limits those
/// paths to declared dependency namespaces before generating the probe workspace.
#[cfg(feature = "rust_inspect")]
pub(crate) fn collect_rust_inspect_derive_probe_paths(modules: &[ParsedModule]) -> Vec<String> {
    use crate::frontend::ast::{Declaration, Decorator, DecoratorArg, Expr, Literal};

    /// Return decorators only for declarations that can emit a concrete Rust type.
    fn declaration_decorators(declaration: &Declaration) -> Option<&[crate::frontend::ast::Spanned<Decorator>]> {
        match declaration {
            Declaration::Model(item) => Some(&item.decorators),
            Declaration::Class(item) => Some(&item.decorators),
            Declaration::Newtype(item) => Some(&item.decorators),
            Declaration::Enum(item) => Some(&item.decorators),
            Declaration::Import(_)
            | Declaration::Const(_)
            | Declaration::Static(_)
            | Declaration::Trait(_)
            | Declaration::Alias(_)
            | Declaration::Partial(_)
            | Declaration::TypeAlias(_)
            | Declaration::Function(_)
            | Declaration::TestModule(_)
            | Declaration::VocabBlock(_)
            | Declaration::Capability(_)
            | Declaration::Docstring(_) => None,
        }
    }

    /// Validate a canonical Rust item path, including raw-identifier segments, before generating a derive probe.
    fn is_valid_rust_path(path: &str) -> bool {
        path.split("::").all(|segment| {
            let segment = segment.strip_prefix("r#").unwrap_or(segment);
            let mut chars = segment.chars();
            chars
                .next()
                .is_some_and(|first| first == '_' || first.is_ascii_alphabetic())
                && chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
        })
    }

    let mut probes = BTreeSet::new();
    for module in modules {
        let mut imported_paths = HashMap::new();
        for declaration in &module.ast.declarations {
            let Declaration::Import(import) = &declaration.node else {
                continue;
            };
            let ImportKind::RustFrom {
                crate_name,
                path,
                items,
                ..
            } = &import.kind
            else {
                continue;
            };
            let mut base = Vec::with_capacity(path.len() + 1);
            base.push(crate_name.replace('-', "_"));
            base.extend(path.iter().cloned());
            for item in items {
                let binding = item.alias.as_ref().unwrap_or(&item.name);
                let mut segments = base.clone();
                segments.push(item.name.clone());
                imported_paths.insert(binding.clone(), segments.join("::"));
            }
        }
        for declaration in &module.ast.declarations {
            let Some(decorators) = declaration_decorators(&declaration.node) else {
                continue;
            };
            for decorator in decorators {
                let Some(decorator_id) = incan_core::lang::decorators::from_segments(&decorator.node.path.segments)
                else {
                    continue;
                };
                for argument in &decorator.node.args {
                    let DecoratorArg::Positional(argument) = argument else {
                        continue;
                    };
                    match (decorator_id, &argument.node) {
                        (incan_core::lang::decorators::DecoratorId::Derive, Expr::Ident(name))
                        | (incan_core::lang::decorators::DecoratorId::RustDerive, Expr::Ident(name)) => {
                            if let Some(path) = imported_paths.get(name) {
                                probes.insert(path.clone());
                            }
                        }
                        (
                            incan_core::lang::decorators::DecoratorId::RustDerive,
                            Expr::Literal(Literal::String(path)),
                        ) if path.contains("::") && is_valid_rust_path(path) => {
                            probes.insert(path.clone());
                        }
                        _ => {}
                    }
                }
            }
        }
    }
    probes.into_iter().collect()
}

/// Marker understood by `rust_inspect` that selects its build-system-neutral `rust-project.json` loader.
///
/// Mark one compiler-authored Rust inspection projection for receipt-bound direct-Rustc loading.
#[cfg(feature = "rust_inspect")]
pub(crate) fn mark_oven_direct_rust_inspection(manifest_dir: &Path) -> CliResult<()> {
    let marker = manifest_dir.join(crate::rust_inspect::OVEN_DIRECT_INSPECTION_MARKER);
    fs::write(&marker, b"receipt-bound direct-rustc inspection\n").map_err(|error| {
        CliError::failure(format!(
            "failed to mark Oven Rust inspection projection {}: {error}",
            marker.display()
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
    read_source_checked(file_path).map_err(|failure| CliError::failure(failure.message))
}

/// Read source for the stable diagnostic path, converting file-system failures into tooling diagnostics.
fn read_source_for_diagnostics(file_path: &str) -> Result<String, CliDiagnosticFailure> {
    read_source_checked(file_path).map_err(|failure| {
        CliDiagnosticFailure::single(
            file_path,
            "",
            diagnostics::CompileError::new(failure.message, Span::default()),
            diagnostics::DiagnosticPhase::Tooling,
        )
    })
}

/// Read source once behind both legacy text errors and structured diagnostic reporting.
fn read_source_checked(file_path: &str) -> Result<String, SourceReadFailure> {
    let metadata = fs::metadata(file_path).map_err(|error| SourceReadFailure {
        message: format!("Cannot access file '{}': {}", file_path, error),
    })?;
    if metadata.len() > MAX_SOURCE_SIZE {
        return Err(SourceReadFailure {
            message: format!(
                "Source file '{}' is too large ({} bytes, max {} bytes)",
                file_path,
                metadata.len(),
                MAX_SOURCE_SIZE
            ),
        });
    }
    fs::read_to_string(file_path).map_err(|error| SourceReadFailure {
        message: format!("Error reading file '{}': {}", file_path, error),
    })
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
/// Unmigrated source-backed stdlib trait modules and builtin fallback traits are still discovered explicitly when the
/// parsed AST needs them. Migrated modules are resolved through the compiled built-in artifact instead.
pub fn collect_modules(entry_path: &str) -> CliResult<Vec<ParsedModule>> {
    collect_modules_detailed(entry_path).map_err(|failure| CliError::failure(failure.render_human()))
}

/// Return whether the SDK catalog claims a module that source collection must never materialize locally.
///
/// Disabled and unavailable providers still own their namespace claims. Their imports must reach provider-aware
/// diagnostics instead of silently loading a nearby stdlib checkout and producing cascaded errors from the wrong
/// source graph.
fn sdk_catalog_claims_module_for_collection(provider_plan: &ProviderPlan, module_path: &[String]) -> bool {
    !matches!(
        provider_plan.resolve_module(module_path),
        ProviderModuleResolution::Unknown
    )
}

/// Collect and parse the entry file and all its dependencies, preserving structured diagnostic context.
pub(crate) fn collect_modules_detailed(entry_path: &str) -> Result<Vec<ParsedModule>, CliDiagnosticFailure> {
    collect_modules_detailed_with_feature_selection(entry_path, &FeatureSelection::default())
}

/// Collect and parse the entry file and all dependencies for one explicit Incan package-feature projection.
pub(crate) fn collect_modules_detailed_with_feature_selection(
    entry_path: &str,
    feature_selection: &FeatureSelection,
) -> Result<Vec<ParsedModule>, CliDiagnosticFailure> {
    collect_modules_detailed_with_selections(entry_path, feature_selection, None)
}

/// Collect and parse the entry file and all dependencies for one package-feature and SDK-profile projection.
pub(crate) fn collect_modules_detailed_with_selections(
    entry_path: &str,
    feature_selection: &FeatureSelection,
    sdk_profile_override: Option<&str>,
) -> Result<Vec<ParsedModule>, CliDiagnosticFailure> {
    let path = if Path::new(entry_path).is_absolute() {
        PathBuf::from(entry_path)
    } else {
        std::env::current_dir()
            .map_err(|error| {
                CliDiagnosticFailure::single(
                    entry_path,
                    "",
                    diagnostics::CompileError::new(
                        format!("failed to determine current directory: {error}"),
                        Span::default(),
                    ),
                    diagnostics::DiagnosticPhase::Tooling,
                )
            })?
            .join(entry_path)
    };
    let session = match sdk_profile_override {
        Some(profile) => CompilationSession::discover_with_selections(&path, feature_selection, Some(profile)),
        None => CompilationSession::discover_with_feature_selection(&path, feature_selection),
    }
    .map_err(|error| {
        CliDiagnosticFailure::single(
            path.to_string_lossy(),
            "",
            diagnostics::CompileError::new(error.message, Span::default()),
            diagnostics::DiagnosticPhase::Import,
        )
    })?;
    collect_modules_detailed_with_session(path, &session)
}

/// Collect one source graph through an already-resolved compilation session.
pub(crate) fn collect_modules_detailed_with_session(
    path: PathBuf,
    session: &CompilationSession,
) -> Result<Vec<ParsedModule>, CliDiagnosticFailure> {
    collect_modules_detailed_from_seeds(
        path.clone(),
        session,
        vec![(
            path.to_string_lossy().to_string(),
            "main".to_string(),
            vec!["main".to_string()],
        )],
    )
}

/// Collect every authored source module for library publication, including modules not imported by `src/lib.incn`.
pub(crate) fn collect_library_modules_detailed_with_session(
    path: PathBuf,
    session: &CompilationSession,
) -> Result<Vec<ParsedModule>, CliDiagnosticFailure> {
    let seeds = library_source_seeds(&path, session).map_err(CliDiagnosticFailure::from)?;
    let entry_path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let seeds = seeds
        .into_iter()
        .map(|(source_file, module_name, path_segments)| {
            (source_file.to_string_lossy().to_string(), module_name, path_segments)
        })
        .collect();
    let mut modules = collect_modules_detailed_from_seeds(path.clone(), session, seeds)?;
    if let Some(entry_index) = modules.iter().position(|module| {
        module
            .file_path
            .canonicalize()
            .unwrap_or_else(|_| module.file_path.clone())
            == entry_path
    }) {
        let entry = modules.remove(entry_index);
        modules.push(entry);
    }
    Ok(modules)
}

/// Build the single validated source set used by checked and unprojected library publication.
pub(crate) fn library_source_seeds(
    path: &Path,
    session: &CompilationSession,
) -> CliResult<Vec<(PathBuf, String, Vec<String>)>> {
    let mut source_files = Vec::new();
    collect_incan_source_files(&session.source_root, &mut source_files).map_err(|error| {
        CliError::failure(format!(
            "failed to discover library source modules under {}: {error}",
            session.source_root.display()
        ))
    })?;
    source_files.sort();

    let entry_path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let sdk_namespace_roots = if env::var_os(SDK_PROVIDER_BUILD_ENV).is_some() {
        let project_root = session
            .manifest
            .as_ref()
            .map(ProjectManifest::project_root)
            .unwrap_or_else(|| path.parent().unwrap_or(Path::new(".")));
        Some(sdk_provider_bootstrap_namespace_roots(project_root)?)
    } else {
        None
    };
    let mut logical_sources = BTreeMap::<Vec<String>, PathBuf>::new();
    let mut seeds = vec![(path.to_path_buf(), "main".to_string(), vec!["main".to_string()])];
    for source_file in source_files {
        let canonical = source_file.canonicalize().unwrap_or_else(|_| source_file.clone());
        if canonical == entry_path {
            continue;
        }
        let Some(path_segments) = logical_module_segments_from_file(&session.source_root, &source_file) else {
            continue;
        };
        if is_unselected_package_entrypoint(&session.source_root, &source_file, &entry_path) {
            continue;
        }
        if sdk_namespace_roots
            .as_ref()
            .is_some_and(|roots| path_segments.first().is_none_or(|root| !roots.contains(root)))
        {
            continue;
        }
        if let Some(existing) = logical_sources.insert(path_segments.clone(), canonical.clone()) {
            return Err(CliError::failure(format!(
                "{} and {} both resolve to library module `{}`; use either a module file or a directory entrypoint",
                existing.display(),
                canonical.display(),
                path_segments.join(".")
            )));
        }
        seeds.push((canonical, path_segments.join("_"), path_segments));
    }
    Ok(seeds)
}

/// Return whether a root `main.incn` or `lib.incn` is the package entrypoint not selected by this build.
fn is_unselected_package_entrypoint(source_root: &Path, source_file: &Path, selected_entrypoint: &Path) -> bool {
    let canonical_source_root = source_root.canonicalize().unwrap_or_else(|_| source_root.to_path_buf());
    let canonical_source_file = source_file.canonicalize().unwrap_or_else(|_| source_file.to_path_buf());
    if canonical_source_file == selected_entrypoint || canonical_source_file.parent() != Some(&canonical_source_root) {
        return false;
    }
    canonical_source_file
        .file_stem()
        .and_then(|stem| stem.to_str())
        .is_some_and(|stem| matches!(stem, "lib" | "main"))
}

/// Recursively collect authored `.incn` files without following directory symlinks.
pub(crate) fn collect_incan_source_files(directory: &Path, files: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            let name = entry.file_name();
            let name = name.to_str().unwrap_or("");
            // Broad source collection is used for package libraries and generated test batches. Compiler, editor,
            // and user tools all place non-source state below hidden directories; treating that state as an Incan
            // module can emit invalid Rust such as `pub mod .incan;`.
            if !name.starts_with('.') && name != "target" && name != "node_modules" {
                collect_incan_source_files(&entry.path(), files)?;
            }
        } else if file_type.is_file() && entry.path().extension().is_some_and(|extension| extension == "incn") {
            files.push(entry.path());
        }
    }
    Ok(())
}

/// Collect one source graph from explicit seed modules through an already-resolved compilation session.
fn collect_modules_detailed_from_seeds(
    path: PathBuf,
    session: &CompilationSession,
    mut to_process: Vec<(String, String, Vec<String>)>,
) -> Result<Vec<ParsedModule>, CliDiagnosticFailure> {
    let base_dir = path.parent().unwrap_or(Path::new("."));
    let mut modules = Vec::new();
    let mut processed = HashSet::new();
    let mut dependency_edges: HashMap<String, HashSet<String>> = HashMap::new();
    let mut incan_source_stdlib_module_paths: HashMap<String, PathBuf> = HashMap::new();
    let compiling_sdk_provider = env::var_os(SDK_PROVIDER_BUILD_ENV).is_some();
    let stdlib_module_segments = |module_path: &[String]| {
        if compiling_sdk_provider {
            module_path.iter().skip(1).cloned().collect()
        } else {
            let mut segments = vec![stdlib::INCAN_STD_NAMESPACE.to_string()];
            segments.extend(module_path.iter().skip(1).cloned());
            segments
        }
    };
    while let Some((file_path, module_name, path_segments)) = to_process.pop() {
        if processed.contains(&file_path) {
            continue;
        }
        processed.insert(file_path.clone());
        dependency_edges.entry(file_path.clone()).or_default();

        let source = read_source_for_diagnostics(&file_path)?;
        let file_path_obj = Path::new(&file_path);
        let is_incan_source_stdlib_module = path_segments
            .first()
            .is_some_and(|segment| segment == stdlib::INCAN_STD_NAMESPACE);
        let ast = match session.parse_source(file_path_obj, &source, !is_incan_source_stdlib_module) {
            Ok(a) => {
                // Surface any non-fatal parser warnings (e.g. RFC 005 dot-notation nudges) immediately,
                // so they reach the user regardless of which build/run/debug command was invoked. They are
                // collected for machine-readable reports later, off the retained AST, by the typecheck pass.
                render_module_warnings(&file_path, &source, &a.warnings);
                a
            }
            Err(errs) => {
                return Err(CliDiagnosticFailure::from_errors(
                    file_path,
                    source,
                    errs,
                    diagnostics::DiagnosticPhase::Parse,
                ));
            }
        };

        let current_base = file_path_obj.parent().unwrap_or(base_dir);
        if uses_iterator_adapter_surface(&ast) {
            let module_path = vec![
                stdlib::STDLIB_ROOT.to_string(),
                "derives".to_string(),
                "collection".to_string(),
            ];
            if !sdk_catalog_claims_module_for_collection(&session.provider_plan, &module_path) {
                let source_path = resolve_stdlib_module_source_path(&module_path)?;
                let module_segments = stdlib_module_segments(&module_path);
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
        }
        if uses_result_combinator_surface(&ast) {
            let module_path = vec![stdlib::STDLIB_ROOT.to_string(), "result".to_string()];
            if !sdk_catalog_claims_module_for_collection(&session.provider_plan, &module_path) {
                let source_path = resolve_stdlib_module_source_path(&module_path)?;
                let module_segments = stdlib_module_segments(&module_path);
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
        }
        for resolved in resolve_program_source_imports(&ast, current_base, Some(&session.source_root)) {
            let span = resolved.span;
            match resolved.resolution {
                SourceModuleImportResolution::Stdlib { module_path } => {
                    if stdlib::stdlib_stub_path(&module_path).is_none() {
                        continue;
                    }
                    if sdk_catalog_claims_module_for_collection(&session.provider_plan, &module_path) {
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

                    let module_segments = stdlib_module_segments(&module_path);
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
                    let module_segments = canonicalize_source_module_segments(&module_ref.path_segments);
                    let module_name = module_segments.join("_");
                    if !processed.contains(&dep_path_str) {
                        to_process.push((dep_path_str.clone(), module_name, module_segments));
                    }
                    dependency_edges
                        .entry(file_path.clone())
                        .or_default()
                        .insert(dep_path_str);
                }
                SourceModuleImportResolution::SelfImport {
                    module_ref,
                    import_path,
                    can_use_root_import,
                } => {
                    return Err(CliDiagnosticFailure::single(
                        file_path,
                        source,
                        diagnostics::CompileError::new(
                            self_import_diagnostic_message(&module_ref, &import_path, can_use_root_import),
                            span,
                        ),
                        diagnostics::DiagnosticPhase::Typecheck,
                    ));
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

    Ok(topologically_sort_modules(modules, &dependency_edges)?)
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

/// Report an ignored `Cargo.toml` beside a Loaf manifest, at most once per project root per invocation.
///
/// RFC 117 rule 11: a `loaf.toml` project containing `Cargo.toml` must warn and ignore the Cargo configuration, and
/// the diagnostic must name the ignored file and explain that Cargo compatibility is selected explicitly. Both are
/// carried by [`ManifestError::CargoIgnored`], which renders the text; this decides only when it reaches the user.
///
/// It warns rather than fails, and never inspects the Cargo file: the RFC requires Oven to "continue as a Loaf
/// project" and say what it ignored, and reading the file to describe it better would be the parsing rule 11
/// forbids. A directory that holds no `loaf.toml` is silent here — a Cargo-only project is Cargo-compatibility
/// mode's subject, not an ignored file.
///
/// Callers are the project-scale command entry points rather than one deep shared helper, because Oven's cached
/// paths return a completed output without preparing the project at all; a warning behind preparation would appear
/// on a cold build and vanish on a warm one, which is worse than not having it.
pub(crate) fn warn_once_about_ignored_cargo_manifest(project_root: &Path) {
    let DiscoveredManifest::Loaf(_) = discovered_manifest_kind(project_root) else {
        return;
    };
    let cargo_manifest = project_root.join(CARGO_MANIFEST_FILENAME);
    if !cargo_manifest.is_file() {
        return;
    }
    let key = project_root
        .canonicalize()
        .unwrap_or_else(|_| project_root.to_path_buf());
    let Ok(mut reported) = IGNORED_CARGO_MANIFESTS_REPORTED.lock() else {
        // A poisoned registry means another thread panicked mid-report. Losing the deduplication is the right
        // failure here: a repeated warning is noise, a dropped one hides that Cargo configuration was ignored.
        eprintln!("warning: {}", ManifestError::CargoIgnored { path: cargo_manifest });
        return;
    };
    if reported.insert(key) {
        eprintln!("warning: {}", ManifestError::CargoIgnored { path: cargo_manifest });
    }
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
/// Bare sibling paths and absolute `crate.*` paths are both local source-module edges and must contribute to the same
/// closure.
///
/// Use this variant inside per-module loops to avoid rebuilding the module key map on every iteration.
pub(crate) fn imported_module_deps_for_with_index<'m>(
    modules: &'m [ParsedModule],
    module_index: usize,
    module_idx_by_key: &HashMap<String, usize>,
) -> Vec<(&'m str, &'m Program)> {
    imported_module_deps_for_with_index_and_plan(modules, module_index, module_idx_by_key, None)
}

/// Resolve imported source dependencies with the SDK producer's bootstrap namespace bridge enabled.
pub(crate) fn imported_module_deps_for_with_provider_plan<'m>(
    modules: &'m [ParsedModule],
    module_index: usize,
    module_idx_by_key: &HashMap<String, usize>,
    provider_plan: &ProviderPlan,
) -> Vec<(&'m str, &'m Program)> {
    imported_module_deps_for_with_index_and_plan(modules, module_index, module_idx_by_key, Some(provider_plan))
}

/// Shared source-dependency closure with an optional provider-build namespace bridge.
fn imported_module_deps_for_with_index_and_plan<'m>(
    modules: &'m [ParsedModule],
    module_index: usize,
    module_idx_by_key: &HashMap<String, usize>,
    provider_plan: Option<&ProviderPlan>,
) -> Vec<(&'m str, &'m Program)> {
    // ---- Context: bounds and setup ----
    if module_index >= modules.len() {
        return Vec::new();
    }

    // ---- Context: walk the transitive local source-module import closure ----
    /// Collect immediate local source dependencies for one module from both bare and absolute `crate.*` imports.
    fn direct_local_dep_indexes(
        modules: &[ParsedModule],
        module_index: usize,
        module_idx_by_key: &HashMap<String, usize>,
        provider_plan: Option<&ProviderPlan>,
    ) -> BTreeSet<usize> {
        /// Resolve one import path to the exact collected source module, including a safe nested-entry fallback.
        fn resolve_local_dep_index(
            current_module_path: &[String],
            path: &ImportPath,
            module_idx_by_key: &HashMap<String, usize>,
            provider_plan: Option<&ProviderPlan>,
        ) -> Option<usize> {
            let exact = logical_source_import_candidates(current_module_path, path)
                .into_iter()
                .find_map(|candidate| {
                    let key = canonicalize_source_module_segments(&candidate).join("_");
                    module_idx_by_key.get(&key).copied()
                });
            if exact.is_some() {
                return exact;
            }

            // An SDK producer writes its public spelling (`std.registry`) while compiling the physical provider
            // source module (`registry`). Only its explicit bootstrap grant authorizes this source-graph edge.
            if path.parent_levels == 0
                && !path.is_absolute
                && provider_plan.is_some_and(|plan| plan.bootstrap_owns_sdk_module(&path.segments))
                && path.segments.first().map(String::as_str) == Some(stdlib::STDLIB_ROOT)
            {
                let physical_key = canonicalize_source_module_segments(&path.segments[1..]).join("_");
                if let Some(index) = module_idx_by_key.get(&physical_key).copied() {
                    return Some(index);
                }
            }
            if path.is_absolute || path.parent_levels > 0 {
                return None;
            }

            // CLI entrypoints retain the synthetic logical name `main` even when their file lives in a nested source
            // directory. The on-disk resolver has already admitted the sibling into this module set; recover that
            // canonical identity only when the bare import suffix identifies exactly one collected module.
            let suffix = canonicalize_source_module_segments(&path.segments).join("_");
            if suffix.is_empty() {
                return None;
            }
            let suffix = format!("_{suffix}");
            let mut matches = module_idx_by_key
                .iter()
                .filter_map(|(key, index)| key.ends_with(&suffix).then_some(*index))
                .collect::<Vec<_>>();
            matches.sort_unstable();
            matches.dedup();
            match matches.as_slice() {
                [index] => Some(*index),
                _ => None,
            }
        }

        let mut dep_indexes: BTreeSet<usize> = BTreeSet::new();
        for decl in &modules[module_index].ast.declarations {
            let crate::frontend::ast::Declaration::Import(import) = &decl.node else {
                continue;
            };
            match &import.kind {
                ImportKind::From { module, .. } => {
                    if let Some(dep_idx) = resolve_local_dep_index(
                        &modules[module_index].path_segments,
                        module,
                        module_idx_by_key,
                        provider_plan,
                    ) && dep_idx != module_index
                    {
                        dep_indexes.insert(dep_idx);
                    }
                }
                ImportKind::Module(path) => {
                    let dep_idx = resolve_local_dep_index(
                        &modules[module_index].path_segments,
                        path,
                        module_idx_by_key,
                        provider_plan,
                    )
                    .or_else(|| {
                        let mut parent_path = path.clone();
                        parent_path.segments.pop();
                        resolve_local_dep_index(
                            &modules[module_index].path_segments,
                            &parent_path,
                            module_idx_by_key,
                            provider_plan,
                        )
                    });
                    if let Some(dep_idx) = dep_idx
                        && dep_idx != module_index
                    {
                        dep_indexes.insert(dep_idx);
                    }
                }
                _ => {}
            }
        }
        dep_indexes
    }

    let mut dep_indexes: BTreeSet<usize> = BTreeSet::new();
    let mut pending: Vec<usize> = direct_local_dep_indexes(modules, module_index, module_idx_by_key, provider_plan)
        .into_iter()
        .collect();
    while let Some(dep_idx) = pending.pop() {
        if dep_idx == module_index || !dep_indexes.insert(dep_idx) {
            continue;
        }
        pending.extend(direct_local_dep_indexes(
            modules,
            dep_idx,
            module_idx_by_key,
            provider_plan,
        ));
    }

    // ---- Context: materialize dependency pairs for typechecker.check_with_imports ----
    dep_indexes
        .into_iter()
        .map(|idx| (modules[idx].name.as_str(), &modules[idx].ast))
        .collect()
}

/// Register every collected module's real path segments with a checker before typechecking.
///
/// The dependency cache is keyed by the flattened, underscore-joined module name, which also names the emitted Rust
/// module and is therefore not injective: `pkg.helpers` and a module literally named `pkg_helpers` flatten to one
/// string. Supplying the true segments lets a canonical identity name the module that answered rather than the
/// spelling that asked. Mirrors the pair `IrCodegen::add_module_with_path_segments` already carries for emission.
pub(crate) fn register_module_path_segments(checker: &mut typechecker::TypeChecker, modules: &[ParsedModule]) {
    for module in modules {
        checker.register_dependency_module_path_segments(&module.name, module.path_segments.clone());
    }
}

/// Typecheck all collected modules in dependency-safe order using shared CLI diagnostics formatting.
///
/// This helper centralizes the per-module checker setup used by `build` and `check` paths so warning/error rendering
/// stays consistent across command flows.
#[cfg(test)]
pub(crate) fn typecheck_modules_with_import_graph(
    modules: &[ParsedModule],
    manifest: Option<&ProjectManifest>,
    provider_plan: &Arc<ProviderPlan>,
    #[cfg(feature = "rust_inspect")] rust_inspect_manifest_dir: Option<&Path>,
) -> CliResult<()> {
    typecheck_modules_with_import_graph_detailed_for_c_abi_target(
        modules,
        manifest,
        provider_plan,
        None,
        #[cfg(feature = "rust_inspect")]
        rust_inspect_manifest_dir,
    )
    .map(|_warnings| ())
    .map_err(|failure| CliError::failure(failure.render_human()))
}

/// Typecheck modules while selecting one already-validated C ABI target for this invocation.
///
/// Returns the non-fatal parser and typechecker diagnostics collected along the way, so callers that emit a
/// machine-readable report can include them; callers that only care about acceptance can discard them.
pub(crate) fn typecheck_modules_with_import_graph_detailed_for_c_abi_target(
    modules: &[ParsedModule],
    manifest: Option<&ProjectManifest>,
    provider_plan: &Arc<ProviderPlan>,
    c_abi_plan: Option<&CAbiVerificationPlan>,
    #[cfg(feature = "rust_inspect")] rust_inspect_manifest_dir: Option<&Path>,
) -> Result<Vec<CliDiagnostic>, CliDiagnosticFailure> {
    typecheck_modules_with_import_graph_artifacts(
        modules,
        manifest,
        provider_plan,
        c_abi_plan,
        #[cfg(feature = "rust_inspect")]
        rust_inspect_manifest_dir,
    )
    .map(|artifacts| artifacts.warnings)
}

/// Products retained from one dependency-safe typechecking pass.
struct TypecheckModuleArtifacts {
    type_infos: Vec<TypeCheckInfo>,
    stdlib_cache: StdlibAstCache,
    /// Non-fatal parser and typechecker diagnostics, for reports that expose them.
    ///
    /// Ordered by module, then by parser-before-typechecker within each module, so a machine-readable report is
    /// byte-stable across runs of the same source.
    warnings: Vec<CliDiagnostic>,
}

/// Typecheck a collected graph in dependency-safe order and retain one result for every input module plus the
/// source-backed stdlib metadata lowering needs.
///
/// Ordering is intentional: session analysis also needs an identity-keyed representation for synthetic modules that
/// share a source file path.
fn typecheck_modules_with_import_graph_artifacts(
    modules: &[ParsedModule],
    manifest: Option<&ProjectManifest>,
    provider_plan: &Arc<ProviderPlan>,
    c_abi_plan: Option<&CAbiVerificationPlan>,
    #[cfg(feature = "rust_inspect")] rust_inspect_manifest_dir: Option<&Path>,
) -> Result<TypecheckModuleArtifacts, CliDiagnosticFailure> {
    let declared = manifest.map(|m| m.declared_rust_crate_names());
    let module_idx_by_key = module_key_index(modules);
    let mut diagnostics_out = Vec::new();
    let mut warnings_out = Vec::new();
    let mut type_infos = Vec::with_capacity(modules.len());
    let mut stdlib_cache = StdlibAstCache::new();

    for (idx, module) in modules.iter().enumerate() {
        let deps_for_module =
            imported_module_deps_for_with_provider_plan(modules, idx, &module_idx_by_key, provider_plan);

        // Parser warnings were already rendered at parse time; collect them here so both warning classes reach
        // machine-readable reports from one deterministic sweep.
        warnings_out.extend(parser_warning_diagnostics(module));

        let mut checker = typechecker::TypeChecker::new();
        checker.stdlib_cache = stdlib_cache.clone();
        if let Some(names) = declared.clone() {
            checker.set_declared_crate_names(names);
        }
        checker.set_current_module_path(Some(module.path_segments.clone()));
        register_module_path_segments(&mut checker, modules);
        checker.set_provider_plan(Arc::clone(provider_plan));
        #[cfg(feature = "rust_inspect")]
        if let Some(rust_inspect_manifest_dir) = rust_inspect_manifest_dir {
            checker.set_rust_inspect_manifest_dir(rust_inspect_manifest_dir.to_path_buf());
        }

        // A provider producer checks its complete source package before publishing the public checked facade.
        let check_result = if provider_plan.bootstrap_sdk_namespace_roots().next().is_some() {
            checker.check_with_imports_allow_private(&module.ast, &deps_for_module)
        } else {
            checker.check_with_imports(&module.ast, &deps_for_module)
        };
        // Warnings accumulate on the checker whether or not checking succeeded, so surface them from both arms:
        // a module that also has an error should not hide its warnings from the user or from the report.
        render_module_warnings(
            module.file_path.to_string_lossy().as_ref(),
            &module.source,
            checker.warnings(),
        );
        warnings_out.extend(typecheck_warning_diagnostics(module, checker.warnings()));

        match check_result {
            Ok(()) => {
                type_infos.push(checker.type_info().clone());
                stdlib_cache = checker.stdlib_cache.clone();
            }
            Err(errs) => {
                stdlib_cache = checker.stdlib_cache.clone();
                diagnostics_out.extend(errs.into_iter().map(|error| CliDiagnostic {
                    file_path: module.file_path.to_string_lossy().to_string(),
                    source: module.source.clone(),
                    phase: typecheck_diagnostic_phase(module, error.span),
                    error,
                }));
            }
        }
    }

    if diagnostics_out.is_empty() {
        verify_checked_c_bindings(modules, manifest, &mut type_infos, c_abi_plan, &mut diagnostics_out);
    }

    if diagnostics_out.is_empty() {
        Ok(TypecheckModuleArtifacts {
            type_infos,
            stdlib_cache,
            warnings: warnings_out,
        })
    } else {
        Err(CliDiagnosticFailure {
            diagnostics: diagnostics_out,
            warnings: warnings_out,
        })
    }
}

/// Verify every checked C descriptor against the host or explicitly selected target before any code generation.
///
/// The typechecker has already established the source contract. This phase only checks those explicit facts against
/// the declared header for the target; it neither imports arbitrary headers nor performs library discovery.
fn verify_checked_c_bindings(
    modules: &[ParsedModule],
    manifest: Option<&ProjectManifest>,
    type_infos: &mut [TypeCheckInfo],
    selected_plan: Option<&CAbiVerificationPlan>,
    diagnostics_out: &mut Vec<CliDiagnostic>,
) {
    let mut bindings = Vec::new();
    for (module_index, (module, type_info)) in modules.iter().zip(type_infos.iter()).enumerate() {
        let mut module_bindings = type_info.c_abi.bindings.values().cloned().collect::<Vec<_>>();
        module_bindings.sort_by(|left, right| left.class_name.cmp(&right.class_name));
        bindings.extend(
            module_bindings
                .into_iter()
                .map(|binding| (module_index, module, binding)),
        );
    }
    if bindings.is_empty() {
        return;
    }
    let Some(plan) = selected_plan.cloned().or_else(CAbiVerificationPlan::host) else {
        for (_, module, binding) in bindings {
            diagnostics_out.push(CliDiagnostic {
                file_path: module.file_path.to_string_lossy().to_string(),
                source: module.source.clone(),
                phase: diagnostics::DiagnosticPhase::Typecheck,
                error: diagnostics::CompileError::type_error(
                    "checked C bindings currently require a Linux x86-64 or macOS arm64 host verification target"
                        .to_string(),
                    binding.span,
                ),
            });
        }
        return;
    };
    let toolchain = match ClangToolchain::discover(&plan) {
        Ok(toolchain) => toolchain,
        Err(error) => {
            for (_, module, binding) in bindings {
                diagnostics_out.push(CliDiagnostic {
                    file_path: module.file_path.to_string_lossy().to_string(),
                    source: module.source.clone(),
                    phase: diagnostics::DiagnosticPhase::Typecheck,
                    error: diagnostics::CompileError::type_error(error.to_string(), binding.span),
                });
            }
            return;
        }
    };
    for (module_index, module, binding) in bindings {
        let verification_binding = resolve_package_owned_c_binding_header(manifest, &binding);
        match verify_checked_c_binding(&toolchain, &plan, &verification_binding) {
            Ok(receipt) => {
                let enum_values = &mut type_infos[module_index].c_abi.enum_values;
                for ((enumeration, variant), value) in receipt.enum_values() {
                    enum_values.insert(
                        (binding.class_name.clone(), enumeration.clone(), variant.clone()),
                        *value,
                    );
                }
            }
            Err(error) => {
                diagnostics_out.push(CliDiagnostic {
                    file_path: module.file_path.to_string_lossy().to_string(),
                    source: module.source.clone(),
                    phase: diagnostics::DiagnosticPhase::Typecheck,
                    error: diagnostics::CompileError::type_error(error.to_string(), binding.span),
                });
            }
        }
    }
}

/// Resolve a manifest-declared package header before passing its spelling to Clang.
///
/// Checked bindings keep the authored, package-relative header spelling in their public descriptor and lock identity.
/// The verifier alone needs a concrete file location. Restricting this translation to a header declared under
/// `[interop.c]` prevents an arbitrary relative binding path from becoming an ambient include-directory search.
fn resolve_package_owned_c_binding_header(
    manifest: Option<&ProjectManifest>,
    binding: &CBindingDescriptor,
) -> CBindingDescriptor {
    let Some(manifest) = manifest else {
        return binding.clone();
    };
    if Path::new(&binding.header).is_absolute() {
        return binding.clone();
    }
    let declared = manifest.interop_c().is_some_and(|interop| {
        interop.targets.iter().any(|target| {
            target.headers.iter().any(|header| header == &binding.header)
                || target
                    .shims
                    .iter()
                    .flat_map(|shim| &shim.headers)
                    .any(|header| header == &binding.header)
        })
    });
    if !declared {
        return binding.clone();
    }
    let mut resolved = binding.clone();
    resolved.header = manifest
        .project_root()
        .join(&binding.header)
        .to_string_lossy()
        .into_owned();
    resolved
}

/// Classify diagnostics that are still emitted by the typechecker but originate from an import declaration span.
fn typecheck_diagnostic_phase(module: &ParsedModule, span: Span) -> diagnostics::DiagnosticPhase {
    diagnostics::phase_for_typecheck_span(&module.ast, span)
}

/// Render one module's non-fatal diagnostics to stderr in the shared CLI format.
///
/// Warnings never fail a command, so they are shown the moment they are produced rather than held until a
/// machine-readable report is assembled: `run`, `build`, and `fmt` invocations that never emit JSON must still
/// surface them.
pub(crate) fn render_module_warnings(file_path: &str, source: &str, warnings: &[diagnostics::CompileError]) {
    for warning in warnings {
        eprint!("{}", diagnostics::format_error(file_path, source, warning));
    }
}

/// Project one module's non-fatal parser diagnostics into the structured shape machine-readable reports consume.
///
/// Parser warnings are read back off [`ParsedModule::ast`] rather than captured at parse time because the parse
/// pass is shared by commands that never typecheck; keeping collection here lets the report gather both warning
/// classes in one deterministic sweep without changing when the user first sees them.
fn parser_warning_diagnostics(module: &ParsedModule) -> Vec<CliDiagnostic> {
    module
        .ast
        .warnings
        .iter()
        .map(|warning| CliDiagnostic {
            file_path: module.file_path.to_string_lossy().to_string(),
            source: module.source.clone(),
            phase: diagnostics::DiagnosticPhase::Parse,
            error: warning.clone(),
        })
        .collect()
}

/// Project one module's non-fatal typechecker diagnostics into the same structured shape.
///
/// Warnings and errors share [`CliDiagnostic`] deliberately: severity already travels on
/// `CompileError::kind` and survives into `StableDiagnostic::severity`, so which collection a diagnostic arrived
/// in carries no information the payload does not already hold.
fn typecheck_warning_diagnostics(module: &ParsedModule, warnings: &[diagnostics::CompileError]) -> Vec<CliDiagnostic> {
    warnings
        .iter()
        .map(|warning| CliDiagnostic {
            file_path: module.file_path.to_string_lossy().to_string(),
            source: module.source.clone(),
            phase: typecheck_diagnostic_phase(module, warning.span),
            error: warning.clone(),
        })
        .collect()
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frontend::typechecker::{self, IdentKind};
    use crate::library_manifest::{LibraryManifest, ProviderFeatureMetadata, VocabExports};
    use incan_core::lang::c_abi::LinkCapabilityId;
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

    #[test]
    fn compilation_session_reuses_provider_plan_for_identical_module_usage() -> Result<(), Box<dyn std::error::Error>> {
        let library_manifest_index = LibraryManifestIndex::default();
        let provider_plan = Arc::new(ProviderPlan::default());
        let session = CompilationSession {
            manifest: None,
            source_root: PathBuf::from("fixture"),
            library_imported_vocab: library_manifest_index.library_imported_vocab(),
            library_imported_dsl_surfaces: library_manifest_index.library_imported_dsl_surfaces(),
            library_manifest_index,
            provider_plans_by_modules: Arc::new(Mutex::new(BTreeMap::from([(
                BTreeSet::new(),
                Arc::clone(&provider_plan),
            )]))),
            provider_plan,
            sdk_inventory: None,
            sdk_components: None,
            package_feature_plan: None,
            active_features: BTreeSet::new(),
            declared_features: BTreeSet::new(),
            contract_model_bundles: Vec::new(),
        };
        let module = parsed_module_for_test("from std.io import stdout\n\ndef main() -> None:\n    pass\n")?;

        let first = session.provider_plan_for_modules(std::slice::from_ref(&module))?;
        let second = session.provider_plan_for_modules(std::slice::from_ref(&module))?;

        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(session.provider_plan_cache_entry_count()?, 2);
        Ok(())
    }

    #[test]
    fn package_declared_c_header_is_resolved_only_for_verification() -> Result<(), Box<dyn std::error::Error>> {
        let manifest = ProjectManifest::from_str(
            "[project]\nname = \"c_header_fixture\"\n\n[interop.c]\nschema = 1\n\n[[interop.c.targets]]\ntarget = \"aarch64-apple-darwin\"\nheaders = [\"interop/include/bridge.h\"]\n",
            Path::new("/workspace/c_header_fixture/loaf.toml"),
        )?;
        let binding = CBindingDescriptor {
            span: Span::new(0, 0),
            class_name: "Bridge".to_string(),
            header: "interop/include/bridge.h".to_string(),
            system_library: "bridge".to_string(),
            link_capability: LinkCapabilityId::SystemLibrary,
            resources: Vec::new(),
            symbols: Vec::new(),
            enums: Vec::new(),
            structs: Vec::new(),
        };

        let resolved = resolve_package_owned_c_binding_header(Some(&manifest), &binding);

        assert_eq!(binding.header, "interop/include/bridge.h");
        assert_eq!(resolved.header, "/workspace/c_header_fixture/interop/include/bridge.h");
        Ok(())
    }

    #[test]
    fn explicit_sdk_selection_rejects_legacy_inventoryless_toolchains() -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let manifest_path = project.path().join("loaf.toml");
        fs::write(
            &manifest_path,
            "[project]\nname = \"demo\"\n\n[sdk]\nprofile = \"minimal\"\n",
        )?;
        let manifest = ProjectManifest::load(&manifest_path)?;
        let error = validate_component_inventory_selection(Some(&manifest), None, None)
            .err()
            .ok_or("expected explicit SDK selection to require an inventory")?;

        assert!(error.message.contains("no component inventory"));
        assert!(
            error.message.contains("loaf.toml:4:1"),
            "expected the explicit SDK table location, got: {}",
            error.message
        );
        assert!(validate_component_inventory_selection(None, None, None).is_ok());
        Ok(())
    }

    #[test]
    fn sdk_selection_errors_retain_manifest_or_command_provenance() -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let manifest_path = project.path().join("loaf.toml");
        fs::write(
            &manifest_path,
            "[project]\nname = \"demo\"\n\n[sdk]\nprofile = \"minimal\"\ncomponents = [\"stdlib-web\"]\n",
        )?;
        let manifest = ProjectManifest::load(&manifest_path)?;
        let selection = SdkComponentSelection::from_manifest(Some(&manifest));
        let component_error = SdkResolutionError::UnknownComponent {
            component: "stdlib-web".to_string(),
            sdk_identity: "incan@0.5.0".to_string(),
        };

        let rendered = format_sdk_selection_error(&component_error, &selection, Some(&manifest), None);
        assert!(
            rendered.contains("loaf.toml:6:15"),
            "expected exact SDK component location, got: {rendered}"
        );

        let profile_error = SdkResolutionError::UnknownProfile {
            profile: "tiny".to_string(),
            sdk_identity: "incan@0.5.0".to_string(),
        };
        let rendered = format_sdk_selection_error(&profile_error, &selection, Some(&manifest), Some("tiny"));
        assert!(
            rendered.contains("current command's `--sdk-profile` override"),
            "expected transient profile provenance, got: {rendered}"
        );
        Ok(())
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
    fn compilation_session_parses_with_imported_library_vocab() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let project_root = tmp.path();
        std::fs::create_dir_all(project_root.join("src"))?;
        std::fs::write(
            project_root.join("loaf.toml"),
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
        std::fs::create_dir_all(project_root.join("deps/widgets"))?;
        std::fs::write(
            project_root.join("deps/widgets/loaf.toml"),
            "[project]\nname = \"widgets\"\nversion = \"0.1.0\"\n",
        )?;
        write_minimal_library_artifact(project_root, "widgets", "widgets_core", &manifest)?;

        let main_path = project_root.join("src/main.incn");
        let source = "import pub::widgets\n\ndef main() -> None:\n  assert true\n";
        std::fs::write(&main_path, source)?;

        let session = CompilationSession::discover_with_feature_selection(&main_path, &FeatureSelection::default())?;
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

    #[cfg(feature = "rust_inspect")]
    #[test]
    fn rust_inspect_query_paths_include_explicit_rust_item_imports() -> Result<(), Box<dyn std::error::Error>> {
        let module = parsed_module_for_test(
            r#"
from rust::datafusion::execution::context import SessionContext
from rust::datafusion::prelude import CsvReadOptions, read_csv
from rust::incan_stdlib::async::runtime import block_on
from rust::std::fs import metadata
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
                "std::fs::metadata".to_string(),
            ]
        );

        let program_paths = collect_rust_inspect_query_paths_from_programs([&parsed_module_for_test(
            r#"
from rust::rustix::fs import flock
"#,
        )?
        .ast]);
        assert_eq!(program_paths, vec!["rustix::fs::flock".to_string()]);
        Ok(())
    }

    #[cfg(feature = "rust_inspect")]
    #[test]
    fn rust_inspect_derive_probes_include_only_used_direct_rust_imports() -> Result<(), Box<dyn std::error::Error>> {
        let module = parsed_module_for_test(
            r#"
from rust::provider::prelude import Component as ForeignComponent, UnusedDerive

@derive(ForeignComponent, Clone)
model Velocity:
  x: f32

@rust.derive(ForeignComponent, "provider::derive::ExplicitComponent", "provider::r#async::Component", "provider::derive::Bad;Drop", Clone)
model ExplicitVelocity:
  x: f32
"#,
        )?;

        assert_eq!(
            collect_rust_inspect_derive_probe_paths(&[module]),
            vec![
                "provider::derive::ExplicitComponent".to_string(),
                "provider::prelude::Component".to_string(),
                "provider::r#async::Component".to_string(),
            ],
            "directly imported and explicit-path Rust derives that are actually invoked must become semantic probes"
        );
        Ok(())
    }

    #[cfg(feature = "rust_inspect")]
    #[test]
    fn rust_inspect_derive_probe_aliases_remain_module_scoped() -> Result<(), Box<dyn std::error::Error>> {
        let left = parsed_module_for_test(
            r#"
from rust::left_provider import Component as SharedDerive

@derive(SharedDerive)
model LeftValue:
  value: int
"#,
        )?;
        let right = parsed_module_for_test(
            r#"
from rust::right_provider import Component as SharedDerive

@rust.derive(SharedDerive)
model RightValue:
  value: int
"#,
        )?;

        assert_eq!(
            collect_rust_inspect_derive_probe_paths(&[left, right]),
            vec![
                "left_provider::Component".to_string(),
                "right_provider::Component".to_string(),
            ],
            "the same visible alias in separate modules must retain both module-local Rust identities"
        );
        Ok(())
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
        let manifest = ProjectManifest::from_str(manifest_content, &project.join("loaf.toml"))?;

        let root = resolve_source_root(&project, Some(&manifest));
        assert_eq!(root, project.join("lib"));
        Ok(())
    }

    #[test]
    fn root_std_imports_select_the_same_provider_modules_as_qualified_imports() -> Result<(), Box<dyn std::error::Error>>
    {
        let root = parsed_module_for_test("from std import math as arithmetic, serde\n")?;
        let qualified = parsed_module_for_test("import std.math\nimport std.serde\n")?;
        let root_paths = provider_used_module_paths(&[root]);
        assert_eq!(root_paths, provider_used_module_paths(&[qualified]));
        assert!(root_paths.contains(&vec!["std".to_string(), "math".to_string()]));
        assert!(root_paths.contains(&vec!["std".to_string(), "serde".to_string()]));
        assert!(!root_paths.contains(&vec!["std".to_string()]));
        Ok(())
    }

    #[test]
    fn root_std_provider_discovery_excludes_external_and_relative_imports() -> Result<(), Box<dyn std::error::Error>> {
        let external = parsed_module_for_test(
            "from rust::std import cmp\nfrom pub::std import math\nfrom ..std import serde\nfrom crate.std import json\n",
        )?;
        let empty = parsed_module_for_test("def main() -> None:\n    pass\n")?;
        assert_eq!(
            provider_used_module_paths(&[external]),
            provider_used_module_paths(&[empty])
        );
        Ok(())
    }

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
    fn collect_modules_canonicalizes_directory_entrypoints() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let project_root = tmp.path();
        std::fs::write(
            project_root.join("loaf.toml"),
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
    fn collect_modules_keeps_migrated_stdlib_sources_out_of_consumer_graphs() -> Result<(), Box<dyn std::error::Error>>
    {
        let tmp = tempfile::tempdir()?;
        let entry = tmp.path().join("main.incn");
        std::fs::write(
            &entry,
            "from std.environ import get_str\n\ndef main() -> None:\n    get_str(\"HOME\")\n",
        )?;

        let modules = collect_modules(&entry.to_string_lossy())?;
        assert_eq!(
            modules.len(),
            1,
            "migrated stdlib imports must be supplied by the artifact, not source modules"
        );
        assert_eq!(modules[0].path_segments, ["main"]);
        Ok(())
    }

    #[test]
    fn collect_modules_supports_init_directory_entrypoints() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let project_root = tmp.path();
        std::fs::write(
            project_root.join("loaf.toml"),
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
            project_root.join("loaf.toml"),
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
            project_root.join("loaf.toml"),
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
    pub rels: list[str]
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
            project_root.join("loaf.toml"),
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
    pub rel_id: str

@derive(Clone)
pub model SubstraitPlan:
    pub plan_id: str
    pub root_rel_id: str
    pub rels: list[SubstraitRelNode]
    pub profile_tags: list[str]

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
    fn provider_bootstrap_std_import_adds_the_physical_source_dependency() -> Result<(), Box<dyn std::error::Error>> {
        let make_module = |name: &str, source: &str| -> Result<ParsedModule, Box<dyn std::error::Error>> {
            let tokens = lexer::lex(source).map_err(|errors| format!("{name} lex failed: {errors:?}"))?;
            let ast = parser::parse(&tokens).map_err(|errors| format!("{name} parse failed: {errors:?}"))?;
            Ok(ParsedModule {
                name: name.to_string(),
                path_segments: vec![name.to_string()],
                file_path: PathBuf::from(format!("{name}.incn")),
                source: source.to_string(),
                ast,
            })
        };
        let modules = vec![
            make_module("registry", "pub model Registry:\n    label: str\n")?,
            make_module(
                "features",
                "from std.registry import Registry\n\npub def label(value: Registry) -> str:\n    return value.label\n",
            )?,
        ];
        let module_idx_by_key = module_key_index(&modules);
        let features_index = modules
            .iter()
            .position(|module| module.path_segments == ["features".to_string()])
            .ok_or("expected features module")?;
        assert!(
            imported_module_deps_for_with_index(&modules, features_index, &module_idx_by_key).is_empty(),
            "an ordinary consumer must not reinterpret std.registry as a local source import"
        );

        let provider_plan = ProviderPlan::default().with_bootstrap_sdk_namespace_roots(["registry".to_string()]);
        let dependencies =
            imported_module_deps_for_with_provider_plan(&modules, features_index, &module_idx_by_key, &provider_plan);
        assert_eq!(
            dependencies.iter().map(|(name, _)| *name).collect::<Vec<_>>(),
            vec!["registry"],
            "the bootstrap grant must add the exact physical provider source edge"
        );
        Ok(())
    }

    #[test]
    fn imported_module_deps_preserve_bare_sibling_class_privacy_issue886() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let src_dir = tmp.path().join("src");
        let pkg_dir = src_dir.join("pkg");
        std::fs::create_dir_all(&pkg_dir)?;
        std::fs::write(
            tmp.path().join("loaf.toml"),
            "[project]\nname = \"sibling_private_class\"\nversion = \"0.1.0\"\n",
        )?;
        std::fs::write(
            pkg_dir.join("vaults.incn"),
            "pub class Vault:\n    secret: str = \"sealed\"\n    pub label: str\n",
        )?;
        let consumer_path = pkg_dir.join("consumer.incn");
        std::fs::write(
            &consumer_path,
            r#"from vaults import Vault

def leak() -> str:
    value = Vault(label="visible")
    return value.secret
"#,
        )?;

        let modules = collect_modules(consumer_path.to_string_lossy().as_ref())?;
        let consumer_index = modules
            .iter()
            .position(|module| module.file_path == consumer_path)
            .ok_or("expected nested consumer module")?;
        let module_idx_by_key = module_key_index(&modules);
        let dependencies = imported_module_deps_for_with_index(&modules, consumer_index, &module_idx_by_key);
        assert!(
            dependencies.iter().any(|(name, _)| *name == "pkg_vaults"),
            "bare sibling imports must retain the canonical nested dependency; modules={:?}, dependencies={:?}",
            modules
                .iter()
                .map(|module| (module.name.clone(), module.path_segments.clone()))
                .collect::<Vec<_>>(),
            dependencies
                .iter()
                .map(|(name, _)| (*name).to_string())
                .collect::<Vec<_>>()
        );

        let mut checker = typechecker::TypeChecker::new();
        checker.set_current_module_path(Some(modules[consumer_index].path_segments.clone()));
        let errors = match checker.check_with_imports(&modules[consumer_index].ast, &dependencies) {
            Ok(()) => return Err("private sibling field access must fail typechecking".into()),
            Err(errors) => errors,
        };
        assert!(
            errors
                .iter()
                .any(|error| error.message.contains("Field 'secret' on 'Vault' is private")),
            "expected private-field diagnostic, got: {:?}",
            errors.iter().map(|error| &error.message).collect::<Vec<_>>()
        );
        Ok(())
    }

    /// Verifies that absolute from-imports and module imports both contribute local dependency metadata before
    /// typechecking.
    #[test]
    fn imported_module_deps_preserve_absolute_crate_public_type_metadata_issue882()
    -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let project_root = tmp.path();
        std::fs::write(
            project_root.join("loaf.toml"),
            r#"[project]
name = "absolute_crate_public_types"
version = "0.1.0"
"#,
        )?;
        let src_dir = project_root.join("src");
        std::fs::create_dir_all(&src_dir)?;

        std::fs::write(
            src_dir.join("types.incn"),
            r#"pub enum Access:
    Allowed
    Denied

pub model Decision:
    pub admitted: bool
    pub reason: str
"#,
        )?;
        std::fs::write(
            src_dir.join("consumer.incn"),
            r#"from crate.types import Access, Decision

pub def allowed() -> Access:
    return Access.Allowed

pub def explain(decision: Decision) -> str:
    if decision.admitted:
        return decision.reason
    return "denied"
"#,
        )?;
        std::fs::write(src_dir.join("module_consumer.incn"), "import crate.types\n")?;
        let entry = src_dir.join("lib.incn");
        std::fs::write(
            &entry,
            r#"pub from crate.consumer import allowed, explain
pub from crate.types import Access, Decision
import crate.module_consumer
"#,
        )?;

        let modules = collect_modules(entry.to_string_lossy().as_ref())?;
        let module_idx_by_key = module_key_index(&modules);
        let consumer_idx = modules
            .iter()
            .position(|module| module.file_path.ends_with("src/consumer.incn"))
            .ok_or("expected src/consumer.incn module")?;
        let consumer_deps = imported_module_deps_for_with_index(&modules, consumer_idx, &module_idx_by_key);
        assert!(
            consumer_deps.iter().any(|(name, _)| *name == "types"),
            "expected absolute from-import dependency `consumer -> types`, got: {:?}",
            consumer_deps
                .iter()
                .map(|(name, _)| (*name).to_string())
                .collect::<Vec<_>>()
        );

        let module_consumer_idx = modules
            .iter()
            .position(|module| module.file_path.ends_with("src/module_consumer.incn"))
            .ok_or("expected src/module_consumer.incn module")?;
        let module_consumer_deps =
            imported_module_deps_for_with_index(&modules, module_consumer_idx, &module_idx_by_key);
        assert!(
            module_consumer_deps.iter().any(|(name, _)| *name == "types"),
            "expected absolute module-import dependency `module_consumer -> types`, got: {:?}",
            module_consumer_deps
                .iter()
                .map(|(name, _)| (*name).to_string())
                .collect::<Vec<_>>()
        );

        for (idx, module) in modules.iter().enumerate() {
            let deps = imported_module_deps_for_with_index(&modules, idx, &module_idx_by_key);
            let mut checker = typechecker::TypeChecker::new();
            if let Err(errs) = checker.check_with_imports(&module.ast, &deps) {
                return Err(format!(
                    "typecheck failed for module {}: {:?}",
                    module.file_path.display(),
                    errs.iter().map(|error| error.message.clone()).collect::<Vec<_>>()
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
            project_root.join("loaf.toml"),
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
            project_root.join("loaf.toml"),
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
    fn session_analysis_keeps_crate_root_facade_class_reexports_as_types() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let project_root = tmp.path();
        let source_root = project_root.join("src");
        let session_root = source_root.join("session");
        std::fs::create_dir_all(&session_root)?;
        std::fs::write(
            project_root.join("loaf.toml"),
            "[project]\nname = \"crate_root_facade\"\nversion = \"0.1.0\"\n",
        )?;
        std::fs::write(session_root.join("types.incn"), "pub class Session:\n    pub id: int\n")?;
        std::fs::write(
            session_root.join("mod.incn"),
            "pub from crate.session.types import Session\n",
        )?;
        let main_path = source_root.join("main.incn");
        let main_source = "from session import Session\n\ndef main() -> None:\n    session = Session(id=1)\n";
        std::fs::write(&main_path, main_source)?;

        let session = CompilationSession::discover_with_feature_selection(&main_path, &FeatureSelection::default())?;
        let modules = collect_modules_detailed_with_session(main_path.clone(), &session)
            .map_err(|failure| failure.render_human())?;
        let module_idx_by_key = module_key_index(&modules);
        let facade_index = modules
            .iter()
            .position(|module| module.file_path.ends_with("src/session/mod.incn"))
            .ok_or("expected the session facade module")?;
        let facade_dependencies = imported_module_deps_for_with_index(&modules, facade_index, &module_idx_by_key);
        assert!(
            facade_dependencies.iter().any(|(name, _)| *name == "session_types"),
            "crate-root imports must contribute their source dependency to the session analysis closure; got: {:?}",
            facade_dependencies
                .iter()
                .map(|(name, _)| (*name).to_string())
                .collect::<Vec<_>>()
        );

        let analysis = session
            .analyze_modules(
                &modules,
                #[cfg(feature = "rust_inspect")]
                None,
            )
            .map_err(|failure| failure.render_human())?;
        let callee_start = main_source.find("Session(id=1)").ok_or("expected constructor call")?;
        assert_eq!(
            analysis
                .type_info_for_path(&main_path)
                .ok_or("expected main session analysis")?
                .ident_kind(Span::new(callee_start, callee_start + "Session".len())),
            Some(IdentKind::TypeName),
            "a public facade re-export of a crate-root class must stay a class constructor in shared session facts"
        );
        Ok(())
    }

    #[test]
    fn dependency_closure_includes_crate_root_module_imports() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let project_root = tmp.path();
        let source_root = project_root.join("src");
        let types_root = source_root.join("types");
        std::fs::create_dir_all(&types_root)?;
        std::fs::write(
            project_root.join("loaf.toml"),
            "[project]\nname = \"crate_root_module_import\"\nversion = \"0.1.0\"\n",
        )?;
        std::fs::write(types_root.join("user.incn"), "pub class User:\n    pub id: int\n")?;
        let consumer_path = source_root.join("consumer.incn");
        std::fs::write(
            &consumer_path,
            "import crate.types.user\n\npub def consume() -> None:\n    pass\n",
        )?;
        let main_path = source_root.join("main.incn");
        std::fs::write(
            &main_path,
            "from consumer import consume\n\ndef main() -> None:\n    consume()\n",
        )?;

        let session = CompilationSession::discover_with_feature_selection(&main_path, &FeatureSelection::default())?;
        let modules = collect_modules_detailed_with_session(main_path.clone(), &session)
            .map_err(|failure| failure.render_human())?;
        let module_idx_by_key = module_key_index(&modules);
        let consumer_index = modules
            .iter()
            .position(|module| module.file_path.ends_with("src/consumer.incn"))
            .ok_or("expected the consumer module")?;
        let dependencies = imported_module_deps_for_with_index(&modules, consumer_index, &module_idx_by_key);
        assert!(
            dependencies.iter().any(|(name, _)| *name == "types_user"),
            "crate-root module imports must contribute their source dependency to the closure; got: {:?}",
            dependencies
                .iter()
                .map(|(name, _)| (*name).to_string())
                .collect::<Vec<_>>()
        );
        Ok(())
    }

    #[test]
    fn collect_modules_supports_example_entry_with_cyclic_src_interfaces() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let project_root = tmp.path();
        std::fs::write(
            project_root.join("loaf.toml"),
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
            project_root.join("loaf.toml"),
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
            project_root.join("loaf.toml"),
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

    #[test]
    fn helper_requirements_keep_unused_active_sdk_path_targets_issue911() -> Result<(), Box<dyn std::error::Error>> {
        let workspace = tempfile::tempdir()?;
        let artifact = workspace.path().join("unused-sdk-provider");
        let record = crate::provider::ProviderRecord {
            identity: crate::provider::ProviderIdentity {
                name: "incan_issue911_unused_sdk".to_string(),
                version: "0.5.0".to_string(),
                digest: "sha256:issue911-unused".to_string(),
                feature_projection: BTreeSet::new(),
            },
            provenance: crate::provider::ProviderProvenance::Sdk {
                sdk_identity: "incan@0.5.0".to_string(),
                component_id: "issue911-unused".to_string(),
                inventory_path: None,
            },
            authority: crate::provider::NamespaceAuthority::SdkReserved,
            namespace_claims: BTreeSet::from([vec!["std".to_string(), "issue911_unused".to_string()]]),
            available: true,
            enabled: true,
            manifest: Some(Arc::new(LibraryManifest::new("incan_issue911_unused_sdk", "0.5.0"))),
            artifact: Some(LibraryArtifactMetadata::from_crate_root(
                "incan_issue911_unused_sdk",
                "incan_issue911_unused_sdk",
                &artifact,
            )),
            implementation_facets: Vec::new(),
        };
        let plan = ProviderPlan::new(
            crate::frontend::library_manifest_index::LibraryManifestIndex::default(),
            vec![record.clone()],
            std::iter::empty(),
        )?;
        assert!(
            plan.sdk_link_roots().is_empty(),
            "unused SDK provider must not become a direct link root"
        );

        let mut requirements = ProjectRequirements::default();
        extend_requirements_with_provider_plan(&mut requirements, &plan)?;

        assert!(
            requirements.dependencies.is_empty(),
            "unused provider must not be linked directly"
        );
        assert_eq!(requirements.sdk_path_dependencies.len(), 1);
        assert!(matches!(
            &requirements.sdk_path_dependencies[0].source,
            DependencySource::Path { path } if path == &artifact
        ));

        let used_plan = ProviderPlan::new(
            crate::frontend::library_manifest_index::LibraryManifestIndex::default(),
            vec![record],
            provider_used_module_paths(&[parsed_module_for_test("from std import issue911_unused as selected\n")?]),
        )?;
        let mut used_requirements = ProjectRequirements::default();
        extend_requirements_with_provider_plan(&mut used_requirements, &used_plan)?;
        assert_eq!(used_requirements.dependencies.len(), 1);
        assert!(
            !used_requirements.dependencies[0].default_features,
            "new direct SDK edges must render explicit default-features = false"
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
            &Arc::new(ProviderPlan::default()),
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
            &Arc::new(ProviderPlan::default()),
            #[cfg(feature = "rust_inspect")]
            None,
        );
        assert!(result.is_err(), "expected unresolved symbol to fail typecheck");

        Ok(())
    }

    #[test]
    fn compilation_session_projects_declared_package_features() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let source_root = tmp.path().join("src");
        std::fs::create_dir_all(&source_root)?;
        std::fs::write(
            tmp.path().join("loaf.toml"),
            "[project]\nname = \"feature_projection\"\n\n[project.features]\ndefault = [\"json\"]\njson = []\n",
        )?;
        let source_path = source_root.join("main.incn");
        let source = "when feature(\"json\"):\n    const JSON_ENABLED = true\n\nconst ALWAYS = true\n";
        std::fs::write(&source_path, source)?;

        let default_session =
            CompilationSession::discover_with_feature_selection(&source_path, &FeatureSelection::default())?;
        let default_program = default_session
            .parse_source(&source_path, source, false)
            .map_err(|errors| std::io::Error::other(format!("default feature parse failed: {errors:?}")))?;
        assert_eq!(default_program.declarations.len(), 2);

        let selection = FeatureSelection {
            no_default_features: true,
            ..FeatureSelection::default()
        };
        let minimal_session = CompilationSession::discover_with_feature_selection(&source_path, &selection)?;
        let tooling_program = minimal_session
            .parse_source_unprojected(&source_path, source, false)
            .map_err(|errors| std::io::Error::other(format!("tooling feature parse failed: {errors:?}")))?;
        assert_eq!(
            tooling_program.declarations.len(),
            2,
            "tooling must retain inactive declarations before semantic projection"
        );
        let minimal_program = minimal_session
            .parse_source(&source_path, source, false)
            .map_err(|errors| std::io::Error::other(format!("minimal feature parse failed: {errors:?}")))?;
        assert_eq!(minimal_program.declarations.len(), 1);

        let unknown_source = "when feature(\"missing\"):\n    const VALUE = true\n";
        let errors = minimal_session
            .parse_source(&source_path, unknown_source, false)
            .err()
            .ok_or("unknown feature should fail source projection")?;
        assert!(
            errors
                .iter()
                .any(|error| error.message.contains("Unknown package feature `missing`"))
        );
        Ok(())
    }

    #[test]
    fn library_source_seeds_deduplicate_imports_across_noncanonical_source_roots_issue948()
    -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let source_root = tmp.path().join("stdlib");
        let project_root = source_root.join("components/core");
        let entrypoint = project_root.join("src/lib.incn");
        let operations = source_root.join("traits/ops.incn");
        std::fs::create_dir_all(entrypoint.parent().ok_or("entrypoint must have a parent")?)?;
        std::fs::create_dir_all(operations.parent().ok_or("operations must have a parent")?)?;
        std::fs::write(
            project_root.join("loaf.toml"),
            "[project]\nname = \"core\"\n\n[build]\nsource-root = \"../..\"\n",
        )?;
        std::fs::write(&entrypoint, "import traits.ops\n")?;
        std::fs::write(
            &operations,
            "pub def add(left: int, right: int) -> int:\n  return left + right\n",
        )?;

        let session = CompilationSession::discover_with_feature_selection(&entrypoint, &FeatureSelection::default())?;
        let modules = collect_library_modules_detailed_with_session(entrypoint, &session)
            .map_err(|failure| failure.render_human())?;
        let operations_modules = modules
            .iter()
            .filter(|module| module.path_segments == ["traits".to_string(), "ops".to_string()])
            .collect::<Vec<_>>();

        assert_eq!(
            operations_modules.len(),
            1,
            "all-source discovery and an authored import must share one canonical source identity"
        );
        assert_eq!(operations_modules[0].file_path, operations.canonicalize()?);
        Ok(())
    }

    #[test]
    fn library_source_seeds_exclude_the_unselected_root_entrypoint_issue948() -> Result<(), Box<dyn std::error::Error>>
    {
        let tmp = tempfile::tempdir()?;
        let source_root = tmp.path().join("src");
        std::fs::create_dir_all(&source_root)?;
        let lib = source_root.join("lib.incn");
        let main = source_root.join("main.incn");
        std::fs::write(&lib, "pub def exported() -> int:\n  return 1\n")?;
        std::fs::write(&main, "def main() -> None:\n  pass\n")?;

        assert!(!is_unselected_package_entrypoint(
            &source_root,
            &lib,
            &lib.canonicalize()?
        ));
        assert!(is_unselected_package_entrypoint(
            &source_root,
            &main,
            &lib.canonicalize()?
        ));
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
    fn dependency_manifest_modes_separate_parser_from_admitted_artifacts() {
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
            crate::oven::receipt_generated_project(
                &crate::oven::OvenGeneratedProjectRequest::new(
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
        let release_path = crate::oven::default_receipt_path(project_root);
        crate::oven::write_receipt(&receipt("release")?, &release_path)?;
        assert!(!oven_library_dependency_has_verified_profile_receipts(project_root));

        let debug_path = release_path.with_file_name("library-debug-receipt.json");
        crate::oven::write_receipt(&receipt("debug")?, &debug_path)?;
        assert!(oven_library_dependency_has_verified_profile_receipts(project_root));

        fs::write(&debug_path, "not a receipt")?;
        assert!(!oven_library_dependency_has_verified_profile_receipts(project_root));
        Ok(())
    }

    #[test]
    fn compilation_session_analysis_bundles_lowering_inputs_with_semantic_facts()
    -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let project_root = tmp.path();
        let source_root = project_root.join("src");
        std::fs::create_dir_all(&source_root)?;
        std::fs::write(
            project_root.join("loaf.toml"),
            "[project]\nname = \"analysis_consumer\"\n",
        )?;
        let main_path = source_root.join("main.incn");
        std::fs::write(
            &main_path,
            "def helper() -> int:\n  return 1\n\ndef main() -> int:\n  return helper()\n",
        )?;

        let session = CompilationSession::discover_for_collection_with_feature_selection(
            &main_path,
            &FeatureSelection::default(),
        )?;
        let modules = collect_modules_detailed_with_session(main_path.clone(), &session)
            .map_err(|failure| failure.render_human())?;
        let analysis = session
            .analyze_modules(
                &modules,
                #[cfg(feature = "rust_inspect")]
                None,
            )
            .map_err(|failure| failure.render_human())?;
        let entry_analysis = analysis
            .module_analysis_for_path(&main_path)
            .ok_or("expected one bundled session analysis for the entry module")?;
        let snapshot = entry_analysis.semantic_snapshot();
        let type_info = entry_analysis.type_info();
        let retained_type_info = analysis
            .type_info_for_path(&main_path)
            .ok_or("expected the session to retain lowering input for the entry module")?;

        assert!(std::ptr::eq(type_info, retained_type_info));
        let entry_module = modules
            .iter()
            .find(|module| module.file_path == main_path)
            .ok_or("expected the session to collect the entry module")?;
        let lowered = crate::frontend::body_ir::build_body_ir_module_v0(
            &entry_module.ast,
            &entry_module.path_segments,
            type_info,
        );
        assert!(lowered.render_snapshot().contains("body main"));
        assert!(snapshot.render_snapshot().contains("decl:main::helper type=() -> int"));
        assert!(
            snapshot
                .render_snapshot()
                .contains("symbol_target=function:main::helper")
        );
        Ok(())
    }

    #[test]
    fn compilation_session_analysis_preserves_same_file_module_identities() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let source_root = tmp.path().join("src");
        std::fs::create_dir_all(&source_root)?;
        std::fs::write(
            tmp.path().join("loaf.toml"),
            "[project]\nname = \"identity_consumer\"\n",
        )?;
        let shared_path = source_root.join("shared.incn");
        std::fs::write(&shared_path, "def value() -> int:\n  return 1\n")?;

        let mut first = parsed_module_for_test("def first() -> int:\n  return 1\n")?;
        first.name = "first".to_string();
        first.path_segments = vec!["first".to_string()];
        first.file_path = shared_path.clone();
        let mut second = parsed_module_for_test("def second() -> int:\n  return 2\n")?;
        second.name = "second".to_string();
        second.path_segments = vec!["second".to_string()];
        second.file_path = shared_path.clone();

        let analysis = CompilationSession::discover_with_feature_selection(&shared_path, &FeatureSelection::default())?
            .analyze_modules(
                &[first, second],
                #[cfg(feature = "rust_inspect")]
                None,
            )
            .map_err(|failure| failure.render_human())?;

        assert!(analysis.type_info_for_module_path(&["first".to_string()]).is_some());
        assert!(analysis.type_info_for_module_path(&["second".to_string()]).is_some());
        Ok(())
    }

    #[test]
    fn broad_source_collection_ignores_hidden_and_generated_directories() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        fs::write(tmp.path().join("root.incn"), "def root() -> None:\n  pass\n")?;
        fs::create_dir_all(tmp.path().join("nested"))?;
        fs::write(tmp.path().join("nested/module.incn"), "def nested() -> None:\n  pass\n")?;
        for directory in [".ralph-cache", ".incan", "target", "node_modules"] {
            let hidden = tmp.path().join(directory);
            fs::create_dir_all(&hidden)?;
            fs::write(hidden.join("not_source.incn"), "def ignored() -> None:\n  pass\n")?;
        }

        let mut files = Vec::new();
        collect_incan_source_files(tmp.path(), &mut files)?;
        files.sort();
        let relative = files
            .iter()
            .map(|path| path.strip_prefix(tmp.path()).map(Path::to_path_buf))
            .collect::<Result<Vec<_>, _>>()?;

        assert_eq!(
            relative,
            vec![PathBuf::from("nested/module.incn"), PathBuf::from("root.incn")]
        );
        Ok(())
    }
}
