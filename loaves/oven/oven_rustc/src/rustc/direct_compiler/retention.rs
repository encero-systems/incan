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

/// Hash the selected compiler and the bounded sysroot closure that can affect one direct compilation or link.
///
/// Package names, versions and installation locations are deliberately absent. The projection contains the invoked
/// compiler bytes, its actual sysroot compiler when distinct, the driver/LLVM libraries beside that compiler, the
/// host/target Rust libraries selected by the invocation, and the host's self-contained linker when the installation
/// ships one (`collect_compiler_closure_linker` says which files and why). Other installed targets are not members.
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
    collect_compiler_closure_linker(&sysroot, &host, &mut members, &mut total_bytes, MAX_MEMBERS, MAX_BYTES)?;
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
/// identity minted under the old shape. Shape 2 admits the host's self-contained linker; shape 1 stopped at the
/// libraries, and an installation without a linker would otherwise digest identically under both.
pub(crate) fn direct_rustc_compiler_closure_digest(
    members: &BTreeMap<String, String>,
) -> Result<String, OvenRustcError> {
    let material = serde_json::to_vec(&("incan.oven.rustc-rlib-closure/2", members)).map_err(|error| {
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
        materialized_directories: Vec::new(),
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

/// Admit the host's self-contained linker into the closure when the installation ships one.
///
/// Since 1.90 `rustc` links binaries on `x86_64-unknown-linux-gnu` through `lib/rustlib/<host>/bin/gcc-ld/ld.lld`,
/// the wrapper `cc -fuse-ld=lld` resolves through `-B`, which in turn executes `lib/rustlib/<host>/bin/rust-lld`
/// beside it. A retained sysroot without them compiles every `rlib` and then fails its first binary link with "the
/// self-contained linker was requested, but it wasn't found in the target's sysroot". Both are host executables, so
/// only the host's copies are members; the emitted-for target's `bin/` is not walked. Each is admitted when present
/// and skipped when absent, because a `cc`-linked host before 1.90 or a distribution package ships neither, and the
/// optional tools that share the directory (`llvm-tools`, `wasm-component-ld`, `rust-objcopy`) stay out so an
/// installed component that takes no part in a native link cannot change the closure identity. Their bytes count
/// against the same member and byte budget as every other member.
pub(crate) fn collect_compiler_closure_linker(
    sysroot: &Path,
    host: &str,
    members: &mut BTreeMap<String, (PathBuf, String)>,
    total_bytes: &mut u64,
    max_members: usize,
    max_bytes: u64,
) -> Result<(), OvenRustcError> {
    let bin = sysroot.join("lib/rustlib").join(host).join("bin");
    let logical_bin = format!("lib/rustlib/{host}/bin");
    let linker = bin.join("rust-lld");
    match fs::symlink_metadata(&linker) {
        Ok(_) => collect_compiler_closure_file(
            &linker,
            format!("{logical_bin}/rust-lld"),
            members,
            total_bytes,
            max_members,
            max_bytes,
        )?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(OvenRustcError::Io {
                path: linker,
                source: error,
            });
        }
    }
    let wrappers = bin.join("gcc-ld");
    match fs::symlink_metadata(&wrappers) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
            collect_compiler_closure_directory(
                &wrappers,
                &format!("{logical_bin}/gcc-ld"),
                true,
                members,
                total_bytes,
                max_members,
                max_bytes,
            )?;
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(OvenRustcError::Io {
                path: wrappers,
                source: error,
            });
        }
        _ => {
            return Err(OvenRustcError::InvalidInput {
                field: "JEC compiler closure",
                message: format!("{} is not a non-symlink directory", wrappers.display()),
            });
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

#[cfg(all(test, unix))]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};

    use super::{collect_compiler_closure_linker, direct_rustc_compiler_evidence};
    use oven_store::digest_bytes;

    const HOST: &str = "test-host";
    const CROSS_TARGET: &str = "cross-target";

    /// Lay out a sysroot whose `bin/rustc` reports `test-host`, with a linker under the host and under a cross
    /// target, an optional-component tool beside the host linker, and one library per target.
    fn write_synthetic_sysroot(root: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
        let sysroot = root.join("toolchain");
        fs::create_dir_all(sysroot.join("bin"))?;
        let rustc = sysroot.join("bin/rustc");
        fs::write(
            &rustc,
            r#"#!/bin/sh
if [ "$1" = "--print" ] && [ "$2" = "sysroot" ]; then
  cd "$(dirname "$0")/.." || exit 1
  pwd -P
elif [ "$1" = "-vV" ]; then
  printf '%s\n' 'rustc 1.99.0-test' 'host: test-host'
else
  exit 2
fi
"#,
        )?;
        fs::set_permissions(&rustc, fs::Permissions::from_mode(0o755))?;
        for (relative, bytes) in [
            ("lib/libdriver.so", "driver bytes"),
            ("lib/rustlib/test-host/lib/libstd-host.rlib", "host standard library"),
            ("lib/rustlib/test-host/bin/rust-lld", "host linker"),
            ("lib/rustlib/test-host/bin/gcc-ld/ld.lld", "host gnu wrapper"),
            ("lib/rustlib/test-host/bin/gcc-ld/wasm-ld", "host wasm wrapper"),
            ("lib/rustlib/test-host/bin/llvm-ar", "optional llvm-tools member"),
            (
                "lib/rustlib/cross-target/lib/libstd-cross.rlib",
                "cross standard library",
            ),
            ("lib/rustlib/cross-target/bin/rust-lld", "cross linker"),
            ("lib/rustlib/cross-target/bin/gcc-ld/ld.lld", "cross gnu wrapper"),
        ] {
            let path = sysroot.join(relative);
            fs::create_dir_all(path.parent().ok_or("synthetic member has no parent")?)?;
            fs::write(&path, bytes)?;
        }
        Ok(sysroot)
    }

    /// The logical coordinates of a closure's members, for comparing what was retained against what was laid out.
    fn member_paths(members: &[super::OvenDirectRustcCompilerMember]) -> BTreeSet<&str> {
        members.iter().map(|member| member.relative_path.as_str()).collect()
    }

    /// The host's `rust-lld` and `gcc-ld/` wrappers are members; the emitted-for target's linker and an optional
    /// tool beside the host linker are not, and a closure without the linker digests differently.
    #[test]
    fn compiler_evidence_retains_the_host_linker_and_wrappers_only() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let sysroot = write_synthetic_sysroot(root.path())?;
        let rustc = sysroot.join("bin/rustc");

        let evidence = direct_rustc_compiler_evidence(&rustc, CROSS_TARGET)?;
        assert_eq!(evidence.host, HOST);
        assert_eq!(evidence.target, CROSS_TARGET);
        assert_eq!(
            member_paths(&evidence.members),
            BTreeSet::from([
                "bin/rustc",
                "lib/libdriver.so",
                "lib/rustlib/cross-target/lib/libstd-cross.rlib",
                "lib/rustlib/test-host/bin/gcc-ld/ld.lld",
                "lib/rustlib/test-host/bin/gcc-ld/wasm-ld",
                "lib/rustlib/test-host/bin/rust-lld",
                "lib/rustlib/test-host/lib/libstd-host.rlib",
            ])
        );
        let linker = evidence
            .members
            .iter()
            .find(|member| member.relative_path == "lib/rustlib/test-host/bin/rust-lld")
            .ok_or("the host linker was not retained")?;
        assert_eq!(linker.digest, digest_bytes(b"host linker"));
        assert_eq!(
            linker.source_path,
            fs::canonicalize(&sysroot)?.join("lib/rustlib/test-host/bin/rust-lld")
        );

        // A closure that lost its linker digests differently: a Store owner retained before the linker was a
        // member cannot be mistaken for one that can link.
        fs::remove_file(sysroot.join("lib/rustlib/test-host/bin/rust-lld"))?;
        fs::remove_dir_all(sysroot.join("lib/rustlib/test-host/bin/gcc-ld"))?;
        let without_linker = direct_rustc_compiler_evidence(&rustc, CROSS_TARGET)?;
        assert_ne!(without_linker.closure_digest, evidence.closure_digest);
        assert!(
            member_paths(&without_linker.members)
                .iter()
                .all(|path| !path.contains("/bin/")),
            "an installation without a self-contained linker retains no bin/ member"
        );
        Ok(())
    }

    /// The linker's bytes are accounted like every other member's: a budget one byte short of them is refused.
    #[test]
    fn linker_members_count_against_the_closure_byte_budget() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let sysroot = write_synthetic_sysroot(root.path())?;
        let linker_bytes = u64::try_from("host linker".len())?;
        let wrapper_bytes = u64::try_from("host gnu wrapper".len() + "host wasm wrapper".len())?;

        let mut members = BTreeMap::new();
        let mut total_bytes = 0_u64;
        collect_compiler_closure_linker(
            &sysroot,
            HOST,
            &mut members,
            &mut total_bytes,
            8,
            linker_bytes + wrapper_bytes,
        )?;
        assert_eq!(members.len(), 3);
        assert_eq!(total_bytes, linker_bytes + wrapper_bytes);

        let mut over_budget = BTreeMap::new();
        let mut over_budget_bytes = 0_u64;
        let refused = collect_compiler_closure_linker(
            &sysroot,
            HOST,
            &mut over_budget,
            &mut over_budget_bytes,
            8,
            linker_bytes + wrapper_bytes - 1,
        );
        assert!(
            refused
                .as_ref()
                .is_err_and(|error| error.to_string().contains("bounded member or byte budget")),
            "{refused:?}"
        );
        Ok(())
    }

    /// A symlinked `rust-lld` is refused the way a symlinked library is, so the retained bytes are always the real
    /// file's.
    #[test]
    fn a_symlinked_linker_is_refused_like_any_other_member() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let sysroot = write_synthetic_sysroot(root.path())?;
        let linker = sysroot.join("lib/rustlib/test-host/bin/rust-lld");
        fs::remove_file(&linker)?;
        std::os::unix::fs::symlink(sysroot.join("lib/rustlib/cross-target/bin/rust-lld"), &linker)?;

        let refused = direct_rustc_compiler_evidence(&sysroot.join("bin/rustc"), HOST);
        assert!(
            refused
                .as_ref()
                .is_err_and(|error| error.to_string().contains("non-symlink regular file")),
            "{refused:?}"
        );
        Ok(())
    }
}
