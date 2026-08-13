//! Incan-owned Oven receipts for the supported Alpha compatibility envelope.
//!
//! This small compiler-owned Rust kernel is temporary implementation debt scoped to the tracked Oven Alpha (#1005,
//! #975): the present language cannot own the required process, file-lock, and durable-publication primitives
//! directly. It must remain a narrow, removable boundary rather than growing into a Rust orchestration layer for
//! the product workflow.
//!
//! Oven reads frozen Cargo declarations only as compatibility evidence. Receipt and consumer paths do not invoke
//! Cargo, inspect a target directory, or claim to have performed native package resolution. The explicitly named
//! `legacy_cargo` baker is the sole Alpha bootstrap boundary that may invoke Cargo. Later Oven store and executor
//! stages consume portable identities and sealed Loafs rather than project-local build paths.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{self, ErrorKind, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::manifest::{DependencySource, DependencySpec, GitReference, ProjectManifest};

pub(crate) mod compiler_suite_env;
pub(crate) mod interop;
pub mod legacy_cargo;
pub mod loaf;
pub mod native_test;
mod process;
pub mod rustc;
pub mod store;

/// Digest the portable dependency facts that select a native Oven closure.
///
/// Registry and Git specifications are represented by their declared immutable selection facts. Path dependencies add
/// only a source-tree digest, never the machine-local path. This keeps compatible clean worktrees reusable while a
/// changed local runtime or declared dependency source necessarily selects a different build unit.
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
            DependencySource::Path { path } => format!("path-tree:{}", digest_source_tree(path)?),
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
pub const DEFAULT_OVEN_MAX_PHYSICAL_BYTES: u64 = 3 * 1024 * 1024 * 1024;
/// Default physical allocation cap for one compatibility domain.
pub const DEFAULT_OVEN_MAX_DOMAIN_PHYSICAL_BYTES: u64 = 1024 * 1024 * 1024;
/// Default logical artifact-byte cap for one compatibility domain.
pub const DEFAULT_OVEN_MAX_DOMAIN_LOGICAL_BYTES: u64 = 768 * 1024 * 1024;
/// Aggregate physical allowance for the complete compiler-suite Loaf and repository-test closure.
pub const DEFAULT_OVEN_COMPILER_SUITE_MAX_PHYSICAL_BYTES: u64 = 16 * 1024 * 1024 * 1024;
/// Physical allowance for the compiler-suite compatibility domain.
pub const DEFAULT_OVEN_COMPILER_SUITE_MAX_DOMAIN_PHYSICAL_BYTES: u64 = 3 * 1024 * 1024 * 1024;
/// Logical artifact-byte allowance for the compiler-suite compatibility domain.
///
/// The complete LSP closure measures 3,271,283,026 logical bytes on Linux;
/// 4 GiB leaves practical policy headroom without relaxing its physical bound.
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

/// Compiler-owned request for the repository's Rust libtest suite receipt.
///
/// This is deliberately distinct from an arbitrary frozen Cargo package: it records the compiler source closure and
/// Cargo declarations as evidence for a bounded repository-suite publisher, then lets the normal consumer select and
/// compile through direct rustc. It does not make Cargo a normal test executor.
#[derive(Debug, Clone)]
pub struct OvenCompilerSuiteRequest {
    project_root: PathBuf,
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
    /// SHA-256 digest of normalized `incan.toml` content when present.
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
    #[error("Oven Alpha compatibility miss: failed to read incan.toml at {path}: {message}")]
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
    /// A generated source input could not be read or did not satisfy the Alpha regular-file closure rules.
    #[error("invalid Oven generated source {path}: {message}")]
    InvalidGeneratedSource { path: PathBuf, message: String },
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
    let project = OvenProjectIdentity {
        name: normalized_value(&request.project.name, "project name")?,
        version: normalized_value(&request.project.version, "project version")?,
    };
    let sources = OvenSourceEvidence {
        cargo_manifest_digest: None,
        cargo_lock_digest: None,
        incan_manifest_digest: None,
        supplemental_digests: generated_source_evidence(request)?,
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
    let project = parse_cargo_package(&cargo_manifest_path, &cargo_manifest)?;
    validate_cargo_lock(&cargo_lock_path, &cargo_lock)?;
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

/// Extract a root package identity without resolving a Cargo dependency graph.
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
    let path = project_root.join("incan.toml");
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
    let mut hasher = Sha256::new();
    hasher.update(content);
    format!("sha256:{}", hex::encode(hasher.finalize()))
}

/// Write, sync, and atomically replace a receipt from a same-directory staged file.
fn write_receipt_staged(payload: &[u8], staged_path: &Path, path: &Path, parent: &Path) -> io::Result<()> {
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
    use std::path::Path;

    use super::{
        OvenCompilerSuiteRequest, OvenGeneratedProjectRequest, OvenImportRequest, OvenReceipt, default_receipt_path,
        digest_bytes, import_frozen_project, receipt_generated_project, receipt_native_compiler_suite,
        receipt_with_build_unit_input, write_receipt,
    };

    fn receipt_without_build_unit_input(
        receipt: &OvenReceipt,
        input: &str,
    ) -> Result<OvenReceipt, Box<dyn std::error::Error>> {
        receipt.verify_identity()?;
        let mut base = receipt.clone();
        if base.sources.build_unit_inputs.remove(input).is_none() {
            return Err(std::io::Error::other(format!("receipt has no build-unit input `{input}`")).into());
        }
        base.identity = super::receipt_identity(&base.project, &base.sources, &base.intent, &base.compatibility)?;
        base.build_unit_identity =
            super::build_unit_identity(&base.intent, &base.compatibility, &base.sources.build_unit_inputs)?;
        Ok(base)
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
            root.join("incan.toml"),
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
