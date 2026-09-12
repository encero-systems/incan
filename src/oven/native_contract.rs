//! Retained native execution wire contracts. Their plan producers were removed with the Cargo authority cut.
//!
//! These descriptors carry already selected inputs; this module does not select a graph or invoke a build tool.

use super::OvenBuildIntent;
use super::rustc::{
    OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION, OvenRustcArtifactExtern, OvenRustcArtifactManifest,
    OvenRustcSupportingArtifact,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const OVEN_PROJECT_EXTENSION_PAYLOAD_SCHEMA_VERSION: u32 = 12;
pub const OVEN_COMPILER_TEST_SUITE_SHARD_SCHEMA_VERSION: u32 = 2;
pub(crate) const OVEN_COMPILER_TEST_SUITE_SHARD_SCHEMA_VERSION_V1: u32 = 1;
pub const OVEN_COMPILER_TEST_SUITE_SCHEMA_VERSION: u32 = 15;
pub const OVEN_COMPILER_TEST_SUITE_FOUNDATION_SCHEMA_VERSION: u32 = 1;
pub const OVEN_COMPILER_TEST_SUITE_TOOLCHAIN_DATA_SCHEMA_VERSION: u32 = 1;

/// Immutable payload retained by a receipt-bound project extension Loaf.
///
/// Re-exported from the legacy-Cargo baker rather than redeclared. Both modules describe the same wire contract, and
/// two same-named types would make the extension a publisher wrote unusable by the consumer that reads it. The
/// declaration moves here once `legacy_cargo` is retired; until then this module names the one that exists.
pub(crate) use super::legacy_cargo::OvenProjectExtensionPayload;


/// Portable source-authority identity for one direct registry dependency declared by a generated project.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct OvenProjectRegistrySourceDependency {
    /// Rust-facing dependency alias from the generated root manifest.
    pub alias: String,
    /// Cargo package name selected by the root resolve edge.
    pub package: String,
    /// Exact locked package version.
    pub version: String,
    /// Exact Cargo registry identity.
    pub registry: String,
    /// Registry archive checksum sealed into the project Loaf.
    pub checksum: String,
}

/// One workspace test root that Oven must execute through a caller-owned Rustc or Rustdoc shard.
///
/// This is publisher planning evidence only: it names a receipt-authorized source root and its resolved direct
/// dependency inputs, never a Cargo-linked executable retained for normal execution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OvenCompilerTestSuiteTarget {
    /// Cargo package declared by the regular manifest owning this source root.
    ///
    /// Target names are only package-local (`tests/smoke.rs` may exist in more than one workspace member), so this
    /// identity is retained with the direct-rustc plan for future independent Oven shard admission.
    #[serde(default)]
    pub package_name: String,
    /// Cargo target name retained for deterministic reporting.
    pub target_name: String,
    /// Cargo target kind such as `lib`, `bin`, `test`, or `proc-macro`.
    pub target_kind: String,
    /// Oven-owned execution mode derived from Cargo's publisher-only unit mode.
    pub runner: String,
    /// Safe compiler-root-relative Rust source path.
    pub source_relative_path: String,
    /// Receipt supplemental-digest key that authorizes this exact source root.
    pub source_evidence_key: String,
    /// Rust identifier passed through `--crate-name`.
    pub crate_name: String,
    /// Rust edition resolved for this target by Cargo's publisher-only unit graph.
    pub edition: String,
    /// Resolved target feature set passed as explicit `--cfg feature=...` arguments.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub features: Vec<String>,
    /// Deterministic package compile environment after inherited Cargo state is cleared.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub compile_environment: BTreeMap<String, String>,
    /// Workspace binary targets whose caller-owned direct-rustc outputs are injected as `CARGO_BIN_EXE_*` values
    /// while this target is compiled and executed. These are execution inputs, never Cargo-linked executables.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub binary_dependencies: Vec<String>,
    /// Workspace libraries and procedural macros that Oven must materialize as caller-owned direct-Rustc inputs
    /// before compiling this root. Third-party externs remain below in immutable selected foundations.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub workspace_library_dependencies: Vec<OvenCompilerWorkspaceLibraryKey>,
    /// Exact direct dependency artifacts selected from the immutable suite closure.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub externs: Vec<OvenRustcArtifactExtern>,
}

/// Stable identity of one receipt-bound compiler-suite root.
///
/// Target names alone are package-local Cargo labels. A future immutable suite index therefore uses this complete
/// key when it refers to independently admitted Oven shards; the current direct planner also uses it to reject only
/// truly duplicate roots.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct OvenCompilerTestSuiteTargetKey {
    /// Package owning the target source.
    pub package_name: String,
    /// Cargo target name.
    pub target_name: String,
    /// Target kind such as `lib`, `bin`, `test`, or `proc-macro`.
    pub target_kind: String,
    /// Oven-owned runner selected for this target.
    pub runner: String,
    /// Receipt-authorized source path below the compiler root.
    pub source_relative_path: String,
}

/// Stable identity of one workspace library or procedural macro in the direct-Rustc materialization DAG.
///
/// This deliberately distinguishes package-local crate names and resolved feature sets. The source path is receipt
/// authorized, while the eventual caller-owned output remains outside the immutable store.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct OvenCompilerWorkspaceLibraryKey {
    /// Workspace package owning the source.
    pub package_name: String,
    /// Rust crate name passed to direct-Rustc `--extern`.
    pub crate_name: String,
    /// Cargo target kind (`lib` or `proc-macro`).
    pub target_kind: String,
    /// Compiler-root-relative source path.
    pub source_relative_path: String,
    /// Resolved Cargo feature set for this exact compilation unit.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub features: Vec<String>,
}

/// One workspace library or procedural macro Oven must bake before a compiler-suite root.
///
/// The immutable suite index retains this compact source/extern plan; the resulting artifact is caller-owned under
/// the suite output directory and is never copied back into the Oven store as a Cargo-shaped target tree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OvenCompilerWorkspaceLibrary {
    /// Stable direct-Rustc DAG identity for this source unit.
    pub key: OvenCompilerWorkspaceLibraryKey,
    /// Receipt supplemental digest that authorizes this source content.
    pub source_evidence_key: String,
    /// Rust edition selected by the publisher-only unit graph.
    pub edition: String,
    /// Compiler package environment reconstructed after Cargo state has been cleared.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub compile_environment: BTreeMap<String, String>,
    /// Immutable third-party foundation externs required by this workspace source.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub externs: Vec<OvenRustcArtifactExtern>,
    /// Other workspace libraries or procedural macros that must be materialized first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dependencies: Vec<OvenCompilerWorkspaceLibraryKey>,
}

/// Immutable payload retained by one separately admitted compiler-suite shard.
///
/// A shard owns only the closure needed for one receipt-bound direct-rustc target plus any caller-owned workspace
/// binaries that target declares through `CARGO_BIN_EXE_*`. The index remains small and refers to the store identity;
/// the shard, not the index, owns the potentially large dependency materialization.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OvenCompilerTestSuiteShardPayload {
    /// Shard wire-schema version.
    pub schema_version: u32,
    /// The one direct Rustc or Rustdoc root executed from this shard.
    pub target: OvenCompilerTestSuiteTarget,
    /// Direct-rustc binary plans required only by this target.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub binary_targets: Vec<OvenCompilerTestSuiteTarget>,
    /// Schema-11 direct-Rustc workspace-library/proc-macro DAG required by the selected roots.
    ///
    /// Schema-10 entries leave this empty. A future publisher must include every key referenced by a root before a
    /// consumer is permitted to materialize the suite.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub workspace_libraries: Vec<OvenCompilerWorkspaceLibrary>,
    /// Immutable compiler dependency foundations required to materialize this root without a Cargo target.
    ///
    /// Schema 10 uses these exact identities to compose the target closure from multiple independently bounded
    /// domains. Schema 9 retains this as empty so older payloads remain unambiguous and refuse the new execution
    /// shape until the index itself advertises schema 10.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub foundation_references: Vec<OvenCompilerTestSuiteFoundationReference>,
    /// Exact immutable direct-rustc dependency closure for this target and its binary plans.
    pub artifact_closure: OvenCompilerTestSuiteArtifactClosure,
}

/// One immutable compiler-suite shard selected by an index.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OvenCompilerTestSuiteShardReference {
    /// Content-addressed Oven store identity of the separately admitted shard artifact.
    pub identity: String,
    /// Complete root identity expected inside that shard payload.
    pub target: OvenCompilerTestSuiteTargetKey,
    /// Digest-verified byte length of the exact receipt-authorized target source.
    ///
    /// A source path may occur in more than one resolved unit, so each immutable shard reference records its own
    /// footprint even when another reference names the same path. Schema-15 consumers use this only to balance
    /// independent replay work; it does not authorize a source or replace the receipt digest check before Rustc.
    #[serde(default)]
    pub source_bytes: u64,
}

/// One immutable compiler dependency foundation selected by a schema-10 root shard.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct OvenCompilerTestSuiteFoundationReference {
    /// Content-addressed Oven store identity of the individually admitted foundation artifact.
    pub identity: String,
    /// Stable publisher label used to reject reordered or substituted foundation sets.
    pub label: String,
}

/// Immutable payload for one policy-addressable part of a compiler test dependency closure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OvenCompilerTestSuiteFoundationPayload {
    /// Foundation wire-schema version.
    pub schema_version: u32,
    /// Stable deterministic partition label, such as `foundation-0000`.
    pub label: String,
    /// The exact fragment of the direct-rustc closure materialized by this foundation.
    pub artifact_closure: OvenCompilerTestSuiteArtifactClosure,
}

/// One immutable compiler-Loaf partition selected by a schema-13 suite before any child starts.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct OvenCompilerTestSuiteToolchainDataReference {
    /// Content-addressed Oven store identity of the independently bounded partition.
    pub identity: String,
    /// Stable publisher label used to reject reordered or substituted partitions.
    pub label: String,
}

/// Immutable payload for one policy-addressable compiler-Loaf data partition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OvenCompilerTestSuiteToolchainDataPayload {
    /// Toolchain-data wire-schema version.
    pub schema_version: u32,
    /// Stable deterministic partition label.
    pub label: String,
}

/// Shared immutable artifact closure used by all direct Rustc and Rustdoc compiler-suite targets.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OvenCompilerTestSuiteArtifactClosure {
    /// Store-relative directories passed as `-L dependency` for each target shard.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dependency_search_paths: Vec<String>,
    /// Store-relative directories passed as `-L native` for each target shard.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub native_search_paths: Vec<String>,
    /// Complete verified artifact set shared by the target plans.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub supporting_artifacts: Vec<OvenRustcSupportingArtifact>,
}

/// Wire payload for the stored full compiler test suite and the CLI fixture it invokes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OvenCompilerTestSuitePayload {
    /// Payload schema for the stored compiler-suite runtime.
    pub schema_version: u32,
    /// Receipt-bound native workspace target plan. Schema 8 executes caller-owned direct-rustc and direct-Rustdoc
    /// shards instead of retaining Cargo-linked test executables, and carries any installed compiler Loaf
    /// data required by their fixture commands.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub test_targets: Vec<OvenCompilerTestSuiteTarget>,
    /// Schema-9 immutable index entries for independently admitted compiler-suite target shards.
    ///
    /// Schema 8 leaves this empty while it retains one transitional shared closure. Schema 9 will require these
    /// references and must not carry that closure alongside them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub shard_references: Vec<OvenCompilerTestSuiteShardReference>,
    /// Schema-10 dependency foundations selected transitively through individual root shards.
    ///
    /// The index retains the complete related set as receipt-bound execution authority so the scheduler can acquire
    /// every lease before its first child. Individual shards repeat only the foundations they require.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub foundation_references: Vec<OvenCompilerTestSuiteFoundationReference>,
    /// Schema-13 Loaf data partitions required by stored-suite fixture children.
    ///
    /// These are separate from direct-rustc foundations because children consume them as compiler data rather than
    /// `--extern` artifacts. Each partition is selected and lease-held before the first child starts.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub toolchain_data_references: Vec<OvenCompilerTestSuiteToolchainDataReference>,
    /// Schema-14 reference to the compiler-owned standard-library Loaf generation consumed directly by suite
    /// children.  Unlike schema-13 partitions, this is a lease-held reference to the installed release-family
    /// envelope rather than a second full copy in the receipt-bound compiler-suite store.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub toolchain_loaf_generation: Option<OvenCompilerTestSuiteToolchainLoafGenerationReference>,
    /// Receipt-bound workspace binary plans required by test-root `CARGO_BIN_EXE_*` inputs. The main `incan` CLI
    /// remains the separately named `cli_target` below because it is also the stored-suite fixture command.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub binary_targets: Vec<OvenCompilerTestSuiteTarget>,
    /// Shared direct-rustc closure for every native workspace target plan.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub test_artifact_closure: Option<OvenCompilerTestSuiteArtifactClosure>,
    /// Direct-rustc closure for the scheduler's separately baked compiler CLI.
    ///
    /// Schema 8 derives this from `test_artifact_closure`. Schema 9 keeps it separate so that the index does not
    /// retain one shared test-root closure alongside independently admitted target shards.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cli_artifact_closure: Option<OvenCompilerTestSuiteArtifactClosure>,
    /// Schema-11 foundations required to materialize the compiler CLI without retaining a Cargo-built workspace
    /// library in the suite index.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cli_foundation_references: Vec<OvenCompilerTestSuiteFoundationReference>,
    /// Direct-rustc compiler CLI plan materialized in the caller output for integration-test children.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cli_target: Option<OvenCompilerTestSuiteTarget>,
    /// Schema-11 caller-owned workspace libraries/proc macros required before baking `cli_target`.
    ///
    /// Their outputs live only beneath the current command's output directory, while third-party externs remain in
    /// the separately validated CLI artifact closure. Schema 10 leaves this empty and does not accept a CLI target
    /// that declares workspace-library edges.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cli_workspace_libraries: Vec<OvenCompilerWorkspaceLibrary>,
    /// Store-relative SDK provider inventory selected by compiler-suite fixture children.
    pub sdk_inventory_relative_path: String,
    /// Digest of the immutable SDK provider inventory.
    pub sdk_inventory_digest: String,
    /// Optional store-relative root of compiler-owned Loaf data copied from the publisher's installed package.
    ///
    /// A direct-rustc child is baked below caller-owned output, so it cannot infer the parent package layout from its
    /// own executable path. The suite owns this copied data rather than depending on an ambient archive location.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub toolchain_data_relative_root: Option<String>,
    /// Complete direct-rustc closure used by compiler tests that validate generated Rust without Cargo.
    ///
    /// Schemas through 11 retain this closure from a small publisher Cargo target. Schema 12 deliberately leaves it
    /// empty: the executor rebuilds the receipt-authorized `incan_stdlib` workspace library from an indexed shard,
    /// avoiding a second concurrently retained Cargo target.
    pub warning_check_artifacts: OvenRustcArtifactManifest,
}

/// One exact compiler-owned Loaf generation retained externally while a compiler-suite invocation runs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OvenCompilerTestSuiteToolchainLoafGenerationReference {
    /// Content identity of the atomically committed compiler-suite Loaf generation.
    pub generation_identity: String,
}

impl OvenCompilerTestSuiteTarget {
    /// Return the complete target identity used for duplicate detection and future shard-index membership.
    #[must_use]
    pub fn key(&self) -> OvenCompilerTestSuiteTargetKey {
        OvenCompilerTestSuiteTargetKey {
            package_name: self.package_name.clone(),
            target_name: self.target_name.clone(),
            target_kind: self.target_kind.clone(),
            runner: self.runner.clone(),
            source_relative_path: self.source_relative_path.clone(),
        }
    }
}

impl OvenCompilerTestSuiteShardPayload {
    /// Return the stable root key an immutable suite index must use to identify this shard.
    #[must_use]
    pub fn target_key(&self) -> OvenCompilerTestSuiteTargetKey {
        self.target.key()
    }
}

impl OvenCompilerTestSuiteArtifactClosure {
    /// Reconstitute the exact manifest for one native target without duplicating the complete closure in the payload.
    #[must_use]
    pub fn manifest_for_target(
        &self,
        target: &OvenCompilerTestSuiteTarget,
        intent: OvenBuildIntent,
    ) -> OvenRustcArtifactManifest {
        let selected = target
            .externs
            .iter()
            .map(|artifact| artifact.relative_path.as_str())
            .collect::<BTreeSet<_>>();
        let supporting_artifacts = self
            .supporting_artifacts
            .iter()
            .filter(|artifact| !selected.contains(artifact.relative_path.as_str()))
            .cloned()
            .collect();
        OvenRustcArtifactManifest {
            schema_version: OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION,
            intent,
            dependency_search_paths: self.dependency_search_paths.clone(),
            native_search_paths: self.native_search_paths.clone(),
            externs: target.externs.clone(),
            entrypoint_externs: BTreeMap::new(),
            registry_leaves: Vec::new(),
            registry_sources: Vec::new(),
            compile_environment: target.compile_environment.clone(),
            vocab_auxiliary_targets: Vec::new(),
            supporting_artifacts,
        }
    }

    /// Reconstitute the immutable third-party inputs for one direct-Rustc workspace-library step.
    #[must_use]
    pub fn manifest_for_workspace_library(
        &self,
        library: &OvenCompilerWorkspaceLibrary,
        intent: OvenBuildIntent,
    ) -> OvenRustcArtifactManifest {
        let selected = library
            .externs
            .iter()
            .map(|artifact| artifact.relative_path.as_str())
            .collect::<BTreeSet<_>>();
        let supporting_artifacts = self
            .supporting_artifacts
            .iter()
            .filter(|artifact| !selected.contains(artifact.relative_path.as_str()))
            .cloned()
            .collect();
        OvenRustcArtifactManifest {
            schema_version: OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION,
            intent,
            dependency_search_paths: self.dependency_search_paths.clone(),
            native_search_paths: self.native_search_paths.clone(),
            externs: library.externs.clone(),
            entrypoint_externs: BTreeMap::new(),
            registry_leaves: Vec::new(),
            registry_sources: Vec::new(),
            compile_environment: library.compile_environment.clone(),
            vocab_auxiliary_targets: Vec::new(),
            supporting_artifacts,
        }
    }
}
