//! Deterministic integrity identity for one relocatable compiled-provider artifact tree.

use std::collections::BTreeMap;
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
    /// An authored provider input is missing or is not a regular file.
    #[error("provider source input {path} is not a regular file")]
    InvalidSourceInput { path: PathBuf },
    /// An authored source resolved outside both its package and compiler-owned shared source roots.
    #[error("provider source input {path} is outside its package and trusted toolchain source roots")]
    OutsideSourceRoots { path: PathBuf },
    /// An authored source input could not be normalized into the semantic identity projection.
    #[error("failed to normalize provider source input {path}: {message}")]
    Normalization { path: PathBuf, message: String },
}

/// Hash every immutable manifest, generated source, and generated-project input in one provider artifact tree.
///
/// Compiler, VCS, and test-runner output directories are deliberately excluded because they are mutable caches
/// rather than provider content. Generated providers normally use an external shared target directory, but these
/// exclusions keep integrity stable if a backend tool creates conventional local output later.
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

/// Hash the authored manifest and logically named Incan source modules that define one compiled provider's semantics.
///
/// The resulting identity deliberately precedes generated Rust and host-derived ABI extraction. It is one input to
/// the checked Oven-native semantic projection, while [`digest_provider_artifact`] remains the byte-exact integrity
/// check for each physical artifact. Logical module labels keep shared toolchain source outside the package directory
/// relocation-stable without excluding it from the semantic identity.
pub(crate) fn digest_provider_source_inputs(
    project_root: &Path,
    manifest_path: &Path,
    source_inputs: &[(String, PathBuf)],
    trusted_source_roots: &[PathBuf],
) -> Result<String, ProviderArtifactDigestError> {
    if !project_root.is_dir() {
        return Err(ProviderArtifactDigestError::InvalidRoot {
            path: project_root.to_path_buf(),
        });
    }
    let resolve = |path: &Path| {
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            project_root.join(path)
        }
    };
    let canonical_project_root = fs::canonicalize(project_root).map_err(|source| ProviderArtifactDigestError::Io {
        path: project_root.to_path_buf(),
        source,
    })?;
    let mut allowed_source_roots = vec![canonical_project_root.clone()];
    for root in trusted_source_roots {
        if !root.is_dir() {
            return Err(ProviderArtifactDigestError::InvalidRoot { path: root.clone() });
        }
        let canonical = fs::canonicalize(root).map_err(|source| ProviderArtifactDigestError::Io {
            path: root.clone(),
            source,
        })?;
        if !allowed_source_roots.contains(&canonical) {
            allowed_source_roots.push(canonical);
        }
    }
    let canonical_source_file = |path: PathBuf| {
        let metadata = fs::symlink_metadata(&path).map_err(|source| {
            if source.kind() == std::io::ErrorKind::NotFound {
                ProviderArtifactDigestError::InvalidSourceInput { path: path.clone() }
            } else {
                ProviderArtifactDigestError::Io {
                    path: path.clone(),
                    source,
                }
            }
        })?;
        if !metadata.file_type().is_file() {
            return Err(ProviderArtifactDigestError::InvalidSourceInput { path });
        }
        fs::canonicalize(&path).map_err(|source| ProviderArtifactDigestError::Io { path, source })
    };
    let manifest_path = canonical_source_file(resolve(manifest_path))?;
    let manifest_label = manifest_path
        .strip_prefix(&canonical_project_root)
        .map_err(|_| ProviderArtifactDigestError::OutsideRoot {
            path: manifest_path.clone(),
            root: canonical_project_root.clone(),
        })?
        .to_string_lossy()
        .replace('\\', "/");
    let mut inputs = BTreeMap::from([(format!("manifest:{manifest_label}"), manifest_path)]);
    for (module_label, path) in source_inputs {
        let path = canonical_source_file(resolve(path))?;
        if !allowed_source_roots.iter().any(|root| path.starts_with(root)) {
            return Err(ProviderArtifactDigestError::OutsideSourceRoots { path });
        }
        let label = format!("module:{module_label}");
        if module_label.is_empty() || inputs.insert(label.clone(), path.clone()).is_some() {
            return Err(ProviderArtifactDigestError::Normalization {
                path,
                message: format!("provider source input has invalid or duplicate logical label `{label}`"),
            });
        }
    }

    let mut hasher = Sha256::new();
    hasher.update(b"incan-provider-source-inputs-v1\0");
    for (relative, path) in inputs {
        let bytes = fs::read(&path).map_err(|source| ProviderArtifactDigestError::Io { path, source })?;
        hash_named_bytes(&mut hasher, &relative, &bytes);
    }
    Ok(format!("sha256:{}", hex::encode(hasher.finalize())))
}

/// Feed one named byte payload into a delimiter-safe semantic digest stream.
fn hash_named_bytes(hasher: &mut Sha256, name: &str, bytes: &[u8]) {
    hasher.update(name.as_bytes());
    hasher.update([0]);
    hasher.update(bytes);
    hasher.update([0xff]);
}

/// Feed one artifact directory into the stable digest in lexical path order while excluding mutable output trees.
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
        let file_name = path.file_name().and_then(|name| name.to_str());
        let file_type = entry.file_type().map_err(|source| ProviderArtifactDigestError::Io {
            path: path.clone(),
            source,
        })?;
        if file_type.is_dir() {
            let is_mutable_output = matches!(file_name, Some(".git" | ".incan" | ".ralph-cache" | "target"));
            // v0.5 providers briefly placed the compiler-owned Rust-inspection Cargo target below the published
            // `oven/` directory. It is mutable preparation state, not provider content. Exclude the legacy location
            // so an existing generated provider remains loadable while current builders place it under `target/`.
            let is_legacy_rust_inspect_output = relative == Path::new("oven/rust-inspect");
            if is_mutable_output || is_legacy_rust_inspect_output {
                continue;
            }
        }
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
    fn digest_tracks_manifest_and_generated_source_but_ignores_mutable_output() -> TestResult {
        let artifact = tempfile::tempdir()?;
        fs::create_dir_all(artifact.path().join("src"))?;
        fs::write(artifact.path().join("provider.incnlib"), "manifest")?;
        fs::write(artifact.path().join("src/lib.rs"), "pub fn value() -> i32 { 1 }")?;
        let initial = digest_provider_artifact(artifact.path())?;

        fs::write(artifact.path().join("src/lib.rs"), "pub fn value() -> i32 { 2 }")?;
        let source_changed = digest_provider_artifact(artifact.path())?;
        assert_ne!(initial, source_changed);

        fs::write(artifact.path().join("target"), "authored provider content")?;
        assert_ne!(source_changed, digest_provider_artifact(artifact.path())?);
        fs::remove_file(artifact.path().join("target"))?;

        for directory in [".git", ".incan/oven", ".ralph-cache/loafs", "target/debug"] {
            fs::create_dir_all(artifact.path().join(directory))?;
            fs::write(artifact.path().join(directory).join("mutable"), "not provider content")?;
        }
        assert_eq!(source_changed, digest_provider_artifact(artifact.path())?);
        Ok(())
    }

    #[test]
    fn authored_provider_source_digest_is_relocation_stable_and_tracks_inputs_issue931() -> TestResult {
        let first = tempfile::tempdir()?;
        let second = tempfile::tempdir()?;
        for workspace in [first.path(), second.path()] {
            let root = workspace.join("component");
            fs::create_dir_all(root.join("src"))?;
            fs::create_dir_all(workspace.join("shared"))?;
            fs::write(
                root.join("loaf.toml"),
                "[project]\nname = \"provider\"\nversion = \"0.1.0\"\n",
            )?;
            fs::write(root.join("src/lib.incn"), "pub def value() -> int:\n    return 1\n")?;
            fs::write(
                workspace.join("shared/item.incn"),
                "pub const LABEL: str = \"stable\"\n",
            )?;
        }
        let source_inputs = |workspace: &Path| {
            vec![
                ("<root>".to_string(), workspace.join("component/src/lib.incn")),
                ("shared.item".to_string(), workspace.join("shared/item.incn")),
            ]
        };
        let first_root = first.path().join("component");
        let second_root = second.path().join("component");
        let first_digest = digest_provider_source_inputs(
            &first_root,
            &first_root.join("loaf.toml"),
            &source_inputs(first.path()),
            &[first.path().join("shared")],
        )?;
        let second_digest = digest_provider_source_inputs(
            &second_root,
            &second_root.join("loaf.toml"),
            &source_inputs(second.path()),
            &[second.path().join("shared")],
        )?;
        assert_eq!(first_digest, second_digest);

        fs::write(
            second.path().join("shared/item.incn"),
            "pub const LABEL: str = \"changed\"\n",
        )?;
        assert_ne!(
            first_digest,
            digest_provider_source_inputs(
                &second_root,
                &second_root.join("loaf.toml"),
                &source_inputs(second.path()),
                &[second.path().join("shared")],
            )?
        );

        let outside = second.path().join("untrusted.incn");
        fs::write(&outside, "pub const LABEL: str = \"outside\"\n")?;
        let error = digest_provider_source_inputs(
            &second_root,
            &second_root.join("loaf.toml"),
            &[("outside".to_string(), outside)],
            &[],
        )
        .err()
        .ok_or("expected untrusted outside-root source input to fail")?;
        assert!(matches!(error, ProviderArtifactDigestError::OutsideSourceRoots { .. }));
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn authored_provider_source_digest_rejects_symlink_inputs_issue931() -> TestResult {
        use std::os::unix::fs::symlink;

        let workspace = tempfile::tempdir()?;
        let root = workspace.path().join("component");
        fs::create_dir_all(root.join("src"))?;
        fs::write(root.join("loaf.toml"), "[project]\nname = \"provider\"\n")?;
        fs::write(root.join("src/real.incn"), "pub const LABEL: str = \"real\"\n")?;
        symlink(root.join("src/real.incn"), root.join("src/link.incn"))?;

        let error = digest_provider_source_inputs(
            &root,
            &root.join("loaf.toml"),
            &[("link".to_string(), root.join("src/link.incn"))],
            &[],
        )
        .err()
        .ok_or("expected symlinked provider source input to fail")?;
        assert!(matches!(error, ProviderArtifactDigestError::InvalidSourceInput { .. }));
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

    #[cfg(unix)]
    #[test]
    fn digest_ignores_legacy_compiler_owned_rust_inspect_output() -> TestResult {
        use std::os::unix::fs::symlink;

        let artifact = tempfile::tempdir()?;
        fs::write(artifact.path().join("provider.incnlib"), "manifest")?;
        fs::create_dir_all(artifact.path().join("oven/debug"))?;
        fs::write(artifact.path().join("oven/debug/libprovider.rlib"), "provider")?;
        let initial = digest_provider_artifact(artifact.path())?;

        let inspection_output = artifact.path().join("oven/rust-inspect/debug/build/tool/out/bin");
        fs::create_dir_all(&inspection_output)?;
        fs::write(inspection_output.join("tool-1"), "compiler-owned output")?;
        symlink("tool-1", inspection_output.join("tool"))?;

        assert_eq!(initial, digest_provider_artifact(artifact.path())?);
        Ok(())
    }
}
