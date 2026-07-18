//! Deterministic integrity identity for one relocatable compiled-provider artifact tree.

use std::fs;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

/// Failure while hashing a complete generated provider artifact.
#[derive(Debug, thiserror::Error)]
pub enum ProviderArtifactDigestError {
    /// The advertised artifact root is absent or not a directory.
    #[error("provider artifact root {path} is not a directory")]
    InvalidRoot { path: PathBuf },
    /// Reading or inspecting one artifact entry failed.
    #[error("failed to inspect provider artifact path {path}: {source}")]
    Io { path: PathBuf, source: std::io::Error },
    /// An entry could not be represented relative to its provider root.
    #[error("provider artifact path {path} is outside root {root}")]
    OutsideRoot { path: PathBuf, root: PathBuf },
    /// Published provider artifacts may not depend on symlinks or other special filesystem entries.
    #[error("provider artifact path {path} is not a regular file or directory")]
    UnsupportedEntry { path: PathBuf },
}

/// Hash every immutable manifest, generated source, and generated-project input in one provider artifact tree.
///
/// A nested Cargo `target/` directory is deliberately excluded because it is a mutable build cache rather than
/// provider content. Generated providers normally use an external shared target directory, but this exclusion keeps
/// integrity stable if a backend tool creates the conventional directory later.
pub fn digest_provider_artifact(root: &Path) -> Result<String, ProviderArtifactDigestError> {
    if !root.is_dir() {
        return Err(ProviderArtifactDigestError::InvalidRoot {
            path: root.to_path_buf(),
        });
    }
    let mut hasher = Sha256::new();
    hasher.update(b"incan-provider-artifact-v1\0");
    hash_directory(root, root, &mut hasher)?;
    Ok(format!("sha256:{}", hex::encode(hasher.finalize())))
}

fn hash_directory(root: &Path, directory: &Path, hasher: &mut Sha256) -> Result<(), ProviderArtifactDigestError> {
    let mut entries = fs::read_dir(directory)
        .map_err(|source| ProviderArtifactDigestError::Io {
            path: directory.to_path_buf(),
            source,
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|source| ProviderArtifactDigestError::Io {
            path: directory.to_path_buf(),
            source,
        })?;
    entries.sort_by_key(|entry| entry.file_name());

    for entry in entries {
        let path = entry.path();
        let relative = path
            .strip_prefix(root)
            .map_err(|_| ProviderArtifactDigestError::OutsideRoot {
                path: path.clone(),
                root: root.to_path_buf(),
            })?;
        if relative
            .components()
            .next()
            .is_some_and(|component| component.as_os_str() == "target")
        {
            continue;
        }
        let file_type = entry.file_type().map_err(|source| ProviderArtifactDigestError::Io {
            path: path.clone(),
            source,
        })?;
        hasher.update(relative.to_string_lossy().replace('\\', "/").as_bytes());
        hasher.update([0]);
        if file_type.is_dir() {
            hasher.update(b"directory\0");
            hash_directory(root, &path, hasher)?;
        } else if file_type.is_file() {
            hasher.update(b"file\0");
            let bytes = fs::read(&path).map_err(|source| ProviderArtifactDigestError::Io {
                path: path.clone(),
                source,
            })?;
            hasher.update(bytes);
        } else {
            return Err(ProviderArtifactDigestError::UnsupportedEntry { path });
        }
        hasher.update([0xff]);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    #[test]
    fn digest_tracks_manifest_and_generated_source_but_ignores_build_cache() -> TestResult {
        let artifact = tempfile::tempdir()?;
        fs::create_dir_all(artifact.path().join("src"))?;
        fs::write(artifact.path().join("provider.incnlib"), "manifest")?;
        fs::write(artifact.path().join("src/lib.rs"), "pub fn value() -> i32 { 1 }")?;
        let initial = digest_provider_artifact(artifact.path())?;

        fs::write(artifact.path().join("src/lib.rs"), "pub fn value() -> i32 { 2 }")?;
        let source_changed = digest_provider_artifact(artifact.path())?;
        assert_ne!(initial, source_changed);

        fs::create_dir_all(artifact.path().join("target/debug"))?;
        fs::write(artifact.path().join("target/debug/cache"), "mutable")?;
        assert_eq!(source_changed, digest_provider_artifact(artifact.path())?);
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn digest_rejects_symlinks() -> TestResult {
        use std::os::unix::fs::symlink;

        let artifact = tempfile::tempdir()?;
        fs::write(artifact.path().join("outside"), "content")?;
        symlink("outside", artifact.path().join("linked"))?;

        assert!(matches!(
            digest_provider_artifact(artifact.path()),
            Err(ProviderArtifactDigestError::UnsupportedEntry { .. })
        ));
        Ok(())
    }
}
