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
}

impl LibraryPublication {
    /// Preserve an existing generated artifact before any generator, compiler or receipt writer can change it.
    pub(super) fn begin(project: &Path, output: &Path, receipts: Vec<PathBuf>) -> CliResult<Self> {
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
        let previous = metadata.is_some();
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
        if previous && let Err(error) = fs::rename(&output, backup.join("artifact")) {
            let _ = fs::remove_dir(&backup);
            return Err(io_error(error));
        }
        let publication = Self {
            output,
            backup,
            previous,
            receipts,
        };
        if let Err(error) = fs::create_dir(&publication.output) {
            return publication.finish::<Self>(Err(io_error(error)));
        }
        Ok(publication)
    }

    /// Complete the build or restore its entire prior generation, surfacing any failed restoration and backup path.
    pub(super) fn finish<T>(self, result: CliResult<T>) -> CliResult<T> {
        let result = result.and_then(|value| {
            if self.previous {
                // Retain unrelated custom-output files, while never resurrecting an obsolete generated module,
                // semantic surface, or native profile from a different successful generation.
                copy_unrelated(&self.backup.join("artifact"), &self.output).map_err(io_error)?;
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

    /// Restore only this transaction's output tree and the receipt paths captured before preparation.
    fn restore(&self) -> io::Result<()> {
        for (path, bytes) in &self.receipts {
            match bytes {
                Some(bytes) => {
                    if let Some(parent) = path.parent() {
                        fs::create_dir_all(parent)?;
                    }
                    fs::write(path, bytes)?;
                }
                None => match fs::remove_file(path) {
                    Ok(()) => {}
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error),
                },
            }
        }
        match fs::remove_dir_all(&self.output) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        if self.previous {
            fs::rename(self.backup.join("artifact"), &self.output)?;
        }
        fs::remove_dir_all(&self.backup)
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
        fs::create_dir_all(output.join("oven/debug"))?;
        fs::create_dir_all(output.join("oven/release"))?;
        fs::create_dir_all(receipt.parent().ok_or("receipt parent missing")?)?;
        for relative in [
            "Cargo.toml",
            "src/lib.rs",
            "src/old.rs",
            "library.incnlib",
            "old.incnsem",
            "oven/debug/lib.rlib",
            "oven/release/lib.rlib",
        ] {
            fs::write(output.join(relative), format!("old {relative}"))?;
        }
        fs::write(&receipt, "old receipt")?;
        let publication = LibraryPublication::begin(temporary.path(), &output, vec![receipt.clone()])?;
        fs::create_dir_all(output.join("src"))?;
        fs::create_dir_all(output.join("oven/debug"))?;
        fs::write(output.join("Cargo.toml"), "new cargo")?;
        fs::write(output.join("src/lib.rs"), "new generated source")?;
        fs::write(output.join("src/new.rs"), "new module")?;
        fs::write(output.join("oven/debug/lib.rlib"), "new debug native")?;
        fs::write(&receipt, "new receipt")?;
        let result = publication.finish::<()>(Err(CliError::failure("release compilation failed")));
        assert!(result.is_err());
        for relative in [
            "Cargo.toml",
            "src/lib.rs",
            "src/old.rs",
            "library.incnlib",
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
