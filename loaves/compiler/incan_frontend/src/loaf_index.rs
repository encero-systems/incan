//! The compiled Loaves a compilation reaches, and the one index every consumer reads them through (DD-0007).
//!
//! A compiled Loaf is one artifact directory per profile: its library manifest (`<name>.incnlib`), its governed bodies
//! (`<name>.incnbody`), its library unit (`lib<name>.rlib`) and the unit's native closure (`<name>.native.json`). An
//! ordinary dependency owns `pub::<key>`; a standard package owns the `std.*` roots the compiler-pinned
//! [`STANDARD_PACKAGE_NAMESPACE_POLICIES`] assign to it. A module path resolves to exactly one entry.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use incan_lang::lang::standard_packages::{STANDARD_PACKAGE_NAMESPACE_POLICIES, StandardPackageNamespacePolicy};
use sha2::{Digest, Sha256};

use crate::library_manifest::LibraryManifest;

/// The artifact directory of a Loaf rooted at `root`, for one profile: `<root>/target/incan/loaf/<profile>`.
pub fn artifact_dir(root: &Path, profile: &str) -> PathBuf {
    root.join("target/incan/loaf").join(profile)
}

/// One compiled Loaf: its artifact directory and the library manifest read from it.
#[derive(Debug, Clone)]
pub struct LoafArtifact {
    /// The Loaf's package name, which names every file in its directory.
    pub name: String,
    /// The artifact directory for the compilation's profile.
    pub dir: PathBuf,
    /// The library manifest: public exports, vocabulary, soft keywords, contract metadata and Rust ABI.
    pub manifest: Arc<LibraryManifest>,
}

impl LoafArtifact {
    /// Read the Loaf `name` from its artifact directory.
    pub fn read(dir: &Path, name: &str) -> Result<Self, LoafIndexError> {
        let path = dir.join(format!("{name}.incnlib"));
        let manifest = LibraryManifest::read_from_path(&path).map_err(|error| LoafIndexError::Unreadable {
            path: path.clone(),
            reason: error.to_string(),
        })?;
        if manifest.name != name {
            return Err(LoafIndexError::Unreadable {
                path,
                reason: format!("the manifest names the Loaf `{}`", manifest.name),
            });
        }
        Ok(Self {
            name: name.to_owned(),
            dir: dir.to_path_buf(),
            manifest: Arc::new(manifest),
        })
    }

    /// The library manifest file.
    pub fn manifest_path(&self) -> PathBuf {
        self.dir.join(format!("{}.incnlib", self.name))
    }

    /// The governed bodies file: the executable representation callers instantiate.
    pub fn bodies_path(&self) -> PathBuf {
        self.dir.join(format!("{}.incnbody", self.name))
    }

    /// The library unit.
    pub fn rlib_path(&self) -> PathBuf {
        self.dir.join(format!("lib{}.rlib", self.name))
    }

    /// The library unit's native closure: the rlibs and search paths it links.
    pub fn native_closure_path(&self) -> PathBuf {
        self.dir.join(format!("{}.native.json", self.name))
    }
}

/// The namespace one Loaf owns in a compilation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoafNamespace {
    /// An ordinary dependency, imported as `pub::<key>`.
    Dependency {
        /// The dependency key the consumer's manifest declares.
        key: String,
    },
    /// A standard package, owning the `std.*` roots its compiler-pinned policy assigns.
    Standard {
        /// The package's policy.
        policy: &'static StandardPackageNamespacePolicy,
    },
}

/// One Loaf of a compilation and the namespace it owns.
#[derive(Debug, Clone)]
pub struct LoafEntry {
    /// The key the entry is indexed by: the dependency key, or the standard package's name.
    pub key: String,
    /// The namespace the Loaf owns.
    pub namespace: LoafNamespace,
    /// The compiled Loaf.
    pub artifact: LoafArtifact,
}

impl LoafEntry {
    /// Whether this Loaf owns `module` (`["pub", key, ..]` for a dependency, `["std", root, ..]` for a standard package).
    pub fn owns(&self, module: &[String]) -> bool {
        match (&self.namespace, module) {
            (LoafNamespace::Dependency { key }, [root, name, ..]) => root == "pub" && name == key,
            (LoafNamespace::Standard { policy }, [root, name, ..]) => {
                root == "std" && policy.namespace_roots.contains(&name.as_str())
            }
            _ => false,
        }
    }
}

/// A Loaf the index cannot hold.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LoafIndexError {
    /// The artifact's manifest is missing, unreadable or names another Loaf.
    #[error("cannot read the Loaf artifact {}: {reason}", path.display())]
    Unreadable {
        /// The manifest path.
        path: PathBuf,
        /// What failed.
        reason: String,
    },
    /// Two entries share one key.
    #[error("two Loaves are indexed as `{key}`")]
    DuplicateKey {
        /// The repeated key.
        key: String,
    },
    /// No standard package policy names this package.
    #[error("`{package}` is not a standard package")]
    UnknownStandardPackage {
        /// The package name.
        package: String,
    },
}

/// Every Loaf a compilation reaches, each owning its namespace.
#[derive(Debug, Clone, Default)]
pub struct LoafIndex {
    entries: BTreeMap<String, LoafEntry>,
}

impl LoafIndex {
    /// Index `entries`, refusing two entries under one key.
    pub fn new(entries: impl IntoIterator<Item = LoafEntry>) -> Result<Self, LoafIndexError> {
        let mut indexed = BTreeMap::new();
        for entry in entries {
            let key = entry.key.clone();
            if indexed.insert(key.clone(), entry).is_some() {
                return Err(LoafIndexError::DuplicateKey { key });
            }
        }
        Ok(Self { entries: indexed })
    }

    /// The entry for one dependency, read from the dependency's own root.
    pub fn dependency(key: &str, name: &str, root: &Path, profile: &str) -> Result<LoafEntry, LoafIndexError> {
        Ok(LoafEntry {
            key: key.to_owned(),
            namespace: LoafNamespace::Dependency { key: key.to_owned() },
            artifact: LoafArtifact::read(&artifact_dir(root, profile), name)?,
        })
    }

    /// The entry for one standard package, read below the toolchain's standard root.
    pub fn standard(package: &str, std_root: &Path, profile: &str) -> Result<LoafEntry, LoafIndexError> {
        let policy = STANDARD_PACKAGE_NAMESPACE_POLICIES
            .iter()
            .find(|policy| policy.package_name == package)
            .ok_or_else(|| LoafIndexError::UnknownStandardPackage {
                package: package.to_owned(),
            })?;
        let root = std_root.join(policy.source_directory);
        Ok(LoafEntry {
            key: policy.package_name.to_owned(),
            namespace: LoafNamespace::Standard { policy },
            artifact: LoafArtifact::read(&artifact_dir(&root, profile), policy.package_name)?,
        })
    }

    /// Every entry, in key order.
    pub fn entries(&self) -> impl Iterator<Item = &LoafEntry> {
        self.entries.values()
    }

    /// The entry indexed as `key`.
    pub fn entry(&self, key: &str) -> Option<&LoafEntry> {
        self.entries.get(key)
    }

    /// The one entry owning `module`, if any.
    pub fn owner_of(&self, module: &[String]) -> Option<&LoafEntry> {
        self.entries.values().find(|entry| entry.owns(module))
    }

    /// A key that changes whenever any indexed Loaf or its manifest changes, for persisted readings.
    pub fn persistent_key(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(b"incan-loaf-index-v1\0");
        for entry in self.entries.values() {
            hasher.update(entry.key.as_bytes());
            hasher.update([0]);
            hasher.update(entry.artifact.dir.as_os_str().as_encoded_bytes());
            hasher.update([0]);
            let manifest = serde_json::to_vec(entry.artifact.manifest.as_ref()).unwrap_or_default();
            hasher.update(manifest.len().to_le_bytes());
            hasher.update(manifest);
        }
        format!("{:x}", hasher.finalize())
    }
}
