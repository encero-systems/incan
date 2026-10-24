//! Read a compiled manifest and retain its content identity only for an unchanged file generation.
//!
//! Records are optional, private toolchain-home accelerators, under the same trusted-home assumption as the driver
//! identity proofs. They do not protect against forged records or filesystem metadata. Parsing and semantic validation
//! still consume the current bytes; missing, corrupt or unwritable records take the full hashing path.

use std::fs::{self, File, Metadata, OpenOptions};
use std::io::{self, Read, Write};
#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// The generation guards used for a digest, including ctime so restored-mtime edits invalidate it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Generation {
    device: u64,
    inode: u64,
    size: u64,
    modified: i64,
    modified_ns: i64,
    changed: i64,
    changed_ns: i64,
}

/// An integrity-checked local record of the digest computed for one canonical file generation.
#[derive(Serialize, Deserialize)]
struct Record {
    version: u32,
    key: String,
    generation: Generation,
    digest: String,
    checksum: String,
}

/// Read current UTF-8 bytes, retaining or computing their digest only while descriptor and path stay stable.
pub(super) fn read(path: &Path) -> io::Result<(String, String)> {
    // ---- Current bytes and their file generation ----
    let canonical = fs::canonicalize(path)?;
    let mut file = File::open(&canonical)?;
    let before = generation(&file.metadata()?);
    let key = format!("{:x}", Sha256::digest(canonical.as_os_str().as_encoded_bytes()));
    let cache = cache_path(&key);
    let retained = before.and_then(|state| cache.as_deref().and_then(|path| retained_digest(path, &key, state)));
    tracing::debug!(target: "incan::invocation", cached = retained.is_some(), "Loaf manifest digest");
    let mut content = String::new();
    file.read_to_string(&mut content)?;

    // ---- Stable descriptor and path ----
    if let Some(state) = before
        && (generation(&file.metadata()?) != Some(state) || generation(&fs::metadata(path)?) != Some(state))
    {
        return Err(io::Error::other("the manifest changed while its bytes were read"));
    }

    // ---- Retained identity or a fresh content hash ----
    if let Some(digest) = retained {
        return Ok((content, digest));
    }
    let digest = format!("sha256:{:x}", Sha256::digest(content.as_bytes()));
    if let (Some(state), Some(path)) = (before, cache) {
        let _ = store(&path, &key, state, &digest);
    }
    Ok((content, digest))
}

/// Observe the same Unix file-generation fields used by the toolchain's identity proofs.
#[cfg(unix)]
pub(super) fn generation(metadata: &Metadata) -> Option<Generation> {
    if !metadata.is_file() {
        return None;
    }
    Some(Generation {
        device: metadata.dev(),
        inode: metadata.ino(),
        size: metadata.len(),
        modified: metadata.mtime(),
        modified_ns: metadata.mtime_nsec(),
        changed: metadata.ctime(),
        changed_ns: metadata.ctime_nsec(),
    })
}

/// Platforms without these generation guards keep the original content-hashing behavior.
#[cfg(not(unix))]
pub(super) fn generation(_metadata: &Metadata) -> Option<Generation> {
    None
}

/// Use an explicit absolute home, or an installed executable's toolchain home; never write into a build directory.
pub(super) fn cache_path(key: &str) -> Option<PathBuf> {
    let explicit = std::env::var_os("INCAN_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute());
    let root = explicit.or_else(|| {
        let executable = std::env::current_exe().ok()?;
        let bin = executable.parent()?;
        let root = bin.parent()?;
        (bin.file_name()? == "bin" && root.join("toolchain.json").is_file()).then(|| root.to_owned())
    })?;
    Some(root.join("cache/loaf-manifest-identities").join(format!("{key}.json")))
}

/// Refuse malformed, oversized, symlinked, stale or corrupt records without changing the manifest's acceptance.
fn retained_digest(path: &Path, key: &str, generation: Generation) -> Option<String> {
    let metadata = fs::symlink_metadata(path).ok()?;
    if !metadata.is_file() || metadata.len() > 8192 {
        return None;
    }
    let mut bytes = Vec::new();
    File::open(path).ok()?.take(8192).read_to_end(&mut bytes).ok()?;
    let record: Record = serde_json::from_slice(&bytes).ok()?;
    if record.version != 1 || record.key != key || record.generation != generation {
        return None;
    }
    let digest = record.digest.strip_prefix("sha256:")?;
    if digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    if record.checksum != checksum(&record.key, generation, &record.digest).ok()? {
        return None;
    }
    Some(record.digest)
}

/// Detect accidental corruption of a record independently of the current manifest's content hash.
fn checksum(key: &str, generation: Generation, digest: &str) -> io::Result<String> {
    let bytes = serde_json::to_vec(&(1_u32, key, generation, digest)).map_err(io::Error::other)?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

/// Publish one complete private record atomically; failures leave the already-computed identity usable.
fn store(path: &Path, key: &str, generation: Generation, digest: &str) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("the manifest record has no parent directory"))?;
    fs::create_dir_all(parent)?;
    let record = Record {
        version: 1,
        key: key.to_owned(),
        generation,
        digest: digest.to_owned(),
        checksum: checksum(key, generation, digest)?,
    };
    let temporary = path.with_extension(format!("{}.tmp", std::process::id()));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(&temporary)?;
    let bytes = serde_json::to_vec(&record).map_err(io::Error::other)?;
    let result = file.write_all(&bytes).and_then(|()| fs::rename(&temporary, path));
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}
