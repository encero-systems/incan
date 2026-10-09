//! Content digests accelerated by observed regular-file identity, never by a caller's recorded digest.

use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Metadata for the exact open file whose bytes were hashed, including replacement and preserved-mtime edits.
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct FileStamp {
    length: u64,
    modified: u128,
    identity: Vec<u64>,
}

/// A local acceleration record; its path is excluded from the returned portable content identity.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileDigestRecord {
    scheme: String,
    stamp: FileStamp,
    digest: String,
}

/// Portable memo of one named authority transformation, selected by actual source bytes and scheme.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AuthorityDigestRecord {
    scheme: String,
    digest: String,
}

/// Reject incomplete or malformed cache identities before they can authorize a reuse decision.
fn valid_digest(digest: &str) -> bool {
    digest
        .strip_prefix("sha256:")
        .is_some_and(|hex| hex.len() == 64 && hex.bytes().all(|byte| byte.is_ascii_hexdigit()))
}

/// Read metadata from the held handle so a path replacement cannot stamp bytes from another file.
fn stamp(file: &fs::File) -> io::Result<FileStamp> {
    stamp_metadata(&file.metadata()?)
}

/// Observe a regular input without reading bytes, for before/after source-closure guards.
pub(super) fn stat_file(path: &Path) -> io::Result<FileStamp> {
    stamp_metadata(&fs::symlink_metadata(path)?)
}

/// Encode replacement-sensitive metadata without mixing the local path into content authority.
fn stamp_metadata(metadata: &fs::Metadata) -> io::Result<FileStamp> {
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
    digest_file_with(path, "raw-sha256-v1", |bytes| {
        Ok(format!("sha256:{:x}", Sha256::digest(bytes)))
    })
}

/// Compute one regular-file digest in an explicit cache root, allowing isolated tests without environment mutation.
#[cfg(test)]
fn digest_file_in(path: &Path, cache: Option<&Path>) -> io::Result<String> {
    digest_file_with_in(path, cache, "raw-sha256-v1", |bytes| {
        Ok(format!("sha256:{:x}", Sha256::digest(bytes)))
    })
}

/// Cache a named authority scheme independently of raw file digests; changed schemes always recompute.
pub(super) fn digest_file_with(
    path: &Path,
    scheme: &str,
    digest: impl FnOnce(&[u8]) -> io::Result<String>,
) -> io::Result<String> {
    digest_file_with_in(path, cache_root().as_deref(), scheme, digest)
}

/// Observe and cache the digest of one open regular file under an explicit authority scheme.
fn digest_file_with_in(
    path: &Path,
    cache: Option<&Path>,
    scheme: &str,
    compute: impl FnOnce(&[u8]) -> io::Result<String>,
) -> io::Result<String> {
    let path = fs::canonicalize(path)?;
    let cache_path = cache.map(|root| {
        root.join(format!(
            "{:x}.json",
            Sha256::digest([scheme.as_bytes(), path.as_os_str().as_encoded_bytes()].concat())
        ))
    });
    let mut file = fs::File::open(&path)?;
    let before = stamp(&file)?;
    if !before.identity.is_empty()
        && let Some(record) = cache_path
            .as_ref()
            .and_then(|path| fs::read(path).ok())
            .and_then(|bytes| serde_json::from_slice::<FileDigestRecord>(&bytes).ok())
        && record.scheme == scheme
        && record.stamp == before
        && valid_digest(&record.digest)
    {
        trace_file_digest(&path, scheme, 0);
        return Ok(record.digest);
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    trace_file_digest(&path, scheme, bytes.len());
    let content_path = cache.filter(|_| scheme != "raw-sha256-v1").map(|root| {
        root.join("content").join(format!(
            "{:x}.json",
            Sha256::digest([scheme.as_bytes(), Sha256::digest(&bytes).as_slice()].concat())
        ))
    });
    let portable = content_path
        .as_ref()
        .and_then(|path| fs::read(path).ok())
        .and_then(|bytes| serde_json::from_slice::<AuthorityDigestRecord>(&bytes).ok())
        .filter(|record| record.scheme == scheme && valid_digest(&record.digest));
    let digest = match portable {
        Some(record) => record.digest,
        None => compute(&bytes)?,
    };
    if stamp(&file)? != before {
        return Err(io::Error::other("build input changed while hashing"));
    }
    if let Some(path) = content_path {
        let record = AuthorityDigestRecord {
            scheme: scheme.to_string(),
            digest: digest.clone(),
        };
        let _published = publish_json(&path, &record);
    }
    if let Some(cache_path) = cache_path {
        let record = FileDigestRecord {
            scheme: scheme.to_string(),
            stamp: before,
            digest: digest.clone(),
        };
        let _published = publish_record(&cache_path, &record);
    }
    Ok(digest)
}

/// Report bytes actually read from the authoritative input, excluding acceleration records and metadata IO.
fn trace_file_digest(path: &Path, scheme: &str, input_bytes_read: usize) {
    if std::env::var_os("INCAN_OVEN_TRACE_FILE_DIGESTS").is_some() {
        eprintln!(
            "Oven file digest: {}",
            serde_json::json!({
                "path": path,
                "scheme": scheme,
                "input_bytes_read": input_bytes_read,
            })
        );
    }
}

/// Publish a complete cache record atomically; concurrent commands may replace equivalent observations.
fn publish_record(path: &Path, record: &FileDigestRecord) -> io::Result<()> {
    publish_json(path, record)
}

/// Atomically publish a complete named cache record without making caching a correctness requirement.
fn publish_json(path: &Path, record: &impl Serialize) -> io::Result<()> {
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

    /// Scheme changes cannot consume another authority's cached digest; unchanged stamps skip computation.
    #[test]
    fn authority_scheme_is_bound_and_warm_hits_skip_computation() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let source = root.path().join("source.incn");
        fs::write(&source, "def main() -> None:\n    pass\n")?;
        let raw = digest_file_in(&source, Some(root.path()))?;
        let authority = format!("sha256:{:x}", Sha256::digest(b"normalized tokens"));
        assert_ne!(raw, authority);
        assert_eq!(
            authority,
            digest_file_with_in(&source, Some(root.path()), "tokens-v1", |_| Ok(authority.clone()))?
        );
        assert_eq!(
            authority,
            digest_file_with_in(&source, Some(root.path()), "tokens-v1", |_| Err(io::Error::other(
                "warm hit recomputed"
            )))?
        );
        assert_eq!(raw, digest_file_in(&source, Some(root.path()))?);
        fs::write(&source, "def main() -> None:\n    print(1)\n")?;
        assert!(
            digest_file_with_in(&source, Some(root.path()), "tokens-v1", |_| Err(io::Error::other(
                "changed stamp recomputed"
            )))
            .is_err()
        );
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
