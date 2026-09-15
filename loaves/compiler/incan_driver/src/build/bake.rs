//! Baking a project's targets: the executable and library bakes, their classification, and the explicit `incan oven
//! bake --project` entry.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::{fs, io};

use crate::backend::selection::BackendExecutionReceipt;
use crate::build::caller_owned::has_caller_owned_project_libraries;
use crate::build::library_exports::resolve_library_project_root;
use crate::build::library_outputs::{
    library_publication_receipts, packaged_library_loaf_store_root, write_library_manifest_artifacts,
};
use crate::build::library_project::prepare_library_project;
use crate::build::output_materialization::{
    completed_output_default_backend_receipt, select_default_project_output,
    warn_for_completed_output_lock_fingerprint_drift,
};
use crate::build::output_paths::{
    library_project_output_sidecars, normalized_project_entrypoint, oven_binary_path, packaged_library_metadata_files,
    project_output_bake_files, project_root_for_completed_output, validated_project_output_relative_path,
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
    project_output_payload_for_bake, publish_project_inspection_authority, publish_project_output_loaf,
};
use crate::build::reuse::try_reuse_baked_project;
use crate::build::source_authority::{
    baked_project_lock_dependencies_fingerprint, canonical_baked_project_lock_path, project_bake_receipt_path,
};
use crate::build::{
    BackendSelectionOptions, BuildCommandOptions, CompletedOutputPolicy, LibraryInspectionConstituent,
    OVEN_PACKAGED_LIBRARY_LOAF_SCHEMA_VERSION, OvenBakeProjectTarget, OvenPackagedLibraryLoafManifest,
    OvenPackagedLibraryLoafProfile, OvenPreparedLibrary, OvenPreparedProject, OvenProjectBakeAuthorityContext,
    OvenProjectBakeProfileReport, OvenProjectBakeReport, OvenProjectOutputBakeRequest, OvenProjectPlanMode,
    OvenStoredProjectOutput, PendingOvenProjectOutput, PreparedLibraryProject, library_publication,
    oven_bake_executable_output_dir, oven_bake_project_target_identity,
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
use incan_core::version::INCAN_VERSION;
use incan_frontend::library_manifest::published_layout::packaged_library_loaf_manifest_path;
use incan_provider::FeatureSelection;
use oven_model::manifest::ProjectManifest;
use oven_rustc::legacy_cargo::direct_rustc_compile_environment;
use oven_rustc::plan::OvenDirectRustcPlanSelection;
use oven_rustc::rustc::{
    OvenRustcError, OvenTrustedDirectRustcTargetRequest, attach_caller_owned_rustc_libraries,
    bake_trusted_direct_rustc_library, bake_trusted_direct_rustc_run,
};
use oven_store::store::OvenArtifactKind;
use oven_store::write_receipt;

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
        )?;
        // The conflict decision must cover every selection path -- including an imported packaged-provider closure,
        // whose composed link carries the SDK base's and the provider's own copies of any shared package exactly
        // like a re-materialized one does.
        if let Some((package, pinned_by)) = caller_owned_provider_registry_conflict(
            registry_authority.as_ref(),
            &closure,
            prepared.plan_selection.artifact_plan(),
        )? {
            return Err(oven_native_closure_refusal(
                &prepared.crate_name,
                &provider_registry_conflict_reason(&package, pinned_by.as_deref()),
            ));
        }
        if !prepared.plan_selection.uses_packaged_provider_closure() {
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
            if let Some((package, pinned_by)) = caller_owned_provider_registry_conflict(
                registry_authority.as_ref(),
                &closure,
                selected.plan_selection.artifact_plan(),
            )? {
                return Err(oven_native_closure_refusal(
                    &oven.crate_name,
                    &provider_registry_conflict_reason(&package, pinned_by.as_deref()),
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
    let direct = bake_trusted_direct_rustc_library(&OvenTrustedDirectRustcTargetRequest {
        receipt: &selected.receipt,
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

    classify_direct_rustc_bake(&oven.crate_name, direct)
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
    if output_dir.is_some() || !options.backend.allows_completed_output_reuse() {
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

/// Explicitly prepare compatible Oven closures for every manifest-backed target in one Incan project.
///
/// This is preparation rather than execution: it records fresh source/lock/SDK/provider receipt evidence, reuses a
/// matching stored or release-scoped stdlib closure when available, and otherwise crosses Oven's explicit bounded
/// publisher exactly once per genuinely missing target/profile. Normal `build`, `run`, and `test` remain Cargo-free
/// consumers of the resulting direct-rustc plans.
pub fn bake_oven_project_targets(
    project: &Path,
    package_features: &FeatureSelection,
) -> CliResult<OvenProjectBakeReport> {
    let project = project
        .to_str()
        .ok_or_else(|| CliError::failure(format!("Oven project path is not valid UTF-8: {}", project.display())))?;
    let project_root = resolve_library_project_root(Some(project))?;
    let targets = discover_oven_bake_project_targets(&project_root)?;
    let dependency_surface_entrypoint = oven_bake_dependency_surface_entrypoint(&targets)
        .ok_or_else(|| CliError::failure("explicit Oven project bake discovered no dependency-surface entrypoint"))?
        .to_path_buf();
    let store = open_default_oven_store()?;
    let mut authority_context = OvenProjectBakeAuthorityContext::default();
    if canonical_baked_project_lock_path(&project_root)?.is_file()
        && let Some(reused) = try_reuse_baked_project(
            &project_root,
            &targets,
            &store,
            package_features,
            &mut authority_context,
        )?
    {
        return Ok(reused);
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
                        &BackendSelectionOptions::default(),
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
                            &BackendSelectionOptions::default(),
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
        let inspection_authority = publish_project_inspection_authority(
            &store,
            &project_root,
            &source_authority_digest,
            &registry_dependencies,
            &dev_registry_dependencies,
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

    use crate::build::source_authority::project_bake_receipt_path;
    use crate::build::{
        OvenBakeProjectTarget, oven_bake_executable_output_dir, oven_bake_project_target_identity,
        oven_executable_entrypoint_evidence_key,
    };

    use oven_rustc::rustc::OvenRustcError;

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
        assert!(refusal.contains("#1241"), "{refusal}");
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
