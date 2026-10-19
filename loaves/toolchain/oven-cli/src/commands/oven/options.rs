//! Option shapes for the `incan oven` commands.
//!
//! One struct per command surface, carrying exactly what the CLI parsed. They hold no behavior beyond the small
//! accessors a caller needs, and are grouped here so the command implementations read as implementations.

use std::ffi::OsString;
use std::path::PathBuf;

pub use incan_driver::oven_store::OvenStoreCommandOptions;

use serde::Serialize;

use crate::{OvenInteropAdapterArgument, OvenLoafEnvelopeArgument, OvenOutputFormat};
use oven_model::manifest::RustBinaryRole;
use oven_store::OvenBuildIntent;

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
    /// Named compiler, runtime, dependency, and provider inputs that compose the build-unit identity. These values are
    /// portable identity evidence, never project-local source paths.
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
