//! Shared original-file identity and stable byte observations for executable-relative compiler data.

use std::fs::{File, Metadata};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Stable original OS identity, independent of content and timestamps.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct FileIdentity {
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
}

/// A failed physical observation, translated by each public layout boundary into its own error contract.
#[derive(Debug)]
pub(super) enum ObservationError {
    /// Original filesystem failure and exact failed coordinate.
    Io { path: PathBuf, source: std::io::Error },
    /// An original regular-file or stable-read invariant failed.
    Invalid { path: PathBuf, reason: &'static str },
    /// A stable same-file witness is not available on this platform.
    #[cfg(not(unix))]
    UnsupportedFileIdentity,
}

/// Metadata observed around a read; timestamps detect concurrent changes but never authorize byte reuse.
#[derive(PartialEq, Eq)]
struct FileObservation {
    identity: FileIdentity,
    length: u64,
    modified: SystemTime,
    #[cfg(unix)]
    changed: (i64, i64),
}

/// Preserve physical identity instead of accepting equal bytes at a replacement coordinate.
#[cfg(unix)]
pub(super) fn file_identity(metadata: &Metadata) -> Result<FileIdentity, ObservationError> {
    use std::os::unix::fs::MetadataExt;
    Ok(FileIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
    })
}

/// Refuse unsupported platforms rather than substituting timestamps for a same-file witness.
#[cfg(not(unix))]
pub(super) fn file_identity(_metadata: &Metadata) -> Result<FileIdentity, ObservationError> {
    Err(ObservationError::UnsupportedFileIdentity)
}

/// Read exact bytes through the original handle and refuse changes observed during the read.
pub(super) fn read_stable(file: &mut File, path: &Path) -> Result<(Vec<u8>, FileIdentity), ObservationError> {
    let before = file_observation(&file.metadata().map_err(|source| io(path, source))?, path)?;
    file.seek(SeekFrom::Start(0)).map_err(|source| io(path, source))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).map_err(|source| io(path, source))?;
    let after = file_observation(&file.metadata().map_err(|source| io(path, source))?, path)?;
    if before != after {
        return Err(ObservationError::Invalid {
            path: path.to_path_buf(),
            reason: "file changed while reading",
        });
    }
    Ok((bytes, after.identity))
}

/// Capture read stability while requiring a regular original file.
fn file_observation(metadata: &Metadata, path: &Path) -> Result<FileObservation, ObservationError> {
    if !metadata.is_file() {
        return Err(ObservationError::Invalid {
            path: path.to_path_buf(),
            reason: "handle must own a regular file",
        });
    }
    #[cfg(unix)]
    use std::os::unix::fs::MetadataExt;
    Ok(FileObservation {
        identity: file_identity(metadata)?,
        length: metadata.len(),
        modified: metadata.modified().map_err(|source| io(path, source))?,
        #[cfg(unix)]
        changed: (metadata.ctime(), metadata.ctime_nsec()),
    })
}

/// Preserve filesystem error provenance for the public boundary's error conversion.
fn io(path: &Path, source: std::io::Error) -> ObservationError {
    ObservationError::Io {
        path: path.to_path_buf(),
        source,
    }
}
