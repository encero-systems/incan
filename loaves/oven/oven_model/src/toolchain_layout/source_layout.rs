//! Opaque executable-relative source coordinates for compiler-owned publication (#1337/#1698).
//!
//! This physical capability retains original directories and regular member bytes. Package identity, namespace policy,
//! semantic completeness and publication receipts remain the compiler's responsibility above Oven.

use std::collections::BTreeSet;
use std::fs::{self, File};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};

use super::file_observation::{FileIdentity, ObservationError, file_identity, read_stable};

/// Source layout discovered only from the actual canonical executable and fixed installed/development geometry.
///
/// No path, environment variable, working directory or compiled-in checkout can publicly construct this capability.
/// Unix original-file identity is currently required. Held handles are observations, not installer pruning locks.
pub struct CompilerOwnedSourceLayout {
    executable: PathBuf,
    canonical_executable: PathBuf,
    coordinate: SourceCoordinate,
    directories: Vec<RetainedDirectory>,
}

/// One original regular member of an already discovered source layout, retaining its parent and exact bytes.
pub struct CompilerOwnedSourceMember {
    layout: Arc<CompilerOwnedSourceLayout>,
    relative: PathBuf,
    path: PathBuf,
    parents: Vec<RetainedDirectory>,
    file: Mutex<File>,
    identity: FileIdentity,
    bytes: Vec<u8>,
}

/// Refusal to discover or reuse the original executable-relative source coordinates.
#[derive(Debug, thiserror::Error)]
pub enum CompilerOwnedSourceLayoutError {
    /// Original filesystem failure at its exact observed coordinate.
    #[error("could not observe compiler-owned source at {}: {source}", path.display())]
    Io {
        /// Failed coordinate.
        path: PathBuf,
        /// Original filesystem error.
        #[source]
        source: std::io::Error,
    },
    /// A present source layout, original owner or byte invariant failed.
    #[error("invalid compiler-owned source at {}: {reason}", path.display())]
    Invalid {
        /// Refused coordinate.
        path: PathBuf,
        /// Failed invariant.
        reason: &'static str,
    },
    /// More than one fixed source root is present beside the actual executable.
    #[error("competing compiler-owned source roots: {roots:?}")]
    CompetingRoots {
        /// Candidate roots; discovery never chooses one by precedence.
        roots: Vec<PathBuf>,
    },
    /// This implementation has no stable original-file identity on this platform.
    #[error("compiler-owned source selection requires an original-file identity witness (currently Unix)")]
    UnsupportedFileIdentity,
}

/// Exact fixed source coordinate and its confined directory ancestry.
#[derive(Debug, PartialEq, Eq)]
struct SourceCoordinate {
    root: PathBuf,
    source: PathBuf,
    directories: Vec<PathBuf>,
}

/// Original directory handle and coordinate, independent of current descendants or timestamps.
struct RetainedDirectory {
    path: PathBuf,
    file: File,
    identity: FileIdentity,
}

impl CompilerOwnedSourceLayout {
    /// Discover sources from the actual executable, with no ambient fallback or caller-selected source root.
    ///
    /// Supported coordinates are installed `ROOT/bin/*` with `ROOT/stdlib`, checkout `ROOT/target/{debug,release}/*`
    /// and `ROOT/target/compiler-development/bin/*` with `ROOT/loaves/stdlib`. Present invalid or competing candidates
    /// refuse. Absence is returned only when no fixed candidate exists.
    pub fn discover() -> Result<Option<Self>, CompilerOwnedSourceLayoutError> {
        let executable = std::env::current_exe().map_err(|source| io(Path::new("<current executable>"), source))?;
        discover_from_executable(&executable)
    }

    /// Revalidate the original executable target, unique source coordinate and retained directory identities.
    pub fn verify(&self) -> Result<(), CompilerOwnedSourceLayoutError> {
        if fs::canonicalize(&self.executable).map_err(|source| io(&self.executable, source))?
            != self.canonical_executable
            || select_source(&self.executable)?.as_ref() != Some(&self.coordinate)
        {
            return Err(invalid(
                &self.executable,
                "executable target or source coordinate changed",
            ));
        }
        for directory in &self.directories {
            directory.verify()?;
        }
        Ok(())
    }

    /// Return the original canonical source directory after revalidating its ownership coordinate.
    pub fn verified_source_root(&self) -> Result<&Path, CompilerOwnedSourceLayoutError> {
        self.verify()?;
        Ok(&self.coordinate.source)
    }

    /// Return the canonical installation/checkout root after revalidating its original directory ownership.
    pub fn verified_installation_root(&self) -> Result<&Path, CompilerOwnedSourceLayoutError> {
        self.verify()?;
        Ok(&self.coordinate.root)
    }

    /// Retain one confined relative regular file; this physical operation grants no namespace or package authority.
    pub fn open_member(
        self: &Arc<Self>,
        relative: &Path,
    ) -> Result<CompilerOwnedSourceMember, CompilerOwnedSourceLayoutError> {
        self.verify()?;
        let (path, parents) = member_coordinate(&self.coordinate.source, relative)?;
        let parents = parents
            .iter()
            .map(|path| RetainedDirectory::open(path))
            .collect::<Result<Vec<_>, _>>()?;
        let expected = file_identity(&fs::symlink_metadata(&path).map_err(|source| io(&path, source))?)?;
        let mut file = File::open(&path).map_err(|source| io(&path, source))?;
        let (bytes, identity) = read_stable(&mut file, &path)?;
        if identity != expected {
            return Err(invalid(&path, "source member changed during acquisition"));
        }
        let member = CompilerOwnedSourceMember {
            layout: Arc::clone(self),
            relative: relative.to_path_buf(),
            path,
            parents,
            file: Mutex::new(file),
            identity,
            bytes,
        };
        member.verified_bytes()?;
        Ok(member)
    }
}

impl CompilerOwnedSourceMember {
    /// Revalidate the original layout, parent directories, held file and current coordinate's exact original bytes.
    pub fn verified_bytes(&self) -> Result<&[u8], CompilerOwnedSourceLayoutError> {
        self.layout.verify()?;
        if member_coordinate(&self.layout.coordinate.source, &self.relative)?.0 != self.path {
            return Err(invalid(&self.path, "source member coordinate changed"));
        }
        for parent in &self.parents {
            parent.verify()?;
        }
        let mut held = self
            .file
            .lock()
            .map_err(|_| invalid(&self.path, "source member handle lock poisoned"))?;
        let (bytes, identity) = read_stable(&mut held, &self.path)?;
        if identity != self.identity || bytes != self.bytes {
            return Err(invalid(&self.path, "original source member bytes changed"));
        }
        let mut current = File::open(&self.path).map_err(|source| io(&self.path, source))?;
        let (bytes, identity) = read_stable(&mut current, &self.path)?;
        if identity != self.identity || bytes != self.bytes {
            return Err(invalid(&self.path, "source member coordinate or bytes changed"));
        }
        self.layout.verify()?;
        for parent in &self.parents {
            parent.verify()?;
        }
        Ok(&self.bytes)
    }

    /// Original canonical member coordinate; call `verified_bytes` at every authority handoff before using it.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl RetainedDirectory {
    /// Retain a real original directory without accepting a symlink or replacement during acquisition.
    fn open(path: &Path) -> Result<Self, CompilerOwnedSourceLayoutError> {
        let metadata = fs::symlink_metadata(path).map_err(|source| io(path, source))?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(invalid(path, "source parent must be a real directory"));
        }
        let identity = file_identity(&metadata)?;
        let file = File::open(path).map_err(|source| io(path, source))?;
        if file_identity(&file.metadata().map_err(|source| io(path, source))?)? != identity {
            return Err(invalid(path, "source directory changed during acquisition"));
        }
        Ok(Self {
            path: path.to_path_buf(),
            file,
            identity,
        })
    }

    /// Require the original retained directory to still own this canonical no-follow coordinate.
    fn verify(&self) -> Result<(), CompilerOwnedSourceLayoutError> {
        let metadata = fs::symlink_metadata(&self.path).map_err(|source| io(&self.path, source))?;
        if metadata.file_type().is_symlink()
            || !metadata.is_dir()
            || file_identity(&metadata)? != self.identity
            || file_identity(&self.file.metadata().map_err(|source| io(&self.path, source))?)? != self.identity
        {
            return Err(invalid(
                &self.path,
                "original source directory no longer owns its coordinate",
            ));
        }
        Ok(())
    }
}

/// Private path injection is used only by actual-executable discovery and isolated filesystem controls.
fn discover_from_executable(
    executable: &Path,
) -> Result<Option<CompilerOwnedSourceLayout>, CompilerOwnedSourceLayoutError> {
    let Some(coordinate) = select_source(executable)? else {
        return Ok(None);
    };
    let canonical_executable = fs::canonicalize(executable).map_err(|source| io(executable, source))?;
    let directories = coordinate
        .directories
        .iter()
        .map(|path| RetainedDirectory::open(path))
        .collect::<Result<Vec<_>, _>>()?;
    let layout = CompilerOwnedSourceLayout {
        executable: executable.to_path_buf(),
        canonical_executable,
        coordinate,
        directories,
    };
    layout.verify()?;
    Ok(Some(layout))
}

/// Select unique fixed geometries from the canonical executable only; launcher ancestors are never candidates.
fn select_source(executable: &Path) -> Result<Option<SourceCoordinate>, CompilerOwnedSourceLayoutError> {
    if !executable.is_absolute()
        || executable
            .components()
            .any(|part| matches!(part, Component::CurDir | Component::ParentDir))
    {
        return Err(invalid(executable, "executable must be absolute and traversal-free"));
    }
    let canonical = fs::canonicalize(executable).map_err(|source| io(executable, source))?;
    if !fs::metadata(&canonical)
        .map_err(|source| io(&canonical, source))?
        .is_file()
    {
        return Err(invalid(executable, "current executable must resolve to a regular file"));
    }
    let directory = canonical
        .parent()
        .ok_or_else(|| invalid(&canonical, "executable parent missing"))?;
    let mut candidates = Vec::new();
    if directory.file_name().is_some_and(|name| name == "bin") {
        if let Some(root) = directory.parent() {
            candidates.push((root, "stdlib"));
            if root.file_name().is_some_and(|name| name == "compiler-development")
                && let Some(target) = root
                    .parent()
                    .filter(|path| path.file_name().is_some_and(|name| name == "target"))
                && let Some(checkout) = target.parent()
            {
                candidates.push((checkout, "loaves/stdlib"));
            }
        }
    } else if directory
        .file_name()
        .is_some_and(|name| name == "debug" || name == "release")
        && let Some(target) = directory
            .parent()
            .filter(|path| path.file_name().is_some_and(|name| name == "target"))
        && let Some(checkout) = target.parent()
    {
        candidates.push((checkout, "loaves/stdlib"));
    }
    let mut present = Vec::new();
    let mut seen = BTreeSet::new();
    for (root, relative) in candidates {
        if let Some(coordinate) = source_coordinate(root, Path::new(relative))?
            && seen.insert(coordinate.source.clone())
        {
            present.push(coordinate);
        }
    }
    if present.len() > 1 {
        return Err(CompilerOwnedSourceLayoutError::CompetingRoots {
            roots: present.into_iter().map(|item| item.source).collect(),
        });
    }
    Ok(present.pop())
}

/// Walk fixed source directory components without following symlinks; any present invalid component refuses.
fn source_coordinate(root: &Path, relative: &Path) -> Result<Option<SourceCoordinate>, CompilerOwnedSourceLayoutError> {
    let mut path = root.to_path_buf();
    let mut directories = vec![path.clone()];
    for component in relative.components() {
        if !matches!(component, Component::Normal(_)) {
            return Err(invalid(&path, "source geometry must be portable and relative"));
        }
        path.push(component.as_os_str());
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(source) => return Err(io(&path, source)),
        };
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(invalid(&path, "source geometry must contain real directories"));
        }
        if fs::canonicalize(&path).map_err(|source| io(&path, source))? != path {
            return Err(invalid(&path, "source geometry escaped its canonical root"));
        }
        directories.push(path.clone());
    }
    Ok(Some(SourceCoordinate {
        root: root.to_path_buf(),
        source: path,
        directories,
    }))
}

/// Resolve one relative regular member while recording all confined parent coordinates for retention.
fn member_coordinate(root: &Path, relative: &Path) -> Result<(PathBuf, Vec<PathBuf>), CompilerOwnedSourceLayoutError> {
    if relative.to_str().is_none_or(|text| {
        text.split('/').any(|segment| matches!(segment, "" | "." | "..")) || text.contains(['\\', ':'])
    }) || relative.components().any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err(invalid(
            relative,
            "source member must be a nonempty portable relative path",
        ));
    }
    let mut path = root.to_path_buf();
    let mut parents = Vec::new();
    for component in relative.components() {
        path.push(component.as_os_str());
        let metadata = fs::symlink_metadata(&path).map_err(|source| io(&path, source))?;
        if metadata.file_type().is_symlink() || fs::canonicalize(&path).map_err(|source| io(&path, source))? != path {
            return Err(invalid(
                &path,
                "source member contains a symlink or escaped its canonical root",
            ));
        }
        if path == root.join(relative) {
            if !metadata.is_file() {
                return Err(invalid(&path, "source member must be a regular file"));
            }
        } else {
            if !metadata.is_dir() {
                return Err(invalid(&path, "source member parent must be a directory"));
            }
            parents.push(path.clone());
        }
    }
    Ok((path, parents))
}

impl From<ObservationError> for CompilerOwnedSourceLayoutError {
    /// Keep shared original-file observation failures in the source boundary's public error domain.
    fn from(error: ObservationError) -> Self {
        match error {
            ObservationError::Io { path, source } => Self::Io { path, source },
            ObservationError::Invalid { path, reason } => Self::Invalid { path, reason },
            #[cfg(not(unix))]
            ObservationError::UnsupportedFileIdentity => Self::UnsupportedFileIdentity,
        }
    }
}

/// Preserve original filesystem errors and failed coordinates.
fn io(path: &Path, source: std::io::Error) -> CompilerOwnedSourceLayoutError {
    CompilerOwnedSourceLayoutError::Io {
        path: path.to_path_buf(),
        source,
    }
}

/// Report an exact physical source-layout or original-owner invariant violation.
fn invalid(path: &Path, reason: &'static str) -> CompilerOwnedSourceLayoutError {
    CompilerOwnedSourceLayoutError::Invalid {
        path: path.to_path_buf(),
        reason,
    }
}

#[cfg(test)]
mod tests;
