//! Lock file generation and resolution for Incan projects.
//!
//! Handles creating and validating `oven.lock` files that pin dependency versions for reproducible builds.
//! Used by both `incan lock` and the build pipeline.

#[cfg(test)]
use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
#[cfg(test)]
use std::fs::OpenOptions;
#[cfg(test)]
use std::io::{self, Write};
use std::path::{Path, PathBuf};
#[cfg(test)]
use std::process::{Command, Stdio};
#[cfg(feature = "rust_inspect")]
use std::sync::Arc;
#[cfg(test)]
use std::thread;
#[cfg(test)]
use std::time::{Duration, Instant, SystemTime};

#[cfg(test)]
use sha2::{Digest, Sha256};

#[cfg(test)]
use crate::backend::ProjectGenerator;
#[cfg(feature = "rust_inspect")]
use crate::backend::project::runner::resolved_cargo_executable;
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
    CargoFeatureSelection, IncanLock, LOCK_FILENAME, PublicationLock, SemanticLockState,
    compute_resolved_fingerprint_with_sdk_paths, semantic_lock_state, workspace_semantic_lock_state,
};
use crate::manifest::{DependencySpec, ProjectManifest};
use crate::oven::legacy_cargo::OvenLegacyCargoInspectionPackage;
#[cfg(feature = "rust_inspect")]
use crate::oven::legacy_cargo::{OVEN_LEGACY_CARGO_INSPECTION_AUTHORITY_ENV, explicit_project_bake_inspection_sources};
#[cfg(feature = "rust_inspect")]
use crate::oven::loaf::{
    OvenToolchainLoaf, resolve_compiler_owned_loaf_by_identity, resolve_compiler_owned_loaf_for_registry_dependencies,
    resolve_toolchain_loaf_for_registry_sources,
};
#[cfg(feature = "rust_inspect")]
use crate::oven::rustc::{
    OVEN_RUSTC_REGISTRY_LOCK_RELATIVE_PATH, OvenLoadedProjectInspectionAuthority, OvenProjectInspectionConstituent,
    OvenProjectInspectionSourceOwner, project_inspection_authority_supports_dependencies,
    project_inspection_test_dependency_envelope_supports_dependencies, validate_project_extension_payload_against_base,
};
#[cfg(feature = "rust_inspect")]
use crate::oven::rustc::{resolve_active_rustc, rustc_host_target, rustc_identity};
#[cfg(feature = "rust_inspect")]
use crate::oven::{OvenGeneratedProjectRequest, receipt_generated_project};
use crate::provider::{FeatureSelection, ProviderPlan, SDK_PROVIDER_BUILD_ENV};
use crate::workspace::WorkspaceGraph;
use incan_core::lang::stdlib;

use super::common::{
    CargoPolicy, CompilationSession, ProjectRequirements, build_source_map, cargo_command_flags,
    collect_modules_detailed_with_session, collect_project_requirements, collect_rust_dependency_uses,
    enforce_project_toolchain_constraint, extend_requirements_with_provider_plan, format_dependency_error,
    merge_project_requirement_dependencies, provider_used_module_paths, semantic_sdk_path_dependencies,
};
#[cfg(feature = "rust_inspect")]
use super::common::{collect_rust_inspect_derive_probe_paths, collect_rust_inspect_query_paths};

#[cfg(test)]
#[allow(dead_code)]
const LOCK_DEPENDENCY_PREHEAT_STALE_LOCK_SECS: u64 = 30 * 60;
#[cfg(test)]
#[allow(dead_code)]
const LIBRARY_DEPENDENCY_PREHEAT_FINGERPRINT_FILE: &str = ".incan_library_dependency_preheat_fingerprint";
#[cfg(test)]
#[allow(dead_code)]
const LIBRARY_DEPENDENCY_PREHEAT_LOCK_FILE: &str = ".incan_library_dependency_preheat.lock";

#[cfg(test)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct ProjectLockCollectionMetrics {
    context_collections: usize,
    session_discoveries: usize,
    authority_snapshot_reads: usize,
    provider_plan_projections: usize,
}

#[cfg(test)]
thread_local! {
    static PROJECT_LOCK_COLLECTION_METRICS: Cell<ProjectLockCollectionMetrics> = const {
        Cell::new(ProjectLockCollectionMetrics {
            context_collections: 0,
            session_discoveries: 0,
            authority_snapshot_reads: 0,
            provider_plan_projections: 0,
        })
    };
}

/// Reset project-lock collection metrics for the current test thread.
#[cfg(test)]
pub(crate) fn reset_project_lock_collection_metrics() {
    PROJECT_LOCK_COLLECTION_METRICS.set(ProjectLockCollectionMetrics::default());
}

/// Return the current project-lock collection metrics for this test thread.
#[cfg(test)]
fn project_lock_collection_metrics() -> ProjectLockCollectionMetrics {
    PROJECT_LOCK_COLLECTION_METRICS.get()
}

/// Apply one update to the project-lock collection metrics for this test thread.
#[cfg(test)]
fn update_project_lock_collection_metrics(update: impl FnOnce(&mut ProjectLockCollectionMetrics)) {
    let mut metrics = PROJECT_LOCK_COLLECTION_METRICS.get();
    update(&mut metrics);
    PROJECT_LOCK_COLLECTION_METRICS.set(metrics);
}

/// Return the context-collection and session-discovery counts for this test thread.
#[cfg(test)]
pub(crate) fn project_lock_collection_counts() -> (usize, usize) {
    let metrics = project_lock_collection_metrics();
    (metrics.context_collections, metrics.session_discoveries)
}

/// Record one project-lock context collection for this test thread.
#[cfg(test)]
fn record_project_lock_context_collection() {
    update_project_lock_collection_metrics(|metrics| {
        metrics.context_collections += 1;
    });
}

/// Record one project-lock compilation-session discovery for this test thread.
#[cfg(test)]
fn record_project_lock_session_discovery() {
    update_project_lock_collection_metrics(|metrics| {
        metrics.session_discoveries += 1;
    });
}

/// Record one project-lock authority snapshot read for this test thread.
#[cfg(test)]
fn record_project_lock_authority_snapshot_read() {
    update_project_lock_collection_metrics(|metrics| {
        metrics.authority_snapshot_reads += 1;
    });
}

/// Record one project-lock provider-plan projection for this test thread.
#[cfg(test)]
fn record_project_lock_provider_plan_projection() {
    update_project_lock_collection_metrics(|metrics| {
        metrics.provider_plan_projections += 1;
    });
}

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
    /// Embedded Cargo.lock payload from `oven.lock`.
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

/// Generate or update oven.lock for a project.
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
        .ok_or_else(|| CliError::failure("No loaf.toml found (run `incan init`)"))?;
    enforce_project_toolchain_constraint(&manifest)?;

    let cargo_features = CargoFeatureSelection {
        cargo_features,
        cargo_no_default_features,
        cargo_all_features,
    }
    .normalized();
    let _ = collect_and_publish_project_lock(
        &manifest,
        entry_file.map(PathBuf::as_path),
        &cargo_features,
        package_features,
        sdk_profile_override,
    )?;

    Ok(ExitCode::SUCCESS)
}

/// Collect and publish the one canonical project or workspace lock, retaining the exact immutable inputs for its
/// explicit Oven publisher.
fn collect_and_publish_project_lock(
    manifest: &ProjectManifest,
    entry_file: Option<&Path>,
    cargo_features: &CargoFeatureSelection,
    package_features: &FeatureSelection,
    sdk_profile_override: Option<&str>,
) -> CliResult<ProjectLockContext> {
    if let Some(workspace) =
        WorkspaceGraph::discover(manifest.project_root()).map_err(|error| CliError::failure(error.to_string()))?
    {
        let lock_path = workspace.root().join(LOCK_FILENAME);
        let publication_lock = crate::lockfile::acquire_publication_lock(&lock_path).map_err(|error| {
            CliError::failure(format!("failed to acquire workspace lock publication guard: {error}"))
        })?;
        let context = collect_workspace_lock_context(
            &workspace,
            entry_file,
            cargo_features,
            package_features,
            sdk_profile_override,
            None,
        )?;
        generate_oven_lockfile(
            workspace.root(),
            &context.resolved,
            &context.project_requirements,
            cargo_features,
            &context.semantic,
            Some(&publication_lock),
        )?;
        return Ok(context);
    }

    let context = collect_project_lock_context(
        manifest,
        entry_file,
        cargo_features,
        package_features,
        sdk_profile_override,
        None,
        None,
    )?
    .ok_or_else(|| CliError::failure("incan lock requires a FILE argument or at least one [project.scripts] entry"))?;
    generate_oven_lockfile(
        manifest.project_root(),
        &context.resolved,
        &context.project_requirements,
        cargo_features,
        &context.semantic,
        None,
    )?;
    Ok(context)
}

/// Checked inputs for one caller's emission requirements and its canonical semantic lock facts.
///
/// A command-owned session retains the existing provider and feature decisions. Workspace collection still includes
/// every member; its canonical lock must not be narrowed to the invoking entrypoint.
pub(crate) struct LockResolutionRequest<'a> {
    pub project_root: &'a Path,
    pub entry_file: Option<&'a Path>,
    pub manifest: Option<&'a ProjectManifest>,
    pub resolved: &'a ResolvedDependencies,
    pub project_requirements: &'a ProjectRequirements,
    pub cargo_features: &'a CargoFeatureSelection,
    pub semantic: Option<&'a SemanticLockState>,
    pub package_features: Option<&'a FeatureSelection>,
    pub sdk_profile_override: Option<&'a str>,
    pub command_session: Option<&'a CompilationSession>,
}

/// Caller-local emission requirements and the separately scoped canonical lock observation.
///
/// A semantic lock describes checked dependency/provider facts. It does not authorize a native or Rust inspection
/// projection, and an absent lock is not permission to prepare one.
pub(crate) struct LockResolution {
    pub resolved: ResolvedDependencies,
    pub project_requirements: ProjectRequirements,
    pub canonical: Option<CanonicalLockFacts>,
}

/// Observed canonical lock state relative to one retained checked fingerprint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SemanticLockStatus {
    /// No canonical lock file was present when the facts were observed.
    Missing,
    /// The observed lock fingerprint matches the retained checked inputs.
    Current,
    /// A canonical lock exists for a different checked fingerprint.
    Stale { actual_fingerprint: String },
}

/// Expected checked lock contents and a read-only observation of the canonical file.
///
/// These facts are not a native admission token. Their observation does not create a publication guard or select an
/// artifact; the invoking control plane decides whether to require or publish a lock.
pub(crate) struct CanonicalLockFacts {
    lock_path: PathBuf,
    expected: IncanLock,
    observed: SemanticLockStatus,
    strict_input_error: Option<String>,
}

impl CanonicalLockFacts {
    /// Return the standalone or workspace-root path observed by this fact collection.
    pub(crate) fn lock_path(&self) -> &Path {
        &self.lock_path
    }

    /// Return the canonical checked contents independently from whether a lock has been published.
    pub(crate) fn expected(&self) -> &IncanLock {
        &self.expected
    }

    /// Return the actual missing, current or stale observation without taking an execution decision.
    pub(crate) fn status(&self) -> &SemanticLockStatus {
        &self.observed
    }
}

/// Demanded Rust metadata for one checked compilation session.
///
/// The selected binding is a separate input from parsed modules and semantic lock facts. An absent selection cannot
/// be repaired by constructing a Cargo workspace or rediscovering registry sources.
#[cfg(feature = "rust_inspect")]
pub(crate) struct RustInspectTypecheckRequest<'a> {
    pub project_root: &'a Path,
    pub modules: &'a [ParsedModule],
    pub selected: Option<SelectedRustInspectWorkspace>,
}

/// An already selected physical projection and the admitted owners retaining its inputs.
///
/// Selection supplies the expected byte bindings and keeps every contributing source owner in these leases. This
/// container does not infer or validate provider authority from source paths; `ValidatedInspectionProject` only
/// verifies the physical projection. Cache and temporary directories must already be allocated outside its inputs.
#[cfg(feature = "rust_inspect")]
pub(crate) struct SelectedRustInspectWorkspace {
    pub context: PathBuf,
    pub temporary_root: PathBuf,
    pub projection: crate::rust_inspect::ValidatedInspectionProject,
    pub source_loaf: Option<OvenToolchainLoaf>,
    pub project_source_authorities: Option<Arc<PreparedOvenProjectRegistrySourceAuthorities>>,
}

/// One inspection demand and the optional explicit selection supplied by the invoking control plane.
#[cfg(feature = "rust_inspect")]
pub(crate) struct RustInspectWorkspaceRequest<'a> {
    pub project_root: &'a Path,
    pub rust_inspect_query_paths: &'a [String],
    pub rust_derive_probe_paths: &'a [String],
    pub selected: Option<SelectedRustInspectWorkspace>,
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

/// A cache-bound selected projection whose source leases remain live through analysis and background queries.
#[cfg(feature = "rust_inspect")]
#[derive(Clone)]
pub(crate) struct PreparedRustInspectWorkspace {
    selected: Arc<SelectedRustInspectWorkspace>,
}

/// Command-local source authority shared by every parallel native-test unit.
///
/// The completed-output, authority, constituent, and compiler-release leases stay live for this value's lifetime. A
/// batch performs only an in-memory exact-root check and projects the one already validated source catalog and lock.
#[cfg(feature = "rust_inspect")]
pub(crate) struct PreparedOvenProjectRegistrySourceAuthorities {
    authority: OvenLoadedProjectInspectionAuthority,
    sources: Vec<crate::rust_inspect::OvenInspectionRegistrySource>,
    registry_lock_source: Option<PathBuf>,
    /// Build-script output directories the explicit bake sealed below the authority root, with their package
    /// versions where the bake recorded them.
    generated_out_dirs: Vec<crate::rust_inspect::SealedGeneratedOutDir>,
    test_dependency_plan: Option<crate::cli::commands::build::OvenDirectRustcPlanSelection>,
    _release_loafs: Vec<OvenToolchainLoaf>,
}

#[cfg(feature = "rust_inspect")]
impl PreparedRustInspectWorkspace {
    /// Return the selected cache context while this handle retains its source owners.
    pub(crate) fn manifest_dir(&self) -> &Path {
        &self.selected.context
    }
}

/// Bind and prewarm an explicitly selected Rust inspection projection without acquiring missing inputs.
///
/// Missing selection is terminal before any cache/output mutation. Keeping the whole selected handle in the result
/// retains its owner leases for semantic analysis and cloned background consumers. Derive expansion remains an
/// explicitly unsupported selected operation until the physical macro boundary is implemented (#991, #1037).
#[cfg(feature = "rust_inspect")]
pub(crate) fn prepare_rust_inspect_workspace(
    request: RustInspectWorkspaceRequest<'_>,
) -> CliResult<Option<PreparedRustInspectWorkspace>> {
    use crate::rust_inspect::{Inspector, InspectorConfig, RustMetadataError};

    if request.rust_inspect_query_paths.is_empty() && request.rust_derive_probe_paths.is_empty() {
        return Ok(None);
    }
    let selected = request.selected.ok_or_else(|| {
        CliError::failure(
            RustMetadataError::SelectedInputUnavailable {
                path: request.project_root.to_path_buf(),
            }
            .to_string(),
        )
    })?;
    if selected.source_loaf.is_none() && selected.project_source_authorities.is_none() {
        return Err(CliError::failure(
            RustMetadataError::InvalidSelectedInput {
                path: selected.context,
                message: "selected inspection projection has no retained source-owner lease".to_string(),
            }
            .to_string(),
        ));
    }
    if !request.rust_derive_probe_paths.is_empty() {
        return Err(CliError::failure(
            RustMetadataError::UnsupportedSelectedOperation {
                operation: "Rust derive expansion requires selected macro execution inputs (#991, #1037)",
            }
            .to_string(),
        ));
    }
    let selected = Arc::new(selected);
    let inspector = Inspector::new(InspectorConfig::new(&selected.context));
    inspector
        .cache()
        .bind_selected_project_with_owner(
            &selected.context,
            selected.projection.clone(),
            &selected.temporary_root,
            Arc::clone(&selected),
        )
        .map_err(|error| CliError::failure(error.to_string()))?;
    // The cache owns the lease as long as its selected database exists, even if extraction fails and this local
    // preparation handle is dropped. A later validated rebind or explicit invalidation releases that exact owner.
    inspector
        .prewarm(request.rust_inspect_query_paths.iter().cloned(), &|message| {
            tracing::debug!("{message}");
        })
        .map_err(|error| CliError::failure(error.to_string()))?;
    Ok(Some(PreparedRustInspectWorkspace { selected }))
}

/// Return whether a normal direct-inspection consumer must refuse generic release-source selection.
///
/// A prepared command authority is direct evidence that the caller belongs to a manifest-backed project, including
/// projects with a custom source root or scripts outside `src`. Conventional source paths retain the earlier defensive
/// check for callers that have not yet propagated that authority. Explicit baking and true standalone files may still
/// select release-owned inspection sources.
#[cfg(feature = "rust_inspect")]
fn normal_inspection_requires_installed_project_authority(
    project_root: &Path,
    explicit_oven_bake: bool,
    command_authority_available: bool,
    command_authority_installed: bool,
) -> bool {
    let conventional_project =
        project_root.join("src/lib.incn").is_file() || project_root.join("src/main.incn").is_file();
    !explicit_oven_bake && !command_authority_installed && (command_authority_available || conventional_project)
}

/// Install one sealed registry-source catalog into a direct Oven inspection workspace.
#[cfg(feature = "rust_inspect")]
fn install_oven_inspection_source_authority(
    manifest_dir: &Path,
    packages: &[crate::oven::rustc::OvenRustcRegistrySourcePackage],
    artifact_root: &Path,
    extension_paths: Option<&BTreeSet<String>>,
    base_source_authority: Option<(&Path, &[crate::oven::rustc::OvenRustcRegistrySourcePackage])>,
) -> CliResult<()> {
    let sources = oven_inspection_sources(packages, artifact_root, extension_paths, base_source_authority)?;
    crate::rust_inspect::write_sealed_oven_inspection_source_authority(manifest_dir, sources)
        .map(|_| ())
        .map_err(|error| CliError::failure(format!("failed to install Oven Rust source authority: {error}")))
}

/// Resolve a sealed registry catalog to immutable source roots without writing caller-owned projection state.
#[cfg(feature = "rust_inspect")]
fn oven_inspection_sources(
    packages: &[crate::oven::rustc::OvenRustcRegistrySourcePackage],
    artifact_root: &Path,
    extension_paths: Option<&BTreeSet<String>>,
    base_source_authority: Option<(&Path, &[crate::oven::rustc::OvenRustcRegistrySourcePackage])>,
) -> CliResult<Vec<crate::rust_inspect::OvenInspectionRegistrySource>> {
    packages
        .iter()
        .map(|package| {
            let extension_owns_source = extension_paths.is_some_and(|paths| {
                let prefix = format!("{}/", package.source.relative_root);
                paths.iter().any(|path| path.starts_with(&prefix))
            });
            let source_root = if extension_paths.is_none() || extension_owns_source {
                artifact_root.join(&package.source.relative_root)
            } else {
                let (base_root, base_packages) = base_source_authority.ok_or_else(|| {
                    CliError::failure(format!(
                        "stored Oven project extension omits sealed source `{}` {} without an exact base Loaf authority",
                        package.package, package.version
                    ))
                })?;
                let mut matching = base_packages.iter().filter(|base| {
                    base.package == package.package
                        && base.version == package.version
                        && base.source.registry == package.source.registry
                        && base.source.checksum == package.source.checksum
                        && base.source.digest == package.source.digest
                });
                let Some(base) = matching.next() else {
                    return Err(CliError::failure(format!(
                        "stored Oven project extension source `{}` {} is absent from its required base Loaf",
                        package.package, package.version
                    )));
                };
                if matching.next().is_some() {
                    return Err(CliError::failure(format!(
                        "stored Oven project extension source `{}` {} is ambiguous in its required base Loaf",
                        package.package, package.version
                    )));
                }
                base_root.join(&base.source.relative_root)
            };
            Ok(crate::rust_inspect::OvenInspectionRegistrySource {
                package: package.package.clone(),
                version: package.version.clone(),
                registry: package.source.registry.clone(),
                checksum: package.source.checksum.clone(),
                features: package.features.clone(),
                source_root,
                source_digest: package.source.digest.clone(),
            })
        })
        .collect()
}

/// Return whether a sealed source catalog owns the requested immutable Cargo source archive.
///
/// This check guards source-root hand-off only; it does not authorize reuse of compiled Rust artifacts, whose
/// feature-sensitive receipts are validated by the direct-rustc plan.
#[cfg(feature = "rust_inspect")]
fn registry_source_is_owned_by_catalog(
    source: &crate::oven::rustc::OvenRustcRegistrySourcePackage,
    catalog: &[crate::oven::rustc::OvenRustcRegistrySourcePackage],
) -> bool {
    catalog.iter().any(|candidate| {
        candidate.package == source.package && candidate.version == source.version && candidate.source == source.source
    })
}

/// Resolve one exact project authority and all named constituents once for the complete test command.
#[cfg(feature = "rust_inspect")]
pub(crate) fn prepare_project_registry_source_authorities(
    mut authority: OvenLoadedProjectInspectionAuthority,
) -> CliResult<Arc<PreparedOvenProjectRegistrySourceAuthorities>> {
    struct ResolvedSourceOwner {
        root: PathBuf,
        catalog: Vec<crate::oven::rustc::OvenRustcRegistrySourcePackage>,
    }

    let mut release_loafs = Vec::new();
    let mut owners = Vec::with_capacity(authority.payload.constituents.len());
    let mut stored_index = 0;
    let mut test_dependency_stored_index = None;
    let mut test_dependency_release_identity = None;
    for (constituent_index, constituent) in authority.payload.constituents.iter().enumerate() {
        match constituent {
            OvenProjectInspectionConstituent::ReleaseLoaf {
                loaf_identity,
                build_unit_identity,
                receipt,
            } => {
                let loaf = resolve_compiler_owned_loaf_by_identity(receipt, loaf_identity)
                    .map_err(|error| CliError::failure(error.to_string()))?
                    .ok_or_else(|| {
                        CliError::failure(format!(
                            "project inspection authority requires release Loaf `{loaf_identity}`, but the active toolchain does not provide it"
                        ))
                    })?;
                if loaf.loaf_build_unit_identity != *build_unit_identity || loaf.artifacts.intent != receipt.intent {
                    return Err(CliError::failure(format!(
                        "project inspection authority release Loaf `{loaf_identity}` has different build-unit or intent evidence"
                    )));
                }
                owners.push(ResolvedSourceOwner {
                    root: loaf.artifact_root.clone(),
                    catalog: loaf.artifacts.registry_sources.clone(),
                });
                if authority
                    .payload
                    .test_dependency_envelope
                    .as_ref()
                    .is_some_and(|envelope| envelope.constituent_index == constituent_index)
                {
                    test_dependency_release_identity = Some(loaf.loaf_identity.clone());
                }
                release_loafs.push(loaf);
            }
            OvenProjectInspectionConstituent::Stored {
                artifact_kind,
                base_loaf_identity,
                ..
            } => {
                if authority
                    .payload
                    .test_dependency_envelope
                    .as_ref()
                    .is_some_and(|envelope| envelope.constituent_index == constituent_index)
                {
                    test_dependency_stored_index = Some(stored_index);
                }
                let selected = authority.stored_constituents.get(stored_index).ok_or_else(|| {
                    CliError::failure("project inspection authority lost a store constituent during preparation")
                })?;
                stored_index += 1;
                let catalog = match artifact_kind {
                    crate::oven::store::OvenArtifactKind::DirectRustcPlan => {
                        serde_json::from_slice::<crate::oven::rustc::OvenRustcArtifactManifest>(&selected.payload)
                            .map_err(|error| {
                                CliError::failure(format!(
                                    "project inspection direct-plan constituent is invalid: {error}"
                                ))
                            })?
                            .registry_sources
                    }
                    crate::oven::store::OvenArtifactKind::ProjectPayload => {
                        let payload =
                            serde_json::from_slice::<crate::oven::native_contract::OvenProjectExtensionPayload>(
                                &selected.payload,
                            )
                            .map_err(|error| {
                                CliError::failure(format!(
                                    "project inspection extension constituent is invalid: {error}"
                                ))
                            })?;
                        let base_identity = base_loaf_identity.as_deref().ok_or_else(|| {
                            CliError::failure("project inspection extension constituent omitted its release Loaf")
                        })?;
                        let base = release_loafs
                            .iter()
                            .find(|loaf| loaf.loaf_identity == base_identity)
                            .ok_or_else(|| {
                                CliError::failure(format!(
                                    "project inspection extension requires unlisted release Loaf `{base_identity}`"
                                ))
                            })?;
                        validate_project_extension_payload_against_base(
                            &payload,
                            &base.loaf_identity,
                            &base.loaf_build_unit_identity,
                            &base.artifacts,
                        )
                        .map_err(|error| CliError::failure(error.to_string()))?;
                        payload.complete_plan.registry_sources
                    }
                    unsupported => {
                        return Err(CliError::failure(format!(
                            "project inspection authority names unsupported constituent kind {unsupported:?}"
                        )));
                    }
                };
                owners.push(ResolvedSourceOwner {
                    root: selected.artifact_root.clone(),
                    catalog,
                });
            }
        }
    }

    let mut sources = Vec::with_capacity(authority.payload.registry_sources.len());
    for source in &authority.payload.registry_sources {
        let (root, catalog) = match source.owner {
            OvenProjectInspectionSourceOwner::Authority => {
                (authority.artifact_root.as_path(), std::slice::from_ref(&source.package))
            }
            OvenProjectInspectionSourceOwner::Constituent { index } => {
                let owner = owners.get(index).ok_or_else(|| {
                    CliError::failure(format!(
                        "project inspection source references missing constituent index {index}"
                    ))
                })?;
                (owner.root.as_path(), owner.catalog.as_slice())
            }
        };
        if !registry_source_is_owned_by_catalog(&source.package, catalog) {
            return Err(CliError::failure(format!(
                "project inspection source `{}` {} has no exact record in its named owner",
                source.package.package, source.package.version
            )));
        }
        sources.push(crate::rust_inspect::OvenInspectionRegistrySource {
            package: source.package.package.clone(),
            version: source.package.version.clone(),
            registry: source.package.source.registry.clone(),
            checksum: source.package.source.checksum.clone(),
            features: source.package.features.clone(),
            source_root: root.join(&source.package.source.relative_root),
            source_digest: source.package.source.digest.clone(),
        });
    }
    let registry_lock_source = if sources.is_empty() {
        None
    } else {
        let path = authority.artifact_root.join(OVEN_RUSTC_REGISTRY_LOCK_RELATIVE_PATH);
        let metadata = fs::symlink_metadata(&path).map_err(|error| {
            CliError::failure(format!(
                "project inspection authority lacks its sealed Cargo.lock at {}: {error}",
                path.display()
            ))
        })?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(CliError::failure(format!(
                "project inspection authority registry lock is not a regular file at {}",
                path.display()
            )));
        }
        Some(path)
    };
    // The explicit bake sealed its Cargo bootstrap's build-script output below the authority root; those files are
    // the only Cargo-free source of generated Rust (prost modules, for one) a direct inspection workspace can read.
    let generated_out_dirs = authority
        .payload
        .generated_out_dirs
        .iter()
        .map(|dir| crate::rust_inspect::SealedGeneratedOutDir {
            out_dir: authority.artifact_root.join(&dir.relative_root),
            version: dir.version.clone(),
        })
        .collect::<Vec<_>>();
    let test_dependency_plan = if let Some(stored_index) = test_dependency_stored_index {
        let constituent_index = authority
            .payload
            .test_dependency_envelope
            .as_ref()
            .ok_or_else(|| CliError::failure("project inspection authority lost its test dependency role"))?
            .constituent_index;
        let receipt = match authority.payload.constituents.get(constituent_index) {
            Some(OvenProjectInspectionConstituent::Stored { receipt, .. }) => receipt.clone(),
            _ => {
                return Err(CliError::failure(
                    "project inspection authority test dependency role no longer names a stored constituent",
                ));
            }
        };
        let selected = authority.stored_constituents.remove(stored_index);
        Some(crate::cli::commands::build::project_test_dependency_plan_from_constituent(selected, &receipt)?)
    } else if let Some(identity) = test_dependency_release_identity {
        let index = release_loafs
            .iter()
            .position(|loaf| loaf.loaf_identity == identity)
            .ok_or_else(|| CliError::failure("project inspection authority lost its role-bearing release Loaf"))?;
        Some(
            crate::cli::commands::build::OvenDirectRustcPlanSelection::ToolchainLoaf(Box::new(
                release_loafs.remove(index),
            )),
        )
    } else {
        if authority.payload.test_dependency_envelope.is_some() {
            return Err(CliError::failure(
                "project inspection authority test dependency role did not resolve to its exact constituent",
            ));
        }
        None
    };
    Ok(Arc::new(PreparedOvenProjectRegistrySourceAuthorities {
        authority,
        sources,
        registry_lock_source,
        generated_out_dirs,
        test_dependency_plan,
        _release_loafs: release_loafs,
    }))
}

#[cfg(feature = "rust_inspect")]
impl PreparedOvenProjectRegistrySourceAuthorities {
    /// Return the exact role-bearing dependency envelope after validating this generated batch's complete surface.
    pub(crate) fn test_dependency_plan(
        &self,
        dependencies: &[DependencySpec],
    ) -> CliResult<Option<&crate::cli::commands::build::OvenDirectRustcPlanSelection>> {
        if self.authority.payload.test_dependency_envelope.is_none() {
            return Ok(None);
        }
        let promoted = crate::cli::commands::build::promoted_oven_test_dependencies(&ResolvedDependencies {
            dependencies: dependencies.to_vec(),
            dev_dependencies: Vec::new(),
        })?;
        if !project_inspection_authority_supports_dependencies(&self.authority.payload, &promoted) {
            return Err(project_inspection_selection_mismatch("this test dependency subset"));
        }
        if !project_inspection_test_dependency_envelope_supports_dependencies(&self.authority.payload, &promoted)
            .map_err(|error| CliError::failure(error.to_string()))?
        {
            let expected = self
                .authority
                .payload
                .test_dependency_envelope
                .as_ref()
                .map(|envelope| envelope.dependency_roots.keys().cloned().collect::<Vec<_>>().join(", "))
                .unwrap_or_default();
            let actual = promoted
                .iter()
                .map(|dependency| dependency.crate_name.replace('-', "_"))
                .collect::<Vec<_>>()
                .join(", ");
            return Err(CliError::failure(format!(
                "Oven Alpha project inspection authority has a missing, stale, or incompatible test dependency root (sealed aliases: [{expected}]; requested aliases: [{actual}]); rerun `incan oven bake --project .`"
            )));
        }
        self.test_dependency_plan.as_ref().map(Some).ok_or_else(|| {
            CliError::failure(
                "project inspection authority lost its exact test dependency plan while retaining its role",
            )
        })
    }

    /// Bind generated test receipts to the exact project authority selected once for this command.
    pub(crate) fn authority_identity(&self) -> &str {
        &self.authority.identity
    }

    /// Project the one complete exact authority for one generated test batch.
    fn install_for_dependencies(&self, manifest_dir: &Path, dependencies: &[DependencySpec]) -> CliResult<bool> {
        let registry_dependency_count = dependencies
            .iter()
            .filter(|dependency| matches!(dependency.source, crate::manifest::DependencySource::Registry))
            .count();
        if registry_dependency_count == 0 {
            // The exact project/output/authority leases are still the conventional project's command authority. No
            // registry projection is needed, but this must not fall through to generic compatibility selection.
            return Ok(true);
        }
        if !project_inspection_authority_supports_dependencies(&self.authority.payload, dependencies) {
            return Err(project_inspection_selection_mismatch(
                "the requested normal and dev registry dependencies",
            ));
        }
        crate::rust_inspect::write_sealed_oven_inspection_source_authority(manifest_dir, self.sources.clone())
            .map_err(|error| CliError::failure(format!("failed to install Oven Rust source authority: {error}")))?;
        crate::rust_inspect::write_oven_generated_out_dirs(manifest_dir, &self.generated_out_dirs).map_err(
            |error| CliError::failure(format!("failed to install Oven generated output directories: {error}")),
        )?;
        if let Some(lock) = self.registry_lock_source.as_deref() {
            install_oven_registry_lock(lock, &manifest_dir.join("Cargo.lock"))?;
        }
        Ok(true)
    }
}

/// Build the diagnostic for a completed project Loaf that does not cover the requested inspection surface.
#[cfg(feature = "rust_inspect")]
fn project_inspection_selection_mismatch(requested_surface: &str) -> CliError {
    CliError::failure(format!(
        "Oven Alpha project inspection authority does not cover {requested_surface}. The command selected registry roots outside the completed project Loaf's baked dependency surface. A command-local `--sdk-profile` or package-feature selection cannot reuse a Loaf baked for different roots. Use the baked selection; for a different SDK profile, persist it in `[sdk]` in `loaf.toml` and rebake; for different package features, rerun `incan oven bake --project .` with the same feature flags."
    ))
}

/// Resolve the exact direct registry roots whose source trees must be available while checking one project.
/// Translate declared registry dependencies into the exact root selectors shared by source inspection and the
/// explicit Oven publisher. Keeping this conversion here gives both phases one package/rename/version boundary.
pub(crate) fn inspection_packages_for_dependencies(
    dependencies: &[DependencySpec],
) -> CliResult<Vec<OvenLegacyCargoInspectionPackage>> {
    let mut packages = dependencies
        .iter()
        .filter(|dependency| matches!(dependency.source, crate::manifest::DependencySource::Registry))
        .map(|dependency| {
            let package = dependency.package.as_deref().unwrap_or(&dependency.crate_name);
            let version_requirement = dependency.version.as_deref().ok_or_else(|| {
                CliError::failure(format!(
                    "Oven inspection source declaration for `{package}` is missing its locked version requirement"
                ))
            })?;
            Ok(OvenLegacyCargoInspectionPackage {
                package: package.to_string(),
                version_requirement: version_requirement.to_string(),
            })
        })
        .collect::<CliResult<Vec<_>>>()?;
    packages.sort();
    packages.dedup();
    Ok(packages)
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

/// Require the exact checked graph whenever a Loaf supplies registry source authority.
///
/// The Rust-inspection loader may otherwise see the copied source directories while resolving their dependencies
/// against ambient state. A missing lock is an invalid Loaf, not permission to consult Cargo or a local registry.
#[cfg(feature = "rust_inspect")]
fn install_required_oven_registry_lock(
    has_registry_sources: bool,
    artifact_root: &Path,
    destination: &Path,
) -> CliResult<()> {
    if !has_registry_sources {
        return Ok(());
    }
    let sealed_lock = artifact_root.join(OVEN_RUSTC_REGISTRY_LOCK_RELATIVE_PATH);
    let metadata = fs::symlink_metadata(&sealed_lock).map_err(|error| {
        CliError::failure(format!(
            "selected Oven Loaf declares registry sources but lacks its sealed Cargo.lock at {}: {error}",
            sealed_lock.display()
        ))
    })?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(CliError::failure(format!(
            "selected Oven Loaf declares registry sources but its sealed Cargo.lock is not a regular file at {}",
            sealed_lock.display()
        )));
    }
    install_oven_registry_lock(&sealed_lock, destination)
}

/// Prepare demanded typecheck metadata from the separately admitted inspection selection.
///
/// Parsed Rust uses determine demand only. Checked dependency/lock services remain available to the control plane;
/// neither a successful semantic lock comparison nor a source catalog can stand in for a selected inspection graph.
#[cfg(feature = "rust_inspect")]
pub(crate) fn prepare_rust_inspect_typecheck_workspace(
    request: RustInspectTypecheckRequest<'_>,
) -> CliResult<Option<PreparedRustInspectWorkspace>> {
    let metadata_query_paths = collect_rust_inspect_query_paths(request.modules);
    let rust_derive_probe_paths = collect_rust_inspect_derive_probe_paths(request.modules);
    prepare_rust_inspect_workspace(RustInspectWorkspaceRequest {
        project_root: request.project_root,
        rust_inspect_query_paths: &metadata_query_paths,
        rust_derive_probe_paths: &rust_derive_probe_paths,
        selected: request.selected,
    })
}

/// Collect caller requirements and observe the canonical semantic lock using the existing checked analysis.
///
/// This never publishes a missing lock or projects a Cargo payload. Caller emission requirements stay local, while the
/// canonical expected lock uses every selected workspace member. An actual native or inspection plan must be admitted
/// separately.
pub(crate) fn resolve_lock_context(request: LockResolutionRequest<'_>) -> CliResult<LockResolution> {
    let LockResolutionRequest {
        project_root,
        entry_file,
        manifest,
        resolved,
        project_requirements,
        cargo_features,
        semantic,
        package_features,
        sdk_profile_override,
        command_session,
    } = request;
    if let Some(session) = command_session {
        match (manifest, session.manifest.as_ref()) {
            (Some(manifest), Some(selected))
                if project_roots_match(manifest.project_root(), selected.project_root()) => {}
            (None, None) => {}
            _ => {
                return Err(CliError::failure(
                    "lock inputs do not match the command-owned manifest authority",
                ));
            }
        }
    }
    let mut caller_resolved = resolved.clone();
    merge_project_requirement_dependencies(&mut caller_resolved, project_requirements)?;
    let Some(manifest) = manifest else {
        return Ok(LockResolution {
            resolved: caller_resolved,
            project_requirements: project_requirements.clone(),
            canonical: None,
        });
    };
    if !project_roots_match(project_root, manifest.project_root()) {
        return Err(CliError::failure(format!(
            "lock input root {} does not belong to manifest project {}",
            project_root.display(),
            manifest.project_root().display(),
        )));
    }
    let default_package_features = FeatureSelection::default();
    let package_features = package_features.unwrap_or(&default_package_features);
    let workspace =
        WorkspaceGraph::discover(manifest.project_root()).map_err(|error| CliError::failure(error.to_string()))?;
    let (canonical_root, context) = if let Some(workspace) = workspace.as_ref() {
        let context = collect_workspace_lock_context(
            workspace,
            entry_file,
            cargo_features,
            package_features,
            sdk_profile_override,
            command_session,
        )?;
        (workspace.root(), context)
    } else {
        let context = collect_project_lock_context(
            manifest,
            entry_file,
            cargo_features,
            package_features,
            sdk_profile_override,
            None,
            command_session,
        )?
        .unwrap_or_else(|| ProjectLockContext {
            resolved: caller_resolved.clone(),
            project_requirements: project_requirements.clone(),
            semantic: semantic.cloned().unwrap_or_default(),
        });
        (manifest.project_root(), context)
    };
    let strict_input_error = strict_git_source_error(&context.resolved);
    let expected = checked_oven_lock(
        canonical_root,
        &context.resolved,
        &context.project_requirements,
        cargo_features,
        &context.semantic,
    );
    let lock_path = canonical_root.join(LOCK_FILENAME);
    let observed = observe_oven_lock(&lock_path, &expected.deps_fingerprint)?;
    Ok(LockResolution {
        resolved: caller_resolved,
        project_requirements: project_requirements.clone(),
        canonical: Some(CanonicalLockFacts {
            lock_path,
            expected,
            observed,
            strict_input_error,
        }),
    })
}

/// Observe an existing semantic lock without creating directories, guards or output files.
///
/// Only an absent file is Missing. Permission errors, malformed contents and unsupported formats remain explicit
/// failures instead of weakening the observation into a cache miss.
fn observe_oven_lock(lock_path: &Path, expected_fingerprint: &str) -> CliResult<SemanticLockStatus> {
    let lock = match IncanLock::load(lock_path) {
        Ok(lock) => lock,
        Err(crate::lockfile::LockfileError::Read { source, .. }) if source.kind() == std::io::ErrorKind::NotFound => {
            return Ok(SemanticLockStatus::Missing);
        }
        Err(error) => return Err(CliError::failure(error.to_string())),
    };
    Ok(if lock.deps_fingerprint == expected_fingerprint {
        SemanticLockStatus::Current
    } else {
        SemanticLockStatus::Stale {
            actual_fingerprint: lock.deps_fingerprint,
        }
    })
}

/// Apply strict lock consistency to a retained observation without collecting a second graph or publishing state.
///
/// Mutable Git requirements retain their existing strict refusal. A Current observation only proves semantic
/// fingerprint equality; native and inspection artifact admission still require their own selected inputs.
pub(crate) fn validate_oven_existing_lock(facts: &CanonicalLockFacts) -> CliResult<()> {
    if let Some(message) = facts.strict_input_error.as_ref() {
        return Err(CliError::failure(message));
    }
    match facts.status() {
        SemanticLockStatus::Missing => Err(CliError::failure(format!(
            "oven.lock is missing; run `incan lock` (canonical path: {})",
            facts.lock_path().display(),
        ))),
        SemanticLockStatus::Current => Ok(()),
        SemanticLockStatus::Stale { actual_fingerprint } => Err(CliError::failure(format!(
            "oven.lock is out of date at {}\n\n expected deps-fingerprint: {}\n   actual deps-fingerprint: {actual_fingerprint}\n\nRun `incan lock` to refresh the canonical lock before using strict Oven execution.",
            facts.lock_path().display(),
            facts.expected().deps_fingerprint,
        ))),
    }
}

/// Fully collected dependency inputs that define a project or workspace lock's freshness surface.
struct ProjectLockContext {
    resolved: ResolvedDependencies,
    project_requirements: ProjectRequirements,
    semantic: SemanticLockState,
}

/// Canonical lock publication retained by one explicit project bake.
///
/// The dependency surface is the exact normal and test closure used to publish `oven.lock`. Keeping it behind this
/// immutable projection prevents source-authority publication from rediscovering the same project graph.
pub(crate) struct PublishedOvenProjectLock {
    dependency_surface: ResolvedDependencies,
}

impl PublishedOvenProjectLock {
    /// Return the exact normal and test dependency surface used to publish the canonical lock.
    pub(crate) fn dependency_surface(&self) -> &ResolvedDependencies {
        &self.dependency_surface
    }
}

/// Publish the canonical project or workspace lock and retain its exact dependency surface for inspection authority.
///
/// Test-only imports and provider requirements are included in the same whole-project walk used by `incan lock`.
/// The explicit baker passes this returned surface forward instead of entering the collector a second time.
pub(crate) fn publish_oven_project_lock(
    project_root: &Path,
    entrypoint: &Path,
    package_features: &FeatureSelection,
) -> CliResult<PublishedOvenProjectLock> {
    let manifest = ProjectManifest::discover(project_root)
        .map_err(|error| CliError::failure(error.to_string()))?
        .ok_or_else(|| CliError::failure("explicit Oven project bake requires an loaf.toml project"))?;
    enforce_project_toolchain_constraint(&manifest)?;
    let cargo_features = CargoFeatureSelection::default().normalized();
    let context =
        collect_and_publish_project_lock(&manifest, Some(entrypoint), &cargo_features, package_features, None)?;
    Ok(PublishedOvenProjectLock {
        dependency_surface: context.resolved,
    })
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
    command_session: Option<&CompilationSession>,
) -> CliResult<ProjectLockContext> {
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
            command_session.filter(|session| {
                session
                    .manifest
                    .as_ref()
                    .is_some_and(|session_manifest| project_roots_match(session_manifest.project_root(), member.root()))
            }),
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
    Ok(ProjectLockContext {
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

/// Compare project ownership independently of the caller's relative or symlinked spelling.
fn project_roots_match(left: &Path, right: &Path) -> bool {
    match (fs::canonicalize(left), fs::canonicalize(right)) {
        (Ok(left), Ok(right)) => left == right,
        _ => left == right,
    }
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
    command_session: Option<&CompilationSession>,
) -> CliResult<Option<ProjectLockContext>> {
    #[cfg(test)]
    record_project_lock_context_collection();
    let command_session_manifest = command_session
        .map(|session| {
            session.manifest.as_ref().ok_or_else(|| {
                CliError::failure(format!(
                    "project lock collection has no manifest authority for {}",
                    manifest.project_root().display()
                ))
            })
        })
        .transpose()?;
    if command_session_manifest
        .is_some_and(|session_manifest| !project_roots_match(session_manifest.project_root(), manifest.project_root()))
    {
        return Err(CliError::failure(format!(
            "command-owned compilation session belongs to a different project than {}",
            manifest.project_root().display()
        )));
    }
    let seed_manifest = command_session_manifest.unwrap_or(manifest);
    let seed_entry_paths = project_lock_entry_paths(seed_manifest, explicit_entry_file);
    if seed_entry_paths.is_empty() {
        return Ok(None);
    }

    let session_entry = seed_entry_paths
        .first()
        .ok_or_else(|| CliError::failure("project lock collection lost its source entrypoints"))?;
    let discovered_session = if command_session.is_none() {
        #[cfg(test)]
        record_project_lock_session_discovery();
        Some(CompilationSession::discover_for_oven(
            session_entry,
            package_features,
            sdk_profile_override,
        )?)
    } else {
        None
    };
    let session = command_session
        .or(discovered_session.as_ref())
        .ok_or_else(|| CliError::failure("project lock collection lost its compilation session"))?;
    let session_manifest = session.manifest.as_ref().ok_or_else(|| {
        CliError::failure(format!(
            "project lock collection has no manifest authority for {}",
            manifest.project_root().display()
        ))
    })?;
    if command_session.is_none() && !project_roots_match(session_manifest.project_root(), manifest.project_root()) {
        return Err(CliError::failure(format!(
            "command-owned compilation session belongs to a different project than {}",
            manifest.project_root().display()
        )));
    }
    #[cfg(test)]
    record_project_lock_authority_snapshot_read();
    let entry_paths = project_lock_entry_paths(session_manifest, explicit_entry_file);
    if entry_paths.is_empty() {
        return Ok(None);
    }
    let mut modules = Vec::new();
    for entry_path in entry_paths {
        let entry_modules = collect_modules_detailed_with_session(entry_path.clone(), session)
            .map_err(|failure| CliError::failure(failure.render_human()))?;
        modules.extend(entry_modules.iter().cloned());
    }

    let TestLockInputs {
        inline_imports: test_inline_imports,
        project_requirement_modules: test_requirement_modules,
    } = collect_test_lock_inputs(
        session_manifest.project_root(),
        workspace,
        Some(&session.library_imported_vocab),
        Some(&session.library_imported_dsl_surfaces),
        Some(&session.library_manifest_index),
        session.provider_plan.as_ref(),
        session,
    )?;

    let mut inline_imports = Vec::new();
    for module in &modules {
        inline_imports.extend(collect_rust_dependency_uses(module, false));
    }
    inline_imports.extend(test_inline_imports);
    let mut project_requirement_modules = modules;
    project_requirement_modules.extend(test_requirement_modules);
    let mut project_requirements =
        collect_project_requirements(&project_requirement_modules, &session.library_manifest_index)?;
    #[cfg(test)]
    record_project_lock_provider_plan_projection();
    let provider_plan =
        session.provider_plan_for_used_module_paths(lock_provider_used_module_paths(&project_requirement_modules))?;
    extend_requirements_with_provider_plan(&mut project_requirements, &provider_plan)?;
    let semantic_sdk_paths = semantic_sdk_path_dependencies(&project_requirements);
    let semantic = semantic_lock_state(
        session_manifest.project_root(),
        session_manifest.interop_c(),
        session.sdk_inventory.as_deref(),
        session.sdk_components.as_ref(),
        session.package_feature_plan.as_ref(),
        &provider_plan,
        &semantic_sdk_paths,
    )
    .map_err(CliError::failure)?;

    let mut resolved = resolve_reachable_dependencies(Some(session_manifest), &inline_imports, true, cargo_features)
        .map_err(|errors| {
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

/// Construct expected lock contents from retained checked facts without observing or publishing a file.
fn checked_oven_lock(
    project_root: &Path,
    resolved: &ResolvedDependencies,
    project_requirements: &ProjectRequirements,
    cargo_features: &CargoFeatureSelection,
    semantic: &SemanticLockState,
) -> IncanLock {
    let semantic_sdk_paths = semantic_sdk_path_dependencies(project_requirements);
    let fingerprint = compute_resolved_fingerprint_with_sdk_paths(
        &resolved.dependencies,
        &resolved.dev_dependencies,
        cargo_features,
        Some(project_root),
        semantic,
        &semantic_sdk_paths,
    );
    IncanLock::new_with_semantic(fingerprint, cargo_features.clone(), semantic.clone())
}

/// Publish the supplied checked dependency and provider facts as the canonical semantic `oven.lock`.
///
/// The publication guard coordinates the physical write. This service does not select native inputs or materialize a
/// Cargo dependency projection.
fn generate_oven_lockfile(
    project_root: &Path,
    resolved: &ResolvedDependencies,
    project_requirements: &ProjectRequirements,
    cargo_features: &CargoFeatureSelection,
    semantic: &SemanticLockState,
    publication_lock: Option<&PublicationLock>,
) -> CliResult<IncanLock> {
    let lock_path = project_root.join(LOCK_FILENAME);
    let owned_publication_lock = if publication_lock.is_none() {
        Some(
            crate::lockfile::acquire_publication_lock(&lock_path)
                .map_err(|error| CliError::failure(format!("failed to acquire lock publication guard: {error}")))?,
        )
    } else {
        None
    };
    let publication_lock = publication_lock.or(owned_publication_lock.as_ref());
    let lock = checked_oven_lock(project_root, resolved, project_requirements, cargo_features, semantic);
    let publication_lock = publication_lock
        .ok_or_else(|| CliError::failure("internal error: lock generation lost its publication guard"))?;
    lock.write_while_locked(&lock_path, publication_lock)
        .map_err(|error| CliError::failure(format!("failed to write oven.lock: {error}")))?;
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
    session: &CompilationSession,
) -> CliResult<TestLockInputs> {
    let mut inline_imports = Vec::new();
    let mut project_requirement_modules = Vec::new();
    let test_files = discover_project_test_files(project_root, workspace, session);
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
        project_requirement_modules.push(test_module);
        project_requirement_modules.extend(source_modules);
    }

    Ok(TestLockInputs {
        inline_imports,
        project_requirement_modules,
    })
}

/// Discover test files owned by one project, excluding descendant workspace members in rooted workspaces.
fn discover_project_test_files(
    project_root: &Path,
    workspace: Option<&WorkspaceGraph>,
    session: &CompilationSession,
) -> Vec<PathBuf> {
    crate::cli::test_runner::discover_test_files_with_compilation_session(project_root, session)
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

    #[cfg(feature = "rust_inspect")]
    #[test]
    fn registry_source_ownership_unifies_feature_variants_with_the_same_source_archive() {
        let source = crate::oven::rustc::OvenRustcRegistrySource {
            registry: "registry+https://example.invalid/index".to_string(),
            checksum: "fixture-checksum".to_string(),
            relative_root: "registry-sources/syn".to_string(),
            digest: "sha256:fixture-source".to_string(),
        };
        let requested = crate::oven::rustc::OvenRustcRegistrySourcePackage {
            package: "syn".to_string(),
            version: "2.0.117".to_string(),
            features: vec!["derive".to_string()],
            source: source.clone(),
        };
        let owner = crate::oven::rustc::OvenRustcRegistrySourcePackage {
            features: vec!["clone-impls".to_string(), "full".to_string()],
            ..requested.clone()
        };

        assert!(registry_source_is_owned_by_catalog(&requested, &[owner]));
        let different_archive = crate::oven::rustc::OvenRustcRegistrySourcePackage {
            source: crate::oven::rustc::OvenRustcRegistrySource {
                checksum: "other-fixture-checksum".to_string(),
                ..source
            },
            ..requested.clone()
        };
        assert!(!registry_source_is_owned_by_catalog(&different_archive, &[requested]));
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

    #[cfg(feature = "rust_inspect")]
    #[test]
    fn registry_sources_refuse_a_loaf_without_its_sealed_lock() -> Result<(), Box<dyn std::error::Error>> {
        let temp_dir = tempfile::tempdir()?;
        let destination = temp_dir.path().join("Cargo.lock");

        let Err(error) = install_required_oven_registry_lock(true, temp_dir.path(), &destination) else {
            return Err(std::io::Error::other("missing sealed registry lock was accepted").into());
        };

        assert!(error.to_string().contains("declares registry sources"));
        assert!(!destination.exists());
        Ok(())
    }

    #[cfg(feature = "rust_inspect")]
    #[test]
    fn project_inspection_selection_mismatch_explains_the_transient_selection_boundary() {
        let diagnostic = project_inspection_selection_mismatch("the requested registry dependencies").to_string();

        assert!(diagnostic.contains("command-local `--sdk-profile` or package-feature selection"));
        assert!(diagnostic.contains("persist it in `[sdk]` in `loaf.toml` and rebake"));
        assert!(diagnostic.contains("with the same feature flags"));
    }

    #[cfg(feature = "rust_inspect")]
    #[test]
    fn custom_project_layout_cannot_fall_through_after_command_authority_mismatch()
    -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        fs::create_dir_all(project.path().join("library"))?;
        fs::create_dir_all(project.path().join("bin"))?;
        fs::write(
            project.path().join("loaf.toml"),
            "[project]\nname = \"custom-layout\"\n\n[project.scripts]\nworker = \"bin/worker.incn\"\n\n[build]\nsource-root = \"library\"\n",
        )?;
        fs::write(
            project.path().join("library/lib.incn"),
            "pub def value() -> int:\n    return 1\n",
        )?;
        fs::write(
            project.path().join("bin/worker.incn"),
            "def main() -> None:\n    pass\n",
        )?;

        assert!(!project.path().join("src/lib.incn").exists());
        assert!(!project.path().join("src/main.incn").exists());
        assert!(normal_inspection_requires_installed_project_authority(
            project.path(),
            false,
            true,
            false,
        ));
        assert!(!normal_inspection_requires_installed_project_authority(
            project.path(),
            false,
            true,
            true,
        ));
        assert!(!normal_inspection_requires_installed_project_authority(
            project.path(),
            true,
            true,
            false,
        ));

        let standalone = tempfile::tempdir()?;
        assert!(!normal_inspection_requires_installed_project_authority(
            standalone.path(),
            false,
            false,
            false,
        ));

        let conventional = tempfile::tempdir()?;
        fs::create_dir_all(conventional.path().join("src"))?;
        fs::write(
            conventional.path().join("src/main.incn"),
            "def main() -> None:\n    pass\n",
        )?;
        assert!(normal_inspection_requires_installed_project_authority(
            conventional.path(),
            false,
            false,
            false,
        ));
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

        let encoded: toml::Value = toml::from_str(&fs::read_to_string(temp_dir.path().join("oven.lock"))?)?;
        assert!(encoded.get("cargo").is_none());
        let loaded = IncanLock::load(&temp_dir.path().join("oven.lock"))?;
        assert_eq!(loaded.deps_fingerprint, lock.deps_fingerprint);
        assert_eq!(loaded.semantic, lock.semantic);
        assert!(temp_dir.path().join("oven.lock").is_file());
        let state_dir = crate::lockfile::compiler_lock_state_dir(temp_dir.path());
        assert!(
            !state_dir.join("Cargo.toml").exists() && !state_dir.join("target").exists(),
            "Oven lock generation may retain its publication guard but must not create generated-Cargo lock state"
        );
        Ok(())
    }

    #[test]
    fn explicit_oven_lock_publication_collects_once_and_retains_the_whole_dependency_surface()
    -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let project_root = project.path();
        fs::create_dir_all(project_root.join("src"))?;
        fs::create_dir_all(project_root.join("tests"))?;
        fs::write(
            project_root.join("loaf.toml"),
            r#"[project]
name = "single_lock_collection"
version = "0.1.0"

[project.scripts]
main = "src/main.incn"
worker = "src/worker.incn"

[rust-dependencies]
semver = "1"
serde_json = "1"

[rust-dev-dependencies]
regex = "1"
"#,
        )?;
        let main = project_root.join("src/main.incn");
        fs::write(
            &main,
            "from rust::serde_json import Value\n\ndef main() -> None:\n    pass\n",
        )?;
        fs::write(
            project_root.join("src/worker.incn"),
            "from rust::semver import Version\n\ndef worker() -> None:\n    pass\n",
        )?;
        fs::write(
            project_root.join("tests/test_match.incn"),
            "from rust::regex import Regex\nfrom std.testing import test\n\n@test\ndef test_match() -> None:\n    assert True\n",
        )?;

        reset_project_lock_collection_metrics();
        let publication = publish_oven_project_lock(project_root, &main, &FeatureSelection::default())?;
        let metrics = project_lock_collection_metrics();
        let normal_dependencies = publication
            .dependency_surface()
            .dependencies
            .iter()
            .map(|dependency| dependency.crate_name.as_str())
            .collect::<BTreeSet<_>>();
        let dev_dependencies = publication
            .dependency_surface()
            .dev_dependencies
            .iter()
            .map(|dependency| dependency.crate_name.as_str())
            .collect::<BTreeSet<_>>();

        assert_eq!(
            metrics,
            ProjectLockCollectionMetrics {
                context_collections: 1,
                session_discoveries: 1,
                authority_snapshot_reads: 1,
                provider_plan_projections: 1,
            },
            "one explicit publication must read one session authority and project one aggregate provider plan across every manifest entry",
        );
        assert!(normal_dependencies.is_superset(&BTreeSet::from(["semver", "serde_json"])));
        assert!(normal_dependencies.is_subset(&BTreeSet::from([
            "incan_stdlib_core",
            "incan_stdlib_testing",
            "semver",
            "serde_json",
        ])));
        assert_eq!(dev_dependencies, BTreeSet::from(["regex"]));
        let lock = IncanLock::load(&project_root.join("oven.lock"))?;
        assert!(!lock.deps_fingerprint.is_empty());
        let encoded: toml::Value = toml::from_str(&fs::read_to_string(project_root.join("oven.lock"))?)?;
        assert!(encoded.get("cargo").is_none());
        Ok(())
    }

    #[cfg(feature = "rust_inspect")]
    #[test]
    fn inspection_demand_requires_selection_before_output_mutation() -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let source = temp.path().join("main.incn");
        fs::write(&source, "def main() -> None:\n  pass\n")?;
        let poison = temp.path().join("Cargo.toml");
        fs::write(&poison, "invalid Cargo input; must not be read or repaired\n")?;
        let empty = prepare_rust_inspect_workspace(RustInspectWorkspaceRequest {
            project_root: temp.path(),
            rust_inspect_query_paths: &[],
            rust_derive_probe_paths: &[],
            selected: None,
        })?;
        assert!(empty.is_none());
        for (queries, derives) in [
            (vec!["regex::Regex".to_string()], Vec::new()),
            (Vec::new(), vec!["serde::Serialize".to_string()]),
        ] {
            let Err(error) = prepare_rust_inspect_workspace(RustInspectWorkspaceRequest {
                project_root: temp.path(),
                rust_inspect_query_paths: &queries,
                rust_derive_probe_paths: &derives,
                selected: None,
            }) else {
                return Err("required inspection without selection was accepted".into());
            };
            assert!(
                error
                    .to_string()
                    .contains("selected Rust inspection inputs are unavailable")
            );
            assert!(error.to_string().contains(temp.path().to_string_lossy().as_ref()));
        }
        assert_eq!(
            fs::read_to_string(&poison)?,
            "invalid Cargo input; must not be read or repaired\n"
        );
        assert!(!temp.path().join("Cargo.lock").exists());
        assert!(!temp.path().join("oven.lock").exists());
        assert!(!temp.path().join("target").exists());
        assert!(!crate::lockfile::compiler_lock_state_dir(temp.path()).exists());
        Ok(())
    }

    #[test]
    fn resolver_observes_lock_state_without_publishing_or_rediscovering_the_session()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp_dir = tempfile::tempdir()?;
        let project_root = temp_dir.path();
        let manifest_path = project_root.join("loaf.toml");
        let entry_path = project_root.join("src/main.incn");
        fs::create_dir_all(entry_path.parent().ok_or("entry path has no parent")?)?;
        fs::write(
            &manifest_path,
            "[project]\nname = \"semantic_lock_demo\"\nversion = \"0.1.0\"\n",
        )?;
        fs::write(&entry_path, "def main() -> None:\n  pass\n")?;
        let features = CargoFeatureSelection::default();
        let package_features = FeatureSelection::default();
        let session = CompilationSession::discover_for_oven(&entry_path, &package_features, None)?;
        let collect = |session: &CompilationSession| {
            let manifest = session
                .manifest
                .as_ref()
                .ok_or_else(|| CliError::failure("session has no manifest"))?;
            resolve_lock_context(LockResolutionRequest {
                project_root,
                entry_file: Some(&entry_path),
                manifest: Some(manifest),
                resolved: &empty_resolved(),
                project_requirements: &empty_project_requirements(),
                cargo_features: &features,
                semantic: None,
                package_features: Some(&package_features),
                sdk_profile_override: None,
                command_session: Some(session),
            })
        };
        reset_project_lock_collection_metrics();
        let missing = collect(&session)?.canonical.ok_or("canonical lock facts missing")?;
        assert_eq!(missing.status(), &SemanticLockStatus::Missing);
        assert!(validate_oven_existing_lock(&missing).is_err());
        assert!(!missing.lock_path().exists());
        assert!(!crate::lockfile::compiler_lock_state_dir(project_root).exists());
        assert_eq!(project_lock_collection_counts(), (1, 0));

        missing.expected().write(missing.lock_path())?;
        let bytes = fs::read(missing.lock_path())?;
        let modified = fs::metadata(missing.lock_path())?.modified()?;
        reset_project_lock_collection_metrics();
        let current = collect(&session)?.canonical.ok_or("canonical lock facts missing")?;
        assert_eq!(current.status(), &SemanticLockStatus::Current);
        validate_oven_existing_lock(&current)?;
        assert_eq!(fs::read(current.lock_path())?, bytes);
        assert_eq!(fs::metadata(current.lock_path())?.modified()?, modified);
        assert_eq!(project_lock_collection_counts(), (1, 0));

        fs::write(
            &entry_path,
            "from rust::regex @ \"1\" import Regex\n\ndef main() -> None:\n  pass\n",
        )?;
        let changed_session = CompilationSession::discover_for_oven(&entry_path, &package_features, None)?;
        let stale = collect(&changed_session)?
            .canonical
            .ok_or("canonical lock facts missing")?;
        assert_eq!(
            stale.status(),
            &SemanticLockStatus::Stale {
                actual_fingerprint: current.expected().deps_fingerprint.clone(),
            }
        );
        assert_ne!(stale.expected().deps_fingerprint, current.expected().deps_fingerprint);
        assert!(validate_oven_existing_lock(&stale).is_err());
        assert_eq!(fs::read(stale.lock_path())?, bytes);
        fs::write(stale.lock_path(), "malformed oven lock")?;
        assert!(collect(&session).is_err());
        assert_eq!(fs::read(stale.lock_path())?, b"malformed oven lock");
        Ok(())
    }

    #[test]
    fn lock_facts_keep_workspace_scope_separate_from_caller_emission_requirements()
    -> Result<(), Box<dyn std::error::Error>> {
        let workspace = tempfile::tempdir()?;
        fs::write(
            workspace.path().join("loaf.toml"),
            r#"[workspace]
members = ["first", "second"]
"#,
        )?;
        for (name, source) in [
            (
                "first",
                "from rust::regex @ \"1\" import Regex\n\ndef main() -> None:\n  pass\n",
            ),
            (
                "second",
                "from rust::semver @ \"1\" import Version\n\ndef main() -> None:\n  pass\n",
            ),
        ] {
            let root = workspace.path().join(name);
            fs::create_dir_all(root.join("src"))?;
            fs::write(
                root.join("loaf.toml"),
                format!(
                    "[project]\nname = \"{name}\"\nversion = \"0.1.0\"\n[project.scripts]\nmain = \"src/main.incn\"\n"
                ),
            )?;
            fs::write(root.join("src/main.incn"), source)?;
        }
        let entry = workspace.path().join("first/src/main.incn");
        let feature_selection = FeatureSelection::default();
        let session = CompilationSession::discover_for_oven(&entry, &feature_selection, None)?;
        let manifest = session.manifest.as_ref().ok_or("session has no manifest")?;
        let caller_dependencies = ResolvedDependencies {
            dependencies: vec![registry_dependency("regex")],
            dev_dependencies: Vec::new(),
        };
        let requirements = ProjectRequirements::default();
        let cargo_features = CargoFeatureSelection::default();
        let collect = |entry_file: &Path| {
            resolve_lock_context(LockResolutionRequest {
                project_root: manifest.project_root(),
                entry_file: Some(entry_file),
                manifest: Some(manifest),
                resolved: &caller_dependencies,
                project_requirements: &requirements,
                cargo_features: &cargo_features,
                semantic: None,
                package_features: Some(&feature_selection),
                sdk_profile_override: None,
                command_session: Some(&session),
            })
        };
        let resolution = collect(&entry)?;
        assert_eq!(resolution.resolved.dependencies, caller_dependencies.dependencies);
        assert!(
            !resolution
                .resolved
                .dependencies
                .iter()
                .any(|dependency| dependency.crate_name == "semver")
        );
        let canonical = resolution.canonical.ok_or("workspace lock facts missing")?;
        assert_eq!(
            canonical.lock_path(),
            workspace.path().canonicalize()?.join(LOCK_FILENAME)
        );
        assert_eq!(canonical.status(), &SemanticLockStatus::Missing);
        assert_eq!(canonical.expected().semantic.workspace_members.len(), 2);
        assert!(!workspace.path().join(LOCK_FILENAME).exists());
        assert!(!crate::lockfile::compiler_lock_state_dir(workspace.path()).exists());

        fs::write(
            workspace.path().join("second/src/main.incn"),
            "from rust::semver @ \"2\" import Version\n\ndef main() -> None:\n  pass\n",
        )?;
        let changed = collect(&entry)?;
        assert_eq!(changed.resolved.dependencies, caller_dependencies.dependencies);
        let changed_canonical = changed.canonical.ok_or("changed workspace lock facts missing")?;
        assert_ne!(
            changed_canonical.expected().deps_fingerprint,
            canonical.expected().deps_fingerprint,
            "another member's authored dependency use must remain in the canonical fingerprint",
        );
        assert!(!workspace.path().join(LOCK_FILENAME).exists());
        assert!(!crate::lockfile::compiler_lock_state_dir(workspace.path()).exists());

        let outside = tempfile::tempdir()?;
        let outside_entry = outside.path().join("main.incn");
        fs::write(&outside_entry, "def main() -> None:\n  pass\n")?;
        assert!(collect(&outside_entry).is_err());
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

        let session = CompilationSession::discover_for_oven(
            &project_root.join("tests/test_internal.incn"),
            &FeatureSelection::default(),
            None,
        )?;
        let inputs =
            collect_test_lock_inputs(project_root, None, None, None, None, &ProviderPlan::default(), &session)?;
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
            root.join("loaf.toml"),
            r#"
[project]
name = "root"

[workspace]
members = ["packages/consumer"]
"#,
        )?;
        fs::write(
            consumer_root.join("loaf.toml"),
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
        let root_session = CompilationSession::discover_for_oven(&root_test, &FeatureSelection::default(), None)?;
        let root_files = discover_project_test_files(workspace.root(), Some(&workspace), &root_session);
        let consumer = workspace
            .members()
            .find(|member| member.name() == "consumer")
            .ok_or("consumer workspace member should exist")?;
        let consumer_session =
            CompilationSession::discover_for_oven(&consumer_test, &FeatureSelection::default(), None)?;
        let consumer_files = discover_project_test_files(consumer.root(), Some(&workspace), &consumer_session);
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
