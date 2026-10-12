//! Replacement-sensitive observations shared by Oven models and compiler input reconciliation.
//!
//! Observations accelerate existing model and source selections. They confer no content or execution authority;
//! changed or unsupported inputs return to their canonical loaders. Paths consulted during a read must be observed
//! before use and reconciled again before the result can be retained.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::{fs, io};

/// Metadata for the exact open file whose bytes were hashed, including replacement and preserved-mtime edits.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct FileStamp {
    length: u64,
    modified: u128,
    identity: Vec<u64>,
}

impl FileStamp {
    /// Logical length of the observed file or directory generation, independent of its path.
    pub fn length(&self) -> u64 {
        self.length
    }

    /// Whether this platform recorded replacement-sensitive identity sufficient for synchronous reuse.
    pub fn supports_retention(&self) -> bool {
        !self.identity.is_empty()
    }

    /// Compare a path ancestor's object identity without treating unrelated children as parser inputs.
    ///
    /// Callers separately guard the complete metadata of directories they enumerate. For a structural ancestor only
    /// replacement or type changes matter; edits to an unrelated sibling must not invalidate every retained project.
    pub fn same_object(&self, other: &Self) -> bool {
        self.supports_retention()
            && other.supports_retention()
            && self.identity.get(..2) == other.identity.get(..2)
            && self.identity.last() == other.identity.last()
    }
}

/// Observe identity, permissions and nanosecond change times for either a regular file or a directory.
pub fn observation_stamp(metadata: &fs::Metadata) -> io::Result<FileStamp> {
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
            u64::from(metadata.mode()),
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

/// Existing metadata observations collected at the model's actual reads, probes and directory enumerations.
#[derive(Default)]
pub struct InputObservations {
    paths: BTreeMap<PathBuf, PathObservation>,
    unsupported: bool,
}

/// Full observations guard consulted inputs; structural ancestors guard resolution without sibling invalidation.
struct PathObservation {
    stamp: Option<FileStamp>,
    structural: bool,
}

impl InputObservations {
    /// Whether all observed paths support replacement-sensitive retained reuse.
    pub fn supports_retention(&self) -> bool {
        !self.unsupported
    }

    /// Refuse retained reuse when a caller's input family cannot be completely observed.
    pub fn disable_retention(&mut self) {
        self.unsupported = true;
    }

    /// Number of consulted paths and structural ancestors reconciled by this observation.
    pub fn len(&self) -> usize {
        self.paths.len()
    }

    /// Whether no path has been observed yet.
    pub fn is_empty(&self) -> bool {
        self.paths.is_empty()
    }

    /// Observe a consulted path and every structural ancestor, refusing retained reuse through symbolic links.
    pub fn observe(&mut self, path: &Path) {
        if !path.is_absolute() {
            self.unsupported = true;
        }
        let mut prefix = PathBuf::new();
        for component in path.components() {
            prefix.push(component);
            self.observe_path(&prefix, prefix != path);
        }
    }

    /// Observe one input beneath an independently authenticated root, excluding already guarded outer ancestors.
    ///
    /// The caller must validate that original root on every handoff. The root and all consulted descendants still
    /// receive non-following observations. Escaping or unnormalized paths disable retention; this does not grant a
    /// filesystem or content capability by itself.
    pub fn observe_within(&mut self, root: &Path, path: &Path) {
        let Ok(relative) = path.strip_prefix(root) else {
            self.disable_retention();
            return;
        };
        if !root.is_absolute()
            || relative
                .components()
                .any(|part| !matches!(part, std::path::Component::Normal(_)))
        {
            self.disable_retention();
            return;
        }
        self.observe_path(root, root != path);
        let mut prefix = root.to_path_buf();
        for component in relative.components() {
            prefix.push(component);
            self.observe_path(&prefix, prefix != path);
        }
    }

    /// Merge an actual consulted input with prior structural observations without changing its original generation.
    fn observe_path(&mut self, path: &Path, structural: bool) {
        if let Some(existing) = self.paths.get_mut(path) {
            existing.structural &= structural;
            return;
        }
        let stamp = match observed_model_path(path) {
            Ok(stamp) => stamp,
            Err(_) => {
                self.unsupported = true;
                return;
            }
        };
        self.unsupported |= stamp.as_ref().is_some_and(|stamp| !stamp.supports_retention());
        self.paths
            .insert(path.to_path_buf(), PathObservation { stamp, structural });
    }

    /// Recheck the same inputs without reading file contents or rediscovering directory children.
    pub fn current(&self) -> bool {
        self.paths.iter().all(|(path, observation)| {
            let Ok(current) = observed_model_path(path) else {
                return false;
            };
            match (&observation.stamp, current) {
                (Some(before), Some(after)) if observation.structural => before.same_object(&after),
                (before, after) => before == &after,
            }
        })
    }
}

/// Use the source detector's existing observation encoding, including explicit absence and no-follow policy.
fn observed_model_path(path: &Path) -> io::Result<Option<FileStamp>> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            Err(io::Error::other("linked model input is not retained"))
        }
        Ok(metadata) if metadata.is_file() || metadata.is_dir() => observation_stamp(&metadata).map(Some),
        Ok(_) => Err(io::Error::other("unsupported project model input")),
        Err(error) if matches!(error.kind(), io::ErrorKind::NotFound | io::ErrorKind::NotADirectory) => Ok(None),
        Err(error) => Err(error),
    }
}
