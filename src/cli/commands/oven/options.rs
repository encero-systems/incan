//! Option shapes for the `incan oven` commands.
//!
//! One struct per command surface, carrying exactly what the CLI parsed. They hold no behavior beyond the small
//! accessors a caller needs, and are grouped here so the command implementations read as implementations.

use std::ffi::OsString;
use std::path::PathBuf;

use serde::Serialize;

use crate::cli::{OvenInteropAdapterArgument, OvenLoafEnvelopeArgument, OvenOutputFormat};
use crate::oven::OvenBuildIntent;

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

impl OvenStoreCommandOptions {
    /// Whether a command will resolve the ordinary compiler-owned Oven store without caller-specific policy.
    pub(super) fn is_ordinary_default(&self) -> bool {
        self.root.is_none()
            && self.max_physical_bytes.is_none()
            && self.max_domain_physical_bytes.is_none()
            && self.max_domain_logical_bytes.is_none()
    }
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

/// Inputs for the explicitly named `legacy_cargo` publisher boundary.
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

/// Inputs for `incan oven interop bake`.
#[derive(Debug, Clone)]
pub struct OvenInteropBakeCommandOptions {
    /// Package root containing the canonical manifest, lock, and package-owned interop inputs.
    pub project: PathBuf,
    /// Exact locked target triple to select and bake.
    pub target: String,
    /// Runtime-only receipt that selects the existing sealed direct-rustc Loaf plan.
    ///
    /// When omitted, Oven prepares an exact debug Rust-only base for a conventional executable before it selects and
    /// seals the declared native inputs. The bootstrap cannot emit a caller-visible binary and never discovers a
    /// native toolchain outside this command.
    pub base_receipt: Option<PathBuf>,
    /// Explicit selected C compiler for a declared toolchain requirement or C shim.
    pub c_compiler: Option<PathBuf>,
    /// Explicit selected C++ compiler for a declared C++ shim.
    pub cxx_compiler: Option<PathBuf>,
    /// Explicit selected static archiver for declared C/C++ shims.
    pub archiver: Option<PathBuf>,
    /// Semantic version of the selected compiler capability.
    pub toolchain_version: Option<String>,
    /// Explicit selected SDK root when the locked target requires an SDK capability.
    pub sdk_root: Option<PathBuf>,
    /// Semantic version of the selected SDK capability.
    pub sdk_version: Option<String>,
    /// Regular selected SDK identity file below `sdk_root`.
    pub sdk_identity_file: Option<PathBuf>,
    /// Bounded store selection and policy.
    pub store: OvenStoreCommandOptions,
    /// Requested rendering format.
    pub format: OvenOutputFormat,
}

/// Inputs for `incan oven interop stage`.
#[derive(Debug, Clone)]
pub struct OvenInteropStageCommandOptions {
    /// Package root containing the canonical manifest, current lock, and selected interop receipt.
    pub project: PathBuf,
    /// Exact locked target triple whose final interop plan will be staged.
    pub target: String,
    /// Runtime-only receipt used to reconstruct the immutable final interop plan receipt.
    pub base_receipt: PathBuf,
    /// Fixed native consumer layout to stage without invoking platform build tools.
    pub adapter: OvenInteropAdapterArgument,
    /// New caller-owned output directory. Existing output is deliberately never replaced.
    pub output: PathBuf,
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

/// Inputs for the direct-rustc compiler workspace-test consumer.
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
    /// Exact tests selected from one receipt-bound target for a diagnostic Oven run.
    ///
    /// Exact selection is deliberately narrow: it preserves the stored target's ordinary direct-rustc build,
    /// receipt, environment, working directory, and timeout supervisor while running every requested case
    /// sequentially from one materialized libtest binary.
    pub exact_names: Vec<String>,
    /// Zero-based index of a deterministic receipt-index partition.
    ///
    /// CI uses this only after one independent prewarm has admitted the complete suite. It is a read-only
    /// projection of the receipt-indexed roots, not a second suite definition or a baking capability.
    pub partition_index: Option<usize>,
    /// Number of deterministic receipt-index partitions.
    pub partition_count: Option<usize>,
    /// Explicit Cargo executable for compiler-suite roots that deliberately exercise the Loaf baker.
    ///
    /// The suite creates a logged proxy and grants it only through its package-qualified capability registry. It is
    /// never available to normal Incan commands or used as an Oven execution fallback.
    pub fixture_cargo: Option<PathBuf>,
    /// Caller-owned directory for linked stored test executables.
    pub output: Option<PathBuf>,
    /// Bounded store selection and policy.
    pub store: OvenStoreCommandOptions,
    /// Requested rendering format.
    pub format: OvenOutputFormat,
}

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
