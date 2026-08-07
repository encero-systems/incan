//! Explicit Oven Alpha command surface for receipts, bounded plans, and native direct-rustc consumers.
//!
//! Consumer commands never wrap, probe, or launch Cargo. Frozen Cargo declarations are compatibility input to
//! `oven import`; only the hidden, explicitly named `oven legacy-cargo` baker may materialize missing compatibility
//! inputs with Cargo. Every Alpha consumer uses sealed Loafs, the receipt-bound Oven store, and the direct-rustc
//! executor below.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::env;
use std::ffi::OsString;
use std::fs::{self, File};
use std::io;
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::cli::{CliError, CliResult, ExitCode, OvenLoafEnvelopeArgument, OvenOutputFormat};
use crate::oven::legacy_cargo::{
    OVEN_COMPILER_TEST_SUITE_FOUNDATION_SCHEMA_VERSION, OVEN_COMPILER_TEST_SUITE_SHARD_SCHEMA_VERSION,
    OVEN_COMPILER_TEST_SUITE_SHARD_SCHEMA_VERSION_V1, OVEN_COMPILER_TEST_SUITE_TOOLCHAIN_DATA_SCHEMA_VERSION,
    OvenCompilerTestSuiteFoundationPayload, OvenCompilerTestSuiteFoundationReference, OvenCompilerTestSuitePayload,
    OvenCompilerTestSuiteShardPayload, OvenCompilerTestSuiteShardReference, OvenCompilerTestSuiteToolchainDataPayload,
    OvenCompilerTestSuiteToolchainDataReference, OvenCompilerWorkspaceLibrary, OvenCompilerWorkspaceLibraryKey,
    OvenLegacyCargoCompilerSuiteResult, OvenLegacyCargoPrepareRequest, OvenLegacyCargoPublicationKind,
    prepare_compiler_test_suite, prepare_direct_rustc_plan,
};
use crate::oven::loaf::{
    LoafTemporaryDirectory, OVEN_LOAF_ENV, OVEN_LOAF_ENVELOPE_MANIFEST_SCHEMA_VERSION, OvenLoafBakerContext,
    OvenLoafEnvelope, OvenLoafEnvelopeManifest, OvenLoafEnvelopeMember, OvenLoafFixtureAction, OvenLoafPreparation,
    acquire_exclusive_loaf_generation_lock, loaf_directory_byte_counts, loaf_envelope_specifications,
    loaf_raw_disk_bytes, prepare_loaf_from_generated_project, validate_stored_loaf,
};
use crate::oven::native_test::{
    OvenNativeTestCaseCounts, OvenNativeTestRequest, run_native_test_batch_all_in_directory_with_timeout,
    run_native_tests,
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
/// Constrained hosted MSRV runners can require more than fifteen minutes for the two largest integration roots even
/// though prepared reference-machine replay remains inside the five-minute suite budget. Keep a deterministic
/// per-root ceiling, but calibrate it with enough headroom that slow hardware is not misreported as a test failure.
const OVEN_COMPILER_TEST_ROOT_TIMEOUT: Duration = Duration::from_secs(30 * 60);

/// Inputs for `incan oven import`.
#[derive(Debug, Clone)]
pub struct OvenImportCommandOptions {
    /// Root containing the frozen Cargo package to import as evidence.
    pub project: PathBuf,
    /// Explicit target triple for the recorded build intent.
    pub target: String,
    /// Exact selected Rust toolchain identity.
    pub toolchain: String,
    /// Explicit profile name for the recorded build intent.
    pub profile: String,
    /// Explicitly selected feature names.
    pub features: Vec<String>,
    /// Named generated source inputs expressed as `NAME=PATH`.
    pub source_inputs: Vec<String>,
    /// Optional receipt output; the project-local Oven receipt path is used otherwise.
    pub output: Option<PathBuf>,
    /// Requested rendering format.
    pub format: OvenOutputFormat,
}

/// Shared bounded-store location and policy inputs for Oven Alpha commands.
#[derive(Debug, Clone)]
pub struct OvenStoreCommandOptions {
    /// Optional explicit store root; the versioned `INCAN_HOME`/home default is used otherwise.
    pub root: Option<PathBuf>,
    /// Optional aggregate physical allocation cap in bytes.
    pub max_physical_bytes: Option<u64>,
    /// Optional per-domain physical allocation cap in bytes.
    pub max_domain_physical_bytes: Option<u64>,
    /// Optional per-domain logical artifact-byte cap in bytes.
    pub max_domain_logical_bytes: Option<u64>,
}

/// Inputs for `incan inspect oven` receipt and build-unit inspection.
#[derive(Debug, Clone)]
pub struct OvenReceiptInspectCommandOptions {
    /// Persisted receipt that authorizes the requested Oven build unit.
    pub receipt: PathBuf,
    /// Bounded store selection and policy.
    pub store: OvenStoreCommandOptions,
    /// Requested rendering format.
    pub format: OvenOutputFormat,
}

/// Receipt/build-unit selection state shown by `incan inspect oven`.
#[derive(Debug, Clone, Serialize)]
pub struct OvenPlanSelectionInspection {
    /// `hit`, `miss`, or `ambiguous`; normal consumers refuse the latter two.
    pub state: String,
    /// Matching immutable direct-rustc plan identities retained in the store.
    pub plan_identities: Vec<String>,
    /// Explicit explanation for a miss or ambiguity, absent for a unique hit.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Command-level Oven receipt, compatibility, and bounded-storage evidence.
#[derive(Debug, Clone, Serialize)]
pub struct OvenReceiptInspection {
    /// Verified complete source receipt identity.
    pub receipt_identity: String,
    /// Portable compatibility identity used to select a reusable native closure.
    pub build_unit_identity: String,
    /// Target/toolchain/profile/features selected by this receipt.
    pub intent: OvenBuildIntent,
    /// Named compiler, runtime, dependency, and provider inputs that compose the build-unit identity.
    /// These values are portable identity evidence, never project-local source paths.
    pub build_unit_inputs: std::collections::BTreeMap<String, String>,
    /// Store-plan selection outcome for the receipt.
    pub selection: OvenPlanSelectionInspection,
    /// Store-wide logical artifact bytes.
    pub logical_artifact_bytes: u64,
    /// Store-wide measured physical allocation bytes.
    pub physical_bytes: u64,
    /// Inactive physical bytes available for policy-driven reclamation.
    pub reclaimable_physical_bytes: u64,
    /// Physical bytes protected by active consumer leases.
    pub active_lease_physical_bytes: u64,
}

/// Inputs for `incan oven plan publish`.
#[derive(Debug, Clone)]
pub struct OvenPlanPublishCommandOptions {
    /// Persisted receipt that authorizes the plan.
    pub receipt: PathBuf,
    /// JSON direct-rustc artifact manifest to validate and retain immutably.
    pub manifest: PathBuf,
    /// Immutable artifact root used for full manifest validation before publication.
    pub artifact_root: PathBuf,
    /// Compatibility domain which owns this retained plan.
    pub domain: String,
    /// Bounded store selection and policy.
    pub store: OvenStoreCommandOptions,
    /// Requested rendering format.
    pub format: OvenOutputFormat,
}

/// Inputs for the explicit temporary `legacy_cargo` publisher.
#[derive(Debug, Clone)]
pub struct OvenLegacyCargoPrepareCommandOptions {
    /// Generated-project receipt that authorizes the direct-rustc build unit.
    pub receipt: PathBuf,
    /// Caller-owned generated Rust project containing `Cargo.toml` and `src/main.rs`.
    pub generated_project: PathBuf,
    /// Explicit Cargo executable used only for this named publisher transition.
    pub cargo: PathBuf,
    /// Explicit Rust compiler used by Cargo and recorded in the receipt.
    pub rustc: PathBuf,
    /// Stable compatibility domain for bounded store admission.
    pub domain: String,
    /// Bounded store selection and policy.
    pub store: OvenStoreCommandOptions,
    /// Requested rendering format.
    pub format: OvenOutputFormat,
}

/// Inputs for the hidden baker that emits one complete compiler-owned Loaf envelope.
#[derive(Debug, Clone)]
pub struct OvenLoafBakeCommandOptions {
    /// Compiler or staged toolchain root used to derive runtime source identity.
    pub compiler_root: PathBuf,
    /// Destination for immutable `<identity>.loaf` directories.
    pub output: PathBuf,
    /// Bounded compiler-suite store baked beside a compiler-suite Loaf envelope.
    pub suite_store: Option<PathBuf>,
    /// Built-in release or compiler-suite envelope.
    pub envelope: OvenLoafEnvelopeArgument,
    /// Exact SDK provider inventory used to derive compatibility identities.
    pub sdk_inventory: PathBuf,
    /// Cargo executable used only by this explicit baker.
    pub cargo: PathBuf,
    /// Rust compiler used by the baker and recorded by each receipt.
    pub rustc: PathBuf,
    /// Aggregate physical allowance for the selected envelope.
    pub max_physical_bytes: Option<u64>,
    /// Per-Loaf physical allowance.
    pub max_domain_physical_bytes: Option<u64>,
    /// Per-Loaf logical allowance.
    pub max_domain_logical_bytes: Option<u64>,
    /// Requested rendering format.
    pub format: OvenOutputFormat,
}

/// Inputs for the Cargo-free compiler workspace-test consumer.
#[derive(Debug, Clone)]
pub struct OvenCompilerLibtestsRunCommandOptions {
    /// Repository root containing the compiler Cargo package and `src/lib.rs`.
    pub compiler_root: PathBuf,
    /// Optional explicit Rust compiler; the active toolchain is resolved when absent.
    pub rustc: Option<PathBuf>,
    /// Requested root-package feature names; default Cargo features remain enabled.
    pub features: Vec<String>,
    /// Optional receipt-bound test source paths selected from the stored suite.
    ///
    /// With no selection the consumer executes every stored root. A selection is a diagnostic and development aid,
    /// not a second suite definition: every requested path must match one indexed source root in the receipt-bound
    /// payload.
    pub targets: Vec<String>,
    /// Caller-owned directory for linked stored test executables.
    pub output: Option<PathBuf>,
    /// Bounded store selection and policy.
    pub store: OvenStoreCommandOptions,
    /// Requested rendering format.
    pub format: OvenOutputFormat,
}

/// Compiler-owned receipt destination for the full native workspace-test compatibility unit.
const COMPILER_LIBTEST_RECEIPT_RELATIVE_PATH: &str = ".incan/oven/compiler-libtests-receipt.json";

/// Inputs for `incan oven test`.
#[derive(Debug, Clone)]
pub struct OvenTestCommandOptions {
    /// Persisted receipt authorizing source and selected direct-rustc plan.
    pub receipt: PathBuf,
    /// Exact immutable store identity of the direct-rustc plan.
    pub plan_identity: String,
    /// Explicit Rust compiler executable.
    pub rustc: PathBuf,
    /// Generated Rust test source authorized by receipt supplemental evidence.
    pub source: PathBuf,
    /// Caller-owned test executable path.
    pub output: PathBuf,
    /// Rust test crate name.
    pub crate_name: String,
    /// Supported Rust edition.
    pub edition: String,
    /// Receipt supplemental source-evidence key for `source`.
    pub source_evidence_key: String,
    /// Exact test names selected only after a full native inventory.
    pub exact_names: Vec<String>,
    /// Bounded store selection and policy.
    pub store: OvenStoreCommandOptions,
    /// Requested rendering format.
    pub format: OvenOutputFormat,
}

/// Inputs for `incan oven run`.
#[derive(Debug, Clone)]
pub struct OvenRunCommandOptions {
    /// Persisted receipt authorizing source and selected direct-rustc plan.
    pub receipt: PathBuf,
    /// Exact immutable store identity of the direct-rustc plan.
    pub plan_identity: String,
    /// Explicit Rust compiler executable.
    pub rustc: PathBuf,
    /// Generated Rust binary source authorized by receipt supplemental evidence.
    pub source: PathBuf,
    /// Caller-owned binary output path.
    pub output: PathBuf,
    /// Rust binary crate name.
    pub crate_name: String,
    /// Supported Rust edition.
    pub edition: String,
    /// Receipt supplemental source-evidence key for `source`.
    pub source_evidence_key: String,
    /// Explicit arguments forwarded only to the compiled native binary.
    pub arguments: Vec<OsString>,
    /// Bounded store selection and policy.
    pub store: OvenStoreCommandOptions,
    /// Requested rendering format.
    pub format: OvenOutputFormat,
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
        domain: options.domain,
        publication_kind: OvenLegacyCargoPublicationKind::Executable,
        source_evidence_key: "generated-root".to_string(),
        compile_environment: std::collections::BTreeMap::new(),
        compact_debug_info: false,
    })
    .map_err(oven_error)?;
    match options.format {
        OvenOutputFormat::Text => println!(
            "Prepared Oven direct-rustc plan {} through the explicit legacy_cargo publisher.",
            result.plan_identity
        ),
        OvenOutputFormat::Json => print_json(&result)?,
    }
    Ok(ExitCode::SUCCESS)
}

/// Result for one checked fixture in a built-in Loaf envelope.
#[derive(Debug, Serialize)]
struct OvenLoafBakeEntryReport {
    label: String,
    profile: String,
    action: String,
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
    cargo_process_started: bool,
    evidence: OvenLoafEnvelopeEvidence,
    loafs: Vec<OvenLoafBakeEntryReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    compiler_suite: Option<OvenCompilerSuiteBakeReport>,
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
    compiler_executable_digest: String,
    sdk_inventory_digest: String,
    rustc_identity: String,
    lock_digest: String,
    fixture_digest: String,
}

/// Return the stable wire name used by one built-in Loaf envelope.
fn loaf_envelope_name(envelope: OvenLoafEnvelope) -> &'static str {
    match envelope {
        OvenLoafEnvelope::Release => "release",
        OvenLoafEnvelope::CompilerSuite => "compiler-suite",
    }
}

/// Convert typed envelope evidence into the canonical string map persisted by `envelope.json`.
fn loaf_envelope_evidence_map(evidence: &OvenLoafEnvelopeEvidence) -> CliResult<BTreeMap<String, String>> {
    serde_json::to_value(evidence)
        .map_err(|error| CliError::failure(format!("could not encode Loaf envelope evidence: {error}")))?
        .as_object()
        .ok_or_else(|| CliError::failure("Loaf envelope evidence is not an object".to_string()))?
        .iter()
        .map(|(key, value)| Ok((key.clone(), value.as_str().unwrap_or_default().to_string())))
        .collect()
}

/// Digest every compiler, SDK, Rust toolchain, lock, and checked-fixture input that can change a built-in envelope.
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
    let lock_path = [
        compiler_root.join("Cargo.lock"),
        compiler_root.join("crates/Cargo.lock"),
    ]
    .into_iter()
    .find(|path| path.is_file())
    .ok_or_else(|| CliError::failure("Loaf compiler root has no canonical Cargo.lock input".to_string()))?;
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
            })
        })
        .collect::<Vec<_>>();
    Ok(OvenLoafEnvelopeEvidence {
        compiler_executable_digest: read_digest(compiler_executable, "compiler executable")?,
        sdk_inventory_digest: read_digest(sdk_inventory, "SDK inventory")?,
        rustc_identity: rustc_identity(rustc).map_err(oven_error)?,
        lock_digest: read_digest(&lock_path, "lock input")?,
        fixture_digest: digest_bytes(
            &serde_json::to_vec(&fixture_evidence)
                .map_err(|error| CliError::failure(format!("could not encode Loaf fixture evidence: {error}")))?,
        ),
    })
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
    let expected_evidence = loaf_envelope_evidence_map(evidence)?;
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
        {
            return Err(CliError::failure(
                "Loaf envelope manifest does not match its checked specification".to_string(),
            ));
        }
        let loaf_path = output.join(&entry.path);
        let result = validate_stored_loaf(&loaf_path, &entry.build_unit_identity).map_err(oven_error)?;
        if result.loaf_identity != entry.loaf_identity {
            return Err(CliError::failure(format!(
                "Loaf `{}` content identity does not match the committed envelope manifest",
                entry.label
            )));
        }
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
    retire_unreferenced_loaf_generations(output, &manifest.generation_identity, scratch)?;
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
        cargo_process_started: false,
        evidence: evidence.clone(),
        loafs: reports,
        compiler_suite: None,
    }))
}

/// Move every generation except the committed authority into owner-scoped scratch for safe reclamation.
fn retire_unreferenced_loaf_generations(output: &Path, generation_identity: &str, scratch: &Path) -> CliResult<()> {
    let generations_root = output.join("generations");
    if !generations_root.is_dir() {
        return Ok(());
    }
    let active_name = generation_identity
        .strip_prefix("sha256:")
        .unwrap_or(generation_identity);
    let active_generation = generations_root.join(active_name);
    let retired_root = scratch.join("retired");
    fs::create_dir_all(&retired_root)
        .map_err(|error| CliError::failure(format!("could not create retired Loaf directory: {error}")))?;
    for entry in fs::read_dir(&generations_root)
        .map_err(|error| CliError::failure(format!("could not inspect Loaf generations: {error}")))?
    {
        let entry = entry.map_err(|error| CliError::failure(format!("could not inspect Loaf output: {error}")))?;
        let path = entry.path();
        if path != active_generation {
            fs::rename(&path, retired_root.join(entry.file_name())).map_err(|error| {
                CliError::failure(format!("could not retire obsolete Loaf {}: {error}", path.display()))
            })?;
        }
    }
    Ok(())
}

/// Durably publish a staged generation before atomically switching the envelope manifest authority.
fn commit_loaf_generation(
    output: &Path,
    generations_root: &Path,
    generation_output: &Path,
    staged_root: &Path,
    manifest: &OvenLoafEnvelopeManifest,
    scratch: &Path,
    before_manifest_commit: impl FnOnce() -> io::Result<()>,
) -> CliResult<()> {
    crate::oven::store::sync_directory_tree(staged_root).map_err(oven_error)?;
    if generation_output.exists() {
        let abandoned = scratch.join("abandoned-generation");
        fs::rename(generation_output, &abandoned).map_err(|error| {
            CliError::failure(format!(
                "could not quarantine an uncommitted Loaf generation {}: {error}",
                generation_output.display()
            ))
        })?;
    }
    fs::rename(staged_root, generation_output).map_err(|error| {
        CliError::failure(format!(
            "could not atomically publish Loaf generation {}: {error}",
            generation_output.display()
        ))
    })?;
    crate::oven::store::sync_directory(generations_root.to_path_buf()).map_err(oven_error)?;
    let staged_manifest = scratch.join("envelope.json");
    fs::write(
        &staged_manifest,
        serde_json::to_vec_pretty(manifest)
            .map_err(|error| CliError::failure(format!("could not encode Loaf envelope manifest: {error}")))?,
    )
    .map_err(|error| CliError::failure(format!("could not stage Loaf envelope manifest: {error}")))?;
    File::open(&staged_manifest)
        .and_then(|file| file.sync_all())
        .map_err(|error| CliError::failure(format!("could not synchronize Loaf envelope manifest: {error}")))?;
    before_manifest_commit()
        .map_err(|error| CliError::failure(format!("Loaf envelope manifest commit was interrupted: {error}")))?;
    fs::rename(&staged_manifest, output.join("envelope.json"))
        .map_err(|error| CliError::failure(format!("could not publish Loaf envelope manifest atomically: {error}")))?;
    crate::oven::store::sync_directory(output.to_path_buf()).map_err(oven_error)
}

/// Bind a checked Loaf fixture probe to the compiler selected by the baker.
///
/// The explicit Cargo executable may come from a nightly toolchain solely because the compiler-suite unit graph
/// requires Cargo's unstable `--unit-graph` interface. That must not let the ambient Rustup toolchain choose the
/// compiler recorded in the receipt: `--rustc` is the compatibility authority for both the probe and publication.
fn pin_loaf_fixture_rustc(command: &mut Command, rustc: &Path) {
    command.env("RUSTC", rustc);
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
    let generation_identity = digest_bytes(
        &serde_json::to_vec(&(loaf_envelope_name(envelope), &evidence))
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
    let mut cargo_process_started = false;
    let mut transient_peak_physical_bytes = 0_u64;
    let compiler_support_target = scratch.path().join("compiler-support-target");
    for specification in loaf_envelope_specifications(envelope) {
        let project_root = scratch.path().join("fixtures").join(specification.label);
        fs::create_dir_all(&project_root).map_err(|error| {
            CliError::failure(format!(
                "could not create checked Loaf fixture {}: {error}",
                specification.label
            ))
        })?;
        let source = project_root.join("main.incn");
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
        pin_loaf_fixture_rustc(&mut command, &options.rustc);
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
        if !probe.status.success()
            && !String::from_utf8_lossy(&probe.stderr).contains("Oven Alpha has no compatible native")
        {
            return Err(CliError::failure(format!(
                "checked Loaf fixture `{}` failed before its expected Oven miss:\n{}",
                specification.label,
                String::from_utf8_lossy(&probe.stderr).trim()
            )));
        }
        if !receipt_path.is_file() || !generated_project.is_dir() {
            return Err(CliError::failure(format!(
                "checked Loaf fixture `{}` did not produce its receipt and generated project",
                specification.label
            )));
        }
        let receipt = read_receipt(&receipt_path)?;
        cargo_process_started = true;
        let result = prepare_loaf_from_generated_project(
            &staged_root,
            &OvenLoafBakerContext {
                compiler_support_target: &compiler_support_target,
                capacity_roots: [&options.output, scratch.path()],
                transient_limit: max_physical_bytes,
                cargo: &options.cargo,
                rustc: &options.rustc,
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
            result,
        });
    }

    let logical_bytes = pending.iter().map(|entry| entry.result.logical_bytes).sum::<u64>();
    let physical_bytes = pending.iter().map(|entry| entry.result.physical_bytes).sum::<u64>();
    if physical_bytes > max_physical_bytes {
        return Err(CliError::failure(format!(
            "Loaf envelope uses {physical_bytes} physical bytes, exceeding its {max_physical_bytes}-byte allowance"
        )));
    }

    let prepared_count = pending.len();
    let manifest = OvenLoafEnvelopeManifest {
        schema_version: OVEN_LOAF_ENVELOPE_MANIFEST_SCHEMA_VERSION,
        envelope: loaf_envelope_name(envelope).to_string(),
        generation_identity: generation_identity.clone(),
        evidence: loaf_envelope_evidence_map(&evidence)?,
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
                    build_unit_identity: entry.result.build_unit_identity.clone(),
                    loaf_identity: entry.result.loaf_identity.clone(),
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
    )?;

    // The new manifest is the envelope's single authority. Retire old content-addressed generations only after that
    // authority has committed, so any earlier publication failure leaves the previous complete envelope usable.
    // A retirement failure is safe: the newly committed Loafs remain valid and obsolete unreferenced data can be
    // reclaimed by the next successful bake.
    retire_unreferenced_loaf_generations(&options.output, &generation_identity, scratch.path())?;

    let (_, owned_physical_bytes) = loaf_directory_byte_counts(&options.output).map_err(oven_error)?;
    let raw_disk_bytes = loaf_raw_disk_bytes(&options.output).map_err(oven_error)?;
    if owned_physical_bytes > max_physical_bytes {
        return Err(CliError::failure(format!(
            "published Loaf output uses {owned_physical_bytes} physical bytes after reclaiming obsolete generations, exceeding its {max_physical_bytes}-byte allowance"
        )));
    }
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
    let (receipt, receipt_path) = compiler_libtests_receipt(&options.compiler_root, &options.rustc, &["lsp".into()])?;
    write_receipt(&receipt, &receipt_path).map_err(oven_error)?;
    let store = open_store(&store_options)?;
    let prepare = prepare_compiler_test_suite(&OvenLegacyCargoPrepareRequest {
        store: &store,
        receipt,
        generated_project: options.compiler_root.clone(),
        cargo: options.cargo.clone(),
        rustc: options.rustc.clone(),
        sdk_inventory: Some(options.sdk_inventory.clone()),
        domain: "compiler-suite-lsp".to_string(),
        publication_kind: OvenLegacyCargoPublicationKind::LibraryTests,
        source_evidence_key: "compiler-libtest-root".to_string(),
        compile_environment: BTreeMap::new(),
        compact_debug_info: false,
    })
    .map_err(oven_error)?;
    let store_inspection = store.inspect().map_err(oven_error)?;
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
    let suite_reused = prepare.cargo_version == "not-run-existing-suite";
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
                "{} {} Oven Loafs ({} logical, {} physical; Cargo baker {}).",
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
                    "Compiler suite: {} ({} logical, {} physical).",
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
/// a normal test workflow.
pub fn oven_run_compiler_libtests(options: OvenCompilerLibtestsRunCommandOptions) -> CliResult<ExitCode> {
    let rustc = options.rustc.unwrap_or(resolve_active_rustc().map_err(oven_error)?);
    let (receipt, receipt_path) = compiler_libtests_receipt(&options.compiler_root, &rustc, &options.features)?;
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
    if !matches!(suite.schema_version, 8..=13) {
        return Err(CliError::failure(format!(
            "stored Oven compiler suite payload schema {} is unsupported",
            suite.schema_version
        )));
    }
    if suite.schema_version == 8 && !options.targets.is_empty() {
        return Err(CliError::failure(
            "stored schema-8 compiler suites do not support target selection; republish an indexed Oven suite"
                .to_string(),
        ));
    }
    let selected_shard_references =
        compiler_suite_selected_shard_references(&suite.shard_references, &options.targets)?;
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
        11..=13 => {
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
    // Schema 12 derives its compiler-owned generated-code check capability from the workspace-library graph. A
    // focused target need not itself own `incan_stdlib`, so retain the complete receipt-bound shard lease set solely
    // while deriving that shared capability. This does not broaden execution: the prepared-child queue below still
    // contains only `selected_shard_references`. It also avoids a fabricated ambient closure or Cargo recovery path.
    let warning_check_shards = if matches!(suite.schema_version, 12 | 13) && !options.targets.is_empty() {
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
        9..=13 => suite.cli_artifact_closure.as_ref().ok_or_else(|| {
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
    let compiler_data_root = if suite.schema_version == 13 {
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
    let warning_check_artifacts = if matches!(suite.schema_version, 12 | 13) {
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
    environment.insert(
        "CARGO_BIN_EXE_incan".to_string(),
        compiler_suite_environment_path(&cli_bake.output)?.display().to_string(),
    );
    let run_children = || -> CliResult<(CompilerSuiteChildrenReport, usize, usize)> {
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
            )?;
            Ok((suite_report, suite.test_targets.len(), suite.binary_targets.len()))
        } else {
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
                let foundations = matches!(suite.schema_version, 10..=13).then_some(&foundation_executions);
                let foundation_references = if matches!(suite.schema_version, 10..=13) {
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
                )?);
                planned_binary_count += shard.payload.binary_targets.len();
            }
            let suite_report = run_prepared_compiler_suite_children(prepared_children, &receipt, &rustc)?;
            Ok((suite_report, shard_executions.len(), planned_binary_count))
        }
    };
    let (mut suite_report, planned_target_count, planned_binary_count) =
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
    let success = suite_report.failed.is_empty()
        && native_test_case_totals.unreported_roots == 0
        && native_test_case_totals.reported_roots + suite_report.doctest_targets == planned_target_count;
    let store_inspection = store.inspect().map_err(oven_error)?;
    let report_path = output_directory.join("compiler-suite-report.json");
    let report = serde_json::json!({
        "success": success,
        "receipt": receipt_path.display().to_string(),
        "report_path": report_path.display().to_string(),
        "native_test_count": suite_report.native_test_count,
        "native_test_case_totals": native_test_case_totals.clone(),
        "native_test_roots": suite_report.native_test_roots.clone(),
        "doctest_targets": suite_report.doctest_targets,
        "test_targets": planned_target_count,
        "binary_targets": planned_binary_count,
        "suite_schema_version": suite.schema_version,
        "shard_count": shard_executions.len(),
        "compiler_cli_reused": cli_bake.reused,
        "cargo_process_started": false,
        "store": store_inspection,
        "failures": suite_report.failed.clone(),
    });
    write_compiler_suite_report(&report_path, &report)?;
    if !success {
        if matches!(options.format, OvenOutputFormat::Json) {
            print_json(&report)?;
            return Ok(ExitCode::FAILURE);
        }
        return Err(CliError::failure(format!(
            "compiler workspace native tests failed after Cargo-free Oven execution: {} passed, {} failed, {} ignored across {} reported libtest root(s): {} green, {} failing, with {} root(s) lacking a terminal libtest summary.\n{}",
            native_test_case_totals.passed,
            native_test_case_totals.failed,
            native_test_case_totals.ignored,
            native_test_case_totals.reported_roots,
            native_test_case_totals.green_roots,
            native_test_case_totals.failed_roots,
            native_test_case_totals.unreported_roots,
            suite_report.failed.join("\n\n")
        )));
    }
    match options.format {
        OvenOutputFormat::Text => println!(
            "Oven executed {} compiler workspace native test(s): {} passed, {} failed, {} ignored across {} reported root(s): {} green and {} failing, plus {} doctest target(s), through its Cargo-free stored target plan (receipt {}).",
            suite_report.native_test_count,
            native_test_case_totals.passed,
            native_test_case_totals.failed,
            native_test_case_totals.ignored,
            native_test_case_totals.reported_roots,
            native_test_case_totals.green_roots,
            native_test_case_totals.failed_roots,
            suite_report.doctest_targets,
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
        (
            "INCAN_OVEN_COMPILER_SUITE_RUSTC".to_string(),
            rustc.display().to_string(),
        ),
        (
            "INCAN_OVEN_COMPILER_SUITE_STDLIB".to_string(),
            stdlib_extern.display().to_string(),
        ),
        (
            "INCAN_OVEN_COMPILER_SUITE_DEPENDENCY_PATH_COUNT".to_string(),
            warning_check_artifacts.dependency_search_paths.len().to_string(),
        ),
        (
            "INCAN_OVEN_COMPILER_SUITE_EXTERN_COUNT".to_string(),
            warning_check_artifacts.externs.len().to_string(),
        ),
    ]);
    for (index, path) in warning_check_artifacts.dependency_search_paths.iter().enumerate() {
        environment.insert(
            format!("INCAN_OVEN_COMPILER_SUITE_DEPENDENCY_PATH_{index}"),
            compiler_suite_environment_path(path)?.display().to_string(),
        );
    }
    for (index, (crate_name, path)) in warning_check_artifacts.externs.iter().enumerate() {
        environment.insert(
            format!("INCAN_OVEN_COMPILER_SUITE_EXTERN_{index}_NAME"),
            crate_name.clone(),
        );
        environment.insert(
            format!("INCAN_OVEN_COMPILER_SUITE_EXTERN_{index}_PATH"),
            compiler_suite_environment_path(path)?.display().to_string(),
        );
    }
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
        "INCAN_OVEN_COMPILER_SUITE_VOCAB",
        rustc,
        vocab_extraction_artifacts,
    )?;
    Ok(environment)
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
    prefix: &str,
    rustc: &Path,
    artifacts: &crate::oven::rustc::OvenRustcArtifactPlan,
) -> CliResult<()> {
    environment.insert(
        format!("{prefix}_RUSTC"),
        compiler_suite_environment_path(rustc)?.display().to_string(),
    );
    environment.insert(
        format!("{prefix}_DEPENDENCY_PATH_COUNT"),
        artifacts.dependency_search_paths.len().to_string(),
    );
    environment.insert(format!("{prefix}_EXTERN_COUNT"), artifacts.externs.len().to_string());
    for (index, path) in artifacts.dependency_search_paths.iter().enumerate() {
        environment.insert(
            format!("{prefix}_DEPENDENCY_PATH_{index}"),
            compiler_suite_environment_path(path)?.display().to_string(),
        );
    }
    for (index, (crate_name, path)) in artifacts.externs.iter().enumerate() {
        environment.insert(format!("{prefix}_EXTERN_{index}_NAME"), crate_name.clone());
        environment.insert(
            format!("{prefix}_EXTERN_{index}_PATH"),
            compiler_suite_environment_path(path)?.display().to_string(),
        );
    }
    Ok(())
}

/// Place the suite-built CLI beside its caller-owned workspace libraries.
///
/// Direct CLI builds can prefer dynamic workspace libraries. A fixed compiler-root target path would outlive the
/// caller output and become unusable when the scheduler responsibly reclaims an interrupted run's artifacts.
fn compiler_suite_cli_output(output_directory: &Path) -> PathBuf {
    output_directory.join("compiler-cli/incan")
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
    let workspace_outputs = bake_planned_compiler_suite_workspace_libraries(
        &shard.payload.workspace_libraries,
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
) -> CliResult<PreparedCompilerSuiteChild<'a>> {
    let source = compiler_suite_target_source(compiler_root, target)?;
    let output = output_directory.join(compiler_suite_target_output_name(0, target));
    let prefer_dynamic = target.target_kind == "proc-macro"
        || compiler_suite_workspace_outputs_include_dylib(&workspace_library_outputs);
    let mut target_environment = environment.clone();
    if !compiler_suite_target_requires_generated_rust_closure(&target.source_relative_path) {
        compiler_suite_remove_generated_rust_closure(&mut target_environment);
    }
    let mut binary_compile_environment = BTreeMap::new();
    // A parallel child may itself invoke `incan`. Give that nested command an isolated mutable home while it reads
    // the shared, leased provider store through the receipt-bound environment above.
    target_environment.insert(
        "INCAN_HOME".to_string(),
        compiler_suite_environment_path(&output_directory.join("incan-home"))?
            .display()
            .to_string(),
    );
    // Nested normal commands may still use generated-project state for source and release-asset fixtures.  A
    // scheduler-wide target directory lets otherwise independent suite roots overwrite that mutable state while
    // they run in parallel.  Keep it with the caller-owned child output directory just like INCAN_HOME; this does
    // not affect the parent-leased immutable native provider closure.
    target_environment.insert(
        "INCAN_GENERATED_CARGO_TARGET_DIR".to_string(),
        compiler_suite_environment_path(&output_directory.join("generated-cargo-target"))?
            .display()
            .to_string(),
    );
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

/// Return whether a stored root compiles generated Rust through the scheduler-provided direct-rustc closure.
///
/// The closure is needed by every root whose nested normal command may consume the direct generated-Rust artifacts.
/// The toolchain-installer root is the one deliberate exception: its Homebrew smoke launches a self-contained
/// fixture compiler and inheriting the large closure overflows that nested process.
fn compiler_suite_target_requires_generated_rust_closure(source_relative_path: &str) -> bool {
    source_relative_path != "tests/toolchain_installer_tests.rs"
}

/// Remove direct generated-Rust closure details while retaining the suite marker used by Cargo-free fixture paths.
fn compiler_suite_remove_generated_rust_closure(environment: &mut BTreeMap<String, String>) {
    environment
        .retain(|key, _| !key.starts_with("INCAN_OVEN_COMPILER_SUITE_") || key == "INCAN_OVEN_COMPILER_SUITE_RUSTC");
}

/// Compile and execute prepared compiler-suite roots with a bounded worker pool.
///
/// Workers start only after the one shared DAG/preparation phase has completed, so parallelism improves root
/// throughput without multiplying provider setup, Cargo-compatible publication, store leases, or generated homes.
fn run_prepared_compiler_suite_children(
    children: Vec<PreparedCompilerSuiteChild<'_>>,
    receipt: &OvenReceipt,
    rustc: &Path,
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
                            run_prepared_compiler_suite_child(child, receipt, rustc).map_err(|error| error.to_string());
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
            let working_directory =
                compiler_suite_target_working_directory(child.compiler_root, &child.source, child.target)?;
            let report = run_native_test_batch_all_in_directory_with_timeout(
                &bake.output,
                &child.environment,
                Some(&working_directory),
                Some(OVEN_COMPILER_TEST_ROOT_TIMEOUT),
            )
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
                native_test_count: report.inventory.names.len(),
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
                }],
            })
        }
        "rustdoc-test" => {
            let temporary_directory = child.output.with_extension("rustdoc-tmp");
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
/// Each direct-Rustc root can itself run a test harness with internal parallelism and launch normal Incan commands.
/// Running one outer worker per CPU therefore oversubscribes constrained CI hosts despite being technically bounded.
/// Reserve headroom while still scaling with the host: one core runs one worker, 2--3 cores run two, 4--7 run three,
/// and larger hosts run four. An explicit operator override remains available for measured release-machine tuning.
fn compiler_suite_auto_parallel_jobs(logical_cores: usize) -> usize {
    match logical_cores {
        0 | 1 => 1,
        2..=3 => 2,
        4..=7 => 3,
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
) -> CliResult<CompilerSuiteChildrenReport> {
    let mut suite_report = CompilerSuiteChildrenReport::default();
    for (index, target) in targets.iter().enumerate() {
        let mut artifacts = closure.manifest_for_target(target, intent.clone());
        let source = compiler_suite_target_source(compiler_root, target)?;
        let output = output_directory.join(compiler_suite_target_output_name(index, target));
        let prefer_dynamic = target.target_kind == "proc-macro"
            || compiler_suite_workspace_outputs_include_dylib(workspace_library_outputs);
        let mut target_environment = environment.clone();
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
}

/// Complete native-test coverage for one compiler-suite invocation.
#[derive(Debug, Default)]
struct CompilerSuiteChildrenReport {
    native_test_count: usize,
    doctest_targets: usize,
    failed: Vec<String>,
    native_test_roots: Vec<CompilerSuiteNativeTestRootReport>,
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
/// authority. This keeps focused diagnosis representative of the same direct-Rustc roots that a complete Oven run
/// will execute, while making an unknown or ambiguous source path fail closed.
fn compiler_suite_selected_shard_references(
    references: &[OvenCompilerTestSuiteShardReference],
    requested_targets: &[String],
) -> CliResult<Vec<OvenCompilerTestSuiteShardReference>> {
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
    let mut selected = store
        .select_payloads_matching_for_execution(|manifest| {
            manifest.kind == OvenArtifactKind::CompilerTestSuite
                && manifest.build_unit_identity == receipt.build_unit_identity
                && manifest.intent == receipt.intent
        })
        .map_err(oven_error)?;
    match selected.len() {
        1 => Ok(selected.remove(0)),
        0 => Err(CliError::failure(format!(
            "no Oven compiler test suite is prepared for this exact receipt. The requested compiler-suite provider/dependency unit is not available for {} with rustc {}.",
            compiler_root.display(),
            rustc.display(),
        ))),
        _ => Err(CliError::failure(format!(
            "multiple Oven compiler test suites are prepared for one build unit: {}",
            selected
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

/// Keep terminal failure reporting actionable without dumping an unbounded libtest transcript into the CLI error path.
///
/// The complete caller-owned transcript is retained beside the direct-rustc test binary on failure.
fn native_test_failure_summary(output: &str) -> String {
    const MAX_CHARS: usize = 12_000;
    let relevant = output
        .lines()
        .filter(|line| {
            line.contains("FAILED")
                || line.contains("panicked")
                || line.contains("Error:")
                || line.starts_with("error:")
                || line.contains("test result:")
        })
        .take(96)
        .collect::<Vec<_>>()
        .join("\n");
    let summary = if relevant.is_empty() { output } else { &relevant };
    let mut bounded = summary.chars().take(MAX_CHARS).collect::<String>();
    if summary.chars().count() > MAX_CHARS {
        bounded.push_str("\n… libtest transcript truncated");
    }
    bounded
}

/// Recompute the source-bound compiler root libtest receipt without invoking Cargo.
fn compiler_libtests_receipt(
    compiler_root: &Path,
    rustc: &Path,
    requested_features: &[String],
) -> CliResult<(OvenReceipt, PathBuf)> {
    let target = rustc_host_target(rustc).map_err(oven_error)?;
    let toolchain = rustc_identity(rustc).map_err(oven_error)?;
    let features = compiler_root_feature_selection(compiler_root, requested_features)?;
    let receipt = receipt_native_compiler_suite(&OvenCompilerSuiteRequest::new(
        compiler_root,
        target,
        toolchain,
        OVEN_COMPILER_TEST_PROFILE,
        features,
    ))
    .map_err(oven_error)?;
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

/// Read and verify a persisted receipt before it authorizes another Oven stage.
fn read_receipt(path: &Path) -> CliResult<OvenReceipt> {
    let bytes = fs::read(path)
        .map_err(|error| CliError::failure(format!("failed to read Oven receipt {}: {error}", path.display())))?;
    let receipt = serde_json::from_slice::<OvenReceipt>(&bytes)
        .map_err(|error| CliError::failure(format!("failed to parse Oven receipt {}: {error}", path.display())))?;
    receipt.verify_identity().map_err(oven_error)?;
    Ok(receipt)
}

/// Resolve the one compiler-owned default store root or a caller-explicit root without consulting Cargo state.
fn open_store(options: &OvenStoreCommandOptions) -> CliResult<OvenStore> {
    open_store_with_defaults(
        options,
        OvenStoreLimits::new(
            DEFAULT_OVEN_MAX_PHYSICAL_BYTES,
            DEFAULT_OVEN_MAX_DOMAIN_PHYSICAL_BYTES,
            DEFAULT_OVEN_MAX_DOMAIN_LOGICAL_BYTES,
        ),
    )
}

/// Open one bounded store using the product profile owned by its command surface.
fn open_store_with_defaults(options: &OvenStoreCommandOptions, defaults: OvenStoreLimits) -> CliResult<OvenStore> {
    let root = match &options.root {
        Some(root) => root.clone(),
        None => default_store_root(env::var_os("INCAN_HOME"), user_home()).ok_or_else(|| {
            CliError::failure("cannot resolve the Oven store root; set INCAN_HOME, HOME, or pass --store")
        })?,
    };
    Ok(OvenStore::new(root, resolve_limits_with_defaults(options, defaults)?))
}

/// Open the one policy-bounded Oven store used by ordinary Alpha commands.
///
/// This keeps normal `build`, `run`, and `test` on the same receipt-owned store as the explicit inspection commands;
/// normal execution never accepts a generated-Cargo target directory as a storage selector.
pub(crate) fn open_default_oven_store() -> CliResult<OvenStore> {
    open_store(&OvenStoreCommandOptions {
        root: None,
        max_physical_bytes: None,
        max_domain_physical_bytes: None,
        max_domain_logical_bytes: None,
    })
}

/// Resolve bounded policy with one command-owned product profile and the real process environment.
fn resolve_limits_with_defaults(
    options: &OvenStoreCommandOptions,
    defaults: OvenStoreLimits,
) -> CliResult<OvenStoreLimits> {
    resolve_limits_with_environment_and_defaults(options, |name| env::var(name).ok(), defaults)
}

/// Apply CLI and environment overrides over one explicit product-owned default profile.
fn resolve_limits_with_environment_and_defaults(
    options: &OvenStoreCommandOptions,
    environment_value: impl Fn(&str) -> Option<String>,
    defaults: OvenStoreLimits,
) -> CliResult<OvenStoreLimits> {
    let aggregate = match options.max_physical_bytes {
        Some(value) => value,
        None => parse_limit_value(
            OVEN_MAX_PHYSICAL_BYTES_ENV,
            environment_value(OVEN_MAX_PHYSICAL_BYTES_ENV),
            defaults.max_physical_bytes,
        )?,
    };
    let domain_physical = match options.max_domain_physical_bytes {
        Some(value) => value,
        None => parse_limit_value(
            OVEN_MAX_DOMAIN_PHYSICAL_BYTES_ENV,
            environment_value(OVEN_MAX_DOMAIN_PHYSICAL_BYTES_ENV),
            defaults.max_domain_physical_bytes,
        )?,
    };
    let domain_logical = match options.max_domain_logical_bytes {
        Some(value) => value,
        None => parse_limit_value(
            OVEN_MAX_DOMAIN_LOGICAL_BYTES_ENV,
            environment_value(OVEN_MAX_DOMAIN_LOGICAL_BYTES_ENV),
            defaults.max_domain_logical_bytes,
        )?,
    };
    if aggregate == 0 || domain_physical == 0 || domain_logical == 0 {
        return Err(CliError::failure(
            "Oven storage policy limits must be greater than zero",
        ));
    }
    if domain_physical > aggregate {
        return Err(CliError::failure(
            "Oven per-domain physical policy must not exceed aggregate physical policy",
        ));
    }
    Ok(OvenStoreLimits::new(aggregate, domain_physical, domain_logical))
}

/// Parse one explicit byte-count environment variable without accepting ambiguous unit suffixes.
fn parse_limit_value(name: &str, value: Option<String>, default: u64) -> CliResult<u64> {
    match value {
        Some(value) if !value.trim().is_empty() => value
            .trim()
            .parse::<u64>()
            .map_err(|error| CliError::failure(format!("invalid {name} value `{value}`; expected bytes: {error}"))),
        Some(_) | None => Ok(default),
    }
}

/// Resolve the versioned Oven store location below `INCAN_HOME` before the user home directory.
fn default_store_root(incan_home: Option<OsString>, home: Option<OsString>) -> Option<PathBuf> {
    incan_home
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            home.filter(|path| !path.is_empty())
                .map(|path| PathBuf::from(path).join(".incan"))
        })
        .map(|root| root.join("oven").join("store").join("v1"))
}

/// Return the platform home environment used by installed Incan binaries.
fn user_home() -> Option<OsString> {
    env::var_os("HOME").or_else(|| env::var_os("USERPROFILE"))
}

/// Resolve the toolchain-manager state needed when a compiler self-test deliberately exercises Rustup fallback.
///
/// Stored normal commands receive a verified absolute `RUSTC`; this path exists solely because compiler tests also
/// verify Rustup discovery after removing that explicit variable. It is intentionally separate from Cargo state.
fn default_rustup_home(rustup_home: Option<OsString>, home: Option<OsString>) -> Option<PathBuf> {
    rustup_home
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            home.filter(|path| !path.is_empty())
                .map(|path| PathBuf::from(path).join(".rustup"))
        })
}

/// Parse a named source argument with a portable digest key and a filesystem input path.
fn parse_named_path(value: &str) -> CliResult<(String, PathBuf)> {
    let Some((name, path)) = value.split_once('=') else {
        return Err(CliError::failure(format!(
            "invalid Oven --source `{value}`; expected NAME=PATH"
        )));
    };
    let name = name.trim();
    let path = path.trim();
    if name.is_empty() || path.is_empty() {
        return Err(CliError::failure(format!(
            "invalid Oven --source `{value}`; expected NAME=PATH"
        )));
    }
    Ok((name.to_string(), PathBuf::from(path)))
}

/// Persist a complete scheduler aggregate beside caller-owned test outputs.
///
/// The terminal is intentionally a convenience surface and can be detached by a CI or desktop-session wrapper.
/// The report is therefore a normal caller-owned output, not an immutable-store artifact, and remains available for
/// a failed batch as well as a green batch. Atomic replacement prevents a reader from observing a partial summary.
fn write_compiler_suite_report(path: &Path, report: &serde_json::Value) -> CliResult<()> {
    let parent = path.parent().ok_or_else(|| {
        CliError::failure(format!(
            "compiler-suite report path {} has no parent directory",
            path.display()
        ))
    })?;
    fs::create_dir_all(parent).map_err(|error| {
        CliError::failure(format!(
            "cannot create compiler-suite report directory {}: {error}",
            parent.display()
        ))
    })?;
    let encoded = serde_json::to_vec_pretty(report)
        .map_err(|error| CliError::failure(format!("failed to serialize compiler-suite report: {error}")))?;
    let temporary = parent.join(format!(".compiler-suite-report-{}.tmp", std::process::id()));
    fs::write(&temporary, encoded).map_err(|error| {
        CliError::failure(format!(
            "cannot write compiler-suite report temporary file {}: {error}",
            temporary.display()
        ))
    })?;
    fs::rename(&temporary, path).map_err(|error| {
        CliError::failure(format!(
            "cannot publish compiler-suite report {}: {error}",
            path.display()
        ))
    })
}

/// Serialize a stable JSON report or convert the failure into standard CLI error vocabulary.
fn print_json(value: &impl serde::Serialize) -> CliResult<()> {
    let payload = serde_json::to_string_pretty(value)
        .map_err(|error| CliError::failure(format!("failed to serialize Oven JSON report: {error}")))?;
    println!("{payload}");
    Ok(())
}

/// Render binary byte units for physical allocation and logical artifact-byte accounting without Cargo-cache
/// terminology.
fn human_bytes(bytes: u64) -> String {
    const KIB: u64 = 1024;
    const MIB: u64 = KIB * 1024;
    const GIB: u64 = MIB * 1024;
    if bytes >= GIB {
        format!("{:.1} GiB", bytes as f64 / GIB as f64)
    } else if bytes >= MIB {
        format!("{:.1} MiB", bytes as f64 / MIB as f64)
    } else if bytes >= KIB {
        format!("{:.1} KiB", bytes as f64 / KIB as f64)
    } else {
        format!("{bytes} B")
    }
}

/// Translate all Oven typed failures through the top-level CLI error boundary.
fn oven_error(error: impl std::fmt::Display) -> CliError {
    CliError::failure(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::{
        CompilerSuiteChildrenReport, CompilerSuiteNativeTestRootReport,
        DEFAULT_OVEN_COMPILER_SUITE_MAX_DOMAIN_LOGICAL_BYTES, DEFAULT_OVEN_COMPILER_SUITE_MAX_DOMAIN_PHYSICAL_BYTES,
        DEFAULT_OVEN_COMPILER_SUITE_MAX_PHYSICAL_BYTES, DEFAULT_OVEN_MAX_DOMAIN_LOGICAL_BYTES,
        DEFAULT_OVEN_MAX_DOMAIN_PHYSICAL_BYTES, DEFAULT_OVEN_MAX_PHYSICAL_BYTES, OvenImportCommandOptions,
        OvenLoafBakeCommandOptions, OvenPlanPublishCommandOptions, OvenRunCommandOptions, OvenStoreCommandOptions,
        OvenTestCommandOptions, attach_compiler_suite_target_workspace_libraries, bake_planned_compiler_suite_binaries,
        bake_planned_compiler_suite_workspace_libraries, commit_loaf_generation, compiler_suite_auto_parallel_jobs,
        compiler_suite_cli_output, compiler_suite_completion_failures, compiler_suite_directory,
        compiler_suite_environment, compiler_suite_environment_path, compiler_suite_file,
        compiler_suite_remove_generated_rust_closure, compiler_suite_selected_shard_references,
        compiler_suite_target_requires_generated_rust_closure, default_rustup_home, default_store_root,
        loaf_envelope_default_limits, loaf_envelope_evidence, oven_import, oven_legacy_cargo_bake_loafs,
        oven_publish_direct_rustc_plan, oven_run, oven_test, parse_named_path, prepare_compiler_suite_child,
        resolve_limits_with_environment_and_defaults, retire_unreferenced_loaf_generations,
        reuse_complete_loaf_envelope, run_compiler_suite_children_with_leases_retained,
        run_prepared_compiler_suite_children, select_compiler_suite_shards, write_compiler_suite_report,
        write_native_test_failure_transcript,
    };
    use crate::cli::{ExitCode, OvenLoafEnvelopeArgument, OvenOutputFormat};
    use crate::oven::legacy_cargo::{
        OVEN_COMPILER_TEST_SUITE_SHARD_SCHEMA_VERSION_V1, OvenCompilerTestSuiteArtifactClosure,
        OvenCompilerTestSuiteFoundationReference, OvenCompilerTestSuitePayload, OvenCompilerTestSuiteShardPayload,
        OvenCompilerTestSuiteShardReference, OvenCompilerTestSuiteTarget, OvenCompilerTestSuiteToolchainDataReference,
        OvenCompilerWorkspaceLibrary, OvenCompilerWorkspaceLibraryKey,
    };
    use crate::oven::loaf::{
        OVEN_LOAF_ENVELOPE_MANIFEST_SCHEMA_VERSION, OVEN_LOAF_SCHEMA_VERSION, OvenLoaf, OvenLoafEnvelope,
        OvenLoafEnvelopeManifest, OvenLoafEnvelopeMember, acquire_exclusive_loaf_generation_lock,
    };
    use crate::oven::native_test::OvenNativeTestCaseCounts;
    use crate::oven::rustc::{
        OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION, OvenRustcArtifactManifest, OvenRustcArtifactPlan,
        OvenTrustedDirectRustcTargetRequest, bake_trusted_direct_rustc_run, resolve_active_rustc,
        rustc_dynamic_library_environment, rustc_host_target,
    };
    use crate::oven::store::{OvenArtifactKind, OvenArtifactPublishRequest, OvenStore, OvenStoreLimits};
    use crate::oven::{OvenBuildIntent, digest_bytes};
    use crate::oven::{OvenCompilerSuiteRequest, receipt_native_compiler_suite};
    use std::collections::BTreeMap;
    use std::ffi::OsString;
    use std::fs;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::time::Instant;

    #[test]
    fn loaf_fixture_probe_pins_the_explicit_baker_rustc() {
        let mut command = Command::new("incan");
        command.env("RUSTC", "/ambient/rustc");

        super::pin_loaf_fixture_rustc(&mut command, Path::new("/explicit/stable/rustc"));

        assert_eq!(
            command
                .get_envs()
                .find(|(name, _)| *name == "RUSTC")
                .and_then(|(_, value)| value),
            Some(std::ffi::OsStr::new("/explicit/stable/rustc"))
        );
    }

    #[cfg(unix)]
    #[test]
    fn complete_envelope_manifest_reuses_verified_loafs_without_preparation() -> Result<(), Box<dyn std::error::Error>>
    {
        let output = tempfile::tempdir()?;
        let scratch = tempfile::tempdir()?;
        let compiler_root = tempfile::tempdir()?;
        let tools = tempfile::tempdir()?;
        fs::write(compiler_root.path().join("Cargo.lock"), "version = 4\n")?;
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
        let current_executable = std::env::current_exe()?;
        let evidence = loaf_envelope_evidence(
            OvenLoafEnvelope::Release,
            compiler_root.path(),
            &current_executable,
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
        for (label, profile, action) in [
            ("core-release", "release", "build"),
            ("foundation-debug", "debug", "run"),
        ] {
            let build_unit_identity = digest_bytes(label.as_bytes());
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
                        profile: profile.to_string(),
                        features: Vec::new(),
                    },
                    dependency_search_paths: Vec::new(),
                    native_search_paths: Vec::new(),
                    externs: Vec::new(),
                    entrypoint_externs: BTreeMap::new(),
                    registry_leaves: Vec::new(),
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
                label: label.to_string(),
                profile: profile.to_string(),
                action: action.to_string(),
                build_unit_identity,
                loaf_identity,
                path: relative_directory.join("loaf.json"),
            });
        }
        fs::write(
            output.path().join("envelope.json"),
            serde_json::to_vec(&OvenLoafEnvelopeManifest {
                schema_version: OVEN_LOAF_ENVELOPE_MANIFEST_SCHEMA_VERSION,
                envelope: "release".to_string(),
                generation_identity,
                evidence: super::loaf_envelope_evidence_map(&evidence)?,
                loafs: members,
            })?,
        )?;

        let report = reuse_complete_loaf_envelope(
            output.path(),
            scratch.path(),
            OvenLoafEnvelope::Release,
            &evidence,
            OvenStoreLimits::new(1024 * 1024, 1024 * 1024, 1024 * 1024),
            Instant::now(),
        )?
        .ok_or("matching complete envelope manifest was not reused")?;

        assert_eq!(report.action, "reused");
        assert_eq!(report.reused_count, 2);
        assert_eq!(report.prepared_count, 0);
        assert!(!report.cargo_process_started);
        let json = serde_json::to_value(&report)?;
        for field in [
            "logical_bytes",
            "physical_bytes",
            "owned_physical_bytes",
            "raw_disk_bytes",
            "reclaimable_physical_bytes",
            "active_lease_physical_bytes",
            "transient_peak_physical_bytes",
            "max_physical_bytes",
            "max_domain_physical_bytes",
            "max_domain_logical_bytes",
        ] {
            assert!(
                json.get(field).and_then(serde_json::Value::as_u64).is_some(),
                "missing numeric JSON field {field}"
            );
        }
        assert!(report.raw_disk_bytes >= report.owned_physical_bytes);

        let exit = oven_legacy_cargo_bake_loafs(OvenLoafBakeCommandOptions {
            compiler_root: compiler_root.path().to_path_buf(),
            output: output.path().to_path_buf(),
            suite_store: None,
            envelope: OvenLoafEnvelopeArgument::Release,
            sdk_inventory,
            cargo,
            rustc,
            max_physical_bytes: Some(1024 * 1024),
            max_domain_physical_bytes: Some(1024 * 1024),
            max_domain_logical_bytes: Some(1024 * 1024),
            format: OvenOutputFormat::Json,
        })?;
        assert_eq!(exit, ExitCode::SUCCESS);
        assert!(!cargo_marker.exists(), "exact public-path reuse must not start Cargo");
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
        let sdk_inventory = compiler_root.path().join("sdk-inventory.json");
        fs::write(&sdk_inventory, "not read on exact suite reuse")?;
        let rustc = resolve_active_rustc()?;
        let (receipt, _) = super::compiler_libtests_receipt(compiler_root.path(), &rustc, &["lsp".to_string()])?;
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
            schema_version: 13,
            test_targets: Vec::new(),
            shard_references: vec![OvenCompilerTestSuiteShardReference {
                identity: "sha256:fixture-shard".to_string(),
                target: target.key(),
            }],
            foundation_references: vec![OvenCompilerTestSuiteFoundationReference {
                identity: "sha256:fixture-foundation".to_string(),
                label: "foundation-0000".to_string(),
            }],
            toolchain_data_references: vec![OvenCompilerTestSuiteToolchainDataReference {
                identity: "sha256:fixture-toolchain-data".to_string(),
                label: "toolchain-data-0000".to_string(),
            }],
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
        store.publish(&OvenArtifactPublishRequest {
            receipt,
            domain: "compiler-suite-lsp".to_string(),
            kind: OvenArtifactKind::CompilerTestSuite,
            payload: serde_json::to_vec(&suite)?,
            materialized_files: Vec::new(),
        })?;
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
        let output = tempfile::tempdir()?;
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
                },
                CompilerSuiteNativeTestRootReport {
                    package_name: "unreported".to_string(),
                    target_kind: "test".to_string(),
                    target_name: "unreported_root".to_string(),
                    source_relative_path: "tests/unreported.rs".to_string(),
                    inventory_count: 0,
                    success: false,
                    case_counts: None,
                },
            ],
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
    }

    #[test]
    fn default_store_root_prefers_incan_home() {
        assert_eq!(
            default_store_root(Some(OsString::from("/incan")), Some(OsString::from("/user"))),
            Some(PathBuf::from("/incan/oven/store/v1"))
        );
        assert_eq!(
            default_store_root(None, Some(OsString::from("/user"))),
            Some(PathBuf::from("/user/.incan/oven/store/v1"))
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
            },
            OvenCompilerTestSuiteShardReference {
                identity: "sha256:second".to_string(),
                target: second.key(),
            },
        ];

        assert_eq!(
            compiler_suite_selected_shard_references(&references, &["tests/second.rs".to_string()])?
                .into_iter()
                .map(|reference| reference.identity)
                .collect::<Vec<_>>(),
            vec!["sha256:second"]
        );
        assert_eq!(compiler_suite_selected_shard_references(&references, &[])?, references);
        assert!(compiler_suite_selected_shard_references(&references, &["tests/missing.rs".to_string()]).is_err());
        assert!(compiler_suite_selected_shard_references(&references, &[" ".to_string()]).is_err());
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
        assert_eq!(limits.max_physical_bytes, 3 * 1024 * 1024 * 1024);
        assert_eq!(limits.max_domain_physical_bytes, DEFAULT_OVEN_MAX_DOMAIN_PHYSICAL_BYTES);
        assert_eq!(limits.max_domain_physical_bytes, 1024 * 1024 * 1024);
        assert_eq!(limits.max_domain_logical_bytes, DEFAULT_OVEN_MAX_DOMAIN_LOGICAL_BYTES);
        assert_eq!(limits.max_domain_logical_bytes, 768 * 1024 * 1024);
        assert!(limits.max_domain_physical_bytes <= limits.max_physical_bytes);
        Ok(())
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
        assert_eq!(limits.max_domain_physical_bytes, 3 * 1024 * 1024 * 1024);
        assert_eq!(
            limits.max_domain_logical_bytes,
            DEFAULT_OVEN_COMPILER_SUITE_MAX_DOMAIN_LOGICAL_BYTES
        );
        assert_eq!(limits.max_domain_logical_bytes, 3 * 1024 * 1024 * 1024);
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

    #[test]
    fn compiler_suite_worker_budget_is_hardware_aware_and_bounded() {
        assert_eq!(compiler_suite_auto_parallel_jobs(0), 1);
        assert_eq!(compiler_suite_auto_parallel_jobs(1), 1);
        assert_eq!(compiler_suite_auto_parallel_jobs(2), 2);
        assert_eq!(compiler_suite_auto_parallel_jobs(3), 2);
        assert_eq!(compiler_suite_auto_parallel_jobs(4), 3);
        assert_eq!(compiler_suite_auto_parallel_jobs(7), 3);
        assert_eq!(compiler_suite_auto_parallel_jobs(8), 4);
        assert_eq!(compiler_suite_auto_parallel_jobs(64), 4);
    }

    #[test]
    fn compiler_suite_limits_generated_rust_closure_to_its_consumers() {
        assert!(compiler_suite_target_requires_generated_rust_closure("src/lib.rs"));
        assert!(compiler_suite_target_requires_generated_rust_closure(
            "tests/generated_rust_native_consumer_tests.rs"
        ));
        assert!(compiler_suite_target_requires_generated_rust_closure(
            "tests/integration_tests.rs"
        ));
        assert!(!compiler_suite_target_requires_generated_rust_closure(
            "tests/toolchain_installer_tests.rs"
        ));

        let mut environment = BTreeMap::from([
            ("INCAN_OVEN_COMPILER_SUITE_RUSTC".to_string(), "rustc".to_string()),
            ("INCAN_OVEN_COMPILER_SUITE_STDLIB".to_string(), "stdlib".to_string()),
            (
                "INCAN_OVEN_COMPILER_SUITE_VOCAB_EXTERN_0_PATH".to_string(),
                "vocab-extern".to_string(),
            ),
            ("INCAN_INTERNAL_OVEN_LOAF_EXECUTION".to_string(), "1".to_string()),
        ]);

        compiler_suite_remove_generated_rust_closure(&mut environment);

        assert_eq!(
            environment.get("INCAN_OVEN_COMPILER_SUITE_RUSTC"),
            Some(&"rustc".to_string())
        );
        assert!(!environment.contains_key("INCAN_OVEN_COMPILER_SUITE_STDLIB"));
        assert!(!environment.contains_key("INCAN_OVEN_COMPILER_SUITE_VOCAB_EXTERN_0_PATH"));
        assert_eq!(
            environment.get("INCAN_INTERNAL_OVEN_LOAF_EXECUTION"),
            Some(&"1".to_string())
        );
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
        assert_eq!(
            environment["INCAN_OVEN_COMPILER_SUITE_STDLIB"],
            stdlib.display().to_string()
        );
        assert_eq!(environment["INCAN_OVEN_COMPILER_SUITE_DEPENDENCY_PATH_COUNT"], "2");
        assert_eq!(
            environment["INCAN_OVEN_COMPILER_SUITE_DEPENDENCY_PATH_0"],
            target_dependencies.display().to_string()
        );
        assert_eq!(
            environment["INCAN_OVEN_COMPILER_SUITE_DEPENDENCY_PATH_1"],
            host_dependencies.display().to_string()
        );
        assert_eq!(environment["INCAN_OVEN_COMPILER_SUITE_EXTERN_COUNT"], "3");
        assert_eq!(environment["INCAN_OVEN_COMPILER_SUITE_EXTERN_0_NAME"], "incan_stdlib");
        assert_eq!(
            environment["INCAN_OVEN_COMPILER_SUITE_EXTERN_0_PATH"],
            stdlib.display().to_string()
        );
        assert_eq!(
            environment["INCAN_OVEN_COMPILER_SUITE_EXTERN_1_NAME"],
            "incan_stdlib_core"
        );
        assert_eq!(
            environment["INCAN_OVEN_COMPILER_SUITE_EXTERN_1_PATH"],
            stdlib_core.display().to_string()
        );
        assert_eq!(environment["INCAN_OVEN_COMPILER_SUITE_EXTERN_2_NAME"], "incan_derive");
        assert_eq!(
            environment["INCAN_OVEN_COMPILER_SUITE_EXTERN_2_PATH"],
            derive.display().to_string()
        );
        assert_eq!(
            environment["INCAN_OVEN_COMPILER_SUITE_VOCAB_RUSTC"],
            rustc.display().to_string()
        );
        assert_eq!(
            environment["INCAN_OVEN_COMPILER_SUITE_VOCAB_DEPENDENCY_PATH_COUNT"],
            "2"
        );
        assert_eq!(environment["INCAN_OVEN_COMPILER_SUITE_VOCAB_EXTERN_COUNT"], "3");
        assert_eq!(
            environment["INCAN_OVEN_COMPILER_SUITE_VOCAB_EXTERN_0_NAME"],
            "incan_stdlib"
        );
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
        assert!(
            generated_target.starts_with(output.path()),
            "the generated target must remain within caller-owned child output: {}",
            generated_target.display()
        );
        assert_eq!(
            generated_target.file_name().and_then(|name| name.to_str()),
            Some("generated-cargo-target"),
            "the isolated generated target should be distinguishable from the shared harness target"
        );
        let report = run_prepared_compiler_suite_children(vec![prepared_child], &receipt, &rustc)?;

        assert_eq!(report.native_test_count, 1);
        assert_eq!(report.doctest_targets, 0);
        assert!(
            report.failed.is_empty(),
            "stored-suite child failures: {:?}",
            report.failed
        );
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
