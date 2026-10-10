//! Preparing a library project: `prepare_library_project`, the one entry every library build, publication and
//! package export goes through.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::env;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

mod current_plans;
pub(crate) mod metadata_replay;

use sha2::Sha256;

use crate::backend::selection::digest_output;
use crate::backend::{IrCodegen, ProjectGenerator};
use crate::build::backend_selection::{finalize_backend_receipt, select_build_backend};
use crate::build::caller_facet::CallerFacetRequest;
use crate::build::caller_owned::append_oven_interop_execution_build_inputs;
use crate::build::library_exports::{
    LibraryReexportResolver, collect_library_rust_abi, collect_library_rust_abi_query_paths, module_key,
    public_ordinal_type_identities, resolve_library_project_root, validate_library_entrypoint,
};
use crate::build::library_outputs::{
    dependency_artifact_skips_canonical_lock, library_output_path, library_rust_inspection_required,
    package_desugarer_artifact, remove_generated_library_self_dependencies,
};
use crate::build::oven_project::project_extension_base_loaf;
use crate::build::plan_authority::{explicit_bake_profiles, oven_source_inline_dependency_specs};
use crate::build::provider_compilation::{
    checked_packaged_provider_profiles, import_packaged_provider_loafs_for_explicit_bake,
};
use crate::build::provider_metadata::{
    collect_unprojected_provider_modules, compiled_provider_metadata, synchronize_projected_provider_dependencies,
};
use crate::build::rust_extern::{collect_rust_extern_contexts, multi_file_output_identity, rust_extern_report_paths};
use crate::build::{
    CompiledProviderMetadataInputs, OvenProjectBakeAuthorityContext, OvenProjectPlanMode, PreparedLibraryProject,
    manifest_project_report, record_timing, source_file_report,
};
use crate::build_report::{
    BuildOvenReport, BuildReportDraft, BuildReportMode, cargo_report, dependencies_report, generated_project_report,
    incan_dependencies_report, interop_report, oven_generated_project_report, semantic_report,
};
use crate::cargo_policy::{CargoPolicy, cargo_command_flags, enforce_project_toolchain_constraint};
use crate::diagnostics::render_module_warnings;
use crate::error::{CliError, CliResult};
use crate::generated_cache::resolve_generated_cargo_target;
#[cfg(feature = "rust_inspect")]
use crate::lock::OvenRustInspectSourceAuthorityRequest;
#[cfg(feature = "rust_inspect")]
use crate::lock::RustInspectWorkspaceRequest;
use crate::lock::resolution::resolve_lock_context;
use crate::lock::{LockResolution, LockResolutionRequest};
use crate::modules::{
    build_source_map, collect_rust_dependency_uses, format_dependency_error,
    imported_module_deps_for_with_provider_plan, module_key_index, register_module_path_segments,
};
use crate::oven_store::open_default_oven_store;
use crate::project::discover_effective_project_manifest;
#[cfg(feature = "rust_inspect")]
use crate::rust_inspect_workspace::collect_rust_inspect_derive_probe_paths;
use crate::session::CompilationSession;
#[cfg(feature = "rust_inspect")]
use ::rust_inspect::RustMetadataCache;
use incan_emit::CallerIdentity;
use incan_frontend::api_metadata::{
    CHECKED_API_METADATA_SCHEMA_VERSION, CheckedApiMetadataPackage, CheckedApiPackageIdentity,
    collect_checked_api_alias_metadata, collect_checked_api_metadata, materialize_api_alias_projections,
    materialize_checked_api_public_namespaces, validate_checked_api_docstrings,
};
use incan_frontend::contract_metadata::ContractMetadataPackage;
use incan_frontend::library_exports::{CheckedNamedExport, checked_exports_by_name, collect_checked_public_exports};
use incan_frontend::library_manifest::LibraryManifest;
use incan_frontend::registry_metadata::{
    CHECKED_REGISTRY_METADATA_SCHEMA_VERSION, CheckedRegistryMetadataPackage, CheckedRegistryPackageIdentity,
    collect_checked_registry_metadata, materialize_registry_reexport_projections,
};
use incan_frontend::typechecker::stdlib_loader::StdlibAstCache;
use incan_frontend::{ParsedModule, diagnostics, typechecker};
use incan_lang::version::INCAN_VERSION;
use incan_provider::compiled_sdk::CompiledSdkModules;
use incan_provider::dependency_resolver::resolve_reachable_dependencies;
use incan_provider::inventory::extend_requirements_with_provider_plan;
use incan_provider::requirements::{
    INTERNAL_LIBRARY_ARTIFACT_ONLY_ENV, collect_project_requirements, merge_project_requirement_dependencies,
    semantic_sdk_path_dependencies,
};
use incan_provider::vocab_extraction::{
    PendingDesugarerArtifact, collect_library_vocab_metadata, oven_vocab_direct_rustc_context_from_plan,
};
use incan_provider::{FeatureSelection, SDK_PROVIDER_BUILD_ENV};
use oven_model::lock::CargoFeatureSelection;
use oven_rustc::loaf::OVEN_SOURCE_COMPILER_VOCAB_SUPPORT_BUILD_INPUT;
use oven_rustc::rustc::{resolve_active_rustc, rustc_host_target, rustc_identity};
use sha2::Digest as _;

#[cfg(test)]
thread_local! {
    static ORDINARY_LIBRARY_PREPARATION_BRANCHES: std::cell::Cell<(usize, usize)> = const {
        std::cell::Cell::new((0, 0))
    };
}

/// Reset actual ordinary preparation branch counts on the invoking test thread.
#[cfg(test)]
pub(crate) fn reset_ordinary_library_preparation_branches() {
    ORDINARY_LIBRARY_PREPARATION_BRANCHES.set((0, 0));
}

/// Return source-loading and admitted-replay branch visits on the invoking test thread.
#[cfg(test)]
pub(crate) fn ordinary_library_preparation_branches() -> (usize, usize) {
    ORDINARY_LIBRARY_PREPARATION_BRANCHES.get()
}

/// Count the actual pre-frontend decision, without timing or pretending that lock collection avoids source parsing.
#[cfg(test)]
fn record_ordinary_library_preparation_branch(replayed: bool) {
    let (fresh, replay) = ORDINARY_LIBRARY_PREPARATION_BRANCHES.get();
    ORDINARY_LIBRARY_PREPARATION_BRANCHES.set((fresh + usize::from(!replayed), replay + usize::from(replayed)));
}

/// Checked public metadata shared by source inspection and durable library publication.
struct CheckedPublicLibraryMetadata {
    manifest: LibraryManifest,
    selected_exports: Vec<CheckedNamedExport>,
    type_info: BTreeMap<PathBuf, typechecker::TypeCheckInfo>,
    stdlib_cache: StdlibAstCache,
}

/// Check the selected package projection once and retain only its public export contract.
///
/// Both publication and an unbaked dependency check must use this projection: importing producer source directly
/// would expose private declarations and trait/default bodies which a public package does not carry.
fn checked_public_library_metadata(
    manifest: &oven_model::manifest::ProjectManifest,
    compilation_session: &CompilationSession,
    modules: &[ParsedModule],
    provider_plan: &Arc<incan_provider::ProviderPlan>,
    project_name: &str,
    project_version: &str,
    timings_ms: &mut BTreeMap<String, u64>,
    #[cfg(feature = "rust_inspect")] rust_inspect_manifest_dir: Option<&Path>,
) -> CliResult<CheckedPublicLibraryMetadata> {
    let lib_module = modules
        .last()
        .ok_or_else(|| CliError::failure("no modules in checked library projection"))?;
    let mut all_errors = String::new();
    let mut checked_exports_by_module: HashMap<String, HashMap<String, Vec<CheckedNamedExport>>> = HashMap::new();
    let mut checked_exports_by_source_module: Vec<(Vec<String>, Vec<CheckedNamedExport>)> = Vec::new();
    let mut api_metadata_modules = Vec::new();
    let module_idx_by_key = module_key_index(&modules);
    let mut stdlib_cache = StdlibAstCache::new();
    let mut checked_type_info_by_path = BTreeMap::new();

    let typecheck_start = Instant::now();
    for (idx, module) in modules.iter().enumerate() {
        let deps_for_module =
            imported_module_deps_for_with_provider_plan(&modules, idx, &module_idx_by_key, &provider_plan);
        let mut checker = typechecker::TypeChecker::new();
        checker.stdlib_cache = stdlib_cache.clone();
        checker.set_current_package_identity(incan_frontend::module::declaration_package_identity(
            Some(project_name),
            Some(&module.path_segments),
        ));
        checker.set_current_module_path(Some(module.path_segments.clone()));
        register_module_path_segments(&mut checker, &modules);
        checker.set_declared_crate_names(manifest.declared_rust_crate_names());
        checker.set_provider_plan(Arc::clone(provider_plan));
        #[cfg(feature = "rust_inspect")]
        if let Some(rust_inspect_manifest_dir) = rust_inspect_manifest_dir {
            checker.set_rust_inspect_manifest_dir(rust_inspect_manifest_dir.to_path_buf());
        }

        // A provider producer checks its complete source package before publishing the public checked facade.
        let check_result = if provider_plan.bootstrap_sdk_namespace_roots().next().is_some()
            || provider_plan.standard_source_publication().is_some()
        {
            checker.check_with_imports_allow_private(&module.ast, &deps_for_module)
        } else {
            checker.check_with_imports(&module.ast, &deps_for_module)
        };
        match check_result {
            Ok(()) => {
                render_module_warnings(
                    module.file_path.to_string_lossy().as_ref(),
                    &module.source,
                    checker.warnings(),
                );
                let module_exports = collect_checked_public_exports(&module.ast, &checker);
                api_metadata_modules.push(collect_checked_api_metadata(
                    &module.ast,
                    &checker,
                    module.path_segments.clone(),
                ));
                checked_exports_by_source_module.push((module.path_segments.clone(), module_exports.clone()));
                checked_exports_by_module.insert(
                    module_key(&module.path_segments),
                    checked_exports_by_name(module_exports),
                );
                checked_type_info_by_path.insert(module.file_path.clone(), checker.type_info().clone());
                stdlib_cache = checker.stdlib_cache.clone();
            }
            Err(errs) => {
                stdlib_cache = checker.stdlib_cache.clone();
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

    if !all_errors.is_empty() {
        return Err(CliError::failure(all_errors.trim_end()));
    }
    #[cfg(feature = "rust_inspect")]
    if let Some(rust_inspect_manifest_dir) = rust_inspect_manifest_dir {
        RustMetadataCache::new()
            .persist_manifest_dir(rust_inspect_manifest_dir)
            .map_err(|error| {
                CliError::failure(format!(
                    "failed to persist batched Rust inspection metadata for {}: {error}",
                    rust_inspect_manifest_dir.display()
                ))
            })?;
    }

    record_timing(timings_ms, "library_typecheck_modules", typecheck_start);
    let api_validation_start = Instant::now();
    materialize_api_alias_projections(&mut api_metadata_modules);
    let registry_module_path = |module: &ParsedModule| {
        if module.file_path == lib_module.file_path {
            vec!["lib".to_string()]
        } else {
            module.path_segments.clone()
        }
    };
    let mut registry_metadata_modules = modules
        .iter()
        .filter_map(|module| {
            checked_type_info_by_path.get(&module.file_path).map(|type_info| {
                collect_checked_registry_metadata(type_info, registry_module_path(module), project_name)
            })
        })
        .collect::<Vec<_>>();
    let registry_alias_modules = modules
        .iter()
        .map(|module| collect_checked_api_alias_metadata(&module.ast, registry_module_path(module)))
        .collect::<Vec<_>>();
    materialize_registry_reexport_projections(&mut registry_metadata_modules, &registry_alias_modules);

    for diagnostic in validate_checked_api_docstrings(&api_metadata_modules) {
        if let Some(module) = modules
            .iter()
            .find(|module| module.path_segments == diagnostic.module_path)
        {
            all_errors.push_str(&diagnostics::format_error(
                module.file_path.to_string_lossy().as_ref(),
                &module.source,
                &diagnostic.error,
            ));
        } else {
            all_errors.push_str(&diagnostic.error.message);
            all_errors.push('\n');
        }
    }

    if !all_errors.is_empty() {
        return Err(CliError::failure(all_errors.trim_end()));
    }

    record_timing(timings_ms, "library_validate_api_metadata", api_validation_start);
    let export_start = Instant::now();
    let selected_exports = LibraryReexportResolver::new(&checked_exports_by_module)
        .resolve(lib_module)
        .map_err(|errs| {
            let mut msg = String::new();
            for err in &errs {
                msg.push_str(&diagnostics::format_error(
                    lib_module.file_path.to_string_lossy().as_ref(),
                    &lib_module.source,
                    err,
                ));
            }
            CliError::failure(msg.trim_end())
        })?;

    record_timing(timings_ms, "library_resolve_exports", export_start);
    let metadata_start = Instant::now();
    let mut library_manifest = LibraryManifest::from_checked_exports(project_name, project_version, &selected_exports);
    library_manifest.contract_metadata.models = ContractMetadataPackage::new(
        compilation_session
            .contract_model_bundles
            .iter()
            .cloned()
            .filter(|bundle| bundle.publishable)
            .collect(),
    );
    let mut checked_api = CheckedApiMetadataPackage {
        schema_version: CHECKED_API_METADATA_SCHEMA_VERSION,
        package: Some(CheckedApiPackageIdentity {
            name: project_name.to_string(),
            version: Some(project_version.to_string()),
        }),
        modules: api_metadata_modules,
        public_namespaces: Vec::new(),
    };
    materialize_checked_api_public_namespaces(&mut checked_api)
        .map_err(|error| CliError::failure(format!("failed to publish checked module namespaces: {error}")))?;
    library_manifest
        .contract_metadata
        .identity_graph
        .extend_checked_api_exports(project_name, &checked_api, &checked_exports_by_source_module)
        .map_err(|error| CliError::failure(format!("failed to publish checked module identities: {error}")))?;
    library_manifest.contract_metadata.api = Some(checked_api);
    let mut registry_metadata = CheckedRegistryMetadataPackage {
        schema_version: CHECKED_REGISTRY_METADATA_SCHEMA_VERSION,
        package: Some(CheckedRegistryPackageIdentity {
            name: project_name.to_string(),
            version: Some(project_version.to_string()),
        }),
        modules: registry_metadata_modules,
    };
    for module in &mut registry_metadata.modules {
        module.registries.retain(|registry| registry.public);
        module.entries.retain(|entry| entry.registry_public);
    }
    registry_metadata
        .modules
        .retain(|module| !module.registries.is_empty() || !module.entries.is_empty());
    library_manifest.contract_metadata.registry = Some(registry_metadata);
    record_timing(timings_ms, "library_build_manifest_metadata", metadata_start);
    Ok(CheckedPublicLibraryMetadata {
        manifest: library_manifest,
        selected_exports,
        type_info: checked_type_info_by_path,
        stdlib_cache,
    })
}

/// Derive an unbaked dependency's checked public metadata without generating or compiling native output.
pub(crate) fn checked_source_library_manifest(
    session: &CompilationSession,
    entry: &Path,
) -> CliResult<LibraryManifest> {
    let manifest = session
        .manifest
        .as_ref()
        .ok_or_else(|| CliError::failure("source provider has no loaf.toml"))?;
    let modules = crate::modules::collect_library_modules_detailed_with_session(entry.to_path_buf(), session)
        .map_err(|failure| CliError::failure(failure.render_human()))?;
    let provider_plan = session.provider_plan_for_modules(&modules)?;
    #[cfg(feature = "rust_inspect")]
    {
        let rust_queries = collect_library_rust_abi_query_paths(&modules, &collect_rust_extern_contexts(&modules));
        if !rust_queries.is_empty() {
            return Err(CliError::failure(format!(
                "checked source provider {} requires Rust ABI metadata for {}; this check cannot compile or inspect the producer",
                manifest.project_root().display(),
                rust_queries.join(", ")
            )));
        }
    }
    let name = manifest
        .project
        .as_ref()
        .and_then(|project| project.name.as_deref())
        .unwrap_or("incan_library");
    let version = manifest
        .project
        .as_ref()
        .and_then(|project| project.version.as_deref())
        .unwrap_or("0.1.0");
    let mut timings_ms = BTreeMap::new();
    let public = checked_public_library_metadata(
        manifest,
        session,
        &modules,
        &provider_plan,
        name,
        version,
        &mut timings_ms,
        #[cfg(feature = "rust_inspect")]
        None,
    )?;
    let mut library_manifest = public.manifest;
    let unprojected = crate::build::provider_metadata::collect_unprojected_provider_modules(entry, session)?;
    let entry_module = modules
        .last()
        .ok_or_else(|| CliError::failure("source provider has no library module"))?;
    let feature_plan = session
        .package_feature_plan
        .as_ref()
        .ok_or_else(|| CliError::failure("source provider has no package feature plan"))?;
    library_manifest.contract_metadata.provider =
        crate::build::provider_metadata::checked_source_provider_metadata(CompiledProviderMetadataInputs {
            manifest,
            feature_plan,
            provider_plan: &provider_plan,
            library_manifest_index: &session.library_manifest_index,
            artifact_root: manifest.project_root(),
            modules: &unprojected,
            active_library_entrypoint: entry_module,
            checked_type_info_by_path: &public.type_info,
        })?;
    // A source check has no persistent artifact. Bind its command-local identity to the checked dependency graph
    // as well as the producer's authored inputs, so a private dependency edit cannot leave the parent's authority
    // unchanged.
    let context = provider_plan
        .records()
        .map(|record| record.identity.stable_key())
        .collect::<Vec<_>>();
    let wire = serde_json::to_vec(&(
        &library_manifest.contract_metadata.provider.semantic_source_digest,
        context,
    ))
    .map_err(|error| CliError::failure(error.to_string()))?;
    library_manifest.contract_metadata.provider.semantic_source_digest =
        Some(format!("sha256:{}", hex::encode(sha2::Sha256::digest(wire))));
    let generated_target = env::var_os(oven_model::toolchain_layout::GENERATED_CARGO_TARGET_DIR_ENV)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from);
    if let Some(vocab) = incan_provider::vocab_extraction::collect_library_vocab_metadata_for_check(
        manifest,
        manifest.project_root(),
        generated_target.as_deref(),
    )? {
        library_manifest.vocab = Some(vocab.payload);
        library_manifest.soft_keywords.activations = vocab.compatibility_activations;
    }
    Ok(library_manifest)
}

/// Validate a library project and generate its Rust project without running Cargo.
///
/// Normal consumers include their already selected interop execution receipt in the runtime identity. Explicit Oven
/// preparation and Rust inspection deliberately omit it, so a package can first produce the base receipt required by
/// `incan oven interop bake`; neither path selects a native tool, discovers a system library, or weakens the normal
/// execution requirement.
#[allow(clippy::too_many_arguments)] // Library preparation receives the same independent CLI selection axes.
pub fn prepare_library_project(
    file_path: Option<&str>,
    output_dir: Option<&str>,
    cargo_policy: CargoPolicy,
    package_features: &FeatureSelection,
    sdk_profile_override: Option<&str>,
    cargo_features: Vec<String>,
    cargo_no_default_features: bool,
    cargo_all_features: bool,
    generated_cargo_target_dir: Option<&Path>,
    normal_oven: bool,
    include_interop_execution: bool,
    oven_plan_mode: OvenProjectPlanMode,
    authority_context: Option<&mut OvenProjectBakeAuthorityContext>,
) -> CliResult<PreparedLibraryProject> {
    prepare_library_project_with_caller_facet(
        file_path,
        output_dir,
        cargo_policy,
        package_features,
        sdk_profile_override,
        cargo_features,
        cargo_no_default_features,
        cargo_all_features,
        generated_cargo_target_dir,
        normal_oven,
        include_interop_execution,
        oven_plan_mode,
        authority_context,
        None,
    )
}

/// Prepare a library while emitting one usage-derived Rust caller projection.
#[allow(clippy::too_many_arguments)]
pub fn prepare_library_project_with_caller_facet(
    file_path: Option<&str>,
    output_dir: Option<&str>,
    cargo_policy: CargoPolicy,
    package_features: &FeatureSelection,
    sdk_profile_override: Option<&str>,
    cargo_features: Vec<String>,
    cargo_no_default_features: bool,
    cargo_all_features: bool,
    generated_cargo_target_dir: Option<&Path>,
    normal_oven: bool,
    include_interop_execution: bool,
    oven_plan_mode: OvenProjectPlanMode,
    authority_context: Option<&mut OvenProjectBakeAuthorityContext>,
    caller_facet: Option<&CallerFacetRequest>,
) -> CliResult<PreparedLibraryProject> {
    match prepare_library_project_with_context(
        file_path,
        output_dir,
        cargo_policy,
        package_features,
        sdk_profile_override,
        cargo_features,
        cargo_no_default_features,
        cargo_all_features,
        generated_cargo_target_dir,
        normal_oven,
        include_interop_execution,
        oven_plan_mode,
        authority_context,
        caller_facet,
        None,
        None,
    )? {
        LibraryPreparation::Project(project) => Ok(*project),
        LibraryPreparation::Native { .. } => Err(CliError::failure("unexpected native library publication")),
    }
}

/// Explicit ordinary dependency/session inputs retained by the invoking publisher (#1337/#1698).
///
/// Ordinary native requests and the temporary SDK compatibility route are explicit alternatives. This input grants
/// no new namespace or macro authority; ordinary callers must prove their checked native demands separately.
pub(crate) struct AdmittedLibraryPreparation {
    session: CompilationSession,
    native: AdmittedLibraryNative,
}

/// Retain the exact native route selected by the caller; absence never selects a different route.
enum AdmittedLibraryNative {
    TemporarySdk(Arc<super::NativeSdkCommandContext>),
    Ordinary(Arc<super::ordinary_library_native::OrdinaryLibraryNativeProfiles>),
}

impl AdmittedLibraryPreparation {
    /// Retain actual admitted dependencies and the original native admission without discovering an SDK.
    pub(crate) fn new(
        session: CompilationSession,
        temporary_native_sdk_context: Arc<super::NativeSdkCommandContext>,
    ) -> CliResult<Self> {
        let dependencies = session
            .admitted_library_dependencies()
            .ok_or_else(|| CliError::failure("explicit ordinary preparation requires admitted library dependencies"))?;
        dependencies.verify()?;
        temporary_native_sdk_context.verify()?;
        Ok(Self {
            session,
            native: AdmittedLibraryNative::TemporarySdk(temporary_native_sdk_context),
        })
    }

    /// Bind an admitted language dependency session to original ordinary native producer requests.
    pub(crate) fn with_ordinary_native(
        session: CompilationSession,
        native: Arc<super::ordinary_library_native::OrdinaryLibraryNativeProfiles>,
    ) -> CliResult<Self> {
        session
            .admitted_library_dependencies()
            .ok_or_else(|| CliError::failure("ordinary native preparation requires admitted language dependencies"))?
            .verify()?;
        native.verify()?;
        Ok(Self {
            session,
            native: AdmittedLibraryNative::Ordinary(native),
        })
    }

    /// Revalidate the original chosen native route without discovery or cross-route fallback.
    pub(crate) fn verify_native(&self) -> CliResult<()> {
        match &self.native {
            AdmittedLibraryNative::TemporarySdk(native) => native.verify(),
            AdmittedLibraryNative::Ordinary(native) => native.verify(),
        }
    }

    /// Borrow the original admitted session for the invoking publisher's source/lock finalization boundary.
    pub(crate) fn session(&self) -> &CompilationSession {
        &self.session
    }

    /// Borrow the explicitly temporary native SDK admission without reacquiring or discovering another owner set.
    pub(crate) fn temporary_native_sdk_context(&self) -> Option<&Arc<super::NativeSdkCommandContext>> {
        match &self.native {
            AdmittedLibraryNative::TemporarySdk(native) => Some(native),
            AdmittedLibraryNative::Ordinary(_) => None,
        }
    }

    /// Borrow complete original ordinary requests through preparation, replay and finalization.
    pub(crate) fn ordinary_native(
        &self,
    ) -> Option<&Arc<super::ordinary_library_native::OrdinaryLibraryNativeProfiles>> {
        match &self.native {
            AdmittedLibraryNative::Ordinary(native) => Some(native),
            AdmittedLibraryNative::TemporarySdk(_) => None,
        }
    }
}

/// A real prepared project borrowing the caller's original session/native leases until publication finishes.
pub(crate) struct PreparedAdmittedLibrary<'a> {
    project: PreparedLibraryProject,
    _inputs: &'a AdmittedLibraryPreparation,
}

impl PreparedAdmittedLibrary<'_> {
    /// Borrow the ordinary project produced by the shared generator and current-profile planners.
    pub(crate) fn project(&self) -> &PreparedLibraryProject {
        &self.project
    }

    /// Finalize ordinary publication without releasing the invoking command's original dependency owners.
    pub(crate) fn project_mut(&mut self) -> &mut PreparedLibraryProject {
        &mut self.project
    }
}

/// Prepare an ordinary package from explicit original admissions without discovering package or SDK authority.
///
/// Legacy CLI callers retain their discovery route. Official standard-source publication additionally requires
/// the genuine namespace issuer, which this entry point cannot manufacture.
pub(crate) fn prepare_admitted_library_project<'a>(
    inputs: &'a AdmittedLibraryPreparation,
    output_dir: Option<&str>,
    package_features: &FeatureSelection,
    include_interop_execution: bool,
    oven_plan_mode: OvenProjectPlanMode,
    authority_context: Option<&mut OvenProjectBakeAuthorityContext>,
) -> CliResult<PreparedAdmittedLibrary<'a>> {
    let manifest = inputs
        .session
        .manifest
        .as_ref()
        .ok_or_else(|| CliError::failure("explicit ordinary preparation has no project manifest"))?;
    let entry = validate_library_entrypoint(manifest)?;
    let entry = entry
        .to_str()
        .ok_or_else(|| CliError::failure("invalid ordinary library entry path"))?;
    let project = match prepare_library_project_with_context(
        Some(entry),
        output_dir,
        CargoPolicy::default(),
        package_features,
        None,
        Vec::new(),
        false,
        false,
        None,
        true,
        include_interop_execution,
        oven_plan_mode,
        authority_context,
        None,
        None,
        Some(inputs),
    )? {
        LibraryPreparation::Project(project) => *project,
        LibraryPreparation::Native { .. } => return Err(CliError::failure("unexpected native library publication")),
    };
    Ok(PreparedAdmittedLibrary {
        project,
        _inputs: inputs,
    })
}

/// Reconstruct current ordinary parsing inputs from the original private dependency capability, refusing drift.
pub(crate) fn current_admitted_library_session(
    input: &CompilationSession,
    entry: &Path,
    package_features: &FeatureSelection,
) -> CliResult<CompilationSession> {
    let dependencies = input
        .admitted_library_dependencies()
        .ok_or_else(|| CliError::failure("explicit ordinary preparation requires admitted library dependencies"))?;
    dependencies.verify()?;
    let original_plan = input.original_admitted_provider_plan()?;
    let current = if let Some(source) = original_plan.standard_source_publication() {
        CompilationSession::discover_with_admitted_standard_source(
            entry,
            package_features,
            Arc::clone(dependencies),
            Arc::clone(source),
        )?
    } else {
        CompilationSession::discover_with_admitted_library_dependencies(
            entry,
            package_features,
            Arc::clone(dependencies),
        )?
    };
    let input_manifest = input
        .manifest
        .as_ref()
        .ok_or_else(|| CliError::failure("explicit ordinary session has no project manifest"))?;
    let current_manifest = current
        .manifest
        .as_ref()
        .ok_or_else(|| CliError::failure("current ordinary session has no project manifest"))?;
    if std::fs::canonicalize(input_manifest.path()).map_err(|error| CliError::failure(error.to_string()))?
        != std::fs::canonicalize(current_manifest.path()).map_err(|error| CliError::failure(error.to_string()))?
        || input.source_root != current.source_root
    {
        return Err(CliError::failure(
            "explicit ordinary session belongs to a different project",
        ));
    }
    let input_features = input
        .package_feature_plan
        .as_ref()
        .ok_or_else(|| CliError::failure("explicit ordinary session has no feature authority"))?;
    let current_features = current
        .package_feature_plan
        .as_ref()
        .ok_or_else(|| CliError::failure("current ordinary session has no feature authority"))?;
    if input.active_features != current.active_features
        || input.declared_features != current.declared_features
        || admitted_feature_contract(input_features) != admitted_feature_contract(current_features)
    {
        return Err(CliError::failure(
            "explicit ordinary session features differ from current selection",
        ));
    }
    Ok(current)
}

/// Compare the complete resolved feature/edge selection without rereading source trees or activation explanations.
/// Current authored source observation remains the metadata preparation's separate production responsibility.
fn admitted_feature_contract(plan: &incan_provider::PackageFeaturePlan) -> serde_json::Value {
    let packages = plan
        .packages()
        .map(|state| {
            serde_json::json!({
                "name": state.package_name, "root": state.project_root,
                "manifest": state.feature_manifest_path,
                "active_dependencies": state.active_dependencies,
                "features": {
                    "active": state.features.active_features,
                    "optional": state.features.active_optional_dependencies,
                    "dependency_features": state.features.dependency_features,
                    "components": state.features.required_sdk_components,
                },
            })
        })
        .collect::<Vec<_>>();
    serde_json::json!({ "packages": packages, "edges": plan.edges().collect::<Vec<_>>() })
}

/// Products of a library preparation, keeping native publication separate from project execution plans.
pub(crate) enum LibraryPreparation {
    /// Generated ordinary project with its execution plan.
    Project(Box<PreparedLibraryProject>),
    /// Checked native SDK facade and portable executable surfaces.
    Native {
        manifest: Box<LibraryManifest>,
        executable: Vec<u8>,
        metadata_owner: Option<Arc<crate::build::library_metadata::SelectedLibraryMetadata>>,
    },
}

/// Check a native SDK component using the same checked export and metadata publication as other libraries.
pub(crate) fn prepare_native_sdk_component(
    project_root: &Path,
    output: &Path,
    context: &crate::build::native_sdk::NativeSdkPublicationContext<'_>,
) -> CliResult<LibraryPreparation> {
    let entry = project_root.join("src/lib.incn");
    let entry = entry
        .to_str()
        .ok_or_else(|| CliError::failure("invalid SDK entry path"))?;
    let output = output
        .to_str()
        .ok_or_else(|| CliError::failure("invalid SDK output path"))?;
    prepare_library_project_with_context(
        Some(entry),
        Some(output),
        CargoPolicy::default(),
        &FeatureSelection::default(),
        None,
        Vec::new(),
        false,
        false,
        None,
        true,
        false,
        OvenProjectPlanMode::ExplicitBake,
        None,
        None,
        Some(context),
        None,
    )
}

/// Prepare ordinary libraries or native SDK publication without crossing into compatibility metadata readers.
#[allow(clippy::too_many_arguments)]
fn prepare_library_project_with_context(
    file_path: Option<&str>,
    output_dir: Option<&str>,
    cargo_policy: CargoPolicy,
    package_features: &FeatureSelection,
    sdk_profile_override: Option<&str>,
    cargo_features: Vec<String>,
    cargo_no_default_features: bool,
    cargo_all_features: bool,
    generated_cargo_target_dir: Option<&Path>,
    normal_oven: bool,
    include_interop_execution: bool,
    oven_plan_mode: OvenProjectPlanMode,
    authority_context: Option<&mut OvenProjectBakeAuthorityContext>,
    caller_facet: Option<&CallerFacetRequest>,
    native_sdk: Option<&crate::build::native_sdk::NativeSdkPublicationContext<'_>>,
    admitted: Option<&AdmittedLibraryPreparation>,
) -> CliResult<LibraryPreparation> {
    let mut authority_context = authority_context;
    let explicit_native_context = admitted.and_then(|input| input.temporary_native_sdk_context().cloned());
    let ordinary_native = admitted.and_then(|input| input.ordinary_native().cloned());
    if let Some(native) = &ordinary_native {
        if !normal_oven
            || native_sdk.is_some()
            || authority_context
                .as_ref()
                .is_some_and(|authority| authority.native_sdk_context.is_some())
        {
            return Err(CliError::failure(
                "ordinary native preparation has competing SDK or legacy authority",
            ));
        }
        native.verify()?;
        if authority_context
            .as_ref()
            .and_then(|authority| authority.requested_target.as_deref())
            .is_some_and(|target| target != native.metadata().target())
        {
            return Err(CliError::failure(
                "ordinary native request differs from the library target",
            ));
        }
    }
    if let Some(context) = &explicit_native_context {
        if !normal_oven || native_sdk.is_some() {
            return Err(CliError::failure(
                "explicit ordinary admission requires the ordinary Oven route",
            ));
        }
        context.verify()?;
        if let Some(authority) = authority_context.as_deref_mut() {
            if authority
                .native_sdk_context
                .as_ref()
                .is_some_and(|selected| !Arc::ptr_eq(selected, context))
            {
                return Err(CliError::failure(
                    "explicit ordinary preparation has competing native admission",
                ));
            }
            authority.native_sdk_context = Some(Arc::clone(context));
        }
    }
    let prepare_start = Instant::now();
    let mut timings_ms = BTreeMap::new();
    let source_load_start = Instant::now();
    let project_root = resolve_library_project_root(file_path)?;
    let out_dir = library_output_path(&project_root, output_dir)?;
    let Some(manifest) = discover_effective_project_manifest(&project_root)? else {
        return Err(CliError::failure(
            "No loaf.toml found for `incan build --lib` (run `incan init` first)",
        ));
    };
    enforce_project_toolchain_constraint(&manifest)?;
    if let Some(context) = native_sdk {
        context.validate_component_facets(&manifest)?;
    }
    let project_version = manifest
        .project
        .as_ref()
        .and_then(|project| project.version.clone())
        .unwrap_or_else(|| "0.1.0".to_string());

    let lib_entry = validate_library_entrypoint(&manifest)?;
    let compilation_session = if let Some(input) = admitted {
        current_admitted_library_session(&input.session, &lib_entry, package_features)?
    } else if let Some(context) = native_sdk {
        CompilationSession::discover_for_native_sdk_component(
            &lib_entry,
            context.inventory,
            &context.namespace_roots,
            &context.native_facets,
        )?
    } else if normal_oven {
        CompilationSession::discover_for_oven(&lib_entry, package_features, sdk_profile_override)?
    } else {
        crate::session::CompilationSession::discover_with_selections(
            &lib_entry,
            package_features,
            sdk_profile_override,
        )?
    };
    let mut metadata_preparation = if normal_oven
        && caller_facet.is_none()
        && cargo_features.is_empty()
        && !cargo_no_default_features
        && !cargo_all_features
    {
        if let Some(native) = &ordinary_native {
            metadata_replay::MetadataPreparation::observe_with_ordinary_native_authority(
                &manifest,
                &compilation_session,
                &out_dir,
                Arc::clone(native.metadata()),
            )?
        } else {
            metadata_replay::MetadataPreparation::observe_with_native_context(
                &manifest,
                &compilation_session,
                &out_dir,
                native_sdk,
                authority_context.as_deref_mut(),
                explicit_native_context.clone(),
            )?
        }
    } else {
        None
    };
    if ordinary_native.is_some() && metadata_preparation.is_none() {
        return Err(CliError::failure(
            "ordinary native library requires observable checked metadata authority",
        ));
    }
    if let Some(preparation) = metadata_preparation.as_ref()
        && let Some(selected) = preparation.select()?
    {
        #[cfg(test)]
        record_ordinary_library_preparation_branch(true);
        return metadata_replay::prepare_replayed_library(metadata_replay::ReplayRequest {
            preparation,
            selected,
            project: &manifest,
            session: &compilation_session,
            native_sdk,
            out_dir,
            entrypoint: lib_entry,
            cargo_policy: &cargo_policy,
            package_features,
            sdk_profile_override,
            oven_plan_mode,
            include_interop_execution,
            authority: authority_context,
            ordinary_native: ordinary_native.clone(),
        });
    }
    #[cfg(test)]
    record_ordinary_library_preparation_branch(false);
    let modules =
        crate::modules::collect_library_modules_detailed_with_session(lib_entry.clone(), &compilation_session)
            .map_err(|failure| CliError::failure(failure.render_human()))?;
    let provider_metadata_modules = collect_unprojected_provider_modules(&lib_entry, &compilation_session)?;

    let Some(lib_module) = modules.last() else {
        return Err(CliError::failure("No modules found for library build"));
    };
    if lib_module.file_path != lib_entry {
        return Err(CliError::failure(format!(
            "Library entrypoint mismatch: expected `{}`, got `{}`",
            lib_entry.display(),
            lib_module.file_path.display()
        )));
    }
    record_timing(&mut timings_ms, "library_load_sources", source_load_start);

    let requirements_start = Instant::now();
    let declared = manifest.declared_rust_crate_names();
    let package_feature_plan = compilation_session
        .package_feature_plan
        .clone()
        .ok_or_else(|| CliError::failure("library compilation session is missing its package feature graph"))?;
    let library_manifest_index = compilation_session.library_manifest_index.clone();
    let mut project_requirements = collect_project_requirements(&modules, &library_manifest_index)?;
    let source_requirements = project_requirements.clone();
    let provider_plan = compilation_session.provider_plan_for_modules(&modules)?;
    let compiled_sdk_modules = CompiledSdkModules::from_provider_plan(&provider_plan);
    extend_requirements_with_provider_plan(&mut project_requirements, &provider_plan)?;
    let semantic_sdk_paths = if native_sdk.is_some() {
        Vec::new()
    } else {
        semantic_sdk_path_dependencies(&project_requirements)
    };
    let provider_semantic_identities =
        compilation_session.provider_semantic_identities(&provider_plan, &semantic_sdk_paths)?;
    let semantic = incan_provider::lock_semantics::semantic_lock_state_with_provider_identities(
        &project_root,
        manifest.interop_c(),
        compilation_session.sdk_inventory.as_deref(),
        compilation_session.sdk_components.as_ref(),
        Some(&package_feature_plan),
        &provider_plan,
        &semantic_sdk_paths,
        &provider_semantic_identities,
    )
    .map_err(CliError::failure)?;
    let rust_extern_contexts = collect_rust_extern_contexts(&modules);
    let dep_modules = &modules[..modules.len() - 1];
    // Library consumers use the same artifact metadata and linked Rust crate as executable and test-batch consumers;
    // migrated modules must not be generated into a second local `__incan_std` tree.
    let emitted_dep_modules: Vec<&ParsedModule> = dep_modules
        .iter()
        .filter(|module| !compiled_sdk_modules.contains_emission_path(&module.path_segments))
        .collect();

    let mut inline_imports = collect_rust_dependency_uses(lib_module, false);
    for module in &emitted_dep_modules {
        inline_imports.extend(collect_rust_dependency_uses(module, false));
    }
    // The compiler-owned standard library facets and Rust's sysroot are supplied by the selected Oven plan. The
    // remaining caller-authored imports are resolved after code generation and compiled through the same
    // direct-Rustc closure materializer used by normal executables and test batches; this library route must not
    // regain a Cargo fallback.
    let source_inline_crates = inline_imports
        .iter()
        .filter(|import| !incan_lang::lang::stdlib::facets::is_facet(&import.crate_name) && import.crate_name != "std")
        .map(|import| import.crate_name.clone())
        .collect::<BTreeSet<_>>();
    let project_name = manifest
        .project
        .as_ref()
        .and_then(|project| project.name.clone())
        .or_else(|| {
            manifest
                .project_root()
                .file_name()
                .and_then(|name| name.to_str())
                .map(str::to_owned)
        })
        .unwrap_or_else(|| "incan_library".to_string());

    let cargo_features = CargoFeatureSelection {
        cargo_features: cargo_features.clone(),
        cargo_no_default_features,
        cargo_all_features,
    }
    .normalized();
    record_timing(&mut timings_ms, "library_collect_requirements", requirements_start);

    let dependency_start = Instant::now();
    // A library projection must consume the same source-reachable dependency graph that owns the canonical project
    // lock. Including every declared-but-unused Rust dependency here can make the caller graph strictly larger than
    // the canonical lock generated from scripts, tests, and this library entry, causing a valid existing lock to be
    // rejected during rust-inspect projection.
    let mut resolved = match resolve_reachable_dependencies(Some(&manifest), &inline_imports, true, &cargo_features) {
        Ok(resolved) => resolved,
        Err(errors) => {
            let mut msg = String::new();
            let sources = build_source_map(&modules);
            for err in errors {
                msg.push_str(&format_dependency_error(&err, &sources));
            }
            return Err(CliError::failure(msg.trim_end()));
        }
    };
    // Compiled Incan providers participate in linking; their checked types do not require Rust source inspection.
    #[cfg(feature = "rust_inspect")]
    let inspection_dependencies = resolved.dependencies.clone();
    merge_project_requirement_dependencies(&mut resolved, &project_requirements)?;
    record_timing(&mut timings_ms, "library_resolve_dependencies", dependency_start);
    #[cfg(feature = "rust_inspect")]
    let metadata_query_paths = collect_library_rust_abi_query_paths(&modules, &rust_extern_contexts);
    #[cfg(not(feature = "rust_inspect"))]
    let metadata_query_paths: Vec<String> = Vec::new();

    if ordinary_native.is_some() {
        crate::build::library_metadata::requirements::capture_checked_native_demands(
            &manifest,
            &modules,
            &source_requirements,
            &provider_plan,
            &metadata_query_paths.iter().cloned().collect(),
        )?
        .require_support_only()?;
        if include_interop_execution {
            return Err(CliError::failure(
                "ordinary support-only library cannot request interop execution",
            ));
        }
    }

    let lock_start = Instant::now();
    let artifact_only = env::var_os(INTERNAL_LIBRARY_ARTIFACT_ONLY_ENV).is_some();
    if normal_oven && native_sdk.is_none() {
        if cargo_no_default_features || cargo_all_features || !cargo_features.cargo_features.is_empty() {
            return Err(CliError::failure(
                "Oven Alpha normal library builds do not accept Cargo feature controls; use Incan package features instead",
            ));
        }
        crate::lock::resolution::validate_oven_lock_policy_with_session(
            crate::lock::OvenLockValidationRequest {
                project_root: &project_root,
                manifest: Some(&manifest),
                entry_file: &lib_entry,
                cargo_features: &cargo_features,
                cargo_policy: &cargo_policy,
                package_features,
                sdk_profile_override,
            },
            &compilation_session,
        )?;
    }
    let (
        lock_payload_for_typecheck,
        cargo_lock_projection_root,
        clear_cargo_lock,
        cargo_flags,
        lock_cargo_package_name,
        managed_target_path,
        managed_target_lease,
        managed_target_identity,
    ) = if normal_oven {
        // A generated Cargo project is retained only as an inspectable source projection. Normal library execution
        // must neither acquire a generated Cargo cache nor derive an authority-bearing Cargo.lock from it.
        (
            None,
            None,
            false,
            Vec::new(),
            project_name.clone(),
            out_dir.join(".cargo-projection"),
            None,
            None,
        )
    } else {
        let dependency_artifact_only =
            dependency_artifact_skips_canonical_lock(artifact_only, env::var_os(SDK_PROVIDER_BUILD_ENV).is_some());
        let lock_resolution = if dependency_artifact_only {
            // Dependency artifact preparation has no Cargo build to constrain with a lock payload. Resolving the
            // canonical workspace lock here would traverse the consumer that requested this still-missing root artifact
            // and recursively launch the same artifact-only child. Keep the already-resolved producer context intact;
            // the parent command remains the sole owner of canonical lock generation and publication. SDK provider
            // artifact builds are excluded because their parent supplies an exact Cargo.lock payload override.
            LockResolution {
                cargo_lock_authority: crate::lock::CargoLockAuthority::None,
                cargo_package_name: project_name.clone(),
                resolved,
                project_requirements,
            }
        } else {
            resolve_lock_context(LockResolutionRequest {
                project_root: &project_root,
                project_name: project_name.as_str(),
                entry_file: Some(&lib_entry),
                manifest: Some(&manifest),
                resolved: &resolved,
                project_requirements: &project_requirements,
                cargo_features: &cargo_features,
                cargo_policy: &cargo_policy,
                semantic: Some(&semantic),
                package_features: Some(package_features),
                sdk_profile_override,
            })?
        };
        let cargo_lock_inputs = lock_resolution.cargo_lock_authority.into_generator_inputs();
        resolved = lock_resolution.resolved;
        project_requirements = lock_resolution.project_requirements;
        let managed_target = resolve_generated_cargo_target(
            generated_cargo_target_dir,
            &project_root,
            &out_dir,
            &lock_resolution.cargo_package_name,
            "release",
            cargo_lock_inputs.payload.as_deref(),
            &cargo_features,
            &cargo_command_flags(&cargo_policy, &cargo_features),
        )
        .map_err(|error| CliError::failure(format!("failed to prepare generated Cargo cache: {error}")))?;
        let (managed_target_path, managed_target_lease, managed_target_identity) = managed_target.into_parts();
        (
            cargo_lock_inputs.payload,
            cargo_lock_inputs.projection_root,
            cargo_lock_inputs.clear_existing,
            cargo_command_flags(&cargo_policy, &cargo_features),
            lock_resolution.cargo_package_name,
            managed_target_path,
            managed_target_lease,
            managed_target_identity,
        )
    };
    record_timing(&mut timings_ms, "library_resolve_lock_payload", lock_start);
    let native_admission_start = Instant::now();
    let native_sdk_context = if let Some(preparation) = metadata_preparation.as_ref() {
        preparation.native_context().cloned()
    } else if let Some(context) = &explicit_native_context {
        Some(Arc::clone(context))
    } else if ordinary_native.is_some() {
        None
    } else if normal_oven && native_sdk.is_none() {
        match authority_context.as_deref_mut() {
            Some(context) => context.native_sdk_context()?,
            None => super::NativeSdkCommandContext::discover()?,
        }
    } else {
        None
    };
    if normal_oven && native_sdk.is_none() {
        record_timing(&mut timings_ms, "library_native_sdk_admission", native_admission_start);
    }
    let mut oven_build_inputs = (normal_oven && native_sdk.is_none())
        .then(|| {
            if ordinary_native.is_some() {
                Ok(BTreeMap::new())
            } else {
                crate::build_unit::oven_build_unit_inputs_with_provider_identities_and_native_sdk(
                    &provider_plan,
                    &project_requirements,
                    &resolved,
                    &provider_semantic_identities,
                    native_sdk_context.as_deref(),
                )
            }
        })
        .transpose()?;
    let source_compiler_vocab_support = normal_oven
        && native_sdk.is_none()
        && manifest.vocab().is_some()
        && oven_cargo_compat::source_compiler_vocab_support_is_available();
    if source_compiler_vocab_support && let Some(build_inputs) = oven_build_inputs.as_mut() {
        // A source-built compiler seals this helper at the explicit publisher boundary. Keep that closure in a
        // distinct build unit so a v0.5.0 plan without it can neither shadow nor become ambiguous with the
        // upgraded receipt. Packaged compilers continue to select their release-cohort helper unchanged.
        build_inputs.insert(
            OVEN_SOURCE_COMPILER_VOCAB_SUPPORT_BUILD_INPUT.to_string(),
            "v1".to_string(),
        );
    }
    let oven_rustc = if let Some(preparation) = &metadata_preparation {
        Some(preparation.rustc.clone())
    } else {
        normal_oven
            .then(resolve_active_rustc)
            .transpose()
            .map_err(|error| CliError::failure(error.to_string()))?
    };
    let requested_target = authority_context
        .as_ref()
        .and_then(|context| context.requested_target.clone());
    let oven_target = if let Some(preparation) = &metadata_preparation {
        Some(preparation.recipe.target.clone())
    } else {
        match (oven_rustc.as_ref(), requested_target) {
            (Some(_), Some(target)) => Some(target),
            (Some(rustc), None) => {
                Some(rustc_host_target(rustc).map_err(|error| CliError::failure(error.to_string()))?)
            }
            (None, _) => None,
        }
    };
    let oven_toolchain = if let Some(preparation) = &metadata_preparation {
        Some(preparation.recipe.toolchain.clone())
    } else {
        oven_rustc
            .as_ref()
            .map(|rustc| rustc_identity(rustc))
            .transpose()
            .map_err(|error| CliError::failure(error.to_string()))?
    };
    let oven_store = (normal_oven && native_sdk.is_none())
        .then(open_default_oven_store)
        .transpose()?;
    if include_interop_execution {
        let (build_inputs, target) = match (oven_build_inputs.as_mut(), oven_target.as_deref()) {
            (Some(build_inputs), Some(target)) => (build_inputs, target),
            _ => {
                return Err(CliError::failure(
                    "interop execution can only be included in an Oven library preparation".to_string(),
                ));
            }
        };
        append_oven_interop_execution_build_inputs(build_inputs, Some(&manifest), target)?;
    }
    let empty_oven_build_inputs = BTreeMap::new();
    #[cfg(feature = "rust_inspect")]
    let rust_inspect_manifest_dir = if let Some(context) = native_sdk {
        Some(context.inspection_workspace()?)
    } else {
        if normal_oven {
            !metadata_query_paths.is_empty()
        } else {
            library_rust_inspection_required(artifact_only, &metadata_query_paths)
        }
        .then(|| {
            if normal_oven {
                Ok((
                    // Rust inspection is compiler-owned preparation state, not part of the generated provider
                    // artifact. Keeping its Cargo target below `target/lib` leaks build-script
                    // outputs (including valid symlinks) into the provider integrity boundary and
                    // needlessly makes every consumer traverse that cache.
                    oven_model::lock::compiler_lock_state_dir(&project_root).join("rust_inspect_target"),
                    None::<crate::generated_cache::GeneratedCacheLease>,
                ))
            } else {
                resolve_generated_cargo_target(
                    generated_cargo_target_dir,
                    &project_root,
                    &project_root,
                    &lock_cargo_package_name,
                    "rust-inspect",
                    lock_payload_for_typecheck.as_deref(),
                    &cargo_features,
                    &cargo_flags,
                )
                .map(|target| {
                    let (path, lease, _identity) = target.into_parts();
                    (path, lease)
                })
                .map_err(|error| CliError::failure(format!("failed to prepare rust-inspect Cargo cache: {error}")))
            }
        })
        .transpose()?
        .map(|(rust_inspect_target_path, _rust_inspect_cache_lease)| {
            let rust_inspect_start = Instant::now();
            let rust_inspect_manifest_dir = crate::lock::rust_inspect::prepare_rust_inspect_workspace_with_native_sdk(
                RustInspectWorkspaceRequest {
                    project_root: &project_root,
                    project_name: project_name.as_str(),
                    cargo_package_name: &lock_cargo_package_name,
                    rust_edition: manifest.rust_edition().map(str::to_string),
                    resolved: &resolved,
                    project_requirements: &project_requirements,
                    lock_payload: lock_payload_for_typecheck.clone(),
                    cargo_lock_projection_root: cargo_lock_projection_root.as_deref(),
                    clear_cargo_lock,
                    cargo_policy_flags: cargo_flags.clone(),
                    cargo_target_dir: &rust_inspect_target_path,
                    rust_inspect_query_paths: &metadata_query_paths,
                    rust_derive_probe_paths: &collect_rust_inspect_derive_probe_paths(&modules),
                    prepare_when_empty: true,
                    direct_oven_inspection: normal_oven,
                    force_direct_prewarm: false,
                    oven_source_authority: normal_oven.then(|| OvenRustInspectSourceAuthorityRequest {
                        project_version: &project_version,
                        target: oven_target.as_deref().unwrap_or_default(),
                        toolchain: oven_toolchain.as_deref().unwrap_or_default(),
                        profile: "debug",
                        features: &cargo_features.cargo_features,
                        build_unit_inputs: oven_build_inputs.as_ref().unwrap_or(&empty_oven_build_inputs),
                        registry_dependencies: &inspection_dependencies,
                    }),
                    prepared_project_source_authorities: None,
                    explicit_oven_bake: normal_oven && oven_plan_mode == OvenProjectPlanMode::ExplicitBake,
                },
                native_sdk_context.as_deref(),
            )?
            .ok_or_else(|| {
                CliError::failure("rust-inspect workspace preparation did not return a manifest directory")
            })?;
            record_timing(&mut timings_ms, "library_rust_inspect_prewarm", rust_inspect_start);
            Ok::<_, CliError>(rust_inspect_manifest_dir)
        })
        .transpose()?
    };

    let public_metadata = checked_public_library_metadata(
        &manifest,
        &compilation_session,
        &modules,
        &provider_plan,
        &project_name,
        &project_version,
        &mut timings_ms,
        #[cfg(feature = "rust_inspect")]
        rust_inspect_manifest_dir
            .as_ref()
            .map(|workspace| workspace.manifest_dir()),
    )?;
    let checked_metadata_ms = timings_ms
        .get("library_build_manifest_metadata")
        .copied()
        .unwrap_or_default();
    let manifest_start = Instant::now();
    let mut library_manifest = public_metadata.manifest;
    let selected_exports = public_metadata.selected_exports;
    let checked_type_info_by_path = public_metadata.type_info;
    let stdlib_cache = public_metadata.stdlib_cache;
    let project_license = manifest.project.as_ref().and_then(|project| project.license.clone());
    std::fs::create_dir_all(&out_dir)
        .map_err(|error| CliError::failure(format!("failed to create {}: {error}", out_dir.display())))?;
    let executable_modules = modules
        .iter()
        .map(|module| {
            let type_info = checked_type_info_by_path
                .get(&module.file_path)
                .ok_or_else(|| CliError::failure("checked library module facts are missing"))?;
            Ok(incan_frontend::body_ir::build_body_ir_module_v0(
                &module.ast,
                &module.path_segments,
                type_info,
            ))
        })
        .collect::<CliResult<Vec<_>>>()?;
    let public_identities =
        incan_frontend::library_manifest::published_layout::public_executable_identities(&library_manifest);
    // Sibling-module immutable values belong to the same declaring compilation. Retain their canonical context
    // before projecting bodies, so a public import is not downgraded to an unsupported global-storage read.
    let executable_modules = modules
        .iter()
        .map(|module| {
            let type_info = checked_type_info_by_path
                .get(&module.file_path)
                .ok_or_else(|| CliError::failure("checked library module facts are missing"))?;
            Ok(
                incan_frontend::body_ir::build_body_ir_module_v0_with_executable_context(
                    &module.ast,
                    &module.path_segments,
                    type_info,
                    &executable_modules,
                ),
            )
        })
        .collect::<Result<Vec<_>, CliError>>()?;
    // A published body is unrepresentable when Body IR has a gap in it, not when one consumer cannot execute it: what
    // a package publishes is a fact about the package, and every consumer reads the same representation.
    let unrepresentable = executable_modules
        .iter()
        .flat_map(|module| module.bodies.iter())
        .filter(|body| body.first_representation_gap().is_some())
        .filter_map(|body| body.canonical.clone())
        .collect();
    let executable_surface = incan_semantics_core::executable_representation::build_surface(
        &executable_modules,
        &project_name,
        &project_version,
        &public_identities,
        &unrepresentable,
    )
    .map_err(|error| CliError::failure(format!("failed to produce public executable representation: {error}")))?;
    library_manifest.contract_metadata.executable_representation =
        Some(incan_frontend::library_manifest::ExecutableRepresentationExport {
            representation_version: incan_semantics_core::executable_representation::EXECUTABLE_REPRESENTATION_VERSION,
            content_digest: hex::encode(Sha256::digest(&executable_surface)),
        });

    library_manifest.contract_metadata.provider = compiled_provider_metadata(CompiledProviderMetadataInputs {
        manifest: &manifest,
        feature_plan: &package_feature_plan,
        provider_plan: &provider_plan,
        library_manifest_index: &library_manifest_index,
        artifact_root: &out_dir,
        modules: &provider_metadata_modules,
        active_library_entrypoint: lib_module,
        checked_type_info_by_path: &checked_type_info_by_path,
    })?;
    #[cfg(feature = "rust_inspect")]
    if let Some(rust_inspect_manifest_dir) = rust_inspect_manifest_dir.as_ref() {
        library_manifest.rust_abi =
            collect_library_rust_abi(rust_inspect_manifest_dir.manifest_dir(), &metadata_query_paths)?;
    }
    record_timing(&mut timings_ms, "library_build_manifest_metadata", manifest_start);
    if let Some(elapsed) = timings_ms.get_mut("library_build_manifest_metadata") {
        *elapsed = elapsed.saturating_add(checked_metadata_ms);
    }
    if let Some(context) = native_sdk {
        library_manifest.contract_metadata.provider.implementation_facets =
            crate::build::provider_metadata::native_sdk_implementation_facets(
                &library_manifest.contract_metadata.provider.namespace_claims,
            )?;
        context.validate_component_facets(&manifest)?;
        let vocab_start = Instant::now();
        // The desugarer module is built here and copied into the component by `package_desugarer_artifact`.
        let desugarer_scratch = tempfile::tempdir().map_err(|error| CliError::failure(error.to_string()))?;
        if let Some(vocab) = incan_provider::vocab_extraction::collect_native_sdk_vocab_metadata(
            &manifest,
            &project_root,
            context.closure,
            &resolve_active_rustc().map_err(|error| CliError::failure(error.to_string()))?,
            desugarer_scratch.path(),
        )? {
            package_desugarer_artifact(&out_dir, vocab.pending_desugarer_artifact.as_ref())?;
            library_manifest.vocab = Some(vocab.payload);
            library_manifest.soft_keywords.activations = vocab.compatibility_activations;
        }
        record_timing(&mut timings_ms, "library_collect_vocab_metadata", vocab_start);
        #[cfg(feature = "rust_inspect")]
        let inspection = rust_inspect_manifest_dir
            .as_ref()
            .map(|workspace| workspace.manifest_dir());
        #[cfg(not(feature = "rust_inspect"))]
        let inspection = None;
        let codegen_start = Instant::now();
        crate::build::native_sdk::generate_native_sdk_sources(
            &out_dir,
            &project_name,
            &modules,
            &checked_type_info_by_path,
            stdlib_cache,
            &provider_plan,
            &mut library_manifest,
            &selected_exports,
            &declared,
            inspection,
        )?;
        record_timing(&mut timings_ms, "library_native_sdk_codegen", codegen_start);
        let metadata_owner = if let Some(preparation) = metadata_preparation.take() {
            let contract = checked_requirement_contract(
                &manifest,
                &compilation_session,
                &source_requirements,
                &library_manifest,
                &inline_imports,
                &selected_exports,
                &project_version,
                &modules,
                lib_module,
                &metadata_query_paths,
                None,
            );
            match contract {
                Ok(contract) => {
                    preparation.revalidate(&manifest, &compilation_session, &out_dir, native_sdk)?;
                    let manifest_path = crate::build::library_outputs::write_checked_library_payload(
                        &out_dir,
                        &library_manifest,
                        &executable_surface,
                    )?;
                    Some(
                        crate::build::library_metadata::publish_library_metadata_with_requirements(
                            &preparation.store,
                            &preparation.recipe,
                            &preparation.receipt,
                            &out_dir,
                            &manifest_path,
                            contract.rust_abi_queries.clone(),
                            Some(contract),
                        )?
                        .retaining_dependencies(preparation.dependency_owners())?,
                    )
                }
                Err(error) => {
                    tracing::debug!(reason = %error, "checked library replay contract is unavailable");
                    None
                }
            }
        } else {
            None
        };

        record_timing(&mut timings_ms, "library_prepare_total", prepare_start);
        for (phase, elapsed_ms) in &timings_ms {
            tracing::debug!(
                component = %project_name,
                phase,
                elapsed_ms,
                checked_modules = modules.len(),
                abi_query_paths = metadata_query_paths.len(),
                "SDK checked library preparation phase completed"
            );
        }
        return Ok(LibraryPreparation::Native {
            manifest: Box::new(library_manifest),
            executable: executable_surface,
            metadata_owner,
        });
    }
    let manifest_path = out_dir.join(format!("{project_name}.incnlib"));

    // ---- Backend selection (#986) — declared before codegen, refused visibly if unavailable ----
    let backend_selection = select_build_backend(&modules);

    let mut codegen = IrCodegen::new();
    codegen.set_preserve_dependency_public_items(true);
    codegen.set_registry_package_identity(Some(project_name.clone()));
    codegen.set_canonical_emission_package_identity(Some(project_name.clone()));
    codegen.set_root_source_module_name(
        lib_module
            .file_path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .map(str::to_string),
    );
    codegen.set_stdlib_cache(stdlib_cache);
    codegen.set_declared_crate_names(declared);
    codegen.set_provider_plan(Arc::clone(&provider_plan));
    let main_type_info = checked_type_info_by_path
        .get(&lib_module.file_path)
        .cloned()
        .ok_or_else(|| {
            CliError::failure(format!(
                "missing checked library analysis for {}",
                lib_module.file_path.display()
            ))
        })?;
    let mut dependency_type_info = HashMap::with_capacity(dep_modules.len());
    for module in dep_modules {
        let type_info = checked_type_info_by_path
            .get(&module.file_path)
            .cloned()
            .ok_or_else(|| {
                CliError::failure(format!(
                    "missing checked library analysis for {}",
                    module.file_path.display()
                ))
            })?;
        dependency_type_info.insert(module.path_segments.clone(), type_info);
    }
    codegen.set_prechecked_type_info(main_type_info, dependency_type_info);
    codegen.set_public_ordinal_type_identities(public_ordinal_type_identities(
        lib_module,
        project_name.as_str(),
        &selected_exports,
    ));
    if let Some(caller_facet) = caller_facet {
        let identity = CallerIdentity {
            package_name: project_name.clone(),
            package_version: project_version.clone(),
            caller_facet_id: caller_facet.facet_id.clone(),
            caller_abi_version: "1".to_string(),
            compiler_version_range: format!("={INCAN_VERSION}"),
            manifest_schema_version: library_manifest.manifest_format,
            target: caller_facet.target.clone(),
            profile: caller_facet.profile.clone(),
            receipt_reference: caller_facet.receipt_reference.clone(),
        };
        codegen = codegen.with_caller_facet(caller_facet.exports.iter().cloned(), identity);
    }
    for module in dep_modules
        .iter()
        .filter(|module| compiled_sdk_modules.contains_emission_path(&module.path_segments))
    {
        codegen.add_dependency_symbol_module_with_path_segments(
            &module.name,
            &module.ast,
            module.path_segments.clone(),
        );
    }
    for module in &emitted_dep_modules {
        codegen.add_module_with_path_segments(&module.name, &module.ast, module.path_segments.clone());
    }
    let mut generator = ProjectGenerator::new(&out_dir, project_name.as_str(), false);
    let checked_api = library_manifest.contract_metadata.api.as_ref().ok_or_else(|| {
        CliError::failure("checked API metadata is unavailable while generating public namespace facades")
    })?;
    generator.set_public_namespace_facades(checked_api);
    // Canonical workspace locking uses a synthetic package name so every member resolves one shared Cargo graph.
    // A published library artifact instead has an identity contract across Cargo.toml, `[lib]`, and `.incnlib`, so
    // its generated Cargo package must retain the selected producer project's name.
    generator.set_package_name(Some(project_name.clone()));
    generator.set_package_metadata(Some(project_version.clone()), project_license);
    generator.set_provider_plan(&provider_plan);
    generator.set_sdk_path_dependencies(project_requirements.sdk_path_dependencies.clone());
    if normal_oven {
        generator.set_cargo_target_dir_override(None);
        generator.set_generated_cache_context(None, None);
    } else {
        generator.set_cargo_target_dir_override(Some(managed_target_path.clone()));
        generator.set_generated_cache_context(managed_target_lease, managed_target_identity);
    }
    generator.set_stdlib_facets(project_requirements.stdlib_facets.clone());
    generator.set_include_dev_dependencies(
        lock_payload_for_typecheck.is_some() || oven_plan_mode == OvenProjectPlanMode::ExplicitBake,
    );
    let rust_edition = manifest.rust_edition().map(str::to_string);
    generator.set_rust_edition(rust_edition.clone());
    #[cfg(feature = "rust_inspect")]
    if let Some(rust_inspect_manifest_dir) = rust_inspect_manifest_dir.as_ref() {
        codegen.set_rust_inspect_manifest_dir(rust_inspect_manifest_dir.manifest_dir().to_path_buf());
    }
    generator.set_cargo_lock_payload(lock_payload_for_typecheck);
    generator.set_cargo_lock_projection_root(cargo_lock_projection_root.clone());
    generator.set_clear_cargo_lock(clear_cargo_lock);
    generator.set_cargo_policy_flags(cargo_flags);
    remove_generated_library_self_dependencies(&mut resolved, &project_root);
    let oven_inline_rust_dependencies = normal_oven
        .then(|| oven_source_inline_dependency_specs(&resolved, &source_inline_crates))
        .transpose()?;
    let rust_dependencies = resolved.dependencies.clone();
    let rust_dev_dependencies = resolved.dev_dependencies.clone();
    let checked_provider_profiles = if normal_oven {
        let target = oven_target
            .as_deref()
            .ok_or_else(|| CliError::failure("normal Oven library build omitted target"))?;
        let toolchain = oven_toolchain
            .as_deref()
            .ok_or_else(|| CliError::failure("normal Oven library build omitted toolchain"))?;
        checked_packaged_provider_profiles(
            &provider_plan,
            &explicit_bake_profiles(),
            target,
            toolchain,
            authority_context,
        )?
    } else {
        Vec::new()
    };
    let oven_plan_dependencies = oven_inline_rust_dependencies.clone().unwrap_or_default();
    if normal_oven {
        let store = oven_store
            .as_ref()
            .ok_or_else(|| CliError::failure("normal Oven library build omitted its bounded store"))?;
        import_packaged_provider_loafs_for_explicit_bake(oven_plan_mode, store, &checked_provider_profiles)?;
    }
    let mut report_draft = BuildReportDraft {
        mode: BuildReportMode::Library,
        profile: "release".to_string(),
        project: manifest_project_report(Some(&manifest), project_name.as_str(), &project_root),
        entrypoint: Some(lib_entry.to_string_lossy().to_string()),
        library_root: Some(project_root.to_string_lossy().to_string()),
        source_files: source_file_report(&modules),
        generated: generated_project_report(
            generator.output_dir(),
            &generator.crate_root_path(),
            &generator.cargo_target_dir(),
        ),
        artifacts: Vec::new(),
        dependencies: dependencies_report(
            &rust_dependencies,
            &rust_dev_dependencies,
            incan_dependencies_report(manifest.library_dependencies().iter().collect()),
            project_requirements.stdlib_facets.clone(),
        ),
        semantic: semantic_report(
            compilation_session.sdk_inventory.as_deref(),
            compilation_session.sdk_components.as_ref(),
            Some(&package_feature_plan),
            &provider_plan,
        ),
        cargo: Some(cargo_report(
            &cargo_policy,
            cargo_features.cargo_features.clone(),
            cargo_features.cargo_no_default_features,
            cargo_features.cargo_all_features,
        )),
        oven: None,
        interop: interop_report(
            &inline_imports,
            rust_extern_report_paths(&rust_extern_contexts),
            metadata_query_paths.clone(),
        ),
        notes: vec![
            "Generated Rust is current backend output for inspection and debugging, not a stable Rust ABI.".to_string(),
        ],
        backend: None,
    };
    let ordinary_runtime = ordinary_native
        .as_ref()
        .map(|native| {
            crate::build_unit::OrdinaryLibraryRuntimeInputs::from_checked(
                Arc::clone(native),
                &project_root,
                &provider_plan,
                &project_requirements,
                &resolved,
                &provider_semantic_identities,
            )
        })
        .transpose()?;
    generator.set_dependencies(resolved.dependencies);
    generator.set_dev_dependencies(resolved.dev_dependencies);

    // Keep the historical aggregate for existing consumers, while separating the stages that were previously
    // attributed misleadingly as one `library_generate_rust` cost in Oven performance evidence.
    codegen.set_publication_api(library_manifest.contract_metadata.api.clone());
    codegen.set_publication_identities(
        library_manifest.name.clone(),
        library_manifest.contract_metadata.identity_graph.clone(),
    );
    let codegen_start = Instant::now();
    let (backend_output_identity, generation_metadata) = if emitted_dep_modules.is_empty() {
        let emit_rust_start = Instant::now();
        let (rust_code, generation_metadata) = codegen
            .try_generate_with_metadata(&lib_module.ast, &lib_module.path_segments)
            .map_err(|e| CliError::failure(format!("Code generation error: {e}")))?;
        record_timing(&mut timings_ms, "library_codegen_emit_rust", emit_rust_start);
        let write_project_start = Instant::now();
        generator
            .generate(&rust_code)
            .map_err(|e| CliError::failure(format!("Error generating project: {e}")))?;
        record_timing(&mut timings_ms, "library_codegen_write_project", write_project_start);
        (digest_output(&[rust_code.as_str()]), generation_metadata)
    } else {
        let module_paths: Vec<Vec<String>> = emitted_dep_modules
            .iter()
            .map(|module| module.path_segments.clone())
            .collect();
        let emit_rust_start = Instant::now();
        let ((main_code, rust_modules), generation_metadata) = codegen
            .try_generate_multi_file_nested_with_metadata(&lib_module.ast, &module_paths, &lib_module.path_segments)
            .map_err(|e| CliError::failure(format!("Code generation error: {e}")))?;
        record_timing(&mut timings_ms, "library_codegen_emit_rust", emit_rust_start);
        let write_project_start = Instant::now();
        generator
            .generate_nested(&main_code, &rust_modules)
            .map_err(|e| CliError::failure(format!("Error generating project: {e}")))?;
        record_timing(&mut timings_ms, "library_codegen_write_project", write_project_start);
        (
            multi_file_output_identity(&main_code, &rust_modules),
            generation_metadata,
        )
    };
    generation_metadata
        .apply_to_library_manifest(&mut library_manifest)
        .map_err(|error| {
            CliError::failure(format!(
                "failed to publish inferred implementation requirements: {error}"
            ))
        })?;
    let backend_receipt = finalize_backend_receipt(&backend_selection, backend_output_identity)?;
    // Not persisted here — see the matching comment in `prepare_oven_project`: this function also runs for
    // internal/dependency callers, and real compilation still follows below. The receipt is published once by
    // `build_library_report` after the whole build succeeds (#986).
    report_draft.backend = Some(backend_receipt);
    let synchronize_provider_dependencies_start = Instant::now();
    synchronize_projected_provider_dependencies(
        &mut library_manifest,
        &out_dir,
        &generator.effective_dependencies().map_err(|error| {
            CliError::failure(format!("failed to resolve projected provider dependencies: {error}"))
        })?,
    )?;
    record_timing(
        &mut timings_ms,
        "library_codegen_sync_provider_dependencies",
        synchronize_provider_dependencies_start,
    );
    let oven_profiles_start = Instant::now();
    let oven = if normal_oven {
        Some(current_plans::prepare_current_library_profiles(
            current_plans::CurrentLibraryPlanInputs {
                project_root: &project_root,
                project_name: &project_name,
                project_version: &project_version,
                generator: &generator,
                provider_plan: &provider_plan,
                checked_provider_profiles: &checked_provider_profiles,
                build_inputs: oven_build_inputs
                    .as_ref()
                    .ok_or_else(|| CliError::failure("library lacks checked runtime inputs"))?,
                inline_dependencies: &oven_plan_dependencies,
                rustc: oven_rustc.ok_or_else(|| CliError::failure("normal Oven library build omitted rustc"))?,
                target: oven_target.ok_or_else(|| CliError::failure("normal Oven library build omitted target"))?,
                toolchain: oven_toolchain
                    .ok_or_else(|| CliError::failure("normal Oven library build omitted toolchain"))?,
                store: oven_store
                    .as_ref()
                    .ok_or_else(|| CliError::failure("normal Oven library build omitted its bounded store"))?,
                native_sdk_context,
                ordinary_runtime,
                oven_plan_mode,
                rust_edition: rust_edition.clone(),
            },
            &mut timings_ms,
        )?)
    } else {
        None
    };
    record_timing(&mut timings_ms, "library_oven_prepare_profiles", oven_profiles_start);
    // A normal Oven library build derives the compiler-owned vocab helper exclusively from its selected immutable
    // release plan, but only when this project actually declares a vocab companion. Constructing that context for
    // every library made a vocab-free explicit bake require unrelated `incan_vocab` artifacts and repeated work.
    // The selected release plan owns its lease through extraction; compatibility publication retains its explicit
    // boundary and normal consumers remain Cargo-free.
    let oven_vocab_context_start = Instant::now();
    let normal_oven_vocab_context = if manifest.vocab().is_some() {
        if let Some(oven) = oven.as_ref() {
            // Prefer the release selection, but fall back to whatever profile this bake actually prepared.
            // An explicit bake may be narrowed to one profile (see `explicit_bake_profiles`), and the vocab
            // helper is a wasm desugarer whose identity does not depend on the host profile that carried it.
            let release = oven
                .profiles
                .get("release")
                .or_else(|| oven.profiles.values().next())
                .ok_or_else(|| CliError::failure("normal Oven library build prepared no profile selection"))?;
            if release.plan_selection.artifacts().vocab_auxiliary_targets.is_empty() {
                // A receipt-exact project closure may predate project-extension publication or legitimately own a
                // disjoint Rust ABI universe. It must not manufacture compiler-private helper artifacts or invoke
                // Cargo merely because the source also declares a vocab companion. The helper has no project ABI:
                // select it only from the exact compiler-owned stdlib Loaf that authorizes this receipt's
                // release-cohort inputs and target, while the project's selected closure remains the sole authority
                // for its code.
                let base = project_extension_base_loaf(&release.receipt)?.ok_or_else(|| {
                    CliError::failure(
                        "selected Oven project closure has no compiler-owned vocabulary helper and the active Incan release has no compatible release-cohort Loaf; run the explicit release Loaf bake for this compiler version. Normal library builds will not invoke Cargo",
                    )
                })?;
                Some(oven_vocab_direct_rustc_context_from_plan(
                    &oven.rustc,
                    &base.artifact_plan,
                    &base.artifacts,
                    &base.artifact_root,
                )?)
            } else {
                let artifact_root = release.plan_selection.vocab_artifact_root().ok_or_else(|| {
                    CliError::failure(
                        "selected Oven project extension splits a vocabulary auxiliary closure across immutable roots; rebake against a compatible standard-library Loaf",
                    )
                })?;
                Some(oven_vocab_direct_rustc_context_from_plan(
                    &oven.rustc,
                    release.plan_selection.artifact_plan(),
                    release.plan_selection.artifacts(),
                    artifact_root,
                )?)
            }
        } else {
            None
        }
    } else {
        None
    };
    record_timing(
        &mut timings_ms,
        "library_oven_prepare_vocab_context",
        oven_vocab_context_start,
    );
    let mut pending_desugarer_artifact: Option<PendingDesugarerArtifact> = None;
    let vocab_start = Instant::now();
    if let Some(vocab_extraction) = collect_library_vocab_metadata(
        &manifest,
        &project_root,
        (!normal_oven).then_some(managed_target_path.as_path()),
        normal_oven_vocab_context.as_ref(),
    )? {
        pending_desugarer_artifact = vocab_extraction.pending_desugarer_artifact;
        library_manifest.vocab = Some(vocab_extraction.payload);
        library_manifest.soft_keywords.activations = vocab_extraction.compatibility_activations;
    }
    record_timing(&mut timings_ms, "library_collect_vocab_metadata", vocab_start);
    package_desugarer_artifact(&out_dir, pending_desugarer_artifact.as_ref())?;
    if let Some(oven) = oven.as_ref() {
        report_draft.generated = oven_generated_project_report(
            generator.output_dir(),
            &generator.crate_root_path(),
            &generator.output_dir().join("oven"),
        );
        report_draft.cargo = None;
        // Report identities from the release selection when present, else the profile that was prepared.
        let release = oven
            .profiles
            .get("release")
            .or_else(|| oven.profiles.values().next())
            .ok_or_else(|| CliError::failure("normal Oven library build prepared no profile selection"))?;
        report_draft.oven = Some(BuildOvenReport {
            receipt_identity: release.receipt.identity.clone(),
            build_unit_identity: release.receipt.build_unit_identity.clone(),
            plan_identity: release.plan_selection.report_identity(),
        });
        report_draft.notes = vec![
            "Oven Alpha selected a receipt-bound direct-rustc plan; normal library execution did not invoke Cargo or inspect a Cargo target directory.".to_string(),
        ];
    }
    record_timing(&mut timings_ms, "library_generate_rust", codegen_start);
    record_timing(&mut timings_ms, "library_prepare_total", prepare_start);

    let checked_requirements = if metadata_preparation.is_some() {
        match checked_requirement_contract(
            &manifest,
            &compilation_session,
            &source_requirements,
            &library_manifest,
            &inline_imports,
            &selected_exports,
            &project_version,
            &modules,
            lib_module,
            &metadata_query_paths,
            report_draft.backend.clone(),
        ) {
            Ok(contract) => {
                if ordinary_native.is_some() {
                    contract.require_support_only_native()?;
                }
                Some(contract)
            }
            Err(error) => {
                if ordinary_native.is_some() {
                    return Err(error);
                }
                tracing::debug!(reason = %error, "checked library replay contract is unavailable");
                None
            }
        }
    } else {
        None
    };
    // The caller namespace names entrypoint-relative declarations. Keep canonical identities intact while removing
    // the checked entrypoint module prefix from this separate caller-selection projection.
    let mut checked_exports = selected_exports;
    for export in &mut checked_exports {
        if export.identity.source_path.starts_with(&lib_module.path_segments) {
            export.identity.source_path.drain(..lib_module.path_segments.len());
        }
    }

    let pending_metadata = match (metadata_preparation, checked_requirements) {
        (Some(preparation), Some(requirements)) => Some(metadata_replay::PendingMetadataPublication {
            preparation,
            session: compilation_session,
            project: manifest,
            requirements,
        }),
        _ => None,
    };

    Ok(LibraryPreparation::Project(Box::new(PreparedLibraryProject {
        checked_exports,
        executable_surface,
        generator,
        project_root,
        entrypoint: lib_entry,
        out_dir,
        manifest_path,
        library_manifest,
        timings_ms,
        report: report_draft,
        oven,
        metadata_owner: None,
        pending_metadata,
        #[cfg(feature = "rust_inspect")]
        rust_inspect_manifest_dir: rust_inspect_manifest_dir
            .as_ref()
            .map(|workspace| workspace.manifest_dir().to_path_buf()),
    })))
}

/// Capture the same checked planning contract for ordinary and standard package preparation.
#[allow(clippy::too_many_arguments)]
fn checked_requirement_contract(
    manifest: &oven_model::manifest::ProjectManifest,
    session: &CompilationSession,
    requirements: &incan_provider::requirements::ProjectRequirements,
    library_manifest: &LibraryManifest,
    imports: &[incan_provider::dependency_resolver::InlineRustImport],
    exports: &[CheckedNamedExport],
    version: &str,
    modules: &[ParsedModule],
    entry: &ParsedModule,
    rust_abi_queries: &[String],
    backend: Option<crate::backend::selection::BackendExecutionReceipt>,
) -> CliResult<crate::build::library_metadata::requirements::CheckedLibraryRequirements> {
    crate::build::library_metadata::validate_required_rust_abi(
        library_manifest,
        &rust_abi_queries.iter().cloned().collect(),
    )?;
    let source_modules = modules
        .iter()
        .map(|module| {
            let relative = module
                .file_path
                .strip_prefix(manifest.project_root())
                .map_err(|_| CliError::failure("checked module source has no portable package binding"))?
                .to_str()
                .ok_or_else(|| CliError::failure("checked module source is not UTF-8"))?
                .replace('\\', "/");
            Ok((relative, module.path_segments.clone()))
        })
        .collect::<CliResult<BTreeMap<_, _>>>()?;
    let provider_plan = session.provider_plan_for_modules(modules)?;
    crate::build::library_metadata::requirements::CheckedLibraryRequirements::capture(
        crate::build::library_metadata::requirements::CheckedLibraryCapture {
            project: manifest,
            index: &session.library_manifest_index,
            requirements,
            imports,
            exports,
            version,
            used_module_paths: session.provider_module_paths(modules),
            source_modules,
            entry_module: entry.path_segments.clone(),
            rust_abi_queries: rust_abi_queries.iter().cloned().collect(),
            native_demands: crate::build::library_metadata::requirements::capture_checked_native_demands(
                manifest,
                modules,
                requirements,
                provider_plan.as_ref(),
                &rust_abi_queries.iter().cloned().collect(),
            )?,
            rust_extern_paths: rust_extern_report_paths(&collect_rust_extern_contexts(modules)),
            backend,
        },
    )
}

#[cfg(test)]
mod admitted_preparation_tests;
