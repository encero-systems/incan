//! Direct `rustc` execution for Oven Alpha's explicitly supported consumer envelope.
//!
//! The executor accepts a verified artifact manifest instead of scanning Cargo output or reproducing Cargo planning.
//! An explicit publisher-side `legacy_cargo` step may create the declared inputs, but this consumer path invokes only
//! the selected Rust compiler and refuses hidden Cargo state.

mod artifact;
mod diagnostics;
mod inspection;

// Split along seams this file already had: the artifact/plan shapes, the project-inspection authority payloads,
// and the rustc diagnostic report. Every path stays where callers expect it -- re-exported here rather than
// re-homed -- so this is a move, not an interface change.
pub use artifact::*;
pub use diagnostics::*;
pub(crate) use inspection::*;

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use semver::{Version, VersionReq};
use serde::{Deserialize, Serialize};

use super::native_contract::{
    OVEN_PROJECT_EXTENSION_PAYLOAD_SCHEMA_VERSION, OvenProjectExtensionPayload, OvenProjectRegistrySourceDependency,
};
use super::process::{isolate_process_group, terminate_process_group};
use super::{OVEN_COMPILER_TEST_PROFILE, OvenBuildIntent, OvenReceipt, digest_bytes};
use crate::manifest::{DependencySource, DependencySpec};
use crate::oven::store::{
    OvenArtifactKind, OvenArtifactManifest, OvenArtifactMaterializedFile, OvenArtifactPublishRequest, OvenStore,
    OvenStoreError, OvenStoreExecutionPayload, OvenStoreLease,
};

/// Wire-format version for an Oven-owned direct-rustc artifact manifest.
/// Version 9 separates the complete sealed Rust-inspection source closure from linkable registry leaves. This keeps
/// transitive proc-macro sources in the same immutable plan even though they do not provide an `.rlib` leaf. Older
/// payloads are intentionally ignored during selection and re-materialized from the active toolchain Loaf.
pub const OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION: u32 = 9;
/// Fixed supporting-artifact path for the publisher lock that owns sealed registry sources.
pub const OVEN_RUSTC_REGISTRY_LOCK_RELATIVE_PATH: &str = "registry-sources/Cargo.lock";
/// Crate-name prefix reserved for the runtime family owned by one Incan release Loaf.
pub const OVEN_COMPILER_RUNTIME_CRATE_PREFIX: &str = "incan_";
/// Schema version for caller-owned native-output reuse evidence.
const OVEN_DIRECT_RUSTC_OUTPUT_RECEIPT_SCHEMA_VERSION: u32 = 3;

/// Original foundation provenance, preserving distinct store receipts and committed Loaf member identities.
pub(crate) enum OvenNativeInputOrigin<'facts> {
    /// A stored direct plan retains the exact original content, receipt and reusable build-unit identities.
    Store {
        identity: &'facts str,
        receipt_identity: &'facts str,
        build_unit_identity: &'facts str,
    },
    /// A toolchain member has committed generation/member/build/plan coordinates, not a store receipt.
    ToolchainLoaf {
        generation_identity: &'facts str,
        member: &'facts super::loaf::OvenLoafEnvelopeMember,
    },
}

/// The actual selected owner retained through fact queries and physical attachment.
enum OvenNativeInputOwner<'owner> {
    Store(&'owner OvenStoreExecutionPayload),
    ToolchainLoaf(&'owner super::loaf::OvenMaterializedLoafCandidate<'owner>),
}

/// Existing stored direct-plan carrier, retaining its original execution owner and one decoded native projection.
///
/// Selection keeps the historical inexpensive materialization path. A batch borrows this carrier only after its
/// existing full owner validation, then enumerates and attaches members without another payload decode or full scan.
pub(crate) struct OvenStoredDirectRustcExecutionPlan {
    owner: OvenStoreExecutionPayload,
    artifacts: OvenRustcArtifactManifest,
    artifact_plan: OvenRustcArtifactPlan,
}

impl OvenStoredDirectRustcExecutionPlan {
    /// Retain an already selected payload and decode its direct plan once, preserving ordinary selection semantics.
    pub(crate) fn from_execution_payload(owner: OvenStoreExecutionPayload) -> Result<Self, OvenRustcError> {
        if owner.manifest.kind != OvenArtifactKind::DirectRustcPlan {
            return Err(OvenRustcError::InvalidInput {
                field: "stored direct plan",
                message: "requires an original selected direct-plan payload".to_string(),
            });
        }
        let artifacts: OvenRustcArtifactManifest =
            serde_json::from_slice(&owner.payload).map_err(|error| OvenRustcError::InvalidInput {
                field: "stored direct plan payload",
                message: error.to_string(),
            })?;
        let artifact_plan = artifacts.materialize_trusted_store(&owner.artifact_root, &owner.manifest.intent)?;
        Ok(Self {
            owner,
            artifacts,
            artifact_plan,
        })
    }

    /// Return the original immutable Store entry identity.
    pub(crate) fn identity(&self) -> &str {
        &self.owner.manifest.identity
    }

    /// Borrow the sole decoded manifest while its original owner remains retained.
    pub(crate) fn artifacts(&self) -> &OvenRustcArtifactManifest {
        &self.artifacts
    }

    /// Borrow the original materialized root; this is not a caller-supplied authority path.
    pub(crate) fn artifact_root(&self) -> &Path {
        &self.owner.artifact_root
    }

    /// Borrow the existing inexpensive materialized plan used by ordinary stored-plan consumers.
    pub(crate) fn artifact_plan(&self) -> &OvenRustcArtifactPlan {
        &self.artifact_plan
    }

    /// Validate the original owner once for a batch and borrow its retained decode and native projection.
    pub(crate) fn native_input_view(&self) -> Result<OvenNativeInputView<'_>, OvenRustcError> {
        self.owner.verify_admitted_payload()?;
        Ok(OvenNativeInputView {
            owner: OvenNativeInputOwner::Store(&self.owner),
            artifacts: Cow::Borrowed(&self.artifacts),
            artifact_root: self.owner.artifact_root.clone(),
            plan: Cow::Borrowed(&self.artifact_plan),
        })
    }
}

/// A command-owned physical view of one admitted foundation, borrowing its actual execution owner.
///
/// Store construction validates the original record/files once; Loaf construction borrows an already materialized
/// candidate. Subsequent fact queries do not touch the filesystem or select compatibility. One Incan batch chooses
/// the foundation before any source unit attaches inputs through it. Neither origin substitutes consumer receipts.
pub(crate) struct OvenNativeInputView<'owner> {
    owner: OvenNativeInputOwner<'owner>,
    artifacts: Cow<'owner, OvenRustcArtifactManifest>,
    artifact_root: PathBuf,
    plan: Cow<'owner, OvenRustcArtifactPlan>,
}

impl<'owner> OvenNativeInputView<'owner> {
    /// Borrow an original store payload after its existing record and physical integrity checks succeed.
    pub(crate) fn from_store_payload(owner: &'owner OvenStoreExecutionPayload) -> Result<Self, OvenRustcError> {
        if owner.manifest.kind != OvenArtifactKind::DirectRustcPlan {
            return Err(OvenRustcError::InvalidInput {
                field: "native input owner",
                message: "requires an original admitted direct-plan payload".to_string(),
            });
        }
        owner.verify_admitted_payload()?;
        let artifacts: OvenRustcArtifactManifest =
            serde_json::from_slice(&owner.payload).map_err(|error| OvenRustcError::InvalidInput {
                field: "native input owner payload",
                message: error.to_string(),
            })?;
        let artifact_root = canonical_directory(&owner.artifact_root, "native input owner root")?;
        let plan = artifacts.materialize_trusted_store(&artifact_root, &owner.manifest.intent)?;
        Ok(Self {
            owner: OvenNativeInputOwner::Store(owner),
            artifacts: Cow::Owned(artifacts),
            artifact_root,
            plan: Cow::Owned(plan),
        })
    }

    /// Borrow an already physically validated committed Loaf without repeating its full artifact walk.
    pub(crate) fn from_materialized_loaf(
        owner: &'owner super::loaf::OvenMaterializedLoafCandidate<'owner>,
    ) -> Result<Self, OvenRustcError> {
        let candidate = owner.candidate();
        Ok(Self {
            owner: OvenNativeInputOwner::ToolchainLoaf(owner),
            artifacts: Cow::Borrowed(&candidate.metadata().plan),
            artifact_root: canonical_directory(owner.artifact_root(), "native input Loaf root")?,
            plan: Cow::Borrowed(owner.artifact_plan()),
        })
    }

    /// Return the original typed provenance without manufacturing a store receipt for a toolchain member.
    pub(crate) fn origin(&self) -> OvenNativeInputOrigin<'_> {
        match &self.owner {
            OvenNativeInputOwner::Store(owner) => OvenNativeInputOrigin::Store {
                identity: &owner.manifest.identity,
                receipt_identity: &owner.manifest.receipt_identity,
                build_unit_identity: &owner.manifest.build_unit_identity,
            },
            OvenNativeInputOwner::ToolchainLoaf(owner) => {
                let candidate = owner.candidate();
                OvenNativeInputOrigin::ToolchainLoaf {
                    generation_identity: candidate.generation_identity(),
                    member: candidate.member(),
                }
            }
        }
    }

    /// Return the original admitted content identity, not a new consumer receipt identity.
    pub(crate) fn identity(&self) -> &str {
        match self.origin() {
            OvenNativeInputOrigin::Store { identity, .. } => identity,
            OvenNativeInputOrigin::ToolchainLoaf { member, .. } => &member.loaf_identity,
        }
    }

    /// Return the original store receipt when this origin has one; toolchain members retain different provenance.
    pub(crate) fn receipt_identity(&self) -> Option<&str> {
        match self.origin() {
            OvenNativeInputOrigin::Store { receipt_identity, .. } => Some(receipt_identity),
            OvenNativeInputOrigin::ToolchainLoaf { .. } => None,
        }
    }

    /// Require the selected Store's original full publisher recipe; an exact key cannot supply missing SDK recipe
    /// facts.
    pub(crate) fn original_native_receipt(&self) -> Result<&OvenReceipt, OvenRustcError> {
        let receipt = match &self.owner {
            OvenNativeInputOwner::Store(owner) => owner.original_native_receipt(),
            OvenNativeInputOwner::ToolchainLoaf(_) => None,
        };
        receipt.ok_or_else(|| OvenRustcError::InvalidInput {
            field: "native input receipt witness",
            message: "selected native owner has no original full publisher receipt witness".to_string(),
        })
    }

    /// Return the original reusable build-unit identity without recomputing compatibility.
    pub(crate) fn build_unit_identity(&self) -> &str {
        match self.origin() {
            OvenNativeInputOrigin::Store {
                build_unit_identity, ..
            } => build_unit_identity,
            OvenNativeInputOrigin::ToolchainLoaf { member, .. } => &member.build_unit_identity,
        }
    }

    /// Borrow the already admitted original catalog for command-table enumeration without another decode or scan.
    pub(crate) fn artifacts(&self) -> &OvenRustcArtifactManifest {
        &self.artifacts
    }

    /// Borrow the admitted physical root only for the existing direct-rustc output containment boundary.
    pub(crate) fn artifact_root(&self) -> &Path {
        &self.artifact_root
    }

    /// Return the original target, toolchain, profile and features as facts, without filtering candidates.
    pub(crate) fn intent(&self) -> &OvenBuildIntent {
        &self.artifacts.intent
    }

    /// Require an original declared source role; a caller's newly generated source key grants nothing here.
    fn require_original_role(&self, source_role: &str) -> Result<(), OvenRustcError> {
        if !self.artifacts.entrypoint_externs.contains_key(source_role) {
            return Err(OvenRustcError::InvalidInput {
                field: "native input source role",
                message: format!("`{source_role}` is not an original declared role of this foundation"),
            });
        }
        Ok(())
    }

    /// Expose every recorded registry leaf in publisher order; version and feature choice belongs to Incan.
    ///
    /// A handle retains the original source role for later physical membership checks. Enumeration itself neither
    /// discovers files nor promotes an anonymous supporting artifact to a named dependency.
    pub(crate) fn registry_candidates(
        &self,
        source_role: &str,
    ) -> Result<Vec<OvenNativeInputCandidate<'_>>, OvenRustcError> {
        self.require_original_role(source_role)?;
        Ok(self
            .artifacts
            .registry_leaves
            .iter()
            .map(|leaf| OvenNativeInputCandidate {
                view: self,
                artifact: &leaf.artifact,
                registry_leaf: Some(leaf),
                source_role: source_role.to_string(),
            })
            .collect())
    }

    /// Expose only the explicitly named externs of an original source role, in manifest declaration order.
    pub(crate) fn named_candidates(
        &self,
        source_role: &str,
    ) -> Result<Vec<OvenNativeInputCandidate<'_>>, OvenRustcError> {
        self.require_original_role(source_role)?;
        let projected = self.artifacts.for_source_evidence(source_role)?;
        Ok(self
            .artifacts
            .externs
            .iter()
            .filter(|artifact| projected.externs.iter().any(|allowed| allowed == *artifact))
            .map(|artifact| OvenNativeInputCandidate {
                view: self,
                artifact,
                registry_leaf: None,
                source_role: source_role.to_string(),
            })
            .collect())
    }

    /// Attach response-selected members transactionally, preserving this exact foundation's original source role.
    ///
    /// The host's command table must first bind the Incan response to the global batch and each definition. This
    /// method checks only original owner/role membership, identifiers, artifact bytes and physical collisions; it
    /// never interprets dependency requirements. Existing role projection owns search/native paths and environment.
    /// No missing directory is added, no alternative file is sought and no output or child is created.
    pub(crate) fn attach_for_source(
        &self,
        source_role: &str,
        selected: &[OvenSelectedNativeInput<'_>],
    ) -> Result<OvenSelectedNativeSourcePlan<'_>, OvenRustcError> {
        self.require_original_role(source_role)?;
        let mut plan = trusted_artifact_plan_for_source_evidence(&self.plan, &self.artifacts, source_role)?;
        let mut aliases = BTreeSet::new();
        for input in selected {
            let candidate = &input.candidate;
            if !std::ptr::eq(candidate.view, self) || candidate.source_role != source_role {
                return Err(OvenRustcError::InvalidInput {
                    field: "selected native input",
                    message: "belongs to a different foundation view or original source role".to_string(),
                });
            }
            if !aliases.insert(input.alias.as_str()) {
                return Err(OvenRustcError::InvalidInput {
                    field: "selected native input",
                    message: format!("duplicates selected alias `{}`", input.alias),
                });
            }
            let artifact = candidate.artifact;
            let output = verified_file(
                &self.artifact_root,
                &artifact.relative_path,
                &artifact.digest,
                "selected native input",
            )?;
            let parent = output.parent().ok_or_else(|| OvenRustcError::InvalidInput {
                field: "selected native input",
                message: "artifact has no dependency directory".to_string(),
            })?;
            if !plan.dependency_search_paths.iter().any(|directory| directory == parent) {
                return Err(OvenRustcError::InvalidInput {
                    field: "selected native input",
                    message: format!(
                        "{} is outside the original source role's dependency directories",
                        output.display()
                    ),
                });
            }
            if let Some((_, existing)) = plan.externs.iter().find(|(name, _)| name == &input.alias) {
                if existing != &output {
                    return Err(OvenRustcError::InvalidInput {
                        field: "selected native input",
                        message: format!("conflicts with existing named extern `{}`", input.alias),
                    });
                }
                continue;
            }
            if plan
                .caller_owned_library_digests
                .insert(input.alias.clone(), artifact.digest.clone())
                .is_some()
            {
                return Err(OvenRustcError::InvalidInput {
                    field: "selected native input",
                    message: format!("conflicts with existing digest evidence for `{}`", input.alias),
                });
            }
            plan.externs.push((input.alias.clone(), output));
        }
        Ok(OvenSelectedNativeSourcePlan { plan, _view: self })
    }
}

/// One original manifest member; only its borrowed foundation view can construct this physical handle.
#[derive(Clone)]
pub(crate) struct OvenNativeInputCandidate<'view> {
    view: &'view OvenNativeInputView<'view>,
    artifact: &'view OvenRustcArtifactExtern,
    registry_leaf: Option<&'view OvenRustcRegistryLeaf>,
    source_role: String,
}

impl<'view> OvenNativeInputCandidate<'view> {
    /// Return recorded registry source/package/version/features, if this is a registry candidate.
    pub(crate) fn registry_leaf(&self) -> Option<&OvenRustcRegistryLeaf> {
        self.registry_leaf
    }

    /// Return the original named artifact declaration, without resolving its filesystem location.
    pub(crate) fn artifact(&self) -> &OvenRustcArtifactExtern {
        self.artifact
    }

    /// Carry the already selected alias; matching that alias to an authored slot remains the Incan batch decision.
    pub(crate) fn with_alias(self, alias: String) -> Result<OvenSelectedNativeInput<'view>, OvenRustcError> {
        validate_rust_identifier(&alias)?;
        Ok(OvenSelectedNativeInput { candidate: self, alias })
    }
}

/// A physical member and explicit direct alias selected by the command's bound Incan batch response.
pub(crate) struct OvenSelectedNativeInput<'view> {
    candidate: OvenNativeInputCandidate<'view>,
    alias: String,
}

/// A source-specific plan whose actual selected foundation owner remains borrowed through its complete use.
pub(crate) struct OvenSelectedNativeSourcePlan<'owner> {
    plan: OvenRustcArtifactPlan,
    _view: &'owner OvenNativeInputView<'owner>,
}

impl OvenSelectedNativeSourcePlan<'_> {
    /// Borrow the attached plan while preserving the selected store lease or committed generation lock.
    pub(crate) fn artifact_plan(&self) -> &OvenRustcArtifactPlan {
        &self.plan
    }
}

/// One registry leaf and the immutable Loaf root that seals its relative artifact path.
#[derive(Debug, Clone)]
struct OvenRegistryLeafAuthorityEntry {
    artifact_root: PathBuf,
    leaf: OvenRustcRegistryLeaf,
    /// Already materialized immutable directories allowed to satisfy this leaf's transitive Rust metadata.
    ///
    /// A direct `--extern` names the immediate leaf, but Rustc still has to load the exact dependency artifacts
    /// recorded in that leaf's metadata. These directories come only from the same verified artifact plan that
    /// sealed the catalog; this is not an ambient Cargo search path.
    dependency_search_paths: Vec<PathBuf>,
}

/// Immutable registry-leaf authority supplied by receipt-compatible Loafs.
///
/// A caller receives this only from compiler Loafs that independently authorize the same receipt or the suite
/// scheduler's leased copies of those Loafs. Each entry retains its own root, so a narrow code plan may use a
/// registry leaf sealed by another compatible unit without treating either catalog as a Cargo-home, registry-index,
/// or download fallback.
#[derive(Debug, Clone)]
pub(crate) struct OvenRegistryLeafAuthority {
    entries: Vec<OvenRegistryLeafAuthorityEntry>,
}

impl OvenRegistryLeafAuthority {
    #[cfg(test)]
    #[must_use]
    /// Construct test-only authority without transitive metadata search directories.
    pub(crate) fn new(artifact_root: PathBuf, leaves: Vec<OvenRustcRegistryLeaf>) -> Self {
        Self::new_with_trusted_dependency_search_paths(artifact_root, leaves, Vec::new())
    }

    /// Construct a catalog whose transitive Rust metadata may be located only in already verified plan paths.
    #[must_use]
    pub(crate) fn new_with_trusted_dependency_search_paths(
        artifact_root: PathBuf,
        leaves: Vec<OvenRustcRegistryLeaf>,
        dependency_search_paths: Vec<PathBuf>,
    ) -> Self {
        Self {
            entries: leaves
                .into_iter()
                .map(|leaf| OvenRegistryLeafAuthorityEntry {
                    artifact_root: artifact_root.clone(),
                    leaf,
                    dependency_search_paths: dependency_search_paths.clone(),
                })
                .collect(),
        }
    }

    /// Construct authority for a complete plan materialized across independently leased immutable roots.
    ///
    /// The caller must first validate the unfragmented manifest and materialize every declared artifact through
    /// [`OvenRustcArtifactManifest::materialize_trusted_store_composed`]. A registry leaf's source tree may be
    /// byte-identical to the base while its target rlib belongs to the extension, so fragment-local catalogs are
    /// intentionally insufficient here. Each entry still names only the root containing its verified rlib; all
    /// transitive metadata directories come from the already verified composed plan.
    #[must_use]
    pub(crate) fn from_composed_plan(
        entries: Vec<(PathBuf, OvenRustcRegistryLeaf)>,
        artifact_plan: &OvenRustcArtifactPlan,
    ) -> Option<Self> {
        (!entries.is_empty()).then(|| Self {
            entries: entries
                .into_iter()
                .map(|(artifact_root, leaf)| OvenRegistryLeafAuthorityEntry {
                    artifact_root,
                    leaf,
                    dependency_search_paths: artifact_plan.dependency_search_paths.clone(),
                })
                .collect(),
        })
    }

    #[must_use]
    /// Join registry-leaf catalogs from independently sealed sources into one lookup surface.
    ///
    /// This does not itself decide compatibility: [`select_sealed_registry_leaf`]'s existing candidate resolution
    /// already tolerates a joined catalog naming the same package at different versions (it picks the
    /// requirement-matching highest one), and separately fails closed if a *specific requested* package/version
    /// resolves to more than one distinct compiled artifact. That per-lookup check is what actually protects against
    /// admitting two incompatible compiled instances of a shared package (most dangerously an async runtime such as
    /// `tokio`, where a runtime object built through one compiled instance becomes invisible to code compiled
    /// against another) -- it is precise about only the package actually being resolved, unlike a blanket
    /// pre-validation of every entry a joined catalog happens to carry, most of which are never looked up together.
    /// Joining catalogs here is therefore safe for any sources whose own artifacts are independently receipt-bound;
    /// it does not require the caller to have already reconciled a shared compiled closure.
    pub(crate) fn aggregate(authorities: impl IntoIterator<Item = Self>) -> Self {
        Self {
            entries: authorities
                .into_iter()
                .flat_map(|authority| authority.entries)
                .collect(),
        }
    }

    /// Return the first package name this authority's own registry leaves would silently link as a second,
    /// incompatible compiled instance of a crate `plan` already links explicitly.
    ///
    /// A caller-owned provider's own registry closure (for example a query-engine library's own third-party
    /// dependency graph) is baked through an independent Cargo resolve from the consumer/SDK's own closure. Both can
    /// legitimately depend on "the same" package at the same version -- most dangerously an async runtime such as
    /// `tokio` -- yet resolve to two byte-distinct compiled artifacts, because a compiled crate's identity depends on
    /// its full compilation context, not only its declared version. Neither [`select_sealed_registry_leaf`] (which
    /// only sees one closure's own catalog) nor [`Self::aggregate`] (which only decides what is *discoverable*, not
    /// what is safe to *use*) can catch this: the danger appears only once a provider's own extern for the shared
    /// package and the consumer's own extern for it are compared directly. This does that comparison, checked
    /// against real evidence: linking a provider's own DataFusion/Tokio closure into the same binary as the SDK's
    /// own Tokio-based `block_on` support (RFC 048/114) is exactly what produced a real "no reactor running" panic
    /// at runtime, discovered as two distinct `tokio` symbol-mangled crate instances in the same linked executable.
    /// A build-time refusal here is far cheaper than that panic. This is deliberately conservative: it compares by
    /// digest, so a package the provider and consumer resolved to the exact same compiled bytes (a legitimate,
    /// harmless case) is never rejected -- only a genuine, byte-distinct duplicate is.
    pub(crate) fn first_conflicting_package_with(
        &self,
        plan: &OvenRustcArtifactPlan,
    ) -> Result<Option<String>, OvenRustcError> {
        for entry in &self.entries {
            let Some((_, existing_path)) = plan
                .externs
                .iter()
                .find(|(crate_name, _)| *crate_name == entry.leaf.crate_name)
            else {
                continue;
            };
            let candidate_path = safe_artifact_path(
                &entry.artifact_root,
                &entry.leaf.artifact.relative_path,
                "registry leaf",
            )?;
            if fs::canonicalize(&candidate_path).ok().as_deref() == fs::canonicalize(existing_path).ok().as_deref() {
                continue;
            }
            let existing_bytes = fs::read(existing_path).map_err(|source| OvenRustcError::Io {
                path: existing_path.clone(),
                source,
            })?;
            if digest_bytes(&existing_bytes) != entry.leaf.artifact.digest {
                return Ok(Some(entry.leaf.package.clone()));
            }
        }
        Ok(None)
    }

    /// Return the first package both authorities carry at the same version but as byte-distinct compiled artifacts.
    ///
    /// [`Self::first_conflicting_package_with`] only sees packages a plan links as a *named* `--extern`, but a
    /// shared package can just as easily enter both sides transitively -- the real `tokio` duplication that
    /// motivated these checks was never a named extern of either compile; both copies loaded purely through
    /// `-L dependency=...` metadata search from their respective dependents. Whenever the consumer's dependents and
    /// a provider's dependents both end up in one link (which is always true for a caller-owned provider: the SDK
    /// runtime and the provider library are both linked), a same-version/different-bytes package in the two catalogs
    /// means two compiled instances of one crate in one binary. Two *different versions* of a package are deliberate,
    /// ordinary Cargo semver coexistence and are not flagged; only a same-version byte divergence -- two independent
    /// compiles of identical source -- is the anomaly this reports.
    pub(crate) fn first_diverging_shared_package(&self, other: &Self) -> Option<String> {
        for entry in &self.entries {
            for candidate in &other.entries {
                if entry.leaf.package == candidate.leaf.package
                    && entry.leaf.version == candidate.leaf.version
                    && entry.leaf.artifact.digest != candidate.leaf.artifact.digest
                {
                    return Some(entry.leaf.package.clone());
                }
            }
        }
        None
    }
}

/// One digest-verified registry leaf plus the plan directories Rustc may use solely for its transitive metadata.
#[derive(Debug)]
struct ResolvedSealedRegistryLeaf {
    artifact: PathBuf,
    dependency_search_paths: Vec<PathBuf>,
}

/// Resolve one registry dependency from the selected Loaf's sealed catalog.
#[cfg(test)]
fn resolve_sealed_registry_leaf(
    dependency: &DependencySpec,
    authority: Option<&OvenRegistryLeafAuthority>,
    profile: &str,
) -> Result<PathBuf, OvenRustcError> {
    Ok(resolve_sealed_registry_leaf_with_search_paths(dependency, authority, profile)?.artifact)
}

/// Verify that a registry dependency already represented by a selected direct-Rustc extern is semantically valid.
///
/// A matching Rust crate name is not sufficient authority: the caller's package, version, and features still have to
/// match one digest-verified registry leaf from a receipt-compatible native catalog. This validates that contract
/// without compiling a duplicate caller-owned `--extern` or consulting Cargo state.
pub(crate) fn validate_sealed_registry_leaf(
    dependency: &DependencySpec,
    authority: Option<&OvenRegistryLeafAuthority>,
    profile: &str,
) -> Result<(), OvenRustcError> {
    let _ = select_sealed_registry_leaf(dependency, authority, profile)?;
    Ok(())
}

/// Verify a registry dependency against the exact materialized extern already selected for its alias.
///
/// This is the execution-time boundary for every selected plan shape. The plan has already fixed an alias to one
/// immutable artifact path; validating that path's sealed leaf avoids repeating a highest-compatible-version choice
/// that could disagree when the catalog contains two semver-compatible package versions.
pub(crate) fn validate_selected_sealed_registry_leaf(
    dependency: &DependencySpec,
    selected_artifact: &Path,
    authority: Option<&OvenRegistryLeafAuthority>,
    profile: &str,
) -> Result<(), OvenRustcError> {
    let authority = authority.ok_or_else(|| OvenRustcError::InvalidInput {
        field: "Oven registry Rust dependency",
        message: format!(
            "`{}` has no receipt-bound Loaf registry catalog",
            dependency.package.as_deref().unwrap_or(&dependency.crate_name)
        ),
    })?;
    let selected_artifact = verified_regular_file(selected_artifact, "selected registry artifact")?;
    let selected_artifact = fs::canonicalize(&selected_artifact).map_err(|source| OvenRustcError::Io {
        path: selected_artifact,
        source,
    })?;
    let mut path_matches = Vec::new();
    for entry in &authority.entries {
        let artifact_path = safe_artifact_path(
            &entry.artifact_root,
            &entry.leaf.artifact.relative_path,
            "sealed registry leaf",
        )?;
        if artifact_path == selected_artifact {
            path_matches.push(entry);
        }
    }
    let [selected] = path_matches.as_slice() else {
        return Err(OvenRustcError::InvalidInput {
            field: "Oven registry Rust dependency",
            message: format!(
                "selected extern `{}` has {} exact receipt-bound registry leaf records",
                selected_artifact.display(),
                path_matches.len()
            ),
        });
    };
    let package = dependency.package.as_deref().unwrap_or(&dependency.crate_name);
    let requirement_text = dependency
        .version
        .as_deref()
        .ok_or_else(|| OvenRustcError::InvalidInput {
            field: "Oven registry Rust dependency",
            message: format!("`{package}` has no declared version requirement"),
        })?;
    let requirement = VersionReq::parse(requirement_text).map_err(|error| OvenRustcError::InvalidInput {
        field: "Oven registry Rust dependency",
        message: format!("`{package}` has invalid version requirement `{requirement_text}`: {error}"),
    })?;
    let available_features = selected.leaf.features.iter().collect::<BTreeSet<_>>();
    let requested_features = dependency.features.iter().collect::<BTreeSet<_>>();
    if selected.leaf.package != package
        || !Version::parse(&selected.leaf.version).is_ok_and(|version| requirement.matches(&version))
        || !requested_features.is_subset(&available_features)
    {
        return Err(OvenRustcError::InvalidInput {
            field: "Oven registry Rust dependency",
            message: format!(
                "selected extern `{}` does not satisfy `{package}` requirement `{requirement_text}` and its requested features",
                selected_artifact.display()
            ),
        });
    }
    let relative_path = Path::new(&selected.leaf.artifact.relative_path);
    let selected_profile = relative_path
        .parent()
        .filter(|parent| parent.file_name().is_some_and(|name| name == "deps"))
        .and_then(Path::parent)
        .and_then(Path::file_name)
        .and_then(|name| name.to_str());
    if let Some(selected_profile) = selected_profile
        && selected_profile != profile
    {
        return Err(OvenRustcError::InvalidInput {
            field: "Oven registry Rust dependency",
            message: format!(
                "selected extern `{}` was baked for profile `{}`, not `{profile}`",
                selected_artifact.display(),
                selected_profile
            ),
        });
    }
    Ok(())
}

/// Select one semantically compatible sealed registry leaf without re-reading its artifact bytes.
///
/// The caller either uses this to validate a selected direct-Rustc extern that the immutable plan has already checked,
/// or [`resolve_sealed_registry_leaf_with_search_paths`] below to verify and attach a new caller-owned leaf.
fn select_sealed_registry_leaf<'a>(
    dependency: &DependencySpec,
    authority: Option<&'a OvenRegistryLeafAuthority>,
    profile: &str,
) -> Result<&'a OvenRegistryLeafAuthorityEntry, OvenRustcError> {
    let authority = authority.ok_or_else(|| OvenRustcError::InvalidInput {
        field: "Oven registry Rust dependency",
        message: format!(
            "`{}` has no receipt-bound Loaf registry catalog; prepare an explicit Oven-native closure",
            dependency.package.as_deref().unwrap_or(&dependency.crate_name)
        ),
    })?;
    let package_name = dependency.package.as_deref().unwrap_or(&dependency.crate_name);
    let requirement_text = dependency
        .version
        .as_deref()
        .ok_or_else(|| OvenRustcError::InvalidInput {
            field: "Oven registry Rust dependency",
            message: format!("`{package_name}` has no declared version requirement"),
        })?;
    let requirement = VersionReq::parse(requirement_text).map_err(|error| OvenRustcError::InvalidInput {
        field: "Oven registry Rust dependency",
        message: format!("`{package_name}` has invalid version requirement `{requirement_text}`: {error}"),
    })?;
    let requested_features = dependency.features.iter().collect::<BTreeSet<_>>();
    let mut candidates = authority
        .entries
        .iter()
        .filter_map(|entry| {
            let leaf = &entry.leaf;
            if leaf.package != package_name {
                return None;
            }
            let version = Version::parse(&leaf.version).ok()?;
            let available_features = leaf.features.iter().collect::<BTreeSet<_>>();
            (requirement.matches(&version) && requested_features.is_subset(&available_features))
                .then_some((version, entry))
        })
        .collect::<Vec<_>>();
    candidates.sort_by(|(left_version, left), (right_version, right)| {
        right_version
            .cmp(left_version)
            .then_with(|| left.leaf.artifact.relative_path.cmp(&right.leaf.artifact.relative_path))
            .then_with(|| left.artifact_root.cmp(&right.artifact_root))
    });
    // A suite ships separate debug and release Loaf catalogs. Prefer the matching profile whenever its sealed
    // catalog contains this dependency; fixtures use short synthetic paths, so retain the complete catalog when no
    // profile-qualified artifact exists.
    let profile_marker = format!("/{profile}/deps/");
    if candidates
        .iter()
        .any(|(_, entry)| entry.leaf.artifact.relative_path.contains(&profile_marker))
    {
        candidates.retain(|(_, entry)| entry.leaf.artifact.relative_path.contains(&profile_marker));
    }
    let Some((selected_version, selected)) = candidates.first().map(|(version, entry)| (version.clone(), *entry))
    else {
        return Err(OvenRustcError::InvalidInput {
            field: "Oven registry Rust dependency",
            message: format!(
                "`{package_name}` requirement `{requirement_text}` has no compatible receipt-bound Loaf registry leaf; prepare an explicit Oven-native closure"
            ),
        });
    };
    let selected_artifact_name = Path::new(&selected.leaf.artifact.relative_path)
        .file_name()
        .and_then(|name| name.to_str());
    let same_compilation = candidates
        .iter()
        .filter(|(version, _)| version == &selected_version)
        .all(|(_, entry)| {
            entry.leaf.crate_name == selected.leaf.crate_name
                && Path::new(&entry.leaf.artifact.relative_path)
                    .file_name()
                    .and_then(|name| name.to_str())
                    == selected_artifact_name
        });
    if !same_compilation {
        return Err(OvenRustcError::InvalidInput {
            field: "Oven registry Rust dependency",
            message: format!(
                "`{package_name}` version `{selected_version}` resolves to multiple receipt-bound Loaf registry leaves; prepare an explicit Oven-native closure"
            ),
        });
    }
    Ok(selected)
}

/// Resolve a registry leaf and the pre-verified search directory required to load its transitive metadata.
fn resolve_sealed_registry_leaf_with_search_paths(
    dependency: &DependencySpec,
    authority: Option<&OvenRegistryLeafAuthority>,
    profile: &str,
) -> Result<ResolvedSealedRegistryLeaf, OvenRustcError> {
    let selected = select_sealed_registry_leaf(dependency, authority, profile)?;
    let package_name = dependency.package.as_deref().unwrap_or(&dependency.crate_name);
    // A Cargo publisher can retain byte-distinct copies of one logical crate across independently sealed native
    // units. Rustc's metadata-bearing artifact name is the compilation identity available at this boundary. Once
    // the requested profile, crate name, version, features, and artifact name agree, choose the canonical sorted
    // copy rather than turning equivalent receipt-bound copies into a false ambiguity. A different artifact name
    // remains fail-closed above.
    let artifact = &selected.leaf.artifact;
    let artifact_path = safe_artifact_path(&selected.artifact_root, &artifact.relative_path, "registry leaf")?;
    let bytes = fs::read(&artifact_path).map_err(|source| OvenRustcError::Io {
        path: artifact_path.clone(),
        source,
    })?;
    if digest_bytes(&bytes) != artifact.digest {
        return Err(OvenRustcError::InvalidInput {
            field: "Oven registry Rust dependency",
            message: format!(
                "sealed registry leaf `{package_name}` at {} failed digest verification",
                artifact_path.display()
            ),
        });
    }
    let extension = artifact_path.extension().and_then(|extension| extension.to_str());
    if extension != Some("rlib") {
        return Err(OvenRustcError::InvalidInput {
            field: "Oven registry Rust dependency",
            message: format!(
                "sealed registry leaf `{package_name}` at {} is not an rlib",
                artifact_path.display()
            ),
        });
    }
    let artifact_parent = artifact_path.parent().ok_or_else(|| OvenRustcError::InvalidInput {
        field: "Oven registry Rust dependency",
        message: format!(
            "sealed registry leaf `{package_name}` at {} has no dependency directory",
            artifact_path.display()
        ),
    })?;
    let mut dependency_search_paths = selected
        .dependency_search_paths
        .iter()
        .map(|path| canonical_directory(path, "registry leaf dependency search path"))
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .filter(|path| path == artifact_parent)
        .collect::<Vec<_>>();
    dependency_search_paths.sort();
    dependency_search_paths.dedup();
    if !selected.dependency_search_paths.is_empty() && dependency_search_paths.is_empty() {
        return Err(OvenRustcError::InvalidInput {
            field: "Oven registry Rust dependency",
            message: format!(
                "sealed registry leaf `{package_name}` at {} is outside its verified dependency search paths",
                artifact_path.display()
            ),
        });
    }
    Ok(ResolvedSealedRegistryLeaf {
        artifact: artifact_path,
        dependency_search_paths,
    })
}

/// Resolve one manifest-recorded Loaf artifact without allowing the leaf catalog to escape its sealed root.
fn safe_artifact_path(
    artifact_root: &Path,
    relative_path: &str,
    kind: &'static str,
) -> Result<PathBuf, OvenRustcError> {
    let normalized = normalized_relative_path(relative_path, kind)?;
    let root = canonical_directory(artifact_root, "registry leaf artifact root")?;
    let artifact = verified_regular_file(&root.join(normalized), kind)?;
    if !artifact.starts_with(&root) {
        return Err(OvenRustcError::InvalidInput {
            field: "Oven registry Rust dependency",
            message: format!("{kind} artifact {} escapes its sealed root", artifact.display()),
        });
    }
    Ok(artifact)
}

/// Normalize a declared package name to the direct-rustc crate identifier it exposes.
fn direct_rustc_crate_name(name: &str) -> Result<String, OvenRustcError> {
    let normalized = name.replace('-', "_");
    validate_rust_identifier(&normalized)?;
    Ok(normalized)
}

/// Add direct-Rustc workspace-library outputs to a previously selected immutable artifact plan.
///
/// The immutable plan continues to own third-party and native inputs. These libraries are the caller-owned bridge
/// between topologically ordered workspace compilation steps, so their already-verified digests are incorporated
/// into the consumer output's reuse identity instead of being mistaken for a Cargo target directory.
pub(crate) fn attach_caller_owned_rustc_libraries(
    plan: &mut OvenRustcArtifactPlan,
    libraries: &[OvenCallerOwnedRustcLibrary],
) -> Result<(), OvenRustcError> {
    let mut crate_names = plan
        .externs
        .iter()
        .map(|(name, _)| name.clone())
        .collect::<BTreeSet<_>>();
    for library in libraries {
        validate_rust_identifier(&library.crate_name)?;
        if library.expose_extern && !crate_names.insert(library.crate_name.clone()) {
            return Err(OvenRustcError::InvalidInput {
                field: "caller-owned library",
                message: format!("duplicates direct-Rustc extern `{}`", library.crate_name),
            });
        }
        let output = verified_regular_file(&library.output, "caller-owned library")?;
        let extension = output.extension().and_then(|extension| extension.to_str());
        if !matches!(extension, Some("rlib" | "dylib" | "so" | "dll")) {
            return Err(OvenRustcError::InvalidInput {
                field: "caller-owned library",
                message: format!("{} must be a Rust library or procedural-macro output", output.display()),
            });
        }
        let parent = output.parent().ok_or_else(|| OvenRustcError::InvalidInput {
            field: "caller-owned library",
            message: format!("{} has no parent directory", output.display()),
        })?;
        if !plan.dependency_search_paths.contains(&parent.to_path_buf()) {
            plan.dependency_search_paths.push(parent.to_path_buf());
        }
        let evidence_key = if library.expose_extern {
            library.crate_name.clone()
        } else {
            format!("transitive:{}:{}", library.crate_name, library.digest)
        };
        if plan
            .caller_owned_library_digests
            .insert(evidence_key, library.digest.clone())
            .is_some()
        {
            return Err(OvenRustcError::InvalidInput {
                field: "caller-owned library",
                message: format!("duplicates reuse evidence for `{}`", library.crate_name),
            });
        }
        if library.expose_extern {
            plan.externs.push((library.crate_name.clone(), output));
        }
    }
    plan.dependency_search_paths.sort();
    plan.dependency_search_paths.dedup();
    Ok(())
}

/// One actively leased immutable root contributing a named fragment to a composed compiler-suite closure.
///
/// Every fragment is verified at publication and held by the scheduler before compilation starts. Direct Rustc
/// receives its paths directly from these roots, avoiding a copied aggregate directory that would itself evade
/// compatibility-domain policy.
pub(crate) struct OvenTrustedRustcArtifactRoot<'a> {
    /// Store-owned root of this selected foundation artifact.
    pub artifact_root: &'a Path,
    /// Direct-rustc closure fragment retained in this root.
    pub dependency_search_paths: &'a [String],
    /// Native-link search paths retained in this root.
    pub native_search_paths: &'a [String],
    /// Every artifact materialized in this root.
    pub supporting_artifacts: &'a [OvenRustcSupportingArtifact],
}

/// Publisher-owned artifact input that has passed manifest validation and may be copied into Oven storage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OvenRustcMaterializedArtifact {
    /// Portable manifest path preserved beneath the store-owned artifact root.
    pub relative_path: String,
    /// Canonical publisher path whose bytes match the manifest digest at validation time.
    pub source_path: PathBuf,
}

/// Request to compile one receipt-bound test source with direct `rustc`.
#[derive(Debug, Clone)]
pub struct OvenDirectRustcTestRequest {
    /// Verified project/build-unit receipt that authorizes source and intent.
    pub receipt: OvenReceipt,
    /// Immutable artifact manifest selected for the receipt intent.
    pub artifacts: OvenRustcArtifactManifest,
    /// Immutable artifact root containing every path named by `artifacts`.
    pub artifact_root: PathBuf,
    /// Explicit Rust compiler executable; PATH lookup is not used as a hidden selector.
    pub rustc: PathBuf,
    /// Caller-owned generated Rust source file.
    pub source: PathBuf,
    /// Caller-owned final test executable output path.
    pub output: PathBuf,
    /// Rust crate name for the test harness.
    pub crate_name: String,
    /// Rust edition supplied to the compiler.
    pub edition: String,
    /// Receipt supplemental-digest key that authorizes the generated source content.
    pub source_evidence_key: String,
}

/// Request to compile one receipt-bound binary source with direct `rustc`.
#[derive(Debug, Clone)]
pub struct OvenDirectRustcRunRequest {
    /// Verified project/build-unit receipt that authorizes source and intent.
    pub receipt: OvenReceipt,
    /// Immutable artifact manifest selected for the receipt intent.
    pub artifacts: OvenRustcArtifactManifest,
    /// Immutable artifact root containing every path named by `artifacts`.
    pub artifact_root: PathBuf,
    /// Explicit Rust compiler executable; PATH lookup is not used as a hidden selector.
    pub rustc: PathBuf,
    /// Caller-owned generated Rust source file.
    pub source: PathBuf,
    /// Caller-owned final binary output path.
    pub output: PathBuf,
    /// Rust crate name for the binary.
    pub crate_name: String,
    /// Rust edition supplied to the compiler.
    pub edition: String,
    /// Receipt supplemental-digest key that authorizes the generated source content.
    pub source_evidence_key: String,
}

/// Request to compile a test through an immutable direct-rustc closure selected from the bounded Oven store.
pub struct OvenStoredDirectRustcTestRequest<'a> {
    /// Bounded Oven store that retains the selected immutable plan.
    pub store: &'a OvenStore,
    /// Exact store identity of the `direct_rustc_plan` artifact to execute.
    pub plan_identity: String,
    /// Verified project/build-unit receipt that authorizes source and intent.
    pub receipt: OvenReceipt,
    /// Explicit Rust compiler executable; PATH lookup is not used as a hidden selector.
    pub rustc: PathBuf,
    /// Caller-owned generated Rust source file.
    pub source: PathBuf,
    /// Caller-owned final test executable output path.
    pub output: PathBuf,
    /// Rust crate name for the test harness.
    pub crate_name: String,
    /// Rust edition supplied to the compiler.
    pub edition: String,
    /// Receipt supplemental-digest key that authorizes the generated source content.
    pub source_evidence_key: String,
}

/// Request to compile a binary through an immutable direct-rustc closure selected from the bounded Oven store.
pub struct OvenStoredDirectRustcRunRequest<'a> {
    /// Bounded Oven store that retains the selected immutable plan.
    pub store: &'a OvenStore,
    /// Exact store identity of the `direct_rustc_plan` artifact to execute.
    pub plan_identity: String,
    /// Verified project/build-unit receipt that authorizes source and intent.
    pub receipt: OvenReceipt,
    /// Explicit Rust compiler executable; PATH lookup is not used as a hidden selector.
    pub rustc: PathBuf,
    /// Caller-owned generated Rust source file.
    pub source: PathBuf,
    /// Caller-owned final binary output path.
    pub output: PathBuf,
    /// Rust crate name for the binary.
    pub crate_name: String,
    /// Supported Rust edition.
    pub edition: String,
    /// Receipt supplemental-digest key that authorizes the generated source content.
    pub source_evidence_key: String,
}

/// Request to compile a caller-owned Rust library through an immutable direct-rustc closure selected from the
/// bounded Oven store.
///
/// This is the normal-command counterpart to [`OvenStoredDirectRustcRunRequest`]. The generated library source and
/// final `.rlib` remain caller-owned, while the third-party/runtime closure is selected only through the receipt and
/// held under a store lease for the compilation.
pub struct OvenStoredDirectRustcLibraryRequest<'a> {
    /// Bounded Oven store that retains the selected immutable plan.
    pub store: &'a OvenStore,
    /// Exact store identity of the `direct_rustc_plan` artifact to execute.
    pub plan_identity: String,
    /// Verified project/build-unit receipt that authorizes source and intent.
    pub receipt: OvenReceipt,
    /// Explicit Rust compiler executable; PATH lookup is not used as a hidden selector.
    pub rustc: PathBuf,
    /// Caller-owned generated Rust source file.
    pub source: PathBuf,
    /// Caller-owned final Rust library output path.
    pub output: PathBuf,
    /// Rust crate name for the library.
    pub crate_name: String,
    /// Supported Rust edition.
    pub edition: String,
    /// Receipt supplemental-digest key that authorizes the generated source content.
    pub source_evidence_key: String,
}

/// Request to compile one target from an already selected, actively leased Oven suite artifact.
///
/// Compiler-suite execution holds the suite lease for the complete inventory and run, but its payload is not a
/// standalone `direct_rustc_plan` store entry. This narrow internal request reuses the trusted-store materialization
/// path without granting callers the ability to select an unleased artifact root.
pub(crate) struct OvenTrustedDirectRustcTargetRequest<'a> {
    /// Exact receipt that authorizes the workspace target source.
    pub receipt: &'a OvenReceipt,
    /// Target-specific direct-rustc closure declared by the immutable suite payload.
    pub artifacts: &'a OvenRustcArtifactManifest,
    /// Materialized root returned alongside the active suite lease.
    pub artifact_root: &'a Path,
    /// Optional plan composed from several separately leased compiler foundations.
    ///
    /// When absent, this request materializes the one legacy schema-8/9 artifact root. Schema 10 supplies an
    /// already validated composed plan and never asks this runner to rebuild a directory tree.
    pub artifact_plan: Option<&'a OvenRustcArtifactPlan>,
    /// Explicit compiler selected by the receipt.
    pub rustc: &'a Path,
    /// Caller-owned workspace test root.
    pub source: &'a Path,
    /// Caller-owned transient native test executable.
    pub output: &'a Path,
    /// Rust crate identifier for this test root.
    pub crate_name: &'a str,
    /// Rust edition declared by the target inventory.
    pub edition: &'a str,
    /// Receipt source-evidence key for this exact target root.
    pub source_evidence_key: &'a str,
    /// Resolved target feature set from the publisher's unit graph.
    pub features: &'a [String],
    /// Whether Cargo's publisher compiled this libtest root with `-C prefer-dynamic`.
    pub prefer_dynamic: bool,
}

/// Request to run one receipt-bound Rustdoc doctest root from an actively leased compiler-suite artifact.
///
/// Rustdoc owns the ephemeral doctest binaries, so Oven records the caller-owned temporary directory rather than
/// pretending there is a stable native executable to store or re-run.
pub(crate) struct OvenTrustedRustdocTestRequest<'a> {
    /// Exact receipt that authorizes the workspace target source.
    pub receipt: &'a OvenReceipt,
    /// Target-specific artifact closure declared by the immutable suite payload.
    pub artifacts: &'a OvenRustcArtifactManifest,
    /// Materialized root returned alongside the active suite lease.
    pub artifact_root: &'a Path,
    /// Optional plan composed from several separately leased compiler foundations.
    pub artifact_plan: Option<&'a OvenRustcArtifactPlan>,
    /// Explicit compiler selected by the receipt; its sibling Rustdoc is derived from the same sysroot.
    pub rustc: &'a Path,
    /// Caller-owned workspace Rustdoc source root.
    pub source: &'a Path,
    /// Caller-owned temporary directory for Rustdoc's ephemeral doctest binaries.
    pub temporary_directory: &'a Path,
    /// Rust crate identifier for this doctest root.
    pub crate_name: &'a str,
    /// Rust edition declared by the target inventory.
    pub edition: &'a str,
    /// Receipt source-evidence key for this exact source root.
    pub source_evidence_key: &'a str,
    /// Resolved target feature set from the publisher's unit graph.
    pub features: &'a [String],
    /// Whether Rustdoc must compile this root as a procedural-macro crate.
    pub is_proc_macro: bool,
    /// Whether the doctest root requires the selected toolchain dynamic library environment.
    pub prefer_dynamic: bool,
    /// Maximum wall-clock duration for the Rustdoc root and its generated doctest descendants.
    pub timeout: Option<Duration>,
}

/// Successful direct Rustdoc doctest execution transcript.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OvenRustdocTestReport {
    /// Combined Rustdoc/doctest output for caller failure reporting.
    pub output: String,
}

/// Successful direct-rustc consumer compilation evidence.
pub struct OvenDirectRustcBake {
    /// Source digest that was matched to receipt evidence before compiler invocation.
    pub source_digest: String,
    /// Final caller-owned test executable path.
    pub output: PathBuf,
    /// Digest of the regular caller-owned output, established before it is exposed to another direct-Rustc step.
    pub output_digest: String,
    /// Whether the receipt- and plan-verified caller-owned native output was reused without launching rustc.
    pub reused: bool,
    /// Held for stored consumers until their caller finishes executing the native test binary.
    lease: Option<OvenStoreLease>,
}

const OVEN_DIRECT_RUSTC_COMPILER_OWNER_SCHEMA_VERSION: u32 = 1;
const OVEN_DIRECT_RUSTC_COMPILER_DOMAIN_PREFIX: &str = "native-compiler";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct OvenDirectRustcCompilerOwnerPayload {
    schema_version: u32,
    binary_digest: String,
    closure_digest: String,
    host: String,
    target: String,
    toolchain: String,
}

/// Toolchain-owned bytes that can affect a direct-Rustc compilation independently of package provenance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OvenDirectRustcCompilerEvidence {
    binary_digest: String,
    closure_digest: String,
    host: String,
    target: String,
    sysroot: PathBuf,
    members: Vec<OvenDirectRustcCompilerMember>,
}

/// One physical compiler member paired with its location-independent sysroot coordinate and byte identity.
#[derive(Debug, Clone, PartialEq, Eq)]
struct OvenDirectRustcCompilerMember {
    relative_path: String,
    source_path: PathBuf,
    digest: String,
}

/// Store-owned compiler closure retained under one lease for an entire native provider batch.
pub(crate) struct OvenOwnedDirectRustcCompiler {
    _owner: OvenStoreExecutionPayload,
    evidence: OvenDirectRustcCompilerEvidence,
    rustc: PathBuf,
}

/// Outcome of trying to retain the selected compiler closure for JEC.
///
/// Unavailability is deliberately distinct from a hard receipt or intent mismatch: callers may keep compiling
/// deterministically through their admitted compiler, but must surface why byte-identical reuse was disabled.
pub(crate) enum OvenDirectRustcCompilerRetention {
    Retained(OvenOwnedDirectRustcCompiler),
    Unavailable { reason: String },
}

impl OvenDirectRustcCompilerEvidence {
    pub(crate) fn binary_digest(&self) -> &str {
        &self.binary_digest
    }

    pub(crate) fn closure_digest(&self) -> &str {
        &self.closure_digest
    }

    pub(crate) fn host(&self) -> &str {
        &self.host
    }

    pub(crate) fn target(&self) -> &str {
        &self.target
    }
}

impl OvenOwnedDirectRustcCompiler {
    pub(crate) fn rustc(&self) -> &Path {
        &self.rustc
    }

    pub(crate) fn evidence(&self) -> &OvenDirectRustcCompilerEvidence {
        &self.evidence
    }

    fn toolchain(&self) -> &str {
        &self._owner.manifest.intent.toolchain
    }
}

/// Logical names paired with the exact physical members of one prepared invocation.
///
/// The admitted native/source-unit plan decides membership. This binding gives the native-compilation policy stable
/// names for those already selected paths; it cannot add an extern, search directory or source file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OvenDirectRustcJecBindings {
    pub(crate) logical_source_root: String,
    pub(crate) logical_entrypoint: String,
    /// Logical native-member slot for every prepared `--extern`, in exact argument order.
    pub(crate) externs: Vec<String>,
    /// Logical search slot for every prepared `-L dependency`, in exact argument order.
    pub(crate) dependency_searches: Vec<String>,
    /// Logical search slot for every prepared `-L native`, in exact argument order.
    pub(crate) native_searches: Vec<String>,
}

/// A fully observed direct-Rustc library invocation before output lookup or compiler execution.
///
/// This owns the source-specific physical plan so the Incan projection and Rustc consume one preparation. Package
/// name/version and Store coordinates remain outside this value; callers retain them only in request bindings.
pub(crate) struct OvenPreparedDirectRustcLibrary {
    rustc: PathBuf,
    compiler: Option<OvenDirectRustcCompilerEvidence>,
    /// Lease-protected Store owner for `compiler`; production JEC preparations retain it instead of rereading its
    /// immutable bytes before every provider unit.
    compiler_owner: Option<Arc<OvenOwnedDirectRustcCompiler>>,
    source: PathBuf,
    source_root: PathBuf,
    source_digest: String,
    artifact_root: PathBuf,
    selected_artifacts: OvenRustcArtifactManifest,
    plan: OvenRustcArtifactPlan,
    target: String,
    profile: String,
    crate_name: String,
    edition: String,
    features: Vec<String>,
    /// Complete deterministic environment derived from the admitted native-plan values.
    ///
    /// Values remain process-local. Callers expose only their digests to the Incan projection.
    frozen_environment: Option<BTreeMap<String, PathBuf>>,
}

/// One environment name observed by Rustc, with a digest only when it was present in the frozen environment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OvenRustcObservedEnvironment {
    pub(crate) name: String,
    pub(crate) value_digest: Option<String>,
}

/// Parsed Rustc dependency observations. Raw dep-info bytes and environment values never cross this boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OvenRustcDepInfoObservation {
    pub(crate) files: Vec<PathBuf>,
    pub(crate) environment: Vec<OvenRustcObservedEnvironment>,
}

/// Sanitized observation result. An unsupported dep-info shape disables reuse without failing a successful compile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum OvenRustcDepInfoOutcome {
    Observed(OvenRustcDepInfoObservation),
    Unavailable { reason: String },
}

/// Fresh direct-Rustc output plus the sanitized observations needed for an Incan-owned compilation key.
pub(crate) struct OvenDirectRustcJecCompilation {
    pub(crate) bake: OvenDirectRustcBake,
    pub(crate) observation: OvenRustcDepInfoOutcome,
}

/// One immutable physical invocation, including its exact output path, argv and cwd.
///
/// Cacheable execution uses the prepared frozen environment. An explicitly uncacheable fallback retains the legacy
/// direct-Rustc inheritance contract.
pub(crate) struct OvenBoundDirectRustcLibrary<'prepared> {
    prepared: &'prepared OvenPreparedDirectRustcLibrary,
    output: PathBuf,
    arguments: Vec<OsString>,
    logical_arguments: Vec<Vec<String>>,
    path_effects_digest: Option<String>,
}

/// The concrete caller-owned output Rustc must produce for one Oven materialization step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))]
enum OvenDirectRustcOutputKind {
    Binary,
    Library,
    Dylib,
    ProcMacro,
}

impl OvenDirectRustcOutputKind {
    /// Return the stable receipt spelling for this caller-owned output kind.
    const fn receipt_value(self) -> &'static str {
        match self {
            Self::Binary => "binary",
            Self::Library => "library",
            Self::Dylib => "dylib",
            Self::ProcMacro => "proc-macro",
        }
    }
}

/// Return the receipt default for callers that predate an explicit output-kind field.
fn default_direct_rustc_output_kind() -> String {
    OvenDirectRustcOutputKind::Binary.receipt_value().to_string()
}

/// Complete input binding retained beside a caller-owned native output.
///
/// This is not Oven store state and never authorizes execution by itself: the caller still verifies the receipt,
/// compiler identity, source digest, and immutable plan before matching these inputs and the persisted output digest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct OvenDirectRustcOutputReceipt {
    schema_version: u32,
    receipt_identity: String,
    artifact_manifest_digest: String,
    source_digest: String,
    crate_name: String,
    edition: String,
    #[serde(default)]
    features: Vec<String>,
    test_harness: bool,
    prefer_dynamic: bool,
    #[serde(default = "default_direct_rustc_output_kind")]
    output_kind: String,
    #[serde(default)]
    caller_owned_library_digests: BTreeMap<String, String>,
}

/// Successful compilation evidence binding the complete verified inputs to the exact produced output bytes.
///
/// Flattening preserves the sidecar's input field names. The schema bump and required digest ensure readers cannot
/// reuse older evidence that never bound the output; a malformed or interrupted record is a cache miss.
#[derive(Debug, Serialize, Deserialize)]
struct OvenDirectRustcOutputRecord {
    #[serde(flatten)]
    inputs: OvenDirectRustcOutputReceipt,
    output_digest: String,
}

/// Backwards-compatible name for a direct-rustc libtest bake.
pub type OvenDirectRustcTestBake = OvenDirectRustcBake;

/// Errors while verifying direct-rustc inputs or compiling an Oven consumer.
#[derive(Debug, thiserror::Error)]
pub enum OvenRustcError {
    /// Artifact manifest schema differs from this executable's supported wire format.
    #[error("unsupported Oven Rust artifact manifest schema version {found}; expected {expected}")]
    UnsupportedSchema { found: u32, expected: u32 },
    /// Artifact plan intent differs from the selected Oven receipt.
    #[error("Oven direct-rustc artifact intent differs from the selected receipt")]
    IntentMismatch,
    /// A project inspection authority uses a wire schema this executable cannot interpret.
    #[error("unsupported Oven project inspection authority schema version {found}; expected {expected}")]
    UnsupportedProjectInspectionAuthoritySchema { found: u32, expected: u32 },
    /// A request field is blank or does not obey the narrow Alpha spelling contract.
    #[error("invalid Oven direct-rustc {field}: {message}")]
    InvalidInput { field: &'static str, message: String },
    /// A declared artifact path escapes the immutable artifact root or is not a regular file/directory.
    #[error("invalid Oven direct-rustc {kind} path {path}: {message}")]
    InvalidArtifactPath {
        kind: &'static str,
        path: PathBuf,
        message: String,
    },
    /// A declared artifact did not retain the digest recorded in the immutable artifact plan.
    #[error("Oven direct-rustc artifact digest mismatch at {path}: expected {expected}, got {actual}")]
    ArtifactDigestMismatch {
        path: PathBuf,
        expected: String,
        actual: String,
    },
    /// A declared search directory contains an unrecorded or unsupported entry.
    #[error("Oven direct-rustc search directory has unrecorded input {path}")]
    UnrecordedSearchArtifact { path: PathBuf },
    /// Source content differs from the receipt-bound supplemental source evidence.
    #[error("Oven direct-rustc source evidence mismatch for `{key}`: expected {expected}, got {actual}")]
    SourceEvidenceMismatch {
        key: String,
        expected: String,
        actual: String,
    },
    /// The selected compiler did not report the exact identity the prebuilt libraries were sealed against.
    ///
    /// Users reach this by building with a different Rust compiler than the one that produced the libraries their
    /// installation ships, so the message names both compilers and the way out rather than the internal reason.
    #[error(
        "Rust compiler mismatch: these prebuilt libraries were built with `{expected}`, but the Rust compiler in \
         use is `{actual}`. Compiled Rust libraries only load under the exact compiler that built them. Reinstall \
         Incan so it provisions its own matching Rust toolchain, or set RUSTC to a `{expected}` compiler."
    )]
    ToolchainMismatch { expected: String, actual: String },
    /// Reading or writing a direct-rustc input/output path failed.
    #[error("Oven direct-rustc I/O failed at {path}: {source}")]
    Io { path: PathBuf, source: io::Error },
    /// Rustc returned a non-success status with structured diagnostic evidence.
    #[error("Oven direct-rustc compilation failed:\n{report}")]
    CompilationFailed { report: OvenRustcDiagnosticReport },
    /// Receipt-bound Rustdoc returned a non-success status while executing doctests.
    #[error("Oven direct Rustdoc doctest failed:\n{output}")]
    RustdocTestFailed { output: String },
    /// A bounded Oven store failed to select the immutable direct-rustc plan.
    #[error("Oven direct-rustc plan store failure: {0}")]
    Store(#[from] OvenStoreError),
    /// A selected store entry is not the receipt-bound direct-rustc plan expected by this executor.
    #[error("invalid Oven direct-rustc stored plan `{identity}`: {message}")]
    InvalidStoredPlan { identity: String, message: String },
    /// The bounded store does not retain a unique direct-rustc plan for the requested receipt.
    #[error("Oven direct-rustc selection failed for receipt `{receipt_identity}`: {message}")]
    PlanSelection { receipt_identity: String, message: String },
}

/// Return the compiler-owned crate name encoded by one Cargo artifact filename.
fn compiler_runtime_name_from_artifact_path(relative_path: &str) -> Option<&str> {
    let filename = Path::new(relative_path).file_name()?.to_str()?;
    let crate_and_digest = filename.strip_prefix("lib")?.split_once('-')?.0;
    crate_and_digest
        .strip_prefix(OVEN_COMPILER_RUNTIME_CRATE_PREFIX)
        .map(|_| crate_and_digest)
}

/// Index the unique host/target runtime family declared by an immutable standard-library Loaf.
///
/// Vocabulary auxiliary targets are deliberately excluded: the same `incan_vocab` crate can occur for the host and
/// Wasm target, while this index owns only the normal generated-program ABI family.
fn compiler_runtime_artifacts_by_name(
    base: &OvenRustcArtifactManifest,
) -> Result<BTreeMap<String, OvenRustcSupportingArtifact>, OvenRustcError> {
    let main_search_paths = base
        .dependency_search_paths
        .iter()
        .chain(&base.native_search_paths)
        .map(String::as_str)
        .collect::<Vec<_>>();
    let artifacts = base
        .externs
        .iter()
        .map(|artifact| OvenRustcSupportingArtifact {
            relative_path: artifact.relative_path.clone(),
            digest: artifact.digest.clone(),
        })
        .chain(base.supporting_artifacts.iter().filter_map(|artifact| {
            main_search_paths
                .iter()
                .any(|search_path| artifact_is_below_search_path(&artifact.relative_path, search_path))
                .then_some(artifact.clone())
        }));
    let mut indexed = BTreeMap::new();
    for artifact in artifacts {
        // A `.rmeta` sidecar accompanies a split-metadata rlib (Rust 1.98+) inside the same directory. It is a
        // required companion of the linkable artifact, never a second runtime candidate, so it must not trip the
        // one-artifact-per-crate refusal below or ever be selected as an extern replacement.
        if artifact.relative_path.ends_with(".rmeta") {
            continue;
        }
        let Some(crate_name) = compiler_runtime_name_from_artifact_path(&artifact.relative_path).map(str::to_string)
        else {
            continue;
        };
        if indexed.insert(crate_name.clone(), artifact).is_some() {
            return Err(OvenRustcError::InvalidInput {
                field: "project extension base",
                message: format!("declares multiple compiler runtime artifacts for `{crate_name}`"),
            });
        }
    }
    Ok(indexed)
}

/// Drop `.rmeta` sidecars whose paired rlib is no longer declared by the composed manifest.
///
/// Cohort and registry-leaf replacement swap an extension's compiled rlibs for the selected release family's
/// artifacts. A sidecar names its exact sibling (`libX-<hash>.rmeta` beside `libX-<hash>.rlib`), so once that
/// sibling is replaced the sidecar describes a discarded compilation: materializing it creates a metadata-only
/// crate candidate that rustc can select and then reject with "required to be available in rlib format". A sidecar
/// therefore lives and dies with its declared sibling.
fn discard_orphaned_metadata_sidecars(manifest: &mut OvenRustcArtifactManifest) {
    let declared_linkable_stems = manifest
        .externs
        .iter()
        .map(|artifact| artifact.relative_path.as_str())
        .chain(
            manifest
                .supporting_artifacts
                .iter()
                .map(|artifact| artifact.relative_path.as_str()),
        )
        .chain(
            manifest
                .registry_leaves
                .iter()
                .map(|leaf| leaf.artifact.relative_path.as_str()),
        )
        .chain(
            manifest
                .vocab_auxiliary_targets
                .iter()
                .flat_map(|auxiliary| auxiliary.externs.iter().map(|artifact| artifact.relative_path.as_str())),
        )
        .filter_map(|path| path.strip_suffix(".rlib").map(str::to_string))
        .collect::<BTreeSet<_>>();
    manifest.supporting_artifacts.retain(|artifact| {
        let Some(stem) = artifact.relative_path.strip_suffix(".rmeta") else {
            return true;
        };
        declared_linkable_stems.contains(stem)
    });
}

/// Directory that holds a project extension's retained artifacts whose filenames collide with the selected release
/// base's execution closure.
///
/// A salted extension unit carries a StableCrateId distinct from the sealed base's twin, so both copies of a
/// shared interior unit legally coexist in one crate graph — but Cargo still names them identically, because the
/// rustc-level `-C metadata` salt never enters Cargo's extra-filename hash. Artifact records are keyed by relative
/// path, so the extension's copy moves into this sibling of its `deps` directory; rustc selects each dependent's
/// copy by recorded hash, and the filename — the linkage identity — never changes.
pub(crate) const OVEN_EXTENSION_REROOT_DIR: &str = "extension-deps";

/// Return the `extension-deps` sibling path for an artifact that lives directly in a `deps` directory.
///
/// Returns `None` for artifacts outside a `deps` directory (registry sources, build-script `out/` staging, native
/// libraries): those have no re-rooted home, so a digest collision there remains a genuine incompatibility.
fn rerooted_extension_artifact_path(relative_path: &str) -> Option<String> {
    let path = Path::new(relative_path);
    let file_name = path.file_name()?;
    let parent = path.parent()?;
    if parent.file_name()? != "deps" {
        return None;
    }
    let rerooted = match parent.parent() {
        Some(grandparent) => grandparent.join(OVEN_EXTENSION_REROOT_DIR).join(file_name),
        None => Path::new(OVEN_EXTENSION_REROOT_DIR).join(file_name),
    };
    Some(rerooted.to_string_lossy().into_owned())
}

/// Return the original `deps` staging location for a re-rooted extension artifact, or `None` when the path is not
/// re-rooted.
///
/// [`OvenRustcArtifactManifest::materialized_artifacts`] resolves every artifact's source as the staging root joined
/// with its recorded relative path, so the publisher stages a re-rooted artifact by linking the built file from this
/// returned `deps` location into its `extension-deps` home before atomic copying.
pub(crate) fn rerooted_artifact_staging_source(relative_path: &str) -> Option<String> {
    let path = Path::new(relative_path);
    let file_name = path.file_name()?;
    let parent = path.parent()?;
    if parent.file_name()? != OVEN_EXTENSION_REROOT_DIR {
        return None;
    }
    let source = match parent.parent() {
        Some(grandparent) => grandparent.join("deps").join(file_name),
        None => Path::new("deps").join(file_name),
    };
    Some(source.to_string_lossy().into_owned())
}

/// Return the metadata-sidecar partner of a compiled artifact path: `libX-<hash>.rlib` pairs with the sibling
/// `libX-<hash>.rmeta` and vice versa.
///
/// A split-metadata rlib (Rust 1.98+) and its sidecar are one compilation: whenever cohort composition moves one of
/// them, the partner must move with it, or the stranded half becomes a metadata-only or metadata-less candidate that
/// rustc selects and then rejects.
fn metadata_sidecar_pair_path(relative_path: &str) -> Option<String> {
    if let Some(stem) = relative_path.strip_suffix(".rlib") {
        return Some(format!("{stem}.rmeta"));
    }
    relative_path.strip_suffix(".rmeta").map(|stem| format!("{stem}.rlib"))
}

/// Index the unique `.rmeta` sidecar declared per compiler-runtime crate by an immutable standard-library Loaf.
///
/// Since Rust 1.98 a runtime rlib may carry only a metadata stub, with the crate's real metadata in the sibling
/// `.rmeta` retained beside it. This index lets cohort replacement rewrite an extension's sidecar to the release
/// family's sidecar; crates whose release rlib embeds metadata simply have no entry here.
fn compiler_runtime_sidecars_by_name(
    base: &OvenRustcArtifactManifest,
) -> Result<BTreeMap<String, OvenRustcSupportingArtifact>, OvenRustcError> {
    let main_search_paths = base
        .dependency_search_paths
        .iter()
        .chain(&base.native_search_paths)
        .map(String::as_str)
        .collect::<Vec<_>>();
    let mut indexed = BTreeMap::new();
    for artifact in base.supporting_artifacts.iter().filter(|artifact| {
        artifact.relative_path.ends_with(".rmeta")
            && main_search_paths
                .iter()
                .any(|search_path| artifact_is_below_search_path(&artifact.relative_path, search_path))
    }) {
        let Some(crate_name) = compiler_runtime_name_from_artifact_path(&artifact.relative_path).map(str::to_string)
        else {
            continue;
        };
        if indexed.insert(crate_name.clone(), artifact.clone()).is_some() {
            return Err(OvenRustcError::InvalidInput {
                field: "project extension base",
                message: format!("declares multiple compiler runtime metadata sidecars for `{crate_name}`"),
            });
        }
    }
    Ok(indexed)
}

/// Replace one compiler-owned direct extern with the exact selected release artifact.
fn replace_compiler_runtime_extern(
    artifact: &mut OvenRustcArtifactExtern,
    base_artifacts: &BTreeMap<String, OvenRustcSupportingArtifact>,
) -> Result<(), OvenRustcError> {
    let Some(base_artifact) = base_artifacts.get(&artifact.crate_name) else {
        // The `incan_` prefix alone does not confer compiler ownership. A caller crate such as `incan_partner`
        // remains project-owned unless the selected release Loaf declares that exact runtime crate identity.
        return Ok(());
    };
    artifact.relative_path = base_artifact.relative_path.clone();
    artifact.digest = base_artifact.digest.clone();
    Ok(())
}

/// Replace one compiler-owned supporting artifact with the exact selected release artifact.
///
/// A `.rmeta` sidecar follows its replaced rlib rather than the linkable index: when the selected release family
/// carries its own sidecar for the crate, the extension's sidecar is rewritten to it; when the release rlib embeds
/// its metadata, the extension's now-orphaned sidecar is dropped (`None`). Rewriting a sidecar to the rlib path
/// would double-declare the rlib in the composed manifest.
fn replace_compiler_runtime_supporting_artifact(
    artifact: OvenRustcSupportingArtifact,
    base_artifacts: &BTreeMap<String, OvenRustcSupportingArtifact>,
    base_sidecars: &BTreeMap<String, OvenRustcSupportingArtifact>,
) -> Option<OvenRustcSupportingArtifact> {
    let Some(crate_name) = compiler_runtime_name_from_artifact_path(&artifact.relative_path) else {
        return Some(artifact);
    };
    if artifact.relative_path.ends_with(".rmeta") {
        if !base_artifacts.contains_key(crate_name) {
            // Preserve a caller-owned `incan_*` sidecar that is absent from the selected release family.
            return Some(artifact);
        }
        return base_sidecars.get(crate_name).cloned();
    }
    let Some(base_artifact) = base_artifacts.get(crate_name) else {
        // Preserve a caller-owned `incan_*` artifact that is absent from the selected release family.
        return Some(artifact);
    };
    Some(base_artifact.clone())
}

/// Return the base artifacts required to execute its main and vocabulary closures.
///
/// A release Loaf can also retain registry source authority and provenance used by its own publisher. Those files
/// do not become inputs to every project extension. The project plan keeps its own locked registry catalog, while
/// this projection carries only files reachable through base search paths. Vocabulary root externs remain declared
/// by their auxiliary roles and are therefore excluded from the supporting list to avoid duplicate path ownership.
fn compiler_runtime_execution_support(
    base: &OvenRustcArtifactManifest,
) -> Result<Vec<OvenRustcSupportingArtifact>, OvenRustcError> {
    let auxiliary_extern_paths = base
        .vocab_auxiliary_targets
        .iter()
        .flat_map(|auxiliary| auxiliary.externs.iter().map(|artifact| artifact.relative_path.as_str()))
        .collect::<BTreeSet<_>>();
    let search_paths = base
        .dependency_search_paths
        .iter()
        .chain(&base.native_search_paths)
        .chain(
            base.vocab_auxiliary_targets
                .iter()
                .flat_map(|auxiliary| auxiliary.dependency_search_paths.iter()),
        )
        .map(String::as_str)
        .collect::<Vec<_>>();
    Ok(base
        .composition_artifacts()?
        .into_iter()
        .filter(|artifact| {
            !auxiliary_extern_paths.contains(artifact.relative_path.as_str())
                && search_paths
                    .iter()
                    .any(|search_path| artifact_is_below_search_path(&artifact.relative_path, search_path))
        })
        .collect())
}

/// Verify that every registry package shared with the release uses the release's exact semantic coordinate.
fn validate_release_registry_cohort(
    project: &OvenRustcArtifactManifest,
    base: &OvenRustcArtifactManifest,
) -> Result<(), OvenRustcError> {
    for package in &project.registry_sources {
        let candidates = base
            .registry_sources
            .iter()
            .filter(|candidate| {
                candidate.package == package.package
                    && candidate.version == package.version
                    && candidate.source.registry == package.source.registry
            })
            .collect::<Vec<_>>();
        if candidates.is_empty() {
            continue;
        }
        if !candidates.iter().any(|candidate| candidate.source == package.source) {
            let release_candidates = base
                .registry_sources
                .iter()
                .filter(|candidate| {
                    candidate.package == package.package
                        && candidate.version == package.version
                        && candidate.source.registry == package.source.registry
                })
                .map(|candidate| {
                    format!(
                        "{} features={:?} checksum={}",
                        candidate.version, candidate.features, candidate.source.checksum
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");
            return Err(OvenRustcError::InvalidInput {
                field: "project extension release cohort",
                message: format!(
                    "registry package `{}` {} checksum={} does not match the selected release source identity ({release_candidates})",
                    package.package, package.version, package.source.checksum
                ),
            });
        }
    }
    Ok(())
}

/// Return whether two registry leaves differ only in their independently published artifact bytes.
fn same_registry_leaf_semantics(left: &OvenRustcRegistryLeaf, right: &OvenRustcRegistryLeaf) -> bool {
    left.package == right.package
        && left.version == right.version
        && left.crate_name == right.crate_name
        && left.source == right.source
        && left.features == right.features
}

/// Return whether a project registry leaf may be replaced by its matching release-base counterpart.
///
/// Matching package, version, source checksum, and declared features is necessary but not sufficient: Cargo's unit
/// identity — the `-<hash>` extra-filename suffix — also folds in resolved transitive features, profile details,
/// and the identities of the unit's own dependencies, so the same declared coordinates can legitimately compile to
/// a different crate identity inside a different closure (Bevy's `rand_core` vs the release closure's, #1227).
///
/// Two substitution regimes follow from who consumes the leaf, and the caller passes that judgement as
/// `allow_cross_identity`. A leaf consumed only by recompiled surfaces — the generated root's direct dependencies,
/// or any leaf in a plan with no prebuilt project crates — may swap onto the release copy even when identities
/// differ: that swap is exactly what unifies the root's trait identities (`serde::Serialize`) with the sealed
/// standard library. A leaf that prebuilt extension crates recorded by exact identity hash must not: substituting a
/// different identity removes the only artifact those records can resolve and the sealed closure stops loading.
///
/// In that conservative regime a matching filename is still not enough. The `-<hash>` suffix summarizes Cargo's
/// declared unit inputs, but the strict version hash recorded by consumers also reflects the concrete build
/// environment — a release base prebuilt on another machine publishes the same filename with a different SVH, and a
/// retained consumer compiled here can only load the local build (#1227, published-toolchain lane). Substitution is
/// therefore restricted to bit-identical artifacts — same filename and same content digest — making it a pure
/// byte-canonicalization onto the release's published copy and nothing more.
///
/// A build-script artifact is excluded in both regimes: `build.rs` can probe the ambient build environment and
/// diverge despite identical recorded inputs. It is staged under `build/<package>/<identity>/out/`, unlike an
/// ordinary crate's `deps/` output, so restrict substitution to leaves on both sides that are not
/// build-script-shaped.
fn registry_leaf_substitution_is_safe(
    project_leaf: &OvenRustcRegistryLeaf,
    release_leaf: &OvenRustcRegistryLeaf,
    allow_cross_identity: bool,
) -> bool {
    let is_build_script_shaped = |relative_path: &str| {
        Path::new(relative_path)
            .parent()
            .and_then(Path::file_name)
            .is_some_and(|name| name == "out")
    };
    if is_build_script_shaped(&project_leaf.artifact.relative_path)
        || is_build_script_shaped(&release_leaf.artifact.relative_path)
    {
        return false;
    }
    if allow_cross_identity {
        return true;
    }
    let project_file_name = Path::new(&project_leaf.artifact.relative_path).file_name();
    let release_file_name = Path::new(&release_leaf.artifact.relative_path).file_name();
    project_file_name.is_some()
        && project_file_name == release_file_name
        && project_leaf.artifact.digest == release_leaf.artifact.digest
}

/// Replace project source-catalog facts with the exact selected release record for each shared coordinate.
fn canonicalize_release_registry_sources(
    project: &mut OvenRustcArtifactManifest,
    base: &OvenRustcArtifactManifest,
) -> Result<(), OvenRustcError> {
    for package in &mut project.registry_sources {
        let project_features = package.features.iter().collect::<BTreeSet<_>>();
        let candidates = base
            .registry_sources
            .iter()
            .filter(|candidate| {
                candidate.package == package.package
                    && candidate.version == package.version
                    && candidate.source == package.source
                    && project_features.is_subset(&candidate.features.iter().collect())
            })
            .collect::<Vec<_>>();
        match candidates.as_slice() {
            [] => {}
            [release] => *package = (*release).clone(),
            _ => {
                return Err(OvenRustcError::InvalidInput {
                    field: "project extension release cohort",
                    message: format!(
                        "registry source `{}` {} matches more than one selected release record",
                        package.package, package.version
                    ),
                });
            }
        }
    }
    Ok(())
}

/// Replace a declared project artifact by its exact release-cohort copy.
fn replace_declared_release_artifact(
    manifest: &mut OvenRustcArtifactManifest,
    project_path: &str,
    release: &OvenRustcArtifactExtern,
) {
    if let Some(artifact) = manifest
        .externs
        .iter_mut()
        .find(|artifact| artifact.relative_path == project_path)
    {
        artifact.relative_path = release.relative_path.clone();
        artifact.digest = release.digest.clone();
    }
    if let Some(artifact) = manifest
        .supporting_artifacts
        .iter_mut()
        .find(|artifact| artifact.relative_path == project_path)
    {
        artifact.relative_path = release.relative_path.clone();
        artifact.digest = release.digest.clone();
    }
}

/// Validate one stored project extension against the exact installed release Loaf it names.
///
/// Both Rust-inspection source selection and final execution call this boundary. A payload may not become source
/// authority merely because its complete plan has a valid shape: the publisher plan, selected base, partition, and
/// retained extension paths must all describe the same immutable closure.
pub(crate) fn validate_project_extension_payload_against_base(
    payload: &OvenProjectExtensionPayload,
    base_loaf_identity: &str,
    base_build_unit_identity: &str,
    base: &OvenRustcArtifactManifest,
) -> Result<OvenRustcArtifactPartition, OvenRustcError> {
    validate_project_extension_payload_shape(payload, &base.intent)?;
    if payload.base_loaf_identity != base_loaf_identity || payload.base_build_unit_identity != base_build_unit_identity
    {
        return Err(OvenRustcError::InvalidInput {
            field: "project extension base",
            message: "does not name the exact installed release Loaf and build unit".to_string(),
        });
    }
    // Recomposition must reproduce the sealed complete plan exactly, so it consumes the payload's own record of
    // which registry packages the generated root declares; see `with_release_cohort_from_base`.
    let root_registry_packages = payload
        .registry_source_dependencies
        .iter()
        .chain(&payload.dev_registry_source_dependencies)
        .map(|dependency| dependency.package.clone())
        .collect::<BTreeSet<_>>();
    let recomposed = payload
        .publisher_plan
        .with_release_cohort_from_base(base, &root_registry_packages)?;
    if recomposed != payload.complete_plan {
        return Err(OvenRustcError::InvalidInput {
            field: "project extension release cohort",
            message: "effective plan does not match its sealed publisher plan and exact release cohort".to_string(),
        });
    }
    let partition = payload.complete_plan.partition_against_base(base)?;
    if payload.extension_paths != partition.extension_paths.iter().cloned().collect::<Vec<_>>() {
        return Err(OvenRustcError::InvalidInput {
            field: "project extension fragment",
            message: "does not retain the exact delta derived from its selected base".to_string(),
        });
    }
    if partition.base_paths.is_empty() || partition.extension_paths.is_empty() {
        return Err(OvenRustcError::InvalidInput {
            field: "project extension fragment",
            message: "must contain both a base fragment and a project-specific fragment".to_string(),
        });
    }
    Ok(partition)
}

/// Validate one stored project extension without reading or hashing its materialized artifact bytes.
///
/// Exact base recomposition remains a later validation step once the compiler-owned release Loaf is resolved. This
/// boundary rejects malformed stored constituents immediately after their exact identity and lease are acquired.
fn validate_project_extension_payload_shape(
    payload: &OvenProjectExtensionPayload,
    intent: &OvenBuildIntent,
) -> Result<(), OvenRustcError> {
    if payload.schema_version != OVEN_PROJECT_EXTENSION_PAYLOAD_SCHEMA_VERSION {
        return Err(OvenRustcError::InvalidInput {
            field: "project extension payload",
            message: format!(
                "schema {} is incompatible with current schema {OVEN_PROJECT_EXTENSION_PAYLOAD_SCHEMA_VERSION}",
                payload.schema_version
            ),
        });
    }
    if payload.base_loaf_identity.trim().is_empty() || payload.base_build_unit_identity.trim().is_empty() {
        return Err(OvenRustcError::InvalidInput {
            field: "project extension base",
            message: "must name one exact release Loaf and build unit".to_string(),
        });
    }
    payload.publisher_plan.validate_shape(intent)?;
    payload.complete_plan.validate_shape(intent)?;
    validate_project_registry_source_dependencies(
        &payload.registry_source_dependencies,
        &payload.complete_plan.registry_sources,
    )?;
    validate_project_registry_source_dependencies(
        &payload.dev_registry_source_dependencies,
        &payload.complete_plan.registry_sources,
    )?;
    let declared = payload.complete_plan.declared_artifact_paths()?;
    let mut prior = None::<String>;
    for path in &payload.extension_paths {
        let normalized = normalized_relative_path(path, "project extension artifact")?;
        if prior.as_deref().is_some_and(|prior| prior >= normalized.as_str()) || !declared.contains(&normalized) {
            return Err(OvenRustcError::InvalidInput {
                field: "project extension fragment",
                message: "must be strictly sorted and contain only artifacts declared by the complete plan".to_string(),
            });
        }
        prior = Some(normalized);
    }
    Ok(())
}

/// Validate the explicit baker's alias-to-source authority against the complete immutable source catalog.
fn validate_project_registry_source_dependencies(
    dependencies: &[OvenProjectRegistrySourceDependency],
    sources: &[OvenRustcRegistrySourcePackage],
) -> Result<(), OvenRustcError> {
    let mut previous_alias = None;
    for dependency in dependencies {
        validate_rust_identifier(&dependency.alias)?;
        if dependency.package.trim().is_empty()
            || dependency.version.trim().is_empty()
            || Version::parse(&dependency.version).is_err()
            || !dependency.registry.starts_with("registry+")
            || dependency.checksum.trim().is_empty()
        {
            return Err(OvenRustcError::InvalidInput {
                field: "project extension registry dependencies",
                message: format!(
                    "dependency alias `{}` has an incomplete or invalid locked source identity",
                    dependency.alias
                ),
            });
        }
        if previous_alias.is_some_and(|previous| previous >= dependency.alias.as_str()) {
            return Err(OvenRustcError::InvalidInput {
                field: "project extension registry dependencies",
                message: "must be strictly sorted by unique dependency alias".to_string(),
            });
        }
        previous_alias = Some(dependency.alias.as_str());
        let matches = sources
            .iter()
            .filter(|source| {
                source.package == dependency.package
                    && source.version == dependency.version
                    && source.source.registry == dependency.registry
                    && source.source.checksum == dependency.checksum
            })
            .count();
        if matches != 1 {
            return Err(OvenRustcError::InvalidInput {
                field: "project extension registry dependencies",
                message: format!(
                    "dependency alias `{}` has {matches} exact records in the sealed registry source catalog",
                    dependency.alias
                ),
            });
        }
    }
    Ok(())
}

/// Validate one singular project inspection authority before publication or selection.
pub(crate) fn validate_project_inspection_authority_payload(
    payload: &OvenProjectInspectionAuthorityPayload,
) -> Result<(), OvenRustcError> {
    if payload.schema_version != OVEN_PROJECT_INSPECTION_AUTHORITY_SCHEMA_VERSION {
        return Err(OvenRustcError::InvalidInput {
            field: "project inspection authority",
            message: format!(
                "schema {} is incompatible with current schema {OVEN_PROJECT_INSPECTION_AUTHORITY_SCHEMA_VERSION}",
                payload.schema_version
            ),
        });
    }
    if payload.project_identity.trim().is_empty()
        || payload.source_authority_digest.trim().is_empty()
        || payload.compiler_version.trim().is_empty()
        || payload.registry_lock_digest.trim().is_empty()
    {
        return Err(OvenRustcError::InvalidInput {
            field: "project inspection authority",
            message: "must bind project, source, compiler, and canonical registry-lock identities".to_string(),
        });
    }
    let mut seen_constituents = Vec::new();
    let mut seen_constituent_identities = BTreeSet::new();
    let mut saw_stored_constituent = false;
    let mut release_identities = BTreeSet::new();
    for constituent in &payload.constituents {
        match constituent {
            OvenProjectInspectionConstituent::ReleaseLoaf {
                loaf_identity,
                build_unit_identity,
                ..
            } if loaf_identity.trim().is_empty() || build_unit_identity.trim().is_empty() => {
                return Err(OvenRustcError::InvalidInput {
                    field: "project inspection authority constituent",
                    message: "release Loaf identity and build-unit identity must be non-empty".to_string(),
                });
            }
            OvenProjectInspectionConstituent::ReleaseLoaf {
                loaf_identity, receipt, ..
            } => {
                if saw_stored_constituent
                    || !release_identities.insert(loaf_identity.as_str())
                    || !seen_constituent_identities.insert(loaf_identity.as_str())
                {
                    return Err(OvenRustcError::InvalidInput {
                        field: "project inspection authority constituents",
                        message: "release Loafs must be unique and precede every store-owned constituent".to_string(),
                    });
                }
                receipt
                    .verify_identity()
                    .map_err(|error| OvenRustcError::InvalidInput {
                        field: "project inspection authority release receipt",
                        message: error.to_string(),
                    })?;
            }
            OvenProjectInspectionConstituent::Stored {
                identity,
                artifact_kind,
                receipt,
                base_loaf_identity,
            } => {
                saw_stored_constituent = true;
                receipt
                    .verify_identity()
                    .map_err(|error| OvenRustcError::InvalidInput {
                        field: "project inspection authority constituent receipt",
                        message: error.to_string(),
                    })?;
                if identity.trim().is_empty()
                    || !seen_constituent_identities.insert(identity.as_str())
                    || !matches!(
                        artifact_kind,
                        OvenArtifactKind::DirectRustcPlan | OvenArtifactKind::ProjectPayload
                    )
                    || matches!(artifact_kind, OvenArtifactKind::DirectRustcPlan) && base_loaf_identity.is_some()
                    || matches!(artifact_kind, OvenArtifactKind::ProjectPayload) && base_loaf_identity.is_none()
                {
                    return Err(OvenRustcError::InvalidInput {
                        field: "project inspection authority constituent",
                        message: "stored constituent has inconsistent identity, kind, or base evidence".to_string(),
                    });
                }
                if let Some(base_loaf_identity) = base_loaf_identity
                    && !release_identities.contains(base_loaf_identity.as_str())
                {
                    return Err(OvenRustcError::InvalidInput {
                        field: "project inspection authority constituent",
                        message: "project extension must follow and name one exact release-Loaf constituent"
                            .to_string(),
                    });
                }
            }
        }
        if seen_constituents.contains(constituent) {
            return Err(OvenRustcError::InvalidInput {
                field: "project inspection authority constituents",
                message: "must not repeat an immutable constituent".to_string(),
            });
        }
        seen_constituents.push(constituent.clone());
    }

    if let Some(envelope) = &payload.test_dependency_envelope {
        if envelope.dependency_surface_digest.trim().is_empty() {
            return Err(OvenRustcError::InvalidInput {
                field: "project inspection test dependency envelope",
                message: "must bind a non-empty canonical dependency-surface digest".to_string(),
            });
        }
        let Some(constituent) = payload.constituents.get(envelope.constituent_index) else {
            return Err(OvenRustcError::InvalidInput {
                field: "project inspection test dependency envelope",
                message: "must name one exact immutable constituent".to_string(),
            });
        };
        let receipt = match constituent {
            OvenProjectInspectionConstituent::ReleaseLoaf { receipt, .. } => receipt,
            OvenProjectInspectionConstituent::Stored {
                artifact_kind: OvenArtifactKind::DirectRustcPlan,
                receipt,
                base_loaf_identity: None,
                ..
            } => receipt,
            OvenProjectInspectionConstituent::Stored {
                artifact_kind: OvenArtifactKind::ProjectPayload,
                receipt,
                base_loaf_identity: Some(_),
                ..
            } => receipt,
            _ => {
                return Err(OvenRustcError::InvalidInput {
                    field: "project inspection test dependency envelope",
                    message: "must name the exact release Loaf, one self-contained direct plan, or one base-partitioned project constituent"
                        .to_string(),
                });
            }
        };
        if receipt.intent.profile != "debug" {
            return Err(OvenRustcError::InvalidInput {
                field: "project inspection test dependency envelope",
                message: "must name a debug-profile dependency constituent".to_string(),
            });
        }
        for (alias, root) in &envelope.dependency_roots {
            validate_rust_identifier(alias)?;
            let (dependency_digest, locked) = match root {
                OvenProjectInspectionTestDependencyRoot::Registry {
                    dependency_digest,
                    locked,
                } => (dependency_digest, Some(locked)),
                OvenProjectInspectionTestDependencyRoot::Path { dependency_digest }
                | OvenProjectInspectionTestDependencyRoot::Git { dependency_digest } => (dependency_digest, None),
            };
            if dependency_digest.trim().is_empty() {
                return Err(OvenRustcError::InvalidInput {
                    field: "project inspection test dependency envelope root",
                    message: format!("dependency alias `{alias}` has no portable root digest"),
                });
            }
            if let Some(locked) = locked
                && (locked.alias != *alias
                    || !payload
                        .registry_source_dependencies
                        .iter()
                        .chain(&payload.dev_registry_source_dependencies)
                        .any(|candidate| candidate == locked))
            {
                return Err(OvenRustcError::InvalidInput {
                    field: "project inspection test dependency envelope root",
                    message: format!(
                        "registry dependency alias `{alias}` does not name one exact normal/dev publisher root"
                    ),
                });
            }
        }
    }

    let mut catalog = Vec::with_capacity(payload.registry_sources.len());
    let mut prior_key: Option<(String, String, String, String)> = None;
    for source in &payload.registry_sources {
        let package = &source.package;
        let key = (
            package.package.clone(),
            package.version.clone(),
            package.source.registry.clone(),
            package.source.checksum.clone(),
        );
        if prior_key.as_ref().is_some_and(|prior| prior >= &key)
            || package.package.trim().is_empty()
            || Version::parse(&package.version).is_err()
            || !package.source.registry.starts_with("registry+")
            || package.source.checksum.trim().is_empty()
            || package.source.digest.trim().is_empty()
            || normalized_relative_path(&package.source.relative_root, "registry source root").is_err()
        {
            return Err(OvenRustcError::InvalidInput {
                field: "project inspection authority source catalog",
                message: "must be strictly sorted and contain complete portable source identities".to_string(),
            });
        }
        if let OvenProjectInspectionSourceOwner::Constituent { index } = source.owner
            && index >= payload.constituents.len()
        {
            return Err(OvenRustcError::InvalidInput {
                field: "project inspection authority source owner",
                message: format!("references missing constituent index {index}"),
            });
        }
        prior_key = Some(key);
        catalog.push(package.clone());
    }
    validate_project_inspection_root_dependencies(&payload.registry_source_dependencies, &catalog)?;
    validate_project_inspection_root_dependencies(&payload.dev_registry_source_dependencies, &catalog)?;
    let normal_by_alias = payload
        .registry_source_dependencies
        .iter()
        .map(|dependency| (dependency.alias.as_str(), dependency))
        .collect::<BTreeMap<_, _>>();
    for dependency in &payload.dev_registry_source_dependencies {
        if normal_by_alias
            .get(dependency.alias.as_str())
            .is_some_and(|normal| *normal != dependency)
        {
            return Err(OvenRustcError::InvalidInput {
                field: "project inspection authority dependencies",
                message: format!(
                    "normal and dev dependency alias `{}` resolve to conflicting exact root edges",
                    dependency.alias
                ),
            });
        }
    }
    Ok(())
}

/// Validate feature-bound root edges against the singular authority's exact source catalog.
fn validate_project_inspection_root_dependencies(
    dependencies: &[OvenProjectInspectionRootDependency],
    sources: &[OvenRustcRegistrySourcePackage],
) -> Result<(), OvenRustcError> {
    let mut previous_alias = None;
    for dependency in dependencies {
        validate_rust_identifier(&dependency.alias)?;
        let mut features = dependency.requested_features.clone();
        features.sort();
        features.dedup();
        if dependency.package.trim().is_empty()
            || Version::parse(&dependency.version).is_err()
            || !dependency.registry.starts_with("registry+")
            || dependency.checksum.trim().is_empty()
            || features != dependency.requested_features
            || previous_alias.is_some_and(|previous| previous >= dependency.alias.as_str())
        {
            return Err(OvenRustcError::InvalidInput {
                field: "project inspection authority dependencies",
                message: "must be strictly alias-sorted and contain complete locked source and feature evidence"
                    .to_string(),
            });
        }
        previous_alias = Some(dependency.alias.as_str());
        let matches = sources
            .iter()
            .filter(|source| {
                source.package == dependency.package
                    && source.version == dependency.version
                    && source.source.registry == dependency.registry
                    && source.source.checksum == dependency.checksum
                    && dependency
                        .requested_features
                        .iter()
                        .all(|feature| source.features.contains(feature))
            })
            .count();
        if matches != 1 {
            return Err(OvenRustcError::InvalidInput {
                field: "project inspection authority dependencies",
                message: format!(
                    "dependency alias `{}` has {matches} exact feature-compatible records in the sealed source catalog",
                    dependency.alias
                ),
            });
        }
    }
    Ok(())
}

impl OvenRustcArtifactManifest {
    /// Return the exact compiler-runtime crate names sealed by this release artifact manifest.
    ///
    /// The `incan_` prefix alone does not establish compiler ownership. Consumers must derive runtime ownership from
    /// the selected release Loaf so caller crates with similar names remain caller-owned.
    pub(crate) fn compiler_runtime_crate_names(&self) -> Result<BTreeSet<String>, OvenRustcError> {
        self.validate_shape(&self.intent)?;
        Ok(compiler_runtime_artifacts_by_name(self)?.into_keys().collect())
    }

    /// Return the complete artifact file set declared by this immutable plan without reading artifact bytes.
    pub(crate) fn declared_artifact_paths(&self) -> Result<BTreeSet<String>, OvenRustcError> {
        self.validate_shape(&self.intent)?;
        Ok(expected_artifacts(self)?.into_keys().collect())
    }

    /// Return the same admitted artifact catalog with its producer-established byte identities.
    ///
    /// Native-compilation projection uses this table to label the already selected native artifact plan. It must not
    /// scan a search directory or derive a second dependency graph merely to recover bytes the publisher already
    /// recorded.
    pub(crate) fn declared_artifact_digests(&self) -> Result<BTreeMap<String, String>, OvenRustcError> {
        self.validate_shape(&self.intent)?;
        expected_artifacts(self)
    }

    /// Return the physical release artifacts required by its main, native, and vocabulary search closures.
    pub(crate) fn release_execution_artifacts(&self) -> Result<Vec<OvenRustcSupportingArtifact>, OvenRustcError> {
        compiler_runtime_execution_support(self)
    }

    /// Replace a generated project's release-owned dependency cohort with the exact selected release-base family.
    ///
    /// Cargo gives path dependencies and even locked registry units publisher-local bytes. The selected release Loaf
    /// owns one ABI cohort: exact `incan_*` artifacts plus every registry unit whose package, version, features,
    /// source, target, profile, and toolchain match the sealed release catalog. Project-only registry and path
    /// dependencies remain byte-exact extension inputs. Vocabulary auxiliaries retain their isolated roles.
    ///
    /// `root_registry_packages` names the registry packages the generated root consumes directly (the declared
    /// `[rust-dependencies]`, resolved to Cargo package names). Those leaves may substitute across compilation
    /// identities because the root is recompiled against whatever the composed plan names — that swap is what
    /// unifies the root's trait identities with the sealed standard library. Every other leaf substitutes only onto
    /// an identical compilation identity; see [`registry_leaf_substitution_is_safe`]. When the plan retains
    /// extension-built consumers, no leaf crosses identities, the root's own included: the root then links the
    /// extension's runtime, and its leaves are also dependencies of that runtime and of the retained consumers,
    /// which recorded them by exact identity hash. Because this parameter shapes the composed plan, stored
    /// extension payloads record it and recomposition passes the recorded set.
    pub(crate) fn with_release_cohort_from_base(
        &self,
        base: &Self,
        root_registry_packages: &BTreeSet<String>,
    ) -> Result<Self, OvenRustcError> {
        self.validate_shape(&self.intent)?;
        base.validate_shape(&self.intent)?;
        validate_release_registry_cohort(self, base)?;
        let project_runtime_indexes = self
            .externs
            .iter()
            .enumerate()
            .filter_map(|(index, artifact)| (artifact.crate_name == "incan_stdlib").then_some(index))
            .collect::<Vec<_>>();
        let [project_runtime_index] = project_runtime_indexes.as_slice() else {
            return Err(OvenRustcError::InvalidInput {
                field: "project extension runtime",
                message: "must declare exactly one `incan_stdlib` root extern".to_string(),
            });
        };
        let base_runtimes = base
            .externs
            .iter()
            .filter(|artifact| artifact.crate_name == "incan_stdlib")
            .collect::<Vec<_>>();
        let [base_runtime] = base_runtimes.as_slice() else {
            return Err(OvenRustcError::InvalidInput {
                field: "project extension base",
                message: "must declare exactly one `incan_stdlib` root extern".to_string(),
            });
        };
        let mut composed = self.clone();
        let root_registry_packages = root_registry_packages
            .iter()
            .map(|package| normalized_package_name(package))
            .collect::<BTreeSet<_>>();
        let root_extern_crates = composed
            .externs
            .iter()
            .map(|artifact| artifact.crate_name.clone())
            .collect::<BTreeSet<_>>();
        // Cross-identity substitution is only safe for a leaf whose extension identity nothing retained records
        // (#1227). Two structural facts force the conservative regime for the whole plan: a root-linked leaf with no
        // semantics-matching release counterpart (Bevy, DataFusion) keeps its extension-built subtree, and a
        // prebuilt project crate outside the leaf set (a workspace path dependency) keeps its recorded dependencies;
        // in both cases retained prebuilt crates hold exact identity hashes of other leaves. Without either fact,
        // every retained consumer is recompiled against the composed plan — the generated root plus the sealed
        // release family — and swapping shared leaves onto the release copies is exactly what unifies the root's
        // trait identities with the standard library. In the conservative regime nothing crosses identities, the
        // root's own leaves included: the root links the extension's runtime (below), and a root-linked leaf such
        // as `serde` is also an ordinary dependency of the retained consumers and of that runtime, so the unified
        // Cargo resolution stays the plan and the base contributes only byte-identical canonicalization.
        let leaf_is_root_linked = |leaf: &OvenRustcRegistryLeaf| {
            root_extern_crates.contains(&leaf.crate_name)
                || root_registry_packages.contains(&normalized_package_name(&leaf.package))
        };
        let leaf_has_release_counterpart = |leaf: &OvenRustcRegistryLeaf| {
            base.registry_leaves
                .iter()
                .any(|candidate| same_registry_leaf_semantics(candidate, leaf))
        };
        let leaf_artifact_paths = composed
            .registry_leaves
            .iter()
            .map(|leaf| leaf.artifact.relative_path.clone())
            .collect::<BTreeSet<_>>();
        let has_project_prebuilt_supporting = composed.supporting_artifacts.iter().any(|artifact| {
            artifact.relative_path.ends_with(".rlib")
                && !leaf_artifact_paths.contains(&artifact.relative_path)
                && compiler_runtime_name_from_artifact_path(&artifact.relative_path).is_none()
        });
        let retains_extension_built_consumers = has_project_prebuilt_supporting
            || composed
                .registry_leaves
                .iter()
                .any(|leaf| leaf_is_root_linked(leaf) && !leaf_has_release_counterpart(leaf));
        // ---- Select the compiler runtime the root links ----
        // Every consumer recompiled against the composed plan links the sealed release runtime, which is what
        // unifies the root's trait identities with the standard library. The conservative regime cannot: its
        // retained consumers were compiled by one unified Cargo resolution together with the project's own copy
        // of the runtime, and a unit with process-global state — an async runtime above all — must be linked once.
        // Taking the base's runtime there would bring the base's `tokio` in through the standard library's async
        // component while DataFusion keeps the extension's, and the reactor one side starts is invisible to the
        // other. So the root keeps the extension's runtime in that regime, and the whole link stays the single
        // resolution Cargo produced. The base still contributes its release execution artifacts and vocabulary
        // auxiliaries below.
        if !retains_extension_built_consumers {
            composed.externs[*project_runtime_index] = (*base_runtime).clone();
            let base_compiler_artifacts = compiler_runtime_artifacts_by_name(base)?;
            let base_compiler_sidecars = compiler_runtime_sidecars_by_name(base)?;
            for artifact in &mut composed.externs {
                replace_compiler_runtime_extern(artifact, &base_compiler_artifacts)?;
            }
            composed.supporting_artifacts = std::mem::take(&mut composed.supporting_artifacts)
                .into_iter()
                .filter_map(|artifact| {
                    replace_compiler_runtime_supporting_artifact(
                        artifact,
                        &base_compiler_artifacts,
                        &base_compiler_sidecars,
                    )
                })
                .collect();
        }
        canonicalize_release_registry_sources(&mut composed, base)?;
        let mut leaf_replacements = Vec::new();
        for (index, project_leaf) in composed.registry_leaves.iter().enumerate() {
            let candidates = base
                .registry_leaves
                .iter()
                .filter(|candidate| {
                    candidate.package == project_leaf.package
                        && candidate.source.registry == project_leaf.source.registry
                })
                .collect::<Vec<_>>();
            if candidates.is_empty() {
                continue;
            }
            let allow_cross_identity = !retains_extension_built_consumers;
            let Some(release_leaf) = candidates.into_iter().find(|candidate| {
                same_registry_leaf_semantics(candidate, project_leaf)
                    && registry_leaf_substitution_is_safe(project_leaf, candidate, allow_cross_identity)
                    && composed.registry_sources.iter().any(|project_source| {
                        base.registry_sources.iter().any(|release_source| {
                            project_source == release_source
                                && release_source.package == candidate.package
                                && release_source.version == candidate.version
                                && release_source.source == candidate.source
                        })
                    })
            }) else {
                continue;
            };
            leaf_replacements.push((
                index,
                project_leaf.artifact.relative_path.clone(),
                (*release_leaf).clone(),
            ));
        }
        for (index, project_path, release_leaf) in leaf_replacements {
            replace_declared_release_artifact(&mut composed, &project_path, &release_leaf.artifact);
            composed.registry_leaves[index] = release_leaf;
        }
        // ---- Reconcile extension artifacts that collide with the base execution closure ----
        // A composed artifact can share its filename — and therefore its relative path — with the base's execution
        // closure while holding different bytes: Cargo's extra-filename summarizes declared unit inputs, not the
        // build environment, so a base prebuilt on another machine publishes the same filename with a different
        // strict version hash. Which copy the final link needs depends on the regime. When every retained consumer
        // is recompiled against the composed plan, the extension's publisher-local bytes are superseded and each
        // colliding record adopts the base copy. When prebuilt extension crates are retained, they recorded the
        // local hashes — including proc-macro dependencies — so the link needs both copies: the extension's moves
        // into the sibling `extension-deps` directory with its project digest intact, its split-metadata partner
        // moves with it so neither half is stranded, and the base's copy joins the plan untouched below. A
        // conservative-regime collision outside a `deps` directory has no re-rooted home and stays a hard
        // incompatibility, reported by the merge below.
        let release_artifacts = base
            .release_execution_artifacts()?
            .into_iter()
            .map(|artifact| (artifact.relative_path.clone(), artifact))
            .collect::<BTreeMap<_, _>>();
        if retains_extension_built_consumers {
            let declared_paths = composed
                .externs
                .iter()
                .map(|artifact| (artifact.relative_path.clone(), artifact.digest.clone()))
                .chain(
                    composed
                        .supporting_artifacts
                        .iter()
                        .map(|artifact| (artifact.relative_path.clone(), artifact.digest.clone())),
                )
                .collect::<BTreeMap<_, _>>();
            // A salted extension unit shares its Cargo filename with the base's twin while carrying a distinct
            // StableCrateId, so the plan needs both files: the extension's copy moves into `extension-deps` with
            // its split-metadata sidecar, the directory joins the search paths, and the base's copy joins the plan
            // untouched below. rustc then selects each dependent's copy by the exact hash it recorded. Only a
            // colliding artifact outside a `deps` directory has no re-rooted home; that remains a hard
            // incompatibility, reported by the merge below.
            // A colliding artifact re-roots only when the plan retained the project's compilation of its whole
            // unit. When the unit's linkable half already follows the release — a root-linked leaf substituted onto
            // the base copy — the colliding metadata sidecar describes the base's compilation and adopts the base
            // digest instead of moving: dragging the substituted record into `extension-deps` would pair a
            // base-digest record with salted project bytes.
            let partner_follows_the_base = |relative_path: &str| {
                metadata_sidecar_pair_path(relative_path).is_some_and(|partner| {
                    declared_paths.get(&partner).is_some_and(|partner_digest| {
                        release_artifacts
                            .get(&partner)
                            .is_some_and(|release| &release.digest == partner_digest)
                    })
                })
            };
            let mut rerooted_paths = BTreeMap::new();
            let mut base_adopted_paths = BTreeSet::new();
            for (relative_path, digest) in &declared_paths {
                let Some(release) = release_artifacts.get(relative_path) else {
                    continue;
                };
                if &release.digest == digest {
                    continue;
                }
                if partner_follows_the_base(relative_path) {
                    base_adopted_paths.insert(relative_path.clone());
                    continue;
                }
                let Some(rerooted) = rerooted_extension_artifact_path(relative_path) else {
                    continue;
                };
                rerooted_paths.insert(relative_path.clone(), rerooted);
            }
            for (relative_path, rerooted) in rerooted_paths.clone() {
                let Some(partner) = metadata_sidecar_pair_path(&relative_path) else {
                    continue;
                };
                if !declared_paths.contains_key(&partner)
                    || rerooted_paths.contains_key(&partner)
                    || base_adopted_paths.contains(&partner)
                {
                    continue;
                }
                let Some(partner_rerooted) = metadata_sidecar_pair_path(&rerooted) else {
                    continue;
                };
                rerooted_paths.insert(partner, partner_rerooted);
            }
            for artifact in &mut composed.externs {
                if base_adopted_paths.contains(&artifact.relative_path)
                    && let Some(release) = release_artifacts.get(&artifact.relative_path)
                {
                    artifact.digest = release.digest.clone();
                }
            }
            for artifact in &mut composed.supporting_artifacts {
                if base_adopted_paths.contains(&artifact.relative_path)
                    && let Some(release) = release_artifacts.get(&artifact.relative_path)
                {
                    artifact.digest = release.digest.clone();
                }
            }
            for leaf in &mut composed.registry_leaves {
                if base_adopted_paths.contains(&leaf.artifact.relative_path)
                    && let Some(release) = release_artifacts.get(&leaf.artifact.relative_path)
                {
                    leaf.artifact.digest = release.digest.clone();
                }
            }
            if !rerooted_paths.is_empty() {
                // A record that already carries the release digest at a colliding path is the base's copy and
                // stays put; only the project's copy under that path moves.
                let follows_the_base = |relative_path: &str, digest: &str| {
                    release_artifacts
                        .get(relative_path)
                        .is_some_and(|release| release.digest == digest)
                };
                for artifact in &mut composed.externs {
                    if let Some(rerooted) = rerooted_paths.get(&artifact.relative_path)
                        && !follows_the_base(&artifact.relative_path, &artifact.digest)
                    {
                        artifact.relative_path = rerooted.clone();
                    }
                }
                for artifact in &mut composed.supporting_artifacts {
                    if let Some(rerooted) = rerooted_paths.get(&artifact.relative_path)
                        && !follows_the_base(&artifact.relative_path, &artifact.digest)
                    {
                        artifact.relative_path = rerooted.clone();
                    }
                }
                for leaf in &mut composed.registry_leaves {
                    if let Some(rerooted) = rerooted_paths.get(&leaf.artifact.relative_path)
                        && !follows_the_base(&leaf.artifact.relative_path, &leaf.artifact.digest)
                    {
                        leaf.artifact.relative_path = rerooted.clone();
                    }
                }
                let mut rerooted_search_dirs = BTreeSet::new();
                for rerooted in rerooted_paths.values() {
                    if let Some(parent) = Path::new(rerooted).parent()
                        && !parent.as_os_str().is_empty()
                    {
                        rerooted_search_dirs.insert(parent.to_string_lossy().into_owned());
                    }
                }
                composed.dependency_search_paths.extend(rerooted_search_dirs);
            }
        } else {
            for artifact in &mut composed.externs {
                if let Some(release) = release_artifacts.get(&artifact.relative_path) {
                    artifact.digest = release.digest.clone();
                }
            }
            for artifact in &mut composed.supporting_artifacts {
                if let Some(release) = release_artifacts.get(&artifact.relative_path) {
                    artifact.digest = release.digest.clone();
                }
            }
            for leaf in &mut composed.registry_leaves {
                if let Some(release) = release_artifacts.get(&leaf.artifact.relative_path) {
                    leaf.artifact.digest = release.digest.clone();
                }
            }
        }
        composed.vocab_auxiliary_targets = base.vocab_auxiliary_targets.clone();
        let mut declared = expected_artifacts(&composed)?;
        for artifact in base.release_execution_artifacts()? {
            if artifact.relative_path == base_runtime.relative_path {
                continue;
            }
            match declared.get(&artifact.relative_path) {
                Some(digest) if digest == &artifact.digest => {}
                Some(digest) => {
                    return Err(OvenRustcError::InvalidInput {
                        field: "project extension release support",
                        message: format!(
                            "release-base artifact `{}` conflicts with project artifact digest {digest}",
                            artifact.relative_path
                        ),
                    });
                }
                None => {
                    declared.insert(artifact.relative_path.clone(), artifact.digest.clone());
                    composed.supporting_artifacts.push(artifact);
                }
            }
        }
        composed
            .dependency_search_paths
            .extend(base.dependency_search_paths.iter().cloned());
        composed.dependency_search_paths.sort();
        composed.dependency_search_paths.dedup();
        composed
            .native_search_paths
            .extend(base.native_search_paths.iter().cloned());
        composed.native_search_paths.sort();
        composed.native_search_paths.dedup();
        discard_orphaned_metadata_sidecars(&mut composed);
        composed.supporting_artifacts.sort_by(|left, right| {
            left.relative_path
                .cmp(&right.relative_path)
                .then_with(|| left.digest.cmp(&right.digest))
        });
        composed.validate_shape(&self.intent)?;
        Ok(composed)
    }

    /// Verify that this project plan already uses the exact release cohort selected from its base.
    ///
    /// An already-composed plan recomposes to itself regardless of the root-package set: leaves that were
    /// substituted are identity-equal to the base copy, and leaves that were kept decline substitution again, so
    /// this check passes an empty root set.
    pub(crate) fn validate_release_cohort_from_base(&self, base: &Self) -> Result<(), OvenRustcError> {
        if self.with_release_cohort_from_base(base, &BTreeSet::new())? != *self {
            return Err(OvenRustcError::InvalidInput {
                field: "project extension release cohort",
                message:
                    "does not use the exact runtime, registry, and vocabulary cohort from its selected release base"
                        .to_string(),
            });
        }
        Ok(())
    }

    /// Partition this complete publisher closure against one already selected base Loaf.
    ///
    /// Only byte-identical declared artifacts can be supplied by the base.  A path collision with a different
    /// digest is a feature/toolchain incompatibility, not an opportunity to select whichever copy happens to be
    /// present first.  The caller retains the two resulting fragments under independent active leases and composes
    /// them through [`Self::materialize_trusted_store_composed`].
    pub(crate) fn partition_against_base(&self, base: &Self) -> Result<OvenRustcArtifactPartition, OvenRustcError> {
        self.validate_shape(&self.intent)?;
        base.validate_shape(&self.intent)?;
        let complete = expected_artifacts(self)?;
        let base_artifacts = expected_artifacts(base)?;
        let mut base_paths = BTreeSet::new();
        let mut extension_paths = BTreeSet::new();
        for (path, digest) in complete {
            match base_artifacts.get(&path) {
                Some(base_digest) if base_digest == &digest => {
                    base_paths.insert(path);
                }
                Some(base_digest) => {
                    return Err(OvenRustcError::InvalidInput {
                        field: "project extension base",
                        message: format!(
                            "artifact `{path}` conflicts with selected base Loaf digest {base_digest}; project extension requires {digest}"
                        ),
                    });
                }
                None => {
                    extension_paths.insert(path);
                }
            }
        }
        Ok(OvenRustcArtifactPartition {
            base_paths,
            extension_paths,
        })
    }

    /// Retain the one verified fragment of this complete closure whose artifacts are named by `paths`.
    ///
    /// The result deliberately omits source-evidence routing: only the complete outer manifest owns the execution
    /// contract.  A fragment is an artifact-root declaration for the compositor, not a second executable plan.
    pub(crate) fn artifact_fragment(&self, paths: &BTreeSet<String>) -> Result<Self, OvenRustcError> {
        self.validate_shape(&self.intent)?;
        let declared = expected_artifacts(self)?;
        if let Some(unknown) = paths.iter().find(|path| !declared.contains_key(*path)) {
            return Err(OvenRustcError::InvalidInput {
                field: "project extension fragment",
                message: format!("names artifact `{unknown}` absent from the complete plan"),
            });
        }
        let includes = |relative_path: &str| paths.contains(relative_path);
        let retains_search_path = |search_path: &str| {
            paths
                .iter()
                .any(|relative_path| artifact_is_below_search_path(relative_path, search_path))
        };
        let mut vocab_auxiliary_targets = Vec::new();
        for auxiliary in &self.vocab_auxiliary_targets {
            let externs = auxiliary
                .externs
                .iter()
                .filter(|artifact| includes(&artifact.relative_path))
                .cloned()
                .collect::<Vec<_>>();
            if !externs.is_empty() {
                vocab_auxiliary_targets.push(OvenRustcAuxiliaryTarget {
                    target: auxiliary.target.clone(),
                    dependency_search_paths: auxiliary
                        .dependency_search_paths
                        .iter()
                        .filter(|path| retains_search_path(path))
                        .cloned()
                        .collect(),
                    externs,
                });
            }
        }
        Ok(Self {
            schema_version: self.schema_version,
            intent: self.intent.clone(),
            dependency_search_paths: self
                .dependency_search_paths
                .iter()
                .filter(|path| retains_search_path(path))
                .cloned()
                .collect(),
            native_search_paths: self
                .native_search_paths
                .iter()
                .filter(|path| retains_search_path(path))
                .cloned()
                .collect(),
            externs: self
                .externs
                .iter()
                .filter(|artifact| includes(&artifact.relative_path))
                .cloned()
                .collect(),
            entrypoint_externs: BTreeMap::new(),
            registry_leaves: Vec::new(),
            registry_sources: Vec::new(),
            compile_environment: self.compile_environment.clone(),
            vocab_auxiliary_targets,
            supporting_artifacts: self
                .supporting_artifacts
                .iter()
                .filter(|artifact| includes(&artifact.relative_path))
                .cloned()
                .collect(),
        })
    }

    /// Return every declared artifact as a root fragment record for trusted multi-root composition.
    ///
    /// `externs` and vocabulary auxiliary externs are execution roles, not separate files.  The compositor needs
    /// every physical path exactly once before it can later map the complete outer manifest's `--extern` list.
    pub(crate) fn composition_artifacts(&self) -> Result<Vec<OvenRustcSupportingArtifact>, OvenRustcError> {
        self.validate_shape(&self.intent)?;
        let mut artifacts = self
            .externs
            .iter()
            .map(|artifact| OvenRustcSupportingArtifact {
                relative_path: artifact.relative_path.clone(),
                digest: artifact.digest.clone(),
            })
            .chain(self.supporting_artifacts.iter().cloned())
            .chain(self.vocab_auxiliary_targets.iter().flat_map(|auxiliary| {
                auxiliary.externs.iter().map(|artifact| OvenRustcSupportingArtifact {
                    relative_path: artifact.relative_path.clone(),
                    digest: artifact.digest.clone(),
                })
            }))
            .collect::<Vec<_>>();
        artifacts.sort_by(|left, right| {
            left.relative_path
                .cmp(&right.relative_path)
                .then_with(|| left.digest.cmp(&right.digest))
        });
        if artifacts
            .windows(2)
            .any(|pair| pair[0].relative_path == pair[1].relative_path)
        {
            return Err(OvenRustcError::InvalidInput {
                field: "artifact manifest",
                message: "declares one composition artifact path more than once".to_string(),
            });
        }
        Ok(artifacts)
    }

    /// Verify and materialize exact compiler inputs without scanning Cargo output or resolving dependencies.
    pub fn materialize(
        &self,
        artifact_root: &Path,
        expected_intent: &OvenBuildIntent,
    ) -> Result<OvenRustcArtifactPlan, OvenRustcError> {
        self.validate_shape(expected_intent)?;
        let root = canonical_directory(artifact_root, "artifact root")?;
        let expected = expected_artifacts(self)?;
        let dependency_search_paths =
            materialize_search_paths(&root, &self.dependency_search_paths, "dependency search", &expected)?;
        let native_search_paths =
            materialize_search_paths(&root, &self.native_search_paths, "native search", &expected)?;
        for auxiliary in &self.vocab_auxiliary_targets {
            let _ = materialize_search_paths(
                &root,
                &auxiliary.dependency_search_paths,
                "vocab auxiliary dependency search",
                &expected,
            )?;
            for artifact in &auxiliary.externs {
                let _ = verified_file(
                    &root,
                    &artifact.relative_path,
                    &artifact.digest,
                    "vocab auxiliary extern",
                )?;
            }
        }
        let externs = self
            .externs
            .iter()
            .map(|artifact| {
                validate_rust_identifier(&artifact.crate_name)?;
                let path = verified_file(&root, &artifact.relative_path, &artifact.digest, "extern")?;
                Ok((artifact.crate_name.clone(), path))
            })
            .collect::<Result<Vec<_>, OvenRustcError>>()?;
        for artifact in &self.supporting_artifacts {
            verified_file(&root, &artifact.relative_path, &artifact.digest, "supporting")?;
        }
        Ok(OvenRustcArtifactPlan {
            dependency_search_paths,
            native_search_paths,
            externs,
            compile_environment: validated_compile_environment(&self.compile_environment)?,
            caller_owned_library_digests: BTreeMap::new(),
        })
    }

    /// Return the complete verified publisher closure for atomic copying into a store-owned artifact root.
    pub fn materialized_artifacts(
        &self,
        artifact_root: &Path,
        expected_intent: &OvenBuildIntent,
    ) -> Result<Vec<OvenRustcMaterializedArtifact>, OvenRustcError> {
        self.validate_shape(expected_intent)?;
        let root = canonical_directory(artifact_root, "artifact root")?;
        let expected = expected_artifacts(self)?;
        validate_publisher_search_paths(&self.dependency_search_paths, "dependency search", &expected)?;
        validate_publisher_search_paths(&self.native_search_paths, "native search", &expected)?;
        for auxiliary in &self.vocab_auxiliary_targets {
            validate_publisher_search_paths(
                &auxiliary.dependency_search_paths,
                "vocab auxiliary dependency search",
                &expected,
            )?;
        }
        expected
            .into_iter()
            .map(|(relative_path, digest)| {
                Ok(OvenRustcMaterializedArtifact {
                    source_path: verified_file(&root, &relative_path, &digest, "materialized")?,
                    relative_path,
                })
            })
            .collect()
    }

    /// Materialize a plan that has already been atomically copied and digest-verified by the Oven store publisher.
    ///
    /// Normal consumers still verify the receipt, plan payload, artifact-root containment, regular-file shape, and
    /// active lease. They intentionally do not rehash every retained dependency: doing so turns a prepared test run
    /// into a multi-gigabyte integrity scan. `inspect oven` remains the explicit full-closure audit operation.
    pub(crate) fn materialize_trusted_store(
        &self,
        artifact_root: &Path,
        expected_intent: &OvenBuildIntent,
    ) -> Result<OvenRustcArtifactPlan, OvenRustcError> {
        self.validate_shape(expected_intent)?;
        let root = canonical_directory(artifact_root, "artifact root")?;
        let expected = expected_artifacts(self)?;
        let mut trusted_parents = BTreeMap::new();
        let dependency_search_paths = trusted_materialize_search_paths(
            &root,
            &self.dependency_search_paths,
            "dependency search",
            &expected,
            &mut trusted_parents,
        )?;
        let native_search_paths = trusted_materialize_search_paths(
            &root,
            &self.native_search_paths,
            "native search",
            &expected,
            &mut trusted_parents,
        )?;
        for auxiliary in &self.vocab_auxiliary_targets {
            let _ = trusted_materialize_search_paths(
                &root,
                &auxiliary.dependency_search_paths,
                "vocab auxiliary dependency search",
                &expected,
                &mut trusted_parents,
            )?;
            for artifact in &auxiliary.externs {
                let _ = trusted_file(
                    &root,
                    &artifact.relative_path,
                    "vocab auxiliary extern",
                    &mut trusted_parents,
                )?;
            }
        }
        let externs = self
            .externs
            .iter()
            .map(|artifact| {
                Ok((
                    artifact.crate_name.clone(),
                    trusted_file(&root, &artifact.relative_path, "extern", &mut trusted_parents)?,
                ))
            })
            .collect::<Result<Vec<_>, OvenRustcError>>()?;
        for artifact in &self.supporting_artifacts {
            trusted_file(&root, &artifact.relative_path, "supporting", &mut trusted_parents)?;
        }
        Ok(OvenRustcArtifactPlan {
            dependency_search_paths,
            native_search_paths,
            externs,
            compile_environment: validated_compile_environment(&self.compile_environment)?,
            caller_owned_library_digests: BTreeMap::new(),
        })
    }

    /// Materialize one publisher-declared vocabulary cross-target closure from an already selected immutable root.
    ///
    /// This is intentionally not part of the ordinary [`OvenRustcArtifactPlan`]: normal host commands must never
    /// hand cross-target artifacts to Rustc. Vocabulary extraction names the target from its verified metadata and
    /// may receive only the exact matching auxiliary closure.
    pub(crate) fn materialize_trusted_vocab_auxiliary_target(
        &self,
        artifact_root: &Path,
        target: &str,
    ) -> Result<Option<OvenRustcAuxiliaryTargetPlan>, OvenRustcError> {
        let root = canonical_directory(artifact_root, "artifact root")?;
        let expected = expected_artifacts(self)?;
        let mut trusted_parents = BTreeMap::new();
        let Some(auxiliary) = self
            .vocab_auxiliary_targets
            .iter()
            .find(|auxiliary| auxiliary.target == target)
        else {
            return Ok(None);
        };
        let dependency_search_paths = trusted_materialize_search_paths(
            &root,
            &auxiliary.dependency_search_paths,
            "vocab auxiliary dependency search",
            &expected,
            &mut trusted_parents,
        )?;
        let externs = auxiliary
            .externs
            .iter()
            .map(|artifact| {
                Ok((
                    artifact.crate_name.clone(),
                    trusted_file(
                        &root,
                        &artifact.relative_path,
                        "vocab auxiliary extern",
                        &mut trusted_parents,
                    )?,
                ))
            })
            .collect::<Result<Vec<_>, OvenRustcError>>()?;
        Ok(Some(OvenRustcAuxiliaryTargetPlan {
            dependency_search_paths,
            externs,
        }))
    }

    /// Materialize a complete direct-rustc plan from several separately bounded, actively leased Oven roots.
    ///
    /// The outer manifest remains the one target-specific execution contract. Each root contributes a disjoint
    /// publisher-declared fragment; this method rejects a missing, duplicate, or substituted path before passing
    /// any `-L` or `--extern` argument to Rustc.
    pub(crate) fn materialize_trusted_store_composed(
        &self,
        roots: &[OvenTrustedRustcArtifactRoot<'_>],
        expected_intent: &OvenBuildIntent,
    ) -> Result<OvenRustcArtifactPlan, OvenRustcError> {
        self.validate_shape(expected_intent)?;
        if roots.is_empty() {
            return Err(OvenRustcError::InvalidInput {
                field: "composed artifact roots",
                message: "must contain at least one actively leased foundation".to_string(),
            });
        }
        let expected = expected_artifacts(self)?;
        let mut locations = BTreeMap::<String, PathBuf>::new();
        let mut dependency_search_paths = Vec::new();
        let mut native_search_paths = Vec::new();
        for fragment in roots {
            let root = canonical_directory(fragment.artifact_root, "composed artifact root")?;
            let mut trusted_parents = BTreeMap::new();
            let mut fragment_expected = BTreeMap::new();
            for artifact in fragment.supporting_artifacts {
                let relative = normalized_relative_path(&artifact.relative_path, "composed supporting artifact")?;
                let expected_digest = expected.get(&relative).ok_or_else(|| OvenRustcError::InvalidInput {
                    field: "composed artifact roots",
                    message: format!("declare unrecognized artifact `{relative}`"),
                })?;
                if expected_digest != &artifact.digest {
                    return Err(OvenRustcError::InvalidInput {
                        field: "composed artifact roots",
                        message: format!("declare mismatched digest for `{relative}`"),
                    });
                }
                let path = trusted_file(&root, &relative, "composed supporting artifact", &mut trusted_parents)?;
                if locations.insert(relative.clone(), path).is_some() {
                    return Err(OvenRustcError::InvalidInput {
                        field: "composed artifact roots",
                        message: format!("declare duplicate artifact `{relative}`"),
                    });
                }
                if fragment_expected.insert(relative, artifact.digest.clone()).is_some() {
                    return Err(OvenRustcError::InvalidInput {
                        field: "composed artifact roots",
                        message: "declare one artifact path more than once".to_string(),
                    });
                }
            }
            if fragment_expected.is_empty() {
                return Err(OvenRustcError::InvalidInput {
                    field: "composed artifact roots",
                    message: "must not contain an empty foundation fragment".to_string(),
                });
            }
            dependency_search_paths.extend(trusted_materialize_search_paths(
                &root,
                fragment.dependency_search_paths,
                "composed dependency search",
                &fragment_expected,
                &mut trusted_parents,
            )?);
            native_search_paths.extend(trusted_materialize_search_paths(
                &root,
                fragment.native_search_paths,
                "composed native search",
                &fragment_expected,
                &mut trusted_parents,
            )?);
        }
        let missing = expected
            .keys()
            .filter(|relative| !locations.contains_key(*relative))
            .cloned()
            .collect::<Vec<_>>();
        if !missing.is_empty() {
            return Err(OvenRustcError::InvalidInput {
                field: "composed artifact roots",
                message: format!("omit required artifact(s): {}", missing.join(", ")),
            });
        }
        dependency_search_paths.sort();
        dependency_search_paths.dedup();
        native_search_paths.sort();
        native_search_paths.dedup();
        let externs = self
            .externs
            .iter()
            .map(|artifact| {
                let relative = normalized_relative_path(&artifact.relative_path, "extern")?;
                let path = locations.get(&relative).ok_or_else(|| OvenRustcError::InvalidInput {
                    field: "composed artifact roots",
                    message: format!("omit extern `{relative}`"),
                })?;
                Ok((artifact.crate_name.clone(), path.clone()))
            })
            .collect::<Result<Vec<_>, OvenRustcError>>()?;
        Ok(OvenRustcArtifactPlan {
            dependency_search_paths,
            native_search_paths,
            externs,
            compile_environment: validated_compile_environment(&self.compile_environment)?,
            caller_owned_library_digests: BTreeMap::new(),
        })
    }

    /// Borrow named members of an original declared role without granting physical execution authority.
    ///
    /// The existing projection validates the declaration once. Returned records still require an original leased
    /// owner and physical attachment before execution; absent roles cannot use legacy broad projection.
    pub(crate) fn named_artifacts_for_source_role(
        &self,
        source_role: &str,
    ) -> Result<Vec<&OvenRustcArtifactExtern>, OvenRustcError> {
        if !self.entrypoint_externs.contains_key(source_role) {
            return Err(OvenRustcError::InvalidInput {
                field: "native input source role",
                message: format!("`{source_role}` is not an original declared role of this foundation"),
            });
        }
        let projected = self.for_source_evidence(source_role)?;
        Ok(self
            .externs
            .iter()
            .filter(|artifact| projected.externs.contains(artifact))
            .collect())
    }

    /// Select the exact direct root externs authorized for one receipt source target.
    ///
    /// The unselected extern artifacts remain declared supporting inputs so strict search-directory completeness and
    /// publisher-time digest verification still cover the entire immutable compatibility closure.
    fn for_source_evidence(&self, source_evidence_key: &str) -> Result<Self, OvenRustcError> {
        let Some(allowed_names) = self.entrypoint_externs.get(source_evidence_key) else {
            return Ok(self.clone());
        };
        let allowed = allowed_names.iter().cloned().collect::<BTreeSet<_>>();
        let available = self
            .externs
            .iter()
            .map(|artifact| artifact.crate_name.clone())
            .collect::<BTreeSet<_>>();
        let missing = allowed
            .iter()
            .filter(|crate_name| !available.contains(*crate_name))
            .cloned()
            .collect::<Vec<_>>();
        if !missing.is_empty() {
            return Err(OvenRustcError::InvalidInput {
                field: "artifact manifest entrypoint externs",
                message: format!(
                    "source evidence `{source_evidence_key}` names undeclared extern(s): {}",
                    missing.join(", ")
                ),
            });
        }
        let mut selected = self.clone();
        // A helper-only dependency directory may contain a second build of a crate that has the same stable Rust
        // identity as a generated-root dependency. Keeping that directory on `-L dependency` after its only direct
        // roots are projected away still lets Rustc discover the conflicting copy. Retain every mixed or
        // transitive-only directory, but remove a directory whose declared direct roots are all excluded.
        let excluded_dependency_search_paths = self
            .dependency_search_paths
            .iter()
            .filter(|search_path| {
                let direct_roots = self
                    .externs
                    .iter()
                    .filter(|artifact| artifact_is_below_search_path(&artifact.relative_path, search_path))
                    .collect::<Vec<_>>();
                !direct_roots.is_empty()
                    && direct_roots
                        .iter()
                        .all(|artifact| !allowed.contains(&artifact.crate_name))
            })
            .cloned()
            .collect::<BTreeSet<_>>();
        let retained = selected
            .externs
            .iter()
            .filter(|artifact| !allowed.contains(&artifact.crate_name))
            .map(|artifact| OvenRustcSupportingArtifact {
                relative_path: artifact.relative_path.clone(),
                digest: artifact.digest.clone(),
            })
            .collect::<Vec<_>>();
        selected
            .externs
            .retain(|artifact| allowed.contains(&artifact.crate_name));
        selected
            .dependency_search_paths
            .retain(|search_path| !excluded_dependency_search_paths.contains(search_path));
        selected.supporting_artifacts.extend(retained);
        Ok(selected)
    }

    /// Validate receipt-independent manifest shape before a stored plan may be selected or republished.
    ///
    /// This deliberately avoids a full byte-by-byte artifact traversal. Publication and execution perform that
    /// stronger validation; selection uses this inexpensive gate so a legacy malformed payload cannot make a newly
    /// corrected plan ambiguous forever.
    pub fn validate_shape(&self, expected_intent: &OvenBuildIntent) -> Result<(), OvenRustcError> {
        if self.schema_version != OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION {
            return Err(OvenRustcError::UnsupportedSchema {
                found: self.schema_version,
                expected: OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION,
            });
        }
        if &self.intent != expected_intent {
            return Err(OvenRustcError::IntentMismatch);
        }
        let _ = validated_compile_environment(&self.compile_environment)?;
        let mut names = BTreeSet::new();
        for artifact in &self.externs {
            validate_rust_identifier(&artifact.crate_name)?;
            if !names.insert(artifact.crate_name.as_str()) {
                return Err(OvenRustcError::InvalidInput {
                    field: "artifact manifest",
                    message: format!("declares duplicate extern crate `{}`", artifact.crate_name),
                });
            }
        }
        for (source_evidence_key, crate_names) in &self.entrypoint_externs {
            if source_evidence_key.trim().is_empty() {
                return Err(OvenRustcError::InvalidInput {
                    field: "artifact manifest entrypoint externs",
                    message: "must not contain an empty source-evidence key".to_string(),
                });
            }
            let mut seen = BTreeSet::new();
            for crate_name in crate_names {
                validate_rust_identifier(crate_name)?;
                if !names.contains(crate_name.as_str()) {
                    return Err(OvenRustcError::InvalidInput {
                        field: "artifact manifest entrypoint externs",
                        message: format!(
                            "source evidence `{source_evidence_key}` names undeclared extern `{crate_name}`"
                        ),
                    });
                }
                if !seen.insert(crate_name.as_str()) {
                    return Err(OvenRustcError::InvalidInput {
                        field: "artifact manifest entrypoint externs",
                        message: format!(
                            "source evidence `{source_evidence_key}` names extern `{crate_name}` more than once"
                        ),
                    });
                }
            }
        }
        let mut declared_artifacts = BTreeMap::new();
        for artifact in self
            .externs
            .iter()
            .map(|artifact| (&artifact.relative_path, &artifact.digest))
            .chain(
                self.supporting_artifacts
                    .iter()
                    .map(|artifact| (&artifact.relative_path, &artifact.digest)),
            )
        {
            if declared_artifacts
                .insert(artifact.0.as_str(), artifact.1.as_str())
                .is_some()
            {
                return Err(OvenRustcError::InvalidInput {
                    field: "artifact manifest registry catalog",
                    message: format!("declares artifact `{}` more than once", artifact.0),
                });
            }
        }
        let mut registry_source_identities = BTreeMap::new();
        let mut registry_package_sources = BTreeSet::new();
        for package in &self.registry_sources {
            if package.package.trim().is_empty() || package.version.trim().is_empty() {
                return Err(OvenRustcError::InvalidInput {
                    field: "artifact manifest registry sources",
                    message: "registry source package and version must not be empty".to_string(),
                });
            }
            let mut features = BTreeSet::new();
            for feature in &package.features {
                if feature.trim().is_empty() || !features.insert(feature.as_str()) {
                    return Err(OvenRustcError::InvalidInput {
                        field: "artifact manifest registry sources",
                        message: format!(
                            "registry source `{}` `{}` declares an empty or duplicate feature",
                            package.package, package.version
                        ),
                    });
                }
            }
            if !package.source.registry.starts_with("registry+")
                || package.source.checksum.trim().is_empty()
                || package.source.digest.trim().is_empty()
            {
                return Err(OvenRustcError::InvalidInput {
                    field: "artifact manifest registry sources",
                    message: format!(
                        "registry source `{}` `{}` has incomplete source identity",
                        package.package, package.version
                    ),
                });
            }
            let source_root = Path::new(&package.source.relative_root);
            if source_root.is_absolute()
                || source_root.as_os_str().is_empty()
                || source_root.components().any(|component| {
                    matches!(
                        component,
                        std::path::Component::ParentDir
                            | std::path::Component::RootDir
                            | std::path::Component::Prefix(_)
                    )
                })
            {
                return Err(OvenRustcError::InvalidInput {
                    field: "artifact manifest registry sources",
                    message: format!(
                        "registry source `{}` `{}` has an unsafe source root",
                        package.package, package.version
                    ),
                });
            }
            let source_manifest = source_root.join("Cargo.toml").to_string_lossy().replace('\\', "/");
            if !declared_artifacts.contains_key(source_manifest.as_str()) {
                return Err(OvenRustcError::InvalidInput {
                    field: "artifact manifest registry sources",
                    message: format!(
                        "registry source `{}` `{}` is not declared by the immutable plan",
                        package.package, package.version
                    ),
                });
            }
            let key = (
                package.package.as_str(),
                package.version.as_str(),
                package.source.registry.as_str(),
                package.source.checksum.as_str(),
            );
            if !registry_package_sources.insert((key.0, key.1, key.2)) {
                return Err(OvenRustcError::InvalidInput {
                    field: "artifact manifest registry sources",
                    message: format!(
                        "declares more than one source identity for registry package `{}` version `{}`",
                        package.package, package.version
                    ),
                });
            }
            if registry_source_identities.insert(key, package).is_some() {
                return Err(OvenRustcError::InvalidInput {
                    field: "artifact manifest registry sources",
                    message: format!(
                        "declares registry source `{}` version `{}` more than once",
                        package.package, package.version
                    ),
                });
            }
        }
        let mut package_versions = BTreeSet::new();
        for leaf in &self.registry_leaves {
            if leaf.package.trim().is_empty() || leaf.version.trim().is_empty() {
                return Err(OvenRustcError::InvalidInput {
                    field: "artifact manifest registry catalog",
                    message: "registry leaf package and version must not be empty".to_string(),
                });
            }
            validate_rust_identifier(&leaf.crate_name)?;
            if leaf.crate_name != leaf.artifact.crate_name {
                return Err(OvenRustcError::InvalidInput {
                    field: "artifact manifest registry catalog",
                    message: format!(
                        "registry leaf `{}` `{}` has inconsistent crate identity",
                        leaf.package, leaf.version
                    ),
                });
            }
            if !package_versions.insert((leaf.package.as_str(), leaf.version.as_str())) {
                return Err(OvenRustcError::InvalidInput {
                    field: "artifact manifest registry catalog",
                    message: format!(
                        "declares package `{}` version `{}` more than once",
                        leaf.package, leaf.version
                    ),
                });
            }
            let mut features = BTreeSet::new();
            for feature in &leaf.features {
                if feature.trim().is_empty() || !features.insert(feature.as_str()) {
                    return Err(OvenRustcError::InvalidInput {
                        field: "artifact manifest registry catalog",
                        message: format!(
                            "registry leaf `{}` `{}` declares an empty or duplicate feature",
                            leaf.package, leaf.version
                        ),
                    });
                }
            }
            if Path::new(&leaf.artifact.relative_path)
                .extension()
                .and_then(|extension| extension.to_str())
                != Some("rlib")
            {
                return Err(OvenRustcError::InvalidInput {
                    field: "artifact manifest registry catalog",
                    message: format!(
                        "registry leaf `{}` `{}` must reference an rlib",
                        leaf.package, leaf.version
                    ),
                });
            }
            match declared_artifacts.get(leaf.artifact.relative_path.as_str()) {
                Some(digest) if *digest == leaf.artifact.digest.as_str() => {}
                Some(_) => {
                    return Err(OvenRustcError::InvalidInput {
                        field: "artifact manifest registry catalog",
                        message: format!(
                            "registry leaf `{}` `{}` has a digest that disagrees with its sealed artifact",
                            leaf.package, leaf.version
                        ),
                    });
                }
                None => {
                    return Err(OvenRustcError::InvalidInput {
                        field: "artifact manifest registry catalog",
                        message: format!(
                            "registry leaf `{}` `{}` references undeclared artifact `{}`",
                            leaf.package, leaf.version, leaf.artifact.relative_path
                        ),
                    });
                }
            }
            let source_key = (
                leaf.package.as_str(),
                leaf.version.as_str(),
                leaf.source.registry.as_str(),
                leaf.source.checksum.as_str(),
            );
            let Some(source) = registry_source_identities.get(&source_key) else {
                return Err(OvenRustcError::InvalidInput {
                    field: "artifact manifest registry catalog",
                    message: format!(
                        "registry leaf `{}` `{}` has no matching sealed inspection source",
                        leaf.package, leaf.version
                    ),
                });
            };
            let source_features = source.features.iter().map(String::as_str).collect::<BTreeSet<_>>();
            if source.source != leaf.source
                || leaf
                    .features
                    .iter()
                    .any(|feature| !source_features.contains(feature.as_str()))
            {
                return Err(OvenRustcError::InvalidInput {
                    field: "artifact manifest registry catalog",
                    message: format!(
                        "registry leaf `{}` `{}` disagrees with or exceeds its sealed inspection source",
                        leaf.package, leaf.version
                    ),
                });
            }
        }
        let mut auxiliary_targets = BTreeSet::new();
        for auxiliary in &self.vocab_auxiliary_targets {
            validate_rust_target(&auxiliary.target)?;
            if !auxiliary_targets.insert(auxiliary.target.as_str()) {
                return Err(OvenRustcError::InvalidInput {
                    field: "artifact manifest vocabulary auxiliary targets",
                    message: format!("declares target `{}` more than once", auxiliary.target),
                });
            }
            let mut auxiliary_names = BTreeSet::new();
            for artifact in &auxiliary.externs {
                validate_rust_identifier(&artifact.crate_name)?;
                if !auxiliary_names.insert(artifact.crate_name.as_str()) {
                    return Err(OvenRustcError::InvalidInput {
                        field: "artifact manifest vocabulary auxiliary targets",
                        message: format!(
                            "target `{}` declares duplicate extern crate `{}`",
                            auxiliary.target, artifact.crate_name
                        ),
                    });
                }
            }
        }
        Ok(())
    }

    /// Expose only this plan's copied, digest-verified registry catalog to a direct-rustc consumer.
    ///
    /// A normal command must not aggregate leaves from a different compiler Loaf: Rust metadata can bind one direct
    /// crate to a particular feature-unified dependency graph even when the package/version names look identical.
    pub(crate) fn registry_leaf_authority(
        &self,
        artifact_root: &Path,
        plan: &OvenRustcArtifactPlan,
    ) -> Option<OvenRegistryLeafAuthority> {
        (!self.registry_leaves.is_empty()).then(|| {
            OvenRegistryLeafAuthority::new_with_trusted_dependency_search_paths(
                artifact_root.to_path_buf(),
                self.registry_leaves.clone(),
                plan.dependency_search_paths.clone(),
            )
        })
    }
}

/// Normalize a Cargo package name for root-registry comparison; Cargo exposes `-` and `_` interchangeably.
fn normalized_package_name(name: &str) -> String {
    name.replace('-', "_")
}

/// Return whether one declared artifact is directly below a declared search directory.
fn artifact_is_below_search_path(relative_path: &str, search_path: &str) -> bool {
    Path::new(relative_path)
        .strip_prefix(Path::new(search_path))
        .is_ok_and(|suffix| !suffix.as_os_str().is_empty())
}

/// Validate a publisher's declared search-path shape before atomic copying.
///
/// Cargo target directories contain object and dep-info files that Oven intentionally does not retain. The eventual
/// store entry is still checked for exact directory completeness by `materialize`; this pre-copy check only confirms
/// that each declared directory is safe, unique, and owns at least one manifest-recorded artifact.
fn validate_publisher_search_paths(
    paths: &[String],
    kind: &'static str,
    expected: &BTreeMap<String, String>,
) -> Result<(), OvenRustcError> {
    let mut seen = BTreeSet::new();
    for relative in paths {
        let normalized = normalized_relative_path(relative, kind)?;
        if !seen.insert(normalized.clone()) {
            return Err(OvenRustcError::InvalidInput {
                field: "artifact manifest",
                message: format!("declares duplicate {kind} path `{relative}`"),
            });
        }
        let prefix = format!("{normalized}/");
        if !expected.keys().any(|artifact| artifact.starts_with(&prefix)) {
            return Err(OvenRustcError::InvalidInput {
                field: "artifact manifest",
                message: format!("declares {kind} path `{relative}` without a recorded artifact"),
            });
        }
    }
    Ok(())
}

/// Validate the small compile-time environment envelope that direct-rustc consumers may restore after ambient Cargo
/// state is cleared.
fn validated_compile_environment(
    environment: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, String>, OvenRustcError> {
    for (name, value) in environment {
        let binary_name = name.strip_prefix("CARGO_BIN_EXE_");
        let allowed = name == "CARGO_MANIFEST_DIR"
            || name.starts_with("CARGO_PKG_")
            || binary_name.is_some_and(|name| !name.is_empty());
        if !allowed {
            return Err(OvenRustcError::InvalidInput {
                field: "artifact manifest compile environment",
                message: format!("does not permit `{name}`"),
            });
        }
        if value.is_empty() || value.contains('\0') {
            return Err(OvenRustcError::InvalidInput {
                field: "artifact manifest compile environment",
                message: format!("has an invalid value for `{name}`"),
            });
        }
        if binary_name.is_some() && !Path::new(value).is_absolute() {
            return Err(OvenRustcError::InvalidInput {
                field: "artifact manifest compile environment",
                message: format!("`{name}` must name an absolute caller-owned binary output"),
            });
        }
        if value == "@oven-source-root" || value.starts_with("@oven-source-ancestor:") {
            if name != "CARGO_MANIFEST_DIR" {
                return Err(OvenRustcError::InvalidInput {
                    field: "artifact manifest compile environment",
                    message: "source-relative tokens are permitted only for CARGO_MANIFEST_DIR".to_string(),
                });
            }
            if value.starts_with("@oven-source-ancestor:") {
                let Some(distance) = value
                    .strip_prefix("@oven-source-ancestor:")
                    .and_then(|value| value.parse::<usize>().ok())
                else {
                    return Err(OvenRustcError::InvalidInput {
                        field: "artifact manifest compile environment",
                        message: "source-relative ancestor token must end in a positive integer".to_string(),
                    });
                };
                if !(1..=16).contains(&distance) {
                    return Err(OvenRustcError::InvalidInput {
                        field: "artifact manifest compile environment",
                        message: "source-relative ancestor distance must be between 1 and 16".to_string(),
                    });
                }
            }
        }
    }
    Ok(environment.clone())
}

/// Resolve a portable caller-relative compile environment token after the source has been receipt-authorized.
pub(crate) fn resolve_compile_environment_value(
    name: &str,
    value: &str,
    source: &Path,
) -> Result<PathBuf, OvenRustcError> {
    let distance = if value == "@oven-source-root" {
        2
    } else if let Some(distance) = value
        .strip_prefix("@oven-source-ancestor:")
        .and_then(|value| value.parse::<usize>().ok())
    {
        distance
    } else {
        return Ok(PathBuf::from(value));
    };
    let mut ancestor = source.parent().ok_or_else(|| OvenRustcError::InvalidInput {
        field: "source",
        message: format!("{} has no parent directory for {name}", source.display()),
    })?;
    for _ in 1..distance {
        ancestor = ancestor.parent().ok_or_else(|| OvenRustcError::InvalidInput {
            field: "source",
            message: format!(
                "{} cannot derive source ancestor {distance} for {name}",
                source.display()
            ),
        })?;
    }
    Ok(ancestor.to_path_buf())
}

/// Select a receipt-bound direct-rustc closure and retain its lease until the caller finishes execution.
pub fn bake_stored_direct_rustc_test(
    request: &OvenStoredDirectRustcTestRequest<'_>,
) -> Result<OvenDirectRustcTestBake, OvenRustcError> {
    bake_stored_direct_rustc_test_with_libraries(request, &[])
}

/// Select a receipt-bound direct-rustc closure and compile a native test while linking caller-owned direct Rust
/// libraries.
///
/// The supplemental libraries are generated only by the path-only Oven materializer. They are caller output rather
/// than store artifacts, but their bytes enter the direct output reuse identity through the same checked attachment
/// mechanism used for materialized Incan library dependencies.
pub(crate) fn bake_stored_direct_rustc_test_with_libraries(
    request: &OvenStoredDirectRustcTestRequest<'_>,
    caller_owned_libraries: &[OvenCallerOwnedRustcLibrary],
) -> Result<OvenDirectRustcTestBake, OvenRustcError> {
    let (artifacts, artifact_root, lease) =
        select_stored_direct_rustc_plan(request.store, &request.plan_identity, &request.receipt)?;
    let mut artifact_plan = artifacts.materialize_trusted_store(&artifact_root, &request.receipt.intent)?;
    attach_caller_owned_rustc_libraries(&mut artifact_plan, caller_owned_libraries)?;
    let mut bake = bake_direct_rustc(
        &request.receipt,
        &artifacts,
        &artifact_root,
        &request.rustc,
        &request.source,
        &request.output,
        &request.crate_name,
        &request.edition,
        &request.source_evidence_key,
        &request.source_evidence_key,
        true,
        OvenDirectRustcOutputKind::Binary,
        true,
        Some(&artifact_plan),
        false,
        &request.receipt.intent.features,
    )?;
    bake.lease = Some(lease);
    Ok(bake)
}

/// Select a receipt-bound direct-rustc closure and retain its lease until the caller finishes running the binary.
pub fn bake_stored_direct_rustc_run(
    request: &OvenStoredDirectRustcRunRequest<'_>,
) -> Result<OvenDirectRustcBake, OvenRustcError> {
    bake_stored_direct_rustc_run_with_libraries(request, &[])
}

/// Select a receipt-bound direct-rustc closure and compile a binary while linking explicit caller-owned libraries.
///
/// The additional libraries are not stored artifacts and never participate in native-plan selection. They are the
/// caller-output bridge for already materialized Incan `pub::` dependencies; their exact bytes are recorded in the
/// consumer output sidecar before reuse is allowed.
pub(crate) fn bake_stored_direct_rustc_run_with_libraries(
    request: &OvenStoredDirectRustcRunRequest<'_>,
    caller_owned_libraries: &[OvenCallerOwnedRustcLibrary],
) -> Result<OvenDirectRustcBake, OvenRustcError> {
    let (artifacts, artifact_root, lease) =
        select_stored_direct_rustc_plan(request.store, &request.plan_identity, &request.receipt)?;
    let mut artifact_plan = artifacts.materialize_trusted_store(&artifact_root, &request.receipt.intent)?;
    attach_caller_owned_rustc_libraries(&mut artifact_plan, caller_owned_libraries)?;
    let mut bake = bake_direct_rustc(
        &request.receipt,
        &artifacts,
        &artifact_root,
        &request.rustc,
        &request.source,
        &request.output,
        &request.crate_name,
        &request.edition,
        &request.source_evidence_key,
        &request.source_evidence_key,
        false,
        OvenDirectRustcOutputKind::Binary,
        true,
        Some(&artifact_plan),
        false,
        &request.receipt.intent.features,
    )?;
    bake.lease = Some(lease);
    Ok(bake)
}

/// Select a receipt-bound direct-rustc closure and compile one regular caller-owned Rust library without a Cargo
/// consumer process.
pub fn bake_stored_direct_rustc_library(
    request: &OvenStoredDirectRustcLibraryRequest<'_>,
) -> Result<OvenDirectRustcBake, OvenRustcError> {
    bake_stored_direct_rustc_library_with_libraries(request, &[])
}

/// Select a receipt-bound direct-rustc closure and compile a library while linking explicit caller-owned libraries.
pub(crate) fn bake_stored_direct_rustc_library_with_libraries(
    request: &OvenStoredDirectRustcLibraryRequest<'_>,
    caller_owned_libraries: &[OvenCallerOwnedRustcLibrary],
) -> Result<OvenDirectRustcBake, OvenRustcError> {
    let (artifacts, artifact_root, lease) =
        select_stored_direct_rustc_plan(request.store, &request.plan_identity, &request.receipt)?;
    let mut artifact_plan = artifacts.materialize_trusted_store(&artifact_root, &request.receipt.intent)?;
    attach_caller_owned_rustc_libraries(&mut artifact_plan, caller_owned_libraries)?;
    let mut bake = bake_direct_rustc(
        &request.receipt,
        &artifacts,
        &artifact_root,
        &request.rustc,
        &request.source,
        &request.output,
        &request.crate_name,
        &request.edition,
        &request.source_evidence_key,
        &request.source_evidence_key,
        false,
        OvenDirectRustcOutputKind::Library,
        true,
        Some(&artifact_plan),
        false,
        &request.receipt.intent.features,
    )?;
    bake.lease = Some(lease);
    Ok(bake)
}

/// Select the unique stored direct-rustc plan authorized by a generated-project receipt and retain its lease.
///
/// Normal Oven commands select through the receipt's reusable build-unit identity rather than accepting a
/// caller-provided cache location or artifact identity. Generated source remains verified independently at execution,
/// so compatible clean worktrees can reuse one native closure without sharing source or final-output directories.
/// Distinct plans remain an explicit publisher error: silently choosing a "latest" plan would make normal command
/// execution non-deterministic. Byte-identical closures from compatible receipts are equivalent reusable entries;
/// those are collapsed by stable identity while future publication deduplicates them at admission. Matching,
/// integrity verification, and lease acquisition occur under one store-manager lock so policy pruning cannot reclaim
/// a compatible candidate after header selection but before execution begins.
pub fn select_direct_rustc_plan_for_execution(
    store: &OvenStore,
    receipt: &OvenReceipt,
) -> Result<Option<OvenStoreExecutionPayload>, OvenRustcError> {
    receipt
        .verify_identity()
        .map_err(|error| OvenRustcError::InvalidInput {
            field: "receipt",
            message: error.to_string(),
        })?;
    let mut matches = Vec::new();
    for selected in store.select_payloads_matching_for_execution(|manifest| {
        manifest.kind == OvenArtifactKind::DirectRustcPlan
            && manifest.build_unit_identity == receipt.build_unit_identity
            && manifest.intent == receipt.intent
    })? {
        let Ok(plan) = serde_json::from_slice::<OvenRustcArtifactManifest>(&selected.payload) else {
            continue;
        };
        // A prior Alpha publisher may have retained a payload whose `--extern` identifiers no longer satisfy the
        // stricter direct-rustc contract. Ignore it for selection so a corrected explicit publication can coexist
        // until ordinary policy-driven pruning reclaims the inactive entry.
        if plan.validate_shape(&receipt.intent).is_ok() {
            matches.push(selected);
        }
    }
    match matches.len() {
        1 => Ok(matches.pop()),
        0 => Ok(None),
        _ if matches
            .iter()
            .skip(1)
            .all(|candidate| reusable_direct_rustc_entry(&matches[0], candidate)) =>
        {
            matches.sort_by(|left, right| left.manifest.identity.cmp(&right.manifest.identity));
            Ok(Some(matches.remove(0)))
        }
        _ => Err(OvenRustcError::PlanSelection {
            receipt_identity: receipt.identity.clone(),
            message: format!(
                "multiple compatible stored direct-rustc plans are available: {}",
                matches
                    .iter()
                    .map(|entry| entry.manifest.identity.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        }),
    }
}

/// Return whether two receipt-compatible selections retain precisely the same reusable immutable closure.
fn reusable_direct_rustc_entry(left: &OvenStoreExecutionPayload, right: &OvenStoreExecutionPayload) -> bool {
    left.manifest.domain == right.manifest.domain
        && left.manifest.payload == right.manifest.payload
        && left.manifest.materialized_files == right.manifest.materialized_files
}

/// Select the unique stored direct-rustc plan identity authorized by a generated-project receipt.
///
/// This compatibility helper drops the returned execution lease with the identity. Normal command execution must
/// use [`select_direct_rustc_plan_for_execution`] so a concurrent bounded-policy publication cannot prune a chosen
/// plan before the caller starts using it.
pub fn select_direct_rustc_plan_identity(store: &OvenStore, receipt: &OvenReceipt) -> Result<String, OvenRustcError> {
    select_direct_rustc_plan_for_execution(store, receipt)?
        .map(|selected| selected.manifest.identity)
        .ok_or_else(|| OvenRustcError::PlanSelection {
            receipt_identity: receipt.identity.clone(),
            message: "no compatible stored direct-rustc plan is available".to_string(),
        })
}

/// Minimal project-inspection header decoded before any version-specific authority fields.
#[derive(Deserialize)]
struct OvenProjectInspectionAuthorityHeader {
    schema_version: u32,
}

/// Reject an unknown project-inspection wire version before decoding its version-specific body.
fn preflight_project_inspection_authority_schema(identity: &str, bytes: &[u8]) -> Result<(), OvenRustcError> {
    let header = serde_json::from_slice::<OvenProjectInspectionAuthorityHeader>(bytes).map_err(|error| {
        OvenRustcError::InvalidStoredPlan {
            identity: identity.to_string(),
            message: format!("payload has no valid project inspection authority header: {error}"),
        }
    })?;
    if header.schema_version != OVEN_PROJECT_INSPECTION_AUTHORITY_SCHEMA_VERSION {
        return Err(OvenRustcError::UnsupportedProjectInspectionAuthoritySchema {
            found: header.schema_version,
            expected: OVEN_PROJECT_INSPECTION_AUTHORITY_SCHEMA_VERSION,
        });
    }
    Ok(())
}

/// Decode one current project inspection authority after its schema and physical owner have been validated.
fn decode_project_inspection_authority(
    identity: &str,
    bytes: &[u8],
) -> Result<OvenProjectInspectionAuthorityPayload, OvenRustcError> {
    serde_json::from_slice::<OvenProjectInspectionAuthorityPayload>(bytes).map_err(|error| {
        OvenRustcError::InvalidStoredPlan {
            identity: identity.to_string(),
            message: format!("payload is not a project inspection authority: {error}"),
        }
    })
}

/// Load the one project inspection authority named by a source-current completed output.
///
/// Selection never searches by dependency compatibility. The authority entry is exact, and all of its store-owned
/// constituents are acquired in one batch before any source is projected. Release-Loaf constituents are resolved
/// separately by the caller against the active immutable toolchain generation.
pub(crate) fn load_project_inspection_authority(
    store: &OvenStore,
    authority_ref: &OvenProjectInspectionAuthorityRef,
    project_identity: &str,
    source_authority_digest: &str,
    compiler_version: &str,
) -> Result<OvenLoadedProjectInspectionAuthority, OvenRustcError> {
    let mut selected = store
        .select_payloads_for_execution(std::slice::from_ref(&authority_ref.identity))
        .map_err(|error| OvenRustcError::PlanSelection {
            receipt_identity: authority_ref.receipt_identity.clone(),
            message: format!(
                "source-current project output cannot select exact project inspection authority `{}`: {error}",
                authority_ref.identity
            ),
        })?;
    let source_owner = selected.pop().ok_or_else(|| OvenRustcError::PlanSelection {
        receipt_identity: authority_ref.receipt_identity.clone(),
        message: format!(
            "source-current project output could not retain exact project inspection authority `{}`",
            authority_ref.identity
        ),
    })?;
    let manifest = &source_owner.manifest;
    if manifest.identity != authority_ref.identity
        || manifest.kind != OvenArtifactKind::ProjectInspectionAuthority
        || manifest.receipt_identity != authority_ref.receipt_identity
        || manifest.build_unit_identity != authority_ref.build_unit_identity
    {
        return Err(OvenRustcError::InvalidStoredPlan {
            identity: manifest.identity.clone(),
            message: "project inspection authority differs from its exact kind, receipt, or build-unit reference"
                .to_string(),
        });
    }
    preflight_project_inspection_authority_schema(&manifest.identity, &source_owner.payload)?;
    source_owner.verify_materialized_files()?;
    let payload = decode_project_inspection_authority(&manifest.identity, &source_owner.payload)?;
    validate_project_inspection_authority_payload(&payload)?;
    if payload.project_identity != project_identity
        || payload.source_authority_digest != source_authority_digest
        || payload.compiler_version != compiler_version
    {
        return Err(OvenRustcError::InvalidStoredPlan {
            identity: manifest.identity.clone(),
            message: "project inspection authority does not match the selected output's project, source, or compiler evidence"
                .to_string(),
        });
    }
    if !payload.registry_sources.is_empty() {
        let lock = source_owner.artifact_root.join(OVEN_RUSTC_REGISTRY_LOCK_RELATIVE_PATH);
        let lock_bytes = fs::read(&lock).map_err(|source| OvenRustcError::Io {
            path: lock.clone(),
            source,
        })?;
        let actual = digest_bytes(&lock_bytes);
        if actual != payload.registry_lock_digest {
            return Err(OvenRustcError::ArtifactDigestMismatch {
                path: lock,
                expected: payload.registry_lock_digest.clone(),
                actual,
            });
        }
    }

    let stored_refs = payload
        .constituents
        .iter()
        .filter_map(|constituent| match constituent {
            OvenProjectInspectionConstituent::Stored { identity, .. } => Some(identity.clone()),
            OvenProjectInspectionConstituent::ReleaseLoaf { .. } => None,
        })
        .collect::<Vec<_>>();
    let stored_constituents = if stored_refs.is_empty() {
        Vec::new()
    } else {
        store.select_payloads_for_execution(&stored_refs)?
    };
    let mut selected_index = 0;
    for constituent in &payload.constituents {
        let OvenProjectInspectionConstituent::Stored {
            identity,
            artifact_kind,
            receipt,
            base_loaf_identity,
        } = constituent
        else {
            continue;
        };
        let selected = stored_constituents
            .get(selected_index)
            .ok_or_else(|| OvenRustcError::PlanSelection {
                receipt_identity: receipt.identity.clone(),
                message: format!("project inspection authority lost constituent `{identity}` during batch selection"),
            })?;
        selected_index += 1;
        if selected.manifest.identity != *identity
            || selected.manifest.kind != *artifact_kind
            || !project_inspection_constituent_matches_receipt(&selected.manifest, *artifact_kind, receipt)
        {
            return Err(OvenRustcError::InvalidStoredPlan {
                identity: identity.clone(),
                message: "project inspection constituent differs from its sealed identity, receipt, kind, or intent"
                    .to_string(),
            });
        }
        match artifact_kind {
            OvenArtifactKind::DirectRustcPlan => {
                let plan = serde_json::from_slice::<OvenRustcArtifactManifest>(&selected.payload).map_err(|error| {
                    OvenRustcError::InvalidStoredPlan {
                        identity: identity.clone(),
                        message: format!("direct-plan constituent payload is invalid: {error}"),
                    }
                })?;
                plan.validate_shape(&receipt.intent)?;
            }
            OvenArtifactKind::ProjectPayload => {
                let extension =
                    serde_json::from_slice::<OvenProjectExtensionPayload>(&selected.payload).map_err(|error| {
                        OvenRustcError::InvalidStoredPlan {
                            identity: identity.clone(),
                            message: format!("project-extension constituent payload is invalid: {error}"),
                        }
                    })?;
                validate_project_extension_payload_shape(&extension, &receipt.intent)?;
                let base_build_unit_matches = payload.constituents.iter().any(|candidate| {
                    matches!(
                        candidate,
                        OvenProjectInspectionConstituent::ReleaseLoaf {
                            loaf_identity,
                            build_unit_identity,
                            ..
                        } if loaf_identity == &extension.base_loaf_identity
                            && build_unit_identity == &extension.base_build_unit_identity
                    )
                });
                if base_loaf_identity.as_deref() != Some(extension.base_loaf_identity.as_str())
                    || !base_build_unit_matches
                {
                    return Err(OvenRustcError::InvalidStoredPlan {
                        identity: identity.clone(),
                        message: "project-extension constituent has different release-Loaf or build-unit evidence"
                            .to_string(),
                    });
                }
            }
            unsupported => {
                return Err(OvenRustcError::InvalidStoredPlan {
                    identity: identity.clone(),
                    message: format!("unsupported project inspection constituent kind {unsupported:?}"),
                });
            }
        }
    }
    Ok(OvenLoadedProjectInspectionAuthority::new(
        source_owner,
        payload,
        stored_constituents,
    ))
}

/// Return whether one stored inspection constituent is authorized by its receiving project receipt.
///
/// Direct-Rustc plans are shared immutable closures, so their original publisher receipt may differ from the
/// receiving project receipt while the reusable build unit and build intent remain exact. Project extensions remain
/// receipt-specific because they contain caller-owned project material.
pub(crate) fn project_inspection_constituent_matches_receipt(
    manifest: &OvenArtifactManifest,
    artifact_kind: OvenArtifactKind,
    receipt: &OvenReceipt,
) -> bool {
    manifest.build_unit_identity == receipt.build_unit_identity
        && manifest.intent == receipt.intent
        && (artifact_kind == OvenArtifactKind::DirectRustcPlan || manifest.receipt_identity == receipt.identity)
}

/// Check a generated inspection batch against the exact normal/dev roots sealed by one project authority.
pub(crate) fn project_inspection_authority_supports_dependencies(
    payload: &OvenProjectInspectionAuthorityPayload,
    dependencies: &[DependencySpec],
) -> bool {
    if validate_project_inspection_authority_payload(payload).is_err() {
        return false;
    }
    let catalog = payload
        .registry_sources
        .iter()
        .map(|source| &source.package)
        .collect::<Vec<_>>();
    let mut aliases = BTreeSet::new();
    dependencies
        .iter()
        .filter(|dependency| matches!(dependency.source, DependencySource::Registry))
        .all(|dependency| {
            let alias = dependency.crate_name.replace('-', "_");
            if !aliases.insert(alias.clone()) {
                return false;
            }
            let Some(requirement) = dependency
                .version
                .as_deref()
                .and_then(|version| VersionReq::parse(version).ok())
            else {
                return false;
            };
            let package = dependency.package.as_deref().unwrap_or(&dependency.crate_name);
            let requested_features = {
                let mut features = dependency.features.clone();
                features.sort();
                features.dedup();
                features
            };
            let matches = payload
                .registry_source_dependencies
                .iter()
                .chain(&payload.dev_registry_source_dependencies)
                .filter(|record| {
                    record.alias == alias
                        && record.package == package
                        && Version::parse(&record.version).is_ok_and(|version| requirement.matches(&version))
                        && record.requested_features == requested_features
                        && record.default_features == dependency.default_features
                        && catalog.iter().any(|source| {
                            source.package == record.package
                                && source.version == record.version
                                && source.source.registry == record.registry
                                && source.source.checksum == record.checksum
                        })
                })
                .collect::<Vec<_>>();
            matches.len() == 1 || matches.len() == 2 && matches[0] == matches[1]
        })
}

/// Check one generated native-test batch against the exact per-root dependency evidence in its singular authority.
///
/// The caller canonicalizes normal/dev duplicates first. This function then admits only a true subset: every named
/// alias must retain the same package/source/version/features/defaults, and path roots must still hash to the sealed
/// source identity. It never searches another Loaf when one root is absent or stale.
pub(crate) fn project_inspection_test_dependency_envelope_supports_dependencies(
    payload: &OvenProjectInspectionAuthorityPayload,
    dependencies: &[DependencySpec],
) -> Result<bool, OvenRustcError> {
    validate_project_inspection_authority_payload(payload)?;
    let Some(envelope) = payload.test_dependency_envelope.as_ref() else {
        return Ok(false);
    };
    let mut aliases = BTreeSet::new();
    for dependency in dependencies {
        let alias = dependency.crate_name.replace('-', "_");
        if !aliases.insert(alias.clone()) {
            return Ok(false);
        }
        let Some(root) = envelope.dependency_roots.get(&alias) else {
            return Ok(false);
        };
        let actual = crate::oven::digest_dependency_specs(std::slice::from_ref(dependency)).map_err(|error| {
            OvenRustcError::InvalidInput {
                field: "project inspection test dependency root",
                message: error.to_string(),
            }
        })?;
        let (expected, source_matches) = match root {
            OvenProjectInspectionTestDependencyRoot::Registry { dependency_digest, .. } => (
                dependency_digest,
                matches!(dependency.source, DependencySource::Registry),
            ),
            OvenProjectInspectionTestDependencyRoot::Path { dependency_digest } => (
                dependency_digest,
                matches!(dependency.source, DependencySource::Path { .. }),
            ),
            OvenProjectInspectionTestDependencyRoot::Git { dependency_digest } => (
                dependency_digest,
                matches!(dependency.source, DependencySource::Git { .. }),
            ),
        };
        if !source_matches || actual != *expected {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Check whether one sealed registry-source catalog covers every selected direct registry dependency.
///
/// This is shared by compiler-shipped and project-owned Loaf selection. It intentionally validates sources only;
/// callers still need a separately selected receipt-bound plan before linking any artifact.
pub(crate) fn registry_source_dependencies_supported_by_catalog(
    sources: &[OvenRustcRegistrySourcePackage],
    dependencies: &[&DependencySpec],
) -> bool {
    dependencies.iter().all(|dependency| {
        let Some(requirement) = dependency
            .version
            .as_deref()
            .and_then(|version| VersionReq::parse(version).ok())
        else {
            return false;
        };
        let package = dependency.package.as_deref().unwrap_or(&dependency.crate_name);
        let required_features = dependency.features.iter().map(String::as_str).collect::<BTreeSet<_>>();
        let matching = sources
            .iter()
            .filter(|source| {
                source.package == package
                    && Version::parse(&source.version).is_ok_and(|version| requirement.matches(&version))
                    && required_features
                        .iter()
                        .all(|feature| source.features.iter().any(|selected| selected == *feature))
            })
            .count();
        matching == 1
    })
}

/// Select a stored plan only when it matches the requested reusable build unit and return its closure with a live
/// lease.
fn select_stored_direct_rustc_plan(
    store: &OvenStore,
    plan_identity: &str,
    receipt: &OvenReceipt,
) -> Result<(OvenRustcArtifactManifest, PathBuf, OvenStoreLease), OvenRustcError> {
    receipt
        .verify_identity()
        .map_err(|error| OvenRustcError::InvalidInput {
            field: "receipt",
            message: error.to_string(),
        })?;
    let (manifest, artifact_root, payload, lease) = store.select_payload_for_execution(plan_identity)?;
    if manifest.kind != OvenArtifactKind::DirectRustcPlan {
        return Err(OvenRustcError::InvalidStoredPlan {
            identity: manifest.identity,
            message: "selected artifact kind is not direct_rustc_plan".to_string(),
        });
    }
    if manifest.build_unit_identity != receipt.build_unit_identity || manifest.intent != receipt.intent {
        return Err(OvenRustcError::InvalidStoredPlan {
            identity: manifest.identity,
            message: "selected artifact is not authorized by the requested Oven build unit".to_string(),
        });
    }
    let artifacts = serde_json::from_slice::<OvenRustcArtifactManifest>(&payload).map_err(|error| {
        OvenRustcError::InvalidStoredPlan {
            identity: plan_identity.to_string(),
            message: format!("stored payload is not an Oven Rust artifact manifest: {error}"),
        }
    })?;
    Ok((artifacts, artifact_root, lease))
}

/// Run one compiler-suite doctest root directly with Rustdoc, without a Cargo consumer process.
///
/// Rustdoc creates and destroys the individual doctest executables internally. Oven therefore verifies the same
/// receipt-bound source and immutable closure as direct-rustc, pins Rustdoc to the selected Rustc sysroot, and keeps
/// all transient files in a caller-owned directory while the enclosing suite lease remains live.
pub(crate) fn run_trusted_rustdoc_test(
    request: &OvenTrustedRustdocTestRequest<'_>,
) -> Result<OvenRustdocTestReport, OvenRustcError> {
    request
        .receipt
        .verify_identity()
        .map_err(|error| OvenRustcError::InvalidInput {
            field: "receipt",
            message: error.to_string(),
        })?;
    verify_rustc_identity(request.rustc, &request.receipt.intent.toolchain)?;
    validate_rust_identifier(request.crate_name)?;
    validate_edition(request.edition)?;
    let source = verified_regular_file(request.source, "source")?;
    let source_bytes = fs::read(&source).map_err(|source_error| OvenRustcError::Io {
        path: source.clone(),
        source: source_error,
    })?;
    let source_digest = digest_bytes(&source_bytes);
    let expected_source_digest = request
        .receipt
        .sources
        .supplemental_digests
        .get(request.source_evidence_key.trim())
        .ok_or_else(|| OvenRustcError::InvalidInput {
            field: "source evidence",
            message: format!("receipt does not declare `{}`", request.source_evidence_key),
        })?;
    if expected_source_digest != &source_digest {
        return Err(OvenRustcError::SourceEvidenceMismatch {
            key: request.source_evidence_key.to_string(),
            expected: expected_source_digest.clone(),
            actual: source_digest,
        });
    }
    let artifacts = request.artifacts.for_source_evidence(request.source_evidence_key)?;
    // `Option::unwrap_or` evaluates its fallback eagerly.  Indexed compiler-suite doctests provide a composed
    // foundation plan while their thin shard deliberately has no local third-party closure, so touching that
    // fallback would fail before Rustdoc can use the supplied plan.  Keep legacy single-root materialization lazy.
    let plan = match request.artifact_plan {
        Some(plan) => plan.clone(),
        None => artifacts.materialize_trusted_store(request.artifact_root, &request.receipt.intent)?,
    };
    let temporary_directory = caller_temporary_directory(request.temporary_directory, request.artifact_root)?;
    let test_run_directory = match plan.compile_environment.get("CARGO_MANIFEST_DIR") {
        Some(value) => resolve_compile_environment_value("CARGO_MANIFEST_DIR", value, &source)?,
        None => source
            .parent()
            .map(Path::to_path_buf)
            .ok_or_else(|| OvenRustcError::InvalidInput {
                field: "source",
                message: format!("{} has no parent directory for Rustdoc", source.display()),
            })?,
    };
    let rustdoc = rustdoc_for_rustc(request.rustc)?;
    let mut command = Command::new(&rustdoc);
    command
        .arg("--test")
        .arg("--target")
        .arg(&request.receipt.intent.target)
        .arg(format!("--edition={}", request.edition))
        .arg("--crate-name")
        .arg(request.crate_name)
        .arg("--test-run-directory")
        .arg(&test_run_directory)
        .arg(&source);
    apply_oven_profile(&mut command, &request.receipt.intent.profile);
    if request.is_proc_macro {
        // Cargo's proc-macro Rustdoc invocation supplies both the crate type and the sysroot-provided
        // `proc_macro` extern. Without the latter, Rustdoc treats the source as an ordinary library and rejects
        // `use proc_macro::…` even though the root is receipt-classified as a proc macro.
        command
            .arg("--crate-type")
            .arg("proc-macro")
            .arg("--extern")
            .arg("proc_macro");
    }
    command.current_dir(&test_run_directory);
    clear_inherited_cargo_environment(&mut command);
    command.env("TMPDIR", &temporary_directory);
    for (name, value) in &plan.compile_environment {
        let value = resolve_compile_environment_value(name, value, &source)?;
        command.env(name, value);
    }
    if request.prefer_dynamic {
        // Rustdoc launches the generated doctest runner itself. That runner can link a caller-owned workspace dylib
        // such as the compiler library, so the selected toolchain libraries alone are not a complete runtime
        // closure. Carry only caller-owned dynamic-library directories: immutable store identities contain `sha256:`
        // and must remain direct `-L` compiler inputs rather than ambiguous path-list environment segments.
        let (name, value) = rustc_dynamic_library_environment_with_caller_owned_paths(request.rustc, &plan)?;
        command.args(["-C", "rpath"]);
        command.env(name, value);
    }
    for feature in request.features {
        command.arg("--cfg").arg(format!("feature={feature:?}"));
    }
    for path in &plan.dependency_search_paths {
        command.arg("-L").arg(format!("dependency={}", path.display()));
    }
    for path in &plan.native_search_paths {
        command.arg("-L").arg(format!("native={}", path.display()));
    }
    append_native_runtime_rpaths(&mut command, &request.receipt.intent.target, &plan.native_search_paths);
    for (crate_name, path) in &plan.externs {
        command.arg("--extern").arg(format!("{crate_name}={}", path.display()));
    }
    let (output, timed_out) = run_supervised_rustdoc_command(command, &rustdoc, request.timeout)?;
    let mut transcript = combined_process_output(&output.stdout, &output.stderr);
    if timed_out {
        if !transcript.ends_with('\n') && !transcript.is_empty() {
            transcript.push('\n');
        }
        if let Some(timeout) = request.timeout {
            transcript.push_str(&format!(
                "Oven Rustdoc execution group timed out after {}ms (source: {})\n",
                timeout.as_millis(),
                source.display(),
            ));
        }
    }
    if timed_out || !output.status.success() {
        return Err(OvenRustcError::RustdocTestFailed { output: transcript });
    }
    Ok(OvenRustdocTestReport { output: transcript })
}

/// Run Rustdoc and its generated doctest children inside the same bounded process group as native suite roots.
fn run_supervised_rustdoc_command(
    mut command: Command,
    rustdoc: &Path,
    timeout: Option<Duration>,
) -> Result<(std::process::Output, bool), OvenRustcError> {
    let Some(timeout) = timeout else {
        let output = command.output().map_err(|source| OvenRustcError::Io {
            path: rustdoc.to_path_buf(),
            source,
        })?;
        return Ok((output, false));
    };

    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    isolate_process_group(&mut command);
    let mut child = command.spawn().map_err(|source| OvenRustcError::Io {
        path: rustdoc.to_path_buf(),
        source,
    })?;
    let mut stdout = child.stdout.take().ok_or_else(|| OvenRustcError::Io {
        path: rustdoc.to_path_buf(),
        source: io::Error::other("Rustdoc stdout was not piped"),
    })?;
    let mut stderr = child.stderr.take().ok_or_else(|| OvenRustcError::Io {
        path: rustdoc.to_path_buf(),
        source: io::Error::other("Rustdoc stderr was not piped"),
    })?;
    let stdout_reader = thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout.read_to_end(&mut bytes)?;
        Ok::<_, io::Error>(bytes)
    });
    let stderr_reader = thread::spawn(move || {
        let mut bytes = Vec::new();
        stderr.read_to_end(&mut bytes)?;
        Ok::<_, io::Error>(bytes)
    });
    let deadline = Instant::now() + timeout;
    let mut timed_out = false;
    let status = loop {
        match child.try_wait().map_err(|source| OvenRustcError::Io {
            path: rustdoc.to_path_buf(),
            source,
        })? {
            Some(status) => break status,
            None if Instant::now() >= deadline => {
                timed_out = true;
                break terminate_process_group(&mut child).map_err(|source| OvenRustcError::Io {
                    path: rustdoc.to_path_buf(),
                    source,
                })?;
            }
            None => thread::sleep(Duration::from_millis(1)),
        }
    };
    let stdout = join_rustdoc_output_reader(stdout_reader, rustdoc, "stdout")?;
    let stderr = join_rustdoc_output_reader(stderr_reader, rustdoc, "stderr")?;
    Ok((std::process::Output { status, stdout, stderr }, timed_out))
}

/// Join one Rustdoc pipe reader and retain the executable path in any I/O diagnostic.
fn join_rustdoc_output_reader(
    reader: thread::JoinHandle<Result<Vec<u8>, io::Error>>,
    rustdoc: &Path,
    stream: &str,
) -> Result<Vec<u8>, OvenRustcError> {
    reader
        .join()
        .map_err(|_| OvenRustcError::Io {
            path: rustdoc.to_path_buf(),
            source: io::Error::other(format!("Rustdoc {stream} reader panicked")),
        })?
        .map_err(|source| OvenRustcError::Io {
            path: rustdoc.to_path_buf(),
            source,
        })
}

/// Compile one receipt-bound generated Rust test target without a Cargo consumer process.
pub fn bake_direct_rustc_test(request: &OvenDirectRustcTestRequest) -> Result<OvenDirectRustcTestBake, OvenRustcError> {
    bake_direct_rustc(
        &request.receipt,
        &request.artifacts,
        &request.artifact_root,
        &request.rustc,
        &request.source,
        &request.output,
        &request.crate_name,
        &request.edition,
        &request.source_evidence_key,
        &request.source_evidence_key,
        true,
        OvenDirectRustcOutputKind::Binary,
        false,
        None,
        false,
        &request.receipt.intent.features,
    )
}

/// Prepare one provider-owned Rust library from the exact admitted source role and physical native plan.
///
/// The result is deliberately output-agnostic: Incan first projects its candidate identity, after which the caller
/// chooses the corresponding candidate directory for either retained-output validation or a fresh compile.
pub(crate) fn prepare_trusted_direct_rustc_library_with_artifact_role(
    request: &OvenTrustedDirectRustcTargetRequest<'_>,
    artifact_role: &str,
    source_root: &Path,
    compiler: Option<&Arc<OvenOwnedDirectRustcCompiler>>,
) -> Result<OvenPreparedDirectRustcLibrary, OvenRustcError> {
    request
        .receipt
        .verify_identity()
        .map_err(|error| OvenRustcError::InvalidInput {
            field: "receipt",
            message: error.to_string(),
        })?;
    if compiler.is_none() {
        verify_rustc_identity(request.rustc, &request.receipt.intent.toolchain)?;
    }
    validate_rust_identifier(request.crate_name)?;
    validate_edition(request.edition)?;
    if request.prefer_dynamic {
        return Err(OvenRustcError::InvalidInput {
            field: "JEC library",
            message: "does not support a dynamic Rust standard-library binding".to_string(),
        });
    }
    if let Some(compiler) = compiler {
        let evidence = compiler.evidence();
        let request_rustc = fs::canonicalize(request.rustc).map_err(|source| OvenRustcError::Io {
            path: request.rustc.to_path_buf(),
            source,
        })?;
        if request_rustc != compiler.rustc
            || evidence.target != request.receipt.intent.target
            || compiler.toolchain() != request.receipt.intent.toolchain
        {
            return Err(OvenRustcError::InvalidInput {
                field: "JEC compiler evidence",
                message: "is detached from the retained receipt-selected compiler owner".to_string(),
            });
        }
    }
    let source_root = canonical_directory(source_root, "JEC source root")?;
    let source = fs::canonicalize(verified_regular_file(request.source, "source")?).map_err(|source_error| {
        OvenRustcError::Io {
            path: request.source.to_path_buf(),
            source: source_error,
        }
    })?;
    if !source.starts_with(&source_root) {
        return Err(OvenRustcError::InvalidInput {
            field: "JEC source root",
            message: "does not contain the prepared source entrypoint".to_string(),
        });
    }
    let source_digest = digest_regular_file(&source, "source")?;
    let expected_source_digest = request
        .receipt
        .sources
        .supplemental_digests
        .get(request.source_evidence_key.trim())
        .ok_or_else(|| OvenRustcError::InvalidInput {
            field: "source evidence",
            message: format!("receipt does not declare `{}`", request.source_evidence_key),
        })?;
    if expected_source_digest != &source_digest {
        return Err(OvenRustcError::SourceEvidenceMismatch {
            key: request.source_evidence_key.to_string(),
            expected: expected_source_digest.clone(),
            actual: source_digest,
        });
    }
    let selected_artifacts = request.artifacts.for_source_evidence(artifact_role)?;
    let plan = if let Some(plan) = request.artifact_plan {
        trusted_artifact_plan_for_source(plan, request.artifacts, &selected_artifacts)
    } else {
        selected_artifacts.materialize_trusted_store(request.artifact_root, &request.receipt.intent)?
    };
    let mut compile_environment = BTreeMap::new();
    for (name, value) in &plan.compile_environment {
        compile_environment.insert(name.clone(), resolve_compile_environment_value(name, value, &source)?);
    }
    let frozen_environment = freeze_direct_rustc_environment(env::vars_os(), &compile_environment);
    if request.features.windows(2).any(|pair| pair[0] >= pair[1]) {
        return Err(OvenRustcError::InvalidInput {
            field: "JEC features",
            message: "must be sorted and unique".to_string(),
        });
    }
    Ok(OvenPreparedDirectRustcLibrary {
        rustc: fs::canonicalize(request.rustc).map_err(|source_error| OvenRustcError::Io {
            path: request.rustc.to_path_buf(),
            source: source_error,
        })?,
        compiler: compiler.map(|compiler| compiler.evidence.clone()),
        compiler_owner: compiler.cloned(),
        source,
        source_root,
        source_digest,
        artifact_root: canonical_directory(request.artifact_root, "artifact root")?,
        selected_artifacts,
        plan,
        target: request.receipt.intent.target.clone(),
        profile: request.receipt.intent.profile.clone(),
        crate_name: request.crate_name.to_string(),
        edition: request.edition.to_string(),
        features: request.features.to_vec(),
        frozen_environment,
    })
}

impl OvenPreparedDirectRustcLibrary {
    pub(crate) fn rustc(&self) -> &Path {
        &self.rustc
    }

    pub(crate) fn compiler(&self) -> Option<&OvenDirectRustcCompilerEvidence> {
        self.compiler.as_ref()
    }

    pub(crate) fn source(&self) -> &Path {
        &self.source
    }

    pub(crate) fn source_root(&self) -> &Path {
        &self.source_root
    }

    pub(crate) fn source_digest(&self) -> &str {
        &self.source_digest
    }

    pub(crate) fn selected_artifacts(&self) -> &OvenRustcArtifactManifest {
        &self.selected_artifacts
    }

    pub(crate) fn artifact_root(&self) -> &Path {
        &self.artifact_root
    }

    pub(crate) fn artifact_plan(&self) -> &OvenRustcArtifactPlan {
        &self.plan
    }

    pub(crate) fn target(&self) -> &str {
        &self.target
    }

    pub(crate) fn profile(&self) -> &str {
        &self.profile
    }

    pub(crate) fn crate_name(&self) -> &str {
        &self.crate_name
    }

    pub(crate) fn edition(&self) -> &str {
        &self.edition
    }

    pub(crate) fn features(&self) -> &[String] {
        &self.features
    }

    /// Return the complete frozen environment for digest-only host projection.
    pub(crate) fn environment(&self) -> Option<&BTreeMap<String, PathBuf>> {
        self.frozen_environment.as_ref()
    }

    /// Freeze the complete typed projection and the exact physical argv for one output location.
    pub(crate) fn bind<'prepared>(
        &'prepared self,
        output: &Path,
        bindings: &OvenDirectRustcJecBindings,
    ) -> Result<OvenBoundDirectRustcLibrary<'prepared>, OvenRustcError> {
        self.verify_current_inputs()?;
        self.validate_bindings(bindings)?;
        let output = caller_output_path(output, &self.artifact_root)?;
        let mut arguments = Vec::new();
        let mut logical_arguments = vec![
            vec!["option".to_string(), "crate-name".to_string(), self.crate_name.clone()],
            vec!["option".to_string(), "crate-type".to_string(), "rlib".to_string()],
            vec!["option".to_string(), "edition".to_string(), self.edition.clone()],
            vec!["option".to_string(), "target".to_string(), self.target.clone()],
        ];
        arguments.extend([
            OsString::from("--crate-name"),
            OsString::from(&self.crate_name),
            OsString::from("--crate-type"),
            OsString::from("rlib"),
            OsString::from(format!("--edition={}", self.edition)),
            OsString::from("--target"),
            OsString::from(&self.target),
        ]);
        for (name, value) in oven_profile_codegen_options(&self.profile) {
            logical_arguments.push(vec!["option".to_string(), name.to_string(), value.to_string()]);
            arguments.extend([OsString::from("-C"), OsString::from(format!("{name}={value}"))]);
        }
        for feature in &self.features {
            let value = format!("feature={feature:?}");
            logical_arguments.push(vec!["option".to_string(), "cfg".to_string(), value.clone()]);
            arguments.extend([OsString::from("--cfg"), OsString::from(value)]);
        }
        logical_arguments.extend([
            vec!["switch".to_string(), "dep-info-stdout".to_string()],
            vec!["switch".to_string(), "json-diagnostics".to_string()],
            vec!["remap-source".to_string(), bindings.logical_source_root.clone()],
        ]);
        arguments.extend([
            OsString::from("--error-format=json"),
            OsString::from("--emit=dep-info=-,link"),
            joined_os_argument(
                "--remap-path-prefix=",
                &self.source_root,
                Some(&bindings.logical_source_root),
            ),
        ]);
        if let Some(compiler) = &self.compiler {
            logical_arguments.push(vec!["remap-toolchain".to_string(), "incan-toolchain".to_string()]);
            arguments.push(joined_os_argument(
                "--remap-path-prefix=",
                &compiler.sysroot,
                Some("incan-toolchain"),
            ));
        }
        for (path, logical) in self
            .plan
            .dependency_search_paths
            .iter()
            .zip(&bindings.dependency_searches)
        {
            logical_arguments.push(vec!["remap-search".to_string(), logical.clone()]);
            arguments.push(joined_os_argument(
                "--remap-path-prefix=",
                path,
                Some(&format!("incan-native/{logical}")),
            ));
            logical_arguments.push(vec!["search".to_string(), logical.clone()]);
            arguments.extend([OsString::from("-L"), joined_os_argument("dependency=", path, None)]);
        }
        for (path, logical) in self.plan.native_search_paths.iter().zip(&bindings.native_searches) {
            logical_arguments.push(vec!["remap-search".to_string(), logical.clone()]);
            arguments.push(joined_os_argument(
                "--remap-path-prefix=",
                path,
                Some(&format!("incan-native/{logical}")),
            ));
            logical_arguments.push(vec!["search".to_string(), logical.clone()]);
            arguments.extend([OsString::from("-L"), joined_os_argument("native=", path, None)]);
        }
        for ((crate_name, path), logical) in self.plan.externs.iter().zip(&bindings.externs) {
            logical_arguments.push(vec!["extern".to_string(), logical.clone()]);
            arguments.extend([
                OsString::from("--extern"),
                joined_os_argument(&format!("{crate_name}="), path, None),
            ]);
        }
        logical_arguments.extend([
            vec!["source".to_string(), bindings.logical_entrypoint.clone()],
            vec!["output".to_string(), "native".to_string()],
        ]);
        arguments.extend([
            self.source.as_os_str().to_os_string(),
            OsString::from("-o"),
            output.as_os_str().to_os_string(),
        ]);
        let path_effects_digest = self
            .compiler
            .as_ref()
            .map(|compiler| {
                serde_json::to_vec(&(
                    "incan.oven.checked-logical-path-effects/1",
                    compiler.closure_digest(),
                    &logical_arguments,
                ))
                .map(|material| digest_bytes(&material))
            })
            .transpose()
            .map_err(|error| OvenRustcError::InvalidInput {
                field: "JEC logical invocation",
                message: format!("cannot encode path effects: {error}"),
            })?;
        Ok(OvenBoundDirectRustcLibrary {
            prepared: self,
            output,
            arguments,
            logical_arguments,
            path_effects_digest,
        })
    }

    fn verify_current_inputs(&self) -> Result<(), OvenRustcError> {
        if self.compiler_owner.is_none() {
            let compiler = digest_regular_file(&self.rustc, "rustc")?;
            if let Some(expected) = &self.compiler
                && compiler != expected.binary_digest
            {
                return Err(OvenRustcError::ArtifactDigestMismatch {
                    path: self.rustc.clone(),
                    expected: expected.binary_digest.clone(),
                    actual: compiler,
                });
            }
        } else if self
            .compiler_owner
            .as_ref()
            .is_none_or(|owner| owner.rustc != self.rustc || Some(owner.evidence()) != self.compiler.as_ref())
        {
            return Err(OvenRustcError::InvalidInput {
                field: "JEC compiler owner",
                message: "is detached from the prepared compiler evidence".to_string(),
            });
        }
        let source = digest_regular_file(&self.source, "source")?;
        if source != self.source_digest {
            return Err(OvenRustcError::SourceEvidenceMismatch {
                key: "prepared JEC source".to_string(),
                expected: self.source_digest.clone(),
                actual: source,
            });
        }
        Ok(())
    }

    fn validate_bindings(&self, bindings: &OvenDirectRustcJecBindings) -> Result<(), OvenRustcError> {
        let invalid = |message: &str| OvenRustcError::InvalidInput {
            field: "JEC logical bindings",
            message: message.to_string(),
        };
        if bindings.logical_source_root.trim().is_empty()
            || bindings.logical_entrypoint.trim().is_empty()
            || bindings.externs.len() != self.plan.externs.len()
            || bindings.dependency_searches.len() != self.plan.dependency_search_paths.len()
            || bindings.native_searches.len() != self.plan.native_search_paths.len()
        {
            return Err(invalid("do not cover the prepared invocation"));
        }
        if bindings
            .externs
            .iter()
            .chain(&bindings.dependency_searches)
            .chain(&bindings.native_searches)
            .any(String::is_empty)
        {
            return Err(invalid("substitute or omit a prepared physical member"));
        }
        Ok(())
    }
}

impl OvenBoundDirectRustcLibrary<'_> {
    pub(crate) fn output(&self) -> &Path {
        &self.output
    }

    pub(crate) fn logical_arguments(&self) -> &[Vec<String>] {
        &self.logical_arguments
    }

    pub(crate) fn path_effects_digest(&self) -> Option<&str> {
        self.path_effects_digest.as_deref()
    }

    /// Rehash one retained output after the current invocation and Incan key have both been admitted.
    pub(crate) fn reuse_from(
        &self,
        retained_output: &Path,
        expected_digest: &str,
    ) -> Result<Option<OvenDirectRustcBake>, OvenRustcError> {
        let metadata = match fs::symlink_metadata(retained_output) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(OvenRustcError::Io {
                    path: retained_output.to_path_buf(),
                    source: error,
                });
            }
            Ok(metadata) => metadata,
        };
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Ok(None);
        }
        let output_digest = digest_regular_file(retained_output, "retained output")?;
        if output_digest != expected_digest {
            return Ok(None);
        }
        let parent = self.output.parent().ok_or_else(|| OvenRustcError::InvalidInput {
            field: "output",
            message: "must have a parent directory".to_string(),
        })?;
        fs::create_dir_all(parent).map_err(|source| OvenRustcError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
        let mut temporary = tempfile::NamedTempFile::new_in(parent).map_err(|source| OvenRustcError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
        let mut retained = fs::File::open(retained_output).map_err(|source| OvenRustcError::Io {
            path: retained_output.to_path_buf(),
            source,
        })?;
        io::copy(&mut retained, &mut temporary).map_err(|source| OvenRustcError::Io {
            path: retained_output.to_path_buf(),
            source,
        })?;
        temporary.as_file().sync_all().map_err(|source| OvenRustcError::Io {
            path: temporary.path().to_path_buf(),
            source,
        })?;
        if digest_regular_file(temporary.path(), "copied retained output")? != expected_digest {
            return Ok(None);
        }
        temporary.persist(&self.output).map_err(|error| OvenRustcError::Io {
            path: self.output.clone(),
            source: error.error,
        })?;
        Ok(Some(OvenDirectRustcBake {
            source_digest: self.prepared.source_digest.clone(),
            output: self.output.clone(),
            output_digest,
            reused: true,
            lease: None,
        }))
    }

    /// Execute the exact frozen argv under its captured environment and return only sanitized dep-info observations.
    pub(crate) fn compile(&self) -> Result<OvenDirectRustcJecCompilation, OvenRustcError> {
        self.compile_with_observation(true)
    }

    /// Execute an explicitly uncacheable invocation under the same compiler environment as a cacheable one.
    ///
    /// Cache availability cannot select program semantics. This path suppresses reusable observations, but it still
    /// clears ambient state and applies only the admitted plan values used by [`Self::compile`].
    pub(crate) fn compile_uncacheable(&self) -> Result<OvenDirectRustcJecCompilation, OvenRustcError> {
        self.compile_with_observation(false)
    }

    fn compile_with_observation(&self, observe: bool) -> Result<OvenDirectRustcJecCompilation, OvenRustcError> {
        self.prepared.verify_current_inputs()?;
        let mut command = Command::new(&self.prepared.rustc);
        apply_direct_rustc_execution_environment(&mut command, self.prepared.frozen_environment.as_ref())?;
        command.current_dir(&self.prepared.source_root).args(&self.arguments);
        let result = command.output().map_err(|source| OvenRustcError::Io {
            path: self.prepared.rustc.clone(),
            source,
        })?;
        if !result.status.success() {
            return Err(OvenRustcError::CompilationFailed {
                report: parse_rustc_diagnostics(&[], &result.stderr).with_invocation(&command),
            });
        }
        let observation = if observe {
            let frozen = self
                .prepared
                .frozen_environment
                .as_ref()
                .ok_or_else(|| OvenRustcError::InvalidInput {
                    field: "JEC environment",
                    message: "is unavailable for reusable compilation".to_string(),
                })?;
            match parse_rustc_dep_info(&result.stdout, frozen) {
                Ok(observation) => OvenRustcDepInfoOutcome::Observed(observation),
                Err(error) => OvenRustcDepInfoOutcome::Unavailable {
                    reason: error.to_string(),
                },
            }
        } else {
            OvenRustcDepInfoOutcome::Unavailable {
                reason: "reuse observation was deliberately suppressed".to_string(),
            }
        };
        verified_regular_file(&self.output, "output")?;
        let output_digest = digest_regular_file(&self.output, "output")?;
        Ok(OvenDirectRustcJecCompilation {
            bake: OvenDirectRustcBake {
                source_digest: self.prepared.source_digest.clone(),
                output: self.output.clone(),
                output_digest,
                reused: false,
                lease: None,
            },
            observation,
        })
    }
}

/// Apply the one deterministic direct-Rustc environment used with or without cache acceleration.
fn apply_direct_rustc_execution_environment(
    command: &mut Command,
    frozen_environment: Option<&BTreeMap<String, PathBuf>>,
) -> Result<(), OvenRustcError> {
    let frozen_environment = frozen_environment.ok_or_else(|| OvenRustcError::InvalidInput {
        field: "JEC environment",
        message: "is unavailable for direct compilation".to_string(),
    })?;
    command.env_clear().envs(frozen_environment);
    Ok(())
}

/// Construct one path-bearing argument without lossy UTF-8 conversion.
fn joined_os_argument(prefix: &str, path: &Path, suffix: Option<&str>) -> OsString {
    let mut value = OsString::from(prefix);
    value.push(path.as_os_str());
    if let Some(suffix) = suffix {
        value.push("=");
        value.push(suffix);
    }
    value
}

/// Parse Rustc's Makefile dep-info and discard every raw environment value before returning.
fn parse_rustc_dep_info(
    bytes: &[u8],
    environment: &BTreeMap<String, PathBuf>,
) -> Result<OvenRustcDepInfoObservation, OvenRustcError> {
    let text = std::str::from_utf8(bytes).map_err(|error| OvenRustcError::InvalidInput {
        field: "JEC dep-info",
        message: format!("is not UTF-8: {error}"),
    })?;
    let unfolded = text.replace("\\\r\n", "").replace("\\\n", "");
    let dependencies = unfolded
        .lines()
        .filter_map(|line| {
            let line = line.trim_end_matches('\r');
            (!line.trim().is_empty() && !line.starts_with('#')).then_some(line)
        })
        .find_map(|line| line.find(": ").map(|separator| &line[separator + 2..]))
        .ok_or_else(|| OvenRustcError::InvalidInput {
            field: "JEC dep-info",
            message: "has no dependency rule".to_string(),
        })?;
    let files = parse_makefile_words(dependencies)?;
    if files.is_empty() {
        return Err(OvenRustcError::InvalidInput {
            field: "JEC dep-info",
            message: "has an empty dependency rule".to_string(),
        });
    }
    let mut files = files.into_iter().map(PathBuf::from).collect::<Vec<_>>();
    files.sort();
    files.dedup();

    let mut observed = BTreeMap::new();
    for line in unfolded.lines() {
        let Some(value) = line.trim_end_matches('\r').strip_prefix("# env-dep:") else {
            continue;
        };
        let (name, supplied) = value
            .split_once('=')
            .map_or((value, None), |(name, value)| (name, Some(value)));
        if name.is_empty() || observed.contains_key(name) {
            return Err(OvenRustcError::InvalidInput {
                field: "JEC dep-info",
                message: "has an empty or repeated environment name".to_string(),
            });
        }
        let current = environment.get(name);
        let matches = match (supplied, current) {
            (None, None) => true,
            (Some(supplied), Some(current)) => current.to_str() == Some(supplied),
            _ => false,
        };
        if !matches {
            return Err(OvenRustcError::InvalidInput {
                field: "JEC dep-info",
                message: format!("does not match the frozen value presence for `{name}`"),
            });
        }
        observed.insert(
            name.to_string(),
            current.map(|value| digest_bytes(value.as_os_str().as_encoded_bytes())),
        );
    }
    Ok(OvenRustcDepInfoObservation {
        files,
        environment: observed
            .into_iter()
            .map(|(name, value_digest)| OvenRustcObservedEnvironment { name, value_digest })
            .collect(),
    })
}

/// Decode the subset of Makefile word escaping emitted by Rustc dep-info.
fn parse_makefile_words(value: &str) -> Result<Vec<String>, OvenRustcError> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut characters = value.chars().peekable();
    while let Some(character) = characters.next() {
        match character {
            '\\' => {
                let escaped = characters.next().ok_or_else(|| OvenRustcError::InvalidInput {
                    field: "JEC dep-info",
                    message: "ends with an incomplete path escape".to_string(),
                })?;
                word.push(escaped);
            }
            '$' if characters.peek() == Some(&'$') => {
                characters.next();
                word.push('$');
            }
            character if character.is_whitespace() => {
                if !word.is_empty() {
                    words.push(std::mem::take(&mut word));
                }
            }
            character => word.push(character),
        }
    }
    if !word.is_empty() {
        words.push(word);
    }
    Ok(words)
}

/// Compile one compiler-suite test root from a caller-held leased store artifact without a Cargo consumer process.
pub(crate) fn bake_trusted_direct_rustc_test(
    request: &OvenTrustedDirectRustcTargetRequest<'_>,
) -> Result<OvenDirectRustcTestBake, OvenRustcError> {
    bake_direct_rustc(
        request.receipt,
        request.artifacts,
        request.artifact_root,
        request.rustc,
        request.source,
        request.output,
        request.crate_name,
        request.edition,
        request.source_evidence_key,
        request.source_evidence_key,
        true,
        OvenDirectRustcOutputKind::Binary,
        true,
        request.artifact_plan,
        request.prefer_dynamic,
        request.features,
    )
}

/// Compile one regular Rust library from selected Oven foundations without a Cargo target directory.
///
/// The library is caller-owned ephemeral materialization. Its reusable sidecar still binds the exact receipt,
/// selected artifact manifest, source digest, resolved feature set, and library output kind before a later direct
/// Rustc target may link it.
pub(crate) fn bake_trusted_direct_rustc_library(
    request: &OvenTrustedDirectRustcTargetRequest<'_>,
) -> Result<OvenDirectRustcBake, OvenRustcError> {
    bake_trusted_direct_rustc_library_with_artifact_role(request, request.source_evidence_key)
}

/// Compile a library using an original declared native role while preserving its receipt source key.
///
/// The caller retains the admitted manifest, physical plan and leases. This argument selects their existing role;
/// it neither aliases physical bindings nor adds source authority to the receipt.
pub(crate) fn bake_trusted_direct_rustc_library_with_artifact_role(
    request: &OvenTrustedDirectRustcTargetRequest<'_>,
    artifact_role: &str,
) -> Result<OvenDirectRustcBake, OvenRustcError> {
    bake_direct_rustc(
        request.receipt,
        request.artifacts,
        request.artifact_root,
        request.rustc,
        request.source,
        request.output,
        request.crate_name,
        request.edition,
        request.source_evidence_key,
        artifact_role,
        false,
        OvenDirectRustcOutputKind::Library,
        true,
        request.artifact_plan,
        request.prefer_dynamic,
        request.features,
    )
}

/// Compile one regular dynamic Rust library from selected Oven foundations without a Cargo target directory.
///
/// The compiler suite uses this only for its shared top-level compiler library. Linking that one expensive crate
/// dynamically keeps each independently executed integration-test root from embedding another static copy.
pub(crate) fn bake_trusted_direct_rustc_dylib(
    request: &OvenTrustedDirectRustcTargetRequest<'_>,
) -> Result<OvenDirectRustcBake, OvenRustcError> {
    bake_direct_rustc(
        request.receipt,
        request.artifacts,
        request.artifact_root,
        request.rustc,
        request.source,
        request.output,
        request.crate_name,
        request.edition,
        request.source_evidence_key,
        request.source_evidence_key,
        false,
        OvenDirectRustcOutputKind::Dylib,
        true,
        request.artifact_plan,
        request.prefer_dynamic,
        request.features,
    )
}

/// Compile one procedural-macro library from selected Oven foundations without a Cargo target directory.
///
/// A workspace materialization DAG treats the resulting dynamic library as a caller-owned `--extern` input for
/// later direct-Rustc steps. Its receipt still binds the exact source, compiler, features, and immutable foundation.
pub(crate) fn bake_trusted_direct_rustc_proc_macro(
    request: &OvenTrustedDirectRustcTargetRequest<'_>,
) -> Result<OvenDirectRustcBake, OvenRustcError> {
    bake_trusted_direct_rustc_proc_macro_with_artifact_role(request, request.source_evidence_key)
}

/// Compile a procedural macro using its original declared native role and unchanged source receipt.
///
/// The caller retains the admitted manifest, physical plan and leases. This argument selects their existing role;
/// it neither aliases physical bindings nor adds source authority to the receipt.
pub(crate) fn bake_trusted_direct_rustc_proc_macro_with_artifact_role(
    request: &OvenTrustedDirectRustcTargetRequest<'_>,
    artifact_role: &str,
) -> Result<OvenDirectRustcBake, OvenRustcError> {
    bake_direct_rustc(
        request.receipt,
        request.artifacts,
        request.artifact_root,
        request.rustc,
        request.source,
        request.output,
        request.crate_name,
        request.edition,
        request.source_evidence_key,
        artifact_role,
        false,
        OvenDirectRustcOutputKind::ProcMacro,
        true,
        request.artifact_plan,
        request.prefer_dynamic,
        request.features,
    )
}

/// Compile one compiler-suite binary root from a caller-held leased store artifact without a Cargo consumer process.
pub(crate) fn bake_trusted_direct_rustc_run(
    request: &OvenTrustedDirectRustcTargetRequest<'_>,
) -> Result<OvenDirectRustcBake, OvenRustcError> {
    bake_direct_rustc(
        request.receipt,
        request.artifacts,
        request.artifact_root,
        request.rustc,
        request.source,
        request.output,
        request.crate_name,
        request.edition,
        request.source_evidence_key,
        request.source_evidence_key,
        false,
        OvenDirectRustcOutputKind::Binary,
        true,
        request.artifact_plan,
        request.prefer_dynamic,
        request.features,
    )
}

/// Compile one receipt-bound generated Rust binary without a Cargo consumer process.
pub fn bake_direct_rustc_run(request: &OvenDirectRustcRunRequest) -> Result<OvenDirectRustcBake, OvenRustcError> {
    bake_direct_rustc(
        &request.receipt,
        &request.artifacts,
        &request.artifact_root,
        &request.rustc,
        &request.source,
        &request.output,
        &request.crate_name,
        &request.edition,
        &request.source_evidence_key,
        &request.source_evidence_key,
        false,
        OvenDirectRustcOutputKind::Binary,
        false,
        None,
        false,
        &request.receipt.intent.features,
    )
}

/// Project a pre-materialized trusted plan onto the receipt-authorized direct extern roots.
///
/// Compiler-suite callers retain an already checked plan to avoid a second traversal of large immutable
/// foundations. That plan can include caller-owned library outputs, so it cannot simply be re-materialized from the
/// filtered manifest. Remove only immutable manifest externs that the source-evidence projection excludes; the
/// caller-owned additions remain exact inputs and continue to participate in output reuse evidence.
fn trusted_artifact_plan_for_source(
    plan: &OvenRustcArtifactPlan,
    declared_artifacts: &OvenRustcArtifactManifest,
    selected_artifacts: &OvenRustcArtifactManifest,
) -> OvenRustcArtifactPlan {
    let declared_names = declared_artifacts
        .externs
        .iter()
        .map(|artifact| artifact.crate_name.as_str())
        .collect::<BTreeSet<_>>();
    let selected_names = selected_artifacts
        .externs
        .iter()
        .map(|artifact| artifact.crate_name.as_str())
        .collect::<BTreeSet<_>>();
    // An expose-extern caller library is identified by its crate-name reuse evidence. This matters when a caller
    // deliberately declares the same crate name as a compiler-private helper retained in the complete manifest:
    // source projection must remove the helper while preserving the caller's separately verified output.
    let caller_owned_extern_names = plan
        .caller_owned_library_digests
        .keys()
        .filter(|key| !key.starts_with("transitive:"))
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let mut selected_plan = plan.clone();
    let mut excluded_direct_root_parents = BTreeSet::new();
    let mut selected_direct_root_parents = BTreeSet::new();
    for (crate_name, path) in &plan.externs {
        if !declared_names.contains(crate_name.as_str()) {
            continue;
        }
        let Some(parent) = path.parent() else {
            continue;
        };
        if selected_names.contains(crate_name.as_str()) || caller_owned_extern_names.contains(crate_name.as_str()) {
            selected_direct_root_parents.insert(parent.to_path_buf());
        } else {
            excluded_direct_root_parents.insert(parent.to_path_buf());
        }
    }
    selected_plan.externs.retain(|(crate_name, _)| {
        !declared_names.contains(crate_name.as_str())
            || selected_names.contains(crate_name.as_str())
            || caller_owned_extern_names.contains(crate_name.as_str())
    });
    selected_plan.dependency_search_paths.retain(|search_path| {
        !excluded_direct_root_parents.contains(search_path) || selected_direct_root_parents.contains(search_path)
    });
    selected_plan
}

/// Project a verified, already-materialized direct-Rustc plan onto one receipt-authorized source root.
///
/// Callers that add a caller-owned registry leaf must make that decision against this projection, rather than the
/// complete Loaf plan. A complete Loaf deliberately retains compiler-private helpers such as the vocabulary
/// serializer; treating one of those helpers as a public caller extern can either hide a declared dependency or
/// expose a second incompatible Rust crate identity.
pub(crate) fn trusted_artifact_plan_for_source_evidence(
    plan: &OvenRustcArtifactPlan,
    artifacts: &OvenRustcArtifactManifest,
    source_evidence_key: &str,
) -> Result<OvenRustcArtifactPlan, OvenRustcError> {
    let selected_artifacts = artifacts.for_source_evidence(source_evidence_key)?;
    Ok(trusted_artifact_plan_for_source(plan, artifacts, &selected_artifacts))
}

/// Return the direct extern names exposed to one receipt-authorized source root.
#[cfg(test)]
pub(crate) fn direct_rustc_source_extern_names(
    artifacts: &OvenRustcArtifactManifest,
    source_evidence_key: &str,
) -> Result<BTreeSet<String>, OvenRustcError> {
    Ok(artifacts
        .for_source_evidence(source_evidence_key)?
        .externs
        .into_iter()
        .map(|artifact| artifact.crate_name)
        .collect())
}

/// Compile one receipt-bound source using its distinct, original native artifact role.
///
/// The receipt key verifies source bytes; the artifact role selects existing native inputs. Ordinary callers retain
/// the same key for both. Provider callers keep their original physical role without modifying either receipt.
#[allow(clippy::too_many_arguments)]
fn bake_direct_rustc(
    receipt: &OvenReceipt,
    artifacts: &OvenRustcArtifactManifest,
    artifact_root: &Path,
    rustc: &Path,
    source: &Path,
    output: &Path,
    crate_name: &str,
    edition: &str,
    source_evidence_key: &str,
    artifact_role: &str,
    test_harness: bool,
    output_kind: OvenDirectRustcOutputKind,
    trusted_store: bool,
    trusted_artifact_plan: Option<&OvenRustcArtifactPlan>,
    prefer_dynamic: bool,
    features: &[String],
) -> Result<OvenDirectRustcBake, OvenRustcError> {
    receipt
        .verify_identity()
        .map_err(|error| OvenRustcError::InvalidInput {
            field: "receipt",
            message: error.to_string(),
        })?;
    verify_rustc_identity(rustc, &receipt.intent.toolchain)?;
    validate_rust_identifier(crate_name)?;
    validate_edition(edition)?;
    let source = verified_regular_file(source, "source")?;
    let output = caller_output_path(output, artifact_root)?;
    let source_bytes = fs::read(&source).map_err(|source_error| OvenRustcError::Io {
        path: source.clone(),
        source: source_error,
    })?;
    let source_digest = digest_bytes(&source_bytes);
    let expected_source_digest = receipt
        .sources
        .supplemental_digests
        .get(source_evidence_key.trim())
        .ok_or_else(|| OvenRustcError::InvalidInput {
            field: "source evidence",
            message: format!("receipt does not declare `{source_evidence_key}`"),
        })?;
    if expected_source_digest != &source_digest {
        return Err(OvenRustcError::SourceEvidenceMismatch {
            key: source_evidence_key.to_string(),
            expected: expected_source_digest.clone(),
            actual: source_digest,
        });
    }
    let selected_artifacts = artifacts.for_source_evidence(artifact_role)?;
    let parent = output.parent().ok_or_else(|| OvenRustcError::InvalidInput {
        field: "output",
        message: "must have a parent directory".to_string(),
    })?;
    fs::create_dir_all(parent).map_err(|source_error| OvenRustcError::Io {
        path: parent.to_path_buf(),
        source: source_error,
    })?;
    let output_receipt = OvenDirectRustcOutputReceipt {
        schema_version: OVEN_DIRECT_RUSTC_OUTPUT_RECEIPT_SCHEMA_VERSION,
        receipt_identity: receipt.identity.clone(),
        artifact_manifest_digest: digest_bytes(&serde_json::to_vec(&selected_artifacts).map_err(|error| {
            OvenRustcError::InvalidInput {
                field: "artifact manifest",
                message: format!("cannot serialize verified manifest identity: {error}"),
            }
        })?),
        source_digest: source_digest.clone(),
        crate_name: crate_name.to_string(),
        edition: edition.to_string(),
        features: features.to_vec(),
        test_harness,
        prefer_dynamic,
        output_kind: output_kind.receipt_value().to_string(),
        caller_owned_library_digests: trusted_artifact_plan
            .map(|plan| plan.caller_owned_library_digests.clone())
            .unwrap_or_default(),
    };
    if let Some(output_digest) = caller_output_reusable_digest(&output, &output_receipt) {
        return Ok(OvenDirectRustcBake {
            source_digest,
            output,
            output_digest,
            reused: true,
            lease: None,
        });
    }

    // Publication has already performed full content verification for a selected store entry. On a genuine caller
    // output miss, normal consumers prove file shape/containment under their active lease rather than rehash every
    // dependency; externally supplied plans retain the stronger byte-for-byte materialization path.
    let plan = if let Some(plan) = trusted_artifact_plan {
        trusted_artifact_plan_for_source(plan, artifacts, &selected_artifacts)
    } else if trusted_store {
        selected_artifacts.materialize_trusted_store(artifact_root, &receipt.intent)?
    } else {
        selected_artifacts.materialize(artifact_root, &receipt.intent)?
    };

    let mut command = Command::new(rustc);
    if test_harness {
        command.arg("--test");
    }
    match output_kind {
        OvenDirectRustcOutputKind::Binary => {}
        OvenDirectRustcOutputKind::Library => {
            command.args(["--crate-type", "lib"]);
        }
        OvenDirectRustcOutputKind::Dylib => {
            command.args(["--crate-type", "dylib"]);
        }
        OvenDirectRustcOutputKind::ProcMacro => {
            // `proc_macro` is supplied by the selected Rustc sysroot rather than by a Cargo-produced artifact.
            // Naming the crate explicitly is still required for edition-2018-and-later sources that import it with
            // `use proc_macro::…`; unlike an `extern crate proc_macro` declaration, that import does not cause
            // Rustc to infer the sysroot dependency.
            command.args(["--crate-type", "proc-macro", "--extern", "proc_macro"]);
        }
    }
    if prefer_dynamic {
        // Cargo emits both flags for a proc-macro libtest. `proc_macro` is provided by the receipt-selected Rust
        // toolchain sysroot rather than a Cargo target artifact, so it is intentionally not represented as a stored
        // third-party `--extern` file. `rpath` is required as well: compiler-suite children can pass a dynamically
        // linked caller-owned CLI through a shell script, and macOS strips `DYLD_*` values when it starts its system
        // shell. The selected Rustc and caller-owned `-L dependency` paths define the embedded loader closure.
        command.args(["-C", "prefer-dynamic", "-C", "rpath", "--extern", "proc_macro"]);
    }
    command
        .arg("--target")
        .arg(&receipt.intent.target)
        .arg(format!("--edition={edition}"))
        .arg("--crate-name")
        .arg(crate_name)
        .arg("--error-format=json")
        .arg(&source)
        .arg("-o")
        .arg(&output);
    apply_oven_profile(&mut command, &receipt.intent.profile);
    clear_inherited_cargo_environment(&mut command);
    for (name, value) in &plan.compile_environment {
        let value = resolve_compile_environment_value(name, value, &source)?;
        command.env(name, value);
    }
    for feature in features {
        command.arg("--cfg").arg(format!("feature={feature:?}"));
    }
    for path in &plan.dependency_search_paths {
        command.arg("-L").arg(format!("dependency={}", path.display()));
    }
    for path in &plan.native_search_paths {
        command.arg("-L").arg(format!("native={}", path.display()));
    }
    append_native_runtime_rpaths(&mut command, &receipt.intent.target, &plan.native_search_paths);
    for (crate_name, path) in &plan.externs {
        command.arg("--extern").arg(format!("{crate_name}={}", path.display()));
    }
    let output_result = command.output().map_err(|source_error| OvenRustcError::Io {
        path: rustc.to_path_buf(),
        source: source_error,
    })?;
    if !output_result.status.success() {
        return Err(OvenRustcError::CompilationFailed {
            report: parse_rustc_diagnostics(&output_result.stdout, &output_result.stderr).with_invocation(&command),
        });
    }
    verified_regular_file(&output, "output")?;
    let output_digest = digest_regular_file(&output, "output")?;
    write_caller_output_record(
        &output,
        &OvenDirectRustcOutputRecord {
            inputs: output_receipt,
            output_digest: output_digest.clone(),
        },
    )?;
    Ok(OvenDirectRustcBake {
        source_digest,
        output,
        output_digest,
        reused: false,
        lease: None,
    })
}

/// Embed the exact receipt-selected native directories required by a host-native Unix dynamic runtime.
///
/// Immutable Oven store identities contain a colon (`sha256:`), so they cannot safely be passed through a Unix
/// colon-separated loader environment. The linker records these already materialized, digest-verified native search
/// directories directly in the caller-owned binary instead. Static-only directories are harmless rpath entries;
/// retaining all selected native directories keeps this transport independent of a filename heuristic and never
/// admits an ambient package or host search path. Cross-target Android/iOS artifacts are staged by their explicit
/// adapter rather than receiving a meaningless path to this host's Oven store.
fn append_native_runtime_rpaths(command: &mut Command, target: &str, native_search_paths: &[PathBuf]) {
    if !is_host_native_unix_target(target) {
        return;
    }
    for path in native_search_paths {
        command.arg("-C").arg("link-arg=-Wl,-rpath");
        command.arg("-C").arg(format!("link-arg={}", path.display()));
    }
}

/// Return whether a target produces an executable that can resolve this host's selected native-store directories.
fn is_host_native_unix_target(target: &str) -> bool {
    let host_architecture = std::env::consts::ARCH;
    (cfg!(target_os = "macos") && target == format!("{host_architecture}-apple-darwin"))
        || (cfg!(all(target_os = "linux", target_env = "gnu"))
            && target == format!("{host_architecture}-unknown-linux-gnu"))
        || (cfg!(all(target_os = "linux", target_env = "musl"))
            && target == format!("{host_architecture}-unknown-linux-musl"))
}

/// Return the caller-owned sidecar path without accepting an output that lacks a safe file name.
fn caller_output_receipt_path(output: &Path) -> Result<PathBuf, OvenRustcError> {
    let name = output
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| OvenRustcError::InvalidInput {
            field: "output",
            message: "must end in a UTF-8 file name to retain Oven reuse evidence".to_string(),
        })?;
    Ok(output.with_file_name(format!("{name}.oven-output.json")))
}

/// Verify complete input evidence and hash the current regular output once, returning the digest only on a match.
///
/// Missing, malformed, old or mismatched evidence is a cache miss. Returning the verified digest lets the caller
/// report reuse without rereading the output. The caller must perform receipt/compiler/input admission first.
fn caller_output_reusable_digest(output: &Path, expected: &OvenDirectRustcOutputReceipt) -> Option<String> {
    let receipt_path = caller_output_receipt_path(output).ok()?;
    let bytes = fs::read(&receipt_path).ok()?;
    let record: OvenDirectRustcOutputRecord = serde_json::from_slice(&bytes).ok()?;
    if record.inputs != *expected {
        return None;
    }
    let output_digest = digest_regular_file(output, "output").ok()?;
    (output_digest == record.output_digest).then_some(output_digest)
}

/// Atomically publish the input/output binding only after rustc has produced and verified its regular output.
fn write_caller_output_record(output: &Path, record: &OvenDirectRustcOutputRecord) -> Result<(), OvenRustcError> {
    let path = caller_output_receipt_path(output)?;
    let parent = path.parent().ok_or_else(|| OvenRustcError::InvalidInput {
        field: "output",
        message: "reuse evidence has no parent directory".to_string(),
    })?;
    let bytes = serde_json::to_vec(record).map_err(|error| OvenRustcError::InvalidInput {
        field: "output receipt",
        message: format!("cannot serialize reuse evidence: {error}"),
    })?;
    let temporary = parent.join(format!(
        ".{}.{}.tmp",
        path.file_name().and_then(|name| name.to_str()).unwrap_or("oven-output"),
        std::process::id(),
    ));
    fs::write(&temporary, bytes).map_err(|source| OvenRustcError::Io {
        path: temporary.clone(),
        source,
    })?;
    fs::rename(&temporary, &path).map_err(|source| OvenRustcError::Io { path, source })
}

/// Build the complete reusable compiler environment from admitted plan values only.
///
/// Ambient values are intentionally ignored. Rustc controls and dynamic-loader paths can change compiler behavior
/// without appearing as source `env!` dependencies, so inherited process state cannot participate in a reusable
/// compilation unless a later policy models it explicitly.
fn freeze_direct_rustc_environment(
    _inherited: impl IntoIterator<Item = (OsString, OsString)>,
    compile_environment: &BTreeMap<String, PathBuf>,
) -> Option<BTreeMap<String, PathBuf>> {
    Some(compile_environment.clone())
}

fn direct_rustc_excludes_inherited_environment(name: &OsStr) -> bool {
    name == "CARGO"
        || name.to_string_lossy().starts_with("CARGO_")
        || matches!(
            name.to_str(),
            Some("RUSTC_WRAPPER" | "RUSTC_WORKSPACE_WRAPPER" | "RUSTFLAGS")
        )
}

/// Clear ambient Cargo state before direct consumer execution; the explicit compiler path remains authoritative.
pub(crate) fn clear_inherited_cargo_environment(command: &mut Command) {
    for (name, _) in env::vars_os().filter(|(name, _)| direct_rustc_excludes_inherited_environment(name)) {
        command.env_remove(name);
    }
}

/// Apply the named Oven compiler-suite contract to a direct compiler invocation.
///
/// Cargo sees the matching manifest-declared profile only inside the explicit bootstrap publisher. Every normal
/// consumer uses this direct-rustc representation instead, so its flags are deliberate receipt semantics rather
/// than inherited Cargo environment state.
fn apply_oven_profile(command: &mut Command, profile: &str) {
    for (name, value) in oven_profile_codegen_options(profile) {
        command.arg("-C").arg(format!("{name}={value}"));
    }
}

/// Return the complete typed profile projection shared by physical invocation and JEC identity.
fn oven_profile_codegen_options(profile: &str) -> &'static [(&'static str, &'static str)] {
    if profile == OVEN_COMPILER_TEST_PROFILE {
        return &[
            ("debuginfo", "0"),
            ("strip", "debuginfo"),
            ("debug-assertions", "on"),
            ("overflow-checks", "on"),
        ];
    }

    // Optimization is part of what a profile *means*, and rustc optimizes nothing unless told to. Cargo used to
    // supply this implicitly from `[profile.release]`; once Oven replaced Cargo on the normal build path, nothing
    // did, so `incan build --release` emitted `opt-level=0` binaries and ran roughly six times slower than the
    // same sources at `-C opt-level=3`. These flags mirror Cargo's own release and dev profiles so a release
    // binary performs like the Rust it compiles to, and both profiles state their level explicitly rather than
    // inheriting a compiler default that has already gone wrong once.
    match profile {
        "release" => &[("opt-level", "3")],
        _ => &[("opt-level", "0")],
    }
}

/// Derive the selected compiler's stable version identity and require an exact receipt match before compilation.
fn verify_rustc_identity(rustc: &Path, expected: &str) -> Result<(), OvenRustcError> {
    let actual = rustc_identity(rustc)?;
    if actual == expected {
        return Ok(());
    }
    Err(OvenRustcError::ToolchainMismatch {
        expected: expected.to_string(),
        actual,
    })
}

/// Ask the user's own Rustup which tool its active toolchain resolves to.
///
/// This answers for the ambient configuration, honoring `RUSTUP_TOOLCHAIN` and any directory override exactly as a
/// hand-typed `rustup which` would. It is the fallback for development checkouts and installations without an
/// Incan-owned toolchain; installations that have one resolve through [`incan_owned_tool`] instead.
fn rustup_reported_tool(tool: &str) -> Option<String> {
    let mut command = Command::new("rustup");
    command.args(["which", tool]);
    clear_inherited_cargo_environment(&mut command);
    let output = command.output().ok()?;
    if !output.status.success() {
        return None;
    }
    let reported = String::from_utf8(output.stdout).ok()?;
    let reported = reported.trim().to_string();
    (!reported.is_empty()).then_some(reported)
}

/// Return the Rustup home Incan provisions for itself, when an installed toolchain has one.
///
/// Incan's Loafs are sealed against the exact compiler that baked them, so building with whatever compiler the user
/// happens to have made their global default fails closed with a Loaf-incompatibility error. The installer therefore
/// provisions the release's own required channel into `$INCAN_HOME/rust` (defaulting below the user home) and makes
/// it the default *within that home only*, leaving the user's own Rustup configuration untouched. This returns that
/// home when it exists, so ordinary builds resolve the compiler Incan was built against.
///
/// Returns `None` for development checkouts and for installations that predate this layout, both of which keep the
/// previous behavior of consulting the ambient Rustup default.
fn incan_owned_rustup_home() -> Option<PathBuf> {
    // The installer links commands into a bin directory, and `current_exe` is documented to be allowed to report
    // the symlink rather than its target. Resolving it first is what lets the ancestor walk see the installation
    // the command actually belongs to when that installation lives outside the user home.
    let executable = env::current_exe()
        .ok()
        .map(|executable| fs::canonicalize(&executable).unwrap_or(executable));
    incan_owned_rustup_home_in(
        env::var_os("INCAN_HOME"),
        executable,
        env::var_os("HOME").or_else(|| env::var_os("USERPROFILE")),
    )
}

/// Resolve the Incan-owned Rustup home from explicit inputs, without reading process-global state.
///
/// Selection order: an explicitly named Incan home, then this executable's own installed layout, then the user
/// home default. The executable-relative step matters because the npm and pip shims install below their own
/// package directory rather than the user home and do not set `INCAN_HOME` when they spawn the compiler, so only
/// the executable's location identifies the provisioning that belongs to *this* installation.
///
/// Every candidate must actually contain `rust/toolchains`; a root without one yields `None` so development
/// checkouts and pre-isolation installations keep resolving through the ambient Rustup default.
fn incan_owned_rustup_home_in(
    incan_home: Option<OsString>,
    executable: Option<PathBuf>,
    user_home: Option<OsString>,
) -> Option<PathBuf> {
    /// Accept a candidate Incan home only when it actually carries a provisioned Rustup layout.
    fn provisioned(root: &Path) -> Option<PathBuf> {
        let rust_root = root.join("rust");
        rust_root.join("toolchains").is_dir().then_some(rust_root)
    }

    if let Some(root) = incan_home.filter(|path| !path.is_empty()).map(PathBuf::from)
        && let Some(rust_root) = provisioned(&root)
    {
        return Some(rust_root);
    }
    if let Some(executable) = executable {
        for ancestor in executable.ancestors().skip(1) {
            if let Some(rust_root) = provisioned(ancestor) {
                return Some(rust_root);
            }
        }
    }
    user_home
        .filter(|path| !path.is_empty())
        .and_then(|path| provisioned(&PathBuf::from(path).join(".incan")))
}

/// Name of the pointer file the installer writes to record which channel it provisioned.
const INCAN_OWNED_CHANNEL_POINTER: &str = "incan-channel.txt";

/// Resolve one tool from Incan's own provisioned toolchain by reading the Rustup layout directly.
///
/// This deliberately does not shell out to `rustup`. Rustup resolves a toolchain name from ambient state --
/// `RUSTUP_TOOLCHAIN` and any directory `rust-toolchain.toml` override both win over a home's default -- so asking
/// it would let the user's environment select a toolchain that does not exist inside Incan's home, reintroducing
/// the very coupling this isolation removes. The installed layout is deterministic, so reading it is both exact
/// and cheaper than a subprocess.
///
/// The installer records the channel it provisioned in [`INCAN_OWNED_CHANNEL_POINTER`]; that pointer disambiguates
/// the toolchain directory when an earlier channel is still present after an upgrade. Without a pointer the home
/// must hold exactly one toolchain, otherwise the choice would be arbitrary and this returns `None` so resolution
/// falls back to the ambient default rather than guessing.
fn incan_owned_tool(rust_root: &Path, tool: &str) -> Option<PathBuf> {
    let toolchains = rust_root.join("toolchains");
    let executable = |directory: &Path| -> Option<PathBuf> {
        let candidate = directory.join("bin").join(tool);
        candidate.is_file().then_some(candidate)
    };

    // ---- Preferred: the channel the installer recorded for this home ----
    if let Ok(channel) = fs::read_to_string(rust_root.join(INCAN_OWNED_CHANNEL_POINTER)) {
        let channel = channel.trim();
        if !channel.is_empty()
            && let Ok(entries) = fs::read_dir(&toolchains)
        {
            let mut matches: Vec<PathBuf> = entries
                .flatten()
                .map(|entry| entry.path())
                .filter(|path| {
                    path.file_name()
                        .and_then(|name| name.to_str())
                        .is_some_and(|name| name == channel || name.starts_with(&format!("{channel}-")))
                })
                .collect();
            matches.sort();
            if let Some(directory) = matches.first()
                && let Some(found) = executable(directory)
            {
                return Some(found);
            }
        }
    }

    // ---- Fallback: an unambiguous single-toolchain home ----
    let mut directories: Vec<PathBuf> = fs::read_dir(&toolchains)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect();
    directories.sort();
    match directories.as_slice() {
        [only] => executable(only),
        _ => None,
    }
}

/// Report whether Incan's own provisioned toolchain carries a Rust target, when such a toolchain exists.
///
/// `None` means there is no Incan-owned toolchain to ask, so callers must fall back to the ambient Rustup. This
/// matters because the installer adds required targets to Incan's home and deliberately leaves the user's own
/// Rustup untouched: asking ambient Rustup about a target Incan provisioned for itself reports a false absence.
///
/// Membership is read from the toolchain layout (`lib/rustlib/<target>`) for the same reason [`incan_owned_tool`]
/// reads it: ambient toolchain selection cannot redirect a directory check.
pub(crate) fn incan_owned_target_installed(target: &str) -> Option<bool> {
    let rust_root = incan_owned_rustup_home()?;
    let rustc = incan_owned_tool(&rust_root, "rustc")?;
    let toolchain_root = rustc.parent()?.parent()?;
    Some(toolchain_root.join("lib").join("rustlib").join(target).is_dir())
}

/// Resolve the Rust compiler belonging to Incan's own provisioned toolchain.
///
/// Pairs with [`incan_owned_cargo`]. A toolchain-direct Cargo does not imply a matching compiler: Cargo resolves
/// `rustc` from `RUSTC` or `PATH`, and on a machine with Rustup installed `PATH` reaches the Rustup shim, which
/// selects the user's default toolchain. Selecting Incan's Cargo without also selecting its compiler therefore
/// builds one dependency graph with two rustc versions, which Cargo only reports much later as
/// "found crate `x` compiled by an incompatible version of rustc".
pub(crate) fn incan_owned_rustc() -> Option<PathBuf> {
    incan_owned_tool(&incan_owned_rustup_home()?, "rustc")
}

/// Resolve the active Rust compiler without involving Cargo or a Cargo target directory.
///
/// An explicit `RUSTC` must be a regular executable file, not a shell fragment. When it is absent, the Rustup
/// toolchain resolver supplies the compiler path; that remains separate from the explicit `legacy_cargo` publisher.
///
/// When `rustup` is not reachable the error names the most common cause: provisioning Rust from within an Incan
/// command wires Rustup into shell profiles for future shells, so the shell that triggered it keeps its original
/// `PATH` and needs to be refreshed before the compiler is visible.
pub fn resolve_active_rustc() -> Result<PathBuf, OvenRustcError> {
    if let Some(path) = env::var_os("RUSTC").filter(|path| !path.is_empty()) {
        return verified_regular_file(Path::new(&path), "RUSTC");
    }
    if let Some(isolated) = incan_owned_rustup_home().and_then(|home| incan_owned_tool(&home, "rustc")) {
        return verified_regular_file(&isolated, "rustc");
    }
    let Some(reported) = rustup_reported_tool("rustc") else {
        return Err(OvenRustcError::InvalidInput {
            field: "rustc",
            message: "could not locate the active Rust compiler. If Rust was just installed, open a new shell (or \
                      run `. \"$HOME/.cargo/env\"`) so `rustup` is on PATH; otherwise install Rust through rustup, \
                      or set RUSTC to an explicit compiler path"
                .to_string(),
        });
    };
    verified_regular_file(Path::new(&reported), "rustc")
}

/// Read one regular Rust compiler's stable `--version` identity without invoking Cargo.
pub fn rustc_identity(rustc: &Path) -> Result<String, OvenRustcError> {
    let rustc = verified_regular_file(rustc, "rustc")?;
    let mut command = Command::new(&rustc);
    command.arg("--version");
    clear_inherited_cargo_environment(&mut command);
    let output = command.output().map_err(|source| OvenRustcError::Io {
        path: rustc.clone(),
        source,
    })?;
    if !output.status.success() {
        return Err(OvenRustcError::InvalidInput {
            field: "rustc",
            message: "must report a successful `--version` identity".to_string(),
        });
    }
    let actual = String::from_utf8(output.stdout).map_err(|error| OvenRustcError::InvalidInput {
        field: "rustc",
        message: format!("reported non-UTF-8 `--version` output: {error}"),
    })?;
    let actual = actual.trim();
    if actual.is_empty() {
        return Err(OvenRustcError::InvalidInput {
            field: "rustc",
            message: "reported an empty `--version` identity".to_string(),
        });
    }
    Ok(actual.to_string())
}

/// Read the active compiler's host target from `rustc -vV` without consulting Cargo metadata.
pub fn rustc_host_target(rustc: &Path) -> Result<String, OvenRustcError> {
    let rustc = verified_regular_file(rustc, "rustc")?;
    let mut command = Command::new(&rustc);
    command.arg("-vV");
    clear_inherited_cargo_environment(&mut command);
    let output = command.output().map_err(|source| OvenRustcError::Io {
        path: rustc.clone(),
        source,
    })?;
    if !output.status.success() {
        return Err(OvenRustcError::InvalidInput {
            field: "rustc",
            message: "must report a successful `-vV` host target".to_string(),
        });
    }
    let output = String::from_utf8(output.stdout).map_err(|error| OvenRustcError::InvalidInput {
        field: "rustc",
        message: format!("reported non-UTF-8 `-vV` output: {error}"),
    })?;
    output
        .lines()
        .find_map(|line| line.strip_prefix("host: "))
        .map(str::trim)
        .filter(|target| !target.is_empty())
        .map(ToString::to_string)
        .ok_or_else(|| OvenRustcError::InvalidInput {
            field: "rustc",
            message: "did not report a host target in `-vV` output".to_string(),
        })
}

/// Resolve the selected compiler's sysroot without consulting Cargo.
fn rustc_sysroot(rustc: &Path) -> Result<PathBuf, OvenRustcError> {
    let rustc = verified_regular_file(rustc, "rustc")?;
    let mut command = Command::new(&rustc);
    command.args(["--print", "sysroot"]);
    clear_inherited_cargo_environment(&mut command);
    let output = command.output().map_err(|source| OvenRustcError::Io {
        path: rustc.clone(),
        source,
    })?;
    if !output.status.success() {
        return Err(OvenRustcError::InvalidInput {
            field: "rustc",
            message: "must report a successful `--print sysroot`".to_string(),
        });
    }
    let sysroot = String::from_utf8(output.stdout).map_err(|error| OvenRustcError::InvalidInput {
        field: "rustc",
        message: format!("reported a non-UTF-8 sysroot: {error}"),
    })?;
    let sysroot = PathBuf::from(sysroot.trim());
    if !sysroot.is_dir() {
        return Err(OvenRustcError::InvalidInput {
            field: "rustc",
            message: format!("reported a missing or non-directory sysroot {}", sysroot.display()),
        });
    }
    Ok(sysroot)
}

/// Hash the selected compiler and the bounded sysroot closure that can affect one direct `rlib` compilation.
///
/// Package names, versions and installation locations are deliberately absent. The projection contains the invoked
/// compiler bytes, its actual sysroot compiler when distinct, the driver/LLVM libraries beside that compiler, and
/// the host/target Rust libraries selected by the invocation. Other installed targets are not members.
pub(crate) fn direct_rustc_compiler_evidence(
    rustc: &Path,
    target: &str,
) -> Result<OvenDirectRustcCompilerEvidence, OvenRustcError> {
    const MAX_MEMBERS: usize = 8_192;
    const MAX_BYTES: u64 = 16 * 1024 * 1024 * 1024;

    if target.trim().is_empty() {
        return Err(OvenRustcError::InvalidInput {
            field: "JEC compiler target",
            message: "must not be empty".to_string(),
        });
    }
    let rustc = fs::canonicalize(verified_regular_file(rustc, "rustc")?).map_err(|source| OvenRustcError::Io {
        path: rustc.to_path_buf(),
        source,
    })?;
    let host = rustc_host_target(&rustc)?;
    let sysroot = fs::canonicalize(rustc_sysroot(&rustc)?).map_err(|source| OvenRustcError::Io {
        path: rustc.clone(),
        source,
    })?;
    let binary_digest = digest_regular_file(&rustc, "rustc")?;
    let sysroot_rustc =
        fs::canonicalize(verified_regular_file(&sysroot.join("bin/rustc"), "sysroot rustc")?).map_err(|source| {
            OvenRustcError::Io {
                path: sysroot.join("bin/rustc"),
                source,
            }
        })?;
    if sysroot_rustc != rustc {
        return Err(OvenRustcError::InvalidInput {
            field: "JEC compiler closure",
            message: "requires the invoked compiler to be the selected sysroot's own bin/rustc".to_string(),
        });
    }
    let mut members = BTreeMap::from([("bin/rustc".to_string(), (rustc.clone(), binary_digest.clone()))]);
    let mut total_bytes = fs::metadata(&rustc)
        .map_err(|source| OvenRustcError::Io {
            path: rustc.clone(),
            source,
        })?
        .len();

    collect_compiler_closure_directory(
        &sysroot.join("lib"),
        "lib",
        false,
        &mut members,
        &mut total_bytes,
        MAX_MEMBERS,
        MAX_BYTES,
    )?;
    let mut targets = BTreeSet::from([host.clone(), target.to_string()]);
    for selected in std::mem::take(&mut targets) {
        let root = sysroot.join("lib/rustlib").join(&selected);
        for child in ["lib", "codegen-backends"] {
            let path = root.join(child);
            match fs::symlink_metadata(&path) {
                Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
                    collect_compiler_closure_directory(
                        &path,
                        &format!("lib/rustlib/{selected}/{child}"),
                        true,
                        &mut members,
                        &mut total_bytes,
                        MAX_MEMBERS,
                        MAX_BYTES,
                    )?;
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound && child == "codegen-backends" => {}
                Err(error) => {
                    return Err(OvenRustcError::Io { path, source: error });
                }
                _ => {
                    return Err(OvenRustcError::InvalidInput {
                        field: "JEC compiler closure",
                        message: format!("{} is not a non-symlink directory", path.display()),
                    });
                }
            }
        }
    }
    let member_digests = members
        .iter()
        .map(|(path, (_, digest))| (path.clone(), digest.clone()))
        .collect::<BTreeMap<_, _>>();
    let closure_digest = direct_rustc_compiler_closure_digest(&member_digests)?;
    let members = members
        .into_iter()
        .map(|(relative_path, (source_path, digest))| OvenDirectRustcCompilerMember {
            relative_path,
            source_path,
            digest,
        })
        .collect();
    Ok(OvenDirectRustcCompilerEvidence {
        binary_digest,
        closure_digest,
        host,
        target: target.to_string(),
        sysroot,
        members,
    })
}

fn direct_rustc_compiler_closure_digest(members: &BTreeMap<String, String>) -> Result<String, OvenRustcError> {
    let material = serde_json::to_vec(&("incan.oven.rustc-rlib-closure/1", members)).map_err(|error| {
        OvenRustcError::InvalidInput {
            field: "JEC compiler closure",
            message: format!("cannot encode compiler-member identities: {error}"),
        }
    })?;
    Ok(digest_bytes(&material))
}

fn direct_rustc_compiler_domain(closure_digest: &str) -> Result<String, OvenRustcError> {
    let Some(hex) = closure_digest.strip_prefix("sha256:") else {
        return Err(OvenRustcError::InvalidInput {
            field: "JEC compiler closure",
            message: "has no SHA-256 identity prefix".to_string(),
        });
    };
    if hex.len() != 64
        || !hex
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(OvenRustcError::InvalidInput {
            field: "JEC compiler closure",
            message: "has a malformed SHA-256 identity".to_string(),
        });
    }
    Ok(format!("{OVEN_DIRECT_RUSTC_COMPILER_DOMAIN_PREFIX}.{hex}"))
}

/// Select or materialize one compiler closure in the immutable Store and retain its lease for native execution.
///
/// Discovery, capacity or Store availability failures disable JEC and leave ordinary deterministic direct-Rustc
/// compilation available. A single compatible Store closure is verified and preferred before the ambient sysroot is
/// hashed. Multiple byte-distinct closures with the same host/target/toolchain identity force a cold observation so
/// the ambient compiler bytes disambiguate them.
///
/// Once selected, that verified Store closure is Oven's compiler authority for the batch. The caller's mutable Rustc
/// path proposes the host/toolchain identity and supplies bytes only for a cold or ambiguous selection; it is never
/// executed in place after a warm owner has been admitted.
pub(crate) fn retain_direct_rustc_compiler(
    store: &OvenStore,
    receipt: &OvenReceipt,
    candidate_rustc: &Path,
    target: &str,
) -> Result<OvenDirectRustcCompilerRetention, OvenRustcError> {
    receipt
        .verify_identity()
        .map_err(|error| OvenRustcError::InvalidInput {
            field: "JEC compiler receipt",
            message: error.to_string(),
        })?;
    if receipt.intent.target != target {
        return Err(OvenRustcError::IntentMismatch);
    }
    let toolchain = rustc_identity(candidate_rustc)?;
    if receipt.intent.toolchain != toolchain {
        return Err(OvenRustcError::ToolchainMismatch {
            expected: receipt.intent.toolchain.clone(),
            actual: toolchain,
        });
    }
    let host = rustc_host_target(candidate_rustc)?;
    let mut selected = match store.select_payloads_matching_for_execution(|manifest| {
        manifest.kind == OvenArtifactKind::NativeCompilerClosure
            && manifest.intent.target == target
            && manifest.intent.toolchain == toolchain
    }) {
        Ok(mut selected) => {
            selected.sort_by(|left, right| left.manifest.identity.cmp(&right.manifest.identity));
            selected
        }
        Err(error) => {
            return Ok(OvenDirectRustcCompilerRetention::Unavailable {
                reason: format!("cannot inspect retained compiler closures: {error}"),
            });
        }
    };
    let compatible = selected
        .iter()
        .enumerate()
        .filter_map(|(index, owner)| {
            matching_direct_rustc_compiler_owner_payload(owner, target, &toolchain, &host)
                .map(|payload| (index, payload))
        })
        .collect::<Vec<_>>();
    let closure_digests = compatible
        .iter()
        .map(|(_, payload)| payload.closure_digest.as_str())
        .collect::<BTreeSet<_>>();
    if closure_digests.len() == 1
        && let Some((index, _)) = compatible.first()
    {
        let owner = selected.remove(*index);
        if let Some(owned) = admit_direct_rustc_compiler_owner(owner, target, &toolchain, &host, None)? {
            return Ok(OvenDirectRustcCompilerRetention::Retained(owned));
        }
    }

    let evidence = match direct_rustc_compiler_evidence(candidate_rustc, target) {
        Ok(evidence) => evidence,
        Err(error) => {
            return Ok(OvenDirectRustcCompilerRetention::Unavailable {
                reason: format!("cannot admit the selected compiler closure: {error}"),
            });
        }
    };
    if evidence.host != host {
        return Ok(OvenDirectRustcCompilerRetention::Unavailable {
            reason: "the selected compiler changed its reported host during closure observation".to_string(),
        });
    }
    let payload = OvenDirectRustcCompilerOwnerPayload {
        schema_version: OVEN_DIRECT_RUSTC_COMPILER_OWNER_SCHEMA_VERSION,
        binary_digest: evidence.binary_digest.clone(),
        closure_digest: evidence.closure_digest.clone(),
        host: evidence.host.clone(),
        target: evidence.target.clone(),
        toolchain: receipt.intent.toolchain.clone(),
    };
    let domain = direct_rustc_compiler_domain(&evidence.closure_digest)?;
    if let Some(index) = selected.iter().position(|owner| {
        matching_direct_rustc_compiler_owner_payload(owner, target, &toolchain, &host).as_ref() == Some(&payload)
    }) {
        let owner = selected.remove(index);
        if let Some(owned) = admit_direct_rustc_compiler_owner(owner, target, &toolchain, &host, Some(&evidence))? {
            return Ok(OvenDirectRustcCompilerRetention::Retained(owned));
        }
    }

    let encoded = serde_json::to_vec(&payload).map_err(|error| OvenRustcError::InvalidInput {
        field: "JEC compiler owner",
        message: format!("cannot encode compiler closure metadata: {error}"),
    })?;
    let materialized_files = evidence
        .members
        .iter()
        .map(|member| OvenArtifactMaterializedFile {
            source_path: member.source_path.clone(),
            relative_path: member.relative_path.clone(),
        })
        .collect();
    let manifest = match store.publish(&OvenArtifactPublishRequest {
        receipt: receipt.clone(),
        domain: domain.clone(),
        kind: OvenArtifactKind::NativeCompilerClosure,
        payload: encoded,
        materialized_files,
    }) {
        Ok(manifest) => manifest,
        Err(error) => {
            return Ok(OvenDirectRustcCompilerRetention::Unavailable {
                reason: format!("cannot publish the selected compiler closure: {error}"),
            });
        }
    };
    let mut selected = match store.select_payloads_for_execution(std::slice::from_ref(&manifest.identity)) {
        Ok(selected) => selected,
        Err(error) => {
            return Ok(OvenDirectRustcCompilerRetention::Unavailable {
                reason: format!("cannot retain the published compiler closure: {error}"),
            });
        }
    };
    let Some(owner) = selected.pop() else {
        return Ok(OvenDirectRustcCompilerRetention::Unavailable {
            reason: "the published compiler closure has no execution owner".to_string(),
        });
    };
    match admit_direct_rustc_compiler_owner(owner, target, &toolchain, &host, Some(&evidence))? {
        Some(owned) => Ok(OvenDirectRustcCompilerRetention::Retained(owned)),
        None => Ok(OvenDirectRustcCompilerRetention::Unavailable {
            reason: "the published compiler closure failed final admission".to_string(),
        }),
    }
}

/// Decode the small owner descriptor used to select a warm compiler without reading its materialized closure.
fn matching_direct_rustc_compiler_owner_payload(
    owner: &OvenStoreExecutionPayload,
    target: &str,
    toolchain: &str,
    host: &str,
) -> Option<OvenDirectRustcCompilerOwnerPayload> {
    if owner.manifest.kind != OvenArtifactKind::NativeCompilerClosure
        || owner.manifest.intent.target != target
        || owner.manifest.intent.toolchain != toolchain
    {
        return None;
    }
    let payload = serde_json::from_slice::<OvenDirectRustcCompilerOwnerPayload>(&owner.payload).ok()?;
    if payload.schema_version != OVEN_DIRECT_RUSTC_COMPILER_OWNER_SCHEMA_VERSION
        || payload.target != target
        || payload.toolchain != toolchain
        || payload.host != host
        || direct_rustc_compiler_domain(&payload.closure_digest).ok().as_deref() != Some(owner.manifest.domain.as_str())
    {
        return None;
    }
    Some(payload)
}

fn admit_direct_rustc_compiler_owner(
    owner: OvenStoreExecutionPayload,
    target: &str,
    toolchain: &str,
    host: &str,
    expected: Option<&OvenDirectRustcCompilerEvidence>,
) -> Result<Option<OvenOwnedDirectRustcCompiler>, OvenRustcError> {
    let Some(payload) = matching_direct_rustc_compiler_owner_payload(&owner, target, toolchain, host) else {
        return Ok(None);
    };
    if expected.is_some_and(|expected| {
        payload.binary_digest != expected.binary_digest
            || payload.closure_digest != expected.closure_digest
            || payload.host != expected.host
            || payload.target != expected.target
    }) {
        return Ok(None);
    }
    if owner.verify_materialized_files().is_err() {
        return Ok(None);
    }
    let mut member_digests = BTreeMap::new();
    let mut members = Vec::with_capacity(owner.manifest.materialized_files.len());
    let mut rustc = None;
    for member in &owner.manifest.materialized_files {
        if member_digests
            .insert(member.relative_path.clone(), member.digest.clone())
            .is_some()
        {
            return Ok(None);
        }
        let source_path = owner.artifact_root.join(&member.relative_path);
        if member.relative_path == "bin/rustc" {
            if !member.executable || member.digest != payload.binary_digest {
                return Ok(None);
            }
            rustc = Some(source_path.clone());
        }
        members.push(OvenDirectRustcCompilerMember {
            relative_path: member.relative_path.clone(),
            source_path,
            digest: member.digest.clone(),
        });
    }
    if direct_rustc_compiler_closure_digest(&member_digests)? != payload.closure_digest {
        return Ok(None);
    }
    let Some(rustc) = rustc else {
        return Ok(None);
    };
    let sysroot = match fs::canonicalize(&owner.artifact_root) {
        Ok(sysroot) => sysroot,
        Err(_) => return Ok(None),
    };
    if fs::canonicalize(rustc_sysroot(&rustc)?).ok().as_deref() != Some(sysroot.as_path())
        || rustc_host_target(&rustc)? != payload.host
        || rustc_identity(&rustc)? != payload.toolchain
    {
        return Ok(None);
    }
    Ok(Some(OvenOwnedDirectRustcCompiler {
        _owner: owner,
        evidence: OvenDirectRustcCompilerEvidence {
            binary_digest: payload.binary_digest.clone(),
            closure_digest: payload.closure_digest.clone(),
            host: payload.host.clone(),
            target: payload.target.clone(),
            sysroot,
            members,
        },
        rustc,
    }))
}

#[allow(clippy::too_many_arguments)]
fn collect_compiler_closure_directory(
    root: &Path,
    logical_root: &str,
    recursive: bool,
    members: &mut BTreeMap<String, (PathBuf, String)>,
    total_bytes: &mut u64,
    max_members: usize,
    max_bytes: u64,
) -> Result<(), OvenRustcError> {
    let mut entries = fs::read_dir(root)
        .map_err(|source| OvenRustcError::Io {
            path: root.to_path_buf(),
            source,
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|source| OvenRustcError::Io {
            path: root.to_path_buf(),
            source,
        })?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| OvenRustcError::InvalidInput {
                field: "JEC compiler closure",
                message: "contains a non-UTF-8 member name".to_string(),
            })?;
        let logical = format!("{logical_root}/{name}");
        let metadata = fs::symlink_metadata(&path).map_err(|source| OvenRustcError::Io {
            path: path.clone(),
            source,
        })?;
        if metadata.file_type().is_symlink() {
            return Err(OvenRustcError::InvalidInput {
                field: "JEC compiler closure",
                message: format!("contains a symlink at {logical}"),
            });
        }
        if metadata.is_file() {
            collect_compiler_closure_file(&path, logical, members, total_bytes, max_members, max_bytes)?;
        } else if metadata.is_dir() && recursive {
            collect_compiler_closure_directory(&path, &logical, true, members, total_bytes, max_members, max_bytes)?;
        }
    }
    Ok(())
}

fn collect_compiler_closure_file(
    path: &Path,
    logical: String,
    members: &mut BTreeMap<String, (PathBuf, String)>,
    total_bytes: &mut u64,
    max_members: usize,
    max_bytes: u64,
) -> Result<(), OvenRustcError> {
    let metadata = fs::symlink_metadata(path).map_err(|source| OvenRustcError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(OvenRustcError::InvalidArtifactPath {
            kind: "JEC compiler closure",
            path: path.to_path_buf(),
            message: "must contain only non-symlink regular files".to_string(),
        });
    }
    *total_bytes = total_bytes
        .checked_add(metadata.len())
        .ok_or_else(|| OvenRustcError::InvalidInput {
            field: "JEC compiler closure",
            message: "byte count overflowed".to_string(),
        })?;
    if members.len() >= max_members || *total_bytes > max_bytes {
        return Err(OvenRustcError::InvalidInput {
            field: "JEC compiler closure",
            message: "exceeds its bounded member or byte budget".to_string(),
        });
    }
    let digest = digest_regular_file(path, "JEC compiler closure member")?;
    if members.insert(logical, (path.to_path_buf(), digest)).is_some() {
        return Err(OvenRustcError::InvalidInput {
            field: "JEC compiler closure",
            message: "contains a duplicate logical member".to_string(),
        });
    }
    Ok(())
}

/// Resolve the Rustdoc executable from the same verified sysroot as a receipt-selected compiler.
pub(crate) fn rustdoc_for_rustc(rustc: &Path) -> Result<PathBuf, OvenRustcError> {
    let rustdoc = rustc_sysroot(rustc)?.join("bin/rustdoc");
    verified_regular_file(&rustdoc, "rustdoc")
}

/// Derive the selected compiler's dynamic-library search environment without consulting Cargo.
///
/// Direct `--test` compilation of a proc-macro crate uses Cargo's `-C prefer-dynamic` convention. Its caller-owned
/// test binary must therefore receive the matching toolchain standard-library directory for both inventory and test
/// execution; ambient Cargo-provided dynamic-library state is not trusted.
pub(crate) fn rustc_dynamic_library_environment(rustc: &Path) -> Result<(String, String), OvenRustcError> {
    let sysroot = rustc_sysroot(rustc)?;
    let host_target = rustc_host_target(rustc)?;
    let target_libraries = sysroot.join("lib/rustlib").join(host_target).join("lib");
    let toolchain_libraries = sysroot.join("lib");
    if !target_libraries.is_dir() || !toolchain_libraries.is_dir() {
        return Err(OvenRustcError::InvalidInput {
            field: "rustc",
            message: format!(
                "sysroot {} does not contain direct-test dynamic library directories",
                sysroot.display()
            ),
        });
    }
    let value =
        env::join_paths([target_libraries, toolchain_libraries]).map_err(|error| OvenRustcError::InvalidInput {
            field: "rustc",
            message: format!("cannot construct direct-test dynamic library search path: {error}"),
        })?;
    let value = value.into_string().map_err(|_| OvenRustcError::InvalidInput {
        field: "rustc",
        message: "direct-test dynamic library search path is not valid UTF-8".to_string(),
    })?;
    let key = if cfg!(target_os = "macos") {
        "DYLD_FALLBACK_LIBRARY_PATH"
    } else if cfg!(target_os = "windows") {
        "PATH"
    } else {
        "LD_LIBRARY_PATH"
    };
    Ok((key.to_string(), value))
}

/// Extend the receipt-selected dynamic toolchain closure with caller-owned direct-Rustc dynamic-library directories.
///
/// Rustdoc owns its generated runner binary and launches it before Oven can attach a separate environment. The
/// caller-owned paths are already validated by the direct-Rustc plan, so they are safe to transport alongside the
/// selected toolchain directories rather than relying on Cargo's ambient loader setup. Store artifact directories
/// are deliberately excluded: their opaque `sha256:` identities are not valid Unix path-list segments and Rustdoc
/// already receives them as individual direct compiler search paths.
fn rustc_dynamic_library_environment_with_caller_owned_paths(
    rustc: &Path,
    plan: &OvenRustcArtifactPlan,
) -> Result<(String, String), OvenRustcError> {
    let (name, toolchain_value) = rustc_dynamic_library_environment(rustc)?;
    let mut paths = BTreeSet::new();
    for (crate_name, artifact) in &plan.externs {
        if !plan.caller_owned_library_digests.contains_key(crate_name) {
            continue;
        }
        let extension = artifact.extension().and_then(|extension| extension.to_str());
        if !matches!(extension, Some("dylib" | "so" | "dll")) {
            continue;
        }
        let parent = artifact.parent().ok_or_else(|| OvenRustcError::InvalidInput {
            field: "caller-owned dynamic library",
            message: format!("{} has no parent directory", artifact.display()),
        })?;
        paths.insert(parent.to_path_buf());
    }
    paths.extend(env::split_paths(&toolchain_value));
    let value = env::join_paths(paths)
        .map_err(|error| OvenRustcError::InvalidInput {
            field: "rustc dynamic library environment",
            message: format!("cannot construct direct-Rustc dynamic library search path: {error}"),
        })?
        .into_string()
        .map_err(|_| OvenRustcError::InvalidInput {
            field: "rustc dynamic library environment",
            message: "direct-Rustc dynamic library search path is not valid UTF-8".to_string(),
        })?;
    Ok((name, value))
}

/// Collect every manifest-recorded artifact by safe relative path for directory completeness checks.
fn expected_artifacts(manifest: &OvenRustcArtifactManifest) -> Result<BTreeMap<String, String>, OvenRustcError> {
    let mut expected = BTreeMap::new();
    let mut sources = BTreeMap::new();
    for (source, artifact) in manifest
        .externs
        .iter()
        .map(|artifact| ("externs", (artifact.relative_path.as_str(), artifact.digest.as_str())))
        .chain(manifest.supporting_artifacts.iter().map(|artifact| {
            (
                "supporting",
                (artifact.relative_path.as_str(), artifact.digest.as_str()),
            )
        }))
        .chain(manifest.vocab_auxiliary_targets.iter().flat_map(|auxiliary| {
            auxiliary.externs.iter().map(|artifact| {
                (
                    "vocab-auxiliary",
                    (artifact.relative_path.as_str(), artifact.digest.as_str()),
                )
            })
        }))
    {
        let normalized = normalized_relative_path(artifact.0, "artifact")?;
        if expected.insert(normalized.clone(), artifact.1.to_string()).is_some() {
            let previous = sources.get(&normalized).copied().unwrap_or("unknown");
            return Err(OvenRustcError::InvalidInput {
                field: "artifact manifest",
                message: format!(
                    "declares one relative artifact path more than once: `{}` ({previous} and {source})",
                    artifact.0
                ),
            });
        }
        sources.insert(normalized, source);
    }
    Ok(expected)
}

/// Verify declared rustc search directories and refuse any regular file not named in the manifest.
fn materialize_search_paths(
    root: &Path,
    paths: &[String],
    kind: &'static str,
    expected: &BTreeMap<String, String>,
) -> Result<Vec<PathBuf>, OvenRustcError> {
    let mut materialized = Vec::new();
    let mut seen = BTreeSet::new();
    for relative in paths {
        let normalized = normalized_relative_path(relative, kind)?;
        if !seen.insert(normalized.clone()) {
            return Err(OvenRustcError::InvalidInput {
                field: "artifact manifest",
                message: format!("declares duplicate {kind} path `{relative}`"),
            });
        }
        let path = safe_path(root, &normalized, kind)?;
        let metadata = fs::symlink_metadata(&path).map_err(|source| OvenRustcError::Io {
            path: path.clone(),
            source,
        })?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(OvenRustcError::InvalidArtifactPath {
                kind,
                path,
                message: "must be a non-symlink directory".to_string(),
            });
        }
        let mut files_in_directory = 0_u64;
        for child in fs::read_dir(&path).map_err(|source| OvenRustcError::Io {
            path: path.clone(),
            source,
        })? {
            let child = child.map_err(|source| OvenRustcError::Io {
                path: path.clone(),
                source,
            })?;
            let child_path = child.path();
            let child_metadata = fs::symlink_metadata(&child_path).map_err(|source| OvenRustcError::Io {
                path: child_path.clone(),
                source,
            })?;
            if !child_metadata.is_file() || child_metadata.file_type().is_symlink() {
                return Err(OvenRustcError::InvalidArtifactPath {
                    kind,
                    path: child_path,
                    message: "search directories may contain only regular files".to_string(),
                });
            }
            let relative_child = child_path
                .strip_prefix(root)
                .map_err(|_| OvenRustcError::InvalidArtifactPath {
                    kind,
                    path: child_path.clone(),
                    message: "resolved path escaped artifact root".to_string(),
                })?
                .to_string_lossy()
                .replace('\\', "/");
            let digest = expected
                .get(&relative_child)
                .ok_or_else(|| OvenRustcError::UnrecordedSearchArtifact {
                    path: child_path.clone(),
                })?;
            let actual = digest_bytes(&fs::read(&child_path).map_err(|source| OvenRustcError::Io {
                path: child_path.clone(),
                source,
            })?);
            if digest != &actual {
                return Err(OvenRustcError::ArtifactDigestMismatch {
                    path: child_path,
                    expected: digest.clone(),
                    actual,
                });
            }
            files_in_directory = files_in_directory.saturating_add(1);
        }
        if files_in_directory == 0 {
            return Err(OvenRustcError::InvalidArtifactPath {
                kind,
                path,
                message: "must contain at least one manifest-recorded regular file".to_string(),
            });
        }
        materialized.push(path);
    }
    Ok(materialized)
}

/// Verify a selected store-owned search directory without repeating publisher-time closure enumeration.
///
/// Publisher-time materialization verifies every child and its digest. A normal consumer proves the selected
/// directory itself is non-symlinked and contains at least one declared artifact, then separately validates every
/// manifest-declared file it can hand to Rustc. Re-enumerating every unrelated child here would make prepared
/// direct-Rustc selection behave like a cold whole-closure audit.
fn trusted_materialize_search_paths(
    root: &Path,
    paths: &[String],
    kind: &'static str,
    expected: &BTreeMap<String, String>,
    trusted_parents: &mut BTreeMap<PathBuf, PathBuf>,
) -> Result<Vec<PathBuf>, OvenRustcError> {
    let mut materialized = Vec::new();
    let mut seen = BTreeSet::new();
    for relative in paths {
        let normalized = normalized_relative_path(relative, kind)?;
        if !seen.insert(normalized.clone()) {
            return Err(OvenRustcError::InvalidInput {
                field: "artifact manifest",
                message: format!("declares duplicate {kind} path `{relative}`"),
            });
        }
        let path = trusted_safe_path(root, &normalized, kind, trusted_parents)?;
        let metadata = fs::symlink_metadata(&path).map_err(|source| OvenRustcError::Io {
            path: path.clone(),
            source,
        })?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(OvenRustcError::InvalidArtifactPath {
                kind,
                path,
                message: "must be a non-symlink directory".to_string(),
            });
        }
        if !expected
            .keys()
            .any(|artifact| Path::new(artifact).starts_with(Path::new(&normalized)))
        {
            return Err(OvenRustcError::InvalidArtifactPath {
                kind,
                path,
                message: "must contain at least one manifest-recorded regular file".to_string(),
            });
        }
        materialized.push(path);
    }
    Ok(materialized)
}

/// Verify one manifest artifact file and return its canonical artifact-root-contained path.
fn verified_file(
    root: &Path,
    relative: &str,
    expected_digest: &str,
    kind: &'static str,
) -> Result<PathBuf, OvenRustcError> {
    let relative = normalized_relative_path(relative, kind)?;
    let path = safe_path(root, &relative, kind)?;
    let metadata = fs::symlink_metadata(&path).map_err(|source| OvenRustcError::Io {
        path: path.clone(),
        source,
    })?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(OvenRustcError::InvalidArtifactPath {
            kind,
            path,
            message: "must be a non-symlink regular file".to_string(),
        });
    }
    let actual = digest_bytes(&fs::read(&path).map_err(|source| OvenRustcError::Io {
        path: path.clone(),
        source,
    })?);
    if expected_digest != actual {
        return Err(OvenRustcError::ArtifactDigestMismatch {
            path,
            expected: expected_digest.to_string(),
            actual,
        });
    }
    Ok(path)
}

/// Return a safe regular store artifact without repeating its publisher-verified content digest.
fn trusted_file(
    root: &Path,
    relative: &str,
    kind: &'static str,
    trusted_parents: &mut BTreeMap<PathBuf, PathBuf>,
) -> Result<PathBuf, OvenRustcError> {
    let relative = normalized_relative_path(relative, kind)?;
    let path = trusted_safe_path(root, &relative, kind, trusted_parents)?;
    let metadata = fs::symlink_metadata(&path).map_err(|source| OvenRustcError::Io {
        path: path.clone(),
        source,
    })?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(OvenRustcError::InvalidArtifactPath {
            kind,
            path,
            message: "must be a non-symlink regular file".to_string(),
        });
    }
    Ok(path)
}

/// Resolve a selected immutable path while canonicalizing each parent directory at most once per plan.
///
/// The generation lock and read-only publisher-owned root make every checked parent stable for the current
/// selection. We still verify that each distinct parent remains beneath the canonical artifact root and validate the
/// requested child as a regular non-symlink file in [`trusted_file`]. This avoids repeating the same filesystem walk
/// for every extern stored below one `deps` directory.
fn trusted_safe_path(
    root: &Path,
    relative: &str,
    kind: &'static str,
    trusted_parents: &mut BTreeMap<PathBuf, PathBuf>,
) -> Result<PathBuf, OvenRustcError> {
    let path = root.join(relative);
    let parent = path.parent().ok_or_else(|| OvenRustcError::InvalidArtifactPath {
        kind,
        path: path.clone(),
        message: "has no parent directory".to_string(),
    })?;
    let canonical_parent = if let Some(parent) = trusted_parents.get(parent) {
        parent.clone()
    } else {
        let canonical_parent = parent.canonicalize().map_err(|source| OvenRustcError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
        trusted_parents.insert(parent.to_path_buf(), canonical_parent.clone());
        canonical_parent
    };
    if !canonical_parent.starts_with(root) {
        return Err(OvenRustcError::InvalidArtifactPath {
            kind,
            path,
            message: "escapes immutable artifact root".to_string(),
        });
    }
    Ok(path)
}

/// Canonicalize an existing artifact root and reject an absent or non-directory root.
fn canonical_directory(path: &Path, kind: &'static str) -> Result<PathBuf, OvenRustcError> {
    let metadata = fs::symlink_metadata(path).map_err(|source| OvenRustcError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(OvenRustcError::InvalidArtifactPath {
            kind,
            path: path.to_path_buf(),
            message: "must be a non-symlink directory".to_string(),
        });
    }
    path.canonicalize().map_err(|source| OvenRustcError::Io {
        path: path.to_path_buf(),
        source,
    })
}

/// Resolve a normalized relative path and prove that its canonical parent remains below the immutable root.
fn safe_path(root: &Path, relative: &str, kind: &'static str) -> Result<PathBuf, OvenRustcError> {
    let path = root.join(relative);
    let parent = path.parent().ok_or_else(|| OvenRustcError::InvalidArtifactPath {
        kind,
        path: path.clone(),
        message: "has no parent directory".to_string(),
    })?;
    let canonical_parent = parent.canonicalize().map_err(|source| OvenRustcError::Io {
        path: parent.to_path_buf(),
        source,
    })?;
    if !canonical_parent.starts_with(root) {
        return Err(OvenRustcError::InvalidArtifactPath {
            kind,
            path,
            message: "escapes immutable artifact root".to_string(),
        });
    }
    Ok(path)
}

/// Normalize one relative artifact path and reject absolute, parent, or platform-prefix components.
fn normalized_relative_path(value: &str, kind: &'static str) -> Result<String, OvenRustcError> {
    let path = Path::new(value);
    if value.trim().is_empty()
        || path.components().any(|component| {
            matches!(
                component,
                Component::Prefix(_) | Component::RootDir | Component::ParentDir | Component::CurDir
            )
        })
    {
        return Err(OvenRustcError::InvalidArtifactPath {
            kind,
            path: PathBuf::from(value),
            message: "must be a non-empty normalized relative path".to_string(),
        });
    }
    Ok(path.to_string_lossy().replace('\\', "/"))
}

/// Validate that a direct-rustc source or output is a non-symlink regular file.
fn verified_regular_file(path: &Path, kind: &'static str) -> Result<PathBuf, OvenRustcError> {
    let metadata = fs::symlink_metadata(path).map_err(|source| OvenRustcError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(OvenRustcError::InvalidArtifactPath {
            kind,
            path: path.to_path_buf(),
            message: "must be a non-symlink regular file".to_string(),
        });
    }
    Ok(path.to_path_buf())
}

/// Read a caller-owned regular file once and return its stable direct-Rustc reuse digest.
fn digest_regular_file(path: &Path, kind: &'static str) -> Result<String, OvenRustcError> {
    let path = verified_regular_file(path, kind)?;
    let bytes = fs::read(&path).map_err(|source| OvenRustcError::Io {
        path: path.clone(),
        source,
    })?;
    Ok(digest_bytes(&bytes))
}

/// Ensure the caller-owned final output cannot become part of the immutable artifact root.
fn caller_output_path(output: &Path, artifact_root: &Path) -> Result<PathBuf, OvenRustcError> {
    if output.as_os_str().is_empty() {
        return Err(OvenRustcError::InvalidInput {
            field: "output",
            message: "must not be empty".to_string(),
        });
    }
    let artifact_root = artifact_root.canonicalize().map_err(|source| OvenRustcError::Io {
        path: artifact_root.to_path_buf(),
        source,
    })?;
    let output_parent = output.parent().ok_or_else(|| OvenRustcError::InvalidInput {
        field: "output",
        message: "must have a parent directory".to_string(),
    })?;
    fs::create_dir_all(output_parent).map_err(|source| OvenRustcError::Io {
        path: output_parent.to_path_buf(),
        source,
    })?;
    let output_parent = output_parent.canonicalize().map_err(|source| OvenRustcError::Io {
        path: output_parent.to_path_buf(),
        source,
    })?;
    let resolved = output_parent.join(output.file_name().ok_or_else(|| OvenRustcError::InvalidInput {
        field: "output",
        message: "must name a file".to_string(),
    })?);
    if resolved.starts_with(&artifact_root) {
        return Err(OvenRustcError::InvalidInput {
            field: "output",
            message: "must remain outside the immutable artifact root".to_string(),
        });
    }
    Ok(resolved)
}

/// Create a caller-owned regular temporary directory without allowing Rustdoc to write into an immutable store entry.
fn caller_temporary_directory(directory: &Path, artifact_root: &Path) -> Result<PathBuf, OvenRustcError> {
    if directory.as_os_str().is_empty() {
        return Err(OvenRustcError::InvalidInput {
            field: "Rustdoc temporary directory",
            message: "must not be empty".to_string(),
        });
    }
    let artifact_root = artifact_root.canonicalize().map_err(|source| OvenRustcError::Io {
        path: artifact_root.to_path_buf(),
        source,
    })?;
    fs::create_dir_all(directory).map_err(|source| OvenRustcError::Io {
        path: directory.to_path_buf(),
        source,
    })?;
    let metadata = fs::symlink_metadata(directory).map_err(|source| OvenRustcError::Io {
        path: directory.to_path_buf(),
        source,
    })?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(OvenRustcError::InvalidArtifactPath {
            kind: "Rustdoc temporary directory",
            path: directory.to_path_buf(),
            message: "must be a non-symlink directory".to_string(),
        });
    }
    let directory = directory.canonicalize().map_err(|source| OvenRustcError::Io {
        path: directory.to_path_buf(),
        source,
    })?;
    if directory.starts_with(&artifact_root) {
        return Err(OvenRustcError::InvalidInput {
            field: "Rustdoc temporary directory",
            message: "must remain outside the immutable artifact root".to_string(),
        });
    }
    Ok(directory)
}

/// Preserve both direct-tool streams for a single actionable Rustdoc failure report.
fn combined_process_output(stdout: &[u8], stderr: &[u8]) -> String {
    format!("{}{}", String::from_utf8_lossy(stdout), String::from_utf8_lossy(stderr))
}

/// Validate the narrow Rust identifier syntax accepted for a direct test crate name.
fn validate_rust_identifier(name: &str) -> Result<(), OvenRustcError> {
    let mut characters = name.chars();
    let Some(first) = characters.next() else {
        return Err(OvenRustcError::InvalidInput {
            field: "crate name",
            message: "must not be empty".to_string(),
        });
    };
    if !(first == '_' || first.is_ascii_alphabetic())
        || !characters.all(|character| character == '_' || character.is_ascii_alphanumeric())
    {
        return Err(OvenRustcError::InvalidInput {
            field: "crate name",
            message: "must use ASCII Rust identifier characters".to_string(),
        });
    }
    Ok(())
}

/// Validate an explicit Rust target triple without accepting command-line syntax or path components.
fn validate_rust_target(target: &str) -> Result<(), OvenRustcError> {
    if target.is_empty()
        || !target
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '-' | '.'))
    {
        return Err(OvenRustcError::InvalidInput {
            field: "vocabulary auxiliary Rust target",
            message: "must use only ASCII target-triple characters".to_string(),
        });
    }
    Ok(())
}

/// Validate editions that the Alpha direct runner explicitly supports.
fn validate_edition(edition: &str) -> Result<(), OvenRustcError> {
    if matches!(edition, "2021" | "2024") {
        return Ok(());
    }
    Err(OvenRustcError::InvalidInput {
        field: "edition",
        message: "must be one of 2021 or 2024".to_string(),
    })
}

/// Decode rustc JSON diagnostics from both captured streams while retaining unstructured lines.
fn parse_rustc_diagnostics(stdout: &[u8], stderr: &[u8]) -> OvenRustcDiagnosticReport {
    let mut diagnostics = Vec::new();
    let mut unstructured = String::new();
    for line in [stdout, stderr].into_iter().flat_map(|stream| {
        String::from_utf8_lossy(stream)
            .lines()
            .map(str::to_owned)
            .collect::<Vec<_>>()
    }) {
        match serde_json::from_str::<RustcJsonDiagnostic>(&line) {
            Ok(record) if record.is_compiler_message() => diagnostics.push(OvenRustcDiagnostic {
                level: record.message.level,
                message: record.message.message,
                code: record.message.code.map(|code| code.code),
                spans: record
                    .message
                    .spans
                    .into_iter()
                    .map(|span| OvenRustcDiagnosticSpan {
                        file_name: span.file_name,
                        line_start: span.line_start,
                        column_start: span.column_start,
                        line_end: span.line_end,
                        column_end: span.column_end,
                        is_primary: span.is_primary,
                    })
                    .collect(),
                rendered: record.message.rendered,
            }),
            _ if line.trim().is_empty() => {}
            _ => {
                unstructured.push_str(&line);
                unstructured.push('\n');
            }
        }
    }
    OvenRustcDiagnosticReport {
        diagnostics,
        unstructured_output: unstructured,
        invocation: None,
    }
}

impl OvenRustcDiagnosticReport {
    /// Attach bounded process evidence to a report after Rustc has exited unsuccessfully.
    fn with_invocation(mut self, command: &Command) -> Self {
        // A legacy-Cargo-published dependency closure with many build-script crates can produce a single-line
        // invocation with hundreds of `-L dependency=...` entries before the first `--extern`. The previous 12,000
        // char bound routinely truncated evidence before any `--extern` flag appeared at all, making failure
        // reports misleading for exactly the invocations most worth diagnosing.
        const MAX_INVOCATION_CHARS: usize = 100_000;

        // Do not render `Command` itself: its debug format may include explicit compile-time environment values.
        // Rustc program and argument evidence is enough to replay the artifact closure without exposing that state.
        let mut rendered = format!("{:?}", command.get_program());
        for argument in command.get_args() {
            rendered.push(' ');
            rendered.push_str(&format!("{argument:?}"));
        }
        let mut invocation = rendered.chars().take(MAX_INVOCATION_CHARS).collect::<String>();
        if rendered.chars().count() > MAX_INVOCATION_CHARS {
            invocation.push_str(" … invocation truncated");
        }
        self.invocation = Some(invocation);
        self
    }
}

/// Minimal rustc JSON envelope for diagnostic preservation.
#[derive(Deserialize)]
struct RustcJsonDiagnostic {
    #[serde(default)]
    reason: String,
    #[serde(default, rename = "$message_type")]
    message_type: String,
    message: RustcJsonMessage,
}

impl RustcJsonDiagnostic {
    /// Cargo wraps rustc diagnostics with `reason`; direct rustc emits the `$message_type` envelope.
    fn is_compiler_message(&self) -> bool {
        self.reason == "compiler-message" || self.message_type == "diagnostic"
    }
}

/// Minimal structured rustc diagnostic message.
#[derive(Deserialize)]
struct RustcJsonMessage {
    level: String,
    message: String,
    code: Option<RustcJsonCode>,
    #[serde(default)]
    spans: Vec<RustcJsonSpan>,
    rendered: Option<String>,
}

/// Minimal rustc error-code representation.
#[derive(Deserialize)]
struct RustcJsonCode {
    code: String,
}

/// Minimal rustc span representation.
#[derive(Deserialize)]
struct RustcJsonSpan {
    file_name: String,
    line_start: u32,
    column_start: u32,
    line_end: u32,
    column_end: u32,
    is_primary: bool,
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};
    use std::ffi::OsString;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    use super::{
        OVEN_PROJECT_INSPECTION_AUTHORITY_SCHEMA_VERSION, OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION,
        OvenCallerOwnedRustcLibrary, OvenDirectRustcTestRequest, OvenProjectInspectionAuthorityPayload,
        OvenProjectInspectionAuthorityRef, OvenProjectInspectionConstituent, OvenProjectInspectionRootDependency,
        OvenProjectInspectionSource, OvenProjectInspectionSourceOwner, OvenProjectInspectionTestDependencyEnvelope,
        OvenProjectInspectionTestDependencyRoot, OvenRegistryLeafAuthority, OvenRustcArtifactExtern,
        OvenRustcArtifactManifest, OvenRustcArtifactPlan, OvenRustcAuxiliaryTarget, OvenRustcError,
        OvenRustcRegistryLeaf, OvenRustcRegistrySource, OvenRustcRegistrySourcePackage, OvenRustcSupportingArtifact,
        OvenStoredDirectRustcRunRequest, OvenStoredDirectRustcTestRequest, OvenTrustedDirectRustcTargetRequest,
        OvenTrustedRustcArtifactRoot, OvenTrustedRustdocTestRequest, apply_oven_profile,
        attach_caller_owned_rustc_libraries, bake_direct_rustc_test, bake_stored_direct_rustc_run,
        bake_stored_direct_rustc_test, bake_trusted_direct_rustc_dylib, bake_trusted_direct_rustc_library,
        bake_trusted_direct_rustc_proc_macro, bake_trusted_direct_rustc_run, bake_trusted_direct_rustc_test,
        combined_process_output, is_host_native_unix_target, load_project_inspection_authority,
        project_inspection_authority_supports_dependencies, project_inspection_constituent_matches_receipt,
        project_inspection_test_dependency_envelope_supports_dependencies, resolve_sealed_registry_leaf,
        run_trusted_rustdoc_test, rustc_dynamic_library_environment, rustc_host_target,
        select_direct_rustc_plan_identity, validate_project_extension_payload_against_base,
        validate_project_inspection_authority_payload,
    };
    use crate::manifest::{DependencySource, DependencySpec};
    use crate::oven::native_contract::{
        OVEN_PROJECT_EXTENSION_PAYLOAD_SCHEMA_VERSION, OvenProjectExtensionPayload, OvenProjectRegistrySourceDependency,
    };
    use crate::oven::native_test::run_native_test_batch_all;
    use crate::oven::store::{
        OvenArtifactKind, OvenArtifactMaterializedFile, OvenArtifactPublishRequest, OvenStore, OvenStoreLimits,
    };
    use crate::oven::{
        OVEN_COMPILER_TEST_PROFILE, OvenGeneratedProjectRequest, digest_bytes, receipt_generated_project,
    };

    #[test]
    fn jec_frozen_environment_contains_only_admitted_plan_values() -> Result<(), Box<dyn std::error::Error>> {
        let inherited = [
            (OsString::from("CARGO"), OsString::from("cargo")),
            (OsString::from("CARGO_HOME"), OsString::from("cargo-home")),
            (OsString::from("RUSTC_WRAPPER"), OsString::from("wrapper")),
            (
                OsString::from("RUSTC_WORKSPACE_WRAPPER"),
                OsString::from("workspace-wrapper"),
            ),
            (OsString::from("RUSTFLAGS"), OsString::from("flags")),
            (OsString::from("RUSTC_BOOTSTRAP"), OsString::from("1")),
            (OsString::from("DYLD_LIBRARY_PATH"), OsString::from("ambient-loader")),
            (OsString::from("KEEP"), OsString::from("ambient")),
            (OsString::from("OVERRIDE"), OsString::from("ambient")),
        ];
        let compile_environment = BTreeMap::from([
            ("CARGO_EXPLICIT".to_string(), PathBuf::from("admitted")),
            ("OVERRIDE".to_string(), PathBuf::from("plan")),
        ]);

        let frozen = super::freeze_direct_rustc_environment(inherited, &compile_environment)
            .ok_or("test environment was not representable")?;

        assert_eq!(frozen.get("OVERRIDE"), Some(&PathBuf::from("plan")));
        assert_eq!(frozen.get("CARGO_EXPLICIT"), Some(&PathBuf::from("admitted")));
        for ambient in [
            "CARGO",
            "CARGO_HOME",
            "RUSTC_WRAPPER",
            "RUSTC_WORKSPACE_WRAPPER",
            "RUSTFLAGS",
            "RUSTC_BOOTSTRAP",
            "DYLD_LIBRARY_PATH",
            "KEEP",
        ] {
            assert!(!frozen.contains_key(ambient));
        }
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn jec_ignores_unrepresentable_ambient_names_and_clears_ambient_execution() -> Result<(), Box<dyn std::error::Error>>
    {
        use std::os::unix::ffi::OsStringExt;

        let inherited = [(OsString::from_vec(vec![0xff]), OsString::from("value"))];
        assert_eq!(
            super::freeze_direct_rustc_environment(inherited, &BTreeMap::new()),
            Some(BTreeMap::new())
        );
        let mut command = Command::new("/usr/bin/env");
        command.env("AMBIENT_VALUE", "ambient");
        super::apply_direct_rustc_execution_environment(&mut command, Some(&BTreeMap::new()))?;
        let output = String::from_utf8(command.output()?.stdout)?;
        assert!(!output.lines().any(|line| line.starts_with("AMBIENT_VALUE=")));
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn jec_execution_environment_is_identical_with_or_without_cache_observation()
    -> Result<(), Box<dyn std::error::Error>> {
        let compile_environment = BTreeMap::from([("PLAN_VALUE".to_string(), PathBuf::from("plan"))]);
        let frozen_environment = super::freeze_direct_rustc_environment(
            [(OsString::from("FROZEN_VALUE"), OsString::from("frozen"))],
            &compile_environment,
        )
        .ok_or("test environment was not representable")?;

        let mut cacheable = Command::new("/usr/bin/env");
        cacheable.env("LEGACY_VALUE", "ambient");
        super::apply_direct_rustc_execution_environment(&mut cacheable, Some(&frozen_environment))?;
        let cacheable_output = String::from_utf8(cacheable.output()?.stdout)?;
        assert!(cacheable_output.lines().any(|line| line == "PLAN_VALUE=plan"));
        assert!(!cacheable_output.lines().any(|line| line.starts_with("FROZEN_VALUE=")));
        assert!(!cacheable_output.lines().any(|line| line.starts_with("LEGACY_VALUE=")));

        let mut uncacheable = Command::new("/usr/bin/env");
        uncacheable.env("LEGACY_VALUE", "ambient");
        super::apply_direct_rustc_execution_environment(&mut uncacheable, Some(&frozen_environment))?;
        let uncacheable_output = String::from_utf8(uncacheable.output()?.stdout)?;
        assert!(uncacheable_output.lines().any(|line| line == "PLAN_VALUE=plan"));
        assert!(!uncacheable_output.lines().any(|line| line.starts_with("FROZEN_VALUE=")));
        assert!(!uncacheable_output.lines().any(|line| line.starts_with("LEGACY_VALUE=")));
        assert_eq!(cacheable_output, uncacheable_output);
        Ok(())
    }

    #[test]
    fn jec_dep_info_keeps_only_logical_files_and_secret_digests() -> Result<(), Box<dyn std::error::Error>> {
        let environment = BTreeMap::from([("PRIVATE_VALUE".to_string(), PathBuf::from("do-not-retain-me"))]);
        let bytes = b"libfixture.rlib: incan-source/src/lib.rs incan-source/src/space\\ file.rs \\\n incan-source/src/dollar$$file.rs\n\n# env-dep:ABSENT\n# env-dep:PRIVATE_VALUE=do-not-retain-me\n";
        let observed = super::parse_rustc_dep_info(bytes, &environment)?;
        assert_eq!(
            observed.files,
            [
                PathBuf::from("incan-source/src/dollar$file.rs"),
                PathBuf::from("incan-source/src/lib.rs"),
                PathBuf::from("incan-source/src/space file.rs"),
            ]
        );
        assert_eq!(observed.environment.len(), 2);
        assert_eq!(observed.environment[0].name, "ABSENT");
        assert_eq!(observed.environment[0].value_digest, None);
        assert_eq!(observed.environment[1].name, "PRIVATE_VALUE");
        assert_eq!(
            observed.environment[1].value_digest.as_deref(),
            Some(digest_bytes(b"do-not-retain-me").as_str())
        );
        assert!(!format!("{observed:?}").contains("do-not-retain-me"));

        let mismatched = b"libfixture.rlib: incan-source/src/lib.rs\n# env-dep:PRIVATE_VALUE=different\n";
        assert!(super::parse_rustc_dep_info(mismatched, &environment).is_err());
        Ok(())
    }

    #[test]
    fn jec_compiler_evidence_hashes_the_selected_target_closure() -> Result<(), Box<dyn std::error::Error>> {
        let rustc = rustc_path()?;
        let target = rustc_host_target(&rustc)?;
        let evidence = super::direct_rustc_compiler_evidence(&rustc, &target)?;
        assert_eq!(evidence.host(), target);
        assert_eq!(evidence.target(), target);
        assert_eq!(evidence.binary_digest(), digest_bytes(&fs::read(&rustc)?));
        assert!(evidence.closure_digest().starts_with("sha256:"));
        assert_eq!(evidence.closure_digest().len(), 71);
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn jec_compiler_owner_is_reused_across_receipts_and_lease_protected() -> Result<(), Box<dyn std::error::Error>> {
        use std::os::unix::fs::PermissionsExt;

        let root = tempfile::tempdir()?;
        let sysroot = root.path().join("toolchain");
        let rustc = sysroot.join("bin/rustc");
        fs::create_dir_all(sysroot.join("bin"))?;
        fs::create_dir_all(sysroot.join("lib/rustlib/test-target/lib"))?;
        fs::write(
            &rustc,
            r#"#!/bin/sh
if [ "$1" = "--print" ] && [ "$2" = "sysroot" ]; then
  cd "$(dirname "$0")/.." || exit 1
  pwd -P
elif [ "$1" = "--version" ]; then
  printf '%s\n' 'rustc 1.99.0-test'
elif [ "$1" = "-vV" ]; then
  printf '%s\n' 'rustc 1.99.0-test' 'host: test-target'
else
  exit 2
fi
"#,
        )?;
        fs::set_permissions(&rustc, fs::Permissions::from_mode(0o755))?;
        let ambient_driver = sysroot.join("lib/libdriver.dylib");
        fs::write(&ambient_driver, b"driver bytes")?;
        fs::write(
            sysroot.join("lib/rustlib/test-target/lib/libstd-test.rlib"),
            b"standard-library bytes",
        )?;
        let source = root.path().join("generated.rs");
        fs::write(&source, "pub fn generated() {}\n")?;
        let receipt = |version: &str| {
            crate::oven::receipt_generated_project(
                &crate::oven::OvenGeneratedProjectRequest::new(
                    root.path(),
                    "compiler-owner-fixture",
                    version,
                    "test-target",
                    "rustc 1.99.0-test",
                    "debug",
                    Vec::new(),
                )
                .with_generated_source("generated-root", &source),
            )
        };
        let store = OvenStore::new(
            root.path().join("store"),
            OvenStoreLimits::new(4 * 1024 * 1024, 4 * 1024 * 1024, 4 * 1024 * 1024),
        );

        let no_capacity = OvenStore::new(root.path().join("no-capacity"), OvenStoreLimits::new(0, 0, 0));
        let unavailable = super::retain_direct_rustc_compiler(&no_capacity, &receipt("0.1.0")?, &rustc, "test-target")?;
        let super::OvenDirectRustcCompilerRetention::Unavailable { reason } = unavailable else {
            return Err("zero-capacity Store unexpectedly retained a compiler owner".into());
        };
        assert!(
            reason.contains("cannot publish the selected compiler closure"),
            "{reason}"
        );

        let super::OvenDirectRustcCompilerRetention::Retained(first) =
            super::retain_direct_rustc_compiler(&store, &receipt("0.1.0")?, &rustc, "test-target")?
        else {
            return Err("compiler owner was not retained".into());
        };
        assert_ne!(first.rustc(), rustc);
        assert!(first.rustc().starts_with(store.root().canonicalize()?));
        assert_eq!(
            super::rustc_sysroot(first.rustc())?.canonicalize()?,
            first.evidence.sysroot
        );
        let first_identity = first._owner.manifest.identity.clone();

        // A warm Store compiler is the batch authority. A mutable ambient sysroot that still reports the same
        // host/toolchain must neither be rescanned nor silently replace the already admitted closure for a new
        // project-version receipt.
        fs::write(&ambient_driver, b"different ambient driver bytes")?;
        let changed_ambient = super::direct_rustc_compiler_evidence(&rustc, "test-target")?;
        assert_ne!(changed_ambient.closure_digest(), first.evidence.closure_digest());

        let super::OvenDirectRustcCompilerRetention::Retained(second) =
            super::retain_direct_rustc_compiler(&store, &receipt("0.2.0")?, &rustc, "test-target")?
        else {
            return Err("compatible compiler owner was not reused".into());
        };
        assert_eq!(second._owner.manifest.identity, first_identity);
        let inspection = store.inspect()?;
        assert_eq!(inspection.entries.len(), 1);
        assert!(inspection.active_lease_physical_bytes > 0);

        let bounded = OvenStore::new(store.root(), OvenStoreLimits::new(0, 0, 0));
        let blocked = bounded.prune()?;
        assert!(blocked.skipped_active_entries.contains(&first_identity));
        assert_eq!(bounded.inspect()?.entries.len(), 1);

        drop(first);
        drop(second);
        let reclaimed = bounded.prune()?;
        assert!(reclaimed.removed_entries.contains(&first_identity));
        assert!(bounded.inspect()?.entries.is_empty());
        Ok(())
    }

    #[test]
    fn jec_bound_rlib_is_byte_identical_after_source_relocation() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let rustc = rustc_path()?;
        let target = rustc_host_target(&rustc)?;
        let rustc_digest = digest_bytes(&fs::read(&rustc)?);
        let sysroot = super::rustc_sysroot(&rustc)?.canonicalize()?;
        let source = "pub fn answer() -> u32 { 42 }\n";
        let source_digest = digest_bytes(source.as_bytes());
        let compiler = super::OvenDirectRustcCompilerEvidence {
            binary_digest: rustc_digest,
            closure_digest: digest_bytes(b"test compiler closure"),
            host: target.clone(),
            target: target.clone(),
            sysroot,
            members: Vec::new(),
        };
        let bindings = super::OvenDirectRustcJecBindings {
            logical_source_root: "incan-source".to_string(),
            logical_entrypoint: "src/lib.rs".to_string(),
            externs: Vec::new(),
            dependency_searches: Vec::new(),
            native_searches: Vec::new(),
        };
        let mut outputs = Vec::new();
        let mut observations = Vec::new();
        for name in ["first", "second"] {
            let source_root = root.path().join(name);
            let artifact_root = root.path().join(format!("{name}-artifacts"));
            let output = root.path().join(format!("{name}-output/libfixture.rlib"));
            fs::create_dir_all(source_root.join("src"))?;
            fs::create_dir(&artifact_root)?;
            fs::write(source_root.join("src/lib.rs"), source)?;
            let prepared = super::OvenPreparedDirectRustcLibrary {
                rustc: rustc.clone(),
                compiler: Some(compiler.clone()),
                compiler_owner: None,
                source: source_root.join("src/lib.rs").canonicalize()?,
                source_root: source_root.canonicalize()?,
                source_digest: source_digest.clone(),
                artifact_root: artifact_root.canonicalize()?,
                selected_artifacts: OvenRustcArtifactManifest {
                    schema_version: OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION,
                    intent: crate::oven::OvenBuildIntent {
                        target: target.clone(),
                        toolchain: rustc_identity(&rustc)?,
                        profile: "debug".to_string(),
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
                plan: OvenRustcArtifactPlan {
                    dependency_search_paths: Vec::new(),
                    native_search_paths: Vec::new(),
                    externs: Vec::new(),
                    compile_environment: BTreeMap::new(),
                    caller_owned_library_digests: BTreeMap::new(),
                },
                target: target.clone(),
                profile: "debug".to_string(),
                crate_name: "fixture".to_string(),
                edition: "2021".to_string(),
                features: Vec::new(),
                frozen_environment: Some(
                    super::freeze_direct_rustc_environment(std::env::vars_os(), &BTreeMap::new())
                        .ok_or("test environment was not representable")?,
                ),
            };
            let compiled = prepared.bind(&output, &bindings)?.compile()?;
            outputs.push(fs::read(&compiled.bake.output)?);
            let super::OvenRustcDepInfoOutcome::Observed(observation) = compiled.observation else {
                return Err("relocated Rustc compilation did not provide dep-info".into());
            };
            observations.push(observation.files);
        }
        assert_eq!(outputs[0], outputs[1]);
        assert_ne!(observations[0], observations[1]);
        assert!(
            observations
                .iter()
                .all(|paths| paths.len() == 1 && paths[0].ends_with(Path::new("src/lib.rs")))
        );
        Ok(())
    }

    #[test]
    fn jec_bound_rlib_is_byte_identical_after_dependency_relocation() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let rustc = rustc_path()?;
        let target = rustc_host_target(&rustc)?;
        let dependency_source = root.path().join("dependency.rs");
        let dependency_output = root.path().join("libfixture_dep.rlib");
        fs::write(&dependency_source, "pub fn answer() -> u32 { 42 }\n")?;
        let status = Command::new(&rustc)
            .env_clear()
            .args([
                "--crate-name",
                "fixture_dep",
                "--crate-type",
                "rlib",
                "--edition=2021",
                "--target",
                &target,
            ])
            .arg(&dependency_source)
            .arg("-o")
            .arg(&dependency_output)
            .status()?;
        if !status.success() {
            return Err("fixture dependency compilation failed".into());
        }
        let source = "pub fn answer() -> u32 { fixture_dep::answer() }\n";
        let source_digest = digest_bytes(source.as_bytes());
        let compiler = super::OvenDirectRustcCompilerEvidence {
            binary_digest: digest_bytes(&fs::read(&rustc)?),
            closure_digest: digest_bytes(b"test compiler closure"),
            host: target.clone(),
            target: target.clone(),
            sysroot: super::rustc_sysroot(&rustc)?.canonicalize()?,
            members: Vec::new(),
        };
        let bindings = super::OvenDirectRustcJecBindings {
            logical_source_root: "incan-source".to_string(),
            logical_entrypoint: "src/lib.rs".to_string(),
            externs: vec!["extern/0000".to_string()],
            dependency_searches: vec!["dependency/0000".to_string()],
            native_searches: Vec::new(),
        };
        let mut outputs = Vec::new();
        for name in ["first", "second"] {
            let source_root = root.path().join(name).join("source");
            let artifact_root = root.path().join(name).join("artifacts");
            let dependency_root = artifact_root.join("deps");
            let relocated_dependency = dependency_root.join("libfixture_dep.rlib");
            let output = root.path().join(name).join("output/libfixture.rlib");
            fs::create_dir_all(source_root.join("src"))?;
            fs::create_dir_all(&dependency_root)?;
            fs::write(source_root.join("src/lib.rs"), source)?;
            fs::copy(&dependency_output, &relocated_dependency)?;
            let dependency_root = dependency_root.canonicalize()?;
            let relocated_dependency = relocated_dependency.canonicalize()?;
            let prepared = super::OvenPreparedDirectRustcLibrary {
                rustc: rustc.clone(),
                compiler: Some(compiler.clone()),
                compiler_owner: None,
                source: source_root.join("src/lib.rs").canonicalize()?,
                source_root: source_root.canonicalize()?,
                source_digest: source_digest.clone(),
                artifact_root: artifact_root.canonicalize()?,
                selected_artifacts: OvenRustcArtifactManifest {
                    schema_version: OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION,
                    intent: crate::oven::OvenBuildIntent {
                        target: target.clone(),
                        toolchain: rustc_identity(&rustc)?,
                        profile: "debug".to_string(),
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
                plan: OvenRustcArtifactPlan {
                    dependency_search_paths: vec![dependency_root],
                    native_search_paths: Vec::new(),
                    externs: vec![("fixture_dep".to_string(), relocated_dependency)],
                    compile_environment: BTreeMap::new(),
                    caller_owned_library_digests: BTreeMap::new(),
                },
                target: target.clone(),
                profile: "debug".to_string(),
                crate_name: "fixture".to_string(),
                edition: "2021".to_string(),
                features: Vec::new(),
                frozen_environment: Some(
                    super::freeze_direct_rustc_environment(std::env::vars_os(), &BTreeMap::new())
                        .ok_or("test environment was not representable")?,
                ),
            };
            let compiled = prepared.bind(&output, &bindings)?.compile()?;
            outputs.push(fs::read(&compiled.bake.output)?);
        }
        assert_eq!(outputs[0], outputs[1]);
        Ok(())
    }

    /// A real store-selected payload with explicit role, registry and transitive physical records.
    struct NativeInputFixture {
        owner: crate::oven::store::OvenStoreExecutionPayload,
        receipt: crate::oven::OvenReceipt,
        store: OvenStore,
        root: tempfile::TempDir,
    }

    /// Publish fixture bytes through the normal store without invoking a compiler or interpreting dependency requests.
    fn native_input_fixture(profile: &str) -> Result<NativeInputFixture, Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        write_project(root.path())?;
        let receipt = receipt_generated_project(
            &OvenGeneratedProjectRequest::new(
                root.path(),
                "native_input_fixture",
                "0.1.0",
                "aarch64-apple-darwin",
                "rustc fixture",
                profile,
                Vec::new(),
            )
            .with_generated_source("generated-root", root.path().join("fixture.rs")),
        )?;
        let source = root.path().join("publisher");
        let files = [
            ("release/deps/libruntime.rlib", "runtime bytes"),
            ("release/deps/libserde_fixture.rlib", "registry bytes"),
            ("release/deps/libtransitive.rlib", "transitive bytes"),
            ("helper/libprivate.rlib", "private helper bytes"),
            ("native/libsupport.a", "native support bytes"),
            (
                "registry-sources/fixture/Cargo.toml",
                "[package]\nname='serde_fixture'\nversion='1.2.3'\n",
            ),
            (super::OVEN_RUSTC_REGISTRY_LOCK_RELATIVE_PATH, "version = 4\n"),
        ];
        for (relative, bytes) in files {
            let path = source.join(relative);
            fs::create_dir_all(path.parent().ok_or("fixture path missing parent")?)?;
            fs::write(path, bytes)?;
        }
        let mut artifacts = empty_manifest(&receipt);
        artifacts.dependency_search_paths = vec!["release/deps".to_string(), "helper".to_string()];
        artifacts.native_search_paths = vec!["native".to_string()];
        artifacts
            .compile_environment
            .insert("CARGO_PKG_NAME".to_string(), "native_input_fixture".to_string());
        artifacts.externs = vec![
            OvenRustcArtifactExtern {
                crate_name: "runtime".to_string(),
                relative_path: files[0].0.to_string(),
                digest: digest_bytes(files[0].1.as_bytes()),
            },
            OvenRustcArtifactExtern {
                crate_name: "private".to_string(),
                relative_path: files[3].0.to_string(),
                digest: digest_bytes(files[3].1.as_bytes()),
            },
        ];
        artifacts.entrypoint_externs = BTreeMap::from([
            ("generated-root".to_string(), vec!["runtime".to_string()]),
            ("compiler-helper".to_string(), vec!["private".to_string()]),
        ]);
        artifacts.supporting_artifacts = files
            .iter()
            .enumerate()
            .filter(|(index, _)| *index != 0 && *index != 3)
            .map(|(_, (relative, bytes))| OvenRustcSupportingArtifact {
                relative_path: (*relative).to_string(),
                digest: digest_bytes(bytes.as_bytes()),
            })
            .collect();
        let registry_source = fixture_registry_source();
        artifacts.registry_sources = vec![OvenRustcRegistrySourcePackage {
            package: "serde_fixture".to_string(),
            version: "1.2.3".to_string(),
            features: vec!["derive".to_string()],
            source: registry_source.clone(),
        }];
        artifacts.registry_leaves = vec![OvenRustcRegistryLeaf {
            package: "serde_fixture".to_string(),
            version: "1.2.3".to_string(),
            crate_name: "serde_fixture".to_string(),
            features: vec!["derive".to_string()],
            source: registry_source,
            artifact: OvenRustcArtifactExtern {
                crate_name: "serde_fixture".to_string(),
                relative_path: files[1].0.to_string(),
                digest: digest_bytes(files[1].1.as_bytes()),
            },
        }];
        let materialized = artifacts
            .materialized_artifacts(&source, &receipt.intent)?
            .into_iter()
            .map(|artifact| crate::oven::store::OvenArtifactMaterializedFile {
                source_path: artifact.source_path,
                relative_path: artifact.relative_path,
            })
            .collect();
        let store = OvenStore::new(
            root.path().join("store"),
            OvenStoreLimits::new(4 * 1024 * 1024, 4 * 1024 * 1024, 4 * 1024 * 1024),
        );
        let published = store.publish(&OvenArtifactPublishRequest {
            receipt: receipt.clone(),
            domain: "native-input-view-fixture".to_string(),
            kind: OvenArtifactKind::DirectRustcPlan,
            payload: serde_json::to_vec(&artifacts)?,
            materialized_files: materialized,
        })?;
        let owner = store
            .select_payloads_for_execution(&[published.identity])?
            .pop()
            .ok_or("selected fixture absent")?;
        Ok(NativeInputFixture {
            owner,
            receipt,
            store,
            root,
        })
    }

    #[test]
    fn native_input_view_exposes_original_facts_without_path_profile_selection()
    -> Result<(), Box<dyn std::error::Error>> {
        let debug = native_input_fixture("debug")?;
        let release = native_input_fixture("release")?;
        for fixture in [&debug, &release] {
            let view = super::OvenNativeInputView::from_store_payload(&fixture.owner)?;
            assert_eq!(view.identity(), fixture.owner.manifest.identity);
            assert_eq!(view.receipt_identity(), Some(fixture.receipt.identity.as_str()));
            assert_eq!(view.build_unit_identity(), fixture.receipt.build_unit_identity);
            assert_eq!(view.intent(), &fixture.receipt.intent);
            let candidates = view.registry_candidates("generated-root")?;
            let candidate = candidates.first().ok_or("registry candidate absent")?;
            assert!(candidate.artifact().relative_path.starts_with("release/"));
            assert_eq!(
                candidate.registry_leaf().ok_or("registry facts absent")?.version,
                "1.2.3"
            );
            assert!(view.named_candidates("new-consumer-source-key").is_err());
            let named = view.named_candidates("generated-root")?;
            assert_eq!(
                named
                    .iter()
                    .map(|item| item.artifact().crate_name.as_str())
                    .collect::<Vec<_>>(),
                vec!["runtime"]
            );
            assert!(named.iter().all(|item| item.registry_leaf().is_none()));
        }
        Ok(())
    }

    #[test]
    fn native_input_view_attaches_selected_alias_and_preserves_role_closure() -> Result<(), Box<dyn std::error::Error>>
    {
        let fixture = native_input_fixture("debug")?;
        let view = super::OvenNativeInputView::from_store_payload(&fixture.owner)?;
        let before = super::trusted_artifact_plan_for_source_evidence(&view.plan, &view.artifacts, "generated-root")?;
        let chosen = view
            .registry_candidates("generated-root")?
            .pop()
            .ok_or("registry candidate absent")?
            .with_alias("json_alias".to_string())?;
        let existing = view
            .named_candidates("generated-root")?
            .pop()
            .ok_or("named runtime absent")?
            .with_alias("runtime".to_string())?;
        let attached = view.attach_for_source("generated-root", &[chosen, existing])?;
        let plan = attached.artifact_plan();
        assert_eq!(plan.dependency_search_paths, before.dependency_search_paths);
        assert_eq!(plan.native_search_paths, before.native_search_paths);
        assert_eq!(plan.compile_environment, before.compile_environment);
        assert_eq!(plan.externs.len(), 2);
        assert_eq!(plan.externs[1].0, "json_alias");
        assert_eq!(
            plan.caller_owned_library_digests.get("json_alias"),
            Some(&digest_bytes(b"registry bytes"))
        );
        assert!(
            !plan
                .externs
                .iter()
                .any(|(name, _)| name == "private" || name == "transitive")
        );
        assert_eq!(view.plan.externs.len(), 2, "attachment must not mutate the foundation");
        Ok(())
    }

    #[test]
    fn native_input_view_refuses_foreign_foundation_role_and_alias_collision() -> Result<(), Box<dyn std::error::Error>>
    {
        let first = native_input_fixture("debug")?;
        let second = native_input_fixture("debug")?;
        let view = super::OvenNativeInputView::from_store_payload(&first.owner)?;
        let other = super::OvenNativeInputView::from_store_payload(&second.owner)?;
        let foreign = other
            .registry_candidates("generated-root")?
            .pop()
            .ok_or("foreign leaf absent")?
            .with_alias("dependency".to_string())?;
        assert!(view.attach_for_source("generated-root", &[foreign]).is_err());
        let wrong_role = view
            .named_candidates("compiler-helper")?
            .pop()
            .ok_or("helper absent")?
            .with_alias("dependency".to_string())?;
        assert!(view.attach_for_source("generated-root", &[wrong_role]).is_err());
        let colliding = view
            .registry_candidates("generated-root")?
            .pop()
            .ok_or("leaf absent")?
            .with_alias("runtime".to_string())?;
        assert!(view.attach_for_source("generated-root", &[colliding]).is_err());
        let invalid_alias = view.registry_candidates("generated-root")?.pop().ok_or("leaf absent")?;
        assert!(invalid_alias.with_alias("not an identifier".to_string()).is_err());
        let excluded = view
            .registry_candidates("compiler-helper")?
            .pop()
            .ok_or("leaf facts absent")?
            .with_alias("dependency".to_string())?;
        assert!(
            view.attach_for_source("compiler-helper", &[excluded]).is_err(),
            "an excluded physical directory must not be restored"
        );
        assert!(!first.root.path().join("output").exists());
        Ok(())
    }

    /// Round-trip a host-issued named-member ID while retaining its actual publisher, role and bytes.
    #[test]
    fn compiler_runtime_member_roundtrip_preserves_original_role_and_rechecks_bytes()
    -> Result<(), Box<dyn std::error::Error>> {
        let fixture = native_input_fixture("debug")?;
        let view = super::OvenNativeInputView::from_store_payload(&fixture.owner)?;
        let members = view.named_candidates("generated-root")?;
        let member = members.first().ok_or("original runtime member absent")?;
        let request = serde_json::json!({
            "grant_id": "original-member-0",
            "plan_identity": view.identity(),
            "source_role": member.source_role,
            "extern_name": member.artifact().crate_name,
            "artifact_digest": member.artifact().digest,
        });
        let decoded: serde_json::Value = serde_json::from_slice(&serde_json::to_vec(&request)?)?;
        assert_eq!(decoded["source_role"], "generated-root");
        assert_eq!(decoded["extern_name"], "runtime");
        assert_eq!(decoded["artifact_digest"], digest_bytes(b"runtime bytes"));
        // The selector returns an ID, never replacement member bytes or an executable path.
        let selected_ids: Vec<String> = serde_json::from_slice(br#"["original-member-0"]"#)?;
        let mut table = BTreeMap::from([(
            "original-member-0".to_string(),
            members.into_iter().next().ok_or("member absent")?,
        )]);
        let selected = selected_ids
            .into_iter()
            .map(|id| {
                table
                    .remove(&id)
                    .ok_or("unknown or repeated member ID")?
                    .with_alias("runtime".to_string())
                    .map_err(Into::into)
            })
            .collect::<Result<Vec<_>, Box<dyn std::error::Error>>>()?;
        assert!(table.remove("child-invented-member").is_none());
        let attached = view.attach_for_source("generated-root", &selected)?;
        assert!(
            attached
                .artifact_plan()
                .externs
                .iter()
                .any(|(name, _)| name == "runtime")
        );
        assert!(
            !attached
                .artifact_plan()
                .externs
                .iter()
                .any(|(name, _)| name == "private")
        );
        assert!(view.attach_for_source("compiler-helper", &selected).is_err());
        fs::write(
            fixture.owner.artifact_root.join("release/deps/libruntime.rlib"),
            b"changed named runtime",
        )?;
        assert!(matches!(
            view.attach_for_source("generated-root", &selected),
            Err(OvenRustcError::ArtifactDigestMismatch { .. })
        ));
        Ok(())
    }

    #[test]
    fn native_input_view_queries_are_read_free_but_attachment_rechecks_selected_bytes()
    -> Result<(), Box<dyn std::error::Error>> {
        let fixture = native_input_fixture("debug")?;
        let view = super::OvenNativeInputView::from_store_payload(&fixture.owner)?;
        let original_plan = view.plan.clone();
        let file = fixture.owner.artifact_root.join("release/deps/libserde_fixture.rlib");
        fs::write(&file, b"changed after admission")?;
        let chosen = view
            .registry_candidates("generated-root")?
            .pop()
            .ok_or("facts changed after physical mutation")?
            .with_alias("dependency".to_string())?;
        assert!(matches!(
            view.attach_for_source("generated-root", &[chosen]),
            Err(OvenRustcError::ArtifactDigestMismatch { .. })
        ));
        assert_eq!(view.plan, original_plan);
        assert!(!fixture.root.path().join("output").exists());
        Ok(())
    }

    #[test]
    fn native_input_view_rejects_mutated_payload_manifest_and_coordinated_retargeting()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut first = native_input_fixture("debug")?;
        let second = native_input_fixture("debug")?;
        assert_eq!(first.owner.manifest, second.owner.manifest);
        assert_eq!(first.owner.payload, second.owner.payload);
        assert_ne!(first.owner.artifact_root, second.owner.artifact_root);
        let original_payload = first.owner.payload.clone();
        first.owner.payload.push(b' ');
        assert!(super::OvenNativeInputView::from_store_payload(&first.owner).is_err());
        first.owner.payload = original_payload;
        let original_manifest = first.owner.manifest.clone();
        first.owner.manifest.intent.profile = "release".to_string();
        assert!(super::OvenNativeInputView::from_store_payload(&first.owner).is_err());
        first.owner.manifest = original_manifest;
        assert!(super::OvenNativeInputView::from_store_payload(&first.owner).is_ok());
        first.owner.manifest = second.owner.manifest.clone();
        first.owner.artifact_root = second.owner.artifact_root.clone();
        first.owner.payload = second.owner.payload.clone();
        assert!(
            super::OvenNativeInputView::from_store_payload(&first.owner).is_err(),
            "even identical valid records cannot retarget the original selected lease to another root"
        );
        Ok(())
    }

    #[test]
    fn native_input_view_keeps_real_store_lease_through_attached_plan_use() -> Result<(), Box<dyn std::error::Error>> {
        let fixture = native_input_fixture("debug")?;
        let bounded = OvenStore::new(
            fixture.root.path().join("store"),
            OvenStoreLimits::new(1, 4 * 1024 * 1024, 4 * 1024 * 1024),
        );
        let view = super::OvenNativeInputView::from_store_payload(&fixture.owner)?;
        let attached = view.attach_for_source("generated-root", &[])?;
        bounded.prune()?;
        let held = fixture.store.inspect()?;
        assert_eq!(held.entries.len(), 1);
        assert_eq!(held.active_lease_physical_bytes, held.physical_bytes);
        assert!(attached.artifact_plan().externs[0].1.is_file());
        drop(attached);
        drop(view);
        drop(fixture.owner);
        bounded.prune()?;
        assert!(bounded.inspect()?.entries.is_empty());
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn native_input_view_refuses_selected_symlink_after_admission() -> Result<(), Box<dyn std::error::Error>> {
        let fixture = native_input_fixture("debug")?;
        let view = super::OvenNativeInputView::from_store_payload(&fixture.owner)?;
        let selected = fixture.owner.artifact_root.join("release/deps/libserde_fixture.rlib");
        let foreign = fixture.root.path().join("foreign.rlib");
        fs::write(&foreign, b"registry bytes")?;
        fs::remove_file(&selected)?;
        std::os::unix::fs::symlink(&foreign, &selected)?;
        let candidate = view
            .registry_candidates("generated-root")?
            .pop()
            .ok_or("leaf absent")?
            .with_alias("dependency".to_string())?;
        assert!(view.attach_for_source("generated-root", &[candidate]).is_err());
        Ok(())
    }

    fn fixture_registry_source() -> OvenRustcRegistrySource {
        OvenRustcRegistrySource {
            registry: "registry+https://example.invalid/index".to_string(),
            checksum: "fixture-checksum".to_string(),
            relative_root: "registry-sources/fixture".to_string(),
            digest: digest_bytes(b"fixture registry source"),
        }
    }

    #[test]
    fn failed_direct_rustc_report_keeps_the_bounded_invocation() {
        let mut command = Command::new("rustc");
        command.args(["--crate-name", "closure_probe"]);
        let report = super::parse_rustc_diagnostics(b"", b"").with_invocation(&command);

        assert_eq!(
            report.invocation.as_deref(),
            Some("\"rustc\" \"--crate-name\" \"closure_probe\"")
        );
        assert_eq!(
            report.to_string(),
            "direct rustc invocation: \"rustc\" \"--crate-name\" \"closure_probe\""
        );
    }

    #[test]
    fn native_runtime_rpaths_apply_only_to_the_host_native_target() {
        let host_architecture = std::env::consts::ARCH;

        #[cfg(target_os = "macos")]
        {
            assert!(is_host_native_unix_target(&format!("{host_architecture}-apple-darwin")));
            assert!(!is_host_native_unix_target(&format!("{host_architecture}-apple-ios")));
            assert!(!is_host_native_unix_target("aarch64-linux-android"));
        }

        #[cfg(target_os = "linux")]
        {
            let host_target = if cfg!(target_env = "gnu") {
                format!("{host_architecture}-unknown-linux-gnu")
            } else if cfg!(target_env = "musl") {
                format!("{host_architecture}-unknown-linux-musl")
            } else {
                String::new()
            };
            if !host_target.is_empty() {
                assert!(is_host_native_unix_target(&host_target));
            }
            assert!(!is_host_native_unix_target(&format!(
                "{host_architecture}-apple-darwin"
            )));
            assert!(
                !is_host_native_unix_target(&format!("{host_architecture}-unknown-linux-musl"))
                    || cfg!(target_env = "musl")
            );
            assert!(!is_host_native_unix_target("aarch64-linux-android"));
        }

        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        {
            assert!(!is_host_native_unix_target(&format!(
                "{host_architecture}-unknown-linux-gnu"
            )));
        }
    }

    #[test]
    fn selects_the_highest_sealed_registry_leaf_matching_the_declared_requirement()
    -> Result<(), Box<dyn std::error::Error>> {
        let registry = tempfile::tempdir()?;
        let mut leaves = Vec::new();
        for version in ["1.0.8", "1.0.18", "2.0.0"] {
            let artifact = registry.path().join(format!("libitoa-{version}.rlib"));
            let bytes = format!("sealed itoa {version}").into_bytes();
            fs::write(&artifact, &bytes)?;
            leaves.push(OvenRustcRegistryLeaf {
                package: "itoa".to_string(),
                version: version.to_string(),
                crate_name: "itoa".to_string(),
                features: if version == "1.0.18" {
                    vec!["std".to_string()]
                } else {
                    Vec::new()
                },
                source: fixture_registry_source(),
                artifact: OvenRustcArtifactExtern {
                    crate_name: "itoa".to_string(),
                    relative_path: artifact
                        .file_name()
                        .and_then(|name| name.to_str())
                        .ok_or("registry artifact name")?
                        .to_string(),
                    digest: digest_bytes(&bytes),
                },
            });
        }
        let authority = OvenRegistryLeafAuthority::new(registry.path().to_path_buf(), leaves);
        let dependency = DependencySpec {
            crate_name: "itoa".to_string(),
            version: Some("1".to_string()),
            features: vec!["std".to_string()],
            default_features: true,
            source: DependencySource::Registry,
            optional: false,
            package: None,
        };
        let selected = resolve_sealed_registry_leaf(&dependency, Some(&authority), "debug")?;
        assert_eq!(
            selected.file_name().and_then(|name| name.to_str()),
            Some("libitoa-1.0.18.rlib")
        );
        let unavailable_dependency = DependencySpec {
            crate_name: "itoa".to_string(),
            version: Some("1".to_string()),
            features: vec!["alloc".to_string()],
            default_features: true,
            source: DependencySource::Registry,
            optional: false,
            package: None,
        };
        let unavailable_feature = resolve_sealed_registry_leaf(&unavailable_dependency, Some(&authority), "debug");
        assert!(matches!(unavailable_feature, Err(OvenRustcError::InvalidInput { .. })));
        Ok(())
    }

    #[test]
    fn validates_the_exact_selected_registry_extern_instead_of_reselecting_highest_semver()
    -> Result<(), Box<dyn std::error::Error>> {
        let registry = tempfile::tempdir()?;
        let dependency_directory = registry.path().join("target/debug/deps");
        fs::create_dir_all(&dependency_directory)?;
        let mut leaves = Vec::new();
        let mut artifacts = BTreeMap::new();
        for version in ["1.2.0", "1.8.0"] {
            let relative_path = format!("target/debug/deps/libshared-{version}.rlib");
            let artifact = registry.path().join(&relative_path);
            let bytes = format!("sealed shared {version}").into_bytes();
            fs::write(&artifact, &bytes)?;
            artifacts.insert(version, fs::canonicalize(&artifact)?);
            leaves.push(OvenRustcRegistryLeaf {
                package: "shared".to_string(),
                version: version.to_string(),
                crate_name: "shared".to_string(),
                features: vec!["default".to_string()],
                source: fixture_registry_source(),
                artifact: OvenRustcArtifactExtern {
                    crate_name: "shared".to_string(),
                    relative_path,
                    digest: digest_bytes(&bytes),
                },
            });
        }
        let authority = OvenRegistryLeafAuthority::new(registry.path().to_path_buf(), leaves);
        let dependency = DependencySpec {
            crate_name: "shared_old".to_string(),
            version: Some("1".to_string()),
            features: vec!["default".to_string()],
            default_features: true,
            source: DependencySource::Registry,
            optional: false,
            package: Some("shared".to_string()),
        };

        assert_eq!(
            super::select_sealed_registry_leaf(&dependency, Some(&authority), "debug")?
                .leaf
                .version,
            "1.8.0",
            "the compatibility catalog retains its existing highest-semver behavior"
        );
        super::validate_selected_sealed_registry_leaf(
            &dependency,
            artifacts.get("1.2.0").ok_or("old artifact missing")?,
            Some(&authority),
            "debug",
        )?;
        let wrong_profile = super::validate_selected_sealed_registry_leaf(
            &dependency,
            artifacts.get("1.2.0").ok_or("old artifact missing")?,
            Some(&authority),
            "release",
        );
        assert!(matches!(wrong_profile, Err(OvenRustcError::InvalidInput { .. })));
        Ok(())
    }

    #[test]
    fn aggregates_compatible_registry_catalogs_without_losing_the_leaf_root() -> Result<(), Box<dyn std::error::Error>>
    {
        let narrow = tempfile::tempdir()?;
        let broad = tempfile::tempdir()?;
        let narrow_artifact = narrow.path().join("libbitflags-v2.rlib");
        let narrow_bytes = b"sealed bitflags 2.13.1";
        fs::write(&narrow_artifact, narrow_bytes)?;
        let broad_artifact = broad.path().join("libbitflags-v1.rlib");
        let broad_bytes = b"sealed bitflags 1.3.2";
        fs::write(&broad_artifact, broad_bytes)?;
        let authority = OvenRegistryLeafAuthority::aggregate([
            OvenRegistryLeafAuthority::new(
                narrow.path().to_path_buf(),
                vec![OvenRustcRegistryLeaf {
                    package: "bitflags".to_string(),
                    version: "2.13.1".to_string(),
                    crate_name: "bitflags".to_string(),
                    features: Vec::new(),
                    source: fixture_registry_source(),
                    artifact: OvenRustcArtifactExtern {
                        crate_name: "bitflags".to_string(),
                        relative_path: "libbitflags-v2.rlib".to_string(),
                        digest: digest_bytes(narrow_bytes),
                    },
                }],
            ),
            OvenRegistryLeafAuthority::new(
                broad.path().to_path_buf(),
                vec![OvenRustcRegistryLeaf {
                    package: "bitflags".to_string(),
                    version: "1.3.2".to_string(),
                    crate_name: "bitflags".to_string(),
                    features: Vec::new(),
                    source: fixture_registry_source(),
                    artifact: OvenRustcArtifactExtern {
                        crate_name: "bitflags".to_string(),
                        relative_path: "libbitflags-v1.rlib".to_string(),
                        digest: digest_bytes(broad_bytes),
                    },
                }],
            ),
        ]);
        let dependency = DependencySpec {
            crate_name: "bitflags".to_string(),
            version: Some("=1.3.2".to_string()),
            features: Vec::new(),
            default_features: true,
            source: DependencySource::Registry,
            optional: false,
            package: None,
        };

        assert_eq!(
            resolve_sealed_registry_leaf(&dependency, Some(&authority), "debug")?,
            fs::canonicalize(broad_artifact)?
        );
        Ok(())
    }

    #[test]
    fn aggregate_admits_a_provider_package_the_consumer_never_declared() -> Result<(), Box<dyn std::error::Error>> {
        let consumer_root = tempfile::tempdir()?;
        let provider_root = tempfile::tempdir()?;
        let provider_artifact = provider_root.path().join("libdatafusion.rlib");
        let provider_bytes = b"sealed datafusion 53.1.0";
        fs::write(&provider_artifact, provider_bytes)?;
        let consumer_authority = OvenRegistryLeafAuthority::new(consumer_root.path().to_path_buf(), Vec::new());
        let provider_authority = OvenRegistryLeafAuthority::new(
            provider_root.path().to_path_buf(),
            vec![OvenRustcRegistryLeaf {
                package: "datafusion".to_string(),
                version: "53.1.0".to_string(),
                crate_name: "datafusion".to_string(),
                features: Vec::new(),
                source: fixture_registry_source(),
                artifact: OvenRustcArtifactExtern {
                    crate_name: "datafusion".to_string(),
                    relative_path: "libdatafusion.rlib".to_string(),
                    digest: digest_bytes(provider_bytes),
                },
            }],
        );
        let joined = OvenRegistryLeafAuthority::aggregate([consumer_authority, provider_authority]);
        let dependency = DependencySpec {
            crate_name: "datafusion".to_string(),
            version: Some("53".to_string()),
            features: Vec::new(),
            default_features: true,
            source: DependencySource::Registry,
            optional: false,
            package: None,
        };
        assert_eq!(
            resolve_sealed_registry_leaf(&dependency, Some(&joined), "debug")?,
            fs::canonicalize(provider_artifact)?
        );
        Ok(())
    }

    #[test]
    fn aggregate_does_not_block_an_unrelated_lookup_when_a_never_requested_package_conflicts()
    -> Result<(), Box<dyn std::error::Error>> {
        // Regression coverage for a real false positive: a provider and consumer can each carry their own build of
        // some common transitive crate (for example `memchr`, pulled in independently by unrelated dependencies on
        // each side) that nobody ever actually resolves through this authority. Joining the two catalogs must not
        // block resolution of a package that IS actually requested and IS only on one side.
        let consumer_root = tempfile::tempdir()?;
        let provider_root = tempfile::tempdir()?;
        let consumer_bytes = b"sealed memchr 2.8.0 consumer build";
        let provider_memchr_bytes = b"sealed memchr 2.8.0 provider build";
        let provider_datafusion_bytes = b"sealed datafusion 53.1.0";
        fs::write(consumer_root.path().join("libmemchr.rlib"), consumer_bytes)?;
        fs::write(provider_root.path().join("libmemchr.rlib"), provider_memchr_bytes)?;
        let provider_datafusion = provider_root.path().join("libdatafusion.rlib");
        fs::write(&provider_datafusion, provider_datafusion_bytes)?;
        let memchr_leaf = |bytes: &[u8]| OvenRustcRegistryLeaf {
            package: "memchr".to_string(),
            version: "2.8.0".to_string(),
            crate_name: "memchr".to_string(),
            features: Vec::new(),
            source: fixture_registry_source(),
            artifact: OvenRustcArtifactExtern {
                crate_name: "memchr".to_string(),
                relative_path: "libmemchr.rlib".to_string(),
                digest: digest_bytes(bytes),
            },
        };
        let consumer_authority =
            OvenRegistryLeafAuthority::new(consumer_root.path().to_path_buf(), vec![memchr_leaf(consumer_bytes)]);
        let provider_authority = OvenRegistryLeafAuthority::new(
            provider_root.path().to_path_buf(),
            vec![
                memchr_leaf(provider_memchr_bytes),
                OvenRustcRegistryLeaf {
                    package: "datafusion".to_string(),
                    version: "53.1.0".to_string(),
                    crate_name: "datafusion".to_string(),
                    features: Vec::new(),
                    source: fixture_registry_source(),
                    artifact: OvenRustcArtifactExtern {
                        crate_name: "datafusion".to_string(),
                        relative_path: "libdatafusion.rlib".to_string(),
                        digest: digest_bytes(provider_datafusion_bytes),
                    },
                },
            ],
        );
        let joined = OvenRegistryLeafAuthority::aggregate([consumer_authority, provider_authority]);
        let datafusion_dependency = DependencySpec {
            crate_name: "datafusion".to_string(),
            version: Some("53".to_string()),
            features: Vec::new(),
            default_features: true,
            source: DependencySource::Registry,
            optional: false,
            package: None,
        };
        assert_eq!(
            resolve_sealed_registry_leaf(&datafusion_dependency, Some(&joined), "debug")?,
            fs::canonicalize(&provider_datafusion)?,
            "an unrelated conflicting memchr entry must not block resolving datafusion"
        );
        Ok(())
    }

    #[test]
    fn aggregate_fails_closed_when_the_actually_requested_package_conflicts() -> Result<(), Box<dyn std::error::Error>>
    {
        let consumer_root = tempfile::tempdir()?;
        let provider_root = tempfile::tempdir()?;
        let consumer_bytes = b"sealed tokio 1.52.3 rt-multi-thread,macros,time,sync,net";
        let provider_bytes = b"sealed tokio 1.52.3 full";
        // Real rustc output embeds a metadata hash in the filename that differs whenever the compiled configuration
        // differs, which is what `same_compilation` actually keys off; give the two conflicting artifacts distinct
        // names here so this fixture matches that shape instead of coincidentally looking like the same compilation.
        fs::write(consumer_root.path().join("libtokio-consumer1234.rlib"), consumer_bytes)?;
        fs::write(provider_root.path().join("libtokio-provider5678.rlib"), provider_bytes)?;
        let leaf = |features: &[&str], bytes: &[u8], relative_path: &str| OvenRustcRegistryLeaf {
            package: "tokio".to_string(),
            version: "1.52.3".to_string(),
            crate_name: "tokio".to_string(),
            features: features.iter().map(|feature| feature.to_string()).collect(),
            source: fixture_registry_source(),
            artifact: OvenRustcArtifactExtern {
                crate_name: "tokio".to_string(),
                relative_path: relative_path.to_string(),
                digest: digest_bytes(bytes),
            },
        };
        let consumer_authority = OvenRegistryLeafAuthority::new(
            consumer_root.path().to_path_buf(),
            vec![leaf(
                &["rt-multi-thread", "macros", "time", "sync", "net"],
                consumer_bytes,
                "libtokio-consumer1234.rlib",
            )],
        );
        let provider_authority = OvenRegistryLeafAuthority::new(
            provider_root.path().to_path_buf(),
            vec![leaf(&["full"], provider_bytes, "libtokio-provider5678.rlib")],
        );
        let joined = OvenRegistryLeafAuthority::aggregate([consumer_authority, provider_authority]);
        let dependency = DependencySpec {
            crate_name: "tokio".to_string(),
            version: Some("1".to_string()),
            features: Vec::new(),
            default_features: true,
            source: DependencySource::Registry,
            optional: false,
            package: None,
        };
        let error = resolve_sealed_registry_leaf(&dependency, Some(&joined), "debug")
            .expect_err("resolving a package that genuinely disagrees between two joined authorities must fail closed");
        assert!(matches!(error, OvenRustcError::InvalidInput { .. }));
        Ok(())
    }

    #[test]
    fn first_conflicting_package_with_reports_a_provider_leaf_that_disagrees_with_an_existing_extern()
    -> Result<(), Box<dyn std::error::Error>> {
        // Reproduces the exact real-world defect this check exists for: a caller-owned provider's own registry
        // closure (a query-engine library's own DataFusion/Tokio dependency graph) resolves a package the consumer
        // already links explicitly (the SDK's own `block_on` support) to a different compiled artifact. Left
        // unchecked, both get linked into one binary as two distinct compiled `tokio` instances -- confirmed by
        // inspecting a real built executable's symbol table -- and the async runtime state silently splits across
        // them, producing a "no reactor running" panic at runtime instead of a build failure.
        let consumer_root = tempfile::tempdir()?;
        let provider_root = tempfile::tempdir()?;
        let consumer_bytes = b"sealed tokio compiled for the SDK's own block_on closure";
        let provider_bytes = b"sealed tokio compiled for the provider's own DataFusion closure";
        let consumer_artifact = consumer_root.path().join("libtokio-sdk1234.rlib");
        let provider_artifact = provider_root.path().join("libtokio-provider5678.rlib");
        fs::write(&consumer_artifact, consumer_bytes)?;
        fs::write(&provider_artifact, provider_bytes)?;
        let plan = OvenRustcArtifactPlan {
            dependency_search_paths: Vec::new(),
            native_search_paths: Vec::new(),
            externs: vec![("tokio".to_string(), consumer_artifact)],
            compile_environment: BTreeMap::new(),
            caller_owned_library_digests: BTreeMap::new(),
        };
        let provider_authority = OvenRegistryLeafAuthority::new(
            provider_root.path().to_path_buf(),
            vec![OvenRustcRegistryLeaf {
                package: "tokio".to_string(),
                version: "1.52.3".to_string(),
                crate_name: "tokio".to_string(),
                features: Vec::new(),
                source: fixture_registry_source(),
                artifact: OvenRustcArtifactExtern {
                    crate_name: "tokio".to_string(),
                    relative_path: "libtokio-provider5678.rlib".to_string(),
                    digest: digest_bytes(provider_bytes),
                },
            }],
        );
        assert_eq!(
            provider_authority.first_conflicting_package_with(&plan)?,
            Some("tokio".to_string())
        );
        Ok(())
    }

    #[test]
    fn first_conflicting_package_with_allows_a_byte_identical_provider_leaf() -> Result<(), Box<dyn std::error::Error>>
    {
        let consumer_root = tempfile::tempdir()?;
        let provider_root = tempfile::tempdir()?;
        let shared_bytes = b"sealed tokio, compiled identically for both closures";
        let consumer_artifact = consumer_root.path().join("libtokio-shared.rlib");
        let provider_artifact = provider_root.path().join("libtokio-shared.rlib");
        fs::write(&consumer_artifact, shared_bytes)?;
        fs::write(&provider_artifact, shared_bytes)?;
        let plan = OvenRustcArtifactPlan {
            dependency_search_paths: Vec::new(),
            native_search_paths: Vec::new(),
            externs: vec![("tokio".to_string(), consumer_artifact)],
            compile_environment: BTreeMap::new(),
            caller_owned_library_digests: BTreeMap::new(),
        };
        let provider_authority = OvenRegistryLeafAuthority::new(
            provider_root.path().to_path_buf(),
            vec![OvenRustcRegistryLeaf {
                package: "tokio".to_string(),
                version: "1.52.3".to_string(),
                crate_name: "tokio".to_string(),
                features: Vec::new(),
                source: fixture_registry_source(),
                artifact: OvenRustcArtifactExtern {
                    crate_name: "tokio".to_string(),
                    relative_path: "libtokio-shared.rlib".to_string(),
                    digest: digest_bytes(shared_bytes),
                },
            }],
        );
        assert_eq!(
            provider_authority.first_conflicting_package_with(&plan)?,
            None,
            "a provider leaf compiled to the exact same bytes as the existing extern must not be rejected"
        );
        Ok(())
    }

    #[test]
    fn first_conflicting_package_with_ignores_an_unrelated_package() -> Result<(), Box<dyn std::error::Error>> {
        let consumer_root = tempfile::tempdir()?;
        let provider_root = tempfile::tempdir()?;
        let consumer_bytes = b"sealed tokio for the consumer";
        let provider_bytes = b"sealed datafusion for the provider";
        let consumer_artifact = consumer_root.path().join("libtokio.rlib");
        let provider_artifact = provider_root.path().join("libdatafusion.rlib");
        fs::write(&consumer_artifact, consumer_bytes)?;
        fs::write(&provider_artifact, provider_bytes)?;
        let plan = OvenRustcArtifactPlan {
            dependency_search_paths: Vec::new(),
            native_search_paths: Vec::new(),
            externs: vec![("tokio".to_string(), consumer_artifact)],
            compile_environment: BTreeMap::new(),
            caller_owned_library_digests: BTreeMap::new(),
        };
        let provider_authority = OvenRegistryLeafAuthority::new(
            provider_root.path().to_path_buf(),
            vec![OvenRustcRegistryLeaf {
                package: "datafusion".to_string(),
                version: "53.1.0".to_string(),
                crate_name: "datafusion".to_string(),
                features: Vec::new(),
                source: fixture_registry_source(),
                artifact: OvenRustcArtifactExtern {
                    crate_name: "datafusion".to_string(),
                    relative_path: "libdatafusion.rlib".to_string(),
                    digest: digest_bytes(provider_bytes),
                },
            }],
        );
        assert_eq!(provider_authority.first_conflicting_package_with(&plan)?, None);
        Ok(())
    }

    #[test]
    fn incan_owned_rustup_home_is_absent_for_a_development_checkout() -> Result<(), Box<dyn std::error::Error>> {
        // A checkout has no `<root>/rust/toolchains`, so the compiler must keep resolving through the ambient
        // Rustup default. Regressing this would break every contributor's `make test`.
        let checkout = tempfile::tempdir()?;
        let executable = checkout.path().join("target").join("debug").join("incan");
        fs::create_dir_all(executable.parent().ok_or("executable has no parent")?)?;
        fs::write(&executable, b"")?;
        let home = tempfile::tempdir()?;
        assert_eq!(
            super::incan_owned_rustup_home_in(
                Some(checkout.path().as_os_str().to_os_string()),
                Some(executable),
                Some(home.path().as_os_str().to_os_string()),
            ),
            None
        );
        Ok(())
    }

    /// Build a Rustup-shaped home holding the named channels, each carrying a `rustc` and `cargo`.
    fn provisioned_rust_root(root: &Path, channels: &[&str]) -> Result<PathBuf, Box<dyn std::error::Error>> {
        let rust_root = root.join("rust");
        for channel in channels {
            let bin = rust_root.join("toolchains").join(channel).join("bin");
            fs::create_dir_all(&bin)?;
            fs::write(bin.join("rustc"), b"")?;
            fs::write(bin.join("cargo"), b"")?;
        }
        Ok(rust_root)
    }

    #[test]
    fn incan_owned_tool_resolves_the_only_toolchain_without_a_pointer() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let rust_root = provisioned_rust_root(root.path(), &["1.98.0-aarch64-apple-darwin"])?;
        assert_eq!(
            super::incan_owned_tool(&rust_root, "rustc"),
            Some(
                rust_root
                    .join("toolchains")
                    .join("1.98.0-aarch64-apple-darwin")
                    .join("bin")
                    .join("rustc")
            )
        );
        Ok(())
    }

    #[test]
    fn incan_owned_tool_uses_the_pointer_when_an_upgrade_left_an_older_channel()
    -> Result<(), Box<dyn std::error::Error>> {
        // An upgrade can leave the previous channel installed. Choosing between them by sort order would silently
        // pick the wrong compiler, so the installer's recorded channel decides.
        let root = tempfile::tempdir()?;
        let rust_root = provisioned_rust_root(
            root.path(),
            &["1.97.0-aarch64-apple-darwin", "1.98.0-aarch64-apple-darwin"],
        )?;
        fs::write(rust_root.join(super::INCAN_OWNED_CHANNEL_POINTER), "1.98.0\n")?;
        assert_eq!(
            super::incan_owned_tool(&rust_root, "rustc"),
            Some(
                rust_root
                    .join("toolchains")
                    .join("1.98.0-aarch64-apple-darwin")
                    .join("bin")
                    .join("rustc")
            )
        );
        Ok(())
    }

    #[test]
    fn incan_owned_tool_declines_an_ambiguous_home_without_a_pointer() -> Result<(), Box<dyn std::error::Error>> {
        // Guessing here would bind the build to an arbitrary compiler; falling back to the ambient default is the
        // honest outcome.
        let root = tempfile::tempdir()?;
        let rust_root = provisioned_rust_root(
            root.path(),
            &["1.97.0-aarch64-apple-darwin", "1.98.0-aarch64-apple-darwin"],
        )?;
        assert_eq!(super::incan_owned_tool(&rust_root, "rustc"), None);
        Ok(())
    }

    #[test]
    fn incan_owned_tool_resolves_cargo_from_the_same_toolchain_as_rustc() -> Result<(), Box<dyn std::error::Error>> {
        // The baker's Cargo and the compiler's rustc must come from one toolchain, or the isolation is pointless.
        let root = tempfile::tempdir()?;
        let rust_root = provisioned_rust_root(root.path(), &["1.98.0-host"])?;
        fs::write(rust_root.join(super::INCAN_OWNED_CHANNEL_POINTER), "1.98.0\n")?;
        let rustc = super::incan_owned_tool(&rust_root, "rustc").ok_or("rustc did not resolve")?;
        let cargo = super::incan_owned_tool(&rust_root, "cargo").ok_or("cargo did not resolve")?;
        assert_eq!(rustc.parent(), cargo.parent());
        Ok(())
    }

    #[test]
    fn incan_owned_tool_declines_a_home_whose_pointer_names_a_missing_channel() -> Result<(), Box<dyn std::error::Error>>
    {
        let root = tempfile::tempdir()?;
        let rust_root = provisioned_rust_root(root.path(), &["1.97.0-host"])?;
        fs::write(rust_root.join(super::INCAN_OWNED_CHANNEL_POINTER), "1.98.0\n")?;
        // The single-toolchain fallback still answers, because that home is unambiguous.
        assert_eq!(
            super::incan_owned_tool(&rust_root, "rustc"),
            Some(
                rust_root
                    .join("toolchains")
                    .join("1.97.0-host")
                    .join("bin")
                    .join("rustc")
            )
        );
        Ok(())
    }

    #[test]
    fn incan_owned_target_membership_reads_the_toolchain_layout() -> Result<(), Box<dyn std::error::Error>> {
        // The installer adds targets to Incan's own toolchain and leaves the user's Rustup alone, so membership
        // has to be read where the target actually lands.
        let root = tempfile::tempdir()?;
        let rust_root = provisioned_rust_root(root.path(), &["1.98.0-host"])?;
        let toolchain = rust_root.join("toolchains").join("1.98.0-host");
        fs::create_dir_all(toolchain.join("lib").join("rustlib").join("wasm32-wasip1"))?;
        let rustc = super::incan_owned_tool(&rust_root, "rustc").ok_or("rustc did not resolve")?;
        let toolchain_root = rustc.parent().and_then(Path::parent).ok_or("no toolchain root")?;
        assert!(
            toolchain_root
                .join("lib")
                .join("rustlib")
                .join("wasm32-wasip1")
                .is_dir()
        );
        assert!(
            !toolchain_root
                .join("lib")
                .join("rustlib")
                .join("aarch64-unknown-none")
                .is_dir()
        );
        Ok(())
    }

    #[test]
    fn incan_owned_rustup_home_prefers_an_explicitly_named_incan_home() -> Result<(), Box<dyn std::error::Error>> {
        let named = tempfile::tempdir()?;
        fs::create_dir_all(named.path().join("rust").join("toolchains").join("1.98.0-host"))?;
        assert_eq!(
            super::incan_owned_rustup_home_in(Some(named.path().as_os_str().to_os_string()), None, None),
            Some(named.path().join("rust"))
        );
        Ok(())
    }

    #[test]
    fn incan_owned_rustup_home_follows_the_executable_for_shim_installations() -> Result<(), Box<dyn std::error::Error>>
    {
        // The npm and pip shims install below their own package directory and do not set `INCAN_HOME` when they
        // spawn the compiler, so the executable's own layout is the only thing that identifies its provisioning.
        let package = tempfile::tempdir()?;
        let incan_home = package.path().join(".incan").join("home");
        fs::create_dir_all(incan_home.join("rust").join("toolchains").join("1.98.0-host"))?;
        let executable = incan_home.join("toolchains").join("0.5.0").join("bin").join("incan");
        fs::create_dir_all(executable.parent().ok_or("executable has no parent")?)?;
        fs::write(&executable, b"")?;
        assert_eq!(
            super::incan_owned_rustup_home_in(None, Some(executable), None),
            Some(incan_home.join("rust"))
        );
        Ok(())
    }

    #[test]
    fn incan_owned_rustup_home_falls_back_to_the_user_home_default() -> Result<(), Box<dyn std::error::Error>> {
        let home = tempfile::tempdir()?;
        fs::create_dir_all(
            home.path()
                .join(".incan")
                .join("rust")
                .join("toolchains")
                .join("1.98.0-host"),
        )?;
        assert_eq!(
            super::incan_owned_rustup_home_in(None, None, Some(home.path().as_os_str().to_os_string())),
            Some(home.path().join(".incan").join("rust"))
        );
        Ok(())
    }

    #[test]
    fn first_diverging_shared_package_reports_a_same_version_byte_distinct_overlap() {
        let leaf = |package: &str, version: &str, digest: &str| OvenRustcRegistryLeaf {
            package: package.to_string(),
            version: version.to_string(),
            crate_name: package.replace('-', "_"),
            features: Vec::new(),
            source: fixture_registry_source(),
            artifact: OvenRustcArtifactExtern {
                crate_name: package.replace('-', "_"),
                relative_path: format!("lib{package}.rlib"),
                digest: digest.to_string(),
            },
        };
        let consumer = OvenRegistryLeafAuthority::new(
            PathBuf::from("/consumer"),
            vec![leaf("tokio", "1.52.3", "sha256:consumer-tokio")],
        );
        let provider = OvenRegistryLeafAuthority::new(
            PathBuf::from("/provider"),
            vec![leaf("tokio", "1.52.3", "sha256:provider-tokio")],
        );
        assert_eq!(
            consumer.first_diverging_shared_package(&provider),
            Some("tokio".to_string()),
            "one package at one version with two byte-distinct compiled artifacts is the exact dangerous shape"
        );

        let identical = OvenRegistryLeafAuthority::new(
            PathBuf::from("/provider"),
            vec![leaf("tokio", "1.52.3", "sha256:consumer-tokio")],
        );
        assert_eq!(
            consumer.first_diverging_shared_package(&identical),
            None,
            "the same compiled bytes on both sides is the harmless shared case"
        );

        let different_version = OvenRegistryLeafAuthority::new(
            PathBuf::from("/provider"),
            vec![leaf("tokio", "1.51.0", "sha256:provider-tokio")],
        );
        assert_eq!(
            consumer.first_diverging_shared_package(&different_version),
            None,
            "distinct versions are ordinary Cargo semver coexistence, not a divergence"
        );

        let unrelated = OvenRegistryLeafAuthority::new(
            PathBuf::from("/provider"),
            vec![leaf("datafusion", "53.1.0", "sha256:provider-datafusion")],
        );
        assert_eq!(consumer.first_diverging_shared_package(&unrelated), None);
    }

    #[test]
    fn selects_one_profile_matched_copy_of_an_equivalent_sealed_registry_leaf() -> Result<(), Box<dyn std::error::Error>>
    {
        let first = tempfile::tempdir()?;
        let second = tempfile::tempdir()?;
        let relative_path = "target/aarch64-apple-darwin/debug/deps/libfixture_registry-1234.rlib";
        let first_artifact = first.path().join(relative_path);
        let second_artifact = second.path().join(relative_path);
        fs::create_dir_all(first_artifact.parent().ok_or("first registry parent")?)?;
        fs::create_dir_all(second_artifact.parent().ok_or("second registry parent")?)?;
        let first_bytes = b"first sealed copy";
        let second_bytes = b"second sealed copy";
        fs::write(&first_artifact, first_bytes)?;
        fs::write(&second_artifact, second_bytes)?;
        let leaf = |digest| OvenRustcRegistryLeaf {
            package: "fixture-registry".to_string(),
            version: "1.0.0".to_string(),
            crate_name: "fixture_registry".to_string(),
            features: vec!["derive".to_string()],
            source: fixture_registry_source(),
            artifact: OvenRustcArtifactExtern {
                crate_name: "fixture_registry".to_string(),
                relative_path: relative_path.to_string(),
                digest,
            },
        };
        let authority = OvenRegistryLeafAuthority::aggregate([
            OvenRegistryLeafAuthority::new(first.path().to_path_buf(), vec![leaf(digest_bytes(first_bytes))]),
            OvenRegistryLeafAuthority::new(second.path().to_path_buf(), vec![leaf(digest_bytes(second_bytes))]),
        ]);
        let dependency = DependencySpec {
            crate_name: "fixture_registry".to_string(),
            version: Some("1".to_string()),
            features: vec!["derive".to_string()],
            default_features: true,
            source: DependencySource::Registry,
            optional: false,
            package: Some("fixture-registry".to_string()),
        };

        let selected = resolve_sealed_registry_leaf(&dependency, Some(&authority), "debug")?;
        let expected = [fs::canonicalize(first_artifact)?, fs::canonicalize(second_artifact)?]
            .into_iter()
            .min()
            .ok_or("expected registry artifact")?;
        assert_eq!(selected, expected);
        Ok(())
    }

    #[test]
    fn compiler_test_profile_has_an_explicit_direct_rustc_contract() {
        let mut command = Command::new("rustc");
        apply_oven_profile(&mut command, OVEN_COMPILER_TEST_PROFILE);
        let arguments = command
            .get_args()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(
            arguments,
            vec![
                "-C",
                "debuginfo=0",
                "-C",
                "strip=debuginfo",
                "-C",
                "debug-assertions=on",
                "-C",
                "overflow-checks=on",
            ]
        );

        let mut developer_profile = Command::new("rustc");
        apply_oven_profile(&mut developer_profile, "debug");
        let developer_arguments = developer_profile
            .get_args()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(developer_arguments, vec!["-C", "opt-level=0"]);
    }

    #[test]
    fn release_profile_optimizes_because_rustc_optimizes_nothing_by_default() {
        // Cargo used to supply this from `[profile.release]`. When Oven replaced Cargo on the normal build path
        // nothing did, and `incan build --release` shipped `opt-level=0` binaries that ran about six times slower
        // than the identical sources at `-C opt-level=3`. Assert the flag rather than trusting a default.
        let mut command = Command::new("rustc");
        apply_oven_profile(&mut command, "release");
        let arguments = command
            .get_args()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(arguments, vec!["-C", "opt-level=3"]);
    }

    #[test]
    fn artifact_manifest_rejects_an_escaping_path() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let manifest = OvenRustcArtifactManifest {
            schema_version: OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION,
            intent: intent(root.path())?.intent,
            dependency_search_paths: vec!["../escape".to_string()],
            native_search_paths: Vec::new(),
            externs: Vec::new(),
            entrypoint_externs: BTreeMap::new(),
            registry_leaves: Vec::new(),
            registry_sources: Vec::new(),
            compile_environment: BTreeMap::new(),
            vocab_auxiliary_targets: Vec::new(),
            supporting_artifacts: Vec::new(),
        };
        let result = manifest.materialize(root.path(), &intent(root.path())?.intent);
        assert!(matches!(result, Err(OvenRustcError::InvalidArtifactPath { .. })));
        Ok(())
    }

    #[test]
    fn composed_trusted_plan_uses_each_foundation_root_without_a_composite_directory()
    -> Result<(), Box<dyn std::error::Error>> {
        let first = tempfile::tempdir()?;
        let second = tempfile::tempdir()?;
        fs::create_dir_all(first.path().join("deps"))?;
        fs::create_dir_all(second.path().join("deps"))?;
        fs::write(first.path().join("deps/libfirst.rlib"), b"first")?;
        fs::write(second.path().join("deps/libsecond.rlib"), b"second")?;
        let receipt = intent(first.path())?;
        let manifest = OvenRustcArtifactManifest {
            schema_version: OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION,
            intent: receipt.intent.clone(),
            dependency_search_paths: vec!["deps".to_string()],
            native_search_paths: Vec::new(),
            externs: Vec::new(),
            entrypoint_externs: BTreeMap::new(),
            registry_leaves: Vec::new(),
            registry_sources: Vec::new(),
            compile_environment: BTreeMap::new(),
            vocab_auxiliary_targets: Vec::new(),
            supporting_artifacts: vec![
                OvenRustcSupportingArtifact {
                    relative_path: "deps/libfirst.rlib".to_string(),
                    digest: "sha256:first".to_string(),
                },
                OvenRustcSupportingArtifact {
                    relative_path: "deps/libsecond.rlib".to_string(),
                    digest: "sha256:second".to_string(),
                },
            ],
        };
        let first_fragment = vec![OvenRustcSupportingArtifact {
            relative_path: "deps/libfirst.rlib".to_string(),
            digest: "sha256:first".to_string(),
        }];
        let second_fragment = vec![OvenRustcSupportingArtifact {
            relative_path: "deps/libsecond.rlib".to_string(),
            digest: "sha256:second".to_string(),
        }];
        let search_paths = vec!["deps".to_string()];
        let roots = [
            OvenTrustedRustcArtifactRoot {
                artifact_root: first.path(),
                dependency_search_paths: &search_paths,
                native_search_paths: &[],
                supporting_artifacts: &first_fragment,
            },
            OvenTrustedRustcArtifactRoot {
                artifact_root: second.path(),
                dependency_search_paths: &search_paths,
                native_search_paths: &[],
                supporting_artifacts: &second_fragment,
            },
        ];

        let plan = manifest.materialize_trusted_store_composed(&roots, &receipt.intent)?;
        let first_deps = fs::canonicalize(first.path().join("deps"))?;
        let second_deps = fs::canonicalize(second.path().join("deps"))?;
        assert_eq!(plan.dependency_search_paths.len(), 2);
        assert!(plan.dependency_search_paths.iter().any(|path| path == &first_deps));
        assert!(plan.dependency_search_paths.iter().any(|path| path == &second_deps));

        let base_paths = BTreeSet::from(["deps/libfirst.rlib".to_string()]);
        let base_manifest = manifest.artifact_fragment(&base_paths)?;
        let partition = manifest.partition_against_base(&base_manifest)?;
        assert_eq!(partition.base_paths, base_paths);
        assert_eq!(
            partition.extension_paths,
            BTreeSet::from(["deps/libsecond.rlib".to_string()])
        );
        let extension_manifest = manifest.artifact_fragment(&partition.extension_paths)?;
        assert_eq!(extension_manifest.supporting_artifacts, second_fragment);

        let mut conflicting_base = base_manifest.clone();
        conflicting_base.supporting_artifacts[0].digest = "sha256:other".to_string();
        let conflict = manifest.partition_against_base(&conflicting_base);
        assert!(matches!(conflict, Err(OvenRustcError::InvalidInput { .. })));
        Ok(())
    }

    #[test]
    fn registry_leaf_substitution_requires_the_same_compilation_identity() {
        // Cargo's `-<hash>` extra-filename suffix summarizes the unit's declared compilation identity, including
        // resolved transitive features and dependency identities. Two closures can share every declared coordinate
        // and still compile a crate to different identities (#1227); substituting across identities removes the only
        // artifact that satisfies retained dependents' recorded hashes. The filename alone is still not a build
        // witness: a base prebuilt on another machine publishes the same filename with a different strict version
        // hash, so the conservative regime additionally demands bit-identical content.
        let leaf = |relative_path: &str, digest_input: &str| OvenRustcRegistryLeaf {
            package: "rand_core".to_string(),
            version: "0.6.4".to_string(),
            crate_name: "rand_core".to_string(),
            features: vec!["std".to_string()],
            source: fixture_registry_source(),
            artifact: OvenRustcArtifactExtern {
                crate_name: "rand_core".to_string(),
                relative_path: relative_path.to_string(),
                digest: digest_bytes(digest_input.as_bytes()),
            },
        };
        let project = leaf("entry/deps/librand_core-cf99342fb36f73de.rlib", "local-build");
        let matching_release = leaf("base/deps/librand_core-cf99342fb36f73de.rlib", "local-build");
        let foreign_release = leaf("base/deps/librand_core-cf99342fb36f73de.rlib", "foreign-build");
        let divergent_release = leaf("base/deps/librand_core-50f0d7ca30c6ae15.rlib", "foreign-build");
        assert!(super::same_registry_leaf_semantics(&project, &divergent_release));
        assert!(
            super::registry_leaf_substitution_is_safe(&project, &matching_release, false),
            "a bit-identical artifact in a different directory is a pure byte-canonicalization"
        );
        assert!(
            !super::registry_leaf_substitution_is_safe(&project, &foreign_release, false),
            "a same-filename artifact from a foreign build has a different SVH: retained dependents cannot load it"
        );
        assert!(
            !super::registry_leaf_substitution_is_safe(&project, &divergent_release, false),
            "a transitive leaf must not substitute across compilation identities: prebuilt dependents recorded its hash"
        );
        assert!(
            super::registry_leaf_substitution_is_safe(&project, &divergent_release, true),
            "a root-extern leaf may cross identities because the generated root recompiles against the release copy"
        );
    }

    #[test]
    fn conservative_regime_keeps_a_root_linked_leaf_on_the_project_identity() -> Result<(), Box<dyn std::error::Error>>
    {
        // `shared` is root-linked and has a release counterpart with different bytes. `engine` — root-linked with
        // no counterpart — keeps its extension-built subtree, and that subtree recorded the project's `shared` by
        // exact identity hash, as does the extension's runtime the root now links. Nothing may cross identities:
        // the root keeps the project's `shared`, re-rooted beside the release twin, or the retained consumer stops
        // loading (#1227, IncQL on a packaged toolchain: `substrait` against the project's `serde`).
        let root = tempfile::tempdir()?;
        let receipt = intent(root.path())?;
        let source = OvenRustcRegistrySource {
            registry: "registry+https://github.com/rust-lang/crates.io-index".to_string(),
            checksum: "shared-checksum".to_string(),
            relative_root: "registry-sources/shared".to_string(),
            digest: "sha256:shared-source".to_string(),
        };
        let registry_source = OvenRustcRegistrySourcePackage {
            package: "shared".to_string(),
            version: "1.0.0".to_string(),
            features: Vec::new(),
            source: source.clone(),
        };
        let leaf = |crate_name: &str, artifact| OvenRustcRegistryLeaf {
            package: crate_name.to_string(),
            version: "1.0.0".to_string(),
            crate_name: crate_name.to_string(),
            features: Vec::new(),
            source: source.clone(),
            artifact,
        };
        let project_shared = OvenRustcArtifactExtern {
            crate_name: "shared".to_string(),
            relative_path: "target/debug/deps/libshared-aaaa.rlib".to_string(),
            digest: "sha256:project-shared".to_string(),
        };
        let base_shared = OvenRustcArtifactExtern {
            crate_name: "shared".to_string(),
            relative_path: "target/debug/deps/libshared-aaaa.rlib".to_string(),
            digest: "sha256:base-shared".to_string(),
        };
        let engine_extern = OvenRustcArtifactExtern {
            crate_name: "engine".to_string(),
            relative_path: "target/debug/deps/libengine-bbbb.rlib".to_string(),
            digest: "sha256:project-engine".to_string(),
        };
        let engine_source = OvenRustcRegistrySourcePackage {
            package: "engine".to_string(),
            ..registry_source.clone()
        };
        let project = OvenRustcArtifactManifest {
            schema_version: OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION,
            intent: receipt.intent.clone(),
            dependency_search_paths: vec!["target/debug/deps".to_string()],
            native_search_paths: Vec::new(),
            externs: vec![
                OvenRustcArtifactExtern {
                    crate_name: "incan_stdlib".to_string(),
                    relative_path: "target/debug/deps/libincan_stdlib-project.rlib".to_string(),
                    digest: "sha256:project-stdlib".to_string(),
                },
                engine_extern.clone(),
                project_shared.clone(),
            ],
            entrypoint_externs: BTreeMap::new(),
            registry_leaves: vec![
                leaf("engine", engine_extern.clone()),
                leaf("shared", project_shared.clone()),
            ],
            registry_sources: vec![registry_source.clone(), engine_source.clone()],
            compile_environment: BTreeMap::new(),
            vocab_auxiliary_targets: Vec::new(),
            // A root-linked leaf's rlib is declared once, as the root extern; only its sidecar is supporting.
            supporting_artifacts: vec![
                OvenRustcSupportingArtifact {
                    relative_path: "target/debug/deps/libshared-aaaa.rmeta".to_string(),
                    digest: "sha256:project-shared-meta".to_string(),
                },
                OvenRustcSupportingArtifact {
                    relative_path: "registry-sources/shared/Cargo.toml".to_string(),
                    digest: "sha256:shared-manifest".to_string(),
                },
            ],
        };
        let base = OvenRustcArtifactManifest {
            schema_version: OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION,
            intent: receipt.intent.clone(),
            dependency_search_paths: vec!["target/debug/deps".to_string()],
            native_search_paths: Vec::new(),
            externs: vec![OvenRustcArtifactExtern {
                crate_name: "incan_stdlib".to_string(),
                relative_path: "target/debug/deps/libincan_stdlib-release.rlib".to_string(),
                digest: "sha256:release-stdlib".to_string(),
            }],
            entrypoint_externs: BTreeMap::new(),
            registry_leaves: vec![leaf("shared", base_shared.clone())],
            registry_sources: vec![registry_source.clone()],
            compile_environment: BTreeMap::new(),
            vocab_auxiliary_targets: Vec::new(),
            supporting_artifacts: vec![
                OvenRustcSupportingArtifact {
                    relative_path: base_shared.relative_path.clone(),
                    digest: base_shared.digest.clone(),
                },
                OvenRustcSupportingArtifact {
                    relative_path: "target/debug/deps/libshared-aaaa.rmeta".to_string(),
                    digest: "sha256:base-shared-meta".to_string(),
                },
                OvenRustcSupportingArtifact {
                    relative_path: "registry-sources/shared/Cargo.toml".to_string(),
                    digest: "sha256:shared-manifest".to_string(),
                },
            ],
        };
        let composed = project
            .with_release_cohort_from_base(&base, &BTreeSet::new())
            .map_err(|error| format!("first composition: {error:?}"))?;
        // The regime retains extension-built consumers, so the root keeps the extension's runtime: one unified
        // resolution links once, and a process-global runtime cannot be brought in twice through the base.
        let runtime = composed
            .externs
            .iter()
            .find(|artifact| artifact.crate_name == "incan_stdlib")
            .ok_or("composed plan lost the runtime extern")?;
        assert_eq!(
            (runtime.relative_path.as_str(), runtime.digest.as_str()),
            (
                "target/debug/deps/libincan_stdlib-project.rlib",
                "sha256:project-stdlib"
            ),
            "the conservative regime links the extension's runtime, not the base's"
        );
        let root_link = composed
            .externs
            .iter()
            .find(|artifact| artifact.crate_name == "shared")
            .ok_or("composed plan lost the shared root extern")?;
        assert_eq!(
            (root_link.relative_path.as_str(), root_link.digest.as_str()),
            (
                "target/debug/extension-deps/libshared-aaaa.rlib",
                "sha256:project-shared"
            ),
            "the root keeps the project's copy, re-rooted beside the release twin"
        );
        let leaf_record = composed
            .registry_leaves
            .iter()
            .find(|candidate| candidate.crate_name == "shared")
            .ok_or("composed plan lost the shared leaf")?;
        assert_eq!(
            (
                leaf_record.artifact.relative_path.as_str(),
                leaf_record.artifact.digest.as_str()
            ),
            (
                "target/debug/extension-deps/libshared-aaaa.rlib",
                "sha256:project-shared"
            )
        );
        let supporting_paths = composed
            .supporting_artifacts
            .iter()
            .map(|artifact| (artifact.relative_path.as_str(), artifact.digest.as_str()))
            .collect::<BTreeSet<_>>();
        assert!(
            supporting_paths.contains(&(
                "target/debug/extension-deps/libshared-aaaa.rmeta",
                "sha256:project-shared-meta"
            )),
            "the split-metadata sidecar must move with the project's rlib"
        );
        // The base's copy and sidecar join the plan at their own paths for the sealed runtime's execution closure.
        assert!(
            supporting_paths.contains(&("target/debug/deps/libshared-aaaa.rlib", "sha256:base-shared")),
            "the base copy must join the plan at its own path"
        );
        assert!(
            supporting_paths.contains(&("target/debug/deps/libshared-aaaa.rmeta", "sha256:base-shared-meta")),
            "the base sidecar must join the plan beside the base copy"
        );
        assert_eq!(
            composed
                .declared_artifact_paths()?
                .iter()
                .filter(|path| path.ends_with("libshared-aaaa.rlib"))
                .count(),
            2,
            "exactly the base copy in `deps` and the project copy in `extension-deps` are declared"
        );
        assert!(
            !supporting_paths.contains(&("target/debug/deps/libshared-aaaa.rlib", "sha256:project-shared")),
            "the project's copy must not remain at the colliding path"
        );
        assert!(
            composed
                .dependency_search_paths
                .iter()
                .any(|path| path == "target/debug/extension-deps"),
            "the re-rooted directory must join the search paths"
        );
        // Recomposition is a fixed point: the stored composed plan validates against its base unchanged.
        let recomposed = composed
            .with_release_cohort_from_base(&base, &BTreeSet::new())
            .map_err(|error| format!("recomposition: {error:?}"))?;
        assert_eq!(recomposed, composed);
        Ok(())
    }

    #[test]
    fn conservative_regime_reroots_retained_leaves_that_collide_with_a_foreign_base_copy()
    -> Result<(), Box<dyn std::error::Error>> {
        // A salted extension unit shares its Cargo filename with the sealed base's twin while carrying a distinct
        // StableCrateId, so the composed plan must keep the project's copy — re-rooted into `extension-deps` with
        // its split-metadata sidecar — while the base's copy joins the plan at its own path for the sealed runtime,
        // and rustc selects each dependent's copy by recorded hash.
        let root = tempfile::tempdir()?;
        let receipt = intent(root.path())?;
        let source = OvenRustcRegistrySource {
            registry: "registry+https://github.com/rust-lang/crates.io-index".to_string(),
            checksum: "cfg-ish-checksum".to_string(),
            relative_root: "registry-sources/cfg_ish".to_string(),
            digest: "sha256:cfg-ish-source".to_string(),
        };
        let registry_source = OvenRustcRegistrySourcePackage {
            package: "cfg_ish".to_string(),
            version: "1.0.0".to_string(),
            features: Vec::new(),
            source: source.clone(),
        };
        let leaf = |crate_name: &str, artifact| OvenRustcRegistryLeaf {
            package: crate_name.replace('_', "-"),
            version: "1.0.0".to_string(),
            crate_name: crate_name.to_string(),
            features: Vec::new(),
            source: source.clone(),
            artifact,
        };
        let project_cfg_ish = OvenRustcArtifactExtern {
            crate_name: "cfg_ish".to_string(),
            relative_path: "target/debug/deps/libcfg_ish-aaaa.rlib".to_string(),
            digest: "sha256:project-cfg-ish".to_string(),
        };
        let engine_extern = OvenRustcArtifactExtern {
            crate_name: "engine".to_string(),
            relative_path: "target/debug/deps/libengine-bbbb.rlib".to_string(),
            digest: "sha256:project-engine".to_string(),
        };
        let cfg_ish_source = OvenRustcRegistrySourcePackage {
            package: "cfg-ish".to_string(),
            ..registry_source.clone()
        };
        let engine_source = OvenRustcRegistrySourcePackage {
            package: "engine".to_string(),
            ..registry_source.clone()
        };
        let project = OvenRustcArtifactManifest {
            schema_version: OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION,
            intent: receipt.intent.clone(),
            dependency_search_paths: vec!["target/debug/deps".to_string()],
            native_search_paths: Vec::new(),
            externs: vec![
                OvenRustcArtifactExtern {
                    crate_name: "incan_stdlib".to_string(),
                    relative_path: "target/debug/deps/libincan_stdlib-project.rlib".to_string(),
                    digest: "sha256:project-stdlib".to_string(),
                },
                engine_extern.clone(),
            ],
            entrypoint_externs: BTreeMap::new(),
            registry_leaves: vec![
                leaf("engine", engine_extern.clone()),
                leaf("cfg_ish", project_cfg_ish.clone()),
            ],
            registry_sources: vec![cfg_ish_source.clone(), engine_source.clone()],
            compile_environment: BTreeMap::new(),
            vocab_auxiliary_targets: Vec::new(),
            supporting_artifacts: vec![
                OvenRustcSupportingArtifact {
                    relative_path: project_cfg_ish.relative_path.clone(),
                    digest: project_cfg_ish.digest.clone(),
                },
                OvenRustcSupportingArtifact {
                    relative_path: "target/debug/deps/libcfg_ish-aaaa.rmeta".to_string(),
                    digest: "sha256:project-cfg-ish-meta".to_string(),
                },
                OvenRustcSupportingArtifact {
                    relative_path: "registry-sources/cfg_ish/Cargo.toml".to_string(),
                    digest: "sha256:cfg-ish-manifest".to_string(),
                },
            ],
        };
        let base = OvenRustcArtifactManifest {
            schema_version: OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION,
            intent: receipt.intent.clone(),
            dependency_search_paths: vec!["target/debug/deps".to_string()],
            native_search_paths: Vec::new(),
            externs: vec![OvenRustcArtifactExtern {
                crate_name: "incan_stdlib".to_string(),
                relative_path: "target/debug/deps/libincan_stdlib-release.rlib".to_string(),
                digest: "sha256:release-stdlib".to_string(),
            }],
            entrypoint_externs: BTreeMap::new(),
            registry_leaves: vec![leaf(
                "cfg_ish",
                OvenRustcArtifactExtern {
                    crate_name: "cfg_ish".to_string(),
                    relative_path: "target/debug/deps/libcfg_ish-aaaa.rlib".to_string(),
                    digest: "sha256:base-cfg-ish".to_string(),
                },
            )],
            registry_sources: vec![cfg_ish_source.clone()],
            compile_environment: BTreeMap::new(),
            vocab_auxiliary_targets: Vec::new(),
            supporting_artifacts: vec![
                OvenRustcSupportingArtifact {
                    relative_path: "target/debug/deps/libcfg_ish-aaaa.rlib".to_string(),
                    digest: "sha256:base-cfg-ish".to_string(),
                },
                OvenRustcSupportingArtifact {
                    relative_path: "target/debug/deps/libcfg_ish-aaaa.rmeta".to_string(),
                    digest: "sha256:base-cfg-ish-meta".to_string(),
                },
                OvenRustcSupportingArtifact {
                    relative_path: "registry-sources/cfg_ish/Cargo.toml".to_string(),
                    digest: "sha256:cfg-ish-manifest".to_string(),
                },
            ],
        };

        // `engine` is root-linked with no base counterpart, so the plan retains extension-built consumers and the
        // shared-filename `cfg_ish` leaf must not adopt the base bytes its dependents never recorded.
        let composed = project.with_release_cohort_from_base(&base, &BTreeSet::new())?;
        let rerooted = composed
            .registry_leaves
            .iter()
            .find(|candidate| candidate.crate_name == "cfg_ish")
            .ok_or("composed plan lost the retained cfg_ish leaf")?;
        assert_eq!(
            rerooted.artifact.relative_path,
            "target/debug/extension-deps/libcfg_ish-aaaa.rlib"
        );
        assert_eq!(rerooted.artifact.digest, "sha256:project-cfg-ish");
        let supporting_paths = composed
            .supporting_artifacts
            .iter()
            .map(|artifact| (artifact.relative_path.as_str(), artifact.digest.as_str()))
            .collect::<BTreeSet<_>>();
        assert!(
            supporting_paths.contains(&(
                "target/debug/extension-deps/libcfg_ish-aaaa.rlib",
                "sha256:project-cfg-ish"
            )),
            "the retained rlib must move to extension-deps with its project digest"
        );
        assert!(
            supporting_paths.contains(&(
                "target/debug/extension-deps/libcfg_ish-aaaa.rmeta",
                "sha256:project-cfg-ish-meta"
            )),
            "the split-metadata sidecar must move with its rlib"
        );
        assert!(
            supporting_paths.contains(&("target/debug/deps/libcfg_ish-aaaa.rlib", "sha256:base-cfg-ish")),
            "the base copy must join the plan at its own path for the sealed runtime"
        );
        assert!(
            composed
                .dependency_search_paths
                .iter()
                .any(|path| path == "target/debug/extension-deps"),
            "the re-rooted directory must join the dependency search paths"
        );
        composed.validate_release_cohort_from_base(&base)?;
        let partition = composed.partition_against_base(&base)?;
        assert!(
            partition
                .extension_paths
                .contains("target/debug/extension-deps/libcfg_ish-aaaa.rlib")
        );
        assert!(partition.base_paths.contains("target/debug/deps/libcfg_ish-aaaa.rlib"));
        assert_eq!(
            super::rerooted_artifact_staging_source("target/debug/extension-deps/libcfg_ish-aaaa.rlib").as_deref(),
            Some("target/debug/deps/libcfg_ish-aaaa.rlib")
        );
        Ok(())
    }

    #[test]
    fn project_extension_replaces_the_complete_release_execution_cohort_with_base_artifacts()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let receipt = intent(root.path())?;
        let project = OvenRustcArtifactManifest {
            schema_version: OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION,
            intent: receipt.intent.clone(),
            dependency_search_paths: vec!["deps".to_string()],
            native_search_paths: Vec::new(),
            externs: vec![
                OvenRustcArtifactExtern {
                    crate_name: "incan_stdlib".to_string(),
                    relative_path: "deps/libincan_stdlib-project.rlib".to_string(),
                    digest: "sha256:project-runtime".to_string(),
                },
                OvenRustcArtifactExtern {
                    crate_name: "project_dependency".to_string(),
                    relative_path: "deps/libproject_dependency.rlib".to_string(),
                    digest: "sha256:project-dependency".to_string(),
                },
                OvenRustcArtifactExtern {
                    crate_name: "incan_partner".to_string(),
                    relative_path: "deps/libincan_partner-project.rlib".to_string(),
                    digest: "sha256:project-partner".to_string(),
                },
            ],
            entrypoint_externs: BTreeMap::new(),
            registry_leaves: Vec::new(),
            registry_sources: Vec::new(),
            compile_environment: BTreeMap::new(),
            vocab_auxiliary_targets: Vec::new(),
            supporting_artifacts: vec![
                OvenRustcSupportingArtifact {
                    relative_path: "deps/libincan_core-project.rlib".to_string(),
                    digest: "sha256:project-core".to_string(),
                },
                OvenRustcSupportingArtifact {
                    relative_path: "deps/libincan_derive-project.dylib".to_string(),
                    digest: "sha256:project-derive".to_string(),
                },
                OvenRustcSupportingArtifact {
                    relative_path: "deps/libincan_partner_helper-project.rlib".to_string(),
                    digest: "sha256:project-partner-helper".to_string(),
                },
                OvenRustcSupportingArtifact {
                    relative_path: "registry-sources/shared/Cargo.toml".to_string(),
                    digest: "sha256:shared-source".to_string(),
                },
            ],
        };
        let base = OvenRustcArtifactManifest {
            schema_version: OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION,
            intent: receipt.intent.clone(),
            dependency_search_paths: vec!["deps".to_string()],
            native_search_paths: Vec::new(),
            externs: vec![OvenRustcArtifactExtern {
                crate_name: "incan_stdlib".to_string(),
                relative_path: "deps/libincan_stdlib-release.rlib".to_string(),
                digest: "sha256:release-runtime".to_string(),
            }],
            entrypoint_externs: BTreeMap::new(),
            registry_leaves: Vec::new(),
            registry_sources: Vec::new(),
            compile_environment: BTreeMap::new(),
            vocab_auxiliary_targets: vec![OvenRustcAuxiliaryTarget {
                target: "wasm32-wasip1".to_string(),
                dependency_search_paths: vec!["vocab/deps".to_string()],
                externs: vec![OvenRustcArtifactExtern {
                    crate_name: "incan_vocab".to_string(),
                    relative_path: "vocab/deps/libincan_vocab-release.rlib".to_string(),
                    digest: "sha256:release-vocab".to_string(),
                }],
            }],
            supporting_artifacts: vec![
                OvenRustcSupportingArtifact {
                    relative_path: "deps/libincan_core-release.rlib".to_string(),
                    digest: "sha256:release-core".to_string(),
                },
                OvenRustcSupportingArtifact {
                    relative_path: "deps/libincan_derive-release.dylib".to_string(),
                    digest: "sha256:release-derive".to_string(),
                },
                OvenRustcSupportingArtifact {
                    relative_path: "deps/libincan_stdlib_system-release.rlib".to_string(),
                    digest: "sha256:release-system".to_string(),
                },
                OvenRustcSupportingArtifact {
                    relative_path: "deps/libbase_runtime_dependency.rlib".to_string(),
                    digest: "sha256:base-runtime-dependency".to_string(),
                },
                OvenRustcSupportingArtifact {
                    relative_path: "vocab/deps/libvocab_dependency.rlib".to_string(),
                    digest: "sha256:vocab-dependency".to_string(),
                },
                OvenRustcSupportingArtifact {
                    relative_path: "registry-sources/shared/Cargo.toml".to_string(),
                    digest: "sha256:shared-source".to_string(),
                },
                OvenRustcSupportingArtifact {
                    relative_path: "registry-sources/base-only/Cargo.toml".to_string(),
                    digest: "sha256:base-only-source".to_string(),
                },
            ],
        };

        assert_eq!(
            base.compiler_runtime_crate_names()?,
            BTreeSet::from([
                "incan_core".to_string(),
                "incan_derive".to_string(),
                "incan_stdlib".to_string(),
                "incan_stdlib_system".to_string(),
            ])
        );
        let composed = project.with_release_cohort_from_base(&base, &BTreeSet::new())?;
        assert_eq!(composed.externs[0].relative_path, "deps/libincan_stdlib-release.rlib");
        assert_eq!(composed.externs[1], project.externs[1]);
        assert_eq!(composed.externs[2], project.externs[2]);
        assert_eq!(composed.vocab_auxiliary_targets, base.vocab_auxiliary_targets);
        assert_eq!(
            composed.supporting_artifacts,
            vec![
                OvenRustcSupportingArtifact {
                    relative_path: "deps/libbase_runtime_dependency.rlib".to_string(),
                    digest: "sha256:base-runtime-dependency".to_string(),
                },
                OvenRustcSupportingArtifact {
                    relative_path: "deps/libincan_core-release.rlib".to_string(),
                    digest: "sha256:release-core".to_string(),
                },
                OvenRustcSupportingArtifact {
                    relative_path: "deps/libincan_derive-release.dylib".to_string(),
                    digest: "sha256:release-derive".to_string(),
                },
                OvenRustcSupportingArtifact {
                    relative_path: "deps/libincan_partner_helper-project.rlib".to_string(),
                    digest: "sha256:project-partner-helper".to_string(),
                },
                OvenRustcSupportingArtifact {
                    relative_path: "deps/libincan_stdlib_system-release.rlib".to_string(),
                    digest: "sha256:release-system".to_string(),
                },
                OvenRustcSupportingArtifact {
                    relative_path: "registry-sources/shared/Cargo.toml".to_string(),
                    digest: "sha256:shared-source".to_string(),
                },
                OvenRustcSupportingArtifact {
                    relative_path: "vocab/deps/libvocab_dependency.rlib".to_string(),
                    digest: "sha256:vocab-dependency".to_string(),
                },
            ]
        );
        assert!(
            composed
                .supporting_artifacts
                .iter()
                .all(|artifact| artifact.relative_path != "registry-sources/base-only/Cargo.toml")
        );
        let partition = composed.partition_against_base(&base)?;
        assert_eq!(
            partition.base_paths,
            BTreeSet::from([
                "deps/libbase_runtime_dependency.rlib".to_string(),
                "deps/libincan_core-release.rlib".to_string(),
                "deps/libincan_derive-release.dylib".to_string(),
                "deps/libincan_stdlib-release.rlib".to_string(),
                "deps/libincan_stdlib_system-release.rlib".to_string(),
                "registry-sources/shared/Cargo.toml".to_string(),
                "vocab/deps/libincan_vocab-release.rlib".to_string(),
                "vocab/deps/libvocab_dependency.rlib".to_string(),
            ])
        );
        assert_eq!(
            partition.extension_paths,
            BTreeSet::from([
                "deps/libincan_partner-project.rlib".to_string(),
                "deps/libincan_partner_helper-project.rlib".to_string(),
                "deps/libproject_dependency.rlib".to_string(),
            ])
        );
        assert!(partition.extension_paths.contains("deps/libincan_partner-project.rlib"));

        let mut incomplete_base = base.clone();
        incomplete_base.externs.clear();
        let incomplete = project.with_release_cohort_from_base(&incomplete_base, &BTreeSet::new());
        assert!(matches!(incomplete, Err(OvenRustcError::InvalidInput { .. })));
        Ok(())
    }

    #[test]
    fn project_extension_canonicalizes_the_locked_release_registry_cohort() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let receipt = intent(root.path())?;
        let source = OvenRustcRegistrySource {
            registry: "registry+https://github.com/rust-lang/crates.io-index".to_string(),
            checksum: "serde-checksum".to_string(),
            relative_root: "registry-sources/serde".to_string(),
            digest: "sha256:serde-source".to_string(),
        };
        let registry_source = OvenRustcRegistrySourcePackage {
            package: "serde".to_string(),
            version: "1.0.228".to_string(),
            features: vec!["derive".to_string()],
            source: source.clone(),
        };
        let release_registry_source = OvenRustcRegistrySourcePackage {
            features: vec!["default".to_string(), "derive".to_string()],
            ..registry_source.clone()
        };
        let release_serde = OvenRustcArtifactExtern {
            crate_name: "serde".to_string(),
            relative_path: "deps/libserde-shared.rlib".to_string(),
            digest: "sha256:release-serde".to_string(),
        };
        let project_serde = OvenRustcArtifactExtern {
            digest: "sha256:publisher-local-serde".to_string(),
            ..release_serde.clone()
        };
        let leaf = |features: &[&str], artifact| OvenRustcRegistryLeaf {
            package: "serde".to_string(),
            version: "1.0.228".to_string(),
            crate_name: "serde".to_string(),
            features: features.iter().map(|feature| (*feature).to_string()).collect(),
            source: source.clone(),
            artifact,
        };
        let mut project = OvenRustcArtifactManifest {
            schema_version: OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION,
            intent: receipt.intent.clone(),
            dependency_search_paths: vec!["deps".to_string()],
            native_search_paths: Vec::new(),
            externs: vec![
                OvenRustcArtifactExtern {
                    crate_name: "incan_stdlib".to_string(),
                    relative_path: "deps/libincan_stdlib-project.rlib".to_string(),
                    digest: "sha256:project-stdlib".to_string(),
                },
                project_serde.clone(),
                OvenRustcArtifactExtern {
                    crate_name: "project_only".to_string(),
                    relative_path: "deps/libproject_only.rlib".to_string(),
                    digest: "sha256:project-only".to_string(),
                },
            ],
            entrypoint_externs: BTreeMap::new(),
            registry_leaves: vec![leaf(&["derive"], project_serde)],
            registry_sources: vec![registry_source.clone()],
            compile_environment: BTreeMap::new(),
            vocab_auxiliary_targets: Vec::new(),
            supporting_artifacts: vec![
                OvenRustcSupportingArtifact {
                    relative_path: "deps/libserde_derive-shared.dylib".to_string(),
                    digest: "sha256:publisher-local-derive".to_string(),
                },
                OvenRustcSupportingArtifact {
                    relative_path: "registry-sources/serde/Cargo.toml".to_string(),
                    digest: "sha256:serde-manifest".to_string(),
                },
            ],
        };
        let base = OvenRustcArtifactManifest {
            schema_version: OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION,
            intent: receipt.intent,
            dependency_search_paths: vec!["deps".to_string()],
            native_search_paths: Vec::new(),
            externs: vec![
                OvenRustcArtifactExtern {
                    crate_name: "incan_stdlib".to_string(),
                    relative_path: "deps/libincan_stdlib-release.rlib".to_string(),
                    digest: "sha256:release-stdlib".to_string(),
                },
                release_serde.clone(),
            ],
            entrypoint_externs: BTreeMap::new(),
            registry_leaves: vec![leaf(&["derive"], release_serde.clone())],
            registry_sources: vec![release_registry_source.clone()],
            compile_environment: BTreeMap::new(),
            vocab_auxiliary_targets: Vec::new(),
            supporting_artifacts: vec![
                OvenRustcSupportingArtifact {
                    relative_path: "deps/libserde_derive-shared.dylib".to_string(),
                    digest: "sha256:release-derive".to_string(),
                },
                OvenRustcSupportingArtifact {
                    relative_path: "registry-sources/serde/Cargo.toml".to_string(),
                    digest: "sha256:serde-manifest".to_string(),
                },
            ],
        };

        let composed = project.with_release_cohort_from_base(&base, &BTreeSet::new())?;
        assert_eq!(composed.registry_leaves[0].artifact, release_serde);
        assert_eq!(composed.registry_leaves[0].features, ["derive"]);
        assert_eq!(composed.registry_sources[0], release_registry_source);
        assert_eq!(
            composed
                .supporting_artifacts
                .iter()
                .find(|artifact| artifact.relative_path == "deps/libserde_derive-shared.dylib")
                .map(|artifact| artifact.digest.as_str()),
            Some("sha256:release-derive")
        );
        let partition = composed.partition_against_base(&base)?;
        assert!(partition.base_paths.contains("deps/libserde-shared.rlib"));
        assert!(partition.base_paths.contains("deps/libserde_derive-shared.dylib"));
        assert!(partition.extension_paths.contains("deps/libproject_only.rlib"));

        let mut project_with_distinct_leaf_features = project.clone();
        let feature_artifact = OvenRustcArtifactExtern {
            crate_name: "serde".to_string(),
            relative_path: "deps/libserde-project-feature.rlib".to_string(),
            digest: "sha256:project-feature-serde".to_string(),
        };
        // The feature-divergent leaf forces the conservative regime, where a shared unit with different bytes is a
        // reproducibility failure. The publisher's serde_derive is the same locked unit as the release's, so under
        // deterministic path remapping it reproduces the release bytes exactly.
        project_with_distinct_leaf_features.supporting_artifacts[0].digest = "sha256:release-derive".to_string();
        project_with_distinct_leaf_features.externs[1] = feature_artifact.clone();
        project_with_distinct_leaf_features.registry_leaves[0]
            .features
            .push("rc".to_string());
        project_with_distinct_leaf_features.registry_leaves[0].artifact = feature_artifact.clone();
        project_with_distinct_leaf_features.registry_sources[0]
            .features
            .push("rc".to_string());
        let feature_distinct =
            project_with_distinct_leaf_features.with_release_cohort_from_base(&base, &BTreeSet::new())?;
        assert_eq!(feature_distinct.registry_leaves[0].artifact, feature_artifact);
        assert!(
            feature_distinct
                .partition_against_base(&base)?
                .extension_paths
                .contains("deps/libserde-project-feature.rlib")
        );

        let mut project_with_alternate = project.clone();
        project_with_alternate
            .registry_sources
            .push(OvenRustcRegistrySourcePackage {
                package: "serde".to_string(),
                version: "2.0.0".to_string(),
                features: vec!["alloc".to_string()],
                source: OvenRustcRegistrySource {
                    registry: source.registry.clone(),
                    checksum: "project-serde-v2-checksum".to_string(),
                    relative_root: "registry-sources/serde-v2".to_string(),
                    digest: "sha256:project-serde-v2-source".to_string(),
                },
            });
        project_with_alternate
            .supporting_artifacts
            .push(OvenRustcSupportingArtifact {
                relative_path: "registry-sources/serde-v2/Cargo.toml".to_string(),
                digest: "sha256:project-serde-v2-manifest".to_string(),
            });
        let composed_with_alternate = project_with_alternate.with_release_cohort_from_base(&base, &BTreeSet::new())?;
        assert!(composed_with_alternate.registry_sources.iter().any(|package| {
            package.package == "serde"
                && package.version == "2.0.0"
                && package.source.checksum == "project-serde-v2-checksum"
        }));
        assert!(
            composed_with_alternate
                .partition_against_base(&base)?
                .extension_paths
                .contains("registry-sources/serde-v2/Cargo.toml")
        );

        project.registry_sources[0].features.push("rc".to_string());
        let feature_extended = project.with_release_cohort_from_base(&base, &BTreeSet::new())?;
        assert_eq!(feature_extended.registry_sources[0].features, ["derive", "rc"]);
        project.registry_sources[0].source.checksum = "mismatched-checksum".to_string();
        let mismatch = project.with_release_cohort_from_base(&base, &BTreeSet::new());
        assert!(matches!(mismatch, Err(OvenRustcError::InvalidInput { .. })));
        Ok(())
    }

    #[test]
    fn project_extension_keeps_its_own_build_script_leaf_despite_matching_release_semantics()
    -> Result<(), Box<dyn std::error::Error>> {
        // Regression: two independent builds of a build-script crate (like `libc`) can compile to link-incompatible
        // artifacts even when package, version, source checksum, and declared features all match, because a build
        // script can read ambient build-environment state that isn't captured by any of those coordinates. Matching
        // "leaf semantics" alone must not be trusted as proof of link compatibility for such a crate.
        let root = tempfile::tempdir()?;
        let receipt = intent(root.path())?;
        let source = OvenRustcRegistrySource {
            registry: "registry+https://github.com/rust-lang/crates.io-index".to_string(),
            checksum: "libc-checksum".to_string(),
            relative_root: "registry-sources/libc".to_string(),
            digest: "sha256:libc-source".to_string(),
        };
        let registry_source = OvenRustcRegistrySourcePackage {
            package: "libc".to_string(),
            version: "0.2.155".to_string(),
            features: vec!["default".to_string()],
            source: source.clone(),
        };
        // Build-script output is staged under `build/<package>/<identity>/out/`, unlike an ordinary crate's flat
        // `deps/` output; this is the structural signal that distinguishes it from a crate like `serde`.
        let release_libc = OvenRustcArtifactExtern {
            crate_name: "libc".to_string(),
            relative_path: "build/libc/release-identity/out/liblibc-release-identity.rlib".to_string(),
            digest: "sha256:release-libc".to_string(),
        };
        let project_libc = OvenRustcArtifactExtern {
            relative_path: "build/libc/project-identity/out/liblibc-project-identity.rlib".to_string(),
            digest: "sha256:project-libc".to_string(),
            ..release_libc.clone()
        };
        let leaf = |artifact| OvenRustcRegistryLeaf {
            package: "libc".to_string(),
            version: "0.2.155".to_string(),
            crate_name: "libc".to_string(),
            features: vec!["default".to_string()],
            source: source.clone(),
            artifact,
        };
        let project = OvenRustcArtifactManifest {
            schema_version: OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION,
            intent: receipt.intent.clone(),
            dependency_search_paths: vec!["build/libc/project-identity/out".to_string()],
            native_search_paths: Vec::new(),
            externs: vec![
                OvenRustcArtifactExtern {
                    crate_name: "incan_stdlib".to_string(),
                    relative_path: "deps/libincan_stdlib-project.rlib".to_string(),
                    digest: "sha256:project-stdlib".to_string(),
                },
                project_libc.clone(),
            ],
            entrypoint_externs: BTreeMap::new(),
            registry_leaves: vec![leaf(project_libc.clone())],
            registry_sources: vec![registry_source.clone()],
            compile_environment: BTreeMap::new(),
            vocab_auxiliary_targets: Vec::new(),
            supporting_artifacts: vec![OvenRustcSupportingArtifact {
                relative_path: "registry-sources/libc/Cargo.toml".to_string(),
                digest: "sha256:libc-manifest".to_string(),
            }],
        };
        let base = OvenRustcArtifactManifest {
            schema_version: OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION,
            intent: receipt.intent,
            dependency_search_paths: vec!["build/libc/release-identity/out".to_string()],
            native_search_paths: Vec::new(),
            externs: vec![
                OvenRustcArtifactExtern {
                    crate_name: "incan_stdlib".to_string(),
                    relative_path: "deps/libincan_stdlib-release.rlib".to_string(),
                    digest: "sha256:release-stdlib".to_string(),
                },
                release_libc.clone(),
            ],
            entrypoint_externs: BTreeMap::new(),
            registry_leaves: vec![leaf(release_libc)],
            registry_sources: vec![registry_source],
            compile_environment: BTreeMap::new(),
            vocab_auxiliary_targets: Vec::new(),
            supporting_artifacts: vec![OvenRustcSupportingArtifact {
                relative_path: "registry-sources/libc/Cargo.toml".to_string(),
                digest: "sha256:libc-manifest".to_string(),
            }],
        };

        let composed = project.with_release_cohort_from_base(&base, &BTreeSet::new())?;
        assert_eq!(
            composed.registry_leaves[0].artifact, project_libc,
            "a build-script leaf must keep the project's own compiled artifact, not the base's independently built one"
        );
        assert_eq!(composed.externs[1], project_libc);
        Ok(())
    }

    #[test]
    fn project_extension_source_authority_requires_the_same_recomposed_payload_as_execution()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let receipt = intent(root.path())?;
        let source = OvenRustcRegistrySource {
            registry: "registry+https://github.com/rust-lang/crates.io-index".to_string(),
            checksum: "project-only-checksum".to_string(),
            relative_root: "registry-sources/project-only".to_string(),
            digest: "sha256:project-only-source".to_string(),
        };
        let registry_source = OvenRustcRegistrySourcePackage {
            package: "project-only".to_string(),
            version: "1.0.0".to_string(),
            features: vec!["default".to_string()],
            source,
        };
        let mut publisher = empty_manifest(&receipt);
        publisher.dependency_search_paths = vec!["deps".to_string()];
        publisher.externs = vec![OvenRustcArtifactExtern {
            crate_name: "incan_stdlib".to_string(),
            relative_path: "deps/libincan_stdlib-project.rlib".to_string(),
            digest: "sha256:project-stdlib".to_string(),
        }];
        publisher.registry_sources = vec![registry_source];
        publisher.supporting_artifacts = vec![OvenRustcSupportingArtifact {
            relative_path: "registry-sources/project-only/Cargo.toml".to_string(),
            digest: "sha256:project-only-manifest".to_string(),
        }];
        let mut base = empty_manifest(&receipt);
        base.dependency_search_paths = vec!["deps".to_string()];
        base.externs = vec![OvenRustcArtifactExtern {
            crate_name: "incan_stdlib".to_string(),
            relative_path: "deps/libincan_stdlib-release.rlib".to_string(),
            digest: "sha256:release-stdlib".to_string(),
        }];
        let complete = publisher.with_release_cohort_from_base(&base, &BTreeSet::new())?;
        let partition = complete.partition_against_base(&base)?;
        let payload = OvenProjectExtensionPayload {
            schema_version: OVEN_PROJECT_EXTENSION_PAYLOAD_SCHEMA_VERSION,
            base_loaf_identity: "sha256:release-loaf".to_string(),
            base_build_unit_identity: "sha256:release-unit".to_string(),
            publisher_plan: publisher,
            complete_plan: complete,
            registry_source_dependencies: vec![OvenProjectRegistrySourceDependency {
                alias: "project_only".to_string(),
                package: "project-only".to_string(),
                version: "1.0.0".to_string(),
                registry: "registry+https://github.com/rust-lang/crates.io-index".to_string(),
                checksum: "project-only-checksum".to_string(),
            }],
            dev_registry_source_dependencies: Vec::new(),
            extension_paths: partition.extension_paths.iter().cloned().collect(),
        };
        assert_eq!(
            validate_project_extension_payload_against_base(
                &payload,
                "sha256:release-loaf",
                "sha256:release-unit",
                &base,
            )?,
            partition
        );

        let mut malformed_fragment = payload.clone();
        malformed_fragment
            .extension_paths
            .push("undeclared/artifact.rlib".to_string());
        let Err(error) = super::validate_project_extension_payload_shape(&malformed_fragment, &receipt.intent) else {
            return Err(std::io::Error::other("malformed stored extension fragment was accepted").into());
        };
        assert!(error.to_string().contains("strictly sorted"));

        let mut malformed_dev_root = payload.clone();
        malformed_dev_root.dev_registry_source_dependencies = vec![OvenProjectRegistrySourceDependency {
            alias: "missing_dev".to_string(),
            package: "missing-dev".to_string(),
            version: "1.0.0".to_string(),
            registry: "registry+https://github.com/rust-lang/crates.io-index".to_string(),
            checksum: "missing-dev-checksum".to_string(),
        }];
        let Err(error) = super::validate_project_extension_payload_shape(&malformed_dev_root, &receipt.intent) else {
            return Err(std::io::Error::other("malformed stored extension dev root was accepted").into());
        };
        assert!(error.to_string().contains("exact records"));

        let mut mismatched = payload;
        mismatched.complete_plan.registry_sources[0]
            .features
            .push("payload-only-drift".to_string());
        let Err(error) = validate_project_extension_payload_against_base(
            &mismatched,
            "sha256:release-loaf",
            "sha256:release-unit",
            &base,
        ) else {
            return Err(std::io::Error::other("mismatched source-authority payload was accepted").into());
        };
        assert!(matches!(error, OvenRustcError::InvalidInput { .. }));
        Ok(())
    }

    #[test]
    fn artifact_manifest_rejects_an_empty_search_directory() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let empty = root.path().join("empty");
        fs::create_dir(&empty)?;
        let receipt = intent(root.path())?;
        let manifest = OvenRustcArtifactManifest {
            schema_version: OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION,
            intent: receipt.intent.clone(),
            dependency_search_paths: vec!["empty".to_string()],
            native_search_paths: Vec::new(),
            externs: Vec::new(),
            entrypoint_externs: BTreeMap::new(),
            registry_leaves: Vec::new(),
            registry_sources: Vec::new(),
            compile_environment: BTreeMap::new(),
            vocab_auxiliary_targets: Vec::new(),
            supporting_artifacts: Vec::new(),
        };

        let result = manifest.materialize(root.path(), &receipt.intent);
        assert!(matches!(result, Err(OvenRustcError::InvalidArtifactPath { .. })));
        Ok(())
    }

    #[test]
    fn trusted_artifact_plan_uses_declared_inputs_without_rescanning_search_children()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let dependencies = root.path().join("deps");
        fs::create_dir(&dependencies)?;
        let declared = dependencies.join("libdeclared.rlib");
        let unrelated = dependencies.join("unrelated-source-file.rs");
        fs::write(&declared, b"declared artifact")?;
        fs::write(&unrelated, b"not a direct rustc input")?;
        let receipt = intent(root.path())?;
        let manifest = OvenRustcArtifactManifest {
            schema_version: OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION,
            intent: receipt.intent.clone(),
            dependency_search_paths: vec!["deps".to_string()],
            native_search_paths: Vec::new(),
            externs: Vec::new(),
            entrypoint_externs: BTreeMap::new(),
            registry_leaves: Vec::new(),
            registry_sources: Vec::new(),
            compile_environment: BTreeMap::new(),
            vocab_auxiliary_targets: Vec::new(),
            supporting_artifacts: vec![OvenRustcSupportingArtifact {
                relative_path: "deps/libdeclared.rlib".to_string(),
                digest: digest_bytes(b"declared artifact"),
            }],
        };

        let trusted = manifest.materialize_trusted_store(root.path(), &receipt.intent)?;
        assert_eq!(trusted.dependency_search_paths, vec![fs::canonicalize(&dependencies)?]);
        assert!(matches!(
            manifest.materialize(root.path(), &receipt.intent),
            Err(OvenRustcError::UnrecordedSearchArtifact { .. })
        ));
        Ok(())
    }

    #[test]
    fn plan_selection_reuses_one_build_unit_across_distinct_generated_sources() -> Result<(), Box<dyn std::error::Error>>
    {
        let first = tempfile::tempdir()?;
        let second = tempfile::tempdir()?;
        for (root, source) in [
            (first.path(), "fn main() { println!(\"first\"); }\n"),
            (second.path(), "fn main() { println!(\"second\"); }\n"),
        ] {
            fs::create_dir_all(root.join("src"))?;
            fs::write(root.join("src/main.rs"), source)?;
        }
        let receipt_for = |root: &Path| {
            receipt_generated_project(
                &OvenGeneratedProjectRequest::new(
                    root,
                    "shared-oven-fixture",
                    "0.1.0",
                    "aarch64-apple-darwin",
                    "rustc 1.96.0",
                    "debug",
                    Vec::new(),
                )
                .with_generated_source("generated-root", root.join("src/main.rs"))
                .with_generated_source_tree("generated-tree", root.join("src"))
                .with_build_unit_input("runtime-lock", "sha256:shared"),
            )
        };
        let first_receipt = receipt_for(first.path())?;
        let second_receipt = receipt_for(second.path())?;
        assert_ne!(first_receipt.identity, second_receipt.identity);
        assert_eq!(first_receipt.build_unit_identity, second_receipt.build_unit_identity);

        let store_root = tempfile::tempdir()?;
        let store = OvenStore::new(
            store_root.path(),
            OvenStoreLimits::new(128 * 1024, 128 * 1024, 64 * 1024),
        );
        let stored = store.publish(&OvenArtifactPublishRequest {
            receipt: first_receipt.clone(),
            domain: "shared-alpha".to_string(),
            kind: OvenArtifactKind::DirectRustcPlan,
            payload: serde_json::to_vec(&empty_manifest(&first_receipt))?,
            materialized_files: Vec::new(),
        })?;
        assert_eq!(
            select_direct_rustc_plan_identity(&store, &second_receipt)?,
            stored.identity
        );
        Ok(())
    }

    #[test]
    fn project_inspection_authority_reuses_a_compatible_direct_plan_across_receipts()
    -> Result<(), Box<dyn std::error::Error>> {
        let first = tempfile::tempdir()?;
        let second = tempfile::tempdir()?;
        for (root, source) in [
            (first.path(), "fn main() { println!(\"first\"); }\n"),
            (second.path(), "fn main() { println!(\"second\"); }\n"),
        ] {
            fs::create_dir_all(root.join("src"))?;
            fs::write(root.join("src/main.rs"), source)?;
        }
        let receipt_for = |root: &Path| {
            receipt_generated_project(
                &OvenGeneratedProjectRequest::new(
                    root,
                    "shared-oven-fixture",
                    "0.1.0",
                    "aarch64-apple-darwin",
                    "rustc 1.96.0",
                    "debug",
                    Vec::new(),
                )
                .with_generated_source("generated-root", root.join("src/main.rs"))
                .with_generated_source_tree("generated-tree", root.join("src"))
                .with_build_unit_input("runtime-lock", "sha256:shared"),
            )
        };
        let first_receipt = receipt_for(first.path())?;
        let second_receipt = receipt_for(second.path())?;
        assert_ne!(first_receipt.identity, second_receipt.identity);
        assert_eq!(first_receipt.build_unit_identity, second_receipt.build_unit_identity);

        let store_root = tempfile::tempdir()?;
        let store = OvenStore::new(
            store_root.path(),
            OvenStoreLimits::new(128 * 1024, 128 * 1024, 64 * 1024),
        );
        let direct_plan = store.publish(&OvenArtifactPublishRequest {
            receipt: first_receipt,
            domain: "shared-alpha".to_string(),
            kind: OvenArtifactKind::DirectRustcPlan,
            payload: serde_json::to_vec(&empty_manifest(&second_receipt))?,
            materialized_files: Vec::new(),
        })?;
        let project_identity = "sha256:project";
        let source_authority_digest = "sha256:source";
        let compiler_version = "0.5.1-test";
        let authority_payload = OvenProjectInspectionAuthorityPayload {
            schema_version: OVEN_PROJECT_INSPECTION_AUTHORITY_SCHEMA_VERSION,
            project_identity: project_identity.to_string(),
            source_authority_digest: source_authority_digest.to_string(),
            compiler_version: compiler_version.to_string(),
            registry_lock_digest: digest_bytes(b"registry lock"),
            registry_source_dependencies: Vec::new(),
            dev_registry_source_dependencies: Vec::new(),
            test_dependency_envelope: None,
            constituents: vec![OvenProjectInspectionConstituent::Stored {
                identity: direct_plan.identity.clone(),
                artifact_kind: OvenArtifactKind::DirectRustcPlan,
                receipt: second_receipt.clone(),
                base_loaf_identity: None,
            }],
            registry_sources: Vec::new(),
        };
        let authority = store.publish(&OvenArtifactPublishRequest {
            receipt: second_receipt.clone(),
            domain: "shared-alpha".to_string(),
            kind: OvenArtifactKind::ProjectInspectionAuthority,
            payload: serde_json::to_vec(&authority_payload)?,
            materialized_files: Vec::new(),
        })?;

        let (selected_manifest, _, _, _) = store.select_payload_for_execution(&direct_plan.identity)?;
        assert!(project_inspection_constituent_matches_receipt(
            &selected_manifest,
            OvenArtifactKind::DirectRustcPlan,
            &second_receipt,
        ));
        assert!(!project_inspection_constituent_matches_receipt(
            &selected_manifest,
            OvenArtifactKind::ProjectPayload,
            &second_receipt,
        ));
        let mut selected = store.select_payloads_for_execution(std::slice::from_ref(&direct_plan.identity))?;
        let selected = selected
            .pop()
            .ok_or("direct plan disappeared after authority selection")?;
        let selection =
            crate::cli::commands::build::project_test_dependency_plan_from_constituent(selected, &second_receipt)?;
        assert!(matches!(
            selection,
            crate::cli::commands::build::OvenDirectRustcPlanSelection::Stored(_)
        ));

        let loaded = load_project_inspection_authority(
            &store,
            &OvenProjectInspectionAuthorityRef {
                identity: authority.identity.clone(),
                receipt_identity: second_receipt.identity.clone(),
                build_unit_identity: second_receipt.build_unit_identity.clone(),
            },
            project_identity,
            source_authority_digest,
            compiler_version,
        )?;
        assert_eq!(loaded.identity(), authority.identity);
        assert_eq!(
            loaded.artifact_root(),
            store.select(&authority.identity)?.0.materialized_root()
        );
        assert_eq!(loaded.payload, authority_payload);
        Ok(())
    }

    #[test]
    fn project_inspection_authority_rejects_unknown_schema_before_decoding_its_body()
    -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let store_root = tempfile::tempdir()?;
        let materialized = tempfile::tempdir()?;
        let receipt = intent(project.path())?;
        let evidence = materialized.path().join("future-evidence.txt");
        fs::write(&evidence, b"future evidence")?;
        let store = OvenStore::new(
            store_root.path(),
            OvenStoreLimits::new(128 * 1024, 128 * 1024, 64 * 1024),
        );
        let future_schema = OVEN_PROJECT_INSPECTION_AUTHORITY_SCHEMA_VERSION + 1;
        let authority = store.publish(&OvenArtifactPublishRequest {
            receipt: receipt.clone(),
            domain: "future-project-inspection-authority".to_string(),
            kind: OvenArtifactKind::ProjectInspectionAuthority,
            payload: serde_json::to_vec(&serde_json::json!({
                "schema_version": future_schema,
                "generated_out_dirs": [{"crate_name": 42}],
            }))?,
            materialized_files: vec![OvenArtifactMaterializedFile {
                source_path: evidence,
                relative_path: "authority/future-evidence.txt".to_string(),
            }],
        })?;
        let (entry, lease) = store.select(&authority.identity)?;
        drop(lease);
        let stored_evidence = entry.materialized_root().join("authority/future-evidence.txt");
        fs::remove_file(&stored_evidence)?;
        fs::write(stored_evidence, b"corrupt evidence")?;

        let result = load_project_inspection_authority(
            &store,
            &OvenProjectInspectionAuthorityRef {
                identity: authority.identity,
                receipt_identity: receipt.identity,
                build_unit_identity: receipt.build_unit_identity,
            },
            "unused-project-identity",
            "unused-source-authority",
            "unused-compiler-version",
        );
        assert!(matches!(
            result,
            Err(OvenRustcError::UnsupportedProjectInspectionAuthoritySchema {
                found,
                expected: OVEN_PROJECT_INSPECTION_AUTHORITY_SCHEMA_VERSION,
            }) if found == future_schema
        ));
        Ok(())
    }

    #[test]
    fn project_inspection_authority_binds_the_requested_store_identity() -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let store_root = tempfile::tempdir()?;
        let receipt = intent(project.path())?;
        let store = OvenStore::new(
            store_root.path(),
            OvenStoreLimits::new(128 * 1024, 128 * 1024, 64 * 1024),
        );
        let project_identity = "sha256:project";
        let source_authority_digest = "sha256:source";
        let compiler_version = "0.5.1-test";
        let payload = OvenProjectInspectionAuthorityPayload {
            schema_version: OVEN_PROJECT_INSPECTION_AUTHORITY_SCHEMA_VERSION,
            project_identity: project_identity.to_string(),
            source_authority_digest: source_authority_digest.to_string(),
            compiler_version: compiler_version.to_string(),
            registry_lock_digest: digest_bytes(b"registry lock"),
            registry_source_dependencies: Vec::new(),
            dev_registry_source_dependencies: Vec::new(),
            test_dependency_envelope: None,
            constituents: Vec::new(),
            registry_sources: Vec::new(),
        };
        let encoded_payload = serde_json::to_vec(&payload)?;
        let publish = |domain: &str| {
            store.publish(&OvenArtifactPublishRequest {
                receipt: receipt.clone(),
                domain: domain.to_string(),
                kind: OvenArtifactKind::ProjectInspectionAuthority,
                payload: encoded_payload.clone(),
                materialized_files: Vec::new(),
            })
        };
        let requested = publish("requested-project-inspection-authority")?;
        let substitute = publish("substitute-project-inspection-authority")?;
        assert_ne!(requested.identity, substitute.identity);
        let (requested_entry, requested_lease) = store.select(&requested.identity)?;
        let (substitute_entry, substitute_lease) = store.select(&substitute.identity)?;
        drop(requested_lease);
        drop(substitute_lease);
        fs::remove_dir_all(&requested_entry.path)?;
        fs::rename(&substitute_entry.path, &requested_entry.path)?;

        let result = load_project_inspection_authority(
            &store,
            &OvenProjectInspectionAuthorityRef {
                identity: requested.identity,
                receipt_identity: receipt.identity,
                build_unit_identity: receipt.build_unit_identity,
            },
            project_identity,
            source_authority_digest,
            compiler_version,
        );
        assert!(matches!(
            result,
            Err(OvenRustcError::InvalidStoredPlan { identity, .. }) if identity == substitute.identity
        ));
        Ok(())
    }

    #[test]
    fn loaded_project_inspection_authority_retains_and_revalidates_its_store_owner()
    -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let store_root = tempfile::tempdir()?;
        let materialized = tempfile::tempdir()?;
        let receipt = intent(project.path())?;
        let evidence = materialized.path().join("authority-evidence.txt");
        fs::write(&evidence, b"authority evidence")?;
        let store = OvenStore::new(
            store_root.path(),
            OvenStoreLimits::new(128 * 1024, 128 * 1024, 64 * 1024),
        );
        let project_identity = "sha256:project";
        let source_authority_digest = "sha256:source";
        let compiler_version = "0.5.1-test";
        let payload = OvenProjectInspectionAuthorityPayload {
            schema_version: OVEN_PROJECT_INSPECTION_AUTHORITY_SCHEMA_VERSION,
            project_identity: project_identity.to_string(),
            source_authority_digest: source_authority_digest.to_string(),
            compiler_version: compiler_version.to_string(),
            registry_lock_digest: digest_bytes(b"registry lock"),
            registry_source_dependencies: Vec::new(),
            dev_registry_source_dependencies: Vec::new(),
            test_dependency_envelope: None,
            constituents: Vec::new(),
            registry_sources: Vec::new(),
        };
        let authority = store.publish(&OvenArtifactPublishRequest {
            receipt: receipt.clone(),
            domain: "retained-project-inspection-authority".to_string(),
            kind: OvenArtifactKind::ProjectInspectionAuthority,
            payload: serde_json::to_vec(&payload)?,
            materialized_files: vec![OvenArtifactMaterializedFile {
                source_path: evidence,
                relative_path: "authority/evidence.txt".to_string(),
            }],
        })?;
        let authority_ref = OvenProjectInspectionAuthorityRef {
            identity: authority.identity.clone(),
            receipt_identity: receipt.identity.clone(),
            build_unit_identity: receipt.build_unit_identity.clone(),
        };

        let loaded = load_project_inspection_authority(
            &store,
            &authority_ref,
            project_identity,
            source_authority_digest,
            compiler_version,
        )?;
        assert_eq!(loaded.identity(), authority.identity);
        assert!(loaded.artifact_root().join("authority/evidence.txt").is_file());
        assert!(store.inspect()?.active_lease_physical_bytes > 0);
        drop(loaded);
        assert_eq!(store.inspect()?.active_lease_physical_bytes, 0);

        let (entry, lease) = store.select(&authority.identity)?;
        drop(lease);
        let stored_evidence = entry.materialized_root().join("authority/evidence.txt");
        fs::remove_file(&stored_evidence)?;
        fs::write(stored_evidence, b"authority evidencf")?;
        let result = load_project_inspection_authority(
            &store,
            &authority_ref,
            project_identity,
            source_authority_digest,
            compiler_version,
        );
        assert!(matches!(result, Err(OvenRustcError::Store(_))));
        Ok(())
    }

    #[test]
    fn plan_selection_skips_a_legacy_manifest_schema_before_materializing_a_replacement()
    -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let store_root = tempfile::tempdir()?;
        write_project(project.path())?;
        let receipt = intent(project.path())?;
        let mut legacy = empty_manifest(&receipt);
        legacy.schema_version = OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION - 1;
        let current = empty_manifest(&receipt);
        let store = OvenStore::new(
            store_root.path(),
            OvenStoreLimits::new(128 * 1024, 128 * 1024, 64 * 1024),
        );
        store.publish(&OvenArtifactPublishRequest {
            receipt: receipt.clone(),
            domain: "legacy-alpha".to_string(),
            kind: OvenArtifactKind::DirectRustcPlan,
            payload: serde_json::to_vec(&legacy)?,
            materialized_files: Vec::new(),
        })?;
        let replacement = store.publish(&OvenArtifactPublishRequest {
            receipt: receipt.clone(),
            domain: "current-alpha".to_string(),
            kind: OvenArtifactKind::DirectRustcPlan,
            payload: serde_json::to_vec(&current)?,
            materialized_files: Vec::new(),
        })?;

        assert_eq!(
            select_direct_rustc_plan_identity(&store, &receipt)?,
            replacement.identity
        );
        Ok(())
    }

    #[test]
    fn direct_rustc_test_runs_without_cargo_in_the_consumer_environment() -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let output = tempfile::tempdir()?;
        write_project(project.path())?;
        let source = output.path().join("consumer.rs");
        fs::write(
            &source,
            "#[test]\nfn cargo_is_not_visible_to_the_consumer() { assert!(option_env!(\"CARGO\").is_none()); assert!(option_env!(\"CARGO_PKG_NAME\").is_none()); }\n",
        )?;
        let rustc = rustc_path()?;
        let receipt = receipt_generated_project(
            &OvenGeneratedProjectRequest::new(
                project.path(),
                "rustc_fixture",
                "0.1.0",
                rustc_host_target(&rustc)?,
                rustc_identity(&rustc)?,
                "release",
                Vec::new(),
            )
            .with_generated_source("direct-rustc-source", &source),
        )?;
        let artifact_root = tempfile::tempdir()?;
        let request = OvenDirectRustcTestRequest {
            receipt: receipt.clone(),
            artifacts: OvenRustcArtifactManifest {
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
            artifact_root: artifact_root.path().to_path_buf(),
            rustc,
            source,
            output: output.path().join("consumer-test"),
            crate_name: "oven_consumer".to_string(),
            edition: "2024".to_string(),
            source_evidence_key: "direct-rustc-source".to_string(),
        };

        let bake = bake_direct_rustc_test(&request)?;
        assert!(!bake.reused);
        assert!(Command::new(&bake.output).status()?.success());
        let reused = bake_direct_rustc_test(&request)?;
        assert!(reused.reused);
        assert_eq!(reused.output, bake.output);
        Ok(())
    }

    #[test]
    fn trusted_direct_rustc_runs_a_proc_macro_libtest_with_its_selected_dynamic_toolchain()
    -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let output = tempfile::tempdir()?;
        let artifact_root = tempfile::tempdir()?;
        write_project(project.path())?;
        let source = output.path().join("proc-macro-test.rs");
        fs::write(
            &source,
            "extern crate proc_macro;\nuse proc_macro::TokenStream;\n#[proc_macro]\npub fn passthrough(input: TokenStream) -> TokenStream { input }\n#[test]\nfn direct_proc_macro_test() {}\n",
        )?;
        let rustc = rustc_path()?;
        let receipt = receipt_generated_project(
            &OvenGeneratedProjectRequest::new(
                project.path(),
                "rustc_fixture",
                "0.1.0",
                rustc_host_target(&rustc)?,
                rustc_identity(&rustc)?,
                "release",
                Vec::new(),
            )
            .with_generated_source("proc-macro-source", &source),
        )?;
        let bake = bake_trusted_direct_rustc_test(&OvenTrustedDirectRustcTargetRequest {
            receipt: &receipt,
            artifacts: &empty_manifest(&receipt),
            artifact_root: artifact_root.path(),
            artifact_plan: None,
            rustc: &rustc,
            source: &source,
            output: &output.path().join("proc-macro-test"),
            crate_name: "oven_proc_macro_test",
            edition: "2024",
            source_evidence_key: "proc-macro-source",
            features: &[],
            prefer_dynamic: true,
        })?;
        #[cfg(unix)]
        {
            let output = Command::new("/bin/sh")
                .env_clear()
                .env("PATH", "/usr/bin:/bin")
                .args(["-c", "exec \"$1\" --list --format terse", "sh"])
                .arg(&bake.output)
                .output()?;
            assert!(
                output.status.success(),
                "a dynamic direct-Rustc test must survive a shell hop without ambient loader state: {}",
                combined_process_output(&output.stdout, &output.stderr)
            );
        }
        let (name, value) = rustc_dynamic_library_environment(&rustc)?;
        let report = run_native_test_batch_all(&bake.output, &BTreeMap::from([(name, value)]))?;
        assert!(report.success);
        assert_eq!(report.inventory.names, ["direct_proc_macro_test"]);
        Ok(())
    }

    /// A real native library rebuilds changed output or invalid evidence and preserves an unchanged warm result.
    #[test]
    fn trusted_direct_rustc_materializes_a_reusable_library_without_cargo() -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let output = tempfile::tempdir()?;
        let artifact_root = tempfile::tempdir()?;
        write_project(project.path())?;
        fs::create_dir_all(project.path().join("src"))?;
        let source = project.path().join("src/materialized.rs");
        fs::write(&source, "pub fn oven_materialized() -> u32 { 42 }\n")?;
        let rustc = rustc_path()?;
        let receipt = receipt_generated_project(
            &OvenGeneratedProjectRequest::new(
                project.path(),
                "rustc_fixture",
                "0.1.0",
                rustc_host_target(&rustc)?,
                rustc_identity(&rustc)?,
                "release",
                Vec::new(),
            )
            .with_generated_source("materialized-library", &source),
        )?;
        let request = OvenTrustedDirectRustcTargetRequest {
            receipt: &receipt,
            artifacts: &empty_manifest(&receipt),
            artifact_root: artifact_root.path(),
            artifact_plan: None,
            rustc: &rustc,
            source: &source,
            output: &output.path().join("liboven_materialized.rlib"),
            crate_name: "oven_materialized",
            edition: "2024",
            source_evidence_key: "materialized-library",
            features: &[],
            prefer_dynamic: false,
        };

        let bake = bake_trusted_direct_rustc_library(&request)?;
        assert!(!bake.reused);
        assert!(bake.output.is_file());
        let sidecar = super::caller_output_receipt_path(&bake.output)?;
        let original_output = fs::read(&bake.output)?;
        let original_sidecar = fs::read(&sidecar)?;
        let output_modified = fs::metadata(&bake.output)?.modified()?;
        let sidecar_modified = fs::metadata(&sidecar)?.modified()?;
        let reused = bake_trusted_direct_rustc_library(&request)?;
        assert!(reused.reused);
        assert_eq!(reused.output, bake.output);
        assert_eq!(reused.output_digest, bake.output_digest);
        assert_eq!(fs::read(&bake.output)?, original_output);
        assert_eq!(fs::read(&sidecar)?, original_sidecar);
        assert_eq!(fs::metadata(&bake.output)?.modified()?, output_modified);
        assert_eq!(fs::metadata(&sidecar)?.modified()?, sidecar_modified);

        // A still-valid input sidecar cannot authorize replacement output bytes.
        fs::write(&bake.output, b"changed output bytes")?;
        let rebuilt = bake_trusted_direct_rustc_library(&request)?;
        assert!(!rebuilt.reused);
        assert_eq!(rebuilt.output_digest, bake.output_digest);
        assert_eq!(rebuilt.output_digest, digest_bytes(&fs::read(&rebuilt.output)?));
        let record: serde_json::Value = serde_json::from_slice(&fs::read(&sidecar)?)?;
        assert_eq!(record["output_digest"], rebuilt.output_digest);
        assert_eq!(
            record["schema_version"],
            super::OVEN_DIRECT_RUSTC_OUTPUT_RECEIPT_SCHEMA_VERSION
        );

        let mut missing_digest = record.clone();
        missing_digest
            .as_object_mut()
            .ok_or("output sidecar must be an object")?
            .remove("output_digest");
        let mut legacy = missing_digest.clone();
        legacy["schema_version"] = serde_json::json!(2);
        let mut malformed_digest = record.clone();
        malformed_digest["output_digest"] = serde_json::json!(42);
        let mut wrong_digest = record.clone();
        wrong_digest["output_digest"] = serde_json::json!(digest_bytes(b"another output"));
        let mut wrong_inputs = record.clone();
        wrong_inputs["receipt_identity"] = serde_json::json!(digest_bytes(b"another receipt"));
        for (case, invalid_record) in [
            ("legacy sidecar", legacy),
            ("missing digest", missing_digest),
            ("malformed digest", malformed_digest),
            ("wrong digest", wrong_digest),
            ("wrong input receipt", wrong_inputs),
        ] {
            fs::write(&sidecar, serde_json::to_vec(&invalid_record)?)?;
            let rebuilt = bake_trusted_direct_rustc_library(&request)?;
            assert!(!rebuilt.reused, "{case} must rebuild");
            assert_eq!(rebuilt.output_digest, bake.output_digest, "{case}");
            let warm = bake_trusted_direct_rustc_library(&request)?;
            assert!(warm.reused, "{case} must publish reusable repaired evidence");
            assert_eq!(warm.output_digest, rebuilt.output_digest, "{case}");
        }
        fs::write(&sidecar, b"{interrupted sidecar")?;
        assert!(!bake_trusted_direct_rustc_library(&request)?.reused);
        Ok(())
    }

    #[test]
    fn transitive_caller_owned_library_keeps_its_search_path_and_reuse_evidence_private()
    -> Result<(), Box<dyn std::error::Error>> {
        let output = tempfile::tempdir()?;
        let transitive = output.path().join("libprovider_child.rlib");
        fs::write(&transitive, "receipt-bound child")?;
        let transitive_digest = digest_bytes(b"receipt-bound child");
        let mut plan = OvenRustcArtifactPlan {
            dependency_search_paths: Vec::new(),
            native_search_paths: Vec::new(),
            externs: Vec::new(),
            compile_environment: BTreeMap::new(),
            caller_owned_library_digests: BTreeMap::new(),
        };

        attach_caller_owned_rustc_libraries(
            &mut plan,
            &[OvenCallerOwnedRustcLibrary {
                crate_name: "provider_child".to_string(),
                output: transitive.clone(),
                digest: transitive_digest.clone(),
                expose_extern: false,
            }],
        )?;

        assert!(plan.externs.is_empty());
        assert_eq!(plan.dependency_search_paths, vec![output.path().to_path_buf()]);
        assert_eq!(plan.caller_owned_library_digests.len(), 1);
        assert_eq!(
            plan.caller_owned_library_digests
                .get(&format!("transitive:provider_child:{transitive_digest}")),
            Some(&transitive_digest),
        );
        assert!(
            plan.caller_owned_library_digests
                .keys()
                .all(|key| key.starts_with("transitive:provider_child:"))
        );
        Ok(())
    }

    #[test]
    fn trusted_direct_rustc_links_an_oven_materialized_library_without_cargo() -> Result<(), Box<dyn std::error::Error>>
    {
        let project = tempfile::tempdir()?;
        let output = tempfile::tempdir()?;
        let artifact_root = tempfile::tempdir()?;
        write_project(project.path())?;
        fs::create_dir_all(project.path().join("src"))?;
        let first_library_source = project.path().join("src/materialized_one.rs");
        let second_library_source = project.path().join("src/materialized_two.rs");
        let binary_source = project.path().join("src/consumer.rs");
        fs::write(&first_library_source, "pub fn answer() -> u32 { 42 }\n")?;
        fs::write(&second_library_source, "pub fn answer() -> u32 { 43 }\n")?;
        fs::write(
            &binary_source,
            "fn main() { println!(\"{}\", oven_materialized::answer()); }\n",
        )?;
        let rustc = rustc_path()?;
        let receipt = receipt_generated_project(
            &OvenGeneratedProjectRequest::new(
                project.path(),
                "rustc_fixture",
                "0.1.0",
                rustc_host_target(&rustc)?,
                rustc_identity(&rustc)?,
                "release",
                Vec::new(),
            )
            .with_generated_source("materialized-library-one", &first_library_source)
            .with_generated_source("materialized-library-two", &second_library_source)
            .with_generated_source("materialized-consumer", &binary_source),
        )?;
        let empty_artifacts = empty_manifest(&receipt);
        let library = bake_trusted_direct_rustc_library(&OvenTrustedDirectRustcTargetRequest {
            receipt: &receipt,
            artifacts: &empty_artifacts,
            artifact_root: artifact_root.path(),
            artifact_plan: None,
            rustc: &rustc,
            source: &first_library_source,
            output: &output.path().join("liboven_materialized_one.rlib"),
            crate_name: "oven_materialized",
            edition: "2024",
            source_evidence_key: "materialized-library-one",
            features: &[],
            prefer_dynamic: false,
        })?;
        let mut consumer_plan = OvenRustcArtifactPlan {
            dependency_search_paths: Vec::new(),
            native_search_paths: Vec::new(),
            externs: Vec::new(),
            compile_environment: BTreeMap::new(),
            caller_owned_library_digests: BTreeMap::new(),
        };
        attach_caller_owned_rustc_libraries(
            &mut consumer_plan,
            &[OvenCallerOwnedRustcLibrary {
                crate_name: "oven_materialized".to_string(),
                output: library.output.clone(),
                digest: library.output_digest.clone(),
                expose_extern: true,
            }],
        )?;
        assert_eq!(
            consumer_plan.caller_owned_library_digests.get("oven_materialized"),
            Some(&library.output_digest),
        );
        let consumer = bake_trusted_direct_rustc_run(&OvenTrustedDirectRustcTargetRequest {
            receipt: &receipt,
            artifacts: &empty_artifacts,
            artifact_root: artifact_root.path(),
            artifact_plan: Some(&consumer_plan),
            rustc: &rustc,
            source: &binary_source,
            output: &output.path().join("oven-consumer"),
            crate_name: "oven_consumer",
            edition: "2024",
            source_evidence_key: "materialized-consumer",
            features: &[],
            prefer_dynamic: false,
        })?;
        let first_run = Command::new(&consumer.output).output()?;
        assert!(first_run.status.success());
        assert_eq!(String::from_utf8(first_run.stdout)?.trim(), "42");

        let replacement_library = bake_trusted_direct_rustc_library(&OvenTrustedDirectRustcTargetRequest {
            receipt: &receipt,
            artifacts: &empty_artifacts,
            artifact_root: artifact_root.path(),
            artifact_plan: None,
            rustc: &rustc,
            source: &second_library_source,
            output: &output.path().join("liboven_materialized_two.rlib"),
            crate_name: "oven_materialized",
            edition: "2024",
            source_evidence_key: "materialized-library-two",
            features: &[],
            prefer_dynamic: false,
        })?;
        let mut replacement_plan = OvenRustcArtifactPlan {
            dependency_search_paths: Vec::new(),
            native_search_paths: Vec::new(),
            externs: Vec::new(),
            compile_environment: BTreeMap::new(),
            caller_owned_library_digests: BTreeMap::new(),
        };
        attach_caller_owned_rustc_libraries(
            &mut replacement_plan,
            &[OvenCallerOwnedRustcLibrary {
                crate_name: "oven_materialized".to_string(),
                output: replacement_library.output.clone(),
                digest: replacement_library.output_digest.clone(),
                expose_extern: true,
            }],
        )?;
        let rebuilt_consumer = bake_trusted_direct_rustc_run(&OvenTrustedDirectRustcTargetRequest {
            receipt: &receipt,
            artifacts: &empty_artifacts,
            artifact_root: artifact_root.path(),
            artifact_plan: Some(&replacement_plan),
            rustc: &rustc,
            source: &binary_source,
            output: &output.path().join("oven-consumer"),
            crate_name: "oven_consumer",
            edition: "2024",
            source_evidence_key: "materialized-consumer",
            features: &[],
            prefer_dynamic: false,
        })?;
        assert!(!rebuilt_consumer.reused);
        let second_run = Command::new(&rebuilt_consumer.output).output()?;
        assert!(second_run.status.success());
        assert_eq!(String::from_utf8(second_run.stdout)?.trim(), "43");
        Ok(())
    }

    #[test]
    fn trusted_direct_rustc_links_an_oven_materialized_dylib_without_cargo() -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let output = tempfile::tempdir()?;
        let artifact_root = tempfile::tempdir()?;
        write_project(project.path())?;
        fs::create_dir_all(project.path().join("src"))?;
        let library_source = project.path().join("src/materialized.rs");
        let binary_source = project.path().join("src/consumer.rs");
        fs::write(&library_source, "pub fn answer() -> u32 { 42 }\n")?;
        fs::write(
            &binary_source,
            "fn main() { println!(\"{}\", oven_materialized::answer()); }\n",
        )?;
        let rustc = rustc_path()?;
        let receipt = receipt_generated_project(
            &OvenGeneratedProjectRequest::new(
                project.path(),
                "rustc_fixture",
                "0.1.0",
                rustc_host_target(&rustc)?,
                rustc_identity(&rustc)?,
                "release",
                Vec::new(),
            )
            .with_generated_source("materialized-dylib", &library_source)
            .with_generated_source("dylib-consumer", &binary_source),
        )?;
        let empty_artifacts = empty_manifest(&receipt);
        let dylib = bake_trusted_direct_rustc_dylib(&OvenTrustedDirectRustcTargetRequest {
            receipt: &receipt,
            artifacts: &empty_artifacts,
            artifact_root: artifact_root.path(),
            artifact_plan: None,
            rustc: &rustc,
            source: &library_source,
            output: &output
                .path()
                .join(format!("liboven_materialized{}", std::env::consts::DLL_SUFFIX)),
            crate_name: "oven_materialized",
            edition: "2024",
            source_evidence_key: "materialized-dylib",
            features: &[],
            prefer_dynamic: true,
        })?;
        let mut consumer_plan = OvenRustcArtifactPlan {
            dependency_search_paths: Vec::new(),
            native_search_paths: Vec::new(),
            externs: Vec::new(),
            compile_environment: BTreeMap::new(),
            caller_owned_library_digests: BTreeMap::new(),
        };
        attach_caller_owned_rustc_libraries(
            &mut consumer_plan,
            &[OvenCallerOwnedRustcLibrary {
                crate_name: "oven_materialized".to_string(),
                output: dylib.output.clone(),
                digest: dylib.output_digest.clone(),
                expose_extern: true,
            }],
        )?;
        let consumer = bake_trusted_direct_rustc_run(&OvenTrustedDirectRustcTargetRequest {
            receipt: &receipt,
            artifacts: &empty_artifacts,
            artifact_root: artifact_root.path(),
            artifact_plan: Some(&consumer_plan),
            rustc: &rustc,
            source: &binary_source,
            output: &output.path().join("oven-dylib-consumer"),
            crate_name: "oven_dylib_consumer",
            edition: "2024",
            source_evidence_key: "dylib-consumer",
            features: &[],
            prefer_dynamic: true,
        })?;
        let (name, toolchain_libraries) = rustc_dynamic_library_environment(&rustc)?;
        let dylib_directory = dylib.output.parent().ok_or("dylib parent missing")?;
        let search_path = std::env::join_paths(
            std::iter::once(dylib_directory.to_path_buf()).chain(std::env::split_paths(&toolchain_libraries)),
        )?;
        let result = Command::new(&consumer.output).env(name, search_path).output()?;
        assert!(result.status.success());
        assert_eq!(String::from_utf8(result.stdout)?.trim(), "42");
        Ok(())
    }

    #[test]
    fn trusted_direct_rustc_links_an_oven_materialized_proc_macro_without_cargo()
    -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let output = tempfile::tempdir()?;
        let artifact_root = tempfile::tempdir()?;
        write_project(project.path())?;
        fs::create_dir_all(project.path().join("src"))?;
        let macro_source = project.path().join("src/oven_macros.rs");
        let consumer_source = project.path().join("src/consumer.rs");
        fs::write(
            &macro_source,
            "use proc_macro::{Literal, TokenStream, TokenTree};\n#[proc_macro]\npub fn answer(_input: TokenStream) -> TokenStream { TokenStream::from(TokenTree::Literal(Literal::u32_unsuffixed(43))) }\n",
        )?;
        fs::write(
            &consumer_source,
            "use oven_macros::answer;\nfn main() { println!(\"{}\", answer!()); }\n",
        )?;
        let rustc = rustc_path()?;
        let receipt = receipt_generated_project(
            &OvenGeneratedProjectRequest::new(
                project.path(),
                "rustc_fixture",
                "0.1.0",
                rustc_host_target(&rustc)?,
                rustc_identity(&rustc)?,
                "release",
                Vec::new(),
            )
            .with_generated_source("materialized-proc-macro", &macro_source)
            .with_generated_source("proc-macro-consumer", &consumer_source),
        )?;
        let empty_artifacts = empty_manifest(&receipt);
        let proc_macro = bake_trusted_direct_rustc_proc_macro(&OvenTrustedDirectRustcTargetRequest {
            receipt: &receipt,
            artifacts: &empty_artifacts,
            artifact_root: artifact_root.path(),
            artifact_plan: None,
            rustc: &rustc,
            source: &macro_source,
            output: &output
                .path()
                .join(format!("liboven_macros{}", std::env::consts::DLL_SUFFIX)),
            crate_name: "oven_macros",
            edition: "2024",
            source_evidence_key: "materialized-proc-macro",
            features: &[],
            prefer_dynamic: false,
        })?;
        let mut consumer_plan = OvenRustcArtifactPlan {
            dependency_search_paths: Vec::new(),
            native_search_paths: Vec::new(),
            externs: Vec::new(),
            compile_environment: BTreeMap::new(),
            caller_owned_library_digests: BTreeMap::new(),
        };
        attach_caller_owned_rustc_libraries(
            &mut consumer_plan,
            &[OvenCallerOwnedRustcLibrary {
                crate_name: "oven_macros".to_string(),
                output: proc_macro.output.clone(),
                digest: proc_macro.output_digest.clone(),
                expose_extern: true,
            }],
        )?;
        let consumer = bake_trusted_direct_rustc_run(&OvenTrustedDirectRustcTargetRequest {
            receipt: &receipt,
            artifacts: &empty_artifacts,
            artifact_root: artifact_root.path(),
            artifact_plan: Some(&consumer_plan),
            rustc: &rustc,
            source: &consumer_source,
            output: &output.path().join("oven-proc-macro-consumer"),
            crate_name: "oven_proc_macro_consumer",
            edition: "2024",
            source_evidence_key: "proc-macro-consumer",
            features: &[],
            prefer_dynamic: false,
        })?;
        let output = Command::new(&consumer.output).output()?;
        assert!(output.status.success());
        assert_eq!(String::from_utf8(output.stdout)?.trim(), "43");
        Ok(())
    }

    #[test]
    fn trusted_rustdoc_runs_a_receipt_bound_doctest_without_cargo() -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let output = tempfile::tempdir()?;
        let artifact_root = tempfile::tempdir()?;
        write_project(project.path())?;
        fs::create_dir_all(project.path().join("src"))?;
        let source = project.path().join("src/doctest.rs");
        fs::write(
            &source,
            "//! ```\n//! assert!(std::env::var_os(\"CARGO\").is_none());\n//! ```\npub struct DoctestFixture;\n",
        )?;
        let rustc = rustc_path()?;
        let receipt = receipt_generated_project(
            &OvenGeneratedProjectRequest::new(
                project.path(),
                "rustc_fixture",
                "0.1.0",
                rustc_host_target(&rustc)?,
                rustc_identity(&rustc)?,
                "release",
                Vec::new(),
            )
            .with_generated_source("doctest-source", &source),
        )?;
        let mut artifacts = empty_manifest(&receipt);
        artifacts
            .compile_environment
            .insert("CARGO_MANIFEST_DIR".to_string(), "@oven-source-ancestor:2".to_string());
        let report = run_trusted_rustdoc_test(&OvenTrustedRustdocTestRequest {
            receipt: &receipt,
            artifacts: &artifacts,
            artifact_root: artifact_root.path(),
            artifact_plan: None,
            rustc: &rustc,
            source: &source,
            temporary_directory: &output.path().join("rustdoc-temporary"),
            crate_name: "oven_doctest_fixture",
            edition: "2024",
            source_evidence_key: "doctest-source",
            features: &[],
            is_proc_macro: false,
            prefer_dynamic: false,
            timeout: None,
        })?;
        assert!(report.output.contains("test result: ok"));
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn trusted_rustdoc_timeout_terminates_a_stalled_doctest_descendant() -> Result<(), Box<dyn std::error::Error>> {
        use std::os::unix::fs::PermissionsExt;
        use std::time::{Duration, Instant};

        let project = tempfile::tempdir()?;
        let output = tempfile::tempdir()?;
        let artifact_root = tempfile::tempdir()?;
        write_project(project.path())?;
        fs::create_dir_all(project.path().join("src"))?;
        let source = project.path().join("src/stalled_doctest.rs");
        fs::write(
            &source,
            "//! ```\n//! assert!(true);\n//! ```\npub struct StalledDoctest;\n",
        )?;

        let sysroot = output.path().join("sysroot");
        let rustc = output.path().join("rustc");
        let rustdoc = sysroot.join("bin/rustdoc");
        let descendant_started = output.path().join("descendant-started");
        fs::create_dir_all(rustdoc.parent().ok_or("Rustdoc parent missing")?)?;
        fs::write(
            &rustc,
            format!(
                "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then\n  printf '%s\\n' 'rustc oven-timeout-fixture'\n  exit 0\nfi\nif [ \"$1\" = \"--print\" ] && [ \"$2\" = \"sysroot\" ]; then\n  printf '%s\\n' \"{}\"\n  exit 0\nfi\nexit 97\n",
                sysroot.display(),
            ),
        )?;
        fs::write(
            &rustdoc,
            format!(
                "#!/bin/sh\nsleep 30 &\nprintf '%s\\n' \"$!\" > \"{}\"\nwait\n",
                descendant_started.display(),
            ),
        )?;
        for executable in [&rustc, &rustdoc] {
            let mut permissions = fs::metadata(executable)?.permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(executable, permissions)?;
        }
        let receipt = receipt_generated_project(
            &OvenGeneratedProjectRequest::new(
                project.path(),
                "rustc_fixture",
                "0.1.0",
                "fixture-target",
                "rustc oven-timeout-fixture",
                "release",
                Vec::new(),
            )
            .with_generated_source("stalled-doctest", &source),
        )?;

        let started = Instant::now();
        let Err(error) = run_trusted_rustdoc_test(&OvenTrustedRustdocTestRequest {
            receipt: &receipt,
            artifacts: &empty_manifest(&receipt),
            artifact_root: artifact_root.path(),
            artifact_plan: None,
            rustc: &rustc,
            source: &source,
            temporary_directory: &output.path().join("rustdoc-temporary"),
            crate_name: "stalled_doctest",
            edition: "2024",
            source_evidence_key: "stalled-doctest",
            features: &[],
            is_proc_macro: false,
            prefer_dynamic: false,
            timeout: Some(Duration::from_secs(10)),
        }) else {
            return Err("stalled Rustdoc unexpectedly completed within its receipt-bound root deadline".into());
        };
        assert!(descendant_started.is_file(), "fake Rustdoc descendant was not started");
        assert!(matches!(error, OvenRustcError::RustdocTestFailed { .. }));
        assert!(error.to_string().contains("timed out after 10000ms"), "{error}");
        assert!(error.to_string().contains(&source.display().to_string()), "{error}");
        assert!(
            started.elapsed() < Duration::from_secs(25),
            "stalled doctest descendant outlived the Rustdoc deadline"
        );
        Ok(())
    }

    #[test]
    fn trusted_rustdoc_uses_a_composed_plan_without_materializing_a_thin_artifact_root()
    -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let output = tempfile::tempdir()?;
        let thin_artifact_root = tempfile::tempdir()?;
        write_project(project.path())?;
        fs::create_dir_all(project.path().join("src"))?;
        let source = project.path().join("src/composed_doctest.rs");
        fs::write(
            &source,
            "//! ```\n//! assert_eq!(2 + 2, 4);\n//! ```\npub struct DoctestFixture;\n",
        )?;
        let rustc = rustc_path()?;
        let receipt = receipt_generated_project(
            &OvenGeneratedProjectRequest::new(
                project.path(),
                "rustc_fixture",
                "0.1.0",
                rustc_host_target(&rustc)?,
                rustc_identity(&rustc)?,
                "release",
                Vec::new(),
            )
            .with_generated_source("composed-doctest-source", &source),
        )?;
        let mut thin_artifacts = empty_manifest(&receipt);
        thin_artifacts.supporting_artifacts.push(OvenRustcSupportingArtifact {
            relative_path: "missing-foundation/libfixture.rlib".to_string(),
            digest: "sha256:fixture".to_string(),
        });
        let composed_plan = OvenRustcArtifactPlan {
            dependency_search_paths: Vec::new(),
            native_search_paths: Vec::new(),
            externs: Vec::new(),
            compile_environment: BTreeMap::new(),
            caller_owned_library_digests: BTreeMap::new(),
        };

        let report = run_trusted_rustdoc_test(&OvenTrustedRustdocTestRequest {
            receipt: &receipt,
            artifacts: &thin_artifacts,
            artifact_root: thin_artifact_root.path(),
            artifact_plan: Some(&composed_plan),
            rustc: &rustc,
            source: &source,
            temporary_directory: &output.path().join("rustdoc-temporary"),
            crate_name: "oven_composed_doctest_fixture",
            edition: "2024",
            source_evidence_key: "composed-doctest-source",
            features: &[],
            is_proc_macro: false,
            prefer_dynamic: false,
            timeout: None,
        })?;
        assert!(report.output.contains("test result: ok"));
        Ok(())
    }

    #[test]
    fn trusted_rustdoc_runs_with_a_caller_owned_dynamic_library_without_cargo() -> Result<(), Box<dyn std::error::Error>>
    {
        let project = tempfile::tempdir()?;
        let output = tempfile::tempdir()?;
        let artifact_root = tempfile::tempdir()?;
        write_project(project.path())?;
        fs::create_dir_all(project.path().join("src"))?;
        let library_source = project.path().join("src/doctest_dylib.rs");
        let source = project.path().join("src/dynamic_doctest.rs");
        fs::write(&library_source, "pub fn answer() -> u32 { 42 }\n")?;
        fs::write(
            &source,
            "//! ```\n//! assert_eq!(oven_doctest_dylib::answer(), 42);\n//! ```\npub struct DynamicDoctestFixture;\n",
        )?;
        let rustc = rustc_path()?;
        let receipt = receipt_generated_project(
            &OvenGeneratedProjectRequest::new(
                project.path(),
                "rustc_fixture",
                "0.1.0",
                rustc_host_target(&rustc)?,
                rustc_identity(&rustc)?,
                "release",
                Vec::new(),
            )
            .with_generated_source("dynamic-doctest-library", &library_source)
            .with_generated_source("dynamic-doctest-source", &source),
        )?;
        let artifacts = empty_manifest(&receipt);
        let dylib = bake_trusted_direct_rustc_dylib(&OvenTrustedDirectRustcTargetRequest {
            receipt: &receipt,
            artifacts: &artifacts,
            artifact_root: artifact_root.path(),
            artifact_plan: None,
            rustc: &rustc,
            source: &library_source,
            output: &output
                .path()
                .join(format!("liboven_doctest_dylib{}", std::env::consts::DLL_SUFFIX)),
            crate_name: "oven_doctest_dylib",
            edition: "2024",
            source_evidence_key: "dynamic-doctest-library",
            features: &[],
            prefer_dynamic: true,
        })?;
        let mut plan = OvenRustcArtifactPlan {
            dependency_search_paths: Vec::new(),
            native_search_paths: Vec::new(),
            externs: Vec::new(),
            compile_environment: BTreeMap::new(),
            caller_owned_library_digests: BTreeMap::new(),
        };
        attach_caller_owned_rustc_libraries(
            &mut plan,
            &[OvenCallerOwnedRustcLibrary {
                crate_name: "oven_doctest_dylib".to_string(),
                output: dylib.output,
                digest: dylib.output_digest,
                expose_extern: true,
            }],
        )?;
        let sealed_dependency_directory = output.path().join("sealed/sha256:immutable-doctest-dependency");
        fs::create_dir_all(&sealed_dependency_directory)?;
        plan.dependency_search_paths.push(sealed_dependency_directory);

        let report = run_trusted_rustdoc_test(&OvenTrustedRustdocTestRequest {
            receipt: &receipt,
            artifacts: &artifacts,
            artifact_root: artifact_root.path(),
            artifact_plan: Some(&plan),
            rustc: &rustc,
            source: &source,
            temporary_directory: &output.path().join("rustdoc-temporary"),
            crate_name: "oven_dynamic_doctest",
            edition: "2024",
            source_evidence_key: "dynamic-doctest-source",
            features: &[],
            is_proc_macro: false,
            prefer_dynamic: true,
            timeout: None,
        })?;
        assert!(report.output.contains("test result: ok"));
        Ok(())
    }

    #[test]
    fn trusted_rustdoc_compiles_a_proc_macro_doctest_root() -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let output = tempfile::tempdir()?;
        let artifact_root = tempfile::tempdir()?;
        write_project(project.path())?;
        fs::create_dir_all(project.path().join("src"))?;
        let source = project.path().join("src/proc_macro_doctest.rs");
        fs::write(
            &source,
            "use proc_macro::TokenStream;\n\n#[proc_macro_derive(OvenFixture)]\npub fn oven_fixture(_input: TokenStream) -> TokenStream { TokenStream::new() }\n",
        )?;
        let rustc = rustc_path()?;
        let receipt = receipt_generated_project(
            &OvenGeneratedProjectRequest::new(
                project.path(),
                "rustc_fixture",
                "0.1.0",
                rustc_host_target(&rustc)?,
                rustc_identity(&rustc)?,
                "release",
                Vec::new(),
            )
            .with_generated_source("proc-macro-doctest-source", &source),
        )?;
        let report = run_trusted_rustdoc_test(&OvenTrustedRustdocTestRequest {
            receipt: &receipt,
            artifacts: &empty_manifest(&receipt),
            artifact_root: artifact_root.path(),
            artifact_plan: None,
            rustc: &rustc,
            source: &source,
            temporary_directory: &output.path().join("rustdoc-temporary"),
            crate_name: "oven_proc_macro_doctest",
            edition: "2024",
            source_evidence_key: "proc-macro-doctest-source",
            features: &[],
            is_proc_macro: true,
            prefer_dynamic: true,
            timeout: None,
        })?;
        assert!(report.output.contains("test result: ok"));
        Ok(())
    }

    #[test]
    fn stored_receipt_bound_plan_runs_without_cargo_and_retains_its_lease() -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let output = tempfile::tempdir()?;
        let store_root = tempfile::tempdir()?;
        write_project(project.path())?;
        let source = output.path().join("stored-consumer.rs");
        fs::write(
            &source,
            "#[test]\nfn oven_plan_is_the_consumer_input() { assert!(option_env!(\"CARGO\").is_none()); }\n",
        )?;
        let rustc = rustc_path()?;
        let receipt = receipt_generated_project(
            &OvenGeneratedProjectRequest::new(
                project.path(),
                "rustc_fixture",
                "0.1.0",
                rustc_host_target(&rustc)?,
                rustc_identity(&rustc)?,
                "release",
                Vec::new(),
            )
            .with_generated_source("direct-rustc-source", &source),
        )?;
        let plan = empty_manifest(&receipt);
        let payload = serde_json::to_vec(&plan)?;
        let store = OvenStore::new(
            store_root.path(),
            OvenStoreLimits::new(128 * 1024, 128 * 1024, 64 * 1024),
        );
        let stored = store.publish(&OvenArtifactPublishRequest {
            receipt: receipt.clone(),
            domain: "alpha-test".to_string(),
            kind: OvenArtifactKind::DirectRustcPlan,
            payload,
            materialized_files: Vec::new(),
        })?;
        assert_eq!(select_direct_rustc_plan_identity(&store, &receipt)?, stored.identity);

        let request = OvenStoredDirectRustcTestRequest {
            store: &store,
            plan_identity: stored.identity.clone(),
            receipt: receipt.clone(),
            rustc: rustc.clone(),
            source: source.clone(),
            output: output.path().join("stored-consumer-test"),
            crate_name: "oven_stored_consumer".to_string(),
            edition: "2024".to_string(),
            source_evidence_key: "direct-rustc-source".to_string(),
        };
        let bake = bake_stored_direct_rustc_test(&request)?;

        assert!(!bake.reused);
        let reused = bake_stored_direct_rustc_test(&request)?;
        assert!(reused.reused);
        assert_eq!(reused.output, bake.output);
        drop(reused);
        let first_physical = store.inspect()?.physical_bytes;
        let bounded = OvenStore::new(
            store_root.path(),
            OvenStoreLimits::new(first_physical.saturating_add(1), 128 * 1024, 64 * 1024),
        );
        let replacement = OvenArtifactPublishRequest {
            receipt,
            domain: "alpha-replacement".to_string(),
            kind: OvenArtifactKind::DirectRustcPlan,
            payload: serde_json::to_vec(&plan)?,
            materialized_files: Vec::new(),
        };
        assert!(matches!(
            bounded.publish(&replacement),
            Err(crate::oven::store::OvenStoreError::CapacityBlocked { .. })
        ));
        assert!(Command::new(&bake.output).status()?.success());
        drop(bake);
        bounded.publish(&replacement)?;
        assert_eq!(bounded.inspect()?.entries.len(), 1);
        Ok(())
    }

    #[test]
    fn receipt_selection_rejects_ambiguous_stored_direct_rustc_plans() -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let store_root = tempfile::tempdir()?;
        write_project(project.path())?;
        let receipt = intent(project.path())?;
        let plan = empty_manifest(&receipt);
        let store = OvenStore::new(
            store_root.path(),
            OvenStoreLimits::new(128 * 1024, 128 * 1024, 64 * 1024),
        );
        for domain in ["alpha-primary", "alpha-duplicate"] {
            store.publish(&OvenArtifactPublishRequest {
                receipt: receipt.clone(),
                domain: domain.to_string(),
                kind: OvenArtifactKind::DirectRustcPlan,
                payload: serde_json::to_vec(&plan)?,
                materialized_files: Vec::new(),
            })?;
        }

        assert!(matches!(
            select_direct_rustc_plan_identity(&store, &receipt),
            Err(OvenRustcError::PlanSelection { .. })
        ));
        Ok(())
    }

    #[test]
    fn stored_receipt_bound_binary_runs_without_cargo_and_retains_its_lease() -> Result<(), Box<dyn std::error::Error>>
    {
        let project = tempfile::tempdir()?;
        let output = tempfile::tempdir()?;
        let store_root = tempfile::tempdir()?;
        write_project(project.path())?;
        let source = output.path().join("stored-binary.rs");
        fs::write(
            &source,
            "fn main() { assert!(option_env!(\"CARGO\").is_none()); assert!(option_env!(\"CARGO_PKG_NAME\").is_none()); }\n",
        )?;
        let rustc = rustc_path()?;
        let receipt = receipt_generated_project(
            &OvenGeneratedProjectRequest::new(
                project.path(),
                "rustc_fixture",
                "0.1.0",
                rustc_host_target(&rustc)?,
                rustc_identity(&rustc)?,
                "release",
                Vec::new(),
            )
            .with_generated_source("direct-rustc-source", &source),
        )?;
        let plan = empty_manifest(&receipt);
        let store = OvenStore::new(
            store_root.path(),
            OvenStoreLimits::new(128 * 1024, 128 * 1024, 64 * 1024),
        );
        let stored = store.publish(&OvenArtifactPublishRequest {
            receipt: receipt.clone(),
            domain: "alpha-run".to_string(),
            kind: OvenArtifactKind::DirectRustcPlan,
            payload: serde_json::to_vec(&plan)?,
            materialized_files: Vec::new(),
        })?;

        let bake = bake_stored_direct_rustc_run(&OvenStoredDirectRustcRunRequest {
            store: &store,
            plan_identity: stored.identity.clone(),
            receipt: receipt.clone(),
            rustc,
            source,
            output: output.path().join("stored-binary"),
            crate_name: "oven_stored_binary".to_string(),
            edition: "2024".to_string(),
            source_evidence_key: "direct-rustc-source".to_string(),
        })?;

        let first_physical = store.inspect()?.physical_bytes;
        let bounded = OvenStore::new(
            store_root.path(),
            OvenStoreLimits::new(first_physical.saturating_add(1), 128 * 1024, 64 * 1024),
        );
        let replacement = OvenArtifactPublishRequest {
            receipt,
            domain: "alpha-run-replacement".to_string(),
            kind: OvenArtifactKind::DirectRustcPlan,
            payload: serde_json::to_vec(&plan)?,
            materialized_files: Vec::new(),
        };
        assert!(matches!(
            bounded.publish(&replacement),
            Err(crate::oven::store::OvenStoreError::CapacityBlocked { .. })
        ));
        assert!(Command::new(&bake.output).status()?.success());
        drop(bake);
        bounded.publish(&replacement)?;
        assert_eq!(bounded.inspect()?.entries.len(), 1);
        Ok(())
    }

    #[test]
    fn direct_rustc_refuses_source_changes_before_invoking_the_compiler() -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let output = tempfile::tempdir()?;
        let artifact_root = tempfile::tempdir()?;
        write_project(project.path())?;
        let source = output.path().join("consumer.rs");
        fs::write(&source, "#[test]\nfn first() {}\n")?;
        let rustc = rustc_path()?;
        let receipt = receipt_generated_project(
            &OvenGeneratedProjectRequest::new(
                project.path(),
                "rustc_fixture",
                "0.1.0",
                rustc_host_target(&rustc)?,
                rustc_identity(&rustc)?,
                "release",
                Vec::new(),
            )
            .with_generated_source("direct-rustc-source", &source),
        )?;
        fs::write(&source, "#[test]\nfn changed() {}\n")?;
        let request = OvenDirectRustcTestRequest {
            artifacts: empty_manifest(&receipt),
            receipt,
            artifact_root: artifact_root.path().to_path_buf(),
            rustc,
            source,
            output: output.path().join("consumer-test"),
            crate_name: "oven_consumer".to_string(),
            edition: "2024".to_string(),
            source_evidence_key: "direct-rustc-source".to_string(),
        };
        let result = bake_direct_rustc_test(&request);
        assert!(matches!(result, Err(OvenRustcError::SourceEvidenceMismatch { .. })));
        Ok(())
    }

    #[test]
    fn direct_rustc_refuses_a_compiler_that_disagrees_with_the_receipt() -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let output = tempfile::tempdir()?;
        let artifact_root = tempfile::tempdir()?;
        write_project(project.path())?;
        let source = output.path().join("consumer.rs");
        fs::write(&source, "#[test]\nfn identity_is_checked() {}\n")?;
        let rustc = rustc_path()?;
        let receipt = receipt_generated_project(
            &OvenGeneratedProjectRequest::new(
                project.path(),
                "rustc_fixture",
                "0.1.0",
                rustc_host_target(&rustc)?,
                "rustc deliberately-not-the-selected-compiler",
                "release",
                Vec::new(),
            )
            .with_generated_source("direct-rustc-source", &source),
        )?;
        let request = OvenDirectRustcTestRequest {
            artifacts: empty_manifest(&receipt),
            receipt,
            artifact_root: artifact_root.path().to_path_buf(),
            rustc,
            source,
            output: output.path().join("consumer-test"),
            crate_name: "oven_consumer".to_string(),
            edition: "2024".to_string(),
            source_evidence_key: "direct-rustc-source".to_string(),
        };

        assert!(matches!(
            bake_direct_rustc_test(&request),
            Err(OvenRustcError::ToolchainMismatch { .. })
        ));
        Ok(())
    }

    fn intent(project: &Path) -> Result<crate::oven::OvenReceipt, Box<dyn std::error::Error>> {
        write_project(project)?;
        Ok(receipt_generated_project(
            &OvenGeneratedProjectRequest::new(
                project,
                "rustc_fixture",
                "0.1.0",
                "aarch64-apple-darwin",
                "rustc 1.96.0",
                "release",
                Vec::new(),
            )
            .with_generated_source("fixture-source", project.join("fixture.rs")),
        )?)
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

    #[test]
    fn release_only_project_inspection_authority_binds_root_features_and_orders_constituents()
    -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let receipt = intent(project.path())?;
        let source = OvenRustcRegistrySourcePackage {
            package: "serde_json".to_string(),
            version: "1.0.140".to_string(),
            features: vec!["preserve_order".to_string(), "std".to_string()],
            source: OvenRustcRegistrySource {
                registry: "registry+https://github.com/rust-lang/crates.io-index".to_string(),
                checksum: "serde-json-checksum".to_string(),
                relative_root: "registry-sources/serde_json-1.0.140".to_string(),
                digest: digest_bytes(b"serde_json source"),
            },
        };
        let root = OvenProjectInspectionRootDependency {
            alias: "serde_json".to_string(),
            package: source.package.clone(),
            version: source.version.clone(),
            registry: source.source.registry.clone(),
            checksum: source.source.checksum.clone(),
            requested_features: vec!["preserve_order".to_string()],
            default_features: false,
        };
        let mut payload = OvenProjectInspectionAuthorityPayload {
            schema_version: OVEN_PROJECT_INSPECTION_AUTHORITY_SCHEMA_VERSION,
            project_identity: "sha256:project".to_string(),
            source_authority_digest: "sha256:source".to_string(),
            compiler_version: "0.5.0-rc0".to_string(),
            registry_lock_digest: digest_bytes(b"lock"),
            registry_source_dependencies: vec![root.clone()],
            dev_registry_source_dependencies: Vec::new(),
            test_dependency_envelope: None,
            constituents: vec![OvenProjectInspectionConstituent::ReleaseLoaf {
                loaf_identity: "sha256:release-loaf".to_string(),
                build_unit_identity: "sha256:release-unit".to_string(),
                receipt: receipt.clone(),
            }],
            registry_sources: vec![OvenProjectInspectionSource {
                package: source,
                owner: OvenProjectInspectionSourceOwner::Constituent { index: 0 },
            }],
        };
        validate_project_inspection_authority_payload(&payload)?;

        let matching = DependencySpec {
            crate_name: "serde_json".to_string(),
            version: Some("1".to_string()),
            features: vec!["preserve_order".to_string()],
            default_features: false,
            source: DependencySource::Registry,
            optional: false,
            package: None,
        };
        assert!(project_inspection_authority_supports_dependencies(
            &payload,
            std::slice::from_ref(&matching)
        ));
        let mut wrong_features = matching.clone();
        wrong_features.features = vec!["raw_value".to_string()];
        assert!(!project_inspection_authority_supports_dependencies(
            &payload,
            &[wrong_features]
        ));
        let mut wrong_defaults = matching.clone();
        wrong_defaults.default_features = true;
        assert!(!project_inspection_authority_supports_dependencies(
            &payload,
            &[wrong_defaults]
        ));

        let debug_receipt = receipt_generated_project(
            &OvenGeneratedProjectRequest::new(
                project.path(),
                "rustc_fixture",
                "0.1.0",
                "aarch64-apple-darwin",
                "rustc 1.96.0",
                "debug",
                Vec::new(),
            )
            .with_generated_source("fixture-source", project.path().join("fixture.rs")),
        )?;
        payload.constituents.push(OvenProjectInspectionConstituent::Stored {
            identity: "sha256:test-dependency-extension".to_string(),
            artifact_kind: OvenArtifactKind::ProjectPayload,
            receipt: debug_receipt.clone(),
            base_loaf_identity: Some("sha256:release-loaf".to_string()),
        });
        payload.test_dependency_envelope = Some(OvenProjectInspectionTestDependencyEnvelope {
            constituent_index: 1,
            dependency_surface_digest: digest_bytes(b"normal+dev dependency surface"),
            dependency_roots: BTreeMap::from([(
                "serde_json".to_string(),
                OvenProjectInspectionTestDependencyRoot::Registry {
                    dependency_digest: crate::oven::digest_dependency_specs(std::slice::from_ref(&matching))?,
                    locked: root,
                },
            )]),
        });
        validate_project_inspection_authority_payload(&payload)?;
        assert!(project_inspection_test_dependency_envelope_supports_dependencies(
            &payload,
            std::slice::from_ref(&matching)
        )?);
        let mut missing = matching.clone();
        missing.crate_name = "missing_alias".to_string();
        assert!(!project_inspection_test_dependency_envelope_supports_dependencies(
            &payload,
            &[missing]
        )?);

        let mut direct_plan_payload = payload.clone();
        direct_plan_payload.constituents[1] = OvenProjectInspectionConstituent::Stored {
            identity: "sha256:test-dependency-direct-plan".to_string(),
            artifact_kind: OvenArtifactKind::DirectRustcPlan,
            receipt: debug_receipt.clone(),
            base_loaf_identity: None,
        };
        validate_project_inspection_authority_payload(&direct_plan_payload)?;
        if let OvenProjectInspectionConstituent::Stored { base_loaf_identity, .. } =
            &mut direct_plan_payload.constituents[1]
        {
            *base_loaf_identity = Some("sha256:invalid-direct-plan-base".to_string());
        }
        let Err(error) = validate_project_inspection_authority_payload(&direct_plan_payload) else {
            return Err("authority accepted base-Loaf evidence on a self-contained direct-plan constituent".into());
        };
        assert!(
            error
                .to_string()
                .contains("inconsistent identity, kind, or base evidence")
        );

        let path_dependency = DependencySpec {
            crate_name: "dev_fixture".to_string(),
            version: Some("0.1.0".to_string()),
            features: vec!["test-support".to_string()],
            default_features: false,
            source: DependencySource::Path {
                path: PathBuf::from("legacy-path-dependency"),
            },
            optional: false,
            package: None,
        };
        payload
            .test_dependency_envelope
            .as_mut()
            .ok_or("test dependency role disappeared")?
            .dependency_roots
            .insert(
                "dev_fixture".to_string(),
                OvenProjectInspectionTestDependencyRoot::Path {
                    dependency_digest: "sha256:legacy-path-envelope".to_string(),
                },
            );
        let Err(error) = project_inspection_test_dependency_envelope_supports_dependencies(
            &payload,
            std::slice::from_ref(&path_dependency),
        ) else {
            return Err(
                "persisted Cargo path envelope authorized execution without checked Oven source evidence".into(),
            );
        };
        assert!(error.to_string().contains("has no checked Oven source-unit identity"));

        payload
            .test_dependency_envelope
            .as_mut()
            .ok_or("test dependency role disappeared")?
            .constituent_index = 0;
        let Err(error) = validate_project_inspection_authority_payload(&payload) else {
            return Err("authority accepted a non-debug release Loaf as its project test dependency envelope".into());
        };
        assert!(error.to_string().contains("debug-profile"));
        if let OvenProjectInspectionConstituent::ReleaseLoaf { receipt, .. } = &mut payload.constituents[0] {
            *receipt = debug_receipt;
        }
        validate_project_inspection_authority_payload(&payload)?;
        payload.test_dependency_envelope = None;
        let _ = payload.constituents.pop();

        payload.constituents.insert(
            0,
            OvenProjectInspectionConstituent::Stored {
                identity: "sha256:direct-plan".to_string(),
                artifact_kind: OvenArtifactKind::DirectRustcPlan,
                receipt,
                base_loaf_identity: None,
            },
        );
        let Err(error) = validate_project_inspection_authority_payload(&payload) else {
            return Err("authority accepted a release Loaf after a store-owned constituent".into());
        };
        assert!(error.to_string().contains("precede"));
        Ok(())
    }

    #[test]
    fn trusted_plan_respects_entrypoint_externs_without_dropping_caller_libraries()
    -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let receipt = intent(project.path())?;
        let mut artifacts = empty_manifest(&receipt);
        artifacts.dependency_search_paths = vec!["runtime-deps".to_string(), "vocab-deps".to_string()];
        artifacts.externs = vec![
            OvenRustcArtifactExtern {
                crate_name: "runtime".to_string(),
                relative_path: "runtime-deps/libruntime.rlib".to_string(),
                digest: digest_bytes(b"runtime"),
            },
            OvenRustcArtifactExtern {
                crate_name: "vocab_helper".to_string(),
                relative_path: "vocab-deps/libvocab_helper.rlib".to_string(),
                digest: digest_bytes(b"vocab helper"),
            },
        ];
        artifacts
            .entrypoint_externs
            .insert("generated-root".to_string(), vec!["runtime".to_string()]);
        let selected_artifacts = artifacts.for_source_evidence("generated-root")?;
        assert_eq!(selected_artifacts.dependency_search_paths, vec!["runtime-deps"]);
        let plan = OvenRustcArtifactPlan {
            dependency_search_paths: vec![
                PathBuf::from("/immutable/runtime-deps"),
                PathBuf::from("/immutable/vocab-deps"),
                PathBuf::from("/caller"),
            ],
            native_search_paths: Vec::new(),
            externs: vec![
                (
                    "runtime".to_string(),
                    PathBuf::from("/immutable/runtime-deps/libruntime.rlib"),
                ),
                (
                    "vocab_helper".to_string(),
                    PathBuf::from("/immutable/vocab-deps/libvocab_helper.rlib"),
                ),
                (
                    "caller_library".to_string(),
                    PathBuf::from("/caller/libcaller_library.rlib"),
                ),
            ],
            compile_environment: BTreeMap::new(),
            caller_owned_library_digests: BTreeMap::from([("caller_library".to_string(), digest_bytes(b"caller"))]),
        };

        let projected = super::trusted_artifact_plan_for_source(&plan, &artifacts, &selected_artifacts);

        assert_eq!(
            projected
                .externs
                .iter()
                .map(|(crate_name, _)| crate_name.as_str())
                .collect::<Vec<_>>(),
            vec!["runtime", "caller_library"]
        );
        assert_eq!(
            projected.dependency_search_paths,
            vec![PathBuf::from("/immutable/runtime-deps"), PathBuf::from("/caller")]
        );
        assert_eq!(
            projected.caller_owned_library_digests,
            plan.caller_owned_library_digests
        );
        Ok(())
    }

    #[test]
    fn source_projection_allows_a_caller_registry_leaf_to_replace_a_private_compiler_helper()
    -> Result<(), Box<dyn std::error::Error>> {
        let project = tempfile::tempdir()?;
        let caller = tempfile::tempdir()?;
        let receipt = intent(project.path())?;
        let caller_serde_json = caller.path().join("libserde_json.rlib");
        fs::write(&caller_serde_json, "caller serde_json")?;

        let mut artifacts = empty_manifest(&receipt);
        artifacts.dependency_search_paths = vec!["runtime-deps".to_string(), "compiler-private".to_string()];
        artifacts.externs = vec![
            OvenRustcArtifactExtern {
                crate_name: "runtime".to_string(),
                relative_path: "runtime-deps/libruntime.rlib".to_string(),
                digest: digest_bytes(b"runtime"),
            },
            OvenRustcArtifactExtern {
                crate_name: "serde_json".to_string(),
                relative_path: "compiler-private/libserde_json.rlib".to_string(),
                digest: digest_bytes(b"compiler serde_json"),
            },
        ];
        artifacts
            .entrypoint_externs
            .insert("generated-root".to_string(), vec!["runtime".to_string()]);
        let plan = OvenRustcArtifactPlan {
            dependency_search_paths: vec![
                PathBuf::from("/immutable/runtime-deps"),
                PathBuf::from("/immutable/compiler-private"),
            ],
            native_search_paths: Vec::new(),
            externs: vec![
                (
                    "runtime".to_string(),
                    PathBuf::from("/immutable/runtime-deps/libruntime.rlib"),
                ),
                (
                    "serde_json".to_string(),
                    PathBuf::from("/immutable/compiler-private/libserde_json.rlib"),
                ),
            ],
            compile_environment: BTreeMap::new(),
            caller_owned_library_digests: BTreeMap::new(),
        };

        let mut projected = super::trusted_artifact_plan_for_source_evidence(&plan, &artifacts, "generated-root")?;
        attach_caller_owned_rustc_libraries(
            &mut projected,
            &[OvenCallerOwnedRustcLibrary {
                crate_name: "serde_json".to_string(),
                output: caller_serde_json.clone(),
                digest: digest_bytes(&fs::read(&caller_serde_json)?),
                expose_extern: true,
            }],
        )?;
        // The final bake projects a trusted plan once more. That second projection must distinguish the caller's
        // exact output from the compiler-private serde_json artifact it replaces.
        let projected = super::trusted_artifact_plan_for_source_evidence(&projected, &artifacts, "generated-root")?;

        assert_eq!(
            projected
                .externs
                .iter()
                .map(|(crate_name, path)| (crate_name.as_str(), path.clone()))
                .collect::<Vec<_>>(),
            vec![
                ("runtime", PathBuf::from("/immutable/runtime-deps/libruntime.rlib")),
                ("serde_json", caller_serde_json),
            ]
        );
        assert!(
            !projected
                .dependency_search_paths
                .contains(&PathBuf::from("/immutable/compiler-private"))
        );
        Ok(())
    }

    fn rustc_path() -> Result<PathBuf, Box<dyn std::error::Error>> {
        let output = Command::new("rustup").args(["which", "rustc"]).output()?;
        if !output.status.success() {
            return Err("rustup could not locate rustc".into());
        }
        let path = String::from_utf8(output.stdout)?;
        let path = PathBuf::from(path.trim());
        if !path.is_file() {
            return Err(format!("rustup returned a non-file rustc path: {}", path.display()).into());
        }
        Ok(path)
    }

    fn rustc_identity(rustc: &Path) -> Result<String, Box<dyn std::error::Error>> {
        let output = Command::new(rustc).arg("--version").output()?;
        if !output.status.success() {
            return Err(format!("rustc could not report its version: {}", rustc.display()).into());
        }
        Ok(String::from_utf8(output.stdout)?.trim().to_string())
    }

    fn write_project(root: &Path) -> Result<(), std::io::Error> {
        fs::write(root.join("fixture.rs"), "pub fn fixture() {}\n")
    }
}
