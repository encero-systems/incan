//! Read a compiled artifact and retain its content identity only for an unchanged file generation.
//!
//! Records are optional, private toolchain-home accelerators, under the same trusted-home assumption as the driver
//! identity proofs. They do not protect against forged records or filesystem metadata. Parsing and semantic validation
//! still consume the current bytes; missing, corrupt or unwritable records take the full hashing path.

use std::fs::{self, File, Metadata};
use std::io::{self, Read};
#[cfg(unix)]
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

/// Maximum encoded digest proof size; oversized optional records never bypass the full content hash.
const MAX_DIGEST_RECORD_BYTES: u64 = 8192;

/// The generation guards used for a digest, including ctime so restored-mtime edits invalidate it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Generation {
    device: u64,
    inode: u64,
    size: u64,
    mode: u32,
    modified: i64,
    modified_ns: i64,
    changed: i64,
    changed_ns: i64,
}

impl Generation {
    /// Bind the complete observed OS generation and its independent file identity to the cache key.
    pub(super) fn guard(self, key: &[u8]) -> [u8; 32] {
        let mut hash = Sha256::new();
        hash.update(b"incan.loaf-manifest-generation/2");
        hash.update(self.device.to_le_bytes());
        hash.update(self.inode.to_le_bytes());
        hash.update(self.size.to_le_bytes());
        hash.update(self.mode.to_le_bytes());
        hash.update(self.modified.to_le_bytes());
        hash.update(self.modified_ns.to_le_bytes());
        hash.update(self.changed.to_le_bytes());
        hash.update(self.changed_ns.to_le_bytes());
        hash.update(key);
        hash.finalize().into()
    }
}

/// Both source and running compiler must match before restoring any compiler-specific interpretation.
pub(super) fn checked_guard(path: &Path, state: Generation, compiler: &(PathBuf, Generation)) -> [u8; 32] {
    let mut hash = Sha256::new();
    hash.update(state.guard(path.as_os_str().as_encoded_bytes()));
    hash.update(compiler.1.guard(compiler.0.as_os_str().as_encoded_bytes()));
    hash.finalize().into()
}

/// Read current UTF-8 bytes, retaining or computing their digest only while descriptor and path stay stable.
pub(super) fn read(path: &Path) -> io::Result<(String, String)> {
    let (bytes, digest) = read_bytes(path)?;
    let content = String::from_utf8(bytes).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    Ok((content, digest))
}

/// Read current binary bytes under the same generation guards as manifests. A retained digest never bypasses the
/// caller's format validation or its comparison with a manifest-authenticated ABI digest.
pub(super) fn read_bytes(path: &Path) -> io::Result<(Vec<u8>, String)> {
    // ---- Current bytes and their file generation ----
    let canonical = fs::canonicalize(path)?;
    let mut file = File::open(&canonical)?;
    let before = generation(&file.metadata()?);
    let key = format!("{:x}", Sha256::digest(canonical.as_os_str().as_encoded_bytes()));
    let cache = cache_path(&key);
    let retained = before.and_then(|state| cache.as_deref().and_then(|path| retained_digest(path, &key, state)));
    tracing::debug!(target: "incan::invocation", cached = retained.is_some(), path = %path.display(), "Loaf artifact digest");
    let mut content = Vec::new();
    file.read_to_end(&mut content)?;

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
    let digest = format!("sha256:{:x}", Sha256::digest(&content));
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
        mode: metadata.mode(),
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
    Some(
        root.join("cache/loaf-manifest-identities")
            .join(format!("{key}.identity.crumb")),
    )
}

/// An optional private Crumb is selected only by current observations. Legacy filenames are independent.
pub(super) fn read_cache(path: &Path, kind: &str, guard: [u8; 32], maximum: u64) -> Option<(String, Vec<u8>)> {
    let metadata = fs::symlink_metadata(path).ok()?;
    if !metadata.is_file() || metadata.len() > maximum {
        return None;
    }
    let mut bytes = Vec::new();
    File::open(path).ok()?.take(maximum + 1).read_to_end(&mut bytes).ok()?;
    if bytes.len() as u64 > maximum {
        return None;
    }
    let value = internal_models::decode_manifest_cache(&kind.to_owned(), &guard.to_vec(), &bytes).ok()?;
    Some((format!("sha256:{}", hex::encode(value.digest)), value.payload))
}

/// The Incan codec owns verification and framing. Failed optional publication leaves full admission available.
pub(super) fn write_cache(path: &Path, kind: &str, guard: [u8; 32], digest: &str, payload: Vec<u8>) -> io::Result<()> {
    let raw = hex::decode(
        digest
            .strip_prefix("sha256:")
            .ok_or_else(|| io::Error::other("missing manifest digest scheme"))?,
    )
    .map_err(io::Error::other)?;
    let bytes = internal_models::encode_manifest_cache(&kind.to_owned(), &guard.to_vec(), &raw, &payload)
        .map_err(io::Error::other)?;
    super::publish_private_record(path, &bytes, "pending")
}

fn retained_digest(path: &Path, key: &str, generation: Generation) -> Option<String> {
    let (digest, _) = read_cache(
        path,
        "identity",
        generation.guard(key.as_bytes()),
        MAX_DIGEST_RECORD_BYTES,
    )?;
    Some(digest)
}

fn store(path: &Path, key: &str, generation: Generation, digest: &str) -> io::Result<()> {
    write_cache(path, "identity", generation.guard(key.as_bytes()), digest, Vec::new())
}
