//! Physical generated-source hashing shared by publication and native receipts.
//!
//! The encoding is the existing generated-source contract: file SHA-256 and a sorted portable path-to-digest JSON
//! map for trees. This module performs no dependency selection or host-tool discovery.

use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

/// A named physical source input could not be read under the generated-source contract.
#[derive(Debug, thiserror::Error)]
pub(crate) enum GeneratedSourceError {
    /// A source is missing, indirect, unsupported or cannot be read.
    #[error("invalid generated source {path}: {message}")]
    Invalid { path: PathBuf, message: String },
    /// The portable source record map could not be encoded.
    #[error("failed to serialize generated source records: {0}")]
    Serialize(String),
}

/// Hash bytes with the established `sha256:` rendering.
pub(crate) fn digest_bytes(content: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content);
    format!("sha256:{}", hex::encode(hasher.finalize()))
}

/// Hash one direct-rustc source file after proving it is a regular, non-symlink input.
pub(crate) fn digest_file(path: &Path) -> Result<String, GeneratedSourceError> {
    let metadata = fs::symlink_metadata(path).map_err(|error| GeneratedSourceError::Invalid {
        path: path.to_path_buf(),
        message: error.to_string(),
    })?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(GeneratedSourceError::Invalid {
            path: path.to_path_buf(),
            message: "must be a regular non-symlink file".to_string(),
        });
    }
    fs::read(path)
        .map(|bytes| digest_bytes(&bytes))
        .map_err(|error| GeneratedSourceError::Invalid {
            path: path.to_path_buf(),
            message: error.to_string(),
        })
}

/// Hash every regular file in an explicitly named generated tree, rejecting symlinks and empty trees.
pub(crate) fn digest_tree(root: &Path) -> Result<String, GeneratedSourceError> {
    let metadata = fs::symlink_metadata(root).map_err(|error| GeneratedSourceError::Invalid {
        path: root.to_path_buf(),
        message: error.to_string(),
    })?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(GeneratedSourceError::Invalid {
            path: root.to_path_buf(),
            message: "must be a directory without symlink indirection".to_string(),
        });
    }
    let mut records = BTreeMap::new();
    collect_generated_source_tree(root, root, &mut records)?;
    if records.is_empty() {
        return Err(GeneratedSourceError::Invalid {
            path: root.to_path_buf(),
            message: "must contain at least one regular file".to_string(),
        });
    }
    let payload = serde_json::to_vec(&records).map_err(|error| GeneratedSourceError::Serialize(error.to_string()))?;
    Ok(digest_bytes(&payload))
}

/// Recursively collect one generated source tree with sorted portable paths and no link traversal.
fn collect_generated_source_tree(
    root: &Path,
    current: &Path,
    records: &mut BTreeMap<String, String>,
) -> Result<(), GeneratedSourceError> {
    let mut entries = fs::read_dir(current)
        .map_err(|error| GeneratedSourceError::Invalid {
            path: current.to_path_buf(),
            message: error.to_string(),
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| GeneratedSourceError::Invalid {
            path: current.to_path_buf(),
            message: error.to_string(),
        })?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path).map_err(|error| GeneratedSourceError::Invalid {
            path: path.clone(),
            message: error.to_string(),
        })?;
        if metadata.file_type().is_symlink() {
            return Err(GeneratedSourceError::Invalid {
                path,
                message: "symlinks are not allowed in a generated source closure".to_string(),
            });
        }
        if metadata.is_dir() {
            collect_generated_source_tree(root, &path, records)?;
            continue;
        }
        if !metadata.is_file() {
            return Err(GeneratedSourceError::Invalid {
                path,
                message: "may contain only regular files and directories".to_string(),
            });
        }
        let relative = path
            .strip_prefix(root)
            .map_err(|_| GeneratedSourceError::Invalid {
                path: path.clone(),
                message: "escaped the declared generated source root".to_string(),
            })?
            .to_string_lossy()
            .replace('\\', "/");
        let digest =
            fs::read(&path)
                .map(|bytes| digest_bytes(&bytes))
                .map_err(|error| GeneratedSourceError::Invalid {
                    path: path.clone(),
                    message: error.to_string(),
                })?;
        if records.insert(relative.clone(), digest).is_some() {
            return Err(GeneratedSourceError::Invalid {
                path,
                message: format!("duplicate portable source path `{relative}`"),
            });
        }
    }
    Ok(())
}
