//! Lock file generation and resolution for Incan projects.
//!
//! Handles creating and validating `incan.lock` files that pin dependency versions for reproducible builds.
//! Used by both `incan lock` and the build pipeline.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
#[cfg(test)]
use std::fs::OpenOptions;
#[cfg(test)]
use std::io::{self, Write};
use std::path::{Path, PathBuf};
#[cfg(test)]
use std::process::{Command, Stdio};
#[cfg(test)]
use std::thread;
#[cfg(test)]
use std::time::{Duration, Instant, SystemTime};

#[cfg(test)]
use sha2::{Digest, Sha256};

#[cfg(test)]
use crate::backend::ProjectGenerator;
#[cfg(test)]
use crate::backend::project::runner::{cargo_command, configure_cargo_target, sanitize_cargo_environment};
use crate::cli::prelude::ParsedModule;
use crate::cli::{CliError, CliResult, ExitCode};
use crate::dependency_resolver::{InlineRustImport, ResolvedDependencies, resolve_reachable_dependencies};
use crate::frontend::ast::{Declaration, ImportKind};
use crate::frontend::library_manifest_index::LibraryManifestIndex;
use crate::frontend::{diagnostics, lexer, parser};
use crate::generated_cache::{GeneratedCacheLease, resolve_generated_cargo_target};
use crate::lockfile::{
    CargoFeatureSelection, IncanLock, PublicationLock, SemanticLockState, compute_resolved_fingerprint_with_sdk_paths,
    semantic_lock_state, workspace_semantic_lock_state,
};
use crate::manifest::{DependencySpec, ProjectManifest};
#[cfg(feature = "rust_inspect")]
use crate::oven::legacy_cargo::OVEN_LEGACY_CARGO_INSPECTION_AUTHORITY_ENV;
#[cfg(feature = "rust_inspect")]
use crate::oven::loaf::{OvenToolchainLoaf, resolve_toolchain_loaf_for_registry_sources};
#[cfg(feature = "rust_inspect")]
use crate::oven::rustc::OVEN_RUSTC_REGISTRY_LOCK_RELATIVE_PATH;
#[cfg(feature = "rust_inspect")]
use crate::oven::rustc::{resolve_active_rustc, rustc_host_target, rustc_identity};
#[cfg(feature = "rust_inspect")]
use crate::oven::{OvenGeneratedProjectRequest, receipt_generated_project};
use crate::provider::{
    FeatureSelection, PackageFeaturePlan, ProviderPlan, SDK_PROVIDER_BUILD_ENV, SdkComponentSelection,
};
use crate::workspace::WorkspaceGraph;
use incan_core::lang::stdlib;

use super::common::{
    CargoPolicy, CompilationSession, INTERNAL_CARGO_LOCK_PAYLOAD_PATH_ENV, ProjectRequirements, build_source_map,
    cargo_command_flags, collect_modules_detailed_with_session, collect_project_requirements,
    collect_rust_dependency_uses, discover_active_sdk_inventory, enforce_project_toolchain_constraint,
    extend_requirements_with_provider_plan, format_dependency_error, merge_project_requirement_dependencies,
    provider_used_module_paths, resolve_sdk_component_selection, semantic_sdk_path_dependencies,
};
#[cfg(feature = "rust_inspect")]
use super::common::{
    collect_rust_inspect_query_paths, ensure_rust_inspect_workspace_with_cargo_package_name,
    mark_oven_direct_rust_inspection, prewarm_rust_inspect_workspace,
};

#[cfg(test)]
#[allow(dead_code)]
const LOCK_DEPENDENCY_PREHEAT_STALE_LOCK_SECS: u64 = 30 * 60;
#[cfg(test)]
#[allow(dead_code)]
const LIBRARY_DEPENDENCY_PREHEAT_FINGERPRINT_FILE: &str = ".incan_library_dependency_preheat_fingerprint";
#[cfg(test)]
#[allow(dead_code)]
const LIBRARY_DEPENDENCY_PREHEAT_LOCK_FILE: &str = ".incan_library_dependency_preheat.lock";

/// Inputs needed to preheat generated-library dependencies into the real generated-library Cargo target domain.
#[cfg(test)]
#[allow(dead_code)]
pub(crate) struct GeneratedLibraryDependencyPreheatRequest<'a> {
    /// Generated project directory used by both dependency preheat and the real Cargo build.
    pub cargo_working_dir: &'a Path,
    /// Dependency-only generated lock workspace directory.
    pub lock_dir: &'a Path,
    /// Cargo package name to use for the dependency-only generated lock workspace.
    pub project_name: &'a str,
    /// Rust edition to write into the dependency-only generated lock workspace.
    pub rust_edition: Option<String>,
    /// Resolved Rust dependencies that define the generated lock workspace.
    pub resolved: &'a ResolvedDependencies,
    /// Stdlib/provider requirements that define generated helper dependencies.
    pub project_requirements: &'a ProjectRequirements,
    /// Cargo feature selection used by the generated library build.
    pub cargo_features: &'a CargoFeatureSelection,
    /// Cargo policy flags used by the generated library build.
    pub cargo_policy: &'a CargoPolicy,
    /// Cargo target directory shared with the real generated library build.
    pub target_dir: &'a Path,
    /// Embedded Cargo.lock payload from `incan.lock`.
    pub cargo_lock_payload: &'a str,
    /// Exact canonical root authorizing Cargo-owned projection for the generated dependency workspace.
    pub cargo_lock_projection_root: Option<&'a str>,
}

/// Dependency graph and Cargo policy shared by generated lock-workspace preheat consumers.
#[cfg(test)]
#[allow(dead_code)]
struct DependencyPreheatContext<'a> {
    project_name: &'a str,
    rust_edition: Option<&'a str>,
    resolved: &'a ResolvedDependencies,
    project_requirements: &'a ProjectRequirements,
    cargo_policy_flags: &'a [String],
}

/// Generate or update incan.lock for a project.
pub fn lock_project(
    entry_file: Option<&PathBuf>,
    package_features: &FeatureSelection,
    sdk_profile_override: Option<&str>,
    cargo_features: Vec<String>,
    cargo_no_default_features: bool,
    cargo_all_features: bool,
) -> CliResult<ExitCode> {
    let start_dir = entry_file
        .and_then(|p| p.parent().map(|p| p.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."));
    let manifest = ProjectManifest::discover(&start_dir)
        .map_err(|e| CliError::failure(e.to_string()))?
        .ok_or_else(|| CliError::failure("No incan.toml found (run `incan init`)"))?;
    enforce_project_toolchain_constraint(&manifest)?;

    let cargo_features = CargoFeatureSelection {
        cargo_features,
        cargo_no_default_features,
        cargo_all_features,
    }
    .normalized();
    if let Some(workspace) =
        WorkspaceGraph::discover(manifest.project_root()).map_err(|error| CliError::failure(error.to_string()))?
    {
        return lock_workspace(
            &workspace,
            entry_file.map(PathBuf::as_path),
            &cargo_features,
            package_features,
            sdk_profile_override,
        );
    }
    let context = collect_project_lock_context(
        &manifest,
        entry_file.map(PathBuf::as_path),
        &cargo_features,
        package_features,
        sdk_profile_override,
        None,
    )?
    .ok_or_else(|| CliError::failure("incan lock requires a FILE argument or at least one [project.scripts] entry"))?;

    generate_oven_lockfile(
        manifest.project_root(),
        &context.resolved,
        &context.project_requirements,
        &cargo_features,
        &context.semantic,
        None,
    )?;

    Ok(ExitCode::SUCCESS)
}

/// Generate the one canonical RFC 077 lockfile for every effective workspace member.
fn lock_workspace(
    workspace: &WorkspaceGraph,
    entry_file: Option<&Path>,
    cargo_features: &CargoFeatureSelection,
    package_features: &FeatureSelection,
    sdk_profile_override: Option<&str>,
) -> CliResult<ExitCode> {
    let lock_path = workspace.root().join("incan.lock");
    let publication_lock = crate::lockfile::acquire_publication_lock(&lock_path)
        .map_err(|error| CliError::failure(format!("failed to acquire workspace lock publication guard: {error}")))?;
    let context = collect_workspace_lock_context(
        workspace,
        entry_file,
        cargo_features,
        package_features,
        sdk_profile_override,
    )?;
    generate_oven_lockfile(
        workspace.root(),
        &context.resolved,
        &context.project_requirements,
        cargo_features,
        &context.semantic,
        Some(&publication_lock),
    )?;
    Ok(ExitCode::SUCCESS)
}

/// Resolve the canonical dependency context and lock payload for a project build.
///
/// Manifest-less standalone builds retain their caller-local dependency context and have no lock payload.
/// Manifest-backed builds keep caller-local resolved dependencies and requirements for generated Cargo manifests,
/// while project- or workspace-wide aggregation owns canonical lock generation. A fresh canonical payload authorizes
/// Cargo-owned projection onto the caller manifest only after package coordinates, checksums, and edges validate.
pub(crate) struct LockResolutionRequest<'a> {
    pub project_root: &'a Path,
    pub project_name: &'a str,
    /// Active source entry, including entries outside `[project.scripts]`, that must participate in the lock context.
    pub entry_file: Option<&'a Path>,
    pub manifest: Option<&'a ProjectManifest>,
    pub resolved: &'a ResolvedDependencies,
    pub project_requirements: &'a ProjectRequirements,
    pub cargo_features: &'a CargoFeatureSelection,
    pub cargo_policy: &'a CargoPolicy,
    pub semantic: Option<&'a SemanticLockState>,
    /// Incan package-feature selection used when rebuilding the canonical project-wide lock context.
    pub package_features: Option<&'a FeatureSelection>,
    /// Command-local SDK profile used when rebuilding the canonical project-wide lock context.
    pub sdk_profile_override: Option<&'a str>,
}

/// Cargo inputs that must be consumed together by generated projects.
pub(crate) struct LockResolution {
    pub cargo_lock_authority: CargoLockAuthority,
    pub cargo_package_name: String,
    pub resolved: ResolvedDependencies,
    pub project_requirements: ProjectRequirements,
}

/// Closed authority state for generated Cargo locks.
pub(crate) enum CargoLockAuthority {
    /// No trusted Cargo lock payload is available and no stale projection cleanup is required.
    None,
    /// A tolerated stale canonical lock must not authorize or leave behind any older generated projection.
    Stale,
    /// An internal artifact build consumes an exact payload directly without caller projection.
    Exact { payload: String },
}

/// Final generator-facing inputs derived from one closed lock authority state.
pub(crate) struct CargoLockGeneratorInputs {
    pub payload: Option<String>,
    pub projection_root: Option<String>,
    pub clear_existing: bool,
}

impl CargoLockAuthority {
    /// Split the closed authority state into generator inputs at the final rendering boundary.
    pub(crate) fn into_generator_inputs(self) -> CargoLockGeneratorInputs {
        match self {
            Self::None => CargoLockGeneratorInputs {
                payload: None,
                projection_root: None,
                clear_existing: false,
            },
            Self::Stale => CargoLockGeneratorInputs {
                payload: None,
                projection_root: None,
                clear_existing: true,
            },
            Self::Exact { payload } => CargoLockGeneratorInputs {
                payload: Some(payload),
                projection_root: None,
                clear_existing: false,
            },
        }
    }
}

/// Read the compiler-owned Cargo.lock payload override used while building an internal artifact.
fn cargo_lock_payload_override(path: Option<PathBuf>) -> CliResult<Option<String>> {
    let Some(path) = path else {
        return Ok(None);
    };
    let payload = fs::read_to_string(&path).map_err(|error| {
        CliError::failure(format!(
            "failed to read internal Cargo.lock payload override {}: {error}",
            path.display()
        ))
    })?;
    Ok(Some(crate::lockfile::normalize_cargo_lock_payload(&payload)))
}

#[cfg(feature = "rust_inspect")]
pub(crate) struct RustInspectTypecheckRequest<'a> {
    pub project_root: &'a Path,
    pub project_name: &'a str,
    pub manifest: Option<&'a ProjectManifest>,
    pub modules: &'a [ParsedModule],
    pub library_manifest_index: &'a LibraryManifestIndex,
    pub cargo_features: &'a CargoFeatureSelection,
    pub cargo_policy: &'a CargoPolicy,
    pub rust_edition: Option<String>,
    pub provider_plan: &'a ProviderPlan,
}

#[cfg(feature = "rust_inspect")]
pub(crate) struct PreparedRustInspectTypecheckWorkspace {
    manifest_dir: PathBuf,
    _cache_lease: Option<GeneratedCacheLease>,
    _source_loaf: Option<OvenToolchainLoaf>,
}

#[cfg(feature = "rust_inspect")]
impl PreparedRustInspectTypecheckWorkspace {
    /// Return the generated Cargo workspace used for rust-inspect typechecking.
    pub(crate) fn manifest_dir(&self) -> &Path {
        &self.manifest_dir
    }
}

#[cfg(feature = "rust_inspect")]
pub(crate) struct RustInspectWorkspaceRequest<'a> {
    pub project_root: &'a Path,
    pub project_name: &'a str,
    pub cargo_package_name: &'a str,
    pub rust_edition: Option<String>,
    pub resolved: &'a ResolvedDependencies,
    pub project_requirements: &'a ProjectRequirements,
    pub lock_payload: Option<String>,
    pub cargo_lock_projection_root: Option<&'a str>,
    pub clear_cargo_lock: bool,
    pub cargo_policy_flags: Vec<String>,
    pub cargo_target_dir: &'a Path,
    pub rust_inspect_query_paths: &'a [String],
    pub prepare_when_empty: bool,
    /// Select rust-analyzer's direct source graph before any inspection action can run Cargo.
    pub direct_oven_inspection: bool,
    /// The named Loaf publisher must materialize provider-source metadata even when ordinary lazy prewarm is
    /// off.
    pub force_direct_prewarm: bool,
    /// Receipt inputs used to select the exact immutable Loaf that owns registry inspection sources.
    pub oven_source_authority: Option<OvenRustInspectSourceAuthorityRequest<'a>>,
}

/// Receipt-compatible Loaf inputs required before a normal direct Oven metadata prewarm.
#[cfg(feature = "rust_inspect")]
pub(crate) struct OvenRustInspectSourceAuthorityRequest<'a> {
    pub project_version: &'a str,
    pub target: &'a str,
    pub toolchain: &'a str,
    pub profile: &'a str,
    pub features: &'a [String],
    pub build_unit_inputs: &'a BTreeMap<String, String>,
    pub registry_dependencies: &'a [DependencySpec],
}

/// Prepared projection and any generation lock retaining its source-owning Loaf through semantic analysis.
#[cfg(feature = "rust_inspect")]
pub(crate) struct PreparedRustInspectWorkspace {
    manifest_dir: PathBuf,
    _source_loaf: Option<OvenToolchainLoaf>,
}

#[cfg(feature = "rust_inspect")]
impl PreparedRustInspectWorkspace {
    /// Return the compiler-authored manifest directory while this workspace retains its source Loaf.
    pub(crate) fn manifest_dir(&self) -> &Path {
        &self.manifest_dir
    }
}

/// Prepare and prewarm the generated Rust workspace used for rust-inspect metadata queries.
#[cfg(feature = "rust_inspect")]
pub(crate) fn prepare_rust_inspect_workspace(
    request: RustInspectWorkspaceRequest<'_>,
) -> CliResult<Option<PreparedRustInspectWorkspace>> {
    let RustInspectWorkspaceRequest {
        project_root,
        project_name,
        cargo_package_name,
        rust_edition,
        resolved,
        project_requirements,
        lock_payload,
        cargo_lock_projection_root,
        clear_cargo_lock,
        cargo_policy_flags,
        cargo_target_dir,
        rust_inspect_query_paths,
        prepare_when_empty,
        direct_oven_inspection,
        force_direct_prewarm,
        oven_source_authority,
    } = request;
    if rust_inspect_query_paths.is_empty() && !prepare_when_empty {
        return Ok(None);
    }

    let rust_inspect_manifest_dir = ensure_rust_inspect_workspace_with_cargo_package_name(
        project_root,
        project_name,
        cargo_package_name,
        rust_edition,
        resolved,
        project_requirements,
        lock_payload,
        cargo_lock_projection_root,
        clear_cargo_lock,
        cargo_target_dir,
        &cargo_policy_flags,
    )?;
    let mut source_loaf = None;
    if direct_oven_inspection {
        if std::env::var_os(crate::oven::loaf::OVEN_LOAF_ENV).is_some_and(|value| value == "1") {
            let source = std::env::var_os(OVEN_LEGACY_CARGO_INSPECTION_AUTHORITY_ENV)
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
                .ok_or_else(|| {
                    CliError::failure("explicit Loaf baker did not supply its locked Rust inspection source authority")
                })?;
            let destination =
                rust_inspect_manifest_dir.join(crate::rust_inspect::OVEN_DIRECT_INSPECTION_AUTHORITY_FILE);
            fs::copy(&source, &destination).map_err(|error| {
                CliError::failure(format!(
                    "failed to install explicit baker Rust inspection authority from {}: {error}",
                    source.display()
                ))
            })?;
        } else if let Some(authority_request) = oven_source_authority {
            let mut receipt_request = OvenGeneratedProjectRequest::new(
                project_root,
                project_name,
                authority_request.project_version,
                authority_request.target,
                authority_request.toolchain,
                authority_request.profile,
                authority_request.features.to_vec(),
            )
            .with_generated_source("generated-root", rust_inspect_manifest_dir.join("src/main.rs"));
            for (name, value) in authority_request.build_unit_inputs {
                receipt_request = receipt_request.with_build_unit_input(name, value);
            }
            let receipt = receipt_generated_project(&receipt_request).map_err(|error| {
                CliError::failure(format!("failed to receipt Oven Rust inspection source: {error}"))
            })?;
            let selected = resolve_toolchain_loaf_for_registry_sources(
                &receipt,
                authority_request.registry_dependencies,
            )
            .map_err(|error| CliError::failure(error.to_string()))?
            .ok_or_else(|| {
                CliError::failure(
                    "Oven Alpha has no receipt-compatible Loaf containing the requested Rust inspection sources",
                )
            })?;
            let sources = selected
                .artifacts
                .registry_sources
                .iter()
                .map(|package| crate::rust_inspect::OvenInspectionRegistrySource {
                    package: package.package.clone(),
                    version: package.version.clone(),
                    registry: package.source.registry.clone(),
                    checksum: package.source.checksum.clone(),
                    features: package.features.clone(),
                    source_root: selected.artifact_root.join(&package.source.relative_root),
                    source_digest: package.source.digest.clone(),
                })
                .collect();
            crate::rust_inspect::write_oven_inspection_source_authority(&rust_inspect_manifest_dir, sources)
                .map_err(|error| CliError::failure(format!("failed to install Loaf Rust source authority: {error}")))?;
            let sealed_lock = selected.artifact_root.join(OVEN_RUSTC_REGISTRY_LOCK_RELATIVE_PATH);
            if sealed_lock.is_file() {
                install_oven_registry_lock(&sealed_lock, &rust_inspect_manifest_dir.join("Cargo.lock"))?;
            }
            source_loaf = Some(selected);
        }
        mark_oven_direct_rust_inspection(&rust_inspect_manifest_dir)?;
    }
    prewarm_rust_inspect_workspace(
        &rust_inspect_manifest_dir,
        cargo_target_dir,
        rust_inspect_query_paths,
        force_direct_prewarm,
    )?;
    Ok(Some(PreparedRustInspectWorkspace {
        manifest_dir: rust_inspect_manifest_dir,
        _source_loaf: source_loaf,
    }))
}

/// Install a sealed registry lock as writable caller-owned inspection state.
///
/// Loaf artifacts are intentionally read-only. Copying their filesystem permissions into the mutable inspection
/// workspace makes the first projection impossible to replace on reuse, so only the verified bytes cross this
/// ownership boundary.
#[cfg(feature = "rust_inspect")]
fn install_oven_registry_lock(source: &Path, destination: &Path) -> CliResult<()> {
    let payload = fs::read(source).map_err(|error| {
        CliError::failure(format!(
            "failed to read Loaf registry lock from {}: {error}",
            source.display()
        ))
    })?;
    if destination.exists() {
        fs::remove_file(destination).map_err(|error| {
            CliError::failure(format!(
                "failed to replace projected Loaf registry lock {}: {error}",
                destination.display()
            ))
        })?;
    }
    fs::write(destination, payload).map_err(|error| {
        CliError::failure(format!(
            "failed to install Loaf registry lock from {}: {error}",
            source.display()
        ))
    })
}

/// Prepare the rust-inspect workspace needed before metadata-backed typechecking.
#[cfg(feature = "rust_inspect")]
pub(crate) fn prepare_rust_inspect_typecheck_workspace(
    request: RustInspectTypecheckRequest<'_>,
) -> CliResult<Option<PreparedRustInspectTypecheckWorkspace>> {
    let RustInspectTypecheckRequest {
        project_root,
        project_name,
        manifest,
        modules,
        library_manifest_index,
        cargo_features,
        cargo_policy,
        rust_edition,
        provider_plan,
    } = request;
    let metadata_query_paths = collect_rust_inspect_query_paths(modules);
    if metadata_query_paths.is_empty() {
        return Ok(None);
    }

    let project_requirements = collect_project_requirements(modules, library_manifest_index)?;
    let inline_imports = modules
        .iter()
        .flat_map(|module| collect_rust_dependency_uses(module, false))
        .collect::<Vec<_>>();
    let mut resolved = match resolve_reachable_dependencies(manifest, &inline_imports, true, cargo_features) {
        Ok(resolved) => resolved,
        Err(errors) => {
            let mut msg = String::new();
            let sources = build_source_map(modules);
            for err in errors {
                msg.push_str(&format_dependency_error(&err, &sources));
            }
            return Err(CliError::failure(msg.trim_end()));
        }
    };
    merge_project_requirement_dependencies(&mut resolved, &project_requirements)?;
    let lock_resolution = resolve_lock_context(LockResolutionRequest {
        project_root,
        project_name,
        entry_file: modules.last().map(|module| module.file_path.as_path()),
        manifest,
        resolved: &resolved,
        project_requirements: &project_requirements,
        cargo_features,
        cargo_policy,
        semantic: None,
        package_features: None,
        sdk_profile_override: None,
    })?;
    let cargo_lock_inputs = lock_resolution.cargo_lock_authority.into_generator_inputs();
    let rust_inspect_cargo_flags = cargo_command_flags(cargo_policy, cargo_features);
    let managed_target = resolve_generated_cargo_target(
        None,
        project_root,
        project_root,
        &lock_resolution.cargo_package_name,
        "rust-inspect",
        cargo_lock_inputs.payload.as_deref(),
        cargo_features,
        &rust_inspect_cargo_flags,
    )
    .map_err(|error| CliError::failure(format!("failed to prepare rust-inspect Cargo cache: {error}")))?;
    let (cargo_target_dir, cache_lease, _cache_identity) = managed_target.into_parts();
    let oven_build_inputs = super::build::oven_build_unit_inputs(
        provider_plan,
        &lock_resolution.project_requirements,
        &lock_resolution.resolved,
    )?;
    let rustc = resolve_active_rustc().map_err(|error| CliError::failure(error.to_string()))?;
    let target = rustc_host_target(&rustc).map_err(|error| CliError::failure(error.to_string()))?;
    let toolchain = rustc_identity(&rustc).map_err(|error| CliError::failure(error.to_string()))?;
    let project_version = manifest
        .and_then(|manifest| manifest.project.as_ref())
        .and_then(|project| project.version.as_deref())
        .unwrap_or("0.1.0");
    let manifest_dir = prepare_rust_inspect_workspace(RustInspectWorkspaceRequest {
        project_root,
        project_name,
        cargo_package_name: &lock_resolution.cargo_package_name,
        rust_edition,
        resolved: &lock_resolution.resolved,
        project_requirements: &lock_resolution.project_requirements,
        lock_payload: cargo_lock_inputs.payload,
        cargo_lock_projection_root: cargo_lock_inputs.projection_root.as_deref(),
        clear_cargo_lock: cargo_lock_inputs.clear_existing,
        cargo_policy_flags: rust_inspect_cargo_flags,
        cargo_target_dir: &cargo_target_dir,
        rust_inspect_query_paths: &metadata_query_paths,
        prepare_when_empty: false,
        // This workspace supports ordinary `incan check` metadata queries. It is an inspectable projection only:
        // Rust-analyzer must load its receipt-derived `rust-project.json`, not rediscover a Cargo workspace from a
        // path dependency. Besides respecting the normal Oven boundary, that avoids Cargo's global package-name
        // ambiguity for independently selected workspace members.
        direct_oven_inspection: true,
        force_direct_prewarm: false,
        oven_source_authority: Some(OvenRustInspectSourceAuthorityRequest {
            project_version,
            target: &target,
            toolchain: &toolchain,
            profile: "debug",
            features: &cargo_features.cargo_features,
            build_unit_inputs: &oven_build_inputs,
            registry_dependencies: &lock_resolution.resolved.dependencies,
        }),
    })?;
    Ok(manifest_dir.map(|workspace| PreparedRustInspectTypecheckWorkspace {
        manifest_dir: workspace.manifest_dir,
        _cache_lease: cache_lease,
        _source_loaf: workspace._source_loaf,
    }))
}

/// Resolve the canonical Oven lock context that normal generated-project callers consume as one unit.
///
/// Manifest-less single-file builds retain their caller context and have no lock payload. Manifest-backed builds use
/// the semantic Incan lock as their authority: a missing non-strict lock is published without constructing a Cargo
/// project, while a stale lock is never reused as an authority. The explicit `legacy_cargo` publisher owns the
/// separate historical Cargo-resolution boundary.
pub(crate) fn resolve_lock_context(request: LockResolutionRequest<'_>) -> CliResult<LockResolution> {
    let LockResolutionRequest {
        project_root,
        project_name,
        entry_file,
        manifest,
        resolved,
        project_requirements,
        cargo_features,
        cargo_policy,
        semantic,
        package_features,
        sdk_profile_override,
    } = request;

    let mut caller_resolved = resolved.clone();
    merge_project_requirement_dependencies(&mut caller_resolved, project_requirements)?;

    if manifest.is_none() {
        return Ok(LockResolution {
            cargo_lock_authority: CargoLockAuthority::None,
            cargo_package_name: project_name.to_string(),
            resolved: caller_resolved,
            project_requirements: project_requirements.clone(),
        });
    }

    if std::env::var_os(SDK_PROVIDER_BUILD_ENV).is_some()
        && let Some(payload) = cargo_lock_payload_override(
            std::env::var_os(INTERNAL_CARGO_LOCK_PAYLOAD_PATH_ENV)
                .filter(|path| !path.is_empty())
                .map(PathBuf::from),
        )?
    {
        return Ok(LockResolution {
            cargo_lock_authority: CargoLockAuthority::Exact { payload },
            cargo_package_name: project_name.to_string(),
            resolved: caller_resolved,
            project_requirements: project_requirements.clone(),
        });
    }

    let default_package_features = FeatureSelection::default();
    if let Some(manifest) = manifest
        && let Some(workspace) =
            WorkspaceGraph::discover(manifest.project_root()).map_err(|error| CliError::failure(error.to_string()))?
    {
        return resolve_workspace_lock_payload(WorkspaceLockResolutionRequest {
            workspace: &workspace,
            caller_project_name: project_name,
            caller_resolved: &caller_resolved,
            caller_project_requirements: project_requirements,
            caller_entry_file: entry_file,
            cargo_features,
            cargo_policy,
            package_features: package_features.unwrap_or(&default_package_features),
            sdk_profile_override,
        });
    }
    let project_context = if let Some(manifest) = manifest {
        collect_project_lock_context(
            manifest,
            entry_file,
            cargo_features,
            package_features.unwrap_or(&default_package_features),
            sdk_profile_override,
            None,
        )?
    } else {
        None
    };
    let (canonical_resolved, canonical_project_requirements) = if let Some(context) = project_context.as_ref() {
        (context.resolved.clone(), context.project_requirements.clone())
    } else {
        (caller_resolved.clone(), project_requirements.clone())
    };
    let lock_path = project_root.join("incan.lock");
    let mut canonical_resolved_with_requirements = canonical_resolved;
    merge_project_requirement_dependencies(
        &mut canonical_resolved_with_requirements,
        &canonical_project_requirements,
    )?;
    // A manifest-backed lock is project-wide, so its canonical context must win over an entrypoint-local semantic
    // snapshot. The supplied snapshot remains authoritative for manifest-less callers, where no project closure can
    // be rebuilt.
    let semantic = project_context
        .as_ref()
        .map(|context| context.semantic.clone())
        .or_else(|| semantic.cloned())
        .unwrap_or_default();
    let semantic_sdk_paths = semantic_sdk_path_dependencies(&canonical_project_requirements);
    let fingerprint = compute_resolved_fingerprint_with_sdk_paths(
        &canonical_resolved_with_requirements.dependencies,
        &canonical_resolved_with_requirements.dev_dependencies,
        cargo_features,
        Some(project_root),
        &semantic,
        &semantic_sdk_paths,
    );

    let strict = cargo_policy.locked || cargo_policy.frozen;
    if strict && let Some(message) = strict_git_source_error(&canonical_resolved_with_requirements) {
        return Err(CliError::failure(message));
    }
    if lock_path.exists() {
        let lock = IncanLock::load(&lock_path).map_err(|e| CliError::failure(e.to_string()))?;
        if lock.deps_fingerprint != fingerprint {
            if strict {
                return Err(CliError::failure(format!(
                    "incan.lock is out of date\n\n\
                     \x20 expected deps-fingerprint: {fingerprint}\n\
                     \x20   actual deps-fingerprint: {actual}\n\n\
                     This usually means your dependency inputs changed since the lock was generated:\n\n\
                     \x20 - incan.toml dependency entries changed, and/or\n\
                     \x20 - inline rust::... annotations changed, and/or\n\
                     \x20 - toolchain known-good defaults changed (if you rely on defaults)\n\
                     \x20 - Incan package-feature or SDK-profile selection changed, and/or\n\
                     \x20 - Cargo feature selection changed\n\n\
                     Fix:\n\n\
                     \x20   incan lock\n\n\
                     Tip: Pin crate versions/features explicitly in incan.toml for stability \
                     across toolchain upgrades.",
                    actual = lock.deps_fingerprint,
                )));
            }
            eprintln!(
                "warning: incan.lock is out of date; continuing without using it as Oven lock authority or \
                 rewriting it. Run `incan lock` to refresh it."
            );
            return Ok(LockResolution {
                cargo_lock_authority: CargoLockAuthority::Stale,
                cargo_package_name: project_name.to_string(),
                resolved: caller_resolved,
                project_requirements: project_requirements.clone(),
            });
        }
        return Ok(LockResolution {
            // Normal Oven execution must not materialize a generated Cargo.lock from the compatibility payload
            // retained in incan.lock. The payload is inert for this route; the semantic fingerprint above is the
            // authority that was just verified.
            cargo_lock_authority: CargoLockAuthority::None,
            cargo_package_name: project_name.to_string(),
            resolved: caller_resolved,
            project_requirements: project_requirements.clone(),
        });
    }

    if strict {
        return Err(CliError::failure("incan.lock is missing; run `incan lock`".to_string()));
    }

    generate_oven_lockfile(
        project_root,
        &canonical_resolved_with_requirements,
        &canonical_project_requirements,
        cargo_features,
        &semantic,
        None,
    )?;
    Ok(LockResolution {
        cargo_lock_authority: CargoLockAuthority::None,
        cargo_package_name: project_name.to_string(),
        resolved: caller_resolved,
        project_requirements: project_requirements.clone(),
    })
}

/// Validate an existing canonical Incan lock for an Oven consumer without publishing any missing SDK/provider or
/// dependency artifacts.
///
/// This derives the same manifest/source/semantic fingerprint from the active SDK inventory and parser-visible
/// dependency metadata without launching Cargo to make missing state appear.
pub(crate) fn validate_oven_lock_policy(
    project_root: &Path,
    manifest: Option<&ProjectManifest>,
    entry_file: &Path,
    cargo_features: &CargoFeatureSelection,
    cargo_policy: &CargoPolicy,
    package_features: &FeatureSelection,
    sdk_profile_override: Option<&str>,
) -> CliResult<()> {
    if !cargo_policy.locked && !cargo_policy.frozen {
        return Ok(());
    }
    let Some(manifest) = manifest else {
        return Ok(());
    };

    if let Some(workspace) =
        WorkspaceGraph::discover(manifest.project_root()).map_err(|error| CliError::failure(error.to_string()))?
    {
        let context = collect_workspace_lock_context(
            &workspace,
            Some(entry_file),
            cargo_features,
            package_features,
            sdk_profile_override,
        )?;
        let mut resolved = context.resolved;
        merge_project_requirement_dependencies(&mut resolved, &context.project_requirements)?;
        if let Some(message) = strict_git_source_error(&resolved) {
            return Err(CliError::failure(message));
        }
        let fingerprint = compute_resolved_fingerprint_with_sdk_paths(
            &resolved.dependencies,
            &resolved.dev_dependencies,
            cargo_features,
            Some(workspace.root()),
            &context.semantic,
            &semantic_sdk_path_dependencies(&context.project_requirements),
        );
        return validate_oven_existing_lock(
            &workspace.root().join("incan.lock"),
            &fingerprint,
            "workspace incan.lock is missing; run `incan lock` from any workspace member or the workspace root",
            "workspace incan.lock",
        );
    }

    let context = collect_project_lock_context(
        manifest,
        Some(entry_file),
        cargo_features,
        package_features,
        sdk_profile_override,
        None,
    )?
    .ok_or_else(|| CliError::failure("incan lock requires a FILE argument or at least one [project.scripts] entry"))?;
    let mut resolved = context.resolved;
    merge_project_requirement_dependencies(&mut resolved, &context.project_requirements)?;
    if let Some(message) = strict_git_source_error(&resolved) {
        return Err(CliError::failure(message));
    }
    let fingerprint = compute_resolved_fingerprint_with_sdk_paths(
        &resolved.dependencies,
        &resolved.dev_dependencies,
        cargo_features,
        Some(project_root),
        &context.semantic,
        &semantic_sdk_path_dependencies(&context.project_requirements),
    );
    validate_oven_existing_lock(
        &project_root.join("incan.lock"),
        &fingerprint,
        "incan.lock is missing; run `incan lock`",
        "incan.lock",
    )
}

/// Compare one already-published canonical lock with an Oven-derived fingerprint.
fn validate_oven_existing_lock(
    lock_path: &Path,
    fingerprint: &str,
    missing_message: &str,
    lock_label: &str,
) -> CliResult<()> {
    if !lock_path.exists() {
        return Err(CliError::failure(missing_message));
    }
    let lock = IncanLock::load(lock_path).map_err(|error| CliError::failure(error.to_string()))?;
    if lock.deps_fingerprint == fingerprint {
        return Ok(());
    }
    Err(CliError::failure(format!(
        "{lock_label} is out of date\n\n\
         \x20 expected deps-fingerprint: {fingerprint}\n\
         \x20   actual deps-fingerprint: {}\n\n\
         Run `incan lock` to refresh the canonical lock before using strict Oven execution.",
        lock.deps_fingerprint
    )))
}

/// Resolve or generate the canonical root lock for a workspace-aware compiler invocation.
///
/// The member that triggered this call is deliberately absent from the path calculation: a workspace lock is built
/// from every member context and lives only at the workspace root.
struct WorkspaceLockResolutionRequest<'a> {
    workspace: &'a WorkspaceGraph,
    caller_project_name: &'a str,
    caller_resolved: &'a ResolvedDependencies,
    caller_project_requirements: &'a ProjectRequirements,
    caller_entry_file: Option<&'a Path>,
    cargo_features: &'a CargoFeatureSelection,
    cargo_policy: &'a CargoPolicy,
    package_features: &'a FeatureSelection,
    sdk_profile_override: Option<&'a str>,
}

/// Resolve the canonical workspace-root Oven lock from every member plus the caller's backend refinements.
fn resolve_workspace_lock_payload(request: WorkspaceLockResolutionRequest<'_>) -> CliResult<LockResolution> {
    let WorkspaceLockResolutionRequest {
        workspace,
        caller_project_name,
        caller_resolved,
        caller_project_requirements,
        caller_entry_file,
        cargo_features,
        cargo_policy,
        package_features,
        sdk_profile_override,
    } = request;
    let context = collect_workspace_lock_context(
        workspace,
        caller_entry_file,
        cargo_features,
        package_features,
        sdk_profile_override,
    )?;
    let mut resolved = context.resolved;
    let requirements = context.project_requirements;
    merge_project_requirement_dependencies(&mut resolved, &requirements)?;
    let semantic_sdk_paths = semantic_sdk_path_dependencies(&requirements);
    let fingerprint = compute_resolved_fingerprint_with_sdk_paths(
        &resolved.dependencies,
        &resolved.dev_dependencies,
        cargo_features,
        Some(workspace.root()),
        &context.semantic,
        &semantic_sdk_paths,
    );
    let strict = cargo_policy.locked || cargo_policy.frozen;
    if strict && let Some(message) = strict_git_source_error(&resolved) {
        return Err(CliError::failure(message));
    }

    let caller_resolved = caller_resolved.clone();

    let lock_path = workspace.root().join("incan.lock");
    if lock_path.exists() {
        let lock = IncanLock::load(&lock_path).map_err(|error| CliError::failure(error.to_string()))?;
        if lock.deps_fingerprint != fingerprint {
            if strict {
                return Err(CliError::failure(format!(
                    "workspace incan.lock is out of date\n\n\
                     \x20 expected deps-fingerprint: {fingerprint}\n\
                     \x20   actual deps-fingerprint: {}\n\n\
                     Run `incan lock` from any workspace member or the workspace root to refresh the canonical lock.",
                    lock.deps_fingerprint
                )));
            }
            eprintln!(
                "warning: workspace incan.lock is out of date; continuing without using it as Oven lock authority \
                 or rewriting it. Run `incan lock` to refresh it."
            );
            return Ok(LockResolution {
                cargo_lock_authority: CargoLockAuthority::Stale,
                cargo_package_name: caller_project_name.to_string(),
                resolved: caller_resolved,
                project_requirements: caller_project_requirements.clone(),
            });
        }
        return Ok(LockResolution {
            cargo_lock_authority: CargoLockAuthority::None,
            cargo_package_name: caller_project_name.to_string(),
            resolved: caller_resolved,
            project_requirements: caller_project_requirements.clone(),
        });
    }

    if strict {
        return Err(CliError::failure(
            "workspace incan.lock is missing; run `incan lock` from any workspace member or the workspace root",
        ));
    }

    let publication_lock = crate::lockfile::acquire_publication_lock(&workspace.root().join("incan.lock"))
        .map_err(|error| CliError::failure(format!("failed to acquire workspace lock publication guard: {error}")))?;
    generate_oven_lockfile(
        workspace.root(),
        &resolved,
        &requirements,
        cargo_features,
        &context.semantic,
        Some(&publication_lock),
    )?;
    Ok(LockResolution {
        cargo_lock_authority: CargoLockAuthority::None,
        cargo_package_name: caller_project_name.to_string(),
        resolved: caller_resolved,
        project_requirements: caller_project_requirements.clone(),
    })
}

/// Fully collected dependency inputs that define a manifest project's lock freshness surface.
struct ProjectLockContext {
    resolved: ResolvedDependencies,
    project_requirements: ProjectRequirements,
    semantic: SemanticLockState,
}

/// The complete dependency inputs for a canonical workspace-root lockfile.
struct WorkspaceLockContext {
    resolved: ResolvedDependencies,
    project_requirements: ProjectRequirements,
    semantic: SemanticLockState,
}

/// Collect every member's effective dependency inputs before lock generation.
///
/// Crucially, this does not accept command scope: RFC 077 makes the root lock a property of the whole graph, so a
/// command started in one member cannot narrow the fingerprint or omit another member's feature activation.
fn collect_workspace_lock_context(
    workspace: &WorkspaceGraph,
    entry_file: Option<&Path>,
    cargo_features: &CargoFeatureSelection,
    package_features: &FeatureSelection,
    sdk_profile_override: Option<&str>,
) -> CliResult<WorkspaceLockContext> {
    let explicit_entry = entry_file.map(resolve_explicit_lock_entry).transpose()?;
    let explicit_entry_owner = explicit_entry
        .as_deref()
        .and_then(|entry| workspace.member_containing_path(entry));
    if let Some(entry) = explicit_entry.as_deref()
        && explicit_entry_owner.is_none()
    {
        return Err(CliError::failure(format!(
            "lock entry {} is not contained by any selected workspace member",
            entry.display()
        )));
    }
    let mut resolved = ResolvedDependencies {
        dependencies: Vec::new(),
        dev_dependencies: Vec::new(),
    };
    let mut project_requirements = ProjectRequirements::default();
    let mut member_semantics = Vec::new();
    let mut has_context = false;

    for member in workspace.members() {
        let manifest = workspace
            .effective_member_manifest(member)
            .map_err(|error| CliError::failure(error.to_string()))?;
        enforce_project_toolchain_constraint(&manifest)?;
        let member_entry = explicit_entry
            .as_deref()
            .filter(|_| explicit_entry_owner.is_some_and(|owner| owner.root() == member.root()));
        let Some(member_context) = collect_project_lock_context(
            &manifest,
            member_entry,
            cargo_features,
            package_features,
            sdk_profile_override,
            Some(workspace),
        )?
        else {
            continue;
        };
        has_context = true;
        resolved = merge_workspace_resolved_dependencies(&resolved, &member_context.resolved)?;
        project_requirements =
            merge_workspace_project_requirements(&project_requirements, &member_context.project_requirements)?;
        member_semantics.push((member.root().to_path_buf(), member_context.semantic));
    }

    if !has_context {
        return Err(CliError::failure(
            "incan lock requires a FILE argument or at least one [project.scripts] entry across the workspace",
        ));
    }
    let semantic = workspace_semantic_lock_state(workspace.root(), member_semantics).map_err(CliError::failure)?;
    Ok(WorkspaceLockContext {
        resolved,
        project_requirements,
        semantic,
    })
}

/// Canonicalize one optional command-line entry before assigning it to a member.
fn resolve_explicit_lock_entry(entry_file: &Path) -> CliResult<PathBuf> {
    let candidate = if entry_file.is_absolute() {
        entry_file.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|error| CliError::failure(format!("failed to determine current directory: {error}")))?
            .join(entry_file)
    };
    fs::canonicalize(&candidate)
        .map_err(|error| CliError::failure(format!("failed to resolve lock entry {}: {error}", candidate.display())))
}

/// Merge member dependency sets into Cargo's workspace-wide feature union without allowing identity drift.
fn merge_workspace_resolved_dependencies(
    current: &ResolvedDependencies,
    extra: &ResolvedDependencies,
) -> CliResult<ResolvedDependencies> {
    let mut merged = current.clone();
    for candidate in &extra.dependencies {
        merge_workspace_dependency(&mut merged.dependencies, &mut merged.dev_dependencies, candidate, false)?;
    }
    for candidate in &extra.dev_dependencies {
        merge_workspace_dependency(&mut merged.dependencies, &mut merged.dev_dependencies, candidate, true)?;
    }
    merged
        .dependencies
        .sort_by(|left, right| left.crate_name.cmp(&right.crate_name));
    merged
        .dev_dependencies
        .sort_by(|left, right| left.crate_name.cmp(&right.crate_name));
    Ok(merged)
}

/// Merge one member request; normal dependency use wins over dev-only use while features/defaults become a union.
fn merge_workspace_dependency(
    dependencies: &mut Vec<DependencySpec>,
    dev_dependencies: &mut Vec<DependencySpec>,
    candidate: &DependencySpec,
    dev_only: bool,
) -> CliResult<()> {
    if let Some(existing) = dependencies
        .iter_mut()
        .find(|spec| spec.crate_name == candidate.crate_name)
    {
        merge_workspace_dependency_spec(existing, candidate)?;
        return Ok(());
    }

    if let Some(index) = dev_dependencies
        .iter()
        .position(|spec| spec.crate_name == candidate.crate_name)
    {
        let mut existing = dev_dependencies.remove(index);
        merge_workspace_dependency_spec(&mut existing, candidate)?;
        if dev_only {
            dev_dependencies.push(existing);
        } else {
            dependencies.push(existing);
        }
        return Ok(());
    }

    if dev_only {
        dev_dependencies.push(candidate.clone());
    } else {
        dependencies.push(candidate.clone());
    }
    Ok(())
}

/// Merge only Cargo-unifiable member refinements; source, version, rename, and identity remain exact.
fn merge_workspace_dependency_spec(existing: &mut DependencySpec, candidate: &DependencySpec) -> CliResult<()> {
    if existing.version != candidate.version
        || existing.source != candidate.source
        || existing.package != candidate.package
    {
        return Err(CliError::failure(format!(
            "dependency `{}` has incompatible workspace member identities; align version, source, and package at the workspace root",
            candidate.crate_name
        )));
    }
    existing.features.extend(candidate.features.iter().cloned());
    existing.features.sort();
    existing.features.dedup();
    existing.default_features |= candidate.default_features;
    existing.optional &= candidate.optional;
    Ok(())
}

/// Merge stdlib/provider requirements with the same feature-union rules used for member Rust dependencies.
fn merge_workspace_project_requirements(
    current: &ProjectRequirements,
    extra: &ProjectRequirements,
) -> CliResult<ProjectRequirements> {
    let mut stdlib_features = current.stdlib_features.clone();
    stdlib_features.extend(extra.stdlib_features.iter().cloned());
    stdlib_features.sort();
    stdlib_features.dedup();
    let mut dependencies = current.dependencies.clone();
    for candidate in &extra.dependencies {
        if let Some(existing) = dependencies
            .iter_mut()
            .find(|spec| spec.crate_name == candidate.crate_name)
        {
            merge_workspace_dependency_spec(existing, candidate)?;
        } else {
            dependencies.push(candidate.clone());
        }
    }
    dependencies.sort_by(|left, right| left.crate_name.cmp(&right.crate_name));
    let mut sdk_dependency_rebindings = current.sdk_dependency_rebindings.clone();
    sdk_dependency_rebindings.extend(extra.sdk_dependency_rebindings.iter().cloned());
    sdk_dependency_rebindings.sort_by(|left, right| {
        (
            &left.containing_artifact.crate_root,
            &left.provider_name,
            &left.dependency_key,
            &left.source_crate_root,
            &left.active_crate_root,
        )
            .cmp(&(
                &right.containing_artifact.crate_root,
                &right.provider_name,
                &right.dependency_key,
                &right.source_crate_root,
                &right.active_crate_root,
            ))
    });
    sdk_dependency_rebindings.dedup();
    let mut sdk_path_dependencies = current.sdk_path_dependencies.clone();
    for candidate in &extra.sdk_path_dependencies {
        if let Some(existing) = sdk_path_dependencies
            .iter()
            .find(|dependency| dependency.crate_name == candidate.crate_name)
        {
            if existing != candidate {
                return Err(CliError::failure(format!(
                    "SDK/toolchain path dependency `{}` conflicts between workspace requirement contexts",
                    candidate.crate_name
                )));
            }
        } else {
            sdk_path_dependencies.push(candidate.clone());
        }
    }
    sdk_path_dependencies.sort_by(|left, right| left.crate_name.cmp(&right.crate_name));
    let mut sdk_artifact_projections = current.sdk_artifact_projections.clone();
    sdk_artifact_projections.extend(extra.sdk_artifact_projections.iter().cloned());
    sdk_artifact_projections.sort_by(|left, right| left.artifact.crate_root.cmp(&right.artifact.crate_root));
    sdk_artifact_projections.dedup_by(|left, right| left.artifact.crate_root == right.artifact.crate_root);
    Ok(ProjectRequirements {
        stdlib_features,
        dependencies,
        sdk_dependency_rebindings,
        sdk_path_dependencies,
        sdk_artifact_projections,
    })
}

/// Test-file dependency inputs that must participate in the same project lock fingerprint as normal scripts.
struct TestLockInputs {
    inline_imports: Vec<InlineRustImport>,
    project_requirement_modules: Vec<ParsedModule>,
    provider_module_groups: Vec<Vec<ParsedModule>>,
}

/// Include provider imports scoped inside `module tests:` when building the project-wide lock context.
fn lock_provider_used_module_paths(modules: &[ParsedModule]) -> BTreeSet<Vec<String>> {
    let mut used = provider_used_module_paths(modules);
    for module in modules {
        for declaration in &module.ast.declarations {
            let Declaration::TestModule(test_module) = &declaration.node else {
                continue;
            };
            for test_declaration in &test_module.body {
                let Declaration::Import(import) = &test_declaration.node else {
                    continue;
                };
                let path = match &import.kind {
                    ImportKind::Module(path) | ImportKind::From { module: path, .. }
                        if path.parent_levels == 0
                            && !path.is_absolute
                            && path.segments.first().map(String::as_str) == Some(stdlib::STDLIB_ROOT) =>
                    {
                        Some(path.segments.clone())
                    }
                    _ => None,
                };
                used.extend(path);
            }
        }
    }
    used
}

/// Return sorted manifest script and conventional library entry paths plus an optional explicitly requested entry.
///
/// A root library need not expose a runnable script. Canonical project and workspace locks nevertheless cover its
/// `src/lib.incn` graph, so a rooted RFC 077 workspace can publish one complete lock from its root without inventing
/// a command-only entrypoint.
fn project_lock_entry_paths(manifest: &ProjectManifest, explicit_entry_file: Option<&Path>) -> Vec<PathBuf> {
    let mut paths = BTreeSet::new();
    if let Some(project) = &manifest.project {
        for script in project.scripts.values() {
            paths.insert(manifest.project_root().join(script));
        }
    }
    let library_entry = manifest.project_root().join("src/lib.incn");
    if library_entry.is_file() {
        paths.insert(library_entry);
    }
    if let Some(file) = explicit_entry_file {
        paths.insert(file.to_path_buf());
    }
    paths.into_iter().collect()
}

/// Collect the project-wide script and owned test dependency inputs used for lock generation and freshness checks.
///
/// When `workspace` is present, descendant member tests are excluded so every test is resolved against the effective
/// manifest of its deepest owning workspace member. Standalone projects retain unrestricted recursive discovery.
fn collect_project_lock_context(
    manifest: &ProjectManifest,
    explicit_entry_file: Option<&Path>,
    cargo_features: &CargoFeatureSelection,
    package_features: &FeatureSelection,
    sdk_profile_override: Option<&str>,
    workspace: Option<&WorkspaceGraph>,
) -> CliResult<Option<ProjectLockContext>> {
    let entry_paths = project_lock_entry_paths(manifest, explicit_entry_file);
    if entry_paths.is_empty() {
        return Ok(None);
    }

    let sdk_inventory = discover_active_sdk_inventory()?;
    let package_feature_plan =
        PackageFeaturePlan::resolve_with_sdk_inventory(manifest, package_features, sdk_inventory.as_deref())
            .map_err(|error| CliError::failure(error.to_string()))?;
    let active_dependencies = package_feature_plan
        .root_package()
        .map(|package| package.active_dependencies.clone())
        .unwrap_or_default();
    let mut modules = Vec::new();
    let mut provider_module_groups = Vec::new();
    for entry_path in entry_paths {
        let session = CompilationSession::discover_for_oven(&entry_path, package_features, sdk_profile_override)?;
        let entry_modules = collect_modules_detailed_with_session(entry_path.clone(), &session)
            .map_err(|failure| CliError::failure(failure.render_human()))?;
        modules.extend(entry_modules.iter().cloned());
        provider_module_groups.push(entry_modules);
    }

    // Each Oven session above prepares a missing local `pub::` dependency before collecting its entry. Re-open the
    // index only after that preparation so the first published lock records the same checked artifact identity that
    // every later `lock` observes. A parser-source placeholder here would make the first lock stale as soon as the
    // preparation wrote its `.incnlib` manifest.
    let library_manifest_index = LibraryManifestIndex::from_project_manifest_dependencies(
        manifest,
        active_dependencies.iter().map(String::as_str),
    );
    let library_imported_vocab = library_manifest_index.library_imported_vocab();
    let library_imported_dsl_surfaces = library_manifest_index.library_imported_dsl_surfaces();

    let sdk_selection =
        SdkComponentSelection::from_manifest_with_profile_override(Some(manifest), sdk_profile_override);
    let sdk_components = sdk_inventory
        .as_ref()
        .map(|inventory| {
            resolve_sdk_component_selection(inventory, &sdk_selection, Some(manifest), sdk_profile_override, true)
        })
        .transpose()?;
    let provider_catalog = ProviderPlan::from_resolved_inputs(
        library_manifest_index.clone(),
        Some(&package_feature_plan),
        sdk_inventory.as_deref(),
        sdk_components.as_ref(),
        std::iter::empty(),
    )
    .map_err(|error| CliError::failure(error.to_string()))?;

    let test_inputs = collect_test_lock_inputs(
        manifest.project_root(),
        workspace,
        Some(&library_imported_vocab),
        Some(&library_imported_dsl_surfaces),
        Some(&library_manifest_index),
        &provider_catalog,
    )?;

    let mut project_requirement_modules = modules.clone();
    project_requirement_modules.extend(test_inputs.project_requirement_modules);
    let mut project_requirements = collect_project_requirements(&project_requirement_modules, &library_manifest_index)?;
    let provider_plan = ProviderPlan::from_resolved_inputs(
        library_manifest_index.clone(),
        Some(&package_feature_plan),
        sdk_inventory.as_deref(),
        sdk_components.as_ref(),
        lock_provider_used_module_paths(&project_requirement_modules),
    )
    .map_err(|error| CliError::failure(error.to_string()))?;
    provider_module_groups.extend(test_inputs.provider_module_groups.iter().cloned());
    for module_group in &provider_module_groups {
        let entry_provider_plan = ProviderPlan::from_resolved_inputs(
            library_manifest_index.clone(),
            Some(&package_feature_plan),
            sdk_inventory.as_deref(),
            sdk_components.as_ref(),
            lock_provider_used_module_paths(module_group),
        )
        .map_err(|error| CliError::failure(error.to_string()))?;
        extend_requirements_with_provider_plan(&mut project_requirements, &entry_provider_plan)?;
    }
    let semantic_sdk_paths = semantic_sdk_path_dependencies(&project_requirements);
    let semantic = semantic_lock_state(
        manifest.project_root(),
        manifest.oven_interop(),
        sdk_inventory.as_deref(),
        sdk_components.as_ref(),
        Some(&package_feature_plan),
        &provider_plan,
        &semantic_sdk_paths,
    )
    .map_err(CliError::failure)?;

    let mut inline_imports = Vec::new();
    for module in &modules {
        inline_imports.extend(collect_rust_dependency_uses(module, false));
    }
    inline_imports.extend(test_inputs.inline_imports);

    let mut resolved =
        resolve_reachable_dependencies(Some(manifest), &inline_imports, true, cargo_features).map_err(|errors| {
            let mut msg = String::new();
            let sources = build_source_map(&project_requirement_modules);
            for err in errors {
                msg.push_str(&format_dependency_error(&err, &sources));
            }
            CliError::failure(msg.trim_end())
        })?;
    merge_project_requirement_dependencies(&mut resolved, &project_requirements)?;
    Ok(Some(ProjectLockContext {
        resolved,
        project_requirements,
        semantic,
    }))
}

#[cfg(test)]
#[allow(dead_code)]
struct LockDependencyPreheatGuard {
    path: PathBuf,
}

#[cfg(test)]
#[allow(dead_code)]
impl Drop for LockDependencyPreheatGuard {
    /// Remove the cooperative dependency-preheat lock file when the writer exits.
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

/// Return whether lock-generation dependency preheat should run for the supplied environment value.
#[cfg(test)]
#[allow(dead_code)]
fn parse_lock_dependency_preheat_env(raw: Option<&str>) -> bool {
    !matches!(raw.map(str::trim), Some("0" | "false" | "no" | "off"))
}

/// Return whether dependency preheat is enabled for this process.
#[cfg(test)]
#[allow(dead_code)]
fn lock_dependency_preheat_enabled() -> bool {
    parse_lock_dependency_preheat_env(std::env::var("INCAN_LOCK_PREHEAT").ok().as_deref())
}

/// Return the age after which an abandoned dependency-preheat lock may be reclaimed.
#[cfg(test)]
#[allow(dead_code)]
fn stale_lock_dependency_preheat_after() -> Duration {
    std::env::var("INCAN_LOCK_PREHEAT_STALE_LOCK_SECS")
        .ok()
        .and_then(|raw| raw.parse::<u64>().ok())
        .map(Duration::from_secs)
        .unwrap_or_else(|| Duration::from_secs(LOCK_DEPENDENCY_PREHEAT_STALE_LOCK_SECS))
}

/// Try to become the single dependency-preheat writer for one lock workspace.
#[cfg(test)]
#[allow(dead_code)]
fn try_acquire_lock_dependency_preheat(lock_path: &Path) -> io::Result<Option<LockDependencyPreheatGuard>> {
    if let Some(parent) = lock_path.parent() {
        fs::create_dir_all(parent)?;
    }
    match OpenOptions::new().write(true).create_new(true).open(lock_path) {
        Ok(mut file) => {
            let _ = writeln!(file, "pid={}", std::process::id());
            Ok(Some(LockDependencyPreheatGuard {
                path: lock_path.to_path_buf(),
            }))
        }
        Err(err) if err.kind() == io::ErrorKind::AlreadyExists => Ok(None),
        Err(err) => Err(err),
    }
}

/// Return whether an existing cooperative dependency-preheat lock is old enough to discard.
#[cfg(test)]
#[allow(dead_code)]
fn lock_dependency_preheat_is_stale(lock_path: &Path, stale_after: Duration) -> bool {
    let Ok(metadata) = fs::metadata(lock_path) else {
        return false;
    };
    let Ok(modified) = metadata.modified() else {
        return false;
    };
    SystemTime::now()
        .duration_since(modified)
        .is_ok_and(|age| age >= stale_after)
}

/// Return whether the recorded dependency-preheat fingerprint matches the current lock workspace.
#[cfg(test)]
#[allow(dead_code)]
fn lock_dependency_preheat_stamp_matches(stamp_path: &Path, fingerprint: &str) -> bool {
    fs::read_to_string(stamp_path)
        .map(|existing| existing.trim() == fingerprint)
        .unwrap_or(false)
}

/// Run a Cargo preheat command with inherited output so long dependency builds remain visible.
#[cfg(test)]
#[allow(dead_code)]
fn run_streamed_cargo_preheat(mut command: Command, context: &str) -> CliResult<()> {
    command.stdout(Stdio::inherit());
    command.stderr(Stdio::inherit());
    let status = command
        .status()
        .map_err(|err| CliError::failure(format!("Failed to run {context}: {err}")))?;
    if !status.success() {
        return Err(CliError::failure(format!(
            "{context} failed with status {status}; Cargo output was streamed above"
        )));
    }
    Ok(())
}

/// Add one lock-workspace input file to the dependency-preheat fingerprint.
#[cfg(test)]
#[allow(dead_code)]
fn hash_lock_dependency_preheat_file(hasher: &mut Sha256, base: &Path, path: &Path) -> io::Result<()> {
    let relative = path.strip_prefix(base).unwrap_or(path);
    hasher.update(relative.to_string_lossy().as_bytes());
    hasher.update(b"\0");
    hasher.update(fs::read(path)?);
    hasher.update(b"\0");
    Ok(())
}

/// Compute the fingerprint that decides whether a dependency preheat can be reused.
#[cfg(test)]
#[allow(dead_code)]
fn compute_dependency_preheat_fingerprint(
    lock_dir: &Path,
    cargo_flags: &[String],
    target_dir: &Path,
    namespace: &[u8],
    command_label: &str,
    fingerprint_file: &str,
    crate_root_file: &str,
) -> io::Result<String> {
    let mut hasher = Sha256::new();
    hasher.update(namespace);
    hasher.update(command_label.as_bytes());
    hasher.update(b"\0");
    hasher.update(target_dir.to_string_lossy().as_bytes());
    hasher.update(b"\0");
    for flag in cargo_flags {
        hasher.update(flag.as_bytes());
        hasher.update(b"\0");
    }
    hash_lock_dependency_preheat_file(&mut hasher, lock_dir, &lock_dir.join("Cargo.toml"))?;
    hash_lock_dependency_preheat_file(&mut hasher, lock_dir, &lock_dir.join("Cargo.lock"))?;
    hash_lock_dependency_preheat_file(&mut hasher, lock_dir, &lock_dir.join("src").join(crate_root_file))?;
    Ok(format!("{}{}", fingerprint_file, hex::encode(hasher.finalize())))
}

/// Compute the fingerprint that decides whether generated-library dependency preheat can be reused.
#[cfg(test)]
#[allow(dead_code)]
fn compute_library_dependency_preheat_fingerprint(
    lock_dir: &Path,
    cargo_flags: &[String],
    target_dir: &Path,
) -> io::Result<String> {
    compute_dependency_preheat_fingerprint(
        lock_dir,
        cargo_flags,
        target_dir,
        b"incan_library_dependency_preheat/1\0",
        "cargo build --release",
        LIBRARY_DEPENDENCY_PREHEAT_FINGERPRINT_FILE,
        "lib.rs",
    )
}

/// Compile the lock workspace dependency graph into the generated-library target/profile domain when stale.
#[cfg(test)]
#[allow(dead_code)]
pub(crate) fn run_generated_library_dependency_preheat(
    request: GeneratedLibraryDependencyPreheatRequest<'_>,
) -> CliResult<()> {
    let GeneratedLibraryDependencyPreheatRequest {
        cargo_working_dir,
        lock_dir,
        project_name,
        rust_edition,
        resolved,
        project_requirements,
        cargo_features,
        cargo_policy,
        target_dir,
        cargo_lock_payload,
        cargo_lock_projection_root,
    } = request;
    if !lock_dependency_preheat_enabled() {
        eprintln!("generated library dependency preheat: disabled by INCAN_LOCK_PREHEAT");
        return Ok(());
    }

    let cargo_flags = cargo_command_flags(cargo_policy, cargo_features);
    let preheat_context = DependencyPreheatContext {
        project_name,
        rust_edition: rust_edition.as_deref(),
        resolved,
        project_requirements,
        cargo_policy_flags: &cargo_flags,
    };
    materialize_dependency_preheat_workspace(
        lock_dir,
        &preheat_context,
        cargo_lock_payload,
        cargo_lock_projection_root,
    )?;

    let fingerprint =
        compute_library_dependency_preheat_fingerprint(lock_dir, &cargo_flags, target_dir).map_err(|err| {
            CliError::failure(format!(
                "Failed to fingerprint generated library dependency preheat: {err}"
            ))
        })?;
    let stamp_path = lock_dir.join(LIBRARY_DEPENDENCY_PREHEAT_FINGERPRINT_FILE);
    if lock_dependency_preheat_stamp_matches(&stamp_path, &fingerprint) {
        eprintln!(
            "generated library dependency preheat: up-to-date (target {}, profile release)",
            target_dir.display()
        );
        return Ok(());
    }

    eprintln!(
        "preheating Cargo dependencies for generated library builds into {} (profile release)",
        target_dir.display()
    );
    let _ = io::stderr().flush();

    let lock_path = lock_dir.join(LIBRARY_DEPENDENCY_PREHEAT_LOCK_FILE);
    let stale_after = stale_lock_dependency_preheat_after();
    let wait_start = Instant::now();
    let mut announced_wait = false;
    let guard = loop {
        if lock_dependency_preheat_stamp_matches(&stamp_path, &fingerprint) {
            eprintln!(
                "generated library dependency preheat: reused after waiting {:.2}s",
                wait_start.elapsed().as_secs_f64()
            );
            return Ok(());
        }
        match try_acquire_lock_dependency_preheat(&lock_path) {
            Ok(Some(guard)) => break guard,
            Ok(None) => {
                if lock_dependency_preheat_is_stale(&lock_path, stale_after) {
                    let _ = fs::remove_file(&lock_path);
                    continue;
                }
                if !announced_wait && wait_start.elapsed() >= Duration::from_secs(1) {
                    eprintln!("waiting for another generated library dependency preheat to finish");
                    let _ = io::stderr().flush();
                    announced_wait = true;
                }
                thread::sleep(Duration::from_millis(100));
            }
            Err(err) => {
                return Err(CliError::failure(format!(
                    "Failed to acquire generated library dependency preheat lock {}: {err}",
                    lock_path.display()
                )));
            }
        }
    };

    if lock_dependency_preheat_stamp_matches(&stamp_path, &fingerprint) {
        drop(guard);
        eprintln!("generated library dependency preheat: up-to-date after lock acquisition");
        return Ok(());
    }

    let start = Instant::now();
    let mut command = cargo_command();
    sanitize_cargo_environment(&mut command);
    configure_cargo_target(&mut command, target_dir);
    command.arg("build");
    command.arg("--release");
    command.arg("--manifest-path");
    command.arg(lock_dir.join("Cargo.toml"));
    for flag in &cargo_flags {
        command.arg(flag);
    }
    command.current_dir(cargo_working_dir);

    run_streamed_cargo_preheat(
        command,
        "cargo build --release for generated library dependency preheat",
    )?;

    fs::write(&stamp_path, &fingerprint).map_err(|err| {
        CliError::failure(format!(
            "Failed to write generated library dependency preheat fingerprint {}: {err}",
            stamp_path.display()
        ))
    })?;
    drop(guard);
    eprintln!(
        "generated library dependency preheat: ran in {:.2}s",
        start.elapsed().as_secs_f64()
    );
    Ok(())
}

/// Materialize the dependency-only generated lock workspace from the current dependency graph and committed lock
/// payload.
#[cfg(test)]
#[allow(dead_code)]
fn materialize_dependency_preheat_workspace(
    lock_dir: &Path,
    context: &DependencyPreheatContext<'_>,
    cargo_lock_payload: &str,
    cargo_lock_projection_root: Option<&str>,
) -> CliResult<()> {
    let mut generator = ProjectGenerator::new(lock_dir, context.project_name, false);
    generator.set_dependencies(context.resolved.dependencies.clone());
    generator.set_dev_dependencies(context.resolved.dev_dependencies.clone());
    generator.set_include_dev_dependencies(true);
    generator.set_rust_edition(context.rust_edition.map(ToOwned::to_owned));
    generator.set_stdlib_features(context.project_requirements.stdlib_features.clone());
    generator.set_sdk_dependency_rebindings(context.project_requirements.sdk_dependency_rebindings.clone());
    generator.set_sdk_path_dependencies(context.project_requirements.sdk_path_dependencies.clone());
    generator.set_sdk_artifact_projections(context.project_requirements.sdk_artifact_projections.clone());
    generator.set_cargo_lock_payload(Some(cargo_lock_payload.to_string()));
    generator.set_cargo_lock_projection_root(cargo_lock_projection_root.map(ToOwned::to_owned));
    generator.set_cargo_policy_flags(context.cargo_policy_flags.to_vec());
    generator
        .generate("pub fn __incan_dependency_preheat() {}")
        .map_err(|err| CliError::failure(format!("Failed to generate dependency preheat project: {err}")))?;
    generator.materialize_cargo_lock_projection().map_err(|error| {
        CliError::failure(format!(
            "Failed to project generated dependency preheat Cargo.lock: {error}"
        ))
    })?;
    Ok(())
}

/// Cargo projection text held in an Oven-native lock.
///
/// `incan.lock` retains this structurally valid inert payload for compatibility with the existing lockfile format,
/// but normal command execution neither materializes nor consumes it. The explicit `legacy_cargo` publisher owns
/// the historical exact Cargo projection below.
const INERT_CARGO_LOCK_PAYLOAD: &str = "version = 4\n";

/// Generate an Oven-native `incan.lock` without constructing a generated Cargo project.
///
/// The semantic provider graph and dependency fingerprint are the normal-command lock authority. A project that
/// needs an actual Cargo resolution is outside this path and must enter the named `legacy_cargo` publisher.
fn generate_oven_lockfile(
    project_root: &Path,
    resolved: &ResolvedDependencies,
    project_requirements: &ProjectRequirements,
    cargo_features: &CargoFeatureSelection,
    semantic: &SemanticLockState,
    publication_lock: Option<&PublicationLock>,
) -> CliResult<IncanLock> {
    let lock_path = project_root.join("incan.lock");
    let owned_publication_lock = if publication_lock.is_none() {
        Some(
            crate::lockfile::acquire_publication_lock(&lock_path)
                .map_err(|error| CliError::failure(format!("failed to acquire lock publication guard: {error}")))?,
        )
    } else {
        None
    };
    let publication_lock = publication_lock.or(owned_publication_lock.as_ref());
    let semantic_sdk_paths = semantic_sdk_path_dependencies(project_requirements);
    let fingerprint = compute_resolved_fingerprint_with_sdk_paths(
        &resolved.dependencies,
        &resolved.dev_dependencies,
        cargo_features,
        Some(project_root),
        semantic,
        &semantic_sdk_paths,
    );
    let lock = IncanLock::new_with_semantic(
        fingerprint,
        cargo_features.clone(),
        semantic.clone(),
        INERT_CARGO_LOCK_PAYLOAD.to_string(),
    );
    let publication_lock = publication_lock
        .ok_or_else(|| CliError::failure("internal error: lock generation lost its publication guard"))?;
    lock.write_while_locked(&lock_path, publication_lock)
        .map_err(|error| CliError::failure(format!("failed to write incan.lock: {error}")))?;
    Ok(lock)
}

/// Collect inline Rust crate imports and stdlib/provider requirements from test files owned by this project.
fn collect_test_lock_inputs(
    project_root: &Path,
    workspace: Option<&WorkspaceGraph>,
    library_imported_vocab: Option<&parser::ImportedLibraryVocab>,
    library_imported_dsl_surfaces: Option<&parser::ImportedLibraryDslSurfaces>,
    library_manifest_index: Option<&LibraryManifestIndex>,
    provider_plan: &ProviderPlan,
) -> CliResult<TestLockInputs> {
    let mut inline_imports = Vec::new();
    let mut project_requirement_modules = Vec::new();
    let mut provider_module_groups = Vec::new();
    let test_files = discover_project_test_files(project_root, workspace);
    let source_root = project_root.join("src");

    for file_path in test_files {
        let source = fs::read_to_string(&file_path)
            .map_err(|e| CliError::failure(format!("Failed to read test file '{}': {}", file_path.display(), e)))?;
        let tokens = lexer::lex(&source).map_err(|errs| {
            let mut msg = String::new();
            for err in &errs {
                msg.push_str(&diagnostics::format_error(&file_path.to_string_lossy(), &source, err));
            }
            CliError::failure(msg.trim_end())
        })?;
        let path_display = file_path.to_string_lossy();
        let ast = parser::parse_with_context_and_surfaces(
            &tokens,
            Some(path_display.as_ref()),
            library_imported_vocab,
            library_imported_dsl_surfaces,
        )
        .map_err(|errs| {
            let mut msg = String::new();
            for err in &errs {
                msg.push_str(&diagnostics::format_error(&file_path.to_string_lossy(), &source, err));
            }
            CliError::failure(msg.trim_end())
        })?;

        let test_module = ParsedModule {
            name: file_path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .unwrap_or("test")
                .to_string(),
            path_segments: vec!["test".to_string()],
            file_path: file_path.clone(),
            source: source.clone(),
            ast: ast.clone(),
        };
        inline_imports.extend(collect_rust_dependency_uses(&test_module, true));

        let source_modules = crate::cli::test_runner::collect_source_modules_for_test(
            &ast,
            &source_root,
            library_imported_vocab,
            library_imported_dsl_surfaces,
            library_manifest_index,
            provider_plan,
        )
        .map_err(CliError::failure)?;
        for module in &source_modules {
            inline_imports.extend(collect_rust_dependency_uses(module, false));
        }
        let mut provider_modules = vec![test_module];
        provider_modules.extend(source_modules);
        project_requirement_modules.extend(provider_modules.iter().cloned());
        provider_module_groups.push(provider_modules);
    }

    Ok(TestLockInputs {
        inline_imports,
        project_requirement_modules,
        provider_module_groups,
    })
}

/// Discover test files owned by one project, excluding descendant workspace members in rooted workspaces.
fn discover_project_test_files(project_root: &Path, workspace: Option<&WorkspaceGraph>) -> Vec<PathBuf> {
    crate::cli::test_runner::discover_test_files(project_root)
        .into_iter()
        .filter(|path| {
            workspace.is_none_or(|graph| {
                graph
                    .member_containing_path(path)
                    .is_some_and(|owner| owner.root() == project_root)
            })
        })
        .collect()
}

/// Check whether any resolved dependency uses a git branch source, which is forbidden in strict
/// (`--locked` / `--frozen`) mode.
fn strict_git_source_error(resolved: &ResolvedDependencies) -> Option<String> {
    for spec in resolved.dependencies.iter().chain(resolved.dev_dependencies.iter()) {
        if let crate::manifest::DependencySource::Git { reference, .. } = &spec.source
            && matches!(reference, crate::manifest::GitReference::Branch(_))
        {
            return Some(format!(
                "strict mode forbids git branch dependencies (crate `{}`); use tag or rev",
                spec.crate_name
            ));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::{DependencySource, DependencySpec};

    fn empty_resolved() -> ResolvedDependencies {
        ResolvedDependencies {
            dependencies: Vec::new(),
            dev_dependencies: Vec::new(),
        }
    }

    fn empty_project_requirements() -> ProjectRequirements {
        ProjectRequirements {
            stdlib_features: Vec::new(),
            dependencies: Vec::new(),
            sdk_dependency_rebindings: Vec::new(),
            sdk_path_dependencies: Vec::new(),
            sdk_artifact_projections: Vec::new(),
        }
    }

    fn registry_dependency(crate_name: &str) -> DependencySpec {
        DependencySpec {
            crate_name: crate_name.to_string(),
            version: Some("1".to_string()),
            features: Vec::new(),
            default_features: true,
            source: DependencySource::Registry,
            optional: false,
            package: None,
        }
    }

    #[test]
    fn cargo_lock_authority_exposes_only_closed_generator_input_pairs() {
        let none = CargoLockAuthority::None.into_generator_inputs();
        assert_eq!(none.payload, None);
        assert_eq!(none.projection_root, None);
        assert!(!none.clear_existing);

        let stale = CargoLockAuthority::Stale.into_generator_inputs();
        assert_eq!(stale.payload, None);
        assert_eq!(stale.projection_root, None);
        assert!(stale.clear_existing);

        let exact = CargoLockAuthority::Exact {
            payload: "exact".to_string(),
        }
        .into_generator_inputs();
        assert_eq!(exact.payload, Some("exact".to_string()));
        assert_eq!(exact.projection_root, None);
        assert!(!exact.clear_existing);
    }

    #[test]
    fn workspace_lock_merge_unifies_cargo_features_without_permitting_identity_drift()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut first = registry_dependency("serde");
        first.features = vec!["alloc".to_string()];
        first.default_features = false;
        first.optional = true;
        let mut second = registry_dependency("serde");
        second.features = vec!["derive".to_string()];

        let merged = merge_workspace_resolved_dependencies(
            &ResolvedDependencies {
                dependencies: vec![first],
                dev_dependencies: Vec::new(),
            },
            &ResolvedDependencies {
                dependencies: vec![second],
                dev_dependencies: Vec::new(),
            },
        )?;
        let serde = merged.dependencies.first().ok_or("merged serde dependency missing")?;
        assert_eq!(serde.features, vec!["alloc", "derive"]);
        assert!(serde.default_features);
        assert!(!serde.optional);

        let mut incompatible = registry_dependency("serde");
        incompatible.version = Some("2".to_string());
        let error = merge_workspace_resolved_dependencies(
            &merged,
            &ResolvedDependencies {
                dependencies: vec![incompatible],
                dev_dependencies: Vec::new(),
            },
        )
        .err()
        .ok_or("incompatible workspace dependency should fail")?;
        assert!(error.message.contains("incompatible workspace member identities"));
        Ok(())
    }

    #[test]
    fn parse_lock_dependency_preheat_env_defaults_to_enabled() {
        assert!(parse_lock_dependency_preheat_env(None));
        assert!(parse_lock_dependency_preheat_env(Some("1")));
        assert!(parse_lock_dependency_preheat_env(Some("true")));
        assert!(!parse_lock_dependency_preheat_env(Some("0")));
        assert!(!parse_lock_dependency_preheat_env(Some("false")));
        assert!(!parse_lock_dependency_preheat_env(Some(" off ")));
    }

    #[test]
    fn cargo_lock_payload_override_normalizes_the_supplied_workspace_lock() -> Result<(), Box<dyn std::error::Error>> {
        let temp_dir = tempfile::tempdir()?;
        let lock_path = temp_dir.path().join("Cargo.lock");
        fs::write(&lock_path, "version = 4\r\n")?;

        assert_eq!(
            cargo_lock_payload_override(Some(lock_path))?,
            Some("version = 4\n".to_string())
        );
        Ok(())
    }

    #[cfg(feature = "rust_inspect")]
    #[test]
    fn sealed_oven_registry_lock_can_replace_a_prior_projection() -> Result<(), Box<dyn std::error::Error>> {
        let temp_dir = tempfile::tempdir()?;
        let sealed_lock = temp_dir.path().join("sealed.lock");
        let projected_lock = temp_dir.path().join("Cargo.lock");
        fs::write(&sealed_lock, "version = 4\n")?;
        let mut sealed_permissions = fs::metadata(&sealed_lock)?.permissions();
        sealed_permissions.set_readonly(true);
        fs::set_permissions(&sealed_lock, sealed_permissions)?;

        install_oven_registry_lock(&sealed_lock, &projected_lock)?;
        let mut projected_permissions = fs::metadata(&projected_lock)?.permissions();
        projected_permissions.set_readonly(true);
        fs::set_permissions(&projected_lock, projected_permissions)?;
        install_oven_registry_lock(&sealed_lock, &projected_lock)?;

        assert_eq!(fs::read_to_string(&projected_lock)?, "version = 4\n");
        assert!(!fs::metadata(&projected_lock)?.permissions().readonly());
        Ok(())
    }

    #[test]
    fn oven_lock_generation_publishes_semantic_state_without_a_cargo_projection()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp_dir = tempfile::tempdir()?;
        let lock = generate_oven_lockfile(
            temp_dir.path(),
            &empty_resolved(),
            &empty_project_requirements(),
            &CargoFeatureSelection::default(),
            &SemanticLockState::default(),
            None,
        )?;

        assert_eq!(lock.cargo_lock_payload, INERT_CARGO_LOCK_PAYLOAD);
        assert!(temp_dir.path().join("incan.lock").is_file());
        let state_dir = crate::lockfile::compiler_lock_state_dir(temp_dir.path());
        assert!(
            !state_dir.join("Cargo.toml").exists() && !state_dir.join("target").exists(),
            "Oven lock generation may retain its publication guard but must not create generated-Cargo lock state"
        );
        Ok(())
    }

    #[test]
    fn resolver_publishes_a_missing_semantic_lock_without_a_cargo_projection() -> Result<(), Box<dyn std::error::Error>>
    {
        let temp_dir = tempfile::tempdir()?;
        let project_root = temp_dir.path();
        let manifest_path = project_root.join("incan.toml");
        let entry_path = project_root.join("src/main.incn");
        fs::create_dir_all(entry_path.parent().ok_or("entry path has no parent")?)?;
        fs::write(
            &manifest_path,
            "[project]\nname = \"semantic_lock_demo\"\nversion = \"0.1.0\"\n",
        )?;
        fs::write(&entry_path, "def main() -> None:\n  pass\n")?;
        let manifest = ProjectManifest::from_str(&fs::read_to_string(&manifest_path)?, &manifest_path)?;
        let cargo_features = CargoFeatureSelection::default();
        let cargo_policy = CargoPolicy::default();

        let resolution = resolve_lock_context(LockResolutionRequest {
            project_root,
            project_name: "semantic_lock_demo",
            entry_file: Some(&entry_path),
            manifest: Some(&manifest),
            resolved: &empty_resolved(),
            project_requirements: &empty_project_requirements(),
            cargo_features: &cargo_features,
            cargo_policy: &cargo_policy,
            semantic: Some(&SemanticLockState::default()),
            package_features: None,
            sdk_profile_override: None,
        })?;

        assert!(matches!(resolution.cargo_lock_authority, CargoLockAuthority::None));
        assert_eq!(
            IncanLock::load(&project_root.join("incan.lock"))?.cargo_lock_payload,
            INERT_CARGO_LOCK_PAYLOAD
        );
        let state_dir = crate::lockfile::compiler_lock_state_dir(project_root);
        assert!(
            !state_dir.join("Cargo.toml").exists() && !state_dir.join("Cargo.lock").exists(),
            "normal lock resolution must not construct a generated Cargo projection"
        );
        Ok(())
    }

    #[test]
    fn lock_collects_test_imported_source_modules_as_normal_deps() -> Result<(), Box<dyn std::error::Error>> {
        let temp_dir = tempfile::tempdir()?;
        let project_root = temp_dir.path();
        fs::create_dir_all(project_root.join("src"))?;
        fs::create_dir_all(project_root.join("tests"))?;
        fs::write(
            project_root.join("src").join("internal.incn"),
            "from rust::datafusion @ \"53\" import SessionContext\n",
        )?;
        fs::write(
            project_root.join("tests").join("test_internal.incn"),
            "from internal import SessionContext\nfrom rust::tokio @ \"1\" import spawn\n",
        )?;

        let inputs = collect_test_lock_inputs(project_root, None, None, None, None, &ProviderPlan::default())?;
        let imports = inputs.inline_imports;
        let tokio = imports
            .iter()
            .find(|import| import.crate_name == "tokio")
            .ok_or("expected direct test tokio import")?;
        let datafusion = imports
            .iter()
            .find(|import| import.crate_name == "datafusion")
            .ok_or("expected test-imported source module datafusion import")?;

        assert!(tokio.is_test_context);
        assert!(!datafusion.is_test_context);
        Ok(())
    }

    #[test]
    fn project_test_discovery_excludes_descendant_workspace_members() -> Result<(), Box<dyn std::error::Error>> {
        let temp_dir = tempfile::tempdir()?;
        let root = temp_dir.path();
        let consumer_root = root.join("packages/consumer");
        fs::create_dir_all(root.join("tests/nested"))?;
        fs::create_dir_all(consumer_root.join("tests"))?;
        fs::write(
            root.join("incan.toml"),
            r#"
[project]
name = "root"

[workspace]
members = ["packages/consumer"]
"#,
        )?;
        fs::write(
            consumer_root.join("incan.toml"),
            r#"
[project]
name = "consumer"
"#,
        )?;
        let root_test = root.join("tests/test_root.incn");
        let nested_root_test = root.join("tests/nested/test_nested.incn");
        let consumer_test = consumer_root.join("tests/test_consumer.incn");
        fs::write(&root_test, "def test_root() -> None:\n    pass\n")?;
        fs::write(&nested_root_test, "def test_nested() -> None:\n    pass\n")?;
        fs::write(&consumer_test, "def test_consumer() -> None:\n    pass\n")?;

        let workspace = WorkspaceGraph::load_from_root(root)?;
        let root_files = discover_project_test_files(workspace.root(), Some(&workspace));
        let consumer = workspace
            .members()
            .find(|member| member.name() == "consumer")
            .ok_or("consumer workspace member should exist")?;
        let consumer_files = discover_project_test_files(consumer.root(), Some(&workspace));
        let mut expected_root_files = vec![fs::canonicalize(root_test)?, fs::canonicalize(nested_root_test)?];
        expected_root_files.sort();

        assert_eq!(root_files, expected_root_files);
        assert_eq!(consumer_files, [fs::canonicalize(consumer_test)?]);
        Ok(())
    }

    #[test]
    fn library_dependency_preheat_fingerprint_uses_separate_profile_domain() -> Result<(), Box<dyn std::error::Error>> {
        let temp_dir = std::env::temp_dir().join(format!("incan_library_preheat_fingerprint_{}", std::process::id()));
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(temp_dir.join("src"))?;
        fs::write(
            temp_dir.join("Cargo.toml"),
            "[package]\nname = \"library_preheat\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )?;
        fs::write(
            temp_dir.join("Cargo.lock"),
            "# This file is automatically @generated by Cargo.\nversion = 4\n",
        )?;
        fs::write(temp_dir.join("src").join("lib.rs"), "pub fn library() {}\n")?;

        let target_dir = temp_dir.join("target").join(".cargo-target");
        let library_preheat = compute_library_dependency_preheat_fingerprint(&temp_dir, &[], &target_dir)?;
        assert!(library_preheat.starts_with(LIBRARY_DEPENDENCY_PREHEAT_FINGERPRINT_FILE));
        fs::write(temp_dir.join("src").join("lib.rs"), "pub fn library_changed() {}\n")?;
        let changed_library_preheat = compute_library_dependency_preheat_fingerprint(&temp_dir, &[], &target_dir)?;
        assert_ne!(
            library_preheat, changed_library_preheat,
            "generated-library preheat fingerprint must track src/lib.rs, not src/main.rs"
        );
        let _ = fs::remove_dir_all(&temp_dir);
        Ok(())
    }
}
