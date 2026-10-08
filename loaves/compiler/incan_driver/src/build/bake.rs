//! Baking a project's targets: the executable and library bakes, their classification, and the explicit `incan oven
//! bake --project` entry.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::{fs, io};

use crate::backend::selection::BackendExecutionReceipt;
use crate::build::caller_facet::{
    CallerFacetRequest, scan_rust_caller_paths, select_checked_caller_exports_with_body_ir,
};
use crate::build::caller_owned::has_caller_owned_project_libraries;
use crate::build::library_exports::resolve_library_project_root;
use crate::build::library_outputs::{
    library_publication_receipts, packaged_library_loaf_store_root, write_library_manifest_artifacts,
};
use crate::build::library_project::{prepare_library_project, prepare_library_project_with_caller_facet};
use crate::build::output_materialization::{
    completed_output_default_backend_receipt, select_default_project_output,
    warn_for_completed_output_lock_fingerprint_drift,
};
use crate::build::output_paths::{
    library_project_output_sidecars, normalized_project_entrypoint, oven_binary_path, packaged_library_metadata_files,
    project_locked_registry_packages, project_output_bake_files, project_root_for_completed_output,
    validated_project_output_relative_path,
};
use crate::build::output_selection::project_output_report_snapshot;
use crate::build::oven_project::{prepare_oven_project, remove_completed_generated_cargo_lock};
use crate::build::package_loafs::{export_selected_package_loaf, write_packaged_library_loaf_manifest};
use crate::build::plan_authority::{
    collect_caller_owned_provider_registry_leaf_authority, explicit_bake_profiles,
    rematerialize_caller_owned_libraries_with_authority_context, replace_caller_owned_package_libraries,
    replace_selected_package_library_externs,
};
use crate::build::plan_selection::{
    caller_owned_provider_registry_conflict, canonical_project_inspection_dependencies, oven_native_closure_refusal,
    prepare_oven_test_dependency_envelope, provider_registry_conflict_reason,
    registry_leaf_authority_for_plan_selection,
};
use crate::build::publication::{
    ProjectInspectionDependencyAuthority, project_output_payload_for_bake, publish_project_inspection_authority,
    publish_project_output_loaf,
};
use crate::build::reuse::try_reuse_baked_project;
use crate::build::source_authority::{
    baked_project_lock_dependencies_fingerprint, canonical_baked_project_lock_path, project_bake_receipt_path,
};
use crate::build::{
    BuildCommandOptions, CompletedOutputPolicy, LibraryInspectionConstituent,
    OVEN_PACKAGED_LIBRARY_LOAF_SCHEMA_VERSION, OvenBakeProjectTarget, OvenPackagedLibraryLoafManifest,
    OvenPackagedLibraryLoafProfile, OvenPreparedLibrary, OvenPreparedProject, OvenProjectBakeAuthorityContext,
    OvenProjectBakeOutputReport, OvenProjectBakeProfileReport, OvenProjectBakeReport, OvenProjectOutputBakeRequest,
    OvenProjectPlanMode, OvenStoredProjectOutput, PendingOvenProjectOutput, PreparedLibraryProject,
    library_publication, oven_bake_executable_output_dir, oven_bake_project_target_identity,
};
use crate::build_report::artifact_report;
use crate::cargo_policy::{CargoPolicy, enforce_project_toolchain_constraint};
use crate::error::{CliError, CliResult, oven_rustc_error};
use crate::lock::PublishedOvenProjectLock;
use crate::lock::resolution::publish_oven_project_lock;
use crate::oven_store::open_default_oven_store;
use crate::project::discover_effective_project_manifest;
#[cfg(feature = "rust_inspect")]
use crate::rust_inspect_workspace::mark_oven_direct_rust_inspection;
use incan_frontend::library_manifest::published_layout::packaged_library_loaf_manifest_path;
use incan_lang::version::INCAN_VERSION;
use incan_provider::FeatureSelection;
use oven_cargo_compat::direct_rustc_compile_environment;
use oven_model::manifest::ProjectManifest;
use oven_rustc::plan::OvenDirectRustcPlanSelection;
use oven_rustc::rustc::{
    OvenCallerOwnedRustcLibrary, OvenRustcError, OvenTrustedDirectRustcTargetRequest,
    attach_caller_owned_rustc_libraries, bake_trusted_direct_rustc_library, bake_trusted_direct_rustc_run,
    bake_trusted_direct_rustc_run_with_artifact_role,
};
use oven_store::store::OvenArtifactKind;
use oven_store::{
    OvenGeneratedProjectRequest, generated_project_source_evidence, receipt_generated_project_with_source_evidence,
    write_receipt,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Compile a receipt-authorized generated executable through the selected direct-rustc Oven plan.
pub fn bake_oven_project(
    prepared: &OvenPreparedProject,
    profile: &str,
    authority_context: Option<&mut OvenProjectBakeAuthorityContext>,
) -> CliResult<oven_rustc::rustc::OvenDirectRustcBake> {
    let mut caller_owned_libraries = prepared.caller_owned_libraries.clone();
    let mut re_materialized_package_library_names = BTreeSet::new();
    let mut registry_authority = registry_leaf_authority_for_plan_selection(&prepared.plan_selection)?;
    let mut extra_dependency_search_paths = Vec::new();
    if has_caller_owned_project_libraries(&prepared.provider_plan) {
        let closure = collect_caller_owned_provider_registry_leaf_authority(
            &open_default_oven_store()?,
            &prepared.provider_plan,
            profile,
            !prepared.plan_selection.uses_packaged_provider_closure(),
        )?;
        // The conflict decision must cover every selection path -- including an imported packaged-provider closure,
        // whose composed link carries the SDK base's and the provider's own copies of any shared package exactly
        // like a re-materialized one does.
        if prepared.plan_selection.uses_packaged_provider_closure() {
            if let Some((package, pinned_by, divergence)) = caller_owned_provider_registry_conflict(
                registry_authority.as_ref(),
                &closure,
                prepared.plan_selection.artifact_plan(),
            )? {
                let reason = provider_registry_conflict_reason(&package, pinned_by.as_deref(), divergence.as_deref());
                return Err(oven_native_closure_refusal(
                    &prepared.crate_name,
                    &format!(
                        "the caller-owned provider is source-free and cannot be recompiled into a coherent closure; {reason}"
                    ),
                ));
            }
        } else {
            extra_dependency_search_paths = closure.dependency_search_paths.clone();
            registry_authority = closure.merged_authority(registry_authority);
            let re_materialized = rematerialize_caller_owned_libraries_with_authority_context(
                &prepared.provider_plan,
                profile,
                prepared.plan_selection.artifacts(),
                prepared.plan_selection.output_guard_root(),
                prepared.plan_selection.artifact_plan(),
                &prepared.rustc,
                prepared.generator.output_dir(),
                registry_authority.as_ref(),
                &extra_dependency_search_paths,
                &closure.compiler_runtime_libraries,
                &closure.compiler_runtime_registry_authorities,
                authority_context,
            )?;
            re_materialized_package_library_names.extend(
                re_materialized
                    .iter()
                    .filter(|library| library.expose_extern)
                    .map(|library| library.crate_name.clone()),
            );
            replace_caller_owned_package_libraries(&mut caller_owned_libraries, re_materialized)?;
        }
    }
    let mut artifact_plan = prepared
        .plan_selection
        .source_artifact_plan("generated-root")
        .map_err(oven_rustc_error)?;
    if !re_materialized_package_library_names.is_empty() {
        replace_selected_package_library_externs(&mut artifact_plan, &re_materialized_package_library_names);
    }
    artifact_plan.compile_environment =
        direct_rustc_compile_environment(prepared.generator.output_dir(), &prepared.generator.crate_root_path())
            .map_err(|error| CliError::failure(error.to_string()))?;
    if let Some(held) = prepared.runtime_foundation.as_ref() {
        let closure = held.closure.as_ref().ok_or_else(|| {
            CliError::failure("selected runtime foundation lost its admitted dependency closure".to_string())
        })?;
        closure
            .compose_artifact_plan(&mut artifact_plan)
            .map_err(oven_rustc_error)?;
    }
    attach_caller_owned_rustc_libraries(&mut artifact_plan, &caller_owned_libraries).map_err(oven_rustc_error)?;
    // Loading a re-materialized caller-owned library's own metadata (for example a query-engine provider linked
    // above) can require Rustc to locate that library's own further dependencies purely through
    // `-L dependency=...` search, the same way `rematerialize_caller_owned_provider_graph` already extends that
    // library's own compile with this same closure. The final consumer binary link needs it too.
    for directory in &extra_dependency_search_paths {
        artifact_plan.retain_caller_dependency_search_path(directory.clone());
    }
    let direct = bake_trusted_direct_rustc_run(&OvenTrustedDirectRustcTargetRequest {
        receipt: &prepared.receipt,
        artifacts: prepared.plan_selection.artifacts(),
        artifact_root: prepared.plan_selection.output_guard_root(),
        artifact_plan: Some(&artifact_plan),
        rustc: &prepared.rustc,
        source: &prepared.generator.crate_root_path(),
        output: &oven_binary_path(prepared, profile),
        crate_name: &prepared.crate_name,
        edition: &prepared.rust_edition,
        source_evidence_key: "generated-root",
        features: &prepared.receipt.intent.features,
        prefer_dynamic: false,
    });
    classify_direct_rustc_bake(&prepared.crate_name, direct)
}

/// Return the caller-owned direct-rustc library artifact path.
///
/// It intentionally lives beside the generated inspection projection rather than in a Cargo target directory. The
/// `.incnlib` and generated `Cargo.toml` remain useful publication/inspection artifacts, but neither authorizes this
/// executable path.
fn oven_library_path(prepared: &PreparedLibraryProject, oven: &OvenPreparedLibrary, profile: &str) -> PathBuf {
    prepared
        .out_dir
        .join("oven")
        .join(profile)
        .join(format!("lib{}.rlib", oven.crate_name))
}

/// Compile a receipt-authorized generated library through the selected direct-rustc Oven plan.
pub fn bake_oven_library(
    prepared: &PreparedLibraryProject,
    oven: &OvenPreparedLibrary,
    profile: &str,
    authority_context: Option<&mut OvenProjectBakeAuthorityContext>,
) -> CliResult<oven_rustc::rustc::OvenDirectRustcBake> {
    bake_oven_library_with_dependencies(prepared, oven, profile, authority_context, None).map(|(bake, _)| bake)
}

/// Retain the actual coherent dependency plan used for a library's native compilation.
///
/// A Rust caller must inherit rematerialized provider externs and search paths, rather than the pre-rematerialization
/// selection. Otherwise its library metadata names dependency artifacts absent from the caller's closure.
fn bake_oven_library_with_dependencies(
    prepared: &PreparedLibraryProject,
    oven: &OvenPreparedLibrary,
    profile: &str,
    authority_context: Option<&mut OvenProjectBakeAuthorityContext>,
    body_ir_host_plan: Option<&oven_rustc::rustc::OvenRustcArtifactPlan>,
) -> CliResult<(
    oven_rustc::rustc::OvenDirectRustcBake,
    oven_rustc::rustc::OvenRustcArtifactPlan,
)> {
    let selected = oven.profiles.get(profile).ok_or_else(|| {
        CliError::failure(format!(
            "normal Oven library build has no prepared `{profile}` direct-rustc selection"
        ))
    })?;
    let mut caller_owned_libraries = selected.caller_owned_libraries.clone();
    let mut re_materialized_package_library_names = BTreeSet::new();
    let mut registry_authority = registry_leaf_authority_for_plan_selection(&selected.plan_selection)?;
    let mut extra_dependency_search_paths = Vec::new();
    if has_caller_owned_project_libraries(&selected.provider_plan) {
        let closure = collect_caller_owned_provider_registry_leaf_authority(
            &open_default_oven_store()?,
            &selected.provider_plan,
            profile,
            !selected.plan_selection.uses_packaged_provider_closure(),
        )?;
        // Refuse only where the conflict cannot be resolved. The re-materialization below rebuilds each provider's
        // Rust dependency libraries against the *merged* authority through
        // `materialize_declared_rust_libraries_with_selected_path_authority`, which is what unifying a diverging
        // package means: one compiled artifact, every dependent relinked against it. A packaged provider closure
        // skips that step and consumes the provider's sealed artifacts as they are, so there the divergence really
        // does survive into the link and failing closed is the only safe answer.
        //
        // The rejection previously ran on every selection path, so the resolvable case never reached the machinery
        // that resolves it.
        if selected.plan_selection.uses_packaged_provider_closure() {
            if let Some((package, pinned_by, divergence)) = caller_owned_provider_registry_conflict(
                registry_authority.as_ref(),
                &closure,
                selected.plan_selection.artifact_plan(),
            )? {
                let reason = provider_registry_conflict_reason(&package, pinned_by.as_deref(), divergence.as_deref());
                return Err(oven_native_closure_refusal(
                    &oven.crate_name,
                    &format!(
                        "the caller-owned provider is source-free and cannot be recompiled into a coherent closure; {reason}"
                    ),
                ));
            }
        } else {
            extra_dependency_search_paths = closure.dependency_search_paths.clone();
            registry_authority = closure.merged_authority(registry_authority);
            let re_materialized = rematerialize_caller_owned_libraries_with_authority_context(
                &selected.provider_plan,
                profile,
                selected.plan_selection.artifacts(),
                selected.plan_selection.output_guard_root(),
                selected.plan_selection.artifact_plan(),
                &oven.rustc,
                &prepared.out_dir,
                registry_authority.as_ref(),
                &extra_dependency_search_paths,
                &closure.compiler_runtime_libraries,
                &closure.compiler_runtime_registry_authorities,
                authority_context,
            )?;
            re_materialized_package_library_names.extend(
                re_materialized
                    .iter()
                    .filter(|library| library.expose_extern)
                    .map(|library| library.crate_name.clone()),
            );
            replace_caller_owned_package_libraries(&mut caller_owned_libraries, re_materialized)?;
        }
    }
    let mut artifact_plan = selected
        .plan_selection
        .source_artifact_plan("generated-root")
        .map_err(oven_rustc_error)?;
    if !re_materialized_package_library_names.is_empty() {
        replace_selected_package_library_externs(&mut artifact_plan, &re_materialized_package_library_names);
    }
    artifact_plan.compile_environment =
        direct_rustc_compile_environment(prepared.generator.output_dir(), &prepared.generator.crate_root_path())
            .map_err(|error| CliError::failure(error.to_string()))?;
    attach_caller_owned_rustc_libraries(&mut artifact_plan, &caller_owned_libraries).map_err(oven_rustc_error)?;
    // See the matching comment in `bake_oven_project`: a re-materialized caller-owned library's own metadata can
    // require this same dependency search closure to load, not only the library's own re-materialization compile.
    for directory in &extra_dependency_search_paths {
        artifact_plan.retain_caller_dependency_search_path(directory.clone());
    }
    let coherent_receipt = if let Some(host) = body_ir_host_plan {
        let digest = apply_body_ir_host_cohort(&mut artifact_plan, host)?;
        oven_store::receipt_with_build_unit_input(&selected.receipt, "body-ir-host-cohort", digest)
            .map_err(|error| CliError::failure(error.to_string()))?
    } else {
        selected.receipt.clone()
    };
    let direct = bake_trusted_direct_rustc_library(&OvenTrustedDirectRustcTargetRequest {
        receipt: &coherent_receipt,
        artifacts: selected.plan_selection.artifacts(),
        artifact_root: selected.plan_selection.output_guard_root(),
        artifact_plan: Some(&artifact_plan),
        rustc: &oven.rustc,
        source: &prepared.generator.crate_root_path(),
        output: &oven_library_path(prepared, oven, profile),
        crate_name: &oven.crate_name,
        edition: &oven.rust_edition,
        source_evidence_key: "generated-root",
        features: &selected.receipt.intent.features,
        prefer_dynamic: false,
    });

    classify_direct_rustc_bake(&oven.crate_name, direct).map(|bake| (bake, artifact_plan))
}

/// Recompile a source-owned Body IR caller against the host's exact compiled semantics-core instance.
///
/// The caller entry supplies this closure only after canonical path and normalized dependency identity agree.
/// Transitive feature unification can still change Rust type identity, so shared source/version alone is insufficient.
/// Exact artifact bytes enter the library recipe and receipt, and its metadata inherits the host's leased searches.
fn apply_body_ir_host_cohort(
    plan: &mut oven_rustc::rustc::OvenRustcArtifactPlan,
    host: &oven_rustc::rustc::OvenRustcArtifactPlan,
) -> CliResult<String> {
    let mut candidates = host.externs.iter().filter(|(name, _)| name == "incan_semantics_core");
    let (_, output) = candidates
        .next()
        .ok_or_else(|| CliError::failure("Body IR host cohort has no semantics-core artifact"))?;
    if candidates.next().is_some() {
        return Err(CliError::failure(
            "Body IR host cohort has ambiguous semantics-core artifacts",
        ));
    }
    let digest = oven_store::digest_bytes(&fs::read(output).map_err(|error| CliError::failure(error.to_string()))?);
    replace_rust_library_bindings(
        plan,
        &[OvenCallerOwnedRustcLibrary {
            crate_name: "incan_semantics_core".into(),
            output: output.clone(),
            digest: digest.clone(),
            expose_extern: true,
        }],
    )?;
    for path in &host.dependency_search_paths {
        plan.retain_caller_dependency_search_path(path.clone());
    }
    Ok(digest)
}

/// Replace already-authorized direct Rust roots and their digest bindings together.
///
/// Both host-cohort selection and final unit composition own these declared aliases. Reattaching an identical
/// root must be idempotent; replacing only its extern leaves stale reuse evidence and refuses the second step.
fn replace_rust_library_bindings(
    plan: &mut oven_rustc::rustc::OvenRustcArtifactPlan,
    libraries: &[OvenCallerOwnedRustcLibrary],
) -> CliResult<()> {
    for library in libraries {
        if library.expose_extern {
            plan.externs.retain(|(name, _)| name != &library.crate_name);
            plan.caller_owned_library_digests.remove(&library.crate_name);
        }
    }
    attach_caller_owned_rustc_libraries(plan, libraries).map_err(oven_rustc_error)
}

/// Turn a direct-rustc composition failure into the named Oven-boundary refusal; pass every other outcome through.
///
/// A crate-loading failure is a composition fault, not a fault in the generated Rust: the sources already
/// typechecked, so rustc rejecting a dependency means the assembled closure is not mutually loadable. Both the
/// executable and the library route name that, rather than surfacing raw `E0463`s or a `StableCrateId` collision
/// about crates the user never named.
fn classify_direct_rustc_bake(
    crate_name: &str,
    direct: Result<oven_rustc::rustc::OvenDirectRustcBake, OvenRustcError>,
) -> CliResult<oven_rustc::rustc::OvenDirectRustcBake> {
    match direct {
        Ok(bake) => Ok(bake),
        Err(error) if direct_rustc_composition_failure(&error) => Err(oven_native_closure_refusal(
            crate_name,
            &format!(
                "its assembled dependency closure is not loadable as independently compiled parts ({})",
                oven_rustc_error(error)
            ),
        )),
        Err(error) => Err(oven_rustc_error(error)),
    }
}

/// Recognize a rustc failure caused by an unloadable dependency closure rather than by the compiled source.
///
/// Only crate-loading failures qualify. `E0463` is a crate that could not be found at all, `E0460`/`E0461`/`E0464`
/// are candidates that were found but rejected for identity, target or ambiguity reasons, and a `StableCrateId`
/// collision -- which rustc reports without an error code -- is two compiled instances of one crate meeting in one
/// link, the diamond over two source-free providers. A type error in generated Rust is never one of these, so this
/// cannot swallow a genuine compilation failure and silently retry it.
fn direct_rustc_composition_failure(error: &OvenRustcError) -> bool {
    let OvenRustcError::CompilationFailed { report } = error else {
        return false;
    };
    const CRATE_LOADING_CODES: [&str; 4] = ["E0460", "E0461", "E0463", "E0464"];
    const STABLE_CRATE_ID_COLLISION: &str = "colliding StableCrateId";
    if report.diagnostics.iter().any(|diagnostic| {
        diagnostic
            .code
            .as_deref()
            .is_some_and(|code| CRATE_LOADING_CODES.contains(&code))
            || diagnostic.message.contains(STABLE_CRATE_ID_COLLISION)
    }) {
        return true;
    }
    // Newer rustc JSON records carry a `$message_type` tag the structured decoder does not recognize, so the
    // whole transcript can arrive as unstructured text with `diagnostics` empty. The codes are still verbatim in
    // it, and missing this case is what makes the failure surface as raw rustc noise instead of a rebuild.
    CRATE_LOADING_CODES
        .iter()
        .any(|code| report.unstructured_output.contains(code))
        || report.unstructured_output.contains(STABLE_CRATE_ID_COLLISION)
}

/// Select the default sealed release output without entering frontend or report reconstruction.
pub fn select_default_executable_project_output(
    file_path: &str,
    output_dir: Option<&String>,
    options: &BuildCommandOptions,
) -> CliResult<Option<(PathBuf, OvenStoredProjectOutput, BackendExecutionReceipt)>> {
    if output_dir.is_some() {
        return Ok(None);
    }
    let completed_output_policy = CompletedOutputPolicy {
        cargo_policy: &options.cargo_policy,
        package_features: &options.package_features,
        sdk_profile: options.sdk_profile.as_deref(),
        cargo_features: &options.cargo_features,
        cargo_no_default_features: options.cargo_no_default_features,
        cargo_all_features: options.cargo_all_features,
    };
    let Some(selected) = select_default_project_output(
        file_path,
        &completed_output_policy,
        OvenBakeProjectTarget::Executable,
        "release",
    )?
    else {
        return Ok(None);
    };
    let project_root = project_root_for_completed_output(&normalized_project_entrypoint(file_path)?)?
        .ok_or_else(|| CliError::failure("selected Oven project-output Loaf has no manifest-backed project root"))?;
    let Some(backend_receipt) = completed_output_default_backend_receipt(&selected) else {
        return Ok(None);
    };
    warn_for_completed_output_lock_fingerprint_drift(&project_root, [&selected])?;
    Ok(Some((project_root, selected, backend_receipt)))
}

/// Resolve and validate every distinct executable entrypoint selected by one effective manifest.
///
/// This is shared by target discovery and source-authority hashing so an executable cannot be baked without the same
/// path also participating in freshness checks.
pub fn discover_oven_executable_entrypoints(manifest: &ProjectManifest) -> CliResult<BTreeMap<String, PathBuf>> {
    let mut executable_paths = BTreeMap::new();
    if let Some(project) = manifest.project.as_ref() {
        let mut scripts = project.scripts.iter().collect::<Vec<_>>();
        scripts.sort_by(|left, right| left.0.cmp(right.0));
        for (name, configured_path) in scripts {
            let relative = validated_project_output_relative_path(configured_path, "declared script")?;
            let entrypoint = manifest.project_root().join(&relative);
            let metadata = fs::symlink_metadata(&entrypoint).map_err(|error| {
                CliError::failure(format!(
                    "declared Oven project script `{name}` must resolve to a regular file at {}: {error}",
                    entrypoint.display()
                ))
            })?;
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                return Err(CliError::failure(format!(
                    "declared Oven project script `{name}` must resolve to a regular file at {}",
                    entrypoint.display()
                )));
            }
            executable_paths.insert(relative.to_string_lossy().replace('\\', "/"), entrypoint);
        }
    }
    let conventional_main = manifest
        .project_root()
        .join(OvenBakeProjectTarget::Executable.source_relative_path());
    match fs::symlink_metadata(&conventional_main) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            return Err(CliError::failure(format!(
                "Oven project executable target must be a regular file: {}",
                conventional_main.display()
            )));
        }
        Ok(_) => {
            executable_paths.insert(
                OvenBakeProjectTarget::Executable.source_relative_path().to_string(),
                conventional_main,
            );
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(CliError::failure(format!(
                "failed to inspect Oven project executable target {}: {error}",
                conventional_main.display()
            )));
        }
    }
    Ok(executable_paths)
}

/// Resolve every manifest-backed target that an explicit project bake must prepare.
///
/// Declared scripts are first-class executable targets rather than aliases for `src/main.incn`. The conventional main
/// remains an implicit fallback when present, and exact duplicate paths are collapsed so one authored entrypoint is
/// never compiled twice merely because it has more than one script name.
pub fn discover_oven_bake_project_targets(project_root: &Path) -> CliResult<Vec<(OvenBakeProjectTarget, PathBuf)>> {
    let Some(manifest) = discover_effective_project_manifest(project_root)? else {
        return Err(CliError::failure(format!(
            "`incan oven bake --project` requires a loaf.toml project at {}",
            project_root.display()
        )));
    };
    enforce_project_toolchain_constraint(&manifest)?;

    let mut targets = Vec::new();
    let library = manifest
        .project_root()
        .join(OvenBakeProjectTarget::Library.source_relative_path());
    match fs::symlink_metadata(&library) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            return Err(CliError::failure(format!(
                "Oven project library target must be a regular file: {}",
                library.display()
            )));
        }
        Ok(_) => targets.push((OvenBakeProjectTarget::Library, library)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(CliError::failure(format!(
                "failed to inspect Oven project library target {}: {error}",
                library.display()
            )));
        }
    }

    targets.extend(
        discover_oven_executable_entrypoints(&manifest)?
            .into_values()
            .map(|entrypoint| (OvenBakeProjectTarget::Executable, entrypoint)),
    );
    if targets.is_empty() {
        return Err(CliError::failure(format!(
            "`incan oven bake --project` requires {}, {}, or a declared [project.scripts] entry below {}",
            OvenBakeProjectTarget::Library.source_relative_path(),
            OvenBakeProjectTarget::Executable.source_relative_path(),
            manifest.project_root().display()
        )));
    }
    Ok(targets)
}

/// Choose one entry that asks the lock collector for the complete explicit-bake dependency surface.
///
/// Lock collection already includes every declared script and the conventional library. Prefer conventional main for
/// stable existing behavior, then any executable, then the library-only root.
fn oven_bake_dependency_surface_entrypoint(targets: &[(OvenBakeProjectTarget, PathBuf)]) -> Option<&Path> {
    targets
        .iter()
        .find(|(target, entrypoint)| {
            *target == OvenBakeProjectTarget::Executable
                && entrypoint.ends_with(OvenBakeProjectTarget::Executable.source_relative_path())
        })
        .or_else(|| {
            targets
                .iter()
                .find(|(target, _)| *target == OvenBakeProjectTarget::Executable)
        })
        .or_else(|| targets.first())
        .map(|(_, entrypoint)| entrypoint.as_path())
}

/// Publish the canonical semantic lock after an explicit bake has materialized any local provider handoff needed
/// to inspect a rooted workspace.
///
/// A cold rooted workspace can contain a consumer of its own root library. The lock collector must read that
/// library's checked package metadata, while the completed project Loaf must in turn bind the lock that the collector
/// publishes. The explicit bake therefore materializes the provider first, publishes the lock once, and only then
/// seals the final project-output authority. This remains inside the named publisher command and never authorizes a
/// normal build, run, test, or lock command to compile a missing provider.
fn publish_project_lock_after_provider_bake(
    project_root: &Path,
    entrypoint: &Path,
    package_features: &FeatureSelection,
) -> CliResult<PublishedOvenProjectLock> {
    publish_oven_project_lock(project_root, entrypoint, package_features)
}

/// Discover the conventional or explicitly declared binary roots of a project's Rust facet.
fn discover_project_rust_binary_units(manifest: &ProjectManifest) -> CliResult<Vec<(String, PathBuf)>> {
    let mut units = manifest
        .rust_binary_roles()
        .iter()
        .map(|role| (role.name.clone(), manifest.project_root().join(&role.path)))
        .collect::<Vec<_>>();
    if units.is_empty() {
        let rust_root = manifest
            .rust_source
            .as_ref()
            .map(|source| manifest.project_root().join(&source.root))
            .unwrap_or_else(|| manifest.project_root().to_path_buf());
        let source = rust_root.join("src/main.rs");
        if source.is_file() {
            let name = manifest
                .project
                .as_ref()
                .and_then(|project| project.name.clone())
                .unwrap_or_else(|| "rust_unit".to_string());
            units.push((name, source));
        }
    }
    for (_, source) in &units {
        let metadata = fs::symlink_metadata(source).map_err(|error| {
            CliError::failure(format!(
                "project Rust binary source {} cannot be read: {error}",
                source.display()
            ))
        })?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(CliError::failure(format!(
                "project Rust binary source must be a regular file: {}",
                source.display()
            )));
        }
    }
    Ok(units)
}

/// Collect the Rust source tree used by a split project unit, refusing symlinked source entries.
///
/// This conservative closure includes every Rust file below the root's directory. The same complete directory
/// enters receipt evidence, so moving a caller import into a module cannot hide it from selection or reuse identity.
fn project_rust_source_text(source: &Path) -> CliResult<String> {
    let root = source
        .parent()
        .ok_or_else(|| CliError::failure("Rust source has no parent directory"))?;
    let mut pending = vec![root.to_path_buf()];
    let mut files = Vec::new();
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(&directory)
            .map_err(|error| CliError::failure(format!("cannot read Rust source directory: {error}")))?
        {
            let entry = entry.map_err(|error| CliError::failure(format!("cannot read Rust source entry: {error}")))?;
            let kind = entry
                .file_type()
                .map_err(|error| CliError::failure(format!("cannot read Rust source type: {error}")))?;
            if kind.is_symlink() {
                return Err(CliError::failure("Rust source tree must not contain symlinks"));
            }
            if kind.is_dir() {
                pending.push(entry.path());
            } else if kind.is_file() && entry.path().extension().is_some_and(|extension| extension == "rs") {
                files.push(entry.path());
            }
        }
    }
    files.sort();
    let mut text = String::new();
    for file in files {
        text.push_str(
            &fs::read_to_string(&file)
                .map_err(|error| CliError::failure(format!("cannot read Rust module {}: {error}", file.display())))?,
        );
        text.push('\n');
    }
    Ok(text)
}

/// Hash the checked caller surface selected for one library and Rust unit.
fn caller_facet_digest(library: &str, exports: &BTreeSet<String>) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"incan-caller-facet-v1\0");
    hasher.update(library.as_bytes());
    for export in exports {
        hasher.update(b"\0");
        hasher.update(export.as_bytes());
    }
    format!("sha256:{}", hex::encode(hasher.finalize()))
}

#[derive(Debug, Serialize, Deserialize)]
struct RustCallerScanCache {
    source_digest: String,
    paths: BTreeMap<String, BTreeSet<String>>,
}

/// Reuse a syntax scan until the Rust unit's source identity changes.
fn cached_rust_caller_paths(
    project_root: &Path,
    unit_name: &str,
    source: &str,
) -> CliResult<BTreeMap<String, BTreeSet<String>>> {
    let source_digest = format!(
        "sha256:{}",
        hex::encode(Sha256::digest(format!("rust-caller-scan-v3\0{source}").as_bytes()))
    );
    let cache_path = project_root
        .join("target/rust/caller-scans")
        .join(format!("{unit_name}.json"));
    if let Ok(bytes) = fs::read(&cache_path)
        && let Ok(cache) = serde_json::from_slice::<RustCallerScanCache>(&bytes)
        && cache.source_digest == source_digest
    {
        return Ok(cache.paths);
    }
    let paths = scan_rust_caller_paths(source);
    let parent = cache_path
        .parent()
        .ok_or_else(|| CliError::failure(format!("caller scan cache has no parent: {}", cache_path.display())))?;
    fs::create_dir_all(parent)
        .map_err(|error| CliError::failure(format!("cannot create caller scan cache {}: {error}", parent.display())))?;
    let payload = serde_json::to_vec_pretty(&RustCallerScanCache {
        source_digest,
        paths: paths.clone(),
    })
    .map_err(|error| CliError::failure(format!("cannot serialize caller scan cache: {error}")))?;
    fs::write(&cache_path, payload).map_err(|error| {
        CliError::failure(format!(
            "cannot write caller scan cache {}: {error}",
            cache_path.display()
        ))
    })?;
    Ok(paths)
}

/// Collect caller exports from the provider's fully inspected checking pass.
///
/// A standalone checker has no Rust dependency metadata and cannot validate a Body IR field access. Preparing the
/// provider retains its admitted inspection authority and exports from the same checking pass used for emission.
/// Checking emits in scratch and restores receipt pointers; it must not rewrite an admitted provider's physical
/// artifact or change the caller's source authority between its pre-bake observation and publication.
fn checked_library_exports(
    source: &Path,
    package_features: &FeatureSelection,
) -> CliResult<Vec<incan_frontend::library_exports::CheckedNamedExport>> {
    let root = source
        .parent()
        .and_then(Path::parent)
        .and_then(Path::to_str)
        .ok_or_else(|| CliError::failure("caller library root is not UTF-8"))?;
    let scratch = tempfile::Builder::new()
        .prefix("incan-caller-check-")
        .tempdir()
        .map_err(|error| CliError::failure(format!("cannot create caller checking scratch: {error}")))?;
    let output = scratch
        .path()
        .to_str()
        .ok_or_else(|| CliError::failure("caller checking output is not UTF-8"))?;
    let publication = library_publication::LibraryPublication::begin_receipt_update(
        Path::new(root),
        library_publication_receipts(Path::new(root))?,
    )?;
    let result = (|| {
        let prepared = prepare_library_project(
            Some(root),
            Some(output),
            CargoPolicy::default(),
            package_features,
            None,
            Vec::new(),
            false,
            false,
            None,
            true,
            false,
            OvenProjectPlanMode::ExplicitBake,
            None,
        )?;
        let mut exports = prepared.checked_exports;
        resolve_caller_imported_shapes(Path::new(root), &mut exports, package_features)?;
        Ok(exports)
    })();
    publication.finish_inspection(result)
}

/// Retain the original checked public shape behind a same-name packaged import.
///
/// Generated library roots expose these imports with `pub use`, so both a returned imported type and a selected
/// facade item name that same definition. ABI validation inspects the original fields or signature rather than
/// treating an import alias as an opaque permission. Renamed and nested imports remain refused.
fn resolve_caller_imported_shapes(
    root: &Path,
    exports: &mut [incan_frontend::library_exports::CheckedNamedExport],
    package_features: &FeatureSelection,
) -> CliResult<()> {
    use incan_frontend::library_exports::CheckedExportKind;

    let manifest = discover_effective_project_manifest(root)?
        .ok_or_else(|| CliError::failure("caller provider has no manifest"))?;
    let mut providers = BTreeMap::new();
    for export in exports {
        let CheckedExportKind::Alias(alias) = &export.kind else {
            continue;
        };
        if alias.projected_type.is_none() && alias.projected_function.is_none() {
            continue;
        }
        let [namespace, library, name] = alias.target_path.as_slice() else {
            continue;
        };
        if namespace != "pub" || name != &export.name {
            continue;
        }
        let Some(dependency) = manifest.library_dependencies().get(library) else {
            continue;
        };
        if !providers.contains_key(library) {
            // A prepared dependency contract has already resolved its own aliases. Cyclic Loaf dependencies are
            // refused by the provider planner before this checked-export projection can recurse.
            let checked = checked_library_exports(&dependency.path.join("src/lib.incn"), package_features)?;
            providers.insert(library.clone(), checked);
        }
        if let Some(shape) = providers
            .get(library)
            .and_then(|checked| checked.iter().find(|item| &item.name == name))
            && matches!(
                shape.kind,
                CheckedExportKind::Model(_) | CheckedExportKind::Enum(_) | CheckedExportKind::Function(_)
            )
        {
            export.kind = shape.kind.clone();
        }
    }
    Ok(())
}

/// Prove that the host and provider select one identical local semantics-core dependency.
///
/// Canonical paths bind package version and source; normalized dependency configuration binds feature selection.
/// Registry and renamed dependencies remain outside this deliberately narrow Body IR caller boundary.
fn caller_body_ir_identity(host: &ProjectManifest, provider_root: &Path) -> CliResult<bool> {
    let Some(provider) = discover_effective_project_manifest(provider_root)? else {
        return Ok(false);
    };
    let Some(host_dependency) = host.rust_dependencies().get("incan_semantics_core") else {
        return Ok(false);
    };
    let Some(provider_dependency) = provider.rust_dependencies().get("incan_semantics_core") else {
        return Ok(false);
    };
    let mut host_dependency = host_dependency.clone().normalized();
    let mut provider_dependency = provider_dependency.clone().normalized();
    for dependency in [&mut host_dependency, &mut provider_dependency] {
        let oven_model::manifest::DependencySource::Path { path } = &mut dependency.source else {
            return Ok(false);
        };
        *path = fs::canonicalize(&path)
            .map_err(|error| CliError::failure(format!("cannot establish Body IR dependency identity: {error}")))?;
        if dependency.optional || dependency.package.is_some() {
            return Ok(false);
        }
    }
    Ok(host_dependency == provider_dependency)
}

/// Build a Rust-only project's binaries against one sibling Incan Loaf by direct rustc.
struct ProjectRustBakeContext<'a> {
    manifest: &'a ProjectManifest,
    package_features: &'a FeatureSelection,
    rustc: &'a Path,
    target: &'a str,
    toolchain: &'a str,
    project_name: &'a str,
    project_version: &'a str,
}

/// Receipt one Rust source unit with the exact callee and caller-facet identities it consumes.
fn project_rust_unit_receipt(
    context: &ProjectRustBakeContext<'_>,
    unit_name: &str,
    source: &Path,
    profile: &str,
    callee_identity: &str,
    facet_id: &str,
) -> CliResult<(oven_store::OvenReceipt, PathBuf)> {
    let receipt_path = context
        .manifest
        .project_root()
        .join("target/rust/receipts")
        .join(format!("{unit_name}-{profile}.json"));
    let request = OvenGeneratedProjectRequest::new(
        context.manifest.project_root(),
        context.project_name,
        context.project_version,
        context.target,
        context.toolchain,
        profile,
        Vec::new(),
    )
    .with_generated_source("rust-unit", source)
    .with_generated_source_tree(
        "rust-unit-modules",
        source
            .parent()
            .ok_or_else(|| CliError::failure("Rust source has no parent directory"))?,
    )
    .with_build_unit_input("callee-incan-unit", callee_identity)
    .with_build_unit_input("caller-facet", facet_id);
    let mut request = request;
    if let Some(role) = context
        .manifest
        .rust_binary_roles()
        .iter()
        .find(|role| role.name == unit_name)
        && let Some(grant) =
            oven_rustc::rustc::driver_grant::authorize_driver_grant(role, context.rustc).map_err(oven_rustc_error)?
    {
        request = request.with_build_unit_input(oven_rustc::rustc::driver_grant::DRIVER_GRANT_INPUT, grant);
    }
    let evidence = generated_project_source_evidence(&request).map_err(|error| CliError::failure(error.to_string()))?;
    let receipt = receipt_generated_project_with_source_evidence(&request, &evidence)
        .map_err(|error| CliError::failure(error.to_string()))?;
    write_receipt(&receipt, &receipt_path).map_err(|error| CliError::failure(error.to_string()))?;
    Ok((receipt, receipt_path))
}

/// One checked Rust caller unit, shared unchanged by its debug and release profile bakes.
struct ProjectRustCallerUnit<'a> {
    unit_name: &'a str,
    source: &'a Path,
    library: &'a str,
    dependency: &'a oven_model::manifest::LibraryDependencySpec,
    selection: &'a crate::build::caller_facet::CallerFacetSelection,
    facet_id: &'a str,
}

/// Publish or select a Rust unit's own dependency closure without granting permissions to its dependencies.
fn prepare_rust_unit_dependencies(
    context: &ProjectRustBakeContext<'_>,
    profile: &str,
    base_receipt: &oven_store::OvenReceipt,
) -> CliResult<Option<(oven_store::OvenReceipt, crate::build::OvenDirectRustcPlanPreparation)>> {
    if context.manifest.rust_dependencies().is_empty() {
        return Ok(None);
    }

    // ---- Declared dependency roots ----
    let mut dependencies = context
        .manifest
        .rust_dependencies()
        .values()
        .cloned()
        .collect::<Vec<_>>();
    dependencies.sort_by(|left, right| left.crate_name.cmp(&right.crate_name));
    let generated = context
        .manifest
        .project_root()
        .join("target/rust/dependencies")
        .join(profile);
    let mut generator = crate::backend::ProjectGenerator::new(&generated, "incan_rust_unit_dependencies", true);
    generator.set_dependencies(dependencies.clone());
    let mut dependency_source = String::from("use incan_std_core::{self as _};\n");
    for dependency in &dependencies {
        dependency_source.push_str(&format!(
            "use {}::{{self as _}};\n",
            dependency.crate_name.replace('-', "_")
        ));
    }
    dependency_source.push_str("fn main() {}\n");
    generator
        .generate(&dependency_source)
        .map_err(|error| CliError::failure(error.to_string()))?;

    // ---- Receipt-bound dependency publisher ----
    let request = OvenGeneratedProjectRequest::new(
        context.manifest.project_root(),
        "incan-rust-unit-dependencies",
        context.project_version,
        context.target,
        context.toolchain,
        profile,
        Vec::new(),
    )
    .with_generated_source("generated-root", generator.crate_root_path());
    let mut request = request;
    for (name, value) in &base_receipt.sources.build_unit_inputs {
        if name != oven_rustc::rustc::driver_grant::DRIVER_GRANT_INPUT {
            request = request.with_build_unit_input(name, value);
        }
    }
    let dependency_digest =
        oven_store::digest_dependency_specs(&dependencies, incan_oven_facet::provider_hooks().as_ref())
            .map_err(|error| CliError::failure(error.to_string()))?;
    request = request.with_build_unit_input("rust-dependencies", dependency_digest);
    let receipt =
        oven_store::receipt_generated_project(&request).map_err(|error| CliError::failure(error.to_string()))?;

    // ---- Explicit plan selection under lease ----
    let store = open_default_oven_store()?;
    let preparation = crate::build::plan_selection::select_or_bake_generated_project_plan(
        OvenProjectPlanMode::ExplicitBake,
        &store,
        &receipt,
        crate::build::OvenProjectDependencySurface {
            selection: &dependencies,
            provider_compilations: &[],
        },
        &generated,
        &generator.crate_root_path(),
        context.rustc,
    )?
    .ok_or_else(|| CliError::failure("Rust unit dependency closure was not prepared"))?;
    Ok(Some((receipt, preparation)))
}

/// Compose separately leased native closures, retaining direct roots and all transitive search bindings.
fn compose_rust_unit_dependencies(
    plan: &mut oven_rustc::rustc::OvenRustcArtifactPlan,
    preparation: &crate::build::OvenDirectRustcPlanPreparation,
    manifest: &ProjectManifest,
) -> CliResult<()> {
    let own = preparation
        .plan_selection
        .source_artifact_plan("generated-root")
        .map_err(oven_rustc_error)?;
    let mut libraries = Vec::new();
    let declared = manifest.declared_rust_crate_names();
    for (name, output) in &own.externs {
        if !declared.contains(name) {
            continue;
        }
        // The unit's declared root owns its alias; the sibling library retains its transitive metadata searches.
        libraries.push(OvenCallerOwnedRustcLibrary {
            crate_name: name.clone(),
            output: output.clone(),
            digest: oven_store::digest_bytes(&fs::read(output).map_err(|error| CliError::failure(error.to_string()))?),
            expose_extern: true,
        });
    }
    replace_rust_library_bindings(plan, &libraries)?;
    for path in own.dependency_search_paths {
        plan.retain_caller_dependency_search_path(path);
    }
    plan.native_search_paths.extend(own.native_search_paths);
    plan.native_search_paths.sort();
    plan.native_search_paths.dedup();
    Ok(())
}

/// Prepare a usage-derived caller projection without modifying its admitted sibling provider.
///
/// Generated caller facets belong to the Rust caller's output tree. Provider receipt pointers and canonical lock
/// bytes are restored after preparation; the returned plan owns its checked receipt and retained artifact evidence.
fn prepare_rust_caller_library(
    context: &ProjectRustBakeContext<'_>,
    unit: &ProjectRustCallerUnit<'_>,
    profile: &str,
) -> CliResult<PreparedLibraryProject> {
    let ProjectRustCallerUnit {
        unit_name,
        library,
        dependency,
        selection,
        facet_id,
        ..
    } = *unit;
    let receipt_reference = format!("target/rust/receipts/{unit_name}-{profile}.json");
    let caller = CallerFacetRequest {
        exports: selection.exports.iter().cloned().collect(),
        facet_id: facet_id.to_string(),
        receipt_reference,
        target: context.target.to_string(),
        profile: profile.to_string(),
    };
    let dependency_path = dependency.path.to_str().ok_or_else(|| {
        CliError::failure(format!(
            "Incan dependency path is not UTF-8: {}",
            dependency.path.display()
        ))
    })?;
    let output = context
        .manifest
        .project_root()
        .join("target/rust/caller-libraries")
        .join(validated_project_output_relative_path(unit_name, "Rust unit")?)
        .join(validated_project_output_relative_path(library, "caller library")?)
        .join(profile);
    let output = output
        .to_str()
        .ok_or_else(|| CliError::failure("caller library output is not UTF-8"))?;
    let publication = library_publication::LibraryPublication::begin_receipt_update(
        &dependency.path,
        library_publication_receipts(&dependency.path)?,
    )?;
    let result = prepare_library_project_with_caller_facet(
        Some(dependency_path),
        Some(output),
        CargoPolicy::default(),
        context.package_features,
        None,
        Vec::new(),
        false,
        false,
        None,
        true,
        false,
        OvenProjectPlanMode::ExplicitBake,
        None,
        Some(&caller),
    );
    publication.finish_inspection(result).map_err(|error| {
        CliError::failure(format!(
            "failed to prepare caller library `{library}` for `{unit_name}`: {error}"
        ))
    })
}

/// Build one profile of one Rust unit against its selected Incan caller artifact.
fn bake_project_rust_profile(
    context: &ProjectRustBakeContext<'_>,
    unit: &ProjectRustCallerUnit<'_>,
    profile: &str,
) -> CliResult<OvenProjectBakeProfileReport> {
    let ProjectRustCallerUnit {
        unit_name,
        source,
        library,
        facet_id,
        ..
    } = *unit;

    // ---- Checked sibling Incan caller ----
    let prepared = prepare_rust_caller_library(context, unit, profile)?;
    let oven = prepared
        .oven
        .as_ref()
        .ok_or_else(|| CliError::failure("caller library preparation did not produce an Oven plan"))?;
    let selected = oven
        .profiles
        .get(profile)
        .ok_or_else(|| CliError::failure(format!("caller library has no `{profile}` Oven profile")))?;
    let own_dependencies = prepare_rust_unit_dependencies(context, profile, &selected.receipt)?;
    let host_body_ir_plan = if caller_body_ir_identity(context.manifest, &prepared.project_root)? {
        own_dependencies
            .as_ref()
            .map(|(_, preparation)| {
                preparation
                    .plan_selection
                    .source_artifact_plan("generated-root")
                    .map_err(oven_rustc_error)
            })
            .transpose()?
    } else {
        None
    };
    let (library_bake, mut artifact_plan) =
        bake_oven_library_with_dependencies(&prepared, oven, profile, None, host_body_ir_plan.as_ref()).map_err(
            |error| {
                CliError::failure(format!(
                    "failed to bake caller library `{library}` for `{unit_name}`: {error}"
                ))
            },
        )?;
    attach_caller_owned_rustc_libraries(
        &mut artifact_plan,
        &[OvenCallerOwnedRustcLibrary {
            crate_name: library.replace('-', "_"),
            output: library_bake.output,
            digest: library_bake.output_digest,
            expose_extern: true,
        }],
    )
    .map_err(oven_rustc_error)?;

    // ---- Rust dependency composition and unit receipt ----
    if let Some((_, preparation)) = &own_dependencies {
        compose_rust_unit_dependencies(&mut artifact_plan, preparation, context.manifest)?;
    }
    let (mut receipt, receipt_path) = project_rust_unit_receipt(
        context,
        unit_name,
        source,
        profile,
        &selected.receipt.identity,
        facet_id,
    )?;
    if let Some((dependency_receipt, preparation)) = &own_dependencies {
        receipt = oven_store::receipt_with_build_unit_input(
            &receipt,
            "rust-unit-dependencies",
            format!(
                "{}:{}",
                dependency_receipt.identity,
                preparation.plan_selection.report_identity()
            ),
        )
        .map_err(|error| CliError::failure(error.to_string()))?;
        write_receipt(&receipt, &receipt_path).map_err(|error| CliError::failure(error.to_string()))?;
    }

    // ---- Receipt-authorized native compilation ----
    let output = context
        .manifest
        .project_root()
        .join("target/rust")
        .join(profile)
        .join(unit_name);
    let bake = bake_trusted_direct_rustc_run_with_artifact_role(
        &OvenTrustedDirectRustcTargetRequest {
            receipt: &receipt,
            artifacts: selected.plan_selection.artifacts(),
            artifact_root: selected.plan_selection.output_guard_root(),
            artifact_plan: Some(&artifact_plan),
            rustc: context.rustc,
            source,
            output: &output,
            crate_name: &unit_name.replace('-', "_"),
            edition: "2024",
            source_evidence_key: "rust-unit",
            features: &[],
            prefer_dynamic: false,
        },
        "generated-root",
    )
    .map_err(oven_rustc_error)?;
    Ok(OvenProjectBakeProfileReport {
        project_target: format!("rust:{unit_name}"),
        profile: profile.to_string(),
        target: context.target.to_string(),
        toolchain: context.toolchain.to_string(),
        receipt: receipt_path,
        receipt_identity: receipt.identity,
        build_unit_identity: receipt.build_unit_identity,
        plan_identity: selected.plan_selection.report_identity(),
        action: if bake.reused { "reused" } else { "baked" },
    })
}

/// Build every Rust-only project binary against its checked sibling Incan caller facet.
fn bake_project_rust_units(
    manifest: &ProjectManifest,
    units: &[(String, PathBuf)],
    package_features: &FeatureSelection,
    requested_target: Option<&str>,
) -> CliResult<OvenProjectBakeReport> {
    if manifest.library_dependencies().is_empty() {
        return Err(CliError::failure(
            "a project Rust unit must declare an Incan `loaf` dependency",
        ));
    }
    let rustc = oven_rustc::rustc::resolve_active_rustc().map_err(oven_rustc_error)?;
    let target = requested_target
        .map(str::to_owned)
        .unwrap_or(oven_rustc::rustc::rustc_host_target(&rustc).map_err(oven_rustc_error)?);
    let toolchain = oven_rustc::rustc::rustc_identity(&rustc).map_err(oven_rustc_error)?;
    let project_name = manifest
        .project
        .as_ref()
        .and_then(|project| project.name.clone())
        .unwrap_or_else(|| "rust_unit".to_string());
    let project_version = manifest
        .project
        .as_ref()
        .and_then(|project| project.version.clone())
        .unwrap_or_else(|| "0.1.0".to_string());
    let store = open_default_oven_store()?;
    let context = ProjectRustBakeContext {
        manifest,
        package_features,
        rustc: &rustc,
        target: &target,
        toolchain: &toolchain,
        project_name: &project_name,
        project_version: &project_version,
    };
    let mut profiles = Vec::new();
    let mut generated_sources = BTreeMap::new();

    for (unit_name, source) in units {
        let source_text = project_rust_source_text(source)?;
        let requested_by_library = cached_rust_caller_paths(manifest.project_root(), unit_name, &source_text)?;
        if requested_by_library.is_empty() {
            return Err(CliError::failure(format!(
                "project Rust unit `{unit_name}` does not reference an Incan `<library>::caller::incan::<export>` path"
            )));
        }
        if requested_by_library.len() > 1 {
            return Err(CliError::failure(format!(
                "project Rust unit `{unit_name}` references more than one Incan caller library; this bounded planner accepts one sibling Loaf per Rust unit"
            )));
        }
        for (library, requested) in requested_by_library {
            let dependency = manifest.library_dependencies().get(&library).ok_or_else(|| {
                CliError::failure(format!("Rust caller path names undeclared Incan Loaf `{library}`"))
            })?;
            let library_source = dependency.path.join("src/lib.incn");
            let checked = checked_library_exports(&library_source, package_features)?;
            let body_ir_identity = caller_body_ir_identity(manifest, &dependency.path)?;
            let selection =
                select_checked_caller_exports_with_body_ir(&library, &requested, &checked, body_ir_identity)
                    .map_err(CliError::failure)?;
            let facet_id = caller_facet_digest(&library, &requested);
            let unit = ProjectRustCallerUnit {
                unit_name,
                source,
                library: &library,
                dependency,
                selection: &selection,
                facet_id: &facet_id,
            };
            for profile in explicit_bake_profiles() {
                generated_sources.insert(format!("rust:{unit_name}"), source.clone());
                profiles.push(bake_project_rust_profile(&context, &unit, profile)?);
            }
        }
    }
    Ok(OvenProjectBakeReport {
        project: manifest.project_root().to_path_buf(),
        generated_sources,
        store: store.root().to_path_buf(),
        profiles,
        outputs: Vec::new(),
    })
}

/// Explicitly prepare compatible Oven closures for every manifest-backed target in one Incan project.
///
/// This is preparation rather than execution: it records fresh source/lock/SDK/provider receipt evidence, reuses a
/// matching stored or release-scoped stdlib closure when available, and otherwise crosses Oven's explicit bounded
/// publisher exactly once per genuinely missing target/profile. Normal `build`, `run`, and `test` remain Cargo-free
/// consumers of the resulting direct-rustc plans.
pub fn bake_oven_project_targets(
    project: &Path,
    package_features: &FeatureSelection,
    requested_target: Option<&str>,
) -> CliResult<OvenProjectBakeReport> {
    if requested_target.is_some_and(|target| target.trim().is_empty()) {
        return Err(CliError::failure("explicit Oven bake target must not be empty"));
    }
    let project = project
        .to_str()
        .ok_or_else(|| CliError::failure(format!("Oven project path is not valid UTF-8: {}", project.display())))?;
    let project_root = resolve_library_project_root(Some(project))?;
    let manifest = discover_effective_project_manifest(&project_root)?.ok_or_else(|| {
        CliError::failure(format!(
            "`incan oven bake --project` requires a loaf.toml project at {}",
            project_root.display()
        ))
    })?;
    let rust_units = discover_project_rust_binary_units(&manifest)?;
    let has_incan_target = project_root
        .join(OvenBakeProjectTarget::Library.source_relative_path())
        .is_file()
        || project_root
            .join(OvenBakeProjectTarget::Executable.source_relative_path())
            .is_file()
        || manifest
            .project
            .as_ref()
            .is_some_and(|project| !project.scripts.is_empty());
    if !rust_units.is_empty() && !has_incan_target {
        let store = open_default_oven_store()?;
        let key = crate::build::rust_bake_reuse::rust_bake_reuse_key(&manifest, package_features, requested_target)?;
        if let Some(key) = &key
            && let Some(report) =
                crate::build::rust_bake_reuse::try_reuse_rust_bake(&project_root, &rust_units, &store, key)?
        {
            return Ok(report);
        }
        if std::env::var_os("INCAN_TEST_REQUIRE_COMPLETED_BAKE_REUSE").is_some() {
            return Err(CliError::failure(
                "completed Rust caller reuse missed before frontend preparation",
            ));
        }
        let report = bake_project_rust_units(&manifest, &rust_units, package_features, requested_target)?;
        if let Some(key) = &key
            && crate::build::rust_bake_reuse::rust_bake_reuse_key(&manifest, package_features, requested_target)?
                .as_ref()
                .is_some_and(|current| current == key)
        {
            crate::build::rust_bake_reuse::publish_rust_bake(&store, key, &report)?;
        }
        return Ok(report);
    }
    let targets = discover_oven_bake_project_targets(&project_root)?;
    let dependency_surface_entrypoint = oven_bake_dependency_surface_entrypoint(&targets)
        .ok_or_else(|| CliError::failure("explicit Oven project bake discovered no dependency-surface entrypoint"))?
        .to_path_buf();
    let store = open_default_oven_store()?;
    let mut authority_context = OvenProjectBakeAuthorityContext {
        requested_target: requested_target.map(str::to_owned),
        ..OvenProjectBakeAuthorityContext::default()
    };
    if canonical_baked_project_lock_path(&project_root)?.is_file()
        && let Some(reused) = try_reuse_baked_project(
            &project_root,
            &targets,
            &store,
            package_features,
            requested_target,
            &mut authority_context,
        )?
    {
        return Ok(reused);
    }
    if std::env::var_os("INCAN_TEST_REQUIRE_COMPLETED_BAKE_REUSE").is_some() {
        return Err(CliError::failure(
            "completed project reuse missed before frontend preparation",
        ));
    }
    let mut source_authority_digest = None;
    let mut published_project_lock = None;
    let mut generated_sources = BTreeMap::new();
    let mut profiles = Vec::new();
    let mut pending_outputs = Vec::new();
    let mut debug_target_receipts = Vec::new();
    let mut library_inspection_constituent: Option<LibraryInspectionConstituent> = None;
    // Every prepared target keeps the store leases of the plans it selected or published. Those leases must outlive
    // the whole bake, not just the target's own loop arm: the inspection authority sealed after the loop names those
    // entries as constituents, and a later admission in the same bake (the test-dependency envelope, the authority
    // itself) prunes unleased entries when the domain policy is tight. A bake that succeeds must leave a loadable
    // closure, so its constituents stay leased until the authority is sealed; if the policy cannot hold them all,
    // admission fails loudly instead.
    let mut retained_preparations: Vec<PreparedLibraryProject> = Vec::new();
    // The same holds for an executable target: its debug plan is published one profile before its release plan, and
    // the release publisher's staging reservation reclaims unleased entries oldest-first (#1230). Dropping the debug
    // preparation at the end of its loop arm handed that plan to the reclaimer.
    let mut retained_executable_preparations: Vec<OvenPreparedProject> = Vec::new();
    #[cfg(feature = "rust_inspect")]
    let mut rust_inspect_manifest_dirs = BTreeSet::new();

    // The explicit bake retains its package Loaf store across generations for the same reason normal replay does:
    // the store is content-addressed, so an entry that is already there is already the entry this bake would
    // write. Without this the artifact root is staged empty and every entry is re-copied and re-`fsync`ed --
    // 2,905 files and 154 MB on a two-line library whose plan the bake itself reports as reused.
    let publication = if targets
        .iter()
        .any(|(target, _)| *target == OvenBakeProjectTarget::Library)
    {
        Some(
            library_publication::LibraryPublication::begin(
                &project_root,
                &project_root.join("target/lib"),
                library_publication_receipts(&project_root)?,
            )?
            .retaining_package_cache(),
        )
    } else {
        None
    };
    let result = (|| {
        for (target, entrypoint) in targets {
            match target {
                OvenBakeProjectTarget::Library => {
                    let mut prepared = prepare_library_project(
                        Some(project),
                        None,
                        CargoPolicy::default(),
                        package_features,
                        None,
                        Vec::new(),
                        false,
                        false,
                        None,
                        true,
                        false,
                        OvenProjectPlanMode::ExplicitBake,
                        Some(&mut authority_context),
                    )?;
                    #[cfg(feature = "rust_inspect")]
                    if let Some(manifest_dir) = prepared.rust_inspect_manifest_dir.as_ref() {
                        rust_inspect_manifest_dirs.insert(manifest_dir.clone());
                    }
                    let selected = prepared.oven.as_ref().ok_or_else(|| {
                        CliError::failure("explicit Oven library preparation did not produce a direct-rustc selection")
                    })?;
                    let backend_receipt = prepared.report.backend.clone().ok_or_else(|| {
                        CliError::failure("explicit Oven library preparation did not produce backend provenance")
                    })?;
                    generated_sources.insert(
                        oven_bake_project_target_identity(&project_root, target, &prepared.entrypoint)?,
                        prepared.generator.crate_root_path(),
                    );
                    let package_store_root = packaged_library_loaf_store_root(&prepared.out_dir);
                    let mut package_profiles = BTreeMap::new();
                    let mut completed_outputs = Vec::new();
                    for (profile, selected_profile) in &selected.profiles {
                        if profile == "debug" {
                            debug_target_receipts.push(selected_profile.receipt.clone());
                        }
                        if profile == "debug" {
                            // The library's debug plan is the constituent that lets a test unit inspect the library's
                            // dependencies. A direct-rustc bake stores it whole, and its rust-inspect workspace holds
                            // the Cargo bootstrap's generated Rust. When the closure is not loadable as independently
                            // compiled parts, the bounded compatibility baker publishes the library as a store-owned
                            // extension of a compiler Loaf instead; the composed manifest under the extension's
                            // identity is the same constituent, and the generated project's own Cargo target holds
                            // the generated Rust.
                            let constituent = match &selected_profile.plan_selection {
                                OvenDirectRustcPlanSelection::Stored(plan) => Some((
                                    plan.identity.clone(),
                                    plan.artifacts.clone(),
                                    OvenArtifactKind::DirectRustcPlan,
                                    None,
                                )),
                                OvenDirectRustcPlanSelection::ProjectExtension(extension) => Some((
                                    extension.extension.identity.clone(),
                                    extension.artifacts.clone(),
                                    OvenArtifactKind::ProjectPayload,
                                    Some(extension.base.loaf_identity.clone()),
                                )),
                                OvenDirectRustcPlanSelection::ToolchainLoaf(_)
                                | OvenDirectRustcPlanSelection::PackagedProvider(_) => None,
                            };
                            if let Some((identity, artifacts, artifact_kind, base_loaf_identity)) = constituent {
                                library_inspection_constituent = Some(LibraryInspectionConstituent {
                                    identity,
                                    artifact_kind,
                                    base_loaf_identity,
                                    receipt: selected_profile.receipt.clone(),
                                    artifacts,
                                    rust_inspect_manifest_dir: prepared.rust_inspect_manifest_dir.clone(),
                                    cargo_target_dir: Some(prepared.generator.cargo_target_dir()),
                                    generated_project_dir: Some(prepared.generator.output_dir().to_path_buf()),
                                });
                            }
                        }
                        let bake = bake_oven_library(&prepared, selected, profile, Some(&mut authority_context))?;
                        let library_relative_path = bake
                            .output
                            .strip_prefix(&prepared.out_dir)
                            .map_err(|_| {
                                CliError::failure(format!(
                                    "baked public library output {} escaped its artifact root {}",
                                    bake.output.display(),
                                    prepared.out_dir.display()
                                ))
                            })?
                            .to_string_lossy()
                            .replace('\\', "/");
                        let entries = export_selected_package_loaf(
                            &store,
                            &package_store_root,
                            &selected_profile.receipt,
                            &selected_profile.plan_selection,
                        )?;
                        completed_outputs.push((
                            profile.clone(),
                            selected_profile.receipt.clone(),
                            selected_profile.plan_selection.report_identity(),
                            bake.output.clone(),
                            entries.clone(),
                        ));
                        package_profiles.insert(
                            profile.clone(),
                            OvenPackagedLibraryLoafProfile {
                                receipt: selected_profile.receipt.clone(),
                                entries,
                                library_relative_path,
                                library_digest: bake.output_digest,
                            },
                        );
                        let receipt = project_bake_receipt_path(&project_root, target, &prepared.entrypoint, profile)?;
                        write_receipt(&selected_profile.receipt, &receipt)
                            .map_err(|error| CliError::failure(error.to_string()))?;
                        profiles.push(OvenProjectBakeProfileReport {
                            project_target: oven_bake_project_target_identity(
                                &project_root,
                                target,
                                &prepared.entrypoint,
                            )?,
                            profile: profile.clone(),
                            target: selected_profile.receipt.intent.target.clone(),
                            toolchain: selected_profile.receipt.intent.toolchain.clone(),
                            receipt,
                            receipt_identity: selected_profile.receipt.identity.clone(),
                            build_unit_identity: selected_profile.receipt.build_unit_identity.clone(),
                            plan_identity: selected_profile.plan_selection.report_identity(),
                            action: selected_profile.materialization.as_str(),
                        });
                    }
                    write_library_manifest_artifacts(&mut prepared)?;
                    published_project_lock = Some(publish_project_lock_after_provider_bake(
                        &project_root,
                        &dependency_surface_entrypoint,
                        package_features,
                    )?);
                    authority_context.lock_published();
                    source_authority_digest = Some(authority_context.project_source_authority(&project_root)?);
                    let source_authority_digest = source_authority_digest.as_deref().ok_or_else(|| {
                        CliError::failure("explicit Oven library bake lost its final source authority")
                    })?;
                    let library_sidecars =
                        library_project_output_sidecars(&prepared.library_manifest, &prepared.out_dir)?;
                    let metadata_files = packaged_library_metadata_files(
                        &prepared.manifest_path,
                        &prepared.library_manifest,
                        &prepared.out_dir,
                    )?;
                    let published_manifest = OvenPackagedLibraryLoafManifest {
                        schema_version: OVEN_PACKAGED_LIBRARY_LOAF_SCHEMA_VERSION,
                        source_authority_digest: source_authority_digest.to_string(),
                        compiler_version: INCAN_VERSION.to_string(),
                        metadata_files,
                        profiles: package_profiles,
                    };
                    write_packaged_library_loaf_manifest(&prepared.out_dir, &published_manifest)?;
                    let package_loaf_manifest = packaged_library_loaf_manifest_path(&prepared.out_dir);
                    let package_loaf_store_relative_path = package_store_root
                        .strip_prefix(&prepared.project_root)
                        .map_err(|_| {
                            CliError::failure(format!(
                                "baked package Loaf store {} escaped project root {}",
                                package_store_root.display(),
                                prepared.project_root.display()
                            ))
                        })?
                        .to_string_lossy()
                        .replace('\\', "/");
                    for (profile, receipt, plan_identity, native_output, required_project_loafs) in completed_outputs {
                        let files = project_output_bake_files(
                            &prepared.project_root,
                            &prepared.generator,
                            &native_output,
                            Some(&prepared.manifest_path),
                            Some(&package_loaf_manifest),
                            &library_sidecars,
                        )?;
                        pending_outputs.push(PendingOvenProjectOutput {
                            entrypoint: prepared.entrypoint.clone(),
                            target,
                            receipt,
                            plan_identity,
                            profile,
                            files,
                            required_project_loafs,
                            package_loaf_store_relative_path: Some(package_loaf_store_relative_path.clone()),
                            backend_receipt: backend_receipt.clone(),
                            build_report: None,
                        });
                    }
                    remove_completed_generated_cargo_lock(prepared.generator.output_dir())?;
                    retained_preparations.push(prepared);
                }
                OvenBakeProjectTarget::Executable => {
                    if published_project_lock.is_none() {
                        published_project_lock = Some(publish_project_lock_after_provider_bake(
                            &project_root,
                            &dependency_surface_entrypoint,
                            package_features,
                        )?);
                        authority_context.lock_published();
                        source_authority_digest = Some(authority_context.project_source_authority(&project_root)?);
                    }
                    source_authority_digest.as_deref().ok_or_else(|| {
                        CliError::failure("explicit Oven executable bake lost its final source authority")
                    })?;
                    let entrypoint = entrypoint.to_str().ok_or_else(|| {
                        CliError::failure(format!("Oven entrypoint is not valid UTF-8: {}", entrypoint.display()))
                    })?;
                    let target_output_dir = oven_bake_executable_output_dir(&project_root, Path::new(entrypoint))?;
                    let target_output_dir = target_output_dir
                        .as_deref()
                        .map(|path| {
                            path.to_str().ok_or_else(|| {
                                CliError::failure(format!(
                                    "Oven target output path is not valid UTF-8: {}",
                                    path.display()
                                ))
                            })
                        })
                        .transpose()?;
                    for profile in explicit_bake_profiles() {
                        let prepared = prepare_oven_project(
                            entrypoint,
                            target_output_dir,
                            &CargoPolicy::default(),
                            package_features,
                            None,
                            Vec::new(),
                            false,
                            false,
                            profile,
                            OvenProjectPlanMode::ExplicitBake,
                            Some(&mut authority_context),
                        )?;
                        #[cfg(feature = "rust_inspect")]
                        if let Some(manifest_dir) = prepared.rust_inspect_manifest_dir.as_ref() {
                            rust_inspect_manifest_dirs.insert(manifest_dir.clone());
                        }
                        if profile == "debug" {
                            debug_target_receipts.push(prepared.receipt.clone());
                        }
                        let receipt = project_bake_receipt_path(&project_root, target, &prepared.entrypoint, profile)?;
                        write_receipt(&prepared.receipt, &receipt)
                            .map_err(|error| CliError::failure(error.to_string()))?;
                        let bake = bake_oven_project(&prepared, profile, Some(&mut authority_context))?;
                        let backend_receipt = prepared.report.backend.clone().ok_or_else(|| {
                            CliError::failure("explicit Oven executable preparation did not produce backend provenance")
                        })?;
                        let mut report = prepared.report.clone();
                        report.artifacts.push(artifact_report("binary", &bake.output));
                        let build_report =
                            project_output_report_snapshot(&project_root, &report.finish(BTreeMap::new()))?;
                        let files = project_output_bake_files(
                            &prepared.project_root,
                            &prepared.generator,
                            &bake.output,
                            None,
                            None,
                            &[],
                        )?;
                        pending_outputs.push(PendingOvenProjectOutput {
                            entrypoint: prepared.entrypoint.clone(),
                            target,
                            receipt: prepared.receipt.clone(),
                            plan_identity: prepared.plan_selection.report_identity(),
                            profile: profile.to_string(),
                            files,
                            required_project_loafs: Vec::new(),
                            package_loaf_store_relative_path: None,
                            backend_receipt,
                            build_report: Some(build_report),
                        });
                        remove_completed_generated_cargo_lock(prepared.generator.output_dir())?;
                        let target_identity =
                            oven_bake_project_target_identity(&project_root, target, &prepared.entrypoint)?;
                        generated_sources
                            .entry(target_identity.clone())
                            .or_insert_with(|| prepared.generator.crate_root_path());
                        profiles.push(OvenProjectBakeProfileReport {
                            project_target: target_identity,
                            profile: profile.to_string(),
                            target: prepared.receipt.intent.target.clone(),
                            toolchain: prepared.receipt.intent.toolchain.clone(),
                            receipt,
                            receipt_identity: prepared.receipt.identity.clone(),
                            build_unit_identity: prepared.receipt.build_unit_identity.clone(),
                            plan_identity: prepared.plan_selection.report_identity(),
                            action: prepared.materialization.as_str(),
                        });
                        retained_executable_preparations.push(prepared);
                    }
                }
            }
        }
        source_authority_digest
            .as_deref()
            .ok_or_else(|| CliError::failure("explicit Oven bake did not finalize its project source authority"))?;
        let dependency_surface = published_project_lock
            .as_ref()
            .ok_or_else(|| CliError::failure("explicit Oven bake did not retain its canonical project lock"))?
            .dependency_surface();
        let test_dependency_envelope = prepare_oven_test_dependency_envelope(
            &store,
            &project_root,
            dependency_surface,
            &debug_target_receipts,
            Some(&mut authority_context),
        )?;
        let (registry_dependencies, dev_registry_dependencies) =
            canonical_project_inspection_dependencies(dependency_surface)?;
        let source_authority_digest = authority_context.final_project_source_authority(&project_root)?;
        #[cfg(feature = "rust_inspect")]
        let project_locked_registry_packages = project_locked_registry_packages(
            rust_inspect_manifest_dirs
                .iter()
                .map(|manifest_dir| manifest_dir.join("Cargo.lock"))
                .collect::<Vec<_>>()
                .iter()
                .map(PathBuf::as_path),
        )?;
        #[cfg(not(feature = "rust_inspect"))]
        let project_locked_registry_packages = Vec::new();
        let inspection_authority = publish_project_inspection_authority(
            &store,
            &project_root,
            &source_authority_digest,
            ProjectInspectionDependencyAuthority {
                registry_dependencies: &registry_dependencies,
                dev_registry_dependencies: &dev_registry_dependencies,
                locked_registry_packages: &project_locked_registry_packages,
            },
            &test_dependency_envelope,
            library_inspection_constituent.as_ref(),
        )?;
        let lock_dependencies_fingerprint = baked_project_lock_dependencies_fingerprint(&project_root)?;
        let mut published_outputs = Vec::with_capacity(pending_outputs.len());
        for pending in pending_outputs {
            let files = pending.files;
            let payload = project_output_payload_for_bake(OvenProjectOutputBakeRequest {
                project_root: &project_root,
                entrypoint: &pending.entrypoint,
                target: pending.target,
                receipt: &pending.receipt,
                plan_identity: pending.plan_identity,
                profile: &pending.profile,
                source_authority_digest: &source_authority_digest,
                lock_dependencies_fingerprint: lock_dependencies_fingerprint.clone(),
                files: files.clone(),
                inspection_authority: inspection_authority.reference.clone(),
                required_project_loafs: pending.required_project_loafs,
                package_loaf_store_relative_path: pending.package_loaf_store_relative_path,
                backend_receipt: pending.backend_receipt,
                build_report: pending.build_report,
            })?;
            published_outputs.push(publish_project_output_loaf(&store, &pending.receipt, &payload, &files)?);
        }
        // Keep every sibling output and the authority leased through completion. A tight policy must fail this bake
        // rather than prune an earlier target/profile and then report a partial project as successfully prepared.
        // The inspection authority names the debug test dependency envelope's exact plan. Retain that selection until
        // every output Loaf is visible: otherwise a later output admission can prune the now-unleased constituent and
        // leave a source-current authority that points at a missing closure.
        let outputs = published_outputs
            .iter()
            .map(OvenProjectBakeOutputReport::from)
            .collect();
        let _complete_publication_set = (test_dependency_envelope, inspection_authority, published_outputs);
        #[cfg(feature = "rust_inspect")]
        for manifest_dir in rust_inspect_manifest_dirs {
            mark_oven_direct_rust_inspection(&manifest_dir)?;
        }
        Ok(OvenProjectBakeReport {
            project: project_root,
            generated_sources,
            store: store.root().to_path_buf(),
            profiles,
            outputs,
        })
    })();
    match publication {
        Some(publication) => publication.finish(result),
        None => result,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    use crate::build::source_authority::{digest_baked_project_source_authority, project_bake_receipt_path};
    use crate::build::{
        OvenBakeProjectTarget, oven_bake_executable_output_dir, oven_bake_project_target_identity,
        oven_executable_entrypoint_evidence_key,
    };

    use oven_rustc::rustc::OvenRustcError;

    /// Export inspection keeps an admitted provider's physical bytes and canonical pointers intact.
    #[test]
    fn checked_exports_preserve_published_provider() -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        fs::create_dir_all(project.path().join("src"))?;
        fs::write(
            project.path().join("loaf.toml"),
            "[project]\nname = \"exports_probe\"\nversion = \"0.1.0\"\n",
        )?;
        fs::write(
            project.path().join("src/lib.incn"),
            "pub def answer() -> int:\n    return 42\n",
        )?;
        let artifact = project.path().join("target/lib");
        fs::create_dir_all(artifact.join("src"))?;
        fs::write(
            artifact.join("Cargo.toml"),
            "[package]\nname = \"published_probe\"\nversion = \"0.1.0\"\n",
        )?;
        fs::write(artifact.join("src/lib.rs"), "pub fn admitted() {}\n")?;
        let expected = incan_frontend::library_manifest::digest_provider_artifact(&artifact)?;
        let authority = digest_baked_project_source_authority(project.path())?;
        let receipts = library_publication_receipts(project.path())?;
        let before = receipts.iter().map(|path| fs::read(path).ok()).collect::<Vec<_>>();
        for _ in 0..2 {
            let exports = checked_library_exports(&project.path().join("src/lib.incn"), &FeatureSelection::default())?;
            assert!(exports.iter().any(|export| export.name == "answer"));
            assert_eq!(
                expected,
                incan_frontend::library_manifest::digest_provider_artifact(&artifact)?
            );
            assert_eq!(authority, digest_baked_project_source_authority(project.path())?);
            assert_eq!(
                before,
                receipts.iter().map(|path| fs::read(path).ok()).collect::<Vec<_>>()
            );
        }
        Ok(())
    }

    /// Checking a caller's provider must also preserve its already prepared transitive dependency projection.
    #[test]
    #[ignore = "requires a prepared native SDK and standard-library family"]
    fn checked_exports_preserve_transitive_provider() -> Result<(), Box<dyn std::error::Error>> {
        let workspace = tempfile::tempdir()?;
        let child = workspace.path().join("child");
        let parent = workspace.path().join("parent");
        fs::create_dir_all(child.join("src"))?;
        fs::create_dir_all(parent.join("src"))?;
        fs::write(
            child.join("loaf.toml"),
            "[project]\nname = \"child\"\nversion = \"0.1.0\"\n",
        )?;
        fs::write(
            child.join("src/lib.incn"),
            "pub def answer() -> int:\n    \"\"\"Return the child's answer.\"\"\"\n    return 42\n",
        )?;
        fs::write(
            parent.join("loaf.toml"),
            "[project]\nname = \"parent\"\nversion = \"0.1.0\"\n[dependencies]\nchild = { loaf = \"child\", path = \"../child\" }\n",
        )?;
        fs::write(
            parent.join("src/lib.incn"),
            "from pub::child import answer\n\npub def outer_answer() -> int:\n    \"\"\"Return the dependency's answer.\"\"\"\n    return answer()\n",
        )?;
        bake_oven_project_targets(&child, &FeatureSelection::default(), None)?;
        let artifact = child.join("target/lib");
        let expected = incan_frontend::library_manifest::digest_provider_artifact(&artifact)?;
        let authority = digest_baked_project_source_authority(&parent)?;
        let receipts = library_publication_receipts(&child)?;
        let pointers = receipts.iter().map(|path| fs::read(path).ok()).collect::<Vec<_>>();
        for _ in 0..2 {
            let exports = checked_library_exports(&parent.join("src/lib.incn"), &FeatureSelection::default())?;
            assert!(exports.iter().any(|export| export.name == "outer_answer"));
            assert_eq!(
                expected,
                incan_frontend::library_manifest::digest_provider_artifact(&artifact)?
            );
            assert_eq!(authority, digest_baked_project_source_authority(&parent)?);
            assert_eq!(
                pointers,
                receipts.iter().map(|path| fs::read(path).ok()).collect::<Vec<_>>()
            );
        }
        Ok(())
    }

    /// Cohort replacement updates both the extern binding and its reuse evidence, preserving unrelated inputs.
    #[test]
    fn body_ir_host_cohort_replaces_artifact_and_reuse_evidence() -> Result<(), Box<dyn std::error::Error>> {
        use oven_rustc::rustc::OvenRustcArtifactPlan;

        let root = tempfile::tempdir()?;
        let old = root.path().join("old.rlib");
        let selected = root.path().join("host.rlib");
        fs::write(&old, b"previous core")?;
        fs::write(&selected, b"selected host core")?;
        let mut plan = OvenRustcArtifactPlan {
            source_path_projection: None,
            dependency_search_paths: Vec::new(),
            native_search_paths: Vec::new(),
            externs: vec![("incan_semantics_core".into(), old)],
            compile_environment: BTreeMap::new(),
            caller_owned_library_digests: BTreeMap::from([
                ("incan_semantics_core".into(), "previous digest".into()),
                ("retained".into(), "retained digest".into()),
            ]),
        };
        let mut host = plan.clone();
        host.externs = vec![("incan_semantics_core".into(), selected.clone())];
        host.dependency_search_paths = vec![root.path().join("host-dependencies")];
        let digest = apply_body_ir_host_cohort(&mut plan, &host)?;
        assert_eq!(digest, oven_store::digest_bytes(b"selected host core"));
        assert_eq!(plan.externs, vec![("incan_semantics_core".into(), selected.clone())]);
        assert_eq!(
            plan.caller_owned_library_digests.get("incan_semantics_core"),
            Some(&digest)
        );
        assert_eq!(
            plan.caller_owned_library_digests.get("retained").map(String::as_str),
            Some("retained digest")
        );
        assert!(
            plan.dependency_search_paths
                .contains(&root.path().join("host-dependencies"))
        );

        let cohort = plan.clone();
        replace_rust_library_bindings(
            &mut plan,
            &[OvenCallerOwnedRustcLibrary {
                crate_name: "incan_semantics_core".into(),
                output: selected.clone(),
                digest,
                expose_extern: true,
            }],
        )?;
        assert_eq!(
            plan, cohort,
            "final unit composition preserves the selected host cohort"
        );

        let unchanged = plan.clone();
        host.externs.push(("incan_semantics_core".into(), selected));
        assert!(apply_body_ir_host_cohort(&mut plan, &host).is_err());
        assert_eq!(
            plan, unchanged,
            "ambiguous host artifacts refuse before changing the caller"
        );
        Ok(())
    }

    /// A `StableCrateId` collision -- rustc's report when two compiled instances of one crate meet in a link, which
    /// carries no error code -- is classified as a composition failure exactly like the coded crate-loading errors,
    /// whether it arrives as a structured diagnostic or as unstructured text; a type error in the generated Rust is
    /// not.
    #[test]
    fn a_stable_crate_id_collision_is_a_composition_failure_not_a_source_fault() {
        let collision = "found crates (`serde_derive` and `serde_derive`) with colliding StableCrateId values";
        let structured = OvenRustcError::CompilationFailed {
            report: oven_rustc::rustc::OvenRustcDiagnosticReport {
                diagnostics: vec![oven_rustc::rustc::OvenRustcDiagnostic {
                    level: "error".to_string(),
                    message: collision.to_string(),
                    code: None,
                    spans: Vec::new(),
                    rendered: None,
                }],
                unstructured_output: String::new(),
                invocation: None,
            },
        };
        assert!(direct_rustc_composition_failure(&structured));
        let unstructured = OvenRustcError::CompilationFailed {
            report: oven_rustc::rustc::OvenRustcDiagnosticReport {
                diagnostics: Vec::new(),
                unstructured_output: format!("error: {collision}\n"),
                invocation: None,
            },
        };
        assert!(direct_rustc_composition_failure(&unstructured));
        let type_error = OvenRustcError::CompilationFailed {
            report: oven_rustc::rustc::OvenRustcDiagnosticReport {
                diagnostics: vec![oven_rustc::rustc::OvenRustcDiagnostic {
                    level: "error".to_string(),
                    message: "mismatched types".to_string(),
                    code: Some("E0308".to_string()),
                    spans: Vec::new(),
                    rendered: None,
                }],
                unstructured_output: String::new(),
                invocation: None,
            },
        };
        assert!(!direct_rustc_composition_failure(&type_error));
        let refusal = classify_direct_rustc_bake("app", Err(structured))
            .err()
            .map(|error| error.to_string())
            .unwrap_or_default();
        assert!(refusal.contains("Oven refuses to build `app`"), "{refusal}");
        assert!(refusal.contains("semantically different compiled units"), "{refusal}");
        assert!(refusal.contains("colliding StableCrateId"), "{refusal}");
    }

    #[test]
    fn oven_bake_discovers_an_initialized_executable_project() -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        fs::create_dir_all(project.path().join("src"))?;
        fs::write(project.path().join("loaf.toml"), "[project]\nname = \"app\"\n")?;
        fs::write(project.path().join("src/main.incn"), "def main() -> None:\n    pass\n")?;

        let targets = discover_oven_bake_project_targets(project.path())?;

        assert_eq!(
            targets,
            vec![(OvenBakeProjectTarget::Executable, project.path().join("src/main.incn"))]
        );
        Ok(())
    }

    #[test]
    fn oven_bake_discovers_library_and_executable_targets_in_stable_order() -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        fs::create_dir_all(project.path().join("src"))?;
        fs::write(project.path().join("loaf.toml"), "[project]\nname = \"mixed\"\n")?;
        fs::write(
            project.path().join("src/lib.incn"),
            "pub def value() -> int:\n    return 1\n",
        )?;
        fs::write(project.path().join("src/main.incn"), "def main() -> None:\n    pass\n")?;

        let targets = discover_oven_bake_project_targets(project.path())?;

        assert_eq!(
            targets,
            vec![
                (OvenBakeProjectTarget::Library, project.path().join("src/lib.incn")),
                (OvenBakeProjectTarget::Executable, project.path().join("src/main.incn")),
            ]
        );
        assert_eq!(
            oven_bake_dependency_surface_entrypoint(&targets),
            Some(project.path().join("src/main.incn").as_path()),
            "a mixed project must collect dependencies reachable only from its executable root",
        );
        Ok(())
    }

    #[test]
    fn oven_bake_discovers_every_distinct_declared_script_with_non_colliding_lineage()
    -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        fs::create_dir_all(project.path().join("src"))?;
        fs::write(
            project.path().join("loaf.toml"),
            "[project]\nname = \"scripts\"\n\n[project.scripts]\nmain = \"src/main.incn\"\nextra = \"src/extra.incn\"\nextra_alias = \"src/extra.incn\"\n",
        )?;
        let main = project.path().join("src/main.incn");
        let extra = project.path().join("src/extra.incn");
        fs::write(&main, "def main() -> None:\n    pass\n")?;
        fs::write(&extra, "def main() -> None:\n    pass\n")?;

        let targets = discover_oven_bake_project_targets(project.path())?;

        assert_eq!(
            targets,
            vec![
                (OvenBakeProjectTarget::Executable, extra.clone()),
                (OvenBakeProjectTarget::Executable, main.clone()),
            ],
            "script aliases must not compile the same authored entrypoint twice"
        );
        assert_eq!(
            oven_bake_project_target_identity(project.path(), OvenBakeProjectTarget::Executable, &main)?,
            "executable"
        );
        assert_eq!(
            oven_bake_project_target_identity(project.path(), OvenBakeProjectTarget::Executable, &extra)?,
            "executable:src/extra.incn"
        );
        let main_receipt =
            project_bake_receipt_path(project.path(), OvenBakeProjectTarget::Executable, &main, "debug")?;
        let extra_receipt =
            project_bake_receipt_path(project.path(), OvenBakeProjectTarget::Executable, &extra, "debug")?;
        assert!(main_receipt.ends_with("executable-debug-receipt.json"));
        assert_ne!(main_receipt, extra_receipt);
        assert_ne!(
            oven_executable_entrypoint_evidence_key(project.path(), &main)?,
            oven_executable_entrypoint_evidence_key(project.path(), &extra)?,
        );
        assert_eq!(oven_bake_executable_output_dir(project.path(), &main)?, None);
        let extra_output = oven_bake_executable_output_dir(project.path(), &extra)?
            .ok_or("custom script did not receive an isolated output root")?;
        assert!(extra_output.starts_with(project.path().join("target/incan/oven-targets")));
        Ok(())
    }

    #[test]
    fn oven_bake_refuses_a_manifest_without_a_conventional_target() -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        fs::write(project.path().join("loaf.toml"), "[project]\nname = \"empty\"\n")?;

        let result = discover_oven_bake_project_targets(project.path());
        let Err(error) = result else {
            return Err("a manifest without src/lib.incn or src/main.incn must not be bakeable".into());
        };
        assert!(error.to_string().contains("src/lib.incn"));
        assert!(error.to_string().contains("src/main.incn"));
        Ok(())
    }
}
