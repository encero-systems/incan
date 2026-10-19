//! The compilation session: one per command, owning the module set, the provider plan, and the analysis every
//! stage after parsing consumes.
//!
//! A session is constructed once from the project requirements and reused by every stage so provider selection,
//! vocab providers, and typechecking are decided once per command; the analysis-invocation scope below is the
//! test instrument that proves that.

#[cfg(test)]
use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use incan_lang::lang::stdlib;

#[cfg(feature = "cli")]
use crate::build::library_dependencies::PreparedLibraryDependencies;
use crate::diagnostics::CliDiagnosticFailure;
use crate::error::{CliError, CliResult};
use crate::project::{discover_effective_project_manifest, resolve_project_root, resolve_source_root};
use crate::typecheck::typecheck_modules_with_import_graph_artifacts;
use incan_frontend::ast::{Program, Span};
use incan_frontend::contract_metadata::{
    CanonicalModelBundle, materialize_contract_models, read_project_model_bundles,
};
use incan_frontend::hir::build_semantic_module_snapshot_v0;
use incan_frontend::library_manifest_index::{
    LibraryArtifactMetadata, LibraryManifestIndex, LibraryManifestIndexEntry,
};
use incan_frontend::parsed_module::ParsedModule;
use incan_frontend::testing_markers::{
    TestingMarkerSemantics, load_testing_marker_semantics, testing_marker_semantics_from_manifest,
};
use incan_frontend::typechecker::TypeCheckInfo;
use incan_frontend::typechecker::stdlib_loader::StdlibAstCache;
use incan_frontend::{diagnostics, lexer, parser, vocab_desugar_pass};
use incan_provider::inventory::{
    discover_active_sdk_inventory, provider_source_used_module_paths, provider_used_module_paths,
    resolve_sdk_component_selection, sdk_provider_bootstrap_namespace_roots, validate_component_inventory_selection,
};
use incan_provider::requirements::{
    DependencyManifestMode, SdkInventorySource, SourceLibraryManifestPreference, checked_source_library_manifest_index,
    parser_only_library_manifest_index, prepare_library_dependency_artifacts,
};
use incan_provider::{
    FeatureSelection, PackageFeatureGraph, PackageFeaturePlan, ProviderModuleResolution, ProviderPlan,
    ProviderProvenance, ResolvedSdkComponents, SdkComponentSelection, SdkInventory,
};
use oven_model::manifest::ProjectManifest;
/// Select only already published dependencies from this exact SDK source catalog. A same-named foreign project
/// remains a source dependency; its name alone cannot grant it the catalog component's checked artifact.
fn published_sdk_dependency_entries(
    project: &ProjectManifest,
    inventory: &SdkInventory,
    catalog: &incan_provider::SdkSourceCatalog,
) -> CliResult<HashMap<String, LibraryManifestIndexEntry>> {
    let mut entries = HashMap::new();
    for (key, dependency) in project.library_dependencies() {
        // ---- Source ownership: only this catalog can replace a public dependency edge ----
        let source = std::fs::canonicalize(&dependency.path).map_err(|error| CliError::failure(error.to_string()))?;
        let mut components = catalog
            .components
            .values()
            .filter(|component| std::fs::canonicalize(&component.project_root).is_ok_and(|root| root == source));
        let Some(component) = components.next() else {
            continue;
        };
        if components.next().is_some() {
            return Err(CliError::failure(
                "SDK source dependency has competing catalog components",
            ));
        }
        // ---- Published identity: bind the selected provider to its source declaration ----
        let declaration = ProjectManifest::discover(&source)
            .map_err(|error| CliError::failure(error.to_string()))?
            .ok_or_else(|| CliError::failure("SDK dependency has no source declaration"))?;
        let declared = declaration
            .project
            .as_ref()
            .ok_or_else(|| CliError::failure("SDK dependency has no project identity"))?;
        let Some(selected) = inventory
            .components
            .get(&component.id)
            .filter(|selected| selected.available)
        else {
            continue;
        };
        let mut providers = selected.providers.iter().filter(|provider| {
            declared.name.as_deref() == Some(provider.name.as_str())
                && declared.version.as_deref() == Some(provider.version.as_str())
        });
        let provider = providers
            .next()
            .ok_or_else(|| CliError::failure("published SDK dependency differs from its source identity"))?;
        if providers.next().is_some() {
            return Err(CliError::failure("SDK dependency has competing published providers"));
        }
        let root = provider
            .crate_root
            .as_ref()
            .ok_or_else(|| CliError::failure("published SDK dependency has no artifact root"))?;
        // ---- Physical artifact: validate bytes and the complete materialized contract ----
        let digest = incan_frontend::library_manifest::digest_provider_artifact(root)
            .map_err(|error| CliError::failure(error.to_string()))?;
        if digest != provider.digest {
            return Err(CliError::failure("published SDK dependency artifact digest changed"));
        }
        let entry = incan_frontend::library_manifest_index::load_provider_dependency_artifact(key, root);
        match &entry {
            LibraryManifestIndexEntry::Loaded { manifest, metadata }
                if manifest.name == provider.name
                    && manifest.version == provider.version
                    && metadata.kind == incan_frontend::library_manifest_index::LibraryArtifactKind::Materialized => {}
            LibraryManifestIndexEntry::Failed(failure) => return Err(CliError::failure(failure.message.clone())),
            _ => {
                return Err(CliError::failure(
                    "published SDK dependency has no checked materialized artifact",
                ));
            }
        }
        entries.insert(key.clone(), entry);
    }
    Ok(entries)
}

/// Shared immutable provider projections indexed by the canonical modules an invocation uses.
type ProviderPlanCache = Arc<Mutex<BTreeMap<BTreeSet<Vec<String>>, Arc<ProviderPlan>>>>;

/// Source dependencies already checked in one invocation, keyed by package root and enabled features.
type CheckedSourceDependencies = BTreeMap<(PathBuf, BTreeSet<String>), CheckedSourceDependency>;

/// Checked public metadata and the original source-backed metadata owners that justified it.
#[derive(Clone)]
struct CheckedSourceDependency {
    manifest: incan_frontend::library_manifest::LibraryManifest,
    provider: Arc<ProviderPlan>,
    source_metadata: Vec<StdlibAstCache>,
}

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
pub struct CompilationSessionAnalysisInvocationScope {
    previous_count: Option<usize>,
}

/// Count compilation-session analysis invocations within a scope on the current test thread.
#[cfg(test)]
pub fn scoped_compilation_session_analysis_invocations() -> CompilationSessionAnalysisInvocationScope {
    let previous_count = COMPILATION_SESSION_ANALYSIS_INVOCATIONS.with(|count| count.replace(Some(0)));
    CompilationSessionAnalysisInvocationScope { previous_count }
}

#[cfg(test)]
impl CompilationSessionAnalysisInvocationScope {
    /// Return the analysis calls observed since this scope was constructed.
    #[cfg(test)]
    pub fn invocation_count(&self) -> usize {
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

/// Checked products produced by one [`CompilationSession`] analysis pass.
///
/// The current Rust-source backend still lowers from [`TypeCheckInfo`], while compiler-facing consumers use the
/// portable snapshot. Keeping both products in one analysis result prevents a CLI command from independently checking
/// the same sources and then treating its second result as authoritative.
///
/// The `TypeCheckInfo` half is a transition bridge. Remove it when Body IR owns every lowering query tracked by #225.
#[derive(Debug, Clone)]
pub struct CompilationAnalysis {
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
pub struct CompilationModuleAnalysis<'analysis> {
    type_info: &'analysis TypeCheckInfo,
    semantic_snapshot: &'analysis incan_semantics_core::SemanticModuleSnapshot,
}

impl CompilationModuleAnalysis<'_> {
    /// Return the session-owned transition bridge Body IR currently requires for lowering.
    pub fn type_info(&self) -> &TypeCheckInfo {
        self.type_info
    }

    /// Return the portable semantic module produced beside this lowering bridge.
    pub fn semantic_snapshot(&self) -> &incan_semantics_core::SemanticModuleSnapshot {
        self.semantic_snapshot
    }
}

impl CompilationAnalysis {
    /// Keep source metadata for generated-output retention only when analysis used no external C header verification.
    /// The metadata cache shares its original source owners with codegen and rejects ambient legacy discovery.
    pub fn test_generation_sources(&self) -> Option<StdlibAstCache> {
        self.type_info_by_path
            .values()
            .all(|info| info.c_abi.bindings.is_empty())
            .then(|| self.stdlib_cache.clone())
    }

    /// Return the paired checked products for one collected source file.
    ///
    /// A caller must consume this bundle when it needs both Body-IR lowering and semantic provenance. Looking up the
    /// halves separately would make it too easy to join facts from different analysis products at the CLI boundary.
    pub fn module_analysis_for_path(&self, path: &Path) -> Option<CompilationModuleAnalysis<'_>> {
        Some(CompilationModuleAnalysis {
            type_info: self.type_info_by_path.get(path)?,
            semantic_snapshot: self.semantic_snapshots_by_path.get(path)?,
        })
    }

    /// Return the lowering input for one collected source file.
    pub fn type_info_for_path(&self, path: &Path) -> Option<&TypeCheckInfo> {
        self.type_info_by_path.get(path)
    }

    /// Return the lowering input for one compiler module identity.
    ///
    /// Identity is distinct from a source file path because the test runner may create multiple compiler modules rooted
    /// at the same file.
    pub fn type_info_for_module_path(&self, path: &[String]) -> Option<&TypeCheckInfo> {
        self.type_info_by_module_path.get(path)
    }

    /// Return portable HIR and semantic fact snapshots keyed by source path.
    pub fn semantic_snapshots(&self) -> &BTreeMap<PathBuf, incan_semantics_core::SemanticModuleSnapshot> {
        &self.semantic_snapshots_by_path
    }

    /// Return the source-backed stdlib metadata accumulated by this analysis.
    ///
    /// Lowering currently queries this cache for source-defined trait and type metadata. It stays part of the session
    /// result until those queries move to portable semantic facts and Body IR (#225).
    pub fn stdlib_cache(&self) -> &StdlibAstCache {
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
/// Provider selection is owned by the session. Consumers borrow it through getters instead of replacing inputs.
///
/// ```compile_fail,E0616
/// fn replace(session: &mut incan_driver::session::CompilationSession) {
///     session.provider_plan = session.provider_plan.clone();
/// }
/// ```
/// ```compile_fail,E0616
/// fn replace(session: &mut incan_driver::session::CompilationSession) {
///     session.sdk_inventory = None;
/// }
/// ```
/// ```compile_fail,E0616
/// fn replace(session: &mut incan_driver::session::CompilationSession) {
///     session.sdk_components = None;
/// }
/// ```
/// ```compile_fail,E0616
/// fn replace(session: &mut incan_driver::session::CompilationSession) {
///     session.package_feature_plan = None;
/// }
/// ```
#[derive(Debug, Clone)]
pub struct CompilationSession {
    /// Existing vocabulary module map shared across cloned sessions; fresh source and guest state remain local.
    pub(crate) vocab_runtime: Arc<Mutex<Option<vocab_desugar_pass::WasmDesugarerRuntime>>>,
    pub manifest: Option<ProjectManifest>,
    pub source_root: PathBuf,
    pub library_manifest_index: LibraryManifestIndex,
    /// Immutable provider projection shared by compiler stages for ordinary package dependencies and SDK providers.
    provider_plan: Arc<ProviderPlan>,
    /// Module-usage projections already derived from the immutable session inputs.
    ///
    /// Collection, requirement discovery, and semantic analysis all need the same projection. Rebuilding it makes
    /// every command rehash provider source roots and, worse, lets a mutable local cache dominate the warm path.
    provider_plans_by_modules: ProviderPlanCache,
    /// Original ordinary checked package capabilities retained throughout analysis and current plan preparation.
    #[cfg(feature = "cli")]
    admitted_library_dependencies: Option<Arc<PreparedLibraryDependencies>>,
    /// Original provider plan issued with the private admitted dependencies, before public session fields can change.
    admitted_provider_plan: Option<Arc<ProviderPlan>>,
    /// Semantic provider identities reused only within this checked compilation context.
    provider_semantic_identities: Arc<incan_provider::lock_semantics::ProviderSemanticIdentitySession>,
    /// Original metadata sources consulted by recursive unbaked dependency checks, retained without execution
    /// authority.
    checked_source_metadata: Vec<StdlibAstCache>,
    /// Integrity-checked active SDK catalog, when this toolchain is component-aware.
    sdk_inventory: Option<Arc<SdkInventory>>,
    /// Project-selected SDK component closure, when an inventory is active.
    sdk_components: Option<ResolvedSdkComponents>,
    /// Typed additive feature closure for the root package and active path dependencies.
    package_feature_plan: Option<PackageFeaturePlan>,
    /// Active root-package features used to project compilation-unit `when feature(...)` declarations.
    pub active_features: BTreeSet<String>,
    /// Declared root-package features used for source diagnostics before projection.
    pub declared_features: BTreeSet<String>,
    pub library_imported_vocab: parser::ImportedLibraryVocab,
    pub library_imported_dsl_surfaces: parser::ImportedLibraryDslSurfaces,
    pub contract_model_bundles: Vec<CanonicalModelBundle>,
}

impl CompilationSession {
    /// Check local provider sources against their public package contract without granting execution authority.
    pub fn discover_for_check(
        entry_path: &Path,
        feature_selection: &FeatureSelection,
        sdk_profile_override: Option<&str>,
    ) -> CliResult<Self> {
        let mut session = Self::discover_with_dependency_mode_and_sdk_source(
            entry_path,
            DependencyManifestMode::CheckMetadata,
            SdkInventorySource::DiscoverOnly,
            feature_selection,
            sdk_profile_override,
        )?;
        let Some(features) = session.package_feature_plan.clone() else {
            return Ok(session);
        };
        session.check_source_dependencies(
            &features,
            sdk_profile_override,
            &mut BTreeMap::new(),
            &mut BTreeSet::new(),
        )?;
        Ok(session)
    }

    /// Reuse checked dependencies only while their original source-backed metadata remains authenticated.
    /// Legacy metadata without retained owners keeps canonical checking on every request.
    pub(crate) fn checked_source_metadata_current(&self) -> bool {
        self.checked_source_metadata
            .iter()
            .all(StdlibAstCache::supports_generation_reuse)
            && self.provider_plan.verify_standard_source_publication().is_ok()
    }

    /// Check one unbaked path dependency through its public metadata, recursing into its own source dependencies.
    #[cfg(feature = "cli")]
    fn check_source_dependency(
        root: &Path,
        selected: BTreeSet<String>,
        features: &PackageFeaturePlan,
        sdk_profile_override: Option<&str>,
        completed: &mut CheckedSourceDependencies,
        visiting: &mut BTreeSet<PathBuf>,
    ) -> CliResult<CheckedSourceDependency> {
        let child_manifest = discover_effective_project_manifest(root)?
            .ok_or_else(|| CliError::failure("source dependency manifest disappeared during checking"))?;
        let child_entry = crate::build::library_exports::validate_library_entrypoint(&child_manifest)?;
        let selection = FeatureSelection {
            requested: selected,
            no_default_features: true,
            all_features: false,
        };
        let mut child = Self::discover_with_dependency_mode_and_sdk_source(
            &child_entry,
            DependencyManifestMode::CheckMetadata,
            SdkInventorySource::DiscoverOnly,
            &selection,
            sdk_profile_override,
        )?;
        child.check_source_dependencies(features, sdk_profile_override, completed, visiting)?;
        let (manifest, metadata) =
            crate::build::library_project::checked_source_library_manifest(&child, &child_entry)?;
        child.checked_source_metadata.push(metadata);
        Ok(CheckedSourceDependency {
            manifest,
            provider: Arc::clone(&child.provider_plan),
            source_metadata: child.checked_source_metadata,
        })
    }

    /// Without the build pipeline (the `cli` feature, #1479) an unbaked path dependency has no public-metadata
    /// checker, so it is refused rather than read as source.
    #[cfg(not(feature = "cli"))]
    fn check_source_dependency(
        root: &Path,
        _selected: BTreeSet<String>,
        _features: &PackageFeaturePlan,
        _sdk_profile_override: Option<&str>,
        _completed: &mut CheckedSourceDependencies,
        _visiting: &mut BTreeSet<PathBuf>,
    ) -> CliResult<CheckedSourceDependency> {
        Err(CliError::failure(format!(
            "checking the unbaked source dependency at {} requires the build pipeline",
            root.display()
        )))
    }

    /// Materialize only command-local checked metadata, sharing exact feature instances across a dependency graph.
    fn check_source_dependencies(
        &mut self,
        features: &PackageFeaturePlan,
        sdk_profile_override: Option<&str>,
        completed: &mut CheckedSourceDependencies,
        visiting: &mut BTreeSet<PathBuf>,
    ) -> CliResult<()> {
        let _trace =
            tracing::trace_span!(target: "incan::invocation", "incan_driver::check_source_dependencies").entered();
        let Some(manifest) = self.manifest.as_ref() else {
            return Ok(());
        };
        let mut entries = HashMap::new();
        let mut checked_dependencies = Vec::new();
        let active = self
            .package_feature_plan
            .as_ref()
            .and_then(|plan| plan.root_package())
            .map(|root| root.active_dependencies.clone())
            .unwrap_or_default();
        for key in active {
            let Some(dependency) = manifest.library_dependencies().get(&key) else {
                continue;
            };
            if !dependency
                .path
                .join(oven_model::manifest::LOAF_MANIFEST_FILENAME)
                .is_file()
            {
                if let Some(entry) = self.library_manifest_index.get(&key) {
                    entries.insert(key, entry.clone());
                }
                continue;
            }
            let root = dependency
                .path
                .canonicalize()
                .map_err(|error| CliError::failure(error.to_string()))?;
            let selected = features
                .package(&root)
                .map(|state| state.features.active_features.clone())
                .unwrap_or_default();
            let identity = (root.clone(), selected.clone());
            let checked = if let Some(checked) = completed.get(&identity) {
                checked.clone()
            } else {
                if !visiting.insert(root.clone()) {
                    return Err(CliError::failure(format!(
                        "checked source provider dependency cycle at {}",
                        root.display()
                    )));
                }
                let checked = Self::check_source_dependency(
                    &root,
                    selected,
                    features,
                    sdk_profile_override,
                    completed,
                    visiting,
                )?;
                visiting.remove(&root);
                completed.insert(identity, checked.clone());
                checked
            };
            self.checked_source_metadata.extend(checked.source_metadata);
            checked_dependencies.push((key.clone(), checked.provider));
            let checked = checked.manifest;
            let metadata = LibraryArtifactMetadata::for_checked_source(&key, &checked.name, root);
            entries.insert(
                key,
                LibraryManifestIndexEntry::Loaded {
                    manifest: Box::new(checked),
                    metadata,
                },
            );
        }
        // Discovery already validated this catalog. Only checked source dependencies introduce new metadata;
        // an empty replacement set must keep the original plan and its exact authority owners.
        if checked_dependencies.is_empty() {
            tracing::trace!(target: "incan::invocation", decision = "reuse_unchanged_checked_provider_plan");
            return Ok(());
        }
        self.library_manifest_index = LibraryManifestIndex::from_entries(entries);
        add_selected_standard_vocab_providers(
            &mut self.library_manifest_index,
            self.sdk_inventory.as_deref(),
            self.sdk_components.as_ref(),
        )?;
        self.library_imported_vocab = self.library_manifest_index.library_imported_vocab();
        self.library_imported_dsl_surfaces = self.library_manifest_index.library_imported_dsl_surfaces();
        let mut plan = ProviderPlan::from_resolved_inputs(
            self.library_manifest_index.clone(),
            self.package_feature_plan.as_ref(),
            self.sdk_inventory.as_deref(),
            self.sdk_components.as_ref(),
            std::iter::empty(),
        )
        .map_err(|error| CliError::failure(error.to_string()))?
        .with_bootstrap_sdk_namespace_roots(self.provider_plan.bootstrap_sdk_namespace_roots().cloned());
        for (dependency, child) in checked_dependencies {
            plan.retain_checked_source_dependencies(&dependency, &child)
                .map_err(CliError::failure)?;
        }
        self.provider_plan = Arc::new(plan);
        self.provider_plans_by_modules = Arc::new(Mutex::new(BTreeMap::from([(
            BTreeSet::new(),
            Arc::clone(&self.provider_plan),
        )])));
        Ok(())
    }

    /// Prepare the CLI check path's Rust metadata context from this session's manifest and selected providers.
    /// Both diagnostics and native Body IR consumers must use this boundary before analysis so Rust signatures and
    /// derives are checked under the same defaults. The returned lease must remain alive through checking; programs
    /// without metadata queries need no workspace.
    #[cfg(feature = "rust_inspect")]
    pub fn prepare_check_rust_inspection(
        &self,
        entry_path: &Path,
        modules: &[ParsedModule],
    ) -> CliResult<Option<crate::lock::PreparedRustInspectTypecheckWorkspace>> {
        let project_root = resolve_project_root(entry_path);
        let provider_plan = self.provider_plan_for_modules(modules)?;
        let project_name = self
            .manifest
            .as_ref()
            .and_then(|manifest| manifest.project.as_ref().and_then(|project| project.name.clone()))
            .or_else(|| {
                entry_path
                    .file_stem()
                    .and_then(|stem| stem.to_str())
                    .map(ToString::to_string)
            })
            .unwrap_or_else(|| "incan_check".to_string());
        let cargo_features = oven_model::lock::CargoFeatureSelection::default().normalized();
        let cargo_policy = crate::cargo_policy::CargoPolicy::default();
        crate::lock::rust_inspect::prepare_rust_inspect_typecheck_workspace(crate::lock::RustInspectTypecheckRequest {
            project_root: &project_root,
            project_name: project_name.as_str(),
            manifest: self.manifest.as_ref(),
            modules,
            library_manifest_index: &self.library_manifest_index,
            cargo_features: &cargo_features,
            cargo_policy: &cargo_policy,
            rust_edition: self
                .manifest
                .as_ref()
                .and_then(|manifest| manifest.rust_edition().map(str::to_string)),
            provider_plan: &provider_plan,
        })
    }

    /// Discover project-level compilation context for an explicit Incan package-feature selection.
    pub fn discover_with_feature_selection(entry_path: &Path, feature_selection: &FeatureSelection) -> CliResult<Self> {
        Self::discover_with_selections(entry_path, feature_selection, None)
    }

    /// Discover project context for explicit package-feature and transient SDK-profile selections.
    pub fn discover_with_selections(
        entry_path: &Path,
        feature_selection: &FeatureSelection,
        sdk_profile_override: Option<&str>,
    ) -> CliResult<Self> {
        Self::discover_with_dependency_mode_and_sdk_source(
            entry_path,
            DependencyManifestMode::FullArtifacts,
            SdkInventorySource::PrepareNativeIfAbsent,
            feature_selection,
            sdk_profile_override,
        )
    }

    /// Discover project-level parsing context without preparing full dependency artifacts.
    pub fn discover_for_collection(entry_path: &Path) -> CliResult<Self> {
        Self::discover_for_collection_with_feature_selection(entry_path, &FeatureSelection::default())
    }

    /// Discover parser-only project context for an explicit Incan package-feature selection.
    pub fn discover_for_collection_with_feature_selection(
        entry_path: &Path,
        feature_selection: &FeatureSelection,
    ) -> CliResult<Self> {
        Self::discover_for_collection_with_selections(entry_path, feature_selection, None)
    }

    /// Discover parser-only project context for explicit package-feature and transient SDK-profile selections.
    ///
    /// Test collection is part of the normal Oven execution path, so a missing SDK inventory is an explicit
    /// preparation error rather than authorization to rebuild it through the former Cargo backend.
    pub fn discover_for_collection_with_selections(
        entry_path: &Path,
        feature_selection: &FeatureSelection,
        sdk_profile_override: Option<&str>,
    ) -> CliResult<Self> {
        Self::discover_with_dependency_mode_and_sdk_source(
            entry_path,
            DependencyManifestMode::ParserOnly,
            SdkInventorySource::DiscoverOnly,
            feature_selection,
            sdk_profile_override,
        )
    }

    /// Discover the semantic context for an Oven consumer without triggering any legacy Cargo publication.
    ///
    /// This permits an already installed or Oven-prepared SDK inventory, but a normal command must never respond to a
    /// cache miss by launching the old provider builder. The explicit `legacy_cargo` publisher owns that transition.
    pub fn discover_for_oven(
        entry_path: &Path,
        feature_selection: &FeatureSelection,
        sdk_profile_override: Option<&str>,
    ) -> CliResult<Self> {
        #[cfg(feature = "cli")]
        if let Some(session) =
            crate::build::library_project::discover_ordinary_dependency_session(entry_path, feature_selection)?
        {
            if sdk_profile_override.is_some() {
                return Err(CliError::failure(
                    "ordinary package admission does not accept an SDK profile override",
                ));
            }
            return Ok(session);
        }
        Self::discover_with_dependency_mode_and_sdk_source(
            entry_path,
            DependencyManifestMode::OvenArtifacts,
            SdkInventorySource::DiscoverOnly,
            feature_selection,
            sdk_profile_override,
        )
    }

    /// Construct an ordinary session from explicitly selected checked package owners, without SDK discovery.
    ///
    /// The caller must prepare every current root dependency and any reserved namespace grant before entering.
    /// Installed and source-backed packages use the same checked catalog. Missing component adapters refuse;
    /// native execution still requires separately selected current profiles and native plans.
    #[cfg(feature = "cli")]
    pub fn discover_with_admitted_library_dependencies(
        entry_path: &Path,
        feature_selection: &FeatureSelection,
        dependencies: Arc<PreparedLibraryDependencies>,
    ) -> CliResult<Self> {
        let inferred_root = resolve_project_root(entry_path);
        let manifest = discover_effective_project_manifest(&inferred_root)?
            .ok_or_else(|| CliError::failure("ordinary admitted session requires a project manifest"))?;
        let project_root = manifest.project_root().to_path_buf();
        let package_feature_plan = PackageFeaturePlan::resolve(&manifest, feature_selection)
            .map_err(|error| CliError::failure(error.to_string()))?;
        dependencies.validate_feature_plan(&package_feature_plan)?;
        let active_features = package_feature_plan
            .root_package()
            .ok_or_else(|| CliError::failure("ordinary admitted session has no root feature state"))?
            .features
            .active_features
            .clone();
        let declared_features = PackageFeatureGraph::from_manifest(&manifest)
            .map_err(|error| CliError::failure(error.to_string()))?
            .declared_features()
            .map(str::to_string)
            .collect();
        let provider_plan = Arc::clone(dependencies.provider_plan());
        let library_manifest_index = provider_plan.library_manifest_index().clone();
        let contract_model_bundles = read_project_model_bundles(&project_root, &manifest.contract_model_bundle_paths())
            .map_err(|error| CliError::failure(error.to_string()))?;
        Ok(Self {
            vocab_runtime: Arc::default(),
            source_root: resolve_source_root(&project_root, Some(&manifest)),
            manifest: Some(manifest),
            library_imported_vocab: library_manifest_index.library_imported_vocab(),
            library_imported_dsl_surfaces: library_manifest_index.library_imported_dsl_surfaces(),
            library_manifest_index,
            provider_plans_by_modules: Arc::new(Mutex::new(BTreeMap::from([(
                BTreeSet::new(),
                Arc::clone(&provider_plan),
            )]))),
            admitted_provider_plan: Some(Arc::clone(&provider_plan)),
            provider_plan,
            #[cfg(feature = "cli")]
            admitted_library_dependencies: Some(dependencies),
            checked_source_metadata: Vec::new(),
            provider_semantic_identities: Arc::new(
                incan_provider::lock_semantics::ProviderSemanticIdentitySession::default(),
            ),
            sdk_inventory: None,
            sdk_components: None,
            package_feature_plan: Some(package_feature_plan),
            active_features,
            declared_features,
            contract_model_bundles,
        })
    }

    /// Construct an ordinary standard-source publisher session with original dependency and own-source admissions.
    /// Own namespaces remain source authority only; dependency metadata/native routes are never fabricated.
    #[cfg(feature = "cli")]
    pub(crate) fn discover_with_admitted_standard_source(
        entry_path: &Path,
        feature_selection: &FeatureSelection,
        dependencies: Arc<PreparedLibraryDependencies>,
        source: Arc<incan_frontend::provider::source_policy::TrustedStandardSourcePublication>,
    ) -> CliResult<Self> {
        let mut session =
            Self::discover_with_admitted_library_dependencies(entry_path, feature_selection, dependencies)?;
        let manifest = session
            .manifest
            .as_ref()
            .ok_or_else(|| CliError::failure("standard source publication has no project manifest"))?;
        let project = manifest
            .project
            .as_ref()
            .ok_or_else(|| CliError::failure("standard source publication has no package identity"))?;
        let name = project
            .name
            .as_deref()
            .ok_or_else(|| CliError::failure("standard source package has no name"))?;
        let version = project
            .version
            .as_deref()
            .ok_or_else(|| CliError::failure("standard source package has no version"))?;
        let plan = session
            .provider_plan
            .as_ref()
            .clone()
            .with_standard_source_publication(source, manifest.project_root(), name, version)
            .map_err(CliError::failure)?;
        session.provider_plan = Arc::new(plan);
        session.admitted_provider_plan = Some(Arc::clone(&session.provider_plan));
        session.provider_plans_by_modules = Arc::new(Mutex::new(BTreeMap::from([(
            BTreeSet::new(),
            Arc::clone(&session.provider_plan),
        )])));
        Ok(session)
    }

    /// Construct a publisher session from the partial inventory and explicit reserved namespace grants.
    ///
    /// The caller retains native receipts and frozen inspection authority. Discovery never prepares another SDK
    /// generation or relies on a process-global bootstrap marker while checking this component.
    pub fn discover_for_native_sdk_component(
        entry_path: &Path,
        inventory: &SdkInventory,
        namespace_roots: &BTreeSet<String>,
        native_facets: &BTreeSet<String>,
    ) -> CliResult<Self> {
        Self::discover_with_inputs(
            entry_path,
            DependencyManifestMode::ParserOnly,
            SdkInventorySource::DiscoverOnly,
            &FeatureSelection::default(),
            None,
            Some((inventory, namespace_roots, native_facets)),
        )
    }

    /// Discover project context with either full dependency artifacts or parser-only dependency metadata.
    fn discover_with_dependency_mode_and_sdk_source(
        entry_path: &Path,
        dependency_mode: DependencyManifestMode,
        sdk_source: SdkInventorySource,
        feature_selection: &FeatureSelection,
        sdk_profile_override: Option<&str>,
    ) -> CliResult<Self> {
        Self::discover_with_inputs(
            entry_path,
            dependency_mode,
            sdk_source,
            feature_selection,
            sdk_profile_override,
            None,
        )
    }

    /// Resolve ordinary session inputs or consume the publisher's explicit component context.
    fn discover_with_inputs(
        entry_path: &Path,
        dependency_mode: DependencyManifestMode,
        sdk_source: SdkInventorySource,
        feature_selection: &FeatureSelection,
        sdk_profile_override: Option<&str>,
        native_sdk: Option<(&SdkInventory, &BTreeSet<String>, &BTreeSet<String>)>,
    ) -> CliResult<Self> {
        let _invocation = tracing::trace_span!(target: "incan::invocation", "incan_driver::discover_session", input = %entry_path.display()).entered();
        let mut phase =
            tracing::trace_span!(target: "incan::invocation", "incan_driver::session_stage", stage = "manifest")
                .entered();
        let inferred_project_root = resolve_project_root(entry_path);
        let manifest = discover_effective_project_manifest(&inferred_project_root)?;
        let manifest = match (manifest, native_sdk) {
            (Some(manifest), Some((_, _, facets))) => Some(
                manifest.with_effective_dependencies(
                    manifest
                        .library_dependencies()
                        .iter()
                        .filter(|(name, _)| !facets.contains(name.as_str()))
                        .map(|(name, spec)| (name.clone(), spec.clone())),
                    manifest
                        .rust_dependencies()
                        .iter()
                        .map(|(name, spec)| (name.clone(), spec.clone())),
                    manifest
                        .rust_dev_dependencies()
                        .iter()
                        .map(|(name, spec)| (name.clone(), spec.clone())),
                ),
            ),
            (manifest, _) => manifest,
        };
        let project_root = manifest
            .as_ref()
            .map(|manifest| manifest.project_root().to_path_buf())
            .unwrap_or(inferred_project_root);
        let source_root = resolve_source_root(&project_root, manifest.as_ref());
        drop(phase);
        phase = tracing::trace_span!(target: "incan::invocation", "incan_driver::session_stage", stage = "inventory")
            .entered();
        let sdk_inventory = if let Some((inventory, _, _)) = native_sdk {
            Some(Arc::new(inventory.clone()))
        } else {
            match sdk_source {
                #[cfg(feature = "cli")]
                SdkInventorySource::PrepareNativeIfAbsent => {
                    crate::build::native_sdk::prepare_or_discover_sdk_inventory()?
                }
                // Without the build pipeline (the `cli` feature, #1479) nothing can prepare providers, so a session
                // reuses the inventory a toolchain command published, as an Oven command does.
                #[cfg(not(feature = "cli"))]
                SdkInventorySource::PrepareNativeIfAbsent => discover_active_sdk_inventory()?,
                // An Oven command never builds the providers, but it reuses the inventory `incan check` published for a
                // source checkout, so both parse a file with the same standard-library vocabulary (#1774).
                SdkInventorySource::DiscoverOnly => discover_active_sdk_inventory()?,
            }
        };
        drop(phase);
        phase = tracing::trace_span!(target: "incan::invocation", "incan_driver::session_stage", stage = "features")
            .entered();
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
        drop(phase);
        phase = tracing::trace_span!(target: "incan::invocation", "incan_driver::session_stage", stage = "library_dependencies").entered();
        if let Some(manifest) = manifest.as_ref()
            && let Some(preparation) = dependency_mode.library_dependency_preparation()
        {
            prepare_library_dependency_artifacts(
                manifest,
                package_feature_plan.as_ref(),
                &active_dependencies,
                preparation,
            )?;
        }
        let mut library_manifest_index = match (manifest.as_ref(), dependency_mode) {
            (Some(manifest), mode) if mode.uses_materialized_library_index() && !active_dependencies.is_empty() => {
                LibraryManifestIndex::from_project_manifest_dependencies(
                    manifest,
                    active_dependencies.iter().map(String::as_str),
                )
            }
            (Some(manifest), DependencyManifestMode::CheckMetadata) if !active_dependencies.is_empty() => {
                checked_source_library_manifest_index(manifest, &active_dependencies)?
            }
            (Some(manifest), DependencyManifestMode::ParserOnly) if !active_dependencies.is_empty() => {
                let preference = if native_sdk.is_some() {
                    SourceLibraryManifestPreference::SourceDeclarations
                } else {
                    SourceLibraryManifestPreference::PublishedArtifacts
                };
                parser_only_library_manifest_index(manifest, &active_dependencies, preference)?
            }
            _ => LibraryManifestIndex::default(),
        };
        drop(phase);
        phase =
            tracing::trace_span!(target: "incan::invocation", "incan_driver::session_stage", stage = "model_bundles")
                .entered();
        let contract_model_bundles = manifest
            .as_ref()
            .map(|manifest| read_project_model_bundles(&project_root, &manifest.contract_model_bundle_paths()))
            .transpose()
            .map_err(|error| CliError::failure(error.to_string()))?
            .unwrap_or_default();
        // Collection and execution must resolve the same SDK catalog. In a source checkout there is no installed
        // inventory to discover, so a session that neither published nor reused the checkout's inventory silently fell
        // back to the legacy monolithic stdlib. Besides losing component-aware diagnostics and the standard library's
        // vocabulary, that made a transient SDK profile fail during collection and allowed the lock projection to
        // drift before execution prepared the artifacts. Publication is content-addressed: the check path publishes
        // or reuses it, the Oven and collection paths reuse it only, and parser-only mode still avoids preparing
        // ordinary dependencies.
        drop(phase);
        phase = tracing::trace_span!(target: "incan::invocation", "incan_driver::session_stage", stage = "component_selection").entered();
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
        drop(phase);
        phase =
            tracing::trace_span!(target: "incan::invocation", "incan_driver::session_stage", stage = "standard_vocab")
                .entered();
        add_selected_standard_vocab_providers(
            &mut library_manifest_index,
            sdk_inventory.as_deref(),
            sdk_components.as_ref(),
        )?;
        let library_imported_vocab = library_manifest_index.library_imported_vocab();
        let library_imported_dsl_surfaces = library_manifest_index.library_imported_dsl_surfaces();
        let bootstrap_sdk_namespace_roots = match native_sdk {
            Some((_, roots, _)) => roots.clone(),
            None => sdk_provider_bootstrap_namespace_roots(&project_root)?,
        };
        drop(phase);
        phase = tracing::trace_span!(target: "incan::invocation", "incan_driver::session_stage", stage = "base_provider_plan").entered();
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

        drop(phase);
        Ok(Self {
            vocab_runtime: Arc::default(),
            manifest,
            source_root,
            library_manifest_index,
            provider_plan,
            provider_plans_by_modules,
            #[cfg(feature = "cli")]
            admitted_library_dependencies: None,
            admitted_provider_plan: None,
            checked_source_metadata: Vec::new(),
            provider_semantic_identities: Arc::new(
                incan_provider::lock_semantics::ProviderSemanticIdentitySession::default(),
            ),
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

    /// Bind publication edges to this transaction's already compiled SDK artifacts while keeping semantic
    /// provider selection unchanged. A public dependency and a reserved SDK import may denote the same physical
    /// owner; replacing the semantic catalog would incorrectly grant that owner two namespace identities.
    pub(crate) fn native_sdk_publication_dependency_index(
        &self,
        inventory: &SdkInventory,
        catalog: &incan_provider::SdkSourceCatalog,
    ) -> CliResult<LibraryManifestIndex> {
        let project = self
            .manifest
            .as_ref()
            .ok_or_else(|| CliError::failure("SDK session has no project declaration"))?;
        let published = published_sdk_dependency_entries(project, inventory, catalog)?;
        Ok(LibraryManifestIndex::from_entries(
            self.library_manifest_index
                .known_libraries()
                .into_iter()
                .filter_map(|key| {
                    published
                        .get(&key)
                        .or_else(|| self.library_manifest_index.get(&key))
                        .cloned()
                        .map(|entry| (key, entry))
                })
                .collect(),
        ))
    }

    /// Borrow the validated catalog shared by this session's module-use projections.
    pub fn provider_plan(&self) -> &Arc<ProviderPlan> {
        &self.provider_plan
    }

    /// Borrow the catalog selected when this session was constructed; callers cannot substitute its authority.
    pub fn sdk_inventory(&self) -> &Option<Arc<SdkInventory>> {
        &self.sdk_inventory
    }

    /// Borrow the component selection that was validated with the original provider catalog.
    pub fn sdk_components(&self) -> &Option<ResolvedSdkComponents> {
        &self.sdk_components
    }

    /// Borrow the feature closure that determined this session's dependency and provider selection.
    pub fn package_feature_plan(&self) -> &Option<PackageFeaturePlan> {
        &self.package_feature_plan
    }

    /// Deliberately corrupt private authority for existing adversarial controls; unavailable in production.
    #[cfg(test)]
    pub(crate) fn replace_provider_plan_for_test(&mut self, plan: Arc<ProviderPlan>) {
        self.provider_plan = plan;
    }

    /// Borrow the ordinary original-owner capability retained by an explicitly admitted dependency session.
    ///
    /// Compatibility discovery sessions return none. Publication and planning callers must not fabricate a checked
    /// owner for those sessions or infer that absent authority means an authenticated empty dependency closure.
    #[cfg(feature = "cli")]
    pub fn admitted_library_dependencies(&self) -> Option<&Arc<PreparedLibraryDependencies>> {
        self.admitted_library_dependencies.as_ref()
    }

    /// Whether explicitly admitted ordinary dependencies drive this session. Without the build pipeline (the `cli`
    /// feature, #1479) no session can be admitted that way, so it is always a discovery session.
    pub(crate) fn has_admitted_library_dependencies(&self) -> bool {
        #[cfg(feature = "cli")]
        {
            self.admitted_library_dependencies.is_some()
        }
        #[cfg(not(feature = "cli"))]
        {
            false
        }
    }

    /// Borrow the original ordinary provider plan, refusing a public replacement or mixed SDK discovery state.
    #[cfg(feature = "cli")]
    pub(crate) fn original_admitted_provider_plan(&self) -> CliResult<&Arc<ProviderPlan>> {
        let original = self
            .admitted_provider_plan
            .as_ref()
            .ok_or_else(|| CliError::failure("explicit ordinary session has no original provider authority"))?;
        if self.admitted_library_dependencies.is_none()
            || self.sdk_inventory.is_some()
            || self.sdk_components.is_some()
            || !Arc::ptr_eq(&self.provider_plan, original)
        {
            return Err(CliError::failure(
                "explicit ordinary session contains competing provider authority",
            ));
        }
        original
            .verify_standard_source_publication()
            .map_err(CliError::failure)?;
        Ok(original)
    }

    /// Return provider semantic identities after revalidating their exact physical and dependency context.
    pub fn provider_semantic_identities(
        &self,
        provider_plan: &ProviderPlan,
        sdk_path_dependencies: &[oven_model::manifest::DependencySpec],
    ) -> CliResult<Arc<incan_provider::lock_semantics::CheckedProviderSemanticIdentities>> {
        let _trace =
            tracing::trace_span!(target: "incan::invocation", "incan_driver::session_provider_identities").entered();
        self.provider_semantic_identities
            .identities(provider_plan, sdk_path_dependencies)
            .map_err(CliError::failure)
    }

    /// Resolve module participation from this session's immutable provider, feature, and SDK inputs.
    pub fn provider_plan_for_modules(&self, modules: &[ParsedModule]) -> CliResult<Arc<ProviderPlan>> {
        self.provider_plan_for_used_module_paths(self.provider_module_paths(modules))
    }

    /// Collect this session's module-use facts without confusing ordinary namespace use with legacy core linkage.
    ///
    /// Admitted ordinary dependencies require exact source use. Discovered legacy sessions retain their existing
    /// synthetic prelude link requirement. Explicit prelude imports are preserved by both routes.
    pub(crate) fn provider_module_paths(&self, modules: &[ParsedModule]) -> BTreeSet<Vec<String>> {
        if self.has_admitted_library_dependencies() {
            provider_source_used_module_paths(modules)
        } else {
            provider_used_module_paths(modules)
        }
    }

    /// Resolve one provider projection from canonical module paths while retaining the session's authority snapshot.
    ///
    /// Callers with nested source constructs can derive their complete module-use set once, then enter the same cache
    /// as ordinary compilation without rebuilding SDK, package-feature, or library-manifest inputs.
    pub fn provider_plan_for_used_module_paths(
        &self,
        used_module_paths: BTreeSet<Vec<String>>,
    ) -> CliResult<Arc<ProviderPlan>> {
        let _invocation = tracing::trace_span!(target: "incan::invocation", "incan_driver::project_provider_plan", module_paths = used_module_paths.len()).entered();
        let authority_trace =
            tracing::trace_span!(target: "incan::invocation", "incan_driver::verify_provider_authority").entered();
        self.provider_plan
            .verify_standard_source_publication()
            .map_err(CliError::failure)?;
        #[cfg(feature = "cli")]
        if let Some(dependencies) = &self.admitted_library_dependencies {
            self.original_admitted_provider_plan()?;
            dependencies.verify()?;
            let dependency_modules = used_module_paths
                .iter()
                .filter(|module| {
                    self.provider_plan.standard_source_publication().is_none()
                        || !self.provider_plan.bootstrap_owns_sdk_module(module)
                })
                .cloned()
                .collect();
            dependencies.validate_module_usage(&dependency_modules)?;
        }
        drop(authority_trace);
        if let Some(plan) = self
            .provider_plans_by_modules
            .lock()
            .map_err(|_| CliError::failure("provider-plan cache lock was poisoned"))?
            .get(&used_module_paths)
            .cloned()
        {
            tracing::trace!(target: "incan::invocation", decision = "reuse_provider_projection");
            return Ok(plan);
        }
        let _projection =
            tracing::trace_span!(target: "incan::invocation", "incan_driver::construct_provider_projection").entered();
        // The session alone owns catalog inputs. Module use changes the projection, never the catalog.
        let plan = Arc::new(self.provider_plan.project_module_usage(used_module_paths.clone()));
        let mut cached = self
            .provider_plans_by_modules
            .lock()
            .map_err(|_| CliError::failure("provider-plan cache lock was poisoned"))?;
        Ok(cached.entry(used_module_paths).or_insert(plan).clone())
    }

    /// Return the number of provider projections cached by this compilation session.
    #[cfg(test)]
    pub fn provider_plan_cache_entry_count(&self) -> CliResult<usize> {
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
    pub fn analyze_modules(
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
    pub fn testing_marker_semantics(&self) -> CliResult<Option<TestingMarkerSemantics>> {
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
    pub fn require_testing_marker_semantics(&self) -> CliResult<TestingMarkerSemantics> {
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
    pub fn declared_crate_names(&self) -> HashSet<String> {
        self.manifest
            .as_ref()
            .map(ProjectManifest::declared_rust_crate_names)
            .unwrap_or_default()
    }

    /// Lex and parse one source file using the project-aware vocabulary surfaces, without running desugarers or
    /// compile-time materialization passes.
    ///
    /// Always parses with the original `source` text available (RFC 081, `#1023`), so a descriptor-gated embedded
    /// fragment (`loaves/kernel/incan_syntax/src/parser/embedded/`) can claim eligible positions in every real
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
    pub fn parse_source_for_collection(
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
    pub fn parse_source(
        &self,
        file_path: &Path,
        source: &str,
        materialize_models: bool,
    ) -> Result<Program, Vec<diagnostics::CompileError>> {
        let parsed = self.parse_source_unprojected(file_path, source, materialize_models)?;
        self.project_parsed_program(parsed)
    }

    /// Parse and desugar one source file while retaining inactive compile-time feature declarations for tooling.
    pub fn parse_source_unprojected(
        &self,
        file_path: &Path,
        source: &str,
        materialize_models: bool,
    ) -> Result<Program, Vec<diagnostics::CompileError>> {
        let _trace = tracing::trace_span!(target: "incan::invocation", "incan_driver::parse_desugar_source", input = %file_path.display()).entered();
        let parsed = {
            let _parse = tracing::trace_span!(target: "incan::invocation", "incan_driver::parse_source", input = %file_path.display()).entered();
            self.parse_source_for_collection(file_path, source)?
        };
        let mut ast = parsed;
        let file_path_display = file_path.to_string_lossy();
        {
            let mut runtime = self.vocab_runtime.lock().map_err(|_| {
                vec![diagnostics::CompileError::new(
                    "vocabulary runtime owner is poisoned".to_string(),
                    Span::default(),
                )]
            })?;
            vocab_desugar_pass::desugar_program_vocab_blocks_with_runtime(
                &mut ast,
                Some(file_path_display.as_ref()),
                &self.library_manifest_index,
                &mut runtime,
            )?;
        }
        if materialize_models && let Err(error) = materialize_contract_models(&mut ast, &self.contract_model_bundles) {
            return Err(vec![diagnostics::CompileError::new(
                format!("Invalid checked contract metadata: {error}"),
                Span::default(),
            )]);
        }
        Ok(ast)
    }

    /// Validate and project one already-parsed source program through this session's active package features.
    pub fn project_parsed_program(&self, parsed: Program) -> Result<Program, Vec<diagnostics::CompileError>> {
        self.validate_parsed_program_features(&parsed)?;
        Ok(parsed.projected_for_features(&self.active_features))
    }

    /// Validate compile-time feature names while retaining the complete unprojected source program.
    pub fn validate_parsed_program_features(&self, parsed: &Program) -> Result<(), Vec<diagnostics::CompileError>> {
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
        declaration: &incan_frontend::ast::Spanned<incan_frontend::ast::Declaration>,
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
        if let incan_frontend::ast::Declaration::TestModule(module) = &declaration.node {
            for nested in &module.body {
                self.validate_declaration_feature_requirements(nested, errors);
            }
        }
    }
}

#[cfg(test)]
mod source_publication_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use incan_frontend::library_manifest::{LibraryManifest, VocabExports};
    use incan_provider::test_support::{parsed_module_for_test, write_minimal_library_artifact};

    /// Ordinary Core artifacts must not change a native publisher's source semantic plan or publication authority.
    #[test]
    fn native_sdk_component_ignores_ordinary_core_artifact_for_source_metadata()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        std::fs::create_dir_all(root.path().join("src"))?;
        std::fs::create_dir_all(root.path().join("deps/core/src"))?;
        std::fs::write(
            root.path().join("loaf.toml"),
            "[project]\nname='publisher'\nversion='0.1.0'\n[dependencies]\nincan_stdlib_core={path='deps/core'}\n",
        )?;
        std::fs::write(
            root.path().join("deps/core/loaf.toml"),
            "[project]\nname='incan_stdlib_core'\nversion='0.1.0'\n",
        )?;
        std::fs::write(
            root.path().join("deps/core/src/lib.incn"),
            "pub def value() -> int:\n    return 42\n",
        )?;
        let entry = root.path().join("src/lib.incn");
        std::fs::write(&entry, "pub def value() -> int:\n    return 42\n")?;
        let manifest = ProjectManifest::load(&root.path().join("loaf.toml"))?;
        let active = BTreeSet::from(["incan_stdlib_core".to_string()]);
        let inventory = SdkInventory {
            root: root.path().to_path_buf(),
            sdk_id: "incan".to_string(),
            sdk_version: "0.1.0".to_string(),
            compiler_requirement: "*".to_string(),
            provider_codegen_revision: incan_lang::version::SDK_PROVIDER_CODEGEN_REVISION,
            components: BTreeMap::new(),
            profiles: BTreeMap::from([("default".to_string(), BTreeSet::new())]),
        };
        let discover = || {
            CompilationSession::discover_for_native_sdk_component(
                &entry,
                &inventory,
                &BTreeSet::new(),
                &BTreeSet::new(),
            )
        };
        let cold = discover()?;
        let cold_manifest = cold
            .library_manifest_index
            .loaded_entries()
            .find_map(|(key, manifest, _)| (key == "incan_stdlib_core").then_some(manifest))
            .ok_or("cold source manifest absent")?;
        write_minimal_library_artifact(
            root.path(),
            "core",
            "incan_stdlib_core",
            &LibraryManifest::new("incan_stdlib_core", "0.1.0"),
        )?;
        let artifact = root.path().join("deps/core/target/lib/incan_stdlib_core.incnlib");
        let original_bytes = std::fs::read(&artifact)?;
        let ordinary = parser_only_library_manifest_index(
            &manifest,
            &active,
            SourceLibraryManifestPreference::PublishedArtifacts,
        )?;
        assert_eq!(
            ordinary
                .loaded_artifact("incan_stdlib_core")
                .ok_or("ordinary Core absent")?
                .kind,
            incan_frontend::library_manifest_index::LibraryArtifactKind::Materialized
        );
        let warm = discover()?;
        assert_eq!(
            warm.library_manifest_index
                .loaded_artifact("incan_stdlib_core")
                .ok_or("warm source Core absent")?
                .kind,
            incan_frontend::library_manifest_index::LibraryArtifactKind::ParserSource
        );
        let warm_manifest = warm
            .library_manifest_index
            .loaded_entries()
            .find_map(|(key, manifest, _)| (key == "incan_stdlib_core").then_some(manifest))
            .ok_or("warm source manifest absent")?;
        assert_eq!(warm_manifest, cold_manifest);
        assert_eq!(std::fs::read(&artifact)?, original_bytes);
        std::fs::remove_file(root.path().join("deps/core/loaf.toml"))?;
        let artifact_only = parser_only_library_manifest_index(
            &manifest,
            &active,
            SourceLibraryManifestPreference::SourceDeclarations,
        )?;
        assert_eq!(
            artifact_only
                .loaded_artifact("incan_stdlib_core")
                .ok_or("artifact-only Core absent")?
                .kind,
            incan_frontend::library_manifest_index::LibraryArtifactKind::Materialized
        );
        Ok(())
    }

    /// Reproduce SDK data publication against the genuine already published Core artifact.
    #[test]
    #[ignore = "requires an explicitly selected matching native SDK publication"]
    fn native_sdk_component_uses_published_public_dependencies() -> Result<(), Box<dyn std::error::Error>> {
        let inventory = discover_active_sdk_inventory()?.ok_or("matching SDK required")?;
        let stdlib = oven_model::toolchain_layout::find_stdlib_root().ok_or("stdlib source required")?;
        let catalog =
            incan_provider::SdkSourceCatalog::read_from_path(&stdlib.join(incan_provider::SDK_SOURCE_CATALOG_FILE))?;
        let data = catalog.components.get("stdlib-data").ok_or("data component missing")?;
        let session = CompilationSession::discover_for_native_sdk_component(
            &data.project_root.join("src/lib.incn"),
            &inventory,
            &data.namespace_roots,
            &BTreeSet::new(),
        )?;
        let semantic_plan = Arc::clone(session.provider_plan());
        let published = session.native_sdk_publication_dependency_index(&inventory, &catalog)?;
        let metadata = published
            .loaded_artifact("incan_stdlib_core")
            .ok_or("Core dependency absent")?;
        assert_eq!(
            metadata.kind,
            incan_frontend::library_manifest_index::LibraryArtifactKind::Materialized
        );
        let core = inventory.components.get("stdlib-core").ok_or("Core component absent")?;
        assert_eq!(
            Some(metadata.crate_root.as_path()),
            core.providers
                .first()
                .ok_or("Core provider absent")?
                .crate_root
                .as_deref()
        );
        assert!(Arc::ptr_eq(&semantic_plan, session.provider_plan()));
        assert_eq!(
            session
                .library_manifest_index
                .loaded_artifact("incan_stdlib_core")
                .ok_or("source Core absent")?
                .kind,
            incan_frontend::library_manifest_index::LibraryArtifactKind::ParserSource
        );

        let mut changed = (*inventory).clone();
        changed
            .components
            .get_mut("stdlib-core")
            .ok_or("Core component absent")?
            .providers
            .first_mut()
            .ok_or("Core provider absent")?
            .digest
            .push_str("-foreign");
        assert!(
            session
                .native_sdk_publication_dependency_index(&changed, &catalog)
                .is_err()
        );
        let mut competing = catalog.clone();
        let duplicate = competing
            .components
            .get("stdlib-core")
            .ok_or("Core component absent")?
            .clone();
        competing.components.insert("competing-core".to_string(), duplicate);
        assert!(
            session
                .native_sdk_publication_dependency_index(&inventory, &competing)
                .is_err()
        );
        let foreign = tempfile::tempdir()?;
        let mut foreign_catalog = catalog.clone();
        foreign_catalog
            .components
            .get_mut("stdlib-core")
            .ok_or("Core component absent")?
            .project_root = foreign.path().to_path_buf();
        let foreign_index = session.native_sdk_publication_dependency_index(&inventory, &foreign_catalog)?;
        assert_eq!(
            foreign_index
                .loaded_artifact("incan_stdlib_core")
                .ok_or("source dependency absent")?
                .kind,
            incan_frontend::library_manifest_index::LibraryArtifactKind::ParserSource
        );
        Ok(())
    }

    /// A metadata check without source dependencies keeps the already validated provider catalog and projection cache.
    #[test]
    fn metadata_check_preserves_catalog_without_source_dependency_replacements()
    -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        std::fs::create_dir_all(tmp.path().join("src"))?;
        std::fs::write(
            tmp.path().join("loaf.toml"),
            "[project]\nname = \"checked_catalog\"\nversion = \"0.1.0\"\n",
        )?;
        let entry = tmp.path().join("src/main.incn");
        std::fs::write(&entry, "def main() -> None:\n    pass\n")?;
        let mut session = CompilationSession::discover_with_dependency_mode_and_sdk_source(
            &entry,
            DependencyManifestMode::CheckMetadata,
            SdkInventorySource::DiscoverOnly,
            &FeatureSelection::default(),
            None,
        )?;
        let original = Arc::clone(&session.provider_plan);
        let projections = Arc::clone(&session.provider_plans_by_modules);
        let features = session
            .package_feature_plan
            .clone()
            .ok_or("project feature plan missing")?;
        session.check_source_dependencies(&features, None, &mut BTreeMap::new(), &mut BTreeSet::new())?;
        assert!(Arc::ptr_eq(&original, &session.provider_plan));
        assert!(Arc::ptr_eq(&projections, &session.provider_plans_by_modules));
        Ok(())
    }

    #[test]
    fn compilation_session_reuses_provider_plan_for_identical_module_usage() -> Result<(), Box<dyn std::error::Error>> {
        let library_manifest_index = LibraryManifestIndex::default();
        let provider_plan = Arc::new(ProviderPlan::default());
        let session = CompilationSession {
            vocab_runtime: Arc::default(),
            manifest: None,
            source_root: PathBuf::from("fixture"),
            library_imported_vocab: library_manifest_index.library_imported_vocab(),
            library_imported_dsl_surfaces: library_manifest_index.library_imported_dsl_surfaces(),
            library_manifest_index,
            provider_plans_by_modules: Arc::new(Mutex::new(BTreeMap::from([(
                BTreeSet::new(),
                Arc::clone(&provider_plan),
            )]))),
            #[cfg(feature = "cli")]
            admitted_library_dependencies: None,
            admitted_provider_plan: None,
            checked_source_metadata: Vec::new(),
            provider_semantic_identities: Arc::new(
                incan_provider::lock_semantics::ProviderSemanticIdentitySession::default(),
            ),
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

    /// The Oven session reuses the SDK inventory the check session published for a source checkout (#1774).
    ///
    /// `incan check` builds its session with [`SdkInventorySource::PrepareNativeIfAbsent`] and publishes the
    /// checkout's component providers; `incan run`, `build` and `oven bake` build theirs with
    /// [`SdkInventorySource::DiscoverOnly`] and never build providers. Before #1774 the Oven session found no
    /// inventory in a checkout and parsed without the standard library's vocabulary. The suite exports
    /// `INCAN_SDK_INVENTORY`, which would hide the difference, so the sessions are discovered in a child process
    /// with that override removed and a synthetic checkout (an empty component catalog) as the only source of an
    /// inventory: nothing before the check session publishes, and exactly the published inventory after.
    #[test]
    fn oven_session_reuses_the_inventory_the_check_session_published_issue1774()
    -> Result<(), Box<dyn std::error::Error>> {
        const CHILD: &str = "INCAN_TEST_1774_SESSION_CHILD";
        const ENTRY: &str = "INCAN_TEST_1774_ENTRY";
        if std::env::var_os(CHILD).is_some() {
            let entry = PathBuf::from(std::env::var_os(ENTRY).ok_or("the child needs its entry path")?);
            let selection = FeatureSelection::default();
            let before = CompilationSession::discover_for_oven(&entry, &selection, None)?;
            if let Some(inventory) = before.sdk_inventory.as_ref() {
                return Err(format!("nothing is published yet, found {}", inventory.root.display()).into());
            }
            let check = CompilationSession::discover_with_selections(&entry, &selection, None)?;
            let published = check
                .sdk_inventory
                .as_ref()
                .ok_or("the check session publishes the checkout's inventory")?
                .root
                .clone();
            let after = CompilationSession::discover_for_oven(&entry, &selection, None)?;
            let reused = after
                .sdk_inventory
                .as_ref()
                .ok_or("the Oven session must reuse the published inventory")?
                .root
                .clone();
            assert_eq!(
                reused, published,
                "the Oven session reads the inventory the check session published"
            );
            return Ok(());
        }

        // ---- A synthetic checkout with native companions and an empty component catalog ----
        let tmp = tempfile::tempdir()?;
        let checkout = tmp.path().join("checkout");
        let stdlib_root = checkout.join("loaves/stdlib");
        std::fs::create_dir_all(checkout.join("loaves/compiler/incan_emit/src"))?;
        std::fs::create_dir_all(&stdlib_root)?;
        write_session_native_sdk_fixture(&checkout)?;
        std::fs::write(
            stdlib_root.join(incan_provider::SDK_SOURCE_CATALOG_FILE),
            format!(
                "[sdk]\nid = \"incan\"\nversion = \"{version}\"\ncompiler-requirement = \"={version}\"\n[profiles]\ndefault = []\nfull = []\n[components]\n",
                version = incan_lang::version::INCAN_VERSION,
            ),
        )?;
        let project = tmp.path().join("project");
        std::fs::create_dir_all(&project)?;
        let entry = project.join("main.incn");
        std::fs::write(&entry, "def main() -> None:\n    pass\n")?;
        let store = tmp.path().join("store");

        let current_exe = std::env::current_exe()?;
        let child = std::process::Command::new(&current_exe)
            .args([
                "--exact",
                "session::tests::oven_session_reuses_the_inventory_the_check_session_published_issue1774",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .env(ENTRY, &entry)
            .env("INCAN_STDLIB", &stdlib_root)
            .env("INCAN_STDLIB_DIR", &stdlib_root)
            .env("INCAN_SOURCE_ROOT", &checkout)
            .env(incan_provider::sdk_store::INTERNAL_SDK_PROVIDER_STORE_ENV, &store)
            .env("INCAN_SDK_NATIVE_BLOBS", &checkout)
            .env("INCAN_SDK_NATIVE_INDEX", &checkout)
            // The fixture executable contributes identity; native preparation never launches it.
            .env("CARGO_BIN_EXE_incan", &current_exe)
            .env_remove(incan_provider::inventory::SDK_INVENTORY_OVERRIDE_ENV)
            .env_remove(incan_provider::sdk_store::INTERNAL_SDK_PROVIDER_PATH_FILE_ENV)
            .env_remove(incan_provider::sdk_store::INTERNAL_SDK_DISTRIBUTION_PROFILE_ENV)
            .env_remove(incan_provider::SDK_PROVIDER_BUILD_ENV)
            .output()?;
        let stdout = String::from_utf8_lossy(&child.stdout);
        assert!(
            child.status.success(),
            "the child session check failed:\n{stdout}\n{}",
            String::from_utf8_lossy(&child.stderr)
        );
        assert!(
            stdout.contains("1 passed"),
            "the child must run exactly this test, not an empty filter:\n{stdout}"
        );
        Ok(())
    }

    /// Supply receipt-bound native companions for the discovery regression without registry resolution.
    fn write_session_native_sdk_fixture(checkout: &Path) -> Result<(), Box<dyn std::error::Error>> {
        std::fs::write(
            checkout.join("loaves/stdlib/sdk-lock.json"),
            r#"{"schema":"incan.oven.loaf-resolution/1","units":[]}"#,
        )?;
        for (relative, name, kind) in [
            ("loaves/kernel/incan_lang", "fixture_lang", "lib"),
            ("loaves/kernel/incan_vocab", "fixture_vocab", "lib"),
            ("loaves/stdlib/derive/incan_derive", "fixture_derive", "proc-macro"),
            (
                "loaves/stdlib/derive/incan_web_macros",
                "fixture_web_macros",
                "proc-macro",
            ),
        ] {
            let root = checkout.join(relative);
            std::fs::create_dir_all(root.join("src"))?;
            std::fs::write(
                root.join("loaf.toml"),
                format!(
                    "[project]\nname='{name}'\nversion='1.0.0'\n[rust]\nname='{name}'\ntype='{kind}'\nedition='2024'\n"
                ),
            )?;
            std::fs::write(
                root.join("src/lib.rs"),
                if kind == "proc-macro" {
                    "extern crate proc_macro; #[proc_macro] pub fn identity(input: proc_macro::TokenStream) -> proc_macro::TokenStream { input }"
                } else {
                    "pub fn value() -> u8 { 1 }"
                },
            )?;
            std::fs::write(root.join("Cargo.toml"), "poisoned metadata")?;
            std::fs::write(root.join("Cargo.lock"), "poisoned lock")?;
            std::fs::write(root.join("build.rs"), "compile_error!(\"must remain inert\");")?;
        }
        Ok(())
    }

    /// A session with no SDK inventory, as a consumer on a fresh home has: every `std` module is collected from its
    /// source under the generated `__incan_std` namespace and checked with the consumer's modules.
    fn session_without_sdk_inventory(project: &Path) -> CompilationSession {
        let library_manifest_index = LibraryManifestIndex::default();
        let provider_plan = Arc::new(ProviderPlan::default());
        CompilationSession {
            vocab_runtime: Arc::default(),
            manifest: None,
            source_root: project.join("src"),
            library_imported_vocab: library_manifest_index.library_imported_vocab(),
            library_imported_dsl_surfaces: library_manifest_index.library_imported_dsl_surfaces(),
            library_manifest_index,
            provider_plans_by_modules: Arc::new(Mutex::new(BTreeMap::from([(
                BTreeSet::new(),
                Arc::clone(&provider_plan),
            )]))),
            #[cfg(feature = "cli")]
            admitted_library_dependencies: None,
            admitted_provider_plan: None,
            checked_source_metadata: Vec::new(),
            provider_semantic_identities: Arc::new(
                incan_provider::lock_semantics::ProviderSemanticIdentitySession::default(),
            ),
            provider_plan,
            sdk_inventory: None,
            sdk_components: None,
            package_feature_plan: None,
            active_features: BTreeSet::new(),
            declared_features: BTreeSet::new(),
            contract_model_bundles: Vec::new(),
        }
    }

    /// Seed the inspected `std::io::Read` methods into a sysroot-only Loaf inspection workspace.
    /// Unseeded lookups use the existing native inspection path; the fixture must never discover dependencies through
    /// Cargo.
    #[cfg(feature = "rust_inspect")]
    fn inspection_workspace_with_std_io_read() -> Result<tempfile::TempDir, Box<dyn std::error::Error>> {
        use incan_lang::interop::{
            RustFunctionSig, RustItemKind, RustItemMetadata, RustParam, RustTraitAssoc, RustTraitInfo, RustVisibility,
        };
        let workspace = tempfile::tempdir()?;
        std::fs::create_dir(workspace.path().join("src"))?;
        std::fs::write(workspace.path().join("src/lib.rs"), "")?;
        std::fs::write(
            workspace.path().join("loaf.toml"),
            "[project]\nname = \"io_repro_inspection\"\nversion = \"0.1.0\"\n[rust]\nname = \"io_repro_inspection\"\ntype = \"lib\"\nedition = \"2024\"\n",
        )?;
        rust_inspect::write_sealed_oven_inspection_source_authority(workspace.path(), Vec::new())?;
        for marker in [
            rust_inspect::OVEN_DIRECT_INSPECTION_MARKER,
            rust_inspect::OVEN_LOAF_ONLY_INSPECTION_MARKER,
        ] {
            std::fs::write(workspace.path().join(marker), "1\n")?;
        }
        let method = |name: &str, receiver: &str, params: &[(&str, &str)], return_type: &str| {
            let receiver = RustParam {
                name: Some("self".to_string()),
                type_display: receiver.to_string(),
            };
            RustTraitAssoc::Function {
                name: name.to_string(),
                signature: RustFunctionSig {
                    receiver_contract: None,
                    type_params: Vec::new(),
                    params: std::iter::once(receiver)
                        .chain(params.iter().map(|(name, ty)| RustParam {
                            name: Some((*name).to_string()),
                            type_display: (*ty).to_string(),
                        }))
                        .collect(),
                    return_type: return_type.to_string(),
                    is_async: false,
                    is_unsafe: false,
                },
            }
        };
        rust_inspect::RustMetadataCache::new().insert_test_item(
            workspace.path(),
            RustItemMetadata {
                canonical_path: "std::io::Read".to_string(),
                definition_path: Some("std::io::Read".to_string()),
                visibility: RustVisibility::Public,
                kind: RustItemKind::Trait(RustTraitInfo {
                    items: vec![
                        method("read", "&mut self", &[("buf", "&mut [u8]")], "Result<usize>"),
                        method("read_to_end", "&mut self", &[("buf", "&mut Vec<u8>")], "Result<usize>"),
                        method("read_exact", "&mut self", &[("buf", "&mut [u8]")], "Result<()>"),
                        method("by_ref", "&mut self", &[], "&mut Self"),
                        method("take", "self", &[("limit", "u64")], "Take<Self>"),
                    ],
                    derive_macro: None,
                }),
            },
        )?;
        Ok(workspace)
    }

    /// Seed the inspected `rustix::fs::flock` a consumer's Rust inspection holds when it mounts `std.fs.locking` from
    /// source into `workspace`, as the inspector reads it: its `Fd: AsFd` descriptor is lent (`&impl AsFd`), so a
    /// lock guard keeps the file it locked.
    #[cfg(feature = "rust_inspect")]
    fn seed_rustix_flock(workspace: &Path) -> Result<(), Box<dyn std::error::Error>> {
        use incan_lang::interop::{RustFunctionSig, RustItemKind, RustItemMetadata, RustParam, RustVisibility};
        let param = |name: &str, type_display: &str| RustParam {
            name: Some(name.to_string()),
            type_display: type_display.to_string(),
        };
        rust_inspect::RustMetadataCache::new().insert_test_item(
            workspace,
            RustItemMetadata {
                canonical_path: "rustix::fs::flock".to_string(),
                definition_path: Some("rustix::fs::flock".to_string()),
                visibility: RustVisibility::Public,
                kind: RustItemKind::Function(RustFunctionSig {
                    receiver_contract: None,
                    type_params: Vec::new(),
                    params: vec![param("fd", "&impl AsFd"), param("operation", "FlockOperation")],
                    return_type: "Result<()>".to_string(),
                    is_async: false,
                    is_unsafe: false,
                }),
            },
        )?;
        Ok(())
    }

    /// #1561: a consumer that imports `std.io` and `std.fs` with no SDK inventory, as on a fresh home, checks those
    /// modules and the `std.derives.collection` they import from the standard library's source, under the generated
    /// `__incan_std` namespace. That source is the standard library's own there as it is under `std` or in an SDK
    /// component build, and it checks with and without the consumer's Rust inspection, where `Read.by_ref(guard)` on
    /// a `borrow_mut()` guard the inspection cannot type stays as open as it is without metadata.
    #[test]
    fn a_consumer_without_an_sdk_inventory_checks_std_io_and_std_fs_from_source_issue1561()
    -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let project = tmp.path().join("io_repro");
        std::fs::create_dir_all(project.join("src"))?;
        let entry = project.join("src/main.incn");
        std::fs::write(
            &entry,
            "from std.io import BytesIO\nfrom std.fs import Path\n\ndef main() -> None:\n    buffer = BytesIO()\n    print(len(buffer.getvalue()))\n    print(Path(\"a\").exists())\n",
        )?;
        let session = session_without_sdk_inventory(&project);
        let modules = crate::modules::collect_modules_detailed_with_session(entry, &session)
            .map_err(|failure| failure.render_human())?;
        for expected in [&["io"][..], &["fs"], &["derives", "collection"]] {
            let path = std::iter::once(stdlib::INCAN_STD_NAMESPACE)
                .chain(expected.iter().copied())
                .map(str::to_string)
                .collect::<Vec<_>>();
            assert!(
                modules.iter().any(|module| module.path_segments == path),
                "`{}` must be collected from source: {:?}",
                path.join("."),
                modules
                    .iter()
                    .map(|module| module.path_segments.join("."))
                    .collect::<Vec<_>>()
            );
        }
        session
            .analyze_modules(
                &modules,
                #[cfg(feature = "rust_inspect")]
                None,
            )
            .map_err(|failure| format!("without Rust inspection:\n{}", failure.render_human()))?;
        #[cfg(feature = "rust_inspect")]
        {
            let inspection = inspection_workspace_with_std_io_read()?;
            session
                .analyze_modules(&modules, Some(inspection.path()))
                .map_err(|failure| format!("with Rust inspection:\n{}", failure.render_human()))?;
        }
        Ok(())
    }

    /// A consumer whose calls reach the methods `std.io` and `std.fs` implement in their own source, trait methods
    /// beside inherent ones: `BytesIO.write` of bytes and the typed `BinaryWrite` writes, on a local, a parameter
    /// spelled `_BytesIO`, a field, a list element and a module-qualified construction; the typed `BinaryRead` reads;
    /// the same calls through a type parameter's bound; the `BinaryReader.chunks` default a `BytesIO` and a `File`
    /// both adopt; and a `File`'s and a `Path`'s text and byte reads and writes. It imports the module
    /// [`STD_IO_CODEC_MODULE`], which makes typed writes of its own.
    #[cfg(feature = "rust_inspect")]
    const STD_IO_AND_FS_CALLS: &str = r#"import std.io as io
from std.fs import Path
from std.io import BinaryRead, BinaryWrite, BytesIO, Endian, IoError, _BytesIO
from codec import encode_pair


class Encoder:
    pub writer: _BytesIO

    def put(self, item: u16) -> Result[None, IoError]:
        return self.writer.write(item, Endian.Big)


def put_bounded[W with BinaryWrite[u32]](writer: W, item: u32) -> Result[None, IoError]:
    return writer.write(item, Endian.Big)


def get_bounded[R with BinaryRead[u16]](reader: R) -> Result[u16, IoError]:
    return reader.read(Endian.Little)


def encode(writer: _BytesIO, item: u32) -> Result[bytes, IoError]:
    writer.write(b"ab")?
    small: u8 = 9
    writer.write(small, Endian.Little)?
    writer.write(item, Endian.Big)?
    return Ok(writer.getvalue())


def decode(data: bytes) -> Result[str, IoError]:
    reader = BytesIO(data)
    head = reader.read_exact(2)?
    small: u8 = reader.read(Endian.Little)?
    item: u32 = reader.read(Endian.Big)?
    return Ok(f"{len(head)} {small} {item}")


def chunk_count(data: bytes) -> Result[int, IoError]:
    mut count = 0
    for chunk in BytesIO(data).chunks(4)?:
        count = count + 1
    return Ok(count)


def round_trip() -> Result[str, IoError]:
    data = encode(BytesIO(), 258)?
    decoded = decode(data)?
    chunks = chunk_count(data)?
    return Ok(f"{len(data)} {decoded} {chunks}")


def other_receivers() -> Result[str, IoError]:
    encoder = Encoder(writer=BytesIO())
    encoder.put(772)?
    stream = io.BytesIO()
    put_bounded(stream, 5)?
    writers = [BytesIO()]
    narrow: u16 = 513
    writers[0].write(narrow, Endian.Little)?
    widened = get_bounded(BytesIO(writers[0].getvalue()))?
    pair = encode_pair(7, 9)?
    return Ok(f"{len(encoder.writer.getvalue())} {len(stream.getvalue())} {widened} {len(pair)}")


def files() -> Result[str, IoError]:
    path = Path("data.txt")
    out = path.open("w")?
    out.write("hello")?
    out.write_bytes(b" world")?
    out.flush()?
    text = path.open("r")?
    head = text.read(5)?
    rest = text.read_bytes(-1)?
    exact = path.open("rb")?.read_exact(3)?
    mut chunks = 0
    for chunk in path.open("rb")?.chunks(4)?:
        chunks = chunks + 1
    raw = Path("data.bin")
    raw.write_bytes(b"xyz")?
    return Ok(f"{head}|{len(rest)}|{len(exact)}|{chunks}|{len(raw.read_bytes()?)}")


def report(outcome: Result[str, IoError]) -> None:
    match outcome:
        Ok(text) => println(text)
        Err(err) => println(err.message())


def main() -> None:
    report(round_trip())
    report(other_receivers())
    report(files())
"#;

    /// The module `codec` of [`STD_IO_AND_FS_CALLS`]: typed `BytesIO` writes made in a module the entry imports.
    #[cfg(feature = "rust_inspect")]
    const STD_IO_CODEC_MODULE: &str = r#"from std.io import BytesIO, Endian, IoError


pub def encode_pair(first: u16, second: i32) -> Result[bytes, IoError]:
    out = BytesIO()
    out.write(first, Endian.Big)?
    out.write(second, Endian.Little)?
    return Ok(out.getvalue())
"#;

    /// The runtime crates a program that mounts `std.io` and `std.fs` from source links beside `incan_std_core`, each
    /// with the version its source names (`rust::byteorder @ "1"`), which selects among the versions a build holds.
    #[cfg(feature = "rust_inspect")]
    const STD_IO_AND_FS_RUNTIME_CRATES: [(&str, Option<&str>); 4] = [
        ("incan_std_core", None),
        ("byteorder", Some("1.")),
        ("encoding_rs", Some("0.8.")),
        ("rustix", Some("1.")),
    ];

    /// Write `source` as the entry of a project with no SDK inventory under `root`, beside each of `modules` as a
    /// module of that name, and collect its module graph as a consumer on a fresh home collects it: every `std` module
    /// the entry reaches comes from source.
    #[cfg(feature = "rust_inspect")]
    fn consumer_without_sdk_inventory(
        root: &Path,
        source: &str,
        modules: &[(&str, &str)],
    ) -> Result<(CompilationSession, Vec<ParsedModule>), Box<dyn std::error::Error>> {
        let project = root.join("consumer");
        std::fs::create_dir_all(project.join("src"))?;
        for (name, module) in modules {
            std::fs::write(project.join("src").join(format!("{name}.incn")), module)?;
        }
        let entry = project.join("src/main.incn");
        std::fs::write(&entry, source)?;
        let session = session_without_sdk_inventory(&project);
        let modules = crate::modules::collect_modules_detailed_with_session(entry, &session)
            .map_err(|failure| failure.render_human())?;
        Ok((session, modules))
    }

    /// Check `modules` in `session` with the Rust inspection in `inspection`, generate the Rust project an Oven build
    /// prepares from those facts under `output`, and return its crate root.
    ///
    /// The entry module is the crate root; every other module, each standard-library module mounted from source
    /// included, is emitted under its module path with its public items kept, and the edition is the Oven's default.
    #[cfg(feature = "rust_inspect")]
    fn generate_consumer_project(
        session: &CompilationSession,
        modules: &[ParsedModule],
        inspection: &Path,
        output: &Path,
    ) -> Result<PathBuf, Box<dyn std::error::Error>> {
        use crate::backend::ir::IrCodegen;
        use crate::backend::project::ProjectGenerator;

        let analysis = session
            .analyze_modules(modules, Some(inspection))
            .map_err(|failure| failure.render_human())?;
        let (entry, dependencies) = modules.split_last().ok_or("the module graph has no entry module")?;
        let type_info_of = |module: &ParsedModule| {
            analysis
                .type_info_for_path(&module.file_path)
                .cloned()
                .ok_or_else(|| format!("no analysis for {}", module.file_path.display()))
        };
        let mut dependency_type_info = std::collections::HashMap::new();
        let mut codegen = IrCodegen::new();
        codegen.set_rust_inspect_manifest_dir(inspection.to_path_buf());
        codegen.set_preserve_dependency_public_items(true);
        codegen.set_registry_package_identity(Some("consumer".to_string()));
        codegen.set_root_source_module_name(Some("main".to_string()));
        codegen.set_provider_plan(session.provider_plan_for_modules(modules)?);
        for module in dependencies {
            codegen.add_module_with_path_segments(&module.name, &module.ast, module.path_segments.clone());
            dependency_type_info.insert(module.path_segments.clone(), type_info_of(module)?);
        }
        codegen.set_stdlib_cache(analysis.stdlib_cache().clone());
        codegen.set_prechecked_type_info(type_info_of(entry)?, dependency_type_info);
        let module_paths = dependencies
            .iter()
            .map(|module| module.path_segments.clone())
            .collect::<Vec<_>>();
        let (main_code, rust_modules) = codegen.try_generate_multi_file_nested(&entry.ast, &module_paths)?;
        let mut generator = ProjectGenerator::new(output, "consumer", true);
        generator.set_rust_edition(Some("2024".to_string()));
        generator.generate_nested(&main_code, &rust_modules)?;
        Ok(generator.crate_root_path())
    }

    /// Build the generated binary crate rooted at `crate_root` with rustc, linking `runtime_crates`, run it in
    /// `run_directory` and return its standard output.
    ///
    /// Under the compiler suite, the stored capability names rustc, the standard library's facets and the directories
    /// the suite built the workspace's crates into; see [`generated_program_rustc`]. Otherwise rustc links the runtime
    /// crates this test binary's own build produced, found beside it in the target profile.
    #[cfg(feature = "rust_inspect")]
    fn build_and_run_generated_program(
        crate_root: &Path,
        runtime_crates: &[(&str, Option<&str>)],
        run_directory: &Path,
    ) -> Result<String, Box<dyn std::error::Error>> {
        let build = tempfile::tempdir()?;
        let program = build.path().join("program");
        let mut command = generated_program_rustc(runtime_crates)?;
        let built = command
            .arg(crate_root)
            .arg("-o")
            .arg(&program)
            .env("CARGO_PKG_NAME", "consumer")
            .env("CARGO_PKG_VERSION", "0.1.0")
            .output()?;
        if !built.status.success() {
            return Err(format!(
                "rustc refused the generated program:\n{}",
                String::from_utf8_lossy(&built.stderr)
            )
            .into());
        }
        let ran = std::process::Command::new(&program)
            .current_dir(run_directory)
            .output()?;
        if !ran.status.success() {
            return Err(format!(
                "the generated program failed:\n{}",
                String::from_utf8_lossy(&ran.stderr)
            )
            .into());
        }
        Ok(String::from_utf8(ran.stdout)?)
    }

    /// Return a rustc invocation for one generated binary with `runtime_crates` as the crates it may name.
    ///
    /// Under the compiler suite, the stored capability's externs are the standard library's facets and the crates
    /// they link, so a runtime crate a module mounted from source names beyond them (`byteorder`, `encoding_rs`,
    /// `rustix`) is not among them: rustc refused `use ::rustix::fs::flock` with E0433. Each such crate is one of this
    /// test binary's own dependencies, which the suite builds into the directories the capability searches, and it is
    /// found there by name and version.
    #[cfg(feature = "rust_inspect")]
    fn generated_program_rustc(
        runtime_crates: &[(&str, Option<&str>)],
    ) -> Result<std::process::Command, Box<dyn std::error::Error>> {
        use oven_store::compiler_suite_env::{OVEN_COMPILER_SUITE_CAPABILITY_ENV, OvenCompilerSuiteCapability};

        let capability = OvenCompilerSuiteCapability::from_environment(OVEN_COMPILER_SUITE_CAPABILITY_ENV)
            .map_err(std::io::Error::other)?;
        let rustc = capability
            .as_ref()
            .map(|capability| capability.rustc.clone())
            .or_else(|| std::env::var_os("RUSTC").map(PathBuf::from))
            .unwrap_or_else(|| "rustc".into());
        let mut command = std::process::Command::new(rustc);
        command.args([
            "--edition=2024",
            "-A",
            "warnings",
            "--crate-name=consumer",
            "--crate-type=bin",
        ]);
        if let Some(capability) = capability {
            for path in &capability.dependency_search_paths {
                command.arg("-L").arg(format!("dependency={}", path.display()));
            }
            for (name, path) in &capability.externs {
                command.arg("--extern").arg(format!("{name}={}", path.display()));
            }
            for (name, version) in runtime_crates {
                if capability.externs.contains_key(*name) {
                    continue;
                }
                let artifact = newest_build_artifact(&capability.dependency_search_paths, name, "rlib", *version)?
                    .ok_or_else(|| {
                        format!("building a generated program requires the compiler suite to hold a compiled `{name}`")
                    })?;
                command.arg("--extern").arg(format!("{name}={}", artifact.display()));
            }
            return Ok(command);
        }
        let directories = test_build_artifact_directories()?;
        for directory in &directories {
            command.arg("-L").arg(format!("dependency={}", directory.display()));
        }
        let derive = newest_build_artifact(&directories, "incan_derive", std::env::consts::DLL_EXTENSION, None)?
            .ok_or("building a generated program requires a compiled incan_derive")?;
        command
            .arg("--extern")
            .arg(format!("incan_derive={}", derive.display()));
        for (name, version) in runtime_crates {
            let artifact = newest_build_artifact(&directories, name, "rlib", *version)?
                .ok_or_else(|| format!("building a generated program requires a compiled `{name}`"))?;
            command.arg("--extern").arg(format!("{name}={}", artifact.display()));
        }
        Ok(command)
    }

    /// Return the directories of this test binary's target profile that hold the crates its own build compiled.
    ///
    /// Cargo places every compiled crate of one build in the same layout as the test binary: beside it in `deps`, or
    /// in each build unit's output directory under `build`. Only that layout is searched, because another toolchain's
    /// build of the same profile, such as the SDK prewarm's, can leave crates in the other one that the rustc on
    /// `PATH` refuses (E0514).
    #[cfg(feature = "rust_inspect")]
    fn test_build_artifact_directories() -> Result<Vec<PathBuf>, Box<dyn std::error::Error>> {
        let executable = std::env::current_exe()?;
        if let Some(deps) = executable
            .parent()
            .filter(|parent| parent.file_name().is_some_and(|name| name == "deps"))
        {
            return Ok(vec![deps.to_path_buf()]);
        }
        let profile = executable
            .ancestors()
            .find(|ancestor| ancestor.join("build").is_dir())
            .ok_or("the test executable has no target profile directory")?;
        let mut directories = Vec::new();
        for package in std::fs::read_dir(profile.join("build"))? {
            for unit in std::fs::read_dir(package?.path())? {
                let output = unit?.path().join("out");
                if output.is_dir() {
                    directories.push(output);
                }
            }
        }
        Ok(directories)
    }

    /// Find the newest compiled `lib<name>-<hash>.<extension>` in `directories`, of the version `version` starts when
    /// one is given.
    ///
    /// The version is read from the crate's source directory (`rustix-1.1.4/src/lib.rs`), which the dependency file
    /// rustc writes beside the artifact names, so a build that holds two versions of one crate yields the one asked
    /// for. A build that keeps no dependency files, as the compiler suite's does, leaves the same source paths in the
    /// artifact's own metadata, which is read instead.
    #[cfg(feature = "rust_inspect")]
    fn newest_build_artifact(
        directories: &[PathBuf],
        name: &str,
        extension: &str,
        version: Option<&str>,
    ) -> Result<Option<PathBuf>, Box<dyn std::error::Error>> {
        let prefix = format!("lib{name}-");
        let mut matches = Vec::new();
        for directory in directories {
            for entry in std::fs::read_dir(directory)? {
                let path = entry?.path();
                let Some(hash) = path
                    .file_name()
                    .and_then(|file| file.to_str())
                    .and_then(|file| file.strip_prefix(&prefix))
                    .and_then(|file| file.strip_suffix(&format!(".{extension}")))
                else {
                    continue;
                };
                if let Some(version) = version {
                    let dependency_file = directory.join(format!("{name}-{hash}.d"));
                    let sources = std::fs::read(dependency_file).or_else(|_| std::fs::read(&path))?;
                    let names_directory = |directory: String| {
                        sources
                            .windows(directory.len())
                            .any(|window| window == directory.as_bytes())
                    };
                    if !names_directory(format!("{}-{version}", name.replace('_', "-")))
                        && !names_directory(format!("{name}-{version}"))
                    {
                        continue;
                    }
                }
                let modified = std::fs::metadata(&path).and_then(|metadata| metadata.modified()).ok();
                matches.push((modified, path));
            }
        }
        matches.sort_by_key(|(modified, _)| *modified);
        Ok(matches.pop().map(|(_, path)| path))
    }

    /// #1561: a consumer with no SDK inventory emits the `std.io` and `std.fs` modules it mounts from source beside
    /// its own, and every standard-library method it calls names an item those modules emit, whatever dispatch
    /// selected it; the program builds with rustc and runs.
    ///
    /// A mounted module declares its methods under `__incan_std.*`, while the consumer and its other modules see them
    /// under `std.*`. A call dispatched through `BinaryWrite`, `BinaryRead` or `BinaryReader` on a `BytesIO` named the
    /// method under `std.io`, which no module emits, and rustc refused the program with E0599, in the entry and in
    /// `codec` alike.
    #[cfg(feature = "rust_inspect")]
    #[test]
    fn a_consumer_without_an_sdk_inventory_builds_std_io_and_std_fs_trait_calls_from_source_issue1561()
    -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let (session, modules) =
            consumer_without_sdk_inventory(tmp.path(), STD_IO_AND_FS_CALLS, &[("codec", STD_IO_CODEC_MODULE)])?;
        let inspection = inspection_workspace_with_std_io_read()?;
        seed_rustix_flock(inspection.path())?;
        let crate_root =
            generate_consumer_project(&session, &modules, inspection.path(), &tmp.path().join("generated"))?;
        let run_directory = tmp.path().join("run");
        std::fs::create_dir_all(&run_directory)?;
        let stdout = build_and_run_generated_program(&crate_root, &STD_IO_AND_FS_RUNTIME_CRATES, &run_directory)?;
        assert_eq!(stdout, "7 2 9 258 2\n2 4 513 6\nhello|6|3|3|3\n");
        Ok(())
    }
}
