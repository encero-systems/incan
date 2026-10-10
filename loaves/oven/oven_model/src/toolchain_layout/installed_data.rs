//! Opaque executable-relative installed data for #1337/#1698 ordinary package admission.
//!
//! This boundary observes exact bytes and their installation coordinate. It assigns no namespace, package-set or
//! policy meaning to the descriptor, and it does not coordinate installer replacement or pruning.

use std::collections::BTreeSet;
use std::fs::{self, File, Metadata};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

use super::executable_search_bases_for;
use crate::digest::digest_bytes;

/// Stable installation-relative ordinary package descriptor, shared by the compiler and installer.
pub const COMPILER_OWNED_PACKAGE_DESCRIPTOR: &str = "share/incan/packages/toolchain.json";

/// A physically retained installed descriptor discovered only beside the actual running executable.
///
/// Public construction accepts no path, environment override or DTO. Executable symlinks resolve to the actual
/// canonical executable before the existing ancestor search; descriptor components below each root must be real
/// directories and a regular file. A verified view preserves the original bytes and handle, but grants no namespace
/// ownership. Unix file identity is currently required; other platforms refuse until an equivalent stable file-identity
/// witness is available.
pub struct CompilerOwnedInstalledData {
    executable: PathBuf,
    canonical_executable: PathBuf,
    selected: DescriptorCoordinate,
    file: Mutex<File>,
    bytes: Vec<u8>,
    identity: String,
}

/// Borrowed coordinates and original descriptor bytes after one complete revalidation.
///
/// Validation is an observation at acquisition, not a lock against future installer activity. Consumers must retain the
/// anchor and revalidate at their own admission/handoff boundary; this view is not a generation lease or policy proof.
pub struct CompilerOwnedInstalledDataView<'a> {
    anchor: &'a CompilerOwnedInstalledData,
}

/// Refusal to discover or reuse a fixed executable-relative descriptor.
#[derive(Debug, thiserror::Error)]
pub enum CompilerOwnedInstalledDataError {
    /// An actual executable, layout component or descriptor could not be read.
    #[error("could not observe compiler-owned installed data at {}: {source}", path.display())]
    Io {
        /// Exact coordinate whose observation failed.
        path: PathBuf,
        /// Original filesystem error.
        #[source]
        source: std::io::Error,
    },
    /// A present coordinate violates the layout or retained descriptor contract.
    #[error("invalid compiler-owned installed data at {}: {reason}", path.display())]
    Invalid {
        /// Coordinate which cannot be admitted.
        path: PathBuf,
        /// Specific physical invariant which failed.
        reason: &'static str,
    },
    /// More than one executable-relative root claims an installed descriptor.
    #[error("competing compiler-owned installed data roots: {roots:?}")]
    CompetingRoots {
        /// Distinct canonical roots, none of which is preferred by search order.
        roots: Vec<PathBuf>,
    },
    /// This platform has no supported original-file identity witness in this implementation.
    #[error("compiler-owned installed data requires a supported original-file identity witness (currently Unix)")]
    UnsupportedFileIdentity,
}

/// Canonical coordinate and original physical file identity; neither is derived from descriptor claims.
#[derive(Debug, PartialEq, Eq)]
struct DescriptorCoordinate {
    root: PathBuf,
    path: PathBuf,
    file_identity: FileIdentity,
}

/// Stable identity of an opened regular file, distinct from its content digest.
#[derive(Debug, PartialEq, Eq)]
struct FileIdentity {
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
}

/// Detect ordinary concurrent modifications while reading bytes without making timestamps authority for reuse.
#[derive(PartialEq, Eq)]
struct FileObservation {
    identity: FileIdentity,
    length: u64,
    modified: SystemTime,
    #[cfg(unix)]
    changed: (i64, i64),
}

impl CompilerOwnedInstalledData {
    /// Discover the fixed descriptor beside the actual current executable, without ambient or development fallback.
    ///
    /// An absent descriptor returns `None`. Any present invalid candidate refuses the entire discovery, including when
    /// another candidate is valid. This does not parse or authenticate package ownership claims inside the bytes.
    pub fn discover() -> Result<Option<Self>, CompilerOwnedInstalledDataError> {
        let executable = std::env::current_exe().map_err(|source| CompilerOwnedInstalledDataError::Io {
            path: PathBuf::from("<current executable>"),
            source,
        })?;
        discover_from_executable(&executable)
    }

    /// Recheck the original handle, actual current bytes, layout confinement and unique unchanged coordinate.
    ///
    /// This creates no writable state or lock files. Replacing even an equal-byte descriptor refuses because the
    /// original file identity no longer owns the coordinate. No lock here promises safety against installer pruning.
    pub fn verified(&self) -> Result<CompilerOwnedInstalledDataView<'_>, CompilerOwnedInstalledDataError> {
        self.verify_coordinate()?;
        let mut held = self
            .file
            .lock()
            .map_err(|_| invalid(&self.selected.path, "descriptor handle lock poisoned"))?;
        let (held_bytes, held_identity) = read_stable(&mut held, &self.selected.path)?;
        if held_identity != self.selected.file_identity || held_bytes != self.bytes {
            return Err(invalid(&self.selected.path, "original descriptor bytes changed"));
        }
        let mut current = open(&self.selected.path)?;
        let (current_bytes, current_identity) = read_stable(&mut current, &self.selected.path)?;
        if current_identity != self.selected.file_identity || current_bytes != self.bytes {
            return Err(invalid(&self.selected.path, "descriptor coordinate or bytes changed"));
        }
        self.verify_coordinate()?;
        Ok(CompilerOwnedInstalledDataView { anchor: self })
    }

    /// Re-enumerate candidates so added competitors and retargeted executable aliases cannot reuse an old anchor.
    fn verify_coordinate(&self) -> Result<(), CompilerOwnedInstalledDataError> {
        if canonicalize(&self.executable)? != self.canonical_executable {
            return Err(invalid(&self.executable, "executable coordinate changed"));
        }
        if select_descriptor(&self.executable)?.as_ref() != Some(&self.selected) {
            return Err(invalid(&self.selected.path, "installed descriptor coordinate changed"));
        }
        Ok(())
    }
}

impl CompilerOwnedInstalledDataView<'_> {
    /// Exact original descriptor bytes; the digest alone is not a substitute for these bytes or upper-layer policy.
    pub fn descriptor_bytes(&self) -> &[u8] {
        &self.anchor.bytes
    }

    /// SHA-256 identity of the retained exact descriptor bytes, independent of installation location.
    pub fn descriptor_identity(&self) -> &str {
        &self.anchor.identity
    }

    /// Canonical executable-relative installation root which owns the fixed descriptor coordinate.
    pub fn installation_root(&self) -> &Path {
        &self.anchor.selected.root
    }

    /// Canonical regular-file coordinate of the descriptor below the installation root.
    pub fn descriptor_path(&self) -> &Path {
        &self.anchor.selected.path
    }
}

/// Private path injection used by the actual-executable factory and isolated filesystem controls only.
fn discover_from_executable(
    executable: &Path,
) -> Result<Option<CompilerOwnedInstalledData>, CompilerOwnedInstalledDataError> {
    let Some(selected) = select_descriptor(executable)? else {
        return Ok(None);
    };
    let canonical_executable = canonicalize(executable)?;
    let mut file = open(&selected.path)?;
    let (bytes, identity) = read_stable(&mut file, &selected.path)?;
    if identity != selected.file_identity {
        return Err(invalid(&selected.path, "descriptor changed during discovery"));
    }
    let anchor = CompilerOwnedInstalledData {
        executable: executable.to_path_buf(),
        canonical_executable,
        identity: digest_bytes(&bytes),
        selected,
        file: Mutex::new(file),
        bytes,
    };
    anchor.verified()?;
    Ok(Some(anchor))
}

/// Admit one unique candidate using existing executable geometry, refusing every present invalid layout.
fn select_descriptor(executable: &Path) -> Result<Option<DescriptorCoordinate>, CompilerOwnedInstalledDataError> {
    if !executable.is_absolute()
        || executable
            .components()
            .any(|part| matches!(part, Component::CurDir | Component::ParentDir))
    {
        return Err(invalid(
            executable,
            "executable coordinate must be absolute and traversal-free",
        ));
    }
    let canonical_executable = canonicalize(executable)?;
    if !metadata(&canonical_executable)?.is_file() {
        return Err(invalid(executable, "current executable must resolve to a regular file"));
    }
    let mut seen = BTreeSet::new();
    let mut candidates = Vec::new();
    for base in executable_search_bases_for(&canonical_executable) {
        let root = canonicalize(&base)?;
        if seen.insert(root.clone())
            && let Some(candidate) = descriptor_in_root(root)?
        {
            candidates.push(candidate);
        }
    }
    if candidates.len() > 1 {
        return Err(CompilerOwnedInstalledDataError::CompetingRoots {
            roots: candidates.into_iter().map(|candidate| candidate.root).collect(),
        });
    }
    Ok(candidates.pop())
}

/// Walk the fixed portable coordinate without following any descriptor component symlink.
fn descriptor_in_root(root: PathBuf) -> Result<Option<DescriptorCoordinate>, CompilerOwnedInstalledDataError> {
    let mut path = root.clone();
    for component in Path::new(COMPILER_OWNED_PACKAGE_DESCRIPTOR).components() {
        if !matches!(component, Component::Normal(_)) {
            return Err(invalid(
                &path,
                "descriptor coordinate must remain relative and traversal-free",
            ));
        }
        path.push(component.as_os_str());
        let observed = match fs::symlink_metadata(&path) {
            Ok(value) => value,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(source) => return Err(CompilerOwnedInstalledDataError::Io { path, source }),
        };
        if observed.file_type().is_symlink() {
            return Err(invalid(&path, "descriptor layout contains a symlink"));
        }
        if path == root.join(COMPILER_OWNED_PACKAGE_DESCRIPTOR) {
            if !observed.is_file() {
                return Err(invalid(&path, "descriptor must be a regular file"));
            }
            if canonicalize(&path)? != path {
                return Err(invalid(&path, "descriptor escaped its canonical root"));
            }
            return Ok(Some(DescriptorCoordinate {
                root,
                file_identity: file_identity(&observed)?,
                path,
            }));
        }
        if !observed.is_dir() {
            return Err(invalid(&path, "descriptor parent must be a directory"));
        }
    }
    Err(invalid(&path, "descriptor coordinate is empty"))
}

/// Read actual bytes through an existing handle and reject modifications observed during that read.
fn read_stable(file: &mut File, path: &Path) -> Result<(Vec<u8>, FileIdentity), CompilerOwnedInstalledDataError> {
    let before = file_observation(&file.metadata().map_err(|source| io(path, source))?, path)?;
    file.seek(SeekFrom::Start(0)).map_err(|source| io(path, source))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).map_err(|source| io(path, source))?;
    let after = file_observation(&file.metadata().map_err(|source| io(path, source))?, path)?;
    if before != after {
        return Err(invalid(path, "descriptor changed while reading"));
    }
    Ok((bytes, after.identity))
}

/// Preserve physical identity rather than treating equal content at a new file as the retained original owner.
#[cfg(unix)]
fn file_identity(metadata: &Metadata) -> Result<FileIdentity, CompilerOwnedInstalledDataError> {
    use std::os::unix::fs::MetadataExt;
    Ok(FileIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
    })
}

/// Refuse unsupported platforms instead of substituting timestamps or equal bytes for a same-file witness.
#[cfg(not(unix))]
fn file_identity(_metadata: &Metadata) -> Result<FileIdentity, CompilerOwnedInstalledDataError> {
    Err(CompilerOwnedInstalledDataError::UnsupportedFileIdentity)
}

/// Capture read stability while leaving timestamps outside the retained descriptor's semantic identity.
fn file_observation(metadata: &Metadata, path: &Path) -> Result<FileObservation, CompilerOwnedInstalledDataError> {
    if !metadata.is_file() {
        return Err(invalid(path, "descriptor handle must own a regular file"));
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

/// Open a descriptor read-only, retaining its original OS file handle.
fn open(path: &Path) -> Result<File, CompilerOwnedInstalledDataError> {
    File::open(path).map_err(|source| io(path, source))
}

/// Canonicalize an observed coordinate without dropping filesystem error provenance.
fn canonicalize(path: &Path) -> Result<PathBuf, CompilerOwnedInstalledDataError> {
    fs::canonicalize(path).map_err(|source| io(path, source))
}

/// Read physical metadata at an already canonical coordinate.
fn metadata(path: &Path) -> Result<Metadata, CompilerOwnedInstalledDataError> {
    fs::metadata(path).map_err(|source| io(path, source))
}

/// Retain exact failed coordinates and original filesystem errors.
fn io(path: &Path, source: std::io::Error) -> CompilerOwnedInstalledDataError {
    CompilerOwnedInstalledDataError::Io {
        path: path.to_path_buf(),
        source,
    }
}

/// Refuse an observed layout or original-owner contract violation.
fn invalid(path: &Path, reason: &'static str) -> CompilerOwnedInstalledDataError {
    CompilerOwnedInstalledDataError::Invalid {
        path: path.to_path_buf(),
        reason,
    }
}

#[cfg(test)]
mod tests;
