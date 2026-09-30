//! Portable identity and physical closure for publisher executable owners.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Component, Path, PathBuf};

use crate::digest_bytes;

/// One physical path admitted from an executable owner's identity-bearing closure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublisherOwnerClosurePath {
    /// Portable path relative to the owner root.
    pub relative: String,
    /// Physical path below the caller-held owner root.
    pub physical: PathBuf,
    /// Whether the path is a real directory whose descendants are in the closure.
    pub directory: bool,
}

/// Why an executable-owner closure cannot be inventoried portably.
#[derive(Debug, thiserror::Error)]
pub enum PublisherOwnerIdentityError {
    /// A declared owner path was not a portable relative path.
    #[error("owner path `{0}` is not a portable relative path")]
    InvalidPath(String),
    /// The owner root is not a real directory.
    #[error("owner root {0} must be a real directory")]
    InvalidRoot(PathBuf),
    /// A path or symlink target cannot enter the UTF-8 inventory.
    #[error("owner closure entry {0} is not portable UTF-8")]
    NonUtf8(PathBuf),
    /// Filesystem inspection failed.
    #[error("owner closure I/O failed at {path}: {source}")]
    Io {
        /// Path being inspected.
        path: PathBuf,
        /// Underlying filesystem error.
        #[source]
        source: std::io::Error,
    },
    /// A special filesystem entry cannot be part of a publisher owner.
    #[error("owner closure entry {0} is not a file, directory, or symlink")]
    Special(PathBuf),
}

/// Compute the canonical identity of the executable plus every declared owner-relative argument path.
///
/// Directories are inventoried recursively. Entries are sorted by owner-relative path and encoded as
/// `<kind>\t<path>\t<value>\n`, where file values are byte SHA-256 identities, symlink values are link text, and
/// directory values are empty. Physical root spelling is deliberately absent.
pub fn publisher_owner_identity<'a>(
    root: &Path,
    paths: impl IntoIterator<Item = &'a str>,
) -> Result<String, PublisherOwnerIdentityError> {
    let (inventory, _) = publisher_owner_inventory(root, paths)?;
    Ok(digest_bytes(inventory.as_bytes()))
}

/// Resolve the exact identity-bearing closure used to confine one publisher executable.
///
/// The returned list includes every recursively inventoried entry once. Callers admit data reads for these paths
/// and metadata-only reads for their ancestors.
pub fn publisher_owner_closure<'a>(
    root: &Path,
    paths: impl IntoIterator<Item = &'a str>,
) -> Result<Vec<PublisherOwnerClosurePath>, PublisherOwnerIdentityError> {
    let root_metadata = fs::symlink_metadata(root).map_err(|source| PublisherOwnerIdentityError::Io {
        path: root.to_path_buf(),
        source,
    })?;
    if root_metadata.file_type().is_symlink() || !root_metadata.is_dir() {
        return Err(PublisherOwnerIdentityError::InvalidRoot(root.to_path_buf()));
    }
    let root = fs::canonicalize(root).map_err(|source| PublisherOwnerIdentityError::Io {
        path: root.to_path_buf(),
        source,
    })?;
    let mut closure = BTreeMap::new();
    for relative in paths {
        validate_owner_path(relative)?;
        let physical = root.join(relative);
        let metadata = fs::symlink_metadata(&physical).map_err(|source| PublisherOwnerIdentityError::Io {
            path: physical.clone(),
            source,
        })?;
        closure
            .entry(relative.to_string())
            .or_insert(PublisherOwnerClosurePath {
                relative: relative.to_string(),
                physical,
                directory: metadata.is_dir(),
            });
    }
    Ok(closure.into_values().collect())
}

/// Build the canonical inventory and resolved closure in one filesystem walk.
fn publisher_owner_inventory<'a>(
    root: &Path,
    paths: impl IntoIterator<Item = &'a str>,
) -> Result<(String, Vec<PublisherOwnerClosurePath>), PublisherOwnerIdentityError> {
    let root_metadata = fs::symlink_metadata(root).map_err(|source| PublisherOwnerIdentityError::Io {
        path: root.to_path_buf(),
        source,
    })?;
    if root_metadata.file_type().is_symlink() || !root_metadata.is_dir() {
        return Err(PublisherOwnerIdentityError::InvalidRoot(root.to_path_buf()));
    }
    let root = fs::canonicalize(root).map_err(|source| PublisherOwnerIdentityError::Io {
        path: root.to_path_buf(),
        source,
    })?;
    let mut entries = BTreeMap::new();
    for relative in paths {
        validate_owner_path(relative)?;
        inventory_entry(&root, Path::new(relative), &mut entries)?;
    }
    let mut inventory = String::new();
    let mut closure = Vec::with_capacity(entries.len());
    for (relative, entry) in entries {
        inventory.push_str(entry.kind);
        inventory.push('\t');
        inventory.push_str(&relative);
        inventory.push('\t');
        inventory.push_str(&entry.value);
        inventory.push('\n');
        closure.push(PublisherOwnerClosurePath {
            physical: root.join(&relative),
            relative,
            directory: entry.kind == "dir",
        });
    }
    Ok((inventory, closure))
}

/// One canonical inventory line before serialization.
struct InventoryEntry {
    kind: &'static str,
    value: String,
}

/// Inventory one entry and recursively expand a real directory without following symlinks.
fn inventory_entry(
    root: &Path,
    relative: &Path,
    entries: &mut BTreeMap<String, InventoryEntry>,
) -> Result<(), PublisherOwnerIdentityError> {
    let relative_text = portable_path(relative)?;
    if entries.contains_key(&relative_text) {
        return Ok(());
    }
    let physical = root.join(relative);
    let metadata = fs::symlink_metadata(&physical).map_err(|source| PublisherOwnerIdentityError::Io {
        path: physical.clone(),
        source,
    })?;
    if metadata.file_type().is_symlink() {
        let target = fs::read_link(&physical).map_err(|source| PublisherOwnerIdentityError::Io {
            path: physical.clone(),
            source,
        })?;
        let target = target
            .to_str()
            .ok_or_else(|| PublisherOwnerIdentityError::NonUtf8(physical.clone()))?;
        entries.insert(
            relative_text,
            InventoryEntry {
                kind: "symlink",
                value: target.to_string(),
            },
        );
        return Ok(());
    }
    if metadata.is_file() {
        let bytes = fs::read(&physical).map_err(|source| PublisherOwnerIdentityError::Io {
            path: physical.clone(),
            source,
        })?;
        entries.insert(
            relative_text,
            InventoryEntry {
                kind: "file",
                value: digest_bytes(&bytes),
            },
        );
        return Ok(());
    }
    if !metadata.is_dir() {
        return Err(PublisherOwnerIdentityError::Special(physical));
    }
    entries.insert(
        relative_text,
        InventoryEntry {
            kind: "dir",
            value: String::new(),
        },
    );
    let mut children = fs::read_dir(&physical)
        .map_err(|source| PublisherOwnerIdentityError::Io {
            path: physical.clone(),
            source,
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|source| PublisherOwnerIdentityError::Io {
            path: physical.clone(),
            source,
        })?;
    children.sort_by_key(std::fs::DirEntry::file_name);
    for child in children {
        inventory_entry(root, &relative.join(child.file_name()), entries)?;
    }
    Ok(())
}

/// Require a non-empty relative path composed only of normal portable components.
fn validate_owner_path(path: &str) -> Result<(), PublisherOwnerIdentityError> {
    let path_value = Path::new(path);
    if path.is_empty()
        || path_value.is_absolute()
        || path_value
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
        || path_value.to_string_lossy().replace('\\', "/") != path
    {
        return Err(PublisherOwnerIdentityError::InvalidPath(path.to_string()));
    }
    Ok(())
}

/// Render one already validated owner-relative path with `/` separators.
fn portable_path(path: &Path) -> Result<String, PublisherOwnerIdentityError> {
    path.to_str()
        .map(|value| value.replace('\\', "/"))
        .ok_or_else(|| PublisherOwnerIdentityError::NonUtf8(path.to_path_buf()))
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::symlink;

    use tempfile::tempdir;

    use super::*;

    /// Equal owner closures have one identity across distinct physical roots and declaration order.
    #[test]
    fn owner_identity_is_stable_across_equal_fixture_roots() -> Result<(), Box<dyn std::error::Error>> {
        let first = tempdir()?;
        let second = tempdir()?;
        for root in [first.path(), second.path()] {
            fs::create_dir_all(root.join("sdk/include/nested"))?;
            fs::write(root.join("bin-clang"), b"compiler")?;
            fs::write(root.join("sdk/include/header.h"), b"header")?;
            fs::write(root.join("sdk/include/nested/value.h"), b"nested")?;
            symlink("header.h", root.join("sdk/include/alias.h"))?;
        }
        let first_identity = publisher_owner_identity(first.path(), ["bin-clang", "sdk/include"])?;
        let second_identity = publisher_owner_identity(second.path(), ["sdk/include", "bin-clang"])?;
        assert_eq!(first_identity, second_identity);
        Ok(())
    }

    /// Absolute paths, traversal, and owner-root shorthand never enter a portable inventory.
    #[test]
    fn owner_identity_refuses_nonportable_paths() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempdir()?;
        for path in ["/usr/bin/clang", "../clang", "sdk/../clang", "."] {
            assert!(
                publisher_owner_identity(root.path(), [path]).is_err(),
                "accepted {path}"
            );
        }
        Ok(())
    }
}
