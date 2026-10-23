//! File-generation proofs for the complete compiler identity and its loaded library's byte equivalence.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
#[cfg(unix)]
use std::os::unix::{
    ffi::OsStrExt,
    fs::{MetadataExt, OpenOptionsExt},
};
use std::path::Path;
use std::process::Command;

use sha2::{Digest, Sha256};

/// Reuse a matching proof or hash the opened library, refusing bytes or file state that changed during verification.
/// Proof storage is only an optimization: missing, malformed, unreadable, or unwritable records cannot bypass hashing.
pub(super) fn verify(library: &Path, digest: &str, proof_path: Option<&Path>) -> io::Result<bool> {
    let mut file = File::open(library)?;
    let before = state(library, &file.metadata()?, "library", digest);
    let cached = proof_path.is_some_and(|path| matches_proof(path, &before));
    tracing::debug!(target: "incan::invocation", cached, "loaded compiler library proof");

    // ---- Byte equivalence on a proof miss ----
    if !cached {
        let mut hash = Sha256::new();
        let mut buffer = [0; 64 * 1024];
        loop {
            let count = file.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            hash.update(&buffer[..count]);
        }
        if hex::encode(hash.finalize()) != digest {
            return Ok(false);
        }
    }

    // ---- Stable file state and proof publication ----
    // Check both the open descriptor and its path: a replacement must not inherit a proof for the old inode.
    if before != state(library, &file.metadata()?, "library", digest)
        || before != state(library, &fs::metadata(library)?, "library", digest)
    {
        return Ok(false);
    }
    if !cached && let Some(path) = proof_path {
        let _ = write_proof(path, &before);
    }
    Ok(true)
}

/// Reuse the complete compiler identity only for the exact executable generation previously queried.
/// Missing or changed records take the original `rustc -vV` path; both the descriptor and path must remain stable.
pub(super) fn verify_compiler(compiler: &Path, identity: &str, proof_path: Option<&Path>) -> io::Result<bool> {
    let file = File::open(compiler)?;
    let before = state(compiler, &file.metadata()?, "compiler", identity);
    let cached = proof_path.is_some_and(|path| matches_proof(path, &before));
    tracing::debug!(target: "incan::invocation", cached, "compiler identity proof");
    if !cached {
        let output = Command::new(compiler)
            .arg("-vV")
            .env_remove("RUSTC_BOOTSTRAP")
            .output()?;
        if !output.status.success() || output.stdout != identity.as_bytes() {
            return Ok(false);
        }
    }
    if before != state(compiler, &file.metadata()?, "compiler", identity)
        || before != state(compiler, &fs::metadata(compiler)?, "compiler", identity)
    {
        return Ok(false);
    }
    if !cached && let Some(path) = proof_path {
        let _ = write_proof(path, &before);
    }
    Ok(true)
}

/// Encode an exact path and file state with its build-bound expectation; ctime invalidates restored-mtime edits.
#[cfg(unix)]
fn state(path: &Path, metadata: &fs::Metadata, kind: &str, expected: &str) -> Vec<u8> {
    serde_json::json!({
        "schema": "incan.driver-file-proof/1",
        "kind": kind,
        "path": hex::encode(path.as_os_str().as_bytes()),
        "device": metadata.dev(),
        "inode": metadata.ino(),
        "size": metadata.len(),
        "mtime": metadata.mtime(),
        "mtime_ns": metadata.mtime_nsec(),
        "ctime": metadata.ctime(),
        "ctime_ns": metadata.ctime_nsec(),
        "expected": expected,
    })
    .to_string()
    .into_bytes()
}

/// Unsupported loader platforms cannot reuse a proof; loaded-library admission already refuses those platforms.
#[cfg(not(unix))]
fn state(_path: &Path, _metadata: &fs::Metadata, _kind: &str, _expected: &str) -> Vec<u8> {
    Vec::new()
}

/// Bound regular-file record reads, treating symlinks, special files, I/O errors, and nonexact records as misses.
fn matches_proof(path: &Path, expected: &[u8]) -> bool {
    if expected.is_empty() {
        return false;
    }
    let result = (|| -> io::Result<bool> {
        if !fs::symlink_metadata(path)?.is_file() {
            return Ok(false);
        }
        let mut bytes = Vec::new();
        File::open(path)?.take(16 * 1024).read_to_end(&mut bytes)?;
        Ok(bytes == expected)
    })();
    result.unwrap_or(false)
}

/// Publish a private, complete record atomically; concurrent verifiers may safely replace the same proof.
/// A stale temporary file or any write failure merely prevents this process from publishing its proof.
fn write_proof(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let temporary = path.with_extension(format!("proof-{}.tmp", std::process::id()));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(&temporary)?;
    let result = file.write_all(bytes).and_then(|()| fs::rename(&temporary, path));
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}
