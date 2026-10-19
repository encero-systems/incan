//! Explicit Oven Alpha command surface for receipts, bounded plans, and native direct-rustc consumers.
//!
//! Normal build, run, and test consumers never wrap, probe, or launch Cargo, and no command here does either. Native
//! interop bakes publish receipt-bound direct-rustc plans; stored plans run and test through the same executor.

pub mod compile_closure;
mod gate;
pub(crate) mod native_compiler_tests;
mod options;
mod suite_retention;
mod support;

pub use gate::oven_gate_registry_pin;
pub use suite_retention::oven_retain_suite_output;

// The command option shapes and the store/limit/reporting helpers move beside this file rather than into it.
// Every path stays where callers expect it through these re-exports, so this is a move, not an interface change.
pub use options::*;
pub(crate) use support::*;

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::{CliError, CliResult, ExitCode, OvenInteropAdapterArgument, OvenOutputFormat};
use incan_driver::interop_plan::locked_interop_plan_target;
use incan_driver::features::FeatureSelection;
use oven_interop::{
    OvenInteropAdapter, OvenInteropAdapterStageRequest, OvenInteropCapabilitySelection, OvenInteropNativeBakeRequest,
    bake_interop_native_plan, default_interop_execution_receipt_path, load_interop_execution_receipt,
    receipt_interop_execution, selected_interop_toolchain_identity, stage_interop_adapter,
    write_interop_execution_receipt,
};
use oven_model::oven_interop::{LockedInteropTarget, ToolchainRequirement};
use oven_rustc::native_test::{OvenNativeTestRequest, run_native_tests};
use oven_rustc::rustc::{
    OvenRustcArtifactManifest, OvenStoredDirectRustcRunRequest, OvenStoredDirectRustcTestRequest,
    bake_stored_direct_rustc_test, clear_inherited_cargo_environment,
};
use oven_store::digest_bytes;
use oven_store::store::{OvenArtifactKind, OvenArtifactMaterializedFile, OvenArtifactPublishRequest};

pub use incan_driver::oven_store::{
    OVEN_MAX_DOMAIN_LOGICAL_BYTES_ENV, OVEN_MAX_DOMAIN_PHYSICAL_BYTES_ENV, OVEN_MAX_PHYSICAL_BYTES_ENV,
};

/// Validate and publish an immutable direct-rustc artifact manifest into the bounded Oven store.
pub fn oven_publish_direct_rustc_plan(options: OvenPlanPublishCommandOptions) -> CliResult<ExitCode> {
    let receipt = read_receipt(&options.receipt)?;
    let payload = fs::read(&options.manifest).map_err(|error| {
        CliError::failure(format!(
            "failed to read Oven direct-rustc manifest {}: {error}",
            options.manifest.display()
        ))
    })?;
    let plan = serde_json::from_slice::<OvenRustcArtifactManifest>(&payload).map_err(|error| {
        CliError::failure(format!(
            "failed to parse Oven direct-rustc manifest {}: {error}",
            options.manifest.display()
        ))
    })?;
    let materialized_files = plan
        .materialized_artifacts(&options.artifact_root, &receipt.intent)
        .map_err(oven_error)?
        .into_iter()
        .map(|artifact| OvenArtifactMaterializedFile {
            source_path: artifact.source_path,
            relative_path: artifact.relative_path,
        })
        .collect();
    let store = open_store(&options.store)?;
    let artifact = store
        .publish(&OvenArtifactPublishRequest {
            receipt,
            domain: options.domain,
            kind: OvenArtifactKind::DirectRustcPlan,
            payload,
            materialized_files,
            materialized_directories: Vec::new(),
        })
        .map_err(oven_error)?;
    match options.format {
        OvenOutputFormat::Text => println!("Published Oven direct-rustc plan {}.", artifact.identity),
        OvenOutputFormat::Json => print_json(&artifact)?,
    }
    Ok(ExitCode::SUCCESS)
}

/// Bake the conventional targets of one Incan project into sealed Oven Loafs.
///
/// The Cargo-compatible publisher this command delegated to is gone. Project bakes return through the direct route
/// together with direct-route executable builds, so until then the command refuses by name instead of reporting a
/// bake it cannot perform.
pub fn oven_bake_project(
    project: PathBuf,
    _target: Option<String>,
    _package_features: FeatureSelection,
    _format: OvenOutputFormat,
) -> CliResult<ExitCode> {
    Err(CliError::failure(format!(
        "{}: `oven bake --project` has no direct-route publisher yet; it returns with direct-route executable builds",
        project.display()
    )))
}

/// Bake one locked C/C++ interop target into a receipt-bound direct-rustc plan.
///
/// This is an explicit Oven publisher, not a normal command fallback. It consumes only the canonical package lock,
/// selected tool/SDK evidence, the sealed runtime receipt named by `--base-receipt`, and declared package files.
/// `pkg-config` and ambient include or link path discovery are intentionally absent.
pub fn oven_interop_bake(options: OvenInteropBakeCommandOptions) -> CliResult<ExitCode> {
    let locked = locked_interop_plan_target(&options.project, &options.target)?;
    let base_receipt_path = options.base_receipt.clone().ok_or_else(|| {
        CliError::failure("oven interop bake needs --base-receipt naming the sealed Rust-only runtime receipt")
    })?;
    let base_receipt = read_receipt(&base_receipt_path)?;
    let toolchain = selected_compiler_capability(&locked.target, &options)?;
    let sdk = selected_sdk_capability(&locked.target, &options)?;
    let execution_receipt = receipt_interop_execution(&locked.target, toolchain, sdk).map_err(CliError::failure)?;
    let store = open_store(&options.store)?;
    let baked = bake_interop_native_plan(OvenInteropNativeBakeRequest {
        store: &store,
        project_root: &locked.project_root,
        target: &locked.target,
        base_receipt: &base_receipt,
        execution_receipt: &execution_receipt,
        c_compiler: options.c_compiler.as_deref(),
        cxx_compiler: options.cxx_compiler.as_deref(),
        archiver: options.archiver.as_deref(),
        sdk_root: options.sdk_root.as_deref(),
        sdk_identity_file: options.sdk_identity_file.as_deref(),
    })
    .map_err(CliError::failure)?;
    let receipt_path = default_interop_execution_receipt_path(&locked.project_root, &locked.target.target);
    write_interop_execution_receipt(&execution_receipt, &receipt_path).map_err(CliError::failure)?;
    let report = OvenInteropBakeReport {
        target: locked.target.target,
        base_receipt: base_receipt_path,
        execution_receipt: receipt_path,
        execution_receipt_identity: execution_receipt.identity,
        plan_identity: baked.plan_identity,
        final_receipt_identity: baked.receipt.identity,
        archives: baked.archive_names,
        bundles: baked.bundle_names,
        reused: baked.reused,
    };
    match options.format {
        OvenOutputFormat::Text => println!(
            "{} Oven interop target {} direct-rustc plan {}.",
            if report.reused { "Reused" } else { "Baked" },
            report.target,
            report.plan_identity
        ),
        OvenOutputFormat::Json => print_json(&report)?,
    }
    Ok(ExitCode::SUCCESS)
}

/// Stable terminal and JSON evidence emitted by one explicit Oven native interop bake.
#[derive(Debug, Serialize)]
struct OvenInteropBakeReport {
    /// Selected locked target triple.
    target: String,
    /// The pre-interop runtime receipt named by `--base-receipt`.
    base_receipt: PathBuf,
    /// Project-local selected execution receipt written only after final plan publication succeeds.
    execution_receipt: PathBuf,
    /// Selected-execution identity bound into the final direct-rustc build unit.
    execution_receipt_identity: String,
    /// Final immutable direct-rustc plan identity.
    plan_identity: String,
    /// Final receipt identity whose build unit authorizes the immutable direct-Rustc plan.
    final_receipt_identity: String,
    /// Static package inputs and compiled shim outputs exposed through the plan's sealed native search path.
    archives: Vec<String>,
    /// Locked dynamic runtime files retained as digest-verified plan artifacts for direct execution or a target-native
    /// packaging adapter.
    bundles: Vec<String>,
    /// Whether this command reused an existing verified interop plan without starting a native tool.
    reused: bool,
}

/// Stable terminal and JSON evidence emitted after staging a selected interop plan for one native adapter.
#[derive(Debug, Serialize)]
struct OvenInteropStageReport {
    /// Exact locked target triple retained in the adapter manifest.
    target: String,
    /// Fixed caller-selected Android or iOS layout.
    adapter: OvenInteropAdapterArgument,
    /// Caller-owned manifest with only output-relative staged paths.
    manifest: PathBuf,
    /// Reconstructed immutable plan receipt used for selection.
    final_receipt_identity: String,
    /// Immutable direct-Rustc plan identity whose bundled files were copied.
    plan_identity: String,
    /// Number of digest-verified bundled runtime files staged.
    bundled_files: usize,
    /// The staging operation never invokes Cargo, Gradle, Xcode, or a signing tool.
    cargo_process_started: bool,
    /// The staging operation never starts a platform build tool.
    platform_build_process_started: bool,
}

/// Stage one already baked selected interop plan in a fixed caller-owned Android or iOS layout.
///
/// This command reads the lock-fresh target and its project-owned selected execution receipt, reconstructs the exact
/// final receipt, and copies only digest-verified bundled runtime files from the selected store plan. It never probes
/// or starts Cargo, Gradle, Xcode, platform signing, or a native compiler.
pub fn oven_interop_stage(options: OvenInteropStageCommandOptions) -> CliResult<ExitCode> {
    let locked = locked_interop_plan_target(&options.project, &options.target)?;
    let base_receipt = read_receipt(&options.base_receipt)?;
    let execution_receipt_path = default_interop_execution_receipt_path(&locked.project_root, &locked.target.target);
    let execution_receipt = load_interop_execution_receipt(&execution_receipt_path).map_err(CliError::failure)?;
    let store = open_store(&options.store)?;
    let adapter = match options.adapter {
        OvenInteropAdapterArgument::Android => OvenInteropAdapter::Android,
        OvenInteropAdapterArgument::Ios => OvenInteropAdapter::Ios,
    };
    let staged = stage_interop_adapter(OvenInteropAdapterStageRequest {
        store: &store,
        target: &locked.target,
        base_receipt: &base_receipt,
        execution_receipt: &execution_receipt,
        adapter,
        output: &options.output,
    })
    .map_err(CliError::failure)?;
    let report = OvenInteropStageReport {
        target: locked.target.target,
        adapter: options.adapter,
        manifest: staged.manifest_path,
        final_receipt_identity: staged.receipt.identity,
        plan_identity: staged.plan_identity,
        bundled_files: staged.bundled_files,
        cargo_process_started: false,
        platform_build_process_started: false,
    };
    match options.format {
        OvenOutputFormat::Text => println!(
            "Staged {} bundled Oven interop runtime file(s) for {} at {} without Cargo or a platform build tool.",
            report.bundled_files,
            report.target,
            report.manifest.display()
        ),
        OvenOutputFormat::Json => print_json(&report)?,
    }
    Ok(ExitCode::SUCCESS)
}


/// Construct the selected compiler capability record from one explicit executable and semantic-version assertion.
fn selected_compiler_capability(
    target: &LockedInteropTarget,
    options: &OvenInteropBakeCommandOptions,
) -> CliResult<Option<OvenInteropCapabilitySelection>> {
    let Some(requirement) = target.toolchain.as_ref() else {
        if options.c_compiler.is_some() || options.toolchain_version.is_some() {
            return Err(CliError::failure(
                "Oven interop bake received compiler capability inputs for a target that declares no toolchain capability",
            ));
        }
        return Ok(None);
    };
    let compiler = options.c_compiler.as_deref().ok_or_else(|| {
        CliError::failure(format!(
            "locked Oven interop target `{}` requires explicit --c-compiler for capability `{}`",
            target.target, requirement.capability
        ))
    })?;
    let version = required_selected_version(options.toolchain_version.as_deref(), "toolchain", requirement)?;
    let identity = selected_interop_toolchain_identity(
        target,
        Some(compiler),
        options.cxx_compiler.as_deref(),
        options.archiver.as_deref(),
    )
    .map_err(CliError::failure)?;
    Ok(Some(OvenInteropCapabilitySelection {
        capability: requirement.capability.clone(),
        version,
        identity,
    }))
}

/// Construct the selected SDK capability record from a declared root and an explicit regular identity file beneath it.
fn selected_sdk_capability(
    target: &LockedInteropTarget,
    options: &OvenInteropBakeCommandOptions,
) -> CliResult<Option<OvenInteropCapabilitySelection>> {
    let Some(requirement) = target.sdk.as_ref() else {
        if options.sdk_root.is_some() || options.sdk_version.is_some() || options.sdk_identity_file.is_some() {
            return Err(CliError::failure(
                "Oven interop bake received SDK capability inputs for a target that declares no SDK capability",
            ));
        }
        return Ok(None);
    };
    let sdk_root = options.sdk_root.as_deref().ok_or_else(|| {
        CliError::failure(format!(
            "locked Oven interop target `{}` requires --sdk-root for capability `{}`",
            target.target, requirement.capability
        ))
    })?;
    let root = fs::canonicalize(sdk_root).map_err(|error| {
        CliError::failure(format!(
            "could not canonicalize selected SDK root {}: {error}",
            sdk_root.display()
        ))
    })?;
    if !root.is_dir() {
        return Err(CliError::failure(format!(
            "selected SDK root is not a directory: {}",
            root.display()
        )));
    }
    let identity_file = options.sdk_identity_file.as_deref().ok_or_else(|| {
        CliError::failure(format!(
            "locked Oven interop target `{}` requires --sdk-identity-file below --sdk-root",
            target.target
        ))
    })?;
    let identity_file = fs::canonicalize(identity_file).map_err(|error| {
        CliError::failure(format!(
            "could not canonicalize selected SDK identity file {}: {error}",
            identity_file.display()
        ))
    })?;
    if !identity_file.starts_with(&root) {
        return Err(CliError::failure(format!(
            "selected SDK identity file {} is outside selected SDK root {}",
            identity_file.display(),
            root.display()
        )));
    }
    let version = required_selected_version(options.sdk_version.as_deref(), "SDK", requirement)?;
    let identity = selected_regular_file_identity(&identity_file, "selected SDK identity file")?;
    Ok(Some(OvenInteropCapabilitySelection {
        capability: requirement.capability.clone(),
        version,
        identity,
    }))
}

/// Require one non-empty semantic version value from the explicitly selected local capability record.
fn required_selected_version(
    value: Option<&str>,
    label: &str,
    requirement: &ToolchainRequirement,
) -> CliResult<String> {
    let version = value.filter(|value| !value.trim().is_empty()).ok_or_else(|| {
        CliError::failure(format!(
            "selected {label} capability `{}` requires a version",
            requirement.capability
        ))
    })?;
    Ok(version.to_string())
}

/// Digest one explicitly selected regular file without representing its local path as portable receipt identity.
fn selected_regular_file_identity(path: &Path, label: &str) -> CliResult<String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| CliError::failure(format!("could not inspect {label} {}: {error}", path.display())))?;
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        return Err(CliError::failure(format!(
            "{label} must be a regular file: {}",
            path.display()
        )));
    }
    let bytes = fs::read(path)
        .map_err(|error| CliError::failure(format!("could not read {label} {}: {error}", path.display())))?;
    Ok(digest_bytes(&bytes))
}

/// Print physical allocation and logical artifact-byte accounting from the bounded Oven store.
pub fn inspect_oven_store(options: OvenStoreCommandOptions, format: OvenOutputFormat) -> CliResult<ExitCode> {
    let inspection = open_store(&options)?.inspect().map_err(oven_error)?;
    match format {
        OvenOutputFormat::Text => {
            println!("Oven store: {}", inspection.root.display());
            println!(
                "Physical allocation: {} across {} artifact(s), aggregate policy {}.",
                human_bytes(inspection.physical_bytes),
                inspection.entries.len(),
                human_bytes(inspection.limits.max_physical_bytes),
            );
            println!(
                "Logical artifact bytes: {}; per-domain physical policy {}, logical policy {}.",
                human_bytes(inspection.logical_bytes),
                human_bytes(inspection.limits.max_domain_physical_bytes),
                human_bytes(inspection.limits.max_domain_logical_bytes),
            );
            println!(
                "Reclaimable physical allocation: {}; lease-protected physical allocation: {}.",
                human_bytes(inspection.reclaimable_physical_bytes),
                human_bytes(inspection.active_lease_physical_bytes),
            );
            for entry in inspection.entries {
                println!(
                    "  {}  domain={}  kind={:?}  logical={}  physical={}",
                    entry.manifest.identity,
                    entry.manifest.domain,
                    entry.manifest.kind,
                    human_bytes(entry.logical_bytes),
                    human_bytes(entry.physical_bytes),
                );
                // Reuse hands a consumer bytes some other invocation produced. The manifest names that receipt;
                // the retained record says what it actually was, and `--format json` carries the whole thing.
                if let Some(receipt) = entry.original_native_receipt.as_ref() {
                    println!(
                        "      produced by receipt {} for build unit {} ({} {} {})",
                        receipt.identity,
                        receipt.build_unit_identity,
                        receipt.intent.target,
                        receipt.intent.profile,
                        receipt.intent.toolchain,
                    );
                }
            }
        }
        OvenOutputFormat::Json => print_json(&inspection)?,
    }
    Ok(ExitCode::SUCCESS)
}

/// Inspect one receipt's reusable build-unit identity, stored-plan selection, and policy-bounded storage state.
///
/// This deliberately does not compile, publish, or prune. It is the command-level evidence surface for a normal
/// Oven consumer: a complete receipt remains source-strict while the build-unit identity explains cross-project
/// plan reuse or the exact reason a new explicit preparation is needed.
pub fn inspect_oven_receipt(options: OvenReceiptInspectCommandOptions) -> CliResult<ExitCode> {
    let receipt = read_receipt(&options.receipt)?;
    let inspection = open_store(&options.store)?.inspect().map_err(oven_error)?;
    let intent_entries = inspection
        .entries
        .iter()
        .filter(|entry| {
            entry.manifest.kind == OvenArtifactKind::DirectRustcPlan && entry.manifest.intent == receipt.intent
        })
        .collect::<Vec<_>>();
    let mut plan_identities = intent_entries
        .iter()
        .filter(|entry| entry.manifest.build_unit_identity == receipt.build_unit_identity)
        .map(|entry| entry.manifest.identity.clone())
        .collect::<Vec<_>>();
    plan_identities.sort();
    let selection = match plan_identities.len() {
        1 => OvenPlanSelectionInspection {
            state: "hit".to_string(),
            plan_identities,
            reason: None,
        },
        0 if intent_entries.is_empty() => OvenPlanSelectionInspection {
            state: "miss".to_string(),
            plan_identities,
            reason: Some(
                "no stored direct-rustc plan has this target, toolchain, profile, and feature intent; no compatible Oven-native provider/dependency unit is installed".to_string(),
            ),
        },
        0 => OvenPlanSelectionInspection {
            state: "miss".to_string(),
            plan_identities,
            reason: Some(
                "stored plans match the execution intent but not this build-unit identity; compiler, runtime, provider, dependency, or selected-feature input changed".to_string(),
            ),
        },
        _ => OvenPlanSelectionInspection {
            state: "ambiguous".to_string(),
            plan_identities,
            reason: Some(
                "multiple immutable direct-rustc plans match this build unit; normal execution refuses ambiguous selection".to_string(),
            ),
        },
    };
    let report = OvenReceiptInspection {
        receipt_identity: receipt.identity.clone(),
        build_unit_identity: receipt.build_unit_identity.clone(),
        intent: receipt.intent,
        build_unit_inputs: receipt.sources.build_unit_inputs,
        selection,
        logical_artifact_bytes: inspection.logical_bytes,
        physical_bytes: inspection.physical_bytes,
        reclaimable_physical_bytes: inspection.reclaimable_physical_bytes,
        active_lease_physical_bytes: inspection.active_lease_physical_bytes,
    };
    match options.format {
        OvenOutputFormat::Text => {
            println!("Oven receipt: {}", report.receipt_identity);
            println!("Build unit: {}", report.build_unit_identity);
            println!("Plan selection: {}", report.selection.state);
            for identity in &report.selection.plan_identities {
                println!("  plan {identity}");
            }
            if let Some(reason) = &report.selection.reason {
                println!("Reason: {reason}");
            }
            println!("Build-unit inputs:");
            for (name, value) in &report.build_unit_inputs {
                println!("  {name}={value}");
            }
            println!(
                "Store: physical {}, logical {}, reclaimable {}, lease-protected {}.",
                human_bytes(report.physical_bytes),
                human_bytes(report.logical_artifact_bytes),
                human_bytes(report.reclaimable_physical_bytes),
                human_bytes(report.active_lease_physical_bytes),
            );
        }
        OvenOutputFormat::Json => print_json(&report)?,
    }
    Ok(ExitCode::SUCCESS)
}

/// Prune only inactive Oven artifacts toward the configured retained physical capacity policy.
pub fn prune_oven_store(
    options: OvenStoreCommandOptions,
    dry_run: bool,
    format: OvenOutputFormat,
) -> CliResult<ExitCode> {
    let store = open_store(&options)?;
    let report = if dry_run { store.preview_prune() } else { store.prune() }.map_err(oven_error)?;
    match format {
        OvenOutputFormat::Text => {
            let action = if report.dry_run { "would reclaim" } else { "reclaimed" };
            println!(
                "Oven physical allocation: {} -> {}; {action} logical artifact bytes {} across {} artifact(s).",
                human_bytes(report.before_physical_bytes),
                human_bytes(report.after_physical_bytes),
                human_bytes(report.removed_logical_bytes),
                report.removed_entries.len(),
            );
            for identity in report.removed_entries {
                println!("  {action} {identity}");
            }
            for identity in report.skipped_active_entries {
                println!("  active {identity}");
            }
        }
        OvenOutputFormat::Json => print_json(&report)?,
    }
    Ok(ExitCode::SUCCESS)
}

/// Select a receipt-bound stored direct-rustc plan, build its native libtest binary, and run verified exact tests.
pub fn oven_test(options: OvenTestCommandOptions) -> CliResult<ExitCode> {
    let receipt = read_receipt(&options.receipt)?;
    let store = open_store(&options.store)?;
    let bake = bake_stored_direct_rustc_test(&OvenStoredDirectRustcTestRequest {
        store: &store,
        plan_identity: options.plan_identity,
        receipt,
        rustc: options.rustc,
        source: options.source,
        output: options.output,
        crate_name: options.crate_name,
        edition: options.edition,
        source_evidence_key: options.source_evidence_key,
    })
    .map_err(oven_error)?;
    let report = run_native_tests(&OvenNativeTestRequest {
        executable: bake.output.clone(),
        exact_names: options.exact_names,
        environment: BTreeMap::new(),
        timeout: None,
    })
    .map_err(oven_error)?;
    match options.format {
        OvenOutputFormat::Text => println!("Oven executed {} exact native test(s).", report.passed.len()),
        OvenOutputFormat::Json => print_json(&report)?,
    }
    Ok(ExitCode::SUCCESS)
}

/// Compile and run one receipt-bound native binary through a stored direct-rustc closure.
pub fn oven_run(options: OvenRunCommandOptions) -> CliResult<ExitCode> {
    let receipt = read_receipt(&options.receipt)?;
    let store = open_store(&options.store)?;
    let (bake, _execution_lease) = crate::project_owner::prepare_stored_native(OvenStoredDirectRustcRunRequest {
        store: &store,
        plan_identity: options.plan_identity,
        receipt,
        rustc: options.rustc,
        source: options.source,
        output: options.output,
        crate_name: options.crate_name,
        edition: options.edition,
        source_evidence_key: options.source_evidence_key,
    })?;
    run_prepared_native(
        &bake.output,
        &options.arguments,
        options.format,
        bake.cargo_process_started,
        bake.reused,
    )
}

/// Execute in the invoking process so stdin, stdout, stderr and program arguments never cross owner transport.
/// The caller must retain its original Store execution lease until this function returns.
fn run_prepared_native(
    executable: &Path,
    arguments: &[std::ffi::OsString],
    format: OvenOutputFormat,
    cargo_process_started: bool,
    reused: bool,
) -> CliResult<ExitCode> {
    let mut command = std::process::Command::new(executable);
    command.args(arguments);
    clear_inherited_cargo_environment(&mut command);
    let status = command.status().map_err(|error| {
        CliError::failure(format!(
            "failed to run Oven native binary {}: {error}",
            executable.display()
        ))
    })?;
    if !status.success() {
        return Err(CliError::failure(format!(
            "Oven native binary {} exited with {status}",
            executable.display(),
        )));
    }
    match format {
        OvenOutputFormat::Text => println!("Oven ran {} without a Cargo consumer.", executable.display()),
        OvenOutputFormat::Json => print_json(&serde_json::json!({
            "executable": executable,
            "cargo_process_started": cargo_process_started,
            "reused": reused,
        }))?,
    }
    Ok(ExitCode::SUCCESS)
}

