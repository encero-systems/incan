//! Build and run pipeline for Incan projects.
//!
//! `incan build` and `incan run` submit executable builds to the shared project owner, which reuses a completed
//! Oven output or builds through the direct route; `incan run` then executes the binary the build reports. Library
//! builds return as direct-route library units.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::commands::build_report::emit_workspace_build_report;
use crate::{CliError, CliResult, ExitCode};
use incan_driver::build::inline_command::{inline_command_project, wrap_inline_command_source};
use incan_driver::build::rust_extern::{RustExternBuildFailureKind, RustExternDeclContext};
use incan_driver::build::{BuildCommandOptions, RunProfile};
use incan_driver::build_report::BuildReportOptions;
use incan_driver::lock::policy::LockPolicy;
use incan_frontend::diagnostics;
use incan_driver::features::FeatureSelection;
use oven_rustc::rustc::clear_inherited_cargo_environment;

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


/// Build an Incan file to a native executable.
pub fn build_file(
    file_path: &str,
    output_dir: Option<&String>,
    options: BuildCommandOptions,
    report_options: BuildReportOptions,
) -> CliResult<ExitCode> {
    let report = build_file_report(file_path, output_dir, options, &report_options)?;
    emit_workspace_build_report(&report, &report_options)?;
    Ok(ExitCode::SUCCESS)
}

/// Submit an executable build to the shared owner; presentation remains local to this request.
pub(crate) fn build_file_report(
    file_path: &str,
    output_dir: Option<&String>,
    options: BuildCommandOptions,
    report_options: &BuildReportOptions,
) -> CliResult<serde_json::Value> {
    crate::project_owner::build(file_path, output_dir, options, |message| {
        print_build_progress(report_options, message);
    })
}

/// Build a Loaf as a library unit (`incan build --lib`).
pub fn build_library(
    file_path: Option<&str>,
    output_dir: Option<&String>,
    options: BuildCommandOptions,
    report_options: BuildReportOptions,
) -> CliResult<ExitCode> {
    let report = build_library_report(file_path, output_dir, options, &report_options)?;
    emit_workspace_build_report(&report, &report_options)?;
    Ok(ExitCode::SUCCESS)
}

/// Build one library project as a library unit (DD-0007), reporting its artifact directory.
pub(crate) fn build_library_report(
    file_path: Option<&str>,
    _output_dir: Option<&String>,
    options: BuildCommandOptions,
    report_options: &BuildReportOptions,
) -> CliResult<serde_json::Value> {
    let entry = incan_driver::compile::library_entry(Path::new(file_path.unwrap_or(".")))?;
    let profile = match options.executable_profile {
        RunProfile::Debug => "debug",
        RunProfile::Release => "release",
    };
    let unit = incan_driver::compile::compile_unit(
        &incan_driver::compile::UnitRequest {
            source: &entry,
            kind: incan_driver::compile::UnitKind::Library,
            profile,
            features: &options.package_features,
            output_dir: None,
        },
        &mut |message| print_build_progress(report_options, message),
    )?;
    Ok(serde_json::json!({
        "status": "success",
        "mode": "library",
        "profile": profile,
        "library_root": unit.project_root,
        "project": { "project_root": unit.project_root, "name": unit.name },
        "artifacts": [{ "kind": "library", "path": unit.artifact }],
    }))
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
/// Mirrors `inspect_oven_receipt`'s read-verify-render shape, but has no bounded store to consult: a backend-selection
/// receipt is self-contained, so verification is just `BackendExecutionReceipt::verify_identity`.
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
            println!("backend revision:   {}", receipt.selection.implementation_revision);
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

/// Build and run an Incan file.
pub fn run_file(
    file_path: &str,
    lock_policy: LockPolicy,
    package_features: FeatureSelection,
    release: bool,
    program_args: Vec<String>,
) -> CliResult<ExitCode> {
    let report = crate::project_owner::build(file_path, None, run_options(lock_policy, package_features, release), |message| {
        eprintln!("{message}");
    })?;
    run_reported_executable(&report, &program_args)
}

/// Build and run inline Incan source from `incan run -c`.
pub fn run_inline_source(
    source: &str,
    lock_policy: LockPolicy,
    package_features: FeatureSelection,
    release: bool,
    program_args: Vec<String>,
) -> CliResult<ExitCode> {
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
    let result = crate::project_owner::build(
        &source_arg,
        Some(&inline_project.output_dir),
        run_options(lock_policy, package_features, release),
        |message| eprintln!("{message}"),
    )
    .and_then(|report| run_reported_executable(&report, &program_args));
    let _ = fs::remove_file(&source_path);
    result
}

/// The executable build `incan run` requests: the debug profile unless `--release` asks otherwise.
fn run_options(lock_policy: LockPolicy, package_features: FeatureSelection, release: bool) -> BuildCommandOptions {
    BuildCommandOptions {
        executable_profile: if release { RunProfile::Release } else { RunProfile::Debug },
        lock_policy,
        package_features,
    }
}

/// Run the binary a successful executable build reported, from its project root, with the program's arguments.
///
/// The child gets no inherited Cargo state; its exit code becomes the command's exit code.
fn run_reported_executable(report: &serde_json::Value, program_args: &[String]) -> CliResult<ExitCode> {
    let binary = report["artifacts"]
        .as_array()
        .and_then(|artifacts| artifacts.iter().find(|artifact| artifact["kind"] == "binary"))
        .and_then(|artifact| artifact["path"].as_str())
        .map(PathBuf::from)
        .ok_or_else(|| CliError::failure("the executable build reported no binary artifact to run"))?;
    let mut command = Command::new(&binary);
    if let Some(project_root) = report["project"]["project_root"].as_str() {
        command.current_dir(Path::new(project_root));
    }
    clear_inherited_cargo_environment(&mut command);
    command.args(program_args);
    let status = command
        .status()
        .map_err(|error| CliError::failure(format!("failed to run {}: {error}", binary.display())))?;
    Ok(ExitCode(status.code().unwrap_or(ExitCode::FAILURE.0)))
}

