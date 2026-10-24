//! Retain rustc's immutable directory listings across fresh native compilation sessions.
//!
//! Only filenames are retained. Rustc still opens and verifies every selected crate's current metadata in each
//! session; modifying an existing archive never reuses decoded crate facts through this cache.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use rustc_session::search_paths::{PathKind, SearchPath};

/// Bound resident directory listings even when one compiler serves many different dependency graphs.
const MAX_DIRECTORIES: usize = 4096;
static LISTINGS: Mutex<BTreeMap<PathBuf, Listing>> = Mutex::new(BTreeMap::new());

/// A directory's identity, including replacement and nanosecond status changes, rather than its path alone.
#[derive(PartialEq, Eq)]
struct Generation {
    device: u64,
    inode: u64,
    mode: u32,
    size: u64,
    modified: (i64, i64),
    changed: (i64, i64),
}

/// Encode a followed file generation for immutable native-store witnesses; content is never inferred from its name.
pub(crate) fn file_identity(path: &Path) -> Option<String> {
    let value = generation(path)?;
    Some(format!(
        "{}:{}:{}:{}:{}.{}:{}.{}",
        value.device,
        value.inode,
        value.size,
        value.mode,
        value.modified.0,
        value.modified.1,
        value.changed.0,
        value.changed.1
    ))
}

/// Rustc's sorted filename index under the exact directory generation that produced it.
struct Listing {
    generation: Generation,
    search: SearchPath,
}

/// Preserve the original extern-parent and dependency-directory order while reusing unchanged filename indexes.
/// User-provided search flags remain separate and are appended afterward by the callback, just as on the CLI.
pub(crate) fn dependencies(externs: &[(String, PathBuf)], directories: &[PathBuf]) -> Vec<SearchPath> {
    let mut seen = BTreeSet::new();
    let paths = externs
        .iter()
        .filter_map(|(_, path)| path.parent())
        .chain(directories.iter().map(PathBuf::as_path))
        .filter(|path| seen.insert(*path));
    let Ok(mut listings) = LISTINGS.lock() else {
        return paths
            .map(|path| SearchPath::new(PathKind::Dependency, path.to_owned()))
            .collect();
    };
    paths.map(|path| listing(&mut listings, path)).collect()
}

/// Admit one listing only while its followed directory generation matches; unstable and unreadable directories
/// use rustc's ordinary fresh scan. A changed archive with an unchanged name is still read afresh by rustc.
fn listing(listings: &mut BTreeMap<PathBuf, Listing>, path: &Path) -> SearchPath {
    let before = generation(path);
    if let (Some(current), Some(held)) = (&before, listings.get(path)) {
        if *current == held.generation {
            return held.search.clone();
        }
    }
    let search = SearchPath::new(PathKind::Dependency, path.to_owned());
    if let Some(before) = before {
        if generation(path).as_ref() == Some(&before) {
            if listings.len() >= MAX_DIRECTORIES {
                listings.clear();
            }
            listings.insert(
                path.to_owned(),
                Listing {
                    generation: before,
                    search: search.clone(),
                },
            );
        }
    }
    search
}

/// Follow directory symlinks so replacement or retargeting invalidates a retained filename index.
#[cfg(unix)]
fn generation(path: &Path) -> Option<Generation> {
    use std::os::unix::fs::MetadataExt;
    let metadata = std::fs::metadata(path).ok()?;
    Some(Generation {
        device: metadata.dev(),
        inode: metadata.ino(),
        mode: metadata.mode(),
        size: metadata.size(),
        modified: (metadata.mtime(), metadata.mtime_nsec()),
        changed: (metadata.ctime(), metadata.ctime_nsec()),
    })
}

/// Without a complete directory-generation observation, always retain rustc's original fresh scan.
#[cfg(not(unix))]
fn generation(_path: &Path) -> Option<Generation> {
    None
}
