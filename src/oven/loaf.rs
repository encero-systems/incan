//! Compiler-owned Loafs for the bounded Oven Alpha envelope.
//!
//! A loaf is an immutable direct-`rustc` closure shipped with the active Incan toolchain. It is deliberately
//! separate from a generated-project receipt: one loaf can satisfy compatible clean worktrees, while each generated
//! source tree keeps its own receipt and final output. Normal commands select a verified compiler Loaf directly, or
//! a receipt-bound project Loaf from the bounded Oven store; neither path inspects a Cargo target or accepts a
//! project-selected native-artifact directory.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::interop::{OVEN_INTEROP_EXECUTION_RECEIPT_INPUT, OVEN_INTEROP_PLAN_SCHEMA_INPUT};
use super::rustc::{
    OvenRegistryLeafAuthority, OvenRustcArtifactManifest, OvenRustcArtifactPlan, OvenRustcError, OvenRustcRegistryLeaf,
    registry_source_dependencies_supported_by_catalog, validate_sealed_registry_leaf,
};
use super::store::OvenStoreError;
use super::{OvenReceipt, digest_bytes, receipt_without_build_unit_input};
use crate::manifest::{DependencySource, DependencySpec};
use crate::version::{INCAN_VERSION, SDK_PROVIDER_CODEGEN_REVISION};

/// Current wire format for one compiler-shipped Oven Loaf.
pub const OVEN_LOAF_SCHEMA_VERSION: u32 = 13;
/// Current wire format for the atomically committed Loaf-envelope manifest.
pub const OVEN_LOAF_ENVELOPE_MANIFEST_SCHEMA_VERSION: u32 = 3;
/// Internal marker enabled only while the named legacy publisher creates a compiler-owned Loaf.
///
/// This is deliberately distinct from normal Oven command selection: it grants compiler source emission the same
/// trusted standard-provider identity as the SDK publisher, but it never authorizes Cargo for a caller command.
pub(crate) const OVEN_LOAF_ENV: &str = "INCAN_OVEN_LOAF";
/// Actionable user guidance for a normal-command miss without turning it into a compatibility-baker fallback.
pub const OVEN_LOAF_MISS_GUIDANCE: &str = "Action: run `incan oven bake --project <project-root>` once. That command compiles this project's dependencies and caches the result, reusing anything already compatible. It is a deliberate, separate step: `incan build`, `incan run`, and `incan test` never compile dependencies on their own.";
/// Opening clause every fail-closed dependency miss in a normal project command reports.
///
/// The baker's cold probe recognizes an intended miss by matching this clause together with
/// [`OVEN_NO_IMPLICIT_DEPENDENCY_BUILD`]. Both sides share these constants rather than repeating the sentence, so
/// rewording user-facing text cannot silently stop the probe from recognizing the miss it is looking for.
pub const OVEN_DEPENDENCY_MISS_SUMMARY: &str = "This project's dependencies have not been compiled yet";
/// Opening clause a nested compiler-suite build reports for the same fail-closed miss.
pub const OVEN_NESTED_DEPENDENCY_MISS_SUMMARY: &str = "This nested build's dependencies have not been compiled yet";
/// Clause stating the no-implicit-build contract, required before a miss counts as fail-closed.
///
/// A miss message without it describes some other failure, so the probe must not treat it as the expected one.
pub const OVEN_NO_IMPLICIT_DEPENDENCY_BUILD: &str = "will not compile them for you";
/// Build-unit input that records a source compiler's sealed vocabulary-helper capability.
///
/// This is project-private publication evidence rather than a runtime-cohort input. A compiler-owned Loaf may omit
/// it only after proving that it seals the vocabulary helper itself.
pub const OVEN_SOURCE_COMPILER_VOCAB_SUPPORT_BUILD_INPUT: &str = "source-compiler-vocab-support";
const TOOLCHAIN_LOAF_RELATIVE_ROOT: &str = "share/incan/oven/loafs";
static LOAF_TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);
const OVEN_LOAF_ENVELOPE_LOCK_FILE: &str = ".envelope.lock";

/// Built-in compiler-owned Loaf set prepared by the explicit baker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OvenLoafEnvelope {
    /// Coherent compiled closures with source authority shipped in a release toolchain.
    Release,
    /// The same complete standard-provider closure for compiler-suite debug and release execution.
    CompilerSuite,
}

/// Normal compiler action used only to derive a receipt and generated project for a checked Loaf fixture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OvenLoafFixtureAction {
    /// Compile the fixture with normal `incan build` semantics.
    Build,
    /// Compile the fixture with normal `incan run` semantics.
    Run,
}

/// The independent authority a typed Loaf member contributes to one release-version envelope.
///
/// Compiled closures remain feature-unified direct-`rustc` inputs. Source-authority members carry the locked registry
/// source trees needed during Rust inspection, so those sources are shared without turning unrelated rlibs into one
/// interchangeable catalog. A checked fixture may deliberately contribute both authorities when its one coherent
/// closure genuinely owns them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OvenLoafMemberRole {
    /// A coherent direct-`rustc` closure that can be materialized for normal execution.
    CompiledClosure,
    /// A source-inspection authority selected independently from a linkable closure.
    SourceAuthority,
    /// One coherent closure that is intentionally both linkable and source-authoritative.
    CompiledClosureAndSourceAuthority,
}

impl OvenLoafMemberRole {
    /// Return whether normal direct-`rustc` execution may select this member.
    pub(crate) const fn provides_compiled_closure(self) -> bool {
        matches!(self, Self::CompiledClosure | Self::CompiledClosureAndSourceAuthority)
    }

    /// Return whether Rust inspection may select this member's sealed source catalog.
    pub(crate) const fn provides_source_authority(self) -> bool {
        matches!(self, Self::SourceAuthority | Self::CompiledClosureAndSourceAuthority)
    }
}

/// One checked Incan fixture in a built-in Loaf envelope.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OvenLoafSpecification {
    /// Stable human-readable family label used in progress and evidence.
    pub label: &'static str,
    /// Project name that determines the generated-project directory.
    pub project_name: &'static str,
    /// Debug or release profile selected for this Loaf.
    pub profile: &'static str,
    /// Normal compiler action used to derive the authorizing receipt.
    pub action: OvenLoafFixtureAction,
    /// Checked Incan source embedded in the compiler binary.
    pub source: &'static str,
    /// Checked Incan project manifest embedded in the compiler binary.
    pub manifest: &'static str,
    /// Checked registry-source inventory embedded separately from the generated fixture manifest.
    ///
    /// Source-only members use this to seal every supported stdlib package without compiling those packages again.
    pub inspection_manifest: &'static str,
    /// The independent authority this immutable member contributes to the envelope.
    pub role: OvenLoafMemberRole,
    /// Whether this linkable closure keeps every registry leaf emitted by its checked fixture.
    pub retain_complete_registry_leaves: bool,
    /// Whether this compiler-owned Loaf keeps every checked fixture dependency as a direct Rustc extern.
    ///
    /// A `stdlib` Loaf is a complete standard-library closure, not a scenario-shaped subset. Compiler-suite Loafs
    /// additionally seal vocabulary support in a target-specific auxiliary closure, so compiler-only roots never
    /// become a second direct-Rustc authority for ordinary generated programs.
    pub retain_checked_direct_dependencies: bool,
}

/// One atomically committed generation of a typed Loaf envelope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OvenLoafEnvelopeManifest {
    /// Manifest wire-schema version.
    pub schema_version: u32,
    /// Built-in envelope name (`release` or `compiler-suite`).
    pub envelope: String,
    /// Content identity of the complete generation and its release-family compatibility evidence.
    pub generation_identity: String,
    /// Canonical release-family compatibility evidence, excluding per-executable baker provenance.
    pub evidence: BTreeMap<String, String>,
    /// Complete typed member list for this generation.
    pub loafs: Vec<OvenLoafEnvelopeMember>,
}

/// One Loaf referenced by a committed envelope generation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OvenLoafEnvelopeMember {
    /// Stable member label from the typed envelope definition.
    pub label: String,
    /// Debug or release profile.
    pub profile: String,
    /// Checked fixture action used to derive its receipt.
    pub action: String,
    /// The authority this member contributes to the envelope.
    pub role: OvenLoafMemberRole,
    /// Receipt compatibility identity stored inside the Loaf.
    pub build_unit_identity: String,
    /// Digest of the canonical Loaf metadata, including every declared artifact digest.
    pub loaf_identity: String,
    /// Digest of the sealed direct-Rustc plan encoded by this Loaf manifest.
    pub plan_identity: String,
    /// Logical bytes measured by the publishing generation.
    pub logical_bytes: u64,
    /// Physical allocation measured by the publishing generation.
    pub physical_bytes: u64,
    /// Relative `generations/<identity>/<identity>.loaf/loaf.json` path.
    pub path: PathBuf,
}

const COMPILER_SUITE_LOAFS: [OvenLoafSpecification; 2] = [
    OvenLoafSpecification {
        label: "stdlib",
        project_name: "oven_compiler_suite_foundation",
        profile: "debug",
        action: OvenLoafFixtureAction::Run,
        source: include_str!("fixtures/compiler_suite_foundation.incn"),
        manifest: include_str!("fixtures/compiler_suite_foundation.toml"),
        inspection_manifest: include_str!("fixtures/compiler_suite_foundation.toml"),
        role: OvenLoafMemberRole::CompiledClosureAndSourceAuthority,
        retain_complete_registry_leaves: true,
        retain_checked_direct_dependencies: true,
    },
    OvenLoafSpecification {
        label: "stdlib",
        project_name: "oven_compiler_suite_foundation",
        profile: "release",
        action: OvenLoafFixtureAction::Run,
        source: include_str!("fixtures/compiler_suite_foundation.incn"),
        manifest: include_str!("fixtures/compiler_suite_foundation.toml"),
        inspection_manifest: include_str!("fixtures/compiler_suite_foundation.toml"),
        role: OvenLoafMemberRole::CompiledClosureAndSourceAuthority,
        retain_complete_registry_leaves: true,
        retain_checked_direct_dependencies: true,
    },
];

const RELEASE_LOAFS: [OvenLoafSpecification; 2] = [
    OvenLoafSpecification {
        label: "stdlib",
        project_name: "oven_release_stdlib",
        profile: "debug",
        action: OvenLoafFixtureAction::Run,
        source: include_str!("fixtures/release_stdlib.incn"),
        manifest: include_str!("fixtures/release_stdlib.toml"),
        inspection_manifest: include_str!("fixtures/release_stdlib.toml"),
        role: OvenLoafMemberRole::CompiledClosureAndSourceAuthority,
        retain_complete_registry_leaves: true,
        retain_checked_direct_dependencies: true,
    },
    OvenLoafSpecification {
        label: "stdlib",
        project_name: "oven_release_stdlib",
        profile: "release",
        action: OvenLoafFixtureAction::Build,
        source: include_str!("fixtures/release_stdlib.incn"),
        manifest: include_str!("fixtures/release_stdlib.toml"),
        inspection_manifest: include_str!("fixtures/release_stdlib.toml"),
        role: OvenLoafMemberRole::CompiledClosureAndSourceAuthority,
        retain_complete_registry_leaves: true,
        retain_checked_direct_dependencies: true,
    },
];

/// Return the complete checked specification for one built-in Loaf envelope.
#[must_use]
pub fn loaf_envelope_specifications(envelope: OvenLoafEnvelope) -> &'static [OvenLoafSpecification] {
    match envelope {
        OvenLoafEnvelope::Release => &RELEASE_LOAFS,
        OvenLoafEnvelope::CompilerSuite => &COMPILER_SUITE_LOAFS,
    }
}

/// Owner-scoped staging directory that is removed unless a verified loaf is atomically published from it.
pub(crate) struct LoafTemporaryDirectory {
    path: PathBuf,
    keep: bool,
}

impl LoafTemporaryDirectory {
    /// Create a unique owner-scoped Loaf staging directory below `parent`.
    pub(crate) fn create(parent: &Path, prefix: &str) -> io::Result<Self> {
        for _ in 0..128 {
            let sequence = LOAF_TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let path = parent.join(format!("{prefix}{}-{sequence}", std::process::id()));
            match fs::create_dir(&path) {
                Ok(()) => return Ok(Self { path, keep: false }),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
        }
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!(
                "could not allocate unique Oven Loaf staging directory below {}",
                parent.display()
            ),
        ))
    }

    /// Return the staging directory path while this owner retains cleanup responsibility.
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    /// Reclaim scratch space at an explicit, measurable boundary and report any filesystem failure.
    ///
    /// A failed removal leaves the remaining path for diagnosis; Drop must not silently retry expensive cleanup
    /// after the caller has already recorded its duration and failure.
    pub(crate) fn close(mut self) -> std::io::Result<()> {
        self.keep = true;
        fs::remove_dir_all(&self.path)
    }

    /// Retain the staging directory after its caller has atomically published it.
    pub(crate) fn persist(mut self) -> PathBuf {
        self.keep = true;
        self.path.clone()
    }
}

impl Drop for LoafTemporaryDirectory {
    fn drop(&mut self) {
        if !self.keep {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

/// Immutable direct-`rustc` closure shipped with one compiler/toolchain distribution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OvenLoaf {
    /// Loaf wire-schema version.
    pub schema_version: u32,
    /// Source-independent identity of the provider/dependency unit this loaf can materialize.
    pub build_unit_identity: String,
    /// Stable provenance for the explicit baker transition that produced this Loaf.
    pub provenance: OvenLoafProvenance,
    /// Payload accounting captured before the self-describing manifest is written.
    pub accounting: OvenLoafAccounting,
    /// Explicit compiler-owned runtime capabilities that may authorize a narrower standard-provider request.
    ///
    /// This is deliberately more restrictive than a generic dependency solver: every runtime input other than
    /// provider selection stays exact. The provider-subset policy is callable only after normal-command routing has
    /// rejected every caller-owned external `rust::` import. Rust's own `rust::std` is compiler-supplied, while a
    /// selected standard provider may contribute its checked transitive Rust closure; only standard-provider modules
    /// and facets may otherwise be subsets of this loaf.
    #[serde(default)]
    pub compatibility: OvenLoafCompatibility,
    /// Exact registry package artifacts emitted by the named Loaf publisher.
    ///
    /// Normal consumers may select only these records; this is deliberately not a Cargo cache, package index, or
    /// source resolver.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub registry_leaves: Vec<OvenRustcRegistryLeaf>,
    /// Direct-`rustc` compiler input closure relative to the loaf file's parent directory.
    pub plan: OvenRustcArtifactManifest,
}

/// Portable provenance carried by one Alpha Loaf.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OvenLoafProvenance {
    /// Incan compiler release that authored the Loaf contract.
    pub compiler_version: String,
    /// Exact Rust toolchain selected by the authorizing receipt.
    pub rust_toolchain: String,
    /// Compiler-owned SDK provider code-generation contract revision.
    pub sdk_provider_codegen_revision: String,
    /// Explicit baker boundary; normal commands never use this as a backend selector.
    pub baker: String,
}

/// Filesystem accounting for the immutable payload beside `loaf.json`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OvenLoafAccounting {
    /// Logical bytes in the copied direct-`rustc` payload before manifest bytes are added.
    pub payload_logical_bytes: u64,
    /// Host filesystem allocation for that payload at bake time.
    pub payload_physical_bytes: u64,
}

/// A receipt-authorized complete standard-library closure resolved from immutable toolchain data.
///
/// The release ships this Loaf once per target/profile. Both ordinary consumers and compiler-suite children retain
/// its generation lock while executing direct `rustc`; neither path copies the same full stdlib closure into every
/// project store. Project-specific compatibility closures remain separately bounded store Loafs.
#[derive(Debug)]
pub struct OvenToolchainLoaf {
    /// Content address of the selected immutable `loaf.json` contract.
    pub loaf_identity: String,
    /// Stable identity of the compiler-shipped loaf selected for this receipt.
    pub loaf_build_unit_identity: String,
    /// Receipt-compatible direct-Rustc manifest retained by the loaf.
    pub artifacts: OvenRustcArtifactManifest,
    /// Exact registry leaves sealed with this Loaf.
    pub registry_leaves: Vec<OvenRustcRegistryLeaf>,
    /// Immutable compiler-data directory containing the manifest's declared files.
    pub artifact_root: PathBuf,
    /// Trusted direct-Rustc invocation inputs resolved from that immutable compiler data.
    pub artifact_plan: OvenRustcArtifactPlan,
    /// Shared publication boundary retained while direct consumers use this generation.
    _generation_lock: Option<OvenLoafGenerationLock>,
}

#[derive(Debug)]
pub(crate) struct OvenLoafGenerationLock {
    file: File,
}

impl Drop for OvenLoafGenerationLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

impl OvenToolchainLoaf {
    #[must_use]
    /// Expose this unit's registry leaves with only its verified transitive metadata directories.
    pub(crate) fn registry_leaf_authority(&self) -> OvenRegistryLeafAuthority {
        OvenRegistryLeafAuthority::new_with_trusted_dependency_search_paths(
            self.artifact_root.clone(),
            self.registry_leaves.clone(),
            self.artifact_plan.dependency_search_paths.clone(),
        )
    }
}

/// Original compiled-member facts retained for one Incan selection request (#991, #1037).
///
/// Intake validates committed metadata and holds its generation lock. It does not compare a caller receipt, select
/// a profile, rank capabilities, or read native/source trees. The Incan decision and host response binding must
/// precede materializing any candidate's declared compiler inputs.
pub(crate) struct OvenNativeLoafCandidates {
    generation_identity: String,
    entries: Vec<OvenNativeLoafMetadata>,
    _generation_lock: OvenLoafGenerationLock,
}

struct OvenNativeLoafMetadata {
    member: OvenLoafEnvelopeMember,
    path: PathBuf,
    loaf: OvenLoaf,
}

impl OvenNativeLoafCandidates {
    /// Read the existing committed envelope without making a native compatibility decision.
    pub(crate) fn from_committed_envelope(
        root: &Path,
        envelope: OvenLoafEnvelope,
    ) -> Result<Option<Self>, OvenLoafError> {
        if !root.join("envelope.json").is_file() {
            return Ok(None);
        }
        let generation_lock = acquire_loaf_generation_lock(root)?;
        let expected_envelope = match envelope {
            OvenLoafEnvelope::Release => "release",
            OvenLoafEnvelope::CompilerSuite => "compiler-suite",
        };
        let (manifest, manifest_path) = committed_loaf_envelope_manifest(root, expected_envelope)?;
        let paths = committed_loaf_metadata_paths_for_authority(root, OvenLoafMemberRole::CompiledClosure)?;
        let mut members = BTreeMap::new();
        for member in &manifest.loafs {
            if members.insert(root.join(&member.path), member).is_some() {
                return Err(OvenLoafError::InvalidLoaf {
                    path: manifest_path,
                    message: "committed envelope repeats a member path".to_string(),
                });
            }
        }
        let mut entries = Vec::with_capacity(paths.len());
        for path in paths {
            let member = members.get(&path).copied().ok_or_else(|| OvenLoafError::InvalidLoaf {
                path: path.clone(),
                message: "compiled member is absent from the retained envelope".to_string(),
            })?;
            let bytes = fs::read(&path).map_err(|source| OvenLoafError::Io {
                path: path.clone(),
                source,
            })?;
            if digest_bytes(&bytes) != member.loaf_identity {
                return Err(OvenLoafError::InvalidLoaf {
                    path,
                    message: "compiled member metadata changed during intake".to_string(),
                });
            }
            let loaf: OvenLoaf = serde_json::from_slice(&bytes).map_err(|error| OvenLoafError::InvalidLoaf {
                path: path.clone(),
                message: format!("invalid compiled member metadata: {error}"),
            })?;
            let plan_bytes = serde_json::to_vec(&loaf.plan).map_err(|error| OvenLoafError::InvalidLoaf {
                path: path.clone(),
                message: format!("cannot encode compiled member plan: {error}"),
            })?;
            if loaf.schema_version != OVEN_LOAF_SCHEMA_VERSION
                || loaf.build_unit_identity != member.build_unit_identity
                || loaf.plan.intent.profile != member.profile
                || digest_bytes(&plan_bytes) != member.plan_identity
                || loaf.registry_leaves != loaf.plan.registry_leaves
            {
                return Err(OvenLoafError::InvalidLoaf {
                    path,
                    message: "compiled member disagrees with its committed coordinates".to_string(),
                });
            }
            loaf.plan.validate_shape(&loaf.plan.intent)?;
            validate_registry_leaf_catalog(&loaf, &path)?;
            entries.push(OvenNativeLoafMetadata {
                member: member.clone(),
                path,
                loaf,
            });
        }
        Ok(Some(Self {
            generation_identity: manifest.generation_identity,
            entries,
            _generation_lock: generation_lock,
        }))
    }

    /// Return the committed generation that owns every offered candidate.
    pub(crate) fn generation_identity(&self) -> &str {
        &self.generation_identity
    }

    /// Offer every compiled member in committed order; native intent and capabilities remain original facts.
    pub(crate) fn candidates(&self) -> impl Iterator<Item = OvenNativeLoafCandidate<'_>> {
        (0..self.entries.len()).map(|index| OvenNativeLoafCandidate { owner: self, index })
    }
}

/// A candidate handle whose private origin retains the actual generation lock.
#[derive(Clone, Copy)]
pub(crate) struct OvenNativeLoafCandidate<'owner> {
    owner: &'owner OvenNativeLoafCandidates,
    index: usize,
}

impl<'owner> OvenNativeLoafCandidate<'owner> {
    /// Return content-addressed member coordinates, without substituting a caller's receipt identity.
    pub(crate) fn member(&self) -> &'owner OvenLoafEnvelopeMember {
        &self.owner.entries[self.index].member
    }

    /// Return the committed generation whose actual lock remains held by this candidate's owner.
    pub(crate) fn generation_identity(&self) -> &'owner str {
        self.owner.generation_identity()
    }

    /// Return the original intent, runtime inputs, capabilities and declared artifact catalog.
    pub(crate) fn metadata(&self) -> &'owner OvenLoaf {
        &self.owner.entries[self.index].loaf
    }

    /// Validate only this selected member's declared native inputs while retaining its original owner.
    ///
    /// This performs physical validation, not caller authorization. The command must first bind the Incan response
    /// to its original request and this candidate. Missing inputs refuse; this operation never builds or publishes.
    pub(crate) fn materialize(self) -> Result<OvenMaterializedLoafCandidate<'owner>, OvenLoafError> {
        let entry = &self.owner.entries[self.index];
        let root = entry.path.parent().ok_or_else(|| OvenLoafError::InvalidLoaf {
            path: entry.path.clone(),
            message: "compiled member has no artifact root".to_string(),
        })?;
        let artifact_plan = entry.loaf.plan.materialize(root, &entry.loaf.plan.intent)?;
        Ok(OvenMaterializedLoafCandidate {
            candidate: self,
            artifact_root: root.to_path_buf(),
            artifact_plan,
        })
    }
}

/// A materialized candidate whose compiler inputs cannot outlive the borrowed committed generation.
pub(crate) struct OvenMaterializedLoafCandidate<'owner> {
    candidate: OvenNativeLoafCandidate<'owner>,
    artifact_root: PathBuf,
    artifact_plan: OvenRustcArtifactPlan,
}

impl OvenMaterializedLoafCandidate<'_> {
    /// Return the original metadata handle paired with these exact materialized inputs.
    pub(crate) fn candidate(&self) -> OvenNativeLoafCandidate<'_> {
        self.candidate
    }

    /// Borrow the original selected root whose declared inputs passed the full materializer.
    pub(crate) fn artifact_root(&self) -> &Path {
        &self.artifact_root
    }

    /// Borrow the verified compiler inputs while the generation remains locked.
    pub(crate) fn artifact_plan(&self) -> &OvenRustcArtifactPlan {
        &self.artifact_plan
    }
}

/// Explicit runtime capability envelope for a compiler-owned Loaf.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OvenLoafCompatibility {
    /// Every receipt build-unit input other than provider selection and its derived feature set.
    ///
    /// Values such as the runtime source digests and lockfile must compare exactly before a loaf can satisfy another
    /// receipt. The resolved Rust-dependency digest remains in the receipt identity, but is intentionally excluded
    /// here: this policy runs only after caller-owned external Rust imports are refused. The standard-library feature
    /// digest is likewise represented by the selected provider modules and facets, which permits a verified provider
    /// superset to serve a narrower compiler-owned request without becoming a dependency resolver.
    pub runtime_inputs: BTreeMap<String, String>,
    /// Standard-provider modules, implementation facets, and direct rlib links compiled into the shipped closure.
    pub providers: Vec<OvenLoafProviderCapability>,
}

/// One provider capability compiled into a Loaf.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OvenLoafProviderCapability {
    /// Stable provider identity supplied by the checked compiler provider plan.
    pub identity: String,
    /// Canonical standard-provider module paths covered by this closure.
    pub modules: Vec<String>,
    /// Exact implementation facets selected while the closure was published.
    pub facets: Vec<String>,
    /// Whether this provider's rlib is a required direct Rust link root even without a source-module import.
    #[serde(default)]
    pub direct_link: bool,
}

/// Authorization policy for compiler-shipped Loaf selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OvenLoafSelection {
    /// Require the loaf's complete build-unit identity to equal the caller receipt.
    Exact,
    /// Permit a compiler-owned standard-provider closure to satisfy a narrower request.
    ///
    /// Callers use this only after rejecting inline `rust::` imports. The relation still requires exact runtime
    /// inputs and loaf-superset provider modules/facets; it is not a dependency resolver or a Cargo fallback.
    CompilerOwnedProviderSuperset,
}

/// How much compiler-owned provider capability a loaf contributes beyond one requested receipt.
///
/// Multiple immutable loafs can safely authorize the same request. Prefer the narrowest one so adding a new
/// provider-family loaf does not make an otherwise valid core request ambiguous. The values are derived only after
/// the exact runtime-input check, so this is a deterministic efficiency choice, never dependency resolution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct OvenLoafProviderExcess {
    providers: usize,
    modules: usize,
    facets: usize,
    direct_links: usize,
}

/// A Loaf that has passed the narrow compiler-owned provider-subset authorization rule.
#[derive(Debug, Clone, PartialEq, Eq)]
struct CompatibleLoaf {
    path: PathBuf,
    excess: OvenLoafProviderExcess,
}

impl OvenLoafCompatibility {
    /// Derive the explicit, portable compatibility envelope from one verified generated-project receipt.
    pub(crate) fn from_receipt(receipt: &OvenReceipt) -> Result<Self, OvenLoafError> {
        let mut runtime_inputs = receipt.sources.build_unit_inputs.clone();
        let provider_records = runtime_inputs.remove("providers").unwrap_or_default();
        let _ = runtime_inputs.remove("rust-dependencies");
        let _ = runtime_inputs.remove("stdlib-features");
        // The selected interop receipt proves package-owned archives and headers, not a compiler-owned runtime
        // capability. Its immutable final plan is independently reconstructed and verified before execution; using
        // it as a Loaf compatibility key would require one shipped Loaf per consumer package.
        let _ = runtime_inputs.remove(OVEN_INTEROP_EXECUTION_RECEIPT_INPUT);
        let _ = runtime_inputs.remove(OVEN_INTEROP_PLAN_SCHEMA_INPUT);
        let provider_plan = runtime_inputs
            .remove("provider-plan")
            .ok_or_else(|| OvenLoafError::Preparation {
                message: "Loaf receipt is missing its provider-plan input".to_string(),
            })?;
        let expected_provider_plan = digest_bytes(provider_records.as_bytes());
        if provider_plan != expected_provider_plan {
            return Err(OvenLoafError::Preparation {
                message: "Loaf receipt provider-plan digest does not match its provider records".to_string(),
            });
        }
        let providers = parse_provider_capabilities(&provider_records)?;
        Ok(Self {
            runtime_inputs,
            providers,
        })
    }

    /// Return the extra capability retained by this compatible loaf, or `None` when it cannot serve `receipt`.
    fn provider_subset_excess(&self, receipt: &OvenReceipt) -> Result<Option<OvenLoafProviderExcess>, OvenLoafError> {
        let requested = Self::from_receipt(receipt)?;
        if self.runtime_inputs != requested.runtime_inputs {
            return Ok(None);
        }
        let mut available = BTreeMap::new();
        for provider in &self.providers {
            if available.insert(provider.identity.as_str(), provider).is_some() {
                return Err(OvenLoafError::InvalidLoaf {
                    path: PathBuf::from("loaf compatibility"),
                    message: format!("declares provider `{}` more than once", provider.identity),
                });
            }
        }
        let requested_by_identity = requested
            .providers
            .iter()
            .map(|provider| (provider.identity.as_str(), provider))
            .collect::<BTreeMap<_, _>>();
        for required in &requested.providers {
            let Some(candidate) = available.get(required.identity.as_str()) else {
                return Ok(None);
            };
            if !required
                .modules
                .iter()
                .all(|module| candidate.modules.binary_search(module).is_ok())
                || !required
                    .facets
                    .iter()
                    .all(|facet| candidate.facets.binary_search(facet).is_ok())
                || (required.direct_link && !candidate.direct_link)
            {
                return Ok(None);
            }
        }
        let mut excess = OvenLoafProviderExcess {
            providers: 0,
            modules: 0,
            facets: 0,
            direct_links: 0,
        };
        for candidate in &self.providers {
            let Some(required) = requested_by_identity.get(candidate.identity.as_str()) else {
                excess.providers += 1;
                excess.modules += candidate.modules.len();
                excess.facets += candidate.facets.len();
                excess.direct_links += usize::from(candidate.direct_link);
                continue;
            };
            excess.modules += candidate
                .modules
                .iter()
                .filter(|module| required.modules.binary_search(module).is_err())
                .count();
            excess.facets += candidate
                .facets
                .iter()
                .filter(|facet| required.facets.binary_search(facet).is_err())
                .count();
            excess.direct_links += usize::from(candidate.direct_link && !required.direct_link);
        }
        Ok(Some(excess))
    }

    /// Return whether this shipped runtime closure can safely satisfy `receipt` under the narrow provider-subset rule.
    fn authorizes_provider_subset(&self, receipt: &OvenReceipt) -> Result<bool, OvenLoafError> {
        Ok(self.provider_subset_excess(receipt)?.is_some())
    }

    /// Return whether this independent source authority may inspect Rust metadata for `receipt`.
    ///
    /// Source inspection never authorizes a linkable direct-`rustc` closure, so provider modules and facets do not
    /// participate in this decision. Compiler/runtime source evidence, target, profile, toolchain, and every package
    /// feature still remain checked by the caller, the selected source catalog, and the plan intent.
    fn authorizes_source_authority(&self, receipt: &OvenReceipt) -> Result<bool, OvenLoafError> {
        Ok(self.runtime_inputs == Self::from_receipt(receipt)?.runtime_inputs)
    }
}

/// Parse the canonical provider-capability records sealed into a Loaf receipt.
fn parse_provider_capabilities(records: &str) -> Result<Vec<OvenLoafProviderCapability>, OvenLoafError> {
    let mut providers = Vec::new();
    for record in records.lines().filter(|record| !record.is_empty()) {
        let mut parts = record.split('|');
        let identity = parts.next().unwrap_or_default().trim();
        let modules = parts.next().unwrap_or_default();
        let facets = parts.next().unwrap_or_default();
        let direct_link = match parts.next() {
            None => false,
            Some("none") => false,
            Some("link") => true,
            Some(_) => {
                return Err(OvenLoafError::Preparation {
                    message: format!("Loaf provider record is not canonical: {record}"),
                });
            }
        };
        if identity.is_empty() || parts.next().is_some() {
            return Err(OvenLoafError::Preparation {
                message: format!("Loaf provider record is not canonical: {record}"),
            });
        }
        let mut modules = modules
            .split(',')
            .filter(|module| !module.is_empty())
            .map(str::to_string)
            .collect::<Vec<_>>();
        let mut facets = facets
            .split(',')
            .filter(|facet| !facet.is_empty())
            .map(str::to_string)
            .collect::<Vec<_>>();
        modules.sort();
        modules.dedup();
        facets.sort();
        facets.dedup();
        providers.push(OvenLoafProviderCapability {
            identity: identity.to_string(),
            modules,
            facets,
            direct_link,
        });
    }
    providers.sort_by(|left, right| left.identity.cmp(&right.identity));
    if providers.windows(2).any(|pair| pair[0].identity == pair[1].identity) {
        return Err(OvenLoafError::Preparation {
            message: "Loaf provider records repeat one provider identity".to_string(),
        });
    }
    Ok(providers)
}

/// Construct the portable runtime portion of a normal generated project's native build-unit identity.
///
/// The caller contributes normalized provider records, selected stdlib features, and the digest of resolved Rust
/// dependencies. Compiler-owned sources and the lockfile are resolved from the active toolchain layout so a packaged
/// compiler never depends on the checkout from which its binary happened to be built.
pub fn runtime_build_unit_inputs(
    provider_records: Vec<String>,
    stdlib_features: &[String],
    rust_dependencies_digest: String,
) -> Result<BTreeMap<String, String>, String> {
    let mut inputs = BTreeMap::new();
    inputs.insert("compiler-version".to_string(), INCAN_VERSION.to_string());
    inputs.insert(
        "sdk-provider-codegen-revision".to_string(),
        SDK_PROVIDER_CODEGEN_REVISION.to_string(),
    );
    for (name, crate_name) in [
        ("runtime-source-incan-core", "incan_core"),
        ("runtime-source-incan-derive", "incan_derive"),
        ("runtime-source-incan-stdlib", "incan_stdlib"),
    ] {
        let path = crate::toolchain_layout::resolve_toolchain_crate_path(crate_name);
        let digest = digest_runtime_crate_source(&path)?;
        inputs.insert(name.to_string(), digest);
    }
    let lock_path = crate::toolchain_layout::resolve_toolchain_runtime_lockfile();
    let lock = fs::read(&lock_path)
        .map_err(|error| format!("failed to read Oven runtime lock {}: {error}", lock_path.display()))?;
    inputs.insert("runtime-lock".to_string(), digest_bytes(&lock));
    inputs.insert(
        "provider-plan".to_string(),
        digest_bytes(provider_records.join("\n").as_bytes()),
    );
    if !provider_records.is_empty() {
        inputs.insert("providers".to_string(), provider_records.join("\n"));
    }
    inputs.insert(
        "stdlib-features".to_string(),
        digest_bytes(stdlib_features.join(",").as_bytes()),
    );
    inputs.insert("rust-dependencies".to_string(), rust_dependencies_digest);
    Ok(inputs)
}

/// Digest exactly the compiler runtime source closure retained by a suite publisher.
///
/// Runtime compatibility is determined by the package manifest and Rust sources that a generated provider can
/// compile against. Test fixtures, documentation, and nested build output are not runtime inputs and the suite
/// publisher deliberately does not retain them. Hashing the whole checkout crate here would make a native loaf
/// incompatible with the publisher's smaller immutable closure even when the compiled runtime is identical.
pub(crate) fn digest_runtime_crate_source(root: &Path) -> Result<String, String> {
    let root_metadata = fs::symlink_metadata(root)
        .map_err(|error| format!("failed to read runtime crate root {}: {error}", root.display()))?;
    if !root_metadata.is_dir() || root_metadata.file_type().is_symlink() {
        return Err(format!(
            "runtime crate root {} must be a directory without symlink indirection",
            root.display()
        ));
    }
    let manifest = root.join("Cargo.toml");
    let manifest_metadata = fs::symlink_metadata(&manifest)
        .map_err(|error| format!("failed to read runtime manifest {}: {error}", manifest.display()))?;
    if !manifest_metadata.is_file() || manifest_metadata.file_type().is_symlink() {
        return Err(format!(
            "runtime manifest {} must be a regular file without symlink indirection",
            manifest.display()
        ));
    }
    let source_root = root.join("src");
    let source_metadata = fs::symlink_metadata(&source_root).map_err(|error| {
        format!(
            "failed to read runtime source directory {}: {error}",
            source_root.display()
        )
    })?;
    if !source_metadata.is_dir() || source_metadata.file_type().is_symlink() {
        return Err(format!(
            "runtime source directory {} must be a directory without symlink indirection",
            source_root.display()
        ));
    }

    let mut records = BTreeMap::new();
    records.insert(
        "Cargo.toml".to_string(),
        digest_bytes(
            &fs::read(&manifest)
                .map_err(|error| format!("failed to read runtime manifest {}: {error}", manifest.display()))?,
        ),
    );
    collect_runtime_source_records(&source_root, &source_root, &mut records)?;
    serde_json::to_vec(&records)
        .map(|payload| digest_bytes(&payload))
        .map_err(|error| {
            format!(
                "failed to serialize runtime source digest for {}: {error}",
                root.display()
            )
        })
}

/// Add the regular files below one runtime crate's `src/` tree to its portable source digest.
fn collect_runtime_source_records(
    source_root: &Path,
    current: &Path,
    records: &mut BTreeMap<String, String>,
) -> Result<(), String> {
    let mut entries = fs::read_dir(current)
        .map_err(|error| format!("failed to read runtime source directory {}: {error}", current.display()))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("failed to read runtime source directory {}: {error}", current.display()))?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path)
            .map_err(|error| format!("failed to inspect runtime source {}: {error}", path.display()))?;
        if metadata.file_type().is_symlink() {
            return Err(format!("runtime source {} must not contain symlinks", path.display()));
        }
        if metadata.is_dir() {
            collect_runtime_source_records(source_root, &path, records)?;
            continue;
        }
        if !metadata.is_file() {
            return Err(format!(
                "runtime source {} must contain only regular files",
                path.display()
            ));
        }
        let relative = path
            .strip_prefix(source_root)
            .map_err(|_| format!("runtime source {} escaped {}", path.display(), source_root.display()))?
            .to_string_lossy()
            .replace('\\', "/");
        let key = format!("src/{relative}");
        let digest = digest_bytes(
            &fs::read(&path).map_err(|error| format!("failed to read runtime source {}: {error}", path.display()))?,
        );
        if records.insert(key.clone(), digest).is_some() {
            return Err(format!("runtime source contains duplicate portable path {key}"));
        }
    }
    Ok(())
}

/// Loaf loading, validation, or store-publication failure.
#[derive(Debug, Error)]
pub enum OvenLoafError {
    /// A compiler-owned loaf file could not be read.
    #[error("failed to read Oven Loaf {path}: {source}")]
    Io { path: PathBuf, source: std::io::Error },
    /// A loaf payload is malformed or belongs to an unsupported schema.
    #[error("invalid Oven Loaf {path}: {message}")]
    InvalidLoaf { path: PathBuf, message: String },
    /// The declared closure is not a valid direct-rustc artifact plan.
    #[error(transparent)]
    Plan(#[from] OvenRustcError),
    /// Bounded publication refused the requested immutable closure.
    #[error(transparent)]
    Store(#[from] OvenStoreError),
    /// A release-stage Loaf could not be assembled safely.
    #[error("failed to prepare Oven Loaf: {message}")]
    Preparation { message: String },
}

/// Validate the committed envelope authority and return only its content-addressed Loaf manifests.
pub(crate) fn committed_loaf_paths(loaf_root: &Path) -> Result<Vec<PathBuf>, OvenLoafError> {
    let paths = committed_loaf_metadata_paths(loaf_root)?;
    for path in &paths {
        let loaf = read_loaf(path)?;
        validate_loaf_declared_file_set(&loaf, path)?;
    }
    Ok(paths)
}

/// Validate the typed committed-envelope authority without traversing every Loaf artifact tree.
///
/// Selection needs only content-addressed metadata and compatibility records to choose one closure. The selected
/// Loaf then validates every manifest-declared Rustc input before it is passed to Rustc; it deliberately does not
/// walk unrelated files below the immutable Loaf directory. Full-generation consumers use [`committed_loaf_paths`]
/// to reject any undeclared file during an explicit whole-Loaf audit.
fn committed_loaf_metadata_paths(loaf_root: &Path) -> Result<Vec<PathBuf>, OvenLoafError> {
    committed_loaf_metadata_paths_with_role(loaf_root, None)
}

/// Resolve only members that provide one authority while still validating the complete envelope.
fn committed_loaf_metadata_paths_for_authority(
    loaf_root: &Path,
    role: OvenLoafMemberRole,
) -> Result<Vec<PathBuf>, OvenLoafError> {
    committed_loaf_metadata_paths_with_role(loaf_root, Some(role))
}

/// Validate a committed envelope and retain the members selected by an optional authority role.
fn committed_loaf_metadata_paths_with_role(
    loaf_root: &Path,
    role: Option<OvenLoafMemberRole>,
) -> Result<Vec<PathBuf>, OvenLoafError> {
    let manifest_path = loaf_root.join("envelope.json");
    let manifest = match fs::read(&manifest_path) {
        Ok(bytes) => {
            serde_json::from_slice::<OvenLoafEnvelopeManifest>(&bytes).map_err(|error| OvenLoafError::InvalidLoaf {
                path: manifest_path.clone(),
                message: format!("invalid committed envelope manifest: {error}"),
            })?
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => {
            return Err(OvenLoafError::Io {
                path: manifest_path,
                source,
            });
        }
    };
    if manifest.schema_version != OVEN_LOAF_ENVELOPE_MANIFEST_SCHEMA_VERSION {
        return Err(OvenLoafError::InvalidLoaf {
            path: manifest_path,
            message: format!("unsupported envelope manifest schema {}", manifest.schema_version),
        });
    }
    let generation_prefix = Path::new("generations").join(
        manifest
            .generation_identity
            .strip_prefix("sha256:")
            .unwrap_or(&manifest.generation_identity),
    );
    let mut paths = Vec::with_capacity(manifest.loafs.len());
    for member in &manifest.loafs {
        if member.path.is_absolute()
            || member.path.components().any(|component| {
                matches!(
                    component,
                    std::path::Component::ParentDir | std::path::Component::RootDir | std::path::Component::Prefix(_)
                )
            })
            || !member.path.starts_with(&generation_prefix)
            || member.path.file_name().and_then(|name| name.to_str()) != Some("loaf.json")
        {
            return Err(OvenLoafError::InvalidLoaf {
                path: manifest_path,
                message: format!("envelope member `{}` has an unsafe or foreign path", member.label),
            });
        }
        let path = loaf_root.join(&member.path);
        if !path.is_file() {
            return Err(OvenLoafError::InvalidLoaf {
                path,
                message: format!("committed envelope member `{}` is missing", member.label),
            });
        }
        let identity = loaf_file_identity(&path)?;
        let expected_name = format!(
            "{}.loaf",
            member
                .loaf_identity
                .strip_prefix("sha256:")
                .unwrap_or(&member.loaf_identity)
        );
        if identity != member.loaf_identity
            || path.parent().and_then(Path::file_name).and_then(|name| name.to_str()) != Some(expected_name.as_str())
        {
            return Err(OvenLoafError::InvalidLoaf {
                path,
                message: format!("committed envelope member `{}` is not content-addressed", member.label),
            });
        }
        if role.is_none_or(|expected| match expected {
            OvenLoafMemberRole::CompiledClosure => member.role.provides_compiled_closure(),
            OvenLoafMemberRole::SourceAuthority => member.role.provides_source_authority(),
            OvenLoafMemberRole::CompiledClosureAndSourceAuthority => {
                member.role == OvenLoafMemberRole::CompiledClosureAndSourceAuthority
            }
        }) {
            paths.push(path);
        }
    }
    Ok(paths)
}

/// Read and validate the one atomically committed typed envelope manifest.
fn committed_loaf_envelope_manifest(
    loaf_root: &Path,
    expected_envelope: &str,
) -> Result<(OvenLoafEnvelopeManifest, PathBuf), OvenLoafError> {
    let manifest_path = loaf_root.join("envelope.json");
    let bytes = fs::read(&manifest_path).map_err(|source| OvenLoafError::Io {
        path: manifest_path.clone(),
        source,
    })?;
    let manifest =
        serde_json::from_slice::<OvenLoafEnvelopeManifest>(&bytes).map_err(|error| OvenLoafError::InvalidLoaf {
            path: manifest_path.clone(),
            message: format!("invalid committed envelope manifest: {error}"),
        })?;
    if manifest.schema_version != OVEN_LOAF_ENVELOPE_MANIFEST_SCHEMA_VERSION {
        return Err(OvenLoafError::InvalidLoaf {
            path: manifest_path,
            message: format!("unsupported envelope manifest schema {}", manifest.schema_version),
        });
    }
    if manifest.envelope != expected_envelope {
        return Err(OvenLoafError::InvalidLoaf {
            path: manifest_path,
            message: format!(
                "committed envelope is `{}`, expected `{expected_envelope}`",
                manifest.envelope
            ),
        });
    }
    let generation_digest =
        manifest
            .generation_identity
            .strip_prefix("sha256:")
            .ok_or_else(|| OvenLoafError::InvalidLoaf {
                path: manifest_path.clone(),
                message: "committed envelope generation identity is not a SHA-256 digest".to_string(),
            })?;
    if generation_digest.len() != 64
        || !generation_digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(OvenLoafError::InvalidLoaf {
            path: manifest_path,
            message: "committed envelope generation identity is not a canonical SHA-256 digest".to_string(),
        });
    }
    Ok((manifest, manifest_path))
}

/// Return the stable compiler-suite compatibility identity of the sealed member set.
///
/// A compiler-suite receipt needs to invalidate when a selected Loaf closure or direct-Rustc plan changes, but not
/// merely because envelope publication evidence changed. In particular, editing an `#[cfg(test)]` compiler source
/// can change the executable digest while leaving every lock/toolchain-bound member unchanged. Key the costly
/// compiler-suite foundation to its selected member identities, never to the enclosing generation path, accounting,
/// or evidence map.
pub(crate) fn committed_loaf_envelope_compatibility_identity(
    loaf_root: &Path,
    expected_envelope: &str,
) -> Result<String, OvenLoafError> {
    let (manifest, manifest_path) = committed_loaf_envelope_manifest(loaf_root, expected_envelope)?;
    let mut members = Vec::with_capacity(manifest.loafs.len());
    let mut variants = BTreeSet::new();
    for member in &manifest.loafs {
        if member.label.trim().is_empty()
            || member.profile.trim().is_empty()
            || member.action.trim().is_empty()
            || member.build_unit_identity.trim().is_empty()
            || member.loaf_identity.trim().is_empty()
            || member.plan_identity.trim().is_empty()
        {
            return Err(OvenLoafError::InvalidLoaf {
                path: manifest_path.clone(),
                message: "committed envelope has an incomplete compatibility member".to_string(),
            });
        }
        if !variants.insert((member.label.clone(), member.profile.clone())) {
            return Err(OvenLoafError::InvalidLoaf {
                path: manifest_path.clone(),
                message: format!(
                    "committed envelope repeats Loaf family `{}` for profile `{}`",
                    member.label, member.profile
                ),
            });
        }
        members.push((
            &member.label,
            &member.profile,
            &member.action,
            member.role,
            &member.build_unit_identity,
            &member.loaf_identity,
            &member.plan_identity,
        ));
    }
    members.sort_by(|left, right| left.0.cmp(right.0).then_with(|| left.1.cmp(right.1)));
    let encoded = serde_json::to_vec(&(manifest.schema_version, &manifest.envelope, members)).map_err(|error| {
        OvenLoafError::Preparation {
            message: format!("could not encode committed envelope compatibility identity: {error}"),
        }
    })?;
    Ok(digest_bytes(&encoded))
}

/// One committed Loaf generation retained under its shared lock.
pub(crate) struct OvenCommittedLoafGeneration {
    generation_identity: String,
    _lock: OvenLoafGenerationLock,
}

impl OvenCommittedLoafGeneration {
    /// Return the exact committed envelope generation protected by this shared lock.
    pub(crate) fn generation_identity(&self) -> &str {
        &self.generation_identity
    }
}

/// Acquire a shared generation lock and resolve the complete currently committed Loaf set.
pub(crate) fn acquire_committed_loaf_generation(
    loaf_root: &Path,
) -> Result<Option<OvenCommittedLoafGeneration>, OvenLoafError> {
    if !loaf_root.join("envelope.json").is_file() {
        return Ok(None);
    }
    let generation_lock = acquire_loaf_generation_lock(loaf_root)?;
    let (manifest, _) = committed_loaf_envelope_manifest(loaf_root, "compiler-suite")?;
    committed_loaf_paths(loaf_root)?;
    Ok(Some(OvenCommittedLoafGeneration {
        generation_identity: manifest.generation_identity,
        _lock: generation_lock,
    }))
}

/// Find the one committed Loaf whose exact build-unit identity matches `receipt`.
fn exact_committed_loaf_path(loaf_root: &Path, receipt: &OvenReceipt) -> Result<Option<PathBuf>, OvenLoafError> {
    for path in committed_loaf_metadata_paths_for_authority(loaf_root, OvenLoafMemberRole::CompiledClosure)? {
        let loaf = read_loaf(&path)?;
        if loaf.build_unit_identity == receipt.build_unit_identity {
            return Ok(Some(path));
        }
    }
    Ok(None)
}

/// Hold one committed generation stable while a consumer verifies and uses its Loafs.
fn acquire_loaf_generation_lock(loaf_root: &Path) -> Result<OvenLoafGenerationLock, OvenLoafError> {
    let path = loaf_root.join(OVEN_LOAF_ENVELOPE_LOCK_FILE);
    let file = File::open(&path).map_err(|source| OvenLoafError::Io {
        path: path.clone(),
        source,
    })?;
    file.lock_shared()
        .map_err(|source| OvenLoafError::Io { path, source })?;
    Ok(OvenLoafGenerationLock { file })
}

/// Check whether one already validated compiler-native loaf can supply all caller-visible registry imports from its
/// own exact catalog. A missing or incompatible leaf disqualifies this loaf; it never widens the caller to Cargo.
fn registry_dependencies_supported_by_loaf(
    native: &OvenToolchainLoaf,
    dependencies: &[&DependencySpec],
    profile: &str,
) -> bool {
    let authority = native.registry_leaf_authority();
    dependencies
        .iter()
        .all(|dependency| validate_sealed_registry_leaf(dependency, Some(&authority), profile).is_ok())
}

/// Registry capability required while choosing a compatible Loaf.
#[derive(Clone, Copy)]
enum OvenLoafRegistryRequirement {
    LinkableLeaf,
}

/// Resolve a compiler-owned full-stdlib Loaf for direct execution without copying it into a mutable Oven store.
///
/// Selection validates the immutable generation, full receipt compatibility, and every caller-visible registry root
/// before it returns a plan. It is not a dependency resolver: a project dependency absent from the sealed Loaf still
/// requires an explicit project bake.
pub fn resolve_toolchain_loaf(
    receipt: &OvenReceipt,
    selection: OvenLoafSelection,
) -> Result<Option<OvenToolchainLoaf>, OvenLoafError> {
    select_toolchain_loaf(receipt, selection, &[], OvenLoafRegistryRequirement::LinkableLeaf)
}

/// Return whether a project receipt requests the source compiler's vocabulary helper.
fn receipt_requests_source_compiler_vocab_support(receipt: &OvenReceipt) -> bool {
    receipt
        .sources
        .build_unit_inputs
        .get(OVEN_SOURCE_COMPILER_VOCAB_SUPPORT_BUILD_INPUT)
        .is_some_and(|value| value == "v1")
}

/// Return whether this compiler-owned Loaf seals the vocabulary helper required by a source-built project.
fn compiler_loaf_supplies_source_compiler_vocab_support(
    loaf: &OvenToolchainLoaf,
    intent: &super::OvenBuildIntent,
) -> bool {
    loaf.artifacts.vocab_auxiliary_targets.iter().any(|target| {
        target.target == intent.target
            && ["incan_vocab", "serde_json"]
                .into_iter()
                .all(|crate_name| target.externs.iter().any(|artifact| artifact.crate_name == crate_name))
    })
}

/// Resolve a compiler-owned full-stdlib Loaf whose linkable catalog satisfies every caller registry root.
pub fn resolve_toolchain_loaf_for_registry_dependencies(
    receipt: &OvenReceipt,
    selection: OvenLoafSelection,
    dependencies: &[DependencySpec],
) -> Result<Option<OvenToolchainLoaf>, OvenLoafError> {
    select_toolchain_loaf(
        receipt,
        selection,
        dependencies,
        OvenLoafRegistryRequirement::LinkableLeaf,
    )
}

/// Resolve a compiler-owned Loaf while treating the source-only vocabulary marker as a capability requirement,
/// rather than a release-cohort change.
///
/// A source-built compiler marks a project only when it may need to publish its own vocabulary helper. That marker
/// is removed before compiler-owned selection, then the selected Loaf must prove that it seals both helper crates
/// for the receipt target. Keeping this rule beside Loaf selection prevents Rust-inspection and direct-Rustc
/// preparation from disagreeing about the same immutable compiler closure.
pub fn resolve_compiler_owned_loaf_for_registry_dependencies(
    receipt: &OvenReceipt,
    dependencies: &[DependencySpec],
) -> Result<Option<OvenToolchainLoaf>, OvenLoafError> {
    if receipt_requests_source_compiler_vocab_support(receipt) {
        let base_receipt = receipt_without_build_unit_input(receipt, OVEN_SOURCE_COMPILER_VOCAB_SUPPORT_BUILD_INPUT)
            .map_err(|error| OvenLoafError::Preparation {
                message: format!("failed to derive source-vocabulary Loaf receipt: {error}"),
            })?;
        let selected = resolve_toolchain_loaf_for_registry_dependencies(
            &base_receipt,
            OvenLoafSelection::CompilerOwnedProviderSuperset,
            dependencies,
        )?;
        return Ok(selected.filter(|loaf| compiler_loaf_supplies_source_compiler_vocab_support(loaf, &receipt.intent)));
    }
    resolve_toolchain_loaf_for_registry_dependencies(
        receipt,
        OvenLoafSelection::CompilerOwnedProviderSuperset,
        dependencies,
    )
}

/// Resolve one compiler-shipped Loaf by the content address recorded in a project extension payload.
///
/// This is intentionally stricter than ordinary compatible-Loaf selection.  An extension is valid only with the
/// exact base it was partitioned against; choosing a newer or merely similarly capable Loaf could redirect a
/// direct-Rustc relative path to different metadata.  The returned value retains the generation lock until its
/// consuming execution finishes.
pub fn resolve_toolchain_loaf_by_identity(
    receipt: &OvenReceipt,
    loaf_identity: &str,
) -> Result<Option<OvenToolchainLoaf>, OvenLoafError> {
    let loaf_root = crate::toolchain_layout::resolve_toolchain_data_path(Path::new(TOOLCHAIN_LOAF_RELATIVE_ROOT));
    if !loaf_root.join("envelope.json").is_file() {
        return Ok(None);
    }
    let generation_lock = acquire_loaf_generation_lock(&loaf_root)?;
    for loaf_path in committed_loaf_metadata_paths_for_authority(&loaf_root, OvenLoafMemberRole::CompiledClosure)? {
        if loaf_file_identity(&loaf_path)? != loaf_identity {
            continue;
        }
        return loaf_from_loaf_with_lock(
            receipt,
            &loaf_path,
            OvenLoafSelection::CompilerOwnedProviderSuperset,
            Some(generation_lock),
        )
        .map(Some);
    }
    Ok(None)
}

/// Resolve the compiler-owned base recorded by a project extension under the source-vocabulary capability rule.
///
/// The extension still pins `loaf_identity`; removing the marker only compares the release-cohort inputs shared by
/// the project and its compiler-owned base.
pub fn resolve_compiler_owned_loaf_by_identity(
    receipt: &OvenReceipt,
    loaf_identity: &str,
) -> Result<Option<OvenToolchainLoaf>, OvenLoafError> {
    if receipt_requests_source_compiler_vocab_support(receipt) {
        let base_receipt = receipt_without_build_unit_input(receipt, OVEN_SOURCE_COMPILER_VOCAB_SUPPORT_BUILD_INPUT)
            .map_err(|error| OvenLoafError::Preparation {
                message: format!("failed to derive source-vocabulary Loaf receipt: {error}"),
            })?;
        let selected = resolve_toolchain_loaf_by_identity(&base_receipt, loaf_identity)?;
        return Ok(selected.filter(|loaf| compiler_loaf_supplies_source_compiler_vocab_support(loaf, &receipt.intent)));
    }
    resolve_toolchain_loaf_by_identity(receipt, loaf_identity)
}

/// Resolve a scheduler-held source-authority Loaf whose immutable source catalog satisfies every registry root.
///
/// Source inspection must not widen direct-`rustc` linkage. This selector therefore chooses only envelope members
/// explicitly marked [`OvenLoafMemberRole::SourceAuthority`], while normal commands continue to select one coherent
/// compiled closure through [`resolve_toolchain_loaf_for_registry_dependencies`].
pub fn resolve_toolchain_loaf_for_registry_sources(
    receipt: &OvenReceipt,
    dependencies: &[DependencySpec],
) -> Result<Option<OvenToolchainLoaf>, OvenLoafError> {
    let registry_dependencies = dependencies
        .iter()
        .filter(|dependency| matches!(dependency.source, DependencySource::Registry))
        .collect::<Vec<_>>();
    let loaf_root = crate::toolchain_layout::resolve_toolchain_data_path(Path::new(TOOLCHAIN_LOAF_RELATIVE_ROOT));
    if !loaf_root.join("envelope.json").is_file() {
        return Ok(None);
    }
    let generation_lock = acquire_loaf_generation_lock(&loaf_root)?;
    let mut candidates = Vec::new();
    for loaf_path in committed_loaf_metadata_paths_for_authority(&loaf_root, OvenLoafMemberRole::SourceAuthority)? {
        let loaf = read_loaf(&loaf_path)?;
        if loaf.schema_version != OVEN_LOAF_SCHEMA_VERSION
            || loaf.plan.intent != receipt.intent
            || !loaf.compatibility.authorizes_source_authority(receipt)?
            || !registry_source_dependencies_supported_by_catalog(&loaf.plan.registry_sources, &registry_dependencies)
        {
            continue;
        }
        candidates.push((loaf.plan.registry_sources.len(), loaf_path));
    }
    candidates.sort();
    let Some((_, loaf_path)) = candidates.into_iter().next() else {
        return Ok(None);
    };
    source_authority_loaf_from_loaf_with_lock(receipt, &loaf_path, Some(generation_lock)).map(Some)
}

/// Select one receipt-compatible compiler-owned Loaf under a shared generation lock.
///
/// Exact identity, provider-superset compatibility, registry-catalog admission, and deterministic tie-breaking live
/// here once. Materializing into a bounded caller store and consuming immutable toolchain data are downstream
/// ownership choices; neither may reinterpret compatibility.
fn select_toolchain_loaf(
    receipt: &OvenReceipt,
    selection: OvenLoafSelection,
    dependencies: &[DependencySpec],
    registry_requirement: OvenLoafRegistryRequirement,
) -> Result<Option<OvenToolchainLoaf>, OvenLoafError> {
    let registry_dependencies = dependencies
        .iter()
        .filter(|dependency| matches!(dependency.source, DependencySource::Registry))
        .collect::<Vec<_>>();
    let loaf_root = crate::toolchain_layout::resolve_toolchain_data_path(Path::new(TOOLCHAIN_LOAF_RELATIVE_ROOT));
    if !loaf_root.join("envelope.json").is_file() {
        return Ok(None);
    }
    let generation_lock = acquire_loaf_generation_lock(&loaf_root)?;
    if let Some(loaf_path) = exact_committed_loaf_path(&loaf_root, receipt)? {
        let native = loaf_from_loaf_with_lock(receipt, &loaf_path, OvenLoafSelection::Exact, Some(generation_lock))?;
        let supported = match registry_requirement {
            OvenLoafRegistryRequirement::LinkableLeaf => {
                registry_dependencies_supported_by_loaf(&native, &registry_dependencies, &receipt.intent.profile)
            }
        };
        return Ok(supported.then_some(native));
    }
    if selection == OvenLoafSelection::Exact {
        return Ok(None);
    }

    let candidates = compatible_loaf_paths(&loaf_root, receipt)?;
    let mut supported = Vec::new();
    for candidate in candidates {
        let native = loaf_from_loaf(
            receipt,
            &candidate.path,
            OvenLoafSelection::CompilerOwnedProviderSuperset,
        )?;
        let candidate_supported = match registry_requirement {
            OvenLoafRegistryRequirement::LinkableLeaf => {
                registry_dependencies_supported_by_loaf(&native, &registry_dependencies, &receipt.intent.profile)
            }
        };
        if candidate_supported {
            supported.push(candidate);
        }
    }
    let Some(candidate) = select_most_specific_compatible_loaf(supported) else {
        return Ok(None);
    };
    loaf_from_loaf_with_lock(
        receipt,
        &candidate.path,
        OvenLoafSelection::CompilerOwnedProviderSuperset,
        Some(generation_lock),
    )
    .map(Some)
}

/// Return every compiler-owned loaf that authorizes the narrow runtime-provider subset rule.
fn compatible_loaf_paths(loaf_root: &Path, receipt: &OvenReceipt) -> Result<Vec<CompatibleLoaf>, OvenLoafError> {
    let mut candidates = Vec::new();
    for loaf_path in committed_loaf_metadata_paths_for_authority(loaf_root, OvenLoafMemberRole::CompiledClosure)? {
        let loaf = read_loaf(&loaf_path)?;
        if loaf.schema_version != OVEN_LOAF_SCHEMA_VERSION || loaf.build_unit_identity == receipt.build_unit_identity {
            continue;
        }
        if loaf.plan.intent == receipt.intent
            && let Some(excess) = loaf.compatibility.provider_subset_excess(receipt)?
        {
            candidates.push(CompatibleLoaf {
                path: loaf_path,
                excess,
            });
        }
    }
    candidates.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(candidates)
}

/// Select the narrowest compatible compiler-owned loaf, with a path tie-breaker for reproducibility.
///
/// Every candidate has already matched all runtime inputs and contains every requested provider module/facet. The
/// tie-breaker therefore cannot widen the authority of the request; it only prevents independent shipped provider
/// families from making a no-provider request fail arbitrarily.
fn select_most_specific_compatible_loaf(mut candidates: Vec<CompatibleLoaf>) -> Option<CompatibleLoaf> {
    candidates.sort_by(|left, right| left.excess.cmp(&right.excess).then_with(|| left.path.cmp(&right.path)));
    candidates.into_iter().next()
}

/// Select the narrowest compatible Loaf after satisfying any caller-visible registry requirement.
///
/// A registry-free caller has no catalog predicate to prove, so it must not validate every compatible immutable
/// closure merely to evaluate an empty conjunction. The final caller validates only the selected Loaf before Rustc
/// receives any artifact path.
#[cfg(test)]
fn select_compatible_loaf_with_registry_requirement(
    candidates: Vec<CompatibleLoaf>,
    registry_dependencies: &[&DependencySpec],
    mut supports_registry_dependencies: impl FnMut(&CompatibleLoaf) -> Result<bool, OvenLoafError>,
) -> Result<Option<CompatibleLoaf>, OvenLoafError> {
    if registry_dependencies.is_empty() {
        return Ok(select_most_specific_compatible_loaf(candidates));
    }
    let mut supported = Vec::new();
    for candidate in candidates {
        if supports_registry_dependencies(&candidate)? {
            supported.push(candidate);
        }
    }
    Ok(select_most_specific_compatible_loaf(supported))
}

/// Verify that one Loaf authorizes `receipt` and resolve its compiler-owned direct-Rustc closure.
///
/// This is the normal consumer boundary. It verifies the content-addressed manifest, the receipt/compatibility
/// relationship, the registry catalog, and every declared file used by the resulting Rustc plan. It intentionally
/// does not recursively inspect unrelated files in the immutable directory: those files cannot become a Rustc input
/// through the sealed manifest, while walking complete source trees on every command would make a prepared Loaf
/// behave like a cold cache. [`committed_loaf_paths`] retains the explicit whole-Loaf audit for publication and
/// inspection flows.
fn loaf_from_loaf(
    receipt: &OvenReceipt,
    loaf_path: &Path,
    selection: OvenLoafSelection,
) -> Result<OvenToolchainLoaf, OvenLoafError> {
    loaf_from_loaf_with_lock(receipt, loaf_path, selection, None)
}

/// Resolve one receipt-authorized Loaf while retaining an optional generation-lifetime lock.
fn loaf_from_loaf_with_lock(
    receipt: &OvenReceipt,
    loaf_path: &Path,
    selection: OvenLoafSelection,
    generation_lock: Option<OvenLoafGenerationLock>,
) -> Result<OvenToolchainLoaf, OvenLoafError> {
    receipt.verify_identity().map_err(|error| OvenLoafError::InvalidLoaf {
        path: loaf_path.to_path_buf(),
        message: format!("requested receipt is invalid: {error}"),
    })?;
    let loaf = read_loaf(loaf_path)?;
    if loaf.schema_version != OVEN_LOAF_SCHEMA_VERSION {
        return Err(OvenLoafError::InvalidLoaf {
            path: loaf_path.to_path_buf(),
            message: format!(
                "schema version {} is unsupported (expected {})",
                loaf.schema_version, OVEN_LOAF_SCHEMA_VERSION
            ),
        });
    }
    let exact_identity = loaf.build_unit_identity == receipt.build_unit_identity;
    let compatible_provider_subset = selection == OvenLoafSelection::CompilerOwnedProviderSuperset
        && loaf.compatibility.authorizes_provider_subset(receipt)?;
    if !exact_identity && !compatible_provider_subset {
        return Err(OvenLoafError::InvalidLoaf {
            path: loaf_path.to_path_buf(),
            message: "build-unit identity does not authorize the requested receipt or provider-subset runtime"
                .to_string(),
        });
    }
    if loaf.plan.intent != receipt.intent {
        return Err(OvenLoafError::InvalidLoaf {
            path: loaf_path.to_path_buf(),
            message: "direct-rustc intent does not authorize the requested receipt".to_string(),
        });
    }
    if loaf.plan.registry_leaves != loaf.registry_leaves {
        return Err(OvenLoafError::InvalidLoaf {
            path: loaf_path.to_path_buf(),
            message: "loaf registry catalog does not match its copied direct-rustc plan".to_string(),
        });
    }
    validate_registry_leaf_catalog(&loaf, loaf_path)?;
    let loaf_identity = loaf_file_identity(loaf_path)?;
    let artifact_root = loaf_path.parent().ok_or_else(|| OvenLoafError::InvalidLoaf {
        path: loaf_path.to_path_buf(),
        message: "loaf file has no parent directory".to_string(),
    })?;
    let artifact_plan = loaf.plan.materialize_trusted_store(artifact_root, &receipt.intent)?;
    Ok(OvenToolchainLoaf {
        loaf_identity,
        loaf_build_unit_identity: loaf.build_unit_identity,
        artifacts: loaf.plan,
        registry_leaves: loaf.registry_leaves,
        artifact_root: artifact_root.to_path_buf(),
        artifact_plan,
        _generation_lock: generation_lock,
    })
}

/// Resolve a receipt-compatible source-authority Loaf without treating its catalog as a linkable closure.
///
/// The envelope has already restricted this path to a source-authority member. This second verification makes that
/// role meaningful at the trust boundary: source inspection accepts exact runtime provenance and intent, while
/// direct-`rustc` callers must still use [`loaf_from_loaf_with_lock`] and its provider/leaf compatibility checks.
/// Like executable selection, this validates only manifest-declared source and artifact paths; an explicit
/// whole-Loaf audit owns undeclared-file discovery.
fn source_authority_loaf_from_loaf_with_lock(
    receipt: &OvenReceipt,
    loaf_path: &Path,
    generation_lock: Option<OvenLoafGenerationLock>,
) -> Result<OvenToolchainLoaf, OvenLoafError> {
    receipt.verify_identity().map_err(|error| OvenLoafError::InvalidLoaf {
        path: loaf_path.to_path_buf(),
        message: format!("requested receipt is invalid: {error}"),
    })?;
    let loaf = read_loaf(loaf_path)?;
    if loaf.schema_version != OVEN_LOAF_SCHEMA_VERSION {
        return Err(OvenLoafError::InvalidLoaf {
            path: loaf_path.to_path_buf(),
            message: format!(
                "schema version {} is unsupported (expected {})",
                loaf.schema_version, OVEN_LOAF_SCHEMA_VERSION
            ),
        });
    }
    if !loaf.compatibility.authorizes_source_authority(receipt)? {
        return Err(OvenLoafError::InvalidLoaf {
            path: loaf_path.to_path_buf(),
            message: "source authority does not authorize the requested runtime provenance".to_string(),
        });
    }
    if loaf.plan.intent != receipt.intent {
        return Err(OvenLoafError::InvalidLoaf {
            path: loaf_path.to_path_buf(),
            message: "source-authority direct-rustc intent does not authorize the requested receipt".to_string(),
        });
    }
    if loaf.plan.registry_leaves != loaf.registry_leaves {
        return Err(OvenLoafError::InvalidLoaf {
            path: loaf_path.to_path_buf(),
            message: "source-authority registry catalog does not match its copied direct-rustc plan".to_string(),
        });
    }
    validate_registry_leaf_catalog(&loaf, loaf_path)?;
    let loaf_identity = loaf_file_identity(loaf_path)?;
    let artifact_root = loaf_path.parent().ok_or_else(|| OvenLoafError::InvalidLoaf {
        path: loaf_path.to_path_buf(),
        message: "source-authority loaf file has no parent directory".to_string(),
    })?;
    let artifact_plan = loaf.plan.materialize_trusted_store(artifact_root, &receipt.intent)?;
    Ok(OvenToolchainLoaf {
        loaf_identity,
        loaf_build_unit_identity: loaf.build_unit_identity,
        artifacts: loaf.plan,
        registry_leaves: loaf.registry_leaves,
        artifact_root: artifact_root.to_path_buf(),
        artifact_plan,
        _generation_lock: generation_lock,
    })
}

/// Reject undeclared, missing, non-portable, or symlinked files in one immutable Loaf directory.
fn validate_loaf_declared_file_set(loaf: &OvenLoaf, loaf_path: &Path) -> Result<(), OvenLoafError> {
    let artifact_root = loaf_path.parent().ok_or_else(|| OvenLoafError::InvalidLoaf {
        path: loaf_path.to_path_buf(),
        message: "loaf file has no parent directory".to_string(),
    })?;
    let mut expected = loaf
        .plan
        .declared_artifact_paths()?
        .into_iter()
        .collect::<BTreeSet<_>>();
    expected.insert("loaf.json".to_string());
    let mut pending = vec![artifact_root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(&directory).map_err(|source| OvenLoafError::Io {
            path: directory.clone(),
            source,
        })? {
            let entry = entry.map_err(|source| OvenLoafError::Io {
                path: directory.clone(),
                source,
            })?;
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path).map_err(|source| OvenLoafError::Io {
                path: path.clone(),
                source,
            })?;
            if metadata.file_type().is_symlink() {
                return Err(OvenLoafError::InvalidLoaf {
                    path,
                    message: "Loaf contains an undeclared symbolic link".to_string(),
                });
            }
            if metadata.is_dir() {
                pending.push(path);
                continue;
            }
            if !metadata.is_file() {
                return Err(OvenLoafError::InvalidLoaf {
                    path,
                    message: "Loaf contains an unsupported filesystem entry".to_string(),
                });
            }
            let relative = path
                .strip_prefix(artifact_root)
                .ok()
                .and_then(Path::to_str)
                .map(|value| value.replace('\\', "/"))
                .ok_or_else(|| OvenLoafError::InvalidLoaf {
                    path: path.clone(),
                    message: "Loaf contains a non-portable file path".to_string(),
                })?;
            if !expected.remove(&relative) {
                return Err(OvenLoafError::InvalidLoaf {
                    path,
                    message: format!("Loaf contains undeclared file `{relative}`"),
                });
            }
        }
    }
    if let Some(missing) = expected.into_iter().next() {
        return Err(OvenLoafError::InvalidLoaf {
            path: loaf_path.to_path_buf(),
            message: format!("Loaf is missing declared file `{missing}`"),
        });
    }
    Ok(())
}

/// Digest one regular `loaf.json` file into its canonical content identity.
fn loaf_file_identity(loaf_path: &Path) -> Result<String, OvenLoafError> {
    fs::read(loaf_path)
        .map(|bytes| digest_bytes(&bytes))
        .map_err(|source| OvenLoafError::Io {
            path: loaf_path.to_path_buf(),
            source,
        })
}

/// Reject registry catalog records that do not describe an artifact already sealed by the Loaf plan.
///
/// The direct-Rustc resolver may select a catalog leaf by package requirement. Binding every leaf to the plan keeps
/// that selection from becoming a second, less constrained artifact channel beside the receipt-owned closure.
fn validate_registry_leaf_catalog(loaf: &OvenLoaf, loaf_path: &Path) -> Result<(), OvenLoafError> {
    let mut plan_artifacts = BTreeMap::new();
    for artifact in loaf
        .plan
        .externs
        .iter()
        .map(|artifact| (&artifact.relative_path, &artifact.digest))
        .chain(
            loaf.plan
                .supporting_artifacts
                .iter()
                .map(|artifact| (&artifact.relative_path, &artifact.digest)),
        )
    {
        if plan_artifacts
            .insert(artifact.0.as_str(), artifact.1.as_str())
            .is_some()
        {
            return Err(OvenLoafError::InvalidLoaf {
                path: loaf_path.to_path_buf(),
                message: format!("direct-rustc plan declares artifact `{}` more than once", artifact.0),
            });
        }
    }

    let mut package_versions = BTreeSet::new();
    for leaf in &loaf.registry_leaves {
        if leaf.package.trim().is_empty() || leaf.version.trim().is_empty() {
            return Err(OvenLoafError::InvalidLoaf {
                path: loaf_path.to_path_buf(),
                message: "registry leaf package and version must not be empty".to_string(),
            });
        }
        if leaf.crate_name.trim().is_empty() || leaf.crate_name != leaf.artifact.crate_name {
            return Err(OvenLoafError::InvalidLoaf {
                path: loaf_path.to_path_buf(),
                message: format!(
                    "registry leaf `{}` `{}` has inconsistent crate identity",
                    leaf.package, leaf.version
                ),
            });
        }
        if !package_versions.insert((leaf.package.as_str(), leaf.version.as_str())) {
            return Err(OvenLoafError::InvalidLoaf {
                path: loaf_path.to_path_buf(),
                message: format!(
                    "registry catalog declares package `{}` version `{}` more than once",
                    leaf.package, leaf.version
                ),
            });
        }
        let mut features = BTreeSet::new();
        for feature in &leaf.features {
            if feature.trim().is_empty() || !features.insert(feature.as_str()) {
                return Err(OvenLoafError::InvalidLoaf {
                    path: loaf_path.to_path_buf(),
                    message: format!(
                        "registry leaf `{}` `{}` declares an empty or duplicate feature",
                        leaf.package, leaf.version
                    ),
                });
            }
        }
        if !leaf.source.registry.starts_with("registry+")
            || leaf.source.checksum.trim().is_empty()
            || leaf.source.digest.trim().is_empty()
        {
            return Err(OvenLoafError::InvalidLoaf {
                path: loaf_path.to_path_buf(),
                message: format!(
                    "registry leaf `{}` `{}` has incomplete registry source identity",
                    leaf.package, leaf.version
                ),
            });
        }
        let source_root = Path::new(&leaf.source.relative_root);
        if source_root.is_absolute()
            || source_root.as_os_str().is_empty()
            || source_root.components().any(|component| {
                matches!(
                    component,
                    std::path::Component::ParentDir | std::path::Component::RootDir | std::path::Component::Prefix(_)
                )
            })
        {
            return Err(OvenLoafError::InvalidLoaf {
                path: loaf_path.to_path_buf(),
                message: format!(
                    "registry leaf `{}` `{}` has an unsafe source root",
                    leaf.package, leaf.version
                ),
            });
        }
        let source_manifest = source_root.join("Cargo.toml").to_string_lossy().replace('\\', "/");
        if !plan_artifacts.contains_key(source_manifest.as_str()) {
            return Err(OvenLoafError::InvalidLoaf {
                path: loaf_path.to_path_buf(),
                message: format!(
                    "registry leaf `{}` `{}` source root is not declared by the direct-rustc plan",
                    leaf.package, leaf.version
                ),
            });
        }
        if Path::new(&leaf.artifact.relative_path)
            .extension()
            .and_then(|extension| extension.to_str())
            != Some("rlib")
        {
            return Err(OvenLoafError::InvalidLoaf {
                path: loaf_path.to_path_buf(),
                message: format!(
                    "registry leaf `{}` `{}` must reference an rlib",
                    leaf.package, leaf.version
                ),
            });
        }
        match plan_artifacts.get(leaf.artifact.relative_path.as_str()) {
            Some(digest) if *digest == leaf.artifact.digest.as_str() => {}
            Some(_) => {
                return Err(OvenLoafError::InvalidLoaf {
                    path: loaf_path.to_path_buf(),
                    message: format!(
                        "registry leaf `{}` `{}` has a digest that disagrees with its sealed direct-rustc plan artifact",
                        leaf.package, leaf.version
                    ),
                });
            }
            None => {
                return Err(OvenLoafError::InvalidLoaf {
                    path: loaf_path.to_path_buf(),
                    message: format!(
                        "registry leaf `{}` `{}` references `{}`, which the sealed direct-rustc plan does not declare",
                        leaf.package, leaf.version, leaf.artifact.relative_path
                    ),
                });
            }
        }
    }
    Ok(())
}

/// Read one Loaf and attach the source path to any decoding failure.
fn read_loaf(loaf_path: &Path) -> Result<OvenLoaf, OvenLoafError> {
    let bytes = fs::read(loaf_path).map_err(|source| OvenLoafError::Io {
        path: loaf_path.to_path_buf(),
        source,
    })?;
    serde_json::from_slice::<OvenLoaf>(&bytes).map_err(|error| OvenLoafError::InvalidLoaf {
        path: loaf_path.to_path_buf(),
        message: format!("must be valid JSON: {error}"),
    })
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::mpsc;
    use std::thread;
    use std::time::Duration;

    use super::{
        CompatibleLoaf, OVEN_LOAF_ENVELOPE_MANIFEST_SCHEMA_VERSION, OVEN_LOAF_SCHEMA_VERSION, OvenLoaf,
        OvenLoafCompatibility, OvenLoafEnvelope, OvenLoafEnvelopeManifest, OvenLoafEnvelopeMember, OvenLoafError,
        OvenLoafFixtureAction, OvenLoafMemberRole, OvenLoafSelection, acquire_loaf_generation_lock,
        committed_loaf_envelope_compatibility_identity, committed_loaf_paths, digest_runtime_crate_source,
        loaf_envelope_specifications, loaf_from_loaf, registry_source_dependencies_supported_by_catalog,
        select_most_specific_compatible_loaf, validate_loaf_declared_file_set,
    };
    use crate::manifest::{DependencySource, DependencySpec};
    use crate::oven::rustc::{
        OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION, OvenRustcArtifactExtern, OvenRustcArtifactManifest,
        OvenRustcArtifactPlan, OvenRustcRegistryLeaf, OvenRustcRegistrySource, OvenRustcRegistrySourcePackage,
        OvenRustcSupportingArtifact,
    };
    use crate::oven::{OvenGeneratedProjectRequest, digest_bytes, digest_source_tree, receipt_generated_project};
    use incan_core::lang::stdlib::{self, StdlibExtraCrateSource};

    /// Acquire a writer lock only for reader-lifetime tests; production no longer owns an envelope publisher.
    fn acquire_test_exclusive_loaf_generation_lock(root: &Path) -> std::io::Result<fs::File> {
        fs::create_dir_all(root)?;
        let file = fs::File::options()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(root.join(super::OVEN_LOAF_ENVELOPE_LOCK_FILE))?;
        file.lock()?;
        Ok(file)
    }

    /// Return the canonical standard-library modules owned by checked SDK component sources.
    ///
    /// Component entrypoints are the source-of-truth provider surface. Normalizing their `*.prelude` implementation
    /// modules to their public facade mirrors provider publication, while `std.interop` is the intentionally
    /// source-less vocabulary-backed provider component.
    fn checked_stdlib_component_modules() -> Result<BTreeSet<String>, Box<dyn std::error::Error>> {
        let component_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("crates/incan_stdlib/stdlib/components");
        let mut modules = BTreeSet::from(["std.interop".to_string()]);
        for entry in fs::read_dir(&component_root)? {
            let entry = entry?;
            let source = entry.path().join("src/lib.incn");
            if !source.is_file() {
                continue;
            }
            for line in fs::read_to_string(&source)?.lines() {
                let Some(import) = line.trim().strip_prefix("import ") else {
                    continue;
                };
                let module = import
                    .split_whitespace()
                    .next()
                    .ok_or("stdlib component import has no module path")?;
                let module = match module.strip_suffix(".prelude") {
                    Some(facade) => facade,
                    None => module,
                };
                modules.insert(format!("std.{module}"));
            }
        }
        Ok(modules)
    }

    /// Return the standard-library imports a checked complete-stdlib fixture declares.
    fn checked_stdlib_fixture_imports(source: &str) -> BTreeSet<String> {
        source
            .lines()
            .filter_map(|line| {
                let line = line.trim();
                let module = line
                    .strip_prefix("import ")
                    .or_else(|| line.strip_prefix("from "))?
                    .split_whitespace()
                    .next()?;
                module.starts_with("std.").then(|| module.to_string())
            })
            .collect()
    }

    #[test]
    fn committed_envelope_ignores_unreferenced_generations_and_rejects_foreign_paths()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let committed_loaf = OvenLoaf {
            schema_version: OVEN_LOAF_SCHEMA_VERSION,
            build_unit_identity: "sha256:one".to_string(),
            provenance: Default::default(),
            accounting: Default::default(),
            compatibility: Default::default(),
            registry_leaves: Vec::new(),
            plan: OvenRustcArtifactManifest {
                schema_version: OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION,
                intent: crate::oven::OvenBuildIntent {
                    target: "fixture-target".to_string(),
                    toolchain: "fixture-rustc".to_string(),
                    profile: "release".to_string(),
                    features: Vec::new(),
                },
                dependency_search_paths: Vec::new(),
                native_search_paths: Vec::new(),
                externs: Vec::new(),
                entrypoint_externs: BTreeMap::new(),
                registry_leaves: Vec::new(),
                registry_sources: Vec::new(),
                compile_environment: BTreeMap::new(),
                vocab_auxiliary_targets: Vec::new(),
                supporting_artifacts: Vec::new(),
            },
        };
        let committed_bytes = serde_json::to_vec_pretty(&committed_loaf)?;
        let committed_identity = digest_bytes(&committed_bytes);
        let committed = PathBuf::from(format!(
            "generations/current/{}.loaf/loaf.json",
            committed_identity
                .strip_prefix("sha256:")
                .unwrap_or(&committed_identity)
        ));
        let stale = root.path().join("generations/stale/two.loaf/loaf.json");
        fs::create_dir_all(root.path().join(committed.parent().ok_or("committed parent missing")?))?;
        fs::create_dir_all(stale.parent().ok_or("stale parent missing")?)?;
        fs::write(root.path().join(&committed), &committed_bytes)?;
        fs::write(&stale, "{}")?;
        fs::write(
            root.path().join("envelope.json"),
            serde_json::to_vec(&OvenLoafEnvelopeManifest {
                schema_version: OVEN_LOAF_ENVELOPE_MANIFEST_SCHEMA_VERSION,
                envelope: "release".to_string(),
                generation_identity: "sha256:current".to_string(),
                evidence: BTreeMap::new(),
                loafs: vec![OvenLoafEnvelopeMember {
                    label: "one".to_string(),
                    profile: "release".to_string(),
                    action: "build".to_string(),
                    role: OvenLoafMemberRole::CompiledClosure,
                    build_unit_identity: "sha256:one".to_string(),
                    loaf_identity: committed_identity,
                    plan_identity: digest_bytes(&serde_json::to_vec(&committed_loaf.plan)?),
                    logical_bytes: committed_bytes.len() as u64,
                    physical_bytes: 0,
                    path: committed.clone(),
                }],
            })?,
        )?;
        assert_eq!(committed_loaf_paths(root.path())?, vec![root.path().join(&committed)]);

        fs::write(root.path().join(&committed), "{\"mutated\":true}")?;
        assert!(matches!(
            committed_loaf_paths(root.path()),
            Err(OvenLoafError::InvalidLoaf { .. })
        ));
        fs::write(root.path().join(&committed), &committed_bytes)?;
        let extra = root
            .path()
            .join(committed.parent().ok_or("committed parent missing")?)
            .join("undeclared.bin");
        fs::write(&extra, "undeclared")?;
        assert!(matches!(
            committed_loaf_paths(root.path()),
            Err(OvenLoafError::InvalidLoaf { .. })
        ));
        assert_eq!(
            super::committed_loaf_metadata_paths(root.path())?,
            vec![root.path().join(&committed)]
        );
        fs::remove_file(extra)?;

        let mut manifest: OvenLoafEnvelopeManifest =
            serde_json::from_slice(&fs::read(root.path().join("envelope.json"))?)?;
        manifest.loafs[0].path = PathBuf::from("../foreign.loaf/loaf.json");
        fs::write(root.path().join("envelope.json"), serde_json::to_vec(&manifest)?)?;
        assert!(matches!(
            committed_loaf_paths(root.path()),
            Err(OvenLoafError::InvalidLoaf { .. })
        ));
        Ok(())
    }

    /// Publish tiny real metadata for two native profiles and one source-only member.
    fn native_candidate_envelope(root: &Path) -> Result<OvenLoafEnvelopeManifest, Box<dyn std::error::Error>> {
        drop(acquire_test_exclusive_loaf_generation_lock(root)?);
        let generation_identity = digest_bytes(b"native candidate generation");
        let mut envelope = OvenLoafEnvelopeManifest {
            schema_version: OVEN_LOAF_ENVELOPE_MANIFEST_SCHEMA_VERSION,
            envelope: "release".to_string(),
            generation_identity: generation_identity.clone(),
            evidence: BTreeMap::new(),
            loafs: Vec::new(),
        };
        let receipt = runtime_receipt_for_plan()?;
        for (label, profile, role) in [
            ("debug-native", "debug", OvenLoafMemberRole::CompiledClosure),
            ("release-native", "release", OvenLoafMemberRole::CompiledClosure),
            ("inspection-only", "debug", OvenLoafMemberRole::SourceAuthority),
        ] {
            let mut plan = empty_manifest(&receipt);
            plan.intent.profile = profile.to_string();
            plan.dependency_search_paths.push("native".to_string());
            plan.externs.push(OvenRustcArtifactExtern {
                crate_name: "fixture".to_string(),
                relative_path: "native/libfixture.rlib".to_string(),
                digest: digest_bytes(label.as_bytes()),
            });
            plan.entrypoint_externs
                .insert("generated-root".to_string(), vec!["fixture".to_string()]);
            let loaf = OvenLoaf {
                schema_version: OVEN_LOAF_SCHEMA_VERSION,
                build_unit_identity: digest_bytes(label.as_bytes()),
                provenance: Default::default(),
                accounting: Default::default(),
                compatibility: OvenLoafCompatibility {
                    runtime_inputs: BTreeMap::from([("original-profile".to_string(), profile.to_string())]),
                    providers: Vec::new(),
                },
                registry_leaves: Vec::new(),
                plan,
            };
            let bytes = serde_json::to_vec_pretty(&loaf)?;
            let identity = digest_bytes(&bytes);
            let path = PathBuf::from("generations")
                .join(
                    generation_identity
                        .strip_prefix("sha256:")
                        .ok_or("fixture generation lacks prefix")?,
                )
                .join(format!(
                    "{}.loaf",
                    identity.strip_prefix("sha256:").ok_or("fixture loaf lacks prefix")?
                ))
                .join("loaf.json");
            let member_root = root.join(path.parent().ok_or("fixture member lacks parent")?);
            fs::create_dir_all(member_root.join("native"))?;
            fs::write(member_root.join("native/libfixture.rlib"), label)?;
            fs::write(root.join(&path), &bytes)?;
            envelope.loafs.push(OvenLoafEnvelopeMember {
                label: label.to_string(),
                profile: profile.to_string(),
                action: "build".to_string(),
                role,
                build_unit_identity: loaf.build_unit_identity,
                loaf_identity: identity,
                plan_identity: digest_bytes(&serde_json::to_vec(&loaf.plan)?),
                logical_bytes: u64::try_from(bytes.len())?,
                physical_bytes: 0,
                path,
            });
        }
        fs::write(root.join("envelope.json"), serde_json::to_vec(&envelope)?)?;
        Ok(envelope)
    }

    #[test]
    fn native_candidate_intake_keeps_original_facts_without_reading_native_files()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let envelope = native_candidate_envelope(root.path())?;
        let first_root = root
            .path()
            .join(envelope.loafs[0].path.parent().ok_or("fixture parent missing")?);
        fs::remove_file(first_root.join("native/libfixture.rlib"))?;
        fs::create_dir_all(root.path().join("generations/unreferenced"))?;
        fs::write(
            root.path().join("generations/unreferenced/loaf.json"),
            b"invalid unreferenced metadata",
        )?;
        let catalog = super::OvenNativeLoafCandidates::from_committed_envelope(root.path(), OvenLoafEnvelope::Release)?
            .ok_or("committed native candidate catalog missing")?;
        assert_eq!(catalog.generation_identity(), envelope.generation_identity);
        let candidates = catalog.candidates().collect::<Vec<_>>();
        assert_eq!(
            candidates.len(),
            2,
            "source-only metadata must not authorize native selection"
        );
        assert_eq!(candidates[0].member(), &envelope.loafs[0]);
        assert_eq!(candidates[1].member(), &envelope.loafs[1]);
        assert_eq!(candidates[0].metadata().plan.intent.profile, "debug");
        assert_eq!(candidates[1].metadata().plan.intent.profile, "release");
        assert_eq!(
            candidates[1]
                .metadata()
                .compatibility
                .runtime_inputs
                .get("original-profile"),
            Some(&"release".to_string()),
        );
        assert!(
            candidates[0].materialize().is_err(),
            "selected missing bytes must refuse"
        );
        let selected = candidates[1].materialize()?;
        assert_eq!(selected.candidate().member(), &envelope.loafs[1]);
        assert_eq!(selected.artifact_plan().externs.len(), 1);
        assert_eq!(selected.artifact_plan().externs[0].0, "fixture");
        fs::write(&selected.artifact_plan().externs[0].1, "changed after admission")?;
        assert!(
            candidates[1].materialize().is_err(),
            "current selected bytes must still be verified"
        );
        Ok(())
    }

    #[test]
    fn native_candidate_intake_rejects_envelope_coordinate_substitution() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let envelope = native_candidate_envelope(root.path())?;
        for coordinate in ["plan", "build", "profile", "path"] {
            let mut changed = envelope.clone();
            match coordinate {
                "plan" => changed.loafs[0].plan_identity = digest_bytes(b"other plan"),
                "build" => changed.loafs[0].build_unit_identity = digest_bytes(b"other build"),
                "profile" => changed.loafs[0].profile = "release".to_string(),
                "path" => changed.loafs.push(changed.loafs[0].clone()),
                _ => return Err("unknown fixture coordinate".into()),
            }
            fs::write(root.path().join("envelope.json"), serde_json::to_vec(&changed)?)?;
            assert!(
                matches!(
                    super::OvenNativeLoafCandidates::from_committed_envelope(root.path(), OvenLoafEnvelope::Release),
                    Err(OvenLoafError::InvalidLoaf { .. })
                ),
                "changed {coordinate} must refuse before providing candidate handles",
            );
        }
        Ok(())
    }

    /// Preserve the original Loaf owner through attachment and refuse changed or foreign native inputs.
    #[test]
    fn native_candidate_view_retains_original_loaf_origin_and_owner() -> Result<(), Box<dyn std::error::Error>> {
        use crate::oven::rustc::{OvenNativeInputOrigin, OvenNativeInputView};

        let root = tempfile::tempdir()?;
        let envelope = native_candidate_envelope(root.path())?;
        let catalog = super::OvenNativeLoafCandidates::from_committed_envelope(root.path(), OvenLoafEnvelope::Release)?
            .ok_or("committed native candidate catalog missing")?;
        let mut candidates = catalog.candidates();
        let selected = candidates
            .next()
            .ok_or("first native candidate missing")?
            .materialize()?;
        let other = candidates
            .next()
            .ok_or("second native candidate missing")?
            .materialize()?;
        drop(candidates);
        let view = OvenNativeInputView::from_materialized_loaf(&selected)?;
        let other_view = OvenNativeInputView::from_materialized_loaf(&other)?;
        match view.origin() {
            OvenNativeInputOrigin::ToolchainLoaf {
                generation_identity,
                member,
            } => {
                assert_eq!(generation_identity, envelope.generation_identity);
                assert_eq!(member, &envelope.loafs[0]);
                assert_eq!(member.plan_identity, selected.candidate().member().plan_identity);
            }
            OvenNativeInputOrigin::Store { .. } => return Err("Loaf origin became a synthetic store receipt".into()),
        }
        assert_eq!(view.identity(), envelope.loafs[0].loaf_identity);
        assert_eq!(view.build_unit_identity(), envelope.loafs[0].build_unit_identity);
        assert_eq!(view.intent(), &selected.candidate().metadata().plan.intent);
        assert_eq!(view.receipt_identity(), None);
        let chosen = view
            .named_candidates("generated-root")?
            .pop()
            .ok_or("declared native input missing")?
            .with_alias("fixture_alias".to_string())?;
        let attached = view.attach_for_source("generated-root", &[chosen])?;
        assert_eq!(attached.artifact_plan().externs.len(), 2);
        assert_eq!(attached.artifact_plan().externs[1].0, "fixture_alias");
        assert_eq!(
            attached.artifact_plan().dependency_search_paths,
            selected.artifact_plan().dependency_search_paths,
        );
        let foreign = other_view
            .named_candidates("generated-root")?
            .pop()
            .ok_or("other member input missing")?
            .with_alias("foreign_member".to_string())?;
        assert!(view.attach_for_source("generated-root", &[foreign]).is_err());

        let lock_file = fs::File::options()
            .read(true)
            .write(true)
            .open(root.path().join(super::OVEN_LOAF_ENVELOPE_LOCK_FILE))?;
        assert!(lock_file.try_lock().is_err());
        assert_eq!(fs::read(&attached.artifact_plan().externs[1].1)?, b"debug-native");
        drop(attached);
        drop(other_view);
        drop(view);

        let selected_file = &selected.artifact_plan().externs[0].1;
        fs::write(selected_file, b"changed after selected materialization")?;
        let borrowed = OvenNativeInputView::from_materialized_loaf(&selected)?;
        let changed = borrowed
            .named_candidates("generated-root")?
            .pop()
            .ok_or("original facts missing after mutation")?
            .with_alias("changed".to_string())?;
        assert!(
            borrowed.attach_for_source("generated-root", &[changed]).is_err(),
            "the adapter does not repeat a full walk, but selected attachment rechecks bytes",
        );
        drop(borrowed);
        drop(other);
        drop(selected);
        drop(catalog);
        lock_file.try_lock()?;
        lock_file.unlock()?;
        Ok(())
    }

    #[test]
    fn native_candidate_catalog_retains_generation_lock() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        native_candidate_envelope(root.path())?;
        let catalog = super::OvenNativeLoafCandidates::from_committed_envelope(root.path(), OvenLoafEnvelope::Release)?
            .ok_or("committed native candidate catalog missing")?;
        let candidate = catalog.candidates().next().ok_or("native candidate missing")?;
        let selected = candidate.materialize()?;
        let lock_file = fs::File::options()
            .read(true)
            .write(true)
            .open(root.path().join(super::OVEN_LOAF_ENVELOPE_LOCK_FILE))?;
        assert!(
            lock_file.try_lock().is_err(),
            "offered/materialized native inputs retain their actual generation"
        );
        assert_eq!(selected.artifact_plan().externs.len(), 1);
        drop(selected);
        drop(catalog);
        lock_file.try_lock()?;
        lock_file.unlock()?;
        Ok(())
    }

    #[test]
    fn committed_envelope_compatibility_tracks_members_not_generation_evidence()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let first_member = OvenLoafEnvelopeMember {
            label: "foundation-debug".to_string(),
            profile: "debug".to_string(),
            action: "run".to_string(),
            role: OvenLoafMemberRole::CompiledClosure,
            build_unit_identity: digest_bytes(b"foundation-build-unit"),
            loaf_identity: digest_bytes(b"foundation-loaf"),
            plan_identity: digest_bytes(b"foundation-plan"),
            logical_bytes: 1,
            physical_bytes: 1,
            path: PathBuf::from("generations/first/foundation.loaf/loaf.json"),
        };
        let write_manifest =
            |generation_identity: String, compiler_evidence: String, member: OvenLoafEnvelopeMember| {
                fs::write(
                    root.path().join("envelope.json"),
                    serde_json::to_vec(&OvenLoafEnvelopeManifest {
                        schema_version: OVEN_LOAF_ENVELOPE_MANIFEST_SCHEMA_VERSION,
                        envelope: "compiler-suite".to_string(),
                        generation_identity,
                        evidence: BTreeMap::from([("compiler_executable_digest".to_string(), compiler_evidence)]),
                        loafs: vec![member],
                    })?,
                )
            };

        write_manifest(
            digest_bytes(b"generation-one"),
            digest_bytes(b"compiler-one"),
            first_member.clone(),
        )?;
        let first = committed_loaf_envelope_compatibility_identity(root.path(), "compiler-suite")?;

        write_manifest(
            digest_bytes(b"generation-two"),
            digest_bytes(b"compiler-two"),
            first_member.clone(),
        )?;
        assert_eq!(
            committed_loaf_envelope_compatibility_identity(root.path(), "compiler-suite")?,
            first,
            "a changed compiler executable may require a new envelope generation but must not rebuild unchanged members"
        );

        let mut changed_member = first_member;
        changed_member.plan_identity = digest_bytes(b"changed-foundation-plan");
        write_manifest(
            digest_bytes(b"generation-three"),
            digest_bytes(b"compiler-three"),
            changed_member,
        )?;
        assert_ne!(
            committed_loaf_envelope_compatibility_identity(root.path(), "compiler-suite")?,
            first,
            "a changed sealed member plan must invalidate compiler-suite reuse"
        );
        Ok(())
    }

    #[test]
    fn active_generation_reader_blocks_replacement_until_release() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let exclusive = acquire_test_exclusive_loaf_generation_lock(root.path())?;
        drop(exclusive);
        let reader = acquire_loaf_generation_lock(root.path())?;
        let path = root.path().to_path_buf();
        let (sender, receiver) = mpsc::channel();
        let replacement = thread::spawn(move || {
            let lock = acquire_test_exclusive_loaf_generation_lock(&path);
            sender.send(lock.is_ok()).ok();
            lock
        });
        assert!(receiver.recv_timeout(Duration::from_millis(50)).is_err());
        drop(reader);
        assert!(receiver.recv_timeout(Duration::from_secs(2))?);
        drop(replacement.join().map_err(|_| "replacement thread panicked")??);
        Ok(())
    }

    #[test]
    fn built_in_envelopes_are_checked_complete_and_unambiguous() {
        for envelope in [OvenLoafEnvelope::Release, OvenLoafEnvelope::CompilerSuite] {
            let specifications = loaf_envelope_specifications(envelope);
            assert_eq!(specifications.len(), 2);
            let identities = specifications
                .iter()
                .map(|specification| (specification.label, specification.profile))
                .collect::<BTreeSet<_>>();
            assert_eq!(identities.len(), specifications.len());
            assert_eq!(identities, BTreeSet::from([("stdlib", "debug"), ("stdlib", "release")]));
            for specification in specifications {
                assert!(!specification.source.trim().is_empty());
                assert!(!specification.manifest.trim().is_empty());
                assert!(!specification.inspection_manifest.trim().is_empty());
                assert!(matches!(specification.profile, "debug" | "release"));
                assert!(
                    specification.profile != "debug" || specification.action != OvenLoafFixtureAction::Build,
                    "a debug Loaf fixture must use `run` so the canonical receipt records debug intent"
                );
                assert!(specification.manifest.contains(specification.project_name));
                assert_eq!(
                    specification.role,
                    OvenLoafMemberRole::CompiledClosureAndSourceAuthority
                );
                assert!(specification.retain_complete_registry_leaves);
                assert!(
                    specification.retain_checked_direct_dependencies,
                    "every `stdlib` Loaf must directly link the complete checked standard-library dependency surface"
                );
                for required_module in [
                    "std.async.channel",
                    "std.compression.zstd",
                    "std.datetime",
                    "std.datetime.runtime",
                    "std.encoding.base64",
                    "std.fs",
                    "std.interop",
                    "std.result",
                    "std.serde",
                    "std.telemetry",
                    "std.traits.callable",
                    "std.web",
                    "std.web.routing",
                ] {
                    assert!(
                        specification.source.contains(required_module),
                        "the full release-version provider Loaf must retain `{required_module}`"
                    );
                }
                for required_facade in ["std.datetime", "std.fs", "std.serde", "std.telemetry", "std.web"] {
                    assert!(
                        specification
                            .source
                            .lines()
                            .any(|line| line.trim() == format!("import {required_facade}")),
                        "the full release-version provider Loaf must activate the public `{required_facade}` facade"
                    );
                }
                assert!(
                    specification.source.contains("@route(\"/oven-loaf-provider\")"),
                    "{envelope:?}/{:?} must exercise the web proc-macro provider rather than merely importing it",
                    specification.label
                );
            }
        }
        let release = loaf_envelope_specifications(OvenLoafEnvelope::Release);
        let source_authority_profiles = release
            .iter()
            .filter(|specification| specification.role.provides_source_authority())
            .map(|specification| specification.profile)
            .collect::<BTreeSet<_>>();
        assert_eq!(source_authority_profiles, BTreeSet::from(["debug", "release"]));
        assert!(
            release
                .iter()
                .all(|specification| specification.role != OvenLoafMemberRole::SourceAuthority)
        );
    }

    #[test]
    fn complete_stdlib_loaf_fixtures_cover_every_checked_component_module() -> Result<(), Box<dyn std::error::Error>> {
        let expected_modules = checked_stdlib_component_modules()?;
        for envelope in [OvenLoafEnvelope::Release, OvenLoafEnvelope::CompilerSuite] {
            for specification in loaf_envelope_specifications(envelope) {
                let fixture_modules = checked_stdlib_fixture_imports(specification.source);
                let missing = expected_modules
                    .difference(&fixture_modules)
                    .cloned()
                    .collect::<Vec<_>>();
                if !missing.is_empty() {
                    return Err(format!(
                        "{envelope:?}/{}/{} complete stdlib fixture omits checked provider modules: {}",
                        specification.label,
                        specification.profile,
                        missing.join(", ")
                    )
                    .into());
                }
            }
        }
        Ok(())
    }

    #[test]
    fn checked_envelope_names_the_complete_declared_repository_test_inspection_surface()
    -> Result<(), Box<dyn std::error::Error>> {
        let specifications = loaf_envelope_specifications(OvenLoafEnvelope::CompilerSuite);
        let declared_packages = |envelope| -> Result<BTreeSet<(String, Option<String>)>, Box<dyn std::error::Error>> {
            let mut packages = BTreeSet::new();
            for specification in loaf_envelope_specifications(envelope) {
                let manifest = crate::manifest::ProjectManifest::from_str(
                    specification.inspection_manifest,
                    Path::new("fixture.toml"),
                )?;
                for dependency in manifest.rust_dependencies().values() {
                    if matches!(dependency.source, DependencySource::Registry) {
                        packages.insert((
                            dependency
                                .package
                                .clone()
                                .unwrap_or_else(|| dependency.crate_name.clone()),
                            dependency.version.clone(),
                        ));
                    }
                }
            }
            Ok(packages)
        };
        let packages = declared_packages(OvenLoafEnvelope::CompilerSuite)?;
        assert_eq!(
            declared_packages(OvenLoafEnvelope::Release)?,
            packages,
            "the release and compiler-suite `stdlib` Loafs must declare one identical complete standard-library dependency surface"
        );
        let mut expected_packages = stdlib::extra_crate_deps()
            .filter(|dependency| matches!(dependency.source, StdlibExtraCrateSource::Version(_)))
            .map(|dependency| {
                stdlib::extra_crate_package_alias(dependency.crate_name)
                    .unwrap_or(dependency.crate_name)
                    .to_string()
            })
            .collect::<BTreeSet<_>>();
        expected_packages.extend([
            "bitflags".to_string(),
            "semver".to_string(),
            "serde".to_string(),
            "serde_json".to_string(),
            "uuid".to_string(),
        ]);
        assert_eq!(
            packages
                .iter()
                .map(|(package, _)| package.clone())
                .collect::<BTreeSet<_>>(),
            expected_packages
        );
        let provider = specifications
            .iter()
            .find(|specification| specification.label == "stdlib" && specification.profile == "debug")
            .ok_or("missing compiler-suite standard-provider Loaf")?;
        let provider_manifest =
            crate::manifest::ProjectManifest::from_str(provider.inspection_manifest, Path::new("provider.toml"))?;
        assert_eq!(
            provider_manifest
                .rust_dependencies()
                .values()
                .filter(|dependency| matches!(dependency.source, DependencySource::Registry))
                .map(|dependency| dependency
                    .package
                    .clone()
                    .unwrap_or_else(|| dependency.crate_name.clone()))
                .collect::<BTreeSet<_>>(),
            expected_packages
        );
        for imported_crate in ["rand", "uuid"] {
            assert!(
                provider.source.contains(&format!("from rust::{imported_crate}")),
                "the compiler-suite provider Loaf must retain an actual checked source import for `{imported_crate}` so reachable dependency resolution produces its direct-rustc leaf"
            );
        }
        assert!(
            provider.source.contains("std.serde"),
            "the compiler-suite provider Loaf must retain the checked stdlib serde surface that produces its derive-enabled direct-rustc leaf"
        );
        for reachable_use in ["Uuid.new_v4()", "thread_rng()", ".gen_range("] {
            assert!(
                provider.source.contains(reachable_use),
                "the compiler-suite provider Loaf must exercise `{reachable_use}` so its declared raw Rust dependency is usable rather than merely imported"
            );
        }
        assert!(provider.role.provides_source_authority());
        assert!(specifications.iter().all(|specification| {
            specification.role == OvenLoafMemberRole::CompiledClosureAndSourceAuthority
                && specification.retain_complete_registry_leaves
        }));
        Ok(())
    }

    #[test]
    fn envelope_source_authority_is_sealed_without_fabricating_a_linkable_leaf()
    -> Result<(), Box<dyn std::error::Error>> {
        let staging = tempfile::tempdir()?;
        let source_root = staging.path().join("registry-sources/blake2");
        fs::create_dir_all(source_root.join("src"))?;
        let manifest = b"[package]\nname = \"blake2\"\nversion = \"0.10.6\"\n";
        fs::write(source_root.join("Cargo.toml"), manifest)?;
        fs::write(source_root.join("src/lib.rs"), b"pub fn sealed() {}\n")?;
        let receipt = runtime_receipt_for_plan()?;
        let mut plan = empty_manifest(&receipt);
        plan.registry_sources.push(OvenRustcRegistrySourcePackage {
            package: "blake2".to_string(),
            version: "0.10.6".to_string(),
            features: vec!["derive".to_string(), "std".to_string()],
            source: OvenRustcRegistrySource {
                registry: "registry+https://example.invalid/index".to_string(),
                checksum: "blake2-checksum".to_string(),
                relative_root: "registry-sources/blake2".to_string(),
                digest: digest_source_tree(&source_root)?,
            },
        });
        for relative_path in [
            "registry-sources/blake2/Cargo.toml",
            "registry-sources/blake2/src/lib.rs",
        ] {
            plan.supporting_artifacts.push(OvenRustcSupportingArtifact {
                relative_path: relative_path.to_string(),
                digest: digest_bytes(&fs::read(staging.path().join(relative_path))?),
            });
        }
        let selected_files = plan.materialized_artifacts(staging.path(), &receipt.intent)?;
        assert_eq!(selected_files.len(), 2);
        assert!(
            selected_files
                .iter()
                .any(|file| file.relative_path == "registry-sources/blake2/src/lib.rs")
        );
        let source_file = source_root.join("src/lib.rs");
        let original_source = fs::read(&source_file)?;
        fs::write(&source_file, b"tampered selected source")?;
        let tamper = plan
            .materialized_artifacts(staging.path(), &receipt.intent)
            .err()
            .ok_or("changed selected source must fail physical validation")?;
        assert!(matches!(tamper, super::OvenRustcError::ArtifactDigestMismatch { .. }));
        fs::write(&source_file, original_source)?;
        assert_eq!(plan.materialized_artifacts(staging.path(), &receipt.intent)?.len(), 2);
        assert!(plan.registry_leaves.is_empty());
        assert!(plan.externs.is_empty());
        assert_eq!(plan.registry_sources.len(), 1);
        let artifact = b"sealed rlib";
        fs::create_dir_all(staging.path().join("deps"))?;
        fs::write(staging.path().join("deps/libblake2.rlib"), artifact)?;
        plan.supporting_artifacts.push(OvenRustcSupportingArtifact {
            relative_path: "deps/libblake2.rlib".to_string(),
            digest: digest_bytes(artifact),
        });
        plan.registry_leaves.push(OvenRustcRegistryLeaf {
            package: "blake2".to_string(),
            version: "0.10.6".to_string(),
            crate_name: "blake2".to_string(),
            features: vec!["std".to_string()],
            source: plan.registry_sources[0].source.clone(),
            artifact: OvenRustcArtifactExtern {
                crate_name: "blake2".to_string(),
                relative_path: "deps/libblake2.rlib".to_string(),
                digest: digest_bytes(artifact),
            },
        });
        plan.validate_shape(&receipt.intent)?;
        assert!(
            plan.supporting_artifacts
                .iter()
                .any(|artifact| artifact.relative_path.ends_with("/Cargo.toml"))
        );
        let mut conflicting = plan.registry_sources[0].clone();
        conflicting.source.checksum = "different-checksum".to_string();
        plan.registry_sources.push(conflicting);
        let error = plan
            .validate_shape(&receipt.intent)
            .err()
            .ok_or("conflicting source identity must fail closed")?;
        assert!(error.to_string().contains("more than one source identity"));
        Ok(())
    }

    #[test]
    fn source_selection_does_not_require_a_fabricated_linkable_leaf() -> Result<(), Box<dyn std::error::Error>> {
        let receipt = runtime_receipt_for_plan()?;
        let mut artifacts = empty_manifest(&receipt);
        artifacts.registry_sources.push(OvenRustcRegistrySourcePackage {
            package: "regex".to_string(),
            version: "1.12.3".to_string(),
            features: vec!["perf".to_string(), "std".to_string()],
            source: OvenRustcRegistrySource {
                registry: "registry+https://example.invalid/index".to_string(),
                checksum: "regex-checksum".to_string(),
                relative_root: "registry-sources/regex".to_string(),
                digest: "sha256:regex".to_string(),
            },
        });
        let native = super::OvenToolchainLoaf {
            loaf_identity: "sha256:fixture-loaf".to_string(),
            loaf_build_unit_identity: receipt.build_unit_identity,
            artifacts,
            registry_leaves: Vec::new(),
            artifact_root: PathBuf::from("/sealed-loaf"),
            artifact_plan: OvenRustcArtifactPlan {
                dependency_search_paths: Vec::new(),
                native_search_paths: Vec::new(),
                externs: Vec::new(),
                compile_environment: BTreeMap::new(),
                caller_owned_library_digests: BTreeMap::new(),
            },
            _generation_lock: None,
        };
        let dependency = DependencySpec {
            crate_name: "regex_alias".to_string(),
            version: Some("1".to_string()),
            features: vec!["std".to_string()],
            default_features: true,
            source: DependencySource::Registry,
            optional: false,
            package: Some("regex".to_string()),
        };

        assert!(registry_source_dependencies_supported_by_catalog(
            &native.artifacts.registry_sources,
            &[&dependency]
        ));
        assert!(native.registry_leaves.is_empty());
        Ok(())
    }

    #[test]
    fn runtime_source_digest_matches_the_staged_minimal_runtime_closure() -> Result<(), Box<dyn std::error::Error>> {
        let source = tempfile::tempdir()?;
        fs::create_dir_all(source.path().join("src/nested"))?;
        fs::create_dir_all(source.path().join("target/temporary"))?;
        fs::write(
            source.path().join("Cargo.toml"),
            "[package]\nname = \"runtime\"\nversion = \"0.1.0\"\n",
        )?;
        fs::write(source.path().join("src/lib.rs"), "pub mod nested;\n")?;
        fs::write(source.path().join("src/nested/mod.rs"), "pub fn value() {}\n")?;
        fs::write(source.path().join("README.md"), "not a runtime input\n")?;
        fs::write(source.path().join("target/temporary/artifact"), "not a runtime input\n")?;

        let staged = tempfile::tempdir()?;
        fs::create_dir_all(staged.path().join("src/nested"))?;
        for relative in ["Cargo.toml", "src/lib.rs", "src/nested/mod.rs"] {
            fs::copy(source.path().join(relative), staged.path().join(relative))?;
        }

        let digest = digest_runtime_crate_source(source.path())?;
        assert_eq!(digest, digest_source_tree(staged.path())?);

        fs::write(source.path().join("README.md"), "still not a runtime input\n")?;
        assert_eq!(digest_runtime_crate_source(source.path())?, digest);
        fs::write(source.path().join("src/nested/mod.rs"), "pub fn changed() {}\n")?;
        assert_ne!(digest_runtime_crate_source(source.path())?, digest);
        Ok(())
    }

    #[test]
    fn a_toolchain_loaf_serves_clean_project_receipts_without_a_store_copy() -> Result<(), Box<dyn std::error::Error>> {
        let first = tempfile::tempdir()?;
        let second = tempfile::tempdir()?;
        let loaf = tempfile::tempdir()?;
        fs::write(first.path().join("main.rs"), "fn main() {}\n")?;
        fs::write(second.path().join("main.rs"), "fn main() { println!(\"second\"); }\n")?;
        let receipt_for = |root: &Path| {
            receipt_generated_project(
                &OvenGeneratedProjectRequest::new(
                    root,
                    "seeded_fixture",
                    "0.1.0",
                    "aarch64-apple-darwin",
                    "rustc seeded-test",
                    "debug",
                    Vec::new(),
                )
                .with_generated_source("generated-root", root.join("main.rs")),
            )
        };
        let first_receipt = receipt_for(first.path())?;
        let second_receipt = receipt_for(second.path())?;
        assert_ne!(first_receipt.identity, second_receipt.identity);
        assert_eq!(first_receipt.build_unit_identity, second_receipt.build_unit_identity);
        let loaf_payload = OvenLoaf {
            schema_version: OVEN_LOAF_SCHEMA_VERSION,
            build_unit_identity: first_receipt.build_unit_identity.clone(),
            provenance: Default::default(),
            accounting: Default::default(),
            compatibility: OvenLoafCompatibility {
                runtime_inputs: BTreeMap::new(),
                providers: Vec::new(),
            },
            registry_leaves: Vec::new(),
            plan: OvenRustcArtifactManifest {
                schema_version: OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION,
                intent: first_receipt.intent.clone(),
                dependency_search_paths: Vec::new(),
                native_search_paths: Vec::new(),
                externs: Vec::new(),
                entrypoint_externs: BTreeMap::new(),
                registry_leaves: Vec::new(),
                registry_sources: Vec::new(),
                compile_environment: BTreeMap::new(),
                vocab_auxiliary_targets: Vec::new(),
                supporting_artifacts: Vec::new(),
            },
        };
        let loaf_path = loaf.path().join("loaf.json");
        fs::write(&loaf_path, serde_json::to_vec(&loaf_payload)?)?;
        let resolved = loaf_from_loaf(&second_receipt, &loaf_path, OvenLoafSelection::Exact)?;
        assert_eq!(resolved.loaf_build_unit_identity, first_receipt.build_unit_identity);
        assert_eq!(resolved.artifact_root, loaf.path());
        assert!(resolved.artifact_plan.externs.is_empty());

        // An immutable consumer validates only the manifest-declared files it can pass to Rustc. An unrelated
        // extra file therefore cannot influence execution and must not trigger a full recursive directory walk on
        // every normal command. The explicit whole-Loaf audit below retains the stronger undeclared-file check.
        fs::write(loaf.path().join("unsealed-extra.bin"), "not part of the Loaf")?;
        let selection = loaf_from_loaf(&second_receipt, &loaf_path, OvenLoafSelection::Exact)?;
        assert_eq!(selection.artifact_root, loaf.path());
        let error = match validate_loaf_declared_file_set(&loaf_payload, &loaf_path) {
            Ok(()) => return Err("a whole-Loaf audit must reject undeclared files".into()),
            Err(error) => error,
        };
        assert!(error.to_string().contains("undeclared file"));
        fs::remove_file(loaf.path().join("unsealed-extra.bin"))?;

        let first_selection = loaf_from_loaf(&first_receipt, &loaf_path, OvenLoafSelection::Exact)?;
        let second_selection = loaf_from_loaf(&second_receipt, &loaf_path, OvenLoafSelection::Exact)?;
        assert_eq!(first_selection.artifact_root, loaf.path());
        assert_eq!(second_selection.artifact_root, loaf.path());
        Ok(())
    }

    #[test]
    fn a_loaf_for_another_build_unit_fails_closed() -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let loaf = tempfile::tempdir()?;
        let source = project.path().join("main.rs");
        fs::write(&source, "fn main() {}\n")?;
        let receipt = receipt_generated_project(
            &OvenGeneratedProjectRequest::new(
                project.path(),
                "mismatch_fixture",
                "0.1.0",
                "aarch64-apple-darwin",
                "rustc seeded-test",
                "debug",
                Vec::new(),
            )
            .with_generated_source("generated-root", &source),
        )?;
        let loaf_payload = OvenLoaf {
            schema_version: OVEN_LOAF_SCHEMA_VERSION,
            build_unit_identity: digest_bytes(b"another-unit"),
            provenance: Default::default(),
            accounting: Default::default(),
            compatibility: OvenLoafCompatibility {
                runtime_inputs: BTreeMap::new(),
                providers: Vec::new(),
            },
            registry_leaves: Vec::new(),
            plan: OvenRustcArtifactManifest {
                schema_version: OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION,
                intent: receipt.intent.clone(),
                dependency_search_paths: Vec::new(),
                native_search_paths: Vec::new(),
                externs: Vec::new(),
                entrypoint_externs: BTreeMap::new(),
                registry_leaves: Vec::new(),
                registry_sources: Vec::new(),
                compile_environment: BTreeMap::new(),
                vocab_auxiliary_targets: Vec::new(),
                supporting_artifacts: Vec::new(),
            },
        };
        let loaf_path = loaf.path().join("loaf.json");
        fs::write(&loaf_path, serde_json::to_vec(&loaf_payload)?)?;
        assert!(matches!(
            loaf_from_loaf(&receipt, &loaf_path, OvenLoafSelection::Exact),
            Err(OvenLoafError::InvalidLoaf { .. })
        ));
        Ok(())
    }

    #[test]
    fn native_loaf_rejects_registry_leaves_outside_its_declared_plan() -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let loaf_root = tempfile::tempdir()?;
        let source = project.path().join("main.rs");
        fs::write(&source, "fn main() {}\n")?;
        let receipt = runtime_receipt(&source, "", "fixture-registry", "fixture-stdlib")?;
        let artifact_relative_path = "deps/libfixture.rlib".to_string();
        let artifact_path = loaf_root.path().join(&artifact_relative_path);
        fs::create_dir_all(artifact_path.parent().ok_or("registry artifact parent")?)?;
        let artifact_bytes = b"sealed fixture registry artifact";
        fs::write(&artifact_path, artifact_bytes)?;
        let artifact_digest = digest_bytes(artifact_bytes);
        let registry_source_relative_root = "registry-sources/fixture".to_string();
        let registry_source_root = loaf_root.path().join(&registry_source_relative_root);
        fs::create_dir_all(&registry_source_root)?;
        let registry_manifest_relative_path = format!("{registry_source_relative_root}/Cargo.toml");
        let registry_manifest = b"[package]\nname = \"fixture-registry\"\nversion = \"1.0.0\"\n";
        fs::write(registry_source_root.join("Cargo.toml"), registry_manifest)?;
        let registry_source_digest = digest_source_tree(&registry_source_root)?;
        let mut plan = empty_manifest(&receipt);
        plan.dependency_search_paths = vec!["deps".to_string()];
        plan.supporting_artifacts.push(OvenRustcSupportingArtifact {
            relative_path: artifact_relative_path.clone(),
            digest: artifact_digest.clone(),
        });
        plan.supporting_artifacts.push(OvenRustcSupportingArtifact {
            relative_path: registry_manifest_relative_path,
            digest: digest_bytes(registry_manifest),
        });
        let registry_leaf = OvenRustcRegistryLeaf {
            package: "fixture-registry".to_string(),
            version: "1.0.0".to_string(),
            crate_name: "fixture_registry".to_string(),
            features: vec!["std".to_string()],
            source: OvenRustcRegistrySource {
                registry: "registry+https://example.invalid/index".to_string(),
                checksum: "fixture-checksum".to_string(),
                relative_root: registry_source_relative_root,
                digest: registry_source_digest,
            },
            artifact: OvenRustcArtifactExtern {
                crate_name: "fixture_registry".to_string(),
                relative_path: artifact_relative_path,
                digest: artifact_digest,
            },
        };
        plan.registry_leaves = vec![registry_leaf.clone()];
        plan.registry_sources = vec![OvenRustcRegistrySourcePackage {
            package: registry_leaf.package.clone(),
            version: registry_leaf.version.clone(),
            features: registry_leaf.features.clone(),
            source: registry_leaf.source.clone(),
        }];
        let mut loaf = OvenLoaf {
            schema_version: OVEN_LOAF_SCHEMA_VERSION,
            build_unit_identity: receipt.build_unit_identity.clone(),
            provenance: Default::default(),
            accounting: Default::default(),
            compatibility: OvenLoafCompatibility::default(),
            registry_leaves: vec![registry_leaf],
            plan,
        };
        let loaf_path = loaf_root.path().join("loaf.json");
        fs::write(&loaf_path, serde_json::to_vec(&loaf)?)?;
        let resolved = loaf_from_loaf(&receipt, &loaf_path, OvenLoafSelection::Exact)?;
        assert_eq!(resolved.registry_leaves.len(), 1);

        loaf.registry_leaves[0].artifact.relative_path = "deps/libunsealed.rlib".to_string();
        loaf.plan.registry_leaves = loaf.registry_leaves.clone();
        fs::write(&loaf_path, serde_json::to_vec(&loaf)?)?;
        let error = match loaf_from_loaf(&receipt, &loaf_path, OvenLoafSelection::Exact) {
            Ok(_) => return Err("a registry leaf outside the sealed plan must fail".into()),
            Err(error) => error,
        };
        assert!(matches!(error, OvenLoafError::InvalidLoaf { .. }));
        assert!(error.to_string().contains("sealed direct-rustc plan does not declare"));
        Ok(())
    }

    #[test]
    fn a_standard_testing_loaf_authorizes_the_core_provider_subset() -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let loaf_root = tempfile::tempdir()?;
        let source = project.path().join("main.rs");
        fs::write(&source, "fn main() {}\n")?;
        let core = runtime_receipt(&source, "", "empty-rust-dependencies", "empty-stdlib-features")?;
        let testing = runtime_receipt(
            &source,
            "incan-stdlib|std.testing|testing",
            "empty-rust-dependencies",
            "fs,json,testing",
        )?;
        let unsupported_facet = runtime_receipt(
            &source,
            "incan-stdlib|std.testing|unsupported",
            "empty-rust-dependencies",
            "empty-stdlib-features",
        )?;
        assert_ne!(core.build_unit_identity, testing.build_unit_identity);

        let compatibility = OvenLoafCompatibility::from_receipt(&testing)?;
        assert!(compatibility.authorizes_provider_subset(&core)?);
        assert!(!OvenLoafCompatibility::from_receipt(&core)?.authorizes_provider_subset(&testing)?);
        assert!(!compatibility.authorizes_provider_subset(&unsupported_facet)?);

        let loaf_payload = OvenLoaf {
            schema_version: OVEN_LOAF_SCHEMA_VERSION,
            build_unit_identity: testing.build_unit_identity.clone(),
            provenance: Default::default(),
            accounting: Default::default(),
            compatibility,
            registry_leaves: Vec::new(),
            plan: empty_manifest(&testing),
        };
        let loaf_path = loaf_root.path().join("loaf.json");
        fs::write(&loaf_path, serde_json::to_vec(&loaf_payload)?)?;
        let selected = loaf_from_loaf(&core, &loaf_path, OvenLoafSelection::CompilerOwnedProviderSuperset)?;
        assert_eq!(selected.artifact_root, loaf_root.path());
        Ok(())
    }

    #[test]
    fn a_private_sdk_direct_link_requires_a_loaf_extern_capability() -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let source = project.path().join("main.rs");
        fs::write(&source, "fn main() {}\n")?;
        let ordinary = runtime_receipt(&source, "incan_stdlib_data|||none", "direct-link", "no-stdlib-features")?;
        let private_sdk_link =
            runtime_receipt(&source, "incan_stdlib_data|||link", "direct-link", "no-stdlib-features")?;

        let ordinary_compatibility = OvenLoafCompatibility::from_receipt(&ordinary)?;
        let linked_compatibility = OvenLoafCompatibility::from_receipt(&private_sdk_link)?;
        assert!(linked_compatibility.authorizes_provider_subset(&ordinary)?);
        assert!(
            !ordinary_compatibility.authorizes_provider_subset(&private_sdk_link)?,
            "a loaf without the direct SDK rlib cannot authorize a provider's private link root"
        );
        Ok(())
    }

    #[test]
    fn an_interop_execution_receipt_does_not_fragment_compiler_owned_loaf_compatibility()
    -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let source = project.path().join("main.rs");
        fs::write(&source, "fn main() {}\n")?;
        let base = runtime_receipt(
            &source,
            "incan-stdlib|std.interop|ffi",
            "empty-rust-dependencies",
            "interop",
        )?;
        let mut selected_interop = base.clone();
        selected_interop.sources.build_unit_inputs.insert(
            "oven-interop-execution-receipt".to_string(),
            "sha256:selected-package-interop-plan".to_string(),
        );
        selected_interop
            .sources
            .build_unit_inputs
            .insert("oven-interop-plan-schema".to_string(), "2".to_string());

        let compatibility = OvenLoafCompatibility::from_receipt(&base)?;
        assert!(compatibility.authorizes_provider_subset(&selected_interop)?);

        selected_interop
            .sources
            .build_unit_inputs
            .insert("unrelated-compiler-input".to_string(), "changed".to_string());
        assert!(!compatibility.authorizes_provider_subset(&selected_interop)?);
        Ok(())
    }

    #[test]
    fn loaf_selection_prefers_the_narrowest_compatible_provider_loaf() -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let source = project.path().join("main.rs");
        fs::write(&source, "fn main() {}\n")?;
        let core = runtime_receipt(&source, "", "empty-rust-dependencies", "empty-stdlib-features")?;
        let encoding = runtime_receipt(
            &source,
            "incan-stdlib|std.encoding.base64|codecs",
            "empty-rust-dependencies",
            "codecs",
        )?;
        let broad = runtime_receipt(
            &source,
            "incan-stdlib|std.async,std.testing|async,testing",
            "empty-rust-dependencies",
            "async,testing",
        )?;

        let encoding_excess = OvenLoafCompatibility::from_receipt(&encoding)?
            .provider_subset_excess(&core)?
            .ok_or("encoding loaf must authorize the core subset")?;
        let broad_excess = OvenLoafCompatibility::from_receipt(&broad)?
            .provider_subset_excess(&core)?
            .ok_or("broad loaf must authorize the core subset")?;
        assert!(encoding_excess < broad_excess);

        let selected = select_most_specific_compatible_loaf(vec![
            CompatibleLoaf {
                path: PathBuf::from("/toolchain/loafs/broad/loaf.json"),
                excess: broad_excess,
            },
            CompatibleLoaf {
                path: PathBuf::from("/toolchain/loafs/encoding/loaf.json"),
                excess: encoding_excess,
            },
        ])
        .ok_or("a compatible loaf must be selected")?;
        assert_eq!(selected.path, PathBuf::from("/toolchain/loafs/encoding/loaf.json"));
        Ok(())
    }

    #[test]
    fn registry_free_selection_skips_compatible_loaf_materialization() -> Result<(), Box<dyn std::error::Error>> {
        let narrow = CompatibleLoaf {
            path: PathBuf::from("/toolchain/loafs/narrow/loaf.json"),
            excess: super::OvenLoafProviderExcess {
                providers: 0,
                modules: 1,
                facets: 1,
                direct_links: 0,
            },
        };
        let broad = CompatibleLoaf {
            path: PathBuf::from("/toolchain/loafs/broad/loaf.json"),
            excess: super::OvenLoafProviderExcess {
                providers: 1,
                modules: 1,
                facets: 1,
                direct_links: 0,
            },
        };
        let materializations = std::cell::Cell::new(0_u8);

        let selected =
            super::select_compatible_loaf_with_registry_requirement(vec![broad, narrow.clone()], &[], |_| {
                materializations.set(materializations.get().saturating_add(1));
                Err(OvenLoafError::Preparation {
                    message: "a registry-free selection must not materialize a candidate".to_string(),
                })
            })?
            .ok_or("a compatible Loaf must be selected")?;

        assert_eq!(materializations.get(), 0);
        assert_eq!(selected, narrow);
        Ok(())
    }

    #[test]
    fn registry_selection_checks_each_compatible_loaf_before_tie_breaking() -> Result<(), Box<dyn std::error::Error>> {
        let narrow = CompatibleLoaf {
            path: PathBuf::from("/toolchain/loafs/narrow/loaf.json"),
            excess: super::OvenLoafProviderExcess {
                providers: 0,
                modules: 1,
                facets: 1,
                direct_links: 0,
            },
        };
        let broad = CompatibleLoaf {
            path: PathBuf::from("/toolchain/loafs/broad/loaf.json"),
            excess: super::OvenLoafProviderExcess {
                providers: 1,
                modules: 1,
                facets: 1,
                direct_links: 0,
            },
        };
        let dependency = DependencySpec {
            crate_name: "fixture_registry".to_string(),
            version: Some("1".to_string()),
            features: Vec::new(),
            default_features: true,
            source: DependencySource::Registry,
            optional: false,
            package: Some("fixture-registry".to_string()),
        };
        let catalog_checks = std::cell::Cell::new(0_u8);

        let selected = super::select_compatible_loaf_with_registry_requirement(
            vec![narrow, broad.clone()],
            &[&dependency],
            |candidate| {
                catalog_checks.set(catalog_checks.get().saturating_add(1));
                Ok(candidate.path == broad.path)
            },
        )?
        .ok_or("the compatible registry Loaf must be selected")?;

        assert_eq!(catalog_checks.get(), 2);
        assert_eq!(selected, broad);
        Ok(())
    }

    fn runtime_receipt(
        source: &Path,
        providers: &str,
        rust_dependencies: &str,
        stdlib_features: &str,
    ) -> Result<crate::oven::OvenReceipt, Box<dyn std::error::Error>> {
        let provider_plan = digest_bytes(providers.as_bytes());
        let mut request = OvenGeneratedProjectRequest::new(
            source.parent().ok_or("source has no parent")?,
            "runtime_fixture",
            "0.1.0",
            "aarch64-apple-darwin",
            "rustc seeded-test",
            "debug",
            Vec::new(),
        )
        .with_generated_source("generated-root", source)
        .with_build_unit_input("runtime-lock", "runtime-lock")
        .with_build_unit_input("rust-dependencies", rust_dependencies)
        .with_build_unit_input("stdlib-features", stdlib_features)
        .with_build_unit_input("provider-plan", provider_plan);
        if !providers.is_empty() {
            request = request.with_build_unit_input("providers", providers);
        }
        Ok(receipt_generated_project(&request)?)
    }

    fn runtime_receipt_for_plan() -> Result<crate::oven::OvenReceipt, Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let source = root.path().join("main.rs");
        fs::write(&source, "fn main() {}\n")?;
        // The receipt owns no filesystem path, so retaining only its value is valid after this helper drops the
        // temporary source tree.
        runtime_receipt(&source, "", "empty-rust-dependencies", "empty-stdlib-features")
    }

    fn empty_manifest(receipt: &crate::oven::OvenReceipt) -> OvenRustcArtifactManifest {
        OvenRustcArtifactManifest {
            schema_version: OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION,
            intent: receipt.intent.clone(),
            dependency_search_paths: Vec::new(),
            native_search_paths: Vec::new(),
            externs: Vec::new(),
            entrypoint_externs: BTreeMap::new(),
            registry_leaves: Vec::new(),
            registry_sources: Vec::new(),
            compile_environment: BTreeMap::new(),
            vocab_auxiliary_targets: Vec::new(),
            supporting_artifacts: Vec::new(),
        }
    }
}
