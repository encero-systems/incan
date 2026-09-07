//! Hidden `legacy_cargo` Loaf baker for Oven Alpha compatibility preparation.
//!
//! This is deliberately not an execution backend. It may be invoked only through the named `legacy_cargo` command
//! while direct closure materialization is being completed for #1005/#975. It bakes typed `.loaf/` envelopes and
//! publishes receipt-bound compiler-suite plans into the bounded Oven store, then removes every private Cargo target
//! before returning. Normal Oven build, run, and test code neither calls this module nor receives a Cargo target path.

mod cargo_json;

// Cargo's own JSON shapes live beside this file rather than inside it. They are deserialization targets with no
// publisher behavior, and every path stays where callers expect it through this re-export, so this is a move.
use cargo_json::*;

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::cli::commands::common::discover_active_sdk_inventory;
use crate::library_manifest::{LibraryManifest, digest_provider_artifact, digest_toolchain_source_tree_with_cache};
use crate::provider::{SDK_INVENTORY_FILE, SdkInventory};

use super::process::{isolate_process_group, terminate_process_group};
use super::rustc::{
    OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION, OVEN_RUSTC_REGISTRY_LOCK_RELATIVE_PATH, OvenRustcArtifactExtern,
    OvenRustcArtifactManifest, OvenRustcRegistryLeaf, OvenRustcRegistrySource, OvenRustcRegistrySourcePackage,
    OvenRustcSupportingArtifact, clear_inherited_cargo_environment, rerooted_artifact_staging_source,
    rustc_host_target, rustc_identity, select_direct_rustc_plan_identity,
    validate_project_extension_payload_against_base,
};
use super::store::{
    OvenArtifactKind, OvenArtifactMaterializedFile, OvenArtifactPublishRequest, OvenStore, OvenStoreError,
};
use super::{
    OVEN_COMPILER_TEST_PROFILE, OvenBuildIntent, OvenCompatibilityKind, OvenReceipt, compiler_suite_source_evidence_key,
};
use super::{digest_bytes, digest_source_tree};
use crate::version::{INCAN_VERSION, SDK_PROVIDER_CODEGEN_REVISION};

/// Wire format retained as an immutable supporting artifact alongside every `legacy_cargo`-prepared closure.
pub const OVEN_LEGACY_CARGO_PROVENANCE_SCHEMA_VERSION: u32 = 2;
/// Wire schema for a receipt-bound project extension Loaf.
///
/// Version 9 binds every direct registry dependency alias to its exact locked package, registry, and checksum. This
/// preserves source authority when one project intentionally selects multiple compatible versions or renamed aliases
/// of a package instead of asking a normal command to infer identity from a semver-compatible source catalog.
/// Version 10 records the generated root's registry packages so recomposition reproduces substitution regimes.
/// Version 11 re-roots retained extension artifacts that collide with the base execution closure into
/// `extension-deps`, so plans composed under the older digest-stamping rule must rebake.
/// Version 12 salts extension crate identities (`-C metadata=incan-extension`) so shared interior units coexist
/// with the sealed base's twins as distinct crates; plans built with unsalted identities must rebake.
pub const OVEN_PROJECT_EXTENSION_PAYLOAD_SCHEMA_VERSION: u32 = 12;
/// Wire schema for one independently admitted compiler-suite target shard.
///
/// Version 2 adds the direct-Rustc workspace library/proc-macro materialization DAG. Consumers of schema-10 suite
/// indexes continue to require version 1, so an older executor can never silently omit those `--extern` edges.
pub const OVEN_COMPILER_TEST_SUITE_SHARD_SCHEMA_VERSION: u32 = 2;
pub(crate) const OVEN_COMPILER_TEST_SUITE_SHARD_SCHEMA_VERSION_V1: u32 = 1;
/// Wire schema for the immutable compiler-suite index.
///
/// Version 15 records a digest-verified source footprint for every independently admitted root. Consumers use that
/// receipt-bound evidence to distribute roots without a mutable timing profile or a test-name scheduling table.
pub const OVEN_COMPILER_TEST_SUITE_SCHEMA_VERSION: u32 = 15;
/// Wire schema for one independently admitted compiler-suite dependency foundation.
pub const OVEN_COMPILER_TEST_SUITE_FOUNDATION_SCHEMA_VERSION: u32 = 1;
/// Wire schema for one independently admitted compiler-Loaf data partition.
pub const OVEN_COMPILER_TEST_SUITE_TOOLCHAIN_DATA_SCHEMA_VERSION: u32 = 1;
/// Reserve payload and manifest headroom when splitting a closure by the logical domain policy.
///
/// The publisher still asks the store to make the authoritative admission decision. This small deterministic margin
/// lets a foundation describe its selected artifacts without accidentally placing a near-limit group over policy.
const COMPILER_TEST_SUITE_FOUNDATION_METADATA_HEADROOM_BYTES: u64 = 64 * 1024;
/// Minimum idle interval between full private-staging capacity scans while the explicit publisher is compiling.
///
/// A scan has to walk Cargo's whole transient tree in order to preserve physical-byte accounting and catch output
/// outside the immediate target subdirectory. Polling it every 25 ms made that guard compete with large dependency
/// compiles and turn one cold publish into repeated multi-gigabyte tree walks. A quarter-second floor keeps small
/// overrun fixtures prompt; longer scans must yield for at least their own duration so the guard cannot monopolize
/// the publisher after the private tree reaches gigabytes.
pub(crate) const PUBLISHER_CAPACITY_POLL_INTERVAL: Duration = Duration::from_millis(250);

/// Return the idle time after one complete capacity scan.
///
/// The physical-byte policy must inspect every file under private staging because a child can write outside its
/// declared Cargo target. The monitor therefore yields for the scan duration when a large tree makes that scan more
/// expensive than the ordinary prompt-poll floor. This bounds the monitor to at most half of one CPU rather than
/// beginning the next full walk immediately after the preceding one.
pub(crate) fn publisher_capacity_probe_delay(scan_elapsed: Duration) -> Duration {
    PUBLISHER_CAPACITY_POLL_INTERVAL.max(scan_elapsed)
}
/// Cargo-profile definition mirrored by every publisher-owned compatibility project that receives the suite receipt.
///
/// The direct-Rustc consumer applies the same contract independently. Keeping this as a single manifest fragment
/// prevents a small publisher helper from accepting an `oven-test` receipt without defining its profile.
const OVEN_COMPILER_TEST_CARGO_PROFILE_MANIFEST: &str =
    "\n[profile.oven-test]\ninherits = \"dev\"\ndebug = 0\nincremental = false\n";

/// The one explicit publisher action authorized for an immutable direct-rustc plan.
///
/// The distinction is recorded at publication time because a library-test closure contains dev-dependencies and is
/// not interchangeable with an executable closure even when Cargo.toml is shared.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum OvenLegacyCargoPublicationKind {
    /// Build one generated executable/library root without test-only dependencies.
    Executable,
    /// Build only a generated executable's publisher-only companion library before native interop is sealed.
    ///
    /// This preserves ordinary executable Cargo topology: only the explicit interop bootstrap emits and selects the
    /// companion `src/main.rs` library target, so Cargo never links a package-owned native library before Oven has
    /// sealed that library into the final direct-rustc plan.
    InteropBootstrap,
    /// Build one library's libtest inputs with Cargo only at this explicit publisher boundary.
    LibraryTests,
}

/// The direct Rust dependency surface sealed by one explicit publisher transaction.
///
/// Ordinary generated projects retain only dependencies their generated Rust source can name. A compiler-owned
/// standard-library Loaf instead retains every dependency declared by its checked fixture manifest: generated
/// standard-library modules are compiled alongside a consuming project and may name a facade's upstream Rust crate
/// even when the small fixture root did not itself exercise that facade. This remains a bounded, checked manifest
/// closure; it is never a scan of an ambient Cargo target directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum OvenLegacyCargoDirectDependencyClosure {
    /// Seal only generated-source roots and documented compiler macro-expansion roots.
    GeneratedSource,
    /// Seal every direct dependency declared by the checked publisher manifest.
    CheckedDeclared,
}

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

/// Explicit input to the hidden `legacy_cargo` publisher.
pub struct OvenLegacyCargoPrepareRequest<'a> {
    /// Bounded Oven store that will own the immutable result.
    pub store: &'a OvenStore,
    /// Generated-project receipt that authorizes the generated Rust root and direct-rustc intent.
    pub receipt: OvenReceipt,
    /// Caller-owned generated Rust project containing `Cargo.toml` and `src/main.rs`.
    pub generated_project: PathBuf,
    /// Explicit Cargo executable. Normal Oven commands never discover or invoke this tool.
    pub cargo: PathBuf,
    /// Explicit Rust compiler used by Cargo and later direct-rustc execution.
    pub rustc: PathBuf,
    /// Exact prebuilt SDK inventory supplied by the Loaf baker for compiler-suite publication.
    ///
    /// Standalone transitional callers may omit this and use the installed-toolchain discovery contract. Normal
    /// consumer commands never construct this publisher request.
    pub sdk_inventory: Option<PathBuf>,
    /// Explicit committed compiler Loaf envelope required by a compiler-suite publication.
    ///
    /// The named baker supplies this root after it atomically publishes the release-family envelope.  Requiring the
    /// exact root prevents the suite publisher from silently selecting a similarly shaped Loaf tree beside another
    /// compiler executable.
    pub compiler_loaf_root: Option<PathBuf>,
    /// Stable compatibility-domain policy bucket for the stored closure.
    pub domain: String,
    /// Explicit publisher operation; normal Oven consumers never receive this authority.
    pub publication_kind: OvenLegacyCargoPublicationKind,
    /// Named receipt source digest that authorizes the root later passed to direct rustc.
    pub source_evidence_key: String,
    /// Deterministic compile-time metadata required by the authorized root after ambient Cargo state is cleared.
    pub compile_environment: BTreeMap<String, String>,
    /// Exact checked Rust-inspection surface to seal into this plan.
    ///
    /// `Some` limits a narrow Loaf to its checked fixture surface, including an empty surface. `None` retains every
    /// registry rlib actually emitted into a broad compiler-suite foundation closure. A caller project partitioned
    /// against a base Loaf additionally seals its complete locked registry-source graph; neither form invents
    /// artifacts or resolves anything beyond this explicit publisher invocation. Normal Oven consumers never
    /// construct this request.
    pub inspection_packages: Option<Vec<OvenLegacyCargoInspectionPackage>>,
    /// Compiler-owned direct dependency closure to retain from the checked generated-project manifest.
    ///
    /// This controls the Rustc `--extern` surface only at the named publisher boundary. Normal consumers merely
    /// select the already sealed plan and never inspect a generated Cargo manifest or target directory.
    pub direct_dependency_closure: OvenLegacyCargoDirectDependencyClosure,
    /// Whether the named Loaf publisher may omit debug information from a debug-profile dependency closure.
    ///
    /// This affects only private `legacy_cargo` publisher artifacts; direct-rustc receipt identity and normal command
    /// semantics remain unchanged. It prevents compiler-shipped sealed Loaf data from consuming policy capacity with
    /// linker-irrelevant debug sections.
    pub compact_debug_info: bool,
    /// Whether this explicit source-built project publication must seal the compiler-owned vocabulary helper.
    ///
    /// This is admitted only by the source-built compiler's explicit Oven bake. The normal build, run, and test
    /// paths select the helper from the already sealed plan and never receive Cargo authority.
    pub source_compiler_vocab_support: bool,
    /// Optional immutable standard-library base selected before an explicit project bake.
    ///
    /// When present, the publisher substitutes the exact release-owned dependency cohort—compiler runtime,
    /// overlapping locked registry units, and vocabulary auxiliaries—then retains only the project's locked
    /// third-party and provider delta. The caller holds the base Loaf generation lock for the transaction; this
    /// request carries the immutable identity and plan evidence needed to partition those responsibilities.
    pub base_loaf: Option<OvenLegacyCargoBaseLoaf<'a>>,
}

/// One exact compiler-shipped Loaf selected as the base for a project-extension publication.
pub struct OvenLegacyCargoBaseLoaf<'a> {
    /// Content address of the selected `loaf.json`, not a crate or filesystem name.
    pub loaf_identity: String,
    /// Compatibility identity that authorized the selected base for this receipt.
    pub build_unit_identity: String,
    /// Verified complete direct-Rustc closure retained by the immutable base.
    pub artifacts: &'a OvenRustcArtifactManifest,
    /// Immutable directory containing every digest-verified base artifact.
    ///
    /// The publisher reads the sealed registry lock from this root before Cargo resolves a project extension. The
    /// root is request-local authority retained under the caller's generation lock; it is never serialized into the
    /// project payload.
    pub artifact_root: &'a Path,
}

/// Immutable payload retained by a receipt-bound project extension Loaf.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct OvenProjectExtensionPayload {
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

/// Outcome from a successful explicit `legacy_cargo` publication.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OvenLegacyCargoPrepareResult {
    /// Identity of the immutable receipt-bound Oven Loaf published by this transaction.
    pub plan_identity: String,
    /// Cargo version observed only at the explicit publisher boundary.
    pub cargo_version: String,
    /// Digest of the generated `Cargo.toml` used by the publisher.
    pub cargo_manifest_digest: String,
    /// Digest of the generated `Cargo.lock` written or verified by the publisher.
    pub cargo_lock_digest: String,
    /// Exact registry package artifacts observed by this named publisher invocation.
    ///
    /// The Loaf exporter seals this small catalog beside the copied direct-Rustc closure. It is never a
    /// normal-command Cargo resolution result.
    pub registry_leaves: Vec<OvenRustcRegistryLeaf>,
    /// Conservative transient publisher allocation high-water mark; this directory is removed before success returns.
    pub transient_reservation_bytes: u64,
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
}

/// Publisher-private foundation payload and its exact staged files, ready for separately bounded admission.
struct OvenCompilerTestSuiteFoundationPlan {
    payload: OvenCompilerTestSuiteFoundationPayload,
    materialized_files: Vec<OvenArtifactMaterializedFile>,
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

/// Publisher-private Loaf partition and its exact staged files, ready for separately bounded admission.
#[cfg(test)]
struct OvenCompilerTestSuiteToolchainDataPlan {
    materialized_files: Vec<OvenArtifactMaterializedFile>,
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

/// Split one publisher-verified compiler closure into deterministic foundation entries.
///
/// A foundation owns every byte it declares. Root shards retain the complete logical dependency declaration, but
/// receive only these exact foundations at execution time; they never recover a Cargo target or a copied composite
/// directory. Partitions retain their parent suite's one compatibility domain, so the store refuses the complete
/// closure when its aggregate exceeds the configured allowance rather than treating each label as a separate cache.
fn compiler_suite_foundation_plans(
    closure: &OvenCompilerTestSuiteArtifactClosure,
    materialized_files: &[OvenArtifactMaterializedFile],
    max_domain_logical_bytes: u64,
) -> Result<Vec<OvenCompilerTestSuiteFoundationPlan>, OvenLegacyCargoError> {
    let content_limit = max_domain_logical_bytes
        .checked_sub(COMPILER_TEST_SUITE_FOUNDATION_METADATA_HEADROOM_BYTES)
        .ok_or_else(|| {
            OvenLegacyCargoError::Plan(format!(
                "compiler foundation logical allowance {max_domain_logical_bytes} leaves no payload metadata headroom"
            ))
        })?;
    let mut files_by_relative_path = BTreeMap::new();
    for file in materialized_files {
        let metadata = fs::symlink_metadata(&file.source_path).map_err(|source| OvenLegacyCargoError::Io {
            path: file.source_path.clone(),
            source,
        })?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(OvenLegacyCargoError::InvalidInput {
                field: "compiler-suite foundation artifact",
                message: format!("{} must be a regular non-symlink file", file.source_path.display()),
            });
        }
        if files_by_relative_path
            .insert(file.relative_path.clone(), (file.clone(), metadata.len()))
            .is_some()
        {
            return Err(OvenLegacyCargoError::InvalidInput {
                field: "compiler-suite foundation artifact",
                message: format!("duplicates materialized path `{}`", file.relative_path),
            });
        }
    }

    let mut foundations = Vec::new();
    let mut current_artifacts = Vec::new();
    let mut current_files = Vec::new();
    let mut current_bytes = 0_u64;
    let mut used_paths = BTreeSet::new();
    for artifact in &closure.supporting_artifacts {
        let (file, logical_bytes) = files_by_relative_path.get(&artifact.relative_path).ok_or_else(|| {
            OvenLegacyCargoError::MissingDirectArtifact {
                crate_name: "compiler-suite foundation closure".to_string(),
                path: PathBuf::from(&artifact.relative_path),
            }
        })?;
        if !used_paths.insert(artifact.relative_path.clone()) {
            return Err(OvenLegacyCargoError::InvalidInput {
                field: "compiler-suite foundation closure",
                message: format!("declares duplicate artifact `{}`", artifact.relative_path),
            });
        }
        if *logical_bytes > content_limit {
            return Err(OvenLegacyCargoError::Plan(format!(
                "compiler foundation artifact `{}` is {} bytes, exceeding the {}-byte logical content allowance",
                artifact.relative_path, logical_bytes, content_limit
            )));
        }
        if !current_artifacts.is_empty() && current_bytes.saturating_add(*logical_bytes) > content_limit {
            foundations.push(OvenCompilerTestSuiteFoundationPlan {
                payload: OvenCompilerTestSuiteFoundationPayload {
                    schema_version: OVEN_COMPILER_TEST_SUITE_FOUNDATION_SCHEMA_VERSION,
                    label: format!("foundation-{:04}", foundations.len()),
                    artifact_closure: compiler_suite_foundation_closure(
                        closure,
                        std::mem::take(&mut current_artifacts),
                    ),
                },
                materialized_files: std::mem::take(&mut current_files),
            });
            current_bytes = 0;
        }
        current_bytes = current_bytes.saturating_add(*logical_bytes);
        current_artifacts.push(artifact.clone());
        current_files.push(file.clone());
    }
    if current_artifacts.is_empty() {
        return Err(OvenLegacyCargoError::Plan(
            "compiler foundation closure has no supporting artifacts to partition".to_string(),
        ));
    }
    if used_paths.len() != files_by_relative_path.len() {
        let unassigned = files_by_relative_path
            .keys()
            .filter(|path| !used_paths.contains(*path))
            .cloned()
            .collect::<Vec<_>>();
        return Err(OvenLegacyCargoError::InvalidInput {
            field: "compiler-suite foundation closure",
            message: format!("does not declare materialized artifact(s): {}", unassigned.join(", ")),
        });
    }
    foundations.push(OvenCompilerTestSuiteFoundationPlan {
        payload: OvenCompilerTestSuiteFoundationPayload {
            schema_version: OVEN_COMPILER_TEST_SUITE_FOUNDATION_SCHEMA_VERSION,
            label: format!("foundation-{:04}", foundations.len()),
            artifact_closure: compiler_suite_foundation_closure(closure, current_artifacts),
        },
        materialized_files: current_files,
    });
    Ok(foundations)
}

/// Split installed compiler-Loaf directories into deterministic schema-13 suite inputs.
///
/// This reader remains only to execute already-published schema-13 suite entries. New schema-14-and-later entries
/// record one exact, lease-held compiler Loaf generation instead of copying these directories into the receipt-bound
/// store.
#[cfg(test)]
fn compiler_suite_toolchain_data_plans(
    data_root: &Path,
    max_domain_logical_bytes: u64,
    expected_runtime_inputs: &BTreeMap<String, String>,
) -> Result<Vec<OvenCompilerTestSuiteToolchainDataPlan>, OvenLegacyCargoError> {
    compiler_suite_toolchain_data_plans_from_loaf_root(
        &data_root.join("share/incan/oven/loafs"),
        max_domain_logical_bytes,
        expected_runtime_inputs,
    )
}

/// Split one explicit committed compiler Loaf envelope into deterministic suite inputs.
#[cfg(test)]
fn compiler_suite_toolchain_data_plans_from_loaf_root(
    loafs: &Path,
    max_domain_logical_bytes: u64,
    expected_runtime_inputs: &BTreeMap<String, String>,
) -> Result<Vec<OvenCompilerTestSuiteToolchainDataPlan>, OvenLegacyCargoError> {
    let content_limit = max_domain_logical_bytes
        .checked_sub(COMPILER_TEST_SUITE_FOUNDATION_METADATA_HEADROOM_BYTES)
        .ok_or_else(|| {
            OvenLegacyCargoError::Plan(format!(
                "compiler Loaf logical allowance {max_domain_logical_bytes} leaves no payload metadata headroom"
            ))
        })?;
    if !loafs.is_dir() {
        return Err(OvenLegacyCargoError::InvalidInput {
            field: "compiler Loaf data",
            message: format!("{} is not a directory", loafs.display()),
        });
    }
    let committed = crate::oven::loaf::acquire_committed_loaf_generation(loafs)
        .map_err(|error| OvenLegacyCargoError::InvalidInput {
            field: "compiler Loaf data",
            message: error.to_string(),
        })?
        .ok_or_else(|| OvenLegacyCargoError::Plan("compiler Loaf data has no committed envelope".to_string()))?;
    let mut loaf_groups = Vec::new();
    for loaf_manifest in committed.paths() {
        let loaf_directory = loaf_manifest
            .parent()
            .ok_or_else(|| OvenLegacyCargoError::InvalidInput {
                field: "compiler Loaf data",
                message: format!("{} has no parent directory", loaf_manifest.display()),
            })?
            .to_path_buf();
        let loaf_name = loaf_directory
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| OvenLegacyCargoError::InvalidInput {
                field: "compiler Loaf data",
                message: format!("{} has a non-UTF-8 Loaf directory name", loaf_directory.display()),
            })?
            .to_string();
        let metadata = fs::symlink_metadata(&loaf_directory).map_err(|source| OvenLegacyCargoError::Io {
            path: loaf_directory.clone(),
            source,
        })?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(OvenLegacyCargoError::InvalidInput {
                field: "compiler Loaf data",
                message: format!("{} must be a non-symlink Loaf directory", loaf_directory.display()),
            });
        }
        let loaf_metadata = fs::symlink_metadata(loaf_manifest).map_err(|source| OvenLegacyCargoError::Io {
            path: loaf_manifest.clone(),
            source,
        })?;
        if loaf_metadata.file_type().is_symlink() || !loaf_metadata.is_file() {
            return Err(OvenLegacyCargoError::InvalidInput {
                field: "compiler Loaf data",
                message: format!("{} must contain a regular loaf.json", loaf_directory.display()),
            });
        }
        let loaf = serde_json::from_slice::<crate::oven::loaf::OvenLoaf>(&regular_file_bytes(loaf_manifest)?).map_err(
            |error| OvenLegacyCargoError::InvalidInput {
                field: "compiler Loaf data",
                message: format!("{} is not a valid sealed Loaf: {error}", loaf_manifest.display()),
            },
        )?;
        crate::oven::loaf::validate_stored_loaf(loaf_manifest, &loaf.build_unit_identity).map_err(|error| {
            OvenLegacyCargoError::InvalidInput {
                field: "compiler Loaf data",
                message: error.to_string(),
            }
        })?;
        validate_compiler_suite_loaf_runtime_inputs(&loaf_name, &loaf, expected_runtime_inputs)?;
        let relative_directory =
            loaf_directory
                .strip_prefix(loafs)
                .map_err(|_| OvenLegacyCargoError::InvalidInput {
                    field: "compiler Loaf data",
                    message: format!("{} escapes compiler data root", loaf_directory.display()),
                })?;
        let relative_root = Path::new("share/incan/oven/loafs")
            .join(relative_directory)
            .to_string_lossy()
            .to_string();
        let files = materialized_files_from_directory(&loaf_directory, &relative_root, "compiler-owned Loaf data")?;
        let logical_bytes = files.iter().try_fold(0_u64, |total, file| {
            let metadata = fs::symlink_metadata(&file.source_path).map_err(|source| OvenLegacyCargoError::Io {
                path: file.source_path.clone(),
                source,
            })?;
            Ok::<_, OvenLegacyCargoError>(total.saturating_add(metadata.len()))
        })?;
        if logical_bytes > content_limit {
            return Err(OvenLegacyCargoError::Plan(format!(
                "compiler Loaf `{loaf_name}` is {logical_bytes} bytes, exceeding the {content_limit}-byte logical content allowance"
            )));
        }
        loaf_groups.push((files, logical_bytes));
    }
    if loaf_groups.is_empty() {
        return Err(OvenLegacyCargoError::Plan(
            "compiler Loaf data has no sealed .loaf directories to partition".to_string(),
        ));
    }

    let control_files = [loafs.join("envelope.json"), loafs.join(".envelope.lock")]
        .into_iter()
        .map(|source_path| {
            let relative = source_path
                .strip_prefix(loafs)
                .map_err(|_| OvenLegacyCargoError::InvalidInput {
                    field: "compiler Loaf data",
                    message: format!("{} escapes compiler data root", source_path.display()),
                })?;
            let relative_path = Path::new("share/incan/oven/loafs")
                .join(relative)
                .to_string_lossy()
                .to_string();
            Ok(OvenArtifactMaterializedFile {
                source_path,
                relative_path,
            })
        })
        .collect::<Result<Vec<_>, OvenLegacyCargoError>>()?;
    loaf_groups[0].0.extend(control_files);

    let mut plans = Vec::new();
    let mut current_files = Vec::new();
    let mut current_bytes = 0_u64;
    for (files, logical_bytes) in loaf_groups {
        if !current_files.is_empty() && current_bytes.saturating_add(logical_bytes) > content_limit {
            plans.push(OvenCompilerTestSuiteToolchainDataPlan {
                materialized_files: std::mem::take(&mut current_files),
            });
            current_bytes = 0;
        }
        current_bytes = current_bytes.saturating_add(logical_bytes);
        current_files.extend(files);
    }
    plans.push(OvenCompilerTestSuiteToolchainDataPlan {
        materialized_files: current_files,
    });
    Ok(plans)
}

/// Validate the compiler-owned standard-library generation that a schema-14-or-later suite will lease at execution
/// time.
///
/// The suite index records this immutable generation identity instead of republishing its 1+ GiB contents into the
/// receipt-bound store.  The same checks that protected schema-13 copied partitions run here before the index is
/// committed, and the runner repeats generation selection while retaining the shared envelope lock.
fn compiler_suite_toolchain_loaf_generation_reference(
    loaf_root: &Path,
    expected_runtime_inputs: &BTreeMap<String, String>,
) -> Result<OvenCompilerTestSuiteToolchainLoafGenerationReference, OvenLegacyCargoError> {
    let committed = crate::oven::loaf::acquire_committed_loaf_generation(loaf_root)
        .map_err(|error| OvenLegacyCargoError::InvalidInput {
            field: "compiler Loaf data",
            message: error.to_string(),
        })?
        .ok_or_else(|| OvenLegacyCargoError::Plan("compiler Loaf data has no committed envelope".to_string()))?;
    if committed.paths().is_empty() {
        return Err(OvenLegacyCargoError::Plan(
            "compiler Loaf data has no sealed .loaf directories".to_string(),
        ));
    }
    for loaf_manifest in committed.paths() {
        let loaf = serde_json::from_slice::<crate::oven::loaf::OvenLoaf>(&regular_file_bytes(loaf_manifest)?).map_err(
            |error| OvenLegacyCargoError::InvalidInput {
                field: "compiler Loaf data",
                message: format!("{} is not a valid sealed Loaf: {error}", loaf_manifest.display()),
            },
        )?;
        crate::oven::loaf::validate_stored_loaf(loaf_manifest, &loaf.build_unit_identity).map_err(|error| {
            OvenLegacyCargoError::InvalidInput {
                field: "compiler Loaf data",
                message: error.to_string(),
            }
        })?;
        let loaf_name = loaf_manifest
            .parent()
            .and_then(Path::file_name)
            .and_then(|name| name.to_str())
            .ok_or_else(|| OvenLegacyCargoError::InvalidInput {
                field: "compiler Loaf data",
                message: format!("{} has a non-UTF-8 Loaf directory name", loaf_manifest.display()),
            })?;
        validate_compiler_suite_loaf_runtime_inputs(loaf_name, &loaf, expected_runtime_inputs)?;
    }
    Ok(OvenCompilerTestSuiteToolchainLoafGenerationReference {
        generation_identity: committed.generation_identity().to_string(),
    })
}

/// Derive the Loaf compatibility inputs from the runtime closure sealed in this suite's SDK inventory.
///
/// The compiler-suite publisher stages that runtime closure as a self-contained immutable input. Its Loafs must
/// describe the same lockfile and compiler-runtime source trees; accepting a nearby toolchain's Loaf
/// would make child selection depend on ambient state and later fail closed only after the suite was admitted.
fn compiler_suite_staged_runtime_inputs(
    staged_sdk_root: &Path,
) -> Result<BTreeMap<String, String>, OvenLegacyCargoError> {
    let runtime_root = staged_sdk_root.join("runtime");
    let runtime_lock = runtime_root.join("Cargo.lock");
    let mut inputs = BTreeMap::new();
    inputs.insert("compiler-version".to_string(), INCAN_VERSION.to_string());
    inputs.insert(
        "sdk-provider-codegen-revision".to_string(),
        SDK_PROVIDER_CODEGEN_REVISION.to_string(),
    );
    for (input_name, crate_name) in [
        ("runtime-source-incan-core", "incan_core"),
        ("runtime-source-incan-derive", "incan_derive"),
        ("runtime-source-incan-stdlib", "incan_stdlib"),
    ] {
        let source_root = runtime_root.join("crates").join(crate_name);
        let digest = crate::oven::loaf::digest_runtime_crate_source(&source_root).map_err(|message| {
            OvenLegacyCargoError::InvalidInput {
                field: "compiler-suite SDK runtime closure",
                message,
            }
        })?;
        inputs.insert(input_name.to_string(), digest);
    }
    let lock_bytes = regular_file_bytes(&runtime_lock)?;
    inputs.insert("runtime-lock".to_string(), digest_bytes(&lock_bytes));
    Ok(inputs)
}

/// Refuse Loaf data from a different compiler runtime than the staged SDK inventory.
///
/// Compatibility is intentionally exact here. Provider modules can be an explicitly authorized Loaf superset, but
/// a runtime source or lockfile mismatch would combine two compiler/package worlds and cannot be repaired by runtime
/// selection. The named baker must regenerate the Loaf with the sealed SDK inventory instead.
fn validate_compiler_suite_loaf_runtime_inputs(
    loaf_name: &str,
    loaf: &crate::oven::loaf::OvenLoaf,
    expected_runtime_inputs: &BTreeMap<String, String>,
) -> Result<(), OvenLegacyCargoError> {
    if loaf.compatibility.runtime_inputs == *expected_runtime_inputs {
        return Ok(());
    }
    let mismatched = expected_runtime_inputs
        .iter()
        .filter_map(|(key, expected)| {
            let actual = loaf.compatibility.runtime_inputs.get(key);
            (actual != Some(expected)).then(|| {
                format!(
                    "{key}: expected {expected}, found {}",
                    actual.map_or("<missing>", String::as_str)
                )
            })
        })
        .collect::<Vec<_>>();
    let unexpected = loaf
        .compatibility
        .runtime_inputs
        .keys()
        .filter(|key| !expected_runtime_inputs.contains_key(*key))
        .map(|key| format!("{key}: unexpected"))
        .collect::<Vec<_>>();
    let details = mismatched.into_iter().chain(unexpected).collect::<Vec<_>>().join(", ");
    Err(OvenLegacyCargoError::Plan(format!(
        "compiler Loaf `{loaf_name}` is incompatible with the staged SDK runtime closure ({details}); regenerate it through the internal compatibility publisher with the same SDK inventory"
    )))
}

/// Restrict a foundation's direct-rustc search directories to paths that actually contain its selected files.
///
/// A composed runner passes every foundation root separately to Rustc. Retaining an empty sibling `deps` directory
/// would make the strict trusted materializer accept an undeclared directory rather than a real dependency input.
fn compiler_suite_foundation_closure(
    closure: &OvenCompilerTestSuiteArtifactClosure,
    supporting_artifacts: Vec<OvenRustcSupportingArtifact>,
) -> OvenCompilerTestSuiteArtifactClosure {
    let contains_artifact = |directory: &str| {
        let prefix = format!("{}/", directory.trim_end_matches('/'));
        supporting_artifacts
            .iter()
            .any(|artifact| artifact.relative_path.starts_with(&prefix))
    };
    OvenCompilerTestSuiteArtifactClosure {
        dependency_search_paths: closure
            .dependency_search_paths
            .iter()
            .filter(|directory| contains_artifact(directory))
            .cloned()
            .collect(),
        native_search_paths: closure
            .native_search_paths
            .iter()
            .filter(|directory| contains_artifact(directory))
            .cloned()
            .collect(),
        supporting_artifacts,
    }
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

/// Successful explicit publication of a compiler libtest runtime pair.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OvenLegacyCargoCompilerSuiteResult {
    /// Identity of the bounded, immutable compiler-suite runtime artifact.
    pub suite_identity: String,
    /// Cargo version observed only at the explicit publisher boundary.
    pub cargo_version: String,
    /// Digest of the compiler Cargo.toml observed by the publisher.
    pub cargo_manifest_digest: String,
    /// Digest of the compiler Cargo.lock observed by the publisher.
    pub cargo_lock_digest: String,
    /// Conservative transient publisher allocation high-water mark; the Cargo target is removed before success
    /// returns.
    pub transient_reservation_bytes: u64,
    /// Product-owned phase timing for the compiler-suite publisher.
    pub timing: OvenLegacyCargoCompilerSuiteTiming,
}

/// Attribution for the explicit compiler-suite publisher after its enclosing Loaf family is available.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct OvenLegacyCargoCompilerSuiteTiming {
    /// Receipt validation, private staging, sealed SDK materialization, and toolchain-data planning.
    pub preflight_and_sdk_elapsed_ms: u128,
    /// Locked Cargo unit-graph discovery only; Cargo does not compile roots in this phase.
    pub unit_graph_elapsed_ms: u128,
    /// The one permitted third-party foundation compilation.
    pub foundation_build_elapsed_ms: u128,
    /// Direct-Rustc root planning, foundation partitioning, and immutable request construction.
    pub direct_plan_elapsed_ms: u128,
    /// Capacity admission and atomic store publication of the complete suite closure.
    pub store_publication_elapsed_ms: u128,
}

/// Failure while preparing a bounded Oven closure through the temporary explicit Cargo boundary.
#[derive(Debug, thiserror::Error)]
pub enum OvenLegacyCargoError {
    /// The caller supplied an invalid publisher input.
    #[error("invalid Oven internal compatibility publisher {field}: {message}")]
    InvalidInput { field: &'static str, message: String },
    /// A receipt does not authorize the requested generated source.
    #[error("Oven internal compatibility publisher receipt mismatch: {message}")]
    ReceiptMismatch { message: String },
    /// Filesystem access failed at an explicit publisher path.
    #[error("Oven internal compatibility publisher I/O failed at {path}: {source}")]
    Io { path: PathBuf, source: io::Error },
    /// Cargo failed while the explicitly named publisher was running.
    #[error("Oven internal compatibility publisher failed: {output}")]
    CargoFailed { output: String },
    /// Temporary publisher storage reached its enforced compatibility-domain allowance.
    #[error(
        "Oven internal compatibility publisher physical reservation at {path} reached {observed_physical_bytes} bytes, exceeding the transient compatibility allowance of {limit_bytes} bytes"
    )]
    TransientCapacityExceeded {
        /// Publisher-owned target or prepared-staging path measured without following links.
        path: PathBuf,
        /// Conservative physical reservation observed when the publisher failed closed.
        observed_physical_bytes: u64,
        /// Enforced compatibility-domain physical allowance.
        limit_bytes: u64,
    },
    /// An expected compiled direct dependency was not available in Cargo's fresh target.
    #[error("Oven internal compatibility publisher could not materialize direct dependency `{crate_name}` from {path}")]
    MissingDirectArtifact { crate_name: String, path: PathBuf },
    /// The bounded Oven store rejected or could not publish the immutable result.
    #[error("Oven internal compatibility publisher store failure: {0}")]
    Store(#[from] OvenStoreError),
    /// A direct-rustc plan could not be serialized or validated.
    #[error("Oven internal compatibility publisher direct-rustc plan failure: {0}")]
    Plan(String),
}

/// Select an already published plan through the exact rule for this request's publication shape.
fn select_existing_direct_rustc_plan_identity(
    request: &OvenLegacyCargoPrepareRequest<'_>,
) -> Result<Option<String>, OvenLegacyCargoError> {
    match request.base_loaf.as_ref() {
        Some(base) => select_existing_project_extension_identity(request.store, &request.receipt, base),
        None => match select_direct_rustc_plan_identity(request.store, &request.receipt) {
            Ok(plan_identity) => {
                if request.source_compiler_vocab_support
                    && !stored_plan_supplies_source_compiler_vocab_support(
                        request.store,
                        &plan_identity,
                        &request.receipt.intent,
                    )?
                {
                    return Ok(None);
                }
                Ok(Some(plan_identity))
            }
            Err(super::rustc::OvenRustcError::PlanSelection { message, .. })
                if message == "no compatible stored direct-rustc plan is available" =>
            {
                Ok(None)
            }
            Err(error) => Err(OvenLegacyCargoError::Plan(error.to_string())),
        },
    }
}

/// Return whether a reusable direct plan already seals the host vocabulary closure required by this source build.
///
/// The base plan selector intentionally shares a build-unit identity across generated source revisions. A
/// pre-patch source-built plan may therefore remain otherwise receipt-compatible while lacking the compiler-owned
/// helper. Only the explicit publisher uses this capability check; normal consumers select an already prepared plan
/// and never use it to acquire Cargo authority.
fn stored_plan_supplies_source_compiler_vocab_support(
    store: &OvenStore,
    plan_identity: &str,
    intent: &OvenBuildIntent,
) -> Result<bool, OvenLegacyCargoError> {
    let (manifest, _artifact_root, payload, _lease) = store.select_payload_for_execution(plan_identity)?;
    if manifest.kind != OvenArtifactKind::DirectRustcPlan || manifest.intent != *intent {
        return Ok(false);
    }
    let plan = serde_json::from_slice::<OvenRustcArtifactManifest>(&payload).map_err(|error| {
        OvenLegacyCargoError::Plan(format!(
            "stored direct-rustc plan `{plan_identity}` has an invalid payload while checking vocabulary support: {error}"
        ))
    })?;
    Ok(plan.vocab_auxiliary_targets.iter().any(|target| {
        target.target == intent.target
            && ["incan_vocab", "serde_json"]
                .into_iter()
                .all(|crate_name| target.externs.iter().any(|artifact| artifact.crate_name == crate_name))
    }))
}

/// Return the observable no-publisher result for one exact existing plan.
fn reused_direct_rustc_plan_result(plan_identity: String) -> OvenLegacyCargoPrepareResult {
    OvenLegacyCargoPrepareResult {
        plan_identity,
        cargo_version: "not-run-existing-plan".to_string(),
        cargo_manifest_digest: "not-run-existing-plan".to_string(),
        cargo_lock_digest: "not-run-existing-plan".to_string(),
        registry_leaves: Vec::new(),
        transient_reservation_bytes: 0,
    }
}

/// Return the compiler checkout whose checked workspace lock owns a source-built vocabulary helper bake.
///
/// `CARGO_MANIFEST_DIR` is compiled into the executable, so this does not accept a caller-selected source directory.
/// A packaged compiler does not carry the checked workspace layout and must instead select a release-cohort Loaf.
fn source_compiler_vocab_support_root() -> Result<PathBuf, OvenLegacyCargoError> {
    let root = canonical_directory(Path::new(env!("CARGO_MANIFEST_DIR")), "compiler source root")?;
    if !source_compiler_vocab_support_is_available() {
        return Err(OvenLegacyCargoError::Plan(format!(
            "source-built compiler vocabulary support requires the checked compiler workspace at {}; no release-cohort Loaf is available",
            root.display()
        )));
    }
    Ok(root)
}

/// Return whether this executable was built from the checked compiler workspace that owns `incan_vocab`.
///
/// This is a compiler-origin capability rather than caller input. It distinguishes a source-built compiler, which
/// may seal its checked helper closure at the explicit publisher boundary, from a packaged compiler that must rely
/// on a shipped release-cohort Loaf.
pub(crate) fn source_compiler_vocab_support_is_available() -> bool {
    let Ok(root) = fs::canonicalize(Path::new(env!("CARGO_MANIFEST_DIR"))) else {
        return false;
    };
    let Ok(executable) = std::env::current_exe().and_then(fs::canonicalize) else {
        return false;
    };
    source_compiler_vocab_support_paths_are_available(&root, &executable)
}

/// Return whether `executable` is a source-build binary beneath the checked compiler workspace at `root`.
fn source_compiler_vocab_support_paths_are_available(root: &Path, executable: &Path) -> bool {
    root.join("Cargo.lock").is_file()
        && root.join("crates/incan_vocab/Cargo.toml").is_file()
        && executable.starts_with(root.join("target"))
}

/// Prepare and publish exactly one receipt-bound direct-rustc closure through the hidden `legacy_cargo` boundary.
///
/// Publication is idempotent: a plain direct plan may reuse the compatible generated-project unit it was published
/// for, while a base-partitioned project extension must match its exact receipt and base. A broad compiler Loaf can
/// share a generated build-unit identity with a caller project without owning that project's complete dependency
/// closure. If multiple valid candidates match the applicable rule, the publisher refuses to guess.
pub fn prepare_direct_rustc_plan(
    request: &OvenLegacyCargoPrepareRequest<'_>,
) -> Result<OvenLegacyCargoPrepareResult, OvenLegacyCargoError> {
    request
        .receipt
        .verify_identity()
        .map_err(|error| OvenLegacyCargoError::ReceiptMismatch {
            message: error.to_string(),
        })?;
    let supported_compatibility = matches!(
        request.receipt.compatibility.kind,
        OvenCompatibilityKind::GeneratedIncanProject | OvenCompatibilityKind::NativeCompilerTestSuite
    );
    if !supported_compatibility
        || (request.receipt.compatibility.cargo_input_only
            && request.receipt.compatibility.kind != OvenCompatibilityKind::NativeCompilerTestSuite)
    {
        return Err(OvenLegacyCargoError::ReceiptMismatch {
            message: "compatibility publication requires a generated-project or native compiler-suite receipt"
                .to_string(),
        });
    }
    let library_tests = request.publication_kind == OvenLegacyCargoPublicationKind::LibraryTests;
    if (request.receipt.compatibility.kind == OvenCompatibilityKind::NativeCompilerTestSuite) != library_tests {
        return Err(OvenLegacyCargoError::ReceiptMismatch {
            message: "native compiler-suite receipts require the explicit library-test publisher; generated-project receipts require an executable or interop-bootstrap publisher".to_string(),
        });
    }
    let rustc_identity = rustc_identity(&request.rustc).map_err(|error| OvenLegacyCargoError::InvalidInput {
        field: "rustc",
        message: error.to_string(),
    })?;
    if rustc_identity != request.receipt.intent.toolchain {
        return Err(OvenLegacyCargoError::ReceiptMismatch {
            message: format!(
                "receipt requires Rust compiler `{}`, but --rustc reports `{rustc_identity}`",
                request.receipt.intent.toolchain
            ),
        });
    }
    if let Some(plan_identity) = select_existing_direct_rustc_plan_identity(request)? {
        return Ok(reused_direct_rustc_plan_result(plan_identity));
    }

    let generated_project = canonical_directory(&request.generated_project, "generated project")?;
    let cargo_manifest = generated_project.join("Cargo.toml");
    let cargo_manifest_bytes = regular_file_bytes(&cargo_manifest)?;
    let staged_metadata = if let Some(base) = request.base_loaf.as_ref() {
        let base_lock = verified_release_cohort_registry_lock(base)?;
        Some(stage_release_cohort_project_lock(
            &request.cargo,
            &generated_project,
            &base_lock,
            &request.receipt.intent.features,
        )?)
    } else {
        None
    };
    let _ =
        receipt_authorized_generated_root_bytes(&generated_project, &request.receipt, &request.source_evidence_key)?;
    let cargo_version = tool_version(&request.cargo, "cargo")?;
    let declared_direct_dependencies = cargo_direct_dependency_names(
        &cargo_manifest_bytes,
        request.publication_kind == OvenLegacyCargoPublicationKind::LibraryTests,
    )?;
    let mut direct_dependencies = publisher_direct_dependencies(
        &generated_project,
        declared_direct_dependencies.clone(),
        request.publication_kind,
        request.direct_dependency_closure,
    )?;
    if request.publication_kind == OvenLegacyCargoPublicationKind::LibraryTests {
        // The direct-rustc compiler CLI is a binary target in this same package. Cargo emits its library as
        // `libincan-*` during the explicit publisher build; recording that self-library lets normal test setup bake
        // `src/main.rs` without consulting a Cargo target directory.
        direct_dependencies.insert(
            request.receipt.project.name.replace('-', "_"),
            request.receipt.project.name.clone(),
        );
    }
    let staging_parent = request.store.root().join("legacy-cargo-staging");
    let publisher_lock = acquire_publisher_lock(&staging_parent)?;
    // The fast lookup above deliberately precedes manifest inspection. Recheck after taking the cross-process
    // publisher lock so a concurrent winner cannot make this request rebuild and publish a second byte-distinct
    // extension for the same exact receipt and build unit.
    if let Some(plan_identity) = select_existing_direct_rustc_plan_identity(request)? {
        return Ok(reused_direct_rustc_plan_result(plan_identity));
    }
    reclaim_stale_publisher_staging(&staging_parent)?;
    let publisher_reservation = request
        .store
        .reserve_legacy_cargo_publisher_capacity(&request.domain)
        .map_err(OvenLegacyCargoError::Store)?;
    let staging = create_publisher_staging(&staging_parent)?;
    let cleanup = PublisherStagingCleanup { path: staging.clone() };
    let target = staging.join("target");
    let transient_limit = publisher_reservation.transient_limit_bytes;
    let cargo_outputs = run_legacy_cargo(
        &request.cargo,
        &request.rustc,
        &cargo_manifest,
        &target,
        &request.receipt.intent.target,
        &request.receipt.intent.profile,
        &request.receipt.intent.features,
        transient_limit,
        request.publication_kind,
        request.compact_debug_info,
        request.base_loaf.is_some(),
    )?;
    let cargo_lock = generated_project.join("Cargo.lock");
    let cargo_lock_bytes = regular_file_bytes(&cargo_lock)?;
    let profile_directory = cargo_profile_directory(&request.receipt.intent.profile)?;
    let requested_target_deps = target
        .join(&request.receipt.intent.target)
        .join(profile_directory)
        .join("deps");
    // Cross-target Cargo builds put target libraries and host-side procedural macros in separate dependency
    // directories. Direct rustc needs both: the target directory supplies root `--extern` inputs, and the host
    // directory supplies the proc-macro dylibs needed while expanding dependency metadata.
    let host_deps = target.join(profile_directory).join("deps");
    let rustc_host = rustc_host_target(&request.rustc)
        .map_err(|error| OvenLegacyCargoError::Plan(format!("cannot identify publisher Rust host target: {error}")))?;
    // Cargo is allowed to omit the redundant target-triple directory for a host-target build. Treat that layout as
    // the target closure only when the receipt target is exactly the publisher host; a cross-target request still
    // fails closed if it did not produce its target-specific output directory.
    let target_deps = if requested_target_deps.is_dir() {
        requested_target_deps
    } else if request.receipt.intent.target == rustc_host && host_deps.is_dir() {
        host_deps.clone()
    } else {
        requested_target_deps
    };
    // A dependency-free generated root can leave Cargo with no `deps` directory at all. Its empty direct-rustc
    // closure is still valid: the later source compilation needs neither `-L dependency` nor `--extern`. Create a
    // private empty directory solely so the shared closure reader can represent that zero-dependency case without
    // weakening the missing-artifact check for a declared direct dependency.
    if direct_dependencies.is_empty() && !target_deps.exists() {
        fs::create_dir_all(&target_deps).map_err(|source| OvenLegacyCargoError::Io {
            path: target_deps.clone(),
            source,
        })?;
    }
    let mut dependency_directories = vec![target_deps.clone()];
    if host_deps.is_dir() && host_deps != target_deps {
        dependency_directories.push(host_deps);
    }
    let metadata = match staged_metadata {
        Some(metadata) => metadata,
        None => read_legacy_cargo_metadata(&request.cargo, &cargo_manifest, &request.receipt.intent.features)?,
    };
    let resolved_direct_dependencies = resolve_direct_dependency_packages(&metadata, &direct_dependencies)?;
    let reported_artifact_files = publisher_output_artifact_paths(&cargo_outputs, &request.receipt.intent.profile)?;
    let (dependency_search_paths, externs, mut supporting_artifacts) = if reported_artifact_files.is_empty() {
        // Cargo's JSON protocol is the normal authority for an explicit bake. Retain the directory reader only as
        // a compatibility path for a dependency-free build whose Cargo version emits no compiler-artifact messages.
        artifact_closure(
            &staging,
            &target_deps,
            &dependency_directories,
            &direct_dependencies,
            request.publication_kind == OvenLegacyCargoPublicationKind::LibraryTests,
        )?
    } else {
        artifact_closure_from_reported_paths(
            &staging,
            &request.receipt.intent.target,
            &request.receipt.intent.profile,
            &resolved_direct_dependencies,
            request.publication_kind == OvenLegacyCargoPublicationKind::LibraryTests,
            &cargo_outputs,
        )?
    };
    let (registry_leaves, registry_source_artifacts) =
        publisher_registry_leaf_catalog(PublisherRegistryLeafCatalogRequest {
            outputs: &cargo_outputs,
            metadata: &metadata,
            cargo_lock: &cargo_lock_bytes,
            staging: &staging,
            intent: &request.receipt.intent,
            rustc_host: &rustc_host,
            externs: &externs,
            supporting_artifacts: &supporting_artifacts,
            inspection_packages: request.inspection_packages.as_deref(),
        })?;
    supporting_artifacts.extend(registry_source_artifacts);
    // The complete-graph source catalog only runs when sealing against a base release Loaf (`base_loaf.is_some()`).
    // Fetch Cargo's own platform-filtered resolve for the receipt's exact target so that closure reflects what this
    // target's build actually requires, not every platform's locked dependencies.
    let platform_filtered_metadata = if request.base_loaf.is_some() {
        Some(read_legacy_cargo_metadata_for_platform(
            &request.cargo,
            &cargo_manifest,
            &request.receipt.intent.features,
            true,
            Some(request.receipt.intent.target.as_str()),
        )?)
    } else {
        None
    };
    let (registry_sources, transitive_registry_source_artifacts) = publisher_registry_source_catalog(
        &metadata,
        &cargo_lock_bytes,
        &staging,
        request.inspection_packages.as_deref(),
        &registry_leaves,
        request.base_loaf.is_some(),
        platform_filtered_metadata.as_ref(),
    )?;
    supporting_artifacts.extend(transitive_registry_source_artifacts);
    if !registry_sources.is_empty() {
        let registry_lock_path = staging.join(OVEN_RUSTC_REGISTRY_LOCK_RELATIVE_PATH);
        if let Some(parent) = registry_lock_path.parent() {
            fs::create_dir_all(parent).map_err(|source| OvenLegacyCargoError::Io {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        fs::write(&registry_lock_path, &cargo_lock_bytes).map_err(|source| OvenLegacyCargoError::Io {
            path: registry_lock_path,
            source,
        })?;
    }
    let provenance_path = staging.join("provenance/legacy-cargo.json");
    let provenance = OvenLegacyCargoProvenance {
        schema_version: OVEN_LEGACY_CARGO_PROVENANCE_SCHEMA_VERSION,
        boundary: "legacy_cargo".to_string(),
        cargo_version: cargo_version.clone(),
        cargo_manifest_digest: digest_bytes(&cargo_manifest_bytes),
        cargo_lock_digest: digest_bytes(&cargo_lock_bytes),
        publication_kind: request.publication_kind,
        target: request.receipt.intent.target.clone(),
        toolchain: request.receipt.intent.toolchain.clone(),
        profile: request.receipt.intent.profile.clone(),
    };
    write_provenance(&provenance_path, &provenance)?;
    let has_registry_sources = !registry_sources.is_empty();
    let mut plan = OvenRustcArtifactManifest {
        schema_version: OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION,
        intent: request.receipt.intent.clone(),
        dependency_search_paths,
        native_search_paths: Vec::new(),
        externs,
        entrypoint_externs: BTreeMap::new(),
        registry_leaves: registry_leaves.clone(),
        registry_sources,
        compile_environment: request.compile_environment.clone(),
        vocab_auxiliary_targets: Vec::new(),
        supporting_artifacts,
    };
    if request.source_compiler_vocab_support {
        if request.base_loaf.is_some() {
            return Err(OvenLegacyCargoError::Plan(
                "a project extension cannot bake a source compiler vocabulary helper beside its selected release-cohort Loaf"
                    .to_string(),
            ));
        }
        let compiler_root = source_compiler_vocab_support_root()?;
        let compiler_support_target = staging.join("compiler-vocab-target");
        crate::oven::loaf::bake_source_compiler_vocab_support(
            crate::oven::loaf::OvenSourceCompilerVocabSupportRequest {
                plan: &mut plan,
                loaf_staging: &staging,
                compiler_root: &compiler_root,
                cargo: &request.cargo,
                rustc: &request.rustc,
                cargo_target: &compiler_support_target,
                capacity_roots: &[&staging],
                transient_limit,
            },
        )
        .map_err(|error| OvenLegacyCargoError::Plan(error.to_string()))?;
    }
    canonicalize_supporting_artifacts(&mut plan.supporting_artifacts)?;
    let (kind, payload, materialized_plan) = if let Some(base) = request.base_loaf.as_ref() {
        if base.artifacts.intent != request.receipt.intent {
            return Err(OvenLegacyCargoError::ReceiptMismatch {
                message: "selected project-extension base Loaf has an incompatible direct-Rustc intent".to_string(),
            });
        }
        let root_registry_packages = declared_direct_dependencies
            .values()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>();
        let complete_plan = plan
            .with_release_cohort_from_base(base.artifacts, &root_registry_packages)
            .map_err(|error| OvenLegacyCargoError::Plan(error.to_string()))?;
        let partition = complete_plan
            .partition_against_base(base.artifacts)
            .map_err(|error| OvenLegacyCargoError::Plan(error.to_string()))?;
        let registry_source_dependencies = project_registry_source_dependencies(
            &metadata,
            &declared_direct_dependencies,
            &complete_plan.registry_sources,
        )?;
        let cargo_manifest =
            std::str::from_utf8(&cargo_manifest_bytes).map_err(|error| OvenLegacyCargoError::InvalidInput {
                field: "Cargo.toml",
                message: format!("must be UTF-8: {error}"),
            })?;
        let cargo_manifest =
            toml::from_str::<toml::Value>(cargo_manifest).map_err(|error| OvenLegacyCargoError::InvalidInput {
                field: "Cargo.toml",
                message: format!("must be valid TOML: {error}"),
            })?;
        let dev_registry_source_dependencies = project_registry_source_dependencies(
            &metadata,
            &direct_dependency_aliases(&cargo_manifest, "dev-dependencies"),
            &complete_plan.registry_sources,
        )?;
        if partition.base_paths.is_empty() {
            return Err(OvenLegacyCargoError::Plan(
                "selected standard-library Loaf shares no byte-identical artifacts with this project closure; refuse a duplicate whole-closure project bake"
                    .to_string(),
            ));
        }
        if partition.extension_paths.is_empty() {
            return Err(OvenLegacyCargoError::Plan(
                "explicit project bake produced no non-stdlib artifacts; consume the selected standard-library Loaf directly"
                    .to_string(),
            ));
        }
        // ---- Stage re-rooted collision artifacts before atomic copying ----
        // Cohort composition moves a salted extension artifact whose filename collides with the base's execution
        // closure into its `extension-deps` sibling directory, but the built file still sits in the Cargo `deps`
        // output. Materialization resolves every source as the staging root joined with the recorded relative path,
        // so link each re-rooted artifact into its recorded home here.
        for relative_path in &partition.extension_paths {
            let Some(source) = rerooted_artifact_staging_source(relative_path) else {
                continue;
            };
            let source_path = staging.join(&source);
            let target_path = staging.join(relative_path);
            if let Some(parent) = target_path.parent() {
                fs::create_dir_all(parent).map_err(|source| OvenLegacyCargoError::Io {
                    path: parent.to_path_buf(),
                    source,
                })?;
            }
            if fs::hard_link(&source_path, &target_path).is_err() {
                fs::copy(&source_path, &target_path).map_err(|source| OvenLegacyCargoError::Io {
                    path: target_path.clone(),
                    source,
                })?;
            }
        }
        let materialized_plan = complete_plan
            .artifact_fragment(&partition.extension_paths)
            .map_err(|error| OvenLegacyCargoError::Plan(error.to_string()))?;
        let payload = serde_json::to_vec(&OvenProjectExtensionPayload {
            schema_version: OVEN_PROJECT_EXTENSION_PAYLOAD_SCHEMA_VERSION,
            base_loaf_identity: base.loaf_identity.clone(),
            base_build_unit_identity: base.build_unit_identity.clone(),
            publisher_plan: plan,
            complete_plan,
            registry_source_dependencies,
            dev_registry_source_dependencies,
            extension_paths: partition.extension_paths.into_iter().collect(),
        })
        .map_err(|error| OvenLegacyCargoError::Plan(error.to_string()))?;
        (OvenArtifactKind::ProjectPayload, payload, materialized_plan)
    } else {
        let payload = serde_json::to_vec(&plan).map_err(|error| OvenLegacyCargoError::Plan(error.to_string()))?;
        (OvenArtifactKind::DirectRustcPlan, payload, plan)
    };
    let mut materialized_files = materialized_plan
        .materialized_artifacts(&staging, &request.receipt.intent)
        .map_err(|error| OvenLegacyCargoError::Plan(error.to_string()))?
        .into_iter()
        .map(|artifact| OvenArtifactMaterializedFile {
            source_path: artifact.source_path,
            relative_path: artifact.relative_path,
        })
        .collect::<Vec<_>>();
    materialize_sealed_registry_lock(&staging, has_registry_sources, &mut materialized_files)?;
    // Publisher metadata belongs in the immutable Loaf, but it is not a direct-Rustc input. Keeping it out of the
    // executable artifact plan lets a project extension share the base closure even though its own Cargo lock and
    // publication receipt naturally differ. The store manifest still digests and verifies this copied file.
    materialized_files.push(OvenArtifactMaterializedFile {
        source_path: provenance_path,
        relative_path: "provenance/legacy-cargo.json".to_string(),
    });
    let publication = OvenArtifactPublishRequest {
        receipt: request.receipt.clone(),
        domain: request.domain.clone(),
        kind,
        payload,
        materialized_files,
    };
    let transient_reservation_bytes = conservative_directory_reservation(&staging)?;
    request
        .store
        .ensure_legacy_cargo_batch_physical_capacity(&staging, std::slice::from_ref(&publication))?;
    let artifact = request.store.publish_from_legacy_cargo(&publication)?;
    drop(cleanup);
    drop(publisher_lock);
    Ok(OvenLegacyCargoPrepareResult {
        plan_identity: artifact.identity,
        cargo_version,
        cargo_manifest_digest: digest_bytes(&cargo_manifest_bytes),
        cargo_lock_digest: digest_bytes(&cargo_lock_bytes),
        registry_leaves,
        transient_reservation_bytes,
    })
}

/// Return a reusable project extension only when its exact project receipt and selected standard-library base match.
///
/// `DirectRustcPlan` entries are intentionally excluded. They may be valid compiler-owned Loafs for a compatible
/// generated unit, yet omit a provider's transitive source/metadata authority. Accepting one here would suppress
/// the only publisher transaction that can seal the caller's complete closure.
fn select_existing_project_extension_identity(
    store: &OvenStore,
    receipt: &OvenReceipt,
    base: &OvenLegacyCargoBaseLoaf<'_>,
) -> Result<Option<String>, OvenLegacyCargoError> {
    let candidates = store.select_payloads_matching_for_execution(|manifest| {
        manifest.kind == OvenArtifactKind::ProjectPayload
            && manifest.receipt_identity == receipt.identity
            && manifest.build_unit_identity == receipt.build_unit_identity
            && manifest.intent == receipt.intent
    })?;
    let mut identities = Vec::new();
    for candidate in candidates {
        let identity = candidate.manifest.identity;
        let payload = serde_json::from_slice::<OvenProjectExtensionPayload>(&candidate.payload).map_err(|error| {
            OvenLegacyCargoError::Plan(format!(
                "stored project extension candidate {identity} has an invalid payload: {error}"
            ))
        })?;
        match validate_project_extension_payload_against_base(
            &payload,
            &base.loaf_identity,
            &base.build_unit_identity,
            base.artifacts,
        ) {
            Ok(_) => identities.push(identity),
            // A receipt-exact extension that no longer validates against the selected base is a publisher
            // decision worth seeing: the caller will seal a second extension for the same receipt, and every later
            // selection for that receipt then has two candidates.
            Err(error) => tracing::debug!(
                "stored project extension {identity} is not reusable against base {}: {error}",
                base.loaf_identity
            ),
        }
    }
    match identities.as_slice() {
        [] => Ok(None),
        [identity] => Ok(Some(identity.clone())),
        _ => Err(OvenLegacyCargoError::Plan(format!(
            "multiple receipt-exact project extensions are authorized by one selected standard-library Loaf: {}",
            identities.join(", ")
        ))),
    }
}

/// Canonicalize the union of compiled artifacts and independently discovered sealed source files.
///
/// One linkable registry leaf and the complete inspection-source catalog may deliberately name the same staged
/// source file. Identical declarations collapse to one manifest record; conflicting digests remain a hard publisher
/// error instead of allowing one authority surface to overwrite the other.
pub(crate) fn canonicalize_supporting_artifacts(
    artifacts: &mut Vec<OvenRustcSupportingArtifact>,
) -> Result<(), OvenLegacyCargoError> {
    artifacts.sort_by(|left, right| (&left.relative_path, &left.digest).cmp(&(&right.relative_path, &right.digest)));
    if let Some(pair) = artifacts
        .windows(2)
        .find(|pair| pair[0].relative_path == pair[1].relative_path && pair[0].digest != pair[1].digest)
    {
        return Err(OvenLegacyCargoError::InvalidInput {
            field: "direct-rustc supporting artifacts",
            message: format!(
                "staged artifact `{}` has conflicting digests {} and {}",
                pair[0].relative_path, pair[0].digest, pair[1].digest
            ),
        });
    }
    artifacts.dedup_by(|left, right| left.relative_path == right.relative_path);
    Ok(())
}

/// Publish the compiler workspace's direct-rustc test-target plan and its CLI fixture through the one explicit Cargo
/// boundary.
///
/// Cargo's private unit graph is observed only at this publisher boundary, then converted into receipt-bound direct
/// targets and an exact immutable dependency closure. Later compiler-suite invocations acquire a lease and compile
/// and execute that verified plan without invoking Cargo or reading a Cargo target directory.
pub fn prepare_compiler_test_suite(
    request: &OvenLegacyCargoPrepareRequest<'_>,
) -> Result<OvenLegacyCargoCompilerSuiteResult, OvenLegacyCargoError> {
    let suite_started = Instant::now();
    request
        .receipt
        .verify_identity()
        .map_err(|error| OvenLegacyCargoError::ReceiptMismatch {
            message: error.to_string(),
        })?;
    if request.publication_kind != OvenLegacyCargoPublicationKind::LibraryTests {
        return Err(OvenLegacyCargoError::InvalidInput {
            field: "compiler suite publication kind",
            message: "must use the explicit library-tests publisher boundary".to_string(),
        });
    }
    let rustc_identity =
        rustc_identity(&request.rustc).map_err(|error| OvenLegacyCargoError::Plan(error.to_string()))?;
    if rustc_identity != request.receipt.intent.toolchain {
        return Err(OvenLegacyCargoError::ReceiptMismatch {
            message: format!(
                "receipt requires Rust compiler `{}`, but --rustc reports `{rustc_identity}`",
                request.receipt.intent.toolchain
            ),
        });
    }
    if let Some(suite_identity) = select_compiler_test_suite_identity(request.store, &request.receipt)? {
        return Ok(OvenLegacyCargoCompilerSuiteResult {
            suite_identity,
            cargo_version: "not-run-existing-suite".to_string(),
            cargo_manifest_digest: "not-run-existing-suite".to_string(),
            cargo_lock_digest: "not-run-existing-suite".to_string(),
            transient_reservation_bytes: 0,
            timing: OvenLegacyCargoCompilerSuiteTiming {
                preflight_and_sdk_elapsed_ms: suite_started.elapsed().as_millis(),
                ..OvenLegacyCargoCompilerSuiteTiming::default()
            },
        });
    }

    let generated_project = canonical_directory(&request.generated_project, "generated project")?;
    let cargo_manifest = generated_project.join("Cargo.toml");
    let cargo_manifest_bytes = regular_file_bytes(&cargo_manifest)?;
    let _ =
        receipt_authorized_generated_root_bytes(&generated_project, &request.receipt, &request.source_evidence_key)?;
    let cargo_version = tool_version(&request.cargo, "cargo")?;
    let staging_parent = request.store.root().join("legacy-cargo-staging");
    let publisher_lock = acquire_publisher_lock(&staging_parent)?;
    reclaim_stale_publisher_staging(&staging_parent)?;
    let publisher_reservation = request
        .store
        .reserve_legacy_cargo_publisher_capacity(&request.domain)
        .map_err(OvenLegacyCargoError::Store)?;
    let staging = create_publisher_staging(&staging_parent)?;
    let cleanup = PublisherStagingCleanup { path: staging.clone() };
    // The suite publisher copies an already prepared, read-only SDK inventory into the immutable entry. Rebuilding
    // source components here used the ordinary `incan build --lib` helper, which can recurse into generated-Cargo
    // work and turn the hidden Loaf baker into an unbounded second build system. A missing inventory is an
    // explicit Oven preparation miss, never authority to launch that helper or create a hidden Cargo cache.
    let prepared_sdk = match request.sdk_inventory.as_deref() {
        Some(inventory) => Arc::new(SdkInventory::read_from_path(inventory).map_err(|error| {
            OvenLegacyCargoError::Plan(format!(
                "failed to load explicit compiler-suite SDK provider inventory {}: {error}",
                inventory.display()
            ))
        })?),
        None => discover_active_sdk_inventory()
            .map_err(|error| {
                OvenLegacyCargoError::Plan(format!("failed to discover active SDK provider inventory: {error}"))
            })?
            .ok_or_else(|| {
                OvenLegacyCargoError::Plan(
                    "compiler-suite publication requires a prebuilt compatible SDK provider inventory; set INCAN_SDK_INVENTORY or use an installed Oven toolchain"
                        .to_string(),
                )
            })?,
    };
    // The component crates retain path dependencies on compiler runtime crates.  Copying only the provider tree
    // would leave those paths pointing back to the publisher checkout, which is both an SDK leak and a later Cargo
    // failure.  Rebase that small compiler-owned runtime source closure inside the immutable provider tree before
    // recording any materialized files.
    let staged_sdk_root = stage_self_contained_sdk_provider_tree(&prepared_sdk.root, &staging)?;
    let staged_runtime_inputs = compiler_suite_staged_runtime_inputs(&staged_sdk_root)?;
    let sdk_inventory_path = staged_sdk_root.join(SDK_INVENTORY_FILE);
    let sdk_inventory_digest = digest_bytes(&regular_file_bytes(&sdk_inventory_path)?);
    let mut index_materialized_files =
        materialized_files_from_directory(&staged_sdk_root, "providers", "SDK provider inventory")?;
    // Direct-rustc children consume the compiler-owned standard-library family under a shared generation lease.
    // Copying this already sealed release artifact into every receipt-bound compiler-suite store doubled retained
    // bytes and made durable publication dominate cold bakes.  The suite instead records the exact committed
    // generation; the scheduler verifies and holds that generation before it starts its first child.
    let compiler_loaf_root = request.compiler_loaf_root.as_deref().ok_or_else(|| {
        OvenLegacyCargoError::Plan(
            "compiler-suite publication requires the explicit committed compiler Loaf envelope selected by its baker"
                .to_string(),
        )
    })?;
    let toolchain_loaf_generation =
        compiler_suite_toolchain_loaf_generation_reference(compiler_loaf_root, &staged_runtime_inputs)?;
    // A compiler-suite publisher's private selection target is bounded by the store's aggregate physical ceiling,
    // not charged as retained bytes in one compatibility domain. Prepared foundation inputs are checked against the
    // same aggregate ceiling as their related immutable publication. The retained suite closure is then admitted
    // against its one compatibility-domain allowance, so no partition label can hide an oversized suite.
    let publisher_transient_limit = publisher_reservation.transient_limit_bytes;
    let prepared_related_limit = publisher_reservation.transient_limit_bytes;
    // Cargo exposes its resolved test-unit graph only at this explicit publisher boundary. The graph is converted
    // below into Oven target plans; it is never retained as a normal-command dependency or target directory.
    let unit_graph_started = Instant::now();
    let unit_graph_target = staging.join("unit-graph-target");
    let unit_graph_output = run_legacy_cargo_invocation(
        &request.cargo,
        &request.rustc,
        &cargo_manifest,
        &unit_graph_target,
        &staging,
        &request.receipt.intent.target,
        &request.receipt.intent.profile,
        &request.receipt.intent.features,
        publisher_transient_limit,
        "test",
        &OvenLegacyCargoInvocationTarget::WorkspaceTests,
        true,
        false,
        false,
    )?;
    let unit_graph_elapsed_ms = unit_graph_started.elapsed().as_millis();
    let unit_graph = parse_compiler_suite_unit_graph(&unit_graph_output)?;
    // Inspect first: a publisher must fail before materializing even the bounded root compatibility closure if its
    // declared suite contains a target class Oven cannot execute without Cargo. The full workspace graph remains
    // planning evidence only; it is never compiled through Cargo as the ordinary test substrate.
    validate_compiler_suite_unit_graph(&generated_project, &unit_graph)?;
    let metadata = read_legacy_cargo_metadata(&request.cargo, &cargo_manifest, &request.receipt.intent.features)?;
    let foundation_build_started = Instant::now();
    let foundation_dependencies = compiler_suite_foundation_dependencies(&generated_project, &unit_graph, &metadata)?;
    let foundation_manifest = compiler_suite_foundation_manifest(&foundation_dependencies)?;
    let third_party_foundation_manifest = stage_compiler_suite_foundation_manifest(
        &staging,
        &foundation_manifest,
        &generated_project.join("Cargo.lock"),
        &foundation_dependencies,
    )?;
    reclaim_unmaterialized_compiler_suite_target_files(&unit_graph_target, &[])?;

    // The only compilation Cargo is authorized to perform is this sealed third-party foundation. Its copied lock
    // file preserves the compiler workspace's exact registry resolution, while its private root has no path
    // dependency on the compiler workspace. Every compiler library, proc macro, CLI and test root below is then
    // rebuilt from receipt-authorized source by the direct-Rustc scheduler.
    let foundation_target = staging.join("third-party-foundation-target");
    let foundation_output = run_legacy_cargo_invocation(
        &request.cargo,
        &request.rustc,
        &third_party_foundation_manifest,
        &foundation_target,
        &staging,
        &request.receipt.intent.target,
        &request.receipt.intent.profile,
        &[],
        publisher_transient_limit,
        "build",
        &OvenLegacyCargoInvocationTarget::PackageLibrary,
        false,
        false,
        false,
    )?;
    // This phase is the one explicitly permitted Cargo compilation.  Keep the manifest preparation and the child
    // process under one clock: reporting only the setup above would misleadingly classify Cargo's actual foundation
    // work as unaccounted suite time.
    let foundation_build_elapsed_ms = foundation_build_started.elapsed().as_millis();
    let profile_directory = cargo_profile_directory(&request.receipt.intent.profile)?;
    let target_deps = foundation_target
        .join(&request.receipt.intent.target)
        .join(profile_directory)
        .join("deps");
    let host_deps = foundation_target.join(profile_directory).join("deps");
    let dependency_directories = compiler_suite_dependency_directories(target_deps, host_deps);
    // The private foundation root itself may be the only Cargo-reported direct-Rustc artifact on a compatible
    // host/profile layout. Keep every exact compiler-artifact path that Cargo reported, rather than relying on
    // the conventional `deps/` directories to exist. The catalog still rejects paths outside publisher staging.
    let foundation_direct_artifact_files = compiler_suite_output_artifact_paths(&foundation_output)?;
    let foundation_catalog =
        compiler_suite_artifact_catalog(&staging, &dependency_directories, &foundation_direct_artifact_files)?;
    let foundation_artifact_index = compiler_suite_artifact_index(&foundation_output, &request.receipt.intent.target)?;
    let mut root_indices = Vec::new();
    for root_index in &unit_graph.roots {
        let root = unit_graph.units.get(*root_index).ok_or_else(|| {
            OvenLegacyCargoError::Plan(format!(
                "compiler-suite unit graph root index {root_index} is outside its unit list"
            ))
        })?;
        if matches!(root.mode.as_str(), "test" | "doctest")
            && compiler_suite_unit_is_in_workspace(&generated_project, root)?
        {
            root_indices.push(*root_index);
        }
    }
    if root_indices.is_empty() {
        return Err(OvenLegacyCargoError::Plan(
            "compiler-suite graph has no workspace test roots for direct-Rustc materialization".to_string(),
        ));
    }
    let direct_plan_started = Instant::now();
    let mut shard_references = Vec::new();
    let mut foundation_references = Vec::new();
    let mut foundation_requests = Vec::new();
    let mut shard_requests = Vec::new();
    for foundation in compiler_suite_foundation_plans(
        &foundation_catalog.closure,
        &foundation_catalog.materialized_files,
        request.store.limits().max_domain_logical_bytes,
    )? {
        let foundation_payload =
            serde_json::to_vec(&foundation.payload).map_err(|error| OvenLegacyCargoError::Plan(error.to_string()))?;
        let foundation_request = OvenArtifactPublishRequest {
            receipt: request.receipt.clone(),
            domain: request.domain.clone(),
            kind: OvenArtifactKind::CompilerTestSuiteFoundation,
            payload: foundation_payload,
            materialized_files: foundation.materialized_files,
        };
        let manifest = request.store.manifest_for_publication(&foundation_request)?;
        foundation_references.push(OvenCompilerTestSuiteFoundationReference {
            identity: manifest.identity,
            label: foundation.payload.label,
        });
        foundation_requests.push(foundation_request);
    }
    foundation_references.sort();
    foundation_references.dedup();
    for root_index in root_indices {
        let (mut shard, _materialized_files) = compiler_suite_direct_target_shard_from_catalog(
            &generated_project,
            &request.receipt,
            &unit_graph,
            root_index,
            &foundation_artifact_index,
            &foundation_catalog,
        )?;
        shard.foundation_references = foundation_references.clone();
        let payload = serde_json::to_vec(&shard).map_err(|error| OvenLegacyCargoError::Plan(error.to_string()))?;
        let shard_request = OvenArtifactPublishRequest {
            receipt: request.receipt.clone(),
            domain: request.domain.clone(),
            kind: OvenArtifactKind::CompilerTestSuiteShard,
            payload,
            materialized_files: Vec::new(),
        };
        let manifest = request.store.manifest_for_publication(&shard_request)?;
        shard_references.push(OvenCompilerTestSuiteShardReference {
            identity: manifest.identity,
            target: shard.target_key(),
            source_bytes: compiler_suite_verified_target_source_bytes(
                &generated_project,
                &request.receipt,
                &shard.target,
            )?,
        });
        shard_requests.push(shard_request);
    }
    shard_references.sort_by(|left, right| left.target.cmp(&right.target));

    // Cargo supplies only the compiler CLI's resolved unit graph here. It does not compile `src/main.rs` or the
    // workspace library; direct Rustc receives their source plans from the sealed foundation catalog below.
    let cli_unit_graph_target = staging.join("cli-unit-graph-target");
    let cli_unit_graph_output = run_legacy_cargo_invocation(
        &request.cargo,
        &request.rustc,
        &cargo_manifest,
        &cli_unit_graph_target,
        &staging,
        &request.receipt.intent.target,
        &request.receipt.intent.profile,
        &request.receipt.intent.features,
        publisher_transient_limit,
        "build",
        &OvenLegacyCargoInvocationTarget::CompilerCli,
        true,
        false,
        false,
    )?;
    let cli_unit_graph = parse_compiler_suite_unit_graph(&cli_unit_graph_output)?;
    let (cli_target, cli_workspace_libraries) = compiler_suite_cli_target_from_artifact_index(
        &generated_project,
        &request.receipt,
        &cli_unit_graph,
        &foundation_artifact_index,
        &foundation_catalog,
    )?;
    reclaim_unmaterialized_compiler_suite_target_files(&cli_unit_graph_target, &[])?;
    enforce_compiler_suite_prepared_staging_capacity(&staging, prepared_related_limit)?;
    // Schema 12 no longer launches a second Cargo build for `generated_rust_warning_clean`.  The scheduler derives
    // its `incan_stdlib` Rustc plan from a selected shard's already-receipted workspace-library DAG and the sealed
    // foundations below.  This keeps one publisher staging root physically bounded instead of holding a second
    // mostly-duplicate Cargo target beside the third-party foundation.
    let warning_check_artifacts = OvenRustcArtifactManifest {
        schema_version: OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION,
        intent: request.receipt.intent.clone(),
        dependency_search_paths: Vec::new(),
        native_search_paths: Vec::new(),
        externs: Vec::new(),
        entrypoint_externs: BTreeMap::new(),
        registry_leaves: Vec::new(),
        registry_sources: Vec::new(),
        compile_environment: BTreeMap::new(),
        vocab_auxiliary_targets: Vec::new(),
        supporting_artifacts: Vec::new(),
    };
    let cargo_lock = generated_project.join("Cargo.lock");
    let cargo_lock_bytes = regular_file_bytes(&cargo_lock)?;
    let provenance_path = staging.join("provenance/legacy-cargo.json");
    let provenance = OvenLegacyCargoProvenance {
        schema_version: OVEN_LEGACY_CARGO_PROVENANCE_SCHEMA_VERSION,
        boundary: "legacy_cargo".to_string(),
        cargo_version: cargo_version.clone(),
        cargo_manifest_digest: digest_bytes(&cargo_manifest_bytes),
        cargo_lock_digest: digest_bytes(&cargo_lock_bytes),
        publication_kind: request.publication_kind,
        target: request.receipt.intent.target.clone(),
        toolchain: request.receipt.intent.toolchain.clone(),
        profile: request.receipt.intent.profile.clone(),
    };
    write_provenance(&provenance_path, &provenance)?;
    let cli_foundation_references = foundation_references.clone();
    let payload = OvenCompilerTestSuitePayload {
        schema_version: OVEN_COMPILER_TEST_SUITE_SCHEMA_VERSION,
        test_targets: Vec::new(),
        shard_references,
        foundation_references,
        toolchain_data_references: Vec::new(),
        toolchain_loaf_generation: Some(toolchain_loaf_generation),
        binary_targets: Vec::new(),
        test_artifact_closure: None,
        cli_artifact_closure: Some(foundation_catalog.closure.clone()),
        cli_foundation_references,
        cli_target: Some(cli_target),
        cli_workspace_libraries,
        sdk_inventory_relative_path: format!("providers/{SDK_INVENTORY_FILE}"),
        sdk_inventory_digest,
        toolchain_data_relative_root: None,
        warning_check_artifacts,
    };
    let payload_bytes = serde_json::to_vec(&payload).map_err(|error| OvenLegacyCargoError::Plan(error.to_string()))?;
    index_materialized_files.extend([OvenArtifactMaterializedFile {
        source_path: provenance_path,
        relative_path: "provenance/legacy-cargo.json".to_string(),
    }]);
    enforce_compiler_suite_prepared_staging_capacity(&staging, prepared_related_limit)?;
    let transient_reservation_bytes = conservative_directory_reservation(&staging)?;
    let index_request = OvenArtifactPublishRequest {
        receipt: request.receipt.clone(),
        domain: request.domain.clone(),
        kind: OvenArtifactKind::CompilerTestSuite,
        payload: payload_bytes,
        materialized_files: index_materialized_files,
    };
    let mut batch = Vec::with_capacity(
        foundation_requests
            .len()
            .saturating_add(shard_requests.len())
            .saturating_add(1),
    );
    batch.extend(foundation_requests);
    batch.extend(shard_requests);
    // The suite index is its sole selection authority. Keep it last in the publisher request as well as the store's
    // durable commit order so future publication refactors cannot accidentally expose it before its members.
    batch.push(index_request);
    let direct_plan_elapsed_ms = direct_plan_started.elapsed().as_millis();
    let store_publication_started = Instant::now();
    request
        .store
        .ensure_legacy_cargo_batch_physical_capacity(&staging, &batch)?;
    let artifacts = request.store.publish_batch_from_legacy_cargo(&batch)?;
    let artifact = artifacts
        .iter()
        .find(|artifact| artifact.kind == OvenArtifactKind::CompilerTestSuite)
        .ok_or_else(|| {
            OvenLegacyCargoError::Plan("compiler-suite batch publication returned no index manifest".to_string())
        })?;
    let store_publication_elapsed_ms = store_publication_started.elapsed().as_millis();
    drop(cleanup);
    drop(publisher_lock);
    Ok(OvenLegacyCargoCompilerSuiteResult {
        suite_identity: artifact.identity.clone(),
        cargo_version,
        cargo_manifest_digest: digest_bytes(&cargo_manifest_bytes),
        cargo_lock_digest: digest_bytes(&cargo_lock_bytes),
        transient_reservation_bytes,
        timing: OvenLegacyCargoCompilerSuiteTiming {
            preflight_and_sdk_elapsed_ms: unit_graph_started.duration_since(suite_started).as_millis(),
            unit_graph_elapsed_ms,
            foundation_build_elapsed_ms,
            direct_plan_elapsed_ms,
            store_publication_elapsed_ms,
        },
    })
}

/// Map the explicit publisher's supported receipt profiles to Cargo's output directory names.
fn cargo_profile_directory(profile: &str) -> Result<&'static str, OvenLegacyCargoError> {
    match profile {
        "debug" => Ok("debug"),
        "release" => Ok("release"),
        OVEN_COMPILER_TEST_PROFILE => Ok(OVEN_COMPILER_TEST_PROFILE),
        _ => Err(OvenLegacyCargoError::InvalidInput {
            field: "receipt profile",
            message: format!(
                "the internal compatibility publisher supports only debug, release, or {OVEN_COMPILER_TEST_PROFILE}, got `{profile}`"
            ),
        }),
    }
}

/// Select one immutable compiler suite only when it is uniquely authorized by the exact receipt build unit.
fn select_compiler_test_suite_identity(
    store: &OvenStore,
    receipt: &OvenReceipt,
) -> Result<Option<String>, OvenLegacyCargoError> {
    let candidates = store.select_payloads_matching_for_execution(|manifest| {
        manifest.kind == OvenArtifactKind::CompilerTestSuite
            && manifest.build_unit_identity == receipt.build_unit_identity
            && manifest.intent == receipt.intent
    })?;
    let mut identities = Vec::new();
    for candidate in candidates {
        let identity = candidate.manifest.identity;
        let payload = serde_json::from_slice::<OvenCompilerTestSuitePayload>(&candidate.payload).map_err(|error| {
            OvenLegacyCargoError::Plan(format!(
                "stored compiler-suite candidate {identity} has an invalid payload: {error}"
            ))
        })?;
        // A prior schema remains readable only for an already-selected historical execution. It is never a
        // publisher reuse hit: a new publisher version must replace it so newly required receipt evidence cannot
        // be silently absent from a supposedly current immutable suite.
        if payload.schema_version == OVEN_COMPILER_TEST_SUITE_SCHEMA_VERSION
            && payload.test_targets.is_empty()
            && payload.test_artifact_closure.is_none()
            && !payload.shard_references.is_empty()
            && payload
                .shard_references
                .iter()
                .all(|reference| reference.source_bytes > 0)
            && payload.cli_target.is_some()
            && payload.cli_artifact_closure.is_some()
            && !payload.foundation_references.is_empty()
            && payload.toolchain_data_relative_root.is_none()
            && payload.toolchain_data_references.is_empty()
            && payload.toolchain_loaf_generation.is_some()
        {
            identities.push(identity);
        }
    }
    match identities.as_slice() {
        [] => Ok(None),
        [identity] => Ok(Some(identity.clone())),
        _ => Err(OvenLegacyCargoError::Plan(format!(
            "multiple compiler test suites are authorized by one build unit: {}",
            identities.join(", ")
        ))),
    }
}

/// Copy an SDK inventory with its compiler-owned runtime path dependencies made self-contained.
///
/// SDK component crates intentionally use path dependencies while they are prepared.  A compiler-suite entry is an
/// immutable package boundary, so preserving those original relative paths would either read an unrelated checkout
/// or fail after the publisher's worktree disappears.  The publisher therefore copies only the runtime crate source
/// needed by component Cargo manifests, gives that closure its own minimal workspace, and rewrites only the four
/// compiler-owned dependency paths.  Component-to-component paths remain relative to the copied provider tree.
fn stage_self_contained_sdk_provider_tree(
    prepared_root: &Path,
    staging_root: &Path,
) -> Result<PathBuf, OvenLegacyCargoError> {
    let provider_root = staging_root.join("providers");
    copy_regular_directory_tree(prepared_root, &provider_root, "SDK provider inventory")?;
    stage_sdk_runtime_crates(&provider_root)?;
    rebase_sdk_component_runtime_paths(&provider_root)?;
    refresh_staged_sdk_provider_digests(&provider_root)?;
    Ok(provider_root)
}

/// Copy the minimal compiler runtime source closure used by installed SDK component Cargo manifests.
fn stage_sdk_runtime_crates(provider_root: &Path) -> Result<(), OvenLegacyCargoError> {
    const RUNTIME_CRATES: [&str; 4] = ["incan_core", "incan_derive", "incan_stdlib", "incan_web_macros"];

    let source_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let runtime_root = provider_root.join("runtime");
    // An inventory may itself have been recovered from an older compiler-suite entry. Its runtime closure was
    // immutable when materialized, and is not an input authority for this publisher. Discard it in staging before
    // rebuilding from this compiler's checked source so a read-only prior `Cargo.lock` neither blocks publication
    // nor silently chooses an older compiler runtime.
    if runtime_root.exists() {
        fs::remove_dir_all(&runtime_root).map_err(|source| OvenLegacyCargoError::Io {
            path: runtime_root.clone(),
            source,
        })?;
    }
    fs::create_dir_all(runtime_root.join("crates")).map_err(|source| OvenLegacyCargoError::Io {
        path: runtime_root.join("crates"),
        source,
    })?;

    // Preserve the active compiler's workspace package metadata while making only the runtime crates workspace
    // members.  Copying the root Cargo.toml verbatim would name compiler crates that are deliberately not retained
    // in this tiny provider closure.
    let source_workspace_manifest = source_root.join("Cargo.toml");
    let source_workspace_text =
        fs::read_to_string(&source_workspace_manifest).map_err(|source| OvenLegacyCargoError::Io {
            path: source_workspace_manifest.clone(),
            source,
        })?;
    let mut workspace_document =
        toml::from_str::<toml::Value>(&source_workspace_text).map_err(|error| OvenLegacyCargoError::InvalidInput {
            field: "compiler runtime workspace Cargo.toml",
            message: error.to_string(),
        })?;
    let root_table = workspace_document
        .as_table_mut()
        .ok_or_else(|| OvenLegacyCargoError::InvalidInput {
            field: "compiler runtime workspace Cargo.toml",
            message: "must be a TOML table".to_string(),
        })?;
    root_table.retain(|key, _| key == "workspace");
    let workspace = root_table
        .get_mut("workspace")
        .and_then(toml::Value::as_table_mut)
        .ok_or_else(|| OvenLegacyCargoError::InvalidInput {
            field: "compiler runtime workspace Cargo.toml",
            message: "must declare [workspace]".to_string(),
        })?;
    workspace.insert(
        "members".to_string(),
        toml::Value::Array(
            RUNTIME_CRATES
                .into_iter()
                .map(|crate_name| toml::Value::String(format!("crates/{crate_name}")))
                .collect(),
        ),
    );
    workspace.remove("exclude");
    let runtime_workspace_manifest = runtime_root.join("Cargo.toml");
    fs::write(
        &runtime_workspace_manifest,
        toml::to_string_pretty(&workspace_document).map_err(|error| OvenLegacyCargoError::InvalidInput {
            field: "compiler runtime workspace Cargo.toml",
            message: error.to_string(),
        })?,
    )
    .map_err(|source| OvenLegacyCargoError::Io {
        path: runtime_workspace_manifest,
        source,
    })?;
    for file_name in ["Cargo.lock", "README.md"] {
        let source = source_root.join(file_name);
        if source.is_file() {
            let destination = runtime_root.join(file_name);
            fs::copy(&source, &destination).map_err(|source_error| OvenLegacyCargoError::Io {
                path: source.clone(),
                source: source_error,
            })?;
        }
    }

    for crate_name in RUNTIME_CRATES {
        let source_crate = source_root.join("crates").join(crate_name);
        let destination_crate = runtime_root.join("crates").join(crate_name);
        let source_manifest = source_crate.join("Cargo.toml");
        if !source_manifest.is_file() {
            return Err(OvenLegacyCargoError::InvalidInput {
                field: "compiler runtime source closure",
                message: format!("is missing {}", source_manifest.display()),
            });
        }
        fs::create_dir_all(&destination_crate).map_err(|source| OvenLegacyCargoError::Io {
            path: destination_crate.clone(),
            source,
        })?;
        let destination_manifest = destination_crate.join("Cargo.toml");
        fs::copy(&source_manifest, &destination_manifest).map_err(|source| OvenLegacyCargoError::Io {
            path: source_manifest,
            source,
        })?;
        copy_regular_directory_tree(
            &source_crate.join("src"),
            &destination_crate.join("src"),
            "compiler runtime source closure",
        )?;
    }
    Ok(())
}

/// Rebase the component manifests' compiler-owned dependencies to the sealed runtime source closure.
fn rebase_sdk_component_runtime_paths(provider_root: &Path) -> Result<(), OvenLegacyCargoError> {
    const RUNTIME_CRATES: [&str; 4] = ["incan_core", "incan_derive", "incan_stdlib", "incan_web_macros"];

    let components_root = provider_root.join("components");
    let components = fs::read_dir(&components_root)
        .map_err(|source| OvenLegacyCargoError::Io {
            path: components_root.clone(),
            source,
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|source| OvenLegacyCargoError::Io {
            path: components_root.clone(),
            source,
        })?;
    for component in components {
        let metadata = component.metadata().map_err(|source| OvenLegacyCargoError::Io {
            path: component.path(),
            source,
        })?;
        if !metadata.is_dir() {
            continue;
        }
        let manifest_path = component.path().join("Cargo.toml");
        if !manifest_path.is_file() {
            continue;
        }
        let manifest_text = fs::read_to_string(&manifest_path).map_err(|source| OvenLegacyCargoError::Io {
            path: manifest_path.clone(),
            source,
        })?;
        let mut manifest =
            toml::from_str::<toml::Value>(&manifest_text).map_err(|error| OvenLegacyCargoError::InvalidInput {
                field: "SDK component Cargo.toml",
                message: format!("{}: {error}", manifest_path.display()),
            })?;
        let Some(dependencies) = manifest.get_mut("dependencies").and_then(toml::Value::as_table_mut) else {
            continue;
        };
        let mut changed = false;
        for crate_name in RUNTIME_CRATES {
            let Some(dependency) = dependencies.get_mut(crate_name).and_then(toml::Value::as_table_mut) else {
                continue;
            };
            if dependency.contains_key("path") {
                dependency.insert(
                    "path".to_string(),
                    toml::Value::String(format!("../../runtime/crates/{crate_name}")),
                );
                changed = true;
            }
        }
        if changed {
            // Prepared SDK artifacts may have been installed read-only.  This publisher-owned staging copy is the
            // sole place where its path metadata is rebased, before the final immutable store copy is made.
            make_publisher_staging_file_writable(&manifest_path)?;
            fs::write(
                &manifest_path,
                toml::to_string_pretty(&manifest).map_err(|error| OvenLegacyCargoError::InvalidInput {
                    field: "SDK component Cargo.toml",
                    message: format!("{}: {error}", manifest_path.display()),
                })?,
            )
            .map_err(|source| OvenLegacyCargoError::Io {
                path: manifest_path,
                source,
            })?;
        }
    }
    Ok(())
}

/// Re-seal provider and dependency artifact digests after their Cargo path metadata is relocated.
///
/// A provider artifact digest deliberately covers `Cargo.toml`; changing its compiler-owned path dependencies must
/// therefore change both the inventory descriptor and every checked provider edge that references it.  Resolve that
/// DAG from the copied manifests, write children before parents, and only then rewrite the copied inventory.  This
/// keeps the normal provider-plan integrity check meaningful after the suite entry has become self-contained.
fn refresh_staged_sdk_provider_digests(provider_root: &Path) -> Result<(), OvenLegacyCargoError> {
    let inventory_path = provider_root.join(SDK_INVENTORY_FILE);
    let mut inventory = SdkInventory::read_from_path(&inventory_path)
        .map_err(|error| OvenLegacyCargoError::Plan(format!("failed to read staged SDK inventory: {error}")))?;
    let canonical_provider_root = fs::canonicalize(provider_root).map_err(|source| OvenLegacyCargoError::Io {
        path: provider_root.to_path_buf(),
        source,
    })?;
    let mut digests = BTreeMap::new();
    let mut visiting = BTreeSet::new();
    for component in inventory.components.values() {
        for descriptor in &component.providers {
            let Some(crate_root) = descriptor.crate_root.as_ref() else {
                continue;
            };
            let digest = refresh_staged_provider_artifact_digest(
                crate_root,
                &canonical_provider_root,
                &mut digests,
                &mut visiting,
            )?;
            digests.insert(crate_root.clone(), digest);
        }
    }
    for component in inventory.components.values_mut() {
        for descriptor in &mut component.providers {
            let Some(crate_root) = descriptor.crate_root.as_ref() else {
                continue;
            };
            let digest = digests.get(crate_root).ok_or_else(|| {
                OvenLegacyCargoError::Plan(format!(
                    "staged SDK provider {} has no refreshed artifact digest",
                    descriptor.name
                ))
            })?;
            descriptor.digest = digest.clone();
        }
    }
    make_publisher_staging_file_writable(&inventory_path)?;
    inventory
        .write_to_path(&inventory_path)
        .map_err(|error| OvenLegacyCargoError::Plan(format!("failed to write staged SDK inventory: {error}")))?;
    Ok(())
}

/// Update a copied provider manifest's dependency digests in dependency-first order and return its new digest.
fn refresh_staged_provider_artifact_digest(
    crate_root: &Path,
    provider_root: &Path,
    digests: &mut BTreeMap<PathBuf, String>,
    visiting: &mut BTreeSet<PathBuf>,
) -> Result<String, OvenLegacyCargoError> {
    let crate_root = fs::canonicalize(crate_root).map_err(|source| OvenLegacyCargoError::Io {
        path: crate_root.to_path_buf(),
        source,
    })?;
    if !crate_root.starts_with(provider_root) {
        return Err(OvenLegacyCargoError::InvalidInput {
            field: "staged SDK provider dependency",
            message: format!(
                "{} escapes staged provider root {}",
                crate_root.display(),
                provider_root.display()
            ),
        });
    }
    if let Some(digest) = digests.get(&crate_root) {
        return Ok(digest.clone());
    }
    if !visiting.insert(crate_root.clone()) {
        return Err(OvenLegacyCargoError::Plan(format!(
            "staged SDK provider dependency graph cycles at {}",
            crate_root.display()
        )));
    }
    let manifest_path = staged_provider_manifest_path(&crate_root)?;
    let mut manifest = LibraryManifest::read_from_path(&manifest_path)
        .map_err(|error| OvenLegacyCargoError::Plan(format!("failed to read {}: {error}", manifest_path.display())))?;
    let mut changed = false;
    for dependency in &mut manifest.contract_metadata.provider.provider_dependencies {
        let dependency_root = crate_root.join(&dependency.relative_artifact_path);
        let digest = refresh_staged_provider_artifact_digest(&dependency_root, provider_root, digests, visiting)?;
        if dependency.artifact_digest != digest {
            dependency.artifact_digest = digest;
            changed = true;
        }
    }
    if changed {
        make_publisher_staging_file_writable(&manifest_path)?;
        manifest.write_to_path(&manifest_path).map_err(|error| {
            OvenLegacyCargoError::Plan(format!("failed to write {}: {error}", manifest_path.display()))
        })?;
    }
    let digest = digest_provider_artifact(&crate_root)
        .map_err(|error| OvenLegacyCargoError::Plan(format!("failed to digest {}: {error}", crate_root.display())))?;
    visiting.remove(&crate_root);
    digests.insert(crate_root, digest.clone());
    Ok(digest)
}

/// Find the one provider library manifest that owns a copied component root.
fn staged_provider_manifest_path(crate_root: &Path) -> Result<PathBuf, OvenLegacyCargoError> {
    let mut manifests = fs::read_dir(crate_root)
        .map_err(|source| OvenLegacyCargoError::Io {
            path: crate_root.to_path_buf(),
            source,
        })?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|extension| extension.to_str()) == Some("incnlib"))
        .collect::<Vec<_>>();
    manifests.sort();
    let [manifest] = manifests.as_slice() else {
        return Err(OvenLegacyCargoError::InvalidInput {
            field: "staged SDK provider artifact",
            message: format!(
                "{} must contain exactly one .incnlib manifest; found {}",
                crate_root.display(),
                manifests.len()
            ),
        });
    };
    Ok(manifest.clone())
}

/// Mark one copied SDK file writable before adjusting its integrity metadata in publisher-owned staging.
fn make_publisher_staging_file_writable(path: &Path) -> Result<(), OvenLegacyCargoError> {
    let mut permissions = fs::metadata(path)
        .map_err(|source| OvenLegacyCargoError::Io {
            path: path.to_path_buf(),
            source,
        })?
        .permissions();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        // The copied staging file may be read-only, but it must retain its existing group/other bits. Clearing
        // `readonly` would make it world-writable on Unix; grant the publisher owner write access only.
        permissions.set_mode(permissions.mode() | 0o200);
    }
    #[cfg(not(unix))]
    permissions.set_readonly(false);
    fs::set_permissions(path, permissions).map_err(|source| OvenLegacyCargoError::Io {
        path: path.to_path_buf(),
        source,
    })
}

/// Copy a directory while making symlinked or special source inputs fail closed.
pub(crate) fn copy_regular_directory_tree(
    source_root: &Path,
    destination_root: &Path,
    field: &'static str,
) -> Result<(), OvenLegacyCargoError> {
    let metadata = fs::symlink_metadata(source_root).map_err(|source| OvenLegacyCargoError::Io {
        path: source_root.to_path_buf(),
        source,
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(OvenLegacyCargoError::InvalidInput {
            field,
            message: format!("expected a real directory at {}", source_root.display()),
        });
    }
    fs::create_dir_all(destination_root).map_err(|source| OvenLegacyCargoError::Io {
        path: destination_root.to_path_buf(),
        source,
    })?;
    let mut entries = fs::read_dir(source_root)
        .map_err(|source| OvenLegacyCargoError::Io {
            path: source_root.to_path_buf(),
            source,
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|source| OvenLegacyCargoError::Io {
            path: source_root.to_path_buf(),
            source,
        })?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let source = entry.path();
        let destination = destination_root.join(entry.file_name());
        let metadata = fs::symlink_metadata(&source).map_err(|source_error| OvenLegacyCargoError::Io {
            path: source.clone(),
            source: source_error,
        })?;
        if metadata.file_type().is_symlink() {
            return Err(OvenLegacyCargoError::InvalidInput {
                field,
                message: format!("refuses symlinked publisher input {}", source.display()),
            });
        }
        if metadata.is_dir() {
            copy_regular_directory_tree(&source, &destination, field)?;
        } else if metadata.is_file() {
            fs::copy(&source, &destination).map_err(|source_error| OvenLegacyCargoError::Io {
                path: source,
                source: source_error,
            })?;
        } else {
            return Err(OvenLegacyCargoError::InvalidInput {
                field,
                message: format!("refuses non-regular publisher input {}", source.display()),
            });
        }
    }
    Ok(())
}

/// Enumerate one publisher-owned directory as immutable files below a safe artifact prefix.
pub(crate) fn materialized_files_from_directory(
    root: &Path,
    prefix: &str,
    field: &'static str,
) -> Result<Vec<OvenArtifactMaterializedFile>, OvenLegacyCargoError> {
    let metadata = fs::symlink_metadata(root).map_err(|source| OvenLegacyCargoError::Io {
        path: root.to_path_buf(),
        source,
    })?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(OvenLegacyCargoError::InvalidInput {
            field,
            message: format!("expected a real directory at {}", root.display()),
        });
    }
    let mut files = Vec::new();
    collect_materialized_directory_files(root, root, prefix, field, &mut files)?;
    Ok(files)
}

/// Retain the exact dependency graph required to inspect sealed registry sources.
///
/// A project extension can share executable artifacts with its compiler Loaf, but its registry-source catalog still
/// needs the exact checked graph that selected those source trees. The lock is deliberately outside the direct-Rustc
/// plan: it is inspection authority, not a linker input. It nevertheless crosses the immutable-store boundary with
/// the same digest verification as every other materialized file.
fn materialize_sealed_registry_lock(
    staging: &Path,
    has_registry_sources: bool,
    materialized_files: &mut Vec<OvenArtifactMaterializedFile>,
) -> Result<(), OvenLegacyCargoError> {
    if !has_registry_sources {
        return Ok(());
    }
    let source_path = verified_regular_file(
        &staging.join(OVEN_RUSTC_REGISTRY_LOCK_RELATIVE_PATH),
        "sealed registry Cargo.lock",
    )?;
    let relative_path = OVEN_RUSTC_REGISTRY_LOCK_RELATIVE_PATH.to_string();
    if materialized_files
        .iter()
        .any(|file| file.relative_path == relative_path)
    {
        return Err(OvenLegacyCargoError::Plan(
            "sealed registry Cargo.lock duplicates a direct artifact path".to_string(),
        ));
    }
    materialized_files.push(OvenArtifactMaterializedFile {
        source_path,
        relative_path,
    });
    Ok(())
}

/// Recursively retain regular provider files in deterministic path order while rejecting symlink indirection.
fn collect_materialized_directory_files(
    root: &Path,
    directory: &Path,
    prefix: &str,
    field: &'static str,
    files: &mut Vec<OvenArtifactMaterializedFile>,
) -> Result<(), OvenLegacyCargoError> {
    let mut entries = fs::read_dir(directory)
        .map_err(|source| OvenLegacyCargoError::Io {
            path: directory.to_path_buf(),
            source,
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|source| OvenLegacyCargoError::Io {
            path: directory.to_path_buf(),
            source,
        })?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path).map_err(|source| OvenLegacyCargoError::Io {
            path: path.clone(),
            source,
        })?;
        if metadata.file_type().is_symlink() {
            return Err(OvenLegacyCargoError::InvalidInput {
                field,
                message: format!("refuses symlinked publisher input {}", path.display()),
            });
        }
        if metadata.is_dir() {
            collect_materialized_directory_files(root, &path, prefix, field, files)?;
            continue;
        }
        if !metadata.is_file() {
            return Err(OvenLegacyCargoError::InvalidInput {
                field,
                message: format!("refuses non-regular publisher input {}", path.display()),
            });
        }
        let relative = path
            .strip_prefix(root)
            .map_err(|_| OvenLegacyCargoError::InvalidInput {
                field,
                message: format!("cannot make {} relative to {}", path.display(), root.display()),
            })?;
        let components = relative
            .components()
            .map(|component| match component {
                std::path::Component::Normal(component) => {
                    component
                        .to_str()
                        .map(str::to_owned)
                        .ok_or_else(|| OvenLegacyCargoError::InvalidInput {
                            field,
                            message: format!("path is not UTF-8: {}", path.display()),
                        })
                }
                _ => Err(OvenLegacyCargoError::InvalidInput {
                    field,
                    message: format!("path is not a safe relative file: {}", path.display()),
                }),
            })
            .collect::<Result<Vec<_>, _>>()?;
        if components.is_empty() {
            return Err(OvenLegacyCargoError::InvalidInput {
                field,
                message: format!("provider root file has no relative path: {}", path.display()),
            });
        }
        files.push(OvenArtifactMaterializedFile {
            source_path: path,
            relative_path: format!("{prefix}/{}", components.join("/")),
        });
    }
    Ok(())
}

/// Metadata search boundary used while resolving one explicit Rust-inspection surface.
#[derive(Clone, Copy, PartialEq, Eq)]
enum InspectionPackageScope {
    /// Select only packages named by the generated project's root dependency edges.
    DirectRoot,
    /// Select any exact package in the already locked compiler graph.
    ResolvedGraph,
    /// Seal every package in the already locked compiler graph.
    CompleteResolvedGraph,
}

/// Resolve an explicit inspection surface and its transitive package-ID closure from locked Cargo metadata.
fn inspection_package_closure_ids(
    metadata: &CargoMetadata,
    requested: &[OvenLegacyCargoInspectionPackage],
    scope: InspectionPackageScope,
) -> Result<BTreeSet<String>, OvenLegacyCargoError> {
    if requested.is_empty() && scope != InspectionPackageScope::CompleteResolvedGraph {
        return Ok(BTreeSet::new());
    }
    let resolve = metadata.resolve.as_ref().ok_or_else(|| {
        OvenLegacyCargoError::Plan(
            "locked Cargo metadata omitted the resolve graph required by the Rust-inspection surface".to_string(),
        )
    })?;
    let nodes = resolve
        .nodes
        .iter()
        .map(|node| (node.id.as_str(), node))
        .collect::<BTreeMap<_, _>>();
    let packages = metadata
        .packages
        .iter()
        .map(|package| (package.id.as_str(), package))
        .collect::<BTreeMap<_, _>>();
    let candidate_ids = match scope {
        InspectionPackageScope::DirectRoot => {
            let root = resolve.root.as_deref().ok_or_else(|| {
                OvenLegacyCargoError::Plan(
                    "locked Cargo metadata omitted the generated-project root package".to_string(),
                )
            })?;
            let root_node = nodes.get(root).ok_or_else(|| {
                OvenLegacyCargoError::Plan(format!(
                    "locked Cargo metadata root package `{root}` has no resolve node"
                ))
            })?;
            root_node.dependencies.iter().map(String::as_str).collect::<Vec<_>>()
        }
        InspectionPackageScope::ResolvedGraph | InspectionPackageScope::CompleteResolvedGraph => {
            resolve.nodes.iter().map(|node| node.id.as_str()).collect()
        }
    };
    if scope == InspectionPackageScope::CompleteResolvedGraph {
        return Ok(candidate_ids.into_iter().map(str::to_string).collect());
    }
    let mut selected = BTreeSet::new();
    for request in requested {
        let requirement = semver::VersionReq::parse(&request.version_requirement).map_err(|error| {
            OvenLegacyCargoError::Plan(format!(
                "invalid Rust-inspection version requirement `{}` for `{}`: {error}",
                request.version_requirement, request.package
            ))
        })?;
        let mut matches = candidate_ids
            .iter()
            .filter_map(|id| packages.get(id).copied())
            .filter(|package| {
                package.name == request.package
                    && package
                        .source
                        .as_deref()
                        .is_some_and(|source| source.starts_with("registry+"))
                    && semver::Version::parse(&package.version).is_ok_and(|version| requirement.matches(&version))
            })
            .map(|package| package.id.clone())
            .collect::<Vec<_>>();
        matches.sort();
        matches.dedup();
        match matches.as_slice() {
            [package_id] => {
                selected.insert(package_id.clone());
            }
            [] => {
                return Err(OvenLegacyCargoError::Plan(format!(
                    "locked Cargo metadata does not resolve Rust-inspection package `{}` `{}` in the selected scope",
                    request.package, request.version_requirement
                )));
            }
            _ => {
                return Err(OvenLegacyCargoError::Plan(format!(
                    "locked Cargo metadata resolves Rust-inspection package `{}` `{}` ambiguously: {}",
                    request.package,
                    request.version_requirement,
                    matches.join(", ")
                )));
            }
        }
    }
    let mut pending = selected.iter().cloned().collect::<Vec<_>>();
    while let Some(package_id) = pending.pop() {
        let node = nodes.get(package_id.as_str()).ok_or_else(|| {
            OvenLegacyCargoError::Plan(format!(
                "Rust-inspection package `{package_id}` has no locked resolve node"
            ))
        })?;
        for dependency in &node.dependencies {
            if selected.insert(dependency.clone()) {
                pending.push(dependency.clone());
            }
        }
    }
    Ok(selected)
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
}

/// Path to the publisher-authored source authority inherited by a cold baker fixture child.
pub const OVEN_LEGACY_CARGO_INSPECTION_AUTHORITY_ENV: &str = "INCAN_OVEN_LEGACY_CARGO_INSPECTION_AUTHORITY";

/// Registry leaf evidence collected before its immutable source tree is staged.
struct PendingRegistryLeaf {
    package: String,
    version: String,
    crate_name: String,
    features: Vec<String>,
    artifact: OvenRustcArtifactExtern,
    registry: String,
    checksum: String,
    source_root: PathBuf,
    target_artifact: bool,
}

/// One external package selected by the frozen compiler unit graph for a sealed foundation publication.
///
/// Workspace sources intentionally do not appear here: Oven materializes those as caller-owned direct-Rustc
/// libraries or procedural macros. The eventual Cargo invocation therefore need not build the compiler root merely
/// to recover its third-party closure.
#[derive(Debug, Clone, PartialEq, Eq)]
struct CompilerSuiteFoundationDependency {
    alias: String,
    package: String,
    version: String,
    source: Option<String>,
    features: Vec<String>,
    path: Option<PathBuf>,
}

/// Evidence retained in the immutable closure proving the only Cargo use was at the named publisher boundary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct OvenLegacyCargoProvenance {
    schema_version: u32,
    boundary: String,
    cargo_version: String,
    cargo_manifest_digest: String,
    cargo_lock_digest: String,
    publication_kind: OvenLegacyCargoPublicationKind,
    target: String,
    toolchain: String,
    profile: String,
}

/// Hold a unique private publisher directory and delete it on every normal return or error path.
struct PublisherStagingCleanup {
    path: PathBuf,
}

impl Drop for PublisherStagingCleanup {
    fn drop(&mut self) {
        // A just-killed Cargo child can briefly retain a directory handle on macOS. The staging root is task-owned
        // and guarded by the publisher lock, so retry a few times rather than silently retaining a failed
        // compatibility-domain allocation for later inspection or admission.
        for attempt in 0..3 {
            if fs::remove_dir_all(&self.path).is_ok() || !self.path.exists() {
                return;
            }
            if attempt < 2 {
                thread::sleep(Duration::from_millis(50));
            }
        }
    }
}

/// Retain an advisory lock for all stale-staging reclamation and one publisher transaction.
struct PublisherLock {
    file: File,
}

impl Drop for PublisherLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

/// Run the explicit Cargo compiler while bounding transient output by the compatibility-domain allowance.
#[allow(clippy::too_many_arguments)]
fn run_legacy_cargo(
    cargo: &Path,
    rustc: &Path,
    cargo_manifest: &Path,
    target: &Path,
    target_triple: &str,
    profile: &str,
    features: &[String],
    transient_limit: u64,
    publication_kind: OvenLegacyCargoPublicationKind,
    compact_debug_info: bool,
    distinct_extension_identities: bool,
) -> Result<Vec<CargoInvocationOutput>, OvenLegacyCargoError> {
    let first = run_legacy_cargo_invocation(
        cargo,
        rustc,
        cargo_manifest,
        target,
        target,
        target_triple,
        profile,
        features,
        transient_limit,
        match publication_kind {
            OvenLegacyCargoPublicationKind::Executable | OvenLegacyCargoPublicationKind::InteropBootstrap => "build",
            OvenLegacyCargoPublicationKind::LibraryTests => "test",
        },
        &match publication_kind {
            // The interop bootstrap alone emits the companion target. Publishing the library validates the exact
            // Rust closure without prematurely linking the package-owned native artifact.
            OvenLegacyCargoPublicationKind::InteropBootstrap | OvenLegacyCargoPublicationKind::LibraryTests => {
                OvenLegacyCargoInvocationTarget::PackageLibrary
            }
            OvenLegacyCargoPublicationKind::Executable => OvenLegacyCargoInvocationTarget::None,
        },
        false,
        compact_debug_info,
        distinct_extension_identities,
    )?;
    let mut outputs = vec![first];
    if publication_kind == OvenLegacyCargoPublicationKind::LibraryTests {
        // The same explicit publisher also materializes the package library needed to bake the compiler CLI through
        // direct rustc. No normal test command receives this Cargo authority or target path.
        outputs.push(run_legacy_cargo_invocation(
            cargo,
            rustc,
            cargo_manifest,
            target,
            target,
            target_triple,
            profile,
            features,
            transient_limit,
            "build",
            &OvenLegacyCargoInvocationTarget::CompilerCli,
            false,
            compact_debug_info,
            distinct_extension_identities,
        )?);
    }
    Ok(outputs)
}

/// Captured output from one explicitly named Cargo publisher invocation.
struct CargoInvocationOutput {
    stdout: Vec<u8>,
}

/// One publisher-only Cargo target selection used to bake a bounded compiler-suite build unit.
///
/// This never describes a normal Incan command. The resulting executable is discarded; only the dependency
/// artifacts, converted receipt plan, and immutable Oven entry survive the transition.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum OvenLegacyCargoInvocationTarget {
    None,
    PackageLibrary,
    CompilerCli,
    WorkspaceTests,
    /// One package library or proc-macro root. This is intentionally narrower than `--workspace --lib`: a later
    /// Oven shard publisher can own and reclaim this selection without inheriting every workspace library output.
    #[cfg_attr(not(test), allow(dead_code))]
    WorkspacePackageLibrary(String),
    /// One package binary root, retained separately because integration tests can depend on its caller-owned output.
    #[cfg_attr(not(test), allow(dead_code))]
    WorkspacePackageBinary {
        package: String,
        target: String,
    },
    /// One package integration-test root.
    #[cfg_attr(not(test), allow(dead_code))]
    WorkspacePackageIntegrationTest {
        package: String,
        target: String,
    },
    /// One package's Rustdoc roots.
    #[cfg_attr(not(test), allow(dead_code))]
    WorkspacePackageDoctests(String),
}

/// Decode the publisher-only Cargo unit graph before it can be transformed into immutable Oven target plans.
fn parse_compiler_suite_unit_graph(output: &CargoInvocationOutput) -> Result<CargoUnitGraph, OvenLegacyCargoError> {
    let graph = serde_json::from_slice::<CargoUnitGraph>(&output.stdout).map_err(|error| {
        OvenLegacyCargoError::Plan(format!(
            "the internal compatibility publisher did not emit a valid compiler-suite unit graph: {error}"
        ))
    })?;
    if graph.version != 1 {
        return Err(OvenLegacyCargoError::Plan(format!(
            "the internal compatibility publisher emitted unsupported compiler-suite unit graph version {}",
            graph.version
        )));
    }
    if graph.roots.is_empty() {
        return Err(OvenLegacyCargoError::Plan(
            "the internal compatibility publisher emitted a compiler-suite unit graph without root units".to_string(),
        ));
    }
    Ok(graph)
}

/// Validate every workspace test root before compiling the transient publisher target.
///
/// This prevents a costly partial suite publication when Cargo discovers a root mode or target kind that no
/// receipt-bound Oven executor can faithfully run. Rustc libtests (including proc macros) and Rustdoc doctests are
/// both explicit supported runner classes.
fn validate_compiler_suite_unit_graph(
    compiler_root: &Path,
    graph: &CargoUnitGraph,
) -> Result<(), OvenLegacyCargoError> {
    let compiler_root = canonical_directory(compiler_root, "compiler root")?;
    for index in &graph.roots {
        let unit = graph.units.get(*index).ok_or_else(|| {
            OvenLegacyCargoError::Plan(format!(
                "compiler-suite unit graph root index {index} is outside its unit list"
            ))
        })?;
        if !compiler_suite_unit_is_in_workspace(&compiler_root, unit)? {
            continue;
        }
        if !matches!(unit.mode.as_str(), "test" | "doctest") {
            return Err(OvenLegacyCargoError::Plan(format!(
                "full compiler-suite publication does not support Cargo root mode `{}` for {}",
                unit.mode,
                unit.target.src_path.display(),
            )));
        }
        compiler_suite_target_kind(&unit.target.kind)?;
        compiler_suite_target_runner(&unit.mode)?;
    }
    Ok(())
}

/// Convert every supported workspace root into an exact publisher command selection.
///
/// Cargo's unit graph identifies a package by an opaque implementation-specific package ID. Oven derives the
/// package name from the nearest regular `Cargo.toml` below the receipt-authorized compiler root instead of parsing
/// that opaque string. This keeps the hidden publisher command stable while preserving a hard source-root
/// boundary. The resulting selections are deliberately package-qualified: two workspace crates may legitimately
/// expose identically named integration targets.
#[cfg(test)]
fn compiler_suite_target_selections(
    compiler_root: &Path,
    graph: &CargoUnitGraph,
) -> Result<Vec<OvenLegacyCargoInvocationTarget>, OvenLegacyCargoError> {
    Ok(compiler_suite_target_selection_groups(compiler_root, graph)?
        .into_iter()
        .map(|(selection, _)| selection)
        .collect())
}

/// Group every supported graph root by the exact publisher selection that materializes it.
///
/// One package doctest selection may provide more than one root, so the publisher keeps the graph indices alongside
/// the command rather than guessing from target names after Cargo returns. The target directory is still reclaimed
/// immediately after every group has been copied into independent Oven shard staging.
#[cfg(test)]
fn compiler_suite_target_selection_groups(
    compiler_root: &Path,
    graph: &CargoUnitGraph,
) -> Result<Vec<(OvenLegacyCargoInvocationTarget, Vec<usize>)>, OvenLegacyCargoError> {
    let compiler_root = canonical_directory(compiler_root, "compiler root")?;
    let mut selections = BTreeMap::<OvenLegacyCargoInvocationTarget, Vec<usize>>::new();
    for index in &graph.roots {
        let unit = graph.units.get(*index).ok_or_else(|| {
            OvenLegacyCargoError::Plan(format!(
                "compiler-suite unit graph root index {index} is outside its unit list"
            ))
        })?;
        if !matches!(unit.mode.as_str(), "test" | "doctest")
            || !compiler_suite_unit_is_in_workspace(&compiler_root, unit)?
        {
            continue;
        }
        let selection = compiler_suite_target_selection_for_unit(&compiler_root, unit)?;
        selections.entry(selection).or_default().push(*index);
    }
    Ok(selections.into_iter().collect())
}

/// Choose the single compiler-library test root used as the bounded direct-Rustc bootstrap closure.
///
/// This is deliberately source-based rather than package-ID-based: Cargo's package identifiers are opaque, while
/// `src/lib.rs` is already receipt-authorized compiler source. A missing or ambiguous bootstrap is a planning
/// refusal; it must not trigger a scan through every package selection.
#[cfg(test)]
fn compiler_suite_bootstrap_selection(
    compiler_root: &Path,
    graph: &CargoUnitGraph,
    selections: &[(OvenLegacyCargoInvocationTarget, Vec<usize>)],
) -> Result<(OvenLegacyCargoInvocationTarget, Vec<usize>), OvenLegacyCargoError> {
    let compiler_root = canonical_directory(compiler_root, "compiler root")?;
    let root_source =
        fs::canonicalize(compiler_root.join("src/lib.rs")).map_err(|source| OvenLegacyCargoError::Io {
            path: compiler_root.join("src/lib.rs"),
            source,
        })?;
    let mut candidates = Vec::new();
    for (selection, root_indices) in selections {
        let contains_bootstrap = root_indices.iter().any(|index| {
            graph.units.get(*index).is_some_and(|unit| {
                unit.mode == "test"
                    && unit.target.kind.iter().any(|kind| kind == "lib")
                    && fs::canonicalize(&unit.target.src_path).ok().as_ref() == Some(&root_source)
            })
        });
        if contains_bootstrap {
            candidates.push((selection.clone(), root_indices.clone()));
        }
    }
    match candidates.as_slice() {
        [selection] => Ok(selection.clone()),
        [] => Err(OvenLegacyCargoError::Plan(
            "compiler-suite graph has no receipt-authorized src/lib.rs test bootstrap".to_string(),
        )),
        _ => Err(OvenLegacyCargoError::Plan(
            "compiler-suite graph has multiple src/lib.rs test bootstrap selections".to_string(),
        )),
    }
}

/// Return the resolved package features for one exact publisher selection.
///
/// The suite receipt's feature list belongs to the root `incan` package. Reusing it for a selected workspace
/// package makes Cargo reject legitimate roots whose package does not define root-only features such as `cli`.
/// Cargo's unit graph already records the resolved features for every root, so preserve that package-local evidence
/// for the one narrow invocation instead of treating the root receipt features as workspace-global.
#[cfg(test)]
fn compiler_suite_target_selection_features(
    graph: &CargoUnitGraph,
    root_indices: &[usize],
) -> Result<Vec<String>, OvenLegacyCargoError> {
    if root_indices.is_empty() {
        return Err(OvenLegacyCargoError::Plan(
            "compiler-suite publisher selection has no root units".to_string(),
        ));
    }
    let mut features = BTreeSet::new();
    for index in root_indices {
        let unit = graph.units.get(*index).ok_or_else(|| {
            OvenLegacyCargoError::Plan(format!(
                "compiler-suite publisher selection root index {index} is outside its unit list"
            ))
        })?;
        features.extend(unit.features.iter().cloned());
    }
    Ok(features.into_iter().collect())
}

/// Derive the one exact `legacy_cargo` invocation authorized for a workspace test root.
#[cfg(test)]
fn compiler_suite_target_selection_for_unit(
    compiler_root: &Path,
    unit: &CargoUnitGraphUnit,
) -> Result<OvenLegacyCargoInvocationTarget, OvenLegacyCargoError> {
    let package = compiler_suite_package_name_for_source(compiler_root, &unit.target.src_path)?;
    let target_kind = compiler_suite_target_kind(&unit.target.kind)?;
    let runner = compiler_suite_target_runner(&unit.mode)?;
    if runner == "rustdoc-test" {
        return Ok(OvenLegacyCargoInvocationTarget::WorkspacePackageDoctests(package));
    }
    match target_kind.as_str() {
        "lib" | "proc-macro" => Ok(OvenLegacyCargoInvocationTarget::WorkspacePackageLibrary(package)),
        "bin" => Ok(OvenLegacyCargoInvocationTarget::WorkspacePackageBinary {
            package,
            target: unit.target.name.clone(),
        }),
        "test" => Ok(OvenLegacyCargoInvocationTarget::WorkspacePackageIntegrationTest {
            package,
            target: unit.target.name.clone(),
        }),
        _ => unreachable!("validated compiler-suite target kind"),
    }
}

/// Find the package manifest that owns one receipt-authorized workspace source and return its declared package name.
fn compiler_suite_package_name_for_source(compiler_root: &Path, source: &Path) -> Result<String, OvenLegacyCargoError> {
    let compiler_root = canonical_directory(compiler_root, "compiler root")?;
    let source = fs::canonicalize(source).map_err(|source_error| OvenLegacyCargoError::Io {
        path: source.to_path_buf(),
        source: source_error,
    })?;
    if !source.starts_with(&compiler_root) {
        return Err(OvenLegacyCargoError::InvalidInput {
            field: "compiler-suite workspace source",
            message: format!(
                "{} escapes receipt-authorized compiler root {}",
                source.display(),
                compiler_root.display()
            ),
        });
    }
    let mut directory = source.parent().ok_or_else(|| OvenLegacyCargoError::InvalidInput {
        field: "compiler-suite workspace source",
        message: format!("{} has no parent directory", source.display()),
    })?;
    loop {
        let manifest = directory.join("Cargo.toml");
        if manifest.exists() {
            let metadata = fs::symlink_metadata(&manifest).map_err(|source_error| OvenLegacyCargoError::Io {
                path: manifest.clone(),
                source: source_error,
            })?;
            if !metadata.is_file() || metadata.file_type().is_symlink() {
                return Err(OvenLegacyCargoError::InvalidInput {
                    field: "compiler-suite package manifest",
                    message: format!("{} must be a regular non-symlink file", manifest.display()),
                });
            }
            let bytes = regular_file_bytes(&manifest)?;
            let document =
                toml::from_slice::<toml::Value>(&bytes).map_err(|error| OvenLegacyCargoError::InvalidInput {
                    field: "compiler-suite package manifest",
                    message: format!("{} is not valid TOML: {error}", manifest.display()),
                })?;
            if let Some(package) = document
                .get("package")
                .and_then(toml::Value::as_table)
                .and_then(|package| package.get("name"))
                .and_then(toml::Value::as_str)
            {
                return Ok(package.to_string());
            }
        }
        if directory == compiler_root {
            break;
        }
        directory = directory.parent().ok_or_else(|| OvenLegacyCargoError::InvalidInput {
            field: "compiler-suite workspace source",
            message: format!("{} has no parent below compiler root", directory.display()),
        })?;
    }
    Err(OvenLegacyCargoError::InvalidInput {
        field: "compiler-suite package manifest",
        message: format!(
            "no package name owns {} below compiler root {}",
            source.display(),
            compiler_root.display()
        ),
    })
}

/// Immutable publisher-side catalog for every direct-rustc input retained beneath one compiler-suite entry.
struct CompilerSuiteArtifactCatalog {
    closure: OvenCompilerTestSuiteArtifactClosure,
    materialized_files: Vec<OvenArtifactMaterializedFile>,
    by_source_path: BTreeMap<PathBuf, (String, String)>,
}

/// The subset of frozen suite roots that one bounded bootstrap closure can already materialize through direct Rustc.
///
/// A failed root is planning evidence, not permission to launch another Cargo selection. The caller must either
/// supply the missing dependency edge through Oven-owned materialization or refuse the suite before allocating a
/// second publisher closure.
#[cfg(test)]
struct CompilerSuiteTargetPlanCoverage {
    targets: Vec<OvenCompilerTestSuiteTarget>,
    failures: Vec<String>,
}

/// Stable lookup key joining Cargo's unit graph dependency edge to its JSON compiler artifact record.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct CargoUnitArtifactKey {
    package_id: String,
    target_name: String,
    source_path: PathBuf,
    features: Vec<String>,
    test_profile: bool,
    platform: Option<String>,
}

/// Scan the bounded publisher target once and retain every compiler/linker input as an immutable shared closure.
fn compiler_suite_artifact_catalog(
    staging: &Path,
    dependency_directories: &[PathBuf],
    direct_artifact_files: &[PathBuf],
) -> Result<CompilerSuiteArtifactCatalog, OvenLegacyCargoError> {
    let mut dependency_search_paths = Vec::new();
    let mut all_files = BTreeMap::new();
    let mut by_source_path = BTreeMap::new();
    for directory in dependency_directories {
        let directory = canonical_directory(directory, "compiler-suite Cargo dependency output")?;
        let directory_relative = relative_path(staging, &directory)?;
        let mut entries = fs::read_dir(&directory)
            .map_err(|source| OvenLegacyCargoError::Io {
                path: directory.clone(),
                source,
            })?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|source| OvenLegacyCargoError::Io {
                path: directory.clone(),
                source,
            })?;
        entries.sort_by_key(|entry| entry.file_name());
        let mut found = false;
        for entry in entries {
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path).map_err(|source| OvenLegacyCargoError::Io {
                path: path.clone(),
                source,
            })?;
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                return Err(OvenLegacyCargoError::InvalidInput {
                    field: "compiler-suite Cargo dependency output",
                    message: format!("{} must contain regular non-symlink files only", path.display()),
                });
            }
            found |=
                insert_compiler_suite_catalog_artifact(staging, &mut all_files, &mut by_source_path, &path, false)?;
        }
        if found {
            dependency_search_paths.push(directory_relative);
        }
    }
    // A package library's `.rlib` is emitted alongside Cargo's profile directory rather than under `deps/`.
    // Materialize only the explicit compiler-artifact paths recorded by the named publisher; scanning that broad
    // profile directory would accidentally admit Cargo bookkeeping and executables.
    for path in direct_artifact_files {
        // These paths already passed the Cargo-recorded build-output identity check in
        // `compiler_suite_output_artifact_paths`. A current Cargo can place a real compiler artifact below
        // `oven-test/build/<crate>/<identity>/out`; retain only that verified record, never a directory scan of
        // arbitrary build-script output.
        let cargo_reported_build_output = compiler_suite_cargo_build_output(path);
        if insert_compiler_suite_catalog_artifact(staging, &mut all_files, &mut by_source_path, path, true)?
            && cargo_reported_build_output
        {
            // A direct `--extern` points at the target library, but Rustc resolves that library's own closure via
            // `-L dependency`. Cargo 1.99 gives each such library its own `out` directory instead of one shared
            // `deps` directory, so retain only the parent directory of every already-verified JSON record.
            let source_path = verified_regular_file(path, "compiler-suite Cargo artifact")?;
            let parent = source_path.parent().ok_or_else(|| OvenLegacyCargoError::InvalidInput {
                field: "compiler-suite Cargo artifact",
                message: format!("{} has no parent directory", source_path.display()),
            })?;
            let parent_relative = relative_path(staging, parent)?;
            if !dependency_search_paths.contains(&parent_relative) {
                dependency_search_paths.push(parent_relative);
            }
        }
    }
    if all_files.is_empty() {
        return Err(OvenLegacyCargoError::MissingDirectArtifact {
            crate_name: "compiler-suite direct-rustc closure".to_string(),
            path: staging.to_path_buf(),
        });
    }
    let supporting_artifacts = all_files
        .iter()
        .map(|(relative_path, digest)| OvenRustcSupportingArtifact {
            relative_path: relative_path.clone(),
            digest: digest.clone(),
        })
        .collect::<Vec<_>>();
    let materialized_files = by_source_path
        .iter()
        .map(|(source_path, (relative_path, _))| OvenArtifactMaterializedFile {
            source_path: source_path.clone(),
            relative_path: relative_path.clone(),
        })
        .collect::<Vec<_>>();
    Ok(CompilerSuiteArtifactCatalog {
        closure: OvenCompilerTestSuiteArtifactClosure {
            dependency_search_paths,
            native_search_paths: Vec::new(),
            supporting_artifacts,
        },
        materialized_files,
        by_source_path,
    })
}

/// Select only dependency directories Cargo actually emitted for one direct-rustc closure.
///
/// An explicit target build normally produces `target/<triple>/<profile>/deps` as well as host-side procedural
/// macro output. A foundation containing only host-built build or procedural-macro dependencies legitimately has no
/// target directory, however. Treating that absence as publisher I/O failure turns a valid host-only closure into a
/// CI-only failure. The catalog still rejects an entirely empty closure and every root still validates its exact
/// artifact records before Oven publishes it.
fn compiler_suite_dependency_directories(target_deps: PathBuf, host_deps: PathBuf) -> Vec<PathBuf> {
    let mut directories = Vec::new();
    for directory in [target_deps, host_deps] {
        if directory.is_dir() && !directories.contains(&directory) {
            directories.push(directory);
        }
    }
    directories
}

/// Add one exact Cargo-reported compiler artifact to the immutable suite catalog.
fn insert_compiler_suite_catalog_artifact(
    staging: &Path,
    all_files: &mut BTreeMap<String, String>,
    by_source_path: &mut BTreeMap<PathBuf, (String, String)>,
    path: &Path,
    permit_cargo_reported_build_output: bool,
) -> Result<bool, OvenLegacyCargoError> {
    let metadata = fs::symlink_metadata(path).map_err(|source| OvenLegacyCargoError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(OvenLegacyCargoError::InvalidInput {
            field: "compiler-suite Cargo artifact",
            message: format!("{} must be a regular non-symlink file", path.display()),
        });
    }
    let file_name =
        path.file_name()
            .and_then(|value| value.to_str())
            .ok_or_else(|| OvenLegacyCargoError::InvalidInput {
                field: "compiler-suite Cargo artifact",
                message: format!("{} has a non-UTF-8 file name", path.display()),
            })?;
    if !is_direct_rustc_artifact(file_name)
        || (!permit_cargo_reported_build_output && compiler_suite_cargo_build_output(path))
    {
        return Ok(false);
    }
    let source_path = verified_regular_file(path, "compiler-suite Cargo artifact")?;
    // `verified_regular_file` returns a canonical path. Canonicalize the trusted staging root before containment so
    // macOS's interchangeable `/var` and `/private/var` spellings do not turn a staged compiler artifact into a
    // false escape, while preserving the non-symlink boundary check above.
    let canonical_staging = canonical_directory(staging, "compiler-suite publisher staging")?;
    if !source_path.starts_with(&canonical_staging) {
        return Err(OvenLegacyCargoError::InvalidInput {
            field: "compiler-suite Cargo artifact",
            message: format!("{} escapes the publisher staging directory", source_path.display()),
        });
    }
    let relative_path = relative_path(&canonical_staging, &source_path)?;
    let digest = digest_bytes(&regular_file_bytes(&source_path)?);
    let materialized = (relative_path.clone(), digest.clone());
    if let Some(previous) = by_source_path.get(&source_path) {
        if previous == &materialized {
            return Ok(true);
        }
        return Err(OvenLegacyCargoError::InvalidInput {
            field: "compiler-suite Cargo artifact",
            message: format!(
                "declares one compiler artifact more than once: {}",
                source_path.display()
            ),
        });
    }
    if all_files.insert(relative_path.clone(), digest).is_some() {
        return Err(OvenLegacyCargoError::InvalidInput {
            field: "compiler-suite Cargo artifact",
            message: format!("declares duplicate compiler artifact `{relative_path}`"),
        });
    }
    by_source_path.insert(source_path, materialized);
    Ok(true)
}

/// Index compiler-artifact records by the corresponding resolved Cargo unit, preserving exact emitted file paths.
fn compiler_suite_artifact_index(
    output: &CargoInvocationOutput,
    target_triple: &str,
) -> Result<BTreeMap<CargoUnitArtifactKey, Vec<PathBuf>>, OvenLegacyCargoError> {
    let mut index = BTreeMap::<CargoUnitArtifactKey, Vec<PathBuf>>::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let Ok(artifact) = serde_json::from_str::<CargoCompilerArtifact>(line) else {
            continue;
        };
        if artifact.reason != "compiler-artifact" || artifact.target.src_path.as_os_str().is_empty() {
            continue;
        }
        let source_path = fs::canonicalize(&artifact.target.src_path).map_err(|source| OvenLegacyCargoError::Io {
            path: artifact.target.src_path.clone(),
            source,
        })?;
        let files = artifact
            .filenames
            .into_iter()
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(is_direct_rustc_artifact)
                    && compiler_suite_cargo_reported_direct_artifact(&artifact.target.name, path)
            })
            .map(|path| {
                fs::canonicalize(&path).map_err(|source| OvenLegacyCargoError::Io {
                    path: path.clone(),
                    source,
                })
            })
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .collect::<Vec<_>>();
        if !files.is_empty() {
            let mut platforms = files
                .iter()
                .map(|path| compiler_artifact_platform(path, target_triple))
                .collect::<Vec<_>>();
            platforms.sort();
            platforms.dedup();
            let platform = match platforms.as_slice() {
                [platform] => platform.clone(),
                _ => {
                    return Err(OvenLegacyCargoError::InvalidInput {
                        field: "compiler-suite Cargo artifact",
                        message: format!(
                            "{} emitted files for multiple compilation platforms",
                            artifact.target.name
                        ),
                    });
                }
            };
            let mut features = artifact.features;
            features.sort();
            features.dedup();
            let key = CargoUnitArtifactKey {
                package_id: artifact.package_id,
                target_name: artifact.target.name,
                source_path,
                features,
                test_profile: artifact.profile.test,
                platform,
            };
            index.entry(key).or_default().extend(files);
        }
    }
    for files in index.values_mut() {
        files.sort();
        files.dedup();
    }
    Ok(index)
}

/// Read the exact compiler/linker files Cargo reported for one named publisher invocation.
///
/// Cargo can place a package's primary `.rlib` beside the profile directory while dependencies appear below `deps`.
/// Newer Cargo releases can instead report a compiler library below a per-crate `build/<crate>/<identity>/out`
/// directory. The catalog admits the latter only when it is an exact Cargo-recorded, crate-and-identity-matching
/// compiler artifact, then verifies it is a regular path below publisher staging.
fn compiler_suite_output_artifact_paths(output: &CargoInvocationOutput) -> Result<Vec<PathBuf>, OvenLegacyCargoError> {
    publisher_output_artifact_paths(std::slice::from_ref(output), OVEN_COMPILER_TEST_PROFILE)
}

/// Cargo's JSON artifact records do not carry a platform field; the target directory in their canonical output paths
/// is the authoritative publisher-side distinction between host build/proc-macro artifacts and target artifacts.
fn compiler_artifact_platform(path: &Path, target_triple: &str) -> Option<String> {
    path.components()
        .any(|component| component.as_os_str() == target_triple)
        .then(|| target_triple.to_string())
}

/// Convert a resolved Cargo unit to the key used by publisher compiler-artifact output.
fn cargo_unit_artifact_key(unit: &CargoUnitGraphUnit) -> Result<CargoUnitArtifactKey, OvenLegacyCargoError> {
    let source_path = fs::canonicalize(&unit.target.src_path).map_err(|source| OvenLegacyCargoError::Io {
        path: unit.target.src_path.clone(),
        source,
    })?;
    let mut features = unit.features.clone();
    features.sort();
    features.dedup();
    Ok(CargoUnitArtifactKey {
        package_id: unit.pkg_id.clone(),
        target_name: unit.target.name.clone(),
        source_path,
        features,
        test_profile: unit.mode == "test",
        platform: unit.platform.clone(),
    })
}

/// Select the exact compiler artifact Cargo emitted for one unit-graph dependency edge.
fn compiler_suite_dependency_artifact(
    unit: &CargoUnitGraphUnit,
    artifact_index: &BTreeMap<CargoUnitArtifactKey, Vec<PathBuf>>,
    catalog: &CompilerSuiteArtifactCatalog,
    crate_name: &str,
    direct_rustc_target: Option<&str>,
) -> Result<OvenRustcArtifactExtern, OvenLegacyCargoError> {
    let key = cargo_unit_artifact_key(unit)?;
    let wants_dynamic = unit.target.crate_types.iter().any(|kind| kind == "proc-macro");
    // The unit graph tracks Cargo's own host/target compilation placement.  Oven subsequently recompiles every
    // regular dependency consumer with the receipt target, including workspace libraries reached by a host-side
    // proc-macro root.  Selecting that host library merely because the source unit was first seen there can choose
    // a feature-incompatible rlib (for example Serde without its `derive` re-export).  Dynamic proc macros remain
    // host inputs; regular libraries must come from the receipt target and fail closed if that exact family is not
    // among the publisher's emitted artifacts.
    let files = if let Some(target) = direct_rustc_target.filter(|_| !wants_dynamic) {
        let mut target_families = artifact_index
            .iter()
            .filter(|(candidate, files)| {
                candidate.package_id == key.package_id
                    && candidate.target_name == key.target_name
                    && candidate.source_path == key.source_path
                    && candidate.platform.as_deref() == Some(target)
                    && files.iter().any(|path| {
                        path.file_name()
                            .and_then(|name| name.to_str())
                            .is_some_and(|name| name.ends_with(".rlib"))
                    })
            })
            .collect::<Vec<_>>();
        target_families.sort_by_key(|(key, _)| *key);
        match target_families.as_slice() {
            [(_, files)] => *files,
            [] => {
                return compiler_suite_catalog_target_library(unit, catalog, crate_name, target);
            }
            _ => {
                return Err(OvenLegacyCargoError::InvalidInput {
                    field: "compiler-suite unit graph",
                    message: format!(
                        "dependency `{crate_name}` has {} receipt-target artifact families",
                        target_families.len()
                    ),
                });
            }
        }
    } else if let Some(files) = artifact_index.get(&key) {
        files
    } else {
        // Cargo's unit graph records the dependency edge's test/platform/features mode, while its stable JSON
        // artifact stream records the emitted library's own compilation mode. A normal dependency of a test root
        // may therefore be the only artifact with the same package/source but different `profile.test` or feature
        // facts. Relax those secondary keys only in ordered steps and only when one emitted artifact family remains;
        // host/target or test/non-test ambiguity is still a deterministic refusal rather than an implicit choice.
        let same_source = |candidate: &CargoUnitArtifactKey| {
            candidate.package_id == key.package_id
                && candidate.target_name == key.target_name
                && candidate.source_path == key.source_path
        };
        let mut compatible = artifact_index
            .iter()
            .filter(|(candidate, _)| {
                same_source(candidate) && candidate.features == key.features && candidate.platform == key.platform
            })
            .collect::<Vec<_>>();
        if compatible.is_empty() {
            compatible = artifact_index
                .iter()
                .filter(|(candidate, _)| same_source(candidate) && candidate.platform == key.platform)
                .collect();
        }
        if compatible.is_empty() {
            compatible = artifact_index
                .iter()
                .filter(|(candidate, _)| same_source(candidate) && candidate.test_profile == key.test_profile)
                .collect();
        }
        if compatible.is_empty() {
            compatible = artifact_index
                .iter()
                .filter(|(candidate, _)| same_source(candidate))
                .collect();
        }
        match compatible.as_slice() {
            [(_, files)] => *files,
            [] => {
                return Err(OvenLegacyCargoError::MissingDirectArtifact {
                    crate_name: crate_name.to_string(),
                    path: unit.target.src_path.clone(),
                });
            }
            _ => {
                return Err(OvenLegacyCargoError::InvalidInput {
                    field: "compiler-suite unit graph",
                    message: format!(
                        "dependency `{crate_name}` has {} emitted artifact families after test/platform reconciliation",
                        compatible.len()
                    ),
                });
            }
        }
    };
    let mut candidates = files
        .iter()
        .filter(|path| {
            let name = path.file_name().and_then(|name| name.to_str()).unwrap_or_default();
            if wants_dynamic {
                [".dylib", ".so", ".dll"]
                    .iter()
                    .any(|extension| name.ends_with(extension))
            } else {
                name.ends_with(".rlib")
            }
        })
        .filter_map(|path| catalog.by_source_path.get(path).cloned())
        .collect::<Vec<_>>();
    candidates.sort();
    candidates.dedup();
    let (relative_path, digest) = match candidates.as_slice() {
        [artifact] => artifact.clone(),
        [] => {
            if let Some(target) = direct_rustc_target.filter(|_| !wants_dynamic) {
                return compiler_suite_catalog_target_library(unit, catalog, crate_name, target);
            }
            return Err(OvenLegacyCargoError::MissingDirectArtifact {
                crate_name: crate_name.to_string(),
                path: unit.target.src_path.clone(),
            });
        }
        _ => {
            return Err(OvenLegacyCargoError::InvalidInput {
                field: "compiler-suite unit graph",
                message: format!(
                    "dependency `{crate_name}` resolves to multiple immutable compiler artifacts: {}",
                    candidates
                        .iter()
                        .map(|(path, _)| path.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            });
        }
    };
    Ok(OvenRustcArtifactExtern {
        crate_name: crate_name.replace('-', "_"),
        relative_path,
        digest,
    })
}

/// Recover one exact target-library extern from the sealed foundation catalog when Cargo's JSON artifact stream did
/// not retain an artifact family for the matching unit-graph edge.
///
/// The compiler-suite publisher scans the named foundation's target `deps` directory into this immutable catalog
/// before it creates a direct-Rustc plan. A Cargo build-script output record can be filtered because it is not a
/// linkable `--extern`, while the corresponding library record is absent or keyed differently in that JSON stream.
/// In that narrow case the verified target catalog remains sufficient only when it has exactly one target `.rlib`
/// named for the dependency unit. Multiple candidates remain a deterministic refusal; this is neither source
/// discovery nor a consumer-side Cargo fallback.
fn compiler_suite_catalog_target_library(
    unit: &CargoUnitGraphUnit,
    catalog: &CompilerSuiteArtifactCatalog,
    crate_name: &str,
    direct_rustc_target: &str,
) -> Result<OvenRustcArtifactExtern, OvenLegacyCargoError> {
    let crate_prefix = format!("lib{}-", unit.target.name.replace('-', "_"));
    let target_dependencies = Path::new("third-party-foundation-target")
        .join(direct_rustc_target)
        .join("oven-test/deps");
    let mut candidates = catalog
        .by_source_path
        .iter()
        .filter(|(source_path, (relative_path, _))| {
            Path::new(relative_path).starts_with(&target_dependencies)
                && source_path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with(&crate_prefix) && name.ends_with(".rlib"))
        })
        .map(|(_, artifact)| artifact.clone())
        .collect::<Vec<_>>();
    candidates.sort();
    candidates.dedup();
    let (relative_path, digest) = match candidates.as_slice() {
        [artifact] => artifact.clone(),
        [] => {
            return Err(OvenLegacyCargoError::MissingDirectArtifact {
                crate_name: crate_name.to_string(),
                path: unit.target.src_path.clone(),
            });
        }
        _ => {
            return Err(OvenLegacyCargoError::InvalidInput {
                field: "compiler-suite unit graph",
                message: format!(
                    "dependency `{crate_name}` has {} sealed receipt-target library artifacts after Cargo JSON reconciliation",
                    candidates.len()
                ),
            });
        }
    };
    Ok(OvenRustcArtifactExtern {
        crate_name: crate_name.replace('-', "_"),
        relative_path,
        digest,
    })
}

/// Derive a stable direct-Rustc workspace-library key from one source unit.
fn compiler_suite_workspace_library_key(
    compiler_root: &Path,
    unit: &CargoUnitGraphUnit,
) -> Result<OvenCompilerWorkspaceLibraryKey, OvenLegacyCargoError> {
    let compiler_root = canonical_directory(compiler_root, "compiler root")?;
    let source = verified_regular_file(&unit.target.src_path, "compiler-suite workspace library source")?;
    let source_relative_path = source
        .strip_prefix(&compiler_root)
        .map_err(|_| OvenLegacyCargoError::InvalidInput {
            field: "compiler-suite unit graph",
            message: format!("workspace library source {} escapes compiler root", source.display()),
        })?
        .to_string_lossy()
        .replace('\\', "/");
    let target_kind = compiler_suite_target_kind(&unit.target.kind)?;
    if !matches!(target_kind.as_str(), "lib" | "proc-macro") {
        return Err(OvenLegacyCargoError::Plan(format!(
            "workspace dependency `{}` has non-library target kind `{target_kind}`",
            unit.target.src_path.display()
        )));
    }
    let mut features = unit.features.clone();
    features.sort();
    features.dedup();
    Ok(OvenCompilerWorkspaceLibraryKey {
        package_name: compiler_suite_package_name_for_source(&compiler_root, &source)?,
        crate_name: unit.target.name.replace('-', "_"),
        target_kind,
        source_relative_path,
        features,
    })
}

/// Add one immutable `--extern` input, refusing a same-name artifact conflict.
fn compiler_suite_insert_extern(
    externs_by_name: &mut BTreeMap<String, OvenRustcArtifactExtern>,
    extern_artifact: OvenRustcArtifactExtern,
) -> Result<(), OvenLegacyCargoError> {
    match externs_by_name.get(&extern_artifact.crate_name) {
        Some(previous) if previous == &extern_artifact => {
            // Cargo can expose the same resolved dependency through more than one graph edge. `rustc` needs one
            // `--extern`; accepting this is safe only when the immutable artifact identity is exactly identical.
        }
        Some(previous) => {
            return Err(OvenLegacyCargoError::InvalidInput {
                field: "compiler-suite unit graph",
                message: format!(
                    "root target resolves extern `{}` to conflicting immutable artifacts {} and {}",
                    extern_artifact.crate_name, previous.relative_path, extern_artifact.relative_path
                ),
            });
        }
        None => {
            externs_by_name.insert(extern_artifact.crate_name.clone(), extern_artifact);
        }
    }
    Ok(())
}

/// Retain only the transitive external proc-macro edges required while rustc expands a direct dependency.
///
/// A Cargo library can re-export derive macros without exposing those macros as direct edges of its dependent.
/// Direct rustc nevertheless needs the host proc-macro dylib as an explicit `--extern`; traversing the publisher-only
/// unit graph preserves that edge without broadening the plan to every transitive library. Build-script and binary
/// units are compile-time publisher concerns, not direct-rustc inputs, so their subgraphs are deliberately excluded.
fn compiler_suite_collect_transitive_proc_macro_externs(
    unit: &CargoUnitGraphUnit,
    graph: &CargoUnitGraph,
    compiler_root: &Path,
    artifact_index: &BTreeMap<CargoUnitArtifactKey, Vec<PathBuf>>,
    catalog: &CompilerSuiteArtifactCatalog,
    externs_by_name: &mut BTreeMap<String, OvenRustcArtifactExtern>,
    visited_unit_indices: &mut BTreeSet<usize>,
) -> Result<(), OvenLegacyCargoError> {
    for dependency in &unit.dependencies {
        if !visited_unit_indices.insert(dependency.index) {
            continue;
        }
        let dependency_unit = graph.units.get(dependency.index).ok_or_else(|| {
            OvenLegacyCargoError::Plan(format!(
                "compiler-suite unit graph dependency index {} is outside its unit list",
                dependency.index
            ))
        })?;
        if dependency_unit
            .target
            .kind
            .iter()
            .any(|kind| matches!(kind.as_str(), "bin" | "custom-build"))
        {
            continue;
        }
        if dependency_unit.target.kind.iter().any(|kind| kind == "proc-macro") {
            if compiler_suite_unit_is_in_workspace(compiler_root, dependency_unit)? {
                return Err(OvenLegacyCargoError::Plan(format!(
                    "external compiler-suite dependency reaches workspace proc macro {} without a direct workspace edge",
                    dependency_unit.target.src_path.display()
                )));
            }
            let crate_name = dependency.extern_crate_name.as_deref().ok_or_else(|| {
                OvenLegacyCargoError::Plan(format!(
                    "compiler-suite proc-macro dependency {} has no extern crate name",
                    dependency_unit.target.src_path.display()
                ))
            })?;
            let extern_artifact =
                compiler_suite_dependency_artifact(dependency_unit, artifact_index, catalog, crate_name, None)?;
            compiler_suite_insert_extern(externs_by_name, extern_artifact)?;
            // The dylib is itself the consumer's Rustc input. Its dependency graph was needed only when Cargo built
            // that dylib, and recursing into it can incorrectly expose a second feature/profile variant of another
            // macro as a root `--extern`.
            continue;
        }
        compiler_suite_collect_transitive_proc_macro_externs(
            dependency_unit,
            graph,
            compiler_root,
            artifact_index,
            catalog,
            externs_by_name,
            visited_unit_indices,
        )?;
    }
    Ok(())
}

/// Resolve external direct externs and direct-Rustc workspace-library edges for one root unit.
fn compiler_suite_target_externs(
    unit: &CargoUnitGraphUnit,
    graph: &CargoUnitGraph,
    compiler_root: &Path,
    artifact_index: &BTreeMap<CargoUnitArtifactKey, Vec<PathBuf>>,
    catalog: &CompilerSuiteArtifactCatalog,
    direct_rustc_target: &str,
) -> Result<(Vec<OvenRustcArtifactExtern>, Vec<OvenCompilerWorkspaceLibraryKey>), OvenLegacyCargoError> {
    let mut externs_by_name: BTreeMap<String, OvenRustcArtifactExtern> = BTreeMap::new();
    let mut workspace_dependencies = BTreeSet::new();
    let mut visited_transitive_units = BTreeSet::new();
    for dependency in &unit.dependencies {
        let Some(crate_name) = dependency.extern_crate_name.as_deref() else {
            continue;
        };
        let dependency_unit = graph.units.get(dependency.index).ok_or_else(|| {
            OvenLegacyCargoError::Plan(format!(
                "compiler-suite unit graph dependency index {} is outside its unit list",
                dependency.index
            ))
        })?;
        // Cargo records workspace binaries as dependency edges so integration targets can receive their
        // `CARGO_BIN_EXE_*` paths. They are executable inputs, not linkable Rust crates, and therefore must never
        // become `--extern` arguments to Oven's direct-rustc shard.
        if dependency_unit.target.kind.iter().any(|kind| kind == "bin") {
            continue;
        }
        if compiler_suite_unit_is_in_workspace(compiler_root, dependency_unit)? {
            workspace_dependencies.insert(compiler_suite_workspace_library_key(compiler_root, dependency_unit)?);
            continue;
        }
        let extern_artifact = compiler_suite_dependency_artifact(
            dependency_unit,
            artifact_index,
            catalog,
            crate_name,
            Some(direct_rustc_target),
        )?;
        compiler_suite_insert_extern(&mut externs_by_name, extern_artifact)?;
        compiler_suite_collect_transitive_proc_macro_externs(
            dependency_unit,
            graph,
            compiler_root,
            artifact_index,
            catalog,
            &mut externs_by_name,
            &mut visited_transitive_units,
        )?;
    }
    Ok((
        externs_by_name.into_values().collect(),
        workspace_dependencies.into_iter().collect(),
    ))
}

/// Convert one Cargo root unit to a portable direct-rustc Oven target plan.
fn compiler_suite_target_from_unit(
    compiler_root: &Path,
    receipt: &OvenReceipt,
    unit: &CargoUnitGraphUnit,
    graph: &CargoUnitGraph,
    artifact_index: &BTreeMap<CargoUnitArtifactKey, Vec<PathBuf>>,
    catalog: &CompilerSuiteArtifactCatalog,
) -> Result<OvenCompilerTestSuiteTarget, OvenLegacyCargoError> {
    let source = verified_regular_file(&unit.target.src_path, "compiler-suite target source")?;
    let compiler_root = canonical_directory(compiler_root, "compiler root")?;
    let source_relative_path = source
        .strip_prefix(&compiler_root)
        .map_err(|_| OvenLegacyCargoError::InvalidInput {
            field: "compiler-suite unit graph",
            message: format!("target source {} escapes compiler root", source.display()),
        })?
        .to_string_lossy()
        .replace('\\', "/");
    if source_relative_path.is_empty() || source_relative_path.contains("..") {
        return Err(OvenLegacyCargoError::InvalidInput {
            field: "compiler-suite unit graph",
            message: format!("target source path `{source_relative_path}` is not portable"),
        });
    }
    let source_evidence_key = compiler_suite_source_evidence_key(&source_relative_path);
    if !receipt.sources.supplemental_digests.contains_key(&source_evidence_key) {
        return Err(OvenLegacyCargoError::ReceiptMismatch {
            message: format!(
                "compiler-suite receipt does not authorize direct-rustc target source `{source_relative_path}`"
            ),
        });
    }
    let target_kind = compiler_suite_target_kind(&unit.target.kind)?;
    let runner = compiler_suite_target_runner(&unit.mode)?;
    let package_name = compiler_suite_package_name_for_source(&compiler_root, &source)?;
    let crate_name = unit.target.name.replace('-', "_");
    let compile_environment = direct_rustc_compile_environment(&compiler_root, &source)?;
    let binary_dependencies = compiler_suite_binary_dependencies(unit, graph, &compiler_root)?;
    let mut features = unit.features.clone();
    features.sort();
    features.dedup();
    let (externs, workspace_library_dependencies) = compiler_suite_target_externs(
        unit,
        graph,
        &compiler_root,
        artifact_index,
        catalog,
        &receipt.intent.target,
    )
    .map_err(|error| match error {
        OvenLegacyCargoError::MissingDirectArtifact { crate_name, path } => OvenLegacyCargoError::Plan(format!(
            "compiler-suite target `{}` ({}) cannot resolve direct dependency `{crate_name}` from {}",
            source_relative_path,
            unit.mode,
            path.display()
        )),
        error => error,
    })?;
    Ok(OvenCompilerTestSuiteTarget {
        package_name,
        target_name: unit.target.name.clone(),
        target_kind,
        runner,
        source_relative_path,
        source_evidence_key,
        crate_name,
        edition: unit.target.edition.clone(),
        features,
        compile_environment,
        binary_dependencies,
        workspace_library_dependencies,
        externs,
    })
}

/// Read one target source once more at publication and bind its footprint to the same receipt digest Rustc will
/// enforce at execution.
///
/// This intentionally records a source-derived upper-level scheduling signal rather than a historic duration. A
/// clean worktree with the same admitted receipt therefore derives the same replay layout without preserving host
/// performance observations or a mutable test-specific profile.
fn compiler_suite_verified_target_source_bytes(
    compiler_root: &Path,
    receipt: &OvenReceipt,
    target: &OvenCompilerTestSuiteTarget,
) -> Result<u64, OvenLegacyCargoError> {
    let compiler_root = canonical_directory(compiler_root, "compiler root")?;
    let source = verified_regular_file(
        &compiler_root.join(&target.source_relative_path),
        "compiler-suite target source footprint",
    )?;
    if !source.starts_with(&compiler_root) {
        return Err(OvenLegacyCargoError::InvalidInput {
            field: "compiler-suite target source footprint",
            message: format!(
                "target source {} escapes compiler root {}",
                source.display(),
                compiler_root.display()
            ),
        });
    }
    let source_bytes = regular_file_bytes(&source)?;
    let expected_digest = receipt
        .sources
        .supplemental_digests
        .get(&target.source_evidence_key)
        .ok_or_else(|| OvenLegacyCargoError::ReceiptMismatch {
            message: format!(
                "compiler-suite receipt does not authorize target source `{}`",
                target.source_relative_path
            ),
        })?;
    let actual_digest = digest_bytes(&source_bytes);
    if &actual_digest != expected_digest {
        return Err(OvenLegacyCargoError::ReceiptMismatch {
            message: format!(
                "compiler-suite target source `{}` does not match its receipt evidence",
                target.source_relative_path
            ),
        });
    }
    u64::try_from(source_bytes.len()).map_err(|_| OvenLegacyCargoError::InvalidInput {
        field: "compiler-suite target source footprint",
        message: format!("target source {} exceeds the supported byte range", source.display()),
    })
}

/// Convert one Cargo workspace library/proc-macro unit into a caller-owned direct-Rustc materialization step.
fn compiler_suite_workspace_library_from_unit(
    compiler_root: &Path,
    receipt: &OvenReceipt,
    unit: &CargoUnitGraphUnit,
    graph: &CargoUnitGraph,
    artifact_index: &BTreeMap<CargoUnitArtifactKey, Vec<PathBuf>>,
    catalog: &CompilerSuiteArtifactCatalog,
) -> Result<OvenCompilerWorkspaceLibrary, OvenLegacyCargoError> {
    let compiler_root = canonical_directory(compiler_root, "compiler root")?;
    let key = compiler_suite_workspace_library_key(&compiler_root, unit)?;
    let source_evidence_key = compiler_suite_source_evidence_key(&key.source_relative_path);
    if !receipt.sources.supplemental_digests.contains_key(&source_evidence_key) {
        return Err(OvenLegacyCargoError::ReceiptMismatch {
            message: format!(
                "compiler-suite receipt does not authorize direct-Rustc workspace library source `{}`",
                key.source_relative_path
            ),
        });
    }
    let source = verified_regular_file(&unit.target.src_path, "compiler-suite workspace library source")?;
    let compile_environment = direct_rustc_compile_environment(&compiler_root, &source)?;
    let (externs, dependencies) = compiler_suite_target_externs(
        unit,
        graph,
        &compiler_root,
        artifact_index,
        catalog,
        &receipt.intent.target,
    )
    .map_err(|error| match error {
        OvenLegacyCargoError::MissingDirectArtifact { crate_name, path } => OvenLegacyCargoError::Plan(format!(
            "compiler-suite workspace library `{}` cannot resolve direct dependency `{crate_name}` from {}",
            key.source_relative_path,
            path.display()
        )),
        error => error,
    })?;
    Ok(OvenCompilerWorkspaceLibrary {
        key,
        source_evidence_key,
        edition: unit.target.edition.clone(),
        compile_environment,
        externs,
        dependencies,
    })
}

/// Plan every workspace library/proc-macro edge required by direct-Rustc roots and their binary inputs.
///
/// Cargo's unit graph is publisher-only reachability evidence. The resulting compact DAG contains no Cargo output
/// paths: its members are receipt-authorized sources, selected immutable third-party externs, and other DAG keys.
fn compiler_suite_workspace_libraries_for_roots(
    compiler_root: &Path,
    receipt: &OvenReceipt,
    graph: &CargoUnitGraph,
    root_indices: &[usize],
    artifact_index: &BTreeMap<CargoUnitArtifactKey, Vec<PathBuf>>,
    catalog: &CompilerSuiteArtifactCatalog,
) -> Result<Vec<OvenCompilerWorkspaceLibrary>, OvenLegacyCargoError> {
    /// Visit one publisher graph unit and retain its receipt-authorized workspace closure.
    #[allow(clippy::too_many_arguments)]
    fn visit(
        index: usize,
        compiler_root: &Path,
        receipt: &OvenReceipt,
        graph: &CargoUnitGraph,
        artifact_index: &BTreeMap<CargoUnitArtifactKey, Vec<PathBuf>>,
        catalog: &CompilerSuiteArtifactCatalog,
        visited: &mut BTreeSet<usize>,
        libraries: &mut BTreeMap<OvenCompilerWorkspaceLibraryKey, OvenCompilerWorkspaceLibrary>,
    ) -> Result<(), OvenLegacyCargoError> {
        if !visited.insert(index) {
            return Ok(());
        }
        let unit = graph.units.get(index).ok_or_else(|| {
            OvenLegacyCargoError::Plan(format!(
                "compiler-suite workspace-library graph index {index} is outside its unit list"
            ))
        })?;
        for dependency in &unit.dependencies {
            let dependency_unit = graph.units.get(dependency.index).ok_or_else(|| {
                OvenLegacyCargoError::Plan(format!(
                    "compiler-suite workspace-library dependency index {} is outside its unit list",
                    dependency.index
                ))
            })?;
            if !compiler_suite_unit_is_in_workspace(compiler_root, dependency_unit)? {
                continue;
            }
            let target_kind = compiler_suite_target_kind(&dependency_unit.target.kind)?;
            match target_kind.as_str() {
                "bin" => {
                    // Integration tests receive the executable itself through CARGO_BIN_EXE_*, but that executable
                    // must first receive its own direct-Rustc library inputs.
                    visit(
                        dependency.index,
                        compiler_root,
                        receipt,
                        graph,
                        artifact_index,
                        catalog,
                        visited,
                        libraries,
                    )?;
                }
                "lib" | "proc-macro" => {
                    let library = compiler_suite_workspace_library_from_unit(
                        compiler_root,
                        receipt,
                        dependency_unit,
                        graph,
                        artifact_index,
                        catalog,
                    )?;
                    match libraries.insert(library.key.clone(), library.clone()) {
                        Some(previous) if previous == library => {}
                        Some(previous) => {
                            return Err(OvenLegacyCargoError::Plan(format!(
                                "compiler-suite workspace library `{}` has conflicting plans for {} and {}",
                                library.key.crate_name,
                                previous.key.source_relative_path,
                                library.key.source_relative_path
                            )));
                        }
                        None => {}
                    }
                    visit(
                        dependency.index,
                        compiler_root,
                        receipt,
                        graph,
                        artifact_index,
                        catalog,
                        visited,
                        libraries,
                    )?;
                }
                target_kind => {
                    return Err(OvenLegacyCargoError::Plan(format!(
                        "compiler-suite workspace dependency {} has unsupported direct-Rustc target kind `{target_kind}`",
                        dependency_unit.target.src_path.display()
                    )));
                }
            }
        }
        Ok(())
    }

    let compiler_root = canonical_directory(compiler_root, "compiler root")?;
    let mut visited = BTreeSet::new();
    let mut libraries = BTreeMap::new();
    for root_index in root_indices {
        visit(
            *root_index,
            &compiler_root,
            receipt,
            graph,
            artifact_index,
            catalog,
            &mut visited,
            &mut libraries,
        )?;
    }
    Ok(libraries.into_values().collect())
}

/// Extract the workspace binary names Cargo provides to this target through the `CARGO_BIN_EXE_*` contract.
fn compiler_suite_binary_dependencies(
    unit: &CargoUnitGraphUnit,
    graph: &CargoUnitGraph,
    compiler_root: &Path,
) -> Result<Vec<String>, OvenLegacyCargoError> {
    let mut names = BTreeSet::new();
    for dependency in &unit.dependencies {
        let dependency_unit = graph.units.get(dependency.index).ok_or_else(|| {
            OvenLegacyCargoError::Plan(format!(
                "compiler-suite binary dependency index {} is outside its unit list",
                dependency.index
            ))
        })?;
        if dependency_unit.target.kind.iter().any(|kind| kind == "bin")
            && compiler_suite_unit_is_in_workspace(compiler_root, dependency_unit)?
        {
            names.insert(dependency_unit.target.name.clone());
        }
    }
    Ok(names.into_iter().collect())
}

/// Accept only native test roots the direct-rustc executor can faithfully compile today.
fn compiler_suite_target_kind(kinds: &[String]) -> Result<String, OvenLegacyCargoError> {
    for kind in ["test", "lib", "bin", "proc-macro"] {
        if kinds.iter().any(|candidate| candidate == kind) {
            return Ok(kind.to_string());
        }
    }
    Err(OvenLegacyCargoError::InvalidInput {
        field: "compiler-suite unit graph",
        message: format!(
            "does not yet support direct-rustc execution of Cargo target kind(s): {}",
            kinds.join(", ")
        ),
    })
}

/// Translate Cargo's publisher-only root mode into an explicit Oven executor rather than inferring it at runtime.
fn compiler_suite_target_runner(mode: &str) -> Result<String, OvenLegacyCargoError> {
    match mode {
        "test" => Ok("rustc-test".to_string()),
        "doctest" => Ok("rustdoc-test".to_string()),
        "build" => Ok("rustc-run".to_string()),
        _ => Err(OvenLegacyCargoError::InvalidInput {
            field: "compiler-suite unit graph",
            message: format!("does not support Cargo root mode `{mode}`"),
        }),
    }
}

/// Find the package manifest owning a direct-rustc root and encode only portable package metadata.
///
/// Both compiler-suite shards and compiler-owned base Loafs must recreate the small, deterministic subset of
/// Cargo's compile-time package metadata after the executor has removed inherited `CARGO_*` state. The caller passes
/// the root of the publisher-owned project tree so workspace-inherited package versions can be resolved without
/// consulting Cargo at execution time.
pub(crate) fn direct_rustc_compile_environment(
    project_root: &Path,
    source: &Path,
) -> Result<BTreeMap<String, String>, OvenLegacyCargoError> {
    let mut directory = source.parent().ok_or_else(|| OvenLegacyCargoError::InvalidInput {
        field: "compiler-suite target source",
        message: format!("{} has no parent directory", source.display()),
    })?;
    let mut ancestor = 1_usize;
    let package_manifest = loop {
        let manifest = directory.join("Cargo.toml");
        if manifest.is_file() {
            break (directory.to_path_buf(), manifest, ancestor);
        }
        if directory == project_root {
            return Err(OvenLegacyCargoError::InvalidInput {
                field: "direct-rustc source",
                message: format!("{} has no owning Cargo.toml", source.display()),
            });
        }
        directory = directory.parent().ok_or_else(|| OvenLegacyCargoError::InvalidInput {
            field: "compiler-suite target source",
            message: format!("{} escapes compiler root", source.display()),
        })?;
        ancestor = ancestor.saturating_add(1);
    };
    let (_, manifest, ancestor) = package_manifest;
    if ancestor > 16 {
        return Err(OvenLegacyCargoError::InvalidInput {
            field: "compiler-suite target source",
            message: format!(
                "{} is too deeply nested for a portable package-root token",
                source.display()
            ),
        });
    }
    let manifest_bytes = regular_file_bytes(&manifest)?;
    let manifest_text = std::str::from_utf8(&manifest_bytes).map_err(|error| OvenLegacyCargoError::InvalidInput {
        field: "compiler-suite package Cargo.toml",
        message: format!("{} is not UTF-8: {error}", manifest.display()),
    })?;
    let document =
        toml::from_str::<toml::Value>(manifest_text).map_err(|error| OvenLegacyCargoError::InvalidInput {
            field: "compiler-suite package Cargo.toml",
            message: format!("{} is not valid TOML: {error}", manifest.display()),
        })?;
    let package =
        document
            .get("package")
            .and_then(toml::Value::as_table)
            .ok_or_else(|| OvenLegacyCargoError::InvalidInput {
                field: "compiler-suite package Cargo.toml",
                message: format!("{} has no [package] table", manifest.display()),
            })?;
    let name = package
        .get("name")
        .and_then(toml::Value::as_str)
        .ok_or_else(|| OvenLegacyCargoError::InvalidInput {
            field: "compiler-suite package Cargo.toml",
            message: format!("{} has no package name", manifest.display()),
        })?;
    let version = package
        .get("version")
        .and_then(toml::Value::as_str)
        .map(ToOwned::to_owned)
        .or_else(|| workspace_package_value(project_root, "version"))
        .ok_or_else(|| OvenLegacyCargoError::InvalidInput {
            field: "compiler-suite package Cargo.toml",
            message: format!("{} has no resolvable package version", manifest.display()),
        })?;
    Ok(BTreeMap::from([
        (
            "CARGO_MANIFEST_DIR".to_string(),
            format!("@oven-source-ancestor:{ancestor}"),
        ),
        ("CARGO_PKG_NAME".to_string(), name.to_string()),
        ("CARGO_PKG_VERSION".to_string(), version),
    ]))
}

/// Return only the portable generated-project environment that may live in a reusable project Loaf.
///
/// Package name and version are properties of the caller's current generated `Cargo.toml`; retaining them in an
/// immutable extension would let a first project's metadata leak into another project with the same compatible
/// dependency closure. Normal direct-Rustc execution derives those two values again from its own generated source
/// root immediately before invoking Rustc. The source-relative manifest token is intentionally reusable.
pub(crate) fn direct_rustc_reusable_project_plan_environment(
    project_root: &Path,
    source: &Path,
) -> Result<BTreeMap<String, String>, OvenLegacyCargoError> {
    let mut environment = direct_rustc_compile_environment(project_root, source)?;
    environment.remove("CARGO_PKG_NAME");
    environment.remove("CARGO_PKG_VERSION");
    Ok(environment)
}

/// Resolve a workspace-inherited package field from the checked-in root manifest without asking Cargo at execution.
fn workspace_package_value(compiler_root: &Path, field: &str) -> Option<String> {
    let manifest = regular_file_bytes(&compiler_root.join("Cargo.toml")).ok()?;
    let document = toml::from_slice::<toml::Value>(&manifest).ok()?;
    document
        .get("workspace")
        .and_then(toml::Value::as_table)
        .and_then(|workspace| workspace.get("package"))
        .and_then(toml::Value::as_table)
        .and_then(|package| package.get(field))
        .and_then(toml::Value::as_str)
        .map(ToOwned::to_owned)
}

/// Turn bounded test-build-unit outputs and one CLI output into one immutable direct-rustc target plan.
///
/// The transient Cargo target is used only to provide compiled dependency artifacts and its resolved unit graph. The
/// returned targets name caller-owned source roots and immutable artifact inputs; they deliberately retain neither
/// Cargo's test executables nor its target directory as a normal runtime substrate.
#[cfg(test)]
fn compiler_suite_target_plan_coverage(
    compiler_root: &Path,
    receipt: &OvenReceipt,
    graph: &CargoUnitGraph,
    artifact_index: &BTreeMap<CargoUnitArtifactKey, Vec<PathBuf>>,
    catalog: &CompilerSuiteArtifactCatalog,
) -> Result<CompilerSuiteTargetPlanCoverage, OvenLegacyCargoError> {
    let compiler_root = canonical_directory(compiler_root, "compiler root")?;
    let mut targets = Vec::new();
    let mut seen = BTreeSet::new();
    let mut failures = Vec::new();
    for index in &graph.roots {
        let unit = graph.units.get(*index).ok_or_else(|| {
            OvenLegacyCargoError::Plan(format!(
                "compiler-suite test unit graph root index {index} is outside its unit list"
            ))
        })?;
        if !matches!(unit.mode.as_str(), "test" | "doctest")
            || !compiler_suite_unit_is_in_workspace(&compiler_root, unit)?
        {
            continue;
        }
        match compiler_suite_target_from_unit(&compiler_root, receipt, unit, graph, artifact_index, catalog) {
            Ok(target) if seen.insert(target.key()) => targets.push(target),
            Ok(_) => {}
            Err(error) => failures.push(format!("{} ({}) — {error}", unit.target.src_path.display(), unit.mode)),
        }
    }
    targets.sort_by(|left, right| {
        (
            &left.package_name,
            &left.runner,
            &left.target_kind,
            &left.target_name,
            &left.source_relative_path,
        )
            .cmp(&(
                &right.package_name,
                &right.runner,
                &right.target_kind,
                &right.target_name,
                &right.source_relative_path,
            ))
    });
    failures.sort();
    failures.dedup();
    if targets.is_empty() && failures.is_empty() {
        return Err(OvenLegacyCargoError::Plan(
            "compiler-suite unit graph contains no workspace native test targets".to_string(),
        ));
    }
    Ok(CompilerSuiteTargetPlanCoverage { targets, failures })
}

#[cfg(test)]
/// Plan the direct-rustc compiler-suite target closure for test-only publisher verification.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn compiler_suite_direct_target_plan(
    compiler_root: &Path,
    receipt: &OvenReceipt,
    staging: &Path,
    target: &Path,
    test_graph: &CargoUnitGraph,
    test_outputs: &[CargoInvocationOutput],
    cli_graph: &CargoUnitGraph,
    cli_output: &CargoInvocationOutput,
) -> Result<
    (
        Vec<OvenCompilerTestSuiteTarget>,
        Vec<OvenCompilerTestSuiteTarget>,
        OvenCompilerTestSuiteTarget,
        OvenCompilerTestSuiteArtifactClosure,
        Vec<OvenArtifactMaterializedFile>,
    ),
    OvenLegacyCargoError,
> {
    let compiler_root = canonical_directory(compiler_root, "compiler root")?;
    let profile_directory = cargo_profile_directory(&receipt.intent.profile)?;
    let target_deps = target.join(&receipt.intent.target).join(profile_directory).join("deps");
    let host_deps = target.join(profile_directory).join("deps");
    let dependency_directories = compiler_suite_dependency_directories(target_deps, host_deps);
    let cli_direct_artifact_files = compiler_suite_output_artifact_paths(cli_output)?;
    let catalog = compiler_suite_artifact_catalog(staging, &dependency_directories, &cli_direct_artifact_files)?;
    // A root libtest and the normal library used by the direct CLI can legitimately compile the same Cargo unit
    // with distinct hashes. Keep each invocation's compiler-artifact records separate so a test target is always
    // linked against the exact closure Cargo selected for the test unit, and the CLI against its normal build unit.
    let mut test_artifact_index = BTreeMap::<CargoUnitArtifactKey, Vec<PathBuf>>::new();
    for output in test_outputs {
        for (key, mut paths) in compiler_suite_artifact_index(output, &receipt.intent.target)? {
            test_artifact_index.entry(key).or_default().append(&mut paths);
        }
    }
    for paths in test_artifact_index.values_mut() {
        paths.sort();
        paths.dedup();
    }

    let coverage =
        compiler_suite_target_plan_coverage(&compiler_root, receipt, test_graph, &test_artifact_index, &catalog)?;
    if !coverage.failures.is_empty() {
        return Err(OvenLegacyCargoError::Plan(format!(
            "compiler-suite direct target coverage is incomplete: {}",
            coverage.failures.join("; ")
        )));
    }
    let targets = coverage.targets;

    let binary_targets =
        compiler_suite_binary_targets(&compiler_root, receipt, test_graph, &test_artifact_index, &catalog)?;

    let (cli_target, _cli_workspace_libraries) = compiler_suite_cli_target_from_catalog(
        &compiler_root,
        receipt,
        cli_graph,
        std::slice::from_ref(cli_output),
        &catalog,
    )?;
    Ok((
        targets,
        binary_targets,
        cli_target,
        catalog.closure,
        catalog.materialized_files,
    ))
}

/// Convert the isolated normal compiler CLI build into an immutable direct-rustc plan.
///
/// The CLI is index-owned rather than shared with test shards. Its publisher target can therefore be copied into
/// prepared index staging and reclaimed before the complete shard batch is admitted.
#[cfg(test)]
fn compiler_suite_direct_cli_plan(
    compiler_root: &Path,
    receipt: &OvenReceipt,
    staging: &Path,
    target: &Path,
    cli_graph: &CargoUnitGraph,
    cli_outputs: &[CargoInvocationOutput],
) -> Result<
    (
        OvenCompilerTestSuiteTarget,
        Vec<OvenCompilerWorkspaceLibrary>,
        OvenCompilerTestSuiteArtifactClosure,
        Vec<OvenArtifactMaterializedFile>,
    ),
    OvenLegacyCargoError,
> {
    let compiler_root = canonical_directory(compiler_root, "compiler root")?;
    let profile_directory = cargo_profile_directory(&receipt.intent.profile)?;
    let target_deps = target.join(&receipt.intent.target).join(profile_directory).join("deps");
    let host_deps = target.join(profile_directory).join("deps");
    let dependency_directories = compiler_suite_dependency_directories(target_deps, host_deps);
    let mut direct_artifact_files = Vec::new();
    for output in cli_outputs {
        direct_artifact_files.extend(compiler_suite_output_artifact_paths(output)?);
    }
    direct_artifact_files.sort();
    direct_artifact_files.dedup();
    let catalog = compiler_suite_artifact_catalog(staging, &dependency_directories, &direct_artifact_files)?;
    let (cli_target, cli_workspace_libraries) =
        compiler_suite_cli_target_from_catalog(&compiler_root, receipt, cli_graph, cli_outputs, &catalog)?;
    Ok((
        cli_target,
        cli_workspace_libraries,
        catalog.closure,
        catalog.materialized_files,
    ))
}

/// Resolve exactly one normal `incan` CLI target from publisher artifacts already catalogued for an isolated target.
#[cfg(test)]
fn compiler_suite_cli_target_from_catalog(
    compiler_root: &Path,
    receipt: &OvenReceipt,
    cli_graph: &CargoUnitGraph,
    cli_outputs: &[CargoInvocationOutput],
    catalog: &CompilerSuiteArtifactCatalog,
) -> Result<(OvenCompilerTestSuiteTarget, Vec<OvenCompilerWorkspaceLibrary>), OvenLegacyCargoError> {
    let mut artifact_index = BTreeMap::<CargoUnitArtifactKey, Vec<PathBuf>>::new();
    for output in cli_outputs {
        for (key, mut paths) in compiler_suite_artifact_index(output, &receipt.intent.target)? {
            artifact_index.entry(key).or_default().append(&mut paths);
        }
    }
    for paths in artifact_index.values_mut() {
        paths.sort();
        paths.dedup();
    }
    compiler_suite_cli_target_from_artifact_index(compiler_root, receipt, cli_graph, &artifact_index, catalog)
}

/// Resolve the normal `incan` CLI from a publisher-built third-party artifact index.
///
/// The index can come from a sealed foundation manifest rather than a Cargo compilation of the compiler workspace.
/// This is the key separation that lets direct Rustc own the compiler's workspace-library edges.
fn compiler_suite_cli_target_from_artifact_index(
    compiler_root: &Path,
    receipt: &OvenReceipt,
    cli_graph: &CargoUnitGraph,
    artifact_index: &BTreeMap<CargoUnitArtifactKey, Vec<PathBuf>>,
    catalog: &CompilerSuiteArtifactCatalog,
) -> Result<(OvenCompilerTestSuiteTarget, Vec<OvenCompilerWorkspaceLibrary>), OvenLegacyCargoError> {
    let mut cli_candidates = Vec::new();
    for index in &cli_graph.roots {
        let unit = cli_graph.units.get(*index).ok_or_else(|| {
            OvenLegacyCargoError::Plan(format!(
                "compiler-suite CLI unit graph root index {index} is outside its unit list"
            ))
        })?;
        if unit.mode == "build"
            && unit.target.name == "incan"
            && unit.target.kind.iter().any(|kind| kind == "bin")
            && compiler_suite_unit_is_in_workspace(compiler_root, unit)?
        {
            cli_candidates.push((
                *index,
                compiler_suite_target_from_unit(compiler_root, receipt, unit, cli_graph, artifact_index, catalog)?,
            ));
        }
    }
    match cli_candidates.as_slice() {
        [(index, target)] => Ok((
            target.clone(),
            compiler_suite_workspace_libraries_for_roots(
                compiler_root,
                receipt,
                cli_graph,
                &[*index],
                artifact_index,
                catalog,
            )?,
        )),
        [] => Err(OvenLegacyCargoError::Plan(
            "compiler-suite CLI unit graph has no normal `incan` binary target".to_string(),
        )),
        _ => Err(OvenLegacyCargoError::Plan(format!(
            "compiler-suite CLI unit graph has multiple normal `incan` binary targets: {}",
            cli_candidates
                .iter()
                .map(|(_, target)| target.source_relative_path.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ))),
    }
}

/// Convert one isolated publisher target-selection closure into one immutable Oven test shard.
///
/// The caller must give this function a target directory used for only the exact package/target selection that
/// produced `output`. This is intentionally different from slicing the former shared catalog after it was built:
/// every returned closure is rooted in its own temporary selection, so a later publisher can admit/reclaim it as an
/// independently bounded shard instead of copying a full workspace closure for every test root.
#[cfg(test)]
fn compiler_suite_direct_target_shard_plan(
    compiler_root: &Path,
    receipt: &OvenReceipt,
    staging: &Path,
    target: &Path,
    graph: &CargoUnitGraph,
    root_index: usize,
    output: &CargoInvocationOutput,
) -> Result<(OvenCompilerTestSuiteShardPayload, Vec<OvenArtifactMaterializedFile>), OvenLegacyCargoError> {
    let compiler_root = canonical_directory(compiler_root, "compiler root")?;
    let root = graph.units.get(root_index).ok_or_else(|| {
        OvenLegacyCargoError::Plan(format!(
            "compiler-suite shard root index {root_index} is outside its unit list"
        ))
    })?;
    if !matches!(root.mode.as_str(), "test" | "doctest") || !compiler_suite_unit_is_in_workspace(&compiler_root, root)?
    {
        return Err(OvenLegacyCargoError::Plan(format!(
            "compiler-suite shard root index {root_index} is not a supported workspace test root"
        )));
    }
    let profile_directory = cargo_profile_directory(&receipt.intent.profile)?;
    let target_deps = target.join(&receipt.intent.target).join(profile_directory).join("deps");
    let host_deps = target.join(profile_directory).join("deps");
    let dependency_directories = compiler_suite_dependency_directories(target_deps, host_deps);
    let catalog = compiler_suite_artifact_catalog(staging, &dependency_directories, &[])?;
    let artifact_index = compiler_suite_artifact_index(output, &receipt.intent.target)?;
    compiler_suite_direct_target_shard_from_catalog(
        &compiler_root,
        receipt,
        graph,
        root_index,
        &artifact_index,
        &catalog,
    )
}

/// Convert one publisher-only unit-graph root using a sealed third-party foundation artifact catalog.
///
/// No compiler-workspace Cargo target is needed: workspace libraries and proc macros remain source DAG nodes, while
/// this catalog contributes only external crate artifacts selected by the unit graph.
fn compiler_suite_direct_target_shard_from_catalog(
    compiler_root: &Path,
    receipt: &OvenReceipt,
    graph: &CargoUnitGraph,
    root_index: usize,
    artifact_index: &BTreeMap<CargoUnitArtifactKey, Vec<PathBuf>>,
    catalog: &CompilerSuiteArtifactCatalog,
) -> Result<(OvenCompilerTestSuiteShardPayload, Vec<OvenArtifactMaterializedFile>), OvenLegacyCargoError> {
    let compiler_root = canonical_directory(compiler_root, "compiler root")?;
    let root = graph.units.get(root_index).ok_or_else(|| {
        OvenLegacyCargoError::Plan(format!(
            "compiler-suite shard root index {root_index} is outside its unit list"
        ))
    })?;
    if !matches!(root.mode.as_str(), "test" | "doctest") || !compiler_suite_unit_is_in_workspace(&compiler_root, root)?
    {
        return Err(OvenLegacyCargoError::Plan(format!(
            "compiler-suite shard root index {root_index} is not a supported workspace test root"
        )));
    }
    let target = compiler_suite_target_from_unit(&compiler_root, receipt, root, graph, artifact_index, catalog)?;
    let binary_targets = compiler_suite_binary_targets_for_roots(
        &compiler_root,
        receipt,
        graph,
        &[root_index],
        artifact_index,
        catalog,
    )?;
    let workspace_libraries = compiler_suite_workspace_libraries_for_roots(
        &compiler_root,
        receipt,
        graph,
        &[root_index],
        artifact_index,
        catalog,
    )?;
    Ok((
        OvenCompilerTestSuiteShardPayload {
            schema_version: OVEN_COMPILER_TEST_SUITE_SHARD_SCHEMA_VERSION,
            target,
            binary_targets,
            workspace_libraries,
            foundation_references: Vec::new(),
            artifact_closure: catalog.closure.clone(),
        },
        catalog.materialized_files.clone(),
    ))
}

/// Plan the non-CLI workspace binary targets Cargo exposes to integration roots through `CARGO_BIN_EXE_*`.
///
/// Cargo's unit graph represents those binaries as dependencies, but they are not Rust `--extern` artifacts. Oven
/// instead compiles the declared binary source directly into caller-owned output, then restores its exact path only
/// for the target that named it. The main `incan` binary is handled by the dedicated suite CLI plan because test
/// children use that same executable to exercise the compiler command surface.
#[cfg(test)]
fn compiler_suite_binary_targets(
    compiler_root: &Path,
    receipt: &OvenReceipt,
    graph: &CargoUnitGraph,
    artifact_index: &BTreeMap<CargoUnitArtifactKey, Vec<PathBuf>>,
    catalog: &CompilerSuiteArtifactCatalog,
) -> Result<Vec<OvenCompilerTestSuiteTarget>, OvenLegacyCargoError> {
    compiler_suite_binary_targets_for_roots(compiler_root, receipt, graph, &graph.roots, artifact_index, catalog)
}

/// Plan only the caller-selected test roots' `CARGO_BIN_EXE_*` dependencies.
///
/// A shard may not inherit workspace binaries from unrelated roots merely because a prior publisher selection warmed
/// the same transient target. Isolated per-selection materialization passes one root index here.
fn compiler_suite_binary_targets_for_roots(
    compiler_root: &Path,
    receipt: &OvenReceipt,
    graph: &CargoUnitGraph,
    root_indices: &[usize],
    artifact_index: &BTreeMap<CargoUnitArtifactKey, Vec<PathBuf>>,
    catalog: &CompilerSuiteArtifactCatalog,
) -> Result<Vec<OvenCompilerTestSuiteTarget>, OvenLegacyCargoError> {
    let mut binary_indices = BTreeSet::new();
    for index in root_indices {
        let unit = graph.units.get(*index).ok_or_else(|| {
            OvenLegacyCargoError::Plan(format!(
                "compiler-suite test unit graph root index {index} is outside its unit list"
            ))
        })?;
        if !matches!(unit.mode.as_str(), "test" | "doctest")
            || !compiler_suite_unit_is_in_workspace(compiler_root, unit)?
        {
            continue;
        }
        for dependency in &unit.dependencies {
            let binary = graph.units.get(dependency.index).ok_or_else(|| {
                OvenLegacyCargoError::Plan(format!(
                    "compiler-suite binary dependency index {} is outside its unit list",
                    dependency.index
                ))
            })?;
            if binary.target.kind.iter().any(|kind| kind == "bin")
                && compiler_suite_unit_is_in_workspace(compiler_root, binary)?
            {
                if binary.mode != "build" {
                    return Err(OvenLegacyCargoError::Plan(format!(
                        "compiler-suite binary dependency `{}` has unsupported Cargo mode `{}`",
                        binary.target.name, binary.mode
                    )));
                }
                if binary.target.name != "incan" {
                    binary_indices.insert(dependency.index);
                }
            }
        }
    }

    let mut targets = BTreeMap::new();
    for index in binary_indices {
        let unit = graph.units.get(index).ok_or_else(|| {
            OvenLegacyCargoError::Plan(format!(
                "compiler-suite binary dependency index {index} is outside its unit list"
            ))
        })?;
        let target = compiler_suite_target_from_unit(compiler_root, receipt, unit, graph, artifact_index, catalog)?;
        if target.runner != "rustc-run" || target.target_kind != "bin" {
            return Err(OvenLegacyCargoError::Plan(format!(
                "compiler-suite binary dependency `{}` did not produce a direct-rustc run plan",
                target.target_name
            )));
        }
        match targets.insert(target.target_name.clone(), target.clone()) {
            Some(previous) if previous == target => {}
            Some(previous) => {
                return Err(OvenLegacyCargoError::Plan(format!(
                    "compiler-suite binary dependency name `{}` resolves to conflicting sources {} and {}",
                    target.target_name, previous.source_relative_path, target.source_relative_path
                )));
            }
            None => {}
        }
    }
    Ok(targets.into_values().collect())
}

/// Return whether a Cargo unit source belongs to this receipt-authorized workspace rather than a registry dependency.
fn compiler_suite_unit_is_in_workspace(
    compiler_root: &Path,
    unit: &CargoUnitGraphUnit,
) -> Result<bool, OvenLegacyCargoError> {
    let compiler_root = canonical_directory(compiler_root, "compiler root")?;
    let source = fs::canonicalize(&unit.target.src_path).map_err(|source| OvenLegacyCargoError::Io {
        path: unit.target.src_path.clone(),
        source,
    })?;
    Ok(source.starts_with(compiler_root))
}

/// Select the non-workspace package closure that a sealed third-party foundation must provide.
///
/// This consumes the planning-only full-suite unit graph. It deliberately excludes compiler workspace libraries and
/// proc macros: those are the direct-Rustc DAG this Alpha path must bake itself, not a reason to make Cargo compile
/// the `incan` root again. Cargo metadata is used only to turn the opaque graph package identity into an exact
/// package/version dependency in the named publisher manifest.
fn compiler_suite_foundation_dependencies(
    compiler_root: &Path,
    graph: &CargoUnitGraph,
    metadata: &CargoMetadata,
) -> Result<Vec<CompilerSuiteFoundationDependency>, OvenLegacyCargoError> {
    let compiler_root = canonical_directory(compiler_root, "compiler root")?;
    let packages = metadata
        .packages
        .iter()
        .cloned()
        .map(|package| (package.id.clone(), package))
        .collect::<BTreeMap<_, _>>();
    let mut selected = BTreeMap::<String, (CargoMetadataPackage, BTreeSet<String>)>::new();
    let mut reachable = BTreeSet::new();
    let mut pending = graph.roots.clone();
    while let Some(index) = pending.pop() {
        if !reachable.insert(index) {
            continue;
        }
        let unit = graph.units.get(index).ok_or_else(|| {
            OvenLegacyCargoError::Plan(format!(
                "compiler-suite foundation unit graph index {index} is outside its unit list"
            ))
        })?;
        pending.extend(unit.dependencies.iter().map(|dependency| dependency.index));
    }
    for index in reachable {
        let unit = graph.units.get(index).ok_or_else(|| {
            OvenLegacyCargoError::Plan(format!(
                "compiler-suite foundation unit graph index {index} is outside its unit list"
            ))
        })?;
        // `proc_macro` is a Rust compiler/sysroot facility, not a package the explicit publisher may try to fetch
        // or materialize. Custom-build units become available while Cargo builds their selected library package.
        if unit.target.name == "proc_macro"
            || !unit
                .target
                .kind
                .iter()
                .any(|kind| matches!(kind.as_str(), "lib" | "proc-macro"))
        {
            continue;
        }
        let package = packages.get(&unit.pkg_id).cloned().ok_or_else(|| {
            OvenLegacyCargoError::Plan(format!(
                "compiler-suite foundation unit `{}` is absent from publisher Cargo metadata",
                unit.pkg_id
            ))
        })?;
        let patch_path = compiler_suite_foundation_patch_path(&compiler_root, &package)?;
        if package.source.is_none() && patch_path.is_none() {
            let manifest = verified_regular_file(&package.manifest_path, "compiler workspace Cargo.toml")?;
            if manifest.starts_with(&compiler_root) {
                // Compiler sources—including ordinary workspace crates—remain caller-owned direct-Rustc nodes.
                continue;
            }
            return Err(OvenLegacyCargoError::Plan(format!(
                "compiler-suite path dependency `{}` is outside the compiler root or approved crates/third_party patch directory",
                package.name
            )));
        }
        if package.source.is_some() {
            let source = package.source.as_deref().ok_or_else(|| {
                OvenLegacyCargoError::Plan(format!(
                    "compiler-suite external unit `{}` has no immutable registry source or approved third-party patch path",
                    package.name
                ))
            })?;
            if !source.starts_with("registry+") {
                return Err(OvenLegacyCargoError::Plan(format!(
                    "compiler-suite external unit `{}` uses unsupported source `{source}`; a sealed Alpha foundation requires an explicit registry source or approved third-party patch",
                    package.name,
                )));
            }
        }
        let entry = selected
            .entry(package.id.clone())
            .or_insert_with(|| (package, BTreeSet::new()));
        entry.1.extend(unit.features.iter().cloned());
    }
    let mut dependencies = selected
        .into_values()
        .map(|(package, features)| (package, features.into_iter().collect::<Vec<_>>()))
        .collect::<Vec<_>>();
    dependencies.sort_by(|(left, _), (right, _)| {
        (&left.name, &left.version, &left.id).cmp(&(&right.name, &right.version, &right.id))
    });
    dependencies
        .into_iter()
        .enumerate()
        .map(|(index, (package, features))| {
            let path = compiler_suite_foundation_patch_path(&compiler_root, &package)?;
            Ok(CompilerSuiteFoundationDependency {
                alias: format!("oven_foundation_{index:04}"),
                package: package.name,
                version: package.version,
                source: package.source,
                features,
                path,
            })
        })
        .collect()
}

/// Return the one class of compiler-tree source Cargo may retain in the sealed third-party foundation.
///
/// Registry patches under `crates/third_party` preserve the checked-in lock graph when an upstream package enables a
/// yanked or otherwise unacceptable optional dependency. They are dependency provenance, never normal Incan source
/// execution: only the named publisher sees this path and Oven retains its verified output thereafter.
fn compiler_suite_foundation_patch_path(
    compiler_root: &Path,
    package: &CargoMetadataPackage,
) -> Result<Option<PathBuf>, OvenLegacyCargoError> {
    if package.source.is_some() {
        return Ok(None);
    }
    let manifest = verified_regular_file(&package.manifest_path, "compiler third-party patch Cargo.toml")?;
    let package_root = manifest.parent().ok_or_else(|| OvenLegacyCargoError::InvalidInput {
        field: "compiler third-party patch Cargo.toml",
        message: format!("{} has no package directory", manifest.display()),
    })?;
    if package_root.starts_with(compiler_root.join("crates/third_party")) {
        return Ok(Some(package_root.to_path_buf()));
    }
    Ok(None)
}

/// Render the deterministic private manifest for the named third-party foundation publisher.
///
/// This manifest is staging input only. The publisher later retains only independently verified Oven foundation
/// artifacts, never this Cargo project or target directory.
fn compiler_suite_foundation_manifest(
    dependencies: &[CompilerSuiteFoundationDependency],
) -> Result<String, OvenLegacyCargoError> {
    if dependencies.is_empty() {
        return Err(OvenLegacyCargoError::Plan(
            "compiler-suite third-party foundation has no external package dependencies".to_string(),
        ));
    }
    let mut manifest = concat!(
        "[package]\n",
        "name = \"oven-compiler-foundation\"\n",
        "version = \"0.0.0\"\n",
        "edition = \"2024\"\n",
        "publish = false\n\n",
        "[workspace]\n\n",
        "[lib]\n",
        "path = \"src/lib.rs\"\n\n",
        "[dependencies]\n"
    )
    .to_string();
    let patch_dependencies = dependencies
        .iter()
        .filter_map(|dependency| dependency.path.as_ref().map(|path| (&dependency.package, path)))
        .collect::<BTreeMap<_, _>>();
    for dependency in dependencies {
        let package = toml::Value::String(dependency.package.clone()).to_string();
        let features = dependency
            .features
            .iter()
            .filter(|feature| feature.as_str() != "default")
            .map(|feature| toml::Value::String(feature.clone()).to_string())
            .collect::<Vec<_>>();
        let default_features = dependency.features.iter().any(|feature| feature == "default");
        manifest.push_str(&format!("{} = {{ package = {package}", dependency.alias));
        match &dependency.path {
            Some(path) => {
                let path = toml::Value::String(path.display().to_string()).to_string();
                manifest.push_str(&format!(", path = {path}"));
            }
            None => {
                let version = toml::Value::String(format!("={}", dependency.version)).to_string();
                manifest.push_str(&format!(", version = {version}"));
            }
        }
        manifest.push_str(&format!(", default-features = {default_features}"));
        if !features.is_empty() {
            manifest.push_str(&format!(", features = [{}]", features.join(", ")));
        }
        manifest.push_str(" }\n");
    }
    if !patch_dependencies.is_empty() {
        manifest.push_str("\n[patch.crates-io]\n");
        for (package, path) in patch_dependencies {
            let path = toml::Value::String(path.display().to_string()).to_string();
            manifest.push_str(&format!("{package} = {{ path = {path} }}\n"));
        }
    }
    // The compiler-suite Loaf baker uses the named receipt profile for every publisher action. The private
    // foundation manifest must declare the same profile rather than silently compiling its sealed dependency
    // artifacts with Cargo's `dev` defaults. Keep this in lockstep with the root manifest and the direct-rustc
    // test-runner contract: this is a publisher-only compatibility setting, never a normal-command fallback.
    manifest.push_str(OVEN_COMPILER_TEST_CARGO_PROFILE_MANIFEST);
    Ok(manifest)
}

/// Read Cargo's package identity metadata at the named publisher boundary without creating a target directory.
///
/// The unit graph remains the feature and edge authority. Metadata only decodes its opaque package IDs so Oven can
/// create a sealed third-party foundation manifest; no normal command calls this helper.
fn read_legacy_cargo_metadata(
    cargo: &Path,
    cargo_manifest: &Path,
    features: &[String],
) -> Result<CargoMetadata, OvenLegacyCargoError> {
    read_legacy_cargo_metadata_with_lock_policy(cargo, cargo_manifest, features, true)
}

/// Read publisher metadata with the caller's explicit Cargo.lock admission policy.
///
/// Compiler-owned publication always requires a pre-existing lock. The sole `false` caller is the explicit user
/// project bake, which may resolve and create its first Cargo.lock before its source and final compiler plans are
/// sealed. Every later publisher invocation is locked and offline against that newly sealed authority.
fn read_legacy_cargo_metadata_with_lock_policy(
    cargo: &Path,
    cargo_manifest: &Path,
    features: &[String],
    require_existing_lock: bool,
) -> Result<CargoMetadata, OvenLegacyCargoError> {
    read_legacy_cargo_metadata_for_platform(cargo, cargo_manifest, features, require_existing_lock, None)
}

/// Read publisher metadata with the caller's explicit Cargo.lock admission policy, optionally pruned to one target.
///
/// Without `filter_platform`, Cargo's resolve graph stays platform-agnostic: it retains every target's locked
/// closure, including a dependency that Cargo itself would never build for the exact target being sealed (for
/// example a Linux-only transitive dependency of a project-declared crate while baking for macOS). Passing the
/// receipt's exact target reproduces Cargo's own `--filter-platform` resolver decision, so a caller that needs to
/// know which locked packages a target's build actually requires gets Cargo's answer instead of a broader,
/// platform-agnostic guess.
fn read_legacy_cargo_metadata_for_platform(
    cargo: &Path,
    cargo_manifest: &Path,
    features: &[String],
    require_existing_lock: bool,
    filter_platform: Option<&str>,
) -> Result<CargoMetadata, OvenLegacyCargoError> {
    let cargo = canonical_tool_file(cargo, "cargo")?;
    let cargo_manifest = verified_regular_file(cargo_manifest, "Cargo manifest")?;
    let package_root = cargo_manifest
        .parent()
        .ok_or_else(|| OvenLegacyCargoError::InvalidInput {
            field: "Cargo manifest",
            message: format!("{} has no package directory", cargo_manifest.display()),
        })?;
    let cargo_lock_exists = package_root.join("Cargo.lock").is_file();
    let mut command = Command::new(&cargo);
    command
        .current_dir(package_root)
        .arg("metadata")
        .arg("--manifest-path")
        .arg(&cargo_manifest)
        .args(["--format-version", "1"]);
    if require_existing_lock || cargo_lock_exists {
        command.arg("--offline");
    }
    if require_existing_lock {
        command.arg("--locked");
    }
    if let Some(target) = filter_platform {
        command.args(["--filter-platform", target]);
    }
    // The package records must be resolved through the same feature selection as the unit graph. Otherwise an
    // optional dependency may occur in the graph but be absent from this identity lookup, which leaves the named
    // foundation publisher unable to reproduce its declared third-party closure.
    if !features.is_empty() {
        command.arg("--features").arg(features.join(","));
    }
    clear_inherited_cargo_environment(&mut command);
    let output = command.output().map_err(|source| OvenLegacyCargoError::Io {
        path: cargo.clone(),
        source,
    })?;
    if !output.status.success() {
        return Err(OvenLegacyCargoError::CargoFailed {
            output: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        });
    }
    serde_json::from_slice(&output.stdout).map_err(|error| {
        OvenLegacyCargoError::Plan(format!(
            "the internal compatibility publisher emitted invalid compiler foundation metadata: {error}"
        ))
    })
}

/// Bind each declared direct dependency alias to the exact resolved Cargo package instance.
///
/// A direct-rustc invocation receives `--extern <alias>=<artifact>`, so choosing by library filename or package name
/// is unsound when a valid lock contains (for example) `substrait` 0.62 transitively and `substrait` 0.63 directly.
/// Cargo's root resolve edges preserve the alias and package ID relationship; retain it only long enough for the
/// named baker to select and seal the matching artifact.
fn resolve_direct_dependency_packages(
    metadata: &CargoMetadata,
    dependencies: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, ResolvedDirectDependency>, OvenLegacyCargoError> {
    if dependencies.is_empty() {
        return Ok(BTreeMap::new());
    }
    let resolve = metadata.resolve.as_ref().ok_or_else(|| {
        OvenLegacyCargoError::Plan(
            "locked Cargo metadata omitted the resolve graph required for direct-rustc dependency selection"
                .to_string(),
        )
    })?;
    let root_id = resolve.root.as_deref().ok_or_else(|| {
        OvenLegacyCargoError::Plan("locked Cargo metadata omitted the generated-project root package".to_string())
    })?;
    let packages = metadata
        .packages
        .iter()
        .map(|package| (package.id.as_str(), package))
        .collect::<BTreeMap<_, _>>();
    let root_package = packages.get(root_id).ok_or_else(|| {
        OvenLegacyCargoError::Plan(format!(
            "locked Cargo metadata root package `{root_id}` is absent from its package records"
        ))
    })?;
    let root = resolve.nodes.iter().find(|node| node.id == root_id).ok_or_else(|| {
        OvenLegacyCargoError::Plan(format!(
            "locked Cargo metadata root package `{root_id}` has no resolve node"
        ))
    })?;
    let normalize = |name: &str| name.replace('-', "_");
    let mut resolved = BTreeMap::new();
    for (alias, package) in dependencies {
        // The library-test publisher adds the root package's own library as an explicit input. Cargo does not model
        // that self-library as a root dependency edge, so it is the one principled exception to edge lookup.
        if normalize(alias) == normalize(&root_package.name) && package == &root_package.name {
            resolved.insert(
                alias.clone(),
                ResolvedDirectDependency {
                    package: package.clone(),
                    package_id: root_id.to_string(),
                },
            );
            continue;
        }
        let mut candidates = root
            .deps
            .iter()
            .filter(|edge| normalize(&edge.name) == normalize(alias))
            .filter(|edge| {
                packages
                    .get(edge.pkg.as_str())
                    .is_some_and(|candidate| candidate.name == *package)
            })
            .map(|edge| edge.pkg.clone())
            .collect::<Vec<_>>();
        candidates.sort();
        candidates.dedup();
        match candidates.as_slice() {
            [package_id] => {
                resolved.insert(
                    alias.clone(),
                    ResolvedDirectDependency {
                        package: package.clone(),
                        package_id: package_id.clone(),
                    },
                );
            }
            [] => {
                return Err(OvenLegacyCargoError::Plan(format!(
                    "locked Cargo metadata has no root dependency edge for direct Rustc extern `{alias}` (package `{package}`)"
                )));
            }
            _ => {
                return Err(OvenLegacyCargoError::Plan(format!(
                    "locked Cargo metadata resolves direct Rustc extern `{alias}` (package `{package}`) ambiguously: {}",
                    candidates.join(", ")
                )));
            }
        }
    }
    Ok(resolved)
}

/// Bind every direct registry alias in the generated root manifest to its exact sealed source identity.
///
/// This deliberately consumes the full declared root dependency map, not the smaller set of crates reachable from
/// one generated Rust source. Source inspection happens before that generated source exists, and two renamed aliases
/// can legitimately select distinct compatible versions of the same package. The explicit baker records Cargo's
/// exact root-edge decision so normal commands never have to repeat or approximate that resolution.
fn project_registry_source_dependencies(
    metadata: &CargoMetadata,
    dependencies: &BTreeMap<String, String>,
    registry_sources: &[OvenRustcRegistrySourcePackage],
) -> Result<Vec<OvenProjectRegistrySourceDependency>, OvenLegacyCargoError> {
    if dependencies.is_empty() {
        return Ok(Vec::new());
    }
    let resolve = metadata.resolve.as_ref().ok_or_else(|| {
        OvenLegacyCargoError::Plan(
            "locked Cargo metadata omitted the resolve graph required for project source authority".to_string(),
        )
    })?;
    let root_id = resolve.root.as_deref().ok_or_else(|| {
        OvenLegacyCargoError::Plan("locked Cargo metadata omitted the generated-project root package".to_string())
    })?;
    let packages = metadata
        .packages
        .iter()
        .map(|package| (package.id.as_str(), package))
        .collect::<BTreeMap<_, _>>();
    let root = resolve.nodes.iter().find(|node| node.id == root_id).ok_or_else(|| {
        OvenLegacyCargoError::Plan(format!(
            "locked Cargo metadata root package `{root_id}` has no resolve node"
        ))
    })?;
    let normalize = |name: &str| name.replace('-', "_");
    let mut selected = Vec::new();
    for (alias, declared_package) in dependencies {
        let mut candidates = root
            .deps
            .iter()
            .filter(|edge| normalize(&edge.name) == normalize(alias))
            .filter_map(|edge| packages.get(edge.pkg.as_str()).copied())
            .filter(|package| package.name == *declared_package)
            .collect::<Vec<_>>();
        candidates.sort_by(|left, right| left.id.cmp(&right.id));
        candidates.dedup_by(|left, right| left.id == right.id);
        let package = match candidates.as_slice() {
            [package] => *package,
            [] => {
                return Err(OvenLegacyCargoError::Plan(format!(
                    "locked Cargo metadata has no root dependency edge for `{alias}` (package `{declared_package}`)"
                )));
            }
            _ => {
                return Err(OvenLegacyCargoError::Plan(format!(
                    "locked Cargo metadata resolves root dependency `{alias}` (package `{declared_package}`) ambiguously"
                )));
            }
        };
        let Some(registry) = package.source.as_deref() else {
            continue;
        };
        if !registry.starts_with("registry+") {
            return Err(OvenLegacyCargoError::Plan(format!(
                "root dependency `{alias}` selected unsupported source `{registry}`"
            )));
        }
        let matches = registry_sources
            .iter()
            .filter(|source| {
                source.package == package.name
                    && source.version == package.version
                    && source.source.registry == registry
            })
            .collect::<Vec<_>>();
        let [source] = matches.as_slice() else {
            return Err(OvenLegacyCargoError::Plan(format!(
                "root registry dependency `{alias}` (package `{}` {} from `{registry}`) has {} exact sealed source records",
                package.name,
                package.version,
                matches.len()
            )));
        };
        selected.push(OvenProjectRegistrySourceDependency {
            alias: alias.clone(),
            package: package.name.clone(),
            version: package.version.clone(),
            registry: registry.to_string(),
            checksum: source.source.checksum.clone(),
        });
    }
    selected.sort_by(|left, right| left.alias.cmp(&right.alias));
    if selected.windows(2).any(|window| window[0].alias == window[1].alias) {
        return Err(OvenLegacyCargoError::Plan(
            "generated project declares duplicate registry dependency aliases".to_string(),
        ));
    }
    Ok(selected)
}

/// Resolve and digest the exact registry sources available to a child of the explicit Loaf baker.
///
/// This is deliberately not a normal-command resolver. It runs locked, offline Cargo metadata at the already named
/// `legacy_cargo` boundary, joins those package IDs to the publisher lock checksums, and returns a typed authority
/// that a fixture child can consume without launching Cargo or searching an ambient Cargo home.
pub fn legacy_cargo_inspection_sources(
    cargo: &Path,
    cargo_manifest: &Path,
    features: &[String],
    inspection_packages: &[OvenLegacyCargoInspectionPackage],
    staging: &Path,
) -> Result<Vec<OvenLegacyCargoInspectionSource>, OvenLegacyCargoError> {
    let metadata = read_legacy_cargo_metadata(cargo, cargo_manifest, features)?;
    let package_root = cargo_manifest
        .parent()
        .ok_or_else(|| OvenLegacyCargoError::InvalidInput {
            field: "Cargo manifest",
            message: format!("{} has no package directory", cargo_manifest.display()),
        })?;
    let lock = regular_file_bytes(&package_root.join("Cargo.lock"))?;
    legacy_cargo_inspection_sources_from_metadata(
        &metadata,
        &lock,
        inspection_packages,
        InspectionPackageScope::ResolvedGraph,
        staging,
    )
}

/// Resolve inspection sources for one explicit user-requested project bake.
///
/// Unlike compiler-owned Loaf publication, a conventional Incan project may have only the semantic `incan.lock` and
/// no pre-existing Cargo.lock. This function therefore permits Cargo to create its first lock while remaining offline
/// and inside the named `incan oven bake` transaction. Its caller immediately seals the copied, digested sources and
/// the final direct-Rustc publisher records its independently generated lock. Normal build, run, and test cannot
/// call this helper.
pub fn explicit_project_bake_inspection_sources(
    cargo: &Path,
    cargo_manifest: &Path,
    features: &[String],
    inspection_packages: &[OvenLegacyCargoInspectionPackage],
    staging: &Path,
    release_registry_lock: Option<&Path>,
) -> Result<Vec<OvenLegacyCargoInspectionSource>, OvenLegacyCargoError> {
    let package_root = cargo_manifest
        .parent()
        .ok_or_else(|| OvenLegacyCargoError::InvalidInput {
            field: "Cargo manifest",
            message: format!("{} has no package directory", cargo_manifest.display()),
        })?;
    let metadata = match release_registry_lock {
        Some(release_registry_lock) => {
            stage_release_cohort_project_lock(cargo, package_root, release_registry_lock, features)?
        }
        None => read_legacy_cargo_metadata_with_lock_policy(cargo, cargo_manifest, features, false)?,
    };
    let lock = regular_file_bytes(&package_root.join("Cargo.lock"))?;
    legacy_cargo_inspection_sources_from_metadata(
        &metadata,
        &lock,
        inspection_packages,
        InspectionPackageScope::CompleteResolvedGraph,
        staging,
    )
}

/// Return the digest-verified registry lock owned by one selected release cohort.
fn verified_release_cohort_registry_lock(base: &OvenLegacyCargoBaseLoaf<'_>) -> Result<PathBuf, OvenLegacyCargoError> {
    base.artifacts
        .validate_shape(&base.artifacts.intent)
        .map_err(|error| OvenLegacyCargoError::Plan(format!("selected release-cohort plan is invalid: {error}")))?;
    let matches = base
        .artifacts
        .supporting_artifacts
        .iter()
        .filter(|artifact| artifact.relative_path == OVEN_RUSTC_REGISTRY_LOCK_RELATIVE_PATH)
        .collect::<Vec<_>>();
    let [declared] = matches.as_slice() else {
        return Err(OvenLegacyCargoError::Plan(format!(
            "selected release Loaf must declare exactly one `{OVEN_RUSTC_REGISTRY_LOCK_RELATIVE_PATH}` artifact, found {}",
            matches.len()
        )));
    };
    let path = base.artifact_root.join(OVEN_RUSTC_REGISTRY_LOCK_RELATIVE_PATH);
    let bytes = regular_file_bytes(&path)?;
    let actual = digest_bytes(&bytes);
    if actual != declared.digest {
        return Err(OvenLegacyCargoError::Plan(format!(
            "selected release Loaf registry lock digest mismatch: expected {}, got {actual}",
            declared.digest
        )));
    }
    Ok(path)
}

/// Resolve and stage every registry source in one locked compiler feature graph.
///
/// The compiler suite synthesizes several workspace locks during its tests. Sealing the canonical resolved graph
/// avoids a second dependency inventory that can drift from those locks, while still keeping this resolution inside
/// the named `legacy_cargo` baker.
pub fn legacy_cargo_resolved_registry_sources(
    cargo: &Path,
    cargo_manifest: &Path,
    features: &[String],
    staging: &Path,
) -> Result<Vec<OvenLegacyCargoInspectionSource>, OvenLegacyCargoError> {
    let metadata = read_legacy_cargo_metadata(cargo, cargo_manifest, features)?;
    let package_root = cargo_manifest
        .parent()
        .ok_or_else(|| OvenLegacyCargoError::InvalidInput {
            field: "Cargo manifest",
            message: format!("{} has no package directory", cargo_manifest.display()),
        })?;
    let lock = regular_file_bytes(&package_root.join("Cargo.lock"))?;
    legacy_cargo_inspection_sources_from_metadata(
        &metadata,
        &lock,
        &[],
        InspectionPackageScope::CompleteResolvedGraph,
        staging,
    )
}

/// Resolve one checked inspection surface from metadata already produced by the named publisher.
fn legacy_cargo_inspection_sources_from_metadata(
    metadata: &CargoMetadata,
    cargo_lock: &[u8],
    inspection_packages: &[OvenLegacyCargoInspectionPackage],
    scope: InspectionPackageScope,
    staging: &Path,
) -> Result<Vec<OvenLegacyCargoInspectionSource>, OvenLegacyCargoError> {
    let selected_package_ids = inspection_package_closure_ids(metadata, inspection_packages, scope)?;
    let checksums = cargo_registry_checksums(cargo_lock)?;
    let resolved_features = metadata
        .resolve
        .iter()
        .flat_map(|resolve| &resolve.nodes)
        .map(|node| (node.id.as_str(), node.features.clone()))
        .collect::<BTreeMap<_, _>>();
    let mut sources = Vec::new();
    for package in &metadata.packages {
        if !selected_package_ids.contains(&package.id) {
            continue;
        }
        let Some(registry) = package
            .source
            .as_deref()
            .filter(|source| source.starts_with("registry+"))
        else {
            continue;
        };
        let checksum = checksums
            .get(&(package.name.clone(), package.version.clone(), registry.to_string()))
            .cloned()
            .ok_or_else(|| {
                OvenLegacyCargoError::Plan(format!(
                    "publisher lock has no checksum for registry package `{}` {} from `{registry}`",
                    package.name, package.version
                ))
            })?;
        let source_root = package
            .manifest_path
            .parent()
            .ok_or_else(|| OvenLegacyCargoError::InvalidInput {
                field: "registry package manifest",
                message: format!("{} has no package directory", package.manifest_path.display()),
            })?;
        let mut package_features = resolved_features.get(package.id.as_str()).cloned().unwrap_or_default();
        package_features.sort();
        package_features.dedup();
        let (source_root, source_digest) = stage_registry_source_directory(
            staging,
            &package.name,
            &package.version,
            registry,
            &checksum,
            source_root,
        )?;
        sources.push(OvenLegacyCargoInspectionSource {
            package: package.name.clone(),
            version: package.version.clone(),
            registry: registry.to_string(),
            checksum,
            features: package_features,
            source_root,
            source_digest,
        });
    }
    sources.sort_by(|left, right| {
        (&left.package, &left.version, &left.registry, &left.checksum).cmp(&(
            &right.package,
            &right.version,
            &right.registry,
            &right.checksum,
        ))
    });
    Ok(sources)
}

/// Stage the private manifest for a future sealed third-party foundation compilation.
///
/// The staging project is deliberately not a normal output and contains no generated compiler root. It is prepared
/// before any compiler-root bootstrap is allowed, then removed with the publisher staging on every failure path.
fn stage_compiler_suite_foundation_manifest(
    staging: &Path,
    manifest: &str,
    source_lock: &Path,
    dependencies: &[CompilerSuiteFoundationDependency],
) -> Result<PathBuf, OvenLegacyCargoError> {
    let root = staging.join("third-party-foundation");
    let source_directory = root.join("src");
    fs::create_dir_all(&source_directory).map_err(|source| OvenLegacyCargoError::Io {
        path: source_directory.clone(),
        source,
    })?;
    let manifest_path = root.join("Cargo.toml");
    fs::write(&manifest_path, manifest).map_err(|source| OvenLegacyCargoError::Io {
        path: manifest_path.clone(),
        source,
    })?;
    let lock_path = root.join("Cargo.lock");
    let lock = compiler_suite_foundation_lock(&regular_file_bytes(source_lock)?, dependencies)?;
    fs::write(&lock_path, lock).map_err(|source| OvenLegacyCargoError::Io {
        path: lock_path,
        source,
    })?;
    let source_path = source_directory.join("lib.rs");
    fs::write(
        &source_path,
        "//! Private Oven third-party foundation publisher root.\n",
    )
    .map_err(|source| OvenLegacyCargoError::Io {
        path: source_path,
        source,
    })?;
    Ok(manifest_path)
}

/// Add the private foundation root to the compiler's already-resolved lock without changing its package graph.
///
/// Copying the compiler lock alone is insufficient: Cargo treats a different root package as an unlocked graph and
/// may select newer transitive versions that happen to exist in the ambient cache. The synthetic root names every
/// exact foundation package selected from the compiler unit graph, after which `--locked` can enforce that no
/// publisher dependency moves independently of the checked compiler lock.
fn compiler_suite_foundation_lock(
    source_lock: &[u8],
    dependencies: &[CompilerSuiteFoundationDependency],
) -> Result<Vec<u8>, OvenLegacyCargoError> {
    let source_lock = std::str::from_utf8(source_lock).map_err(|error| OvenLegacyCargoError::InvalidInput {
        field: "compiler Cargo.lock",
        message: error.to_string(),
    })?;
    let mut document =
        toml::from_str::<toml::Value>(source_lock).map_err(|error| OvenLegacyCargoError::InvalidInput {
            field: "compiler Cargo.lock",
            message: error.to_string(),
        })?;
    let packages = document
        .get_mut("package")
        .and_then(toml::Value::as_array_mut)
        .ok_or_else(|| OvenLegacyCargoError::InvalidInput {
            field: "compiler Cargo.lock",
            message: "must contain a package array".to_string(),
        })?;
    packages.retain(|package| package.get("name").and_then(toml::Value::as_str) != Some("oven-compiler-foundation"));
    let locked_packages = packages
        .iter()
        .filter_map(|package| {
            Some((
                package.get("name")?.as_str()?.to_string(),
                package.get("version")?.as_str()?.to_string(),
                package.get("source").and_then(toml::Value::as_str).map(str::to_string),
            ))
        })
        .collect::<Vec<_>>();
    let mut root_dependencies = Vec::with_capacity(dependencies.len());
    for dependency in dependencies {
        let matching = locked_packages
            .iter()
            .filter(|(name, version, source)| {
                name == &dependency.package && version == &dependency.version && source == &dependency.source
            })
            .count();
        if matching != 1 {
            return Err(OvenLegacyCargoError::Plan(format!(
                "compiler lock must contain exactly one `{}` {} package from {:?}, found {matching}",
                dependency.package, dependency.version, dependency.source
            )));
        }
        let same_name = locked_packages
            .iter()
            .filter(|(name, _, _)| name == &dependency.package)
            .count();
        let same_name_version = locked_packages
            .iter()
            .filter(|(name, version, _)| name == &dependency.package && version == &dependency.version)
            .count();
        let identity = if same_name == 1 {
            dependency.package.clone()
        } else if same_name_version == 1 {
            format!("{} {}", dependency.package, dependency.version)
        } else if let Some(source) = &dependency.source {
            format!("{} {} ({source})", dependency.package, dependency.version)
        } else {
            format!("{} {}", dependency.package, dependency.version)
        };
        root_dependencies.push(toml::Value::String(identity));
    }
    root_dependencies.sort_by(|left, right| left.as_str().cmp(&right.as_str()));
    root_dependencies.dedup();
    let mut root = toml::map::Map::new();
    root.insert(
        "name".to_string(),
        toml::Value::String("oven-compiler-foundation".to_string()),
    );
    root.insert("version".to_string(), toml::Value::String("0.0.0".to_string()));
    root.insert("dependencies".to_string(), toml::Value::Array(root_dependencies));
    packages.push(toml::Value::Table(root));
    prune_lock_to_package(&mut document, "oven-compiler-foundation", "0.0.0", None)?;
    toml::to_string_pretty(&document)
        .map(String::into_bytes)
        .map_err(|error| OvenLegacyCargoError::InvalidInput {
            field: "compiler Cargo.lock",
            message: error.to_string(),
        })
}

/// Stage a generated Loaf fixture with the compiler's exact registry lock and its explicit local path packages.
///
/// Checked fixture manifests intentionally exercise a subset of the compiler and SDK closure. Letting Cargo resolve
/// that subset from an ambient offline index can choose a newer cached transitive than the compiler itself uses.
/// This helper carries the compiler's locked registry graph forward, adds only the generated root and local path
/// package records Cargo requires, and makes the subsequent named publisher invocation unconditionally locked.
pub fn stage_locked_loaf_fixture(
    cargo: &Path,
    generated_project: &Path,
    compiler_lock: &Path,
) -> Result<(), OvenLegacyCargoError> {
    let generated_project = canonical_directory(generated_project, "generated Loaf fixture")?;
    let manifest_path = generated_project.join("Cargo.toml");
    let manifest = regular_file_bytes(&manifest_path)?;
    let source_lock = regular_file_bytes(compiler_lock)?;
    let lock = locked_generated_project(&manifest_path, &manifest, &source_lock)?;
    let lock_path = generated_project.join("Cargo.lock");
    fs::write(&lock_path, lock).map_err(|source| OvenLegacyCargoError::Io {
        path: lock_path.clone(),
        source,
    })?;

    // Cargo owns local feature unification and therefore the dependency lists attached to path-package lock
    // records. Let it normalize only that local graph while offline, then reject the result unless every registry
    // identity and checksum is a member of the checked compiler lock. All later compilation sees the normalized
    // file and is unconditionally `--locked`.
    let cargo = canonical_tool_file(cargo, "cargo")?;
    let mut command = Command::new(&cargo);
    command
        .current_dir(&generated_project)
        .arg("metadata")
        .arg("--manifest-path")
        .arg(&manifest_path)
        .args(["--offline", "--format-version", "1"]);
    clear_inherited_cargo_environment(&mut command);
    let output = command
        .output()
        .map_err(|source| OvenLegacyCargoError::Io { path: cargo, source })?;
    if !output.status.success() {
        return Err(OvenLegacyCargoError::CargoFailed {
            output: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        });
    }
    validate_generated_registry_lock(&source_lock, &regular_file_bytes(&lock_path)?)
}

/// Seed an explicit project publisher from one release cohort without excluding project-only registry packages.
///
/// The selected release lock pins every retained release identity and dependency edge. Cargo may add project-owned
/// package identities, including alternate versions of packages that also occur in the release graph, but it may not
/// mutate the release graph itself. The normalized lock is written before the actual publisher build, so that build
/// is unconditionally `--locked`.
fn stage_release_cohort_project_lock(
    cargo: &Path,
    generated_project: &Path,
    release_lock: &Path,
    features: &[String],
) -> Result<CargoMetadata, OvenLegacyCargoError> {
    let generated_project = canonical_directory(generated_project, "generated release-cohort project")?;
    let manifest_path = generated_project.join("Cargo.toml");
    let manifest = regular_file_bytes(&manifest_path)?;
    let release_lock_bytes = regular_file_bytes(release_lock)?;
    let lock = release_cohort_generated_project_lock(&manifest_path, &manifest, &release_lock_bytes)?;
    let lock_path = generated_project.join("Cargo.lock");
    fs::write(&lock_path, &lock).map_err(|source| OvenLegacyCargoError::Io {
        path: lock_path.clone(),
        source,
    })?;

    // Normalize local and newly introduced project-only edges while preserving every compatible release pin already
    // present in the seed. This is the sole unlocked metadata invocation and remains inside the explicit baker.
    let metadata = read_legacy_cargo_metadata_with_lock_policy(cargo, &manifest_path, features, false)?;
    validate_release_cohort_registry_lock(&release_lock_bytes, &lock, &regular_file_bytes(&lock_path)?)?;
    Ok(metadata)
}

type CargoLockPackageIdentity = (String, String, Option<String>);

/// One resolved package node from a Cargo lock graph.
struct CargoLockPackageNode {
    checksum: Option<String>,
    dependencies: BTreeSet<CargoLockPackageIdentity>,
}

/// Decode a Cargo lock into exact package identities and resolved dependency edges.
fn cargo_lock_package_graph(
    cargo_lock: &[u8],
    field: &'static str,
) -> Result<BTreeMap<CargoLockPackageIdentity, CargoLockPackageNode>, OvenLegacyCargoError> {
    let lock = toml::from_slice::<toml::Value>(cargo_lock).map_err(|error| OvenLegacyCargoError::InvalidInput {
        field,
        message: error.to_string(),
    })?;
    let packages =
        lock.get("package")
            .and_then(toml::Value::as_array)
            .ok_or_else(|| OvenLegacyCargoError::InvalidInput {
                field,
                message: "must contain a package array".to_string(),
            })?;
    let identities = lock_package_identities(&lock);
    if packages.len() != identities.len() {
        return Err(OvenLegacyCargoError::InvalidInput {
            field,
            message: "every package must declare a name and version".to_string(),
        });
    }
    if identities.iter().collect::<BTreeSet<_>>().len() != identities.len() {
        return Err(OvenLegacyCargoError::InvalidInput {
            field,
            message: "must not contain duplicate package identities".to_string(),
        });
    }
    let mut references = BTreeMap::new();
    for identity in &identities {
        let same_name = identities.iter().filter(|candidate| candidate.0 == identity.0).count();
        let same_name_version = identities
            .iter()
            .filter(|candidate| candidate.0 == identity.0 && candidate.1 == identity.1)
            .count();
        let mut aliases = Vec::new();
        if same_name == 1 {
            aliases.push(identity.0.clone());
        }
        if same_name_version == 1 {
            aliases.push(format!("{} {}", identity.0, identity.1));
        }
        if let Some(source) = identity.2.as_deref() {
            aliases.push(format!("{} {} ({source})", identity.0, identity.1));
        }
        for reference in aliases {
            if let Some(previous) = references.insert(reference.clone(), identity.clone())
                && previous != *identity
            {
                return Err(OvenLegacyCargoError::Plan(format!(
                    "{field} has ambiguous dependency reference `{reference}`"
                )));
            }
        }
    }
    let mut graph = BTreeMap::new();
    for (package, identity) in packages.iter().zip(identities) {
        let dependencies = package
            .get("dependencies")
            .and_then(toml::Value::as_array)
            .into_iter()
            .flatten()
            .map(|dependency| {
                let dependency = dependency.as_str().ok_or_else(|| OvenLegacyCargoError::InvalidInput {
                    field,
                    message: "package dependency must be a string".to_string(),
                })?;
                references.get(dependency).cloned().ok_or_else(|| {
                    OvenLegacyCargoError::Plan(format!("{field} dependency `{dependency}` has no exact package record"))
                })
            })
            .collect::<Result<BTreeSet<_>, _>>()?;
        let node = CargoLockPackageNode {
            checksum: package
                .get("checksum")
                .and_then(toml::Value::as_str)
                .map(str::to_string),
            dependencies,
        };
        if graph.insert(identity.clone(), node).is_some() {
            return Err(OvenLegacyCargoError::Plan(format!(
                "{field} contains duplicate package `{}` {} from {:?}",
                identity.0, identity.1, identity.2
            )));
        }
    }
    Ok(graph)
}

/// Verify that Cargo retained the seeded release graph while admitting project-only registry coordinates.
///
/// Package names are not globally owned by a release: a project dependency may legitimately require an incompatible
/// version of a package also used by the standard library. The invariant is instead graph-shaped. Every package and
/// dependency edge on a release-owned local/path node retained by the generated root must remain a subset of the
/// release graph because its compiled artifact is later replaced by the complete release copy. Cargo may prune
/// feature-disabled local edges, normalize target- and feature-sensitive dependency lists on registry nodes, or
/// prune unreachable release nodes; their exact coordinate/checksum and the later feature-bound artifact catalog
/// remain the authority. Extra registry coordinates remain project-owned.
fn validate_release_cohort_registry_lock(
    release_lock: &[u8],
    seeded_lock: &[u8],
    generated_lock: &[u8],
) -> Result<(), OvenLegacyCargoError> {
    let release_checksums = cargo_registry_checksums(release_lock)?;
    let release = cargo_lock_package_graph(release_lock, "selected release Cargo.lock")?;
    let seeded = cargo_lock_package_graph(seeded_lock, "release-derived project Cargo.lock")?;
    let generated = cargo_lock_package_graph(generated_lock, "generated project Cargo.lock")?;

    for (stage, graph) in [("release-derived project", &seeded), ("generated project", &generated)] {
        for (identity, node) in graph {
            let Some(release_node) = release.get(identity) else {
                continue;
            };
            let is_registry = identity
                .2
                .as_deref()
                .is_some_and(|source| source.starts_with("registry+"));
            if !is_registry && !node.dependencies.is_subset(&release_node.dependencies) {
                return Err(OvenLegacyCargoError::Plan(format!(
                    "{stage} lock changed the release-derived dependency edges for `{}` {}",
                    identity.0, identity.1,
                )));
            }
            if is_registry && node.checksum != release_node.checksum {
                return Err(OvenLegacyCargoError::Plan(format!(
                    "{stage} lock changed the release-derived checksum for `{}` {}",
                    identity.0, identity.1,
                )));
            }
        }
    }

    for (identity, generated_node) in &generated {
        let Some(source) = identity.2.as_deref() else {
            continue;
        };
        if !source.starts_with("registry+") {
            return Err(OvenLegacyCargoError::Plan(format!(
                "generated project lock selected unsupported source `{source}` for `{}` {}",
                identity.0, identity.1
            )));
        }
        let Some(release_checksum) =
            release_checksums.get(&(identity.0.clone(), identity.1.clone(), source.to_string()))
        else {
            continue;
        };
        if generated_node.checksum.as_ref() != Some(release_checksum) {
            return Err(OvenLegacyCargoError::Plan(format!(
                "generated project lock checksum for release coordinate `{}` {} disagrees with the selected release cohort",
                identity.0, identity.1
            )));
        }
    }
    Ok(())
}

/// Prove that local Cargo normalization introduced no registry identity outside the checked compiler lock.
fn validate_generated_registry_lock(compiler_lock: &[u8], generated_lock: &[u8]) -> Result<(), OvenLegacyCargoError> {
    let compiler_checksums = cargo_registry_checksums(compiler_lock)?;
    let generated = toml::from_slice::<CargoChecksumLock>(generated_lock).map_err(|error| {
        OvenLegacyCargoError::Plan(format!(
            "generated Loaf Cargo.lock is not valid checksum authority: {error}"
        ))
    })?;
    for package in generated.package {
        let Some(source) = package.source else {
            continue;
        };
        if !source.starts_with("registry+") {
            return Err(OvenLegacyCargoError::Plan(format!(
                "generated Loaf lock selected unsupported source `{source}` for `{}` {}",
                package.name, package.version
            )));
        }
        let checksum = package
            .checksum
            .filter(|checksum| !checksum.trim().is_empty())
            .ok_or_else(|| {
                OvenLegacyCargoError::Plan(format!(
                    "generated Loaf lock omits the checksum for registry package `{}` {}",
                    package.name, package.version
                ))
            })?;
        let key = (package.name, package.version, source);
        let Some(compiler_checksum) = compiler_checksums.get(&key) else {
            return Err(OvenLegacyCargoError::Plan(format!(
                "generated Loaf lock selected registry package `{}` {} outside the checked compiler lock",
                key.0, key.1
            )));
        };
        if compiler_checksum != &checksum {
            return Err(OvenLegacyCargoError::Plan(format!(
                "generated Loaf lock checksum for `{}` {} disagrees with the checked compiler lock",
                key.0, key.1
            )));
        }
    }
    Ok(())
}

/// Extend one checked compiler lock with the local packages reachable from a generated Cargo manifest.
fn locked_generated_project(
    manifest_path: &Path,
    manifest: &[u8],
    source_lock: &[u8],
) -> Result<Vec<u8>, OvenLegacyCargoError> {
    locked_generated_project_with_registry_policy(manifest_path, manifest, source_lock, false)
}

/// Seed a generated project from one release lock while allowing genuinely project-owned registry packages.
fn release_cohort_generated_project_lock(
    manifest_path: &Path,
    manifest: &[u8],
    source_lock: &[u8],
) -> Result<Vec<u8>, OvenLegacyCargoError> {
    locked_generated_project_with_registry_policy(manifest_path, manifest, source_lock, true)
}

/// Extend one checked release lock with local packages reachable from a generated Cargo manifest.
fn locked_generated_project_with_registry_policy(
    manifest_path: &Path,
    manifest: &[u8],
    source_lock: &[u8],
    allow_project_registry_packages: bool,
) -> Result<Vec<u8>, OvenLegacyCargoError> {
    let source_lock = std::str::from_utf8(source_lock).map_err(|error| OvenLegacyCargoError::InvalidInput {
        field: "compiler Cargo.lock",
        message: error.to_string(),
    })?;
    let mut lock = toml::from_str::<toml::Value>(source_lock).map_err(|error| OvenLegacyCargoError::InvalidInput {
        field: "compiler Cargo.lock",
        message: error.to_string(),
    })?;
    let manifest_text = std::str::from_utf8(manifest).map_err(|error| OvenLegacyCargoError::InvalidInput {
        field: "generated Loaf Cargo.toml",
        message: error.to_string(),
    })?;
    let manifest =
        toml::from_str::<toml::Value>(manifest_text).map_err(|error| OvenLegacyCargoError::InvalidInput {
            field: "generated Loaf Cargo.toml",
            message: error.to_string(),
        })?;
    let mut visiting = BTreeSet::new();
    let root = locked_local_package(
        manifest_path,
        &manifest,
        &mut lock,
        &mut visiting,
        true,
        allow_project_registry_packages,
    )?;
    let packages = lock
        .get_mut("package")
        .and_then(toml::Value::as_array_mut)
        .ok_or_else(|| OvenLegacyCargoError::InvalidInput {
            field: "compiler Cargo.lock",
            message: "must contain a package array".to_string(),
        })?;
    packages.retain(|package| {
        !(package.get("name").and_then(toml::Value::as_str) == Some(root.name.as_str())
            && package.get("version").and_then(toml::Value::as_str) == Some(root.version.as_str())
            && package.get("source").is_none())
    });
    packages.push(root.value);
    prune_lock_to_package(&mut lock, &root.name, &root.version, None)?;
    toml::to_string_pretty(&lock)
        .map(String::into_bytes)
        .map_err(|error| OvenLegacyCargoError::InvalidInput {
            field: "generated Loaf Cargo.lock",
            message: error.to_string(),
        })
}

/// One local package record ready to append to a staged lock.
struct LockedLocalPackage {
    name: String,
    version: String,
    value: toml::Value,
}

/// Cargo workspace authority inherited by one local package manifest.
///
/// The manifest is retained alongside its canonical root so inherited dependency paths are resolved against the
/// workspace that declared them rather than the member that selected them. This effective projection is then folded
/// into the generated lock graph, which binds the selected workspace package and dependency authority into the same
/// identity used by the explicit publisher.
struct LocalCargoWorkspaceAuthority {
    root: PathBuf,
    manifest_path: PathBuf,
    manifest: toml::Value,
}

/// One dependency specification after applying Cargo workspace inheritance.
struct EffectiveLocalCargoDependency {
    alias: String,
    specification: toml::Value,
    declaration_root: PathBuf,
    inherited_workspace: bool,
}

/// Digest only the effective Cargo workspace facts selected by one local package.
///
/// The package's own tree remains separate source authority. This supplemental identity includes each
/// `[workspace.package]` field explicitly selected with `workspace = true` and every effective
/// `[workspace.dependencies]` declaration selected by ordinary, build, dev, or target-specific dependencies. A
/// selected workspace-relative path dependency contributes its Cargo-semantic source digest instead of a local path.
/// Packages with no inherited workspace facts return `None`, avoiding invalidation from unrelated workspace edits.
pub(crate) fn digest_local_cargo_workspace_authority(
    package_root: &Path,
) -> Result<Option<String>, OvenLegacyCargoError> {
    let manifest_path = verified_regular_file(
        &package_root.join("Cargo.toml"),
        "local Cargo package workspace authority",
    )?;
    let manifest_bytes = fs::read(&manifest_path).map_err(|source| OvenLegacyCargoError::Io {
        path: manifest_path.clone(),
        source,
    })?;
    let manifest =
        toml::from_slice::<toml::Value>(&manifest_bytes).map_err(|error| OvenLegacyCargoError::InvalidInput {
            field: "local Cargo package workspace authority",
            message: format!("{} is not valid TOML: {error}", manifest_path.display()),
        })?;
    let workspace = local_cargo_workspace_authority(&manifest_path, &manifest)?;
    let mut records = BTreeSet::new();
    if let Some(package) = manifest.get("package").and_then(toml::Value::as_table) {
        for (field, selection) in package {
            let Some(selection) = selection.as_table().and_then(|selection| selection.get("workspace")) else {
                continue;
            };
            if selection.as_bool() != Some(true) {
                return Err(OvenLegacyCargoError::InvalidInput {
                    field: "local Cargo package workspace authority",
                    message: format!(
                        "{} package field `{field}` must set workspace = true to inherit workspace authority",
                        manifest_path.display()
                    ),
                });
            }
            let workspace = workspace.as_ref().ok_or_else(|| OvenLegacyCargoError::InvalidInput {
                field: "local Cargo package workspace authority",
                message: format!(
                    "{} package field `{field}` inherits [workspace.package] but no containing Cargo workspace was found",
                    manifest_path.display()
                ),
            })?;
            let inherited = workspace
                .manifest
                .get("workspace")
                .and_then(toml::Value::as_table)
                .and_then(|workspace| workspace.get("package"))
                .and_then(toml::Value::as_table)
                .and_then(|package| package.get(field))
                .ok_or_else(|| OvenLegacyCargoError::InvalidInput {
                    field: "local Cargo package workspace authority",
                    message: format!(
                        "{} has no [workspace.package].{field} inherited by {}",
                        workspace.manifest_path.display(),
                        manifest_path.display()
                    ),
                })?;
            records.insert(format!(
                "package:{field}:{}",
                serde_json::to_string(inherited).map_err(|error| OvenLegacyCargoError::Plan(error.to_string()))?
            ));
        }
    }
    let mut resolved_packages = BTreeMap::new();
    for dependency in manifest_dependency_entries(&manifest_path, &manifest, workspace.as_ref())?
        .into_iter()
        .filter(|dependency| dependency.inherited_workspace)
    {
        let mut specification = dependency.specification;
        if let Some(table) = specification.as_table_mut()
            && let Some(path) = table.get("path").and_then(toml::Value::as_str)
        {
            let dependency_root = dependency.declaration_root.join(path);
            let digest =
                digest_toolchain_source_tree_with_cache(&dependency_root, &mut resolved_packages).map_err(|error| {
                    OvenLegacyCargoError::Plan(format!(
                        "cannot digest inherited workspace dependency `{}` at {}: {error}",
                        dependency.alias,
                        dependency_root.display()
                    ))
                })?;
            table.insert(
                "path".to_string(),
                toml::Value::String(format!("incan-cargo-package:{digest}")),
            );
        }
        records.insert(format!(
            "dependency:{}:{}",
            dependency.alias,
            serde_json::to_string(&specification).map_err(|error| OvenLegacyCargoError::Plan(error.to_string()))?
        ));
    }
    if records.is_empty() {
        return Ok(None);
    }
    let payload = serde_json::to_vec(&records).map_err(|error| OvenLegacyCargoError::Plan(error.to_string()))?;
    Ok(Some(digest_bytes(&payload)))
}

/// Materialize one local package record and recursively add path dependencies missing from the compiler lock.
fn locked_local_package(
    manifest_path: &Path,
    manifest: &toml::Value,
    lock: &mut toml::Value,
    visiting: &mut BTreeSet<PathBuf>,
    force_record: bool,
    allow_project_registry_packages: bool,
) -> Result<LockedLocalPackage, OvenLegacyCargoError> {
    let manifest_path = verified_regular_file(manifest_path, "generated Loaf Cargo.toml")?;
    if !visiting.insert(manifest_path.clone()) {
        return Err(OvenLegacyCargoError::Plan(format!(
            "generated Loaf path dependency cycle reaches {}",
            manifest_path.display()
        )));
    }
    let package =
        manifest
            .get("package")
            .and_then(toml::Value::as_table)
            .ok_or_else(|| OvenLegacyCargoError::InvalidInput {
                field: "generated Loaf Cargo.toml",
                message: format!("{} has no [package] table", manifest_path.display()),
            })?;
    let name = package
        .get("name")
        .and_then(toml::Value::as_str)
        .ok_or_else(|| OvenLegacyCargoError::InvalidInput {
            field: "generated Loaf Cargo.toml",
            message: format!("{} has no package name", manifest_path.display()),
        })?
        .to_string();
    let workspace = local_cargo_workspace_authority(&manifest_path, manifest)?;
    let (version, detached_compiler_lock_authority) =
        if let Some(version) = package.get("version").and_then(toml::Value::as_str) {
            (version.to_string(), false)
        } else if package
            .get("version")
            .and_then(toml::Value::as_table)
            .and_then(|version| version.get("workspace"))
            .and_then(toml::Value::as_bool)
            == Some(true)
        {
            match workspace.as_ref() {
                Some(workspace) => (inherited_workspace_package_version(workspace, &manifest_path)?, false),
                None => (detached_compiler_package_version(lock, &name, &manifest_path)?, true),
            }
        } else {
            return Err(OvenLegacyCargoError::InvalidInput {
                field: "generated Loaf Cargo.toml",
                message: format!("{} has no package version", manifest_path.display()),
            });
        };
    if !force_record && detached_compiler_lock_authority && lock_contains_package(lock, &name, &version, None) {
        visiting.remove(&manifest_path);
        return Ok(LockedLocalPackage {
            name,
            version,
            value: toml::Value::Table(toml::map::Map::new()),
        });
    }
    let mut dependencies = Vec::new();
    for dependency in manifest_dependency_entries(&manifest_path, manifest, workspace.as_ref())? {
        let dependency = locked_manifest_dependency(
            &dependency.declaration_root,
            &dependency.alias,
            &dependency.specification,
            lock,
            visiting,
            allow_project_registry_packages,
        )?;
        if let Some(dependency) = dependency {
            dependencies.push(dependency);
        }
    }
    dependencies.sort();
    dependencies.dedup();
    let mut value = toml::map::Map::new();
    value.insert("name".to_string(), toml::Value::String(name.clone()));
    value.insert("version".to_string(), toml::Value::String(version.clone()));
    if !dependencies.is_empty() {
        value.insert(
            "dependencies".to_string(),
            toml::Value::Array(dependencies.into_iter().map(toml::Value::String).collect()),
        );
    }
    if !force_record {
        let packages = lock
            .get_mut("package")
            .and_then(toml::Value::as_array_mut)
            .ok_or_else(|| OvenLegacyCargoError::InvalidInput {
                field: "compiler Cargo.lock",
                message: "must contain a package array".to_string(),
            })?;
        packages.retain(|package| {
            !(package.get("name").and_then(toml::Value::as_str) == Some(name.as_str())
                && package.get("version").and_then(toml::Value::as_str) == Some(version.as_str())
                && package.get("source").is_none())
        });
    }
    visiting.remove(&manifest_path);
    Ok(LockedLocalPackage {
        name,
        version,
        value: toml::Value::Table(value),
    })
}

/// Locate explicit Cargo workspace authority, otherwise the nearest containing workspace manifest.
fn local_cargo_workspace_authority(
    manifest_path: &Path,
    manifest: &toml::Value,
) -> Result<Option<LocalCargoWorkspaceAuthority>, OvenLegacyCargoError> {
    let manifest_root = manifest_path
        .parent()
        .ok_or_else(|| OvenLegacyCargoError::InvalidInput {
            field: "generated Loaf Cargo.toml",
            message: format!("{} has no package directory", manifest_path.display()),
        })?;
    if let Some(explicit_workspace) = manifest
        .get("package")
        .and_then(toml::Value::as_table)
        .and_then(|package| package.get("workspace"))
    {
        let explicit_workspace = explicit_workspace
            .as_str()
            .ok_or_else(|| OvenLegacyCargoError::InvalidInput {
                field: "generated Loaf Cargo.toml",
                message: format!("{} has a non-string package.workspace path", manifest_path.display()),
            })?;
        return read_local_cargo_workspace_manifest(&manifest_root.join(explicit_workspace).join("Cargo.toml"))
            .map(Some);
    }
    if manifest.get("workspace").is_some() {
        if manifest.get("workspace").and_then(toml::Value::as_table).is_none() {
            return Err(OvenLegacyCargoError::InvalidInput {
                field: "generated Loaf Cargo.toml",
                message: format!("{} has a non-table [workspace] value", manifest_path.display()),
            });
        }
        return Ok(Some(LocalCargoWorkspaceAuthority {
            root: manifest_root.to_path_buf(),
            manifest_path: manifest_path.to_path_buf(),
            manifest: manifest.clone(),
        }));
    }
    for ancestor in manifest_root.ancestors().skip(1) {
        let candidate = ancestor.join("Cargo.toml");
        if !candidate.is_file() {
            continue;
        }
        let workspace = read_local_cargo_manifest(&candidate)?;
        match workspace.manifest.get("workspace") {
            Some(value) if value.is_table() => return Ok(Some(workspace)),
            Some(_) => {
                return Err(OvenLegacyCargoError::InvalidInput {
                    field: "generated Loaf workspace Cargo.toml",
                    message: format!(
                        "{} has a non-table [workspace] value",
                        workspace.manifest_path.display()
                    ),
                });
            }
            None => {}
        }
    }
    Ok(None)
}

/// Read and validate one explicitly selected Cargo workspace manifest.
fn read_local_cargo_workspace_manifest(
    manifest_path: &Path,
) -> Result<LocalCargoWorkspaceAuthority, OvenLegacyCargoError> {
    let workspace = read_local_cargo_manifest(manifest_path)?;
    if workspace
        .manifest
        .get("workspace")
        .and_then(toml::Value::as_table)
        .is_none()
    {
        return Err(OvenLegacyCargoError::InvalidInput {
            field: "generated Loaf workspace Cargo.toml",
            message: format!(
                "{} selected by package.workspace has no [workspace] table",
                workspace.manifest_path.display()
            ),
        });
    }
    Ok(workspace)
}

/// Parse one candidate workspace manifest while retaining its canonical path for diagnostics and dependency roots.
fn read_local_cargo_manifest(manifest_path: &Path) -> Result<LocalCargoWorkspaceAuthority, OvenLegacyCargoError> {
    let manifest_path = verified_regular_file(manifest_path, "generated Loaf workspace Cargo.toml")?;
    let bytes = fs::read(&manifest_path).map_err(|source| OvenLegacyCargoError::Io {
        path: manifest_path.clone(),
        source,
    })?;
    let manifest = toml::from_slice::<toml::Value>(&bytes).map_err(|error| OvenLegacyCargoError::InvalidInput {
        field: "generated Loaf workspace Cargo.toml",
        message: format!("{} is not valid TOML: {error}", manifest_path.display()),
    })?;
    let root = manifest_path
        .parent()
        .ok_or_else(|| OvenLegacyCargoError::InvalidInput {
            field: "generated Loaf workspace Cargo.toml",
            message: format!("{} has no workspace directory", manifest_path.display()),
        })?
        .to_path_buf();
    Ok(LocalCargoWorkspaceAuthority {
        root,
        manifest_path,
        manifest,
    })
}

/// Resolve the lock identity field inherited from `[workspace.package]`.
fn inherited_workspace_package_version(
    workspace: &LocalCargoWorkspaceAuthority,
    package_manifest_path: &Path,
) -> Result<String, OvenLegacyCargoError> {
    workspace
        .manifest
        .get("workspace")
        .and_then(toml::Value::as_table)
        .and_then(|workspace| workspace.get("package"))
        .and_then(toml::Value::as_table)
        .and_then(|package| package.get("version"))
        .and_then(toml::Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| OvenLegacyCargoError::InvalidInput {
            field: "generated Loaf workspace Cargo.toml",
            message: format!(
                "{} has no string [workspace.package].version inherited by {}",
                workspace.manifest_path.display(),
                package_manifest_path.display()
            ),
        })
}

/// Retain the checked compiler-lock fallback only for a package detached from its source workspace.
fn detached_compiler_package_version(
    lock: &toml::Value,
    name: &str,
    manifest_path: &Path,
) -> Result<String, OvenLegacyCargoError> {
    let candidates = lock_package_identities(lock)
        .into_iter()
        .filter(|(candidate_name, _, source)| candidate_name == name && source.is_none())
        .collect::<Vec<_>>();
    let [(_, version, _)] = candidates.as_slice() else {
        return Err(OvenLegacyCargoError::Plan(format!(
            "detached workspace package `{name}` at {} must select exactly one local version from the checked compiler lock, found {}",
            manifest_path.display(),
            candidates.len()
        )));
    };
    Ok(version.clone())
}

/// Collect effective dependency specifications from ordinary and target-specific Cargo manifest tables.
fn manifest_dependency_entries(
    manifest_path: &Path,
    manifest: &toml::Value,
    workspace: Option<&LocalCargoWorkspaceAuthority>,
) -> Result<Vec<EffectiveLocalCargoDependency>, OvenLegacyCargoError> {
    const SECTIONS: [&str; 3] = ["dependencies", "build-dependencies", "dev-dependencies"];
    let manifest_root = manifest_path
        .parent()
        .ok_or_else(|| OvenLegacyCargoError::InvalidInput {
            field: "generated Loaf Cargo.toml",
            message: format!("{} has no package directory", manifest_path.display()),
        })?;
    let mut entries = Vec::new();
    for section in SECTIONS {
        if let Some(dependencies) = manifest.get(section).and_then(toml::Value::as_table) {
            collect_manifest_dependency_entries(manifest_path, manifest_root, dependencies, workspace, &mut entries)?;
        }
    }
    if let Some(targets) = manifest.get("target").and_then(toml::Value::as_table) {
        for target in targets.values().filter_map(toml::Value::as_table) {
            for section in SECTIONS {
                if let Some(dependencies) = target.get(section).and_then(toml::Value::as_table) {
                    collect_manifest_dependency_entries(
                        manifest_path,
                        manifest_root,
                        dependencies,
                        workspace,
                        &mut entries,
                    )?;
                }
            }
        }
    }
    Ok(entries)
}

/// Apply workspace inheritance to one dependency table without asking Cargo to rediscover its authority.
fn collect_manifest_dependency_entries(
    manifest_path: &Path,
    manifest_root: &Path,
    dependencies: &toml::map::Map<String, toml::Value>,
    workspace: Option<&LocalCargoWorkspaceAuthority>,
    entries: &mut Vec<EffectiveLocalCargoDependency>,
) -> Result<(), OvenLegacyCargoError> {
    for (alias, specification) in dependencies {
        let Some(member_table) = specification.as_table() else {
            entries.push(EffectiveLocalCargoDependency {
                alias: alias.clone(),
                specification: specification.clone(),
                declaration_root: manifest_root.to_path_buf(),
                inherited_workspace: false,
            });
            continue;
        };
        let Some(inherits) = member_table.get("workspace") else {
            entries.push(EffectiveLocalCargoDependency {
                alias: alias.clone(),
                specification: specification.clone(),
                declaration_root: manifest_root.to_path_buf(),
                inherited_workspace: false,
            });
            continue;
        };
        if inherits.as_bool() != Some(true) {
            return Err(OvenLegacyCargoError::InvalidInput {
                field: "generated Loaf Cargo.toml",
                message: format!(
                    "{} dependency `{alias}` must set workspace = true to inherit workspace authority",
                    manifest_path.display()
                ),
            });
        }
        let workspace = workspace.ok_or_else(|| OvenLegacyCargoError::InvalidInput {
            field: "generated Loaf Cargo.toml",
            message: format!(
                "{} dependency `{alias}` inherits [workspace.dependencies] but no containing Cargo workspace was found",
                manifest_path.display()
            ),
        })?;
        let inherited = workspace
            .manifest
            .get("workspace")
            .and_then(toml::Value::as_table)
            .and_then(|workspace| workspace.get("dependencies"))
            .and_then(toml::Value::as_table)
            .and_then(|dependencies| dependencies.get(alias))
            .ok_or_else(|| OvenLegacyCargoError::InvalidInput {
                field: "generated Loaf workspace Cargo.toml",
                message: format!(
                    "{} has no [workspace.dependencies].{alias} inherited by {}",
                    workspace.manifest_path.display(),
                    manifest_path.display()
                ),
            })?;
        entries.push(EffectiveLocalCargoDependency {
            alias: alias.clone(),
            specification: merged_workspace_dependency_specification(
                alias,
                inherited,
                member_table,
                &workspace.manifest_path,
                manifest_path,
            )?,
            declaration_root: workspace.root.clone(),
            inherited_workspace: true,
        });
    }
    Ok(())
}

/// Merge Cargo's member-local workspace dependency modifiers into the selected workspace declaration.
fn merged_workspace_dependency_specification(
    alias: &str,
    inherited: &toml::Value,
    member: &toml::map::Map<String, toml::Value>,
    workspace_manifest_path: &Path,
    member_manifest_path: &Path,
) -> Result<toml::Value, OvenLegacyCargoError> {
    let mut effective = match inherited {
        toml::Value::String(version) => {
            toml::map::Map::from_iter([("version".to_string(), toml::Value::String(version.clone()))])
        }
        toml::Value::Table(table) => table.clone(),
        _ => {
            return Err(OvenLegacyCargoError::InvalidInput {
                field: "generated Loaf workspace Cargo.toml",
                message: format!(
                    "{} [workspace.dependencies].{alias} must be a version string or dependency table",
                    workspace_manifest_path.display()
                ),
            });
        }
    };
    for (key, value) in member {
        match key.as_str() {
            "workspace" => {}
            "features" => merge_workspace_dependency_features(
                alias,
                &mut effective,
                value,
                workspace_manifest_path,
                member_manifest_path,
            )?,
            "optional" | "default-features" | "public" if value.is_bool() => {
                effective.insert(key.clone(), value.clone());
            }
            "optional" | "default-features" | "public" => {
                return Err(OvenLegacyCargoError::InvalidInput {
                    field: "generated Loaf Cargo.toml",
                    message: format!(
                        "{} dependency `{alias}` has a non-boolean `{key}` workspace modifier",
                        member_manifest_path.display()
                    ),
                });
            }
            _ => {
                return Err(OvenLegacyCargoError::InvalidInput {
                    field: "generated Loaf Cargo.toml",
                    message: format!(
                        "{} dependency `{alias}` cannot override workspace authority field `{key}`",
                        member_manifest_path.display()
                    ),
                });
            }
        }
    }
    if effective.get("workspace").is_some() {
        return Err(OvenLegacyCargoError::InvalidInput {
            field: "generated Loaf workspace Cargo.toml",
            message: format!(
                "{} [workspace.dependencies].{alias} cannot inherit from another workspace dependency",
                workspace_manifest_path.display()
            ),
        });
    }
    Ok(toml::Value::Table(effective))
}

/// Union the feature sets contributed by the workspace and its member declaration.
fn merge_workspace_dependency_features(
    alias: &str,
    effective: &mut toml::map::Map<String, toml::Value>,
    member_features: &toml::Value,
    workspace_manifest_path: &Path,
    member_manifest_path: &Path,
) -> Result<(), OvenLegacyCargoError> {
    let mut features = BTreeSet::new();
    if let Some(workspace_features) = effective.get("features") {
        let workspace_features = workspace_features
            .as_array()
            .ok_or_else(|| OvenLegacyCargoError::InvalidInput {
                field: "generated Loaf workspace Cargo.toml",
                message: format!(
                    "{} [workspace.dependencies].{alias}.features must be an array",
                    workspace_manifest_path.display()
                ),
            })?;
        for feature in workspace_features {
            let feature = feature.as_str().ok_or_else(|| OvenLegacyCargoError::InvalidInput {
                field: "generated Loaf workspace Cargo.toml",
                message: format!(
                    "{} [workspace.dependencies].{alias}.features contains a non-string value",
                    workspace_manifest_path.display()
                ),
            })?;
            features.insert(feature.to_string());
        }
    }
    let member_features = member_features
        .as_array()
        .ok_or_else(|| OvenLegacyCargoError::InvalidInput {
            field: "generated Loaf Cargo.toml",
            message: format!(
                "{} dependency `{alias}` has a non-array features workspace modifier",
                member_manifest_path.display()
            ),
        })?;
    for feature in member_features {
        let feature = feature.as_str().ok_or_else(|| OvenLegacyCargoError::InvalidInput {
            field: "generated Loaf Cargo.toml",
            message: format!(
                "{} dependency `{alias}` features contains a non-string value",
                member_manifest_path.display()
            ),
        })?;
        features.insert(feature.to_string());
    }
    effective.insert(
        "features".to_string(),
        toml::Value::Array(features.into_iter().map(toml::Value::String).collect()),
    );
    Ok(())
}

/// Resolve one generated-manifest dependency to an exact package identity already admitted by compiler authority.
fn locked_manifest_dependency(
    manifest_root: &Path,
    alias: &str,
    specification: &toml::Value,
    lock: &mut toml::Value,
    visiting: &mut BTreeSet<PathBuf>,
    allow_project_registry_packages: bool,
) -> Result<Option<String>, OvenLegacyCargoError> {
    if let Some(table) = specification.as_table()
        && let Some(path) = table.get("path").and_then(toml::Value::as_str)
    {
        let dependency_manifest = manifest_root.join(path).join("Cargo.toml");
        let dependency_text = regular_file_bytes(&dependency_manifest)?;
        let dependency_manifest_value =
            toml::from_str::<toml::Value>(std::str::from_utf8(&dependency_text).map_err(|error| {
                OvenLegacyCargoError::InvalidInput {
                    field: "generated Loaf path dependency Cargo.toml",
                    message: error.to_string(),
                }
            })?)
            .map_err(|error| OvenLegacyCargoError::InvalidInput {
                field: "generated Loaf path dependency Cargo.toml",
                message: error.to_string(),
            })?;
        let package = locked_local_package(
            &dependency_manifest,
            &dependency_manifest_value,
            lock,
            visiting,
            false,
            allow_project_registry_packages,
        )?;
        let declared_package = table.get("package").and_then(toml::Value::as_str).unwrap_or(alias);
        if declared_package != package.name {
            return Err(OvenLegacyCargoError::Plan(format!(
                "generated Loaf dependency `{alias}` declares package `{declared_package}` but {} names `{}`",
                dependency_manifest.display(),
                package.name
            )));
        }
        if !lock_contains_package(lock, &package.name, &package.version, None) {
            let packages = lock
                .get_mut("package")
                .and_then(toml::Value::as_array_mut)
                .ok_or_else(|| OvenLegacyCargoError::InvalidInput {
                    field: "compiler Cargo.lock",
                    message: "must contain a package array".to_string(),
                })?;
            packages.push(package.value);
        }
        let _ = lock_package_reference(lock, &package.name, &package.version, None)?;
        return Ok(Some(format!("{} {}", package.name, package.version)));
    }
    let package = specification
        .as_table()
        .and_then(|table| table.get("package"))
        .and_then(toml::Value::as_str)
        .unwrap_or(alias);
    let requirement = specification
        .as_str()
        .or_else(|| {
            specification
                .as_table()
                .and_then(|table| table.get("version"))
                .and_then(toml::Value::as_str)
        })
        .unwrap_or("*");
    let requirement = semver::VersionReq::parse(requirement).map_err(|error| OvenLegacyCargoError::InvalidInput {
        field: "generated Loaf Cargo.toml dependency version",
        message: format!("`{alias}` has invalid requirement: {error}"),
    })?;
    let candidates = lock_package_identities(lock)
        .into_iter()
        .filter(|(name, version, source)| {
            name == package
                && source.as_deref().is_some_and(|source| source.starts_with("registry+"))
                && semver::Version::parse(version).is_ok_and(|version| requirement.matches(&version))
        })
        .collect::<Vec<_>>();
    if candidates.is_empty() && allow_project_registry_packages {
        return Ok(None);
    }
    let [(name, version, source)] = candidates.as_slice() else {
        return Err(OvenLegacyCargoError::Plan(format!(
            "generated Loaf dependency `{alias}` ({package} {requirement}) must select exactly one package from the compiler lock, found {}",
            candidates.len()
        )));
    };
    lock_package_reference(lock, name, version, source.as_deref()).map(Some)
}

/// Decode package identities from one Cargo lock document.
fn lock_package_identities(lock: &toml::Value) -> Vec<(String, String, Option<String>)> {
    lock.get("package")
        .and_then(toml::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|package| {
            Some((
                package.get("name")?.as_str()?.to_string(),
                package.get("version")?.as_str()?.to_string(),
                package.get("source").and_then(toml::Value::as_str).map(str::to_string),
            ))
        })
        .collect()
}

/// Return whether one exact package identity is already present in a staged lock.
fn lock_contains_package(lock: &toml::Value, name: &str, version: &str, source: Option<&str>) -> bool {
    lock_package_identities(lock)
        .iter()
        .any(|(candidate_name, candidate_version, candidate_source)| {
            candidate_name == name && candidate_version == version && candidate_source.as_deref() == source
        })
}

/// Render Cargo's shortest unambiguous dependency reference for one exact locked package.
fn lock_package_reference(
    lock: &toml::Value,
    name: &str,
    version: &str,
    source: Option<&str>,
) -> Result<String, OvenLegacyCargoError> {
    let identities = lock_package_identities(lock);
    let exact = identities
        .iter()
        .filter(|(candidate_name, candidate_version, candidate_source)| {
            candidate_name == name && candidate_version == version && candidate_source.as_deref() == source
        })
        .count();
    if exact != 1 {
        return Err(OvenLegacyCargoError::Plan(format!(
            "staged Loaf lock must contain exactly one `{name}` {version} package from {source:?}, found {exact}"
        )));
    }
    let same_name = identities
        .iter()
        .filter(|(candidate_name, _, _)| candidate_name == name)
        .count();
    let same_name_version = identities
        .iter()
        .filter(|(candidate_name, candidate_version, _)| candidate_name == name && candidate_version == version)
        .count();
    Ok(if same_name == 1 {
        name.to_string()
    } else if same_name_version == 1 {
        format!("{name} {version}")
    } else if let Some(source) = source {
        format!("{name} {version} ({source})")
    } else {
        format!("{name} {version}")
    })
}

/// Retain only the package graph reachable from one synthetic root without re-resolving any package identity.
///
/// Cargo regards unreachable package records as a lock-file update. A compiler lock therefore cannot be copied
/// wholesale into a smaller private publisher even when every selected version is correct. Following the lock's own
/// exact dependency references produces the minimal accepted closure while preserving its versions and checksums.
fn prune_lock_to_package(
    lock: &mut toml::Value,
    root_name: &str,
    root_version: &str,
    root_source: Option<&str>,
) -> Result<(), OvenLegacyCargoError> {
    let packages = lock
        .get("package")
        .and_then(toml::Value::as_array)
        .ok_or_else(|| OvenLegacyCargoError::InvalidInput {
            field: "staged Loaf Cargo.lock",
            message: "must contain a package array".to_string(),
        })?
        .clone();
    let identities = lock_package_identities(lock);
    if packages.len() != identities.len() {
        return Err(OvenLegacyCargoError::InvalidInput {
            field: "staged Loaf Cargo.lock",
            message: "every package must declare a name and version".to_string(),
        });
    }
    let mut references = BTreeMap::new();
    for (index, (name, version, source)) in identities.iter().enumerate() {
        let reference = lock_package_reference(lock, name, version, source.as_deref())?;
        if references.insert(reference.clone(), index).is_some() {
            return Err(OvenLegacyCargoError::Plan(format!(
                "staged Loaf lock has ambiguous dependency reference `{reference}`"
            )));
        }
        if source.is_none() {
            let versioned_reference = format!("{name} {version}");
            if references
                .insert(versioned_reference.clone(), index)
                .is_some_and(|previous| previous != index)
            {
                return Err(OvenLegacyCargoError::Plan(format!(
                    "staged Loaf lock has ambiguous dependency reference `{versioned_reference}`"
                )));
            }
        }
    }
    let root = identities
        .iter()
        .position(|(name, version, source)| {
            name == root_name && version == root_version && source.as_deref() == root_source
        })
        .ok_or_else(|| {
            OvenLegacyCargoError::Plan(format!(
                "staged Loaf lock has no synthetic root `{root_name}` {root_version}"
            ))
        })?;
    let mut reachable = BTreeSet::new();
    let mut pending = vec![root];
    while let Some(index) = pending.pop() {
        if !reachable.insert(index) {
            continue;
        }
        let package = packages.get(index).ok_or_else(|| {
            OvenLegacyCargoError::Plan(format!(
                "staged Loaf lock package index {index} is outside its package list"
            ))
        })?;
        for dependency in package
            .get("dependencies")
            .and_then(toml::Value::as_array)
            .into_iter()
            .flatten()
        {
            let dependency = dependency.as_str().ok_or_else(|| OvenLegacyCargoError::InvalidInput {
                field: "staged Loaf Cargo.lock dependency",
                message: "must be a string".to_string(),
            })?;
            let dependency_index = references.get(dependency).copied().ok_or_else(|| {
                OvenLegacyCargoError::Plan(format!(
                    "staged Loaf lock dependency `{dependency}` has no exact package record"
                ))
            })?;
            pending.push(dependency_index);
        }
    }
    let retained = packages
        .into_iter()
        .enumerate()
        .filter_map(|(index, package)| reachable.contains(&index).then_some(package))
        .collect();
    lock.as_table_mut()
        .ok_or_else(|| OvenLegacyCargoError::InvalidInput {
            field: "staged Loaf Cargo.lock",
            message: "must be a TOML table".to_string(),
        })?
        .insert("package".to_string(), toml::Value::Array(retained));
    Ok(())
}

/// Run one named Cargo publisher invocation while continuously enforcing its enclosing transient allocation allowance.
#[allow(clippy::too_many_arguments)]
fn run_legacy_cargo_invocation(
    cargo: &Path,
    rustc: &Path,
    cargo_manifest: &Path,
    target: &Path,
    capacity_root: &Path,
    target_triple: &str,
    profile: &str,
    features: &[String],
    transient_limit: u64,
    command_name: &'static str,
    target_selection: &OvenLegacyCargoInvocationTarget,
    unit_graph: bool,
    compact_debug_info: bool,
    distinct_extension_identities: bool,
) -> Result<CargoInvocationOutput, OvenLegacyCargoError> {
    let cargo = canonical_tool_file(cargo, "cargo")?;
    let rustc = canonical_tool_file(rustc, "rustc")?;
    let package_root = cargo_manifest
        .parent()
        .ok_or_else(|| OvenLegacyCargoError::InvalidInput {
            field: "Cargo manifest",
            message: format!("{} has no package directory", cargo_manifest.display()),
        })?;
    let mut command = Command::new(&cargo);
    command
        .current_dir(package_root)
        .arg(command_name)
        .arg("--manifest-path")
        .arg(cargo_manifest)
        .arg("--target")
        .arg(target_triple)
        .arg("--target-dir")
        .arg(target)
        .arg("--message-format=json-render-diagnostics");
    // Existing locks are immutable publisher authority. The explicit project publisher alone may resolve a missing
    // first lock; once Cargo writes it, every remaining publisher action is locked and offline. Compiler-root and
    // synthetic foundation projects always arrive with locks and therefore cannot silently re-resolve their graph.
    if package_root.join("Cargo.lock").is_file() {
        command.arg("--offline");
        command.arg("--locked");
    }
    if !features.is_empty() {
        command.arg("--features").arg(features.join(","));
    }
    match target_selection {
        OvenLegacyCargoInvocationTarget::None => {}
        OvenLegacyCargoInvocationTarget::PackageLibrary => {
            command.arg("--lib");
            if command_name == "test" {
                command.arg("--no-run");
            }
        }
        OvenLegacyCargoInvocationTarget::CompilerCli => {
            command.args(["--bin", "incan"]);
        }
        OvenLegacyCargoInvocationTarget::WorkspaceTests => {
            command.args(["--all", "--no-run"]);
        }
        OvenLegacyCargoInvocationTarget::WorkspacePackageLibrary(package) => {
            command.args(["--package", package, "--lib", "--no-run"]);
        }
        OvenLegacyCargoInvocationTarget::WorkspacePackageBinary { package, target } => {
            command.args(["--package", package, "--bin", target, "--no-run"]);
        }
        OvenLegacyCargoInvocationTarget::WorkspacePackageIntegrationTest { package, target } => {
            command.args(["--package", package, "--test", target, "--no-run"]);
        }
        OvenLegacyCargoInvocationTarget::WorkspacePackageDoctests(package) => {
            command.args(["--package", package, "--doc", "--no-run"]);
        }
    }
    if unit_graph {
        command.args(["-Z", "unstable-options", "--unit-graph"]);
    }
    let _ = cargo_profile_directory(profile)?;
    if profile == "release" {
        command.arg("--release");
    } else if profile == OVEN_COMPILER_TEST_PROFILE {
        command.args(["--profile", OVEN_COMPILER_TEST_PROFILE]);
    }
    fs::create_dir_all(target).map_err(|source| OvenLegacyCargoError::Io {
        path: target.to_path_buf(),
        source,
    })?;
    let capture_stem = format!(".oven-cargo-{}-{}", std::process::id(), command_name);
    let stdout_path = target.join(format!("{capture_stem}.stdout"));
    let stderr_path = target.join(format!("{capture_stem}.stderr"));
    let stdout = File::create(&stdout_path).map_err(|source| OvenLegacyCargoError::Io {
        path: stdout_path.clone(),
        source,
    })?;
    let stderr = File::create(&stderr_path).map_err(|source| OvenLegacyCargoError::Io {
        path: stderr_path.clone(),
        source,
    })?;
    clear_inherited_cargo_environment(&mut command);
    // ---- Deterministic path remapping for reproducible unit bytes ----
    // Cargo's `-<hash>` extra-filename and rustc's StableCrateId summarize declared unit inputs, so the same locked
    // unit compiles to the same identity on every machine — but the strict version hash also reflects absolute
    // source paths (registry checkouts under the Cargo home, the staged package, the target directory). Left
    // unmapped, a release base baked on one machine and an extension baked on another publish the same identity
    // with different bytes, and rustc refuses to load both halves of that split in one crate graph (colliding
    // StableCrateId values). Remapping every machine-variant root to a stable virtual prefix makes identical units
    // byte-identical everywhere, so shared leaves reconcile by digest instead. RUSTFLAGS do not enter the
    // extra-filename hash, so these per-machine flag strings never fork unit identities.
    let cargo_home = std::env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cargo")));
    let mut remap_flags: Vec<String> = Vec::new();
    if let Some(cargo_home) = &cargo_home {
        remap_flags.push(format!(
            "--remap-path-prefix={}=/incan/cargo-home",
            cargo_home.display()
        ));
    }
    remap_flags.push(format!("--remap-path-prefix={}=/incan/package", package_root.display()));
    remap_flags.push(format!("--remap-path-prefix={}=/incan/target", target.display()));
    // Standard-library spans leak through inlined core/alloc generics. A toolchain with the `rust-src` component
    // resolves them to its real sysroot checkout while one without emits the virtual `/rustc/<commit>` form, so the
    // same unit compiles to different bytes depending on which components happen to be installed. Remap the source
    // checkout onto the exact virtual form so every toolchain agrees; on a src-less toolchain the prefix never
    // matches and the flag is inert.
    if let Some(toolchain_root) = rustc.parent().and_then(Path::parent)
        && let Some(commit) = rustc_commit_hash(&rustc)
    {
        remap_flags.push(format!(
            "--remap-path-prefix={}=/rustc/{commit}",
            toolchain_root.join("lib/rustlib/src/rust").display()
        ));
    }
    // ---- Distinct extension crate identities ----
    // rustc refuses to load two crates with one StableCrateId, and byte-identity across machines is unattainable
    // because rustc folds its own install path into the strict version hash. A project extension therefore salts
    // every unit's `-C metadata` set (rustc hashes all values together with Cargo's own entries), giving extension
    // units StableCrateIds distinct from the sealed base's twins: both copies of a shared interior unit may then
    // legally coexist in one crate graph, and rustc selects each dependent's copy by recorded hash. Declared
    // root-linked crates still substitute onto the base copy by package semantics, so trait identities that can
    // cross the boundary keep unifying. Base loaf bakes never salt — theirs are the canonical identities.
    if distinct_extension_identities {
        remap_flags.push("-C".to_string());
        remap_flags.push("metadata=incan-extension".to_string());
    }
    command.env("CARGO_ENCODED_RUSTFLAGS", remap_flags.join("\u{1f}"));
    command
        .env("RUSTC", &rustc)
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr));
    if compact_debug_info && profile == "debug" {
        command.env("CARGO_PROFILE_DEV_DEBUG", "0");
    }
    isolate_process_group(&mut command);
    let mut child = command.spawn().map_err(|source| OvenLegacyCargoError::Io {
        path: cargo.clone(),
        source,
    })?;
    let output = loop {
        match child.try_wait().map_err(|source| OvenLegacyCargoError::Io {
            path: cargo.clone(),
            source,
        })? {
            Some(_) => {
                break child.wait_with_output().map_err(|source| OvenLegacyCargoError::Io {
                    path: cargo.clone(),
                    source,
                })?;
            }
            None => {
                let scan_started = Instant::now();
                let reservation = if capacity_root.exists() {
                    conservative_directory_reservation(capacity_root)?
                } else {
                    0
                };
                if reservation > transient_limit {
                    terminate_process_group(&mut child).map_err(|source| OvenLegacyCargoError::Io {
                        path: cargo.clone(),
                        source,
                    })?;
                    return Err(OvenLegacyCargoError::TransientCapacityExceeded {
                        path: capacity_root.to_path_buf(),
                        observed_physical_bytes: reservation,
                        limit_bytes: transient_limit,
                    });
                }
                thread::sleep(publisher_capacity_probe_delay(scan_started.elapsed()));
            }
        }
    };
    let stdout = fs::read(&stdout_path).map_err(|source| OvenLegacyCargoError::Io {
        path: stdout_path.clone(),
        source,
    })?;
    let stderr = fs::read(&stderr_path).map_err(|source| OvenLegacyCargoError::Io {
        path: stderr_path.clone(),
        source,
    })?;
    let _ = fs::remove_file(&stdout_path);
    let _ = fs::remove_file(&stderr_path);
    if !output.status.success() {
        let stdout = String::from_utf8_lossy(&stdout);
        let stderr = String::from_utf8_lossy(&stderr);
        return Err(OvenLegacyCargoError::CargoFailed {
            output: format!("{stdout}\n{stderr}").trim().to_string(),
        });
    }
    let reservation = conservative_directory_reservation(capacity_root)?;
    if reservation > transient_limit {
        return Err(OvenLegacyCargoError::TransientCapacityExceeded {
            path: capacity_root.to_path_buf(),
            observed_physical_bytes: reservation,
            limit_bytes: transient_limit,
        });
    }
    Ok(CargoInvocationOutput { stdout })
}

type PublisherArtifactClosure = (
    Vec<String>,
    Vec<OvenRustcArtifactExtern>,
    Vec<OvenRustcSupportingArtifact>,
);

/// Derive direct `--extern` inputs and the complete declared dependency search directory closure.
///
/// A cross-target Cargo build has one target dependency directory plus a host dependency directory for procedural
/// macros. Target artifacts satisfy ordinary generated-program root `--extern` arguments; a host dynamic library can
/// satisfy a procedural-macro root when Cargo emitted no target artifact for that direct dependency.
fn artifact_closure(
    staging: &Path,
    target_deps: &Path,
    dependency_directories: &[PathBuf],
    direct_dependencies: &BTreeMap<String, String>,
    permit_absent_declared_dependencies: bool,
) -> Result<PublisherArtifactClosure, OvenLegacyCargoError> {
    let target_deps = canonical_directory(target_deps, "target Cargo dependency output")?;
    let mut target_files = BTreeMap::new();
    let mut host_dynamic_files = BTreeMap::new();
    let mut supporting = BTreeMap::new();
    let mut dependency_search_paths = Vec::new();
    for directory in dependency_directories {
        let directory = canonical_directory(directory, "Cargo dependency output")?;
        let directory_relative = relative_path(staging, &directory)?;
        let mut contained_artifact = false;
        for entry in fs::read_dir(&directory).map_err(|source| OvenLegacyCargoError::Io {
            path: directory.clone(),
            source,
        })? {
            let entry = entry.map_err(|source| OvenLegacyCargoError::Io {
                path: directory.clone(),
                source,
            })?;
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path).map_err(|source| OvenLegacyCargoError::Io {
                path: path.clone(),
                source,
            })?;
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                return Err(OvenLegacyCargoError::InvalidInput {
                    field: "Cargo dependency output",
                    message: format!("{} must contain regular non-symlink files only", path.display()),
                });
            }
            let name =
                path.file_name()
                    .and_then(|name| name.to_str())
                    .ok_or_else(|| OvenLegacyCargoError::InvalidInput {
                        field: "Cargo dependency output",
                        message: format!("{} has a non-UTF-8 file name", path.display()),
                    })?;
            if !is_direct_rustc_artifact(name) {
                continue;
            }
            contained_artifact = true;
            let digest = digest_bytes(&regular_file_bytes(&path)?);
            let relative_path = format!("{directory_relative}/{name}");
            if directory == target_deps {
                target_files.insert(name.to_string(), (relative_path.clone(), digest.clone()));
            } else if is_dynamic_rustc_artifact(name) {
                // Cargo compiles procedural macros for the host, not the cross target. Keep this separate from
                // target artifacts so a host `.rlib` can never be selected for an ordinary generated-program root.
                host_dynamic_files.insert(name.to_string(), (relative_path.clone(), digest.clone()));
            }
            supporting.insert(relative_path, digest);
        }
        if contained_artifact {
            dependency_search_paths.push(directory_relative);
        }
    }
    let mut externs = Vec::new();
    let mut selected = BTreeSet::new();
    for (dependency, package) in direct_dependencies {
        let Some(artifact) = select_direct_artifact(&target_files, package)
            .or_else(|| select_direct_proc_macro_artifact(&host_dynamic_files, package))
        else {
            if permit_absent_declared_dependencies {
                continue;
            }
            return Err(OvenLegacyCargoError::MissingDirectArtifact {
                crate_name: dependency.clone(),
                path: target_deps.clone(),
            });
        };
        selected.insert(artifact.0.clone());
        externs.push(OvenRustcArtifactExtern {
            crate_name: dependency.clone(),
            relative_path: artifact.0,
            digest: artifact.1,
        });
    }
    let supporting_artifacts = supporting
        .into_iter()
        .filter(|(relative_path, _)| !selected.contains(relative_path))
        .map(|(relative_path, digest)| OvenRustcSupportingArtifact { relative_path, digest })
        .collect();
    Ok((dependency_search_paths, externs, supporting_artifacts))
}

/// Derive one direct-rustc closure from the exact compiler artifacts the explicit Cargo invocation reported.
///
/// Cargo's target layout is an implementation detail: recent Cargo versions can place a dependency library below
/// `target/<triple>/<profile>/build/<package>/<identity>/out`, while older versions use `deps/`. The JSON message
/// stream is the stable publisher authority in both cases. Every path remains confined to private staging, must be
/// a regular non-symlink file, and must have Cargo's crate-and-identity-shaped build-output form before it enters a
/// Loaf.
fn artifact_closure_from_reported_paths(
    staging: &Path,
    target_triple: &str,
    profile: &str,
    direct_dependencies: &BTreeMap<String, ResolvedDirectDependency>,
    permit_absent_declared_dependencies: bool,
    outputs: &[CargoInvocationOutput],
) -> Result<PublisherArtifactClosure, OvenLegacyCargoError> {
    let canonical_staging = canonical_directory(staging, "publisher staging")?;
    let mut target_artifacts = Vec::new();
    let mut host_dynamic_artifacts = Vec::new();
    let mut supporting = BTreeMap::new();
    let mut dependency_search_paths = BTreeSet::new();
    let mut searchable_artifacts = BTreeSet::new();

    for output in outputs {
        for line in String::from_utf8_lossy(&output.stdout).lines() {
            let Ok(artifact) = serde_json::from_str::<CargoCompilerArtifact>(line) else {
                continue;
            };
            if artifact.reason != "compiler-artifact" {
                continue;
            }
            for path in artifact.filenames {
                let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
                    return Err(OvenLegacyCargoError::InvalidInput {
                        field: "Cargo-reported publisher artifact",
                        message: format!("{} has a non-UTF-8 file name", path.display()),
                    });
                };
                if !is_direct_rustc_artifact(file_name)
                    || !cargo_reported_direct_artifact(profile, &artifact.target.name, &path)
                {
                    continue;
                }
                let metadata = fs::symlink_metadata(&path).map_err(|source| OvenLegacyCargoError::Io {
                    path: path.clone(),
                    source,
                })?;
                if metadata.file_type().is_symlink() || !metadata.is_file() {
                    return Err(OvenLegacyCargoError::InvalidInput {
                        field: "Cargo-reported publisher artifact",
                        message: format!("{} must be a regular non-symlink file", path.display()),
                    });
                }
                let source_path = verified_regular_file(&path, "Cargo-reported publisher artifact")?;
                if !source_path.starts_with(&canonical_staging) {
                    return Err(OvenLegacyCargoError::InvalidInput {
                        field: "Cargo-reported publisher artifact",
                        message: format!("{} escapes publisher staging", source_path.display()),
                    });
                }
                let target_artifact = compiler_artifact_platform(&source_path, target_triple).is_some();
                if !target_artifact && !is_dynamic_rustc_artifact(file_name) {
                    // A cross-target direct-rustc plan must not accidentally retain a host `.rlib`. Host dynamic
                    // artifacts are the only supported host-side inputs because procedural macros execute there.
                    continue;
                }
                let parent = source_path.parent().ok_or_else(|| OvenLegacyCargoError::InvalidInput {
                    field: "Cargo-reported publisher artifact",
                    message: format!("{} has no parent directory", source_path.display()),
                })?;
                let artifact_relative_path = relative_path(&canonical_staging, &source_path)?;
                // A `deps` directory or Cargo 1.99's identity-owned `out` directory is flat after Oven materializes
                // its recorded files. A profile root is provenance only, never a Rustc dependency search directory.
                if parent
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| matches!(name, "deps" | "out"))
                {
                    dependency_search_paths.insert(relative_path(&canonical_staging, parent)?);
                    searchable_artifacts.insert(artifact_relative_path.clone());
                }
                let digest = digest_bytes(&regular_file_bytes(&source_path)?);
                let retained = PublisherReportedArtifact {
                    package_id: artifact.package_id.clone(),
                    relative_path: artifact_relative_path.clone(),
                    digest: digest.clone(),
                };
                if target_artifact {
                    target_artifacts.push(retained);
                } else {
                    host_dynamic_artifacts.push(retained);
                }
                if let Some(previous) = supporting.insert(artifact_relative_path.clone(), digest.clone())
                    && previous != digest
                {
                    return Err(OvenLegacyCargoError::InvalidInput {
                        field: "Cargo-reported publisher artifact",
                        message: format!("multiple artifacts share path `{artifact_relative_path}`"),
                    });
                }
            }
        }
    }

    let mut externs = Vec::new();
    let mut selected = BTreeSet::new();
    for (dependency, package) in direct_dependencies {
        let artifact =
            match select_reported_direct_artifact(&target_artifacts, package, &["rlib", "dylib", "so", "dll"])? {
                Some(artifact) => Some(artifact),
                None => select_reported_direct_artifact(&host_dynamic_artifacts, package, &["dylib", "so", "dll"])?,
            };
        let Some(artifact) = artifact else {
            if permit_absent_declared_dependencies {
                continue;
            }
            return Err(OvenLegacyCargoError::MissingDirectArtifact {
                crate_name: dependency.clone(),
                path: canonical_staging.clone(),
            });
        };
        if !searchable_artifacts.contains(&artifact.0) {
            return Err(OvenLegacyCargoError::Plan(format!(
                "named publisher selected direct dependency `{dependency}` from {} outside a sealed Rustc search directory",
                artifact.0
            )));
        }
        selected.insert(artifact.0.clone());
        externs.push(OvenRustcArtifactExtern {
            crate_name: dependency.clone(),
            relative_path: artifact.0,
            digest: artifact.1,
        });
    }
    let supporting_artifacts = supporting
        .into_iter()
        .filter(|(relative_path, _)| !selected.contains(relative_path))
        .map(|(relative_path, digest)| OvenRustcSupportingArtifact { relative_path, digest })
        .collect();
    Ok((
        dependency_search_paths.into_iter().collect(),
        externs,
        supporting_artifacts,
    ))
}

/// One direct-rustc artifact reported by Cargo together with the opaque package identity that emitted it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct PublisherReportedArtifact {
    package_id: String,
    relative_path: String,
    digest: String,
}

/// Select the one artifact emitted by Cargo for a particular resolved package instance.
fn select_reported_direct_artifact(
    artifacts: &[PublisherReportedArtifact],
    dependency: &ResolvedDirectDependency,
    extensions: &[&str],
) -> Result<Option<(String, String)>, OvenLegacyCargoError> {
    for extension in extensions {
        let suffix = format!(".{extension}");
        let mut candidates = artifacts
            .iter()
            .filter(|artifact| artifact.package_id == dependency.package_id)
            .filter(|artifact| artifact.relative_path.ends_with(&suffix))
            .map(|artifact| (artifact.relative_path.clone(), artifact.digest.clone()))
            .collect::<Vec<_>>();
        candidates.sort();
        candidates.dedup();
        match candidates.as_slice() {
            [] => continue,
            [artifact] => return Ok(Some(artifact.clone())),
            _ => {
                return Err(OvenLegacyCargoError::Plan(format!(
                    "named publisher resolved direct dependency `{}` to multiple `{extension}` artifacts for Cargo package `{}`: {}",
                    dependency.package,
                    dependency.package_id,
                    candidates
                        .iter()
                        .map(|(path, _)| path.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                )));
            }
        }
    }
    Ok(None)
}

/// Read every exact direct-rustc artifact from the named Cargo publisher's stable JSON records.
fn publisher_output_artifact_paths(
    outputs: &[CargoInvocationOutput],
    profile: &str,
) -> Result<Vec<PathBuf>, OvenLegacyCargoError> {
    let mut paths = BTreeSet::new();
    for output in outputs {
        for line in String::from_utf8_lossy(&output.stdout).lines() {
            let Ok(artifact) = serde_json::from_str::<CargoCompilerArtifact>(line) else {
                continue;
            };
            if artifact.reason != "compiler-artifact" {
                continue;
            }
            for path in artifact.filenames {
                if !path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(is_direct_rustc_artifact)
                    || !cargo_reported_direct_artifact(profile, &artifact.target.name, &path)
                {
                    continue;
                }
                let path = fs::canonicalize(&path).map_err(|source| OvenLegacyCargoError::Io {
                    path: path.clone(),
                    source,
                })?;
                paths.insert(path);
            }
        }
    }
    Ok(paths.into_iter().collect())
}

/// Decode exact registry checksums from the lock consumed by the named publisher.
fn cargo_registry_checksums(
    cargo_lock: &[u8],
) -> Result<BTreeMap<(String, String, String), String>, OvenLegacyCargoError> {
    let lock = toml::from_slice::<CargoChecksumLock>(cargo_lock).map_err(|error| {
        OvenLegacyCargoError::Plan(format!("publisher Cargo.lock is not valid checksum authority: {error}"))
    })?;
    let mut checksums = BTreeMap::new();
    for package in lock.package {
        let Some(registry) = package.source.filter(|source| source.starts_with("registry+")) else {
            continue;
        };
        let checksum = package
            .checksum
            .filter(|checksum| !checksum.trim().is_empty())
            .ok_or_else(|| {
                OvenLegacyCargoError::Plan(format!(
                    "publisher Cargo.lock omits the checksum for registry package `{}` {}",
                    package.name, package.version
                ))
            })?;
        let key = (package.name, package.version, registry);
        if let Some(previous) = checksums.insert(key.clone(), checksum.clone())
            && previous != checksum
        {
            return Err(OvenLegacyCargoError::Plan(format!(
                "publisher Cargo.lock contains conflicting checksums for `{}` {} from `{}`",
                key.0, key.1, key.2
            )));
        }
    }
    Ok(checksums)
}

/// Copy one exact registry package into publisher staging and declare every retained source file.
fn stage_registry_source(
    staging: &Path,
    package: &str,
    version: &str,
    registry: &str,
    checksum: &str,
    source_root: &Path,
    source_artifacts: &mut Vec<OvenRustcSupportingArtifact>,
) -> Result<OvenRustcRegistrySource, OvenLegacyCargoError> {
    let (staged_root, digest) =
        stage_registry_source_directory(staging, package, version, registry, checksum, source_root)?;
    let relative_root = staged_root
        .strip_prefix(staging)
        .map_err(|_| OvenLegacyCargoError::Plan("staged registry source escaped publisher staging".to_string()))?
        .to_string_lossy()
        .replace('\\', "/");
    for file in materialized_files_from_directory(&staged_root, &relative_root, "registry package source")? {
        source_artifacts.push(OvenRustcSupportingArtifact {
            relative_path: file.relative_path,
            digest: digest_bytes(&regular_file_bytes(&file.source_path)?),
        });
    }
    Ok(OvenRustcRegistrySource {
        registry: registry.to_string(),
        checksum: checksum.to_string(),
        relative_root,
        digest,
    })
}

/// Return the exact commit hash reported by `rustc -vV`, used to remap installed `rust-src` checkouts onto the
/// virtual `/rustc/<commit>` prefix a source-less toolchain embeds in standard-library debug spans.
fn rustc_commit_hash(rustc: &Path) -> Option<String> {
    let output = Command::new(rustc).arg("-vV").output().ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8(output.stdout).ok()?;
    stdout
        .lines()
        .find_map(|line| line.strip_prefix("commit-hash: "))
        .map(|hash| hash.trim().to_string())
}

/// Copy one registry package into private baker state without Cargo's mutable package-local target cache.
fn stage_registry_source_directory(
    staging: &Path,
    package: &str,
    version: &str,
    registry: &str,
    checksum: &str,
    source_root: &Path,
) -> Result<(PathBuf, String), OvenLegacyCargoError> {
    let source_root = canonical_directory(source_root, "registry package source")?;
    let identity = digest_bytes(format!("{registry}\0{package}\0{version}\0{checksum}").as_bytes());
    let identity = identity.strip_prefix("sha256:").unwrap_or(&identity);
    let staged_root = staging.join("registry-sources").join(identity);
    if !staged_root.exists() {
        copy_registry_source_tree(&source_root, &staged_root)?;
    }
    let digest = digest_source_tree(&staged_root).map_err(|error| {
        OvenLegacyCargoError::Plan(format!(
            "could not digest staged registry package `{package}` {version}: {error}"
        ))
    })?;
    Ok((staged_root, digest))
}

/// Copy registry package source while excluding mutable output that is not part of the package archive.
fn copy_registry_source_tree(source_root: &Path, destination_root: &Path) -> Result<(), OvenLegacyCargoError> {
    let source_root = canonical_directory(source_root, "registry package source")?;
    fs::create_dir_all(destination_root).map_err(|source| OvenLegacyCargoError::Io {
        path: destination_root.to_path_buf(),
        source,
    })?;
    let mut entries = fs::read_dir(&source_root)
        .map_err(|source| OvenLegacyCargoError::Io {
            path: source_root.clone(),
            source,
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|source| OvenLegacyCargoError::Io {
            path: source_root.clone(),
            source,
        })?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        if entry.file_name() == "target" {
            continue;
        }
        let source = entry.path();
        let destination = destination_root.join(entry.file_name());
        let metadata = fs::symlink_metadata(&source).map_err(|source_error| OvenLegacyCargoError::Io {
            path: source.clone(),
            source: source_error,
        })?;
        if metadata.file_type().is_symlink() {
            return Err(OvenLegacyCargoError::InvalidInput {
                field: "registry package source",
                message: format!("refuses symlinked publisher input {}", source.display()),
            });
        }
        if metadata.is_dir() {
            copy_regular_directory_tree(&source, &destination, "registry package source")?;
        } else if metadata.is_file() {
            fs::copy(&source, &destination).map_err(|source_error| OvenLegacyCargoError::Io {
                path: source,
                source: source_error,
            })?;
        } else {
            return Err(OvenLegacyCargoError::InvalidInput {
                field: "registry package source",
                message: format!("refuses non-regular publisher input {}", source.display()),
            });
        }
    }
    Ok(())
}

/// Build the plan's complete sealed source authority independently from its linkable registry leaves.
///
/// A transitive procedural macro may be required by rust-analyzer's source graph without producing an `.rlib` that
/// normal dependency selection can expose. Typed Loaf envelopes therefore retain the complete checked inspection
/// closure here; the older broad transitional request continues to derive source authority from its compiled leaves.
fn publisher_registry_source_catalog(
    metadata: &CargoMetadata,
    cargo_lock: &[u8],
    staging: &Path,
    inspection_packages: Option<&[OvenLegacyCargoInspectionPackage]>,
    registry_leaves: &[OvenRustcRegistryLeaf],
    complete_resolved_source_catalog: bool,
    platform_applicable_metadata: Option<&CargoMetadata>,
) -> Result<(Vec<OvenRustcRegistrySourcePackage>, Vec<OvenRustcSupportingArtifact>), OvenLegacyCargoError> {
    let mut sources = registry_leaves
        .iter()
        .map(|leaf| OvenRustcRegistrySourcePackage {
            package: leaf.package.clone(),
            version: leaf.version.clone(),
            features: leaf.features.clone(),
            source: leaf.source.clone(),
        })
        .collect::<Vec<_>>();
    let inspection_sources = match inspection_packages {
        Some(inspection_packages) => Some(legacy_cargo_inspection_sources_from_metadata(
            metadata,
            cargo_lock,
            inspection_packages,
            InspectionPackageScope::CompleteResolvedGraph,
            staging,
        )?),
        // The complete-graph catalog seals a single target's build. Prefer the platform-filtered resolve graph so
        // this closure matches the packages Cargo actually selected for that target, rather than every platform's
        // locked closure; a target-inapplicable package (for example a Linux-only transitive dependency while
        // baking for macOS) must not be required to carry source authority it was never built with.
        None if complete_resolved_source_catalog => Some(legacy_cargo_inspection_sources_from_metadata(
            platform_applicable_metadata.unwrap_or(metadata),
            cargo_lock,
            &[],
            InspectionPackageScope::CompleteResolvedGraph,
            staging,
        )?),
        None => None,
    };
    let Some(inspection_sources) = inspection_sources else {
        sources.sort_by(|left, right| (&left.package, &left.version).cmp(&(&right.package, &right.version)));
        return Ok((sources, Vec::new()));
    };
    let mut source_artifacts = Vec::new();
    for source in inspection_sources {
        let relative_root = source
            .source_root
            .strip_prefix(staging)
            .map_err(|_| OvenLegacyCargoError::Plan("staged registry source escaped publisher staging".to_string()))?
            .to_string_lossy()
            .replace('\\', "/");
        for file in
            materialized_files_from_directory(&source.source_root, &relative_root, "registry inspection source")?
        {
            source_artifacts.push(OvenRustcSupportingArtifact {
                relative_path: file.relative_path,
                digest: digest_bytes(&regular_file_bytes(&file.source_path)?),
            });
        }
        sources.push(OvenRustcRegistrySourcePackage {
            package: source.package,
            version: source.version,
            features: source.features,
            source: OvenRustcRegistrySource {
                registry: source.registry,
                checksum: source.checksum,
                relative_root,
                digest: source.source_digest,
            },
        });
    }
    sources.sort_by(|left, right| {
        (
            &left.package,
            &left.version,
            &left.source.registry,
            &left.source.checksum,
        )
            .cmp(&(
                &right.package,
                &right.version,
                &right.source.registry,
                &right.source.checksum,
            ))
    });
    sources.dedup_by(|left, right| {
        left.package == right.package
            && left.version == right.version
            && left.source.registry == right.source.registry
            && left.source.checksum == right.source.checksum
    });
    source_artifacts.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
    source_artifacts.dedup_by(|left, right| left.relative_path == right.relative_path && left.digest == right.digest);
    Ok((sources, source_artifacts))
}

/// Retain exact registry leaves that the named publisher actually compiled into one Loaf.
///
/// Cargo's JSON artifact record is correlated with its publisher-only metadata package record while both are still
/// inside the explicit transition boundary. The resulting catalog names a single checked artifact already retained by
/// the direct-Rustc plan; it is not a registry index and cannot trigger source discovery or Cargo at consumption.
struct PublisherRegistryLeafCatalogRequest<'a> {
    outputs: &'a [CargoInvocationOutput],
    metadata: &'a CargoMetadata,
    cargo_lock: &'a [u8],
    staging: &'a Path,
    intent: &'a OvenBuildIntent,
    rustc_host: &'a str,
    externs: &'a [OvenRustcArtifactExtern],
    supporting_artifacts: &'a [OvenRustcSupportingArtifact],
    inspection_packages: Option<&'a [OvenLegacyCargoInspectionPackage]>,
}

/// Build the immutable registry-leaf catalog from one explicit publisher result.
fn publisher_registry_leaf_catalog(
    request: PublisherRegistryLeafCatalogRequest<'_>,
) -> Result<(Vec<OvenRustcRegistryLeaf>, Vec<OvenRustcSupportingArtifact>), OvenLegacyCargoError> {
    let PublisherRegistryLeafCatalogRequest {
        outputs,
        metadata,
        cargo_lock,
        staging,
        intent,
        rustc_host,
        externs,
        supporting_artifacts,
        inspection_packages,
    } = request;
    let mut retained = BTreeMap::<String, String>::new();
    for artifact in externs {
        retained.insert(artifact.relative_path.clone(), artifact.digest.clone());
    }
    for artifact in supporting_artifacts {
        retained.insert(artifact.relative_path.clone(), artifact.digest.clone());
    }
    let packages = metadata
        .packages
        .iter()
        .map(|package| (package.id.as_str(), package))
        .collect::<BTreeMap<_, _>>();
    let checksums = cargo_registry_checksums(cargo_lock)?;
    let selected_package_ids = inspection_packages
        .map(|packages| inspection_package_closure_ids(metadata, packages, InspectionPackageScope::DirectRoot))
        .transpose()?;
    let mut candidates = Vec::<PendingRegistryLeaf>::new();
    for output in outputs {
        for line in String::from_utf8_lossy(&output.stdout).lines() {
            let Ok(artifact) = serde_json::from_str::<CargoCompilerArtifact>(line) else {
                continue;
            };
            if artifact.reason != "compiler-artifact" {
                continue;
            }
            if selected_package_ids
                .as_ref()
                .is_some_and(|selected| !selected.contains(&artifact.package_id))
            {
                continue;
            }
            let Some(package) = packages.get(artifact.package_id.as_str()) else {
                return Err(OvenLegacyCargoError::Plan(format!(
                    "named Loaf publisher emitted package `{}` absent from its metadata",
                    artifact.package_id
                )));
            };
            let Some(registry) = package
                .source
                .as_deref()
                .filter(|source| source.starts_with("registry+"))
            else {
                continue;
            };
            let mut artifacts = artifact
                .filenames
                .into_iter()
                .filter(|path| path.extension().and_then(|extension| extension.to_str()) == Some("rlib"))
                .filter_map(|path| {
                    let canonical = fs::canonicalize(&path).ok()?;
                    let target_artifact =
                        compiler_artifact_platform(&canonical, &intent.target) == Some(intent.target.clone());
                    // Cargo's host rlibs are required to direct-compile a host proc macro. They are admissible only
                    // when the receipt target is the compiler host: this catalog has no target dimension, so a
                    // cross-target consumer must keep failing closed rather than accidentally selecting host code.
                    (target_artifact || rustc_host == intent.target).then_some((canonical, target_artifact))
                })
                .filter_map(|(path, target_artifact)| {
                    let relative = relative_path(staging, &path).ok()?;
                    retained
                        .get(&relative)
                        .cloned()
                        .map(|digest| (relative, digest, target_artifact))
                })
                .collect::<Vec<_>>();
            artifacts.sort();
            artifacts.dedup();
            let Some((relative_path, digest, target_artifact)) = artifacts.as_slice().first().cloned() else {
                continue;
            };
            if artifacts.len() != 1 {
                return Err(OvenLegacyCargoError::Plan(format!(
                    "named Loaf publisher emitted multiple target rlibs for registry package `{}` {}",
                    package.name, package.version
                )));
            }
            let crate_name = artifact.target.name.replace('-', "_");
            let mut features = artifact.features;
            features.sort();
            features.dedup();
            let checksum = checksums
                .get(&(package.name.clone(), package.version.clone(), registry.to_string()))
                .cloned()
                .ok_or_else(|| {
                    OvenLegacyCargoError::Plan(format!(
                        "publisher lock has no checksum for registry package `{}` {} from `{registry}`",
                        package.name, package.version
                    ))
                })?;
            let source_root = package
                .manifest_path
                .parent()
                .ok_or_else(|| OvenLegacyCargoError::InvalidInput {
                    field: "registry package manifest",
                    message: format!("{} has no package root", package.manifest_path.display()),
                })?
                .to_path_buf();
            let leaf = PendingRegistryLeaf {
                package: package.name.clone(),
                version: package.version.clone(),
                crate_name: crate_name.clone(),
                features,
                artifact: OvenRustcArtifactExtern {
                    crate_name: crate_name.clone(),
                    relative_path,
                    digest,
                },
                registry: registry.to_string(),
                checksum,
                source_root,
                target_artifact,
            };
            candidates.push(leaf);
        }
    }
    let target_keys = candidates
        .iter()
        .filter(|leaf| leaf.target_artifact)
        .map(|leaf| (leaf.package.clone(), leaf.version.clone(), leaf.crate_name.clone()))
        .collect::<BTreeSet<_>>();
    candidates.sort_by(|left, right| {
        (
            left.package.as_str(),
            left.version.as_str(),
            left.crate_name.as_str(),
            !left.target_artifact,
            left.artifact.relative_path.as_str(),
        )
            .cmp(&(
                right.package.as_str(),
                right.version.as_str(),
                right.crate_name.as_str(),
                !right.target_artifact,
                right.artifact.relative_path.as_str(),
            ))
    });
    let mut leaves = BTreeMap::<(String, String, String), PendingRegistryLeaf>::new();
    for leaf in candidates {
        let key = (leaf.package.clone(), leaf.version.clone(), leaf.crate_name.clone());
        if !leaf.target_artifact && target_keys.contains(&key) {
            continue;
        }
        match leaves.get(&key) {
            Some(existing)
                if existing.artifact == leaf.artifact
                    && existing.features == leaf.features
                    && existing.registry == leaf.registry
                    && existing.checksum == leaf.checksum
                    && existing.source_root == leaf.source_root => {}
            Some(existing) => {
                return Err(OvenLegacyCargoError::Plan(format!(
                    "named Loaf publisher emitted conflicting registry leaf `{}` {}: {} and {}",
                    leaf.package, leaf.version, existing.artifact.relative_path, leaf.artifact.relative_path
                )));
            }
            None => {
                leaves.insert(key, leaf);
            }
        }
    }
    let mut source_artifacts = Vec::new();
    let mut sealed = Vec::new();
    for leaf in leaves.into_values() {
        let source = stage_registry_source(
            staging,
            &leaf.package,
            &leaf.version,
            &leaf.registry,
            &leaf.checksum,
            &leaf.source_root,
            &mut source_artifacts,
        )?;
        sealed.push(OvenRustcRegistryLeaf {
            package: leaf.package,
            version: leaf.version,
            crate_name: leaf.crate_name,
            features: leaf.features,
            source,
            artifact: leaf.artifact,
        });
    }
    source_artifacts.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
    source_artifacts.dedup_by(|left, right| left.relative_path == right.relative_path && left.digest == right.digest);
    Ok((sealed, source_artifacts))
}

/// Select one `.rlib` or dynamic-library artifact for a direct dependency from a fresh Cargo target directory.
fn select_direct_artifact(files: &BTreeMap<String, (String, String)>, dependency: &str) -> Option<(String, String)> {
    select_direct_artifact_with_extensions(files, dependency, &["rlib", "dylib", "so", "dll"])
}

/// Select a host-built procedural macro only after no target artifact can satisfy the direct dependency.
fn select_direct_proc_macro_artifact(
    files: &BTreeMap<String, (String, String)>,
    dependency: &str,
) -> Option<(String, String)> {
    select_direct_artifact_with_extensions(files, dependency, &["dylib", "so", "dll"])
}

/// Select one artifact with an allowed direct-rustc extension for a dependency name.
fn select_direct_artifact_with_extensions(
    files: &BTreeMap<String, (String, String)>,
    dependency: &str,
    extensions: &[&str],
) -> Option<(String, String)> {
    // Cargo generally replaces package hyphens with underscores in an artifact crate name, but packages may expose a
    // different library name (for example `md-5` exposes `md5`). Preserve both deterministic normalizations until
    // the publisher gains a Cargo-unit-graph reader of its own.
    let crate_names = [dependency.replace('-', "_"), dependency.replace('-', "")];
    crate_names.into_iter().find_map(|crate_name| {
        let prefix = format!("lib{crate_name}-");
        extensions.iter().find_map(|extension| {
            files
                .keys()
                .find(|name| name.starts_with(&prefix) && name.ends_with(&format!(".{extension}")))
                .and_then(|name| files.get(name))
                .cloned()
        })
    })
}

/// Return whether an artifact filename is a dynamically loaded rustc library.
fn is_dynamic_rustc_artifact(name: &str) -> bool {
    [".dylib", ".so", ".dll"]
        .iter()
        .any(|extension| name.ends_with(extension))
}

/// Retain direct Rustc crate inputs, never Cargo's object files, dep-info files, or a prior executable.
///
/// `.rmeta` sidecars are load-bearing, not redundant: since Rust 1.98 Cargo can emit an `.rlib` that carries only a
/// metadata stub (an archive with a `lib.rmeta-link` member), with the crate's real metadata living solely in the
/// sibling `.rmeta` file that rustc discovers next to the rlib. Dropping the sidecar makes every direct-rustc
/// consumer of such a closure fail with E0463 "can't find crate", so the sealed closure must ship both files.
/// Extern selection still names only `.rlib`/dynamic libraries; the sidecar rides along as a supporting artifact.
fn is_direct_rustc_artifact(name: &str) -> bool {
    [".rlib", ".rmeta", ".dylib", ".so", ".dll", ".a", ".lib"]
        .iter()
        .any(|extension| name.ends_with(extension))
}

/// Identify Cargo's `oven-test/build/<package>/<identity>/out` output layout.
///
/// An arbitrary file below this path is still build-script implementation detail, never a direct-Rustc crate input.
/// Newer Cargo releases can report a real compiler artifact in the same layout, so callers admitting a Cargo JSON
/// record must additionally prove its package, target, and identity-shaped filename in
/// `compiler_suite_cargo_reported_direct_artifact`. The compiler-suite publisher always uses the named `oven-test`
/// profile, so fence the match to that profile rather than treating an unrelated checkout component named `build` as
/// a Cargo implementation detail. Cargo can add an identity directory between the package and `out`, so require the
/// final parent to be `out` instead of assuming a fixed number of path components.
fn cargo_reported_build_output(profile: &str, path: &Path) -> bool {
    let components = path.components().collect::<Vec<_>>();
    let Some((_, parent_components)) = components.split_last() else {
        return false;
    };
    parent_components
        .last()
        .is_some_and(|component| component.as_os_str() == "out")
        && parent_components
            .windows(2)
            .any(|components| components[0].as_os_str() == profile && components[1].as_os_str() == "build")
}

/// Identify compiler-suite `oven-test/build/<package>/<identity>/out` output without duplicating the generic
/// publisher predicate used by ordinary project Loafs.
fn compiler_suite_cargo_build_output(path: &Path) -> bool {
    cargo_reported_build_output(OVEN_COMPILER_TEST_PROFILE, path)
}

/// Accept one direct-Rustc compiler artifact Cargo explicitly reported from its newer build-output layout.
///
/// Cargo 1.99 writes normal dependency libraries as
/// `oven-test/build/<package>/<identity>/out/lib<target>-<identity>.<linker-extension>`. A build script may also
/// write files below `out`, but it cannot pass this target-and-identity-shaped filename test unless Cargo has
/// presented it as that target's compiler artifact. The package directory need not match the library target (for
/// example, package `coreaudio-rs` emits target `coreaudio`). Directory scans keep rejecting all `build` output;
/// this predicate is used only for the JSON-recorded file list from the named publisher.
fn cargo_reported_direct_artifact(profile: &str, target_name: &str, path: &Path) -> bool {
    if !cargo_reported_build_output(profile, path) {
        return true;
    }
    let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    let Some(parent) = path.parent() else {
        return false;
    };
    let parent_components = parent.components().collect::<Vec<_>>();
    let Some(out_index) = parent_components
        .iter()
        .rposition(|component| component.as_os_str() == "out")
    else {
        return false;
    };
    if out_index + 1 != parent_components.len() || out_index < 3 {
        return false;
    }
    let identity = parent_components[out_index - 1].as_os_str().to_str();
    let Some(identity) = identity else {
        return false;
    };
    if identity.is_empty() {
        return false;
    }
    [".rlib", ".dylib", ".so", ".dll", ".a", ".lib"]
        .iter()
        .any(|extension| file_name == format!("lib{}-{identity}{extension}", target_name.replace('-', "_")))
}

/// Keep compiler-suite callers on the named profile while sharing the exact build-output proof with project Loafs.
fn compiler_suite_cargo_reported_direct_artifact(target_name: &str, path: &Path) -> bool {
    cargo_reported_direct_artifact(OVEN_COMPILER_TEST_PROFILE, target_name, path)
}

/// Read direct dependency aliases from the generated root `Cargo.toml` without asking Cargo for metadata.
fn cargo_direct_dependency_names(
    cargo_manifest: &[u8],
    include_dev_dependencies: bool,
) -> Result<BTreeMap<String, String>, OvenLegacyCargoError> {
    let content = std::str::from_utf8(cargo_manifest).map_err(|error| OvenLegacyCargoError::InvalidInput {
        field: "Cargo.toml",
        message: format!("must be UTF-8: {error}"),
    })?;
    let document = toml::from_str::<toml::Value>(content).map_err(|error| OvenLegacyCargoError::InvalidInput {
        field: "Cargo.toml",
        message: format!("must be valid TOML: {error}"),
    })?;
    let mut dependencies = direct_dependency_aliases(&document, "dependencies");
    if include_dev_dependencies {
        dependencies.extend(direct_dependency_aliases(&document, "dev-dependencies"));
    }
    Ok(dependencies)
}

/// Return Cargo dependency aliases mapped to the package artifact name Cargo emits into `target/.../deps`.
fn direct_dependency_aliases(document: &toml::Value, section: &str) -> BTreeMap<String, String> {
    document
        .get(section)
        .and_then(toml::Value::as_table)
        .map(|dependencies| {
            dependencies
                .iter()
                .map(|(alias, specification)| {
                    let package = specification
                        .as_table()
                        .and_then(|table| table.get("package"))
                        .and_then(toml::Value::as_str)
                        .unwrap_or(alias)
                        .to_string();
                    // Cargo package keys may contain hyphens, while both Rust paths and `rustc --extern` use the
                    // corresponding underscore identifier (`wasmtime-wasi` -> `wasmtime_wasi`).
                    (alias.replace('-', "_"), package)
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Keep generated-code roots and documented compiler-owned macro-expansion roots for direct rustc.
///
/// Cargo compiles src/main.rs or src/lib.rs together with every nested generated module. Restricting this scan to
/// the root silently omits dependencies used only by a child module, such as the compiler-owned incan_derive
/// procedural macro emitted for a model declaration. The explicit publisher therefore reads every regular .rs file
/// below the generated src tree before it freezes the immutable plan. A proc macro can also create root paths after
/// that scan, so documented compiler-owned macro expansion roots are retained from the same declared manifest.
/// Normal Oven consumers still inspect neither this generated Cargo project nor any Cargo target directory.
fn generated_project_direct_dependencies(
    generated_project: &Path,
    declared_dependencies: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, String>, OvenLegacyCargoError> {
    let source_root = canonical_directory(&generated_project.join("src"), "generated Rust source tree")?;
    let mut pending = vec![source_root];
    let mut generated_sources = String::new();
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(&directory).map_err(|source| OvenLegacyCargoError::Io {
            path: directory.clone(),
            source,
        })? {
            let entry = entry.map_err(|source| OvenLegacyCargoError::Io {
                path: directory.clone(),
                source,
            })?;
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path).map_err(|source| OvenLegacyCargoError::Io {
                path: path.clone(),
                source,
            })?;
            if metadata.file_type().is_symlink() {
                return Err(OvenLegacyCargoError::InvalidInput {
                    field: "generated Rust source tree",
                    message: format!("{} must not contain symlinks", path.display()),
                });
            }
            if metadata.is_dir() {
                pending.push(path);
                continue;
            }
            if !metadata.is_file() {
                return Err(OvenLegacyCargoError::InvalidInput {
                    field: "generated Rust source tree",
                    message: format!("{} must contain only regular files and directories", path.display()),
                });
            }
            if path.extension().and_then(|extension| extension.to_str()) == Some("rs") {
                let source = regular_file_bytes(&path)?;
                let source = std::str::from_utf8(&source).map_err(|error| OvenLegacyCargoError::InvalidInput {
                    field: "generated Rust source",
                    message: format!("{} must be UTF-8: {error}", path.display()),
                })?;
                generated_sources.push_str(source);
                generated_sources.push('\n');
            }
        }
    }
    let mut selected = declared_dependencies
        .iter()
        .filter(|(dependency, _)| {
            let crate_name = dependency.replace('-', "_");
            generated_sources.contains(&format!("{crate_name}::"))
        })
        .map(|(alias, package)| (alias.clone(), package.clone()))
        .collect::<BTreeMap<_, _>>();
    for (marker, expansion_roots) in GENERATED_PROC_MACRO_EXPANSION_ROOTS {
        if !generated_sources.contains(marker) {
            continue;
        }
        for crate_name in *expansion_roots {
            let package = declared_dependencies.get(*crate_name).ok_or_else(|| {
                OvenLegacyCargoError::Plan(format!(
                    "generated macro {marker} requires declared direct dependency {crate_name}"
                ))
            })?;
            selected.insert((*crate_name).to_string(), package.clone());
        }
    }
    Ok(selected)
}

/// Choose the bounded manifest dependencies that become named `rustc --extern` inputs.
///
/// A regular generated root needs only source-reachable crates. Library tests and complete standard-library Loafs
/// compile source trees beyond their small root, so their checked manifest is the authoritative complete direct
/// dependency surface. Cargo may omit a conditionally declared package; the artifact reader preserves that
/// compatibility behavior by tolerating the corresponding absent artifact where appropriate.
fn publisher_direct_dependencies(
    generated_project: &Path,
    declared_dependencies: BTreeMap<String, String>,
    publication_kind: OvenLegacyCargoPublicationKind,
    closure: OvenLegacyCargoDirectDependencyClosure,
) -> Result<BTreeMap<String, String>, OvenLegacyCargoError> {
    if publication_kind == OvenLegacyCargoPublicationKind::LibraryTests
        || closure == OvenLegacyCargoDirectDependencyClosure::CheckedDeclared
    {
        return Ok(declared_dependencies);
    }
    generated_project_direct_dependencies(generated_project, &declared_dependencies)
}

/// Compiler-owned procedural macros whose generated Rust introduces root crate paths absent from pre-expansion source.
///
/// This is deliberately a finite publisher contract, rather than inferring arbitrary third-party macro behavior or
/// broadening a native plan to every Cargo dependency. Each root must already be a declared generated-project direct
/// dependency, then passes the same sealed artifact/digest path as ordinary direct externs.
const GENERATED_PROC_MACRO_EXPANSION_ROOTS: &[(&str, &[&str])] =
    &[("#[incan_web_macros::route", &["inventory", "axum"])];

/// Write portable publisher provenance inside the temporary closure without retaining local paths.
fn write_provenance(path: &Path, provenance: &OvenLegacyCargoProvenance) -> Result<(), OvenLegacyCargoError> {
    let parent = path.parent().ok_or_else(|| OvenLegacyCargoError::InvalidInput {
        field: "provenance path",
        message: "must have a parent directory".to_string(),
    })?;
    fs::create_dir_all(parent).map_err(|source| OvenLegacyCargoError::Io {
        path: parent.to_path_buf(),
        source,
    })?;
    let payload =
        serde_json::to_vec_pretty(provenance).map_err(|error| OvenLegacyCargoError::Plan(error.to_string()))?;
    fs::write(path, payload).map_err(|source| OvenLegacyCargoError::Io {
        path: path.to_path_buf(),
        source,
    })
}

/// Create and lock the publisher staging root before reclaiming stale successful-or-interrupted staging directories.
fn acquire_publisher_lock(parent: &Path) -> Result<PublisherLock, OvenLegacyCargoError> {
    fs::create_dir_all(parent).map_err(|source| OvenLegacyCargoError::Io {
        path: parent.to_path_buf(),
        source,
    })?;
    let path = parent.join(".publisher.lock");
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&path)
        .map_err(|source| OvenLegacyCargoError::Io { path, source })?;
    file.lock().map_err(|source| OvenLegacyCargoError::Io {
        path: parent.join(".publisher.lock"),
        source,
    })?;
    Ok(PublisherLock { file })
}

/// Reclaim only publisher-owned stale directories while the publisher lock is held.
fn reclaim_stale_publisher_staging(parent: &Path) -> Result<(), OvenLegacyCargoError> {
    for entry in fs::read_dir(parent).map_err(|source| OvenLegacyCargoError::Io {
        path: parent.to_path_buf(),
        source,
    })? {
        let entry = entry.map_err(|source| OvenLegacyCargoError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
        let name = entry.file_name();
        if !name.to_string_lossy().starts_with(".legacy-cargo-") {
            continue;
        }
        let path = entry.path();
        if path.is_dir() {
            fs::remove_dir_all(&path).map_err(|source| OvenLegacyCargoError::Io { path, source })?;
        }
    }
    Ok(())
}

/// Allocate a collision-resistant publisher-owned staging root below the Oven store.
fn create_publisher_staging(parent: &Path) -> Result<PathBuf, OvenLegacyCargoError> {
    let parent = fs::canonicalize(parent).map_err(|source| OvenLegacyCargoError::Io {
        path: parent.to_path_buf(),
        source,
    })?;
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| OvenLegacyCargoError::InvalidInput {
            field: "system clock",
            message: error.to_string(),
        })?
        .as_nanos();
    for sequence in 0_u32..128 {
        let path = parent.join(format!(".legacy-cargo-{}-{timestamp}-{sequence}", std::process::id()));
        match fs::create_dir(&path) {
            Ok(()) => return Ok(path),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(source) => return Err(OvenLegacyCargoError::Io { path, source }),
        }
    }
    Err(OvenLegacyCargoError::InvalidInput {
        field: "publisher staging",
        message: "could not allocate a unique staging directory".to_string(),
    })
}

/// Read a non-symlink regular file in the publisher closure.
fn regular_file_bytes(path: &Path) -> Result<Vec<u8>, OvenLegacyCargoError> {
    let path = verified_regular_file(path, "publisher input")?;
    fs::read(&path).map_err(|source| OvenLegacyCargoError::Io { path, source })
}

/// Locate the generated Cargo entrypoint authorized by the receipt without assuming a binary target.
///
/// Normal Oven build/run uses `src/main.rs`; the native test harness is a library target at `src/lib.rs`. Both are
/// compatible inputs for the explicit publisher, but exactly one must match the receipt-bound generated-root digest.
fn receipt_authorized_generated_root_bytes(
    generated_project: &Path,
    receipt: &OvenReceipt,
    source_evidence_key: &str,
) -> Result<Vec<u8>, OvenLegacyCargoError> {
    let expected = receipt
        .sources
        .supplemental_digests
        .get(source_evidence_key)
        .ok_or_else(|| OvenLegacyCargoError::ReceiptMismatch {
            message: format!("receipt does not declare {source_evidence_key} source evidence"),
        })?;
    let mut matching = Vec::new();
    for relative in ["src/main.rs", "src/lib.rs"] {
        let candidate = generated_project.join(relative);
        if !candidate.exists() {
            continue;
        }
        let bytes = regular_file_bytes(&candidate)?;
        let actual = digest_bytes(&bytes);
        if &actual == expected {
            matching.push((candidate, bytes));
        }
    }
    match matching.len() {
        1 => Ok(matching.remove(0).1),
        0 => Err(OvenLegacyCargoError::ReceiptMismatch {
            message: format!(
                "none of src/main.rs or src/lib.rs matches {source_evidence_key} receipt digest {expected}"
            ),
        }),
        _ => Err(OvenLegacyCargoError::ReceiptMismatch {
            message: "multiple generated roots match the receipt; publisher refuses an ambiguous target".to_string(),
        }),
    }
}

/// Validate one non-symlink regular file and return its canonical path for subsequent containment checks.
///
/// The non-symlink check happens before canonicalization, so a caller cannot smuggle a link through the publisher
/// boundary. Returning the canonical form keeps path containment sound on platforms that expose the same temporary
/// directory through aliases such as macOS `/var` and `/private/var`.
fn verified_regular_file(path: &Path, field: &'static str) -> Result<PathBuf, OvenLegacyCargoError> {
    let metadata = fs::symlink_metadata(path).map_err(|source| OvenLegacyCargoError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(OvenLegacyCargoError::InvalidInput {
            field,
            message: format!("{} must be a regular non-symlink file", path.display()),
        });
    }
    fs::canonicalize(path).map_err(|source| OvenLegacyCargoError::Io {
        path: path.to_path_buf(),
        source,
    })
}

/// Validate one non-symlink directory and canonicalize it for publisher input isolation.
fn canonical_directory(path: &Path, field: &'static str) -> Result<PathBuf, OvenLegacyCargoError> {
    let metadata = fs::symlink_metadata(path).map_err(|source| OvenLegacyCargoError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(OvenLegacyCargoError::InvalidInput {
            field,
            message: format!("{} must be a non-symlink directory", path.display()),
        });
    }
    fs::canonicalize(path).map_err(|source| OvenLegacyCargoError::Io {
        path: path.to_path_buf(),
        source,
    })
}

/// Read the named tool's stable version report without relying on a shell or Cargo metadata.
fn tool_version(path: &Path, field: &'static str) -> Result<String, OvenLegacyCargoError> {
    let path = canonical_tool_file(path, field)?;
    let output = Command::new(&path)
        .arg("--version")
        .output()
        .map_err(|source| OvenLegacyCargoError::Io {
            path: path.clone(),
            source,
        })?;
    if !output.status.success() {
        return Err(OvenLegacyCargoError::InvalidInput {
            field,
            message: "must report a successful --version identity".to_string(),
        });
    }
    let version = String::from_utf8(output.stdout).map_err(|error| OvenLegacyCargoError::InvalidInput {
        field,
        message: format!("reported non-UTF-8 --version output: {error}"),
    })?;
    let version = version.trim();
    if version.is_empty() {
        return Err(OvenLegacyCargoError::InvalidInput {
            field,
            message: "reported an empty --version identity".to_string(),
        });
    }
    Ok(version.to_string())
}

/// Validate a conventional toolchain-manager entrypoint while preserving its original executable name.
///
/// `cargo` is commonly a `rustup-init` symlink. Canonicalizing and executing its terminal file changes `argv[0]` to
/// `rustup-init`, so that shim no longer dispatches Cargo subcommands. Verify the terminal regular file, but return
/// the absolute caller entrypoint so `Command` retains the approved `cargo` or `rustc` invocation identity.
fn canonical_tool_file(path: &Path, field: &'static str) -> Result<PathBuf, OvenLegacyCargoError> {
    let invocation = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|source| OvenLegacyCargoError::Io {
                path: path.to_path_buf(),
                source,
            })?
            .join(path)
    };
    let canonical = fs::canonicalize(path).map_err(|source| OvenLegacyCargoError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let _ = verified_regular_file(&canonical, field)?;
    Ok(invocation)
}

/// Return a safe path below `root` with forward slashes for an Oven manifest.
fn relative_path(root: &Path, path: &Path) -> Result<String, OvenLegacyCargoError> {
    let root = canonical_directory(root, "artifact root")?;
    let path = fs::canonicalize(path).map_err(|source| OvenLegacyCargoError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let relative = path
        .strip_prefix(&root)
        .map_err(|_| OvenLegacyCargoError::InvalidInput {
            field: "artifact path",
            message: format!("{} is not below {}", path.display(), root.display()),
        })?;
    let value = relative.to_string_lossy().replace('\\', "/");
    if value.is_empty() || value.starts_with('/') || value.contains("../") {
        return Err(OvenLegacyCargoError::InvalidInput {
            field: "artifact path",
            message: format!("{} is not a portable relative artifact path", relative.display()),
        });
    }
    Ok(value)
}

/// Copy one shard's verified closure out of a disposable Cargo target before the next selection starts.
///
/// The publisher must not keep every selection target alive until `OvenStore::publish_batch` can receive the final
/// suite. Each selection instead gets a private prepared directory below the publisher staging root. The returned
/// paths preserve the immutable entry-relative names used by the shard payload, while their sources now point at the
/// publisher-owned prepared copy. That lets the caller reclaim the closed Cargo target without turning the prepared
/// suite into a Cargo cache.
#[cfg(test)]
fn stage_compiler_suite_shard_files(
    staging: &Path,
    shard_index: usize,
    materialized_files: &[OvenArtifactMaterializedFile],
) -> Result<Vec<OvenArtifactMaterializedFile>, OvenLegacyCargoError> {
    stage_compiler_suite_files_at(
        &staging.join("prepared-shards").join(format!("{shard_index:04}")),
        materialized_files,
    )
}

/// Link a verified direct-rustc closure into one newly-created publisher-owned prepared directory.
///
/// The prepared directory and disposable selection target are siblings below the same Oven-owned store root. A hard
/// link therefore survives reclamation of the Cargo target without allocating a second physical copy, while the
/// final bounded store publisher still verifies and owns its immutable files. This is not a Cargo cache: the target
/// link is removed immediately after staging and normal commands never receive either path.
#[cfg(test)]
fn stage_compiler_suite_files_at(
    prepared_root: &Path,
    materialized_files: &[OvenArtifactMaterializedFile],
) -> Result<Vec<OvenArtifactMaterializedFile>, OvenLegacyCargoError> {
    let prepared_parent = prepared_root
        .parent()
        .ok_or_else(|| OvenLegacyCargoError::InvalidInput {
            field: "compiler-suite prepared artifact",
            message: format!("{} has no parent staging directory", prepared_root.display()),
        })?;
    fs::create_dir_all(prepared_parent).map_err(|source| OvenLegacyCargoError::Io {
        path: prepared_parent.to_path_buf(),
        source,
    })?;
    fs::create_dir(prepared_root).map_err(|source| OvenLegacyCargoError::Io {
        path: prepared_root.to_path_buf(),
        source,
    })?;

    let staged = (|| {
        let mut staged = Vec::with_capacity(materialized_files.len());
        let mut relative_paths = BTreeSet::new();
        for materialized in materialized_files {
            let source = verified_regular_file(&materialized.source_path, "compiler-suite shard artifact")?;
            let relative = compiler_suite_shard_relative_path(&materialized.relative_path)?;
            if !relative_paths.insert(materialized.relative_path.clone()) {
                return Err(OvenLegacyCargoError::InvalidInput {
                    field: "compiler-suite shard artifact",
                    message: format!(
                        "declares duplicate prepared artifact path {}",
                        materialized.relative_path
                    ),
                });
            }
            let destination = prepared_root.join(&relative);
            let destination_parent = destination.parent().ok_or_else(|| OvenLegacyCargoError::InvalidInput {
                field: "compiler-suite shard artifact",
                message: format!("cannot derive parent for prepared artifact {}", destination.display()),
            })?;
            fs::create_dir_all(destination_parent).map_err(|source_error| OvenLegacyCargoError::Io {
                path: destination_parent.to_path_buf(),
                source: source_error,
            })?;
            match fs::symlink_metadata(&destination) {
                Ok(_) => {
                    return Err(OvenLegacyCargoError::InvalidInput {
                        field: "compiler-suite shard artifact",
                        message: format!("prepared artifact path already exists: {}", destination.display()),
                    });
                }
                Err(source_error) if source_error.kind() == io::ErrorKind::NotFound => {}
                Err(source_error) => {
                    return Err(OvenLegacyCargoError::Io {
                        path: destination,
                        source: source_error,
                    });
                }
            }
            fs::hard_link(&source, &destination).map_err(|source_error| OvenLegacyCargoError::Io {
                path: destination.clone(),
                source: source_error,
            })?;
            staged.push(OvenArtifactMaterializedFile {
                source_path: destination,
                relative_path: materialized.relative_path.clone(),
            });
        }
        Ok(staged)
    })();
    if staged.is_err() {
        let _ = fs::remove_dir_all(prepared_root);
    }
    staged
}

/// Validate the portable name that will be recreated below a publisher-owned prepared-shard directory.
#[cfg(test)]
fn compiler_suite_shard_relative_path(value: &str) -> Result<PathBuf, OvenLegacyCargoError> {
    let path = Path::new(value);
    if value.trim().is_empty()
        || path
            .components()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        return Err(OvenLegacyCargoError::InvalidInput {
            field: "compiler-suite shard artifact path",
            message: "must be a non-empty normalized relative path".to_string(),
        });
    }
    Ok(path.to_path_buf())
}

/// Drop Cargo-only files from the publisher's private target after the direct-rustc closure is fully declared.
///
/// The target directory is created beneath `legacy-cargo-staging` by this publisher and is no longer observed by
/// Cargo at this point. Keeping only source paths passed to `OvenStore::publish` prevents temporary object files
/// from consuming a second compatibility-domain allocation while the immutable store entry is copied and verified.
fn reclaim_unmaterialized_compiler_suite_target_files(
    target: &Path,
    materialized_files: &[OvenArtifactMaterializedFile],
) -> Result<(), OvenLegacyCargoError> {
    let target = canonical_directory(target, "compiler-suite transient target")?;
    let mut retained = BTreeSet::new();
    for materialized in materialized_files {
        let source = verified_regular_file(&materialized.source_path, "compiler-suite retained artifact")?;
        if source.starts_with(&target) {
            retained.insert(source);
        }
    }
    reclaim_unmaterialized_compiler_suite_target_directory(&target, &target, &retained)
}

/// Recursively reclaim a closed publisher target without following links or crossing its staging boundary.
fn reclaim_unmaterialized_compiler_suite_target_directory(
    target: &Path,
    directory: &Path,
    retained: &BTreeSet<PathBuf>,
) -> Result<(), OvenLegacyCargoError> {
    let mut entries = fs::read_dir(directory)
        .map_err(|source| OvenLegacyCargoError::Io {
            path: directory.to_path_buf(),
            source,
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|source| OvenLegacyCargoError::Io {
            path: directory.to_path_buf(),
            source,
        })?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path).map_err(|source| OvenLegacyCargoError::Io {
            path: path.clone(),
            source,
        })?;
        if metadata.file_type().is_symlink() {
            // This removes the link itself; it never follows a Cargo-produced link outside publisher staging.
            fs::remove_file(&path).map_err(|source| OvenLegacyCargoError::Io { path, source })?;
            continue;
        }
        if metadata.is_dir() {
            reclaim_unmaterialized_compiler_suite_target_directory(target, &path, retained)?;
            let mut remaining = fs::read_dir(&path).map_err(|source| OvenLegacyCargoError::Io {
                path: path.clone(),
                source,
            })?;
            match remaining.next() {
                None => {
                    fs::remove_dir(&path).map_err(|source| OvenLegacyCargoError::Io {
                        path: path.clone(),
                        source,
                    })?;
                }
                Some(Ok(_)) => {}
                Some(Err(source)) => {
                    return Err(OvenLegacyCargoError::Io { path, source });
                }
            }
            continue;
        }
        if !metadata.is_file() {
            return Err(OvenLegacyCargoError::InvalidInput {
                field: "compiler-suite transient target",
                message: format!("{} is neither a regular file nor directory", path.display()),
            });
        }
        let canonical = fs::canonicalize(&path).map_err(|source| OvenLegacyCargoError::Io {
            path: path.clone(),
            source,
        })?;
        if !canonical.starts_with(target) {
            return Err(OvenLegacyCargoError::InvalidInput {
                field: "compiler-suite transient target",
                message: format!("{} escapes publisher staging", canonical.display()),
            });
        }
        if !retained.contains(&canonical) {
            fs::remove_file(&path).map_err(|source| OvenLegacyCargoError::Io { path, source })?;
        }
    }
    Ok(())
}

/// Measure a conservative physical reservation for transient staging without following links or relying on logical
/// bytes as physical allocation.
///
/// Cargo can create short-lived symlinks in its target directory on platforms that expose dynamic libraries. They
/// count as one allocation apiece here, but their targets are never followed. Later artifact admission still rejects
/// every symlink, so no link can become a retained Oven input.
#[cfg(unix)]
pub(crate) fn conservative_directory_reservation(root: &Path) -> Result<u64, OvenLegacyCargoError> {
    let mut seen_files = BTreeSet::new();
    conservative_directory_reservation_with_seen_files(root, &mut seen_files)
}

/// Treat hard-linked publisher inputs as one physical allocation while still rejecting links that could escape the
/// explicit publisher root. Prepared shard roots use this after their disposable selection target has been reclaimed.
#[cfg(unix)]
fn conservative_directory_reservation_with_seen_files(
    root: &Path,
    seen_files: &mut BTreeSet<(u64, u64)>,
) -> Result<u64, OvenLegacyCargoError> {
    use std::os::unix::fs::MetadataExt;

    let metadata = fs::symlink_metadata(root).map_err(|source| OvenLegacyCargoError::Io {
        path: root.to_path_buf(),
        source,
    })?;
    if metadata.file_type().is_symlink() {
        return Ok(round_physical(metadata.len()));
    }
    if metadata.is_file() {
        let identity = (metadata.dev(), metadata.ino());
        return Ok(if seen_files.insert(identity) {
            round_physical(metadata.len())
        } else {
            0
        });
    }
    if !metadata.is_dir() {
        return Err(OvenLegacyCargoError::InvalidInput {
            field: "publisher staging",
            message: format!("{} is neither a regular file nor directory", root.display()),
        });
    }
    let mut total = 4096_u64;
    for entry in fs::read_dir(root).map_err(|source| OvenLegacyCargoError::Io {
        path: root.to_path_buf(),
        source,
    })? {
        let entry = match entry {
            Ok(entry) => entry,
            // Cargo atomically replaces transient archives while publishing a fresh dependency. The next quota poll
            // sees the replacement; this one must not fail the publisher merely because the old directory entry lost
            // the race after `read_dir` yielded it.
            Err(source) if source.kind() == io::ErrorKind::NotFound => continue,
            Err(source) => {
                return Err(OvenLegacyCargoError::Io {
                    path: root.to_path_buf(),
                    source,
                });
            }
        };
        let path = entry.path();
        match conservative_directory_reservation_with_seen_files(&path, seen_files) {
            Ok(reservation) => total = total.saturating_add(reservation),
            Err(OvenLegacyCargoError::Io { source, .. }) if source.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(total)
}

/// Platforms without inode identity retain conservative per-directory accounting.
#[cfg(not(unix))]
pub(crate) fn conservative_directory_reservation(root: &Path) -> Result<u64, OvenLegacyCargoError> {
    let metadata = fs::symlink_metadata(root).map_err(|source| OvenLegacyCargoError::Io {
        path: root.to_path_buf(),
        source,
    })?;
    if metadata.file_type().is_symlink() {
        return Ok(round_physical(metadata.len()));
    }
    if metadata.is_file() {
        return Ok(round_physical(metadata.len()));
    }
    if !metadata.is_dir() {
        return Err(OvenLegacyCargoError::InvalidInput {
            field: "publisher staging",
            message: format!("{} is neither a regular file nor directory", root.display()),
        });
    }
    let mut total = 4096_u64;
    for entry in fs::read_dir(root).map_err(|source| OvenLegacyCargoError::Io {
        path: root.to_path_buf(),
        source,
    })? {
        let entry = match entry {
            Ok(entry) => entry,
            Err(source) if source.kind() == io::ErrorKind::NotFound => continue,
            Err(source) => {
                return Err(OvenLegacyCargoError::Io {
                    path: root.to_path_buf(),
                    source,
                });
            }
        };
        let path = entry.path();
        match conservative_directory_reservation(&path) {
            Ok(reservation) => total = total.saturating_add(reservation),
            Err(OvenLegacyCargoError::Io { source, .. }) if source.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(total)
}

/// Refuse prepared publisher staging once its retained shard/index inputs exceed the compatibility-domain allowance.
///
/// A selection target has already been reclaimed when this runs, so the measurement is the bounded immutable batch
/// input rather than private Cargo cache state. The final store batch performs its own physical measurement and is the
/// only operation that makes the artifacts visible.
fn enforce_compiler_suite_prepared_staging_capacity(
    staging: &Path,
    transient_limit: u64,
) -> Result<(), OvenLegacyCargoError> {
    let reservation = conservative_directory_reservation(staging)?;
    if reservation > transient_limit {
        return Err(OvenLegacyCargoError::TransientCapacityExceeded {
            path: staging.to_path_buf(),
            observed_physical_bytes: reservation,
            limit_bytes: transient_limit,
        });
    }
    Ok(())
}

/// Reserve a full 4 KiB block for each transient regular file, matching store admission's conservative accounting.
fn round_physical(bytes: u64) -> u64 {
    const BLOCK: u64 = 4096;
    bytes.saturating_add(BLOCK - 1) / BLOCK * BLOCK
}

#[cfg(test)]
mod tests {
    use super::{
        CargoInvocationOutput, CargoMetadata, CargoMetadataPackage, CargoMetadataResolve,
        CargoMetadataResolveDependency, CargoMetadataResolveNode, CargoUnitGraph, CargoUnitGraphDependency,
        CargoUnitGraphTarget, CargoUnitGraphUnit, CompilerSuiteArtifactCatalog, InspectionPackageScope,
        OVEN_COMPILER_TEST_PROFILE, OVEN_COMPILER_TEST_SUITE_SCHEMA_VERSION,
        OVEN_COMPILER_TEST_SUITE_SHARD_SCHEMA_VERSION, OVEN_PROJECT_EXTENSION_PAYLOAD_SCHEMA_VERSION,
        OvenCompilerTestSuiteArtifactClosure, OvenCompilerTestSuiteFoundationReference, OvenCompilerTestSuitePayload,
        OvenCompilerTestSuiteShardPayload, OvenCompilerTestSuiteShardReference, OvenCompilerTestSuiteTarget,
        OvenCompilerTestSuiteToolchainLoafGenerationReference, OvenLegacyCargoBaseLoaf,
        OvenLegacyCargoDirectDependencyClosure, OvenLegacyCargoInspectionPackage, OvenLegacyCargoInvocationTarget,
        OvenLegacyCargoPrepareRequest, OvenLegacyCargoPublicationKind, OvenProjectExtensionPayload,
        ResolvedDirectDependency, artifact_closure, artifact_closure_from_reported_paths,
        canonicalize_supporting_artifacts, compiler_suite_artifact_catalog, compiler_suite_artifact_index,
        compiler_suite_bootstrap_selection, compiler_suite_cargo_build_output,
        compiler_suite_cli_target_from_artifact_index, compiler_suite_dependency_artifact,
        compiler_suite_dependency_directories, compiler_suite_direct_cli_plan, compiler_suite_direct_target_plan,
        compiler_suite_direct_target_shard_from_catalog, compiler_suite_direct_target_shard_plan,
        compiler_suite_foundation_dependencies, compiler_suite_foundation_lock, compiler_suite_foundation_manifest,
        compiler_suite_foundation_plans, compiler_suite_output_artifact_paths, compiler_suite_target_externs,
        compiler_suite_target_from_unit, compiler_suite_target_runner, compiler_suite_target_selection_features,
        compiler_suite_target_selection_groups, compiler_suite_target_selections, compiler_suite_toolchain_data_plans,
        compiler_suite_toolchain_loaf_generation_reference, compiler_suite_verified_target_source_bytes,
        compiler_suite_workspace_libraries_for_roots, conservative_directory_reservation, create_publisher_staging,
        digest_local_cargo_workspace_authority, direct_rustc_compile_environment,
        direct_rustc_reusable_project_plan_environment, explicit_project_bake_inspection_sources,
        generated_project_direct_dependencies, inspection_package_closure_ids, legacy_cargo_inspection_sources,
        locked_generated_project, materialize_sealed_registry_lock, materialized_files_from_directory,
        prepare_compiler_test_suite, prepare_direct_rustc_plan, project_registry_source_dependencies,
        publisher_direct_dependencies, publisher_registry_source_catalog, read_legacy_cargo_metadata_with_lock_policy,
        reclaim_unmaterialized_compiler_suite_target_files, release_cohort_generated_project_lock,
        resolve_direct_dependency_packages, run_legacy_cargo, run_legacy_cargo_invocation,
        select_compiler_test_suite_identity, select_existing_project_extension_identity,
        source_compiler_vocab_support_paths_are_available, stage_compiler_suite_shard_files,
        stage_registry_source_directory, stage_self_contained_sdk_provider_tree, validate_compiler_suite_unit_graph,
        validate_generated_registry_lock, validate_release_cohort_registry_lock,
    };
    use crate::oven::loaf::{
        OVEN_LOAF_ENVELOPE_MANIFEST_SCHEMA_VERSION, OVEN_LOAF_SCHEMA_VERSION, OvenLoaf, OvenLoafEnvelopeManifest,
        OvenLoafEnvelopeMember, OvenLoafMemberRole,
    };
    use crate::oven::rustc::{
        OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION, OVEN_RUSTC_REGISTRY_LOCK_RELATIVE_PATH, OvenRustcArtifactExtern,
        OvenRustcArtifactManifest, OvenRustcRegistrySource, OvenRustcRegistrySourcePackage,
        OvenRustcSupportingArtifact, rustc_host_target, rustc_identity,
    };
    use crate::oven::store::{
        OvenArtifactKind, OvenArtifactMaterializedFile, OvenArtifactPublishRequest, OvenStore, OvenStoreLimits,
    };
    use crate::oven::{
        OvenBuildIntent, OvenCompilerSuiteRequest, OvenGeneratedProjectRequest, digest_bytes, digest_source_tree,
        receipt_generated_project, receipt_native_compiler_suite,
    };
    use std::{
        collections::BTreeMap,
        fs,
        path::{Path, PathBuf},
        process::Command,
    };

    #[test]
    fn source_compiler_vocab_support_requires_a_binary_beneath_the_source_target()
    -> Result<(), Box<dyn std::error::Error>> {
        let source = tempfile::tempdir()?;
        let source_root = source.path();
        fs::create_dir_all(source_root.join("crates/incan_vocab"))?;
        fs::create_dir_all(source_root.join("target/release"))?;
        fs::write(source_root.join("Cargo.lock"), "# fixture\n")?;
        fs::write(
            source_root.join("crates/incan_vocab/Cargo.toml"),
            "[package]\nname = \"incan_vocab\"\n",
        )?;

        let source_binary = source_root.join("target/release/incan");
        assert!(source_compiler_vocab_support_paths_are_available(
            source_root,
            &source_binary
        ));

        let installed_binary = tempfile::tempdir()?.path().join("toolchains/0.5.1-rc2/bin/incan");
        assert!(!source_compiler_vocab_support_paths_are_available(
            source_root,
            &installed_binary
        ));
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn transient_reservation_counts_a_symlink_without_following_its_target() -> Result<(), Box<dyn std::error::Error>> {
        let staging = tempfile::tempdir()?;
        let outside = tempfile::tempdir()?;
        let target = outside.path().join("large-external-payload");
        fs::write(&target, vec![0_u8; 1024 * 1024])?;
        std::os::unix::fs::symlink(&target, staging.path().join("cargo-transient-link"))?;

        let reservation = conservative_directory_reservation(staging.path())?;
        assert!(
            reservation >= 2 * 4096,
            "the staging root and link must be accounted for"
        );
        assert!(
            reservation < 1024 * 1024,
            "transient reservation must not follow a symlink outside publisher staging"
        );
        Ok(())
    }

    fn write_test_loaf_envelope(
        loafs: &std::path::Path,
        members: Vec<OvenLoafEnvelopeMember>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        fs::write(loafs.join(".envelope.lock"), "")?;
        fs::write(
            loafs.join("envelope.json"),
            serde_json::to_vec(&OvenLoafEnvelopeManifest {
                schema_version: OVEN_LOAF_ENVELOPE_MANIFEST_SCHEMA_VERSION,
                envelope: "compiler-suite".to_string(),
                generation_identity: "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                    .to_string(),
                evidence: BTreeMap::new(),
                loafs: members,
            })?,
        )?;
        Ok(())
    }

    #[test]
    fn inspection_surface_keeps_declared_registry_closure_but_not_provider_sources()
    -> Result<(), Box<dyn std::error::Error>> {
        let registry = Some("registry+https://example.invalid/index".to_string());
        let package = |id: &str, name: &str, version: &str, source: Option<String>| CargoMetadataPackage {
            id: id.to_string(),
            name: name.to_string(),
            version: version.to_string(),
            manifest_path: PathBuf::from(format!("/{name}/Cargo.toml")),
            source,
        };
        let node = |id: &str, dependencies: &[&str]| CargoMetadataResolveNode {
            id: id.to_string(),
            features: Vec::new(),
            dependencies: dependencies
                .iter()
                .map(|dependency| (*dependency).to_string())
                .collect(),
            deps: Vec::new(),
        };
        let metadata = CargoMetadata {
            packages: vec![
                package("root", "fixture", "0.1.0", None),
                package("declared", "declared-rust", "1.2.3", registry.clone()),
                package("transitive", "declared-transitive", "2.0.0", registry.clone()),
                package("provider", "provider-runtime", "9.0.0", registry),
            ],
            resolve: Some(CargoMetadataResolve {
                root: Some("root".to_string()),
                nodes: vec![
                    node("root", &["declared", "provider"]),
                    node("declared", &["transitive"]),
                    node("transitive", &[]),
                    node("provider", &[]),
                ],
            }),
        };

        let selected = inspection_package_closure_ids(
            &metadata,
            &[OvenLegacyCargoInspectionPackage {
                package: "declared-rust".to_string(),
                version_requirement: "1".to_string(),
            }],
            InspectionPackageScope::DirectRoot,
        )?;

        assert_eq!(
            selected,
            ["declared".to_string(), "transitive".to_string()].into_iter().collect()
        );
        assert!(!selected.contains("provider"));

        let complete = inspection_package_closure_ids(&metadata, &[], InspectionPackageScope::CompleteResolvedGraph)?;
        assert_eq!(
            complete,
            ["declared", "provider", "root", "transitive"]
                .into_iter()
                .map(str::to_string)
                .collect()
        );
        Ok(())
    }

    #[test]
    fn project_extension_catalog_keeps_every_locked_source_without_fabricating_a_leaf()
    -> Result<(), Box<dyn std::error::Error>> {
        let fixture = tempfile::tempdir()?;
        let staging = fixture.path().join("staging");
        fs::create_dir_all(&staging)?;
        let registry = "registry+https://example.invalid/index";
        let package = |id: &str, name: &str, dependencies: &[&str]| -> Result<_, Box<dyn std::error::Error>> {
            let root = fixture.path().join(name);
            fs::create_dir_all(root.join("src"))?;
            fs::write(
                root.join("Cargo.toml"),
                format!("[package]\nname = \"{name}\"\nversion = \"1.0.0\"\n"),
            )?;
            fs::write(root.join("src/lib.rs"), "pub fn sealed() {}\n")?;
            Ok((
                CargoMetadataPackage {
                    id: id.to_string(),
                    name: name.to_string(),
                    version: "1.0.0".to_string(),
                    manifest_path: root.join("Cargo.toml"),
                    source: Some(registry.to_string()),
                },
                CargoMetadataResolveNode {
                    id: id.to_string(),
                    features: Vec::new(),
                    dependencies: dependencies
                        .iter()
                        .map(|dependency| (*dependency).to_string())
                        .collect(),
                    deps: Vec::new(),
                },
            ))
        };
        let (serde_json, serde_json_node) = package("serde-json", "serde_json", &["serde"])?;
        let (serde, serde_node) = package("serde", "serde", &["serde-derive"])?;
        let (serde_derive, serde_derive_node) = package("serde-derive", "serde_derive", &[])?;
        let (uninspected, uninspected_node) = package("uninspected", "uninspected", &[])?;
        let metadata = CargoMetadata {
            packages: vec![
                CargoMetadataPackage {
                    id: "root".to_string(),
                    name: "fixture".to_string(),
                    version: "0.1.0".to_string(),
                    manifest_path: fixture.path().join("Cargo.toml"),
                    source: None,
                },
                serde_json,
                serde,
                serde_derive,
                uninspected,
            ],
            resolve: Some(CargoMetadataResolve {
                root: Some("root".to_string()),
                nodes: vec![
                    CargoMetadataResolveNode {
                        id: "root".to_string(),
                        features: Vec::new(),
                        dependencies: vec!["serde-json".to_string()],
                        deps: Vec::new(),
                    },
                    serde_json_node,
                    serde_node,
                    serde_derive_node,
                    uninspected_node,
                ],
            }),
        };
        let lock = format!(
            "version = 4\n\n{}",
            ["serde_json", "serde", "serde_derive", "uninspected"]
                .into_iter()
                .map(|name| format!(
                    "[[package]]\nname = \"{name}\"\nversion = \"1.0.0\"\nsource = \"{registry}\"\nchecksum = \"{name}-checksum\"\n\n"
                ))
                .collect::<String>()
        );

        let (sources, source_artifacts) =
            publisher_registry_source_catalog(&metadata, lock.as_bytes(), &staging, None, &[], true, None)?;

        assert_eq!(
            sources.iter().map(|source| source.package.as_str()).collect::<Vec<_>>(),
            ["serde", "serde_derive", "serde_json", "uninspected"]
        );
        assert!(sources.iter().any(|source| source.package == "serde_derive"));
        assert!(
            source_artifacts
                .iter()
                .any(|artifact| artifact.relative_path.ends_with("/Cargo.toml"))
        );
        Ok(())
    }

    #[test]
    fn supporting_artifact_union_collapses_identical_source_records_and_rejects_conflicts()
    -> Result<(), Box<dyn std::error::Error>> {
        let artifact = |digest: &str| OvenRustcSupportingArtifact {
            relative_path: "registry-sources/fixture/Cargo.toml".to_string(),
            digest: digest.to_string(),
        };
        let mut identical = vec![artifact("sha256:fixture"), artifact("sha256:fixture")];
        canonicalize_supporting_artifacts(&mut identical)?;
        assert_eq!(identical, vec![artifact("sha256:fixture")]);

        let mut conflicting = vec![artifact("sha256:first"), artifact("sha256:second")];
        let error = canonicalize_supporting_artifacts(&mut conflicting)
            .err()
            .ok_or("conflicting source records must fail closed")?;
        assert!(error.to_string().contains("conflicting digests"));
        Ok(())
    }

    #[test]
    fn inspection_surface_rejects_an_ambiguous_locked_package() -> Result<(), Box<dyn std::error::Error>> {
        let registry = Some("registry+https://example.invalid/index".to_string());
        let metadata = CargoMetadata {
            packages: vec![
                CargoMetadataPackage {
                    id: "root".to_string(),
                    name: "compiler".to_string(),
                    version: "0.1.0".to_string(),
                    manifest_path: PathBuf::from("/compiler/Cargo.toml"),
                    source: None,
                },
                CargoMetadataPackage {
                    id: "duplicate-1".to_string(),
                    name: "duplicate".to_string(),
                    version: "1.2.0".to_string(),
                    manifest_path: PathBuf::from("/duplicate-1/Cargo.toml"),
                    source: registry.clone(),
                },
                CargoMetadataPackage {
                    id: "duplicate-2".to_string(),
                    name: "duplicate".to_string(),
                    version: "1.3.0".to_string(),
                    manifest_path: PathBuf::from("/duplicate-2/Cargo.toml"),
                    source: registry,
                },
            ],
            resolve: Some(CargoMetadataResolve {
                root: Some("root".to_string()),
                nodes: vec![
                    CargoMetadataResolveNode {
                        id: "root".to_string(),
                        features: Vec::new(),
                        dependencies: vec!["duplicate-1".to_string(), "duplicate-2".to_string()],
                        deps: Vec::new(),
                    },
                    CargoMetadataResolveNode {
                        id: "duplicate-1".to_string(),
                        features: Vec::new(),
                        dependencies: Vec::new(),
                        deps: Vec::new(),
                    },
                    CargoMetadataResolveNode {
                        id: "duplicate-2".to_string(),
                        features: Vec::new(),
                        dependencies: Vec::new(),
                        deps: Vec::new(),
                    },
                ],
            }),
        };

        let error = match inspection_package_closure_ids(
            &metadata,
            &[OvenLegacyCargoInspectionPackage {
                package: "duplicate".to_string(),
                version_requirement: "1".to_string(),
            }],
            InspectionPackageScope::DirectRoot,
        ) {
            Ok(selected) => return Err(format!("ambiguous package unexpectedly selected: {selected:?}").into()),
            Err(error) => error,
        };
        assert!(error.to_string().contains("ambiguously"), "unexpected error: {error}");
        Ok(())
    }

    #[test]
    fn compiler_suite_dependency_directories_preserves_a_host_only_foundation() -> Result<(), Box<dyn std::error::Error>>
    {
        let staging = tempfile::tempdir()?;
        let target_deps = staging.path().join("target/aarch64-apple-darwin/oven-test/deps");
        let host_deps = staging.path().join("target/oven-test/deps");
        fs::create_dir_all(&host_deps)?;
        fs::write(host_deps.join("libfixture-abc.dylib"), "host-only foundation")?;

        let directories = compiler_suite_dependency_directories(target_deps.clone(), host_deps.clone());

        assert_eq!(directories, vec![host_deps.clone()]);
        let catalog = compiler_suite_artifact_catalog(staging.path(), &directories, &[])?;
        assert_eq!(catalog.closure.dependency_search_paths, vec!["target/oven-test/deps"]);
        assert_eq!(catalog.materialized_files.len(), 1);
        assert!(!target_deps.exists());
        Ok(())
    }

    #[test]
    fn compiler_suite_catalog_uses_a_reported_foundation_artifact_without_a_deps_directory()
    -> Result<(), Box<dyn std::error::Error>> {
        let staging = tempfile::tempdir()?;
        let artifact = staging
            .path()
            .join("third-party-foundation-target/aarch64-apple-darwin/oven-test/liboven_compiler_foundation.rlib");
        fs::create_dir_all(artifact.parent().ok_or("foundation artifact parent missing")?)?;
        fs::write(&artifact, "foundation artifact")?;
        let output = CargoInvocationOutput {
            stdout: format!(
                "{}\n",
                serde_json::json!({
                    "reason": "compiler-artifact",
                    "package_id": "oven-compiler-foundation 0.0.0",
                    "target": { "name": "oven_compiler_foundation" },
                    "filenames": [artifact],
                })
            )
            .into_bytes(),
        };

        let reported_artifacts = compiler_suite_output_artifact_paths(&output)?;
        let catalog = compiler_suite_artifact_catalog(staging.path(), &[], &reported_artifacts)?;

        assert_eq!(reported_artifacts, vec![fs::canonicalize(&artifact)?]);
        assert_eq!(catalog.closure.dependency_search_paths, Vec::<String>::new());
        assert_eq!(catalog.materialized_files.len(), 1);
        assert_eq!(
            catalog.materialized_files[0].relative_path,
            "third-party-foundation-target/aarch64-apple-darwin/oven-test/liboven_compiler_foundation.rlib"
        );
        Ok(())
    }

    #[test]
    fn publisher_staging_canonicalizes_a_relative_store_path() -> Result<(), Box<dyn std::error::Error>> {
        let parent = tempfile::tempdir_in(".")?;
        let relative_parent =
            std::path::PathBuf::from(parent.path().file_name().ok_or("temporary parent has no name")?);

        let staging = create_publisher_staging(&relative_parent)?;

        assert!(staging.is_absolute());
        assert!(staging.starts_with(fs::canonicalize(parent.path())?));
        Ok(())
    }

    #[test]
    fn publisher_selects_a_host_proc_macro_dylib_as_a_missing_direct_extern() -> Result<(), Box<dyn std::error::Error>>
    {
        let staging = tempfile::tempdir()?;
        let target_deps = staging.path().join("target/aarch64-apple-darwin/debug/deps");
        let host_deps = staging.path().join("target/debug/deps");
        fs::create_dir_all(&target_deps)?;
        fs::create_dir_all(&host_deps)?;
        fs::write(host_deps.join("libincan_web_macros-abc.dylib"), "host proc macro")?;

        let direct_dependencies = BTreeMap::from([("incan_web_macros".to_string(), "incan_web_macros".to_string())]);
        let (_, externs, supporting) = artifact_closure(
            staging.path(),
            &target_deps,
            &[target_deps.clone(), host_deps],
            &direct_dependencies,
            false,
        )?;

        assert_eq!(externs.len(), 1);
        assert_eq!(externs[0].crate_name, "incan_web_macros");
        assert_eq!(
            externs[0].relative_path,
            "target/debug/deps/libincan_web_macros-abc.dylib"
        );
        assert!(
            supporting
                .iter()
                .all(|artifact| artifact.relative_path != externs[0].relative_path)
        );
        Ok(())
    }

    #[test]
    fn publisher_prefers_a_target_direct_extern_over_a_matching_host_dylib() -> Result<(), Box<dyn std::error::Error>> {
        let staging = tempfile::tempdir()?;
        let target_deps = staging.path().join("target/aarch64-apple-darwin/debug/deps");
        let host_deps = staging.path().join("target/debug/deps");
        fs::create_dir_all(&target_deps)?;
        fs::create_dir_all(&host_deps)?;
        fs::write(target_deps.join("libfixture-abc.rlib"), "target library")?;
        fs::write(host_deps.join("libfixture-def.dylib"), "host dylib")?;

        let direct_dependencies = BTreeMap::from([("fixture".to_string(), "fixture".to_string())]);
        let (_, externs, _) = artifact_closure(
            staging.path(),
            &target_deps,
            &[target_deps.clone(), host_deps],
            &direct_dependencies,
            false,
        )?;

        assert_eq!(externs.len(), 1);
        assert_eq!(
            externs[0].relative_path,
            "target/aarch64-apple-darwin/debug/deps/libfixture-abc.rlib"
        );
        Ok(())
    }

    #[test]
    fn reported_artifact_closure_keeps_the_direct_package_instance_when_versions_share_a_crate_name()
    -> Result<(), Box<dyn std::error::Error>> {
        let staging = tempfile::tempdir()?;
        let target_triple = "aarch64-apple-darwin";
        let target_dependencies = staging.path().join(format!("target/{target_triple}/debug/deps"));
        fs::create_dir_all(&target_dependencies)?;
        let transitive = target_dependencies.join("libsubstrait-aaaa.rlib");
        let direct = target_dependencies.join("libsubstrait-zzzz.rlib");
        fs::write(&transitive, "substrait 0.62 transitive")?;
        fs::write(&direct, "substrait 0.63 direct")?;
        let compiler_artifact = |package_id: &str, artifact: &Path| {
            serde_json::json!({
                "reason": "compiler-artifact",
                "package_id": package_id,
                "target": { "name": "substrait" },
                "filenames": [artifact],
            })
            .to_string()
        };
        let output = CargoInvocationOutput {
            stdout: format!(
                "{}\n{}\n",
                compiler_artifact("substrait 0.62.2 (registry+https://example.invalid)", &transitive),
                compiler_artifact("substrait 0.63.0 (registry+https://example.invalid)", &direct),
            )
            .into_bytes(),
        };
        let dependencies = BTreeMap::from([(
            "substrait".to_string(),
            ResolvedDirectDependency {
                package: "substrait".to_string(),
                package_id: "substrait 0.63.0 (registry+https://example.invalid)".to_string(),
            },
        )]);

        let (_, externs, _) = artifact_closure_from_reported_paths(
            staging.path(),
            target_triple,
            "debug",
            &dependencies,
            false,
            &[output],
        )?;

        assert_eq!(externs.len(), 1);
        assert_eq!(externs[0].crate_name, "substrait");
        assert!(
            externs[0].relative_path.ends_with("libsubstrait-zzzz.rlib"),
            "the root --extern must use the direct 0.63 package, not its 0.62 transitive namesake: {}",
            externs[0].relative_path
        );
        Ok(())
    }

    #[test]
    fn reported_artifact_closure_retains_a_renamed_library_target_from_cargo_build_output()
    -> Result<(), Box<dyn std::error::Error>> {
        let staging = tempfile::tempdir()?;
        let target_triple = "aarch64-apple-darwin";
        let output_directory = staging
            .path()
            .join(format!("target/{target_triple}/debug/build/coreaudio-rs/abc123/out"));
        fs::create_dir_all(&output_directory)?;
        let library = output_directory.join("libcoreaudio-abc123.rlib");
        fs::write(&library, "coreaudio-rs library target")?;
        let compiler_artifact = serde_json::json!({
            "reason": "compiler-artifact",
            "package_id": "coreaudio-rs 0.2.16 (registry+https://example.invalid)",
            "target": { "name": "coreaudio" },
            "filenames": [library],
        });
        let output = CargoInvocationOutput {
            stdout: format!("{compiler_artifact}\n").into_bytes(),
        };

        let (search_paths, _, supporting) = artifact_closure_from_reported_paths(
            staging.path(),
            target_triple,
            "debug",
            &BTreeMap::new(),
            false,
            &[output],
        )?;

        assert_eq!(
            search_paths,
            vec!["target/aarch64-apple-darwin/debug/build/coreaudio-rs/abc123/out".to_string()]
        );
        assert_eq!(supporting.len(), 1);
        assert!(supporting[0].relative_path.ends_with("libcoreaudio-abc123.rlib"));
        Ok(())
    }

    #[test]
    fn resolved_direct_dependency_packages_preserve_the_root_alias_to_package_id()
    -> Result<(), Box<dyn std::error::Error>> {
        let metadata = CargoMetadata {
            packages: vec![
                CargoMetadataPackage {
                    id: "root".to_string(),
                    name: "fixture".to_string(),
                    version: "0.1.0".to_string(),
                    manifest_path: PathBuf::from("/fixture/Cargo.toml"),
                    source: None,
                },
                CargoMetadataPackage {
                    id: "substrait 0.62".to_string(),
                    name: "substrait".to_string(),
                    version: "0.62.2".to_string(),
                    manifest_path: PathBuf::from("/registry/substrait-0.62/Cargo.toml"),
                    source: Some("registry+https://example.invalid".to_string()),
                },
                CargoMetadataPackage {
                    id: "substrait 0.63".to_string(),
                    name: "substrait".to_string(),
                    version: "0.63.0".to_string(),
                    manifest_path: PathBuf::from("/registry/substrait-0.63/Cargo.toml"),
                    source: Some("registry+https://example.invalid".to_string()),
                },
            ],
            resolve: Some(CargoMetadataResolve {
                root: Some("root".to_string()),
                nodes: vec![
                    CargoMetadataResolveNode {
                        id: "root".to_string(),
                        features: Vec::new(),
                        dependencies: vec!["substrait 0.63".to_string()],
                        deps: vec![CargoMetadataResolveDependency {
                            name: "substrait".to_string(),
                            pkg: "substrait 0.63".to_string(),
                        }],
                    },
                    CargoMetadataResolveNode {
                        id: "substrait 0.62".to_string(),
                        features: Vec::new(),
                        dependencies: Vec::new(),
                        deps: Vec::new(),
                    },
                    CargoMetadataResolveNode {
                        id: "substrait 0.63".to_string(),
                        features: Vec::new(),
                        dependencies: vec!["substrait 0.62".to_string()],
                        deps: Vec::new(),
                    },
                ],
            }),
        };

        let resolved = resolve_direct_dependency_packages(
            &metadata,
            &BTreeMap::from([("substrait".to_string(), "substrait".to_string())]),
        )?;

        assert_eq!(resolved["substrait"].package_id, "substrait 0.63");
        Ok(())
    }

    #[test]
    fn project_registry_source_dependencies_preserve_two_compatible_root_aliases()
    -> Result<(), Box<dyn std::error::Error>> {
        let registry = "registry+https://example.invalid";
        let metadata = CargoMetadata {
            packages: vec![
                CargoMetadataPackage {
                    id: "root".to_string(),
                    name: "fixture".to_string(),
                    version: "0.1.0".to_string(),
                    manifest_path: PathBuf::from("/fixture/Cargo.toml"),
                    source: None,
                },
                CargoMetadataPackage {
                    id: "shared 1.2".to_string(),
                    name: "shared".to_string(),
                    version: "1.2.0".to_string(),
                    manifest_path: PathBuf::from("/registry/shared-1.2/Cargo.toml"),
                    source: Some(registry.to_string()),
                },
                CargoMetadataPackage {
                    id: "shared 1.8".to_string(),
                    name: "shared".to_string(),
                    version: "1.8.0".to_string(),
                    manifest_path: PathBuf::from("/registry/shared-1.8/Cargo.toml"),
                    source: Some(registry.to_string()),
                },
            ],
            resolve: Some(CargoMetadataResolve {
                root: Some("root".to_string()),
                nodes: vec![CargoMetadataResolveNode {
                    id: "root".to_string(),
                    features: Vec::new(),
                    dependencies: vec!["shared 1.2".to_string(), "shared 1.8".to_string()],
                    deps: vec![
                        CargoMetadataResolveDependency {
                            name: "shared_old".to_string(),
                            pkg: "shared 1.2".to_string(),
                        },
                        CargoMetadataResolveDependency {
                            name: "shared_new".to_string(),
                            pkg: "shared 1.8".to_string(),
                        },
                    ],
                }],
            }),
        };
        let source = |version: &str, checksum: &str| OvenRustcRegistrySourcePackage {
            package: "shared".to_string(),
            version: version.to_string(),
            features: Vec::new(),
            source: OvenRustcRegistrySource {
                registry: registry.to_string(),
                checksum: checksum.to_string(),
                relative_root: format!("registry-sources/shared-{version}"),
                digest: format!("sha256:shared-{version}"),
            },
        };
        let sources = vec![source("1.2.0", "old-checksum"), source("1.8.0", "new-checksum")];
        let dependencies = BTreeMap::from([
            ("shared_new".to_string(), "shared".to_string()),
            ("shared_old".to_string(), "shared".to_string()),
        ]);

        let selected = project_registry_source_dependencies(&metadata, &dependencies, &sources)?;

        assert_eq!(selected.len(), 2);
        assert_eq!(selected[0].alias, "shared_new");
        assert_eq!(selected[0].version, "1.8.0");
        assert_eq!(selected[0].checksum, "new-checksum");
        assert_eq!(selected[1].alias, "shared_old");
        assert_eq!(selected[1].version, "1.2.0");
        assert_eq!(selected[1].checksum, "old-checksum");

        let Err(error) = project_registry_source_dependencies(&metadata, &dependencies, &sources[..1]) else {
            return Err(std::io::Error::other("missing exact source record was accepted").into());
        };
        assert!(error.to_string().contains("has 0 exact sealed source records"));
        Ok(())
    }

    #[test]
    fn publisher_recovers_one_sealed_target_library_when_cargo_json_omits_its_artifact_family()
    -> Result<(), Box<dyn std::error::Error>> {
        let staging = tempfile::tempdir()?;
        let source = staging.path().join("registry/blake2/src/lib.rs");
        let target_dependencies = staging
            .path()
            .join("third-party-foundation-target/x86_64-unknown-linux-gnu/oven-test/deps");
        fs::create_dir_all(source.parent().ok_or("registry source parent missing")?)?;
        fs::create_dir_all(&target_dependencies)?;
        fs::write(&source, "pub fn fixture() {}\n")?;
        let resolved_library = target_dependencies.join("libblake2-resolved.rlib");
        fs::write(&resolved_library, "resolved blake2 library")?;
        let unit = CargoUnitGraphUnit {
            pkg_id: "blake2 0.10.6 (registry+https://example.invalid)".to_string(),
            target: CargoUnitGraphTarget {
                kind: vec!["lib".to_string()],
                crate_types: vec!["lib".to_string()],
                name: "blake2".to_string(),
                src_path: source.clone(),
                edition: "2024".to_string(),
            },
            mode: "build".to_string(),
            platform: Some("x86_64-unknown-linux-gnu".to_string()),
            features: Vec::new(),
            dependencies: Vec::new(),
        };
        let catalog = compiler_suite_artifact_catalog(staging.path(), std::slice::from_ref(&target_dependencies), &[])?;
        let selected = compiler_suite_dependency_artifact(
            &unit,
            &BTreeMap::new(),
            &catalog,
            "blake2",
            Some("x86_64-unknown-linux-gnu"),
        )?;
        assert_eq!(
            selected.relative_path,
            "third-party-foundation-target/x86_64-unknown-linux-gnu/oven-test/deps/libblake2-resolved.rlib"
        );

        // The sealed-catalog recovery is deliberately unique: two receipt-target libraries remain an explicit
        // publisher error rather than an arbitrary direct-Rustc selection.
        fs::write(
            target_dependencies.join("libblake2-second.rlib"),
            "second incompatible blake2 library",
        )?;
        let ambiguous_catalog = compiler_suite_artifact_catalog(staging.path(), &[target_dependencies], &[])?;
        assert!(matches!(
            compiler_suite_dependency_artifact(
                &unit,
                &BTreeMap::new(),
                &ambiguous_catalog,
                "blake2",
                Some("x86_64-unknown-linux-gnu"),
            ),
            Err(super::OvenLegacyCargoError::InvalidInput {
                field: "compiler-suite unit graph",
                ..
            })
        ));
        Ok(())
    }

    #[test]
    fn publisher_materializes_a_complete_provider_tree_with_portable_paths() -> Result<(), Box<dyn std::error::Error>> {
        let provider = tempfile::tempdir()?;
        fs::create_dir_all(provider.path().join("components/core"))?;
        fs::write(
            provider.path().join("sdk-inventory.json"),
            "{\n  \"schema_version\": 2,\n  \"sdk_id\": \"fixture\",\n  \"sdk_version\": \"0.1.0\",\n  \"compiler_requirement\": \">=0.1.0\",\n  \"provider_codegen_revision\": 5,\n  \"components\": {},\n  \"profiles\": {\"default\": []}\n}\n",
        )?;
        fs::write(provider.path().join("components/core/provider.incnlib"), "provider")?;

        let files = materialized_files_from_directory(provider.path(), "providers", "SDK provider inventory")?;
        let relative_paths = files.into_iter().map(|file| file.relative_path).collect::<Vec<_>>();
        assert_eq!(
            relative_paths,
            vec![
                "providers/components/core/provider.incnlib".to_string(),
                "providers/sdk-inventory.json".to_string(),
            ]
        );
        Ok(())
    }

    #[test]
    fn publisher_materializes_the_sealed_registry_lock_with_registry_sources() -> Result<(), Box<dyn std::error::Error>>
    {
        let staging = tempfile::tempdir()?;
        let sealed_lock = staging.path().join(OVEN_RUSTC_REGISTRY_LOCK_RELATIVE_PATH);
        let parent = sealed_lock.parent().ok_or("registry lock has no parent")?;
        fs::create_dir_all(parent)?;
        fs::write(&sealed_lock, "version = 4\n")?;

        let mut files = Vec::new();
        materialize_sealed_registry_lock(staging.path(), true, &mut files)?;

        assert_eq!(files.len(), 1);
        assert_eq!(files[0].relative_path, OVEN_RUSTC_REGISTRY_LOCK_RELATIVE_PATH);
        assert_eq!(files[0].source_path, fs::canonicalize(&sealed_lock)?);
        Ok(())
    }

    #[test]
    fn publisher_seals_sdk_component_runtime_paths_inside_the_suite_entry() -> Result<(), Box<dyn std::error::Error>> {
        let provider = tempfile::tempdir()?;
        let component = provider.path().join("components/stdlib-core");
        let inherited_runtime = provider.path().join("runtime");
        fs::create_dir_all(&component)?;
        fs::create_dir_all(&inherited_runtime)?;
        fs::write(
            provider.path().join("sdk-inventory.json"),
            "{\n  \"schema_version\": 2,\n  \"sdk_id\": \"fixture\",\n  \"sdk_version\": \"0.1.0\",\n  \"compiler_requirement\": \">=0.1.0\",\n  \"provider_codegen_revision\": 5,\n  \"components\": {},\n  \"profiles\": {\"default\": []}\n}\n",
        )?;
        fs::write(
            component.join("Cargo.toml"),
            "[package]\nname = \"fixture_provider\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[dependencies.incan_derive]\npath = \"../../../../outside/incan_derive\"\n\n[dependencies.incan_stdlib]\npath = \"../../../../outside/incan_stdlib\"\n",
        )?;
        let inherited_lock = inherited_runtime.join("Cargo.lock");
        fs::write(&inherited_lock, "stale sealed runtime lock\n")?;
        fs::write(inherited_runtime.join("obsolete-runtime-file"), "stale")?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            fs::set_permissions(&inherited_lock, fs::Permissions::from_mode(0o444))?;
        }
        let staging = tempfile::tempdir()?;

        let staged = stage_self_contained_sdk_provider_tree(provider.path(), staging.path())?;
        let manifest = fs::read_to_string(staged.join("components/stdlib-core/Cargo.toml"))?;

        assert!(manifest.contains("path = \"../../runtime/crates/incan_derive\""));
        assert!(manifest.contains("path = \"../../runtime/crates/incan_stdlib\""));
        assert!(staged.join("runtime/Cargo.toml").is_file());
        assert!(staged.join("runtime/Cargo.lock").is_file());
        assert_ne!(
            fs::read_to_string(staged.join("runtime/Cargo.lock"))?,
            "stale sealed runtime lock\n"
        );
        assert!(!staged.join("runtime/obsolete-runtime-file").exists());
        assert!(staged.join("runtime/crates/incan_core/src/lib.rs").is_file());
        assert!(staged.join("runtime/crates/incan_derive/src/lib.rs").is_file());
        assert!(staged.join("runtime/crates/incan_stdlib/src/lib.rs").is_file());
        assert!(staged.join("runtime/crates/incan_web_macros/src/lib.rs").is_file());

        let files = materialized_files_from_directory(&staged, "providers", "SDK provider inventory")?;
        assert!(
            files
                .iter()
                .any(|file| file.relative_path == "providers/runtime/crates/incan_stdlib/src/lib.rs")
        );
        Ok(())
    }

    #[test]
    fn publisher_keeps_dependency_used_only_by_nested_generated_module() -> Result<(), Box<dyn std::error::Error>> {
        let generated = tempfile::tempdir()?;
        fs::create_dir_all(generated.path().join("src/models"))?;
        fs::write(generated.path().join("src/lib.rs"), "pub mod models;\n")?;
        fs::write(
            generated.path().join("src/models/generated.rs"),
            "#[derive(incan_derive::IncanModel)]\npub struct Model;\n",
        )?;
        let declared: BTreeMap<_, _> = [
            ("incan_derive".to_string(), "incan_derive".to_string()),
            ("unused_crate".to_string(), "unused_crate".to_string()),
        ]
        .into_iter()
        .collect();

        let selected = generated_project_direct_dependencies(generated.path(), &declared)?;

        assert_eq!(
            selected,
            [("incan_derive".to_string(), "incan_derive".to_string())]
                .into_iter()
                .collect()
        );
        Ok(())
    }

    #[test]
    fn complete_stdlib_closure_keeps_checked_dependency_not_exercised_by_fixture()
    -> Result<(), Box<dyn std::error::Error>> {
        let generated = tempfile::tempdir()?;
        fs::create_dir_all(generated.path().join("src"))?;
        fs::write(generated.path().join("src/main.rs"), "fn main() {}\n")?;
        let declared: BTreeMap<_, _> = [
            ("byteorder".to_string(), "byteorder".to_string()),
            ("incan_stdlib".to_string(), "incan_stdlib".to_string()),
        ]
        .into_iter()
        .collect();

        let selected = publisher_direct_dependencies(
            generated.path(),
            declared.clone(),
            OvenLegacyCargoPublicationKind::Executable,
            OvenLegacyCargoDirectDependencyClosure::CheckedDeclared,
        )?;

        assert_eq!(selected, declared);
        Ok(())
    }

    #[test]
    fn publisher_keeps_compiler_owned_route_macro_expansion_roots() -> Result<(), Box<dyn std::error::Error>> {
        let generated = tempfile::tempdir()?;
        fs::create_dir_all(generated.path().join("src"))?;
        fs::write(
            generated.path().join("src/main.rs"),
            "#[incan_web_macros::route(\"/health\")]\nfn health() {}\n",
        )?;
        let declared = [
            ("axum".to_string(), "axum".to_string()),
            ("incan_web_macros".to_string(), "incan_web_macros".to_string()),
            ("inventory".to_string(), "inventory".to_string()),
            ("unused_crate".to_string(), "unused_crate".to_string()),
        ]
        .into_iter()
        .collect();

        let selected = generated_project_direct_dependencies(generated.path(), &declared)?;

        assert_eq!(
            selected,
            [
                ("axum".to_string(), "axum".to_string()),
                ("incan_web_macros".to_string(), "incan_web_macros".to_string()),
                ("inventory".to_string(), "inventory".to_string()),
            ]
            .into_iter()
            .collect()
        );
        Ok(())
    }

    #[test]
    fn compiler_foundation_manifest_excludes_workspace_sources_and_preserves_resolved_features()
    -> Result<(), Box<dyn std::error::Error>> {
        let compiler_root = tempfile::tempdir()?;
        let external_root = tempfile::tempdir()?;
        let unused_root = tempfile::tempdir()?;
        fs::create_dir_all(compiler_root.path().join("src"))?;
        fs::create_dir_all(external_root.path().join("src"))?;
        fs::create_dir_all(unused_root.path().join("src"))?;
        fs::write(
            compiler_root.path().join("Cargo.toml"),
            "[package]\nname = \"compiler\"\nversion = \"0.1.0\"\n",
        )?;
        fs::write(compiler_root.path().join("src/lib.rs"), "pub fn compiler() {}\n")?;
        fs::write(external_root.path().join("src/lib.rs"), "pub fn dependency() {}\n")?;
        fs::write(unused_root.path().join("src/lib.rs"), "pub fn unused() {}\n")?;
        let graph = CargoUnitGraph {
            version: 1,
            units: vec![
                CargoUnitGraphUnit {
                    pkg_id: "compiler 0.1.0 (path+file:///compiler)".to_string(),
                    target: CargoUnitGraphTarget {
                        kind: vec!["lib".to_string()],
                        crate_types: vec!["lib".to_string()],
                        name: "compiler".to_string(),
                        src_path: compiler_root.path().join("src/lib.rs"),
                        edition: "2024".to_string(),
                    },
                    mode: "test".to_string(),
                    platform: Some("aarch64-apple-darwin".to_string()),
                    features: Vec::new(),
                    dependencies: vec![CargoUnitGraphDependency {
                        index: 1,
                        extern_crate_name: Some("external_dep".to_string()),
                    }],
                },
                CargoUnitGraphUnit {
                    pkg_id: "external_dep 1.2.3 (registry+https://example.invalid/index)".to_string(),
                    target: CargoUnitGraphTarget {
                        kind: vec!["lib".to_string()],
                        crate_types: vec!["lib".to_string()],
                        name: "external_dep".to_string(),
                        src_path: external_root.path().join("src/lib.rs"),
                        edition: "2021".to_string(),
                    },
                    mode: "build".to_string(),
                    platform: Some("aarch64-apple-darwin".to_string()),
                    features: vec!["default".to_string(), "serde".to_string()],
                    dependencies: Vec::new(),
                },
                CargoUnitGraphUnit {
                    pkg_id: "unused 9.9.9 (registry+https://example.invalid/index)".to_string(),
                    target: CargoUnitGraphTarget {
                        kind: vec!["lib".to_string()],
                        crate_types: vec!["lib".to_string()],
                        name: "unused".to_string(),
                        src_path: unused_root.path().join("src/lib.rs"),
                        edition: "2021".to_string(),
                    },
                    mode: "build".to_string(),
                    platform: Some("aarch64-apple-darwin".to_string()),
                    features: Vec::new(),
                    dependencies: Vec::new(),
                },
            ],
            roots: vec![0],
        };
        let metadata = CargoMetadata {
            resolve: None,
            packages: vec![
                CargoMetadataPackage {
                    id: "compiler 0.1.0 (path+file:///compiler)".to_string(),
                    name: "compiler".to_string(),
                    version: "0.1.0".to_string(),
                    manifest_path: compiler_root.path().join("Cargo.toml"),
                    source: None,
                },
                CargoMetadataPackage {
                    id: "external_dep 1.2.3 (registry+https://example.invalid/index)".to_string(),
                    name: "external-dep".to_string(),
                    version: "1.2.3".to_string(),
                    manifest_path: external_root.path().join("Cargo.toml"),
                    source: Some("registry+https://example.invalid/index".to_string()),
                },
                CargoMetadataPackage {
                    id: "unused 9.9.9 (registry+https://example.invalid/index)".to_string(),
                    name: "unused".to_string(),
                    version: "9.9.9".to_string(),
                    manifest_path: unused_root.path().join("Cargo.toml"),
                    source: Some("registry+https://example.invalid/index".to_string()),
                },
            ],
        };

        let dependencies = compiler_suite_foundation_dependencies(compiler_root.path(), &graph, &metadata)?;
        assert_eq!(dependencies.len(), 1, "foundation dependencies: {dependencies:?}");
        assert_eq!(dependencies[0].alias, "oven_foundation_0000");
        assert_eq!(dependencies[0].package, "external-dep");
        assert_eq!(dependencies[0].version, "1.2.3");
        assert_eq!(
            dependencies[0].source.as_deref(),
            Some("registry+https://example.invalid/index")
        );
        assert_eq!(dependencies[0].features, ["default", "serde"]);
        assert_eq!(dependencies[0].path, None);
        let manifest = compiler_suite_foundation_manifest(&dependencies)?;
        assert!(manifest.contains("package = \"external-dep\""));
        assert!(manifest.contains("version = \"=1.2.3\""));
        assert!(manifest.contains("default-features = true"));
        assert!(manifest.contains("features = [\"serde\"]"));
        assert!(manifest.contains("[profile.oven-test]\ninherits = \"dev\"\ndebug = 0\nincremental = false"));
        assert!(!manifest.contains("unused"));
        assert!(!manifest.contains("compiler 0.1.0"));
        let lock = compiler_suite_foundation_lock(
            br#"version = 4

[[package]]
name = "compiler"
version = "0.1.0"

[[package]]
name = "external-dep"
version = "1.2.3"
source = "registry+https://example.invalid/index"
checksum = "fixture"
"#,
            &dependencies,
        )?;
        let lock = toml::from_str::<toml::Value>(std::str::from_utf8(&lock)?)?;
        let foundation = lock["package"]
            .as_array()
            .and_then(|packages| {
                packages
                    .iter()
                    .find(|package| package["name"].as_str() == Some("oven-compiler-foundation"))
            })
            .ok_or_else(|| std::io::Error::other("foundation lock package"))?;
        assert_eq!(foundation["version"].as_str(), Some("0.0.0"));
        let locked_dependencies = foundation["dependencies"]
            .as_array()
            .ok_or_else(|| std::io::Error::other("foundation lock dependencies"))?;
        assert_eq!(locked_dependencies.len(), 1);
        assert_eq!(locked_dependencies[0].as_str(), Some("external-dep"));
        Ok(())
    }

    #[test]
    fn compiler_foundation_manifest_preserves_checked_in_third_party_patch_resolution()
    -> Result<(), Box<dyn std::error::Error>> {
        let compiler_root = tempfile::tempdir()?;
        let patch_root = compiler_root.path().join("crates/third_party/registry_patch");
        fs::create_dir_all(compiler_root.path().join("src"))?;
        fs::create_dir_all(patch_root.join("src"))?;
        fs::write(
            compiler_root.path().join("Cargo.toml"),
            "[package]\nname = \"compiler\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )?;
        fs::write(compiler_root.path().join("src/lib.rs"), "pub fn compiler() {}\n")?;
        fs::write(
            patch_root.join("Cargo.toml"),
            "[package]\nname = \"registry-patch\"\nversion = \"1.2.3\"\nedition = \"2024\"\n",
        )?;
        fs::write(patch_root.join("src/lib.rs"), "pub fn patched() {}\n")?;
        let graph = CargoUnitGraph {
            version: 1,
            units: vec![
                CargoUnitGraphUnit {
                    pkg_id: "compiler 0.1.0 (path+file:///compiler)".to_string(),
                    target: CargoUnitGraphTarget {
                        kind: vec!["lib".to_string()],
                        crate_types: vec!["lib".to_string()],
                        name: "compiler".to_string(),
                        src_path: compiler_root.path().join("src/lib.rs"),
                        edition: "2024".to_string(),
                    },
                    mode: "test".to_string(),
                    platform: None,
                    features: Vec::new(),
                    dependencies: vec![CargoUnitGraphDependency {
                        index: 1,
                        extern_crate_name: Some("registry_patch".to_string()),
                    }],
                },
                CargoUnitGraphUnit {
                    pkg_id: "registry-patch 1.2.3 (path+file:///registry-patch)".to_string(),
                    target: CargoUnitGraphTarget {
                        kind: vec!["lib".to_string()],
                        crate_types: vec!["lib".to_string()],
                        name: "registry_patch".to_string(),
                        src_path: patch_root.join("src/lib.rs"),
                        edition: "2024".to_string(),
                    },
                    mode: "build".to_string(),
                    platform: None,
                    features: vec!["default".to_string(), "alloc".to_string()],
                    dependencies: Vec::new(),
                },
            ],
            roots: vec![0],
        };
        let metadata = CargoMetadata {
            resolve: None,
            packages: vec![
                CargoMetadataPackage {
                    id: "compiler 0.1.0 (path+file:///compiler)".to_string(),
                    name: "compiler".to_string(),
                    version: "0.1.0".to_string(),
                    manifest_path: compiler_root.path().join("Cargo.toml"),
                    source: None,
                },
                CargoMetadataPackage {
                    id: "registry-patch 1.2.3 (path+file:///registry-patch)".to_string(),
                    name: "registry-patch".to_string(),
                    version: "1.2.3".to_string(),
                    manifest_path: patch_root.join("Cargo.toml"),
                    source: None,
                },
            ],
        };
        let dependencies = compiler_suite_foundation_dependencies(compiler_root.path(), &graph, &metadata)?;
        let canonical_patch_root = fs::canonicalize(&patch_root)?;
        assert_eq!(dependencies.len(), 1);
        assert_eq!(dependencies[0].package, "registry-patch");
        assert_eq!(dependencies[0].features, ["alloc", "default"]);
        assert_eq!(dependencies[0].path.as_deref(), Some(canonical_patch_root.as_path()));
        let manifest = compiler_suite_foundation_manifest(&dependencies)?;
        assert!(manifest.contains("[patch.crates-io]"));
        assert!(manifest.contains("registry-patch = { path ="));
        assert!(manifest.contains(&canonical_patch_root.display().to_string()));
        assert!(!manifest.contains("package = \"compiler\""));
        Ok(())
    }

    #[test]
    fn generated_loaf_lock_keeps_compiler_registry_versions_and_adds_local_packages()
    -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let component = project.path().join("component");
        let compiler_component = project.path().join("compiler-component");
        fs::create_dir_all(component.join("src"))?;
        fs::create_dir_all(compiler_component.join("src"))?;
        let root_manifest = concat!(
            "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n",
            "[dependencies]\nserde = \"1\"\ncomponent = { path = \"component\" }\n",
            "compiler_component = { package = \"compiler-component\", path = \"compiler-component\" }\n",
        );
        let root_manifest_path = project.path().join("Cargo.toml");
        fs::write(&root_manifest_path, root_manifest)?;
        fs::write(
            component.join("Cargo.toml"),
            "[package]\nname = \"component\"\nversion = \"0.2.0\"\nedition = \"2024\"\n\n[dependencies]\nserde = \"1\"\n",
        )?;
        fs::write(component.join("src/lib.rs"), "pub fn component() {}\n")?;
        fs::write(
            compiler_component.join("Cargo.toml"),
            "[package]\nname = \"compiler-component\"\nversion.workspace = true\nedition.workspace = true\n",
        )?;
        fs::write(
            compiler_component.join("src/lib.rs"),
            "pub fn compiler_component() {}\n",
        )?;
        let compiler_lock = br#"version = 4

[[package]]
name = "compiler-component"
version = "0.5.0"

[[package]]
name = "serde"
version = "0.9.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "old"

[[package]]
name = "serde"
version = "1.0.228"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "selected"
"#;

        let lock = locked_generated_project(&root_manifest_path, root_manifest.as_bytes(), compiler_lock)?;
        validate_generated_registry_lock(compiler_lock, &lock)?;
        let mismatched_lock =
            String::from_utf8(lock.clone())?.replace("checksum = \"selected\"", "checksum = \"drift\"");
        let Err(error) = validate_generated_registry_lock(compiler_lock, mismatched_lock.as_bytes()) else {
            return Err(std::io::Error::other("mismatched registry checksum was accepted").into());
        };
        assert!(error.to_string().contains("disagrees with the checked compiler lock"));
        let lock = toml::from_str::<toml::Value>(std::str::from_utf8(&lock)?)?;
        let packages = lock["package"]
            .as_array()
            .ok_or_else(|| std::io::Error::other("generated package array"))?;
        let fixture = packages
            .iter()
            .find(|package| package["name"].as_str() == Some("fixture"))
            .ok_or_else(|| std::io::Error::other("generated fixture lock root"))?;
        let component = packages
            .iter()
            .find(|package| package["name"].as_str() == Some("component"))
            .ok_or_else(|| std::io::Error::other("generated component lock package"))?;
        let compiler_component = packages
            .iter()
            .find(|package| package["name"].as_str() == Some("compiler-component"))
            .ok_or_else(|| std::io::Error::other("generated detached compiler component lock package"))?;
        let fixture_dependencies = fixture["dependencies"]
            .as_array()
            .ok_or_else(|| std::io::Error::other("generated fixture dependencies"))?;
        assert!(
            fixture_dependencies
                .iter()
                .any(|dependency| dependency.as_str() == Some("component 0.2.0"))
        );
        assert!(
            fixture_dependencies
                .iter()
                .any(|dependency| dependency.as_str() == Some("compiler-component 0.5.0"))
        );
        assert!(
            fixture_dependencies
                .iter()
                .any(|dependency| dependency.as_str() == Some("serde 1.0.228"))
        );
        assert_eq!(
            component["dependencies"]
                .as_array()
                .and_then(|dependencies| dependencies.first())
                .and_then(toml::Value::as_str),
            Some("serde 1.0.228")
        );
        assert_eq!(
            compiler_component["version"].as_str(),
            Some("0.5.0"),
            "a detached compiler-owned package retains the checked local lock version fallback"
        );
        assert_eq!(
            packages
                .iter()
                .filter(|package| package["name"].as_str() == Some("serde"))
                .count(),
            1,
            "unreachable compiler packages are pruned without changing the selected registry version"
        );
        Ok(())
    }

    #[test]
    fn generated_loaf_lock_qualifies_same_named_local_package_versions() -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let generated = project.path().join("generated");
        let foo_v1 = project.path().join("foo-v1");
        let foo_v2 = project.path().join("foo-v2");
        fs::create_dir_all(generated.join("src"))?;
        fs::create_dir_all(foo_v1.join("src"))?;
        fs::create_dir_all(foo_v2.join("src"))?;
        let generated_manifest = concat!(
            "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n",
            "[dependencies]\n",
            "foo_old = { package = \"foo\", path = \"../foo-v1\" }\n",
            "foo_new = { package = \"foo\", path = \"../foo-v2\" }\n",
        );
        let generated_manifest_path = generated.join("Cargo.toml");
        fs::write(&generated_manifest_path, generated_manifest)?;
        fs::write(generated.join("src/lib.rs"), "pub fn fixture() {}\n")?;
        fs::write(
            foo_v1.join("Cargo.toml"),
            "[package]\nname = \"foo\"\nversion = \"1.0.0\"\nedition = \"2024\"\n",
        )?;
        fs::write(foo_v1.join("src/lib.rs"), "pub fn foo_v1() {}\n")?;
        fs::write(
            foo_v2.join("Cargo.toml"),
            "[package]\nname = \"foo\"\nversion = \"2.0.0\"\nedition = \"2024\"\n",
        )?;
        fs::write(foo_v2.join("src/lib.rs"), "pub fn foo_v2() {}\n")?;
        let compiler_lock = br#"version = 4

[[package]]
name = "compiler-only"
version = "1.0.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "compiler"
"#;

        let lock = locked_generated_project(&generated_manifest_path, generated_manifest.as_bytes(), compiler_lock)?;
        let lock = toml::from_slice::<toml::Value>(&lock)?;
        let packages = lock["package"]
            .as_array()
            .ok_or_else(|| std::io::Error::other("generated package array"))?;
        let fixture = packages
            .iter()
            .find(|package| package["name"].as_str() == Some("fixture"))
            .ok_or_else(|| std::io::Error::other("generated fixture lock root"))?;
        let dependencies = fixture["dependencies"]
            .as_array()
            .ok_or_else(|| std::io::Error::other("generated fixture dependencies"))?
            .iter()
            .filter_map(toml::Value::as_str)
            .collect::<Vec<_>>();
        assert_eq!(dependencies, ["foo 1.0.0", "foo 2.0.0"]);
        assert_eq!(
            packages
                .iter()
                .filter(|package| package["name"].as_str() == Some("foo"))
                .count(),
            2
        );
        Ok(())
    }

    #[test]
    fn generated_lock_inherits_nearest_workspace_package_and_dependencies() -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let workspace = project.path().join("vendor");
        let member = workspace.join("member");
        let support = workspace.join("support");
        let generated = project.path().join("generated");
        fs::create_dir_all(member.join("src"))?;
        fs::create_dir_all(support.join("src"))?;
        fs::create_dir_all(generated.join("src"))?;
        let workspace_manifest_path = workspace.join("Cargo.toml");
        fs::write(
            &workspace_manifest_path,
            concat!(
                "[workspace]\nmembers = [\"member\", \"support\"]\n\n",
                "[workspace.package]\nversion = \"1.2.3\"\n\n",
                "[workspace.dependencies]\n",
                "serde = { version = \"=1.0.228\", features = [\"derive\"] }\n",
                "support = { path = \"support\" }\n",
            ),
        )?;
        let member_manifest = concat!(
            "[package]\nname = \"member\"\nversion.workspace = true\nedition = \"2024\"\n\n",
            "[dependencies]\n",
            "serde = { workspace = true, features = [\"alloc\"] }\n",
            "support.workspace = true\n",
        );
        let member_manifest_path = member.join("Cargo.toml");
        fs::write(&member_manifest_path, member_manifest)?;
        fs::write(member.join("src/lib.rs"), "pub fn member() {}\n")?;
        fs::write(
            support.join("Cargo.toml"),
            "[package]\nname = \"support\"\nversion = \"0.4.0\"\nedition = \"2024\"\n",
        )?;
        fs::write(support.join("src/lib.rs"), "pub fn support() {}\n")?;
        let generated_manifest = concat!(
            "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n",
            "[dependencies]\nmember = { path = \"../vendor/member\" }\n",
        );
        let generated_manifest_path = generated.join("Cargo.toml");
        fs::write(&generated_manifest_path, generated_manifest)?;
        fs::write(generated.join("src/lib.rs"), "pub fn fixture() {}\n")?;
        let compiler_lock = br#"version = 4

[[package]]
name = "member"
version = "1.2.3"
dependencies = [
 "serde 1.0.228",
]

[[package]]
name = "serde"
version = "1.0.228"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "first"

[[package]]
name = "serde"
version = "1.0.229"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "second"
"#;

        let first_authority = digest_local_cargo_workspace_authority(&member)?
            .ok_or_else(|| std::io::Error::other("first inherited workspace authority digest"))?;
        let first = locked_generated_project(&generated_manifest_path, generated_manifest.as_bytes(), compiler_lock)?;
        let first_lock = toml::from_slice::<toml::Value>(&first)?;
        let first_packages = first_lock["package"]
            .as_array()
            .ok_or_else(|| std::io::Error::other("first generated workspace package array"))?;
        let first_member = first_packages
            .iter()
            .find(|package| package["name"].as_str() == Some("member"))
            .ok_or_else(|| std::io::Error::other("first generated workspace member"))?;
        assert_eq!(first_member["version"].as_str(), Some("1.2.3"));
        assert!(
            first_packages
                .iter()
                .any(|package| package["name"].as_str() == Some("support")),
            "workspace-relative path dependency was not added to the effective local graph"
        );
        assert!(
            first_packages.iter().any(|package| {
                package["name"].as_str() == Some("serde") && package["version"].as_str() == Some("1.0.228")
            }),
            "workspace-selected registry dependency was not retained"
        );

        fs::write(
            &workspace_manifest_path,
            concat!(
                "[workspace]\nmembers = [\"member\", \"support\"]\n\n",
                "[workspace.package]\nversion = \"1.2.3\"\n\n",
                "[workspace.dependencies]\n",
                "serde = { version = \"=1.0.229\", features = [\"derive\"] }\n",
                "support = { path = \"support\" }\n",
            ),
        )?;
        let second_authority = digest_local_cargo_workspace_authority(&member)?
            .ok_or_else(|| std::io::Error::other("second inherited workspace authority digest"))?;
        assert_ne!(
            first_authority, second_authority,
            "selected workspace dependency authority must change the normal source identity"
        );
        let second = locked_generated_project(&generated_manifest_path, generated_manifest.as_bytes(), compiler_lock)?;
        assert_ne!(
            first, second,
            "selected workspace dependency authority must change the generated lock identity"
        );
        let second_lock = toml::from_slice::<toml::Value>(&second)?;
        let second_packages = second_lock["package"]
            .as_array()
            .ok_or_else(|| std::io::Error::other("second generated workspace package array"))?;
        assert!(
            second_packages.iter().any(|package| {
                package["name"].as_str() == Some("serde") && package["version"].as_str() == Some("1.0.229")
            }),
            "updated workspace registry authority was not selected"
        );
        Ok(())
    }

    #[test]
    fn generated_lock_reports_missing_workspace_authority_without_compiler_lock_fallback()
    -> Result<(), Box<dyn std::error::Error>> {
        let workspace = tempfile::tempdir()?;
        let member = workspace.path().join("member");
        fs::create_dir_all(member.join("src"))?;
        let workspace_manifest_path = workspace.path().join("Cargo.toml");
        fs::write(&workspace_manifest_path, "[workspace]\nmembers = [\"member\"]\n")?;
        let member_manifest = "[package]\nname = \"member\"\nversion.workspace = true\n";
        let member_manifest_path = member.join("Cargo.toml");
        fs::write(&member_manifest_path, member_manifest)?;
        fs::write(member.join("src/lib.rs"), "pub fn member() {}\n")?;
        let compiler_lock = b"version = 4\n\n[[package]]\nname = \"member\"\nversion = \"9.9.9\"\n";

        let Err(error) = locked_generated_project(&member_manifest_path, member_manifest.as_bytes(), compiler_lock)
        else {
            return Err(std::io::Error::other("missing workspace package version was accepted").into());
        };
        let workspace_manifest_path = fs::canonicalize(&workspace_manifest_path)?;
        let diagnostic = error.to_string();
        assert!(diagnostic.contains(&workspace_manifest_path.display().to_string()));
        assert!(diagnostic.contains("[workspace.package].version"));
        assert!(
            !diagnostic.contains("9.9.9"),
            "a containing workspace must not fall back to a compiler-lock coincidence"
        );

        fs::write(
            &workspace_manifest_path,
            "[workspace]\nmembers = [\"member\"]\n\n[workspace.package]\nversion = \"1.2.3\"\n",
        )?;
        let member_manifest = concat!(
            "[package]\nname = \"member\"\nversion.workspace = true\n\n",
            "[dependencies]\nserde.workspace = true\n",
        );
        fs::write(&member_manifest_path, member_manifest)?;
        let Err(error) = locked_generated_project(&member_manifest_path, member_manifest.as_bytes(), compiler_lock)
        else {
            return Err(std::io::Error::other("missing workspace dependency was accepted").into());
        };
        let diagnostic = error.to_string();
        assert!(diagnostic.contains(&workspace_manifest_path.display().to_string()));
        assert!(diagnostic.contains("[workspace.dependencies].serde"));
        Ok(())
    }

    #[test]
    fn release_cohort_lock_pins_overlap_while_permitting_project_only_packages()
    -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let manifest_path = project.path().join("Cargo.toml");
        let manifest = concat!(
            "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n",
            "[dependencies]\nserde = \"1\"\nproject-only = \"2\"\n",
        );
        fs::write(&manifest_path, manifest)?;
        let release_lock = br#"version = 4

[[package]]
name = "serde"
version = "1.0.228"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "release-serde"
"#;

        let seed = release_cohort_generated_project_lock(&manifest_path, manifest.as_bytes(), release_lock)?;
        let seed_text = std::str::from_utf8(&seed)?;
        assert!(seed_text.contains("name = \"serde\""));
        assert!(seed_text.contains("version = \"1.0.228\""));
        assert!(!seed_text.contains("project-only"));

        let normalized = br#"version = 4

[[package]]
name = "fixture"
version = "0.1.0"
dependencies = [
 "project-only",
 "serde",
]

[[package]]
name = "project-only"
version = "2.4.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "project-only-checksum"

[[package]]
name = "serde"
version = "1.0.228"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "release-serde"
"#;
        validate_release_cohort_registry_lock(release_lock, &seed, normalized)?;

        let tampered = String::from_utf8(normalized.to_vec())?.replace("release-serde", "tampered");
        let Err(error) = validate_release_cohort_registry_lock(release_lock, &seed, tampered.as_bytes()) else {
            return Err(std::io::Error::other("tampered release-owned checksum was accepted").into());
        };
        assert!(error.to_string().contains("changed the release-derived checksum"));
        Ok(())
    }

    #[test]
    fn release_cohort_lock_allows_cargo_to_prune_an_unreachable_release_node() -> Result<(), Box<dyn std::error::Error>>
    {
        let release_lock = br#"version = 4

[[package]]
name = "atomic-waker"
version = "1.1.2"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "release-atomic-waker"
"#;
        let seeded_lock = br#"version = 4

[[package]]
name = "fixture"
version = "0.1.0"

[[package]]
name = "atomic-waker"
version = "1.1.2"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "release-atomic-waker"
"#;
        let generated_lock = br#"version = 4

[[package]]
name = "fixture"
version = "0.1.0"
"#;

        validate_release_cohort_registry_lock(release_lock, seeded_lock, generated_lock)?;
        Ok(())
    }

    #[test]
    fn release_cohort_lock_allows_project_only_versions_and_registry_edge_normalization()
    -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let manifest_path = project.path().join("Cargo.toml");
        let manifest = concat!(
            "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n",
            "[dependencies]\nshared = \"1\"\ndatafusion-shaped = \"1\"\n",
        );
        fs::write(&manifest_path, manifest)?;
        let release_lock = br#"version = 4

[[package]]
name = "shared"
version = "1.0.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "release-shared"
dependencies = ["transitive"]

[[package]]
name = "transitive"
version = "1.0.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "release-transitive"
"#;
        let seed = release_cohort_generated_project_lock(&manifest_path, manifest.as_bytes(), release_lock)?;
        let normalized = br#"version = 4

[[package]]
name = "fixture"
version = "0.1.0"
dependencies = [
 "datafusion-shaped",
 "shared",
]

[[package]]
name = "datafusion-shaped"
version = "1.0.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "project-datafusion"
dependencies = ["transitive 2.0.0"]

[[package]]
name = "shared"
version = "1.0.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "release-shared"
dependencies = ["transitive 1.0.0"]

[[package]]
name = "transitive"
version = "1.0.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "release-transitive"

[[package]]
name = "transitive"
version = "2.0.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "project-transitive"
"#;

        validate_release_cohort_registry_lock(release_lock, &seed, normalized)?;

        let normalized_registry_edges = String::from_utf8(normalized.to_vec())?.replace(
            "dependencies = [\"transitive 1.0.0\"]",
            "dependencies = [\"transitive 2.0.0\"]",
        );
        validate_release_cohort_registry_lock(release_lock, &seed, normalized_registry_edges.as_bytes())?;
        Ok(())
    }

    #[test]
    fn release_cohort_lock_allows_pruned_local_edges_but_rejects_new_release_identity_edges()
    -> Result<(), Box<dyn std::error::Error>> {
        let release_lock = br#"version = 4

[[package]]
name = "release-local"
version = "1.0.0"
dependencies = ["shared"]

[[package]]
name = "shared"
version = "1.0.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "release-shared"
"#;
        let seeded_lock = br#"version = 4

[[package]]
name = "fixture"
version = "0.1.0"
dependencies = ["release-local"]

[[package]]
name = "release-local"
version = "1.0.0"
dependencies = ["shared"]

[[package]]
name = "shared"
version = "1.0.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "release-shared"
"#;
        let generated_lock = br#"version = 4

[[package]]
name = "fixture"
version = "0.1.0"
dependencies = ["release-local"]

[[package]]
name = "project-only"
version = "1.0.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "project-only"

[[package]]
name = "release-local"
version = "1.0.0"
dependencies = ["project-only", "shared"]

[[package]]
name = "shared"
version = "1.0.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "release-shared"
"#;

        let pruned_local_lock = br#"version = 4

[[package]]
name = "fixture"
version = "0.1.0"
dependencies = ["release-local"]

[[package]]
name = "release-local"
version = "1.0.0"
"#;
        validate_release_cohort_registry_lock(release_lock, seeded_lock, pruned_local_lock)?;

        let Err(error) = validate_release_cohort_registry_lock(release_lock, seeded_lock, generated_lock) else {
            return Err(std::io::Error::other("new edge on a retained local release package was accepted").into());
        };
        assert!(
            error
                .to_string()
                .contains("changed the release-derived dependency edges for `release-local` 1.0.0")
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn explicit_project_inspection_reuses_the_release_lock_metadata_walk() -> Result<(), Box<dyn std::error::Error>> {
        use std::os::unix::fs::PermissionsExt;

        let project = tempfile::tempdir()?;
        let manifest = project.path().join("Cargo.toml");
        fs::write(
            &manifest,
            "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[dependencies]\nlocal = { path = \"local\" }\n",
        )?;
        let local = project.path().join("local");
        fs::create_dir_all(local.join("src"))?;
        fs::write(
            local.join("Cargo.toml"),
            "[package]\nname = \"local\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[dependencies]\nhidden = \"1\"\n",
        )?;
        fs::write(local.join("src/lib.rs"), "pub fn local() {}\n")?;
        let registry_source = "registry+https://example.invalid/index";
        let checksum = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let hidden = project.path().join("registry-hidden");
        fs::create_dir_all(hidden.join("src"))?;
        fs::write(
            hidden.join("Cargo.toml"),
            "[package]\nname = \"hidden\"\nversion = \"1.0.0\"\nedition = \"2024\"\n",
        )?;
        fs::write(hidden.join("src/lib.rs"), "pub fn hidden() {}\n")?;
        let release_lock = project.path().join("release-Cargo.lock");
        fs::write(
            &release_lock,
            format!(
                "version = 4\n\n[[package]]\nname = \"hidden\"\nversion = \"1.0.0\"\nsource = \"{registry_source}\"\nchecksum = \"{checksum}\"\n"
            ),
        )?;
        let metadata = project.path().join("metadata.json");
        fs::write(
            &metadata,
            serde_json::to_vec(&serde_json::json!({
                "packages": [{
                    "id": "fixture 0.1.0",
                    "name": "fixture",
                    "version": "0.1.0",
                    "manifest_path": manifest,
                }, {
                    "id": "local 0.1.0",
                    "name": "local",
                    "version": "0.1.0",
                    "manifest_path": local.join("Cargo.toml"),
                }, {
                    "id": "hidden 1.0.0",
                    "name": "hidden",
                    "version": "1.0.0",
                    "manifest_path": hidden.join("Cargo.toml"),
                    "source": registry_source,
                }],
                "resolve": {
                    "root": "fixture 0.1.0",
                    "nodes": [{
                        "id": "fixture 0.1.0",
                        "dependencies": ["local 0.1.0"],
                        "deps": [{ "name": "local", "pkg": "local 0.1.0" }],
                    }, {
                        "id": "local 0.1.0",
                        "dependencies": ["hidden 1.0.0"],
                        "deps": [{ "name": "hidden", "pkg": "hidden 1.0.0" }],
                    }, {
                        "id": "hidden 1.0.0",
                        "dependencies": [],
                        "deps": [],
                    }],
                },
            }))?,
        )?;
        let marker = project.path().join("metadata-invocations");
        let cargo = project.path().join("cargo");
        fs::write(
            &cargo,
            format!(
                "#!/bin/sh\nprintf x >> '{}'\ncat '{}'\n",
                marker.display(),
                metadata.display()
            ),
        )?;
        fs::set_permissions(&cargo, fs::Permissions::from_mode(0o755))?;
        let staging = tempfile::tempdir()?;

        let sources =
            explicit_project_bake_inspection_sources(&cargo, &manifest, &[], &[], staging.path(), Some(&release_lock))?;

        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0].package, "hidden");
        assert_eq!(sources[0].checksum, checksum);
        assert_eq!(
            fs::read(&marker)?,
            b"x",
            "metadata must run once per explicit inspection workspace"
        );
        Ok(())
    }

    #[test]
    fn compiler_suite_source_footprint_is_bound_to_receipt_evidence() -> Result<(), Box<dyn std::error::Error>> {
        let compiler_root = tempfile::tempdir()?;
        fs::create_dir_all(compiler_root.path().join("src"))?;
        fs::write(
            compiler_root.path().join("Cargo.toml"),
            "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )?;
        fs::write(compiler_root.path().join("Cargo.lock"), "version = 4\n")?;
        let source_text = "pub fn fixture() {}\n";
        fs::write(compiler_root.path().join("src/lib.rs"), source_text)?;
        fs::write(compiler_root.path().join("src/main.rs"), "fn main() {}\n")?;
        let receipt = receipt_native_compiler_suite(&OvenCompilerSuiteRequest::new(
            compiler_root.path(),
            "aarch64-apple-darwin",
            "rustc fixture",
            "debug",
            Vec::new(),
        ))?;
        let target = OvenCompilerTestSuiteTarget {
            package_name: "fixture".to_string(),
            target_name: "fixture".to_string(),
            target_kind: "lib".to_string(),
            runner: "rustc-test".to_string(),
            source_relative_path: "src/lib.rs".to_string(),
            source_evidence_key: "compiler-suite-source:src/lib.rs".to_string(),
            crate_name: "fixture".to_string(),
            edition: "2024".to_string(),
            features: Vec::new(),
            compile_environment: BTreeMap::new(),
            binary_dependencies: Vec::new(),
            workspace_library_dependencies: Vec::new(),
            externs: Vec::new(),
        };

        assert_eq!(
            compiler_suite_verified_target_source_bytes(compiler_root.path(), &receipt, &target)?,
            u64::try_from(source_text.len())?
        );

        fs::write(compiler_root.path().join("src/lib.rs"), "pub fn changed() {}\n")?;
        let error = compiler_suite_verified_target_source_bytes(compiler_root.path(), &receipt, &target)
            .err()
            .ok_or("receipt-mismatched source footprint unexpectedly succeeded")?;
        assert!(error.to_string().contains("does not match its receipt evidence"));
        Ok(())
    }

    #[test]
    fn compiler_target_plan_keeps_workspace_library_edges_out_of_the_cargo_artifact_closure()
    -> Result<(), Box<dyn std::error::Error>> {
        let compiler_root = tempfile::tempdir()?;
        let helper_root = compiler_root.path().join("crates/helper");
        fs::create_dir_all(compiler_root.path().join("src"))?;
        fs::create_dir_all(helper_root.join("src"))?;
        fs::write(
            compiler_root.path().join("Cargo.toml"),
            "[package]\nname = \"compiler\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )?;
        fs::write(compiler_root.path().join("Cargo.lock"), "version = 4\n")?;
        fs::write(compiler_root.path().join("src/lib.rs"), "pub fn compiler() {}\n")?;
        fs::write(compiler_root.path().join("src/main.rs"), "fn main() {}\n")?;
        fs::write(
            helper_root.join("Cargo.toml"),
            "[package]\nname = \"helper\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )?;
        fs::write(helper_root.join("src/lib.rs"), "pub fn helper() {}\n")?;
        let receipt = receipt_native_compiler_suite(&OvenCompilerSuiteRequest::new(
            compiler_root.path(),
            "aarch64-apple-darwin",
            "rustc fixture",
            "debug",
            Vec::new(),
        ))?;
        let graph = CargoUnitGraph {
            version: 1,
            units: vec![
                CargoUnitGraphUnit {
                    pkg_id: "helper 0.1.0 (path+file:///helper)".to_string(),
                    target: CargoUnitGraphTarget {
                        kind: vec!["lib".to_string()],
                        crate_types: vec!["lib".to_string()],
                        name: "helper".to_string(),
                        src_path: helper_root.join("src/lib.rs"),
                        edition: "2024".to_string(),
                    },
                    mode: "build".to_string(),
                    platform: Some("aarch64-apple-darwin".to_string()),
                    features: vec!["serde".to_string()],
                    dependencies: Vec::new(),
                },
                CargoUnitGraphUnit {
                    pkg_id: "compiler 0.1.0 (path+file:///compiler)".to_string(),
                    target: CargoUnitGraphTarget {
                        kind: vec!["lib".to_string()],
                        crate_types: vec!["lib".to_string()],
                        name: "compiler".to_string(),
                        src_path: compiler_root.path().join("src/lib.rs"),
                        edition: "2024".to_string(),
                    },
                    mode: "test".to_string(),
                    platform: Some("aarch64-apple-darwin".to_string()),
                    features: Vec::new(),
                    dependencies: vec![CargoUnitGraphDependency {
                        index: 0,
                        extern_crate_name: Some("helper".to_string()),
                    }],
                },
            ],
            roots: vec![1],
        };
        let empty_catalog = CompilerSuiteArtifactCatalog {
            closure: OvenCompilerTestSuiteArtifactClosure {
                dependency_search_paths: Vec::new(),
                native_search_paths: Vec::new(),
                supporting_artifacts: Vec::new(),
            },
            materialized_files: Vec::new(),
            by_source_path: Default::default(),
        };
        let target = compiler_suite_target_from_unit(
            compiler_root.path(),
            &receipt,
            &graph.units[1],
            &graph,
            &Default::default(),
            &empty_catalog,
        )?;
        assert!(target.externs.is_empty());
        assert_eq!(target.workspace_library_dependencies.len(), 1);
        assert_eq!(target.workspace_library_dependencies[0].package_name, "helper");
        assert_eq!(target.workspace_library_dependencies[0].crate_name, "helper");
        assert_eq!(target.workspace_library_dependencies[0].target_kind, "lib");
        assert_eq!(
            target.workspace_library_dependencies[0].source_relative_path,
            "crates/helper/src/lib.rs"
        );
        assert_eq!(target.workspace_library_dependencies[0].features, ["serde"]);
        let workspace_libraries = compiler_suite_workspace_libraries_for_roots(
            compiler_root.path(),
            &receipt,
            &graph,
            &[1],
            &Default::default(),
            &empty_catalog,
        )?;
        assert_eq!(workspace_libraries.len(), 1);
        assert_eq!(workspace_libraries[0].key, target.workspace_library_dependencies[0]);
        assert_eq!(
            workspace_libraries[0].source_evidence_key,
            "compiler-suite-source:crates/helper/src/lib.rs"
        );
        assert!(workspace_libraries[0].externs.is_empty());
        assert!(workspace_libraries[0].dependencies.is_empty());
        Ok(())
    }

    #[test]
    fn compiler_foundation_partition_is_deterministic_and_leaves_manifest_headroom()
    -> Result<(), Box<dyn std::error::Error>> {
        let files = tempfile::tempdir()?;
        let first = files.path().join("a.rlib");
        let second = files.path().join("b.rlib");
        fs::write(&first, vec![b'a'; 30_000])?;
        fs::write(&second, vec![b'b'; 30_000])?;
        let closure = OvenCompilerTestSuiteArtifactClosure {
            dependency_search_paths: vec!["deps".to_string()],
            native_search_paths: Vec::new(),
            supporting_artifacts: vec![
                OvenRustcSupportingArtifact {
                    relative_path: "deps/a.rlib".to_string(),
                    digest: "sha256:a".to_string(),
                },
                OvenRustcSupportingArtifact {
                    relative_path: "deps/b.rlib".to_string(),
                    digest: "sha256:b".to_string(),
                },
            ],
        };
        let plans = compiler_suite_foundation_plans(
            &closure,
            &[
                OvenArtifactMaterializedFile {
                    source_path: first,
                    relative_path: "deps/a.rlib".to_string(),
                },
                OvenArtifactMaterializedFile {
                    source_path: second,
                    relative_path: "deps/b.rlib".to_string(),
                },
            ],
            100_000,
        )?;

        assert_eq!(plans.len(), 2);
        assert_eq!(plans[0].payload.label, "foundation-0000");
        assert_eq!(plans[1].payload.label, "foundation-0001");
        assert_eq!(plans[0].materialized_files[0].relative_path, "deps/a.rlib");
        assert_eq!(plans[1].materialized_files[0].relative_path, "deps/b.rlib");
        assert!(plans.iter().all(|plan| plan.materialized_files.len() == 1));
        Ok(())
    }

    #[test]
    fn publisher_materializes_compiler_loaf_data_below_its_own_prefix() -> Result<(), Box<dyn std::error::Error>> {
        let toolchain = tempfile::tempdir()?;
        let loafs = toolchain.path().join("share/incan/oven/loafs");
        let loaf = loafs.join("fixture.loaf/loaf.json");
        fs::create_dir_all(loaf.parent().ok_or("Loaf parent missing")?)?;
        fs::write(&loaf, "sealed Loaf")?;

        let files = materialized_files_from_directory(
            &loafs,
            "toolchain-data/share/incan/oven/loafs",
            "compiler-owned Loaf data",
        )?;

        assert_eq!(files.len(), 1);
        assert_eq!(
            files[0].relative_path,
            "toolchain-data/share/incan/oven/loafs/fixture.loaf/loaf.json"
        );
        Ok(())
    }

    #[test]
    fn staged_registry_source_excludes_mutable_package_target_output() -> Result<(), Box<dyn std::error::Error>> {
        let source = tempfile::tempdir()?;
        let staging = tempfile::tempdir()?;
        fs::create_dir_all(source.path().join("src"))?;
        fs::create_dir_all(source.path().join("target/debug"))?;
        fs::write(
            source.path().join("Cargo.toml"),
            "[package]\nname = \"fixture\"\nversion = \"1.0.0\"\n",
        )?;
        fs::write(source.path().join("src/lib.rs"), "pub fn fixture() {}\n")?;
        fs::write(
            source.path().join("target/debug/libfixture.rlib"),
            "mutable cache output",
        )?;

        let (staged, first_digest) = stage_registry_source_directory(
            staging.path(),
            "fixture",
            "1.0.0",
            "registry+https://example.invalid/index",
            "fixture-checksum",
            source.path(),
        )?;

        assert!(staged.join("Cargo.toml").is_file());
        assert!(staged.join("src/lib.rs").is_file());
        assert!(!staged.join("target").exists());
        fs::write(source.path().join("target/debug/another.rlib"), "more mutable output")?;
        assert_eq!(first_digest, digest_source_tree(&staged)?);
        Ok(())
    }

    #[test]
    fn compiler_suite_toolchain_loaf_generation_covers_debug_and_release_variants()
    -> Result<(), Box<dyn std::error::Error>> {
        let toolchain = tempfile::tempdir()?;
        let loafs = toolchain.path().join("share/incan/oven/loafs");
        let mut members = Vec::new();
        for (name, profile) in [("debug", "debug"), ("release", "release")] {
            let loaf = OvenLoaf {
                schema_version: OVEN_LOAF_SCHEMA_VERSION,
                build_unit_identity: format!("sha256:{name}"),
                provenance: Default::default(),
                accounting: Default::default(),
                compatibility: Default::default(),
                registry_leaves: Vec::new(),
                plan: OvenRustcArtifactManifest {
                    schema_version: OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION,
                    intent: OvenBuildIntent {
                        target: "fixture-target".to_string(),
                        toolchain: "fixture-rustc".to_string(),
                        profile: profile.to_string(),
                        features: Vec::new(),
                    },
                    dependency_search_paths: Vec::new(),
                    native_search_paths: Vec::new(),
                    externs: Vec::new(),
                    entrypoint_externs: Default::default(),
                    registry_leaves: Vec::new(),
                    registry_sources: Vec::new(),
                    compile_environment: Default::default(),
                    vocab_auxiliary_targets: Vec::new(),
                    supporting_artifacts: Vec::new(),
                },
            };
            let loaf_identity = crate::oven::digest_bytes(&serde_json::to_vec_pretty(&loaf)?);
            let relative = PathBuf::from(format!(
                "generations/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/{}.loaf/loaf.json",
                loaf_identity.strip_prefix("sha256:").unwrap_or(&loaf_identity)
            ));
            let loaf_path = loafs.join(&relative);
            fs::create_dir_all(loaf_path.parent().ok_or("Loaf parent missing")?)?;
            fs::write(loaf_path, serde_json::to_vec_pretty(&loaf)?)?;
            members.push(OvenLoafEnvelopeMember {
                label: name.to_string(),
                profile: profile.to_string(),
                action: "run".to_string(),
                role: OvenLoafMemberRole::CompiledClosure,
                build_unit_identity: format!("sha256:{name}"),
                loaf_identity,
                plan_identity: crate::oven::digest_bytes(&serde_json::to_vec(&loaf.plan)?),
                logical_bytes: serde_json::to_vec_pretty(&loaf)?.len() as u64,
                physical_bytes: 0,
                path: relative,
            });
        }
        write_test_loaf_envelope(&loafs, members)?;

        let reference = compiler_suite_toolchain_loaf_generation_reference(&loafs, &BTreeMap::new())?;
        assert_eq!(
            reference.generation_identity,
            "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        );

        // Schema-13 data copies remain readable only for already-stored suite entries. New publication records the
        // generation reference above and does not publish these files again.
        let plans = compiler_suite_toolchain_data_plans(toolchain.path(), 1024 * 1024, &BTreeMap::new())?;
        let paths = plans
            .into_iter()
            .flat_map(|plan| plan.materialized_files)
            .map(|file| file.relative_path)
            .collect::<Vec<_>>();

        assert_eq!(paths.iter().filter(|path| path.ends_with("loaf.json")).count(), 2);
        Ok(())
    }

    #[test]
    fn compiler_suite_toolchain_data_rejects_loaf_for_a_different_sealed_runtime()
    -> Result<(), Box<dyn std::error::Error>> {
        let toolchain = tempfile::tempdir()?;
        let loafs = toolchain.path().join("share/incan/oven/loafs");
        let mut loaf = OvenLoaf {
            schema_version: OVEN_LOAF_SCHEMA_VERSION,
            build_unit_identity: "sha256:fixture".to_string(),
            provenance: Default::default(),
            accounting: Default::default(),
            compatibility: Default::default(),
            registry_leaves: Vec::new(),
            plan: OvenRustcArtifactManifest {
                schema_version: OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION,
                intent: OvenBuildIntent {
                    target: "fixture-target".to_string(),
                    toolchain: "fixture-rustc".to_string(),
                    profile: "debug".to_string(),
                    features: Vec::new(),
                },
                dependency_search_paths: Vec::new(),
                native_search_paths: Vec::new(),
                externs: Vec::new(),
                entrypoint_externs: Default::default(),
                registry_leaves: Vec::new(),
                registry_sources: Vec::new(),
                compile_environment: Default::default(),
                vocab_auxiliary_targets: Vec::new(),
                supporting_artifacts: Vec::new(),
            },
        };
        loaf.compatibility
            .runtime_inputs
            .insert("runtime-lock".to_string(), "sha256:old".to_string());
        let loaf_identity = crate::oven::digest_bytes(&serde_json::to_vec_pretty(&loaf)?);
        let relative = PathBuf::from(format!(
            "generations/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/{}.loaf/loaf.json",
            loaf_identity.strip_prefix("sha256:").unwrap_or(&loaf_identity)
        ));
        let loaf_path = loafs.join(&relative);
        fs::create_dir_all(loaf_path.parent().ok_or("Loaf parent missing")?)?;
        fs::write(&loaf_path, serde_json::to_vec_pretty(&loaf)?)?;
        write_test_loaf_envelope(
            &loafs,
            vec![OvenLoafEnvelopeMember {
                label: "fixture".to_string(),
                profile: "debug".to_string(),
                action: "run".to_string(),
                role: OvenLoafMemberRole::CompiledClosure,
                build_unit_identity: "sha256:fixture".to_string(),
                loaf_identity,
                plan_identity: crate::oven::digest_bytes(&serde_json::to_vec(&loaf.plan)?),
                logical_bytes: serde_json::to_vec_pretty(&loaf)?.len() as u64,
                physical_bytes: 0,
                path: relative,
            }],
        )?;
        let expected = BTreeMap::from([("runtime-lock".to_string(), "sha256:new".to_string())]);

        let error = match compiler_suite_toolchain_loaf_generation_reference(&loafs, &expected) {
            Ok(_) => return Err("a Loaf from another staged SDK runtime must be refused".into()),
            Err(error) => error,
        };

        assert!(
            error
                .to_string()
                .contains("runtime-lock: expected sha256:new, found sha256:old")
        );
        assert!(
            error
                .to_string()
                .contains("regenerate it through the internal compatibility publisher")
        );
        Ok(())
    }

    #[test]
    fn publisher_reclaims_cargo_only_target_files_before_store_materialization()
    -> Result<(), Box<dyn std::error::Error>> {
        let staging = tempfile::tempdir()?;
        let target = staging.path().join("target");
        let retained = target.join("aarch64-apple-darwin/debug/deps/libfixture.rlib");
        let discarded_object = target.join("aarch64-apple-darwin/debug/deps/fixture.o");
        let discarded_dep_info = target.join("aarch64-apple-darwin/debug/deps/fixture.d");
        let discarded_profile_file = target.join("aarch64-apple-darwin/debug/fixture");
        fs::create_dir_all(retained.parent().ok_or("retained parent missing")?)?;
        fs::write(&retained, "retained direct-rustc artifact")?;
        fs::write(&discarded_object, "Cargo object")?;
        fs::write(&discarded_dep_info, "Cargo dep-info")?;
        fs::write(&discarded_profile_file, "Cargo executable")?;

        reclaim_unmaterialized_compiler_suite_target_files(
            &target,
            &[OvenArtifactMaterializedFile {
                source_path: retained.clone(),
                relative_path: "target/aarch64-apple-darwin/debug/deps/libfixture.rlib".to_string(),
            }],
        )?;

        assert!(retained.is_file());
        assert!(!discarded_object.exists());
        assert!(!discarded_dep_info.exists());
        assert!(!discarded_profile_file.exists());
        Ok(())
    }

    #[test]
    fn publisher_stages_a_shard_before_reclaiming_its_transient_target() -> Result<(), Box<dyn std::error::Error>> {
        let staging = tempfile::tempdir()?;
        let source = staging.path().join("selection-target/debug/deps/libfixture.rlib");
        fs::create_dir_all(source.parent().ok_or("artifact parent missing")?)?;
        fs::write(&source, "verified direct-rustc input")?;
        let materialized = vec![OvenArtifactMaterializedFile {
            source_path: source.clone(),
            relative_path: "target/debug/deps/libfixture.rlib".to_string(),
        }];

        let staged = stage_compiler_suite_shard_files(staging.path(), 7, &materialized)?;

        assert_eq!(staged.len(), 1);
        assert_eq!(staged[0].relative_path, materialized[0].relative_path);
        assert!(
            staged[0]
                .source_path
                .starts_with(staging.path().join("prepared-shards/0007"))
        );
        fs::remove_file(source)?;
        assert_eq!(fs::read(&staged[0].source_path)?, b"verified direct-rustc input");
        assert!(fs::symlink_metadata(&staged[0].source_path)?.is_file());
        Ok(())
    }

    #[test]
    fn publisher_reuses_only_a_complete_current_schema_suite() -> Result<(), Box<dyn std::error::Error>> {
        let compiler_root = tempfile::tempdir()?;
        fs::create_dir_all(compiler_root.path().join("src"))?;
        fs::write(
            compiler_root.path().join("Cargo.toml"),
            "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )?;
        fs::write(compiler_root.path().join("Cargo.lock"), "version = 4\n")?;
        fs::write(compiler_root.path().join("src/lib.rs"), "pub fn fixture() {}\n")?;
        fs::write(compiler_root.path().join("src/main.rs"), "fn main() {}\n")?;
        let receipt = receipt_native_compiler_suite(&OvenCompilerSuiteRequest::new(
            compiler_root.path(),
            "aarch64-apple-darwin",
            "rustc fixture",
            "debug",
            Vec::new(),
        ))?;
        let store_root = tempfile::tempdir()?;
        let store = OvenStore::new(store_root.path(), OvenStoreLimits::new(1_000_000, 1_000_000, 1_000_000));
        let target = OvenCompilerTestSuiteTarget {
            package_name: "fixture".to_string(),
            target_name: "fixture".to_string(),
            target_kind: "lib".to_string(),
            runner: "rustc-test".to_string(),
            source_relative_path: "src/lib.rs".to_string(),
            source_evidence_key: "compiler-suite-source:src/lib.rs".to_string(),
            crate_name: "fixture".to_string(),
            edition: "2024".to_string(),
            features: vec!["unit-graph-test-mode".to_string()],
            compile_environment: Default::default(),
            binary_dependencies: Vec::new(),
            workspace_library_dependencies: Vec::new(),
            externs: Vec::new(),
        };
        let empty_artifacts = OvenRustcArtifactManifest {
            schema_version: OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION,
            intent: receipt.intent.clone(),
            dependency_search_paths: Vec::new(),
            native_search_paths: Vec::new(),
            externs: Vec::new(),
            entrypoint_externs: Default::default(),
            registry_leaves: Vec::new(),
            registry_sources: Vec::new(),
            compile_environment: Default::default(),
            vocab_auxiliary_targets: Vec::new(),
            supporting_artifacts: Vec::new(),
        };
        let empty_closure = OvenCompilerTestSuiteArtifactClosure {
            dependency_search_paths: Vec::new(),
            native_search_paths: Vec::new(),
            supporting_artifacts: Vec::new(),
        };
        let schema_eight = OvenCompilerTestSuitePayload {
            schema_version: 8,
            test_targets: vec![target.clone()],
            shard_references: Vec::new(),
            foundation_references: Vec::new(),
            toolchain_data_references: Vec::new(),
            toolchain_loaf_generation: None,
            binary_targets: Vec::new(),
            test_artifact_closure: Some(empty_closure.clone()),
            cli_artifact_closure: None,
            cli_foundation_references: Vec::new(),
            cli_target: None,
            cli_workspace_libraries: Vec::new(),
            sdk_inventory_relative_path: "providers/sdk-inventory.json".to_string(),
            sdk_inventory_digest: "fixture".to_string(),
            toolchain_data_relative_root: None,
            warning_check_artifacts: empty_artifacts.clone(),
        };
        let _ = store.publish(&OvenArtifactPublishRequest {
            receipt: receipt.clone(),
            domain: "compiler-suite".to_string(),
            kind: OvenArtifactKind::CompilerTestSuite,
            payload: serde_json::to_vec(&schema_eight)?,
            materialized_files: Vec::new(),
        })?;
        assert_eq!(select_compiler_test_suite_identity(&store, &receipt)?, None);

        let schema_nine = OvenCompilerTestSuitePayload {
            schema_version: 9,
            test_targets: Vec::new(),
            shard_references: vec![OvenCompilerTestSuiteShardReference {
                identity: "sha256:fixture-shard".to_string(),
                target: target.key(),
                source_bytes: 0,
            }],
            foundation_references: Vec::new(),
            toolchain_data_references: Vec::new(),
            toolchain_loaf_generation: None,
            binary_targets: Vec::new(),
            test_artifact_closure: None,
            cli_artifact_closure: Some(empty_closure.clone()),
            cli_foundation_references: Vec::new(),
            cli_target: Some(target.clone()),
            cli_workspace_libraries: Vec::new(),
            sdk_inventory_relative_path: "providers/sdk-inventory.json".to_string(),
            sdk_inventory_digest: "fixture".to_string(),
            toolchain_data_relative_root: None,
            warning_check_artifacts: empty_artifacts.clone(),
        };
        let _ = store.publish(&OvenArtifactPublishRequest {
            receipt: receipt.clone(),
            domain: "compiler-suite".to_string(),
            kind: OvenArtifactKind::CompilerTestSuite,
            payload: serde_json::to_vec(&schema_nine)?,
            materialized_files: Vec::new(),
        })?;
        assert_eq!(select_compiler_test_suite_identity(&store, &receipt)?, None);

        let schema_fifteen = OvenCompilerTestSuitePayload {
            schema_version: OVEN_COMPILER_TEST_SUITE_SCHEMA_VERSION,
            test_targets: Vec::new(),
            shard_references: vec![OvenCompilerTestSuiteShardReference {
                identity: "sha256:fixture-shard".to_string(),
                target: target.key(),
                source_bytes: 1,
            }],
            foundation_references: vec![OvenCompilerTestSuiteFoundationReference {
                identity: "sha256:fixture-foundation".to_string(),
                label: "foundation-0000".to_string(),
            }],
            toolchain_data_references: Vec::new(),
            toolchain_loaf_generation: Some(OvenCompilerTestSuiteToolchainLoafGenerationReference {
                generation_identity: "sha256:fixture-generation".to_string(),
            }),
            binary_targets: Vec::new(),
            test_artifact_closure: None,
            cli_artifact_closure: Some(empty_closure),
            cli_foundation_references: Vec::new(),
            cli_target: Some(target),
            cli_workspace_libraries: Vec::new(),
            sdk_inventory_relative_path: "providers/sdk-inventory.json".to_string(),
            sdk_inventory_digest: "fixture".to_string(),
            toolchain_data_relative_root: None,
            warning_check_artifacts: empty_artifacts,
        };
        let current_manifest = store.publish(&OvenArtifactPublishRequest {
            receipt: receipt.clone(),
            domain: "compiler-suite".to_string(),
            kind: OvenArtifactKind::CompilerTestSuite,
            payload: serde_json::to_vec(&schema_fifteen)?,
            materialized_files: Vec::new(),
        })?;
        assert_eq!(
            select_compiler_test_suite_identity(&store, &receipt)?,
            Some(current_manifest.identity)
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn compiler_suite_prepare_reuses_a_current_suite_without_invoking_cargo() -> Result<(), Box<dyn std::error::Error>>
    {
        use std::os::unix::fs::PermissionsExt;

        let compiler_root = tempfile::tempdir()?;
        fs::create_dir_all(compiler_root.path().join("src"))?;
        fs::write(
            compiler_root.path().join("Cargo.toml"),
            "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )?;
        fs::write(compiler_root.path().join("Cargo.lock"), "version = 4\n")?;
        fs::write(compiler_root.path().join("src/lib.rs"), "pub fn fixture() {}\n")?;
        fs::write(compiler_root.path().join("src/main.rs"), "fn main() {}\n")?;
        let rustc_output = Command::new("rustup").args(["which", "rustc"]).output()?;
        assert!(rustc_output.status.success(), "rustup which rustc failed");
        let rustc = PathBuf::from(String::from_utf8(rustc_output.stdout)?.trim());
        let receipt = receipt_native_compiler_suite(&OvenCompilerSuiteRequest::new(
            compiler_root.path(),
            "aarch64-apple-darwin",
            rustc_identity(&rustc)?,
            "debug",
            Vec::new(),
        ))?;
        let store_root = tempfile::tempdir()?;
        let store = OvenStore::new(store_root.path(), OvenStoreLimits::new(1_000_000, 1_000_000, 1_000_000));
        let target = OvenCompilerTestSuiteTarget {
            package_name: "fixture".to_string(),
            target_name: "fixture".to_string(),
            target_kind: "lib".to_string(),
            runner: "rustc-test".to_string(),
            source_relative_path: "src/lib.rs".to_string(),
            source_evidence_key: "compiler-suite-source:src/lib.rs".to_string(),
            crate_name: "fixture".to_string(),
            edition: "2024".to_string(),
            features: Vec::new(),
            compile_environment: Default::default(),
            binary_dependencies: Vec::new(),
            workspace_library_dependencies: Vec::new(),
            externs: Vec::new(),
        };
        let empty_closure = OvenCompilerTestSuiteArtifactClosure {
            dependency_search_paths: Vec::new(),
            native_search_paths: Vec::new(),
            supporting_artifacts: Vec::new(),
        };
        let suite = OvenCompilerTestSuitePayload {
            schema_version: OVEN_COMPILER_TEST_SUITE_SCHEMA_VERSION,
            test_targets: Vec::new(),
            shard_references: vec![OvenCompilerTestSuiteShardReference {
                identity: "sha256:fixture-shard".to_string(),
                target: target.key(),
                source_bytes: 1,
            }],
            foundation_references: vec![OvenCompilerTestSuiteFoundationReference {
                identity: "sha256:fixture-foundation".to_string(),
                label: "foundation-0000".to_string(),
            }],
            toolchain_data_references: Vec::new(),
            toolchain_loaf_generation: Some(OvenCompilerTestSuiteToolchainLoafGenerationReference {
                generation_identity: "sha256:fixture-generation".to_string(),
            }),
            binary_targets: Vec::new(),
            test_artifact_closure: None,
            cli_artifact_closure: Some(empty_closure),
            cli_foundation_references: Vec::new(),
            cli_target: Some(target),
            cli_workspace_libraries: Vec::new(),
            sdk_inventory_relative_path: "providers/sdk-inventory.json".to_string(),
            sdk_inventory_digest: "fixture".to_string(),
            toolchain_data_relative_root: None,
            warning_check_artifacts: OvenRustcArtifactManifest {
                schema_version: OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION,
                intent: receipt.intent.clone(),
                dependency_search_paths: Vec::new(),
                native_search_paths: Vec::new(),
                externs: Vec::new(),
                entrypoint_externs: Default::default(),
                registry_leaves: Vec::new(),
                registry_sources: Vec::new(),
                compile_environment: Default::default(),
                vocab_auxiliary_targets: Vec::new(),
                supporting_artifacts: Vec::new(),
            },
        };
        let stored = store.publish(&OvenArtifactPublishRequest {
            receipt: receipt.clone(),
            domain: "compiler-suite".to_string(),
            kind: OvenArtifactKind::CompilerTestSuite,
            payload: serde_json::to_vec(&suite)?,
            materialized_files: Vec::new(),
        })?;
        fs::write(
            compiler_root.path().join("src/lib.rs"),
            "pub fn fixture_changed_without_graph_change() {}\n",
        )?;
        let current_receipt = receipt_native_compiler_suite(&OvenCompilerSuiteRequest::new(
            compiler_root.path(),
            "aarch64-apple-darwin",
            rustc_identity(&rustc)?,
            "debug",
            Vec::new(),
        ))?;
        assert_ne!(receipt.identity, current_receipt.identity);
        assert_eq!(receipt.build_unit_identity, current_receipt.build_unit_identity);
        let fixture = tempfile::tempdir()?;
        let cargo_marker = fixture.path().join("unexpected-cargo-invocation");
        let cargo = fixture.path().join("cargo");
        fs::write(
            &cargo,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$*\" > \"{}\"\nexit 97\n",
                cargo_marker.display()
            ),
        )?;
        fs::set_permissions(&cargo, fs::Permissions::from_mode(0o755))?;

        let result = prepare_compiler_test_suite(&OvenLegacyCargoPrepareRequest {
            store: &store,
            receipt: current_receipt,
            generated_project: fixture.path().join("unused-generated-project"),
            cargo,
            rustc,
            sdk_inventory: None,
            compiler_loaf_root: None,
            domain: "compiler-suite".to_string(),
            publication_kind: OvenLegacyCargoPublicationKind::LibraryTests,
            source_evidence_key: "compiler-libtest-root".to_string(),
            compile_environment: Default::default(),
            inspection_packages: Some(Vec::new()),
            direct_dependency_closure: OvenLegacyCargoDirectDependencyClosure::CheckedDeclared,
            compact_debug_info: false,
            source_compiler_vocab_support: false,
            base_loaf: None,
        })?;

        assert_eq!(result.suite_identity, stored.identity);
        assert_eq!(result.cargo_version, "not-run-existing-suite");
        assert_eq!(result.cargo_manifest_digest, "not-run-existing-suite");
        assert_eq!(result.cargo_lock_digest, "not-run-existing-suite");
        assert_eq!(result.transient_reservation_bytes, 0);
        assert_eq!(result.timing.unit_graph_elapsed_ms, 0);
        assert_eq!(result.timing.foundation_build_elapsed_ms, 0);
        assert_eq!(result.timing.direct_plan_elapsed_ms, 0);
        assert_eq!(result.timing.store_publication_elapsed_ms, 0);
        let timing = serde_json::to_value(&result)?;
        assert!(timing["timing"].get("preflight_and_sdk_elapsed_ms").is_some());
        assert!(
            !cargo_marker.exists(),
            "a compatible stored suite must return before invoking the supplied Cargo executable"
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn project_extension_reuse_requires_a_complete_current_payload() -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let generated_root = project.path().join("src/main.rs");
        fs::create_dir_all(generated_root.parent().ok_or("generated source parent missing")?)?;
        fs::write(
            project.path().join("Cargo.toml"),
            "[package]\nname = \"oven_schema_fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )?;
        fs::write(&generated_root, "fn main() {}\n")?;
        let rustc_output = Command::new("rustup").args(["which", "rustc"]).output()?;
        assert!(rustc_output.status.success(), "rustup which rustc failed");
        let rustc = PathBuf::from(String::from_utf8(rustc_output.stdout)?.trim());
        let receipt = receipt_generated_project(
            &OvenGeneratedProjectRequest::new(
                project.path(),
                "oven_schema_fixture",
                "0.1.0",
                rustc_host_target(&rustc)?,
                rustc_identity(&rustc)?,
                "debug",
                Vec::new(),
            )
            .with_generated_source("generated-root", &generated_root),
        )?;
        let base_plan = OvenRustcArtifactManifest {
            schema_version: OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION,
            intent: receipt.intent.clone(),
            dependency_search_paths: vec!["deps".to_string()],
            native_search_paths: Vec::new(),
            externs: vec![OvenRustcArtifactExtern {
                crate_name: "incan_stdlib".to_string(),
                relative_path: "deps/libincan_stdlib-release.rlib".to_string(),
                digest: digest_bytes(b"release stdlib"),
            }],
            entrypoint_externs: BTreeMap::new(),
            registry_leaves: Vec::new(),
            registry_sources: Vec::new(),
            compile_environment: BTreeMap::new(),
            vocab_auxiliary_targets: Vec::new(),
            supporting_artifacts: Vec::new(),
        };
        let mut publisher_plan = base_plan.clone();
        publisher_plan.externs = vec![
            OvenRustcArtifactExtern {
                crate_name: "incan_stdlib".to_string(),
                relative_path: "deps/libincan_stdlib-project.rlib".to_string(),
                digest: digest_bytes(b"project stdlib"),
            },
            OvenRustcArtifactExtern {
                crate_name: "project_dep".to_string(),
                relative_path: "deps/libproject_dep.rlib".to_string(),
                digest: digest_bytes(b"project dependency"),
            },
        ];
        let complete_plan =
            publisher_plan.with_release_cohort_from_base(&base_plan, &std::collections::BTreeSet::new())?;
        let partition = complete_plan.partition_against_base(&base_plan)?;
        assert!(!partition.base_paths.is_empty());
        assert!(!partition.extension_paths.is_empty());
        let base_identity = "sha256:release-base".to_string();
        let base = OvenLegacyCargoBaseLoaf {
            loaf_identity: base_identity.clone(),
            build_unit_identity: receipt.build_unit_identity.clone(),
            artifacts: &base_plan,
            artifact_root: project.path(),
        };
        let store_root = tempfile::tempdir()?;
        let store = OvenStore::new(store_root.path(), OvenStoreLimits::new(1_000_000, 1_000_000, 1_000_000));
        let payload = |schema_version| OvenProjectExtensionPayload {
            schema_version,
            base_loaf_identity: base_identity.clone(),
            base_build_unit_identity: receipt.build_unit_identity.clone(),
            publisher_plan: publisher_plan.clone(),
            complete_plan: complete_plan.clone(),
            registry_source_dependencies: Vec::new(),
            dev_registry_source_dependencies: Vec::new(),
            extension_paths: partition.extension_paths.iter().cloned().collect(),
        };
        let mut stale_payload = serde_json::to_value(payload(OVEN_PROJECT_EXTENSION_PAYLOAD_SCHEMA_VERSION - 1))?;
        let _ = stale_payload
            .as_object_mut()
            .ok_or("project extension fixture payload is not an object")?
            .remove("registry_source_dependencies");
        let _stale = store.publish(&OvenArtifactPublishRequest {
            receipt: receipt.clone(),
            domain: "incan-release-fixture".to_string(),
            kind: OvenArtifactKind::ProjectPayload,
            payload: serde_json::to_vec(&stale_payload)?,
            materialized_files: Vec::new(),
        })?;

        assert_eq!(
            select_existing_project_extension_identity(&store, &receipt, &base)?,
            None
        );

        let mut complete_plan_drift = payload(OVEN_PROJECT_EXTENSION_PAYLOAD_SCHEMA_VERSION);
        let project_dependency = complete_plan_drift
            .complete_plan
            .externs
            .iter_mut()
            .find(|artifact| artifact.crate_name == "project_dep")
            .ok_or("project dependency missing from complete fixture plan")?;
        project_dependency.digest = digest_bytes(b"drifted project dependency");
        store.publish(&OvenArtifactPublishRequest {
            receipt: receipt.clone(),
            domain: "incan-release-fixture".to_string(),
            kind: OvenArtifactKind::ProjectPayload,
            payload: serde_json::to_vec(&complete_plan_drift)?,
            materialized_files: Vec::new(),
        })?;
        let mut extension_paths_drift = payload(OVEN_PROJECT_EXTENSION_PAYLOAD_SCHEMA_VERSION);
        let _ = extension_paths_drift.extension_paths.pop();
        store.publish(&OvenArtifactPublishRequest {
            receipt: receipt.clone(),
            domain: "incan-release-fixture".to_string(),
            kind: OvenArtifactKind::ProjectPayload,
            payload: serde_json::to_vec(&extension_paths_drift)?,
            materialized_files: Vec::new(),
        })?;
        assert_eq!(
            select_existing_project_extension_identity(&store, &receipt, &base)?,
            None,
            "current-schema payload drift must trigger a replacement bake"
        );

        let current = store.publish(&OvenArtifactPublishRequest {
            receipt: receipt.clone(),
            domain: "incan-release-fixture".to_string(),
            kind: OvenArtifactKind::ProjectPayload,
            payload: serde_json::to_vec(&payload(OVEN_PROJECT_EXTENSION_PAYLOAD_SCHEMA_VERSION))?,
            materialized_files: Vec::new(),
        })?;
        assert_eq!(
            select_existing_project_extension_identity(&store, &receipt, &base)?,
            Some(current.identity)
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn generated_project_prepare_reuses_a_current_plan_without_invoking_cargo() -> Result<(), Box<dyn std::error::Error>>
    {
        use std::os::unix::fs::PermissionsExt;

        let project = tempfile::tempdir()?;
        let generated_root = project.path().join("src/main.rs");
        fs::create_dir_all(generated_root.parent().ok_or("generated source parent missing")?)?;
        fs::write(
            project.path().join("Cargo.toml"),
            "[package]\nname = \"oven_reuse_fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )?;
        fs::write(&generated_root, "fn main() {}\n")?;
        let rustc_output = Command::new("rustup").args(["which", "rustc"]).output()?;
        assert!(rustc_output.status.success(), "rustup which rustc failed");
        let rustc = PathBuf::from(String::from_utf8(rustc_output.stdout)?.trim());
        let receipt = receipt_generated_project(
            &OvenGeneratedProjectRequest::new(
                project.path(),
                "oven_reuse_fixture",
                "0.1.0",
                rustc_host_target(&rustc)?,
                rustc_identity(&rustc)?,
                "debug",
                Vec::new(),
            )
            .with_generated_source("generated-root", &generated_root),
        )?;
        let store_root = tempfile::tempdir()?;
        let store = OvenStore::new(store_root.path(), OvenStoreLimits::new(1_000_000, 1_000_000, 1_000_000));
        let plan = OvenRustcArtifactManifest {
            schema_version: OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION,
            intent: receipt.intent.clone(),
            dependency_search_paths: Vec::new(),
            native_search_paths: Vec::new(),
            externs: Vec::new(),
            entrypoint_externs: BTreeMap::new(),
            registry_leaves: Vec::new(),
            registry_sources: Vec::new(),
            compile_environment: Default::default(),
            vocab_auxiliary_targets: Vec::new(),
            supporting_artifacts: Vec::new(),
        };
        let stored = store.publish(&OvenArtifactPublishRequest {
            receipt: receipt.clone(),
            domain: "incan-release-fixture".to_string(),
            kind: OvenArtifactKind::DirectRustcPlan,
            payload: serde_json::to_vec(&plan)?,
            materialized_files: Vec::new(),
        })?;
        let fixture = tempfile::tempdir()?;
        let cargo_marker = fixture.path().join("unexpected-cargo-invocation");
        let cargo = fixture.path().join("cargo");
        fs::write(
            &cargo,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$*\" > \"{}\"\nexit 97\n",
                cargo_marker.display()
            ),
        )?;
        fs::set_permissions(&cargo, fs::Permissions::from_mode(0o755))?;

        let result = prepare_direct_rustc_plan(&OvenLegacyCargoPrepareRequest {
            store: &store,
            receipt,
            generated_project: project.path().to_path_buf(),
            cargo,
            rustc,
            sdk_inventory: None,
            compiler_loaf_root: None,
            domain: "incan-release-fixture".to_string(),
            publication_kind: OvenLegacyCargoPublicationKind::Executable,
            source_evidence_key: "generated-root".to_string(),
            compile_environment: Default::default(),
            inspection_packages: None,
            direct_dependency_closure: OvenLegacyCargoDirectDependencyClosure::GeneratedSource,
            compact_debug_info: false,
            source_compiler_vocab_support: false,
            base_loaf: None,
        })?;

        assert_eq!(result.plan_identity, stored.identity);
        assert_eq!(result.cargo_version, "not-run-existing-plan");
        assert_eq!(result.transient_reservation_bytes, 0);
        assert!(
            !cargo_marker.exists(),
            "a compatible stored project Loaf must return before invoking the supplied Cargo executable"
        );
        Ok(())
    }

    #[test]
    fn direct_rustc_environment_captures_generated_package_metadata() -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let source = project.path().join("src/main.rs");
        fs::create_dir_all(source.parent().ok_or("source parent missing")?)?;
        fs::write(
            project.path().join("Cargo.toml"),
            "[package]\nname = \"native_seed\"\nversion = \"7.2.1\"\n",
        )?;
        fs::write(&source, "fn main() {}\n")?;

        let environment = direct_rustc_compile_environment(project.path(), &source)?;

        assert_eq!(
            environment.get("CARGO_MANIFEST_DIR"),
            Some(&"@oven-source-ancestor:2".to_string())
        );
        assert_eq!(environment.get("CARGO_PKG_NAME"), Some(&"native_seed".to_string()));
        assert_eq!(environment.get("CARGO_PKG_VERSION"), Some(&"7.2.1".to_string()));
        Ok(())
    }

    #[test]
    fn reusable_project_plan_environment_excludes_generated_package_metadata() -> Result<(), Box<dyn std::error::Error>>
    {
        let project = tempfile::tempdir()?;
        let source = project.path().join("src/main.rs");
        fs::create_dir_all(source.parent().ok_or("source parent missing")?)?;
        fs::write(
            project.path().join("Cargo.toml"),
            "[package]\nname = \"shared_extension\"\nversion = \"8.1.0\"\n",
        )?;
        fs::write(&source, "fn main() {}\n")?;

        let environment = direct_rustc_reusable_project_plan_environment(project.path(), &source)?;

        assert_eq!(
            environment.get("CARGO_MANIFEST_DIR"),
            Some(&"@oven-source-ancestor:2".to_string())
        );
        assert!(!environment.contains_key("CARGO_PKG_NAME"));
        assert!(!environment.contains_key("CARGO_PKG_VERSION"));
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn interop_bootstrap_publisher_compiles_the_companion_library_before_native_interop_is_sealed()
    -> Result<(), Box<dyn std::error::Error>> {
        let fixture = tempfile::tempdir()?;
        let project = fixture.path().join("generated-project");
        let source = project.join("src/main.rs");
        fs::create_dir_all(source.parent().ok_or("source parent missing")?)?;
        fs::write(
            project.join("Cargo.toml"),
            "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[lib]\nname = \"fixture\"\npath = \"src/main.rs\"\n\n[[bin]]\nname = \"fixture\"\npath = \"src/main.rs\"\n",
        )?;
        // A binary target would need this absent native library at link time. The compatibility publisher must
        // instead compile the companion library target and leave native linking to the sealed interop plan.
        fs::write(
            &source,
            "#[link(name = \"not-yet-sealed-native\")]\nunsafe extern \"C\" {}\nfn main() {}\n",
        )?;
        fs::write(
            project.join("Cargo.lock"),
            "# This file is automatically @generated by Cargo.\n# It is not intended for manual editing.\nversion = 4\n\n[[package]]\nname = \"fixture\"\nversion = \"0.1.0\"\n",
        )?;
        let cargo_output = Command::new("rustup").args(["which", "cargo"]).output()?;
        assert!(cargo_output.status.success(), "rustup which cargo failed");
        let cargo = PathBuf::from(String::from_utf8(cargo_output.stdout)?.trim());
        let rustc_output = Command::new("rustup").args(["which", "rustc"]).output()?;
        assert!(rustc_output.status.success(), "rustup which rustc failed");
        let rustc = PathBuf::from(String::from_utf8(rustc_output.stdout)?.trim());

        let outputs = run_legacy_cargo(
            &cargo,
            &rustc,
            &project.join("Cargo.toml"),
            &fixture.path().join("target"),
            &rustc_host_target(&rustc)?,
            "debug",
            &[],
            1024 * 1024,
            OvenLegacyCargoPublicationKind::InteropBootstrap,
            false,
            false,
        )?;

        assert_eq!(outputs.len(), 1);
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn publisher_uses_network_only_for_a_fresh_explicit_project_resolution() -> Result<(), Box<dyn std::error::Error>> {
        use std::os::unix::fs::PermissionsExt;

        let fixture = tempfile::tempdir()?;
        let project = fixture.path().join("generated-project");
        let source = project.join("src/main.rs");
        let cargo = fixture.path().join("cargo");
        let rustc = fixture.path().join("rustc");
        let observed_directory = fixture.path().join("cargo-current-directory");
        let observed_debug_setting = fixture.path().join("cargo-debug-setting");
        let observed_incremental_setting = fixture.path().join("cargo-incremental-setting");
        let observed_arguments = fixture.path().join("cargo-arguments");
        fs::create_dir_all(source.parent().ok_or("source parent missing")?)?;
        fs::write(
            project.join("Cargo.toml"),
            "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\n",
        )?;
        fs::write(&source, "fn main() {}\n")?;
        fs::write(
            &cargo,
            format!(
                "#!/bin/sh\npwd > \"{}\"\nprintf '%s' \"$CARGO_PROFILE_DEV_DEBUG\" > \"{}\"\nprintf '%s' \"$CARGO_INCREMENTAL\" > \"{}\"\nprintf '%s\\n' \"$@\" > \"{}\"\n",
                observed_directory.display(),
                observed_debug_setting.display(),
                observed_incremental_setting.display(),
                observed_arguments.display(),
            ),
        )?;
        fs::write(&rustc, "#!/bin/sh\nexit 0\n")?;
        fs::set_permissions(&cargo, fs::Permissions::from_mode(0o755))?;
        fs::set_permissions(&rustc, fs::Permissions::from_mode(0o755))?;

        let _ = run_legacy_cargo_invocation(
            &cargo,
            &rustc,
            &project.join("Cargo.toml"),
            &fixture.path().join("target"),
            fixture.path(),
            "aarch64-apple-darwin",
            OVEN_COMPILER_TEST_PROFILE,
            &[],
            1024 * 1024,
            "build",
            &OvenLegacyCargoInvocationTarget::None,
            false,
            false,
            false,
        )?;

        assert_eq!(
            fs::canonicalize(&project)?,
            fs::canonicalize(fs::read_to_string(observed_directory)?.trim())?
        );
        assert_eq!(fs::read_to_string(observed_debug_setting)?, "");
        assert_eq!(fs::read_to_string(observed_incremental_setting)?, "");
        let arguments = fs::read_to_string(&observed_arguments)?;
        assert!(arguments.lines().any(|argument| argument == "--profile"));
        assert!(arguments.lines().any(|argument| argument == OVEN_COMPILER_TEST_PROFILE));
        assert!(
            !arguments.lines().any(|argument| argument == "--offline"),
            "a fresh explicit project bake must be able to establish its initial Cargo.lock"
        );
        assert!(
            !arguments.lines().any(|argument| argument == "--locked"),
            "a fresh explicit project bake cannot require a lock before Cargo has created it"
        );

        fs::write(project.join("Cargo.lock"), "version = 4\n")?;
        let _ = run_legacy_cargo_invocation(
            &cargo,
            &rustc,
            &project.join("Cargo.toml"),
            &fixture.path().join("target"),
            fixture.path(),
            "aarch64-apple-darwin",
            OVEN_COMPILER_TEST_PROFILE,
            &[],
            1024 * 1024,
            "build",
            &OvenLegacyCargoInvocationTarget::None,
            false,
            false,
            false,
        )?;
        let locked_arguments = fs::read_to_string(&observed_arguments)?;
        assert!(locked_arguments.lines().any(|argument| argument == "--offline"));
        assert!(locked_arguments.lines().any(|argument| argument == "--locked"));
        Ok(())
    }

    #[test]
    fn explicit_project_bake_creates_and_reuses_a_local_cargo_lock_issue1196() -> Result<(), Box<dyn std::error::Error>>
    {
        let fixture = tempfile::tempdir()?;
        let project = fixture.path().join("generated-project");
        let helper = project.join("helper");
        fs::create_dir_all(project.join("src"))?;
        fs::create_dir_all(helper.join("src"))?;
        fs::write(
            project.join("Cargo.toml"),
            "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[dependencies]\nhelper = { path = \"helper\" }\n",
        )?;
        fs::write(project.join("src/main.rs"), "fn main() { helper::marker(); }\n")?;
        fs::write(
            helper.join("Cargo.toml"),
            "[package]\nname = \"helper\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )?;
        fs::write(helper.join("src/lib.rs"), "pub fn marker() {}\n")?;

        let cargo = crate::backend::project::runner::resolved_cargo_executable()?;
        let manifest = project.join("Cargo.toml");
        let staging = fixture.path().join("staging");
        let initial_sources = explicit_project_bake_inspection_sources(&cargo, &manifest, &[], &[], &staging, None)?;
        assert!(
            initial_sources.is_empty(),
            "the local-only fixture has no registry sources to stage"
        );
        assert!(
            project.join("Cargo.lock").is_file(),
            "the one explicit bake boundary must establish the missing Cargo.lock"
        );

        let replayed_sources = legacy_cargo_inspection_sources(&cargo, &manifest, &[], &[], &staging)?;
        assert!(
            replayed_sources.is_empty(),
            "the newly created lock must support the locked publisher replay"
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn fresh_explicit_project_metadata_uses_online_then_locked_policy_issue1196()
    -> Result<(), Box<dyn std::error::Error>> {
        use std::os::unix::fs::PermissionsExt;

        let fixture = tempfile::tempdir()?;
        let project = fixture.path().join("generated-project");
        let cargo = fixture.path().join("cargo");
        let observed_arguments = fixture.path().join("cargo-metadata-arguments");
        fs::create_dir_all(&project)?;
        fs::write(
            project.join("Cargo.toml"),
            "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )?;
        fs::write(
            &cargo,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"{}\"\nif [ ! -f Cargo.lock ]; then printf 'version = 4\\n' > Cargo.lock; fi\nprintf '%s\\n' '{{\"packages\":[],\"resolve\":null}}'\n",
                observed_arguments.display(),
            ),
        )?;
        fs::set_permissions(&cargo, fs::Permissions::from_mode(0o755))?;
        let manifest = project.join("Cargo.toml");

        let _ = read_legacy_cargo_metadata_with_lock_policy(&cargo, &manifest, &[], false)?;
        let initial_arguments = fs::read_to_string(&observed_arguments)?;
        assert!(
            !initial_arguments.contains("--offline") && !initial_arguments.contains("--locked"),
            "the first explicit project metadata resolve must be allowed to establish Cargo.lock: {initial_arguments}"
        );
        assert!(project.join("Cargo.lock").is_file());

        let _ = read_legacy_cargo_metadata_with_lock_policy(&cargo, &manifest, &[], true)?;
        let invocations = fs::read_to_string(&observed_arguments)?;
        let replay_arguments = invocations.lines().last().ok_or("missing replay metadata invocation")?;
        assert!(
            replay_arguments.contains("--offline") && replay_arguments.contains("--locked"),
            "every later publisher metadata read must be locked and offline: {replay_arguments}"
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn cargo_monitor_counts_the_whole_private_publisher_staging_root() -> Result<(), Box<dyn std::error::Error>> {
        use std::os::unix::fs::PermissionsExt;
        use std::time::{Duration, Instant};

        let fixture = tempfile::tempdir()?;
        let cargo = fixture.path().join("cargo");
        let rustc = fixture.path().join("rustc");
        let manifest = fixture.path().join("Cargo.toml");
        let staging = fixture.path().join("legacy-cargo-staging");
        let target = staging.join("current-target");
        let retained_output = staging.join("earlier-target/overflow");
        let descendant_pid = fixture.path().join("cargo-descendant-pid");
        fs::create_dir_all(retained_output.parent().ok_or("retained output parent missing")?)?;
        fs::write(&manifest, "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\n")?;
        // Zero-filled files can remain sparse or compressed on APFS, which does not exercise the physical-byte limit.
        fs::write(
            &cargo,
            format!(
                "#!/bin/sh\nsleep 30 &\nprintf '%s\\n' \"$!\" > \"{}\"\ndd if=/dev/urandom of=\"{}\" bs=131072 count=1 2>/dev/null\nwait\n",
                descendant_pid.display(),
                retained_output.display(),
            ),
        )?;
        fs::write(&rustc, "#!/bin/sh\nexit 0\n")?;
        fs::set_permissions(&cargo, fs::Permissions::from_mode(0o755))?;
        fs::set_permissions(&rustc, fs::Permissions::from_mode(0o755))?;

        let started = Instant::now();
        let result = run_legacy_cargo_invocation(
            &cargo,
            &rustc,
            &manifest,
            &target,
            &staging,
            "aarch64-apple-darwin",
            "debug",
            &[],
            64 * 1024,
            "build",
            &OvenLegacyCargoInvocationTarget::None,
            false,
            false,
            false,
        );

        assert!(matches!(
            result,
            Err(super::OvenLegacyCargoError::TransientCapacityExceeded { path, .. }) if path == staging
        ));
        // The fixture's Cargo sleeps 30s, so this asserts the monitor aborted on the capacity breach rather than
        // waiting for the child. The bound is deliberately loose: it only has to separate "aborted" from "waited",
        // and a tight one measures machine load instead. At 2s this failed under the parallel suite while taking
        // 0.26s in isolation, which tested the host rather than the monitor.
        let elapsed = started.elapsed();
        assert!(
            elapsed < Duration::from_secs(10),
            "monitor should abort on the capacity breach, not wait for the 30s child; took {elapsed:?}",
        );
        fs::remove_dir_all(&staging)?;
        assert!(
            !staging.exists(),
            "capacity-aborted publisher staging was not removable"
        );
        let pid = fs::read_to_string(descendant_pid)?.trim().parse::<u32>()?;
        for _ in 0..100 {
            if !crate::oven::process::process_is_running(pid)? {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        Err("capacity abort left the fake-Cargo descendant running".into())
    }

    #[test]
    fn publisher_capacity_monitor_yields_after_a_long_full_tree_scan() {
        assert_eq!(
            super::publisher_capacity_probe_delay(std::time::Duration::from_millis(5)),
            super::PUBLISHER_CAPACITY_POLL_INTERVAL
        );
        assert_eq!(
            super::publisher_capacity_probe_delay(std::time::Duration::from_secs(2)),
            std::time::Duration::from_secs(2)
        );
    }

    #[cfg(unix)]
    #[test]
    fn compiler_suite_bootstrap_builds_the_declared_workspace_test_units() -> Result<(), Box<dyn std::error::Error>> {
        use std::os::unix::fs::PermissionsExt;

        let fixture = tempfile::tempdir()?;
        let cargo = fixture.path().join("cargo");
        let rustc = fixture.path().join("rustc");
        let arguments = fixture.path().join("cargo-arguments");
        fs::write(
            &cargo,
            format!("#!/bin/sh\nprintf '%s\\n' \"$@\" > \"{}\"\n", arguments.display()),
        )?;
        fs::write(&rustc, "#!/bin/sh\nexit 0\n")?;
        fs::set_permissions(&cargo, fs::Permissions::from_mode(0o755))?;
        fs::set_permissions(&rustc, fs::Permissions::from_mode(0o755))?;
        let manifest = fixture.path().join("Cargo.toml");
        fs::write(&manifest, "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\n")?;

        let _ = run_legacy_cargo_invocation(
            &cargo,
            &rustc,
            &manifest,
            &fixture.path().join("target"),
            fixture.path(),
            "aarch64-apple-darwin",
            "debug",
            &[],
            1024 * 1024,
            "test",
            &OvenLegacyCargoInvocationTarget::WorkspaceTests,
            false,
            false,
            false,
        )?;
        let arguments = fs::read_to_string(arguments)?;
        assert!(arguments.lines().any(|argument| argument == "--all"));
        assert!(arguments.lines().any(|argument| argument == "--no-run"));
        assert!(!arguments.lines().any(|argument| argument == "--lib"));
        assert!(!arguments.lines().any(|argument| argument == "--bin"));
        Ok(())
    }

    #[test]
    fn publisher_retains_transitive_external_proc_macro_externs() -> Result<(), Box<dyn std::error::Error>> {
        let compiler_root = tempfile::tempdir()?;
        let staging = tempfile::tempdir()?;
        let root_source = compiler_root.path().join("src/lib.rs");
        fs::create_dir_all(root_source.parent().ok_or("root source parent missing")?)?;
        fs::write(&root_source, "pub fn fixture() {}\n")?;

        let serde_source = staging.path().join("registry/serde/src/lib.rs");
        let serde_core_source = staging.path().join("registry/serde_core/src/lib.rs");
        let serde_derive_source = staging.path().join("registry/serde_derive/src/lib.rs");
        let macro_build_input_source = staging.path().join("registry/macro_build_input/src/lib.rs");
        for source in [
            &serde_source,
            &serde_core_source,
            &serde_derive_source,
            &macro_build_input_source,
        ] {
            fs::create_dir_all(source.parent().ok_or("registry source parent missing")?)?;
            fs::write(source, "pub fn fixture() {}\n")?;
        }
        let target_dependencies = staging.path().join("target/aarch64-apple-darwin/debug/deps");
        let host_dependencies = staging.path().join("target/debug/deps");
        fs::create_dir_all(&target_dependencies)?;
        fs::create_dir_all(&host_dependencies)?;
        let serde_artifact = target_dependencies.join("libserde-abc123.rlib");
        let host_serde_artifact = host_dependencies.join("libserde-host456.rlib");
        let serde_core_artifact = target_dependencies.join("libserde_core-abc123.rlib");
        let serde_derive_artifact = host_dependencies.join("libserde_derive-abc123.dylib");
        let macro_build_input_artifact = host_dependencies.join("libmacro_build_input-abc123.dylib");
        fs::write(&serde_artifact, "serde library")?;
        fs::write(&host_serde_artifact, "host serde library")?;
        fs::write(&serde_core_artifact, "serde core library")?;
        fs::write(&serde_derive_artifact, "serde derive proc macro")?;
        fs::write(&macro_build_input_artifact, "proc-macro compiler-only input")?;

        let graph = CargoUnitGraph {
            version: 1,
            units: vec![
                CargoUnitGraphUnit {
                    pkg_id: "fixture 0.1.0 (path+file:///fixture)".to_string(),
                    target: CargoUnitGraphTarget {
                        kind: vec!["lib".to_string()],
                        crate_types: vec!["lib".to_string()],
                        name: "fixture".to_string(),
                        src_path: root_source.clone(),
                        edition: "2024".to_string(),
                    },
                    mode: "test".to_string(),
                    platform: Some("aarch64-apple-darwin".to_string()),
                    features: Vec::new(),
                    dependencies: vec![CargoUnitGraphDependency {
                        index: 1,
                        extern_crate_name: Some("serde".to_string()),
                    }],
                },
                CargoUnitGraphUnit {
                    pkg_id: "serde 1.0.0 (registry+https://example.invalid)".to_string(),
                    target: CargoUnitGraphTarget {
                        kind: vec!["lib".to_string()],
                        crate_types: vec!["lib".to_string()],
                        name: "serde".to_string(),
                        src_path: serde_source.clone(),
                        edition: "2024".to_string(),
                    },
                    mode: "build".to_string(),
                    // Cargo can describe this unit as host-side when it is reached through a proc-macro root even
                    // though Oven recompiles the workspace library with its explicit receipt target.
                    platform: None,
                    features: vec!["derive".to_string()],
                    dependencies: vec![
                        CargoUnitGraphDependency {
                            index: 2,
                            extern_crate_name: Some("serde_core".to_string()),
                        },
                        CargoUnitGraphDependency {
                            index: 3,
                            extern_crate_name: Some("serde_derive".to_string()),
                        },
                    ],
                },
                CargoUnitGraphUnit {
                    pkg_id: "serde_core 1.0.0 (registry+https://example.invalid)".to_string(),
                    target: CargoUnitGraphTarget {
                        kind: vec!["lib".to_string()],
                        crate_types: vec!["lib".to_string()],
                        name: "serde_core".to_string(),
                        src_path: serde_core_source.clone(),
                        edition: "2024".to_string(),
                    },
                    mode: "build".to_string(),
                    platform: Some("aarch64-apple-darwin".to_string()),
                    features: Vec::new(),
                    dependencies: Vec::new(),
                },
                CargoUnitGraphUnit {
                    pkg_id: "serde_derive 1.0.0 (registry+https://example.invalid)".to_string(),
                    target: CargoUnitGraphTarget {
                        kind: vec!["proc-macro".to_string()],
                        crate_types: vec!["proc-macro".to_string()],
                        name: "serde_derive".to_string(),
                        src_path: serde_derive_source.clone(),
                        edition: "2024".to_string(),
                    },
                    mode: "build".to_string(),
                    platform: None,
                    features: Vec::new(),
                    dependencies: vec![CargoUnitGraphDependency {
                        index: 4,
                        extern_crate_name: Some("macro_build_input".to_string()),
                    }],
                },
                CargoUnitGraphUnit {
                    pkg_id: "macro_build_input 1.0.0 (registry+https://example.invalid)".to_string(),
                    target: CargoUnitGraphTarget {
                        kind: vec!["proc-macro".to_string()],
                        crate_types: vec!["proc-macro".to_string()],
                        name: "macro_build_input".to_string(),
                        src_path: macro_build_input_source.clone(),
                        edition: "2024".to_string(),
                    },
                    mode: "build".to_string(),
                    platform: None,
                    features: Vec::new(),
                    dependencies: Vec::new(),
                },
            ],
            roots: vec![0],
        };
        let serde_artifact_message = serde_json::json!({
            "reason": "compiler-artifact",
            "package_id": "serde 1.0.0 (registry+https://example.invalid)",
            "target": { "name": "serde", "src_path": serde_source },
            "features": ["derive"],
            "filenames": [serde_artifact],
            "profile": { "test": false },
        });
        let serde_core_artifact_message = serde_json::json!({
            "reason": "compiler-artifact",
            "package_id": "serde_core 1.0.0 (registry+https://example.invalid)",
            "target": { "name": "serde_core", "src_path": serde_core_source },
            "features": [],
            "filenames": [serde_core_artifact],
            "profile": { "test": false },
        });
        let host_serde_artifact_message = serde_json::json!({
            "reason": "compiler-artifact",
            "package_id": "serde 1.0.0 (registry+https://example.invalid)",
            "target": { "name": "serde", "src_path": serde_source },
            "features": ["derive"],
            "filenames": [host_serde_artifact],
            "profile": { "test": false },
        });
        let serde_derive_artifact_message = serde_json::json!({
            "reason": "compiler-artifact",
            "package_id": "serde_derive 1.0.0 (registry+https://example.invalid)",
            "target": { "name": "serde_derive", "src_path": serde_derive_source },
            "features": [],
            "filenames": [serde_derive_artifact],
            "profile": { "test": false },
        });
        let macro_build_input_artifact_message = serde_json::json!({
            "reason": "compiler-artifact",
            "package_id": "macro_build_input 1.0.0 (registry+https://example.invalid)",
            "target": { "name": "macro_build_input", "src_path": macro_build_input_source },
            "features": [],
            "filenames": [macro_build_input_artifact],
            "profile": { "test": false },
        });
        let output = CargoInvocationOutput {
            stdout: format!(
                "{serde_artifact_message}\n{host_serde_artifact_message}\n{serde_core_artifact_message}\n{serde_derive_artifact_message}\n{macro_build_input_artifact_message}\n"
            )
            .into_bytes(),
        };
        let artifact_index = compiler_suite_artifact_index(&output, "aarch64-apple-darwin")?;
        let catalog = compiler_suite_artifact_catalog(staging.path(), &[target_dependencies, host_dependencies], &[])?;

        let (externs, workspace_dependencies) = compiler_suite_target_externs(
            &graph.units[0],
            &graph,
            compiler_root.path(),
            &artifact_index,
            &catalog,
            "aarch64-apple-darwin",
        )?;

        assert!(workspace_dependencies.is_empty());
        assert_eq!(
            externs
                .iter()
                .map(|artifact| artifact.crate_name.as_str())
                .collect::<Vec<_>>(),
            ["serde", "serde_derive"]
        );
        assert!(externs[0].relative_path.ends_with("libserde-abc123.rlib"));
        assert!(externs[1].relative_path.ends_with("libserde_derive-abc123.dylib"));
        Ok(())
    }

    #[test]
    fn publisher_excludes_unmatched_build_output_but_retains_cargo_reported_compiler_artifacts()
    -> Result<(), Box<dyn std::error::Error>> {
        let staging = tempfile::tempdir()?;
        // The parent `build` component is deliberately unrelated to Cargo's build-script layout. The direct
        // dependency must remain admissible while both supported nested build-script `out` layouts are excluded.
        let workspace_root = staging.path().join("workspace/build");
        let serde_source = workspace_root.join("registry/serde/src/lib.rs");
        fs::create_dir_all(serde_source.parent().ok_or("serde source parent missing")?)?;
        fs::write(&serde_source, "pub fn fixture() {}\n")?;
        let dependency_directory = workspace_root.join("target/x86_64-unknown-linux-gnu/oven-test/deps");
        let build_output_directory =
            workspace_root.join("target/x86_64-unknown-linux-gnu/oven-test/build/serde-fixture/out");
        let build_output_with_identity_directory =
            workspace_root.join("target/x86_64-unknown-linux-gnu/oven-test/build/serde-fixture/abc123/out");
        let cargo_reported_output_directory =
            workspace_root.join("target/x86_64-unknown-linux-gnu/oven-test/build/serde/real123/out");
        fs::create_dir_all(&dependency_directory)?;
        fs::create_dir_all(&build_output_directory)?;
        fs::create_dir_all(&build_output_with_identity_directory)?;
        fs::create_dir_all(&cargo_reported_output_directory)?;
        let resolved_library = dependency_directory.join("libserde-resolved.rlib");
        let build_output_library = build_output_directory.join("libserde-build-output.rlib");
        let build_output_with_identity_library =
            build_output_with_identity_directory.join("libserde-build-output-with-identity.rlib");
        let cargo_reported_library = cargo_reported_output_directory.join("libserde-real123.rlib");
        let unrelated_build_output = workspace_root.join("fixtures/out/libfixture.rlib");
        assert!(!compiler_suite_cargo_build_output(&unrelated_build_output));
        assert!(compiler_suite_cargo_build_output(&build_output_library));
        assert!(compiler_suite_cargo_build_output(&build_output_with_identity_library));
        fs::write(&resolved_library, "resolved serde library")?;
        fs::write(&build_output_library, "not a Cargo dependency artifact")?;
        fs::write(
            &build_output_with_identity_library,
            "not a Cargo dependency artifact with an identity directory",
        )?;
        fs::write(&cargo_reported_library, "Cargo-reported serde compiler artifact")?;
        let artifact_message = serde_json::json!({
            "reason": "compiler-artifact",
            "package_id": "serde 1.0.0 (registry+https://example.invalid)",
            "target": { "name": "serde", "src_path": serde_source },
            "features": ["derive"],
            "filenames": [
                resolved_library,
                build_output_library,
                build_output_with_identity_library,
                cargo_reported_library,
            ],
            "profile": { "test": false },
        });
        let output = CargoInvocationOutput {
            stdout: format!("{artifact_message}\n").into_bytes(),
        };

        let artifact_index = compiler_suite_artifact_index(&output, "x86_64-unknown-linux-gnu")?;
        let indexed = artifact_index.values().flatten().collect::<Vec<_>>();
        let canonical_resolved_library = fs::canonicalize(&resolved_library)?;
        let canonical_cargo_reported_library = fs::canonicalize(&cargo_reported_library)?;
        let canonical_build_output_library = fs::canonicalize(&build_output_library)?;
        let canonical_build_output_with_identity_library = fs::canonicalize(&build_output_with_identity_library)?;
        assert_eq!(
            indexed,
            vec![&canonical_cargo_reported_library, &canonical_resolved_library]
        );
        let reported = compiler_suite_output_artifact_paths(&output)?;
        assert_eq!(
            reported,
            vec![canonical_cargo_reported_library.clone(), canonical_resolved_library]
        );
        let catalog = compiler_suite_artifact_catalog(&workspace_root, &[dependency_directory], &reported)?;
        assert_eq!(catalog.materialized_files.len(), 2);
        assert!(
            catalog
                .closure
                .dependency_search_paths
                .contains(&"target/x86_64-unknown-linux-gnu/oven-test/build/serde/real123/out".to_string())
        );
        assert!(
            catalog
                .materialized_files
                .iter()
                .any(|artifact| artifact.source_path == canonical_cargo_reported_library)
        );
        assert!(catalog.materialized_files.iter().all(|artifact| {
            artifact.source_path != canonical_build_output_library
                && artifact.source_path != canonical_build_output_with_identity_library
        }));
        Ok(())
    }

    #[test]
    fn publisher_converts_workspace_unit_graph_into_direct_rustc_targets() -> Result<(), Box<dyn std::error::Error>> {
        let compiler_root = tempfile::tempdir()?;
        let staging = tempfile::tempdir()?;
        fs::create_dir_all(compiler_root.path().join("src"))?;
        fs::write(
            compiler_root.path().join("Cargo.toml"),
            "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )?;
        fs::write(compiler_root.path().join("Cargo.lock"), "version = 4\n")?;
        let library_source = compiler_root.path().join("src/lib.rs");
        let cli_source = compiler_root.path().join("src/main.rs");
        let binary_source = compiler_root.path().join("src/bin/generate_fixture.rs");
        fs::write(&library_source, "pub fn fixture() {}\n")?;
        fs::write(&cli_source, "fn main() {}\n")?;
        fs::create_dir_all(binary_source.parent().ok_or("binary source parent missing")?)?;
        fs::write(&binary_source, "fn main() {}\n")?;
        let receipt = receipt_native_compiler_suite(&OvenCompilerSuiteRequest::new(
            compiler_root.path(),
            "aarch64-apple-darwin",
            "rustc fixture",
            "debug",
            Vec::new(),
        ))?;
        let dependency_source = staging.path().join("fixture_dep.rs");
        fs::write(&dependency_source, "pub fn dependency() {}\n")?;
        let dependency_directory = staging.path().join("target/aarch64-apple-darwin/debug/deps");
        let host_dependency_directory = staging.path().join("target/debug/deps");
        let cli_library_directory = staging.path().join("target/aarch64-apple-darwin/debug");
        fs::create_dir_all(&dependency_directory)?;
        fs::create_dir_all(&host_dependency_directory)?;
        fs::create_dir_all(&cli_library_directory)?;
        let dependency_artifact = dependency_directory.join("libfixture_dep-abc123.rlib");
        let cli_dependency_artifact = cli_library_directory.join("libfixture_dep.rlib");
        let host_dependency_artifact = host_dependency_directory.join("libfixture_dep-host456.rlib");
        fs::write(&dependency_artifact, "fixture dependency artifact")?;
        fs::write(&cli_dependency_artifact, "fixture CLI dependency artifact")?;
        fs::write(&host_dependency_artifact, "fixture host dependency artifact")?;

        let dependency_unit = CargoUnitGraphUnit {
            pkg_id: "fixture_dep 0.1.0 (path+file:///fixture-dep)".to_string(),
            target: CargoUnitGraphTarget {
                kind: vec!["lib".to_string()],
                crate_types: vec!["lib".to_string()],
                name: "fixture_dep".to_string(),
                src_path: dependency_source.clone(),
                edition: "2024".to_string(),
            },
            // Cargo's unit graph may describe a dependency under its test-root mode even when Cargo's JSON message
            // correctly reports the emitted dependency library with `profile.test = false`.
            mode: "test".to_string(),
            platform: Some("aarch64-apple-darwin".to_string()),
            features: vec!["root-feature".to_string()],
            dependencies: Vec::new(),
        };
        let test_unit = CargoUnitGraphUnit {
            pkg_id: "fixture 0.1.0 (path+file:///fixture)".to_string(),
            target: CargoUnitGraphTarget {
                kind: vec!["lib".to_string()],
                crate_types: vec!["lib".to_string()],
                name: "fixture".to_string(),
                src_path: library_source.clone(),
                edition: "2024".to_string(),
            },
            mode: "test".to_string(),
            platform: Some("aarch64-apple-darwin".to_string()),
            features: vec!["default".to_string()],
            dependencies: vec![
                CargoUnitGraphDependency {
                    index: 0,
                    extern_crate_name: Some("fixture_dep".to_string()),
                },
                CargoUnitGraphDependency {
                    index: 0,
                    extern_crate_name: Some("fixture_dep".to_string()),
                },
                CargoUnitGraphDependency {
                    index: 1,
                    extern_crate_name: Some("generate_fixture".to_string()),
                },
            ],
        };
        let binary_unit = CargoUnitGraphUnit {
            pkg_id: "fixture 0.1.0 (path+file:///fixture)".to_string(),
            target: CargoUnitGraphTarget {
                kind: vec!["bin".to_string()],
                crate_types: vec!["bin".to_string()],
                name: "generate_fixture".to_string(),
                src_path: binary_source,
                edition: "2024".to_string(),
            },
            mode: "build".to_string(),
            platform: Some("aarch64-apple-darwin".to_string()),
            features: vec!["default".to_string()],
            dependencies: vec![CargoUnitGraphDependency {
                index: 0,
                extern_crate_name: Some("fixture_dep".to_string()),
            }],
        };
        let cli_unit = CargoUnitGraphUnit {
            pkg_id: "fixture 0.1.0 (path+file:///fixture)".to_string(),
            target: CargoUnitGraphTarget {
                kind: vec!["bin".to_string()],
                crate_types: vec!["bin".to_string()],
                name: "incan".to_string(),
                src_path: cli_source.clone(),
                edition: "2024".to_string(),
            },
            mode: "build".to_string(),
            platform: Some("aarch64-apple-darwin".to_string()),
            features: vec!["default".to_string()],
            dependencies: vec![CargoUnitGraphDependency {
                index: 0,
                extern_crate_name: Some("fixture_dep".to_string()),
            }],
        };
        let doctest_unit = CargoUnitGraphUnit {
            mode: "doctest".to_string(),
            ..test_unit.clone()
        };
        let test_graph = CargoUnitGraph {
            version: 1,
            units: vec![dependency_unit, binary_unit, test_unit, doctest_unit],
            roots: vec![2, 3],
        };
        let cli_graph = CargoUnitGraph {
            version: 1,
            units: vec![
                CargoUnitGraphUnit {
                    pkg_id: "fixture_dep 0.1.0 (path+file:///fixture-dep)".to_string(),
                    target: CargoUnitGraphTarget {
                        kind: vec!["lib".to_string()],
                        crate_types: vec!["lib".to_string()],
                        name: "fixture_dep".to_string(),
                        src_path: dependency_source.clone(),
                        edition: "2024".to_string(),
                    },
                    mode: "build".to_string(),
                    platform: Some("aarch64-apple-darwin".to_string()),
                    features: vec!["root-feature".to_string()],
                    dependencies: Vec::new(),
                },
                cli_unit,
            ],
            roots: vec![1],
        };
        let test_artifact_message = serde_json::json!({
            "reason": "compiler-artifact",
            "package_id": "fixture_dep 0.1.0 (path+file:///fixture-dep)",
            "target": {
                "name": "fixture_dep",
                "src_path": dependency_source,
            },
            "features": [],
            "filenames": [dependency_artifact],
            "profile": { "test": false },
        });
        let host_artifact_message = serde_json::json!({
            "reason": "compiler-artifact",
            "package_id": "fixture_dep 0.1.0 (path+file:///fixture-dep)",
            "target": {
                "name": "fixture_dep",
                "src_path": dependency_source,
            },
            "features": [],
            "filenames": [host_dependency_artifact],
            "profile": { "test": false },
        });
        let cli_artifact_message = serde_json::json!({
            "reason": "compiler-artifact",
            "package_id": "fixture_dep 0.1.0 (path+file:///fixture-dep)",
            "target": {
                "name": "fixture_dep",
                "src_path": dependency_source,
            },
            "features": [],
            "filenames": [cli_dependency_artifact],
            "profile": { "test": false },
        });
        let test_output = CargoInvocationOutput {
            stdout: format!("{test_artifact_message}\n{host_artifact_message}\n").into_bytes(),
        };
        let cli_output = CargoInvocationOutput {
            stdout: format!("{cli_artifact_message}\n").into_bytes(),
        };
        let target = staging.path().join("target");
        let isolated_selection_output = CargoInvocationOutput {
            stdout: format!("{test_artifact_message}\n{host_artifact_message}\n").into_bytes(),
        };
        let (isolated_shard, isolated_files) = compiler_suite_direct_target_shard_plan(
            compiler_root.path(),
            &receipt,
            staging.path(),
            &target,
            &test_graph,
            2,
            &isolated_selection_output,
        )?;
        assert_eq!(isolated_shard.target.key().package_name, "fixture");
        assert_eq!(isolated_shard.target.runner, "rustc-test");
        assert_eq!(isolated_shard.binary_targets.len(), 1);
        assert_eq!(isolated_shard.binary_targets[0].target_name, "generate_fixture");
        assert_eq!(isolated_shard.artifact_closure.supporting_artifacts.len(), 2);
        assert_eq!(isolated_files.len(), 2);
        let sealed_foundation_catalog = compiler_suite_artifact_catalog(
            staging.path(),
            &[dependency_directory.clone(), host_dependency_directory.clone()],
            &[],
        )?;
        let sealed_foundation_index =
            compiler_suite_artifact_index(&isolated_selection_output, &receipt.intent.target)?;
        let (sealed_foundation_shard, sealed_foundation_files) = compiler_suite_direct_target_shard_from_catalog(
            compiler_root.path(),
            &receipt,
            &test_graph,
            2,
            &sealed_foundation_index,
            &sealed_foundation_catalog,
        )?;
        assert_eq!(sealed_foundation_shard.target, isolated_shard.target);
        assert_eq!(sealed_foundation_shard.binary_targets, isolated_shard.binary_targets);
        assert_eq!(
            sealed_foundation_shard.workspace_libraries,
            isolated_shard.workspace_libraries
        );
        assert_eq!(sealed_foundation_files.len(), isolated_files.len());
        assert_eq!(
            sealed_foundation_files
                .iter()
                .map(|file| file.relative_path.as_str())
                .collect::<Vec<_>>(),
            isolated_files
                .iter()
                .map(|file| file.relative_path.as_str())
                .collect::<Vec<_>>()
        );
        let (isolated_cli, isolated_cli_workspace_libraries, isolated_cli_closure, isolated_cli_files) =
            compiler_suite_direct_cli_plan(
                compiler_root.path(),
                &receipt,
                staging.path(),
                &target,
                &cli_graph,
                std::slice::from_ref(&cli_output),
            )?;
        assert_eq!(isolated_cli.source_relative_path, "src/main.rs");
        assert_eq!(isolated_cli.runner, "rustc-run");
        assert!(isolated_cli_workspace_libraries.is_empty());
        assert_eq!(isolated_cli_closure.supporting_artifacts.len(), 3);
        assert_eq!(isolated_cli_files.len(), 3);
        let (sealed_foundation_cli, sealed_foundation_cli_libraries) = compiler_suite_cli_target_from_artifact_index(
            compiler_root.path(),
            &receipt,
            &cli_graph,
            &sealed_foundation_index,
            &sealed_foundation_catalog,
        )?;
        assert_eq!(sealed_foundation_cli.key(), isolated_cli.key());
        assert_eq!(sealed_foundation_cli.features, isolated_cli.features);
        assert_eq!(
            sealed_foundation_cli.compile_environment,
            isolated_cli.compile_environment
        );
        assert_eq!(sealed_foundation_cli.externs.len(), 1);
        assert!(
            sealed_foundation_cli.externs[0]
                .relative_path
                .ends_with("libfixture_dep-abc123.rlib")
        );
        assert_eq!(sealed_foundation_cli_libraries, isolated_cli_workspace_libraries);
        let (targets, binary_targets, cli_target, closure, materialized) = compiler_suite_direct_target_plan(
            compiler_root.path(),
            &receipt,
            staging.path(),
            &target,
            &test_graph,
            &[test_output],
            &cli_graph,
            &cli_output,
        )?;

        assert_eq!(targets.len(), 2);
        assert_eq!(targets[0].binary_dependencies, ["generate_fixture"]);
        assert_eq!(targets[0].package_name, "fixture");
        assert_eq!(binary_targets.len(), 1);
        assert_eq!(binary_targets[0].target_name, "generate_fixture");
        assert_eq!(binary_targets[0].runner, "rustc-run");
        assert_eq!(targets[0].source_relative_path, "src/lib.rs");
        assert_eq!(targets[0].runner, "rustc-test");
        assert_eq!(targets[0].features, vec!["default"]);
        assert_eq!(targets[0].externs.len(), 1);
        assert_eq!(targets[0].externs[0].crate_name, "fixture_dep");
        assert!(
            targets[0].externs[0]
                .relative_path
                .ends_with("libfixture_dep-abc123.rlib")
        );
        assert_eq!(targets[1].source_relative_path, "src/lib.rs");
        assert_eq!(targets[1].runner, "rustdoc-test");
        assert_eq!(cli_target.source_relative_path, "src/main.rs");
        assert_eq!(cli_target.runner, "rustc-run");
        assert!(
            cli_target.externs[0]
                .relative_path
                .ends_with("target/aarch64-apple-darwin/debug/libfixture_dep.rlib")
        );
        assert!(
            materialized
                .iter()
                .any(|file| { file.relative_path == "target/aarch64-apple-darwin/debug/libfixture_dep.rlib" })
        );
        // Cargo can emit matching package/target/profile records for the host and selected target. The target
        // unit must resolve its target-triple artifact, while the immutable closure retains both for host-side
        // proc-macro/build dependencies that may be named by other roots.
        assert_eq!(closure.supporting_artifacts.len(), 3);
        assert_eq!(materialized.len(), 3);
        let shard = OvenCompilerTestSuiteShardPayload {
            schema_version: OVEN_COMPILER_TEST_SUITE_SHARD_SCHEMA_VERSION,
            target: targets[0].clone(),
            binary_targets: binary_targets.clone(),
            workspace_libraries: Vec::new(),
            foundation_references: Vec::new(),
            artifact_closure: closure.clone(),
        };
        let restored = serde_json::from_slice::<OvenCompilerTestSuiteShardPayload>(&serde_json::to_vec(&shard)?)?;
        assert_eq!(restored.schema_version, OVEN_COMPILER_TEST_SUITE_SHARD_SCHEMA_VERSION);
        assert_eq!(restored.target_key(), targets[0].key());
        assert_eq!(restored.binary_targets, binary_targets);
        Ok(())
    }

    #[test]
    fn publisher_assigns_direct_rustdoc_to_doctest_roots() -> Result<(), Box<dyn std::error::Error>> {
        assert_eq!(compiler_suite_target_runner("doctest")?, "rustdoc-test");
        Ok(())
    }

    #[test]
    fn publisher_accepts_doctest_roots_for_direct_rustdoc() -> Result<(), Box<dyn std::error::Error>> {
        let compiler_root = tempfile::tempdir()?;
        let source = compiler_root.path().join("src/lib.rs");
        fs::create_dir_all(source.parent().ok_or("lib source parent missing")?)?;
        fs::write(&source, "//! a doctest\n")?;
        let graph = CargoUnitGraph {
            version: 1,
            units: vec![CargoUnitGraphUnit {
                pkg_id: "fixture 0.1.0 (path+file:///fixture)".to_string(),
                target: CargoUnitGraphTarget {
                    kind: vec!["lib".to_string()],
                    crate_types: vec!["lib".to_string()],
                    name: "fixture".to_string(),
                    src_path: source,
                    edition: "2024".to_string(),
                },
                mode: "doctest".to_string(),
                platform: None,
                features: Vec::new(),
                dependencies: Vec::new(),
            }],
            roots: vec![0],
        };

        validate_compiler_suite_unit_graph(compiler_root.path(), &graph)?;
        Ok(())
    }

    #[test]
    fn publisher_accepts_proc_macro_test_roots_for_direct_rustc() -> Result<(), Box<dyn std::error::Error>> {
        let compiler_root = tempfile::tempdir()?;
        let source = compiler_root.path().join("crates/macros/src/lib.rs");
        fs::create_dir_all(source.parent().ok_or("macro source parent missing")?)?;
        fs::write(&source, "pub fn fixture() {}\n")?;
        let graph = CargoUnitGraph {
            version: 1,
            units: vec![CargoUnitGraphUnit {
                pkg_id: "fixture-macros 0.1.0 (path+file:///fixture-macros)".to_string(),
                target: CargoUnitGraphTarget {
                    kind: vec!["proc-macro".to_string()],
                    crate_types: vec!["proc-macro".to_string()],
                    name: "fixture_macros".to_string(),
                    src_path: source,
                    edition: "2024".to_string(),
                },
                mode: "test".to_string(),
                platform: None,
                features: Vec::new(),
                dependencies: Vec::new(),
            }],
            roots: vec![0],
        };

        validate_compiler_suite_unit_graph(compiler_root.path(), &graph)?;
        Ok(())
    }

    #[test]
    fn compiler_suite_plans_exact_package_qualified_root_selections() -> Result<(), Box<dyn std::error::Error>> {
        let compiler_root = tempfile::tempdir()?;
        let root_source = compiler_root.path().join("src/lib.rs");
        let root_binary = compiler_root.path().join("src/main.rs");
        let root_integration = compiler_root.path().join("tests/smoke.rs");
        let macro_source = compiler_root.path().join("crates/macros/src/lib.rs");
        let nested_integration = compiler_root.path().join("crates/other/tests/smoke.rs");
        fs::create_dir_all(root_source.parent().ok_or("root source parent missing")?)?;
        fs::create_dir_all(root_integration.parent().ok_or("root integration parent missing")?)?;
        fs::create_dir_all(macro_source.parent().ok_or("macro source parent missing")?)?;
        fs::create_dir_all(nested_integration.parent().ok_or("nested integration parent missing")?)?;
        fs::write(
            compiler_root.path().join("Cargo.toml"),
            "[package]\nname = \"root-package\"\nversion = \"0.1.0\"\n",
        )?;
        fs::write(
            compiler_root.path().join("crates/macros/Cargo.toml"),
            "[package]\nname = \"fixture-macros\"\nversion = \"0.1.0\"\n",
        )?;
        fs::write(
            compiler_root.path().join("crates/other/Cargo.toml"),
            "[package]\nname = \"other-package\"\nversion = \"0.1.0\"\n",
        )?;
        for source in [
            &root_source,
            &root_binary,
            &root_integration,
            &macro_source,
            &nested_integration,
        ] {
            fs::write(source, "fn fixture() {}\n")?;
        }
        let root_library = CargoUnitGraphUnit {
            pkg_id: "opaque-root-id".to_string(),
            target: CargoUnitGraphTarget {
                kind: vec!["lib".to_string()],
                crate_types: vec!["lib".to_string()],
                name: "root_package".to_string(),
                src_path: root_source.clone(),
                edition: "2024".to_string(),
            },
            mode: "test".to_string(),
            platform: None,
            features: vec!["root-feature".to_string()],
            dependencies: Vec::new(),
        };
        let graph = CargoUnitGraph {
            version: 1,
            units: vec![
                root_library.clone(),
                CargoUnitGraphUnit {
                    pkg_id: "opaque-root-id".to_string(),
                    target: CargoUnitGraphTarget {
                        kind: vec!["bin".to_string()],
                        crate_types: vec!["bin".to_string()],
                        name: "root-cli".to_string(),
                        src_path: root_binary,
                        edition: "2024".to_string(),
                    },
                    mode: "test".to_string(),
                    platform: None,
                    features: vec!["root-feature".to_string()],
                    dependencies: Vec::new(),
                },
                CargoUnitGraphUnit {
                    pkg_id: "opaque-root-id".to_string(),
                    target: CargoUnitGraphTarget {
                        kind: vec!["test".to_string()],
                        crate_types: Vec::new(),
                        name: "smoke".to_string(),
                        src_path: root_integration,
                        edition: "2024".to_string(),
                    },
                    mode: "test".to_string(),
                    platform: None,
                    features: vec!["root-feature".to_string()],
                    dependencies: Vec::new(),
                },
                CargoUnitGraphUnit {
                    pkg_id: "opaque-macro-id".to_string(),
                    target: CargoUnitGraphTarget {
                        kind: vec!["proc-macro".to_string()],
                        crate_types: vec!["proc-macro".to_string()],
                        name: "fixture_macros".to_string(),
                        src_path: macro_source,
                        edition: "2024".to_string(),
                    },
                    mode: "test".to_string(),
                    platform: None,
                    features: vec!["macro-feature".to_string()],
                    dependencies: Vec::new(),
                },
                CargoUnitGraphUnit {
                    mode: "doctest".to_string(),
                    ..root_library
                },
                CargoUnitGraphUnit {
                    pkg_id: "opaque-other-id".to_string(),
                    target: CargoUnitGraphTarget {
                        kind: vec!["test".to_string()],
                        crate_types: Vec::new(),
                        name: "smoke".to_string(),
                        src_path: nested_integration,
                        edition: "2024".to_string(),
                    },
                    mode: "test".to_string(),
                    platform: None,
                    features: vec!["other-feature".to_string()],
                    dependencies: Vec::new(),
                },
            ],
            roots: vec![0, 1, 2, 3, 4, 5],
        };

        let selections = compiler_suite_target_selections(compiler_root.path(), &graph)?;

        assert_eq!(
            selections,
            vec![
                OvenLegacyCargoInvocationTarget::WorkspacePackageLibrary("fixture-macros".to_string()),
                OvenLegacyCargoInvocationTarget::WorkspacePackageLibrary("root-package".to_string()),
                OvenLegacyCargoInvocationTarget::WorkspacePackageBinary {
                    package: "root-package".to_string(),
                    target: "root-cli".to_string(),
                },
                OvenLegacyCargoInvocationTarget::WorkspacePackageIntegrationTest {
                    package: "other-package".to_string(),
                    target: "smoke".to_string(),
                },
                OvenLegacyCargoInvocationTarget::WorkspacePackageIntegrationTest {
                    package: "root-package".to_string(),
                    target: "smoke".to_string(),
                },
                OvenLegacyCargoInvocationTarget::WorkspacePackageDoctests("root-package".to_string()),
            ]
        );
        let groups = compiler_suite_target_selection_groups(compiler_root.path(), &graph)?;
        assert_eq!(
            compiler_suite_bootstrap_selection(compiler_root.path(), &graph, &groups)?,
            (
                OvenLegacyCargoInvocationTarget::WorkspacePackageLibrary("root-package".to_string()),
                vec![0],
            )
        );
        assert_eq!(
            groups
                .iter()
                .map(|(selection, _)| selection.clone())
                .collect::<Vec<_>>(),
            selections
        );
        assert_eq!(
            groups.iter().map(|(_, indices)| indices.clone()).collect::<Vec<_>>(),
            vec![vec![3], vec![0], vec![1], vec![5], vec![2], vec![4]]
        );
        assert_eq!(
            groups
                .iter()
                .map(|(_, indices)| compiler_suite_target_selection_features(&graph, indices))
                .collect::<Result<Vec<_>, _>>()?,
            vec![
                vec!["macro-feature".to_string()],
                vec!["root-feature".to_string()],
                vec!["root-feature".to_string()],
                vec!["other-feature".to_string()],
                vec!["root-feature".to_string()],
                vec!["root-feature".to_string()],
            ]
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn publisher_invokes_one_package_qualified_integration_target() -> Result<(), Box<dyn std::error::Error>> {
        use std::os::unix::fs::PermissionsExt;

        let fixture = tempfile::tempdir()?;
        let cargo = fixture.path().join("cargo");
        let rustc = fixture.path().join("rustc");
        let arguments = fixture.path().join("cargo-arguments");
        let manifest = fixture.path().join("Cargo.toml");
        fs::write(
            &cargo,
            format!("#!/bin/sh\nprintf '%s\\n' \"$@\" > \"{}\"\n", arguments.display()),
        )?;
        fs::write(&rustc, "#!/bin/sh\nexit 0\n")?;
        fs::write(&manifest, "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\n")?;
        fs::set_permissions(&cargo, fs::Permissions::from_mode(0o755))?;
        fs::set_permissions(&rustc, fs::Permissions::from_mode(0o755))?;

        let _ = run_legacy_cargo_invocation(
            &cargo,
            &rustc,
            &manifest,
            &fixture.path().join("target"),
            fixture.path(),
            "aarch64-apple-darwin",
            "debug",
            &[],
            1024 * 1024,
            "test",
            &OvenLegacyCargoInvocationTarget::WorkspacePackageIntegrationTest {
                package: "fixture-package".to_string(),
                target: "smoke".to_string(),
            },
            false,
            false,
            false,
        )?;
        let arguments = fs::read_to_string(arguments)?;
        assert!(arguments.lines().any(|argument| argument == "--package"));
        assert!(arguments.lines().any(|argument| argument == "fixture-package"));
        assert!(arguments.lines().any(|argument| argument == "--test"));
        assert!(arguments.lines().any(|argument| argument == "smoke"));
        assert!(!arguments.lines().any(|argument| argument == "--workspace"));
        Ok(())
    }
}
