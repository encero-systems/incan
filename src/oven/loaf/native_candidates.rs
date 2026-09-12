//! Committed-envelope candidate intake for native Loaf selection.
//!
//! Split out of `loaf.rs` rather than annotated in place: that module is already several thousand lines, and this
//! is a self-contained unit whose only reader is the gate that has not landed yet.
//!
//! Intake reads a committed envelope and holds its generation lock. It deliberately does not compare a caller
//! receipt, select a profile, rank capabilities, or read native and source trees — the Incan decision and the
//! host response binding both have to precede materializing any candidate's declared compiler inputs.
#![allow(
    dead_code,
    reason = "Gates 6 and 7 of RFC 119 are the reader; this substrate lands first so their blockers have something to change"
)]

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use super::{
    OVEN_LOAF_SCHEMA_VERSION, OvenLoaf, OvenLoafEnvelope, OvenLoafEnvelopeMember, OvenLoafError,
    OvenLoafGenerationLock, OvenLoafMemberRole, acquire_loaf_generation_lock, committed_loaf_envelope_manifest,
    committed_loaf_metadata_paths_for_authority, validate_registry_leaf_catalog,
};
use crate::oven::digest_bytes;
use crate::oven::rustc::OvenRustcArtifactPlan;

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
