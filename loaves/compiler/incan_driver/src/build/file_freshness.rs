//! Content digests accelerated by observed regular-file identity, never by a caller's recorded digest.

use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Metadata for the exact open file whose bytes were hashed, including replacement and preserved-mtime edits.
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
struct FileStamp {
    length: u64,
    modified: u128,
    identity: Vec<u64>,
}

/// A local acceleration record; its path is excluded from the returned portable content identity.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileDigestRecord {
    stamp: FileStamp,
    digest: String,
}

/// Read metadata from the held handle so a path replacement cannot stamp bytes from another file.
fn stamp(file: &fs::File) -> io::Result<FileStamp> {
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(io::Error::other("build input must be a regular file"));
    }
    let modified = metadata
        .modified()?
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(io::Error::other)?
        .as_nanos();
    #[cfg(unix)]
    let identity = {
        use std::os::unix::fs::MetadataExt;
        vec![
            metadata.dev(),
            metadata.ino(),
            metadata.ctime().cast_unsigned(),
            metadata.ctime_nsec().cast_unsigned(),
        ]
    };
    #[cfg(not(unix))]
    let identity = Vec::new();
    Ok(FileStamp {
        length: metadata.len(),
        modified,
        identity,
    })
}

/// Resolve optional local cache storage through the same home precedence as the Oven store.
fn cache_root() -> Option<PathBuf> {
    std::env::var_os("INCAN_HOME")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            crate::oven_store::user_home()
                .filter(|path| !path.is_empty())
                .map(|path| PathBuf::from(path).join(".incan"))
        })
        .map(|root| root.join("cache/file-digests-v1"))
}

/// Return a content identity with optional stat-based acceleration; unsupported platforms always read bytes.
///
/// Cache write failure does not affect correctness. A changed stamp always rehashes, and a file changing during
/// that read fails rather than publishing a stamp for bytes that were never proved. On Unix, inode and ctime
/// reject replacements and edits that retain size and mtime. Other platforms conservatively do not reuse stamps.
pub(super) fn digest_file(path: &Path) -> io::Result<String> {
    digest_file_in(path, cache_root().as_deref())
}

/// Compute one regular-file digest in an explicit cache root, allowing isolated tests without environment mutation.
fn digest_file_in(path: &Path, cache: Option<&Path>) -> io::Result<String> {
    let path = fs::canonicalize(path)?;
    let cache_path = cache.map(|root| {
        root.join(format!(
            "{:x}.json",
            Sha256::digest(path.as_os_str().as_encoded_bytes())
        ))
    });
    let mut file = fs::File::open(&path)?;
    let before = stamp(&file)?;
    if !before.identity.is_empty()
        && let Some(record) = cache_path
            .as_ref()
            .and_then(|path| fs::read(path).ok())
            .and_then(|bytes| serde_json::from_slice::<FileDigestRecord>(&bytes).ok())
        && record.stamp == before
        && record
            .digest
            .strip_prefix("sha256:")
            .is_some_and(|hex| hex.len() == 64 && hex.bytes().all(|byte| byte.is_ascii_hexdigit()))
    {
        return Ok(record.digest);
    }
    let mut hash = Sha256::new();
    let mut buffer = [0; 65536];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hash.update(&buffer[..read]);
    }
    if stamp(&file)? != before {
        return Err(io::Error::other("build input changed while hashing"));
    }
    let digest = format!("sha256:{:x}", hash.finalize());
    if let Some(cache_path) = cache_path {
        let record = FileDigestRecord {
            stamp: before,
            digest: digest.clone(),
        };
        let _published = publish_record(&cache_path, &record);
    }
    Ok(digest)
}

/// Publish a complete cache record atomically; concurrent commands may replace equivalent observations.
fn publish_record(path: &Path, record: &FileDigestRecord) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("digest cache has no parent"))?;
    fs::create_dir_all(parent)?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    temporary.write_all(&serde_json::to_vec(record)?)?;
    temporary.persist(path).map_err(|error| error.error)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Relocation changes the stat lookup coordinate, but never the returned content identity.
    #[test]
    fn copied_files_share_content_identity_and_edits_invalidate() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let cache = root.path().join("cache");
        let original = root.path().join("original");
        let copied = root.path().join("copied");
        fs::write(&original, b"same bytes")?;
        let first = digest_file_in(&original, Some(&cache))?;
        assert_eq!(first, digest_file_in(&original, Some(&cache))?);
        fs::copy(&original, &copied)?;
        assert_eq!(first, digest_file_in(&copied, Some(&cache))?);
        let modified = fs::metadata(&original)?.modified()?;
        fs::write(&original, b"edit bytes")?;
        fs::File::options()
            .write(true)
            .open(&original)?
            .set_times(fs::FileTimes::new().set_modified(modified))?;
        assert_ne!(first, digest_file_in(&original, Some(&cache))?);
        assert_eq!(first, digest_file_in(&copied, Some(&cache))?);
        Ok(())
    }

    /// Missing or malformed acceleration records fall back to bytes rather than accepting an unknown identity.
    #[test]
    fn malformed_record_is_a_miss() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let cache = root.path().join("cache");
        let source = root.path().join("source");
        fs::write(&source, b"authoritative")?;
        let digest = digest_file_in(&source, Some(&cache))?;
        for entry in fs::read_dir(&cache)? {
            fs::write(entry?.path(), b"invalid")?;
        }
        assert_eq!(digest, digest_file_in(&source, Some(&cache))?);
        Ok(())
    }

    /// Measure actual executable-sized input validation separately from graph planning and output restoration.
    #[test]
    fn unchanged_executable_digest_reports_cold_and_warm_probe_times() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let executable = std::env::current_exe()?;
        let started = std::time::Instant::now();
        let digest = digest_file_in(&executable, Some(root.path()))?;
        let cold = started.elapsed();
        let started = std::time::Instant::now();
        for _ in 0..4 {
            assert_eq!(digest, digest_file_in(&executable, Some(root.path()))?);
        }
        eprintln!(
            "executable freshness: {} bytes, cold {:.3} ms, four warm probes {:.3} ms",
            fs::metadata(executable)?.len(),
            cold.as_secs_f64() * 1000.0,
            started.elapsed().as_secs_f64() * 1000.0
        );
        Ok(())
    }
}
