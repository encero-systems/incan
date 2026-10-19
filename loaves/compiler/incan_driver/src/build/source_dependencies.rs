//! Checking an Incan Loaf dependency from source: its checked public contract, derived without compiling or
//! publishing anything, so a consumer is checked against exactly what the dependency exports.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::env;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use sha2::Digest;

use crate::build::library_exports::{LibraryReexportResolver, module_key};
#[cfg(feature = "rust_inspect")]
use crate::build::library_exports::collect_library_rust_abi_query_paths;
#[cfg(feature = "rust_inspect")]
use crate::build::rust_extern::collect_rust_extern_contexts;
use crate::build::{CompiledProviderMetadataInputs, record_timing};
use crate::diagnostics::render_module_warnings;
use crate::error::{CliError, CliResult};
use crate::modules::{imported_module_deps_for_with_provider_plan, module_key_index, register_module_path_segments};
use crate::session::CompilationSession;
#[cfg(feature = "rust_inspect")]
use ::rust_inspect::RustMetadataCache;
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

/// Checked public metadata shared by source inspection and durable library publication.
struct CheckedPublicLibraryMetadata {
    manifest: LibraryManifest,
    selected_exports: Vec<CheckedNamedExport>,
    type_info: BTreeMap<PathBuf, typechecker::TypeCheckInfo>,
    stdlib_cache: StdlibAstCache,
    #[cfg(feature = "rust_inspect")]
    rust_metadata_queries: BTreeSet<String>,
}

/// Load referenced immutable values and nominal layouts from the already selected executable artifacts.
///
/// Checked manifests supply types and callable signatures, but constant values and field defaults belong to their
/// executable records. Only providers that publish that representation enter this context; a linking-only provider
/// remains a canonical external reference, never an excuse to discover its source. The existing resolver owns public
/// membership, dependency closure and physical digest checks. Explicitly uncovered requested types contribute no
/// context; their native linking route remains available without granting executable coverage. Context is not
/// republished as this library's bodies.
fn checked_dependency_executable_context(
    plan: &incan_provider::ProviderPlan,
    checked: &BTreeMap<PathBuf, typechecker::TypeCheckInfo>,
    current_package: &str,
) -> CliResult<Vec<incan_semantics_core::body_ir::BodyIrModule>> {
    use incan_semantics_core::{SemanticSourceTargetKind, SymbolOrigin};
    let published = plan
        .public_artifacts()
        .filter(|artifact| artifact.manifest.contract_metadata.executable_representation.is_some())
        .map(|artifact| artifact.identity.name.as_str())
        .chain(plan.active_sdk_records().filter_map(|record| {
            record
                .manifest
                .as_ref()?
                .contract_metadata
                .executable_representation
                .as_ref()?;
            Some(record.identity.name.as_str())
        }))
        .collect::<BTreeSet<_>>();
    let required = checked
        .values()
        .flat_map(|info| {
            info.references
                .resolved_identities
                .values()
                .chain(info.declarations.resolved_import_identities.values())
                .chain(info.declarations.named_type_identities.values())
                .chain(
                    info.expressions
                        .expression_type_identities
                        .values()
                        .flat_map(|leaves| leaves.values()),
                )
        })
        .filter(|identity| {
            matches!(
                identity.kind,
                SemanticSourceTargetKind::Const
                    | SemanticSourceTargetKind::Model
                    | SemanticSourceTargetKind::Class
                    | SemanticSourceTargetKind::Enum
                    | SemanticSourceTargetKind::Newtype
                    | SemanticSourceTargetKind::TypeAlias
            ) && matches!(&identity.origin, SymbolOrigin::Package { library, .. }
                    if library != current_package && published.contains(library.as_str()))
        })
        .cloned()
        .collect();
    incan_frontend::executable_resolution::resolve_available_executable_context(plan, &required)
        .map(|resolved| resolved.modules)
        .map_err(|error| CliError::failure(format!("checked library dependency representation: {error}")))
}

/// Check the selected package projection once and retain only its public export contract.
///
/// Both publication and an unbaked dependency check must use this projection: importing producer source directly
/// would expose private declarations and trait/default bodies which a public package does not carry.
#[allow(clippy::too_many_arguments)]
pub(crate) fn checked_public_library_metadata(
    manifest: &oven_model::manifest::ProjectManifest,
    compilation_session: &CompilationSession,
    modules: &[ParsedModule],
    provider_plan: &Arc<incan_provider::ProviderPlan>,
    package_identity: (&str, &str),
    timings_ms: &mut BTreeMap<String, u64>,
    #[cfg(feature = "rust_inspect")] rust_inspect_manifest_dir: Option<&Path>,
) -> CliResult<CheckedPublicLibraryMetadata> {
    let (project_name, project_version) = package_identity;
    let lib_module = modules
        .last()
        .ok_or_else(|| CliError::failure("no modules in checked library projection"))?;
    let mut all_errors = String::new();
    let mut checked_exports_by_module: HashMap<String, HashMap<String, Vec<CheckedNamedExport>>> = HashMap::new();
    let mut checked_exports_by_source_module: Vec<(Vec<String>, Vec<CheckedNamedExport>)> = Vec::new();
    let mut api_metadata_modules = Vec::new();
    let module_idx_by_key = module_key_index(modules);
    let mut stdlib_cache = StdlibAstCache::new();
    let mut checked_type_info_by_path = BTreeMap::new();
    #[cfg(feature = "rust_inspect")]
    let mut rust_metadata_queries = BTreeSet::new();

    let typecheck_start = Instant::now();
    for (idx, module) in modules.iter().enumerate() {
        let deps_for_module =
            imported_module_deps_for_with_provider_plan(modules, idx, &module_idx_by_key, provider_plan);
        let mut checker = typechecker::TypeChecker::new();
        checker.stdlib_cache = stdlib_cache.clone();
        checker.set_current_package_identity(incan_frontend::module::declaration_package_identity(
            Some(project_name),
            Some(&module.path_segments),
        ));
        checker.set_current_module_path(Some(module.path_segments.clone()));
        register_module_path_segments(&mut checker, modules);
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
                #[cfg(feature = "rust_inspect")]
                rust_metadata_queries.extend(checker.observed_rust_metadata_queries());
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
            .filter(|&bundle| bundle.publishable)
            .cloned()
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
        #[cfg(feature = "rust_inspect")]
        rust_metadata_queries,
    })
}

/// Derive an unbaked dependency's checked public metadata without generating or compiling native output.
pub(crate) fn checked_source_library_manifest(
    session: &CompilationSession,
    entry: &Path,
) -> CliResult<(LibraryManifest, StdlibAstCache)> {
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
        (name, version),
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
        .package_feature_plan()
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
    Ok((library_manifest, public.stdlib_cache))
}

