//! Materializing and verifying admitted compiler, search-path, and output foundations.

use super::{
    BTreeMap, BTreeSet, Command, Component, OvenRustcError, OvenRustcSupportingArtifact, Path, PathBuf,
    clear_inherited_cargo_environment, digest_bytes, fs, rustc_identity,
};

/// Derive the selected compiler's stable version identity and require an exact receipt match before compilation.
pub(super) fn verify_rustc_identity(rustc: &Path, expected: &str) -> Result<(), OvenRustcError> {
    let actual = rustc_identity(rustc)?;
    if actual == expected {
        return Ok(());
    }
    Err(OvenRustcError::ToolchainMismatch {
        expected: expected.to_string(),
        actual,
    })
}

/// Ask the user's own Rustup which tool its active toolchain resolves to.
///
/// This answers for the ambient configuration, honoring `RUSTUP_TOOLCHAIN` and any directory override exactly as a
/// hand-typed `rustup which` would. It is the fallback for development checkouts and installations without an
/// Incan-owned toolchain; installations that have one resolve through [`incan_owned_tool`] instead.
pub(super) fn rustup_reported_tool(tool: &str) -> Option<String> {
    let mut command = Command::new("rustup");
    command.args(["which", tool]);
    clear_inherited_cargo_environment(&mut command);
    let output = command.output().ok()?;
    if !output.status.success() {
        return None;
    }
    let reported = String::from_utf8(output.stdout).ok()?;
    let reported = reported.trim().to_string();
    (!reported.is_empty()).then_some(reported)
}

/// Verify declared rustc search directories and refuse any regular file not named in the manifest.
pub(super) fn materialize_search_paths(
    root: &Path,
    paths: &[String],
    kind: &'static str,
    expected: &BTreeMap<String, String>,
) -> Result<Vec<PathBuf>, OvenRustcError> {
    let mut materialized = Vec::new();
    let mut seen = BTreeSet::new();
    for relative in paths {
        let normalized = normalized_relative_path(relative, kind)?;
        if !seen.insert(normalized.clone()) {
            return Err(OvenRustcError::InvalidInput {
                field: "artifact manifest",
                message: format!("declares duplicate {kind} path `{relative}`"),
            });
        }
        let path = safe_path(root, &normalized, kind)?;
        let metadata = fs::symlink_metadata(&path).map_err(|source| OvenRustcError::Io {
            path: path.clone(),
            source,
        })?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(OvenRustcError::InvalidArtifactPath {
                kind,
                path,
                message: "must be a non-symlink directory".to_string(),
            });
        }
        let mut files_in_directory = 0_u64;
        for child in fs::read_dir(&path).map_err(|source| OvenRustcError::Io {
            path: path.clone(),
            source,
        })? {
            let child = child.map_err(|source| OvenRustcError::Io {
                path: path.clone(),
                source,
            })?;
            let child_path = child.path();
            let child_metadata = fs::symlink_metadata(&child_path).map_err(|source| OvenRustcError::Io {
                path: child_path.clone(),
                source,
            })?;
            if !child_metadata.is_file() || child_metadata.file_type().is_symlink() {
                return Err(OvenRustcError::InvalidArtifactPath {
                    kind,
                    path: child_path,
                    message: "search directories may contain only regular files".to_string(),
                });
            }
            let relative_child = child_path
                .strip_prefix(root)
                .map_err(|_| OvenRustcError::InvalidArtifactPath {
                    kind,
                    path: child_path.clone(),
                    message: "resolved path escaped artifact root".to_string(),
                })?
                .to_string_lossy()
                .replace('\\', "/");
            let digest = expected
                .get(&relative_child)
                .ok_or_else(|| OvenRustcError::UnrecordedSearchArtifact {
                    path: child_path.clone(),
                })?;
            let actual = digest_bytes(&fs::read(&child_path).map_err(|source| OvenRustcError::Io {
                path: child_path.clone(),
                source,
            })?);
            if digest != &actual {
                return Err(OvenRustcError::ArtifactDigestMismatch {
                    path: child_path,
                    expected: digest.clone(),
                    actual,
                });
            }
            files_in_directory = files_in_directory.saturating_add(1);
        }
        if files_in_directory == 0 {
            return Err(OvenRustcError::InvalidArtifactPath {
                kind,
                path,
                message: "must contain at least one manifest-recorded regular file".to_string(),
            });
        }
        materialized.push(path);
    }
    Ok(materialized)
}

/// Validate one complete admitted inventory without reading its files or widening the selected artifact set.
pub(super) fn admitted_search_inventory(
    artifacts: &[OvenRustcSupportingArtifact],
    expected: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, String>, OvenRustcError> {
    let mut complete = BTreeMap::new();
    for artifact in artifacts {
        let relative = normalized_relative_path(&artifact.relative_path, "leased root inventory")?;
        if expected.get(&relative).is_some_and(|digest| digest != &artifact.digest) {
            return Err(OvenRustcError::InvalidInput {
                field: "composed artifact roots",
                message: format!("admitted search inventory conflicts with selected artifact `{relative}`"),
            });
        }
        if complete.insert(relative, artifact.digest.clone()).is_some() {
            return Err(OvenRustcError::InvalidInput {
                field: "composed artifact roots",
                message: "complete root inventory repeats an artifact".to_string(),
            });
        }
    }
    Ok(complete)
}

/// Whether a trusted materialization makes its per-file shape checks or relies on a written closure proof.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TrustedShape {
    /// Stat every declared file and search directory: present, regular or directory, never a symlink.
    Checked,
    /// A proof for this exact closure exists; normalize and contain paths, but do not stat each one.
    Proven,
}

/// Verify a selected store-owned search directory without repeating publisher-time closure enumeration.
///
/// Publisher-time materialization verifies every child and its digest. A normal consumer proves the selected
/// directory itself is non-symlinked and contains at least one declared artifact, then separately validates every
/// manifest-declared file it can hand to Rustc. Re-enumerating every unrelated child here would make prepared
/// direct-Rustc selection behave like a cold whole-closure audit.
pub(super) fn trusted_materialize_search_paths(
    root: &Path,
    paths: &[String],
    kind: &'static str,
    expected: &BTreeMap<String, String>,
    trusted_parents: &mut BTreeMap<PathBuf, PathBuf>,
    shape: TrustedShape,
) -> Result<Vec<PathBuf>, OvenRustcError> {
    let mut materialized = Vec::new();
    let mut seen = BTreeSet::new();
    for relative in paths {
        let normalized = normalized_relative_path(relative, kind)?;
        if !seen.insert(normalized.clone()) {
            return Err(OvenRustcError::InvalidInput {
                field: "artifact manifest",
                message: format!("declares duplicate {kind} path `{relative}`"),
            });
        }
        let path = trusted_safe_path(root, &normalized, kind, trusted_parents)?;
        if shape == TrustedShape::Checked {
            let metadata = fs::symlink_metadata(&path).map_err(|source| OvenRustcError::Io {
                path: path.clone(),
                source,
            })?;
            if !metadata.is_dir() || metadata.file_type().is_symlink() {
                return Err(OvenRustcError::InvalidArtifactPath {
                    kind,
                    path,
                    message: "must be a non-symlink directory".to_string(),
                });
            }
        }
        if !expected
            .keys()
            .any(|artifact| Path::new(artifact).starts_with(Path::new(&normalized)))
        {
            return Err(OvenRustcError::InvalidArtifactPath {
                kind,
                path,
                message: "must contain at least one manifest-recorded regular file".to_string(),
            });
        }
        materialized.push(path);
    }
    Ok(materialized)
}

/// Verify one manifest artifact file and return its canonical artifact-root-contained path.
pub(super) fn verified_file(
    root: &Path,
    relative: &str,
    expected_digest: &str,
    kind: &'static str,
) -> Result<PathBuf, OvenRustcError> {
    let relative = normalized_relative_path(relative, kind)?;
    let path = safe_path(root, &relative, kind)?;
    let metadata = fs::symlink_metadata(&path).map_err(|source| OvenRustcError::Io {
        path: path.clone(),
        source,
    })?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(OvenRustcError::InvalidArtifactPath {
            kind,
            path,
            message: "must be a non-symlink regular file".to_string(),
        });
    }
    let actual = digest_bytes(&fs::read(&path).map_err(|source| OvenRustcError::Io {
        path: path.clone(),
        source,
    })?);
    if expected_digest != actual {
        return Err(OvenRustcError::ArtifactDigestMismatch {
            path,
            expected: expected_digest.to_string(),
            actual,
        });
    }
    Ok(path)
}

/// Return a safe regular store artifact without repeating its publisher-verified content digest.
///
/// Under [`TrustedShape::Proven`] the path is still normalized and contained below a canonical parent, but the
/// file itself is not stated: a written proof says this closure's files were all checked once already.
pub(super) fn trusted_file(
    root: &Path,
    relative: &str,
    kind: &'static str,
    trusted_parents: &mut BTreeMap<PathBuf, PathBuf>,
    shape: TrustedShape,
) -> Result<PathBuf, OvenRustcError> {
    let relative = normalized_relative_path(relative, kind)?;
    if shape == TrustedShape::Proven {
        // The proof covered this file's parent too; re-canonicalizing thousands of source directories is the
        // other half of the walk the proof exists to retire.
        return Ok(root.join(relative));
    }
    let path = trusted_safe_path(root, &relative, kind, trusted_parents)?;
    let metadata = fs::symlink_metadata(&path).map_err(|source| OvenRustcError::Io {
        path: path.clone(),
        source,
    })?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(OvenRustcError::InvalidArtifactPath {
            kind,
            path,
            message: "must be a non-symlink regular file".to_string(),
        });
    }
    Ok(path)
}

/// Resolve a selected immutable path while canonicalizing each parent directory at most once per plan.
///
/// The generation lock and read-only publisher-owned root make every checked parent stable for the current
/// selection. We still verify that each distinct parent remains beneath the canonical artifact root and validate the
/// requested child as a regular non-symlink file in [`trusted_file`]. This avoids repeating the same filesystem walk
/// for every extern stored below one `deps` directory.
pub(super) fn trusted_safe_path(
    root: &Path,
    relative: &str,
    kind: &'static str,
    trusted_parents: &mut BTreeMap<PathBuf, PathBuf>,
) -> Result<PathBuf, OvenRustcError> {
    let path = root.join(relative);
    let parent = path.parent().ok_or_else(|| OvenRustcError::InvalidArtifactPath {
        kind,
        path: path.clone(),
        message: "has no parent directory".to_string(),
    })?;
    let canonical_parent = if let Some(parent) = trusted_parents.get(parent) {
        parent.clone()
    } else {
        let canonical_parent = parent.canonicalize().map_err(|source| OvenRustcError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
        trusted_parents.insert(parent.to_path_buf(), canonical_parent.clone());
        canonical_parent
    };
    if !canonical_parent.starts_with(root) {
        return Err(OvenRustcError::InvalidArtifactPath {
            kind,
            path,
            message: "escapes immutable artifact root".to_string(),
        });
    }
    Ok(path)
}

/// Canonicalize an existing artifact root and reject an absent or non-directory root.
pub(super) fn canonical_directory(path: &Path, kind: &'static str) -> Result<PathBuf, OvenRustcError> {
    let metadata = fs::symlink_metadata(path).map_err(|source| OvenRustcError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(OvenRustcError::InvalidArtifactPath {
            kind,
            path: path.to_path_buf(),
            message: "must be a non-symlink directory".to_string(),
        });
    }
    path.canonicalize().map_err(|source| OvenRustcError::Io {
        path: path.to_path_buf(),
        source,
    })
}

/// Resolve a normalized relative path and prove that its canonical parent remains below the immutable root.
pub(super) fn safe_path(root: &Path, relative: &str, kind: &'static str) -> Result<PathBuf, OvenRustcError> {
    let path = root.join(relative);
    let parent = path.parent().ok_or_else(|| OvenRustcError::InvalidArtifactPath {
        kind,
        path: path.clone(),
        message: "has no parent directory".to_string(),
    })?;
    let canonical_parent = parent.canonicalize().map_err(|source| OvenRustcError::Io {
        path: parent.to_path_buf(),
        source,
    })?;
    if !canonical_parent.starts_with(root) {
        return Err(OvenRustcError::InvalidArtifactPath {
            kind,
            path,
            message: "escapes immutable artifact root".to_string(),
        });
    }
    Ok(path)
}

/// Normalize one relative artifact path and reject absolute, parent, or platform-prefix components.
pub(super) fn normalized_relative_path(value: &str, kind: &'static str) -> Result<String, OvenRustcError> {
    let path = Path::new(value);
    if value.trim().is_empty()
        || path.components().any(|component| {
            matches!(
                component,
                Component::Prefix(_) | Component::RootDir | Component::ParentDir | Component::CurDir
            )
        })
    {
        return Err(OvenRustcError::InvalidArtifactPath {
            kind,
            path: PathBuf::from(value),
            message: "must be a non-empty normalized relative path".to_string(),
        });
    }
    Ok(path.to_string_lossy().replace('\\', "/"))
}

/// Validate that a direct-rustc source or output is a non-symlink regular file.
pub(super) fn verified_regular_file(path: &Path, kind: &'static str) -> Result<PathBuf, OvenRustcError> {
    let metadata = fs::symlink_metadata(path).map_err(|source| OvenRustcError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(OvenRustcError::InvalidArtifactPath {
            kind,
            path: path.to_path_buf(),
            message: "must be a non-symlink regular file".to_string(),
        });
    }
    Ok(path.to_path_buf())
}

/// Verify a caller-owned path and observe its content digest without rereading unchanged executable bytes.
///
/// Persistent acceleration is bound to the exact held file's replacement-sensitive metadata. This does not replace
/// receipt, ownership or invocation-input validation at the caller.
pub(super) fn digest_regular_file(path: &Path, kind: &'static str) -> Result<String, OvenRustcError> {
    let path = verified_regular_file(path, kind)?;
    let (_, digest) = oven_store::store::digest_regular_file(&path)?;
    Ok(digest)
}

#[cfg(test)]
mod digest_tests {
    use super::*;

    /// Cached output observations still reject symlink substitutions and detect preserved-mtime edits.
    #[test]
    fn caller_file_digest_preserves_path_admission() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let output = root.path().join("output");
        fs::write(&output, b"native bytes")?;
        assert_eq!(
            digest_regular_file(&output, "test output")?,
            digest_bytes(b"native bytes")
        );
        assert_eq!(
            digest_regular_file(&output, "test output")?,
            digest_bytes(b"native bytes")
        );
        let modified = fs::metadata(&output)?.modified()?;
        fs::write(&output, b"edited bytes")?;
        fs::File::options()
            .write(true)
            .open(&output)?
            .set_times(fs::FileTimes::new().set_modified(modified))?;
        assert_eq!(
            digest_regular_file(&output, "test output")?,
            digest_bytes(b"edited bytes")
        );
        assert!(digest_regular_file(root.path(), "test output").is_err());
        #[cfg(unix)]
        {
            let link = root.path().join("link");
            std::os::unix::fs::symlink(&output, &link)?;
            assert!(matches!(
                digest_regular_file(&link, "test output"),
                Err(OvenRustcError::InvalidArtifactPath { .. })
            ));
        }
        Ok(())
    }
}

/// Ensure the caller-owned final output cannot become part of the immutable artifact root.
pub(super) fn caller_output_path(output: &Path, artifact_root: &Path) -> Result<PathBuf, OvenRustcError> {
    if output.as_os_str().is_empty() {
        return Err(OvenRustcError::InvalidInput {
            field: "output",
            message: "must not be empty".to_string(),
        });
    }
    let artifact_root = artifact_root.canonicalize().map_err(|source| OvenRustcError::Io {
        path: artifact_root.to_path_buf(),
        source,
    })?;
    let output_parent = output.parent().ok_or_else(|| OvenRustcError::InvalidInput {
        field: "output",
        message: "must have a parent directory".to_string(),
    })?;
    fs::create_dir_all(output_parent).map_err(|source| OvenRustcError::Io {
        path: output_parent.to_path_buf(),
        source,
    })?;
    let output_parent = output_parent.canonicalize().map_err(|source| OvenRustcError::Io {
        path: output_parent.to_path_buf(),
        source,
    })?;
    let resolved = output_parent.join(output.file_name().ok_or_else(|| OvenRustcError::InvalidInput {
        field: "output",
        message: "must name a file".to_string(),
    })?);
    if resolved.starts_with(&artifact_root) {
        return Err(OvenRustcError::InvalidInput {
            field: "output",
            message: "must remain outside the immutable artifact root".to_string(),
        });
    }
    Ok(resolved)
}

/// Create a caller-owned regular temporary directory without allowing Rustdoc to write into an immutable store entry.
pub(super) fn caller_temporary_directory(directory: &Path, artifact_root: &Path) -> Result<PathBuf, OvenRustcError> {
    if directory.as_os_str().is_empty() {
        return Err(OvenRustcError::InvalidInput {
            field: "Rustdoc temporary directory",
            message: "must not be empty".to_string(),
        });
    }
    let artifact_root = artifact_root.canonicalize().map_err(|source| OvenRustcError::Io {
        path: artifact_root.to_path_buf(),
        source,
    })?;
    fs::create_dir_all(directory).map_err(|source| OvenRustcError::Io {
        path: directory.to_path_buf(),
        source,
    })?;
    let metadata = fs::symlink_metadata(directory).map_err(|source| OvenRustcError::Io {
        path: directory.to_path_buf(),
        source,
    })?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(OvenRustcError::InvalidArtifactPath {
            kind: "Rustdoc temporary directory",
            path: directory.to_path_buf(),
            message: "must be a non-symlink directory".to_string(),
        });
    }
    let directory = directory.canonicalize().map_err(|source| OvenRustcError::Io {
        path: directory.to_path_buf(),
        source,
    })?;
    if directory.starts_with(&artifact_root) {
        return Err(OvenRustcError::InvalidInput {
            field: "Rustdoc temporary directory",
            message: "must remain outside the immutable artifact root".to_string(),
        });
    }
    Ok(directory)
}

/// Preserve both direct-tool streams for a single actionable Rustdoc failure report.
pub(super) fn combined_process_output(stdout: &[u8], stderr: &[u8]) -> String {
    format!("{}{}", String::from_utf8_lossy(stdout), String::from_utf8_lossy(stderr))
}

/// Validate the narrow Rust identifier syntax accepted for a direct test crate name.
pub(super) fn validate_rust_identifier(name: &str) -> Result<(), OvenRustcError> {
    let mut characters = name.chars();
    let Some(first) = characters.next() else {
        return Err(OvenRustcError::InvalidInput {
            field: "crate name",
            message: "must not be empty".to_string(),
        });
    };
    if !(first == '_' || first.is_ascii_alphabetic())
        || !characters.all(|character| character == '_' || character.is_ascii_alphanumeric())
    {
        return Err(OvenRustcError::InvalidInput {
            field: "crate name",
            message: "must use ASCII Rust identifier characters".to_string(),
        });
    }
    Ok(())
}

/// Validate an explicit Rust target triple without accepting command-line syntax or path components.
pub(super) fn validate_rust_target(target: &str) -> Result<(), OvenRustcError> {
    if target.is_empty()
        || !target
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '-' | '.'))
    {
        return Err(OvenRustcError::InvalidInput {
            field: "vocabulary auxiliary Rust target",
            message: "must use only ASCII target-triple characters".to_string(),
        });
    }
    Ok(())
}

/// Validate Rust editions accepted by the pinned compiler for adopted Loaf sources.
pub(super) fn validate_edition(edition: &str) -> Result<(), OvenRustcError> {
    if matches!(edition, "2015" | "2018" | "2021" | "2024") {
        return Ok(());
    }
    Err(OvenRustcError::InvalidInput {
        field: "edition",
        message: "must be one of 2015, 2018, 2021 or 2024".to_string(),
    })
}
