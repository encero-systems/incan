//! Oven's store and what it stores: receipts, the build-unit identities they seal, the bounded content-addressed
//! store those identities key, its read-only mirrors, closure proofs, publication progress and the process facts a
//! lease records.
//!
//! Nothing here names a compiler crate. What Oven has to know about the compiler that drives it arrives as data:
//! its [`oven_model::compiler_identity::CompilerIdentity`] and the provider facts behind [`OvenProviderHooks`].
//!
//! Oven reads frozen Cargo declarations only as compatibility evidence. Receipt and consumer paths do not invoke
//! Cargo, inspect a target directory, or claim to have performed native package resolution. The explicitly named
//! `legacy_cargo` baker in `oven_rustc` is the sole Alpha bootstrap boundary that may invoke Cargo. Later Oven
//! store and executor stages consume portable identities and sealed Loafs rather than project-local build paths.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{self, ErrorKind, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use oven_model::digest::digest_cargo_path_source_tree_with_cache;
use oven_model::manifest::{DependencySource, DependencySpec, GitReference, ProjectManifest};

pub mod closure_proof;
pub use oven_model::compiler_suite_env;
pub mod process;
pub mod progress;
pub mod store;
pub mod store_mirror;
#[cfg(any(test, feature = "test_support"))]
pub mod test_support;

/// Digest the portable dependency facts that select a native Oven closure.
///
/// Registry and Git specifications are represented by their declared immutable selection facts. Path dependencies add
/// only a source-tree digest, never the machine-local path. This keeps compatible clean worktrees reusable while a
/// changed local runtime or declared dependency source necessarily selects a different build unit.
pub fn digest_dependency_specs(
    dependencies: &[DependencySpec],
    provider_hooks: &dyn OvenProviderHooks,
) -> Result<String, OvenError> {
    let mut records = Vec::with_capacity(dependencies.len());
    let mut resolved_path_packages = BTreeMap::new();
    for dependency in dependencies {
        let mut features = dependency.features.clone();
        features.sort();
        features.dedup();
        let source = match &dependency.source {
            DependencySource::Registry => "registry".to_string(),
            DependencySource::Git { url, reference } => match reference {
                GitReference::Branch(branch) => format!("git:{url}:branch:{branch}"),
                GitReference::Tag(tag) => format!("git:{url}:tag:{tag}"),
                GitReference::Rev(revision) => format!("git:{url}:rev:{revision}"),
            },
            // A packaged Incan provider is identified by its sealed artifact tree, never by walking the Cargo
            // edges its generated manifest still spells out: those point at the producer's private Rust sources,
            // which an admitted package does not need and a source-free consumer does not have (#1469).
            //
            // Any other path dependency is selected by its recursive Cargo-semantic source closure, not by compiler
            // output or unrelated repository files. Sharing the package memo also avoids rescanning a common sibling
            // reached through several top-level dependencies.
            DependencySource::Path { path } => match provider_hooks.packaged_provider_digest(path) {
                Some(digest) => {
                    let digest = digest.map_err(|source| OvenError::ProviderHook {
                        path: path.clone(),
                        source,
                    })?;
                    format!("packaged-provider:{digest}")
                }
                None => {
                    let digest = digest_cargo_path_source_tree_with_cache(path, &mut resolved_path_packages).map_err(
                        |error| OvenError::InvalidProjectSource {
                            path: path.clone(),
                            message: error.to_string(),
                        },
                    )?;
                    format!("path-tree:{digest}")
                }
            },
        };
        records.push(format!(
            "{}|{}|{}|{}|{}|{}|{}",
            dependency.crate_name,
            dependency.package.as_deref().unwrap_or(""),
            dependency.version.as_deref().unwrap_or(""),
            dependency.default_features,
            dependency.optional,
            features.join(","),
            source,
        ));
    }
    records.sort();
    Ok(digest_bytes(records.join("\n").as_bytes()))
}

/// The provider facts Oven asks the compiler for instead of reading them itself.
///
/// The compat publisher stages the compiler's SDK provider tree and rewrites the staged providers' dependency
/// digests; a dependency digest has to recognise a packaged Incan provider by its sealed artifact. Both are facts
/// about Incan packages, so the compiler implements this and hands it in with every request that needs it; the Oven
/// ring names no compiler crate.
pub trait OvenProviderHooks: Send + Sync {
    /// The root of the SDK provider tree to stage: the one an explicit inventory path describes, or the active
    /// toolchain's when there is none. An error is a preparation miss the caller reports verbatim.
    fn sdk_provider_root(&self, explicit_inventory: Option<&Path>) -> Result<PathBuf, OvenProviderHookError>;

    /// The inventory file's name inside a provider root.
    fn sdk_inventory_file(&self) -> &'static str;

    /// Rewrite the dependency digests of the providers copied into `provider_root` so the staged tree is
    /// self-consistent after its compiler-owned path dependencies were rebased.
    fn refresh_staged_sdk_provider_digests(&self, provider_root: &Path) -> Result<(), OvenProviderHookError>;

    /// The sealed artifact digest of a packaged provider at `dependency_root`, or `None` when the path is an
    /// authored crate and the caller should digest its source tree instead.
    fn packaged_provider_digest(&self, dependency_root: &Path) -> Option<Result<String, OvenProviderHookError>>;
}

/// The compiler's own error type behind a hook failure, kept whole so a caller can still reach it through
/// [`std::error::Error::source`] without Oven naming the compiler crate that defines it.
pub type OvenProviderHookSource = Box<dyn std::error::Error + Send + Sync + 'static>;

/// Why a provider hook could not answer.
///
/// Each variant is the category Oven acts on — a missing inventory is a preparation miss, a digest that cannot be
/// refreshed or read means the staged tree is not self-consistent — and carries the compiler's typed error as its
/// source rather than a rendering of it, so nothing about the failure is lost at the ring boundary.
#[derive(Debug, thiserror::Error)]
pub enum OvenProviderHookError {
    /// The explicitly named SDK provider inventory could not be read.
    #[error("failed to load explicit SDK provider inventory {path}: {source}")]
    InventoryUnreadable {
        /// The inventory file that was named.
        path: PathBuf,
        /// The compiler's error while reading it.
        #[source]
        source: OvenProviderHookSource,
    },
    /// Discovering the active toolchain's SDK provider inventory failed.
    #[error("failed to discover active SDK provider inventory: {source}")]
    InventoryDiscovery {
        /// The compiler's error while looking for it.
        #[source]
        source: OvenProviderHookSource,
    },
    /// No SDK provider inventory is available to stage from; `guidance` says what would make one available.
    #[error("{guidance}")]
    InventoryUnavailable {
        /// What the caller can do about it.
        guidance: String,
    },
    /// The digests of the providers staged below `provider_root` could not be refreshed.
    #[error("failed to refresh staged SDK provider digests below {provider_root}: {source}")]
    DigestRefresh {
        /// The staged provider tree.
        provider_root: PathBuf,
        /// The compiler's error while rewriting them.
        #[source]
        source: OvenProviderHookSource,
    },
    /// The sealed digest of the packaged provider at `dependency_root` could not be read.
    #[error("failed to digest packaged provider at {dependency_root}: {source}")]
    PackagedDigest {
        /// The packaged provider's root.
        dependency_root: PathBuf,
        /// The compiler's error while digesting it.
        #[source]
        source: OvenProviderHookSource,
    },
}

/// Hooks for a caller that has no compiler providers to speak of: no SDK to stage, nothing packaged, no refresh.
///
/// Oven's own tests run under these; a real compiler hands in its own implementation.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoProviderHooks;

impl OvenProviderHooks for NoProviderHooks {
    fn sdk_provider_root(&self, explicit_inventory: Option<&Path>) -> Result<PathBuf, OvenProviderHookError> {
        explicit_inventory
            .and_then(Path::parent)
            .map(Path::to_path_buf)
            .ok_or_else(|| OvenProviderHookError::InventoryUnavailable {
                guidance: "no SDK provider inventory is available without a compiler".to_string(),
            })
    }

    fn sdk_inventory_file(&self) -> &'static str {
        "sdk-inventory.json"
    }

    fn refresh_staged_sdk_provider_digests(&self, _provider_root: &Path) -> Result<(), OvenProviderHookError> {
        Ok(())
    }

    fn packaged_provider_digest(&self, _dependency_root: &Path) -> Option<Result<String, OvenProviderHookError>> {
        None
    }
}

/// Current wire format for persisted Oven receipts.
pub const OVEN_RECEIPT_SCHEMA_VERSION: u32 = 3;

/// Build-unit input key that binds one compiler-release root-intent authority to its final receipt.
pub const OVEN_COMPILER_SUPPORT_ROOT_INTENT_BUILD_UNIT_INPUT: &str = "compiler-support-root-intent";
/// Compiler-owned, project-relative destination for a default Oven receipt.
pub const DEFAULT_RECEIPT_RELATIVE_PATH: &str = ".incan/oven/receipt.json";

/// Default aggregate physical allocation retained by an everyday Alpha Oven store.
///
/// A project bake retains independent debug and release plans. A measured IncQL/DataFusion provider retains about
/// 4.23 GiB while its consumer's compatibility publisher transiently needs about 3.80 GiB, and a Bevy-scale
/// debug-plus-release pair retains about 3 GiB. Twelve GiB lets two such projects share one home and still leaves
/// the publisher's staging floor free, so switching between them reuses rather than re-bakes (#1230); the
/// publisher's private target stays bounded by that floor and the store prunes to this cap, so it is a ceiling on
/// what is kept, never a reservation.
pub const DEFAULT_OVEN_MAX_PHYSICAL_BYTES: u64 = 12 * 1024 * 1024 * 1024;
/// Default physical allocation cap for one compatibility domain.
///
/// Every project baked by one Incan release shares one compatibility domain, so for the ordinary single-release home
/// this cap is the aggregate cap under another name; it equals the aggregate so it cannot starve a second project of
/// staging before the aggregate would. A superseded release's entries are reclaimed at reservation time, which is
/// what keeps an upgraded home from hoarding; callers may still choose a stricter explicit limit.
pub const DEFAULT_OVEN_MAX_DOMAIN_PHYSICAL_BYTES: u64 = 12 * 1024 * 1024 * 1024;
/// Transient staging the explicit compatibility baker reserves before it runs, reclaiming inactive store entries
/// oldest-first to reach it.
///
/// The consumer of a measured IncQL/DataFusion provider stages about 3.80 GiB and a bare-Bevy bake about 3.2 GB
/// before publication; four GiB covers both with headroom. A bake that needs less simply leaves the rest unused, and
/// one that needs more is still bounded by the same monitor as before, so this floor only decides how much of another
/// project's inactive closure may be evicted to let this one finish.
pub const DEFAULT_OVEN_PUBLISHER_STAGING_FLOOR_BYTES: u64 = 4 * 1024 * 1024 * 1024;
/// Default logical artifact-byte cap for one compatibility domain.
///
/// One explicit bake of a project whose closure is not loadable as independently compiled parts retains two extensions
/// of a compiler Loaf: the library's delta and the test-dependency envelope's, each carrying the unified closure, its
/// re-rooted copies of shared units, and the extension's own runtime. Measured for IncQL/DataFusion on Linux, each is
/// 1.5 GiB, so a single debug-profile bake retains 3.0 GiB before its outputs and authority. Six GiB, half the physical
/// allowance, admits that bake with the release profile or a second project beside it; callers may still choose a
/// stricter explicit limit.
pub const DEFAULT_OVEN_MAX_DOMAIN_LOGICAL_BYTES: u64 = 6 * 1024 * 1024 * 1024;
/// Aggregate physical allowance for the complete compiler-suite Loaf and repository-test closure.
pub const DEFAULT_OVEN_COMPILER_SUITE_MAX_PHYSICAL_BYTES: u64 = 16 * 1024 * 1024 * 1024;
/// Physical allowance for the compiler-suite compatibility domain.
///
/// The retained suite currently measures about 3.6 GiB and a compatible publisher staging tree reaches 1.44 GiB.
/// Six GiB keeps the aggregate domain bounded while allowing one complete replacement with practical headroom.
pub const DEFAULT_OVEN_COMPILER_SUITE_MAX_DOMAIN_PHYSICAL_BYTES: u64 = 6 * 1024 * 1024 * 1024;
/// Logical artifact-byte allowance for the compiler-suite compatibility domain.
///
/// The complete LSP closure measures 3,271,283,026 logical bytes on Linux; 4 GiB leaves practical policy headroom
/// without relaxing its physical bound.
pub const DEFAULT_OVEN_COMPILER_SUITE_MAX_DOMAIN_LOGICAL_BYTES: u64 = 4 * 1024 * 1024 * 1024;

/// Explicit, portable build facts for one frozen-project import.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OvenImportRequest {
    project_root: PathBuf,
    target: String,
    toolchain: String,
    profile: String,
    features: Vec<String>,
    supplemental_source_digests: BTreeMap<String, String>,
}

/// Compiler-owned request to receipt generated Rust without making Cargo metadata a normal-command dependency.
///
/// A generated project keeps its source and final outputs caller-owned. Oven records only content digests from the
/// generated source closure, never its filesystem location. The selected direct-rustc artifact plan carries every
/// reusable native dependency separately in the bounded store.
#[derive(Debug, Clone)]
pub struct OvenGeneratedProjectRequest {
    project_root: PathBuf,
    project: OvenProjectIdentity,
    target: String,
    toolchain: String,
    profile: String,
    features: Vec<String>,
    generated_sources: BTreeMap<String, PathBuf>,
    generated_source_trees: BTreeMap<String, PathBuf>,
    build_unit_inputs: BTreeMap<String, String>,
}

/// Verified generated-source closure that may be shared by profile-specific receipts in one command.
///
/// The source closure is independent of the direct-Rustc profile. Keeping this proof separate lets a normal
/// library build derive debug and release receipt identities without walking the same generated tree twice.
#[derive(Debug, Clone)]
pub struct OvenGeneratedProjectSourceEvidence {
    names: BTreeSet<String>,
    supplemental_digests: BTreeMap<String, String>,
}

/// Compiler-owned request for the repository's Rust libtest suite receipt.
///
/// This is deliberately distinct from an arbitrary frozen Cargo package: it records the compiler source closure and
/// Cargo declarations as evidence for a bounded repository-suite publisher, then lets the normal consumer select and
/// compile through direct rustc. It does not make Cargo a normal test executor. The compiler workspace has no Cargo
/// package of its own, so the caller names it; its version is the workspace's.
#[derive(Debug, Clone)]
pub struct OvenCompilerSuiteRequest {
    project_root: PathBuf,
    workspace_name: String,
    target: String,
    toolchain: String,
    profile: String,
    features: Vec<String>,
    loaf_compatibility_identity: Option<String>,
}

impl OvenGeneratedProjectRequest {
    /// Construct a receipt request for one generated Incan project identity and direct-rustc build intent.
    #[must_use]
    pub fn new(
        project_root: impl AsRef<Path>,
        name: impl Into<String>,
        version: impl Into<String>,
        target: impl Into<String>,
        toolchain: impl Into<String>,
        profile: impl Into<String>,
        features: Vec<String>,
    ) -> Self {
        Self {
            project_root: project_root.as_ref().to_path_buf(),
            project: OvenProjectIdentity {
                name: name.into(),
                version: version.into(),
            },
            target: target.into(),
            toolchain: toolchain.into(),
            profile: profile.into(),
            features,
            generated_sources: BTreeMap::new(),
            generated_source_trees: BTreeMap::new(),
            build_unit_inputs: BTreeMap::new(),
        }
    }

    /// Add one generated source file whose exact bytes later authorize a direct-rustc consumer input.
    #[must_use]
    pub fn with_generated_source(mut self, name: impl Into<String>, path: impl AsRef<Path>) -> Self {
        self.generated_sources.insert(name.into(), path.as_ref().to_path_buf());
        self
    }

    /// Add one generated source tree whose complete closure affects the selected build unit.
    #[must_use]
    pub fn with_generated_source_tree(mut self, name: impl Into<String>, path: impl AsRef<Path>) -> Self {
        self.generated_source_trees
            .insert(name.into(), path.as_ref().to_path_buf());
        self
    }

    /// Add a portable compiler, SDK, provider, or lock input that selects the reusable native build unit.
    ///
    /// Unlike generated-source evidence, these inputs intentionally do not make the native closure project-specific:
    /// compatible clean worktrees may select the same stored Oven plan while retaining distinct generated source and
    /// final-output directories.
    #[must_use]
    pub fn with_build_unit_input(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.build_unit_inputs.insert(name.into(), value.into());
        self
    }

    /// Return the caller-owned generated-project root.
    #[must_use]
    pub fn project_root(&self) -> &Path {
        &self.project_root
    }
}

impl OvenCompilerSuiteRequest {
    /// Construct a request for the compiler workspace's direct-rustc test-suite compatibility unit.
    ///
    /// `workspace_name` is the identity the receipt records for the workspace: Cargo gives a virtual workspace no
    /// name, and a checkout's directory name is not portable across machines.
    #[must_use]
    pub fn new(
        project_root: impl AsRef<Path>,
        workspace_name: impl Into<String>,
        target: impl Into<String>,
        toolchain: impl Into<String>,
        profile: impl Into<String>,
        features: Vec<String>,
    ) -> Self {
        Self {
            project_root: project_root.as_ref().to_path_buf(),
            workspace_name: workspace_name.into(),
            target: target.into(),
            toolchain: toolchain.into(),
            profile: profile.into(),
            features,
            loaf_compatibility_identity: None,
        }
    }

    /// Bind the compiler self-suite to the compatible committed Loaf member set its nested commands consume.
    ///
    /// This reusable build-unit input changes when a sealed member closure or direct-Rustc plan changes. It excludes
    /// envelope publication evidence, so an otherwise irrelevant compiler executable rebuild does not invalidate the
    /// lock/toolchain-bound compiler-suite foundation.
    #[must_use]
    pub fn with_loaf_compatibility_identity(mut self, identity: impl Into<String>) -> Self {
        self.loaf_compatibility_identity = Some(identity.into());
        self
    }
}

impl OvenImportRequest {
    /// Construct a request whose target and toolchain are caller-provided evidence rather than host defaults.
    #[must_use]
    pub fn new(
        project_root: impl AsRef<Path>,
        target: impl Into<String>,
        toolchain: impl Into<String>,
        profile: impl Into<String>,
        features: Vec<String>,
    ) -> Self {
        Self {
            project_root: project_root.as_ref().to_path_buf(),
            target: target.into(),
            toolchain: toolchain.into(),
            profile: profile.into(),
            features,
            supplemental_source_digests: BTreeMap::new(),
        }
    }

    /// Add immutable source evidence not expressed by Cargo declarations, such as a generated Incan test harness.
    #[must_use]
    pub fn with_supplemental_source_digest(mut self, name: impl Into<String>, digest: impl Into<String>) -> Self {
        self.supplemental_source_digests.insert(name.into(), digest.into());
        self
    }

    /// Return the root whose frozen declarations are imported.
    #[must_use]
    pub fn project_root(&self) -> &Path {
        &self.project_root
    }
}

/// Stable package identity shared by the imported Cargo package and optional Incan project declaration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OvenProjectIdentity {
    /// Package/distribution name.
    pub name: String,
    /// Complete package version.
    pub version: String,
}

/// Normalized source evidence that authorizes one frozen project receipt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OvenSourceEvidence {
    /// SHA-256 digest of normalized `Cargo.toml` content when a frozen Cargo package was explicitly imported.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cargo_manifest_digest: Option<String>,
    /// SHA-256 digest of normalized `Cargo.lock` content when a frozen Cargo package was explicitly imported.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cargo_lock_digest: Option<String>,
    /// SHA-256 digest of normalized `loaf.toml` content when present.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub incan_manifest_digest: Option<String>,
    /// Additional content-derived inputs from the source closure; local paths are deliberately excluded.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub supplemental_digests: BTreeMap<String, String>,
    /// Portable compiler, SDK, provider, and lock inputs that select a reusable native build unit.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub build_unit_inputs: BTreeMap<String, String>,
}

/// Explicit build facts whose change requires a distinct Oven build-unit selection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OvenBuildIntent {
    /// Target triple selected for the requested build unit.
    pub target: String,
    /// Exact selected Rust toolchain identity.
    pub toolchain: String,
    /// Named build profile.
    pub profile: String,
    /// Deterministically ordered enabled feature set.
    pub features: Vec<String>,
}

/// Named compilation contract for Oven's receipt-bound compiler-suite publisher and direct-rustc runner.
///
/// This is intentionally separate from a developer's default Cargo `dev` profile: normal Incan commands consume
/// the stored direct-rustc unit and never launch Cargo. The explicit publisher's matching Cargo profile is declared
/// in the compiler manifest solely to bootstrap that immutable Oven unit during Alpha.
pub const OVEN_COMPILER_TEST_PROFILE: &str = "oven-test";

/// Compatibility envelope used to construct an Oven receipt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OvenCompatibilityKind {
    /// One root Cargo package with an available lock file; virtual workspaces are not supported in Alpha.
    FrozenCargoPackage,
    /// Generated Rust from one checked Incan project, selected without a Cargo consumer process or target directory.
    GeneratedIncanProject,
    /// The repository compiler's source-backed Rust libtest suite, executed by a receipt-bound direct-rustc runner.
    NativeCompilerTestSuite,
}

/// Process and compatibility conditions for one receipt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OvenCompatibility {
    /// Alpha compatibility envelope selected by the receipt.
    pub kind: OvenCompatibilityKind,
    /// Cargo files were read as evidence only; no Cargo process participated.
    pub cargo_input_only: bool,
}

/// Portable, versioned identity of a frozen Oven project import.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OvenReceipt {
    /// Receipt wire-schema version.
    pub schema_version: u32,
    /// Content-derived `sha256:` identity.
    pub identity: String,
    /// Content-derived identity of the reusable compiler/SDK/provider build unit, separate from project source.
    pub build_unit_identity: String,
    /// Frozen root package identity.
    pub project: OvenProjectIdentity,
    /// Imported source declarations and supplemental closure evidence.
    pub sources: OvenSourceEvidence,
    /// Target/toolchain/profile/features selected for the build unit.
    pub intent: OvenBuildIntent,
    /// Compatibility envelope and Cargo process boundary.
    pub compatibility: OvenCompatibility,
}

/// Typed failure while importing or atomically publishing an Oven receipt.
#[derive(Debug, thiserror::Error)]
pub enum OvenError {
    /// A persisted receipt uses an unsupported schema version.
    #[error("unsupported Oven receipt schema version {found}; expected {expected}")]
    UnsupportedReceiptSchema { found: u32, expected: u32 },
    /// A receipt's claimed content identity did not match its immutable fields.
    #[error("Oven receipt identity mismatch: expected {expected}, got {actual}")]
    ReceiptIdentityMismatch { expected: String, actual: String },
    /// The reusable compiler/SDK/provider identity is inconsistent with the receipt's persisted build inputs.
    #[error("Oven build-unit identity mismatch: expected {expected}, got {actual}")]
    BuildUnitIdentityMismatch { expected: String, actual: String },
    /// A required frozen Cargo input was absent.
    #[error("Oven Alpha compatibility miss: {file_name} is required at {path}")]
    MissingCargoInput { file_name: &'static str, path: PathBuf },
    /// An input could not be read.
    #[error("failed to read Oven input {path}: {source}")]
    ReadInput { path: PathBuf, source: io::Error },
    /// The compiler's provider hooks could not answer for the path dependency at `path`.
    #[error("Oven provider hook failed for {path}: {source}")]
    ProviderHook {
        /// The path dependency the hook was asked about.
        path: PathBuf,
        /// The hook's typed failure.
        #[source]
        source: OvenProviderHookError,
    },
    /// Cargo manifest text did not parse as TOML.
    #[error("Oven Alpha compatibility miss: failed to parse Cargo.toml at {path}: {message}")]
    InvalidCargoManifest { path: PathBuf, message: String },
    /// Cargo lock text did not parse as TOML.
    #[error("Oven Alpha compatibility miss: failed to parse Cargo.lock at {path}: {message}")]
    InvalidCargoLock { path: PathBuf, message: String },
    /// Imported Cargo metadata lies outside the narrow Alpha support envelope.
    #[error("Oven Alpha compatibility miss: Cargo.toml at {path} {message}")]
    UnsupportedCargoPackage { path: PathBuf, message: String },
    /// An optional Incan declaration could not support shared project identity validation.
    #[error("Oven Alpha compatibility miss: failed to read loaf.toml at {path}: {message}")]
    InvalidIncanManifest { path: PathBuf, message: String },
    /// Cargo and Incan declarations disagreed on one shared identity field.
    #[error(
        "Oven Alpha compatibility miss: Cargo {field} `{cargo}` disagrees with Incan project {field} `{incan}` at {path}"
    )]
    ProjectIdentityMismatch {
        path: PathBuf,
        field: &'static str,
        cargo: String,
        incan: String,
    },
    /// A required build identity value was blank.
    #[error("Oven import requires a non-empty {field}")]
    EmptyBuildIntent { field: &'static str },
    /// Supplemental source evidence cannot identify a portable build unit.
    #[error("Oven import requires a non-empty supplemental source {field}")]
    EmptySupplementalSource { field: &'static str },
    /// A selected build unit has no compatible native dependency closure retained for it.
    ///
    /// Distinct from an ordinary cache miss: the closure is a selection the Incan Oven control plane supplies, and
    /// its absence is a refusal rather than a reason to rebuild.
    #[error(
        "Oven selected native plan unavailable for build unit {build_unit_identity}; execution requires a compatible dependency closure"
    )]
    SelectedNativePlanUnavailable { build_unit_identity: String },
    /// The store holds a native plan for this build unit that this compiler cannot read.
    ///
    /// Deliberately distinct from [`OvenError::SelectedNativePlanUnavailable`]: something *is* published and
    /// reading it failed. The two ask for opposite responses — bake, or look at the store — so reporting a corrupt
    /// or newer-than-this-build record as an absence sends a reader to rebuild something that already exists.
    #[error("Oven selected native plan for build unit {build_unit_identity} cannot be read: {message}")]
    SelectedNativePlanUnreadable {
        build_unit_identity: String,
        message: String,
    },
    /// A requested receipt transformation named a build-unit input that was not present.
    #[error("Oven receipt has no build-unit input `{input}`")]
    MissingBuildUnitInput { input: String },
    /// A typed receipt transition would replace an identity input it must add exactly once.
    #[error("Oven receipt already has build-unit input `{input}`")]
    ExistingBuildUnitInput { input: String },
    /// A generated source input could not be read or did not satisfy the Alpha regular-file closure rules.
    #[error("invalid Oven generated source {path}: {message}")]
    InvalidGeneratedSource { path: PathBuf, message: String },
    /// An authored project source input could not be read or did not satisfy the Alpha regular-file closure rules.
    #[error("invalid Oven project source {path}: {message}")]
    InvalidProjectSource { path: PathBuf, message: String },
    /// Receipt JSON could not be serialized.
    #[error("failed to serialize Oven receipt: {0}")]
    Serialize(String),
    /// The requested receipt destination cannot support a safe publication.
    #[error("invalid Oven receipt output path {path}")]
    InvalidReceiptPath { path: PathBuf },
    /// Atomic receipt publication failed.
    #[error("failed to publish Oven receipt at {path}: {source}")]
    WriteReceipt { path: PathBuf, source: io::Error },
}

impl OvenReceipt {
    /// Recompute the content identity before a later Oven stage trusts this receipt as authorization.
    pub fn verify_identity(&self) -> Result<(), OvenError> {
        if self.schema_version != OVEN_RECEIPT_SCHEMA_VERSION {
            return Err(OvenError::UnsupportedReceiptSchema {
                found: self.schema_version,
                expected: OVEN_RECEIPT_SCHEMA_VERSION,
            });
        }
        let actual = receipt_identity(&self.project, &self.sources, &self.intent, &self.compatibility)?;
        if actual != self.identity {
            return Err(OvenError::ReceiptIdentityMismatch {
                expected: self.identity.clone(),
                actual,
            });
        }
        let actual_build_unit =
            build_unit_identity(&self.intent, &self.compatibility, &self.sources.build_unit_inputs)?;
        if actual_build_unit == self.build_unit_identity {
            return Ok(());
        }
        Err(OvenError::BuildUnitIdentityMismatch {
            expected: self.build_unit_identity.clone(),
            actual: actual_build_unit,
        })
    }
}

/// Import a frozen root Cargo package without resolving dependencies or launching Cargo.
pub fn import_frozen_project(request: &OvenImportRequest) -> Result<OvenReceipt, OvenError> {
    let cargo_manifest_path = request.project_root.join("Cargo.toml");
    let cargo_lock_path = request.project_root.join("Cargo.lock");
    let cargo_manifest = read_required_input(&cargo_manifest_path, "Cargo.toml")?;
    let cargo_lock = read_required_input(&cargo_lock_path, "Cargo.lock")?;
    let project = parse_cargo_package(&cargo_manifest_path, &cargo_manifest)?;
    validate_cargo_lock(&cargo_lock_path, &cargo_lock)?;
    let incan_manifest_digest = validate_optional_incan_identity(&request.project_root, &project)?;
    let sources = OvenSourceEvidence {
        cargo_manifest_digest: Some(digest_content(&cargo_manifest)),
        cargo_lock_digest: Some(digest_content(&cargo_lock)),
        incan_manifest_digest,
        supplemental_digests: normalized_supplemental_source_digests(request)?,
        build_unit_inputs: BTreeMap::new(),
    };
    let intent = normalized_intent(request)?;
    let compatibility = OvenCompatibility {
        kind: OvenCompatibilityKind::FrozenCargoPackage,
        cargo_input_only: true,
    };
    let identity = receipt_identity(&project, &sources, &intent, &compatibility)?;
    let build_unit_identity = build_unit_identity(&intent, &compatibility, &BTreeMap::new())?;
    Ok(OvenReceipt {
        schema_version: OVEN_RECEIPT_SCHEMA_VERSION,
        identity,
        build_unit_identity,
        project,
        sources,
        intent,
        compatibility,
    })
}

/// Receipt one generated Incan/Rust source closure without reading Cargo metadata or launching Cargo.
///
/// The resulting identity is portable across clean worktrees because it records project identity, explicit build
/// intent, and generated-source digests rather than any local source or output path. A later Oven plan selection
/// must still prove its exact native dependency closure against this receipt before `rustc` runs.
pub fn receipt_generated_project(request: &OvenGeneratedProjectRequest) -> Result<OvenReceipt, OvenError> {
    let source_evidence = generated_project_source_evidence(request)?;
    receipt_generated_project_with_source_evidence(request, &source_evidence)
}

/// Derive one reusable generated-source proof for profile-specific receipts in the current command.
///
/// This reads and verifies every requested source exactly once. The opaque result can be supplied only to a request
/// with the same normalized source-evidence keys, so a debug or release intent cannot silently borrow unrelated
/// generated inputs.
pub fn generated_project_source_evidence(
    request: &OvenGeneratedProjectRequest,
) -> Result<OvenGeneratedProjectSourceEvidence, OvenError> {
    let names = generated_source_evidence_names(request)?;
    let supplemental_digests = generated_source_evidence(request)?;
    Ok(OvenGeneratedProjectSourceEvidence {
        names,
        supplemental_digests,
    })
}

/// Receipt a generated project with a previously verified source closure.
///
/// The caller may vary build intent, including debug versus release profile, but the request must retain exactly the
/// source-evidence keys that produced `source_evidence`. This is an in-process reuse boundary, not a persisted
/// cache: each returned receipt still carries complete content-derived source evidence and verifies normally.
pub fn receipt_generated_project_with_source_evidence(
    request: &OvenGeneratedProjectRequest,
    source_evidence: &OvenGeneratedProjectSourceEvidence,
) -> Result<OvenReceipt, OvenError> {
    let expected_names = generated_source_evidence_names(request)?;
    if source_evidence.names != expected_names {
        return Err(OvenError::InvalidGeneratedSource {
            path: request.project_root.clone(),
            message: "reused source evidence does not match the request's generated source keys".to_string(),
        });
    }
    let project = OvenProjectIdentity {
        name: normalized_value(&request.project.name, "project name")?,
        version: normalized_value(&request.project.version, "project version")?,
    };
    let sources = OvenSourceEvidence {
        cargo_manifest_digest: None,
        cargo_lock_digest: None,
        incan_manifest_digest: None,
        supplemental_digests: source_evidence.supplemental_digests.clone(),
        build_unit_inputs: normalized_build_unit_inputs(&request.build_unit_inputs)?,
    };
    let intent = normalized_build_intent(&request.target, &request.toolchain, &request.profile, &request.features)?;
    let compatibility = OvenCompatibility {
        kind: OvenCompatibilityKind::GeneratedIncanProject,
        cargo_input_only: false,
    };
    let identity = receipt_identity(&project, &sources, &intent, &compatibility)?;
    let build_unit_identity = build_unit_identity(&intent, &compatibility, &sources.build_unit_inputs)?;
    Ok(OvenReceipt {
        schema_version: OVEN_RECEIPT_SCHEMA_VERSION,
        identity,
        build_unit_identity,
        project,
        sources,
        intent,
        compatibility,
    })
}

/// Derive a new complete receipt whose reusable build unit carries one explicit selected-tool input.
///
/// This is an immutable value transformation; callers persist the returned receipt atomically only after their
/// explicit publisher has verified the selected input. Replacing the same key is deliberate: a reselected compiler
/// or SDK must invalidate a prior build unit rather than accumulate stale selection identities.
pub fn receipt_with_build_unit_input(
    receipt: &OvenReceipt,
    input: impl Into<String>,
    value: impl Into<String>,
) -> Result<OvenReceipt, OvenError> {
    receipt.verify_identity()?;
    let input = normalized_value(&input.into(), "build-unit input")?;
    let value = normalized_value(&value.into(), "build-unit input value")?;
    let mut selected = receipt.clone();
    selected.sources.build_unit_inputs.insert(input, value);
    selected.identity = receipt_identity(
        &selected.project,
        &selected.sources,
        &selected.intent,
        &selected.compatibility,
    )?;
    selected.build_unit_identity = build_unit_identity(
        &selected.intent,
        &selected.compatibility,
        &selected.sources.build_unit_inputs,
    )?;
    Ok(selected)
}

/// Derive the final authority receipt after one capture-only compiler-release transaction.
///
/// The capture receipt must already verify and must not yet carry compiler-release root intent. This method preserves
/// its complete project, source, intent, and compatibility evidence, adds exactly the canonical root-intent digest,
/// then recomputes and verifies both identities. It does not authorize any artifact produced by the capture-only
/// transaction under the returned receipt; the caller must publish final artifacts only after this transition.
pub fn receipt_with_compiler_support_root_intent(
    capture_receipt: &OvenReceipt,
    root_intent_digest: impl AsRef<str>,
) -> Result<OvenReceipt, OvenError> {
    capture_receipt.verify_identity()?;
    let digest = normalized_value(root_intent_digest.as_ref(), "compiler support root-intent digest")?;
    if capture_receipt
        .sources
        .build_unit_inputs
        .contains_key(OVEN_COMPILER_SUPPORT_ROOT_INTENT_BUILD_UNIT_INPUT)
    {
        return Err(OvenError::ExistingBuildUnitInput {
            input: OVEN_COMPILER_SUPPORT_ROOT_INTENT_BUILD_UNIT_INPUT.to_string(),
        });
    }
    let mut final_receipt = capture_receipt.clone();
    final_receipt
        .sources
        .build_unit_inputs
        .insert(OVEN_COMPILER_SUPPORT_ROOT_INTENT_BUILD_UNIT_INPUT.to_string(), digest);
    final_receipt.identity = receipt_identity(
        &final_receipt.project,
        &final_receipt.sources,
        &final_receipt.intent,
        &final_receipt.compatibility,
    )?;
    final_receipt.build_unit_identity = build_unit_identity(
        &final_receipt.intent,
        &final_receipt.compatibility,
        &final_receipt.sources.build_unit_inputs,
    )?;
    final_receipt.verify_identity()?;
    Ok(final_receipt)
}

/// Derive a new complete receipt with one selected build-unit input removed.
///
/// This is the inverse of [`receipt_with_build_unit_input`] for the narrow cases where a compiler-owned capability
/// must be selected independently from a project-only input. The returned receipt is independently identity-checked;
/// callers must still prove that any selected immutable artifact provides the capability they need.
pub fn receipt_without_build_unit_input(receipt: &OvenReceipt, input: &str) -> Result<OvenReceipt, OvenError> {
    receipt.verify_identity()?;
    let mut selected = receipt.clone();
    if selected.sources.build_unit_inputs.remove(input).is_none() {
        return Err(OvenError::MissingBuildUnitInput {
            input: input.to_string(),
        });
    }
    selected.identity = receipt_identity(
        &selected.project,
        &selected.sources,
        &selected.intent,
        &selected.compatibility,
    )?;
    selected.build_unit_identity = build_unit_identity(
        &selected.intent,
        &selected.compatibility,
        &selected.sources.build_unit_inputs,
    )?;
    Ok(selected)
}

/// Receipt the compiler's full native workspace-test source closure without invoking Cargo.
///
/// Cargo.toml and Cargo.lock are immutable compatibility evidence only. The explicit `legacy_cargo` publisher may
/// later materialize an exact native workspace target plan and dependency closure; normal suite execution compiles
/// and runs those receipt-bound targets without inspecting a Cargo target directory.
pub fn receipt_native_compiler_suite(request: &OvenCompilerSuiteRequest) -> Result<OvenReceipt, OvenError> {
    let cargo_manifest_path = request.project_root.join("Cargo.toml");
    let cargo_lock_path = request.project_root.join("Cargo.lock");
    let cargo_manifest = read_required_input(&cargo_manifest_path, "Cargo.toml")?;
    let cargo_lock = read_required_input(&cargo_lock_path, "Cargo.lock")?;
    let project = parse_compiler_workspace_identity(&cargo_manifest_path, &cargo_manifest, &request.workspace_name)?;
    validate_cargo_lock(&cargo_lock_path, &cargo_lock)?;
    let cargo_manifest_digest = digest_content(&cargo_manifest);
    let cargo_lock_digest = digest_content(&cargo_lock);
    let compiler_source_records = compiler_suite_source_records(&request.project_root)?;
    let compiler_source_tree_digest = digest_compiler_suite_source_records(&compiler_source_records)?;
    let compiler_plan_digest = compiler_suite_plan_digest(&request.project_root, &compiler_source_records)?;
    let mut build_unit_inputs = BTreeMap::new();
    build_unit_inputs.insert("compiler-cargo-manifest".to_string(), cargo_manifest_digest.clone());
    build_unit_inputs.insert("compiler-cargo-lock".to_string(), cargo_lock_digest.clone());
    build_unit_inputs.insert("compiler-suite-plan".to_string(), compiler_plan_digest);
    if let Some(identity) = &request.loaf_compatibility_identity {
        build_unit_inputs.insert("compiler-loaf-compatibility".to_string(), identity.clone());
    }
    let mut supplemental_digests = BTreeMap::from([
        (
            COMPILER_WORKSPACE_MANIFEST_EVIDENCE_KEY.to_string(),
            cargo_manifest_digest.clone(),
        ),
        ("compiler-suite-source-tree".to_string(), compiler_source_tree_digest),
    ]);
    // A full native-suite plan must authorize each root passed to direct rustc; the workspace manifest is the one
    // source the publisher binds to by name, every Rust file is a tree record. Source bytes belong to the exact
    // command receipt, while the reusable build unit above records only inputs that can change Cargo's
    // target/dependency plan. Editing an existing Rust module therefore reuses the immutable foundation; adding a
    // new source path or changing a manifest still requires an explicit rebake.
    for (relative_path, digest) in &compiler_source_records {
        if relative_path.ends_with(".rs") {
            supplemental_digests.insert(compiler_suite_source_evidence_key(relative_path), digest.clone());
        }
    }
    let sources = OvenSourceEvidence {
        cargo_manifest_digest: Some(cargo_manifest_digest),
        cargo_lock_digest: Some(cargo_lock_digest),
        incan_manifest_digest: None,
        supplemental_digests,
        build_unit_inputs: normalized_build_unit_inputs(&build_unit_inputs)?,
    };
    let intent = normalized_build_intent(&request.target, &request.toolchain, &request.profile, &request.features)?;
    let compatibility = OvenCompatibility {
        kind: OvenCompatibilityKind::NativeCompilerTestSuite,
        cargo_input_only: true,
    };
    let identity = receipt_identity(&project, &sources, &intent, &compatibility)?;
    let build_unit_identity = build_unit_identity(&intent, &compatibility, &sources.build_unit_inputs)?;
    Ok(OvenReceipt {
        schema_version: OVEN_RECEIPT_SCHEMA_VERSION,
        identity,
        build_unit_identity,
        project,
        sources,
        intent,
        compatibility,
    })
}

/// Return the compiler-owned project-relative destination for an Oven receipt.
#[must_use]
pub fn default_receipt_path(project_root: impl AsRef<Path>) -> PathBuf {
    project_root.as_ref().join(DEFAULT_RECEIPT_RELATIVE_PATH)
}

/// Publish a complete receipt through a same-directory staged file and atomic replacement.
pub fn write_receipt(receipt: &OvenReceipt, path: impl AsRef<Path>) -> Result<(), OvenError> {
    let path = path.as_ref();
    let parent = path.parent().ok_or_else(|| OvenError::InvalidReceiptPath {
        path: path.to_path_buf(),
    })?;
    let file_name = path.file_name().ok_or_else(|| OvenError::InvalidReceiptPath {
        path: path.to_path_buf(),
    })?;
    fs::create_dir_all(parent).map_err(|source| OvenError::WriteReceipt {
        path: path.to_path_buf(),
        source,
    })?;
    let payload = serde_json::to_vec_pretty(receipt).map_err(|error| OvenError::Serialize(error.to_string()))?;
    let staged_path = parent.join(format!(".{}.tmp-{}", file_name.to_string_lossy(), std::process::id()));
    let result = write_receipt_staged(&payload, &staged_path, path, parent);
    if result.is_err() && staged_path.exists() {
        let _ = fs::remove_file(&staged_path);
    }
    result.map_err(|source| OvenError::WriteReceipt {
        path: path.to_path_buf(),
        source,
    })
}

/// Read and normalize one required frozen input.
fn read_required_input(path: &Path, file_name: &'static str) -> Result<String, OvenError> {
    match fs::read_to_string(path) {
        Ok(content) => Ok(normalize_content(&content)),
        Err(source) if source.kind() == ErrorKind::NotFound => Err(OvenError::MissingCargoInput {
            file_name,
            path: path.to_path_buf(),
        }),
        Err(source) => Err(OvenError::ReadInput {
            path: path.to_path_buf(),
            source,
        }),
    }
}

/// Supplemental-digest key under which a native compiler-suite receipt records its workspace manifest.
///
/// The explicit library-tests publisher binds its publication to this key: the workspace `Cargo.toml` is the one
/// source the suite names, where a generated project names its `src/main.rs` or `src/lib.rs`.
pub const COMPILER_WORKSPACE_MANIFEST_EVIDENCE_KEY: &str = "compiler-workspace-manifest";

/// Resolve the identity a compiler-suite receipt records for the workspace at `path`.
///
/// A Cargo workspace has no name of its own, so the caller supplies one; the version is `[workspace.package]`'s,
/// or `[package]`'s when the suite root is a rooted fixture package rather than a virtual manifest.
fn parse_compiler_workspace_identity(
    path: &Path,
    content: &str,
    workspace_name: &str,
) -> Result<OvenProjectIdentity, OvenError> {
    let document = toml::from_str::<toml::Value>(content).map_err(|error| OvenError::InvalidCargoManifest {
        path: path.to_path_buf(),
        message: error.to_string(),
    })?;
    let workspace_package = document
        .get("workspace")
        .and_then(toml::Value::as_table)
        .and_then(|workspace| workspace.get("package"))
        .and_then(toml::Value::as_table);
    let version = match document.get("package").and_then(toml::Value::as_table) {
        Some(package) => package_string_field(path, package, workspace_package, "version")?,
        None => {
            let value = workspace_package
                .and_then(|workspace| workspace.get("version"))
                .and_then(toml::Value::as_str)
                .ok_or_else(|| OvenError::UnsupportedCargoPackage {
                    path: path.to_path_buf(),
                    message: "a compiler workspace must declare [workspace.package].version".to_string(),
                })?;
            normalized_value(value, "version")?
        }
    };
    Ok(OvenProjectIdentity {
        name: workspace_name.to_string(),
        version,
    })
}

fn parse_cargo_package(path: &Path, content: &str) -> Result<OvenProjectIdentity, OvenError> {
    let document = toml::from_str::<toml::Value>(content).map_err(|error| OvenError::InvalidCargoManifest {
        path: path.to_path_buf(),
        message: error.to_string(),
    })?;
    let package =
        document
            .get("package")
            .and_then(toml::Value::as_table)
            .ok_or_else(|| OvenError::UnsupportedCargoPackage {
                path: path.to_path_buf(),
                message: "must declare one [package] table; virtual workspaces are not supported".to_string(),
            })?;
    let workspace_package = document
        .get("workspace")
        .and_then(toml::Value::as_table)
        .and_then(|workspace| workspace.get("package"))
        .and_then(toml::Value::as_table);
    let name = package_string_field(path, package, workspace_package, "name")?;
    let version = package_string_field(path, package, workspace_package, "version")?;
    Ok(OvenProjectIdentity { name, version })
}

/// Validate that the imported lock retains Cargo's TOML-based frozen representation.
fn validate_cargo_lock(path: &Path, content: &str) -> Result<(), OvenError> {
    toml::from_str::<toml::Value>(content)
        .map(|_| ())
        .map_err(|error| OvenError::InvalidCargoLock {
            path: path.to_path_buf(),
            message: error.to_string(),
        })
}

/// Resolve one root package field, including explicit Cargo workspace-package inheritance.
fn package_string_field(
    path: &Path,
    package: &toml::map::Map<String, toml::Value>,
    workspace_package: Option<&toml::map::Map<String, toml::Value>>,
    field: &'static str,
) -> Result<String, OvenError> {
    if let Some(value) = package.get(field).and_then(toml::Value::as_str) {
        return normalized_value(value, field);
    }
    let inherits_workspace_value = package
        .get(field)
        .and_then(toml::Value::as_table)
        .and_then(|value| value.get("workspace"))
        .and_then(toml::Value::as_bool)
        .unwrap_or(false);
    if inherits_workspace_value
        && let Some(value) = workspace_package
            .and_then(|workspace| workspace.get(field))
            .and_then(toml::Value::as_str)
    {
        return normalized_value(value, field);
    }
    Err(OvenError::UnsupportedCargoPackage {
        path: path.to_path_buf(),
        message: format!("must declare [package].{field} as a string or inherit it from [workspace.package].{field}"),
    })
}

/// Validate optional Incan identity evidence and return its normalized content digest.
fn validate_optional_incan_identity(
    project_root: &Path,
    cargo_project: &OvenProjectIdentity,
) -> Result<Option<String>, OvenError> {
    let path = project_root.join("loaf.toml");
    if !path.exists() {
        return Ok(None);
    }
    let content = fs::read_to_string(&path).map_err(|source| OvenError::InvalidIncanManifest {
        path: path.clone(),
        message: source.to_string(),
    })?;
    let manifest = ProjectManifest::load(&path).map_err(|error| OvenError::InvalidIncanManifest {
        path: path.clone(),
        message: error.to_string(),
    })?;
    if let Some(project) = manifest.project {
        if let Some(name) = project.name {
            compare_identity_field(&path, "name", &cargo_project.name, &name)?;
        }
        if let Some(version) = project.version {
            compare_identity_field(&path, "version", &cargo_project.version, &version)?;
        }
    }
    Ok(Some(digest_content(&normalize_content(&content))))
}

/// Reject conflicting project identity declarations.
fn compare_identity_field(path: &Path, field: &'static str, cargo: &str, incan: &str) -> Result<(), OvenError> {
    if cargo == incan {
        return Ok(());
    }
    Err(OvenError::ProjectIdentityMismatch {
        path: path.to_path_buf(),
        field,
        cargo: cargo.to_string(),
        incan: incan.to_string(),
    })
}

/// Normalize explicit target, toolchain, profile, and feature inputs before identity calculation.
fn normalized_intent(request: &OvenImportRequest) -> Result<OvenBuildIntent, OvenError> {
    normalized_build_intent(&request.target, &request.toolchain, &request.profile, &request.features)
}

/// Normalize explicit target, toolchain, profile, and feature inputs shared by imported and generated receipts.
pub fn normalized_build_intent(
    target: &str,
    toolchain: &str,
    profile: &str,
    requested_features: &[String],
) -> Result<OvenBuildIntent, OvenError> {
    let mut features = BTreeSet::new();
    for feature in requested_features {
        features.insert(normalized_value(feature, "feature")?);
    }
    Ok(OvenBuildIntent {
        target: normalized_value(target, "target")?,
        toolchain: normalized_value(toolchain, "toolchain")?,
        profile: normalized_value(profile, "profile")?,
        features: features.into_iter().collect(),
    })
}

/// Normalize caller-provided content digests before they join a receipt identity.
fn normalized_supplemental_source_digests(request: &OvenImportRequest) -> Result<BTreeMap<String, String>, OvenError> {
    let mut normalized = BTreeMap::new();
    for (name, digest) in &request.supplemental_source_digests {
        let name = name.trim();
        if name.is_empty() {
            return Err(OvenError::EmptySupplementalSource { field: "name" });
        }
        let digest = digest.trim();
        if digest.is_empty() {
            return Err(OvenError::EmptySupplementalSource { field: "digest" });
        }
        normalized.insert(name.to_string(), digest.to_string());
    }
    Ok(normalized)
}

/// Normalize explicit reusable build-unit inputs without permitting blank identity records.
fn normalized_build_unit_inputs(inputs: &BTreeMap<String, String>) -> Result<BTreeMap<String, String>, OvenError> {
    let mut normalized = BTreeMap::new();
    for (name, value) in inputs {
        let name = name.trim();
        if name.is_empty() {
            return Err(OvenError::EmptySupplementalSource {
                field: "build-unit input name",
            });
        }
        let value = value.trim();
        if value.is_empty() {
            return Err(OvenError::EmptySupplementalSource {
                field: "build-unit input value",
            });
        }
        normalized.insert(name.to_string(), value.to_string());
    }
    Ok(normalized)
}

/// Digest every generated source input while rejecting symlinks and duplicate evidence keys.
fn generated_source_evidence(request: &OvenGeneratedProjectRequest) -> Result<BTreeMap<String, String>, OvenError> {
    let mut digests = BTreeMap::new();
    for (name, path) in &request.generated_sources {
        let name = normalized_generated_source_name(name)?;
        let digest = digest_generated_source_file(path)?;
        if digests.insert(name.clone(), digest).is_some() {
            return Err(OvenError::InvalidGeneratedSource {
                path: path.clone(),
                message: format!("duplicate generated source evidence key `{name}`"),
            });
        }
    }
    for (name, path) in &request.generated_source_trees {
        let name = normalized_generated_source_name(name)?;
        let digest = digest_source_tree(path)?;
        if digests.insert(name.clone(), digest).is_some() {
            return Err(OvenError::InvalidGeneratedSource {
                path: path.clone(),
                message: format!("duplicate generated source evidence key `{name}`"),
            });
        }
    }
    if digests.is_empty() {
        return Err(OvenError::InvalidGeneratedSource {
            path: request.project_root.clone(),
            message: "must declare at least one generated source file or tree".to_string(),
        });
    }
    Ok(digests)
}

/// Return the normalized source-evidence key set without reading the requested files.
fn generated_source_evidence_names(request: &OvenGeneratedProjectRequest) -> Result<BTreeSet<String>, OvenError> {
    let mut names = BTreeSet::new();
    for (name, path) in request
        .generated_sources
        .iter()
        .chain(request.generated_source_trees.iter())
    {
        let name = normalized_generated_source_name(name)?;
        if !names.insert(name.clone()) {
            return Err(OvenError::InvalidGeneratedSource {
                path: path.clone(),
                message: format!("duplicate generated source evidence key `{name}`"),
            });
        }
    }
    if names.is_empty() {
        return Err(OvenError::InvalidGeneratedSource {
            path: request.project_root.clone(),
            message: "must declare at least one generated source file or tree".to_string(),
        });
    }
    Ok(names)
}

/// Normalize a caller-facing source-evidence key without allowing blank identity records.
fn normalized_generated_source_name(name: &str) -> Result<String, OvenError> {
    let normalized = name.trim();
    if normalized.is_empty() {
        return Err(OvenError::EmptySupplementalSource { field: "name" });
    }
    Ok(normalized.to_string())
}

/// Hash one direct-rustc source file after proving it is a regular, non-symlink input.
fn digest_generated_source_file(path: &Path) -> Result<String, OvenError> {
    let metadata = fs::symlink_metadata(path).map_err(|error| OvenError::InvalidGeneratedSource {
        path: path.to_path_buf(),
        message: error.to_string(),
    })?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(OvenError::InvalidGeneratedSource {
            path: path.to_path_buf(),
            message: "must be a regular non-symlink file".to_string(),
        });
    }
    fs::read(path)
        .map(|bytes| digest_bytes(&bytes))
        .map_err(|error| OvenError::InvalidGeneratedSource {
            path: path.to_path_buf(),
            message: error.to_string(),
        })
}

/// Hash the workspace source and fixture closure that determines the repository's native test-suite behaviour.
///
/// Oven deliberately excludes caller outputs such as `.incan` and `target`: those are neither compiler source nor test
/// fixtures, and allowing them into the receipt would make a successful test run invalidate its own stored suite. Every
/// tracked source, fixture, snapshot, and nested crate manifest below the declared roots remains identity-bearing.
///
/// Return the portable source-to-digest records that make up one native compiler-suite receipt.
fn compiler_suite_source_records(project_root: &Path) -> Result<BTreeMap<String, String>, OvenError> {
    let mut records = BTreeMap::new();
    for root_name in ["src", "tests", "crates", "loaves"] {
        let root = project_root.join(root_name);
        if !root.exists() {
            continue;
        }
        collect_compiler_suite_source_tree(&root, &root, &mut records)?;
    }
    let root_build_script = project_root.join("build.rs");
    if root_build_script.exists() {
        let metadata = fs::symlink_metadata(&root_build_script).map_err(|error| OvenError::InvalidGeneratedSource {
            path: root_build_script.clone(),
            message: error.to_string(),
        })?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(OvenError::InvalidGeneratedSource {
                path: root_build_script,
                message: "must be a regular non-symlink file".to_string(),
            });
        }
        records.insert(
            "build.rs".to_string(),
            digest_bytes(
                &fs::read(&root_build_script).map_err(|error| OvenError::InvalidGeneratedSource {
                    path: root_build_script,
                    message: error.to_string(),
                })?,
            ),
        );
    }
    Ok(records)
}

/// Digest the complete source record map without embedding checkout-specific paths in a receipt.
fn digest_compiler_suite_source_records(records: &BTreeMap<String, String>) -> Result<String, OvenError> {
    let payload = serde_json::to_vec(records).map_err(|error| OvenError::Serialize(error.to_string()))?;
    Ok(digest_bytes(&payload))
}

/// Digest only compiler-workspace inputs that can change the reusable target and dependency plan.
///
/// Rust source contents remain exact receipt evidence because direct Rustc compiles the current checkout. Cargo's
/// automatically discovered target topology does depend on source paths, so the complete portable `.rs` path set is
/// retained here. Manifest and build-script bytes remain plan inputs; ordinary module edits do not force the hidden
/// compatibility baker to rebuild an otherwise identical third-party foundation.
fn compiler_suite_plan_digest(
    project_root: &Path,
    source_records: &BTreeMap<String, String>,
) -> Result<String, OvenError> {
    let mut records = BTreeMap::new();
    for (path, digest) in source_records {
        if path.ends_with("Cargo.toml") || path.ends_with("build.rs") {
            records.insert(format!("content:{path}"), digest.clone());
        }
        if path.ends_with(".rs") {
            records.insert(format!("source-path:{path}"), String::new());
        }
    }
    for relative in [".cargo/config.toml", ".cargo/config"] {
        let path = project_root.join(relative);
        if !path.exists() {
            continue;
        }
        let metadata = fs::symlink_metadata(&path).map_err(|error| OvenError::InvalidGeneratedSource {
            path: path.clone(),
            message: error.to_string(),
        })?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(OvenError::InvalidGeneratedSource {
                path,
                message: "must be a regular non-symlink compiler-suite planning input".to_string(),
            });
        }
        records.insert(
            format!("content:{relative}"),
            digest_bytes(&fs::read(&path).map_err(|error| OvenError::InvalidGeneratedSource {
                path: path.clone(),
                message: error.to_string(),
            })?),
        );
    }
    let payload = serde_json::to_vec(&records).map_err(|error| OvenError::Serialize(error.to_string()))?;
    Ok(digest_bytes(&payload))
}

/// Stable receipt key for one direct-rustc compiler-suite target root.
#[must_use]
pub fn compiler_suite_source_evidence_key(relative_path: &str) -> String {
    format!("compiler-suite-source:{relative_path}")
}

/// Recursively collect compiler-suite inputs while excluding known caller-owned or generated output directories.
fn collect_compiler_suite_source_tree(
    root: &Path,
    current: &Path,
    records: &mut BTreeMap<String, String>,
) -> Result<(), OvenError> {
    let metadata = fs::symlink_metadata(current).map_err(|error| OvenError::InvalidGeneratedSource {
        path: current.to_path_buf(),
        message: error.to_string(),
    })?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(OvenError::InvalidGeneratedSource {
            path: current.to_path_buf(),
            message: "must be a directory without symlink indirection".to_string(),
        });
    }
    let mut entries = fs::read_dir(current)
        .map_err(|error| OvenError::InvalidGeneratedSource {
            path: current.to_path_buf(),
            message: error.to_string(),
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| OvenError::InvalidGeneratedSource {
            path: current.to_path_buf(),
            message: error.to_string(),
        })?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if matches!(name.as_ref(), ".incan" | ".ralph-cache" | "target") || name.ends_with(".snap.new") {
            continue;
        }
        let metadata = fs::symlink_metadata(&path).map_err(|error| OvenError::InvalidGeneratedSource {
            path: path.clone(),
            message: error.to_string(),
        })?;
        if metadata.file_type().is_symlink() {
            return Err(OvenError::InvalidGeneratedSource {
                path,
                message: "symlinks are not allowed in a compiler test-suite closure".to_string(),
            });
        }
        if metadata.is_dir() {
            collect_compiler_suite_source_tree(root, &path, records)?;
            continue;
        }
        if !metadata.is_file() {
            return Err(OvenError::InvalidGeneratedSource {
                path,
                message: "may contain only regular files and directories".to_string(),
            });
        }
        let relative = path
            .strip_prefix(root)
            .map_err(|_| OvenError::InvalidGeneratedSource {
                path: path.clone(),
                message: "escaped the declared compiler test-suite root".to_string(),
            })?
            .to_string_lossy()
            .replace('\\', "/");
        let root_name =
            root.file_name()
                .and_then(|name| name.to_str())
                .ok_or_else(|| OvenError::InvalidGeneratedSource {
                    path: root.to_path_buf(),
                    message: "has no UTF-8 root directory name".to_string(),
                })?;
        let key = format!("{root_name}/{relative}");
        let digest = digest_bytes(&fs::read(&path).map_err(|error| OvenError::InvalidGeneratedSource {
            path: path.clone(),
            message: error.to_string(),
        })?);
        if records.insert(key.clone(), digest).is_some() {
            return Err(OvenError::InvalidGeneratedSource {
                path,
                message: format!("duplicate portable compiler test-suite path `{key}`"),
            });
        }
    }
    Ok(())
}

/// Hash a regular-file source closure by sorted portable relative path and exact bytes without capturing its path.
///
/// This is shared by generated-source receipt evidence and the compiler/SDK runtime inputs that select reusable Oven
/// build units across clean worktrees.
pub fn digest_source_tree(root: &Path) -> Result<String, OvenError> {
    let metadata = fs::symlink_metadata(root).map_err(|error| OvenError::InvalidGeneratedSource {
        path: root.to_path_buf(),
        message: error.to_string(),
    })?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(OvenError::InvalidGeneratedSource {
            path: root.to_path_buf(),
            message: "must be a directory without symlink indirection".to_string(),
        });
    }
    let mut records = BTreeMap::new();
    collect_generated_source_tree(root, root, &mut records)?;
    if records.is_empty() {
        return Err(OvenError::InvalidGeneratedSource {
            path: root.to_path_buf(),
            message: "must contain at least one regular file".to_string(),
        });
    }
    let payload = serde_json::to_vec(&records).map_err(|error| OvenError::Serialize(error.to_string()))?;
    Ok(digest_bytes(&payload))
}

/// Hash authored project inputs while excluding compiler- and tool-owned mutable output trees.
///
/// This intentionally differs from [`digest_source_tree`], whose callers supply an exact generated-source closure
/// and therefore need every regular file represented. A path dependency instead names an authored project root;
/// its `.incan`, `.ralph-cache`, `target`, and `.git` directories are not inputs to the dependency's semantics.
/// Excluding them makes the identity stable across valid local reuse without overlooking any authored file outside
/// those reserved output locations.
pub fn digest_project_source_tree(root: &Path) -> Result<String, OvenError> {
    let metadata = fs::symlink_metadata(root).map_err(|error| OvenError::InvalidProjectSource {
        path: root.to_path_buf(),
        message: error.to_string(),
    })?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(OvenError::InvalidProjectSource {
            path: root.to_path_buf(),
            message: "must be a directory without symlink indirection".to_string(),
        });
    }
    let mut records = BTreeMap::new();
    collect_project_source_tree(root, root, &mut records)?;
    if records.is_empty() {
        return Err(OvenError::InvalidProjectSource {
            path: root.to_path_buf(),
            message: "must contain at least one authored regular file".to_string(),
        });
    }
    let payload = serde_json::to_vec(&records).map_err(|error| OvenError::Serialize(error.to_string()))?;
    Ok(digest_bytes(&payload))
}

/// Whether a directory is compiler, VCS, or test-runner output rather than authored dependency source.
fn is_mutable_project_output_directory(path: &Path) -> bool {
    matches!(
        path.file_name().and_then(|name| name.to_str()),
        Some(".git" | ".incan" | ".ralph-cache" | "target")
    )
}

/// Collect authored project files without descending into reserved mutable output directories.
fn collect_project_source_tree(
    root: &Path,
    current: &Path,
    records: &mut BTreeMap<String, String>,
) -> Result<(), OvenError> {
    let mut entries = fs::read_dir(current)
        .map_err(|error| OvenError::InvalidProjectSource {
            path: current.to_path_buf(),
            message: error.to_string(),
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| OvenError::InvalidProjectSource {
            path: current.to_path_buf(),
            message: error.to_string(),
        })?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path).map_err(|error| OvenError::InvalidProjectSource {
            path: path.clone(),
            message: error.to_string(),
        })?;
        if metadata.is_dir() && is_mutable_project_output_directory(&path) {
            continue;
        }
        if metadata.file_type().is_symlink() {
            return Err(OvenError::InvalidProjectSource {
                path,
                message: "symlinks are not allowed in an authored project source closure".to_string(),
            });
        }
        if metadata.is_dir() {
            collect_project_source_tree(root, &path, records)?;
            continue;
        }
        if !metadata.is_file() {
            return Err(OvenError::InvalidProjectSource {
                path,
                message: "may contain only regular files and directories".to_string(),
            });
        }
        let relative = path
            .strip_prefix(root)
            .map_err(|_| OvenError::InvalidProjectSource {
                path: path.clone(),
                message: "escaped the declared project source root".to_string(),
            })?
            .to_string_lossy()
            .replace('\\', "/");
        let digest =
            fs::read(&path)
                .map(|bytes| digest_bytes(&bytes))
                .map_err(|error| OvenError::InvalidProjectSource {
                    path: path.clone(),
                    message: error.to_string(),
                })?;
        if records.insert(relative.clone(), digest).is_some() {
            return Err(OvenError::InvalidProjectSource {
                path,
                message: format!("duplicate portable source path `{relative}`"),
            });
        }
    }
    Ok(())
}

/// Recursively collect one generated source tree with sorted portable paths and no link traversal.
fn collect_generated_source_tree(
    root: &Path,
    current: &Path,
    records: &mut BTreeMap<String, String>,
) -> Result<(), OvenError> {
    let mut entries = fs::read_dir(current)
        .map_err(|error| OvenError::InvalidGeneratedSource {
            path: current.to_path_buf(),
            message: error.to_string(),
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| OvenError::InvalidGeneratedSource {
            path: current.to_path_buf(),
            message: error.to_string(),
        })?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path).map_err(|error| OvenError::InvalidGeneratedSource {
            path: path.clone(),
            message: error.to_string(),
        })?;
        if metadata.file_type().is_symlink() {
            return Err(OvenError::InvalidGeneratedSource {
                path,
                message: "symlinks are not allowed in a generated source closure".to_string(),
            });
        }
        if metadata.is_dir() {
            collect_generated_source_tree(root, &path, records)?;
            continue;
        }
        if !metadata.is_file() {
            return Err(OvenError::InvalidGeneratedSource {
                path,
                message: "may contain only regular files and directories".to_string(),
            });
        }
        let relative = path
            .strip_prefix(root)
            .map_err(|_| OvenError::InvalidGeneratedSource {
                path: path.clone(),
                message: "escaped the declared generated source root".to_string(),
            })?
            .to_string_lossy()
            .replace('\\', "/");
        let digest =
            fs::read(&path)
                .map(|bytes| digest_bytes(&bytes))
                .map_err(|error| OvenError::InvalidGeneratedSource {
                    path: path.clone(),
                    message: error.to_string(),
                })?;
        if records.insert(relative.clone(), digest).is_some() {
            return Err(OvenError::InvalidGeneratedSource {
                path,
                message: format!("duplicate portable source path `{relative}`"),
            });
        }
    }
    Ok(())
}

/// Normalize a required identity field and reject blank values that collapse distinct build units.
fn normalized_value(value: &str, field: &'static str) -> Result<String, OvenError> {
    let normalized = value.trim();
    if normalized.is_empty() {
        return Err(OvenError::EmptyBuildIntent { field });
    }
    Ok(normalized.to_string())
}

/// Hash the portable receipt input while excluding checkout and cache paths.
pub fn receipt_identity(
    project: &OvenProjectIdentity,
    sources: &OvenSourceEvidence,
    intent: &OvenBuildIntent,
    compatibility: &OvenCompatibility,
) -> Result<String, OvenError> {
    let input = ReceiptIdentityInput {
        schema_version: OVEN_RECEIPT_SCHEMA_VERSION,
        project,
        sources,
        intent,
        compatibility,
    };
    let serialized = serde_json::to_vec(&input).map_err(|error| OvenError::Serialize(error.to_string()))?;
    Ok(digest_bytes(&serialized))
}

/// Hash only portable inputs that decide whether a reusable native closure is compatible.
///
/// Project source and generated output deliberately do not enter this identity: they are authorized by the full
/// command receipt and may vary across clean worktrees while selecting one compatible Oven closure.
pub fn build_unit_identity(
    intent: &OvenBuildIntent,
    compatibility: &OvenCompatibility,
    inputs: &BTreeMap<String, String>,
) -> Result<String, OvenError> {
    let input = BuildUnitIdentityInput {
        schema_version: OVEN_RECEIPT_SCHEMA_VERSION,
        intent,
        compatibility,
        inputs,
    };
    let serialized = serde_json::to_vec(&input).map_err(|error| OvenError::Serialize(error.to_string()))?;
    Ok(digest_bytes(&serialized))
}

/// Canonicalize line endings and final newlines before textual content enters a receipt.
fn normalize_content(content: &str) -> String {
    let mut normalized = content.replace("\r\n", "\n");
    if !normalized.ends_with('\n') {
        normalized.push('\n');
    }
    normalized
}

pub use oven_model::digest::{digest_bytes, digest_content};

/// Write, sync, and atomically replace a receipt from a same-directory staged file.
pub fn write_receipt_staged(payload: &[u8], staged_path: &Path, path: &Path, parent: &Path) -> io::Result<()> {
    let mut staged = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(staged_path)?;
    staged.write_all(payload)?;
    staged.write_all(b"\n")?;
    staged.sync_all()?;
    fs::rename(staged_path, path)?;
    File::open(parent)?.sync_all()
}

/// Canonical receipt fields used only for content-addressed identity serialization.
#[derive(Serialize)]
struct ReceiptIdentityInput<'a> {
    schema_version: u32,
    project: &'a OvenProjectIdentity,
    sources: &'a OvenSourceEvidence,
    intent: &'a OvenBuildIntent,
    compatibility: &'a OvenCompatibility,
}

/// Canonical reusable-native-closure fields used only for build-unit selection.
#[derive(Serialize)]
struct BuildUnitIdentityInput<'a> {
    schema_version: u32,
    intent: &'a OvenBuildIntent,
    compatibility: &'a OvenCompatibility,
    inputs: &'a BTreeMap<String, String>,
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use oven_model::manifest::{DependencySource, DependencySpec};

    use super::{
        OvenCompilerSuiteRequest, OvenGeneratedProjectRequest, OvenImportRequest, OvenProviderHookError,
        OvenProviderHooks, OvenReceipt, default_receipt_path, digest_bytes, generated_project_source_evidence,
        import_frozen_project, receipt_generated_project, receipt_generated_project_with_source_evidence,
        receipt_native_compiler_suite, receipt_with_build_unit_input, receipt_without_build_unit_input, write_receipt,
    };

    /// A hook that fails the way a compiler would, with its own typed error behind the hook error.
    struct FailingProviderHooks;

    #[derive(Debug, thiserror::Error)]
    #[error("the compiler could not read the manifest")]
    struct CompilerSideFailure;

    impl super::OvenProviderHooks for FailingProviderHooks {
        fn sdk_provider_root(&self, _explicit_inventory: Option<&Path>) -> Result<PathBuf, OvenProviderHookError> {
            Err(OvenProviderHookError::InventoryDiscovery {
                source: Box::new(CompilerSideFailure),
            })
        }

        fn sdk_inventory_file(&self) -> &'static str {
            "sdk-inventory.json"
        }

        fn refresh_staged_sdk_provider_digests(&self, _provider_root: &Path) -> Result<(), OvenProviderHookError> {
            Ok(())
        }

        fn packaged_provider_digest(&self, dependency_root: &Path) -> Option<Result<String, OvenProviderHookError>> {
            Some(Err(OvenProviderHookError::PackagedDigest {
                dependency_root: dependency_root.to_path_buf(),
                source: Box::new(CompilerSideFailure),
            }))
        }
    }

    #[test]
    fn provider_hook_errors_keep_the_compiler_failure_as_their_source() -> Result<(), Box<dyn std::error::Error>> {
        use std::error::Error as _;

        let Err(error) = FailingProviderHooks.sdk_provider_root(None) else {
            return Err("discovery failure was not reported".into());
        };
        assert!(
            error
                .to_string()
                .starts_with("failed to discover active SDK provider inventory")
        );
        assert!(error.source().is_some_and(|source| source.is::<CompilerSideFailure>()));

        let Err(error) = super::NoProviderHooks.sdk_provider_root(None) else {
            return Err("the compiler-less hooks reported an inventory".into());
        };
        assert!(matches!(error, OvenProviderHookError::InventoryUnavailable { .. }));
        assert!(error.source().is_none());

        let dependency = DependencySpec {
            crate_name: "packaged_provider".to_string(),
            version: None,
            features: Vec::new(),
            default_features: true,
            source: DependencySource::Path {
                path: PathBuf::from("packaged-provider"),
            },
            optional: false,
            package: None,
        };
        let Err(error) = super::digest_dependency_specs(std::slice::from_ref(&dependency), &FailingProviderHooks)
        else {
            return Err("a packaged provider that cannot be digested was accepted".into());
        };
        let super::OvenError::ProviderHook { path, source } = error else {
            return Err(format!("unexpected error category: {error}").into());
        };
        assert_eq!(path, PathBuf::from("packaged-provider"));
        assert!(source.source().is_some_and(|source| source.is::<CompilerSideFailure>()));
        Ok(())
    }

    #[test]
    fn receipt_identity_is_portable_and_observes_explicit_build_inputs() -> Result<(), Box<dyn std::error::Error>> {
        let first = tempfile::tempdir()?;
        let second = tempfile::tempdir()?;
        write_frozen_project(first.path())?;
        write_frozen_project(second.path())?;

        let first_receipt = import_frozen_project(&request(first.path()))?;
        let second_receipt = import_frozen_project(&request(second.path()))?;
        let changed = import_frozen_project(&OvenImportRequest::new(
            second.path(),
            "x86_64-unknown-linux-gnu",
            "rustc 1.96.0",
            "release",
            vec!["serde".to_string()],
        ))?;

        assert_eq!(first_receipt.identity, second_receipt.identity);
        assert_ne!(first_receipt.identity, changed.identity);
        assert!(first_receipt.compatibility.cargo_input_only);
        Ok(())
    }

    #[test]
    fn supplemental_source_evidence_changes_identity_without_recording_paths() -> Result<(), Box<dyn std::error::Error>>
    {
        let project = tempfile::tempdir()?;
        write_frozen_project(project.path())?;
        let first = import_frozen_project(
            &request(project.path()).with_supplemental_source_digest("generated-test-harness", "sha256:first"),
        )?;
        let second = import_frozen_project(
            &request(project.path()).with_supplemental_source_digest("generated-test-harness", "sha256:second"),
        )?;

        assert_ne!(first.identity, second.identity);
        assert_eq!(first.sources.supplemental_digests.len(), 1);
        Ok(())
    }

    #[test]
    fn path_dependency_identity_ignores_mutable_project_output_but_tracks_authored_files()
    -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        fs::create_dir_all(project.path().join("src"))?;
        fs::write(
            project.path().join("Cargo.toml"),
            "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\n",
        )?;
        fs::write(project.path().join("src/lib.rs"), "pub fn value() -> i32 { 1 }\n")?;
        let dependency = DependencySpec {
            crate_name: "fixture".to_string(),
            version: None,
            features: Vec::new(),
            default_features: true,
            source: DependencySource::Path {
                path: project.path().to_path_buf(),
            },
            optional: false,
            package: None,
        };
        let initial = super::digest_dependency_specs(std::slice::from_ref(&dependency), &super::NoProviderHooks)?;

        fs::write(
            project.path().join("target"),
            "an authored file, not an output directory",
        )?;
        assert_ne!(
            initial,
            super::digest_dependency_specs(std::slice::from_ref(&dependency), &super::NoProviderHooks)?
        );
        fs::remove_file(project.path().join("target"))?;

        for directory in [".git", ".incan/oven", ".ralph-cache/loafs", "target/debug"] {
            fs::create_dir_all(project.path().join(directory))?;
            fs::write(project.path().join(directory).join("mutable"), "not authored")?;
        }
        assert_eq!(
            initial,
            super::digest_dependency_specs(std::slice::from_ref(&dependency), &super::NoProviderHooks)?
        );

        fs::write(project.path().join("native-schema.json"), "{\"version\": 1}\n")?;
        assert_ne!(
            initial,
            super::digest_dependency_specs(std::slice::from_ref(&dependency), &super::NoProviderHooks)?
        );
        fs::remove_file(project.path().join("native-schema.json"))?;

        fs::write(project.path().join("src/lib.rs"), "pub fn value() -> i32 { 2 }\n")?;
        assert_ne!(
            initial,
            super::digest_dependency_specs(&[dependency], &super::NoProviderHooks)?
        );
        Ok(())
    }

    #[test]
    fn path_dependency_identity_tracks_recursive_sibling_path_source() -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let first = temp.path().join("source-a");
        let second = temp.path().join("source-b");
        let write_packages = |workspace: &Path| -> Result<(), Box<dyn std::error::Error>> {
            let foo = workspace.join("foo");
            let bar = workspace.join("bar");
            for package in [&foo, &bar] {
                fs::create_dir_all(package.join("src"))?;
            }
            fs::write(
                foo.join("Cargo.toml"),
                "[package]\nname = \"foo\"\nversion = \"0.1.0\"\n\n[dependencies]\nbar = { path = \"../bar\" }\n",
            )?;
            fs::write(foo.join("src/lib.rs"), "pub fn value() -> i32 { bar::value() }\n")?;
            fs::write(
                bar.join("Cargo.toml"),
                "[package]\nname = \"bar\"\nversion = \"0.1.0\"\n",
            )?;
            fs::write(bar.join("src/lib.rs"), "pub fn value() -> i32 { 1 }\n")?;
            fs::write(bar.join("native-schema.json"), "{\"version\": 1}\n")?;
            Ok(())
        };
        write_packages(&first)?;
        write_packages(&second)?;
        let dependency = |workspace: &Path| DependencySpec {
            crate_name: "foo".to_string(),
            version: None,
            features: Vec::new(),
            default_features: true,
            source: DependencySource::Path {
                path: workspace.join("foo"),
            },
            optional: false,
            package: None,
        };
        let first_dependency = dependency(&first);
        let second_dependency = dependency(&second);
        let stable = super::digest_dependency_specs(std::slice::from_ref(&first_dependency), &super::NoProviderHooks)?;
        assert_eq!(
            stable,
            super::digest_dependency_specs(std::slice::from_ref(&second_dependency), &super::NoProviderHooks)?
        );

        let second_bar = second.join("bar");
        fs::write(second_bar.join("native-schema.json"), "{\"version\": 2}\n")?;
        let source_changed =
            super::digest_dependency_specs(std::slice::from_ref(&second_dependency), &super::NoProviderHooks)?;
        assert_ne!(stable, source_changed);

        fs::create_dir_all(second_bar.join("target/debug"))?;
        fs::write(second_bar.join("target/debug/cache"), "mutable output")?;
        assert_eq!(
            source_changed,
            super::digest_dependency_specs(std::slice::from_ref(&second_dependency), &super::NoProviderHooks)?
        );
        Ok(())
    }

    #[test]
    fn generated_project_receipt_needs_no_cargo_input_and_is_portable() -> Result<(), Box<dyn std::error::Error>> {
        let first = tempfile::tempdir()?;
        let second = tempfile::tempdir()?;
        write_generated_source_closure(first.path(), "fn main() { println!(\"oven\"); }\n")?;
        write_generated_source_closure(second.path(), "fn main() { println!(\"oven\"); }\n")?;

        let first_receipt = receipt_generated_project(&generated_request(first.path()))?;
        let second_receipt = receipt_generated_project(&generated_request(second.path()))?;

        assert_eq!(first_receipt.identity, second_receipt.identity);
        assert!(!first_receipt.compatibility.cargo_input_only);
        assert!(first_receipt.sources.cargo_manifest_digest.is_none());
        assert!(first_receipt.sources.cargo_lock_digest.is_none());

        fs::write(
            first.path().join("src/main.rs"),
            "fn main() { println!(\"changed\"); }\n",
        )?;
        let changed = receipt_generated_project(&generated_request(first.path()))?;
        assert_ne!(first_receipt.identity, changed.identity);
        assert_eq!(first_receipt.build_unit_identity, changed.build_unit_identity);

        let runtime_changed = receipt_generated_project(
            &generated_request(second.path()).with_build_unit_input("runtime-lock", "sha256:changed"),
        )?;
        assert_ne!(first_receipt.build_unit_identity, runtime_changed.build_unit_identity);
        Ok(())
    }

    #[test]
    fn generated_project_receipts_reuse_one_verified_source_closure_across_profiles()
    -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        write_generated_source_closure(project.path(), "fn main() { println!(\"oven\"); }\n")?;
        let release_request = generated_request(project.path());
        let debug_request = OvenGeneratedProjectRequest::new(
            project.path(),
            "generated_fixture",
            "0.1.0",
            "aarch64-apple-darwin",
            "rustc 1.96.0",
            "debug",
            vec!["default".to_string()],
        )
        .with_generated_source("generated-rust-main", project.path().join("src/main.rs"))
        .with_generated_source_tree("generated-rust-tree", project.path().join("src"));

        let source_evidence = generated_project_source_evidence(&release_request)?;
        let reused_release = receipt_generated_project_with_source_evidence(&release_request, &source_evidence)?;
        let reused_debug = receipt_generated_project_with_source_evidence(&debug_request, &source_evidence)?;

        assert_eq!(reused_release, receipt_generated_project(&release_request)?);
        assert_eq!(reused_debug, receipt_generated_project(&debug_request)?);
        assert_ne!(reused_release.identity, reused_debug.identity);

        let mismatched_request = OvenGeneratedProjectRequest::new(
            project.path(),
            "generated_fixture",
            "0.1.0",
            "aarch64-apple-darwin",
            "rustc 1.96.0",
            "debug",
            vec!["default".to_string()],
        )
        .with_generated_source("different-generated-root", project.path().join("src/main.rs"))
        .with_generated_source_tree("generated-rust-tree", project.path().join("src"));
        let error = match receipt_generated_project_with_source_evidence(&mismatched_request, &source_evidence) {
            Ok(_) => return Err("mismatched source evidence must not authorize another request".into()),
            Err(error) => error,
        };
        assert!(error.to_string().contains("does not match"));
        Ok(())
    }

    #[test]
    fn publisher_base_receipt_removes_only_the_requested_interop_selection_input()
    -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        write_generated_source_closure(project.path(), "fn main() {}\n")?;
        let receipt = receipt_generated_project(
            &generated_request(project.path())
                .with_build_unit_input("runtime-lock", "sha256:runtime")
                .with_build_unit_input("oven-interop-execution-receipt", "sha256:interop"),
        )?;
        let base = receipt_without_build_unit_input(&receipt, "oven-interop-execution-receipt")?;
        assert_ne!(base.identity, receipt.identity);
        assert_ne!(base.build_unit_identity, receipt.build_unit_identity);
        assert_eq!(
            base.sources.build_unit_inputs.get("runtime-lock"),
            Some(&"sha256:runtime".to_string())
        );
        assert!(
            !base
                .sources
                .build_unit_inputs
                .contains_key("oven-interop-execution-receipt")
        );
        base.verify_identity()?;
        assert!(receipt_without_build_unit_input(&base, "oven-interop-execution-receipt").is_err());
        let reselected = receipt_with_build_unit_input(&base, "oven-interop-execution-receipt", "sha256:reselected")?;
        assert_ne!(reselected.identity, base.identity);
        assert_eq!(
            reselected
                .sources
                .build_unit_inputs
                .get("oven-interop-execution-receipt")
                .map(String::as_str),
            Some("sha256:reselected")
        );
        reselected.verify_identity()?;
        Ok(())
    }

    #[test]
    fn compiler_support_final_receipt_adds_only_its_sealed_root_intent() -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        write_generated_source_closure(project.path(), "fn main() {}\n")?;
        let capture = receipt_generated_project(
            &generated_request(project.path()).with_build_unit_input("runtime-lock", "sha256:runtime"),
        )?;
        let final_receipt = receipt_with_compiler_support_root_intent(&capture, "sha256:compiler-root-intent")?;

        assert_ne!(final_receipt.identity, capture.identity);
        assert_ne!(final_receipt.build_unit_identity, capture.build_unit_identity);
        assert_eq!(final_receipt.project, capture.project);
        assert_eq!(final_receipt.intent, capture.intent);
        assert_eq!(final_receipt.compatibility, capture.compatibility);
        assert_eq!(
            final_receipt.sources.supplemental_digests,
            capture.sources.supplemental_digests
        );
        assert_eq!(
            final_receipt.sources.build_unit_inputs.get("runtime-lock"),
            Some(&"sha256:runtime".to_string())
        );
        assert_eq!(
            final_receipt
                .sources
                .build_unit_inputs
                .get(OVEN_COMPILER_SUPPORT_ROOT_INTENT_BUILD_UNIT_INPUT),
            Some(&"sha256:compiler-root-intent".to_string())
        );
        final_receipt.verify_identity()?;
        assert!(matches!(
            receipt_with_compiler_support_root_intent(&final_receipt, "sha256:other"),
            Err(OvenError::ExistingBuildUnitInput { .. })
        ));

        let mut tampered_capture = capture;
        tampered_capture
            .sources
            .build_unit_inputs
            .insert("unsealed-change".to_string(), "sha256:changed".to_string());
        assert!(matches!(
            receipt_with_compiler_support_root_intent(&tampered_capture, "sha256:other"),
            Err(OvenError::ReceiptIdentityMismatch { .. }) | Err(OvenError::BuildUnitIdentityMismatch { .. })
        ));
        Ok(())
    }

    #[test]
    fn generated_project_receipt_rejects_symlinked_source_closure() -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        write_generated_source_closure(project.path(), "fn main() {}\n")?;
        #[cfg(unix)]
        std::os::unix::fs::symlink("main.rs", project.path().join("src/linked.rs"))?;
        #[cfg(unix)]
        {
            let error = receipt_generated_project(&generated_request(project.path()))
                .err()
                .ok_or("symlinked generated source must be rejected")?;
            assert!(error.to_string().contains("symlinks are not allowed"));
        }
        Ok(())
    }

    #[test]
    fn compiler_suite_source_content_changes_reuse_its_native_foundation() -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        write_frozen_project(project.path())?;
        fs::create_dir_all(project.path().join("src"))?;
        fs::write(project.path().join("src/lib.rs"), "pub fn stable() {}\n")?;
        fs::write(project.path().join("src/main.rs"), "fn main() {}\n")?;
        let request = || {
            OvenCompilerSuiteRequest::new(
                project.path(),
                "frozen-suite",
                "aarch64-apple-darwin",
                "rustc 1.96.0",
                "debug",
                Vec::new(),
            )
        };

        let first = receipt_native_compiler_suite(&request())?;
        fs::write(project.path().join("src/lib.rs"), "pub fn changed() {}\n")?;
        let changed = receipt_native_compiler_suite(&request())?;

        assert_ne!(first.identity, changed.identity);
        assert_eq!(first.build_unit_identity, changed.build_unit_identity);

        fs::write(project.path().join("src/new_target_input.rs"), "pub fn added() {}\n")?;
        let topology_changed = receipt_native_compiler_suite(&request())?;
        assert_ne!(changed.build_unit_identity, topology_changed.build_unit_identity);

        let first_loaf_compatibility = receipt_native_compiler_suite(
            &request().with_loaf_compatibility_identity(digest_bytes(b"compiler-suite-members-one")),
        )?;
        let next_loaf_compatibility = receipt_native_compiler_suite(
            &request().with_loaf_compatibility_identity(digest_bytes(b"compiler-suite-members-two")),
        )?;
        assert_ne!(
            first_loaf_compatibility.build_unit_identity, next_loaf_compatibility.build_unit_identity,
            "a changed compatible Loaf member set must invalidate the suite and its toolchain-data partitions"
        );
        Ok(())
    }

    #[test]
    fn import_rejects_virtual_or_unlocked_cargo_inputs() -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        fs::write(project.path().join("Cargo.toml"), "[workspace]\nmembers = []\n")?;
        fs::write(project.path().join("Cargo.lock"), "version = 4\n")?;
        let virtual_error = import_frozen_project(&request(project.path()))
            .err()
            .ok_or("virtual workspace must be a compatibility miss")?;
        assert!(virtual_error.to_string().contains("virtual workspaces"));

        fs::write(
            project.path().join("Cargo.toml"),
            "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\n",
        )?;
        fs::remove_file(project.path().join("Cargo.lock"))?;
        let lock_error = import_frozen_project(&request(project.path()))
            .err()
            .ok_or("missing lock must be a compatibility miss")?;
        assert!(lock_error.to_string().contains("Cargo.lock"));
        Ok(())
    }

    #[test]
    fn receipt_publication_is_complete_json_at_the_default_project_path() -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        write_frozen_project(project.path())?;
        let receipt = import_frozen_project(&request(project.path()))?;
        let path = default_receipt_path(project.path());
        write_receipt(&receipt, &path)?;

        let payload = fs::read_to_string(path)?;
        let decoded: OvenReceipt = serde_json::from_str(&payload)?;
        assert_eq!(decoded, receipt);
        Ok(())
    }

    fn request(project_root: &Path) -> OvenImportRequest {
        OvenImportRequest::new(
            project_root,
            "aarch64-apple-darwin",
            "rustc 1.96.0",
            "release",
            vec!["serde".to_string()],
        )
    }

    fn write_frozen_project(root: &Path) -> Result<(), std::io::Error> {
        fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"oven_fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )?;
        fs::write(root.join("Cargo.lock"), "version = 4\n")?;
        fs::write(
            root.join("loaf.toml"),
            "[project]\nname = \"oven_fixture\"\nversion = \"0.1.0\"\n",
        )?;
        Ok(())
    }

    fn generated_request(project_root: &Path) -> OvenGeneratedProjectRequest {
        OvenGeneratedProjectRequest::new(
            project_root,
            "generated_fixture",
            "0.1.0",
            "aarch64-apple-darwin",
            "rustc 1.96.0",
            "release",
            vec!["default".to_string()],
        )
        .with_generated_source("generated-rust-main", project_root.join("src/main.rs"))
        .with_generated_source_tree("generated-rust-tree", project_root.join("src"))
    }

    fn write_generated_source_closure(root: &Path, main: &str) -> Result<(), std::io::Error> {
        fs::create_dir_all(root.join("src/nested"))?;
        fs::write(root.join("src/main.rs"), main)?;
        fs::write(root.join("src/nested/mod.rs"), "pub fn helper() {}\n")
    }
}
