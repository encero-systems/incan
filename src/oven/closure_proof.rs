//! A written-down proof that one sealed artifact closure was materialized in full.
//!
//! Every normal command that selects the standard-library Loaf walks its closure — nine and a half thousand files —
//! and checks each one for shape: present, a regular file, not a symlink, below the artifact root. It never rehashes
//! them; the publisher did that once, and the Loaf's identity is the digest of the manifest that names every file. So
//! the shape check re-derives, per process, a fact that is a pure function of a name that is already a digest (#1546).
//! This module writes that fact down once, beside the envelope, so the next process reads one small file instead of
//! stating ten thousand.
//!
//! What a proof does and does not carry. It says: a process materialized this exact closure (by identity and file
//! count) with every check passing. It lets a later process skip the per-file shape checks. It cannot make a missing
//! file usable — `rustc` reports that on its own — and it says nothing about content, which the trusted path never
//! rehashed anyway. `inspect oven` remains the full audit and never consults a proof.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Wire format of one closure proof.
pub(crate) const OVEN_CLOSURE_PROOF_SCHEMA_VERSION: u32 = 1;

/// Directory below a Loaf envelope root (or an Oven store root) that holds closure proofs by identity.
pub(crate) const CLOSURE_PROOF_DIRECTORY: &str = "closure-proofs";

/// The fact one full materialization established, keyed by the closure it was established for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct OvenClosureProof {
    /// Schema of this record.
    pub schema_version: u32,
    /// Identity of the closure — the digest of the manifest naming every file — that was materialized in full.
    pub closure_identity: String,
    /// Number of declared files the materialization checked, so a manifest that later declares more or fewer files
    /// under the same identity (impossible for a sealed Loaf, cheap to refuse) misses.
    pub artifact_count: u64,
}

impl OvenClosureProof {
    /// Where the proof for `closure_identity` lives below `proof_root`.
    pub(crate) fn path(proof_root: &Path, closure_identity: &str) -> PathBuf {
        proof_root
            .join(CLOSURE_PROOF_DIRECTORY)
            .join(format!("{}.json", closure_identity.replace(':', "-")))
    }

    /// Read the proof at `path` and accept it only when it is for exactly this closure.
    ///
    /// Anything else — no file, unreadable, another schema, another identity, another count — is `None`: the caller
    /// then materializes in full, which produces the honest error if something is actually wrong.
    pub(crate) fn read_matching(path: &Path, closure_identity: &str, artifact_count: u64) -> Option<Self> {
        let bytes = fs::read(path).ok()?;
        let proof = serde_json::from_slice::<Self>(&bytes).ok()?;
        (proof.schema_version == OVEN_CLOSURE_PROOF_SCHEMA_VERSION
            && proof.closure_identity == closure_identity
            && proof.artifact_count == artifact_count)
            .then_some(proof)
    }

    /// Write the proof atomically: a sibling temporary file renamed into place, so a concurrent reader sees either
    /// no proof or a whole one, and two writers racing on the same proof leave identical content.
    pub(crate) fn write(&self, path: &Path) -> io::Result<()> {
        let parent = path
            .parent()
            .ok_or_else(|| io::Error::other("closure proof path has no parent"))?;
        fs::create_dir_all(parent)?;
        let staged = parent.join(format!(".{}.{}", std::process::id(), "proof.tmp"));
        fs::write(&staged, serde_json::to_vec_pretty(self).map_err(io::Error::other)?)?;
        fs::rename(&staged, path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A proof is served back only to the closure it was written for.
    #[test]
    fn a_proof_is_only_read_back_for_its_own_closure_issue1546() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let path = OvenClosureProof::path(root.path(), "sha256:abc");
        assert!(OvenClosureProof::read_matching(&path, "sha256:abc", 3).is_none());
        let proof = OvenClosureProof {
            schema_version: OVEN_CLOSURE_PROOF_SCHEMA_VERSION,
            closure_identity: "sha256:abc".to_string(),
            artifact_count: 3,
        };
        proof.write(&path)?;
        assert_eq!(OvenClosureProof::read_matching(&path, "sha256:abc", 3), Some(proof));
        assert!(OvenClosureProof::read_matching(&path, "sha256:abc", 4).is_none());
        assert!(OvenClosureProof::read_matching(&path, "sha256:def", 3).is_none());
        fs::write(
            &path,
            b"{\"schema_version\":0,\"closure_identity\":\"sha256:abc\",\"artifact_count\":3}",
        )?;
        assert!(OvenClosureProof::read_matching(&path, "sha256:abc", 3).is_none());
        Ok(())
    }
}
