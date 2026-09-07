//! Explicit Oven Alpha command surface for receipts, bounded plans, and native direct-rustc consumers.
//!
//! Normal build, run, and test consumers never wrap, probe, or launch Cargo. Frozen Cargo declarations are
//! compatibility input to `oven import`; only the hidden, explicitly named `oven legacy-cargo` baker may materialize
//! missing compatibility inputs with Cargo. The compiler self-suite uses the same sealed direct-rustc executor and
//! may grant a logged Cargo proxy only to roots whose tests explicitly verify Cargo compatibility.

mod options;
mod support;

// The command option shapes and the store/limit/reporting helpers move beside this file rather than into it.
// Every path stays where callers expect it through these re-exports, so this is a move, not an interface change.
pub use options::*;
pub(crate) use support::*;

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::env;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::cli::commands::interop_plan::locked_interop_plan_target;
use crate::cli::{
    CliError, CliResult, ExitCode, OvenInteropAdapterArgument, OvenLoafEnvelopeArgument, OvenOutputFormat,
};
use crate::oven::compiler_suite_env::{
    OVEN_COMPILER_SUITE_CAPABILITY_ENV, OVEN_COMPILER_SUITE_EXPLICIT_BAKE_CARGO_ENV,
    OVEN_COMPILER_SUITE_EXPLICIT_BAKE_HOME_ENV, OVEN_COMPILER_SUITE_FIXTURE_CARGO_LOG_ENV,
    OVEN_COMPILER_SUITE_FIXTURE_CARGO_REAL_ENV, OVEN_COMPILER_SUITE_FIXTURE_RUSTC_REAL_ENV,
    OVEN_COMPILER_SUITE_RUSTC_ENV, OVEN_COMPILER_SUITE_VOCAB_CAPABILITY_ENV, OvenCompilerSuiteCapability,
    OvenCompilerSuiteTargetCapabilities,
};
use crate::oven::interop::{
    OvenInteropAdapter, OvenInteropAdapterStageRequest, OvenInteropCapabilitySelection, OvenInteropNativeBakeRequest,
    bake_interop_native_plan, default_interop_execution_receipt_path, load_interop_execution_receipt,
    receipt_interop_execution, selected_interop_toolchain_identity, stage_interop_adapter,
    write_interop_execution_receipt,
};
use crate::oven::legacy_cargo::{
    OVEN_COMPILER_TEST_SUITE_FOUNDATION_SCHEMA_VERSION, OVEN_COMPILER_TEST_SUITE_SCHEMA_VERSION,
    OVEN_COMPILER_TEST_SUITE_SHARD_SCHEMA_VERSION, OVEN_COMPILER_TEST_SUITE_SHARD_SCHEMA_VERSION_V1,
    OVEN_COMPILER_TEST_SUITE_TOOLCHAIN_DATA_SCHEMA_VERSION, OVEN_LEGACY_CARGO_INSPECTION_AUTHORITY_ENV,
    OvenCompilerTestSuiteFoundationPayload, OvenCompilerTestSuiteFoundationReference, OvenCompilerTestSuitePayload,
    OvenCompilerTestSuiteShardPayload, OvenCompilerTestSuiteShardReference, OvenCompilerTestSuiteToolchainDataPayload,
    OvenCompilerTestSuiteToolchainDataReference, OvenCompilerWorkspaceLibrary, OvenCompilerWorkspaceLibraryKey,
    OvenLegacyCargoCompilerSuiteResult, OvenLegacyCargoDirectDependencyClosure, OvenLegacyCargoInspectionSource,
    OvenLegacyCargoPrepareRequest, OvenLegacyCargoPublicationKind, legacy_cargo_inspection_sources,
    legacy_cargo_resolved_registry_sources, prepare_compiler_test_suite, prepare_direct_rustc_plan,
    stage_locked_loaf_fixture,
};
use crate::oven::loaf::{
    LoafTemporaryDirectory, OVEN_LOAF_ENV, OVEN_LOAF_ENVELOPE_MANIFEST_SCHEMA_VERSION, OvenLoafBakerContext,
    OvenLoafEnvelope, OvenLoafEnvelopeManifest, OvenLoafEnvelopeMember, OvenLoafFixtureAction, OvenLoafMemberRole,
    OvenLoafPreparation, acquire_committed_loaf_generation, acquire_exclusive_loaf_generation_lock,
    commit_loaf_generation, digest_runtime_crate_source, loaf_directory_byte_counts, loaf_envelope_inspection_packages,
    loaf_envelope_specifications, loaf_raw_disk_bytes, prepare_loaf_from_generated_project,
    retire_unreferenced_loaf_generations, validate_stored_loaf_for_reuse,
};
use crate::oven::native_test::{
    OvenNativeTestCaseCounts, OvenNativeTestCaseTiming, OvenNativeTestCommandTiming, OvenNativeTestRequest,
    run_native_test_batch_all_in_directory_with_timeout,
    run_native_test_batch_all_in_directory_with_timeout_and_threads, run_native_tests,
    run_native_tests_exact_in_directory_with_timeout,
};
use crate::oven::rustc::{
    OvenCallerOwnedRustcLibrary, OvenRustcArtifactManifest, OvenRustcArtifactPlan, OvenStoredDirectRustcRunRequest,
    OvenStoredDirectRustcTestRequest, OvenTrustedDirectRustcTargetRequest, OvenTrustedRustcArtifactRoot,
    OvenTrustedRustdocTestRequest, attach_caller_owned_rustc_libraries, bake_stored_direct_rustc_run,
    bake_stored_direct_rustc_test, bake_trusted_direct_rustc_dylib, bake_trusted_direct_rustc_library,
    bake_trusted_direct_rustc_proc_macro, bake_trusted_direct_rustc_run, bake_trusted_direct_rustc_test,
    clear_inherited_cargo_environment, resolve_active_rustc, resolve_compile_environment_value,
    run_trusted_rustdoc_test, rustc_dynamic_library_environment, rustc_host_target, rustc_identity,
};
use crate::oven::store::{
    OvenArtifactKind, OvenArtifactMaterializedFile, OvenArtifactPublishRequest, OvenStore, OvenStoreExecutionPayload,
    OvenStoreInspection, OvenStoreLease, OvenStoreLimits,
};
use crate::oven::{
    DEFAULT_OVEN_COMPILER_SUITE_MAX_DOMAIN_LOGICAL_BYTES, DEFAULT_OVEN_COMPILER_SUITE_MAX_DOMAIN_PHYSICAL_BYTES,
    DEFAULT_OVEN_COMPILER_SUITE_MAX_PHYSICAL_BYTES, DEFAULT_OVEN_MAX_DOMAIN_LOGICAL_BYTES,
    DEFAULT_OVEN_MAX_DOMAIN_PHYSICAL_BYTES, DEFAULT_OVEN_MAX_PHYSICAL_BYTES, OVEN_COMPILER_TEST_PROFILE,
    OvenBuildIntent, OvenCompilerSuiteRequest, OvenImportRequest, OvenReceipt, default_receipt_path, digest_bytes,
    import_frozen_project, receipt_native_compiler_suite, write_receipt,
};
use crate::oven_interop::{LockedInteropTarget, ToolchainRequirement};
use crate::provider::FeatureSelection;
#[cfg(feature = "rust_inspect")]
use crate::rust_inspect::{OvenInspectionRegistrySource, write_sealed_oven_inspection_source_authority};
use crate::version::INCAN_VERSION;

/// Environment override for aggregate physical allocation policy.
pub const OVEN_MAX_PHYSICAL_BYTES_ENV: &str = "INCAN_OVEN_MAX_PHYSICAL_BYTES";
/// Environment override for per-domain physical allocation policy.
pub const OVEN_MAX_DOMAIN_PHYSICAL_BYTES_ENV: &str = "INCAN_OVEN_MAX_DOMAIN_PHYSICAL_BYTES";
/// Environment override for per-domain logical artifact-byte policy.
pub const OVEN_MAX_DOMAIN_LOGICAL_BYTES_ENV: &str = "INCAN_OVEN_MAX_DOMAIN_LOGICAL_BYTES";
/// Optional bounded worker count for independent compiler-suite roots after the shared direct-Rustc DAG is ready.
pub const OVEN_COMPILER_TEST_JOBS_ENV: &str = "INCAN_OVEN_COMPILER_TEST_JOBS";
/// Maximum wall-clock time for one stored compiler-suite root before the scheduler records a bounded failure.
///
/// A root that exceeds this limit is a suite failure with its partial libtest transcript retained; it must not hold
/// the complete worker pool indefinitely on one host-specific child process.
///
/// Constrained hosted runners can require more than fifteen minutes for the two largest integration roots even
/// though prepared reference-machine replay remains inside the five-minute suite budget. The unsharded release
/// evidence workflow (`oven_evidence.yml`) runs every root in one job rather than the four-way split the ordinary
/// CI workflow uses, so its two largest roots (`cli_integration`, `integration_tests`) have repeatedly needed more
/// than thirty minutes under real hosted-runner contention. Keep a deterministic per-root ceiling, but calibrate it
/// with enough headroom that slow hardware is not misreported as a test failure.
const OVEN_COMPILER_TEST_ROOT_TIMEOUT: Duration = Duration::from_secs(60 * 60);

/// Compiler-owned receipt destination for the full native workspace-test compatibility unit.
const COMPILER_LIBTEST_RECEIPT_RELATIVE_PATH: &str = ".incan/oven/compiler-libtests-receipt.json";

/// Explicitly select or bake sealed Loafs for the conventional targets in one supported Incan project.
///
/// This is Oven's explicit project publication boundary. It records one generated-project receipt per present
/// library or executable profile, reuses a selected closure when available, and otherwise delegates one bounded
/// compatibility publication to the normal Oven store. Normal build, run, and test remain consumers: they neither
/// discover nor launch Cargo.
pub fn oven_bake_project(
    project: PathBuf,
    package_features: FeatureSelection,
    format: OvenOutputFormat,
) -> CliResult<ExitCode> {
    let report = super::build::bake_oven_project_targets(&project, &package_features)?;
    match format {
        OvenOutputFormat::Text => {
            for profile in &report.profiles {
                let verb = match profile.action {
                    "reused" => "Reused",
                    "toolchain_loaf" => "Selected",
                    "baked" => "Baked",
                    _ => "Prepared",
                };
                let detail = match profile.action {
                    "baked" => "through the bounded compatibility baker.",
                    "toolchain_loaf" => "from the active full-stdlib Loaf without invoking Cargo.",
                    _ => "without invoking Cargo.",
                };
                println!(
                    "{verb} Oven {} {profile} plan {} for {} {detail}",
                    profile.project_target,
                    profile.plan_identity,
                    report.project.display(),
                    profile = profile.profile,
                );
                println!("  Receipt: {}", profile.receipt.display());
            }
            println!("Store: {}", report.store.display());
        }
        OvenOutputFormat::Json => print_json(&report)?,
    }
    Ok(ExitCode::SUCCESS)
}

/// Import frozen project declarations, record named source digests, and atomically publish a portable Oven receipt.
pub fn oven_import(options: OvenImportCommandOptions) -> CliResult<ExitCode> {
    let mut request = OvenImportRequest::new(
        &options.project,
        options.target,
        options.toolchain,
        options.profile,
        options.features,
    );
    for source_input in &options.source_inputs {
        let (name, path) = parse_named_path(source_input)?;
        let bytes = fs::read(&path).map_err(|error| {
            CliError::failure(format!("failed to read Oven source input {}: {error}", path.display()))
        })?;
        request = request.with_supplemental_source_digest(name, digest_bytes(&bytes));
    }
    let receipt = import_frozen_project(&request).map_err(oven_error)?;
    let output = options
        .output
        .unwrap_or_else(|| default_receipt_path(request.project_root()));
    write_receipt(&receipt, &output).map_err(oven_error)?;
    match options.format {
        OvenOutputFormat::Text => println!("Published Oven receipt {} at {}.", receipt.identity, output.display()),
        OvenOutputFormat::Json => print_json(&receipt)?,
    }
    Ok(ExitCode::SUCCESS)
}

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
        })
        .map_err(oven_error)?;
    match options.format {
        OvenOutputFormat::Text => println!("Published Oven direct-rustc plan {}.", artifact.identity),
        OvenOutputFormat::Json => print_json(&artifact)?,
    }
    Ok(ExitCode::SUCCESS)
}

/// Prepare one store-owned direct-rustc closure through the explicitly named hidden `legacy_cargo` boundary.
///
/// This command is intentionally separate from normal `build`, `run`, and `test`. It retains the resulting Oven
/// plan and provenance only; its private Cargo target is reclaimed before success returns.
pub fn oven_legacy_cargo_prepare(options: OvenLegacyCargoPrepareCommandOptions) -> CliResult<ExitCode> {
    let receipt = read_receipt(&options.receipt)?;
    let store = open_store(&options.store)?;
    let result = prepare_direct_rustc_plan(&OvenLegacyCargoPrepareRequest {
        store: &store,
        receipt,
        generated_project: options.generated_project,
        cargo: options.cargo,
        rustc: options.rustc,
        sdk_inventory: None,
        compiler_loaf_root: None,
        domain: options.domain,
        publication_kind: OvenLegacyCargoPublicationKind::Executable,
        source_evidence_key: "generated-root".to_string(),
        compile_environment: std::collections::BTreeMap::new(),
        inspection_packages: None,
        direct_dependency_closure: OvenLegacyCargoDirectDependencyClosure::GeneratedSource,
        compact_debug_info: false,
        source_compiler_vocab_support: false,
        base_loaf: None,
    })
    .map_err(oven_error)?;
    match options.format {
        OvenOutputFormat::Text => println!(
            "Prepared Oven direct-rustc plan {} through the internal compatibility publisher.",
            result.plan_identity
        ),
        OvenOutputFormat::Json => print_json(&result)?,
    }
    Ok(ExitCode::SUCCESS)
}

/// Stable terminal and JSON evidence emitted by one explicit Oven native interop bake.
#[derive(Debug, Serialize)]
struct OvenInteropBakeReport {
    /// Selected locked target triple.
    target: String,
    /// The pre-interop runtime receipt selected directly or prepared by this command.
    base_receipt: PathBuf,
    /// Whether this invocation prepared its pre-interop Rust closure instead of receiving `--base-receipt`.
    bootstrap_prepared: bool,
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
    /// Whether an automatic base bootstrap invoked the named compatibility publisher.
    cargo_process_started: bool,
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

/// Select declared native tools, bake locked C/C++ inputs, and publish one receipt-bound direct-rustc plan.
///
/// This is an explicit Oven publisher, not a normal command fallback. It consumes only the canonical package lock,
/// selected tool/SDK evidence, a sealed runtime receipt, and declared package files. When no base receipt is supplied,
/// the command first invokes the named compatibility publisher for the Rust-only closure; it never uses Cargo to
/// discover native inputs. `pkg-config` and ambient include or link path discovery are intentionally absent.
pub fn oven_interop_bake(options: OvenInteropBakeCommandOptions) -> CliResult<ExitCode> {
    let locked = locked_interop_plan_target(&options.project, &options.target)?;
    if options.base_receipt.is_none() && !options.store.is_ordinary_default() {
        return Err(CliError::failure(
            "automatic interop bootstrap uses the ordinary compiler-owned Oven store; set INCAN_HOME for that store or provide --base-receipt when selecting an explicit --store or storage policy",
        ));
    }
    let (base_receipt, base_receipt_path, bootstrap_prepared, cargo_process_started) =
        match options.base_receipt.as_ref() {
            Some(path) => (read_receipt(path)?, path.clone(), false, false),
            None => {
                let (receipt, path, cargo_process_started) =
                    crate::cli::commands::build::prepare_oven_interop_bootstrap(
                        &locked.project_root,
                        &locked.target.target,
                    )?;
                (receipt, path, true, cargo_process_started)
            }
        };
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
        bootstrap_prepared,
        execution_receipt: receipt_path,
        execution_receipt_identity: execution_receipt.identity,
        plan_identity: baked.plan_identity,
        final_receipt_identity: baked.receipt.identity,
        archives: baked.archive_names,
        bundles: baked.bundle_names,
        reused: baked.reused,
        cargo_process_started,
    };
    match options.format {
        OvenOutputFormat::Text => println!("{}", interop_bake_terminal_message(&report)),
        OvenOutputFormat::Json => print_json(&report)?,
    }
    Ok(ExitCode::SUCCESS)
}

/// Render process-authority evidence for one explicit native interop bake.
fn interop_bake_terminal_message(report: &OvenInteropBakeReport) -> String {
    format!(
        "{} Oven interop target {} direct-rustc plan {}{}.",
        if report.reused { "Reused" } else { "Baked" },
        report.target,
        report.plan_identity,
        if report.cargo_process_started {
            " after preparing its Rust-only base through the named compatibility publisher"
        } else {
            " without invoking Cargo"
        }
    )
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

/// Result for one checked fixture in a built-in Loaf envelope.
#[derive(Debug, Serialize)]
struct OvenLoafBakeEntryReport {
    label: String,
    profile: String,
    action: String,
    role: OvenLoafMemberRole,
    result: OvenLoafPreparation,
}

/// Complete result from the hidden, explicit `legacy_cargo` Loaf baker.
#[derive(Debug, Serialize)]
struct OvenLoafBakeReport {
    action: String,
    envelope: String,
    loaf_count: usize,
    prepared_count: usize,
    reused_count: usize,
    logical_bytes: u64,
    physical_bytes: u64,
    owned_physical_bytes: u64,
    raw_disk_bytes: u64,
    reclaimable_physical_bytes: u64,
    active_lease_physical_bytes: u64,
    transient_peak_physical_bytes: u64,
    max_physical_bytes: u64,
    max_domain_physical_bytes: u64,
    max_domain_logical_bytes: u64,
    elapsed_ms: u128,
    /// Cold-baker phase ledger. These phases are measured by Oven itself so CI never has to infer work from shell
    /// command boundaries or Cargo's human output.
    phase_timing: OvenLoafBakePhaseTiming,
    cargo_process_started: bool,
    evidence: OvenLoafEnvelopeEvidence,
    loafs: Vec<OvenLoafBakeEntryReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    compiler_suite: Option<OvenCompilerSuiteBakeReport>,
}

/// Product-owned elapsed-time attribution for one Loaf-baker invocation.
#[derive(Debug, Default, Serialize)]
struct OvenLoafBakePhaseTiming {
    /// Input validation, compatibility evidence, and exact-envelope reuse inspection.
    preflight_elapsed_ms: u128,
    /// Locked registry/inspection authority preparation for a cold envelope.
    inspection_authority_elapsed_ms: u128,
    /// Checked fixture receipt generation and direct-Rustc Loaf preparation.
    fixture_preparation_elapsed_ms: u128,
    /// Atomic generation publication, retirement, and owned-byte accounting.
    envelope_publication_elapsed_ms: u128,
    /// Receipt-bound compiler-suite index/foundation preparation after the envelope is available.
    compiler_suite_preparation_elapsed_ms: u128,
}

/// Source-plan and bounded-store evidence baked with the compiler-suite Loaf envelope.
#[derive(Debug, Serialize)]
struct OvenCompilerSuiteBakeReport {
    receipt: PathBuf,
    prepare: OvenLegacyCargoCompilerSuiteResult,
    store: OvenStoreInspection,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq, Serialize)]
struct OvenLoafEnvelopeEvidence {
    incan_release_version: String,
    /// Report-only provenance for the executable that performed this baker invocation.
    ///
    /// This must not participate in release-family compatibility: rebuilding the same Incan release locally must
    /// not cause every complete standard-library Loaf to be republished.
    compiler_executable_digest: String,
    sdk_inventory_digest: String,
    rustc_identity: String,
    lock_digest: String,
    /// Complete source evidence for the runtime crates compiled into every standard-library Loaf.
    runtime_source_digest: String,
    fixture_digest: String,
}

/// Return the stable wire name used by one built-in Loaf envelope.
fn loaf_envelope_name(envelope: OvenLoafEnvelope) -> &'static str {
    match envelope {
        OvenLoafEnvelope::Release => "release",
        OvenLoafEnvelope::CompilerSuite => "compiler-suite",
    }
}

/// Return the release-family compatibility evidence committed into `envelope.json`.
///
/// The family is selected by the Incan release plus immutable SDK, Rust toolchain, lock, and checked-fixture
/// contracts. The baker executable digest remains report provenance only, so a development rebuild of the same
/// release does not invalidate a complete standard-library Loaf family.
fn loaf_envelope_compatibility_map(evidence: &OvenLoafEnvelopeEvidence) -> BTreeMap<String, String> {
    BTreeMap::from([
        (
            "incan_release_version".to_string(),
            evidence.incan_release_version.clone(),
        ),
        (
            "sdk_inventory_digest".to_string(),
            evidence.sdk_inventory_digest.clone(),
        ),
        ("rustc_identity".to_string(), evidence.rustc_identity.clone()),
        ("lock_digest".to_string(), evidence.lock_digest.clone()),
        (
            "runtime_source_digest".to_string(),
            evidence.runtime_source_digest.clone(),
        ),
        ("fixture_digest".to_string(), evidence.fixture_digest.clone()),
    ])
}

/// Gather release-family compatibility evidence and baker provenance for a built-in envelope.
fn loaf_envelope_evidence(
    envelope: OvenLoafEnvelope,
    compiler_root: &Path,
    compiler_executable: &Path,
    sdk_inventory: &Path,
    rustc: &Path,
) -> CliResult<OvenLoafEnvelopeEvidence> {
    let read_digest = |path: &Path, label: &str| -> CliResult<String> {
        let bytes = fs::read(path)
            .map_err(|error| CliError::failure(format!("could not read Loaf {label} {}: {error}", path.display())))?;
        Ok(digest_bytes(&bytes))
    };
    let lock_path = loaf_compiler_lock_path(compiler_root)?;
    let fixture_evidence = loaf_envelope_specifications(envelope)
        .iter()
        .map(|specification| {
            serde_json::json!({
                "label": specification.label,
                "project_name": specification.project_name,
                "profile": specification.profile,
                "action": match specification.action {
                    OvenLoafFixtureAction::Build => "build",
                    OvenLoafFixtureAction::Run => "run",
                },
                "source": specification.source,
                "manifest": specification.manifest,
                "inspection_manifest": specification.inspection_manifest,
                "role": specification.role,
                "retain_complete_registry_leaves": specification.retain_complete_registry_leaves,
                "retain_checked_direct_dependencies": specification.retain_checked_direct_dependencies,
            })
        })
        .collect::<Vec<_>>();
    let inspection_packages = loaf_envelope_inspection_packages(envelope).map_err(CliError::failure)?;
    Ok(OvenLoafEnvelopeEvidence {
        incan_release_version: INCAN_VERSION.to_string(),
        compiler_executable_digest: read_digest(compiler_executable, "compiler executable")?,
        sdk_inventory_digest: read_digest(sdk_inventory, "SDK inventory")?,
        rustc_identity: rustc_identity(rustc).map_err(oven_error)?,
        lock_digest: read_digest(&lock_path, "lock input")?,
        runtime_source_digest: loaf_runtime_source_digest(compiler_root)?,
        fixture_digest: digest_bytes(
            &serde_json::to_vec(&(fixture_evidence, inspection_packages))
                .map_err(|error| CliError::failure(format!("could not encode Loaf fixture evidence: {error}")))?,
        ),
    })
}

/// Hash the complete compiler-runtime source closure that the Loaf fixture links into its sealed artifacts.
///
/// The executable digest is provenance only: rebuilding the same release binary must not invalidate an otherwise
/// compatible envelope. The runtime crates are different: changing their source without selecting a new envelope
/// could pair an updated compiler with stale `incan_stdlib` or support-crate archives. Keep this evidence portable by
/// recording named content digests rather than checkout paths.
fn loaf_runtime_source_digest(compiler_root: &Path) -> CliResult<String> {
    let mut records = BTreeMap::new();
    let manifest = loaf_compiler_manifest_path(compiler_root)?;
    let manifest_bytes = fs::read(&manifest).map_err(|error| {
        CliError::failure(format!(
            "could not read Loaf runtime manifest {}: {error}",
            manifest.display()
        ))
    })?;
    records.insert("Cargo.toml".to_string(), digest_bytes(&manifest_bytes));
    for (label, relative) in [
        ("incan_core", "crates/incan_core"),
        ("incan_derive", "crates/incan_derive"),
        ("incan_stdlib", "crates/incan_stdlib"),
    ] {
        let root = compiler_root.join(relative);
        let digest = digest_runtime_crate_source(&root).map_err(CliError::failure)?;
        records.insert(label.to_string(), digest);
    }
    let bytes = serde_json::to_vec(&records)
        .map_err(|error| CliError::failure(format!("could not encode Loaf runtime source evidence: {error}")))?;
    Ok(digest_bytes(&bytes))
}

/// Return the checked Cargo workspace manifest for a compiler checkout or packaged toolchain.
///
/// Source checkouts own the complete workspace at the compiler root. Release archives intentionally ship only the
/// runtime support workspace under `crates/`; the Loaf baker must bind to that staged workspace instead of assuming
/// the archive contains the compiler's development-only root manifest.
fn loaf_compiler_manifest_path(compiler_root: &Path) -> CliResult<PathBuf> {
    [
        compiler_root.join("Cargo.toml"),
        compiler_root.join("crates/Cargo.toml"),
    ]
    .into_iter()
    .find(|path| path.is_file())
    .ok_or_else(|| CliError::failure("Loaf compiler root has no canonical Cargo.toml input".to_string()))
}

/// Return the one checked compiler lock used by both envelope identity and cold fixture publication.
fn loaf_compiler_lock_path(compiler_root: &Path) -> CliResult<PathBuf> {
    [
        compiler_root.join("Cargo.lock"),
        compiler_root.join("crates/Cargo.lock"),
    ]
    .into_iter()
    .find(|path| path.is_file())
    .ok_or_else(|| CliError::failure("Loaf compiler root has no canonical Cargo.lock input".to_string()))
}

/// Validate and reuse one exact committed envelope without fixture probes or a Cargo process.
fn reuse_complete_loaf_envelope(
    output: &Path,
    scratch: &Path,
    envelope: OvenLoafEnvelope,
    evidence: &OvenLoafEnvelopeEvidence,
    limits: OvenStoreLimits,
    started: Instant,
) -> CliResult<Option<OvenLoafBakeReport>> {
    let manifest_path = output.join("envelope.json");
    if !manifest_path.is_file() {
        return Ok(None);
    }
    let manifest = serde_json::from_slice::<OvenLoafEnvelopeManifest>(&fs::read(&manifest_path).map_err(|error| {
        CliError::failure(format!(
            "could not read Loaf envelope manifest {}: {error}",
            manifest_path.display()
        ))
    })?)
    .map_err(|error| {
        CliError::failure(format!(
            "invalid Loaf envelope manifest {}: {error}",
            manifest_path.display()
        ))
    })?;
    let expected_evidence = loaf_envelope_compatibility_map(evidence);
    if manifest.schema_version != OVEN_LOAF_ENVELOPE_MANIFEST_SCHEMA_VERSION
        || manifest.envelope != loaf_envelope_name(envelope)
        || manifest.evidence != expected_evidence
    {
        return Ok(None);
    }
    let specifications = loaf_envelope_specifications(envelope);
    if manifest.loafs.len() != specifications.len() {
        return Err(CliError::failure("Loaf envelope manifest is incomplete".to_string()));
    }
    let mut reports = Vec::with_capacity(manifest.loafs.len());
    for (entry, specification) in manifest.loafs.iter().zip(specifications) {
        let expected_action = match specification.action {
            OvenLoafFixtureAction::Build => "build",
            OvenLoafFixtureAction::Run => "run",
        };
        if entry.label != specification.label
            || entry.profile != specification.profile
            || entry.action != expected_action
            || entry.role != specification.role
        {
            return Err(CliError::failure(
                "Loaf envelope manifest does not match its checked specification".to_string(),
            ));
        }
        let loaf_path = output.join(&entry.path);
        let result = validate_stored_loaf_for_reuse(&loaf_path, entry).map_err(oven_error)?;
        if result.logical_bytes > limits.max_domain_logical_bytes
            || result.physical_bytes > limits.max_domain_physical_bytes
        {
            return Err(CliError::failure(format!(
                "stored Loaf `{}` exceeds the active compatibility-domain allowance",
                entry.label
            )));
        }
        reports.push(OvenLoafBakeEntryReport {
            label: entry.label.clone(),
            profile: entry.profile.clone(),
            action: entry.action.clone(),
            role: entry.role,
            result,
        });
    }
    let logical_bytes = reports.iter().map(|entry| entry.result.logical_bytes).sum::<u64>();
    let physical_bytes = reports.iter().map(|entry| entry.result.physical_bytes).sum::<u64>();
    if physical_bytes > limits.max_physical_bytes {
        return Err(CliError::failure(format!(
            "stored Loaf envelope uses {physical_bytes} physical bytes, exceeding its {}-byte allowance",
            limits.max_physical_bytes
        )));
    }
    retire_unreferenced_loaf_generations(output, &manifest.generation_identity, scratch).map_err(oven_error)?;
    let (_, owned_physical_bytes) = loaf_directory_byte_counts(output).map_err(oven_error)?;
    let raw_disk_bytes = loaf_raw_disk_bytes(output).map_err(oven_error)?;
    if owned_physical_bytes > limits.max_physical_bytes {
        return Err(CliError::failure(format!(
            "stored Loaf output uses {owned_physical_bytes} physical bytes after reclaiming obsolete generations, exceeding its {}-byte allowance",
            limits.max_physical_bytes
        )));
    }
    Ok(Some(OvenLoafBakeReport {
        action: "reused".to_string(),
        envelope: loaf_envelope_name(envelope).to_string(),
        loaf_count: reports.len(),
        prepared_count: 0,
        reused_count: reports.len(),
        logical_bytes,
        physical_bytes,
        owned_physical_bytes,
        raw_disk_bytes,
        // Exact reuse holds the exclusive generation lock and has already reclaimed every unreferenced generation.
        // The remaining owned overhead is the active envelope manifest/lock, not reclaimable artifact data.
        reclaimable_physical_bytes: 0,
        active_lease_physical_bytes: 0,
        transient_peak_physical_bytes: 0,
        max_physical_bytes: limits.max_physical_bytes,
        max_domain_physical_bytes: limits.max_domain_physical_bytes,
        max_domain_logical_bytes: limits.max_domain_logical_bytes,
        elapsed_ms: started.elapsed().as_millis(),
        phase_timing: OvenLoafBakePhaseTiming {
            preflight_elapsed_ms: started.elapsed().as_millis(),
            ..OvenLoafBakePhaseTiming::default()
        },
        cargo_process_started: false,
        evidence: evidence.clone(),
        loafs: reports,
        compiler_suite: None,
    }))
}

/// Bind a checked Loaf fixture probe to the compiler selected by the baker.
///
/// The explicit Cargo executable may come from a nightly toolchain solely because the compiler-suite unit graph
/// requires Cargo's unstable `--unit-graph` interface. That must not let the ambient Rustup toolchain choose the
/// compiler recorded in the receipt: `--rustc` is the compatibility authority for both the probe and publication.
fn pin_loaf_fixture_rustc(command: &mut Command, rustc: &Path) {
    command.env("RUSTC", rustc);
}

/// Keep a cold baker probe from selecting an obsolete Loaf generation beside the development executable.
///
/// The probe exists only to derive a fresh receipt and generated project. Giving that maintainer-owned child an
/// empty compiler-data layout makes the intended Oven miss deterministic while the previous committed generation
/// remains intact until its atomic replacement is ready.
fn isolate_loaf_fixture_toolchain_data(command: &mut Command, toolchain_data_root: &Path) {
    command
        .env("INCAN_INTERNAL_OVEN_LOAF_EXECUTION", "1")
        .env("INCAN_INTERNAL_TOOLCHAIN_DATA_ROOT", toolchain_data_root);
}

/// Recognize only the two fail-closed native-plan misses a baker-owned fixture may legitimately produce.
///
/// Both clauses come from the shared constants the messages themselves are built from, so reworded user-facing
/// text stays recognizable here instead of silently turning an intended miss into an unrecognized failure.
fn loaf_fixture_probe_is_expected_miss(stderr: &str) -> bool {
    [
        crate::oven::loaf::OVEN_DEPENDENCY_MISS_SUMMARY,
        crate::oven::loaf::OVEN_NESTED_DEPENDENCY_MISS_SUMMARY,
    ]
    .iter()
    .any(|summary| stderr.contains(summary))
        && stderr.contains(crate::oven::loaf::OVEN_NO_IMPLICIT_DEPENDENCY_BUILD)
}

/// Bake or exactly reuse one complete compiler-owned Alpha Loaf envelope.
///
/// The command is hidden beneath `legacy_cargo` because Cargo may run only for a genuine Loaf miss. Normal
/// build/run/test commands never call this function and never fall back to it.
pub fn oven_legacy_cargo_bake_loafs(options: OvenLoafBakeCommandOptions) -> CliResult<ExitCode> {
    let started = Instant::now();
    if !options.compiler_root.is_dir() {
        return Err(CliError::failure(format!(
            "Loaf compiler root is not a directory: {}",
            options.compiler_root.display()
        )));
    }
    if !options.sdk_inventory.is_file() {
        return Err(CliError::failure(format!(
            "Loaf SDK inventory is not a regular file: {}",
            options.sdk_inventory.display()
        )));
    }
    if !options.cargo.is_file() || !options.rustc.is_file() {
        return Err(CliError::failure(
            "the explicit Loaf baker requires regular --cargo and --rustc executables".to_string(),
        ));
    }
    fs::create_dir_all(&options.output).map_err(|error| {
        CliError::failure(format!(
            "could not create Loaf output {}: {error}",
            options.output.display()
        ))
    })?;
    let publication_lock = acquire_exclusive_loaf_generation_lock(&options.output).map_err(oven_error)?;
    let output_parent = options
        .output
        .parent()
        .ok_or_else(|| CliError::failure("Loaf output has no parent directory".to_string()))?;
    let scratch = LoafTemporaryDirectory::create(output_parent, ".incan-oven-loaf-envelope-")
        .map_err(|error| CliError::failure(format!("could not allocate Loaf baker scratch directory: {error}")))?;
    let staged_root = scratch.path().join("staged");
    fs::create_dir_all(&staged_root)
        .map_err(|error| CliError::failure(format!("could not create Loaf staging root: {error}")))?;

    let envelope = match options.envelope {
        OvenLoafEnvelopeArgument::Release => OvenLoafEnvelope::Release,
        OvenLoafEnvelopeArgument::CompilerSuite => OvenLoafEnvelope::CompilerSuite,
    };
    let default_limits = loaf_envelope_default_limits(envelope);
    let combined_max_physical_bytes = options.max_physical_bytes.unwrap_or(default_limits.max_physical_bytes);
    let existing_suite_physical_bytes = if envelope == OvenLoafEnvelope::CompilerSuite {
        let suite_store = compiler_suite_store_path(&options)?;
        if suite_store.is_dir() {
            crate::oven::legacy_cargo::conservative_directory_reservation(&suite_store).map_err(oven_error)?
        } else {
            0
        }
    } else {
        0
    };
    let max_physical_bytes = combined_max_physical_bytes
        .checked_sub(existing_suite_physical_bytes)
        .filter(|remaining| *remaining > 0)
        .ok_or_else(|| {
            CliError::failure(format!(
                "existing compiler-suite storage uses {existing_suite_physical_bytes} bytes of the complete {combined_max_physical_bytes}-byte baker allowance"
            ))
        })?;
    let max_domain_physical_bytes = options
        .max_domain_physical_bytes
        .unwrap_or(default_limits.max_domain_physical_bytes)
        .min(max_physical_bytes);
    let max_domain_logical_bytes = options
        .max_domain_logical_bytes
        .unwrap_or(default_limits.max_domain_logical_bytes);
    let limits = OvenStoreLimits::new(max_physical_bytes, max_domain_physical_bytes, max_domain_logical_bytes);
    if max_physical_bytes == 0 || max_domain_physical_bytes == 0 || max_domain_logical_bytes == 0 {
        return Err(CliError::failure(
            "Loaf storage limits must be greater than zero".to_string(),
        ));
    }
    if options
        .max_domain_physical_bytes
        .unwrap_or(default_limits.max_domain_physical_bytes)
        > combined_max_physical_bytes
    {
        return Err(CliError::failure(
            "Loaf per-domain physical limit cannot exceed its aggregate physical limit".to_string(),
        ));
    }

    let current_executable = env::current_exe()
        .map_err(|error| CliError::failure(format!("could not resolve the active Incan executable: {error}")))?;
    let evidence = loaf_envelope_evidence(
        envelope,
        &options.compiler_root,
        &current_executable,
        &options.sdk_inventory,
        &options.rustc,
    )?;
    let mut phase_timing = OvenLoafBakePhaseTiming {
        preflight_elapsed_ms: started.elapsed().as_millis(),
        ..OvenLoafBakePhaseTiming::default()
    };
    if let Some(report) =
        reuse_complete_loaf_envelope(&options.output, scratch.path(), envelope, &evidence, limits, started)?
    {
        // Exact envelope validation and retirement require exclusive publication authority. Compiler-suite
        // completion then consumes the committed Loafs through a shared generation lease, so retaining the writer
        // lock across that transition would make this process wait on itself.
        let report = finish_loaf_bake_after_publication(publication_lock, &options, envelope, report, started)?;
        print_loaf_bake_report(&report, options.format)?;
        return Ok(ExitCode::SUCCESS);
    }
    // Private fixture analysis may execute a normal Incan command. That command must be able to take a shared lease
    // on the currently committed generation while it determines whether the old Loaf is compatible. Holding the
    // publisher's exclusive lock here would make the parent wait for a child that is waiting for the parent. The
    // staged generation is private and has no publication authority, so release exclusivity until the atomic commit.
    drop(publication_lock);
    let compatibility_evidence = loaf_envelope_compatibility_map(&evidence);
    let generation_identity = digest_bytes(
        &serde_json::to_vec(&(loaf_envelope_name(envelope), &compatibility_evidence))
            .map_err(|error| CliError::failure(format!("could not encode Loaf generation identity: {error}")))?,
    );
    let generation_name = generation_identity
        .strip_prefix("sha256:")
        .unwrap_or(&generation_identity);
    let generation_relative = Path::new("generations").join(generation_name);
    let generation_output = options.output.join(&generation_relative);
    let generations_root = options.output.join("generations");
    fs::create_dir_all(&generations_root)
        .map_err(|error| CliError::failure(format!("could not create Loaf generations root: {error}")))?;
    let mut pending = Vec::new();
    let envelope_inspection_packages = loaf_envelope_inspection_packages(envelope).map_err(CliError::failure)?;
    // A cold first bake cannot consume a Loaf that does not exist yet. Resolve its Rust inspection sources once at
    // this already explicit Cargo boundary, then hand the typed locked authority to every no-Cargo fixture child.
    let inspection_authority_started = Instant::now();
    let authority_dir = scratch.path().join("rust-inspect-authority");
    fs::create_dir_all(&authority_dir).map_err(|error| {
        CliError::failure(format!(
            "could not create explicit baker Rust inspection authority directory: {error}"
        ))
    })?;
    let compiler_manifest = loaf_compiler_manifest_path(&options.compiler_root)?;
    let envelope_inspection_sources = match envelope {
        OvenLoafEnvelope::CompilerSuite => legacy_cargo_resolved_registry_sources(
            &options.cargo,
            &compiler_manifest,
            &["lsp".to_string()],
            &authority_dir,
        ),
        OvenLoafEnvelope::Release => legacy_cargo_inspection_sources(
            &options.cargo,
            &compiler_manifest,
            &[],
            &envelope_inspection_packages,
            &authority_dir,
        ),
    }
    .map_err(oven_error)?;
    #[cfg(feature = "rust_inspect")]
    let baker_inspection_authority = {
        let sources = envelope_inspection_sources
            .iter()
            .map(|source| OvenInspectionRegistrySource {
                package: source.package.clone(),
                version: source.version.clone(),
                registry: source.registry.clone(),
                checksum: source.checksum.clone(),
                features: source.features.clone(),
                source_root: source.source_root.clone(),
                source_digest: source.source_digest.clone(),
            })
            .collect();
        write_sealed_oven_inspection_source_authority(&authority_dir, sources).map_err(|error| {
            CliError::failure(format!(
                "could not write explicit baker Rust inspection authority: {error}"
            ))
        })?
    };
    phase_timing.inspection_authority_elapsed_ms = inspection_authority_started.elapsed().as_millis();
    let cargo_process_started = true;
    let mut transient_peak_physical_bytes = 0_u64;
    let compiler_support_target = scratch.path().join("compiler-support-target");
    let probe_toolchain_data_root = scratch.path().join("probe-toolchain-data");
    fs::create_dir_all(probe_toolchain_data_root.join("share/incan/oven/loafs")).map_err(|error| {
        CliError::failure(format!(
            "could not create isolated Loaf fixture toolchain data: {error}"
        ))
    })?;
    let compiler_lock = loaf_compiler_lock_path(&options.compiler_root)?;
    let fixture_preparation_started = Instant::now();
    for specification in loaf_envelope_specifications(envelope) {
        let inspection_packages = if specification.role.provides_source_authority() {
            specification.inspection_packages().map_err(CliError::failure)?
        } else {
            Vec::new()
        };
        let inspection_sources: &[OvenLegacyCargoInspectionSource] = if specification.role.provides_source_authority() {
            &envelope_inspection_sources
        } else {
            &[]
        };
        let project_root = scratch
            .path()
            .join("fixtures")
            .join(specification.label)
            .join(specification.profile);
        fs::create_dir_all(&project_root).map_err(|error| {
            CliError::failure(format!(
                "could not create checked Loaf fixture {}: {error}",
                specification.label
            ))
        })?;
        // Project-output authority hashes the conventional `src/` tree. The fixture must use that shape so its
        // deliberate first Oven miss still reaches receipt creation.
        let source_root = project_root.join("src");
        fs::create_dir_all(&source_root).map_err(|error| {
            CliError::failure(format!(
                "could not create checked Loaf fixture source directory {}: {error}",
                source_root.display()
            ))
        })?;
        let source = source_root.join("main.incn");
        fs::write(&source, specification.source).map_err(|error| {
            CliError::failure(format!(
                "could not write checked Loaf fixture {}: {error}",
                source.display()
            ))
        })?;
        fs::write(project_root.join("incan.toml"), specification.manifest).map_err(|error| {
            CliError::failure(format!(
                "could not write checked Loaf manifest {}: {error}",
                specification.label
            ))
        })?;
        let mut command = Command::new(&current_executable);
        command
            .env_remove("INCAN_STDLIB")
            .env_remove("INCAN_STDLIB_DIR")
            .env("INCAN_SOURCE_ROOT", &options.compiler_root)
            .env("INCAN_SDK_INVENTORY", &options.sdk_inventory)
            .env(OVEN_LOAF_ENV, "1")
            .env("INCAN_HOME", project_root.join(".oven-home"));
        #[cfg(feature = "rust_inspect")]
        command.env(OVEN_LEGACY_CARGO_INSPECTION_AUTHORITY_ENV, &baker_inspection_authority);
        pin_loaf_fixture_rustc(&mut command, &options.rustc);
        isolate_loaf_fixture_toolchain_data(&mut command, &probe_toolchain_data_root);
        match (specification.action, specification.profile) {
            (OvenLoafFixtureAction::Build, "release") => {
                command.args(["build", "--release"]).arg(&source);
            }
            (OvenLoafFixtureAction::Build, _) => {
                command.arg("build").arg(&source);
            }
            (OvenLoafFixtureAction::Run, "release") => {
                command.args(["run", "--release"]).arg(&source);
            }
            (OvenLoafFixtureAction::Run, _) => {
                command.arg("run").arg(&source);
            }
        }
        let probe = command.output().map_err(|error| {
            CliError::failure(format!(
                "could not analyze checked Loaf fixture {}: {error}",
                specification.label
            ))
        })?;
        let receipt_path = project_root.join(".incan/oven/receipt.json");
        let generated_project = project_root.join("target/incan").join(specification.project_name);
        let probe_stderr = String::from_utf8_lossy(&probe.stderr);
        if !probe.status.success() && !loaf_fixture_probe_is_expected_miss(&probe_stderr) {
            return Err(CliError::failure(format!(
                "checked Loaf fixture `{}` failed before its expected Oven miss:\n{}",
                specification.label,
                probe_stderr.trim()
            )));
        }
        if !receipt_path.is_file() || !generated_project.is_dir() {
            return Err(CliError::failure(format!(
                "checked Loaf fixture `{}` did not produce its receipt and generated project",
                specification.label
            )));
        }
        stage_locked_loaf_fixture(&options.cargo, &generated_project, &compiler_lock).map_err(oven_error)?;
        let receipt = read_receipt(&receipt_path)?;
        let result = prepare_loaf_from_generated_project(
            &staged_root,
            &OvenLoafBakerContext {
                compiler_root: &options.compiler_root,
                compiler_support_target: &compiler_support_target,
                capacity_roots: [&options.output, scratch.path()],
                transient_limit: max_physical_bytes,
                cargo: &options.cargo,
                rustc: &options.rustc,
                inspection_packages: &inspection_packages,
                inspection_sources,
                retain_complete_registry_leaves: specification.retain_complete_registry_leaves,
                retain_checked_direct_dependencies: specification.retain_checked_direct_dependencies,
                limits,
            },
            receipt,
            &generated_project,
        )
        .map_err(oven_error)?;
        let observed_transient = crate::oven::legacy_cargo::conservative_directory_reservation(&options.output)
            .and_then(|owned| {
                crate::oven::legacy_cargo::conservative_directory_reservation(scratch.path())
                    .map(|transient| owned.saturating_add(transient))
            })
            .map_err(oven_error)?;
        transient_peak_physical_bytes = transient_peak_physical_bytes
            .max(result.transient_peak_physical_bytes)
            .max(observed_transient);
        if observed_transient > max_physical_bytes {
            return Err(CliError::failure(format!(
                "Loaf baker transient storage reached {observed_transient} bytes, exceeding its {max_physical_bytes}-byte allowance"
            )));
        }
        if result.logical_bytes > max_domain_logical_bytes {
            return Err(CliError::failure(format!(
                "Loaf `{}` uses {} logical bytes, exceeding its {}-byte domain allowance",
                specification.label, result.logical_bytes, max_domain_logical_bytes
            )));
        }
        if result.physical_bytes > max_domain_physical_bytes {
            return Err(CliError::failure(format!(
                "Loaf `{}` uses {} physical bytes, exceeding its {}-byte domain allowance",
                specification.label, result.physical_bytes, max_domain_physical_bytes
            )));
        }
        pending.push(OvenLoafBakeEntryReport {
            label: specification.label.to_string(),
            profile: specification.profile.to_string(),
            action: match specification.action {
                OvenLoafFixtureAction::Build => "build",
                OvenLoafFixtureAction::Run => "run",
            }
            .to_string(),
            role: specification.role,
            result,
        });
    }
    phase_timing.fixture_preparation_elapsed_ms = fixture_preparation_started.elapsed().as_millis();

    let logical_bytes = pending.iter().map(|entry| entry.result.logical_bytes).sum::<u64>();
    let physical_bytes = pending.iter().map(|entry| entry.result.physical_bytes).sum::<u64>();
    if physical_bytes > max_physical_bytes {
        return Err(CliError::failure(format!(
            "Loaf envelope uses {physical_bytes} physical bytes, exceeding its {max_physical_bytes}-byte allowance"
        )));
    }

    let prepared_count = pending.len();
    let envelope_publication_started = Instant::now();
    let manifest = OvenLoafEnvelopeManifest {
        schema_version: OVEN_LOAF_ENVELOPE_MANIFEST_SCHEMA_VERSION,
        envelope: loaf_envelope_name(envelope).to_string(),
        generation_identity: generation_identity.clone(),
        evidence: compatibility_evidence,
        loafs: pending
            .iter()
            .map(|entry| {
                let identity = entry
                    .result
                    .loaf_identity
                    .strip_prefix("sha256:")
                    .unwrap_or(&entry.result.loaf_identity);
                OvenLoafEnvelopeMember {
                    label: entry.label.clone(),
                    profile: entry.profile.clone(),
                    action: entry.action.clone(),
                    role: entry.role,
                    build_unit_identity: entry.result.build_unit_identity.clone(),
                    loaf_identity: entry.result.loaf_identity.clone(),
                    plan_identity: entry.result.plan_identity.clone(),
                    logical_bytes: entry.result.logical_bytes,
                    physical_bytes: entry.result.physical_bytes,
                    path: generation_relative.join(format!("{identity}.loaf/loaf.json")),
                }
            })
            .collect(),
    };
    let publication_lock = acquire_exclusive_loaf_generation_lock(&options.output).map_err(oven_error)?;
    let replacement_high_water = crate::oven::legacy_cargo::conservative_directory_reservation(&options.output)
        .and_then(|owned| {
            crate::oven::legacy_cargo::conservative_directory_reservation(scratch.path())
                .map(|transient| owned.saturating_add(transient))
        })
        .map_err(oven_error)?;
    transient_peak_physical_bytes = transient_peak_physical_bytes.max(replacement_high_water);
    if replacement_high_water > max_physical_bytes {
        return Err(CliError::failure(format!(
            "Loaf replacement high water reached {replacement_high_water} bytes, exceeding its {max_physical_bytes}-byte allowance"
        )));
    }
    commit_loaf_generation(
        &options.output,
        &generations_root,
        &generation_output,
        &staged_root,
        &manifest,
        scratch.path(),
        || Ok(()),
    )
    .map_err(oven_error)?;

    // The new manifest is the envelope's single authority. Retire old content-addressed generations only after that
    // authority has committed, so any earlier publication failure leaves the previous complete envelope usable.
    // A retirement failure is safe: the newly committed Loafs remain valid and obsolete unreferenced data can be
    // reclaimed by the next successful bake.
    retire_unreferenced_loaf_generations(&options.output, &generation_identity, scratch.path()).map_err(oven_error)?;

    let (_, owned_physical_bytes) = loaf_directory_byte_counts(&options.output).map_err(oven_error)?;
    let raw_disk_bytes = loaf_raw_disk_bytes(&options.output).map_err(oven_error)?;
    if owned_physical_bytes > max_physical_bytes {
        return Err(CliError::failure(format!(
            "published Loaf output uses {owned_physical_bytes} physical bytes after reclaiming obsolete generations, exceeding its {max_physical_bytes}-byte allowance"
        )));
    }
    phase_timing.envelope_publication_elapsed_ms = envelope_publication_started.elapsed().as_millis();
    let reused_count = 0;
    let report = OvenLoafBakeReport {
        action: "prepared".to_string(),
        envelope: loaf_envelope_name(envelope).to_string(),
        loaf_count: pending.len(),
        prepared_count,
        reused_count,
        logical_bytes,
        physical_bytes,
        owned_physical_bytes,
        raw_disk_bytes,
        // Publication retires every obsolete generation before this report. The active manifest/lock account for
        // owned overhead beyond the referenced Loafs and must not be misreported as reclaimable data.
        reclaimable_physical_bytes: 0,
        active_lease_physical_bytes: 0,
        transient_peak_physical_bytes,
        max_physical_bytes,
        max_domain_physical_bytes,
        max_domain_logical_bytes,
        elapsed_ms: started.elapsed().as_millis(),
        phase_timing,
        cargo_process_started,
        evidence,
        loafs: pending,
        compiler_suite: None,
    };
    // `finish_loaf_bake` opens the committed Loafs as a normal shared-lease consumer. Publication and retirement
    // are complete, so release exclusive authority before crossing into that consumer phase.
    let report = finish_loaf_bake_after_publication(publication_lock, &options, envelope, report, started)?;
    print_loaf_bake_report(&report, options.format)?;
    Ok(ExitCode::SUCCESS)
}

/// Cross from exclusive envelope publication into normal shared-lease consumption.
fn finish_loaf_bake_after_publication(
    publication_lock: crate::oven::loaf::OvenLoafGenerationLock,
    options: &OvenLoafBakeCommandOptions,
    envelope: OvenLoafEnvelope,
    report: OvenLoafBakeReport,
    started: Instant,
) -> CliResult<OvenLoafBakeReport> {
    drop(publication_lock);
    finish_loaf_bake(options, envelope, report, started)
}

/// Complete the typed compiler-suite envelope with its source-plan/foundation store through the same baker.
fn finish_loaf_bake(
    options: &OvenLoafBakeCommandOptions,
    envelope: OvenLoafEnvelope,
    mut report: OvenLoafBakeReport,
    started: Instant,
) -> CliResult<OvenLoafBakeReport> {
    if envelope != OvenLoafEnvelope::CompilerSuite {
        report.elapsed_ms = started.elapsed().as_millis();
        return Ok(report);
    }
    let compiler_suite_preparation_started = Instant::now();
    let suite_store = compiler_suite_store_path(options)?;
    let default_limits = loaf_envelope_default_limits(envelope);
    let max_physical_bytes = options.max_physical_bytes.unwrap_or(default_limits.max_physical_bytes);
    let remaining_physical_bytes = max_physical_bytes
        .checked_sub(report.owned_physical_bytes)
        .filter(|remaining| *remaining > 0)
        .ok_or_else(|| {
            CliError::failure(format!(
                "Loaf envelope already uses {} bytes of its {max_physical_bytes}-byte combined allowance",
                report.owned_physical_bytes
            ))
        })?;
    let max_domain_physical_bytes = options
        .max_domain_physical_bytes
        .unwrap_or(default_limits.max_domain_physical_bytes)
        .min(remaining_physical_bytes);
    let max_domain_logical_bytes = options
        .max_domain_logical_bytes
        .unwrap_or(default_limits.max_domain_logical_bytes);
    let store_options = OvenStoreCommandOptions {
        root: Some(suite_store.clone()),
        max_physical_bytes: Some(remaining_physical_bytes),
        max_domain_physical_bytes: Some(max_domain_physical_bytes),
        max_domain_logical_bytes: Some(max_domain_logical_bytes),
    };
    let (receipt, receipt_path) = compiler_libtests_receipt(
        &options.compiler_root,
        &options.rustc,
        &["lsp".into()],
        Some(&options.output),
    )?;
    write_receipt(&receipt, &receipt_path).map_err(oven_error)?;
    let store = open_store(&store_options)?;
    let prepare = prepare_compiler_test_suite(&OvenLegacyCargoPrepareRequest {
        store: &store,
        receipt,
        generated_project: options.compiler_root.clone(),
        cargo: options.cargo.clone(),
        rustc: options.rustc.clone(),
        sdk_inventory: Some(options.sdk_inventory.clone()),
        compiler_loaf_root: Some(options.output.clone()),
        domain: "compiler-suite-lsp".to_string(),
        publication_kind: OvenLegacyCargoPublicationKind::LibraryTests,
        source_evidence_key: "compiler-libtest-root".to_string(),
        compile_environment: BTreeMap::new(),
        inspection_packages: Some(Vec::new()),
        direct_dependency_closure: OvenLegacyCargoDirectDependencyClosure::CheckedDeclared,
        compact_debug_info: false,
        source_compiler_vocab_support: false,
        base_loaf: None,
    })
    .map_err(oven_error)?;
    let suite_reused = prepare.cargo_version == "not-run-existing-suite";
    let store_inspection = if suite_reused {
        store.inspect_for_exact_reuse()
    } else {
        store.inspect()
    }
    .map_err(oven_error)?;
    let loaf_owned_physical_bytes = report.owned_physical_bytes;
    report.logical_bytes = report.logical_bytes.saturating_add(store_inspection.logical_bytes);
    report.physical_bytes = report.physical_bytes.saturating_add(store_inspection.physical_bytes);
    report.owned_physical_bytes = report
        .owned_physical_bytes
        .saturating_add(store_inspection.physical_bytes);
    report.raw_disk_bytes = report
        .raw_disk_bytes
        .saturating_add(loaf_raw_disk_bytes(&suite_store).map_err(oven_error)?);
    report.reclaimable_physical_bytes = store_inspection.reclaimable_physical_bytes;
    report.active_lease_physical_bytes = store_inspection.active_lease_physical_bytes;
    report.transient_peak_physical_bytes = report
        .transient_peak_physical_bytes
        .max(loaf_owned_physical_bytes.saturating_add(prepare.transient_reservation_bytes));
    report.max_physical_bytes = max_physical_bytes;
    report.max_domain_physical_bytes = options
        .max_domain_physical_bytes
        .unwrap_or(default_limits.max_domain_physical_bytes);
    report.max_domain_logical_bytes = max_domain_logical_bytes;
    if !suite_reused {
        report.action = "prepared".to_string();
        report.cargo_process_started = true;
    }
    if report.owned_physical_bytes > max_physical_bytes {
        return Err(CliError::failure(format!(
            "combined Loaf and compiler-suite storage uses {} physical bytes, exceeding its {max_physical_bytes}-byte allowance",
            report.owned_physical_bytes
        )));
    }
    report.elapsed_ms = started.elapsed().as_millis();
    report.phase_timing.compiler_suite_preparation_elapsed_ms =
        compiler_suite_preparation_started.elapsed().as_millis();
    report.compiler_suite = Some(OvenCompilerSuiteBakeReport {
        receipt: receipt_path,
        prepare,
        store: store_inspection,
    });
    Ok(report)
}

/// Product-owned storage policy for each built-in Loaf envelope.
fn loaf_envelope_default_limits(envelope: OvenLoafEnvelope) -> OvenStoreLimits {
    match envelope {
        OvenLoafEnvelope::Release => OvenStoreLimits::new(
            DEFAULT_OVEN_MAX_PHYSICAL_BYTES,
            DEFAULT_OVEN_MAX_DOMAIN_PHYSICAL_BYTES,
            DEFAULT_OVEN_MAX_DOMAIN_LOGICAL_BYTES,
        ),
        OvenLoafEnvelope::CompilerSuite => OvenStoreLimits::new(
            DEFAULT_OVEN_COMPILER_SUITE_MAX_PHYSICAL_BYTES,
            DEFAULT_OVEN_COMPILER_SUITE_MAX_DOMAIN_PHYSICAL_BYTES,
            DEFAULT_OVEN_COMPILER_SUITE_MAX_DOMAIN_LOGICAL_BYTES,
        ),
    }
}

/// Resolve the compiler-suite store owned by one typed envelope and reject overlapping policy roots.
fn compiler_suite_store_path(options: &OvenLoafBakeCommandOptions) -> CliResult<PathBuf> {
    let suite_store = match &options.suite_store {
        Some(path) => path.clone(),
        None => options
            .output
            .parent()
            .ok_or_else(|| CliError::failure("Loaf output has no parent for its compiler-suite store".to_string()))?
            .join("compiler-suite-store"),
    };
    if suite_store.starts_with(&options.output) || options.output.starts_with(&suite_store) {
        return Err(CliError::failure(
            "compiler-suite store and Loaf output must be separate non-nested bounded roots".to_string(),
        ));
    }
    Ok(suite_store)
}

/// Render one complete baker result without making Make or CI reconstruct product accounting.
fn print_loaf_bake_report(report: &OvenLoafBakeReport, format: OvenOutputFormat) -> CliResult<()> {
    match format {
        OvenOutputFormat::Text => {
            println!(
                "{} complete standard-library Loaf family ({} profile variants; {} logical, {} physical; Cargo baker {}).",
                if report.action == "reused" {
                    "Reused"
                } else {
                    "Prepared"
                },
                report.loaf_count,
                human_bytes(report.logical_bytes),
                human_bytes(report.physical_bytes),
                if report.cargo_process_started {
                    "used"
                } else {
                    "not used"
                },
            );
            if let Some(suite) = &report.compiler_suite {
                println!(
                    "Compiler-suite standard-library family: {} ({} logical, {} physical).",
                    if suite.prepare.cargo_version == "not-run-existing-suite" {
                        "reused"
                    } else {
                        "prepared"
                    },
                    human_bytes(suite.store.logical_bytes),
                    human_bytes(suite.store.physical_bytes),
                );
            }
        }
        OvenOutputFormat::Json => print_json(report)?,
    }
    Ok(())
}

/// Compile and execute every stored compiler workspace native target plan through the Oven runtime suite.
///
/// The caller must have a matching native compiler-suite unit for the same exact source receipt and feature
/// selection. A plan miss fails explicitly rather than silently invoking Cargo or converting a legacy bootstrap into
/// a normal test workflow. An explicit test-only Cargo proxy is restricted to package-qualified interoperability
/// roots and is not available to normal Incan commands.
pub fn oven_run_compiler_libtests(options: OvenCompilerLibtestsRunCommandOptions) -> CliResult<ExitCode> {
    let suite_started = Instant::now();
    let rustc = options.rustc.unwrap_or(resolve_active_rustc().map_err(oven_error)?);
    let compiler_data_root = crate::toolchain_layout::compiler_owned_oven_data_root().ok_or_else(|| {
        CliError::failure(
            "no committed compiler-owned Oven Loaf envelope is available for this compiler suite; run the explicit Loaf baker first",
        )
    })?;
    let loaf_root = compiler_data_root.join("share/incan/oven/loafs");
    let (receipt, receipt_path) =
        compiler_libtests_receipt(&options.compiler_root, &rustc, &options.features, Some(&loaf_root))?;
    let store = open_store_with_defaults(
        &options.store,
        OvenStoreLimits::new(
            DEFAULT_OVEN_COMPILER_SUITE_MAX_PHYSICAL_BYTES,
            DEFAULT_OVEN_COMPILER_SUITE_MAX_DOMAIN_PHYSICAL_BYTES,
            DEFAULT_OVEN_COMPILER_SUITE_MAX_DOMAIN_LOGICAL_BYTES,
        ),
    )?;
    let selected_suite = select_compiler_test_suite(&store, &receipt, &options.compiler_root, &rustc)?;
    let (manifest, artifact_root, payload, suite_lease) = selected_suite.into_parts();
    if manifest.kind != OvenArtifactKind::CompilerTestSuite
        || manifest.build_unit_identity != receipt.build_unit_identity
        || manifest.intent != receipt.intent
    {
        return Err(CliError::failure(
            "selected Oven compiler suite is not authorized by the current compiler receipt".to_string(),
        ));
    }
    let suite = serde_json::from_slice::<OvenCompilerTestSuitePayload>(&payload)
        .map_err(|error| CliError::failure(format!("stored Oven compiler suite payload is invalid: {error}")))?;
    if !matches!(suite.schema_version, 8..=OVEN_COMPILER_TEST_SUITE_SCHEMA_VERSION) {
        return Err(CliError::failure(format!(
            "stored Oven compiler suite payload schema {} is unsupported",
            suite.schema_version
        )));
    }
    if suite.schema_version == 8
        && (!options.targets.is_empty()
            || !options.exact_names.is_empty()
            || options.partition_index.is_some()
            || options.partition_count.is_some())
    {
        return Err(CliError::failure(
            "stored schema-8 compiler suites do not support target, exact-test, or partition selection; republish an indexed Oven suite"
                .to_string(),
        ));
    }
    if options.partition_index.is_some() && suite.schema_version < OVEN_COMPILER_TEST_SUITE_SCHEMA_VERSION {
        return Err(CliError::failure(format!(
            "stored schema-{} compiler suites do not carry digest-verified source footprints for deterministic partitioning; republish the Oven suite",
            suite.schema_version
        )));
    }
    let selected_shard_references = compiler_suite_selected_shard_references(
        &suite.shard_references,
        &options.targets,
        options.partition_index,
        options.partition_count,
    )?;
    let (selected_shard_references, exact_test_names) = compiler_suite_exact_test_selection(
        &options.exact_names,
        selected_shard_references,
        options.partition_index.is_some(),
    )?;
    // Indexed schemas acquire every shard and foundation lease before their first child starts. Keeping both values
    // alive through the complete command prevents policy pruning from removing a later root or its foundation while
    // an earlier compiler test executes.
    let (shard_executions, foundation_executions) = match suite.schema_version {
        8 => {
            if !suite.shard_references.is_empty()
                || !suite.foundation_references.is_empty()
                || suite.cli_artifact_closure.is_some()
            {
                return Err(CliError::failure(
                    "schema-8 Oven compiler suites must use their transitional shared closure without indexed shards or foundations",
                ));
            }
            (Vec::new(), BTreeMap::new())
        }
        9 => {
            if !suite.test_targets.is_empty()
                || !suite.binary_targets.is_empty()
                || suite.test_artifact_closure.is_some()
                || !suite.foundation_references.is_empty()
            {
                return Err(CliError::failure(
                    "schema-9 Oven compiler suite indexes must not retain shared test targets, binary targets, a shared test closure, or foundations",
                ));
            }
            (
                select_compiler_suite_shards(
                    &store,
                    &receipt,
                    &selected_shard_references,
                    OVEN_COMPILER_TEST_SUITE_SHARD_SCHEMA_VERSION_V1,
                )?,
                BTreeMap::new(),
            )
        }
        10 => {
            if !suite.test_targets.is_empty()
                || !suite.binary_targets.is_empty()
                || suite.test_artifact_closure.is_some()
            {
                return Err(CliError::failure(
                    "schema-10 Oven compiler suite indexes must use thin shards and separately admitted foundations",
                ));
            }
            let shards = select_compiler_suite_shards(
                &store,
                &receipt,
                &selected_shard_references,
                OVEN_COMPILER_TEST_SUITE_SHARD_SCHEMA_VERSION_V1,
            )?;
            let foundations = select_compiler_suite_foundations(&store, &receipt, &suite.foundation_references)?;
            (shards, foundations)
        }
        11..=OVEN_COMPILER_TEST_SUITE_SCHEMA_VERSION => {
            if !suite.test_targets.is_empty()
                || !suite.binary_targets.is_empty()
                || suite.test_artifact_closure.is_some()
            {
                return Err(CliError::failure(
                    "schema-11-or-later Oven compiler suite indexes must use thin shards and separately admitted foundations",
                ));
            }
            let shards = select_compiler_suite_shards(
                &store,
                &receipt,
                &selected_shard_references,
                OVEN_COMPILER_TEST_SUITE_SHARD_SCHEMA_VERSION,
            )?;
            let foundations = select_compiler_suite_foundations(&store, &receipt, &suite.foundation_references)?;
            (shards, foundations)
        }
        _ => unreachable!("schema was validated above"),
    };
    let toolchain_data_executions = if suite.schema_version == 13 {
        select_compiler_suite_toolchain_data(&store, &receipt, &suite.toolchain_data_references)?
    } else {
        Vec::new()
    };
    let (external_toolchain_data_root, _external_toolchain_generation) = if suite.schema_version >= 14 {
        let reference = suite.toolchain_loaf_generation.as_ref().ok_or_else(|| {
            CliError::failure(
                "schema-14-or-later Oven compiler-suite index has no compiler Loaf generation reference".to_string(),
            )
        })?;
        if !suite.toolchain_data_references.is_empty() {
            return Err(CliError::failure(
                "schema-14-or-later Oven compiler-suite index must not retain copied Loaf data partitions".to_string(),
            ));
        }
        let data_root = crate::toolchain_layout::compiler_owned_oven_data_root().ok_or_else(|| {
            CliError::failure(
                "schema-14-or-later Oven compiler-suite index requires the committed compiler-owned Loaf envelope selected by its receipt"
                    .to_string(),
            )
        })?;
        let loaf_root = data_root.join("share/incan/oven/loafs");
        let generation = acquire_committed_loaf_generation(&loaf_root)
            .map_err(oven_error)?
            .ok_or_else(|| {
                CliError::failure(
                    "schema-14-or-later Oven compiler-suite index requires a committed compiler-owned Loaf generation"
                        .to_string(),
                )
            })?;
        if generation.generation_identity() != reference.generation_identity {
            return Err(CliError::failure(format!(
                "schema-14-or-later Oven compiler-suite index requires compiler Loaf generation `{}`, but the active toolchain provides `{}`",
                reference.generation_identity,
                generation.generation_identity(),
            )));
        }
        (Some(data_root), Some(generation))
    } else {
        (None, None)
    };
    // Schema 12 derives its compiler-owned generated-code check capability from the workspace-library graph. A
    // focused target need not itself own `incan_stdlib`, so retain the complete receipt-bound shard lease set solely
    // while deriving that shared capability. This does not broaden execution: the prepared-child queue below still
    // contains only `selected_shard_references`. It also avoids a fabricated ambient closure or Cargo recovery path.
    let warning_check_shards = if matches!(suite.schema_version, 12..=OVEN_COMPILER_TEST_SUITE_SCHEMA_VERSION)
        && !options.targets.is_empty()
    {
        Some(select_compiler_suite_shards(
            &store,
            &receipt,
            &suite.shard_references,
            OVEN_COMPILER_TEST_SUITE_SHARD_SCHEMA_VERSION,
        )?)
    } else {
        None
    };
    let warning_check_shards = warning_check_shards.as_deref().unwrap_or(&shard_executions);
    let cli_artifact_closure = match suite.schema_version {
        8 => suite.test_artifact_closure.as_ref().ok_or_else(|| {
            CliError::failure("stored compiler-suite payload has no direct-rustc test closure".to_string())
        })?,
        9..=OVEN_COMPILER_TEST_SUITE_SCHEMA_VERSION => suite.cli_artifact_closure.as_ref().ok_or_else(|| {
            CliError::failure("stored indexed compiler-suite payload has no compiler CLI closure".to_string())
        })?,
        _ => unreachable!("schema was validated above"),
    };
    if suite.schema_version == 8 && suite.test_targets.is_empty() {
        return Err(CliError::failure(
            "stored compiler-suite payload has no direct-rustc native test targets".to_string(),
        ));
    }
    let output_directory = options
        .output
        .unwrap_or_else(|| options.compiler_root.join("target/incan/oven/compiler-tests"));
    fs::create_dir_all(&output_directory).map_err(|error| {
        CliError::failure(format!(
            "cannot create compiler-suite caller output directory {}: {error}",
            output_directory.display()
        ))
    })?;
    let receipt_and_selection_elapsed_ms = suite_started.elapsed().as_millis();
    let shared_setup_started = Instant::now();
    let fixture_cargo = options
        .fixture_cargo
        .as_deref()
        .map(|cargo| prepare_compiler_suite_fixture_cargo_proxy(&output_directory, cargo))
        .transpose()?;
    let stored_sdk_inventory = fs::canonicalize(compiler_suite_file(
        &artifact_root,
        &suite.sdk_inventory_relative_path,
        &suite.sdk_inventory_digest,
        "SDK provider inventory",
    )?)
    .map_err(|error| {
        CliError::failure(format!(
            "cannot canonicalize stored compiler-suite SDK provider inventory: {error}"
        ))
    })?;
    let compiler_data_root = if suite.schema_version >= 14 {
        external_toolchain_data_root
    } else if suite.schema_version == 13 {
        Some(materialize_compiler_suite_toolchain_data(
            &output_directory,
            &toolchain_data_executions,
        )?)
    } else {
        suite
            .toolchain_data_relative_root
            .as_deref()
            .map(|relative_root| compiler_suite_directory(&artifact_root, relative_root, "Loaf data"))
            .transpose()?
    };
    // A suite's shards commonly share most of their direct workspace DAG. Keep one invocation-scoped, recipe-bound
    // map so a full test run does not rebuild (or retain) the same compiler libraries once per test root.
    let mut workspace_library_cache = BTreeMap::new();
    let mut binary_cache = BTreeMap::new();
    let warning_check_artifacts = if matches!(suite.schema_version, 12..=OVEN_COMPILER_TEST_SUITE_SCHEMA_VERSION) {
        bake_compiler_suite_warning_check_artifacts(
            warning_check_shards,
            &receipt,
            &rustc,
            &options.compiler_root,
            &output_directory,
            &foundation_executions,
            &mut workspace_library_cache,
        )?
    } else {
        suite
            .warning_check_artifacts
            .materialize(&artifact_root, &manifest.intent)
            .map_err(|error| {
                CliError::failure(format!(
                    "stored compiler-suite generated-code closure is invalid: {error}"
                ))
            })?
    };
    let cli_target = suite.cli_target.as_ref().ok_or_else(|| {
        CliError::failure("stored compiler-suite payload has no direct-rustc compiler CLI target".to_string())
    })?;
    if cli_target.runner != "rustc-run" {
        return Err(CliError::failure(
            "stored compiler-suite CLI target must use the direct-rustc run executor".to_string(),
        ));
    }
    if suite.schema_version < 11
        && (!suite.cli_workspace_libraries.is_empty()
            || !suite.cli_foundation_references.is_empty()
            || !cli_target.workspace_library_dependencies.is_empty())
    {
        return Err(CliError::failure(
            "schema-10-or-earlier Oven compiler suite declares workspace-library edges that its stored schema cannot execute",
        ));
    }
    let cli_artifacts = cli_artifact_closure.manifest_for_target(cli_target, manifest.intent.clone());
    let cli_workspace_library_outputs = if suite.schema_version >= 11 {
        bake_planned_compiler_suite_workspace_libraries(
            &suite.cli_workspace_libraries,
            cli_artifact_closure,
            &manifest.intent,
            &receipt,
            &artifact_root,
            &rustc,
            &options.compiler_root,
            &output_directory,
            &suite.cli_foundation_references,
            Some(&foundation_executions),
            &mut workspace_library_cache,
        )?
    } else {
        BTreeMap::new()
    };
    let mut cli_artifact_plan = if suite.schema_version >= 11 {
        compiler_suite_composed_artifact_plan(
            &cli_artifacts,
            &suite.cli_foundation_references,
            &foundation_executions,
            &manifest.intent,
        )?
    } else {
        cli_artifacts
            .materialize_trusted_store(&artifact_root, &manifest.intent)
            .map_err(oven_error)?
    };
    attach_compiler_suite_target_workspace_libraries(
        &mut cli_artifact_plan,
        cli_target,
        &suite.cli_workspace_libraries,
        &cli_workspace_library_outputs,
    )?;
    let cli_source = compiler_suite_target_source(&options.compiler_root, cli_target)?;
    let cli_output = compiler_suite_cli_output(&output_directory);
    let cli_bake = bake_trusted_direct_rustc_run(&OvenTrustedDirectRustcTargetRequest {
        receipt: &receipt,
        artifacts: &cli_artifacts,
        artifact_root: &artifact_root,
        artifact_plan: Some(&cli_artifact_plan),
        rustc: &rustc,
        source: &cli_source,
        output: &cli_output,
        crate_name: &cli_target.crate_name,
        edition: &cli_target.edition,
        source_evidence_key: &cli_target.source_evidence_key,
        features: &cli_target.features,
        prefer_dynamic: compiler_suite_workspace_outputs_include_dylib(&cli_workspace_library_outputs),
    })
    .map_err(oven_error)?;
    let mut environment = compiler_suite_environment_with_vocab(
        &options.compiler_root,
        &stored_sdk_inventory,
        &rustc,
        &warning_check_artifacts,
        &cli_artifact_plan,
        compiler_data_root.as_deref(),
        &output_directory,
    )?;
    // `rust-analyzer` gives each fixture workspace a short-lived lockfile copy below `TMPDIR`. A caller can invoke
    // the suite from a deeply nested worktree, where forwarding that path makes otherwise-valid Cargo metadata
    // inspection fail before Oven has a chance to execute the stored root. Keep this mutable scratch directory
    // invocation-owned but deliberately short; all durable suite output remains under the caller-selected output.
    let suite_temporary_directory = compiler_suite_temporary_directory()?;
    environment.insert(
        "TMPDIR".to_string(),
        suite_temporary_directory.path().display().to_string(),
    );
    environment.insert(
        "CARGO_BIN_EXE_incan".to_string(),
        compiler_suite_environment_path(&cli_bake.output)?.display().to_string(),
    );
    let shared_setup_elapsed_ms = shared_setup_started.elapsed().as_millis();
    let run_children = || -> CliResult<(CompilerSuiteChildrenReport, usize, usize, CompilerSuiteChildPhaseTimings)> {
        if suite.schema_version == 8 {
            let test_artifact_closure = suite.test_artifact_closure.as_ref().ok_or_else(|| {
                CliError::failure("stored compiler-suite payload has no direct-rustc test closure".to_string())
            })?;
            let binary_outputs = bake_planned_compiler_suite_binaries(
                &suite.binary_targets,
                test_artifact_closure,
                &manifest.intent,
                &receipt,
                &artifact_root,
                &rustc,
                &options.compiler_root,
                &output_directory,
                &cli_bake.output,
                &[],
                &BTreeMap::new(),
                &[],
                None,
                &mut binary_cache,
            )?;
            let root_execution_started = Instant::now();
            let suite_report = run_planned_compiler_suite_children(
                &suite.test_targets,
                test_artifact_closure,
                &manifest.intent,
                &receipt,
                &artifact_root,
                &rustc,
                &options.compiler_root,
                &output_directory,
                &environment,
                &binary_outputs,
                &[],
                &BTreeMap::new(),
                &[],
                None,
                fixture_cargo.as_ref(),
            )?;
            Ok((
                suite_report,
                suite.test_targets.len(),
                suite.binary_targets.len(),
                CompilerSuiteChildPhaseTimings {
                    target_preparation_elapsed_ms: 0,
                    root_execution_elapsed_ms: root_execution_started.elapsed().as_millis(),
                },
            ))
        } else {
            let target_preparation_started = Instant::now();
            let mut prepared_children = Vec::with_capacity(shard_executions.len());
            let mut planned_binary_count = 0;
            for (index, shard) in shard_executions.iter().enumerate() {
                let shard_output = output_directory.join("shards").join(format!("{index:04}"));
                if suite.schema_version < 11
                    && (!shard.payload.workspace_libraries.is_empty()
                        || !shard.payload.target.workspace_library_dependencies.is_empty()
                        || shard
                            .payload
                            .binary_targets
                            .iter()
                            .any(|target| !target.workspace_library_dependencies.is_empty()))
                {
                    return Err(CliError::failure(
                        "schema-10-or-earlier Oven compiler suite shard declares workspace-library edges that its stored schema cannot execute",
                    ));
                }
                let foundations = compiler_suite_uses_indexed_foundations(suite.schema_version)
                    .then_some(&foundation_executions);
                let foundation_references = if compiler_suite_uses_indexed_foundations(suite.schema_version) {
                    shard.payload.foundation_references.as_slice()
                } else {
                    &[]
                };
                let workspace_library_outputs = if suite.schema_version >= 11 {
                    bake_planned_compiler_suite_workspace_libraries(
                        &shard.payload.workspace_libraries,
                        &shard.payload.artifact_closure,
                        &shard.stored.manifest.intent,
                        &receipt,
                        &shard.stored.artifact_root,
                        &rustc,
                        &options.compiler_root,
                        &shard_output,
                        foundation_references,
                        foundations,
                        &mut workspace_library_cache,
                    )?
                } else {
                    BTreeMap::new()
                };
                let binary_outputs = bake_planned_compiler_suite_binaries(
                    &shard.payload.binary_targets,
                    &shard.payload.artifact_closure,
                    &shard.stored.manifest.intent,
                    &receipt,
                    &shard.stored.artifact_root,
                    &rustc,
                    &options.compiler_root,
                    &shard_output,
                    &cli_bake.output,
                    &shard.payload.workspace_libraries,
                    &workspace_library_outputs,
                    foundation_references,
                    foundations,
                    &mut binary_cache,
                )?;
                prepared_children.push(prepare_compiler_suite_child(
                    &shard.payload.target,
                    &shard.payload.artifact_closure,
                    &shard.stored.manifest.intent,
                    &shard.stored.artifact_root,
                    &rustc,
                    &options.compiler_root,
                    &shard_output,
                    &environment,
                    &binary_outputs,
                    &shard.payload.workspace_libraries,
                    workspace_library_outputs,
                    foundation_references,
                    foundations,
                    fixture_cargo.as_ref(),
                )?);
                planned_binary_count += shard.payload.binary_targets.len();
            }
            let target_preparation_elapsed_ms = target_preparation_started.elapsed().as_millis();
            let root_execution_started = Instant::now();
            let suite_report = run_prepared_compiler_suite_children(
                prepared_children,
                &receipt,
                &rustc,
                exact_test_names.as_deref(),
            )?;
            Ok((
                suite_report,
                shard_executions.len(),
                planned_binary_count,
                CompilerSuiteChildPhaseTimings {
                    target_preparation_elapsed_ms,
                    root_execution_elapsed_ms: root_execution_started.elapsed().as_millis(),
                },
            ))
        }
    };
    let (mut suite_report, planned_target_count, planned_binary_count, child_phase_timings) =
        run_compiler_suite_children_with_leases_retained(
            &suite_lease,
            &shard_executions,
            &foundation_executions,
            &toolchain_data_executions,
            run_children,
        )?;
    let completion_failures = compiler_suite_completion_failures(&suite_report, planned_target_count);
    suite_report.failed.extend(completion_failures);
    let native_test_case_totals = suite_report.native_test_case_totals();
    let slowest_native_test_cases = suite_report.slowest_native_test_cases(25);
    let fixture_cargo_invocations = fixture_cargo
        .as_ref()
        .map(CompilerSuiteFixtureCargoProxy::invocation_count)
        .transpose()?
        .unwrap_or(0);
    let fixture_cargo_roots = suite_report
        .native_test_roots
        .iter()
        .filter(|root| {
            OvenCompilerSuiteTargetCapabilities::for_target(
                &root.package_name,
                &root.target_kind,
                &root.source_relative_path,
            )
            .cargo_fixture
        })
        .map(|root| {
            format!(
                "{}:{}:{}",
                root.package_name, root.target_kind, root.source_relative_path
            )
        })
        .collect::<Vec<_>>();
    let success = suite_report.failed.is_empty()
        && native_test_case_totals.unreported_roots == 0
        && native_test_case_totals.reported_roots + suite_report.doctest_targets == planned_target_count;
    let selection = compiler_suite_selection_report(
        exact_test_names.as_deref(),
        &options.targets,
        options.partition_index,
        options.partition_count,
        planned_target_count,
    );
    let complete_root_success = success && selection.complete_root_evidence;
    let complete_suite_success = success && selection.complete_suite_evidence;
    let timing = CompilerSuiteTimingReport {
        receipt_and_selection_elapsed_ms,
        shared_setup_elapsed_ms,
        target_preparation_elapsed_ms: child_phase_timings.target_preparation_elapsed_ms,
        root_execution_elapsed_ms: child_phase_timings.root_execution_elapsed_ms,
        total_elapsed_ms: suite_started.elapsed().as_millis(),
    };
    let store_inspection = store.inspect().map_err(oven_error)?;
    let report_path = output_directory.join("compiler-suite-report.json");
    let report = serde_json::json!({
        "success": success,
        "complete_root_success": complete_root_success,
        "complete_suite_success": complete_suite_success,
        "selection": selection.clone(),
        "receipt": receipt_path.display().to_string(),
        "report_path": report_path.display().to_string(),
        "native_test_count": suite_report.native_test_count,
        "native_test_case_totals": native_test_case_totals.clone(),
        "native_test_roots": suite_report.native_test_roots.clone(),
        "slowest_native_test_cases": slowest_native_test_cases,
        "rustdoc_test_roots": suite_report.rustdoc_test_roots.clone(),
        "doctest_targets": suite_report.doctest_targets,
        "test_targets": planned_target_count,
        "binary_targets": planned_binary_count,
        "suite_schema_version": suite.schema_version,
        "shard_count": shard_executions.len(),
        "compiler_cli_reused": cli_bake.reused,
        "cargo_process_started": false,
        "fixture_cargo": {
            "configured": fixture_cargo.is_some(),
            "invocations": fixture_cargo_invocations,
            "roots": fixture_cargo_roots,
        },
        "timing": timing,
        "store": store_inspection,
        "failures": suite_report.failed.clone(),
    });
    write_compiler_suite_report(&report_path, &report)?;
    if !success {
        if matches!(options.format, OvenOutputFormat::Json) {
            print_json(&report)?;
            return Ok(ExitCode::FAILURE);
        }
        let selection_context = match selection.mode {
            "complete-suite" => "complete compiler workspace native suite",
            "selected-complete-roots" => "selected complete-root replay (not complete-suite evidence)",
            _ => "partial exact diagnostic (not complete-root or complete-suite evidence)",
        };
        return Err(CliError::failure(format!(
            "Oven {selection_context} failed: {} passed, {} failed, {} ignored across {} reported libtest root(s): {} green, {} failing, with {} root(s) lacking a terminal libtest summary. The explicit compatibility fixture launched Cargo {} time(s).\n{}",
            native_test_case_totals.passed,
            native_test_case_totals.failed,
            native_test_case_totals.ignored,
            native_test_case_totals.reported_roots,
            native_test_case_totals.green_roots,
            native_test_case_totals.failed_roots,
            native_test_case_totals.unreported_roots,
            fixture_cargo_invocations,
            suite_report.failed.join("\n\n")
        )));
    }
    match options.format {
        OvenOutputFormat::Text if selection.complete_suite_evidence => println!(
            "Oven executed the complete compiler workspace suite: {} native test(s), with {} passed, {} failed, and {} ignored across {} reported root(s): {} green and {} failing, plus {} doctest target(s), through its stored direct-Rustc target plan. The explicit compatibility fixture launched Cargo {} time(s) (receipt {}).",
            suite_report.native_test_count,
            native_test_case_totals.passed,
            native_test_case_totals.failed,
            native_test_case_totals.ignored,
            native_test_case_totals.reported_roots,
            native_test_case_totals.green_roots,
            native_test_case_totals.failed_roots,
            suite_report.doctest_targets,
            fixture_cargo_invocations,
            receipt_path.display(),
        ),
        OvenOutputFormat::Text if selection.complete_root_evidence => println!(
            "Oven executed {} selected complete compiler-suite root(s): {} native test(s), with {} passed, {} failed, and {} ignored across {} reported root(s): {} green and {} failing, plus {} doctest target(s), through stored direct-Rustc target plans. Selection: {}. This is complete-root evidence for the selected roots, not complete compiler workspace suite evidence. The explicit compatibility fixture launched Cargo {} time(s) (receipt {}).",
            selection.selected_root_count,
            suite_report.native_test_count,
            native_test_case_totals.passed,
            native_test_case_totals.failed,
            native_test_case_totals.ignored,
            native_test_case_totals.reported_roots,
            native_test_case_totals.green_roots,
            native_test_case_totals.failed_roots,
            suite_report.doctest_targets,
            compiler_suite_selection_context(&selection),
            fixture_cargo_invocations,
            receipt_path.display(),
        ),
        OvenOutputFormat::Text => println!(
            "Oven exact diagnostic executed {} selected native test(s): {} passed, {} failed, and {} ignored from one receipt-bound root through its stored direct-Rustc target plan. Selected names: {}. This partial diagnostic is neither complete-root nor complete-suite evidence. The explicit compatibility fixture launched Cargo {} time(s) (receipt {}).",
            selection.selected_case_count,
            native_test_case_totals.passed,
            native_test_case_totals.failed,
            native_test_case_totals.ignored,
            selection.normalized_exact_names.join(", "),
            fixture_cargo_invocations,
            receipt_path.display(),
        ),
        OvenOutputFormat::Json => print_json(&report)?,
    }
    Ok(ExitCode::SUCCESS)
}

/// Pin compiler-suite fixture children to the source checkout authorized by the current receipt.
///
/// Tests invoke the stored CLI to prepare local fixture providers. Inheriting `INCAN_SOURCE_ROOT` from an unrelated
/// developer checkout silently mixes compiler sources and makes an otherwise valid suite non-reproducible.
#[cfg(test)]
fn compiler_suite_environment(
    compiler_root: &Path,
    sdk_inventory: &Path,
    rustc: &Path,
    warning_check_artifacts: &crate::oven::rustc::OvenRustcArtifactPlan,
    compiler_data_root: Option<&Path>,
    output_directory: &Path,
) -> CliResult<BTreeMap<String, String>> {
    compiler_suite_environment_with_vocab(
        compiler_root,
        sdk_inventory,
        rustc,
        warning_check_artifacts,
        warning_check_artifacts,
        compiler_data_root,
        output_directory,
    )
}

/// Construct fixture-child state with distinct direct-Rustc closures for generated code and vocab extraction.
fn compiler_suite_environment_with_vocab(
    compiler_root: &Path,
    sdk_inventory: &Path,
    rustc: &Path,
    warning_check_artifacts: &crate::oven::rustc::OvenRustcArtifactPlan,
    vocab_extraction_artifacts: &crate::oven::rustc::OvenRustcArtifactPlan,
    compiler_data_root: Option<&Path>,
    output_directory: &Path,
) -> CliResult<BTreeMap<String, String>> {
    let compiler_root = fs::canonicalize(compiler_root).map_err(|error| {
        CliError::failure(format!(
            "cannot canonicalize compiler-suite root {}: {error}",
            compiler_root.display()
        ))
    })?;
    let stdlib_root = compiler_root.join("crates/incan_stdlib/stdlib");
    if !stdlib_root.is_dir() {
        return Err(CliError::failure(format!(
            "compiler-suite root {} has no stdlib directory {}",
            compiler_root.display(),
            stdlib_root.display()
        )));
    }
    let sdk_inventory = fs::canonicalize(sdk_inventory).map_err(|error| {
        CliError::failure(format!(
            "cannot canonicalize compiler-suite SDK provider inventory {}: {error}",
            sdk_inventory.display()
        ))
    })?;
    if !sdk_inventory.is_file() {
        return Err(CliError::failure(format!(
            "compiler-suite SDK provider inventory {} is not a regular file",
            sdk_inventory.display()
        )));
    }
    let sdk_provider_root = sdk_inventory.parent().ok_or_else(|| {
        CliError::failure(format!(
            "compiler-suite SDK provider inventory {} has no provider root",
            sdk_inventory.display()
        ))
    })?;
    let output_directory = compiler_suite_environment_path(output_directory)?;
    let compiler_data_root = compiler_data_root.map(compiler_suite_environment_path).transpose()?;
    let runtime_root = sdk_inventory
        .parent()
        .map(|parent| parent.join("runtime"))
        .filter(|root| root.join("Cargo.lock").is_file())
        .ok_or_else(|| {
            CliError::failure(format!(
                "compiler-suite SDK provider inventory {} has no sealed runtime closure",
                sdk_inventory.display()
            ))
        })?;
    let stdlib_extern = warning_check_artifacts
        .externs
        .iter()
        .find_map(|(crate_name, path)| (crate_name == "incan_stdlib").then_some(path))
        .ok_or_else(|| {
            CliError::failure(
                "stored compiler-suite direct-rustc closure has no `incan_stdlib` extern for generated-code checks",
            )
        })?;
    let stdlib_extern = compiler_suite_environment_path(stdlib_extern)?;
    let rustup_home = default_rustup_home(env::var_os("RUSTUP_HOME"), user_home());
    // Store identities contain `sha256:`. On Unix, `:` is the path-list separator, so joining these verified
    // absolute paths into one environment variable would either be ambiguous or rejected. Transport each opaque
    // direct-rustc search path in its own environment value instead.
    let mut environment = BTreeMap::from([
        // Insta otherwise discovers its workspace by launching `cargo metadata`. The receipt already authorizes
        // this canonical compiler root, so make snapshot resolution direct and Cargo-free for stored suite children.
        ("INSTA_WORKSPACE_ROOT".to_string(), compiler_root.display().to_string()),
        ("INCAN_SOURCE_ROOT".to_string(), compiler_root.display().to_string()),
        ("INCAN_STDLIB".to_string(), stdlib_root.display().to_string()),
        ("INCAN_SDK_INVENTORY".to_string(), sdk_inventory.display().to_string()),
        (
            "INCAN_INTERNAL_SDK_PROVIDER_STORE".to_string(),
            sdk_provider_root.display().to_string(),
        ),
        // Provider discovery may intentionally clear INCAN_SDK_INVENTORY in a cold-store fixture. Keep runtime
        // source identity separately receipt-bound, so that action cannot make the same native closure appear to
        // belong to an ambient checkout.
        (
            "INCAN_INTERNAL_OVEN_RUNTIME_ROOT".to_string(),
            runtime_root.display().to_string(),
        ),
        // Stored-suite children may need a managed Oven store, but that state belongs to the suite invocation. Never
        // allow a scheduler run to silently write under the developer's default `~/.incan` home.
        (
            "INCAN_HOME".to_string(),
            output_directory.join("incan-home").display().to_string(),
        ),
        // A fixture may intentionally clear INCAN_HOME while testing the default command path. Keep that default
        // inside the scheduler-owned output as well: inherited developer HOME state must neither make a stored suite
        // non-reproducible nor send normal nested Oven commands outside the active invocation's bounded policy.
        ("HOME".to_string(), output_directory.join("home").display().to_string()),
        // The scheduler has already validated this exact direct-rustc executable. Propagate it explicitly so the
        // isolated suite home cannot make a nested normal command ask rustup to rediscover a developer toolchain.
        // This selects the compiler; it does not authorize Cargo.
        (
            "RUSTC".to_string(),
            compiler_suite_environment_path(rustc)?.display().to_string(),
        ),
        // Some compiler self-tests deliberately clear RUSTC to exercise Rustup's compiler discovery. Keep only the
        // parent toolchain-manager state available for that test behavior; normal child commands use RUSTC above,
        // and this does not expose or authorize Cargo state.
        (
            "RUSTUP_HOME".to_string(),
            rustup_home.map_or_else(String::new, |root| root.display().to_string()),
        ),
        // The parent scheduler derives this only from its active installed toolchain. An empty value deliberately
        // clears any inherited override when the parent is a development binary without package data.
        (
            "INCAN_INTERNAL_TOOLCHAIN_DATA_ROOT".to_string(),
            compiler_data_root
                .as_ref()
                .map_or_else(String::new, |root| root.display().to_string()),
        ),
        // This is an internal scheduler capability, not a user-selectable artifact path. It authorizes nested normal
        // commands to consume the parent-leased, read-only Loaf directly instead of copying its closure into
        // every fixture's small mutable Oven home.
        ("INCAN_INTERNAL_OVEN_LOAF_EXECUTION".to_string(), "1".to_string()),
        (OVEN_COMPILER_SUITE_RUSTC_ENV.to_string(), rustc.display().to_string()),
    ]);
    let warning_capability = OvenCompilerSuiteCapability::new(
        compiler_suite_environment_path(rustc)?,
        warning_check_artifacts
            .dependency_search_paths
            .iter()
            .map(|path| compiler_suite_environment_path(path))
            .collect::<CliResult<Vec<_>>>()?,
        warning_check_artifacts
            .externs
            .iter()
            .map(|(crate_name, path)| Ok((crate_name.clone(), compiler_suite_environment_path(path)?)))
            .collect::<CliResult<BTreeMap<_, _>>>()?,
    );
    if warning_capability.externs.get("incan_stdlib") != Some(&compiler_suite_environment_path(&stdlib_extern)?) {
        return Err(CliError::failure(
            "compiler-suite warning capability does not bind its selected incan_stdlib artifact",
        ));
    }
    environment.insert(
        OVEN_COMPILER_SUITE_CAPABILITY_ENV.to_string(),
        warning_capability.encode().map_err(CliError::failure)?,
    );
    // The suite-built CLI is a caller-owned direct-Rustc output. A root can launch that CLI through
    // `CARGO_BIN_EXE_incan` even when the root itself has no dynamic workspace-library dependency, so exporting
    // this receipt-selected loader path only for `prefer_dynamic` roots leaves those nested launches broken.
    // Keep the toolchain path in the base child environment: native test execution and every child it starts then
    // inherit the exact selected standard-library closure without trusting a Cargo-provided loader environment.
    let (dynamic_library_environment_name, dynamic_library_environment_value) =
        rustc_dynamic_library_environment(rustc).map_err(oven_error)?;
    environment.insert(dynamic_library_environment_name, dynamic_library_environment_value);
    append_compiler_suite_direct_rustc_environment(
        &mut environment,
        OVEN_COMPILER_SUITE_VOCAB_CAPABILITY_ENV,
        rustc,
        vocab_extraction_artifacts,
    )?;
    Ok(environment)
}

/// Create short-lived scratch space for compiler-suite fixture tools.
///
/// The v0.5 suite runs on Unix hosts. `/tmp` keeps rust-analyzer's nested Cargo lockfile paths below platform limits
/// even when the caller's worktree or selected suite output has a long absolute path. The guard object remains live
/// for the entire scheduler invocation, so children cannot observe a reclaimed directory.
#[cfg(unix)]
fn compiler_suite_temporary_directory() -> CliResult<LoafTemporaryDirectory> {
    LoafTemporaryDirectory::create(Path::new("/tmp"), ".incan-oven-suite-").map_err(|error| {
        CliError::failure(format!(
            "cannot create short compiler-suite temporary directory: {error}"
        ))
    })
}

/// Create invocation-owned scratch space on platforms outside the supported v0.5 compiler-suite hosts.
#[cfg(not(unix))]
fn compiler_suite_temporary_directory() -> CliResult<LoafTemporaryDirectory> {
    LoafTemporaryDirectory::create(&env::temp_dir(), ".incan-oven-suite-")
        .map_err(|error| CliError::failure(format!("cannot create compiler-suite temporary directory: {error}")))
}

/// Convert a scheduler-selected path into an absolute environment value before a test changes directory.
///
/// Stored suite entries may be selected from a relative `--store` path, while compiler tests deliberately create
/// nested fixture directories. Forwarding those paths verbatim would make a receipt-authorized inventory or native
/// Loaf disappear from a nested `incan` process despite still being held by the parent suite lease.
fn compiler_suite_environment_path(path: &Path) -> CliResult<PathBuf> {
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }
    env::current_dir()
        .map(|directory| directory.join(path))
        .map_err(|error| {
            CliError::failure(format!(
                "cannot resolve compiler-suite environment path {} from the current directory: {error}",
                path.display()
            ))
        })
}

/// Export a second, named direct-Rustc closure for compiler-internal vocab companion extraction.
///
/// The generated-code warning checker and the compiler CLI can legitimately use different dependency closures. Keep
/// them separate instead of merging same-named Rust crates from independent receipt-bound foundations into an
/// ambiguous `--extern` list.
fn append_compiler_suite_direct_rustc_environment(
    environment: &mut BTreeMap<String, String>,
    variable: &str,
    rustc: &Path,
    artifacts: &crate::oven::rustc::OvenRustcArtifactPlan,
) -> CliResult<()> {
    let capability = OvenCompilerSuiteCapability::new(
        compiler_suite_environment_path(rustc)?,
        artifacts
            .dependency_search_paths
            .iter()
            .map(|path| compiler_suite_environment_path(path))
            .collect::<CliResult<Vec<_>>>()?,
        artifacts
            .externs
            .iter()
            .map(|(crate_name, path)| Ok((crate_name.clone(), compiler_suite_environment_path(path)?)))
            .collect::<CliResult<BTreeMap<_, _>>>()?,
    );
    environment.insert(variable.to_string(), capability.encode().map_err(CliError::failure)?);
    Ok(())
}

/// Place the suite-built CLI beside its caller-owned workspace libraries.
///
/// Direct CLI builds can prefer dynamic workspace libraries. A fixed compiler-root target path would outlive the
/// caller output and become unusable when the scheduler responsibly reclaims an interrupted run's artifacts.
fn compiler_suite_cli_output(output_directory: &Path) -> PathBuf {
    output_directory.join("compiler-cli/incan")
}

/// Select one workspace library and only the declared workspace libraries it transitively requires.
///
/// A compiler-suite shard can contain the full workspace DAG needed by its test root, while a generated-code warning
/// check consumes only `incan_stdlib`. Rebuilding unrelated compiler, LSP, or inspection crates merely to obtain
/// that one checked input repeats expensive work without strengthening the warning-check contract.
fn compiler_suite_workspace_library_dependency_closure(
    libraries: &[OvenCompilerWorkspaceLibrary],
    root: &OvenCompilerWorkspaceLibraryKey,
) -> CliResult<Vec<OvenCompilerWorkspaceLibrary>> {
    let mut declared = BTreeMap::new();
    for library in libraries {
        if declared.insert(library.key.clone(), library).is_some() {
            return Err(CliError::failure(format!(
                "stored compiler-suite workspace library `{}` is declared more than once",
                library.key.crate_name
            )));
        }
    }
    let mut selected = BTreeSet::new();
    let mut pending = vec![root.clone()];
    while let Some(key) = pending.pop() {
        if !selected.insert(key.clone()) {
            continue;
        }
        let library = declared.get(&key).ok_or_else(|| {
            CliError::failure(format!(
                "stored compiler-suite workspace library closure is missing `{}`",
                key.crate_name
            ))
        })?;
        pending.extend(library.dependencies.iter().cloned());
    }
    Ok(declared
        .into_iter()
        .filter_map(|(key, library)| selected.contains(&key).then(|| library.clone()))
        .collect())
}

/// Rebuild the generated-code warning check's `incan_stdlib` input from a receipt-bound workspace-library shard.
///
/// Schema 12 replaces the former second Cargo target with this caller-owned direct-Rustc bake. The selected shard
/// and every foundation remain leased for the complete suite command, so this plan cannot fall back to a Cargo
/// target or an ambient compiler cache after publication.
fn bake_compiler_suite_warning_check_artifacts(
    shards: &[CompilerSuiteShardExecution],
    receipt: &OvenReceipt,
    rustc: &Path,
    compiler_root: &Path,
    output_directory: &Path,
    foundations: &BTreeMap<String, CompilerSuiteFoundationExecution>,
    workspace_library_cache: &mut BTreeMap<String, OvenCallerOwnedRustcLibrary>,
) -> CliResult<OvenRustcArtifactPlan> {
    let mut selected: Option<(&CompilerSuiteShardExecution, &OvenCompilerWorkspaceLibrary)> = None;
    for shard in shards {
        for library in &shard.payload.workspace_libraries {
            if library.key.package_name != "incan_stdlib"
                || library.key.crate_name != "incan_stdlib"
                || library.key.target_kind != "lib"
                || library.key.source_relative_path != "crates/incan_stdlib/src/lib.rs"
            {
                continue;
            }
            match selected {
                Some((_, previous)) if previous.key != library.key => {
                    return Err(CliError::failure(
                        "schema-12 compiler-suite shards disagree on the source or feature identity of their direct-Rustc `incan_stdlib` warning-check plan",
                    ));
                }
                Some(_) => {}
                None => selected = Some((shard, library)),
            }
        }
    }
    let (shard, library) = selected.ok_or_else(|| {
        CliError::failure(
            "schema-12 compiler suite has no receipt-bound `incan_stdlib` workspace library for generated-code checks",
        )
    })?;
    let warning_check_libraries =
        compiler_suite_workspace_library_dependency_closure(&shard.payload.workspace_libraries, &library.key)?;
    let workspace_outputs = bake_planned_compiler_suite_workspace_libraries(
        &warning_check_libraries,
        &shard.payload.artifact_closure,
        &shard.stored.manifest.intent,
        receipt,
        &shard.stored.artifact_root,
        rustc,
        compiler_root,
        &output_directory.join("warning-check"),
        &shard.payload.foundation_references,
        Some(foundations),
        workspace_library_cache,
    )?;
    let artifacts = shard
        .payload
        .artifact_closure
        .manifest_for_workspace_library(library, shard.stored.manifest.intent.clone());
    let mut artifact_plan = compiler_suite_composed_artifact_plan(
        &artifacts,
        &shard.payload.foundation_references,
        foundations,
        &shard.stored.manifest.intent,
    )?;
    let dependencies = library
        .dependencies
        .iter()
        .map(|dependency| {
            workspace_outputs.get(dependency).cloned().ok_or_else(|| {
                CliError::failure(format!(
                    "schema-12 generated-code warning check requires missing workspace library `{}`",
                    dependency.crate_name
                ))
            })
        })
        .collect::<CliResult<Vec<_>>>()?;
    attach_caller_owned_rustc_libraries(&mut artifact_plan, &dependencies).map_err(oven_error)?;
    let stdlib = workspace_outputs.get(&library.key).cloned().ok_or_else(|| {
        CliError::failure(
            "schema-12 generated-code warning check did not materialize its `incan_stdlib` workspace library",
        )
    })?;
    attach_caller_owned_rustc_libraries(&mut artifact_plan, std::slice::from_ref(&stdlib)).map_err(oven_error)?;
    Ok(artifact_plan)
}

/// Compile every receipt-bound workspace binary that Cargo supplied to integration targets through
/// `CARGO_BIN_EXE_*`. The outputs are caller-owned and are injected only into the declared direct-rustc targets;
/// no Cargo-produced binary is retained or executed from the immutable Oven entry.
#[allow(clippy::too_many_arguments)]
fn bake_planned_compiler_suite_binaries(
    targets: &[crate::oven::legacy_cargo::OvenCompilerTestSuiteTarget],
    closure: &crate::oven::legacy_cargo::OvenCompilerTestSuiteArtifactClosure,
    intent: &OvenBuildIntent,
    receipt: &OvenReceipt,
    artifact_root: &Path,
    rustc: &Path,
    compiler_root: &Path,
    output_directory: &Path,
    cli_output: &Path,
    workspace_libraries: &[OvenCompilerWorkspaceLibrary],
    workspace_library_outputs: &BTreeMap<OvenCompilerWorkspaceLibraryKey, OvenCallerOwnedRustcLibrary>,
    foundation_references: &[OvenCompilerTestSuiteFoundationReference],
    foundations: Option<&BTreeMap<String, CompilerSuiteFoundationExecution>>,
    binary_cache: &mut BTreeMap<String, PathBuf>,
) -> CliResult<BTreeMap<String, PathBuf>> {
    let mut outputs = BTreeMap::from([("incan".to_string(), cli_output.to_path_buf())]);
    if targets.is_empty() {
        return Ok(outputs);
    }
    let closure_identity = compiler_suite_artifact_closure_cache_identity(closure)?;
    for (index, target) in targets.iter().enumerate() {
        if target.runner != "rustc-run" || target.target_kind != "bin" {
            return Err(CliError::failure(format!(
                "stored compiler-suite binary target `{}` must use the direct-rustc binary executor",
                target.target_name
            )));
        }
        if target.target_name == "incan" {
            return Err(CliError::failure(
                "stored compiler-suite binary targets must not duplicate the dedicated `incan` CLI target".to_string(),
            ));
        }
        let cache_key = compiler_suite_target_cache_key(
            receipt,
            target,
            &closure_identity,
            foundation_references,
            workspace_library_outputs,
        )?;
        if let Some(cached) = binary_cache.get(&cache_key) {
            if outputs.insert(target.target_name.clone(), cached.clone()).is_some() {
                return Err(CliError::failure(format!(
                    "stored compiler-suite binary target `{}` is declared more than once",
                    target.target_name
                )));
            }
            continue;
        }
        let artifacts = closure.manifest_for_target(target, intent.clone());
        let mut artifact_plan = match foundations {
            Some(foundations) => {
                compiler_suite_composed_artifact_plan(&artifacts, foundation_references, foundations, intent)?
            }
            None => artifacts
                .materialize_trusted_store(artifact_root, intent)
                .map_err(oven_error)?,
        };
        attach_compiler_suite_target_workspace_libraries(
            &mut artifact_plan,
            target,
            workspace_libraries,
            workspace_library_outputs,
        )?;
        let source = compiler_suite_target_source(compiler_root, target)?;
        let output = output_directory
            .join("binaries")
            .join(compiler_suite_target_output_name(index, target));
        let bake = bake_trusted_direct_rustc_run(&OvenTrustedDirectRustcTargetRequest {
            receipt,
            artifacts: &artifacts,
            artifact_root,
            artifact_plan: Some(&artifact_plan),
            rustc,
            source: &source,
            output: &output,
            crate_name: &target.crate_name,
            edition: &target.edition,
            source_evidence_key: &target.source_evidence_key,
            features: &target.features,
            prefer_dynamic: compiler_suite_workspace_outputs_include_dylib(workspace_library_outputs),
        })
        .map_err(oven_error)?;
        binary_cache.insert(cache_key, bake.output.clone());
        if outputs.insert(target.target_name.clone(), bake.output).is_some() {
            return Err(CliError::failure(format!(
                "stored compiler-suite binary target `{}` is declared more than once",
                target.target_name
            )));
        }
    }
    Ok(outputs)
}

/// Suite-owned proxy for the explicit Cargo capability used by compatibility tests.
struct CompilerSuiteFixtureCargoProxy {
    executable: PathBuf,
    real: PathBuf,
    /// Fallback compiler only when the named publisher did not select one from its receipt.
    real_rustc: PathBuf,
    home: PathBuf,
    log: PathBuf,
}

impl CompilerSuiteFixtureCargoProxy {
    /// Count the invocations appended by the proxy after all suite workers have joined.
    fn invocation_count(&self) -> CliResult<usize> {
        fs::read_to_string(&self.log)
            .map(|payload| payload.lines().count())
            .map_err(|error| {
                CliError::failure(format!(
                    "cannot read compiler-suite fixture Cargo log {}: {error}",
                    self.log.display()
                ))
            })
    }
}

/// Create a transient logged Cargo proxy inside caller-owned suite output.
#[cfg(unix)]
fn prepare_compiler_suite_fixture_cargo_proxy(
    output_directory: &Path,
    cargo: &Path,
) -> CliResult<CompilerSuiteFixtureCargoProxy> {
    use std::os::unix::fs::PermissionsExt;

    let real = fs::canonicalize(cargo).map_err(|error| {
        CliError::failure(format!(
            "cannot resolve explicit compiler-suite fixture Cargo {}: {error}",
            cargo.display()
        ))
    })?;
    let metadata = fs::symlink_metadata(&real).map_err(|error| {
        CliError::failure(format!(
            "cannot inspect explicit compiler-suite fixture Cargo {}: {error}",
            real.display()
        ))
    })?;
    if !metadata.is_file() || metadata.permissions().mode() & 0o111 == 0 {
        return Err(CliError::failure(format!(
            "explicit compiler-suite fixture Cargo must be an executable regular file: {}",
            real.display()
        )));
    }
    let real_rustc = real
        .parent()
        .map(|directory| directory.join("rustc"))
        .ok_or_else(|| CliError::failure("explicit compiler-suite fixture Cargo has no toolchain directory"))?;
    let rustc_metadata = fs::symlink_metadata(&real_rustc).map_err(|error| {
        CliError::failure(format!(
            "cannot inspect matching compiler-suite fixture Rustc {}: {error}",
            real_rustc.display()
        ))
    })?;
    if !rustc_metadata.is_file() || rustc_metadata.permissions().mode() & 0o111 == 0 {
        return Err(CliError::failure(format!(
            "explicit compiler-suite fixture Cargo requires its matching executable Rustc: {}",
            real_rustc.display()
        )));
    }
    let home = user_home()
        .map(PathBuf::from)
        .filter(|path| path.is_dir())
        .ok_or_else(|| {
            CliError::failure(
                "explicit compiler-suite fixture Cargo requires a readable HOME for its offline source cache",
            )
        })?;
    let root = output_directory.join("fixture-cargo");
    fs::create_dir_all(&root).map_err(|error| {
        CliError::failure(format!(
            "cannot create compiler-suite fixture Cargo proxy directory {}: {error}",
            root.display()
        ))
    })?;
    let executable = root.join("cargo");
    let log = root.join("invocations.log");
    let proxy = concat!(
        "#!/bin/sh\n",
        "printf '%s\\n' \"$*\" >> \"$INCAN_INTERNAL_OVEN_FIXTURE_CARGO_LOG\"\n",
        "if [ -z \"${RUSTC:-}\" ]; then export RUSTC=\"$INCAN_INTERNAL_OVEN_FIXTURE_RUSTC_REAL\"; fi\n",
        "exec \"$INCAN_INTERNAL_OVEN_FIXTURE_CARGO_REAL\" \"$@\"\n",
    );
    fs::write(&executable, proxy).map_err(|error| {
        CliError::failure(format!(
            "cannot write compiler-suite fixture Cargo proxy {}: {error}",
            executable.display()
        ))
    })?;
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o500)).map_err(|error| {
        CliError::failure(format!(
            "cannot make compiler-suite fixture Cargo proxy executable {}: {error}",
            executable.display()
        ))
    })?;
    fs::write(&log, []).map_err(|error| {
        CliError::failure(format!(
            "cannot initialize compiler-suite fixture Cargo log {}: {error}",
            log.display()
        ))
    })?;
    Ok(CompilerSuiteFixtureCargoProxy {
        executable: compiler_suite_environment_path(&executable)?,
        real,
        real_rustc,
        home: compiler_suite_environment_path(&home)?,
        log: compiler_suite_environment_path(&log)?,
    })
}

/// The v0.5 compiler suite is supported on the Unix hosts used by release CI.
#[cfg(not(unix))]
fn prepare_compiler_suite_fixture_cargo_proxy(
    _output_directory: &Path,
    _cargo: &Path,
) -> CliResult<CompilerSuiteFixtureCargoProxy> {
    Err(CliError::failure(
        "the compiler-suite compatibility Cargo fixture proxy is supported only on Unix hosts".to_string(),
    ))
}

/// Resolve every mutable input for one compiler-suite child before the bounded parallel execution phase starts.
///
/// This phase runs after the suite has acquired every shard/foundation lease and after shared workspace libraries and
/// helper binaries are materialized once. A prepared child owns its output/environment state and borrows only the
/// already-leased immutable inputs. Each worker reconstitutes its own target manifest, so the scheduler does not clone
/// the complete shared artifact closure serially before the bounded parallel execution phase starts.
#[allow(clippy::too_many_arguments)]
fn prepare_compiler_suite_child<'a>(
    target: &'a crate::oven::legacy_cargo::OvenCompilerTestSuiteTarget,
    closure: &'a crate::oven::legacy_cargo::OvenCompilerTestSuiteArtifactClosure,
    intent: &'a OvenBuildIntent,
    artifact_root: &'a Path,
    rustc: &Path,
    compiler_root: &'a Path,
    output_directory: &Path,
    environment: &BTreeMap<String, String>,
    binary_outputs: &BTreeMap<String, PathBuf>,
    workspace_libraries: &'a [OvenCompilerWorkspaceLibrary],
    workspace_library_outputs: BTreeMap<OvenCompilerWorkspaceLibraryKey, OvenCallerOwnedRustcLibrary>,
    foundation_references: &'a [OvenCompilerTestSuiteFoundationReference],
    foundations: Option<&'a BTreeMap<String, CompilerSuiteFoundationExecution>>,
    fixture_cargo: Option<&CompilerSuiteFixtureCargoProxy>,
) -> CliResult<PreparedCompilerSuiteChild<'a>> {
    let source = compiler_suite_target_source(compiler_root, target)?;
    let output = output_directory.join(compiler_suite_target_output_name(0, target));
    let prefer_dynamic = target.target_kind == "proc-macro"
        || compiler_suite_workspace_outputs_include_dylib(&workspace_library_outputs);
    let mut target_environment = environment.clone();
    let mut binary_compile_environment = BTreeMap::new();
    let child_state_root = compiler_suite_child_state_root(output_directory, target);
    // A parallel child may itself invoke `incan`. Give that nested command an isolated mutable home while it reads
    // the shared, leased provider store through the receipt-bound environment above.
    target_environment.insert(
        "INCAN_HOME".to_string(),
        compiler_suite_environment_path(&child_state_root.join("incan-home"))?
            .display()
            .to_string(),
    );
    // Tests that intentionally clear `INCAN_HOME` must still resolve their default state below the same child-owned
    // boundary rather than a sibling root's home. This remains separate from the parent-leased immutable closure.
    target_environment.insert(
        "HOME".to_string(),
        compiler_suite_environment_path(&child_state_root.join("home"))?
            .display()
            .to_string(),
    );
    // Nested normal commands may still use generated-project state for source and release-asset fixtures. Keep it
    // per child: parallel roots can otherwise race over mutable build-script, lock, and cleanup outputs.
    target_environment.insert(
        "INCAN_GENERATED_CARGO_TARGET_DIR".to_string(),
        compiler_suite_environment_path(&child_state_root.join("generated-cargo-target"))?
            .display()
            .to_string(),
    );
    // Apply the exceptional baker capability only after child-local state has been installed. The capability may
    // replace HOME with the caller-approved offline Cargo source cache; ordinary roots retain this child-owned home.
    apply_compiler_suite_target_capabilities(target, &mut target_environment, fixture_cargo)?;
    for dependency in &target.binary_dependencies {
        let output = binary_outputs.get(dependency).ok_or_else(|| {
            CliError::failure(format!(
                "stored compiler-suite target `{}` requires missing direct binary `{dependency}`",
                target.target_name
            ))
        })?;
        let name = format!("CARGO_BIN_EXE_{dependency}");
        let value = compiler_suite_environment_path(output)?.display().to_string();
        target_environment.insert(name.clone(), value.clone());
        binary_compile_environment.insert(name, value);
    }
    if prefer_dynamic {
        let (name, value) = compiler_suite_dynamic_library_environment(rustc, &workspace_library_outputs)?;
        target_environment.insert(name, value);
    }
    Ok(PreparedCompilerSuiteChild {
        target,
        closure,
        intent,
        artifact_root,
        compiler_root,
        source,
        output,
        environment: target_environment,
        prefer_dynamic,
        binary_compile_environment,
        workspace_libraries,
        workspace_library_outputs,
        foundation_references,
        foundations,
    })
}

/// Return whether one indexed compiler-suite schema composes its direct-Rustc plan from leased foundations.
///
/// Schema 14 and later change only how children reach the compiler-owned standard-library Loaf family. They retain
/// the schema-10 foundation contract for Cargo-published third-party artifacts, so they must never bypass this path.
fn compiler_suite_uses_indexed_foundations(schema_version: u32) -> bool {
    matches!(schema_version, 10..=OVEN_COMPILER_TEST_SUITE_SCHEMA_VERSION)
}

/// Apply the package-qualified process capabilities owned by the compiler-suite registry.
fn apply_compiler_suite_target_capabilities(
    target: &crate::oven::legacy_cargo::OvenCompilerTestSuiteTarget,
    environment: &mut BTreeMap<String, String>,
    fixture_cargo: Option<&CompilerSuiteFixtureCargoProxy>,
) -> CliResult<()> {
    let capabilities = OvenCompilerSuiteTargetCapabilities::for_target(
        &target.package_name,
        &target.target_kind,
        &target.source_relative_path,
    );
    if !capabilities.generated_rust_closure {
        compiler_suite_remove_generated_rust_closure(environment);
    }
    if capabilities.cargo_fixture {
        let fixture_cargo = fixture_cargo.ok_or_else(|| {
            CliError::failure(format!(
                "compiler-suite root `{}` exercises Cargo compatibility and requires explicit --fixture-cargo",
                target.source_relative_path
            ))
        })?;
        environment.insert("CARGO".to_string(), fixture_cargo.executable.display().to_string());
        environment.insert(
            OVEN_COMPILER_SUITE_FIXTURE_CARGO_REAL_ENV.to_string(),
            fixture_cargo.real.display().to_string(),
        );
        environment.insert(
            OVEN_COMPILER_SUITE_FIXTURE_RUSTC_REAL_ENV.to_string(),
            fixture_cargo.real_rustc.display().to_string(),
        );
        environment.insert(
            OVEN_COMPILER_SUITE_FIXTURE_CARGO_LOG_ENV.to_string(),
            fixture_cargo.log.display().to_string(),
        );
        // Only a package-qualified explicit-bake root receives the caller's offline Cargo source cache. The baker
        // copies and digests the sources it uses; ordinary roots remain in their child-owned homes without Cargo.
        environment.insert("HOME".to_string(), fixture_cargo.home.display().to_string());
    }
    if capabilities.explicit_bake_cargo {
        let fixture_cargo = fixture_cargo.ok_or_else(|| {
            CliError::failure(format!(
                "compiler-suite root `{}` explicitly bakes a Loaf and requires --fixture-cargo",
                target.source_relative_path
            ))
        })?;
        // Do not set `CARGO` here. The test helper receives the three values below and installs them only on its
        // explicit `incan oven bake` child; all normal command probes retain the scheduler's Cargo guard.
        environment.insert(
            OVEN_COMPILER_SUITE_EXPLICIT_BAKE_CARGO_ENV.to_string(),
            fixture_cargo.real.display().to_string(),
        );
        environment.insert(
            OVEN_COMPILER_SUITE_EXPLICIT_BAKE_HOME_ENV.to_string(),
            fixture_cargo.home.display().to_string(),
        );
    }
    Ok(())
}

/// Remove direct generated-Rust closure details while retaining the suite marker used by Cargo-free fixture paths.
fn compiler_suite_remove_generated_rust_closure(environment: &mut BTreeMap<String, String>) {
    environment.retain(|key, _| !key.starts_with("INCAN_OVEN_COMPILER_SUITE_") || key == OVEN_COMPILER_SUITE_RUSTC_ENV);
}

/// Compile and execute prepared compiler-suite roots with a bounded worker pool.
///
/// Workers start only after the one shared DAG/preparation phase has completed, so parallelism improves root
/// throughput without multiplying provider setup, Cargo-compatible publication, store leases, or generated homes.
fn run_prepared_compiler_suite_children(
    children: Vec<PreparedCompilerSuiteChild<'_>>,
    receipt: &OvenReceipt,
    rustc: &Path,
    exact_test_names: Option<&[String]>,
) -> CliResult<CompilerSuiteChildrenReport> {
    let child_count = children.len();
    if child_count == 0 {
        return Ok(CompilerSuiteChildrenReport::default());
    }
    let queue = Mutex::new(VecDeque::from(children));
    let results = Mutex::new(Vec::<Result<CompilerSuiteChildrenReport, String>>::with_capacity(
        child_count,
    ));
    let worker_count = compiler_suite_parallel_jobs(child_count);
    let logical_cores = thread::available_parallelism().map(usize::from).unwrap_or(1);
    let libtest_threads = compiler_suite_libtest_threads(logical_cores, worker_count);
    let panicked = thread::scope(|scope| {
        let workers = (0..worker_count)
            .map(|_| {
                scope.spawn(|| {
                    loop {
                        let child = match queue.lock() {
                            Ok(mut queue) => queue.pop_front(),
                            Err(_) => {
                                if let Ok(mut results) = results.lock() {
                                    results.push(Err("compiler-suite worker queue was poisoned".to_string()));
                                }
                                return;
                            }
                        };
                        let Some(child) = child else {
                            return;
                        };
                        let result =
                            run_prepared_compiler_suite_child(child, receipt, rustc, libtest_threads, exact_test_names)
                                .map_err(|error| error.to_string());
                        if let Ok(mut results) = results.lock() {
                            results.push(result);
                        }
                    }
                })
            })
            .collect::<Vec<_>>();
        workers.into_iter().any(|worker| worker.join().is_err())
    });
    if panicked {
        return Err(CliError::failure(
            "a bounded compiler-suite direct-Rustc worker panicked".to_string(),
        ));
    }
    let results = results
        .into_inner()
        .map_err(|_| CliError::failure("compiler-suite worker results were poisoned".to_string()))?;
    if results.len() != child_count {
        return Err(CliError::failure(format!(
            "compiler-suite worker pool produced {} result(s) for {child_count} prepared child(ren)",
            results.len()
        )));
    }
    let mut report = CompilerSuiteChildrenReport::default();
    for result in results {
        match result {
            Ok(child_report) => report.append(child_report),
            Err(error) => report
                .failed
                .push(format!("Oven direct-Rustc compiler-suite worker failed:\n{error}")),
        }
    }
    report.native_test_roots.sort_by(|left, right| {
        (
            left.package_name.as_str(),
            left.target_kind.as_str(),
            left.target_name.as_str(),
        )
            .cmp(&(
                right.package_name.as_str(),
                right.target_kind.as_str(),
                right.target_name.as_str(),
            ))
    });
    Ok(report)
}

/// Execute one prepared direct-Rustc compiler-suite child with no Cargo process or store mutation.
fn run_prepared_compiler_suite_child(
    child: PreparedCompilerSuiteChild<'_>,
    receipt: &OvenReceipt,
    rustc: &Path,
    libtest_threads: usize,
    exact_test_names: Option<&[String]>,
) -> CliResult<CompilerSuiteChildrenReport> {
    let mut artifacts = child.closure.manifest_for_target(child.target, child.intent.clone());
    for (name, value) in &child.binary_compile_environment {
        artifacts.compile_environment.insert(name.clone(), value.clone());
    }
    let mut artifact_plan = match child.foundations {
        Some(foundations) => {
            compiler_suite_composed_artifact_plan(&artifacts, child.foundation_references, foundations, child.intent)?
        }
        None => artifacts
            .materialize_trusted_store(child.artifact_root, child.intent)
            .map_err(oven_error)?,
    };
    attach_compiler_suite_target_workspace_libraries(
        &mut artifact_plan,
        child.target,
        child.workspace_libraries,
        &child.workspace_library_outputs,
    )?;
    match child.target.runner.as_str() {
        "rustc-test" => {
            let bake_started = Instant::now();
            let bake = bake_trusted_direct_rustc_test(&OvenTrustedDirectRustcTargetRequest {
                receipt,
                artifacts: &artifacts,
                artifact_root: child.artifact_root,
                artifact_plan: Some(&artifact_plan),
                rustc,
                source: &child.source,
                output: &child.output,
                crate_name: &child.target.crate_name,
                edition: &child.target.edition,
                source_evidence_key: &child.target.source_evidence_key,
                features: &child.target.features,
                prefer_dynamic: child.prefer_dynamic,
            })
            .map_err(oven_error)?;
            let direct_rustc_bake_elapsed_ms = bake_started.elapsed().as_millis();
            let working_directory =
                compiler_suite_target_working_directory(child.compiler_root, &child.source, child.target)?;
            let report = match exact_test_names {
                Some(exact_test_names) => run_native_tests_exact_in_directory_with_timeout(
                    &bake.output,
                    exact_test_names,
                    &child.environment,
                    Some(&working_directory),
                    Some(OVEN_COMPILER_TEST_ROOT_TIMEOUT),
                ),
                None => run_native_test_batch_all_in_directory_with_timeout_and_threads(
                    &bake.output,
                    &child.environment,
                    Some(&working_directory),
                    Some(OVEN_COMPILER_TEST_ROOT_TIMEOUT),
                    libtest_threads,
                ),
            }
            .map_err(oven_error)?;
            let failures = if report.success {
                Vec::new()
            } else {
                let transcript = write_native_test_failure_transcript(&child.output, &report.output)?;
                vec![format!(
                    "{} target `{}` failed; full libtest transcript: {}\n{}",
                    child.target.target_kind,
                    child.target.target_name,
                    transcript.display(),
                    native_test_failure_summary(&report.output)
                )]
            };
            Ok(CompilerSuiteChildrenReport {
                native_test_count: if exact_test_names.is_some() {
                    report
                        .case_counts
                        .as_ref()
                        .map(|counts| counts.passed + counts.failed + counts.ignored)
                        .unwrap_or(0)
                } else {
                    report.inventory.names.len()
                },
                doctest_targets: 0,
                failed: failures,
                native_test_roots: vec![CompilerSuiteNativeTestRootReport {
                    package_name: child.target.package_name.clone(),
                    target_kind: child.target.target_kind.clone(),
                    target_name: child.target.target_name.clone(),
                    source_relative_path: child.target.source_relative_path.clone(),
                    inventory_count: report.inventory.names.len(),
                    success: report.success,
                    case_counts: report.case_counts,
                    case_timings: report.case_timings,
                    command_timings: report.command_timings,
                    direct_rustc_bake_elapsed_ms,
                    libtest_inventory_elapsed_ms: report.timing.inventory_elapsed_ms,
                    libtest_execution_elapsed_ms: report.timing.execution_elapsed_ms,
                }],
                rustdoc_test_roots: Vec::new(),
            })
        }
        "rustdoc-test" => {
            let temporary_directory = child.output.with_extension("rustdoc-tmp");
            let rustdoc_started = Instant::now();
            run_trusted_rustdoc_test(&OvenTrustedRustdocTestRequest {
                receipt,
                artifacts: &artifacts,
                artifact_root: child.artifact_root,
                artifact_plan: Some(&artifact_plan),
                rustc,
                source: &child.source,
                temporary_directory: &temporary_directory,
                crate_name: &child.target.crate_name,
                edition: &child.target.edition,
                source_evidence_key: &child.target.source_evidence_key,
                features: &child.target.features,
                is_proc_macro: child.target.target_kind == "proc-macro",
                prefer_dynamic: child.prefer_dynamic,
                timeout: Some(OVEN_COMPILER_TEST_ROOT_TIMEOUT),
            })
            .map_err(oven_error)?;
            Ok(CompilerSuiteChildrenReport {
                native_test_count: 0,
                doctest_targets: 1,
                failed: Vec::new(),
                native_test_roots: Vec::new(),
                rustdoc_test_roots: vec![CompilerSuiteRustdocTestRootReport {
                    package_name: child.target.package_name.clone(),
                    target_kind: child.target.target_kind.clone(),
                    target_name: child.target.target_name.clone(),
                    source_relative_path: child.target.source_relative_path.clone(),
                    execution_elapsed_ms: rustdoc_started.elapsed().as_millis(),
                }],
            })
        }
        runner => Err(CliError::failure(format!(
            "stored compiler-suite target `{}` declares unsupported Oven runner `{runner}`",
            child.target.target_name
        ))),
    }
}

/// Derive a root-worker count that leaves capacity for each root's libtest and nested Incan children.
///
/// Each direct-Rustc root is a substantial libtest binary that can run test cases in parallel and launch normal Incan
/// commands. On a constrained host, prefer that useful inner concurrency over making several large roots serial:
/// one core runs one root, 2--3 cores run one root with two libtest threads, and wider hosts add roots only when the
/// resulting root-worker and libtest-thread product remains within the host budget. An explicit operator override
/// remains available for measured release-machine tuning.
fn compiler_suite_auto_parallel_jobs(logical_cores: usize) -> usize {
    match logical_cores {
        0 | 1 => 1,
        2..=3 => 1,
        4..=5 => 2,
        6..=7 => 3,
        _ => 4,
    }
}

/// Bound direct compiler-suite root parallelism while allowing a release machine to select a measured safe value.
fn compiler_suite_parallel_jobs(child_count: usize) -> usize {
    let configured = env::var(OVEN_COMPILER_TEST_JOBS_ENV)
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0);
    let detected = thread::available_parallelism().map(usize::from).unwrap_or(1);
    configured
        .unwrap_or_else(|| compiler_suite_auto_parallel_jobs(detected))
        .min(child_count)
        .max(1)
}

/// Divide the detected host capacity among the concurrently scheduled libtest roots.
///
/// Root workers mainly wait for their libtest child, while that child can launch nested normal Incan commands. Keep
/// the aggregate libtest fan-out at or below the host count, and cap each root at two workers so a large machine
/// cannot recreate a wide nested-command burst merely because libtest observed many CPUs.
fn compiler_suite_libtest_threads(logical_cores: usize, root_workers: usize) -> usize {
    let logical_cores = logical_cores.max(1);
    let root_workers = root_workers.max(1);
    (logical_cores / root_workers).clamp(1, 2)
}

/// Split the host budget between bounded root workers and the libtest threads inside each root.
///
/// A root can launch nested normal Incan commands, so reserving one outer-worker share prevents the scheduler from
/// recreating the unbounded `outer roots × default libtest threads` fan-out that delays capacity and timeout guards.
/// Compile/inventory/execute every receipt-bound Rustc or Rustdoc workspace test target while the suite lease is
/// still held by the caller. No Cargo-linked test executable is copied or run from the immutable entry.
#[allow(clippy::too_many_arguments)]
fn run_planned_compiler_suite_children(
    targets: &[crate::oven::legacy_cargo::OvenCompilerTestSuiteTarget],
    closure: &crate::oven::legacy_cargo::OvenCompilerTestSuiteArtifactClosure,
    intent: &OvenBuildIntent,
    receipt: &OvenReceipt,
    artifact_root: &Path,
    rustc: &Path,
    compiler_root: &Path,
    output_directory: &Path,
    environment: &BTreeMap<String, String>,
    binary_outputs: &BTreeMap<String, PathBuf>,
    workspace_libraries: &[OvenCompilerWorkspaceLibrary],
    workspace_library_outputs: &BTreeMap<OvenCompilerWorkspaceLibraryKey, OvenCallerOwnedRustcLibrary>,
    foundation_references: &[OvenCompilerTestSuiteFoundationReference],
    foundations: Option<&BTreeMap<String, CompilerSuiteFoundationExecution>>,
    fixture_cargo: Option<&CompilerSuiteFixtureCargoProxy>,
) -> CliResult<CompilerSuiteChildrenReport> {
    let mut suite_report = CompilerSuiteChildrenReport::default();
    for (index, target) in targets.iter().enumerate() {
        let mut artifacts = closure.manifest_for_target(target, intent.clone());
        let source = compiler_suite_target_source(compiler_root, target)?;
        let output = output_directory.join(compiler_suite_target_output_name(index, target));
        let prefer_dynamic = target.target_kind == "proc-macro"
            || compiler_suite_workspace_outputs_include_dylib(workspace_library_outputs);
        let mut target_environment = environment.clone();
        apply_compiler_suite_target_capabilities(target, &mut target_environment, fixture_cargo)?;
        for dependency in &target.binary_dependencies {
            let output = binary_outputs.get(dependency).ok_or_else(|| {
                CliError::failure(format!(
                    "stored compiler-suite target `{}` requires missing direct binary `{dependency}`",
                    target.target_name
                ))
            })?;
            let name = format!("CARGO_BIN_EXE_{dependency}");
            let value = compiler_suite_environment_path(output)?.display().to_string();
            artifacts.compile_environment.insert(name.clone(), value.clone());
            target_environment.insert(name, value);
        }
        if prefer_dynamic {
            let (name, value) = compiler_suite_dynamic_library_environment(rustc, workspace_library_outputs)?;
            target_environment.insert(name, value);
        }
        let mut artifact_plan = match foundations {
            Some(foundations) => {
                compiler_suite_composed_artifact_plan(&artifacts, foundation_references, foundations, intent)?
            }
            None => artifacts
                .materialize_trusted_store(artifact_root, intent)
                .map_err(oven_error)?,
        };
        attach_compiler_suite_target_workspace_libraries(
            &mut artifact_plan,
            target,
            workspace_libraries,
            workspace_library_outputs,
        )?;
        match target.runner.as_str() {
            "rustc-test" => {
                let bake_started = Instant::now();
                let bake = bake_trusted_direct_rustc_test(&OvenTrustedDirectRustcTargetRequest {
                    receipt,
                    artifacts: &artifacts,
                    artifact_root,
                    artifact_plan: Some(&artifact_plan),
                    rustc,
                    source: &source,
                    output: &output,
                    crate_name: &target.crate_name,
                    edition: &target.edition,
                    source_evidence_key: &target.source_evidence_key,
                    features: &target.features,
                    prefer_dynamic,
                })
                .map_err(oven_error)?;
                let direct_rustc_bake_elapsed_ms = bake_started.elapsed().as_millis();
                let working_directory = compiler_suite_target_working_directory(compiler_root, &source, target)?;
                let report = run_native_test_batch_all_in_directory_with_timeout(
                    &bake.output,
                    &target_environment,
                    Some(&working_directory),
                    Some(OVEN_COMPILER_TEST_ROOT_TIMEOUT),
                )
                .map_err(oven_error)?;
                suite_report.native_test_count += report.inventory.names.len();
                suite_report.native_test_roots.push(CompilerSuiteNativeTestRootReport {
                    package_name: target.package_name.clone(),
                    target_kind: target.target_kind.clone(),
                    target_name: target.target_name.clone(),
                    source_relative_path: target.source_relative_path.clone(),
                    inventory_count: report.inventory.names.len(),
                    success: report.success,
                    case_counts: report.case_counts.clone(),
                    case_timings: report.case_timings.clone(),
                    command_timings: report.command_timings.clone(),
                    direct_rustc_bake_elapsed_ms,
                    libtest_inventory_elapsed_ms: report.timing.inventory_elapsed_ms,
                    libtest_execution_elapsed_ms: report.timing.execution_elapsed_ms,
                });
                if !report.success {
                    let transcript = write_native_test_failure_transcript(&output, &report.output)?;
                    suite_report.failed.push(format!(
                        "{} target `{}` failed; full libtest transcript: {}\n{}",
                        target.target_kind,
                        target.target_name,
                        transcript.display(),
                        native_test_failure_summary(&report.output)
                    ));
                }
            }
            "rustdoc-test" => {
                let temporary_directory = output.with_extension("rustdoc-tmp");
                let rustdoc_started = Instant::now();
                let _report = run_trusted_rustdoc_test(&OvenTrustedRustdocTestRequest {
                    receipt,
                    artifacts: &artifacts,
                    artifact_root,
                    artifact_plan: Some(&artifact_plan),
                    rustc,
                    source: &source,
                    temporary_directory: &temporary_directory,
                    crate_name: &target.crate_name,
                    edition: &target.edition,
                    source_evidence_key: &target.source_evidence_key,
                    features: &target.features,
                    is_proc_macro: target.target_kind == "proc-macro",
                    prefer_dynamic,
                    timeout: Some(OVEN_COMPILER_TEST_ROOT_TIMEOUT),
                })
                .map_err(oven_error)?;
                suite_report.doctest_targets += 1;
                suite_report
                    .rustdoc_test_roots
                    .push(CompilerSuiteRustdocTestRootReport {
                        package_name: target.package_name.clone(),
                        target_kind: target.target_kind.clone(),
                        target_name: target.target_name.clone(),
                        source_relative_path: target.source_relative_path.clone(),
                        execution_elapsed_ms: rustdoc_started.elapsed().as_millis(),
                    });
            }
            runner => {
                return Err(CliError::failure(format!(
                    "stored compiler-suite target `{}` declares unsupported Oven runner `{runner}`",
                    target.target_name
                )));
            }
        }
    }
    Ok(suite_report)
}

/// Resolve a target-plan source only beneath the current receipt-authorized compiler root.
fn compiler_suite_target_source(
    compiler_root: &Path,
    target: &crate::oven::legacy_cargo::OvenCompilerTestSuiteTarget,
) -> CliResult<PathBuf> {
    compiler_suite_source_path(
        compiler_root,
        &target.source_relative_path,
        &format!("target `{}`", target.target_name),
    )
}

/// Resolve a planned workspace library source beneath the receipt-authorized compiler root.
fn compiler_suite_workspace_library_source(
    compiler_root: &Path,
    library: &OvenCompilerWorkspaceLibrary,
) -> CliResult<PathBuf> {
    compiler_suite_source_path(
        compiler_root,
        &library.key.source_relative_path,
        &format!("workspace library `{}`", library.key.crate_name),
    )
}

/// Resolve one receipt-bound relative source path while rejecting traversal and symlink escape.
fn compiler_suite_source_path(compiler_root: &Path, relative_path: &str, subject: &str) -> CliResult<PathBuf> {
    let compiler_root = fs::canonicalize(compiler_root).map_err(|error| {
        CliError::failure(format!(
            "cannot canonicalize compiler-suite root {}: {error}",
            compiler_root.display()
        ))
    })?;
    let mut source = compiler_root.clone();
    for component in Path::new(relative_path).components() {
        let Component::Normal(component) = component else {
            return Err(CliError::failure(format!(
                "stored compiler-suite {subject} has unsafe source path `{relative_path}`"
            )));
        };
        source.push(component);
    }
    let metadata = fs::symlink_metadata(&source).map_err(|error| {
        CliError::failure(format!(
            "cannot inspect compiler-suite {subject} source {}: {error}",
            source.display(),
        ))
    })?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(CliError::failure(format!(
            "compiler-suite {subject} source {} must be a regular non-symlink file",
            source.display(),
        )));
    }
    let source = fs::canonicalize(&source).map_err(|error| {
        CliError::failure(format!(
            "cannot canonicalize compiler-suite {subject} source {}: {error}",
            source.display(),
        ))
    })?;
    if !source.starts_with(&compiler_root) {
        return Err(CliError::failure(format!(
            "compiler-suite {subject} source {} escapes compiler root {}",
            source.display(),
            compiler_root.display()
        )));
    }
    Ok(source)
}

/// Recover the package-root working directory Cargo would have supplied to one compiler test target.
///
/// The target's compile environment is receipt-bound publisher data, but execution must still reject a directory
/// outside the caller-authorized compiler root. That preserves Cargo's package-root semantics for snapshots and
/// fixtures without treating the caller-owned output directory as a source root.
fn compiler_suite_target_working_directory(
    compiler_root: &Path,
    source: &Path,
    target: &crate::oven::legacy_cargo::OvenCompilerTestSuiteTarget,
) -> CliResult<PathBuf> {
    let compiler_root = fs::canonicalize(compiler_root).map_err(|error| {
        CliError::failure(format!(
            "cannot canonicalize compiler-suite root {}: {error}",
            compiler_root.display()
        ))
    })?;
    let declared = target.compile_environment.get("CARGO_MANIFEST_DIR").ok_or_else(|| {
        CliError::failure(format!(
            "stored compiler-suite target `{}` has no package working-directory declaration",
            target.target_name
        ))
    })?;
    let working_directory =
        resolve_compile_environment_value("CARGO_MANIFEST_DIR", declared, source).map_err(oven_error)?;
    let working_directory = fs::canonicalize(&working_directory).map_err(|error| {
        CliError::failure(format!(
            "cannot canonicalize compiler-suite working directory {} for target `{}`: {error}",
            working_directory.display(),
            target.target_name
        ))
    })?;
    if !working_directory.starts_with(&compiler_root) {
        return Err(CliError::failure(format!(
            "stored compiler-suite target `{}` declares working directory {} outside compiler root {}",
            target.target_name,
            working_directory.display(),
            compiler_root.display()
        )));
    }
    if !working_directory.is_dir() {
        return Err(CliError::failure(format!(
            "stored compiler-suite target `{}` working directory {} is not a directory",
            target.target_name,
            working_directory.display()
        )));
    }
    Ok(working_directory)
}

/// Keep caller-owned test shard paths deterministic and safely inside the selected output directory.
fn compiler_suite_target_output_name(
    index: usize,
    target: &crate::oven::legacy_cargo::OvenCompilerTestSuiteTarget,
) -> String {
    format!(
        "{index:04}-{}-{}-{}",
        compiler_suite_output_segment(&target.package_name),
        compiler_suite_output_segment(&target.target_kind),
        compiler_suite_output_segment(&target.target_name)
    )
}

/// Return the mutable caller-owned state root for one scheduled compiler-suite child.
///
/// The immutable Loaf envelope is shared and lease-protected at the suite level. Homes and generated-project
/// targets are not: each root receives a distinct location so its nested normal commands cannot race a sibling.
fn compiler_suite_child_state_root(
    output_directory: &Path,
    target: &crate::oven::legacy_cargo::OvenCompilerTestSuiteTarget,
) -> PathBuf {
    output_directory
        .join("children")
        .join(compiler_suite_target_output_name(0, target))
}

/// Keep direct-Rustc workspace library outputs deterministic and separate from executable target outputs.
fn compiler_suite_workspace_library_output_name(index: usize, key: &OvenCompilerWorkspaceLibraryKey) -> String {
    let extension = if key.target_kind == "proc-macro" || compiler_suite_workspace_library_uses_dylib(key) {
        std::env::consts::DLL_SUFFIX
    } else {
        ".rlib"
    };
    format!(
        "lib{}-{index:04}-{}{}",
        compiler_suite_output_segment(&key.crate_name),
        compiler_suite_output_segment(&key.package_name),
        extension,
    )
}

/// Build the top-level compiler library once as a dynamic direct-Rustc boundary.
///
/// Every integration root otherwise statically embeds this large shared library, which defeats whole-suite reuse
/// even after its workspace DAG is deduplicated. This remains an Oven caller output, not a Cargo target artifact.
fn compiler_suite_workspace_library_uses_dylib(key: &OvenCompilerWorkspaceLibraryKey) -> bool {
    key.target_kind == "lib" && key.package_name == "incan" && key.crate_name == "incan"
}

/// Convert publisher-provided display labels into one portable output-path segment.
fn compiler_suite_output_segment(value: &str) -> String {
    let segment = value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.') {
                character
            } else {
                '-'
            }
        })
        .collect::<String>();
    if segment.is_empty() {
        "unnamed".to_string()
    } else {
        segment
    }
}

/// One schema-9 shard whose verified immutable store payload remains lease-protected for a complete suite batch.
///
/// The payload deliberately stays paired with its store selection. Extracting the payload while dropping the
/// selection would let policy pruning reclaim a later shard after an earlier compiler test starts.
struct CompilerSuiteShardExecution {
    stored: OvenStoreExecutionPayload,
    payload: OvenCompilerTestSuiteShardPayload,
}

/// One schema-10 foundation whose selected payload and store root remain actively leased for the whole suite run.
struct CompilerSuiteFoundationExecution {
    stored: OvenStoreExecutionPayload,
    payload: OvenCompilerTestSuiteFoundationPayload,
}

/// One schema-13 Loaf partition whose immutable store entry remains leased for the whole suite run.
struct CompilerSuiteToolchainDataExecution {
    stored: OvenStoreExecutionPayload,
    payload: OvenCompilerTestSuiteToolchainDataPayload,
}

/// One direct-Rustc libtest root's terminal coverage result.
#[derive(Debug, Clone, Serialize)]
struct CompilerSuiteNativeTestRootReport {
    package_name: String,
    target_kind: String,
    target_name: String,
    source_relative_path: String,
    inventory_count: usize,
    /// Whether this root reached a successful terminal libtest result.
    success: bool,
    case_counts: Option<OvenNativeTestCaseCounts>,
    /// Opt-in case timings emitted by this root's already-executed libtest process.
    case_timings: Vec<OvenNativeTestCaseTiming>,
    /// Opt-in nested Incan command timings parsed from this root's captured libtest transcript.
    command_timings: Vec<OvenNativeTestCommandTiming>,
    /// Time spent compiling this caller-owned libtest binary through direct Rustc.
    direct_rustc_bake_elapsed_ms: u128,
    /// Time spent obtaining the binary's libtest inventory before execution.
    libtest_inventory_elapsed_ms: u64,
    /// Time spent executing this root's verified libtest process.
    libtest_execution_elapsed_ms: u64,
}

/// One receipt-bound Rustdoc root's timing result.
#[derive(Debug, Clone, Serialize)]
struct CompilerSuiteRustdocTestRootReport {
    package_name: String,
    target_kind: String,
    target_name: String,
    source_relative_path: String,
    /// Time spent running Rustdoc and the doctests it owns for this source root.
    execution_elapsed_ms: u128,
}

/// One slow native libtest case across the compiler suite.
///
/// This is a flattened top-25 view of root-local timing diagnostics. The source root stays attached so a test name
/// alone cannot obscure which package or integration suite owns the cost.
#[derive(Debug, Clone, Serialize)]
struct CompilerSuiteSlowNativeTestCaseReport {
    package_name: String,
    target_kind: String,
    target_name: String,
    source_relative_path: String,
    test_name: String,
    elapsed_ms: u64,
}

/// Phase timings collected from work the compiler-suite command already performs.
///
/// These are measured in-process rather than inferred from shell logs, so release evidence can identify a shared
/// setup bottleneck separately from a direct-Rustc compile or one native test root.
#[derive(Debug, Clone, Default, Serialize)]
struct CompilerSuiteTimingReport {
    receipt_and_selection_elapsed_ms: u128,
    shared_setup_elapsed_ms: u128,
    target_preparation_elapsed_ms: u128,
    root_execution_elapsed_ms: u128,
    total_elapsed_ms: u128,
}

/// Internal split returned by the schema-specific child planner.
#[derive(Debug, Clone, Default)]
struct CompilerSuiteChildPhaseTimings {
    target_preparation_elapsed_ms: u128,
    root_execution_elapsed_ms: u128,
}

/// Explicit coverage boundary for one compiler-suite report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct CompilerSuiteSelectionReport {
    /// `complete-suite`, `selected-complete-roots`, or `exact-diagnostic`.
    mode: &'static str,
    /// Deterministically normalized exact names; empty for every non-exact run.
    normalized_exact_names: Vec<String>,
    /// Number of exact cases requested from the selected root; zero for a complete-root run.
    selected_case_count: usize,
    /// Deterministically normalized paths from explicit `--target` arguments.
    requested_target_paths: Vec<String>,
    /// Zero-based receipt-index partition when the caller requested one.
    partition_index: Option<usize>,
    /// Total receipt-index partition count paired with `partition_index`.
    partition_count: Option<usize>,
    /// Number of receipt-bound roots selected by target, partition, or complete-suite selection.
    selected_root_count: usize,
    /// Whether a green invocation can serve as complete-root evidence for every selected root.
    complete_root_evidence: bool,
    /// Whether a green invocation covers the complete compiler workspace suite.
    complete_suite_evidence: bool,
}

/// Describe the exact, target, partition, or full-suite boundary of one invocation.
fn compiler_suite_selection_report(
    exact_test_names: Option<&[String]>,
    requested_target_paths: &[String],
    partition_index: Option<usize>,
    partition_count: Option<usize>,
    selected_root_count: usize,
) -> CompilerSuiteSelectionReport {
    let requested_target_paths = requested_target_paths
        .iter()
        .map(|path| path.trim())
        .filter(|path| !path.is_empty())
        .map(str::to_string)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    match exact_test_names {
        Some(exact_test_names) => CompilerSuiteSelectionReport {
            mode: "exact-diagnostic",
            normalized_exact_names: exact_test_names.to_vec(),
            selected_case_count: exact_test_names.len(),
            requested_target_paths,
            partition_index,
            partition_count,
            selected_root_count,
            complete_root_evidence: false,
            complete_suite_evidence: false,
        },
        None if requested_target_paths.is_empty() && partition_index.is_none() => CompilerSuiteSelectionReport {
            mode: "complete-suite",
            normalized_exact_names: Vec::new(),
            selected_case_count: 0,
            requested_target_paths,
            partition_index,
            partition_count,
            selected_root_count,
            complete_root_evidence: true,
            complete_suite_evidence: true,
        },
        None => CompilerSuiteSelectionReport {
            mode: "selected-complete-roots",
            normalized_exact_names: Vec::new(),
            selected_case_count: 0,
            requested_target_paths,
            partition_index,
            partition_count,
            selected_root_count,
            complete_root_evidence: true,
            complete_suite_evidence: false,
        },
    }
}

/// Render the requested target or partition boundary for selected-root terminal output.
fn compiler_suite_selection_context(selection: &CompilerSuiteSelectionReport) -> String {
    if !selection.requested_target_paths.is_empty() {
        return format!("target path(s) {}", selection.requested_target_paths.join(", "));
    }
    match (selection.partition_index, selection.partition_count) {
        (Some(index), Some(count)) => format!("zero-based partition {index} of {count}"),
        _ => "all receipt-bound roots".to_string(),
    }
}

/// Complete native-test coverage for one compiler-suite invocation.
#[derive(Debug, Default)]
struct CompilerSuiteChildrenReport {
    native_test_count: usize,
    doctest_targets: usize,
    failed: Vec<String>,
    native_test_roots: Vec<CompilerSuiteNativeTestRootReport>,
    rustdoc_test_roots: Vec<CompilerSuiteRustdocTestRootReport>,
}

/// Aggregate case counts from libtest summaries already captured by the worker processes.
#[derive(Debug, Clone, Default, Serialize)]
struct CompilerSuiteNativeTestCaseTotals {
    passed: usize,
    failed: usize,
    ignored: usize,
    reported_roots: usize,
    green_roots: usize,
    failed_roots: usize,
    unreported_roots: usize,
}

impl CompilerSuiteChildrenReport {
    /// Add one child result without losing its root-level coverage accounting.
    fn append(&mut self, other: Self) {
        self.native_test_count += other.native_test_count;
        self.doctest_targets += other.doctest_targets;
        self.failed.extend(other.failed);
        self.native_test_roots.extend(other.native_test_roots);
        self.rustdoc_test_roots.extend(other.rustdoc_test_roots);
    }

    /// Summarize libtest cases and root outcomes without a second execution or caller-output scan.
    fn native_test_case_totals(&self) -> CompilerSuiteNativeTestCaseTotals {
        let mut totals = CompilerSuiteNativeTestCaseTotals::default();
        for root in &self.native_test_roots {
            if let Some(counts) = &root.case_counts {
                totals.passed += counts.passed;
                totals.failed += counts.failed;
                totals.ignored += counts.ignored;
                totals.reported_roots += 1;
            } else {
                totals.unreported_roots += 1;
            }
            match (&root.case_counts, root.success) {
                (Some(_), true) => totals.green_roots += 1,
                (Some(_), false) => totals.failed_roots += 1,
                (None, _) => {}
            }
        }
        totals
    }

    /// Return the requested slowest observed cases without another test run or transcript scan.
    fn slowest_native_test_cases(&self, limit: usize) -> Vec<CompilerSuiteSlowNativeTestCaseReport> {
        let mut cases = self
            .native_test_roots
            .iter()
            .flat_map(|root| {
                root.case_timings
                    .iter()
                    .map(move |timing| CompilerSuiteSlowNativeTestCaseReport {
                        package_name: root.package_name.clone(),
                        target_kind: root.target_kind.clone(),
                        target_name: root.target_name.clone(),
                        source_relative_path: root.source_relative_path.clone(),
                        test_name: timing.name.clone(),
                        elapsed_ms: timing.elapsed_ms,
                    })
            })
            .collect::<Vec<_>>();
        cases.sort_by(|left, right| {
            right
                .elapsed_ms
                .cmp(&left.elapsed_ms)
                .then_with(|| left.source_relative_path.cmp(&right.source_relative_path))
                .then_with(|| left.test_name.cmp(&right.test_name))
        });
        cases.truncate(limit);
        cases
    }
}

/// Return aggregate failures when planned roots did not produce one complete terminal outcome each.
fn compiler_suite_completion_failures(
    report: &CompilerSuiteChildrenReport,
    planned_target_count: usize,
) -> Vec<String> {
    let totals = report.native_test_case_totals();
    let mut failures = Vec::new();
    if totals.unreported_roots > 0 {
        failures.push(format!(
            "{count} native compiler-suite root(s) did not report a terminal libtest summary",
            count = totals.unreported_roots
        ));
    }
    let terminal_target_count = report.native_test_roots.len().saturating_add(report.doctest_targets);
    if terminal_target_count != planned_target_count {
        failures.push(format!(
            "compiler-suite planned {planned_target_count} root(s), but received {terminal_target_count} terminal root outcome(s)"
        ));
    }
    failures
}

/// Run one worker phase while retaining every immutable input's advisory lease.
///
/// Worker preparation borrows only manifest data from the selected payloads. Without an explicit use after the
/// worker phase, non-lexical lifetimes may release the sibling payload field that owns an advisory lock before the
/// workers finish. That would let policy pruning remove a not-yet-finished suite input. Keep the index, every shard,
/// every foundation, and every toolchain-data partition observably live until the worker phase has returned.
fn run_compiler_suite_children_with_leases_retained<T>(
    suite_lease: &OvenStoreLease,
    shards: &[CompilerSuiteShardExecution],
    foundations: &BTreeMap<String, CompilerSuiteFoundationExecution>,
    toolchain_data: &[CompilerSuiteToolchainDataExecution],
    run: impl FnOnce() -> T,
) -> T {
    let result = run();
    std::hint::black_box(suite_lease);
    for shard in shards {
        std::hint::black_box(&shard.stored);
    }
    for foundation in foundations.values() {
        std::hint::black_box(&foundation.stored);
    }
    for partition in toolchain_data {
        std::hint::black_box(&partition.stored);
    }
    result
}

/// One direct compiler-suite root after shared caller-owned inputs are ready.
///
/// It intentionally borrows the enclosing suite's still-leased immutable payload, so a worker can reconstruct its
/// target-local manifest without serially cloning the complete closure for every root before execution begins.
struct PreparedCompilerSuiteChild<'a> {
    target: &'a crate::oven::legacy_cargo::OvenCompilerTestSuiteTarget,
    closure: &'a crate::oven::legacy_cargo::OvenCompilerTestSuiteArtifactClosure,
    intent: &'a OvenBuildIntent,
    artifact_root: &'a Path,
    compiler_root: &'a Path,
    source: PathBuf,
    output: PathBuf,
    environment: BTreeMap<String, String>,
    prefer_dynamic: bool,
    binary_compile_environment: BTreeMap<String, String>,
    workspace_libraries: &'a [OvenCompilerWorkspaceLibrary],
    workspace_library_outputs: BTreeMap<OvenCompilerWorkspaceLibraryKey, OvenCallerOwnedRustcLibrary>,
    foundation_references: &'a [OvenCompilerTestSuiteFoundationReference],
    foundations: Option<&'a BTreeMap<String, CompilerSuiteFoundationExecution>>,
}

/// Select receipt-bound suite shards by their unique source-relative paths.
///
/// Target selection is intentionally a read-only projection of the admitted suite rather than a new planning
/// authority. Partitioned selection uses the publisher-recorded, digest-verified source footprints as a stable
/// scheduling signal; it never consults prior timings or persists a mutable performance profile. This keeps focused
/// diagnosis representative of the same direct-Rustc roots that a complete Oven run will execute, while making an
/// unknown or ambiguous source path fail closed.
fn compiler_suite_selected_shard_references(
    references: &[OvenCompilerTestSuiteShardReference],
    requested_targets: &[String],
    partition_index: Option<usize>,
    partition_count: Option<usize>,
) -> CliResult<Vec<OvenCompilerTestSuiteShardReference>> {
    let partition = match (partition_index, partition_count) {
        (None, None) => None,
        (Some(index), Some(count)) if count > 0 && index < count => Some((index, count)),
        (Some(_), Some(0)) => {
            return Err(CliError::failure(
                "Oven compiler-suite partition count must be greater than zero".to_string(),
            ));
        }
        (Some(index), Some(count)) => {
            return Err(CliError::failure(format!(
                "Oven compiler-suite partition index {index} must be smaller than partition count {count}"
            )));
        }
        _ => {
            return Err(CliError::failure(
                "Oven compiler-suite partition selection requires both --partition-index and --partition-count"
                    .to_string(),
            ));
        }
    };
    if partition.is_some() && !requested_targets.is_empty() {
        return Err(CliError::failure(
            "Oven compiler-suite partition selection cannot be combined with --target".to_string(),
        ));
    }
    if let Some((index, count)) = partition {
        if count > references.len() {
            return Err(CliError::failure(format!(
                "Oven compiler-suite partition count {count} exceeds the {} receipt-bound roots",
                references.len()
            )));
        }
        let mut ordered = references.to_vec();
        ordered.sort_by(|left, right| {
            right
                .source_bytes
                .cmp(&left.source_bytes)
                .then_with(|| left.target.source_relative_path.cmp(&right.target.source_relative_path))
                .then_with(|| left.identity.cmp(&right.identity))
        });
        let mut partitions = vec![Vec::new(); count];
        let mut partition_weights = vec![0_u64; count];
        for reference in ordered {
            if reference.source_bytes == 0 {
                return Err(CliError::failure(format!(
                    "receipt-bound compiler-suite root `{}` has no digest-verified source footprint; republish the Oven suite",
                    reference.target.source_relative_path
                )));
            }
            let selected_partition = partition_weights
                .iter()
                .enumerate()
                .min_by_key(|(partition_index, weight)| (**weight, *partition_index))
                .map(|(partition_index, _)| partition_index)
                .ok_or_else(|| CliError::failure("Oven compiler-suite has no partition capacity".to_string()))?;
            partition_weights[selected_partition] = partition_weights[selected_partition]
                .checked_add(reference.source_bytes)
                .ok_or_else(|| {
                    CliError::failure("Oven compiler-suite partition source footprint overflowed".to_string())
                })?;
            partitions[selected_partition].push(reference);
        }
        let mut selected = partitions
            .get(index)
            .cloned()
            .ok_or_else(|| CliError::failure("Oven compiler-suite partition index is unavailable".to_string()))?;
        selected.sort_by(|left, right| {
            left.target
                .source_relative_path
                .cmp(&right.target.source_relative_path)
                .then_with(|| left.identity.cmp(&right.identity))
        });
        return Ok(selected);
    }
    if requested_targets.is_empty() {
        return Ok(references.to_vec());
    }

    let requested = requested_targets
        .iter()
        .map(|target| target.trim())
        .map(|target| {
            (!target.is_empty())
                .then_some(target.to_string())
                .ok_or_else(|| CliError::failure("Oven compiler-suite target selection cannot be empty".to_string()))
        })
        .collect::<CliResult<BTreeSet<_>>>()?;
    let known = references
        .iter()
        .map(|reference| reference.target.source_relative_path.clone())
        .collect::<BTreeSet<_>>();
    let unknown = requested.difference(&known).cloned().collect::<Vec<_>>();
    if !unknown.is_empty() {
        return Err(CliError::failure(format!(
            "stored Oven compiler suite has no receipt-bound target source path(s): {}",
            unknown.join(", ")
        )));
    }

    Ok(references
        .iter()
        .filter(|reference| requested.contains(&reference.target.source_relative_path))
        .cloned()
        .collect())
}

/// Resolve and validate the deliberately narrow compiler-suite exact-test diagnostic selection.
///
/// A full suite remains the correctness gate. Exact execution exists only to attribute known cases without
/// needlessly recompiling the same expensive root, so every name must select one receipt-bound target. This does not
/// create a separate suite definition or relax ordinary coverage. Cargo can publish Rustc and Rustdoc roots for the
/// same source target; in that one companion case, exact execution selects the unique native libtest root.
fn compiler_suite_exact_test_selection(
    requested: &[String],
    mut selected_targets: Vec<OvenCompilerTestSuiteShardReference>,
    partition_selected: bool,
) -> CliResult<(Vec<OvenCompilerTestSuiteShardReference>, Option<Vec<String>>)> {
    if requested.is_empty() {
        return Ok((selected_targets, None));
    }
    if partition_selected {
        return Err(CliError::failure(
            "Oven compiler-suite exact-test selection cannot be combined with receipt partition selection".to_string(),
        ));
    }
    if let Some(rustc_index) = compiler_suite_exact_rustc_companion_index(&selected_targets) {
        let rustc_target = selected_targets.swap_remove(rustc_index);
        selected_targets.clear();
        selected_targets.push(rustc_target);
    }
    let [selected_target] = selected_targets.as_slice() else {
        return Err(CliError::failure(
            "Oven compiler-suite --exact requires exactly one receipt-bound --target".to_string(),
        ));
    };
    if selected_target.target.runner != "rustc-test" {
        return Err(CliError::failure(format!(
            "Oven compiler-suite --exact requires a receipt-bound rustc-test target; `{}` uses `{}`",
            selected_target.target.source_relative_path, selected_target.target.runner
        )));
    }
    let mut seen = BTreeSet::new();
    for name in requested {
        let name = name.trim();
        if name.is_empty() {
            return Err(CliError::failure(
                "Oven compiler-suite exact-test selection cannot be empty".to_string(),
            ));
        }
        if !seen.insert(name.to_string()) {
            return Err(CliError::failure(format!(
                "Oven compiler-suite exact-test selection `{name}` is duplicated"
            )));
        }
    }
    Ok((selected_targets, Some(seen.into_iter().collect())))
}

/// Return the native member of one Rustc/Rustdoc companion pair for the same Cargo target.
fn compiler_suite_exact_rustc_companion_index(
    selected_targets: &[OvenCompilerTestSuiteShardReference],
) -> Option<usize> {
    let [first, second] = selected_targets else {
        return None;
    };
    let (rustc_index, rustdoc_index) = match (first.target.runner.as_str(), second.target.runner.as_str()) {
        ("rustc-test", "rustdoc-test") => (0, 1),
        ("rustdoc-test", "rustc-test") => (1, 0),
        _ => return None,
    };
    let rustc_target = &selected_targets[rustc_index].target;
    let rustdoc_target = &selected_targets[rustdoc_index].target;
    (rustc_target.package_name == rustdoc_target.package_name
        && rustc_target.target_name == rustdoc_target.target_name
        && rustc_target.target_kind == rustdoc_target.target_kind
        && rustc_target.source_relative_path == rustdoc_target.source_relative_path)
        .then_some(rustc_index)
}

/// Resolve every shard named by a schema-9 compiler-suite index before baking the compiler CLI or starting a child.
///
/// An index is execution authority only when each reference, manifest, and payload agrees with the current receipt.
/// The batch store selection retains all active leases, so an admission under aggregate or compatibility-domain
/// pressure cannot prune an unstarted root in the same scheduled test batch.
fn select_compiler_suite_shards(
    store: &OvenStore,
    receipt: &OvenReceipt,
    references: &[OvenCompilerTestSuiteShardReference],
    expected_schema_version: u32,
) -> CliResult<Vec<CompilerSuiteShardExecution>> {
    if references.is_empty() {
        return Err(CliError::failure(
            "schema-9 Oven compiler-suite index has no target shards".to_string(),
        ));
    }
    let mut identities = BTreeSet::new();
    let mut targets = BTreeSet::new();
    for reference in references {
        if reference.identity.is_empty() {
            return Err(CliError::failure(
                "schema-9 Oven compiler-suite index contains an empty shard identity".to_string(),
            ));
        }
        if !identities.insert(reference.identity.clone()) {
            return Err(CliError::failure(format!(
                "schema-9 Oven compiler-suite index repeats shard identity `{}`",
                reference.identity
            )));
        }
        if !targets.insert(reference.target.clone()) {
            return Err(CliError::failure(format!(
                "schema-9 Oven compiler-suite index repeats target `{}/{}/{}`",
                reference.target.package_name, reference.target.target_kind, reference.target.target_name
            )));
        }
    }
    let identities = references
        .iter()
        .map(|reference| reference.identity.clone())
        .collect::<Vec<_>>();
    let selected = store.select_payloads_for_execution(&identities).map_err(oven_error)?;
    let mut shards = Vec::with_capacity(references.len());
    for (reference, stored) in references.iter().zip(selected) {
        if stored.manifest.kind != OvenArtifactKind::CompilerTestSuiteShard
            || stored.manifest.build_unit_identity != receipt.build_unit_identity
            || stored.manifest.intent != receipt.intent
        {
            return Err(CliError::failure(format!(
                "schema-9 Oven compiler-suite shard `{}` is not authorized by the current compiler receipt",
                reference.identity
            )));
        }
        let payload =
            serde_json::from_slice::<OvenCompilerTestSuiteShardPayload>(&stored.payload).map_err(|error| {
                CliError::failure(format!(
                    "stored Oven compiler-suite shard `{}` payload is invalid: {error}",
                    reference.identity
                ))
            })?;
        if payload.schema_version != expected_schema_version {
            return Err(CliError::failure(format!(
                "stored Oven compiler-suite shard `{}` schema {} does not match suite schema expectation {}",
                reference.identity, payload.schema_version, expected_schema_version
            )));
        }
        if payload.target.package_name.is_empty()
            || payload.target.target_name.is_empty()
            || payload.target.target_kind.is_empty()
            || payload.target.runner.is_empty()
            || payload.target.source_relative_path.is_empty()
        {
            return Err(CliError::failure(format!(
                "stored Oven compiler-suite shard `{}` has an incomplete target key",
                reference.identity
            )));
        }
        if payload.target_key() != reference.target {
            return Err(CliError::failure(format!(
                "stored Oven compiler-suite shard `{}` does not match its index target key",
                reference.identity
            )));
        }
        shards.push(CompilerSuiteShardExecution { stored, payload });
    }
    if shards.len() != references.len() {
        return Err(CliError::failure(
            "schema-9 Oven compiler-suite shard selection returned an incomplete batch".to_string(),
        ));
    }
    Ok(shards)
}

/// Select every foundation named by a schema-10 index before starting its first compiler child.
fn select_compiler_suite_foundations(
    store: &OvenStore,
    receipt: &OvenReceipt,
    references: &[OvenCompilerTestSuiteFoundationReference],
) -> CliResult<BTreeMap<String, CompilerSuiteFoundationExecution>> {
    if references.is_empty() {
        return Err(CliError::failure(
            "schema-10 Oven compiler-suite index has no dependency foundations".to_string(),
        ));
    }
    let mut identities = BTreeSet::new();
    let mut labels = BTreeSet::new();
    for reference in references {
        if reference.identity.is_empty() || reference.label.is_empty() {
            return Err(CliError::failure(
                "schema-10 Oven compiler-suite index contains an incomplete foundation reference".to_string(),
            ));
        }
        if !identities.insert(reference.identity.clone()) || !labels.insert(reference.label.clone()) {
            return Err(CliError::failure(
                "schema-10 Oven compiler-suite index repeats a foundation identity or label".to_string(),
            ));
        }
    }
    let selected = store
        .select_payloads_for_execution(
            &references
                .iter()
                .map(|reference| reference.identity.clone())
                .collect::<Vec<_>>(),
        )
        .map_err(oven_error)?;
    let mut foundations = BTreeMap::new();
    for (reference, stored) in references.iter().zip(selected) {
        if stored.manifest.kind != OvenArtifactKind::CompilerTestSuiteFoundation
            || stored.manifest.build_unit_identity != receipt.build_unit_identity
            || stored.manifest.intent != receipt.intent
        {
            return Err(CliError::failure(format!(
                "schema-10 Oven compiler-suite foundation `{}` is not authorized by the current compiler receipt",
                reference.identity
            )));
        }
        let payload =
            serde_json::from_slice::<OvenCompilerTestSuiteFoundationPayload>(&stored.payload).map_err(|error| {
                CliError::failure(format!(
                    "stored Oven compiler-suite foundation `{}` payload is invalid: {error}",
                    reference.identity
                ))
            })?;
        if payload.schema_version != OVEN_COMPILER_TEST_SUITE_FOUNDATION_SCHEMA_VERSION
            || payload.label != reference.label
            || payload.artifact_closure.supporting_artifacts.is_empty()
        {
            return Err(CliError::failure(format!(
                "stored Oven compiler-suite foundation `{}` does not match its index reference",
                reference.identity
            )));
        }
        foundations.insert(
            reference.identity.clone(),
            CompilerSuiteFoundationExecution { stored, payload },
        );
    }
    Ok(foundations)
}

/// Select every compiler-Loaf data partition before the first suite child can run.
fn select_compiler_suite_toolchain_data(
    store: &OvenStore,
    receipt: &OvenReceipt,
    references: &[OvenCompilerTestSuiteToolchainDataReference],
) -> CliResult<Vec<CompilerSuiteToolchainDataExecution>> {
    if references.is_empty() {
        return Err(CliError::failure(
            "schema-13 Oven compiler-suite index has no Loaf data partitions".to_string(),
        ));
    }
    let mut identities = BTreeSet::new();
    let mut labels = BTreeSet::new();
    for reference in references {
        if reference.identity.is_empty() || reference.label.is_empty() {
            return Err(CliError::failure(
                "schema-13 Oven compiler-suite index contains an incomplete Loaf data reference".to_string(),
            ));
        }
        if !identities.insert(reference.identity.clone()) || !labels.insert(reference.label.clone()) {
            return Err(CliError::failure(
                "schema-13 Oven compiler-suite index repeats a Loaf data identity or label".to_string(),
            ));
        }
    }
    let selected = store
        .select_payloads_for_execution(
            &references
                .iter()
                .map(|reference| reference.identity.clone())
                .collect::<Vec<_>>(),
        )
        .map_err(oven_error)?;
    let mut partitions = Vec::with_capacity(references.len());
    for (reference, stored) in references.iter().zip(selected) {
        if stored.manifest.kind != OvenArtifactKind::CompilerTestSuiteToolchainData
            || stored.manifest.build_unit_identity != receipt.build_unit_identity
            || stored.manifest.intent != receipt.intent
        {
            return Err(CliError::failure(format!(
                "schema-13 Oven compiler-suite Loaf data `{}` is not authorized by the current compiler receipt",
                reference.identity
            )));
        }
        let payload =
            serde_json::from_slice::<OvenCompilerTestSuiteToolchainDataPayload>(&stored.payload).map_err(|error| {
                CliError::failure(format!(
                    "stored Oven compiler-suite Loaf data `{}` payload is invalid: {error}",
                    reference.identity
                ))
            })?;
        if payload.schema_version != OVEN_COMPILER_TEST_SUITE_TOOLCHAIN_DATA_SCHEMA_VERSION
            || payload.label != reference.label
            || stored.manifest.materialized_files.is_empty()
        {
            return Err(CliError::failure(format!(
                "stored Oven compiler-suite Loaf data `{}` does not match its index reference",
                reference.identity
            )));
        }
        partitions.push(CompilerSuiteToolchainDataExecution { stored, payload });
    }
    Ok(partitions)
}

/// Copy separately leased Loaf partitions into caller-owned suite output.
///
/// A stored-suite child expects one compiler-data root. Copy rather than hard-link the verified immutable files: a
/// test process runs with the developer's uid, and a writable hard link would let it mutate the Oven store. This
/// caller output is deliberately ephemeral and outside the store's retained physical-policy accounting.
fn materialize_compiler_suite_toolchain_data(
    output_directory: &Path,
    partitions: &[CompilerSuiteToolchainDataExecution],
) -> CliResult<PathBuf> {
    if partitions.is_empty() {
        return Err(CliError::failure(
            "schema-13 Oven compiler suite selected no Loaf data partitions".to_string(),
        ));
    }
    let root = output_directory.join("toolchain-data");
    if root.exists() {
        return Err(CliError::failure(format!(
            "compiler-suite caller output already contains Loaf data at {}; use a fresh output directory",
            root.display()
        )));
    }
    let mut seen_paths = BTreeSet::new();
    for partition in partitions {
        for file in &partition.stored.manifest.materialized_files {
            if !seen_paths.insert(file.relative_path.clone()) {
                return Err(CliError::failure(format!(
                    "schema-13 Oven compiler-suite Loaf partition `{}` overlaps another partition at `{}`",
                    partition.payload.label, file.relative_path
                )));
            }
            let source = compiler_suite_file(
                &partition.stored.artifact_root,
                &file.relative_path,
                &file.digest,
                "Loaf data file",
            )?;
            let destination = compiler_suite_output_path(&root, &file.relative_path, "Loaf data file")?;
            let parent = destination.parent().ok_or_else(|| {
                CliError::failure(format!(
                    "Loaf caller destination {} has no parent",
                    destination.display()
                ))
            })?;
            fs::create_dir_all(parent).map_err(|error| {
                CliError::failure(format!(
                    "cannot create Loaf caller directory {}: {error}",
                    parent.display()
                ))
            })?;
            fs::copy(&source, &destination).map_err(|error| {
                CliError::failure(format!(
                    "cannot copy verified Loaf data {} to {}: {error}",
                    source.display(),
                    destination.display()
                ))
            })?;
            let mut permissions = fs::metadata(&destination)
                .map_err(|error| {
                    CliError::failure(format!(
                        "cannot inspect copied Loaf data {}: {error}",
                        destination.display()
                    ))
                })?
                .permissions();
            permissions.set_readonly(true);
            fs::set_permissions(&destination, permissions).map_err(|error| {
                CliError::failure(format!(
                    "cannot make copied Loaf data {} read-only: {error}",
                    destination.display()
                ))
            })?;
        }
    }
    compiler_suite_directory(output_directory, "toolchain-data", "Loaf data")
}

/// Form a target-specific direct-rustc plan from the exact foundations named by one thin schema-10 shard.
fn compiler_suite_composed_artifact_plan(
    artifacts: &OvenRustcArtifactManifest,
    references: &[OvenCompilerTestSuiteFoundationReference],
    foundations: &BTreeMap<String, CompilerSuiteFoundationExecution>,
    intent: &OvenBuildIntent,
) -> CliResult<OvenRustcArtifactPlan> {
    if references.is_empty() {
        return Err(CliError::failure(
            "schema-10 Oven compiler-suite shard has no dependency foundations".to_string(),
        ));
    }
    let mut identities = BTreeSet::new();
    let mut labels = BTreeSet::new();
    let mut roots = Vec::with_capacity(references.len());
    for reference in references {
        if !identities.insert(reference.identity.clone()) || !labels.insert(reference.label.clone()) {
            return Err(CliError::failure(
                "schema-10 Oven compiler-suite shard repeats a foundation reference".to_string(),
            ));
        }
        let foundation = foundations.get(&reference.identity).ok_or_else(|| {
            CliError::failure(format!(
                "schema-10 Oven compiler-suite shard requires unselected foundation `{}`",
                reference.identity
            ))
        })?;
        if foundation.payload.label != reference.label {
            return Err(CliError::failure(format!(
                "schema-10 Oven compiler-suite shard foundation `{}` has a mismatched label",
                reference.identity
            )));
        }
        roots.push(OvenTrustedRustcArtifactRoot {
            artifact_root: &foundation.stored.artifact_root,
            dependency_search_paths: &foundation.payload.artifact_closure.dependency_search_paths,
            native_search_paths: &foundation.payload.artifact_closure.native_search_paths,
            supporting_artifacts: &foundation.payload.artifact_closure.supporting_artifacts,
        });
    }
    artifacts
        .materialize_trusted_store_composed(&roots, intent)
        .map_err(oven_error)
}

/// Materialize the direct-Rustc workspace library/proc-macro DAG declared by one future schema-11 execution unit.
///
/// These outputs deliberately remain caller-owned below `output_directory`: a workspace crate is compiler source,
/// not a third-party Cargo artifact that may be copied into an Oven compatibility domain. Every downstream direct
/// Rustc invocation receives the selected files as explicit `--extern` inputs whose digests participate in its
/// output-reuse receipt.
#[allow(clippy::too_many_arguments)]
fn bake_planned_compiler_suite_workspace_libraries(
    libraries: &[OvenCompilerWorkspaceLibrary],
    closure: &crate::oven::legacy_cargo::OvenCompilerTestSuiteArtifactClosure,
    intent: &OvenBuildIntent,
    receipt: &OvenReceipt,
    artifact_root: &Path,
    rustc: &Path,
    compiler_root: &Path,
    output_directory: &Path,
    foundation_references: &[OvenCompilerTestSuiteFoundationReference],
    foundations: Option<&BTreeMap<String, CompilerSuiteFoundationExecution>>,
    workspace_library_cache: &mut BTreeMap<String, OvenCallerOwnedRustcLibrary>,
) -> CliResult<BTreeMap<OvenCompilerWorkspaceLibraryKey, OvenCallerOwnedRustcLibrary>> {
    let mut pending = BTreeMap::<OvenCompilerWorkspaceLibraryKey, &OvenCompilerWorkspaceLibrary>::new();
    for library in libraries {
        if !matches!(library.key.target_kind.as_str(), "lib" | "proc-macro") {
            return Err(CliError::failure(format!(
                "stored compiler-suite workspace library `{}` has unsupported target kind `{}`",
                library.key.crate_name, library.key.target_kind
            )));
        }
        if pending.insert(library.key.clone(), library).is_some() {
            return Err(CliError::failure(format!(
                "stored compiler-suite workspace library `{}` is declared more than once",
                library.key.crate_name
            )));
        }
    }
    for library in pending.values() {
        for dependency in &library.dependencies {
            if !pending.contains_key(dependency) {
                return Err(CliError::failure(format!(
                    "stored compiler-suite workspace library `{}` requires undeclared workspace library `{}`",
                    library.key.crate_name, dependency.crate_name
                )));
            }
        }
    }
    let closure_identity = compiler_suite_artifact_closure_cache_identity(closure)?;

    let mut outputs = BTreeMap::<OvenCompilerWorkspaceLibraryKey, OvenCallerOwnedRustcLibrary>::new();
    let mut output_recipe_keys = BTreeMap::<OvenCompilerWorkspaceLibraryKey, String>::new();
    while !pending.is_empty() {
        let ready = pending
            .iter()
            .filter_map(|(key, library)| {
                library
                    .dependencies
                    .iter()
                    .all(|dependency| outputs.contains_key(dependency))
                    .then_some(key.clone())
            })
            .collect::<Vec<_>>();
        if ready.is_empty() {
            let cycle = pending
                .values()
                .map(|library| library.key.crate_name.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            return Err(CliError::failure(format!(
                "stored compiler-suite workspace-library graph contains a dependency cycle among: {cycle}"
            )));
        }
        for key in ready {
            let library = pending.remove(&key).ok_or_else(|| {
                CliError::failure(format!(
                    "stored compiler-suite workspace library `{}` disappeared before scheduling",
                    key.crate_name
                ))
            })?;
            let dependencies = library
                .dependencies
                .iter()
                .map(|dependency| {
                    outputs.get(dependency).cloned().ok_or_else(|| {
                        CliError::failure(format!(
                            "stored compiler-suite workspace library `{}` requires unavailable workspace library `{}`",
                            library.key.crate_name, dependency.crate_name
                        ))
                    })
                })
                .collect::<CliResult<Vec<_>>>()?;
            let dependency_recipe_keys = library
                .dependencies
                .iter()
                .map(|dependency| {
                    output_recipe_keys.get(dependency).cloned().ok_or_else(|| {
                        CliError::failure(format!(
                            "stored compiler-suite workspace library `{}` has no recipe identity for dependency `{}`",
                            library.key.crate_name, dependency.crate_name
                        ))
                    })
                })
                .collect::<CliResult<Vec<_>>>()?;
            let cache_key = compiler_suite_workspace_library_cache_key(
                receipt,
                library,
                &closure_identity,
                foundation_references,
                &dependency_recipe_keys,
            )?;
            if let Some(cached) = workspace_library_cache.get(&cache_key) {
                outputs.insert(key, cached.clone());
                output_recipe_keys.insert(library.key.clone(), cache_key);
                continue;
            }
            let artifacts = closure.manifest_for_workspace_library(library, intent.clone());
            let mut artifact_plan = match foundations {
                Some(foundations) => {
                    compiler_suite_composed_artifact_plan(&artifacts, foundation_references, foundations, intent)?
                }
                None => artifacts
                    .materialize_trusted_store(artifact_root, intent)
                    .map_err(oven_error)?,
            };
            attach_caller_owned_rustc_libraries(&mut artifact_plan, &dependencies).map_err(oven_error)?;
            let source = compiler_suite_workspace_library_source(compiler_root, library)?;
            let output =
                output_directory
                    .join("workspace-libraries")
                    .join(compiler_suite_workspace_library_output_name(
                        outputs.len(),
                        &library.key,
                    ));
            let request = OvenTrustedDirectRustcTargetRequest {
                receipt,
                artifacts: &artifacts,
                artifact_root,
                artifact_plan: Some(&artifact_plan),
                rustc,
                source: &source,
                output: &output,
                crate_name: &library.key.crate_name,
                edition: &library.edition,
                source_evidence_key: &library.source_evidence_key,
                features: &library.key.features,
                prefer_dynamic: compiler_suite_workspace_library_uses_dylib(&library.key),
            };
            let bake = match library.key.target_kind.as_str() {
                "lib" if compiler_suite_workspace_library_uses_dylib(&library.key) => {
                    bake_trusted_direct_rustc_dylib(&request)
                }
                "lib" => bake_trusted_direct_rustc_library(&request),
                "proc-macro" => bake_trusted_direct_rustc_proc_macro(&request),
                _ => unreachable!("target kind was validated before scheduling"),
            }
            .map_err(oven_error)?;
            let output = OvenCallerOwnedRustcLibrary {
                crate_name: library.key.crate_name.clone(),
                output: bake.output,
                digest: bake.output_digest,
                expose_extern: true,
            };
            workspace_library_cache.insert(cache_key.clone(), output.clone());
            output_recipe_keys.insert(library.key.clone(), cache_key);
            outputs.insert(key, output);
        }
    }
    Ok(outputs)
}

/// Return an invocation-local direct-Rustc workspace-library recipe identity.
///
/// A cache hit is allowed only when the publisher's library declaration, immutable closure identity, selected
/// foundation identities, and direct-dependency recipe identities all agree. This shares repeated compiler DAG nodes
/// without rebuilding the closure-sized manifest on a hit, while retaining the exact immutable-input boundary.
fn compiler_suite_workspace_library_cache_key(
    receipt: &OvenReceipt,
    library: &OvenCompilerWorkspaceLibrary,
    closure_identity: &str,
    foundation_references: &[OvenCompilerTestSuiteFoundationReference],
    dependency_recipe_keys: &[String],
) -> CliResult<String> {
    serde_json::to_vec(&(
        &receipt.identity,
        library,
        closure_identity,
        foundation_references,
        dependency_recipe_keys,
    ))
    .map(|bytes| digest_bytes(&bytes))
    .map_err(|error| {
        CliError::failure(format!(
            "cannot derive direct-Rustc workspace-library cache identity: {error}"
        ))
    })
}

/// Return an invocation-local direct-Rustc binary recipe identity.
///
/// Integration roots can request the same receipt-bound helper binary from many shards. Reuse is confined to the
/// exact target declaration, immutable closure, foundations, and selected caller-owned workspace outputs.
fn compiler_suite_target_cache_key(
    receipt: &OvenReceipt,
    target: &crate::oven::legacy_cargo::OvenCompilerTestSuiteTarget,
    closure_identity: &str,
    foundation_references: &[OvenCompilerTestSuiteFoundationReference],
    workspace_library_outputs: &BTreeMap<OvenCompilerWorkspaceLibraryKey, OvenCallerOwnedRustcLibrary>,
) -> CliResult<String> {
    let workspace_outputs = workspace_library_outputs
        .iter()
        .map(|(key, library)| (key, &library.output))
        .collect::<Vec<_>>();
    serde_json::to_vec(&(
        &receipt.identity,
        target,
        closure_identity,
        foundation_references,
        workspace_outputs,
    ))
    .map(|bytes| digest_bytes(&bytes))
    .map_err(|error| CliError::failure(format!("cannot derive direct-Rustc binary cache identity: {error}")))
}

/// Digest a complete immutable compiler-suite closure once per scheduling boundary.
///
/// The closure is the full provenance input to every target manifest, but eagerly reconstructing that manifest for
/// every shared workspace library turns a cache hit into repeated closure-sized allocation and serialization. Its
/// deterministic digest preserves the same cache boundary without that work on each library node.
fn compiler_suite_artifact_closure_cache_identity(
    closure: &crate::oven::legacy_cargo::OvenCompilerTestSuiteArtifactClosure,
) -> CliResult<String> {
    serde_json::to_vec(closure)
        .map(|bytes| digest_bytes(&bytes))
        .map_err(|error| {
            CliError::failure(format!(
                "cannot derive direct-Rustc compiler-suite closure identity: {error}"
            ))
        })
}

/// Whether a direct workspace DAG includes an executable dynamic library boundary.
fn compiler_suite_workspace_outputs_include_dylib(
    workspace_library_outputs: &BTreeMap<OvenCompilerWorkspaceLibraryKey, OvenCallerOwnedRustcLibrary>,
) -> bool {
    workspace_library_outputs.values().any(|library| {
        library
            .output
            .extension()
            .is_some_and(|extension| extension == std::env::consts::DLL_SUFFIX.trim_start_matches('.'))
    })
}

/// Supply the exact dynamic workspace-library and selected-Rustc search paths required by a direct test child.
///
/// These paths are constructed from caller-owned outputs and the receipt-selected toolchain only; inherited loader
/// environment remains cleared along with the rest of the Cargo process state.
fn compiler_suite_dynamic_library_environment(
    rustc: &Path,
    workspace_library_outputs: &BTreeMap<OvenCompilerWorkspaceLibraryKey, OvenCallerOwnedRustcLibrary>,
) -> CliResult<(String, String)> {
    let (name, toolchain_value) = rustc_dynamic_library_environment(rustc).map_err(oven_error)?;
    let mut paths = BTreeSet::new();
    for library in workspace_library_outputs.values() {
        if library
            .output
            .extension()
            .is_some_and(|extension| extension == std::env::consts::DLL_SUFFIX.trim_start_matches('.'))
        {
            let parent = library.output.parent().ok_or_else(|| {
                CliError::failure(format!(
                    "direct dynamic workspace library {} has no parent directory",
                    library.output.display()
                ))
            })?;
            paths.insert(parent.to_path_buf());
        }
    }
    paths.extend(env::split_paths(&toolchain_value));
    let value = env::join_paths(paths)
        .map_err(|error| CliError::failure(format!("cannot construct direct dynamic library search path: {error}")))?
        .into_string()
        .map_err(|_| CliError::failure("direct dynamic library search path is not valid UTF-8".to_string()))?;
    Ok((name, value))
}

/// Attach a target's declared direct-Rustc workspace inputs after every prerequisite has been materialized.
fn attach_compiler_suite_target_workspace_libraries(
    artifact_plan: &mut OvenRustcArtifactPlan,
    target: &crate::oven::legacy_cargo::OvenCompilerTestSuiteTarget,
    libraries: &[OvenCompilerWorkspaceLibrary],
    outputs: &BTreeMap<OvenCompilerWorkspaceLibraryKey, OvenCallerOwnedRustcLibrary>,
) -> CliResult<()> {
    /// Visit one workspace library and its dependencies in direct-rustc order.
    fn visit(
        key: &OvenCompilerWorkspaceLibraryKey,
        target_name: &str,
        plans: &BTreeMap<OvenCompilerWorkspaceLibraryKey, &OvenCompilerWorkspaceLibrary>,
        outputs: &BTreeMap<OvenCompilerWorkspaceLibraryKey, OvenCallerOwnedRustcLibrary>,
        visiting: &mut BTreeSet<OvenCompilerWorkspaceLibraryKey>,
        selected: &mut BTreeSet<OvenCompilerWorkspaceLibraryKey>,
        libraries: &mut Vec<OvenCallerOwnedRustcLibrary>,
    ) -> CliResult<()> {
        if !visiting.insert(key.clone()) {
            return Err(CliError::failure(format!(
                "stored compiler-suite target `{target_name}` has a cyclic workspace-library dependency at `{}`",
                key.crate_name
            )));
        }
        let plan = plans.get(key).ok_or_else(|| {
            CliError::failure(format!(
                "stored compiler-suite target `{target_name}` requires undeclared workspace library `{}`",
                key.crate_name
            ))
        })?;
        for dependency in &plan.dependencies {
            visit(dependency, target_name, plans, outputs, visiting, selected, libraries)?;
        }
        visiting.remove(key);
        if selected.insert(key.clone()) {
            libraries.push(outputs.get(key).cloned().ok_or_else(|| {
                CliError::failure(format!(
                    "stored compiler-suite target `{target_name}` requires unmaterialized workspace library `{}`",
                    key.crate_name
                ))
            })?);
        }
        Ok(())
    }

    let plans = libraries
        .iter()
        .map(|library| (library.key.clone(), library))
        .collect::<BTreeMap<_, _>>();
    if plans.len() != libraries.len() {
        return Err(CliError::failure(
            "stored compiler-suite workspace-library plan repeats a library key".to_string(),
        ));
    }
    let mut visiting = BTreeSet::new();
    let mut selected = BTreeSet::new();
    let mut selected_libraries = Vec::new();
    for key in &target.workspace_library_dependencies {
        visit(
            key,
            &target.target_name,
            &plans,
            outputs,
            &mut visiting,
            &mut selected,
            &mut selected_libraries,
        )?;
    }
    attach_caller_owned_rustc_libraries(artifact_plan, &selected_libraries).map_err(oven_error)
}

/// Select the one stored compiler-suite executable pair authorized by the current receipt.
fn select_compiler_test_suite(
    store: &OvenStore,
    receipt: &OvenReceipt,
    compiler_root: &Path,
    rustc: &Path,
) -> CliResult<OvenStoreExecutionPayload> {
    let selected = store
        .select_payloads_matching_for_execution(|manifest| {
            manifest.kind == OvenArtifactKind::CompilerTestSuite
                && manifest.build_unit_identity == receipt.build_unit_identity
                && manifest.intent == receipt.intent
        })
        .map_err(oven_error)?;
    let mut current_schema = Vec::new();
    for candidate in selected {
        let payload = serde_json::from_slice::<OvenCompilerTestSuitePayload>(&candidate.payload).map_err(|error| {
            CliError::failure(format!(
                "stored Oven compiler suite {} has an invalid payload: {error}",
                candidate.manifest.identity
            ))
        })?;
        if payload.schema_version == OVEN_COMPILER_TEST_SUITE_SCHEMA_VERSION {
            current_schema.push(candidate);
        }
    }
    match current_schema.len() {
        1 => Ok(current_schema.remove(0)),
        0 => Err(CliError::failure(format!(
            "no current-schema Oven compiler test suite is prepared for this exact receipt. Republish the explicit Oven suite for {} with rustc {}.",
            compiler_root.display(),
            rustc.display(),
        ))),
        _ => Err(CliError::failure(format!(
            "multiple current-schema Oven compiler test suites are prepared for one build unit: {}",
            current_schema
                .iter()
                .map(|entry| entry.manifest.identity.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ))),
    }
}

/// Resolve and verify one regular, non-symlink file below the active leased Oven artifact root.
fn compiler_suite_file(
    artifact_root: &Path,
    relative_path: &str,
    expected_digest: &str,
    role: &str,
) -> CliResult<PathBuf> {
    let mut path = artifact_root.to_path_buf();
    for component in Path::new(relative_path).components() {
        let Component::Normal(component) = component else {
            return Err(CliError::failure(format!(
                "stored Oven compiler suite contains an unsafe {role} path `{relative_path}`"
            )));
        };
        path.push(component);
    }
    let metadata = fs::symlink_metadata(&path).map_err(|error| {
        CliError::failure(format!(
            "cannot read stored Oven compiler {role} {}: {error}",
            path.display()
        ))
    })?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(CliError::failure(format!(
            "stored Oven compiler {role} {} is not a regular file",
            path.display()
        )));
    }
    let bytes = fs::read(&path).map_err(|error| {
        CliError::failure(format!(
            "cannot read stored Oven compiler {role} {}: {error}",
            path.display()
        ))
    })?;
    if digest_bytes(&bytes) != expected_digest {
        return Err(CliError::failure(format!(
            "stored Oven compiler {role} {} does not match its receipt-bound digest",
            path.display()
        )));
    }
    Ok(path)
}

/// Resolve one safe caller-output path without permitting a stored relative path to escape its Loaf root.
fn compiler_suite_output_path(root: &Path, relative_path: &str, role: &str) -> CliResult<PathBuf> {
    let mut path = root.to_path_buf();
    for component in Path::new(relative_path).components() {
        let Component::Normal(component) = component else {
            return Err(CliError::failure(format!(
                "stored Oven compiler suite contains an unsafe {role} path `{relative_path}`"
            )));
        };
        path.push(component);
    }
    Ok(path)
}

/// Resolve one real directory below the active leased Oven artifact root.
///
/// Suite payloads name only a store-relative directory. The Loaf reader still validates the selected Loaf and
/// every referenced artifact, while this check prevents a direct-rustc child from inheriting a path outside the
/// immutable entry.
fn compiler_suite_directory(artifact_root: &Path, relative_path: &str, role: &str) -> CliResult<PathBuf> {
    let mut path = artifact_root.to_path_buf();
    for component in Path::new(relative_path).components() {
        let Component::Normal(component) = component else {
            return Err(CliError::failure(format!(
                "stored Oven compiler suite contains an unsafe {role} path `{relative_path}`"
            )));
        };
        path.push(component);
    }
    let metadata = fs::symlink_metadata(&path).map_err(|error| {
        CliError::failure(format!(
            "cannot read stored Oven compiler {role} directory {}: {error}",
            path.display()
        ))
    })?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(CliError::failure(format!(
            "stored Oven compiler {role} directory {} is not a real directory",
            path.display()
        )));
    }
    let loafs = path.join("share/incan/oven/loafs");
    let native_metadata = fs::symlink_metadata(&loafs).map_err(|error| {
        CliError::failure(format!(
            "stored Oven compiler {role} directory {} has no Loaf layout: {error}",
            path.display()
        ))
    })?;
    if !native_metadata.is_dir() || native_metadata.file_type().is_symlink() {
        return Err(CliError::failure(format!(
            "stored Oven compiler {role} directory {} has no real Loaf layout",
            path.display()
        )));
    }
    Ok(path)
}

/// Persist the full caller-owned libtest transcript before returning its bounded terminal summary.
fn write_native_test_failure_transcript(output: &Path, transcript: &str) -> CliResult<PathBuf> {
    let path = output.with_extension("libtest-output.txt");
    fs::write(&path, transcript).map_err(|error| {
        CliError::failure(format!(
            "failed to retain Oven native-test transcript {}: {error}",
            path.display()
        ))
    })?;
    Ok(path)
}

/// Collect every test libtest reported as failing, in the order it first named them.
///
/// Two spellings have to be read because the suite runs libtest in both modes. With output captured, each failure
/// body is introduced by `---- <name> stdout ----`. With output passed through, no such header is printed and the
/// only place the test is named is the panic line, where libtest has set the thread name to the test name. Reading
/// both keeps the roster complete in either mode, and de-duplicating keeps one entry per test when both appear.
fn failing_test_names(output: &str) -> Vec<&str> {
    let mut names = Vec::new();
    for line in output.lines() {
        let trimmed = line.trim();
        let captured = trimmed
            .strip_prefix("---- ")
            .and_then(|rest| rest.strip_suffix(" stdout ----"));
        let passed_through = trimmed
            .contains("panicked at")
            .then(|| trimmed.split_once("thread '"))
            .flatten()
            .and_then(|(_, rest)| rest.split_once('\''))
            .map(|(name, _)| name);
        let Some(name) = captured.or(passed_through).map(str::trim) else {
            continue;
        };
        // A panic on the harness thread names the runner, not a test; it would add noise to every roster.
        if name.is_empty() || name == "main" || names.contains(&name) {
            continue;
        }
        names.push(name);
    }
    names
}

/// Keep terminal failure reporting actionable without dumping an unbounded libtest transcript into the CLI error path.
///
/// The roster of failing test names is emitted first and is never truncated: a handful of large panic bodies must not
/// be able to spend the character budget and leave the remaining failures invisible to whoever reads CI output. The
/// complete caller-owned transcript is retained beside the direct-rustc test binary on failure.
fn native_test_failure_summary(output: &str) -> String {
    const MAX_CHARS: usize = 12_000;
    const PANIC_CONTEXT_LINES: usize = 12;

    // ---- Roster: which tests failed, ahead of any bounded detail ----
    let mut summary = String::new();
    let failing = failing_test_names(output);
    if !failing.is_empty() {
        summary.push_str(&format!("failing tests ({}):\n", failing.len()));
        for name in &failing {
            summary.push_str("    ");
            summary.push_str(name);
            summary.push('\n');
        }
        summary.push('\n');
    }

    // ---- Detail: panic sites, their immediate context, and terminal libtest lines ----
    let mut relevant = Vec::new();
    let mut panic_context_remaining = 0;
    for line in output.lines() {
        let is_relevant = line.contains("FAILED")
            || line.contains("panicked")
            || line.contains("Error:")
            || line.starts_with("error:")
            || line.contains("test result:");
        if is_relevant || panic_context_remaining > 0 {
            relevant.push(line);
        }
        if line.contains("panicked") {
            panic_context_remaining = PANIC_CONTEXT_LINES;
        } else {
            panic_context_remaining = panic_context_remaining.saturating_sub(1);
        }
        if relevant.len() == 96 {
            break;
        }
    }
    let relevant = relevant.join("\n");
    let detail = if relevant.is_empty() { output } else { &relevant };
    summary.extend(detail.chars().take(MAX_CHARS));
    if detail.chars().count() > MAX_CHARS {
        summary.push_str("\n… libtest transcript truncated");
    }
    summary
}

/// Recompute the source-bound compiler root libtest receipt without invoking Cargo.
fn compiler_libtests_receipt(
    compiler_root: &Path,
    rustc: &Path,
    requested_features: &[String],
    loaf_root: Option<&Path>,
) -> CliResult<(OvenReceipt, PathBuf)> {
    let target = rustc_host_target(rustc).map_err(oven_error)?;
    let toolchain = rustc_identity(rustc).map_err(oven_error)?;
    let features = compiler_root_feature_selection(compiler_root, requested_features)?;
    let mut request =
        OvenCompilerSuiteRequest::new(compiler_root, target, toolchain, OVEN_COMPILER_TEST_PROFILE, features);
    if let Some(loaf_root) = loaf_root {
        let compatibility_identity =
            crate::oven::loaf::committed_loaf_envelope_compatibility_identity(loaf_root, "compiler-suite")
                .map_err(oven_error)?;
        request = request.with_loaf_compatibility_identity(compatibility_identity);
    }
    let receipt = receipt_native_compiler_suite(&request).map_err(oven_error)?;
    Ok((receipt, compiler_root.join(COMPILER_LIBTEST_RECEIPT_RELATIVE_PATH)))
}

/// Resolve the root package's enabled feature closure from its checked-in manifest, without asking Cargo to plan it.
///
/// The explicit compiler-suite publisher uses the same resolved set both for Cargo's one permitted preparation and
/// for the direct-rustc `--cfg feature=...` consumer. Dependency feature requests (`dep/feature`) are deliberately
/// ignored here because they are not root-package `cfg(feature)` values.
fn compiler_root_feature_selection(compiler_root: &Path, requested_features: &[String]) -> CliResult<Vec<String>> {
    let manifest = compiler_root.join("Cargo.toml");
    let content = fs::read_to_string(&manifest).map_err(|error| {
        CliError::failure(format!(
            "failed to read compiler Cargo manifest {}: {error}",
            manifest.display()
        ))
    })?;
    let document = toml::from_str::<toml::Value>(&content).map_err(|error| {
        CliError::failure(format!(
            "failed to parse compiler Cargo manifest {}: {error}",
            manifest.display()
        ))
    })?;
    let declared = document
        .get("features")
        .and_then(toml::Value::as_table)
        .ok_or_else(|| {
            CliError::failure(format!(
                "compiler Cargo manifest {} has no [features] table",
                manifest.display()
            ))
        })?;
    let mut pending = vec!["default".to_string()];
    pending.extend(requested_features.iter().cloned());
    let mut enabled = std::collections::BTreeSet::new();
    while let Some(feature) = pending.pop() {
        let feature = feature.trim();
        if feature.is_empty()
            || feature.starts_with("dep:")
            || feature.contains('/')
            || !enabled.insert(feature.to_string())
        {
            continue;
        }
        let values = declared.get(feature).and_then(toml::Value::as_array).ok_or_else(|| {
            CliError::failure(format!(
                "compiler Cargo manifest {} does not declare feature `{feature}`",
                manifest.display()
            ))
        })?;
        for value in values {
            let value = value.as_str().ok_or_else(|| {
                CliError::failure(format!(
                    "compiler Cargo manifest {} has a non-string feature member",
                    manifest.display()
                ))
            })?;
            if !value.starts_with("dep:") && !value.contains('/') {
                pending.push(value.to_string());
            }
        }
    }
    enabled.remove("default");
    Ok(enabled.into_iter().collect())
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
    let bake = bake_stored_direct_rustc_run(&OvenStoredDirectRustcRunRequest {
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
    let executable = bake.output.clone();
    let mut command = std::process::Command::new(&executable);
    command.args(&options.arguments);
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
    match options.format {
        OvenOutputFormat::Text => println!("Oven ran {} without a Cargo consumer.", executable.display()),
        OvenOutputFormat::Json => print_json(&serde_json::json!({
            "executable": executable,
            "cargo_process_started": bake.cargo_process_started,
            "reused": bake.reused,
        }))?,
    }
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::{
        CompilerSuiteChildrenReport, CompilerSuiteFixtureCargoProxy, CompilerSuiteNativeTestRootReport,
        CompilerSuiteRustdocTestRootReport, CompilerSuiteTimingReport,
        DEFAULT_OVEN_COMPILER_SUITE_MAX_DOMAIN_LOGICAL_BYTES, DEFAULT_OVEN_COMPILER_SUITE_MAX_DOMAIN_PHYSICAL_BYTES,
        DEFAULT_OVEN_COMPILER_SUITE_MAX_PHYSICAL_BYTES, DEFAULT_OVEN_MAX_DOMAIN_LOGICAL_BYTES,
        DEFAULT_OVEN_MAX_DOMAIN_PHYSICAL_BYTES, DEFAULT_OVEN_MAX_PHYSICAL_BYTES, OvenCompilerSuiteTargetCapabilities,
        OvenImportCommandOptions, OvenLoafBakeCommandOptions, OvenPlanPublishCommandOptions, OvenRunCommandOptions,
        OvenStoreCommandOptions, OvenTestCommandOptions, apply_compiler_suite_target_capabilities,
        attach_compiler_suite_target_workspace_libraries, bake_planned_compiler_suite_binaries,
        bake_planned_compiler_suite_workspace_libraries, compiler_suite_auto_parallel_jobs,
        compiler_suite_child_state_root, compiler_suite_cli_output, compiler_suite_completion_failures,
        compiler_suite_directory, compiler_suite_environment, compiler_suite_environment_path,
        compiler_suite_exact_test_selection, compiler_suite_file, compiler_suite_libtest_threads,
        compiler_suite_remove_generated_rust_closure, compiler_suite_selected_shard_references,
        compiler_suite_selection_context, compiler_suite_selection_report, compiler_suite_temporary_directory,
        compiler_suite_uses_indexed_foundations, compiler_suite_workspace_library_dependency_closure,
        default_rustup_home, default_store_root, interop_bake_terminal_message, loaf_envelope_default_limits,
        loaf_envelope_evidence, native_test_failure_summary, oven_import, oven_publish_direct_rustc_plan, oven_run,
        oven_test, parse_named_path, prepare_compiler_suite_child, resolve_limits_with_environment_and_defaults,
        reuse_complete_loaf_envelope, run_compiler_suite_children_with_leases_retained,
        run_prepared_compiler_suite_children, select_compiler_suite_shards, write_compiler_suite_report,
        write_native_test_failure_transcript,
    };
    use crate::cli::{CliResult, OvenLoafEnvelopeArgument, OvenOutputFormat};
    use crate::oven::legacy_cargo::{
        OVEN_COMPILER_TEST_SUITE_SCHEMA_VERSION, OVEN_COMPILER_TEST_SUITE_SHARD_SCHEMA_VERSION_V1,
        OvenCompilerTestSuiteArtifactClosure, OvenCompilerTestSuiteFoundationReference, OvenCompilerTestSuitePayload,
        OvenCompilerTestSuiteShardPayload, OvenCompilerTestSuiteShardReference, OvenCompilerTestSuiteTarget,
        OvenCompilerTestSuiteTargetKey, OvenCompilerTestSuiteToolchainLoafGenerationReference,
        OvenCompilerWorkspaceLibrary, OvenCompilerWorkspaceLibraryKey,
    };
    use crate::oven::loaf::{
        OVEN_LOAF_ENVELOPE_MANIFEST_SCHEMA_VERSION, OVEN_LOAF_SCHEMA_VERSION, OvenLoaf, OvenLoafEnvelope,
        OvenLoafEnvelopeManifest, OvenLoafEnvelopeMember, OvenLoafFixtureAction, OvenLoafMemberRole,
        acquire_exclusive_loaf_generation_lock, loaf_envelope_specifications,
    };
    use crate::oven::loaf::{commit_loaf_generation, retire_unreferenced_loaf_generations};
    use crate::oven::native_test::{OvenNativeTestCaseCounts, OvenNativeTestCaseTiming};
    use crate::oven::rustc::{
        OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION, OvenRustcArtifactManifest, OvenRustcArtifactPlan,
        OvenTrustedDirectRustcTargetRequest, bake_trusted_direct_rustc_run, resolve_active_rustc,
        rustc_dynamic_library_environment, rustc_host_target,
    };
    use crate::oven::store::{OvenArtifactKind, OvenArtifactPublishRequest, OvenStore, OvenStoreLimits};
    use crate::oven::{OvenBuildIntent, digest_bytes};
    use crate::oven::{OvenCompilerSuiteRequest, receipt_native_compiler_suite};
    use std::collections::{BTreeMap, BTreeSet};
    use std::ffi::OsString;
    use std::fs;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::time::Instant;

    #[test]
    fn interop_bake_text_evidence_distinguishes_automatic_bootstrap_cargo() -> Result<(), Box<dyn std::error::Error>> {
        let report = super::OvenInteropBakeReport {
            target: "aarch64-apple-darwin".to_string(),
            base_receipt: PathBuf::from("base-receipt.json"),
            bootstrap_prepared: true,
            execution_receipt: PathBuf::from("execution-receipt.json"),
            execution_receipt_identity: "sha256:execution".to_string(),
            plan_identity: "sha256:plan".to_string(),
            final_receipt_identity: "sha256:final".to_string(),
            archives: Vec::new(),
            bundles: Vec::new(),
            reused: false,
            cargo_process_started: true,
        };

        let message = interop_bake_terminal_message(&report);

        assert!(message.contains("named compatibility publisher"));
        assert!(!message.contains("without invoking Cargo"));
        assert_eq!(serde_json::to_value(&report)?["cargo_process_started"], true);
        Ok(())
    }

    #[test]
    fn loaf_fixture_probe_pins_the_explicit_baker_authorities() {
        let mut command = Command::new("incan");
        command.env("RUSTC", "/ambient/rustc");

        super::pin_loaf_fixture_rustc(&mut command, Path::new("/explicit/stable/rustc"));
        super::isolate_loaf_fixture_toolchain_data(&mut command, Path::new("/isolated/toolchain"));

        assert_eq!(
            command
                .get_envs()
                .find(|(name, _)| *name == "RUSTC")
                .and_then(|(_, value)| value),
            Some(std::ffi::OsStr::new("/explicit/stable/rustc"))
        );
        assert_eq!(
            command
                .get_envs()
                .find(|(name, _)| *name == "INCAN_INTERNAL_OVEN_LOAF_EXECUTION")
                .and_then(|(_, value)| value),
            Some(std::ffi::OsStr::new("1"))
        );
        assert_eq!(
            command
                .get_envs()
                .find(|(name, _)| *name == "INCAN_INTERNAL_TOOLCHAIN_DATA_ROOT")
                .and_then(|(_, value)| value),
            Some(std::ffi::OsStr::new("/isolated/toolchain"))
        );
    }

    #[test]
    fn loaf_fixture_probe_accepts_only_fail_closed_native_misses() {
        use crate::oven::loaf::{
            OVEN_DEPENDENCY_MISS_SUMMARY, OVEN_NESTED_DEPENDENCY_MISS_SUMMARY, OVEN_NO_IMPLICIT_DEPENDENCY_BUILD,
        };

        for summary in [OVEN_DEPENDENCY_MISS_SUMMARY, OVEN_NESTED_DEPENDENCY_MISS_SUMMARY] {
            assert!(super::loaf_fixture_probe_is_expected_miss(&format!(
                "{summary}, and `incan build` {OVEN_NO_IMPLICIT_DEPENDENCY_BUILD}."
            )));
        }
        // A miss summary without the no-implicit-build contract describes a different failure.
        assert!(!super::loaf_fixture_probe_is_expected_miss(
            OVEN_NESTED_DEPENDENCY_MISS_SUMMARY
        ));
        assert!(!super::loaf_fixture_probe_is_expected_miss(
            "Cargo failed while preparing a native provider/dependency unit"
        ));
    }

    #[test]
    fn loaf_compiler_manifest_accepts_packaged_support_workspace() -> Result<(), Box<dyn std::error::Error>> {
        let compiler_root = tempfile::tempdir()?;
        let packaged_manifest = compiler_root.path().join("crates/Cargo.toml");
        fs::create_dir_all(packaged_manifest.parent().ok_or("packaged manifest has no parent")?)?;
        fs::write(&packaged_manifest, "[workspace]\nresolver = \"2\"\n")?;

        assert_eq!(
            super::loaf_compiler_manifest_path(compiler_root.path())?,
            packaged_manifest,
            "release packaging must bind Loaf evidence to the shipped support workspace"
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn complete_envelope_reuses_nonsemantic_churn_without_cargo() -> Result<(), Box<dyn std::error::Error>> {
        let output = tempfile::tempdir()?;
        let scratch = tempfile::tempdir()?;
        let compiler_root = tempfile::tempdir()?;
        let tools = tempfile::tempdir()?;
        fs::write(
            compiler_root.path().join("Cargo.toml"),
            "[workspace]\nresolver = \"3\"\n",
        )?;
        fs::write(compiler_root.path().join("Cargo.lock"), "version = 4\n")?;
        for crate_name in ["incan_core", "incan_derive", "incan_stdlib"] {
            let crate_root = compiler_root.path().join("crates").join(crate_name);
            fs::create_dir_all(crate_root.join("src"))?;
            fs::write(
                crate_root.join("Cargo.toml"),
                format!("[package]\nname = \"{crate_name}\"\nversion = \"0.1.0\"\n"),
            )?;
            fs::write(crate_root.join("src/lib.rs"), "pub fn fixture() {}\n")?;
        }
        let sdk_inventory = compiler_root.path().join("sdk-inventory.json");
        fs::write(&sdk_inventory, "sealed sdk inventory")?;
        let cargo_marker = tools.path().join("cargo-started");
        let cargo = tools.path().join("cargo");
        let rustc = tools.path().join("rustc");
        write_executable(
            &cargo,
            &format!(
                "#!/bin/sh\nif [ \"$1\" = --version ]; then printf 'cargo fixture\\n'; exit 0; fi\nprintf started > \"{}\"\nexit 97\n",
                cargo_marker.display()
            ),
        )?;
        write_executable(&rustc, "#!/bin/sh\nprintf 'rustc fixture\\n'\n")?;
        let compiler_one = tools.path().join("incan-one");
        let compiler_two = tools.path().join("incan-two");
        write_executable(&compiler_one, "#!/bin/sh\nprintf 'incan one\\n'\n")?;
        write_executable(&compiler_two, "#!/bin/sh\nprintf 'incan two\\n'\n")?;
        let first_evidence = loaf_envelope_evidence(
            OvenLoafEnvelope::Release,
            compiler_root.path(),
            &compiler_one,
            &sdk_inventory,
            &rustc,
        )?;
        let second_evidence = loaf_envelope_evidence(
            OvenLoafEnvelope::Release,
            compiler_root.path(),
            &compiler_two,
            &sdk_inventory,
            &rustc,
        )?;
        let nested_output = compiler_root
            .path()
            .join("crates/incan_stdlib/stdlib/components/stdlib-data/target/incan_lock/rust_inspect");
        fs::create_dir_all(&nested_output)?;
        fs::write(
            nested_output.join(".incan_rust_inspect_cache.json"),
            "mutable cache output\n",
        )?;
        let output_churn_evidence = loaf_envelope_evidence(
            OvenLoafEnvelope::Release,
            compiler_root.path(),
            &compiler_two,
            &sdk_inventory,
            &rustc,
        )?;
        let generation_identity = digest_bytes(b"generation");
        let generation = Path::new("generations").join(
            generation_identity
                .strip_prefix("sha256:")
                .unwrap_or(&generation_identity),
        );
        let mut members = Vec::new();
        for specification in loaf_envelope_specifications(OvenLoafEnvelope::Release) {
            let action = match specification.action {
                OvenLoafFixtureAction::Build => "build",
                OvenLoafFixtureAction::Run => "run",
            };
            let build_unit_identity =
                digest_bytes(format!("{}:{}", specification.label, specification.profile).as_bytes());
            let loaf = OvenLoaf {
                schema_version: OVEN_LOAF_SCHEMA_VERSION,
                build_unit_identity: build_unit_identity.clone(),
                provenance: Default::default(),
                accounting: Default::default(),
                compatibility: Default::default(),
                registry_leaves: Vec::new(),
                plan: OvenRustcArtifactManifest {
                    schema_version: OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION,
                    intent: OvenBuildIntent {
                        target: "fixture-target".to_string(),
                        toolchain: "rustc fixture".to_string(),
                        profile: specification.profile.to_string(),
                        features: Vec::new(),
                    },
                    dependency_search_paths: Vec::new(),
                    native_search_paths: Vec::new(),
                    externs: Vec::new(),
                    entrypoint_externs: BTreeMap::new(),
                    registry_leaves: Vec::new(),
                    registry_sources: Vec::new(),
                    compile_environment: BTreeMap::new(),
                    vocab_auxiliary_targets: Vec::new(),
                    supporting_artifacts: Vec::new(),
                },
            };
            let loaf_identity = digest_bytes(&serde_json::to_vec_pretty(&loaf)?);
            let relative_directory = generation.join(format!(
                "{}.loaf",
                loaf_identity.strip_prefix("sha256:").unwrap_or(&loaf_identity)
            ));
            let directory = output.path().join(&relative_directory);
            fs::create_dir_all(&directory)?;
            fs::write(directory.join("loaf.json"), serde_json::to_vec_pretty(&loaf)?)?;
            members.push(OvenLoafEnvelopeMember {
                label: specification.label.to_string(),
                profile: specification.profile.to_string(),
                action: action.to_string(),
                role: specification.role,
                build_unit_identity,
                loaf_identity,
                plan_identity: digest_bytes(&serde_json::to_vec(&loaf.plan)?),
                logical_bytes: serde_json::to_vec_pretty(&loaf)?.len() as u64,
                physical_bytes: 0,
                path: relative_directory.join("loaf.json"),
            });
        }
        fs::write(
            output.path().join("envelope.json"),
            serde_json::to_vec(&OvenLoafEnvelopeManifest {
                schema_version: OVEN_LOAF_ENVELOPE_MANIFEST_SCHEMA_VERSION,
                envelope: "release".to_string(),
                generation_identity,
                evidence: super::loaf_envelope_compatibility_map(&first_evidence),
                loafs: members,
            })?,
        )?;

        let report = reuse_complete_loaf_envelope(
            output.path(),
            scratch.path(),
            OvenLoafEnvelope::Release,
            &output_churn_evidence,
            OvenStoreLimits::new(1024 * 1024, 1024 * 1024, 1024 * 1024),
            Instant::now(),
        )?
        .ok_or("matching release compatibility must reuse the committed envelope")?;

        assert_eq!(
            report.action, "reused",
            "executable bytes are provenance, not release compatibility"
        );
        assert_eq!(
            report.reused_count,
            loaf_envelope_specifications(OvenLoafEnvelope::Release).len(),
            "a compatible release envelope must reuse every complete stdlib profile variant"
        );
        assert_ne!(
            first_evidence.compiler_executable_digest, second_evidence.compiler_executable_digest,
            "the regression requires distinct compiler executable provenance"
        );
        assert_eq!(
            first_evidence.runtime_source_digest, second_evidence.runtime_source_digest,
            "executable provenance must not affect runtime-source compatibility"
        );
        assert_eq!(
            first_evidence.runtime_source_digest, output_churn_evidence.runtime_source_digest,
            "nested runtime-crate target output must not invalidate authored runtime source"
        );
        assert!(
            !cargo_marker.exists(),
            "exact reuse after nonsemantic output churn must not start the explicit publisher"
        );
        fs::write(
            compiler_root.path().join("crates/incan_stdlib/src/lib.rs"),
            "pub fn changed_runtime() {}\n",
        )?;
        let changed_runtime_evidence = loaf_envelope_evidence(
            OvenLoafEnvelope::Release,
            compiler_root.path(),
            &compiler_two,
            &sdk_inventory,
            &rustc,
        )?;
        assert_ne!(
            first_evidence.runtime_source_digest, changed_runtime_evidence.runtime_source_digest,
            "a changed runtime source must not reuse stale compiled standard-library artifacts"
        );
        assert!(
            reuse_complete_loaf_envelope(
                output.path(),
                scratch.path(),
                OvenLoafEnvelope::Release,
                &changed_runtime_evidence,
                OvenStoreLimits::new(1024 * 1024, 1024 * 1024, 1024 * 1024),
                Instant::now(),
            )?
            .is_none(),
            "runtime-source drift must force the explicit baker path"
        );
        assert!(
            !cargo_marker.exists(),
            "reuse rejection itself must not start the explicit publisher"
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn compiler_suite_envelope_reuses_its_source_plan_without_a_second_cargo_command()
    -> Result<(), Box<dyn std::error::Error>> {
        let compiler_root = tempfile::tempdir()?;
        fs::create_dir_all(compiler_root.path().join("src"))?;
        fs::write(
            compiler_root.path().join("Cargo.toml"),
            "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[features]\ndefault = []\nlsp = []\n",
        )?;
        fs::write(compiler_root.path().join("Cargo.lock"), "version = 4\n")?;
        fs::write(compiler_root.path().join("src/lib.rs"), "pub fn fixture() {}\n")?;
        fs::write(compiler_root.path().join("src/main.rs"), "fn main() {}\n")?;
        for crate_name in ["incan_core", "incan_derive", "incan_stdlib"] {
            let crate_root = compiler_root.path().join("crates").join(crate_name);
            fs::create_dir_all(crate_root.join("src"))?;
            fs::write(
                crate_root.join("Cargo.toml"),
                format!("[package]\nname = \"{crate_name}\"\nversion = \"0.1.0\"\n"),
            )?;
            fs::write(crate_root.join("src/lib.rs"), "pub fn fixture() {}\n")?;
        }
        let sdk_inventory = compiler_root.path().join("sdk-inventory.json");
        fs::write(&sdk_inventory, "not read on exact suite reuse")?;
        let rustc = resolve_active_rustc()?;
        let output = tempfile::tempdir()?;
        fs::write(
            output.path().join("envelope.json"),
            serde_json::to_vec(&OvenLoafEnvelopeManifest {
                schema_version: OVEN_LOAF_ENVELOPE_MANIFEST_SCHEMA_VERSION,
                envelope: "compiler-suite".to_string(),
                generation_identity: digest_bytes(b"fixture compiler-suite generation"),
                evidence: BTreeMap::new(),
                loafs: Vec::new(),
            })?,
        )?;
        let (receipt, _) =
            super::compiler_libtests_receipt(compiler_root.path(), &rustc, &["lsp".to_string()], Some(output.path()))?;
        let target = OvenCompilerTestSuiteTarget {
            package_name: "fixture".to_string(),
            target_name: "fixture".to_string(),
            target_kind: "lib".to_string(),
            runner: "rustc-test".to_string(),
            source_relative_path: "src/lib.rs".to_string(),
            source_evidence_key: "compiler-suite-source:src/lib.rs".to_string(),
            crate_name: "fixture".to_string(),
            edition: "2024".to_string(),
            features: Vec::new(),
            compile_environment: BTreeMap::new(),
            binary_dependencies: Vec::new(),
            workspace_library_dependencies: Vec::new(),
            externs: Vec::new(),
        };
        let suite = OvenCompilerTestSuitePayload {
            schema_version: OVEN_COMPILER_TEST_SUITE_SCHEMA_VERSION,
            test_targets: Vec::new(),
            shard_references: vec![OvenCompilerTestSuiteShardReference {
                identity: "sha256:fixture-shard".to_string(),
                target: target.key(),
                source_bytes: 1,
            }],
            foundation_references: vec![OvenCompilerTestSuiteFoundationReference {
                identity: "sha256:fixture-foundation".to_string(),
                label: "foundation-0000".to_string(),
            }],
            toolchain_data_references: Vec::new(),
            toolchain_loaf_generation: Some(OvenCompilerTestSuiteToolchainLoafGenerationReference {
                generation_identity: "sha256:fixture-generation".to_string(),
            }),
            binary_targets: Vec::new(),
            test_artifact_closure: None,
            cli_artifact_closure: Some(OvenCompilerTestSuiteArtifactClosure {
                dependency_search_paths: Vec::new(),
                native_search_paths: Vec::new(),
                supporting_artifacts: Vec::new(),
            }),
            cli_foundation_references: Vec::new(),
            cli_target: Some(target),
            cli_workspace_libraries: Vec::new(),
            sdk_inventory_relative_path: "providers/sdk-inventory.json".to_string(),
            sdk_inventory_digest: "fixture".to_string(),
            toolchain_data_relative_root: None,
            warning_check_artifacts: OvenRustcArtifactManifest {
                schema_version: OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION,
                intent: receipt.intent.clone(),
                dependency_search_paths: Vec::new(),
                native_search_paths: Vec::new(),
                externs: Vec::new(),
                entrypoint_externs: BTreeMap::new(),
                registry_leaves: Vec::new(),
                registry_sources: Vec::new(),
                compile_environment: BTreeMap::new(),
                vocab_auxiliary_targets: Vec::new(),
                supporting_artifacts: Vec::new(),
            },
        };
        let suite_store = tempfile::tempdir()?;
        let store = OvenStore::new(
            suite_store.path(),
            OvenStoreLimits::new(10_000_000, 10_000_000, 10_000_000),
        );
        let mut superseded_suite = suite.clone();
        superseded_suite.schema_version = OVEN_COMPILER_TEST_SUITE_SCHEMA_VERSION - 1;
        store.publish(&OvenArtifactPublishRequest {
            receipt: receipt.clone(),
            domain: "compiler-suite-lsp".to_string(),
            kind: OvenArtifactKind::CompilerTestSuite,
            payload: serde_json::to_vec(&superseded_suite)?,
            materialized_files: Vec::new(),
        })?;
        let current_manifest = store.publish(&OvenArtifactPublishRequest {
            receipt: receipt.clone(),
            domain: "compiler-suite-lsp".to_string(),
            kind: OvenArtifactKind::CompilerTestSuite,
            payload: serde_json::to_vec(&suite)?,
            materialized_files: Vec::new(),
        })?;
        let selected = super::select_compiler_test_suite(&store, &receipt, compiler_root.path(), &rustc)?;
        assert_eq!(selected.manifest.identity, current_manifest.identity);
        let tools = tempfile::tempdir()?;
        let cargo_marker = tools.path().join("cargo-started");
        let cargo = tools.path().join("cargo");
        write_executable(
            &cargo,
            &format!(
                "#!/bin/sh\nif [ \"$1\" = --version ]; then printf 'cargo fixture\\n'; exit 0; fi\nprintf started > \"{}\"\nexit 97\n",
                cargo_marker.display()
            ),
        )?;
        let evidence = loaf_envelope_evidence(
            OvenLoafEnvelope::CompilerSuite,
            compiler_root.path(),
            &std::env::current_exe()?,
            &sdk_inventory,
            &rustc,
        )?;
        let report = super::OvenLoafBakeReport {
            action: "reused".to_string(),
            envelope: "compiler-suite".to_string(),
            loaf_count: 6,
            prepared_count: 0,
            reused_count: 6,
            logical_bytes: 0,
            physical_bytes: 0,
            owned_physical_bytes: 0,
            raw_disk_bytes: 0,
            reclaimable_physical_bytes: 0,
            active_lease_physical_bytes: 0,
            transient_peak_physical_bytes: 0,
            max_physical_bytes: 10_000_000,
            max_domain_physical_bytes: 10_000_000,
            max_domain_logical_bytes: 10_000_000,
            elapsed_ms: 0,
            phase_timing: super::OvenLoafBakePhaseTiming::default(),
            cargo_process_started: false,
            evidence,
            loafs: Vec::new(),
            compiler_suite: None,
        };
        let publication_lock = acquire_exclusive_loaf_generation_lock(output.path())?;
        let report = super::finish_loaf_bake_after_publication(
            publication_lock,
            &OvenLoafBakeCommandOptions {
                compiler_root: compiler_root.path().to_path_buf(),
                output: output.path().to_path_buf(),
                suite_store: Some(suite_store.path().to_path_buf()),
                envelope: OvenLoafEnvelopeArgument::CompilerSuite,
                sdk_inventory,
                cargo,
                rustc,
                max_physical_bytes: Some(10_000_000),
                max_domain_physical_bytes: Some(10_000_000),
                max_domain_logical_bytes: Some(10_000_000),
                format: OvenOutputFormat::Json,
            },
            OvenLoafEnvelope::CompilerSuite,
            report,
            Instant::now(),
        )?;

        assert!(!cargo_marker.exists());
        assert_eq!(report.action, "reused");
        assert_eq!(
            report
                .compiler_suite
                .as_ref()
                .map(|suite| suite.prepare.cargo_version.as_str()),
            Some("not-run-existing-suite")
        );
        Ok(())
    }

    #[test]
    fn compiler_suite_receipt_reuses_member_compatible_envelope_across_generation_evidence()
    -> Result<(), Box<dyn std::error::Error>> {
        let compiler_root = tempfile::tempdir()?;
        fs::create_dir_all(compiler_root.path().join("src"))?;
        fs::write(
            compiler_root.path().join("Cargo.toml"),
            "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[features]\ndefault = []\nlsp = []\n",
        )?;
        fs::write(compiler_root.path().join("Cargo.lock"), "version = 4\n")?;
        fs::write(compiler_root.path().join("src/lib.rs"), "pub fn fixture() {}\n")?;
        fs::write(compiler_root.path().join("src/main.rs"), "fn main() {}\n")?;
        let rustc = resolve_active_rustc()?;
        let member = OvenLoafEnvelopeMember {
            label: "foundation-debug".to_string(),
            profile: "debug".to_string(),
            action: "run".to_string(),
            role: OvenLoafMemberRole::CompiledClosure,
            build_unit_identity: digest_bytes(b"foundation-build-unit"),
            loaf_identity: digest_bytes(b"foundation-loaf"),
            plan_identity: digest_bytes(b"foundation-plan"),
            logical_bytes: 1,
            physical_bytes: 1,
            path: PathBuf::from("generations/current/foundation.loaf/loaf.json"),
        };
        let write_envelope = |root: &Path,
                              generation: &str,
                              compiler_evidence: &str,
                              member: OvenLoafEnvelopeMember|
         -> Result<(), Box<dyn std::error::Error>> {
            fs::write(
                root.join("envelope.json"),
                serde_json::to_vec(&OvenLoafEnvelopeManifest {
                    schema_version: OVEN_LOAF_ENVELOPE_MANIFEST_SCHEMA_VERSION,
                    envelope: "compiler-suite".to_string(),
                    generation_identity: digest_bytes(generation.as_bytes()),
                    evidence: BTreeMap::from([(
                        "compiler_executable_digest".to_string(),
                        digest_bytes(compiler_evidence.as_bytes()),
                    )]),
                    loafs: vec![member],
                })?,
            )?;
            Ok(())
        };
        let first_loafs = tempfile::tempdir()?;
        let second_loafs = tempfile::tempdir()?;
        let third_loafs = tempfile::tempdir()?;
        write_envelope(first_loafs.path(), "generation-one", "compiler-one", member.clone())?;
        write_envelope(second_loafs.path(), "generation-two", "compiler-two", member.clone())?;
        let mut changed_member = member;
        changed_member.plan_identity = digest_bytes(b"changed-plan");
        write_envelope(third_loafs.path(), "generation-three", "compiler-three", changed_member)?;

        let (first, _) = super::compiler_libtests_receipt(
            compiler_root.path(),
            &rustc,
            &["lsp".to_string()],
            Some(first_loafs.path()),
        )?;
        let (second, _) = super::compiler_libtests_receipt(
            compiler_root.path(),
            &rustc,
            &["lsp".to_string()],
            Some(second_loafs.path()),
        )?;
        let (third, _) = super::compiler_libtests_receipt(
            compiler_root.path(),
            &rustc,
            &["lsp".to_string()],
            Some(third_loafs.path()),
        )?;
        assert_eq!(
            first.build_unit_identity, second.build_unit_identity,
            "changed envelope evidence must not rebuild an unchanged suite foundation"
        );
        assert_ne!(
            first.build_unit_identity, third.build_unit_identity,
            "a changed sealed member plan must rebuild the suite foundation"
        );
        Ok(())
    }

    #[test]
    fn interrupted_envelope_commit_preserves_the_previous_manifest() -> Result<(), Box<dyn std::error::Error>> {
        let output = tempfile::tempdir()?;
        let scratch = tempfile::tempdir()?;
        let generations = output.path().join("generations");
        let generation_output = generations.join("new-generation");
        let staged = scratch.path().join("staged");
        fs::create_dir_all(&staged)?;
        fs::create_dir_all(&generations)?;
        fs::write(staged.join("payload"), "new payload")?;
        let previous_manifest = b"previous authoritative manifest";
        fs::write(output.path().join("envelope.json"), previous_manifest)?;
        let manifest = OvenLoafEnvelopeManifest {
            schema_version: OVEN_LOAF_ENVELOPE_MANIFEST_SCHEMA_VERSION,
            envelope: "release".to_string(),
            generation_identity: "sha256:new-generation".to_string(),
            evidence: BTreeMap::new(),
            loafs: Vec::new(),
        };

        let result = commit_loaf_generation(
            output.path(),
            &generations,
            &generation_output,
            &staged,
            &manifest,
            scratch.path(),
            || Err(std::io::Error::other("simulated interruption")),
        );

        assert!(result.is_err());
        assert_eq!(fs::read(output.path().join("envelope.json"))?, previous_manifest);
        assert_eq!(fs::read(generation_output.join("payload"))?, b"new payload");
        Ok(())
    }

    #[test]
    fn concurrent_envelope_writers_leave_one_complete_authoritative_generation()
    -> Result<(), Box<dyn std::error::Error>> {
        let output = tempfile::tempdir()?;
        fs::create_dir_all(output.path().join("generations"))?;
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let mut writers = Vec::new();
        for index in 0..2 {
            let output = output.path().to_path_buf();
            let barrier = barrier.clone();
            writers.push(std::thread::spawn(move || -> Result<(), String> {
                let scratch = output.join(format!("writer-{index}"));
                let staged = scratch.join("staged");
                fs::create_dir_all(&staged).map_err(|error| error.to_string())?;
                fs::write(staged.join("payload"), format!("generation {index}")).map_err(|error| error.to_string())?;
                let generation_identity = format!("sha256:generation-{index}");
                let generation_output = output.join("generations").join(format!("generation-{index}"));
                let manifest = OvenLoafEnvelopeManifest {
                    schema_version: OVEN_LOAF_ENVELOPE_MANIFEST_SCHEMA_VERSION,
                    envelope: "release".to_string(),
                    generation_identity: generation_identity.clone(),
                    evidence: BTreeMap::new(),
                    loafs: Vec::new(),
                };
                barrier.wait();
                let _lock = acquire_exclusive_loaf_generation_lock(&output).map_err(|error| error.to_string())?;
                commit_loaf_generation(
                    &output,
                    &output.join("generations"),
                    &generation_output,
                    &staged,
                    &manifest,
                    &scratch,
                    || Ok(()),
                )
                .map_err(|error| error.to_string())?;
                retire_unreferenced_loaf_generations(&output, &generation_identity, &scratch)
                    .map_err(|error| error.to_string())
            }));
        }
        for writer in writers {
            match writer.join() {
                Ok(result) => result.map_err(|error| -> Box<dyn std::error::Error> { error.into() })?,
                Err(_) => return Err("concurrent Loaf writer panicked".into()),
            }
        }

        let manifest: OvenLoafEnvelopeManifest =
            serde_json::from_slice(&fs::read(output.path().join("envelope.json"))?)?;
        let generation = manifest
            .generation_identity
            .strip_prefix("sha256:")
            .ok_or("generation identity is not content-addressed")?;
        assert!(
            output
                .path()
                .join("generations")
                .join(generation)
                .join("payload")
                .is_file()
        );
        assert_eq!(fs::read_dir(output.path().join("generations"))?.count(), 1);
        Ok(())
    }

    #[test]
    fn native_test_failure_transcript_is_retained_beside_caller_output() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let output = directory.path().join("direct-rustc-test");
        let transcript = write_native_test_failure_transcript(&output, "one failing libtest\n")?;

        assert_eq!(transcript, output.with_extension("libtest-output.txt"));
        assert_eq!(fs::read_to_string(transcript)?, "one failing libtest\n");
        Ok(())
    }

    #[test]
    fn native_test_failure_summary_keeps_panic_diagnostics() {
        let summary = native_test_failure_summary(
            "test fixture ... FAILED\nthread 'fixture' panicked at src/fixture.rs:12:3:\nbenchmark fixture failed\nstdout: missing Loaf\nstderr: no Cargo fallback\ntest result: FAILED\n",
        );

        assert!(summary.contains("benchmark fixture failed"));
        assert!(summary.contains("stdout: missing Loaf"));
        assert!(summary.contains("stderr: no Cargo fallback"));
    }

    #[test]
    fn native_test_failure_summary_names_tests_libtest_only_identified_by_panic_thread() {
        let bulky_panic_body = "y".repeat(20_000);
        let transcript = format!(
            "thread 'noisy_first_failure' (2719) panicked at tests/integration_tests.rs:1324:9:\n{bulky_panic_body}\nthread 'quiet_last_failure' (4810) panicked at tests/integration_tests.rs:8361:9:\nassertion failed\n"
        );

        let summary = native_test_failure_summary(&transcript);

        assert!(
            summary.starts_with("failing tests (2):\n    noisy_first_failure\n    quiet_last_failure\n"),
            "a pass-through libtest run names its failures only on the panic line: {}",
            &summary[..summary.len().min(200)]
        );
    }

    #[test]
    fn native_test_failure_summary_names_every_failing_test_before_bounded_detail() {
        let bulky_panic_body = "x".repeat(20_000);
        let transcript = format!(
            "failures:\n\n---- noisy_first_failure stdout ----\nthread 'noisy_first_failure' panicked at src/fixture.rs:1:1:\n{bulky_panic_body}\n\n---- quiet_last_failure stdout ----\nthread 'quiet_last_failure' panicked at src/fixture.rs:2:2:\nassertion failed\n\ntest result: FAILED. 1 passed; 2 failed; 0 ignored\n"
        );

        let summary = native_test_failure_summary(&transcript);

        assert!(
            summary.starts_with("failing tests (2):\n    noisy_first_failure\n    quiet_last_failure\n"),
            "roster must lead the summary: {}",
            &summary[..summary.len().min(200)]
        );
        assert!(
            summary.contains("… libtest transcript truncated"),
            "the oversized body should still be bounded"
        );
    }

    #[test]
    fn compiler_suite_aggregate_is_persisted_beside_caller_outputs() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let report_path = directory.path().join("caller-output/compiler-suite-report.json");
        let report = serde_json::json!({
            "success": false,
            "native_test_case_totals": { "passed": 12, "failed": 1, "ignored": 2 },
        });

        write_compiler_suite_report(&report_path, &report)?;

        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&fs::read(&report_path)?)?,
            report
        );
        Ok(())
    }

    #[test]
    fn compiler_suite_timing_report_keeps_shared_and_root_measurements_distinct()
    -> Result<(), Box<dyn std::error::Error>> {
        let report = serde_json::json!({
            "timing": CompilerSuiteTimingReport {
                receipt_and_selection_elapsed_ms: 10,
                shared_setup_elapsed_ms: 20,
                target_preparation_elapsed_ms: 30,
                root_execution_elapsed_ms: 40,
                total_elapsed_ms: 100,
            },
            "native_test_roots": [CompilerSuiteNativeTestRootReport {
                package_name: "fixture".to_string(),
                target_kind: "test".to_string(),
                target_name: "fixture_root".to_string(),
                source_relative_path: "tests/fixture.rs".to_string(),
                inventory_count: 1,
                success: true,
                case_counts: Some(OvenNativeTestCaseCounts {
                    passed: 1,
                    failed: 0,
                    ignored: 0,
                }),
                case_timings: Vec::new(),
                command_timings: Vec::new(),
                direct_rustc_bake_elapsed_ms: 50,
                libtest_inventory_elapsed_ms: 6,
                libtest_execution_elapsed_ms: 7,
            }],
            "rustdoc_test_roots": [CompilerSuiteRustdocTestRootReport {
                package_name: "fixture".to_string(),
                target_kind: "lib".to_string(),
                target_name: "fixture_docs".to_string(),
                source_relative_path: "src/lib.rs".to_string(),
                execution_elapsed_ms: 8,
            }],
        });

        assert_eq!(report["timing"]["shared_setup_elapsed_ms"], 20);
        assert_eq!(report["native_test_roots"][0]["direct_rustc_bake_elapsed_ms"], 50);
        assert_eq!(report["native_test_roots"][0]["libtest_inventory_elapsed_ms"], 6);
        assert_eq!(report["native_test_roots"][0]["libtest_execution_elapsed_ms"], 7);
        assert_eq!(report["rustdoc_test_roots"][0]["execution_elapsed_ms"], 8);
        Ok(())
    }

    #[test]
    fn compiler_suite_case_totals_distinguish_green_failed_and_unreported_roots() {
        let report = CompilerSuiteChildrenReport {
            native_test_count: 4,
            doctest_targets: 0,
            failed: vec!["failed root".to_string()],
            native_test_roots: vec![
                CompilerSuiteNativeTestRootReport {
                    package_name: "green".to_string(),
                    target_kind: "test".to_string(),
                    target_name: "green_root".to_string(),
                    source_relative_path: "tests/green.rs".to_string(),
                    inventory_count: 2,
                    success: true,
                    case_counts: Some(OvenNativeTestCaseCounts {
                        passed: 2,
                        failed: 0,
                        ignored: 1,
                    }),
                    case_timings: vec![
                        OvenNativeTestCaseTiming {
                            name: "fast".to_string(),
                            elapsed_ms: 15,
                        },
                        OvenNativeTestCaseTiming {
                            name: "slow".to_string(),
                            elapsed_ms: 120,
                        },
                    ],
                    command_timings: Vec::new(),
                    direct_rustc_bake_elapsed_ms: 10,
                    libtest_inventory_elapsed_ms: 1,
                    libtest_execution_elapsed_ms: 2,
                },
                CompilerSuiteNativeTestRootReport {
                    package_name: "failed".to_string(),
                    target_kind: "test".to_string(),
                    target_name: "failed_root".to_string(),
                    source_relative_path: "tests/failed.rs".to_string(),
                    inventory_count: 2,
                    success: false,
                    case_counts: Some(OvenNativeTestCaseCounts {
                        passed: 1,
                        failed: 1,
                        ignored: 0,
                    }),
                    case_timings: vec![OvenNativeTestCaseTiming {
                        name: "also_slow".to_string(),
                        elapsed_ms: 120,
                    }],
                    command_timings: Vec::new(),
                    direct_rustc_bake_elapsed_ms: 11,
                    libtest_inventory_elapsed_ms: 1,
                    libtest_execution_elapsed_ms: 3,
                },
                CompilerSuiteNativeTestRootReport {
                    package_name: "unreported".to_string(),
                    target_kind: "test".to_string(),
                    target_name: "unreported_root".to_string(),
                    source_relative_path: "tests/unreported.rs".to_string(),
                    inventory_count: 0,
                    success: false,
                    case_counts: None,
                    case_timings: Vec::new(),
                    command_timings: Vec::new(),
                    direct_rustc_bake_elapsed_ms: 12,
                    libtest_inventory_elapsed_ms: 1,
                    libtest_execution_elapsed_ms: 4,
                },
            ],
            rustdoc_test_roots: Vec::new(),
        };

        let totals = report.native_test_case_totals();
        assert_eq!(totals.passed, 3);
        assert_eq!(totals.failed, 1);
        assert_eq!(totals.ignored, 1);
        assert_eq!(totals.reported_roots, 2);
        assert_eq!(totals.green_roots, 1);
        assert_eq!(totals.failed_roots, 1);
        assert_eq!(totals.unreported_roots, 1);
        assert_eq!(compiler_suite_completion_failures(&report, 3).len(), 1);
        let failures = compiler_suite_completion_failures(&report, 4);
        assert_eq!(failures.len(), 2);
        assert!(
            failures
                .iter()
                .any(|failure| failure.contains("terminal libtest summary"))
        );
        assert!(failures.iter().any(|failure| failure.contains("planned 4 root")));

        let slowest = report.slowest_native_test_cases(2);
        assert_eq!(slowest.len(), 2);
        assert_eq!(slowest[0].elapsed_ms, 120);
        assert_eq!(slowest[0].source_relative_path, "tests/failed.rs");
        assert_eq!(slowest[0].test_name, "also_slow");
        assert_eq!(slowest[1].elapsed_ms, 120);
        assert_eq!(slowest[1].source_relative_path, "tests/green.rs");
        assert_eq!(slowest[1].test_name, "slow");
    }

    #[test]
    fn default_store_root_prefers_incan_home() {
        assert_eq!(
            default_store_root(Some(OsString::from("/incan")), Some(OsString::from("/user"))),
            Some(PathBuf::from("/incan/oven/store/v2"))
        );
        assert_eq!(
            default_store_root(None, Some(OsString::from("/user"))),
            Some(PathBuf::from("/user/.incan/oven/store/v2"))
        );
    }

    #[test]
    fn default_rustup_home_prefers_an_explicit_toolchain_manager_root() {
        assert_eq!(
            default_rustup_home(Some(OsString::from("/rustup")), Some(OsString::from("/user"))),
            Some(PathBuf::from("/rustup"))
        );
        assert_eq!(
            default_rustup_home(None, Some(OsString::from("/user"))),
            Some(PathBuf::from("/user/.rustup"))
        );
    }

    #[test]
    fn source_input_requires_a_named_path() -> Result<(), Box<dyn std::error::Error>> {
        assert_eq!(
            parse_named_path("generated=out/test.rs")?,
            ("generated".to_string(), PathBuf::from("out/test.rs"))
        );
        assert!(parse_named_path("out/test.rs").is_err());
        Ok(())
    }

    #[test]
    fn compiler_suite_target_selection_is_receipt_bound_and_fails_closed() -> Result<(), Box<dyn std::error::Error>> {
        let target = OvenCompilerTestSuiteTarget {
            package_name: "fixture".to_string(),
            target_name: "first".to_string(),
            target_kind: "test".to_string(),
            runner: "rustc-test".to_string(),
            source_relative_path: "tests/first.rs".to_string(),
            source_evidence_key: "compiler-suite-source:tests/first.rs".to_string(),
            crate_name: "first".to_string(),
            edition: "2024".to_string(),
            features: Vec::new(),
            compile_environment: BTreeMap::new(),
            binary_dependencies: Vec::new(),
            workspace_library_dependencies: Vec::new(),
            externs: Vec::new(),
        };
        let mut second = target.clone();
        second.target_name = "second".to_string();
        second.source_relative_path = "tests/second.rs".to_string();
        second.source_evidence_key = "compiler-suite-source:tests/second.rs".to_string();
        second.crate_name = "second".to_string();
        let references = vec![
            OvenCompilerTestSuiteShardReference {
                identity: "sha256:first".to_string(),
                target: target.key(),
                source_bytes: 1,
            },
            OvenCompilerTestSuiteShardReference {
                identity: "sha256:second".to_string(),
                target: second.key(),
                source_bytes: 1,
            },
        ];

        assert_eq!(
            compiler_suite_selected_shard_references(&references, &["tests/second.rs".to_string()], None, None,)?
                .into_iter()
                .map(|reference| reference.identity)
                .collect::<Vec<_>>(),
            vec!["sha256:second"]
        );
        assert_eq!(
            compiler_suite_selected_shard_references(&references, &[], None, None)?,
            references
        );
        assert!(
            compiler_suite_selected_shard_references(&references, &["tests/missing.rs".to_string()], None, None,)
                .is_err()
        );
        assert!(compiler_suite_selected_shard_references(&references, &[" ".to_string()], None, None,).is_err());
        Ok(())
    }

    #[test]
    fn compiler_suite_partition_selection_is_weighted_deterministic_complete_and_receipt_bound()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut target = OvenCompilerTestSuiteTarget {
            package_name: "fixture".to_string(),
            target_name: "root".to_string(),
            target_kind: "test".to_string(),
            runner: "rustc-test".to_string(),
            source_relative_path: "tests/largest.rs".to_string(),
            source_evidence_key: "compiler-suite-source:tests/largest.rs".to_string(),
            crate_name: "largest".to_string(),
            edition: "2024".to_string(),
            features: Vec::new(),
            compile_environment: BTreeMap::new(),
            binary_dependencies: Vec::new(),
            workspace_library_dependencies: Vec::new(),
            externs: Vec::new(),
        };
        let mut references = [
            ("largest", "tests/largest.rs", 100_u64),
            ("medium_one", "tests/medium_one.rs", 30_u64),
            ("medium_two", "tests/medium_two.rs", 30_u64),
            ("medium_three", "tests/medium_three.rs", 30_u64),
            ("shared_one", "tests/shared.rs", 1_u64),
            ("shared_two", "tests/shared.rs", 1_u64),
            ("small", "tests/small.rs", 1_u64),
        ]
        .into_iter()
        .map(|(name, source_relative_path, source_bytes)| {
            target.target_name = name.to_string();
            target.source_relative_path = source_relative_path.to_string();
            target.source_evidence_key = format!("compiler-suite-source:{source_relative_path}");
            target.crate_name = name.to_string();
            OvenCompilerTestSuiteShardReference {
                identity: format!("sha256:{name}"),
                target: target.key(),
                source_bytes,
            }
        })
        .collect::<Vec<_>>();
        references.reverse();

        let selected = (0..4)
            .map(|index| compiler_suite_selected_shard_references(&references, &[], Some(index), Some(4)))
            .collect::<CliResult<Vec<_>>>()?;
        let mut reordered_references = references.clone();
        reordered_references.reverse();
        let selected_from_reordered = (0..4)
            .map(|index| compiler_suite_selected_shard_references(&reordered_references, &[], Some(index), Some(4)))
            .collect::<CliResult<Vec<_>>>()?;
        assert_eq!(selected, selected_from_reordered);
        let largest_partition = selected
            .iter()
            .find(|partition| partition.iter().any(|reference| reference.identity == "sha256:largest"))
            .ok_or_else(|| "partition selection omitted the largest receipt root".to_string())?;
        assert_eq!(largest_partition.len(), 1);
        assert_eq!(largest_partition[0].identity, "sha256:largest");
        let selected_identities = selected
            .iter()
            .flatten()
            .map(|reference| reference.identity.clone())
            .collect::<BTreeSet<_>>();
        let receipt_identities = references
            .iter()
            .map(|reference| reference.identity.clone())
            .collect::<BTreeSet<_>>();
        assert_eq!(selected_identities, receipt_identities);
        assert_eq!(selected.iter().map(Vec::len).sum::<usize>(), references.len());
        let shared_identities = selected
            .iter()
            .flatten()
            .filter(|reference| reference.target.source_relative_path == "tests/shared.rs")
            .map(|reference| reference.identity.clone())
            .collect::<BTreeSet<_>>();
        assert_eq!(
            shared_identities,
            BTreeSet::from(["sha256:shared_one".to_string(), "sha256:shared_two".to_string()])
        );
        assert!(compiler_suite_selected_shard_references(&references, &[], Some(0), Some(0)).is_err());
        assert!(compiler_suite_selected_shard_references(&references, &[], Some(4), Some(4)).is_err());
        assert!(compiler_suite_selected_shard_references(&references, &[], Some(0), None).is_err());
        assert!(
            compiler_suite_selected_shard_references(&references, &["tests/largest.rs".to_string()], Some(0), Some(4),)
                .is_err()
        );
        let mut missing_footprint = references.clone();
        missing_footprint[0].source_bytes = 0;
        assert!(compiler_suite_selected_shard_references(&missing_footprint, &[], Some(0), Some(4)).is_err());
        Ok(())
    }

    fn exact_selection_reference(runner: &str) -> OvenCompilerTestSuiteShardReference {
        OvenCompilerTestSuiteShardReference {
            identity: format!("sha256:{runner}"),
            target: OvenCompilerTestSuiteTargetKey {
                package_name: "incan".to_string(),
                target_name: "incan".to_string(),
                target_kind: "lib".to_string(),
                runner: runner.to_string(),
                source_relative_path: "src/lib.rs".to_string(),
            },
            source_bytes: 1,
        }
    }

    #[test]
    fn compiler_suite_exact_selection_requires_one_target_and_unique_nonempty_names()
    -> Result<(), Box<dyn std::error::Error>> {
        let rustc_test = exact_selection_reference("rustc-test");
        let requested = ["selected::second".to_string(), " selected::first ".to_string()];
        let (selected, names) = compiler_suite_exact_test_selection(&requested, vec![rustc_test.clone()], false)?;
        assert_eq!(selected, std::slice::from_ref(&rustc_test));
        assert_eq!(
            names,
            Some(vec!["selected::first".to_string(), "selected::second".to_string()])
        );
        assert_eq!(
            compiler_suite_exact_test_selection(&[], vec![rustc_test.clone()], false)?,
            (vec![rustc_test.clone()], None)
        );
        assert!(compiler_suite_exact_test_selection(&["selected::case".to_string()], Vec::new(), false).is_err());
        let mut second_rustc_test = rustc_test.clone();
        second_rustc_test.identity = "sha256:second-rustc-test".to_string();
        second_rustc_test.target.target_name = "other".to_string();
        assert!(
            compiler_suite_exact_test_selection(
                &["selected::case".to_string()],
                vec![rustc_test.clone(), second_rustc_test],
                false,
            )
            .is_err()
        );
        assert!(compiler_suite_exact_test_selection(&[" ".to_string()], vec![rustc_test.clone()], false).is_err());
        assert!(
            compiler_suite_exact_test_selection(
                &["selected::case".to_string(), " selected::case ".to_string()],
                vec![rustc_test.clone()],
                false,
            )
            .is_err()
        );
        assert!(
            compiler_suite_exact_test_selection(
                &["selected::case".to_string(), " ".to_string()],
                vec![rustc_test],
                false,
            )
            .is_err()
        );
        Ok(())
    }

    #[test]
    fn compiler_suite_exact_selection_chooses_the_native_cargo_target_companion()
    -> Result<(), Box<dyn std::error::Error>> {
        let rustc_test = exact_selection_reference("rustc-test");
        let rustdoc_test = exact_selection_reference("rustdoc-test");
        let references = vec![rustdoc_test.clone(), rustc_test.clone()];
        let path_selected =
            compiler_suite_selected_shard_references(&references, &["src/lib.rs".to_string()], None, None)?;
        assert_eq!(path_selected, references);
        assert_eq!(
            compiler_suite_exact_test_selection(&[], path_selected.clone(), false)?,
            (path_selected.clone(), None),
            "a complete-root source selection must retain both Cargo companions"
        );

        let (selected, names) =
            compiler_suite_exact_test_selection(&["selected::case".to_string()], path_selected, false)?;

        assert_eq!(selected, [rustc_test]);
        assert_eq!(names, Some(vec!["selected::case".to_string()]));
        Ok(())
    }

    #[test]
    fn compiler_suite_exact_selection_rejects_noncompanion_and_partition_selections() {
        let rustc_test = exact_selection_reference("rustc-test");
        let mut unrelated_rustdoc = exact_selection_reference("rustdoc-test");
        unrelated_rustdoc.target.target_name = "other".to_string();
        let unrelated = compiler_suite_exact_test_selection(
            &["selected::case".to_string()],
            vec![rustc_test.clone(), unrelated_rustdoc],
            false,
        );
        assert!(matches!(
            unrelated,
            Err(error) if error.to_string().contains("requires exactly one receipt-bound --target")
        ));

        let partition = compiler_suite_exact_test_selection(&["selected::case".to_string()], vec![rustc_test], true);
        assert!(matches!(
            partition,
            Err(error) if error.to_string().contains("cannot be combined with receipt partition selection")
        ));
    }

    #[test]
    fn compiler_suite_exact_selection_rejects_a_rustdoc_target_before_preparation() {
        let rustdoc = exact_selection_reference("rustdoc-test");
        let error = compiler_suite_exact_test_selection(&["documented_example".to_string()], vec![rustdoc], false);

        assert!(
            matches!(error, Err(error) if error.to_string().contains("requires a receipt-bound rustc-test target"))
        );
    }

    #[test]
    fn compiler_suite_selection_report_distinguishes_full_selected_root_and_exact_evidence()
    -> Result<(), Box<dyn std::error::Error>> {
        let names = vec!["selected::first".to_string(), "selected::second".to_string()];
        let diagnostic = compiler_suite_selection_report(
            Some(&names),
            &[" tests/integration_tests.rs ".to_string()],
            None,
            None,
            1,
        );
        let selected =
            compiler_suite_selection_report(None, &["tests/integration_tests.rs".to_string()], None, None, 1);
        let partition = compiler_suite_selection_report(None, &[], Some(2), Some(4), 9);
        let complete = compiler_suite_selection_report(None, &[], None, None, 37);

        assert_eq!(diagnostic.mode, "exact-diagnostic");
        assert_eq!(diagnostic.normalized_exact_names, names);
        assert_eq!(diagnostic.selected_case_count, 2);
        assert_eq!(diagnostic.requested_target_paths, ["tests/integration_tests.rs"]);
        assert_eq!(diagnostic.selected_root_count, 1);
        assert!(!diagnostic.complete_root_evidence);
        assert!(!diagnostic.complete_suite_evidence);
        assert_eq!(selected.mode, "selected-complete-roots");
        assert!(selected.complete_root_evidence);
        assert!(!selected.complete_suite_evidence);
        assert_eq!(
            compiler_suite_selection_context(&selected),
            "target path(s) tests/integration_tests.rs"
        );
        assert_eq!(partition.mode, "selected-complete-roots");
        assert!(partition.complete_root_evidence);
        assert!(!partition.complete_suite_evidence);
        assert_eq!(partition.selected_root_count, 9);
        assert_eq!(
            compiler_suite_selection_context(&partition),
            "zero-based partition 2 of 4"
        );
        assert_eq!(complete.mode, "complete-suite");
        assert!(complete.normalized_exact_names.is_empty());
        assert_eq!(complete.selected_case_count, 0);
        assert_eq!(complete.selected_root_count, 37);
        assert!(complete.complete_root_evidence);
        assert!(complete.complete_suite_evidence);
        let serialized = serde_json::to_value(&diagnostic)?;
        assert_eq!(serialized["mode"], "exact-diagnostic");
        assert_eq!(serialized["selected_case_count"], 2);
        assert_eq!(serialized["complete_root_evidence"], false);
        assert_eq!(serialized["complete_suite_evidence"], false);
        Ok(())
    }

    #[test]
    fn storage_policy_has_bounded_defaults() -> Result<(), Box<dyn std::error::Error>> {
        let limits = resolve_limits_with_environment_and_defaults(
            &OvenStoreCommandOptions {
                root: None,
                max_physical_bytes: None,
                max_domain_physical_bytes: None,
                max_domain_logical_bytes: None,
            },
            |_| None,
            OvenStoreLimits::new(
                DEFAULT_OVEN_MAX_PHYSICAL_BYTES,
                DEFAULT_OVEN_MAX_DOMAIN_PHYSICAL_BYTES,
                DEFAULT_OVEN_MAX_DOMAIN_LOGICAL_BYTES,
            ),
        )?;
        assert_eq!(limits.max_physical_bytes, DEFAULT_OVEN_MAX_PHYSICAL_BYTES);
        assert_eq!(limits.max_physical_bytes, 9 * 1024 * 1024 * 1024);
        assert_eq!(limits.max_domain_physical_bytes, DEFAULT_OVEN_MAX_DOMAIN_PHYSICAL_BYTES);
        assert_eq!(limits.max_domain_physical_bytes, 6 * 1024 * 1024 * 1024);
        assert_eq!(limits.max_domain_logical_bytes, DEFAULT_OVEN_MAX_DOMAIN_LOGICAL_BYTES);
        assert_eq!(limits.max_domain_logical_bytes, 6 * 1024 * 1024 * 1024);
        assert!(limits.max_domain_physical_bytes <= limits.max_physical_bytes);
        Ok(())
    }

    #[test]
    fn storage_policy_clamps_an_inherited_domain_limit_to_an_explicit_aggregate_limit()
    -> Result<(), Box<dyn std::error::Error>> {
        let limits = resolve_limits_with_environment_and_defaults(
            &OvenStoreCommandOptions {
                root: None,
                max_physical_bytes: Some(4 * 1024),
                max_domain_physical_bytes: None,
                max_domain_logical_bytes: None,
            },
            |_| None,
            OvenStoreLimits::new(8 * 1024, 6 * 1024, 3 * 1024),
        )?;

        assert_eq!(limits.max_physical_bytes, 4 * 1024);
        assert_eq!(limits.max_domain_physical_bytes, 4 * 1024);
        assert_eq!(limits.max_domain_logical_bytes, 3 * 1024);
        Ok(())
    }

    #[test]
    fn compiler_suite_schema_fifteen_composes_its_leased_foundations() {
        assert!(!compiler_suite_uses_indexed_foundations(9));
        for schema_version in 10..=OVEN_COMPILER_TEST_SUITE_SCHEMA_VERSION {
            assert!(compiler_suite_uses_indexed_foundations(schema_version));
        }
    }

    #[test]
    fn compiler_suite_storage_policy_has_measured_headroom() {
        let limits = loaf_envelope_default_limits(OvenLoafEnvelope::CompilerSuite);
        assert_eq!(
            limits.max_physical_bytes,
            DEFAULT_OVEN_COMPILER_SUITE_MAX_PHYSICAL_BYTES
        );
        assert_eq!(limits.max_physical_bytes, 16 * 1024 * 1024 * 1024);
        assert_eq!(
            limits.max_domain_physical_bytes,
            DEFAULT_OVEN_COMPILER_SUITE_MAX_DOMAIN_PHYSICAL_BYTES
        );
        assert_eq!(limits.max_domain_physical_bytes, 6 * 1024 * 1024 * 1024);
        assert_eq!(
            limits.max_domain_logical_bytes,
            DEFAULT_OVEN_COMPILER_SUITE_MAX_DOMAIN_LOGICAL_BYTES
        );
        assert_eq!(limits.max_domain_logical_bytes, 4 * 1024 * 1024 * 1024);
    }

    #[test]
    fn compiler_suite_files_are_receipt_digest_verified() -> Result<(), Box<dyn std::error::Error>> {
        let artifact_root = tempfile::tempdir()?;
        let inventory = artifact_root.path().join("providers/sdk-inventory.json");
        fs::create_dir_all(inventory.parent().ok_or("inventory parent missing")?)?;
        fs::write(&inventory, "sealed inventory")?;
        let digest = digest_bytes(&fs::read(&inventory)?);

        assert_eq!(
            compiler_suite_file(
                artifact_root.path(),
                "providers/sdk-inventory.json",
                &digest,
                "SDK provider inventory",
            )?,
            inventory
        );
        fs::write(&inventory, "mutated inventory")?;
        assert!(
            compiler_suite_file(
                artifact_root.path(),
                "providers/sdk-inventory.json",
                &digest,
                "SDK provider inventory",
            )
            .is_err()
        );
        Ok(())
    }

    #[test]
    fn schema_nine_index_selects_and_lease_protects_every_receipt_authorized_shard()
    -> Result<(), Box<dyn std::error::Error>> {
        let compiler_root = tempfile::tempdir()?;
        let store_root = tempfile::tempdir()?;
        fs::create_dir_all(compiler_root.path().join("src"))?;
        fs::write(
            compiler_root.path().join("Cargo.toml"),
            "[package]\nname = \"shard_fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )?;
        fs::write(compiler_root.path().join("Cargo.lock"), "version = 4\n")?;
        fs::write(compiler_root.path().join("src/lib.rs"), "pub fn fixture() {}\n")?;
        fs::write(compiler_root.path().join("src/main.rs"), "fn main() {}\n")?;
        let rustc = resolve_active_rustc()?;
        let receipt = receipt_native_compiler_suite(&OvenCompilerSuiteRequest::new(
            compiler_root.path(),
            rustc_host_target(&rustc)?,
            crate::oven::rustc::rustc_identity(&rustc)?,
            "debug",
            Vec::new(),
        ))?;
        let target = OvenCompilerTestSuiteTarget {
            package_name: "shard_fixture".to_string(),
            target_name: "shard_fixture".to_string(),
            target_kind: "lib".to_string(),
            runner: "rustc-test".to_string(),
            source_relative_path: "src/lib.rs".to_string(),
            source_evidence_key: "compiler-suite-source:src/lib.rs".to_string(),
            crate_name: "shard_fixture".to_string(),
            edition: "2024".to_string(),
            features: Vec::new(),
            compile_environment: BTreeMap::new(),
            binary_dependencies: Vec::new(),
            workspace_library_dependencies: Vec::new(),
            externs: Vec::new(),
        };
        let shard = OvenCompilerTestSuiteShardPayload {
            schema_version: OVEN_COMPILER_TEST_SUITE_SHARD_SCHEMA_VERSION_V1,
            target: target.clone(),
            binary_targets: Vec::new(),
            workspace_libraries: Vec::new(),
            foundation_references: Vec::new(),
            artifact_closure: OvenCompilerTestSuiteArtifactClosure {
                dependency_search_paths: Vec::new(),
                native_search_paths: Vec::new(),
                supporting_artifacts: Vec::new(),
            },
        };
        let store = OvenStore::new(store_root.path(), OvenStoreLimits::new(1_000_000, 1_000_000, 1_000_000));
        let manifest = store.publish(&OvenArtifactPublishRequest {
            receipt: receipt.clone(),
            domain: "compiler-suite-fixture".to_string(),
            kind: OvenArtifactKind::CompilerTestSuiteShard,
            payload: serde_json::to_vec(&shard)?,
            materialized_files: Vec::new(),
        })?;
        let selected = select_compiler_suite_shards(
            &store,
            &receipt,
            &[OvenCompilerTestSuiteShardReference {
                identity: manifest.identity.clone(),
                target: target.key(),
                source_bytes: 0,
            }],
            OVEN_COMPILER_TEST_SUITE_SHARD_SCHEMA_VERSION_V1,
        )?;
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].stored.manifest.identity, manifest.identity);
        assert_eq!(selected[0].payload, shard);
        let inspection = store.inspect()?;
        assert_eq!(inspection.active_lease_physical_bytes, inspection.physical_bytes);

        let constrained = OvenStore::new(store_root.path(), OvenStoreLimits::new(1, 1, 1));
        let (_index_entry, suite_lease) = constrained.select(&manifest.identity)?;
        let preview =
            run_compiler_suite_children_with_leases_retained(&suite_lease, &selected, &BTreeMap::new(), &[], || {
                constrained.preview_prune()
            })?;
        assert!(preview.removed_entries.is_empty());
        assert_eq!(preview.skipped_active_entries, vec![manifest.identity.clone()]);
        drop(selected);
        drop(suite_lease);
        assert_eq!(store.inspect()?.active_lease_physical_bytes, 0);
        Ok(())
    }

    #[test]
    fn warning_check_workspace_library_closure_omits_unrelated_shard_nodes() -> Result<(), Box<dyn std::error::Error>> {
        fn workspace_library(
            key: OvenCompilerWorkspaceLibraryKey,
            dependencies: Vec<OvenCompilerWorkspaceLibraryKey>,
        ) -> OvenCompilerWorkspaceLibrary {
            OvenCompilerWorkspaceLibrary {
                source_evidence_key: format!("compiler-suite-source:{}", key.source_relative_path),
                key,
                edition: "2024".to_string(),
                compile_environment: BTreeMap::new(),
                externs: Vec::new(),
                dependencies,
            }
        }

        let core = OvenCompilerWorkspaceLibraryKey {
            package_name: "incan_core".to_string(),
            crate_name: "incan_core".to_string(),
            target_kind: "lib".to_string(),
            source_relative_path: "crates/incan_core/src/lib.rs".to_string(),
            features: Vec::new(),
        };
        let derive = OvenCompilerWorkspaceLibraryKey {
            package_name: "incan_derive".to_string(),
            crate_name: "incan_derive".to_string(),
            target_kind: "proc-macro".to_string(),
            source_relative_path: "crates/incan_derive/src/lib.rs".to_string(),
            features: Vec::new(),
        };
        let stdlib = OvenCompilerWorkspaceLibraryKey {
            package_name: "incan_stdlib".to_string(),
            crate_name: "incan_stdlib".to_string(),
            target_kind: "lib".to_string(),
            source_relative_path: "crates/incan_stdlib/src/lib.rs".to_string(),
            features: Vec::new(),
        };
        let unrelated = OvenCompilerWorkspaceLibraryKey {
            package_name: "incan".to_string(),
            crate_name: "incan".to_string(),
            target_kind: "lib".to_string(),
            source_relative_path: "src/lib.rs".to_string(),
            features: Vec::new(),
        };
        let selected = compiler_suite_workspace_library_dependency_closure(
            &[
                workspace_library(unrelated, Vec::new()),
                workspace_library(stdlib.clone(), vec![core.clone(), derive.clone()]),
                workspace_library(core.clone(), Vec::new()),
                workspace_library(derive.clone(), Vec::new()),
            ],
            &stdlib,
        )?;

        assert_eq!(
            selected
                .iter()
                .map(|library| library.key.crate_name.as_str())
                .collect::<Vec<_>>(),
            vec!["incan_core", "incan_derive", "incan_stdlib"]
        );
        Ok(())
    }

    #[test]
    fn compiler_suite_workspace_library_dag_bakes_and_links_without_cargo() -> Result<(), Box<dyn std::error::Error>> {
        let compiler_root = tempfile::tempdir()?;
        let artifact_root = tempfile::tempdir()?;
        let output = tempfile::tempdir()?;
        fs::create_dir_all(compiler_root.path().join("src"))?;
        fs::write(
            compiler_root.path().join("Cargo.toml"),
            "[package]\nname = \"workspace_dag_fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )?;
        fs::write(compiler_root.path().join("Cargo.lock"), "version = 4\n")?;
        fs::write(compiler_root.path().join("src/lib.rs"), "pub fn fixture_root() {}\n")?;
        fs::write(
            compiler_root.path().join("src/foundation.rs"),
            "pub fn answer() -> u32 { 42 }\n",
        )?;
        fs::write(
            compiler_root.path().join("src/middle.rs"),
            "pub fn answer() -> u32 { fixture_foundation::answer() }\n",
        )?;
        fs::write(
            compiler_root.path().join("src/macros.rs"),
            "extern crate proc_macro;\nuse proc_macro::{Literal, TokenStream, TokenTree};\n#[proc_macro]\npub fn answer(_input: TokenStream) -> TokenStream { TokenStream::from(TokenTree::Literal(Literal::u32_unsuffixed(42))) }\n",
        )?;
        fs::write(
            compiler_root.path().join("src/main.rs"),
            "use fixture_macro::answer;\nfn main() { println!(\"{}\", answer!() + fixture_middle::answer()); }\n",
        )?;
        let rustc = resolve_active_rustc()?;
        let receipt = receipt_native_compiler_suite(&OvenCompilerSuiteRequest::new(
            compiler_root.path(),
            rustc_host_target(&rustc)?,
            crate::oven::rustc::rustc_identity(&rustc)?,
            "debug",
            Vec::new(),
        ))?;
        let foundation_key = OvenCompilerWorkspaceLibraryKey {
            package_name: "workspace_dag_fixture".to_string(),
            crate_name: "fixture_foundation".to_string(),
            target_kind: "lib".to_string(),
            source_relative_path: "src/foundation.rs".to_string(),
            features: Vec::new(),
        };
        let middle_key = OvenCompilerWorkspaceLibraryKey {
            package_name: "workspace_dag_fixture".to_string(),
            crate_name: "fixture_middle".to_string(),
            target_kind: "lib".to_string(),
            source_relative_path: "src/middle.rs".to_string(),
            features: Vec::new(),
        };
        let macro_key = OvenCompilerWorkspaceLibraryKey {
            package_name: "workspace_dag_fixture".to_string(),
            crate_name: "fixture_macro".to_string(),
            target_kind: "proc-macro".to_string(),
            source_relative_path: "src/macros.rs".to_string(),
            features: Vec::new(),
        };
        let closure = OvenCompilerTestSuiteArtifactClosure {
            dependency_search_paths: Vec::new(),
            native_search_paths: Vec::new(),
            supporting_artifacts: Vec::new(),
        };
        let mut workspace_library_cache = BTreeMap::new();
        let libraries = vec![
            OvenCompilerWorkspaceLibrary {
                key: middle_key.clone(),
                source_evidence_key: "compiler-suite-source:src/middle.rs".to_string(),
                edition: "2024".to_string(),
                compile_environment: BTreeMap::new(),
                externs: Vec::new(),
                dependencies: vec![foundation_key.clone()],
            },
            OvenCompilerWorkspaceLibrary {
                key: foundation_key.clone(),
                source_evidence_key: "compiler-suite-source:src/foundation.rs".to_string(),
                edition: "2024".to_string(),
                compile_environment: BTreeMap::new(),
                externs: Vec::new(),
                dependencies: Vec::new(),
            },
            OvenCompilerWorkspaceLibrary {
                key: macro_key.clone(),
                source_evidence_key: "compiler-suite-source:src/macros.rs".to_string(),
                edition: "2024".to_string(),
                compile_environment: BTreeMap::new(),
                externs: Vec::new(),
                dependencies: Vec::new(),
            },
        ];
        let outputs = bake_planned_compiler_suite_workspace_libraries(
            &libraries,
            &closure,
            &receipt.intent,
            &receipt,
            artifact_root.path(),
            &rustc,
            compiler_root.path(),
            output.path(),
            &[],
            None,
            &mut workspace_library_cache,
        )?;
        assert_eq!(outputs.len(), 3);
        assert!(outputs[&foundation_key].output.is_file());
        assert!(outputs[&middle_key].output.is_file());
        assert!(outputs[&macro_key].output.is_file());
        let duplicate_output = output.path().join("duplicate-workspace-libraries");
        let duplicate_outputs = bake_planned_compiler_suite_workspace_libraries(
            &libraries,
            &closure,
            &receipt.intent,
            &receipt,
            artifact_root.path(),
            &rustc,
            compiler_root.path(),
            &duplicate_output,
            &[],
            None,
            &mut workspace_library_cache,
        )?;
        assert_eq!(duplicate_outputs, outputs);
        assert!(!duplicate_output.exists());

        let target = OvenCompilerTestSuiteTarget {
            package_name: "workspace_dag_fixture".to_string(),
            target_name: "workspace_dag_runner".to_string(),
            target_kind: "bin".to_string(),
            runner: "rustc-run".to_string(),
            source_relative_path: "src/main.rs".to_string(),
            source_evidence_key: "compiler-suite-source:src/main.rs".to_string(),
            crate_name: "workspace_dag_runner".to_string(),
            edition: "2024".to_string(),
            features: Vec::new(),
            compile_environment: BTreeMap::new(),
            binary_dependencies: Vec::new(),
            workspace_library_dependencies: vec![middle_key, macro_key],
            externs: Vec::new(),
        };
        let artifacts = closure.manifest_for_target(&target, receipt.intent.clone());
        let mut artifact_plan = artifacts.materialize_trusted_store(artifact_root.path(), &receipt.intent)?;
        attach_compiler_suite_target_workspace_libraries(&mut artifact_plan, &target, &libraries, &outputs)?;
        let bake = bake_trusted_direct_rustc_run(&OvenTrustedDirectRustcTargetRequest {
            receipt: &receipt,
            artifacts: &artifacts,
            artifact_root: artifact_root.path(),
            artifact_plan: Some(&artifact_plan),
            rustc: &rustc,
            source: &compiler_root.path().join("src/main.rs"),
            output: &output.path().join("workspace-dag-runner"),
            crate_name: &target.crate_name,
            edition: &target.edition,
            source_evidence_key: &target.source_evidence_key,
            features: &target.features,
            prefer_dynamic: false,
        })?;
        assert!(!bake.cargo_process_started);
        let result = Command::new(&bake.output).output()?;
        assert!(result.status.success());
        assert_eq!(String::from_utf8(result.stdout)?.trim(), "84");
        Ok(())
    }

    #[test]
    fn compiler_suite_loaf_directory_is_confined_to_the_immutable_entry() -> Result<(), Box<dyn std::error::Error>> {
        let artifact_root = tempfile::tempdir()?;
        let loafs = artifact_root.path().join("toolchain-data/share/incan/oven/loafs");
        fs::create_dir_all(&loafs)?;

        assert_eq!(
            compiler_suite_directory(artifact_root.path(), "toolchain-data", "Loaf data")?,
            artifact_root.path().join("toolchain-data")
        );
        assert!(compiler_suite_directory(artifact_root.path(), "../outside", "Loaf data").is_err());
        Ok(())
    }

    #[test]
    fn compiler_suite_cli_output_stays_with_caller_owned_output() -> Result<(), Box<dyn std::error::Error>> {
        let output = tempfile::tempdir()?;
        let cli = compiler_suite_cli_output(output.path());

        assert_eq!(cli, output.path().join("compiler-cli/incan"));
        assert!(cli.starts_with(output.path()));
        Ok(())
    }

    /// Fixture metadata must not inherit an arbitrarily deep caller temporary directory.
    #[cfg(unix)]
    #[test]
    fn compiler_suite_temporary_directory_uses_the_short_system_root() -> Result<(), Box<dyn std::error::Error>> {
        let temporary = compiler_suite_temporary_directory()?;

        assert_eq!(temporary.path().parent(), Some(Path::new("/tmp")));
        assert!(temporary.path().is_dir());
        Ok(())
    }

    #[test]
    fn compiler_suite_worker_budget_is_hardware_aware_and_bounded() {
        assert_eq!(compiler_suite_auto_parallel_jobs(0), 1);
        assert_eq!(compiler_suite_auto_parallel_jobs(1), 1);
        assert_eq!(compiler_suite_auto_parallel_jobs(2), 1);
        assert_eq!(compiler_suite_auto_parallel_jobs(3), 1);
        assert_eq!(compiler_suite_auto_parallel_jobs(4), 2);
        assert_eq!(compiler_suite_auto_parallel_jobs(5), 2);
        assert_eq!(compiler_suite_auto_parallel_jobs(6), 3);
        assert_eq!(compiler_suite_auto_parallel_jobs(7), 3);
        assert_eq!(compiler_suite_auto_parallel_jobs(8), 4);
        assert_eq!(compiler_suite_auto_parallel_jobs(64), 4);
    }

    #[test]
    fn compiler_suite_libtest_budget_does_not_oversubscribe_root_workers() {
        assert_eq!(compiler_suite_libtest_threads(0, 0), 1);
        assert_eq!(compiler_suite_libtest_threads(1, 1), 1);
        assert_eq!(compiler_suite_libtest_threads(2, 1), 2);
        assert_eq!(compiler_suite_libtest_threads(3, 1), 2);
        assert_eq!(compiler_suite_libtest_threads(4, 2), 2);
        assert_eq!(compiler_suite_libtest_threads(6, 3), 2);
        assert_eq!(compiler_suite_libtest_threads(8, 4), 2);
        assert_eq!(compiler_suite_libtest_threads(64, 4), 2);
    }

    #[test]
    fn compiler_suite_limits_generated_rust_closure_to_its_consumers() {
        assert!(OvenCompilerSuiteTargetCapabilities::for_target("incan", "lib", "src/lib.rs").generated_rust_closure);
        assert!(
            OvenCompilerSuiteTargetCapabilities::for_target(
                "incan",
                "test",
                "tests/generated_rust_native_consumer_tests.rs"
            )
            .generated_rust_closure
        );
        assert!(
            OvenCompilerSuiteTargetCapabilities::for_target("incan", "test", "tests/integration_tests.rs")
                .generated_rust_closure
        );
        assert!(
            !OvenCompilerSuiteTargetCapabilities::for_target("incan", "test", "tests/toolchain_installer_tests.rs")
                .generated_rust_closure
        );
        assert!(
            OvenCompilerSuiteTargetCapabilities::for_target(
                "incan",
                "test",
                "tests/generated_rust_callability_artifact_tests.rs"
            )
            .cargo_fixture
        );
        assert!(
            OvenCompilerSuiteTargetCapabilities::for_target("incan", "test", "tests/cli_integration.rs")
                .explicit_bake_cargo
        );
        assert!(
            OvenCompilerSuiteTargetCapabilities::for_target("incan", "test", "tests/integration_tests.rs")
                .explicit_bake_cargo
        );
        assert!(
            !OvenCompilerSuiteTargetCapabilities::for_target(
                "incan",
                "test",
                "tests/generated_rust_native_consumer_tests.rs"
            )
            .explicit_bake_cargo
        );
        assert!(
            !OvenCompilerSuiteTargetCapabilities::for_target(
                "incan",
                "test",
                "tests/generated_rust_native_consumer_tests.rs"
            )
            .cargo_fixture
        );

        let mut environment = BTreeMap::from([
            ("INCAN_OVEN_COMPILER_SUITE_RUSTC".to_string(), "rustc".to_string()),
            (
                crate::oven::compiler_suite_env::OVEN_COMPILER_SUITE_CAPABILITY_ENV.to_string(),
                "warning-capability".to_string(),
            ),
            (
                crate::oven::compiler_suite_env::OVEN_COMPILER_SUITE_VOCAB_CAPABILITY_ENV.to_string(),
                "vocab-capability".to_string(),
            ),
            ("INCAN_INTERNAL_OVEN_LOAF_EXECUTION".to_string(), "1".to_string()),
        ]);

        compiler_suite_remove_generated_rust_closure(&mut environment);

        assert_eq!(
            environment.get("INCAN_OVEN_COMPILER_SUITE_RUSTC"),
            Some(&"rustc".to_string())
        );
        assert!(!environment.contains_key(crate::oven::compiler_suite_env::OVEN_COMPILER_SUITE_CAPABILITY_ENV));
        assert!(!environment.contains_key(crate::oven::compiler_suite_env::OVEN_COMPILER_SUITE_VOCAB_CAPABILITY_ENV));
        assert_eq!(
            environment.get("INCAN_INTERNAL_OVEN_LOAF_EXECUTION"),
            Some(&"1".to_string())
        );
    }

    #[test]
    fn compiler_suite_cargo_fixture_capability_is_package_qualified_and_explicit()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut target = OvenCompilerTestSuiteTarget {
            package_name: "incan".to_string(),
            target_name: "incan".to_string(),
            target_kind: "lib".to_string(),
            runner: "rustc-test".to_string(),
            source_relative_path: "src/lib.rs".to_string(),
            source_evidence_key: "compiler-suite-source:src/lib.rs".to_string(),
            crate_name: "incan".to_string(),
            edition: "2024".to_string(),
            features: Vec::new(),
            compile_environment: BTreeMap::new(),
            binary_dependencies: Vec::new(),
            workspace_library_dependencies: Vec::new(),
            externs: Vec::new(),
        };
        let mut environment = BTreeMap::new();
        assert!(apply_compiler_suite_target_capabilities(&target, &mut environment, None).is_err());
        assert!(!environment.contains_key("CARGO"));

        let fixture = CompilerSuiteFixtureCargoProxy {
            executable: PathBuf::from("/fixture/proxy/cargo"),
            real: PathBuf::from("/fixture/real/cargo"),
            real_rustc: PathBuf::from("/fixture/real/rustc"),
            home: PathBuf::from("/fixture/home"),
            log: PathBuf::from("/fixture/invocations.log"),
        };
        apply_compiler_suite_target_capabilities(&target, &mut environment, Some(&fixture))?;
        assert_eq!(environment.get("CARGO"), Some(&"/fixture/proxy/cargo".to_string()));
        assert_eq!(environment.get("HOME"), Some(&"/fixture/home".to_string()));
        assert_eq!(
            environment.get(crate::oven::compiler_suite_env::OVEN_COMPILER_SUITE_FIXTURE_RUSTC_REAL_ENV),
            Some(&"/fixture/real/rustc".to_string())
        );

        target.target_kind = "test".to_string();
        target.target_name = "cli_integration".to_string();
        target.source_relative_path = "tests/cli_integration.rs".to_string();
        let mut ordinary_environment = BTreeMap::new();
        assert!(apply_compiler_suite_target_capabilities(&target, &mut ordinary_environment, None).is_err());
        assert!(!ordinary_environment.contains_key("CARGO"));
        apply_compiler_suite_target_capabilities(&target, &mut ordinary_environment, Some(&fixture))?;
        assert_eq!(
            ordinary_environment.get(crate::oven::compiler_suite_env::OVEN_COMPILER_SUITE_EXPLICIT_BAKE_CARGO_ENV),
            Some(&"/fixture/real/cargo".to_string())
        );
        assert_eq!(
            ordinary_environment.get(crate::oven::compiler_suite_env::OVEN_COMPILER_SUITE_EXPLICIT_BAKE_HOME_ENV),
            Some(&"/fixture/home".to_string())
        );
        assert!(
            !ordinary_environment.contains_key("RUSTC"),
            "the explicit Cargo publisher must retain the suite-selected consumer Rustc"
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn compiler_suite_fixture_cargo_preserves_the_receipt_selected_rustc() -> Result<(), Box<dyn std::error::Error>> {
        let output = tempfile::tempdir()?;
        let tools = tempfile::tempdir()?;
        let cargo = tools.path().join("cargo");
        let rustc = tools.path().join("rustc");
        let marker = tools.path().join("observed-rustc");
        write_executable(
            &cargo,
            &format!("#!/bin/sh\nprintf '%s\\n' \"$RUSTC\" >> \"{}\"\n", marker.display()),
        )?;
        write_executable(&rustc, "#!/bin/sh\nexit 0\n")?;

        let fixture = super::prepare_compiler_suite_fixture_cargo_proxy(output.path(), &cargo)?;
        let mut explicit = Command::new(&fixture.executable);
        explicit
            .env(
                crate::oven::compiler_suite_env::OVEN_COMPILER_SUITE_FIXTURE_CARGO_REAL_ENV,
                &fixture.real,
            )
            .env(
                crate::oven::compiler_suite_env::OVEN_COMPILER_SUITE_FIXTURE_RUSTC_REAL_ENV,
                &fixture.real_rustc,
            )
            .env(
                crate::oven::compiler_suite_env::OVEN_COMPILER_SUITE_FIXTURE_CARGO_LOG_ENV,
                &fixture.log,
            )
            .env("RUSTC", "/receipt-selected/rustc");
        assert!(explicit.status()?.success());

        let mut fallback = Command::new(&fixture.executable);
        fallback
            .env(
                crate::oven::compiler_suite_env::OVEN_COMPILER_SUITE_FIXTURE_CARGO_REAL_ENV,
                &fixture.real,
            )
            .env(
                crate::oven::compiler_suite_env::OVEN_COMPILER_SUITE_FIXTURE_RUSTC_REAL_ENV,
                &fixture.real_rustc,
            )
            .env(
                crate::oven::compiler_suite_env::OVEN_COMPILER_SUITE_FIXTURE_CARGO_LOG_ENV,
                &fixture.log,
            )
            .env_remove("RUSTC");
        assert!(fallback.status()?.success());

        assert_eq!(
            fs::read_to_string(marker)?,
            format!("/receipt-selected/rustc\n{}\n", fixture.real_rustc.display())
        );
        Ok(())
    }

    #[test]
    fn compiler_suite_environment_paths_are_absolute_before_nested_tests_change_directory()
    -> Result<(), Box<dyn std::error::Error>> {
        let relative = Path::new("target/compiler-suite-relative-environment-value");

        assert_eq!(
            compiler_suite_environment_path(relative)?,
            std::env::current_dir()?.join(relative),
        );
        Ok(())
    }

    #[test]
    fn compiler_suite_environment_transports_the_complete_direct_rustc_closure()
    -> Result<(), Box<dyn std::error::Error>> {
        let compiler_root = tempfile::tempdir()?;
        let stdlib_root = compiler_root.path().join("crates/incan_stdlib/stdlib");
        fs::create_dir_all(&stdlib_root)?;
        let inventory = compiler_root.path().join("providers/sdk-inventory.json");
        fs::create_dir_all(inventory.parent().ok_or("SDK inventory parent missing")?)?;
        fs::write(&inventory, "sealed SDK inventory")?;
        fs::create_dir_all(
            inventory
                .parent()
                .ok_or("SDK inventory parent missing")?
                .join("runtime"),
        )?;
        fs::write(
            inventory
                .parent()
                .ok_or("SDK inventory parent missing")?
                .join("runtime/Cargo.lock"),
            "version = 4\n",
        )?;
        let target_dependencies = compiler_root.path().join("target/deps");
        let host_dependencies = compiler_root.path().join("host/deps");
        let stdlib = target_dependencies.join("libincan_stdlib.rlib");
        let stdlib_core = target_dependencies.join("libincan_stdlib_core.rlib");
        let derive = host_dependencies.join("libincan_derive.dylib");
        let toolchain_data_root = compiler_root.path().join("installed-toolchain");
        fs::create_dir_all(toolchain_data_root.join("share/incan/oven/loafs"))?;
        let output_directory = compiler_root.path().join("suite-output");
        let rustc = resolve_active_rustc()?;
        let (dynamic_library_environment_name, dynamic_library_environment_value) =
            rustc_dynamic_library_environment(&rustc)?;
        let environment = compiler_suite_environment(
            compiler_root.path(),
            &inventory,
            &rustc,
            &OvenRustcArtifactPlan {
                dependency_search_paths: vec![target_dependencies.clone(), host_dependencies.clone()],
                native_search_paths: Vec::new(),
                externs: vec![
                    ("incan_stdlib".to_string(), stdlib.clone()),
                    ("incan_stdlib_core".to_string(), stdlib_core.clone()),
                    ("incan_derive".to_string(), derive.clone()),
                ],
                compile_environment: BTreeMap::new(),
                caller_owned_library_digests: BTreeMap::new(),
            },
            Some(&toolchain_data_root),
            &output_directory,
        )?;

        assert_eq!(
            environment["INCAN_HOME"],
            output_directory.join("incan-home").display().to_string()
        );
        assert_eq!(environment["HOME"], output_directory.join("home").display().to_string());
        assert_eq!(environment["RUSTC"], rustc.display().to_string());
        assert_eq!(
            environment[&dynamic_library_environment_name],
            dynamic_library_environment_value
        );
        assert_eq!(
            environment["INSTA_WORKSPACE_ROOT"],
            compiler_root.path().canonicalize()?.display().to_string()
        );
        assert_eq!(
            environment["INCAN_INTERNAL_TOOLCHAIN_DATA_ROOT"],
            toolchain_data_root.display().to_string()
        );
        assert_eq!(environment["INCAN_INTERNAL_OVEN_LOAF_EXECUTION"], "1");
        assert_eq!(
            environment["INCAN_INTERNAL_OVEN_RUNTIME_ROOT"],
            inventory
                .parent()
                .ok_or("SDK inventory parent missing")?
                .join("runtime")
                .canonicalize()?
                .display()
                .to_string(),
        );
        let warning_capability = crate::oven::compiler_suite_env::OvenCompilerSuiteCapability::decode(
            &environment[crate::oven::compiler_suite_env::OVEN_COMPILER_SUITE_CAPABILITY_ENV],
        )?;
        assert_eq!(warning_capability.rustc, rustc);
        assert_eq!(
            warning_capability.dependency_search_paths,
            [target_dependencies.clone(), host_dependencies.clone()]
        );
        assert_eq!(warning_capability.externs["incan_stdlib"], stdlib);
        assert_eq!(warning_capability.externs["incan_stdlib_core"], stdlib_core);
        assert_eq!(warning_capability.externs["incan_derive"], derive);
        let vocab_capability = crate::oven::compiler_suite_env::OvenCompilerSuiteCapability::decode(
            &environment[crate::oven::compiler_suite_env::OVEN_COMPILER_SUITE_VOCAB_CAPABILITY_ENV],
        )?;
        assert_eq!(vocab_capability.rustc, rustc);
        assert_eq!(vocab_capability.externs.len(), 3);
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn stored_suite_child_consumes_sdk_inventory_without_a_cargo_launch() -> Result<(), Box<dyn std::error::Error>> {
        let compiler_root = tempfile::tempdir()?;
        let artifact_root = tempfile::tempdir()?;
        let output = tempfile::tempdir()?;
        let provider_root = artifact_root.path().join("providers");
        fs::create_dir_all(&provider_root)?;
        let inventory = provider_root.join("sdk-inventory.json");
        fs::write(&inventory, "sealed SDK inventory")?;
        fs::create_dir_all(provider_root.join("runtime"))?;
        fs::write(provider_root.join("runtime/Cargo.lock"), "version = 4\n")?;
        let stdlib_root = compiler_root.path().join("crates/incan_stdlib/stdlib");
        fs::create_dir_all(&stdlib_root)?;
        let toolchain_data_root = artifact_root.path().join("toolchain-data");
        fs::create_dir_all(toolchain_data_root.join("share/incan/oven/loafs"))?;
        let stdlib_extern = artifact_root.path().join("libincan_stdlib.rlib");
        fs::create_dir_all(compiler_root.path().join("src"))?;
        fs::write(
            compiler_root.path().join("Cargo.toml"),
            "[package]\nname = \"suite_fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )?;
        fs::write(compiler_root.path().join("Cargo.lock"), "version = 4\n")?;
        fs::write(compiler_root.path().join("src/main.rs"), "fn main() {}\n")?;
        fs::create_dir_all(compiler_root.path().join("src/bin"))?;
        fs::write(
            compiler_root.path().join("src/bin/suite_helper.rs"),
            "fn main() { println!(\"Oven helper\"); }\n",
        )?;
        fs::write(
            compiler_root.path().join("src/lib.rs"),
            r#"#[test]
fn planned_suite_child_uses_sdk_inventory() -> Result<(), String> {
    let inventory = std::env::var("INCAN_SDK_INVENTORY").map_err(|error| format!("SDK inventory: {error}"))?;
    let inventory_path = std::path::Path::new(&inventory);
    if !inventory_path.is_file() {
        return Err("SDK inventory is not a file".to_string());
    }
    let provider_root = std::env::var("INCAN_INTERNAL_SDK_PROVIDER_STORE")
        .map_err(|error| format!("SDK provider root: {error}"))?;
    let inventory_parent = inventory_path.parent().ok_or_else(|| "SDK inventory has no parent".to_string())?;
    if inventory_parent != std::path::Path::new(&provider_root) {
        return Err("SDK inventory must be rooted in the injected provider tree".to_string());
    }
    let suite_home = std::env::var("INCAN_HOME").map_err(|error| format!("suite home: {error}"))?;
    if std::path::Path::new(&suite_home).file_name().and_then(|name| name.to_str()) != Some("incan-home") {
        return Err("stored child must receive its caller-owned suite home".to_string());
    }
    let generated_target = std::env::var("INCAN_GENERATED_CARGO_TARGET_DIR")
        .map_err(|error| format!("generated target: {error}"))?;
    if std::path::Path::new(&generated_target).file_name().and_then(|name| name.to_str())
        != Some("generated-cargo-target")
    {
        return Err("stored child must receive its isolated generated target directory".to_string());
    }
    let native_root = std::env::var("INCAN_INTERNAL_TOOLCHAIN_DATA_ROOT")
        .map_err(|error| format!("Loaf root: {error}"))?;
    if !std::path::Path::new(&native_root).join("share/incan/oven/loafs").is_dir() {
        return Err("stored child must receive its Loaf root".to_string());
    }
    let current_directory = std::env::current_dir().map_err(|error| format!("current directory: {error}"))?.canonicalize()
        .map_err(|error| format!("canonical current directory: {error}"))?;
    let package_directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).canonicalize()
        .map_err(|error| format!("canonical package directory: {error}"))?;
    if current_directory != package_directory {
        return Err("stored direct-rustc test must run from its package root".to_string());
    }
    let dynamic_key = std::env::var("OVEN_TEST_DYNAMIC_LIBRARY_ENVIRONMENT_NAME")
        .map_err(|error| format!("dynamic key: {error}"))?;
    let dynamic_value = std::env::var("OVEN_TEST_DYNAMIC_LIBRARY_ENVIRONMENT_VALUE")
        .map_err(|error| format!("dynamic value: {error}"))?;
    if std::env::var(&dynamic_key).map_err(|error| format!("direct Rustc loader path: {error}"))? != dynamic_value {
        return Err("stored child received the wrong direct Rustc loader path".to_string());
    }
    let oven_cli = std::env::var("CARGO_BIN_EXE_incan").map_err(|error| format!("Oven CLI: {error}"))?;
    let oven_status = std::process::Command::new(oven_cli).arg(&inventory).status()
        .map_err(|error| format!("Oven CLI starts: {error}"))?;
    if !oven_status.success() {
        return Err("Oven CLI exited unsuccessfully".to_string());
    }
    let helper_status = std::process::Command::new(env!("CARGO_BIN_EXE_suite_helper")).status()
        .map_err(|error| format!("Oven helper starts: {error}"))?;
    if !helper_status.success() {
        return Err("Oven helper exited unsuccessfully".to_string());
    }
    Ok(())
}

#[test]
fn planned_suite_second_exact_case_keeps_cargo_guarded() -> Result<(), String> {
    if std::env::var_os("CARGO").is_some() || std::env::var_os("CARGO_MANIFEST_PATH").is_some() {
        return Err("exact native execution inherited Cargo process state".to_string());
    }
    let inventory = std::env::var("INCAN_SDK_INVENTORY").map_err(|error| format!("SDK inventory: {error}"))?;
    if !std::path::Path::new(&inventory).is_file() {
        return Err("second exact case did not retain the sealed SDK inventory".to_string());
    }
    Ok(())
}
"#,
        )?;
        let rustc = resolve_active_rustc()?;
        let (dynamic_library_environment_name, dynamic_library_environment_value) =
            rustc_dynamic_library_environment(&rustc)?;
        let cli = artifact_root.path().join("oven-incan");
        write_executable(
            &cli,
            "#!/bin/sh\n[ \"$1\" = \"$INCAN_SDK_INVENTORY\" ] && [ -f \"$INCAN_SDK_INVENTORY\" ] || exit 41\nexit 0\n",
        )?;
        let cargo_guard_directory = artifact_root.path().join("cargo-guard");
        fs::create_dir_all(&cargo_guard_directory)?;
        let cargo_marker = artifact_root.path().join("cargo-was-started");
        write_executable(
            &cargo_guard_directory.join("cargo"),
            &format!("#!/bin/sh\nprintf cargo > \"{}\"\nexit 97\n", cargo_marker.display()),
        )?;

        let receipt = receipt_native_compiler_suite(&OvenCompilerSuiteRequest::new(
            compiler_root.path(),
            rustc_host_target(&rustc)?,
            rustc_identity(&rustc)?,
            "debug",
            Vec::new(),
        ))?;
        let mut environment = compiler_suite_environment(
            compiler_root.path(),
            &inventory,
            &rustc,
            &OvenRustcArtifactPlan {
                dependency_search_paths: Vec::new(),
                native_search_paths: Vec::new(),
                externs: vec![("incan_stdlib".to_string(), stdlib_extern)],
                compile_environment: BTreeMap::new(),
                caller_owned_library_digests: BTreeMap::new(),
            },
            Some(&toolchain_data_root),
            output.path(),
        )?;
        environment.insert("CARGO_BIN_EXE_incan".to_string(), cli.display().to_string());
        environment.insert(
            "OVEN_TEST_DYNAMIC_LIBRARY_ENVIRONMENT_NAME".to_string(),
            dynamic_library_environment_name,
        );
        environment.insert(
            "OVEN_TEST_DYNAMIC_LIBRARY_ENVIRONMENT_VALUE".to_string(),
            dynamic_library_environment_value,
        );
        environment.insert(
            "PATH".to_string(),
            format!("{}:/usr/bin:/bin", cargo_guard_directory.display()),
        );
        let suite_helper = OvenCompilerTestSuiteTarget {
            package_name: "suite_fixture".to_string(),
            target_name: "suite_helper".to_string(),
            target_kind: "bin".to_string(),
            runner: "rustc-run".to_string(),
            source_relative_path: "src/bin/suite_helper.rs".to_string(),
            source_evidence_key: "compiler-suite-source:src/bin/suite_helper.rs".to_string(),
            crate_name: "suite_helper".to_string(),
            edition: "2024".to_string(),
            features: Vec::new(),
            compile_environment: BTreeMap::from([
                ("CARGO_MANIFEST_DIR".to_string(), "@oven-source-ancestor:3".to_string()),
                ("CARGO_PKG_NAME".to_string(), "suite_fixture".to_string()),
                ("CARGO_PKG_VERSION".to_string(), "0.1.0".to_string()),
            ]),
            binary_dependencies: Vec::new(),
            workspace_library_dependencies: Vec::new(),
            externs: Vec::new(),
        };
        let mut binary_cache = BTreeMap::new();
        let binary_outputs = bake_planned_compiler_suite_binaries(
            &[suite_helper],
            &OvenCompilerTestSuiteArtifactClosure {
                dependency_search_paths: Vec::new(),
                native_search_paths: Vec::new(),
                supporting_artifacts: Vec::new(),
            },
            &receipt.intent,
            &receipt,
            artifact_root.path(),
            &rustc,
            compiler_root.path(),
            output.path(),
            &cli,
            &[],
            &BTreeMap::new(),
            &[],
            None,
            &mut binary_cache,
        )?;
        let suite_child = OvenCompilerTestSuiteTarget {
            package_name: "suite_fixture".to_string(),
            target_name: "cli_integration".to_string(),
            target_kind: "test".to_string(),
            runner: "rustc-test".to_string(),
            source_relative_path: "src/lib.rs".to_string(),
            source_evidence_key: "compiler-suite-source:src/lib.rs".to_string(),
            crate_name: "suite_fixture".to_string(),
            edition: "2024".to_string(),
            features: Vec::new(),
            compile_environment: BTreeMap::from([
                ("CARGO_MANIFEST_DIR".to_string(), "@oven-source-ancestor:2".to_string()),
                ("CARGO_PKG_NAME".to_string(), "suite_fixture".to_string()),
                ("CARGO_PKG_VERSION".to_string(), "0.1.0".to_string()),
            ]),
            binary_dependencies: vec!["suite_helper".to_string()],
            workspace_library_dependencies: Vec::new(),
            externs: Vec::new(),
        };
        let closure = OvenCompilerTestSuiteArtifactClosure {
            dependency_search_paths: Vec::new(),
            native_search_paths: Vec::new(),
            supporting_artifacts: Vec::new(),
        };
        let prepared_child = prepare_compiler_suite_child(
            &suite_child,
            &closure,
            &receipt.intent,
            artifact_root.path(),
            &rustc,
            compiler_root.path(),
            output.path(),
            &environment,
            &binary_outputs,
            &[],
            BTreeMap::new(),
            &[],
            None,
            None,
        )?;
        assert!(
            std::ptr::eq(prepared_child.closure, &closure),
            "the scheduler must retain the selected immutable closure by reference until a worker owns the target"
        );
        assert!(
            std::ptr::eq(prepared_child.target, &suite_child),
            "the scheduler must retain the selected target by reference until a worker owns the target"
        );
        assert_eq!(
            prepared_child
                .binary_compile_environment
                .get("CARGO_BIN_EXE_suite_helper"),
            binary_outputs
                .get("suite_helper")
                .map(|path| path.display().to_string())
                .as_ref(),
            "the worker must retain the exact direct helper-binary compile environment"
        );
        let generated_target = Path::new(
            prepared_child
                .environment
                .get("INCAN_GENERATED_CARGO_TARGET_DIR")
                .ok_or("prepared child has no isolated generated target directory")?,
        );
        let child_state_root = compiler_suite_child_state_root(output.path(), &suite_child);
        assert!(
            generated_target.starts_with(&child_state_root),
            "the generated target must remain within its caller-owned child state: {}",
            generated_target.display()
        );
        assert_eq!(
            generated_target.file_name().and_then(|name| name.to_str()),
            Some("generated-cargo-target"),
            "the isolated generated target should be distinguishable from the shared harness target"
        );
        let child_incan_home = child_state_root.join("incan-home").display().to_string();
        let child_home = child_state_root.join("home").display().to_string();
        assert_eq!(
            prepared_child.environment.get("INCAN_HOME"),
            Some(&child_incan_home),
            "each stored child must receive its own mutable Oven home"
        );
        assert_eq!(
            prepared_child.environment.get("HOME"),
            Some(&child_home),
            "default state must remain inside the same child-owned boundary"
        );
        let mut sibling = suite_child.clone();
        sibling.target_name = "cli_integration_sibling".to_string();
        assert_ne!(
            compiler_suite_child_state_root(output.path(), &suite_child),
            compiler_suite_child_state_root(output.path(), &sibling),
            "distinct parallel roots must not share mutable state"
        );
        let exact_test_names = vec![
            "planned_suite_child_uses_sdk_inventory".to_string(),
            "planned_suite_second_exact_case_keeps_cargo_guarded".to_string(),
        ];
        let report =
            run_prepared_compiler_suite_children(vec![prepared_child], &receipt, &rustc, Some(&exact_test_names))?;

        assert_eq!(report.native_test_count, 2);
        assert_eq!(report.doctest_targets, 0);
        assert!(
            report.failed.is_empty(),
            "stored-suite child failures: {:?}",
            report.failed
        );
        assert_eq!(report.native_test_roots.len(), 1);
        let counts = report.native_test_roots[0]
            .case_counts
            .as_ref()
            .ok_or("multi-exact run did not report aggregate libtest counts")?;
        assert_eq!(counts.passed, 2);
        assert_eq!(counts.failed, 0);
        assert_eq!(counts.ignored, 0);
        assert_eq!(report.native_test_roots[0].inventory_count, 2);
        assert!(
            !cargo_marker.exists(),
            "the Cargo guard was executed, so a direct-rustc planned suite child attempted a Cargo launch"
        );
        Ok(())
    }

    #[cfg(unix)]
    fn write_executable(path: &Path, contents: &str) -> Result<(), std::io::Error> {
        fs::write(path, contents)?;
        fs::set_permissions(path, fs::Permissions::from_mode(0o755))
    }

    #[test]
    fn command_surface_runs_a_stored_native_test_without_a_cargo_consumer() -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let artifacts = tempfile::tempdir()?;
        let output = tempfile::tempdir()?;
        let store_root = tempfile::tempdir()?;
        write_project(project.path())?;
        let source = output.path().join("command-surface.rs");
        fs::write(
            &source,
            "#[test]\nfn alpha_consumer_has_no_cargo_environment() { assert!(std::env::var_os(\"CARGO\").is_none()); assert!(std::env::var_os(\"CARGO_PKG_NAME\").is_none()); }\nfn main() { assert!(std::env::var_os(\"CARGO\").is_none()); assert!(std::env::var_os(\"CARGO_PKG_NAME\").is_none()); assert_eq!(std::env::args().nth(1).as_deref(), Some(\"--oven-proof\")); }\n",
        )?;
        let receipt = output.path().join("receipt.json");
        let rustc = rustc_path()?;
        oven_import(OvenImportCommandOptions {
            project: project.path().to_path_buf(),
            target: rustc_host_target(&rustc)?,
            toolchain: rustc_identity(&rustc)?,
            profile: "release".to_string(),
            features: Vec::new(),
            source_inputs: vec![format!("direct-rustc-source={}", source.display())],
            output: Some(receipt.clone()),
            format: OvenOutputFormat::Json,
        })?;
        let receipt_data = fs::read(&receipt)?;
        let receipt_model: crate::oven::OvenReceipt = serde_json::from_slice(&receipt_data)?;
        let plan_path = output.path().join("plan.json");
        fs::write(
            &plan_path,
            serde_json::to_vec(&OvenRustcArtifactManifest {
                schema_version: OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION,
                intent: receipt_model.intent,
                dependency_search_paths: Vec::new(),
                native_search_paths: Vec::new(),
                externs: Vec::new(),
                entrypoint_externs: std::collections::BTreeMap::new(),
                registry_leaves: Vec::new(),
                registry_sources: Vec::new(),
                compile_environment: std::collections::BTreeMap::new(),
                vocab_auxiliary_targets: Vec::new(),
                supporting_artifacts: vec![crate::oven::rustc::OvenRustcSupportingArtifact {
                    relative_path: "support/libpublisher-proof.rlib".to_string(),
                    digest: digest_bytes(b"publisher proof"),
                }],
            })?,
        )?;
        let publisher_support_directory = artifacts.path().join("support");
        fs::create_dir_all(&publisher_support_directory)?;
        let publisher_support = publisher_support_directory.join("libpublisher-proof.rlib");
        fs::write(&publisher_support, b"publisher proof")?;
        let store = OvenStoreCommandOptions {
            root: Some(store_root.path().to_path_buf()),
            max_physical_bytes: Some(128 * 1024),
            max_domain_physical_bytes: Some(128 * 1024),
            max_domain_logical_bytes: Some(64 * 1024),
        };
        oven_publish_direct_rustc_plan(OvenPlanPublishCommandOptions {
            receipt: receipt.clone(),
            manifest: plan_path,
            artifact_root: artifacts.path().to_path_buf(),
            domain: "command-surface".to_string(),
            store: store.clone(),
            format: OvenOutputFormat::Json,
        })?;
        let limits = resolve_limits_with_environment_and_defaults(
            &store,
            |_| None,
            OvenStoreLimits::new(
                DEFAULT_OVEN_MAX_PHYSICAL_BYTES,
                DEFAULT_OVEN_MAX_DOMAIN_PHYSICAL_BYTES,
                DEFAULT_OVEN_MAX_DOMAIN_LOGICAL_BYTES,
            ),
        )?;
        let inspection = OvenStore::new(store_root.path(), limits).inspect()?;
        assert_eq!(inspection.entries.len(), 1);
        assert!(inspection.physical_bytes >= inspection.logical_bytes);
        assert_eq!(
            fs::read(
                inspection.entries[0]
                    .materialized_root()
                    .join("support/libpublisher-proof.rlib")
            )?,
            b"publisher proof"
        );
        fs::remove_dir_all(artifacts.path())?;
        oven_test(OvenTestCommandOptions {
            receipt: receipt.clone(),
            plan_identity: inspection.entries[0].manifest.identity.clone(),
            rustc: rustc.clone(),
            source: source.clone(),
            output: output.path().join("command-surface-test"),
            crate_name: "oven_command_surface".to_string(),
            edition: "2024".to_string(),
            source_evidence_key: "direct-rustc-source".to_string(),
            exact_names: vec!["alpha_consumer_has_no_cargo_environment".to_string()],
            store: store.clone(),
            format: OvenOutputFormat::Json,
        })?;
        oven_run(OvenRunCommandOptions {
            receipt,
            plan_identity: inspection.entries[0].manifest.identity.clone(),
            rustc,
            source,
            output: output.path().join("command-surface-run"),
            crate_name: "oven_command_surface_run".to_string(),
            edition: "2024".to_string(),
            source_evidence_key: "direct-rustc-source".to_string(),
            arguments: vec![OsString::from("--oven-proof")],
            store,
            format: OvenOutputFormat::Json,
        })?;
        Ok(())
    }

    fn write_project(path: &std::path::Path) -> Result<(), std::io::Error> {
        fs::write(
            path.join("Cargo.toml"),
            "[package]\nname = \"oven-command-surface\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )?;
        fs::write(
            path.join("Cargo.lock"),
            "# This file is automatically @generated by Cargo.\nversion = 4\n",
        )
    }

    fn rustc_path() -> Result<PathBuf, Box<dyn std::error::Error>> {
        let output = Command::new("rustup").args(["which", "rustc"]).output()?;
        if !output.status.success() {
            return Err("rustup could not locate rustc".into());
        }
        let path = PathBuf::from(String::from_utf8(output.stdout)?.trim());
        if !path.is_file() {
            return Err(format!("rustup returned a non-file rustc path: {}", path.display()).into());
        }
        Ok(path)
    }

    fn rustc_identity(rustc: &Path) -> Result<String, Box<dyn std::error::Error>> {
        let output = Command::new(rustc).arg("--version").output()?;
        if !output.status.success() {
            return Err(format!("rustc could not report its version: {}", rustc.display()).into());
        }
        Ok(String::from_utf8(output.stdout)?.trim().to_string())
    }
}
