//! Building a project: the options, prepared-project shapes, packaged-Loaf records and output payloads every
//! stage of `incan build`, `run`, `test` and the explicit Oven bake exchange.
//!
//! The work is in the submodules; this module owns the types they pass between them, and the few small helpers
//! (timings, report identity blocks, bake target identities) every one of them uses, so the same prepared project
//! flows from selection through bake to the report the CLI renders.

pub mod backend_selection;
pub mod bake;
pub mod caller_owned;
pub mod inline_command;
pub mod library_exports;
pub mod library_outputs;
pub mod library_project;
pub mod library_publication;
pub mod output_materialization;
pub mod output_paths;
pub mod output_selection;
pub mod oven_project;
pub mod package_loafs;
pub mod plan_authority;
pub mod plan_selection;
#[cfg(test)]
pub mod prepare_project;
pub mod provider_compilation;
pub mod provider_metadata;
pub mod publication;
pub mod replacement;
pub mod reuse;
pub mod rust_extern;
pub mod source_authority;
#[cfg(test)]
pub(crate) mod test_support;

use std::collections::{BTreeMap, HashMap};
use std::env;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::backend::ProjectGenerator;
use crate::backend::project::generator::GENERATED_CARGO_TARGET_DIR_ENV;
use crate::backend::selection::{BackendExecutionReceipt, BackendKind, FallbackPolicy};
use crate::driver::build::output_paths::{project_relative_entrypoint, validated_project_output_relative_path};
use crate::driver::build_report::{BuildReportDraft, BuildReportProject, SourceFileReport};
use crate::driver::cargo_policy::CargoPolicy;
use crate::driver::error::{CliError, CliResult};
use crate::frontend::library_manifest_index::{LibraryArtifactMetadata, LibraryManifestIndex};
use crate::frontend::{ParsedModule, typechecker};
use crate::library_manifest::LibraryManifest;
use crate::lockfile::CargoFeatureSelection;
use crate::manifest::{DependencySpec, ProjectManifest};
use crate::oven::digest_bytes;
use crate::oven::legacy_cargo::OvenCompilerMacroDependency;
use crate::oven::plan::{OvenDirectRustcPlanSelection, OvenPackagedLibraryLoafEntry, PackagedProviderCandidate};
use crate::oven::rustc::{OvenCallerOwnedRustcLibrary, OvenProjectInspectionAuthorityRef, OvenRustcArtifactManifest};
use crate::oven::store::{OvenArtifactKind, OvenStoreLease};
use crate::provider::{FeatureSelection, PackageFeaturePlan, ProviderPlan};

pub(crate) const INLINE_COMMAND_PROJECT_PREFIX: &str = "incan_inline_command";

pub(crate) const INLINE_COMMAND_OUTPUT_PARENT: &str = "target/incan/inline";

/// Stable package-artifact location for immutable provider Loafs.
pub(crate) const OVEN_PACKAGED_LIBRARY_LOAF_STORE_RELATIVE_PATH: &str = "oven/loafs";

/// Current wire schema for package-owned Oven Loaf handoff metadata.
///
/// Version 6 seals the checked `.incnlib` manifest and every manifest-declared provider sidecar by relative path and
/// digest, in addition to requiring release-cohort project-extension entries.
pub(crate) const OVEN_PACKAGED_LIBRARY_LOAF_SCHEMA_VERSION: u32 = 6;

/// Current wire schema for completed receipt-bound project-output Loafs.
///
/// Version 13 carries the verified default backend execution receipt alongside every completed output. This lets a
/// source-current cache hit retain the same compiler-backend provenance as the explicit bake without trusting a stale
/// report snapshot.
pub(crate) const OVEN_PROJECT_OUTPUT_PAYLOAD_SCHEMA_VERSION: u32 = 13;

pub(crate) const OVEN_PROJECT_OUTPUT_PROJECTION_SCHEMA_VERSION: u32 = 3;

pub(crate) const OVEN_PROJECT_OUTPUT_REPORT_SCHEMA_VERSION: u32 = 2;

pub(crate) const OVEN_PROJECT_OUTPUT_REPORT_PATH_TAG: &str = "$incan_portable_path";

pub(crate) const OVEN_PROJECT_OUTPUT_REPORT_EXTERNAL_ROOT: &str = "$INCAN_EXTERNAL_AUTHORITY";

/// Maximum time an explicit project bake waits for another explicit publisher's bounded staging transaction.
pub(crate) const OVEN_PROJECT_OUTPUT_PUBLICATION_WAIT: Duration = Duration::from_secs(5 * 60);

/// Short cooperative backoff while the named publisher retains the only safe staging reservation.
pub(crate) const OVEN_PROJECT_OUTPUT_PUBLICATION_RETRY: Duration = Duration::from_millis(100);

/// Portable path below one project-output Loaf's immutable artifact root.
pub(crate) const OVEN_PROJECT_OUTPUT_ARTIFACT_PATH: &str = "output/native";

/// Prepared source and immutable build-unit selection for the normal Oven Alpha executable path.
///
/// This deliberately contains no Cargo target path or command. Generated Rust and the final binary are caller-owned;
/// the selected native closure retains its store lease until the direct-Rustc bake and any child execution complete.
pub(crate) struct OvenPreparedProject {
    pub(crate) generator: ProjectGenerator,
    pub(crate) project_root: PathBuf,
    pub(crate) entrypoint: PathBuf,
    pub(crate) provider_plan: Arc<ProviderPlan>,
    pub(crate) receipt: crate::oven::OvenReceipt,
    pub(crate) plan_selection: OvenDirectRustcPlanSelection,
    pub(crate) materialization: OvenToolchainMaterialization,
    pub(crate) cargo_process_started: bool,
    pub(crate) rustc: PathBuf,
    pub(crate) crate_name: String,
    pub(crate) rust_edition: String,
    pub(crate) caller_owned_libraries: Vec<OvenCallerOwnedRustcLibrary>,
    pub(crate) report: BuildReportDraft,
    /// Per-phase laps of the prepare step, merged into the report's `timings_ms`.
    pub(crate) prepare_timings: BTreeMap<String, u64>,
    #[cfg(feature = "rust_inspect")]
    pub(crate) rust_inspect_manifest_dir: Option<PathBuf>,
}

/// CLI-facing backend-selection request for one build (`--backend`, `--backend-fallback`, `--shadow`).
///
/// Bridges those flags to [`select_backend`]. The default (no flags given) declares the legacy
/// backend explicitly with [`FallbackPolicy::Refuse`] — matching the "declared legacy capability
/// selection" behavior #986 requires even when nothing was explicitly requested, rather than
/// leaving the default path unrecorded.
#[derive(Debug, Clone)]
pub struct BackendSelectionOptions {
    /// Backend requested for this build.
    pub requested: BackendKind,
    /// Whether `requested` came from an explicit `--backend` flag rather than the default.
    pub explicit: bool,
    /// Whether `--shadow` was given, requesting a comparison against the replacement backend.
    pub shadow: bool,
    /// What to do if `requested` cannot execute.
    pub fallback_policy: FallbackPolicy,
}

impl Default for BackendSelectionOptions {
    fn default() -> Self {
        Self {
            requested: BackendKind::Legacy,
            explicit: false,
            shadow: false,
            fallback_policy: FallbackPolicy::Refuse,
        }
    }
}

impl BackendSelectionOptions {
    /// Whether this request can reuse a completed project output without changing its recorded backend provenance.
    ///
    /// An explicit backend, fallback policy, or shadow comparison is a new declared selection and must take the
    /// source-aware preparation path so it can be recorded against the current invocation. Only the implicit legacy
    /// default can reuse the verified default receipt sealed into a completed output.
    pub(crate) fn allows_completed_output_reuse(&self) -> bool {
        self.requested == BackendKind::Legacy
            && !self.explicit
            && !self.shadow
            && self.fallback_policy == FallbackPolicy::Refuse
    }
}

#[derive(Debug, Clone, Default)]
pub struct BuildCommandOptions {
    pub cargo_policy: CargoPolicy,
    pub package_features: FeatureSelection,
    pub sdk_profile: Option<String>,
    pub cargo_features: Vec<String>,
    pub cargo_no_default_features: bool,
    pub cargo_all_features: bool,
    pub generated_cargo_target_dir: Option<PathBuf>,
    pub backend: BackendSelectionOptions,
}

impl BuildCommandOptions {
    /// Return the retired generated-Cargo target override for the explicit publisher boundary only.
    pub(crate) fn effective_generated_cargo_target_dir(&self) -> Option<PathBuf> {
        self.generated_cargo_target_dir.clone().or_else(|| {
            env::var_os(GENERATED_CARGO_TARGET_DIR_ENV)
                .filter(|raw| !raw.is_empty())
                .map(PathBuf::from)
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InlineCommandProject {
    pub(crate) source_path: PathBuf,
    pub(crate) project_name: String,
    pub(crate) output_dir: String,
}

/// A prepared library project after Incan validation and Rust source generation.
///
/// The legacy publisher may still retain Cargo-only preparation state. Normal `incan build --lib`, however, always
/// carries an Oven selection and compiles through direct `rustc` without entering that state.
pub(crate) struct PreparedLibraryProject {
    pub(crate) generator: ProjectGenerator,
    pub(crate) project_root: PathBuf,
    pub(crate) entrypoint: PathBuf,
    pub(crate) out_dir: PathBuf,
    pub(crate) manifest_path: PathBuf,
    pub(crate) library_manifest: LibraryManifest,
    /// One identity-addressed public executable closure selected from the finalized manifest and the same checked
    /// compilation.
    pub(crate) executable_surface: Vec<u8>,
    pub(crate) timings_ms: BTreeMap<String, u64>,
    pub(crate) report: BuildReportDraft,
    pub(crate) oven: Option<OvenPreparedLibrary>,
    #[cfg(feature = "rust_inspect")]
    pub(crate) rust_inspect_manifest_dir: Option<PathBuf>,
}

/// Receipt-selected direct-rustc materialization state for a normal Oven library build.
pub(crate) struct OvenPreparedLibrary {
    pub(crate) rustc: PathBuf,
    pub(crate) crate_name: String,
    pub(crate) rust_edition: String,
    pub(crate) profiles: BTreeMap<String, OvenPreparedLibraryProfile>,
}

/// One profile-specific direct-rustc library selection.
///
/// A normal library build publishes both debug and release caller-owned outputs. `incan run` defaults to debug while
/// `incan build --lib` has historically produced a release artifact; retaining both avoids linking a library against
/// a different profile's hashed Rust dependencies and never delegates that mismatch to Cargo.
pub(crate) struct OvenPreparedLibraryProfile {
    pub(crate) receipt: crate::oven::OvenReceipt,
    pub(crate) plan_selection: OvenDirectRustcPlanSelection,
    pub(crate) materialization: OvenToolchainMaterialization,
    pub(crate) provider_plan: Arc<ProviderPlan>,
    pub(crate) caller_owned_libraries: Vec<OvenCallerOwnedRustcLibrary>,
}

/// Observable outcome of acquiring one receipt-compatible Oven closure.
///
/// `ToolchainLoaf` means the complete release-version standard-library closure was selected directly from the active
/// immutable toolchain generation. It is intentionally not copied into every project store.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OvenToolchainMaterialization {
    Reused,
    ToolchainLoaf,
    CompatibilityBaked,
}

impl OvenToolchainMaterialization {
    /// Return the stable report spelling for this caller-visible selection outcome.
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Reused => "reused",
            Self::ToolchainLoaf => "toolchain_loaf",
            Self::CompatibilityBaked => "baked",
        }
    }
}

/// Decide whether preparation may cross the explicit project-bake boundary.
///
/// Normal commands consume an already selected closure and fail with actionable
/// guidance on a miss. Only `incan oven bake --project` may invoke the bounded
/// compatibility baker, so normal build, run, and test never regain a Cargo
/// fallback.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OvenProjectPlanMode {
    ConsumeOnly,
    ExplicitBake,
    /// Explicitly prepare the Rust-only base plan for one later native interop bake.
    ///
    /// The interop baker is a separate authority because it selects native toolchain facts and seals package-owned
    /// native artifacts. This mode may publish the pre-interop Rust closure, but never emits a caller-visible
    /// executable or pretends the package already has a final interop plan.
    InteropBootstrap,
}

impl OvenProjectPlanMode {
    /// Return whether this caller may invoke Oven's named compatibility publisher.
    pub(crate) const fn is_explicit_publisher(self) -> bool {
        matches!(self, Self::ExplicitBake | Self::InteropBootstrap)
    }
}

/// Receipt and sealed-plan evidence emitted by explicit `incan oven bake` for one project target/profile.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct OvenProjectBakeProfileReport {
    /// Project target whose generated Rust source this receipt authorizes.
    pub project_target: String,
    /// Profile whose source closure and intent were recorded.
    pub profile: String,
    /// Target selected from the active Rust toolchain.
    pub target: String,
    /// Exact Rust toolchain identity recorded by the receipt.
    pub toolchain: String,
    /// Project-local receipt that binds source, lock, SDK, provider, and package-closure evidence.
    pub receipt: PathBuf,
    /// Complete content identity of that receipt.
    pub receipt_identity: String,
    /// Reusable compiler/runtime/provider/dependency compatibility identity.
    pub build_unit_identity: String,
    /// Immutable direct-rustc plan or compiler-shipped Loaf selected for this profile.
    pub plan_identity: String,
    /// Whether this invocation reused, materialized, or explicitly baked the compatible closure.
    pub action: &'static str,
}

/// Evidence emitted by explicit `incan oven bake` for one Incan project.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct OvenProjectBakeReport {
    /// Project root whose checked sources supplied the receipt evidence.
    pub project: PathBuf,
    /// Caller-visible generated Rust keyed by `library` or `executable`; exact replay copies are retained immutably in
    /// the bounded project-output Loafs.
    pub generated_sources: BTreeMap<String, PathBuf>,
    /// Bounded local store that retains project-specific Loafs when this project needs one.
    pub store: PathBuf,
    /// One receipt and selection outcome for each discovered project target/profile.
    pub profiles: Vec<OvenProjectBakeProfileReport>,
}

/// Immutable package-owned Loaf evidence written beside a baked public library.
///
/// A public provider's direct library output is meaningful only together with the sealed Rust closure it was
/// compiled against. Keeping this compact index below the provider artifact lets a fresh consumer import and select
/// that exact closure without resolving or recompiling the provider's third-party dependencies.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct OvenPackagedLibraryLoafManifest {
    pub(crate) schema_version: u32,
    /// Complete authored root and transitive path-dependency authority that produced these package Loafs.
    pub(crate) source_authority_digest: String,
    /// Incan release that generated the sealed provider output.
    ///
    /// A release version is the compiler compatibility boundary for package Loafs. Hashing the complete compiler
    /// executable again on each warm explicit bake would make a valid reuse reread hundreds of megabytes despite
    /// the receipt already recording this release fact.
    pub(crate) compiler_version: String,
    /// Caller-visible checked metadata and manifest-declared sidecars authorized by this package handoff.
    ///
    /// Consumers read `.incnlib` before linking the native output, so sealing only the rlib would permit mutable
    /// metadata to describe a different API, vocabulary surface, or desugarer than the explicit provider bake
    /// produced. These records bind that complete public handoff without copying it into each profile.
    pub(crate) metadata_files: Vec<OvenPackagedLibraryMetadataFile>,
    pub(crate) profiles: BTreeMap<String, OvenPackagedLibraryLoafProfile>,
}

/// One caller-visible provider metadata file sealed by a package Loaf handoff.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct OvenPackagedLibraryMetadataFile {
    /// Safe path below the provider's generated artifact root.
    pub(crate) relative_path: String,
    /// Exact content digest published by the explicit provider bake.
    pub(crate) digest: String,
}

/// One profile-specific public-library handoff record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct OvenPackagedLibraryLoafProfile {
    /// Original producer receipt whose source, provider, lock, and toolchain facts authorized the closure.
    pub(crate) receipt: crate::oven::OvenReceipt,
    /// Every immutable entry whose closure the provider library links against.
    ///
    /// A package can depend on several independently baked public providers.  Retaining the entire compatible
    /// collection here keeps the next consumer from silently recompiling one omitted upstream dependency.
    #[serde(default)]
    pub(crate) entries: Vec<OvenPackagedLibraryLoafEntry>,
    /// Provider library path relative to its generated artifact root.
    pub(crate) library_relative_path: String,
    /// Digest of the direct-rustc provider library output.
    pub(crate) library_digest: String,
}

/// One package profile admitted once for the duration of a single consumer preparation.
///
/// This is deliberately command-local rather than a persistent cache: provider source authority and sealed output
/// bytes are checked once, then the same immutable facts flow through import and final selection.
#[derive(Clone)]
pub(crate) struct CheckedPackagedProviderProfile {
    pub(crate) dependency_key: String,
    pub(crate) artifact_root: PathBuf,
    pub(crate) profile: String,
    pub(crate) package: OvenPackagedLibraryLoafProfile,
}

/// Project the checked provider profiles for one build profile into the plan layer's package candidates.
pub(crate) fn packaged_provider_candidates<'a>(
    checked_profiles: &'a [CheckedPackagedProviderProfile],
    profile: &str,
) -> Vec<PackagedProviderCandidate<'a>> {
    checked_profiles
        .iter()
        .filter(|checked| checked.profile == profile)
        .map(|checked| PackagedProviderCandidate {
            dependency_key: &checked.dependency_key,
            receipt: &checked.package.receipt,
            entries: &checked.package.entries,
        })
        .collect()
}

/// One provider index retained only while a single explicit project bake is running.
pub(crate) struct MemoizedPackagedProviderAuthority {
    pub(crate) artifact: LibraryArtifactMetadata,
    pub(crate) manifest_path: PathBuf,
    pub(crate) manifest_digest: String,
    pub(crate) manifest: Arc<OvenPackagedLibraryLoafManifest>,
    pub(crate) source_project_root: Option<PathBuf>,
    pub(crate) source_authority_verified: bool,
    pub(crate) admitted_profiles: BTreeMap<String, (String, String)>,
}

/// Source-authority state shared by every target/profile preparation inside one explicit project bake.
///
/// The context is stack-owned by `bake_oven_project_targets`; it deliberately has no static lifetime, timestamp-based
/// invalidation, or representation outside this command invocation.
#[derive(Default)]
pub(crate) struct OvenProjectBakeAuthorityContext {
    pub(crate) source_digester: ProjectSourceAuthorityDigester,
    pub(crate) providers: HashMap<PathBuf, MemoizedPackagedProviderAuthority>,
    pub(crate) initial_project_source_authority: Option<String>,
}

/// One manifest-backed Incan entrypoint admitted by `incan oven bake`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OvenBakeProjectTarget {
    Library,
    Executable,
}

impl OvenBakeProjectTarget {
    /// Return the stable user-facing target label.
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Library => "library",
            Self::Executable => "executable",
        }
    }

    /// Return the conventional manifest-backed source entrypoint.
    pub(crate) const fn source_relative_path(self) -> &'static str {
        match self {
            Self::Library => "src/lib.incn",
            Self::Executable => "src/main.incn",
        }
    }
}

/// Return the portable identity of one project target without collapsing separately declared executable scripts.
pub(crate) fn oven_bake_project_target_identity(
    project_root: &Path,
    target: OvenBakeProjectTarget,
    entrypoint: &Path,
) -> CliResult<String> {
    let relative = project_relative_entrypoint(project_root, entrypoint).ok_or_else(|| {
        CliError::failure(format!(
            "Oven project target {} escaped its project root {}",
            entrypoint.display(),
            project_root.display()
        ))
    })?;
    let _ = validated_project_output_relative_path(&relative, "project target")?;
    match target {
        OvenBakeProjectTarget::Library if relative == OvenBakeProjectTarget::Library.source_relative_path() => {
            Ok(OvenBakeProjectTarget::Library.as_str().to_string())
        }
        OvenBakeProjectTarget::Library => Err(CliError::failure(format!(
            "Oven library target must use {} rather than {relative}",
            OvenBakeProjectTarget::Library.source_relative_path()
        ))),
        OvenBakeProjectTarget::Executable if relative == OvenBakeProjectTarget::Executable.source_relative_path() => {
            Ok(OvenBakeProjectTarget::Executable.as_str().to_string())
        }
        OvenBakeProjectTarget::Executable => Ok(format!("executable:{relative}")),
    }
}

/// Return a stable source-evidence key that keeps identical executable scripts on distinct receipt lineages.
pub(crate) fn oven_executable_entrypoint_evidence_key(project_root: &Path, entrypoint: &Path) -> CliResult<String> {
    let identity = oven_bake_project_target_identity(project_root, OvenBakeProjectTarget::Executable, entrypoint)?;
    Ok(format!(
        "incan-entrypoint-{}",
        digest_bytes(identity.as_bytes()).trim_start_matches("sha256:")
    ))
}

/// Isolate generated and native files for a non-conventional executable target.
///
/// The conventional main target keeps its established caller-visible path. Every other declared script receives one
/// stable project-local root so a later target cannot overwrite bytes retained by a deferred ProjectOutput
/// publication.
pub(crate) fn oven_bake_executable_output_dir(project_root: &Path, entrypoint: &Path) -> CliResult<Option<PathBuf>> {
    let identity = oven_bake_project_target_identity(project_root, OvenBakeProjectTarget::Executable, entrypoint)?;
    if identity == OvenBakeProjectTarget::Executable.as_str() {
        return Ok(None);
    }
    Ok(Some(
        project_root
            .join("target/incan/oven-targets")
            .join(digest_bytes(identity.as_bytes()).trim_start_matches("sha256:")),
    ))
}

/// Receipt-bound completed native output published only by an explicit project bake.
///
/// A direct-rustc plan is a reusable dependency closure, whereas this payload is the completed executable or library
/// for one exact authored project. Keeping those contracts distinct means a normal command can prove a source match and
/// select the completed output before source collection, typechecking, or code generation. The store manifest binds
/// this payload to the original receipt and retains the materialized native file under an active execution lease.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct OvenProjectOutputPayload {
    pub(crate) schema_version: u32,
    pub(crate) project_target: String,
    /// Stable target identity within the authored project. Conventional library and main targets retain their
    /// historical labels; every other declared executable is keyed by its project-relative entrypoint.
    pub(crate) target_identity: String,
    /// Stable logical owner of this completed output.
    ///
    /// This intentionally excludes authored source and lock evidence: those are the separate `source_authority_digest`
    /// that must match for reuse. The stable owner lets a changed project fail closed at the explicit bake boundary
    /// without putting an absolute worktree path into a portable Loaf.
    pub(crate) project_identity: String,
    pub(crate) source_authority_digest: String,
    /// Derived semantic dependency fingerprint recorded by the canonical lock at bake time.
    ///
    /// The canonical lock projection remains part of `source_authority_digest`, but excludes this one derived field.
    /// A non-strict exact replay can therefore warn and reuse when only the recorded fingerprint was edited, while
    /// strict commands still recompute and reject it before completed-output selection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) lock_dependencies_fingerprint: Option<String>,
    pub(crate) compiler_version: String,
    pub(crate) entrypoint_relative_path: String,
    pub(crate) build_unit_identity: String,
    pub(crate) receipt_identity: String,
    pub(crate) plan_identity: String,
    /// Verified backend selection and execution provenance from the explicit bake that produced this output.
    pub(crate) backend_receipt: BackendExecutionReceipt,
    /// Singular project-level Rust inspection authority selected through this source-current output.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) inspection_authority: Option<OvenProjectInspectionAuthorityRef>,
    /// Every generated source, package handoff record, and final native artifact retained by this completed project
    /// result. The Loaf is therefore a complete project result, not a binary-only cache entry.
    pub(crate) files: Vec<OvenProjectOutputFile>,
    /// Project-specific closure entries associated with this public library result. They remain receipt-addressed in
    /// the primary Oven store when a normal command restores this completed output.
    #[serde(default)]
    pub(crate) required_project_loafs: Vec<OvenPackagedLibraryLoafEntry>,
    /// Caller-relative root of the portable package-owned store produced by an explicit library bake. Executable
    /// results have no package handoff store.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) package_loaf_store_relative_path: Option<String>,
    /// Portable bake-time report facts used by report-capable replay without re-entering the frontend.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) build_report: Option<OvenProjectOutputReportSnapshot>,
}

/// Portable machine-readable report retained by one completed executable output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct OvenProjectOutputReportSnapshot {
    pub(crate) schema_version: u32,
    pub(crate) report: serde_json::Value,
}

/// One immutable file in a receipt-bound completed project result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct OvenProjectOutputFile {
    /// Caller-owned path below the project root where the file is restored.
    pub(crate) caller_relative_path: String,
    /// Immutable path below the completed project-output Loaf.
    pub(crate) output_relative_path: String,
    /// Exact file content retained in the Loaf.
    pub(crate) digest: String,
    /// Exact caller-visible byte length used for a cheap projection-presence check.
    #[serde(default)]
    pub(crate) logical_bytes: u64,
}

/// Caller-owned marker for an already materialized completed project result.
///
/// The immutable store entry has validated every file at publication. The marker binds the exact selected output
/// identity and caller-visible content digests so mutable projections can never become an authority of their own.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct OvenProjectOutputProjection {
    pub(crate) schema_version: u32,
    pub(crate) output_identity: String,
    pub(crate) files: Vec<OvenProjectOutputProjectionFile>,
}

/// One caller-visible file retained by a completed-output projection marker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct OvenProjectOutputProjectionFile {
    pub(crate) caller_relative_path: String,
    pub(crate) digest: String,
    pub(crate) logical_bytes: u64,
}

/// Input file for a project-output Loaf before the publisher computes its sealed relative paths and digests.
#[derive(Clone)]
pub(crate) struct OvenProjectOutputBakeFile {
    pub(crate) source_path: PathBuf,
    pub(crate) caller_relative_path: String,
    pub(crate) output_relative_path: String,
}

/// One completed project-output publication request, grouped so the authority helper stays readable and cannot silently
/// lose an input fact.
pub(crate) struct OvenProjectOutputBakeRequest<'a> {
    pub(crate) project_root: &'a Path,
    pub(crate) entrypoint: &'a Path,
    pub(crate) target: OvenBakeProjectTarget,
    pub(crate) receipt: &'a crate::oven::OvenReceipt,
    pub(crate) plan_identity: String,
    pub(crate) profile: &'a str,
    pub(crate) source_authority_digest: &'a str,
    pub(crate) lock_dependencies_fingerprint: Option<String>,
    pub(crate) files: Vec<OvenProjectOutputBakeFile>,
    pub(crate) inspection_authority: OvenProjectInspectionAuthorityRef,
    pub(crate) required_project_loafs: Vec<OvenPackagedLibraryLoafEntry>,
    pub(crate) package_loaf_store_relative_path: Option<String>,
    pub(crate) backend_receipt: BackendExecutionReceipt,
    pub(crate) build_report: Option<OvenProjectOutputReportSnapshot>,
}

/// A selected completed project output with the store lease held for its use.
pub(crate) struct OvenStoredProjectOutput {
    pub(crate) identity: String,
    pub(crate) profile: String,
    pub(crate) intent: crate::oven::OvenBuildIntent,
    pub(crate) payload: OvenProjectOutputPayload,
    pub(crate) artifact_root: PathBuf,
    pub(crate) native_output: PathBuf,
    pub(crate) _lease: OvenStoreLease,
}

/// Newly published project inspection authority retained until every completed output names it.
pub(crate) struct PublishedProjectInspectionAuthority {
    pub(crate) reference: OvenProjectInspectionAuthorityRef,
    pub(crate) _lease: OvenStoreLease,
}

/// Completed target data retained until one whole-project inspection authority is finalized.
pub(crate) struct PendingOvenProjectOutput {
    pub(crate) entrypoint: PathBuf,
    pub(crate) target: OvenBakeProjectTarget,
    pub(crate) receipt: crate::oven::OvenReceipt,
    pub(crate) plan_identity: String,
    pub(crate) profile: String,
    pub(crate) files: Vec<OvenProjectOutputBakeFile>,
    pub(crate) required_project_loafs: Vec<OvenPackagedLibraryLoafEntry>,
    pub(crate) package_loaf_store_relative_path: Option<String>,
    pub(crate) backend_receipt: BackendExecutionReceipt,
    pub(crate) build_report: Option<OvenProjectOutputReportSnapshot>,
}

/// Exact debug output lineage expected from one discovered target's local explicit-bake receipt.
pub(crate) struct CurrentDebugProjectOutputExpectation {
    pub(crate) target: OvenBakeProjectTarget,
    pub(crate) target_identity: String,
    pub(crate) entrypoint_relative_path: String,
    pub(crate) receipt: crate::oven::OvenReceipt,
}

/// Build the project identity block used by build and generated Rust inspection reports.
pub(crate) fn manifest_project_report(
    manifest: Option<&ProjectManifest>,
    project_name: &str,
    project_root: &Path,
) -> BuildReportProject {
    BuildReportProject {
        name: project_name.to_string(),
        version: manifest.and_then(|manifest| manifest.project.as_ref().and_then(|project| project.version.clone())),
        project_root: project_root.to_string_lossy().to_string(),
    }
}

/// Convert collected Incan modules into source breadcrumbs for machine-readable reports.
pub(crate) fn source_file_report(modules: &[ParsedModule]) -> Vec<SourceFileReport> {
    modules
        .iter()
        .map(|module| SourceFileReport {
            path: module.file_path.to_string_lossy().to_string(),
            module_path: module.path_segments.clone(),
        })
        .collect()
}

/// Return elapsed milliseconds as a bounded `u64` for report payloads.
pub(crate) fn elapsed_ms(start: Instant) -> u64 {
    start.elapsed().as_millis().min(u128::from(u64::MAX)) as u64
}

/// Record one named build phase timing.
pub(crate) fn record_timing(timings: &mut BTreeMap<String, u64>, name: &str, start: Instant) {
    timings.insert(name.to_string(), elapsed_ms(start));
}

/// The complete checked Rust dependency surface used to select one project Loaf.
///
/// Source inspection happens before code generation through its own compiler session. This value instead covers
/// every direct-Rustc root the generated program and its caller-owned public providers may re-materialize.
pub(crate) struct OvenProjectDependencySurface<'a> {
    pub(crate) selection: &'a [DependencySpec],
    pub(crate) provider_compilations: &'a [OvenCompilerMacroDependency],
}

/// Receipt-selected dependency closure prepared once for every generated native-test batch in a project bake.
pub(crate) struct PreparedOvenTestDependencyEnvelope {
    /// Receipt for the immutable non-package constituent selected by the authority.
    pub(crate) receipt: crate::oven::OvenReceipt,
    /// Complete normal/dev/test surface, including roots owned by separately validated package Loafs.
    pub(crate) dependency_surface_digest: String,
    /// Complete dependency records retained for exact per-root authority checks.
    pub(crate) dependencies: Vec<DependencySpec>,
    pub(crate) dependency_root_digests: BTreeMap<String, String>,
    /// Direct-Rustc plan for the non-package delta; public package libraries are attached from their own Loafs.
    pub(crate) plan_selection: OvenDirectRustcPlanSelection,
}

/// Receipt-selected plan plus the explicit local-store decision that produced it.
pub(crate) struct OvenDirectRustcPlanPreparation {
    pub(crate) plan_selection: OvenDirectRustcPlanSelection,
    pub(crate) materialization: OvenToolchainMaterialization,
    pub(crate) cargo_process_started: bool,
}

/// Publish and lease the receipt-bound project inspection authority for the selected execution plan.
/// The library's own receipt-bound direct-rustc plan, named by the project inspection authority as a constituent.
///
/// It is the only sealed artifact that carries the build-script output (`OUT_DIR`) Rust generated while compiling the
/// library's dependencies — prost's `oneof` enums, for one. A test unit inspects the library's dependencies through
/// the authority and never runs Cargo, so without this constituent it could not see those items at all.
pub(crate) struct LibraryInspectionConstituent {
    pub identity: String,
    /// How the store holds the constituent: a self-contained direct-rustc plan, or a project payload that extends
    /// the compiler Loaf named by `base_loaf_identity`. The authority records the same shape, because a consumer
    /// validates every constituent against its sealed kind before trusting it.
    pub artifact_kind: OvenArtifactKind,
    pub base_loaf_identity: Option<String>,
    pub receipt: crate::oven::OvenReceipt,
    pub artifacts: OvenRustcArtifactManifest,
    /// The bake's rust-inspect workspace, whose Cargo bootstrap wrote the build-script output to seal.
    pub rust_inspect_manifest_dir: Option<PathBuf>,
    /// The generated project's selected Cargo target, where the bounded compatibility baker's unified Cargo
    /// invocation wrote its build-script output when the closure was not loadable as independently compiled parts.
    pub cargo_target_dir: Option<PathBuf>,
    /// The generated project directory, beside which the compatibility build records which package version each
    /// executed build script's output belongs to.
    pub generated_project_dir: Option<PathBuf>,
}

/// One build-script output directory the explicit bake can seal for direct inspection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BakeGeneratedOutDir {
    pub(crate) crate_name: String,
    /// Build-unit path below the target's `build/` directory, in whichever layout the bootstrap used.
    pub(crate) unit_relative_path: String,
    pub(crate) out_dir: PathBuf,
    /// Exact package version whose build script wrote the directory, when a package-keyed map named it.
    pub(crate) version: Option<String>,
}

/// Normal-command policy that must be satisfied before a completed project output may be selected.
pub(crate) struct CompletedOutputPolicy<'a> {
    pub(crate) cargo_policy: &'a CargoPolicy,
    pub(crate) package_features: &'a FeatureSelection,
    pub(crate) sdk_profile: Option<&'a str>,
    pub(crate) cargo_features: &'a [String],
    pub(crate) cargo_no_default_features: bool,
    pub(crate) cargo_all_features: bool,
}

impl CompletedOutputPolicy<'_> {
    /// Reject Cargo feature controls before any completed-output lookup can bypass the normal Oven contract.
    pub(crate) fn reject_cargo_feature_controls(&self, command_kind: &str) -> CliResult<()> {
        if self.cargo_no_default_features || self.cargo_all_features || !self.cargo_features.is_empty() {
            return Err(CliError::failure(format!(
                "Oven Alpha normal {command_kind} do not accept Cargo feature controls; use Incan package features instead"
            )));
        }
        Ok(())
    }

    /// Return the normalized Cargo feature evidence used by canonical lock validation.
    pub(crate) fn cargo_feature_selection(&self) -> CargoFeatureSelection {
        CargoFeatureSelection {
            cargo_features: self.cargo_features.to_vec(),
            cargo_no_default_features: self.cargo_no_default_features,
            cargo_all_features: self.cargo_all_features,
        }
        .normalized()
    }
}

/// Borrowed inputs needed to freeze checked provider facts into a library artifact.
///
/// The checked type information stays in the compiler pipeline until this projection. It carries the canonical
/// callable-to-capability facts that provider operation lowering consumes through the selected manifest.
pub(crate) struct CompiledProviderMetadataInputs<'a> {
    pub(crate) manifest: &'a ProjectManifest,
    pub(crate) feature_plan: &'a PackageFeaturePlan,
    pub(crate) provider_plan: &'a ProviderPlan,
    pub(crate) library_manifest_index: &'a LibraryManifestIndex,
    pub(crate) artifact_root: &'a Path,
    pub(crate) modules: &'a [ParsedModule],
    pub(crate) active_library_entrypoint: &'a ParsedModule,
    pub(crate) checked_type_info_by_path: &'a BTreeMap<PathBuf, typechecker::TypeCheckInfo>,
}

/// Command-local memo for exact project source-authority nodes.
///
/// One explicit project bake can prepare several targets and profiles that all reach the same provider roots. The
/// memo avoids walking those authored trees again within that command; it is never stored globally or carried into a
/// later command. Callers that need a final publication check deliberately create a fresh digester instead.
#[derive(Default)]
pub(crate) struct ProjectSourceAuthorityDigester {
    pub(crate) project_digests: HashMap<PathBuf, String>,
    pub(crate) rust_crate_digests: HashMap<PathBuf, String>,
    pub(crate) rust_source_closure_digests: BTreeMap<PathBuf, String>,
    #[cfg(test)]
    pub(crate) project_scan_counts: HashMap<PathBuf, usize>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::driver::build::test_support::packaged_provider_authority_fixture;
    use std::fs;

    use crate::manifest::LOAF_MANIFEST_FILENAME;

    #[test]
    fn inline_command_uses_bounded_generated_project_prefixes() {
        assert_eq!(INLINE_COMMAND_PROJECT_PREFIX, "incan_inline_command");
        assert_eq!(INLINE_COMMAND_OUTPUT_PARENT, "target/incan/inline");
    }

    #[test]
    fn explicit_bake_provider_authority_is_scanned_once_and_fresh_final_scan_rejects_an_edit()
    -> Result<(), Box<dyn std::error::Error>> {
        let (package, artifact) = packaged_provider_authority_fixture(&["debug", "release"])?;
        let mut context = OvenProjectBakeAuthorityContext::default();

        for profiles in [
            &["debug", "release"][..],
            &["debug"][..],
            &["release"][..],
            &["debug"][..],
        ] {
            let selected = context
                .checked_packaged_library_loaf_profiles(&artifact, profiles, "aarch64-apple-darwin", "rustc fixture")?
                .ok_or("fixture package profiles were not admitted")?;
            assert_eq!(selected.len(), profiles.len());
        }
        assert_eq!(context.source_digester.project_scan_count(package.path()), 1);

        let consumer = package.path().join("consumer");
        fs::create_dir_all(consumer.join("src"))?;
        fs::write(
            consumer.join(LOAF_MANIFEST_FILENAME),
            "[project]\nname = \"consumer\"\n\n[dependencies]\nprovider = { path = \"..\" }\n",
        )?;
        fs::write(consumer.join("src/main.incn"), "def main() -> None:\n    pass\n")?;
        context.project_source_authority(&consumer)?;
        assert_eq!(
            context.source_digester.project_scan_count(package.path()),
            1,
            "the root scan must reuse the provider node already admitted by target/profile preparation"
        );

        fs::write(
            package.path().join("src/lib.incn"),
            "pub def provider() -> int:\n    return 2\n",
        )?;
        let error = context
            .final_project_source_authority(&consumer)
            .err()
            .ok_or("a provider edit after memoized admission must fail final publication")?;
        assert!(error.to_string().contains("source authority changed"));
        Ok(())
    }

    #[test]
    fn explicit_bake_missing_release_provider_profile_fails_before_deep_source_scan()
    -> Result<(), Box<dyn std::error::Error>> {
        let (package, artifact) = packaged_provider_authority_fixture(&["debug"])?;
        let mut context = OvenProjectBakeAuthorityContext::default();

        assert!(
            context
                .checked_packaged_library_loaf_profiles(
                    &artifact,
                    &["debug", "release"],
                    "aarch64-apple-darwin",
                    "rustc fixture",
                )?
                .is_none()
        );
        assert_eq!(context.source_digester.project_scan_count(package.path()), 0);
        Ok(())
    }
}
