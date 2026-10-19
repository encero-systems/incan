//! Building a project: the options, project targets, completed-output payloads and report helpers every stage of
//! `incan build`, `run` and `test` exchanges.
//!
//! The work is in the submodules; this module owns the types they pass between them, and the few small helpers
//! (timings, report identity blocks, target identities) every one of them uses.

pub mod bake;
pub mod check;
pub mod executable;
mod file_freshness;
pub mod inline_command;
mod inspection_snapshot;
pub mod native_loaf_plan;
pub mod native_runtime_inputs;
pub mod native_source_output;
pub mod output_materialization;
pub mod output_paths;
mod output_publication;
pub mod output_selection;
pub mod publication;
mod retained_inputs;
mod rust_source_freshness;
pub mod source_authority;
#[cfg(any(test, feature = "test_support"))]
pub mod test_support;

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::backend::selection::BackendExecutionReceipt;
use crate::build::output_paths::{project_relative_entrypoint, validated_project_output_relative_path};
use crate::build_report::{BuildReportProject, SourceFileReport};
use crate::error::{CliError, CliResult};
use crate::lock::policy::LockPolicy;
use incan_frontend::ParsedModule;
use crate::features::FeatureSelection;
use oven_model::lock::CargoFeatureSelection;
use oven_model::manifest::ProjectManifest;
use oven_rustc::plan::OvenPackagedLibraryLoafEntry;
use oven_rustc::rustc::OvenProjectInspectionAuthorityRef;
use oven_store::digest_bytes;
use oven_store::store::OvenStoreLease;

pub const INLINE_COMMAND_PROJECT_PREFIX: &str = "incan_inline_command";

pub const INLINE_COMMAND_OUTPUT_PARENT: &str = "target/incan/inline";

/// Current wire schema for completed receipt-bound project-output Loafs.
///
/// Version 14 also admits successful ordinary executable builds with exact preparation identity, without requiring
/// the whole-project inspection authority owned by an explicit multi-target bake. Version 13 introduced the verified
/// backend receipt retained by both publication forms.
pub const OVEN_PROJECT_OUTPUT_PAYLOAD_SCHEMA_VERSION: u32 = 14;

pub const OVEN_PROJECT_OUTPUT_PROJECTION_SCHEMA_VERSION: u32 = 3;

pub const OVEN_PROJECT_OUTPUT_REPORT_SCHEMA_VERSION: u32 = 2;

pub const OVEN_PROJECT_OUTPUT_REPORT_PATH_TAG: &str = "$incan_portable_path";

pub const OVEN_PROJECT_OUTPUT_REPORT_EXTERNAL_ROOT: &str = "$INCAN_EXTERNAL_AUTHORITY";

/// Maximum time an explicit project bake waits for another explicit publisher's bounded staging transaction.
pub const OVEN_PROJECT_OUTPUT_PUBLICATION_WAIT: Duration = Duration::from_secs(5 * 60);

/// Short cooperative backoff while the named publisher retains the only safe staging reservation.
pub const OVEN_PROJECT_OUTPUT_PUBLICATION_RETRY: Duration = Duration::from_millis(100);

/// Portable path below one project-output Loaf's immutable artifact root.
pub const OVEN_PROJECT_OUTPUT_ARTIFACT_PATH: &str = "output/native";


/// Executable optimization profile for one build or run request.
///
/// Build requests default to release for compatibility; run callers select debug explicitly unless requested otherwise.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RunProfile {
    /// Unoptimized executable with debug information.
    Debug,
    /// Optimized executable; the established default for build requests.
    #[default]
    Release,
}

impl RunProfile {
    /// Return the canonical profile spelling used in Oven requests and output identities.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Debug => "debug",
            Self::Release => "release",
        }
    }
}

/// One executable build request, shared by direct and retained-owner callers.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BuildCommandOptions {
    /// Requested executable profile. Older requests without this field retain the established release default.
    #[serde(default)]
    pub executable_profile: RunProfile,
    pub lock_policy: LockPolicy,
    pub package_features: FeatureSelection,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InlineCommandProject {
    pub source_path: PathBuf,
    pub project_name: String,
    pub output_dir: String,
}

/// One manifest-backed Incan entrypoint admitted by `incan oven bake`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OvenBakeProjectTarget {
    Library,
    Executable,
}

impl OvenBakeProjectTarget {
    /// Return the stable user-facing target label.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Library => "library",
            Self::Executable => "executable",
        }
    }

    /// Return the conventional manifest-backed source entrypoint.
    pub const fn source_relative_path(self) -> &'static str {
        match self {
            Self::Library => "src/lib.incn",
            Self::Executable => "src/main.incn",
        }
    }
}

/// Return the portable identity of one project target without collapsing separately declared executable scripts.
pub fn oven_bake_project_target_identity(
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
pub fn oven_executable_entrypoint_evidence_key(project_root: &Path, entrypoint: &Path) -> CliResult<String> {
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
pub fn oven_bake_executable_output_dir(project_root: &Path, entrypoint: &Path) -> CliResult<Option<PathBuf>> {
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
pub struct OvenProjectOutputPayload {
    pub schema_version: u32,
    pub project_target: String,
    /// Stable target identity within the authored project. Conventional library and main targets retain their
    /// historical labels; every other declared executable is keyed by its project-relative entrypoint.
    pub target_identity: String,
    /// Stable logical owner of this completed output.
    ///
    /// This intentionally excludes authored source and lock evidence: those are the separate `source_authority_digest`
    /// that must match for reuse. The stable owner lets a changed project fail closed at the explicit bake boundary
    /// without putting an absolute worktree path into a portable Loaf.
    pub project_identity: String,
    pub source_authority_digest: String,
    /// Authority of dependencies and non-Incan inputs, allowing development source edits without replaying stale
    /// outputs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dependency_authority_digest: Option<String>,
    /// Derived semantic dependency fingerprint recorded by the canonical lock at bake time.
    ///
    /// The canonical lock projection remains part of `source_authority_digest`, but excludes this one derived field.
    /// A non-strict exact replay can therefore warn and reuse when only the recorded fingerprint was edited, while
    /// strict commands still recompute and reject it before completed-output selection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lock_dependencies_fingerprint: Option<String>,
    /// Exact compiler executable that produced this output; absent legacy records cannot authorize an upfront bake
    /// hit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compiler_identity_digest: Option<String>,
    /// Ordinary-command preparation inputs, including exact compiler bytes and explicit SDK/environment selections.
    /// Explicit project bakes retain their independently sealed inspection authority instead.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preparation_identity_digest: Option<String>,
    pub compiler_version: String,
    pub entrypoint_relative_path: String,
    pub build_unit_identity: String,
    pub receipt_identity: String,
    pub plan_identity: String,
    /// Verified backend selection and execution provenance from the explicit bake that produced this output.
    pub backend_receipt: BackendExecutionReceipt,
    /// Singular project-level Rust inspection authority selected through this source-current output.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inspection_authority: Option<OvenProjectInspectionAuthorityRef>,
    /// Exact inspection metadata and its receipt retained by this completed loaf. Its files live under the internal
    /// inspection-authority prefix, allowing ordinary bounded Store eviction to reclaim the standalone metadata owner.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inspection_authority_snapshot: Option<OvenProjectInspectionSnapshot>,
    /// Every generated source, package handoff record, and final native artifact retained by this completed project
    /// result. The Loaf is therefore a complete project result, not a binary-only cache entry.
    pub files: Vec<OvenProjectOutputFile>,
    /// Project-specific closure entries associated with this public library result. They remain receipt-addressed in
    /// the primary Oven store when a normal command restores this completed output.
    #[serde(default)]
    pub required_project_loafs: Vec<OvenPackagedLibraryLoafEntry>,
    /// Caller-relative root of the portable package-owned store produced by an explicit library bake. Executable
    /// results have no package handoff store.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package_loaf_store_relative_path: Option<String>,
    /// Portable bake-time report facts used by report-capable replay without re-entering the frontend.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build_report: Option<OvenProjectOutputReportSnapshot>,
}

/// Portable machine-readable report retained by one completed executable output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OvenProjectOutputReportSnapshot {
    pub schema_version: u32,
    pub report: serde_json::Value,
}

/// One immutable file in a receipt-bound completed project result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OvenProjectOutputFile {
    /// Caller-owned path below the project root where the file is restored.
    pub caller_relative_path: String,
    /// Immutable path below the completed project-output Loaf.
    pub output_relative_path: String,
    /// Exact file content retained in the Loaf.
    pub digest: String,
    /// Exact caller-visible byte length used for a cheap projection-presence check.
    #[serde(default)]
    pub logical_bytes: u64,
}

/// Caller-owned marker for an already materialized completed project result.
///
/// The immutable store entry has validated every file at publication. The marker binds the exact selected output
/// identity and caller-visible content digests so mutable projections can never become an authority of their own.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OvenProjectOutputProjection {
    pub schema_version: u32,
    pub output_identity: String,
    pub files: Vec<OvenProjectOutputProjectionFile>,
}

/// One caller-visible file retained by a completed-output projection marker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OvenProjectOutputProjectionFile {
    pub caller_relative_path: String,
    pub digest: String,
    pub logical_bytes: u64,
}

/// Input file for a project-output Loaf before the publisher computes its sealed relative paths and digests.
#[derive(Clone)]
pub struct OvenProjectOutputBakeFile {
    pub source_path: PathBuf,
    pub caller_relative_path: String,
    pub output_relative_path: String,
}

/// One completed project-output publication request, grouped so the authority helper stays readable and cannot silently
/// lose an input fact.
pub struct OvenProjectOutputBakeRequest<'a> {
    pub project_root: &'a Path,
    pub entrypoint: &'a Path,
    pub target: OvenBakeProjectTarget,
    pub receipt: &'a oven_store::OvenReceipt,
    pub plan_identity: String,
    pub profile: &'a str,
    pub source_authority_digest: &'a str,
    pub lock_dependencies_fingerprint: Option<String>,
    pub files: Vec<OvenProjectOutputBakeFile>,
    /// Explicit bakes retain inspection authority; ordinary completed outputs need only their executable authority.
    pub inspection_authority: Option<OvenProjectInspectionAuthorityRef>,
    pub required_project_loafs: Vec<OvenPackagedLibraryLoafEntry>,
    pub package_loaf_store_relative_path: Option<String>,
    pub backend_receipt: BackendExecutionReceipt,
    pub build_report: Option<OvenProjectOutputReportSnapshot>,
}

/// A selected completed project output with the store lease held for its use.
pub struct OvenStoredProjectOutput {
    pub identity: String,
    pub profile: String,
    pub intent: oven_store::OvenBuildIntent,
    pub payload: OvenProjectOutputPayload,
    pub artifact_root: PathBuf,
    pub native_output: PathBuf,
    pub _lease: OvenStoreLease,
}

/// Newly published project inspection authority retained until every completed output names it.
pub struct PublishedProjectInspectionAuthority {
    pub reference: OvenProjectInspectionAuthorityRef,
    pub snapshot: OvenProjectInspectionSnapshot,
    pub _lease: OvenStoreLease,
}

/// Original metadata publication retained by a completed output, excluding its independently selected constituents.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OvenProjectInspectionSnapshot {
    pub manifest: oven_store::store::OvenArtifactManifest,
    pub receipt: oven_store::OvenReceipt,
    /// Original JSON bytes preserved as UTF-8 rather than reserialized into a different authority identity.
    pub payload: String,
}

/// Completed target data retained until one whole-project inspection authority is finalized.
pub struct PendingOvenProjectOutput {
    pub entrypoint: PathBuf,
    pub target: OvenBakeProjectTarget,
    pub receipt: oven_store::OvenReceipt,
    pub plan_identity: String,
    pub profile: String,
    pub files: Vec<OvenProjectOutputBakeFile>,
    pub required_project_loafs: Vec<OvenPackagedLibraryLoafEntry>,
    pub package_loaf_store_relative_path: Option<String>,
    pub backend_receipt: BackendExecutionReceipt,
    pub build_report: Option<OvenProjectOutputReportSnapshot>,
}

/// Exact debug output lineage expected from one discovered target's local explicit-bake receipt.
pub struct CurrentDebugProjectOutputExpectation {
    pub target: OvenBakeProjectTarget,
    pub target_identity: String,
    pub entrypoint_relative_path: String,
    pub receipt: oven_store::OvenReceipt,
}

/// Build the project identity block used by build and generated Rust inspection reports.
pub fn manifest_project_report(
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
pub fn source_file_report(modules: &[ParsedModule]) -> Vec<SourceFileReport> {
    modules
        .iter()
        .map(|module| SourceFileReport {
            path: module.file_path.to_string_lossy().to_string(),
            module_path: module.path_segments.clone(),
        })
        .collect()
}

/// Return elapsed milliseconds as a bounded `u64` for report payloads.
pub fn elapsed_ms(start: Instant) -> u64 {
    start.elapsed().as_millis().min(u128::from(u64::MAX)) as u64
}

/// Record one named build phase timing.
pub fn record_timing(timings: &mut BTreeMap<String, u64>, name: &str, start: Instant) {
    timings.insert(name.to_string(), elapsed_ms(start));
}

/// Normal-command policy that must be satisfied before a completed project output may be selected.
pub struct CompletedOutputPolicy<'a> {
    pub lock_policy: &'a LockPolicy,
    pub package_features: &'a FeatureSelection,
}

impl CompletedOutputPolicy<'_> {
    /// Return the dependency feature evidence canonical lock validation records; commands select none of their own.
    pub fn cargo_feature_selection(&self) -> CargoFeatureSelection {
        CargoFeatureSelection::default().normalized()
    }
}

/// Borrowed inputs needed to freeze checked provider facts into a library's checked contract.
///
/// The checked type information stays in the compiler pipeline until this projection. It carries the canonical
/// callable-to-capability facts that provider operation lowering consumes through the selected manifest.
pub struct CompiledProviderMetadataInputs<'a> {
    pub manifest: &'a ProjectManifest,
    pub feature_plan: &'a incan_provider::PackageFeaturePlan,
    pub provider_plan: &'a incan_provider::ProviderPlan,
    pub library_manifest_index: &'a incan_frontend::library_manifest_index::LibraryManifestIndex,
    pub artifact_root: &'a Path,
    pub modules: &'a [ParsedModule],
    pub active_library_entrypoint: &'a ParsedModule,
    pub checked_type_info_by_path: &'a BTreeMap<PathBuf, incan_frontend::typechecker::TypeCheckInfo>,
}

/// Command-local source-authority memo with observation-guarded typed inputs retained by serial owners.
///
/// One explicit project bake can prepare several targets and profiles that all reach the same provider roots. The
/// memo avoids walking authored trees again within that command. Rust closure digests additionally have a persistent
/// stat-guarded cache, checked against observed inputs on every command. Serial executable owners clear all final
/// digests at the start of a request and retain only typed inputs that revalidate their observations. Final publication
/// uses a fresh digester.
#[derive(Default)]
pub struct ProjectSourceAuthorityDigester {
    /// Typed canonical lock inputs; unlike final source digests, these revalidate their observations on lookup.
    retained_inputs: retained_inputs::RetainedProjectInputs,
    /// Opt-in original authored input observations for cold publication, separate from portable content identity.
    source_observations: Option<oven_model::observed_inputs::InputObservations>,
    /// Only this root may omit its own Incan sources when hashing development dependency authority.
    pub development_root: Option<PathBuf>,
    pub project_digests: HashMap<PathBuf, String>,
    pub rust_crate_digests: HashMap<PathBuf, String>,
    pub rust_source_closure_digests: BTreeMap<PathBuf, String>,
    #[cfg(test)]
    pub project_scan_counts: HashMap<PathBuf, usize>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inline_command_uses_bounded_generated_project_prefixes() {
        assert_eq!(INLINE_COMMAND_PROJECT_PREFIX, "incan_inline_command");
        assert_eq!(INLINE_COMMAND_OUTPUT_PARENT, "target/incan/inline");
    }
}
