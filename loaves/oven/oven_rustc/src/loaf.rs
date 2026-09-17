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

use crate::native_contract::OvenLegacyCargoInspectionPackage;
use crate::rustc::{
    OvenRegistryLeafAuthority, OvenRustcArtifactManifest, OvenRustcArtifactPlan, OvenRustcError, OvenRustcRegistryLeaf,
    registry_source_dependencies_supported_by_catalog, validate_sealed_registry_leaf,
};
use oven_model::compiler_identity::CompilerIdentity;
use oven_model::manifest::{DependencySource, DependencySpec, ProjectManifest};
use oven_model::oven_interop::{OVEN_INTEROP_EXECUTION_RECEIPT_INPUT, OVEN_INTEROP_PLAN_SCHEMA_INPUT};
use oven_store::closure_proof::OvenClosureProof;
use oven_store::store::{OvenArtifactKind, OvenStoreError, OvenStoreExecutionPayload, PublishedOvenStore};
use oven_store::{OvenReceipt, digest_bytes, receipt_without_build_unit_input};

pub mod native_candidates;

// The candidate intake moved into a submodule; its types are still named through `oven::loaf` by every caller.
pub use native_candidates::OvenMaterializedLoafCandidate;

/// Current wire format for one compiler-shipped Oven Loaf.
pub const OVEN_LOAF_SCHEMA_VERSION: u32 = 13;
/// Current wire format for the atomically committed Loaf-envelope manifest.
pub const OVEN_LOAF_ENVELOPE_MANIFEST_SCHEMA_VERSION: u32 = 5;
/// Wire schema for an optional generic store member embedded beside one Loaf generation.
pub const OVEN_RELEASE_STORE_MEMBER_SCHEMA_VERSION: u32 = 1;
/// Wire schema for a runtime foundation bound to one compiled release Loaf.
pub const OVEN_RELEASE_RUNTIME_FOUNDATION_MEMBER_SCHEMA_VERSION: u32 = 2;
/// Stable release-envelope label for the runtime foundation carrier.
pub const OVEN_RELEASE_RUNTIME_FOUNDATION_MEMBER_LABEL: &str = "rust-policy-foundation";
pub use oven_model::compiler_suite_env::OVEN_LOAF_ENV;
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
pub static LOAF_TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);
pub(crate) const OVEN_LOAF_ENVELOPE_LOCK_FILE: &str = ".envelope.lock";

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
    pub const fn provides_compiled_closure(self) -> bool {
        matches!(self, Self::CompiledClosure | Self::CompiledClosureAndSourceAuthority)
    }

    /// Return whether Rust inspection may select this member's sealed source catalog.
    pub const fn provides_source_authority(self) -> bool {
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

impl OvenLoafSpecification {
    /// Return the exact registry packages whose Rust source this checked fixture may inspect.
    pub fn inspection_packages(&self) -> Result<Vec<OvenLegacyCargoInspectionPackage>, String> {
        let path = Path::new("loaves/oven/oven_rustc/src/fixtures").join(format!("{}.toml", self.project_name));
        inspection_packages_from_manifest(self.inspection_manifest, &path, self.label)
    }
}

/// Parse one checked manifest into the registry selectors resolved only by the explicit Loaf baker.
fn inspection_packages_from_manifest(
    contents: &str,
    path: &Path,
    label: &str,
) -> Result<Vec<OvenLegacyCargoInspectionPackage>, String> {
    let manifest = ProjectManifest::from_str(contents, path)
        .map_err(|error| format!("invalid checked Rust source manifest for `{label}`: {error}"))?;
    let mut packages = manifest
        .rust_dependencies()
        .values()
        .filter(|dependency| matches!(dependency.source, DependencySource::Registry))
        .map(|dependency| {
            let version_requirement = dependency.version.clone().ok_or_else(|| {
                format!(
                    "checked Loaf manifest `{}` omits a registry version for `{}`",
                    label, dependency.crate_name
                )
            })?;
            Ok(OvenLegacyCargoInspectionPackage {
                package: dependency
                    .package
                    .clone()
                    .unwrap_or_else(|| dependency.crate_name.clone()),
                version_requirement,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    packages.sort();
    packages.dedup();
    Ok(packages)
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
    /// Optional exact generic store entry shipped with this generation for an upper-layer release consumer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release_store_member: Option<OvenReleaseStoreMember>,
    /// Optional physical runtime foundation bound to one compiled closure in this generation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_foundation: Option<OvenReleaseRuntimeFoundationMember>,
    /// Optional rebuilt dependency closure committed atomically with its runtime foundation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_closure: Option<OvenReleaseRuntimeClosureMember>,
}

/// One release-owned runtime closure and its exact foundation/compiler bindings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OvenReleaseRuntimeClosureMember {
    /// Reference schema version.
    pub schema_version: u32,
    /// Stable publisher-selected member label.
    pub label: String,
    /// Safe generation-relative root of the embedded immutable Store.
    pub store_relative_path: PathBuf,
    /// Exact Store artifact identity of the published closure.
    pub artifact_identity: String,
    /// Content identity of the runtime closure payload.
    pub closure_identity: String,
    /// Exact foundation identity rebuilt by this closure.
    pub foundation_identity: String,
    /// Exact retained compiler closure that produced the artifacts.
    pub compiler_closure_identity: String,
}

/// Current release runtime-closure member schema.
pub const OVEN_RELEASE_RUNTIME_CLOSURE_MEMBER_SCHEMA_VERSION: u32 = 1;

/// Generation evidence key binding the typed runtime-closure descriptor.
pub const OVEN_RUNTIME_CLOSURE_DESCRIPTOR_DIGEST_EVIDENCE: &str = "runtime_closure_descriptor_digest";

/// Bind one runtime-closure descriptor into release generation evidence.
pub fn bind_release_runtime_closure_evidence(
    evidence: &mut BTreeMap<String, String>,
    member: &OvenReleaseRuntimeClosureMember,
) -> Result<(), OvenLoafError> {
    let bytes = serde_json::to_vec(member).map_err(|error| OvenLoafError::Preparation {
        message: format!("could not encode runtime-closure descriptor: {error}"),
    })?;
    evidence.insert(
        OVEN_RUNTIME_CLOSURE_DESCRIPTOR_DIGEST_EVIDENCE.to_string(),
        digest_bytes(&bytes),
    );
    Ok(())
}

/// Validate one runtime closure against the same generation's foundation and compatibility evidence.
pub fn validate_release_runtime_closure_member(
    manifest: &OvenLoafEnvelopeManifest,
    member: &OvenReleaseRuntimeClosureMember,
) -> Result<(), String> {
    if member.schema_version != OVEN_RELEASE_RUNTIME_CLOSURE_MEMBER_SCHEMA_VERSION
        || member.label.is_empty()
        || !safe_generation_relative_path(&member.store_relative_path)
        || !canonical_sha256_identity(&member.artifact_identity)
        || !canonical_sha256_identity(&member.closure_identity)
    {
        return Err("runtime-closure member is unsafe or non-canonical".to_string());
    }
    let foundation = manifest
        .runtime_foundation
        .as_ref()
        .ok_or_else(|| "runtime closure has no same-generation foundation".to_string())?;
    if member.foundation_identity != foundation.foundation_identity
        || member.compiler_closure_identity != foundation.compiler_closure_identity
    {
        return Err("runtime closure does not match its same-generation foundation/compiler".to_string());
    }
    let bytes = serde_json::to_vec(member).map_err(|error| error.to_string())?;
    if manifest.evidence.get(OVEN_RUNTIME_CLOSURE_DESCRIPTOR_DIGEST_EVIDENCE) != Some(&digest_bytes(&bytes)) {
        return Err("runtime-closure member is not bound into generation evidence".to_string());
    }
    Ok(())
}

/// Prove the exact runtime closure stored below a copied or committed generation.
pub fn prove_release_runtime_closure_member(
    generation: &Path,
    manifest: &OvenLoafEnvelopeManifest,
    member: &OvenReleaseRuntimeClosureMember,
) -> Result<crate::rustc::OvenSelectedRuntimeClosure, OvenLoafError> {
    validate_release_runtime_closure_member(manifest, member).map_err(|message| OvenLoafError::InvalidLoaf {
        path: generation.to_path_buf(),
        message,
    })?;
    let store_root = generation.join(&member.store_relative_path);
    let mut selected = PublishedOvenStore::new(&store_root)
        .select_payloads_matching_for_execution(|candidate| candidate.identity == member.artifact_identity)?;
    if selected.len() != 1 {
        return Err(OvenLoafError::InvalidLoaf {
            path: store_root,
            message: "runtime closure did not select exactly one declared Store artifact".to_string(),
        });
    }
    let payload = selected.pop().ok_or_else(|| OvenLoafError::Preparation {
        message: "runtime closure selection became empty".to_string(),
    })?;
    payload.verify_materialized_files()?;
    crate::rustc::admit_declared_runtime_closure(
        payload,
        &member.closure_identity,
        &member.foundation_identity,
        &member.compiler_closure_identity,
    )
    .map_err(|error| OvenLoafError::InvalidLoaf {
        path: store_root,
        message: error.to_string(),
    })
}

/// One release-owned runtime foundation and its exact compiled-Loaf/toolchain bindings.
///
/// This carrier locates physical Oven authority only. Package selection and feature policy remain outside Rust.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OvenReleaseRuntimeFoundationMember {
    /// Reference schema version.
    pub schema_version: u32,
    /// Stable publisher-selected member label.
    pub label: String,
    /// Safe generation-relative directory containing `foundation.json` and its declared members.
    pub foundation_relative_path: PathBuf,
    /// Canonical identity encoded by `foundation.json`.
    pub foundation_identity: String,
    /// Exact committed compiled-Loaf identity whose artifact closure the foundation describes.
    pub compiled_loaf_identity: String,
    /// Exact direct-Rustc plan identity of that compiled Loaf.
    pub compiled_plan_identity: String,
    /// Exact Toolchain owner identity declared by the selected physical graph.
    pub toolchain_owner_identity: String,
    /// Exact bounded rustc/sysroot closure digest used by runtime rebuild execution.
    pub compiler_closure_identity: String,
    /// Safe generation-relative directory holding that Toolchain owner's physical members.
    pub toolchain_root_relative_path: PathBuf,
    /// Exact compiler-owned regular files retained below `toolchain_root_relative_path`.
    pub toolchain_members: Vec<OvenReleaseToolchainMember>,
}

/// One digest-verified compiler-owned member retained for runtime foundation materialization.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OvenReleaseToolchainMember {
    /// Safe path relative to the retained Toolchain root.
    pub relative_path: PathBuf,
    /// SHA-256 identity of the exact retained bytes.
    pub digest: String,
}

/// Retain the exact bounded compiler/sysroot closure used for one runtime-foundation target.
pub fn stage_release_runtime_foundation_toolchain(
    rustc: &Path,
    target: &str,
    destination: &Path,
) -> Result<(String, Vec<OvenReleaseToolchainMember>), OvenLoafError> {
    if destination.exists() {
        return Err(OvenLoafError::Preparation {
            message: format!(
                "retained Toolchain destination already exists: {}",
                destination.display()
            ),
        });
    }
    fs::create_dir_all(destination).map_err(|source| OvenLoafError::Io {
        path: destination.to_path_buf(),
        source,
    })?;
    let evidence =
        crate::rustc::direct_compiler::retention::direct_rustc_compiler_evidence(rustc, target).map_err(|error| {
            OvenLoafError::Preparation {
                message: error.to_string(),
            }
        })?;
    let closure_digest = evidence.closure_digest.clone();
    let mut retained = Vec::with_capacity(evidence.members.len());
    for member in evidence.members {
        let relative_path = PathBuf::from(member.relative_path);
        if !safe_generation_relative_path(&relative_path) {
            return Err(OvenLoafError::Preparation {
                message: "compiler closure contains an unsafe member path".to_string(),
            });
        }
        let output = destination.join(&relative_path);
        fs::create_dir_all(output.parent().ok_or_else(|| OvenLoafError::Preparation {
            message: "compiler closure member has no parent".to_string(),
        })?)
        .map_err(|source| OvenLoafError::Io {
            path: output.clone(),
            source,
        })?;
        fs::copy(&member.source_path, &output).map_err(|source| OvenLoafError::Io {
            path: member.source_path,
            source,
        })?;
        retained.push(OvenReleaseToolchainMember {
            relative_path,
            digest: member.digest,
        });
    }
    retained.sort();
    Ok((closure_digest, retained))
}

/// Return the canonical descriptor digest publishers include in release compatibility evidence.
pub fn release_runtime_foundation_member_descriptor_digest(
    member: &OvenReleaseRuntimeFoundationMember,
) -> Result<String, OvenLoafError> {
    let bytes = serde_json::to_vec(member).map_err(|error| OvenLoafError::Preparation {
        message: format!("could not encode runtime-foundation member: {error}"),
    })?;
    Ok(digest_bytes(&bytes))
}

/// Compatibility-evidence key binding the runtime-foundation descriptor into the envelope generation.
pub const OVEN_RUNTIME_FOUNDATION_DESCRIPTOR_DIGEST_EVIDENCE: &str = "runtime_foundation_descriptor_digest";

/// Compatibility-evidence key binding the runtime-foundation authority identity into the envelope generation.
pub const OVEN_RUNTIME_FOUNDATION_IDENTITY_EVIDENCE: &str = "runtime_foundation_identity";

/// Bind one runtime-foundation carrier into the evidence used to derive its envelope generation identity.
pub fn bind_release_runtime_foundation_evidence(
    evidence: &mut BTreeMap<String, String>,
    member: &OvenReleaseRuntimeFoundationMember,
) -> Result<(), OvenLoafError> {
    evidence.insert(
        OVEN_RUNTIME_FOUNDATION_DESCRIPTOR_DIGEST_EVIDENCE.to_string(),
        release_runtime_foundation_member_descriptor_digest(member)?,
    );
    evidence.insert(
        OVEN_RUNTIME_FOUNDATION_IDENTITY_EVIDENCE.to_string(),
        member.foundation_identity.clone(),
    );
    Ok(())
}

/// Validate one runtime-foundation carrier against this envelope's compiled member identities.
pub fn validate_release_runtime_foundation_member(
    manifest: &OvenLoafEnvelopeManifest,
    member: &OvenReleaseRuntimeFoundationMember,
) -> Result<(), String> {
    if member.schema_version != OVEN_RELEASE_RUNTIME_FOUNDATION_MEMBER_SCHEMA_VERSION {
        return Err(format!(
            "unsupported runtime-foundation member schema {}",
            member.schema_version
        ));
    }
    if member.label != OVEN_RELEASE_RUNTIME_FOUNDATION_MEMBER_LABEL
        || !canonical_sha256_identity(&member.foundation_identity)
        || !canonical_sha256_identity(&member.compiled_loaf_identity)
        || !canonical_sha256_identity(&member.compiled_plan_identity)
        || !canonical_sha256_identity(&member.toolchain_owner_identity)
        || !canonical_sha256_identity(&member.compiler_closure_identity)
        || !safe_generation_relative_path(&member.foundation_relative_path)
        || !safe_generation_relative_path(&member.toolchain_root_relative_path)
        || member.toolchain_members.is_empty()
    {
        return Err("runtime-foundation member has incomplete identity or unsafe paths".to_string());
    }
    let mut prior = None;
    for toolchain_member in &member.toolchain_members {
        if !safe_generation_relative_path(&toolchain_member.relative_path)
            || !canonical_sha256_identity(&toolchain_member.digest)
            || prior
                .as_ref()
                .is_some_and(|path| path >= &toolchain_member.relative_path)
        {
            return Err("runtime-foundation Toolchain members are unsafe or not canonical".to_string());
        }
        prior = Some(toolchain_member.relative_path.clone());
    }
    if member
        .foundation_relative_path
        .starts_with(&member.toolchain_root_relative_path)
        || member
            .toolchain_root_relative_path
            .starts_with(&member.foundation_relative_path)
    {
        return Err("runtime-foundation and Toolchain roots must be disjoint".to_string());
    }
    let descriptor_digest =
        release_runtime_foundation_member_descriptor_digest(member).map_err(|error| error.to_string())?;
    if manifest
        .evidence
        .get(OVEN_RUNTIME_FOUNDATION_DESCRIPTOR_DIGEST_EVIDENCE)
        != Some(&descriptor_digest)
        || manifest.evidence.get(OVEN_RUNTIME_FOUNDATION_IDENTITY_EVIDENCE) != Some(&member.foundation_identity)
    {
        return Err("runtime-foundation member is not bound into generation compatibility evidence".to_string());
    }
    let matches = manifest
        .loafs
        .iter()
        .filter(|candidate| {
            candidate.role.provides_compiled_closure()
                && candidate.loaf_identity == member.compiled_loaf_identity
                && candidate.plan_identity == member.compiled_plan_identity
        })
        .count();
    if matches != 1 {
        return Err("runtime-foundation member must bind exactly one committed compiled Loaf and plan".to_string());
    }
    Ok(())
}

/// Re-prove a runtime foundation from paths held under its committed generation.
///
/// This performs only physical identity and closure checks. It does not select packages or execute policy.
pub fn prove_release_runtime_foundation_member(
    loaf_root: &Path,
    manifest: &OvenLoafEnvelopeManifest,
    member: &OvenReleaseRuntimeFoundationMember,
) -> Result<crate::rustc::OvenMaterializedRuntimeFoundationAsset, OvenLoafError> {
    validate_release_runtime_foundation_member(manifest, member).map_err(|message| OvenLoafError::InvalidLoaf {
        path: loaf_root.join("envelope.json"),
        message,
    })?;
    let generation = loaf_root.join(generation_directory_path(&manifest.generation_identity));
    let canonical_generation = fs::canonicalize(&generation).map_err(|source| OvenLoafError::Io {
        path: generation.clone(),
        source,
    })?;
    let foundation_root = generation.join(&member.foundation_relative_path);
    let toolchain_root = generation.join(&member.toolchain_root_relative_path);
    let canonical_foundation = fs::canonicalize(&foundation_root).map_err(|source| OvenLoafError::Io {
        path: foundation_root.clone(),
        source,
    })?;
    let canonical_toolchain = fs::canonicalize(&toolchain_root).map_err(|source| OvenLoafError::Io {
        path: toolchain_root.clone(),
        source,
    })?;
    if !canonical_foundation.starts_with(&canonical_generation)
        || !canonical_toolchain.starts_with(&canonical_generation)
    {
        return Err(OvenLoafError::InvalidLoaf {
            path: foundation_root,
            message: "runtime-foundation member resolves outside its held generation".to_string(),
        });
    }
    let mut retained_paths = Vec::new();
    collect_regular_member_paths(&canonical_toolchain, &canonical_toolchain, &mut retained_paths)?;
    if retained_paths
        != member
            .toolchain_members
            .iter()
            .map(|candidate| candidate.relative_path.clone())
            .collect::<Vec<_>>()
    {
        return Err(OvenLoafError::InvalidLoaf {
            path: canonical_toolchain.clone(),
            message: "retained Toolchain directory does not exactly match its declared members".to_string(),
        });
    }
    for toolchain_member in &member.toolchain_members {
        let path = canonical_toolchain.join(&toolchain_member.relative_path);
        let canonical = fs::canonicalize(&path).map_err(|source| OvenLoafError::Io {
            path: path.clone(),
            source,
        })?;
        let metadata = fs::symlink_metadata(&path).map_err(|source| OvenLoafError::Io {
            path: path.clone(),
            source,
        })?;
        if !canonical.starts_with(&canonical_toolchain)
            || !metadata.file_type().is_file()
            || digest_bytes(&fs::read(&canonical).map_err(|source| OvenLoafError::Io {
                path: canonical.clone(),
                source,
            })?) != toolchain_member.digest
        {
            return Err(OvenLoafError::InvalidLoaf {
                path,
                message: "retained Toolchain member identity does not match its descriptor".to_string(),
            });
        }
    }
    let admitted =
        crate::rustc::admit_runtime_foundation_asset_for_publication(&canonical_foundation, &canonical_toolchain)
            .map_err(|error| OvenLoafError::Preparation {
                message: error.to_string(),
            })?;
    let materialized = admitted
        .materialize_asset_for_publication()
        .map_err(|error| OvenLoafError::Preparation {
            message: error.to_string(),
        })?;
    if materialized.foundation_identity() != member.foundation_identity
        || materialized.foundation().compiler_closure_digest() != member.compiler_closure_identity
        || materialized.foundation().artifact_owner() == member.toolchain_owner_identity
        || !materialized
            .foundation()
            .selected_graph()
            .graph()
            .owners
            .iter()
            .any(|owner| {
                owner.identity == member.toolchain_owner_identity
                    && owner.kind == crate::rustc::OvenSelectedRustFacetOwnerKind::Toolchain
            })
    {
        return Err(OvenLoafError::InvalidLoaf {
            path: canonical_foundation,
            message: "runtime-foundation descriptor identity or Toolchain owner disagrees with its carrier".to_string(),
        });
    }
    let compiled = manifest
        .loafs
        .iter()
        .find(|candidate| {
            candidate.loaf_identity == member.compiled_loaf_identity
                && candidate.plan_identity == member.compiled_plan_identity
                && candidate.role.provides_compiled_closure()
        })
        .ok_or_else(|| OvenLoafError::InvalidLoaf {
            path: loaf_root.join("envelope.json"),
            message: "runtime-foundation compiled Loaf binding disappeared".to_string(),
        })?;
    let compiled_path = loaf_root.join(&compiled.path);
    let loaf = read_loaf(&compiled_path)?;
    if loaf_file_identity(&compiled_path)? != member.compiled_loaf_identity
        || digest_bytes(
            &serde_json::to_vec(&loaf.plan).map_err(|error| OvenLoafError::Preparation {
                message: error.to_string(),
            })?,
        ) != member.compiled_plan_identity
        || materialized.foundation().artifacts() != &loaf.plan
    {
        return Err(OvenLoafError::InvalidLoaf {
            path: compiled_path,
            message: "runtime foundation does not describe its bound compiled Loaf artifact manifest".to_string(),
        });
    }
    Ok(materialized)
}

/// Enumerate one retained member tree without following links or admitting special files.
pub(crate) fn collect_regular_member_paths(
    root: &Path,
    directory: &Path,
    members: &mut Vec<PathBuf>,
) -> Result<(), OvenLoafError> {
    let mut entries = fs::read_dir(directory)
        .map_err(|source| OvenLoafError::Io {
            path: directory.to_path_buf(),
            source,
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|source| OvenLoafError::Io {
            path: directory.to_path_buf(),
            source,
        })?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        let file_type = entry.file_type().map_err(|source| OvenLoafError::Io {
            path: path.clone(),
            source,
        })?;
        if file_type.is_dir() {
            collect_regular_member_paths(root, &path, members)?;
        } else if file_type.is_file() {
            members.push(
                path.strip_prefix(root)
                    .map_err(|_| OvenLoafError::InvalidLoaf {
                        path: path.clone(),
                        message: "retained Toolchain member is outside its root".to_string(),
                    })?
                    .to_path_buf(),
            );
        } else {
            return Err(OvenLoafError::InvalidLoaf {
                path,
                message: "retained Toolchain tree contains a link or special file".to_string(),
            });
        }
    }
    Ok(())
}

/// Return whether one externally stored identity is a canonical lowercase SHA-256 digest.
fn canonical_sha256_identity(identity: &str) -> bool {
    identity.strip_prefix("sha256:").is_some_and(|digest| {
        digest.len() == 64
            && digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
}

/// One exact generic Oven store entry embedded under a committed release generation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OvenReleaseStoreMember {
    /// Reference schema version.
    pub schema_version: u32,
    /// Stable publisher-selected member label.
    pub label: String,
    /// Safe path below this generation that contains the embedded Oven store root.
    pub store_relative_path: PathBuf,
    /// Exact immutable Oven artifact identity selected from that store.
    pub artifact_identity: String,
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

/// Return the registry-source selectors declared by one built-in envelope's checked fixtures.
///
/// The compiler-suite baker additionally seals its complete locked compiler graph. That graph comes from the
/// canonical compiler manifest, features, and lock rather than a second hand-maintained dependency list.
pub fn loaf_envelope_inspection_packages(
    envelope: OvenLoafEnvelope,
) -> Result<Vec<OvenLegacyCargoInspectionPackage>, String> {
    let mut packages = loaf_envelope_specifications(envelope)
        .iter()
        .filter(|specification| specification.role.provides_source_authority())
        .map(OvenLoafSpecification::inspection_packages)
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    packages.sort();
    packages.dedup();
    Ok(packages)
}

/// Owner-scoped staging directory that is removed unless a verified loaf is atomically published from it.
pub struct LoafTemporaryDirectory {
    path: PathBuf,
    keep: bool,
}

impl LoafTemporaryDirectory {
    /// Create a unique owner-scoped Loaf staging directory below `parent`.
    pub fn create(parent: &Path, prefix: &str) -> io::Result<Self> {
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
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Reclaim scratch space at an explicit, measurable boundary and report any filesystem failure.
    ///
    /// A failed removal leaves the remaining path for diagnosis; Drop must not silently retry expensive cleanup
    /// after the caller has already recorded its duration and failure.
    pub fn close(mut self) -> std::io::Result<()> {
        self.keep = true;
        fs::remove_dir_all(&self.path)
    }

    /// Retain the staging directory after its caller has atomically published it.
    pub fn persist(mut self) -> PathBuf {
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

/// Move every obsolete generation out of the authoritative envelope tree before scratch reclamation.
pub fn retire_unreferenced_loaf_generations(
    output: &Path,
    generation_identity: &str,
    scratch: &Path,
) -> Result<(), OvenLoafError> {
    let generations_root = output.join("generations");
    if !generations_root.is_dir() {
        return Ok(());
    }
    let active_name = generation_identity
        .strip_prefix("sha256:")
        .unwrap_or(generation_identity);
    let active_generation = generations_root.join(active_name);
    let retired_root = scratch.join("retired");
    fs::create_dir_all(&retired_root).map_err(|source| OvenLoafError::Io {
        path: retired_root.clone(),
        source,
    })?;
    for entry in fs::read_dir(&generations_root).map_err(|source| OvenLoafError::Io {
        path: generations_root.clone(),
        source,
    })? {
        let entry = entry.map_err(|source| OvenLoafError::Io {
            path: generations_root.clone(),
            source,
        })?;
        let path = entry.path();
        if path != active_generation {
            let destination = retired_root.join(entry.file_name());
            fs::rename(&path, &destination).map_err(|source| OvenLoafError::Io { path, source })?;
        }
    }
    Ok(())
}

/// Durably publish one staged generation before atomically switching the envelope authority.
pub fn commit_loaf_generation(
    output: &Path,
    generations_root: &Path,
    generation_output: &Path,
    staged_root: &Path,
    manifest: &OvenLoafEnvelopeManifest,
    scratch: &Path,
    before_manifest_commit: impl FnOnce() -> io::Result<()>,
) -> Result<(), OvenLoafError> {
    oven_store::store::sync_directory_tree(staged_root)?;
    if generation_output.exists() {
        let abandoned = scratch.join("abandoned-generation");
        fs::rename(generation_output, &abandoned).map_err(|source| OvenLoafError::Io {
            path: generation_output.to_path_buf(),
            source,
        })?;
    }
    fs::rename(staged_root, generation_output).map_err(|source| OvenLoafError::Io {
        path: generation_output.to_path_buf(),
        source,
    })?;
    oven_store::store::sync_directory(generations_root.to_path_buf())?;
    let staged_manifest = scratch.join("envelope.json");
    let payload = serde_json::to_vec_pretty(manifest).map_err(|error| OvenLoafError::Preparation {
        message: format!("could not encode Loaf envelope manifest: {error}"),
    })?;
    fs::write(&staged_manifest, payload).map_err(|source| OvenLoafError::Io {
        path: staged_manifest.clone(),
        source,
    })?;
    File::open(&staged_manifest)
        .and_then(|file| file.sync_all())
        .map_err(|source| OvenLoafError::Io {
            path: staged_manifest.clone(),
            source,
        })?;
    before_manifest_commit().map_err(|source| OvenLoafError::Io {
        path: staged_manifest.clone(),
        source,
    })?;
    let manifest_path = output.join("envelope.json");
    fs::rename(&staged_manifest, &manifest_path).map_err(|source| OvenLoafError::Io {
        path: manifest_path,
        source,
    })?;
    oven_store::store::sync_directory(output.to_path_buf())?;
    Ok(())
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
pub struct OvenLoafGenerationLock {
    file: File,
}

impl Drop for OvenLoafGenerationLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

/// Acquire exclusive authority to validate, switch, and retire generations below one Loaf envelope root.
pub fn acquire_exclusive_loaf_generation_lock(root: &Path) -> Result<OvenLoafGenerationLock, OvenLoafError> {
    fs::create_dir_all(root).map_err(|source| OvenLoafError::Io {
        path: root.to_path_buf(),
        source,
    })?;
    let path = root.join(OVEN_LOAF_ENVELOPE_LOCK_FILE);
    let file = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&path)
        .map_err(|source| OvenLoafError::Io {
            path: path.clone(),
            source,
        })?;
    file.lock().map_err(|source| OvenLoafError::Io { path, source })?;
    Ok(OvenLoafGenerationLock { file })
}

impl OvenToolchainLoaf {
    #[must_use]
    /// Expose this unit's registry leaves with only its verified transitive metadata directories.
    pub fn registry_leaf_authority(&self) -> OvenRegistryLeafAuthority {
        OvenRegistryLeafAuthority::new_with_trusted_dependency_search_paths(
            self.artifact_root.clone(),
            self.registry_leaves.clone(),
            self.artifact_plan.dependency_search_paths.clone(),
        )
    }

    /// Return the verified release-envelope root that owns this lock-held Loaf.
    ///
    /// Standalone test Loafs have no generation lock and therefore expose no envelope authority.
    pub fn release_envelope_root(&self) -> Result<Option<PathBuf>, OvenLoafError> {
        if self._generation_lock.is_none() {
            return Ok(None);
        }
        let Some(root) = self.artifact_root.ancestors().nth(3).map(Path::to_path_buf) else {
            return Ok(None);
        };
        let manifest_path = root.join("envelope.json");
        if !manifest_path.is_file() {
            return Ok(None);
        }
        let bytes = fs::read(&manifest_path).map_err(|source| OvenLoafError::Io {
            path: manifest_path.clone(),
            source,
        })?;
        let manifest: OvenLoafEnvelopeManifest =
            serde_json::from_slice(&bytes).map_err(|error| OvenLoafError::InvalidLoaf {
                path: manifest_path.clone(),
                message: format!("invalid committed envelope manifest: {error}"),
            })?;
        if manifest.schema_version != OVEN_LOAF_ENVELOPE_MANIFEST_SCHEMA_VERSION {
            return Err(OvenLoafError::InvalidLoaf {
                path: manifest_path,
                message: format!("unsupported envelope manifest schema {}", manifest.schema_version),
            });
        }
        Ok((manifest.envelope == "release").then_some(root))
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
    /// The candidate's sealed registry catalog, read once with its manifest so registry support can be decided
    /// without materializing every candidate's artifact closure.
    registry_leaves: Vec<OvenRustcRegistryLeaf>,
}

impl OvenLoafCompatibility {
    /// Derive the explicit, portable compatibility envelope from one verified generated-project receipt.
    pub fn from_receipt(receipt: &OvenReceipt) -> Result<Self, OvenLoafError> {
        let mut runtime_inputs = receipt.sources.build_unit_inputs.clone();
        let provider_records = runtime_inputs.remove("providers").unwrap_or_default();
        let _ = runtime_inputs.remove("rust-dependencies");
        let _ = runtime_inputs.remove("stdlib-facets");
        // The selected interop receipt proves package-owned archives and headers, not a compiler-owned runtime
        // capability. Its immutable final plan is independently reconstructed and verified before execution; using
        // it as a Loaf compatibility key would require one shipped Loaf per consumer package.
        let _ = runtime_inputs.remove(OVEN_INTEROP_EXECUTION_RECEIPT_INPUT);
        let _ = runtime_inputs.remove(OVEN_INTEROP_PLAN_SCHEMA_INPUT);
        // A consumer records the macro set its own providers must compile against; a compiler-owned Loaf is sealed
        // before any consumer exists and so can never carry one. Treating it as a compatibility key therefore asks
        // the shipped Loaf for a fact only the caller has, which is the same bargain the interop inputs above
        // decline, and it disqualifies every release Loaf for every consumer that has providers at all. The
        // requirement itself is not going unchecked: it is derived from checked packaged provider profiles by
        // `checked_provider_compilation_requirements`, and `provider_compilation_externs` refuses a requirement whose
        // macro is absent from the selected dependencies.
        let _ = runtime_inputs.remove("provider-compilation-requirements");
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

    /// Name the first condition that stops this loaf serving `receipt`, for diagnostics only.
    ///
    /// `provider_subset_excess` collapses several distinct rejections into one `None`, which is fine for selection and
    /// useless in an error message. This mirrors its order exactly and returns the reason, so a caller can say whether
    /// the runtime inputs differed, a provider was absent, or a present provider lacked a required module or facet.
    fn provider_subset_rejection(&self, receipt: &OvenReceipt) -> Result<Option<String>, OvenLoafError> {
        let requested = Self::from_receipt(receipt)?;
        if self.runtime_inputs != requested.runtime_inputs {
            // Naming the keys matters more than the fact. `from_receipt` deliberately drops providers,
            // rust-dependencies, stdlib-facets, the interop pair and provider-plan, so a difference here is always
            // a key nobody decided to exclude, and the key's name is the whole lead.
            let mut differences = Vec::new();
            for (key, value) in &requested.runtime_inputs {
                match self.runtime_inputs.get(key) {
                    None => differences.push(format!("`{key}` is absent from the Loaf")),
                    Some(found) if found != value => differences.push(format!("`{key}` differs")),
                    Some(_) => {}
                }
            }
            for key in self.runtime_inputs.keys() {
                if !requested.runtime_inputs.contains_key(key) {
                    differences.push(format!("`{key}` is present only in the Loaf"));
                }
            }
            differences.truncate(4);
            return Ok(Some(format!(
                "its runtime inputs differ from the request: {}",
                differences.join(", ")
            )));
        }
        let mut available = BTreeMap::new();
        for provider in &self.providers {
            available.insert(provider.identity.as_str(), provider);
        }
        for required in &requested.providers {
            let Some(candidate) = available.get(required.identity.as_str()) else {
                return Ok(Some(format!("it does not ship provider `{}`", required.identity)));
            };
            if let Some(module) = required
                .modules
                .iter()
                .find(|module| candidate.modules.binary_search(module).is_err())
            {
                return Ok(Some(format!(
                    "provider `{}` is missing module `{module}`",
                    required.identity
                )));
            }
            if let Some(facet) = required
                .facets
                .iter()
                .find(|facet| candidate.facets.binary_search(facet).is_err())
            {
                return Ok(Some(format!(
                    "provider `{}` is missing facet `{facet}`",
                    required.identity
                )));
            }
            if required.direct_link && !candidate.direct_link {
                return Ok(Some(format!(
                    "provider `{}` is not directly linkable",
                    required.identity
                )));
            }
        }
        Ok(None)
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

/// Result from a release-stage base-runtime loaf preparation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OvenLoafPreparation {
    /// Reusable native compatibility identity represented by the loaf.
    pub build_unit_identity: String,
    /// Content identity of the canonical Loaf metadata and its declared artifact digests.
    pub loaf_identity: String,
    /// Content identity of the final sealed direct-rustc plan stored in the Loaf.
    pub plan_identity: String,
    /// Logical bytes in the final compiler-shipped loaf directory, including its verified plan.
    pub logical_bytes: u64,
    /// Measured allocation in the final compiler-shipped loaf directory.
    pub physical_bytes: u64,
    /// Highest observed physical allocation in baker-owned transient state.
    pub transient_peak_physical_bytes: u64,
}

/// Construct the portable runtime portion of a normal generated project's native build-unit identity.
///
/// The caller contributes normalized provider records, selected stdlib features, and the digest of resolved Rust
/// dependencies. Compiler-owned sources and the lockfile are resolved from the active toolchain layout so a packaged
/// compiler never depends on the checkout from which its binary happened to be built.
pub fn runtime_build_unit_inputs(
    compiler: &CompilerIdentity,
    provider_records: Vec<String>,
    stdlib_facets: &[String],
    rust_dependencies_digest: String,
) -> Result<BTreeMap<String, String>, String> {
    let mut inputs = BTreeMap::new();
    inputs.insert("compiler-version".to_string(), compiler.version.clone());
    inputs.insert(
        "sdk-provider-codegen-revision".to_string(),
        compiler.sdk_provider_codegen_revision.to_string(),
    );
    // One receipt input per compiler-owned runtime crate, so a change to any facet's source re-keys the Loaf.
    for crate_name in oven_model::toolchain_layout::SDK_RUNTIME_CRATES {
        let path = oven_model::toolchain_layout::resolve_toolchain_crate_path(crate_name);
        let digest = digest_runtime_crate_source(&path)?;
        inputs.insert(format!("runtime-source-{}", crate_name.replace('_', "-")), digest);
    }
    let lock_path = oven_model::toolchain_layout::resolve_toolchain_runtime_lockfile();
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
        "stdlib-facets".to_string(),
        digest_bytes(stdlib_facets.join(",").as_bytes()),
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
pub fn digest_runtime_crate_source(root: &Path) -> Result<String, String> {
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
    /// The release-stage publisher could not prepare its temporary direct-rustc closure. The publisher's own error
    /// type lives with it in `oven_cargo_compat`, over this crate; what crosses back is its rendered failure.
    #[error("{0}")]
    Publisher(String),
    /// A release-stage Loaf could not be assembled safely.
    #[error("failed to prepare Oven Loaf: {message}")]
    Preparation { message: String },
}

/// Inputs owned by the explicit publisher that seals a source compiler's vocabulary helper closure.
pub struct OvenSourceCompilerVocabSupportRequest<'a> {
    /// Plan that receives the digest-verified helper artifacts.
    pub plan: &'a mut OvenRustcArtifactManifest,
    /// Private publisher staging root that owns copied helper artifacts.
    pub loaf_staging: &'a Path,
    /// Checked compiler workspace that owns `incan_vocab` and its lockfile.
    pub compiler_root: &'a Path,
    /// Cargo executable admitted only for this explicit publisher operation.
    pub cargo: &'a Path,
    /// Rust compiler whose target/profile must match the sealed plan.
    pub rustc: &'a Path,
    /// Private Cargo target root for the helper's native and Wasm builds.
    pub cargo_target: &'a Path,
    /// Publisher-private directories included in transient-capacity enforcement.
    pub capacity_roots: &'a [&'a Path],
    /// Maximum physical allocation allowed while compiling the helper.
    pub transient_limit: u64,
}

/// Measure the exact directory that will be copied into a toolchain archive.
///
/// This intentionally includes `loaf.json`: while the plan is control metadata rather than a link input, it is a
/// retained physical file and therefore belongs in the release accounting. Publisher source files are copied rather
/// than linked, so summing regular-file allocation gives a conservative, portable report for this final closure.
pub fn loaf_directory_byte_counts(root: &Path) -> Result<(u64, u64), OvenLoafError> {
    let metadata = fs::symlink_metadata(root).map_err(|source| OvenLoafError::Io {
        path: root.to_path_buf(),
        source,
    })?;
    if metadata.file_type().is_symlink() {
        return Err(OvenLoafError::Preparation {
            message: format!("Loaf may not contain a symlink: {}", root.display()),
        });
    }
    if metadata.is_file() {
        return Ok((metadata.len(), loaf_file_physical_bytes(&metadata)));
    }
    if !metadata.is_dir() {
        return Err(OvenLoafError::Preparation {
            message: format!(
                "Loaf may contain only regular files and directories: {}",
                root.display()
            ),
        });
    }

    let mut logical_bytes = 0_u64;
    let mut physical_bytes = 0_u64;
    for child in fs::read_dir(root).map_err(|source| OvenLoafError::Io {
        path: root.to_path_buf(),
        source,
    })? {
        let child = child.map_err(|source| OvenLoafError::Io {
            path: root.to_path_buf(),
            source,
        })?;
        let (child_logical_bytes, child_physical_bytes) = loaf_directory_byte_counts(&child.path())?;
        logical_bytes = logical_bytes.saturating_add(child_logical_bytes);
        physical_bytes = physical_bytes.saturating_add(child_physical_bytes);
    }
    Ok((logical_bytes, physical_bytes))
}

/// Measure raw host allocation for the complete Loaf directory tree, including directory metadata.
///
/// Policy-accounted physical bytes deliberately cover retained regular artifacts. This second measurement follows
/// the ordinary `du` hard-link rule: one inode contributes its allocated blocks once even when multiple Loaf or store
/// paths name it. Reports therefore include directory and control-file allocation without inventing extra disk use for
/// the store's content-preserving hard links.
pub fn loaf_raw_disk_bytes(root: &Path) -> Result<u64, OvenLoafError> {
    #[cfg(unix)]
    {
        loaf_raw_disk_bytes_unix(root, &mut std::collections::BTreeSet::new())
    }

    #[cfg(not(unix))]
    {
        loaf_raw_disk_bytes_portable(root)
    }
}

/// Traverse a Unix Loaf tree while counting each allocated inode once.
#[cfg(unix)]
fn loaf_raw_disk_bytes_unix(
    root: &Path,
    seen: &mut std::collections::BTreeSet<(u64, u64)>,
) -> Result<u64, OvenLoafError> {
    use std::os::unix::fs::MetadataExt;

    let metadata = fs::symlink_metadata(root).map_err(|source| OvenLoafError::Io {
        path: root.to_path_buf(),
        source,
    })?;
    if metadata.file_type().is_symlink() {
        return Err(OvenLoafError::Preparation {
            message: format!("Loaf may not contain a symlink: {}", root.display()),
        });
    }
    if !seen.insert((metadata.dev(), metadata.ino())) {
        return Ok(0);
    }
    if metadata.is_file() {
        return Ok(loaf_file_physical_bytes(&metadata));
    }
    if !metadata.is_dir() {
        return Err(OvenLoafError::Preparation {
            message: format!(
                "Loaf may contain only regular files and directories: {}",
                root.display()
            ),
        });
    }

    let mut bytes = loaf_file_physical_bytes(&metadata);
    for child in fs::read_dir(root).map_err(|source| OvenLoafError::Io {
        path: root.to_path_buf(),
        source,
    })? {
        let child = child.map_err(|source| OvenLoafError::Io {
            path: root.to_path_buf(),
            source,
        })?;
        bytes = bytes.saturating_add(loaf_raw_disk_bytes_unix(&child.path(), seen)?);
    }
    Ok(bytes)
}

/// Traverse a Loaf tree on hosts that do not expose a stable device/inode identity.
#[cfg(not(unix))]
fn loaf_raw_disk_bytes_portable(root: &Path) -> Result<u64, OvenLoafError> {
    let metadata = fs::symlink_metadata(root).map_err(|source| OvenLoafError::Io {
        path: root.to_path_buf(),
        source,
    })?;
    if metadata.file_type().is_symlink() {
        return Err(OvenLoafError::Preparation {
            message: format!("Loaf may not contain a symlink: {}", root.display()),
        });
    }
    if metadata.is_file() {
        return Ok(loaf_file_physical_bytes(&metadata));
    }
    if !metadata.is_dir() {
        return Err(OvenLoafError::Preparation {
            message: format!(
                "Loaf may contain only regular files and directories: {}",
                root.display()
            ),
        });
    }

    let mut bytes = loaf_file_physical_bytes(&metadata);
    for child in fs::read_dir(root).map_err(|source| OvenLoafError::Io {
        path: root.to_path_buf(),
        source,
    })? {
        let child = child.map_err(|source| OvenLoafError::Io {
            path: root.to_path_buf(),
            source,
        })?;
        bytes = bytes.saturating_add(loaf_raw_disk_bytes_portable(&child.path())?);
    }
    Ok(bytes)
}

/// Return physical allocation for one compiler-shipped loaf file, preserving a portable fallback outside Unix.
#[cfg(unix)]
fn loaf_file_physical_bytes(metadata: &fs::Metadata) -> u64 {
    use std::os::unix::fs::MetadataExt;

    metadata.blocks().saturating_mul(512)
}

/// Return logical bytes where the host cannot expose allocated Unix block counts.
#[cfg(not(unix))]
fn loaf_file_physical_bytes(metadata: &fs::Metadata) -> u64 {
    metadata.len()
}

/// Validate the committed envelope authority and return only its content-addressed Loaf manifests.
pub fn committed_loaf_paths(loaf_root: &Path) -> Result<Vec<PathBuf>, OvenLoafError> {
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
    if let Some(member) = manifest.runtime_foundation.as_ref() {
        validate_release_runtime_foundation_member(&manifest, member).map_err(|message| {
            OvenLoafError::InvalidLoaf {
                path: manifest_path.clone(),
                message,
            }
        })?;
    }
    if let Some(member) = manifest.runtime_closure.as_ref() {
        validate_release_runtime_closure_member(&manifest, member).map_err(|message| OvenLoafError::InvalidLoaf {
            path: manifest_path.clone(),
            message,
        })?;
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
    if let Some(member) = manifest.runtime_foundation.as_ref() {
        validate_release_runtime_foundation_member(&manifest, member).map_err(|message| {
            OvenLoafError::InvalidLoaf {
                path: manifest_path.clone(),
                message,
            }
        })?;
    }
    if let Some(member) = manifest.runtime_closure.as_ref() {
        validate_release_runtime_closure_member(&manifest, member).map_err(|message| OvenLoafError::InvalidLoaf {
            path: manifest_path.clone(),
            message,
        })?;
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
pub fn committed_loaf_envelope_compatibility_identity(
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

/// One committed Loaf generation whose paths remain stable for the lifetime of its shared lock.
pub struct OvenCommittedLoafGeneration {
    generation_identity: String,
    paths: Vec<PathBuf>,
    _lock: OvenLoafGenerationLock,
}

/// A generation-held generic executable store member selected for an upper-layer consumer.
pub struct OvenHeldReleaseStoreMember {
    /// Publisher-selected member label.
    pub label: String,
    /// One verified executable materialized file below the held store entry.
    pub executable: PathBuf,
    /// Verified generic store payload whose active lease protects the complete entry.
    pub payload: OvenStoreExecutionPayload,
    _generation_lock: OvenLoafGenerationLock,
}

/// A physically admitted runtime foundation whose envelope generation remains shared-locked.
pub struct OvenHeldReleaseRuntimeFoundation {
    /// Publisher-selected member label.
    pub label: String,
    /// Materialized foundation and exact selected physical facts admitted from the held generation.
    pub asset: crate::rustc::OvenMaterializedRuntimeFoundationAsset,
    /// Exact compiled Loaf manifest whose artifact catalogue the foundation matches.
    pub compiled_loaf: PathBuf,
    /// Exact retained compiler/sysroot closure used for runtime rebuilds.
    pub compiler: crate::rustc::OvenRuntimeCompilerClosure,
    /// Exact rebuilt dependency closure admitted from this same held generation.
    pub closure: Option<crate::rustc::OvenSelectedRuntimeClosure>,
    _generation_lock: OvenLoafGenerationLock,
}

/// Select and prove one generic executable store member below a held envelope generation.
///
/// This is the single physical validation path used by both mirror admission and runtime acquisition. It assigns no
/// Incan meaning to the payload.
pub(crate) fn prove_release_store_member_payload(
    generation: &Path,
    member: &OvenReleaseStoreMember,
) -> Result<(OvenStoreExecutionPayload, PathBuf), OvenLoafError> {
    if member.schema_version != OVEN_RELEASE_STORE_MEMBER_SCHEMA_VERSION
        || !safe_generation_relative_path(&member.store_relative_path)
    {
        return Err(OvenLoafError::InvalidLoaf {
            path: generation.to_path_buf(),
            message: "release store member has an invalid descriptor".to_string(),
        });
    }
    let canonical_generation = fs::canonicalize(generation).map_err(|source| OvenLoafError::Io {
        path: generation.to_path_buf(),
        source,
    })?;
    let store_root = generation.join(&member.store_relative_path);
    let canonical_store_root = fs::canonicalize(&store_root).map_err(|source| OvenLoafError::Io {
        path: store_root.clone(),
        source,
    })?;
    if !canonical_store_root.starts_with(&canonical_generation) {
        return Err(OvenLoafError::InvalidLoaf {
            path: store_root,
            message: "release store member resolves outside its held generation".to_string(),
        });
    }
    let mut selected = PublishedOvenStore::new(&canonical_store_root)
        .select_payloads_matching_for_execution(|candidate| candidate.identity == member.artifact_identity)?;
    if selected.len() != 1 {
        return Err(OvenLoafError::InvalidLoaf {
            path: canonical_store_root,
            message: "release store member did not select exactly one declared artifact".to_string(),
        });
    }
    let payload = selected.pop().ok_or_else(|| OvenLoafError::Preparation {
        message: "release store selection became empty".to_string(),
    })?;
    payload.verify_materialized_files()?;
    if payload.manifest.kind != OvenArtifactKind::ProjectOutput {
        return Err(OvenLoafError::InvalidLoaf {
            path: canonical_store_root,
            message: "release store member must retain a ProjectOutput artifact".to_string(),
        });
    }
    let executables = payload
        .admitted_materialized_files()
        .iter()
        .filter(|file| file.executable)
        .collect::<Vec<_>>();
    if executables.len() != 1 {
        return Err(OvenLoafError::InvalidLoaf {
            path: canonical_store_root,
            message: "release store member must retain exactly one executable materialized file".to_string(),
        });
    }
    let executable = payload.artifact_root.join(&executables[0].relative_path);
    let canonical_executable = fs::canonicalize(&executable).map_err(|source| OvenLoafError::Io {
        path: executable,
        source,
    })?;
    if !canonical_executable.starts_with(&canonical_store_root) {
        return Err(OvenLoafError::InvalidLoaf {
            path: canonical_executable,
            message: "release store executable resolves outside its embedded store".to_string(),
        });
    }
    Ok((payload, canonical_executable))
}

/// Acquire and verify one labelled optional release store member under the committed generation lock.
///
/// The generic carrier verifies only the exact store identity, ProjectOutput kind, and singular executable file. It
/// neither decodes the payload nor assigns project, compiler, or policy meaning to those bytes.
pub fn acquire_committed_release_store_member(
    loaf_root: &Path,
    label: &str,
) -> Result<Option<OvenHeldReleaseStoreMember>, OvenLoafError> {
    if !loaf_root.join("envelope.json").is_file() {
        return Ok(None);
    }
    let generation_lock = acquire_loaf_generation_lock(loaf_root)?;
    let (manifest, manifest_path) = committed_loaf_envelope_manifest(loaf_root, "release")?;
    let Some(member) = manifest.release_store_member.as_ref() else {
        return Ok(None);
    };
    if member.label != label {
        return Ok(None);
    }
    let generation = generation_directory_path(&manifest.generation_identity);
    let (payload, executable) =
        prove_release_store_member_payload(&loaf_root.join(generation), member).map_err(|error| match error {
            OvenLoafError::InvalidLoaf { message, .. } => OvenLoafError::InvalidLoaf {
                path: manifest_path.clone(),
                message,
            },
            other => other,
        })?;
    Ok(Some(OvenHeldReleaseStoreMember {
        label: member.label.clone(),
        executable,
        payload,
        _generation_lock: generation_lock,
    }))
}

/// Acquire and prove one labelled runtime foundation under the committed release-generation lock.
///
/// This resolves only descriptor-bound paths below the held generation and performs physical identity, file-set,
/// owner and compiled-Loaf checks. It does not interpret package policy.
pub fn acquire_committed_release_runtime_foundation(
    loaf_root: &Path,
    label: &str,
) -> Result<Option<OvenHeldReleaseRuntimeFoundation>, OvenLoafError> {
    if !loaf_root.join("envelope.json").is_file() {
        return Ok(None);
    }
    let generation_lock = acquire_loaf_generation_lock(loaf_root)?;
    let (manifest, manifest_path) = committed_loaf_envelope_manifest(loaf_root, "release")?;
    let Some(member) = manifest.runtime_foundation.as_ref() else {
        return Ok(None);
    };
    if member.label != label {
        return Ok(None);
    }
    let asset = prove_release_runtime_foundation_member(loaf_root, &manifest, member).map_err(|error| match error {
        OvenLoafError::InvalidLoaf { message, .. } => OvenLoafError::InvalidLoaf {
            path: manifest_path,
            message,
        },
        other => other,
    })?;
    let compiled = manifest
        .loafs
        .iter()
        .find(|candidate| {
            candidate.loaf_identity == member.compiled_loaf_identity
                && candidate.plan_identity == member.compiled_plan_identity
                && candidate.role.provides_compiled_closure()
        })
        .ok_or_else(|| OvenLoafError::InvalidLoaf {
            path: loaf_root.join("envelope.json"),
            message: "runtime-foundation compiled Loaf binding disappeared".to_string(),
        })?;
    let closure = if let Some(closure_member) = manifest.runtime_closure.as_ref() {
        validate_release_runtime_closure_member(&manifest, closure_member).map_err(|message| {
            OvenLoafError::InvalidLoaf {
                path: loaf_root.join("envelope.json"),
                message,
            }
        })?;
        let store_root = loaf_root
            .join(generation_directory_path(&manifest.generation_identity))
            .join(&closure_member.store_relative_path);
        let mut candidates =
            PublishedOvenStore::new(&store_root).select_payloads_matching_for_execution(|candidate| {
                candidate.identity == closure_member.artifact_identity
            })?;
        if candidates.len() != 1 {
            return Err(OvenLoafError::InvalidLoaf {
                path: store_root,
                message: "runtime closure did not select exactly one declared Store artifact".to_string(),
            });
        }
        let payload = candidates.pop().ok_or_else(|| OvenLoafError::Preparation {
            message: "runtime closure selection became empty".to_string(),
        })?;
        payload.verify_materialized_files()?;
        Some(
            crate::rustc::admit_declared_runtime_closure(
                payload,
                &closure_member.closure_identity,
                &closure_member.foundation_identity,
                &closure_member.compiler_closure_identity,
            )
            .map_err(|error| OvenLoafError::InvalidLoaf {
                path: loaf_root.join("envelope.json"),
                message: error.to_string(),
            })?,
        )
    } else {
        None
    };
    Ok(Some(OvenHeldReleaseRuntimeFoundation {
        label: member.label.clone(),
        asset,
        compiled_loaf: loaf_root.join(&compiled.path),
        compiler: crate::rustc::OvenRuntimeCompilerClosure::new(
            loaf_root
                .join(generation_directory_path(&manifest.generation_identity))
                .join(&member.toolchain_root_relative_path)
                .join("bin/rustc"),
            member.compiler_closure_identity.clone(),
        ),
        closure,
        _generation_lock: generation_lock,
    }))
}

/// Return exact typed runtime members from an already committed release generation.
///
/// This is an explicit-publisher reuse probe. It validates the envelope and both descriptors but acquires no
/// execution paths; normal consumers use [`acquire_committed_release_runtime_foundation`] and retain its locks.
pub fn committed_release_runtime_members(
    loaf_root: &Path,
) -> Result<Option<(OvenReleaseRuntimeFoundationMember, OvenReleaseRuntimeClosureMember)>, OvenLoafError> {
    if !loaf_root.join("envelope.json").is_file() {
        return Ok(None);
    }
    let manifest_path = loaf_root.join("envelope.json");
    let bytes = fs::read(&manifest_path).map_err(|source| OvenLoafError::Io {
        path: manifest_path.clone(),
        source,
    })?;
    let manifest: OvenLoafEnvelopeManifest =
        serde_json::from_slice(&bytes).map_err(|error| OvenLoafError::InvalidLoaf {
            path: manifest_path.clone(),
            message: format!("invalid committed envelope manifest: {error}"),
        })?;
    if manifest.schema_version != OVEN_LOAF_ENVELOPE_MANIFEST_SCHEMA_VERSION || manifest.envelope != "release" {
        return Ok(None);
    }
    let (Some(foundation), Some(closure)) = (manifest.runtime_foundation.as_ref(), manifest.runtime_closure.as_ref())
    else {
        return Ok(None);
    };
    validate_release_runtime_foundation_member(&manifest, foundation).map_err(|message| {
        OvenLoafError::InvalidLoaf {
            path: manifest_path.clone(),
            message,
        }
    })?;
    validate_release_runtime_closure_member(&manifest, closure).map_err(|message| OvenLoafError::InvalidLoaf {
        path: manifest_path,
        message,
    })?;
    Ok(Some((foundation.clone(), closure.clone())))
}

/// Construct the committed generation path named by one canonical envelope identity.
fn generation_directory_path(generation_identity: &str) -> PathBuf {
    Path::new("generations").join(
        generation_identity
            .strip_prefix("sha256:")
            .unwrap_or(generation_identity),
    )
}

/// Reject absolute or escaping embedded-store roots before opening any store files.
fn safe_generation_relative_path(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && !path.is_absolute()
        && path
            .components()
            .all(|component| matches!(component, std::path::Component::Normal(_)))
}

impl OvenCommittedLoafGeneration {
    /// Return the exact committed envelope generation protected by this shared lock.
    pub fn generation_identity(&self) -> &str {
        &self.generation_identity
    }

    /// Return the verified Loaf manifests protected by this generation's shared lock.
    pub fn paths(&self) -> &[PathBuf] {
        &self.paths
    }
}

/// Acquire a shared generation lock and resolve the complete currently committed Loaf set.
pub fn acquire_committed_loaf_generation(
    loaf_root: &Path,
) -> Result<Option<OvenCommittedLoafGeneration>, OvenLoafError> {
    if !loaf_root.join("envelope.json").is_file() {
        return Ok(None);
    }
    let generation_lock = acquire_loaf_generation_lock(loaf_root)?;
    let (manifest, _) = committed_loaf_envelope_manifest(loaf_root, "compiler-suite")?;
    let paths = committed_loaf_paths(loaf_root)?;
    Ok(Some(OvenCommittedLoafGeneration {
        generation_identity: manifest.generation_identity,
        paths,
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
    intent: &oven_store::OvenBuildIntent,
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
    let loaf_root = oven_model::toolchain_layout::resolve_toolchain_data_path(Path::new(TOOLCHAIN_LOAF_RELATIVE_ROOT));
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
    let loaf_root = oven_model::toolchain_layout::resolve_toolchain_data_path(Path::new(TOOLCHAIN_LOAF_RELATIVE_ROOT));
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
    let loaf_root = oven_model::toolchain_layout::resolve_toolchain_data_path(Path::new(TOOLCHAIN_LOAF_RELATIVE_ROOT));
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

    // Deciding which candidate can supply the caller's registry roots needs only its sealed catalog, already read
    // beside its manifest. Materializing each candidate's whole closure here -- ten thousand file checks per Loaf --
    // was the single largest cost of a warm no-change build (#1111); the one candidate selected below is still
    // materialized and validated in full.
    let candidates = compatible_loaf_paths(&loaf_root, receipt)?;
    let profile = receipt.intent.profile.as_str();
    let Some(candidate) =
        select_compatible_loaf_with_registry_requirement(candidates, &registry_dependencies, |candidate| {
            let candidate_root = candidate.path.parent().ok_or_else(|| OvenLoafError::InvalidLoaf {
                path: candidate.path.clone(),
                message: "loaf file has no parent directory".to_string(),
            })?;
            Ok(match registry_requirement {
                OvenLoafRegistryRequirement::LinkableLeaf => {
                    let authority = OvenRegistryLeafAuthority::new_with_trusted_dependency_search_paths(
                        candidate_root.to_path_buf(),
                        candidate.registry_leaves.clone(),
                        Vec::new(),
                    );
                    registry_dependencies
                        .iter()
                        .all(|dependency| validate_sealed_registry_leaf(dependency, Some(&authority), profile).is_ok())
                }
            })
        })?
    else {
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
                registry_leaves: loaf.registry_leaves,
            });
        }
    }
    candidates.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(candidates)
}

/// Explain why no compiler-owned Loaf could serve `receipt`.
///
/// The nested-build guard reported a missing registry-dependency set whenever compiler-owned selection came back
/// empty. When the receipt declares no registry dependency that rendered as "Needs: none.", which points a reader at
/// dependency resolution while the rejection actually happened during Loaf compatibility. This walks the same
/// committed Loafs `compatible_loaf_paths` walks and tallies which condition rejected each one, so the caller can name
/// the condition that actually held rather than the one it assumed.
///
/// This is diagnostic only: it never selects, locks, or mutates a Loaf, and a read failure degrades to a short note
/// rather than masking the caller's original error.
pub fn describe_compiler_owned_loaf_miss(receipt: &OvenReceipt) -> String {
    let loaf_root = oven_model::toolchain_layout::resolve_toolchain_data_path(Path::new(TOOLCHAIN_LOAF_RELATIVE_ROOT));
    if !loaf_root.join("envelope.json").is_file() {
        return "no committed compiler-owned Loaf envelope exists".to_string();
    }
    let paths = match committed_loaf_metadata_paths_for_authority(&loaf_root, OvenLoafMemberRole::CompiledClosure) {
        Ok(paths) => paths,
        Err(error) => return format!("committed Loaf metadata could not be read: {error}"),
    };
    let (mut schema, mut same_unit, mut intent, mut providers, mut total) =
        (0_usize, 0_usize, 0_usize, 0_usize, 0_usize);
    let mut sample_intent = None;
    let mut sample_provider: Option<String> = None;
    for path in paths {
        let Ok(loaf) = read_loaf(&path) else { continue };
        total += 1;
        if loaf.schema_version != OVEN_LOAF_SCHEMA_VERSION {
            schema += 1;
        } else if loaf.build_unit_identity == receipt.build_unit_identity {
            same_unit += 1;
        } else if loaf.plan.intent != receipt.intent {
            intent += 1;
            if sample_intent.is_none() {
                sample_intent = Some(loaf.plan.intent.clone());
            }
        } else if let Ok(Some(reason)) = loaf.compatibility.provider_subset_rejection(receipt) {
            providers += 1;
            if sample_provider.is_none() {
                sample_provider = Some(reason);
            }
        }
    }
    if total == 0 {
        return "no committed compiler-owned Loaf exists for this authority".to_string();
    }
    let requested = describe_build_intent(&receipt.intent);
    let mut reasons = Vec::new();
    if schema > 0 {
        reasons.push(format!("{schema} on a different Loaf schema"));
    }
    if same_unit > 0 {
        reasons.push(format!("{same_unit} already rejected as this receipt's own build unit"));
    }
    if intent > 0 {
        let sample = sample_intent.as_ref().map_or_else(String::new, |found| {
            format!(" (one such Loaf declares {})", describe_build_intent(found))
        });
        reasons.push(format!("{intent} on a different build intent{sample}"));
    }
    if providers > 0 {
        let sample = sample_provider
            .as_ref()
            .map_or_else(String::new, |found| format!(" (one because {found})"));
        reasons.push(format!("{providers} on runtime inputs or provider coverage{sample}"));
    }
    if reasons.is_empty() {
        return format!("{total} committed compiler-owned Loaf(s) exist and none matched the request for {requested}");
    }
    format!(
        "the request is for {requested}; of {total} committed compiler-owned Loaf(s), {}",
        reasons.join(", ")
    )
}

/// Render one build intent as a short, stable, single-line phrase for diagnostics.
fn describe_build_intent(intent: &oven_store::OvenBuildIntent) -> String {
    let features = if intent.features.is_empty() {
        "no features".to_string()
    } else {
        format!("features [{}]", intent.features.join(", "))
    };
    format!(
        "target `{}`, toolchain `{}`, profile `{}`, {features}",
        intent.target, intent.toolchain, intent.profile
    )
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
/// closure merely to evaluate an empty conjunction; a caller with registry roots proves them against each
/// candidate's sealed catalog, which its manifest already carries. The final caller validates only the selected
/// Loaf, in full, before Rustc receives any artifact path.
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

/// Verify one Loaf for `receipt` without retaining a generation lock; the test-side spelling of the lock-holding
/// resolver below, which production selection always uses.
#[cfg(test)]
fn loaf_from_loaf(
    receipt: &OvenReceipt,
    loaf_path: &Path,
    selection: OvenLoafSelection,
) -> Result<OvenToolchainLoaf, OvenLoafError> {
    loaf_from_loaf_with_lock(receipt, loaf_path, selection, None)
}

/// Verify that one Loaf authorizes `receipt` and resolve its compiler-owned direct-Rustc closure, retaining an
/// optional generation-lifetime lock.
///
/// This is the normal consumer boundary. It verifies the content-addressed manifest, the receipt/compatibility
/// relationship, the registry catalog, and every declared file used by the resulting Rustc plan. It intentionally
/// does not recursively inspect unrelated files in the immutable directory: those files cannot become a Rustc input
/// through the sealed manifest, while walking complete source trees on every command would make a prepared Loaf
/// behave like a cold cache. [`committed_loaf_paths`] retains the explicit whole-Loaf audit for publication and
/// inspection flows.
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
    // The envelope's generation lock is held (shared) by every caller that reaches a committed Loaf, so the proof
    // below is written into a root no publisher is replacing; a Loaf reached outside an envelope has nowhere to
    // keep one and takes the full walk every time.
    let artifact_plan = match closure_proof_path(loaf_path, &loaf_identity) {
        Some(proof_path) => {
            loaf.plan
                .materialize_proven_store(artifact_root, &receipt.intent, &loaf_identity, &proof_path)?
        }
        None => loaf.plan.materialize_trusted_store(artifact_root, &receipt.intent)?,
    };
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

/// Validate a stored Loaf independently of a generated-project receipt.
///
/// The envelope manifest binds the Loaf identity to release-family SDK, toolchain, lock, and fixture evidence.
/// This check then verifies the immutable payload itself, allowing a complete warm baker invocation to avoid
/// rerunning compiler behaviour merely to rediscover an already-bound receipt.
pub fn validate_stored_loaf(
    loaf_path: &Path,
    expected_build_unit_identity: &str,
) -> Result<OvenLoafPreparation, OvenLoafError> {
    let loaf = read_loaf(loaf_path)?;
    let loaf_identity = validate_loaf_content_address(loaf_path)?;
    validate_loaf_declared_file_set(&loaf, loaf_path)?;
    if loaf.schema_version != OVEN_LOAF_SCHEMA_VERSION {
        return Err(OvenLoafError::InvalidLoaf {
            path: loaf_path.to_path_buf(),
            message: format!(
                "schema version {} is unsupported (expected {})",
                loaf.schema_version, OVEN_LOAF_SCHEMA_VERSION
            ),
        });
    }
    if loaf.build_unit_identity != expected_build_unit_identity {
        return Err(OvenLoafError::InvalidLoaf {
            path: loaf_path.to_path_buf(),
            message: "build-unit identity does not match the envelope manifest".to_string(),
        });
    }
    if loaf.plan.registry_leaves != loaf.registry_leaves {
        return Err(OvenLoafError::InvalidLoaf {
            path: loaf_path.to_path_buf(),
            message: "loaf registry catalog does not match its copied direct-rustc plan".to_string(),
        });
    }
    validate_registry_leaf_catalog(&loaf, loaf_path)?;
    let artifact_root = loaf_path.parent().ok_or_else(|| OvenLoafError::InvalidLoaf {
        path: loaf_path.to_path_buf(),
        message: "loaf file has no parent directory".to_string(),
    })?;
    loaf.plan.materialize_trusted_store(artifact_root, &loaf.plan.intent)?;
    let plan_identity = digest_bytes(
        &serde_json::to_vec(&loaf.plan).map_err(|error| OvenLoafError::Preparation {
            message: format!("could not encode reused Loaf plan identity: {error}"),
        })?,
    );
    let (logical_bytes, physical_bytes) = loaf_directory_byte_counts(artifact_root)?;
    let manifest_logical_bytes = fs::symlink_metadata(loaf_path)
        .map_err(|source| OvenLoafError::Io {
            path: loaf_path.to_path_buf(),
            source,
        })?
        .len();
    let payload_logical_bytes = logical_bytes.saturating_sub(manifest_logical_bytes);
    if loaf.accounting.payload_logical_bytes != payload_logical_bytes {
        return Err(OvenLoafError::InvalidLoaf {
            path: loaf_path.to_path_buf(),
            message: format!(
                "payload accounting declares {} logical bytes but the verified payload contains {payload_logical_bytes}",
                loaf.accounting.payload_logical_bytes
            ),
        });
    }
    Ok(OvenLoafPreparation {
        build_unit_identity: loaf.build_unit_identity,
        loaf_identity,
        plan_identity,
        logical_bytes,
        physical_bytes,
        transient_peak_physical_bytes: 0,
    })
}

/// Validate the small authority surface required for an exact default reuse decision.
///
/// This hashes `loaf.json`, checks its content-addressed directory name and typed identities, and verifies the plan
/// digest already committed by the envelope. It deliberately does not rehash every artifact. Any later selected Loaf
/// still performs full materialization verification while its generation lease is held; operators can likewise use
/// the explicit inspection path for an eager whole-envelope audit.
pub fn validate_stored_loaf_for_reuse(
    loaf_path: &Path,
    member: &OvenLoafEnvelopeMember,
) -> Result<OvenLoafPreparation, OvenLoafError> {
    let loaf = read_loaf(loaf_path)?;
    let loaf_identity = validate_loaf_content_address(loaf_path)?;
    if loaf_identity != member.loaf_identity {
        return Err(OvenLoafError::InvalidLoaf {
            path: loaf_path.to_path_buf(),
            message: "content identity does not match the envelope manifest".to_string(),
        });
    }
    if loaf.schema_version != OVEN_LOAF_SCHEMA_VERSION || loaf.build_unit_identity != member.build_unit_identity {
        return Err(OvenLoafError::InvalidLoaf {
            path: loaf_path.to_path_buf(),
            message: "schema or build-unit identity does not match the envelope manifest".to_string(),
        });
    }
    if loaf.plan.registry_leaves != loaf.registry_leaves {
        return Err(OvenLoafError::InvalidLoaf {
            path: loaf_path.to_path_buf(),
            message: "loaf registry catalog does not match its copied direct-rustc plan".to_string(),
        });
    }
    let plan_identity = digest_bytes(
        &serde_json::to_vec(&loaf.plan).map_err(|error| OvenLoafError::Preparation {
            message: format!("could not encode reused Loaf plan identity: {error}"),
        })?,
    );
    if plan_identity != member.plan_identity {
        return Err(OvenLoafError::InvalidLoaf {
            path: loaf_path.to_path_buf(),
            message: "plan identity does not match the envelope manifest".to_string(),
        });
    }
    Ok(OvenLoafPreparation {
        build_unit_identity: loaf.build_unit_identity,
        loaf_identity,
        plan_identity,
        logical_bytes: member.logical_bytes,
        physical_bytes: member.physical_bytes,
        transient_peak_physical_bytes: 0,
    })
}

/// Verify that a Loaf manifest's digest is also the name of its containing `.loaf` directory.
fn validate_loaf_content_address(loaf_path: &Path) -> Result<String, OvenLoafError> {
    let identity = loaf_file_identity(loaf_path)?;
    let expected_name = format!("{}.loaf", identity.strip_prefix("sha256:").unwrap_or(&identity));
    let actual_name = loaf_path
        .parent()
        .and_then(Path::file_name)
        .and_then(|name| name.to_str());
    if actual_name != Some(expected_name.as_str()) {
        return Err(OvenLoafError::InvalidLoaf {
            path: loaf_path.to_path_buf(),
            message: format!("content identity is {identity}, but its directory is not named `{expected_name}`"),
        });
    }
    Ok(identity)
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

/// Where the closure proof for a committed Loaf lives: beside its envelope, never inside the sealed `.loaf`
/// directory, whose declared file set admits nothing undeclared. `None` when `loaf_path` is not below an envelope.
fn closure_proof_path(loaf_path: &Path, loaf_identity: &str) -> Option<PathBuf> {
    let envelope_root = loaf_path
        .ancestors()
        .find(|ancestor| ancestor.join("envelope.json").is_file())?;
    Some(OvenClosureProof::path(envelope_root, loaf_identity))
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
    use super::{OvenReleaseToolchainMember, stage_release_runtime_foundation_toolchain};

    use std::collections::{BTreeMap, BTreeSet};
    use std::path::{Path, PathBuf};
    use std::sync::mpsc;
    use std::time::Duration;
    use std::{fs, thread};

    use oven_model::manifest::{DependencySource, DependencySpec};
    use oven_store::store::{
        OvenArtifactKind, OvenArtifactMaterializedFile, OvenStore, OvenStoreLimits, PublishedOvenStore,
    };
    use oven_store::test_support::{request as store_request, write_project as write_store_project};
    use oven_store::{OvenGeneratedProjectRequest, digest_bytes, digest_source_tree, receipt_generated_project};

    use super::{
        CompatibleLoaf, OVEN_LOAF_ENVELOPE_LOCK_FILE, OVEN_LOAF_ENVELOPE_MANIFEST_SCHEMA_VERSION,
        OVEN_LOAF_SCHEMA_VERSION, OVEN_RELEASE_RUNTIME_FOUNDATION_MEMBER_SCHEMA_VERSION,
        OVEN_RELEASE_STORE_MEMBER_SCHEMA_VERSION, OvenLoaf, OvenLoafCompatibility, OvenLoafEnvelope,
        OvenLoafEnvelopeManifest, OvenLoafEnvelopeMember, OvenLoafError, OvenLoafFixtureAction, OvenLoafMemberRole,
        OvenLoafSelection, OvenReleaseRuntimeFoundationMember, OvenReleaseStoreMember,
        acquire_committed_release_store_member, acquire_exclusive_loaf_generation_lock, acquire_loaf_generation_lock,
        bind_release_runtime_foundation_evidence, closure_proof_path, committed_loaf_envelope_compatibility_identity,
        committed_loaf_paths, digest_runtime_crate_source, generation_directory_path, loaf_envelope_specifications,
        loaf_from_loaf, prove_release_store_member_payload, registry_source_dependencies_supported_by_catalog,
        select_most_specific_compatible_loaf, validate_loaf_declared_file_set,
        validate_release_runtime_foundation_member,
    };

    #[cfg(unix)]
    fn publish_release_store_fixture(
        generation: &Path,
        kind: OvenArtifactKind,
        executable_count: usize,
    ) -> Result<OvenReleaseStoreMember, Box<dyn std::error::Error>> {
        use std::os::unix::fs::PermissionsExt;

        let project = tempfile::tempdir()?;
        write_store_project(project.path())?;
        let sources = tempfile::tempdir()?;
        let mut request = store_request(project.path(), "release-member", b"opaque release payload")?;
        request.kind = kind;
        for index in 0..executable_count {
            let source = sources.path().join(format!("engine-{index}"));
            fs::write(&source, b"#!/bin/sh\nexit 0\n")?;
            fs::set_permissions(&source, fs::Permissions::from_mode(0o755))?;
            request.materialized_files.push(OvenArtifactMaterializedFile {
                source_path: source,
                relative_path: format!("bin/engine-{index}"),
            });
        }
        let store_relative_path = PathBuf::from("release-store");
        let store = OvenStore::new(
            generation.join(&store_relative_path),
            OvenStoreLimits::new(1_000_000, 1_000_000, 1_000_000),
        );
        let manifest = store.publish(&request)?;
        Ok(OvenReleaseStoreMember {
            schema_version: OVEN_RELEASE_STORE_MEMBER_SCHEMA_VERSION,
            label: "engine".to_string(),
            store_relative_path,
            artifact_identity: manifest.identity,
        })
    }

    #[cfg(unix)]
    #[test]
    fn release_store_member_proof_accepts_one_project_output_executable_and_holds_its_lease()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let generation = root.path().join("generation");
        fs::create_dir_all(&generation)?;
        let member = publish_release_store_fixture(&generation, OvenArtifactKind::ProjectOutput, 1)?;
        let (payload, executable) = prove_release_store_member_payload(&generation, &member)?;
        assert!(executable.is_file());
        let lease = fs::File::open(
            payload
                .artifact_root
                .parent()
                .ok_or("materialized root has no entry")?
                .join(".active.lock"),
        )?;
        assert!(matches!(lease.try_lock(), Err(fs::TryLockError::WouldBlock)));
        drop(payload);
        lease.try_lock()?;
        Ok(())
    }

    #[test]
    fn runtime_foundation_member_binds_one_compiled_loaf_and_safe_distinct_roots()
    -> Result<(), Box<dyn std::error::Error>> {
        let compiled = OvenLoafEnvelopeMember {
            label: "release-core".to_string(),
            profile: "release".to_string(),
            action: "build".to_string(),
            role: OvenLoafMemberRole::CompiledClosure,
            build_unit_identity: digest_bytes(b"unit"),
            loaf_identity: digest_bytes(b"loaf"),
            plan_identity: digest_bytes(b"plan"),
            logical_bytes: 1,
            physical_bytes: 1,
            path: PathBuf::from("generations/foundation/release.loaf/loaf.json"),
        };
        let member = OvenReleaseRuntimeFoundationMember {
            schema_version: OVEN_RELEASE_RUNTIME_FOUNDATION_MEMBER_SCHEMA_VERSION,
            label: "rust-policy-foundation".to_string(),
            foundation_relative_path: PathBuf::from("runtime-foundation/foundation"),
            foundation_identity: digest_bytes(b"foundation"),
            compiled_loaf_identity: compiled.loaf_identity.clone(),
            compiled_plan_identity: compiled.plan_identity.clone(),
            toolchain_owner_identity: digest_bytes(b"toolchain-owner"),
            compiler_closure_identity: digest_bytes(b"compiler-closure"),
            toolchain_root_relative_path: PathBuf::from("runtime-foundation/toolchain"),
            toolchain_members: vec![OvenReleaseToolchainMember {
                relative_path: PathBuf::from("bin/rustc"),
                digest: digest_bytes(b"rustc"),
            }],
        };
        let mut evidence = BTreeMap::new();
        bind_release_runtime_foundation_evidence(&mut evidence, &member)?;
        let manifest = OvenLoafEnvelopeManifest {
            schema_version: OVEN_LOAF_ENVELOPE_MANIFEST_SCHEMA_VERSION,
            envelope: "release".to_string(),
            generation_identity: digest_bytes(b"generation"),
            evidence,
            loafs: vec![compiled.clone()],
            release_store_member: None,
            runtime_foundation: None,
            runtime_closure: None,
        };
        validate_release_runtime_foundation_member(&manifest, &member)?;

        let mut unbound = manifest.clone();
        unbound.evidence.clear();
        assert!(validate_release_runtime_foundation_member(&unbound, &member).is_err());
        let mut wrong_label = member.clone();
        wrong_label.label = "other-foundation".to_string();
        assert!(validate_release_runtime_foundation_member(&manifest, &wrong_label).is_err());

        let mut wrong_plan = member.clone();
        wrong_plan.compiled_plan_identity = digest_bytes(b"other-plan");
        assert!(validate_release_runtime_foundation_member(&manifest, &wrong_plan).is_err());
        let mut escaped = member.clone();
        escaped.foundation_relative_path = PathBuf::from("../outside");
        assert!(validate_release_runtime_foundation_member(&manifest, &escaped).is_err());
        let mut aliased_roots = member;
        aliased_roots.toolchain_root_relative_path = aliased_roots.foundation_relative_path.clone();
        assert!(validate_release_runtime_foundation_member(&manifest, &aliased_roots).is_err());
        let mut toolchain_nested = aliased_roots.clone();
        toolchain_nested.toolchain_root_relative_path = toolchain_nested.foundation_relative_path.join("toolchain");
        assert!(validate_release_runtime_foundation_member(&manifest, &toolchain_nested).is_err());
        let mut foundation_nested = aliased_roots;
        foundation_nested.foundation_relative_path = foundation_nested.toolchain_root_relative_path.join("foundation");
        assert!(validate_release_runtime_foundation_member(&manifest, &foundation_nested).is_err());
        Ok(())
    }

    /// A closure descriptor is valid only beside the exact foundation and compiler closure it rebuilds.
    #[test]
    fn runtime_closure_member_binds_same_generation_foundation() -> Result<(), Box<dyn std::error::Error>> {
        let foundation = OvenReleaseRuntimeFoundationMember {
            schema_version: OVEN_RELEASE_RUNTIME_FOUNDATION_MEMBER_SCHEMA_VERSION,
            label: "rust-policy-foundation".to_string(),
            foundation_relative_path: PathBuf::from("runtime-foundation/foundation"),
            foundation_identity: digest_bytes(b"foundation"),
            compiled_loaf_identity: digest_bytes(b"loaf"),
            compiled_plan_identity: digest_bytes(b"plan"),
            toolchain_owner_identity: digest_bytes(b"toolchain-owner"),
            compiler_closure_identity: digest_bytes(b"compiler-closure"),
            toolchain_root_relative_path: PathBuf::from("runtime-foundation/toolchain"),
            toolchain_members: Vec::new(),
        };
        let closure = OvenReleaseRuntimeClosureMember {
            schema_version: OVEN_RELEASE_RUNTIME_CLOSURE_MEMBER_SCHEMA_VERSION,
            label: "rust-policy-closure".to_string(),
            store_relative_path: PathBuf::from("runtime-closures/store"),
            artifact_identity: digest_bytes(b"artifact"),
            closure_identity: digest_bytes(b"closure"),
            foundation_identity: foundation.foundation_identity.clone(),
            compiler_closure_identity: foundation.compiler_closure_identity.clone(),
        };
        let mut evidence = BTreeMap::new();
        bind_release_runtime_closure_evidence(&mut evidence, &closure)?;
        let manifest = OvenLoafEnvelopeManifest {
            schema_version: OVEN_LOAF_ENVELOPE_MANIFEST_SCHEMA_VERSION,
            envelope: "release".to_string(),
            generation_identity: digest_bytes(b"generation"),
            evidence,
            loafs: Vec::new(),
            release_store_member: None,
            runtime_foundation: Some(foundation),
            runtime_closure: Some(closure.clone()),
        };
        validate_release_runtime_closure_member(&manifest, &closure)?;

        let mut substituted = closure.clone();
        substituted.foundation_identity = digest_bytes(b"other-foundation");
        assert!(validate_release_runtime_closure_member(&manifest, &substituted).is_err());
        let mut missing_foundation = manifest;
        missing_foundation.runtime_foundation = None;
        assert!(validate_release_runtime_closure_member(&missing_foundation, &closure).is_err());
        Ok(())
    }

    #[test]
    fn retained_runtime_compiler_executes_after_source_closure_is_removed() -> Result<(), Box<dyn std::error::Error>> {
        let rustc = if let Some(rustc) = std::env::var_os("RUSTC") {
            PathBuf::from(rustc)
        } else {
            let selected = std::process::Command::new("rustup").args(["which", "rustc"]).output()?;
            if !selected.status.success() {
                return Err("rustup could not locate the selected rustc".into());
            }
            PathBuf::from(String::from_utf8(selected.stdout)?.trim())
        };
        let rustc = fs::canonicalize(rustc)?;
        let target = crate::rustc::rustc_host_target(&rustc)?;
        let first_parent = tempfile::tempdir()?;
        let first = first_parent.path().join("source-toolchain");
        let (first_identity, _) = stage_release_runtime_foundation_toolchain(&rustc, &target, &first)?;
        let second_parent = tempfile::tempdir()?;
        let second = second_parent.path().join("retained-toolchain");
        let (second_identity, second_members) =
            stage_release_runtime_foundation_toolchain(&first.join("bin/rustc"), &target, &second)?;
        assert_eq!(second_identity, first_identity);
        assert!(
            second_members
                .iter()
                .any(|member| member.relative_path == Path::new("bin/rustc"))
        );

        drop(first_parent);
        let output = std::process::Command::new(second.join("bin/rustc"))
            .arg("-vV")
            .output()?;
        assert!(
            output.status.success(),
            "retained rustc did not execute: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let sysroot = std::process::Command::new(second.join("bin/rustc"))
            .args(["--print", "sysroot"])
            .output()?;
        assert!(sysroot.status.success());
        assert_eq!(
            fs::canonicalize(String::from_utf8(sysroot.stdout)?.trim())?,
            fs::canonicalize(&second)?,
            "retained rustc must select the retained sysroot rather than an ambient installation"
        );
        let source = second_parent.path().join("detached.rs");
        let executable = second_parent
            .path()
            .join(format!("detached{}", std::env::consts::EXE_SUFFIX));
        fs::write(&source, "fn main() { println!(\"retained sysroot\"); }\n")?;
        let compile = std::process::Command::new(second.join("bin/rustc"))
            .arg(&source)
            .arg("-o")
            .arg(&executable)
            .output()?;
        assert!(
            compile.status.success(),
            "retained rustc could not compile with its detached sysroot: {}",
            String::from_utf8_lossy(&compile.stderr)
        );
        let executed = std::process::Command::new(&executable).output()?;
        assert!(executed.status.success());
        assert_eq!(String::from_utf8(executed.stdout)?, "retained sysroot\n");
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn committed_release_store_member_is_optional_and_runtime_acquires_the_real_payload()
    -> Result<(), Box<dyn std::error::Error>> {
        let absent = tempfile::tempdir()?;
        assert!(acquire_committed_release_store_member(absent.path(), "engine")?.is_none());

        let root = tempfile::tempdir()?;
        let generation_identity = digest_bytes(b"release generation");
        let generation = root.path().join(generation_directory_path(&generation_identity));
        fs::create_dir_all(&generation)?;
        let member = publish_release_store_fixture(&generation, OvenArtifactKind::ProjectOutput, 1)?;
        fs::write(root.path().join(OVEN_LOAF_ENVELOPE_LOCK_FILE), b"")?;
        fs::write(
            root.path().join("envelope.json"),
            serde_json::to_vec(&OvenLoafEnvelopeManifest {
                schema_version: OVEN_LOAF_ENVELOPE_MANIFEST_SCHEMA_VERSION,
                envelope: "release".to_string(),
                generation_identity,
                evidence: BTreeMap::new(),
                loafs: Vec::new(),
                release_store_member: Some(member),
                runtime_foundation: None,
                runtime_closure: None,
            })?,
        )?;
        let held = acquire_committed_release_store_member(root.path(), "engine")?.ok_or("member not acquired")?;
        assert!(held.executable.is_file());
        assert_eq!(held.payload.manifest.kind, OvenArtifactKind::ProjectOutput);
        let generation_lock = fs::File::open(root.path().join(OVEN_LOAF_ENVELOPE_LOCK_FILE))?;
        assert!(matches!(generation_lock.try_lock(), Err(fs::TryLockError::WouldBlock)));
        drop(held);
        generation_lock.try_lock()?;
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn release_store_member_proof_refuses_wrong_kind_and_multiple_executables() -> Result<(), Box<dyn std::error::Error>>
    {
        for (kind, count) in [(OvenArtifactKind::Engine, 1), (OvenArtifactKind::ProjectOutput, 2)] {
            let root = tempfile::tempdir()?;
            let generation = root.path().join("generation");
            fs::create_dir_all(&generation)?;
            let member = publish_release_store_fixture(&generation, kind, count)?;
            assert!(prove_release_store_member_payload(&generation, &member).is_err());
        }
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn release_store_member_proof_refuses_tampered_bytes_and_store_symlink_escape()
    -> Result<(), Box<dyn std::error::Error>> {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir()?;
        let generation = root.path().join("generation");
        fs::create_dir_all(&generation)?;
        let member = publish_release_store_fixture(&generation, OvenArtifactKind::ProjectOutput, 1)?;
        let selected = PublishedOvenStore::new(generation.join(&member.store_relative_path))
            .select_payloads_matching_for_execution(|candidate| candidate.identity == member.artifact_identity)?;
        let executable = selected[0].artifact_root.join("bin/engine-0");
        drop(selected);
        fs::remove_file(&executable)?;
        fs::write(&executable, b"tampered executable")?;
        assert!(prove_release_store_member_payload(&generation, &member).is_err());

        let outside = tempfile::tempdir()?;
        let escaped = generation.join("escaped-store");
        symlink(outside.path(), &escaped)?;
        let escaped_member = OvenReleaseStoreMember {
            store_relative_path: PathBuf::from("escaped-store"),
            ..member
        };
        assert!(prove_release_store_member_payload(&generation, &escaped_member).is_err());
        Ok(())
    }
    use crate::rustc::{
        OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION, OvenRustcArtifactExtern, OvenRustcArtifactManifest,
        OvenRustcArtifactPlan, OvenRustcRegistryLeaf, OvenRustcRegistrySource, OvenRustcRegistrySourcePackage,
        OvenRustcSupportingArtifact,
    };
    fn runtime_receipt(
        source: &Path,
        providers: &str,
        rust_dependencies: &str,
        stdlib_facets: &str,
    ) -> Result<oven_store::OvenReceipt, Box<dyn std::error::Error>> {
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
        .with_build_unit_input("stdlib-facets", stdlib_facets)
        .with_build_unit_input("provider-plan", provider_plan);
        if !providers.is_empty() {
            request = request.with_build_unit_input("providers", providers);
        }
        Ok(receipt_generated_project(&request)?)
    }

    fn runtime_receipt_for_plan() -> Result<oven_store::OvenReceipt, Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let source = root.path().join("main.rs");
        fs::write(&source, "fn main() {}\n")?;
        // The receipt owns no filesystem path, so retaining only its value is valid after this helper drops the
        // temporary source tree.
        runtime_receipt(&source, "", "empty-rust-dependencies", "empty-stdlib-facets")
    }

    fn empty_manifest(receipt: &oven_store::OvenReceipt) -> OvenRustcArtifactManifest {
        OvenRustcArtifactManifest {
            schema_version: OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION,
            intent: receipt.intent.clone(),
            dependency_search_paths: Vec::new(),
            native_search_paths: Vec::new(),
            externs: Vec::new(),
            entrypoint_dependency_search_paths: Default::default(),
            entrypoint_externs: BTreeMap::new(),
            registry_leaves: Vec::new(),
            registry_sources: Vec::new(),
            compile_environment: BTreeMap::new(),
            vocab_auxiliary_targets: Vec::new(),
            supporting_artifacts: Vec::new(),
        }
    }

    /// A committed Loaf's closure proof lives beside its envelope, never inside the sealed `.loaf` directory whose
    /// declared file set admits nothing undeclared, and a `loaf.json` below no envelope has nowhere to put one.
    #[test]
    fn a_closure_proof_lives_beside_the_envelope_and_never_inside_the_loaf_issue1546()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let envelope = root.path().join("stdlib");
        let loaf_dir = envelope.join("generations").join("gen-1").join("abc.loaf");
        fs::create_dir_all(&loaf_dir)?;
        fs::write(envelope.join("envelope.json"), "{}")?;
        let loaf_path = loaf_dir.join("loaf.json");
        fs::write(&loaf_path, "{}")?;

        let proof = closure_proof_path(&loaf_path, "sha256:abc").ok_or("a committed Loaf has a proof path")?;
        assert_eq!(proof, envelope.join("closure-proofs").join("sha256-abc.json"));
        assert!(
            !proof.starts_with(&loaf_dir),
            "the proof must not be filed inside the sealed Loaf directory: {}",
            proof.display()
        );

        let detached = root.path().join("detached").join("loaf.json");
        fs::create_dir_all(detached.parent().ok_or("parent")?)?;
        fs::write(&detached, "{}")?;
        assert!(closure_proof_path(&detached, "sha256:abc").is_none());
        Ok(())
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
                intent: oven_store::OvenBuildIntent {
                    target: "fixture-target".to_string(),
                    toolchain: "fixture-rustc".to_string(),
                    profile: "release".to_string(),
                    features: Vec::new(),
                },
                dependency_search_paths: Vec::new(),
                native_search_paths: Vec::new(),
                externs: Vec::new(),
                entrypoint_dependency_search_paths: Default::default(),
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
                release_store_member: None,
                runtime_foundation: None,
                runtime_closure: None,
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
                        release_store_member: None,
                        runtime_foundation: None,
                        runtime_closure: None,
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
        let exclusive = acquire_exclusive_loaf_generation_lock(root.path())?;
        drop(exclusive);
        let reader = acquire_loaf_generation_lock(root.path())?;
        let path = root.path().to_path_buf();
        let (sender, receiver) = mpsc::channel();
        let replacement = thread::spawn(move || {
            let lock = acquire_exclusive_loaf_generation_lock(&path);
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
                source_path_projection: None,
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
                entrypoint_dependency_search_paths: Default::default(),
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
                entrypoint_dependency_search_paths: Default::default(),
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
            selected_unit_identity: None,
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
        let core = runtime_receipt(&source, "", "empty-rust-dependencies", "empty-stdlib-facets")?;
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
            "empty-stdlib-facets",
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
        let ordinary = runtime_receipt(&source, "incan_stdlib_data|||none", "direct-link", "no-stdlib-facets")?;
        let private_sdk_link = runtime_receipt(&source, "incan_stdlib_data|||link", "direct-link", "no-stdlib-facets")?;

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
    fn a_consumer_provider_compilation_requirement_does_not_fragment_compiler_owned_loaf_compatibility()
    -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let source = project.path().join("main.rs");
        fs::write(&source, "fn main() {}\n")?;
        let shipped = runtime_receipt(
            &source,
            "incan-stdlib|std.interop|ffi",
            "empty-rust-dependencies",
            "interop",
        )?;
        let mut consumer = shipped.clone();
        consumer.sources.build_unit_inputs.insert(
            "provider-compilation-requirements".to_string(),
            "sha256:consumer-selected-macro-set".to_string(),
        );

        // A shipped compiler-owned Loaf is built before any consumer exists, so it can never carry a consumer's
        // selected macro set. Keying compatibility on it would demand one shipped Loaf per consumer.
        let compatibility = OvenLoafCompatibility::from_receipt(&shipped)?;
        assert!(
            compatibility.authorizes_provider_subset(&consumer)?,
            "a consumer's provider compilation requirement must not disqualify the shipped Loaf"
        );

        // The exclusion is narrow: any other differing runtime input still fragments compatibility.
        consumer
            .sources
            .build_unit_inputs
            .insert("unrelated-compiler-input".to_string(), "changed".to_string());
        assert!(!compatibility.authorizes_provider_subset(&consumer)?);
        Ok(())
    }

    #[test]
    fn loaf_selection_prefers_the_narrowest_compatible_provider_loaf() -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let source = project.path().join("main.rs");
        fs::write(&source, "fn main() {}\n")?;
        let core = runtime_receipt(&source, "", "empty-rust-dependencies", "empty-stdlib-facets")?;
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
                registry_leaves: Vec::new(),
            },
            CompatibleLoaf {
                path: PathBuf::from("/toolchain/loafs/encoding/loaf.json"),
                excess: encoding_excess,
                registry_leaves: Vec::new(),
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
            registry_leaves: Vec::new(),
        };
        let broad = CompatibleLoaf {
            path: PathBuf::from("/toolchain/loafs/broad/loaf.json"),
            excess: super::OvenLoafProviderExcess {
                providers: 1,
                modules: 1,
                facets: 1,
                direct_links: 0,
            },
            registry_leaves: Vec::new(),
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
            registry_leaves: Vec::new(),
        };
        let broad = CompatibleLoaf {
            path: PathBuf::from("/toolchain/loafs/broad/loaf.json"),
            excess: super::OvenLoafProviderExcess {
                providers: 1,
                modules: 1,
                facets: 1,
                direct_links: 0,
            },
            registry_leaves: Vec::new(),
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

    #[test]
    fn native_loaf_accounting_measures_the_final_loaf_directory() -> Result<(), Box<dyn std::error::Error>> {
        let loaf = tempfile::tempdir()?;
        fs::write(loaf.path().join("loaf.json"), b"plan")?;
        fs::create_dir(loaf.path().join("artifacts"))?;
        fs::write(loaf.path().join("artifacts/runtime.rlib"), b"runtime")?;

        let (logical_bytes, physical_bytes) = super::loaf_directory_byte_counts(loaf.path())?;

        assert_eq!(logical_bytes, 11);
        assert!(physical_bytes >= logical_bytes);
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn raw_disk_accounting_counts_hard_linked_payload_once() -> Result<(), Box<dyn std::error::Error>> {
        let loaf = tempfile::tempdir()?;
        let original = loaf.path().join("original.rlib");
        let linked = loaf.path().join("linked.rlib");
        fs::write(&original, vec![0_u8; 8192])?;
        fs::hard_link(&original, &linked)?;

        let directory_bytes = super::loaf_file_physical_bytes(&fs::symlink_metadata(loaf.path())?);
        let payload_bytes = super::loaf_file_physical_bytes(&fs::symlink_metadata(&original)?);
        let raw_disk_bytes = super::loaf_raw_disk_bytes(loaf.path())?;

        assert_eq!(raw_disk_bytes, directory_bytes.saturating_add(payload_bytes));
        assert!(raw_disk_bytes < directory_bytes.saturating_add(payload_bytes.saturating_mul(2)));
        Ok(())
    }
}
