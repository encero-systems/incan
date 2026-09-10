//! Incan-owned Oven receipts for the supported Alpha compatibility envelope.
//!
//! This small compiler-owned Rust kernel is temporary implementation debt scoped to the tracked Oven Alpha (#1005,
//! #975): the present language cannot own the required process, file-lock, and durable-publication primitives
//! directly. It must remain a narrow, removable boundary rather than growing into a Rust orchestration layer for
//! the product workflow.
//!
//! Receipts and executors consume checked identities and selected native inputs. Cargo publication, frozen-project
//! adoption and its plan producers have been removed.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{self, ErrorKind, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::manifest::{DependencySource, DependencySpec, GitReference};

pub(crate) mod compiler_suite_env;
pub(crate) mod interop;
pub mod loaf;
pub mod native_contract;
pub mod native_test;
pub(crate) mod process;
pub mod progress;
pub mod rustc;
pub mod store;

/// Digest the portable dependency facts that select a native Oven closure.
///
/// Registry and Git specifications are represented by their declared immutable selection facts. Local Rust packages
/// require a checked Oven source-unit identity and are rejected here until that identity has been projected; a Cargo
/// path and an ad hoc filesystem digest are not valid native authority.
pub fn digest_dependency_specs(dependencies: &[DependencySpec]) -> Result<String, OvenError> {
    let mut records = Vec::with_capacity(dependencies.len());
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
            DependencySource::Path { path } => {
                return Err(OvenError::InvalidProjectSource {
                    path: path.clone(),
                    message: format!(
                        "Rust path dependency `{}` has no checked Oven source-unit identity; local Rust packages must be admitted as Loaf units",
                        dependency.crate_name
                    ),
                });
            }
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

/// Current wire format for persisted Oven receipts.
pub const OVEN_RECEIPT_SCHEMA_VERSION: u32 = 3;
/// Compiler-owned, project-relative destination for a default Oven receipt.
pub const DEFAULT_RECEIPT_RELATIVE_PATH: &str = ".incan/oven/receipt.json";

/// Default aggregate physical allocation retained by an everyday Alpha Oven store.
///
/// A project bake retains independent debug and release plans. A measured IncQL/DataFusion provider retains about
/// 4.23 GiB while its consumer's compatibility publisher transiently needs about 3.80 GiB. Nine GiB admits that
/// ordinary provider-to-consumer hand-off with practical headroom while keeping the publisher's private target
/// bounded.
pub const DEFAULT_OVEN_MAX_PHYSICAL_BYTES: u64 = 9 * 1024 * 1024 * 1024;
/// Default physical allocation cap for one compatibility domain.
///
/// A checked IncQL/DataFusion debug plan retains 1.21 GiB while its following release publisher needs a bounded
/// transient closure. Six GiB covers that serialized two-profile hand-off with practical headroom; callers may
/// still choose a stricter explicit limit.
pub const DEFAULT_OVEN_MAX_DOMAIN_PHYSICAL_BYTES: u64 = 6 * 1024 * 1024 * 1024;
/// Default logical artifact-byte cap for one compatibility domain.
///
/// One explicit bake of a project whose closure is not loadable as independently compiled parts retains two
/// extensions of a compiler Loaf: the library's delta and the test-dependency envelope's, each carrying the unified
/// closure, its re-rooted copies of shared units, and the extension's own runtime. Measured for IncQL/DataFusion on
/// Linux, each is 1.5 GiB, so a single debug-profile bake retains 3.0 GiB before its outputs and authority. Six GiB,
/// the same as the physical allowance, admits that bake with the release profile or a second project beside it;
/// callers may still choose a stricter explicit limit.
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
/// The complete LSP closure measures 3,271,283,026 logical bytes on Linux;
/// 4 GiB leaves practical policy headroom without relaxing its physical bound.
pub const DEFAULT_OVEN_COMPILER_SUITE_MAX_DOMAIN_LOGICAL_BYTES: u64 = 4 * 1024 * 1024 * 1024;

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
pub(crate) struct OvenGeneratedProjectSourceEvidence {
    files: BTreeMap<String, PathBuf>,
    trees: BTreeMap<String, PathBuf>,
    supplemental_digests: BTreeMap<String, String>,
    tree_members: BTreeMap<String, BTreeMap<String, String>>,
}

/// Compiler-owned request for the repository's Rust libtest suite receipt.
///
/// This is deliberately distinct from an arbitrary frozen Cargo package: it records the compiler source closure and
/// Cargo declarations as evidence for a bounded repository-suite publisher, then lets the normal consumer select and
/// compile through direct rustc. It does not make Cargo a normal test executor.
#[derive(Debug, Clone)]
pub struct OvenCompilerSuiteRequest {
    project_root: PathBuf,
    project: OvenProjectIdentity,
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
    /// Construct a request for the root compiler library's direct-rustc libtest compatibility unit.
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

/// Stable package identity supplied by the selected project declaration.
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

/// Typed failure while validating selected Oven inputs or atomically publishing a receipt.
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
    /// Native execution was requested without a compatible already-selected dependency plan.
    ///
    /// This is a terminal input refusal. The physical executor cannot discover a graph or prepare dependencies;
    /// the Incan Oven control plane must supply the selected closure before execution can proceed.
    #[error(
        "Oven selected native plan unavailable for build unit {build_unit_identity}; execution requires a compatible dependency closure"
    )]
    SelectedNativePlanUnavailable { build_unit_identity: String },
    /// A requested receipt transformation named a build-unit input that was not present.
    #[error("Oven receipt has no build-unit input `{input}`")]
    MissingBuildUnitInput { input: String },
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
/// with the same normalized source-evidence keys, input kinds and paths, so a debug or release intent cannot silently
/// borrow unrelated generated inputs.
pub(crate) fn generated_project_source_evidence(
    request: &OvenGeneratedProjectRequest,
) -> Result<OvenGeneratedProjectSourceEvidence, OvenError> {
    generated_source_evidence_for_inputs(&request.generated_sources, &request.generated_source_trees).map_err(|error| {
        match error {
            OvenError::InvalidGeneratedSource { path, message } if path.as_os_str().is_empty() => {
                OvenError::InvalidGeneratedSource {
                    path: request.project_root.clone(),
                    message,
                }
            }
            other => other,
        }
    })
}

/// Capture explicit generated inputs without requiring a native build intent.
pub(crate) fn generated_source_evidence_for_inputs(
    files: &BTreeMap<String, PathBuf>,
    trees: &BTreeMap<String, PathBuf>,
) -> Result<OvenGeneratedProjectSourceEvidence, OvenError> {
    let files = normalized_generated_source_bindings(files)?;
    let trees = normalized_generated_source_bindings(trees)?;
    let (supplemental_digests, tree_members) = generated_source_evidence(&files, &trees)?;
    Ok(OvenGeneratedProjectSourceEvidence {
        files,
        trees,
        supplemental_digests,
        tree_members,
    })
}

impl OvenGeneratedProjectSourceEvidence {
    /// Read a verified named digest without exposing mutable proof records.
    pub(crate) fn digest(&self, name: &str) -> Option<&str> {
        self.supplemental_digests.get(name).map(String::as_str)
    }

    /// Borrow one verified generated tree's logical member-to-byte-digest projection.
    ///
    /// These records were collected while deriving the receipt's aggregate tree digest. Returning the same map lets
    /// the declaring compilation publish member-granular JEC evidence without a second filesystem traversal.
    pub(crate) fn tree_members(&self, name: &str) -> Option<&BTreeMap<String, String>> {
        self.tree_members.get(name)
    }
}

/// Receipt a generated project with a previously verified source closure.
///
/// The caller may vary build intent, including debug versus release profile, but the request must retain exactly the
/// source-evidence keys, input kinds and paths that produced `source_evidence`. The caller must keep these generated
/// inputs unchanged until receipt construction finishes. This is an in-process reuse boundary, not a persisted
/// cache: each returned receipt still carries complete content-derived source evidence and verifies normally.
pub(crate) fn receipt_generated_project_with_source_evidence(
    request: &OvenGeneratedProjectRequest,
    source_evidence: &OvenGeneratedProjectSourceEvidence,
) -> Result<OvenReceipt, OvenError> {
    if source_evidence.files != normalized_generated_source_bindings(&request.generated_sources)?
        || source_evidence.trees != normalized_generated_source_bindings(&request.generated_source_trees)?
    {
        return Err(OvenError::InvalidGeneratedSource {
            path: request.project_root.clone(),
            message: "reused source evidence does not match the request's generated source bindings".to_string(),
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
pub(crate) fn receipt_with_build_unit_input(
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

/// Derive a new complete receipt with one selected build-unit input removed.
///
/// This is the inverse of [`receipt_with_build_unit_input`] for the narrow cases where a compiler-owned capability
/// must be selected independently from a project-only input. The returned receipt is independently identity-checked;
/// callers must still prove that any selected immutable artifact provides the capability they need.
pub(crate) fn receipt_without_build_unit_input(receipt: &OvenReceipt, input: &str) -> Result<OvenReceipt, OvenError> {
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
    let project = request.project.clone();
    let lib_root = request.project_root.join("src/lib.rs");
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
            "compiler-libtest-root".to_string(),
            digest_generated_source_file(&lib_root)?,
        ),
        (
            "compiler-cli-root".to_string(),
            digest_generated_source_file(&request.project_root.join("src/main.rs"))?,
        ),
        ("compiler-suite-source-tree".to_string(), compiler_source_tree_digest),
    ]);
    // A full native-suite plan must authorize each root passed to direct rustc, not just `src/lib.rs`. Source bytes
    // belong to the exact command receipt, while the reusable build unit above records only inputs that can change
    // Cargo's target/dependency plan. Editing an existing Rust module therefore reuses the immutable foundation;
    // adding a new source path or changing a manifest still requires an explicit rebake.
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

/// Normalize explicit target, toolchain, profile, and feature inputs shared by imported and generated receipts.
fn normalized_build_intent(
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
fn generated_source_evidence(
    files: &BTreeMap<String, PathBuf>,
    trees: &BTreeMap<String, PathBuf>,
) -> Result<(BTreeMap<String, String>, BTreeMap<String, BTreeMap<String, String>>), OvenError> {
    let mut digests = BTreeMap::new();
    let mut tree_members = BTreeMap::new();
    for (name, path) in files {
        let name = normalized_generated_source_name(name)?;
        let digest = digest_generated_source_file(path)?;
        if digests.insert(name.clone(), digest).is_some() {
            return Err(OvenError::InvalidGeneratedSource {
                path: path.clone(),
                message: format!("duplicate generated source evidence key `{name}`"),
            });
        }
    }
    for (name, path) in trees {
        let name = normalized_generated_source_name(name)?;
        let members = crate::generated_source::tree_records(path).map_err(OvenError::from)?;
        let digest = crate::generated_source::digest_tree_records(&members).map_err(OvenError::from)?;
        if digests.insert(name.clone(), digest).is_some() {
            return Err(OvenError::InvalidGeneratedSource {
                path: path.clone(),
                message: format!("duplicate generated source evidence key `{name}`"),
            });
        }
        tree_members.insert(name, members);
    }
    if digests.is_empty() {
        return Err(OvenError::InvalidGeneratedSource {
            path: PathBuf::new(),
            message: "must declare at least one generated source file or tree".to_string(),
        });
    }
    Ok((digests, tree_members))
}

/// Normalize proof keys while retaining exact command-local paths and input kinds.
fn normalized_generated_source_bindings(
    inputs: &BTreeMap<String, PathBuf>,
) -> Result<BTreeMap<String, PathBuf>, OvenError> {
    let mut bindings = BTreeMap::new();
    for (name, path) in inputs {
        let name = normalized_generated_source_name(name)?;
        if bindings.insert(name.clone(), path.clone()).is_some() {
            return Err(OvenError::InvalidGeneratedSource {
                path: path.clone(),
                message: format!("duplicate generated source evidence key `{name}`"),
            });
        }
    }
    Ok(bindings)
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
    crate::generated_source::digest_file(path).map_err(OvenError::from)
}

/// Hash the workspace source and fixture closure that determines the repository's native test-suite behaviour.
///
/// Oven deliberately excludes caller outputs such as `.incan` and `target`: those are neither compiler source nor
/// test fixtures, and allowing them into the receipt would make a successful test run invalidate its own stored
/// suite. Every tracked source, fixture, snapshot, and nested crate manifest below the declared roots remains
/// identity-bearing.
/// Return the portable source-to-digest records that make up one native compiler-suite receipt.
fn compiler_suite_source_records(project_root: &Path) -> Result<BTreeMap<String, String>, OvenError> {
    let mut records = BTreeMap::new();
    for root_name in ["src", "tests", "crates"] {
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
    crate::generated_source::digest_tree(root).map_err(OvenError::from)
}

/// Hash authored project inputs while excluding compiler- and tool-owned mutable output trees.
///
/// This intentionally differs from [`digest_source_tree`], whose callers supply an exact generated-source closure
/// and therefore need every regular file represented. A path dependency instead names an authored project root;
/// its `.incan`, `.ralph-cache`, `target`, and `.git` directories are not inputs to the dependency's semantics.
/// Excluding them makes the identity stable across valid local reuse without overlooking any authored file outside
/// those reserved output locations.
pub(crate) fn digest_project_source_tree(root: &Path) -> Result<String, OvenError> {
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

/// Normalize a required identity field and reject blank values that collapse distinct build units.
fn normalized_value(value: &str, field: &'static str) -> Result<String, OvenError> {
    let normalized = value.trim();
    if normalized.is_empty() {
        return Err(OvenError::EmptyBuildIntent { field });
    }
    Ok(normalized.to_string())
}

/// Hash the portable receipt input while excluding checkout and cache paths.
fn receipt_identity(
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
fn build_unit_identity(
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

/// Hash canonical text with Oven's stable `sha256:` rendering.
fn digest_content(content: &str) -> String {
    digest_bytes(content.as_bytes())
}

/// Hash arbitrary canonical identity bytes with Oven's stable `sha256:` rendering.
pub(crate) fn digest_bytes(content: &[u8]) -> String {
    crate::generated_source::digest_bytes(content)
}

/// Write, sync, and atomically replace a receipt from a same-directory staged file.
pub(crate) fn write_receipt_staged(payload: &[u8], staged_path: &Path, path: &Path, parent: &Path) -> io::Result<()> {
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

impl From<crate::generated_source::GeneratedSourceError> for OvenError {
    /// Preserve the original Oven error variants and details at the physical hashing boundary.
    fn from(error: crate::generated_source::GeneratedSourceError) -> Self {
        match error {
            crate::generated_source::GeneratedSourceError::Invalid { path, message } => {
                Self::InvalidGeneratedSource { path, message }
            }
            crate::generated_source::GeneratedSourceError::Serialize(message) => Self::Serialize(message),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use crate::manifest::{DependencySource, DependencySpec};

    use super::{
        OvenCompilerSuiteRequest, OvenGeneratedProjectRequest, OvenReceipt, default_receipt_path, digest_bytes,
        generated_project_source_evidence, receipt_generated_project, receipt_generated_project_with_source_evidence,
        receipt_native_compiler_suite, receipt_with_build_unit_input, receipt_without_build_unit_input, write_receipt,
    };

    #[test]
    fn receipt_identity_is_portable_and_observes_explicit_build_inputs() -> Result<(), Box<dyn std::error::Error>> {
        let first = tempfile::tempdir()?;
        let second = tempfile::tempdir()?;
        write_generated_source_closure(first.path(), "fn main() {}\n")?;
        write_generated_source_closure(second.path(), "fn main() {}\n")?;
        let first_receipt = receipt_generated_project(&generated_request(first.path()))?;
        let second_receipt = receipt_generated_project(&generated_request(second.path()))?;
        assert_eq!(first_receipt.identity, second_receipt.identity);
        for dimension in ["target", "toolchain", "profile", "features"] {
            let mut request = generated_request(second.path());
            match dimension {
                "target" => request.target = "x86_64-unknown-linux-gnu".to_string(),
                "toolchain" => request.toolchain = "rustc 1.98.0".to_string(),
                "profile" => request.profile = "debug".to_string(),
                _ => request.features.push("serde".to_string()),
            }
            assert_ne!(
                first_receipt.identity,
                receipt_generated_project(&request)?.identity,
                "{dimension}"
            );
        }
        assert!(!first_receipt.compatibility.cargo_input_only);
        Ok(())
    }

    #[test]
    fn supplemental_source_evidence_changes_identity_without_recording_paths() -> Result<(), Box<dyn std::error::Error>>
    {
        let project = tempfile::tempdir()?;
        write_generated_source_closure(project.path(), "fn main() {}\n")?;
        let first = receipt_generated_project(&generated_request(project.path()))?;
        fs::write(
            project.path().join("src/main.rs"),
            "fn main() { println!(\"changed\"); }\n",
        )?;
        let second = receipt_generated_project(&generated_request(project.path()))?;
        assert_ne!(first.identity, second.identity);
        assert_ne!(first.sources.supplemental_digests, second.sources.supplemental_digests);
        assert!(!serde_json::to_string(&second)?.contains(&project.path().display().to_string()));
        Ok(())
    }

    #[test]
    fn path_dependency_identity_requires_checked_oven_source_unit() -> Result<(), Box<dyn std::error::Error>> {
        let dependency = DependencySpec {
            crate_name: "local_helper".to_string(),
            version: None,
            features: Vec::new(),
            default_features: true,
            source: DependencySource::Path {
                path: std::path::PathBuf::from("local-helper"),
            },
            optional: false,
            package: None,
        };

        let error = super::digest_dependency_specs(std::slice::from_ref(&dependency))
            .err()
            .ok_or("local Rust path dependency produced an identity without checked Oven evidence")?;
        match error {
            super::OvenError::InvalidProjectSource { path, message } => {
                assert_eq!(path, Path::new("local-helper"));
                assert_eq!(
                    message,
                    "Rust path dependency `local_helper` has no checked Oven source-unit identity; local Rust packages must be admitted as Loaf units"
                );
            }
            other => return Err(format!("unexpected local Rust dependency error: {other}").into()),
        }
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
        let relocated = tempfile::tempdir()?;
        write_generated_source_closure(relocated.path(), "fn main() { println!(\"oven\"); }\n")?;
        let same_names_other_paths = generated_request(relocated.path());
        assert_eq!(receipt_generated_project(&same_names_other_paths)?, reused_release);
        assert!(
            receipt_generated_project_with_source_evidence(&same_names_other_paths, &source_evidence).is_err(),
            "same source names and bytes must not transfer an in-process proof to different paths"
        );
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
                "oven_fixture",
                "0.1.0",
                "aarch64-apple-darwin",
                "rustc 1.96.0",
                "debug",
                vec!["lsp".to_string()],
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
    fn receipt_publication_is_complete_json_at_the_default_project_path() -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        write_generated_source_closure(project.path(), "fn main() {}\n")?;
        let receipt = receipt_generated_project(&generated_request(project.path()))?;
        let path = default_receipt_path(project.path());
        write_receipt(&receipt, &path)?;

        let payload = fs::read_to_string(path)?;
        let decoded: OvenReceipt = serde_json::from_str(&payload)?;
        assert_eq!(decoded, receipt);
        Ok(())
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
