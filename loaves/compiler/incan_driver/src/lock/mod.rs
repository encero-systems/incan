//! Resolving and publishing a project's lock: the request and result types every lock operation shares, and the
//! collection metrics the tests read.
//!
//! A lock is resolved once per command and consumed by build, run, test and the LSP alike; the submodules own the
//! work (`resolution`, `workspace`, `registry_sources`, `rust_inspect`, `test_inputs`) and this module owns
//! the shapes they exchange, so a request built by one command is the same request another resolves.

#[cfg(feature = "rust_inspect")]
pub mod registry_sources;
pub mod resolution;
#[cfg(feature = "rust_inspect")]
pub mod rust_inspect;
pub mod test_inputs;
pub mod workspace;

#[cfg(any(test, feature = "test_support"))]
use std::cell::Cell;
#[cfg(feature = "rust_inspect")]
use std::collections::BTreeMap;
#[cfg(test)]
use std::fs;
use std::path::Path;
#[cfg(any(feature = "rust_inspect", test))]
use std::path::PathBuf;
#[cfg(feature = "rust_inspect")]
use std::sync::Arc;

use crate::cargo_policy::CargoPolicy;
use crate::error::CliError;
#[cfg(feature = "rust_inspect")]
use crate::generated_cache::GeneratedCacheLease;
use incan_frontend::ParsedModule;
#[cfg(feature = "rust_inspect")]
use incan_frontend::library_manifest_index::LibraryManifestIndex;
use incan_provider::FeatureSelection;
#[cfg(feature = "rust_inspect")]
use incan_provider::ProviderPlan;
use incan_provider::dependency_resolver::{InlineRustImport, ResolvedDependencies};
use incan_provider::requirements::ProjectRequirements;
use oven_model::lock::{CargoFeatureSelection, SemanticLockState};
#[cfg(feature = "rust_inspect")]
use oven_model::manifest::DependencySpec;
use oven_model::manifest::ProjectManifest;
use oven_model::workspace::WorkspaceGraph;
#[cfg(feature = "rust_inspect")]
use oven_rustc::loaf::OvenToolchainLoaf;
#[cfg(feature = "rust_inspect")]
use oven_rustc::rustc::OvenLoadedProjectInspectionAuthority;

#[cfg(any(test, feature = "test_support"))]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct ProjectLockCollectionMetrics {
    context_collections: usize,
    session_discoveries: usize,
    authority_snapshot_reads: usize,
    provider_plan_projections: usize,
}

#[cfg(any(test, feature = "test_support"))]
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
#[cfg(any(test, feature = "test_support"))]
pub fn reset_project_lock_collection_metrics() {
    PROJECT_LOCK_COLLECTION_METRICS.set(ProjectLockCollectionMetrics::default());
}

/// Return the current project-lock collection metrics for this test thread.
#[cfg(any(test, feature = "test_support"))]
fn project_lock_collection_metrics() -> ProjectLockCollectionMetrics {
    PROJECT_LOCK_COLLECTION_METRICS.get()
}

/// Apply one update to the project-lock collection metrics for this test thread.
#[cfg(any(test, feature = "test_support"))]
fn update_project_lock_collection_metrics(update: impl FnOnce(&mut ProjectLockCollectionMetrics)) {
    let mut metrics = PROJECT_LOCK_COLLECTION_METRICS.get();
    update(&mut metrics);
    PROJECT_LOCK_COLLECTION_METRICS.set(metrics);
}

/// Return the context-collection and session-discovery counts for this test thread.
#[cfg(any(test, feature = "test_support"))]
pub fn project_lock_collection_counts() -> (usize, usize) {
    let metrics = project_lock_collection_metrics();
    (metrics.context_collections, metrics.session_discoveries)
}

/// Record one project-lock context collection for this test thread.
#[cfg(any(test, feature = "test_support"))]
pub fn record_project_lock_context_collection() {
    update_project_lock_collection_metrics(|metrics| {
        metrics.context_collections += 1;
    });
}

/// Record one project-lock compilation-session discovery for this test thread.
#[cfg(any(test, feature = "test_support"))]
pub fn record_project_lock_session_discovery() {
    update_project_lock_collection_metrics(|metrics| {
        metrics.session_discoveries += 1;
    });
}

/// Record one project-lock authority snapshot read for this test thread.
#[cfg(any(test, feature = "test_support"))]
pub fn record_project_lock_authority_snapshot_read() {
    update_project_lock_collection_metrics(|metrics| {
        metrics.authority_snapshot_reads += 1;
    });
}

/// Record one project-lock provider-plan projection for this test thread.
#[cfg(any(test, feature = "test_support"))]
pub fn record_project_lock_provider_plan_projection() {
    update_project_lock_collection_metrics(|metrics| {
        metrics.provider_plan_projections += 1;
    });
}

/// Resolve the canonical dependency context and lock payload for a project build.
///
/// Manifest-less standalone builds retain their caller-local dependency context and have no lock payload.
/// Manifest-backed builds keep caller-local resolved dependencies and requirements for generated Cargo manifests,
/// while project- or workspace-wide aggregation owns canonical lock generation. A fresh canonical payload authorizes
/// Cargo-owned projection onto the caller manifest only after package coordinates, checksums, and edges validate.
pub struct LockResolutionRequest<'a> {
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
pub struct LockResolution {
    pub cargo_lock_authority: CargoLockAuthority,
    pub cargo_package_name: String,
    pub resolved: ResolvedDependencies,
    pub project_requirements: ProjectRequirements,
}

/// Closed authority state for generated Cargo locks.
pub enum CargoLockAuthority {
    /// No trusted Cargo lock payload is available and no stale projection cleanup is required.
    None,
    /// A tolerated stale canonical lock must not authorize or leave behind any older generated projection.
    Stale,
    /// An internal artifact build consumes an exact payload directly without caller projection.
    Exact { payload: String },
}

/// Final generator-facing inputs derived from one closed lock authority state.
pub struct CargoLockGeneratorInputs {
    pub payload: Option<String>,
    pub projection_root: Option<String>,
    pub clear_existing: bool,
}

impl CargoLockAuthority {
    /// Split the closed authority state into generator inputs at the final rendering boundary.
    pub fn into_generator_inputs(self) -> CargoLockGeneratorInputs {
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

#[cfg(feature = "rust_inspect")]
pub struct RustInspectTypecheckRequest<'a> {
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
pub struct PreparedRustInspectTypecheckWorkspace {
    manifest_dir: PathBuf,
    _cache_lease: Option<GeneratedCacheLease>,
    _source_loaf: Option<OvenToolchainLoaf>,
}

#[cfg(feature = "rust_inspect")]
impl PreparedRustInspectTypecheckWorkspace {
    /// Return the generated Cargo workspace used for rust-inspect typechecking.
    pub fn manifest_dir(&self) -> &Path {
        &self.manifest_dir
    }
}

#[cfg(feature = "rust_inspect")]
pub struct RustInspectWorkspaceRequest<'a> {
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
    /// Exact external Rust derive macros used by concrete Incan declarations.
    pub rust_derive_probe_paths: &'a [String],
    pub prepare_when_empty: bool,
    /// Select rust-analyzer's direct source graph before any inspection action can run Cargo.
    pub direct_oven_inspection: bool,
    /// The named Loaf publisher must materialize provider-source metadata even when ordinary lazy prewarm is
    /// off.
    pub force_direct_prewarm: bool,
    /// Receipt inputs used to select the exact immutable Loaf that owns registry inspection sources.
    pub oven_source_authority: Option<OvenRustInspectSourceAuthorityRequest<'a>>,
    /// Command-local source authority selected once from source-current completed project outputs.
    pub prepared_project_source_authorities: Option<Arc<PreparedOvenProjectRegistrySourceAuthorities>>,
    /// Permit one locked source-authority acquisition only at `incan oven bake`'s explicit publisher boundary.
    pub explicit_oven_bake: bool,
}

/// Receipt-compatible Loaf inputs required before a normal direct Oven metadata prewarm.
#[cfg(feature = "rust_inspect")]
pub struct OvenRustInspectSourceAuthorityRequest<'a> {
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
pub struct PreparedRustInspectWorkspace {
    manifest_dir: PathBuf,
    _source_loaf: Option<OvenToolchainLoaf>,
    _project_source_authorities: Option<Arc<PreparedOvenProjectRegistrySourceAuthorities>>,
}

/// Command-local source authority shared by every parallel native-test unit.
///
/// The completed-output, authority, constituent, and compiler-release leases stay live for this value's lifetime. A
/// batch performs only an in-memory exact-root check and projects the one already validated source catalog and lock.
#[cfg(feature = "rust_inspect")]
pub struct PreparedOvenProjectRegistrySourceAuthorities {
    authority: OvenLoadedProjectInspectionAuthority,
    sources: Vec<::rust_inspect::OvenInspectionRegistrySource>,
    registry_lock_source: Option<PathBuf>,
    /// Build-script output directories the explicit bake sealed below the authority root, with their package
    /// versions where the bake recorded them.
    generated_out_dirs: Vec<::rust_inspect::SealedGeneratedOutDir>,
    test_dependency_plan: Option<oven_rustc::plan::OvenDirectRustcPlanSelection>,
    _release_loafs: Vec<OvenToolchainLoaf>,
}

#[cfg(feature = "rust_inspect")]
impl PreparedRustInspectWorkspace {
    /// Return the compiler-authored manifest directory while this workspace retains its source Loaf.
    pub fn manifest_dir(&self) -> &Path {
        &self.manifest_dir
    }
}

/// Validate an existing canonical Incan lock for an Oven consumer without publishing any missing SDK/provider or
/// dependency artifacts.
///
/// This derives the same manifest/source/semantic fingerprint from the active SDK inventory and parser-visible
/// dependency metadata without launching Cargo to make missing state appear.
pub struct OvenLockValidationRequest<'a> {
    pub project_root: &'a Path,
    pub manifest: Option<&'a ProjectManifest>,
    pub entry_file: &'a Path,
    pub cargo_features: &'a CargoFeatureSelection,
    pub cargo_policy: &'a CargoPolicy,
    pub package_features: &'a FeatureSelection,
    pub sdk_profile_override: Option<&'a str>,
}

/// Resolve or generate the canonical root lock for a workspace-aware compiler invocation.
///
/// The member that triggered this call is deliberately absent from the path calculation: a workspace lock is built
/// from every member context and lives only at the workspace root.
pub struct WorkspaceLockResolutionRequest<'a> {
    workspace: &'a WorkspaceGraph,
    caller_project_name: &'a str,
    caller_root: &'a Path,
    caller_resolved: &'a ResolvedDependencies,
    caller_project_requirements: &'a ProjectRequirements,
    caller_entry_file: Option<&'a Path>,
    cargo_features: &'a CargoFeatureSelection,
    cargo_policy: &'a CargoPolicy,
    sdk_profile_override: Option<&'a str>,
}

/// Fully collected dependency inputs that define a project or workspace lock's freshness surface.
pub struct ProjectLockContext {
    resolved: ResolvedDependencies,
    project_requirements: ProjectRequirements,
    semantic: SemanticLockState,
}

/// Canonical lock publication retained by one explicit project bake.
///
/// The dependency surface is the exact normal and test closure used to publish `oven.lock`. Keeping it behind this
/// immutable projection prevents source-authority publication from rediscovering the same project graph.
pub struct PublishedOvenProjectLock {
    dependency_surface: ResolvedDependencies,
}

impl PublishedOvenProjectLock {
    /// Return the exact normal and test dependency surface used to publish the canonical lock.
    pub fn dependency_surface(&self) -> &ResolvedDependencies {
        &self.dependency_surface
    }
}

/// Outcome of publishing the canonical lock on behalf of one explicit provider bake.
pub enum ProviderBakeLockPublication {
    /// The whole-graph lock was collected and written.
    Published(ProjectLockContext),
    /// At least one sibling member could not resolve yet. The root lock was written with every member that could,
    /// and the whole-graph fingerprint stays stale until the remaining members resolve.
    Deferred {
        member: String,
        context: ProjectLockContext,
        reason: String,
    },
}

/// Why collecting the whole-workspace lock stopped.
///
/// The tolerant collection records unresolved siblings on its success path instead, so the failure carries only the
/// error that ended the collection.
pub struct WorkspaceLockMemberFailure {
    error: CliError,
}

/// A workspace lock collection together with the members it had to leave out.
pub struct WorkspaceLockCollection {
    context: ProjectLockContext,
    /// Members skipped because they could not resolve yet, with the reason each gave. Empty for a strict collection.
    unresolved: Vec<(String, CliError)>,
}

/// Test-file dependency inputs that must participate in the same project lock fingerprint as normal scripts.
pub struct TestLockInputs {
    inline_imports: Vec<InlineRustImport>,
    project_requirement_modules: Vec<ParsedModule>,
}

/// Cargo projection text held in an Oven-native lock.
///
/// `oven.lock` retains this structurally valid inert payload for compatibility with the existing lockfile format,
/// but normal command execution neither materializes nor consumes it. The explicit `legacy_cargo` publisher owns
/// the historical exact Cargo projection below.
pub const INERT_CARGO_LOCK_PAYLOAD: &str = "version = 4\n";

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    use crate::lock::resolution::publish_oven_project_lock;
    use oven_model::lock::IncanLock;

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
        assert!(
            normal_dependencies.is_superset(&BTreeSet::from(["semver", "serde_json"])),
            "normal dependencies: {normal_dependencies:?}"
        );
        // `std.testing` brings its component and, since facets became demand-driven, the Rust facet it names.
        assert!(
            normal_dependencies.is_subset(&BTreeSet::from([
                "incan_std_testing",
                "incan_stdlib_core",
                "incan_stdlib_testing",
                "semver",
                "serde_json",
            ])),
            "normal dependencies: {normal_dependencies:?}"
        );
        assert_eq!(
            dev_dependencies,
            BTreeSet::from(["regex"]),
            "dev dependencies: {dev_dependencies:?}"
        );
        let lock = IncanLock::load(&project_root.join("oven.lock"))?;
        assert!(!lock.deps_fingerprint.is_empty());
        assert_eq!(lock.cargo_lock_payload, INERT_CARGO_LOCK_PAYLOAD);
        Ok(())
    }
}

/// Fixtures the lock tests build requests from.
#[cfg(test)]
pub mod test_support {
    use incan_provider::dependency_resolver::ResolvedDependencies;
    use incan_provider::requirements::ProjectRequirements;
    use oven_model::manifest::DependencySource;
    use oven_model::manifest::DependencySpec;

    /// A resolution with no dependencies at all.
    pub fn empty_resolved() -> ResolvedDependencies {
        ResolvedDependencies {
            dependencies: Vec::new(),
            dev_dependencies: Vec::new(),
        }
    }

    /// Project requirements with nothing selected.
    pub fn empty_project_requirements() -> ProjectRequirements {
        ProjectRequirements {
            stdlib_facets: Vec::new(),
            dependencies: Vec::new(),
            sdk_dependency_rebindings: Vec::new(),
            sdk_path_dependencies: Vec::new(),
            sdk_artifact_projections: Vec::new(),
        }
    }

    /// A registry dependency spec for `name` at `version`.
    pub fn registry_dependency(crate_name: &str) -> DependencySpec {
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
}
