//! Build and run pipeline for Incan projects.
//!
//! This module handles the full compilation flow: module collection, type checking, codegen configuration, dependency
//! resolution, project generation, and receipt-bound direct-`rustc` Oven execution.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;
use std::{env, fs};

use crate::commands::build_report::{emit_build_report, emit_rust_inspection_report, emit_workspace_build_report};
use crate::{CliError, CliResult, ExitCode};
use incan_driver::backend::selection::{BackendKind, resolve_execution, select_backend};
use incan_driver::build::backend_selection::{default_backend_receipt_path, write_backend_receipt};
use incan_driver::build::bake::{bake_oven_library, bake_oven_project, select_default_executable_project_output};
use incan_driver::build::inline_command::{inline_command_project, wrap_inline_command_source};
use incan_driver::build::library_exports::resolve_library_project_root;
use incan_driver::build::library_outputs::{
    library_output_path, library_publication_receipts, write_library_manifest_artifacts,
};
use incan_driver::build::library_project::prepare_library_project;
use incan_driver::build::output_materialization::{
    completed_executable_output_report, completed_library_output_report, completed_output_default_backend_receipt,
    materialize_completed_executable_output, materialize_completed_library_outputs, materialize_project_output,
    select_default_library_project_outputs, select_default_project_output, verify_stored_project_output_native,
    warn_for_completed_output_lock_fingerprint_drift,
};
use incan_driver::build::output_paths::{normalized_project_entrypoint, project_root_for_completed_output};
use incan_driver::build::oven_project::prepare_oven_project;
use incan_driver::build::plan_authority::explicit_bake_profiles;
use incan_driver::build::replacement::{build_replacement_file_report, reject_normal_cargo_controls};
use incan_driver::build::rust_extern::{RustExternBuildFailureKind, RustExternDeclContext};
use incan_driver::build::{
    BackendSelectionOptions, BuildCommandOptions, CompletedOutputPolicy, OvenBakeProjectTarget, OvenPreparedProject,
    OvenProjectPlanMode, elapsed_ms, library_publication,
};
use incan_driver::build_report::{
    BuildReportMode, BuildReportOptions, RustInspectionFormat, artifact_report, rust_inspection_report,
};
use incan_driver::cargo_policy::CargoPolicy;
use incan_driver::project::resolve_project_root;
use incan_frontend::diagnostics;
use incan_provider::FeatureSelection;
use incan_provider::requirements::INTERNAL_LIBRARY_ARTIFACT_ONLY_ENV;
use oven_rustc::rustc::clear_inherited_cargo_environment;

/// Pre-flight refusal check for a declared `--backend` request (#986).
///
/// Must run before any "reuse a sealed cache-hit Loaf" shortcut in `build_file`,
/// `build_file_report`, `build_library`, and `build_library_report`: those shortcuts return
/// success without ever calling `prepare_oven_project`/`prepare_library_project`, which is where
/// backend selection normally runs, so a refused request (for example `--backend replacement`
/// with no working fallback) would otherwise be silently masked by reusing a previously sealed
/// artifact instead of failing visibly.
///
/// Refusal depends only on the requested backend and its fallback policy, never on source
/// content, so this uses a placeholder source identity rather than loading and hashing the
/// project's modules just to decide whether to proceed — the real, source-identified selection is
/// still built fresh inside `prepare_oven_project`/`prepare_library_project` whenever a build
/// actually reaches them.
fn ensure_backend_request_available(backend_options: &BackendSelectionOptions) -> CliResult<()> {
    let selection = select_backend(
        backend_options.requested,
        backend_options.explicit,
        backend_options.shadow,
        "",
        backend_options.fallback_policy,
    );
    resolve_execution(&selection, selection.selected_backend.is_implemented())
        .map_err(|error| CliError::failure(error.to_string()))?;
    Ok(())
}

/// Print human build progress to stderr when stdout is reserved for a machine-readable report.
fn print_build_progress(report_options: &BuildReportOptions, message: impl AsRef<str>) {
    if report_options.enabled() {
        eprintln!("{}", message.as_ref());
    } else {
        println!("{}", message.as_ref());
    }
}

#[allow(dead_code)]
/// Classify a supported Rust compiler failure that mentions the declared item or its backing module.
fn classify_rust_extern_build_failure(
    stderr: &str,
    item_name: &str,
    rust_module_path: &str,
) -> Option<RustExternBuildFailureKind> {
    if !stderr.contains(item_name) && !stderr.contains(rust_module_path) {
        return None;
    }
    if stderr.contains("gated behind the")
        || stderr.contains("configured out")
        || stderr.contains("the item is gated behind")
    {
        return Some(RustExternBuildFailureKind::FeatureGatedBackingPath);
    }
    if stderr.contains("mismatched types") || stderr.contains("error[E0308]") {
        return Some(RustExternBuildFailureKind::SignatureMismatch);
    }
    if stderr.contains("cannot find")
        || stderr.contains("failed to resolve")
        || stderr.contains("unresolved import")
        || stderr.contains("error[E0425]")
    {
        return Some(RustExternBuildFailureKind::UnresolvedBackingItem);
    }
    None
}

#[allow(dead_code)]
/// Render deduplicated Incan diagnostics for recognized Rust extern build failures.
fn format_rust_extern_wrapped_diagnostics(stderr: &str, contexts: &[RustExternDeclContext]) -> Option<String> {
    let mut rendered = String::new();
    let mut seen: HashSet<String> = HashSet::new();
    for ctx in contexts {
        let Some(kind) = classify_rust_extern_build_failure(stderr, &ctx.item_name, &ctx.rust_module_path) else {
            continue;
        };
        let key = format!(
            "{}:{}:{}:{}",
            ctx.file_path.display(),
            ctx.item_name,
            ctx.span.start,
            ctx.span.end
        );
        if !seen.insert(key) {
            continue;
        }
        let err = match kind {
            RustExternBuildFailureKind::UnresolvedBackingItem => {
                diagnostics::errors::rust_extern_unresolved_backing_item(
                    &ctx.item_name,
                    &ctx.rust_module_path,
                    ctx.span,
                )
            }
            RustExternBuildFailureKind::SignatureMismatch => {
                diagnostics::errors::rust_extern_signature_mismatch(&ctx.item_name, &ctx.rust_module_path, ctx.span)
            }
            RustExternBuildFailureKind::FeatureGatedBackingPath => {
                diagnostics::errors::rust_extern_feature_gated_backing_path(
                    &ctx.item_name,
                    &ctx.rust_module_path,
                    ctx.span,
                )
            }
        };
        rendered.push_str(&diagnostics::format_error(
            ctx.file_path.to_string_lossy().as_ref(),
            &ctx.source,
            &err,
        ));
    }
    if rendered.is_empty() { None } else { Some(rendered) }
}

/// Build an Incan file to a Rust project.
pub fn build_file(
    file_path: &str,
    output_dir: Option<&String>,
    options: BuildCommandOptions,
    report_options: BuildReportOptions,
) -> CliResult<ExitCode> {
    reject_normal_cargo_controls(&options.cargo_policy, options.generated_cargo_target_dir.as_ref())?;
    ensure_backend_request_available(&options.backend)?;
    incan_driver::project::warn_once_about_ignored_cargo_manifest(&resolve_project_root(Path::new(file_path)));
    if options.backend.requested == BackendKind::Replacement {
        let report = build_replacement_file_report(file_path, options, &report_options)?;
        emit_workspace_build_report(&report, &report_options)?;
        return Ok(ExitCode::SUCCESS);
    }
    if !report_options.enabled()
        && let Some((project_root, selected, backend_receipt)) =
            select_default_executable_project_output(file_path, output_dir, &options)?
    {
        materialize_completed_executable_output(&project_root, &selected, &backend_receipt)?;
        println!(
            "✓ Oven build reused sealed project Loaf: {}",
            selected.native_output.display()
        );
        return Ok(ExitCode::SUCCESS);
    }
    let report = build_file_report(file_path, output_dir, options, &report_options)?;
    emit_workspace_build_report(&report, &report_options)?;
    Ok(ExitCode::SUCCESS)
}

/// Build one executable project and retain its completed report for workspace-level aggregation.
pub(crate) fn build_file_report(
    file_path: &str,
    output_dir: Option<&String>,
    options: BuildCommandOptions,
    report_options: &BuildReportOptions,
) -> CliResult<serde_json::Value> {
    reject_normal_cargo_controls(&options.cargo_policy, options.generated_cargo_target_dir.as_ref())?;
    ensure_backend_request_available(&options.backend)?;
    if options.backend.requested == BackendKind::Replacement {
        return build_replacement_file_report(file_path, options, report_options);
    }
    let total_start = Instant::now();
    if let Some((project_root, selected, backend_receipt)) =
        select_default_executable_project_output(file_path, output_dir, &options)?
    {
        materialize_completed_executable_output(&project_root, &selected, &backend_receipt)?;
        print_build_progress(
            report_options,
            format!(
                "✓ Oven build reused sealed project Loaf: {}",
                selected.native_output.display()
            ),
        );
        return completed_executable_output_report(&project_root, &selected, &backend_receipt, total_start);
    }
    let prepare_start = Instant::now();
    let prepared = prepare_oven_project(
        file_path,
        output_dir.map(|path| path.as_str()),
        &options.cargo_policy,
        &options.package_features,
        options.sdk_profile.as_deref(),
        options.cargo_features,
        options.cargo_no_default_features,
        options.cargo_all_features,
        "release",
        OvenProjectPlanMode::ConsumeOnly,
        None,
        &options.backend,
    )?;
    let prepare_ms = elapsed_ms(prepare_start);

    print_build_progress(
        report_options,
        format!(
            "Generated Rust project in: {}",
            prepared.generator.output_dir().display()
        ),
    );
    print_build_progress(report_options, "Building with Oven Alpha...");
    let oven_build_start = Instant::now();
    let bake = bake_oven_project(&prepared, "release", None)?;
    let oven_build_ms = elapsed_ms(oven_build_start);
    print_build_progress(report_options, "✓ Oven build successful!");
    print_build_progress(report_options, format!("Binary: {}", bake.output.display()));
    let mut report_draft = prepared.report.clone();
    report_draft.artifacts.push(artifact_report("binary", &bake.output));
    // Published only now that the whole build — codegen, Oven plan selection, and the rustc bake
    // above — has actually succeeded (#986); `prepare_oven_project` itself never persists this,
    // since it also runs for internal/dependency callers that must not overwrite a real receipt.
    if let Some(backend_receipt) = report_draft.backend.as_ref() {
        write_backend_receipt(backend_receipt, &default_backend_receipt_path(&prepared.project_root))?;
    }
    let mut timings_ms = prepared.prepare_timings.clone();
    timings_ms.insert("prepare".to_string(), prepare_ms);
    timings_ms.insert("oven_build".to_string(), oven_build_ms);
    timings_ms.insert("total".to_string(), elapsed_ms(total_start));
    let report = report_draft.finish(timings_ms);
    serde_json::to_value(report)
        .map_err(|error| CliError::failure(format!("failed to serialize Oven build report: {error}")))
}

/// Validate RFC 031 library-mode preconditions.
pub fn build_library(
    file_path: Option<&str>,
    output_dir: Option<&String>,
    options: BuildCommandOptions,
    report_options: BuildReportOptions,
) -> CliResult<ExitCode> {
    ensure_backend_request_available(&options.backend)?;
    if options.backend.requested == BackendKind::Replacement {
        return Err(CliError::failure(
            "replacement backend #988 supports source-only executable free functions, not libraries or package artifacts",
        ));
    }
    let artifact_only = env::var_os(INTERNAL_LIBRARY_ARTIFACT_ONLY_ENV).is_some();
    if !artifact_only {
        // A nested dependency-library build is not the user's project, so it does not repeat the warning for a
        // directory they did not invoke a command in. Without an explicit entry file the library root is the
        // working directory, which is where `incan build --lib` was run.
        let library_root = match file_path {
            Some(path) => resolve_project_root(Path::new(path)),
            None => PathBuf::from("."),
        };
        incan_driver::project::warn_once_about_ignored_cargo_manifest(&library_root);
    }
    if !artifact_only {
        reject_normal_cargo_controls(&options.cargo_policy, options.generated_cargo_target_dir.as_ref())?;
        let completed_output_policy = CompletedOutputPolicy {
            cargo_policy: &options.cargo_policy,
            package_features: &options.package_features,
            sdk_profile: options.sdk_profile.as_deref(),
            cargo_features: &options.cargo_features,
            cargo_no_default_features: options.cargo_no_default_features,
            cargo_all_features: options.cargo_all_features,
        };
        if output_dir.is_none()
            && !report_options.enabled()
            && let Some(outputs) =
                select_default_library_project_outputs(file_path, &completed_output_policy, &options.backend)?
        {
            let project_root = resolve_library_project_root(file_path)?;
            warn_for_completed_output_lock_fingerprint_drift(&project_root, outputs.iter())?;
            // Prefer the release output's backend receipt, but accept the profile this project actually baked.
            // A bake narrowed by `explicit_bake_profiles` has no release output to find, and the default backend
            // receipt records the selected plan rather than anything profile-specific.
            let backend_receipt = outputs
                .iter()
                .find(|output| output.profile == "release")
                .or_else(|| outputs.first())
                .and_then(completed_output_default_backend_receipt)
                .ok_or_else(|| CliError::failure("completed Oven library output has no verified backend receipt"))?;
            for selected in outputs {
                materialize_project_output(&project_root, &selected)?;
                println!(
                    "✓ Oven library build reused sealed project Loaf: {}",
                    selected.native_output.display()
                );
            }
            write_backend_receipt(&backend_receipt, &default_backend_receipt_path(&project_root))?;
            return Ok(ExitCode::SUCCESS);
        }
    }
    let report = build_library_report(file_path, output_dir, options, &report_options)?;
    emit_build_report(&report, &report_options)?;
    Ok(ExitCode::SUCCESS)
}

/// Build one library project and retain its completed report for workspace-level aggregation.
pub(crate) fn build_library_report(
    file_path: Option<&str>,
    output_dir: Option<&String>,
    options: BuildCommandOptions,
    report_options: &BuildReportOptions,
) -> CliResult<incan_driver::build_report::BuildReport> {
    ensure_backend_request_available(&options.backend)?;
    if options.backend.requested == BackendKind::Replacement {
        return Err(CliError::failure(
            "replacement backend #988 supports source-only executable free functions, not libraries or package artifacts",
        ));
    }
    let total_start = Instant::now();
    let artifact_only = env::var_os(INTERNAL_LIBRARY_ARTIFACT_ONLY_ENV).is_some();
    if !artifact_only {
        reject_normal_cargo_controls(&options.cargo_policy, options.generated_cargo_target_dir.as_ref())?;
        let completed_output_policy = CompletedOutputPolicy {
            cargo_policy: &options.cargo_policy,
            package_features: &options.package_features,
            sdk_profile: options.sdk_profile.as_deref(),
            cargo_features: &options.cargo_features,
            cargo_no_default_features: options.cargo_no_default_features,
            cargo_all_features: options.cargo_all_features,
        };
        if output_dir.is_none()
            && let Some(outputs) =
                select_default_library_project_outputs(file_path, &completed_output_policy, &options.backend)?
        {
            let project_root = resolve_library_project_root(file_path)?;
            warn_for_completed_output_lock_fingerprint_drift(&project_root, outputs.iter())?;
            // Prefer the release output's backend receipt, but accept the profile this project actually baked.
            // A bake narrowed by `explicit_bake_profiles` has no release output to find, and the default backend
            // receipt records the selected plan rather than anything profile-specific.
            let backend_receipt = outputs
                .iter()
                .find(|output| output.profile == "release")
                .or_else(|| outputs.first())
                .and_then(completed_output_default_backend_receipt)
                .ok_or_else(|| CliError::failure("completed Oven library output has no verified backend receipt"))?;
            return materialize_completed_library_outputs(&project_root, &outputs, &backend_receipt, || {
                completed_library_output_report(&project_root, &outputs, total_start)
            });
        }
    }
    let project_root = resolve_library_project_root(file_path)?;
    let publication = library_publication::LibraryPublication::begin(
        &project_root,
        &library_output_path(&project_root, output_dir.map(String::as_str))?,
        library_publication_receipts(&project_root)?,
    )?;
    let result = (|| {
        let generated_cargo_target_dir = options.effective_generated_cargo_target_dir();
        let mut prepared = prepare_library_project(
            file_path,
            output_dir.map(String::as_str),
            options.cargo_policy,
            &options.package_features,
            options.sdk_profile.as_deref(),
            options.cargo_features,
            options.cargo_no_default_features,
            options.cargo_all_features,
            generated_cargo_target_dir.as_deref(),
            !artifact_only,
            !artifact_only,
            OvenProjectPlanMode::ConsumeOnly,
            None,
            &options.backend,
        )?;

        if artifact_only {
            write_library_manifest_artifacts(&mut prepared)?;
            print_build_progress(report_options, "✓ Library dependency artifact prepared!");
            print_build_progress(
                report_options,
                format!("Generated manifest: {}", prepared.manifest_path.display()),
            );
            let mut timings_ms = prepared.timings_ms.clone();
            timings_ms.insert("total".to_string(), elapsed_ms(total_start));
            let report = prepared.report.finish(timings_ms);
            return Ok(report);
        }

        if prepared.oven.is_some() {
            let oven_build_start = Instant::now();
            let oven = prepared.oven.as_ref().ok_or_else(|| {
                CliError::failure("normal Oven library build lost its prepared direct-rustc selection")
            })?;
            let mut bakes = Vec::new();
            for profile in explicit_bake_profiles() {
                bakes.push((profile, bake_oven_library(&prepared, oven, profile, None)?));
            }
            // The manifest selects this build's immutable semantic content only after every native profile succeeds.
            write_library_manifest_artifacts(&mut prepared)?;
            let oven_build_ms = elapsed_ms(oven_build_start);
            print_build_progress(report_options, "✓ Oven library build successful!");
            for (profile, bake) in &bakes {
                print_build_progress(report_options, format!("{profile} library: {}", bake.output.display()));
            }
            print_build_progress(
                report_options,
                format!("Generated manifest: {}", prepared.manifest_path.display()),
            );
            let mut report_draft = prepared.report.clone();
            for (profile, bake) in &bakes {
                report_draft
                    .artifacts
                    .push(artifact_report(format!("rust_library_{profile}"), &bake.output));
            }
            // Published only now that the whole build — codegen, Oven plan selection, and both
            // debug/release rustc bakes above — has actually succeeded (#986); `prepare_library_project`
            // itself never persists this (see the matching comment there).
            if let Some(backend_receipt) = report_draft.backend.as_ref() {
                write_backend_receipt(backend_receipt, &default_backend_receipt_path(&prepared.project_root))?;
            }
            let mut timings_ms = prepared.timings_ms.clone();
            timings_ms.insert("oven_build".to_string(), oven_build_ms);
            timings_ms.insert("total".to_string(), elapsed_ms(total_start));
            return Ok(report_draft.finish(timings_ms));
        }

        Err(CliError::failure(
            "normal `incan build --lib` requires a prepared Oven direct-rustc selection; Cargo library execution is not an available fallback",
        ))
    })();
    publication.finish(result)
}

/// Output format for `incan inspect backend-selection`.
#[derive(clap::ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendSelectionInspectFormat {
    /// Human-readable summary of the receipt's key fields.
    Text,
    /// The complete receipt, pretty-printed.
    Json,
}

/// Read, verify, and render one persisted backend-selection execution receipt (#986).
///
/// Mirrors `inspect_oven_receipt`'s read-verify-render shape, but has no bounded store to
/// consult: a backend-selection receipt is self-contained, so verification is just
/// `BackendExecutionReceipt::verify_identity`.
pub fn inspect_backend_selection(path: &Path, format: BackendSelectionInspectFormat) -> CliResult<ExitCode> {
    let bytes = fs::read(path).map_err(|error| {
        CliError::failure(format!(
            "failed to read backend-selection receipt {}: {error}",
            path.display()
        ))
    })?;
    let receipt = serde_json::from_slice::<incan_driver::backend::selection::BackendExecutionReceipt>(&bytes).map_err(
        |error| {
            CliError::failure(format!(
                "failed to parse backend-selection receipt {}: {error}",
                path.display()
            ))
        },
    )?;
    receipt
        .verify_identity()
        .map_err(|error| CliError::failure(error.to_string()))?;
    match format {
        BackendSelectionInspectFormat::Text => {
            println!("selected backend:   {:?}", receipt.selection.selected_backend);
            println!("executed backend:   {:?}", receipt.executed_backend);
            println!("selection reason:   {:?}", receipt.selection.selection_reason);
            println!("fallback policy:    {:?}", receipt.selection.fallback_policy);
            println!("fallback outcome:   {:?}", receipt.fallback_outcome);
            println!("shadow comparison:  {:?}", receipt.shadow_comparison);
            println!("compiler version:   {}", receipt.compiler_version);
            println!("selection identity: {}", receipt.selection.identity);
            println!("receipt identity:   {}", receipt.identity);
        }
        BackendSelectionInspectFormat::Json => {
            let json = serde_json::to_string_pretty(&receipt).map_err(|error| {
                CliError::failure(format!("failed to serialize backend-selection receipt: {error}"))
            })?;
            println!("{json}");
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// Generate and inspect the same Oven Alpha Rust projection used by normal commands without running its binary.
pub fn inspect_rust(path: &Path, lib_mode: bool, format: RustInspectionFormat) -> CliResult<ExitCode> {
    let path_arg = path.to_string_lossy();
    let report = if lib_mode {
        let prepared = prepare_library_project(
            Some(path_arg.as_ref()),
            None,
            CargoPolicy::default(),
            &FeatureSelection::default(),
            None,
            Vec::new(),
            false,
            false,
            None,
            true,
            false,
            OvenProjectPlanMode::ConsumeOnly,
            None,
            &BackendSelectionOptions::default(),
        )?;
        rust_inspection_report(
            BuildReportMode::Library,
            prepared.report.generated,
            prepared.report.source_files,
            prepared.report.notes,
        )?
    } else {
        let prepared = prepare_oven_project(
            path_arg.as_ref(),
            None,
            &CargoPolicy::default(),
            &FeatureSelection::default(),
            None,
            Vec::new(),
            false,
            false,
            "release",
            OvenProjectPlanMode::ConsumeOnly,
            None,
            &BackendSelectionOptions::default(),
        )?;
        rust_inspection_report(
            BuildReportMode::Executable,
            prepared.report.generated,
            prepared.report.source_files,
            prepared.report.notes,
        )?
    };
    emit_rust_inspection_report(&report, format)?;
    Ok(ExitCode::SUCCESS)
}

/// Build and run an Incan file.
#[allow(clippy::too_many_arguments)] // Public CLI dispatch keeps the parsed command axes explicit at this boundary.
pub fn run_file(
    file_path: &str,
    cargo_policy: CargoPolicy,
    package_features: FeatureSelection,
    sdk_profile: Option<String>,
    cargo_features: Vec<String>,
    cargo_no_default_features: bool,
    cargo_all_features: bool,
    release: bool,
) -> CliResult<ExitCode> {
    reject_normal_cargo_controls(&cargo_policy, None)?;
    incan_driver::project::warn_once_about_ignored_cargo_manifest(&resolve_project_root(Path::new(file_path)));
    let profile = if release { "release" } else { "debug" };
    let completed_output_policy = CompletedOutputPolicy {
        cargo_policy: &cargo_policy,
        package_features: &package_features,
        sdk_profile: sdk_profile.as_deref(),
        cargo_features: &cargo_features,
        cargo_no_default_features,
        cargo_all_features,
    };
    if let Some(selected) = select_default_project_output(
        file_path,
        &completed_output_policy,
        OvenBakeProjectTarget::Executable,
        profile,
    )? {
        let project_root =
            project_root_for_completed_output(&normalized_project_entrypoint(file_path)?)?.ok_or_else(|| {
                CliError::failure("selected Oven project-output Loaf has no manifest-backed project root")
            })?;
        warn_for_completed_output_lock_fingerprint_drift(&project_root, [&selected])?;
        verify_stored_project_output_native(&selected)?;
        let mut command = Command::new(&selected.native_output);
        command.current_dir(project_root);
        clear_inherited_cargo_environment(&mut command);
        let status = command.status().map_err(|error| {
            CliError::failure(format!(
                "failed to run selected Oven project-output Loaf {}: {error}",
                selected.native_output.display()
            ))
        })?;
        return Ok(ExitCode(status.code().unwrap_or(ExitCode::FAILURE.0)));
    }
    let prepared = prepare_oven_project(
        file_path,
        None,
        &cargo_policy,
        &package_features,
        sdk_profile.as_deref(),
        cargo_features,
        cargo_no_default_features,
        cargo_all_features,
        profile,
        OvenProjectPlanMode::ConsumeOnly,
        None,
        &BackendSelectionOptions::default(),
    )?;
    run_oven_prepared_project(prepared, profile)
}

/// Build and run inline Incan source from `incan run -c`.
#[allow(clippy::too_many_arguments)] // Inline and file execution intentionally share the explicit CLI contract.
pub fn run_inline_source(
    source: &str,
    cargo_policy: CargoPolicy,
    package_features: FeatureSelection,
    sdk_profile: Option<String>,
    cargo_features: Vec<String>,
    cargo_no_default_features: bool,
    cargo_all_features: bool,
    release: bool,
) -> CliResult<ExitCode> {
    reject_normal_cargo_controls(&cargo_policy, None)?;
    let wrapped_source = wrap_inline_command_source(source);
    let inline_project = inline_command_project(&wrapped_source)?;
    let source_path = inline_project.source_path;
    let source_parent = source_path.parent().ok_or_else(|| {
        CliError::failure(format!(
            "failed to determine temporary inline command directory for {}",
            source_path.display()
        ))
    })?;
    fs::create_dir_all(source_parent).map_err(|err| {
        CliError::failure(format!(
            "Error creating temporary inline command directory {}: {err}",
            source_parent.display()
        ))
    })?;
    let _inline_command_lock = oven_model::lock::acquire_publication_lock(&source_path).map_err(|error| {
        CliError::failure(format!(
            "failed to coordinate temporary inline command project {}: {error}",
            source_parent.display()
        ))
    })?;
    fs::write(&source_path, wrapped_source).map_err(|err| {
        CliError::failure(format!(
            "Error writing temporary inline command file {}: {err}",
            source_path.display()
        ))
    })?;

    let source_arg = source_path.to_string_lossy().to_string();
    let result = prepare_oven_project(
        &source_arg,
        Some(inline_project.output_dir.as_str()),
        &cargo_policy,
        &package_features,
        sdk_profile.as_deref(),
        cargo_features,
        cargo_no_default_features,
        cargo_all_features,
        if release { "release" } else { "debug" },
        OvenProjectPlanMode::ConsumeOnly,
        None,
        &BackendSelectionOptions::default(),
    )
    .and_then(|prepared| run_oven_prepared_project(prepared, if release { "release" } else { "debug" }));
    let _ = fs::remove_file(&source_path);
    result
}

/// Run a receipt-selected native Oven executable while retaining its entry lease for the full process lifetime.
fn run_oven_prepared_project(prepared: OvenPreparedProject, profile: &str) -> CliResult<ExitCode> {
    let bake = bake_oven_project(&prepared, profile, None)?;
    let mut command = Command::new(&bake.output);
    command.current_dir(&prepared.project_root);
    clear_inherited_cargo_environment(&mut command);
    let status = command
        .status()
        .map_err(|error| CliError::failure(format!("failed to run Oven binary {}: {error}", bake.output.display())))?;
    Ok(ExitCode(status.code().unwrap_or(ExitCode::FAILURE.0)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use incan_driver::build::test_support::undesugared_vocab_declaration;
    use std::fs;
    use std::path::{Path, PathBuf};

    use incan_driver::backend::selection::{BackendKind, FallbackPolicy};
    use incan_driver::build::library_exports::collect_library_rust_abi;
    use incan_driver::build::library_outputs::write_library_manifest_artifacts;
    use incan_driver::build::library_project::prepare_library_project;
    use incan_driver::build::rust_extern::{RustExternBuildFailureKind, RustExternDeclContext};
    use incan_driver::build::{BackendSelectionOptions, BuildCommandOptions, OvenProjectPlanMode};
    use incan_driver::build_report::BuildReportOptions;
    use incan_driver::cargo_policy::CargoPolicy;
    use incan_driver::error::{CliError, ExitCode};
    use incan_frontend::api_metadata::ApiDeclaration;
    use incan_frontend::ast::{Declaration, Span};
    use incan_frontend::{body_ir, lexer, parser};

    use incan_frontend::library_manifest::LibraryManifest;
    use incan_provider::FeatureSelection;
    use oven_model::lock::{CargoFeatureSelection, IncanLock, compute_deps_fingerprint};
    #[cfg(feature = "rust_inspect")]
    use rust_inspect::Inspector;
    #[cfg(feature = "rust_inspect")]
    use rust_inspect::InspectorConfig;

    #[test]
    fn completed_output_reuse_requires_the_implicit_default_backend_selection() {
        let default = BackendSelectionOptions::default();
        assert!(default.allows_completed_output_reuse());
        assert!(ensure_backend_request_available(&default).is_ok());

        let replacement_refusal = BackendSelectionOptions {
            requested: BackendKind::Replacement,
            explicit: true,
            shadow: false,
            fallback_policy: FallbackPolicy::Refuse,
        };
        assert!(!replacement_refusal.allows_completed_output_reuse());
        assert!(
            ensure_backend_request_available(&replacement_refusal).is_ok(),
            "the capability preflight recognizes the partial executor; source-profile support is resolved later through the source-bound selection"
        );

        let declared_fallback = BackendSelectionOptions {
            requested: BackendKind::Replacement,
            explicit: true,
            shadow: false,
            fallback_policy: FallbackPolicy::AllowTo(BackendKind::Legacy),
        };
        assert!(!declared_fallback.allows_completed_output_reuse());
        assert!(ensure_backend_request_available(&declared_fallback).is_ok());
    }

    #[test]
    fn classify_signature_mismatch_for_rust_extern_context() {
        let stderr = "error[E0308]: mismatched types in `incan_std_testing::fail`\n  --> src/main.rs:10:5";
        let kind = classify_rust_extern_build_failure(stderr, "fail", "incan_std_testing");
        assert_eq!(kind, Some(RustExternBuildFailureKind::SignatureMismatch));
    }

    #[test]
    fn classify_unresolved_backing_item_for_rust_extern_context() {
        let stderr = "error[E0425]: cannot find function `fail` in module `incan_std_testing`";
        let kind = classify_rust_extern_build_failure(stderr, "fail", "incan_std_testing");
        assert_eq!(kind, Some(RustExternBuildFailureKind::UnresolvedBackingItem));
    }

    #[test]
    fn wraps_rust_extern_failure_back_to_incan_declaration_span() {
        let stderr = "error[E0425]: cannot find function `fail` in module `incan_std_testing`";
        let contexts = vec![RustExternDeclContext {
            file_path: PathBuf::from("stdlib/testing.incn"),
            source: "rust.module(\"incan_std_testing\")\n@rust.extern\ndef fail(msg: str) -> None:\n  ...\n"
                .to_string(),
            item_name: "fail".to_string(),
            rust_module_path: "incan_std_testing".to_string(),
            span: Span { start: 35, end: 73 },
        }];
        let rendered = format_rust_extern_wrapped_diagnostics(stderr, &contexts);
        let Some(rendered) = rendered else {
            panic!("expected wrapped diagnostic");
        };
        assert!(rendered.contains("Rust backing item"));
        assert!(rendered.contains("incan_std_testing::fail"));
    }

    #[cfg(feature = "rust_inspect")]
    #[test]
    fn library_rust_abi_is_independent_of_partial_prewarm_cache_issue922() -> Result<(), Box<dyn std::error::Error>> {
        let workspace = tempfile::tempdir()?;
        let root = workspace.path().join("root");
        let dependency = workspace.path().join("source-dep");
        fs::create_dir_all(root.join("src"))?;
        fs::create_dir_all(dependency.join("src"))?;
        fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"root\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nsource-dep = { path = \"../source-dep\" }\n",
        )?;
        fs::write(root.join("src/lib.rs"), "pub fn keep() {}\n")?;
        fs::write(
            dependency.join("Cargo.toml"),
            "[package]\nname = \"source-dep\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[lib]\nname = \"source_dep\"\n",
        )?;
        fs::write(
            dependency.join("src/lib.rs"),
            r#"
pub struct ChildId(String);

impl ChildId {
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}
"#,
        )?;

        let query_path = "source_dep::ChildId".to_string();
        let inspector = Inspector::new(InspectorConfig::new(root.clone()));
        inspector.prewarm([query_path.clone()], &|_| ())?;
        let prewarmed = inspector.get(&query_path)?;
        let incan_lang::interop::RustItemKind::Type(prewarmed_type) = &prewarmed.metadata.kind else {
            return Err("expected prewarmed ChildId type metadata".into());
        };
        assert!(
            !prewarmed_type.metadata_completeness.has_methods(),
            "the regression requires the fast prewarm route to persist partial source metadata"
        );

        let query_paths = vec![query_path.clone()];
        let cold = collect_library_rust_abi(&root, &query_paths)?.ok_or("expected cold library Rust ABI")?;
        inspector.cache().get_or_extract_complete(&root, &query_path, &|_| ())?;
        let warm = collect_library_rust_abi(&root, &query_paths)?.ok_or("expected warm library Rust ABI")?;

        assert_eq!(
            cold, warm,
            "library ABI publication must not depend on whether a previous compiler query upgraded the shared cache"
        );
        let child_id = warm.get(&query_path).ok_or("expected ChildId ABI item")?;
        let incan_lang::interop::RustItemKind::Type(child_id_type) = &child_id.kind else {
            return Err("expected ChildId ABI type metadata".into());
        };
        assert!(child_id_type.metadata_completeness.has_methods());
        assert!(child_id_type.methods.iter().any(|method| method.name == "as_str"));
        Ok(())
    }

    #[test]
    fn build_library_canonicalizes_explicit_and_implicit_nested_module_imports()
    -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let project_root = tmp.path();
        let src_dir = project_root.join("src");
        std::fs::create_dir_all(src_dir.join("dataset"))?;

        std::fs::write(
            project_root.join("loaf.toml"),
            "[project]\nname = \"nestedlib\"\nversion = \"0.1.0\"\n",
        )?;
        std::fs::write(
            src_dir.join("lib.incn"),
            "pub from dataset import DataSet\npub from dataset.mod import DataSet as ExplicitDataSet\npub from dataset.ops import filter_ds\n",
        )?;
        std::fs::write(
            src_dir.join("dataset").join("mod.incn"),
            "pub trait DataSet[T]:\n    pass\n",
        )?;
        std::fs::write(
            src_dir.join("dataset").join("ops.incn"),
            "from dataset.mod import DataSet\npub def filter_ds[T](ds: DataSet[T]) -> DataSet[T]:\n    return ds\n",
        )?;

        let cargo_lock_payload =
            std::fs::read_to_string(oven_model::toolchain_layout::development_root().join("Cargo.lock"))?;
        let fingerprint = compute_deps_fingerprint(&[], &[], &CargoFeatureSelection::default(), Some(project_root));
        let incan_lock = IncanLock::new(
            incan_lang::version::INCAN_VERSION,
            fingerprint,
            CargoFeatureSelection::default(),
            cargo_lock_payload,
        );
        incan_lock.write(&project_root.join("oven.lock"))?;

        let lib_path = src_dir.join("lib.incn");
        let lib_path_str = lib_path
            .to_str()
            .ok_or("lib path should be valid utf-8 for build_library test")?;
        let exit = build_library(
            Some(lib_path_str),
            None,
            BuildCommandOptions::default(),
            BuildReportOptions::default(),
        )?;
        assert_eq!(exit, ExitCode::SUCCESS);

        let generated_lib = project_root.join("target").join("lib").join("src").join("lib.rs");
        let generated_dataset = project_root
            .join("target")
            .join("lib")
            .join("src")
            .join("dataset")
            .join("mod.rs");
        let generated_flat_dataset = project_root.join("target").join("lib").join("src").join("dataset.rs");

        let generated_lib_source = std::fs::read_to_string(&generated_lib)?;
        let generated_dataset_source = std::fs::read_to_string(&generated_dataset)?;

        assert!(
            !generated_lib_source.contains("crate::dataset::r#mod"),
            "generated lib.rs should not reference crate::dataset::r#mod"
        );
        assert!(
            !generated_dataset_source.contains("crate::dataset::r#mod"),
            "generated dataset/mod.rs should not reference crate::dataset::r#mod"
        );
        assert!(
            !generated_flat_dataset.exists(),
            "stale flat dataset.rs should not exist after nested library build"
        );

        Ok(())
    }

    #[test]
    fn build_library_publishes_an_executable_surface_joined_to_the_manifest_by_identity()
    -> Result<(), Box<dyn std::error::Error>> {
        // RFC 123's core claim is that the manifest and the representation are joined by the identity space rather
        // than by file naming or ordering. This proves exactly that: the identity is taken from the manifest the
        // build published, hydrated, and looked up in the surface. Nothing here reconstructs an identity or matches
        // on a spelling, because a consumer is forbidden from doing either.
        use incan_semantics_core::SymbolOrigin;
        use incan_semantics_core::executable_representation::SurfaceReader;

        let tmp = tempfile::tempdir()?;
        let project_root = tmp.path();
        let src_dir = project_root.join("src");
        std::fs::create_dir_all(&src_dir)?;
        std::fs::write(
            project_root.join("loaf.toml"),
            "[project]\nname = \"surfacelib\"\nversion = \"0.1.0\"\n",
        )?;
        std::fs::write(
            src_dir.join("lib.incn"),
            "pub def doubled(value: int) -> int:\n    return value * 2\n",
        )?;

        let cargo_lock_payload =
            std::fs::read_to_string(oven_model::toolchain_layout::development_root().join("Cargo.lock"))?;
        let fingerprint = compute_deps_fingerprint(&[], &[], &CargoFeatureSelection::default(), Some(project_root));
        IncanLock::new(
            incan_lang::version::INCAN_VERSION,
            fingerprint,
            CargoFeatureSelection::default(),
            cargo_lock_payload,
        )
        .write(&project_root.join("oven.lock"))?;

        let lib_path = src_dir.join("lib.incn");
        let lib_path_str = lib_path.to_str().ok_or("lib path should be valid utf-8")?;
        assert_eq!(
            build_library(
                Some(lib_path_str),
                None,
                BuildCommandOptions::default(),
                BuildReportOptions::default(),
            )?,
            ExitCode::SUCCESS
        );

        let manifest_path = project_root.join("target").join("lib").join("surfacelib.incnlib");
        let manifest = LibraryManifest::read_from_path(&manifest_path)?;
        let published = manifest
            .contract_metadata
            .identity_graph
            .function_identities_for_public_name("doubled")
            .into_iter()
            .flatten()
            .next()
            .ok_or("the manifest must publish a canonical identity for the exported function")?;

        // The module path comes from the published identity, never from the source file's name. A consumer has only
        // the identity, so a test that hardcodes a path tests something no consumer can do — and would still pass
        // against a producer that wrote the file somewhere else entirely.
        let SymbolOrigin::Package { .. } = &published.origin else {
            return Err("a published library export must carry a package origin".into());
        };
        let surface_path =
            incan_frontend::library_manifest::published_layout::executable_surface_path(&manifest_path, &manifest)
                .ok_or("the surface path must be derivable from the manifest path")?;
        assert!(
            surface_path.is_file(),
            "a library build must publish its executable representation at {}",
            surface_path.display()
        );

        let bytes = std::fs::read(&surface_path)?;
        let reader = SurfaceReader::open(&bytes)?;
        assert!(
            reader.covers(&published),
            "the surface must cover the identity the manifest published; it covers {:?}",
            reader.covered_identities().collect::<Vec<_>>()
        );
        assert_eq!(
            reader.declaration(&published)?.name,
            "doubled",
            "resolving the manifest's identity must reach the declaration it names"
        );
        Ok(())
    }

    /// Exercise the artifact-only `incan build --lib` publisher without mutating its process-wide internal mode flag.
    #[test]
    fn build_library_omits_private_generic_implementation_requirements_issue1280()
    -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let project_root = tmp.path();
        let src_dir = project_root.join("src");
        std::fs::create_dir_all(&src_dir)?;

        std::fs::write(
            project_root.join("loaf.toml"),
            "[project]\nname = \"privateimpl\"\nversion = \"0.1.0\"\n",
        )?;
        std::fs::write(
            src_dir.join("lib.incn"),
            r#"
"""Exercise a private generic implementation without publishing its compiler metadata."""

trait Copyable:
    def copy(self) -> Self: ...

model PrivateValue[T] with Copyable:
    value: T

    def copy(self) -> Self:
        return self

pub def answer() -> int:
    """Return the public library value."""
    return 42
"#,
        )?;

        let cargo_lock_payload =
            std::fs::read_to_string(oven_model::toolchain_layout::development_root().join("Cargo.lock"))?;
        let fingerprint = compute_deps_fingerprint(&[], &[], &CargoFeatureSelection::default(), Some(project_root));
        let incan_lock = IncanLock::new(
            incan_lang::version::INCAN_VERSION,
            fingerprint,
            CargoFeatureSelection::default(),
            cargo_lock_payload,
        );
        incan_lock.write(&project_root.join("oven.lock"))?;

        let lib_path = src_dir.join("lib.incn");
        let lib_path_str = lib_path
            .to_str()
            .ok_or("lib path should be valid utf-8 for build_library test")?;
        let mut prepared = prepare_library_project(
            Some(lib_path_str),
            None,
            CargoPolicy::default(),
            &FeatureSelection::default(),
            None,
            Vec::new(),
            false,
            false,
            None,
            false,
            false,
            OvenProjectPlanMode::ConsumeOnly,
            None,
            &BackendSelectionOptions::default(),
        )?;
        write_library_manifest_artifacts(&mut prepared)?;

        let manifest_path = project_root.join("target/lib/privateimpl.incnlib");
        let manifest = LibraryManifest::read_from_path(&manifest_path)?;
        let checked_api = manifest
            .contract_metadata
            .api
            .as_ref()
            .ok_or("library should publish checked API metadata")?;
        assert!(
            !checked_api
                .modules
                .iter()
                .flat_map(|module| &module.declarations)
                .any(|declaration| {
                    matches!(declaration, ApiDeclaration::Model(model) if model.name == "PrivateValue")
                }),
            "private implementation targets must not leak into checked API metadata"
        );
        assert!(
            !manifest.exports.models.iter().any(|model| model.name == "PrivateValue"),
            "private implementation targets must not leak into public model exports"
        );
        assert!(
            !std::fs::read_to_string(manifest_path)?.contains("PrivateValue"),
            "private implementation targets must not leak into the serialized library manifest"
        );

        Ok(())
    }

    #[test]
    fn build_library_publishes_public_registry_metadata_and_facade_projections()
    -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let project_root = tmp.path();
        let src_dir = project_root.join("src");
        std::fs::create_dir_all(&src_dir)?;

        std::fs::write(
            project_root.join("loaf.toml"),
            "[project]\nname = \"registrylib\"\nversion = \"0.1.0\"\n",
        )?;
        std::fs::write(
            src_dir.join("lib.incn"),
            r#"
from std.registry import Registry, SubjectKind, describe

@derive(Clone, Eq)
pub type FunctionId = newtype str

@derive(Descriptor)
pub model FunctionSpec:
    pub summary: str

pub static public_functions: Registry[FunctionId, FunctionSpec] = Registry.define(
    subjects=[SubjectKind.Function],
)
static private_functions: Registry[FunctionId, FunctionSpec] = Registry.define(
    subjects=[SubjectKind.Function],
)

@describe(public_functions, FunctionId("public"), FunctionSpec(summary="public"))
pub def public_function() -> None:
    pass

@describe(private_functions, FunctionId("private"), FunctionSpec(summary="private"))
def private_function() -> None:
    pass

pub from crate.feature import functions as public_feature_functions
pub from crate.feature import normalize as public_normalize
"#,
        )?;
        std::fs::write(
            src_dir.join("feature.incn"),
            r#"
from std.registry import Registry, SubjectKind, describe

@derive(Clone, Eq)
pub type FunctionId = newtype str

@derive(Descriptor)
pub model FunctionSpec:
    pub summary: str

pub static functions: Registry[FunctionId, FunctionSpec] = Registry.define(
    subjects=[SubjectKind.Function],
)

@describe(functions, FunctionId("normalize"), FunctionSpec(summary="Normalize text"))
pub def normalize(value: str) -> str:
    return value
"#,
        )?;

        let cargo_lock_payload =
            std::fs::read_to_string(oven_model::toolchain_layout::development_root().join("Cargo.lock"))?;
        let fingerprint = compute_deps_fingerprint(&[], &[], &CargoFeatureSelection::default(), Some(project_root));
        let incan_lock = IncanLock::new(
            incan_lang::version::INCAN_VERSION,
            fingerprint,
            CargoFeatureSelection::default(),
            cargo_lock_payload,
        );
        incan_lock.write(&project_root.join("oven.lock"))?;

        let lib_path = src_dir.join("lib.incn");
        let lib_path_str = lib_path
            .to_str()
            .ok_or("lib path should be valid utf-8 for build_library test")?;
        let exit = build_library(
            Some(lib_path_str),
            None,
            BuildCommandOptions::default(),
            BuildReportOptions::default(),
        )?;
        assert_eq!(exit, ExitCode::SUCCESS);

        let manifest = LibraryManifest::read_from_path(&project_root.join("target/lib/registrylib.incnlib"))?;
        let registry = manifest
            .contract_metadata
            .registry
            .ok_or("library should publish checked registry metadata")?;
        assert_eq!(
            registry.schema_version,
            incan_frontend::registry_metadata::CHECKED_REGISTRY_METADATA_SCHEMA_VERSION
        );
        assert_eq!(
            registry.package,
            Some(incan_frontend::registry_metadata::CheckedRegistryPackageIdentity {
                name: "registrylib".to_string(),
                version: Some("0.1.0".to_string()),
            })
        );
        assert_eq!(registry.modules.len(), 2);
        let root = registry
            .modules
            .iter()
            .find(|module| module.module_path == ["lib".to_string()])
            .ok_or("root module should retain its explicit public registry facts")?;
        assert_eq!(root.registries.len(), 1);
        assert_eq!(root.registries[0].identity, "lib::public_functions");
        assert_eq!(root.entries.len(), 1);
        assert_eq!(root.entries[0].subject_identity, "lib.public_function");
        let feature = registry
            .modules
            .iter()
            .find(|module| module.module_path == ["feature".to_string()])
            .ok_or("feature module should retain canonical registry facts")?;
        assert_eq!(feature.registries.len(), 1);
        assert_eq!(feature.entries.len(), 1);
        assert_eq!(
            feature.registries[0]
                .reexport_paths
                .iter()
                .map(|projection| projection.path.clone())
                .collect::<Vec<_>>(),
            vec![vec!["lib".to_string(), "public_feature_functions".to_string()]]
        );
        assert_eq!(
            feature.entries[0]
                .reexport_paths
                .iter()
                .map(|projection| projection.path.clone())
                .collect::<Vec<_>>(),
            vec![vec!["lib".to_string(), "public_normalize".to_string()]]
        );
        assert!(
            !registry
                .modules
                .iter()
                .flat_map(|module| module.registries.iter())
                .any(
                    |definition| definition.identity.contains("__incan_std") || definition.identity.contains("private")
                ),
            "library artifact must contain only its explicit public registry surface"
        );
        Ok(())
    }

    #[test]
    fn the_contract_step_refuses_a_vocab_declaration_with_the_desugar_pass_own_diagnostic()
    -> Result<(), Box<dyn std::error::Error>> {
        // The replacement path owes Body IR a desugared program. This is what "owes" means concretely: a vocab
        // declaration whose library is unavailable stops here, with the resolution failure the desugar pass already
        // reports, rather than travelling on to become a lowering refusal at the same span.
        let program = incan_frontend::ast::Program {
            declarations: vec![undesugared_vocab_declaration()],
            ..Default::default()
        };

        let errors = body_ir::apply_body_ir_input_contract(program, Path::new("/fixture/main.incn"))
            .err()
            .ok_or("an undesugared vocab declaration must not pass the Body IR input contract")?;
        let messages = errors
            .iter()
            .map(|error| error.message.clone())
            .collect::<Vec<_>>()
            .join("; ");
        assert!(
            messages.contains("desugarer resolution failed") && messages.contains("demo.query"),
            "the desugar pass must own this diagnostic, naming the unavailable dependency: {messages}"
        );
        assert!(
            !messages.contains("input-contract violation"),
            "one unavailable-manifest condition must not produce two divergent diagnostics: {messages}"
        );
        Ok(())
    }

    #[test]
    fn the_contract_step_projects_a_body_behind_an_inactive_feature_out_of_the_program()
    -> Result<(), Box<dyn std::error::Error>> {
        // Feature projection has to happen before the module-profile gate and before lowering, so this checks the
        // program the rest of the pipeline actually receives rather than only the eventual build outcome.
        let source =
            "when feature(\"beta\"):\n    def gated() -> int:\n        return 7\n\ndef main() -> int:\n    return 1\n";
        let tokens = lexer::lex(source).map_err(|errors| CliError::failure(format!("{errors:?}")))?;
        let parsed = parser::parse(&tokens).map_err(|errors| CliError::failure(format!("{errors:?}")))?;
        assert_eq!(
            parsed.declarations.len(),
            2,
            "the fixture must carry a gated declaration, or the assertion below proves nothing"
        );

        let projected = body_ir::apply_body_ir_input_contract(parsed, Path::new("/fixture/main.incn"))
            .map_err(|errors| CliError::failure(format!("{errors:?}")))?;
        let names = projected
            .declarations
            .iter()
            .filter_map(|declaration| match &declaration.node {
                Declaration::Function(function) => Some(function.name.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            ["main"],
            "a declaration behind an inactive feature must not survive the contract step"
        );
        Ok(())
    }
}
