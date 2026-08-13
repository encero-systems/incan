//! Build and run pipeline for Incan projects.
//!
//! This module handles the full compilation flow: module collection, type checking, codegen configuration, dependency
//! resolution, project generation, and receipt-bound direct-`rustc` Oven execution.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::Instant;

use serde::Serialize;

use crate::backend::project::generator::GENERATED_CARGO_TARGET_DIR_ENV;
use crate::backend::project::runner::resolved_cargo_executable;
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
use crate::frontend::contract_metadata::{ContractMetadataPackage, read_project_model_bundles};
use crate::frontend::library_exports::{CheckedExportKind, CheckedNamedExport, collect_checked_public_exports};
use crate::frontend::library_manifest_index::{
    LibraryArtifactKind, LibraryArtifactMetadata, LibraryManifestIndex, LibraryManifestIndexEntry,
    load_provider_dependency_artifact,
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
    ProviderImplementationFacet, ProviderModuleClaim, digest_provider_artifact, digest_provider_source_inputs,
};
use crate::lockfile::{CargoFeatureSelection, provider_semantic_identities, semantic_lock_state};
use crate::manifest::{DependencySource, DependencySpec, ProjectManifest};
use crate::oven::interop::{
    OVEN_INTEROP_EXECUTION_RECEIPT_INPUT, default_interop_execution_receipt_path, interop_execution_build_unit_inputs,
    load_interop_execution_receipt, validate_interop_execution_receipt,
};
use crate::oven::legacy_cargo::{
    OvenLegacyCargoDirectDependencyClosure, OvenLegacyCargoPrepareRequest, OvenLegacyCargoPublicationKind,
    direct_rustc_compile_environment, prepare_direct_rustc_plan,
};
use crate::oven::loaf::{
    OVEN_LOAF_ENV, OVEN_LOAF_MISS_GUIDANCE, OvenLoafSelection, OvenToolchainLoaf,
    resolve_toolchain_loaf_for_registry_dependencies, runtime_build_unit_inputs,
};
use crate::oven::rustc::{
    OvenCallerOwnedRustcLibrary, OvenRegistryLeafAuthority, OvenRustcArtifactManifest, OvenRustcArtifactPlan,
    OvenRustcError, OvenSelectedPathRustcAuthority, OvenTrustedDirectRustcTargetRequest,
    attach_caller_owned_rustc_libraries, bake_trusted_direct_rustc_library, bake_trusted_direct_rustc_proc_macro,
    bake_trusted_direct_rustc_run, clear_inherited_cargo_environment, direct_rustc_source_extern_names,
    materialize_declared_rust_libraries_with_selected_path_authority, resolve_active_rustc, rustc_host_target,
    rustc_identity, select_direct_rustc_plan_for_execution, trusted_artifact_plan_for_source_evidence,
    validate_sealed_registry_leaf,
};
use crate::oven::store::{OvenArtifactKind, OvenStore, OvenStoreLease};
use crate::oven::{
    OvenGeneratedProjectRequest, digest_bytes, digest_dependency_specs, receipt_generated_project, write_receipt,
};
use crate::oven_interop::locked_oven_interop_targets;
use crate::provider::{
    FeatureSelection, PackageFeatureGraph, PackageFeaturePlan, ProviderPlan, SDK_PROVIDER_BUILD_ENV,
};
use crate::version::INCAN_VERSION;

use super::build_report::{
    BuildOvenReport, BuildReportDraft, BuildReportMode, BuildReportOptions, BuildReportProject, RustInspectionFormat,
    SourceFileReport, artifact_report, cargo_report, dependencies_report, emit_build_report,
    emit_rust_inspection_report, generated_project_report, incan_dependencies_report, interop_report,
    oven_generated_project_report, rust_inspection_report, semantic_report,
};
use super::common::{
    CargoPolicy, CompilationSession, INTERNAL_LIBRARY_ARTIFACT_ONLY_ENV, ProjectRequirements, build_source_map,
    cargo_command_flags, collect_incan_source_files, collect_project_requirements, collect_rust_dependency_uses,
    discover_effective_project_manifest, enforce_project_toolchain_constraint, extend_requirements_with_provider_plan,
    format_dependency_error, imported_module_deps_for_with_index, merge_project_requirement_dependencies,
    module_key_index, resolve_project_root, resolve_source_root, semantic_sdk_path_dependencies, validate_output_dir,
};
#[cfg(feature = "rust_inspect")]
use super::common::{collect_rust_inspect_query_paths, collect_rust_inspect_query_paths_from_programs};
use super::lock::{LockResolution, LockResolutionRequest, resolve_lock_context, validate_oven_lock_policy};
#[cfg(feature = "rust_inspect")]
use super::lock::{OvenRustInspectSourceAuthorityRequest, RustInspectWorkspaceRequest, prepare_rust_inspect_workspace};
use super::oven::open_default_oven_store;
use super::vocab_extraction::{
    PendingDesugarerArtifact, collect_library_vocab_metadata, oven_vocab_direct_rustc_context_from_plan,
};
use crate::cli::prelude::ParsedModule;
#[cfg(feature = "rust_inspect")]
use crate::rust_inspect::{Inspector, InspectorConfig, RustMetadataError};
use sha2::{Digest as _, Sha256};

// ============================================================================
// Project Preparation (shared between build and run)
// ============================================================================

const INLINE_COMMAND_PROJECT_PREFIX: &str = "incan_inline_command";
const INLINE_COMMAND_OUTPUT_PARENT: &str = "target/incan/inline";

/// Prepared source and immutable build-unit selection for the normal Oven Alpha executable path.
///
/// This deliberately contains no Cargo target path or command. Generated Rust and the final binary are caller-owned;
/// the selected native closure retains its store lease until the direct-Rustc bake and any child execution complete.
struct OvenPreparedProject {
    generator: ProjectGenerator,
    project_root: PathBuf,
    provider_plan: Arc<ProviderPlan>,
    receipt: crate::oven::OvenReceipt,
    plan_selection: OvenDirectRustcPlanSelection,
    materialization: OvenToolchainMaterialization,
    rustc: PathBuf,
    crate_name: String,
    rust_edition: String,
    caller_owned_libraries: Vec<OvenCallerOwnedRustcLibrary>,
    report: BuildReportDraft,
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
    out_dir: PathBuf,
    manifest_path: PathBuf,
    library_manifest: LibraryManifest,
    timings_ms: BTreeMap<String, u64>,
    report: BuildReportDraft,
    oven: Option<OvenPreparedLibrary>,
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
    /// Caller-owned generated Rust keyed by `library` or `executable`; it is never copied into the bounded Oven store.
    pub generated_sources: BTreeMap<String, PathBuf>,
    /// Bounded local store that retains project-specific Loafs when this project needs one.
    pub store: PathBuf,
    /// One receipt and selection outcome for each discovered project target/profile.
    pub profiles: Vec<OvenProjectBakeProfileReport>,
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

/// Receipt-selected direct-Rustc closure for a normal Oven command.
///
/// A selection is either a receipt-bound project closure in the bounded local store or a complete compiler-shipped
/// standard-library Loaf held stable by its immutable generation lock. The latter remains direct so one versioned
/// stdlib closure cannot become many per-project cache copies.
pub(crate) enum OvenDirectRustcPlanSelection {
    Stored(Box<OvenStoredDirectRustcExecutionPlan>),
    ToolchainLoaf(Box<OvenToolchainLoaf>),
}

impl OvenDirectRustcPlanSelection {
    /// Return the receipt-bound identity included in a normal-command build report.
    fn report_identity(&self) -> String {
        match self {
            Self::Stored(selected) => selected.identity.clone(),
            Self::ToolchainLoaf(native) => {
                format!("loaf:{}", native.loaf_build_unit_identity)
            }
        }
    }

    /// Return the exact already-selected direct-Rustc closure.
    ///
    /// Callers must derive both omission and selected-path authority from this same plan. Reconstructing only its
    /// crate-name set loses the path/receipt relationship that distinguishes a compiler runtime from a lookalike
    /// caller dependency.
    fn artifact_plan(&self) -> &OvenRustcArtifactPlan {
        match self {
            Self::Stored(selected) => &selected.artifact_plan,
            Self::ToolchainLoaf(native) => &native.artifact_plan,
        }
    }

    /// Project the verified selected plan to the externs that its receipt admits to one generated source root.
    ///
    /// The complete plan also carries compiler-private support crates. They remain available to compiler-owned
    /// roots, but must not cause a normal project declaration with the same crate name to be skipped.
    fn source_artifact_plan(&self, source_evidence_key: &str) -> Result<OvenRustcArtifactPlan, OvenRustcError> {
        match self {
            Self::Stored(selected) => trusted_artifact_plan_for_source_evidence(
                &selected.artifact_plan,
                &selected.artifacts,
                source_evidence_key,
            ),
            Self::ToolchainLoaf(native) => {
                trusted_artifact_plan_for_source_evidence(&native.artifact_plan, &native.artifacts, source_evidence_key)
            }
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

fn has_rust_extern_decorator(decorators: &[Spanned<Decorator>]) -> bool {
    decorators
        .iter()
        .any(|d| d.node.path.segments.join(".") == "rust.extern")
}

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
        CheckedExportKind::Alias(alias_export) => alias_export.name = exported_name.to_string(),
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
        let mut exported_names: HashSet<String> = HashSet::new();
        let known_modules: Vec<String> = self.module_exports.keys().cloned().collect();

        if let Some(exports_by_name) = self.module_exports.get(&module_key(&lib_module.path_segments)) {
            for (export_name, export_span) in Self::direct_public_exports(lib_module) {
                if !exported_names.insert(export_name.clone()) {
                    errors.push(diagnostics::errors::duplicate_library_export(&export_name, export_span));
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
                    if !exported_names.insert(exported_name.clone()) {
                        errors.push(diagnostics::errors::duplicate_library_export(&exported_name, decl.span));
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
                if !exported_names.insert(exported_name.clone()) {
                    errors.push(diagnostics::errors::duplicate_library_export(&exported_name, decl.span));
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
                        .map(|export| rename_checked_export(export, &exported_name)),
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
            prepare_when_empty: true,
            direct_oven_inspection: false,
            force_direct_prewarm: false,
            oven_source_authority: None,
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
) -> CliResult<crate::oven::OvenReceipt> {
    let project_root = artifact.crate_root.parent().and_then(Path::parent).ok_or_else(|| {
        CliError::failure(format!(
            "Oven Alpha cannot locate the project root for pub::{} from generated artifact root {}",
            artifact.dependency_key,
            artifact.crate_root.display()
        ))
    })?;
    let receipt_path = if profile == "release" {
        crate::oven::default_receipt_path(project_root)
    } else {
        crate::oven::default_receipt_path(project_root).with_file_name("library-debug-receipt.json")
    };
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
        return Err(CliError::failure(format!(
            "Oven Alpha cannot re-materialize pub::{}: producer receipt intent does not match the selected consumer direct-Rustc plan",
            artifact.dependency_key
        )));
    }
    Ok(receipt)
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

/// Exclude provider manifest dependencies already supplied by the selected direct-Rustc plan.
///
/// A matching registry crate is compiler-owned whenever it is selected by the plan. A path dependency with the same
/// name remains caller-owned unless it resolves below an active compiler-owned root. Those roots are either the
/// scheduler's sealed runtime/provider trees or an exact crate root from the active compiler layout. That containment
/// check preserves user package authority while recognizing generated references such as `incan_core`.
fn caller_owned_library_dependencies_missing_from_selected_plan(
    dependencies: &[DependencySpec],
    artifact_plan: &OvenRustcArtifactPlan,
) -> Vec<DependencySpec> {
    caller_owned_library_dependencies_missing_from_selected_plan_with_owned_roots(
        dependencies,
        artifact_plan,
        &compiler_owned_roots(artifact_plan),
    )
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
/// component, not `crates/incan_stdlib_core`), but it remains compiler-owned precisely when the checked provider
/// plan says so *and* the selected direct-Rustc plan exposes its exact crate name. Project `pub::` artifacts never
/// meet this condition and remain caller-owned.
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
    selected_path_authority: Option<&OvenSelectedPathRustcAuthority>,
    visiting: &mut BTreeSet<PathBuf>,
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
                selected_path_authority,
                visiting,
            )?;
            nested_libraries.append(&mut materialized);
        }
        deduplicate_caller_owned_libraries_prefer_extern(&mut nested_libraries);

        let receipt = caller_owned_library_receipt(artifact, profile, artifacts)?;
        let edition = caller_owned_library_edition(artifact)?;
        let is_proc_macro = caller_owned_library_is_proc_macro(artifact)?;
        let provider_dependencies = caller_owned_library_rust_dependencies(artifact)?;
        let provider_dependencies =
            caller_owned_library_dependencies_without_public_provider_edges(provider_dependencies, manifest);
        let provider_dependencies =
            caller_owned_library_dependencies_missing_from_selected_plan(&provider_dependencies, artifact_plan);
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

/// Rebuild selected caller-owned Rust libraries in the consumer's direct-Rustc cohort.
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
    let mut libraries = Vec::new();
    let mut visiting = BTreeSet::new();
    let selected_path_authority = compiler_selected_path_authority(artifact_plan, Some(provider_plan));
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
            selected_path_authority.as_ref(),
            &mut visiting,
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

/// Return direct-Rustc extern names supplied by the receipt-selected immutable plan.
///
/// A stored plan is already lease-held by [`OvenDirectRustcPlanSelection`] before this classification. The same
/// selection stays live through caller-owned materialization and direct-Rustc baking.
fn selected_direct_rustc_source_extern_names(
    selection: &OvenDirectRustcPlanSelection,
    source_evidence_key: &str,
) -> CliResult<BTreeSet<String>> {
    match selection {
        OvenDirectRustcPlanSelection::Stored(selected) => {
            direct_rustc_source_extern_names(&selected.artifacts, source_evidence_key).map_err(oven_rustc_error)
        }
        OvenDirectRustcPlanSelection::ToolchainLoaf(native) => {
            direct_rustc_source_extern_names(&native.artifacts, source_evidence_key).map_err(oven_rustc_error)
        }
    }
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
    let selected_externs = artifact_plan
        .externs
        .iter()
        .map(|(crate_name, _)| crate_name.clone())
        .collect::<BTreeSet<_>>();
    declared_rust_libraries_missing_from_selected_plan_with_owned_roots(
        dependencies,
        &selected_externs,
        &compiler_owned_roots(artifact_plan),
    )
}

/// Verify the semantic registry contract for every dependency omitted because the selected plan exposes its crate.
///
/// Direct-Rustc extern names carry no Cargo package/version information. Without this check an impossible declared
/// version could silently borrow an unrelated compiler-owned artifact solely because both normalize to one crate
/// name. The sealed native catalog remains the only resolver; no Cargo cache, index, or network state is consulted.
fn validate_selected_plan_registry_dependencies(
    dependencies: &[DependencySpec],
    selected_externs: &BTreeSet<String>,
    registry_authority: Option<&OvenRegistryLeafAuthority>,
    profile: &str,
) -> CliResult<()> {
    for dependency in dependencies {
        if matches!(dependency.source, DependencySource::Registry)
            && selected_externs.contains(&dependency.crate_name.replace('-', "_"))
        {
            validate_sealed_registry_leaf(dependency, registry_authority, profile).map_err(oven_rustc_error)?;
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
    let dep_modules = &modules[..modules.len() - 1];
    let project_root = manifest
        .as_ref()
        .map(|manifest| manifest.project_root().to_path_buf())
        .unwrap_or(inferred_project_root);
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
    generator.set_include_dev_dependencies(false);
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
    append_oven_interop_execution_build_inputs(&mut oven_build_inputs, manifest.as_ref(), &rustc_target)?;

    #[cfg(feature = "rust_inspect")]
    let rust_inspect_manifest_dir = {
        let metadata_query_paths = loaf_rust_inspect_query_paths(&modules, &compilation_session)?;
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
    generator.set_dependencies(resolved.dependencies);
    generator.set_dev_dependencies(resolved.dev_dependencies);

    let has_deps = !emitted_dep_modules.is_empty()
        || dep_modules
            .iter()
            .any(|module| compiled_sdk_modules.contains_emission_path(&module.path_segments));
    if has_deps {
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
    } else {
        let rust_code = codegen
            .try_generate(&main_module.ast)
            .map_err(|error| CliError::failure(format!("Code generation error: {error}")))?;
        generator
            .generate(&rust_code)
            .map_err(|error| CliError::failure(format!("Error generating project: {error}")))?;
    }

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
    .with_generated_source_tree("generated-source-tree", generator.output_dir().join("src"));
    for (name, value) in &oven_build_inputs {
        receipt_request = receipt_request.with_build_unit_input(name.clone(), value.clone());
    }
    let receipt = receipt_generated_project(&receipt_request).map_err(|error| CliError::failure(error.to_string()))?;
    write_receipt(&receipt, crate::oven::default_receipt_path(&project_root))
        .map_err(|error| CliError::failure(error.to_string()))?;
    let store = open_default_oven_store()?;
    let required_registry_dependencies = format_oven_registry_dependency_requirements(&inline_path_dependencies);
    let plan_preparation = select_or_bake_generated_project_plan(
        oven_plan_mode,
        &store,
        &receipt,
        &inline_path_dependencies,
        generator.output_dir(),
        &generator.crate_root_path(),
        &rustc,
    )?;
    let plan_preparation = plan_preparation.ok_or_else(|| {
        CliError::failure(format!(
            "Oven Alpha has no compatible native provider/dependency unit for receipt {}. Required sealed registry dependencies: {}. Generated project: {}; receipt: {}. Normal build and run will not invoke Cargo; the active toolchain does not ship a compatible Oven Loaf. {}",
            receipt.identity,
            required_registry_dependencies,
            generator.output_dir().display(),
            crate::oven::default_receipt_path(&project_root).display(),
            OVEN_LOAF_MISS_GUIDANCE,
        ))
    })?;
    let plan_selection = plan_preparation.plan_selection;
    let registry_authority = registry_leaf_authority_for_plan_selection(&plan_selection)?;
    let full_artifact_plan = plan_selection.artifact_plan();
    let artifact_plan = plan_selection
        .source_artifact_plan("generated-root")
        .map_err(oven_rustc_error)?;
    let selected_externs = selected_direct_rustc_source_extern_names(&plan_selection, "generated-root")?;
    validate_selected_plan_registry_dependencies(
        &inline_path_dependencies,
        &selected_externs,
        registry_authority.as_ref(),
        profile,
    )?;
    let inline_libraries =
        declared_rust_libraries_missing_from_selected_plan(&inline_path_dependencies, &artifact_plan);
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
    };
    Ok(OvenPreparedProject {
        generator,
        project_root,
        provider_plan,
        receipt,
        plan_selection,
        materialization: plan_preparation.materialization,
        rustc,
        crate_name: ProjectGenerator::rust_target_name(&project_name),
        rust_edition,
        caller_owned_libraries,
        report,
    })
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
        let native = resolve_toolchain_loaf_for_registry_dependencies(
            receipt,
            OvenLoafSelection::CompilerOwnedProviderSuperset,
            registry_dependencies,
        )
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
            }));
        }
        return Err(CliError::failure(format!(
            "Oven Alpha has no compatible compiler-suite native provider/dependency unit. Required sealed registry dependencies: {}. Nested build and run will not materialize a caller-owned store entry or invoke Cargo",
            format_oven_registry_dependency_requirements(registry_dependencies),
        )));
    }

    if receipt_requires_final_interop_plan(receipt) {
        return Err(interop_final_plan_required_error());
    }
    if let Some(native) = resolve_toolchain_loaf_for_registry_dependencies(
        receipt,
        OvenLoafSelection::CompilerOwnedProviderSuperset,
        registry_dependencies,
    )
    .map_err(|error| CliError::failure(error.to_string()))?
    {
        return Ok(Some(OvenDirectRustcPlanPreparation {
            plan_selection: OvenDirectRustcPlanSelection::ToolchainLoaf(Box::new(native)),
            materialization: OvenToolchainMaterialization::ToolchainLoaf,
        }));
    }
    Ok(
        select_receipt_direct_rustc_execution_plan(store, receipt)?.map(|selected| OvenDirectRustcPlanPreparation {
            plan_selection: OvenDirectRustcPlanSelection::Stored(Box::new(selected)),
            materialization: OvenToolchainMaterialization::Reused,
        }),
    )
}

/// Publish a receipt-compatible generated-project closure at Oven's one
/// explicit project-bake boundary.
///
/// The compatibility baker owns Cargo only for this transaction. It creates a
/// private bounded target, seals the verified direct-rustc artifacts into the
/// shared Oven store, and removes the private target before returning. The
/// release-scoped domain deliberately groups compatible project closures by
/// Incan version rather than by checkout path or generated-source digest; the
/// receipt and direct-rustc plan remain the exact selection authority.
fn bake_generated_project_compatibility_plan(
    store: &OvenStore,
    receipt: &crate::oven::OvenReceipt,
    generated_project: &Path,
    generated_root: &Path,
    rustc: &Path,
) -> CliResult<OvenToolchainMaterialization> {
    let compile_environment = direct_rustc_compile_environment(generated_project, generated_root)
        .map_err(|error| CliError::failure(error.to_string()))?;
    let publication = prepare_direct_rustc_plan(&OvenLegacyCargoPrepareRequest {
        store,
        receipt: receipt.clone(),
        generated_project: generated_project.to_path_buf(),
        cargo: resolved_cargo_executable()
            .map_err(|error| CliError::failure(format!("cannot resolve Cargo for explicit Oven bake: {error}")))?,
        rustc: rustc.to_path_buf(),
        sdk_inventory: None,
        domain: format!("incan-release-{INCAN_VERSION}"),
        publication_kind: OvenLegacyCargoPublicationKind::Executable,
        source_evidence_key: "generated-root".to_string(),
        compile_environment,
        inspection_packages: None,
        direct_dependency_closure: OvenLegacyCargoDirectDependencyClosure::GeneratedSource,
        compact_debug_info: false,
    })
    .map_err(|error| CliError::failure(error.to_string()))?;
    Ok(if publication.cargo_version == "not-run-existing-plan" {
        OvenToolchainMaterialization::Reused
    } else {
        OvenToolchainMaterialization::CompatibilityBaked
    })
}

/// Select a plan for an explicit project bake, publishing exactly once only
/// after the ordinary local-store and installed-Loaf paths both miss.
fn select_or_bake_generated_project_plan(
    mode: OvenProjectPlanMode,
    store: &OvenStore,
    receipt: &crate::oven::OvenReceipt,
    registry_dependencies: &[DependencySpec],
    generated_project: &Path,
    generated_root: &Path,
    rustc: &Path,
) -> CliResult<Option<OvenDirectRustcPlanPreparation>> {
    let compiler_suite_native =
        std::env::var_os("INCAN_INTERNAL_OVEN_LOAF_EXECUTION").is_some_and(|value| value == "1");
    if mode == OvenProjectPlanMode::ExplicitBake && compiler_suite_native {
        // A compiler-suite child normally consumes only the immutable shared Loaf family. The one explicit bake
        // action is different: it is the named publisher boundary, so a test of that action must be able to publish
        // and then select its private receipt-exact Loaf. Do not ask the compiler-suite superset selector here: it
        // would either hide the bake behind an unrelated stdlib Loaf or reject the miss before this explicit action
        // has a chance to use its package-qualified Cargo capability. Normal build, run, and test never take this
        // branch because they pass `ConsumeOnly`.
        if let Some(selected) = select_receipt_direct_rustc_execution_plan(store, receipt)? {
            return Ok(Some(OvenDirectRustcPlanPreparation {
                plan_selection: OvenDirectRustcPlanSelection::Stored(Box::new(selected)),
                materialization: OvenToolchainMaterialization::Reused,
            }));
        }
        let materialization =
            bake_generated_project_compatibility_plan(store, receipt, generated_project, generated_root, rustc)?;
        return select_receipt_direct_rustc_execution_plan(store, receipt)?
            .map(|selected| {
                Some(OvenDirectRustcPlanPreparation {
                    plan_selection: OvenDirectRustcPlanSelection::Stored(Box::new(selected)),
                    materialization,
                })
            })
            .ok_or_else(|| {
                CliError::failure(
                    "the explicit Oven project bake completed without a receipt-compatible direct-rustc plan",
                )
            });
    }
    if let Some(selected) = select_oven_direct_rustc_plan_with_materialization(store, receipt, registry_dependencies)? {
        return Ok(Some(selected));
    }
    if mode != OvenProjectPlanMode::ExplicitBake {
        return Ok(None);
    }
    let materialization =
        bake_generated_project_compatibility_plan(store, receipt, generated_project, generated_root, rustc)?;
    select_receipt_direct_rustc_execution_plan(store, receipt)?
        .map(|selected| {
            Some(OvenDirectRustcPlanPreparation {
                plan_selection: OvenDirectRustcPlanSelection::Stored(Box::new(selected)),
                materialization,
            })
        })
        .ok_or_else(|| {
            CliError::failure("the explicit Oven project bake completed without a receipt-compatible direct-rustc plan")
        })
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
    match selection {
        OvenDirectRustcPlanSelection::Stored(selected) => Ok(selected
            .artifacts
            .registry_leaf_authority(&selected.artifact_root, &selected.artifact_plan)),
        OvenDirectRustcPlanSelection::ToolchainLoaf(native) => Ok(native
            .artifacts
            .registry_leaf_authority(&native.artifact_root, &native.artifact_plan)),
    }
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

/// Compile a receipt-authorized generated executable through the selected direct-rustc Oven plan.
fn bake_oven_project(
    prepared: &OvenPreparedProject,
    profile: &str,
) -> CliResult<crate::oven::rustc::OvenDirectRustcBake> {
    let mut caller_owned_libraries = prepared.caller_owned_libraries.clone();
    let registry_authority = registry_leaf_authority_for_plan_selection(&prepared.plan_selection)?;
    match &prepared.plan_selection {
        OvenDirectRustcPlanSelection::Stored(selected) => {
            if has_caller_owned_project_libraries(&prepared.provider_plan) {
                let re_materialized = rematerialize_caller_owned_libraries(
                    &prepared.provider_plan,
                    profile,
                    &selected.artifacts,
                    &selected.artifact_root,
                    &selected.artifact_plan,
                    &prepared.rustc,
                    prepared.generator.output_dir(),
                    registry_authority.as_ref(),
                )?;
                replace_caller_owned_package_libraries(&mut caller_owned_libraries, re_materialized)?;
            }
            let mut artifact_plan = trusted_artifact_plan_for_source_evidence(
                &selected.artifact_plan,
                &selected.artifacts,
                "generated-root",
            )
            .map_err(oven_rustc_error)?;
            attach_caller_owned_rustc_libraries(&mut artifact_plan, &caller_owned_libraries)
                .map_err(oven_rustc_error)?;
            bake_trusted_direct_rustc_run(&OvenTrustedDirectRustcTargetRequest {
                receipt: &prepared.receipt,
                artifacts: &selected.artifacts,
                artifact_root: &selected.artifact_root,
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
        OvenDirectRustcPlanSelection::ToolchainLoaf(native) => {
            if has_caller_owned_project_libraries(&prepared.provider_plan) {
                let re_materialized = rematerialize_caller_owned_libraries(
                    &prepared.provider_plan,
                    profile,
                    &native.artifacts,
                    &native.artifact_root,
                    &native.artifact_plan,
                    &prepared.rustc,
                    prepared.generator.output_dir(),
                    registry_authority.as_ref(),
                )?;
                replace_caller_owned_package_libraries(&mut caller_owned_libraries, re_materialized)?;
            }
            let mut artifact_plan =
                trusted_artifact_plan_for_source_evidence(&native.artifact_plan, &native.artifacts, "generated-root")
                    .map_err(oven_rustc_error)?;
            attach_caller_owned_rustc_libraries(&mut artifact_plan, &caller_owned_libraries)
                .map_err(oven_rustc_error)?;
            bake_trusted_direct_rustc_run(&OvenTrustedDirectRustcTargetRequest {
                receipt: &prepared.receipt,
                artifacts: &native.artifacts,
                artifact_root: &native.artifact_root,
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
    }
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
) -> CliResult<crate::oven::rustc::OvenDirectRustcBake> {
    let selected = oven.profiles.get(profile).ok_or_else(|| {
        CliError::failure(format!(
            "normal Oven library build has no prepared `{profile}` direct-rustc selection"
        ))
    })?;
    let mut caller_owned_libraries = selected.caller_owned_libraries.clone();
    let registry_authority = registry_leaf_authority_for_plan_selection(&selected.plan_selection)?;
    match &selected.plan_selection {
        OvenDirectRustcPlanSelection::Stored(stored_plan) => {
            if has_caller_owned_project_libraries(&selected.provider_plan) {
                let re_materialized = rematerialize_caller_owned_libraries(
                    &selected.provider_plan,
                    profile,
                    &stored_plan.artifacts,
                    &stored_plan.artifact_root,
                    &stored_plan.artifact_plan,
                    &oven.rustc,
                    &prepared.out_dir,
                    registry_authority.as_ref(),
                )?;
                replace_caller_owned_package_libraries(&mut caller_owned_libraries, re_materialized)?;
            }
            let mut artifact_plan = trusted_artifact_plan_for_source_evidence(
                &stored_plan.artifact_plan,
                &stored_plan.artifacts,
                "generated-root",
            )
            .map_err(oven_rustc_error)?;
            attach_caller_owned_rustc_libraries(&mut artifact_plan, &caller_owned_libraries)
                .map_err(oven_rustc_error)?;
            bake_trusted_direct_rustc_library(&OvenTrustedDirectRustcTargetRequest {
                receipt: &selected.receipt,
                artifacts: &stored_plan.artifacts,
                artifact_root: &stored_plan.artifact_root,
                artifact_plan: Some(&artifact_plan),
                rustc: &oven.rustc,
                source: &prepared.generator.crate_root_path(),
                output: &oven_library_path(prepared, oven, profile),
                crate_name: &oven.crate_name,
                edition: &oven.rust_edition,
                source_evidence_key: "generated-root",
                features: &selected.receipt.intent.features,
                prefer_dynamic: false,
            })
            .map_err(oven_rustc_error)
        }
        OvenDirectRustcPlanSelection::ToolchainLoaf(native) => {
            if has_caller_owned_project_libraries(&selected.provider_plan) {
                let re_materialized = rematerialize_caller_owned_libraries(
                    &selected.provider_plan,
                    profile,
                    &native.artifacts,
                    &native.artifact_root,
                    &native.artifact_plan,
                    &oven.rustc,
                    &prepared.out_dir,
                    registry_authority.as_ref(),
                )?;
                replace_caller_owned_package_libraries(&mut caller_owned_libraries, re_materialized)?;
            }
            let mut artifact_plan =
                trusted_artifact_plan_for_source_evidence(&native.artifact_plan, &native.artifacts, "generated-root")
                    .map_err(oven_rustc_error)?;
            attach_caller_owned_rustc_libraries(&mut artifact_plan, &caller_owned_libraries)
                .map_err(oven_rustc_error)?;
            bake_trusted_direct_rustc_library(&OvenTrustedDirectRustcTargetRequest {
                receipt: &selected.receipt,
                artifacts: &native.artifacts,
                artifact_root: &native.artifact_root,
                artifact_plan: Some(&artifact_plan),
                rustc: &oven.rustc,
                source: &prepared.generator.crate_root_path(),
                output: &oven_library_path(prepared, oven, profile),
                crate_name: &oven.crate_name,
                edition: &oven.rust_edition,
                source_evidence_key: "generated-root",
                features: &selected.receipt.intent.features,
                prefer_dynamic: false,
            })
            .map_err(oven_rustc_error)
        }
    }
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
            let output = format!("{rendered}\n{}", report.unstructured_output).trim().to_string();
            CliError::failure(if output.is_empty() {
                "Oven direct-rustc compilation failed without a diagnostic transcript".to_string()
            } else {
                format!("Oven direct-rustc compilation failed:\n{output}")
            })
        }
        error => CliError::failure(error.to_string()),
    }
}

/// Build an Incan file to a Rust project.
pub fn build_file(
    file_path: &str,
    output_dir: Option<&String>,
    options: BuildCommandOptions,
    report_options: BuildReportOptions,
) -> CliResult<ExitCode> {
    let report = build_file_report(file_path, output_dir, options, &report_options)?;
    emit_build_report(&report, &report_options)?;
    Ok(ExitCode::SUCCESS)
}

/// Build one executable project and retain its completed report for workspace-level aggregation.
pub(crate) fn build_file_report(
    file_path: &str,
    output_dir: Option<&String>,
    options: BuildCommandOptions,
    report_options: &BuildReportOptions,
) -> CliResult<crate::cli::commands::build_report::BuildReport> {
    reject_normal_cargo_controls(&options.cargo_policy, options.generated_cargo_target_dir.as_ref())?;
    let total_start = Instant::now();
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
    let bake = bake_oven_project(&prepared, "release")?;
    let oven_build_ms = elapsed_ms(oven_build_start);
    print_build_progress(report_options, "✓ Oven build successful!");
    print_build_progress(report_options, format!("Binary: {}", bake.output.display()));
    let mut report_draft = prepared.report.clone();
    report_draft.artifacts.push(artifact_report("binary", &bake.output));
    Ok(report_draft.finish(BTreeMap::from([
        ("prepare".to_string(), prepare_ms),
        ("oven_build".to_string(), oven_build_ms),
        ("total".to_string(), elapsed_ms(total_start)),
    ])))
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
                out_dir.join("oven").join("rust-inspect"),
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
        })?
        .ok_or_else(|| CliError::failure("rust-inspect workspace preparation did not return a manifest directory"))?;
        record_timing(&mut timings_ms, "library_rust_inspect_prewarm", rust_inspect_start);
        Ok(rust_inspect_manifest_dir)
    })
    .transpose()?;

    let typecheck_start = Instant::now();
    let mut all_errors = String::new();
    let mut checked_exports_by_module: HashMap<String, HashMap<String, Vec<CheckedNamedExport>>> = HashMap::new();
    let mut api_metadata_modules = Vec::new();
    let module_idx_by_key = module_key_index(&modules);
    let mut stdlib_cache = StdlibAstCache::new();
    let mut checked_type_info_by_path = BTreeMap::new();

    for (idx, module) in modules.iter().enumerate() {
        let deps_for_module = imported_module_deps_for_with_index(&modules, idx, &module_idx_by_key);
        let mut checker = typechecker::TypeChecker::new();
        checker.stdlib_cache = stdlib_cache.clone();
        checker.set_current_module_path(Some(module.path_segments.clone()));
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
                for warn in checker.warnings() {
                    eprint!(
                        "{}",
                        diagnostics::format_error(module.file_path.to_string_lossy().as_ref(), &module.source, warn)
                    );
                }
                let module_exports = collect_checked_public_exports(&module.ast, &checker);
                api_metadata_modules.push(collect_checked_api_metadata(
                    &module.ast,
                    &checker,
                    module.path_segments.clone(),
                ));
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
    library_manifest.contract_metadata.api = Some(checked_api);
    library_manifest.contract_metadata.provider = compiled_provider_metadata(
        &manifest,
        &package_feature_plan,
        &provider_plan,
        &library_manifest_index,
        &out_dir,
        &provider_metadata_modules,
        lib_module,
    )?;
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

    let mut codegen = IrCodegen::new();
    codegen.set_preserve_dependency_public_items(true);
    codegen.set_registry_package_identity(Some(project_name.clone()));
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
    generator.set_include_dev_dependencies(lock_payload_for_typecheck.is_some());
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
    };
    generator.set_dependencies(resolved.dependencies);
    generator.set_dev_dependencies(resolved.dev_dependencies);

    // Keep the historical aggregate for existing consumers, while separating the stages that were previously
    // attributed misleadingly as one `library_generate_rust` cost in Oven performance evidence.
    let codegen_start = Instant::now();
    if emitted_dep_modules.is_empty() {
        let emit_rust_start = Instant::now();
        let rust_code = codegen
            .try_generate(&lib_module.ast)
            .map_err(|e| CliError::failure(format!("Code generation error: {e}")))?;
        record_timing(&mut timings_ms, "library_codegen_emit_rust", emit_rust_start);
        let write_project_start = Instant::now();
        generator
            .generate(&rust_code)
            .map_err(|e| CliError::failure(format!("Error generating project: {e}")))?;
        record_timing(&mut timings_ms, "library_codegen_write_project", write_project_start);
    } else {
        let module_paths: Vec<Vec<String>> = emitted_dep_modules
            .iter()
            .map(|module| module.path_segments.clone())
            .collect();
        let emit_rust_start = Instant::now();
        let (main_code, rust_modules) = codegen
            .try_generate_multi_file_nested(&lib_module.ast, &module_paths)
            .map_err(|e| CliError::failure(format!("Code generation error: {e}")))?;
        record_timing(&mut timings_ms, "library_codegen_emit_rust", emit_rust_start);
        let write_project_start = Instant::now();
        generator
            .generate_nested(&main_code, &rust_modules)
            .map_err(|e| CliError::failure(format!("Error generating project: {e}")))?;
        record_timing(&mut timings_ms, "library_codegen_write_project", write_project_start);
    }
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
        let store = open_default_oven_store()?;
        let mut profiles = BTreeMap::new();
        for profile in ["debug", "release"] {
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
            let receipt =
                receipt_generated_project(&receipt_request).map_err(|error| CliError::failure(error.to_string()))?;
            let receipt_path = if profile == "release" {
                crate::oven::default_receipt_path(&project_root)
            } else {
                crate::oven::default_receipt_path(&project_root).with_file_name("library-debug-receipt.json")
            };
            write_receipt(&receipt, receipt_path.clone()).map_err(|error| CliError::failure(error.to_string()))?;
            let required_registry_dependencies = format_oven_registry_dependency_requirements(
                oven_inline_rust_dependencies.as_deref().unwrap_or_default(),
            );
            let plan_preparation = select_or_bake_generated_project_plan(
                oven_plan_mode,
                &store,
                &receipt,
                oven_inline_rust_dependencies.as_deref().unwrap_or_default(),
                generator.output_dir(),
                &generator.crate_root_path(),
                &rustc,
            )?
            .ok_or_else(|| {
                CliError::failure(format!(
                    "Oven Alpha has no compatible native provider/dependency unit for `{profile}` library receipt {}. Required sealed registry dependencies: {}. Generated project: {}; receipt: {}. Normal build --lib will not invoke Cargo; the active toolchain does not ship a compatible Oven Loaf. {}",
                    receipt.identity,
                    required_registry_dependencies,
                    generator.output_dir().display(),
                    receipt_path.display(),
                    OVEN_LOAF_MISS_GUIDANCE,
                ))
            })?;
            let plan_selection = plan_preparation.plan_selection;
            let registry_authority = registry_leaf_authority_for_plan_selection(&plan_selection)?;
            let full_artifact_plan = plan_selection.artifact_plan();
            let artifact_plan = plan_selection
                .source_artifact_plan("generated-root")
                .map_err(oven_rustc_error)?;
            let selected_externs = selected_direct_rustc_source_extern_names(&plan_selection, "generated-root")?;
            validate_selected_plan_registry_dependencies(
                oven_inline_rust_dependencies.as_deref().unwrap_or_default(),
                &selected_externs,
                registry_authority.as_ref(),
                profile,
            )?;
            let inline_libraries = declared_rust_libraries_missing_from_selected_plan(
                oven_inline_rust_dependencies.as_deref().unwrap_or_default(),
                &artifact_plan,
            );
            let selected_path_authority = compiler_selected_path_authority(full_artifact_plan, Some(&provider_plan));
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
            let release = oven
                .profiles
                .get("release")
                .ok_or_else(|| CliError::failure("normal Oven library build did not prepare its release selection"))?;
            match &release.plan_selection {
                OvenDirectRustcPlanSelection::Stored(selected) => {
                    let context = oven_vocab_direct_rustc_context_from_plan(
                        &oven.rustc,
                        &selected.artifact_plan,
                        &selected.artifacts,
                        &selected.artifact_root,
                    )?;
                    Some(context)
                }
                OvenDirectRustcPlanSelection::ToolchainLoaf(native) => {
                    let context = oven_vocab_direct_rustc_context_from_plan(
                        &oven.rustc,
                        &native.artifact_plan,
                        &native.artifacts,
                        &native.artifact_root,
                    )?;
                    Some(context)
                }
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
        let release = oven
            .profiles
            .get("release")
            .ok_or_else(|| CliError::failure("normal Oven library build did not prepare its release selection"))?;
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
        out_dir,
        manifest_path,
        library_manifest,
        timings_ms,
        report: report_draft,
        oven,
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

/// Build transport-stable provider facts from the checked physical artifact projection.
fn compiled_provider_metadata(
    manifest: &ProjectManifest,
    feature_plan: &PackageFeaturePlan,
    provider_plan: &ProviderPlan,
    library_manifest_index: &LibraryManifestIndex,
    artifact_root: &Path,
    modules: &[ParsedModule],
    active_library_entrypoint: &ParsedModule,
) -> CliResult<CompiledProviderMetadata> {
    let graph = PackageFeatureGraph::from_manifest(manifest).map_err(|error| CliError::failure(error.to_string()))?;
    let root_features = feature_plan
        .root_package()
        .map(|package| &package.features)
        .ok_or_else(|| CliError::failure("resolved package feature plan is missing its root package"))?;
    let library_entrypoint = modules
        .iter()
        .find(|module| module.file_path == active_library_entrypoint.file_path)
        .ok_or_else(|| CliError::failure("unprojected provider graph is missing its library entrypoint"))?;
    let source_root = resolve_source_root(manifest.project_root(), Some(manifest));
    let module_requirements = provider_module_reachability_requirements(modules, library_entrypoint, &source_root)?;
    let mut namespace_claims = modules
        .iter()
        .filter(|module| {
            module.file_path != active_library_entrypoint.file_path
                && !module.path_segments.is_empty()
                && !module_is_owned_by_dependency_provider(provider_plan, &module.path_segments)
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
    for module in modules
        .iter()
        .filter(|module| !module_is_owned_by_dependency_provider(provider_plan, &module.path_segments))
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

    let provider_dependencies =
        compiled_provider_dependencies(feature_plan, library_manifest_index, provider_plan, artifact_root)?;
    let implementation_facets = provider_implementation_facets(&namespace_claims);
    let semantic_source_inputs = modules
        .iter()
        .filter(|module| !module_is_owned_by_dependency_provider(provider_plan, &module.path_segments))
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
        manifest.project_root(),
        manifest.path(),
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
        ..CompiledProviderMetadata::default()
    })
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
        Declaration::Import(_) | Declaration::VocabBlock(_) | Declaration::Docstring(_) => None,
    }
}

/// Return whether one declaration contributes to the package's public checked surface.
fn provider_declaration_is_public(declaration: &Declaration) -> bool {
    let visibility = match declaration {
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

/// Validate RFC 031 library-mode preconditions.
pub fn build_library(
    file_path: Option<&str>,
    output_dir: Option<&String>,
    options: BuildCommandOptions,
    report_options: BuildReportOptions,
) -> CliResult<ExitCode> {
    let report = build_library_report(file_path, output_dir, options, &report_options)?;
    emit_build_report(&report, &report_options)?;
    Ok(ExitCode::SUCCESS)
}

/// Resolve every conventional manifest-backed target that an explicit project bake must prepare.
///
/// An initialized application has `src/main.incn`; a published library has `src/lib.incn`; a mixed project owns both
/// independent generated-Rust roots. Baking only one would leave a normal command with an actionable bake command
/// that cannot actually prepare its own target, so the explicit command admits every present conventional root.
fn discover_oven_bake_project_targets(project_root: &Path) -> CliResult<Vec<(OvenBakeProjectTarget, PathBuf)>> {
    let Some(manifest) = discover_effective_project_manifest(project_root)? else {
        return Err(CliError::failure(format!(
            "`incan oven bake --project` requires an incan.toml project at {}",
            project_root.display()
        )));
    };
    enforce_project_toolchain_constraint(&manifest)?;

    let mut targets = Vec::new();
    for target in [OvenBakeProjectTarget::Library, OvenBakeProjectTarget::Executable] {
        let entrypoint = manifest.project_root().join(target.source_relative_path());
        if entrypoint.is_file() {
            targets.push((target, entrypoint));
        }
    }
    if targets.is_empty() {
        return Err(CliError::failure(format!(
            "`incan oven bake --project` requires {} or {} below {}",
            OvenBakeProjectTarget::Library.source_relative_path(),
            OvenBakeProjectTarget::Executable.source_relative_path(),
            manifest.project_root().display()
        )));
    }
    Ok(targets)
}

/// Return the durable profile-specific receipt path retained by an explicit executable project bake.
///
/// Normal executable commands continue to refresh their current selection receipt. The explicit bake additionally
/// keeps one receipt per profile, so a debug preparation cannot be silently overwritten by the following release
/// preparation before the developer can inspect it.
fn executable_bake_receipt_path(project_root: &Path, profile: &str) -> PathBuf {
    crate::oven::default_receipt_path(project_root).with_file_name(format!("executable-{profile}-receipt.json"))
}

/// Explicitly prepare compatible Oven closures for every conventional target in one Incan project.
///
/// This is preparation rather than execution: it records fresh source/lock/SDK/provider receipt evidence, reuses a
/// matching stored or release-scoped stdlib closure when available, and otherwise crosses Oven's explicit bounded
/// publisher exactly once per genuinely missing target/profile. Normal `build`, `run`, and `test` remain Cargo-free
/// consumers of the resulting direct-rustc plans.
pub(crate) fn bake_oven_project_targets(project: &Path) -> CliResult<OvenProjectBakeReport> {
    let project = project
        .to_str()
        .ok_or_else(|| CliError::failure(format!("Oven project path is not valid UTF-8: {}", project.display())))?;
    let project_root = resolve_library_project_root(Some(project))?;
    let targets = discover_oven_bake_project_targets(&project_root)?;
    let store = open_default_oven_store()?;
    let mut generated_sources = BTreeMap::new();
    let mut profiles = Vec::new();

    for (target, entrypoint) in targets {
        match target {
            OvenBakeProjectTarget::Library => {
                let prepared = prepare_library_project(
                    Some(project),
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
                    OvenProjectPlanMode::ExplicitBake,
                )?;
                let selected = prepared.oven.as_ref().ok_or_else(|| {
                    CliError::failure("explicit Oven library preparation did not produce a direct-rustc selection")
                })?;
                generated_sources.insert(target.as_str().to_string(), prepared.generator.crate_root_path());
                for (profile, selected_profile) in &selected.profiles {
                    let receipt = if profile == "release" {
                        crate::oven::default_receipt_path(&project_root)
                    } else {
                        crate::oven::default_receipt_path(&project_root).with_file_name("library-debug-receipt.json")
                    };
                    profiles.push(OvenProjectBakeProfileReport {
                        project_target: target.as_str().to_string(),
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
            }
            OvenBakeProjectTarget::Executable => {
                let entrypoint = entrypoint.to_str().ok_or_else(|| {
                    CliError::failure(format!("Oven entrypoint is not valid UTF-8: {}", entrypoint.display()))
                })?;
                for profile in ["debug", "release"] {
                    let prepared = prepare_oven_project(
                        entrypoint,
                        None,
                        &CargoPolicy::default(),
                        &FeatureSelection::default(),
                        None,
                        Vec::new(),
                        false,
                        false,
                        profile,
                        OvenProjectPlanMode::ExplicitBake,
                    )?;
                    let receipt = executable_bake_receipt_path(&project_root, profile);
                    write_receipt(&prepared.receipt, &receipt).map_err(|error| CliError::failure(error.to_string()))?;
                    generated_sources
                        .entry(target.as_str().to_string())
                        .or_insert_with(|| prepared.generator.crate_root_path());
                    profiles.push(OvenProjectBakeProfileReport {
                        project_target: target.as_str().to_string(),
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
    let total_start = Instant::now();
    let artifact_only = env::var_os(INTERNAL_LIBRARY_ARTIFACT_ONLY_ENV).is_some();
    if !artifact_only {
        reject_normal_cargo_controls(&options.cargo_policy, options.generated_cargo_target_dir.as_ref())?;
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
        for profile in ["debug", "release"] {
            bakes.push((profile, bake_oven_library(&prepared, oven, profile)?));
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
        let mut timings_ms = prepared.timings_ms.clone();
        timings_ms.insert("oven_build".to_string(), oven_build_ms);
        timings_ms.insert("total".to_string(), elapsed_ms(total_start));
        return Ok(report_draft.finish(timings_ms));
    }

    Err(CliError::failure(
        "normal `incan build --lib` requires a prepared Oven direct-rustc selection; Cargo library execution is not an available fallback",
    ))
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
    let prepared = prepare_oven_project(
        file_path,
        None,
        &cargo_policy,
        &package_features,
        sdk_profile.as_deref(),
        cargo_features,
        cargo_no_default_features,
        cargo_all_features,
        if release { "release" } else { "debug" },
        OvenProjectPlanMode::ConsumeOnly,
    )?;
    run_oven_prepared_project(prepared, if release { "release" } else { "debug" })
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
    let bake = bake_oven_project(&prepared, profile)?;
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
    use crate::frontend::lexer;
    use crate::frontend::library_exports::CheckedExportIdentity;
    use crate::frontend::parser;
    use crate::frontend::symbols::ResolvedType;
    use crate::lockfile::{
        CargoFeatureSelection, IncanLock, LockedOvenState, SemanticLockState, compute_deps_fingerprint,
    };
    use crate::manifest::ProjectManifest;
    use crate::oven::interop::{
        OvenInteropCapabilitySelection, default_interop_execution_receipt_path, receipt_interop_execution,
        write_interop_execution_receipt,
    };
    use crate::oven_interop::locked_oven_interop_targets;
    use std::fs;

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
            &[],
            project.path(),
            &generated_root,
            &rustc,
        );
        let compiler_suite_native = env::var_os("INCAN_INTERNAL_OVEN_LOAF_EXECUTION").is_some_and(|value| value == "1");
        if compiler_suite_native {
            let Err(error) = consume_only else {
                return Err("a compiler-suite normal consumer must reject a caller-owned Loaf miss".into());
            };
            assert!(
                error
                    .to_string()
                    .contains("Nested build and run will not materialize a caller-owned store entry"),
                "compiler-suite normal consumers must remain Cargo-free even when the explicit baker is tested"
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

        let first = select_or_bake_generated_project_plan(
            OvenProjectPlanMode::ExplicitBake,
            &store,
            &receipt,
            &[],
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
            &[],
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
        let selected_externs = BTreeSet::from(["serde_json".to_string()]);

        validate_selected_plan_registry_dependencies(
            &[dependency("1.0")],
            &selected_externs,
            Some(&authority),
            "debug",
        )?;
        let error = match validate_selected_plan_registry_dependencies(
            &[dependency("999.0.0")],
            &selected_externs,
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

        let resolved = LibraryReexportResolver::new(&module_exports)
            .resolve(&lib_module)
            .map_err(|errs| format!("{errs:?}"))?;
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].name, "PublicWidget");
        match &resolved[0].kind {
            CheckedExportKind::TypeAlias(alias) => assert_eq!(alias.name, "PublicWidget"),
            _ => panic!("expected type alias export"),
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
}
