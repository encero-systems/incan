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
///
/// Only regular-file byte observations are accelerated. Every call still enumerates real directories and reads
/// symlink text so topology changes cannot be hidden by an aggregate tree stamp.
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
        let (_, digest) = crate::store::digest_regular_file(&physical).map_err(|error| {
            let source = match error {
                crate::store::OvenStoreError::Io { source, .. } => source,
                error => std::io::Error::other(error),
            };
            PublisherOwnerIdentityError::Io {
                path: physical.clone(),
                source,
            }
        })?;
        entries.insert(
            relative_text,
            InventoryEntry {
                kind: "file",
                value: digest,
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

    /// Expected canonical bytes come from this fixed fixture's contents and topology, never the observed digest cache.
    fn observed_owner_fixture_identity(header: &[u8], added: bool, link_text: &str) -> String {
        let added_file = if added {
            format!("file\tsdk/include/nested/extra.h\t{}\n", digest_bytes(b"added one"))
        } else {
            String::new()
        };
        let added_directory = if added { "dir\tsdk/include/new-dir\t\n" } else { "" };
        let inventory = format!(
            concat!(
                "file\tbin/tool\t{tool}\n",
                "dir\tsdk/include\t\n",
                "symlink\tsdk/include/alias.h\t{link}\n",
                "file\tsdk/include/header.h\t{header}\n",
                "dir\tsdk/include/nested\t\n",
                "{added_file}",
                "file\tsdk/include/nested/value.h\t{nested}\n",
                "{added_directory}",
            ),
            tool = digest_bytes(b"native one"),
            link = link_text,
            header = digest_bytes(header),
            added_file = added_file,
            nested = digest_bytes(b"nested one"),
            added_directory = added_directory,
        );
        digest_bytes(inventory.as_bytes())
    }

    /// Run the public inventory caller with a private persistent observation cache and inspect its actual byte reads.
    fn assert_owner_fixture_observation(
        root: &Path,
        expected: &str,
        expected_reads: &[u64],
    ) -> Result<(), Box<dyn std::error::Error>> {
        let root = root.canonicalize()?;
        let output = std::process::Command::new(std::env::current_exe()?)
            .args([
                "--exact",
                "publisher_owner::tests::dev7_publisher_owner_observation_preserves_identity_and_freshness",
                "--nocapture",
            ])
            .env("INCAN_DEV7_PUBLISHER_OWNER_CONTROL_ROOT", &root)
            .env("INCAN_DEV7_PUBLISHER_OWNER_CONTROL_EXPECTED", expected)
            .env("INCAN_HOME", root.join("incan-home"))
            .env("INCAN_OVEN_TRACE_FILE_DIGESTS", "1")
            .output()?;
        assert!(
            output.status.success(),
            "child failed: {} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
        let observations = String::from_utf8(output.stderr)?
            .lines()
            .filter_map(|line| line.strip_prefix("Oven file digest: "))
            .map(serde_json::from_str::<serde_json::Value>)
            .collect::<Result<Vec<_>, _>>()?;
        let reads = observations
            .iter()
            .map(|record| {
                record["input_bytes_read"]
                    .as_u64()
                    .ok_or("missing owner byte-read observation")
            })
            .collect::<Result<Vec<_>, _>>()?;
        assert_eq!(reads, expected_reads);
        for record in observations {
            assert_eq!(record["scheme"], "raw-sha256-v1");
            let path = record["path"].as_str().ok_or("missing observed owner path")?;
            assert!(Path::new(path).starts_with(root.join("owner")));
            assert_ne!(
                Path::new(path).file_name(),
                Some(std::ffi::OsStr::new("alias.h")),
                "symlinks contribute link text rather than followed bytes"
            );
        }
        Ok(())
    }

    /// The actual owner identity caller reuses regular bytes while freshly enumerating topology and link text.
    #[test]
    fn dev7_publisher_owner_observation_preserves_identity_and_freshness() -> Result<(), Box<dyn std::error::Error>> {
        if let Some(root) = std::env::var_os("INCAN_DEV7_PUBLISHER_OWNER_CONTROL_ROOT") {
            let owner = Path::new(&root).join("owner");
            let expected = std::env::var("INCAN_DEV7_PUBLISHER_OWNER_CONTROL_EXPECTED")?;
            let result = publisher_owner_identity(&owner, ["bin/tool", "sdk/include"]);
            if matches!(expected.as_str(), "missing-file" | "missing-root") {
                let error = result.err().ok_or("missing owner path was accepted")?;
                let PublisherOwnerIdentityError::Io { path, source } = error else {
                    return Err("missing owner did not preserve its I/O error".into());
                };
                let expected_path = if expected == "missing-root" {
                    owner
                } else {
                    owner.join("bin/tool")
                };
                assert_eq!(path, expected_path);
                assert_eq!(source.kind(), std::io::ErrorKind::NotFound);
            } else {
                assert_eq!(result?, expected);
            }
            return Ok(());
        }
        let root = tempdir()?;
        let owner = root.path().join("owner");
        fs::create_dir_all(owner.join("bin"))?;
        fs::create_dir_all(owner.join("sdk/include/nested"))?;
        fs::write(owner.join("bin/tool"), b"native one")?;
        fs::write(owner.join("sdk/include/header.h"), b"header one")?;
        fs::write(owner.join("sdk/include/nested/value.h"), b"nested one")?;
        symlink("header.h", owner.join("sdk/include/alias.h"))?;
        let initial = observed_owner_fixture_identity(b"header one", false, "header.h");
        assert_owner_fixture_observation(root.path(), &initial, &[10, 10, 10])?;
        assert_owner_fixture_observation(root.path(), &initial, &[0, 0, 0])?;

        let header = owner.join("sdk/include/header.h");
        let modified = fs::metadata(&header)?.modified()?;
        fs::write(&header, b"header two")?;
        fs::OpenOptions::new()
            .write(true)
            .open(&header)?
            .set_times(fs::FileTimes::new().set_modified(modified))?;
        assert_eq!(fs::metadata(&header)?.modified()?, modified);
        let edited = observed_owner_fixture_identity(b"header two", false, "header.h");
        assert_ne!(edited, initial);
        assert_owner_fixture_observation(root.path(), &edited, &[0, 10, 0])?;

        fs::write(owner.join("sdk/include/nested/extra.h"), b"added one")?;
        fs::create_dir(owner.join("sdk/include/new-dir"))?;
        let added = observed_owner_fixture_identity(b"header two", true, "header.h");
        assert_ne!(added, edited);
        assert_owner_fixture_observation(root.path(), &added, &[0, 0, 9, 0])?;
        fs::remove_file(owner.join("sdk/include/nested/extra.h"))?;
        fs::remove_dir(owner.join("sdk/include/new-dir"))?;
        assert_owner_fixture_observation(root.path(), &edited, &[0, 0, 0])?;

        let alias = owner.join("sdk/include/alias.h");
        fs::remove_file(&alias)?;
        symlink("nested/value.h", &alias)?;
        let retargeted = observed_owner_fixture_identity(b"header two", false, "nested/value.h");
        assert_ne!(retargeted, edited);
        assert_owner_fixture_observation(root.path(), &retargeted, &[0, 0, 0])?;
        assert_owner_fixture_observation(root.path(), &retargeted, &[0, 0, 0])?;

        fs::remove_file(owner.join("bin/tool"))?;
        assert_owner_fixture_observation(root.path(), "missing-file", &[])?;
        fs::rename(&owner, root.path().join("owner-moved"))?;
        assert_owner_fixture_observation(root.path(), "missing-root", &[])?;
        Ok(())
    }
}
