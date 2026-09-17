//! Preparing a project for Oven: the direct-rustc plan it selects or bakes, the interop bootstrap, and the
//! generated-project compatibility plan the Cargo path still needs.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;
use std::{env, fs};

use crate::backend::selection::digest_output;
use crate::backend::{IrCodegen, ProjectGenerator};
use crate::build::backend_selection::{finalize_backend_receipt, select_and_resolve_backend};
use crate::build::bake::discover_oven_bake_project_targets;
use crate::build::caller_owned::{append_oven_interop_execution_build_inputs, oven_caller_owned_libraries};
use crate::build::library_exports::resolve_library_project_root;
use crate::build::output_selection::load_current_project_registry_source_authorities;
use crate::build::plan_authority::{
    compiler_selected_path_authority, declared_rust_libraries_missing_from_selected_plan_with_current_project_paths,
    explicit_bake_profiles, oven_source_inline_dependency_specs, validate_selected_plan_registry_dependencies,
};
use crate::build::plan_selection::{
    format_oven_registry_dependency_requirements, interop_final_plan_required_error,
    receipt_requires_final_interop_plan, registry_leaf_authority_for_plan_selection,
    select_or_bake_generated_project_plan, select_published_project_plan,
};
use crate::build::provider_compilation::{
    checked_packaged_provider_profiles, checked_provider_compilation_requirements,
    import_packaged_provider_loafs_for_explicit_bake,
};
use crate::build::rust_extern::multi_file_output_identity;
use crate::build::source_authority::{interop_bootstrap_receipt_path, prepared_oven_receipt_path};
use crate::build::{
    BackendSelectionOptions, OvenBakeProjectTarget, OvenDirectRustcPlanPreparation, OvenPreparedProject,
    OvenProjectBakeAuthorityContext, OvenProjectDependencySurface, OvenProjectPlanMode, OvenToolchainMaterialization,
    manifest_project_report, oven_executable_entrypoint_evidence_key, packaged_provider_candidates, record_timing,
    source_file_report,
};
use crate::build_report::{
    BuildOvenReport, BuildReportDraft, BuildReportMode, dependencies_report, incan_dependencies_report, interop_report,
    oven_generated_project_report, semantic_report,
};
use crate::build_unit::oven_build_unit_inputs;
use crate::cargo_policy::{CargoPolicy, enforce_project_toolchain_constraint};
use crate::error::{CliError, CliResult, oven_plan_error, oven_rustc_error};
#[cfg(feature = "rust_inspect")]
use crate::lock::OvenRustInspectSourceAuthorityRequest;
#[cfg(feature = "rust_inspect")]
use crate::lock::RustInspectWorkspaceRequest;
#[cfg(feature = "rust_inspect")]
use crate::lock::registry_sources::prepare_project_registry_source_authorities;
use crate::lock::resolution::validate_oven_lock_policy;
#[cfg(feature = "rust_inspect")]
use crate::lock::rust_inspect::prepare_rust_inspect_workspace;
use crate::modules::{build_source_map, collect_rust_dependency_uses, format_dependency_error};
use crate::oven_store::open_default_oven_store;
use crate::project::{collect_incan_source_files, resolve_project_root, validate_output_dir};
#[cfg(feature = "rust_inspect")]
use crate::rust_inspect_workspace::collect_rust_inspect_derive_probe_paths;
#[cfg(feature = "rust_inspect")]
use crate::rust_inspect_workspace::collect_rust_inspect_query_paths;
#[cfg(feature = "rust_inspect")]
use crate::rust_inspect_workspace::collect_rust_inspect_query_paths_from_programs;
use crate::session::CompilationSession;
use incan_frontend::{ParsedModule, diagnostics};
use incan_lang::version::INCAN_VERSION;
use incan_provider::FeatureSelection;
use incan_provider::compiled_sdk::CompiledSdkModules;
use incan_provider::dependency_resolver::resolve_reachable_dependencies;
use incan_provider::inventory::extend_requirements_with_provider_plan;
use incan_provider::requirements::{collect_project_requirements, merge_project_requirement_dependencies};
use oven_cargo_compat::cargo_process::resolved_cargo_executable;
use oven_cargo_compat::{
    OvenCompilerMacroDependency, OvenLegacyCargoBaseLoaf, OvenLegacyCargoDirectDependencyClosure,
    OvenLegacyCargoPrepareRequest, OvenLegacyCargoPublicationKind, direct_rustc_reusable_project_plan_environment,
    prepare_direct_rustc_plan, provider_compilation_requirements_digest,
};
use oven_model::lock::CargoFeatureSelection;
use oven_model::manifest::DependencySpec;
use oven_rustc::loaf::{
    OVEN_DEPENDENCY_MISS_SUMMARY, OVEN_LOAF_ENV, OVEN_LOAF_MISS_GUIDANCE, OVEN_NESTED_DEPENDENCY_MISS_SUMMARY,
    OVEN_NO_IMPLICIT_DEPENDENCY_BUILD, OvenToolchainLoaf, acquire_active_release_runtime_foundation,
    acquire_committed_release_runtime_foundation, resolve_compiler_owned_loaf_for_registry_dependencies,
};
use oven_rustc::plan::OvenDirectRustcPlanSelection;
use oven_rustc::plan::composition::compose_selected_packaged_provider_plan;
use oven_rustc::plan::selection::select_packaged_provider_plans;
use oven_rustc::rustc::{
    materialize_declared_rust_libraries_with_selected_path_authority, resolve_active_rustc, rustc_host_target,
    rustc_identity,
};
use oven_store::store::OvenStore;
use oven_store::{OvenGeneratedProjectRequest, receipt_generated_project, write_receipt};

/// Analyze, generate, receipt, and select the direct-Rustc plan for one normal Oven executable command.
#[allow(clippy::too_many_arguments)]
pub fn prepare_oven_project(
    file_path: &str,
    output_dir: Option<&str>,
    cargo_policy: &CargoPolicy,
    package_features: &FeatureSelection,
    sdk_profile_override: Option<&str>,
    cargo_features: Vec<String>,
    cargo_no_default_features: bool,
    cargo_all_features: bool,
    profile: &str,
    oven_plan_mode: OvenProjectPlanMode,
    authority_context: Option<&mut OvenProjectBakeAuthorityContext>,
    backend_options: &BackendSelectionOptions,
) -> CliResult<OvenPreparedProject> {
    if cargo_no_default_features || cargo_all_features || !cargo_features.is_empty() {
        return Err(CliError::failure(
            "Oven Alpha normal build and run do not accept Cargo feature controls; use Incan package features instead",
        ));
    }
    // Phase laps of the prepare step, reported in the build report's `timings_ms` so a slow no-change rebuild can be
    // attributed to the stage that spent the time rather than to "prepare" as a whole (#1111).
    let mut prepare_timings = BTreeMap::new();
    let mut lap = Instant::now();
    let normalized_file_path = if Path::new(file_path).is_absolute() {
        PathBuf::from(file_path)
    } else {
        env::current_dir()
            .map_err(|error| CliError::failure(format!("failed to determine current directory: {error}")))?
            .join(file_path)
    };
    let path = normalized_file_path.as_path();
    let inferred_project_root = resolve_project_root(path);
    let compilation_session = CompilationSession::discover_for_oven(path, package_features, sdk_profile_override)?;
    let manifest = compilation_session.manifest.clone();
    if let Some(manifest) = manifest.as_ref() {
        enforce_project_toolchain_constraint(manifest)?;
    }
    let modules =
        crate::modules::collect_modules_detailed_with_session(normalized_file_path.clone(), &compilation_session)
            .map_err(|failure| CliError::failure(failure.render_human()))?;
    let Some(main_module) = modules.last() else {
        return Err(CliError::failure("No modules found"));
    };
    record_timing(&mut prepare_timings, "prepare_session_and_modules", lap);
    lap = Instant::now();
    // ---- Backend selection (#986) — declared before codegen, refused visibly if unavailable ----
    let (backend_selection, backend_executed) = select_and_resolve_backend(backend_options, &modules)?;
    let dep_modules = &modules[..modules.len() - 1];
    let project_root = manifest
        .as_ref()
        .map(|manifest| manifest.project_root().to_path_buf())
        .unwrap_or(inferred_project_root);
    let entrypoint_evidence_key = oven_executable_entrypoint_evidence_key(&project_root, path)?;
    let package_feature_plan = compilation_session.package_feature_plan.clone();
    let library_manifest_index = compilation_session.library_manifest_index.clone();
    let mut project_requirements = collect_project_requirements(&modules, &library_manifest_index)?;
    record_timing(&mut prepare_timings, "prepare_backend_selection", lap);
    lap = Instant::now();
    let provider_plan = compilation_session.provider_plan_for_modules(&modules)?;
    record_timing(&mut prepare_timings, "prepare_provider_plan", lap);
    lap = Instant::now();
    let mut caller_owned_libraries = oven_caller_owned_libraries(&provider_plan, profile)?;
    let compiled_sdk_modules = CompiledSdkModules::from_provider_plan(&provider_plan);
    extend_requirements_with_provider_plan(&mut project_requirements, &provider_plan)?;
    ensure_loaf_stdlib_facets(&mut project_requirements.stdlib_facets, loaf_codegen_mode());
    let emitted_dep_modules: Vec<&ParsedModule> = dep_modules
        .iter()
        .filter(|module| !compiled_sdk_modules.contains_emission_path(&module.path_segments))
        .collect();

    let project_name = manifest
        .as_ref()
        .and_then(|manifest| manifest.project.as_ref().and_then(|project| project.name.clone()))
        .unwrap_or_else(|| {
            path.file_stem()
                .and_then(|stem| stem.to_str())
                .unwrap_or("incan_project")
                .to_string()
        });
    let project_version = manifest
        .as_ref()
        .and_then(|manifest| manifest.project.as_ref().and_then(|project| project.version.clone()))
        .unwrap_or_else(|| "0.1.0".to_string());
    // Normal Oven output belongs to the caller's project, not the compiler process's current directory. A caller may
    // still explicitly choose an output destination; only the no-flag default is project-local.
    let out_dir = match output_dir {
        Some(output_dir) => {
            validate_output_dir(output_dir)?;
            PathBuf::from(output_dir)
        }
        None => project_root.join("target").join("incan").join(&project_name),
    };

    let mut codegen = IrCodegen::new();
    // A source-emitted provider module must retain its public implementation closure. Its public protocol methods
    // can construct public adapter models declared later in the same module even when the root program does not name
    // those adapters directly. Pruning them made a normal Oven source projection ill-formed (`FallibleIterator.map`
    // referenced an omitted `MapFallibleIterator`). A completely dependency-free program can still use the smaller
    // projection; the named Loaf publisher always retains the complete compiler-owned provider envelope.
    codegen.set_preserve_dependency_public_items(preserve_source_dependency_public_items(
        loaf_codegen_mode(),
        emitted_dep_modules.len(),
    ));
    codegen.set_registry_package_identity(Some(project_name.clone()));
    codegen.set_root_source_module_name(path.file_stem().and_then(|stem| stem.to_str()).map(str::to_string));
    if let Some(manifest) = manifest.as_ref() {
        codegen.set_declared_crate_names(manifest.declared_rust_crate_names());
    }
    codegen.set_provider_plan(Arc::clone(&provider_plan));
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

    let mut generator = ProjectGenerator::new(&out_dir, project_name.as_str(), true);
    if let Some(project) = manifest.as_ref().and_then(|manifest| manifest.project.as_ref()) {
        generator.set_package_metadata(project.version.clone(), project.license.clone());
    }
    generator.set_provider_plan(&provider_plan);
    generator.set_sdk_path_dependencies(project_requirements.sdk_path_dependencies.clone());
    generator.set_stdlib_facets(project_requirements.stdlib_facets.clone());
    if oven_plan_mode == OvenProjectPlanMode::InteropBootstrap {
        generator.enable_companion_library_target();
    }
    // An interop bootstrap must generate the exact normal executable source closure. It is allowed to publish that
    // closure through the named compatibility boundary, but it cannot pull in publisher-only development inputs or
    // its pre-interop receipt would not be selectable by the later normal direct-rustc consumer.
    generator.set_include_dev_dependencies(oven_plan_mode == OvenProjectPlanMode::ExplicitBake);
    let rust_edition = manifest
        .as_ref()
        .and_then(|manifest| manifest.build.as_ref().and_then(|build| build.rust_edition.clone()))
        .unwrap_or_else(|| "2024".to_string());
    generator.set_rust_edition(Some(rust_edition.clone()));

    let mut source_inline_imports = collect_rust_dependency_uses(main_module, false);
    let mut inline_imports = source_inline_imports.clone();
    for module in &emitted_dep_modules {
        let module_imports = collect_rust_dependency_uses(module, false);
        // A source-backed stdlib module has not yet become an installed compiled SDK artifact, but it still belongs to
        // the compiler-owned provider closure. Its Rust imports may be admitted to the explicit baker so a Loaf
        // captures their exact direct-rustc inputs. Rust imports from any caller-owned module remain a separate Alpha
        // boundary and cannot be smuggled through the standard-library source path.
        if module.path_segments.first().map(String::as_str) != Some("__incan_std") {
            source_inline_imports.extend(module_imports.clone());
        }
        inline_imports.extend(module_imports);
    }
    // `std.*` source modules lower through the compiler-owned standard library facets. ProjectGenerator supplies
    // those crates directly, so their internal `rust.module("incan_std_<facet>::...")` declarations are not
    // user-selected Cargo inputs. Rust's own `std` crate is likewise supplied by the selected compiler. The selected
    // stdlib provider may have its own external Rust closure, which the named publisher records in `inline_imports`;
    // caller-owned Rust imports are materialized only through the narrow direct-rustc path-package seam below.
    source_inline_imports
        .retain(|import| !incan_lang::lang::stdlib::facets::is_facet(&import.crate_name) && import.crate_name != "std");
    let source_inline_crates = source_inline_imports
        .iter()
        .map(|import| import.crate_name.clone())
        .collect::<BTreeSet<_>>();
    inline_imports
        .retain(|import| !incan_lang::lang::stdlib::facets::is_facet(&import.crate_name) && import.crate_name != "std");
    let cargo_features = CargoFeatureSelection {
        cargo_features,
        cargo_no_default_features,
        cargo_all_features,
    }
    .normalized();
    let mut resolved = resolve_reachable_dependencies(manifest.as_ref(), &inline_imports, true, &cargo_features)
        .map_err(|errors| {
            let sources = build_source_map(&modules);
            let message = errors
                .iter()
                .map(|error| format_dependency_error(error, &sources))
                .collect::<String>();
            CliError::failure(message.trim_end())
        })?;
    merge_project_requirement_dependencies(&mut resolved, &project_requirements)?;
    let inline_path_dependencies = oven_source_inline_dependency_specs(&resolved, &source_inline_crates)?;
    record_timing(&mut prepare_timings, "prepare_resolve_dependencies", lap);
    lap = Instant::now();
    // Strict flags are Incan lock promises, not authorization to re-enter the Cargo projection path. The Oven
    // validator recomputes the canonical fingerprint from read-only metadata and fails on a missing or stale lock.
    validate_oven_lock_policy(
        &project_root,
        manifest.as_ref(),
        &normalized_file_path,
        &cargo_features,
        cargo_policy,
        package_features,
        sdk_profile_override,
    )?;
    record_timing(&mut prepare_timings, "prepare_lock_policy", lap);
    lap = Instant::now();
    let mut oven_build_inputs = oven_build_unit_inputs(&provider_plan, &project_requirements, &resolved)?;
    let mut active_runtime_foundation = if loaf_codegen_mode() {
        None
    } else {
        acquire_active_release_runtime_foundation("rust-policy-foundation")
            .map_err(|error| CliError::failure(error.to_string()))?
    };
    let mut rustc = if let Some(held) = active_runtime_foundation.as_ref() {
        held.compiler.rustc().to_path_buf()
    } else {
        resolve_active_rustc().map_err(|error| CliError::failure(error.to_string()))?
    };
    let rustc_target = authority_context
        .as_ref()
        .and_then(|context| context.requested_target.clone())
        .map_or_else(
            || {
                active_runtime_foundation.as_ref().map_or_else(
                    || rustc_host_target(&rustc).map_err(|error| CliError::failure(error.to_string())),
                    |held| {
                        Ok(held
                            .asset
                            .foundation()
                            .selected_graph()
                            .graph()
                            .selection
                            .intent
                            .target
                            .clone())
                    },
                )
            },
            Ok,
        )?;
    let rustc_toolchain = active_runtime_foundation.as_ref().map_or_else(
        || rustc_identity(&rustc).map_err(|error| CliError::failure(error.to_string())),
        |held| {
            Ok(held
                .asset
                .foundation()
                .selected_graph()
                .graph()
                .selection
                .intent
                .toolchain
                .clone())
        },
    )?;
    if active_runtime_foundation.is_some() && rustc_identity(&rustc).map_err(oven_rustc_error)? != rustc_toolchain {
        return Err(CliError::failure(
            "active runtime dependency closure retained a compiler with the wrong toolchain identity".to_string(),
        ));
    }
    if oven_plan_mode != OvenProjectPlanMode::InteropBootstrap {
        append_oven_interop_execution_build_inputs(&mut oven_build_inputs, manifest.as_ref(), &rustc_target)?;
    }
    record_timing(&mut prepare_timings, "prepare_toolchain_identity", lap);
    lap = Instant::now();
    let oven_store = open_default_oven_store()?;

    #[cfg(feature = "rust_inspect")]
    let rust_inspect_manifest_dir = {
        let metadata_query_paths = loaf_rust_inspect_query_paths(&modules, &compilation_session)?;
        let prepared_project_source_authorities = if oven_plan_mode == OvenProjectPlanMode::ConsumeOnly
            && !loaf_codegen_mode()
            && manifest.is_some()
            && !metadata_query_paths.is_empty()
        {
            let authority = load_current_project_registry_source_authorities(&oven_store, &project_root)?
                .ok_or_else(|| {
                    CliError::failure(
                        "Oven Alpha has no source-current project inspection authority; rerun `incan oven bake --project .`",
                    )
                })?;
            Some(prepare_project_registry_source_authorities(authority)?)
        } else {
            None
        };
        let rust_inspect_manifest_dir = prepare_rust_inspect_workspace(RustInspectWorkspaceRequest {
            project_root: &project_root,
            project_name: project_name.as_str(),
            cargo_package_name: project_name.as_str(),
            rust_edition: Some(rust_edition.clone()),
            resolved: &resolved,
            project_requirements: &project_requirements,
            lock_payload: None,
            cargo_lock_projection_root: None,
            clear_cargo_lock: false,
            cargo_policy_flags: Vec::new(),
            cargo_target_dir: &generator.output_dir().join("oven").join("rust-inspect"),
            rust_inspect_query_paths: &metadata_query_paths,
            rust_derive_probe_paths: &collect_rust_inspect_derive_probe_paths(&modules),
            prepare_when_empty: false,
            direct_oven_inspection: true,
            force_direct_prewarm: loaf_codegen_mode(),
            oven_source_authority: Some(OvenRustInspectSourceAuthorityRequest {
                project_version: &project_version,
                target: &rustc_target,
                toolchain: &rustc_toolchain,
                profile,
                features: &cargo_features.cargo_features,
                build_unit_inputs: &oven_build_inputs,
                registry_dependencies: &resolved.dependencies,
            }),
            prepared_project_source_authorities,
            explicit_oven_bake: oven_plan_mode == OvenProjectPlanMode::ExplicitBake,
        })?;
        if let Some(manifest_dir) = rust_inspect_manifest_dir.as_ref() {
            codegen.set_rust_inspect_manifest_dir(manifest_dir.manifest_dir().to_path_buf());
        }
        rust_inspect_manifest_dir
    };

    record_timing(&mut prepare_timings, "prepare_store_and_rust_inspect", lap);
    lap = Instant::now();
    let analysis = compilation_session
        .analyze_modules(
            &modules,
            #[cfg(feature = "rust_inspect")]
            rust_inspect_manifest_dir
                .as_ref()
                .map(|workspace| workspace.manifest_dir()),
        )
        .map_err(|failure| CliError::failure(failure.render_human()))?;
    let main_type_info = analysis
        .type_info_for_path(&main_module.file_path)
        .cloned()
        .ok_or_else(|| {
            CliError::failure(format!(
                "missing session analysis for {}",
                main_module.file_path.display()
            ))
        })?;
    let mut dependency_type_info = HashMap::with_capacity(dep_modules.len());
    for module in dep_modules {
        let type_info = analysis
            .type_info_for_path(&module.file_path)
            .cloned()
            .ok_or_else(|| CliError::failure(format!("missing session analysis for {}", module.file_path.display())))?;
        dependency_type_info.insert(module.path_segments.clone(), type_info);
    }
    codegen.set_stdlib_cache(analysis.stdlib_cache().clone());
    codegen.set_prechecked_type_info(main_type_info, dependency_type_info);
    // The Oven executor consumes the generated source directly, but its report remains an inspection surface for
    // the same resolved dependency inputs that produced the receipt. Keep those inputs before moving them into the
    // generator so normal direct-rustc reports do not falsely claim that the project has no Rust dependencies.
    let rust_dependencies = resolved.dependencies.clone();
    let rust_dev_dependencies = resolved.dev_dependencies.clone();
    let requested_provider_profiles = if oven_plan_mode == OvenProjectPlanMode::ExplicitBake {
        explicit_bake_profiles()
    } else {
        vec![profile]
    };
    record_timing(&mut prepare_timings, "prepare_typecheck", lap);
    lap = Instant::now();
    let checked_provider_profiles = checked_packaged_provider_profiles(
        &provider_plan,
        &requested_provider_profiles,
        &rustc_target,
        &rustc_toolchain,
        authority_context,
    )?;
    let oven_plan_dependencies = inline_path_dependencies.clone();
    import_packaged_provider_loafs_for_explicit_bake(oven_plan_mode, &oven_store, &checked_provider_profiles)?;
    let provider_candidates = packaged_provider_candidates(&checked_provider_profiles, profile);
    let selected_provider_inputs =
        select_packaged_provider_plans(&oven_store, &provider_candidates).map_err(oven_plan_error)?;
    let provider_compilations = checked_provider_compilation_requirements(
        &selected_provider_inputs,
        &checked_provider_profiles,
        profile,
        &oven_build_inputs,
    )?;
    generator.set_dependencies(resolved.dependencies);
    generator.set_dev_dependencies(resolved.dev_dependencies);

    record_timing(&mut prepare_timings, "prepare_provider_profiles", lap);
    lap = Instant::now();
    let has_deps = !emitted_dep_modules.is_empty()
        || dep_modules
            .iter()
            .any(|module| compiled_sdk_modules.contains_emission_path(&module.path_segments));
    let backend_output_identity = if has_deps {
        let module_paths = emitted_dep_modules
            .iter()
            .map(|module| module.path_segments.clone())
            .collect::<Vec<_>>();
        let (main_code, rust_modules) = codegen
            .try_generate_multi_file_nested(&main_module.ast, &module_paths)
            .map_err(|error| CliError::failure(format!("Code generation error: {error}")))?;
        generator
            .generate_nested(&main_code, &rust_modules)
            .map_err(|error| CliError::failure(format!("Error generating project: {error}")))?;
        multi_file_output_identity(&main_code, &rust_modules)
    } else {
        let rust_code = codegen
            .try_generate(&main_module.ast)
            .map_err(|error| CliError::failure(format!("Code generation error: {error}")))?;
        generator
            .generate(&rust_code)
            .map_err(|error| CliError::failure(format!("Error generating project: {error}")))?;
        digest_output(&[rust_code.as_str()])
    };
    record_timing(&mut prepare_timings, "prepare_codegen_and_generate", lap);
    lap = Instant::now();
    let backend_receipt = finalize_backend_receipt(&backend_selection, backend_executed, backend_output_identity)?;
    // Not persisted here: `prepare_oven_project` runs for internal/dependency callers too (see
    // `BackendSelectionOptions::default()` call sites), and real compilation (the Oven plan selection and rustc bake
    // below) can still fail after this point. The receipt is instead published by the top-level
    // `build_file_report`/`build_library_report` entry points, once and only once the whole build has actually
    // succeeded (#986).

    let mut receipt_request = OvenGeneratedProjectRequest::new(
        &project_root,
        &project_name,
        &project_version,
        rustc_target,
        &rustc_toolchain,
        profile,
        cargo_features.cargo_features.clone(),
    )
    .with_generated_source("generated-root", generator.crate_root_path())
    .with_generated_source_tree("generated-source-tree", generator.output_dir().join("src"))
    .with_generated_source(&entrypoint_evidence_key, path);
    for (name, value) in &oven_build_inputs {
        receipt_request = receipt_request.with_build_unit_input(name.clone(), value.clone());
    }
    if !provider_compilations.is_empty() {
        receipt_request = receipt_request.with_build_unit_input(
            "provider-compilation-requirements",
            provider_compilation_requirements_digest(&provider_compilations)
                .map_err(|error| CliError::failure(error.to_string()))?,
        );
    }
    let receipt = receipt_generated_project(&receipt_request).map_err(|error| CliError::failure(error.to_string()))?;
    let receipt_path =
        prepared_oven_receipt_path(&project_root, oven_plan_mode, &receipt.intent.target, path, profile)?;
    write_receipt(&receipt, &receipt_path).map_err(|error| CliError::failure(error.to_string()))?;
    record_timing(&mut prepare_timings, "prepare_receipt", lap);
    lap = Instant::now();
    let required_registry_dependencies = format_oven_registry_dependency_requirements(&oven_plan_dependencies);
    // An imported package Loaf is sufficient for a consume-only command, and for an explicit bake of a consumer that
    // declares no direct registry root of its own. The explicit baker must otherwise publish the consumer's own
    // direct registry roots with its complete generated source closure; a provider's catalog must never become the
    // registry authority for a consumer-declared dependency. With nothing consumer-declared there is nothing for it
    // to usurp, and composing the sealed package closures is the route that links a single provider's registry
    // closure once (#1469); rebuilding the consumer against the base Loaf and relinking the provider's closure
    // beside it refused `itoa` twice. A diamond whose providers each compiled one shared unit for themselves is
    // the shape neither route builds: rustc refuses the colliding `StableCrateId`s, and the reconciliation that
    // would keep one compiled instance of every shared registry unit across sealed closures is #1241.
    let consumer_declares_registry_roots = !oven_plan_dependencies.is_empty();
    let packaged_provider_selection = if oven_plan_mode == OvenProjectPlanMode::ConsumeOnly
        || (oven_plan_mode == OvenProjectPlanMode::ExplicitBake && !consumer_declares_registry_roots)
    {
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
            &oven_store,
            &receipt,
            OvenProjectDependencySurface {
                selection: &oven_plan_dependencies,
                provider_compilations: &provider_compilations,
            },
            generator.output_dir(),
            &generator.crate_root_path(),
            &rustc,
        )?
    };
    let plan_preparation = plan_preparation.ok_or_else(|| {
        CliError::failure(format!(
            "{}. `incan build` and `incan run` {}. {} (Needs: {}. Build record {}; generated project: {}; receipt: {}.)",
            OVEN_DEPENDENCY_MISS_SUMMARY,
            OVEN_NO_IMPLICIT_DEPENDENCY_BUILD,
            OVEN_LOAF_MISS_GUIDANCE,
            required_registry_dependencies,
            receipt.identity,
            generator.output_dir().display(),
            receipt_path.display(),
        ))
    })?;
    record_timing(&mut prepare_timings, "prepare_plan_selection", lap);
    lap = Instant::now();
    let plan_selection = plan_preparation.plan_selection;
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
        &inline_path_dependencies,
        &artifact_plan,
        plan_selection.seals_current_project_path_dependencies(),
    );
    let selected_path_authority = compiler_selected_path_authority(full_artifact_plan, Some(&provider_plan));
    caller_owned_libraries.extend(
        materialize_declared_rust_libraries_with_selected_path_authority(
            &generator.output_dir().join("oven").join("inline-rust"),
            &rustc,
            &receipt.intent.target,
            profile,
            &inline_libraries,
            registry_authority.as_ref(),
            selected_path_authority.as_ref(),
        )
        .map_err(oven_rustc_error)?,
    );
    caller_owned_libraries.sort_by(|left, right| left.crate_name.cmp(&right.crate_name));
    if caller_owned_libraries
        .windows(2)
        .any(|pair| pair[0].crate_name == pair[1].crate_name)
    {
        return Err(CliError::failure(
            "Oven Alpha resolved duplicate caller-owned Rust library crate names",
        ));
    }

    record_timing(&mut prepare_timings, "prepare_registry_validation", lap);
    let report = BuildReportDraft {
        mode: BuildReportMode::Executable,
        profile: profile.to_string(),
        project: manifest_project_report(manifest.as_ref(), &project_name, &project_root),
        entrypoint: Some(normalized_file_path.to_string_lossy().to_string()),
        library_root: None,
        source_files: source_file_report(&modules),
        generated: oven_generated_project_report(
            generator.output_dir(),
            &generator.crate_root_path(),
            &generator.output_dir().join("oven"),
        ),
        artifacts: Vec::new(),
        dependencies: dependencies_report(
            &rust_dependencies,
            &rust_dev_dependencies,
            manifest
                .as_ref()
                .map(|manifest| incan_dependencies_report(manifest.library_dependencies().iter().collect()))
                .unwrap_or_default(),
            project_requirements.stdlib_facets.clone(),
        ),
        semantic: semantic_report(
            compilation_session.sdk_inventory.as_deref(),
            compilation_session.sdk_components.as_ref(),
            package_feature_plan.as_ref(),
            &provider_plan,
        ),
        cargo: None,
        oven: Some(BuildOvenReport {
            receipt_identity: receipt.identity.clone(),
            build_unit_identity: receipt.build_unit_identity.clone(),
            plan_identity: plan_selection.report_identity(),
        }),
        interop: interop_report(&inline_imports, Vec::new(), Vec::new()),
        notes: vec![
            "Oven Alpha selected a receipt-bound direct-rustc plan; normal execution did not invoke Cargo or inspect a Cargo target directory.".to_string(),
        ],
        backend: Some(backend_receipt),
    };
    let runtime_foundation = match &plan_selection {
        OvenDirectRustcPlanSelection::ToolchainLoaf(native) => {
            let Some(root) = native
                .release_envelope_root()
                .map_err(|error| CliError::failure(error.to_string()))?
            else {
                return Ok(OvenPreparedProject {
                    generator,
                    project_root,
                    entrypoint: normalized_file_path,
                    provider_plan,
                    receipt,
                    plan_selection,
                    runtime_foundation: None,
                    materialization: plan_preparation.materialization,
                    cargo_process_started: plan_preparation.cargo_process_started,
                    rustc,
                    crate_name: ProjectGenerator::rust_target_name(&project_name),
                    rust_edition,
                    caller_owned_libraries,
                    report,
                    prepare_timings,
                    #[cfg(feature = "rust_inspect")]
                    rust_inspect_manifest_dir: rust_inspect_manifest_dir
                        .as_ref()
                        .map(|workspace| workspace.manifest_dir().to_path_buf()),
                });
            };
            let held = if let Some(held) = active_runtime_foundation.take() {
                held
            } else {
                acquire_committed_release_runtime_foundation(&root, "rust-policy-foundation")
                    .map_err(|error| CliError::failure(error.to_string()))?
                    .ok_or_else(|| {
                        CliError::failure(
                            "selected ToolchainLoaf release has no admitted runtime dependency foundation".to_string(),
                        )
                    })?
            };
            if held.closure.is_none() {
                return Err(CliError::failure(
                    "selected ToolchainLoaf release has no admitted runtime dependency closure".to_string(),
                ));
            }
            let retained_identity = rustc_identity(held.compiler.rustc()).map_err(oven_rustc_error)?;
            if retained_identity != rustc_toolchain {
                return Err(CliError::failure(
                    "selected runtime dependency closure uses a different retained compiler".to_string(),
                ));
            }
            rustc = held.compiler.rustc().to_path_buf();
            Some(held)
        }
        _ => {
            if active_runtime_foundation.is_some() {
                let ambient = resolve_active_rustc().map_err(|error| CliError::failure(error.to_string()))?;
                let ambient_identity = rustc_identity(&ambient).map_err(oven_rustc_error)?;
                if ambient_identity != rustc_toolchain {
                    return Err(CliError::failure(
                        "selected non-release plan requires the ambient compiler matching its build intent".to_string(),
                    ));
                }
                rustc = ambient;
            }
            None
        }
    };
    Ok(OvenPreparedProject {
        generator,
        project_root,
        entrypoint: normalized_file_path,
        provider_plan,
        receipt,
        plan_selection,
        runtime_foundation,
        materialization: plan_preparation.materialization,
        cargo_process_started: plan_preparation.cargo_process_started,
        rustc,
        crate_name: ProjectGenerator::rust_target_name(&project_name),
        rust_edition,
        caller_owned_libraries,
        report,
        prepare_timings,
        #[cfg(feature = "rust_inspect")]
        rust_inspect_manifest_dir: rust_inspect_manifest_dir
            .as_ref()
            .map(|workspace| workspace.manifest_dir().to_path_buf()),
    })
}

/// Prepare the Rust-only direct-rustc base required before Oven can seal a package's declared native artifacts.
///
/// A checked C binding still lowers directly into the final generated Rust root. The compatibility publisher must
/// nevertheless prepare its Rust dependency closure before that root can link a package-owned dynamic library. This
/// dedicated bootstrap stops at that boundary: it publishes no caller-visible binary and does not select a native
/// toolchain. `incan oven interop bake` alone performs those later actions.
pub fn prepare_oven_interop_bootstrap(
    project: &Path,
    target: &str,
) -> CliResult<(oven_store::OvenReceipt, PathBuf, bool)> {
    let project = project.to_str().ok_or_else(|| {
        CliError::failure(format!(
            "Oven interop project path is not valid UTF-8: {}",
            project.display()
        ))
    })?;
    let project_root = resolve_library_project_root(Some(project))?;
    let (kind, entrypoint) = sole_oven_interop_executable_target(discover_oven_bake_project_targets(&project_root)?)?;
    let entrypoint_text = entrypoint.to_str().ok_or_else(|| {
        CliError::failure(format!(
            "Oven interop entrypoint is not valid UTF-8: {}",
            entrypoint.display()
        ))
    })?;
    let prepared = prepare_oven_project(
        entrypoint_text,
        None,
        &CargoPolicy::default(),
        &FeatureSelection::default(),
        None,
        Vec::new(),
        false,
        false,
        "debug",
        OvenProjectPlanMode::InteropBootstrap,
        None,
        &BackendSelectionOptions::default(),
    )?;
    if prepared.receipt.intent.target != target {
        return Err(CliError::failure(format!(
            "Oven interop bootstrap prepared Rust target `{}`, but the declared native target is `{target}`; select a Rust toolchain for that target before baking native interop",
            prepared.receipt.intent.target
        )));
    }
    let receipt_path = interop_bootstrap_receipt_path(
        &project_root,
        &prepared.receipt.intent.target,
        kind,
        &entrypoint,
        "debug",
    )?;
    Ok((prepared.receipt, receipt_path, prepared.cargo_process_started))
}

/// Select the one executable an automatic interop bootstrap may prepare.
///
/// An explicit `--base-receipt` remains available for packages whose author intentionally selects one of several
/// scripts. Automatic selection must fail closed rather than letting filesystem or manifest discovery order decide
/// which generated root becomes native-plan authority.
fn sole_oven_interop_executable_target(
    targets: Vec<(OvenBakeProjectTarget, PathBuf)>,
) -> CliResult<(OvenBakeProjectTarget, PathBuf)> {
    let executable_targets = targets
        .into_iter()
        .filter(|(kind, _)| *kind == OvenBakeProjectTarget::Executable)
        .collect::<Vec<_>>();
    match executable_targets.as_slice() {
        [(kind, entrypoint)] => Ok((*kind, entrypoint.clone())),
        [] => Err(CliError::failure(
            "Oven interop bootstrap currently requires src/main.incn or one declared [project.scripts] executable entrypoint",
        )),
        _ => {
            let entrypoints = executable_targets
                .iter()
                .map(|(_, entrypoint)| entrypoint.display().to_string())
                .collect::<Vec<_>>()
                .join(", ");
            Err(CliError::failure(format!(
                "Oven interop bootstrap requires one executable entrypoint, but this package declares: {entrypoints}; provide an explicit base receipt for the intended entrypoint",
            )))
        }
    }
}

/// Return whether an explicit Loaf publisher is constructing its compiler-owned source closure.
///
/// This marker is intentionally not a normal-command fallback: it changes only generated-source retention before
/// the separately named `legacy_cargo` publisher seals a Loaf. Baked normal build/run/test consumers merely
/// select that immutable plan and execute direct `rustc`.
fn loaf_codegen_mode() -> bool {
    std::env::var_os(OVEN_LOAF_ENV).is_some_and(|value| value == "1")
}

/// Preserve public implementation items whenever the Oven projection emits dependency source.
///
/// A dependency's public protocol methods can construct sibling public adapter models that the root source does not
/// name directly. Emitting the protocol while pruning those adapters produces invalid Rust, so source-backed
/// dependencies form an implementation closure rather than a root-reachability-only projection.
fn preserve_source_dependency_public_items(loaf: bool, emitted_dependency_count: usize) -> bool {
    loaf || emitted_dependency_count > 0
}

/// Include compiler-owned provider Rust imports in the Loaf's inspection workspace.
///
/// Provider modules are deliberately metadata-only for ordinary Oven consumers, so their `rust::` imports are absent
/// from a caller module graph. The named Loaf baker compiles the complete provider source closure instead. It
/// must therefore inspect those exact source imports before codegen, or ownership-sensitive Rust calls (for example
/// `rustix::fs::flock(&impl AsFd, ...)`) degrade to an untyped by-value call. This source walk remains confined to the
/// explicit release-publishing marker and never runs for normal build, run, or test commands.
#[cfg(feature = "rust_inspect")]
fn loaf_rust_inspect_query_paths(
    modules: &[ParsedModule],
    compilation_session: &CompilationSession,
) -> CliResult<Vec<String>> {
    let mut query_paths: BTreeSet<String> = collect_rust_inspect_query_paths(modules).into_iter().collect();
    if !loaf_codegen_mode() {
        return Ok(query_paths.into_iter().collect());
    }

    let stdlib_root = oven_model::toolchain_layout::find_stdlib_root()
        .ok_or_else(|| CliError::failure("cannot locate compiler-owned stdlib sources while preparing an Oven Loaf"))?;
    let mut source_files = Vec::new();
    collect_incan_source_files(&stdlib_root, &mut source_files).map_err(|error| {
        CliError::failure(format!(
            "failed to discover compiler-owned stdlib sources under {}: {error}",
            stdlib_root.display()
        ))
    })?;
    source_files.sort();

    for source_path in source_files {
        let source = fs::read_to_string(&source_path)
            .map_err(|error| CliError::failure(format!("failed to read {}: {error}", source_path.display())))?;
        let program = compilation_session
            .parse_source(&source_path, &source, false)
            .map_err(|errors| {
                let rendered = errors
                    .iter()
                    .map(|error| diagnostics::format_error(source_path.to_string_lossy().as_ref(), &source, error))
                    .collect::<String>();
                CliError::failure(rendered.trim_end())
            })?;
        query_paths.extend(collect_rust_inspect_query_paths_from_programs([&program]));
    }
    Ok(query_paths.into_iter().collect())
}

/// Make a compiler-owned Loaf internally consistent with its retained provider source.
///
/// The named publisher deliberately retains the complete standard-provider envelope, including the modules every
/// optional facet serves. The generated crate must link the same runtime surface; otherwise the sealed source refers
/// to facet items no linked crate supplies while preparing the one explicit publisher artifact. This never changes
/// an ordinary Oven project's facet set.
fn ensure_loaf_stdlib_facets(stdlib_facets: &mut Vec<String>, loaf: bool) {
    if !loaf {
        return;
    }

    stdlib_facets.extend(
        incan_lang::lang::stdlib::facets::ALL
            .into_iter()
            .filter(|facet| *facet != incan_lang::lang::stdlib::facets::CORE)
            .map(str::to_string),
    );
    stdlib_facets.sort();
    stdlib_facets.dedup();
}

/// Return the receipt-compatible direct-Rustc selection for one normal command.
///
/// The compiler-suite scheduler marks nested commands explicitly and supplies a read-only compiler-data root whose
/// store partition remains leased by the parent. Those children consume that shared Loaf directly; copying it
/// into every fixture's small mutable store would consume the same capacity repeatedly and allow policy pruning to
/// remove a just-selected plan before execution. Every other normal command retains the ordinary bounded-store path.
pub fn select_oven_direct_rustc_plan(
    store: &OvenStore,
    receipt: &oven_store::OvenReceipt,
    registry_dependencies: &[DependencySpec],
) -> CliResult<Option<OvenDirectRustcPlanSelection>> {
    select_oven_direct_rustc_plan_with_materialization(store, receipt, registry_dependencies)
        .map(|selection| selection.map(|selection| selection.plan_selection))
}

/// Select a receipt-compatible direct-rustc plan and retain the user-visible local-store outcome.
///
/// This is the one selector used by normal consumers and explicit project preparation. It deliberately keeps
/// receipt matching, Loaf compatibility, atomic store publication, and lease acquisition in the existing paths;
/// the additional outcome only makes that existing decision visible to `incan oven bake`.
pub fn select_oven_direct_rustc_plan_with_materialization(
    store: &OvenStore,
    receipt: &oven_store::OvenReceipt,
    registry_dependencies: &[DependencySpec],
) -> CliResult<Option<OvenDirectRustcPlanPreparation>> {
    let compiler_suite_native =
        std::env::var_os("INCAN_INTERNAL_OVEN_LOAF_EXECUTION").is_some_and(|value| value == "1");
    if compiler_suite_native && receipt_requires_final_interop_plan(receipt) {
        return Err(interop_final_plan_required_error());
    }
    if compiler_suite_native {
        let toolchain_data_root = std::env::var_os("INCAN_INTERNAL_TOOLCHAIN_DATA_ROOT")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .filter(|path| path.is_dir())
            .ok_or_else(|| {
                CliError::failure("compiler-suite native execution requires a readable immutable toolchain-data root")
            })?;
        let native = resolve_compiler_owned_loaf_for_registry_dependencies(receipt, registry_dependencies)
            .map_err(|error| CliError::failure(error.to_string()))?;
        if let Some(native) = native {
            if !native.artifact_root.starts_with(&toolchain_data_root) {
                return Err(CliError::failure(
                    "compiler-suite native selection escaped its immutable toolchain-data root",
                ));
            }
            return Ok(Some(OvenDirectRustcPlanPreparation {
                plan_selection: OvenDirectRustcPlanSelection::ToolchainLoaf(Box::new(native)),
                materialization: OvenToolchainMaterialization::ToolchainLoaf,
                cargo_process_started: false,
            }));
        }
        // A miss here has two different causes and they need different words. When the receipt names registry
        // dependencies, the caller really does have to bake them first. When it names none, nothing was missing to
        // bake and the rejection happened during Loaf compatibility instead; reporting "Needs: none." there sends the
        // reader to dependency resolution while the actual condition sits in the selected Loaf's intent or providers.
        let requirements = format_oven_registry_dependency_requirements(registry_dependencies);
        if requirements == "none" {
            return Err(CliError::failure(format!(
                "{}. Nested build and run {}. No dependency is missing: {}.",
                OVEN_NESTED_DEPENDENCY_MISS_SUMMARY,
                OVEN_NO_IMPLICIT_DEPENDENCY_BUILD,
                oven_rustc::loaf::describe_compiler_owned_loaf_miss(receipt),
            )));
        }
        return Err(CliError::failure(format!(
            "{}. Nested build and run {}. (Needs: {requirements}.)",
            OVEN_NESTED_DEPENDENCY_MISS_SUMMARY, OVEN_NO_IMPLICIT_DEPENDENCY_BUILD,
        )));
    }

    if receipt_requires_final_interop_plan(receipt) {
        return select_published_project_plan(store, receipt, OvenToolchainMaterialization::Reused)?.map_or_else(
            || Err(interop_final_plan_required_error()),
            |selection| Ok(Some(selection)),
        );
    }
    // A receipt-exact project Loaf is narrower than the release-wide standard-library family and must win when it
    // exists. Selecting the broad family first would expose its fixture-only direct externs to a normal project,
    // then bypass the exact base-plus-extension composition that the explicit baker already sealed.
    if let Some(selected) = select_published_project_plan(store, receipt, OvenToolchainMaterialization::Reused)? {
        return Ok(Some(selected));
    }
    if let Some(native) = resolve_compiler_owned_loaf_for_registry_dependencies(receipt, registry_dependencies)
        .map_err(|error| CliError::failure(error.to_string()))?
    {
        return Ok(Some(OvenDirectRustcPlanPreparation {
            plan_selection: OvenDirectRustcPlanSelection::ToolchainLoaf(Box::new(native)),
            materialization: OvenToolchainMaterialization::ToolchainLoaf,
            cargo_process_started: false,
        }));
    }
    Ok(None)
}

/// Publish a receipt-compatible generated-project Loaf at Oven's one explicit project-bake boundary.
///
/// The compatibility baker owns Cargo only for this transaction. It creates a private bounded target, seals the
/// verified direct-rustc artifacts into the shared Oven store, and removes the private target before returning. A
/// project bake retains the locked third-party and provider artifacts that extend one exact Incan release Loaf. Before
/// publishing the project delta, the baker canonicalizes compiler-owned runtime artifacts, overlapping locked registry
/// units, and vocabulary auxiliaries against that exact base cohort. This prevents later consumers from observing
/// distinct Incan release cohorts while preserving the project's own dependency lock.
#[allow(clippy::too_many_arguments)] // Each input is a distinct publisher authority: store, receipt, roots, rustc, base Loaf, vocab support, kind.
pub fn bake_generated_project_compatibility_plan(
    store: &OvenStore,
    receipt: &oven_store::OvenReceipt,
    generated_project: &Path,
    generated_root: &Path,
    rustc: &Path,
    base_loaf: Option<&OvenToolchainLoaf>,
    source_compiler_vocab_support: bool,
    publication_kind: OvenLegacyCargoPublicationKind,
    provider_compilations: &[OvenCompilerMacroDependency],
) -> CliResult<OvenToolchainMaterialization> {
    let compile_environment = direct_rustc_reusable_project_plan_environment(generated_project, generated_root)
        .map_err(|error| CliError::failure(error.to_string()))?;
    let publication = prepare_direct_rustc_plan(&OvenLegacyCargoPrepareRequest {
        compiler: incan_oven_facet::compiler_identity(),
        provider_hooks: incan_oven_facet::provider_hooks(),
        store,
        receipt: receipt.clone(),
        generated_project: generated_project.to_path_buf(),
        cargo: resolved_cargo_executable()
            .map_err(|error| CliError::failure(format!("cannot resolve Cargo for explicit Oven bake: {error}")))?,
        rustc: rustc.to_path_buf(),
        sdk_inventory: None,
        compiler_loaf_root: None,
        domain: format!("incan-release-{INCAN_VERSION}"),
        publication_kind,
        source_evidence_key: "generated-root".to_string(),
        compile_environment,
        // A project Loaf is the complete exact closure for its generated project, including caller-owned `pub::`
        // providers. Retaining only source-inspection roots would copy an upstream crate's rlibs but omit their
        // receipt-bound registry catalog entries, making legitimate re-materialization fail closed later.
        inspection_packages: None,
        direct_dependency_closure: OvenLegacyCargoDirectDependencyClosure::GeneratedSource,
        provider_compilations,
        // The stored direct-rustc plan needs debuggable generated source and verified link inputs, not Cargo's
        // multi-gigabyte dependency DWARF payload. Keep the named debug publisher compact so one project closure stays
        // inside Oven's bounded compatibility domain.
        compact_debug_info: true,
        source_compiler_vocab_support: source_compiler_vocab_support && base_loaf.is_none(),
        // Rust package identity is carried through artifact metadata, not only source or crate names. The base owns the
        // complete Incan release cohort; the project contributes its locked third-party and provider delta.
        base_loaf: base_loaf.map(|base| OvenLegacyCargoBaseLoaf {
            loaf_identity: base.loaf_identity.clone(),
            build_unit_identity: base.loaf_build_unit_identity.clone(),
            artifacts: &base.artifacts,
            artifact_root: &base.artifact_root,
        }),
    })
    .map_err(|error| CliError::failure(error.to_string()))?;
    if !publication.reclaimed_store_entries.is_empty() {
        eprintln!(
            "note: reclaimed {} inactive Oven store entr{} to reserve staging for this bake; entries under a live \
             lease were kept: {}",
            publication.reclaimed_store_entries.len(),
            if publication.reclaimed_store_entries.len() == 1 {
                "y"
            } else {
                "ies"
            },
            publication.reclaimed_store_entries.join(", ")
        );
    }
    Ok(if publication.cargo_version == "not-run-existing-plan" {
        OvenToolchainMaterialization::Reused
    } else {
        OvenToolchainMaterialization::CompatibilityBaked
    })
}

/// Remove the compiler-generated publisher lock after an explicit bake has sealed its digest and direct-Rustc closure
/// into immutable Loafs.
///
/// The lock is publisher input, not a caller-facing Oven artifact. Retaining it below `target/` would make a completed
/// direct-Rustc output look like a mutable Cargo workspace and invite an unsupported normal-command path.
pub fn remove_completed_generated_cargo_lock(generated_project: &Path) -> CliResult<()> {
    let lock_path = generated_project.join("Cargo.lock");
    match fs::remove_file(&lock_path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(CliError::failure(format!(
            "could not remove explicit Oven bake publisher lock {}: {error}",
            lock_path.display()
        ))),
    }
}

/// Select the immutable full-stdlib base that supplies the release-owned Incan dependency cohort.
///
/// The project retains its own locked third-party closure, while compiler-owned runtime artifacts, overlapping locked
/// registry units, and vocabulary auxiliaries inherit this exact release base. Normal consumers still require a
/// compatible prebuilt Loaf or stored project plan and never invoke this helper as a fallback.
pub fn project_extension_base_loaf(receipt: &oven_store::OvenReceipt) -> CliResult<Option<OvenToolchainLoaf>> {
    resolve_compiler_owned_loaf_for_registry_dependencies(receipt, &[])
        .map_err(|error| CliError::failure(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    use crate::build::OvenBakeProjectTarget;
    use oven_interop::OVEN_INTEROP_EXECUTION_RECEIPT_INPUT;
    use oven_store::store::OvenStore;
    use oven_store::{OvenGeneratedProjectRequest, receipt_generated_project};

    #[test]
    fn loaf_enables_the_complete_stdlib_runtime_envelope() {
        let mut seeded = vec!["incan_std_data".to_string()];
        ensure_loaf_stdlib_facets(&mut seeded, true);
        assert_eq!(
            seeded,
            [
                "incan_std_async",
                "incan_std_data",
                "incan_std_testing",
                "incan_std_web"
            ]
        );

        let mut ordinary = vec!["incan_std_data".to_string()];
        ensure_loaf_stdlib_facets(&mut ordinary, false);
        assert_eq!(ordinary, ["incan_std_data"]);
    }

    #[test]
    fn normal_oven_source_dependencies_keep_their_public_implementation_closure() {
        assert!(preserve_source_dependency_public_items(false, 1));
        assert!(preserve_source_dependency_public_items(true, 0));
        assert!(!preserve_source_dependency_public_items(false, 0));
    }

    #[test]
    fn interop_receipt_miss_never_materializes_a_generic_toolchain_loaf() -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let source = project.path().join("lib.rs");
        fs::write(&source, "pub fn fixture() {}\n")?;
        let receipt = receipt_generated_project(
            &OvenGeneratedProjectRequest::new(
                project.path(),
                "interop-miss",
                "0.1.0",
                "aarch64-apple-darwin",
                "rustc fixture",
                "debug",
                Vec::new(),
            )
            .with_generated_source("lib.rs", &source)
            .with_build_unit_input(OVEN_INTEROP_EXECUTION_RECEIPT_INPUT, "sha256:selected-interop"),
        )?;
        let store = OvenStore::new(
            project.path().join("oven-store"),
            oven_store::store::OvenStoreLimits::new(1024 * 1024, 1024 * 1024, 1024 * 1024),
        );

        let selection = select_oven_direct_rustc_plan(&store, &receipt, &[]);
        let Err(error) = selection else {
            return Err("an interop receipt miss must not materialize a generic Loaf".into());
        };
        assert!(error.to_string().contains("incan oven interop bake"));
        let entries = project.path().join("oven-store/entries");
        match fs::read_dir(&entries) {
            Ok(mut entries) => assert!(entries.next().is_none()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        Ok(())
    }

    #[test]
    fn oven_interop_bootstrap_selects_only_one_declared_executable() -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let main = project.path().join("src/main.incn");
        let extra = project.path().join("src/extra.incn");

        let selected = sole_oven_interop_executable_target(vec![
            (OvenBakeProjectTarget::Library, project.path().join("src/lib.incn")),
            (OvenBakeProjectTarget::Executable, main.clone()),
        ])?;
        assert_eq!(selected, (OvenBakeProjectTarget::Executable, main));

        let error = match sole_oven_interop_executable_target(vec![
            (OvenBakeProjectTarget::Executable, project.path().join("src/main.incn")),
            (OvenBakeProjectTarget::Executable, extra),
        ]) {
            Err(error) => error,
            Ok(selected) => {
                return Err(format!(
                    "automatic interop bootstrap must not select one of several scripts, but selected {selected:?}"
                )
                .into());
            }
        };
        assert!(error.to_string().contains("provide an explicit base receipt"));
        Ok(())
    }
}
