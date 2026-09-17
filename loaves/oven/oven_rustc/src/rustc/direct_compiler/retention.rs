//! Retaining a store-owned `rustc` under one lease: the evidence that names the exact binary and closure digests, the
//! domain it is filed under, admission of an already retained owner, and the closure walk that collects what a
//! compiler's sysroot ships.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use super::super::{
    OvenRustcError, digest_regular_file, rustc_host_target, rustc_identity, rustc_sysroot, verified_regular_file,
};
use super::{
    OVEN_DIRECT_RUSTC_COMPILER_DOMAIN_PREFIX, OVEN_DIRECT_RUSTC_COMPILER_OWNER_SCHEMA_VERSION,
    OvenDirectRustcCompilerEvidence, OvenDirectRustcCompilerMember, OvenDirectRustcCompilerOwnerPayload,
    OvenDirectRustcCompilerRetention, OvenOwnedDirectRustcCompiler,
};
use oven_store::store::{
    OvenArtifactKind, OvenArtifactMaterializedFile, OvenArtifactPublishRequest, OvenStore, OvenStoreExecutionPayload,
};
use oven_store::{OvenReceipt, digest_bytes};

/// Hash the selected compiler and the bounded sysroot closure that can affect one direct `rlib` compilation.
///
/// Package names, versions and installation locations are deliberately absent. The projection contains the invoked
/// compiler bytes, its actual sysroot compiler when distinct, the driver/LLVM libraries beside that compiler, and
/// the host/target Rust libraries selected by the invocation. Other installed targets are not members.
pub fn direct_rustc_compiler_evidence(
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

/// Digest one compiler closure from its members' logical coordinates and byte identities.
///
/// The schema tag is folded in so a later change to what a closure contains cannot silently collide with an
/// identity minted under the old shape.
pub(crate) fn direct_rustc_compiler_closure_digest(
    members: &BTreeMap<String, String>,
) -> Result<String, OvenRustcError> {
    let material = serde_json::to_vec(&("incan.oven.rustc-rlib-closure/1", members)).map_err(|error| {
        OvenRustcError::InvalidInput {
            field: "JEC compiler closure",
            message: format!("cannot encode compiler-member identities: {error}"),
        }
    })?;
    Ok(digest_bytes(&material))
}

/// Derive the store compatibility domain one compiler closure is published into.
///
/// Keying the domain on the closure digest is what keeps two installations of the same toolchain version from
/// sharing an entry when their closures actually differ.
pub(crate) fn direct_rustc_compiler_domain(closure_digest: &str) -> Result<String, OvenRustcError> {
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
/// compilation available. A single compatible Store closure is verified and preferred before the ambient compiler is
/// executed or its sysroot is hashed. Multiple byte-distinct closures with the same target/toolchain identity force a
/// cold observation so the ambient compiler bytes disambiguate them.
///
/// Once selected, that verified Store closure is Oven's compiler authority for the batch. The caller's mutable Rustc
/// path supplies identity and bytes only for a cold or ambiguous selection; it is never executed in place when a
/// unique warm owner can be admitted.
pub fn retain_direct_rustc_compiler(
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
    let toolchain = receipt.intent.toolchain.clone();
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
            matching_direct_rustc_compiler_owner_payload(owner, target, &toolchain, None)
                .map(|payload| (index, payload))
        })
        .collect::<Vec<_>>();
    let closure_identities = compatible
        .iter()
        .map(|(_, payload)| {
            (
                payload.closure_digest.as_str(),
                payload.binary_digest.as_str(),
                payload.host.as_str(),
            )
        })
        .collect::<BTreeSet<_>>();
    if closure_identities.len() == 1
        && let Some((index, payload)) = compatible.first()
    {
        let host = payload.host.clone();
        let owner = selected.remove(*index);
        if let Some(owned) = admit_direct_rustc_compiler_owner(owner, target, &toolchain, &host, None)? {
            return Ok(OvenDirectRustcCompilerRetention::Retained(Box::new(owned)));
        }
    }

    let candidate_toolchain = rustc_identity(candidate_rustc)?;
    if toolchain != candidate_toolchain {
        return Err(OvenRustcError::ToolchainMismatch {
            expected: toolchain,
            actual: candidate_toolchain,
        });
    }
    let host = rustc_host_target(candidate_rustc)?;
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
        matching_direct_rustc_compiler_owner_payload(owner, target, &toolchain, Some(&host)).as_ref() == Some(&payload)
    }) {
        let owner = selected.remove(index);
        if let Some(owned) = admit_direct_rustc_compiler_owner(owner, target, &toolchain, &host, Some(&evidence))? {
            return Ok(OvenDirectRustcCompilerRetention::Retained(Box::new(owned)));
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
        Some(owned) => Ok(OvenDirectRustcCompilerRetention::Retained(Box::new(owned))),
        None => Ok(OvenDirectRustcCompilerRetention::Unavailable {
            reason: "the published compiler closure failed final admission".to_string(),
        }),
    }
}

/// Decode the small owner descriptor used to select a warm compiler without reading its materialized closure.
pub(crate) fn matching_direct_rustc_compiler_owner_payload(
    owner: &OvenStoreExecutionPayload,
    target: &str,
    toolchain: &str,
    host: Option<&str>,
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
        || host.is_some_and(|host| payload.host != host)
        || direct_rustc_compiler_domain(&payload.closure_digest).ok().as_deref() != Some(owner.manifest.domain.as_str())
    {
        return None;
    }
    Some(payload)
}

/// Admit one retained store entry as this batch's compiler, or say why it cannot serve.
///
/// The entry's recorded host, target and toolchain must match the request, and where the caller already holds
/// expected evidence it must match that too. A mismatch is reported as unavailability rather than as an error:
/// the caller can still compile through its own admitted compiler, it just loses byte-identical reuse.
pub(crate) fn admit_direct_rustc_compiler_owner(
    owner: OvenStoreExecutionPayload,
    target: &str,
    toolchain: &str,
    host: &str,
    expected: Option<&OvenDirectRustcCompilerEvidence>,
) -> Result<Option<OvenOwnedDirectRustcCompiler>, OvenRustcError> {
    let Some(payload) = matching_direct_rustc_compiler_owner_payload(&owner, target, toolchain, Some(host)) else {
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
        owner,
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
/// Walk one directory of the compiler installation into logical members, bounded by count and bytes.
///
/// The bounds are the point: this reads a directory the caller named, and an unbounded walk of a sysroot would
/// admit an arbitrary amount of material into an identity. `logical_root` keeps each member's coordinate
/// independent of where the installation happens to live.
pub(crate) fn collect_compiler_closure_directory(
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

/// Admit one compiler-closure file under its logical coordinate, enforcing the member and byte bounds.
pub(crate) fn collect_compiler_closure_file(
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
