//! Ordinary-error rollback for a generated library artifact and its externally published receipts.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::cli::{CliError, CliResult};

/// One generated output generation, retained at the same absolute path while new source is compiled.
///
/// This is an ordinary-error transaction. It provides no concurrent-reader or process-crash atomicity guarantee.
pub(super) struct LibraryPublication {
    output: PathBuf,
    backup: PathBuf,
    previous: bool,
    receipts: Vec<(PathBuf, Option<Vec<u8>>)>,
    retain_package_cache: bool,
    replace_artifact: bool,
}

impl LibraryPublication {
    /// Preserve an existing generated artifact before any generator, compiler or receipt writer can change it.
    pub(super) fn begin(project: &Path, output: &Path, receipts: Vec<PathBuf>) -> CliResult<Self> {
        Self::begin_mode(project, output, receipts, true)
    }

    /// A current artifact needs only its external receipt captured, with no directory move or payload copy.
    pub(super) fn begin_receipt_update(project: &Path, receipts: Vec<PathBuf>) -> CliResult<Self> {
        Self::begin_mode(project, &project.join("target/lib"), receipts, false)
    }

    /// Capture recovery material for the precise caller projection that the operation can change.
    fn begin_mode(project: &Path, output: &Path, receipts: Vec<PathBuf>, replace_artifact: bool) -> CliResult<Self> {
        let project = fs::canonicalize(project).map_err(io_error)?;
        let parent = output
            .parent()
            .ok_or_else(|| CliError::failure("library output has no parent directory"))?;
        fs::create_dir_all(parent).map_err(io_error)?;
        let parent = fs::canonicalize(parent).map_err(io_error)?;
        let output = parent.join(
            output
                .file_name()
                .ok_or_else(|| CliError::failure("library output has no directory name"))?,
        );
        if output == project || project.starts_with(&output) || output == project.join("src") {
            return Err(CliError::failure(
                "library output must be a separate generated directory, outside authored source",
            ));
        }
        let metadata = match fs::symlink_metadata(&output) {
            Ok(metadata) => Some(metadata),
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => return Err(io_error(error)),
        };
        let previous = metadata.is_some() && replace_artifact;
        if let Some(metadata) = metadata {
            if !metadata.is_dir() || metadata.file_type().is_symlink() {
                return Err(CliError::failure(
                    "library output must be a regular generated directory",
                ));
            }
            if output != project.join("target/lib")
                && fs::read_dir(&output).map_err(io_error)?.next().is_some()
                && !owned_library_output(&output)?
            {
                return Err(CliError::failure(format!(
                    "existing custom output {} is not a generated Incan library; choose an empty directory or a previous library artifact",
                    output.display()
                )));
            }
        }
        let receipts = receipts
            .into_iter()
            .map(|path| {
                let bytes = match fs::read(&path) {
                    Ok(bytes) => Some(bytes),
                    Err(error) if error.kind() == io::ErrorKind::NotFound => None,
                    Err(error) => return Err(io_error(error)),
                };
                Ok((path, bytes))
            })
            .collect::<CliResult<Vec<_>>>()?;
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let backup = loop {
            let path = parent.join(format!(
                ".incan-library-publication-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            match fs::create_dir(&path) {
                Ok(()) => break path,
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(io_error(error)),
            }
        };
        // Keep recovery bytes on disk until every restoration succeeds, including receipts outside the output tree.
        let recovery = (|| -> io::Result<()> {
            let mut entries = Vec::new();
            for (index, (path, bytes)) in receipts.iter().enumerate() {
                let file = bytes.as_ref().map(|_| format!("receipt-{index}"));
                if let (Some(bytes), Some(file)) = (bytes, &file) {
                    fs::write(backup.join(file), bytes)?;
                }
                entries.push((path, file));
            }
            fs::write(
                backup.join("receipts.json"),
                serde_json::to_vec_pretty(&entries).map_err(io::Error::other)?,
            )
        })();
        if let Err(error) = recovery {
            let _ = fs::remove_dir_all(&backup);
            return Err(io_error(error));
        }
        if previous && let Err(error) = fs::rename(&output, backup.join("artifact")) {
            let _ = fs::remove_dir_all(&backup);
            return Err(io_error(error));
        }
        let publication = Self {
            output,
            backup,
            previous,
            receipts,
            retain_package_cache: false,
            replace_artifact,
        };
        if replace_artifact && let Err(error) = fs::create_dir(&publication.output) {
            return publication.finish::<Self>(Err(io_error(error)));
        }
        Ok(publication)
    }

    /// Normal replay keeps the prior immutable package cache when its selected output omits that portable store.
    pub(super) fn retaining_package_cache(mut self) -> Self {
        self.retain_package_cache = true;
        self
    }

    /// Complete the build or restore its entire prior generation, surfacing any failed restoration and backup path.
    pub(super) fn finish<T>(self, result: CliResult<T>) -> CliResult<T> {
        let result = result.and_then(|value| {
            if self.previous {
                // Retain unrelated custom-output files, while never resurrecting an obsolete generated module,
                // semantic surface, or native profile from a different successful generation.
                copy_unrelated(&self.backup.join("artifact"), &self.output).map_err(io_error)?;
                let previous_cache = self.backup.join("artifact/oven/loafs");
                let current_cache = self.output.join("oven/loafs");
                if self.retain_package_cache && previous_cache.exists() && !current_cache.exists() {
                    fs::create_dir_all(self.output.join("oven")).map_err(io_error)?;
                    // This is the last fallible commit step. A failed rename leaves the backup intact; success
                    // moves immutable cache bytes without duplicating them or reviving old native profiles.
                    fs::rename(previous_cache, current_cache).map_err(io_error)?;
                }
            }
            Ok(value)
        });
        match result {
            Ok(value) => {
                if let Err(error) = fs::remove_dir_all(&self.backup) {
                    eprintln!(
                        "warning: library was published; unable to remove prior artifact backup {}: {error}",
                        self.backup.display()
                    );
                }
                Ok(value)
            }
            Err(original) => {
                if let Err(error) = self.restore() {
                    return Err(CliError::failure(format!(
                        "{original}; library rollback failed: {error}. Inspect output {} and retained recovery directory {}",
                        self.output.display(),
                        self.backup.display()
                    )));
                }
                Err(original)
            }
        }
    }

    /// A cache miss after provisional materialization must restore the prior generation before a fresh bake starts.
    pub(super) fn finish_reuse<T>(self, result: CliResult<Option<T>>) -> CliResult<Option<T>> {
        if matches!(result, Ok(None)) {
            self.restore().map_err(|error| {
                CliError::failure(format!(
                    "library cache reuse was declined; library rollback failed: {error}. Inspect output {} and retained recovery directory {}",
                    self.output.display(),
                    self.backup.display()
                ))
            })?;
            Ok(None)
        } else {
            self.finish(result)
        }
    }

    /// Attempt artifact and receipt restoration independently, retaining all recovery material when any step fails.
    fn restore(&self) -> io::Result<()> {
        let mut errors = Vec::new();
        let artifact = (|| -> io::Result<()> {
            if !self.replace_artifact {
                return Ok(());
            }
            match fs::remove_dir_all(&self.output) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
            if self.previous {
                fs::rename(self.backup.join("artifact"), &self.output)?;
            }
            Ok(())
        })();
        if let Err(error) = artifact {
            errors.push(format!("artifact {}: {error}", self.output.display()));
        }
        for (path, bytes) in &self.receipts {
            let result = (|| -> io::Result<()> {
                match bytes {
                    Some(bytes) => {
                        if let Some(parent) = path.parent() {
                            fs::create_dir_all(parent)?;
                        }
                        fs::write(path, bytes)
                    }
                    None => match fs::remove_file(path) {
                        Ok(()) => Ok(()),
                        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
                        Err(error) => Err(error),
                    },
                }
            })();
            if let Err(error) = result {
                errors.push(format!("receipt {}: {error}", path.display()));
            }
        }
        if errors.is_empty() {
            fs::remove_dir_all(&self.backup)
        } else {
            Err(io::Error::other(errors.join("; ")))
        }
    }
}

/// A custom output must identify itself as a previous generated library before its directory can be moved.
fn owned_library_output(path: &Path) -> CliResult<bool> {
    if !path.join("Cargo.toml").is_file() || !path.join("src/lib.rs").is_file() {
        return Ok(false);
    }
    for entry in fs::read_dir(path).map_err(io_error)? {
        let entry = entry.map_err(io_error)?;
        if entry.path().extension().is_some_and(|extension| extension == "incnlib")
            && crate::library_manifest::LibraryManifest::read_from_path(&entry.path()).is_ok()
        {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Preserve only output-root entries outside the generator's artifact namespace.
fn copy_unrelated(source: &Path, destination: &Path) -> io::Result<()> {
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let name = entry.file_name();
        let path = PathBuf::from(&name);
        if matches!(
            name.to_str(),
            Some("Cargo.toml" | "Cargo.lock" | "src" | "semantic" | "oven" | "target")
        ) || path
            .extension()
            .is_some_and(|extension| extension == "incnlib" || extension == "incnsem")
        {
            continue;
        }
        let target = destination.join(&name);
        if entry.file_type()?.is_dir() {
            if !target.exists() {
                fs::create_dir(&target)?;
            }
            if fs::symlink_metadata(&target)?.file_type().is_dir() {
                copy_missing(&entry.path(), &target)?;
            }
        } else if fs::symlink_metadata(&target).is_err_and(|error| error.kind() == io::ErrorKind::NotFound) {
            if entry.file_type()?.is_symlink() {
                copy_symlink(&entry.path(), &target)?;
            } else {
                fs::copy(entry.path(), target)?;
            }
        }
    }
    Ok(())
}

/// Retain unrelated nested files without overwriting new contents.
fn copy_missing(source: &Path, destination: &Path) -> io::Result<()> {
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let target = destination.join(entry.file_name());
        let kind = entry.file_type()?;
        let target_kind = match fs::symlink_metadata(&target) {
            Ok(metadata) => Some(metadata.file_type()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => return Err(error),
        };
        if kind.is_dir() {
            if target_kind.is_some_and(|kind| !kind.is_dir()) {
                continue;
            }
            fs::create_dir_all(&target)?;
            copy_missing(&entry.path(), &target)?;
        } else if target_kind.is_none() {
            if kind.is_symlink() {
                copy_symlink(&entry.path(), &target)?;
            } else {
                fs::copy(entry.path(), target)?;
            }
        }
    }
    Ok(())
}

/// Preserve an unrelated symbolic link as a link, without reading or copying its target.
fn copy_symlink(source: &Path, target: &Path) -> io::Result<()> {
    let link = fs::read_link(source)?;
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(link, target)
    }
    #[cfg(windows)]
    {
        if source.is_dir() {
            std::os::windows::fs::symlink_dir(link, target)
        } else {
            std::os::windows::fs::symlink_file(link, target)
        }
    }
}

/// Keep filesystem context visible in the CLI's ordinary failure path.
fn io_error(error: io::Error) -> CliError {
    CliError::failure(format!("library publication: {error}"))
}

#[cfg(test)]
mod tests {
    use std::error::Error;
    use std::fs;

    use super::LibraryPublication;
    use crate::cli::CliError;

    /// A failure after overwriting one native profile restores Rust, metadata, stale files and external receipts.
    #[test]
    fn failed_rebuild_restores_the_complete_prior_generation() -> Result<(), Box<dyn Error>> {
        let temporary = tempfile::tempdir()?;
        let output = temporary.path().join("target/lib");
        let receipt = temporary.path().join(".incan/backend/receipt.json");
        fs::create_dir_all(output.join("src"))?;
        fs::create_dir_all(output.join("native"))?;
        fs::create_dir_all(output.join("oven/debug"))?;
        fs::create_dir_all(output.join("oven/release"))?;
        fs::create_dir_all(receipt.parent().ok_or("receipt parent missing")?)?;
        for relative in [
            "Cargo.toml",
            "src/lib.rs",
            "src/old.rs",
            "library.incnlib",
            "native/source-unit.json",
            "old.incnsem",
            "oven/debug/lib.rlib",
            "oven/release/lib.rlib",
        ] {
            fs::write(output.join(relative), format!("old {relative}"))?;
        }
        fs::write(&receipt, "old receipt")?;
        let publication = LibraryPublication::begin(temporary.path(), &output, vec![receipt.clone()])?;
        fs::create_dir_all(output.join("src"))?;
        fs::create_dir_all(output.join("native"))?;
        fs::create_dir_all(output.join("oven/debug"))?;
        fs::write(output.join("Cargo.toml"), "new cargo")?;
        fs::write(output.join("src/lib.rs"), "new generated source")?;
        fs::write(output.join("src/new.rs"), "new module")?;
        fs::write(output.join("oven/debug/lib.rlib"), "new debug native")?;
        fs::write(&receipt, "new receipt")?;
        super::super::publish_library_file(&output.join("native/source-unit.json"), b"new source definition")?;
        // Fail the real final file publication after the new source definition has already been written.
        fs::create_dir(output.join("library.incnlib"))?;
        let failure = super::super::publish_library_file(&output.join("library.incnlib"), b"new checked manifest");
        assert!(failure.is_err());
        let result = publication.finish(failure);
        assert!(result.is_err());
        for relative in [
            "Cargo.toml",
            "src/lib.rs",
            "src/old.rs",
            "library.incnlib",
            "native/source-unit.json",
            "old.incnsem",
            "oven/debug/lib.rlib",
            "oven/release/lib.rlib",
        ] {
            assert_eq!(fs::read_to_string(output.join(relative))?, format!("old {relative}"));
        }
        assert!(!output.join("src/new.rs").exists());
        assert_eq!(fs::read_to_string(receipt)?, "old receipt");
        Ok(())
    }

    /// A first failed build restores absence, while a successful rebuild retains unrelated custom-output files.
    #[test]
    fn absent_outputs_and_unrelated_custom_files_keep_their_prior_contract() -> Result<(), Box<dyn Error>> {
        let temporary = tempfile::tempdir()?;
        let output = temporary.path().join("custom");
        let publication = LibraryPublication::begin(temporary.path(), &output, vec![])?;
        fs::write(output.join("partial"), "partial")?;
        assert!(publication.finish::<()>(Err(CliError::failure("failed"))).is_err());
        assert!(!output.exists());
        fs::create_dir_all(output.join("src"))?;
        fs::write(output.join("Cargo.toml"), "old cargo")?;
        fs::write(output.join("src/lib.rs"), "old Rust")?;
        fs::write(output.join("notes.txt"), "user notes")?;
        fs::write(output.join("src/obsolete.rs"), "obsolete generated source")?;
        fs::create_dir_all(output.join("oven/release"))?;
        fs::write(output.join("oven/release/old.rlib"), "old native release")?;
        crate::library_manifest::LibraryManifest::new("library", "1.2.3")
            .write_to_path(&output.join("library.incnlib"))?;
        let publication = LibraryPublication::begin(temporary.path(), &output, vec![])?;
        fs::create_dir_all(output.join("src"))?;
        fs::write(output.join("src/lib.rs"), "new Rust")?;
        publication.finish(Ok(()))?;
        assert_eq!(fs::read_to_string(output.join("notes.txt"))?, "user notes");
        assert_eq!(fs::read_to_string(output.join("src/lib.rs"))?, "new Rust");
        assert!(!output.join("src/obsolete.rs").exists());
        assert!(!output.join("oven/release/old.rlib").exists());
        Ok(())
    }

    /// A declined replay restores the old artifact; an accepted normal replay retains only its immutable cache.
    #[test]
    fn completed_replay_restores_cache_misses_and_preserves_only_the_package_cache() -> Result<(), Box<dyn Error>> {
        let temporary = tempfile::tempdir()?;
        let output = temporary.path().join("target/lib");
        fs::create_dir_all(output.join("oven/loafs/entry"))?;
        fs::create_dir_all(output.join("oven/release"))?;
        fs::write(output.join("oven/loafs/entry/object"), "immutable cached bytes")?;
        fs::write(output.join("oven/release/old.rlib"), "old native bytes")?;
        let publication = LibraryPublication::begin(temporary.path(), &output, vec![])?;
        fs::write(output.join("new-manifest"), "provisional manifest")?;
        assert!(publication.finish_reuse::<()>(Ok(None))?.is_none());
        assert_eq!(
            fs::read_to_string(output.join("oven/release/old.rlib"))?,
            "old native bytes"
        );
        assert!(!output.join("new-manifest").exists());

        let publication = LibraryPublication::begin(temporary.path(), &output, vec![])?.retaining_package_cache();
        fs::write(output.join("new-manifest"), "selected manifest")?;
        assert!(
            publication
                .finish::<()>(Err(CliError::failure("report publication failed")))
                .is_err()
        );
        assert_eq!(
            fs::read_to_string(output.join("oven/loafs/entry/object"))?,
            "immutable cached bytes"
        );
        assert_eq!(
            fs::read_to_string(output.join("oven/release/old.rlib"))?,
            "old native bytes"
        );

        let publication = LibraryPublication::begin(temporary.path(), &output, vec![])?.retaining_package_cache();
        fs::write(output.join("oven"), "blocks cache destination directory")?;
        assert!(publication.finish(Ok(())).is_err());
        assert_eq!(
            fs::read_to_string(output.join("oven/loafs/entry/object"))?,
            "immutable cached bytes"
        );
        assert_eq!(
            fs::read_to_string(output.join("oven/release/old.rlib"))?,
            "old native bytes"
        );

        let publication = LibraryPublication::begin(temporary.path(), &output, vec![])?.retaining_package_cache();
        fs::write(output.join("new-manifest"), "selected manifest")?;
        publication.finish(Ok(()))?;
        assert_eq!(
            fs::read_to_string(output.join("oven/loafs/entry/object"))?,
            "immutable cached bytes"
        );
        assert!(!output.join("oven/release/old.rlib").exists());
        Ok(())
    }

    /// Repairing only a receipt keeps the verified artifact available and restores the receipt on a later error.
    #[test]
    fn receipt_only_replay_never_moves_the_current_artifact() -> Result<(), Box<dyn Error>> {
        let temporary = tempfile::tempdir()?;
        let output = temporary.path().join("target/lib");
        let receipt = temporary.path().join("receipt.json");
        fs::create_dir_all(&output)?;
        fs::write(output.join("native.rlib"), "current native bytes")?;
        fs::write(&receipt, "previous receipt")?;
        let publication = LibraryPublication::begin_receipt_update(temporary.path(), vec![receipt.clone()])?;
        assert_eq!(fs::read_to_string(output.join("native.rlib"))?, "current native bytes");
        assert!(!publication.backup.join("artifact").exists());
        fs::write(&receipt, "new receipt")?;
        assert!(
            publication
                .finish::<()>(Err(CliError::failure("report failed")))
                .is_err()
        );
        assert_eq!(fs::read_to_string(receipt)?, "previous receipt");
        assert_eq!(fs::read_to_string(output.join("native.rlib"))?, "current native bytes");
        Ok(())
    }

    /// A failed receipt restoration cannot prevent restoring the artifact or another independent receipt.
    #[test]
    fn receipt_failure_still_restores_artifact_and_retains_failed_recovery_bytes() -> Result<(), Box<dyn Error>> {
        let temporary = tempfile::tempdir()?;
        let output = temporary.path().join("target/lib");
        let failed = temporary.path().join("failed-receipt.json");
        let other = temporary.path().join("other-receipt.json");
        fs::create_dir_all(&output)?;
        fs::write(output.join("old"), "old artifact")?;
        fs::write(&failed, "old failed receipt")?;
        fs::write(&other, "old other receipt")?;
        let publication = LibraryPublication::begin(temporary.path(), &output, vec![failed.clone(), other.clone()])?;
        let backup = publication.backup.clone();
        fs::write(output.join("new"), "new artifact")?;
        fs::remove_file(&failed)?;
        fs::create_dir(&failed)?;
        fs::write(&other, "new receipt")?;
        let Err(error) = publication.finish::<()>(Err(CliError::failure("native failure"))) else {
            return Err("receipt restoration unexpectedly succeeded".into());
        };
        assert!(error.to_string().contains(&failed.display().to_string()));
        assert_eq!(fs::read_to_string(output.join("old"))?, "old artifact");
        assert!(!output.join("new").exists());
        assert_eq!(fs::read_to_string(&other)?, "old other receipt");
        assert_eq!(fs::read_to_string(backup.join("receipt-0"))?, "old failed receipt");
        assert!(backup.join("receipts.json").is_file());
        Ok(())
    }

    /// Failed recovery names retained recovery data instead of silently returning only the original build error.
    #[test]
    fn a_failed_restoration_keeps_the_backup_and_names_its_path() -> Result<(), Box<dyn Error>> {
        let temporary = tempfile::tempdir()?;
        let output = temporary.path().join("target/lib");
        fs::create_dir_all(&output)?;
        fs::write(output.join("old"), "old artifact")?;
        let publication = LibraryPublication::begin(temporary.path(), &output, vec![])?;
        let backup = publication.backup.clone();
        fs::remove_dir(&output)?;
        fs::write(&output, "obstruction")?;
        let Err(error) = publication.finish::<()>(Err(CliError::failure("original failure"))) else {
            return Err("rollback obstruction unexpectedly succeeded".into());
        };
        assert!(error.to_string().contains("rollback failed"));
        assert!(error.to_string().contains(&backup.display().to_string()));
        assert_eq!(fs::read_to_string(backup.join("artifact/old"))?, "old artifact");
        Ok(())
    }
}
