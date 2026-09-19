//! The Cargo-free wire contract the direct-rustc route publishes and consumes.
//!
//! This module declares the payload, suite, shard, foundation and toolchain-data types the native route reads and
//! writes, the schema constants that version them, the provider-compilation evidence key, and the inspection-source
//! handoff the Loaf envelope describes. The compatibility baker that produces most of them lives in
//! `oven_cargo_compat`, over this crate, and re-exports these declarations so a publisher and the consumer that
//! reads its output hold one type: two same-named structs with identical fields are not one type, and redeclaring
//! `OvenProjectExtensionPayload` across that boundary once made it unusable.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use oven_store::OvenBuildIntent;
use serde::{Deserialize, Serialize};

use crate::rustc::{
    OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION, OvenRustcArtifactExtern, OvenRustcArtifactManifest,
    OvenRustcSupportingArtifact,
};

/// Wire schema for a receipt-bound project extension Loaf.
///
/// Version 9 binds every direct registry dependency alias to its exact locked package, registry, and checksum. This
/// preserves source authority when one project intentionally selects multiple compatible versions or renamed aliases of
/// a package instead of asking a normal command to infer identity from a semver-compatible source catalog. Version 10
/// records the generated root's registry packages so recomposition reproduces substitution regimes. Version 11 re-roots
/// retained extension artifacts that collide with the base execution closure into `extension-deps`, so plans composed
/// under the older digest-stamping rule must rebake. Version 12 salts extension crate identities (`-C
/// metadata=incan-extension`) so shared interior units coexist with the sealed base's twins as distinct crates; plans
/// built with unsalted identities must rebake.
pub const OVEN_PROJECT_EXTENSION_PAYLOAD_SCHEMA_VERSION: u32 = 12;

/// Wire schema for one independently admitted compiler-suite target shard.
///
/// Version 2 adds the direct-Rustc workspace library/proc-macro materialization DAG. Consumers of schema-10 suite
/// indexes continue to require version 1, so an older executor can never silently omit those `--extern` edges.
pub const OVEN_COMPILER_TEST_SUITE_SHARD_SCHEMA_VERSION: u32 = 2;

pub const OVEN_COMPILER_TEST_SUITE_SHARD_SCHEMA_VERSION_V1: u32 = 1;

/// Wire schema for the immutable compiler-suite index.
///
/// Version 15 records a digest-verified source footprint for every independently admitted root. Consumers use that
/// receipt-bound evidence to distribute roots without a mutable timing profile or a test-name scheduling table.
/// Version 16 references schema-2 foundations only, which carry their own key and portable artifact index (#1564); a
/// schema-15 index is never reused, so its schema-1 foundations are never leased by a current scheduler.
pub const OVEN_COMPILER_TEST_SUITE_SCHEMA_VERSION: u32 = 16;

/// Wire schema for one independently admitted compiler-suite dependency foundation.
///
/// Version 2 records the family record described by [`OvenCompilerTestSuiteFoundationFamily`]: the foundation's own
/// key, its partition coordinates and a portable Cargo artifact index. A later suite publication selects the
/// foundation by that key and plans its shards from the index, so a compiler source edit no longer asks Cargo to
/// compile the third-party closure again (#1564).
pub const OVEN_COMPILER_TEST_SUITE_FOUNDATION_SCHEMA_VERSION: u32 = 2;

/// Wire schema for one independently admitted compiler-Loaf data partition.
pub const OVEN_COMPILER_TEST_SUITE_TOOLCHAIN_DATA_SCHEMA_VERSION: u32 = 1;

/// One registry package whose source may be inspected while compiling a checked Incan fixture.
///
/// The hidden baker resolves this selector against its locked Cargo graph. Compiled provider/runtime artifacts remain
/// part of the direct-Rustc closure, but their source trees are not copied into a Loaf unless this explicit surface
/// reaches them.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct OvenLegacyCargoInspectionPackage {
    /// Cargo package name, after applying an Incan dependency's optional `package` rename.
    pub package: String,
    /// Cargo-compatible version requirement declared by the checked Incan manifest.
    pub version_requirement: String,
}

/// Source-evidence projection reserved for rebuilding checked providers rather than the consumer root.
pub const OVEN_PROVIDER_COMPILATION_KEY: &str = "provider-compilation";

/// Immutable payload retained by a receipt-bound project extension Loaf.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OvenProjectExtensionPayload {
    /// Version of this extension wire contract.
    pub schema_version: u32,
    /// Content address of the exact compiler-shipped base Loaf that supplies the selected release cohort.
    pub base_loaf_identity: String,
    /// Compatibility identity that must still authorize the project receipt when the extension is consumed.
    pub base_build_unit_identity: String,
    /// Raw publisher-derived direct-Rustc plan retained as immutable provenance.
    ///
    /// This is never executed by a normal command. It records the project publisher's original closure before its
    /// compiler-owned runtime, overlapping registry, and vocabulary inputs are canonicalized against the exact base.
    pub publisher_plan: OvenRustcArtifactManifest,
    /// Complete direct-Rustc execution contract after release-cohort canonicalization and base composition.
    pub complete_plan: OvenRustcArtifactManifest,
    /// Exact root registry dependency identities selected by the explicit baker, sorted by their Rust-facing aliases.
    #[serde(default)]
    pub registry_source_dependencies: Vec<OvenProjectRegistrySourceDependency>,
    /// Exact dev-only root registry dependency identities selected by the same canonical publisher lock.
    ///
    /// These remain separate from normal roots so an inspection consumer can validate the complete test surface
    /// without pretending a dev-only crate belongs to a normal generated executable.
    #[serde(default)]
    pub dev_registry_source_dependencies: Vec<OvenProjectRegistrySourceDependency>,
    /// Sorted paths physically retained below this extension's immutable artifact root.
    pub extension_paths: Vec<String>,
}

/// Portable source-authority identity for one direct registry dependency declared by a generated project.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OvenProjectRegistrySourceDependency {
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

impl OvenCompilerTestSuiteShardPayload {
    /// Return the stable root key an immutable suite index must use to identify this shard.
    #[must_use]
    pub fn target_key(&self) -> OvenCompilerTestSuiteTargetKey {
        self.target.key()
    }
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
    /// Schema-2 family record: the foundation's own key and everything a later publication needs to reuse the family
    /// without Cargo. `None` only on a schema-1 entry, which no current suite index references.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub family: Option<OvenCompilerTestSuiteFoundationFamily>,
}

/// What identifies one compiler-suite foundation family and lets a later suite publication plan against it (#1564).
///
/// Every partition of a family carries the same record, so a family is complete when every `partition_index` below
/// `partition_count` is present under one `key`. The search paths and the artifact index describe the unpartitioned
/// closure: partition payloads narrow `artifact_closure` to the files they own, and a consumer reunites them here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OvenCompilerTestSuiteFoundationFamily {
    /// The foundation key, `sha256:` over the foundation's own inputs only — never the compiler's source digests.
    pub key: String,
    /// Zero-based position of this partition within the family.
    pub partition_index: u32,
    /// Number of partitions the family was split into.
    pub partition_count: u32,
    /// Staging-relative `-L dependency` directories of the complete closure, in publisher order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dependency_search_paths: Vec<String>,
    /// Staging-relative `-L native` directories of the complete closure, in publisher order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub native_search_paths: Vec<String>,
    /// Cargo's compiler-artifact records for the complete family, in a form that survives another machine.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifact_index: Vec<OvenCompilerTestSuiteFoundationArtifactRecord>,
}

/// One Cargo compiler-artifact record of a foundation, keyed so a later unit graph on any machine can find it.
///
/// Cargo's JSON stream names a unit by an opaque package ID and the absolute path of its crate root. A registry
/// package ID is stable, but a checked-in third-party patch carries the checkout path, and the crate root lives under
/// whichever Cargo home built it. The record therefore names the package the way its lock does — name, version and
/// registry source, or the patch root relative to the compiler root — and the crate root relative to the package root.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct OvenCompilerTestSuiteFoundationArtifactRecord {
    /// Cargo package name.
    pub package: String,
    /// Exact package version.
    pub version: String,
    /// Registry source URL, or `None` for a checked-in third-party patch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// For a patch, the package root relative to the compiler root; `None` for a registry package.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package_relative_path: Option<String>,
    /// Cargo target name of the compiled unit.
    pub target_name: String,
    /// Crate root source relative to the package root, with `/` separators.
    pub source_relative_path: String,
    /// Sorted feature set the unit was compiled with.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub features: Vec<String>,
    /// Whether Cargo compiled the unit under its test profile.
    #[serde(default)]
    pub test_profile: bool,
    /// The receipt target when the unit was compiled for it, `None` for a host-side unit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform: Option<String>,
    /// Staging-relative artifact files, spelled exactly as the closure's `supporting_artifacts` name them.
    pub files: Vec<String>,
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
            entrypoint_dependency_search_paths: Default::default(),
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
            entrypoint_dependency_search_paths: Default::default(),
            entrypoint_externs: BTreeMap::new(),
            registry_leaves: Vec::new(),
            registry_sources: Vec::new(),
            compile_environment: library.compile_environment.clone(),
            vocab_auxiliary_targets: Vec::new(),
            supporting_artifacts,
        }
    }
}

/// Wire payload for the stored full compiler test suite and the CLI fixture it invokes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OvenCompilerTestSuitePayload {
    /// Payload schema for the stored compiler-suite runtime.
    pub schema_version: u32,
    /// Receipt-bound native workspace target plan. Schema 8 executes caller-owned direct-rustc and direct-Rustdoc
    /// shards instead of retaining Cargo-linked test executables, and carries any installed compiler Loaf data
    /// required by their fixture commands.
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
    /// empty: the executor rebuilds the receipt-authorized standard library facets from an indexed shard,
    /// avoiding a second concurrently retained Cargo target.
    pub warning_check_artifacts: OvenRustcArtifactManifest,
}

/// One exact compiler-owned Loaf generation retained externally while a compiler-suite invocation runs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OvenCompilerTestSuiteToolchainLoafGenerationReference {
    /// Content identity of the atomically committed compiler-suite Loaf generation.
    pub generation_identity: String,
}

/// Typed source handoff used only by children of the explicit `legacy_cargo` baker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OvenLegacyCargoInspectionSource {
    /// Cargo package name selected by the frozen publisher resolution.
    pub package: String,
    /// Exact Cargo package version selected by the frozen publisher resolution.
    pub version: String,
    /// Canonical registry source identifier from Cargo metadata.
    pub registry: String,
    /// Registry checksum recorded in the publisher's locked dependency graph.
    pub checksum: String,
    /// Features selected for this package by the resolved publisher graph.
    pub features: Vec<String>,
    /// Exact registry source root visible only inside the named baker boundary.
    pub source_root: PathBuf,
    /// Digest of the complete regular-file source tree beneath `source_root`.
    pub source_digest: String,
    /// Complete ordered regular-file inventory retained from the staged immutable source tree.
    pub members: Vec<OvenLegacyCargoInspectionSourceMember>,
}

/// One exact regular file retained by the explicit publisher's staged registry source authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OvenLegacyCargoInspectionSourceMember {
    /// Safe package-root-relative path.
    pub path: String,
    /// Digest of the regular file's exact bytes.
    pub digest: String,
}
