//! Direct `rustc` execution for Oven Alpha's explicitly supported consumer envelope.
//!
//! The executor accepts a verified artifact manifest instead of scanning Cargo output or reproducing Cargo planning.
//! An explicit publisher-side `legacy_cargo` step may create the declared inputs, but this consumer path invokes only
//! the selected Rust compiler and refuses hidden Cargo state.

mod artifact;
mod compiled_unit;
mod diagnostics;
mod inspection;
mod manifest_cohort;
#[cfg(test)]
pub(crate) use manifest_cohort::select_generated_root_registry_externs;
mod manifest_materialize;
mod manifest_source_roles;
// `inspection_toolchain` is deliberately absent. It is the compiler/sysroot-closure-under-lease surface, and the
// only thing here that needs `rust_inspect`'s selected-projection API, which this tree does not have; it lands
// together with that port. Nothing in Gates 6 or 7 of RFC 119 depends on it, so its absence is what lets the four
// runtime modules those gates do use compile on their own.
pub mod direct_compiler;
pub mod native_input;
mod registry_leaf;
mod runtime_closure;
mod runtime_executor;
mod runtime_foundation;
mod selected_unit;
pub mod substitution;
mod toolchain;

// The native-input view moved into a submodule; callers and this module's own tests still name these directly.

// Split along seams this file already had: the artifact/plan shapes, the project-inspection authority payloads,
// and the rustc diagnostic report. Every path stays where callers expect it -- re-exported here rather than
// re-homed -- so this is a move, not an interface change.
pub use artifact::*;
#[allow(
    unused_imports,
    reason = "the native source publisher consumes compiled-unit identities in its next wiring slice"
)]
pub use compiled_unit::*;
pub use diagnostics::*;
pub use direct_compiler::OvenPublisherLinkProduct;
pub use inspection::*;
pub use registry_leaf::*;
#[allow(
    unused_imports,
    reason = "normal-build selection consumes the published runtime closure in the hot-path gate"
)]
pub use runtime_closure::*;
#[allow(
    unused_imports,
    reason = "runtime closure publication consumes the direct-Rustc executor in the next gate"
)]
pub use runtime_executor::*;
#[allow(
    unused_imports,
    reason = "the runtime source publisher loads this sealed authority in its next wiring slice"
)]
pub use runtime_foundation::*;
#[allow(
    unused_imports,
    reason = "the native source publisher consumes selected-unit materialization in its next wiring slice"
)]
pub use selected_unit::*;
pub use toolchain::*;
pub use toolchain::{resolve_active_rustc, rustc_host_target, rustc_identity};

use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use semver::{Version, VersionReq};
use serde::{Deserialize, Serialize};

use crate::native_contract::{
    OVEN_PROJECT_EXTENSION_PAYLOAD_SCHEMA_VERSION, OvenProjectExtensionPayload, OvenProjectRegistrySourceDependency,
};
use oven_model::manifest::{DependencySource, DependencySpec};
use oven_store::closure_proof::{OVEN_CLOSURE_PROOF_SCHEMA_VERSION, OvenClosureProof};
use oven_store::process::{isolate_process_group, terminate_process_group};
use oven_store::store::{
    OvenArtifactKind, OvenArtifactManifest, OvenStore, OvenStoreError, OvenStoreExecutionPayload, OvenStoreLease,
};
use oven_store::{OVEN_COMPILER_TEST_PROFILE, OvenBuildIntent, OvenReceipt, digest_bytes, digest_source_tree};

/// Wire-format version for an Oven-owned direct-rustc artifact manifest.
///
/// Version 10 retains the publisher-selected search closure separately from each source role's direct externs.
/// Version 9 remains readable using its original legacy projection; it never gains inferred role-path evidence.
pub const OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION: u32 = 10;
/// Last manifest version without explicit source-role search closures.
const OVEN_RUSTC_LEGACY_ARTIFACT_MANIFEST_SCHEMA_VERSION: u32 = 9;
/// Fixed supporting-artifact path for the publisher lock that owns sealed registry sources.
pub const OVEN_RUSTC_REGISTRY_LOCK_RELATIVE_PATH: &str = "registry-sources/Cargo.lock";
/// Crate-name prefix reserved for the runtime family owned by one Incan release Loaf.
pub const OVEN_COMPILER_RUNTIME_CRATE_PREFIX: &str = "incan_";
/// Schema version for caller-owned native-output reuse evidence.
const OVEN_DIRECT_RUSTC_OUTPUT_RECEIPT_SCHEMA_VERSION: u32 = 3;

mod compile_environment;
mod diagnostic_parsing;
mod environment;
mod foundation_composition;
mod foundations;
mod inspection_validation;
mod invocation;
mod manifest_validation;
mod path_libraries;
mod plan_building;
mod receipt_selection;

pub use compile_environment::resolve_compile_environment_value;
use compile_environment::*;
use diagnostic_parsing::*;
use environment::*;
pub use environment::{clear_inherited_cargo_environment, clear_inherited_cargo_environment_for_cargo};
pub(crate) use foundation_composition::metadata_sidecar_pair_path;
use foundation_composition::*;
pub use foundation_composition::{OVEN_EXTENSION_REROOT_DIR, rerooted_artifact_staging_source};
use foundations::*;
use inspection_validation::*;
pub use inspection_validation::{
    validate_project_extension_payload_against_base, validate_project_inspection_authority_payload,
};
#[cfg(test)]
pub use invocation::direct_rustc_source_extern_names;
use invocation::*;
pub use invocation::{
    bake_direct_rustc_run, bake_direct_rustc_test, bake_trusted_direct_rustc_dylib, bake_trusted_direct_rustc_library,
    bake_trusted_direct_rustc_library_with_artifact_role, bake_trusted_direct_rustc_proc_macro,
    bake_trusted_direct_rustc_proc_macro_with_artifact_role, bake_trusted_direct_rustc_run,
    bake_trusted_direct_rustc_test, run_trusted_rustdoc_test, trusted_artifact_plan_for_source_evidence,
};
use manifest_validation::*;
#[cfg(test)]
pub use path_libraries::materialize_declared_rust_libraries;
pub use path_libraries::{
    OvenSelectedPathRustcAuthority, incan_owned_cargo, materialize_declared_rust_libraries_with_selected_path_authority,
};
pub use plan_building::attach_caller_owned_rustc_libraries;
use plan_building::*;
pub use receipt_selection::{
    bake_stored_direct_rustc_library, bake_stored_direct_rustc_library_with_libraries, bake_stored_direct_rustc_run,
    bake_stored_direct_rustc_run_with_libraries, bake_stored_direct_rustc_test,
    bake_stored_direct_rustc_test_with_libraries, load_project_inspection_authority,
    project_inspection_authority_supports_dependencies, project_inspection_constituent_matches_receipt,
    project_inspection_test_dependency_envelope_mismatch,
    project_inspection_test_dependency_envelope_supports_dependencies,
    registry_source_dependencies_supported_by_catalog, select_direct_rustc_plan_for_execution,
    select_direct_rustc_plan_identity,
};

/// One actively leased immutable root contributing a named fragment to a composed compiler-suite closure.
///
/// Every fragment is verified at publication and held by the scheduler before compilation starts. Direct Rustc
/// receives its paths directly from these roots, avoiding a copied aggregate directory that would itself evade
/// compatibility-domain policy.
pub struct OvenTrustedRustcArtifactRoot<'a> {
    /// Store-owned root of this selected foundation artifact.
    pub artifact_root: &'a Path,
    /// Direct-rustc closure fragment retained in this root.
    pub dependency_search_paths: &'a [String],
    /// Native-link search paths retained in this root.
    pub native_search_paths: &'a [String],
    /// Every artifact materialized in this root.
    pub supporting_artifacts: &'a [OvenRustcSupportingArtifact],
    /// Complete artifact inventory from this exact leased root, before assigning or deduplicating fragments.
    /// Missing inventory cannot establish source-directory isolation for current-schema roles.
    pub root_inventory: Option<&'a [OvenRustcSupportingArtifact]>,
}

/// Complete search evidence from one already admitted, actively leased root, independent of byte assignment.
///
/// A duplicate-only contributor can provide a clean physical directory without owning another canonical artifact.
/// Callers retain the original selection and lease; these records neither discover nor grant additional artifacts.
pub struct OvenTrustedRustcSearchRoot<'a> {
    /// Original immutable materialized root held by the selection.
    pub artifact_root: &'a Path,
    /// Original publisher-declared dependency search paths, before fragment deduplication.
    pub dependency_search_paths: &'a [String],
    /// Complete admitted inventory of that same root, including excluded co-resident files.
    pub root_inventory: &'a [OvenRustcSupportingArtifact],
}

/// Publisher-owned artifact input that has passed manifest validation and may be copied into Oven storage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OvenRustcMaterializedArtifact {
    /// Portable manifest path preserved beneath the store-owned artifact root.
    pub relative_path: String,
    /// Canonical publisher path whose bytes match the manifest digest at validation time.
    pub source_path: PathBuf,
}

/// Request to compile one receipt-bound test source with direct `rustc`.
#[derive(Debug, Clone)]
pub struct OvenDirectRustcTestRequest {
    /// Verified project/build-unit receipt that authorizes source and intent.
    pub receipt: OvenReceipt,
    /// Immutable artifact manifest selected for the receipt intent.
    pub artifacts: OvenRustcArtifactManifest,
    /// Immutable artifact root containing every path named by `artifacts`.
    pub artifact_root: PathBuf,
    /// Explicit Rust compiler executable; PATH lookup is not used as a hidden selector.
    pub rustc: PathBuf,
    /// Caller-owned generated Rust source file.
    pub source: PathBuf,
    /// Caller-owned final test executable output path.
    pub output: PathBuf,
    /// Rust crate name for the test harness.
    pub crate_name: String,
    /// Rust edition supplied to the compiler.
    pub edition: String,
    /// Receipt supplemental-digest key that authorizes the generated source content.
    pub source_evidence_key: String,
}

/// Request to compile one receipt-bound binary source with direct `rustc`.
#[derive(Debug, Clone)]
pub struct OvenDirectRustcRunRequest {
    /// Verified project/build-unit receipt that authorizes source and intent.
    pub receipt: OvenReceipt,
    /// Immutable artifact manifest selected for the receipt intent.
    pub artifacts: OvenRustcArtifactManifest,
    /// Immutable artifact root containing every path named by `artifacts`.
    pub artifact_root: PathBuf,
    /// Explicit Rust compiler executable; PATH lookup is not used as a hidden selector.
    pub rustc: PathBuf,
    /// Caller-owned generated Rust source file.
    pub source: PathBuf,
    /// Caller-owned final binary output path.
    pub output: PathBuf,
    /// Rust crate name for the binary.
    pub crate_name: String,
    /// Rust edition supplied to the compiler.
    pub edition: String,
    /// Receipt supplemental-digest key that authorizes the generated source content.
    pub source_evidence_key: String,
}

/// Request to compile a test through an immutable direct-rustc closure selected from the bounded Oven store.
pub struct OvenStoredDirectRustcTestRequest<'a> {
    /// Bounded Oven store that retains the selected immutable plan.
    pub store: &'a OvenStore,
    /// Exact store identity of the `direct_rustc_plan` artifact to execute.
    pub plan_identity: String,
    /// Verified project/build-unit receipt that authorizes source and intent.
    pub receipt: OvenReceipt,
    /// Explicit Rust compiler executable; PATH lookup is not used as a hidden selector.
    pub rustc: PathBuf,
    /// Caller-owned generated Rust source file.
    pub source: PathBuf,
    /// Caller-owned final test executable output path.
    pub output: PathBuf,
    /// Rust crate name for the test harness.
    pub crate_name: String,
    /// Rust edition supplied to the compiler.
    pub edition: String,
    /// Receipt supplemental-digest key that authorizes the generated source content.
    pub source_evidence_key: String,
}

/// Request to compile a binary through an immutable direct-rustc closure selected from the bounded Oven store.
pub struct OvenStoredDirectRustcRunRequest<'a> {
    /// Bounded Oven store that retains the selected immutable plan.
    pub store: &'a OvenStore,
    /// Exact store identity of the `direct_rustc_plan` artifact to execute.
    pub plan_identity: String,
    /// Verified project/build-unit receipt that authorizes source and intent.
    pub receipt: OvenReceipt,
    /// Explicit Rust compiler executable; PATH lookup is not used as a hidden selector.
    pub rustc: PathBuf,
    /// Caller-owned generated Rust source file.
    pub source: PathBuf,
    /// Caller-owned final binary output path.
    pub output: PathBuf,
    /// Rust crate name for the binary.
    pub crate_name: String,
    /// Supported Rust edition.
    pub edition: String,
    /// Receipt supplemental-digest key that authorizes the generated source content.
    pub source_evidence_key: String,
}

/// Request to compile a caller-owned Rust library through an immutable direct-rustc closure selected from the
/// bounded Oven store.
///
/// This is the normal-command counterpart to [`OvenStoredDirectRustcRunRequest`]. The generated library source and
/// final `.rlib` remain caller-owned, while the third-party/runtime closure is selected only through the receipt and
/// held under a store lease for the compilation.
pub struct OvenStoredDirectRustcLibraryRequest<'a> {
    /// Bounded Oven store that retains the selected immutable plan.
    pub store: &'a OvenStore,
    /// Exact store identity of the `direct_rustc_plan` artifact to execute.
    pub plan_identity: String,
    /// Verified project/build-unit receipt that authorizes source and intent.
    pub receipt: OvenReceipt,
    /// Explicit Rust compiler executable; PATH lookup is not used as a hidden selector.
    pub rustc: PathBuf,
    /// Caller-owned generated Rust source file.
    pub source: PathBuf,
    /// Caller-owned final Rust library output path.
    pub output: PathBuf,
    /// Rust crate name for the library.
    pub crate_name: String,
    /// Supported Rust edition.
    pub edition: String,
    /// Receipt supplemental-digest key that authorizes the generated source content.
    pub source_evidence_key: String,
}

/// Request to compile one target from an already selected, actively leased Oven suite artifact.
///
/// Compiler-suite execution holds the suite lease for the complete inventory and run, but its payload is not a
/// standalone `direct_rustc_plan` store entry. This narrow internal request reuses the trusted-store materialization
/// path without granting callers the ability to select an unleased artifact root.
pub struct OvenTrustedDirectRustcTargetRequest<'a> {
    /// Exact receipt that authorizes the workspace target source.
    pub receipt: &'a OvenReceipt,
    /// Target-specific direct-rustc closure declared by the immutable suite payload.
    pub artifacts: &'a OvenRustcArtifactManifest,
    /// Materialized root returned alongside the active suite lease.
    pub artifact_root: &'a Path,
    /// Optional plan composed from several separately leased compiler foundations.
    ///
    /// When absent, this request materializes the one legacy schema-8/9 artifact root. Schema 10 supplies an
    /// already validated composed plan and never asks this runner to rebuild a directory tree.
    pub artifact_plan: Option<&'a OvenRustcArtifactPlan>,
    /// Explicit compiler selected by the receipt.
    pub rustc: &'a Path,
    /// Caller-owned workspace test root.
    pub source: &'a Path,
    /// Caller-owned transient native test executable.
    pub output: &'a Path,
    /// Rust crate identifier for this test root.
    pub crate_name: &'a str,
    /// Rust edition declared by the target inventory.
    pub edition: &'a str,
    /// Receipt source-evidence key for this exact target root.
    pub source_evidence_key: &'a str,
    /// Resolved target feature set from the publisher's unit graph.
    pub features: &'a [String],
    /// Whether Cargo's publisher compiled this libtest root with `-C prefer-dynamic`.
    pub prefer_dynamic: bool,
}

/// Request to run one receipt-bound Rustdoc doctest root from an actively leased compiler-suite artifact.
///
/// Rustdoc owns the ephemeral doctest binaries, so Oven records the caller-owned temporary directory rather than
/// pretending there is a stable native executable to store or re-run.
pub struct OvenTrustedRustdocTestRequest<'a> {
    /// Exact receipt that authorizes the workspace target source.
    pub receipt: &'a OvenReceipt,
    /// Target-specific artifact closure declared by the immutable suite payload.
    pub artifacts: &'a OvenRustcArtifactManifest,
    /// Materialized root returned alongside the active suite lease.
    pub artifact_root: &'a Path,
    /// Optional plan composed from several separately leased compiler foundations.
    pub artifact_plan: Option<&'a OvenRustcArtifactPlan>,
    /// Explicit compiler selected by the receipt; its sibling Rustdoc is derived from the same sysroot.
    pub rustc: &'a Path,
    /// Caller-owned workspace Rustdoc source root.
    pub source: &'a Path,
    /// Caller-owned temporary directory for Rustdoc's ephemeral doctest binaries.
    pub temporary_directory: &'a Path,
    /// Rust crate identifier for this doctest root.
    pub crate_name: &'a str,
    /// Rust edition declared by the target inventory.
    pub edition: &'a str,
    /// Receipt source-evidence key for this exact source root.
    pub source_evidence_key: &'a str,
    /// Resolved target feature set from the publisher's unit graph.
    pub features: &'a [String],
    /// Whether Rustdoc must compile this root as a procedural-macro crate.
    pub is_proc_macro: bool,
    /// Whether the doctest root requires the selected toolchain dynamic library environment.
    pub prefer_dynamic: bool,
    /// Maximum wall-clock duration for the Rustdoc root and its generated doctest descendants.
    pub timeout: Option<Duration>,
}

/// Successful direct Rustdoc doctest execution transcript.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OvenRustdocTestReport {
    /// Combined Rustdoc/doctest output for caller failure reporting.
    pub output: String,
}

/// Successful direct-rustc consumer compilation evidence.
pub struct OvenDirectRustcBake {
    /// Source digest that was matched to receipt evidence before compiler invocation.
    pub source_digest: String,
    /// Final caller-owned test executable path.
    pub output: PathBuf,
    /// Digest of the regular caller-owned output, established before it is exposed to another direct-Rustc step.
    pub output_digest: String,
    /// Whether a Cargo process performed this compile. Every direct-rustc constructor sets `false`; the flag exists
    /// so reports and consumers can assert the route rather than infer it from the command.
    pub cargo_process_started: bool,
    /// Whether the receipt- and plan-verified caller-owned native output was reused without launching rustc.
    pub reused: bool,
    /// Held for stored consumers until their caller finishes executing the native test binary.
    lease: Option<OvenStoreLease>,
}

/// The concrete caller-owned output Rustc must produce for one Oven materialization step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))]
enum OvenDirectRustcOutputKind {
    Binary,
    Library,
    Dylib,
    ProcMacro,
}

impl OvenDirectRustcOutputKind {
    /// Return the stable receipt spelling for this caller-owned output kind.
    const fn receipt_value(self) -> &'static str {
        match self {
            Self::Binary => "binary",
            Self::Library => "library",
            Self::Dylib => "dylib",
            Self::ProcMacro => "proc-macro",
        }
    }
}

/// Return the receipt default for callers that predate an explicit output-kind field.
fn default_direct_rustc_output_kind() -> String {
    OvenDirectRustcOutputKind::Binary.receipt_value().to_string()
}

/// Complete input binding retained beside a caller-owned native output.
///
/// This is not Oven store state and never authorizes execution by itself: the caller still verifies the receipt,
/// compiler identity, source digest, and immutable plan before matching these inputs and the persisted output digest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct OvenDirectRustcOutputReceipt {
    schema_version: u32,
    receipt_identity: String,
    artifact_manifest_digest: String,
    source_digest: String,
    crate_name: String,
    edition: String,
    #[serde(default)]
    features: Vec<String>,
    test_harness: bool,
    prefer_dynamic: bool,
    #[serde(default = "default_direct_rustc_output_kind")]
    output_kind: String,
    #[serde(default)]
    caller_owned_library_digests: BTreeMap<String, String>,
}

/// Successful compilation evidence binding the complete verified inputs to the exact produced output bytes.
///
/// Flattening preserves the sidecar's input field names. The schema bump and required digest ensure readers cannot
/// reuse older evidence that never bound the output; a malformed or interrupted record is a cache miss.
#[derive(Debug, Serialize, Deserialize)]
struct OvenDirectRustcOutputRecord {
    #[serde(flatten)]
    inputs: OvenDirectRustcOutputReceipt,
    output_digest: String,
}

/// Backwards-compatible name for a direct-rustc libtest bake.
pub type OvenDirectRustcTestBake = OvenDirectRustcBake;

/// Errors while verifying direct-rustc inputs or compiling an Oven consumer.
#[derive(Debug, thiserror::Error)]
pub enum OvenRustcError {
    /// Artifact manifest schema differs from this executable's supported wire format.
    #[error("unsupported Oven Rust artifact manifest schema version {found}; expected {expected}")]
    UnsupportedSchema { found: u32, expected: u32 },
    /// Artifact plan intent differs from the selected Oven receipt.
    #[error("Oven direct-rustc artifact intent differs from the selected receipt")]
    IntentMismatch,
    /// A project inspection authority uses a wire schema this executable cannot interpret.
    #[error("unsupported Oven project inspection authority schema version {found}; expected {expected}")]
    UnsupportedProjectInspectionAuthoritySchema { found: u32, expected: u32 },
    /// A Rust inspection toolchain owner uses a wire schema this executable cannot interpret.
    #[error("unsupported Oven Rust inspection toolchain schema version {found}; expected {expected}")]
    UnsupportedRustInspectionToolchainSchema { found: u32, expected: u32 },
    /// A request field is blank or does not obey the narrow Alpha spelling contract.
    #[error("invalid Oven direct-rustc {field}: {message}")]
    InvalidInput { field: &'static str, message: String },
    /// A publisher probe selected a compiler that does not carry the auxiliary Rust target being verified.
    #[error("Rust target `{target}` is not installed for compiler `{compiler}`:\n{stderr}")]
    TargetNotInstalled {
        target: String,
        compiler: PathBuf,
        stderr: String,
    },
    /// A declared artifact path escapes the immutable artifact root or is not a regular file/directory.
    #[error("invalid Oven direct-rustc {kind} path {path}: {message}")]
    InvalidArtifactPath {
        kind: &'static str,
        path: PathBuf,
        message: String,
    },
    /// A declared artifact did not retain the digest recorded in the immutable artifact plan.
    #[error("Oven direct-rustc artifact digest mismatch at {path}: expected {expected}, got {actual}")]
    ArtifactDigestMismatch {
        path: PathBuf,
        expected: String,
        actual: String,
    },
    /// A declared search directory contains an unrecorded or unsupported entry.
    #[error("Oven direct-rustc search directory has unrecorded input {path}")]
    UnrecordedSearchArtifact { path: PathBuf },
    /// Source content differs from the receipt-bound supplemental source evidence.
    #[error("Oven direct-rustc source evidence mismatch for `{key}`: expected {expected}, got {actual}")]
    SourceEvidenceMismatch {
        key: String,
        expected: String,
        actual: String,
    },
    /// The selected compiler did not report the exact identity the prebuilt libraries were sealed against.
    ///
    /// Users reach this by building with a different Rust compiler than the one that produced the libraries their
    /// installation ships, so the message names both compilers and the way out rather than the internal reason.
    #[error(
        "Rust compiler mismatch: these prebuilt libraries were built with `{expected}`, but the Rust compiler in \
         use is `{actual}`. Compiled Rust libraries only load under the exact compiler that built them. Reinstall \
         Incan so it provisions its own matching Rust toolchain, or set RUSTC to a `{expected}` compiler."
    )]
    ToolchainMismatch { expected: String, actual: String },
    /// Reading or writing a direct-rustc input/output path failed.
    #[error("Oven direct-rustc I/O failed at {path}: {source}")]
    Io { path: PathBuf, source: io::Error },
    /// Rustc returned a non-success status with structured diagnostic evidence.
    #[error("Oven direct-rustc compilation failed:\n{report}")]
    CompilationFailed { report: OvenRustcDiagnosticReport },
    /// Receipt-bound Rustdoc returned a non-success status while executing doctests.
    #[error("Oven direct Rustdoc doctest failed:\n{output}")]
    RustdocTestFailed { output: String },
    /// A bounded Oven store failed to select the immutable direct-rustc plan.
    #[error("Oven direct-rustc plan store failure: {0}")]
    Store(#[from] OvenStoreError),
    /// A selected store entry is not the receipt-bound direct-rustc plan expected by this executor.
    #[error("invalid Oven direct-rustc stored plan `{identity}`: {message}")]
    InvalidStoredPlan { identity: String, message: String },
    /// The bounded store does not retain a unique direct-rustc plan for the requested receipt.
    #[error("Oven direct-rustc selection failed for receipt `{receipt_identity}`: {message}")]
    PlanSelection { receipt_identity: String, message: String },
}

#[cfg(test)]
mod tests;
