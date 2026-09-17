//! Read-only mirrors of a typed Loaf envelope root, consulted when the local root has no reusable generation.
//!
//! The standard-library Loaf family (`release`, `compiler-suite`) lives in an envelope root: `envelope.json` names
//! one committed generation and binds it to release-family evidence — fixture, lock, runtime-source, compiler and
//! SDK-inventory digests — and each member is a content-addressed `<identity>.loaf/` directory under
//! `generations/<generation>/`. A cold checkout has no generation and bakes one; on this machine that is minutes of
//! Cargo for what another machine already holds byte-identically.
//!
//! A mirror is another envelope root, read and never written. It is used only when its `envelope.json` names the
//! exact generation the local bake would produce — same schema, envelope, evidence, and checked member list — so a
//! family baked for another compiler or SDK contributes nothing. The generation is copied into scratch first and
//! proven there — the eager whole-Loaf audit, the digest of every compiled artifact, and the digest of every registry
//! source tree the Loaf declares — and only a generation that proves in full is committed through the same atomic
//! path a local bake uses. A mirror entry that fails any check leaves nothing behind and the bake proceeds as if the
//! mirror were absent. Ordinary reuse then runs on the committed generation exactly as it would on a local one; the
//! copy adds no trust.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use crate::loaf::{
    OvenLoaf, OvenLoafEnvelopeManifest, OvenLoafEnvelopeMember, OvenLoafMemberRole, OvenReleaseRuntimeClosureMember,
    OvenReleaseRuntimeFoundationMember, OvenReleaseStoreMember, commit_loaf_generation,
    prove_release_runtime_closure_member, prove_release_runtime_foundation_member, prove_release_store_member_payload,
    validate_stored_loaf,
};
use oven_store::digest_source_tree;

/// The exact generation a local bake would commit, used to decide whether a mirror's envelope is the same one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoafEnvelopeExpectation<'a> {
    /// Envelope manifest schema the local baker writes.
    pub schema_version: u32,
    /// Built-in envelope name (`release` or `compiler-suite`).
    pub envelope: &'a str,
    /// Generation identity derived from the envelope name and evidence.
    pub generation_identity: &'a str,
    /// Release-family compatibility evidence the local bake computed.
    pub evidence: &'a BTreeMap<String, String>,
    /// Checked member list, in envelope order.
    pub members: &'a [LoafMemberExpectation],
    /// Optional exact generic store member the local publisher intends to bind into this generation.
    pub release_store_member: Option<&'a OvenReleaseStoreMember>,
    /// Optional exact runtime-foundation carrier the local publisher intends to bind into this generation.
    pub runtime_foundation: Option<&'a OvenReleaseRuntimeFoundationMember>,
    /// Optional exact runtime closure committed beside the foundation.
    pub runtime_closure: Option<&'a OvenReleaseRuntimeClosureMember>,
}

/// The checked specification one envelope member must carry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoafMemberExpectation {
    /// Stable member label from the typed envelope definition.
    pub label: String,
    /// Debug or release profile.
    pub profile: String,
    /// Checked fixture action (`build` or `run`).
    pub action: String,
    /// The authority the member contributes.
    pub role: OvenLoafMemberRole,
}

/// Why no mirror supplied the envelope; every variant means "bake as if no mirror".
#[derive(Debug)]
pub enum LoafMirrorMiss {
    /// No mirror offered the expected generation, or every one that did failed its proof.
    NoCompatibleEnvelope,
    /// A mirror root exists but its `envelope.json` could not be read or decoded.
    Unreadable { mirror: PathBuf, message: String },
}

impl std::fmt::Display for LoafMirrorMiss {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoCompatibleEnvelope => write!(f, "no mirror carries the expected Loaf envelope generation"),
            Self::Unreadable { mirror, message } => {
                write!(f, "Loaf mirror {} is unreadable: {message}", mirror.display())
            }
        }
    }
}

/// One generation admitted from a mirror.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MirroredLoafEnvelope {
    /// The mirror root the generation came from.
    pub mirror: PathBuf,
    /// Members admitted.
    pub member_count: usize,
}

/// Commit the expected generation from the first mirror that proves in full, or report why none did.
///
/// The caller must hold the envelope's exclusive publication lock. `scratch` must be on the same filesystem as
/// `output` so the final rename is atomic; anything staged there is removed before returning.
pub fn import_loaf_envelope_from_mirrors(
    output: &Path,
    scratch: &Path,
    expectation: &LoafEnvelopeExpectation<'_>,
    mirrors: &[PathBuf],
) -> Result<MirroredLoafEnvelope, LoafMirrorMiss> {
    let generation_relative = generation_directory(expectation.generation_identity);
    for mirror in mirrors {
        let Some(manifest) = read_mirror_manifest(mirror)? else {
            continue;
        };
        if !manifest_matches(&manifest, expectation, &generation_relative) {
            continue;
        }
        let staging = scratch.join("loaf-mirror");
        let admitted = stage_and_prove_generation(mirror, &manifest, &staging, &generation_relative)
            .and_then(|staged| commit_staged_generation(output, scratch, &manifest, &staged, &generation_relative));
        let _ = fs::remove_dir_all(&staging);
        if admitted.is_ok() {
            return Ok(MirroredLoafEnvelope {
                mirror: mirror.clone(),
                member_count: manifest.loafs.len(),
            });
        }
    }
    Err(LoafMirrorMiss::NoCompatibleEnvelope)
}

/// Read a mirror's `envelope.json`, distinguishing an absent root from a corrupt one.
fn read_mirror_manifest(mirror: &Path) -> Result<Option<OvenLoafEnvelopeManifest>, LoafMirrorMiss> {
    let manifest_path = mirror.join("envelope.json");
    let bytes = match fs::read(&manifest_path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(LoafMirrorMiss::Unreadable {
                mirror: mirror.to_path_buf(),
                message: error.to_string(),
            });
        }
    };
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|error| LoafMirrorMiss::Unreadable {
            mirror: mirror.to_path_buf(),
            message: format!("invalid envelope.json: {error}"),
        })
}

/// Decide whether a mirror manifest names exactly the generation the local bake would commit.
///
/// Every member path must sit directly under the expected generation directory and end in `loaf.json`; a manifest
/// that points anywhere else is refused before a single byte is copied.
fn manifest_matches(
    manifest: &OvenLoafEnvelopeManifest,
    expectation: &LoafEnvelopeExpectation<'_>,
    generation_relative: &Path,
) -> bool {
    if manifest.schema_version != expectation.schema_version
        || manifest.envelope != expectation.envelope
        || manifest.generation_identity != expectation.generation_identity
        || &manifest.evidence != expectation.evidence
        || manifest.loafs.len() != expectation.members.len()
        || manifest.release_store_member.as_ref() != expectation.release_store_member
        || manifest.runtime_foundation.as_ref() != expectation.runtime_foundation
        || manifest.runtime_closure.as_ref() != expectation.runtime_closure
    {
        return false;
    }
    manifest
        .loafs
        .iter()
        .zip(expectation.members)
        .all(|(member, expected)| {
            member.label == expected.label
                && member.profile == expected.profile
                && member.action == expected.action
                && member.role == expected.role
                && member_path_is_contained(&member.path, generation_relative)
        })
}

/// `generations/<loaf-dir>/loaf.json` beneath the expected generation, with no absolute, parent, or extra component.
fn member_path_is_contained(path: &Path, generation_relative: &Path) -> bool {
    let Ok(below) = path.strip_prefix(generation_relative) else {
        return false;
    };
    let components = below.components().collect::<Vec<_>>();
    matches!(
        components.as_slice(),
        [Component::Normal(directory), Component::Normal(file)]
            if *file == "loaf.json" && directory.to_str().is_some_and(|name| name.ends_with(".loaf"))
    )
}

/// Copy the generation from the mirror into `staging` and prove every member there; return the staged directory.
fn stage_and_prove_generation(
    mirror: &Path,
    manifest: &OvenLoafEnvelopeManifest,
    staging: &Path,
    generation_relative: &Path,
) -> io::Result<PathBuf> {
    let staged_generation = staging.join(generation_relative);
    copy_tree(&mirror.join(generation_relative), &staged_generation)?;
    for member in &manifest.loafs {
        let loaf_root = staging.join(&member.path);
        let loaf_root = loaf_root
            .parent()
            .ok_or_else(|| io::Error::other("member path has no parent"))?;
        if !prove_member(loaf_root, member) {
            return Err(io::Error::other(format!(
                "mirror member `{}` failed its proof",
                member.label
            )));
        }
    }
    if let Some(member) = &manifest.release_store_member {
        prove_release_store_member(&staged_generation, member)?;
    }
    if let Some(member) = &manifest.runtime_foundation {
        prove_release_runtime_foundation_member(staging, manifest, member)
            .map(|_| ())
            .map_err(|error| io::Error::other(error.to_string()))?;
    }
    if let Some(member) = &manifest.runtime_closure {
        prove_release_runtime_closure_member(&staged_generation, manifest, member)
            .map(|_| ())
            .map_err(|error| io::Error::other(error.to_string()))?;
    }
    Ok(staged_generation)
}

/// Prove the copied embedded store entry before its generation becomes the committed envelope authority.
fn prove_release_store_member(generation: &Path, member: &OvenReleaseStoreMember) -> io::Result<()> {
    prove_release_store_member_payload(generation, member)
        .map(|_| ())
        .map_err(|error| io::Error::other(error.to_string()))
}

/// Every check a copied member must pass before it may be committed.
///
/// The stored-Loaf audit is the eager whole-envelope check ordinary reuse skips: content address, declared file set
/// with no undeclared file or symlink, schema, catalog, and payload accounting. Ordinary materialization then rehashes
/// every declared artifact against its recorded digest, and each sealed registry source tree is digested the way its
/// publisher digested it. The identities the envelope member records are compared too, so a member the mirror's
/// manifest describes differently from ours is refused even when its bytes are self-consistent.
fn prove_member(loaf_root: &Path, member: &OvenLoafEnvelopeMember) -> bool {
    let loaf_path = loaf_root.join("loaf.json");
    let Ok(preparation) = validate_stored_loaf(&loaf_path, &member.build_unit_identity) else {
        return false;
    };
    if preparation.loaf_identity != member.loaf_identity || preparation.plan_identity != member.plan_identity {
        return false;
    }
    let Ok(bytes) = fs::read(&loaf_path) else {
        return false;
    };
    let Ok(loaf) = serde_json::from_slice::<OvenLoaf>(&bytes) else {
        return false;
    };
    if loaf.plan.materialize(loaf_root, &loaf.plan.intent).is_err() {
        return false;
    }
    let sources = loaf
        .plan
        .registry_sources
        .iter()
        .map(|package| &package.source)
        .chain(loaf.registry_leaves.iter().map(|leaf| &leaf.source))
        .map(|source| (source.relative_root.as_str(), source.digest.as_str()))
        .collect::<BTreeMap<_, _>>();
    sources.into_iter().all(|(relative_root, digest)| {
        matches!(digest_source_tree(&loaf_root.join(relative_root)), Ok(actual) if actual == digest)
    })
}

/// Commit the proven generation through the same atomic path a local bake uses.
fn commit_staged_generation(
    output: &Path,
    scratch: &Path,
    manifest: &OvenLoafEnvelopeManifest,
    staged_generation: &Path,
    generation_relative: &Path,
) -> io::Result<()> {
    let generations_root = output.join("generations");
    fs::create_dir_all(&generations_root)?;
    commit_loaf_generation(
        output,
        &generations_root,
        &output.join(generation_relative),
        staged_generation,
        manifest,
        scratch,
        || Ok(()),
    )
    .map_err(|error| io::Error::other(error.to_string()))
}

/// `generations/<identity-without-prefix>` for one generation identity.
fn generation_directory(generation_identity: &str) -> PathBuf {
    Path::new("generations").join(
        generation_identity
            .strip_prefix("sha256:")
            .unwrap_or(generation_identity),
    )
}

/// Copy a directory tree, hard-linking regular files where the filesystem allows and copying otherwise.
///
/// Symlinks are refused: a mirror is a plain copy of immutable content, and a link could point outside it.
fn copy_tree(source: &Path, destination: &Path) -> io::Result<()> {
    fs::create_dir_all(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let target = destination.join(entry.file_name());
        if file_type.is_symlink() {
            return Err(io::Error::other(format!(
                "mirror content contains a symlink at {}",
                entry.path().display()
            )));
        }
        if file_type.is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else if fs::hard_link(entry.path(), &target).is_err() {
            fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::loaf::OvenReleaseToolchainMember;
    use std::collections::BTreeMap;

    use super::*;
    use crate::loaf::{OVEN_LOAF_ENVELOPE_MANIFEST_SCHEMA_VERSION, OVEN_LOAF_SCHEMA_VERSION, OvenLoafAccounting};
    use crate::rustc::{
        OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION, OvenRustcArtifactExtern, OvenRustcArtifactManifest,
    };
    use oven_store::store::{OvenArtifactKind, OvenArtifactMaterializedFile, OvenStore, OvenStoreLimits};
    use oven_store::test_support::{request as store_request, write_project as write_store_project};
    use oven_store::{OvenBuildIntent, digest_bytes};

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    const EXTERN_BYTES: &[u8] = b"compiled artifact bytes";

    fn evidence() -> BTreeMap<String, String> {
        BTreeMap::from([
            ("rustc_identity".to_string(), "rustc fixture".to_string()),
            ("lock_digest".to_string(), "sha256:lock".to_string()),
        ])
    }

    fn generation_identity(evidence: &BTreeMap<String, String>) -> Result<String, Box<dyn std::error::Error>> {
        Ok(digest_bytes(&serde_json::to_vec(&("release", evidence))?))
    }

    fn members() -> Vec<LoafMemberExpectation> {
        vec![LoafMemberExpectation {
            label: "stdlib".to_string(),
            profile: "debug".to_string(),
            action: "build".to_string(),
            role: OvenLoafMemberRole::CompiledClosureAndSourceAuthority,
        }]
    }

    /// Write one synthetic single-member envelope with a real extern file into `root`, returning its manifest.
    fn write_envelope(
        root: &Path,
        evidence: &BTreeMap<String, String>,
    ) -> Result<OvenLoafEnvelopeManifest, Box<dyn std::error::Error>> {
        let generation_identity = generation_identity(evidence)?;
        let generation = generation_directory(&generation_identity);
        let loaf = OvenLoaf {
            schema_version: OVEN_LOAF_SCHEMA_VERSION,
            build_unit_identity: digest_bytes(b"unit"),
            provenance: Default::default(),
            accounting: OvenLoafAccounting {
                payload_logical_bytes: EXTERN_BYTES.len() as u64,
                payload_physical_bytes: 0,
            },
            compatibility: Default::default(),
            registry_leaves: Vec::new(),
            plan: OvenRustcArtifactManifest {
                schema_version: OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION,
                intent: OvenBuildIntent {
                    target: "fixture-target".to_string(),
                    toolchain: "rustc fixture".to_string(),
                    profile: "debug".to_string(),
                    features: Vec::new(),
                },
                dependency_search_paths: vec!["target/debug/deps".to_string()],
                native_search_paths: Vec::new(),
                externs: vec![OvenRustcArtifactExtern {
                    crate_name: "leaf".to_string(),
                    relative_path: "target/debug/deps/libleaf.rlib".to_string(),
                    digest: digest_bytes(EXTERN_BYTES),
                }],
                entrypoint_dependency_search_paths: Default::default(),
                entrypoint_externs: BTreeMap::new(),
                registry_leaves: Vec::new(),
                registry_sources: Vec::new(),
                compile_environment: BTreeMap::new(),
                vocab_auxiliary_targets: Vec::new(),
                supporting_artifacts: Vec::new(),
            },
        };
        let loaf_bytes = serde_json::to_vec_pretty(&loaf)?;
        let loaf_identity = digest_bytes(&loaf_bytes);
        let relative_directory = generation.join(format!("{}.loaf", loaf_identity.trim_start_matches("sha256:")));
        let directory = root.join(&relative_directory);
        fs::create_dir_all(directory.join("target/debug/deps"))?;
        fs::write(directory.join("loaf.json"), &loaf_bytes)?;
        fs::write(directory.join("target/debug/deps/libleaf.rlib"), EXTERN_BYTES)?;
        let manifest = OvenLoafEnvelopeManifest {
            schema_version: OVEN_LOAF_ENVELOPE_MANIFEST_SCHEMA_VERSION,
            envelope: "release".to_string(),
            generation_identity,
            evidence: evidence.clone(),
            loafs: vec![OvenLoafEnvelopeMember {
                label: "stdlib".to_string(),
                profile: "debug".to_string(),
                action: "build".to_string(),
                role: OvenLoafMemberRole::CompiledClosureAndSourceAuthority,
                build_unit_identity: loaf.build_unit_identity.clone(),
                loaf_identity,
                plan_identity: digest_bytes(&serde_json::to_vec(&loaf.plan)?),
                logical_bytes: loaf_bytes.len() as u64,
                physical_bytes: 0,
                path: relative_directory.join("loaf.json"),
            }],
            release_store_member: None,
            runtime_foundation: None,
            runtime_closure: None,
        };
        fs::write(root.join("envelope.json"), serde_json::to_vec(&manifest)?)?;
        Ok(manifest)
    }

    fn import(
        output: &Path,
        scratch: &Path,
        evidence: &BTreeMap<String, String>,
        mirrors: &[PathBuf],
    ) -> Result<Result<MirroredLoafEnvelope, LoafMirrorMiss>, Box<dyn std::error::Error>> {
        let generation_identity = generation_identity(evidence)?;
        let members = members();
        Ok(import_loaf_envelope_from_mirrors(
            output,
            scratch,
            &LoafEnvelopeExpectation {
                schema_version: OVEN_LOAF_ENVELOPE_MANIFEST_SCHEMA_VERSION,
                envelope: "release",
                generation_identity: &generation_identity,
                evidence,
                members: &members,
                release_store_member: None,
                runtime_foundation: None,
                runtime_closure: None,
            },
            mirrors,
        ))
    }

    #[cfg(unix)]
    fn attach_release_store_member(
        root: &Path,
        manifest: &mut OvenLoafEnvelopeManifest,
    ) -> Result<OvenReleaseStoreMember, Box<dyn std::error::Error>> {
        use std::os::unix::fs::PermissionsExt;

        let project = tempfile::tempdir()?;
        write_store_project(project.path())?;
        let source_root = tempfile::tempdir()?;
        let executable = source_root.path().join("engine");
        fs::write(&executable, b"#!/bin/sh\nexit 0\n")?;
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o755))?;
        let mut request = store_request(project.path(), "release-member", b"opaque release payload")?;
        request.kind = OvenArtifactKind::ProjectOutput;
        request.materialized_files = vec![OvenArtifactMaterializedFile {
            source_path: executable,
            relative_path: "bin/engine".to_string(),
        }];
        let store_relative_path = PathBuf::from("release-store");
        let generation = root.join(generation_directory(&manifest.generation_identity));
        let store = OvenStore::new(
            generation.join(&store_relative_path),
            OvenStoreLimits::new(1_000_000, 1_000_000, 1_000_000),
        );
        let published = store.publish(&request)?;
        let member = OvenReleaseStoreMember {
            schema_version: 1,
            label: "engine".to_string(),
            store_relative_path,
            artifact_identity: published.identity,
        };
        manifest.release_store_member = Some(member.clone());
        fs::write(root.join("envelope.json"), serde_json::to_vec(manifest)?)?;
        Ok(member)
    }

    #[test]
    fn the_expected_generation_is_proven_and_committed() -> TestResult {
        let mirror = tempfile::tempdir()?;
        let output = tempfile::tempdir()?;
        let scratch = tempfile::tempdir_in(output.path())?;
        let manifest = write_envelope(mirror.path(), &evidence())?;
        let admitted = import(
            output.path(),
            scratch.path(),
            &evidence(),
            &[mirror.path().to_path_buf()],
        )?
        .map_err(|miss| miss.to_string())?;
        assert_eq!(admitted.member_count, 1);
        let committed: OvenLoafEnvelopeManifest =
            serde_json::from_slice(&fs::read(output.path().join("envelope.json"))?)?;
        assert_eq!(committed, manifest);
        let loaf_json = output.path().join(&manifest.loafs[0].path);
        assert!(loaf_json.is_file(), "the member's loaf.json is in the root");
        assert_eq!(
            fs::read(
                loaf_json
                    .parent()
                    .ok_or("no parent")?
                    .join("target/debug/deps/libleaf.rlib")
            )?,
            EXTERN_BYTES
        );
        assert!(
            fs::read_dir(scratch.path())?.next().is_none(),
            "nothing is left in scratch after a commit"
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn a_real_release_store_member_is_proven_and_committed_from_a_mirror() -> TestResult {
        let mirror = tempfile::tempdir()?;
        let output = tempfile::tempdir()?;
        let scratch = tempfile::tempdir_in(output.path())?;
        let compatibility = evidence();
        let mut manifest = write_envelope(mirror.path(), &compatibility)?;
        let member = attach_release_store_member(mirror.path(), &mut manifest)?;
        let expected_members = members();
        let expectation = LoafEnvelopeExpectation {
            schema_version: OVEN_LOAF_ENVELOPE_MANIFEST_SCHEMA_VERSION,
            envelope: "release",
            generation_identity: &manifest.generation_identity,
            evidence: &compatibility,
            members: &expected_members,
            release_store_member: Some(&member),
            runtime_foundation: None,
            runtime_closure: None,
        };
        import_loaf_envelope_from_mirrors(
            output.path(),
            scratch.path(),
            &expectation,
            &[mirror.path().to_path_buf()],
        )
        .map_err(|miss| miss.to_string())?;
        let committed: OvenLoafEnvelopeManifest =
            serde_json::from_slice(&fs::read(output.path().join("envelope.json"))?)?;
        assert_eq!(committed.release_store_member, Some(member));
        Ok(())
    }

    #[test]
    fn other_evidence_is_skipped_and_an_absent_mirror_is_not_an_error() -> TestResult {
        let mirror = tempfile::tempdir()?;
        let output = tempfile::tempdir()?;
        let scratch = tempfile::tempdir_in(output.path())?;
        write_envelope(mirror.path(), &evidence())?;
        let mut other = evidence();
        other.insert("lock_digest".to_string(), "sha256:other-lock".to_string());
        let absent = tempfile::tempdir()?;
        let miss = import(
            output.path(),
            scratch.path(),
            &other,
            &[absent.path().to_path_buf(), mirror.path().to_path_buf()],
        )?;
        assert!(
            matches!(miss, Err(LoafMirrorMiss::NoCompatibleEnvelope)),
            "got {miss:?}"
        );
        assert!(!output.path().join("envelope.json").exists());
        Ok(())
    }

    #[test]
    fn a_tampered_artifact_is_refused_and_leaves_nothing_behind() -> TestResult {
        let mirror = tempfile::tempdir()?;
        let output = tempfile::tempdir()?;
        let scratch = tempfile::tempdir_in(output.path())?;
        let manifest = write_envelope(mirror.path(), &evidence())?;
        let artifact = mirror
            .path()
            .join(&manifest.loafs[0].path)
            .parent()
            .ok_or("no parent")?
            .join("target/debug/deps/libleaf.rlib");
        fs::write(&artifact, b"altered on the mirror, same length")?;
        let miss = import(
            output.path(),
            scratch.path(),
            &evidence(),
            &[mirror.path().to_path_buf()],
        )?;
        assert!(
            matches!(miss, Err(LoafMirrorMiss::NoCompatibleEnvelope)),
            "got {miss:?}"
        );
        assert!(!output.path().join("envelope.json").exists());
        assert!(
            !output.path().join("generations").exists(),
            "no generation was committed"
        );
        assert!(fs::read_dir(scratch.path())?.next().is_none(), "staging was removed");
        Ok(())
    }

    #[test]
    fn a_manifest_describing_the_member_differently_is_refused() -> TestResult {
        let mirror = tempfile::tempdir()?;
        let output = tempfile::tempdir()?;
        let scratch = tempfile::tempdir_in(output.path())?;
        let mut manifest = write_envelope(mirror.path(), &evidence())?;
        manifest.loafs[0].build_unit_identity = digest_bytes(b"another unit");
        fs::write(mirror.path().join("envelope.json"), serde_json::to_vec(&manifest)?)?;
        let miss = import(
            output.path(),
            scratch.path(),
            &evidence(),
            &[mirror.path().to_path_buf()],
        )?;
        assert!(
            matches!(miss, Err(LoafMirrorMiss::NoCompatibleEnvelope)),
            "got {miss:?}"
        );
        assert!(!output.path().join("generations").exists());
        Ok(())
    }

    #[test]
    fn a_different_release_store_member_cannot_match_the_expected_generation() -> TestResult {
        let mirror = tempfile::tempdir()?;
        let manifest = write_envelope(mirror.path(), &evidence())?;
        let generation = generation_directory(&manifest.generation_identity);
        let expected = OvenReleaseStoreMember {
            schema_version: 1,
            label: "engine".to_string(),
            store_relative_path: PathBuf::from("release-store"),
            artifact_identity: digest_bytes(b"expected engine"),
        };
        let mut swapped = manifest.clone();
        swapped.release_store_member = Some(OvenReleaseStoreMember {
            artifact_identity: digest_bytes(b"swapped engine"),
            ..expected.clone()
        });
        let members = members();
        let compatibility = evidence();
        let expectation = LoafEnvelopeExpectation {
            schema_version: OVEN_LOAF_ENVELOPE_MANIFEST_SCHEMA_VERSION,
            envelope: "release",
            generation_identity: &manifest.generation_identity,
            evidence: &compatibility,
            members: &members,
            release_store_member: Some(&expected),
            runtime_foundation: None,
            runtime_closure: None,
        };
        assert!(!manifest_matches(&swapped, &expectation, &generation));
        Ok(())
    }

    #[test]
    fn a_different_runtime_foundation_cannot_match_the_expected_generation() -> TestResult {
        let mirror = tempfile::tempdir()?;
        let mut manifest = write_envelope(mirror.path(), &evidence())?;
        let generation = generation_directory(&manifest.generation_identity);
        let compiled = manifest.loafs.first().ok_or("fixture has no compiled Loaf")?;
        let expected = OvenReleaseRuntimeFoundationMember {
            schema_version: crate::loaf::OVEN_RELEASE_RUNTIME_FOUNDATION_MEMBER_SCHEMA_VERSION,
            label: "rust-policy-foundation".to_string(),
            foundation_relative_path: PathBuf::from("runtime-foundation/foundation"),
            foundation_identity: digest_bytes(b"expected foundation"),
            compiled_loaf_identity: compiled.loaf_identity.clone(),
            compiled_plan_identity: compiled.plan_identity.clone(),
            toolchain_owner_identity: digest_bytes(b"toolchain owner"),
            compiler_closure_identity: digest_bytes(b"compiler-closure"),
            toolchain_root_relative_path: PathBuf::from("runtime-foundation/toolchain"),
            toolchain_members: vec![OvenReleaseToolchainMember {
                relative_path: PathBuf::from("bin/rustc"),
                digest: digest_bytes(b"rustc"),
            }],
        };
        manifest.runtime_foundation = Some(OvenReleaseRuntimeFoundationMember {
            foundation_identity: digest_bytes(b"swapped foundation"),
            ..expected.clone()
        });
        let members = members();
        let compatibility = evidence();
        let expectation = LoafEnvelopeExpectation {
            schema_version: OVEN_LOAF_ENVELOPE_MANIFEST_SCHEMA_VERSION,
            envelope: "release",
            generation_identity: &manifest.generation_identity,
            evidence: &compatibility,
            members: &members,
            release_store_member: None,
            runtime_foundation: Some(&expected),
            runtime_closure: None,
        };
        assert!(!manifest_matches(&manifest, &expectation, &generation));
        Ok(())
    }

    #[test]
    fn a_member_path_outside_the_generation_is_refused_before_copying() -> TestResult {
        let mirror = tempfile::tempdir()?;
        let output = tempfile::tempdir()?;
        let scratch = tempfile::tempdir_in(output.path())?;
        let mut manifest = write_envelope(mirror.path(), &evidence())?;
        manifest.loafs[0].path = PathBuf::from("generations/../elsewhere/x.loaf/loaf.json");
        fs::write(mirror.path().join("envelope.json"), serde_json::to_vec(&manifest)?)?;
        let miss = import(
            output.path(),
            scratch.path(),
            &evidence(),
            &[mirror.path().to_path_buf()],
        )?;
        assert!(
            matches!(miss, Err(LoafMirrorMiss::NoCompatibleEnvelope)),
            "got {miss:?}"
        );
        assert!(!output.path().join("generations").exists());
        Ok(())
    }

    #[test]
    fn a_corrupt_mirror_manifest_is_reported() -> TestResult {
        let mirror = tempfile::tempdir()?;
        let output = tempfile::tempdir()?;
        let scratch = tempfile::tempdir_in(output.path())?;
        fs::write(mirror.path().join("envelope.json"), b"{not json")?;
        let miss = import(
            output.path(),
            scratch.path(),
            &evidence(),
            &[mirror.path().to_path_buf()],
        )?;
        assert!(matches!(miss, Err(LoafMirrorMiss::Unreadable { .. })), "got {miss:?}");
        Ok(())
    }
}
