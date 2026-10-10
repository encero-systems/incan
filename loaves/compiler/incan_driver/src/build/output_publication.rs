//! Ordinary-error rollback for the precise caller files replaced by executable replay.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::build::output_paths::validated_project_output_relative_path;
use crate::error::{CliError, CliResult};

/// Retain prior files without copying large native outputs or modifying immutable aliases in place.
///
/// Artifact writers must replace files atomically. Small receipts are copied because completion callbacks may
/// rewrite them in place. This transaction covers returned errors and reuse misses, not crashes or concurrent readers.
pub(super) struct OutputPublication {
    backup: PathBuf,
    files: Vec<(PathBuf, Option<PathBuf>)>,
}

impl OutputPublication {
    /// Snapshot only the selected projection's files and replaceable metadata before publication starts.
    pub(super) fn begin(project: &Path, artifacts: Vec<PathBuf>, metadata: Vec<PathBuf>) -> CliResult<Self> {
        let root = fs::canonicalize(project).map_err(io_error)?;
        let mut paths = BTreeMap::new();
        for (path, copy) in artifacts
            .into_iter()
            .map(|path| (path, false))
            .chain(metadata.into_iter().map(|path| (path, true)))
        {
            let relative = path
                .strip_prefix(project)
                .map_err(|_| CliError::failure("publication file is outside its project"))?;
            let relative = validated_project_output_relative_path(&relative.to_string_lossy(), "publication file")?;
            let path = root.join(relative);
            validate_parent(&root, &path)?;
            paths
                .entry(path)
                .and_modify(|existing| *existing |= copy)
                .or_insert(copy);
        }
        let directory = root.join(".incan/oven");
        validate_parent(&root, &directory.join("publication"))?;
        fs::create_dir_all(&directory).map_err(io_error)?;
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let backup = loop {
            let candidate = directory.join(format!(
                ".output-publication-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            match fs::create_dir(&candidate) {
                Ok(()) => break candidate,
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(io_error(error)),
            }
        };
        let result = (|| {
            let mut files = Vec::new();
            for (index, (path, copy)) in paths.into_iter().enumerate() {
                let saved = match fs::symlink_metadata(&path) {
                    Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {
                        let saved = backup.join(format!("file-{index}"));
                        if copy {
                            fs::copy(&path, &saved).map_err(io_error)?;
                        } else {
                            link_or_copy(&path, &saved).map_err(io_error)?;
                        }
                        Some(saved)
                    }
                    Ok(_) => {
                        return Err(CliError::failure(format!(
                            "publication file {} is not a regular file",
                            path.display()
                        )));
                    }
                    Err(error) if error.kind() == io::ErrorKind::NotFound => None,
                    Err(error) => return Err(io_error(error)),
                };
                files.push((path, saved));
            }
            let bytes = serde_json::to_vec_pretty(&files).map_err(|error| CliError::failure(error.to_string()))?;
            fs::write(backup.join("files.json"), bytes).map_err(io_error)?;
            Ok(files)
        })();
        match result {
            Ok(files) => Ok(Self { backup, files }),
            Err(error) => {
                let _ = fs::remove_dir_all(&backup);
                Err(error)
            }
        }
    }

    /// Commit a successful replay, or atomically restore every prior file after an ordinary error.
    pub(super) fn finish<T>(self, result: CliResult<T>) -> CliResult<T> {
        let rollback = result.is_err();
        self.finish_result(result, rollback)
    }

    /// Apply the same recovery boundary to errors and late reuse misses without interpreting error messages.
    fn finish_result<T>(self, result: CliResult<T>, rollback: bool) -> CliResult<T> {
        if rollback {
            if let Err(rollback) = self.restore() {
                let cause = match &result {
                    Err(error) => error.to_string(),
                    Ok(_) => "completed output handoff was unavailable".to_string(),
                };
                return Err(CliError::failure(format!(
                    "{cause}; output rollback failed: {rollback}; recovery files retained at {}",
                    self.backup.display()
                )));
            }
        }
        if let Err(error) = fs::remove_dir_all(&self.backup) {
            eprintln!(
                "warning: unable to remove output publication backup {}: {error}",
                self.backup.display()
            );
        }
        result
    }

    /// A late authority miss must restore the caller generation before a fresh bake is allowed to start.
    pub(super) fn finish_reuse<T>(self, result: CliResult<Option<T>>) -> CliResult<Option<T>> {
        let rollback = !matches!(result, Ok(Some(_)));
        self.finish_result(result, rollback)
    }

    /// Keep recovery material until every restoration succeeds; never write into an existing native inode.
    fn restore(&self) -> CliResult<()> {
        for (index, (path, saved)) in self.files.iter().enumerate() {
            if let Some(saved) = saved {
                let parent = path
                    .parent()
                    .ok_or_else(|| CliError::failure("publication file has no parent"))?;
                fs::create_dir_all(parent).map_err(io_error)?;
                let staged = parent.join(format!(
                    ".output-restore-{}-{index}",
                    self.backup.file_name().unwrap_or_default().to_string_lossy()
                ));
                link_or_copy(saved, &staged).map_err(io_error)?;
                if let Err(error) = fs::rename(&staged, path) {
                    let _ = fs::remove_file(&staged);
                    return Err(io_error(error));
                }
            } else {
                match fs::remove_file(path) {
                    Ok(()) => {}
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                    Err(error) => return Err(io_error(error)),
                }
            }
        }
        Ok(())
    }
}

/// Refuse an existing parent that resolves outside the canonical project before creating or restoring files.
fn validate_parent(root: &Path, path: &Path) -> CliResult<()> {
    let mut parent = path
        .parent()
        .ok_or_else(|| CliError::failure("publication file has no parent"))?;
    loop {
        match fs::canonicalize(parent) {
            Ok(resolved) if resolved.starts_with(root) => return Ok(()),
            Ok(_) => return Err(CliError::failure("publication parent resolves outside its project")),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                parent = parent
                    .parent()
                    .ok_or_else(|| CliError::failure("publication parent is unavailable"))?;
            }
            Err(error) => return Err(io_error(error)),
        }
    }
}

/// Share preserved immutable bytes where supported, falling back to a bounded filesystem copy.
fn link_or_copy(source: &Path, destination: &Path) -> io::Result<()> {
    fs::hard_link(source, destination).or_else(|_| fs::copy(source, destination).map(|_| ()))
}

/// Preserve the I/O cause at the existing CLI error boundary.
fn io_error(error: io::Error) -> CliError {
    CliError::failure(format!("output publication failed: {error}"))
}
