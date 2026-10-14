//! Bounded identity for a trusted system installation, without copying or inventorying its SDK.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::{PublisherOwnerIdentityError, publisher_owner_closure, validate_owner_path};

const IDENTITY_DOMAIN: &str = "incan.virtual-toolchain-loaf/1:";
const METADATA_LIMIT: u64 = 128 * 1024;
const METADATA_NAMES: &[&str] = &[
    "incan-toolchain.json",
    "SDKSettings.json",
    "source.properties",
    "package.xml",
    "SDKManifest.xml",
];

/// A virtual loaf identifies compiler bytes, declared locations and small installation version files.
///
/// Directory contents are trusted as part of the system installation; this is deliberately not a byte identity of an
/// SDK tree. Physical root spelling is excluded. Consumers of already published native outputs need neither this
/// descriptor nor the installation. Producers re-read it immediately before execution and retain it in their receipt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VirtualToolchainLoaf {
    executable: String,
    executable_digest: String,
    /// `None` denotes a required directory; regular files retain exact byte identities.
    locations: BTreeMap<String, Option<String>>,
    versions: BTreeMap<String, String>,
}

impl VirtualToolchainLoaf {
    /// Read only the executable, explicit regular-file arguments and directly named version files.
    ///
    /// No directory is enumerated. Directory installations need at least one version file at their root or a declared
    /// directory. Callers with another SDK format can provide `incan-toolchain.json` instead of introducing
    /// platform-specific traversal.
    pub fn read<'a>(
        root: &Path,
        executable: &str,
        paths: impl IntoIterator<Item = &'a str>,
    ) -> Result<Self, PublisherOwnerIdentityError> {
        validate_owner_path(executable)?;
        let closure = publisher_owner_closure(root, paths)?;
        let mut locations = BTreeMap::new();
        let mut versions = BTreeMap::new();
        let mut metadata_roots = vec![String::new()];
        for entry in closure {
            let metadata = checked_metadata(root, &entry.relative)?;
            if metadata.is_dir() {
                metadata_roots.push(entry.relative.clone());
                locations.insert(entry.relative, None);
            } else if metadata.is_file() {
                locations.insert(entry.relative.clone(), Some(file_digest(&entry.physical)?));
            } else {
                return Err(PublisherOwnerIdentityError::Special(entry.physical));
            }
        }
        for directory in metadata_roots {
            for name in METADATA_NAMES {
                let relative = if directory.is_empty() {
                    (*name).to_string()
                } else {
                    format!("{directory}/{name}")
                };
                match fs::symlink_metadata(root.join(&relative)) {
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                    Err(source) => {
                        return Err(PublisherOwnerIdentityError::Io {
                            path: root.join(&relative),
                            source,
                        });
                    }
                    Ok(_) => {}
                }
                let metadata = checked_metadata(root, &relative)?;
                if !metadata.is_file() || metadata.len() == 0 || metadata.len() > METADATA_LIMIT {
                    return Err(PublisherOwnerIdentityError::InvalidToolchain(format!(
                        "version file {relative} must be a nonempty regular file of at most {METADATA_LIMIT} bytes"
                    )));
                }
                versions.insert(relative.clone(), file_digest(&root.join(relative))?);
            }
        }
        let executable_digest = locations
            .get(executable)
            .and_then(Option::as_ref)
            .ok_or_else(|| {
                PublisherOwnerIdentityError::InvalidToolchain("declared executable must be a regular file".to_string())
            })?
            .clone();
        let descriptor = Self {
            executable: executable.to_string(),
            executable_digest,
            locations,
            versions,
        };
        descriptor.validate()?;
        Ok(descriptor)
    }

    /// Exact compiler byte identity required by the selected native fact.
    pub fn executable_digest(&self) -> &str {
        &self.executable_digest
    }

    /// Portable descriptor identity, domain-separated from byte-owned publisher closures.
    pub fn identity(&self) -> Result<String, PublisherOwnerIdentityError> {
        self.validate()?;
        let bytes = serde_json::to_vec(self)
            .map_err(|error| PublisherOwnerIdentityError::InvalidToolchain(error.to_string()))?;
        Ok(format!("{IDENTITY_DOMAIN}{}", crate::digest_bytes(&bytes)))
    }

    /// Whether an owner identity explicitly requests version-based system installation trust.
    pub fn owns_identity(identity: &str) -> bool {
        identity.starts_with(IDENTITY_DOMAIN)
    }

    /// Reject malformed persisted descriptors without consulting the producer's installation.
    fn validate(&self) -> Result<(), PublisherOwnerIdentityError> {
        validate_owner_path(&self.executable)?;
        if self.locations.get(&self.executable).and_then(Option::as_ref) != Some(&self.executable_digest)
            || (self.locations.values().any(Option::is_none) && self.versions.is_empty())
        {
            return Err(PublisherOwnerIdentityError::InvalidToolchain(
                "missing executable or installation version evidence".to_string(),
            ));
        }
        for (path, digest) in self
            .locations
            .iter()
            .filter_map(|(path, digest)| digest.as_ref().map(|digest| (path, digest)))
            .chain(self.versions.iter())
        {
            validate_owner_path(path)?;
            if digest.len() != 71
                || !digest.starts_with("sha256:")
                || !digest[7..]
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            {
                return Err(PublisherOwnerIdentityError::InvalidToolchain(format!(
                    "invalid digest for {path}"
                )));
            }
        }
        for path in self.locations.keys() {
            validate_owner_path(path)?;
        }
        Ok(())
    }
}

/// Reject symlinks in declared locations and their ancestors, including paths escaping a real owner root.
fn checked_metadata(root: &Path, relative: &str) -> Result<fs::Metadata, PublisherOwnerIdentityError> {
    validate_owner_path(relative)?;
    let mut path = root.to_path_buf();
    for component in Path::new(relative).components() {
        path.push(component);
        let metadata = fs::symlink_metadata(&path).map_err(|source| PublisherOwnerIdentityError::Io {
            path: path.clone(),
            source,
        })?;
        if metadata.file_type().is_symlink() {
            return Err(PublisherOwnerIdentityError::InvalidToolchain(format!(
                "symlink in required location {relative}"
            )));
        }
    }
    fs::symlink_metadata(&path).map_err(|source| PublisherOwnerIdentityError::Io { path, source })
}

/// Use the existing regular-file observation boundary, preserving exact bytes and freshness checks.
fn file_digest(path: &Path) -> Result<String, PublisherOwnerIdentityError> {
    crate::store::digest_regular_file(path)
        .map(|(_, digest)| digest)
        .map_err(|error| PublisherOwnerIdentityError::Io {
            path: path.to_path_buf(),
            source: std::io::Error::other(error),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// SDK descendants never affect a virtual descriptor; compiler bytes, version and required locations do.
    #[test]
    fn virtual_toolchain_is_bounded_versioned_and_relocatable() -> Result<(), Box<dyn std::error::Error>> {
        let first = tempfile::tempdir()?;
        let second = tempfile::tempdir()?;
        for root in [first.path(), second.path()] {
            fs::create_dir_all(root.join("sdk/usr/share/man"))?;
            fs::write(root.join("cc"), b"compiler")?;
            fs::write(root.join("sdk/SDKSettings.json"), b"{\"build\":\"one\"}")?;
            fs::write(root.join("sdk/usr/share/man/ignored"), b"documentation")?;
        }
        let read = |root: &Path| VirtualToolchainLoaf::read(root, "cc", ["cc", "sdk"]);
        let initial = read(first.path())?;
        assert_eq!(initial, read(second.path())?);
        assert_eq!(initial.identity()?, read(second.path())?.identity()?);
        fs::write(
            first.path().join("sdk/usr/share/man/ignored"),
            b"changed unrelated documentation",
        )?;
        // A dangling descendant would fail a recursive regular-file inventory but is irrelevant here.
        #[cfg(unix)]
        std::os::unix::fs::symlink("absent", first.path().join("sdk/usr/share/man/dangling"))?;
        assert_eq!(initial, read(first.path())?);
        fs::write(first.path().join("sdk/SDKSettings.json"), b"{\"build\":\"two\"}")?;
        assert_ne!(initial.identity()?, read(first.path())?.identity()?);
        fs::write(first.path().join("cc"), b"different compiler")?;
        assert_ne!(initial.executable_digest(), read(first.path())?.executable_digest());
        fs::rename(first.path().join("sdk"), first.path().join("missing-sdk"))?;
        assert!(read(first.path()).is_err());
        Ok(())
    }

    /// Portable installations can name a small explicit descriptor; missing, oversized or symlinked evidence refuses.
    #[test]
    fn virtual_toolchain_refuses_unversioned_or_substituted_installations() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        fs::create_dir(root.path().join("sysroot"))?;
        fs::write(root.path().join("cc"), b"compiler")?;
        let read = || VirtualToolchainLoaf::read(root.path(), "cc", ["cc", "sysroot"]);
        assert!(read().is_err());
        for marker in METADATA_NAMES {
            let path = root.path().join(marker);
            fs::write(&path, b"version=1 build=exact")?;
            read()?.identity()?;
            fs::remove_file(path)?;
        }
        fs::write(
            root.path().join("incan-toolchain.json"),
            vec![b'x'; usize::try_from(METADATA_LIMIT)? + 1],
        )?;
        assert!(read().is_err());
        fs::remove_file(root.path().join("incan-toolchain.json"))?;
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("cc", root.path().join("incan-toolchain.json"))?;
            assert!(read().is_err());
        }
        assert!(VirtualToolchainLoaf::read(root.path(), "../cc", ["cc"]).is_err());
        Ok(())
    }
}
