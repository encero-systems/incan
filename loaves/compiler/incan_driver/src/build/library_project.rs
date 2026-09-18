//! Preparing a library project: `prepare_library_project`, the one entry every library build, publication and
//! package export goes through.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::env;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use sha2::Sha256;

use crate::backend::selection::digest_output;
use crate::backend::{IrCodegen, ProjectGenerator};
use crate::build::backend_selection::{finalize_backend_receipt, select_and_resolve_backend};
use crate::build::caller_owned::{append_oven_interop_execution_build_inputs, oven_caller_owned_libraries};
use crate::build::library_exports::{
    LibraryReexportResolver, collect_library_rust_abi, collect_library_rust_abi_query_paths, module_key,
    public_ordinal_type_identities, resolve_library_project_root, validate_library_entrypoint,
};
use crate::build::library_outputs::{
    dependency_artifact_skips_canonical_lock, library_output_path, library_rust_inspection_required,
    package_desugarer_artifact, remove_generated_library_self_dependencies,
};
use crate::build::oven_project::project_extension_base_loaf;
use crate::build::plan_authority::{
    compiler_selected_path_authority, declared_rust_libraries_missing_from_selected_plan_with_current_project_paths,
    explicit_bake_profiles, oven_source_inline_dependency_specs, validate_selected_plan_registry_dependencies,
};
use crate::build::plan_selection::{
    format_oven_registry_dependency_requirements, registry_leaf_authority_for_plan_selection,
    select_or_bake_generated_project_plan,
};
use crate::build::provider_compilation::{
    checked_packaged_provider_profiles, checked_provider_compilation_requirements,
    import_packaged_provider_loafs_for_explicit_bake,
};
use crate::build::provider_metadata::{
    collect_unprojected_provider_modules, compiled_provider_metadata, synchronize_projected_provider_dependencies,
};
use crate::build::rust_extern::{collect_rust_extern_contexts, multi_file_output_identity, rust_extern_report_paths};
use crate::build::{
    BackendSelectionOptions, CompiledProviderMetadataInputs, OvenDirectRustcPlanPreparation, OvenPreparedLibrary,
    OvenPreparedLibraryProfile, OvenProjectBakeAuthorityContext, OvenProjectDependencySurface, OvenProjectPlanMode,
    OvenToolchainMaterialization, PreparedLibraryProject, manifest_project_report, packaged_provider_candidates,
    record_timing, source_file_report,
};
use crate::build_report::{
    BuildOvenReport, BuildReportDraft, BuildReportMode, cargo_report, dependencies_report, generated_project_report,
    incan_dependencies_report, interop_report, oven_generated_project_report, semantic_report,
};
use crate::build_unit::oven_build_unit_inputs_with_provider_identities;
use crate::cargo_policy::{CargoPolicy, cargo_command_flags, enforce_project_toolchain_constraint};
use crate::diagnostics::render_module_warnings;
use crate::error::{CliError, CliResult, oven_plan_error, oven_rustc_error};
use crate::generated_cache::resolve_generated_cargo_target;
#[cfg(feature = "rust_inspect")]
use crate::lock::OvenRustInspectSourceAuthorityRequest;
#[cfg(feature = "rust_inspect")]
use crate::lock::RustInspectWorkspaceRequest;
use crate::lock::resolution::{resolve_lock_context, validate_oven_lock_policy};
#[cfg(feature = "rust_inspect")]
use crate::lock::rust_inspect::prepare_rust_inspect_workspace;
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
use incan_frontend::api_metadata::{
    CHECKED_API_METADATA_SCHEMA_VERSION, CheckedApiMetadataPackage, CheckedApiPackageIdentity,
    collect_checked_api_alias_metadata, collect_checked_api_metadata, materialize_api_alias_projections,
    materialize_checked_api_public_namespaces, validate_checked_api_docstrings,
};
use incan_frontend::contract_metadata::{ContractMetadataPackage, read_project_model_bundles};
use incan_frontend::library_exports::{CheckedNamedExport, checked_exports_by_name, collect_checked_public_exports};
use incan_frontend::library_manifest::LibraryManifest;
use incan_frontend::registry_metadata::{
    CHECKED_REGISTRY_METADATA_SCHEMA_VERSION, CheckedRegistryMetadataPackage, CheckedRegistryPackageIdentity,
    collect_checked_registry_metadata, materialize_registry_reexport_projections,
};
use incan_frontend::typechecker::stdlib_loader::StdlibAstCache;
use incan_frontend::{ParsedModule, diagnostics, typechecker};
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
use oven_cargo_compat::provider_compilation_requirements_digest;
use oven_model::lock::CargoFeatureSelection;
use oven_rustc::loaf::{
    OVEN_DEPENDENCY_MISS_SUMMARY, OVEN_LOAF_MISS_GUIDANCE, OVEN_NO_IMPLICIT_DEPENDENCY_BUILD,
    OVEN_SOURCE_COMPILER_VOCAB_SUPPORT_BUILD_INPUT,
};
use oven_rustc::plan::composition::compose_selected_packaged_provider_plan;
use oven_rustc::plan::selection::select_packaged_provider_plans;
use oven_rustc::rustc::{
    materialize_declared_rust_libraries_with_selected_path_authority, resolve_active_rustc, rustc_host_target,
    rustc_identity,
};
use oven_store::{
    OvenGeneratedProjectRequest, generated_project_source_evidence, receipt_generated_project_with_source_evidence,
    write_receipt,
};
use sha2::Digest as _;

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
    backend_options: &BackendSelectionOptions,
) -> CliResult<PreparedLibraryProject> {
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
    let project_version = manifest
        .project
        .as_ref()
        .and_then(|project| project.version.clone())
        .unwrap_or_else(|| "0.1.0".to_string());

    let lib_entry = validate_library_entrypoint(&manifest)?;
    let compilation_session = if normal_oven {
        CompilationSession::discover_for_oven(&lib_entry, package_features, sdk_profile_override)?
    } else {
        crate::session::CompilationSession::discover_with_selections(
            &lib_entry,
            package_features,
            sdk_profile_override,
        )?
    };
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
    let provider_plan = compilation_session.provider_plan_for_modules(&modules)?;
    let compiled_sdk_modules = CompiledSdkModules::from_provider_plan(&provider_plan);
    extend_requirements_with_provider_plan(&mut project_requirements, &provider_plan)?;
    let semantic_sdk_paths = semantic_sdk_path_dependencies(&project_requirements);
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
    let contract_model_bundles = read_project_model_bundles(&project_root, &manifest.contract_model_bundle_paths())
        .map_err(|error| CliError::failure(error.to_string()))?;
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
    merge_project_requirement_dependencies(&mut resolved, &project_requirements)?;
    record_timing(&mut timings_ms, "library_resolve_dependencies", dependency_start);
    #[cfg(feature = "rust_inspect")]
    let metadata_query_paths = collect_library_rust_abi_query_paths(&modules, &rust_extern_contexts);
    #[cfg(not(feature = "rust_inspect"))]
    let metadata_query_paths: Vec<String> = Vec::new();

    let lock_start = Instant::now();
    let artifact_only = env::var_os(INTERNAL_LIBRARY_ARTIFACT_ONLY_ENV).is_some();
    if normal_oven {
        if cargo_no_default_features || cargo_all_features || !cargo_features.cargo_features.is_empty() {
            return Err(CliError::failure(
                "Oven Alpha normal library builds do not accept Cargo feature controls; use Incan package features instead",
            ));
        }
        validate_oven_lock_policy(
            &project_root,
            Some(&manifest),
            &lib_entry,
            &cargo_features,
            &cargo_policy,
            package_features,
            sdk_profile_override,
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
    let mut oven_build_inputs = normal_oven
        .then(|| {
            oven_build_unit_inputs_with_provider_identities(
                &provider_plan,
                &project_requirements,
                &resolved,
                &provider_semantic_identities,
            )
        })
        .transpose()?;
    let source_compiler_vocab_support =
        normal_oven && manifest.vocab().is_some() && oven_cargo_compat::source_compiler_vocab_support_is_available();
    if source_compiler_vocab_support && let Some(build_inputs) = oven_build_inputs.as_mut() {
        // A source-built compiler seals this helper at the explicit publisher boundary. Keep that closure in a
        // distinct build unit so a v0.5.0 plan without it can neither shadow nor become ambiguous with the
        // upgraded receipt. Packaged compilers continue to select their release-cohort helper unchanged.
        build_inputs.insert(
            OVEN_SOURCE_COMPILER_VOCAB_SUPPORT_BUILD_INPUT.to_string(),
            "v1".to_string(),
        );
    }
    let oven_rustc = normal_oven
        .then(resolve_active_rustc)
        .transpose()
        .map_err(|error| CliError::failure(error.to_string()))?;
    let oven_target = oven_rustc
        .as_ref()
        .map(|rustc| rustc_host_target(rustc))
        .transpose()
        .map_err(|error| CliError::failure(error.to_string()))?;
    let oven_toolchain = oven_rustc
        .as_ref()
        .map(|rustc| rustc_identity(rustc))
        .transpose()
        .map_err(|error| CliError::failure(error.to_string()))?;
    let oven_store = normal_oven.then(open_default_oven_store).transpose()?;
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
    let rust_inspect_manifest_dir = if normal_oven {
        !metadata_query_paths.is_empty()
    } else {
        library_rust_inspection_required(artifact_only, &metadata_query_paths)
    }
    .then(|| {
        if normal_oven {
            Ok((
                // Rust inspection is compiler-owned preparation state, not part of the generated provider artifact.
                // Keeping its Cargo target below `target/lib` leaks build-script outputs (including valid symlinks)
                // into the provider integrity boundary and needlessly makes every consumer traverse that cache.
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
        let rust_inspect_manifest_dir = prepare_rust_inspect_workspace(RustInspectWorkspaceRequest {
            project_root: &project_root,
            project_name: project_name.as_str(),
            cargo_package_name: &lock_cargo_package_name,
            rust_edition: manifest.build.as_ref().and_then(|build| build.rust_edition.clone()),
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
                registry_dependencies: &resolved.dependencies,
            }),
            prepared_project_source_authorities: None,
            explicit_oven_bake: normal_oven && oven_plan_mode == OvenProjectPlanMode::ExplicitBake,
        })?
        .ok_or_else(|| CliError::failure("rust-inspect workspace preparation did not return a manifest directory"))?;
        record_timing(&mut timings_ms, "library_rust_inspect_prewarm", rust_inspect_start);
        Ok::<_, CliError>(rust_inspect_manifest_dir)
    })
    .transpose()?;

    let typecheck_start = Instant::now();
    let mut all_errors = String::new();
    let mut checked_exports_by_module: HashMap<String, HashMap<String, Vec<CheckedNamedExport>>> = HashMap::new();
    let mut checked_exports_by_source_module: Vec<(Vec<String>, Vec<CheckedNamedExport>)> = Vec::new();
    let mut api_metadata_modules = Vec::new();
    let module_idx_by_key = module_key_index(&modules);
    let mut stdlib_cache = StdlibAstCache::new();
    let mut checked_type_info_by_path = BTreeMap::new();
    let mut executable_modules = Vec::new();

    for (idx, module) in modules.iter().enumerate() {
        let deps_for_module =
            imported_module_deps_for_with_provider_plan(&modules, idx, &module_idx_by_key, &provider_plan);
        let mut checker = typechecker::TypeChecker::new();
        checker.stdlib_cache = stdlib_cache.clone();
        checker.set_current_package_identity(incan_frontend::module::declaration_package_identity(
            Some(&project_name),
            Some(&module.path_segments),
        ));
        checker.set_current_module_path(Some(module.path_segments.clone()));
        register_module_path_segments(&mut checker, &modules);
        checker.set_declared_crate_names(declared.clone());
        checker.set_provider_plan(Arc::clone(&provider_plan));
        #[cfg(feature = "rust_inspect")]
        if let Some(rust_inspect_manifest_dir) = rust_inspect_manifest_dir.as_ref() {
            checker.set_rust_inspect_manifest_dir(rust_inspect_manifest_dir.manifest_dir().to_path_buf());
        }

        // A provider producer checks its complete source package before publishing the public checked facade.
        let check_result = if provider_plan.bootstrap_sdk_namespace_roots().next().is_some() {
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
                executable_modules.push(incan_frontend::body_ir::build_body_ir_module_v0(
                    &module.ast,
                    &module.path_segments,
                    checker.type_info(),
                ));
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
    if let Some(rust_inspect_manifest_dir) = rust_inspect_manifest_dir.as_ref() {
        RustMetadataCache::new()
            .persist_manifest_dir(rust_inspect_manifest_dir.manifest_dir())
            .map_err(|error| {
                CliError::failure(format!(
                    "failed to persist batched Rust inspection metadata for {}: {error}",
                    rust_inspect_manifest_dir.manifest_dir().display()
                ))
            })?;
    }
    record_timing(&mut timings_ms, "library_typecheck_modules", typecheck_start);

    let api_validation_start = Instant::now();
    materialize_api_alias_projections(&mut api_metadata_modules);
    let registry_module_path = |module: &ParsedModule| {
        if module.file_path == lib_entry {
            vec!["lib".to_string()]
        } else {
            module.path_segments.clone()
        }
    };
    let mut registry_metadata_modules = modules
        .iter()
        .filter_map(|module| {
            checked_type_info_by_path.get(&module.file_path).map(|type_info| {
                collect_checked_registry_metadata(type_info, registry_module_path(module), project_name.as_str())
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
    record_timing(&mut timings_ms, "library_validate_api_metadata", api_validation_start);

    std::fs::create_dir_all(&out_dir)
        .map_err(|error| CliError::failure(format!("failed to create {}: {error}", out_dir.display())))?;

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
    record_timing(&mut timings_ms, "library_resolve_exports", export_start);

    let manifest_start = Instant::now();
    let project_license = manifest.project.as_ref().and_then(|project| project.license.clone());

    let mut library_manifest =
        LibraryManifest::from_checked_exports(project_name.clone(), project_version.clone(), &selected_exports);
    library_manifest.contract_metadata.models = ContractMetadataPackage::new(
        contract_model_bundles
            .into_iter()
            .filter(|bundle| bundle.publishable)
            .collect(),
    );
    let mut checked_api = CheckedApiMetadataPackage {
        schema_version: CHECKED_API_METADATA_SCHEMA_VERSION,
        package: Some(CheckedApiPackageIdentity {
            name: project_name.clone(),
            version: Some(project_version.clone()),
        }),
        modules: api_metadata_modules,
        public_namespaces: Vec::new(),
    };
    materialize_checked_api_public_namespaces(&mut checked_api)
        .map_err(|error| CliError::failure(format!("failed to publish checked module namespaces: {error}")))?;
    library_manifest
        .contract_metadata
        .identity_graph
        .extend_checked_api_exports(&project_name, &checked_api, &checked_exports_by_source_module)
        .map_err(|error| CliError::failure(format!("failed to publish checked module identities: {error}")))?;
    library_manifest.contract_metadata.api = Some(checked_api);
    let public_identities =
        incan_frontend::library_manifest::published_layout::public_executable_identities(&library_manifest);
    let unrepresentable = executable_modules
        .iter()
        .flat_map(|module| module.bodies.iter())
        .filter(|body| crate::backend::replacement::validate_direct_body_profile(body).is_err())
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
    let mut registry_metadata = CheckedRegistryMetadataPackage {
        schema_version: CHECKED_REGISTRY_METADATA_SCHEMA_VERSION,
        package: Some(CheckedRegistryPackageIdentity {
            name: project_name.clone(),
            version: Some(project_version.clone()),
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
    #[cfg(feature = "rust_inspect")]
    if let Some(rust_inspect_manifest_dir) = rust_inspect_manifest_dir.as_ref() {
        library_manifest.rust_abi =
            collect_library_rust_abi(rust_inspect_manifest_dir.manifest_dir(), &metadata_query_paths)?;
    }
    record_timing(&mut timings_ms, "library_build_manifest_metadata", manifest_start);
    let manifest_path = out_dir.join(format!("{project_name}.incnlib"));

    // ---- Backend selection (#986) — declared before codegen, refused visibly if unavailable ----
    let (backend_selection, backend_executed) = select_and_resolve_backend(backend_options, &modules)?;

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
    let rust_edition = manifest.build.as_ref().and_then(|build| build.rust_edition.clone());
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
    let backend_receipt = finalize_backend_receipt(&backend_selection, backend_executed, backend_output_identity)?;
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
        let rustc = oven_rustc.ok_or_else(|| CliError::failure("normal Oven library build omitted rustc"))?;
        let target = oven_target.ok_or_else(|| CliError::failure("normal Oven library build omitted target"))?;
        let toolchain =
            oven_toolchain.ok_or_else(|| CliError::failure("normal Oven library build omitted toolchain"))?;
        let store = oven_store
            .as_ref()
            .ok_or_else(|| CliError::failure("normal Oven library build omitted its bounded store"))?;
        let mut profiles = BTreeMap::new();
        let oven_receipt_source_evidence_start = Instant::now();
        let mut source_evidence_request = OvenGeneratedProjectRequest::new(
            &project_root,
            &project_name,
            &project_version,
            target.clone(),
            toolchain.clone(),
            "debug",
            Vec::new(),
        )
        .with_generated_source("generated-root", generator.crate_root_path())
        .with_generated_source_tree("generated-source-tree", generator.output_dir().join("src"));
        for (name, value) in oven_build_inputs.as_ref().into_iter().flat_map(|inputs| inputs.iter()) {
            source_evidence_request = source_evidence_request.with_build_unit_input(name.clone(), value.clone());
        }
        let generated_source_evidence = generated_project_source_evidence(&source_evidence_request)
            .map_err(|error| CliError::failure(error.to_string()))?;
        record_timing(
            &mut timings_ms,
            "library_oven_receipt_source_evidence",
            oven_receipt_source_evidence_start,
        );
        for profile in explicit_bake_profiles() {
            let mut receipt_request = OvenGeneratedProjectRequest::new(
                &project_root,
                &project_name,
                &project_version,
                target.clone(),
                toolchain.clone(),
                profile,
                Vec::new(),
            )
            .with_generated_source("generated-root", generator.crate_root_path())
            .with_generated_source_tree("generated-source-tree", generator.output_dir().join("src"));
            for (name, value) in oven_build_inputs.as_ref().into_iter().flat_map(|inputs| inputs.iter()) {
                receipt_request = receipt_request.with_build_unit_input(name.clone(), value.clone());
            }
            let provider_candidates = packaged_provider_candidates(&checked_provider_profiles, profile);
            let selected_provider_inputs =
                select_packaged_provider_plans(store, &provider_candidates).map_err(oven_plan_error)?;
            let provider_compilations = checked_provider_compilation_requirements(
                &selected_provider_inputs,
                &checked_provider_profiles,
                profile,
                oven_build_inputs
                    .as_ref()
                    .ok_or_else(|| CliError::failure("library lacks checked runtime inputs"))?,
            )?;
            if !provider_compilations.is_empty() {
                receipt_request = receipt_request.with_build_unit_input(
                    "provider-compilation-requirements",
                    provider_compilation_requirements_digest(&provider_compilations)
                        .map_err(|error| CliError::failure(error.to_string()))?,
                );
            }
            let receipt = receipt_generated_project_with_source_evidence(&receipt_request, &generated_source_evidence)
                .map_err(|error| CliError::failure(error.to_string()))?;
            let receipt_path = if profile == "release" {
                oven_store::default_receipt_path(&project_root)
            } else {
                oven_store::default_receipt_path(&project_root).with_file_name("library-debug-receipt.json")
            };
            write_receipt(&receipt, receipt_path.clone()).map_err(|error| CliError::failure(error.to_string()))?;
            let required_registry_dependencies = format_oven_registry_dependency_requirements(&oven_plan_dependencies);
            let oven_select_direct_rustc_plan_start = Instant::now();
            // An imported package Loaf is sufficient only for consume-only commands. An explicit library bake must
            // instead publish the library's own direct registry roots with its complete generated source closure.
            let packaged_provider_selection = if oven_plan_mode == OvenProjectPlanMode::ConsumeOnly {
                compose_selected_packaged_provider_plan(selected_provider_inputs, &provider_candidates, &receipt)
                    .map_err(oven_plan_error)?
            } else {
                None
            };
            let plan_preparation = if let Some(selection) = packaged_provider_selection {
                Some(OvenDirectRustcPlanPreparation {
                    plan_selection: selection,
                    materialization: OvenToolchainMaterialization::Reused,
                    cargo_process_started: false,
                })
            } else {
                select_or_bake_generated_project_plan(
                    oven_plan_mode,
                    store,
                    &receipt,
                    OvenProjectDependencySurface {
                        selection: &oven_plan_dependencies,
                    provider_compilations: &provider_compilations,
                    },
                    generator.output_dir(),
                    &generator.crate_root_path(),
                    &rustc,
                )?
            }
            .ok_or_else(|| {
                CliError::failure(format!(
                    "{}. `incan build --lib` {}. {} (Needs: {}. `{profile}` build record {}; generated project: {}; receipt: {}.)",
                    OVEN_DEPENDENCY_MISS_SUMMARY,
                    OVEN_NO_IMPLICIT_DEPENDENCY_BUILD,
                    OVEN_LOAF_MISS_GUIDANCE,
                    required_registry_dependencies,
                    receipt.identity,
                    generator.output_dir().display(),
                    receipt_path.display(),
                ))
            })?;
            record_timing(
                &mut timings_ms,
                "library_oven_select_direct_rustc_plan",
                oven_select_direct_rustc_plan_start,
            );
            let plan_selection = plan_preparation.plan_selection;
            let oven_validate_direct_rustc_plan_start = Instant::now();
            let registry_authority = registry_leaf_authority_for_plan_selection(&plan_selection)?;
            let full_artifact_plan = plan_selection.artifact_plan();
            let artifact_plan = plan_selection
                .source_artifact_plan("generated-root")
                .map_err(oven_rustc_error)?;
            validate_selected_plan_registry_dependencies(
                &oven_plan_dependencies,
                &artifact_plan,
                registry_authority.as_ref(),
                profile,
            )?;
            let inline_libraries = declared_rust_libraries_missing_from_selected_plan_with_current_project_paths(
                oven_inline_rust_dependencies.as_deref().unwrap_or_default(),
                &artifact_plan,
                plan_selection.seals_current_project_path_dependencies(),
            );
            let selected_path_authority = compiler_selected_path_authority(full_artifact_plan, Some(&provider_plan));
            record_timing(
                &mut timings_ms,
                "library_oven_validate_direct_rustc_plan",
                oven_validate_direct_rustc_plan_start,
            );
            let oven_prepare_caller_owned_libraries_start = Instant::now();
            let mut caller_owned_libraries = oven_caller_owned_libraries(&provider_plan, profile)?;
            caller_owned_libraries.extend(
                materialize_declared_rust_libraries_with_selected_path_authority(
                    &generator.output_dir().join("oven").join("inline-rust"),
                    &rustc,
                    &target,
                    profile,
                    &inline_libraries,
                    registry_authority.as_ref(),
                    selected_path_authority.as_ref(),
                )
                .map_err(oven_rustc_error)?,
            );
            record_timing(
                &mut timings_ms,
                "library_oven_prepare_caller_owned_libraries",
                oven_prepare_caller_owned_libraries_start,
            );
            caller_owned_libraries.sort_by(|left, right| left.crate_name.cmp(&right.crate_name));
            if caller_owned_libraries
                .windows(2)
                .any(|pair| pair[0].crate_name == pair[1].crate_name)
            {
                return Err(CliError::failure(
                    "Oven Alpha resolved duplicate caller-owned Rust library crate names while preparing a library",
                ));
            }
            profiles.insert(
                profile.to_string(),
                OvenPreparedLibraryProfile {
                    receipt,
                    plan_selection,
                    materialization: plan_preparation.materialization,
                    provider_plan: provider_plan.clone(),
                    caller_owned_libraries,
                },
            );
        }
        Some(OvenPreparedLibrary {
            rustc,
            crate_name: ProjectGenerator::rust_target_name(&project_name),
            rust_edition: rust_edition.clone().unwrap_or_else(|| "2024".to_string()),
            profiles,
        })
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

    Ok(PreparedLibraryProject {
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
        #[cfg(feature = "rust_inspect")]
        rust_inspect_manifest_dir: rust_inspect_manifest_dir
            .as_ref()
            .map(|workspace| workspace.manifest_dir().to_path_buf()),
    })
}
