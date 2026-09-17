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
    OVEN_RELEASE_STORE_MEMBER_SCHEMA_VERSION, OvenLoaf, OvenLoafEnvelopeManifest, OvenLoafEnvelopeMember,
    OvenLoafMemberRole, OvenReleaseStoreMember, commit_loaf_generation, validate_stored_loaf,
};
use oven_store::{OvenArtifactKind, PublishedOvenStore, digest_source_tree};

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
    Ok(staged_generation)
}

/// Prove the copied embedded store entry before its generation becomes the committed envelope authority.
fn prove_release_store_member(generation: &Path, member: &OvenReleaseStoreMember) -> io::Result<()> {
    if member.schema_version != OVEN_RELEASE_STORE_MEMBER_SCHEMA_VERSION
        || member.store_relative_path.as_os_str().is_empty()
        || member.store_relative_path.is_absolute()
        || !member
            .store_relative_path
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
    {
        return Err(io::Error::other(
            "mirror release store member has an invalid descriptor",
        ));
    }
    let store_root = generation.join(&member.store_relative_path);
    let selected = PublishedOvenStore::new(&store_root)
        .select_payloads_matching_for_execution(|candidate| candidate.identity == member.artifact_identity)
        .map_err(|error| io::Error::other(error.to_string()))?;
    if selected.len() != 1 {
        return Err(io::Error::other(
            "mirror release store member did not select exactly one artifact",
        ));
    }
    let payload = selected
        .into_iter()
        .next()
        .ok_or_else(|| io::Error::other("mirror release store selection vanished"))?;
    payload
        .verify_materialized_files()
        .map_err(|error| io::Error::other(error.to_string()))?;
    if payload.manifest.kind != OvenArtifactKind::ProjectOutput
        || payload
            .admitted_materialized_files()
            .iter()
            .filter(|file| file.executable)
            .count()
            != 1
    {
        return Err(io::Error::other(
            "mirror release store member has no singular executable ProjectOutput",
        ));
    }
    Ok(())
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
    use std::collections::BTreeMap;

    use super::*;
    use crate::loaf::{OVEN_LOAF_ENVELOPE_MANIFEST_SCHEMA_VERSION, OVEN_LOAF_SCHEMA_VERSION, OvenLoafAccounting};
    use crate::rustc::{
        OVEN_RUSTC_ARTIFACT_MANIFEST_SCHEMA_VERSION, OvenRustcArtifactExtern, OvenRustcArtifactManifest,
    };
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
            },
            mirrors,
        ))
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
