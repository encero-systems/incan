//! Build and run pipeline for Incan projects.
//!
//! This module handles the full compilation flow: module collection, type checking, codegen configuration, dependency
//! resolution, project generation, and receipt-bound direct-`rustc` Oven execution.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::env;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use incan_semantics_core::HirSourceSpan;
use serde::{Deserialize, Serialize};

use crate::backend::project::generator::GENERATED_CARGO_TARGET_DIR_ENV;
use crate::backend::project::runner::resolved_cargo_executable;
use crate::backend::replacement::{
    ReplacementExecutionError, ReplacementExecutionGraph, execute_prevalidated_free_function,
    prepare_free_function_execution_in_graph,
};
use crate::backend::selection::{
    BackendExecutionReceipt, BackendKind, BackendSelection, FallbackOutcome, FallbackPolicy, SemanticModuleProvenance,
    ShadowComparisonState, digest_output, finalize_receipt, finalize_receipt_with_semantic_module, resolve_execution,
    select_backend, unavailable_shadow_comparison,
};
use crate::backend::shadow::PROGRAM_ENTRYPOINT_UNAVAILABLE_REASON;
use crate::backend::{IrCodegen, ProjectGenerator};
use crate::cli::{CliError, CliResult, ExitCode};
use crate::compiled_sdk::CompiledSdkModules;
use crate::dependency_resolver::{ResolvedDependencies, resolve_reachable_dependencies};
use crate::frontend::api_metadata::{
    CHECKED_API_METADATA_SCHEMA_VERSION, CheckedApiMetadataPackage, CheckedApiPackageIdentity,
    collect_checked_api_alias_metadata, collect_checked_api_metadata, materialize_api_alias_projections,
    materialize_checked_api_public_namespaces, validate_checked_api_docstrings,
};
use crate::frontend::ast::{Declaration, Decorator, Expr, ImportKind, Literal, Span, Spanned, Statement, Visibility};
use crate::frontend::body_ir::{
    build_body_ir_module_v0, is_direct_replacement_fieldless_enum, is_direct_replacement_plain_model,
    is_direct_replacement_value_enum,
};
use crate::frontend::contract_metadata::{ContractMetadataPackage, read_project_model_bundles};
use crate::frontend::library_exports::{
    CheckedExportKind, CheckedNamedExport, LibraryExportBindingRegistry, collect_checked_public_exports,
};
use crate::frontend::library_manifest_index::{
    LibraryArtifactKind, LibraryArtifactMetadata, LibraryManifestIndex, LibraryManifestIndexEntry,
    dependency_project_root, load_provider_dependency_artifact,
};
use crate::frontend::module::{
    SourceModuleImportResolution, canonicalize_source_module_segments, resolve_program_source_imports,
    resolve_source_module_import_from_source_file, self_import_diagnostic_message,
};
use crate::frontend::registry_metadata::{
    CHECKED_REGISTRY_METADATA_SCHEMA_VERSION, CheckedRegistryMetadataPackage, CheckedRegistryPackageIdentity,
    collect_checked_registry_metadata, materialize_registry_reexport_projections,
};
use crate::frontend::typechecker::stdlib_loader::StdlibAstCache;
use crate::frontend::{diagnostics, typechecker};
use crate::generated_cache::resolve_generated_cargo_target;
#[cfg(feature = "rust_inspect")]
use crate::library_manifest::LibraryRustAbi;
use crate::library_manifest::{
    CompiledProviderMetadata, LibraryManifest, ProviderCargoDependency, ProviderCargoDependencySource,
    ProviderDependencyKind, ProviderDependencyMetadata, ProviderFactKind, ProviderFactRequirement,
    ProviderImplementationFacet, ProviderModuleClaim, ProviderOperationMetadata,
    digest_cargo_path_source_tree_with_cache, digest_provider_artifact, digest_provider_source_inputs,
};
use crate::lockfile::{CargoFeatureSelection, IncanLock, provider_semantic_identities, semantic_lock_state};
use crate::manifest::{DependencySource, DependencySpec, GitReference, MANIFEST_FILENAME, ProjectManifest};
use crate::oven::interop::{
    OVEN_INTEROP_EXECUTION_RECEIPT_INPUT, default_interop_execution_receipt_path, interop_execution_build_unit_inputs,
    load_interop_execution_receipt, validate_interop_execution_receipt,
};
use crate::oven::legacy_cargo::{
    OVEN_PROJECT_EXTENSION_PAYLOAD_SCHEMA_VERSION, OvenLegacyCargoBaseLoaf, OvenLegacyCargoDirectDependencyClosure,
    OvenLegacyCargoPrepareRequest, OvenLegacyCargoPublicationKind, OvenProjectExtensionPayload,
    OvenProjectRegistrySourceDependency, digest_local_cargo_workspace_authority, direct_rustc_compile_environment,
    direct_rustc_reusable_project_plan_environment, prepare_direct_rustc_plan, stage_locked_loaf_fixture,
};
use crate::oven::loaf::{
    OVEN_DEPENDENCY_MISS_SUMMARY, OVEN_LOAF_ENV, OVEN_LOAF_MISS_GUIDANCE, OVEN_NESTED_DEPENDENCY_MISS_SUMMARY,
    OVEN_NO_IMPLICIT_DEPENDENCY_BUILD, OVEN_SOURCE_COMPILER_VOCAB_SUPPORT_BUILD_INPUT, OvenToolchainLoaf,
    resolve_compiler_owned_loaf_by_identity, resolve_compiler_owned_loaf_for_registry_dependencies,
    runtime_build_unit_inputs,
};
#[cfg(test)]
use crate::oven::rustc::direct_rustc_source_extern_names;
use crate::oven::rustc::{
    OVEN_PROJECT_INSPECTION_AUTHORITY_SCHEMA_VERSION, OVEN_RUSTC_REGISTRY_LOCK_RELATIVE_PATH,
    OvenCallerOwnedRustcLibrary, OvenLoadedProjectInspectionAuthority, OvenProjectInspectionAuthorityPayload,
    OvenProjectInspectionAuthorityRef, OvenProjectInspectionConstituent, OvenProjectInspectionGeneratedOutDir,
    OvenProjectInspectionRootDependency, OvenProjectInspectionSource, OvenProjectInspectionSourceOwner,
    OvenProjectInspectionTestDependencyEnvelope, OvenProjectInspectionTestDependencyRoot, OvenRegistryLeafAuthority,
    OvenRustcArtifactExtern, OvenRustcArtifactManifest, OvenRustcArtifactPlan, OvenRustcError, OvenRustcRegistryLeaf,
    OvenRustcRegistrySourcePackage, OvenRustcSupportingArtifact, OvenSelectedPathRustcAuthority,
    OvenTrustedDirectRustcTargetRequest, OvenTrustedRustcArtifactRoot, attach_caller_owned_rustc_libraries,
    bake_trusted_direct_rustc_library, bake_trusted_direct_rustc_proc_macro, bake_trusted_direct_rustc_run,
    clear_inherited_cargo_environment, load_project_inspection_authority,
    materialize_declared_rust_libraries_with_selected_path_authority, project_inspection_constituent_matches_receipt,
    resolve_active_rustc, rustc_host_target, rustc_identity, select_direct_rustc_plan_for_execution,
    trusted_artifact_plan_for_source_evidence, validate_project_extension_payload_against_base,
    validate_project_inspection_authority_payload, validate_selected_sealed_registry_leaf,
};
use crate::oven::store::{
    OvenArtifactKind, OvenArtifactMaterializedFile, OvenArtifactPublishRequest, OvenStore, OvenStoreError,
    OvenStoreLease,
};
use crate::oven::{
    OvenGeneratedProjectRequest, digest_bytes, digest_dependency_specs, digest_project_source_tree,
    generated_project_source_evidence, receipt_generated_project, receipt_generated_project_with_source_evidence,
    write_receipt,
};
use crate::oven_interop::locked_oven_interop_targets;
use crate::provider::{
    FeatureSelection, PackageFeatureGraph, PackageFeaturePlan, ProviderPlan, SDK_PROVIDER_BUILD_ENV,
};
use crate::version::INCAN_VERSION;

use super::build_report::{
    BUILD_REPORT_SCHEMA_VERSION, BuildOvenReport, BuildReport, BuildReportDraft, BuildReportMode, BuildReportOptions,
    BuildReportProject, RustInspectionFormat, SourceFileReport, artifact_report, cargo_report, dependencies_report,
    emit_build_report, emit_rust_inspection_report, emit_workspace_build_report, generated_project_report,
    incan_dependencies_report, interop_report, oven_generated_project_report, rust_inspection_report, semantic_report,
};
#[cfg(test)]
use super::common::dependency_specs_match;
use super::common::{
    CargoPolicy, CompilationSession, INTERNAL_LIBRARY_ARTIFACT_ONLY_ENV, ProjectRequirements, build_source_map,
    cargo_command_flags, collect_incan_source_files, collect_modules_detailed_with_session,
    collect_project_requirements, collect_rust_dependency_uses, discover_effective_project_manifest,
    effective_project_manifest_for_exact_root, enforce_project_toolchain_constraint,
    extend_requirements_with_provider_plan, format_dependency_error, imported_module_deps_for_with_provider_plan,
    merge_project_requirement_dependencies, module_key_index, register_module_path_segments, render_module_warnings,
    resolve_project_root, resolve_source_root, semantic_sdk_path_dependencies, validate_output_dir,
};
#[cfg(feature = "rust_inspect")]
use super::common::{
    collect_rust_inspect_derive_probe_paths, collect_rust_inspect_query_paths,
    collect_rust_inspect_query_paths_from_programs, mark_oven_direct_rust_inspection,
};
use super::lock::{
    LockResolution, LockResolutionRequest, PublishedOvenProjectLock, publish_oven_project_lock, resolve_lock_context,
    validate_oven_lock_policy,
};
#[cfg(feature = "rust_inspect")]
use super::lock::{
    OvenRustInspectSourceAuthorityRequest, RustInspectWorkspaceRequest, prepare_project_registry_source_authorities,
    prepare_rust_inspect_workspace,
};
use super::oven::open_default_oven_store;
use super::vocab_extraction::{
    PendingDesugarerArtifact, collect_library_vocab_metadata, oven_vocab_direct_rustc_context_from_plan,
};
use crate::cli::prelude::ParsedModule;
#[cfg(feature = "rust_inspect")]
use crate::rust_inspect::{Inspector, InspectorConfig, RustMetadataCache, RustMetadataError};
use sha2::{Digest as _, Sha256};

// ============================================================================
// Project Preparation (shared between build and run)
// ============================================================================

const INLINE_COMMAND_PROJECT_PREFIX: &str = "incan_inline_command";
const INLINE_COMMAND_OUTPUT_PARENT: &str = "target/incan/inline";
/// Stable package-artifact location for immutable provider Loafs.
const OVEN_PACKAGED_LIBRARY_LOAF_STORE_RELATIVE_PATH: &str = "oven/loafs";
/// Stable package-artifact manifest that maps each provider profile to its sealed Loaf and direct library output.
const OVEN_PACKAGED_LIBRARY_LOAF_MANIFEST_RELATIVE_PATH: &str = "oven/package-loafs.json";
/// Current wire schema for package-owned Oven Loaf handoff metadata.
///
/// Version 6 seals the checked `.incnlib` manifest and every manifest-declared provider sidecar by relative path and
/// digest, in addition to requiring release-cohort project-extension entries.
const OVEN_PACKAGED_LIBRARY_LOAF_SCHEMA_VERSION: u32 = 6;
/// Current wire schema for completed receipt-bound project-output Loafs.
///
/// Version 13 carries the verified default backend execution receipt alongside every completed output. This lets a
/// source-current cache hit retain the same compiler-backend provenance as the explicit bake without trusting a stale
/// report snapshot.
const OVEN_PROJECT_OUTPUT_PAYLOAD_SCHEMA_VERSION: u32 = 13;
const OVEN_PROJECT_OUTPUT_PROJECTION_SCHEMA_VERSION: u32 = 3;
const OVEN_PROJECT_OUTPUT_REPORT_SCHEMA_VERSION: u32 = 2;
const OVEN_PROJECT_OUTPUT_REPORT_PATH_TAG: &str = "$incan_portable_path";
const OVEN_PROJECT_OUTPUT_REPORT_EXTERNAL_ROOT: &str = "$INCAN_EXTERNAL_AUTHORITY";
/// Maximum time an explicit project bake waits for another explicit publisher's bounded staging transaction.
const OVEN_PROJECT_OUTPUT_PUBLICATION_WAIT: Duration = Duration::from_secs(5 * 60);
/// Short cooperative backoff while the named publisher retains the only safe staging reservation.
const OVEN_PROJECT_OUTPUT_PUBLICATION_RETRY: Duration = Duration::from_millis(100);
/// Portable path below one project-output Loaf's immutable artifact root.
const OVEN_PROJECT_OUTPUT_ARTIFACT_PATH: &str = "output/native";

/// Prepared source and immutable build-unit selection for the normal Oven Alpha executable path.
///
/// This deliberately contains no Cargo target path or command. Generated Rust and the final binary are caller-owned;
/// the selected native closure retains its store lease until the direct-Rustc bake and any child execution complete.
struct OvenPreparedProject {
    generator: ProjectGenerator,
    project_root: PathBuf,
    entrypoint: PathBuf,
    provider_plan: Arc<ProviderPlan>,
    receipt: crate::oven::OvenReceipt,
    plan_selection: OvenDirectRustcPlanSelection,
    materialization: OvenToolchainMaterialization,
    cargo_process_started: bool,
    rustc: PathBuf,
    crate_name: String,
    rust_edition: String,
    caller_owned_libraries: Vec<OvenCallerOwnedRustcLibrary>,
    report: BuildReportDraft,
    #[cfg(feature = "rust_inspect")]
    rust_inspect_manifest_dir: Option<PathBuf>,
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
    fn allows_completed_output_reuse(&self) -> bool {
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
    fn effective_generated_cargo_target_dir(&self) -> Option<PathBuf> {
        self.generated_cargo_target_dir.clone().or_else(|| {
            env::var_os(GENERATED_CARGO_TARGET_DIR_ENV)
                .filter(|raw| !raw.is_empty())
                .map(PathBuf::from)
        })
    }
}

#[derive(Debug, Clone, Copy, Default)]
#[cfg(test)]
struct PrepareProjectOptions<'a> {
    output_dir: Option<&'a str>,
    project_name_override: Option<&'a str>,
    generated_cargo_target_dir: Option<&'a Path>,
    cargo_profile: &'a str,
    sdk_profile_override: Option<&'a str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct InlineCommandProject {
    source_path: PathBuf,
    project_name: String,
    output_dir: String,
}

/// A prepared library project after Incan validation and Rust source generation.
///
/// The legacy publisher may still retain Cargo-only preparation state. Normal `incan build --lib`, however, always
/// carries an Oven selection and compiles through direct `rustc` without entering that state.
struct PreparedLibraryProject {
    generator: ProjectGenerator,
    project_root: PathBuf,
    entrypoint: PathBuf,
    out_dir: PathBuf,
    manifest_path: PathBuf,
    library_manifest: LibraryManifest,
    timings_ms: BTreeMap<String, u64>,
    report: BuildReportDraft,
    oven: Option<OvenPreparedLibrary>,
    #[cfg(feature = "rust_inspect")]
    rust_inspect_manifest_dir: Option<PathBuf>,
}

/// Receipt-selected direct-rustc materialization state for a normal Oven library build.
struct OvenPreparedLibrary {
    rustc: PathBuf,
    crate_name: String,
    rust_edition: String,
    profiles: BTreeMap<String, OvenPreparedLibraryProfile>,
}

/// One profile-specific direct-rustc library selection.
///
/// A normal library build publishes both debug and release caller-owned outputs. `incan run` defaults to debug while
/// `incan build --lib` has historically produced a release artifact; retaining both avoids linking a library against
/// a different profile's hashed Rust dependencies and never delegates that mismatch to Cargo.
struct OvenPreparedLibraryProfile {
    receipt: crate::oven::OvenReceipt,
    plan_selection: OvenDirectRustcPlanSelection,
    materialization: OvenToolchainMaterialization,
    provider_plan: Arc<ProviderPlan>,
    caller_owned_libraries: Vec<OvenCallerOwnedRustcLibrary>,
}

/// Observable outcome of acquiring one receipt-compatible Oven closure.
///
/// `ToolchainLoaf` means the complete release-version standard-library closure was selected directly from the active
/// immutable toolchain generation. It is intentionally not copied into every project store.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OvenToolchainMaterialization {
    Reused,
    ToolchainLoaf,
    CompatibilityBaked,
}

impl OvenToolchainMaterialization {
    /// Return the stable report spelling for this caller-visible selection outcome.
    const fn as_str(self) -> &'static str {
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
enum OvenProjectPlanMode {
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
    const fn is_explicit_publisher(self) -> bool {
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
struct OvenPackagedLibraryLoafManifest {
    schema_version: u32,
    /// Complete authored root and transitive path-dependency authority that produced these package Loafs.
    source_authority_digest: String,
    /// Incan release that generated the sealed provider output.
    ///
    /// A release version is the compiler compatibility boundary for package Loafs. Hashing the complete compiler
    /// executable again on each warm explicit bake would make a valid reuse reread hundreds of megabytes despite
    /// the receipt already recording this release fact.
    compiler_version: String,
    /// Caller-visible checked metadata and manifest-declared sidecars authorized by this package handoff.
    ///
    /// Consumers read `.incnlib` before linking the native output, so sealing only the rlib would permit mutable
    /// metadata to describe a different API, vocabulary surface, or desugarer than the explicit provider bake
    /// produced. These records bind that complete public handoff without copying it into each profile.
    metadata_files: Vec<OvenPackagedLibraryMetadataFile>,
    profiles: BTreeMap<String, OvenPackagedLibraryLoafProfile>,
}

/// One caller-visible provider metadata file sealed by a package Loaf handoff.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct OvenPackagedLibraryMetadataFile {
    /// Safe path below the provider's generated artifact root.
    relative_path: String,
    /// Exact content digest published by the explicit provider bake.
    digest: String,
}

/// One profile-specific public-library handoff record.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct OvenPackagedLibraryLoafProfile {
    /// Original producer receipt whose source, provider, lock, and toolchain facts authorized the closure.
    receipt: crate::oven::OvenReceipt,
    /// Every immutable entry whose closure the provider library links against.
    ///
    /// A package can depend on several independently baked public providers.  Retaining the entire compatible
    /// collection here keeps the next consumer from silently recompiling one omitted upstream dependency.
    #[serde(default)]
    entries: Vec<OvenPackagedLibraryLoafEntry>,
    /// Provider library path relative to its generated artifact root.
    library_relative_path: String,
    /// Digest of the direct-rustc provider library output.
    library_digest: String,
}

/// One package profile admitted once for the duration of a single consumer preparation.
///
/// This is deliberately command-local rather than a persistent cache: provider source authority and sealed output
/// bytes are checked once, then the same immutable facts flow through import and final selection.
#[derive(Clone)]
struct CheckedPackagedProviderProfile {
    dependency_key: String,
    artifact_root: PathBuf,
    profile: String,
    package: OvenPackagedLibraryLoafProfile,
}

/// One provider index retained only while a single explicit project bake is running.
struct MemoizedPackagedProviderAuthority {
    artifact: LibraryArtifactMetadata,
    manifest_path: PathBuf,
    manifest_digest: String,
    manifest: Arc<OvenPackagedLibraryLoafManifest>,
    source_project_root: Option<PathBuf>,
    source_authority_verified: bool,
    admitted_profiles: BTreeMap<String, (String, String)>,
}

/// Source-authority state shared by every target/profile preparation inside one explicit project bake.
///
/// The context is stack-owned by `bake_oven_project_targets`; it deliberately has no static lifetime, timestamp-based
/// invalidation, or representation outside this command invocation.
#[derive(Default)]
struct OvenProjectBakeAuthorityContext {
    source_digester: ProjectSourceAuthorityDigester,
    providers: HashMap<PathBuf, MemoizedPackagedProviderAuthority>,
    initial_project_source_authority: Option<String>,
}

/// One immutable store entry transported by a public library package.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct OvenPackagedLibraryLoafEntry {
    /// Receipt that originally authorized this immutable store entry.
    receipt: crate::oven::OvenReceipt,
    /// Content address of the entry payload.
    identity: String,
    /// Semantic payload role of `identity`.
    kind: OvenArtifactKind,
    /// Exact compiler-shipped base Loaf required by a project-extension entry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    base_loaf_identity: Option<String>,
}

/// One manifest-backed Incan entrypoint admitted by `incan oven bake`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OvenBakeProjectTarget {
    Library,
    Executable,
}

impl OvenBakeProjectTarget {
    /// Return the stable user-facing target label.
    const fn as_str(self) -> &'static str {
        match self {
            Self::Library => "library",
            Self::Executable => "executable",
        }
    }

    /// Return the conventional manifest-backed source entrypoint.
    const fn source_relative_path(self) -> &'static str {
        match self {
            Self::Library => "src/lib.incn",
            Self::Executable => "src/main.incn",
        }
    }
}

/// Return the portable identity of one project target without collapsing separately declared executable scripts.
fn oven_bake_project_target_identity(
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
fn oven_executable_entrypoint_evidence_key(project_root: &Path, entrypoint: &Path) -> CliResult<String> {
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
fn oven_bake_executable_output_dir(project_root: &Path, entrypoint: &Path) -> CliResult<Option<PathBuf>> {
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
struct OvenProjectOutputPayload {
    schema_version: u32,
    project_target: String,
    /// Stable target identity within the authored project. Conventional library and main targets retain their
    /// historical labels; every other declared executable is keyed by its project-relative entrypoint.
    target_identity: String,
    /// Stable logical owner of this completed output.
    ///
    /// This intentionally excludes authored source and lock evidence: those are the separate `source_authority_digest`
    /// that must match for reuse. The stable owner lets a changed project fail closed at the explicit bake boundary
    /// without putting an absolute worktree path into a portable Loaf.
    project_identity: String,
    source_authority_digest: String,
    /// Derived semantic dependency fingerprint recorded by the canonical lock at bake time.
    ///
    /// The canonical lock projection remains part of `source_authority_digest`, but excludes this one derived field.
    /// A non-strict exact replay can therefore warn and reuse when only the recorded fingerprint was edited, while
    /// strict commands still recompute and reject it before completed-output selection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    lock_dependencies_fingerprint: Option<String>,
    compiler_version: String,
    entrypoint_relative_path: String,
    build_unit_identity: String,
    receipt_identity: String,
    plan_identity: String,
    /// Verified backend selection and execution provenance from the explicit bake that produced this output.
    backend_receipt: BackendExecutionReceipt,
    /// Singular project-level Rust inspection authority selected through this source-current output.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    inspection_authority: Option<OvenProjectInspectionAuthorityRef>,
    /// Every generated source, package handoff record, and final native artifact retained by this completed project
    /// result. The Loaf is therefore a complete project result, not a binary-only cache entry.
    files: Vec<OvenProjectOutputFile>,
    /// Project-specific closure entries associated with this public library result. They remain receipt-addressed in
    /// the primary Oven store when a normal command restores this completed output.
    #[serde(default)]
    required_project_loafs: Vec<OvenPackagedLibraryLoafEntry>,
    /// Caller-relative root of the portable package-owned store produced by an explicit library bake. Executable
    /// results have no package handoff store.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    package_loaf_store_relative_path: Option<String>,
    /// Portable bake-time report facts used by report-capable replay without re-entering the frontend.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    build_report: Option<OvenProjectOutputReportSnapshot>,
}

/// Portable machine-readable report retained by one completed executable output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct OvenProjectOutputReportSnapshot {
    schema_version: u32,
    report: serde_json::Value,
}

/// One immutable file in a receipt-bound completed project result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct OvenProjectOutputFile {
    /// Caller-owned path below the project root where the file is restored.
    caller_relative_path: String,
    /// Immutable path below the completed project-output Loaf.
    output_relative_path: String,
    /// Exact file content retained in the Loaf.
    digest: String,
    /// Exact caller-visible byte length used for a cheap projection-presence check.
    #[serde(default)]
    logical_bytes: u64,
}

/// Caller-owned marker for an already materialized completed project result.
///
/// The immutable store entry has validated every file at publication. The marker binds the exact selected output
/// identity and caller-visible content digests so mutable projections can never become an authority of their own.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct OvenProjectOutputProjection {
    schema_version: u32,
    output_identity: String,
    files: Vec<OvenProjectOutputProjectionFile>,
}

/// One caller-visible file retained by a completed-output projection marker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct OvenProjectOutputProjectionFile {
    caller_relative_path: String,
    digest: String,
    logical_bytes: u64,
}

/// Input file for a project-output Loaf before the publisher computes its sealed relative paths and digests.
#[derive(Clone)]
struct OvenProjectOutputBakeFile {
    source_path: PathBuf,
    caller_relative_path: String,
    output_relative_path: String,
}

/// One completed project-output publication request, grouped so the authority helper stays readable and cannot silently
/// lose an input fact.
struct OvenProjectOutputBakeRequest<'a> {
    project_root: &'a Path,
    entrypoint: &'a Path,
    target: OvenBakeProjectTarget,
    receipt: &'a crate::oven::OvenReceipt,
    plan_identity: String,
    profile: &'a str,
    source_authority_digest: &'a str,
    lock_dependencies_fingerprint: Option<String>,
    files: Vec<OvenProjectOutputBakeFile>,
    inspection_authority: OvenProjectInspectionAuthorityRef,
    required_project_loafs: Vec<OvenPackagedLibraryLoafEntry>,
    package_loaf_store_relative_path: Option<String>,
    backend_receipt: BackendExecutionReceipt,
    build_report: Option<OvenProjectOutputReportSnapshot>,
}

/// A selected completed project output with the store lease held for its use.
struct OvenStoredProjectOutput {
    identity: String,
    profile: String,
    intent: crate::oven::OvenBuildIntent,
    payload: OvenProjectOutputPayload,
    artifact_root: PathBuf,
    native_output: PathBuf,
    _lease: OvenStoreLease,
}

/// Newly published project inspection authority retained until every completed output names it.
struct PublishedProjectInspectionAuthority {
    reference: OvenProjectInspectionAuthorityRef,
    _lease: OvenStoreLease,
}

/// Completed target data retained until one whole-project inspection authority is finalized.
struct PendingOvenProjectOutput {
    entrypoint: PathBuf,
    target: OvenBakeProjectTarget,
    receipt: crate::oven::OvenReceipt,
    plan_identity: String,
    profile: String,
    files: Vec<OvenProjectOutputBakeFile>,
    required_project_loafs: Vec<OvenPackagedLibraryLoafEntry>,
    package_loaf_store_relative_path: Option<String>,
    backend_receipt: BackendExecutionReceipt,
    build_report: Option<OvenProjectOutputReportSnapshot>,
}

/// Exact debug output lineage expected from one discovered target's local explicit-bake receipt.
struct CurrentDebugProjectOutputExpectation {
    target: OvenBakeProjectTarget,
    target_identity: String,
    entrypoint_relative_path: String,
    receipt: crate::oven::OvenReceipt,
}

/// Receipt-selected direct-Rustc closure for a normal Oven command.
///
/// A selection is either a receipt-bound project closure in the bounded local store or a complete compiler-shipped
/// standard-library Loaf held stable by its immutable generation lock. The latter remains direct so one versioned
/// stdlib closure cannot become many per-project cache copies.
pub(crate) enum OvenDirectRustcPlanSelection {
    Stored(Box<OvenStoredDirectRustcExecutionPlan>),
    ToolchainLoaf(Box<OvenToolchainLoaf>),
    ProjectExtension(Box<OvenProjectExtensionExecutionPlan>),
    /// The ABI-compatible package Loaf closure selected for public providers in this consumer.
    PackagedProvider(Box<OvenPackagedProviderExecutionPlan>),
}

impl OvenDirectRustcPlanSelection {
    /// Return the receipt-bound identity included in a normal-command build report.
    fn report_identity(&self) -> String {
        match self {
            Self::Stored(selected) => selected.identity.clone(),
            Self::ToolchainLoaf(native) => {
                format!("loaf:{}", native.loaf_build_unit_identity)
            }
            Self::ProjectExtension(extension) => format!(
                "loaf:{}+extension:{}",
                extension.base.loaf_identity, extension.extension.identity
            ),
            Self::PackagedProvider(packages) => packages.report_identity(),
        }
    }

    /// Return every immutable project entry that must travel with a public package.
    ///
    /// Compiler-shipped Loafs remain in the installed release envelope. Project deltas, by contrast, contain the
    /// provider's third-party Rust closure and must be carried by the package that owns that provider.
    fn package_entries(&self, receipt: &crate::oven::OvenReceipt) -> Vec<OvenPackagedLibraryLoafEntry> {
        match self {
            Self::Stored(selected) => vec![OvenPackagedLibraryLoafEntry {
                receipt: receipt.clone(),
                identity: selected.identity.clone(),
                kind: OvenArtifactKind::DirectRustcPlan,
                base_loaf_identity: None,
            }],
            Self::ProjectExtension(extension) => vec![OvenPackagedLibraryLoafEntry {
                receipt: receipt.clone(),
                identity: extension.extension.identity.clone(),
                kind: OvenArtifactKind::ProjectPayload,
                base_loaf_identity: Some(extension.base.loaf_identity.clone()),
            }],
            Self::PackagedProvider(packages) => packages.package_entries(),
            Self::ToolchainLoaf(_) => Vec::new(),
        }
    }

    /// Return the exact already-selected direct-Rustc closure.
    ///
    /// Callers must derive both omission and selected-path authority from this same plan. Reconstructing only its
    /// crate-name set loses the path/receipt relationship that distinguishes a compiler runtime from a lookalike
    /// caller dependency.
    pub(crate) fn artifact_plan(&self) -> &OvenRustcArtifactPlan {
        match self {
            Self::Stored(selected) => &selected.artifact_plan,
            Self::ToolchainLoaf(native) => &native.artifact_plan,
            Self::ProjectExtension(extension) => &extension.artifact_plan,
            Self::PackagedProvider(packages) => packages.artifact_plan(),
        }
    }

    /// Project the verified selected plan to the externs that its receipt admits to one generated source root.
    ///
    /// The complete plan also carries compiler-private support crates. They remain available to compiler-owned
    /// roots, but must not cause a normal project declaration with the same crate name to be skipped.
    pub(crate) fn source_artifact_plan(
        &self,
        source_evidence_key: &str,
    ) -> Result<OvenRustcArtifactPlan, OvenRustcError> {
        let mut plan =
            trusted_artifact_plan_for_source_evidence(self.artifact_plan(), self.artifacts(), source_evidence_key)?;
        if let Self::PackagedProvider(packages) = self {
            packages.retain_fragment_dependency_search_paths(&mut plan);
        }
        Ok(plan)
    }

    /// Return the complete execution manifest retained by this selected closure.
    pub(crate) fn artifacts(&self) -> &OvenRustcArtifactManifest {
        match self {
            Self::Stored(selected) => &selected.artifacts,
            Self::ToolchainLoaf(native) => &native.artifacts,
            Self::ProjectExtension(extension) => &extension.artifacts,
            Self::PackagedProvider(packages) => packages.artifacts(),
        }
    }

    /// Return one immutable root used only for caller-output containment checks.
    ///
    /// A composed extension passes its extension root here while its already verified `artifact_plan` supplies paths
    /// from both leased roots.  The output itself remains caller-owned and must be outside either root.
    pub(crate) fn output_guard_root(&self) -> &Path {
        match self {
            Self::Stored(selected) => &selected.artifact_root,
            Self::ToolchainLoaf(native) => &native.artifact_root,
            Self::ProjectExtension(extension) => &extension.extension.artifact_root,
            Self::PackagedProvider(packages) => packages.output_guard_root(),
        }
    }

    /// Return whether this selection already contains the public provider's complete compiled Rust closure.
    pub(crate) fn uses_packaged_provider_closure(&self) -> bool {
        matches!(self, Self::PackagedProvider(_))
    }

    /// Return whether this exact receipt-selected plan already seals the current generated project's path crates.
    ///
    /// A stored direct plan and a project extension are both produced for the current receipt, so their projected
    /// externs can satisfy matching declared path dependencies without a second materialization. A toolchain Loaf
    /// and an imported public-package closure are not current-project authority: a same-named caller path crate
    /// there must remain explicit rather than being mistaken for compiler or provider-private code.
    fn seals_current_project_path_dependencies(&self) -> bool {
        matches!(self, Self::Stored(_) | Self::ProjectExtension(_))
    }

    /// Return the one root that contains every vocabulary auxiliary closure, when it is not split across fragments.
    pub(crate) fn vocab_artifact_root(&self) -> Option<&Path> {
        match self {
            Self::Stored(selected) => Some(&selected.artifact_root),
            Self::ToolchainLoaf(native) => Some(&native.artifact_root),
            Self::ProjectExtension(extension) => extension.vocab_artifact_root.as_deref(),
            Self::PackagedProvider(packages) => packages.vocab_artifact_root(),
        }
    }

    /// Build the registry authority from exactly the roots that form this selected closure.
    pub(crate) fn registry_leaf_authority(&self) -> Option<OvenRegistryLeafAuthority> {
        match self {
            Self::Stored(selected) => selected
                .artifacts
                .registry_leaf_authority(&selected.artifact_root, &selected.artifact_plan),
            Self::ToolchainLoaf(native) => Some(native.registry_leaf_authority()),
            Self::ProjectExtension(extension) => extension.registry_leaf_authority.clone(),
            Self::PackagedProvider(packages) => packages.registry_leaf_authority(),
        }
    }
}

/// Receipt-validated stored direct-Rustc inputs held under a caller-owned lease.
///
/// The lease stays alive while a caller-owned package is re-materialized, so policy pruning cannot remove the
/// selected cohort between that compilation and the consuming normal Oven bake.
pub(crate) struct OvenStoredDirectRustcExecutionPlan {
    pub identity: String,
    pub artifacts: OvenRustcArtifactManifest,
    pub artifact_root: PathBuf,
    pub artifact_plan: OvenRustcArtifactPlan,
    _lease: OvenStoreLease,
}

/// Receipt-bound store entry that contributes only project-specific files to one exact compiler Loaf.
struct OvenStoredProjectExtensionExecutionPlan {
    identity: String,
    artifact_root: PathBuf,
    receipt: crate::oven::OvenReceipt,
    _lease: OvenStoreLease,
}

/// One complete direct-Rustc execution contract composed from a compiler Loaf and a store-owned project extension.
///
/// Both fields hold their independent leases/locks for the entire normal command.  The composed plan is resolved
/// once before any compiler process starts, so publication/pruning cannot replace or reclaim one side mid-command.
pub(crate) struct OvenProjectExtensionExecutionPlan {
    base: OvenToolchainLoaf,
    extension: OvenStoredProjectExtensionExecutionPlan,
    artifacts: OvenRustcArtifactManifest,
    artifact_plan: OvenRustcArtifactPlan,
    registry_leaf_authority: Option<OvenRegistryLeafAuthority>,
    vocab_artifact_root: Option<PathBuf>,
    source_payload: OvenProjectExtensionPayload,
}

/// One package-owned project-extension fragment retained while a consumer uses the composed closure.
///
/// A public package may contribute a closure that overlaps another package's compiler base or registry leaves.
/// The compositor owns the one canonical copy of every byte-identical path and keeps this lease only for the
/// package-specific paths it contributes.  This prevents package count from becoming an artificial compatibility
/// limit while retaining a distinct lease for every source of executable artifacts.
struct OvenPackagedProviderFragment {
    dependency_key: String,
    receipt: crate::oven::OvenReceipt,
    identity: String,
    extension: OvenStoredProjectExtensionExecutionPlan,
    dependency_search_paths: Vec<String>,
    native_search_paths: Vec<String>,
    supporting_artifacts: Vec<OvenRustcSupportingArtifact>,
}

/// Complete direct-Rustc closure assembled from compatible public package Loafs.
///
/// Public providers are independently baked and may share compiler/runtime artifacts.  The consumer never asks
/// Cargo to resolve those packages again: it admits only exact extension entries that agree on the compiler base,
/// build intent, artifact bytes, registry source identity, and public crate identities.  Distinct compatible
/// package deltas are then materialized from their separately leased immutable roots.
pub(crate) struct OvenExtensionPackagedProviderExecutionPlan {
    base: OvenToolchainLoaf,
    fragments: Vec<OvenPackagedProviderFragment>,
    artifacts: OvenRustcArtifactManifest,
    artifact_plan: OvenRustcArtifactPlan,
    registry_leaf_authority: Option<OvenRegistryLeafAuthority>,
    vocab_artifact_root: Option<PathBuf>,
    output_guard_root: PathBuf,
}

/// One self-contained direct-plan package fragment retained while a consumer composes compatible providers.
///
/// This form is used only when a provider was baked outside an installed compiler Loaf layout. Its closure is still
/// receipt-bound and immutable; it simply cannot be partitioned against a compiler-owned base that was unavailable
/// to the explicit publisher.
struct OvenPackagedDirectProviderFragment {
    dependency_key: String,
    receipt: crate::oven::OvenReceipt,
    identity: String,
    plan: OvenStoredDirectRustcExecutionPlan,
    dependency_search_paths: Vec<String>,
    native_search_paths: Vec<String>,
    supporting_artifacts: Vec<OvenRustcSupportingArtifact>,
}

/// Complete direct-Rustc closure assembled from compatible self-contained public package Loafs.
pub(crate) struct OvenDirectPackagedProviderExecutionPlan {
    fragments: Vec<OvenPackagedDirectProviderFragment>,
    artifacts: OvenRustcArtifactManifest,
    artifact_plan: OvenRustcArtifactPlan,
    registry_leaf_authority: Option<OvenRegistryLeafAuthority>,
    vocab_artifact_root: Option<PathBuf>,
    output_guard_root: PathBuf,
}

/// One complete public-provider closure. Providers baked with the same authority compose either through one shared
/// compiler base or through compatible self-contained direct plans; a mixed authority set is refused before Rustc
/// observes an ambiguous runtime closure.
pub(crate) enum OvenPackagedProviderExecutionPlan {
    Extensions(Box<OvenExtensionPackagedProviderExecutionPlan>),
    Direct(Box<OvenDirectPackagedProviderExecutionPlan>),
}

impl OvenPackagedProviderExecutionPlan {
    /// Retain the verified private dependency paths needed to load each public provider library.
    ///
    /// Source projection deliberately keeps provider-private crates out of the consumer's direct extern set. A
    /// caller-owned provider rlib may still refer to those crates in its metadata, so Rustc needs the package
    /// fragment's exact dependency directories on its search path. Composition has already restricted these paths to
    /// directories containing digest-verified fragment artifacts; restoring them here does not expose a private crate
    /// as a direct consumer dependency.
    fn retain_fragment_dependency_search_paths(&self, plan: &mut OvenRustcArtifactPlan) {
        match self {
            Self::Extensions(packages) => retain_packaged_provider_fragment_dependency_search_paths(
                plan,
                packages.fragments.iter().map(|fragment| {
                    (
                        fragment.extension.artifact_root.as_path(),
                        fragment.dependency_search_paths.as_slice(),
                    )
                }),
            ),
            Self::Direct(packages) => retain_packaged_provider_fragment_dependency_search_paths(
                plan,
                packages.fragments.iter().map(|fragment| {
                    (
                        fragment.plan.artifact_root.as_path(),
                        fragment.dependency_search_paths.as_slice(),
                    )
                }),
            ),
        }
    }

    /// Report every selected package entry, not merely the first compatible provider.
    fn report_identity(&self) -> String {
        match self {
            Self::Extensions(packages) => {
                let extensions = packages
                    .fragments
                    .iter()
                    .map(|fragment| format!("{}:{}", fragment.dependency_key, fragment.identity))
                    .collect::<Vec<_>>()
                    .join(",");
                format!("package-loafs:{}+extensions:{extensions}", packages.base.loaf_identity)
            }
            Self::Direct(packages) => {
                let plans = packages
                    .fragments
                    .iter()
                    .map(|fragment| format!("{}:{}", fragment.dependency_key, fragment.identity))
                    .collect::<Vec<_>>()
                    .join(",");
                format!("package-loafs:direct:{plans}")
            }
        }
    }

    /// Return the complete imported closure so a library depending on several public packages exports all of it.
    fn package_entries(&self) -> Vec<OvenPackagedLibraryLoafEntry> {
        match self {
            Self::Extensions(packages) => packages
                .fragments
                .iter()
                .map(|fragment| OvenPackagedLibraryLoafEntry {
                    receipt: fragment.receipt.clone(),
                    identity: fragment.identity.clone(),
                    kind: OvenArtifactKind::ProjectPayload,
                    base_loaf_identity: Some(packages.base.loaf_identity.clone()),
                })
                .collect(),
            Self::Direct(packages) => packages
                .fragments
                .iter()
                .map(|fragment| OvenPackagedLibraryLoafEntry {
                    receipt: fragment.receipt.clone(),
                    identity: fragment.identity.clone(),
                    kind: OvenArtifactKind::DirectRustcPlan,
                    base_loaf_identity: None,
                })
                .collect(),
        }
    }

    /// Return the composed direct-Rustc plan for this packaged-provider closure.
    fn artifact_plan(&self) -> &OvenRustcArtifactPlan {
        match self {
            Self::Extensions(packages) => &packages.artifact_plan,
            Self::Direct(packages) => &packages.artifact_plan,
        }
    }

    /// Return the merged artifact manifest for this packaged-provider closure.
    fn artifacts(&self) -> &OvenRustcArtifactManifest {
        match self {
            Self::Extensions(packages) => &packages.artifacts,
            Self::Direct(packages) => &packages.artifacts,
        }
    }

    /// Return the immutable root used for caller-output containment checks.
    fn output_guard_root(&self) -> &Path {
        match self {
            Self::Extensions(packages) => &packages.output_guard_root,
            Self::Direct(packages) => &packages.output_guard_root,
        }
    }

    /// Return the root containing the complete vocabulary auxiliary closure, when one exists.
    fn vocab_artifact_root(&self) -> Option<&Path> {
        match self {
            Self::Extensions(packages) => packages.vocab_artifact_root.as_deref(),
            Self::Direct(packages) => packages.vocab_artifact_root.as_deref(),
        }
    }

    /// Return the registry-leaf authority assembled for the selected package fragments.
    fn registry_leaf_authority(&self) -> Option<OvenRegistryLeafAuthority> {
        match self {
            Self::Extensions(packages) => packages.registry_leaf_authority.clone(),
            Self::Direct(packages) => packages.registry_leaf_authority.clone(),
        }
    }
}

/// Restore package-fragment dependency paths under their owning roots, then normalize the resulting search list.
fn retain_packaged_provider_fragment_dependency_search_paths<'a>(
    plan: &mut OvenRustcArtifactPlan,
    fragments: impl IntoIterator<Item = (&'a Path, &'a [String])>,
) {
    for (artifact_root, dependency_search_paths) in fragments {
        plan.dependency_search_paths.extend(
            dependency_search_paths
                .iter()
                .map(|dependency_search_path| artifact_root.join(dependency_search_path)),
        );
    }
    plan.dependency_search_paths.sort();
    plan.dependency_search_paths.dedup();
}

/// Resolve a receipt-compatible direct-Rustc payload while retaining the execution lease acquired during matching.
///
/// A normal Oven consumer never converts an unleased manifest header into a later identity lookup: policy pruning may
/// legitimately reclaim that inactive entry between those steps. The store's matching selector verifies the payload
/// and acquires its active lease atomically, while integrity and authorization failures still fail closed.
pub(crate) fn select_receipt_direct_rustc_execution_plan(
    store: &OvenStore,
    receipt: &crate::oven::OvenReceipt,
) -> CliResult<Option<OvenStoredDirectRustcExecutionPlan>> {
    let Some(selected) = select_direct_rustc_plan_for_execution(store, receipt).map_err(oven_rustc_error)? else {
        return Ok(None);
    };
    let (stored_manifest, artifact_root, payload, lease) = selected.into_parts();
    let plan_identity = &stored_manifest.identity;
    if stored_manifest.kind != OvenArtifactKind::DirectRustcPlan
        || stored_manifest.build_unit_identity != receipt.build_unit_identity
        || stored_manifest.intent != receipt.intent
    {
        return Err(CliError::failure(
            "selected Oven store entry is not the receipt-bound direct-Rustc plan".to_string(),
        ));
    }
    let artifacts = serde_json::from_slice::<OvenRustcArtifactManifest>(&payload).map_err(|error| {
        CliError::failure(format!(
            "selected Oven direct-Rustc plan has an invalid payload: {error}"
        ))
    })?;
    let artifact_plan = artifacts
        .materialize_trusted_store(&artifact_root, &receipt.intent)
        .map_err(oven_rustc_error)?;
    Ok(Some(OvenStoredDirectRustcExecutionPlan {
        identity: plan_identity.clone(),
        artifacts,
        artifact_root,
        artifact_plan,
        _lease: lease,
    }))
}

/// Select one exact direct-plan entry transported by a public package.
///
/// Unlike ordinary receipt selection, package composition carries an immutable entry identity. Requiring that identity
/// prevents a compatible-but-different direct plan from replacing the provider's sealed Rust ABI closure after its
/// package crossed the explicit bake boundary.
fn select_packaged_direct_rustc_execution_plan(
    store: &OvenStore,
    receipt: &crate::oven::OvenReceipt,
    required_identity: &str,
) -> CliResult<Option<OvenStoredDirectRustcExecutionPlan>> {
    receipt
        .verify_identity()
        .map_err(|error| CliError::failure(format!("invalid packaged Oven receipt: {error}")))?;
    let mut selected = store
        .select_payloads_matching_for_execution(|manifest| {
            manifest.identity == required_identity
                && manifest.kind == OvenArtifactKind::DirectRustcPlan
                && manifest.receipt_identity == receipt.identity
                && manifest.build_unit_identity == receipt.build_unit_identity
                && manifest.intent == receipt.intent
        })
        .map_err(|error| CliError::failure(format!("failed to select packaged Oven direct-plan Loaf: {error}")))?;
    if selected.len() > 1 {
        return Err(CliError::failure(format!(
            "Oven Alpha found multiple imported direct-plan Loafs for sealed package entry `{required_identity}`"
        )));
    }
    let Some(selected) = selected.pop() else {
        return Ok(None);
    };
    let (manifest, artifact_root, payload, lease) = selected.into_parts();
    let artifacts = serde_json::from_slice::<OvenRustcArtifactManifest>(&payload).map_err(|error| {
        CliError::failure(format!(
            "selected Oven direct-plan Loaf `{}` has an invalid payload: {error}",
            manifest.identity
        ))
    })?;
    let artifact_plan = artifacts
        .materialize_trusted_store(&artifact_root, &receipt.intent)
        .map_err(oven_rustc_error)?;
    Ok(Some(OvenStoredDirectRustcExecutionPlan {
        identity: manifest.identity,
        artifacts,
        artifact_root,
        artifact_plan,
        _lease: lease,
    }))
}

/// Select one receipt-bound project extension and reconstitute its exact base-plus-extension execution set.
///
/// The extension payload names the content address of the standard-library Loaf it was partitioned against.  A
/// compatible substitute is deliberately not accepted: Rust metadata and a `-L` path are paired artifacts, so a
/// newer Loaf with matching crate names could still be semantically different.  Both roots remain leased/locked in
/// the returned selection and are composed before any direct-Rustc command starts.
fn select_receipt_project_extension_execution_plan(
    store: &OvenStore,
    receipt: &crate::oven::OvenReceipt,
    required_identity: Option<&str>,
) -> CliResult<Option<OvenProjectExtensionExecutionPlan>> {
    let selected = store
        .select_payloads_matching_for_execution(|manifest| {
            manifest.kind == OvenArtifactKind::ProjectPayload
                && manifest.receipt_identity == receipt.identity
                && manifest.build_unit_identity == receipt.build_unit_identity
                && manifest.intent == receipt.intent
                && required_identity.is_none_or(|identity| manifest.identity == identity)
        })
        .map_err(|error| CliError::failure(error.to_string()))?;
    let mut selected = selected
        .into_iter()
        .filter_map(|candidate| {
            let payload = serde_json::from_slice::<OvenProjectExtensionPayload>(&candidate.payload).map_err(|error| {
                CliError::failure(format!(
                    "selected Oven project extension Loaf `{}` has an invalid payload: {error}",
                    candidate.manifest.identity
                ))
            });
            match payload {
                Ok(payload) if payload.schema_version == OVEN_PROJECT_EXTENSION_PAYLOAD_SCHEMA_VERSION => {
                    Some(Ok((candidate, payload)))
                }
                Ok(_) => None,
                Err(error) => Some(Err(error)),
            }
        })
        .collect::<CliResult<Vec<_>>>()?;
    if selected.is_empty() {
        tracing::debug!(
            "no stored project extension matches receipt {} (build unit {})",
            receipt.identity,
            receipt.build_unit_identity
        );
        return Ok(None);
    }
    if selected.len() != 1 {
        let identities = selected
            .iter()
            .map(|(candidate, _)| candidate.manifest.identity.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        let first_payload = &selected[0].1;
        if selected.iter().all(|(_, payload)| payload == first_payload) {
            // A project extension contains only the receipt-bound dependency delta, never the caller's generated root.
            // Independent projects can therefore publish byte-identical extensions concurrently. They are
            // interchangeable; choose the stable content address rather than rejecting a correct shared closure as
            // ambiguous.
            selected.sort_by(|(left, _), (right, _)| left.manifest.identity.cmp(&right.manifest.identity));
        } else {
            return Err(CliError::failure(format!(
                "multiple distinct receipt-compatible Oven project extension Loafs are available: {identities}"
            )));
        }
    }
    let (selected, extension_payload) = selected.remove(0);
    project_extension_execution_plan_from_selected(selected, extension_payload, receipt).map(Some)
}

/// Reconstitute an exact base-plus-extension plan from one already leased store constituent.
///
/// Project inspection authorities batch-select every named constituent once. Native test setup passes that exact
/// leased value here instead of scanning the store again or accepting another compatibility match.
fn project_extension_execution_plan_from_selected(
    selected: crate::oven::store::OvenStoreExecutionPayload,
    extension_payload: OvenProjectExtensionPayload,
    receipt: &crate::oven::OvenReceipt,
) -> CliResult<OvenProjectExtensionExecutionPlan> {
    let (manifest, artifact_root, _payload, lease) = selected.into_parts();
    let base = resolve_compiler_owned_loaf_by_identity(receipt, &extension_payload.base_loaf_identity)
        .map_err(|error| CliError::failure(error.to_string()))?
        .ok_or_else(|| {
            CliError::failure(format!(
                "selected Oven project extension Loaf requires base `{}`, but that exact installed standard-library Loaf is unavailable; rebake the project for this Incan release",
                extension_payload.base_loaf_identity
            ))
        })?;
    let partition = validate_project_extension_payload_against_base(
        &extension_payload,
        &base.loaf_identity,
        &base.loaf_build_unit_identity,
        &base.artifacts,
    )
    .map_err(oven_rustc_error)?;
    let base_fragment = extension_payload
        .complete_plan
        .artifact_fragment(&partition.base_paths)
        .map_err(oven_rustc_error)?;
    let extension_fragment = extension_payload
        .complete_plan
        .artifact_fragment(&partition.extension_paths)
        .map_err(oven_rustc_error)?;
    let base_artifacts = base_fragment.composition_artifacts().map_err(oven_rustc_error)?;
    let extension_artifacts = extension_fragment.composition_artifacts().map_err(oven_rustc_error)?;
    let roots = [
        OvenTrustedRustcArtifactRoot {
            artifact_root: &base.artifact_root,
            dependency_search_paths: &base_fragment.dependency_search_paths,
            native_search_paths: &base_fragment.native_search_paths,
            supporting_artifacts: &base_artifacts,
        },
        OvenTrustedRustcArtifactRoot {
            artifact_root: &artifact_root,
            dependency_search_paths: &extension_fragment.dependency_search_paths,
            native_search_paths: &extension_fragment.native_search_paths,
            supporting_artifacts: &extension_artifacts,
        },
    ];
    let artifact_plan = extension_payload
        .complete_plan
        .materialize_trusted_store_composed(&roots, &receipt.intent)
        .map_err(oven_rustc_error)?;
    let registry_leaf_entries = extension_payload
        .complete_plan
        .registry_leaves
        .iter()
        .map(|leaf| {
            let artifact_root = if partition.base_paths.contains(&leaf.artifact.relative_path) {
                base.artifact_root.clone()
            } else if partition.extension_paths.contains(&leaf.artifact.relative_path) {
                artifact_root.clone()
            } else {
                return Err(CliError::failure(format!(
                    "selected Oven project extension Loaf has a registry leaf outside its declared base and extension fragments: {}",
                    leaf.artifact.relative_path
                )));
            };
            Ok((artifact_root, leaf.clone()))
        })
        .collect::<CliResult<Vec<_>>>()?;
    let registry_leaf_authority = OvenRegistryLeafAuthority::from_composed_plan(registry_leaf_entries, &artifact_plan);
    let vocab_paths = extension_payload
        .complete_plan
        .vocab_auxiliary_targets
        .iter()
        .flat_map(|target| target.externs.iter().map(|artifact| artifact.relative_path.as_str()))
        .collect::<BTreeSet<_>>();
    let vocab_artifact_root =
        if vocab_paths.is_empty() || vocab_paths.iter().all(|path| partition.base_paths.contains(*path)) {
            Some(base.artifact_root.clone())
        } else if vocab_paths.iter().all(|path| partition.extension_paths.contains(*path)) {
            Some(artifact_root.clone())
        } else {
            None
        };
    Ok(OvenProjectExtensionExecutionPlan {
        base,
        extension: OvenStoredProjectExtensionExecutionPlan {
            identity: manifest.identity,
            artifact_root,
            receipt: receipt.clone(),
            _lease: lease,
        },
        artifacts: extension_payload.complete_plan.clone(),
        artifact_plan,
        registry_leaf_authority,
        vocab_artifact_root,
        source_payload: extension_payload,
    })
}

/// Build the role-bearing test dependency plan from one exact constituent selected with its authority.
pub(crate) fn project_test_dependency_plan_from_constituent(
    selected: crate::oven::store::OvenStoreExecutionPayload,
    receipt: &crate::oven::OvenReceipt,
) -> CliResult<OvenDirectRustcPlanSelection> {
    if !project_inspection_constituent_matches_receipt(&selected.manifest, selected.manifest.kind, receipt) {
        return Err(CliError::failure(
            "project inspection test dependency constituent changed kind, receipt, build unit, or intent",
        ));
    }
    match selected.manifest.kind {
        OvenArtifactKind::DirectRustcPlan => {
            let (manifest, artifact_root, payload, lease) = selected.into_parts();
            let artifacts = serde_json::from_slice::<OvenRustcArtifactManifest>(&payload).map_err(|error| {
                CliError::failure(format!(
                    "project inspection test dependency constituent `{}` has an invalid direct-plan payload: {error}",
                    manifest.identity
                ))
            })?;
            let artifact_plan = artifacts
                .materialize_trusted_store(&artifact_root, &receipt.intent)
                .map_err(oven_rustc_error)?;
            Ok(OvenDirectRustcPlanSelection::Stored(Box::new(
                OvenStoredDirectRustcExecutionPlan {
                    identity: manifest.identity,
                    artifacts,
                    artifact_root,
                    artifact_plan,
                    _lease: lease,
                },
            )))
        }
        OvenArtifactKind::ProjectPayload => {
            let payload = serde_json::from_slice::<OvenProjectExtensionPayload>(&selected.payload).map_err(|error| {
                CliError::failure(format!(
                    "project inspection test dependency constituent `{}` has an invalid project-extension payload: {error}",
                    selected.manifest.identity
                ))
            })?;
            if payload.schema_version != OVEN_PROJECT_EXTENSION_PAYLOAD_SCHEMA_VERSION {
                return Err(CliError::failure(
                    "project inspection test dependency constituent uses an incompatible project-extension schema",
                ));
            }
            project_extension_execution_plan_from_selected(selected, payload, receipt)
                .map(|plan| OvenDirectRustcPlanSelection::ProjectExtension(Box::new(plan)))
        }
        _ => Err(CliError::failure(
            "project inspection test dependency constituent changed kind, receipt, build unit, or intent",
        )),
    }
}

/// Select one receipt-bound project-extension publication from the bounded store.
///
/// An extension is the unambiguous representation of a caller project: it records both its complete Cargo-published
/// closure and the exact installed standard-library base it deliberately partitions from.  A plain direct-Rustc
/// plan can instead be a broad compiler Loaf with coincidentally matching build-unit inputs, so an explicit project
/// bake must never treat one as evidence that its caller closure has already been published.
fn select_published_project_extension_plan(
    store: &OvenStore,
    receipt: &crate::oven::OvenReceipt,
    materialization: OvenToolchainMaterialization,
) -> CliResult<Option<OvenDirectRustcPlanPreparation>> {
    Ok(
        select_receipt_project_extension_execution_plan(store, receipt, None)?.map(|selected| {
            OvenDirectRustcPlanPreparation {
                plan_selection: OvenDirectRustcPlanSelection::ProjectExtension(Box::new(selected)),
                materialization,
                cargo_process_started: false,
            }
        }),
    )
}

/// Select either receipt-bound project publication form from the bounded store.
///
/// Current project Loafs are extensions and win over a plain direct plan. Older self-contained project Loafs can
/// still run during the migration, but only after the unambiguous extension form was absent. This preserves valid
/// older local work while ensuring an installed compiler Loaf can never shadow a newly baked caller closure.
fn select_published_project_plan(
    store: &OvenStore,
    receipt: &crate::oven::OvenReceipt,
    materialization: OvenToolchainMaterialization,
) -> CliResult<Option<OvenDirectRustcPlanPreparation>> {
    if let Some(selected) = select_published_project_extension_plan(store, receipt, materialization)? {
        return Ok(Some(selected));
    }
    Ok(
        select_receipt_direct_rustc_execution_plan(store, receipt)?.map(|selected| OvenDirectRustcPlanPreparation {
            plan_selection: OvenDirectRustcPlanSelection::Stored(Box::new(selected)),
            materialization,
            cargo_process_started: false,
        }),
    )
}

/// Select an imported public-provider Loaf as the consumer's complete Rust ABI foundation.
///
/// A provider such as IncQL was compiled against its own sealed `incan_stdlib`, DataFusion, and transitive Rust
/// artifacts. Attaching only its top-level rlib to an unrelated consumer plan would permit Rust to discover two ABI
/// closures. This selector instead lets the consumer compile against the exact package closures after the explicit
/// consumer bake imported them into the consumer's bounded store. It is deliberately a closure compositor rather
/// than a first-provider shortcut: independent packages may contribute one ABI-compatible collection of Loafs.
fn select_packaged_provider_plan(
    store: &OvenStore,
    checked_profiles: &[CheckedPackagedProviderProfile],
    profile: &str,
    consumer_receipt: &crate::oven::OvenReceipt,
) -> CliResult<Option<OvenDirectRustcPlanSelection>> {
    let mut candidates = Vec::new();
    for checked in checked_profiles.iter().filter(|checked| checked.profile == profile) {
        let package_profile = &checked.package;
        if package_profile.entries.is_empty() {
            // A provider that uses only the compiler-shipped base can safely use the consumer's ordinary base
            // selection. There is no package delta to compose here.
            continue;
        }
        candidates.push((checked.dependency_key.clone(), package_profile.clone()));
    }
    if candidates.is_empty() {
        return Ok(None);
    }
    let mut extension_selected = Vec::new();
    let mut direct_selected = Vec::new();
    for (dependency_key, package_profile) in candidates {
        if package_profile.receipt.intent != consumer_receipt.intent {
            return Err(CliError::failure(format!(
                "Oven Alpha cannot compose pub::{dependency_key} package Loaf with this consumer: the sealed provider intent differs from the consumer intent; rebake the provider for the selected target, toolchain, profile, and feature set"
            )));
        }
        for entry in package_profile.entries {
            match entry.kind {
                OvenArtifactKind::ProjectPayload => {
                    let plan = select_receipt_project_extension_execution_plan(
                        store,
                        &entry.receipt,
                        Some(&entry.identity),
                    )?
                    .ok_or_else(|| {
                        CliError::failure(format!(
                            "Oven Alpha has no imported package Loaf `{}` for pub::{dependency_key}; run `incan oven bake --project .` in this consumer to import the already baked provider closure",
                            entry.identity
                        ))
                    })?;
                    if let Some(expected_base) = entry.base_loaf_identity.as_deref()
                        && plan.base.loaf_identity != expected_base
                    {
                        return Err(CliError::failure(format!(
                            "Oven Alpha refuses pub::{dependency_key} package Loaf: selected base `{}` differs from sealed package base `{expected_base}`",
                            plan.base.loaf_identity
                        )));
                    }
                    extension_selected.push((dependency_key.clone(), entry, plan));
                }
                OvenArtifactKind::DirectRustcPlan => {
                    if entry.base_loaf_identity.is_some() {
                        return Err(CliError::failure(format!(
                            "Oven Alpha refuses pub::{dependency_key} package Loaf `{}` because a self-contained direct plan cannot name a compiler base",
                            entry.identity
                        )));
                    }
                    let plan = select_packaged_direct_rustc_execution_plan(
                        store,
                        &entry.receipt,
                        &entry.identity,
                    )?
                    .ok_or_else(|| {
                        CliError::failure(format!(
                            "Oven Alpha has no imported package Loaf `{}` for pub::{dependency_key}; run `incan oven bake --project .` in this consumer to import the already baked provider closure",
                            entry.identity
                        ))
                    })?;
                    direct_selected.push((dependency_key.clone(), entry, plan));
                }
                _ => {
                    return Err(CliError::failure(format!(
                        "Oven Alpha cannot compose pub::{dependency_key} package Loaf `{}` because its stored role is not a direct Rust closure",
                        entry.identity
                    )));
                }
            }
        }
    }
    if !extension_selected.is_empty() && !direct_selected.is_empty() {
        return Err(CliError::failure(
            "Oven Alpha cannot compose package Loafs that mix compiler-base extensions with self-contained direct plans; rebake the participating providers with one installed Incan release so their Rust ABI closure authority is uniform",
        ));
    }
    let composed = if !extension_selected.is_empty() {
        compose_packaged_provider_plan(extension_selected, &consumer_receipt.intent)?
    } else {
        compose_direct_packaged_provider_plan(direct_selected, &consumer_receipt.intent)?
    };
    Ok(Some(OvenDirectRustcPlanSelection::PackagedProvider(Box::new(composed))))
}

/// Build one direct-Rustc execution plan from every ABI-compatible public package Loaf.
///
/// Each input has already passed receipt selection and retains its lease.  Composition is strictly byte based:
/// duplicate artifact paths are accepted only when their digest is identical; duplicate public extern names or
/// registry package identities must also agree on their sealed artifact and feature facts.  Those are genuine ABI
/// conflicts, unlike merely having more than one public package.
fn compose_packaged_provider_plan(
    selected: Vec<(String, OvenPackagedLibraryLoafEntry, OvenProjectExtensionExecutionPlan)>,
    expected_intent: &crate::oven::OvenBuildIntent,
) -> CliResult<OvenPackagedProviderExecutionPlan> {
    let mut selected = selected.into_iter();
    let (first_dependency, first_entry, first) = selected
        .next()
        .ok_or_else(|| CliError::failure("package Loaf composition requires at least one selected provider"))?;
    let OvenProjectExtensionExecutionPlan {
        base,
        extension,
        artifacts,
        ..
    } = first;
    if artifacts.intent != *expected_intent {
        return Err(CliError::failure(
            "Oven Alpha refuses package Loaf composition because the first provider intent differs from the consumer",
        ));
    }
    let base_identity = base.loaf_identity.clone();
    let base_artifacts = base.artifacts.clone();
    let mut inputs = vec![(first_dependency, first_entry.receipt, extension, artifacts)];
    for (dependency_key, entry, plan) in selected {
        if plan.base.loaf_identity != base_identity {
            return Err(CliError::failure(format!(
                "Oven Alpha cannot compose pub::{dependency_key}: its package Loaf requires compiler base `{}`, while the selected package collection requires `{base_identity}`; rebake the packages with one Incan release",
                plan.base.loaf_identity
            )));
        }
        if plan.artifacts.intent != *expected_intent {
            return Err(CliError::failure(format!(
                "Oven Alpha cannot compose pub::{dependency_key}: its sealed intent differs from this consumer; rebake it for the selected target, toolchain, profile, and feature set"
            )));
        }
        inputs.push((dependency_key, entry.receipt, plan.extension, plan.artifacts));
    }
    let manifest_inputs = inputs
        .iter()
        .map(|(dependency_key, _, _, artifacts)| (dependency_key.as_str(), artifacts))
        .collect::<Vec<_>>();
    let artifacts = merge_packaged_provider_artifact_manifests_with_release_base(
        &manifest_inputs,
        &base_artifacts,
        expected_intent,
    )?;
    let base_paths = artifacts
        .partition_against_base(&base_artifacts)
        .map_err(oven_rustc_error)?
        .base_paths;
    let base_fragment = artifacts.artifact_fragment(&base_paths).map_err(oven_rustc_error)?;
    let base_supporting_artifacts = base_fragment.composition_artifacts().map_err(oven_rustc_error)?;
    let mut owned_paths = base_supporting_artifacts
        .iter()
        .map(|artifact| artifact.relative_path.clone())
        .collect::<BTreeSet<_>>();
    let mut fragments = Vec::new();
    for (dependency_key, receipt, extension, provider_artifacts) in inputs {
        let partition = provider_artifacts
            .partition_against_base(&base_artifacts)
            .map_err(oven_rustc_error)?;
        let extension_fragment = provider_artifacts
            .artifact_fragment(&partition.extension_paths)
            .map_err(oven_rustc_error)?;
        let mut supporting_artifacts = extension_fragment
            .composition_artifacts()
            .map_err(oven_rustc_error)?
            .into_iter()
            .filter(|artifact| owned_paths.insert(artifact.relative_path.clone()))
            .collect::<Vec<_>>();
        supporting_artifacts.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
        let retains_artifact_below = |search_path: &str| {
            supporting_artifacts
                .iter()
                .any(|artifact| Path::new(&artifact.relative_path).starts_with(Path::new(search_path)))
        };
        let mut dependency_search_paths = extension_fragment
            .dependency_search_paths
            .into_iter()
            .filter(|path| retains_artifact_below(path))
            .collect::<Vec<_>>();
        dependency_search_paths.sort();
        dependency_search_paths.dedup();
        let mut native_search_paths = extension_fragment
            .native_search_paths
            .into_iter()
            .filter(|path| retains_artifact_below(path))
            .collect::<Vec<_>>();
        native_search_paths.sort();
        native_search_paths.dedup();
        fragments.push(OvenPackagedProviderFragment {
            dependency_key,
            receipt,
            identity: extension.identity.clone(),
            extension,
            dependency_search_paths,
            native_search_paths,
            supporting_artifacts,
        });
    }
    let output_guard_root = fragments
        .first()
        .map(|fragment| fragment.extension.artifact_root.clone())
        .ok_or_else(|| CliError::failure("package Loaf composition lost its provider artifact root"))?;
    let mut composed = OvenExtensionPackagedProviderExecutionPlan {
        base,
        fragments,
        artifacts,
        artifact_plan: OvenRustcArtifactPlan {
            dependency_search_paths: Vec::new(),
            native_search_paths: Vec::new(),
            externs: Vec::new(),
            compile_environment: BTreeMap::new(),
            caller_owned_library_digests: BTreeMap::new(),
        },
        registry_leaf_authority: None,
        vocab_artifact_root: None,
        output_guard_root,
    };
    let mut roots = vec![OvenTrustedRustcArtifactRoot {
        artifact_root: &composed.base.artifact_root,
        dependency_search_paths: &base_fragment.dependency_search_paths,
        native_search_paths: &base_fragment.native_search_paths,
        supporting_artifacts: &base_supporting_artifacts,
    }];
    roots.extend(
        composed
            .fragments
            .iter()
            .filter(|fragment| !fragment.supporting_artifacts.is_empty())
            .map(|fragment| OvenTrustedRustcArtifactRoot {
                artifact_root: &fragment.extension.artifact_root,
                dependency_search_paths: &fragment.dependency_search_paths,
                native_search_paths: &fragment.native_search_paths,
                supporting_artifacts: &fragment.supporting_artifacts,
            }),
    );
    composed.artifact_plan = composed
        .artifacts
        .materialize_trusted_store_composed(&roots, expected_intent)
        .map_err(oven_rustc_error)?;
    for fragment in &composed.fragments {
        if composed
            .artifact_plan
            .caller_owned_library_digests
            .insert(
                format!("package-loaf:{}:{}", fragment.dependency_key, fragment.identity),
                fragment.identity.clone(),
            )
            .is_some()
        {
            return Err(CliError::failure(format!(
                "Oven Alpha package Loaf composition found duplicate entry `{}` for pub::{}",
                fragment.identity, fragment.dependency_key
            )));
        }
    }
    let mut artifact_roots = base_supporting_artifacts
        .iter()
        .map(|artifact| (artifact.relative_path.as_str(), composed.base.artifact_root.clone()))
        .collect::<BTreeMap<_, _>>();
    for fragment in &composed.fragments {
        for artifact in &fragment.supporting_artifacts {
            artifact_roots.insert(
                artifact.relative_path.as_str(),
                fragment.extension.artifact_root.clone(),
            );
        }
    }
    let registry_leaf_entries = composed
        .artifacts
        .registry_leaves
        .iter()
        .map(|leaf| {
            let root = artifact_roots
                .get(leaf.artifact.relative_path.as_str())
                .ok_or_else(|| {
                    CliError::failure(format!(
                        "Oven Alpha package Loaf composition omitted registry leaf `{}` `{}`",
                        leaf.package, leaf.version
                    ))
                })?;
            Ok((root.clone(), leaf.clone()))
        })
        .collect::<CliResult<Vec<_>>>()?;
    composed.registry_leaf_authority =
        OvenRegistryLeafAuthority::from_composed_plan(registry_leaf_entries, &composed.artifact_plan);
    let base_vocab_paths = base_artifacts
        .vocab_auxiliary_targets
        .iter()
        .flat_map(|target| target.externs.iter().map(|artifact| artifact.relative_path.as_str()))
        .collect::<BTreeSet<_>>();
    if base_vocab_paths.iter().all(|path| base_paths.contains(*path)) {
        composed.vocab_artifact_root = Some(composed.base.artifact_root.clone());
    } else {
        return Err(CliError::failure(
            "Oven Alpha package Loaf composition found vocabulary support outside its exact compiler base",
        ));
    }
    Ok(OvenPackagedProviderExecutionPlan::Extensions(Box::new(composed)))
}

/// Compose independently baked self-contained public-provider closures.
///
/// An explicit provider bake may legitimately have no installed compiler Loaf to partition against. Its direct plan
/// consequently contains the complete sealed Rust closure. Consumers still must not resolve that closure again: this
/// compositor verifies every package entry by its receipt and immutable identity, accepts only byte-identical overlap,
/// and materializes the union directly from the separately leased package roots.
fn compose_direct_packaged_provider_plan(
    selected: Vec<(String, OvenPackagedLibraryLoafEntry, OvenStoredDirectRustcExecutionPlan)>,
    expected_intent: &crate::oven::OvenBuildIntent,
) -> CliResult<OvenPackagedProviderExecutionPlan> {
    if selected.is_empty() {
        return Err(CliError::failure(
            "self-contained package Loaf composition requires at least one selected provider",
        ));
    }
    let manifest_inputs = selected
        .iter()
        .map(|(dependency_key, _, plan)| (dependency_key.as_str(), &plan.artifacts))
        .collect::<Vec<_>>();
    let artifacts = merge_packaged_provider_artifact_manifests(&manifest_inputs, expected_intent)?;
    let mut owned_paths = BTreeSet::new();
    let mut fragments = Vec::new();
    for (dependency_key, entry, plan) in selected {
        if plan.artifacts.intent != *expected_intent {
            return Err(CliError::failure(format!(
                "Oven Alpha cannot compose pub::{dependency_key}: its sealed direct-plan intent differs from this consumer; rebake it for the selected target, toolchain, profile, and feature set"
            )));
        }
        let mut supporting_artifacts = plan
            .artifacts
            .composition_artifacts()
            .map_err(oven_rustc_error)?
            .into_iter()
            .filter(|artifact| owned_paths.insert(artifact.relative_path.clone()))
            .collect::<Vec<_>>();
        supporting_artifacts.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
        let retains_artifact_below = |search_path: &str| {
            supporting_artifacts
                .iter()
                .any(|artifact| Path::new(&artifact.relative_path).starts_with(Path::new(search_path)))
        };
        let mut dependency_search_paths = plan
            .artifacts
            .dependency_search_paths
            .iter()
            .filter(|path| retains_artifact_below(path))
            .cloned()
            .collect::<Vec<_>>();
        dependency_search_paths.sort();
        dependency_search_paths.dedup();
        let mut native_search_paths = plan
            .artifacts
            .native_search_paths
            .iter()
            .filter(|path| retains_artifact_below(path))
            .cloned()
            .collect::<Vec<_>>();
        native_search_paths.sort();
        native_search_paths.dedup();
        fragments.push(OvenPackagedDirectProviderFragment {
            dependency_key,
            receipt: entry.receipt,
            identity: plan.identity.clone(),
            plan,
            dependency_search_paths,
            native_search_paths,
            supporting_artifacts,
        });
    }
    let output_guard_root = fragments
        .first()
        .map(|fragment| fragment.plan.artifact_root.clone())
        .ok_or_else(|| CliError::failure("package Loaf composition lost its provider artifact root"))?;
    let mut composed = OvenDirectPackagedProviderExecutionPlan {
        fragments,
        artifacts,
        artifact_plan: OvenRustcArtifactPlan {
            dependency_search_paths: Vec::new(),
            native_search_paths: Vec::new(),
            externs: Vec::new(),
            compile_environment: BTreeMap::new(),
            caller_owned_library_digests: BTreeMap::new(),
        },
        registry_leaf_authority: None,
        vocab_artifact_root: None,
        output_guard_root,
    };
    let roots = composed
        .fragments
        .iter()
        .filter(|fragment| !fragment.supporting_artifacts.is_empty())
        .map(|fragment| OvenTrustedRustcArtifactRoot {
            artifact_root: &fragment.plan.artifact_root,
            dependency_search_paths: &fragment.dependency_search_paths,
            native_search_paths: &fragment.native_search_paths,
            supporting_artifacts: &fragment.supporting_artifacts,
        })
        .collect::<Vec<_>>();
    composed.artifact_plan = composed
        .artifacts
        .materialize_trusted_store_composed(&roots, expected_intent)
        .map_err(oven_rustc_error)?;
    for fragment in &composed.fragments {
        if composed
            .artifact_plan
            .caller_owned_library_digests
            .insert(
                format!("package-loaf:{}:{}", fragment.dependency_key, fragment.identity),
                fragment.identity.clone(),
            )
            .is_some()
        {
            return Err(CliError::failure(format!(
                "Oven Alpha package Loaf composition found duplicate entry `{}` for pub::{}",
                fragment.identity, fragment.dependency_key
            )));
        }
    }
    let mut artifact_roots = BTreeMap::new();
    for fragment in &composed.fragments {
        for artifact in &fragment.supporting_artifacts {
            artifact_roots.insert(artifact.relative_path.as_str(), fragment.plan.artifact_root.clone());
        }
    }
    let registry_leaf_entries = composed
        .artifacts
        .registry_leaves
        .iter()
        .map(|leaf| {
            let root = artifact_roots
                .get(leaf.artifact.relative_path.as_str())
                .ok_or_else(|| {
                    CliError::failure(format!(
                        "Oven Alpha package Loaf composition omitted registry leaf `{}` `{}`",
                        leaf.package, leaf.version
                    ))
                })?;
            Ok((root.clone(), leaf.clone()))
        })
        .collect::<CliResult<Vec<_>>>()?;
    composed.registry_leaf_authority =
        OvenRegistryLeafAuthority::from_composed_plan(registry_leaf_entries, &composed.artifact_plan);
    let vocab_paths = composed
        .artifacts
        .vocab_auxiliary_targets
        .iter()
        .flat_map(|target| target.externs.iter().map(|artifact| artifact.relative_path.as_str()))
        .collect::<BTreeSet<_>>();
    if vocab_paths.is_empty() {
        composed.vocab_artifact_root = composed
            .fragments
            .first()
            .map(|fragment| fragment.plan.artifact_root.clone());
    } else if let Some(fragment) = composed.fragments.iter().find(|fragment| {
        let paths = fragment
            .supporting_artifacts
            .iter()
            .map(|artifact| artifact.relative_path.as_str())
            .collect::<BTreeSet<_>>();
        vocab_paths.iter().all(|path| paths.contains(path))
    }) {
        composed.vocab_artifact_root = Some(fragment.plan.artifact_root.clone());
    } else {
        return Err(CliError::failure(
            "Oven Alpha package Loaf composition split one compiler-owned vocabulary closure across self-contained provider Loafs",
        ));
    }
    Ok(OvenPackagedProviderExecutionPlan::Direct(Box::new(composed)))
}

/// Project an exact release-base Loaf into a package collection without replacing provider-owned ABI roots.
///
/// A project extension records only the direct externs its producer source used. A different consumer may use any
/// public root shipped by the same release, so package composition must retain the exact base's generated-root map
/// and artifact closure. When a provider already owns a same-named direct extern, that provider artifact remains the
/// consumer's direct root and the base variant becomes supporting metadata for crates compiled against it. The
/// ordinary compositor still rejects path, digest, registry, vocabulary, and compile-environment conflicts.
fn release_base_consumer_overlay(
    base: &OvenRustcArtifactManifest,
    providers: &[&OvenRustcArtifactManifest],
    compiler_runtime_crate_names: &BTreeSet<String>,
) -> CliResult<OvenRustcArtifactManifest> {
    base.validate_shape(&base.intent).map_err(oven_rustc_error)?;
    let mut provider_extern_names = BTreeSet::new();
    for provider in providers {
        provider.validate_shape(&base.intent).map_err(oven_rustc_error)?;
        provider
            .validate_release_cohort_from_base(base)
            .map_err(oven_rustc_error)?;
        provider_extern_names.extend(provider.externs.iter().map(|artifact| artifact.crate_name.as_str()));
    }
    let mut overlay = base.clone();
    let base_extern_paths = base
        .externs
        .iter()
        .map(|artifact| artifact.relative_path.as_str())
        .collect::<BTreeSet<_>>();
    overlay.supporting_artifacts = base
        .release_execution_artifacts()
        .map_err(oven_rustc_error)?
        .into_iter()
        .filter(|artifact| !base_extern_paths.contains(artifact.relative_path.as_str()))
        .collect();
    let mut retained_externs = Vec::new();
    for artifact in std::mem::take(&mut overlay.externs) {
        if compiler_runtime_crate_names.contains(&artifact.crate_name)
            && !provider_extern_names.contains(artifact.crate_name.as_str())
        {
            retained_externs.push(artifact);
        } else {
            overlay.supporting_artifacts.push(OvenRustcSupportingArtifact {
                relative_path: artifact.relative_path,
                digest: artifact.digest,
            });
        }
    }
    overlay.externs = retained_externs;
    for crate_names in overlay.entrypoint_externs.values_mut() {
        crate_names.retain(|crate_name| {
            compiler_runtime_crate_names.contains(crate_name) && !provider_extern_names.contains(crate_name.as_str())
        });
    }
    overlay.registry_leaves.clear();
    overlay.registry_sources.clear();
    overlay.compile_environment.clear();
    overlay
        .supporting_artifacts
        .sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
    overlay.validate_shape(&base.intent).map_err(oven_rustc_error)?;
    Ok(overlay)
}

/// Merge provider extensions with the exact release base and restore the base's portable consumer-root map.
fn merge_packaged_provider_artifact_manifests_with_release_base(
    providers: &[(&str, &OvenRustcArtifactManifest)],
    base: &OvenRustcArtifactManifest,
    expected_intent: &crate::oven::OvenBuildIntent,
) -> CliResult<OvenRustcArtifactManifest> {
    let compiler_runtime_crate_names = base.compiler_runtime_crate_names().map_err(oven_rustc_error)?;
    let base_overlay = release_base_consumer_overlay(
        base,
        &providers.iter().map(|(_, artifacts)| *artifacts).collect::<Vec<_>>(),
        &compiler_runtime_crate_names,
    )?;
    let mut inputs = providers.to_vec();
    inputs.push(("Incan release base", &base_overlay));
    let mut composed = merge_packaged_provider_artifact_manifests(&inputs, expected_intent)?;
    for (source_key, base_names) in &base.entrypoint_externs {
        let names = composed.entrypoint_externs.entry(source_key.clone()).or_default();
        names.extend(
            base_names
                .iter()
                .filter(|crate_name| compiler_runtime_crate_names.contains(*crate_name))
                .cloned(),
        );
        names.sort();
        names.dedup();
    }
    composed.validate_shape(expected_intent).map_err(oven_rustc_error)?;
    Ok(composed)
}

/// Merge the compatible artifact declarations of independently baked public package Loafs.
///
/// This is intentionally not a Cargo-style resolver.  The package publishers already resolved their independent
/// graphs.  Oven only accepts their union when every overlap is byte-identical and every public crate or registry
/// identity denotes one sealed ABI; otherwise it returns an actionable incompatibility instead of selecting an
/// arbitrary first match.
fn merge_packaged_provider_artifact_manifests(
    inputs: &[(&str, &OvenRustcArtifactManifest)],
    expected_intent: &crate::oven::OvenBuildIntent,
) -> CliResult<OvenRustcArtifactManifest> {
    let (first_name, first) = inputs
        .first()
        .ok_or_else(|| CliError::failure("package Loaf manifest composition requires at least one provider"))?;
    first.validate_shape(expected_intent).map_err(oven_rustc_error)?;
    let mut dependency_search_paths = BTreeSet::new();
    let mut native_search_paths = BTreeSet::new();
    let mut externs = BTreeMap::<String, OvenRustcArtifactExtern>::new();
    let mut artifact_digests = BTreeMap::<String, String>::new();
    let mut supporting_artifacts = BTreeMap::<String, OvenRustcSupportingArtifact>::new();
    let mut entrypoint_externs = BTreeMap::<String, BTreeSet<String>>::new();
    let mut registry_leaves = BTreeMap::<(String, String), OvenRustcRegistryLeaf>::new();
    let mut registry_sources = BTreeMap::<(String, String, String), OvenRustcRegistrySourcePackage>::new();
    let mut compile_environment = BTreeMap::<String, String>::new();
    let vocabulary = first.vocab_auxiliary_targets.clone();
    for (name, manifest) in inputs {
        manifest.validate_shape(expected_intent).map_err(oven_rustc_error)?;
        if manifest.vocab_auxiliary_targets != vocabulary {
            return Err(CliError::failure(format!(
                "Oven Alpha cannot compose pub::{name} with pub::{first_name}: their compiler-owned vocabulary closures differ"
            )));
        }
        dependency_search_paths.extend(manifest.dependency_search_paths.iter().cloned());
        native_search_paths.extend(manifest.native_search_paths.iter().cloned());
        for (key, value) in &manifest.compile_environment {
            if let Some(existing) = compile_environment.insert(key.clone(), value.clone())
                && existing != *value
            {
                return Err(CliError::failure(format!(
                    "Oven Alpha cannot compose pub::{name}: compile environment `{key}` conflicts with another sealed package Loaf"
                )));
            }
        }
        for artifact in &manifest.externs {
            if let Some(existing) = externs.get(&artifact.crate_name)
                && existing != artifact
            {
                return Err(CliError::failure(format!(
                    "Oven Alpha cannot compose pub::{name}: direct Rust crate `{}` has incompatible sealed artifacts",
                    artifact.crate_name
                )));
            }
            if let Some(existing_digest) =
                artifact_digests.insert(artifact.relative_path.clone(), artifact.digest.clone())
                && existing_digest != artifact.digest
            {
                return Err(CliError::failure(format!(
                    "Oven Alpha cannot compose pub::{name}: artifact path `{}` has conflicting sealed bytes",
                    artifact.relative_path
                )));
            }
            supporting_artifacts.remove(&artifact.relative_path);
            externs.insert(artifact.crate_name.clone(), artifact.clone());
        }
        for artifact in &manifest.supporting_artifacts {
            if let Some(existing_digest) =
                artifact_digests.insert(artifact.relative_path.clone(), artifact.digest.clone())
                && existing_digest != artifact.digest
            {
                return Err(CliError::failure(format!(
                    "Oven Alpha cannot compose pub::{name}: artifact path `{}` has conflicting sealed bytes",
                    artifact.relative_path
                )));
            }
            if !externs
                .values()
                .any(|direct| direct.relative_path == artifact.relative_path)
            {
                supporting_artifacts.insert(artifact.relative_path.clone(), artifact.clone());
            }
        }
        for (source_key, names) in &manifest.entrypoint_externs {
            entrypoint_externs
                .entry(source_key.clone())
                .or_default()
                .extend(names.iter().cloned());
        }
        for leaf in &manifest.registry_leaves {
            let key = (leaf.package.clone(), leaf.version.clone());
            if let Some(existing) = registry_leaves.get(&key)
                && existing != leaf
            {
                return Err(CliError::failure(format!(
                    "Oven Alpha cannot compose pub::{name}: registry package `{}` `{}` has incompatible sealed features or artifacts",
                    leaf.package, leaf.version
                )));
            }
            registry_leaves.insert(key, leaf.clone());
        }
        for source in &manifest.registry_sources {
            let key = (
                source.package.clone(),
                source.version.clone(),
                source.source.registry.clone(),
            );
            if let Some(existing) = registry_sources.get(&key)
                && existing != source
            {
                return Err(CliError::failure(format!(
                    "Oven Alpha cannot compose pub::{name}: registry source `{}` `{}` has incompatible sealed source or feature facts",
                    source.package, source.version
                )));
            }
            registry_sources.insert(key, source.clone());
        }
    }
    let mut vocabulary_artifacts = BTreeSet::<String>::new();
    for target in &vocabulary {
        for artifact in &target.externs {
            if let Some(existing_digest) =
                artifact_digests.insert(artifact.relative_path.clone(), artifact.digest.clone())
                && existing_digest != artifact.digest
            {
                return Err(CliError::failure(format!(
                    "Oven Alpha cannot compose package Loafs: vocabulary artifact path `{}` has conflicting sealed bytes",
                    artifact.relative_path
                )));
            }
            vocabulary_artifacts.insert(artifact.relative_path.clone());
        }
    }
    for relative_path in vocabulary_artifacts {
        if !externs.values().any(|artifact| artifact.relative_path == relative_path) {
            supporting_artifacts.remove(&relative_path);
            // The vocabulary role carries the physical artifact; retaining it a second time would violate the
            // immutable manifest's one-path rule.
        }
    }
    let composed = OvenRustcArtifactManifest {
        schema_version: first.schema_version,
        intent: expected_intent.clone(),
        dependency_search_paths: dependency_search_paths.into_iter().collect(),
        native_search_paths: native_search_paths.into_iter().collect(),
        externs: externs.into_values().collect(),
        entrypoint_externs: entrypoint_externs
            .into_iter()
            .map(|(key, names)| (key, names.into_iter().collect()))
            .collect(),
        registry_leaves: registry_leaves.into_values().collect(),
        registry_sources: registry_sources.into_values().collect(),
        compile_environment,
        vocab_auxiliary_targets: vocabulary,
        supporting_artifacts: supporting_artifacts.into_values().collect(),
    };
    composed.validate_shape(expected_intent).map_err(oven_rustc_error)?;
    Ok(composed)
}

#[derive(Debug, Clone)]
struct RustExternDeclContext {
    #[allow(dead_code)]
    file_path: PathBuf,
    #[allow(dead_code)]
    source: String,
    item_name: String,
    rust_module_path: String,
    #[allow(dead_code)]
    span: Span,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RustExternBuildFailureKind {
    UnresolvedBackingItem,
    SignatureMismatch,
    FeatureGatedBackingPath,
}

/// Return whether a declaration's decorators include `rust.extern`.
fn has_rust_extern_decorator(decorators: &[Spanned<Decorator>]) -> bool {
    decorators
        .iter()
        .any(|d| d.node.path.segments.join(".") == "rust.extern")
}

/// Collect source contexts for Rust extern declarations in Rust-backed modules.
fn collect_rust_extern_contexts(modules: &[ParsedModule]) -> Vec<RustExternDeclContext> {
    let mut contexts = Vec::new();
    for module in modules {
        let Some(rust_module) = module.ast.rust_module_path.as_ref().map(|p| p.node.clone()) else {
            continue;
        };
        for decl in &module.ast.declarations {
            match &decl.node {
                Declaration::Function(func) if has_rust_extern_decorator(&func.decorators) => {
                    contexts.push(RustExternDeclContext {
                        file_path: module.file_path.clone(),
                        source: module.source.clone(),
                        item_name: func.name.clone(),
                        rust_module_path: rust_module.clone(),
                        span: decl.span,
                    });
                }
                Declaration::Trait(tr) => {
                    for method in &tr.methods {
                        if has_rust_extern_decorator(&method.node.decorators) {
                            contexts.push(RustExternDeclContext {
                                file_path: module.file_path.clone(),
                                source: module.source.clone(),
                                item_name: method.node.name.clone(),
                                rust_module_path: rust_module.clone(),
                                span: method.span,
                            });
                        }
                    }
                }
                Declaration::Model(model) => {
                    for method in &model.methods {
                        if method.node.receiver.is_none() && has_rust_extern_decorator(&method.node.decorators) {
                            contexts.push(RustExternDeclContext {
                                file_path: module.file_path.clone(),
                                source: module.source.clone(),
                                item_name: method.node.name.clone(),
                                rust_module_path: rust_module.clone(),
                                span: method.span,
                            });
                        }
                    }
                }
                Declaration::Class(class) => {
                    for method in &class.methods {
                        if method.node.receiver.is_none() && has_rust_extern_decorator(&method.node.decorators) {
                            contexts.push(RustExternDeclContext {
                                file_path: module.file_path.clone(),
                                source: module.source.clone(),
                                item_name: method.node.name.clone(),
                                rust_module_path: rust_module.clone(),
                                span: method.span,
                            });
                        }
                    }
                }
                Declaration::Newtype(nt) => {
                    for method in &nt.methods {
                        if method.node.receiver.is_none() && has_rust_extern_decorator(&method.node.decorators) {
                            contexts.push(RustExternDeclContext {
                                file_path: module.file_path.clone(),
                                source: module.source.clone(),
                                item_name: method.node.name.clone(),
                                rust_module_path: rust_module.clone(),
                                span: method.span,
                            });
                        }
                    }
                }
                _ => {}
            }
        }
    }
    contexts
}

/// Return stable `rust.module::item` labels for Rust extern declarations that influenced this generated build.
fn rust_extern_report_paths(contexts: &[RustExternDeclContext]) -> Vec<String> {
    let mut paths = contexts
        .iter()
        .map(|context| format!("{}::{}", context.rust_module_path, context.item_name))
        .collect::<Vec<_>>();
    paths.sort();
    paths.dedup();
    paths
}

/// Content-derived identity of the source modules about to be compiled, computed before codegen runs.
///
/// Ordered by file path rather than collection order so the identity does not depend on module
/// discovery order. Used as a [`BackendSelection`]'s `source_identity`.
fn module_source_identity(modules: &[ParsedModule]) -> String {
    let mut ordered: Vec<&ParsedModule> = modules.iter().collect();
    ordered.sort_by(|left, right| left.file_path.cmp(&right.file_path));
    let parts: Vec<&str> = ordered.iter().map(|module| module.source.as_str()).collect();
    digest_output(&parts)
}

/// Content-derived identity of multi-file generated Rust output, used as a backend execution
/// receipt's `output_identity`.
///
/// `rust_modules` is a `HashMap`, so entries are sorted by module path before digesting; the
/// identity must not depend on `HashMap` iteration order.
fn multi_file_output_identity(main_code: &str, rust_modules: &HashMap<Vec<String>, String>) -> String {
    let mut sorted: Vec<(&Vec<String>, &String)> = rust_modules.iter().collect();
    sorted.sort_by(|left, right| left.0.cmp(right.0));
    let mut parts: Vec<&str> = vec![main_code];
    parts.extend(sorted.into_iter().map(|(_, code)| code.as_str()));
    digest_output(&parts)
}

/// Shadow-comparison state for one build's backend execution receipt.
///
/// #1146 implements a real source-observable comparison, but only for the bounded profile in
/// `crate::backend::shadow`: one module, one named free function that is not the program entrypoint, and concrete
/// scalar arguments. Every build path observes the module's `main` instead, whose return value the produced
/// process does not expose, so a requested comparison stays explicitly `Unavailable` with that reason rather than
/// silently `NotRequested` or inferred from generated Rust.
fn backend_shadow_comparison(selection: &BackendSelection) -> ShadowComparisonState {
    unavailable_shadow_comparison(selection.shadow_requested, PROGRAM_ENTRYPOINT_UNAVAILABLE_REASON)
}

/// Declare and resolve a backend selection for one build, before codegen runs (#986).
///
/// Combines [`select_backend`] and [`resolve_execution`] — identical at both the executable
/// (`prepare_oven_project`) and library (`prepare_library_project`) call sites — into the one
/// step callers actually need: a declared selection plus the backend they must invoke, or a
/// visible refusal.
fn select_and_resolve_backend(
    backend_options: &BackendSelectionOptions,
    modules: &[ParsedModule],
) -> CliResult<(BackendSelection, BackendKind)> {
    let selection = select_backend(
        backend_options.requested,
        backend_options.explicit,
        backend_options.shadow,
        module_source_identity(modules),
        backend_options.fallback_policy,
    );
    let executed = resolve_execution(&selection, selection.selected_backend.is_implemented())
        .map_err(|error| CliError::failure(error.to_string()))?;
    Ok((selection, executed))
}

/// Bind a real output identity to a resolved backend selection and surface any declared fallback
/// (#986). Combines [`finalize_receipt`], [`backend_shadow_comparison`], and
/// [`report_backend_fallback`] — identical at both build-path call sites — into one step.
fn finalize_backend_receipt(
    selection: &BackendSelection,
    executed: BackendKind,
    output_identity: String,
) -> CliResult<crate::backend::selection::BackendExecutionReceipt> {
    let receipt = finalize_receipt(
        selection,
        executed,
        output_identity,
        backend_shadow_comparison(selection),
        diagnostics::DIAGNOSTIC_SCHEMA_VERSION,
    )
    .map_err(|error| CliError::failure(error.to_string()))?;
    report_backend_fallback(&receipt);
    Ok(receipt)
}

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

/// Surface a declared backend fallback on stderr, visible even when `--report` is not requested.
fn report_backend_fallback(receipt: &crate::backend::selection::BackendExecutionReceipt) {
    if let FallbackOutcome::Declared { from, to } = receipt.fallback_outcome {
        eprintln!(
            "⚠ backend fallback: `{from:?}` was selected but is not available; executed `{to:?}` instead (declared, not silent)"
        );
    }
}

/// Compiler-owned, project-relative destination for a build's backend-selection execution receipt.
///
/// Kept separate from `crate::oven::DEFAULT_RECEIPT_RELATIVE_PATH`: the Oven receipt is the
/// build-unit/native-plan boundary, while this receipt is the backend-selection/execution
/// boundary (#986). Oven and other clients consume this without reading private HIR/Body IR.
const DEFAULT_BACKEND_RECEIPT_RELATIVE_PATH: &str = ".incan/backend/receipt.json";

/// Stable schema marker for the direct Body-IR replacement execution report.
///
/// This is distinct from the Oven build-report schema because this path has no generated Rust, artifacts, or Oven
/// plan to report. Consumers must inspect its backend receipt and direct-execution evidence rather than treating it
/// as a partial legacy build report.
const REPLACEMENT_EXECUTION_REPORT_SCHEMA_VERSION: &str = "incan.replacement_execution.v1";

/// Return the compiler-owned project-relative destination for a backend-selection execution receipt.
fn default_backend_receipt_path(project_root: &Path) -> PathBuf {
    project_root.join(DEFAULT_BACKEND_RECEIPT_RELATIVE_PATH)
}

/// Publish a backend-selection execution receipt through a same-directory staged file and atomic
/// replacement, mirroring `crate::oven::write_receipt`'s durability guarantee for its own receipt.
fn write_backend_receipt(receipt: &crate::backend::selection::BackendExecutionReceipt, path: &Path) -> CliResult<()> {
    let parent = path
        .parent()
        .ok_or_else(|| CliError::failure(format!("invalid backend-selection receipt path {}", path.display())))?;
    fs::create_dir_all(parent)
        .map_err(|error| CliError::failure(format!("failed to create {}: {error}", parent.display())))?;
    let payload = serde_json::to_vec_pretty(receipt)
        .map_err(|error| CliError::failure(format!("failed to serialize backend-selection receipt: {error}")))?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| CliError::failure(format!("invalid backend-selection receipt path {}", path.display())))?;
    let staged_path = parent.join(format!(".{file_name}.tmp-{}", std::process::id()));
    let result = crate::oven::write_receipt_staged(&payload, &staged_path, path, parent);
    if result.is_err() && staged_path.exists() {
        let _ = fs::remove_file(&staged_path);
    }
    result.map_err(|error| {
        CliError::failure(format!(
            "failed to publish backend-selection receipt {}: {error}",
            path.display()
        ))
    })
}

/// Convert a typed replacement refusal into the CLI's stable source-location presentation.
///
/// `CliError` predates typed frontend diagnostics and carries display text only, so this adapter retains the
/// replacement diagnostic code, entrypoint path, and original Body-IR span rather than discarding them at the CLI
/// boundary.
fn replacement_profile_cli_error(error: ReplacementExecutionError, entrypoint: &Path) -> CliError {
    match error.primary_span() {
        Some(span) => CliError::failure(format!(
            "{}: {error}\nprimary Incan source location: {}:{}..{}",
            error.diagnostic_code(),
            entrypoint.display(),
            span.start,
            span.end
        )),
        None => CliError::failure(format!("{}: {error}", error.diagnostic_code())),
    }
}

/// Whether one import names another module of the project currently being built.
///
/// #1260 executes a call that leaves the entry module, so an import naming a sibling of that module is now inside the
/// profile: the session analyzes the whole source graph at once, and the execution graph resolves the callee's
/// canonical identity to the module that declares it. Everything reaching outside that graph stays refused -- the
/// standard library, a `pub::` package dependency, and every Rust or Python interop form each bring a boundary the
/// source-only profile has no evidence for, and each is owned by a separate child of #989.
fn replacement_local_module_import(import: &crate::frontend::ast::ImportDecl) -> bool {
    let path = match &import.kind {
        ImportKind::Module(path) | ImportKind::From { module: path, .. } => path,
        // `pub::` dependencies, Rust crates and Python modules are other packages by construction.
        _ => return false,
    };
    let Some(first) = path.segments.first() else {
        return false;
    };
    first.as_str() != incan_core::lang::stdlib::STDLIB_ROOT
}

/// Return the first unsupported source-module boundary with its original Incan source span.
fn replacement_module_profile_error(program: &crate::frontend::ast::Program) -> Option<ReplacementExecutionError> {
    if let Some(rust_module) = &program.rust_module_path {
        return Some(ReplacementExecutionError::unsupported_profile(
            "Rust interop `rust.module` directive",
            HirSourceSpan::new(rust_module.span.start, rust_module.span.end),
        ));
    }
    let mut async_activation_seen = false;
    for declaration in &program.declarations {
        if matches!(declaration.node, Declaration::Function(_) | Declaration::Docstring(_))
            || matches!(&declaration.node, Declaration::Model(model) if is_direct_replacement_plain_model(model))
            || matches!(&declaration.node, Declaration::Enum(enum_decl) if is_direct_replacement_fieldless_enum(enum_decl))
            || matches!(&declaration.node, Declaration::Enum(enum_decl) if is_direct_replacement_value_enum(enum_decl))
        {
            continue;
        }
        if let Declaration::Import(import) = &declaration.node {
            let exact_async_activation = matches!(
                (&import.visibility, &import.kind, &import.alias),
                (Visibility::Private, ImportKind::Module(path), None)
                    if !path.is_absolute
                        && path.parent_levels == 0
                        && path.segments == ["std", "async"]
            );
            if exact_async_activation && !async_activation_seen {
                async_activation_seen = true;
                continue;
            }
            if !exact_async_activation && replacement_local_module_import(import) {
                continue;
            }
            let description = if exact_async_activation {
                "duplicate `import std.async` replacement activation"
            } else {
                "import declaration"
            };
            return Some(ReplacementExecutionError::unsupported_profile(
                description,
                HirSourceSpan::new(declaration.span.start, declaration.span.end),
            ));
        }
        return Some(ReplacementExecutionError::unsupported_profile(
            "non-function top-level declaration",
            HirSourceSpan::new(declaration.span.start, declaration.span.end),
        ));
    }
    None
}

/// Refuse one unsupported replacement profile through the canonical #986 selection boundary.
///
/// The resolver must reject the availability claim before the profile diagnostic reaches the CLI. If a future
/// fallback policy resolves it anyway, that is a separate, visible failure rather than implicit legacy execution.
fn refuse_replacement_profile<T>(
    selection: &BackendSelection,
    error: ReplacementExecutionError,
    entrypoint: &Path,
) -> CliResult<T> {
    match resolve_execution(selection, false) {
        Err(_) => Err(replacement_profile_cli_error(error, entrypoint)),
        Ok(executed) => Err(CliError::failure(format!(
            "{}: replacement source-profile refusal cannot execute `{executed:?}` because this CLI exposes no receipt-bound fallback path",
            error.diagnostic_code()
        ))),
    }
}

/// Resolve an available direct replacement selection through the canonical #986 boundary.
fn resolve_available_replacement_execution(selection: &BackendSelection) -> CliResult<BackendKind> {
    match resolve_execution(selection, true) {
        Ok(BackendKind::Replacement) => Ok(BackendKind::Replacement),
        Ok(executed) => Err(CliError::failure(format!(
            "replacement profile selection resolved unexpected backend `{executed:?}`"
        ))),
        Err(error) => Err(CliError::failure(error.to_string())),
    }
}

/// Checked entry facts the direct replacement executor may consume from one compilation session.
///
/// This private bundle is the replacement CLI's authority boundary: no later stage may lex, parse, re-resolve, or
/// typecheck the source again. `TypeCheckInfo` is retained only as Body IR's transitional lowering bridge; semantic
/// provenance comes from the sibling portable snapshot produced by that same analysis pass.
struct ReplacementSessionInputs {
    program: crate::frontend::ast::Program,
    module_path: Vec<String>,
    type_info: typechecker::TypeCheckInfo,
    semantic_module: SemanticModuleProvenance,
    /// Every non-entry module the one analysis already checked, in collection order.
    ///
    /// The session collects and analyzes the whole root source graph and previously kept only the entrypoint, which
    /// is all the same-module #988 profile can execute. #1260 executes a call that leaves the entry module, so the
    /// modules that call may reach have to survive the same analysis rather than be re-collected or re-checked
    /// later: re-analysis would produce a second checker authority, and identities minted by two analyses cannot be
    /// compared.
    ///
    /// The entrypoint is deliberately not repeated here. It stays in the fields above so existing readers keep
    /// working unchanged, and the execution graph is assembled with the entry module as its primary.
    reachable_modules: Vec<ReplacementModuleInputs>,
}

/// One checked non-entry module retained from the replacement session's single analysis.
struct ReplacementModuleInputs {
    program: crate::frontend::ast::Program,
    module_path: Vec<String>,
    type_info: typechecker::TypeCheckInfo,
}

/// Collect and analyze the replacement entrypoint once through the project-selected compilation session.
///
/// The entry AST, Body-IR lowering bridge, and semantic provenance are extracted as one product. This is deliberately
/// the only constructor for [`ReplacementSessionInputs`], making an independent replacement CLI typecheck impossible
/// without crossing this explicit boundary.
fn replacement_session_inputs(
    entrypoint: &Path,
    compilation_session: &CompilationSession,
) -> CliResult<ReplacementSessionInputs> {
    let modules = collect_modules_detailed_with_session(entrypoint.to_path_buf(), compilation_session)
        .map_err(|failure| CliError::failure(failure.render_human()))?;
    let analysis = compilation_session
        .analyze_modules(
            &modules,
            #[cfg(feature = "rust_inspect")]
            None,
        )
        .map_err(|failure| CliError::failure(failure.render_human()))?;
    let entry_module = modules
        .iter()
        .find(|module| module.file_path == entrypoint)
        .ok_or_else(|| {
            CliError::failure(format!(
                "replacement session did not collect entrypoint {}",
                entrypoint.display()
            ))
        })?;
    let entry_analysis = analysis
        .module_analysis_for_path(&entry_module.file_path)
        .ok_or_else(|| {
            CliError::failure(format!(
                "replacement session did not retain checked analysis for entrypoint {}",
                entrypoint.display()
            ))
        })?;
    let semantic_snapshot = entry_analysis.semantic_snapshot();
    let semantic_snapshot_rendering = semantic_snapshot.render_snapshot();
    let source_identity = digest_output(&[entry_module.source.as_str()]);

    let reachable_modules = modules
        .iter()
        .filter(|module| module.file_path != entry_module.file_path)
        .filter_map(|module| {
            analysis
                .module_analysis_for_path(&module.file_path)
                .map(|checked| ReplacementModuleInputs {
                    program: module.ast.clone(),
                    module_path: module.path_segments.clone(),
                    type_info: checked.type_info().clone(),
                })
        })
        .collect();

    Ok(ReplacementSessionInputs {
        program: entry_module.ast.clone(),
        module_path: entry_module.path_segments.clone(),
        type_info: entry_analysis.type_info().clone(),
        reachable_modules,
        semantic_module: SemanticModuleProvenance::new(
            semantic_snapshot.hir.id.to_string(),
            semantic_snapshot.hir.path.clone(),
            source_identity,
            digest_output(&[semantic_snapshot_rendering.as_str()]),
        ),
    })
}

/// Execute the first #988 replacement-backend profile directly from typed Body IR.
///
/// This intentionally has no `ProjectGenerator`, Oven, or generated-Rust path. It accepts only one source module
/// containing free functions and executes its zero-argument `main` body through the replacement executor. A
/// requested replacement build therefore either records a replacement receipt over a real Body-IR result or fails
/// visibly at the original Incan span; it can never reach `IrCodegen` as an implicit compatibility fallback.
///
/// The pipeline constructs one [`CompilationSession`], collects and analyzes the selected module graph through it,
/// applies the module-profile gate to its projected entry AST, and lowers only from the resulting checked facts. The
/// session owns parsing, vocab desugaring, feature projection, and typechecking; this CLI path must never derive a
/// second authority for the same source.
fn build_replacement_file_report(
    file_path: &str,
    options: BuildCommandOptions,
    report_options: &BuildReportOptions,
) -> CliResult<serde_json::Value> {
    if report_options.enabled() && report_options.output_path.is_none() {
        return Err(CliError::failure(
            "replacement execution keeps stdout and stderr for the program; use --report-output <file> with --report json",
        ));
    }
    reject_normal_cargo_controls(&options.cargo_policy, options.generated_cargo_target_dir.as_ref())?;
    let start = Instant::now();
    let entrypoint = if Path::new(file_path).is_absolute() {
        PathBuf::from(file_path)
    } else {
        env::current_dir()
            .map_err(|error| CliError::failure(format!("failed to determine current directory: {error}")))?
            .join(file_path)
    };
    let compilation_session = CompilationSession::discover_for_collection_with_selections(
        &entrypoint,
        &options.package_features,
        options.sdk_profile.as_deref(),
    )?;
    let session_inputs = replacement_session_inputs(&entrypoint, &compilation_session)?;
    let selection = select_backend(
        options.backend.requested,
        options.backend.explicit,
        options.backend.shadow,
        session_inputs.semantic_module.source_identity(),
        options.backend.fallback_policy,
    );
    // Every module the call can reach has to satisfy the profile, not only the entrypoint. Allowing a local import
    // means an unsupported declaration is now reachable from a file the entry never names, and refusing it here keeps
    // the boundary a property of the executed graph rather than of whichever file happened to be the entrypoint.
    let profile_error = replacement_module_profile_error(&session_inputs.program).or_else(|| {
        session_inputs
            .reachable_modules
            .iter()
            // The one analysis also checks the standard-library modules the graph pulled in, which arrive under the
            // generated `__incan_std` namespace rather than the source `std` spelling. Those are not project source
            // and were never meant to satisfy a source-only profile -- `import std.async` legitimately reaches stdlib
            // modules full of classes and imports. Only the project's own modules are executed here, so only they are
            // held to the profile.
            .filter(|module| {
                !matches!(
                    module.module_path.first().map(String::as_str),
                    Some(incan_core::lang::stdlib::STDLIB_ROOT | incan_core::lang::stdlib::INCAN_STD_NAMESPACE)
                )
            })
            .find_map(|module| replacement_module_profile_error(&module.program))
    });
    if let Some(error) = profile_error {
        return refuse_replacement_profile(&selection, error, &entrypoint);
    }
    let body_ir = build_body_ir_module_v0(
        &session_inputs.program,
        &session_inputs.module_path,
        &session_inputs.type_info,
    );
    // Lower every module the one analysis checked, not just the entrypoint. A call that leaves the entry module can
    // only resolve if its callee's module was lowered from that same analysis; lowering it later, or from a second
    // analysis, would mint identities that cannot be compared with the ones the entry module carries.
    let reachable_body_ir: Vec<_> = session_inputs
        .reachable_modules
        .iter()
        .map(|module| build_body_ir_module_v0(&module.program, &module.module_path, &module.type_info))
        .collect();
    let execution_graph = match ReplacementExecutionGraph::new(&body_ir, reachable_body_ir.iter()) {
        Ok(graph) => graph,
        Err(error) => return refuse_replacement_profile(&selection, error, &entrypoint),
    };
    let execution_plan = match prepare_free_function_execution_in_graph(execution_graph, "main", &[], None) {
        Ok(plan) => plan,
        Err(error) => return refuse_replacement_profile(&selection, error, &entrypoint),
    };
    let executed = resolve_available_replacement_execution(&selection)?;
    let execution = execute_prevalidated_free_function(execution_plan)
        .map_err(|error| replacement_profile_cli_error(error, &entrypoint))?;
    let result_type = execution.value.scalar_type_name().ok_or_else(|| {
        CliError::failure("replacement execution produced a non-scalar value after scalar-result validation")
    })?;
    let shadow_comparison = backend_shadow_comparison(&selection);
    let backend_receipt = finalize_receipt_with_semantic_module(
        &selection,
        executed,
        execution.output_identity.clone(),
        shadow_comparison,
        diagnostics::DIAGNOSTIC_SCHEMA_VERSION,
        Some(session_inputs.semantic_module.clone()),
    )
    .map_err(|error| CliError::failure(error.to_string()))?;
    let project_root = resolve_project_root(&entrypoint);
    write_backend_receipt(&backend_receipt, &default_backend_receipt_path(&project_root))?;
    Ok(serde_json::json!({
        "schema_version": REPLACEMENT_EXECUTION_REPORT_SCHEMA_VERSION,
        "compiler_version": crate::version::INCAN_VERSION,
        "status": "success",
        "mode": "executable",
        "entrypoint": entrypoint,
        "backend": backend_receipt,
        "semantic_module": session_inputs.semantic_module,
        "replacement_execution": {
            "result": execution.value.observable_text(),
            "result_type": result_type,
            "output_identity": execution.output_identity,
            "emitted_output": execution.emitted_output(),
            "stdout_bytes": execution.output.stdout(),
            "stderr_bytes": execution.output.stderr(),
            "body_snapshot": execution.body_snapshot,
            "ownership_reads": execution.ownership_evidence(),
            "runtime_requirements": execution.runtime_requirement_evidence(),
            "task_lifecycle": execution.task_lifecycle_evidence(),
        },
        "timings_ms": { "total": elapsed_ms(start) },
    }))
}

/// Build the project identity block used by build and generated Rust inspection reports.
fn manifest_project_report(
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
fn source_file_report(modules: &[ParsedModule]) -> Vec<SourceFileReport> {
    modules
        .iter()
        .map(|module| SourceFileReport {
            path: module.file_path.to_string_lossy().to_string(),
            module_path: module.path_segments.clone(),
        })
        .collect()
}

/// Return elapsed milliseconds as a bounded `u64` for report payloads.
fn elapsed_ms(start: Instant) -> u64 {
    start.elapsed().as_millis().min(u128::from(u64::MAX)) as u64
}

/// Record one named build phase timing.
fn record_timing(timings: &mut BTreeMap<String, u64>, name: &str, start: Instant) {
    timings.insert(name.to_string(), elapsed_ms(start));
}

/// Print human build progress to stderr when stdout is reserved for a machine-readable report.
fn print_build_progress(report_options: &BuildReportOptions, message: impl AsRef<str>) {
    if report_options.enabled() {
        eprintln!("{}", message.as_ref());
    } else {
        println!("{}", message.as_ref());
    }
}

/// Return the stable cache key used for one wrapped inline command source from one working directory.
fn inline_command_cache_key(cwd: &Path, wrapped_source: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(cwd.to_string_lossy().as_bytes());
    hasher.update(b"\0");
    hasher.update(wrapped_source.as_bytes());
    let digest = hasher.finalize();
    hex::encode(&digest[..8])
}

/// Return the stable generated project identity used for one `incan run -c` source.
fn inline_command_project_for_cwd(cwd: &Path, wrapped_source: &str) -> InlineCommandProject {
    let digest = inline_command_cache_key(cwd, wrapped_source);
    let project_name = format!("{INLINE_COMMAND_PROJECT_PREFIX}_{digest}");
    let source_path = env::temp_dir().join(&project_name).join("main.incn");
    let output_dir = format!("{INLINE_COMMAND_OUTPUT_PARENT}/{project_name}");
    InlineCommandProject {
        source_path,
        project_name,
        output_dir,
    }
}

/// Resolve the current invocation's stable inline-command generated project identity.
fn inline_command_project(wrapped_source: &str) -> CliResult<InlineCommandProject> {
    let cwd = env::current_dir().map_err(|err| {
        CliError::failure(format!(
            "failed to determine current directory for inline command cache: {err}"
        ))
    })?;
    Ok(inline_command_project_for_cwd(&cwd, wrapped_source))
}

/// Preserve the legacy `run -c` behavior by adding a no-op `main` only when the snippet did not define one.
fn wrap_inline_command_source(source: &str) -> String {
    if source.contains("def main") {
        source.to_string()
    } else {
        format!("{source}\n\ndef main() -> Unit:\n  pass\n")
    }
}

#[cfg(feature = "rust_inspect")]
/// Collect canonical Rust metadata paths that must be shipped in a library manifest's ABI payload.
fn collect_library_rust_abi_query_paths(
    modules: &[ParsedModule],
    rust_extern_contexts: &[RustExternDeclContext],
) -> Vec<String> {
    let mut paths: BTreeSet<String> = collect_rust_inspect_query_paths(modules).into_iter().collect();
    for context in rust_extern_contexts {
        paths.insert(format!("{}::{}", context.rust_module_path, context.item_name));
    }
    paths.into_iter().collect()
}

#[cfg(feature = "rust_inspect")]
/// Extract complete Rust metadata from the generated inspect workspace and package it as manifest ABI.
///
/// Prewarm deliberately permits a fast syntax-only fallback. A library artifact is a durable semantic boundary, so
/// publishing whatever happens to be in that shared cache would make its ABI depend on earlier compiler queries.
fn collect_library_rust_abi(
    rust_inspect_manifest_dir: &Path,
    query_paths: &[String],
) -> CliResult<Option<LibraryRustAbi>> {
    if query_paths.is_empty() {
        return Ok(None);
    }

    let inspector = Inspector::new(InspectorConfig::new(rust_inspect_manifest_dir.to_path_buf()));
    let mut items = Vec::new();
    for path in query_paths {
        let Some(lookup_path) = Inspector::normalize_lookup_path(path) else {
            continue;
        };
        match inspector
            .cache()
            .get_or_extract_complete(rust_inspect_manifest_dir, lookup_path, &|_| ())
        {
            Ok(metadata) => items.push((*metadata).clone()),
            Err(
                RustMetadataError::CrateNotFound(_)
                | RustMetadataError::PathNotResolved(_)
                | RustMetadataError::UnsupportedMacro(_),
            ) => {}
            Err(err) => {
                return Err(CliError::failure(format!(
                    "failed to extract complete Rust ABI metadata for `{path}` from {}: {err}",
                    rust_inspect_manifest_dir.display()
                )));
            }
        }
    }
    Ok(LibraryRustAbi::from_items(items))
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

/// Resolve the project root for library commands from an optional source path or project directory.
fn resolve_library_project_root(file_path: Option<&str>) -> CliResult<PathBuf> {
    if let Some(file_path) = file_path {
        let normalized = if Path::new(file_path).is_absolute() {
            PathBuf::from(file_path)
        } else {
            env::current_dir()
                .map_err(|e| CliError::failure(format!("failed to determine current directory: {e}")))?
                .join(file_path)
        };
        if normalized.is_dir() {
            return Ok(normalized);
        }
        return Ok(resolve_project_root(&normalized));
    }

    env::current_dir().map_err(|e| CliError::failure(format!("failed to determine current directory: {e}")))
}

/// Resolve and require the project's canonical `src/lib.incn` entrypoint.
fn validate_library_entrypoint(manifest: &ProjectManifest) -> CliResult<PathBuf> {
    let lib_entry = manifest.project_root().join("src").join("lib.incn");
    if !lib_entry.is_file() {
        return Err(CliError::failure(format!(
            "`incan build --lib` requires `{}`",
            lib_entry.display()
        )));
    }
    Ok(lib_entry)
}

/// Return the generated module key for canonicalized source path segments.
fn module_key(path_segments: &[String]) -> String {
    canonicalize_source_module_segments(path_segments).join("_")
}

/// Rename one checked export while preserving its semantic export kind.
fn rename_checked_export(export: &CheckedNamedExport, exported_name: &str) -> CheckedNamedExport {
    let mut renamed = export.clone();
    renamed.name = exported_name.to_string();

    match &mut renamed.kind {
        CheckedExportKind::Function(function_export) => function_export.name = exported_name.to_string(),
        CheckedExportKind::Partial(partial_export) => partial_export.name = exported_name.to_string(),
        CheckedExportKind::Alias(alias_export) => {
            alias_export.name = exported_name.to_string();
            // Rename the callable the alias projects along with the alias itself. The projection describes the
            // binding a consumer resolves under this public name, so a renaming re-export must carry the new name
            // here too; only `emitted_name` stays put, because the declaration behind the rename is unchanged.
            if let Some(projected_function) = alias_export.projected_function.as_mut() {
                projected_function.name = exported_name.to_string();
            }
        }
        CheckedExportKind::TypeAlias(type_alias_export) => type_alias_export.name = exported_name.to_string(),
        CheckedExportKind::Model(model_export) => model_export.name = exported_name.to_string(),
        CheckedExportKind::Class(class_export) => class_export.name = exported_name.to_string(),
        CheckedExportKind::Trait(trait_export) => trait_export.name = exported_name.to_string(),
        CheckedExportKind::Enum(enum_export) => enum_export.name = exported_name.to_string(),
        CheckedExportKind::Newtype(newtype_export) => newtype_export.name = exported_name.to_string(),
        CheckedExportKind::Const(const_export) => const_export.name = exported_name.to_string(),
        CheckedExportKind::Static(static_export) => static_export.name = exported_name.to_string(),
    }

    renamed
}

/// Project a checked provider export through the entrypoint binding that actually re-exports it.
///
/// The provider export retains the concrete declaration shape (model, trait, function, and so on), while the
/// entrypoint's checked export owns the re-export path and target identity. Combining those two checked products
/// avoids relabeling a renamed declaration as a direct export whose public name no longer matches its canonical
/// declaration.
fn project_checked_reexport(
    export: &CheckedNamedExport,
    exported_name: &str,
    entrypoint_exports: Option<&HashMap<String, Vec<CheckedNamedExport>>>,
) -> CheckedNamedExport {
    let mut projected = rename_checked_export(export, exported_name);
    let Some(candidates) = entrypoint_exports.and_then(|exports| exports.get(exported_name)) else {
        return projected;
    };
    let checked_projection = candidates
        .iter()
        .find(|candidate| candidate.identity.canonical == export.identity.canonical)
        .or_else(|| (candidates.len() == 1).then(|| &candidates[0]));
    if let Some(checked_projection) = checked_projection {
        projected.identity = checked_projection.identity.clone();
    }
    projected
}

/// Group checked exports by public source name while preserving same-name function overload entries.
fn checked_exports_by_name(exports: Vec<CheckedNamedExport>) -> HashMap<String, Vec<CheckedNamedExport>> {
    let mut grouped: HashMap<String, Vec<CheckedNamedExport>> = HashMap::new();
    for export in exports {
        grouped.entry(export.name.clone()).or_default().push(export);
    }
    grouped
}

/// Map exported scalar value enums to the serialized identities used by library consumers.
fn public_ordinal_type_identities(
    lib_module: &ParsedModule,
    project_name: &str,
    selected_exports: &[CheckedNamedExport],
) -> HashMap<String, String> {
    let exported_value_enums = selected_exports
        .iter()
        .filter_map(|export| match &export.kind {
            CheckedExportKind::Enum(enum_export) if enum_export.value_type.is_some() => Some(export.name.as_str()),
            _ => None,
        })
        .collect::<HashSet<_>>();
    if exported_value_enums.is_empty() {
        return HashMap::new();
    }

    let mut identities = HashMap::new();
    for decl in &lib_module.ast.declarations {
        let Declaration::Enum(enum_decl) = &decl.node else {
            continue;
        };
        if !matches!(enum_decl.visibility, crate::frontend::ast::Visibility::Public) {
            continue;
        }
        if exported_value_enums.contains(enum_decl.name.as_str()) {
            identities.insert(
                format!("lib.{}", enum_decl.name),
                format!("{project_name}.{}", enum_decl.name),
            );
        }
    }
    for decl in &lib_module.ast.declarations {
        let Declaration::Import(import) = &decl.node else {
            continue;
        };
        if !matches!(import.visibility, crate::frontend::ast::Visibility::Public) {
            continue;
        }
        let ImportKind::From { module, items } = &import.kind else {
            continue;
        };
        let source_module = canonicalize_source_module_segments(&module.segments).join(".");
        for item in items {
            let exported_name = item.alias.as_deref().unwrap_or(item.name.as_str());
            if exported_value_enums.contains(exported_name) {
                identities.insert(
                    format!("{source_module}.{}", item.name),
                    format!("{project_name}.{exported_name}"),
                );
            }
        }
    }
    identities
}

struct LibraryReexportResolver<'a> {
    module_exports: &'a HashMap<String, HashMap<String, Vec<CheckedNamedExport>>>,
}

impl<'a> LibraryReexportResolver<'a> {
    /// Create a resolver over checked exports grouped by canonical source-module name and source export name.
    fn new(module_exports: &'a HashMap<String, HashMap<String, Vec<CheckedNamedExport>>>) -> Self {
        Self { module_exports }
    }

    /// Resolve direct public declarations and `pub from ... import ...` declarations in a library entrypoint into
    /// checked public exports.
    ///
    /// A single source name can map to several checked exports when the provider exposes same-name overloads. The
    /// resolver therefore preserves all matching exports and only applies the consumer-facing alias to each one.
    fn resolve(
        &self,
        lib_module: &ParsedModule,
    ) -> Result<Vec<CheckedNamedExport>, Vec<crate::frontend::diagnostics::CompileError>> {
        let mut errors = Vec::new();
        let mut resolved = Vec::new();
        let mut exported_names = LibraryExportBindingRegistry::default();
        let known_modules: Vec<String> = self.module_exports.keys().cloned().collect();
        let entrypoint_exports = self.module_exports.get(&module_key(&lib_module.path_segments));

        if let Some(exports_by_name) = self.module_exports.get(&module_key(&lib_module.path_segments)) {
            for (export_name, export_span) in Self::direct_public_exports(lib_module) {
                if let Err(error) = exported_names.register(&export_name, export_span) {
                    errors.push(error);
                    continue;
                }
                if let Some(exports) = exports_by_name.get(&export_name) {
                    resolved.extend(exports.iter().cloned());
                }
            }
        }

        for decl in &lib_module.ast.declarations {
            let Declaration::Import(import) = &decl.node else {
                continue;
            };
            if !matches!(import.visibility, crate::frontend::ast::Visibility::Public) {
                continue;
            }

            if let ImportKind::RustFrom {
                crate_name,
                path,
                items,
                ..
            } = &import.kind
            {
                let Some(exports_by_name) = self.module_exports.get(&module_key(&lib_module.path_segments)) else {
                    errors.push(diagnostics::errors::library_reexport_unknown_module(
                        &module_key(&lib_module.path_segments),
                        &known_modules,
                        decl.span,
                    ));
                    continue;
                };
                let mut source_segments = vec!["rust".to_string(), crate_name.clone()];
                source_segments.extend(path.iter().cloned());
                let source_path = source_segments.join("::");

                for item in items {
                    let exported_name = item.alias.as_ref().unwrap_or(&item.name).clone();
                    if let Err(error) = exported_names.register(&exported_name, decl.span) {
                        errors.push(error);
                        continue;
                    }

                    let Some(exports) = exports_by_name.get(&exported_name) else {
                        let available: Vec<String> = exports_by_name.keys().cloned().collect();
                        errors.push(diagnostics::errors::import_not_exported(
                            &item.name,
                            &source_path,
                            &available,
                            decl.span,
                        ));
                        continue;
                    };
                    resolved.extend(exports.iter().cloned());
                }
                continue;
            }

            let ImportKind::From { module, items } = &import.kind else {
                errors.push(diagnostics::errors::library_pub_reexport_requires_from(decl.span));
                continue;
            };

            let module_name = module_key(&module.segments);
            let Some(exports_by_name) = self.module_exports.get(&module_name) else {
                errors.push(diagnostics::errors::library_reexport_unknown_module(
                    &module.to_rust_path(),
                    &known_modules,
                    decl.span,
                ));
                continue;
            };

            for item in items {
                let exported_name = item.alias.as_ref().unwrap_or(&item.name).clone();
                if let Err(error) = exported_names.register(&exported_name, decl.span) {
                    errors.push(error);
                    continue;
                }

                let Some(exports) = exports_by_name.get(&item.name) else {
                    let available: Vec<String> = exports_by_name.keys().cloned().collect();
                    errors.push(diagnostics::errors::import_not_exported(
                        &item.name,
                        &module.to_rust_path(),
                        &available,
                        decl.span,
                    ));
                    continue;
                };

                resolved.extend(
                    exports
                        .iter()
                        .map(|export| project_checked_reexport(export, &exported_name, entrypoint_exports)),
                );
            }
        }

        if errors.is_empty() { Ok(resolved) } else { Err(errors) }
    }

    /// Return public names declared directly by the library entrypoint, excluding public imports that are resolved from
    /// their source module below.
    fn direct_public_exports(lib_module: &ParsedModule) -> Vec<(String, crate::frontend::ast::Span)> {
        lib_module
            .ast
            .declarations
            .iter()
            .filter_map(|decl| match &decl.node {
                Declaration::Function(function)
                    if matches!(function.visibility, crate::frontend::ast::Visibility::Public) =>
                {
                    Some((function.name.clone(), decl.span))
                }
                Declaration::Model(model) if matches!(model.visibility, crate::frontend::ast::Visibility::Public) => {
                    Some((model.name.clone(), decl.span))
                }
                Declaration::Class(class) if matches!(class.visibility, crate::frontend::ast::Visibility::Public) => {
                    Some((class.name.clone(), decl.span))
                }
                Declaration::Trait(trait_decl)
                    if matches!(trait_decl.visibility, crate::frontend::ast::Visibility::Public) =>
                {
                    Some((trait_decl.name.clone(), decl.span))
                }
                Declaration::Enum(enum_decl)
                    if matches!(enum_decl.visibility, crate::frontend::ast::Visibility::Public) =>
                {
                    Some((enum_decl.name.clone(), decl.span))
                }
                Declaration::Newtype(newtype_decl)
                    if matches!(newtype_decl.visibility, crate::frontend::ast::Visibility::Public) =>
                {
                    Some((newtype_decl.name.clone(), decl.span))
                }
                Declaration::TypeAlias(alias)
                    if matches!(alias.visibility, crate::frontend::ast::Visibility::Public) =>
                {
                    Some((alias.name.clone(), decl.span))
                }
                Declaration::Const(konst) if matches!(konst.visibility, crate::frontend::ast::Visibility::Public) => {
                    Some((konst.name.clone(), decl.span))
                }
                Declaration::Static(static_decl)
                    if matches!(static_decl.visibility, crate::frontend::ast::Visibility::Public) =>
                {
                    Some((static_decl.name.clone(), decl.span))
                }
                Declaration::Alias(alias) if matches!(alias.visibility, crate::frontend::ast::Visibility::Public) => {
                    Some((alias.name.clone(), decl.span))
                }
                Declaration::Partial(partial)
                    if matches!(partial.visibility, crate::frontend::ast::Visibility::Public) =>
                {
                    Some((partial.name.clone(), decl.span))
                }
                _ => None,
            })
            .collect()
    }
}

/// Prepare an Incan project for building or running.
///
/// This function performs all the shared setup:
/// 1. Collect and parse modules
/// 2. Type check
/// 3. Configure codegen (serde, async, web, etc.)
/// 4. Add Rust crate dependencies
/// 5. Generate Rust project files
#[allow(clippy::too_many_arguments)] // This orchestration boundary mirrors independent CLI feature and Cargo axes.
#[cfg(test)]
fn prepare_project(
    file_path: &str,
    output_dir: Option<&str>,
    cargo_policy: &CargoPolicy,
    package_features: &FeatureSelection,
    sdk_profile_override: Option<&str>,
    cargo_features: Vec<String>,
    cargo_no_default_features: bool,
    cargo_all_features: bool,
    cargo_profile: &str,
) -> CliResult<()> {
    prepare_project_with_options(
        file_path,
        PrepareProjectOptions {
            output_dir,
            project_name_override: None,
            generated_cargo_target_dir: None,
            cargo_profile,
            sdk_profile_override,
        },
        cargo_policy,
        package_features,
        cargo_features,
        cargo_no_default_features,
        cargo_all_features,
    )
}

/// Prepare an executable project with optional internal identity overrides for callers that need bounded cache names.
#[cfg(test)]
fn prepare_project_with_options(
    file_path: &str,
    options: PrepareProjectOptions<'_>,
    cargo_policy: &CargoPolicy,
    package_features: &FeatureSelection,
    cargo_features: Vec<String>,
    cargo_no_default_features: bool,
    cargo_all_features: bool,
) -> CliResult<()> {
    let normalized_file_path = if Path::new(file_path).is_absolute() {
        PathBuf::from(file_path)
    } else {
        env::current_dir()
            .map_err(|e| CliError::failure(format!("failed to determine current directory: {e}")))?
            .join(file_path)
    };
    let path = normalized_file_path.as_path();
    let inferred_project_root = resolve_project_root(path);
    let compilation_session = super::common::CompilationSession::discover_with_selections(
        path,
        package_features,
        options.sdk_profile_override,
    )?;
    let manifest = compilation_session.manifest.clone();
    if let Some(manifest) = manifest.as_ref() {
        enforce_project_toolchain_constraint(manifest)?;
    }

    let modules =
        super::common::collect_modules_detailed_with_session(normalized_file_path.clone(), &compilation_session)
            .map_err(|failure| CliError::failure(failure.render_human()))?;
    let rust_extern_contexts = collect_rust_extern_contexts(&modules);

    let Some(main_module) = modules.last() else {
        return Err(CliError::failure("No modules found"));
    };

    let dep_modules = &modules[..modules.len() - 1];
    let project_root = manifest
        .as_ref()
        .map(|manifest| manifest.project_root().to_path_buf())
        .unwrap_or(inferred_project_root);
    let package_feature_plan = compilation_session.package_feature_plan.clone();
    let library_manifest_index = compilation_session.library_manifest_index.clone();
    let mut project_requirements = collect_project_requirements(&modules, &library_manifest_index)?;
    let provider_plan = compilation_session.provider_plan_for_modules(&modules)?;
    let compiled_sdk_modules = CompiledSdkModules::from_provider_plan(&provider_plan);
    extend_requirements_with_provider_plan(&mut project_requirements, &provider_plan)?;
    let semantic_sdk_paths = semantic_sdk_path_dependencies(&project_requirements);
    let semantic = semantic_lock_state(
        &project_root,
        manifest.as_ref().and_then(ProjectManifest::oven_interop),
        compilation_session.sdk_inventory.as_deref(),
        compilation_session.sdk_components.as_ref(),
        package_feature_plan.as_ref(),
        &provider_plan,
        &semantic_sdk_paths,
    )
    .map_err(CliError::failure)?;
    // Artifact-owned stdlib modules resolve from checked metadata and are supplied by its linked Rust crate. Keep
    // them out of local emission so consumers cannot materialize a second `__incan_std` tree.
    let emitted_dep_modules: Vec<&ParsedModule> = dep_modules
        .iter()
        .filter(|module| !compiled_sdk_modules.contains_emission_path(&module.path_segments))
        .collect();

    // Derive project name (manifest overrides filename)
    let project_name = options
        .project_name_override
        .map(ToString::to_string)
        .unwrap_or_else(|| {
            manifest
                .as_ref()
                .and_then(|m| m.project.as_ref().and_then(|p| p.name.clone()))
                .unwrap_or_else(|| {
                    path.file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("incan_project")
                        .to_string()
                })
        });

    let out_dir = options
        .output_dir
        .map(|s| s.to_string())
        .unwrap_or_else(|| format!("target/incan/{}", project_name));

    // Validate output directory path to prevent path traversal
    validate_output_dir(&out_dir)?;

    // ---- Setup codegen ----
    let mut codegen = IrCodegen::new();
    codegen.set_preserve_dependency_public_items(false);
    codegen.set_registry_package_identity(Some(project_name.clone()));
    codegen.set_root_source_module_name(path.file_stem().and_then(|stem| stem.to_str()).map(str::to_string));
    if let Some(m) = manifest.as_ref() {
        codegen.set_declared_crate_names(m.declared_rust_crate_names());
    }
    codegen.set_provider_plan(Arc::clone(&provider_plan));
    for module in dep_modules
        .iter()
        .filter(|module| compiled_sdk_modules.contains_emission_path(&module.path_segments))
    {
        codegen.add_dependency_symbol_module_with_path_segments(
            &module.name,
            &module.ast,
            module.path_segments.clone(),
        );
    }
    // Add user dependency modules
    for module in &emitted_dep_modules {
        codegen.add_module_with_path_segments(&module.name, &module.ast, module.path_segments.clone());
    }
    // ---- Setup project generator ----
    let mut generator = ProjectGenerator::new(&out_dir, project_name.as_str(), true);
    if let Some(project) = manifest.as_ref().and_then(|manifest| manifest.project.as_ref()) {
        generator.set_package_metadata(project.version.clone(), project.license.clone());
    }
    generator.set_provider_plan(&provider_plan);
    generator.set_sdk_path_dependencies(project_requirements.sdk_path_dependencies.clone());
    generator.set_cargo_target_dir_override(options.generated_cargo_target_dir.map(Path::to_path_buf));
    generator.set_stdlib_features(project_requirements.stdlib_features.clone());
    generator.set_include_dev_dependencies(false);
    generator.set_rust_edition(
        manifest
            .as_ref()
            .and_then(|m| m.build.as_ref().and_then(|b| b.rust_edition.clone())),
    );

    let mut inline_imports = collect_rust_dependency_uses(main_module, false);
    for module in &emitted_dep_modules {
        inline_imports.extend(collect_rust_dependency_uses(module, false));
    }
    // RFC 023: Stdlib modules should not have inline rust imports (they use rust.module() + @rust.extern instead),
    // so we skip collecting from them.

    let cargo_features = CargoFeatureSelection {
        cargo_features,
        cargo_no_default_features,
        cargo_all_features,
    }
    .normalized();

    let mut resolved = match resolve_reachable_dependencies(manifest.as_ref(), &inline_imports, true, &cargo_features) {
        Ok(resolved) => resolved,
        Err(errors) => {
            let mut msg = String::new();
            let sources = build_source_map(&modules);
            for err in errors {
                msg.push_str(&format_dependency_error(&err, &sources));
            }
            return Err(CliError::failure(msg.trim_end()));
        }
    };
    merge_project_requirement_dependencies(&mut resolved, &project_requirements)?;
    #[cfg(feature = "rust_inspect")]
    let metadata_query_paths = collect_library_rust_abi_query_paths(&modules, &rust_extern_contexts);
    #[cfg(not(feature = "rust_inspect"))]
    let metadata_query_paths: Vec<String> = Vec::new();

    // Resolve lock payload before moving deps into generator (borrows resolved)
    let lock_resolution = resolve_lock_context(LockResolutionRequest {
        project_root: &project_root,
        project_name: project_name.as_str(),
        entry_file: Some(&normalized_file_path),
        manifest: manifest.as_ref(),
        resolved: &resolved,
        project_requirements: &project_requirements,
        cargo_features: &cargo_features,
        cargo_policy,
        semantic: Some(&semantic),
        package_features: Some(package_features),
        sdk_profile_override: options.sdk_profile_override,
    })?;
    let cargo_lock_inputs = lock_resolution.cargo_lock_authority.into_generator_inputs();
    let lock_payload = cargo_lock_inputs.payload;
    let cargo_lock_projection_root = cargo_lock_inputs.projection_root;
    let clear_cargo_lock = cargo_lock_inputs.clear_existing;
    let cargo_flags = cargo_command_flags(cargo_policy, &cargo_features);
    resolved = lock_resolution.resolved;
    project_requirements = lock_resolution.project_requirements;
    let cargo_package_name = lock_resolution.cargo_package_name;
    let managed_target = resolve_generated_cargo_target(
        options.generated_cargo_target_dir,
        &project_root,
        Path::new(&out_dir),
        &cargo_package_name,
        options.cargo_profile,
        lock_payload.as_deref(),
        &cargo_features,
        &cargo_flags,
    )
    .map_err(|error| CliError::failure(format!("failed to prepare generated Cargo cache: {error}")))?;
    let (managed_target_path, managed_target_lease, managed_target_identity) = managed_target.into_parts();
    #[cfg(feature = "rust_inspect")]
    let rust_inspect_target = resolve_generated_cargo_target(
        options.generated_cargo_target_dir,
        &project_root,
        &project_root,
        &cargo_package_name,
        "rust-inspect",
        lock_payload.as_deref(),
        &cargo_features,
        &cargo_flags,
    )
    .map_err(|error| CliError::failure(format!("failed to prepare rust-inspect Cargo cache: {error}")))?;
    #[cfg(feature = "rust_inspect")]
    let (rust_inspect_target_path, _rust_inspect_cache_lease, _rust_inspect_cache_identity) =
        rust_inspect_target.into_parts();
    generator.set_cargo_target_dir_override(Some(managed_target_path.clone()));
    generator.set_generated_cache_context(managed_target_lease, managed_target_identity);
    generator.set_package_name(Some(cargo_package_name.clone()));
    generator.set_stdlib_features(project_requirements.stdlib_features.clone());
    generator.set_include_dev_dependencies(lock_payload.is_some());
    #[cfg(feature = "rust_inspect")]
    let rust_inspect_manifest_dir = {
        let rust_inspect_manifest_dir = prepare_rust_inspect_workspace(RustInspectWorkspaceRequest {
            project_root: &project_root,
            project_name: project_name.as_str(),
            cargo_package_name: &cargo_package_name,
            rust_edition: manifest
                .as_ref()
                .and_then(|m| m.build.as_ref().and_then(|b| b.rust_edition.clone())),
            resolved: &resolved,
            project_requirements: &project_requirements,
            lock_payload: lock_payload.clone(),
            cargo_lock_projection_root: cargo_lock_projection_root.as_deref(),
            clear_cargo_lock,
            cargo_policy_flags: cargo_flags.clone(),
            cargo_target_dir: &rust_inspect_target_path,
            rust_inspect_query_paths: &metadata_query_paths,
            rust_derive_probe_paths: &collect_rust_inspect_derive_probe_paths(&modules),
            prepare_when_empty: true,
            direct_oven_inspection: false,
            force_direct_prewarm: false,
            oven_source_authority: None,
            prepared_project_source_authorities: None,
            explicit_oven_bake: false,
        })?
        .ok_or_else(|| CliError::failure("rust-inspect workspace preparation did not return a manifest directory"))?;
        codegen.set_rust_inspect_manifest_dir(rust_inspect_manifest_dir.manifest_dir().to_path_buf());
        Some(rust_inspect_manifest_dir)
    };

    // Type check all modules (dependencies + stdlib first), so diagnostics are associated with the correct file.
    //
    // This must run after rust-inspect preparation. Direct Rust calls expose their callable signatures through the
    // prepared metadata workspace; checking before that step degrades those calls to `Unknown` and breaks source-level
    // constructs such as `?` on Rust `Result<T, E>` returns.
    let compilation_analysis = compilation_session
        .analyze_modules(
            &modules,
            #[cfg(feature = "rust_inspect")]
            rust_inspect_manifest_dir
                .as_ref()
                .map(|workspace| workspace.manifest_dir()),
        )
        .map_err(|failure| CliError::failure(failure.render_human()))?;
    let main_type_info = compilation_analysis
        .type_info_for_path(&main_module.file_path)
        .cloned()
        .ok_or_else(|| {
            CliError::failure(format!(
                "missing session analysis for {}",
                main_module.file_path.display()
            ))
        })?;
    let mut dependency_type_info = HashMap::with_capacity(dep_modules.len());
    for module in dep_modules {
        let type_info = compilation_analysis
            .type_info_for_path(&module.file_path)
            .cloned()
            .ok_or_else(|| CliError::failure(format!("missing session analysis for {}", module.file_path.display())))?;
        dependency_type_info.insert(module.path_segments.clone(), type_info);
    }
    codegen.set_stdlib_cache(compilation_analysis.stdlib_cache().clone());
    codegen.set_prechecked_type_info(main_type_info, dependency_type_info);
    generator.set_cargo_lock_payload(lock_payload);
    generator.set_cargo_lock_projection_root(cargo_lock_projection_root);
    generator.set_clear_cargo_lock(clear_cargo_lock);

    generator.set_cargo_policy_flags(cargo_flags);

    generator.set_dependencies(resolved.dependencies);
    generator.set_dev_dependencies(resolved.dev_dependencies);

    // ---- Generate Rust project files ----
    let has_deps = !emitted_dep_modules.is_empty()
        || dep_modules
            .iter()
            .any(|module| compiled_sdk_modules.contains_emission_path(&module.path_segments));
    let _project_changed = if has_deps {
        let module_paths: Vec<Vec<String>> = emitted_dep_modules.iter().map(|m| m.path_segments.clone()).collect();
        let (main_code, rust_modules) = codegen
            .try_generate_multi_file_nested(&main_module.ast, &module_paths)
            .map_err(|e| CliError::failure(format!("Code generation error: {}", e)))?;

        generator
            .generate_nested(&main_code, &rust_modules)
            .map_err(|e| CliError::failure(format!("Error generating project: {}", e)))?
    } else {
        let rust_code = codegen
            .try_generate(&main_module.ast)
            .map_err(|e| CliError::failure(format!("Code generation error: {}", e)))?;
        generator
            .generate(&rust_code)
            .map_err(|e| CliError::failure(format!("Error generating project: {}", e)))?
    };

    Ok(())
}

/// Prepare an executable for Oven Alpha without launching Cargo, inspecting a Cargo target, or auto-publishing SDK
/// providers.
///
/// The generated Rust remains caller-owned so it can be inspected and regenerated normally. Reusable native inputs
/// are not derived from that directory: normal execution selects one matching direct-rustc provider/dependency plan
/// from the bounded Oven store. Incan-generated programs always link the standard runtime, so an empty plan is not a
/// meaningful normal-command fallback.
pub(crate) fn oven_build_unit_inputs(
    provider_plan: &ProviderPlan,
    requirements: &ProjectRequirements,
    resolved: &ResolvedDependencies,
) -> CliResult<BTreeMap<String, String>> {
    let provider_records = oven_native_provider_records(provider_plan, &semantic_sdk_path_dependencies(requirements))?;
    let mut dependencies = resolved.dependencies.clone();
    dependencies.extend(resolved.dev_dependencies.clone());
    let dependency_digest =
        digest_dependency_specs(&dependencies).map_err(|error| CliError::failure(error.to_string()))?;
    runtime_build_unit_inputs(provider_records, &requirements.stdlib_features, dependency_digest)
        .map_err(CliError::failure)
}

/// Add the exact selected interop execution receipt when this normal command targets a declared interop profile.
///
/// The portable lock remains the declaration authority; the small project-owned receipt proves which compatible
/// compiler and SDK Oven selected for that lock. A normal build only reads and revalidates this receipt. It neither
/// rediscovers a native toolchain nor tries Cargo when an interop-native plan is absent.
pub(crate) fn append_oven_interop_execution_build_inputs(
    build_inputs: &mut BTreeMap<String, String>,
    manifest: Option<&ProjectManifest>,
    target: &str,
) -> CliResult<()> {
    let Some(manifest) = manifest else {
        return Ok(());
    };
    let locked_targets = locked_oven_interop_targets(manifest)
        .map_err(|error| CliError::failure(format!("invalid locked Oven interop requirements: {error}")))?;
    if !locked_targets.iter().any(|candidate| candidate.target == target) {
        return Ok(());
    }
    // Reuse the command-side resolver here rather than treating a freshly recomputed declaration as sufficient.
    // It proves the package's current file receipts still equal the canonical standalone/workspace lock before a
    // normal consumer can select an immutable native plan.
    let locked = crate::cli::commands::interop_plan::locked_interop_plan_target(manifest.project_root(), target)?;
    let locked_target = &locked.target;
    let receipt_path = default_interop_execution_receipt_path(manifest.project_root(), target);
    let receipt = load_interop_execution_receipt(&receipt_path).map_err(|error| {
        CliError::failure(format!(
            "Oven interop target `{target}` has no current selected execution receipt at {}: {error}. Run the explicit `incan oven interop bake` command; normal build and run will not discover native tools or invoke Cargo.",
            receipt_path.display()
        ))
    })?;
    validate_interop_execution_receipt(locked_target, &receipt).map_err(|error| {
        CliError::failure(format!(
            "Oven interop target `{target}` has a stale selected execution receipt at {}: {error}. Re-run the explicit `incan oven interop bake` command; normal build and run will not fall back to Cargo.",
            receipt_path.display()
        ))
    })?;
    for (name, value) in interop_execution_build_unit_inputs(&receipt) {
        if build_inputs.insert(name.clone(), value).is_some() {
            return Err(CliError::failure(format!(
                "normal Oven build inputs already contain reserved interop key `{name}`"
            )));
        }
    }
    Ok(())
}

/// Encode only the compiler-owned SDK capabilities a generated native crate can exercise.
///
/// The active provider catalog contains every installed SDK component so semantic analysis can resolve imports
/// deterministically. An enabled but unused component contributes neither a generated Rust extern nor a selected
/// implementation facet. Retaining its identity in a Loaf receipt would let an unrelated provider relocation
/// prevent a safe compiler-owned Loaf match. Direct-link roots remain records even without a module claim because a
/// checked project-library projection can require their rlib explicitly.
pub(crate) fn oven_native_provider_records(
    provider_plan: &ProviderPlan,
    sdk_path_dependencies: &[DependencySpec],
) -> CliResult<Vec<String>> {
    let semantic_identities =
        provider_semantic_identities(provider_plan, sdk_path_dependencies).map_err(CliError::failure)?;
    let direct_sdk_link_roots = provider_plan
        .sdk_link_roots()
        .into_iter()
        .map(|provider| provider.identity.stable_key())
        .collect::<BTreeSet<_>>();
    let mut provider_records = Vec::new();
    for provider in provider_plan.active_sdk_records() {
        let used_modules = provider_plan
            .used_modules(provider)
            .into_iter()
            .map(|module| module.join("."))
            .collect::<Vec<_>>();
        let facets = provider_plan
            .selected_implementation_facets(provider)
            .into_iter()
            .map(|facet| facet.id.as_str())
            .collect::<Vec<_>>();
        let direct_link = direct_sdk_link_roots.contains(&provider.identity.stable_key());
        if used_modules.is_empty() && facets.is_empty() && !direct_link {
            continue;
        }
        let raw_identity = provider.identity.stable_key();
        let identity = semantic_identities.get(&raw_identity).ok_or_else(|| {
            CliError::failure(format!(
                "native provider compatibility identity is missing for `{raw_identity}`"
            ))
        })?;
        provider_records.push(format!(
            "{identity}|{}|{}|{}",
            used_modules.join(","),
            facets.join(","),
            if direct_link { "link" } else { "none" }
        ));
    }
    Ok(provider_records)
}

/// Resolve the direct-Rustc outputs of materialized caller-owned `pub::` dependencies.
///
/// These libraries are intentionally outside the immutable Loaf plan: they belong to the caller's project
/// graph, whereas that plan is restricted to compiler-owned SDK/runtime inputs. A prior Oven library materialization
/// establishes the caller-owned source and receipt boundary. Its old rlib is only an optional fast-path attachment:
/// a consumer can re-materialize that verified source under its selected direct-Rustc cohort when the old output is
/// absent or belongs to a different cohort. This is never a reason to invoke Cargo.
pub(crate) fn has_caller_owned_project_libraries(provider_plan: &ProviderPlan) -> bool {
    provider_plan.active_records().any(|provider| {
        matches!(
            provider.authority,
            crate::provider::NamespaceAuthority::ProjectDependency { .. }
        )
    })
}

/// Return the checked caller-owned `pub::` Rust libraries that an Oven consumer must attach directly.
pub(crate) fn oven_caller_owned_libraries(
    provider_plan: &ProviderPlan,
    profile: &str,
) -> CliResult<Vec<OvenCallerOwnedRustcLibrary>> {
    let mut libraries = Vec::new();
    for provider in provider_plan.active_records().filter(|provider| {
        matches!(
            provider.authority,
            crate::provider::NamespaceAuthority::ProjectDependency { .. }
        )
    }) {
        let artifact = provider.artifact.as_ref().ok_or_else(|| {
            CliError::failure(format!(
                "Oven Alpha cannot link pub::{} because its generated library artifact is unavailable",
                provider.identity.name
            ))
        })?;
        if artifact.kind != LibraryArtifactKind::Materialized {
            return Err(CliError::failure(format!(
                "Oven Alpha cannot link pub::{} from source-only metadata; run `incan build --lib` for that dependency to produce its caller-owned Oven library",
                artifact.dependency_key
            )));
        }
        let output = artifact.crate_root.join("oven").join(profile).join(format!(
            "lib{}.rlib",
            ProjectGenerator::rust_target_name(&artifact.manifest_name)
        ));
        if !output.is_file() {
            // `rematerialize_caller_owned_libraries` follows immediately after native-plan selection and rebuilds
            // the receipt-authorized generated source with that exact cohort. Do not turn a missing convenience
            // output into a Cargo fallback or a false prerequisite for a source that is already materialized.
            continue;
        }
        let digest = digest_bytes(&fs::read(&output).map_err(|source| {
            CliError::failure(format!(
                "Oven Alpha cannot read caller-owned Rust library {}: {source}",
                output.display()
            ))
        })?);
        libraries.push(OvenCallerOwnedRustcLibrary {
            crate_name: artifact.dependency_key.clone(),
            output,
            digest,
            expose_extern: true,
        });
    }
    libraries.sort_by(|left, right| left.crate_name.cmp(&right.crate_name));
    if libraries
        .windows(2)
        .any(|pair| pair[0].crate_name == pair[1].crate_name)
    {
        return Err(CliError::failure(
            "Oven Alpha resolved duplicate caller-owned Rust library crate names",
        ));
    }
    Ok(libraries)
}

/// Rebuild eligible caller-owned package libraries under a consumer's selected direct-Rustc cohort.
///
/// A prior `incan build --lib` output proves that the package was deliberately materialized, but its strict-version
/// hashes belong to the producer's old native plan. Attaching that rlib to a consumer selected with another complete
/// plan can therefore fail before the test harness runs. Rebuilding the producer's receipt-authorized generated
/// source with the already selected consumer plan keeps every direct extern in one Rustc cohort without making Cargo
/// a resolver or executor.
///
/// Compiler-owned private edges must already be part of the selected foundation plan. Public package edges follow
/// the separately receipt-authorized caller-owned recursion below; treating them as foundation inputs would widen a
/// selected Loaf with arbitrary package artifacts.
fn first_unselected_private_provider_edge<'a>(
    manifest: &'a LibraryManifest,
    artifact_plan: &OvenRustcArtifactPlan,
) -> Option<&'a ProviderDependencyMetadata> {
    manifest
        .contract_metadata
        .provider
        .provider_dependencies
        .iter()
        .find(|dependency| {
            dependency.kind == ProviderDependencyKind::PrivateImplementation
                && !artifact_plan
                    .externs
                    .iter()
                    .any(|(crate_name, _)| crate_name == &dependency.dependency_key)
        })
}

/// Exclude generated Cargo projection entries already supplied by a checked public provider edge.
///
/// A public `pub::` dependency is materialized from its digest-verified `.incnlib` graph above. Its generated Rust
/// projection also contains the same crate as a path dependency, but compiling that second projection would create a
/// distinct caller-owned rlib with the same Rust crate name. Keep the checked public graph authoritative while
/// letting every non-public Rust dependency continue through the direct-Rustc materializer.
fn caller_owned_library_dependencies_without_public_provider_edges(
    dependencies: Vec<DependencySpec>,
    manifest: &LibraryManifest,
) -> Vec<DependencySpec> {
    let public_keys = manifest
        .contract_metadata
        .provider
        .provider_dependencies
        .iter()
        .filter(|dependency| dependency.kind == ProviderDependencyKind::PublicPackage)
        .map(|dependency| dependency.dependency_key.replace('-', "_"))
        .collect::<BTreeSet<_>>();
    dependencies
        .into_iter()
        .filter(|dependency| !public_keys.contains(&dependency.crate_name.replace('-', "_")))
        .collect()
}

/// Collapse identical caller-owned artifacts while retaining the strongest direct-extern requirement.
///
/// A public provider can pass one artifact upward as transitive while its parent names that exact artifact directly.
/// Both search-path records identify the same bytes, but the direct parent still needs a `--extern` binding to compile.
/// Prefer that binding only for an identical crate/output pair; distinct outputs remain visible as an ambiguity.
fn deduplicate_caller_owned_libraries_prefer_extern(libraries: &mut Vec<OvenCallerOwnedRustcLibrary>) {
    libraries.sort_by(|left, right| {
        left.crate_name
            .cmp(&right.crate_name)
            .then_with(|| left.output.cmp(&right.output))
            .then_with(|| right.expose_extern.cmp(&left.expose_extern))
    });
    libraries.dedup_by(|left, right| left.crate_name == right.crate_name && left.output == right.output);
}

/// Load one public provider edge from the parent artifact's checked, relocation-safe projection.
///
/// The parent manifest supplies both the exact artifact-tree digest and the expected provider identity. Resolving the
/// relative path therefore does not restore normal dependency discovery: a child is admissible only when all three
/// values agree, and the normal artifact loader confirms its generated source and Cargo projection are complete.
fn load_receipted_public_provider_dependency(
    parent: &LibraryArtifactMetadata,
    dependency: &ProviderDependencyMetadata,
) -> CliResult<(LibraryManifest, LibraryArtifactMetadata)> {
    let candidate = parent.crate_root.join(&dependency.relative_artifact_path);
    let actual_digest = digest_provider_artifact(&candidate).map_err(|error| {
        CliError::failure(format!(
            "Oven Alpha cannot re-materialize pub::{} because provider edge `{}` has an invalid artifact at {}: {error}",
            parent.dependency_key,
            dependency.dependency_key,
            candidate.display()
        ))
    })?;
    if actual_digest != dependency.artifact_digest {
        return Err(CliError::failure(format!(
            "Oven Alpha refuses provider edge `{}` below pub::{}: artifact digest {actual_digest} does not match its checked manifest identity {}",
            dependency.dependency_key, parent.dependency_key, dependency.artifact_digest
        )));
    }
    let entry = load_provider_dependency_artifact(&dependency.dependency_key, &candidate);
    let (manifest, metadata) = match entry {
        LibraryManifestIndexEntry::Loaded { manifest, metadata } => (*manifest, metadata),
        LibraryManifestIndexEntry::Failed(failure) => {
            return Err(CliError::failure(format!(
                "Oven Alpha cannot re-materialize provider edge `{}` below pub::{}: {failure}",
                dependency.dependency_key, parent.dependency_key
            )));
        }
    };
    if manifest.name != dependency.provider_name || manifest.version != dependency.provider_version {
        return Err(CliError::failure(format!(
            "Oven Alpha refuses provider edge `{}` below pub::{}: checked identity {}@{} differs from discovered {}@{}",
            dependency.dependency_key,
            parent.dependency_key,
            dependency.provider_name,
            dependency.provider_version,
            manifest.name,
            manifest.version
        )));
    }
    if metadata.kind != LibraryArtifactKind::Materialized {
        return Err(CliError::failure(format!(
            "Oven Alpha cannot re-materialize provider edge `{}` below pub::{} from parser-only metadata",
            dependency.dependency_key, parent.dependency_key
        )));
    }
    Ok((manifest, metadata))
}

/// Read and verify the producer receipt that authorizes one caller-owned generated library source.
fn caller_owned_library_receipt(
    artifact: &LibraryArtifactMetadata,
    profile: &str,
    artifacts: &OvenRustcArtifactManifest,
    authority_context: Option<&mut OvenProjectBakeAuthorityContext>,
) -> CliResult<crate::oven::OvenReceipt> {
    let project_root = artifact.crate_root.parent().and_then(Path::parent).ok_or_else(|| {
        CliError::failure(format!(
            "Oven Alpha cannot locate the project root for pub::{} from generated artifact root {}",
            artifact.dependency_key,
            artifact.crate_root.display()
        ))
    })?;
    let selected_package = if let Some(context) = authority_context {
        context
            .checked_packaged_library_loaf_profiles(
                artifact,
                &[profile],
                &artifacts.intent.target,
                &artifacts.intent.toolchain,
            )?
            .and_then(|mut profiles| profiles.pop())
    } else if let Some(package) = read_packaged_library_loaf_manifest(artifact)? {
        validated_packaged_library_loaf_profile(
            artifact,
            &package,
            profile,
            &artifacts.intent.target,
            &artifacts.intent.toolchain,
        )?
    } else {
        None
    };
    if packaged_library_loaf_manifest_path(&artifact.crate_root).is_file() {
        let selected = selected_package.ok_or_else(|| {
            CliError::failure(format!(
                "Oven Alpha has no compatible `{profile}` package receipt for pub::{} in the selected direct-Rustc cohort",
                artifact.dependency_key
            ))
        })?;
        return Ok(selected.receipt);
    }
    let receipt_path = project_bake_receipt_path(
        project_root,
        OvenBakeProjectTarget::Library,
        &project_root.join(OvenBakeProjectTarget::Library.source_relative_path()),
        profile,
    )?;
    let receipt = match fs::read(&receipt_path) {
        Ok(receipt_bytes) => {
            let receipt = serde_json::from_slice::<crate::oven::OvenReceipt>(&receipt_bytes).map_err(|error| {
                CliError::failure(format!(
                    "Oven Alpha cannot parse the `{profile}` library receipt for pub::{} at {}: {error}",
                    artifact.dependency_key,
                    receipt_path.display()
                ))
            })?;
            receipt.verify_identity().map_err(|error| {
                CliError::failure(format!(
                    "Oven Alpha refuses pub::{} because its library receipt at {} is invalid: {error}",
                    artifact.dependency_key,
                    receipt_path.display()
                ))
            })?;
            receipt
        }
        Err(error)
            if error.kind() == std::io::ErrorKind::NotFound
                && !project_root.join(crate::manifest::MANIFEST_FILENAME).is_file() =>
        {
            mint_artifact_only_library_receipt(artifact, project_root, profile, &artifacts.intent, &receipt_path)?
        }
        Err(error) => {
            return Err(CliError::failure(format!(
                "Oven Alpha cannot read the `{profile}` library receipt for pub::{} at {}: {error}",
                artifact.dependency_key,
                receipt_path.display()
            )));
        }
    };
    if receipt.intent != artifacts.intent {
        return rebind_caller_owned_library_receipt(artifact, profile, &artifacts.intent, &receipt);
    }
    Ok(receipt)
}

/// Bind verified caller-owned provider source to the consumer's selected direct-Rustc cohort.
///
/// A producer receipt names the compiler cohort that originally materialized a library. A downstream consumer may
/// legitimately select a newer compatible toolchain or a different feature-unified project Loaf, so retaining that
/// historical intent would prevent the promised source re-materialization. The replacement receipt is in-memory and
/// records both the complete current artifact digest and the verified producer receipt identity; it does not mutate
/// the provider's stored receipt or invoke Cargo.
fn rebind_caller_owned_library_receipt(
    artifact: &LibraryArtifactMetadata,
    profile: &str,
    intent: &crate::oven::OvenBuildIntent,
    producer_receipt: &crate::oven::OvenReceipt,
) -> CliResult<crate::oven::OvenReceipt> {
    if intent.profile != profile {
        return Err(CliError::failure(format!(
            "Oven Alpha cannot bind the `{profile}` provider receipt for pub::{} to selected `{}` artifacts",
            artifact.dependency_key, intent.profile
        )));
    }
    let manifest = LibraryManifest::read_from_path(&artifact.manifest_path).map_err(|error| {
        CliError::failure(format!(
            "Oven Alpha cannot read the checked artifact manifest for pub::{} at {}: {error}",
            artifact.dependency_key,
            artifact.manifest_path.display()
        ))
    })?;
    let artifact_digest = digest_provider_artifact(&artifact.crate_root).map_err(|error| {
        CliError::failure(format!(
            "Oven Alpha cannot fingerprint re-materialized pub::{} artifact at {}: {error}",
            artifact.dependency_key,
            artifact.crate_root.display()
        ))
    })?;
    let receipt_request = OvenGeneratedProjectRequest::new(
        &artifact.crate_root,
        manifest.name,
        manifest.version,
        intent.target.clone(),
        intent.toolchain.clone(),
        profile,
        intent.features.clone(),
    )
    .with_generated_source("generated-root", &artifact.crate_lib_path)
    .with_generated_source_tree("generated-source-tree", artifact.crate_root.join("src"))
    .with_generated_source("provider-contract", &artifact.manifest_path)
    .with_build_unit_input("caller-owned-provider-digest", artifact_digest)
    .with_build_unit_input("producer-library-receipt", producer_receipt.identity.clone());
    receipt_generated_project(&receipt_request).map_err(|error| CliError::failure(error.to_string()))
}

/// Mint a local receipt for a checked source-free provider artifact without invoking Cargo.
///
/// A portable `.incnlib` package can intentionally ship only its generated Rust projection, so it has no project
/// manifest from which a nested `incan build --lib` could create a receipt. Its checked provider manifest, generated
/// source tree, and complete artifact digest are sufficient authority for a receipt bound to the already-selected
/// direct-Rustc intent. Source-backed dependencies still use the normal nested Oven library preparation path.
fn mint_artifact_only_library_receipt(
    artifact: &LibraryArtifactMetadata,
    project_root: &Path,
    profile: &str,
    intent: &crate::oven::OvenBuildIntent,
    receipt_path: &Path,
) -> CliResult<crate::oven::OvenReceipt> {
    if intent.profile != profile {
        return Err(CliError::failure(format!(
            "Oven Alpha cannot mint the `{profile}` receipt for pub::{} because the selected direct-Rustc plan uses `{}`",
            artifact.dependency_key, intent.profile
        )));
    }
    let manifest = LibraryManifest::read_from_path(&artifact.manifest_path).map_err(|error| {
        CliError::failure(format!(
            "Oven Alpha cannot read the checked artifact manifest for pub::{} at {}: {error}",
            artifact.dependency_key,
            artifact.manifest_path.display()
        ))
    })?;
    let source_tree = artifact.crate_root.join("src");
    let artifact_digest = digest_provider_artifact(&artifact.crate_root).map_err(|error| {
        CliError::failure(format!(
            "Oven Alpha cannot fingerprint source-free pub::{} artifact at {}: {error}",
            artifact.dependency_key,
            artifact.crate_root.display()
        ))
    })?;
    let receipt_request = OvenGeneratedProjectRequest::new(
        project_root,
        manifest.name,
        manifest.version,
        intent.target.clone(),
        intent.toolchain.clone(),
        profile,
        intent.features.clone(),
    )
    .with_generated_source("generated-root", &artifact.crate_lib_path)
    .with_generated_source_tree("generated-source-tree", source_tree)
    .with_generated_source("provider-contract", &artifact.manifest_path)
    .with_build_unit_input("artifact-only-provider-digest", artifact_digest);
    let receipt = receipt_generated_project(&receipt_request).map_err(|error| CliError::failure(error.to_string()))?;
    write_receipt(&receipt, receipt_path).map_err(|error| CliError::failure(error.to_string()))?;
    Ok(receipt)
}

/// Parse the edition directly from a generated provider's checked Cargo projection.
fn caller_owned_library_edition(artifact: &LibraryArtifactMetadata) -> CliResult<String> {
    if !artifact.crate_lib_path.is_file() {
        return Err(CliError::failure(format!(
            "Oven Alpha cannot re-materialize pub::{} because its generated library source is absent at {}",
            artifact.dependency_key,
            artifact.crate_lib_path.display()
        )));
    }
    let cargo_manifest = fs::read_to_string(&artifact.cargo_toml_path).map_err(|error| {
        CliError::failure(format!(
            "Oven Alpha cannot read the generated library manifest for pub::{} at {}: {error}",
            artifact.dependency_key,
            artifact.cargo_toml_path.display()
        ))
    })?;
    let cargo_manifest = toml::from_str::<toml::Value>(&cargo_manifest).map_err(|error| {
        CliError::failure(format!(
            "Oven Alpha cannot parse the generated library manifest for pub::{} at {}: {error}",
            artifact.dependency_key,
            artifact.cargo_toml_path.display()
        ))
    })?;
    cargo_manifest
        .get("package")
        .and_then(toml::Value::as_table)
        .and_then(|package| package.get("edition"))
        .and_then(toml::Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| {
            CliError::failure(format!(
                "Oven Alpha cannot re-materialize pub::{} because its generated library manifest has no package edition",
                artifact.dependency_key
            ))
        })
}

/// Determine whether the checked generated provider must be compiled as a procedural macro.
///
/// This is manifest interpretation only: it selects a direct-`rustc` crate type and never invokes Cargo or accepts
/// target-conditional metadata. A malformed value fails closed rather than being treated as an ordinary library.
fn caller_owned_library_is_proc_macro(artifact: &LibraryArtifactMetadata) -> CliResult<bool> {
    let manifest_text = fs::read_to_string(&artifact.cargo_toml_path).map_err(|error| {
        CliError::failure(format!(
            "Oven Alpha cannot read the generated library manifest for pub::{} at {}: {error}",
            artifact.dependency_key,
            artifact.cargo_toml_path.display()
        ))
    })?;
    let manifest = toml::from_str::<toml::Value>(&manifest_text).map_err(|error| {
        CliError::failure(format!(
            "Oven Alpha cannot parse the generated library manifest for pub::{} at {}: {error}",
            artifact.dependency_key,
            artifact.cargo_toml_path.display()
        ))
    })?;
    let Some(lib) = manifest.get("lib") else {
        return Ok(false);
    };
    let lib = lib.as_table().ok_or_else(|| {
        CliError::failure(format!(
            "Oven Alpha cannot re-materialize pub::{} because {} has a non-table [lib] declaration",
            artifact.dependency_key,
            artifact.cargo_toml_path.display()
        ))
    })?;
    lib.get("proc-macro")
        .map(|value| {
            value.as_bool().ok_or_else(|| {
                CliError::failure(format!(
                    "Oven Alpha cannot re-materialize pub::{} because {} has a non-boolean lib.proc-macro declaration",
                    artifact.dependency_key,
                    artifact.cargo_toml_path.display()
                ))
            })
        })
        .transpose()
        .map(|value| value.unwrap_or(false))
}

/// Recover the narrow Rust dependency closure required to compile a generated provider library.
///
/// The generated `Cargo.toml` remains a checked projection of the provider artifact, not an instruction to run
/// Cargo. Oven reads only its unconditional library dependencies and converts them to the existing direct-Rustc
/// dependency representation. Target-conditional, workspace-inherited, Git, and malformed declarations fail closed
/// because selecting those semantics would reintroduce an unreceipted resolver policy.
fn caller_owned_library_rust_dependencies(artifact: &LibraryArtifactMetadata) -> CliResult<Vec<DependencySpec>> {
    let manifest_text = fs::read_to_string(&artifact.cargo_toml_path).map_err(|error| {
        CliError::failure(format!(
            "Oven Alpha cannot read the generated library manifest for pub::{} at {}: {error}",
            artifact.dependency_key,
            artifact.cargo_toml_path.display()
        ))
    })?;
    let manifest = toml::from_str::<toml::Value>(&manifest_text).map_err(|error| {
        CliError::failure(format!(
            "Oven Alpha cannot parse the generated library manifest for pub::{} at {}: {error}",
            artifact.dependency_key,
            artifact.cargo_toml_path.display()
        ))
    })?;
    if manifest.get("target").is_some() {
        return Err(CliError::failure(format!(
            "Oven Alpha cannot re-materialize pub::{} because {} declares target-conditional Rust dependencies; prepare an explicit Oven-native closure",
            artifact.dependency_key,
            artifact.cargo_toml_path.display()
        )));
    }
    let manifest_directory = artifact.cargo_toml_path.parent().ok_or_else(|| {
        CliError::failure(format!(
            "Oven Alpha cannot determine the generated library directory for pub::{} at {}",
            artifact.dependency_key,
            artifact.cargo_toml_path.display()
        ))
    })?;
    let mut dependencies = BTreeMap::new();
    let Some(dependency_table) = manifest.get("dependencies").and_then(toml::Value::as_table) else {
        return Ok(Vec::new());
    };
    for (crate_name, value) in dependency_table {
        let dependency = match value {
            toml::Value::String(version) => DependencySpec {
                crate_name: crate_name.clone(),
                version: Some(version.clone()),
                features: Vec::new(),
                default_features: true,
                source: DependencySource::Registry,
                optional: false,
                package: None,
            },
            toml::Value::Table(table) => {
                if table.get("workspace").and_then(toml::Value::as_bool) == Some(true) {
                    return Err(CliError::failure(format!(
                        "Oven Alpha cannot re-materialize pub::{} because dependency `{crate_name}` inherits a Cargo workspace declaration; prepare an explicit Oven-native closure",
                        artifact.dependency_key
                    )));
                }
                if table.get("git").is_some() {
                    return Err(CliError::failure(format!(
                        "Oven Alpha cannot re-materialize pub::{} because dependency `{crate_name}` is Git-sourced; prepare an explicit Oven-native closure",
                        artifact.dependency_key
                    )));
                }
                let features = table
                    .get("features")
                    .map(|features| {
                        features
                            .as_array()
                            .ok_or_else(|| {
                                CliError::failure(format!(
                                    "Oven Alpha cannot re-materialize pub::{} because dependency `{crate_name}` has a non-array feature declaration",
                                    artifact.dependency_key
                                ))
                            })?
                            .iter()
                            .map(|feature| {
                                feature.as_str().map(str::to_string).ok_or_else(|| {
                                    CliError::failure(format!(
                                        "Oven Alpha cannot re-materialize pub::{} because dependency `{crate_name}` has a non-string feature",
                                        artifact.dependency_key
                                    ))
                                })
                            })
                            .collect::<CliResult<Vec<_>>>()
                    })
                    .transpose()?
                    .unwrap_or_default();
                let package = table
                    .get("package")
                    .map(|package| {
                        package.as_str().map(str::to_string).ok_or_else(|| {
                            CliError::failure(format!(
                                "Oven Alpha cannot re-materialize pub::{} because dependency `{crate_name}` has a non-string package alias",
                                artifact.dependency_key
                            ))
                        })
                    })
                    .transpose()?;
                let default_features = table
                    .get("default-features")
                    .map(|value| {
                        value.as_bool().ok_or_else(|| {
                            CliError::failure(format!(
                                "Oven Alpha cannot re-materialize pub::{} because dependency `{crate_name}` has a non-boolean default-features value",
                                artifact.dependency_key
                            ))
                        })
                    })
                    .transpose()?
                    .unwrap_or(true);
                let optional = table
                    .get("optional")
                    .map(|value| {
                        value.as_bool().ok_or_else(|| {
                            CliError::failure(format!(
                                "Oven Alpha cannot re-materialize pub::{} because dependency `{crate_name}` has a non-boolean optional value",
                                artifact.dependency_key
                            ))
                        })
                    })
                    .transpose()?
                    .unwrap_or(false);
                let version = table
                    .get("version")
                    .map(|value| {
                        value.as_str().map(str::to_string).ok_or_else(|| {
                            CliError::failure(format!(
                                "Oven Alpha cannot re-materialize pub::{} because dependency `{crate_name}` has a non-string version",
                                artifact.dependency_key
                            ))
                        })
                    })
                    .transpose()?;
                let source = match table.get("path") {
                    Some(path) => {
                        let path = path.as_str().ok_or_else(|| {
                            CliError::failure(format!(
                                "Oven Alpha cannot re-materialize pub::{} because dependency `{crate_name}` has a non-string path",
                                artifact.dependency_key
                            ))
                        })?;
                        DependencySource::Path {
                            path: manifest_directory.join(path),
                        }
                    }
                    None => {
                        if version.is_none() {
                            return Err(CliError::failure(format!(
                                "Oven Alpha cannot re-materialize pub::{} because registry dependency `{crate_name}` has no version requirement",
                                artifact.dependency_key
                            )));
                        }
                        DependencySource::Registry
                    }
                };
                DependencySpec {
                    crate_name: crate_name.clone(),
                    version,
                    features,
                    default_features,
                    source,
                    optional,
                    package,
                }
            }
            _ => {
                return Err(CliError::failure(format!(
                    "Oven Alpha cannot re-materialize pub::{} because dependency `{crate_name}` has an unsupported Cargo manifest shape",
                    artifact.dependency_key
                )));
            }
        }
        .normalized();
        if let Some(existing) = dependencies.insert(crate_name.clone(), dependency.clone())
            && existing != dependency
        {
            return Err(CliError::failure(format!(
                "Oven Alpha cannot re-materialize pub::{} because generated manifest dependency `{crate_name}` is ambiguous",
                artifact.dependency_key
            )));
        }
    }
    Ok(dependencies.into_values().collect())
}

/// Omit the generated Cargo projection's unconditional derive support when its Rust source does not invoke it.
///
/// Every generated package declares the compiler-owned `incan_derive` path dependency, but direct `rustc` needs a
/// procedural macro only when the generated source names it. Treating an unused declaration as a caller-owned
/// dependency would recursively resolve the macro's private registry build closure, even though the provider itself
/// is being rebuilt only to share the consumer's already selected runtime cohort.
fn caller_owned_library_dependencies_without_unused_incan_derive(
    artifact: &LibraryArtifactMetadata,
    dependencies: Vec<DependencySpec>,
) -> CliResult<Vec<DependencySpec>> {
    let source = fs::read_to_string(&artifact.crate_lib_path).map_err(|error| {
        CliError::failure(format!(
            "Oven Alpha cannot read generated provider source for pub::{} at {}: {error}",
            artifact.dependency_key,
            artifact.crate_lib_path.display()
        ))
    })?;
    Ok(dependencies
        .into_iter()
        .filter(|dependency| dependency.crate_name != "incan_derive" || source.contains("incan_derive"))
        .collect())
}

/// Validate every required package profile once for one consumer preparation.
///
/// A public provider is an independently baked unit. Adding its registry dependencies to every consumer selection
/// would rebuild DataFusion (or any analogous provider closure) under that consumer's plan. Instead, an explicit
/// provider bake seals those dependencies once; a consumer selection retains only its own Rust surface and composes
/// the verified provider Loaf later. The checked records are command-local and flow through import and selection,
/// avoiding repeated recursive source scans without introducing a cross-command cache.
fn checked_packaged_provider_profiles(
    provider_plan: &ProviderPlan,
    profiles: &[&str],
    target: &str,
    toolchain: &str,
    mut authority_context: Option<&mut OvenProjectBakeAuthorityContext>,
) -> CliResult<Vec<CheckedPackagedProviderProfile>> {
    let mut checked = Vec::new();
    for provider in provider_plan.active_records().filter(|provider| {
        matches!(
            provider.authority,
            crate::provider::NamespaceAuthority::ProjectDependency { .. }
        )
    }) {
        let artifact = provider.artifact.as_ref().ok_or_else(|| {
            CliError::failure(format!(
                "Oven Alpha cannot select pub::{} because its generated library artifact is unavailable",
                provider.identity.name
            ))
        })?;
        if artifact.kind != LibraryArtifactKind::Materialized {
            return Err(CliError::failure(format!(
                "Oven Alpha requires an explicit package Loaf for pub::{}; bake that provider with `incan oven bake --project {}` before baking this consumer",
                artifact.dependency_key,
                artifact
                    .crate_root
                    .parent()
                    .and_then(Path::parent)
                    .unwrap_or(&artifact.crate_root)
                    .display()
            )));
        }
        let packages = if let Some(context) = authority_context.as_deref_mut() {
            context.checked_packaged_library_loaf_profiles(artifact, profiles, target, toolchain)?
        } else {
            let Some(manifest) = read_packaged_library_loaf_manifest(artifact)? else {
                return Err(CliError::failure(format!(
                    "Oven Alpha requires an explicit package Loaf for pub::{}; bake that provider with `incan oven bake --project {}` before baking this consumer",
                    artifact.dependency_key,
                    artifact
                        .crate_root
                        .parent()
                        .and_then(Path::parent)
                        .unwrap_or(&artifact.crate_root)
                        .display()
                )));
            };
            let mut selected = Vec::with_capacity(profiles.len());
            for profile in profiles {
                let Some(package) =
                    validated_packaged_library_loaf_profile(artifact, &manifest, profile, target, toolchain)?
                else {
                    return Err(CliError::failure(format!(
                        "Oven Alpha has no compatible `{profile}` package Loaf for pub::{} (target `{target}`, toolchain `{toolchain}`); bake that provider with the active Incan release before baking this consumer",
                        artifact.dependency_key
                    )));
                };
                selected.push(package);
            }
            Some(selected)
        };
        let Some(packages) = packages else {
            let requested_profiles = profiles.join("`, `");
            return Err(CliError::failure(format!(
                "Oven Alpha has no compatible `{requested_profiles}` package Loaf for pub::{} (target `{target}`, toolchain `{toolchain}`); bake that provider with the active Incan release before baking this consumer",
                artifact.dependency_key
            )));
        };
        for (profile, package) in profiles.iter().zip(packages) {
            checked.push(CheckedPackagedProviderProfile {
                dependency_key: artifact.dependency_key.clone(),
                artifact_root: artifact.crate_root.clone(),
                profile: (*profile).to_string(),
                package,
            });
        }
    }
    Ok(checked)
}

/// Validate package-provider roots from the complete project dependency surface, including test-only imports.
///
/// Conventional main/library provider plans do not necessarily observe a package imported only by an owned test.
/// The canonical lock surface does: compiled Incan packages appear there as path dependencies rooted at their
/// generated artifact. An adjacent package-Loaf manifest is the explicit ownership marker; ordinary Rust path crates
/// remain in the publisher delta.
fn checked_test_dependency_package_profiles(
    dependencies: &[DependencySpec],
    receipt: &crate::oven::OvenReceipt,
    mut authority_context: Option<&mut OvenProjectBakeAuthorityContext>,
) -> CliResult<Vec<CheckedPackagedProviderProfile>> {
    let mut checked = Vec::new();
    for dependency in dependencies {
        let DependencySource::Path { path } = &dependency.source else {
            continue;
        };
        let manifest_name = dependency.package.as_deref().unwrap_or(&dependency.crate_name);
        let artifact =
            LibraryArtifactMetadata::from_crate_root(dependency.crate_name.clone(), manifest_name.to_string(), path);
        if !packaged_library_loaf_manifest_path(&artifact.crate_root).is_file() {
            continue;
        }
        let package = if let Some(context) = authority_context.as_deref_mut() {
            context
                .checked_packaged_library_loaf_profiles(
                    &artifact,
                    &["debug"],
                    &receipt.intent.target,
                    &receipt.intent.toolchain,
                )?
                .and_then(|mut profiles| profiles.pop())
        } else {
            let manifest = read_packaged_library_loaf_manifest(&artifact)?.ok_or_else(|| {
                CliError::failure(format!(
                    "Oven Alpha cannot validate the package Loaf declared by test dependency `{}`",
                    dependency.crate_name
                ))
            })?;
            validated_packaged_library_loaf_profile(
                &artifact,
                &manifest,
                "debug",
                &receipt.intent.target,
                &receipt.intent.toolchain,
            )?
        }
        .ok_or_else(|| {
            CliError::failure(format!(
                "Oven Alpha has no compatible `debug` package Loaf for test dependency `{}`; rebake that provider with the active Incan release before baking this consumer",
                dependency.crate_name
            ))
        })?;
        checked.push(CheckedPackagedProviderProfile {
            dependency_key: artifact.dependency_key,
            artifact_root: artifact.crate_root,
            profile: "debug".to_string(),
            package,
        });
    }
    checked.sort_by(|left, right| left.dependency_key.cmp(&right.dependency_key));
    checked.dedup_by(|left, right| left.dependency_key == right.dependency_key);
    Ok(checked)
}

/// Materialize every already baked provider Loaf into the consumer's bounded store at the explicit bake boundary.
fn import_packaged_provider_loafs_for_explicit_bake(
    mode: OvenProjectPlanMode,
    consumer_store: &OvenStore,
    checked_profiles: &[CheckedPackagedProviderProfile],
) -> CliResult<()> {
    if !mode.is_explicit_publisher() {
        return Ok(());
    }
    for checked in checked_profiles {
        import_checked_packaged_library_loaf(consumer_store, checked)?;
    }
    Ok(())
}

/// Follow the historical digest-verified public-provider graph for targeted migration coverage.
///
/// Production selection now requires one baked package Loaf for each public provider, so it no longer walks this
/// graph or rebuilds its registry closure from every consumer. The helper remains only to preserve direct coverage
/// of the checked graph traversal itself.
#[cfg(test)]
fn collect_caller_owned_project_rust_dependencies(
    artifact: &LibraryArtifactMetadata,
    manifest: &LibraryManifest,
    visiting: &mut BTreeSet<PathBuf>,
    dependencies: &mut Vec<DependencySpec>,
) -> CliResult<()> {
    let canonical_root = fs::canonicalize(&artifact.crate_root).map_err(|error| {
        CliError::failure(format!(
            "Oven Alpha cannot canonicalize generated artifact root for pub::{} at {}: {error}",
            artifact.dependency_key,
            artifact.crate_root.display()
        ))
    })?;
    if !visiting.insert(canonical_root.clone()) {
        return Err(CliError::failure(format!(
            "Oven Alpha refuses a cyclic public provider graph while selecting pub::{} at {}",
            artifact.dependency_key,
            canonical_root.display()
        )));
    }
    let result = (|| {
        for dependency in manifest
            .contract_metadata
            .provider
            .provider_dependencies
            .iter()
            .filter(|dependency| dependency.kind == ProviderDependencyKind::PublicPackage)
        {
            let (nested_manifest, nested_artifact) = load_receipted_public_provider_dependency(artifact, dependency)?;
            collect_caller_owned_project_rust_dependencies(&nested_artifact, &nested_manifest, visiting, dependencies)?;
        }

        let provider_dependencies = caller_owned_library_rust_dependencies(artifact)?;
        let provider_dependencies =
            caller_owned_library_dependencies_without_public_provider_edges(provider_dependencies, manifest);
        for dependency in provider_dependencies {
            merge_oven_dependency_surface(dependencies, dependency, &artifact.dependency_key)?;
        }
        Ok(())
    })();
    visiting.remove(&canonical_root);
    result
}

/// Merge Cargo-unifiable requirements while retaining a fail-closed identity boundary.
#[cfg(test)]
fn merge_oven_dependency_surface(
    dependencies: &mut Vec<DependencySpec>,
    candidate: DependencySpec,
    provider: &str,
) -> CliResult<()> {
    if let Some(existing) = dependencies
        .iter_mut()
        .find(|dependency| dependency.crate_name == candidate.crate_name)
    {
        let mut existing_identity = existing.clone();
        let mut candidate_identity = candidate.clone();
        for dependency in [&mut existing_identity, &mut candidate_identity] {
            dependency.features.clear();
            dependency.default_features = false;
            dependency.optional = false;
        }
        if !dependency_specs_match(&existing_identity, &candidate_identity) {
            return Err(CliError::failure(format!(
                "Oven Alpha cannot unify caller-owned dependency `{}` required by pub::{provider}; align its source, version, and package identity before baking an explicit project Loaf",
                candidate.crate_name
            )));
        }
        existing.features.extend(candidate.features);
        existing.features.sort();
        existing.features.dedup();
        existing.default_features |= candidate.default_features;
        existing.optional &= candidate.optional;
        return Ok(());
    }
    dependencies.push(candidate);
    Ok(())
}

/// Variant with explicit scheduler-owned roots so the authority rule is independently testable.
fn caller_owned_library_dependencies_missing_from_selected_plan_with_owned_roots(
    dependencies: &[DependencySpec],
    artifact_plan: &OvenRustcArtifactPlan,
    owned_roots: &[PathBuf],
) -> Vec<DependencySpec> {
    let selected_externs = artifact_plan
        .externs
        .iter()
        .map(|(crate_name, _)| crate_name.as_str())
        .collect::<BTreeSet<_>>();
    dependencies
        .iter()
        .filter(|dependency| match dependency.source {
            DependencySource::Registry => !selected_externs.contains(dependency.crate_name.replace('-', "_").as_str()),
            DependencySource::Path { .. } => {
                !is_selected_compiler_runtime_path_dependency(dependency, &selected_externs, owned_roots)
            }
            DependencySource::Git { .. } => true,
        })
        .cloned()
        .collect()
}

/// Return the immutable roots supplied by the compiler-suite scheduler.
fn compiler_suite_owned_roots() -> Vec<PathBuf> {
    if env::var_os("INCAN_INTERNAL_OVEN_LOAF_EXECUTION").is_none_or(|value| value != "1") {
        return Vec::new();
    }
    [
        "INCAN_INTERNAL_TOOLCHAIN_DATA_ROOT",
        "INCAN_INTERNAL_OVEN_RUNTIME_ROOT",
        "INCAN_INTERNAL_SDK_PROVIDER_STORE",
    ]
    .into_iter()
    .filter_map(env::var_os)
    .filter(|value| !value.is_empty())
    .map(PathBuf::from)
    .filter(|path| path.is_dir())
    .filter_map(|path| fs::canonicalize(path).ok())
    .collect()
}

/// Return compiler-owned path roots that may pair with an exact selected plan extern.
///
/// Besides a scheduler's sealed data roots, a normal command may reuse an active toolchain crate only when that
/// exact crate is exposed by its receipt-selected plan. Project paths and lookalike crates remain caller-owned.
fn compiler_owned_roots(artifact_plan: &OvenRustcArtifactPlan) -> Vec<PathBuf> {
    let mut roots = compiler_suite_owned_roots();
    for (crate_name, _) in &artifact_plan.externs {
        let candidate = crate::toolchain_layout::resolve_toolchain_crate_path(crate_name);
        if candidate.join("Cargo.toml").is_file()
            && let Ok(canonical) = fs::canonicalize(candidate)
        {
            roots.push(canonical);
        }
    }
    roots.sort();
    roots.dedup();
    roots
}

/// Extend compiler-source authority with receipt-selected SDK/provider artifact roots.
///
/// A normal command may receive an SDK component's generated Cargo projection as a path dependency. That source
/// root is not necessarily one of the compiler crate directories (for example, `incan_stdlib_core` is a sealed SDK
/// component, not `crates/incan_stdlib_core`). A compiled provider can also retain a historical physical SDK path;
/// that path remains compiler-owned only when the checked provider plan has already rebound it to an equivalent active
/// SDK artifact *and* the selected direct-Rustc plan exposes its exact crate name. Project `pub::` artifacts never
/// meet either condition and remain caller-owned.
fn compiler_owned_roots_with_provider_plan(
    artifact_plan: &OvenRustcArtifactPlan,
    provider_plan: Option<&ProviderPlan>,
) -> Vec<PathBuf> {
    let mut roots = compiler_owned_roots(artifact_plan);
    let selected_externs = artifact_plan
        .externs
        .iter()
        .map(|(crate_name, _)| crate_name.replace('-', "_"))
        .collect::<BTreeSet<_>>();
    if let Some(provider_plan) = provider_plan {
        for provider in provider_plan.active_records().filter(|provider| {
            !matches!(
                provider.authority,
                crate::provider::NamespaceAuthority::ProjectDependency { .. }
            )
        }) {
            let Some(artifact) = provider.artifact.as_ref() else {
                continue;
            };
            let names = [
                artifact.dependency_key.replace('-', "_"),
                artifact.manifest_name.replace('-', "_"),
                provider.identity.name.replace('-', "_"),
            ];
            if names.iter().any(|name| selected_externs.contains(name))
                && let Ok(root) = fs::canonicalize(&artifact.crate_root)
            {
                roots.push(root);
            }
        }
        for rebinding in provider_plan.sdk_dependency_rebindings() {
            let names = [
                rebinding.provider_name.replace('-', "_"),
                rebinding.dependency_key.replace('-', "_"),
            ];
            if names.iter().any(|name| selected_externs.contains(name))
                && let Ok(root) = fs::canonicalize(&rebinding.source_crate_root)
            {
                // The provider plan has checked the frozen private edge against the active SDK's semantic identity.
                // This root is only a legacy coordinate: direct Rustc still consumes the selected sealed extern.
                roots.push(root);
            }
        }
    }
    roots.sort();
    roots.dedup();
    roots
}

/// Build the narrow selected-path authority for compiler-owned dependencies.
pub(crate) fn compiler_selected_path_authority(
    artifact_plan: &OvenRustcArtifactPlan,
    provider_plan: Option<&ProviderPlan>,
) -> Option<OvenSelectedPathRustcAuthority> {
    let owned_roots = compiler_owned_roots_with_provider_plan(artifact_plan, provider_plan);
    (!owned_roots.is_empty()).then(|| OvenSelectedPathRustcAuthority::new(&owned_roots, artifact_plan))
}

/// Identify a generated compiler-runtime path only when the selected plan owns the same crate name.
///
/// The roots are compiler-owned and the plan must expose the same crate name. A lookalike path outside those roots is
/// still a caller package and must stay explicit, even if it uses the same crate name.
fn is_selected_compiler_runtime_path_dependency(
    dependency: &DependencySpec,
    selected_externs: &BTreeSet<&str>,
    owned_roots: &[PathBuf],
) -> bool {
    let DependencySource::Path { path } = &dependency.source else {
        return false;
    };
    let normalized_name = dependency.crate_name.replace('-', "_");
    selected_externs.contains(normalized_name.as_str())
        && fs::canonicalize(path)
            .ok()
            .is_some_and(|path| owned_roots.iter().any(|root| path.starts_with(root)))
}

/// Re-materialize one public provider graph by following only digest-verified public edges.
///
/// Every nested output is retained as a verified direct-Rustc search path. Only the current graph root is exposed to
/// its caller, which prevents an implementation dependency from becoming an accidental public package extern.
#[allow(clippy::too_many_arguments)]
fn rematerialize_caller_owned_provider_graph(
    artifact: &LibraryArtifactMetadata,
    manifest: &LibraryManifest,
    profile: &str,
    artifacts: &OvenRustcArtifactManifest,
    artifact_root: &Path,
    artifact_plan: &OvenRustcArtifactPlan,
    rustc: &Path,
    consumer_output_root: &Path,
    registry_authority: Option<&OvenRegistryLeafAuthority>,
    extra_dependency_search_paths: &[PathBuf],
    compiler_owned_roots: &[PathBuf],
    selected_path_authority: Option<&OvenSelectedPathRustcAuthority>,
    visiting: &mut BTreeSet<PathBuf>,
    authority_context: &mut Option<&mut OvenProjectBakeAuthorityContext>,
) -> CliResult<Vec<OvenCallerOwnedRustcLibrary>> {
    let canonical_root = fs::canonicalize(&artifact.crate_root).map_err(|error| {
        CliError::failure(format!(
            "Oven Alpha cannot canonicalize generated artifact root for pub::{} at {}: {error}",
            artifact.dependency_key,
            artifact.crate_root.display()
        ))
    })?;
    if !visiting.insert(canonical_root.clone()) {
        return Err(CliError::failure(format!(
            "Oven Alpha refuses a cyclic public provider graph while re-materializing pub::{} at {}",
            artifact.dependency_key,
            canonical_root.display()
        )));
    }
    let result = (|| {
        if let Some(dependency) = first_unselected_private_provider_edge(manifest, artifact_plan) {
            return Err(CliError::failure(format!(
                "Oven Alpha cannot re-materialize pub::{} because private provider edge `{}` is not a selected direct-Rustc foundation extern",
                artifact.dependency_key, dependency.dependency_key
            )));
        }

        let mut nested_libraries = Vec::new();
        for dependency in manifest
            .contract_metadata
            .provider
            .provider_dependencies
            .iter()
            .filter(|dependency| dependency.kind == ProviderDependencyKind::PublicPackage)
        {
            let (nested_manifest, nested_artifact) = load_receipted_public_provider_dependency(artifact, dependency)?;
            let mut materialized = rematerialize_caller_owned_provider_graph(
                &nested_artifact,
                &nested_manifest,
                profile,
                artifacts,
                artifact_root,
                artifact_plan,
                rustc,
                consumer_output_root,
                registry_authority,
                extra_dependency_search_paths,
                compiler_owned_roots,
                selected_path_authority,
                visiting,
                authority_context,
            )?;
            nested_libraries.append(&mut materialized);
        }
        deduplicate_caller_owned_libraries_prefer_extern(&mut nested_libraries);

        let receipt = caller_owned_library_receipt(artifact, profile, artifacts, authority_context.as_deref_mut())?;
        let edition = caller_owned_library_edition(artifact)?;
        let is_proc_macro = caller_owned_library_is_proc_macro(artifact)?;
        let provider_dependencies = caller_owned_library_rust_dependencies(artifact)?;
        let provider_dependencies =
            caller_owned_library_dependencies_without_unused_incan_derive(artifact, provider_dependencies)?;
        let provider_dependencies =
            caller_owned_library_dependencies_without_public_provider_edges(provider_dependencies, manifest);
        let provider_dependencies = caller_owned_library_dependencies_missing_from_selected_plan_with_owned_roots(
            &provider_dependencies,
            artifact_plan,
            compiler_owned_roots,
        );
        let mut provider_rust_libraries = materialize_declared_rust_libraries_with_selected_path_authority(
            &consumer_output_root
                .join("oven")
                .join("caller-owned-libraries")
                .join(profile)
                .join("provider-rust-dependencies"),
            rustc,
            &receipt.intent.target,
            profile,
            &provider_dependencies,
            registry_authority,
            selected_path_authority,
        )
        .map_err(oven_rustc_error)?;
        nested_libraries.append(&mut provider_rust_libraries);
        deduplicate_caller_owned_libraries_prefer_extern(&mut nested_libraries);

        let crate_name = ProjectGenerator::rust_target_name(&artifact.manifest_name);
        let artifact_digest = digest_provider_artifact(&artifact.crate_root).map_err(|error| {
            CliError::failure(format!(
                "Oven Alpha cannot fingerprint generated provider artifact for pub::{} at {}: {error}",
                artifact.dependency_key,
                artifact.crate_root.display()
            ))
        })?;
        let output = consumer_output_root
            .join("oven")
            .join("caller-owned-libraries")
            .join(profile)
            .join(artifact_digest.trim_start_matches("sha256:"))
            .join(if is_proc_macro {
                format!("lib{crate_name}{}", std::env::consts::DLL_SUFFIX)
            } else {
                format!("lib{crate_name}.rlib")
            });
        let mut provider_plan = artifact_plan.clone();
        attach_caller_owned_rustc_libraries(&mut provider_plan, &nested_libraries).map_err(oven_rustc_error)?;
        if provider_dependencies
            .iter()
            .any(|dependency| matches!(dependency.source, DependencySource::Registry))
        {
            // The provider's own registry dependencies were each attached above as a direct `--extern`, but loading
            // any one of them can require Rustc to locate its *own* further dependencies purely through
            // `-L dependency=...` search -- including proc-macro/build-script outputs that never become a named
            // registry leaf at all. `extra_dependency_search_paths` is this provider's own already-materialized
            // closure (see `caller_owned_provider_registry_leaf_authority`), the same directories that made this
            // provider's own standalone bake link successfully.
            for directory in extra_dependency_search_paths {
                if !provider_plan.dependency_search_paths.contains(directory) {
                    provider_plan.dependency_search_paths.push(directory.clone());
                }
            }
        }
        let bake_request = OvenTrustedDirectRustcTargetRequest {
            receipt: &receipt,
            artifacts,
            artifact_root,
            artifact_plan: Some(&provider_plan),
            rustc,
            source: &artifact.crate_lib_path,
            output: &output,
            crate_name: &crate_name,
            edition: &edition,
            source_evidence_key: "generated-root",
            features: &receipt.intent.features,
            prefer_dynamic: false,
        };
        let bake = if is_proc_macro {
            bake_trusted_direct_rustc_proc_macro(&bake_request)
        } else {
            bake_trusted_direct_rustc_library(&bake_request)
        }
        .map_err(oven_rustc_error)?;

        for nested in &mut nested_libraries {
            nested.expose_extern = false;
        }
        nested_libraries.push(OvenCallerOwnedRustcLibrary {
            crate_name: artifact.dependency_key.clone(),
            output: bake.output,
            digest: bake.output_digest,
            expose_extern: true,
        });
        Ok(nested_libraries)
    })();
    visiting.remove(&canonical_root);
    result
}

/// Registry-leaf authorities and dependency-search closure collected from every caller-owned path-dependency
/// provider.
///
/// A `pub::` provider consumed by path (for example a query-engine library) was compiled against its own sealed
/// third-party registry closure. [`rematerialize_caller_owned_provider_graph`] re-materializes that provider's
/// compiled libraries into the consumer's own direct-Rustc plan, but resolving the provider's *own* declared
/// registry dependencies (its `[rust-dependencies]`) needs the provider's own registry-leaf authority and full
/// dependency search closure, not just the consumer's -- the consumer's own closure knows nothing about a package
/// the provider alone depends on, directly or transitively. Each provider's authority is kept separate here rather
/// than pre-merged so [`caller_owned_provider_registry_conflict`] can compare it against the consumer's own
/// authority before anything is joined; `dependency_search_paths` additionally exposes every directory the
/// providers' own standalone bakes needed to load their externs' further dependencies (including
/// proc-macro/build-script outputs that never become a named registry leaf at all) purely through Rustc's ordinary
/// `-L dependency=...` search.
#[derive(Default)]
struct CallerOwnedProviderRegistryClosure {
    provider_authorities: Vec<OvenRegistryLeafAuthority>,
    dependency_search_paths: Vec<PathBuf>,
}

impl CallerOwnedProviderRegistryClosure {
    /// Join the consumer's own authority with every collected provider authority into one lookup surface.
    ///
    /// Joining decides only what is *discoverable*; safety against a genuinely diverging shared package is decided
    /// beforehand by [`caller_owned_provider_registry_conflict`] and per-lookup by `select_sealed_registry_leaf`'s
    /// existing same-compilation check.
    fn merged_authority(&self, consumer: Option<OvenRegistryLeafAuthority>) -> Option<OvenRegistryLeafAuthority> {
        if self.provider_authorities.is_empty() {
            return consumer;
        }
        Some(OvenRegistryLeafAuthority::aggregate(
            consumer.into_iter().chain(self.provider_authorities.iter().cloned()),
        ))
    }
}

/// Collect the registry-leaf authorities and dependency search closure owned by every caller-owned path-dependency
/// provider.
///
/// Walks the exact same caller-owned provider graph [`rematerialize_caller_owned_provider_graph`] re-materializes.
/// The collected authorities feed both the pre-bake conflict decision
/// ([`caller_owned_provider_registry_conflict`]) and, via
/// [`CallerOwnedProviderRegistryClosure::merged_authority`], the re-materialization lookup surface.
fn collect_caller_owned_provider_registry_leaf_authority(
    store: &OvenStore,
    provider_plan: &ProviderPlan,
    profile: &str,
) -> CliResult<CallerOwnedProviderRegistryClosure> {
    let mut closure = CallerOwnedProviderRegistryClosure::default();
    let mut visiting = BTreeSet::new();
    for provider in provider_plan.active_records().filter(|provider| {
        matches!(
            provider.authority,
            crate::provider::NamespaceAuthority::ProjectDependency { .. }
        )
    }) {
        let artifact = provider.artifact.as_ref().ok_or_else(|| {
            CliError::failure(format!(
                "Oven Alpha cannot resolve pub::{} because its generated library artifact is unavailable",
                provider.identity.name
            ))
        })?;
        let manifest = provider.manifest.as_ref().ok_or_else(|| {
            CliError::failure(format!(
                "Oven Alpha cannot resolve pub::{} because its checked provider manifest is unavailable",
                artifact.dependency_key
            ))
        })?;
        collect_caller_owned_provider_registry_leaf_authority_graph(
            store,
            artifact,
            manifest,
            profile,
            &mut closure,
            &mut visiting,
        )?;
    }
    closure.dependency_search_paths.sort();
    closure.dependency_search_paths.dedup();
    Ok(closure)
}

/// Recursive worker for [`collect_caller_owned_provider_registry_leaf_authority`].
///
/// Follows the same public-package provider edges [`rematerialize_caller_owned_provider_graph`] follows, so both
/// walks agree on which providers exist and which are another provider's own nested public-package dependency.
fn collect_caller_owned_provider_registry_leaf_authority_graph(
    store: &OvenStore,
    artifact: &LibraryArtifactMetadata,
    manifest: &LibraryManifest,
    profile: &str,
    closure: &mut CallerOwnedProviderRegistryClosure,
    visiting: &mut BTreeSet<PathBuf>,
) -> CliResult<()> {
    let canonical_root = fs::canonicalize(&artifact.crate_root).map_err(|error| {
        CliError::failure(format!(
            "Oven Alpha cannot canonicalize generated artifact root for pub::{} at {}: {error}",
            artifact.dependency_key,
            artifact.crate_root.display()
        ))
    })?;
    if !visiting.insert(canonical_root.clone()) {
        return Err(CliError::failure(format!(
            "Oven Alpha refuses a cyclic public provider graph while resolving pub::{} at {}",
            artifact.dependency_key,
            canonical_root.display()
        )));
    }
    let result = (|| {
        for dependency in manifest
            .contract_metadata
            .provider
            .provider_dependencies
            .iter()
            .filter(|dependency| dependency.kind == ProviderDependencyKind::PublicPackage)
        {
            let (nested_manifest, nested_artifact) = load_receipted_public_provider_dependency(artifact, dependency)?;
            collect_caller_owned_provider_registry_leaf_authority_graph(
                store,
                &nested_artifact,
                &nested_manifest,
                profile,
                closure,
                visiting,
            )?;
        }
        if let Some((provider_authority, provider_search_paths)) =
            caller_owned_provider_registry_leaf_authority(store, artifact, profile)?
        {
            closure.provider_authorities.push(provider_authority);
            closure.dependency_search_paths.extend(provider_search_paths);
        }
        Ok(())
    })();
    visiting.remove(&canonical_root);
    result
}

/// Return one caller-owned provider's own receipt-bound registry-leaf authority and dependency search closure, if
/// it declared any registry dependencies of its own.
///
/// This never bakes or invokes Cargo -- it only selects an already-published receipt, the same select-only step
/// normal build/run try before falling back to the explicit baker. A provider without a verified receipt for this
/// profile, or without any registry dependencies of its own, contributes nothing here; the ordinary
/// [`materialize_declared_rust_libraries_with_selected_path_authority`] failure surfaces an actionable error once
/// something actually needs a registry leaf this authority does not have. The returned search paths are the
/// provider's own already-materialized `artifact_plan().dependency_search_paths` -- the same directories that made
/// this provider's own standalone bake link successfully, including proc-macro/build-script outputs that have no
/// registry-leaf entry of their own.
fn caller_owned_provider_registry_leaf_authority(
    store: &OvenStore,
    artifact: &LibraryArtifactMetadata,
    profile: &str,
) -> CliResult<Option<(OvenRegistryLeafAuthority, Vec<PathBuf>)>> {
    let Some(project_root) = dependency_project_root(&artifact.crate_root) else {
        return Ok(None);
    };
    let Some(receipt) = read_verified_caller_owned_provider_receipt(&project_root, profile) else {
        return Ok(None);
    };
    let Some(selection) = select_published_project_plan(store, &receipt, OvenToolchainMaterialization::Reused)? else {
        return Ok(None);
    };
    let Some(authority) = registry_leaf_authority_for_plan_selection(&selection.plan_selection)? else {
        return Ok(None);
    };
    let search_paths = selection.plan_selection.artifact_plan().dependency_search_paths.clone();
    Ok(Some((authority, search_paths)))
}

/// Read and identity-verify one caller-owned provider's own Oven receipt for `profile`, if one exists.
///
/// An explicit project bake writes the library receipt under its target-qualified path.
///
/// The generic receipt is a legacy `incan build --lib` handoff; an explicit bake with a declared script overwrites it
/// with the last target prepared. Prefer the library-specific receipt so a consumer never borrows a script's unrelated
/// native closure. Retain the generic path only for a legacy standalone library build.
fn read_verified_caller_owned_provider_receipt(project_root: &Path, profile: &str) -> Option<crate::oven::OvenReceipt> {
    let library_entrypoint = project_root.join(OvenBakeProjectTarget::Library.source_relative_path());
    let library_receipt = project_bake_receipt_path(
        project_root,
        OvenBakeProjectTarget::Library,
        &library_entrypoint,
        profile,
    )
    .ok();
    let path = match library_receipt {
        Some(path) => match fs::symlink_metadata(&path) {
            Ok(_) => path,
            Err(error) if error.kind() == io::ErrorKind::NotFound => crate::oven::default_receipt_path(project_root),
            Err(_) => return None,
        },
        None => crate::oven::default_receipt_path(project_root),
    };
    let receipt = fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<crate::oven::OvenReceipt>(&bytes).ok())?;
    receipt.verify_identity().ok()?;
    Some(receipt)
}

/// Rebuild selected caller-owned Rust libraries in the consumer's direct-Rustc cohort.
#[allow(clippy::too_many_arguments)]
fn rematerialize_caller_owned_libraries_with_authority_context(
    provider_plan: &ProviderPlan,
    profile: &str,
    artifacts: &OvenRustcArtifactManifest,
    artifact_root: &Path,
    artifact_plan: &OvenRustcArtifactPlan,
    rustc: &Path,
    consumer_output_root: &Path,
    registry_authority: Option<&OvenRegistryLeafAuthority>,
    extra_dependency_search_paths: &[PathBuf],
    mut authority_context: Option<&mut OvenProjectBakeAuthorityContext>,
) -> CliResult<Vec<OvenCallerOwnedRustcLibrary>> {
    let mut libraries = Vec::new();
    let mut visiting = BTreeSet::new();
    let compiler_owned_roots = compiler_owned_roots_with_provider_plan(artifact_plan, Some(provider_plan));
    let selected_path_authority = (!compiler_owned_roots.is_empty())
        .then(|| OvenSelectedPathRustcAuthority::new(&compiler_owned_roots, artifact_plan));
    for provider in provider_plan.active_records().filter(|provider| {
        matches!(
            provider.authority,
            crate::provider::NamespaceAuthority::ProjectDependency { .. }
        )
    }) {
        let artifact = provider.artifact.as_ref().ok_or_else(|| {
            CliError::failure(format!(
                "Oven Alpha cannot re-materialize pub::{} because its generated library artifact is unavailable",
                provider.identity.name
            ))
        })?;
        if artifact.kind != LibraryArtifactKind::Materialized {
            return Err(CliError::failure(format!(
                "Oven Alpha cannot re-materialize pub::{} from source-only metadata; run `incan build --lib` for that dependency first",
                artifact.dependency_key
            )));
        }
        let manifest = provider.manifest.as_ref().ok_or_else(|| {
            CliError::failure(format!(
                "Oven Alpha cannot re-materialize pub::{} because its checked provider manifest is unavailable",
                artifact.dependency_key
            ))
        })?;
        libraries.extend(rematerialize_caller_owned_provider_graph(
            artifact,
            manifest,
            profile,
            artifacts,
            artifact_root,
            artifact_plan,
            rustc,
            consumer_output_root,
            registry_authority,
            extra_dependency_search_paths,
            &compiler_owned_roots,
            selected_path_authority.as_ref(),
            &mut visiting,
            &mut authority_context,
        )?);
    }
    libraries.sort_by(|left, right| {
        left.crate_name
            .cmp(&right.crate_name)
            .then_with(|| left.output.cmp(&right.output))
            .then_with(|| left.expose_extern.cmp(&right.expose_extern))
    });
    libraries.dedup_by(|left, right| {
        left.crate_name == right.crate_name && left.output == right.output && left.expose_extern == right.expose_extern
    });
    if libraries
        .windows(2)
        .any(|pair| pair[0].expose_extern && pair[1].expose_extern && pair[0].crate_name == pair[1].crate_name)
    {
        return Err(CliError::failure(
            "Oven Alpha resolved duplicate re-materialized caller-owned Rust library crate names",
        ));
    }
    Ok(libraries)
}

/// Rebuild selected caller-owned Rust libraries for ordinary consumers without an explicit-bake memo.
///
/// Unlike [`bake_oven_project`]/[`bake_oven_library`], this entry point does not (yet) collect each caller-owned
/// provider's own registry-leaf authority and dependency search closure via
/// [`collect_caller_owned_provider_registry_leaf_authority`] -- a caller-owned provider that declares registry
/// dependencies of its own is not yet supported through this path. Ordinary providers without their own registry
/// dependencies are unaffected.
#[allow(clippy::too_many_arguments)]
pub(crate) fn rematerialize_caller_owned_libraries(
    provider_plan: &ProviderPlan,
    profile: &str,
    artifacts: &OvenRustcArtifactManifest,
    artifact_root: &Path,
    artifact_plan: &OvenRustcArtifactPlan,
    rustc: &Path,
    consumer_output_root: &Path,
    registry_authority: Option<&OvenRegistryLeafAuthority>,
) -> CliResult<Vec<OvenCallerOwnedRustcLibrary>> {
    rematerialize_caller_owned_libraries_with_authority_context(
        provider_plan,
        profile,
        artifacts,
        artifact_root,
        artifact_plan,
        rustc,
        consumer_output_root,
        registry_authority,
        &[],
        None,
    )
}

/// Replace package-library attachments while preserving independently materialized inline Rust dependencies.
pub(crate) fn replace_caller_owned_package_libraries(
    libraries: &mut Vec<OvenCallerOwnedRustcLibrary>,
    re_materialized: Vec<OvenCallerOwnedRustcLibrary>,
) -> CliResult<()> {
    if re_materialized.is_empty() {
        return Ok(());
    }
    let rematerialized_names = re_materialized
        .iter()
        .filter(|library| library.expose_extern)
        .map(|library| library.crate_name.as_str())
        .collect::<BTreeSet<_>>();
    libraries.retain(|library| !rematerialized_names.contains(library.crate_name.as_str()));
    libraries.extend(re_materialized);
    libraries.sort_by(|left, right| {
        left.crate_name
            .cmp(&right.crate_name)
            .then_with(|| left.output.cmp(&right.output))
            .then_with(|| left.expose_extern.cmp(&right.expose_extern))
    });
    if libraries
        .windows(2)
        .any(|pair| pair[0].expose_extern && pair[1].expose_extern && pair[0].crate_name == pair[1].crate_name)
    {
        return Err(CliError::failure(
            "Oven Alpha resolved duplicate caller-owned Rust library crate names after package re-materialization",
        ));
    }
    Ok(())
}

/// Replace receipt-selected historical package outputs with their current-cohort re-materializations.
///
/// An explicit project plan can retain a public provider as a generated-source root, but that stored rlib belongs to
/// the producer's Rustc metadata cohort. Once the provider has been rebuilt from its receipt-checked source against
/// the consumer's selected plan, retaining both `--extern` values would either duplicate the crate name or allow a
/// mismatched provider closure to reach Rustc. Only direct package-root names are removed; registry leaves and every
/// other immutable compiler artifact remain owned by the selected plan.
fn replace_selected_package_library_externs(
    artifact_plan: &mut OvenRustcArtifactPlan,
    replacement_names: &BTreeSet<String>,
) {
    if replacement_names.is_empty() {
        return;
    }
    let removed_parents = artifact_plan
        .externs
        .iter()
        .filter(|(crate_name, _)| replacement_names.contains(crate_name))
        .filter_map(|(_, path)| path.parent().map(Path::to_path_buf))
        .collect::<BTreeSet<_>>();
    artifact_plan
        .externs
        .retain(|(crate_name, _)| !replacement_names.contains(crate_name));
    let retained_parents = artifact_plan
        .externs
        .iter()
        .filter_map(|(_, path)| path.parent().map(Path::to_path_buf))
        .collect::<BTreeSet<_>>();
    artifact_plan
        .dependency_search_paths
        .retain(|search_path| !removed_parents.contains(search_path) || retained_parents.contains(search_path));
}

/// Select exactly the resolved dependency specifications used by caller-authored inline `rust::` imports.
///
/// Provider-owned source imports remain inside the selected compiler-native closure. This prevents a caller from
/// asking the path materializer to rebuild the SDK/provider graph, while aliases such as `prost-types` /
/// `prost_types` retain Cargo's conventional spelling equivalence without invoking Cargo.
fn oven_source_inline_dependency_specs(
    resolved: &ResolvedDependencies,
    source_inline_crates: &BTreeSet<String>,
) -> CliResult<Vec<DependencySpec>> {
    let normalize = |name: &str| name.replace('-', "_");
    let mut dependencies = Vec::new();
    for source_crate in source_inline_crates {
        let normalized = normalize(source_crate);
        let dependency = resolved
            .dependencies
            .iter()
            .chain(&resolved.dev_dependencies)
            .find(|dependency| {
                normalize(&dependency.crate_name) == normalized
                    || dependency
                        .package
                        .as_deref()
                        .is_some_and(|package| normalize(package) == normalized)
            })
            .ok_or_else(|| {
                CliError::failure(format!(
                    "Oven Alpha could not resolve caller Rust import `{source_crate}` to a declared dependency"
                ))
            })?;
        dependencies.push(dependency.clone());
    }
    dependencies.sort_by(|left, right| left.crate_name.cmp(&right.crate_name));
    dependencies.dedup_by(|left, right| left.crate_name == right.crate_name);
    Ok(dependencies)
}

/// Keep only declared Rust dependencies that the selected immutable plan does not already provide.
///
/// Registry leaves in a receipt-bound plan are compiler-owned direct-Rustc inputs. Recompiling one as a
/// caller-owned library would attach two `--extern` values with one crate name. A path dependency remains
/// caller-owned even with an overlapping selected extern, except for an explicit compiler-suite path under a
/// scheduler-leased immutable root; that narrow exception is the same ownership rule used while re-materializing a
/// source-backed provider graph.
pub(crate) fn declared_rust_libraries_missing_from_selected_plan(
    dependencies: &[DependencySpec],
    artifact_plan: &OvenRustcArtifactPlan,
) -> Vec<DependencySpec> {
    declared_rust_libraries_missing_from_selected_plan_with_current_project_paths(dependencies, artifact_plan, false)
}

/// Keep direct Rust paths explicit unless the exact current-project plan already seals their projected externs.
///
/// `true` is valid only for a receipt-selected project plan. Imported packages may expose their own private path
/// dependencies with the same crate name, and compiler Loafs may expose compiler-owned paths, so neither is
/// authority to omit a caller declaration.
fn declared_rust_libraries_missing_from_selected_plan_with_current_project_paths(
    dependencies: &[DependencySpec],
    artifact_plan: &OvenRustcArtifactPlan,
    current_project_paths_are_sealed: bool,
) -> Vec<DependencySpec> {
    let selected_externs = artifact_plan
        .externs
        .iter()
        .map(|(crate_name, _)| crate_name.clone())
        .collect::<BTreeSet<_>>();
    let mut remaining = declared_rust_libraries_missing_from_selected_plan_with_owned_roots(
        dependencies,
        &selected_externs,
        &compiler_owned_roots(artifact_plan),
    );
    if current_project_paths_are_sealed {
        remaining.retain(|dependency| {
            !matches!(dependency.source, DependencySource::Path { .. })
                || !selected_externs.contains(&dependency.crate_name.replace('-', "_"))
        });
    }
    remaining
}

/// Verify the semantic registry contract for every dependency omitted because the selected plan exposes its crate.
///
/// Direct-Rustc extern names carry no Cargo package/version information. Without this check an impossible declared
/// version could silently borrow an unrelated compiler-owned artifact solely because both normalize to one crate
/// name. The sealed native catalog remains the only resolver; no Cargo cache, index, or network state is consulted.
fn validate_selected_plan_registry_dependencies(
    dependencies: &[DependencySpec],
    selected_plan: &OvenRustcArtifactPlan,
    registry_authority: Option<&OvenRegistryLeafAuthority>,
    profile: &str,
) -> CliResult<()> {
    for dependency in dependencies {
        if matches!(dependency.source, DependencySource::Registry) {
            let crate_name = dependency.crate_name.replace('-', "_");
            if let Some((_, selected_artifact)) = selected_plan
                .externs
                .iter()
                .find(|(selected_crate, _)| selected_crate == &crate_name)
            {
                validate_selected_sealed_registry_leaf(dependency, selected_artifact, registry_authority, profile)
                    .map_err(oven_rustc_error)?;
            }
        }
    }
    Ok(())
}

/// Variant with explicit scheduler-owned roots so the path-authority boundary is independently testable.
fn declared_rust_libraries_missing_from_selected_plan_with_owned_roots(
    dependencies: &[DependencySpec],
    selected_externs: &BTreeSet<String>,
    owned_roots: &[PathBuf],
) -> Vec<DependencySpec> {
    let selected_extern_names = selected_externs.iter().map(String::as_str).collect::<BTreeSet<_>>();
    dependencies
        .iter()
        .filter(|dependency| match dependency.source {
            DependencySource::Registry => !selected_externs.contains(&dependency.crate_name.replace('-', "_")),
            DependencySource::Path { .. } => {
                !is_selected_compiler_runtime_path_dependency(dependency, &selected_extern_names, owned_roots)
            }
            DependencySource::Git { .. } => true,
        })
        .cloned()
        .collect()
}

/// Profiles an explicit `oven bake` materializes, and that its consumers then expect to find.
///
/// A bake normally produces every profile a later `build`, `test`, or `run` could select, so a project with a
/// large `[rust-dependencies]` closure pays a full optimized build even when only `debug` is ever loaded. On
/// IncQL that release half measured 2098 of 2717 rustc CPU seconds.
///
/// This is deliberately an environment policy rather than an `oven bake` flag. The profile set is not private to
/// the bake: `build` verifies project inspection authority across the same set, and a narrowed bake seen by a
/// consumer that still expects both profiles reports "no source-current project inspection authority is
/// available" rather than the profile it is actually missing. An environment override applies to every command in
/// a session — one `env:` block in CI covers `bake`, `build`, and `test` alike — whereas a per-invocation flag
/// would leave the consumers disagreeing with the bake that produced their inputs.
///
/// A future change could record the materialized profile set in the authority itself and let each consumer check
/// only the profile it needs; until then the set must be stated the same way to every command.
fn explicit_bake_profiles() -> Vec<&'static str> {
    match std::env::var("INCAN_OVEN_BAKE_PROFILES").ok().as_deref().map(str::trim) {
        Some("debug") => vec!["debug"],
        Some("release") => vec!["release"],
        _ => vec!["debug", "release"],
    }
}

/// Analyze, generate, receipt, and select the direct-Rustc plan for one normal Oven executable command.
#[allow(clippy::too_many_arguments)]
fn prepare_oven_project(
    file_path: &str,
    output_dir: Option<&str>,
    cargo_policy: &CargoPolicy,
    package_features: &FeatureSelection,
    sdk_profile_override: Option<&str>,
    cargo_features: Vec<String>,
    cargo_no_default_features: bool,
    cargo_all_features: bool,
    profile: &str,
    oven_plan_mode: OvenProjectPlanMode,
    authority_context: Option<&mut OvenProjectBakeAuthorityContext>,
    backend_options: &BackendSelectionOptions,
) -> CliResult<OvenPreparedProject> {
    if cargo_no_default_features || cargo_all_features || !cargo_features.is_empty() {
        return Err(CliError::failure(
            "Oven Alpha normal build and run do not accept Cargo feature controls; use Incan package features instead",
        ));
    }
    let normalized_file_path = if Path::new(file_path).is_absolute() {
        PathBuf::from(file_path)
    } else {
        env::current_dir()
            .map_err(|error| CliError::failure(format!("failed to determine current directory: {error}")))?
            .join(file_path)
    };
    let path = normalized_file_path.as_path();
    let inferred_project_root = resolve_project_root(path);
    let compilation_session = CompilationSession::discover_for_oven(path, package_features, sdk_profile_override)?;
    let manifest = compilation_session.manifest.clone();
    if let Some(manifest) = manifest.as_ref() {
        enforce_project_toolchain_constraint(manifest)?;
    }
    let modules =
        super::common::collect_modules_detailed_with_session(normalized_file_path.clone(), &compilation_session)
            .map_err(|failure| CliError::failure(failure.render_human()))?;
    let Some(main_module) = modules.last() else {
        return Err(CliError::failure("No modules found"));
    };
    // ---- Backend selection (#986) — declared before codegen, refused visibly if unavailable ----
    let (backend_selection, backend_executed) = select_and_resolve_backend(backend_options, &modules)?;
    let dep_modules = &modules[..modules.len() - 1];
    let project_root = manifest
        .as_ref()
        .map(|manifest| manifest.project_root().to_path_buf())
        .unwrap_or(inferred_project_root);
    let entrypoint_evidence_key = oven_executable_entrypoint_evidence_key(&project_root, path)?;
    let package_feature_plan = compilation_session.package_feature_plan.clone();
    let library_manifest_index = compilation_session.library_manifest_index.clone();
    let mut project_requirements = collect_project_requirements(&modules, &library_manifest_index)?;
    let provider_plan = compilation_session.provider_plan_for_modules(&modules)?;
    let mut caller_owned_libraries = oven_caller_owned_libraries(&provider_plan, profile)?;
    let compiled_sdk_modules = CompiledSdkModules::from_provider_plan(&provider_plan);
    extend_requirements_with_provider_plan(&mut project_requirements, &provider_plan)?;
    ensure_loaf_stdlib_features(&mut project_requirements.stdlib_features, loaf_codegen_mode());
    let emitted_dep_modules: Vec<&ParsedModule> = dep_modules
        .iter()
        .filter(|module| !compiled_sdk_modules.contains_emission_path(&module.path_segments))
        .collect();

    let project_name = manifest
        .as_ref()
        .and_then(|manifest| manifest.project.as_ref().and_then(|project| project.name.clone()))
        .unwrap_or_else(|| {
            path.file_stem()
                .and_then(|stem| stem.to_str())
                .unwrap_or("incan_project")
                .to_string()
        });
    let project_version = manifest
        .as_ref()
        .and_then(|manifest| manifest.project.as_ref().and_then(|project| project.version.clone()))
        .unwrap_or_else(|| "0.1.0".to_string());
    // Normal Oven output belongs to the caller's project, not the compiler process's current directory. A caller may
    // still explicitly choose an output destination; only the no-flag default is project-local.
    let out_dir = match output_dir {
        Some(output_dir) => {
            validate_output_dir(output_dir)?;
            PathBuf::from(output_dir)
        }
        None => project_root.join("target").join("incan").join(&project_name),
    };

    let mut codegen = IrCodegen::new();
    // A source-emitted provider module must retain its public implementation closure. Its public protocol methods
    // can construct public adapter models declared later in the same module even when the root program does not name
    // those adapters directly. Pruning them made a normal Oven source projection ill-formed (`FallibleIterator.map`
    // referenced an omitted `MapFallibleIterator`). A completely dependency-free program can still use the smaller
    // projection; the named Loaf publisher always retains the complete compiler-owned provider envelope.
    codegen.set_preserve_dependency_public_items(preserve_source_dependency_public_items(
        loaf_codegen_mode(),
        emitted_dep_modules.len(),
    ));
    codegen.set_registry_package_identity(Some(project_name.clone()));
    codegen.set_root_source_module_name(path.file_stem().and_then(|stem| stem.to_str()).map(str::to_string));
    if let Some(manifest) = manifest.as_ref() {
        codegen.set_declared_crate_names(manifest.declared_rust_crate_names());
    }
    codegen.set_provider_plan(Arc::clone(&provider_plan));
    for module in dep_modules
        .iter()
        .filter(|module| compiled_sdk_modules.contains_emission_path(&module.path_segments))
    {
        codegen.add_dependency_symbol_module_with_path_segments(
            &module.name,
            &module.ast,
            module.path_segments.clone(),
        );
    }
    for module in &emitted_dep_modules {
        codegen.add_module_with_path_segments(&module.name, &module.ast, module.path_segments.clone());
    }

    let mut generator = ProjectGenerator::new(&out_dir, project_name.as_str(), true);
    if let Some(project) = manifest.as_ref().and_then(|manifest| manifest.project.as_ref()) {
        generator.set_package_metadata(project.version.clone(), project.license.clone());
    }
    generator.set_provider_plan(&provider_plan);
    generator.set_sdk_path_dependencies(project_requirements.sdk_path_dependencies.clone());
    generator.set_stdlib_features(project_requirements.stdlib_features.clone());
    if oven_plan_mode == OvenProjectPlanMode::InteropBootstrap {
        generator.enable_companion_library_target();
    }
    // An interop bootstrap must generate the exact normal executable source closure. It is allowed to publish that
    // closure through the named compatibility boundary, but it cannot pull in publisher-only development inputs or
    // its pre-interop receipt would not be selectable by the later normal direct-rustc consumer.
    generator.set_include_dev_dependencies(oven_plan_mode == OvenProjectPlanMode::ExplicitBake);
    let rust_edition = manifest
        .as_ref()
        .and_then(|manifest| manifest.build.as_ref().and_then(|build| build.rust_edition.clone()))
        .unwrap_or_else(|| "2024".to_string());
    generator.set_rust_edition(Some(rust_edition.clone()));

    let mut source_inline_imports = collect_rust_dependency_uses(main_module, false);
    let mut inline_imports = source_inline_imports.clone();
    for module in &emitted_dep_modules {
        let module_imports = collect_rust_dependency_uses(module, false);
        // A source-backed stdlib module has not yet become an installed compiled SDK artifact, but it still belongs to
        // the compiler-owned provider closure. Its Rust imports may be admitted to the explicit baker so a Loaf
        // captures their exact direct-rustc inputs. Rust imports from any caller-owned module remain a separate Alpha
        // boundary and cannot be smuggled through the standard-library source path.
        if module.path_segments.first().map(String::as_str) != Some("__incan_std") {
            source_inline_imports.extend(module_imports.clone());
        }
        inline_imports.extend(module_imports);
    }
    // `std.*` source modules lower through the compiler-owned `incan_stdlib` crate. ProjectGenerator supplies that
    // runtime directly, so its internal `rust.module("incan_stdlib::...")` declarations are not user-selected Cargo
    // inputs. Rust's own `std` crate is likewise supplied by the selected compiler. The selected stdlib provider may
    // have its own external Rust closure, which the named publisher records in `inline_imports`; caller-owned Rust
    // imports are materialized only through the narrow direct-rustc path-package seam below.
    source_inline_imports.retain(|import| import.crate_name != "incan_stdlib" && import.crate_name != "std");
    let source_inline_crates = source_inline_imports
        .iter()
        .map(|import| import.crate_name.clone())
        .collect::<BTreeSet<_>>();
    inline_imports.retain(|import| import.crate_name != "incan_stdlib" && import.crate_name != "std");
    let cargo_features = CargoFeatureSelection {
        cargo_features,
        cargo_no_default_features,
        cargo_all_features,
    }
    .normalized();
    let mut resolved = resolve_reachable_dependencies(manifest.as_ref(), &inline_imports, true, &cargo_features)
        .map_err(|errors| {
            let sources = build_source_map(&modules);
            let message = errors
                .iter()
                .map(|error| format_dependency_error(error, &sources))
                .collect::<String>();
            CliError::failure(message.trim_end())
        })?;
    merge_project_requirement_dependencies(&mut resolved, &project_requirements)?;
    let inline_path_dependencies = oven_source_inline_dependency_specs(&resolved, &source_inline_crates)?;
    // Strict flags are Incan lock promises, not authorization to re-enter the Cargo projection path. The Oven
    // validator recomputes the canonical fingerprint from read-only metadata and fails on a missing or stale lock.
    validate_oven_lock_policy(
        &project_root,
        manifest.as_ref(),
        &normalized_file_path,
        &cargo_features,
        cargo_policy,
        package_features,
        sdk_profile_override,
    )?;
    let mut oven_build_inputs = oven_build_unit_inputs(&provider_plan, &project_requirements, &resolved)?;
    let rustc = resolve_active_rustc().map_err(|error| CliError::failure(error.to_string()))?;
    let rustc_target = rustc_host_target(&rustc).map_err(|error| CliError::failure(error.to_string()))?;
    let rustc_toolchain = rustc_identity(&rustc).map_err(|error| CliError::failure(error.to_string()))?;
    if oven_plan_mode != OvenProjectPlanMode::InteropBootstrap {
        append_oven_interop_execution_build_inputs(&mut oven_build_inputs, manifest.as_ref(), &rustc_target)?;
    }
    let oven_store = open_default_oven_store()?;

    #[cfg(feature = "rust_inspect")]
    let rust_inspect_manifest_dir = {
        let metadata_query_paths = loaf_rust_inspect_query_paths(&modules, &compilation_session)?;
        let prepared_project_source_authorities = if oven_plan_mode == OvenProjectPlanMode::ConsumeOnly
            && !loaf_codegen_mode()
            && manifest.is_some()
            && !metadata_query_paths.is_empty()
        {
            let authority = load_current_project_registry_source_authorities(&oven_store, &project_root)?
                .ok_or_else(|| {
                    CliError::failure(
                        "Oven Alpha has no source-current project inspection authority; rerun `incan oven bake --project .`",
                    )
                })?;
            Some(prepare_project_registry_source_authorities(authority)?)
        } else {
            None
        };
        let rust_inspect_manifest_dir = prepare_rust_inspect_workspace(RustInspectWorkspaceRequest {
            project_root: &project_root,
            project_name: project_name.as_str(),
            cargo_package_name: project_name.as_str(),
            rust_edition: Some(rust_edition.clone()),
            resolved: &resolved,
            project_requirements: &project_requirements,
            lock_payload: None,
            cargo_lock_projection_root: None,
            clear_cargo_lock: false,
            cargo_policy_flags: Vec::new(),
            cargo_target_dir: &generator.output_dir().join("oven").join("rust-inspect"),
            rust_inspect_query_paths: &metadata_query_paths,
            rust_derive_probe_paths: &collect_rust_inspect_derive_probe_paths(&modules),
            prepare_when_empty: false,
            direct_oven_inspection: true,
            force_direct_prewarm: loaf_codegen_mode(),
            oven_source_authority: Some(OvenRustInspectSourceAuthorityRequest {
                project_version: &project_version,
                target: &rustc_target,
                toolchain: &rustc_toolchain,
                profile,
                features: &cargo_features.cargo_features,
                build_unit_inputs: &oven_build_inputs,
                registry_dependencies: &resolved.dependencies,
            }),
            prepared_project_source_authorities,
            explicit_oven_bake: oven_plan_mode == OvenProjectPlanMode::ExplicitBake,
        })?;
        if let Some(manifest_dir) = rust_inspect_manifest_dir.as_ref() {
            codegen.set_rust_inspect_manifest_dir(manifest_dir.manifest_dir().to_path_buf());
        }
        rust_inspect_manifest_dir
    };

    let analysis = compilation_session
        .analyze_modules(
            &modules,
            #[cfg(feature = "rust_inspect")]
            rust_inspect_manifest_dir
                .as_ref()
                .map(|workspace| workspace.manifest_dir()),
        )
        .map_err(|failure| CliError::failure(failure.render_human()))?;
    let main_type_info = analysis
        .type_info_for_path(&main_module.file_path)
        .cloned()
        .ok_or_else(|| {
            CliError::failure(format!(
                "missing session analysis for {}",
                main_module.file_path.display()
            ))
        })?;
    let mut dependency_type_info = HashMap::with_capacity(dep_modules.len());
    for module in dep_modules {
        let type_info = analysis
            .type_info_for_path(&module.file_path)
            .cloned()
            .ok_or_else(|| CliError::failure(format!("missing session analysis for {}", module.file_path.display())))?;
        dependency_type_info.insert(module.path_segments.clone(), type_info);
    }
    codegen.set_stdlib_cache(analysis.stdlib_cache().clone());
    codegen.set_prechecked_type_info(main_type_info, dependency_type_info);
    // The Oven executor consumes the generated source directly, but its report remains an inspection surface for
    // the same resolved dependency inputs that produced the receipt. Keep those inputs before moving them into the
    // generator so normal direct-rustc reports do not falsely claim that the project has no Rust dependencies.
    let rust_dependencies = resolved.dependencies.clone();
    let rust_dev_dependencies = resolved.dev_dependencies.clone();
    let requested_provider_profiles = if oven_plan_mode == OvenProjectPlanMode::ExplicitBake {
        explicit_bake_profiles()
    } else {
        vec![profile]
    };
    let checked_provider_profiles = checked_packaged_provider_profiles(
        &provider_plan,
        &requested_provider_profiles,
        &rustc_target,
        &rustc_toolchain,
        authority_context,
    )?;
    let oven_plan_dependencies = inline_path_dependencies.clone();
    import_packaged_provider_loafs_for_explicit_bake(oven_plan_mode, &oven_store, &checked_provider_profiles)?;
    generator.set_dependencies(resolved.dependencies);
    generator.set_dev_dependencies(resolved.dev_dependencies);

    let has_deps = !emitted_dep_modules.is_empty()
        || dep_modules
            .iter()
            .any(|module| compiled_sdk_modules.contains_emission_path(&module.path_segments));
    let backend_output_identity = if has_deps {
        let module_paths = emitted_dep_modules
            .iter()
            .map(|module| module.path_segments.clone())
            .collect::<Vec<_>>();
        let (main_code, rust_modules) = codegen
            .try_generate_multi_file_nested(&main_module.ast, &module_paths)
            .map_err(|error| CliError::failure(format!("Code generation error: {error}")))?;
        generator
            .generate_nested(&main_code, &rust_modules)
            .map_err(|error| CliError::failure(format!("Error generating project: {error}")))?;
        multi_file_output_identity(&main_code, &rust_modules)
    } else {
        let rust_code = codegen
            .try_generate(&main_module.ast)
            .map_err(|error| CliError::failure(format!("Code generation error: {error}")))?;
        generator
            .generate(&rust_code)
            .map_err(|error| CliError::failure(format!("Error generating project: {error}")))?;
        digest_output(&[rust_code.as_str()])
    };
    let backend_receipt = finalize_backend_receipt(&backend_selection, backend_executed, backend_output_identity)?;
    // Not persisted here: `prepare_oven_project` runs for internal/dependency callers too (see
    // `BackendSelectionOptions::default()` call sites), and real compilation (the Oven plan
    // selection and rustc bake below) can still fail after this point. The receipt is instead
    // published by the top-level `build_file_report`/`build_library_report` entry points, once
    // and only once the whole build has actually succeeded (#986).

    let mut receipt_request = OvenGeneratedProjectRequest::new(
        &project_root,
        &project_name,
        &project_version,
        rustc_target,
        rustc_toolchain,
        profile,
        cargo_features.cargo_features.clone(),
    )
    .with_generated_source("generated-root", generator.crate_root_path())
    .with_generated_source_tree("generated-source-tree", generator.output_dir().join("src"))
    .with_generated_source(&entrypoint_evidence_key, path);
    for (name, value) in &oven_build_inputs {
        receipt_request = receipt_request.with_build_unit_input(name.clone(), value.clone());
    }
    let receipt = receipt_generated_project(&receipt_request).map_err(|error| CliError::failure(error.to_string()))?;
    let receipt_path =
        prepared_oven_receipt_path(&project_root, oven_plan_mode, &receipt.intent.target, path, profile)?;
    write_receipt(&receipt, &receipt_path).map_err(|error| CliError::failure(error.to_string()))?;
    let required_registry_dependencies = format_oven_registry_dependency_requirements(&oven_plan_dependencies);
    // An imported package Loaf is sufficient only for consume-only commands. The explicit baker must publish the
    // consumer's own direct registry roots with its complete generated source closure; otherwise the provider's
    // catalog would incorrectly become the registry authority for a consumer-declared dependency.
    let packaged_provider_selection = if oven_plan_mode == OvenProjectPlanMode::ConsumeOnly {
        select_packaged_provider_plan(&oven_store, &checked_provider_profiles, profile, &receipt)?
    } else {
        None
    };
    let plan_preparation = if let Some(selection) = packaged_provider_selection {
        Some(OvenDirectRustcPlanPreparation {
            plan_selection: selection,
            materialization: OvenToolchainMaterialization::Reused,
            cargo_process_started: false,
        })
    } else {
        select_or_bake_generated_project_plan(
            oven_plan_mode,
            &oven_store,
            &receipt,
            OvenProjectDependencySurface {
                selection: &oven_plan_dependencies,
            },
            generator.output_dir(),
            &generator.crate_root_path(),
            &rustc,
        )?
    };
    let plan_preparation = plan_preparation.ok_or_else(|| {
        CliError::failure(format!(
            "{}. `incan build` and `incan run` {}. {} (Needs: {}. Build record {}; generated project: {}; receipt: {}.)",
            OVEN_DEPENDENCY_MISS_SUMMARY,
            OVEN_NO_IMPLICIT_DEPENDENCY_BUILD,
            OVEN_LOAF_MISS_GUIDANCE,
            required_registry_dependencies,
            receipt.identity,
            generator.output_dir().display(),
            receipt_path.display(),
        ))
    })?;
    let plan_selection = plan_preparation.plan_selection;
    let registry_authority = registry_leaf_authority_for_plan_selection(&plan_selection)?;
    let full_artifact_plan = plan_selection.artifact_plan();
    let artifact_plan = plan_selection
        .source_artifact_plan("generated-root")
        .map_err(oven_rustc_error)?;
    validate_selected_plan_registry_dependencies(
        &oven_plan_dependencies,
        &artifact_plan,
        registry_authority.as_ref(),
        profile,
    )?;
    let inline_libraries = declared_rust_libraries_missing_from_selected_plan_with_current_project_paths(
        &inline_path_dependencies,
        &artifact_plan,
        plan_selection.seals_current_project_path_dependencies(),
    );
    let selected_path_authority = compiler_selected_path_authority(full_artifact_plan, Some(&provider_plan));
    caller_owned_libraries.extend(
        materialize_declared_rust_libraries_with_selected_path_authority(
            &generator.output_dir().join("oven").join("inline-rust"),
            &rustc,
            &receipt.intent.target,
            profile,
            &inline_libraries,
            registry_authority.as_ref(),
            selected_path_authority.as_ref(),
        )
        .map_err(oven_rustc_error)?,
    );
    caller_owned_libraries.sort_by(|left, right| left.crate_name.cmp(&right.crate_name));
    if caller_owned_libraries
        .windows(2)
        .any(|pair| pair[0].crate_name == pair[1].crate_name)
    {
        return Err(CliError::failure(
            "Oven Alpha resolved duplicate caller-owned Rust library crate names",
        ));
    }

    let report = BuildReportDraft {
        mode: BuildReportMode::Executable,
        profile: profile.to_string(),
        project: manifest_project_report(manifest.as_ref(), &project_name, &project_root),
        entrypoint: Some(normalized_file_path.to_string_lossy().to_string()),
        library_root: None,
        source_files: source_file_report(&modules),
        generated: oven_generated_project_report(
            generator.output_dir(),
            &generator.crate_root_path(),
            &generator.output_dir().join("oven"),
        ),
        artifacts: Vec::new(),
        dependencies: dependencies_report(
            &rust_dependencies,
            &rust_dev_dependencies,
            manifest
                .as_ref()
                .map(|manifest| incan_dependencies_report(manifest.library_dependencies().iter().collect()))
                .unwrap_or_default(),
            project_requirements.stdlib_features.clone(),
        ),
        semantic: semantic_report(
            compilation_session.sdk_inventory.as_deref(),
            compilation_session.sdk_components.as_ref(),
            package_feature_plan.as_ref(),
            &provider_plan,
        ),
        cargo: None,
        oven: Some(BuildOvenReport {
            receipt_identity: receipt.identity.clone(),
            build_unit_identity: receipt.build_unit_identity.clone(),
            plan_identity: plan_selection.report_identity(),
        }),
        interop: interop_report(&inline_imports, Vec::new(), Vec::new()),
        notes: vec![
            "Oven Alpha selected a receipt-bound direct-rustc plan; normal execution did not invoke Cargo or inspect a Cargo target directory.".to_string(),
        ],
        backend: Some(backend_receipt),
    };
    Ok(OvenPreparedProject {
        generator,
        project_root,
        entrypoint: normalized_file_path,
        provider_plan,
        receipt,
        plan_selection,
        materialization: plan_preparation.materialization,
        cargo_process_started: plan_preparation.cargo_process_started,
        rustc,
        crate_name: ProjectGenerator::rust_target_name(&project_name),
        rust_edition,
        caller_owned_libraries,
        report,
        #[cfg(feature = "rust_inspect")]
        rust_inspect_manifest_dir: rust_inspect_manifest_dir
            .as_ref()
            .map(|workspace| workspace.manifest_dir().to_path_buf()),
    })
}

/// Prepare the Rust-only direct-rustc base required before Oven can seal a package's declared native artifacts.
///
/// A checked C binding still lowers directly into the final generated Rust root. The compatibility publisher must
/// nevertheless prepare its Rust dependency closure before that root can link a package-owned dynamic library. This
/// dedicated bootstrap stops at that boundary: it publishes no caller-visible binary and does not select a native
/// toolchain. `incan oven interop bake` alone performs those later actions.
pub(crate) fn prepare_oven_interop_bootstrap(
    project: &Path,
    target: &str,
) -> CliResult<(crate::oven::OvenReceipt, PathBuf, bool)> {
    let project = project.to_str().ok_or_else(|| {
        CliError::failure(format!(
            "Oven interop project path is not valid UTF-8: {}",
            project.display()
        ))
    })?;
    let project_root = resolve_library_project_root(Some(project))?;
    let (kind, entrypoint) = sole_oven_interop_executable_target(discover_oven_bake_project_targets(&project_root)?)?;
    let entrypoint_text = entrypoint.to_str().ok_or_else(|| {
        CliError::failure(format!(
            "Oven interop entrypoint is not valid UTF-8: {}",
            entrypoint.display()
        ))
    })?;
    let prepared = prepare_oven_project(
        entrypoint_text,
        None,
        &CargoPolicy::default(),
        &FeatureSelection::default(),
        None,
        Vec::new(),
        false,
        false,
        "debug",
        OvenProjectPlanMode::InteropBootstrap,
        None,
        &BackendSelectionOptions::default(),
    )?;
    if prepared.receipt.intent.target != target {
        return Err(CliError::failure(format!(
            "Oven interop bootstrap prepared Rust target `{}`, but the declared native target is `{target}`; select a Rust toolchain for that target before baking native interop",
            prepared.receipt.intent.target
        )));
    }
    let receipt_path = interop_bootstrap_receipt_path(
        &project_root,
        &prepared.receipt.intent.target,
        kind,
        &entrypoint,
        "debug",
    )?;
    Ok((prepared.receipt, receipt_path, prepared.cargo_process_started))
}

/// Select the one executable an automatic interop bootstrap may prepare.
///
/// An explicit `--base-receipt` remains available for packages whose author intentionally selects one of several
/// scripts. Automatic selection must fail closed rather than letting filesystem or manifest discovery order decide
/// which generated root becomes native-plan authority.
fn sole_oven_interop_executable_target(
    targets: Vec<(OvenBakeProjectTarget, PathBuf)>,
) -> CliResult<(OvenBakeProjectTarget, PathBuf)> {
    let executable_targets = targets
        .into_iter()
        .filter(|(kind, _)| *kind == OvenBakeProjectTarget::Executable)
        .collect::<Vec<_>>();
    match executable_targets.as_slice() {
        [(kind, entrypoint)] => Ok((*kind, entrypoint.clone())),
        [] => Err(CliError::failure(
            "Oven interop bootstrap currently requires src/main.incn or one declared [project.scripts] executable entrypoint",
        )),
        _ => {
            let entrypoints = executable_targets
                .iter()
                .map(|(_, entrypoint)| entrypoint.display().to_string())
                .collect::<Vec<_>>()
                .join(", ");
            Err(CliError::failure(format!(
                "Oven interop bootstrap requires one executable entrypoint, but this package declares: {entrypoints}; provide an explicit base receipt for the intended entrypoint",
            )))
        }
    }
}

/// Return whether an explicit Loaf publisher is constructing its compiler-owned source closure.
///
/// This marker is intentionally not a normal-command fallback: it changes only generated-source retention before
/// the separately named `legacy_cargo` publisher seals a Loaf. Baked normal build/run/test consumers merely
/// select that immutable plan and execute direct `rustc`.
fn loaf_codegen_mode() -> bool {
    std::env::var_os(OVEN_LOAF_ENV).is_some_and(|value| value == "1")
}

/// Preserve public implementation items whenever the Oven projection emits dependency source.
///
/// A dependency's public protocol methods can construct sibling public adapter models that the root source does not
/// name directly. Emitting the protocol while pruning those adapters produces invalid Rust, so source-backed
/// dependencies form an implementation closure rather than a root-reachability-only projection.
fn preserve_source_dependency_public_items(loaf: bool, emitted_dependency_count: usize) -> bool {
    loaf || emitted_dependency_count > 0
}

/// Include compiler-owned provider Rust imports in the Loaf's inspection workspace.
///
/// Provider modules are deliberately metadata-only for ordinary Oven consumers, so their `rust::` imports are absent
/// from a caller module graph. The named Loaf baker compiles the complete provider source closure instead. It
/// must therefore inspect those exact source imports before codegen, or ownership-sensitive Rust calls (for example
/// `rustix::fs::flock(&impl AsFd, ...)`) degrade to an untyped by-value call. This source walk remains confined to the
/// explicit release-publishing marker and never runs for normal build, run, or test commands.
#[cfg(feature = "rust_inspect")]
fn loaf_rust_inspect_query_paths(
    modules: &[ParsedModule],
    compilation_session: &CompilationSession,
) -> CliResult<Vec<String>> {
    let mut query_paths: BTreeSet<String> = collect_rust_inspect_query_paths(modules).into_iter().collect();
    if !loaf_codegen_mode() {
        return Ok(query_paths.into_iter().collect());
    }

    let stdlib_root = crate::cli::prelude::find_stdlib_dir()
        .ok_or_else(|| CliError::failure("cannot locate compiler-owned stdlib sources while preparing an Oven Loaf"))?;
    let mut source_files = Vec::new();
    collect_incan_source_files(&stdlib_root, &mut source_files).map_err(|error| {
        CliError::failure(format!(
            "failed to discover compiler-owned stdlib sources under {}: {error}",
            stdlib_root.display()
        ))
    })?;
    source_files.sort();

    for source_path in source_files {
        let source = fs::read_to_string(&source_path)
            .map_err(|error| CliError::failure(format!("failed to read {}: {error}", source_path.display())))?;
        let program = compilation_session
            .parse_source(&source_path, &source, false)
            .map_err(|errors| {
                let rendered = errors
                    .iter()
                    .map(|error| diagnostics::format_error(source_path.to_string_lossy().as_ref(), &source, error))
                    .collect::<String>();
                CliError::failure(rendered.trim_end())
            })?;
        query_paths.extend(collect_rust_inspect_query_paths_from_programs([&program]));
    }
    Ok(query_paths.into_iter().collect())
}

/// Make a compiler-owned Loaf internally consistent with its retained provider source.
///
/// The named publisher deliberately retains the complete standard-provider envelope, including modules behind all
/// optional `incan_stdlib` runtime features. The generated crate must enable the same runtime surface; otherwise the
/// sealed source refers to cfg-gated `incan_stdlib` modules that Cargo omitted while preparing the one explicit
/// publisher artifact. This never changes an ordinary Oven project's feature set.
fn ensure_loaf_stdlib_features(stdlib_features: &mut Vec<String>, loaf: bool) {
    if !loaf {
        return;
    }

    stdlib_features.extend(["async", "json", "ordinal", "web"].into_iter().map(str::to_string));
    stdlib_features.sort();
    stdlib_features.dedup();
}

/// Return the receipt-compatible direct-Rustc selection for one normal command.
///
/// The compiler-suite scheduler marks nested commands explicitly and supplies a read-only compiler-data root whose
/// store partition remains leased by the parent. Those children consume that shared Loaf directly; copying it
/// into every fixture's small mutable store would consume the same capacity repeatedly and allow policy pruning to
/// remove a just-selected plan before execution. Every other normal command retains the ordinary bounded-store path.
pub(crate) fn select_oven_direct_rustc_plan(
    store: &OvenStore,
    receipt: &crate::oven::OvenReceipt,
    registry_dependencies: &[DependencySpec],
) -> CliResult<Option<OvenDirectRustcPlanSelection>> {
    select_oven_direct_rustc_plan_with_materialization(store, receipt, registry_dependencies)
        .map(|selection| selection.map(|selection| selection.plan_selection))
}

/// Select a receipt-compatible direct-rustc plan and retain the user-visible local-store outcome.
///
/// This is the one selector used by normal consumers and explicit project preparation. It deliberately keeps
/// receipt matching, Loaf compatibility, atomic store publication, and lease acquisition in the existing paths;
/// the additional outcome only makes that existing decision visible to `incan oven bake`.
fn select_oven_direct_rustc_plan_with_materialization(
    store: &OvenStore,
    receipt: &crate::oven::OvenReceipt,
    registry_dependencies: &[DependencySpec],
) -> CliResult<Option<OvenDirectRustcPlanPreparation>> {
    let compiler_suite_native =
        std::env::var_os("INCAN_INTERNAL_OVEN_LOAF_EXECUTION").is_some_and(|value| value == "1");
    if compiler_suite_native && receipt_requires_final_interop_plan(receipt) {
        return Err(interop_final_plan_required_error());
    }
    if compiler_suite_native {
        let toolchain_data_root = std::env::var_os("INCAN_INTERNAL_TOOLCHAIN_DATA_ROOT")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .filter(|path| path.is_dir())
            .ok_or_else(|| {
                CliError::failure("compiler-suite native execution requires a readable immutable toolchain-data root")
            })?;
        let native = resolve_compiler_owned_loaf_for_registry_dependencies(receipt, registry_dependencies)
            .map_err(|error| CliError::failure(error.to_string()))?;
        if let Some(native) = native {
            if !native.artifact_root.starts_with(&toolchain_data_root) {
                return Err(CliError::failure(
                    "compiler-suite native selection escaped its immutable toolchain-data root",
                ));
            }
            return Ok(Some(OvenDirectRustcPlanPreparation {
                plan_selection: OvenDirectRustcPlanSelection::ToolchainLoaf(Box::new(native)),
                materialization: OvenToolchainMaterialization::ToolchainLoaf,
                cargo_process_started: false,
            }));
        }
        return Err(CliError::failure(format!(
            "{}. Nested build and run {}. (Needs: {}.)",
            OVEN_NESTED_DEPENDENCY_MISS_SUMMARY,
            OVEN_NO_IMPLICIT_DEPENDENCY_BUILD,
            format_oven_registry_dependency_requirements(registry_dependencies),
        )));
    }

    if receipt_requires_final_interop_plan(receipt) {
        return select_published_project_plan(store, receipt, OvenToolchainMaterialization::Reused)?.map_or_else(
            || Err(interop_final_plan_required_error()),
            |selection| Ok(Some(selection)),
        );
    }
    // A receipt-exact project Loaf is narrower than the release-wide standard-library family and must win when it
    // exists. Selecting the broad family first would expose its fixture-only direct externs to a normal project,
    // then bypass the exact base-plus-extension composition that the explicit baker already sealed.
    if let Some(selected) = select_published_project_plan(store, receipt, OvenToolchainMaterialization::Reused)? {
        return Ok(Some(selected));
    }
    if let Some(native) = resolve_compiler_owned_loaf_for_registry_dependencies(receipt, registry_dependencies)
        .map_err(|error| CliError::failure(error.to_string()))?
    {
        return Ok(Some(OvenDirectRustcPlanPreparation {
            plan_selection: OvenDirectRustcPlanSelection::ToolchainLoaf(Box::new(native)),
            materialization: OvenToolchainMaterialization::ToolchainLoaf,
            cargo_process_started: false,
        }));
    }
    Ok(None)
}

/// Publish a receipt-compatible generated-project Loaf at Oven's one explicit project-bake boundary.
///
/// The compatibility baker owns Cargo only for this transaction. It creates a private bounded target, seals the
/// verified direct-rustc artifacts into the shared Oven store, and removes the private target before returning. A
/// project bake retains the locked third-party and provider artifacts that extend one exact Incan release Loaf. Before
/// publishing the project delta, the baker canonicalizes compiler-owned runtime artifacts, overlapping locked registry
/// units, and vocabulary auxiliaries against that exact base cohort. This prevents later consumers from observing
/// distinct Incan release cohorts while preserving the project's own dependency lock.
#[allow(clippy::too_many_arguments)] // Each input is a distinct publisher authority: store, receipt, roots, rustc, base Loaf, vocab support, kind.
fn bake_generated_project_compatibility_plan(
    store: &OvenStore,
    receipt: &crate::oven::OvenReceipt,
    generated_project: &Path,
    generated_root: &Path,
    rustc: &Path,
    base_loaf: Option<&OvenToolchainLoaf>,
    source_compiler_vocab_support: bool,
    publication_kind: OvenLegacyCargoPublicationKind,
) -> CliResult<OvenToolchainMaterialization> {
    let compile_environment = direct_rustc_reusable_project_plan_environment(generated_project, generated_root)
        .map_err(|error| CliError::failure(error.to_string()))?;
    let publication = prepare_direct_rustc_plan(&OvenLegacyCargoPrepareRequest {
        store,
        receipt: receipt.clone(),
        generated_project: generated_project.to_path_buf(),
        cargo: resolved_cargo_executable()
            .map_err(|error| CliError::failure(format!("cannot resolve Cargo for explicit Oven bake: {error}")))?,
        rustc: rustc.to_path_buf(),
        sdk_inventory: None,
        compiler_loaf_root: None,
        domain: format!("incan-release-{INCAN_VERSION}"),
        publication_kind,
        source_evidence_key: "generated-root".to_string(),
        compile_environment,
        // A project Loaf is the complete exact closure for its generated project, including caller-owned `pub::`
        // providers. Retaining only source-inspection roots would copy an upstream crate's rlibs but omit their
        // receipt-bound registry catalog entries, making legitimate re-materialization fail closed later.
        inspection_packages: None,
        direct_dependency_closure: OvenLegacyCargoDirectDependencyClosure::GeneratedSource,
        // The stored direct-rustc plan needs debuggable generated source and verified link inputs, not Cargo's
        // multi-gigabyte dependency DWARF payload. Keep the named debug publisher compact so one project closure stays
        // inside Oven's bounded compatibility domain.
        compact_debug_info: true,
        source_compiler_vocab_support: source_compiler_vocab_support && base_loaf.is_none(),
        // Rust package identity is carried through artifact metadata, not only source or crate names. The base owns the
        // complete Incan release cohort; the project contributes its locked third-party and provider delta.
        base_loaf: base_loaf.map(|base| OvenLegacyCargoBaseLoaf {
            loaf_identity: base.loaf_identity.clone(),
            build_unit_identity: base.loaf_build_unit_identity.clone(),
            artifacts: &base.artifacts,
            artifact_root: &base.artifact_root,
        }),
    })
    .map_err(|error| CliError::failure(error.to_string()))?;
    Ok(if publication.cargo_version == "not-run-existing-plan" {
        OvenToolchainMaterialization::Reused
    } else {
        OvenToolchainMaterialization::CompatibilityBaked
    })
}

/// Remove the compiler-generated publisher lock after an explicit bake has sealed its digest and direct-Rustc closure
/// into immutable Loafs.
///
/// The lock is publisher input, not a caller-facing Oven artifact. Retaining it below `target/` would make a completed
/// direct-Rustc output look like a mutable Cargo workspace and invite an unsupported normal-command path.
fn remove_completed_generated_cargo_lock(generated_project: &Path) -> CliResult<()> {
    let lock_path = generated_project.join("Cargo.lock");
    match fs::remove_file(&lock_path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(CliError::failure(format!(
            "could not remove explicit Oven bake publisher lock {}: {error}",
            lock_path.display()
        ))),
    }
}

/// Select the immutable full-stdlib base that supplies the release-owned Incan dependency cohort.
///
/// The project retains its own locked third-party closure, while compiler-owned runtime artifacts, overlapping locked
/// registry units, and vocabulary auxiliaries inherit this exact release base. Normal consumers still require a
/// compatible prebuilt Loaf or stored project plan and never invoke this helper as a fallback.
fn project_extension_base_loaf(receipt: &crate::oven::OvenReceipt) -> CliResult<Option<OvenToolchainLoaf>> {
    resolve_compiler_owned_loaf_for_registry_dependencies(receipt, &[])
        .map_err(|error| CliError::failure(error.to_string()))
}

/// The complete checked Rust dependency surface used to select one project Loaf.
///
/// Source inspection happens before code generation through its own compiler session. This value instead covers
/// every direct-Rustc root the generated program and its caller-owned public providers may re-materialize.
struct OvenProjectDependencySurface<'a> {
    selection: &'a [DependencySpec],
}

/// Promote the complete canonical normal/dev surface into one generated-test dependency set.
///
/// Cargo unifies features and default-feature activation for duplicate package edges. The synthetic envelope mirrors
/// that behavior once at explicit bake time while preserving the dependency key, package rename, source, and version.
/// Every selected edge becomes non-optional because a generated native test may use any dependency reachable from the
/// checked project/test graph.
pub(crate) fn promoted_oven_test_dependencies(resolved: &ResolvedDependencies) -> CliResult<Vec<DependencySpec>> {
    let mut promoted = Vec::new();
    for candidate in resolved.dependencies.iter().chain(&resolved.dev_dependencies) {
        if let Some(existing) = promoted
            .iter_mut()
            .find(|dependency: &&mut DependencySpec| dependency.crate_name == candidate.crate_name)
        {
            if existing.version != candidate.version
                || existing.source != candidate.source
                || existing.package != candidate.package
            {
                return Err(CliError::failure(format!(
                    "test dependency `{}` conflicts between the canonical normal and dev surfaces",
                    candidate.crate_name
                )));
            }
            existing.features.extend(candidate.features.iter().cloned());
            existing.features.sort();
            existing.features.dedup();
            existing.default_features |= candidate.default_features;
            existing.optional = false;
            continue;
        }
        let mut candidate = candidate.clone().normalized();
        candidate.optional = false;
        promoted.push(candidate);
    }
    promoted.sort_by(|left, right| left.crate_name.cmp(&right.crate_name));
    Ok(promoted)
}

/// Remove public package-provider roots from the Cargo-published test delta without narrowing project authority.
///
/// The caller retains `dependencies` unchanged for the singular inspection authority. A validated package Loaf owns
/// its public direct-Rustc library and complete Rust closure, so publishing the same path root in the synthetic test
/// constituent would attach two independently materialized externs with one Rust-facing alias.
fn test_dependency_publisher_dependencies(
    dependencies: &[DependencySpec],
    packaged_provider_aliases: &BTreeSet<String>,
) -> Vec<DependencySpec> {
    dependencies
        .iter()
        .filter(|dependency| {
            let alias = dependency.crate_name.replace('-', "_");
            !packaged_provider_aliases.contains(&alias)
        })
        .cloned()
        .collect()
}

/// Preserve normal/dev ownership labels while applying Cargo's canonical feature union to duplicate root aliases.
fn canonical_project_inspection_dependencies(
    resolved: &ResolvedDependencies,
) -> CliResult<(Vec<DependencySpec>, Vec<DependencySpec>)> {
    let promoted = promoted_oven_test_dependencies(resolved)?;
    let select = |dependencies: &[DependencySpec]| {
        dependencies
            .iter()
            .map(|dependency| {
                promoted
                    .iter()
                    .find(|candidate| candidate.crate_name == dependency.crate_name)
                    .cloned()
                    .ok_or_else(|| {
                        CliError::failure(format!(
                            "canonical project inspection dependency `{}` disappeared during promotion",
                            dependency.crate_name
                        ))
                    })
            })
            .collect::<CliResult<Vec<_>>>()
    };
    Ok((select(&resolved.dependencies)?, select(&resolved.dev_dependencies)?))
}

/// Receipt-selected dependency closure prepared once for every generated native-test batch in a project bake.
struct PreparedOvenTestDependencyEnvelope {
    /// Receipt for the immutable non-package constituent selected by the authority.
    receipt: crate::oven::OvenReceipt,
    /// Complete normal/dev/test surface, including roots owned by separately validated package Loafs.
    dependency_surface_digest: String,
    /// Complete dependency records retained for exact per-root authority checks.
    dependencies: Vec<DependencySpec>,
    dependency_root_digests: BTreeMap<String, String>,
    /// Direct-Rustc plan for the non-package delta; public package libraries are attached from their own Loafs.
    plan_selection: OvenDirectRustcPlanSelection,
}

/// Digest each promoted dependency independently so generated test batches can prove an exact subset later.
fn oven_test_dependency_root_digests(dependencies: &[DependencySpec]) -> CliResult<BTreeMap<String, String>> {
    let mut roots = BTreeMap::new();
    for dependency in dependencies {
        let alias = dependency.crate_name.replace('-', "_");
        let digest = digest_dependency_specs(std::slice::from_ref(dependency))
            .map_err(|error| CliError::failure(error.to_string()))?;
        if roots.insert(alias.clone(), digest).is_some() {
            return Err(CliError::failure(format!(
                "test dependency surface contains duplicate Rust-facing alias `{alias}`"
            )));
        }
    }
    Ok(roots)
}

/// Return whether a prepared debug target already binds the exact non-package dependency delta for tests.
fn debug_target_receipt_covers_test_publisher_dependencies(
    receipt: &crate::oven::OvenReceipt,
    dependency_surface_digest: &str,
) -> bool {
    receipt.intent.profile == "debug"
        && receipt.compatibility.kind == crate::oven::OvenCompatibilityKind::GeneratedIncanProject
        && receipt
            .sources
            .build_unit_inputs
            .get("rust-dependencies")
            .is_some_and(|digest| digest == dependency_surface_digest)
}

/// Publish the one project-owned test dependency delta through Cargo's explicit compatibility boundary.
fn bake_generated_project_test_dependency_plan(
    store: &OvenStore,
    receipt: &crate::oven::OvenReceipt,
    generated_project: &Path,
    generated_root: &Path,
    rustc: &Path,
    base_loaf: Option<&OvenToolchainLoaf>,
) -> CliResult<OvenToolchainMaterialization> {
    let compile_environment = direct_rustc_reusable_project_plan_environment(generated_project, generated_root)
        .map_err(|error| CliError::failure(error.to_string()))?;
    let publication = prepare_direct_rustc_plan(&OvenLegacyCargoPrepareRequest {
        store,
        receipt: receipt.clone(),
        generated_project: generated_project.to_path_buf(),
        cargo: resolved_cargo_executable()
            .map_err(|error| CliError::failure(format!("cannot resolve Cargo for explicit Oven bake: {error}")))?,
        rustc: rustc.to_path_buf(),
        sdk_inventory: None,
        compiler_loaf_root: None,
        domain: format!("incan-release-{INCAN_VERSION}"),
        publication_kind: OvenLegacyCargoPublicationKind::Executable,
        source_evidence_key: "generated-root".to_string(),
        compile_environment,
        inspection_packages: None,
        direct_dependency_closure: OvenLegacyCargoDirectDependencyClosure::CheckedDeclared,
        compact_debug_info: true,
        source_compiler_vocab_support: false,
        base_loaf: base_loaf.map(|base| OvenLegacyCargoBaseLoaf {
            loaf_identity: base.loaf_identity.clone(),
            build_unit_identity: base.loaf_build_unit_identity.clone(),
            artifacts: &base.artifacts,
            artifact_root: &base.artifact_root,
        }),
    })
    .map_err(|error| CliError::failure(error.to_string()))?;
    Ok(if publication.cargo_version == "not-run-existing-plan" {
        OvenToolchainMaterialization::Reused
    } else {
        OvenToolchainMaterialization::CompatibilityBaked
    })
}

/// Prepare one debug-only dependency envelope from the same whole-project graph used by `incan lock`.
///
/// The generated root is intentionally stable and contains no authored test code. Package-provider roots remain in
/// the singular authority but are supplied by their separately validated Loafs. A compiler-shipped release Loaf is
/// returned directly when it covers the remaining selected surface; only a genuine third-party/path delta crosses
/// the explicit Cargo baker, and it does so with `build --locked --offline` through the executable publisher.
fn prepare_oven_test_dependency_envelope(
    store: &OvenStore,
    project_root: &Path,
    resolved: &ResolvedDependencies,
    debug_target_receipts: &[crate::oven::OvenReceipt],
    authority_context: Option<&mut OvenProjectBakeAuthorityContext>,
) -> CliResult<PreparedOvenTestDependencyEnvelope> {
    let dependencies = promoted_oven_test_dependencies(resolved)?;
    let dependency_surface_digest =
        digest_dependency_specs(&dependencies).map_err(|error| CliError::failure(error.to_string()))?;
    let dependency_root_digests = oven_test_dependency_root_digests(&dependencies)?;
    let base_receipt = debug_target_receipts.first().ok_or_else(|| {
        CliError::failure("explicit Oven project bake prepared no debug target receipt for its test dependency surface")
    })?;
    let checked_package_profiles =
        checked_test_dependency_package_profiles(&dependencies, base_receipt, authority_context)?;
    for checked in &checked_package_profiles {
        import_checked_packaged_library_loaf(store, checked)?;
    }
    let packaged_provider_aliases = checked_package_profiles
        .iter()
        .map(|checked| checked.dependency_key.replace('-', "_"))
        .collect::<BTreeSet<_>>();
    let publisher_dependencies = test_dependency_publisher_dependencies(&dependencies, &packaged_provider_aliases);
    let publisher_dependency_surface_digest =
        digest_dependency_specs(&publisher_dependencies).map_err(|error| CliError::failure(error.to_string()))?;
    for receipt in debug_target_receipts {
        let covers =
            debug_target_receipt_covers_test_publisher_dependencies(receipt, &publisher_dependency_surface_digest);
        tracing::debug!(
            "test dependency envelope: debug target receipt {} kind={:?} rust-dependencies={:?} publisher-surface={} covers={covers}",
            receipt.identity,
            receipt.compatibility.kind,
            receipt.sources.build_unit_inputs.get("rust-dependencies"),
            publisher_dependency_surface_digest
        );
        if !covers {
            continue;
        }
        let Some(plan_selection) = select_oven_direct_rustc_plan(store, receipt, &publisher_dependencies)? else {
            tracing::debug!(
                "test dependency envelope: receipt {} covers the surface but selects no stored plan",
                receipt.identity
            );
            continue;
        };
        if matches!(
            &plan_selection,
            OvenDirectRustcPlanSelection::Stored(_)
                | OvenDirectRustcPlanSelection::ToolchainLoaf(_)
                | OvenDirectRustcPlanSelection::ProjectExtension(_)
        ) {
            return Ok(PreparedOvenTestDependencyEnvelope {
                receipt: receipt.clone(),
                dependency_surface_digest,
                dependencies,
                dependency_root_digests,
                plan_selection,
            });
        }
    }
    let generated_project = project_root
        .join("target")
        .join("incan")
        .join("oven")
        .join("test-dependency-envelope");
    let mut generator = ProjectGenerator::new(&generated_project, "incan_test_dependency_envelope", true);
    generator.set_package_metadata(Some(INCAN_VERSION.to_string()), None);
    generator.set_dependencies(publisher_dependencies.clone());
    generator.set_dev_dependencies(Vec::new());
    generator
        .generate("fn main() {}\n")
        .map_err(|error| CliError::failure(format!("failed to generate Oven test dependency envelope: {error}")))?;
    let rustc = resolve_active_rustc().map_err(|error| CliError::failure(error.to_string()))?;
    let mut receipt_request = OvenGeneratedProjectRequest::new(
        project_root,
        "incan-test-dependency-envelope",
        INCAN_VERSION,
        base_receipt.intent.target.clone(),
        base_receipt.intent.toolchain.clone(),
        "debug",
        base_receipt.intent.features.clone(),
    )
    .with_generated_source("generated-root", generator.crate_root_path())
    .with_generated_source_tree("generated-source-tree", generator.output_dir().join("src"));
    for (name, value) in &base_receipt.sources.build_unit_inputs {
        receipt_request = receipt_request.with_build_unit_input(name.clone(), value.clone());
    }
    receipt_request = receipt_request.with_build_unit_input("rust-dependencies", publisher_dependency_surface_digest);
    let receipt = receipt_generated_project(&receipt_request).map_err(|error| CliError::failure(error.to_string()))?;
    let plan_selection = if publisher_dependencies
        .iter()
        .all(|dependency| matches!(dependency.source, DependencySource::Registry))
        && let Some(loaf) = resolve_compiler_owned_loaf_for_registry_dependencies(&receipt, &publisher_dependencies)
            .map_err(|error| CliError::failure(error.to_string()))?
    {
        OvenDirectRustcPlanSelection::ToolchainLoaf(Box::new(loaf))
    } else {
        let base_loaf = project_extension_base_loaf(&receipt)?;
        let materialization = bake_generated_project_test_dependency_plan(
            store,
            &receipt,
            generator.output_dir(),
            &generator.crate_root_path(),
            &rustc,
            base_loaf.as_ref(),
        )?;
        select_published_project_plan(store, &receipt, materialization)?
            .ok_or_else(|| {
                CliError::failure(
                    "the explicit Oven project bake completed without its checked test dependency envelope",
                )
            })?
            .plan_selection
    };
    remove_completed_generated_cargo_lock(generator.output_dir())?;
    Ok(PreparedOvenTestDependencyEnvelope {
        receipt,
        dependency_surface_digest,
        dependencies,
        dependency_root_digests,
        plan_selection,
    })
}

/// Select a plan for an explicit project bake, reusing only an exact project Loaf before publishing once.
fn select_or_bake_generated_project_plan(
    mode: OvenProjectPlanMode,
    store: &OvenStore,
    receipt: &crate::oven::OvenReceipt,
    dependency_surface: OvenProjectDependencySurface<'_>,
    generated_project: &Path,
    generated_root: &Path,
    rustc: &Path,
) -> CliResult<Option<OvenDirectRustcPlanPreparation>> {
    if receipt_requires_final_interop_plan(receipt) {
        return select_published_project_plan(store, receipt, OvenToolchainMaterialization::Reused)?.map_or_else(
            || Err(interop_final_plan_required_error()),
            |selection| Ok(Some(selection)),
        );
    }
    let source_compiler_vocab_support = receipt
        .sources
        .build_unit_inputs
        .get(OVEN_SOURCE_COMPILER_VOCAB_SUPPORT_BUILD_INPUT)
        .is_some_and(|value| value == "v1");
    if mode.is_explicit_publisher() {
        // The completed project-output Loaf owns generated project sources and the final native result. Do not bake
        // an empty project extension when the installed release Loaf already supplies the complete native dependency
        // closure: that would duplicate release-owned bytes without adding project authority.
        if let Some(selected) =
            select_published_project_extension_plan(store, receipt, OvenToolchainMaterialization::Reused)?
        {
            return Ok(Some(selected));
        }
        if mode == OvenProjectPlanMode::ExplicitBake
            && dependency_surface
                .selection
                .iter()
                .all(|dependency| matches!(dependency.source, DependencySource::Registry))
            && let Some(loaf) =
                resolve_compiler_owned_loaf_for_registry_dependencies(receipt, dependency_surface.selection)
                    .map_err(|error| CliError::failure(error.to_string()))?
        {
            return Ok(Some(OvenDirectRustcPlanPreparation {
                plan_selection: OvenDirectRustcPlanSelection::ToolchainLoaf(Box::new(loaf)),
                materialization: OvenToolchainMaterialization::ToolchainLoaf,
                cargo_process_started: false,
            }));
        }
        let bootstrap_lock_seeded = if mode == OvenProjectPlanMode::InteropBootstrap {
            // The bootstrap has no caller-owned Rust registry inputs. Seed its generated manifest from the checked
            // compiler lock, normalize the local path records offline, and make the later compatibility build
            // unconditionally locked. That closes the first-plan loop without turning native interop into ambient
            // Cargo or network discovery. The lock is resolved through the toolchain layout: an installed release
            // carries it below `crates/Cargo.lock`, and only a development checkout keeps it at the workspace root.
            let compiler_lock = crate::toolchain_layout::resolve_toolchain_runtime_lockfile();
            let cargo = resolved_cargo_executable()
                .map_err(|error| CliError::failure(format!("cannot resolve Cargo for interop bootstrap: {error}")))?;
            stage_locked_loaf_fixture(&cargo, generated_project, &compiler_lock).map_err(|error| {
                CliError::failure(format!("could not seed the interop bootstrap Cargo.lock: {error}"))
            })?;
            true
        } else {
            false
        };
        // A direct-C bootstrap must publish a project-owned base plan even when a compiler Loaf could otherwise
        // satisfy the Rust closure. `oven interop bake` extends that exact stored plan with the locked native
        // search paths and runtime bundles; a Loaf selected outside this store would leave no base artifact to
        // extend, and would reintroduce the circular "link before sealed" failure.
        let base_loaf = project_extension_base_loaf(receipt)?;
        let materialization = bake_generated_project_compatibility_plan(
            store,
            receipt,
            generated_project,
            generated_root,
            rustc,
            base_loaf.as_ref(),
            source_compiler_vocab_support,
            if mode == OvenProjectPlanMode::InteropBootstrap {
                OvenLegacyCargoPublicationKind::InteropBootstrap
            } else {
                OvenLegacyCargoPublicationKind::Executable
            },
        )?;
        let mut prepared = select_published_project_plan(store, receipt, materialization)?.ok_or_else(|| {
            CliError::failure("the explicit Oven project bake completed without a receipt-compatible direct-rustc plan")
        })?;
        prepared.cargo_process_started =
            bootstrap_lock_seeded || materialization == OvenToolchainMaterialization::CompatibilityBaked;
        return Ok(Some(prepared));
    }
    select_oven_direct_rustc_plan_with_materialization(store, receipt, dependency_surface.selection)
}

/// Return whether this receipt requires an exact final native interop plan rather than a base Loaf.
fn receipt_requires_final_interop_plan(receipt: &crate::oven::OvenReceipt) -> bool {
    receipt
        .sources
        .build_unit_inputs
        .contains_key(OVEN_INTEROP_EXECUTION_RECEIPT_INPUT)
}

/// Return the actionable fail-closed error for a selected interop receipt without its final native plan.
fn interop_final_plan_required_error() -> CliError {
    CliError::failure(
        "Oven interop has a selected execution receipt but no matching final native direct-Rustc plan. Run `incan oven interop bake` for this locked target; normal build and run will not materialize a generic Loaf, discover native tools, or invoke Cargo.",
    )
}

/// Receipt-selected plan plus the explicit local-store decision that produced it.
struct OvenDirectRustcPlanPreparation {
    plan_selection: OvenDirectRustcPlanSelection,
    materialization: OvenToolchainMaterialization,
    cargo_process_started: bool,
}

/// Render the registry requirements that made sealed Loaf selection impossible.
///
/// Oven does not invoke Cargo to diagnose an unavailable registry version, so this preserves the manifest-level
/// dependency identity that a user must correct instead of returning an opaque Loaf-selection failure.
fn format_oven_registry_dependency_requirements(dependencies: &[DependencySpec]) -> String {
    let mut requirements = dependencies
        .iter()
        .filter(|dependency| matches!(dependency.source, DependencySource::Registry))
        .map(|dependency| {
            let package = dependency.package.as_deref().unwrap_or(&dependency.crate_name);
            let version = dependency.version.as_deref().unwrap_or("<missing version>");
            format!("`{package}` `{version}`")
        })
        .collect::<Vec<_>>();
    requirements.sort();
    requirements.dedup();
    if requirements.is_empty() {
        "none".to_string()
    } else {
        requirements.join(", ")
    }
}

/// Return the registry catalog copied with the active, receipt-selected direct-rustc plan.
///
/// A registry artifact's metadata is valid only with the feature-unified compatibility domain that published it.
/// Normal selection therefore chooses a Loaf that covers every caller-visible registry root, then resolves only the
/// catalog sealed into that one leased plan—never an aggregate Cargo cache or a second Loaf's dependency directory.
fn registry_leaf_authority_for_plan_selection(
    selection: &OvenDirectRustcPlanSelection,
) -> CliResult<Option<OvenRegistryLeafAuthority>> {
    Ok(selection.registry_leaf_authority())
}

/// Detect whether a caller-owned provider's own registry closure would silently link a second, incompatible
/// compiled instance of a package `plan` already links explicitly, returning the first such package.
///
/// Linking a provider's own registry-resolved package (for example an async runtime a query-engine provider pulls
/// in through its own dependency graph) alongside the SDK/consumer's own separately compiled copy of that same
/// package is a real, reproduced defect, not a theoretical one: it produced a runtime panic ("no reactor running")
/// from two distinct compiled `tokio` instances silently linked into one binary, discovered only by inspecting the
/// linked executable's own symbol table after the build otherwise succeeded. Properly unifying a provider's
/// independently Cargo-resolved registry closure with the consumer's own is out of scope for Oven Alpha's
/// direct-rustc execution. An executable bake routes this shape through the unified-Cargo fallback
/// ([`cargo_fallback_bake_oven_project`]); a library bake, which has no Cargo fallback yet, fails closed via
/// [`reject_caller_owned_provider_registry_conflict`].
fn caller_owned_provider_registry_conflict(
    consumer_authority: Option<&OvenRegistryLeafAuthority>,
    closure: &CallerOwnedProviderRegistryClosure,
    plan: &OvenRustcArtifactPlan,
) -> CliResult<Option<String>> {
    for provider_authority in &closure.provider_authorities {
        // A shared package can enter both closures transitively without ever being a named extern of either
        // compile (the reproduced `tokio` duplication was exactly this shape), so the catalogs themselves are
        // compared first; the extern comparison then covers packages the selected plan links directly.
        if let Some(consumer_authority) = consumer_authority
            && let Some(package) = consumer_authority.first_diverging_shared_package(provider_authority)
        {
            return Ok(Some(package));
        }
        if let Some(package) = provider_authority
            .first_conflicting_package_with(plan)
            .map_err(oven_rustc_error)?
        {
            return Ok(Some(package));
        }
    }
    Ok(None)
}

/// Fail closed on a provider registry conflict for bake paths that have no unified-Cargo fallback.
///
/// See [`caller_owned_provider_registry_conflict`] for why the conflict is dangerous. Refusing to build here, with
/// the exact conflicting package named, is safer than shipping an artifact whose async runtime state silently
/// splits across two incompatible copies.
fn reject_caller_owned_provider_registry_conflict(
    consumer_authority: Option<&OvenRegistryLeafAuthority>,
    closure: &CallerOwnedProviderRegistryClosure,
    plan: &OvenRustcArtifactPlan,
) -> CliResult<()> {
    if let Some(package) = caller_owned_provider_registry_conflict(consumer_authority, closure, plan)? {
        return Err(CliError::failure(format!(
            "Oven Alpha refuses to build: a caller-owned provider's own registry closure resolves `{package}` to a \
             different compiled artifact than this project's own closure already links. Linking both would silently \
             admit two incompatible compiled instances of the same crate into one binary -- for a crate that carries \
             process-wide runtime state (most dangerously an async runtime), this can produce a runtime panic instead \
             of a build failure. Oven Alpha does not yet unify a caller-owned provider's independently resolved \
             registry closure with the consumer's own for library outputs; build this consumer as an executable \
             project, or prepare an explicit Oven-native closure that reconciles `{package}` to one shared compiled \
             artifact."
        )));
    }
    Ok(())
}

/// Compile a conflicted-provider project through one unified Cargo invocation instead of direct-rustc composition.
///
/// This is the routing target for the one project shape direct-rustc composition cannot yet build safely (see
/// [`caller_owned_provider_registry_conflict`]). The generated project on disk already carries the complete Cargo
/// wiring -- the consumer's manifest, each `pub::` provider as a Cargo path dependency, and the provider's own
/// registry dependencies -- so one `cargo build` resolves everything as a single feature-unified graph in which
/// exactly one compiled instance of each package exists by construction. This is the same build path v0.4 shipped
/// with; only projects that actually hit the conflict pay its cost. The produced binary is published to the same
/// [`oven_binary_path`] destination a direct-rustc bake uses, so run/report consumers are unaffected.
fn cargo_fallback_bake_oven_project(
    prepared: &OvenPreparedProject,
    profile: &str,
    conflicting_package: &str,
) -> CliResult<crate::oven::rustc::OvenDirectRustcBake> {
    eprintln!(
        "Oven: building `{}` through unified Cargo resolution: provider registry package `{conflicting_package}` \
         requires one shared compiled closure.",
        prepared.crate_name
    );
    let release = profile == "release";
    let result = prepared.generator.cargo_build(release).map_err(|error| {
        CliError::failure(format!(
            "unified Cargo fallback build failed to start for `{}`: {error}",
            prepared.crate_name
        ))
    })?;
    if !result.success {
        return Err(CliError::failure(format!(
            "unified Cargo fallback build failed for `{}`:\n{}",
            prepared.crate_name, result.stderr
        )));
    }
    let built = prepared.generator.cargo_build_binary_path(release);
    let bytes = fs::read(&built).map_err(|error| {
        CliError::failure(format!(
            "unified Cargo fallback build reported success but its binary is unreadable at {}: {error}",
            built.display()
        ))
    })?;
    let output_digest = crate::oven::digest_bytes(&bytes);
    let output = oven_binary_path(prepared, profile);
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            CliError::failure(format!(
                "could not create Oven binary destination {}: {error}",
                parent.display()
            ))
        })?;
    }
    fs::copy(&built, &output).map_err(|error| {
        CliError::failure(format!(
            "could not publish unified Cargo fallback binary to {}: {error}",
            output.display()
        ))
    })?;
    Ok(crate::oven::rustc::OvenDirectRustcBake::from_external_cargo_build(
        prepared.receipt.identity.clone(),
        output,
        output_digest,
    ))
}

/// Return the caller-owned Oven binary destination, intentionally outside generated-Cargo target layout.
fn oven_binary_path(prepared: &OvenPreparedProject, profile: &str) -> PathBuf {
    prepared
        .generator
        .output_dir()
        .join("oven")
        .join(profile)
        .join(&prepared.crate_name)
}

/// Return the native host target without asking an ambient Rust installation to decide whether a completed Loaf may
/// run. A completed project output is already linked; only a fresh compile needs to resolve `rustc`.
fn native_project_output_target() -> Option<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => Some("aarch64-apple-darwin"),
        ("macos", "x86_64") => Some("x86_64-apple-darwin"),
        ("linux", "x86_64") => Some("x86_64-unknown-linux-gnu"),
        ("linux", "aarch64") => Some("aarch64-unknown-linux-gnu"),
        _ => None,
    }
}

/// Normalize one entrypoint exactly as project preparation does, without discovering modules or constructing a compiler
/// session.
fn normalized_project_entrypoint(file_path: &str) -> CliResult<PathBuf> {
    if Path::new(file_path).is_absolute() {
        return Ok(PathBuf::from(file_path));
    }
    env::current_dir()
        .map_err(|error| CliError::failure(format!("failed to determine current directory: {error}")))
        .map(|current_dir| current_dir.join(file_path))
}

/// Return a portable project-relative entrypoint path or decline the optional output fast path. Non-project invocations
/// remain supported by normal Oven preparation; only explicit project bakes may have published this form.
fn project_relative_entrypoint(project_root: &Path, entrypoint: &Path) -> Option<String> {
    entrypoint
        .strip_prefix(project_root)
        .ok()
        .map(|relative| relative.to_string_lossy().replace('\\', "/"))
        .filter(|relative| !relative.is_empty())
}

/// Discover the manifest-owning project root for a completed-output lookup.
///
/// This avoids assuming a conventional `src/` layout: a custom source root is still an exact project bake, while a
/// standalone file intentionally remains on the normal explicit-preparation path.
fn project_root_for_completed_output(entrypoint: &Path) -> CliResult<Option<PathBuf>> {
    let inferred_root = resolve_project_root(entrypoint);
    let Some(manifest) = discover_effective_project_manifest(&inferred_root)? else {
        return Ok(None);
    };
    enforce_project_toolchain_constraint(&manifest)?;
    Ok(Some(manifest.project_root().to_path_buf()))
}

/// Reject a store- or caller-relative path that could escape its declared root.
fn validated_project_output_relative_path(relative_path: &str, role: &str) -> CliResult<PathBuf> {
    let relative = Path::new(relative_path);
    if relative.is_absolute()
        || relative.components().any(|component| {
            matches!(
                component,
                std::path::Component::ParentDir | std::path::Component::RootDir | std::path::Component::Prefix(_)
            )
        })
    {
        return Err(CliError::failure(format!(
            "selected Oven project-output Loaf has an unsafe {role} path"
        )));
    }
    Ok(relative.to_path_buf())
}

/// Add one regular file below a generated project result to the completed-Loaf publication set.
fn append_project_output_bake_file(
    project_root: &Path,
    source_path: &Path,
    output_relative_path: String,
    files: &mut Vec<OvenProjectOutputBakeFile>,
) -> CliResult<()> {
    let metadata = fs::symlink_metadata(source_path).map_err(|error| {
        CliError::failure(format!(
            "cannot retain generated Oven project output {}: {error}",
            source_path.display()
        ))
    })?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(CliError::failure(format!(
            "completed Oven project output must be a regular non-symlink file: {}",
            source_path.display()
        )));
    }
    let caller_relative_path = source_path
        .strip_prefix(project_root)
        .map_err(|_| {
            CliError::failure(format!(
                "completed Oven project output {} escaped project root {}",
                source_path.display(),
                project_root.display()
            ))
        })?
        .to_string_lossy()
        .replace('\\', "/");
    let output_relative_path = validated_project_output_relative_path(&output_relative_path, "stored output")?
        .to_string_lossy()
        .replace('\\', "/");
    let _ = validated_project_output_relative_path(&caller_relative_path, "caller output")?;
    files.push(OvenProjectOutputBakeFile {
        source_path: source_path.to_path_buf(),
        caller_relative_path,
        output_relative_path,
    });
    Ok(())
}

/// Recursively collect generated source files in stable order without treating unrelated worktree output or a mutable
/// inspection cache as completed output.
fn append_project_output_tree(
    project_root: &Path,
    source_root: &Path,
    output_prefix: &str,
    files: &mut Vec<OvenProjectOutputBakeFile>,
) -> CliResult<()> {
    let mut entries = fs::read_dir(source_root)
        .map_err(|error| {
            CliError::failure(format!(
                "failed to read generated Oven source {}: {error}",
                source_root.display()
            ))
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| {
            CliError::failure(format!(
                "failed to enumerate generated Oven source {}: {error}",
                source_root.display()
            ))
        })?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        let file_type = entry.file_type().map_err(|error| {
            CliError::failure(format!(
                "failed to inspect generated Oven source {}: {error}",
                path.display()
            ))
        })?;
        if file_type.is_dir() {
            let child_prefix = format!(
                "{}/{}",
                output_prefix.trim_end_matches('/'),
                entry.file_name().to_string_lossy()
            );
            append_project_output_tree(project_root, &path, &child_prefix, files)?;
        } else if file_type.is_file() {
            let output_relative_path = format!(
                "{}/{}",
                output_prefix.trim_end_matches('/'),
                entry.file_name().to_string_lossy()
            );
            append_project_output_bake_file(project_root, &path, output_relative_path, files)?;
        } else {
            return Err(CliError::failure(format!(
                "generated Oven source contains a non-regular entry: {}",
                path.display()
            )));
        }
    }
    Ok(())
}

/// Return every manifest-declared provider sidecar that a consumer must retain beside its library artifact.
fn library_project_output_sidecars(
    manifest: &LibraryManifest,
    artifact_root: &Path,
) -> CliResult<Vec<(PathBuf, String)>> {
    let Some(desugarer) = manifest
        .vocab
        .as_ref()
        .and_then(|vocab| vocab.desugarer_artifact.as_ref())
    else {
        return Ok(Vec::new());
    };
    let relative = validated_project_output_relative_path(&desugarer.relative_path, "vocab desugarer artifact")?;
    let source = artifact_root.join(&relative);
    if !source.is_file() {
        return Err(CliError::failure(format!(
            "completed Oven library output is missing manifest-declared vocab desugarer artifact {}",
            source.display()
        )));
    }
    Ok(vec![(
        source,
        format!(
            "generated/provider-sidecars/{}",
            relative.to_string_lossy().replace('\\', "/")
        ),
    )])
}

/// Seal the checked provider manifest and every sidecar it authorizes as one package handoff.
fn packaged_library_metadata_files(
    manifest_path: &Path,
    manifest: &LibraryManifest,
    artifact_root: &Path,
) -> CliResult<Vec<OvenPackagedLibraryMetadataFile>> {
    let mut paths = vec![manifest_path.to_path_buf()];
    paths.extend(
        library_project_output_sidecars(manifest, artifact_root)?
            .into_iter()
            .map(|(path, _)| path),
    );
    let canonical_root = fs::canonicalize(artifact_root).map_err(|error| {
        CliError::failure(format!(
            "failed to resolve package artifact root {}: {error}",
            artifact_root.display()
        ))
    })?;
    let mut files = Vec::with_capacity(paths.len());
    for path in paths {
        let metadata = fs::symlink_metadata(&path).map_err(|error| {
            CliError::failure(format!(
                "failed to inspect package metadata file {}: {error}",
                path.display()
            ))
        })?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(CliError::failure(format!(
                "package metadata must be a regular file below its artifact root: {}",
                path.display()
            )));
        }
        let canonical = fs::canonicalize(&path).map_err(|error| {
            CliError::failure(format!(
                "failed to resolve package metadata file {}: {error}",
                path.display()
            ))
        })?;
        let relative = canonical.strip_prefix(&canonical_root).map_err(|_| {
            CliError::failure(format!(
                "package metadata file {} escapes artifact root {}",
                path.display(),
                artifact_root.display()
            ))
        })?;
        let relative =
            validated_project_output_relative_path(&relative.to_string_lossy().replace('\\', "/"), "package metadata")?;
        let (_, digest) = digest_project_output_projection_file(&canonical)?;
        files.push(OvenPackagedLibraryMetadataFile {
            relative_path: relative.to_string_lossy().replace('\\', "/"),
            digest,
        });
    }
    files.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
    if files
        .windows(2)
        .any(|pair| pair[0].relative_path == pair[1].relative_path)
    {
        return Err(CliError::failure(
            "package metadata authority contains duplicate relative paths",
        ));
    }
    Ok(files)
}

/// Verify that a package handoff still describes its exact checked manifest and declared sidecars.
fn validate_packaged_library_metadata_files(
    artifact: &LibraryArtifactMetadata,
    manifest: &OvenPackagedLibraryLoafManifest,
) -> CliResult<()> {
    let library_manifest = LibraryManifest::read_from_path(&artifact.manifest_path).map_err(|error| {
        CliError::failure(format!(
            "Oven Alpha cannot read checked package metadata for pub::{} at {}: {error}",
            artifact.dependency_key,
            artifact.manifest_path.display()
        ))
    })?;
    let actual = packaged_library_metadata_files(&artifact.manifest_path, &library_manifest, &artifact.crate_root)?;
    if actual != manifest.metadata_files {
        return Err(CliError::failure(format!(
            "Oven Alpha refuses pub::{} because its checked library metadata or declared sidecars changed after the package Loaf was baked; rebake the provider",
            artifact.dependency_key
        )));
    }
    Ok(())
}

/// Describe the durable generated project result rather than a binary alone.
///
/// Rust-inspection caches, direct-rustc sidecars, and the selected dependency Loafs remain separately managed
/// authorities. The generated source, package handoff index, and final output are the caller-visible project result
/// that an exact hot path may restore without front-end work.
fn project_output_bake_files(
    project_root: &Path,
    generator: &ProjectGenerator,
    native_output: &Path,
    library_manifest: Option<&Path>,
    package_loaf_manifest: Option<&Path>,
    library_sidecars: &[(PathBuf, String)],
) -> CliResult<Vec<OvenProjectOutputBakeFile>> {
    let mut files = Vec::new();
    append_project_output_bake_file(
        project_root,
        &generator.cargo_manifest_path(),
        "generated/Cargo.toml".to_string(),
        &mut files,
    )?;
    append_project_output_tree(
        project_root,
        &generator.output_dir().join("src"),
        "generated/src",
        &mut files,
    )?;
    if let Some(library_manifest) = library_manifest {
        append_project_output_bake_file(
            project_root,
            library_manifest,
            "generated/library.incnlib".to_string(),
            &mut files,
        )?;
    }
    if let Some(package_loaf_manifest) = package_loaf_manifest {
        append_project_output_bake_file(
            project_root,
            package_loaf_manifest,
            "generated/oven/package-loafs.json".to_string(),
            &mut files,
        )?;
    }
    for (sidecar, output_relative_path) in library_sidecars {
        append_project_output_bake_file(project_root, sidecar, output_relative_path.clone(), &mut files)?;
    }
    append_project_output_bake_file(
        project_root,
        native_output,
        OVEN_PROJECT_OUTPUT_ARTIFACT_PATH.to_string(),
        &mut files,
    )?;
    let mut caller_paths = BTreeSet::new();
    let mut output_paths = BTreeSet::new();
    for file in &files {
        if !caller_paths.insert(file.caller_relative_path.clone())
            || !output_paths.insert(file.output_relative_path.clone())
        {
            return Err(CliError::failure(
                "completed Oven project-output Loaf contains duplicate generated paths",
            ));
        }
    }
    Ok(files)
}

/// Publish one singular, project-level Rust inspection authority from the preferred debug plan.
///
/// A current project extension already splits every source tree between its exact release Loaf and its bounded
/// project fragment. The authority therefore materializes only the canonical publisher lock and names those two
/// immutable constituents; it does not copy their source trees into a third closure.
fn project_inspection_root_dependencies(
    dependencies: &[DependencySpec],
    catalog: &[OvenRustcRegistrySourcePackage],
    publisher_roots: Option<&[OvenProjectRegistrySourceDependency]>,
) -> CliResult<Vec<OvenProjectInspectionRootDependency>> {
    let mut roots = Vec::new();
    for dependency in dependencies
        .iter()
        .filter(|dependency| matches!(dependency.source, DependencySource::Registry))
    {
        let alias = dependency.crate_name.replace('-', "_");
        let package = dependency.package.as_deref().unwrap_or(&dependency.crate_name);
        let requirement = dependency
            .version
            .as_deref()
            .and_then(|version| semver::VersionReq::parse(version).ok())
            .ok_or_else(|| {
                CliError::failure(format!(
                    "project inspection dependency `{alias}` has no valid registry version requirement"
                ))
            })?;
        let mut requested_features = dependency.features.clone();
        requested_features.sort();
        requested_features.dedup();
        let matches = catalog
            .iter()
            .filter(|source| {
                source.package == package
                    && semver::Version::parse(&source.version).is_ok_and(|version| requirement.matches(&version))
                    && requested_features
                        .iter()
                        .all(|feature| source.features.contains(feature))
            })
            .collect::<Vec<_>>();
        let [source] = matches.as_slice() else {
            return Err(CliError::failure(format!(
                "project inspection dependency `{alias}` has {} feature-compatible exact records in the selected immutable source catalog",
                matches.len()
            )));
        };
        if let Some(publisher_roots) = publisher_roots {
            let mut matches = publisher_roots.iter().filter(|root| root.alias == alias);
            let Some(exact) = matches.next() else {
                return Err(CliError::failure(format!(
                    "project inspection dependency `{alias}` has no exact root-edge record in the publisher payload"
                )));
            };
            if matches.any(|candidate| candidate != exact) {
                return Err(CliError::failure(format!(
                    "project inspection dependency `{alias}` has conflicting exact root-edge records in the publisher payload"
                )));
            }
            if exact.package != source.package
                || exact.version != source.version
                || exact.registry != source.source.registry
                || exact.checksum != source.source.checksum
            {
                return Err(CliError::failure(format!(
                    "project inspection dependency `{alias}` differs from its exact publisher root edge"
                )));
            }
        }
        roots.push(OvenProjectInspectionRootDependency {
            alias,
            package: source.package.clone(),
            version: source.version.clone(),
            registry: source.source.registry.clone(),
            checksum: source.source.checksum.clone(),
            requested_features,
            default_features: dependency.default_features,
        });
    }
    roots.sort_by(|left, right| left.alias.cmp(&right.alias));
    if roots.windows(2).any(|window| window[0].alias == window[1].alias) {
        return Err(CliError::failure(
            "project inspection dependencies contain duplicate Rust-facing registry aliases",
        ));
    }
    Ok(roots)
}

/// Bind every promoted test dependency to its portable declaration/source digest and, for registry roots, Cargo's
/// exact locked package identity from the selected publisher closure.
fn project_inspection_test_dependency_roots(
    dependencies: &[DependencySpec],
    dependency_root_digests: &BTreeMap<String, String>,
    registry_source_dependencies: &[OvenProjectInspectionRootDependency],
    dev_registry_source_dependencies: &[OvenProjectInspectionRootDependency],
) -> CliResult<BTreeMap<String, OvenProjectInspectionTestDependencyRoot>> {
    let mut roots = BTreeMap::new();
    for dependency in dependencies {
        let alias = dependency.crate_name.replace('-', "_");
        let dependency_digest = dependency_root_digests.get(&alias).cloned().ok_or_else(|| {
            CliError::failure(format!(
                "project inspection test dependency `{alias}` lost its portable root digest"
            ))
        })?;
        let root = match dependency.source {
            DependencySource::Registry => {
                let mut matching = registry_source_dependencies
                    .iter()
                    .chain(dev_registry_source_dependencies)
                    .filter(|root| root.alias == alias);
                let locked = matching.next().cloned().ok_or_else(|| {
                    CliError::failure(format!(
                        "project inspection test dependency `{alias}` has no exact locked publisher root"
                    ))
                })?;
                if matching.any(|candidate| candidate != &locked) {
                    return Err(CliError::failure(format!(
                        "project inspection test dependency `{alias}` has conflicting normal/dev publisher roots"
                    )));
                }
                let package = dependency.package.as_deref().unwrap_or(&dependency.crate_name);
                let requirement = dependency
                    .version
                    .as_deref()
                    .and_then(|version| semver::VersionReq::parse(version).ok());
                let mut requested_features = dependency.features.clone();
                requested_features.sort();
                requested_features.dedup();
                if locked.package != package
                    || !requirement.is_some_and(|requirement| {
                        semver::Version::parse(&locked.version).is_ok_and(|version| requirement.matches(&version))
                    })
                    || locked.requested_features != requested_features
                    || locked.default_features != dependency.default_features
                {
                    return Err(CliError::failure(format!(
                        "project inspection test dependency `{alias}` differs from its exact locked publisher root"
                    )));
                }
                OvenProjectInspectionTestDependencyRoot::Registry {
                    dependency_digest,
                    locked,
                }
            }
            DependencySource::Path { .. } => OvenProjectInspectionTestDependencyRoot::Path { dependency_digest },
            DependencySource::Git { .. } => OvenProjectInspectionTestDependencyRoot::Git { dependency_digest },
        };
        if roots.insert(alias.clone(), root).is_some() {
            return Err(CliError::failure(format!(
                "project inspection test dependency surface repeats Rust-facing alias `{alias}`"
            )));
        }
    }
    Ok(roots)
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

/// Return every build-script output directory the explicit bake can seal for one library, versioned where known.
///
/// Units named by a package-keyed map come first: the inspection workspace loader writes one from rust-analyzer's
/// crate graph, and the compatibility build writes one from Cargo's own messages. A directory scan of the same
/// Cargo targets then adds any unit the maps did not name, without a version. Units are deduplicated by build-unit
/// path, so a directory the maps already named is not sealed twice.
fn bake_generated_out_dir_units(library: &LibraryInspectionConstituent) -> CliResult<Vec<BakeGeneratedOutDir>> {
    let mut units = Vec::new();
    let mut seen = BTreeSet::new();
    for map_dir in [
        library.rust_inspect_manifest_dir.as_deref(),
        library.generated_project_dir.as_deref(),
    ]
    .into_iter()
    .flatten()
    {
        for (package, version, out_dir) in generated_out_dir_map_records(map_dir) {
            let Some(unit_relative_path) = build_unit_relative_path(&out_dir) else {
                continue;
            };
            if !out_dir_holds_rust(&out_dir) || !seen.insert(unit_relative_path.clone()) {
                continue;
            }
            units.push(BakeGeneratedOutDir {
                crate_name: package,
                unit_relative_path,
                out_dir,
                version: Some(version),
            });
        }
    }
    for target_dir in bake_generated_out_dir_targets(library)? {
        for generated in bake_generated_out_dirs(&target_dir)? {
            if seen.insert(generated.unit_relative_path.clone()) {
                units.push(generated);
            }
        }
    }
    Ok(units)
}

/// Read the package-keyed build-script output map written beside `dir`, as `(package, version, OUT_DIR)`.
#[allow(unused_variables)]
fn generated_out_dir_map_records(dir: &Path) -> Vec<(String, String, PathBuf)> {
    #[cfg(feature = "rust_inspect")]
    let records = crate::rust_inspect::read_generated_out_dirs_map(dir)
        .into_iter()
        .map(|record| (record.package, record.version, record.out_dir))
        .collect();
    #[cfg(not(feature = "rust_inspect"))]
    let records = Vec::new();
    records
}

/// Return the build-unit path below `build/` for one `out` directory, in either layout.
///
/// Oven's bootstrap lays build units out as `build/<crate>/<hash>/out`; a plain Cargo target uses
/// `build/<crate>-<hash>/out`. Anything else is not a build-script output directory this bake seals.
fn build_unit_relative_path(out_dir: &Path) -> Option<String> {
    if out_dir.file_name()? != "out" {
        return None;
    }
    let mut components = Vec::new();
    let mut cursor = out_dir.parent()?;
    loop {
        let name = cursor.file_name()?.to_str()?;
        if name == "build" {
            break;
        }
        if components.len() == 2 {
            return None;
        }
        components.push(name.to_string());
        cursor = cursor.parent()?;
    }
    components.reverse();
    Some(components.join("/"))
}

/// Return whether one build-script output directory holds generated Rust worth sealing.
fn out_dir_holds_rust(dir: &Path) -> bool {
    fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .any(|entry| entry.path().extension().and_then(|ext| ext.to_str()) == Some("rs"))
        })
        .unwrap_or(false)
}

/// Return the Cargo target directories whose build-script output the explicit bake can seal for one library.
///
/// Generated Rust reaches the bake by two routes. A direct-rustc bake's rust-inspect workspace names its Cargo
/// target through `.cargo/config.toml`; the bounded compatibility baker builds the library through one unified Cargo
/// invocation in the generated project's own selected target and leaves no rust-inspect config behind. Either can
/// exist alone, so both are offered, in that order, and the sealer deduplicates by build unit.
fn bake_generated_out_dir_targets(library: &LibraryInspectionConstituent) -> CliResult<Vec<PathBuf>> {
    let mut targets = Vec::new();
    if let Some(manifest_dir) = library.rust_inspect_manifest_dir.as_deref()
        && let Some(target_dir) = rust_inspect_workspace_cargo_target(manifest_dir)?
    {
        targets.push(target_dir);
    }
    if let Some(target_dir) = library.cargo_target_dir.clone()
        && !targets.contains(&target_dir)
    {
        targets.push(target_dir);
    }
    Ok(targets)
}

/// Return the Cargo target a rust-inspect workspace names through its `.cargo/config.toml`, if it names one.
fn rust_inspect_workspace_cargo_target(rust_inspect_manifest_dir: &Path) -> CliResult<Option<PathBuf>> {
    let config_path = rust_inspect_manifest_dir.join(".cargo").join("config.toml");
    let Ok(config) = fs::read_to_string(&config_path) else {
        return Ok(None);
    };
    let config = toml::from_str::<toml::Value>(&config).map_err(|error| {
        CliError::failure(format!(
            "rust-inspect Cargo config {} is not valid TOML: {error}",
            config_path.display()
        ))
    })?;
    Ok(config
        .get("build")
        .and_then(|build| build.get("target-dir"))
        .and_then(toml::Value::as_str)
        .map(PathBuf::from))
}

/// Return the build-script output directories the explicit bake's Cargo bootstrap left below one Cargo target.
///
/// Oven's bootstrap lays build units out as `debug/build/<crate>/<hash>/out`; a plain Cargo target uses
/// `debug/build/<crate>-<hash>/out`. Every `out` that holds generated Rust is returned with its package name and the
/// build-unit path below `build/`, so the sealed copy keeps the layout the generated-code route already recognizes.
fn bake_generated_out_dirs(cargo_target_dir: &Path) -> CliResult<Vec<BakeGeneratedOutDir>> {
    let build_dir = cargo_target_dir.join("debug").join("build");
    let Ok(units) = fs::read_dir(&build_dir) else {
        return Ok(Vec::new());
    };
    let holds_rust = out_dir_holds_rust;
    let mut out_dirs = Vec::new();
    for unit in units.flatten() {
        let unit_name = unit.file_name().to_string_lossy().into_owned();
        let direct_out = unit.path().join("out");
        if direct_out.is_dir() {
            // Cargo layout: `<crate>-<hash>/out`.
            if let Some((crate_name, _)) = unit_name.rsplit_once('-')
                && holds_rust(&direct_out)
            {
                out_dirs.push(BakeGeneratedOutDir {
                    crate_name: crate_name.to_string(),
                    unit_relative_path: unit_name.clone(),
                    out_dir: direct_out,
                    version: None,
                });
            }
            continue;
        }
        // Oven layout: `<crate>/<hash>/out`.
        let Ok(hashes) = fs::read_dir(unit.path()) else {
            continue;
        };
        for hash in hashes.flatten() {
            let out_dir = hash.path().join("out");
            if out_dir.is_dir() && holds_rust(&out_dir) {
                out_dirs.push(BakeGeneratedOutDir {
                    crate_name: unit_name.clone(),
                    unit_relative_path: format!("{unit_name}/{}", hash.file_name().to_string_lossy()),
                    out_dir,
                    version: None,
                });
            }
        }
    }
    out_dirs.sort_by(|left, right| left.unit_relative_path.cmp(&right.unit_relative_path));
    Ok(out_dirs)
}

/// One build-script output directory the explicit bake can seal for direct inspection.
#[derive(Debug, Clone, PartialEq, Eq)]
struct BakeGeneratedOutDir {
    crate_name: String,
    /// Build-unit path below the target's `build/` directory, in whichever layout the bootstrap used.
    unit_relative_path: String,
    out_dir: PathBuf,
    /// Exact package version whose build script wrote the directory, when a package-keyed map named it.
    version: Option<String>,
}

/// Seal the project's inspection authority: the constituents a normal command may inspect through, the registry
/// sources each one owns, the test-dependency envelope, and the build-script output the bake's Cargo targets wrote.
///
/// The library constituent, when the bake produced one, joins the constituents under the kind and base its selection
/// had, and its registry sources are added only where no earlier constituent already names the locked package.
fn publish_project_inspection_authority(
    store: &OvenStore,
    project_root: &Path,
    source_authority_digest: &str,
    registry_dependencies: &[DependencySpec],
    dev_registry_dependencies: &[DependencySpec],
    test_dependency_envelope: &PreparedOvenTestDependencyEnvelope,
    library: Option<&LibraryInspectionConstituent>,
) -> CliResult<PublishedProjectInspectionAuthority> {
    let receipt = &test_dependency_envelope.receipt;
    let selection = &test_dependency_envelope.plan_selection;
    let (
        constituents,
        mut registry_sources,
        lock_path,
        publisher_normal_roots,
        publisher_dev_roots,
        test_dependency_constituent_index,
    ) = match selection {
        OvenDirectRustcPlanSelection::Stored(selected) => {
            if selected.artifacts.intent != receipt.intent {
                return Err(CliError::failure(
                    "project inspection authority selected a stored direct plan with a different build intent",
                ));
            }
            let sources = selected
                .artifacts
                .registry_sources
                .iter()
                .cloned()
                .map(|package| OvenProjectInspectionSource {
                    package,
                    owner: OvenProjectInspectionSourceOwner::Constituent { index: 0 },
                })
                .collect::<Vec<_>>();
            (
                vec![OvenProjectInspectionConstituent::Stored {
                    identity: selected.identity.clone(),
                    artifact_kind: OvenArtifactKind::DirectRustcPlan,
                    receipt: receipt.clone(),
                    base_loaf_identity: None,
                }],
                sources,
                selected.artifact_root.join(OVEN_RUSTC_REGISTRY_LOCK_RELATIVE_PATH),
                None,
                None,
                Some(0),
            )
        }
        OvenDirectRustcPlanSelection::ToolchainLoaf(native) => {
            if native.artifacts.intent != receipt.intent {
                return Err(CliError::failure(
                    "project inspection authority selected a release Loaf with a different build intent",
                ));
            }
            let sources = native
                .artifacts
                .registry_sources
                .iter()
                .cloned()
                .map(|package| OvenProjectInspectionSource {
                    package,
                    owner: OvenProjectInspectionSourceOwner::Constituent { index: 0 },
                })
                .collect::<Vec<_>>();
            (
                vec![OvenProjectInspectionConstituent::ReleaseLoaf {
                    loaf_identity: native.loaf_identity.clone(),
                    build_unit_identity: native.loaf_build_unit_identity.clone(),
                    receipt: receipt.clone(),
                }],
                sources,
                native.artifact_root.join(OVEN_RUSTC_REGISTRY_LOCK_RELATIVE_PATH),
                None,
                None,
                Some(0),
            )
        }
        OvenDirectRustcPlanSelection::ProjectExtension(extension) => {
            if extension.source_payload.complete_plan.intent != receipt.intent
                || extension.extension.receipt.identity != receipt.identity
            {
                return Err(CliError::failure(
                    "project inspection authority selected a different receipt or build intent from its debug output",
                ));
            }
            let constituents = vec![
                OvenProjectInspectionConstituent::ReleaseLoaf {
                    loaf_identity: extension.base.loaf_identity.clone(),
                    build_unit_identity: extension.base.loaf_build_unit_identity.clone(),
                    receipt: receipt.clone(),
                },
                OvenProjectInspectionConstituent::Stored {
                    identity: extension.extension.identity.clone(),
                    artifact_kind: OvenArtifactKind::ProjectPayload,
                    receipt: extension.extension.receipt.clone(),
                    base_loaf_identity: Some(extension.base.loaf_identity.clone()),
                },
            ];
            let extension_paths = extension
                .source_payload
                .extension_paths
                .iter()
                .map(String::as_str)
                .collect::<BTreeSet<_>>();
            let mut sources = Vec::with_capacity(extension.source_payload.complete_plan.registry_sources.len());
            for package in &extension.source_payload.complete_plan.registry_sources {
                let prefix = format!("{}/", package.source.relative_root);
                let extension_owns = extension_paths
                    .iter()
                    .any(|path| *path == package.source.relative_root || path.starts_with(&prefix));
                let owner = if extension_owns {
                    OvenProjectInspectionSourceOwner::Constituent { index: 1 }
                } else {
                    // Source-inspection authority only needs the same locked source text, not the same compiled
                    // feature selection: `package`, `version`, and `source` (registry, checksum, staged content
                    // digest) together already pin one exact, receipt-checked source archive. Two independently
                    // resolved builds can unify a shared transitive dependency's Cargo features differently (for
                    // example the base release Loaf's own closure enabling only `std` where a project's closure
                    // also enables `default`) without the underlying vendored source ever differing. Compiled
                    // artifact reuse remains governed separately by `registry_leaves`, which does still require an
                    // exact feature match.
                    let matching_base = extension
                        .base
                        .artifacts
                        .registry_sources
                        .iter()
                        .filter(|candidate| {
                            candidate.package == package.package
                                && candidate.version == package.version
                                && candidate.source == package.source
                        })
                        .count();
                    if matching_base != 1 {
                        return Err(CliError::failure(format!(
                            "project inspection source `{}` {} is absent from both its project fragment and exact release Loaf",
                            package.package, package.version
                        )));
                    }
                    OvenProjectInspectionSourceOwner::Constituent { index: 0 }
                };
                sources.push(OvenProjectInspectionSource {
                    package: package.clone(),
                    owner,
                });
            }
            (
                constituents,
                sources,
                extension
                    .extension
                    .artifact_root
                    .join(OVEN_RUSTC_REGISTRY_LOCK_RELATIVE_PATH),
                Some(extension.source_payload.registry_source_dependencies.as_slice()),
                Some(extension.source_payload.dev_registry_source_dependencies.as_slice()),
                Some(1),
            )
        }
        OvenDirectRustcPlanSelection::PackagedProvider(_) => {
            return Err(CliError::failure(
                "explicit Oven project bake cannot use a package-provider composition as its project-owned test dependency envelope",
            ));
        }
    };
    let mut constituents = constituents;
    if let Some(library) = library
        && !constituents.iter().any(|constituent| {
            matches!(constituent, OvenProjectInspectionConstituent::Stored { identity, .. } if *identity == library.identity)
        })
    {
        if library.artifacts.intent != library.receipt.intent {
            return Err(CliError::failure(
                "project inspection authority library constituent has a different build intent from its receipt",
            ));
        }
        let index = constituents.len();
        constituents.push(OvenProjectInspectionConstituent::Stored {
            identity: library.identity.clone(),
            artifact_kind: library.artifact_kind,
            receipt: library.receipt.clone(),
            base_loaf_identity: library.base_loaf_identity.clone(),
        });
        // The library's registry sources overlap the test envelope's almost entirely; only the sources absent from
        // every earlier constituent are added, because the authority may name each locked package once.
        for package in &library.artifacts.registry_sources {
            let known = registry_sources.iter().any(|source| {
                source.package.package == package.package
                    && source.package.version == package.version
                    && source.package.source.registry == package.source.registry
                    && source.package.source.checksum == package.source.checksum
            });
            if !known {
                registry_sources.push(OvenProjectInspectionSource {
                    package: package.clone(),
                    owner: OvenProjectInspectionSourceOwner::Constituent { index },
                });
            }
        }
    }
    registry_sources.sort_by(|left, right| {
        (
            &left.package.package,
            &left.package.version,
            &left.package.source.registry,
            &left.package.source.checksum,
        )
            .cmp(&(
                &right.package.package,
                &right.package.version,
                &right.package.source.registry,
                &right.package.source.checksum,
            ))
    });
    let source_catalog = registry_sources
        .iter()
        .map(|source| source.package.clone())
        .collect::<Vec<_>>();
    let publisher_roots = publisher_normal_roots
        .into_iter()
        .flatten()
        .chain(publisher_dev_roots.into_iter().flatten())
        .cloned()
        .collect::<Vec<_>>();
    let publisher_roots = (!publisher_roots.is_empty()).then_some(publisher_roots.as_slice());
    let registry_source_dependencies =
        project_inspection_root_dependencies(registry_dependencies, &source_catalog, publisher_roots)?;
    let dev_registry_source_dependencies =
        project_inspection_root_dependencies(dev_registry_dependencies, &source_catalog, publisher_roots)?;
    let test_dependency_envelope = test_dependency_constituent_index
        .map(|constituent_index| {
            project_inspection_test_dependency_roots(
                &test_dependency_envelope.dependencies,
                &test_dependency_envelope.dependency_root_digests,
                &registry_source_dependencies,
                &dev_registry_source_dependencies,
            )
            .map(|dependency_roots| OvenProjectInspectionTestDependencyEnvelope {
                constituent_index,
                dependency_surface_digest: test_dependency_envelope.dependency_surface_digest.clone(),
                dependency_roots,
            })
        })
        .transpose()?;
    let (registry_lock_digest, mut materialized_files) = if registry_sources.is_empty() {
        (digest_bytes(&[]), Vec::new())
    } else {
        let lock = fs::read(&lock_path).map_err(|error| {
            CliError::failure(format!(
                "project inspection authority cannot read its canonical publisher lock {}: {error}",
                lock_path.display()
            ))
        })?;
        (
            digest_bytes(&lock),
            vec![OvenArtifactMaterializedFile {
                source_path: lock_path,
                relative_path: OVEN_RUSTC_REGISTRY_LOCK_RELATIVE_PATH.to_string(),
            }],
        )
    };
    // Seal the generated Rust the Cargo bootstrap wrote for the library's dependencies. Those files are the only
    // form in which prost's `include!`d modules exist; a normal command has no Cargo to regenerate them, so the
    // authority carries them and direct inspection workspaces read them from here.
    let mut generated_out_dirs = Vec::new();
    let generated_units = library
        .map(bake_generated_out_dir_units)
        .transpose()?
        .unwrap_or_default();
    for generated in generated_units {
        {
            let BakeGeneratedOutDir {
                crate_name,
                unit_relative_path,
                out_dir,
                version,
            } = generated;
            let relative_root = format!("generated-out-dirs/build/{unit_relative_path}/out");
            let mut sealed = false;
            for entry in fs::read_dir(&out_dir)
                .map_err(|error| CliError::failure(format!("cannot read {}: {error}", out_dir.display())))?
                .flatten()
            {
                let path = entry.path();
                if path.extension().and_then(|ext| ext.to_str()) != Some("rs") {
                    continue;
                }
                let file_name = entry.file_name().to_string_lossy().into_owned();
                materialized_files.push(OvenArtifactMaterializedFile {
                    source_path: path,
                    relative_path: format!("{relative_root}/{file_name}"),
                });
                sealed = true;
            }
            if sealed {
                generated_out_dirs.push(OvenProjectInspectionGeneratedOutDir {
                    crate_name,
                    relative_root,
                    version,
                });
            }
        }
    }
    let payload = OvenProjectInspectionAuthorityPayload {
        schema_version: OVEN_PROJECT_INSPECTION_AUTHORITY_SCHEMA_VERSION,
        project_identity: baked_project_owner_identity(project_root)?,
        source_authority_digest: source_authority_digest.to_string(),
        compiler_version: INCAN_VERSION.to_string(),
        registry_lock_digest,
        registry_source_dependencies,
        dev_registry_source_dependencies,
        test_dependency_envelope,
        constituents,
        registry_sources,
        generated_out_dirs,
    };
    validate_project_inspection_authority_payload(&payload).map_err(oven_rustc_error)?;
    let payload = serde_json::to_vec(&payload)
        .map_err(|error| CliError::failure(format!("failed to serialize project inspection authority: {error}")))?;
    let request = OvenArtifactPublishRequest {
        receipt: receipt.clone(),
        domain: format!("incan-release-{INCAN_VERSION}"),
        kind: OvenArtifactKind::ProjectInspectionAuthority,
        payload,
        materialized_files,
    };
    let deadline = Instant::now() + OVEN_PROJECT_OUTPUT_PUBLICATION_WAIT;
    let manifest = loop {
        match store.publish(&request) {
            Ok(manifest) => break manifest,
            Err(OvenStoreError::LegacyPublisherStagingActive { .. }) if Instant::now() < deadline => {
                std::thread::sleep(OVEN_PROJECT_OUTPUT_PUBLICATION_RETRY);
            }
            Err(error) => {
                return Err(CliError::failure(format!(
                    "failed to publish project inspection authority: {error}"
                )));
            }
        }
    };
    let (selected, _, _, lease) = store
        .select_payload_for_execution(&manifest.identity)
        .map_err(|error| CliError::failure(format!("failed to lease project inspection authority: {error}")))?;
    if selected.kind != OvenArtifactKind::ProjectInspectionAuthority
        || selected.receipt_identity != receipt.identity
        || selected.build_unit_identity != receipt.build_unit_identity
    {
        return Err(CliError::failure(
            "published project inspection authority changed identity, receipt, or kind during lease acquisition",
        ));
    }
    Ok(PublishedProjectInspectionAuthority {
        reference: OvenProjectInspectionAuthorityRef {
            identity: selected.identity,
            receipt_identity: selected.receipt_identity,
            build_unit_identity: selected.build_unit_identity,
        },
        _lease: lease,
    })
}

/// Publish one completed project-native output at the explicit project bake boundary.
///
/// The project-output Loaf shares the release compatibility domain with its selected direct-rustc inputs, so ordinary
/// bounded retention can reclaim an inactive completed output without creating a separate unbounded cache class.
fn publish_project_output_loaf(
    store: &OvenStore,
    receipt: &crate::oven::OvenReceipt,
    payload: &OvenProjectOutputPayload,
    files: &[OvenProjectOutputBakeFile],
) -> CliResult<OvenStoredProjectOutput> {
    receipt
        .verify_identity()
        .map_err(|error| CliError::failure(format!("cannot publish invalid Oven project-output receipt: {error}")))?;
    if payload.schema_version != OVEN_PROJECT_OUTPUT_PAYLOAD_SCHEMA_VERSION
        || payload.compiler_version != INCAN_VERSION
        || payload.target_identity.trim().is_empty()
        || payload.receipt_identity != receipt.identity
        || payload.build_unit_identity != receipt.build_unit_identity
        || payload.plan_identity.trim().is_empty()
        || payload.files.is_empty()
        || payload
            .build_report
            .as_ref()
            .is_some_and(|snapshot| snapshot.schema_version != OVEN_PROJECT_OUTPUT_REPORT_SCHEMA_VERSION)
    {
        return Err(CliError::failure(
            "cannot publish project-output Loaf with inconsistent receipt or output authority",
        ));
    }
    let sources = files
        .iter()
        .map(|file| (file.output_relative_path.as_str(), file))
        .collect::<BTreeMap<_, _>>();
    if sources.len() != files.len() || sources.len() != payload.files.len() {
        return Err(CliError::failure(
            "cannot publish Oven project-output Loaf with duplicate or missing output files",
        ));
    }
    for file in &payload.files {
        let Some(source) = sources.get(file.output_relative_path.as_str()) else {
            return Err(CliError::failure(
                "cannot publish Oven project-output Loaf whose payload omits a source file",
            ));
        };
        if source.caller_relative_path != file.caller_relative_path
            || digest_bytes(&fs::read(&source.source_path).map_err(|error| {
                CliError::failure(format!(
                    "cannot publish missing Oven project output {}: {error}",
                    source.source_path.display()
                ))
            })?) != file.digest
        {
            return Err(CliError::failure(
                "cannot publish Oven project output whose source differs from its payload",
            ));
        }
    }
    let payload_bytes = serde_json::to_vec(payload)
        .map_err(|error| CliError::failure(format!("failed to serialize Oven project-output Loaf: {error}")))?;
    let request = OvenArtifactPublishRequest {
        receipt: receipt.clone(),
        domain: format!("incan-release-{INCAN_VERSION}"),
        kind: OvenArtifactKind::ProjectOutput,
        payload: payload_bytes,
        materialized_files: files
            .iter()
            .map(|file| OvenArtifactMaterializedFile {
                source_path: file.source_path.clone(),
                relative_path: file.output_relative_path.clone(),
            })
            .collect(),
    };
    let deadline = Instant::now() + OVEN_PROJECT_OUTPUT_PUBLICATION_WAIT;
    let manifest = loop {
        match store.publish(&request) {
            Ok(manifest) => break manifest,
            Err(OvenStoreError::LegacyPublisherStagingActive { .. }) if Instant::now() < deadline => {
                // The active named publisher owns the remaining physical staging capacity. Waiting preserves that hard
                // bound while allowing independent explicit project bakes to converge safely.
                std::thread::sleep(OVEN_PROJECT_OUTPUT_PUBLICATION_RETRY);
            }
            Err(error) => {
                return Err(CliError::failure(format!(
                    "failed to publish Oven project-output Loaf: {error}"
                )));
            }
        }
    };
    let selected = store
        .select_payload_for_execution(&manifest.identity)
        .map_err(|error| {
            CliError::failure(format!(
                "failed to acquire the newly published Oven project-output lease: {error}"
            ))
        })?;
    if selected.0 != manifest {
        return Err(CliError::failure(
            "published Oven project output changed its immutable manifest during exact lease acquisition",
        ));
    }
    let selected_payload = serde_json::from_slice::<OvenProjectOutputPayload>(&selected.2).map_err(|error| {
        CliError::failure(format!(
            "newly published Oven project output has an invalid payload: {error}"
        ))
    })?;
    if &selected_payload != payload {
        return Err(CliError::failure(
            "newly published Oven project output differs from the completed target payload",
        ));
    }
    stored_project_output_from_parts(selected.0, selected.1, selected_payload, selected.3)
}

/// Validate one already leased project-output payload without rehashing its immutable artifact bytes.
fn stored_project_output_from_parts(
    manifest: crate::oven::store::OvenArtifactManifest,
    artifact_root: PathBuf,
    payload: OvenProjectOutputPayload,
    lease: OvenStoreLease,
) -> CliResult<OvenStoredProjectOutput> {
    if manifest.kind != OvenArtifactKind::ProjectOutput
        || payload.schema_version != OVEN_PROJECT_OUTPUT_PAYLOAD_SCHEMA_VERSION
        || payload.compiler_version != INCAN_VERSION
        || payload.target_identity.trim().is_empty()
        || payload.receipt_identity != manifest.receipt_identity
        || payload.build_unit_identity != manifest.build_unit_identity
        || payload.plan_identity.trim().is_empty()
        || payload.files.is_empty()
        || payload
            .build_report
            .as_ref()
            .is_some_and(|snapshot| snapshot.schema_version != OVEN_PROJECT_OUTPUT_REPORT_SCHEMA_VERSION)
    {
        return Err(CliError::failure(format!(
            "selected Oven project-output Loaf `{}` has inconsistent schema, receipt, compiler, or output authority",
            manifest.identity
        )));
    }
    let inspection_authority = payload.inspection_authority.as_ref().ok_or_else(|| {
        CliError::failure(format!(
            "selected Oven project-output Loaf `{}` has no project inspection authority",
            manifest.identity
        ))
    })?;
    if inspection_authority.identity.trim().is_empty()
        || inspection_authority.receipt_identity.trim().is_empty()
        || inspection_authority.build_unit_identity.trim().is_empty()
    {
        return Err(CliError::failure(format!(
            "selected Oven project-output Loaf `{}` has an incomplete project inspection authority",
            manifest.identity
        )));
    }
    let native_outputs = payload
        .files
        .iter()
        .filter(|file| file.output_relative_path == OVEN_PROJECT_OUTPUT_ARTIFACT_PATH)
        .collect::<Vec<_>>();
    if native_outputs.len() != 1 {
        return Err(CliError::failure(format!(
            "selected Oven project-output Loaf `{}` must contain exactly one native output",
            manifest.identity
        )));
    }
    let mut stored_paths = BTreeSet::new();
    let mut caller_paths = BTreeSet::new();
    for file in &payload.files {
        let _ = validated_project_output_relative_path(&file.output_relative_path, "stored output")?;
        let _ = validated_project_output_relative_path(&file.caller_relative_path, "caller output")?;
        if !stored_paths.insert(file.output_relative_path.as_str())
            || !caller_paths.insert(file.caller_relative_path.as_str())
        {
            return Err(CliError::failure(
                "selected Oven project-output Loaf contains duplicate output paths",
            ));
        }
    }
    if let Some(package_loaf_store_relative_path) = payload.package_loaf_store_relative_path.as_deref() {
        let _ = validated_project_output_relative_path(package_loaf_store_relative_path, "package Loaf store")?;
    } else if !payload.required_project_loafs.is_empty() {
        return Err(CliError::failure(
            "selected executable Oven project-output Loaf unexpectedly carries library package Loafs",
        ));
    }
    let native = native_outputs[0];
    let native_output = artifact_root.join(validated_project_output_relative_path(
        &native.output_relative_path,
        "stored output",
    )?);
    let metadata = fs::metadata(&native_output).map_err(|error| {
        CliError::failure(format!(
            "selected Oven project-output Loaf is missing its sealed native output {}: {error}",
            native_output.display()
        ))
    })?;
    if !metadata.is_file() || metadata.len() != native.logical_bytes {
        return Err(CliError::failure(format!(
            "selected Oven project-output Loaf native output length differs at {}",
            native_output.display()
        )));
    }
    let profile = manifest.intent.profile.clone();
    let intent = manifest.intent.clone();
    Ok(OvenStoredProjectOutput {
        identity: manifest.identity,
        profile,
        intent,
        payload,
        artifact_root,
        native_output,
        _lease: lease,
    })
}

/// Derive the completed-output authority from the project state that the explicit baker has already validated and
/// compiled.
fn project_output_payload_for_bake(request: OvenProjectOutputBakeRequest<'_>) -> CliResult<OvenProjectOutputPayload> {
    request.backend_receipt.verify_identity().map_err(|error| {
        CliError::failure(format!(
            "completed Oven project output has an invalid backend receipt: {error}"
        ))
    })?;
    let entrypoint_relative_path =
        project_relative_entrypoint(request.project_root, request.entrypoint).ok_or_else(|| {
            CliError::failure(format!(
                "explicit Oven project bake entrypoint {} is outside its project root {}",
                request.entrypoint.display(),
                request.project_root.display()
            ))
        })?;
    let target_identity = oven_bake_project_target_identity(request.project_root, request.target, request.entrypoint)?;
    if request.receipt.intent.profile != request.profile {
        return Err(CliError::failure(
            "explicit Oven project bake profile differs from its prepared receipt",
        ));
    }
    let mut files = request
        .files
        .into_iter()
        .map(|file| {
            let bytes = fs::read(&file.source_path).map_err(|error| {
                CliError::failure(format!(
                    "failed to digest completed Oven project output {}: {error}",
                    file.source_path.display()
                ))
            })?;
            let logical_bytes = u64::try_from(bytes.len()).map_err(|_| {
                CliError::failure(format!(
                    "completed Oven project output {} is too large to account",
                    file.source_path.display()
                ))
            })?;
            Ok(OvenProjectOutputFile {
                caller_relative_path: file.caller_relative_path,
                output_relative_path: file.output_relative_path,
                digest: digest_bytes(&bytes),
                logical_bytes,
            })
        })
        .collect::<CliResult<Vec<_>>>()?;
    files.sort_by(|left, right| left.output_relative_path.cmp(&right.output_relative_path));
    let native_outputs = files
        .iter()
        .filter(|file| file.output_relative_path == OVEN_PROJECT_OUTPUT_ARTIFACT_PATH)
        .count();
    if native_outputs != 1 {
        return Err(CliError::failure(
            "completed Oven project-output Loaf must contain exactly one native output",
        ));
    }
    if request.inspection_authority.identity.trim().is_empty()
        || request.inspection_authority.receipt_identity.trim().is_empty()
        || request.inspection_authority.build_unit_identity.trim().is_empty()
    {
        return Err(CliError::failure(
            "completed Oven project output must name one exact project inspection authority",
        ));
    }
    let mut required_project_loafs = request.required_project_loafs;
    required_project_loafs.sort_by(|left, right| {
        (&left.identity, &left.receipt.identity).cmp(&(&right.identity, &right.receipt.identity))
    });
    required_project_loafs.dedup_by(|left, right| {
        left.identity == right.identity && left.receipt.identity == right.receipt.identity && left.kind == right.kind
    });
    if request.package_loaf_store_relative_path.is_none() && !required_project_loafs.is_empty() {
        return Err(CliError::failure(
            "completed executable Oven project output cannot carry library package Loafs",
        ));
    }
    if let Some(path) = request.package_loaf_store_relative_path.as_deref() {
        let _ = validated_project_output_relative_path(path, "package Loaf store")?;
    }
    if request
        .lock_dependencies_fingerprint
        .as_deref()
        .is_some_and(|fingerprint| fingerprint.trim().is_empty())
    {
        return Err(CliError::failure(
            "completed Oven project output cannot carry an empty lock dependency fingerprint",
        ));
    }
    if request.build_report.as_ref().is_some_and(|snapshot| {
        snapshot.schema_version != OVEN_PROJECT_OUTPUT_REPORT_SCHEMA_VERSION || !snapshot.report.is_object()
    }) {
        return Err(CliError::failure(
            "completed Oven project output cannot carry an invalid build-report snapshot",
        ));
    }
    Ok(OvenProjectOutputPayload {
        schema_version: OVEN_PROJECT_OUTPUT_PAYLOAD_SCHEMA_VERSION,
        project_target: request.target.as_str().to_string(),
        target_identity,
        project_identity: baked_project_owner_identity(request.project_root)?,
        source_authority_digest: request.source_authority_digest.to_string(),
        lock_dependencies_fingerprint: request.lock_dependencies_fingerprint,
        compiler_version: INCAN_VERSION.to_string(),
        entrypoint_relative_path,
        build_unit_identity: request.receipt.build_unit_identity.clone(),
        receipt_identity: request.receipt.identity.clone(),
        plan_identity: request.plan_identity,
        backend_receipt: request.backend_receipt,
        inspection_authority: Some(request.inspection_authority),
        files,
        required_project_loafs,
        package_loaf_store_relative_path: request.package_loaf_store_relative_path,
        build_report: request.build_report,
    })
}

/// Select a completed executable output before frontend work on an exact project match.
///
/// This intentionally consults no ambient `rustc`: the stored native output is already bound to its publisher receipt
/// and target. Fresh compilation remains responsible for resolving a compatible Rust compiler when this fast path
/// misses.
#[cfg(test)]
fn select_baked_project_output(
    store: &OvenStore,
    project_root: &Path,
    entrypoint: &Path,
    target: OvenBakeProjectTarget,
    profile: &str,
) -> CliResult<Option<OvenStoredProjectOutput>> {
    let source_authority_digest = digest_baked_project_source_authority(project_root)?;
    select_baked_project_output_with_source_authority(
        store,
        project_root,
        entrypoint,
        target,
        profile,
        &source_authority_digest,
        None,
    )
}

/// Return a stable, portable owner identity for a manifest-backed project.
///
/// Project Loafs must remain reusable from an identical clean worktree, so an absolute path cannot be part of this
/// identity. The project distribution name is the durable coordinate; the target and entrypoint remain separate payload
/// facts checked by the selector. Exact source authority prevents a same-named unrelated project from reusing a
/// different completed output; project-local receipts separately govern stale-output diagnostics.
fn baked_project_owner_identity(project_root: &Path) -> CliResult<String> {
    let manifest_path = project_root.join(MANIFEST_FILENAME);
    let manifest = ProjectManifest::load(&manifest_path).map_err(|error| CliError::failure(error.to_string()))?;
    let project_name = manifest
        .project
        .as_ref()
        .and_then(|project| project.name.as_deref())
        .unwrap_or("unnamed-incan-project");
    Ok(digest_bytes(
        format!("incan_oven_project_output_owner/1\\0{project_name}").as_bytes(),
    ))
}

/// Whether this local project has a previously baked completed output whose source authority is no longer exact.
///
/// This is deliberately distinct from ordinary selection. It never authorizes an output. The project-local receipt is
/// the lineage proof: scanning the global store by package name alone would let an unrelated same-named project trigger
/// a false stale-output refusal. Exact output selection remains portable and path-independent; only this diagnostic
/// requires evidence that this checkout crossed the explicit bake boundary.
fn has_stale_baked_project_output(
    store: &OvenStore,
    project_root: &Path,
    entrypoint: &Path,
    target: OvenBakeProjectTarget,
    profile: &str,
) -> CliResult<bool> {
    let Some(entrypoint_relative_path) = project_relative_entrypoint(project_root, entrypoint) else {
        return Ok(false);
    };
    let target_identity = oven_bake_project_target_identity(project_root, target, entrypoint)?;
    let Some(native_target) = native_project_output_target() else {
        return Ok(false);
    };
    let receipt_path = project_bake_receipt_path(project_root, target, entrypoint, profile)?;
    let receipt = match fs::read(&receipt_path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<crate::oven::OvenReceipt>(&bytes).ok())
    {
        Some(receipt)
            if receipt.verify_identity().is_ok()
                && receipt.intent.profile == profile
                && receipt.intent.target == native_target =>
        {
            receipt
        }
        Some(_) | None => return Ok(false),
    };
    let candidates = store
        .select_payloads_matching_for_execution(|manifest| {
            manifest.kind == OvenArtifactKind::ProjectOutput
                && manifest.intent.profile == profile
                && manifest.intent.target == native_target
                && manifest.receipt_identity == receipt.identity
                && manifest.build_unit_identity == receipt.build_unit_identity
        })
        .map_err(|error| CliError::failure(format!("failed to inspect Oven project-output Loafs: {error}")))?;
    for candidate in candidates {
        let Ok(payload) = serde_json::from_slice::<OvenProjectOutputPayload>(&candidate.payload) else {
            continue;
        };
        if payload.schema_version == OVEN_PROJECT_OUTPUT_PAYLOAD_SCHEMA_VERSION
            && payload.project_target == target.as_str()
            && payload.target_identity == target_identity
            && payload.compiler_version == INCAN_VERSION
            && payload.entrypoint_relative_path == entrypoint_relative_path
            && payload.receipt_identity == receipt.identity
            && payload.build_unit_identity == receipt.build_unit_identity
        {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Select a completed output after a caller has already computed the exact build-authority digest.
fn select_baked_project_output_with_source_authority(
    store: &OvenStore,
    project_root: &Path,
    entrypoint: &Path,
    target: OvenBakeProjectTarget,
    profile: &str,
    source_authority_digest: &str,
    required_target_toolchain: Option<(&str, &str)>,
) -> CliResult<Option<OvenStoredProjectOutput>> {
    let Some(entrypoint_relative_path) = project_relative_entrypoint(project_root, entrypoint) else {
        return Ok(None);
    };
    let target_identity = oven_bake_project_target_identity(project_root, target, entrypoint)?;
    if !project_root.join(MANIFEST_FILENAME).is_file() {
        return Ok(None);
    }
    let Some(native_target) = native_project_output_target() else {
        return Ok(None);
    };
    let selected = store
        .select_payloads_matching_for_execution(|manifest| {
            manifest.kind == OvenArtifactKind::ProjectOutput
                && manifest.intent.profile == profile
                && manifest.intent.target == native_target
                && required_target_toolchain.is_none_or(|(target, toolchain)| {
                    manifest.intent.target == target && manifest.intent.toolchain == toolchain
                })
        })
        .map_err(|error| CliError::failure(format!("failed to select Oven project-output Loaf: {error}")))?;
    let mut matches = Vec::new();
    for selected in selected {
        let Ok(payload) = serde_json::from_slice::<OvenProjectOutputPayload>(&selected.payload) else {
            // An unrelated prior/corrupt output is not this project's authority. Exact-lineage selection used by
            // normal tests rejects malformed matching receipts separately.
            continue;
        };
        if payload.schema_version != OVEN_PROJECT_OUTPUT_PAYLOAD_SCHEMA_VERSION
            || payload.project_target != target.as_str()
            || payload.target_identity != target_identity
            || payload.source_authority_digest != source_authority_digest
            || payload.compiler_version != INCAN_VERSION
            || payload.entrypoint_relative_path != entrypoint_relative_path
            || payload.receipt_identity != selected.manifest.receipt_identity
            || payload.build_unit_identity != selected.manifest.build_unit_identity
            || payload.plan_identity.trim().is_empty()
        {
            continue;
        }
        let (manifest, artifact_root, _payload, lease) = selected.into_parts();
        matches.push(stored_project_output_from_parts(
            manifest,
            artifact_root,
            payload,
            lease,
        )?);
    }
    // An interrupted or repeated explicit bake may legitimately leave more than one fully verified result for the same
    // source, receipt, plan, profile, and native target. They are interchangeable at the completed project-output
    // boundary, so normal selection is deterministic and bounded retention can reclaim inactive duplicates without
    // asking the user to bake again.
    matches.sort_by(|left, right| left.identity.cmp(&right.identity));
    Ok(matches.into_iter().next())
}

/// Select every source-current baked debug output through one leased ProjectOutput candidate scan.
///
/// Local explicit-bake receipts identify the exact target lineages before any payload is decoded. A malformed payload
/// from another project is therefore irrelevant, while malformed bytes under an expected receipt fail closed.
#[cfg(feature = "rust_inspect")]
fn select_current_debug_project_outputs(
    store: &OvenStore,
    project_root: &Path,
    targets: &[(OvenBakeProjectTarget, PathBuf)],
    source_authority_digest: &str,
    native_target: &str,
    toolchain: &str,
) -> CliResult<Option<Vec<(OvenBakeProjectTarget, OvenStoredProjectOutput)>>> {
    let project_identity = baked_project_owner_identity(project_root)?;
    let mut expected = Vec::with_capacity(targets.len());
    for (target, entrypoint) in targets {
        let Some(entrypoint_relative_path) = project_relative_entrypoint(project_root, entrypoint) else {
            return Ok(None);
        };
        let target_identity = oven_bake_project_target_identity(project_root, *target, entrypoint)?;
        let receipt_path = project_bake_receipt_path(project_root, *target, entrypoint, "debug")?;
        let receipt = match fs::read(&receipt_path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<crate::oven::OvenReceipt>(&bytes).ok())
        {
            Some(receipt)
                if receipt.verify_identity().is_ok()
                    && receipt.intent.profile == "debug"
                    && receipt.intent.target == native_target
                    && receipt.intent.toolchain == toolchain =>
            {
                receipt
            }
            Some(_) | None => return Ok(None),
        };
        expected.push(CurrentDebugProjectOutputExpectation {
            target: *target,
            target_identity,
            entrypoint_relative_path,
            receipt,
        });
    }
    let candidates = store
        .select_payloads_matching_for_execution(|manifest| {
            manifest.kind == OvenArtifactKind::ProjectOutput
                && manifest.intent.profile == "debug"
                && manifest.intent.target == native_target
                && manifest.intent.toolchain == toolchain
                && expected.iter().any(|expected| {
                    manifest.receipt_identity == expected.receipt.identity
                        && manifest.build_unit_identity == expected.receipt.build_unit_identity
                        && manifest.intent == expected.receipt.intent
                })
        })
        .map_err(|error| CliError::failure(format!("failed to select Oven project-output Loafs: {error}")))?;
    let mut grouped = (0..expected.len()).map(|_| Vec::new()).collect::<Vec<_>>();
    for candidate in candidates {
        let matching = expected
            .iter()
            .enumerate()
            .filter(|(_, expected)| {
                candidate.manifest.receipt_identity == expected.receipt.identity
                    && candidate.manifest.build_unit_identity == expected.receipt.build_unit_identity
                    && candidate.manifest.intent == expected.receipt.intent
            })
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        match matching.as_slice() {
            [] => {
                // Do not decode unrelated output payloads. Their leases are dropped after this one in-memory scan.
            }
            [index] => grouped[*index].push(candidate),
            _ => {
                return Err(CliError::failure(
                    "baked debug targets unexpectedly share one project-output receipt lineage",
                ));
            }
        }
    }

    let mut outputs = Vec::with_capacity(expected.len());
    for (expected, mut candidates) in expected.into_iter().zip(grouped) {
        if candidates.is_empty() {
            return Ok(None);
        }
        candidates.sort_by(|left, right| left.manifest.identity.cmp(&right.manifest.identity));
        let mut exact = Vec::with_capacity(candidates.len());
        let mut rejected = Vec::new();
        for candidate in candidates {
            let payload = match serde_json::from_slice::<OvenProjectOutputPayload>(&candidate.payload) {
                Ok(payload) => payload,
                Err(error) => {
                    rejected.push(CliError::failure(format!(
                        "exact debug {} project-output Loaf `{}` has an invalid payload: {error}",
                        expected.target.as_str(),
                        candidate.manifest.identity
                    )));
                    continue;
                }
            };
            if payload.project_target != expected.target.as_str()
                || payload.target_identity != expected.target_identity
                || payload.project_identity != project_identity
                || payload.source_authority_digest != source_authority_digest
                || payload.compiler_version != INCAN_VERSION
                || payload.entrypoint_relative_path != expected.entrypoint_relative_path
                || payload.receipt_identity != expected.receipt.identity
                || payload.build_unit_identity != expected.receipt.build_unit_identity
            {
                rejected.push(CliError::failure(format!(
                    "exact debug {} project-output Loaf `{}` disagrees with its source-current lineage; rerun `incan oven bake --project .`",
                    expected.target.as_str(),
                    candidate.manifest.identity
                )));
                continue;
            }
            let (manifest, artifact_root, _payload, lease) = candidate.into_parts();
            match stored_project_output_from_parts(manifest, artifact_root, payload, lease) {
                Ok(output) => exact.push(output),
                Err(error) => rejected.push(error),
            }
        }
        exact.sort_by(|left, right| left.identity.cmp(&right.identity));
        let selected = if let Some(selected) = exact.into_iter().next() {
            selected
        } else if let Some(error) = rejected.into_iter().next() {
            return Err(error);
        } else {
            return Err(CliError::failure(
                "exact debug project-output lineage disappeared during in-memory selection",
            ));
        };
        outputs.push((expected.target, selected));
    }
    Ok(Some(outputs))
}

/// Load immutable Rust-inspection lineage from all source-current baked debug outputs.
///
/// This is command-scoped authority preparation for `incan test`: the caller opens one bounded store, computes one
/// source digest, and retains both completed-output and exact entry leases across all scheduled harness batches.
/// Missing target output is a cache miss; nonempty malformed or absent lineage is an explicit rebake error.
#[cfg(feature = "rust_inspect")]
pub(crate) fn load_current_project_registry_source_authorities(
    store: &OvenStore,
    project_root: &Path,
) -> CliResult<Option<OvenLoadedProjectInspectionAuthority>> {
    let targets = discover_oven_bake_project_targets(project_root)?;
    let source_authority_digest = digest_baked_project_source_authority(project_root)?;
    let rustc = resolve_active_rustc().map_err(|error| CliError::failure(error.to_string()))?;
    let target = rustc_host_target(&rustc).map_err(|error| CliError::failure(error.to_string()))?;
    let toolchain = rustc_identity(&rustc).map_err(|error| CliError::failure(error.to_string()))?;
    let Some(outputs) = select_current_debug_project_outputs(
        store,
        project_root,
        &targets,
        &source_authority_digest,
        &target,
        &toolchain,
    )?
    else {
        return Ok(None);
    };
    let preferred = outputs
        .iter()
        .find(|(project_target, _)| *project_target == OvenBakeProjectTarget::Library)
        .or_else(|| outputs.first())
        .ok_or_else(|| CliError::failure("Oven project target discovery returned no baked target"))?;
    let authority_ref = preferred.1.payload.inspection_authority.clone().ok_or_else(|| {
        CliError::failure(
            "source-current Oven project output has no project inspection authority; rerun `incan oven bake --project .`",
        )
    })?;
    if outputs.iter().any(|(_, output)| {
        output.payload.inspection_authority.as_ref() != Some(&authority_ref)
            || output.payload.project_identity != preferred.1.payload.project_identity
            || output.payload.source_authority_digest != preferred.1.payload.source_authority_digest
            || output.payload.compiler_version != preferred.1.payload.compiler_version
    }) {
        return Err(CliError::failure(
            "source-current debug project outputs disagree on their singular Rust inspection authority; rerun `incan oven bake --project .`",
        ));
    }
    let project_identity = preferred.1.payload.project_identity.clone();
    let compiler_version = preferred.1.payload.compiler_version.clone();
    let output_leases = outputs.into_iter().map(|(_, output)| output._lease).collect();
    let mut authority = load_project_inspection_authority(
        store,
        &authority_ref,
        &project_identity,
        &source_authority_digest,
        &compiler_version,
    )
    .map_err(|error| CliError::failure(error.to_string()))?;
    authority.retain_lineage_leases(output_leases);
    Ok(Some(authority))
}

/// Visit only compiler-owned filesystem fields in a serialized build report.
///
/// Semantic strings such as package names, features, imports, and notes deliberately never enter this traversal. A
/// tagged portable path therefore cannot collide with authored text that happens to resemble an internal token.
fn transform_project_output_report_paths(
    report: &mut serde_json::Value,
    mut transform: impl FnMut(&mut serde_json::Value) -> CliResult<()>,
) -> CliResult<()> {
    for pointer in [
        "/project/project_root",
        "/entrypoint",
        "/library_root",
        "/generated/project_path",
        "/generated/manifest_path",
        "/generated/crate_root",
        "/generated/cargo_target_dir",
        "/generated/oven_output_dir",
    ] {
        if let Some(value) = report.pointer_mut(pointer) {
            transform(value)?;
        }
    }

    for (pointer, fields) in [
        ("/source_files", &["path"][..]),
        ("/artifacts", &["path"][..]),
        ("/dependencies/incan", &["path"][..]),
        ("/semantic/packages", &["project_root"][..]),
        ("/semantic/feature_edges", &["from", "to"][..]),
    ] {
        let Some(values) = report.pointer_mut(pointer) else {
            continue;
        };
        let values = values.as_array_mut().ok_or_else(|| {
            CliError::failure(format!("completed Oven build report field `{pointer}` is not an array"))
        })?;
        for value in values {
            let object = value.as_object_mut().ok_or_else(|| {
                CliError::failure(format!(
                    "completed Oven build report field `{pointer}` contains a non-object row"
                ))
            })?;
            for field in fields {
                if let Some(path) = object.get_mut(*field) {
                    transform(path)?;
                }
            }
        }
    }

    for pointer in ["/dependencies/rust", "/dependencies/rust_dev"] {
        let Some(values) = report.pointer_mut(pointer) else {
            continue;
        };
        let values = values.as_array_mut().ok_or_else(|| {
            CliError::failure(format!("completed Oven build report field `{pointer}` is not an array"))
        })?;
        for value in values {
            let object = value.as_object_mut().ok_or_else(|| {
                CliError::failure(format!(
                    "completed Oven build report field `{pointer}` contains a non-object row"
                ))
            })?;
            if object.get("source").and_then(serde_json::Value::as_str) == Some("path")
                && let Some(path) = object.get_mut("source_detail")
            {
                transform(path)?;
            }
        }
    }

    if let Some(values) = report.pointer_mut("/semantic/providers") {
        let values = values.as_array_mut().ok_or_else(|| {
            CliError::failure("completed Oven build report field `/semantic/providers` is not an array")
        })?;
        for value in values {
            let object = value.as_object_mut().ok_or_else(|| {
                CliError::failure("completed Oven build report field `/semantic/providers` contains a non-object row")
            })?;
            if let Some(path) = object.get_mut("manifest_path") {
                transform(path)?;
            }
            if let Some(provenance) = object.get_mut("provenance") {
                let provenance = provenance.as_object_mut().ok_or_else(|| {
                    CliError::failure("completed Oven build report provider provenance is not an object")
                })?;
                for field in ["manifest_path", "inventory_path"] {
                    if let Some(path) = provenance.get_mut(field) {
                        transform(path)?;
                    }
                }
            }
        }
    }
    Ok(())
}

/// Replace one absolute report path with a tagged portable project path or opaque external-authority slot.
fn seal_project_output_report_path(
    value: &mut serde_json::Value,
    project_root: &Path,
    external_paths: &mut BTreeMap<String, u64>,
) -> CliResult<()> {
    if value.is_null() {
        return Ok(());
    }
    let text = value
        .as_str()
        .ok_or_else(|| CliError::failure("completed Oven build report path field is not a string"))?;
    let path = Path::new(text);
    if !path.is_absolute() {
        return Ok(());
    }
    if let Ok(relative) = path.strip_prefix(project_root) {
        let relative = relative.to_string_lossy().replace('\\', "/");
        if relative.is_empty() || validated_project_output_relative_path(&relative, "build-report projection").is_ok() {
            *value = serde_json::json!({
                (OVEN_PROJECT_OUTPUT_REPORT_PATH_TAG): {
                    "root": "project",
                    "relative": relative,
                }
            });
            return Ok(());
        }
    }
    let next_slot = u64::try_from(external_paths.len())
        .map_err(|_| CliError::failure("completed Oven build report has too many external path authorities"))?;
    let slot = *external_paths.entry(text.to_string()).or_insert(next_slot);
    *value = serde_json::json!({
        (OVEN_PROJECT_OUTPUT_REPORT_PATH_TAG): {
            "root": "external",
            "slot": slot,
        }
    });
    Ok(())
}

/// Restore one tagged report path after exact completed-output selection.
fn restore_project_output_report_path(value: &mut serde_json::Value, project_root: &Path) -> CliResult<()> {
    if value.is_null() || value.is_string() {
        return Ok(());
    }
    let tagged = value
        .as_object()
        .filter(|object| object.len() == 1)
        .and_then(|object| object.get(OVEN_PROJECT_OUTPUT_REPORT_PATH_TAG))
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| CliError::failure("completed Oven build report path field has an invalid portable tag"))?;
    match tagged.get("root").and_then(serde_json::Value::as_str) {
        Some("project") => {
            let relative = tagged
                .get("relative")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| CliError::failure("completed Oven build report project path has no relative value"))?;
            let restored = if relative.is_empty() {
                project_root.to_path_buf()
            } else {
                project_root.join(validated_project_output_relative_path(
                    relative,
                    "build-report projection",
                )?)
            };
            *value = serde_json::Value::String(restored.to_string_lossy().to_string());
        }
        Some("external") => {
            let slot = tagged
                .get("slot")
                .and_then(serde_json::Value::as_u64)
                .ok_or_else(|| CliError::failure("completed Oven build report external path has no authority slot"))?;
            *value = serde_json::Value::String(format!("{OVEN_PROJECT_OUTPUT_REPORT_EXTERNAL_ROOT}/{slot}"));
        }
        Some(_) | None => {
            return Err(CliError::failure(
                "completed Oven build report path has an unknown portable authority",
            ));
        }
    }
    Ok(())
}

/// Reject an unclassified absolute string before a portable report can be sealed.
fn reject_unclassified_absolute_report_strings(value: &serde_json::Value) -> CliResult<()> {
    match value {
        serde_json::Value::Array(values) => {
            for value in values {
                reject_unclassified_absolute_report_strings(value)?;
            }
        }
        serde_json::Value::Object(values) => {
            for value in values.values() {
                reject_unclassified_absolute_report_strings(value)?;
            }
        }
        serde_json::Value::String(text) if Path::new(text).is_absolute() => {
            return Err(CliError::failure(
                "completed Oven build report contains an unclassified absolute path",
            ));
        }
        serde_json::Value::Null
        | serde_json::Value::Bool(_)
        | serde_json::Value::Number(_)
        | serde_json::Value::String(_) => {}
    }
    Ok(())
}

/// Replace filesystem fields in a sealed build report with collision-proof portable path tags.
fn make_project_output_report_portable(value: &mut serde_json::Value, project_root: &Path) -> CliResult<()> {
    let mut external_paths = BTreeMap::new();
    transform_project_output_report_paths(value, |path| {
        seal_project_output_report_path(path, project_root, &mut external_paths)
    })?;
    reject_unclassified_absolute_report_strings(value)
}

/// Restore only tagged project paths after an exact output has been selected; external paths remain logical tokens.
fn restore_project_output_report_paths(value: &mut serde_json::Value, project_root: &Path) -> CliResult<()> {
    transform_project_output_report_paths(value, |path| restore_project_output_report_path(path, project_root))
}

/// Seal one complete executable report while keeping caller-owned project paths relocatable.
fn project_output_report_snapshot(
    project_root: &Path,
    report: &BuildReport,
) -> CliResult<OvenProjectOutputReportSnapshot> {
    let mut report = serde_json::to_value(report)
        .map_err(|error| CliError::failure(format!("failed to serialize completed Oven build report: {error}")))?;
    make_project_output_report_portable(&mut report, project_root)?;
    Ok(OvenProjectOutputReportSnapshot {
        schema_version: OVEN_PROJECT_OUTPUT_REPORT_SCHEMA_VERSION,
        report,
    })
}

/// Return the verified implicit-default backend receipt sealed into one completed project output.
///
/// A completed output is reusable only when its own immutable payload proves that an explicit bake selected and
/// executed the ordinary legacy default. Older, malformed, or differently selected outputs deliberately return
/// `None` so the caller takes the normal source-aware path rather than treating cached provenance as current.
fn completed_output_default_backend_receipt(output: &OvenStoredProjectOutput) -> Option<BackendExecutionReceipt> {
    let receipt = &output.payload.backend_receipt;
    if receipt.verify_identity().is_err()
        || receipt.selection.selected_backend != BackendKind::Legacy
        || receipt.selection.selection_reason != crate::backend::selection::SelectionReason::Default
        || receipt.selection.fallback_policy != FallbackPolicy::Refuse
        || receipt.selection.shadow_requested
        || receipt.executed_backend != BackendKind::Legacy
        || receipt.fallback_outcome != FallbackOutcome::NotNeeded
        || receipt.shadow_comparison != ShadowComparisonState::NotRequested
    {
        return None;
    }
    Some(receipt.clone())
}

/// Materialize an executable completed output and republish the verified backend provenance it carries.
///
/// Keeping this coupled prevents either normal `build` output path from restoring native bytes while leaving a stale
/// or unrelated project-local backend receipt behind.
fn materialize_completed_executable_output(
    project_root: &Path,
    output: &OvenStoredProjectOutput,
    backend_receipt: &BackendExecutionReceipt,
) -> CliResult<()> {
    if completed_output_default_backend_receipt(output).as_ref() != Some(backend_receipt) {
        return Err(CliError::failure(
            "completed Oven executable output does not carry the backend receipt selected for reuse",
        ));
    }
    materialize_project_output(project_root, output)?;
    write_backend_receipt(backend_receipt, &default_backend_receipt_path(project_root))
}

/// Restore and validate one bake-time executable report without reconstructing frontend-owned facts.
fn completed_executable_output_report(
    project_root: &Path,
    output: &OvenStoredProjectOutput,
    backend_receipt: &BackendExecutionReceipt,
    total_start: Instant,
) -> CliResult<serde_json::Value> {
    let snapshot = output.payload.build_report.as_ref().ok_or_else(|| {
        CliError::failure(
            "completed Oven executable output has no sealed build report; rerun `incan oven bake --project .`",
        )
    })?;
    if snapshot.schema_version != OVEN_PROJECT_OUTPUT_REPORT_SCHEMA_VERSION {
        return Err(CliError::failure(
            "completed Oven executable output has an unsupported build-report schema",
        ));
    }
    let mut report = snapshot.report.clone();
    restore_project_output_report_paths(&mut report, project_root)?;
    let expected_entrypoint = project_root
        .join(validated_project_output_relative_path(
            &output.payload.entrypoint_relative_path,
            "entrypoint",
        )?)
        .to_string_lossy()
        .to_string();
    let expected = [
        ("/schema_version", BUILD_REPORT_SCHEMA_VERSION.to_string()),
        ("/compiler_version", INCAN_VERSION.to_string()),
        ("/status", "success".to_string()),
        ("/mode", "executable".to_string()),
        ("/profile", output.profile.clone()),
        ("/project/project_root", project_root.to_string_lossy().to_string()),
        ("/entrypoint", expected_entrypoint),
        ("/oven/receipt_identity", output.payload.receipt_identity.clone()),
        ("/oven/build_unit_identity", output.payload.build_unit_identity.clone()),
        ("/oven/plan_identity", output.payload.plan_identity.clone()),
    ];
    for (pointer, expected) in expected {
        let actual = report.pointer(pointer).and_then(|value| match value {
            serde_json::Value::String(value) => Some(value.clone()),
            serde_json::Value::Number(value) => Some(value.to_string()),
            _ => None,
        });
        if actual.as_deref() != Some(expected.as_str()) {
            return Err(CliError::failure(format!(
                "completed Oven executable build report disagrees with sealed output field `{pointer}`"
            )));
        }
    }
    let object = report
        .as_object_mut()
        .ok_or_else(|| CliError::failure("completed Oven executable build report is not a JSON object"))?;
    object.remove("workspace");
    let backend = serde_json::to_value(backend_receipt)
        .map_err(|error| CliError::failure(format!("failed to serialize completed-output backend receipt: {error}")))?;
    object.insert("backend".to_string(), backend);
    let elapsed = elapsed_ms(total_start);
    object.insert(
        "timings_ms".to_string(),
        serde_json::json!({
            "completed_project_output_reuse": elapsed,
            "total": elapsed,
        }),
    );
    let notes = object
        .get_mut("notes")
        .and_then(serde_json::Value::as_array_mut)
        .ok_or_else(|| CliError::failure("completed Oven executable build report has no notes array"))?;
    notes.push(serde_json::Value::String(
        "Reused a completed Oven project-output Loaf; detailed source, dependency, semantic, and interop facts were verified and sealed at explicit bake time.".to_string(),
    ));
    Ok(report)
}

/// Reconstruct the machine-readable library result from completed Loaf authority without re-entering compiler work.
fn completed_library_output_report(
    project_root: &Path,
    outputs: &[OvenStoredProjectOutput],
    total_start: Instant,
) -> CliResult<crate::cli::commands::build_report::BuildReport> {
    let Some(manifest) = discover_effective_project_manifest(project_root)? else {
        return Err(CliError::failure(format!(
            "completed Oven library output has no project manifest at {}",
            project_root.display()
        )));
    };
    let entrypoint = validate_library_entrypoint(&manifest)?;
    let project_name = manifest
        .project
        .as_ref()
        .and_then(|project| project.name.clone())
        .or_else(|| {
            project_root
                .file_name()
                .and_then(|name| name.to_str())
                .map(str::to_owned)
        })
        .unwrap_or_else(|| "incan_library".to_string());
    // Prefer the release output, but accept the profile this project actually baked: a bake narrowed by
    // `explicit_bake_profiles` has no release output, and this only names the reused library for the report.
    let release = outputs
        .iter()
        .find(|output| output.profile == "release")
        .or_else(|| outputs.first())
        .ok_or_else(|| CliError::failure("completed Oven library output has no profile to report"))?;
    let mut artifacts = Vec::new();
    for output in outputs {
        let native = output
            .payload
            .files
            .iter()
            .find(|file| file.output_relative_path == OVEN_PROJECT_OUTPUT_ARTIFACT_PATH)
            .ok_or_else(|| CliError::failure("completed Oven library output has no native artifact"))?;
        artifacts.push(artifact_report(
            format!("rust_library_{}", output.profile),
            &caller_project_output_path(project_root, &native.caller_relative_path)?,
        ));
    }
    let backend_receipt = completed_output_default_backend_receipt(release).ok_or_else(|| {
        CliError::failure("completed Oven library output has no verified implicit-default backend receipt")
    })?;
    if outputs
        .iter()
        .any(|output| completed_output_default_backend_receipt(output).as_ref() != Some(&backend_receipt))
    {
        return Err(CliError::failure(
            "completed Oven library outputs disagree on their verified implicit-default backend receipt",
        ));
    }
    let report = BuildReportDraft {
        mode: BuildReportMode::Library,
        profile: "release".to_string(),
        project: manifest_project_report(Some(&manifest), &project_name, project_root),
        entrypoint: Some(entrypoint.to_string_lossy().to_string()),
        library_root: Some(project_root.to_string_lossy().to_string()),
        source_files: Vec::new(),
        generated: oven_generated_project_report(
            &project_root.join("target/lib"),
            &project_root.join("target/lib/src/lib.rs"),
            &project_root.join("target/lib/oven"),
        ),
        artifacts,
        dependencies: crate::cli::commands::build_report::BuildDependencyReport {
            rust: Vec::new(),
            rust_dev: Vec::new(),
            incan: Vec::new(),
            stdlib_features: Vec::new(),
        },
        semantic: crate::cli::commands::build_report::BuildSemanticReport {
            sdk: None,
            packages: Vec::new(),
            feature_edges: Vec::new(),
            providers: Vec::new(),
        },
        cargo: None,
        oven: Some(BuildOvenReport {
            receipt_identity: release.payload.receipt_identity.clone(),
            build_unit_identity: release.payload.build_unit_identity.clone(),
            plan_identity: release.payload.plan_identity.clone(),
        }),
        interop: crate::cli::commands::build_report::BuildInteropReport {
            rust_imports: Vec::new(),
            rust_externs: Vec::new(),
            rust_abi_query_paths: Vec::new(),
        },
        notes: vec![
            "Reused a completed Oven project-output Loaf; source, dependency, semantic, and interop details were verified at explicit bake time and are intentionally not recomputed during this replay."
                .to_string(),
        ],
        backend: Some(backend_receipt),
    };
    let mut timings_ms = BTreeMap::new();
    timings_ms.insert("completed_project_output_reuse".to_string(), elapsed_ms(total_start));
    timings_ms.insert("total".to_string(), elapsed_ms(total_start));
    Ok(report.finish(timings_ms))
}

/// Return the caller-owned path that an exact project-output Loaf may materialize. The baked relative path is a
/// portability contract, never an arbitrary store-controlled destination.
fn caller_project_output_path(project_root: &Path, relative_path: &str) -> CliResult<PathBuf> {
    Ok(project_root.join(validated_project_output_relative_path(relative_path, "caller output")?))
}

/// Return the small caller-owned projection marker path for one completed output profile.
fn project_output_projection_marker_path(project_root: &Path, output: &OvenStoredProjectOutput) -> CliResult<PathBuf> {
    let native = output
        .payload
        .files
        .iter()
        .find(|file| file.output_relative_path == OVEN_PROJECT_OUTPUT_ARTIFACT_PATH)
        .ok_or_else(|| CliError::failure("completed Oven project-output Loaf has no native artifact"))?;
    let native_path = caller_project_output_path(project_root, &native.caller_relative_path)?;
    let parent = native_path.parent().ok_or_else(|| {
        CliError::failure(format!(
            "completed Oven project output has no native artifact directory: {}",
            native_path.display()
        ))
    })?;
    Ok(parent.join(format!(
        ".oven-project-output-{}.json",
        digest_bytes(output.payload.target_identity.as_bytes()).trim_start_matches("sha256:")
    )))
}

/// Return the stable small projection descriptor expected beside one caller-owned native output.
fn project_output_projection(output: &OvenStoredProjectOutput) -> OvenProjectOutputProjection {
    let mut files = output
        .payload
        .files
        .iter()
        .map(|file| OvenProjectOutputProjectionFile {
            caller_relative_path: file.caller_relative_path.clone(),
            digest: file.digest.clone(),
            logical_bytes: file.logical_bytes,
        })
        .collect::<Vec<_>>();
    files.sort_by(|left, right| left.caller_relative_path.cmp(&right.caller_relative_path));
    OvenProjectOutputProjection {
        schema_version: OVEN_PROJECT_OUTPUT_PROJECTION_SCHEMA_VERSION,
        output_identity: output.identity.clone(),
        files,
    }
}

/// Hash one mutable projection in bounded memory while retaining exact byte-count verification.
fn digest_project_output_projection_file(path: &Path) -> CliResult<(u64, String)> {
    let mut file = fs::File::open(path).map_err(|error| {
        CliError::failure(format!(
            "failed to open Oven project-output projection {}: {error}",
            path.display()
        ))
    })?;
    let mut hasher = Sha256::new();
    let mut logical_bytes = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(|error| {
            CliError::failure(format!(
                "failed to verify Oven project-output projection {}: {error}",
                path.display()
            ))
        })?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        let read = u64::try_from(read)
            .map_err(|_| CliError::failure("Oven project-output projection byte count exceeds the supported range"))?;
        logical_bytes = logical_bytes.checked_add(read).ok_or_else(|| {
            CliError::failure("Oven project-output projection byte count exceeds the supported range")
        })?;
    }
    Ok((logical_bytes, format!("sha256:{}", hex::encode(hasher.finalize()))))
}

/// Verify the one store-owned executable immediately before a normal `incan run` launches it.
///
/// Completed-output selection validates immutable manifest structure, while build materialization verifies every copied
/// file. Run executes directly from the store for the hot path, so it must perform this bounded native-file check
/// itself rather than trusting file length or a mutable caller projection.
fn verify_stored_project_output_native(output: &OvenStoredProjectOutput) -> CliResult<()> {
    let native = output
        .payload
        .files
        .iter()
        .find(|file| file.output_relative_path == OVEN_PROJECT_OUTPUT_ARTIFACT_PATH)
        .ok_or_else(|| CliError::failure("completed Oven project-output Loaf has no native artifact"))?;
    let metadata = fs::symlink_metadata(&output.native_output).map_err(|error| {
        CliError::failure(format!(
            "failed to inspect sealed Oven project output {}: {error}",
            output.native_output.display()
        ))
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(CliError::failure(format!(
            "sealed Oven project output must be a regular file: {}",
            output.native_output.display()
        )));
    }
    let (logical_bytes, digest) = digest_project_output_projection_file(&output.native_output)?;
    if logical_bytes != native.logical_bytes || digest != native.digest {
        return Err(CliError::failure(format!(
            "sealed Oven project output digest differs at {}",
            output.native_output.display()
        )));
    }
    Ok(())
}

/// Return whether the caller already holds the exact selected result.
fn project_output_projection_is_current(project_root: &Path, output: &OvenStoredProjectOutput) -> CliResult<bool> {
    let marker_path = project_output_projection_marker_path(project_root, output)?;
    let marker = match fs::read(&marker_path) {
        Ok(bytes) => match serde_json::from_slice::<OvenProjectOutputProjection>(&bytes) {
            Ok(marker) => marker,
            Err(_) => return Ok(false),
        },
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => {
            return Err(CliError::failure(format!(
                "failed to read Oven project-output projection {}: {error}",
                marker_path.display()
            )));
        }
    };
    let expected = project_output_projection(output);
    if marker != expected {
        return Ok(false);
    }
    for file in &expected.files {
        let path = caller_project_output_path(project_root, &file.caller_relative_path)?;
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(error) => {
                return Err(CliError::failure(format!(
                    "failed to inspect Oven project-output projection {}: {error}",
                    path.display()
                )));
            }
        };
        if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.len() != file.logical_bytes {
            return Ok(false);
        }
        let (logical_bytes, digest) = digest_project_output_projection_file(&path)?;
        if logical_bytes != file.logical_bytes || digest != file.digest {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Atomically retain the successful caller projection after its immutable source bytes were verified.
fn write_project_output_projection(project_root: &Path, output: &OvenStoredProjectOutput) -> CliResult<()> {
    let marker_path = project_output_projection_marker_path(project_root, output)?;
    let parent = marker_path.parent().ok_or_else(|| {
        CliError::failure(format!(
            "Oven project-output projection has no parent: {}",
            marker_path.display()
        ))
    })?;
    fs::create_dir_all(parent).map_err(|error| {
        CliError::failure(format!(
            "failed to create Oven project-output projection directory {}: {error}",
            parent.display()
        ))
    })?;
    let bytes = serde_json::to_vec_pretty(&project_output_projection(output))
        .map_err(|error| CliError::failure(format!("failed to encode Oven project-output projection: {error}")))?;
    let staged = parent.join(format!(".oven-project-output-{}.tmp", std::process::id()));
    crate::oven::write_receipt_staged(&bytes, &staged, &marker_path, parent).map_err(|error| {
        CliError::failure(format!(
            "failed to publish Oven project-output projection {}: {error}",
            marker_path.display()
        ))
    })
}

/// Normal-command policy that must be satisfied before a completed project output may be selected.
struct CompletedOutputPolicy<'a> {
    cargo_policy: &'a CargoPolicy,
    package_features: &'a FeatureSelection,
    sdk_profile: Option<&'a str>,
    cargo_features: &'a [String],
    cargo_no_default_features: bool,
    cargo_all_features: bool,
}

impl CompletedOutputPolicy<'_> {
    /// Reject Cargo feature controls before any completed-output lookup can bypass the normal Oven contract.
    fn reject_cargo_feature_controls(&self, command_kind: &str) -> CliResult<()> {
        if self.cargo_no_default_features || self.cargo_all_features || !self.cargo_features.is_empty() {
            return Err(CliError::failure(format!(
                "Oven Alpha normal {command_kind} do not accept Cargo feature controls; use Incan package features instead"
            )));
        }
        Ok(())
    }

    /// Return the normalized Cargo feature evidence used by canonical lock validation.
    fn cargo_feature_selection(&self) -> CargoFeatureSelection {
        CargoFeatureSelection {
            cargo_features: self.cargo_features.to_vec(),
            cargo_no_default_features: self.cargo_no_default_features,
            cargo_all_features: self.cargo_all_features,
        }
        .normalized()
    }
}

/// Select an exact default-profile project output without constructing a compilation session. Explicit package-feature
/// or SDK selections retain the normal source-aware preparation route until their own selection facts are part of the
/// project-output payload schema.
fn select_default_project_output(
    file_path: &str,
    policy: &CompletedOutputPolicy<'_>,
    target: OvenBakeProjectTarget,
    profile: &str,
) -> CliResult<Option<OvenStoredProjectOutput>> {
    policy.reject_cargo_feature_controls("build and run")?;
    if policy.package_features != &FeatureSelection::default() || policy.sdk_profile.is_some() {
        return Ok(None);
    }
    let entrypoint = normalized_project_entrypoint(file_path)?;
    let Some(project_root) = project_root_for_completed_output(&entrypoint)? else {
        return Ok(None);
    };
    let manifest = ProjectManifest::load(&project_root.join(MANIFEST_FILENAME))
        .map_err(|error| CliError::failure(error.to_string()))?;
    validate_completed_output_lock_policy(&project_root, &manifest, &entrypoint, policy)?;
    let store = open_default_oven_store()?;
    let source_authority_digest = digest_baked_project_source_authority(&project_root)?;
    if let Some(selected) = select_baked_project_output_with_source_authority(
        &store,
        &project_root,
        &entrypoint,
        target,
        profile,
        &source_authority_digest,
        None,
    )? {
        return Ok(Some(selected));
    }
    if has_stale_baked_project_output(&store, &project_root, &entrypoint, target, profile)? {
        return Err(CliError::failure(
            "Oven Alpha has no receipt-compatible Loaf for this project's current source authority; its source or lock changed after an explicit bake. Run `incan oven bake --project .` before a normal build or run.",
        ));
    }
    Ok(None)
}

/// Validate strict lock promises before a completed-output fast path can return a stale-output diagnostic.
///
/// `--locked` and `--frozen` are user-visible assertions about canonical `incan.lock`. They remain read-only and
/// Cargo-free here, but must retain their canonical diagnostic precedence even when a project has a previous completed
/// Loaf.
fn validate_completed_output_lock_policy(
    project_root: &Path,
    manifest: &ProjectManifest,
    entrypoint: &Path,
    policy: &CompletedOutputPolicy<'_>,
) -> CliResult<()> {
    if !policy.cargo_policy.locked && !policy.cargo_policy.frozen {
        return Ok(());
    }
    let cargo_features = policy.cargo_feature_selection();
    validate_oven_lock_policy(
        project_root,
        Some(manifest),
        entrypoint,
        &cargo_features,
        policy.cargo_policy,
        policy.package_features,
        policy.sdk_profile,
    )
}

/// Preserve the normal non-strict stale-lock warning when only the lock's derived fingerprint differs.
fn warn_for_completed_output_lock_fingerprint_drift<'a>(
    project_root: &Path,
    outputs: impl IntoIterator<Item = &'a OvenStoredProjectOutput>,
) -> CliResult<()> {
    let mut expected = None;
    for output in outputs {
        let Some(fingerprint) = output.payload.lock_dependencies_fingerprint.as_deref() else {
            continue;
        };
        match expected {
            Some(previous) if previous != fingerprint => {
                return Err(CliError::failure(
                    "completed Oven project outputs disagree on their canonical lock dependency fingerprint",
                ));
            }
            Some(_) => {}
            None => expected = Some(fingerprint),
        }
    }
    let Some(expected) = expected else {
        return Ok(());
    };
    let Some(actual) = baked_project_lock_dependencies_fingerprint(project_root)? else {
        return Ok(());
    };
    if actual == expected {
        return Ok(());
    }
    let workspace = crate::workspace::WorkspaceGraph::discover(project_root)
        .map_err(|error| CliError::failure(format!("failed to resolve Oven project workspace: {error}")))?;
    if workspace.is_some() {
        eprintln!(
            "warning: workspace incan.lock is out of date; continuing without using it as Oven lock authority or rewriting it. Run `incan lock` to refresh it."
        );
    } else {
        eprintln!(
            "warning: incan.lock is out of date; continuing without using it as Oven lock authority or rewriting it. Run `incan lock` to refresh it."
        );
    }
    Ok(())
}

/// Select both profile outputs required by a normal `build --lib` without entering the library frontend. A partial hit
/// is deliberately ignored: the historical command contract publishes debug and release rlibs together.
fn select_default_library_project_outputs(
    file_path: Option<&str>,
    policy: &CompletedOutputPolicy<'_>,
    backend_options: &BackendSelectionOptions,
) -> CliResult<Option<Vec<OvenStoredProjectOutput>>> {
    policy.reject_cargo_feature_controls("library builds")?;
    if policy.package_features != &FeatureSelection::default()
        || policy.sdk_profile.is_some()
        || !backend_options.allows_completed_output_reuse()
    {
        return Ok(None);
    }
    let project_root = resolve_library_project_root(file_path)?;
    let Some(manifest) = discover_effective_project_manifest(&project_root)? else {
        return Ok(None);
    };
    let entrypoint = validate_library_entrypoint(&manifest)?;
    validate_completed_output_lock_policy(&project_root, &manifest, &entrypoint, policy)?;
    let store = open_default_oven_store()?;
    let source_authority_digest = digest_baked_project_source_authority(&project_root)?;
    let rustc = resolve_active_rustc().map_err(|error| CliError::failure(error.to_string()))?;
    let target = rustc_host_target(&rustc).map_err(|error| CliError::failure(error.to_string()))?;
    let toolchain = rustc_identity(&rustc).map_err(|error| CliError::failure(error.to_string()))?;
    let mut outputs = Vec::new();
    for profile in explicit_bake_profiles() {
        let Some(selected) = select_baked_project_output_with_source_authority(
            &store,
            &project_root,
            &entrypoint,
            OvenBakeProjectTarget::Library,
            profile,
            &source_authority_digest,
            Some((&target, &toolchain)),
        )?
        else {
            if has_stale_baked_project_output(
                &store,
                &project_root,
                &entrypoint,
                OvenBakeProjectTarget::Library,
                profile,
            )? {
                return Err(CliError::failure(
                    "Oven Alpha has no receipt-compatible Loaf for this project's current source authority; its source or lock changed after an explicit bake. Run `incan oven bake --project .` before a normal library build.",
                ));
            }
            return Ok(None);
        };
        if completed_output_default_backend_receipt(&selected).is_none() {
            return Ok(None);
        }
        outputs.push(selected);
    }
    Ok(Some(outputs))
}

/// Restore caller-visible generated sources, package handoff records, and native artifacts from a completed immutable
/// result into the caller's project.
///
/// The selected project's dependency closure deliberately remains in the primary Oven store. Re-publishing that already
/// verified closure into `target/lib/oven/loafs` here would make an ordinary hot build synchronously copy and fsync
/// every dependency file. An explicit provider bake retains the portable package-store export; a normal build needs
/// only the completed output itself.
fn materialize_project_output(project_root: &Path, output: &OvenStoredProjectOutput) -> CliResult<()> {
    if project_output_projection_is_current(project_root, output)? {
        return Ok(());
    }
    for file in &output.payload.files {
        let destination = caller_project_output_path(project_root, &file.caller_relative_path)?;
        if let Ok(existing) = fs::read(&destination)
            && u64::try_from(existing.len()).ok() == Some(file.logical_bytes)
            && digest_bytes(&existing) == file.digest
        {
            continue;
        }
        let source = output.artifact_root.join(validated_project_output_relative_path(
            &file.output_relative_path,
            "stored output",
        )?);
        let source_bytes = fs::read(&source).map_err(|error| {
            CliError::failure(format!(
                "selected Oven project-output Loaf is missing {}: {error}",
                source.display()
            ))
        })?;
        if u64::try_from(source_bytes.len()).ok() != Some(file.logical_bytes)
            || digest_bytes(&source_bytes) != file.digest
        {
            return Err(CliError::failure(format!(
                "selected Oven project-output Loaf digest differs at {}",
                source.display()
            )));
        }
        let parent = destination.parent().ok_or_else(|| {
            CliError::failure(format!(
                "Oven project output has no parent path: {}",
                destination.display()
            ))
        })?;
        fs::create_dir_all(parent).map_err(|error| {
            CliError::failure(format!(
                "failed to create Oven output directory {}: {error}",
                parent.display()
            ))
        })?;
        let temporary = parent.join(format!(
            ".{}.oven-project-output-{}.tmp",
            destination
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("output"),
            std::process::id()
        ));
        fs::write(&temporary, &source_bytes).map_err(|error| {
            CliError::failure(format!(
                "failed to materialize Oven project output {} -> {}: {error}",
                source.display(),
                temporary.display()
            ))
        })?;
        let source_permissions = fs::metadata(&source)
            .map_err(|error| {
                CliError::failure(format!(
                    "failed to read Oven project output permissions {}: {error}",
                    source.display()
                ))
            })?
            .permissions();
        // Store materializations are immutable by design. The caller-owned projection must remain writable so a later
        // source miss can generate a replacement rather than failing against a read-only cache copy.
        #[cfg(unix)]
        let caller_permissions = fs::Permissions::from_mode(if source_permissions.mode() & 0o111 == 0 {
            0o644
        } else {
            0o755
        });
        #[cfg(not(unix))]
        let caller_permissions = {
            let mut permissions = source_permissions;
            permissions.set_readonly(false);
            permissions
        };
        fs::set_permissions(&temporary, caller_permissions).map_err(|error| {
            CliError::failure(format!(
                "failed to preserve Oven project output permissions {}: {error}",
                temporary.display()
            ))
        })?;
        fs::rename(&temporary, &destination).map_err(|error| {
            CliError::failure(format!(
                "failed to atomically publish Oven project output {}: {error}",
                destination.display()
            ))
        })?;
    }
    write_project_output_projection(project_root, output)?;
    Ok(())
}

/// Compile a receipt-authorized generated executable through the selected direct-rustc Oven plan.
fn bake_oven_project(
    prepared: &OvenPreparedProject,
    profile: &str,
    authority_context: Option<&mut OvenProjectBakeAuthorityContext>,
) -> CliResult<crate::oven::rustc::OvenDirectRustcBake> {
    let mut caller_owned_libraries = prepared.caller_owned_libraries.clone();
    let mut re_materialized_package_library_names = BTreeSet::new();
    let mut registry_authority = registry_leaf_authority_for_plan_selection(&prepared.plan_selection)?;
    let mut extra_dependency_search_paths = Vec::new();
    if has_caller_owned_project_libraries(&prepared.provider_plan) {
        let closure = collect_caller_owned_provider_registry_leaf_authority(
            &open_default_oven_store()?,
            &prepared.provider_plan,
            profile,
        )?;
        // The conflict decision must cover every selection path -- including an imported packaged-provider closure,
        // whose composed link carries the SDK base's and the provider's own copies of any shared package exactly
        // like a re-materialized one does.
        if let Some(package) = caller_owned_provider_registry_conflict(
            registry_authority.as_ref(),
            &closure,
            prepared.plan_selection.artifact_plan(),
        )? {
            return cargo_fallback_bake_oven_project(prepared, profile, &package);
        }
        if !prepared.plan_selection.uses_packaged_provider_closure() {
            extra_dependency_search_paths = closure.dependency_search_paths.clone();
            registry_authority = closure.merged_authority(registry_authority);
            let re_materialized = rematerialize_caller_owned_libraries_with_authority_context(
                &prepared.provider_plan,
                profile,
                prepared.plan_selection.artifacts(),
                prepared.plan_selection.output_guard_root(),
                prepared.plan_selection.artifact_plan(),
                &prepared.rustc,
                prepared.generator.output_dir(),
                registry_authority.as_ref(),
                &extra_dependency_search_paths,
                authority_context,
            )?;
            re_materialized_package_library_names.extend(
                re_materialized
                    .iter()
                    .filter(|library| library.expose_extern)
                    .map(|library| library.crate_name.clone()),
            );
            replace_caller_owned_package_libraries(&mut caller_owned_libraries, re_materialized)?;
        }
    }
    let mut artifact_plan = prepared
        .plan_selection
        .source_artifact_plan("generated-root")
        .map_err(oven_rustc_error)?;
    if !re_materialized_package_library_names.is_empty() {
        replace_selected_package_library_externs(&mut artifact_plan, &re_materialized_package_library_names);
    }
    artifact_plan.compile_environment =
        direct_rustc_compile_environment(prepared.generator.output_dir(), &prepared.generator.crate_root_path())
            .map_err(|error| CliError::failure(error.to_string()))?;
    attach_caller_owned_rustc_libraries(&mut artifact_plan, &caller_owned_libraries).map_err(oven_rustc_error)?;
    // Loading a re-materialized caller-owned library's own metadata (for example a query-engine provider linked
    // above) can require Rustc to locate that library's own further dependencies purely through
    // `-L dependency=...` search, the same way `rematerialize_caller_owned_provider_graph` already extends that
    // library's own compile with this same closure. The final consumer binary link needs it too.
    for directory in &extra_dependency_search_paths {
        if !artifact_plan.dependency_search_paths.contains(directory) {
            artifact_plan.dependency_search_paths.push(directory.clone());
        }
    }
    bake_trusted_direct_rustc_run(&OvenTrustedDirectRustcTargetRequest {
        receipt: &prepared.receipt,
        artifacts: prepared.plan_selection.artifacts(),
        artifact_root: prepared.plan_selection.output_guard_root(),
        artifact_plan: Some(&artifact_plan),
        rustc: &prepared.rustc,
        source: &prepared.generator.crate_root_path(),
        output: &oven_binary_path(prepared, profile),
        crate_name: &prepared.crate_name,
        edition: &prepared.rust_edition,
        source_evidence_key: "generated-root",
        features: &prepared.receipt.intent.features,
        prefer_dynamic: false,
    })
    .map_err(oven_rustc_error)
}

/// Return the caller-owned direct-rustc library artifact path.
///
/// It intentionally lives beside the generated inspection projection rather than in a Cargo target directory. The
/// `.incnlib` and generated `Cargo.toml` remain useful publication/inspection artifacts, but neither authorizes this
/// executable path.
fn oven_library_path(prepared: &PreparedLibraryProject, oven: &OvenPreparedLibrary, profile: &str) -> PathBuf {
    prepared
        .out_dir
        .join("oven")
        .join(profile)
        .join(format!("lib{}.rlib", oven.crate_name))
}

/// Compile a receipt-authorized generated library through the selected direct-rustc Oven plan.
fn bake_oven_library(
    prepared: &PreparedLibraryProject,
    oven: &OvenPreparedLibrary,
    profile: &str,
    authority_context: Option<&mut OvenProjectBakeAuthorityContext>,
) -> CliResult<crate::oven::rustc::OvenDirectRustcBake> {
    let selected = oven.profiles.get(profile).ok_or_else(|| {
        CliError::failure(format!(
            "normal Oven library build has no prepared `{profile}` direct-rustc selection"
        ))
    })?;
    let mut caller_owned_libraries = selected.caller_owned_libraries.clone();
    let mut re_materialized_package_library_names = BTreeSet::new();
    let mut registry_authority = registry_leaf_authority_for_plan_selection(&selected.plan_selection)?;
    let mut extra_dependency_search_paths = Vec::new();
    if has_caller_owned_project_libraries(&selected.provider_plan) {
        let closure = collect_caller_owned_provider_registry_leaf_authority(
            &open_default_oven_store()?,
            &selected.provider_plan,
            profile,
        )?;
        // Library outputs have no unified-Cargo fallback, so a conflicted provider closure fails closed on every
        // selection path, packaged-provider composition included.
        reject_caller_owned_provider_registry_conflict(
            registry_authority.as_ref(),
            &closure,
            selected.plan_selection.artifact_plan(),
        )?;
        if !selected.plan_selection.uses_packaged_provider_closure() {
            extra_dependency_search_paths = closure.dependency_search_paths.clone();
            registry_authority = closure.merged_authority(registry_authority);
            let re_materialized = rematerialize_caller_owned_libraries_with_authority_context(
                &selected.provider_plan,
                profile,
                selected.plan_selection.artifacts(),
                selected.plan_selection.output_guard_root(),
                selected.plan_selection.artifact_plan(),
                &oven.rustc,
                &prepared.out_dir,
                registry_authority.as_ref(),
                &extra_dependency_search_paths,
                authority_context,
            )?;
            re_materialized_package_library_names.extend(
                re_materialized
                    .iter()
                    .filter(|library| library.expose_extern)
                    .map(|library| library.crate_name.clone()),
            );
            replace_caller_owned_package_libraries(&mut caller_owned_libraries, re_materialized)?;
        }
    }
    let mut artifact_plan = selected
        .plan_selection
        .source_artifact_plan("generated-root")
        .map_err(oven_rustc_error)?;
    if !re_materialized_package_library_names.is_empty() {
        replace_selected_package_library_externs(&mut artifact_plan, &re_materialized_package_library_names);
    }
    artifact_plan.compile_environment =
        direct_rustc_compile_environment(prepared.generator.output_dir(), &prepared.generator.crate_root_path())
            .map_err(|error| CliError::failure(error.to_string()))?;
    attach_caller_owned_rustc_libraries(&mut artifact_plan, &caller_owned_libraries).map_err(oven_rustc_error)?;
    // See the matching comment in `bake_oven_project`: a re-materialized caller-owned library's own metadata can
    // require this same dependency search closure to load, not only the library's own re-materialization compile.
    for directory in &extra_dependency_search_paths {
        if !artifact_plan.dependency_search_paths.contains(directory) {
            artifact_plan.dependency_search_paths.push(directory.clone());
        }
    }
    let direct = bake_trusted_direct_rustc_library(&OvenTrustedDirectRustcTargetRequest {
        receipt: &selected.receipt,
        artifacts: selected.plan_selection.artifacts(),
        artifact_root: selected.plan_selection.output_guard_root(),
        artifact_plan: Some(&artifact_plan),
        rustc: &oven.rustc,
        source: &prepared.generator.crate_root_path(),
        output: &oven_library_path(prepared, oven, profile),
        crate_name: &oven.crate_name,
        edition: &oven.rust_edition,
        source_evidence_key: "generated-root",
        features: &selected.receipt.intent.features,
        prefer_dynamic: false,
    });

    match direct {
        Ok(bake) => Ok(bake),
        // A crate-loading failure is a composition fault, not a fault in the generated Rust: the sources already
        // typechecked, so rustc rejecting a dependency means the assembled closure is not mutually loadable. That
        // is the same shape an executable resolves by rebuilding through one unified Cargo resolution, and it is
        // what a library needs here too, rather than surfacing raw `E0463`s about crates the user never named.
        Err(error) if direct_rustc_composition_failure(&error) => {
            cargo_fallback_bake_oven_library(prepared, oven, profile)
        }
        Err(error) => Err(oven_rustc_error(error)),
    }
}

/// Recognize a rustc failure caused by an unloadable dependency closure rather than by the compiled source.
///
/// Only crate-loading diagnostics qualify. `E0463` is a crate that could not be found at all, `E0460`/`E0461`/`E0464`
/// are candidates that were found but rejected for identity, target or ambiguity reasons. A type error in generated
/// Rust is never one of these, so this cannot swallow a genuine compilation failure and silently retry it.
fn direct_rustc_composition_failure(error: &OvenRustcError) -> bool {
    let OvenRustcError::CompilationFailed { report } = error else {
        return false;
    };
    const CRATE_LOADING_CODES: [&str; 4] = ["E0460", "E0461", "E0463", "E0464"];
    if report.diagnostics.iter().any(|diagnostic| {
        diagnostic
            .code
            .as_deref()
            .is_some_and(|code| CRATE_LOADING_CODES.contains(&code))
    }) {
        return true;
    }
    // Newer rustc JSON records carry a `$message_type` tag the structured decoder does not recognize, so the
    // whole transcript can arrive as unstructured text with `diagnostics` empty. The codes are still verbatim in
    // it, and missing this case is what makes the failure surface as raw rustc noise instead of a rebuild.
    CRATE_LOADING_CODES
        .iter()
        .any(|code| report.unstructured_output.contains(code))
}

/// Rebuild a generated library through one unified Cargo resolution when direct-rustc composition cannot load.
///
/// The generated project on disk already carries the complete Cargo manifest, so Cargo resolves every dependency
/// once and produces an internally consistent closure. Only libraries that actually hit the fault pay this cost;
/// the produced `rlib` is published to the same path the direct-rustc bake would have written.
fn cargo_fallback_bake_oven_library(
    prepared: &PreparedLibraryProject,
    oven: &OvenPreparedLibrary,
    profile: &str,
) -> CliResult<crate::oven::rustc::OvenDirectRustcBake> {
    eprintln!(
        "Oven: building library `{}` through unified Cargo resolution: its dependency closure is not loadable as \
         independently compiled parts.",
        oven.crate_name
    );
    let release = profile == "release";
    let result = prepared.generator.cargo_build(release).map_err(|error| {
        CliError::failure(format!(
            "unified Cargo fallback build failed to start for library `{}`: {error}",
            oven.crate_name
        ))
    })?;
    if !result.success {
        return Err(CliError::failure(format!(
            "unified Cargo fallback build failed for library `{}`:\n{}",
            oven.crate_name, result.stderr
        )));
    }
    let built = prepared.generator.cargo_build_library_path(release);
    let bytes = fs::read(&built).map_err(|error| {
        CliError::failure(format!(
            "unified Cargo fallback build reported success but its library is unreadable at {}: {error}",
            built.display()
        ))
    })?;
    let output = oven_library_path(prepared, oven, profile);
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            CliError::failure(format!(
                "could not create the Oven library output directory {}: {error}",
                parent.display()
            ))
        })?;
    }
    fs::write(&output, &bytes).map_err(|error| {
        CliError::failure(format!(
            "could not publish the unified Cargo library to {}: {error}",
            output.display()
        ))
    })?;
    let selected = oven.profiles.get(profile).ok_or_else(|| {
        CliError::failure(format!(
            "unified Cargo library fallback has no prepared `{profile}` selection to record provenance against"
        ))
    })?;
    Ok(crate::oven::rustc::OvenDirectRustcBake::from_external_cargo_build(
        selected.receipt.identity.clone(),
        output,
        crate::oven::digest_bytes(&bytes),
    ))
}

/// Preserve direct-rustc diagnostics rather than reducing a normal Oven compilation failure to a generic status.
fn oven_rustc_error(error: OvenRustcError) -> CliError {
    match error {
        OvenRustcError::CompilationFailed { report } => {
            let rendered = report
                .diagnostics
                .into_iter()
                .map(|diagnostic| {
                    diagnostic
                        .rendered
                        .unwrap_or_else(|| format!("{}: {}", diagnostic.level, diagnostic.message))
                })
                .collect::<Vec<_>>()
                .join("\n");
            let mut output = format!("{rendered}\n{}", report.unstructured_output).trim().to_string();
            if let Some(invocation) = report.invocation {
                if !output.is_empty() {
                    output.push('\n');
                }
                output.push_str("direct rustc invocation: ");
                output.push_str(&invocation);
            }
            CliError::failure(if output.is_empty() {
                "Oven direct-rustc compilation failed without a diagnostic transcript".to_string()
            } else {
                format!("Oven direct-rustc compilation failed:\n{output}")
            })
        }
        error => CliError::failure(error.to_string()),
    }
}

/// Select the default sealed release output without entering frontend or report reconstruction.
fn select_default_executable_project_output(
    file_path: &str,
    output_dir: Option<&String>,
    options: &BuildCommandOptions,
) -> CliResult<Option<(PathBuf, OvenStoredProjectOutput, BackendExecutionReceipt)>> {
    if output_dir.is_some() || !options.backend.allows_completed_output_reuse() {
        return Ok(None);
    }
    let completed_output_policy = CompletedOutputPolicy {
        cargo_policy: &options.cargo_policy,
        package_features: &options.package_features,
        sdk_profile: options.sdk_profile.as_deref(),
        cargo_features: &options.cargo_features,
        cargo_no_default_features: options.cargo_no_default_features,
        cargo_all_features: options.cargo_all_features,
    };
    let Some(selected) = select_default_project_output(
        file_path,
        &completed_output_policy,
        OvenBakeProjectTarget::Executable,
        "release",
    )?
    else {
        return Ok(None);
    };
    let project_root = project_root_for_completed_output(&normalized_project_entrypoint(file_path)?)?
        .ok_or_else(|| CliError::failure("selected Oven project-output Loaf has no manifest-backed project root"))?;
    let Some(backend_receipt) = completed_output_default_backend_receipt(&selected) else {
        return Ok(None);
    };
    warn_for_completed_output_lock_fingerprint_drift(&project_root, [&selected])?;
    Ok(Some((project_root, selected, backend_receipt)))
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
    let report = report_draft.finish(BTreeMap::from([
        ("prepare".to_string(), prepare_ms),
        ("oven_build".to_string(), oven_build_ms),
        ("total".to_string(), elapsed_ms(total_start)),
    ]));
    serde_json::to_value(report)
        .map_err(|error| CliError::failure(format!("failed to serialize Oven build report: {error}")))
}

/// Return whether an internal library artifact build must avoid canonical workspace lock resolution.
///
/// Ordinary dependency artifacts are prepared before their parent can finish the canonical workspace lock, so they
/// must retain producer-local resolution. SDK artifacts are different: their publisher supplies an exact lock
/// override that remains part of preparation.
fn dependency_artifact_skips_canonical_lock(artifact_only: bool, sdk_provider_build: bool) -> bool {
    artifact_only && !sdk_provider_build
}

/// Return whether this library compilation needs a rust-inspect workspace to preserve Rust-call signatures.
///
/// An ordinary library build retains its complete ABI inspection contract. An artifact-only SDK provider may skip an
/// empty inspection workspace, but it must prepare one when its source imports Rust items: those signatures can carry
/// ownership facts such as `&impl AsFd` that code generation must preserve. This remains inside the explicit provider
/// publisher; normal Oven consumers never invoke this preparation path.
#[cfg(feature = "rust_inspect")]
fn library_rust_inspection_required(artifact_only: bool, metadata_query_paths: &[String]) -> bool {
    !artifact_only || !metadata_query_paths.is_empty()
}

/// Remove path dependencies that point back to the selected project's generated library crate.
///
/// A rooted workspace lock includes the root library as a dependency of its consumers. That aggregate dependency is
/// valid for the synthetic lock/preheat package, but the selected root library artifact cannot depend on its own
/// canonical `target/lib` crate after adopting the producer's Cargo package identity. This comparison deliberately
/// uses the project-owned artifact path rather than a command-specific output override.
fn remove_generated_library_self_dependencies(resolved: &mut ResolvedDependencies, project_root: &Path) {
    let artifact_root = project_root.join("target/lib");
    let canonical_artifact_root = fs::canonicalize(&artifact_root).unwrap_or(artifact_root);
    let points_to_generated_crate = |spec: &DependencySpec| match &spec.source {
        DependencySource::Path { path } => {
            fs::canonicalize(path).unwrap_or_else(|_| path.clone()) == canonical_artifact_root
        }
        DependencySource::Registry | DependencySource::Git { .. } => false,
    };
    resolved.dependencies.retain(|spec| !points_to_generated_crate(spec));
    resolved
        .dev_dependencies
        .retain(|spec| !points_to_generated_crate(spec));
}

/// Validate a library project and generate its Rust project without running Cargo.
///
/// Normal consumers include their already selected interop execution receipt in the runtime identity. Explicit Oven
/// preparation and Rust inspection deliberately omit it, so a package can first produce the base receipt required by
/// `incan oven interop bake`; neither path selects a native tool, discovers a system library, or weakens the normal
/// execution requirement.
#[allow(clippy::too_many_arguments)] // Library preparation receives the same independent CLI selection axes.
fn prepare_library_project(
    file_path: Option<&str>,
    output_dir: Option<&str>,
    cargo_policy: CargoPolicy,
    package_features: &FeatureSelection,
    sdk_profile_override: Option<&str>,
    cargo_features: Vec<String>,
    cargo_no_default_features: bool,
    cargo_all_features: bool,
    generated_cargo_target_dir: Option<&Path>,
    normal_oven: bool,
    include_interop_execution: bool,
    oven_plan_mode: OvenProjectPlanMode,
    authority_context: Option<&mut OvenProjectBakeAuthorityContext>,
    backend_options: &BackendSelectionOptions,
) -> CliResult<PreparedLibraryProject> {
    let prepare_start = Instant::now();
    let mut timings_ms = BTreeMap::new();
    let source_load_start = Instant::now();
    let project_root = resolve_library_project_root(file_path)?;
    let out_dir = match output_dir {
        Some(output_dir) => {
            validate_output_dir(output_dir)?;
            let output_dir = PathBuf::from(output_dir);
            if output_dir.is_absolute() {
                output_dir
            } else {
                project_root.join(output_dir)
            }
        }
        None => project_root.join("target").join("lib"),
    };
    let Some(manifest) = discover_effective_project_manifest(&project_root)? else {
        return Err(CliError::failure(
            "No incan.toml found for `incan build --lib` (run `incan init` first)",
        ));
    };
    enforce_project_toolchain_constraint(&manifest)?;
    let project_version = manifest
        .project
        .as_ref()
        .and_then(|project| project.version.clone())
        .unwrap_or_else(|| "0.1.0".to_string());

    let lib_entry = validate_library_entrypoint(&manifest)?;
    let compilation_session = if normal_oven {
        CompilationSession::discover_for_oven(&lib_entry, package_features, sdk_profile_override)?
    } else {
        super::common::CompilationSession::discover_with_selections(&lib_entry, package_features, sdk_profile_override)?
    };
    let modules = super::common::collect_library_modules_detailed_with_session(lib_entry.clone(), &compilation_session)
        .map_err(|failure| CliError::failure(failure.render_human()))?;
    let provider_metadata_modules = collect_unprojected_provider_modules(&lib_entry, &compilation_session)?;

    let Some(lib_module) = modules.last() else {
        return Err(CliError::failure("No modules found for library build"));
    };
    if lib_module.file_path != lib_entry {
        return Err(CliError::failure(format!(
            "Library entrypoint mismatch: expected `{}`, got `{}`",
            lib_entry.display(),
            lib_module.file_path.display()
        )));
    }
    record_timing(&mut timings_ms, "library_load_sources", source_load_start);

    let requirements_start = Instant::now();
    let declared = manifest.declared_rust_crate_names();
    let package_feature_plan = compilation_session
        .package_feature_plan
        .clone()
        .ok_or_else(|| CliError::failure("library compilation session is missing its package feature graph"))?;
    let library_manifest_index = compilation_session.library_manifest_index.clone();
    let mut project_requirements = collect_project_requirements(&modules, &library_manifest_index)?;
    let provider_plan = compilation_session.provider_plan_for_modules(&modules)?;
    let compiled_sdk_modules = CompiledSdkModules::from_provider_plan(&provider_plan);
    extend_requirements_with_provider_plan(&mut project_requirements, &provider_plan)?;
    let semantic_sdk_paths = semantic_sdk_path_dependencies(&project_requirements);
    let semantic = semantic_lock_state(
        &project_root,
        manifest.oven_interop(),
        compilation_session.sdk_inventory.as_deref(),
        compilation_session.sdk_components.as_ref(),
        Some(&package_feature_plan),
        &provider_plan,
        &semantic_sdk_paths,
    )
    .map_err(CliError::failure)?;
    let contract_model_bundles = read_project_model_bundles(&project_root, &manifest.contract_model_bundle_paths())
        .map_err(|error| CliError::failure(error.to_string()))?;
    let rust_extern_contexts = collect_rust_extern_contexts(&modules);
    let dep_modules = &modules[..modules.len() - 1];
    // Library consumers use the same artifact metadata and linked Rust crate as executable and test-batch consumers;
    // migrated modules must not be generated into a second local `__incan_std` tree.
    let emitted_dep_modules: Vec<&ParsedModule> = dep_modules
        .iter()
        .filter(|module| !compiled_sdk_modules.contains_emission_path(&module.path_segments))
        .collect();

    let mut inline_imports = collect_rust_dependency_uses(lib_module, false);
    for module in &emitted_dep_modules {
        inline_imports.extend(collect_rust_dependency_uses(module, false));
    }
    // Compiler-owned `incan_stdlib` and Rust's sysroot are supplied by the selected Oven plan. The remaining
    // caller-authored imports are resolved after code generation and compiled through the same direct-Rustc closure
    // materializer used by normal executables and test batches; this library route must not regain a Cargo fallback.
    let source_inline_crates = inline_imports
        .iter()
        .filter(|import| import.crate_name != "incan_stdlib" && import.crate_name != "std")
        .map(|import| import.crate_name.clone())
        .collect::<BTreeSet<_>>();
    let project_name = manifest
        .project
        .as_ref()
        .and_then(|project| project.name.clone())
        .or_else(|| {
            manifest
                .project_root()
                .file_name()
                .and_then(|name| name.to_str())
                .map(str::to_owned)
        })
        .unwrap_or_else(|| "incan_library".to_string());

    let cargo_features = CargoFeatureSelection {
        cargo_features: cargo_features.clone(),
        cargo_no_default_features,
        cargo_all_features,
    }
    .normalized();
    record_timing(&mut timings_ms, "library_collect_requirements", requirements_start);

    let dependency_start = Instant::now();
    // A library projection must consume the same source-reachable dependency graph that owns the canonical project
    // lock. Including every declared-but-unused Rust dependency here can make the caller graph strictly larger than
    // the canonical lock generated from scripts, tests, and this library entry, causing a valid existing lock to be
    // rejected during rust-inspect projection.
    let mut resolved = match resolve_reachable_dependencies(Some(&manifest), &inline_imports, true, &cargo_features) {
        Ok(resolved) => resolved,
        Err(errors) => {
            let mut msg = String::new();
            let sources = build_source_map(&modules);
            for err in errors {
                msg.push_str(&format_dependency_error(&err, &sources));
            }
            return Err(CliError::failure(msg.trim_end()));
        }
    };
    merge_project_requirement_dependencies(&mut resolved, &project_requirements)?;
    record_timing(&mut timings_ms, "library_resolve_dependencies", dependency_start);
    #[cfg(feature = "rust_inspect")]
    let metadata_query_paths = collect_library_rust_abi_query_paths(&modules, &rust_extern_contexts);
    #[cfg(not(feature = "rust_inspect"))]
    let metadata_query_paths: Vec<String> = Vec::new();

    let lock_start = Instant::now();
    let artifact_only = env::var_os(INTERNAL_LIBRARY_ARTIFACT_ONLY_ENV).is_some();
    if normal_oven {
        if cargo_no_default_features || cargo_all_features || !cargo_features.cargo_features.is_empty() {
            return Err(CliError::failure(
                "Oven Alpha normal library builds do not accept Cargo feature controls; use Incan package features instead",
            ));
        }
        validate_oven_lock_policy(
            &project_root,
            Some(&manifest),
            &lib_entry,
            &cargo_features,
            &cargo_policy,
            package_features,
            sdk_profile_override,
        )?;
    }
    let (
        lock_payload_for_typecheck,
        cargo_lock_projection_root,
        clear_cargo_lock,
        cargo_flags,
        lock_cargo_package_name,
        managed_target_path,
        managed_target_lease,
        managed_target_identity,
    ) = if normal_oven {
        // A generated Cargo project is retained only as an inspectable source projection. Normal library execution
        // must neither acquire a generated Cargo cache nor derive an authority-bearing Cargo.lock from it.
        (
            None,
            None,
            false,
            Vec::new(),
            project_name.clone(),
            out_dir.join(".cargo-projection"),
            None,
            None,
        )
    } else {
        let dependency_artifact_only =
            dependency_artifact_skips_canonical_lock(artifact_only, env::var_os(SDK_PROVIDER_BUILD_ENV).is_some());
        let lock_resolution = if dependency_artifact_only {
            // Dependency artifact preparation has no Cargo build to constrain with a lock payload. Resolving the
            // canonical workspace lock here would traverse the consumer that requested this still-missing root artifact
            // and recursively launch the same artifact-only child. Keep the already-resolved producer context intact;
            // the parent command remains the sole owner of canonical lock generation and publication. SDK provider
            // artifact builds are excluded because their parent supplies an exact Cargo.lock payload override.
            LockResolution {
                cargo_lock_authority: super::lock::CargoLockAuthority::None,
                cargo_package_name: project_name.clone(),
                resolved,
                project_requirements,
            }
        } else {
            resolve_lock_context(LockResolutionRequest {
                project_root: &project_root,
                project_name: project_name.as_str(),
                entry_file: Some(&lib_entry),
                manifest: Some(&manifest),
                resolved: &resolved,
                project_requirements: &project_requirements,
                cargo_features: &cargo_features,
                cargo_policy: &cargo_policy,
                semantic: Some(&semantic),
                package_features: Some(package_features),
                sdk_profile_override,
            })?
        };
        let cargo_lock_inputs = lock_resolution.cargo_lock_authority.into_generator_inputs();
        resolved = lock_resolution.resolved;
        project_requirements = lock_resolution.project_requirements;
        let managed_target = resolve_generated_cargo_target(
            generated_cargo_target_dir,
            &project_root,
            &out_dir,
            &lock_resolution.cargo_package_name,
            "release",
            cargo_lock_inputs.payload.as_deref(),
            &cargo_features,
            &cargo_command_flags(&cargo_policy, &cargo_features),
        )
        .map_err(|error| CliError::failure(format!("failed to prepare generated Cargo cache: {error}")))?;
        let (managed_target_path, managed_target_lease, managed_target_identity) = managed_target.into_parts();
        (
            cargo_lock_inputs.payload,
            cargo_lock_inputs.projection_root,
            cargo_lock_inputs.clear_existing,
            cargo_command_flags(&cargo_policy, &cargo_features),
            lock_resolution.cargo_package_name,
            managed_target_path,
            managed_target_lease,
            managed_target_identity,
        )
    };
    record_timing(&mut timings_ms, "library_resolve_lock_payload", lock_start);
    let mut oven_build_inputs = normal_oven
        .then(|| oven_build_unit_inputs(&provider_plan, &project_requirements, &resolved))
        .transpose()?;
    let source_compiler_vocab_support = normal_oven
        && manifest.vocab().is_some()
        && crate::oven::legacy_cargo::source_compiler_vocab_support_is_available();
    if source_compiler_vocab_support && let Some(build_inputs) = oven_build_inputs.as_mut() {
        // A source-built compiler seals this helper at the explicit publisher boundary. Keep that closure in a
        // distinct build unit so a v0.5.0 plan without it can neither shadow nor become ambiguous with the
        // upgraded receipt. Packaged compilers continue to select their release-cohort helper unchanged.
        build_inputs.insert(
            OVEN_SOURCE_COMPILER_VOCAB_SUPPORT_BUILD_INPUT.to_string(),
            "v1".to_string(),
        );
    }
    let oven_rustc = normal_oven
        .then(resolve_active_rustc)
        .transpose()
        .map_err(|error| CliError::failure(error.to_string()))?;
    let oven_target = oven_rustc
        .as_ref()
        .map(|rustc| rustc_host_target(rustc))
        .transpose()
        .map_err(|error| CliError::failure(error.to_string()))?;
    let oven_toolchain = oven_rustc
        .as_ref()
        .map(|rustc| rustc_identity(rustc))
        .transpose()
        .map_err(|error| CliError::failure(error.to_string()))?;
    let oven_store = normal_oven.then(open_default_oven_store).transpose()?;
    if include_interop_execution {
        let (build_inputs, target) = match (oven_build_inputs.as_mut(), oven_target.as_deref()) {
            (Some(build_inputs), Some(target)) => (build_inputs, target),
            _ => {
                return Err(CliError::failure(
                    "interop execution can only be included in an Oven library preparation".to_string(),
                ));
            }
        };
        append_oven_interop_execution_build_inputs(build_inputs, Some(&manifest), target)?;
    }
    let empty_oven_build_inputs = BTreeMap::new();
    #[cfg(feature = "rust_inspect")]
    let rust_inspect_manifest_dir = if normal_oven {
        !metadata_query_paths.is_empty()
    } else {
        library_rust_inspection_required(artifact_only, &metadata_query_paths)
    }
    .then(|| {
        if normal_oven {
            Ok((
                // Rust inspection is compiler-owned preparation state, not part of the generated provider artifact.
                // Keeping its Cargo target below `target/lib` leaks build-script outputs (including valid symlinks)
                // into the provider integrity boundary and needlessly makes every consumer traverse that cache.
                crate::lockfile::compiler_lock_state_dir(&project_root).join("rust_inspect_target"),
                None::<crate::generated_cache::GeneratedCacheLease>,
            ))
        } else {
            resolve_generated_cargo_target(
                generated_cargo_target_dir,
                &project_root,
                &project_root,
                &lock_cargo_package_name,
                "rust-inspect",
                lock_payload_for_typecheck.as_deref(),
                &cargo_features,
                &cargo_flags,
            )
            .map(|target| {
                let (path, lease, _identity) = target.into_parts();
                (path, lease)
            })
            .map_err(|error| CliError::failure(format!("failed to prepare rust-inspect Cargo cache: {error}")))
        }
    })
    .transpose()?
    .map(|(rust_inspect_target_path, _rust_inspect_cache_lease)| {
        let rust_inspect_start = Instant::now();
        let rust_inspect_manifest_dir = prepare_rust_inspect_workspace(RustInspectWorkspaceRequest {
            project_root: &project_root,
            project_name: project_name.as_str(),
            cargo_package_name: &lock_cargo_package_name,
            rust_edition: manifest.build.as_ref().and_then(|build| build.rust_edition.clone()),
            resolved: &resolved,
            project_requirements: &project_requirements,
            lock_payload: lock_payload_for_typecheck.clone(),
            cargo_lock_projection_root: cargo_lock_projection_root.as_deref(),
            clear_cargo_lock,
            cargo_policy_flags: cargo_flags.clone(),
            cargo_target_dir: &rust_inspect_target_path,
            rust_inspect_query_paths: &metadata_query_paths,
            rust_derive_probe_paths: &collect_rust_inspect_derive_probe_paths(&modules),
            prepare_when_empty: true,
            direct_oven_inspection: normal_oven,
            force_direct_prewarm: false,
            oven_source_authority: normal_oven.then(|| OvenRustInspectSourceAuthorityRequest {
                project_version: &project_version,
                target: oven_target.as_deref().unwrap_or_default(),
                toolchain: oven_toolchain.as_deref().unwrap_or_default(),
                profile: "debug",
                features: &cargo_features.cargo_features,
                build_unit_inputs: oven_build_inputs.as_ref().unwrap_or(&empty_oven_build_inputs),
                registry_dependencies: &resolved.dependencies,
            }),
            prepared_project_source_authorities: None,
            explicit_oven_bake: normal_oven && oven_plan_mode == OvenProjectPlanMode::ExplicitBake,
        })?
        .ok_or_else(|| CliError::failure("rust-inspect workspace preparation did not return a manifest directory"))?;
        record_timing(&mut timings_ms, "library_rust_inspect_prewarm", rust_inspect_start);
        Ok(rust_inspect_manifest_dir)
    })
    .transpose()?;

    let typecheck_start = Instant::now();
    let mut all_errors = String::new();
    let mut checked_exports_by_module: HashMap<String, HashMap<String, Vec<CheckedNamedExport>>> = HashMap::new();
    let mut checked_exports_by_source_module: Vec<(Vec<String>, Vec<CheckedNamedExport>)> = Vec::new();
    let mut api_metadata_modules = Vec::new();
    let module_idx_by_key = module_key_index(&modules);
    let mut stdlib_cache = StdlibAstCache::new();
    let mut checked_type_info_by_path = BTreeMap::new();

    for (idx, module) in modules.iter().enumerate() {
        let deps_for_module =
            imported_module_deps_for_with_provider_plan(&modules, idx, &module_idx_by_key, &provider_plan);
        let mut checker = typechecker::TypeChecker::new();
        checker.stdlib_cache = stdlib_cache.clone();
        checker.set_current_package_identity(crate::frontend::module::declaration_package_identity(
            Some(&project_name),
            Some(&module.path_segments),
        ));
        checker.set_current_module_path(Some(module.path_segments.clone()));
        register_module_path_segments(&mut checker, &modules);
        checker.set_declared_crate_names(declared.clone());
        checker.set_provider_plan(Arc::clone(&provider_plan));
        #[cfg(feature = "rust_inspect")]
        if let Some(rust_inspect_manifest_dir) = rust_inspect_manifest_dir.as_ref() {
            checker.set_rust_inspect_manifest_dir(rust_inspect_manifest_dir.manifest_dir().to_path_buf());
        }

        // A provider producer checks its complete source package before publishing the public checked facade.
        let check_result = if provider_plan.bootstrap_sdk_namespace_roots().next().is_some() {
            checker.check_with_imports_allow_private(&module.ast, &deps_for_module)
        } else {
            checker.check_with_imports(&module.ast, &deps_for_module)
        };
        match check_result {
            Ok(()) => {
                render_module_warnings(
                    module.file_path.to_string_lossy().as_ref(),
                    &module.source,
                    checker.warnings(),
                );
                let module_exports = collect_checked_public_exports(&module.ast, &checker);
                api_metadata_modules.push(collect_checked_api_metadata(
                    &module.ast,
                    &checker,
                    module.path_segments.clone(),
                ));
                checked_exports_by_source_module.push((module.path_segments.clone(), module_exports.clone()));
                checked_exports_by_module.insert(
                    module_key(&module.path_segments),
                    checked_exports_by_name(module_exports),
                );
                checked_type_info_by_path.insert(module.file_path.clone(), checker.type_info().clone());
                stdlib_cache = checker.stdlib_cache.clone();
            }
            Err(errs) => {
                stdlib_cache = checker.stdlib_cache.clone();
                for err in &errs {
                    all_errors.push_str(&diagnostics::format_error(
                        module.file_path.to_string_lossy().as_ref(),
                        &module.source,
                        err,
                    ));
                }
            }
        }
    }

    if !all_errors.is_empty() {
        return Err(CliError::failure(all_errors.trim_end()));
    }
    #[cfg(feature = "rust_inspect")]
    if let Some(rust_inspect_manifest_dir) = rust_inspect_manifest_dir.as_ref() {
        RustMetadataCache::new()
            .persist_manifest_dir(rust_inspect_manifest_dir.manifest_dir())
            .map_err(|error| {
                CliError::failure(format!(
                    "failed to persist batched Rust inspection metadata for {}: {error}",
                    rust_inspect_manifest_dir.manifest_dir().display()
                ))
            })?;
    }
    record_timing(&mut timings_ms, "library_typecheck_modules", typecheck_start);

    let api_validation_start = Instant::now();
    materialize_api_alias_projections(&mut api_metadata_modules);
    let registry_module_path = |module: &ParsedModule| {
        if module.file_path == lib_entry {
            vec!["lib".to_string()]
        } else {
            module.path_segments.clone()
        }
    };
    let mut registry_metadata_modules = modules
        .iter()
        .filter_map(|module| {
            checked_type_info_by_path.get(&module.file_path).map(|type_info| {
                collect_checked_registry_metadata(type_info, registry_module_path(module), project_name.as_str())
            })
        })
        .collect::<Vec<_>>();
    let registry_alias_modules = modules
        .iter()
        .map(|module| collect_checked_api_alias_metadata(&module.ast, registry_module_path(module)))
        .collect::<Vec<_>>();
    materialize_registry_reexport_projections(&mut registry_metadata_modules, &registry_alias_modules);

    for diagnostic in validate_checked_api_docstrings(&api_metadata_modules) {
        if let Some(module) = modules
            .iter()
            .find(|module| module.path_segments == diagnostic.module_path)
        {
            all_errors.push_str(&diagnostics::format_error(
                module.file_path.to_string_lossy().as_ref(),
                &module.source,
                &diagnostic.error,
            ));
        } else {
            all_errors.push_str(&diagnostic.error.message);
            all_errors.push('\n');
        }
    }

    if !all_errors.is_empty() {
        return Err(CliError::failure(all_errors.trim_end()));
    }
    record_timing(&mut timings_ms, "library_validate_api_metadata", api_validation_start);

    std::fs::create_dir_all(&out_dir)
        .map_err(|error| CliError::failure(format!("failed to create {}: {error}", out_dir.display())))?;

    let export_start = Instant::now();
    let selected_exports = LibraryReexportResolver::new(&checked_exports_by_module)
        .resolve(lib_module)
        .map_err(|errs| {
            let mut msg = String::new();
            for err in &errs {
                msg.push_str(&diagnostics::format_error(
                    lib_module.file_path.to_string_lossy().as_ref(),
                    &lib_module.source,
                    err,
                ));
            }
            CliError::failure(msg.trim_end())
        })?;
    record_timing(&mut timings_ms, "library_resolve_exports", export_start);

    let manifest_start = Instant::now();
    let project_license = manifest.project.as_ref().and_then(|project| project.license.clone());

    let mut library_manifest =
        LibraryManifest::from_checked_exports(project_name.clone(), project_version.clone(), &selected_exports);
    library_manifest.contract_metadata.models = ContractMetadataPackage::new(
        contract_model_bundles
            .into_iter()
            .filter(|bundle| bundle.publishable)
            .collect(),
    );
    let mut checked_api = CheckedApiMetadataPackage {
        schema_version: CHECKED_API_METADATA_SCHEMA_VERSION,
        package: Some(CheckedApiPackageIdentity {
            name: project_name.clone(),
            version: Some(project_version.clone()),
        }),
        modules: api_metadata_modules,
        public_namespaces: Vec::new(),
    };
    materialize_checked_api_public_namespaces(&mut checked_api)
        .map_err(|error| CliError::failure(format!("failed to publish checked module namespaces: {error}")))?;
    library_manifest
        .contract_metadata
        .identity_graph
        .extend_checked_api_exports(&project_name, &checked_api, &checked_exports_by_source_module)
        .map_err(|error| CliError::failure(format!("failed to publish checked module identities: {error}")))?;
    library_manifest.contract_metadata.api = Some(checked_api);
    library_manifest.contract_metadata.provider = compiled_provider_metadata(CompiledProviderMetadataInputs {
        manifest: &manifest,
        feature_plan: &package_feature_plan,
        provider_plan: &provider_plan,
        library_manifest_index: &library_manifest_index,
        artifact_root: &out_dir,
        modules: &provider_metadata_modules,
        active_library_entrypoint: lib_module,
        checked_type_info_by_path: &checked_type_info_by_path,
    })?;
    let mut registry_metadata = CheckedRegistryMetadataPackage {
        schema_version: CHECKED_REGISTRY_METADATA_SCHEMA_VERSION,
        package: Some(CheckedRegistryPackageIdentity {
            name: project_name.clone(),
            version: Some(project_version.clone()),
        }),
        modules: registry_metadata_modules,
    };
    for module in &mut registry_metadata.modules {
        module.registries.retain(|registry| registry.public);
        module.entries.retain(|entry| entry.registry_public);
    }
    registry_metadata
        .modules
        .retain(|module| !module.registries.is_empty() || !module.entries.is_empty());
    library_manifest.contract_metadata.registry = Some(registry_metadata);
    #[cfg(feature = "rust_inspect")]
    if let Some(rust_inspect_manifest_dir) = rust_inspect_manifest_dir.as_ref() {
        library_manifest.rust_abi =
            collect_library_rust_abi(rust_inspect_manifest_dir.manifest_dir(), &metadata_query_paths)?;
    }
    record_timing(&mut timings_ms, "library_build_manifest_metadata", manifest_start);
    let manifest_path = out_dir.join(format!("{project_name}.incnlib"));

    // ---- Backend selection (#986) — declared before codegen, refused visibly if unavailable ----
    let (backend_selection, backend_executed) = select_and_resolve_backend(backend_options, &modules)?;

    let mut codegen = IrCodegen::new();
    codegen.set_preserve_dependency_public_items(true);
    codegen.set_registry_package_identity(Some(project_name.clone()));
    codegen.set_canonical_emission_package_identity(Some(project_name.clone()));
    codegen.set_root_source_module_name(
        lib_module
            .file_path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .map(str::to_string),
    );
    codegen.set_stdlib_cache(stdlib_cache);
    codegen.set_declared_crate_names(declared);
    codegen.set_provider_plan(Arc::clone(&provider_plan));
    let main_type_info = checked_type_info_by_path
        .get(&lib_module.file_path)
        .cloned()
        .ok_or_else(|| {
            CliError::failure(format!(
                "missing checked library analysis for {}",
                lib_module.file_path.display()
            ))
        })?;
    let mut dependency_type_info = HashMap::with_capacity(dep_modules.len());
    for module in dep_modules {
        let type_info = checked_type_info_by_path
            .get(&module.file_path)
            .cloned()
            .ok_or_else(|| {
                CliError::failure(format!(
                    "missing checked library analysis for {}",
                    module.file_path.display()
                ))
            })?;
        dependency_type_info.insert(module.path_segments.clone(), type_info);
    }
    codegen.set_prechecked_type_info(main_type_info, dependency_type_info);
    codegen.set_public_ordinal_type_identities(public_ordinal_type_identities(
        lib_module,
        project_name.as_str(),
        &selected_exports,
    ));
    for module in dep_modules
        .iter()
        .filter(|module| compiled_sdk_modules.contains_emission_path(&module.path_segments))
    {
        codegen.add_dependency_symbol_module_with_path_segments(
            &module.name,
            &module.ast,
            module.path_segments.clone(),
        );
    }
    for module in &emitted_dep_modules {
        codegen.add_module_with_path_segments(&module.name, &module.ast, module.path_segments.clone());
    }
    let mut generator = ProjectGenerator::new(&out_dir, project_name.as_str(), false);
    let checked_api = library_manifest.contract_metadata.api.as_ref().ok_or_else(|| {
        CliError::failure("checked API metadata is unavailable while generating public namespace facades")
    })?;
    generator.set_public_namespace_facades(checked_api);
    // Canonical workspace locking uses a synthetic package name so every member resolves one shared Cargo graph.
    // A published library artifact instead has an identity contract across Cargo.toml, `[lib]`, and `.incnlib`, so
    // its generated Cargo package must retain the selected producer project's name.
    generator.set_package_name(Some(project_name.clone()));
    generator.set_package_metadata(Some(project_version.clone()), project_license);
    generator.set_provider_plan(&provider_plan);
    generator.set_sdk_path_dependencies(project_requirements.sdk_path_dependencies.clone());
    if normal_oven {
        generator.set_cargo_target_dir_override(None);
        generator.set_generated_cache_context(None, None);
    } else {
        generator.set_cargo_target_dir_override(Some(managed_target_path.clone()));
        generator.set_generated_cache_context(managed_target_lease, managed_target_identity);
    }
    generator.set_stdlib_features(project_requirements.stdlib_features.clone());
    generator.set_include_dev_dependencies(
        lock_payload_for_typecheck.is_some() || oven_plan_mode == OvenProjectPlanMode::ExplicitBake,
    );
    let rust_edition = manifest.build.as_ref().and_then(|build| build.rust_edition.clone());
    generator.set_rust_edition(rust_edition.clone());
    #[cfg(feature = "rust_inspect")]
    if let Some(rust_inspect_manifest_dir) = rust_inspect_manifest_dir.as_ref() {
        codegen.set_rust_inspect_manifest_dir(rust_inspect_manifest_dir.manifest_dir().to_path_buf());
    }
    generator.set_cargo_lock_payload(lock_payload_for_typecheck);
    generator.set_cargo_lock_projection_root(cargo_lock_projection_root.clone());
    generator.set_clear_cargo_lock(clear_cargo_lock);
    generator.set_cargo_policy_flags(cargo_flags);
    remove_generated_library_self_dependencies(&mut resolved, &project_root);
    let oven_inline_rust_dependencies = normal_oven
        .then(|| oven_source_inline_dependency_specs(&resolved, &source_inline_crates))
        .transpose()?;
    let rust_dependencies = resolved.dependencies.clone();
    let rust_dev_dependencies = resolved.dev_dependencies.clone();
    let checked_provider_profiles = if normal_oven {
        let target = oven_target
            .as_deref()
            .ok_or_else(|| CliError::failure("normal Oven library build omitted target"))?;
        let toolchain = oven_toolchain
            .as_deref()
            .ok_or_else(|| CliError::failure("normal Oven library build omitted toolchain"))?;
        checked_packaged_provider_profiles(
            &provider_plan,
            &explicit_bake_profiles(),
            target,
            toolchain,
            authority_context,
        )?
    } else {
        Vec::new()
    };
    let oven_plan_dependencies = oven_inline_rust_dependencies.clone().unwrap_or_default();
    if normal_oven {
        let store = oven_store
            .as_ref()
            .ok_or_else(|| CliError::failure("normal Oven library build omitted its bounded store"))?;
        import_packaged_provider_loafs_for_explicit_bake(oven_plan_mode, store, &checked_provider_profiles)?;
    }
    let mut report_draft = BuildReportDraft {
        mode: BuildReportMode::Library,
        profile: "release".to_string(),
        project: manifest_project_report(Some(&manifest), project_name.as_str(), &project_root),
        entrypoint: Some(lib_entry.to_string_lossy().to_string()),
        library_root: Some(project_root.to_string_lossy().to_string()),
        source_files: source_file_report(&modules),
        generated: generated_project_report(
            generator.output_dir(),
            &generator.crate_root_path(),
            &generator.cargo_target_dir(),
        ),
        artifacts: Vec::new(),
        dependencies: dependencies_report(
            &rust_dependencies,
            &rust_dev_dependencies,
            incan_dependencies_report(manifest.library_dependencies().iter().collect()),
            project_requirements.stdlib_features.clone(),
        ),
        semantic: semantic_report(
            compilation_session.sdk_inventory.as_deref(),
            compilation_session.sdk_components.as_ref(),
            Some(&package_feature_plan),
            &provider_plan,
        ),
        cargo: Some(cargo_report(
            &cargo_policy,
            cargo_features.cargo_features.clone(),
            cargo_features.cargo_no_default_features,
            cargo_features.cargo_all_features,
        )),
        oven: None,
        interop: interop_report(
            &inline_imports,
            rust_extern_report_paths(&rust_extern_contexts),
            metadata_query_paths.clone(),
        ),
        notes: vec![
            "Generated Rust is current backend output for inspection and debugging, not a stable Rust ABI.".to_string(),
        ],
        backend: None,
    };
    generator.set_dependencies(resolved.dependencies);
    generator.set_dev_dependencies(resolved.dev_dependencies);

    // Keep the historical aggregate for existing consumers, while separating the stages that were previously
    // attributed misleadingly as one `library_generate_rust` cost in Oven performance evidence.
    let codegen_start = Instant::now();
    let (backend_output_identity, generation_metadata) = if emitted_dep_modules.is_empty() {
        let emit_rust_start = Instant::now();
        let (rust_code, generation_metadata) = codegen
            .try_generate_with_metadata(&lib_module.ast, &lib_module.path_segments)
            .map_err(|e| CliError::failure(format!("Code generation error: {e}")))?;
        record_timing(&mut timings_ms, "library_codegen_emit_rust", emit_rust_start);
        let write_project_start = Instant::now();
        generator
            .generate(&rust_code)
            .map_err(|e| CliError::failure(format!("Error generating project: {e}")))?;
        record_timing(&mut timings_ms, "library_codegen_write_project", write_project_start);
        (digest_output(&[rust_code.as_str()]), generation_metadata)
    } else {
        let module_paths: Vec<Vec<String>> = emitted_dep_modules
            .iter()
            .map(|module| module.path_segments.clone())
            .collect();
        let emit_rust_start = Instant::now();
        let ((main_code, rust_modules), generation_metadata) = codegen
            .try_generate_multi_file_nested_with_metadata(&lib_module.ast, &module_paths, &lib_module.path_segments)
            .map_err(|e| CliError::failure(format!("Code generation error: {e}")))?;
        record_timing(&mut timings_ms, "library_codegen_emit_rust", emit_rust_start);
        let write_project_start = Instant::now();
        generator
            .generate_nested(&main_code, &rust_modules)
            .map_err(|e| CliError::failure(format!("Error generating project: {e}")))?;
        record_timing(&mut timings_ms, "library_codegen_write_project", write_project_start);
        (
            multi_file_output_identity(&main_code, &rust_modules),
            generation_metadata,
        )
    };
    generation_metadata
        .apply_to_library_manifest(&mut library_manifest)
        .map_err(|error| {
            CliError::failure(format!(
                "failed to publish inferred implementation requirements: {error}"
            ))
        })?;
    let backend_receipt = finalize_backend_receipt(&backend_selection, backend_executed, backend_output_identity)?;
    // Not persisted here — see the matching comment in `prepare_oven_project`: this function
    // also runs for internal/dependency callers, and real compilation still follows below. The
    // receipt is published once by `build_library_report` after the whole build succeeds (#986).
    report_draft.backend = Some(backend_receipt);
    let synchronize_provider_dependencies_start = Instant::now();
    synchronize_projected_provider_dependencies(
        &mut library_manifest,
        &out_dir,
        &generator.effective_dependencies().map_err(|error| {
            CliError::failure(format!("failed to resolve projected provider dependencies: {error}"))
        })?,
    )?;
    record_timing(
        &mut timings_ms,
        "library_codegen_sync_provider_dependencies",
        synchronize_provider_dependencies_start,
    );
    let oven_profiles_start = Instant::now();
    let oven = if normal_oven {
        let rustc = oven_rustc.ok_or_else(|| CliError::failure("normal Oven library build omitted rustc"))?;
        let target = oven_target.ok_or_else(|| CliError::failure("normal Oven library build omitted target"))?;
        let toolchain =
            oven_toolchain.ok_or_else(|| CliError::failure("normal Oven library build omitted toolchain"))?;
        let store = oven_store
            .as_ref()
            .ok_or_else(|| CliError::failure("normal Oven library build omitted its bounded store"))?;
        let mut profiles = BTreeMap::new();
        let oven_receipt_source_evidence_start = Instant::now();
        let mut source_evidence_request = OvenGeneratedProjectRequest::new(
            &project_root,
            &project_name,
            &project_version,
            target.clone(),
            toolchain.clone(),
            "debug",
            Vec::new(),
        )
        .with_generated_source("generated-root", generator.crate_root_path())
        .with_generated_source_tree("generated-source-tree", generator.output_dir().join("src"));
        for (name, value) in oven_build_inputs.as_ref().into_iter().flat_map(|inputs| inputs.iter()) {
            source_evidence_request = source_evidence_request.with_build_unit_input(name.clone(), value.clone());
        }
        let generated_source_evidence = generated_project_source_evidence(&source_evidence_request)
            .map_err(|error| CliError::failure(error.to_string()))?;
        record_timing(
            &mut timings_ms,
            "library_oven_receipt_source_evidence",
            oven_receipt_source_evidence_start,
        );
        for profile in explicit_bake_profiles() {
            let mut receipt_request = OvenGeneratedProjectRequest::new(
                &project_root,
                &project_name,
                &project_version,
                target.clone(),
                toolchain.clone(),
                profile,
                Vec::new(),
            )
            .with_generated_source("generated-root", generator.crate_root_path())
            .with_generated_source_tree("generated-source-tree", generator.output_dir().join("src"));
            for (name, value) in oven_build_inputs.as_ref().into_iter().flat_map(|inputs| inputs.iter()) {
                receipt_request = receipt_request.with_build_unit_input(name.clone(), value.clone());
            }
            let receipt = receipt_generated_project_with_source_evidence(&receipt_request, &generated_source_evidence)
                .map_err(|error| CliError::failure(error.to_string()))?;
            let receipt_path = if profile == "release" {
                crate::oven::default_receipt_path(&project_root)
            } else {
                crate::oven::default_receipt_path(&project_root).with_file_name("library-debug-receipt.json")
            };
            write_receipt(&receipt, receipt_path.clone()).map_err(|error| CliError::failure(error.to_string()))?;
            let required_registry_dependencies = format_oven_registry_dependency_requirements(&oven_plan_dependencies);
            let oven_select_direct_rustc_plan_start = Instant::now();
            // An imported package Loaf is sufficient only for consume-only commands. An explicit library bake must
            // instead publish the library's own direct registry roots with its complete generated source closure.
            let packaged_provider_selection = if oven_plan_mode == OvenProjectPlanMode::ConsumeOnly {
                select_packaged_provider_plan(store, &checked_provider_profiles, profile, &receipt)?
            } else {
                None
            };
            let plan_preparation = if let Some(selection) = packaged_provider_selection {
                Some(OvenDirectRustcPlanPreparation {
                    plan_selection: selection,
                    materialization: OvenToolchainMaterialization::Reused,
                    cargo_process_started: false,
                })
            } else {
                select_or_bake_generated_project_plan(
                    oven_plan_mode,
                    store,
                    &receipt,
                    OvenProjectDependencySurface {
                        selection: &oven_plan_dependencies,
                    },
                    generator.output_dir(),
                    &generator.crate_root_path(),
                    &rustc,
                )?
            }
            .ok_or_else(|| {
                CliError::failure(format!(
                    "{}. `incan build --lib` {}. {} (Needs: {}. `{profile}` build record {}; generated project: {}; receipt: {}.)",
                    OVEN_DEPENDENCY_MISS_SUMMARY,
                    OVEN_NO_IMPLICIT_DEPENDENCY_BUILD,
                    OVEN_LOAF_MISS_GUIDANCE,
                    required_registry_dependencies,
                    receipt.identity,
                    generator.output_dir().display(),
                    receipt_path.display(),
                ))
            })?;
            record_timing(
                &mut timings_ms,
                "library_oven_select_direct_rustc_plan",
                oven_select_direct_rustc_plan_start,
            );
            let plan_selection = plan_preparation.plan_selection;
            let oven_validate_direct_rustc_plan_start = Instant::now();
            let registry_authority = registry_leaf_authority_for_plan_selection(&plan_selection)?;
            let full_artifact_plan = plan_selection.artifact_plan();
            let artifact_plan = plan_selection
                .source_artifact_plan("generated-root")
                .map_err(oven_rustc_error)?;
            validate_selected_plan_registry_dependencies(
                &oven_plan_dependencies,
                &artifact_plan,
                registry_authority.as_ref(),
                profile,
            )?;
            let inline_libraries = declared_rust_libraries_missing_from_selected_plan_with_current_project_paths(
                oven_inline_rust_dependencies.as_deref().unwrap_or_default(),
                &artifact_plan,
                plan_selection.seals_current_project_path_dependencies(),
            );
            let selected_path_authority = compiler_selected_path_authority(full_artifact_plan, Some(&provider_plan));
            record_timing(
                &mut timings_ms,
                "library_oven_validate_direct_rustc_plan",
                oven_validate_direct_rustc_plan_start,
            );
            let oven_prepare_caller_owned_libraries_start = Instant::now();
            let mut caller_owned_libraries = oven_caller_owned_libraries(&provider_plan, profile)?;
            caller_owned_libraries.extend(
                materialize_declared_rust_libraries_with_selected_path_authority(
                    &generator.output_dir().join("oven").join("inline-rust"),
                    &rustc,
                    &target,
                    profile,
                    &inline_libraries,
                    registry_authority.as_ref(),
                    selected_path_authority.as_ref(),
                )
                .map_err(oven_rustc_error)?,
            );
            record_timing(
                &mut timings_ms,
                "library_oven_prepare_caller_owned_libraries",
                oven_prepare_caller_owned_libraries_start,
            );
            caller_owned_libraries.sort_by(|left, right| left.crate_name.cmp(&right.crate_name));
            if caller_owned_libraries
                .windows(2)
                .any(|pair| pair[0].crate_name == pair[1].crate_name)
            {
                return Err(CliError::failure(
                    "Oven Alpha resolved duplicate caller-owned Rust library crate names while preparing a library",
                ));
            }
            profiles.insert(
                profile.to_string(),
                OvenPreparedLibraryProfile {
                    receipt,
                    plan_selection,
                    materialization: plan_preparation.materialization,
                    provider_plan: provider_plan.clone(),
                    caller_owned_libraries,
                },
            );
        }
        Some(OvenPreparedLibrary {
            rustc,
            crate_name: ProjectGenerator::rust_target_name(&project_name),
            rust_edition: rust_edition.clone().unwrap_or_else(|| "2024".to_string()),
            profiles,
        })
    } else {
        None
    };
    record_timing(&mut timings_ms, "library_oven_prepare_profiles", oven_profiles_start);
    // A normal Oven library build derives the compiler-owned vocab helper exclusively from its selected immutable
    // release plan, but only when this project actually declares a vocab companion. Constructing that context for
    // every library made a vocab-free explicit bake require unrelated `incan_vocab` artifacts and repeated work.
    // The selected release plan owns its lease through extraction; compatibility publication retains its explicit
    // boundary and normal consumers remain Cargo-free.
    let oven_vocab_context_start = Instant::now();
    let normal_oven_vocab_context = if manifest.vocab().is_some() {
        if let Some(oven) = oven.as_ref() {
            // Prefer the release selection, but fall back to whatever profile this bake actually prepared.
            // An explicit bake may be narrowed to one profile (see `explicit_bake_profiles`), and the vocab
            // helper is a wasm desugarer whose identity does not depend on the host profile that carried it.
            let release = oven
                .profiles
                .get("release")
                .or_else(|| oven.profiles.values().next())
                .ok_or_else(|| CliError::failure("normal Oven library build prepared no profile selection"))?;
            if release.plan_selection.artifacts().vocab_auxiliary_targets.is_empty() {
                // A receipt-exact project closure may predate project-extension publication or legitimately own a
                // disjoint Rust ABI universe. It must not manufacture compiler-private helper artifacts or invoke
                // Cargo merely because the source also declares a vocab companion. The helper has no project ABI:
                // select it only from the exact compiler-owned stdlib Loaf that authorizes this receipt's
                // release-cohort inputs and target, while the project's selected closure remains the sole authority
                // for its code.
                let base = project_extension_base_loaf(&release.receipt)?.ok_or_else(|| {
                    CliError::failure(
                        "selected Oven project closure has no compiler-owned vocabulary helper and the active Incan release has no compatible release-cohort Loaf; run the explicit release Loaf bake for this compiler version. Normal library builds will not invoke Cargo",
                    )
                })?;
                Some(oven_vocab_direct_rustc_context_from_plan(
                    &oven.rustc,
                    &base.artifact_plan,
                    &base.artifacts,
                    &base.artifact_root,
                )?)
            } else {
                let artifact_root = release.plan_selection.vocab_artifact_root().ok_or_else(|| {
                    CliError::failure(
                        "selected Oven project extension splits a vocabulary auxiliary closure across immutable roots; rebake against a compatible standard-library Loaf",
                    )
                })?;
                Some(oven_vocab_direct_rustc_context_from_plan(
                    &oven.rustc,
                    release.plan_selection.artifact_plan(),
                    release.plan_selection.artifacts(),
                    artifact_root,
                )?)
            }
        } else {
            None
        }
    } else {
        None
    };
    record_timing(
        &mut timings_ms,
        "library_oven_prepare_vocab_context",
        oven_vocab_context_start,
    );
    let mut pending_desugarer_artifact: Option<PendingDesugarerArtifact> = None;
    let vocab_start = Instant::now();
    if let Some(vocab_extraction) = collect_library_vocab_metadata(
        &manifest,
        &project_root,
        (!normal_oven).then_some(managed_target_path.as_path()),
        normal_oven_vocab_context.as_ref(),
    )? {
        pending_desugarer_artifact = vocab_extraction.pending_desugarer_artifact;
        library_manifest.vocab = Some(vocab_extraction.payload);
        library_manifest.soft_keywords.activations = vocab_extraction.compatibility_activations;
    }
    record_timing(&mut timings_ms, "library_collect_vocab_metadata", vocab_start);
    package_desugarer_artifact(&out_dir, pending_desugarer_artifact.as_ref())?;
    if let Some(oven) = oven.as_ref() {
        report_draft.generated = oven_generated_project_report(
            generator.output_dir(),
            &generator.crate_root_path(),
            &generator.output_dir().join("oven"),
        );
        report_draft.cargo = None;
        // Report identities from the release selection when present, else the profile that was prepared.
        let release = oven
            .profiles
            .get("release")
            .or_else(|| oven.profiles.values().next())
            .ok_or_else(|| CliError::failure("normal Oven library build prepared no profile selection"))?;
        report_draft.oven = Some(BuildOvenReport {
            receipt_identity: release.receipt.identity.clone(),
            build_unit_identity: release.receipt.build_unit_identity.clone(),
            plan_identity: release.plan_selection.report_identity(),
        });
        report_draft.notes = vec![
            "Oven Alpha selected a receipt-bound direct-rustc plan; normal library execution did not invoke Cargo or inspect a Cargo target directory.".to_string(),
        ];
    }
    record_timing(&mut timings_ms, "library_generate_rust", codegen_start);
    record_timing(&mut timings_ms, "library_prepare_total", prepare_start);

    Ok(PreparedLibraryProject {
        generator,
        project_root,
        entrypoint: lib_entry,
        out_dir,
        manifest_path,
        library_manifest,
        timings_ms,
        report: report_draft,
        oven,
        #[cfg(feature = "rust_inspect")]
        rust_inspect_manifest_dir: rust_inspect_manifest_dir
            .as_ref()
            .map(|workspace| workspace.manifest_dir().to_path_buf()),
    })
}

/// Synchronize newly published public dependency metadata with the exact projected paths rendered into Cargo.toml.
fn synchronize_projected_provider_dependencies(
    library_manifest: &mut LibraryManifest,
    artifact_root: &Path,
    dependencies: &[DependencySpec],
) -> CliResult<()> {
    for descriptor in library_manifest
        .contract_metadata
        .provider
        .provider_dependencies
        .iter_mut()
        .filter(|dependency| dependency.kind == ProviderDependencyKind::PublicPackage)
    {
        let Some(dependency) = dependencies
            .iter()
            .find(|dependency| dependency.crate_name == descriptor.dependency_key)
        else {
            continue;
        };
        let cargo_package = dependency.package.as_deref().unwrap_or(dependency.crate_name.as_str());
        if cargo_package != descriptor.provider_name {
            return Err(CliError::failure(format!(
                "projected Cargo dependency `{}` names package `{cargo_package}`, but its checked provider edge names `{}`",
                descriptor.dependency_key, descriptor.provider_name
            )));
        }
        let DependencySource::Path { path } = &dependency.source else {
            continue;
        };
        descriptor.relative_artifact_path = relative_provider_artifact_path(artifact_root, path)?;
        descriptor.artifact_digest = digest_provider_artifact(path).map_err(|error| {
            CliError::failure(format!(
                "failed to hash projected provider dependency `{}` artifact {}: {error}",
                descriptor.dependency_key,
                path.display()
            ))
        })?;
    }
    Ok(())
}

/// Borrowed inputs needed to freeze checked provider facts into a library artifact.
///
/// The checked type information stays in the compiler pipeline until this projection. It carries the canonical
/// callable-to-capability facts that provider operation lowering consumes through the selected manifest.
struct CompiledProviderMetadataInputs<'a> {
    manifest: &'a ProjectManifest,
    feature_plan: &'a PackageFeaturePlan,
    provider_plan: &'a ProviderPlan,
    library_manifest_index: &'a LibraryManifestIndex,
    artifact_root: &'a Path,
    modules: &'a [ParsedModule],
    active_library_entrypoint: &'a ParsedModule,
    checked_type_info_by_path: &'a BTreeMap<PathBuf, typechecker::TypeCheckInfo>,
}

/// Build transport-stable provider facts from the checked physical artifact projection.
fn compiled_provider_metadata(inputs: CompiledProviderMetadataInputs<'_>) -> CliResult<CompiledProviderMetadata> {
    let graph =
        PackageFeatureGraph::from_manifest(inputs.manifest).map_err(|error| CliError::failure(error.to_string()))?;
    let root_features = inputs
        .feature_plan
        .root_package()
        .map(|package| &package.features)
        .ok_or_else(|| CliError::failure("resolved package feature plan is missing its root package"))?;
    let library_entrypoint = inputs
        .modules
        .iter()
        .find(|module| module.file_path == inputs.active_library_entrypoint.file_path)
        .ok_or_else(|| CliError::failure("unprojected provider graph is missing its library entrypoint"))?;
    let source_root = resolve_source_root(inputs.manifest.project_root(), Some(inputs.manifest));
    let module_requirements =
        provider_module_reachability_requirements(inputs.modules, library_entrypoint, &source_root)?;
    let mut namespace_claims = inputs
        .modules
        .iter()
        .filter(|module| {
            module.file_path != inputs.active_library_entrypoint.file_path
                && !module.path_segments.is_empty()
                && !module_is_owned_by_dependency_provider(inputs.provider_plan, &module.path_segments)
        })
        .flat_map(|module| {
            module_requirements
                .get(&module.path_segments)
                .into_iter()
                .flatten()
                .map(|required_features| ProviderModuleClaim {
                    module_path: module.path_segments.clone(),
                    required_features: required_features.clone(),
                })
        })
        .collect::<Vec<_>>();
    namespace_claims.sort();
    namespace_claims.dedup();

    let public_features = graph.provider_metadata();
    let mut fact_requirements = Vec::new();
    for module in inputs
        .modules
        .iter()
        .filter(|module| !module_is_owned_by_dependency_provider(inputs.provider_plan, &module.path_segments))
    {
        let requirements = module_requirements.get(&module.path_segments).ok_or_else(|| {
            CliError::failure(format!(
                "unprojected provider module `{}` has no reachability predicate from the library entrypoint",
                module.path_segments.join(".")
            ))
        })?;
        fact_requirements.extend(provider_fact_requirements(module, requirements));
    }
    fact_requirements.extend(
        namespace_claims
            .iter()
            .filter(|claim| !claim.required_features.is_empty())
            .map(|claim| ProviderFactRequirement {
                kind: ProviderFactKind::Module,
                identity: claim.module_path.join("."),
                required_features: claim.required_features.clone(),
            }),
    );
    fact_requirements.extend(public_features.iter().flat_map(|(feature, metadata)| {
        metadata
            .required_sdk_components
            .iter()
            .map(move |component| ProviderFactRequirement {
                kind: ProviderFactKind::ComponentRequirement,
                identity: component.clone(),
                required_features: BTreeSet::from([feature.clone()]),
            })
    }));
    fact_requirements.sort();
    fact_requirements.dedup();

    let provider_dependencies = compiled_provider_dependencies(
        inputs.feature_plan,
        inputs.library_manifest_index,
        inputs.provider_plan,
        inputs.artifact_root,
    )?;
    let implementation_facets = provider_implementation_facets(&namespace_claims);
    let operation_descriptors = provider_operation_metadata_from_checked_type_info(inputs.checked_type_info_by_path)?;
    let semantic_source_inputs = inputs
        .modules
        .iter()
        .filter(|module| !module_is_owned_by_dependency_provider(inputs.provider_plan, &module.path_segments))
        .map(|module| {
            let label = if module.path_segments.is_empty() {
                "<root>".to_string()
            } else {
                module.path_segments.join(".")
            };
            (label, module.file_path.clone())
        })
        .collect::<Vec<_>>();
    let trusted_source_roots = crate::toolchain_layout::find_stdlib_source_dir()
        .into_iter()
        .collect::<Vec<_>>();
    let semantic_source_digest = digest_provider_source_inputs(
        inputs.manifest.project_root(),
        inputs.manifest.path(),
        &semantic_source_inputs,
        &trusted_source_roots,
    )
    .map_err(|error| CliError::failure(format!("failed to fingerprint authored provider inputs: {error}")))?;
    Ok(CompiledProviderMetadata {
        semantic_source_digest: Some(semantic_source_digest),
        namespace_claims,
        public_features,
        active_features: root_features.active_features.clone(),
        provider_dependencies,
        fact_requirements,
        required_sdk_components: root_features.required_sdk_components.clone(),
        implementation_facets,
        operation_descriptors,
        ..CompiledProviderMetadata::default()
    })
}

/// Project declaration-side provider-operation facts into the selected library artifact.
///
/// The typechecker is the only source of these pairs: it resolved each decorated operation's capability in the
/// declaring module. Sorting and rejecting duplicate canonical identities makes manifest output deterministic and
/// prevents a package with two copies of one declaration from acquiring order-dependent provider meaning.
fn provider_operation_metadata_from_checked_type_info(
    checked_type_info_by_path: &BTreeMap<PathBuf, typechecker::TypeCheckInfo>,
) -> CliResult<Vec<ProviderOperationMetadata>> {
    let mut operation_descriptors = checked_type_info_by_path
        .values()
        .flat_map(|type_info| type_info.declarations.provider_operations.values())
        .map(|operation| ProviderOperationMetadata {
            operation: operation.operation.clone(),
            required_capability: operation.required_capability.clone(),
            runtime_requirements: operation.runtime_requirements.clone(),
        })
        .collect::<Vec<_>>();
    operation_descriptors.sort_by(|left, right| left.operation.cmp(&right.operation));
    if let Some(duplicate) = operation_descriptors
        .windows(2)
        .find(|entries| entries[0].operation == entries[1].operation)
    {
        return Err(CliError::failure(format!(
            "provider operation metadata contains duplicate declaration `{}`",
            duplicate[0].operation.declaration_name
        )));
    }
    Ok(operation_descriptors)
}

/// Freeze the active Incan dependency edges into artifact-owned, relocation-safe provider metadata.
fn compiled_provider_dependencies(
    feature_plan: &PackageFeaturePlan,
    library_manifest_index: &LibraryManifestIndex,
    provider_plan: &ProviderPlan,
    artifact_root: &Path,
) -> CliResult<Vec<ProviderDependencyMetadata>> {
    let mut dependencies = Vec::new();
    for edge in feature_plan
        .edges()
        .filter(|edge| edge.from.as_path() == feature_plan.root())
    {
        let entry = library_manifest_index.get(&edge.dependency_key).ok_or_else(|| {
            CliError::failure(format!(
                "active provider dependency `pub::{}` is missing from the checked library manifest index",
                edge.dependency_key
            ))
        })?;
        let (manifest, metadata) = match entry {
            LibraryManifestIndexEntry::Loaded { manifest, metadata } => (manifest, metadata),
            LibraryManifestIndexEntry::Failed(failure) => {
                return Err(CliError::failure(format!(
                    "active provider dependency `pub::{}` could not be loaded from {}: {}",
                    edge.dependency_key,
                    failure.path.display(),
                    failure.message
                )));
            }
        };
        if metadata.kind != LibraryArtifactKind::Materialized {
            return Err(CliError::failure(format!(
                "active provider dependency `pub::{}` has parser-only metadata; build its compiled artifact before publishing this provider",
                edge.dependency_key
            )));
        }
        let artifact_digest = digest_provider_artifact(&metadata.crate_root).map_err(|error| {
            CliError::failure(format!(
                "failed to hash provider dependency `pub::{}` artifact {}: {error}",
                edge.dependency_key,
                metadata.crate_root.display()
            ))
        })?;
        dependencies.push(ProviderDependencyMetadata {
            kind: crate::library_manifest::ProviderDependencyKind::PublicPackage,
            dependency_key: edge.dependency_key.clone(),
            provider_name: manifest.name.clone(),
            provider_version: manifest.version.clone(),
            artifact_digest,
            relative_artifact_path: relative_provider_artifact_path(artifact_root, &metadata.crate_root)?,
            requested_features: edge.requested_features.clone(),
            default_features: edge.default_features,
            optional: edge.optional,
        });
    }
    for provider in provider_plan.sdk_link_roots() {
        let Some(metadata) = provider.artifact.as_ref() else {
            continue;
        };
        let artifact_digest = digest_provider_artifact(&metadata.crate_root).map_err(|error| {
            CliError::failure(format!(
                "failed to hash private SDK provider dependency `{}` artifact {}: {error}",
                provider.identity.name,
                metadata.crate_root.display()
            ))
        })?;
        dependencies.push(ProviderDependencyMetadata {
            kind: crate::library_manifest::ProviderDependencyKind::PrivateImplementation,
            dependency_key: metadata.dependency_key.clone(),
            provider_name: provider.identity.name.clone(),
            provider_version: provider.identity.version.clone(),
            artifact_digest,
            relative_artifact_path: relative_provider_artifact_path(artifact_root, &metadata.crate_root)?,
            requested_features: provider.identity.feature_projection.clone(),
            default_features: false,
            optional: false,
        });
    }
    dependencies.sort();
    dependencies.dedup();
    Ok(dependencies)
}

/// Compute one normalized portable path between two existing provider artifact roots.
fn relative_provider_artifact_path(from: &Path, to: &Path) -> CliResult<String> {
    let from = fs::canonicalize(from).map_err(|error| {
        CliError::failure(format!(
            "failed to canonicalize provider artifact root {}: {error}",
            from.display()
        ))
    })?;
    let to = fs::canonicalize(to).map_err(|error| {
        CliError::failure(format!(
            "failed to canonicalize dependency artifact root {}: {error}",
            to.display()
        ))
    })?;
    let from_components = from.components().collect::<Vec<_>>();
    let to_components = to.components().collect::<Vec<_>>();
    let common = from_components
        .iter()
        .zip(&to_components)
        .take_while(|(left, right)| left == right)
        .count();
    if common == 0 {
        return Err(CliError::failure(format!(
            "provider artifact roots {} and {} have no relocatable filesystem ancestor",
            from.display(),
            to.display()
        )));
    }
    let mut relative = PathBuf::new();
    for _ in common..from_components.len() {
        relative.push("..");
    }
    for component in &to_components[common..] {
        relative.push(component.as_os_str());
    }
    let rendered = relative.to_string_lossy().replace('\\', "/");
    if rendered.is_empty() {
        return Err(CliError::failure("a provider artifact cannot depend on itself"));
    }
    Ok(rendered)
}

/// Parse the complete local provider graph without dropping inactive feature-conditioned declarations.
///
/// The checked API and generated Rust remain specialized to the selected feature projection. This parallel metadata
/// view preserves the complete positive condition inventory so consumers and inspection can explain inactive facts
/// without reparsing provider source.
fn collect_unprojected_provider_modules(
    library_entrypoint: &Path,
    session: &super::common::CompilationSession,
) -> CliResult<Vec<ParsedModule>> {
    let mut pending = super::common::library_source_seeds(library_entrypoint, session)?;
    let mut processed = HashSet::new();
    let mut modules = Vec::new();

    while let Some((file_path, module_name, path_segments)) = pending.pop() {
        let canonical_path = file_path.canonicalize().unwrap_or_else(|_| file_path.clone());
        if !processed.insert(canonical_path) {
            continue;
        }
        let source = fs::read_to_string(&file_path)
            .map_err(|error| CliError::failure(format!("failed to read {}: {error}", file_path.display())))?;
        let ast = session
            .parse_source_unprojected(&file_path, &source, false)
            .map_err(|errors| {
                let rendered = errors
                    .iter()
                    .map(|error| diagnostics::format_error(file_path.to_string_lossy().as_ref(), &source, error))
                    .collect::<String>();
                CliError::failure(rendered.trim_end())
            })?;
        session.validate_parsed_program_features(&ast).map_err(|errors| {
            let rendered = errors
                .iter()
                .map(|error| diagnostics::format_error(file_path.to_string_lossy().as_ref(), &source, error))
                .collect::<String>();
            CliError::failure(rendered.trim_end())
        })?;
        let base_dir = file_path.parent().unwrap_or(session.source_root.as_path());
        for resolved in resolve_program_source_imports(&ast, base_dir, Some(&session.source_root)) {
            match resolved.resolution {
                SourceModuleImportResolution::Local(module) => {
                    pending.push((module.file_path, module.module_name, module.path_segments));
                }
                SourceModuleImportResolution::SelfImport {
                    module_ref,
                    import_path,
                    can_use_root_import,
                } => {
                    let error = diagnostics::CompileError::new(
                        self_import_diagnostic_message(&module_ref, &import_path, can_use_root_import),
                        resolved.span,
                    );
                    return Err(CliError::failure(
                        diagnostics::format_error(file_path.to_string_lossy().as_ref(), &source, &error).trim_end(),
                    ));
                }
                SourceModuleImportResolution::Stdlib { .. } | SourceModuleImportResolution::External => {}
            }
        }
        modules.push(ParsedModule {
            name: module_name,
            path_segments,
            file_path,
            source,
            ast,
        });
    }

    Ok(modules)
}

/// Freeze the source SDK publisher's current Rust-backend mappings into provider-owned artifact facets.
///
/// Consumers read these mappings from `.incnlib`; they never rediscover Cargo features or dependencies from a
/// compiler-side stdlib module inventory. This bootstrap adapter can disappear once provider source can author the
/// equivalent backend mappings directly.
fn provider_implementation_facets(namespace_claims: &[ProviderModuleClaim]) -> Vec<ProviderImplementationFacet> {
    if env::var_os(SDK_PROVIDER_BUILD_ENV).is_none() {
        return Vec::new();
    }
    let roots = namespace_claims
        .iter()
        .filter_map(|claim| claim.module_path.first().cloned())
        .collect::<BTreeSet<_>>();
    roots
        .into_iter()
        .filter_map(|root| {
            let namespace = incan_core::lang::stdlib::find_namespace(&root)?;
            let required_modules = namespace_claims
                .iter()
                .filter(|claim| claim.module_path.first() == Some(&root))
                .map(|claim| claim.module_path.clone())
                .collect();
            let cargo_features = namespace
                .feature
                .map(|feature| {
                    BTreeMap::from([(
                        crate::backend::project::INCAN_STDLIB_CRATE_NAME.to_string(),
                        BTreeSet::from([feature.to_string()]),
                    )])
                })
                .unwrap_or_default();
            let cargo_dependencies = namespace
                .extra_crate_deps
                .iter()
                .map(|dependency| ProviderCargoDependency {
                    crate_name: dependency.crate_name.to_string(),
                    package: incan_core::lang::stdlib::extra_crate_package_alias(dependency.crate_name)
                        .map(str::to_string),
                    version: match dependency.source {
                        incan_core::lang::stdlib::StdlibExtraCrateSource::Version(version) => Some(version.to_string()),
                        incan_core::lang::stdlib::StdlibExtraCrateSource::Path(_) => None,
                    },
                    features: dependency
                        .features
                        .iter()
                        .map(|feature| (*feature).to_string())
                        .collect(),
                    default_features: true,
                    source: match dependency.source {
                        incan_core::lang::stdlib::StdlibExtraCrateSource::Version(_) => {
                            ProviderCargoDependencySource::Registry
                        }
                        incan_core::lang::stdlib::StdlibExtraCrateSource::Path(relative_path) => {
                            ProviderCargoDependencySource::Toolchain {
                                relative_path: relative_path.to_string(),
                            }
                        }
                    },
                })
                .collect();
            Some(ProviderImplementationFacet {
                id: format!("rust_{root}"),
                required_modules,
                required_features: BTreeSet::new(),
                cargo_features,
                cargo_dependencies,
            })
        })
        .collect()
}

/// Return whether an already-linked SDK provider owns this emitted `__incan_std.*` module.
fn module_is_owned_by_dependency_provider(provider_plan: &ProviderPlan, emission_path: &[String]) -> bool {
    let prefix = [incan_core::lang::stdlib::INCAN_STD_NAMESPACE.to_string()];
    let relative = if let Some(relative) = emission_path.strip_prefix(prefix.as_slice()) {
        relative
    } else if env::var_os(SDK_PROVIDER_BUILD_ENV).is_some() {
        emission_path
    } else {
        return false;
    };
    let mut canonical = vec![incan_core::lang::stdlib::STDLIB_ROOT.to_string()];
    canonical.extend(relative.iter().cloned());
    provider_plan.active_sdk_provider_for_module(&canonical).is_some()
}

/// Derive positive feature predicates for entrypoint-reachable modules and disconnected automatic namespace roots.
///
/// Multiple incomparable predicates represent alternative additive paths. A broader predicate subsumes narrower paths,
/// so an unconditional import collapses every conditional route to the same module. Conditions accumulate across nested
/// imports, while modules outside the entrypoint graph remain unconditional roots of the published source hierarchy.
fn provider_module_reachability_requirements(
    modules: &[ParsedModule],
    entrypoint: &ParsedModule,
    source_root: &Path,
) -> CliResult<BTreeMap<Vec<String>, Vec<BTreeSet<String>>>> {
    let modules_by_path = modules
        .iter()
        .map(|module| (canonical_provider_source_path(&module.file_path), module))
        .collect::<BTreeMap<_, _>>();
    let entrypoint_path = canonical_provider_source_path(&entrypoint.file_path);
    if !modules_by_path.contains_key(&entrypoint_path) {
        return Err(CliError::failure(
            "unprojected provider graph does not contain its library entrypoint",
        ));
    }

    let mut requirements = BTreeMap::new();
    insert_provider_feature_predicate(&mut requirements, entrypoint.path_segments.clone(), BTreeSet::new());
    let mut pending = vec![(entrypoint_path, BTreeSet::new())];
    propagate_provider_feature_predicates(&modules_by_path, source_root, &mut requirements, &mut pending)?;

    let disconnected_modules = modules
        .iter()
        .filter(|module| !requirements.contains_key(&module.path_segments))
        .collect::<Vec<_>>();
    for module in disconnected_modules {
        insert_provider_feature_predicate(&mut requirements, module.path_segments.clone(), BTreeSet::new());
        pending.push((canonical_provider_source_path(&module.file_path), BTreeSet::new()));
    }
    propagate_provider_feature_predicates(&modules_by_path, source_root, &mut requirements, &mut pending)?;

    Ok(requirements)
}

/// Propagate inherited feature predicates through one bounded set of local provider imports.
fn propagate_provider_feature_predicates(
    modules_by_path: &BTreeMap<PathBuf, &ParsedModule>,
    source_root: &Path,
    requirements: &mut BTreeMap<Vec<String>, Vec<BTreeSet<String>>>,
    pending: &mut Vec<(PathBuf, BTreeSet<String>)>,
) -> CliResult<()> {
    while let Some((module_path, inherited_features)) = pending.pop() {
        let Some(module) = modules_by_path.get(&module_path) else {
            return Err(CliError::failure(format!(
                "unprojected provider graph lost module {}",
                module_path.display()
            )));
        };
        let base_dir = module.file_path.parent().unwrap_or(source_root);
        for declaration in &module.ast.declarations {
            let Declaration::Import(import) = &declaration.node else {
                continue;
            };
            let target = match resolve_source_module_import_from_source_file(
                base_dir,
                Some(source_root),
                Some(&module.file_path),
                import,
            ) {
                SourceModuleImportResolution::Local(target) => target,
                SourceModuleImportResolution::SelfImport {
                    module_ref,
                    import_path,
                    can_use_root_import,
                } => {
                    return Err(CliError::failure(self_import_diagnostic_message(
                        &module_ref,
                        &import_path,
                        can_use_root_import,
                    )));
                }
                SourceModuleImportResolution::Stdlib { .. } | SourceModuleImportResolution::External => continue,
            };
            let target_path = canonical_provider_source_path(&target.file_path);
            let Some(target_module) = modules_by_path.get(&target_path) else {
                return Err(CliError::failure(format!(
                    "unprojected provider graph is missing imported module `{}` at {}",
                    target.path_segments.join("."),
                    target.file_path.display()
                )));
            };
            let mut required_features = inherited_features.clone();
            required_features.extend(declaration.required_features.iter().cloned());
            if insert_provider_feature_predicate(
                requirements,
                target_module.path_segments.clone(),
                required_features.clone(),
            ) {
                pending.push((target_path, required_features));
            }
        }
    }

    Ok(())
}

/// Canonicalize source identity when possible while retaining useful fixture paths when it is not.
fn canonical_provider_source_path(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Insert one predicate into a deterministic minimal antichain for a provider module.
fn insert_provider_feature_predicate(
    requirements: &mut BTreeMap<Vec<String>, Vec<BTreeSet<String>>>,
    module_path: Vec<String>,
    candidate: BTreeSet<String>,
) -> bool {
    let predicates = requirements.entry(module_path).or_default();
    if predicates.iter().any(|existing| existing.is_subset(&candidate)) {
        return false;
    }
    predicates.retain(|existing| !candidate.is_subset(existing));
    predicates.push(candidate);
    predicates.sort();
    true
}

/// Preserve positive feature predicates on checked declarations for inspection and artifact projection.
fn provider_fact_requirements(
    module: &ParsedModule,
    module_requirements: &[BTreeSet<String>],
) -> Vec<ProviderFactRequirement> {
    let module_name = module.path_segments.join(".");
    let mut requirements = Vec::new();
    for declaration in &module.ast.declarations {
        let mut combined_requirements = Vec::new();
        for module_requirement in module_requirements {
            let mut combined = module_requirement.clone();
            combined.extend(declaration.required_features.iter().cloned());
            if !combined.is_empty()
                && !combined_requirements
                    .iter()
                    .any(|existing: &BTreeSet<String>| existing.is_subset(&combined))
            {
                combined_requirements.retain(|existing| !combined.is_subset(existing));
                combined_requirements.push(combined);
            }
        }
        combined_requirements.sort();

        for required_features in combined_requirements {
            match &declaration.node {
                Declaration::Import(import) => {
                    requirements.push(ProviderFactRequirement {
                        kind: ProviderFactKind::ProviderDependency,
                        identity: format!("{module_name}::{}", provider_import_identity(&import.kind)),
                        required_features: required_features.clone(),
                    });
                    if import.visibility == Visibility::Public {
                        let reexported_items = match &import.kind {
                            ImportKind::From { items, .. } | ImportKind::PubFrom { items, .. } => items.as_slice(),
                            _ => &[],
                        };
                        requirements.extend(reexported_items.iter().map(|item| ProviderFactRequirement {
                            kind: ProviderFactKind::Export,
                            identity: format!("{module_name}::{}", item.alias.as_deref().unwrap_or(item.name.as_str())),
                            required_features: required_features.clone(),
                        }));
                    }
                }
                Declaration::Docstring(_) => requirements.push(ProviderFactRequirement {
                    kind: ProviderFactKind::Documentation,
                    identity: format!("{module_name}::module-docstring"),
                    required_features,
                }),
                Declaration::TestModule(test_module) => {
                    requirements.push(ProviderFactRequirement {
                        kind: ProviderFactKind::Export,
                        identity: format!("{module_name}::{}", test_module.name),
                        required_features: required_features.clone(),
                    });
                    requirements.extend(provider_nested_test_fact_requirements(
                        &module_name,
                        &test_module.body,
                        &required_features,
                    ));
                }
                declaration => {
                    let Some(name) = provider_declaration_name(declaration) else {
                        continue;
                    };
                    let identity = format!("{module_name}::{name}");
                    requirements.push(ProviderFactRequirement {
                        kind: if provider_declaration_is_public(declaration) {
                            ProviderFactKind::Export
                        } else {
                            ProviderFactKind::ImplementationFacet
                        },
                        identity: identity.clone(),
                        required_features: required_features.clone(),
                    });
                    if provider_declaration_has_docstring(declaration) {
                        requirements.push(ProviderFactRequirement {
                            kind: ProviderFactKind::Documentation,
                            identity: identity.clone(),
                            required_features: required_features.clone(),
                        });
                    }
                    if provider_declaration_is_registry_entry(declaration) {
                        requirements.push(ProviderFactRequirement {
                            kind: ProviderFactKind::RegistryEntry,
                            identity,
                            required_features,
                        });
                    }
                }
            }
        }
    }
    requirements
}

/// Preserve nested inline-test predicates together with their enclosing test-module predicate.
fn provider_nested_test_fact_requirements(
    module_name: &str,
    declarations: &[Spanned<Declaration>],
    parent_features: &BTreeSet<String>,
) -> Vec<ProviderFactRequirement> {
    declarations
        .iter()
        .filter_map(|declaration| {
            let name = provider_declaration_name(&declaration.node)?;
            let mut required_features = parent_features.clone();
            required_features.extend(declaration.required_features.iter().cloned());
            Some(ProviderFactRequirement {
                kind: ProviderFactKind::ImplementationFacet,
                identity: format!("{module_name}::tests::{name}"),
                required_features,
            })
        })
        .collect()
}

/// Render a stable provider-local import identity without depending on source offsets.
fn provider_import_identity(import: &ImportKind) -> String {
    match import {
        ImportKind::Module(path) => format!("import:{}", path.segments.join(".")),
        ImportKind::From { module, .. } => format!("from:{}", module.segments.join(".")),
        ImportKind::PubLibrary { library, path } => {
            format!("import:pub::{library}{}", format_pub_module_suffix(path))
        }
        ImportKind::PubFrom { library, path, .. } => {
            format!("from:pub::{library}{}", format_pub_module_suffix(path))
        }
        ImportKind::Python(module) => format!("import:python:{module}"),
        ImportKind::RustCrate { crate_name, path, .. } => {
            format!("import:rust::{crate_name}::{}", path.join("::"))
        }
        ImportKind::RustFrom { crate_name, path, .. } => {
            format!("from:rust::{crate_name}::{}", path.join("::"))
        }
    }
}

/// Render a nested public-package module path for stable provider import identity.
fn format_pub_module_suffix(path: &[String]) -> String {
    path.iter().map(|segment| format!(".{segment}")).collect()
}

/// Return one declaration's stable local name.
fn provider_declaration_name(declaration: &Declaration) -> Option<&str> {
    match declaration {
        Declaration::Const(item) => Some(&item.name),
        Declaration::Static(item) => Some(&item.name),
        Declaration::Model(item) => Some(&item.name),
        Declaration::Class(item) => Some(&item.name),
        Declaration::Trait(item) => Some(&item.name),
        Declaration::Alias(item) => Some(&item.name),
        Declaration::Partial(item) => Some(&item.name),
        Declaration::TypeAlias(item) => Some(&item.name),
        Declaration::Newtype(item) => Some(&item.name),
        Declaration::Enum(item) => Some(&item.name),
        Declaration::Function(item) => Some(&item.name),
        Declaration::TestModule(item) => Some(&item.name),
        Declaration::Capability(item) => Some(&item.name),
        Declaration::Import(_) | Declaration::VocabBlock(_) | Declaration::Docstring(_) => None,
    }
}

/// Return whether one declaration contributes to the package's public checked surface.
fn provider_declaration_is_public(declaration: &Declaration) -> bool {
    let visibility = match declaration {
        Declaration::Capability(item) => item.visibility,
        Declaration::Const(item) => item.visibility,
        Declaration::Static(item) => item.visibility,
        Declaration::Model(item) => item.visibility,
        Declaration::Class(item) => item.visibility,
        Declaration::Trait(item) => item.visibility,
        Declaration::Alias(item) => item.visibility,
        Declaration::Partial(item) => item.visibility,
        Declaration::TypeAlias(item) => item.visibility,
        Declaration::Newtype(item) => item.visibility,
        Declaration::Enum(item) => item.visibility,
        Declaration::Function(item) => item.visibility,
        Declaration::Import(item) => item.visibility,
        Declaration::TestModule(_) | Declaration::VocabBlock(_) | Declaration::Docstring(_) => Visibility::Private,
    };
    matches!(visibility, Visibility::Public)
}

/// Return whether the declaration owns checked source documentation.
fn provider_declaration_has_docstring(declaration: &Declaration) -> bool {
    match declaration {
        Declaration::Function(item) => item.body.first().is_some_and(|statement| {
            matches!(
                &statement.node,
                Statement::Expr(expression)
                    if matches!(&expression.node, Expr::Literal(Literal::String(_)))
            )
        }),
        Declaration::Model(item) => item.docstring.is_some(),
        Declaration::Class(item) => item.docstring.is_some(),
        Declaration::Trait(item) => item.docstring.is_some(),
        Declaration::Newtype(item) => item.docstring.is_some(),
        Declaration::Enum(item) => item.docstring.is_some(),
        _ => false,
    }
}

/// Return whether the declaration is a checked `std.registry` entry described by `@describe`.
fn provider_declaration_is_registry_entry(declaration: &Declaration) -> bool {
    let decorators = match declaration {
        Declaration::Model(item) => &item.decorators,
        Declaration::Class(item) => &item.decorators,
        Declaration::Trait(item) => &item.decorators,
        Declaration::Newtype(item) => &item.decorators,
        Declaration::Enum(item) => &item.decorators,
        Declaration::Function(item) => &item.decorators,
        _ => return false,
    };
    decorators.iter().any(|decorator| decorator.node.name == "describe")
}

/// Write the `.incnlib` manifest and build-report artifact paths for a prepared library project.
fn write_library_manifest_artifacts(prepared: &mut PreparedLibraryProject) -> CliResult<()> {
    prepared
        .library_manifest
        .write_to_path(&prepared.manifest_path)
        .map_err(|err| CliError::failure(format!("failed to write {}: {err}", prepared.manifest_path.display())))?;

    prepared
        .report
        .artifacts
        .push(artifact_report("incan_library_manifest", &prepared.manifest_path));
    prepared.report.artifacts.push(artifact_report(
        "generated_cargo_manifest",
        &prepared.generator.cargo_manifest_path(),
    ));
    Ok(())
}

/// Return the package-owned bounded Oven store that carries this public library's project Loafs.
fn packaged_library_loaf_store_root(artifact_root: &Path) -> PathBuf {
    artifact_root.join(OVEN_PACKAGED_LIBRARY_LOAF_STORE_RELATIVE_PATH)
}

/// Return the package-owned Loaf index retained beside one generated public library artifact.
fn packaged_library_loaf_manifest_path(artifact_root: &Path) -> PathBuf {
    artifact_root.join(OVEN_PACKAGED_LIBRARY_LOAF_MANIFEST_RELATIVE_PATH)
}

/// Command-local memo for exact project source-authority nodes.
///
/// One explicit project bake can prepare several targets and profiles that all reach the same provider roots. The
/// memo avoids walking those authored trees again within that command; it is never stored globally or carried into a
/// later command. Callers that need a final publication check deliberately create a fresh digester instead.
#[derive(Default)]
struct ProjectSourceAuthorityDigester {
    project_digests: HashMap<PathBuf, String>,
    rust_crate_digests: HashMap<PathBuf, String>,
    rust_source_closure_digests: BTreeMap<PathBuf, String>,
    #[cfg(test)]
    project_scan_counts: HashMap<PathBuf, usize>,
}

impl ProjectSourceAuthorityDigester {
    /// Digest the exact build-input graph for one project without observing generated or unrelated files.
    fn digest(&mut self, project_root: &Path) -> CliResult<String> {
        self.digest_project_node(project_root, &mut HashSet::new())
    }

    /// Return how many cache-miss project-tree scans this digester performed for one canonical root.
    #[cfg(test)]
    fn project_scan_count(&self, project_root: &Path) -> usize {
        fs::canonicalize(project_root)
            .ok()
            .and_then(|root| self.project_scan_counts.get(&root).copied())
            .unwrap_or_default()
    }

    /// Record one cache-miss project-tree scan for this canonical root.
    #[cfg(test)]
    fn record_project_scan(&mut self, canonical_root: &Path) {
        *self
            .project_scan_counts
            .entry(canonical_root.to_path_buf())
            .or_default() += 1;
    }

    /// Ignore cache-miss scan accounting outside tests.
    #[cfg(not(test))]
    fn record_project_scan(&mut self, _canonical_root: &Path) {}

    /// Digest one local Rust package together with only the Cargo-workspace facts that it actually inherits.
    fn digest_rust_path_crate_authority(
        package_root: &Path,
        rust_source_closure_digests: &mut BTreeMap<PathBuf, String>,
    ) -> CliResult<String> {
        let source_tree =
            digest_cargo_path_source_tree_with_cache(package_root, rust_source_closure_digests).map_err(|error| {
                CliError::failure(format!(
                    "Oven Alpha cannot digest Rust path dependency source authority at {}: {error}",
                    package_root.display()
                ))
            })?;
        let mut records = BTreeMap::from([("package-source-tree", source_tree)]);
        if let Some(workspace_authority) = digest_local_cargo_workspace_authority(package_root).map_err(|error| {
            CliError::failure(format!(
                "Oven Alpha cannot resolve Rust path dependency workspace authority at {}: {error}",
                package_root.display()
            ))
        })? {
            records.insert("inherited-cargo-workspace", workspace_authority);
        }
        let payload = serde_json::to_vec(&records).map_err(|error| {
            CliError::failure(format!(
                "failed to serialize Rust path dependency source authority at {}: {error}",
                package_root.display()
            ))
        })?;
        Ok(digest_bytes(&payload))
    }

    /// Digest one reachable Incan node and its named dependency edges.
    fn digest_project_node(&mut self, root: &Path, visiting: &mut HashSet<PathBuf>) -> CliResult<String> {
        let canonical_root = fs::canonicalize(root).map_err(|error| {
            CliError::failure(format!(
                "Oven Alpha cannot resolve project source authority at {}: {error}",
                root.display()
            ))
        })?;
        if let Some(digest) = self.project_digests.get(&canonical_root) {
            return Ok(digest.clone());
        }
        if !visiting.insert(canonical_root.clone()) {
            return Err(CliError::failure(format!(
                "Oven Alpha project source authority contains a cyclic path dependency at {}",
                canonical_root.display()
            )));
        }
        self.record_project_scan(&canonical_root);
        let manifest = effective_project_manifest_for_exact_root(&canonical_root)?;
        let mut records = BTreeMap::from([
            (
                "project-build-inputs".to_string(),
                digest_baked_project_build_tree(&canonical_root, &manifest)?,
            ),
            (
                "project-dependency-selections".to_string(),
                digest_baked_project_dependency_selections(&manifest)?,
            ),
        ]);
        let mut library_dependencies = manifest.library_dependencies().iter().collect::<Vec<_>>();
        library_dependencies.sort_by_key(|(name, _)| *name);
        for (name, dependency) in library_dependencies {
            let child_manifest = dependency.path.join(MANIFEST_FILENAME);
            let child_digest = match fs::symlink_metadata(&child_manifest) {
                Ok(_) => self.digest_project_node(&dependency.path, visiting)?,
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    let artifact_root = dependency.path.join("target/lib");
                    let entry = load_provider_dependency_artifact(name, &artifact_root);
                    let artifact = match entry {
                        LibraryManifestIndexEntry::Loaded { metadata, .. }
                            if metadata.kind == LibraryArtifactKind::Materialized =>
                        {
                            metadata
                        }
                        LibraryManifestIndexEntry::Loaded { .. } => {
                            return Err(CliError::failure(format!(
                                "Oven Alpha cannot bind source-free pub::{name}: its checked package artifact is not materialized"
                            )));
                        }
                        LibraryManifestIndexEntry::Failed(failure) => {
                            return Err(CliError::failure(format!(
                                "Oven Alpha cannot bind source-free pub::{name} at {}: {failure}",
                                dependency.path.display()
                            )));
                        }
                    };
                    let package = read_packaged_library_loaf_manifest(&artifact)?.ok_or_else(|| {
                        CliError::failure(format!(
                            "Oven Alpha cannot bind source-free pub::{name} at {} without its sealed package Loaf; rebake or reinstall that provider",
                            dependency.path.display()
                        ))
                    })?;
                    package.source_authority_digest
                }
                Err(error) => {
                    return Err(CliError::failure(format!(
                        "Oven Alpha cannot inspect the source manifest for pub::{name} at {}: {error}",
                        child_manifest.display()
                    )));
                }
            };
            records.insert(format!("incan-dependency:{name}"), child_digest);
        }

        let mut rust_path_dependencies =
            manifest
                .rust_dependencies()
                .iter()
                .filter_map(|(name, dependency)| match &dependency.source {
                    DependencySource::Path { path } => Some(("normal", name, path)),
                    DependencySource::Registry | DependencySource::Git { .. } => None,
                })
                .chain(manifest.rust_dev_dependencies().iter().filter_map(
                    |(name, dependency)| match &dependency.source {
                        DependencySource::Path { path } => Some(("dev", name, path)),
                        DependencySource::Registry | DependencySource::Git { .. } => None,
                    },
                ))
                .collect::<Vec<_>>();
        rust_path_dependencies.sort_by(|left, right| (left.0, left.1).cmp(&(right.0, right.1)));
        for (kind, name, dependency) in rust_path_dependencies {
            let canonical_dependency = fs::canonicalize(dependency).map_err(|error| {
                CliError::failure(format!(
                    "Oven Alpha cannot resolve Rust path dependency source authority at {}: {error}",
                    dependency.display()
                ))
            })?;
            let child_digest = if let Some(digest) = self.rust_crate_digests.get(&canonical_dependency) {
                digest.clone()
            } else {
                let digest = Self::digest_rust_path_crate_authority(
                    &canonical_dependency,
                    &mut self.rust_source_closure_digests,
                )?;
                self.rust_crate_digests.insert(canonical_dependency, digest.clone());
                digest
            };
            records.insert(format!("rust-{kind}-dependency:{name}"), child_digest);
        }

        let payload = serde_json::to_vec(&records).map_err(|error| {
            CliError::failure(format!("failed to serialize Oven project source authority: {error}"))
        })?;
        let digest = digest_bytes(&payload);
        visiting.remove(&canonical_root);
        self.project_digests.insert(canonical_root, digest.clone());
        Ok(digest)
    }
}

/// Digest the exact build-input graph for a completed project output without observing generated or unrelated files.
///
/// Each ordinary call owns a fresh memo, preserving the existing behavior outside explicit project bake orchestration.
fn digest_baked_project_source_authority(project_root: &Path) -> CliResult<String> {
    ProjectSourceAuthorityDigester::default().digest(project_root)
}

/// Digest one effective project's portable dependency selections without walking dependency source twice.
///
/// Named graph edges below bind path dependencies to their authored content. This record binds the remaining
/// identity facts, including RFC 077 inherited features and registry or Git selection, while intentionally omitting
/// machine-local path spelling so equivalent relocated worktrees can reuse a completed output.
fn digest_baked_project_dependency_selections(manifest: &ProjectManifest) -> CliResult<String> {
    let mut records = BTreeMap::new();
    for (name, dependency) in manifest.library_dependencies() {
        let mut features = dependency.features.clone();
        features.sort();
        features.dedup();
        records.insert(
            format!("incan:{name}"),
            format!(
                "{}|{}|{}|{}",
                dependency.library_name,
                dependency.default_features,
                dependency.optional,
                features.join(",")
            ),
        );
    }
    for (kind, dependencies) in [
        ("normal", manifest.rust_dependencies()),
        ("dev", manifest.rust_dev_dependencies()),
    ] {
        for (name, dependency) in dependencies {
            let mut features = dependency.features.clone();
            features.sort();
            features.dedup();
            let source = match &dependency.source {
                DependencySource::Registry => "registry".to_string(),
                DependencySource::Path { .. } => "path-tree".to_string(),
                DependencySource::Git { url, reference } => match reference {
                    GitReference::Branch(branch) => format!("git:{url}:branch:{branch}"),
                    GitReference::Tag(tag) => format!("git:{url}:tag:{tag}"),
                    GitReference::Rev(revision) => format!("git:{url}:rev:{revision}"),
                },
            };
            records.insert(
                format!("rust-{kind}:{name}"),
                format!(
                    "{}|{}|{}|{}|{}|{}|{}",
                    dependency.crate_name,
                    dependency.package.as_deref().unwrap_or(""),
                    dependency.version.as_deref().unwrap_or(""),
                    dependency.default_features,
                    dependency.optional,
                    features.join(","),
                    source
                ),
            );
        }
    }
    let payload = serde_json::to_vec(&records).map_err(|error| {
        CliError::failure(format!(
            "failed to serialize Oven project dependency selections: {error}"
        ))
    })?;
    Ok(digest_bytes(&payload))
}

/// Resolve the one canonical semantic lock that governs a project bake.
///
/// Workspace members share the root RFC 077 lock. Reading a member-local file instead would let a stale side file
/// hide changes to the real workspace authority, or make equivalent members disagree about the lock they consumed.
fn canonical_baked_project_lock_path(project_root: &Path) -> CliResult<PathBuf> {
    let workspace = crate::workspace::WorkspaceGraph::discover(project_root)
        .map_err(|error| CliError::failure(format!("failed to resolve Oven project workspace: {error}")))?;
    Ok(workspace
        .map(|workspace| workspace.root().join("incan.lock"))
        .unwrap_or_else(|| project_root.join("incan.lock")))
}

/// Load the derived dependency fingerprint from the canonical project or workspace lock, when present.
fn baked_project_lock_dependencies_fingerprint(project_root: &Path) -> CliResult<Option<String>> {
    let lock_path = canonical_baked_project_lock_path(project_root)?;
    if !lock_path.is_file() {
        return Ok(None);
    }
    IncanLock::load(&lock_path)
        .map(|lock| Some(lock.deps_fingerprint))
        .map_err(|error| CliError::failure(error.to_string()))
}

/// Hash the canonical lock fields that can change build meaning while excluding derived and structural fields.
///
/// Lock format 1 and 2 decode into the same semantic authority projection. An explicit bake is allowed to migrate
/// that representation, so recording the format here would make the publisher reject the state it just wrote.
fn digest_baked_project_lock_authority(lock_path: &Path) -> CliResult<String> {
    let lock = IncanLock::load(lock_path).map_err(|error| CliError::failure(error.to_string()))?;
    let mut semantic = lock.semantic;
    // A bake refreshes compiler-owned SDK identity records to the active release cohort. Those records are already
    // bound by the compiler/runtime receipt inputs, so treating them as authored project authority would make the
    // publisher reject its own lock refresh. Package, feature, custom-provider, Oven, workspace, and Cargo-lock
    // selections remain part of this lock authority.
    semantic.sdk = None;
    semantic
        .providers
        .retain(|provider| !provider.identity.starts_with("incan_stdlib_"));
    for member in &mut semantic.workspace_members {
        member.sdk = None;
        member
            .providers
            .retain(|provider| !provider.identity.starts_with("incan_stdlib_"));
    }
    let projection = serde_json::json!({
        "cargo_features": lock.cargo_features,
        "semantic": semantic,
        "cargo_lock_payload": lock.cargo_lock_payload,
    });
    serde_json::to_vec(&projection)
        .map(|bytes| digest_bytes(&bytes))
        .map_err(|error| CliError::failure(format!("failed to serialize canonical Oven lock authority: {error}")))
}

/// Hash only the declared Incan project inputs that can affect a normal build.
fn digest_baked_project_build_tree(project_root: &Path, manifest: &ProjectManifest) -> CliResult<String> {
    /// Record one regular authority file under a caller-selected portable key.
    fn append_named_file(path: &Path, record_key: String, records: &mut BTreeMap<String, String>) -> CliResult<()> {
        let metadata = fs::symlink_metadata(path).map_err(|error| {
            CliError::failure(format!(
                "Oven Alpha cannot read project build authority at {}: {error}",
                path.display()
            ))
        })?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(CliError::failure(format!(
                "Oven Alpha project build authority must use regular files, found {}",
                path.display()
            )));
        }
        let digest = digest_bytes(&fs::read(path).map_err(|error| {
            CliError::failure(format!(
                "Oven Alpha cannot hash project build authority at {}: {error}",
                path.display()
            ))
        })?);
        if records.insert(record_key.clone(), digest).is_some() {
            return Err(CliError::failure(format!(
                "Oven Alpha project build authority contains duplicate path `{record_key}`"
            )));
        }
        Ok(())
    }

    /// Record one regular project input by its portable path and content digest.
    fn append_file(root: &Path, path: &Path, records: &mut BTreeMap<String, String>) -> CliResult<()> {
        let relative = path
            .strip_prefix(root)
            .map_err(|_| CliError::failure(format!("project build authority escaped {}", root.display())))?
            .to_string_lossy()
            .replace('\\', "/");
        append_named_file(path, relative, records)
    }

    /// Traverse a regular source directory deterministically and record its input files.
    ///
    /// `already_recorded` names files this traversal must not re-record: the manifest and lock file
    /// are always recorded separately above under their own dedicated (and, for the lock, semantically
    /// filtered) digest, but a flat-layout project whose source root is the project root itself would
    /// otherwise walk straight over them here too, producing a spurious duplicate-path failure.
    fn collect_directory(
        root: &Path,
        directory: &Path,
        records: &mut BTreeMap<String, String>,
        already_recorded: &HashSet<PathBuf>,
    ) -> CliResult<()> {
        let mut entries = fs::read_dir(directory)
            .map_err(|error| {
                CliError::failure(format!(
                    "Oven Alpha cannot read project build authority at {}: {error}",
                    directory.display()
                ))
            })?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| {
                CliError::failure(format!(
                    "Oven Alpha cannot read project build authority at {}: {error}",
                    directory.display()
                ))
            })?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path).map_err(|error| {
                CliError::failure(format!(
                    "Oven Alpha cannot read project build authority at {}: {error}",
                    path.display()
                ))
            })?;
            if metadata.file_type().is_symlink() {
                return Err(CliError::failure(format!(
                    "Oven Alpha project build authority does not allow symlinks: {}",
                    path.display()
                )));
            }
            if metadata.is_dir() {
                // Tool-owned output directories are never part of the build authority. A project whose
                // source root is the project root itself (no dedicated `src/`) would otherwise scan its
                // own `.incan`/`target` output back into the digest, making an explicit bake refuse to
                // publish because the authority it just computed changed while writing that same output.
                if matches!(
                    path.file_name().and_then(|name| name.to_str()),
                    Some(".git" | ".incan" | ".ralph-cache" | "target")
                ) {
                    continue;
                }
                collect_directory(root, &path, records, already_recorded)?;
            } else if !already_recorded.contains(&path) {
                append_file(root, &path, records)?;
            }
        }
        Ok(())
    }

    let mut records = BTreeMap::new();
    let mut already_recorded = HashSet::new();
    let manifest_path = project_root.join(MANIFEST_FILENAME);
    append_file(project_root, &manifest_path, &mut records)?;
    already_recorded.insert(manifest_path);
    let lockfile = canonical_baked_project_lock_path(project_root)?;
    if lockfile.is_file() {
        records.insert(
            "incan.lock".to_string(),
            digest_baked_project_lock_authority(&lockfile)?,
        );
        already_recorded.insert(lockfile);
    }
    let source_root = resolve_source_root(project_root, Some(manifest));
    let source_metadata = fs::symlink_metadata(&source_root).map_err(|error| {
        CliError::failure(format!(
            "Oven Alpha project build authority requires {}: {error}",
            source_root.display()
        ))
    })?;
    if source_metadata.file_type().is_symlink() || !source_metadata.is_dir() {
        return Err(CliError::failure(format!(
            "Oven Alpha project build authority requires a regular source directory at {}",
            source_root.display()
        )));
    }
    let canonical_project_root = fs::canonicalize(project_root).map_err(|error| {
        CliError::failure(format!(
            "Oven Alpha cannot resolve project build authority root {}: {error}",
            project_root.display()
        ))
    })?;
    let canonical_source_root = fs::canonicalize(&source_root).map_err(|error| {
        CliError::failure(format!(
            "Oven Alpha cannot resolve project source authority at {}: {error}",
            source_root.display()
        ))
    })?;
    if canonical_source_root.starts_with(&canonical_project_root) {
        collect_directory(project_root, &source_root, &mut records, &already_recorded)?;
    } else {
        records.insert(
            "configured-source-root".to_string(),
            digest_project_source_tree(&canonical_source_root).map_err(|error| {
                CliError::failure(format!(
                    "Oven Alpha cannot digest configured source authority at {}: {error}",
                    canonical_source_root.display()
                ))
            })?,
        );
    }

    for (relative, entrypoint) in discover_oven_executable_entrypoints(manifest)? {
        let canonical_entrypoint = fs::canonicalize(&entrypoint).map_err(|error| {
            CliError::failure(format!(
                "Oven Alpha cannot resolve declared executable source authority at {}: {error}",
                entrypoint.display()
            ))
        })?;
        if canonical_entrypoint.starts_with(&canonical_source_root) {
            continue;
        }
        if !canonical_entrypoint.starts_with(&canonical_project_root) {
            return Err(CliError::failure(format!(
                "Oven Alpha declared executable source authority escaped project root {}: {}",
                project_root.display(),
                entrypoint.display()
            )));
        }
        append_named_file(&entrypoint, format!("declared-executable:{relative}"), &mut records)?;
    }

    if let Some(configured_path) = manifest.vocab.as_ref().and_then(|vocab| vocab.crate_path.as_deref()) {
        let companion_root = if Path::new(configured_path).is_absolute() {
            PathBuf::from(configured_path)
        } else {
            project_root.join(configured_path)
        };
        let companion_root = fs::canonicalize(&companion_root).map_err(|error| {
            CliError::failure(format!(
                "Oven Alpha cannot resolve vocabulary companion source authority at {}: {error}",
                companion_root.display()
            ))
        })?;
        let companion_digest = digest_project_source_tree(&companion_root).map_err(|error| {
            CliError::failure(format!(
                "Oven Alpha cannot digest vocabulary companion source authority at {}: {error}",
                companion_root.display()
            ))
        })?;
        records.insert("vocab-companion".to_string(), companion_digest);
    }

    for (index, configured_path) in manifest.contract_model_bundle_paths().iter().enumerate() {
        let path = if Path::new(configured_path).is_absolute() {
            PathBuf::from(configured_path)
        } else {
            project_root.join(configured_path)
        };
        append_named_file(&path, format!("contract-model-bundle:{index}"), &mut records)?;
    }

    let locked_interop_targets = locked_oven_interop_targets(manifest).map_err(|error| {
        CliError::failure(format!(
            "Oven Alpha cannot lock declared interop inputs for project source authority: {error}"
        ))
    })?;
    for target in locked_interop_targets {
        let locked_payload = serde_json::to_vec(&target).map_err(|error| {
            CliError::failure(format!(
                "failed to serialize locked Oven interop target `{}`: {error}",
                target.target
            ))
        })?;
        records.insert(
            format!("oven-interop-locked-target:{}", target.target),
            digest_bytes(&locked_payload),
        );

        let receipt_path = default_interop_execution_receipt_path(project_root, &target.target);
        let receipt_metadata = match fs::symlink_metadata(&receipt_path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(CliError::failure(format!(
                    "Oven Alpha cannot inspect selected interop receipt {}: {error}",
                    receipt_path.display()
                )));
            }
        };
        if receipt_metadata.file_type().is_symlink() || !receipt_metadata.is_file() {
            return Err(CliError::failure(format!(
                "Oven Alpha selected interop receipt must be a regular file: {}",
                receipt_path.display()
            )));
        }
        let receipt = load_interop_execution_receipt(&receipt_path).map_err(CliError::failure)?;
        validate_interop_execution_receipt(&target, &receipt).map_err(|error| {
            CliError::failure(format!(
                "Oven Alpha selected interop receipt {} is stale: {error}",
                receipt_path.display()
            ))
        })?;
        records.insert(
            format!("oven-interop-execution-receipt:{}", target.target),
            receipt.identity,
        );
    }

    let payload = serde_json::to_vec(&records)
        .map_err(|error| CliError::failure(format!("failed to serialize Oven project build authority: {error}")))?;
    Ok(digest_bytes(&payload))
}

/// Return the durable receipt path for one explicitly baked target/profile.
///
/// Conventional library and main targets retain their historical paths. Other scripts use the complete digest of
/// their portable target identity, so two declared executables can never overwrite one another's lineage.
fn project_bake_receipt_path(
    project_root: &Path,
    target: OvenBakeProjectTarget,
    entrypoint: &Path,
    profile: &str,
) -> CliResult<PathBuf> {
    let target_identity = oven_bake_project_target_identity(project_root, target, entrypoint)?;
    let file_name = if target_identity == target.as_str() {
        format!("{}-{profile}-receipt.json", target.as_str())
    } else {
        format!(
            "executable-{}-{profile}-receipt.json",
            digest_bytes(target_identity.as_bytes()).trim_start_matches("sha256:")
        )
    };
    Ok(crate::oven::default_receipt_path(project_root).with_file_name(file_name))
}

/// Return the private pre-interop receipt path for one Rust target, entrypoint, and profile.
///
/// This never shares the ordinary explicit-bake receipt namespace: the automatic bootstrap deliberately excludes
/// publisher-only development dependencies and is valid only as the base later extended by a selected native plan.
fn interop_bootstrap_receipt_path(
    project_root: &Path,
    rust_target: &str,
    target: OvenBakeProjectTarget,
    entrypoint: &Path,
    profile: &str,
) -> CliResult<PathBuf> {
    let target_identity = oven_bake_project_target_identity(project_root, target, entrypoint)?;
    let identity = digest_bytes(format!("{rust_target}\0{target_identity}").as_bytes())
        .trim_start_matches("sha256:")
        .to_string();
    Ok(crate::oven::default_receipt_path(project_root)
        .with_file_name(format!("interop-bootstrap-{identity}-{profile}-receipt.json")))
}

/// Return the durable receipt path appropriate to one prepared Oven command mode.
fn prepared_oven_receipt_path(
    project_root: &Path,
    mode: OvenProjectPlanMode,
    rust_target: &str,
    entrypoint: &Path,
    profile: &str,
) -> CliResult<PathBuf> {
    if mode == OvenProjectPlanMode::InteropBootstrap {
        interop_bootstrap_receipt_path(
            project_root,
            rust_target,
            OvenBakeProjectTarget::Executable,
            entrypoint,
            profile,
        )
    } else {
        Ok(crate::oven::default_receipt_path(project_root))
    }
}

/// Restore and validate the portable package handoff carried by reused library outputs.
fn restore_reused_library_package(
    project_root: &Path,
    store: &OvenStore,
    source_authority_digest: &str,
    outputs: &[&OvenStoredProjectOutput],
) -> CliResult<bool> {
    if outputs.is_empty() {
        return Ok(true);
    }
    let artifact_root = project_root.join("target/lib");
    let manifest_path = packaged_library_loaf_manifest_path(&artifact_root);
    let bytes = match fs::read(&manifest_path) {
        Ok(bytes) => bytes,
        Err(_) => return Ok(false),
    };
    let manifest = match serde_json::from_slice::<OvenPackagedLibraryLoafManifest>(&bytes) {
        Ok(manifest)
            if manifest.schema_version == OVEN_PACKAGED_LIBRARY_LOAF_SCHEMA_VERSION
                && manifest.source_authority_digest == source_authority_digest
                && manifest.compiler_version == INCAN_VERSION =>
        {
            manifest
        }
        Ok(_) | Err(_) => return Ok(false),
    };
    if manifest.profiles.len() != outputs.len() {
        return Ok(false);
    }
    let package_store = OvenStore::new(packaged_library_loaf_store_root(&artifact_root), *store.limits());
    for output in outputs {
        let Some(candidate) = manifest.profiles.get(&output.profile) else {
            return Ok(false);
        };
        if candidate.receipt.verify_identity().is_err()
            || candidate.receipt.identity != output.payload.receipt_identity
            || candidate.receipt.build_unit_identity != output.payload.build_unit_identity
            || candidate.receipt.intent != output.intent
            || candidate
                .receipt
                .sources
                .build_unit_inputs
                .get("compiler-version")
                .is_none_or(|version| version != INCAN_VERSION)
        {
            return Ok(false);
        }
        let Some(native) = output
            .payload
            .files
            .iter()
            .find(|file| file.output_relative_path == OVEN_PROJECT_OUTPUT_ARTIFACT_PATH)
        else {
            return Ok(false);
        };
        let expected_library_relative_path = match Path::new(&native.caller_relative_path).strip_prefix("target/lib") {
            Ok(path) => path.to_string_lossy().replace('\\', "/"),
            Err(_) => return Ok(false),
        };
        if validated_project_output_relative_path(&candidate.library_relative_path, "package library").is_err()
            || candidate.library_relative_path != expected_library_relative_path
            || candidate.library_digest != native.digest
            || output.payload.package_loaf_store_relative_path.as_deref() != Some("target/lib/oven/loafs")
        {
            return Ok(false);
        }
        let mut candidate_entries = candidate.entries.clone();
        candidate_entries.sort_by(|left, right| {
            (&left.identity, &left.receipt.identity).cmp(&(&right.identity, &right.receipt.identity))
        });
        let mut output_entries = output.payload.required_project_loafs.clone();
        output_entries.sort_by(|left, right| {
            (&left.identity, &left.receipt.identity).cmp(&(&right.identity, &right.receipt.identity))
        });
        if candidate_entries != output_entries {
            return Ok(false);
        }
        if candidate.entries.is_empty()
            && resolve_compiler_owned_loaf_for_registry_dependencies(&candidate.receipt, &[])
                .map_or(true, |selected| selected.is_none())
        {
            return Ok(false);
        }
        for entry in &candidate.entries {
            if entry.receipt.verify_identity().is_err() || entry.receipt.intent != candidate.receipt.intent {
                return Ok(false);
            }
            if let Some(base_loaf_identity) = entry.base_loaf_identity.as_deref()
                && resolve_compiler_owned_loaf_by_identity(&candidate.receipt, base_loaf_identity)
                    .map_or(true, |selected| selected.is_none())
            {
                return Ok(false);
            }
            let packaged = package_store.select_payloads_matching_for_execution(|stored| {
                stored.identity == entry.identity
                    && stored.kind == entry.kind
                    && stored.receipt_identity == entry.receipt.identity
                    && stored.build_unit_identity == entry.receipt.build_unit_identity
                    && stored.intent == entry.receipt.intent
            });
            match packaged {
                Ok(selected) if selected.len() == 1 => {}
                Ok(selected) if selected.is_empty() => {
                    let source = store.select_payloads_matching_for_execution(|stored| {
                        stored.identity == entry.identity
                            && stored.kind == entry.kind
                            && stored.receipt_identity == entry.receipt.identity
                            && stored.build_unit_identity == entry.receipt.build_unit_identity
                            && stored.intent == entry.receipt.intent
                    });
                    if source.map_or(true, |selected| selected.len() != 1) {
                        return Ok(false);
                    }
                    let copied = copy_receipted_oven_store_entry(
                        store,
                        &package_store,
                        &entry.receipt,
                        &entry.identity,
                        entry.kind,
                        "project bake package export",
                    )?;
                    if copied.identity != entry.identity || copied.kind != entry.kind {
                        return Err(CliError::failure(
                            "completed Oven library output changed identity while restoring its package Loaf",
                        ));
                    }
                }
                Ok(_) | Err(_) => return Ok(false),
            }
        }
    }
    Ok(true)
}

/// Return a previously baked project report only when every discovered target/profile remains exact.
///
/// Any stale, absent, or malformed evidence returns a cache miss so the explicit baker can repair it. Selection
/// completes before any caller projection is restored, and a full hit returns before frontend, codegen, or Rustc.
fn try_reuse_baked_project(
    project_root: &Path,
    targets: &[(OvenBakeProjectTarget, PathBuf)],
    store: &OvenStore,
    package_features: &FeatureSelection,
    authority_context: &mut OvenProjectBakeAuthorityContext,
) -> CliResult<Option<OvenProjectBakeReport>> {
    // A completed project-output payload is selected only for the default command projection. Feature-qualified project
    // outputs remain explicit bake results until their selection facts are part of the public normal command payload,
    // so never reuse the default package export for one.
    if package_features != &FeatureSelection::default() {
        return Ok(None);
    }
    let rustc = resolve_active_rustc().map_err(|error| CliError::failure(error.to_string()))?;
    let target = rustc_host_target(&rustc).map_err(|error| CliError::failure(error.to_string()))?;
    let toolchain = rustc_identity(&rustc).map_err(|error| CliError::failure(error.to_string()))?;
    let lock_dependencies_fingerprint = baked_project_lock_dependencies_fingerprint(project_root)?;
    let mut expected_outputs = Vec::new();
    for (project_target, entrypoint) in targets {
        for profile in explicit_bake_profiles() {
            let receipt_path = project_bake_receipt_path(project_root, *project_target, entrypoint, profile)?;
            let receipt = match fs::read(&receipt_path)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<crate::oven::OvenReceipt>(&bytes).ok())
            {
                Some(receipt)
                    if receipt.verify_identity().is_ok()
                        && receipt.intent.profile == profile
                        && receipt.intent.target == target
                        && receipt.intent.toolchain == toolchain =>
                {
                    receipt
                }
                Some(_) | None => return Ok(None),
            };
            expected_outputs.push((
                *project_target,
                entrypoint.clone(),
                profile.to_string(),
                receipt_path,
                receipt,
            ));
        }
    }
    let headers = store
        .manifests_for_selection()
        .map_err(|error| CliError::failure(format!("failed to inspect Oven project-output headers: {error}")))?;
    if expected_outputs.iter().any(|(_, _, _, _, receipt)| {
        !headers.iter().any(|manifest| {
            manifest.kind == OvenArtifactKind::ProjectOutput
                && manifest.receipt_identity == receipt.identity
                && manifest.build_unit_identity == receipt.build_unit_identity
                && manifest.intent == receipt.intent
        })
    }) {
        return Ok(None);
    }

    // Only an exact local receipt set with matching immutable store headers earns the recursive authored-source scan.
    // Headers are a reject-only optimization; exact payload selection below remains the execution authority.
    // A cache candidate is only tentative until every receipt, payload, and inspection authority validates below.
    // Do not preserve this pre-refresh lock projection as this command's publication authority: an explicit bake
    // may refresh an old lock after the cache probe misses, and that compiler-owned refresh is not an authored edit.
    let source_authority_digest = authority_context.cache_probe_source_authority(project_root)?;
    let mut selected_outputs = Vec::new();
    for (project_target, entrypoint, profile, receipt_path, receipt) in expected_outputs {
        let Some(output) = select_baked_project_output_with_source_authority(
            store,
            project_root,
            &entrypoint,
            project_target,
            &profile,
            &source_authority_digest,
            Some((&target, &toolchain)),
        )?
        else {
            return Ok(None);
        };
        if output.payload.lock_dependencies_fingerprint != lock_dependencies_fingerprint {
            return Ok(None);
        }
        match project_target {
            OvenBakeProjectTarget::Library
                if output.payload.package_loaf_store_relative_path.as_deref() == Some("target/lib/oven/loafs") => {}
            OvenBakeProjectTarget::Library => return Ok(None),
            OvenBakeProjectTarget::Executable
                if output.payload.package_loaf_store_relative_path.is_none()
                    && output.payload.required_project_loafs.is_empty() => {}
            OvenBakeProjectTarget::Executable => return Ok(None),
        }
        if receipt.identity != output.payload.receipt_identity
            || receipt.build_unit_identity != output.payload.build_unit_identity
            || receipt.intent != output.intent
        {
            return Ok(None);
        }
        selected_outputs.push((project_target, output, receipt_path, receipt));
    }
    let authority_ref = selected_outputs
        .first()
        .and_then(|(_, output, _, _)| output.payload.inspection_authority.as_ref())
        .cloned()
        .ok_or_else(|| CliError::failure("completed Oven project outputs have no inspection authority"))?;
    if selected_outputs
        .iter()
        .any(|(_, output, _, _)| output.payload.inspection_authority.as_ref() != Some(&authority_ref))
    {
        return Ok(None);
    }
    let authority = load_project_inspection_authority(
        store,
        &authority_ref,
        &baked_project_owner_identity(project_root)?,
        &source_authority_digest,
        INCAN_VERSION,
    )
    .map_err(|error| CliError::failure(error.to_string()))?;
    let _validated_authority = super::lock::prepare_project_registry_source_authorities(authority)?;
    for (_, output, _, _) in &selected_outputs {
        materialize_project_output(project_root, output)?;
    }
    let library_outputs = selected_outputs
        .iter()
        .filter_map(|(project_target, output, _, _)| {
            (*project_target == OvenBakeProjectTarget::Library).then_some(output)
        })
        .collect::<Vec<_>>();
    if !restore_reused_library_package(project_root, store, &source_authority_digest, &library_outputs)? {
        return Ok(None);
    }

    let mut generated_sources = BTreeMap::new();
    let mut profiles = Vec::new();
    for (project_target, output, receipt_path, _) in selected_outputs {
        let generated_relative_path = match project_target {
            OvenBakeProjectTarget::Library => "generated/src/lib.rs",
            OvenBakeProjectTarget::Executable => "generated/src/main.rs",
        };
        let Some(generated) = output
            .payload
            .files
            .iter()
            .find(|file| file.output_relative_path == generated_relative_path)
        else {
            return Ok(None);
        };
        let generated_path = caller_project_output_path(project_root, &generated.caller_relative_path)?;
        match generated_sources.entry(output.payload.target_identity.clone()) {
            std::collections::btree_map::Entry::Vacant(entry) => {
                entry.insert(generated_path);
            }
            std::collections::btree_map::Entry::Occupied(entry) if entry.get() == &generated_path => {}
            std::collections::btree_map::Entry::Occupied(_) => return Ok(None),
        }
        profiles.push(OvenProjectBakeProfileReport {
            project_target: output.payload.target_identity.clone(),
            profile: output.profile.clone(),
            target: output.intent.target.clone(),
            toolchain: output.intent.toolchain.clone(),
            receipt: receipt_path,
            receipt_identity: output.payload.receipt_identity.clone(),
            build_unit_identity: output.payload.build_unit_identity.clone(),
            plan_identity: output.payload.plan_identity.clone(),
            action: "reused",
        });
    }
    Ok(Some(OvenProjectBakeReport {
        project: project_root.to_path_buf(),
        generated_sources,
        store: store.root().to_path_buf(),
        profiles,
    }))
}

/// Copy one already selected project Loaf into the public provider artifact through normal immutable-store admission.
///
/// This is intentionally an explicit-bake operation. The source store selection retains its active lease while the
/// package store validates every file and performs its atomic publication, so a package can never point to a
/// mutable cache directory or a half-copied third-party closure.
fn export_selected_package_loaf(
    source_store: &OvenStore,
    package_store_root: &Path,
    receipt: &crate::oven::OvenReceipt,
    selection: &OvenDirectRustcPlanSelection,
) -> CliResult<Vec<OvenPackagedLibraryLoafEntry>> {
    let package_store = OvenStore::new(package_store_root, *source_store.limits());
    selection
        .package_entries(receipt)
        .into_iter()
        .map(|entry| {
            let exported = copy_receipted_oven_store_entry(
                source_store,
                &package_store,
                &entry.receipt,
                &entry.identity,
                entry.kind,
                "package export",
            )?;
            if exported.kind != entry.kind
                || exported.receipt_identity != entry.receipt.identity
                || exported.build_unit_identity != entry.receipt.build_unit_identity
                || exported.intent != entry.receipt.intent
            {
                return Err(CliError::failure(
                    "package Loaf changed its receipt-bound execution contract during immutable export",
                ));
            }
            // A shared direct plan can be byte-identical to a plan first sealed under another compatible receipt.
            // The package store must still publish that verified content under this library output's receipt, so its
            // portable entry identity is the destination publication rather than the reusable source entry.
            Ok(OvenPackagedLibraryLoafEntry {
                receipt: entry.receipt,
                identity: exported.identity,
                kind: entry.kind,
                base_loaf_identity: entry.base_loaf_identity,
            })
        })
        .collect()
}

/// Copy one selected immutable entry, or its receipt-compatible direct-plan equivalent, through destination validation.
///
/// A package Loaf is never a directory alias to another cache. The destination verifies every copied artifact and
/// calculates its own bounded admission before making the entry visible.
fn copy_receipted_oven_store_entry(
    source_store: &OvenStore,
    destination_store: &OvenStore,
    receipt: &crate::oven::OvenReceipt,
    entry_identity: &str,
    entry_kind: OvenArtifactKind,
    operation: &str,
) -> CliResult<crate::oven::store::OvenArtifactManifest> {
    let existing = destination_store
        .select_payloads_matching_for_execution(|manifest| {
            manifest.identity == entry_identity
                && manifest.kind == entry_kind
                && manifest.receipt_identity == receipt.identity
                && manifest.build_unit_identity == receipt.build_unit_identity
                && manifest.intent == receipt.intent
        })
        .map_err(|error| CliError::failure(format!("failed to inspect provider Loaf before {operation}: {error}")))?;
    if existing.len() > 1 {
        return Err(CliError::failure(format!(
            "expected at most one existing provider Loaf `{entry_identity}` before {operation}, found {}",
            existing.len()
        )));
    }
    if let Some(existing) = existing.into_iter().next() {
        // Immutable store selection validates the manifest and payload while holding an active lease. Repeating the
        // full closure hash on a warm explicit bake would turn a valid package reuse into a multi-gigabyte scan.
        return Ok(existing.manifest);
    }
    let mut selected = source_store
        .select_payloads_matching_for_execution(|manifest| {
            manifest.identity == entry_identity
                && manifest.kind == entry_kind
                && manifest.build_unit_identity == receipt.build_unit_identity
                && manifest.intent == receipt.intent
        })
        .map_err(|error| CliError::failure(format!("failed to select provider Loaf for {operation}: {error}")))?;
    if selected.is_empty() && entry_kind == OvenArtifactKind::DirectRustcPlan {
        // Direct plans are reusable by their verified build unit and intent. A semantically inert source edit can
        // change the caller receipt while preserving the exact sealed Rust closure, leaving its source-store identity
        // under the original receipt. Select that compatible closure only through the ordinary receipt-aware planner,
        // then re-publish it below with the package's current receipt.
        if let Some(compatible) =
            select_direct_rustc_plan_for_execution(source_store, receipt).map_err(oven_rustc_error)?
        {
            selected.push(compatible);
        }
    }
    if selected.len() != 1 {
        return Err(CliError::failure(format!(
            "expected one receipt-selected provider Loaf `{entry_identity}` for {operation}, found {}",
            selected.len()
        )));
    }
    let (manifest, artifact_root, payload, _lease) = selected.remove(0).into_parts();
    if manifest.kind != entry_kind
        || manifest.build_unit_identity != receipt.build_unit_identity
        || manifest.intent != receipt.intent
        || (entry_kind != OvenArtifactKind::DirectRustcPlan && manifest.receipt_identity != receipt.identity)
    {
        return Err(CliError::failure(format!(
            "selected provider Loaf `{entry_identity}` changed its receipt-bound execution contract during {operation}"
        )));
    }
    let materialized_files = manifest
        .materialized_files
        .iter()
        .map(|file| OvenArtifactMaterializedFile {
            source_path: artifact_root.join(&file.relative_path),
            relative_path: file.relative_path.clone(),
        })
        .collect::<Vec<_>>();
    let exported = destination_store
        .publish_receipt_bound(&OvenArtifactPublishRequest {
            receipt: receipt.clone(),
            domain: manifest.domain.clone(),
            kind: manifest.kind,
            payload,
            materialized_files,
        })
        .map_err(|error| CliError::failure(format!("failed to publish provider Loaf during {operation}: {error}")))?;
    if exported.kind != entry_kind
        || exported.receipt_identity != receipt.identity
        || exported.build_unit_identity != receipt.build_unit_identity
        || exported.intent != receipt.intent
    {
        return Err(CliError::failure(
            "provider Loaf changed its receipt-bound execution contract during immutable store copy",
        ));
    }
    Ok(exported)
}

/// Atomically publish the package-local index only after every referenced Loaf and library output exists.
fn write_packaged_library_loaf_manifest(
    artifact_root: &Path,
    manifest: &OvenPackagedLibraryLoafManifest,
) -> CliResult<()> {
    let path = packaged_library_loaf_manifest_path(artifact_root);
    let parent = path
        .parent()
        .ok_or_else(|| CliError::failure(format!("package Loaf manifest path has no parent: {}", path.display())))?;
    fs::create_dir_all(parent).map_err(|error| {
        CliError::failure(format!(
            "failed to create package Loaf directory {}: {error}",
            parent.display()
        ))
    })?;
    let payload = serde_json::to_vec_pretty(manifest)
        .map_err(|error| CliError::failure(format!("failed to encode package Loaf manifest: {error}")))?;
    let staged = parent.join(format!(".package-loafs-{}.tmp", std::process::id()));
    crate::oven::write_receipt_staged(&payload, &staged, &path, parent).map_err(|error| {
        CliError::failure(format!(
            "failed to publish package Loaf manifest {}: {error}",
            path.display()
        ))
    })
}

/// Decode one provider's small package-Loaf index and validate its release-level facts.
fn decode_packaged_library_loaf_manifest(
    artifact: &LibraryArtifactMetadata,
) -> CliResult<Option<(PathBuf, String, OvenPackagedLibraryLoafManifest)>> {
    let path = packaged_library_loaf_manifest_path(&artifact.crate_root);
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(CliError::failure(format!(
                "Oven Alpha cannot read package Loaf manifest for pub::{} at {}: {error}",
                artifact.dependency_key,
                path.display()
            )));
        }
    };
    let manifest = serde_json::from_slice::<OvenPackagedLibraryLoafManifest>(&bytes).map_err(|error| {
        CliError::failure(format!(
            "Oven Alpha cannot parse package Loaf manifest for pub::{} at {}: {error}",
            artifact.dependency_key,
            path.display()
        ))
    })?;
    if manifest.schema_version != OVEN_PACKAGED_LIBRARY_LOAF_SCHEMA_VERSION {
        return Err(CliError::failure(format!(
            "Oven Alpha cannot use pub::{} package Loaf manifest at {}: schema {} is unsupported; rebake the provider with this Incan release",
            artifact.dependency_key,
            path.display(),
            manifest.schema_version
        )));
    }
    if manifest.compiler_version != INCAN_VERSION {
        return Err(CliError::failure(format!(
            "Oven Alpha cannot use pub::{} package Loaf manifest at {}: it was baked by Incan {}, but this compiler is {}; rebake the provider with this Incan release",
            artifact.dependency_key,
            path.display(),
            manifest.compiler_version,
            INCAN_VERSION
        )));
    }
    let source_authority_hex = manifest
        .source_authority_digest
        .strip_prefix("sha256:")
        .ok_or_else(|| {
            CliError::failure(format!(
                "Oven Alpha cannot use pub::{} package Loaf manifest at {}: its source authority is not a canonical SHA-256 digest",
                artifact.dependency_key,
                path.display()
            ))
        })?;
    if source_authority_hex.len() != 64
        || !source_authority_hex
            .chars()
            .all(|character| character.is_ascii_hexdigit())
    {
        return Err(CliError::failure(format!(
            "Oven Alpha cannot use pub::{} package Loaf manifest at {}: its source authority is not a canonical SHA-256 digest",
            artifact.dependency_key,
            path.display()
        )));
    }
    let canonical_path = fs::canonicalize(&path).map_err(|error| {
        CliError::failure(format!(
            "Oven Alpha cannot resolve package Loaf manifest for pub::{} at {}: {error}",
            artifact.dependency_key,
            path.display()
        ))
    })?;
    Ok(Some((canonical_path, digest_bytes(&bytes), manifest)))
}

/// Read one provider's immutable package-Loaf index and verify its release and local source authority.
///
/// Installed artifact-only providers have no authored project tree and are validated solely through their sealed
/// manifest, receipts, and output digests. A path dependency still has its source project beside `target/lib`; its
/// recursive authored digest must match so an edited provider can never be hidden behind an older package Loaf.
fn read_packaged_library_loaf_manifest(
    artifact: &LibraryArtifactMetadata,
) -> CliResult<Option<OvenPackagedLibraryLoafManifest>> {
    let Some((_path, _manifest_digest, manifest)) = decode_packaged_library_loaf_manifest(artifact)? else {
        return Ok(None);
    };
    validate_packaged_library_metadata_files(artifact, &manifest)?;
    if let Some(project_root) = artifact.crate_root.parent().and_then(Path::parent)
        && project_root.join(MANIFEST_FILENAME).is_file()
    {
        let source_authority_digest = digest_baked_project_source_authority(project_root)?;
        if manifest.source_authority_digest != source_authority_digest {
            return Err(CliError::failure(format!(
                "Oven Alpha refuses pub::{} because its source project at {} changed after the package Loaf was baked; rebake that provider before baking or running a consumer",
                artifact.dependency_key,
                project_root.display()
            )));
        }
    }
    Ok(Some(manifest))
}

/// Return whether a provider declares an explicit package-Loaf handoff.
///
/// The common command preflight uses this only to refuse an implicit provider rebuild. The consumer's Oven planner
/// immediately follows with full schema, receipt, artifact-digest, target, toolchain, and closure validation in
/// [`packaged_library_loaf_profile`] and [`import_packaged_library_loaf`]. Keeping those checks in one place avoids
/// two subtly different package validators.
pub(crate) fn oven_library_dependency_declares_package_loaf(dependency_root: &Path) -> bool {
    let artifact_root = dependency_root.join("target").join("lib");
    let manifest_path = packaged_library_loaf_manifest_path(&artifact_root);
    manifest_path.is_file()
}

/// Resolve one package-owned native output without permitting a symlink escape from its artifact root.
fn validated_packaged_library_output_path(
    artifact: &LibraryArtifactMetadata,
    relative_path: &str,
    profile: &str,
) -> CliResult<PathBuf> {
    let relative = validated_project_output_relative_path(relative_path, "package-owned library output")?;
    let output = artifact.crate_root.join(relative);
    let metadata = fs::symlink_metadata(&output).map_err(|error| {
        CliError::failure(format!(
            "Oven Alpha cannot inspect package-owned `{profile}` library for pub::{} at {}: {error}",
            artifact.dependency_key,
            output.display()
        ))
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(CliError::failure(format!(
            "Oven Alpha refuses package Loaf for pub::{}: `{profile}` library must be a regular file below its artifact root",
            artifact.dependency_key
        )));
    }
    let canonical_root = fs::canonicalize(&artifact.crate_root).map_err(|error| {
        CliError::failure(format!(
            "Oven Alpha cannot resolve package artifact root for pub::{} at {}: {error}",
            artifact.dependency_key,
            artifact.crate_root.display()
        ))
    })?;
    let canonical_output = fs::canonicalize(&output).map_err(|error| {
        CliError::failure(format!(
            "Oven Alpha cannot resolve package-owned `{profile}` library for pub::{} at {}: {error}",
            artifact.dependency_key,
            output.display()
        ))
    })?;
    if !canonical_output.starts_with(&canonical_root) {
        return Err(CliError::failure(format!(
            "Oven Alpha refuses package Loaf for pub::{}: `{profile}` library escapes its artifact root through a symlink",
            artifact.dependency_key
        )));
    }
    Ok(output)
}

/// Return a package-owned profile record only when it can link with the active direct-Rustc target and toolchain.
#[cfg(test)]
fn packaged_library_loaf_profile(
    artifact: &LibraryArtifactMetadata,
    profile: &str,
    target: &str,
    toolchain: &str,
) -> CliResult<Option<OvenPackagedLibraryLoafProfile>> {
    let Some(manifest) = read_packaged_library_loaf_manifest(artifact)? else {
        return Ok(None);
    };
    validated_packaged_library_loaf_profile(artifact, &manifest, profile, target, toolchain)
}

/// Validate one profile against an already admitted provider manifest.
fn validated_packaged_library_loaf_profile(
    artifact: &LibraryArtifactMetadata,
    manifest: &OvenPackagedLibraryLoafManifest,
    profile: &str,
    target: &str,
    toolchain: &str,
) -> CliResult<Option<OvenPackagedLibraryLoafProfile>> {
    let Some(candidate) = manifest.profiles.get(profile) else {
        return Ok(None);
    };
    candidate.receipt.verify_identity().map_err(|error| {
        CliError::failure(format!(
            "Oven Alpha refuses package Loaf for pub::{} because its `{profile}` receipt is invalid: {error}",
            artifact.dependency_key
        ))
    })?;
    if candidate.receipt.intent.profile != profile
        || candidate.receipt.intent.target != target
        || candidate.receipt.intent.toolchain != toolchain
    {
        return Ok(None);
    }
    for entry in &candidate.entries {
        entry.receipt.verify_identity().map_err(|error| {
            CliError::failure(format!(
                "Oven Alpha refuses package Loaf for pub::{} because entry `{}` has an invalid receipt: {error}",
                artifact.dependency_key, entry.identity
            ))
        })?;
        if entry.identity.trim().is_empty() {
            return Err(CliError::failure(format!(
                "Oven Alpha refuses package Loaf for pub::{}: its `{profile}` entries must have an immutable identity",
                artifact.dependency_key
            )));
        }
        if entry.receipt.intent != candidate.receipt.intent {
            return Err(CliError::failure(format!(
                "Oven Alpha refuses package Loaf for pub::{}: entry `{}` has a different sealed intent from its library output",
                artifact.dependency_key, entry.identity
            )));
        }
    }
    let output = validated_packaged_library_output_path(artifact, &candidate.library_relative_path, profile)?;
    let actual_digest = digest_bytes(&fs::read(&output).map_err(|error| {
        CliError::failure(format!(
            "Oven Alpha cannot read package-owned `{profile}` library for pub::{} at {}: {error}",
            artifact.dependency_key,
            output.display()
        ))
    })?);
    if actual_digest != candidate.library_digest {
        return Err(CliError::failure(format!(
            "Oven Alpha refuses package Loaf for pub::{}: `{profile}` library digest {actual_digest} differs from sealed package digest {}",
            artifact.dependency_key, candidate.library_digest
        )));
    }
    Ok(Some(candidate.clone()))
}

impl OvenProjectBakeAuthorityContext {
    /// Validate requested provider profiles before spending a deep authored-tree scan, then memoize that one scan.
    fn checked_packaged_library_loaf_profiles(
        &mut self,
        artifact: &LibraryArtifactMetadata,
        profiles: &[&str],
        target: &str,
        toolchain: &str,
    ) -> CliResult<Option<Vec<OvenPackagedLibraryLoafProfile>>> {
        let canonical_artifact_root = fs::canonicalize(&artifact.crate_root).map_err(|error| {
            CliError::failure(format!(
                "Oven Alpha cannot resolve package artifact root for pub::{} at {}: {error}",
                artifact.dependency_key,
                artifact.crate_root.display()
            ))
        })?;
        let Some((manifest_path, manifest_digest, decoded_manifest)) = decode_packaged_library_loaf_manifest(artifact)?
        else {
            if self.providers.contains_key(&canonical_artifact_root) {
                return Err(CliError::failure(format!(
                    "Oven Alpha refuses pub::{} because its package Loaf manifest disappeared during this explicit bake",
                    artifact.dependency_key
                )));
            }
            return Ok(None);
        };

        let (manifest, source_project_root, source_authority_verified) = if let Some(memoized) =
            self.providers.get(&canonical_artifact_root)
        {
            if memoized.manifest_path != manifest_path || memoized.manifest_digest != manifest_digest {
                return Err(CliError::failure(format!(
                    "Oven Alpha refuses pub::{} because its package Loaf manifest changed during this explicit bake",
                    artifact.dependency_key
                )));
            }
            (
                Arc::clone(&memoized.manifest),
                memoized.source_project_root.clone(),
                memoized.source_authority_verified,
            )
        } else {
            let source_project_root = artifact
                .crate_root
                .parent()
                .and_then(Path::parent)
                .filter(|root| root.join(MANIFEST_FILENAME).is_file())
                .map(|root| {
                    fs::canonicalize(root).map_err(|error| {
                        CliError::failure(format!(
                            "Oven Alpha cannot resolve source project for pub::{} at {}: {error}",
                            artifact.dependency_key,
                            root.display()
                        ))
                    })
                })
                .transpose()?;
            let manifest = Arc::new(decoded_manifest);
            self.providers.insert(
                canonical_artifact_root.clone(),
                MemoizedPackagedProviderAuthority {
                    artifact: artifact.clone(),
                    manifest_path,
                    manifest_digest,
                    manifest: Arc::clone(&manifest),
                    source_project_root: source_project_root.clone(),
                    source_authority_verified: false,
                    admitted_profiles: BTreeMap::new(),
                },
            );
            (manifest, source_project_root, false)
        };

        // Profile receipts, target/toolchain intent, entry receipts, and the native output digest are all cheaper than
        // recursively hashing a provider source tree. A missing release handoff therefore fails before the first deep
        // scan even when debug was the first target/profile requested by the caller.
        let mut selected = Vec::with_capacity(profiles.len());
        for profile in profiles {
            let Some(candidate) =
                validated_packaged_library_loaf_profile(artifact, &manifest, profile, target, toolchain)?
            else {
                return Ok(None);
            };
            selected.push(candidate);
        }
        validate_packaged_library_metadata_files(artifact, &manifest)?;

        if !source_authority_verified {
            if let Some(source_project_root) = source_project_root.as_deref() {
                let actual = self.source_digester.digest(source_project_root)?;
                if actual != manifest.source_authority_digest {
                    return Err(CliError::failure(format!(
                        "Oven Alpha refuses pub::{} because its source project at {} changed after the package Loaf was baked; rebake that provider before baking or running a consumer",
                        artifact.dependency_key,
                        source_project_root.display()
                    )));
                }
            }
            let memoized = self.providers.get_mut(&canonical_artifact_root).ok_or_else(|| {
                CliError::failure(format!(
                    "Oven Alpha lost the command-local provider authority for pub::{} at {} before source validation completed",
                    artifact.dependency_key,
                    canonical_artifact_root.display()
                ))
            })?;
            memoized.source_authority_verified = true;
        }
        let memoized = self.providers.get_mut(&canonical_artifact_root).ok_or_else(|| {
            CliError::failure(format!(
                "Oven Alpha lost the command-local provider authority for pub::{} at {} before profile admission completed",
                artifact.dependency_key,
                canonical_artifact_root.display()
            ))
        })?;
        for profile in profiles {
            memoized
                .admitted_profiles
                .insert((*profile).to_string(), (target.to_string(), toolchain.to_string()));
        }
        Ok(Some(selected))
    }

    /// Scan a tentative cache candidate without committing its current lock projection as publication authority.
    ///
    /// A cache miss must leave this context unbound. In particular, an old completed-output receipt can require an
    /// authority scan before the explicit baker refreshes a legacy lock; binding that old projection would make the
    /// final publication check reject the baker's own lock refresh.
    fn cache_probe_source_authority(&self, project_root: &Path) -> CliResult<String> {
        digest_baked_project_source_authority(project_root)
    }

    /// Return the memoized root authority used while preparing this command's targets.
    fn project_source_authority(&mut self, project_root: &Path) -> CliResult<String> {
        let digest = self.source_digester.digest(project_root)?;
        match self.initial_project_source_authority.as_deref() {
            Some(initial) if initial != digest => Err(CliError::failure(
                "explicit Oven project source authority changed during command-local preparation",
            )),
            Some(_) => Ok(digest),
            None => {
                self.initial_project_source_authority = Some(digest.clone());
                Ok(digest)
            }
        }
    }

    /// Recheck cheap mutable provider facts, then perform the fresh deep scan that gates final publication.
    fn final_project_source_authority(&self, project_root: &Path) -> CliResult<String> {
        for memoized in self.providers.values() {
            let Some((manifest_path, manifest_digest, _decoded_manifest)) =
                decode_packaged_library_loaf_manifest(&memoized.artifact)?
            else {
                return Err(CliError::failure(format!(
                    "Oven Alpha refuses pub::{} because its package Loaf manifest disappeared before final publication",
                    memoized.artifact.dependency_key
                )));
            };
            if manifest_path != memoized.manifest_path || manifest_digest != memoized.manifest_digest {
                return Err(CliError::failure(format!(
                    "Oven Alpha refuses pub::{} because its package Loaf manifest changed before final publication",
                    memoized.artifact.dependency_key
                )));
            }
            for (profile, (target, toolchain)) in &memoized.admitted_profiles {
                if validated_packaged_library_loaf_profile(
                    &memoized.artifact,
                    &memoized.manifest,
                    profile,
                    target,
                    toolchain,
                )?
                .is_none()
                {
                    return Err(CliError::failure(format!(
                        "Oven Alpha refuses pub::{} because its `{profile}` package Loaf became incompatible before final publication",
                        memoized.artifact.dependency_key
                    )));
                }
            }
            validate_packaged_library_metadata_files(&memoized.artifact, &memoized.manifest)?;
        }

        let mut fresh = ProjectSourceAuthorityDigester::default();
        let final_project_authority = fresh.digest(project_root)?;
        if self
            .initial_project_source_authority
            .as_deref()
            .is_some_and(|initial| initial != final_project_authority)
        {
            return Err(CliError::failure(
                "Oven Alpha refuses to publish this explicit bake because the project source authority changed during preparation",
            ));
        }
        for memoized in self.providers.values() {
            let Some(source_project_root) = memoized.source_project_root.as_deref() else {
                continue;
            };
            let actual = fresh.digest(source_project_root)?;
            if actual != memoized.manifest.source_authority_digest {
                return Err(CliError::failure(format!(
                    "Oven Alpha refuses pub::{} because its source project at {} changed before final publication; rebake that provider before baking this consumer",
                    memoized.artifact.dependency_key,
                    source_project_root.display()
                )));
            }
        }
        Ok(final_project_authority)
    }
}

/// Return whether a store already contains every exact immutable entry required by one package profile.
///
/// This is intentionally receipt-bound rather than an identity-only cache probe: an unrelated artifact with the
/// same digest-shaped name cannot become a substitute for a provider's sealed Rust dependency closure.
fn has_complete_packaged_library_loaf(store: &OvenStore, entries: &[OvenPackagedLibraryLoafEntry]) -> CliResult<bool> {
    for entry in entries {
        let selected = store
            .select_payloads_matching_for_execution(|stored| {
                stored.identity == entry.identity
                    && stored.kind == entry.kind
                    && stored.receipt_identity == entry.receipt.identity
                    && stored.build_unit_identity == entry.receipt.build_unit_identity
                    && stored.intent == entry.receipt.intent
            })
            .map_err(|error| {
                CliError::failure(format!(
                    "failed to inspect package Loaf `{}` before consumer import: {error}",
                    entry.identity
                ))
            })?;
        if selected.len() != 1 {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Import a compatible public provider closure into the consumer's bounded Oven store without running Cargo.
///
/// This is the explicit consumer bake boundary. A provider artifact normally carries its own portable package-store
/// export. After a local output-only restoration, that duplicate export is deliberately absent; the matching
/// receipt-bound entries remain in the primary Oven store and can be selected there without re-publishing them on a
/// normal provider build.
fn import_checked_packaged_library_loaf(
    consumer_store: &OvenStore,
    checked: &CheckedPackagedProviderProfile,
) -> CliResult<()> {
    let package_profile = &checked.package;
    let package_store = OvenStore::new(
        packaged_library_loaf_store_root(&checked.artifact_root),
        *consumer_store.limits(),
    );
    let source_store = if has_complete_packaged_library_loaf(&package_store, &package_profile.entries)? {
        &package_store
    } else if has_complete_packaged_library_loaf(consumer_store, &package_profile.entries)? {
        consumer_store
    } else {
        return Err(CliError::failure(format!(
            "Oven Alpha cannot import pub::{}: its portable package Loaf is absent and the current Oven store has no matching receipt-bound closure; run `incan oven bake --project {}`",
            checked.dependency_key,
            checked.artifact_root.display()
        )));
    };
    for entry in &package_profile.entries {
        let imported = copy_receipted_oven_store_entry(
            source_store,
            consumer_store,
            &entry.receipt,
            &entry.identity,
            entry.kind,
            "consumer package-Loaf import",
        )?;
        if imported.identity != entry.identity {
            return Err(CliError::failure(format!(
                "consumer package-Loaf import changed the sealed entry identity `{}`",
                entry.identity
            )));
        }
    }
    Ok(())
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

/// Resolve and validate every distinct executable entrypoint selected by one effective manifest.
///
/// This is shared by target discovery and source-authority hashing so an executable cannot be baked without the same
/// path also participating in freshness checks.
fn discover_oven_executable_entrypoints(manifest: &ProjectManifest) -> CliResult<BTreeMap<String, PathBuf>> {
    let mut executable_paths = BTreeMap::new();
    if let Some(project) = manifest.project.as_ref() {
        let mut scripts = project.scripts.iter().collect::<Vec<_>>();
        scripts.sort_by(|left, right| left.0.cmp(right.0));
        for (name, configured_path) in scripts {
            let relative = validated_project_output_relative_path(configured_path, "declared script")?;
            let entrypoint = manifest.project_root().join(&relative);
            let metadata = fs::symlink_metadata(&entrypoint).map_err(|error| {
                CliError::failure(format!(
                    "declared Oven project script `{name}` must resolve to a regular file at {}: {error}",
                    entrypoint.display()
                ))
            })?;
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                return Err(CliError::failure(format!(
                    "declared Oven project script `{name}` must resolve to a regular file at {}",
                    entrypoint.display()
                )));
            }
            executable_paths.insert(relative.to_string_lossy().replace('\\', "/"), entrypoint);
        }
    }
    let conventional_main = manifest
        .project_root()
        .join(OvenBakeProjectTarget::Executable.source_relative_path());
    match fs::symlink_metadata(&conventional_main) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            return Err(CliError::failure(format!(
                "Oven project executable target must be a regular file: {}",
                conventional_main.display()
            )));
        }
        Ok(_) => {
            executable_paths.insert(
                OvenBakeProjectTarget::Executable.source_relative_path().to_string(),
                conventional_main,
            );
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(CliError::failure(format!(
                "failed to inspect Oven project executable target {}: {error}",
                conventional_main.display()
            )));
        }
    }
    Ok(executable_paths)
}

/// Resolve every manifest-backed target that an explicit project bake must prepare.
///
/// Declared scripts are first-class executable targets rather than aliases for `src/main.incn`. The conventional main
/// remains an implicit fallback when present, and exact duplicate paths are collapsed so one authored entrypoint is
/// never compiled twice merely because it has more than one script name.
fn discover_oven_bake_project_targets(project_root: &Path) -> CliResult<Vec<(OvenBakeProjectTarget, PathBuf)>> {
    let Some(manifest) = discover_effective_project_manifest(project_root)? else {
        return Err(CliError::failure(format!(
            "`incan oven bake --project` requires an incan.toml project at {}",
            project_root.display()
        )));
    };
    enforce_project_toolchain_constraint(&manifest)?;

    let mut targets = Vec::new();
    let library = manifest
        .project_root()
        .join(OvenBakeProjectTarget::Library.source_relative_path());
    match fs::symlink_metadata(&library) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            return Err(CliError::failure(format!(
                "Oven project library target must be a regular file: {}",
                library.display()
            )));
        }
        Ok(_) => targets.push((OvenBakeProjectTarget::Library, library)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(CliError::failure(format!(
                "failed to inspect Oven project library target {}: {error}",
                library.display()
            )));
        }
    }

    targets.extend(
        discover_oven_executable_entrypoints(&manifest)?
            .into_values()
            .map(|entrypoint| (OvenBakeProjectTarget::Executable, entrypoint)),
    );
    if targets.is_empty() {
        return Err(CliError::failure(format!(
            "`incan oven bake --project` requires {}, {}, or a declared [project.scripts] entry below {}",
            OvenBakeProjectTarget::Library.source_relative_path(),
            OvenBakeProjectTarget::Executable.source_relative_path(),
            manifest.project_root().display()
        )));
    }
    Ok(targets)
}

/// Choose one entry that asks the lock collector for the complete explicit-bake dependency surface.
///
/// Lock collection already includes every declared script and the conventional library. Prefer conventional main for
/// stable existing behavior, then any executable, then the library-only root.
fn oven_bake_dependency_surface_entrypoint(targets: &[(OvenBakeProjectTarget, PathBuf)]) -> Option<&Path> {
    targets
        .iter()
        .find(|(target, entrypoint)| {
            *target == OvenBakeProjectTarget::Executable
                && entrypoint.ends_with(OvenBakeProjectTarget::Executable.source_relative_path())
        })
        .or_else(|| {
            targets
                .iter()
                .find(|(target, _)| *target == OvenBakeProjectTarget::Executable)
        })
        .or_else(|| targets.first())
        .map(|(_, entrypoint)| entrypoint.as_path())
}

/// Publish the canonical semantic lock after an explicit bake has materialized any local provider handoff needed
/// to inspect a rooted workspace.
///
/// A cold rooted workspace can contain a consumer of its own root library. The lock collector must read that
/// library's checked package metadata, while the completed project Loaf must in turn bind the lock that the collector
/// publishes. The explicit bake therefore materializes the provider first, publishes the lock once, and only then
/// seals the final project-output authority. This remains inside the named publisher command and never authorizes a
/// normal build, run, test, or lock command to compile a missing provider.
fn publish_project_lock_after_provider_bake(
    project_root: &Path,
    entrypoint: &Path,
    package_features: &FeatureSelection,
) -> CliResult<PublishedOvenProjectLock> {
    publish_oven_project_lock(project_root, entrypoint, package_features)
}

/// Explicitly prepare compatible Oven closures for every manifest-backed target in one Incan project.
///
/// This is preparation rather than execution: it records fresh source/lock/SDK/provider receipt evidence, reuses a
/// matching stored or release-scoped stdlib closure when available, and otherwise crosses Oven's explicit bounded
/// publisher exactly once per genuinely missing target/profile. Normal `build`, `run`, and `test` remain Cargo-free
/// consumers of the resulting direct-rustc plans.
pub(crate) fn bake_oven_project_targets(
    project: &Path,
    package_features: &FeatureSelection,
) -> CliResult<OvenProjectBakeReport> {
    let project = project
        .to_str()
        .ok_or_else(|| CliError::failure(format!("Oven project path is not valid UTF-8: {}", project.display())))?;
    let project_root = resolve_library_project_root(Some(project))?;
    let targets = discover_oven_bake_project_targets(&project_root)?;
    let dependency_surface_entrypoint = oven_bake_dependency_surface_entrypoint(&targets)
        .ok_or_else(|| CliError::failure("explicit Oven project bake discovered no dependency-surface entrypoint"))?
        .to_path_buf();
    let store = open_default_oven_store()?;
    let mut authority_context = OvenProjectBakeAuthorityContext::default();
    if canonical_baked_project_lock_path(&project_root)?.is_file()
        && let Some(reused) = try_reuse_baked_project(
            &project_root,
            &targets,
            &store,
            package_features,
            &mut authority_context,
        )?
    {
        return Ok(reused);
    }
    let mut source_authority_digest = None;
    let mut published_project_lock = None;
    let mut generated_sources = BTreeMap::new();
    let mut profiles = Vec::new();
    let mut pending_outputs = Vec::new();
    let mut debug_target_receipts = Vec::new();
    let mut library_inspection_constituent: Option<LibraryInspectionConstituent> = None;
    // Every prepared target keeps the store leases of the plans it selected or published. Those leases must outlive
    // the whole bake, not just the target's own loop arm: the inspection authority sealed after the loop names those
    // entries as constituents, and a later admission in the same bake (the test-dependency envelope, the authority
    // itself) prunes unleased entries when the domain policy is tight. A bake that succeeds must leave a loadable
    // closure, so its constituents stay leased until the authority is sealed; if the policy cannot hold them all,
    // admission fails loudly instead.
    let mut retained_preparations: Vec<PreparedLibraryProject> = Vec::new();
    #[cfg(feature = "rust_inspect")]
    let mut rust_inspect_manifest_dirs = BTreeSet::new();

    for (target, entrypoint) in targets {
        match target {
            OvenBakeProjectTarget::Library => {
                let mut prepared = prepare_library_project(
                    Some(project),
                    None,
                    CargoPolicy::default(),
                    package_features,
                    None,
                    Vec::new(),
                    false,
                    false,
                    None,
                    true,
                    false,
                    OvenProjectPlanMode::ExplicitBake,
                    Some(&mut authority_context),
                    &BackendSelectionOptions::default(),
                )?;
                #[cfg(feature = "rust_inspect")]
                if let Some(manifest_dir) = prepared.rust_inspect_manifest_dir.as_ref() {
                    rust_inspect_manifest_dirs.insert(manifest_dir.clone());
                }
                write_library_manifest_artifacts(&mut prepared)?;
                let selected = prepared.oven.as_ref().ok_or_else(|| {
                    CliError::failure("explicit Oven library preparation did not produce a direct-rustc selection")
                })?;
                let backend_receipt = prepared.report.backend.clone().ok_or_else(|| {
                    CliError::failure("explicit Oven library preparation did not produce backend provenance")
                })?;
                generated_sources.insert(
                    oven_bake_project_target_identity(&project_root, target, &prepared.entrypoint)?,
                    prepared.generator.crate_root_path(),
                );
                let package_store_root = packaged_library_loaf_store_root(&prepared.out_dir);
                let mut package_profiles = BTreeMap::new();
                let mut completed_outputs = Vec::new();
                for (profile, selected_profile) in &selected.profiles {
                    if profile == "debug" {
                        debug_target_receipts.push(selected_profile.receipt.clone());
                    }
                    if profile == "debug" {
                        // The library's debug plan is the constituent that lets a test unit inspect the library's
                        // dependencies. A direct-rustc bake stores it whole, and its rust-inspect workspace holds
                        // the Cargo bootstrap's generated Rust. When the closure is not loadable as independently
                        // compiled parts, the bounded compatibility baker publishes the library as a store-owned
                        // extension of a compiler Loaf instead; the composed manifest under the extension's
                        // identity is the same constituent, and the generated project's own Cargo target holds
                        // the generated Rust.
                        let constituent = match &selected_profile.plan_selection {
                            OvenDirectRustcPlanSelection::Stored(plan) => Some((
                                plan.identity.clone(),
                                plan.artifacts.clone(),
                                OvenArtifactKind::DirectRustcPlan,
                                None,
                            )),
                            OvenDirectRustcPlanSelection::ProjectExtension(extension) => Some((
                                extension.extension.identity.clone(),
                                extension.artifacts.clone(),
                                OvenArtifactKind::ProjectPayload,
                                Some(extension.base.loaf_identity.clone()),
                            )),
                            OvenDirectRustcPlanSelection::ToolchainLoaf(_)
                            | OvenDirectRustcPlanSelection::PackagedProvider(_) => None,
                        };
                        if let Some((identity, artifacts, artifact_kind, base_loaf_identity)) = constituent {
                            library_inspection_constituent = Some(LibraryInspectionConstituent {
                                identity,
                                artifact_kind,
                                base_loaf_identity,
                                receipt: selected_profile.receipt.clone(),
                                artifacts,
                                rust_inspect_manifest_dir: prepared.rust_inspect_manifest_dir.clone(),
                                cargo_target_dir: Some(prepared.generator.cargo_target_dir()),
                                generated_project_dir: Some(prepared.generator.output_dir().to_path_buf()),
                            });
                        }
                    }
                    let bake = bake_oven_library(&prepared, selected, profile, Some(&mut authority_context))?;
                    let library_relative_path = bake
                        .output
                        .strip_prefix(&prepared.out_dir)
                        .map_err(|_| {
                            CliError::failure(format!(
                                "baked public library output {} escaped its artifact root {}",
                                bake.output.display(),
                                prepared.out_dir.display()
                            ))
                        })?
                        .to_string_lossy()
                        .replace('\\', "/");
                    let entries = export_selected_package_loaf(
                        &store,
                        &package_store_root,
                        &selected_profile.receipt,
                        &selected_profile.plan_selection,
                    )?;
                    completed_outputs.push((
                        profile.clone(),
                        selected_profile.receipt.clone(),
                        selected_profile.plan_selection.report_identity(),
                        bake.output.clone(),
                        entries.clone(),
                    ));
                    package_profiles.insert(
                        profile.clone(),
                        OvenPackagedLibraryLoafProfile {
                            receipt: selected_profile.receipt.clone(),
                            entries,
                            library_relative_path,
                            library_digest: bake.output_digest,
                        },
                    );
                    let receipt = project_bake_receipt_path(&project_root, target, &prepared.entrypoint, profile)?;
                    write_receipt(&selected_profile.receipt, &receipt)
                        .map_err(|error| CliError::failure(error.to_string()))?;
                    profiles.push(OvenProjectBakeProfileReport {
                        project_target: oven_bake_project_target_identity(&project_root, target, &prepared.entrypoint)?,
                        profile: profile.clone(),
                        target: selected_profile.receipt.intent.target.clone(),
                        toolchain: selected_profile.receipt.intent.toolchain.clone(),
                        receipt,
                        receipt_identity: selected_profile.receipt.identity.clone(),
                        build_unit_identity: selected_profile.receipt.build_unit_identity.clone(),
                        plan_identity: selected_profile.plan_selection.report_identity(),
                        action: selected_profile.materialization.as_str(),
                    });
                }
                published_project_lock = Some(publish_project_lock_after_provider_bake(
                    &project_root,
                    &dependency_surface_entrypoint,
                    package_features,
                )?);
                source_authority_digest = Some(authority_context.project_source_authority(&project_root)?);
                let source_authority_digest = source_authority_digest
                    .as_deref()
                    .ok_or_else(|| CliError::failure("explicit Oven library bake lost its final source authority"))?;
                let library_sidecars = library_project_output_sidecars(&prepared.library_manifest, &prepared.out_dir)?;
                let metadata_files = packaged_library_metadata_files(
                    &prepared.manifest_path,
                    &prepared.library_manifest,
                    &prepared.out_dir,
                )?;
                write_packaged_library_loaf_manifest(
                    &prepared.out_dir,
                    &OvenPackagedLibraryLoafManifest {
                        schema_version: OVEN_PACKAGED_LIBRARY_LOAF_SCHEMA_VERSION,
                        source_authority_digest: source_authority_digest.to_string(),
                        compiler_version: INCAN_VERSION.to_string(),
                        metadata_files,
                        profiles: package_profiles,
                    },
                )?;
                let package_loaf_manifest = packaged_library_loaf_manifest_path(&prepared.out_dir);
                let package_loaf_store_relative_path = package_store_root
                    .strip_prefix(&prepared.project_root)
                    .map_err(|_| {
                        CliError::failure(format!(
                            "baked package Loaf store {} escaped project root {}",
                            package_store_root.display(),
                            prepared.project_root.display()
                        ))
                    })?
                    .to_string_lossy()
                    .replace('\\', "/");
                for (profile, receipt, plan_identity, native_output, required_project_loafs) in completed_outputs {
                    let files = project_output_bake_files(
                        &prepared.project_root,
                        &prepared.generator,
                        &native_output,
                        Some(&prepared.manifest_path),
                        Some(&package_loaf_manifest),
                        &library_sidecars,
                    )?;
                    pending_outputs.push(PendingOvenProjectOutput {
                        entrypoint: prepared.entrypoint.clone(),
                        target,
                        receipt,
                        plan_identity,
                        profile,
                        files,
                        required_project_loafs,
                        package_loaf_store_relative_path: Some(package_loaf_store_relative_path.clone()),
                        backend_receipt: backend_receipt.clone(),
                        build_report: None,
                    });
                }
                remove_completed_generated_cargo_lock(prepared.generator.output_dir())?;
                retained_preparations.push(prepared);
            }
            OvenBakeProjectTarget::Executable => {
                if published_project_lock.is_none() {
                    published_project_lock = Some(publish_project_lock_after_provider_bake(
                        &project_root,
                        &dependency_surface_entrypoint,
                        package_features,
                    )?);
                    source_authority_digest = Some(authority_context.project_source_authority(&project_root)?);
                }
                source_authority_digest.as_deref().ok_or_else(|| {
                    CliError::failure("explicit Oven executable bake lost its final source authority")
                })?;
                let entrypoint = entrypoint.to_str().ok_or_else(|| {
                    CliError::failure(format!("Oven entrypoint is not valid UTF-8: {}", entrypoint.display()))
                })?;
                let target_output_dir = oven_bake_executable_output_dir(&project_root, Path::new(entrypoint))?;
                let target_output_dir = target_output_dir
                    .as_deref()
                    .map(|path| {
                        path.to_str().ok_or_else(|| {
                            CliError::failure(format!(
                                "Oven target output path is not valid UTF-8: {}",
                                path.display()
                            ))
                        })
                    })
                    .transpose()?;
                for profile in explicit_bake_profiles() {
                    let prepared = prepare_oven_project(
                        entrypoint,
                        target_output_dir,
                        &CargoPolicy::default(),
                        package_features,
                        None,
                        Vec::new(),
                        false,
                        false,
                        profile,
                        OvenProjectPlanMode::ExplicitBake,
                        Some(&mut authority_context),
                        &BackendSelectionOptions::default(),
                    )?;
                    #[cfg(feature = "rust_inspect")]
                    if let Some(manifest_dir) = prepared.rust_inspect_manifest_dir.as_ref() {
                        rust_inspect_manifest_dirs.insert(manifest_dir.clone());
                    }
                    if profile == "debug" {
                        debug_target_receipts.push(prepared.receipt.clone());
                    }
                    let receipt = project_bake_receipt_path(&project_root, target, &prepared.entrypoint, profile)?;
                    write_receipt(&prepared.receipt, &receipt).map_err(|error| CliError::failure(error.to_string()))?;
                    let bake = bake_oven_project(&prepared, profile, Some(&mut authority_context))?;
                    let backend_receipt = prepared.report.backend.clone().ok_or_else(|| {
                        CliError::failure("explicit Oven executable preparation did not produce backend provenance")
                    })?;
                    let mut report = prepared.report.clone();
                    report.artifacts.push(artifact_report("binary", &bake.output));
                    let build_report = project_output_report_snapshot(&project_root, &report.finish(BTreeMap::new()))?;
                    let files = project_output_bake_files(
                        &prepared.project_root,
                        &prepared.generator,
                        &bake.output,
                        None,
                        None,
                        &[],
                    )?;
                    pending_outputs.push(PendingOvenProjectOutput {
                        entrypoint: prepared.entrypoint.clone(),
                        target,
                        receipt: prepared.receipt.clone(),
                        plan_identity: prepared.plan_selection.report_identity(),
                        profile: profile.to_string(),
                        files,
                        required_project_loafs: Vec::new(),
                        package_loaf_store_relative_path: None,
                        backend_receipt,
                        build_report: Some(build_report),
                    });
                    remove_completed_generated_cargo_lock(prepared.generator.output_dir())?;
                    let target_identity =
                        oven_bake_project_target_identity(&project_root, target, &prepared.entrypoint)?;
                    generated_sources
                        .entry(target_identity.clone())
                        .or_insert_with(|| prepared.generator.crate_root_path());
                    profiles.push(OvenProjectBakeProfileReport {
                        project_target: target_identity,
                        profile: profile.to_string(),
                        target: prepared.receipt.intent.target.clone(),
                        toolchain: prepared.receipt.intent.toolchain.clone(),
                        receipt,
                        receipt_identity: prepared.receipt.identity.clone(),
                        build_unit_identity: prepared.receipt.build_unit_identity.clone(),
                        plan_identity: prepared.plan_selection.report_identity(),
                        action: prepared.materialization.as_str(),
                    });
                }
            }
        }
    }
    source_authority_digest
        .as_deref()
        .ok_or_else(|| CliError::failure("explicit Oven bake did not finalize its project source authority"))?;
    let dependency_surface = published_project_lock
        .as_ref()
        .ok_or_else(|| CliError::failure("explicit Oven bake did not retain its canonical project lock"))?
        .dependency_surface();
    let test_dependency_envelope = prepare_oven_test_dependency_envelope(
        &store,
        &project_root,
        dependency_surface,
        &debug_target_receipts,
        Some(&mut authority_context),
    )?;
    let (registry_dependencies, dev_registry_dependencies) =
        canonical_project_inspection_dependencies(dependency_surface)?;
    let source_authority_digest = authority_context.final_project_source_authority(&project_root)?;
    let inspection_authority = publish_project_inspection_authority(
        &store,
        &project_root,
        &source_authority_digest,
        &registry_dependencies,
        &dev_registry_dependencies,
        &test_dependency_envelope,
        library_inspection_constituent.as_ref(),
    )?;
    let lock_dependencies_fingerprint = baked_project_lock_dependencies_fingerprint(&project_root)?;
    let mut published_outputs = Vec::with_capacity(pending_outputs.len());
    for pending in pending_outputs {
        let files = pending.files;
        let payload = project_output_payload_for_bake(OvenProjectOutputBakeRequest {
            project_root: &project_root,
            entrypoint: &pending.entrypoint,
            target: pending.target,
            receipt: &pending.receipt,
            plan_identity: pending.plan_identity,
            profile: &pending.profile,
            source_authority_digest: &source_authority_digest,
            lock_dependencies_fingerprint: lock_dependencies_fingerprint.clone(),
            files: files.clone(),
            inspection_authority: inspection_authority.reference.clone(),
            required_project_loafs: pending.required_project_loafs,
            package_loaf_store_relative_path: pending.package_loaf_store_relative_path,
            backend_receipt: pending.backend_receipt,
            build_report: pending.build_report,
        })?;
        published_outputs.push(publish_project_output_loaf(&store, &pending.receipt, &payload, &files)?);
    }
    // Keep every sibling output and the authority leased through completion. A tight policy must fail this bake
    // rather than prune an earlier target/profile and then report a partial project as successfully prepared.
    // The inspection authority names the debug test dependency envelope's exact plan. Retain that selection until
    // every output Loaf is visible: otherwise a later output admission can prune the now-unleased constituent and
    // leave a source-current authority that points at a missing closure.
    let _complete_publication_set = (test_dependency_envelope, inspection_authority, published_outputs);
    #[cfg(feature = "rust_inspect")]
    for manifest_dir in rust_inspect_manifest_dirs {
        mark_oven_direct_rust_inspection(&manifest_dir)?;
    }
    Ok(OvenProjectBakeReport {
        project: project_root,
        generated_sources,
        store: store.root().to_path_buf(),
        profiles,
    })
}

/// Build one library project and retain its completed report for workspace-level aggregation.
pub(crate) fn build_library_report(
    file_path: Option<&str>,
    output_dir: Option<&String>,
    options: BuildCommandOptions,
    report_options: &BuildReportOptions,
) -> CliResult<crate::cli::commands::build_report::BuildReport> {
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
            for output in &outputs {
                materialize_project_output(&project_root, output)?;
            }
            write_backend_receipt(&backend_receipt, &default_backend_receipt_path(&project_root))?;
            return completed_library_output_report(&project_root, &outputs, total_start);
        }
    }
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
        write_library_manifest_artifacts(&mut prepared)?;
        let oven_build_start = Instant::now();
        let oven = prepared
            .oven
            .as_ref()
            .ok_or_else(|| CliError::failure("normal Oven library build lost its prepared direct-rustc selection"))?;
        let mut bakes = Vec::new();
        for profile in explicit_bake_profiles() {
            bakes.push((profile, bake_oven_library(&prepared, oven, profile, None)?));
        }
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
    let receipt =
        serde_json::from_slice::<crate::backend::selection::BackendExecutionReceipt>(&bytes).map_err(|error| {
            CliError::failure(format!(
                "failed to parse backend-selection receipt {}: {error}",
                path.display()
            ))
        })?;
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

/// Copy a pending desugarer artifact into its declared path beneath the output directory.
fn package_desugarer_artifact(out_dir: &Path, artifact: Option<&PendingDesugarerArtifact>) -> CliResult<()> {
    let Some(artifact) = artifact else {
        return Ok(());
    };

    let destination = out_dir.join(&artifact.metadata.relative_path);
    let destination_parent = destination.parent().ok_or_else(|| {
        CliError::failure(format!(
            "invalid desugarer artifact destination path: {}",
            destination.display()
        ))
    })?;

    fs::create_dir_all(destination_parent).map_err(|err| {
        CliError::failure(format!(
            "failed to create desugarer artifact directory {}: {err}",
            destination_parent.display()
        ))
    })?;
    fs::copy(&artifact.source_path, &destination).map_err(|err| {
        CliError::failure(format!(
            "failed to package vocab desugarer artifact {} -> {}: {err}",
            artifact.source_path.display(),
            destination.display()
        ))
    })?;

    Ok(())
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
    let _inline_command_lock = crate::lockfile::acquire_publication_lock(&source_path).map_err(|error| {
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

/// Reject controls that only have meaning for the retired Cargo execution backend.
///
/// Lock strictness is deliberately not rejected: it validates compiler-owned `incan.lock` consistency before Oven
/// selection without launching Cargo. Offline is already satisfied because this normal path starts neither Cargo nor
/// a networked dependency resolver.
fn reject_normal_cargo_controls(cargo_policy: &CargoPolicy, target_dir: Option<&PathBuf>) -> CliResult<()> {
    if !cargo_policy.extra_args.is_empty() || target_dir.is_some() {
        return Err(CliError::failure(
            "Oven Alpha normal build and run do not accept Cargo passthrough or target-directory controls; use the supported Oven-native provider/dependency envelope instead",
        ));
    }
    Ok(())
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
    use crate::frontend::api_metadata::ApiDeclaration;
    use crate::frontend::body_ir;
    use crate::frontend::lexer;
    use crate::frontend::library_exports::CheckedExportIdentity;
    use crate::frontend::parser;
    use crate::frontend::symbols::ResolvedType;
    use crate::lockfile::{
        CargoFeatureSelection, IncanLock, LockedOvenState, LockedProvider, LockedSdkComponent, LockedSdkState,
        SemanticLockState, compute_deps_fingerprint,
    };
    use crate::manifest::ProjectManifest;
    use crate::oven::interop::{
        OvenInteropCapabilitySelection, default_interop_execution_receipt_path, receipt_interop_execution,
        write_interop_execution_receipt,
    };
    use crate::oven_interop::locked_oven_interop_targets;
    use std::fs;

    #[test]
    fn provider_operation_metadata_is_projected_from_checked_declaration_facts()
    -> Result<(), Box<dyn std::error::Error>> {
        use std::collections::BTreeMap;
        use std::path::PathBuf;

        use crate::frontend::typechecker::{ProviderOperationDeclarationInfo, TypeCheckInfo};
        use incan_semantics_core::{CanonicalSymbolId, HirSourceSpan, SemanticSourceTargetKind};

        let operation = CanonicalSymbolId::module_declaration(
            vec!["provider".to_string(), "billing".to_string()],
            "charge",
            SemanticSourceTargetKind::Function,
            HirSourceSpan::new(20, 26),
        );
        let required_capability = CanonicalSymbolId::module_declaration(
            vec!["provider".to_string(), "billing".to_string()],
            "charge_card",
            SemanticSourceTargetKind::Capability,
            HirSourceSpan::new(1, 12),
        );
        let mut type_info = TypeCheckInfo::default();
        type_info.declarations.provider_operations.insert(
            operation.clone(),
            ProviderOperationDeclarationInfo {
                operation: operation.clone(),
                required_capability: required_capability.clone(),
                runtime_requirements: Vec::new(),
            },
        );

        let descriptors = provider_operation_metadata_from_checked_type_info(&BTreeMap::from([(
            PathBuf::from("src/billing.incn"),
            type_info,
        )]))?;
        assert_eq!(descriptors.len(), 1);
        assert_eq!(descriptors[0].operation, operation);
        assert_eq!(descriptors[0].required_capability, required_capability);
        assert!(descriptors[0].runtime_requirements.is_empty());
        Ok(())
    }

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
    fn test_dependency_envelope_promotes_aliases_features_and_paths_into_dependencies()
    -> Result<(), Box<dyn std::error::Error>> {
        let path_package = tempfile::tempdir()?;
        fs::write(
            path_package.path().join("Cargo.toml"),
            "[package]\nname = \"fixture_path\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )?;
        fs::create_dir(path_package.path().join("src"))?;
        fs::write(path_package.path().join("src/lib.rs"), "pub fn value() {}\n")?;
        let normal = DependencySpec {
            crate_name: "json_api".to_string(),
            version: Some("1".to_string()),
            features: vec!["preserve_order".to_string()],
            default_features: false,
            source: DependencySource::Registry,
            optional: true,
            package: Some("serde_json".to_string()),
        };
        let mut dev = normal.clone();
        dev.features = vec!["raw_value".to_string()];
        dev.default_features = true;
        let path = DependencySpec {
            crate_name: "fixture_alias".to_string(),
            version: Some("0.1.0".to_string()),
            features: vec!["testing".to_string()],
            default_features: false,
            source: DependencySource::Path {
                path: path_package.path().to_path_buf(),
            },
            optional: true,
            package: Some("fixture_path".to_string()),
        };

        let promoted = promoted_oven_test_dependencies(&ResolvedDependencies {
            dependencies: vec![normal],
            dev_dependencies: vec![dev, path],
        })?;
        assert_eq!(promoted.len(), 2);
        let json = promoted
            .iter()
            .find(|dependency| dependency.crate_name == "json_api")
            .ok_or("renamed registry dependency disappeared")?;
        assert_eq!(json.package.as_deref(), Some("serde_json"));
        assert_eq!(json.features, ["preserve_order", "raw_value"]);
        assert!(json.default_features);
        assert!(!json.optional);
        let path = promoted
            .iter()
            .find(|dependency| dependency.crate_name == "fixture_alias")
            .ok_or("path dependency disappeared")?;
        assert!(matches!(path.source, DependencySource::Path { .. }));
        assert!(!path.optional);
        let root_digests = oven_test_dependency_root_digests(&promoted)?;
        let locked_json = OvenProjectInspectionRootDependency {
            alias: "json_api".to_string(),
            package: "serde_json".to_string(),
            version: "1.0.140".to_string(),
            registry: "registry+https://github.com/rust-lang/crates.io-index".to_string(),
            checksum: "serde-json-checksum".to_string(),
            requested_features: vec!["preserve_order".to_string(), "raw_value".to_string()],
            default_features: true,
        };
        let exact_roots = project_inspection_test_dependency_roots(
            &promoted,
            &root_digests,
            std::slice::from_ref(&locked_json),
            &[],
        )?;
        assert!(matches!(
            exact_roots.get("json_api"),
            Some(OvenProjectInspectionTestDependencyRoot::Registry { locked, .. }) if locked == &locked_json
        ));
        assert!(matches!(
            exact_roots.get("fixture_alias"),
            Some(OvenProjectInspectionTestDependencyRoot::Path { .. })
        ));

        let generated = tempfile::tempdir()?;
        let mut generator = ProjectGenerator::new(generated.path(), "dependency_envelope", true);
        generator.set_dependencies(promoted.clone());
        generator.set_dev_dependencies(Vec::new());
        generator.generate("fn main() {}\n")?;
        let manifest = fs::read_to_string(generated.path().join("Cargo.toml"))?;
        assert!(manifest.contains("[dependencies.json_api]"));
        assert!(manifest.contains("package = \"serde_json\""));
        assert!(manifest.contains("features = [\"preserve_order\", \"raw_value\"]"));
        assert!(manifest.contains("[dependencies.fixture_alias]"));
        assert!(manifest.contains("package = \"fixture_path\""));
        assert!(!manifest.contains("[dev-dependencies"));

        fs::write(path_package.path().join("src/lib.rs"), "pub fn changed() {}\n")?;
        let changed_root_digests = oven_test_dependency_root_digests(&promoted)?;
        assert_ne!(
            root_digests.get("fixture_alias"),
            changed_root_digests.get("fixture_alias")
        );
        Ok(())
    }

    #[test]
    fn packaged_provider_root_stays_authoritative_without_entering_test_dependency_publisher_issue951()
    -> Result<(), Box<dyn std::error::Error>> {
        let provider = tempfile::tempdir()?;
        fs::create_dir(provider.path().join("src"))?;
        fs::write(
            provider.path().join("Cargo.toml"),
            "[package]\nname = \"set_library\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )?;
        fs::write(provider.path().join("src/lib.rs"), "pub fn unique() {}\n")?;
        let provider_dependency = DependencySpec {
            crate_name: "set_library".to_string(),
            version: Some("0.1.0".to_string()),
            features: Vec::new(),
            default_features: true,
            source: DependencySource::Path {
                path: provider.path().to_path_buf(),
            },
            optional: false,
            package: None,
        };
        let serde = DependencySpec {
            crate_name: "serde".to_string(),
            version: Some("1".to_string()),
            features: vec!["derive".to_string()],
            default_features: true,
            source: DependencySource::Registry,
            optional: false,
            package: None,
        };
        let promoted = promoted_oven_test_dependencies(&ResolvedDependencies {
            dependencies: vec![provider_dependency],
            dev_dependencies: vec![serde],
        })?;
        let packaged_provider_aliases = BTreeSet::from(["set_library".to_string()]);
        let publisher_dependencies = test_dependency_publisher_dependencies(&promoted, &packaged_provider_aliases);

        assert!(promoted.iter().any(|dependency| dependency.crate_name == "set_library"));
        assert!(
            oven_test_dependency_root_digests(&promoted)?.contains_key("set_library"),
            "the singular project authority must retain the provider root"
        );
        assert!(
            publisher_dependencies
                .iter()
                .all(|dependency| dependency.crate_name != "set_library")
        );
        assert!(
            publisher_dependencies
                .iter()
                .any(|dependency| dependency.crate_name == "serde")
        );
        assert!(
            publisher_dependencies
                .iter()
                .map(|dependency| dependency.crate_name.replace('-', "_"))
                .all(|alias| !packaged_provider_aliases.contains(&alias)),
            "the Cargo-published test delta and sealed package-provider externs must be disjoint"
        );
        Ok(())
    }

    #[test]
    fn caller_owned_provider_authority_prefers_the_explicit_library_receipt() -> Result<(), Box<dyn std::error::Error>>
    {
        let project = tempfile::tempdir()?;
        let library_source = project.path().join("generated/library.rs");
        let script_source = project.path().join("generated/checker.rs");
        fs::create_dir_all(library_source.parent().ok_or("library source has no parent")?)?;
        fs::write(&library_source, "pub fn library() {}\n")?;
        fs::write(&script_source, "fn main() {}\n")?;

        let library_receipt = receipt_generated_project(
            &OvenGeneratedProjectRequest::new(
                project.path(),
                "provider",
                "0.1.0",
                "aarch64-apple-darwin",
                "rustc fixture",
                "release",
                Vec::new(),
            )
            .with_generated_source("generated-root", &library_source),
        )?;
        let script_receipt = receipt_generated_project(
            &OvenGeneratedProjectRequest::new(
                project.path(),
                "provider",
                "0.1.0",
                "aarch64-apple-darwin",
                "rustc fixture",
                "release",
                Vec::new(),
            )
            .with_generated_source("generated-root", &script_source),
        )?;
        assert_ne!(library_receipt.identity, script_receipt.identity);

        write_receipt(
            &library_receipt,
            project_bake_receipt_path(
                project.path(),
                OvenBakeProjectTarget::Library,
                &project.path().join("src/lib.incn"),
                "release",
            )?,
        )?;
        write_receipt(&script_receipt, crate::oven::default_receipt_path(project.path()))?;

        let selected = read_verified_caller_owned_provider_receipt(project.path(), "release")
            .ok_or("library receipt should be selected")?;
        assert_eq!(selected.identity, library_receipt.identity);
        Ok(())
    }

    #[test]
    fn prepared_debug_target_reuse_requires_exact_generated_dependency_surface()
    -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let generated = project.path().join("generated/src/main.rs");
        fs::create_dir_all(generated.parent().ok_or("generated source has no parent")?)?;
        fs::write(&generated, "fn main() {}\n")?;
        let exact_digest = digest_bytes(b"canonical normal+dev dependency surface");
        let debug_request = OvenGeneratedProjectRequest::new(
            project.path(),
            "fixture",
            "0.1.0",
            "aarch64-apple-darwin",
            "rustc 1.96.0",
            "debug",
            Vec::new(),
        )
        .with_generated_source("generated-root", &generated)
        .with_build_unit_input("rust-dependencies", exact_digest.clone());
        let debug = receipt_generated_project(&debug_request)?;
        assert!(debug_target_receipt_covers_test_publisher_dependencies(
            &debug,
            &exact_digest
        ));
        assert!(!debug_target_receipt_covers_test_publisher_dependencies(
            &debug,
            &digest_bytes(b"stale surface")
        ));

        let missing = receipt_generated_project(
            &OvenGeneratedProjectRequest::new(
                project.path(),
                "fixture",
                "0.1.0",
                "aarch64-apple-darwin",
                "rustc 1.96.0",
                "debug",
                Vec::new(),
            )
            .with_generated_source("generated-root", &generated),
        )?;
        assert!(!debug_target_receipt_covers_test_publisher_dependencies(
            &missing,
            &exact_digest
        ));

        let mut wrong_kind = debug.clone();
        wrong_kind.compatibility.kind = crate::oven::OvenCompatibilityKind::FrozenCargoPackage;
        assert!(!debug_target_receipt_covers_test_publisher_dependencies(
            &wrong_kind,
            &exact_digest
        ));

        let release = receipt_generated_project(
            &OvenGeneratedProjectRequest::new(
                project.path(),
                "fixture",
                "0.1.0",
                "aarch64-apple-darwin",
                "rustc 1.96.0",
                "release",
                Vec::new(),
            )
            .with_generated_source("generated-root", &generated)
            .with_build_unit_input("rust-dependencies", exact_digest.clone()),
        )?;
        assert!(!debug_target_receipt_covers_test_publisher_dependencies(
            &release,
            &exact_digest
        ));
        Ok(())
    }

    #[test]
    fn project_inspection_roots_bind_release_owned_features_without_an_extension()
    -> Result<(), Box<dyn std::error::Error>> {
        let catalog = vec![OvenRustcRegistrySourcePackage {
            package: "serde_json".to_string(),
            version: "1.0.140".to_string(),
            features: vec!["preserve_order".to_string(), "std".to_string()],
            source: crate::oven::rustc::OvenRustcRegistrySource {
                registry: "registry+https://github.com/rust-lang/crates.io-index".to_string(),
                checksum: "serde-json-checksum".to_string(),
                relative_root: "registry-sources/serde_json-1.0.140".to_string(),
                digest: digest_bytes(b"serde_json source"),
            },
        }];
        let dependency = DependencySpec {
            crate_name: "serde_json".to_string(),
            version: Some("1".to_string()),
            features: vec!["preserve_order".to_string()],
            default_features: false,
            source: DependencySource::Registry,
            optional: false,
            package: None,
        };

        let roots = project_inspection_root_dependencies(std::slice::from_ref(&dependency), &catalog, None)?;
        assert_eq!(roots.len(), 1);
        assert_eq!(roots[0].package, "serde_json");
        assert_eq!(roots[0].requested_features, ["preserve_order"]);
        assert!(!roots[0].default_features);
        let publisher_root = OvenProjectRegistrySourceDependency {
            alias: "serde_json".to_string(),
            package: "serde_json".to_string(),
            version: "1.0.140".to_string(),
            registry: "registry+https://github.com/rust-lang/crates.io-index".to_string(),
            checksum: "serde-json-checksum".to_string(),
        };
        let promoted_publisher_roots = [publisher_root.clone(), publisher_root];
        assert_eq!(
            project_inspection_root_dependencies(
                std::slice::from_ref(&dependency),
                &catalog,
                Some(&promoted_publisher_roots),
            )?,
            roots
        );

        let mut unavailable = dependency;
        unavailable.features = vec!["raw_value".to_string()];
        let Err(error) = project_inspection_root_dependencies(&[unavailable], &catalog, None) else {
            return Err("release-only source authority accepted a feature absent from the Loaf".into());
        };
        assert!(error.to_string().contains("0 feature-compatible"));
        Ok(())
    }

    #[test]
    fn baked_project_source_authority_tracks_transitive_path_source_not_its_output()
    -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let provider = project.path().join("provider");
        fs::create_dir_all(project.path().join("src"))?;
        fs::create_dir_all(provider.join("src"))?;
        fs::write(
            project.path().join("incan.toml"),
            "[project]\nname = \"consumer\"\n\n[dependencies]\nprovider = { path = \"provider\" }\n",
        )?;
        fs::write(project.path().join("src/main.incn"), "def main() -> None:\n    pass\n")?;
        fs::write(provider.join("incan.toml"), "[project]\nname = \"provider\"\n")?;
        fs::write(provider.join("src/lib.incn"), "pub def value() -> int:\n    return 1\n")?;
        let initial = digest_baked_project_source_authority(project.path())?;

        fs::create_dir_all(provider.join(".ralph-cache/loafs"))?;
        fs::write(provider.join(".ralph-cache/loafs/mutable"), "not source authority")?;
        assert_eq!(initial, digest_baked_project_source_authority(project.path())?);

        fs::create_dir_all(project.path().join("docs"))?;
        fs::write(
            project.path().join("docs/architecture.png"),
            "documentation is not a build input",
        )?;
        assert_eq!(initial, digest_baked_project_source_authority(project.path())?);

        fs::write(provider.join("src/lib.incn"), "pub def value() -> int:\n    return 2\n")?;
        assert_ne!(initial, digest_baked_project_source_authority(project.path())?);
        Ok(())
    }

    #[test]
    fn baked_project_source_authority_tracks_rust_path_crate_without_an_incan_manifest()
    -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let rust_workspace = project.path().join("rust-workspace");
        let rust_crate = rust_workspace.join("rust-helper");
        let rust_leaf = rust_workspace.join("rust-leaf");
        fs::create_dir_all(project.path().join("src"))?;
        fs::create_dir_all(rust_crate.join("src"))?;
        fs::create_dir_all(rust_leaf.join("src"))?;
        fs::write(
            project.path().join("incan.toml"),
            "[project]\nname = \"consumer\"\n\n[rust-dependencies.rust_helper]\npath = \"rust-workspace/rust-helper\"\n",
        )?;
        fs::write(project.path().join("src/main.incn"), "def main() -> None:\n    pass\n")?;
        fs::write(
            rust_workspace.join("Cargo.toml"),
            "[workspace]\nmembers = [\"rust-helper\", \"rust-leaf\"]\nresolver = \"2\"\n\n[workspace.package]\nversion = \"0.1.0\"\n\n[workspace.dependencies]\nitoa = \"1\"\n",
        )?;
        fs::write(
            rust_crate.join("Cargo.toml"),
            "[package]\nname = \"rust_helper\"\nversion.workspace = true\nedition = \"2024\"\n\n[dependencies]\nitoa.workspace = true\nrust_leaf = { path = \"../rust-leaf\" }\n",
        )?;
        fs::write(rust_crate.join("src/lib.rs"), "pub fn value() -> i64 { 1 }\n")?;
        fs::write(
            rust_leaf.join("Cargo.toml"),
            "[package]\nname = \"rust_leaf\"\nversion.workspace = true\nedition = \"2024\"\n",
        )?;
        fs::write(rust_leaf.join("src/lib.rs"), "pub fn leaf() -> i64 { 1 }\n")?;

        let initial = digest_baked_project_source_authority(project.path())?;
        fs::create_dir_all(rust_crate.join("target/debug"))?;
        fs::write(rust_crate.join("target/debug/libhelper.rlib"), "mutable output")?;
        assert_eq!(initial, digest_baked_project_source_authority(project.path())?);

        fs::write(
            rust_workspace.join("Cargo.toml"),
            "[workspace]\nmembers = [\"rust-helper\", \"rust-leaf\"]\nresolver = \"2\"\n\n[workspace.package]\nversion = \"0.2.0\"\n\n[workspace.dependencies]\nitoa = \"1\"\n",
        )?;
        let inherited_workspace_changed = digest_baked_project_source_authority(project.path())?;
        assert_ne!(initial, inherited_workspace_changed);

        fs::write(rust_crate.join("src/lib.rs"), "pub fn value() -> i64 { 2 }\n")?;
        assert_ne!(
            inherited_workspace_changed,
            digest_baked_project_source_authority(project.path())?
        );
        let direct_source_changed = digest_baked_project_source_authority(project.path())?;
        fs::write(rust_leaf.join("src/lib.rs"), "pub fn leaf() -> i64 { 2 }\n")?;
        assert_ne!(
            direct_source_changed,
            digest_baked_project_source_authority(project.path())?,
            "a transitive sibling Cargo path dependency must remain part of the project source authority"
        );
        Ok(())
    }

    #[test]
    fn baked_project_source_authority_binds_dependency_content_to_its_named_edge()
    -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let left = project.path().join("deps/left");
        let right = project.path().join("deps/right");
        fs::create_dir_all(project.path().join("src"))?;
        fs::create_dir_all(left.join("src"))?;
        fs::create_dir_all(right.join("src"))?;
        fs::write(
            project.path().join("incan.toml"),
            "[project]\nname = \"consumer\"\n\n[dependencies]\nleft = { path = \"deps/left\" }\nright = { path = \"deps/right\" }\n",
        )?;
        fs::write(project.path().join("src/main.incn"), "def main() -> None:\n    pass\n")?;
        let left_manifest = "[project]\nname = \"left_provider\"\n";
        let right_manifest = "[project]\nname = \"right_provider\"\n";
        let left_source = "pub def value() -> int:\n    return 1\n";
        let right_source = "pub def value() -> int:\n    return 2\n";
        fs::write(left.join("incan.toml"), left_manifest)?;
        fs::write(left.join("src/lib.incn"), left_source)?;
        fs::write(right.join("incan.toml"), right_manifest)?;
        fs::write(right.join("src/lib.incn"), right_source)?;
        let initial = digest_baked_project_source_authority(project.path())?;

        fs::write(left.join("incan.toml"), right_manifest)?;
        fs::write(left.join("src/lib.incn"), right_source)?;
        fs::write(right.join("incan.toml"), left_manifest)?;
        fs::write(right.join("src/lib.incn"), left_source)?;

        assert_ne!(
            initial,
            digest_baked_project_source_authority(project.path())?,
            "swapping the same reachable child trees between named dependency slots must invalidate the root"
        );
        Ok(())
    }

    #[test]
    fn baked_project_source_authority_tracks_every_declared_model_bundle_file() -> Result<(), Box<dyn std::error::Error>>
    {
        let project = tempfile::tempdir()?;
        fs::create_dir_all(project.path().join("src"))?;
        fs::create_dir_all(project.path().join("contracts"))?;
        fs::write(
            project.path().join("incan.toml"),
            "[project]\nname = \"consumer\"\n\n[tool.incan.metadata]\nmodel-bundles = [\"contracts/a.json\", \"contracts/b.json\"]\n",
        )?;
        fs::write(project.path().join("src/main.incn"), "def main() -> None:\n    pass\n")?;
        fs::write(project.path().join("contracts/a.json"), "{\"model\":\"a-one\"}\n")?;
        fs::write(project.path().join("contracts/b.json"), "{\"model\":\"b-one\"}\n")?;
        let initial = digest_baked_project_source_authority(project.path())?;

        fs::write(project.path().join("contracts/b.json"), "{\"model\":\"b-two\"}\n")?;
        assert_ne!(initial, digest_baked_project_source_authority(project.path())?);
        fs::write(project.path().join("contracts/b.json"), "{\"model\":\"b-one\"}\n")?;
        fs::write(project.path().join("contracts/a.json"), "{\"model\":\"a-two\"}\n")?;
        assert_ne!(initial, digest_baked_project_source_authority(project.path())?);
        Ok(())
    }

    #[test]
    fn baked_project_source_authority_tracks_the_declared_vocab_companion() -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        fs::create_dir_all(project.path().join("src"))?;
        fs::create_dir_all(project.path().join("vocab_companion/src"))?;
        fs::write(
            project.path().join("incan.toml"),
            "[project]\nname = \"consumer\"\n\n[vocab]\ncrate = \"vocab_companion\"\n",
        )?;
        fs::write(
            project.path().join("src/lib.incn"),
            "pub def value() -> int:\n    return 1\n",
        )?;
        fs::write(
            project.path().join("vocab_companion/Cargo.toml"),
            "[package]\nname = \"fixture_vocab\"\nversion = \"0.1.0\"\n",
        )?;
        let companion_source = project.path().join("vocab_companion/src/lib.rs");
        fs::write(&companion_source, "pub fn library_vocab() { }\n")?;
        let initial = digest_baked_project_source_authority(project.path())?;

        fs::write(&companion_source, "pub fn library_vocab() { let _changed = true; }\n")?;
        assert_ne!(
            initial,
            digest_baked_project_source_authority(project.path())?,
            "a changed declared vocabulary companion must invalidate its completed provider output"
        );
        Ok(())
    }

    #[test]
    fn baked_project_source_authority_uses_the_configured_source_root() -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        fs::create_dir_all(project.path().join("src"))?;
        fs::create_dir_all(project.path().join("library"))?;
        fs::write(
            project.path().join("incan.toml"),
            "[project]\nname = \"consumer\"\n\n[build]\nsource-root = \"library\"\n",
        )?;
        fs::write(project.path().join("src/ignored.incn"), "pub const IGNORED: int = 1\n")?;
        let configured_source = project.path().join("library/lib.incn");
        fs::write(&configured_source, "pub def value() -> int:\n    return 1\n")?;
        let initial = digest_baked_project_source_authority(project.path())?;

        fs::write(project.path().join("src/ignored.incn"), "pub const IGNORED: int = 2\n")?;
        assert_eq!(
            initial,
            digest_baked_project_source_authority(project.path())?,
            "the conventional src directory must not become authority when the manifest selects another source root"
        );
        fs::write(&configured_source, "pub def value() -> int:\n    return 2\n")?;
        assert_ne!(
            initial,
            digest_baked_project_source_authority(project.path())?,
            "a change below the configured source root must invalidate the completed project output"
        );
        Ok(())
    }

    #[test]
    fn baked_project_source_authority_tracks_declared_scripts_outside_the_source_root()
    -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        fs::create_dir(project.path().join("library"))?;
        fs::create_dir(project.path().join("scripts"))?;
        fs::write(
            project.path().join("incan.toml"),
            "[project]\nname = \"consumer\"\n\n[project.scripts]\ncli = \"scripts/cli.incn\"\ncli_alias = \"scripts/cli.incn\"\n\n[build]\nsource-root = \"library\"\n",
        )?;
        fs::write(
            project.path().join("library/lib.incn"),
            "pub def value() -> int:\n    return 1\n",
        )?;
        let script = project.path().join("scripts/cli.incn");
        fs::write(&script, "def main() -> None:\n    println(1)\n")?;
        let initial = digest_baked_project_source_authority(project.path())?;

        fs::write(&script, "def main() -> None:\n    println(2)\n")?;

        assert_ne!(
            initial,
            digest_baked_project_source_authority(project.path())?,
            "a declared executable outside the configured source root must invalidate completed outputs"
        );
        Ok(())
    }

    #[test]
    fn baked_project_source_authority_tracks_fresh_interop_inputs_and_selected_receipt()
    -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        fs::create_dir_all(project.path().join("src"))?;
        fs::create_dir_all(project.path().join("interop/include"))?;
        fs::write(
            project.path().join("incan.toml"),
            r#"[project]
name = "consumer"

[oven.interop]
schema = 1

[[oven.interop.targets]]
target = "aarch64-apple-darwin"
toolchain = { capability = "apple-clang", version = ">=17, <19" }
headers = ["interop/include/bridge.h"]
"#,
        )?;
        fs::write(project.path().join("src/main.incn"), "def main() -> None:\n    pass\n")?;
        let header = project.path().join("interop/include/bridge.h");
        fs::write(&header, "int incan_bridge(void);\n")?;
        let manifest = ProjectManifest::load(&project.path().join("incan.toml"))?;
        let locked = locked_oven_interop_targets(&manifest)?;
        let target = locked.first().ok_or("expected one locked interop target")?;
        let first_receipt = receipt_interop_execution(
            target,
            Some(OvenInteropCapabilitySelection {
                capability: "apple-clang".to_string(),
                version: "17.0.6".to_string(),
                identity: "sha256:clang-17".to_string(),
            }),
            None,
        )?;
        let receipt_path = default_interop_execution_receipt_path(project.path(), &target.target);
        write_interop_execution_receipt(&first_receipt, &receipt_path)?;
        let initial = digest_baked_project_source_authority(project.path())?;

        let second_receipt = receipt_interop_execution(
            target,
            Some(OvenInteropCapabilitySelection {
                capability: "apple-clang".to_string(),
                version: "18.0.0".to_string(),
                identity: "sha256:clang-18".to_string(),
            }),
            None,
        )?;
        write_interop_execution_receipt(&second_receipt, &receipt_path)?;
        assert_ne!(
            initial,
            digest_baked_project_source_authority(project.path())?,
            "a different valid selected execution receipt must invalidate a completed output"
        );

        write_interop_execution_receipt(&first_receipt, &receipt_path)?;
        fs::write(&header, "int incan_bridge_changed(void);\n")?;
        assert!(
            digest_baked_project_source_authority(project.path()).is_err(),
            "fresh interop input identities must reject the now-stale selected receipt"
        );
        let changed_manifest = ProjectManifest::load(&project.path().join("incan.toml"))?;
        let changed_locked = locked_oven_interop_targets(&changed_manifest)?;
        let changed_target = changed_locked
            .first()
            .ok_or("expected one changed locked interop target")?;
        let changed_receipt = receipt_interop_execution(
            changed_target,
            Some(OvenInteropCapabilitySelection {
                capability: "apple-clang".to_string(),
                version: "17.0.6".to_string(),
                identity: "sha256:clang-17".to_string(),
            }),
            None,
        )?;
        write_interop_execution_receipt(&changed_receipt, &receipt_path)?;
        assert_ne!(
            initial,
            digest_baked_project_source_authority(project.path())?,
            "a declared interop header mutation must invalidate a completed output after reselection"
        );
        Ok(())
    }

    #[test]
    fn normal_oven_consumer_selects_a_sealed_final_interop_plan() -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let generated = project.path().join("generated/src/main.rs");
        fs::create_dir_all(generated.parent().ok_or("generated source parent missing")?)?;
        fs::write(&generated, "fn main() {}\n")?;
        let receipt = receipt_generated_project(
            &OvenGeneratedProjectRequest::new(
                project.path(),
                "interop-final-consumer",
                "0.1.0",
                "aarch64-apple-darwin",
                "rustc fixture",
                "debug",
                Vec::new(),
            )
            .with_generated_source("generated-root", &generated)
            .with_build_unit_input(OVEN_INTEROP_EXECUTION_RECEIPT_INPUT, "sha256:selected-native-execution")
            .with_build_unit_input(crate::oven::interop::OVEN_INTEROP_PLAN_SCHEMA_INPUT, "5"),
        )?;
        let store = OvenStore::new(
            project.path().join("oven-store"),
            crate::oven::store::OvenStoreLimits::new(1024 * 1024, 1024 * 1024, 1024 * 1024),
        );
        store.publish(&OvenArtifactPublishRequest {
            receipt: receipt.clone(),
            domain: "interop-final-consumer".to_string(),
            kind: OvenArtifactKind::DirectRustcPlan,
            payload: serde_json::to_vec(&OvenRustcArtifactManifest {
                schema_version: crate::oven::rustc::OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION,
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
            })?,
            materialized_files: Vec::new(),
        })?;

        let selected = select_or_bake_generated_project_plan(
            OvenProjectPlanMode::ConsumeOnly,
            &store,
            &receipt,
            OvenProjectDependencySurface { selection: &[] },
            project.path(),
            &generated,
            &PathBuf::from("/usr/bin/rustc"),
        )?
        .ok_or("normal Oven consumer did not select its exact final interop plan")?;
        assert!(matches!(
            selected.plan_selection,
            OvenDirectRustcPlanSelection::Stored(_)
        ));
        assert!(!selected.cargo_process_started);
        Ok(())
    }

    #[test]
    fn bake_generated_out_dirs_reads_only_rust_bearing_cargo_build_units() -> Result<(), Box<dyn std::error::Error>> {
        // The bake's rust-inspect workspace names its Cargo target through `.cargo/config.toml`. Only build units
        // whose `out` holds generated Rust are worth sealing, in Oven's `<crate>/<hash>/out` layout as well as
        // Cargo's `<crate>-<hash>/out`.
        let tmp = tempfile::tempdir()?;
        let manifest_dir = tmp.path().join("inspect");
        let target_dir = tmp.path().join("inspect-target");
        fs::create_dir_all(manifest_dir.join(".cargo"))?;
        fs::write(
            manifest_dir.join(".cargo/config.toml"),
            format!("[build]\ntarget-dir = \"{}\"\n", target_dir.display()),
        )?;
        let oven_out = target_dir.join("debug/build/substrait/157348677c93f659/out");
        let cargo_out = target_dir.join("debug/build/prost-types-9741e23407182c1c/out");
        let plain_out = target_dir.join("debug/build/cc/0123456789abcdef/out");
        for dir in [&oven_out, &cargo_out, &plain_out] {
            fs::create_dir_all(dir)?;
        }
        fs::write(oven_out.join("substrait.rs"), "pub mod proto {}\n")?;
        fs::write(cargo_out.join("types.rs"), "pub struct Duration;\n")?;
        fs::write(plain_out.join("flags"), "")?;

        let named_target =
            rust_inspect_workspace_cargo_target(&manifest_dir)?.ok_or("the workspace config names a Cargo target")?;
        assert_eq!(named_target, target_dir);
        let dirs = bake_generated_out_dirs(&named_target)?;

        assert_eq!(
            dirs,
            vec![
                BakeGeneratedOutDir {
                    crate_name: "prost-types".to_string(),
                    unit_relative_path: "prost-types-9741e23407182c1c".to_string(),
                    out_dir: cargo_out.clone(),
                    version: None,
                },
                BakeGeneratedOutDir {
                    crate_name: "substrait".to_string(),
                    unit_relative_path: "substrait/157348677c93f659".to_string(),
                    out_dir: oven_out.clone(),
                    version: None,
                },
            ]
        );
        assert!(
            rust_inspect_workspace_cargo_target(tmp.path())?.is_none(),
            "a workspace without a Cargo config names no target"
        );
        assert!(
            bake_generated_out_dirs(&tmp.path().join("never-built"))?.is_empty(),
            "a Cargo target without build units seals nothing"
        );
        // The build-unit path is read from either layout and only below a `build` directory.
        assert_eq!(
            build_unit_relative_path(&cargo_out).as_deref(),
            Some("prost-types-9741e23407182c1c")
        );
        assert_eq!(
            build_unit_relative_path(&oven_out).as_deref(),
            Some("substrait/157348677c93f659")
        );
        assert_eq!(build_unit_relative_path(&tmp.path().join("deps/out")), None);
        assert_eq!(
            build_unit_relative_path(&target_dir.join("debug/build/x/y/z/out")),
            None
        );
        Ok(())
    }

    #[test]
    fn baked_workspace_member_source_authority_uses_only_the_canonical_root_lock()
    -> Result<(), Box<dyn std::error::Error>> {
        let workspace = tempfile::tempdir()?;
        let member = workspace.path().join("member");
        let provider = workspace.path().join("provider");
        let rust_helper = workspace.path().join("rust-helper");
        fs::create_dir_all(member.join("src"))?;
        fs::create_dir_all(provider.join("src"))?;
        fs::create_dir_all(rust_helper.join("src"))?;
        fs::write(
            workspace.path().join("incan.toml"),
            "[workspace]\nmembers = [\"member\"]\n\n[workspace.dependencies]\nprovider = { path = \"provider\" }\n\n[workspace.rust-dependencies]\nrust_helper = { path = \"rust-helper\" }\n",
        )?;
        fs::write(
            member.join("incan.toml"),
            "[project]\nname = \"member\"\nversion = \"0.1.0\"\n\n[project.scripts]\nmain = \"src/main.incn\"\n\n[dependencies]\nprovider = { workspace = true }\n\n[rust-dependencies]\nrust_helper = { workspace = true }\n",
        )?;
        fs::write(member.join("src/main.incn"), "def main() -> None:\n    pass\n")?;
        fs::write(provider.join("incan.toml"), "[project]\nname = \"provider\"\n")?;
        fs::write(provider.join("src/lib.incn"), "pub def value() -> int:\n    return 1\n")?;
        fs::write(
            rust_helper.join("Cargo.toml"),
            "[package]\nname = \"rust_helper\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )?;
        fs::write(rust_helper.join("src/lib.rs"), "pub fn value() -> i64 { 1 }\n")?;
        IncanLock::new(
            "sha256:canonical-one".to_string(),
            CargoFeatureSelection::default(),
            "version = 4\n".to_string(),
        )
        .write(&workspace.path().join("incan.lock"))?;
        fs::write(member.join("incan.lock"), "obsolete-member-lock-one\n")?;

        let initial = digest_baked_project_source_authority(&member)?;
        fs::write(member.join("incan.lock"), "obsolete-member-lock-two\n")?;
        assert_eq!(
            initial,
            digest_baked_project_source_authority(&member)?,
            "a workspace member must ignore a non-authoritative member-local lock"
        );

        IncanLock::new(
            "sha256:canonical-two".to_string(),
            CargoFeatureSelection::default(),
            "version = 4\n".to_string(),
        )
        .write(&workspace.path().join("incan.lock"))?;
        assert_eq!(
            initial,
            digest_baked_project_source_authority(&member)?,
            "a derived dependency fingerprint must not replace the canonical semantic lock authority"
        );

        IncanLock::new(
            "sha256:canonical-two".to_string(),
            CargoFeatureSelection::default(),
            "version = 4\n\n[[package]]\nname = \"changed\"\nversion = \"1.0.0\"\n".to_string(),
        )
        .write(&workspace.path().join("incan.lock"))?;
        assert_ne!(
            initial,
            digest_baked_project_source_authority(&member)?,
            "a canonical lock payload change must invalidate completed project authority"
        );

        let changed_lock = digest_baked_project_source_authority(&member)?;
        fs::write(provider.join("src/lib.incn"), "pub def value() -> int:\n    return 2\n")?;
        assert_ne!(
            changed_lock,
            digest_baked_project_source_authority(&member)?,
            "workspace-inherited Incan provider source must remain part of completed-output authority"
        );
        fs::write(provider.join("src/lib.incn"), "pub def value() -> int:\n    return 1\n")?;
        fs::write(rust_helper.join("src/lib.rs"), "pub fn value() -> i64 { 2 }\n")?;
        assert_ne!(
            changed_lock,
            digest_baked_project_source_authority(&member)?,
            "workspace-inherited Rust path source must remain part of completed-output authority"
        );
        Ok(())
    }

    #[test]
    fn baked_project_source_authority_ignores_lock_format_migration_issue1194() -> Result<(), Box<dyn std::error::Error>>
    {
        let project = tempfile::tempdir()?;
        fs::create_dir_all(project.path().join("src"))?;
        fs::write(
            project.path().join("incan.toml"),
            "[project]\nname = \"lock_migration_fixture\"\nversion = \"0.1.0\"\n",
        )?;
        fs::write(project.path().join("src/main.incn"), "def main() -> None:\n    pass\n")?;
        let lock_path = project.path().join("incan.lock");
        let mut lock = IncanLock::new(
            "sha256:lock-migration".to_string(),
            CargoFeatureSelection::default(),
            "version = 4\n".to_string(),
        );
        lock.format = 1;
        lock.write(&lock_path)?;
        let format_one = digest_baked_project_source_authority(project.path())?;

        lock.format = 2;
        lock.incan_version = "0.5.1-rc2".to_string();
        lock.write(&lock_path)?;
        assert_eq!(
            format_one,
            digest_baked_project_source_authority(project.path())?,
            "a structural or compiler-cohort lock refresh must not invalidate unchanged authored project input"
        );
        Ok(())
    }

    #[test]
    fn baked_project_source_authority_ignores_sdk_cohort_refresh_issue1194() -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        fs::create_dir_all(project.path().join("src"))?;
        fs::write(
            project.path().join("incan.toml"),
            "[project]\nname = \"sdk_cohort_fixture\"\nversion = \"0.1.0\"\n",
        )?;
        fs::write(project.path().join("src/main.incn"), "def main() -> None:\n    pass\n")?;
        let lock_path = project.path().join("incan.lock");
        let mut lock = IncanLock::new_with_semantic(
            "sha256:sdk-cohort".to_string(),
            CargoFeatureSelection::default(),
            SemanticLockState {
                sdk: Some(LockedSdkState {
                    identity: "incan@0.5.0".to_string(),
                    inventory_digest: "sha256:old-sdk".to_string(),
                    profile: "default".to_string(),
                    components: vec![LockedSdkComponent {
                        id: "stdlib-core".to_string(),
                        version: "0.5.0".to_string(),
                        reason: "mandatory".to_string(),
                    }],
                }),
                providers: vec![
                    LockedProvider {
                        identity: "incan_stdlib_core@0.5.0#sha256:old[]".to_string(),
                        participation: "used".to_string(),
                        namespace_claims: BTreeSet::new(),
                        used_modules: BTreeSet::new(),
                        implementation_facets: Vec::new(),
                        backend_requirements: BTreeSet::new(),
                    },
                    LockedProvider {
                        identity: "example_provider@1.0.0#sha256:stable[]".to_string(),
                        participation: "used".to_string(),
                        namespace_claims: BTreeSet::new(),
                        used_modules: BTreeSet::new(),
                        implementation_facets: Vec::new(),
                        backend_requirements: BTreeSet::new(),
                    },
                ],
                ..SemanticLockState::default()
            },
            "version = 4\n".to_string(),
        );
        lock.write(&lock_path)?;
        let initial = digest_baked_project_source_authority(project.path())?;

        lock.incan_version = "0.5.1-rc2".to_string();
        let sdk = lock.semantic.sdk.as_mut().ok_or("fixture lost SDK state")?;
        sdk.identity = "incan@0.5.1-rc2".to_string();
        sdk.inventory_digest = "sha256:new-sdk".to_string();
        sdk.components[0].version = "0.5.1-rc2".to_string();
        lock.semantic.providers[0].identity = "incan_stdlib_core@0.5.1-rc2#sha256:new[]".to_string();
        lock.write(&lock_path)?;
        assert_eq!(
            initial,
            digest_baked_project_source_authority(project.path())?,
            "a compiler-owned SDK cohort refresh must not invalidate unchanged authored project input"
        );

        lock.semantic.providers[1].identity = "example_provider@1.0.0#sha256:changed[]".to_string();
        lock.write(&lock_path)?;
        assert_ne!(
            initial,
            digest_baked_project_source_authority(project.path())?,
            "a non-SDK provider selection must remain part of project authority"
        );
        Ok(())
    }

    fn fixture_project_output_publication(
        project_root: &Path,
        profile: &str,
        label: &str,
    ) -> Result<
        (
            crate::oven::OvenReceipt,
            OvenProjectOutputPayload,
            Vec<OvenProjectOutputBakeFile>,
        ),
        Box<dyn std::error::Error>,
    > {
        let entrypoint = project_root.join("src/main.incn");
        let generated_source = project_root.join(format!("target/fixture/generated-{label}.rs"));
        let native_output = project_root.join(format!("target/fixture/native-{label}"));
        fs::create_dir_all(generated_source.parent().ok_or("generated source has no parent")?)?;
        fs::write(&generated_source, format!("fn {label}() {{}}\n"))?;
        fs::write(&native_output, format!("native-{label}"))?;
        let rustc = resolve_active_rustc()?;
        let receipt = receipt_generated_project(
            &OvenGeneratedProjectRequest::new(
                project_root,
                "fixture",
                "0.1.0",
                rustc_host_target(&rustc)?,
                rustc_identity(&rustc)?,
                profile,
                Vec::new(),
            )
            .with_generated_source("generated-root", &generated_source)
            .with_build_unit_input("compiler-version", INCAN_VERSION),
        )?;
        let files = vec![OvenProjectOutputBakeFile {
            source_path: native_output,
            caller_relative_path: format!("target/fixture/{profile}-{label}"),
            output_relative_path: OVEN_PROJECT_OUTPUT_ARTIFACT_PATH.to_string(),
        }];
        let backend_selection = select_backend(
            BackendKind::Legacy,
            false,
            false,
            format!("sha256:fixture-source-{label}"),
            FallbackPolicy::Refuse,
        );
        let backend_receipt = finalize_receipt(
            &backend_selection,
            BackendKind::Legacy,
            format!("sha256:fixture-output-{label}"),
            ShadowComparisonState::NotRequested,
            diagnostics::DIAGNOSTIC_SCHEMA_VERSION,
        )?;
        let payload = project_output_payload_for_bake(OvenProjectOutputBakeRequest {
            project_root,
            entrypoint: &entrypoint,
            target: OvenBakeProjectTarget::Executable,
            receipt: &receipt,
            plan_identity: format!("fixture-plan-{label}"),
            profile,
            source_authority_digest: &digest_baked_project_source_authority(project_root)?,
            lock_dependencies_fingerprint: baked_project_lock_dependencies_fingerprint(project_root)?,
            files: files.clone(),
            inspection_authority: OvenProjectInspectionAuthorityRef {
                identity: "sha256:fixture-project-authority".to_string(),
                receipt_identity: "sha256:fixture-authority-receipt".to_string(),
                build_unit_identity: "sha256:fixture-authority-build-unit".to_string(),
            },
            required_project_loafs: Vec::new(),
            package_loaf_store_relative_path: None,
            backend_receipt,
            build_report: None,
        })?;
        Ok((receipt, payload, files))
    }

    #[test]
    fn completed_executable_report_replays_sealed_dependencies_and_rebases_project_paths()
    -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let external = tempfile::tempdir()?;
        let relocated = tempfile::tempdir()?;
        let lexical_external = project.path().join("../set_library");
        fs::create_dir(project.path().join("src"))?;
        fs::write(project.path().join("incan.toml"), "[project]\nname = \"fixture\"\n")?;
        fs::write(project.path().join("src/main.incn"), "def main() -> None:\n    pass\n")?;
        let (receipt, mut payload, files) = fixture_project_output_publication(project.path(), "release", "report")?;
        let authored_sentinel = "$INCAN_PROJECT_ROOT/ordinary-authored-string";
        let mut report = serde_json::json!({
            "schema_version": BUILD_REPORT_SCHEMA_VERSION,
            "compiler_version": INCAN_VERSION,
            "status": "success",
            "mode": "executable",
            "profile": "release",
            "project": {
                "name": "fixture",
                "version": null,
                "project_root": project.path().to_string_lossy(),
            },
            "entrypoint": project.path().join("src/main.incn").to_string_lossy(),
            "library_root": null,
            "source_files": [{
                "path": external.path().join("src/provider.incn").to_string_lossy(),
                "module_path": ["provider"],
            }],
            "generated": {
                "project_path": project.path().join("target/fixture").to_string_lossy(),
                "manifest_path": project.path().join("target/fixture/Cargo.toml").to_string_lossy(),
                "crate_root": project.path().join("target/fixture/src/main.rs").to_string_lossy(),
                "cargo_target_dir": null,
                "oven_output_dir": project.path().join("target/fixture/oven").to_string_lossy(),
            },
            "artifacts": [{
                "kind": "binary",
                "path": project.path().join("target/fixture/native-report").to_string_lossy(),
                "exists": true,
                "size_bytes": 1,
            }],
            "dependencies": {
                "rust": [{
                    "crate_name": "itoa",
                    "source": "path",
                    "source_detail": external.path().join("rust/itoa").to_string_lossy(),
                }],
                "rust_dev": [],
                "incan": [{
                    "library_name": "provider",
                    "path": external.path().join("incan/provider").to_string_lossy(),
                }, {
                    "library_name": "lexical-sibling",
                    "path": lexical_external.to_string_lossy(),
                }],
                "stdlib_features": [authored_sentinel],
            },
            "semantic": {
                "sdk": null,
                "packages": [{
                    "package": "provider",
                    "project_root": external.path().join("incan/provider").to_string_lossy(),
                }],
                "feature_edges": [{
                    "from": project.path().to_string_lossy(),
                    "to": external.path().join("incan/provider").to_string_lossy(),
                }],
                "providers": [{
                    "manifest_path": external.path().join("incan/provider/target/lib/library.incnlib").to_string_lossy(),
                    "provenance": {
                        "kind": "sdk",
                        "inventory_path": external.path().join("sdk/inventory.json").to_string_lossy(),
                    },
                }],
            },
            "oven": {
                "receipt_identity": payload.receipt_identity.clone(),
                "build_unit_identity": payload.build_unit_identity.clone(),
                "plan_identity": payload.plan_identity.clone(),
            },
            "interop": {
                "rust_imports": [authored_sentinel],
                "rust_externs": [],
                "rust_abi_query_paths": ["itoa::Buffer"],
            },
            "timings_ms": { "bake": 1 },
            "notes": [authored_sentinel],
            "workspace": { "member_name": "must-not-survive" },
        });
        make_project_output_report_portable(&mut report, project.path())?;
        let lexical_tag = report
            .pointer("/dependencies/incan/1/path")
            .and_then(serde_json::Value::as_object)
            .and_then(|path| path.get(OVEN_PROJECT_OUTPUT_REPORT_PATH_TAG))
            .and_then(serde_json::Value::as_object)
            .ok_or("lexical sibling dependency was not sealed as a portable path")?;
        assert_eq!(lexical_tag.get("root"), Some(&serde_json::json!("external")));
        let sealed = serde_json::to_string(&report)?;
        assert!(!sealed.contains(project.path().to_string_lossy().as_ref()));
        assert!(!sealed.contains(external.path().to_string_lossy().as_ref()));
        let mut relocated_report = report.clone();
        restore_project_output_report_paths(&mut relocated_report, relocated.path())?;
        assert_eq!(
            relocated_report.pointer("/project/project_root"),
            Some(&serde_json::json!(relocated.path().to_string_lossy()))
        );
        assert_eq!(
            relocated_report.pointer("/entrypoint"),
            Some(&serde_json::json!(
                relocated.path().join("src/main.incn").to_string_lossy()
            ))
        );
        let relocated_rust_source = relocated_report
            .pointer("/dependencies/rust/0/source_detail")
            .and_then(serde_json::Value::as_str)
            .ok_or("relocated Rust dependency report lost its source authority")?;
        assert!(relocated_rust_source.starts_with(OVEN_PROJECT_OUTPUT_REPORT_EXTERNAL_ROOT));
        assert!(!Path::new(relocated_rust_source).is_absolute());
        let relocated_lexical_sibling = relocated_report
            .pointer("/dependencies/incan/1/path")
            .and_then(serde_json::Value::as_str)
            .ok_or("relocated lexical sibling dependency lost its external authority slot")?;
        assert!(relocated_lexical_sibling.starts_with(OVEN_PROJECT_OUTPUT_REPORT_EXTERNAL_ROOT));
        assert!(!Path::new(relocated_lexical_sibling).is_absolute());
        assert_eq!(
            relocated_report.pointer("/notes/0"),
            Some(&serde_json::json!(authored_sentinel)),
            "ordinary authored strings that resemble old path tokens must remain byte-exact"
        );
        payload.build_report = Some(OvenProjectOutputReportSnapshot {
            schema_version: OVEN_PROJECT_OUTPUT_REPORT_SCHEMA_VERSION,
            report,
        });
        let store_root = tempfile::tempdir()?;
        let store = OvenStore::new(
            store_root.path(),
            crate::oven::store::OvenStoreLimits::new(1024 * 1024, 1024 * 1024, 1024 * 1024),
        );
        publish_project_output_loaf(&store, &receipt, &payload, &files)?;
        let selected = select_baked_project_output(
            &store,
            project.path(),
            &project.path().join("src/main.incn"),
            OvenBakeProjectTarget::Executable,
            "release",
        )?
        .ok_or("published completed executable report was not selected")?;
        let backend_receipt = completed_output_default_backend_receipt(&selected)
            .ok_or("completed output did not retain the verified default backend receipt")?;
        materialize_completed_executable_output(project.path(), &selected, &backend_receipt)?;
        let report = completed_executable_output_report(project.path(), &selected, &backend_receipt, Instant::now())?;

        assert_eq!(
            report.pointer("/dependencies/rust/0/crate_name"),
            Some(&serde_json::json!("itoa"))
        );
        assert_eq!(
            report.pointer("/project/project_root"),
            Some(&serde_json::json!(project.path().to_string_lossy()))
        );
        assert!(report.get("workspace").is_none());
        assert_eq!(
            report.pointer("/backend/identity"),
            Some(&serde_json::json!(&backend_receipt.identity))
        );
        let persisted = fs::read(default_backend_receipt_path(project.path()))?;
        let persisted = serde_json::from_slice::<BackendExecutionReceipt>(&persisted)?;
        assert_eq!(persisted, backend_receipt);
        assert!(report.pointer("/timings_ms/completed_project_output_reuse").is_some());
        Ok(())
    }

    #[test]
    fn current_debug_output_scan_selects_exact_lineage_and_rejects_a_stale_only_cohort()
    -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        fs::create_dir(project.path().join("src"))?;
        fs::write(project.path().join("incan.toml"), "[project]\nname = \"fixture\"\n")?;
        fs::write(project.path().join("src/main.incn"), "def main() -> None:\n    pass\n")?;
        let (receipt, payload, files) = fixture_project_output_publication(project.path(), "debug", "current")?;
        write_receipt(
            &receipt,
            project_bake_receipt_path(
                project.path(),
                OvenBakeProjectTarget::Executable,
                &project.path().join("src/main.incn"),
                "debug",
            )?,
        )?;
        let store_root = tempfile::tempdir()?;
        let store = OvenStore::new(
            store_root.path(),
            crate::oven::store::OvenStoreLimits::new(1024 * 1024, 1024 * 1024, 1024 * 1024),
        );
        let _published = publish_project_output_loaf(&store, &receipt, &payload, &files)?;

        let generated_source = project.path().join("target/fixture/generated-current.rs");
        let unrelated_receipt = receipt_generated_project(
            &OvenGeneratedProjectRequest::new(
                project.path(),
                "fixture",
                "0.1.0",
                receipt.intent.target.clone(),
                receipt.intent.toolchain.clone(),
                "debug",
                Vec::new(),
            )
            .with_generated_source("generated-root", &generated_source)
            .with_build_unit_input("unrelated-lineage", "true"),
        )?;
        store.publish(&OvenArtifactPublishRequest {
            receipt: unrelated_receipt,
            domain: format!("incan-release-{INCAN_VERSION}"),
            kind: OvenArtifactKind::ProjectOutput,
            payload: b"malformed unrelated project output".to_vec(),
            materialized_files: Vec::new(),
        })?;
        let mut stale_payload = payload.clone();
        stale_payload.source_authority_digest = digest_bytes(b"stale authored source");
        let stale = publish_project_output_loaf(&store, &receipt, &stale_payload, &files)?;
        drop(stale);
        let targets = discover_oven_bake_project_targets(project.path())?;
        let selected = select_current_debug_project_outputs(
            &store,
            project.path(),
            &targets,
            &digest_baked_project_source_authority(project.path())?,
            &receipt.intent.target,
            &receipt.intent.toolchain,
        )?
        .ok_or("valid exact debug output was poisoned by an unrelated malformed payload")?;
        assert_eq!(selected.len(), 1);
        drop(selected);

        store.publish(&OvenArtifactPublishRequest {
            receipt: receipt.clone(),
            domain: format!("incan-release-{INCAN_VERSION}"),
            kind: OvenArtifactKind::ProjectOutput,
            payload: b"malformed exact project output".to_vec(),
            materialized_files: Vec::new(),
        })?;
        let selected = select_current_debug_project_outputs(
            &store,
            project.path(),
            &targets,
            &digest_baked_project_source_authority(project.path())?,
            &receipt.intent.target,
            &receipt.intent.toolchain,
        )?
        .ok_or("a stale or malformed sibling poisoned the valid exact output")?;
        assert_eq!(selected.len(), 1);
        drop(selected);

        let stale_only_root = tempfile::tempdir()?;
        let stale_only = OvenStore::new(
            stale_only_root.path(),
            crate::oven::store::OvenStoreLimits::new(1024 * 1024, 1024 * 1024, 1024 * 1024),
        );
        let stale = publish_project_output_loaf(&stale_only, &receipt, &stale_payload, &files)?;
        drop(stale);
        let stale_result = select_current_debug_project_outputs(
            &stale_only,
            project.path(),
            &targets,
            &digest_baked_project_source_authority(project.path())?,
            &receipt.intent.target,
            &receipt.intent.toolchain,
        );
        let Err(error) = stale_result else {
            return Err("a same-receipt cohort with no source-current output did not fail closed".into());
        };
        assert!(error.message.contains("disagrees with its source-current lineage"));
        Ok(())
    }

    #[test]
    fn project_output_publication_retains_the_complete_set_under_tight_policy() -> Result<(), Box<dyn std::error::Error>>
    {
        let project = tempfile::tempdir()?;
        fs::create_dir(project.path().join("src"))?;
        fs::write(project.path().join("incan.toml"), "[project]\nname = \"fixture\"\n")?;
        fs::write(project.path().join("src/main.incn"), "def main() -> None:\n    pass\n")?;
        let debug = fixture_project_output_publication(project.path(), "debug", "debug")?;
        let release = fixture_project_output_publication(project.path(), "release", "release")?;

        let prototype_root = tempfile::tempdir()?;
        let prototype = OvenStore::new(
            prototype_root.path(),
            crate::oven::store::OvenStoreLimits::new(1024 * 1024, 1024 * 1024, 1024 * 1024),
        );
        let prototype_debug = publish_project_output_loaf(&prototype, &debug.0, &debug.1, &debug.2)?;
        let prototype_release = publish_project_output_loaf(&prototype, &release.0, &release.1, &release.2)?;
        let measured = prototype.inspect()?;
        drop((prototype_debug, prototype_release));

        let tight_root = tempfile::tempdir()?;
        let tight = OvenStore::new(
            tight_root.path(),
            crate::oven::store::OvenStoreLimits::new(
                measured.physical_bytes,
                measured.physical_bytes,
                measured.logical_bytes,
            ),
        );
        let retained_debug = publish_project_output_loaf(&tight, &debug.0, &debug.1, &debug.2)?;
        let retained_release = publish_project_output_loaf(&tight, &release.0, &release.1, &release.2)?;
        let complete = tight.inspect()?;
        assert_eq!(complete.entries.len(), 2);
        assert_eq!(complete.active_lease_physical_bytes, complete.physical_bytes);
        assert_eq!(complete.reclaimable_physical_bytes, 0);

        let overflow = fixture_project_output_publication(project.path(), "debug", "overflow")?;
        let result = publish_project_output_loaf(&tight, &overflow.0, &overflow.1, &overflow.2);
        let Err(error) = result else {
            return Err("tight policy admitted a third output by pruning a leased project sibling".into());
        };
        assert!(error.message.contains("policy cannot admit"));
        let after = tight.inspect()?;
        assert_eq!(after.entries.len(), 2);
        let identities = after
            .entries
            .iter()
            .map(|entry| entry.manifest.identity.as_str())
            .collect::<BTreeSet<_>>();
        assert!(identities.contains(retained_debug.identity.as_str()));
        assert!(identities.contains(retained_release.identity.as_str()));
        Ok(())
    }

    #[test]
    fn project_output_loaf_selects_only_the_exact_authored_project_before_frontend_work()
    -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let entrypoint = project.path().join("src/main.incn");
        let generated_source = project.path().join("generated/main.rs");
        let native_output = project.path().join("publisher/fixture");
        fs::create_dir_all(entrypoint.parent().ok_or("entrypoint has no parent")?)?;
        fs::create_dir_all(generated_source.parent().ok_or("generated source has no parent")?)?;
        fs::create_dir_all(native_output.parent().ok_or("native output has no parent")?)?;
        fs::write(project.path().join("incan.toml"), "[project]\nname = \"fixture\"\n")?;
        fs::write(&entrypoint, "def main() -> None:\n    pass\n")?;
        fs::write(&generated_source, "fn main() {}\n")?;
        fs::write(&native_output, "fixture native output")?;

        let rustc = resolve_active_rustc()?;
        let receipt = receipt_generated_project(
            &OvenGeneratedProjectRequest::new(
                project.path(),
                "fixture",
                "0.1.0",
                rustc_host_target(&rustc)?,
                rustc_identity(&rustc)?,
                "release",
                Vec::new(),
            )
            .with_generated_source("generated-root", &generated_source)
            .with_build_unit_input("compiler-version", INCAN_VERSION),
        )?;
        let store_root = tempfile::tempdir()?;
        let store = OvenStore::new(
            store_root.path(),
            crate::oven::store::OvenStoreLimits::new(1024 * 1024, 1024 * 1024, 1024 * 1024),
        );
        let payload = OvenProjectOutputPayload {
            schema_version: OVEN_PROJECT_OUTPUT_PAYLOAD_SCHEMA_VERSION,
            project_target: OvenBakeProjectTarget::Executable.as_str().to_string(),
            target_identity: OvenBakeProjectTarget::Executable.as_str().to_string(),
            project_identity: baked_project_owner_identity(project.path())?,
            source_authority_digest: digest_baked_project_source_authority(project.path())?,
            lock_dependencies_fingerprint: baked_project_lock_dependencies_fingerprint(project.path())?,
            compiler_version: INCAN_VERSION.to_string(),
            entrypoint_relative_path: "src/main.incn".to_string(),
            build_unit_identity: receipt.build_unit_identity.clone(),
            receipt_identity: receipt.identity.clone(),
            plan_identity: "fixture-plan".to_string(),
            inspection_authority: Some(OvenProjectInspectionAuthorityRef {
                identity: "fixture-inspection-authority".to_string(),
                receipt_identity: receipt.identity.clone(),
                build_unit_identity: receipt.build_unit_identity.clone(),
            }),
            files: vec![
                OvenProjectOutputFile {
                    caller_relative_path: "generated/main.rs".to_string(),
                    output_relative_path: "generated/main.rs".to_string(),
                    digest: digest_bytes(&fs::read(&generated_source)?),
                    logical_bytes: fs::metadata(&generated_source)?.len(),
                },
                OvenProjectOutputFile {
                    caller_relative_path: "target/incan/fixture/oven/release/fixture".to_string(),
                    output_relative_path: OVEN_PROJECT_OUTPUT_ARTIFACT_PATH.to_string(),
                    digest: digest_bytes(&fs::read(&native_output)?),
                    logical_bytes: fs::metadata(&native_output)?.len(),
                },
            ],
            required_project_loafs: Vec::new(),
            package_loaf_store_relative_path: None,
            backend_receipt: finalize_receipt(
                &select_backend(
                    BackendKind::Legacy,
                    false,
                    false,
                    "sha256:fixture-source",
                    FallbackPolicy::Refuse,
                ),
                BackendKind::Legacy,
                "sha256:fixture-output",
                ShadowComparisonState::NotRequested,
                diagnostics::DIAGNOSTIC_SCHEMA_VERSION,
            )?,
            build_report: None,
        };
        let files = vec![
            OvenProjectOutputBakeFile {
                source_path: generated_source.clone(),
                caller_relative_path: "generated/main.rs".to_string(),
                output_relative_path: "generated/main.rs".to_string(),
            },
            OvenProjectOutputBakeFile {
                source_path: native_output.clone(),
                caller_relative_path: "target/incan/fixture/oven/release/fixture".to_string(),
                output_relative_path: OVEN_PROJECT_OUTPUT_ARTIFACT_PATH.to_string(),
            },
        ];
        let inconsistent = project_output_payload_for_bake(OvenProjectOutputBakeRequest {
            project_root: project.path(),
            entrypoint: &entrypoint,
            target: OvenBakeProjectTarget::Executable,
            receipt: &receipt,
            plan_identity: "fixture-plan".to_string(),
            profile: "release",
            source_authority_digest: &payload.source_authority_digest,
            lock_dependencies_fingerprint: payload.lock_dependencies_fingerprint.clone(),
            files: files.clone(),
            inspection_authority: OvenProjectInspectionAuthorityRef {
                identity: String::new(),
                receipt_identity: receipt.identity.clone(),
                build_unit_identity: receipt.build_unit_identity.clone(),
            },
            required_project_loafs: Vec::new(),
            package_loaf_store_relative_path: None,
            backend_receipt: payload.backend_receipt.clone(),
            build_report: None,
        });
        let Err(inconsistent) = inconsistent else {
            return Err("empty project inspection authority was accepted".into());
        };
        assert!(inconsistent.message.contains("one exact project inspection authority"));

        let mut current_payload = payload.clone();
        current_payload.lock_dependencies_fingerprint = Some("sha256:fixture-lock-fingerprint".to_string());
        current_payload.build_report = Some(OvenProjectOutputReportSnapshot {
            schema_version: OVEN_PROJECT_OUTPUT_REPORT_SCHEMA_VERSION,
            report: serde_json::json!({ "schema_version": BUILD_REPORT_SCHEMA_VERSION }),
        });
        let current_roundtrip =
            serde_json::from_slice::<OvenProjectOutputPayload>(&serde_json::to_vec(&current_payload)?)?;
        assert_eq!(current_roundtrip.target_identity, current_payload.target_identity);
        assert_eq!(
            current_roundtrip.lock_dependencies_fingerprint,
            current_payload.lock_dependencies_fingerprint
        );
        assert_eq!(current_roundtrip.build_report, current_payload.build_report);
        assert_eq!(
            current_roundtrip.inspection_authority,
            current_payload.inspection_authority
        );

        let mut stale_payload = payload.clone();
        stale_payload.schema_version = OVEN_PROJECT_OUTPUT_PAYLOAD_SCHEMA_VERSION - 1;
        let mut stale_payload_value = serde_json::to_value(&stale_payload)?;
        let stale_payload_object = stale_payload_value
            .as_object_mut()
            .ok_or("serialized project output payload was not an object")?;
        stale_payload_object.remove("target_identity");
        stale_payload_object.remove("lock_dependencies_fingerprint");
        stale_payload_object.remove("build_report");
        let _stale = store.publish(&OvenArtifactPublishRequest {
            receipt: receipt.clone(),
            domain: format!("incan-release-{INCAN_VERSION}"),
            kind: OvenArtifactKind::ProjectOutput,
            payload: serde_json::to_vec(&stale_payload_value)?,
            materialized_files: files
                .iter()
                .map(|file| OvenArtifactMaterializedFile {
                    source_path: file.source_path.clone(),
                    relative_path: file.output_relative_path.clone(),
                })
                .collect(),
        })?;
        assert!(
            select_baked_project_output(
                &store,
                project.path(),
                &entrypoint,
                OvenBakeProjectTarget::Executable,
                "release",
            )?
            .is_none(),
            "a completed output from the previous project-output release-cohort schema must not be replayed"
        );
        publish_project_output_loaf(&store, &receipt, &payload, &files)?;

        // A second explicit publication can arise after an interrupted caller projection. Its byte-level native output
        // need not share the first content address, but it is governed by the same complete source and direct-rustc
        // authority, so normal selection must remain usable.
        let duplicate_native_output = project.path().join("publisher/fixture-republished");
        fs::write(&duplicate_native_output, "republished fixture native output")?;
        let mut duplicate_payload = payload.clone();
        let duplicate_native = duplicate_payload
            .files
            .iter_mut()
            .find(|file| file.output_relative_path == OVEN_PROJECT_OUTPUT_ARTIFACT_PATH)
            .ok_or("duplicate project output should retain a native artifact")?;
        duplicate_native.digest = digest_bytes(&fs::read(&duplicate_native_output)?);
        duplicate_native.logical_bytes = fs::metadata(&duplicate_native_output)?.len();
        let duplicate_files = vec![
            OvenProjectOutputBakeFile {
                source_path: generated_source.clone(),
                caller_relative_path: "generated/main.rs".to_string(),
                output_relative_path: "generated/main.rs".to_string(),
            },
            OvenProjectOutputBakeFile {
                source_path: duplicate_native_output,
                caller_relative_path: "target/incan/fixture/oven/release/fixture".to_string(),
                output_relative_path: OVEN_PROJECT_OUTPUT_ARTIFACT_PATH.to_string(),
            },
        ];
        publish_project_output_loaf(&store, &receipt, &duplicate_payload, &duplicate_files)?;

        let source_authority_digest = digest_baked_project_source_authority(project.path())?;
        assert!(
            select_baked_project_output_with_source_authority(
                &store,
                project.path(),
                &entrypoint,
                OvenBakeProjectTarget::Executable,
                "release",
                &source_authority_digest,
                Some((&receipt.intent.target, "rustc incompatible fixture")),
            )?
            .is_none(),
            "a completed output from another Rust toolchain must not be replayed"
        );
        assert!(
            select_baked_project_output_with_source_authority(
                &store,
                project.path(),
                &entrypoint,
                OvenBakeProjectTarget::Executable,
                "release",
                &source_authority_digest,
                Some((&receipt.intent.target, &receipt.intent.toolchain)),
            )?
            .is_some(),
            "the exact target/toolchain output should remain selectable"
        );

        let selected = select_baked_project_output(
            &store,
            project.path(),
            &entrypoint,
            OvenBakeProjectTarget::Executable,
            "release",
        )?
        .ok_or("exact project output Loaf should be selected")?;
        assert!(selected.payload == payload || selected.payload == duplicate_payload);
        let selected_native = fs::read(&selected.native_output)?;
        assert!(selected_native == b"fixture native output" || selected_native == b"republished fixture native output");
        let original_permissions = fs::metadata(&selected.native_output)?.permissions();
        #[cfg(unix)]
        fs::set_permissions(&selected.native_output, fs::Permissions::from_mode(0o644))?;
        #[cfg(not(unix))]
        {
            let mut writable = original_permissions.clone();
            writable.set_readonly(false);
            fs::set_permissions(&selected.native_output, writable)?;
        }
        let mut corrupted_native = selected_native.clone();
        let Some(first) = corrupted_native.first_mut() else {
            return Err("fixture native output must not be empty".into());
        };
        *first ^= 0xff;
        fs::write(&selected.native_output, &corrupted_native)?;
        let Err(error) = verify_stored_project_output_native(&selected) else {
            return Err("normal run accepted same-length corruption of its store-owned executable".into());
        };
        assert!(error.message.contains("digest differs"));
        fs::write(&selected.native_output, &selected_native)?;
        fs::set_permissions(&selected.native_output, original_permissions)?;
        verify_stored_project_output_native(&selected)?;
        fs::remove_file(&generated_source)?;
        materialize_project_output(project.path(), &selected)?;
        assert_eq!(fs::read(&generated_source)?, b"fn main() {}\n");

        fs::write(&entrypoint, "def main() -> None:\n    return\n")?;
        assert!(
            select_baked_project_output(
                &store,
                project.path(),
                &entrypoint,
                OvenBakeProjectTarget::Executable,
                "release",
            )?
            .is_none()
        );
        let receipt_path = project_bake_receipt_path(
            project.path(),
            OvenBakeProjectTarget::Executable,
            &entrypoint,
            "release",
        )?;
        write_receipt(&receipt, &receipt_path)?;
        assert!(has_stale_baked_project_output(
            &store,
            project.path(),
            &entrypoint,
            OvenBakeProjectTarget::Executable,
            "release",
        )?);

        let unrelated = tempfile::tempdir()?;
        let unrelated_entrypoint = unrelated.path().join("src/main.incn");
        fs::create_dir_all(
            unrelated_entrypoint
                .parent()
                .ok_or("unrelated entrypoint has no parent")?,
        )?;
        fs::write(unrelated.path().join("incan.toml"), "[project]\nname = \"fixture\"\n")?;
        fs::write(&unrelated_entrypoint, "def main() -> None:\n    return\n")?;
        assert!(
            !has_stale_baked_project_output(
                &store,
                unrelated.path(),
                &unrelated_entrypoint,
                OvenBakeProjectTarget::Executable,
                "release",
            )?,
            "a same-named unrelated project without this bake's local receipt must not inherit its stale marker"
        );
        Ok(())
    }

    #[test]
    fn project_output_loaf_restores_without_republishing_its_dependency_closure()
    -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let entrypoint = project.path().join("src/lib.incn");
        let generated_source = project.path().join("target/lib/src/lib.rs");
        let native_output = project.path().join("target/lib/oven/release/libfixture.rlib");
        fs::create_dir_all(entrypoint.parent().ok_or("entrypoint has no parent")?)?;
        fs::create_dir_all(generated_source.parent().ok_or("generated source has no parent")?)?;
        fs::create_dir_all(native_output.parent().ok_or("native output has no parent")?)?;
        fs::write(project.path().join("incan.toml"), "[project]\nname = \"fixture\"\n")?;
        fs::write(&entrypoint, "pub def value() -> int:\n    return 42\n")?;
        fs::write(&generated_source, "pub fn value() -> i64 { 42 }\n")?;
        fs::write(&native_output, "fixture native output")?;

        let rustc = resolve_active_rustc()?;
        let receipt = receipt_generated_project(
            &OvenGeneratedProjectRequest::new(
                project.path(),
                "fixture",
                "0.1.0",
                rustc_host_target(&rustc)?,
                rustc_identity(&rustc)?,
                "release",
                Vec::new(),
            )
            .with_generated_source("generated-root", &generated_source)
            .with_build_unit_input("compiler-version", INCAN_VERSION),
        )?;
        let store_root = tempfile::tempdir()?;
        let store = OvenStore::new(
            store_root.path(),
            crate::oven::store::OvenStoreLimits::new(1024 * 1024, 1024 * 1024, 1024 * 1024),
        );
        let payload = OvenProjectOutputPayload {
            schema_version: OVEN_PROJECT_OUTPUT_PAYLOAD_SCHEMA_VERSION,
            project_target: OvenBakeProjectTarget::Library.as_str().to_string(),
            target_identity: OvenBakeProjectTarget::Library.as_str().to_string(),
            project_identity: baked_project_owner_identity(project.path())?,
            source_authority_digest: digest_baked_project_source_authority(project.path())?,
            lock_dependencies_fingerprint: baked_project_lock_dependencies_fingerprint(project.path())?,
            compiler_version: INCAN_VERSION.to_string(),
            entrypoint_relative_path: "src/lib.incn".to_string(),
            build_unit_identity: receipt.build_unit_identity.clone(),
            receipt_identity: receipt.identity.clone(),
            plan_identity: "fixture-plan".to_string(),
            inspection_authority: Some(OvenProjectInspectionAuthorityRef {
                identity: "fixture-inspection-authority".to_string(),
                receipt_identity: receipt.identity.clone(),
                build_unit_identity: receipt.build_unit_identity.clone(),
            }),
            files: vec![
                OvenProjectOutputFile {
                    caller_relative_path: "target/lib/src/lib.rs".to_string(),
                    output_relative_path: "generated/src/lib.rs".to_string(),
                    digest: digest_bytes(&fs::read(&generated_source)?),
                    logical_bytes: fs::metadata(&generated_source)?.len(),
                },
                OvenProjectOutputFile {
                    caller_relative_path: "target/lib/oven/release/libfixture.rlib".to_string(),
                    output_relative_path: OVEN_PROJECT_OUTPUT_ARTIFACT_PATH.to_string(),
                    digest: digest_bytes(&fs::read(&native_output)?),
                    logical_bytes: fs::metadata(&native_output)?.len(),
                },
            ],
            required_project_loafs: vec![OvenPackagedLibraryLoafEntry {
                receipt: receipt.clone(),
                identity: "fixture-dependency-plan".to_string(),
                kind: OvenArtifactKind::ProjectPayload,
                base_loaf_identity: None,
            }],
            package_loaf_store_relative_path: Some("target/lib/oven/loafs".to_string()),
            backend_receipt: finalize_receipt(
                &select_backend(
                    BackendKind::Legacy,
                    false,
                    false,
                    "sha256:fixture-library-source",
                    FallbackPolicy::Refuse,
                ),
                BackendKind::Legacy,
                "sha256:fixture-library-output",
                ShadowComparisonState::NotRequested,
                diagnostics::DIAGNOSTIC_SCHEMA_VERSION,
            )?,
            build_report: None,
        };
        let files = vec![
            OvenProjectOutputBakeFile {
                source_path: generated_source.clone(),
                caller_relative_path: "target/lib/src/lib.rs".to_string(),
                output_relative_path: "generated/src/lib.rs".to_string(),
            },
            OvenProjectOutputBakeFile {
                source_path: native_output.clone(),
                caller_relative_path: "target/lib/oven/release/libfixture.rlib".to_string(),
                output_relative_path: OVEN_PROJECT_OUTPUT_ARTIFACT_PATH.to_string(),
            },
        ];
        publish_project_output_loaf(&store, &receipt, &payload, &files)?;

        fs::remove_dir_all(project.path().join("target/lib"))?;
        let selected = select_baked_project_output(
            &store,
            project.path(),
            &entrypoint,
            OvenBakeProjectTarget::Library,
            "release",
        )?
        .ok_or("exact library output Loaf should be selected")?;
        materialize_project_output(project.path(), &selected)?;

        assert_eq!(fs::read(&generated_source)?, b"pub fn value() -> i64 { 42 }\n");
        assert_eq!(fs::read(&native_output)?, b"fixture native output");
        assert!(!project.path().join("target/lib/oven/loafs").exists());
        assert!(project_output_projection_is_current(project.path(), &selected)?);
        fs::remove_file(&native_output)?;
        assert!(!project_output_projection_is_current(project.path(), &selected)?);
        materialize_project_output(project.path(), &selected)?;
        assert_eq!(fs::read(&native_output)?, b"fixture native output");
        assert!(project_output_projection_is_current(project.path(), &selected)?);

        let mut tampered = fs::read(&native_output)?;
        let first = tampered.first_mut().ok_or("fixture native output must not be empty")?;
        *first = b'F';
        fs::write(&native_output, &tampered)?;
        assert_eq!(fs::metadata(&native_output)?.len(), payload.files[1].logical_bytes);
        assert!(
            !project_output_projection_is_current(project.path(), &selected)?,
            "same-length caller-output corruption must invalidate the mutable projection"
        );
        materialize_project_output(project.path(), &selected)?;
        assert_eq!(fs::read(&native_output)?, b"fixture native output");
        assert!(project_output_projection_is_current(project.path(), &selected)?);
        Ok(())
    }

    #[test]
    fn project_output_loaf_retains_durable_generated_files_but_not_inspection_cache()
    -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let output_root = project.path().join("target/incan/fixture");
        let generator = ProjectGenerator::new(&output_root, "fixture", false);
        generator.generate("pub fn answer() -> i64 { 42 }\n")?;
        fs::write(output_root.join("Cargo.lock"), "version = 4\n")?;
        let inspection_cache = output_root.join("oven/rust-inspect/metadata.json");
        fs::create_dir_all(inspection_cache.parent().ok_or("inspection cache has no parent")?)?;
        fs::write(&inspection_cache, "mutable inspection cache")?;
        let native_output = output_root.join("oven/release/libfixture.rlib");
        fs::create_dir_all(native_output.parent().ok_or("native output has no parent")?)?;
        fs::write(&native_output, "native output")?;
        let desugarer = output_root.join("desugarers/fixture.wasm");
        fs::create_dir_all(desugarer.parent().ok_or("desugarer has no parent")?)?;
        fs::write(&desugarer, "sealed vocab desugarer")?;

        let files = project_output_bake_files(
            project.path(),
            &generator,
            &native_output,
            None,
            None,
            &[(
                desugarer,
                "generated/provider-sidecars/desugarers/fixture.wasm".to_string(),
            )],
        )?;
        let output_paths = files
            .iter()
            .map(|file| file.output_relative_path.as_str())
            .collect::<BTreeSet<_>>();
        assert!(output_paths.contains("generated/Cargo.toml"));
        assert!(
            !output_paths.contains("generated/Cargo.lock"),
            "the explicit publisher lock must not become a caller-visible completed-output artifact"
        );
        assert!(output_paths.contains("generated/src/lib.rs"));
        assert!(output_paths.contains("generated/provider-sidecars/desugarers/fixture.wasm"));
        assert!(output_paths.contains(OVEN_PROJECT_OUTPUT_ARTIFACT_PATH));
        assert!(output_paths.iter().all(|path| !path.contains("rust-inspect")));
        Ok(())
    }

    #[test]
    fn baked_library_reuse_requires_completed_project_outputs_beside_package_loafs()
    -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let artifact_root = project.path().join("target/lib");
        let generated_source = artifact_root.join("src/lib.rs");
        fs::create_dir_all(project.path().join("src"))?;
        fs::create_dir_all(generated_source.parent().ok_or("generated source has no parent")?)?;
        fs::write(
            project.path().join("incan.toml"),
            "[project]\nname = \"fixture\"\nversion = \"0.1.0\"\n",
        )?;
        fs::write(
            project.path().join("src/lib.incn"),
            "pub def value() -> int:\n    return 1\n",
        )?;
        fs::write(&generated_source, "pub fn value() -> i32 { 1 }\n")?;
        let library_manifest = LibraryManifest::new("fixture", "0.1.0");
        let library_manifest_path = artifact_root.join("fixture.incnlib");
        library_manifest.write_to_path(&library_manifest_path)?;

        let rustc = resolve_active_rustc()?;
        let target = rustc_host_target(&rustc)?;
        let toolchain = rustc_identity(&rustc)?;
        let limits = crate::oven::store::OvenStoreLimits::new(1024 * 1024, 1024 * 1024, 1024 * 1024);
        let package_store = OvenStore::new(packaged_library_loaf_store_root(&artifact_root), limits);
        let mut profiles = BTreeMap::new();
        for profile in ["debug", "release"] {
            let receipt = receipt_generated_project(
                &OvenGeneratedProjectRequest::new(
                    &artifact_root,
                    "fixture",
                    "0.1.0",
                    target.clone(),
                    toolchain.clone(),
                    profile,
                    Vec::new(),
                )
                .with_generated_source("generated-root", &generated_source)
                .with_build_unit_input("compiler-version", INCAN_VERSION),
            )?;
            let stored = package_store.publish(&OvenArtifactPublishRequest {
                receipt: receipt.clone(),
                domain: "fixture".to_string(),
                kind: OvenArtifactKind::ProjectPayload,
                payload: format!("fixture-{profile}-payload").into_bytes(),
                materialized_files: Vec::new(),
            })?;
            let output = artifact_root.join(format!("oven/{profile}/libfixture.rlib"));
            fs::create_dir_all(output.parent().ok_or("library output has no parent")?)?;
            fs::write(&output, format!("fixture-{profile}-library"))?;
            profiles.insert(
                profile.to_string(),
                OvenPackagedLibraryLoafProfile {
                    receipt: receipt.clone(),
                    entries: vec![OvenPackagedLibraryLoafEntry {
                        receipt,
                        identity: stored.identity,
                        kind: OvenArtifactKind::ProjectPayload,
                        base_loaf_identity: None,
                    }],
                    library_relative_path: format!("oven/{profile}/libfixture.rlib"),
                    library_digest: digest_bytes(&fs::read(output)?),
                },
            );
        }
        let manifest = OvenPackagedLibraryLoafManifest {
            schema_version: OVEN_PACKAGED_LIBRARY_LOAF_SCHEMA_VERSION,
            source_authority_digest: digest_baked_project_source_authority(project.path())?,
            compiler_version: INCAN_VERSION.to_string(),
            metadata_files: packaged_library_metadata_files(&library_manifest_path, &library_manifest, &artifact_root)?,
            profiles,
        };
        write_packaged_library_loaf_manifest(&artifact_root, &manifest)?;
        assert!(oven_library_dependency_declares_package_loaf(project.path()));
        let consumer_store = OvenStore::new(project.path().join("consumer-store"), limits);
        let targets = vec![(OvenBakeProjectTarget::Library, project.path().join("src/lib.incn"))];
        let mut authority_context = OvenProjectBakeAuthorityContext::default();

        assert!(
            try_reuse_baked_project(
                project.path(),
                &targets,
                &consumer_store,
                &FeatureSelection::default(),
                &mut authority_context,
            )?
            .is_none(),
            "a package Loaf alone cannot skip producing this project's completed outputs"
        );
        fs::remove_file(packaged_library_loaf_manifest_path(&artifact_root))?;
        assert!(!oven_library_dependency_declares_package_loaf(project.path()));
        Ok(())
    }

    #[test]
    fn dependency_artifact_only_build_skips_canonical_lock_issue908() {
        assert!(dependency_artifact_skips_canonical_lock(true, false));
        assert!(!dependency_artifact_skips_canonical_lock(true, true));
        assert!(!dependency_artifact_skips_canonical_lock(false, false));
    }

    #[test]
    fn loaf_enables_the_complete_stdlib_runtime_envelope() {
        let mut seeded = vec!["json".to_string()];
        ensure_loaf_stdlib_features(&mut seeded, true);
        assert_eq!(seeded, ["async", "json", "ordinal", "web"]);

        let mut ordinary = vec!["json".to_string()];
        ensure_loaf_stdlib_features(&mut ordinary, false);
        assert_eq!(ordinary, ["json"]);
    }

    #[test]
    fn normal_oven_source_dependencies_keep_their_public_implementation_closure() {
        assert!(preserve_source_dependency_public_items(false, 1));
        assert!(preserve_source_dependency_public_items(true, 0));
        assert!(!preserve_source_dependency_public_items(false, 0));
    }

    #[test]
    fn normal_oven_build_inputs_require_a_current_selected_interop_receipt() -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        fs::create_dir_all(project.path().join("interop/include"))?;
        let header = project.path().join("interop/include/bridge.h");
        fs::write(&header, "int incan_bridge(void);\n")?;
        let manifest_source = r#"
[oven.interop]
schema = 1

[[oven.interop.targets]]
target = "aarch64-apple-darwin"
toolchain = { capability = "apple-clang", version = ">=17, <18" }
sdk = { capability = "macosx", version = ">=18, <19" }
headers = ["interop/include/bridge.h"]
"#;
        let manifest_path = project.path().join("incan.toml");
        fs::write(&manifest_path, manifest_source)?;
        let manifest = ProjectManifest::from_str(manifest_source, &manifest_path)?;
        let locked = locked_oven_interop_targets(&manifest)?;
        IncanLock::new_with_semantic(
            "fixture".to_string(),
            CargoFeatureSelection::default(),
            SemanticLockState {
                oven: Some(LockedOvenState {
                    interop: locked.clone(),
                }),
                ..SemanticLockState::default()
            },
            String::new(),
        )
        .write(&project.path().join("incan.lock"))?;
        let receipt = receipt_interop_execution(
            &locked[0],
            Some(OvenInteropCapabilitySelection {
                capability: "apple-clang".to_string(),
                version: "17.0.6".to_string(),
                identity: "sha256:clang".to_string(),
            }),
            Some(OvenInteropCapabilitySelection {
                capability: "macosx".to_string(),
                version: "18.5.0".to_string(),
                identity: "sha256:sdk".to_string(),
            }),
        )?;
        write_interop_execution_receipt(
            &receipt,
            default_interop_execution_receipt_path(project.path(), "aarch64-apple-darwin"),
        )?;
        let mut inputs = BTreeMap::new();
        append_oven_interop_execution_build_inputs(&mut inputs, Some(&manifest), "aarch64-apple-darwin")?;
        assert_eq!(inputs, interop_execution_build_unit_inputs(&receipt));

        fs::write(header, "int incan_bridge_changed(void);\n")?;
        assert!(
            append_oven_interop_execution_build_inputs(&mut BTreeMap::new(), Some(&manifest), "aarch64-apple-darwin")
                .is_err()
        );
        Ok(())
    }

    #[test]
    fn interop_receipt_miss_never_materializes_a_generic_toolchain_loaf() -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let source = project.path().join("lib.rs");
        fs::write(&source, "pub fn fixture() {}\n")?;
        let receipt = receipt_generated_project(
            &OvenGeneratedProjectRequest::new(
                project.path(),
                "interop-miss",
                "0.1.0",
                "aarch64-apple-darwin",
                "rustc fixture",
                "debug",
                Vec::new(),
            )
            .with_generated_source("lib.rs", &source)
            .with_build_unit_input(OVEN_INTEROP_EXECUTION_RECEIPT_INPUT, "sha256:selected-interop"),
        )?;
        let store = OvenStore::new(
            project.path().join("oven-store"),
            crate::oven::store::OvenStoreLimits::new(1024 * 1024, 1024 * 1024, 1024 * 1024),
        );

        let selection = select_oven_direct_rustc_plan(&store, &receipt, &[]);
        let Err(error) = selection else {
            return Err("an interop receipt miss must not materialize a generic Loaf".into());
        };
        assert!(error.to_string().contains("incan oven interop bake"));
        let entries = project.path().join("oven-store/entries");
        match fs::read_dir(&entries) {
            Ok(mut entries) => assert!(entries.next().is_none()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        Ok(())
    }

    #[test]
    fn explicit_project_bake_publishes_a_generated_receipt_closure() -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let generated_root = project.path().join("src/main.rs");
        let dependency_root = project.path().join("dependency");
        fs::create_dir_all(generated_root.parent().ok_or("generated root has no parent")?)?;
        fs::create_dir_all(dependency_root.join("src"))?;
        fs::write(
            project.path().join("Cargo.toml"),
            "[package]\nname = \"oven_explicit_bake_fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[dependencies]\noven_bake_dependency = { path = \"dependency\" }\n",
        )?;
        fs::write(
            dependency_root.join("Cargo.toml"),
            "[package]\nname = \"oven_bake_dependency\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )?;
        fs::write(dependency_root.join("src/lib.rs"), "pub fn value() -> u8 { 7 }\n")?;
        fs::write(
            &generated_root,
            "fn main() { let _ = oven_bake_dependency::value(); }\n",
        )?;

        let rustc = resolve_active_rustc()?;
        let receipt = receipt_generated_project(
            &OvenGeneratedProjectRequest::new(
                project.path(),
                "oven_explicit_bake_fixture",
                "0.1.0",
                rustc_host_target(&rustc)?,
                rustc_identity(&rustc)?,
                "debug",
                Vec::new(),
            )
            .with_generated_source("generated-root", &generated_root)
            .with_build_unit_input("provider-plan", digest_bytes(b"")),
        )?;
        let store = OvenStore::new(
            project.path().join("oven-store"),
            crate::oven::store::OvenStoreLimits::new(1024 * 1024 * 1024, 1024 * 1024 * 1024, 1024 * 1024 * 1024),
        );

        let consume_only = select_or_bake_generated_project_plan(
            OvenProjectPlanMode::ConsumeOnly,
            &store,
            &receipt,
            OvenProjectDependencySurface { selection: &[] },
            project.path(),
            &generated_root,
            &rustc,
        );
        let compiler_suite_native = env::var_os("INCAN_INTERNAL_OVEN_LOAF_EXECUTION").is_some_and(|value| value == "1");
        if compiler_suite_native {
            let Err(error) = consume_only else {
                return Err("a compiler-suite normal consumer must reject a caller-owned Loaf miss".into());
            };
            let message = error.to_string();
            assert!(
                message.contains(OVEN_NESTED_DEPENDENCY_MISS_SUMMARY)
                    && message.contains(OVEN_NO_IMPLICIT_DEPENDENCY_BUILD),
                "compiler-suite normal consumers must remain Cargo-free even when the explicit baker is tested, got: {message}"
            );
        } else {
            let consume_only = consume_only?;
            assert!(
                consume_only.is_none(),
                "a normal consumer must not invoke the compatibility baker on a miss"
            );
        }
        assert!(
            !project.path().join("Cargo.lock").exists(),
            "a consume-only normal command must not create Cargo publisher state"
        );
        fs::write(
            project.path().join("Cargo.lock"),
            "version = 4\n\n[[package]]\nname = \"oven_bake_dependency\"\nversion = \"0.1.0\"\n\n[[package]]\nname = \"oven_explicit_bake_fixture\"\nversion = \"0.1.0\"\ndependencies = [\"oven_bake_dependency\"]\n",
        )?;

        let first = select_or_bake_generated_project_plan(
            OvenProjectPlanMode::ExplicitBake,
            &store,
            &receipt,
            OvenProjectDependencySurface { selection: &[] },
            project.path(),
            &generated_root,
            &rustc,
        )?
        .ok_or("explicit Oven bake did not select its published plan")?;
        assert_eq!(first.materialization, OvenToolchainMaterialization::CompatibilityBaked);
        assert!(project.path().join("Cargo.lock").is_file());
        let OvenDirectRustcPlanSelection::Stored(first_stored) = &first.plan_selection else {
            return Err("an explicit project bake must select its project Loaf".into());
        };
        let loaf_root = first_stored
            .artifact_root
            .parent()
            .ok_or("a project Loaf artifact root must have its owning entry directory")?;
        assert_eq!(
            loaf_root.extension().and_then(|extension| extension.to_str()),
            Some("loaf")
        );
        assert!(loaf_root.join("loaf.json").is_file());

        let second = select_or_bake_generated_project_plan(
            OvenProjectPlanMode::ExplicitBake,
            &store,
            &receipt,
            OvenProjectDependencySurface { selection: &[] },
            project.path(),
            &generated_root,
            &rustc,
        )?
        .ok_or("repeated explicit Oven bake lost its selected plan")?;
        assert_eq!(second.materialization, OvenToolchainMaterialization::Reused);
        assert_eq!(
            first.plan_selection.report_identity(),
            second.plan_selection.report_identity(),
            "an unchanged project receipt must reuse the already published direct-rustc plan"
        );
        Ok(())
    }

    #[cfg(feature = "rust_inspect")]
    #[test]
    fn artifact_only_provider_preserves_required_rust_call_metadata() {
        assert!(library_rust_inspection_required(
            true,
            &["rustix::fs::flock".to_string()]
        ));
        assert!(!library_rust_inspection_required(true, &[]));
        assert!(library_rust_inspection_required(false, &[]));
    }

    #[test]
    fn oven_normal_commands_keep_lock_strictness_but_reject_cargo_backend_controls() {
        assert!(reject_normal_cargo_controls(&CargoPolicy::explicit(true, false, false, Vec::new()), None).is_ok());
        assert!(reject_normal_cargo_controls(&CargoPolicy::explicit(false, true, false, Vec::new()), None).is_ok());
        assert!(reject_normal_cargo_controls(&CargoPolicy::explicit(false, false, true, Vec::new()), None).is_ok());
        assert!(
            reject_normal_cargo_controls(
                &CargoPolicy::explicit(false, false, false, vec!["--timings".to_string()]),
                None,
            )
            .is_err()
        );
        assert!(
            reject_normal_cargo_controls(&CargoPolicy::default(), Some(&PathBuf::from("target/generated-cargo")),)
                .is_err()
        );
    }

    #[test]
    fn source_free_provider_artifact_mints_a_verified_oven_receipt() -> Result<(), Box<dyn std::error::Error>> {
        let workspace = tempfile::tempdir()?;
        let project_root = workspace.path();
        let artifact_root = project_root.join("target/lib");
        let source_root = artifact_root.join("src");
        fs::create_dir_all(&source_root)?;
        let crate_lib_path = source_root.join("lib.rs");
        fs::write(&crate_lib_path, "pub fn provider() {}\n")?;
        fs::write(
            artifact_root.join("Cargo.toml"),
            "[package]\nname = \"provider\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )?;
        let manifest_path = artifact_root.join("provider.incnlib");
        LibraryManifest::new("provider", "0.1.0").write_to_path(&manifest_path)?;
        let artifact = LibraryArtifactMetadata {
            dependency_key: "provider".to_string(),
            manifest_name: "provider".to_string(),
            manifest_path,
            crate_root: artifact_root.clone(),
            cargo_toml_path: artifact_root.join("Cargo.toml"),
            crate_lib_path,
            kind: LibraryArtifactKind::Materialized,
        };
        let intent = crate::oven::OvenBuildIntent {
            target: "aarch64-apple-darwin".to_string(),
            toolchain: "rustc test".to_string(),
            profile: "debug".to_string(),
            features: Vec::new(),
        };
        let receipt_path = crate::oven::default_receipt_path(project_root).with_file_name("library-debug-receipt.json");

        let receipt = mint_artifact_only_library_receipt(&artifact, project_root, "debug", &intent, &receipt_path)?;

        assert_eq!(receipt.intent, intent);
        assert!(receipt_path.is_file());
        receipt.verify_identity()?;
        Ok(())
    }

    #[test]
    fn caller_owned_provider_receipt_rebinds_to_the_selected_consumer_cohort() -> Result<(), Box<dyn std::error::Error>>
    {
        let workspace = tempfile::tempdir()?;
        let artifact_root = workspace.path().join("target/lib");
        fs::create_dir_all(artifact_root.join("src"))?;
        fs::write(
            artifact_root.join("Cargo.toml"),
            "[package]\nname = \"provider\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )?;
        let crate_lib_path = artifact_root.join("src/lib.rs");
        fs::write(&crate_lib_path, "pub fn provider() {}\n")?;
        let manifest_path = artifact_root.join("provider.incnlib");
        LibraryManifest::new("provider", "0.1.0").write_to_path(&manifest_path)?;
        let artifact = LibraryArtifactMetadata {
            dependency_key: "provider".to_string(),
            manifest_name: "provider".to_string(),
            manifest_path: manifest_path.clone(),
            crate_root: artifact_root.clone(),
            cargo_toml_path: artifact_root.join("Cargo.toml"),
            crate_lib_path: crate_lib_path.clone(),
            kind: LibraryArtifactKind::Materialized,
        };
        let producer_request = OvenGeneratedProjectRequest::new(
            workspace.path(),
            "provider",
            "0.1.0",
            "aarch64-apple-darwin",
            "rustc 1.95.0",
            "debug",
            Vec::new(),
        )
        .with_generated_source("generated-root", &crate_lib_path)
        .with_generated_source_tree("generated-source-tree", artifact_root.join("src"))
        .with_generated_source("provider-contract", &manifest_path);
        let producer_receipt = receipt_generated_project(&producer_request)?;
        let consumer_intent = crate::oven::OvenBuildIntent {
            target: "aarch64-apple-darwin".to_string(),
            toolchain: "rustc 1.99.0-nightly".to_string(),
            profile: "debug".to_string(),
            features: Vec::new(),
        };

        let rebound = rebind_caller_owned_library_receipt(&artifact, "debug", &consumer_intent, &producer_receipt)?;

        assert_eq!(rebound.intent, consumer_intent);
        assert_eq!(
            rebound.sources.build_unit_inputs.get("producer-library-receipt"),
            Some(&producer_receipt.identity)
        );
        rebound.verify_identity()?;
        Ok(())
    }

    #[test]
    fn selected_plan_registry_extern_is_not_materialized_as_caller_owned() {
        let registry = |crate_name: &str| DependencySpec {
            crate_name: crate_name.to_string(),
            version: Some("1".to_string()),
            features: Vec::new(),
            default_features: true,
            source: DependencySource::Registry,
            optional: false,
            package: None,
        };
        let path = DependencySpec {
            crate_name: "serde_json".to_string(),
            version: None,
            features: Vec::new(),
            default_features: true,
            source: DependencySource::Path {
                path: PathBuf::from("caller-owned-serde-json"),
            },
            optional: false,
            package: None,
        };
        let plan = OvenRustcArtifactPlan {
            dependency_search_paths: Vec::new(),
            native_search_paths: Vec::new(),
            externs: vec![("serde_json".to_string(), PathBuf::from("sealed/serde_json.rlib"))],
            compile_environment: BTreeMap::new(),
            caller_owned_library_digests: BTreeMap::new(),
        };

        let remaining = declared_rust_libraries_missing_from_selected_plan(
            &[registry("serde-json"), registry("regex"), path.clone()],
            &plan,
        );

        assert_eq!(
            remaining,
            vec![registry("regex"), path],
            "only the compiler-owned registry leaf may be omitted; a caller path dependency remains explicit"
        );
    }

    #[test]
    fn current_project_plan_reuses_its_sealed_direct_path_extern() {
        let path = DependencySpec {
            crate_name: "receiver_factory".to_string(),
            version: None,
            features: Vec::new(),
            default_features: true,
            source: DependencySource::Path {
                path: PathBuf::from("current-project-receiver-factory"),
            },
            optional: false,
            package: None,
        };
        let plan = OvenRustcArtifactPlan {
            dependency_search_paths: Vec::new(),
            native_search_paths: Vec::new(),
            externs: vec![(
                "receiver_factory".to_string(),
                PathBuf::from("sealed/receiver_factory.rlib"),
            )],
            compile_environment: BTreeMap::new(),
            caller_owned_library_digests: BTreeMap::new(),
        };

        let remaining =
            declared_rust_libraries_missing_from_selected_plan_with_current_project_paths(&[path], &plan, true);

        assert!(
            remaining.is_empty(),
            "a receipt-selected project plan must reuse its own sealed direct path extern"
        );
    }

    #[test]
    fn selected_registry_extern_still_requires_a_compatible_sealed_version() -> Result<(), Box<dyn std::error::Error>> {
        use sha2::{Digest, Sha256};

        let root = tempfile::tempdir()?;
        let relative_path = "debug/deps/libserde_json.rlib";
        let artifact_path = root.path().join(relative_path);
        let artifact_parent = artifact_path.parent().ok_or("artifact parent")?;
        fs::create_dir_all(artifact_parent)?;
        let bytes = b"sealed serde_json 1.0.0";
        fs::write(&artifact_path, bytes)?;
        let authority = OvenRegistryLeafAuthority::new(
            root.path().to_path_buf(),
            vec![crate::oven::rustc::OvenRustcRegistryLeaf {
                package: "serde_json".to_string(),
                version: "1.0.0".to_string(),
                crate_name: "serde_json".to_string(),
                features: Vec::new(),
                source: crate::oven::rustc::OvenRustcRegistrySource {
                    registry: "registry+https://example.invalid/index".to_string(),
                    checksum: "fixture-checksum".to_string(),
                    relative_root: "registry-sources/serde-json".to_string(),
                    digest: crate::oven::digest_bytes(b"fixture registry source"),
                },
                artifact: crate::oven::rustc::OvenRustcArtifactExtern {
                    crate_name: "serde_json".to_string(),
                    relative_path: relative_path.to_string(),
                    digest: format!("sha256:{}", hex::encode(Sha256::digest(bytes))),
                },
            }],
        );
        let dependency = |version: &str| DependencySpec {
            crate_name: "serde_json".to_string(),
            version: Some(version.to_string()),
            features: Vec::new(),
            default_features: true,
            source: DependencySource::Registry,
            optional: false,
            package: None,
        };
        let selected_plan = OvenRustcArtifactPlan {
            dependency_search_paths: Vec::new(),
            native_search_paths: Vec::new(),
            externs: vec![("serde_json".to_string(), artifact_path)],
            compile_environment: BTreeMap::new(),
            caller_owned_library_digests: BTreeMap::new(),
        };

        validate_selected_plan_registry_dependencies(&[dependency("1.0")], &selected_plan, Some(&authority), "debug")?;
        let error = match validate_selected_plan_registry_dependencies(
            &[dependency("999.0.0")],
            &selected_plan,
            Some(&authority),
            "debug",
        ) {
            Ok(()) => return Err("an incompatible declared version borrowed a selected extern by crate name".into()),
            Err(error) => error,
        };
        assert!(
            error.to_string().contains("999.0.0"),
            "unexpected version diagnostic: {error}"
        );
        Ok(())
    }

    #[test]
    fn selected_scheduler_owned_path_extern_is_not_materialized_as_inline_library()
    -> Result<(), Box<dyn std::error::Error>> {
        let workspace = tempfile::tempdir()?;
        let provider_root = workspace.path().join("sealed-providers");
        let scheduler_path = provider_root.join("components/stdlib-core");
        let caller_path = workspace.path().join("caller/incan_stdlib_core");
        fs::create_dir_all(&scheduler_path)?;
        fs::create_dir_all(&caller_path)?;
        let scheduler_dependency = DependencySpec {
            crate_name: "incan_stdlib_core".to_string(),
            version: None,
            features: Vec::new(),
            default_features: false,
            source: DependencySource::Path { path: scheduler_path },
            optional: false,
            package: None,
        };
        let caller_dependency = DependencySpec {
            source: DependencySource::Path { path: caller_path },
            ..scheduler_dependency.clone()
        };
        let selected_externs = BTreeSet::from(["incan_stdlib_core".to_string()]);

        let remaining = declared_rust_libraries_missing_from_selected_plan_with_owned_roots(
            &[scheduler_dependency, caller_dependency.clone()],
            &selected_externs,
            &[fs::canonicalize(provider_root)?],
        );

        assert_eq!(remaining, vec![caller_dependency]);
        Ok(())
    }

    #[test]
    fn caller_owned_provider_manifest_dependencies_are_explicit_direct_rustc_inputs()
    -> Result<(), Box<dyn std::error::Error>> {
        let workspace = tempfile::tempdir()?;
        let artifact_root = workspace.path().join("target/lib");
        let toolchain_root = workspace.path().join("toolchain");
        fs::create_dir_all(artifact_root.join("src"))?;
        fs::create_dir_all(toolchain_root.join("incan_stdlib"))?;
        fs::write(
            artifact_root.join("Cargo.toml"),
            "[package]\nname = \"provider\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[dependencies]\nincan_stdlib = { path = \"../../toolchain/incan_stdlib\" }\nserde = { version = \"1.0\", features = [\"derive\"] }\nrust_shadow = { path = \"../../rust_shadow\" }\n",
        )?;
        fs::write(artifact_root.join("src/lib.rs"), "pub fn marker() {}\n")?;
        let artifact = LibraryArtifactMetadata {
            dependency_key: "provider".to_string(),
            manifest_name: "provider".to_string(),
            manifest_path: workspace.path().join("provider.incnlib"),
            crate_root: artifact_root.clone(),
            cargo_toml_path: artifact_root.join("Cargo.toml"),
            crate_lib_path: artifact_root.join("src/lib.rs"),
            kind: LibraryArtifactKind::Materialized,
        };

        let dependencies = caller_owned_library_rust_dependencies(&artifact)?;
        assert_eq!(dependencies.len(), 3);
        assert!(dependencies.iter().any(|dependency| {
            dependency.crate_name == "serde"
                && dependency.version.as_deref() == Some("1.0")
                && dependency.features == ["derive"]
                && dependency.source == DependencySource::Registry
        }));
        assert!(dependencies.iter().any(|dependency| {
            dependency.crate_name == "rust_shadow"
                && matches!(
                    &dependency.source,
                    DependencySource::Path { path } if path == &artifact_root.join("../../rust_shadow")
                )
        }));

        let plan = OvenRustcArtifactPlan {
            dependency_search_paths: Vec::new(),
            native_search_paths: Vec::new(),
            externs: vec![(
                "incan_stdlib".to_string(),
                workspace.path().join("sealed/incan_stdlib.rlib"),
            )],
            compile_environment: BTreeMap::new(),
            caller_owned_library_digests: BTreeMap::new(),
        };
        let remaining = caller_owned_library_dependencies_missing_from_selected_plan_with_owned_roots(
            &dependencies,
            &plan,
            &[fs::canonicalize(&toolchain_root)?],
        );
        assert_eq!(remaining.len(), 2);
        assert!(remaining.iter().any(|dependency| dependency.crate_name == "serde"));
        assert!(
            remaining
                .iter()
                .any(|dependency| dependency.crate_name == "rust_shadow")
        );
        Ok(())
    }

    #[test]
    fn selected_compiler_runtime_path_is_not_rematerialized_as_a_caller_dependency()
    -> Result<(), Box<dyn std::error::Error>> {
        let workspace = tempfile::tempdir()?;
        let toolchain_data_root = workspace.path().join("toolchain-data");
        let runtime_root = workspace.path().join("sdk-runtime");
        let provider_root = workspace.path().join("sdk-providers");
        let runtime_path = runtime_root.join("crates/incan_stdlib");
        let component_path = provider_root.join("components/stdlib-core");
        let caller_path = workspace.path().join("caller/incan_stdlib");
        fs::create_dir_all(&runtime_path)?;
        fs::create_dir_all(&component_path)?;
        fs::create_dir_all(&caller_path)?;
        let runtime = DependencySpec {
            crate_name: "incan_stdlib".to_string(),
            version: None,
            features: Vec::new(),
            default_features: true,
            source: DependencySource::Path {
                path: runtime_path.clone(),
            },
            optional: false,
            package: None,
        };
        let caller = DependencySpec {
            source: DependencySource::Path {
                path: caller_path.clone(),
            },
            ..runtime.clone()
        };
        let component = DependencySpec {
            crate_name: "incan_stdlib_core".to_string(),
            source: DependencySource::Path {
                path: component_path.clone(),
            },
            ..runtime.clone()
        };
        let selected_names = BTreeSet::from(["incan_stdlib", "incan_stdlib_core"]);
        fs::create_dir_all(toolchain_data_root.join("share/incan/oven/loafs"))?;
        let owned_roots = vec![
            fs::canonicalize(&toolchain_data_root)?,
            fs::canonicalize(&runtime_root)?,
            fs::canonicalize(&provider_root)?,
        ];

        assert!(is_selected_compiler_runtime_path_dependency(
            &runtime,
            &selected_names,
            &owned_roots,
        ));
        assert!(is_selected_compiler_runtime_path_dependency(
            &component,
            &selected_names,
            &owned_roots,
        ));
        assert!(!is_selected_compiler_runtime_path_dependency(
            &caller,
            &selected_names,
            &owned_roots,
        ));
        let plan = OvenRustcArtifactPlan {
            dependency_search_paths: Vec::new(),
            native_search_paths: Vec::new(),
            externs: vec![
                (
                    "incan_stdlib".to_string(),
                    workspace.path().join("sealed/incan_stdlib.rlib"),
                ),
                (
                    "incan_stdlib_core".to_string(),
                    workspace.path().join("sealed/incan_stdlib_core.rlib"),
                ),
            ],
            compile_environment: BTreeMap::new(),
            caller_owned_library_digests: BTreeMap::new(),
        };
        let remaining = caller_owned_library_dependencies_missing_from_selected_plan_with_owned_roots(
            &[runtime, component, caller.clone()],
            &plan,
            &owned_roots,
        );
        assert_eq!(remaining, vec![caller]);
        Ok(())
    }

    #[test]
    fn checked_historical_sdk_rebinding_reuses_the_selected_sealed_extern() -> Result<(), Box<dyn std::error::Error>> {
        use crate::provider::{NamespaceAuthority, ProviderIdentity, ProviderProvenance, ProviderRecord};

        let workspace = tempfile::tempdir()?;
        let library_root = workspace.path().join("library/target/lib");
        let historical_sdk_root = library_root.join("private/stdlib-core");
        let active_sdk_root = workspace.path().join("active-sdk/stdlib-core");
        let caller_lookalike = workspace.path().join("caller/incan_stdlib_core");
        for root in [&library_root, &historical_sdk_root, &active_sdk_root, &caller_lookalike] {
            fs::create_dir_all(root.join("src"))?;
            fs::write(
                root.join("Cargo.toml"),
                "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
            )?;
            fs::write(root.join("src/lib.rs"), "pub fn marker() {}\n")?;
        }

        let active_digest = digest_provider_artifact(&active_sdk_root)?;
        let mut library_manifest = LibraryManifest::new("library", "0.1.0");
        library_manifest
            .contract_metadata
            .provider
            .provider_dependencies
            .push(ProviderDependencyMetadata {
                kind: ProviderDependencyKind::PrivateImplementation,
                dependency_key: "incan_stdlib_core".to_string(),
                provider_name: "incan_stdlib_core".to_string(),
                provider_version: "0.5.0".to_string(),
                artifact_digest: active_digest.clone(),
                relative_artifact_path: "private/stdlib-core".to_string(),
                requested_features: BTreeSet::new(),
                default_features: false,
                optional: false,
            });
        let library_manifest_path = library_root.join("library.incnlib");
        library_manifest.write_to_path(&library_manifest_path)?;
        let library_artifact = LibraryArtifactMetadata::from_crate_root("library", "library", &library_root);
        let active_sdk_artifact =
            LibraryArtifactMetadata::from_crate_root("incan_stdlib_core", "incan_stdlib_core", &active_sdk_root);
        let provider_plan = ProviderPlan::new(
            LibraryManifestIndex::default(),
            vec![
                ProviderRecord {
                    identity: ProviderIdentity {
                        name: "library".to_string(),
                        version: "0.1.0".to_string(),
                        digest: digest_provider_artifact(&library_root)?,
                        feature_projection: BTreeSet::new(),
                    },
                    provenance: ProviderProvenance::ProjectDependency {
                        dependency_key: "library".to_string(),
                        manifest_path: library_manifest_path,
                    },
                    authority: NamespaceAuthority::ProjectDependency {
                        dependency_key: "library".to_string(),
                    },
                    namespace_claims: BTreeSet::new(),
                    available: true,
                    enabled: true,
                    manifest: Some(Arc::new(library_manifest)),
                    artifact: Some(library_artifact),
                    implementation_facets: Vec::new(),
                },
                ProviderRecord {
                    identity: ProviderIdentity {
                        name: "incan_stdlib_core".to_string(),
                        version: "0.5.0".to_string(),
                        digest: active_digest,
                        feature_projection: BTreeSet::new(),
                    },
                    provenance: ProviderProvenance::Sdk {
                        sdk_identity: "incan@0.5.1-rc2".to_string(),
                        component_id: "stdlib-core".to_string(),
                        inventory_path: None,
                    },
                    authority: NamespaceAuthority::SdkReserved,
                    namespace_claims: BTreeSet::new(),
                    available: true,
                    enabled: true,
                    manifest: Some(Arc::new(LibraryManifest::new("incan_stdlib_core", "0.5.0"))),
                    artifact: Some(active_sdk_artifact),
                    implementation_facets: Vec::new(),
                },
            ],
            [],
        )?;
        let artifact_plan = OvenRustcArtifactPlan {
            dependency_search_paths: Vec::new(),
            native_search_paths: Vec::new(),
            externs: vec![(
                "incan_stdlib_core".to_string(),
                workspace.path().join("sealed/incan_stdlib_core.rlib"),
            )],
            compile_environment: BTreeMap::new(),
            caller_owned_library_digests: BTreeMap::new(),
        };

        let owned_roots = compiler_owned_roots_with_provider_plan(&artifact_plan, Some(&provider_plan));
        assert!(
            owned_roots.contains(&fs::canonicalize(&historical_sdk_root)?),
            "the provider plan verified this stale coordinate against the active SDK artifact"
        );
        let historical_dependency = DependencySpec {
            crate_name: "incan_stdlib_core".to_string(),
            version: None,
            features: Vec::new(),
            default_features: false,
            source: DependencySource::Path {
                path: historical_sdk_root,
            },
            optional: false,
            package: None,
        };
        let lookalike_dependency = DependencySpec {
            source: DependencySource::Path { path: caller_lookalike },
            ..historical_dependency.clone()
        };
        let remaining = caller_owned_library_dependencies_missing_from_selected_plan_with_owned_roots(
            &[historical_dependency, lookalike_dependency.clone()],
            &artifact_plan,
            &owned_roots,
        );
        assert_eq!(remaining, vec![lookalike_dependency]);
        Ok(())
    }

    #[test]
    fn public_provider_registry_closure_constrains_loaf_selection() -> Result<(), Box<dyn std::error::Error>> {
        let workspace = tempfile::tempdir()?;
        let artifact_root = workspace.path().join("target/lib");
        fs::create_dir_all(artifact_root.join("src"))?;
        fs::write(
            artifact_root.join("Cargo.toml"),
            "[package]\nname = \"query_provider\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[dependencies]\ndatafusion = \"53\"\nsubstrait = \"0.58\"\n",
        )?;
        fs::write(artifact_root.join("src/lib.rs"), "pub fn query() {}\n")?;
        let artifact = LibraryArtifactMetadata {
            dependency_key: "query_provider".to_string(),
            manifest_name: "query_provider".to_string(),
            manifest_path: workspace.path().join("query_provider.incnlib"),
            crate_root: artifact_root.clone(),
            cargo_toml_path: artifact_root.join("Cargo.toml"),
            crate_lib_path: artifact_root.join("src/lib.rs"),
            kind: LibraryArtifactKind::Materialized,
        };
        let manifest = LibraryManifest::new("query_provider", "0.1.0");
        let mut dependencies = Vec::new();
        let mut visiting = BTreeSet::new();

        collect_caller_owned_project_rust_dependencies(&artifact, &manifest, &mut visiting, &mut dependencies)?;

        assert_eq!(dependencies.len(), 2);
        assert!(dependencies.iter().any(|dependency| {
            dependency.crate_name == "datafusion"
                && dependency.version.as_deref() == Some("53")
                && dependency.source == DependencySource::Registry
        }));
        assert!(dependencies.iter().any(|dependency| {
            dependency.crate_name == "substrait"
                && dependency.version.as_deref() == Some("0.58")
                && dependency.source == DependencySource::Registry
        }));
        Ok(())
    }

    #[test]
    fn caller_owned_provider_proc_macro_is_classified_from_its_checked_manifest()
    -> Result<(), Box<dyn std::error::Error>> {
        let workspace = tempfile::tempdir()?;
        let artifact_root = workspace.path().join("target/lib");
        fs::create_dir_all(artifact_root.join("src"))?;
        fs::write(
            artifact_root.join("Cargo.toml"),
            "[package]\nname = \"provider_macros\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[lib]\nproc-macro = true\n",
        )?;
        fs::write(artifact_root.join("src/lib.rs"), "pub fn marker() {}\n")?;
        let artifact = LibraryArtifactMetadata {
            dependency_key: "provider_macros".to_string(),
            manifest_name: "provider_macros".to_string(),
            manifest_path: workspace.path().join("provider_macros.incnlib"),
            crate_root: artifact_root.clone(),
            cargo_toml_path: artifact_root.join("Cargo.toml"),
            crate_lib_path: artifact_root.join("src/lib.rs"),
            kind: LibraryArtifactKind::Materialized,
        };

        assert!(caller_owned_library_is_proc_macro(&artifact)?);
        Ok(())
    }

    #[test]
    fn emitted_library_metadata_tracks_projected_dependency_issue911() -> Result<(), Box<dyn std::error::Error>> {
        let workspace = tempfile::tempdir()?;
        let artifact_root = workspace.path().join("published");
        let projected_root = workspace.path().join("projected");
        fs::create_dir_all(&artifact_root)?;
        fs::create_dir_all(projected_root.join("src"))?;
        fs::write(
            projected_root.join("Cargo.toml"),
            "[package]\nname = \"projected_provider\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )?;
        fs::write(projected_root.join("src/lib.rs"), "pub fn marker() {}\n")?;
        let mut manifest = LibraryManifest::new("published", "0.1.0");
        manifest
            .contract_metadata
            .provider
            .provider_dependencies
            .push(ProviderDependencyMetadata {
                kind: ProviderDependencyKind::PublicPackage,
                dependency_key: "provider_alias".to_string(),
                provider_name: "projected_provider".to_string(),
                provider_version: "0.1.0".to_string(),
                artifact_digest: "sha256:stale".to_string(),
                relative_artifact_path: "../stale".to_string(),
                requested_features: BTreeSet::new(),
                default_features: false,
                optional: false,
            });
        let dependencies = vec![DependencySpec {
            crate_name: "provider_alias".to_string(),
            version: None,
            features: Vec::new(),
            default_features: false,
            source: DependencySource::Path {
                path: projected_root.clone(),
            },
            optional: false,
            package: Some("projected_provider".to_string()),
        }];

        synchronize_projected_provider_dependencies(&mut manifest, &artifact_root, &dependencies)?;

        let descriptor = &manifest.contract_metadata.provider.provider_dependencies[0];
        assert_eq!(descriptor.artifact_digest, digest_provider_artifact(&projected_root)?);
        assert_eq!(
            fs::canonicalize(artifact_root.join(&descriptor.relative_artifact_path))?,
            fs::canonicalize(projected_root)?
        );
        Ok(())
    }

    #[test]
    fn rooted_library_removes_selected_project_self_dependency_issue909() -> Result<(), Box<dyn std::error::Error>> {
        let project_root = tempfile::tempdir()?;
        let artifact_root = project_root.path().join("target/lib");
        let external_root = project_root.path().join("external/artifact");
        fs::create_dir_all(&artifact_root)?;
        fs::create_dir_all(&external_root)?;
        let path_dependency = |crate_name: &str, path: PathBuf| DependencySpec {
            crate_name: crate_name.to_string(),
            version: None,
            features: Vec::new(),
            default_features: true,
            source: DependencySource::Path { path },
            optional: false,
            package: None,
        };
        let mut resolved = ResolvedDependencies {
            dependencies: vec![
                path_dependency("root_lib", artifact_root.clone()),
                path_dependency("external", external_root),
            ],
            dev_dependencies: vec![path_dependency("root_lib_dev_alias", artifact_root)],
        };

        remove_generated_library_self_dependencies(&mut resolved, project_root.path());

        assert_eq!(resolved.dependencies.len(), 1);
        assert_eq!(resolved.dependencies[0].crate_name, "external");
        assert!(resolved.dev_dependencies.is_empty());
        Ok(())
    }

    #[test]
    fn classify_signature_mismatch_for_rust_extern_context() {
        let stderr = "error[E0308]: mismatched types in `incan_stdlib::testing::fail`\n  --> src/main.rs:10:5";
        let kind = classify_rust_extern_build_failure(stderr, "fail", "incan_stdlib::testing");
        assert_eq!(kind, Some(RustExternBuildFailureKind::SignatureMismatch));
    }

    #[test]
    fn classify_unresolved_backing_item_for_rust_extern_context() {
        let stderr = "error[E0425]: cannot find function `fail` in module `incan_stdlib::testing`";
        let kind = classify_rust_extern_build_failure(stderr, "fail", "incan_stdlib::testing");
        assert_eq!(kind, Some(RustExternBuildFailureKind::UnresolvedBackingItem));
    }

    #[test]
    fn wraps_rust_extern_failure_back_to_incan_declaration_span() {
        let stderr = "error[E0425]: cannot find function `fail` in module `incan_stdlib::testing`";
        let contexts = vec![RustExternDeclContext {
            file_path: PathBuf::from("stdlib/testing.incn"),
            source: "rust.module(\"incan_stdlib::testing\")\n@rust.extern\ndef fail(msg: str) -> None:\n  ...\n"
                .to_string(),
            item_name: "fail".to_string(),
            rust_module_path: "incan_stdlib::testing".to_string(),
            span: Span { start: 35, end: 73 },
        }];
        let rendered = format_rust_extern_wrapped_diagnostics(stderr, &contexts);
        let Some(rendered) = rendered else {
            panic!("expected wrapped diagnostic");
        };
        assert!(rendered.contains("Rust backing item"));
        assert!(rendered.contains("incan_stdlib::testing::fail"));
    }

    #[test]
    fn inline_command_project_is_stable_for_same_source_and_working_directory() {
        let cwd = Path::new("/tmp/incan-inline-cache/project");
        let source = wrap_inline_command_source("println(\"ok\")");
        let first = inline_command_project_for_cwd(cwd, &source);
        let second = inline_command_project_for_cwd(cwd, &source);

        assert_eq!(first, second);
        assert_eq!(
            first.source_path.file_name().and_then(|name| name.to_str()),
            Some("main.incn")
        );
        let rendered = first.source_path.to_string_lossy();
        assert!(
            rendered.contains("incan_inline_command_"),
            "inline command temp source should use the stable inline-command prefix: {rendered}"
        );
        assert!(
            !rendered.contains("incan_cmd_"),
            "inline command temp source must not use timestamped incan_cmd names: {rendered}"
        );
        assert!(first.project_name.starts_with("incan_inline_command_"));
        assert!(
            first
                .output_dir
                .starts_with("target/incan/inline/incan_inline_command_")
        );
    }

    #[test]
    fn inline_command_project_is_partitioned_by_working_directory() {
        let source = wrap_inline_command_source("println(\"ok\")");
        let first = inline_command_project_for_cwd(Path::new("/tmp/incan-inline-cache/one"), &source);
        let second = inline_command_project_for_cwd(Path::new("/tmp/incan-inline-cache/two"), &source);

        assert_ne!(
            first, second,
            "different working directories should not race on one inline command temp source"
        );
    }

    #[test]
    fn inline_command_project_is_partitioned_by_source_content() {
        let cwd = Path::new("/tmp/incan-inline-cache/project");
        let first = inline_command_project_for_cwd(cwd, &wrap_inline_command_source("println(\"one\")"));
        let second = inline_command_project_for_cwd(cwd, &wrap_inline_command_source("println(\"two\")"));

        assert_ne!(
            first, second,
            "different inline snippets in the same working directory must not race on one generated cargo target"
        );
    }

    #[test]
    fn inline_command_uses_bounded_generated_project_prefixes() {
        assert_eq!(INLINE_COMMAND_PROJECT_PREFIX, "incan_inline_command");
        assert_eq!(INLINE_COMMAND_OUTPUT_PARENT, "target/incan/inline");
    }

    #[test]
    fn inline_command_source_wrapper_preserves_existing_main() {
        let source = "def main() -> None:\n    println(\"ok\")\n";

        assert_eq!(wrap_inline_command_source(source), source);
    }

    #[test]
    fn inline_command_source_wrapper_adds_stub_main_for_expression_snippets() {
        let wrapped = wrap_inline_command_source("println(\"ok\")");

        assert!(
            wrapped.contains("def main() -> Unit:\n  pass"),
            "inline snippets without a main should preserve existing run -c stub behavior: {wrapped}"
        );
    }

    #[test]
    fn run_entrypoint_omits_unused_manifest_rust_dependencies() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let project_root = tmp.path();
        let scripts_dir = project_root.join("scripts");
        let declared_unused_rust_dependencies = ["itoa", "ryu"];
        std::fs::create_dir_all(&scripts_dir)?;
        std::fs::write(
            project_root.join("incan.toml"),
            "[project]\nname = \"unused_rust_dep_run_repro\"\nversion = \"0.1.0\"\n\n[rust-dependencies]\nitoa = \"1\"\nryu = \"1\"\n",
        )?;
        std::fs::write(
            scripts_dir.join("check.incn"),
            "def main() -> None:\n    println(\"ok\")\n",
        )?;

        let cargo_lock_payload = std::fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.lock"))?;
        let fingerprint = compute_deps_fingerprint(&[], &[], &CargoFeatureSelection::default(), Some(project_root));
        let incan_lock = IncanLock::new(fingerprint, CargoFeatureSelection::default(), cargo_lock_payload);
        incan_lock.write(&project_root.join("incan.lock"))?;

        let entry_path = scripts_dir.join("check.incn");
        let output_dir = project_root.join("target").join("incan").join("check");
        let entry_arg = entry_path
            .to_str()
            .ok_or("entry path should be valid utf-8 for prepare_project test")?;
        let output_arg = output_dir
            .to_str()
            .ok_or("output path should be valid utf-8 for prepare_project test")?;

        prepare_project(
            entry_arg,
            Some(output_arg),
            &CargoPolicy::default(),
            &FeatureSelection::default(),
            None,
            Vec::new(),
            false,
            false,
            "release",
        )?;

        let generated_manifest = std::fs::read_to_string(output_dir.join("Cargo.toml"))?;
        let manifest = toml::from_str::<toml::Value>(&generated_manifest)?;
        let dependency_table = manifest
            .get("dependencies")
            .and_then(toml::Value::as_table)
            .ok_or("generated manifest should contain a dependencies table")?;
        let emitted_unused_dependencies = declared_unused_rust_dependencies
            .iter()
            .filter(|dependency| dependency_table.contains_key(**dependency))
            .copied()
            .collect::<Vec<_>>();
        assert!(
            emitted_unused_dependencies.is_empty(),
            "unused package-level rust dependencies should not be emitted for a script run; emitted {emitted_unused_dependencies:?}:\n{generated_manifest}"
        );
        Ok(())
    }

    #[cfg(feature = "rust_inspect")]
    #[test]
    fn library_rust_abi_query_paths_include_rust_extern_backing_items() -> Result<(), Box<dyn std::error::Error>> {
        let source =
            "rust.module(\"incan_stdlib::num\")\n@rust.extern\npub def gcd_i64(a: int, b: int) -> int:\n  ...\n";
        let tokens = lexer::lex(source).map_err(|errs| format!("lex errors: {errs:?}"))?;
        let ast = parser::parse(&tokens).map_err(|errs| format!("parse errors: {errs:?}"))?;
        let module = ParsedModule {
            name: "lib".to_string(),
            path_segments: vec!["lib".to_string()],
            file_path: PathBuf::from("src/lib.incn"),
            source: source.to_string(),
            ast,
        };

        let modules = vec![module];
        let contexts = collect_rust_extern_contexts(&modules);
        let paths = collect_library_rust_abi_query_paths(&modules, &contexts);

        assert!(
            paths.iter().any(|path| path == "incan_stdlib::num::gcd_i64"),
            "expected rust.extern backing item in ABI query paths, got: {paths:?}"
        );
        Ok(())
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
        let incan_core::interop::RustItemKind::Type(prewarmed_type) = &prewarmed.metadata.kind else {
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
        let incan_core::interop::RustItemKind::Type(child_id_type) = &child_id.kind else {
            return Err("expected ChildId ABI type metadata".into());
        };
        assert!(child_id_type.metadata_completeness.has_methods());
        assert!(child_id_type.methods.iter().any(|method| method.name == "as_str"));
        Ok(())
    }

    /// Proves complete published ABI extraction is independent of unrelated downstream impl crates in the graph.
    #[cfg(feature = "rust_inspect")]
    #[test]
    fn library_rust_abi_ignores_loaded_downstream_trait_impls_issue924() -> Result<(), Box<dyn std::error::Error>> {
        // ---- Fixture: clean and downstream-loaded views of one Rust surface ----
        let workspace = tempfile::tempdir()?;
        let trait_api = workspace.path().join("trait-api");
        let surface_api = workspace.path().join("surface-api");
        let downstream_api = workspace.path().join("downstream-api");
        let clean_probe = workspace.path().join("clean-probe");
        let polluted_probe = workspace.path().join("polluted-probe");
        for root in [&trait_api, &surface_api, &downstream_api, &clean_probe, &polluted_probe] {
            fs::create_dir_all(root.join("src"))?;
        }
        fs::write(
            trait_api.join("Cargo.toml"),
            "[package]\nname = \"abi_trait_api\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )?;
        fs::write(trait_api.join("src/lib.rs"), "pub trait Intrinsic {}\n")?;
        fs::write(
            surface_api.join("Cargo.toml"),
            "[package]\nname = \"abi_surface_api\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nabi_trait_api = { path = \"../trait-api\" }\n",
        )?;
        fs::write(
            surface_api.join("src/lib.rs"),
            "pub struct Thing;\n\nimpl abi_trait_api::Intrinsic for Thing {}\n",
        )?;
        fs::write(
            downstream_api.join("Cargo.toml"),
            "[package]\nname = \"abi_downstream_api\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nabi_surface_api = { path = \"../surface-api\" }\n",
        )?;
        fs::write(
            downstream_api.join("src/lib.rs"),
            "pub trait Ambient {}\n\nimpl Ambient for abi_surface_api::Thing {}\n",
        )?;
        fs::write(
            clean_probe.join("Cargo.toml"),
            "[package]\nname = \"abi_clean_probe\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nabi_surface_api = { path = \"../surface-api\" }\n",
        )?;
        fs::write(clean_probe.join("src/lib.rs"), "pub fn load_surface() {}\n")?;
        fs::write(
            polluted_probe.join("Cargo.toml"),
            "[package]\nname = \"abi_polluted_probe\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nabi_surface_api = { path = \"../surface-api\" }\nabi_downstream_api = { path = \"../downstream-api\" }\n",
        )?;
        fs::write(polluted_probe.join("src/lib.rs"), "pub fn load_graph() {}\n")?;

        // ---- Publication: complete ABI must converge across both loaded graphs ----
        let query_paths = vec!["abi_surface_api::Thing".to_string()];
        let clean = collect_library_rust_abi(&clean_probe, &query_paths)?.ok_or("expected clean library Rust ABI")?;
        let polluted =
            collect_library_rust_abi(&polluted_probe, &query_paths)?.ok_or("expected polluted library Rust ABI")?;

        assert_eq!(
            clean, polluted,
            "published library ABI must not depend on unrelated downstream impl crates"
        );

        // ---- Contract: preserve intrinsic traits and exclude downstream-only traits ----
        let thing = polluted
            .get("abi_surface_api::Thing")
            .ok_or("expected Thing ABI item")?;
        let incan_core::interop::RustItemKind::Type(thing_type) = &thing.kind else {
            return Err("expected Thing ABI type metadata".into());
        };
        assert!(
            thing_type
                .implemented_traits
                .iter()
                .any(|implemented| implemented.path == "abi_trait_api::Intrinsic")
        );
        assert!(
            thing_type
                .implemented_traits
                .iter()
                .all(|implemented| implemented.path != "abi_downstream_api::Ambient")
        );
        Ok(())
    }

    #[test]
    fn library_entrypoint_precondition_fails_when_missing() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let manifest_path = tmp.path().join("incan.toml");
        let manifest_content = "[project]\nname = \"mylib\"\n";
        fs::write(&manifest_path, manifest_content)?;
        let manifest = ProjectManifest::from_str(manifest_content, &manifest_path)?;

        let err = validate_library_entrypoint(&manifest);
        assert!(err.is_err(), "expected missing src/lib.incn to fail");
        Ok(())
    }

    #[test]
    fn library_entrypoint_precondition_passes_when_present() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let src_dir = tmp.path().join("src");
        fs::create_dir_all(&src_dir)?;
        fs::write(src_dir.join("lib.incn"), "\"\"\"lib\"\"\"\n")?;
        let manifest_path = tmp.path().join("incan.toml");
        let manifest_content = "[project]\nname = \"mylib\"\n";
        fs::write(&manifest_path, manifest_content)?;
        let manifest = ProjectManifest::from_str(manifest_content, &manifest_path)?;

        let lib_path = validate_library_entrypoint(&manifest)?;
        assert!(lib_path.ends_with("src/lib.incn"));
        Ok(())
    }

    #[test]
    fn oven_bake_discovers_an_initialized_executable_project() -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        fs::create_dir_all(project.path().join("src"))?;
        fs::write(project.path().join("incan.toml"), "[project]\nname = \"app\"\n")?;
        fs::write(project.path().join("src/main.incn"), "def main() -> None:\n    pass\n")?;

        let targets = discover_oven_bake_project_targets(project.path())?;

        assert_eq!(
            targets,
            vec![(OvenBakeProjectTarget::Executable, project.path().join("src/main.incn"))]
        );
        Ok(())
    }

    #[test]
    fn oven_interop_bootstrap_selects_only_one_declared_executable() -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let main = project.path().join("src/main.incn");
        let extra = project.path().join("src/extra.incn");

        let selected = sole_oven_interop_executable_target(vec![
            (OvenBakeProjectTarget::Library, project.path().join("src/lib.incn")),
            (OvenBakeProjectTarget::Executable, main.clone()),
        ])?;
        assert_eq!(selected, (OvenBakeProjectTarget::Executable, main));

        let error = match sole_oven_interop_executable_target(vec![
            (OvenBakeProjectTarget::Executable, project.path().join("src/main.incn")),
            (OvenBakeProjectTarget::Executable, extra),
        ]) {
            Err(error) => error,
            Ok(selected) => {
                return Err(format!(
                    "automatic interop bootstrap must not select one of several scripts, but selected {selected:?}"
                )
                .into());
            }
        };
        assert!(error.to_string().contains("provide an explicit base receipt"));
        Ok(())
    }

    #[test]
    fn oven_interop_bootstrap_receipt_isolated_from_explicit_bake_receipts() -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let entrypoint = project.path().join("src/main.incn");
        let explicit =
            project_bake_receipt_path(project.path(), OvenBakeProjectTarget::Executable, &entrypoint, "debug")?;
        let bootstrap = interop_bootstrap_receipt_path(
            project.path(),
            "aarch64-apple-darwin",
            OvenBakeProjectTarget::Executable,
            &entrypoint,
            "debug",
        )?;

        assert_ne!(bootstrap, explicit);
        assert!(
            bootstrap
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("interop-bootstrap-") && name.ends_with("-debug-receipt.json")),
            "unexpected bootstrap receipt path: {}",
            bootstrap.display()
        );
        Ok(())
    }

    #[test]
    fn oven_bake_discovers_library_and_executable_targets_in_stable_order() -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        fs::create_dir_all(project.path().join("src"))?;
        fs::write(project.path().join("incan.toml"), "[project]\nname = \"mixed\"\n")?;
        fs::write(
            project.path().join("src/lib.incn"),
            "pub def value() -> int:\n    return 1\n",
        )?;
        fs::write(project.path().join("src/main.incn"), "def main() -> None:\n    pass\n")?;

        let targets = discover_oven_bake_project_targets(project.path())?;

        assert_eq!(
            targets,
            vec![
                (OvenBakeProjectTarget::Library, project.path().join("src/lib.incn")),
                (OvenBakeProjectTarget::Executable, project.path().join("src/main.incn")),
            ]
        );
        assert_eq!(
            oven_bake_dependency_surface_entrypoint(&targets),
            Some(project.path().join("src/main.incn").as_path()),
            "a mixed project must collect dependencies reachable only from its executable root",
        );
        Ok(())
    }

    #[test]
    fn oven_bake_discovers_every_distinct_declared_script_with_non_colliding_lineage()
    -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        fs::create_dir_all(project.path().join("src"))?;
        fs::write(
            project.path().join("incan.toml"),
            "[project]\nname = \"scripts\"\n\n[project.scripts]\nmain = \"src/main.incn\"\nextra = \"src/extra.incn\"\nextra_alias = \"src/extra.incn\"\n",
        )?;
        let main = project.path().join("src/main.incn");
        let extra = project.path().join("src/extra.incn");
        fs::write(&main, "def main() -> None:\n    pass\n")?;
        fs::write(&extra, "def main() -> None:\n    pass\n")?;

        let targets = discover_oven_bake_project_targets(project.path())?;

        assert_eq!(
            targets,
            vec![
                (OvenBakeProjectTarget::Executable, extra.clone()),
                (OvenBakeProjectTarget::Executable, main.clone()),
            ],
            "script aliases must not compile the same authored entrypoint twice"
        );
        assert_eq!(
            oven_bake_project_target_identity(project.path(), OvenBakeProjectTarget::Executable, &main)?,
            "executable"
        );
        assert_eq!(
            oven_bake_project_target_identity(project.path(), OvenBakeProjectTarget::Executable, &extra)?,
            "executable:src/extra.incn"
        );
        let main_receipt =
            project_bake_receipt_path(project.path(), OvenBakeProjectTarget::Executable, &main, "debug")?;
        let extra_receipt =
            project_bake_receipt_path(project.path(), OvenBakeProjectTarget::Executable, &extra, "debug")?;
        assert!(main_receipt.ends_with("executable-debug-receipt.json"));
        assert_ne!(main_receipt, extra_receipt);
        assert_ne!(
            oven_executable_entrypoint_evidence_key(project.path(), &main)?,
            oven_executable_entrypoint_evidence_key(project.path(), &extra)?,
        );
        assert_eq!(oven_bake_executable_output_dir(project.path(), &main)?, None);
        let extra_output = oven_bake_executable_output_dir(project.path(), &extra)?
            .ok_or("custom script did not receive an isolated output root")?;
        assert!(extra_output.starts_with(project.path().join("target/incan/oven-targets")));
        Ok(())
    }

    #[test]
    fn oven_bake_refuses_a_manifest_without_a_conventional_target() -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        fs::write(project.path().join("incan.toml"), "[project]\nname = \"empty\"\n")?;

        let result = discover_oven_bake_project_targets(project.path());
        let Err(error) = result else {
            return Err("a manifest without src/lib.incn or src/main.incn must not be bakeable".into());
        };
        assert!(error.to_string().contains("src/lib.incn"));
        assert!(error.to_string().contains("src/main.incn"));
        Ok(())
    }

    #[test]
    fn resolve_library_reexports_success_with_alias() -> Result<(), Box<dyn std::error::Error>> {
        let source = "pub from widgets import Widget as PublicWidget\n";
        let tokens = lexer::lex(source).map_err(|errs| format!("lex errors: {errs:?}"))?;
        let ast = parser::parse_with_module_path(&tokens, Some("project/src/lib.incn"))
            .map_err(|errs| format!("parse errors: {errs:?}"))?;
        let lib_module = ParsedModule {
            name: "main".to_string(),
            path_segments: vec!["main".to_string()],
            file_path: PathBuf::from("project/src/lib.incn"),
            source: source.to_string(),
            ast,
        };

        let widget_export = CheckedNamedExport {
            name: "Widget".to_string(),
            identity: CheckedExportIdentity::direct(vec!["widgets".to_string(), "Widget".to_string()]),
            kind: CheckedExportKind::TypeAlias(crate::frontend::library_exports::CheckedTypeAliasExport {
                name: "Widget".to_string(),
                type_params: Vec::new(),
                target: ResolvedType::Named("Widget".to_string()),
            }),
        };
        let mut module_exports: HashMap<String, HashMap<String, Vec<CheckedNamedExport>>> = HashMap::new();
        module_exports.insert(
            "widgets".to_string(),
            HashMap::from([(widget_export.name.clone(), vec![widget_export])]),
        );
        let public_widget_projection = CheckedNamedExport {
            name: "PublicWidget".to_string(),
            identity: CheckedExportIdentity::reexport(
                vec!["widgets".to_string(), "Widget".to_string()],
                vec!["widgets".to_string(), "Widget".to_string()],
            ),
            kind: CheckedExportKind::Alias(crate::frontend::library_exports::CheckedAliasExport {
                name: "PublicWidget".to_string(),
                target_path: vec!["widgets".to_string(), "Widget".to_string()],
                projected_function: None,
            }),
        };
        module_exports.insert(
            "main".to_string(),
            HashMap::from([("PublicWidget".to_string(), vec![public_widget_projection])]),
        );

        let resolved = LibraryReexportResolver::new(&module_exports)
            .resolve(&lib_module)
            .map_err(|errs| format!("{errs:?}"))?;
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].name, "PublicWidget");
        match &resolved[0].kind {
            CheckedExportKind::TypeAlias(alias) => assert_eq!(alias.name, "PublicWidget"),
            _ => panic!("expected type alias export"),
        }
        assert!(
            matches!(
                resolved[0].identity.projection,
                crate::frontend::library_exports::CheckedExportProjection::Reexport { .. }
            ),
            "the package-root export must retain the checked entrypoint re-export projection"
        );
        Ok(())
    }

    /// A renamed re-export of a callable alias must republish the callable under the new public name.
    ///
    /// The manifest's callable projection describes the binding a consumer resolves at this public name, so leaving
    /// the inner hop's name on it makes the manifest advertise `run` for an export named `public_target`.
    #[test]
    fn resolve_library_reexports_renames_the_callable_an_alias_projects() -> Result<(), Box<dyn std::error::Error>> {
        let source = "pub from provider import run as public_target\n";
        let tokens = lexer::lex(source).map_err(|errs| format!("lex errors: {errs:?}"))?;
        let ast = parser::parse_with_module_path(&tokens, Some("project/src/lib.incn"))
            .map_err(|errs| format!("parse errors: {errs:?}"))?;
        let lib_module = ParsedModule {
            name: "main".to_string(),
            path_segments: vec!["main".to_string()],
            file_path: PathBuf::from("project/src/lib.incn"),
            source: source.to_string(),
            ast,
        };

        let callable = crate::frontend::library_exports::CheckedFunctionExport {
            name: "run".to_string(),
            emitted_name: None,
            type_params: Vec::new(),
            params: Vec::new(),
            param_defaults: Vec::new(),
            return_type: ResolvedType::Int,
            is_async: false,
        };
        let run_export = CheckedNamedExport {
            name: "run".to_string(),
            identity: CheckedExportIdentity::alias(
                vec!["provider".to_string(), "run".to_string()],
                vec!["provider".to_string(), "helper".to_string()],
            ),
            kind: CheckedExportKind::Alias(crate::frontend::library_exports::CheckedAliasExport {
                name: "run".to_string(),
                target_path: vec!["provider".to_string(), "helper".to_string()],
                projected_function: Some(callable),
            }),
        };
        let mut module_exports: HashMap<String, HashMap<String, Vec<CheckedNamedExport>>> = HashMap::new();
        module_exports.insert(
            "provider".to_string(),
            HashMap::from([("run".to_string(), vec![run_export])]),
        );

        let resolved = LibraryReexportResolver::new(&module_exports)
            .resolve(&lib_module)
            .map_err(|errs| format!("{errs:?}"))?;
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].name, "public_target");
        match &resolved[0].kind {
            CheckedExportKind::Alias(alias) => {
                assert_eq!(alias.name, "public_target");
                let projected = alias
                    .projected_function
                    .as_ref()
                    .ok_or("the renamed re-export must keep the callable the alias projects")?;
                assert_eq!(
                    projected.name, "public_target",
                    "the projected callable must carry the name the re-export published it under"
                );
            }
            other => panic!("expected an alias export, got {other:?}"),
        }
        Ok(())
    }

    #[test]
    fn resolve_library_reexports_accepts_checked_rust_imports() -> Result<(), Box<dyn std::error::Error>> {
        let source = "pub from rust::receiver_factory import PairFactory as PublicPairFactory\n";
        let tokens = lexer::lex(source).map_err(|errs| format!("lex errors: {errs:?}"))?;
        let ast = parser::parse_with_module_path(&tokens, Some("project/src/lib.incn"))
            .map_err(|errs| format!("parse errors: {errs:?}"))?;
        let lib_module = ParsedModule {
            name: "main".to_string(),
            path_segments: vec!["main".to_string()],
            file_path: PathBuf::from("project/src/lib.incn"),
            source: source.to_string(),
            ast,
        };

        let pair_factory_export = CheckedNamedExport {
            name: "PublicPairFactory".to_string(),
            identity: CheckedExportIdentity::reexport(
                vec![
                    "rust".to_string(),
                    "receiver_factory".to_string(),
                    "PairFactory".to_string(),
                ],
                vec![
                    "rust".to_string(),
                    "receiver_factory".to_string(),
                    "PairFactory".to_string(),
                ],
            ),
            kind: CheckedExportKind::Alias(crate::frontend::library_exports::CheckedAliasExport {
                name: "PublicPairFactory".to_string(),
                target_path: vec![
                    "rust".to_string(),
                    "receiver_factory".to_string(),
                    "PairFactory".to_string(),
                ],
                projected_function: None,
            }),
        };
        let mut module_exports: HashMap<String, HashMap<String, Vec<CheckedNamedExport>>> = HashMap::new();
        module_exports.insert(
            "main".to_string(),
            HashMap::from([(pair_factory_export.name.clone(), vec![pair_factory_export])]),
        );

        let resolved = LibraryReexportResolver::new(&module_exports)
            .resolve(&lib_module)
            .map_err(|errs| format!("{errs:?}"))?;
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].name, "PublicPairFactory");
        let crate::frontend::library_exports::CheckedExportProjection::Reexport { target_path } =
            &resolved[0].identity.projection
        else {
            return Err("expected Rust import reexport identity".into());
        };
        assert_eq!(
            target_path,
            &[
                "rust".to_string(),
                "receiver_factory".to_string(),
                "PairFactory".to_string(),
            ]
        );
        Ok(())
    }

    #[test]
    fn resolve_library_reexports_reports_missing_module() -> Result<(), Box<dyn std::error::Error>> {
        let source = "pub from widgets import Widget\n";
        let tokens = lexer::lex(source).map_err(|errs| format!("lex errors: {errs:?}"))?;
        let ast = parser::parse_with_module_path(&tokens, Some("project/src/lib.incn"))
            .map_err(|errs| format!("parse errors: {errs:?}"))?;
        let lib_module = ParsedModule {
            name: "main".to_string(),
            path_segments: vec!["main".to_string()],
            file_path: PathBuf::from("project/src/lib.incn"),
            source: source.to_string(),
            ast,
        };

        let module_exports: HashMap<String, HashMap<String, Vec<CheckedNamedExport>>> = HashMap::new();
        let result = LibraryReexportResolver::new(&module_exports).resolve(&lib_module);
        assert!(result.is_err(), "expected missing module to fail");
        Ok(())
    }

    #[test]
    fn resolve_library_reexports_reports_duplicates() -> Result<(), Box<dyn std::error::Error>> {
        let source = "pub from widgets import Widget\npub from widgets import Widget\n";
        let tokens = lexer::lex(source).map_err(|errs| format!("lex errors: {errs:?}"))?;
        let ast = parser::parse_with_module_path(&tokens, Some("project/src/lib.incn"))
            .map_err(|errs| format!("parse errors: {errs:?}"))?;
        let lib_module = ParsedModule {
            name: "main".to_string(),
            path_segments: vec!["main".to_string()],
            file_path: PathBuf::from("project/src/lib.incn"),
            source: source.to_string(),
            ast,
        };

        let widget_export = CheckedNamedExport {
            name: "Widget".to_string(),
            identity: CheckedExportIdentity::direct(vec!["widgets".to_string(), "Widget".to_string()]),
            kind: CheckedExportKind::TypeAlias(crate::frontend::library_exports::CheckedTypeAliasExport {
                name: "Widget".to_string(),
                type_params: Vec::new(),
                target: ResolvedType::Named("Widget".to_string()),
            }),
        };
        let mut module_exports: HashMap<String, HashMap<String, Vec<CheckedNamedExport>>> = HashMap::new();
        module_exports.insert(
            "widgets".to_string(),
            HashMap::from([(widget_export.name.clone(), vec![widget_export])]),
        );

        let result = LibraryReexportResolver::new(&module_exports).resolve(&lib_module);
        assert!(result.is_err(), "expected duplicate export to fail");
        Ok(())
    }

    #[test]
    fn resolve_library_reexports_accepts_directory_entrypoint_spelling() -> Result<(), Box<dyn std::error::Error>> {
        let source = "pub from dataset.mod import DataSet\npub from dataset.ops import filter_ds\n";
        let tokens = lexer::lex(source).map_err(|errs| format!("lex errors: {errs:?}"))?;
        let ast = parser::parse_with_module_path(&tokens, Some("project/src/lib.incn"))
            .map_err(|errs| format!("parse errors: {errs:?}"))?;
        let lib_module = ParsedModule {
            name: "main".to_string(),
            path_segments: vec!["main".to_string()],
            file_path: PathBuf::from("project/src/lib.incn"),
            source: source.to_string(),
            ast,
        };

        let dataset_export = CheckedNamedExport {
            name: "DataSet".to_string(),
            identity: CheckedExportIdentity::direct(vec!["dataset".to_string(), "DataSet".to_string()]),
            kind: CheckedExportKind::TypeAlias(crate::frontend::library_exports::CheckedTypeAliasExport {
                name: "DataSet".to_string(),
                type_params: Vec::new(),
                target: ResolvedType::Named("DataSet".to_string()),
            }),
        };
        let filter_export = CheckedNamedExport {
            name: "filter_ds".to_string(),
            identity: CheckedExportIdentity::direct(vec!["dataset_ops".to_string(), "filter_ds".to_string()]),
            kind: CheckedExportKind::Function(crate::frontend::library_exports::CheckedFunctionExport {
                name: "filter_ds".to_string(),
                emitted_name: None,
                type_params: Vec::new(),
                params: Vec::new(),
                param_defaults: Vec::new(),
                return_type: ResolvedType::Named("DataSet".to_string()),
                is_async: false,
            }),
        };
        let mut module_exports: HashMap<String, HashMap<String, Vec<CheckedNamedExport>>> = HashMap::new();
        module_exports.insert(
            "dataset".to_string(),
            HashMap::from([(dataset_export.name.clone(), vec![dataset_export])]),
        );
        module_exports.insert(
            "dataset_ops".to_string(),
            HashMap::from([(filter_export.name.clone(), vec![filter_export])]),
        );

        let resolved = LibraryReexportResolver::new(&module_exports)
            .resolve(&lib_module)
            .map_err(|errs| format!("{errs:?}"))?;
        assert_eq!(resolved.len(), 2);
        assert!(resolved.iter().any(|export| export.name == "DataSet"));
        assert!(resolved.iter().any(|export| export.name == "filter_ds"));

        Ok(())
    }

    #[test]
    fn resolve_library_reexports_accepts_canonical_nested_module_spelling() -> Result<(), Box<dyn std::error::Error>> {
        let source = "pub from dataset import DataSet\npub from dataset.ops import filter_ds\n";
        let tokens = lexer::lex(source).map_err(|errs| format!("lex errors: {errs:?}"))?;
        let ast = parser::parse_with_module_path(&tokens, Some("project/src/lib.incn"))
            .map_err(|errs| format!("parse errors: {errs:?}"))?;
        let lib_module = ParsedModule {
            name: "main".to_string(),
            path_segments: vec!["main".to_string()],
            file_path: PathBuf::from("project/src/lib.incn"),
            source: source.to_string(),
            ast,
        };

        let dataset_export = CheckedNamedExport {
            name: "DataSet".to_string(),
            identity: CheckedExportIdentity::direct(vec!["dataset".to_string(), "DataSet".to_string()]),
            kind: CheckedExportKind::TypeAlias(crate::frontend::library_exports::CheckedTypeAliasExport {
                name: "DataSet".to_string(),
                type_params: Vec::new(),
                target: ResolvedType::Named("DataSet".to_string()),
            }),
        };
        let filter_export = CheckedNamedExport {
            name: "filter_ds".to_string(),
            identity: CheckedExportIdentity::direct(vec!["dataset_ops".to_string(), "filter_ds".to_string()]),
            kind: CheckedExportKind::Function(crate::frontend::library_exports::CheckedFunctionExport {
                name: "filter_ds".to_string(),
                emitted_name: None,
                type_params: Vec::new(),
                params: Vec::new(),
                param_defaults: Vec::new(),
                return_type: ResolvedType::Named("DataSet".to_string()),
                is_async: false,
            }),
        };
        let mut module_exports: HashMap<String, HashMap<String, Vec<CheckedNamedExport>>> = HashMap::new();
        module_exports.insert(
            "dataset".to_string(),
            HashMap::from([(dataset_export.name.clone(), vec![dataset_export])]),
        );
        module_exports.insert(
            "dataset_ops".to_string(),
            HashMap::from([(filter_export.name.clone(), vec![filter_export])]),
        );

        let resolved = LibraryReexportResolver::new(&module_exports)
            .resolve(&lib_module)
            .map_err(|errs| format!("{errs:?}"))?;
        assert_eq!(resolved.len(), 2);
        assert!(resolved.iter().any(|export| export.name == "DataSet"));
        assert!(resolved.iter().any(|export| export.name == "filter_ds"));

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
            project_root.join("incan.toml"),
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

        let cargo_lock_payload = std::fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.lock"))?;
        let fingerprint = compute_deps_fingerprint(&[], &[], &CargoFeatureSelection::default(), Some(project_root));
        let incan_lock = IncanLock::new(fingerprint, CargoFeatureSelection::default(), cargo_lock_payload);
        incan_lock.write(&project_root.join("incan.lock"))?;

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

    /// Exercise the artifact-only `incan build --lib` publisher without mutating its process-wide internal mode flag.
    #[test]
    fn build_library_omits_private_generic_implementation_requirements_issue1280()
    -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let project_root = tmp.path();
        let src_dir = project_root.join("src");
        std::fs::create_dir_all(&src_dir)?;

        std::fs::write(
            project_root.join("incan.toml"),
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

        let cargo_lock_payload = std::fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.lock"))?;
        let fingerprint = compute_deps_fingerprint(&[], &[], &CargoFeatureSelection::default(), Some(project_root));
        let incan_lock = IncanLock::new(fingerprint, CargoFeatureSelection::default(), cargo_lock_payload);
        incan_lock.write(&project_root.join("incan.lock"))?;

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
            project_root.join("incan.toml"),
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

        let cargo_lock_payload = std::fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.lock"))?;
        let fingerprint = compute_deps_fingerprint(&[], &[], &CargoFeatureSelection::default(), Some(project_root));
        let incan_lock = IncanLock::new(fingerprint, CargoFeatureSelection::default(), cargo_lock_payload);
        incan_lock.write(&project_root.join("incan.lock"))?;

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
            crate::frontend::registry_metadata::CHECKED_REGISTRY_METADATA_SCHEMA_VERSION
        );
        assert_eq!(
            registry.package,
            Some(crate::frontend::registry_metadata::CheckedRegistryPackageIdentity {
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
    fn feature_conditions_are_preserved_for_provider_exports_docs_registries_and_reexports()
    -> Result<(), Box<dyn std::error::Error>> {
        let source = r#"when feature("catalog"):
    @describe(summary="Catalog entry")
    pub def catalog_entry() -> str:
        """Return the selected catalog entry."""
        return "catalog"

when feature("widgets"):
    pub from widgets import Widget
"#;
        let tokens = lexer::lex(source).map_err(|errors| format!("lex errors: {errors:?}"))?;
        let ast = parser::parse_with_module_path(&tokens, Some("project/src/lib.incn"))
            .map_err(|errors| format!("parse errors: {errors:?}"))?;
        let module = ParsedModule {
            name: "main".to_string(),
            path_segments: vec!["main".to_string()],
            file_path: PathBuf::from("project/src/lib.incn"),
            source: source.to_string(),
            ast,
        };

        let requirements = provider_fact_requirements(&module, &[BTreeSet::new()]);
        let catalog_features = BTreeSet::from(["catalog".to_string()]);
        for kind in [
            ProviderFactKind::Export,
            ProviderFactKind::Documentation,
            ProviderFactKind::RegistryEntry,
        ] {
            assert!(requirements.iter().any(|requirement| {
                requirement.kind == kind
                    && requirement.identity == "main::catalog_entry"
                    && requirement.required_features == catalog_features
            }));
        }
        assert!(requirements.iter().any(|requirement| {
            requirement.kind == ProviderFactKind::ProviderDependency
                && requirement.identity == "main::from:widgets"
                && requirement.required_features == BTreeSet::from(["widgets".to_string()])
        }));
        assert!(requirements.iter().any(|requirement| {
            requirement.kind == ProviderFactKind::Export
                && requirement.identity == "main::Widget"
                && requirement.required_features == BTreeSet::from(["widgets".to_string()])
        }));
        Ok(())
    }

    #[test]
    fn provider_module_conditions_preserve_nested_and_alternative_feature_paths()
    -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let source_root = project.path().join("src");
        fs::create_dir_all(&source_root)?;
        let entry_path = source_root.join("lib.incn");
        let nested_path = source_root.join("nested.incn");
        let leaf_path = source_root.join("leaf.incn");
        let entry_source = r#"when feature("outer"):
    from nested import Nested

when feature("alternate"):
    from leaf import Leaf
"#;
        let nested_source = r#"when feature("inner"):
    from leaf import Leaf

pub model Nested:
    pub value: int
"#;
        let leaf_source = "pub model Leaf:\n    pub value: int\n";
        fs::write(&entry_path, entry_source)?;
        fs::write(&nested_path, nested_source)?;
        fs::write(&leaf_path, leaf_source)?;

        let parse_module = |name: &str,
                            path_segments: Vec<String>,
                            file_path: PathBuf,
                            source: &str|
         -> Result<ParsedModule, Box<dyn std::error::Error>> {
            let tokens = lexer::lex(source).map_err(|errors| format!("lex errors: {errors:?}"))?;
            let ast = parser::parse_with_module_path(&tokens, file_path.to_str())
                .map_err(|errors| format!("parse errors: {errors:?}"))?;
            Ok(ParsedModule {
                name: name.to_string(),
                path_segments,
                file_path,
                source: source.to_string(),
                ast,
            })
        };
        let entry = parse_module("main", vec!["main".to_string()], entry_path, entry_source)?;
        let nested = parse_module("nested", vec!["nested".to_string()], nested_path, nested_source)?;
        let leaf = parse_module("leaf", vec!["leaf".to_string()], leaf_path, leaf_source)?;
        let modules = vec![entry.clone(), nested, leaf.clone()];

        let requirements = provider_module_reachability_requirements(&modules, &entry, &source_root)?;
        let nested_key = vec!["nested".to_string()];
        let leaf_key = vec!["leaf".to_string()];
        assert_eq!(
            requirements.get(&nested_key),
            Some(&vec![BTreeSet::from(["outer".to_string()])])
        );
        assert_eq!(
            requirements.get(&leaf_key),
            Some(&vec![
                BTreeSet::from(["alternate".to_string()]),
                BTreeSet::from(["inner".to_string(), "outer".to_string()]),
            ])
        );

        let leaf_facts = provider_fact_requirements(
            &leaf,
            requirements
                .get(&leaf_key)
                .ok_or("leaf reachability should be present")?,
        );
        assert!(leaf_facts.iter().any(|fact| {
            fact.kind == ProviderFactKind::Export
                && fact.identity == "leaf::Leaf"
                && fact.required_features == BTreeSet::from(["alternate".to_string()])
        }));
        assert!(leaf_facts.iter().any(|fact| {
            fact.kind == ProviderFactKind::Export
                && fact.identity == "leaf::Leaf"
                && fact.required_features == BTreeSet::from(["inner".to_string(), "outer".to_string()])
        }));
        Ok(())
    }

    #[test]
    fn unprojected_provider_collection_rejects_unknown_features_in_inactive_modules()
    -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let source_root = project.path().join("src");
        fs::create_dir_all(&source_root)?;
        fs::write(
            project.path().join("incan.toml"),
            "[project]\nname = \"provider_features\"\n\n[project.features]\ndefault = []\nouter = []\n",
        )?;
        let entry_path = source_root.join("lib.incn");
        fs::write(&entry_path, "when feature(\"outer\"):\n    from nested import Nested\n")?;
        fs::write(
            source_root.join("nested.incn"),
            "when feature(\"missing\"):\n    pub model Nested:\n        pub value: int\n",
        )?;

        let session = super::super::common::CompilationSession::discover_with_feature_selection(
            &entry_path,
            &FeatureSelection::default(),
        )?;
        let error = collect_unprojected_provider_modules(&entry_path, &session)
            .err()
            .ok_or("unknown feature in inactive provider module should fail collection")?;

        assert!(error.message.contains("Unknown package feature `missing`"));
        Ok(())
    }

    #[test]
    fn caller_owned_re_materialization_requires_only_private_provider_edges_in_the_foundation_plan() {
        let mut manifest = LibraryManifest::new("set_library", "0.1.0");
        manifest
            .contract_metadata
            .provider
            .provider_dependencies
            .push(ProviderDependencyMetadata {
                kind: ProviderDependencyKind::PrivateImplementation,
                dependency_key: "incan_stdlib_core".to_string(),
                provider_name: "incan_stdlib_core".to_string(),
                provider_version: "0.5.0".to_string(),
                artifact_digest: "sha256:core".to_string(),
                relative_artifact_path: "providers/stdlib-core".to_string(),
                requested_features: BTreeSet::new(),
                default_features: false,
                optional: false,
            });
        let plan = OvenRustcArtifactPlan {
            dependency_search_paths: Vec::new(),
            native_search_paths: Vec::new(),
            externs: vec![("incan_stdlib_core".to_string(), PathBuf::from("core.rlib"))],
            compile_environment: BTreeMap::new(),
            caller_owned_library_digests: BTreeMap::new(),
        };
        assert!(first_unselected_private_provider_edge(&manifest, &plan).is_none());

        manifest.contract_metadata.provider.provider_dependencies[0].dependency_key = "missing_core".to_string();
        assert_eq!(
            first_unselected_private_provider_edge(&manifest, &plan)
                .map(|dependency| dependency.dependency_key.as_str()),
            Some("missing_core")
        );

        manifest.contract_metadata.provider.provider_dependencies[0].kind = ProviderDependencyKind::PublicPackage;
        manifest.contract_metadata.provider.provider_dependencies[0].dependency_key = "incan_stdlib_core".to_string();
        assert_eq!(
            first_unselected_private_provider_edge(&manifest, &plan)
                .map(|dependency| dependency.dependency_key.as_str()),
            None
        );
    }

    #[test]
    fn caller_owned_provider_graph_prefers_checked_public_edges_over_duplicate_cargo_projection_paths() {
        let mut manifest = LibraryManifest::new("parent", "0.1.0");
        manifest
            .contract_metadata
            .provider
            .provider_dependencies
            .push(ProviderDependencyMetadata {
                kind: ProviderDependencyKind::PublicPackage,
                dependency_key: "compiled-leaf".to_string(),
                provider_name: "compiled_leaf".to_string(),
                provider_version: "0.1.0".to_string(),
                artifact_digest: "sha256:leaf".to_string(),
                relative_artifact_path: "providers/compiled-leaf".to_string(),
                requested_features: BTreeSet::new(),
                default_features: false,
                optional: false,
            });
        let dependencies = vec![
            DependencySpec {
                crate_name: "compiled_leaf".to_string(),
                version: None,
                features: Vec::new(),
                default_features: true,
                source: DependencySource::Path {
                    path: PathBuf::from("generated/compiled-leaf"),
                },
                optional: false,
                package: None,
            },
            DependencySpec {
                crate_name: "rust_shadow".to_string(),
                version: None,
                features: Vec::new(),
                default_features: true,
                source: DependencySource::Path {
                    path: PathBuf::from("generated/rust-shadow"),
                },
                optional: false,
                package: None,
            },
        ];

        let remaining = caller_owned_library_dependencies_without_public_provider_edges(dependencies, &manifest);
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].crate_name, "rust_shadow");
    }

    #[test]
    fn caller_owned_library_deduplication_keeps_an_identical_direct_extern() {
        let output = PathBuf::from("direct-rustc/rust-shadow.rlib");
        let mut libraries = vec![
            OvenCallerOwnedRustcLibrary {
                crate_name: "rust_shadow".to_string(),
                output: output.clone(),
                digest: "sha256:rust-shadow".to_string(),
                expose_extern: false,
            },
            OvenCallerOwnedRustcLibrary {
                crate_name: "rust_shadow".to_string(),
                output,
                digest: "sha256:rust-shadow".to_string(),
                expose_extern: true,
            },
        ];

        deduplicate_caller_owned_libraries_prefer_extern(&mut libraries);
        assert_eq!(libraries.len(), 1);
        assert!(libraries[0].expose_extern);
    }

    fn package_loaf_manifest(
        intent: crate::oven::OvenBuildIntent,
        provider_crate: &str,
        provider_digest: &str,
    ) -> OvenRustcArtifactManifest {
        OvenRustcArtifactManifest {
            schema_version: crate::oven::rustc::OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION,
            intent,
            dependency_search_paths: vec!["artifacts/deps".to_string()],
            native_search_paths: Vec::new(),
            externs: vec![
                OvenRustcArtifactExtern {
                    crate_name: "incan_stdlib".to_string(),
                    relative_path: "artifacts/deps/libincan_stdlib-shared.rlib".to_string(),
                    digest: "sha256:shared".to_string(),
                },
                OvenRustcArtifactExtern {
                    crate_name: provider_crate.to_string(),
                    relative_path: format!("artifacts/deps/lib{provider_crate}-{provider_digest}.rlib"),
                    digest: provider_digest.to_string(),
                },
            ],
            entrypoint_externs: BTreeMap::from([(
                "generated-root".to_string(),
                vec!["incan_stdlib".to_string(), provider_crate.to_string()],
            )]),
            registry_leaves: Vec::new(),
            registry_sources: Vec::new(),
            compile_environment: BTreeMap::new(),
            vocab_auxiliary_targets: Vec::new(),
            supporting_artifacts: Vec::new(),
        }
    }

    fn packaged_provider_authority_fixture(
        profiles: &[&str],
    ) -> Result<(tempfile::TempDir, LibraryArtifactMetadata), Box<dyn std::error::Error>> {
        let package = tempfile::tempdir()?;
        let artifact_root = package.path().join("target/lib");
        let authored_source = package.path().join("src/lib.incn");
        let generated_source = artifact_root.join("src/lib.rs");
        fs::create_dir_all(authored_source.parent().ok_or("provider source has no parent")?)?;
        fs::create_dir_all(
            generated_source
                .parent()
                .ok_or("generated provider source has no parent")?,
        )?;
        fs::write(
            package.path().join(MANIFEST_FILENAME),
            "[project]\nname = \"provider\"\nversion = \"0.1.0\"\n",
        )?;
        fs::write(&authored_source, "pub def provider() -> int:\n    return 1\n")?;
        fs::write(&generated_source, "pub fn provider() {}\n")?;
        let library_manifest = LibraryManifest::new("provider", "0.1.0");
        let library_manifest_path = artifact_root.join("provider.incnlib");
        library_manifest.write_to_path(&library_manifest_path)?;
        let artifact = LibraryArtifactMetadata::from_crate_root("provider", "provider", &artifact_root);
        let mut package_profiles = BTreeMap::new();
        for profile in profiles {
            let output = artifact_root.join(format!("oven/{profile}/libprovider.rlib"));
            fs::create_dir_all(output.parent().ok_or("provider output has no parent")?)?;
            fs::write(&output, format!("sealed {profile} provider output"))?;
            let receipt = crate::oven::receipt_generated_project(
                &crate::oven::OvenGeneratedProjectRequest::new(
                    &artifact_root,
                    "provider",
                    "0.1.0",
                    "aarch64-apple-darwin",
                    "rustc fixture",
                    *profile,
                    Vec::new(),
                )
                .with_generated_source("generated-root", &generated_source),
            )?;
            package_profiles.insert(
                (*profile).to_string(),
                OvenPackagedLibraryLoafProfile {
                    receipt,
                    entries: Vec::new(),
                    library_relative_path: format!("oven/{profile}/libprovider.rlib"),
                    library_digest: digest_bytes(&fs::read(output)?),
                },
            );
        }
        write_packaged_library_loaf_manifest(
            &artifact_root,
            &OvenPackagedLibraryLoafManifest {
                schema_version: OVEN_PACKAGED_LIBRARY_LOAF_SCHEMA_VERSION,
                source_authority_digest: digest_baked_project_source_authority(package.path())?,
                compiler_version: INCAN_VERSION.to_string(),
                metadata_files: packaged_library_metadata_files(
                    &library_manifest_path,
                    &library_manifest,
                    &artifact_root,
                )?,
                profiles: package_profiles,
            },
        )?;
        Ok((package, artifact))
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
            consumer.join(MANIFEST_FILENAME),
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

    #[test]
    fn warm_project_reuse_rejects_missing_headers_before_deep_scan() -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let entrypoint = project.path().join("src/main.incn");
        let generated_source = project.path().join("target/incan/fixture/src/main.rs");
        fs::create_dir_all(entrypoint.parent().ok_or("entrypoint has no parent")?)?;
        fs::create_dir_all(generated_source.parent().ok_or("generated source has no parent")?)?;
        fs::write(
            project.path().join(MANIFEST_FILENAME),
            "[project]\nname = \"fixture\"\n",
        )?;
        fs::write(&entrypoint, "def main() -> None:\n    pass\n")?;
        fs::write(&generated_source, "fn main() {}\n")?;
        let rustc = resolve_active_rustc()?;
        let target = rustc_host_target(&rustc)?;
        let toolchain = rustc_identity(&rustc)?;
        let mut receipts = Vec::new();
        for profile in ["debug", "release"] {
            let receipt = receipt_generated_project(
                &OvenGeneratedProjectRequest::new(
                    project.path(),
                    "fixture",
                    "0.1.0",
                    target.clone(),
                    toolchain.clone(),
                    profile,
                    Vec::new(),
                )
                .with_generated_source("generated-root", &generated_source),
            )?;
            write_receipt(
                &receipt,
                project_bake_receipt_path(project.path(), OvenBakeProjectTarget::Executable, &entrypoint, profile)?,
            )?;
            receipts.push(receipt);
        }
        let store_root = tempfile::tempdir()?;
        let store = OvenStore::new(
            store_root.path(),
            crate::oven::store::OvenStoreLimits::new(1024 * 1024, 1024 * 1024, 1024 * 1024),
        );
        let targets = vec![(OvenBakeProjectTarget::Executable, entrypoint)];
        let mut context = OvenProjectBakeAuthorityContext::default();
        assert!(
            try_reuse_baked_project(
                project.path(),
                &targets,
                &store,
                &FeatureSelection::default(),
                &mut context,
            )?
            .is_none()
        );
        assert_eq!(context.source_digester.project_scan_count(project.path()), 0);
        assert!(context.initial_project_source_authority.is_none());

        for receipt in receipts {
            store.publish(&OvenArtifactPublishRequest {
                receipt,
                domain: format!("incan-release-{INCAN_VERSION}"),
                kind: OvenArtifactKind::ProjectOutput,
                payload: b"malformed candidate after cheap header gate".to_vec(),
                materialized_files: Vec::new(),
            })?;
        }
        let mut context = OvenProjectBakeAuthorityContext::default();
        assert!(
            try_reuse_baked_project(
                project.path(),
                &targets,
                &store,
                &FeatureSelection::default(),
                &mut context,
            )?
            .is_none()
        );
        assert_eq!(context.source_digester.project_scan_count(project.path()), 0);
        assert!(
            context.initial_project_source_authority.is_none(),
            "a failed cache probe must not preserve pre-refresh authority for final publication"
        );
        Ok(())
    }

    #[test]
    fn packaged_provider_source_projection_keeps_private_transitive_path_without_exposing_extern()
    -> Result<(), Box<dyn std::error::Error>> {
        let intent = crate::oven::OvenBuildIntent {
            target: "aarch64-apple-darwin".to_string(),
            toolchain: "rustc fixture".to_string(),
            profile: "debug".to_string(),
            features: Vec::new(),
        };
        let mut artifacts = package_loaf_manifest(intent, "receiver_factory", "sha256:receiver-factory");
        artifacts.dependency_search_paths = vec![
            "runtime/deps".to_string(),
            "target/aarch64-apple-darwin/debug/deps".to_string(),
        ];
        artifacts.externs[0].relative_path = "runtime/deps/libincan_stdlib.rlib".to_string();
        artifacts.externs[1].relative_path =
            "target/aarch64-apple-darwin/debug/deps/libreceiver_factory.rlib".to_string();
        artifacts
            .entrypoint_externs
            .insert("generated-root".to_string(), vec!["incan_stdlib".to_string()]);
        let package_root = PathBuf::from("sealed-provider.loaf");
        let private_dependency_path = package_root.join("target/aarch64-apple-darwin/debug/deps");
        let plan = OvenRustcArtifactPlan {
            dependency_search_paths: vec![package_root.join("runtime/deps"), private_dependency_path.clone()],
            native_search_paths: Vec::new(),
            externs: vec![
                (
                    "incan_stdlib".to_string(),
                    package_root.join("runtime/deps/libincan_stdlib.rlib"),
                ),
                (
                    "receiver_factory".to_string(),
                    private_dependency_path.join("libreceiver_factory.rlib"),
                ),
            ],
            compile_environment: BTreeMap::new(),
            caller_owned_library_digests: BTreeMap::new(),
        };

        let mut projected = trusted_artifact_plan_for_source_evidence(&plan, &artifacts, "generated-root")?;
        assert!(!projected.dependency_search_paths.contains(&private_dependency_path));
        retain_packaged_provider_fragment_dependency_search_paths(
            &mut projected,
            [(package_root.as_path(), &artifacts.dependency_search_paths[1..])],
        );

        assert!(projected.dependency_search_paths.contains(&private_dependency_path));
        assert_eq!(
            projected
                .externs
                .iter()
                .map(|(crate_name, _)| crate_name.as_str())
                .collect::<Vec<_>>(),
            vec!["incan_stdlib"]
        );
        Ok(())
    }

    #[test]
    fn package_loaf_composition_unifies_compatible_provider_closures() -> Result<(), Box<dyn std::error::Error>> {
        let intent = crate::oven::OvenBuildIntent {
            target: "aarch64-apple-darwin".to_string(),
            toolchain: "rustc fixture".to_string(),
            profile: "debug".to_string(),
            features: Vec::new(),
        };
        let incql = package_loaf_manifest(intent.clone(), "incql", "sha256:incql");
        let analytics = package_loaf_manifest(intent.clone(), "analytics", "sha256:analytics");

        let composed =
            merge_packaged_provider_artifact_manifests(&[("incql", &incql), ("analytics", &analytics)], &intent)?;

        assert_eq!(
            composed
                .externs
                .iter()
                .map(|artifact| artifact.crate_name.as_str())
                .collect::<Vec<_>>(),
            vec!["analytics", "incan_stdlib", "incql"]
        );
        assert_eq!(
            composed.entrypoint_externs.get("generated-root"),
            Some(&vec![
                "analytics".to_string(),
                "incan_stdlib".to_string(),
                "incql".to_string()
            ])
        );
        Ok(())
    }

    #[test]
    fn direct_plan_package_loaf_composes_from_provider_into_consumer() -> Result<(), Box<dyn std::error::Error>> {
        let intent = crate::oven::OvenBuildIntent {
            target: "aarch64-apple-darwin".to_string(),
            toolchain: "rustc fixture".to_string(),
            profile: "debug".to_string(),
            features: Vec::new(),
        };
        let provider = tempfile::tempdir()?;
        let provider_source = provider.path().join("src/lib.rs");
        fs::create_dir_all(provider_source.parent().ok_or("provider source has no parent")?)?;
        fs::write(&provider_source, "pub fn provider() {}\n")?;
        let provider_receipt = receipt_generated_project(
            &OvenGeneratedProjectRequest::new(
                provider.path(),
                "provider",
                "0.1.0",
                intent.target.clone(),
                intent.toolchain.clone(),
                intent.profile.clone(),
                Vec::new(),
            )
            .with_generated_source("generated-root", &provider_source),
        )?;
        let consumer = tempfile::tempdir()?;
        let consumer_source = consumer.path().join("src/main.rs");
        fs::create_dir_all(consumer_source.parent().ok_or("consumer source has no parent")?)?;
        fs::write(&consumer_source, "fn main() {}\n")?;
        let consumer_receipt = receipt_generated_project(
            &OvenGeneratedProjectRequest::new(
                consumer.path(),
                "consumer",
                "0.1.0",
                intent.target.clone(),
                intent.toolchain.clone(),
                intent.profile.clone(),
                Vec::new(),
            )
            .with_generated_source("generated-root", &consumer_source),
        )?;
        let mut artifacts = package_loaf_manifest(intent, "provider", "sha256:provider");
        let materialized_files = artifacts
            .externs
            .iter_mut()
            .enumerate()
            .map(|(index, artifact)| {
                let source = provider.path().join(format!("sealed-{index}.rlib"));
                fs::write(&source, format!("sealed {} artifact", artifact.crate_name))?;
                artifact.digest = digest_bytes(&fs::read(&source)?);
                Ok(OvenArtifactMaterializedFile {
                    source_path: source,
                    relative_path: artifact.relative_path.clone(),
                })
            })
            .collect::<Result<Vec<_>, std::io::Error>>()?;
        let limits = crate::oven::store::OvenStoreLimits::new(1024 * 1024, 1024 * 1024, 1024 * 1024);
        let artifact_root = provider.path().join("target/incan/provider");
        let package_store = OvenStore::new(packaged_library_loaf_store_root(&artifact_root), limits);
        let stored = package_store.publish(&OvenArtifactPublishRequest {
            receipt: provider_receipt.clone(),
            domain: "provider-package".to_string(),
            kind: OvenArtifactKind::DirectRustcPlan,
            payload: serde_json::to_vec(&artifacts)?,
            materialized_files,
        })?;
        let checked = CheckedPackagedProviderProfile {
            dependency_key: "provider".to_string(),
            artifact_root,
            profile: "debug".to_string(),
            package: OvenPackagedLibraryLoafProfile {
                receipt: provider_receipt.clone(),
                entries: vec![OvenPackagedLibraryLoafEntry {
                    receipt: provider_receipt,
                    identity: stored.identity.clone(),
                    kind: OvenArtifactKind::DirectRustcPlan,
                    base_loaf_identity: None,
                }],
                library_relative_path: "oven/debug/libprovider.rlib".to_string(),
                library_digest: "sha256:provider-library".to_string(),
            },
        };
        let consumer_store_root = tempfile::tempdir()?;
        let consumer_store = OvenStore::new(consumer_store_root.path(), limits);
        import_checked_packaged_library_loaf(&consumer_store, &checked)?;
        let selected = select_packaged_provider_plan(&consumer_store, &[checked], "debug", &consumer_receipt)?
            .ok_or("consumer should select the imported direct-plan package Loaf")?;
        let OvenDirectRustcPlanSelection::PackagedProvider(packages) = selected else {
            return Err("consumer did not retain the packaged provider closure".into());
        };
        assert!(matches!(&*packages, OvenPackagedProviderExecutionPlan::Direct(_)));
        assert_eq!(
            packages
                .artifact_plan()
                .externs
                .iter()
                .map(|(crate_name, _)| crate_name.as_str())
                .collect::<BTreeSet<_>>(),
            BTreeSet::from(["incan_stdlib", "provider"])
        );
        assert!(packages.report_identity().contains(&stored.identity));
        Ok(())
    }

    #[test]
    fn package_loaf_collection_exposes_missing_roots_from_one_release_cohort() -> Result<(), Box<dyn std::error::Error>>
    {
        let intent = crate::oven::OvenBuildIntent {
            target: "aarch64-apple-darwin".to_string(),
            toolchain: "rustc fixture".to_string(),
            profile: "debug".to_string(),
            features: Vec::new(),
        };
        let mut base = package_loaf_manifest(intent.clone(), "incan_stdlib_system", "sha256:base-system");
        base.vocab_auxiliary_targets = vec![
            crate::oven::rustc::OvenRustcAuxiliaryTarget {
                target: intent.target.clone(),
                dependency_search_paths: vec!["compiler-support/host/deps".to_string()],
                externs: vec![OvenRustcArtifactExtern {
                    crate_name: "incan_vocab".to_string(),
                    relative_path: "compiler-support/host/deps/libincan_vocab-host.rlib".to_string(),
                    digest: "sha256:incan-vocab-host".to_string(),
                }],
            },
            crate::oven::rustc::OvenRustcAuxiliaryTarget {
                target: "wasm32-wasip1".to_string(),
                dependency_search_paths: vec!["compiler-support/wasm/deps".to_string()],
                externs: vec![OvenRustcArtifactExtern {
                    crate_name: "incan_vocab".to_string(),
                    relative_path: "compiler-support/wasm/deps/libincan_vocab-wasm.rlib".to_string(),
                    digest: "sha256:incan-vocab-wasm".to_string(),
                }],
            },
        ];
        base.externs.push(OvenRustcArtifactExtern {
            crate_name: "windows_sys".to_string(),
            relative_path: "artifacts/deps/libwindows_sys-base.rlib".to_string(),
            digest: "sha256:base-windows-sys".to_string(),
        });
        base.externs.push(OvenRustcArtifactExtern {
            crate_name: "incan_partner".to_string(),
            relative_path: "artifacts/deps/libpartner_alias-base.rlib".to_string(),
            digest: "sha256:base-partner-alias".to_string(),
        });
        base.entrypoint_externs
            .get_mut("generated-root")
            .ok_or("base fixture omitted generated-root externs")?
            .push("windows_sys".to_string());
        base.entrypoint_externs
            .get_mut("generated-root")
            .ok_or("base fixture omitted generated-root externs")?
            .push("incan_partner".to_string());
        base.registry_sources = vec![OvenRustcRegistrySourcePackage {
            package: "windows-sys".to_string(),
            version: "0.61.2".to_string(),
            features: vec!["Win32".to_string()],
            source: crate::oven::rustc::OvenRustcRegistrySource {
                registry: "registry+https://example.invalid/index".to_string(),
                checksum: "windows-sys-checksum".to_string(),
                relative_root: "registry-sources/windows-sys".to_string(),
                digest: "sha256:windows-sys-source".to_string(),
            },
        }];
        base.supporting_artifacts.push(OvenRustcSupportingArtifact {
            relative_path: "registry-sources/windows-sys/Cargo.toml".to_string(),
            digest: "sha256:windows-sys-cargo-toml".to_string(),
        });
        base.supporting_artifacts.push(OvenRustcSupportingArtifact {
            relative_path: "registry-sources/base-only/Cargo.toml".to_string(),
            digest: "sha256:base-only-source".to_string(),
        });
        let mut provider = package_loaf_manifest(intent.clone(), "analytics", "sha256:analytics");
        provider.vocab_auxiliary_targets = base.vocab_auxiliary_targets.clone();
        provider.registry_sources = vec![OvenRustcRegistrySourcePackage {
            package: "windows-sys".to_string(),
            version: "0.61.2".to_string(),
            features: vec!["Win32".to_string(), "Win32_Foundation".to_string()],
            source: crate::oven::rustc::OvenRustcRegistrySource {
                registry: "registry+https://example.invalid/index".to_string(),
                checksum: "windows-sys-checksum".to_string(),
                relative_root: "registry-sources/windows-sys".to_string(),
                digest: "sha256:windows-sys-source".to_string(),
            },
        }];
        provider.supporting_artifacts.push(OvenRustcSupportingArtifact {
            relative_path: "registry-sources/windows-sys/Cargo.toml".to_string(),
            digest: "sha256:windows-sys-cargo-toml".to_string(),
        });
        let mut divergent_provider = provider.clone();
        divergent_provider.externs[0] = OvenRustcArtifactExtern {
            crate_name: "incan_stdlib".to_string(),
            relative_path: "provider/deps/libincan_stdlib-provider.rlib".to_string(),
            digest: "sha256:provider-stdlib".to_string(),
        };
        divergent_provider
            .dependency_search_paths
            .push("provider/deps".to_string());
        let divergent = merge_packaged_provider_artifact_manifests_with_release_base(
            &[("analytics", &divergent_provider)],
            &base,
            &intent,
        );
        assert!(
            divergent.is_err(),
            "a provider that did not inherit its exact release runtime must fail closed"
        );
        let provider = provider.with_release_cohort_from_base(&base, &BTreeSet::new())?;

        let composed =
            merge_packaged_provider_artifact_manifests_with_release_base(&[("analytics", &provider)], &base, &intent)?;

        assert!(composed.externs.iter().any(|artifact| {
            artifact.crate_name == "incan_stdlib"
                && artifact.relative_path == "artifacts/deps/libincan_stdlib-shared.rlib"
        }));
        assert!(
            composed
                .externs
                .iter()
                .any(|artifact| artifact.crate_name == "incan_stdlib_system")
        );
        assert!(
            composed
                .supporting_artifacts
                .iter()
                .all(|artifact| { artifact.relative_path != "artifacts/deps/libincan_stdlib-shared.rlib" })
        );
        assert!(
            composed
                .supporting_artifacts
                .iter()
                .any(|artifact| artifact.relative_path == "artifacts/deps/libwindows_sys-base.rlib")
        );
        assert!(
            composed
                .supporting_artifacts
                .iter()
                .any(|artifact| artifact.relative_path == "artifacts/deps/libpartner_alias-base.rlib"),
            "an `incan_*` alias is not release-owned unless its sealed artifact belongs to the runtime family"
        );
        assert!(
            composed
                .supporting_artifacts
                .iter()
                .all(|artifact| { artifact.relative_path != "registry-sources/base-only/Cargo.toml" })
        );
        assert_eq!(composed.vocab_auxiliary_targets, base.vocab_auxiliary_targets);
        assert_eq!(composed.registry_sources, provider.registry_sources);
        assert_eq!(
            direct_rustc_source_extern_names(&composed, "generated-root")?,
            BTreeSet::from([
                "analytics".to_string(),
                "incan_stdlib".to_string(),
                "incan_stdlib_system".to_string(),
            ])
        );
        assert!(!direct_rustc_source_extern_names(&composed, "generated-root")?.contains("incan_vocab"));
        assert!(!direct_rustc_source_extern_names(&composed, "generated-root")?.contains("windows_sys"));
        assert!(!direct_rustc_source_extern_names(&composed, "generated-root")?.contains("incan_partner"));
        Ok(())
    }

    #[test]
    fn package_loaf_composition_rejects_conflicting_public_crate_identity() -> Result<(), Box<dyn std::error::Error>> {
        let intent = crate::oven::OvenBuildIntent {
            target: "aarch64-apple-darwin".to_string(),
            toolchain: "rustc fixture".to_string(),
            profile: "debug".to_string(),
            features: Vec::new(),
        };
        let first = package_loaf_manifest(intent.clone(), "shared_provider", "sha256:first");
        let second = package_loaf_manifest(intent.clone(), "shared_provider", "sha256:second");

        let result = merge_packaged_provider_artifact_manifests(&[("first", &first), ("second", &second)], &intent);
        let Err(error) = result else {
            return Err("distinct sealed public crate artifacts must not be composed".into());
        };

        assert!(error.to_string().contains("direct Rust crate `shared_provider`"));
        Ok(())
    }

    #[test]
    fn packaged_library_loaf_profile_requires_its_sealed_output_and_matching_cohort()
    -> Result<(), Box<dyn std::error::Error>> {
        let package = tempfile::tempdir()?;
        let artifact_root = package.path().join("target/lib");
        let authored_source = package.path().join("src/lib.incn");
        let source = artifact_root.join("src/lib.rs");
        let output = artifact_root.join("oven/debug/libprovider.rlib");
        fs::create_dir_all(authored_source.parent().ok_or("provider source has no parent")?)?;
        fs::create_dir_all(source.parent().ok_or("provider source has no parent")?)?;
        fs::create_dir_all(output.parent().ok_or("provider output has no parent")?)?;
        fs::write(
            package.path().join(MANIFEST_FILENAME),
            "[project]\nname = \"provider\"\nversion = \"0.1.0\"\n",
        )?;
        fs::write(&authored_source, "pub def provider() -> int:\n    return 1\n")?;
        fs::write(&source, "pub fn provider() {}\n")?;
        fs::write(&output, b"sealed provider output")?;
        let sidecar = artifact_root.join("desugarers/provider.wasm");
        fs::create_dir_all(sidecar.parent().ok_or("provider sidecar has no parent")?)?;
        let sealed_sidecar = b"sealed provider desugarer";
        fs::write(&sidecar, sealed_sidecar)?;
        let mut library_manifest = LibraryManifest::new("provider", "0.1.0");
        library_manifest.vocab = Some(crate::library_manifest::VocabExports {
            crate_path: "vocab_companion".to_string(),
            package_name: "provider_vocab_companion".to_string(),
            keyword_registrations: Vec::new(),
            dsl_surfaces: Vec::new(),
            provider_manifest: incan_vocab::LibraryManifest::default(),
            desugarer_artifact: Some(crate::library_manifest::VocabDesugarerArtifact {
                artifact_kind: incan_vocab::DesugarerArtifactKind::WasmModule,
                abi_version: incan_vocab::WASM_DESUGAR_ABI_VERSION,
                relative_path: "desugarers/provider.wasm".to_string(),
                target: "wasm32-wasip1".to_string(),
                profile: "release".to_string(),
                entrypoint: "desugar_block".to_string(),
                sha256: hex::encode(Sha256::digest(sealed_sidecar)),
            }),
        });
        let library_manifest_path = artifact_root.join("provider.incnlib");
        library_manifest.write_to_path(&library_manifest_path)?;
        fs::write(
            artifact_root.join("Cargo.toml"),
            "[package]\nname = \"provider\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )?;
        let artifact = LibraryArtifactMetadata::from_crate_root("provider", "provider", &artifact_root);
        let receipt = crate::oven::receipt_generated_project(
            &crate::oven::OvenGeneratedProjectRequest::new(
                &artifact_root,
                "provider",
                "0.1.0",
                "aarch64-apple-darwin",
                "rustc fixture",
                "debug",
                Vec::new(),
            )
            .with_generated_source("generated-root", &source),
        )?;
        let release_receipt = crate::oven::receipt_generated_project(
            &crate::oven::OvenGeneratedProjectRequest::new(
                &artifact_root,
                "provider",
                "0.1.0",
                "aarch64-apple-darwin",
                "rustc fixture",
                "release",
                Vec::new(),
            )
            .with_generated_source("generated-root", &source),
        )?;
        let release_receipt_identity = release_receipt.identity.clone();
        let sealed_output_digest = digest_bytes(&fs::read(&output)?);
        let mut manifest = OvenPackagedLibraryLoafManifest {
            schema_version: OVEN_PACKAGED_LIBRARY_LOAF_SCHEMA_VERSION,
            source_authority_digest: digest_baked_project_source_authority(package.path())?,
            compiler_version: INCAN_VERSION.to_string(),
            metadata_files: packaged_library_metadata_files(&library_manifest_path, &library_manifest, &artifact_root)?,
            profiles: BTreeMap::from([
                (
                    "debug".to_string(),
                    OvenPackagedLibraryLoafProfile {
                        receipt,
                        entries: Vec::new(),
                        library_relative_path: "oven/debug/libprovider.rlib".to_string(),
                        library_digest: sealed_output_digest.clone(),
                    },
                ),
                (
                    "release".to_string(),
                    OvenPackagedLibraryLoafProfile {
                        receipt: release_receipt,
                        entries: Vec::new(),
                        library_relative_path: "oven/debug/libprovider.rlib".to_string(),
                        library_digest: sealed_output_digest.clone(),
                    },
                ),
            ]),
        };
        write_packaged_library_loaf_manifest(&artifact_root, &manifest)?;

        let executable_receipt = crate::oven::receipt_generated_project(
            &crate::oven::OvenGeneratedProjectRequest::new(
                &artifact_root,
                "provider_executable",
                "0.1.0",
                "aarch64-apple-darwin",
                "rustc fixture",
                "release",
                Vec::new(),
            )
            .with_generated_source("generated-root", &source),
        )?;
        write_receipt(&executable_receipt, crate::oven::default_receipt_path(package.path()))?;
        let release_artifacts =
            package_loaf_manifest(executable_receipt.intent.clone(), "provider", &sealed_output_digest);
        let selected_library_receipt = caller_owned_library_receipt(&artifact, "release", &release_artifacts, None)?;
        assert_eq!(selected_library_receipt.identity, release_receipt_identity);
        assert_ne!(selected_library_receipt.identity, executable_receipt.identity);

        assert!(packaged_library_loaf_profile(&artifact, "debug", "aarch64-apple-darwin", "rustc fixture")?.is_some());
        assert!(
            packaged_library_loaf_profile(&artifact, "debug", "x86_64-unknown-linux-gnu", "rustc fixture")?.is_none()
        );

        let sealed_library_manifest = fs::read(&library_manifest_path)?;
        let mut changed_library_manifest = sealed_library_manifest.clone();
        changed_library_manifest.push(b'\n');
        fs::write(&library_manifest_path, changed_library_manifest)?;
        let metadata_error = packaged_library_loaf_profile(&artifact, "debug", "aarch64-apple-darwin", "rustc fixture")
            .err()
            .ok_or("changed checked package metadata must fail closed")?;
        assert!(metadata_error.to_string().contains("checked library metadata"));
        fs::write(&library_manifest_path, &sealed_library_manifest)?;

        fs::write(&sidecar, b"changed provider desugarer")?;
        let sidecar_error = packaged_library_loaf_profile(&artifact, "debug", "aarch64-apple-darwin", "rustc fixture")
            .err()
            .ok_or("changed manifest-declared package sidecar must fail closed")?;
        assert!(sidecar_error.to_string().contains("declared sidecars"));
        fs::write(&sidecar, sealed_sidecar)?;

        manifest.schema_version = OVEN_PACKAGED_LIBRARY_LOAF_SCHEMA_VERSION - 1;
        write_packaged_library_loaf_manifest(&artifact_root, &manifest)?;
        let schema_error = packaged_library_loaf_profile(&artifact, "debug", "aarch64-apple-darwin", "rustc fixture")
            .err()
            .ok_or("a package manifest from the previous package release-cohort schema must fail closed")?;
        assert!(schema_error.to_string().contains("schema"));
        manifest.schema_version = OVEN_PACKAGED_LIBRARY_LOAF_SCHEMA_VERSION;

        manifest.compiler_version = "0.4.0".to_string();
        write_packaged_library_loaf_manifest(&artifact_root, &manifest)?;
        let release_error = packaged_library_loaf_profile(&artifact, "debug", "aarch64-apple-darwin", "rustc fixture")
            .err()
            .ok_or("a package Loaf from another Incan release must fail closed")?;
        assert!(release_error.to_string().contains("baked by Incan"));
        manifest.compiler_version = INCAN_VERSION.to_string();

        let outside_output = package.path().join("target/outside.rlib");
        fs::write(&outside_output, b"outside provider output")?;
        let profile = manifest
            .profiles
            .get_mut("debug")
            .ok_or("fixture package manifest has no debug profile")?;
        profile.library_relative_path = "../outside.rlib".to_string();
        profile.library_digest = digest_bytes(&fs::read(&outside_output)?);
        write_packaged_library_loaf_manifest(&artifact_root, &manifest)?;
        let path_error = packaged_library_loaf_profile(&artifact, "debug", "aarch64-apple-darwin", "rustc fixture")
            .err()
            .ok_or("an escaping package library path must fail closed")?;
        assert!(
            path_error
                .to_string()
                .contains("unsafe package-owned library output path")
        );
        let profile = manifest
            .profiles
            .get_mut("debug")
            .ok_or("fixture package manifest has no debug profile")?;
        profile.library_relative_path = "oven/debug/libprovider.rlib".to_string();
        profile.library_digest = sealed_output_digest.clone();
        write_packaged_library_loaf_manifest(&artifact_root, &manifest)?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;

            fs::remove_file(&output)?;
            symlink(&outside_output, &output)?;
            let symlink_error =
                packaged_library_loaf_profile(&artifact, "debug", "aarch64-apple-darwin", "rustc fixture")
                    .err()
                    .ok_or("a package-owned library symlink must fail closed")?;
            assert!(symlink_error.to_string().contains("must be a regular file"));
            fs::remove_file(&output)?;
            fs::write(&output, b"sealed provider output")?;
        }

        fs::write(&authored_source, "pub def provider() -> int:\n    return 2\n")?;
        let source_error = packaged_library_loaf_profile(&artifact, "debug", "aarch64-apple-darwin", "rustc fixture")
            .err()
            .ok_or("an edited provider source tree must fail closed")?;
        assert!(
            source_error
                .to_string()
                .contains("changed after the package Loaf was baked")
        );
        fs::write(&authored_source, "pub def provider() -> int:\n    return 1\n")?;

        fs::write(&output, b"mutated provider output")?;
        let error = packaged_library_loaf_profile(&artifact, "debug", "aarch64-apple-darwin", "rustc fixture")
            .err()
            .ok_or("mutated packaged provider output must fail closed")?;
        assert!(error.to_string().contains("digest"));

        fs::write(&output, b"sealed provider output")?;
        let consumer = package.path().join("consumer");
        fs::create_dir_all(consumer.join("src"))?;
        fs::write(
            consumer.join(MANIFEST_FILENAME),
            "[project]\nname = \"consumer\"\n\n[dependencies]\nprovider = { path = \"..\" }\n",
        )?;
        fs::write(consumer.join("src/main.incn"), "def main() -> None:\n    pass\n")?;
        let source_backed_consumer_authority = digest_baked_project_source_authority(&consumer)?;

        fs::remove_file(package.path().join(MANIFEST_FILENAME))?;
        fs::remove_dir_all(package.path().join("src"))?;
        assert!(
            packaged_library_loaf_profile(&artifact, "debug", "aarch64-apple-darwin", "rustc fixture")?.is_some(),
            "a source-free installed package remains governed by its sealed release, receipt, path, and output facts"
        );
        assert_eq!(
            source_backed_consumer_authority,
            digest_baked_project_source_authority(&consumer)?,
            "relocating a sealed provider without its source tree must preserve the consumer's exact lineage"
        );

        manifest.source_authority_digest = digest_bytes(b"updated sealed provider source authority");
        write_packaged_library_loaf_manifest(&artifact_root, &manifest)?;
        assert_ne!(
            source_backed_consumer_authority,
            digest_baked_project_source_authority(&consumer)?,
            "a source-free provider edge must bind the package Loaf's sealed source authority"
        );

        manifest.source_authority_digest = "sha256:not-a-digest".to_string();
        write_packaged_library_loaf_manifest(&artifact_root, &manifest)?;
        let malformed_authority =
            packaged_library_loaf_profile(&artifact, "debug", "aarch64-apple-darwin", "rustc fixture")
                .err()
                .ok_or("a malformed package source-authority digest must fail closed")?;
        assert!(
            malformed_authority.to_string().contains("canonical SHA-256 digest"),
            "unexpected malformed source-authority diagnostic: {malformed_authority}"
        );

        fs::remove_file(packaged_library_loaf_manifest_path(&artifact_root))?;
        let missing_handoff = digest_baked_project_source_authority(&consumer)
            .err()
            .ok_or("a source-free provider without a sealed package Loaf must fail closed")?;
        assert!(missing_handoff.to_string().contains("without its sealed package Loaf"));
        Ok(())
    }

    // ---- Body IR input contract (#1166) ----

    /// A raw vocab declaration whose owning library manifest this compilation never loaded.
    ///
    /// The parser only produces one of these when an import activates a library vocabulary, which the replacement
    /// module profile still refuses, so building it by hand is the only way to put the desugar pass in front of a
    /// node it must resolve.
    fn undesugared_vocab_declaration() -> Spanned<Declaration> {
        Spanned::new(
            Declaration::VocabBlock(crate::frontend::ast::VocabBlockStmt {
                keyword: "query".to_string(),
                keyword_binding: crate::frontend::ast::VocabKeywordBinding {
                    is_declaration_owned_clause: false,
                    dependency_key: "demo.query".to_string(),
                    activation_namespace: "demo".to_string(),
                    surface_kind: incan_vocab::KeywordSurfaceKind::FunctionDecl,
                    compound_tokens: Vec::new(),
                    placement: incan_vocab::KeywordPlacement::TopLevel,
                    clause_body_kind: None,
                },
                decorators: Vec::new(),
                signature_head: None,
                header_args: Vec::new(),
                body: Vec::new(),
                body_item_trailing_commas: Vec::new(),
            }),
            Span::new(0, 1),
        )
    }

    /// Build the replacement-backend options a `--backend replacement` build resolves to.
    fn replacement_build_options() -> BuildCommandOptions {
        BuildCommandOptions {
            backend: BackendSelectionOptions {
                requested: BackendKind::Replacement,
                explicit: true,
                shadow: false,
                fallback_policy: FallbackPolicy::Refuse,
            },
            ..BuildCommandOptions::default()
        }
    }

    #[test]
    fn the_contract_step_refuses_a_vocab_declaration_with_the_desugar_pass_own_diagnostic()
    -> Result<(), Box<dyn std::error::Error>> {
        // The replacement path owes Body IR a desugared program. This is what "owes" means concretely: a vocab
        // declaration whose library is unavailable stops here, with the resolution failure the desugar pass already
        // reports, rather than travelling on to become a lowering refusal at the same span.
        let program = crate::frontend::ast::Program {
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

    #[test]
    fn the_replacement_build_uses_the_session_selected_package_feature_projection()
    -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let entrypoint = project.path().join("src/main.incn");
        fs::create_dir_all(entrypoint.parent().ok_or("fixture entrypoint has no parent")?)?;
        fs::write(
            project.path().join("incan.toml"),
            "[project]\nname = \"replacement_features\"\n\n[project.features]\nbeta = []\n",
        )?;
        fs::write(
            &entrypoint,
            "when feature(\"beta\"):\n    def main() -> int:\n        return 7\n",
        )?;

        let mut options = replacement_build_options();
        options.package_features = FeatureSelection::new(["beta"]);
        let report =
            build_replacement_file_report(&entrypoint.to_string_lossy(), options, &BuildReportOptions::default())?;
        assert_eq!(report["replacement_execution"]["result"], "7");
        assert_eq!(report["semantic_module"]["module_path"], "main");
        Ok(())
    }

    #[test]
    fn the_replacement_report_retains_exact_numeric_result_type() -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let entrypoint = project.path().join("main.incn");
        fs::write(
            project.path().join("incan.toml"),
            "[project]\nname = \"typed_report\"\n",
        )?;
        fs::write(&entrypoint, "def main() -> f32:\n    return 1.23456789\n")?;

        let report = build_replacement_file_report(
            &entrypoint.to_string_lossy(),
            replacement_build_options(),
            &BuildReportOptions::default(),
        )?;
        assert_eq!(report["schema_version"], REPLACEMENT_EXECUTION_REPORT_SCHEMA_VERSION);
        assert_eq!(report["replacement_execution"]["result"], 1.234_567_9_f32.to_string());
        assert_eq!(report["replacement_execution"]["result_type"], "f32");
        assert!(
            report["replacement_execution"]["output_identity"]
                .as_str()
                .is_some_and(|identity| identity.starts_with("sha256:"))
        );
        Ok(())
    }

    #[test]
    fn the_replacement_build_pipeline_analyzes_its_session_exactly_once() -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let entrypoint = project.path().join("src/main.incn");
        fs::create_dir_all(entrypoint.parent().ok_or("fixture entrypoint has no parent")?)?;
        fs::write(
            project.path().join("incan.toml"),
            "[project]\nname = \"replacement_one_analysis\"\n",
        )?;
        fs::write(
            &entrypoint,
            "def helper() -> int:\n    return 1\n\ndef main() -> int:\n    return helper()\n",
        )?;

        let analysis_scope = super::super::common::scoped_compilation_session_analysis_invocations();
        let report = build_replacement_file_report(
            &entrypoint.to_string_lossy(),
            replacement_build_options(),
            &BuildReportOptions::default(),
        )?;

        assert_eq!(report["replacement_execution"]["result"], "1");
        assert_eq!(
            analysis_scope.invocation_count(),
            1,
            "the actual replacement build pipeline must analyze its compilation session exactly once"
        );
        Ok(())
    }

    #[test]
    fn the_replacement_build_never_executes_a_main_behind_an_inactive_feature() -> Result<(), Box<dyn std::error::Error>>
    {
        // The end-to-end consequence, through the real CLI entry point: with no active feature there is no `main`
        // to lower, so the build must refuse rather than execute a body this compilation does not contain.
        let project = tempfile::tempdir()?;
        let entrypoint = project.path().join("main.incn");
        fs::write(
            project.path().join("incan.toml"),
            "[project]\nname = \"replacement_inactive_feature\"\n\n[project.features]\nbeta = []\n",
        )?;
        fs::write(
            &entrypoint,
            "when feature(\"beta\"):\n    def main() -> int:\n        return 7\n",
        )?;

        let error = build_replacement_file_report(
            &entrypoint.to_string_lossy(),
            replacement_build_options(),
            &BuildReportOptions::default(),
        )
        .err()
        .ok_or("a `main` behind an inactive feature must not produce a successful replacement build")?;
        assert!(
            !error.to_string().contains("7"),
            "no gated body may have been executed: {error}"
        );

        // Same source, same contract, through the direct API the parity corpus and unit tests use: both entry
        // points must agree that nothing was lowered, which is the point of stating the contract at all.
        let tokens =
            lexer::lex(&fs::read_to_string(&entrypoint)?).map_err(|errors| CliError::failure(format!("{errors:?}")))?;
        let parsed = parser::parse(&tokens).map_err(|errors| CliError::failure(format!("{errors:?}")))?;
        let program = body_ir::apply_body_ir_input_contract(parsed, &entrypoint)
            .map_err(|errors| CliError::failure(format!("{errors:?}")))?;
        let module_path = vec!["main".to_string()];
        let mut checker = typechecker::TypeChecker::new();
        checker.set_current_module_path(Some(module_path.clone()));
        checker
            .check_program(&program)
            .map_err(|errors| CliError::failure(format!("{errors:?}")))?;
        let body_ir = build_body_ir_module_v0(&program, &module_path, checker.type_info());
        assert!(
            body_ir.bodies.is_empty(),
            "the direct API must lower the same nothing the CLI path did: {:?}",
            body_ir.bodies.iter().map(|body| &body.name).collect::<Vec<_>>()
        );
        Ok(())
    }
}
