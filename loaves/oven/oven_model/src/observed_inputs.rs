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

    /// Observe a regular file and its plain directory ancestors once beneath an authenticated root.
    ///
    /// Returns the same non-following leaf metadata retained by this observer, so a byte consumer can reuse it
    /// instead of restating the initial filesystem checks. The caller must still reconcile `current()` after use
    /// and authenticate the enclosing root independently. Invalid coordinates, missing inputs, links and wrong
    /// file types are errors; unsupported metadata identities disable acceleration without bypassing validation.
    pub fn observe_regular_file_within(&mut self, root: &Path, path: &Path) -> io::Result<fs::Metadata> {
        self.observe_regular_file_within_with_metadata(root, path, |path| fs::symlink_metadata(path))
    }

    /// Share each actual syscall result between path validation and the existing observation state.
    fn observe_regular_file_within_with_metadata(
        &mut self,
        root: &Path,
        path: &Path,
        mut metadata: impl FnMut(&Path) -> io::Result<fs::Metadata>,
    ) -> io::Result<fs::Metadata> {
        let relative = path.strip_prefix(root).map_err(io::Error::other)?;
        if !root.is_absolute()
            || relative.as_os_str().is_empty()
            || relative
                .components()
                .any(|part| !matches!(part, std::path::Component::Normal(_)))
        {
            return Err(io::Error::other(
                "observed file requires a normalized descendant coordinate",
            ));
        }
        let mut prefix = root.to_path_buf();
        for component in relative.components() {
            let parent = metadata(&prefix)?;
            if !parent.is_dir() || parent.file_type().is_symlink() {
                return Err(io::Error::other("observed file ancestor must be a plain directory"));
            }
            self.retain_observation(&prefix, Some(observation_stamp(&parent)?), true);
            prefix.push(component);
        }
        let leaf = metadata(path)?;
        if !leaf.is_file() || leaf.file_type().is_symlink() {
            return Err(io::Error::other("observed input must be a regular non-symlink file"));
        }
        self.retain_observation(path, Some(observation_stamp(&leaf)?), false);
        Ok(leaf)
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
        self.retain_observation(path, stamp, structural);
    }

    /// Preserve the first generation when supplied metadata and ordinary path probes share an input.
    fn retain_observation(&mut self, path: &Path, stamp: Option<FileStamp>, structural: bool) {
        if let Some(existing) = self.paths.get_mut(path) {
            existing.structural &= structural;
            return;
        }
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

#[cfg(all(test, unix))]
mod tests {
    use super::InputObservations;
    use std::{fs, path::PathBuf};

    /// Initial validation visits each coordinate once and preserves the existing same-size edit guard.
    #[test]
    fn regular_file_observation_shares_metadata_and_detects_edits() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let parent = root.path().join("source");
        fs::create_dir(&parent)?;
        let file = parent.join("loaf.toml");
        fs::write(&file, b"original")?;
        let mut inputs = InputObservations::default();
        let mut visited = Vec::<PathBuf>::new();
        let metadata = inputs.observe_regular_file_within_with_metadata(root.path(), &file, |path| {
            visited.push(path.to_path_buf());
            fs::symlink_metadata(path)
        })?;
        assert_eq!(visited, [root.path().to_path_buf(), parent, file.clone()]);
        assert_eq!(metadata.len(), 8);
        assert_eq!(inputs.len(), 3);
        assert!(inputs.current());
        fs::write(&file, b"modified")?;
        fs::File::options()
            .write(true)
            .open(&file)?
            .set_times(fs::FileTimes::new().set_modified(metadata.modified()?))?;
        assert!(!inputs.current());
        // Reobserving cannot overwrite the first generation and hide a change during one operation.
        inputs.observe_regular_file_within(root.path(), &file)?;
        assert!(!inputs.current());
        Ok(())
    }

    /// Plain path validation refuses links, wrong file types and escaping coordinates before reuse.
    #[test]
    fn regular_file_observation_refuses_path_substitution() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let parent = root.path().join("source");
        fs::create_dir(&parent)?;
        let file = parent.join("loaf.toml");
        fs::write(&file, b"original")?;
        let held = root.path().join("held");
        fs::rename(&parent, &held)?;
        std::os::unix::fs::symlink(&held, &parent)?;
        assert!(
            InputObservations::default()
                .observe_regular_file_within(root.path(), &file)
                .is_err()
        );
        fs::remove_file(&parent)?;
        fs::rename(&held, &parent)?;
        let link = parent.join("link");
        std::os::unix::fs::symlink(&file, &link)?;
        for invalid in [
            link,
            parent,
            root.path().join("missing"),
            root.path().join("../outside"),
        ] {
            assert!(
                InputObservations::default()
                    .observe_regular_file_within(root.path(), &invalid)
                    .is_err()
            );
        }
        assert!(
            InputObservations::default()
                .observe_regular_file_within(root.path(), root.path())
                .is_err()
        );
        Ok(())
    }

    /// A parent replaced during the initial walk cannot be retained even if the new file has identical bytes.
    #[test]
    fn regular_file_observation_detects_parent_replacement_during_walk() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let parent = root.path().join("source");
        fs::create_dir(&parent)?;
        let file = parent.join("loaf.toml");
        fs::write(&file, b"original")?;
        let mut inputs = InputObservations::default();
        inputs.observe_regular_file_within_with_metadata(root.path(), &file, |path| {
            if path == file {
                fs::rename(&parent, root.path().join("held"))?;
                fs::create_dir(&parent)?;
                fs::write(&file, b"original")?;
            }
            fs::symlink_metadata(path)
        })?;
        assert!(!inputs.current());
        Ok(())
    }
}
