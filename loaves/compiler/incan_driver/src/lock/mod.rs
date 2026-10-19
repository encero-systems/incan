//! Resolving and publishing a project's lock: the request and result types every lock operation shares, and the
//! collection metrics the tests read.
//!
//! A lock is resolved once per command and consumed by build, run, test and the LSP alike; the submodules own the work
//! (`policy`, `resolution`, `workspace`, `rust_inspect`, `test_inputs`) and this module owns the shapes they
//! exchange, so a request built by one command is the same request another resolves.


pub mod policy;
pub mod resolution;
pub(crate) mod reuse;
#[cfg(feature = "rust_inspect")]
pub mod rust_inspect;
pub mod test_inputs;
pub mod workspace;

#[cfg(any(test, feature = "test_support"))]
use std::cell::Cell;
#[cfg(test)]
use std::fs;
use std::path::Path;
use std::path::PathBuf;
#[cfg(feature = "rust_inspect")]
use std::sync::Arc;


use crate::error::{CliError, CliResult};
use policy::LockPolicy;

use incan_frontend::ParsedModule;
use crate::features::FeatureSelection;
use incan_provider::dependency_resolver::{InlineRustImport, ResolvedDependencies};
use incan_provider::requirements::ProjectRequirements;
use oven_model::lock::{CargoFeatureSelection, SemanticLockState};
#[cfg(feature = "rust_inspect")]
use oven_model::manifest::DependencySpec;
use oven_model::manifest::ProjectManifest;
use oven_model::workspace::WorkspaceGraph;

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
    pub lock_policy: &'a LockPolicy,
    pub semantic: Option<&'a SemanticLockState>,
    /// Incan package-feature selection used when rebuilding the canonical project-wide lock context.
    pub package_features: Option<&'a FeatureSelection>,
    /// Command-local SDK profile used when rebuilding the canonical project-wide lock context.
    pub sdk_profile_override: Option<&'a str>,
}

/// The canonical dependency context one lock resolution selected for a project.
pub struct LockResolution {
    pub resolved: ResolvedDependencies,
    pub project_requirements: ProjectRequirements,
}

/// Inputs for the inspection root a metadata-backed typecheck needs.
#[cfg(feature = "rust_inspect")]
pub struct RustInspectTypecheckRequest<'a> {
    pub project_root: &'a Path,
    pub project_name: &'a str,
    pub manifest: Option<&'a ProjectManifest>,
    pub modules: &'a [ParsedModule],
    pub cargo_features: &'a CargoFeatureSelection,
    pub rust_edition: Option<String>,
}

/// Typecheck projection retaining its complete original source, toolchain and native owners until the caller
/// finishes semantic analysis. A path alone cannot preserve those capabilities.
#[cfg(feature = "rust_inspect")]
pub struct PreparedRustInspectTypecheckWorkspace {
    workspace: PreparedRustInspectWorkspace,
}

#[cfg(feature = "rust_inspect")]
impl PreparedRustInspectTypecheckWorkspace {
    /// Return the projection while retaining every original source, compiler and native owner.
    pub fn manifest_dir(&self) -> &Path {
        self.workspace.manifest_dir()
    }

    /// Revalidate original native inputs before and after semantic consumption of this projection.
    pub fn verify_ordinary_native(&self) -> CliResult<()> {
        self.workspace.verify_ordinary_native()
    }
}

/// One direct inspection root request: the query surface and the native selection that answers it.
#[cfg(feature = "rust_inspect")]
pub struct RustInspectWorkspaceRequest<'a> {
    pub project_root: &'a Path,
    pub project_name: &'a str,
    pub rust_edition: Option<String>,
    /// The inspector's cache directory for this projection.
    pub cache_dir: &'a Path,
    pub rust_inspect_query_paths: &'a [String],
    /// Exact external Rust derive macros used by concrete Incan declarations.
    pub rust_derive_probe_paths: &'a [String],
    pub prepare_when_empty: bool,
    /// A publisher must materialize provider-source metadata even when ordinary lazy prewarm is off.
    pub force_direct_prewarm: bool,
    /// The compiler intent and declared Rust roots that select the inspected native closure.
    pub authority: OvenRustInspectSourceAuthorityRequest<'a>,
}

/// The compiler intent and declared Rust roots one inspection selects from the original native producer.
#[cfg(feature = "rust_inspect")]
pub struct OvenRustInspectSourceAuthorityRequest<'a> {
    pub target: &'a str,
    pub toolchain: &'a str,
    pub profile: &'a str,
    pub features: &'a [String],
    pub registry_dependencies: &'a [DependencySpec],
}

/// Prepared projection retaining its original ordinary producer request and compiler-bound toolchain through
/// semantic analysis.
#[cfg(feature = "rust_inspect")]
pub struct PreparedRustInspectWorkspace {
    manifest_dir: PathBuf,
    /// Complete original ordinary request retained across checked metadata preparation and final ABI extraction.
    _ordinary_native: oven_rustc::native_loaf::NativeLoafRequestObservation,
    /// Original compiler-bound Rust source owner retained through final semantic metadata extraction.
    _ordinary_toolchain: Arc<oven_rustc::rustc::OvenRustInspectionToolchain>,
}

#[cfg(feature = "rust_inspect")]
impl PreparedRustInspectWorkspace {
    /// Return the compiler-authored manifest directory while this workspace retains its owners.
    pub fn manifest_dir(&self) -> &Path {
        &self.manifest_dir
    }

    /// Revalidate complete original ordinary inputs at semantic handoffs without selecting new source owners.
    pub fn verify_ordinary_native(&self) -> CliResult<()> {
        self._ordinary_native
            .verify()
            .map_err(|error| CliError::failure(error.to_string()))?;
        self._ordinary_toolchain
            .verify()
            .map_err(|error| CliError::failure(error.to_string()))
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
    pub lock_policy: &'a LockPolicy,
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
    lock_policy: &'a LockPolicy,
    sdk_profile_override: Option<&'a str>,
}

/// Fully collected dependency inputs that define a project or workspace lock's freshness surface.
pub struct ProjectLockContext {
    /// Canonical test discovery observations for every contributing project, including all workspace members.
    #[cfg(feature = "cli")]
    pub(crate) test_discovery: Vec<oven_model::observed_inputs::InputObservations>,
    resolved: ResolvedDependencies,
    project_requirements: ProjectRequirements,
    semantic: SemanticLockState,
}

/// Canonical lock publication retained by one explicit project bake.
///
/// The dependency surface is the exact normal and test closure owned by the project being baked. A workspace lock
/// still contains every member, but a member's inspection and test plans must never adopt sibling dependencies.
pub struct PublishedOvenProjectLock {
    dependency_surface: ResolvedDependencies,
    publication: PublishedLockFile,
}

/// Exact bytes and canonical location observed by the lock writer while holding its publication guard.
pub(crate) struct PublishedLockFile {
    pub(crate) canonical_path: PathBuf,
    pub(crate) content_digest: String,
}

impl PublishedOvenProjectLock {
    /// Return the exact normal and test dependency surface used to publish the canonical lock.
    pub fn dependency_surface(&self) -> &ResolvedDependencies {
        &self.dependency_surface
    }

    /// Return the canonical file actually written, including the workspace root when applicable.
    pub(crate) fn canonical_lock_path(&self) -> &Path {
        &self.publication.canonical_path
    }

    /// Return the digest captured by the writer before releasing the publication guard.
    pub(crate) fn published_content_digest(&self) -> &str {
        &self.publication.content_digest
    }

    /// Refuse replacement, removal or changed bytes before accepting a producer-owned lock transition.
    pub(crate) fn verify_published_file(&self) -> CliResult<()> {
        let path = self.canonical_lock_path();
        let metadata = std::fs::symlink_metadata(path).map_err(|error| CliError::failure(error.to_string()))?;
        if !metadata.file_type().is_file() {
            return Err(CliError::failure(
                "published canonical lock is no longer a regular file",
            ));
        }
        let bytes = std::fs::read(path).map_err(|error| CliError::failure(error.to_string()))?;
        if oven_store::digest_bytes(&bytes) != self.published_content_digest() {
            return Err(CliError::failure(
                "published canonical lock changed before metadata finalization",
            ));
        }
        Ok(())
    }
}

/// Outcome of publishing the canonical lock on behalf of one explicit provider bake.
pub enum ProviderBakeLockPublication {
    /// The whole-graph lock was collected and written.
    Published {
        /// Dependency surface owned by the project that requested the bake.
        project_dependency_surface: ResolvedDependencies,
    },
    /// At least one sibling member could not resolve yet. The root lock was written with every member that could,
    /// and the whole-graph fingerprint stays stale until the remaining members resolve.
    Deferred {
        member: String,
        /// Dependency surface owned by the project that requested the bake.
        project_dependency_surface: ResolvedDependencies,
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
    /// Dependency surface of the member that requested this collection, before workspace union.
    project_dependency_surface: ResolvedDependencies,
    /// Members skipped because they could not resolve yet, with the reason each gave. Empty for a strict collection.
    unresolved: Vec<(String, CliError)>,
}

/// Test-file dependency inputs that must participate in the same project lock fingerprint as normal scripts.
pub struct TestLockInputs {
    /// Consulted source files, directory membership and nested manifest boundaries from canonical discovery.
    #[cfg(feature = "cli")]
    discovery_inputs: oven_model::observed_inputs::InputObservations,
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

    /// One lock collection retains imports from every script and test in the ordinary dependency surface.
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

[dependencies]
semver = { loaf = "crates-io/semver", version = "1" }
serde_json = { loaf = "crates-io/serde_json", version = "1" }
regex = { loaf = "crates-io/regex", version = "1" }
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
            normal_dependencies.is_superset(&BTreeSet::from(["regex", "semver", "serde_json"])),
            "normal dependencies: {normal_dependencies:?}"
        );
        // `std.testing` brings its component and, since facets became demand-driven, the Rust facet it names.
        assert!(
            normal_dependencies.is_subset(&BTreeSet::from([
                "incan_std_testing",
                "incan_stdlib_core",
                "incan_stdlib_testing",
                "regex",
                "semver",
                "serde_json",
            ])),
            "normal dependencies: {normal_dependencies:?}"
        );
        assert!(
            dev_dependencies.is_empty(),
            "ordinary test dependencies: {dev_dependencies:?}"
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
