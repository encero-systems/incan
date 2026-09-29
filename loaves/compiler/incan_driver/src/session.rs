//! The compilation session: one per command, owning the module set, the provider plan, and the analysis every
//! stage after parsing consumes.
//!
//! A session is constructed once from the project requirements and reused by every stage so provider selection,
//! vocab providers, and typechecking are decided once per command; the analysis-invocation scope below is the
//! test instrument that proves that.

#[cfg(test)]
use std::cell::Cell;
use std::collections::HashSet;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use incan_lang::lang::stdlib;

use crate::diagnostics::CliDiagnosticFailure;
use crate::error::{CliError, CliResult};
use crate::project::{discover_effective_project_manifest, resolve_project_root, resolve_source_root};
use crate::typecheck::typecheck_modules_with_import_graph_artifacts;
use incan_frontend::ast::{Program, Span};
use incan_frontend::contract_metadata::{
    CanonicalModelBundle, materialize_contract_models, read_project_model_bundles,
};
use incan_frontend::hir::build_semantic_module_snapshot_v0;
use incan_frontend::library_manifest_index::LibraryManifestIndex;
use incan_frontend::parsed_module::ParsedModule;
use incan_frontend::testing_markers::{
    TestingMarkerSemantics, load_testing_marker_semantics, testing_marker_semantics_from_manifest,
};
use incan_frontend::typechecker::TypeCheckInfo;
use incan_frontend::typechecker::stdlib_loader::StdlibAstCache;
use incan_frontend::{diagnostics, lexer, parser, vocab_desugar_pass};
use incan_provider::inventory::{
    discover_or_reuse_published_sdk_inventory, prepare_or_discover_sdk_inventory, provider_used_module_paths,
    resolve_sdk_component_selection, sdk_provider_bootstrap_namespace_roots, validate_component_inventory_selection,
};
use incan_provider::requirements::{
    DependencyManifestMode, SdkInventorySource, parser_only_library_manifest_index,
    prepare_library_dependency_artifacts,
};
use incan_provider::{
    FeatureSelection, PackageFeatureGraph, PackageFeaturePlan, ProviderModuleResolution, ProviderPlan,
    ProviderProvenance, ResolvedSdkComponents, SdkComponentSelection, SdkInventory,
};
use oven_model::manifest::ProjectManifest;
/// Shared immutable provider projections indexed by the canonical modules an invocation uses.
type ProviderPlanCache = Arc<Mutex<BTreeMap<BTreeSet<Vec<String>>, Arc<ProviderPlan>>>>;

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
#[derive(Debug, Clone)]
pub struct CompilationSession {
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
    /// Semantic provider identities reused only within this checked compilation context.
    provider_semantic_identities: Arc<incan_provider::lock_semantics::ProviderSemanticIdentitySession>,
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
            SdkInventorySource::PrepareLegacyCargoIfAbsent,
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
        Self::discover_with_dependency_mode_and_sdk_source(
            entry_path,
            DependencyManifestMode::OvenArtifacts,
            SdkInventorySource::DiscoverOnly,
            feature_selection,
            sdk_profile_override,
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
        let inferred_project_root = resolve_project_root(entry_path);
        let manifest = discover_effective_project_manifest(&inferred_project_root)?;
        let project_root = manifest
            .as_ref()
            .map(|manifest| manifest.project_root().to_path_buf())
            .unwrap_or(inferred_project_root);
        let source_root = resolve_source_root(&project_root, manifest.as_ref());
        let sdk_inventory = match sdk_source {
            SdkInventorySource::PrepareLegacyCargoIfAbsent => prepare_or_discover_sdk_inventory()?,
            // An Oven command never builds the providers, but it reuses the inventory `incan check` published for a
            // source checkout, so both parse a file with the same standard-library vocabulary (#1774).
            SdkInventorySource::DiscoverOnly => discover_or_reuse_published_sdk_inventory()?,
        };
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
        // inventory to discover, so a session that neither published nor reused the checkout's inventory silently fell
        // back to the legacy monolithic stdlib. Besides losing component-aware diagnostics and the standard library's
        // vocabulary, that made a transient SDK profile fail during collection and allowed the lock projection to
        // drift before execution prepared the artifacts. Publication is content-addressed: the check path publishes
        // or reuses it, the Oven and collection paths reuse it only, and parser-only mode still avoids preparing
        // ordinary dependencies.
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

    /// Return provider semantic identities after revalidating their exact physical and dependency context.
    pub fn provider_semantic_identities(
        &self,
        provider_plan: &ProviderPlan,
        sdk_path_dependencies: &[oven_model::manifest::DependencySpec],
    ) -> CliResult<Arc<incan_provider::lock_semantics::CheckedProviderSemanticIdentities>> {
        self.provider_semantic_identities
            .identities(provider_plan, sdk_path_dependencies)
            .map_err(CliError::failure)
    }

    /// Resolve module participation from this session's immutable provider, feature, and SDK inputs.
    pub fn provider_plan_for_modules(&self, modules: &[ParsedModule]) -> CliResult<Arc<ProviderPlan>> {
        self.provider_plan_for_used_module_paths(provider_used_module_paths(modules))
    }

    /// Resolve one provider projection from canonical module paths while retaining the session's authority snapshot.
    ///
    /// Callers with nested source constructs can derive their complete module-use set once, then enter the same cache
    /// as ordinary compilation without rebuilding SDK, package-feature, or library-manifest inputs.
    pub fn provider_plan_for_used_module_paths(
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
mod tests {
    use super::*;
    use incan_frontend::library_manifest::{LibraryManifest, VocabExports};
    use incan_provider::test_support::{parsed_module_for_test, write_minimal_library_artifact};

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
    /// `incan check` builds its session with [`SdkInventorySource::PrepareLegacyCargoIfAbsent`] and publishes the
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

        // ---- A synthetic compiler checkout whose component catalog publishes without building anything ----
        let tmp = tempfile::tempdir()?;
        let checkout = tmp.path().join("checkout");
        let stdlib_root = checkout.join("loaves/stdlib");
        std::fs::create_dir_all(checkout.join("loaves/compiler/incan_emit/src"))?;
        std::fs::create_dir_all(&stdlib_root)?;
        std::fs::write(checkout.join("Cargo.toml"), "[workspace]\nmembers = []\n")?;
        std::fs::write(
            checkout.join("loaves/compiler/incan_emit/Cargo.toml"),
            "[package]\nname = \"incan_emit\"\n",
        )?;
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
            // Publication needs a builder executable to name; an empty catalog never launches it.
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

    /// A session with no SDK inventory, as a consumer on a fresh home has: every `std` module is collected from its
    /// source under the generated `__incan_std` namespace and checked with the consumer's modules.
    fn session_without_sdk_inventory(project: &Path) -> CompilationSession {
        let library_manifest_index = LibraryManifestIndex::default();
        let provider_plan = Arc::new(ProviderPlan::default());
        CompilationSession {
            manifest: None,
            source_root: project.join("src"),
            library_imported_vocab: library_manifest_index.library_imported_vocab(),
            library_imported_dsl_surfaces: library_manifest_index.library_imported_dsl_surfaces(),
            library_manifest_index,
            provider_plans_by_modules: Arc::new(Mutex::new(BTreeMap::from([(
                BTreeSet::new(),
                Arc::clone(&provider_plan),
            )]))),
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

    /// Seed the inspected `std::io::Read` a consumer's Rust inspection holds, with the methods the standard library's
    /// `Read.by_ref(guard).take(size).read_to_end(out)` reads call, into a fresh inspection workspace.
    #[cfg(feature = "rust_inspect")]
    fn inspection_workspace_with_std_io_read() -> Result<tempfile::TempDir, Box<dyn std::error::Error>> {
        use incan_lang::interop::{
            RustFunctionSig, RustItemKind, RustItemMetadata, RustParam, RustTraitAssoc, RustTraitInfo, RustVisibility,
        };
        let workspace = tempfile::tempdir()?;
        std::fs::write(
            workspace.path().join("Cargo.toml"),
            "[package]\nname = \"io_repro_inspection\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )?;
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
    /// Under the compiler suite, the stored capability names rustc and every runtime crate. Otherwise rustc links the
    /// runtime crates this test binary's own build produced, found beside it in the target profile.
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
            for path in capability.dependency_search_paths {
                command.arg("-L").arg(format!("dependency={}", path.display()));
            }
            for (name, path) in capability.externs {
                command.arg("--extern").arg(format!("{name}={}", path.display()));
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
    /// The version is read from the dependency file rustc writes beside the artifact, which names the crate's source
    /// directory (`rustix-1.1.4/src/lib.rs`), so a build that holds two versions of one crate yields the one asked for.
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
                    let sources = std::fs::read_to_string(dependency_file).unwrap_or_default();
                    let source_directory = format!("{}-{version}", name.replace('_', "-"));
                    let exact_directory = format!("{name}-{version}");
                    if !sources.contains(&source_directory) && !sources.contains(&exact_directory) {
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
