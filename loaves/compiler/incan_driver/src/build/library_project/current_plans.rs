//! Rebuild current profile execution plans from current admitted dependencies, shared by fresh checking and metadata
//! replay.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use crate::backend::ProjectGenerator;
use crate::build::caller_owned::oven_caller_owned_libraries;
use crate::build::plan_authority::{
    compiler_selected_path_authority, declared_rust_libraries_missing_from_selected_plan_with_current_project_paths,
    explicit_bake_profiles,
};
use crate::build::plan_selection::{
    format_oven_registry_dependency_requirements, packaged_provider_selection_links_required_stdlib,
    registry_leaf_authority_for_plan_selection,
};
use crate::build::provider_compilation::checked_provider_compilation_requirements;
use crate::build::{
    OvenDirectRustcPlanPreparation, OvenPreparedLibrary, OvenPreparedLibraryProfile, OvenProjectDependencySurface,
    OvenProjectPlanMode, OvenToolchainMaterialization, packaged_provider_candidates, record_timing,
};
use crate::error::{CliError, CliResult, oven_plan_error, oven_rustc_error};
use oven_cargo_compat::provider_compilation_requirements_digest;
use oven_rustc::loaf::{OVEN_DEPENDENCY_MISS_SUMMARY, OVEN_LOAF_MISS_GUIDANCE, OVEN_NO_IMPLICIT_DEPENDENCY_BUILD};
use oven_rustc::plan::composition::compose_selected_packaged_provider_plan;
use oven_rustc::plan::selection::select_packaged_provider_plans;
use oven_rustc::rustc::materialize_declared_rust_libraries_with_selected_path_authority;
use oven_store::{
    OvenGeneratedProjectRequest, generated_project_source_evidence, receipt_generated_project_with_source_evidence,
    write_receipt,
};

/// Current authority and generated output entering profile planning; no persisted native plan or lease is accepted.
pub(super) struct CurrentLibraryPlanInputs<'a> {
    pub project_root: &'a Path,
    pub project_name: &'a str,
    pub project_version: &'a str,
    pub generator: &'a ProjectGenerator,
    pub provider_plan: &'a Arc<incan_provider::ProviderPlan>,
    pub checked_provider_profiles: &'a [crate::build::CheckedPackagedProviderProfile],
    pub build_inputs: &'a BTreeMap<String, String>,
    pub inline_dependencies: &'a [oven_model::manifest::DependencySpec],
    pub rustc: PathBuf,
    pub target: String,
    pub toolchain: String,
    pub store: &'a oven_store::store::OvenStore,
    pub native_sdk_context: Option<Arc<crate::build::NativeSdkCommandContext>>,
    pub ordinary_runtime: Option<crate::build_unit::OrdinaryLibraryRuntimeInputs>,
    pub oven_plan_mode: OvenProjectPlanMode,
    pub rust_edition: Option<String>,
}

/// Prepare each requested profile using this command's original current owner selections.
pub(super) fn prepare_current_library_profiles(
    inputs: CurrentLibraryPlanInputs<'_>,
    timings_ms: &mut BTreeMap<String, u64>,
) -> CliResult<OvenPreparedLibrary> {
    let CurrentLibraryPlanInputs {
        project_root,
        project_name,
        project_version,
        generator,
        provider_plan,
        checked_provider_profiles,
        build_inputs,
        inline_dependencies,
        rustc,
        target,
        toolchain,
        store,
        native_sdk_context,
        ordinary_runtime,
        oven_plan_mode,
        rust_edition,
    } = inputs;
    let link_closure = oven_rustc::rustc::pinned_link_closure_identity(&rustc, &target).map_err(oven_rustc_error)?;
    let mut profiles = BTreeMap::new();
    let oven_receipt_source_evidence_start = Instant::now();
    let mut source_evidence_request = OvenGeneratedProjectRequest::new(
        project_root,
        project_name,
        project_version,
        target.clone(),
        toolchain.clone(),
        "debug",
        Vec::new(),
    )
    .with_generated_source("generated-root", generator.crate_root_path())
    .with_generated_source_tree("generated-source-tree", generator.output_dir().join("src"));
    for (name, value) in build_inputs {
        source_evidence_request = source_evidence_request.with_build_unit_input(name.clone(), value.clone());
    }
    let generated_source_evidence = generated_project_source_evidence(&source_evidence_request)
        .map_err(|error| CliError::failure(error.to_string()))?;
    record_timing(
        timings_ms,
        "library_oven_receipt_source_evidence",
        oven_receipt_source_evidence_start,
    );
    if ordinary_runtime.is_some() && native_sdk_context.is_some() {
        return Err(CliError::failure(
            "library profile planning has competing ordinary and SDK authority",
        ));
    }
    let requested_profiles = match &ordinary_runtime {
        Some(runtime) => runtime
            .native()
            .metadata()
            .observations()
            .keys()
            .map(String::as_str)
            .collect(),
        None => explicit_bake_profiles(),
    };
    for profile in requested_profiles {
        let mut profile_build_inputs = build_inputs.clone();
        if let Some(runtime) = &ordinary_runtime {
            let intent = oven_store::OvenBuildIntent {
                target: target.clone(),
                toolchain: toolchain.clone(),
                profile: profile.to_string(),
                features: Vec::new(),
            };
            for (key, value) in runtime.for_profile(&intent, inline_dependencies)? {
                if profile_build_inputs.insert(key, value).is_some() {
                    return Err(CliError::failure(
                        "ordinary physical input conflicts with a shared library input",
                    ));
                }
            }
        }
        let mut receipt_request = OvenGeneratedProjectRequest::new(
            project_root,
            project_name,
            project_version,
            target.clone(),
            toolchain.clone(),
            profile,
            Vec::new(),
        )
        .with_generated_source("generated-root", generator.crate_root_path())
        .with_generated_source_tree("generated-source-tree", generator.output_dir().join("src"));
        for (name, value) in &profile_build_inputs {
            receipt_request = receipt_request.with_build_unit_input(name.clone(), value.clone());
        }
        let provider_candidates = packaged_provider_candidates(checked_provider_profiles, profile);
        let selected_provider_inputs =
            select_packaged_provider_plans(store, &provider_candidates).map_err(oven_plan_error)?;
        let provider_compilations = checked_provider_compilation_requirements(
            &selected_provider_inputs,
            checked_provider_profiles,
            profile,
            build_inputs,
        )?;
        if !provider_compilations.is_empty() {
            receipt_request = receipt_request.with_build_unit_input(
                "provider-compilation-requirements",
                provider_compilation_requirements_digest(&provider_compilations)
                    .map_err(|error| CliError::failure(error.to_string()))?,
            );
        }
        if let Some(identity) = &link_closure {
            receipt_request = receipt_request.with_build_unit_input("link-closure", identity);
        }
        let mut receipt = receipt_generated_project_with_source_evidence(&receipt_request, &generated_source_evidence)
            .map_err(|error| CliError::failure(error.to_string()))?;
        let receipt_path = if profile == "release" {
            oven_store::default_receipt_path(project_root)
        } else {
            oven_store::default_receipt_path(project_root).with_file_name("library-debug-receipt.json")
        };
        if ordinary_runtime.is_none() {
            write_receipt(&receipt, receipt_path.clone()).map_err(|error| CliError::failure(error.to_string()))?;
        }
        let required_registry_dependencies = format_oven_registry_dependency_requirements(inline_dependencies);
        let oven_select_direct_rustc_plan_start = Instant::now();
        // An imported package Loaf is sufficient only for consume-only commands. An explicit library bake must
        // instead publish the library's own direct registry roots with its complete generated source closure.
        let packaged_provider_selection =
            if ordinary_runtime.is_none() && oven_plan_mode == OvenProjectPlanMode::ConsumeOnly {
                packaged_provider_selection_links_required_stdlib(
                    compose_selected_packaged_provider_plan(selected_provider_inputs, &provider_candidates, &receipt)
                        .map_err(oven_plan_error)?,
                    provider_plan,
                )?
            } else {
                None
            };
        let plan_preparation = if let Some(runtime) = &ordinary_runtime {
            let (bound, plan_selection) = runtime.native().select_plan(store, &receipt, inline_dependencies, project_root)?;
            receipt = bound;
            write_receipt(&receipt, receipt_path.clone()).map_err(|error| CliError::failure(error.to_string()))?;
            Some(OvenDirectRustcPlanPreparation { plan_selection, materialization: OvenToolchainMaterialization::Reused,
                cargo_process_started: false })
        } else if let Some(selection) = packaged_provider_selection {
            Some(OvenDirectRustcPlanPreparation {
                plan_selection: selection,
                materialization: OvenToolchainMaterialization::Reused,
                cargo_process_started: false,
            })
        } else {
            crate::build::plan_selection::select_or_bake_generated_project_plan_with_native_sdk(
                oven_plan_mode,
                store,
                &receipt,
                OvenProjectDependencySurface {
                    selection: inline_dependencies,
                    provider_compilations: &provider_compilations,
                },
                generator.output_dir(),
                &generator.crate_root_path(),
                &rustc,
                native_sdk_context.as_deref(),
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
            timings_ms,
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
        if ordinary_runtime.is_none() {
            crate::build::plan_authority::validate_selected_plan_registry_dependencies_with_native_sdk(
                inline_dependencies,
                &artifact_plan,
                registry_authority.as_ref(),
                profile,
                native_sdk_context.as_deref(),
            )?;
        }
        // The ordinary constructor selected every exact authored root from original producer declarations.
        // Its authenticated plan already supplies them; the legacy path rematerializes missing caller sources.
        let inline_libraries = if ordinary_runtime.is_some() {
            Vec::new()
        } else {
            declared_rust_libraries_missing_from_selected_plan_with_current_project_paths(
                inline_dependencies,
                &artifact_plan,
                plan_selection.seals_current_project_path_dependencies(),
            )
        };
        let selected_path_authority = compiler_selected_path_authority(full_artifact_plan, Some(provider_plan));
        record_timing(
            timings_ms,
            "library_oven_validate_direct_rustc_plan",
            oven_validate_direct_rustc_plan_start,
        );
        let oven_prepare_caller_owned_libraries_start = Instant::now();
        let mut caller_owned_libraries = oven_caller_owned_libraries(provider_plan, profile)?;
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
            timings_ms,
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
                native_sdk_context: native_sdk_context.clone(),
                ordinary_native: ordinary_runtime.as_ref().map(|runtime| Arc::clone(runtime.native())),
                receipt,
                plan_selection,
                materialization: plan_preparation.materialization,
                provider_plan: provider_plan.clone(),
                caller_owned_libraries,
            },
        );
    }
    Ok(OvenPreparedLibrary {
        rustc,
        crate_name: ProjectGenerator::rust_target_name(project_name),
        rust_edition: rust_edition.unwrap_or_else(|| "2024".to_string()),
        profiles,
    })
}
