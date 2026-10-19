//! The compiled Loaves a compilation reaches, and the one index every consumer reads them through (DD-0007).
//!
//! A compiled Loaf is one artifact directory per profile: its library manifest (`<name>.incnlib`), its governed bodies
//! (`<name>.incnbody`), its library unit (`lib<name>.rlib`) and the unit's native closure (`<name>.native.json`). An
//! ordinary dependency owns `pub::<key>`; a standard package owns the `std.*` roots the compiler-pinned
//! [`STANDARD_PACKAGE_NAMESPACE_POLICIES`] assign to it. A module path resolves to exactly one entry.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use incan_lang::interop::RustItemMetadata;
use incan_lang::lang::standard_packages::{STANDARD_PACKAGE_NAMESPACE_POLICIES, StandardPackageNamespacePolicy};
use incan_vocab::{KeywordActivation, KeywordRegistration, KeywordSpec};
use sha2::{Digest, Sha256};

use incan_semantics_core::{CanonicalSymbolId, SymbolOrigin};

use crate::library_manifest::{ExportIdentity, ExportIdentityKind, LibraryManifest};

/// The artifact directory of a Loaf rooted at `root`, for one profile: `<root>/target/incan/loaf/<profile>`.
pub fn artifact_dir(root: &Path, profile: &str) -> PathBuf {
    root.join("target/incan/loaf").join(profile)
}

/// The identity of one compiled Loaf: its package, version, manifest digest and public feature projection.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct LoafIdentity {
    /// The Loaf's package name.
    pub name: String,
    /// The Loaf's exact version.
    pub version: String,
    /// The digest of the Loaf's library manifest.
    pub digest: String,
    /// The public features the artifact was compiled with.
    pub feature_projection: BTreeSet<String>,
}

impl LoafIdentity {
    /// A deterministic key for maps, reports and lock records: `name@version#digest[features]`.
    pub fn stable_key(&self) -> String {
        let features = self.feature_projection.iter().cloned().collect::<Vec<_>>().join(",");
        format!("{}@{}#{}[{}]", self.name, self.version, self.digest, features)
    }
}

/// One compiled Loaf: its identity, artifact directory and the library manifest read from it.
#[derive(Debug, Clone)]
pub struct LoafArtifact {
    /// The Loaf's identity.
    pub identity: LoafIdentity,
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
        let unreadable = |reason: String| LoafIndexError::Unreadable {
            path: path.clone(),
            reason,
        };
        let bytes = std::fs::read(&path).map_err(|error| unreadable(error.to_string()))?;
        let manifest = LibraryManifest::read_from_path(&path).map_err(|error| unreadable(error.to_string()))?;
        if manifest.name != name {
            return Err(unreadable(format!("the manifest names the Loaf `{}`", manifest.name)));
        }
        let identity = LoafIdentity {
            name: manifest.name.clone(),
            version: manifest.version.clone(),
            digest: format!("sha256:{:x}", Sha256::digest(&bytes)),
            feature_projection: BTreeSet::new(),
        };
        Ok(Self {
            identity,
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

    /// Read the library unit's native closure.
    pub fn native_closure(&self) -> Result<LoafNativeClosure, LoafIndexError> {
        let path = self.native_closure_path();
        let bytes = std::fs::read(&path).map_err(|error| LoafIndexError::Unreadable {
            path: path.clone(),
            reason: error.to_string(),
        })?;
        serde_json::from_slice(&bytes).map_err(|error| LoafIndexError::Unreadable {
            path,
            reason: error.to_string(),
        })
    }
}

/// A library unit's native closure (`<name>.native.json`): the crates it links by name and the directories their own
/// dependencies are found in. A consumer links the unit with the same closure.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LoafNativeClosure {
    /// The crates the unit links, by crate name and rlib.
    pub externs: Vec<(String, PathBuf)>,
    /// The directories the linked crates' own dependencies are found in.
    pub search: Vec<PathBuf>,
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

    /// The path this Loaf's public modules are bound under: `["pub", key]` for a dependency, `["std"]` for a standard
    /// package, whose manifest spells its modules without the `std` root.
    pub fn binding_root(&self) -> Vec<String> {
        match &self.namespace {
            LoafNamespace::Dependency { key } => vec!["pub".to_owned(), key.clone()],
            LoafNamespace::Standard { .. } => vec!["std".to_owned()],
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

    /// The Loaf that declares `canonical` (a package identity) among its public exports.
    ///
    /// Every Loaf a compilation reaches is indexed, transitive ones included, so the declaring Loaf is found by the
    /// identity's package and its identity graph rather than by walking import routes.
    pub fn declaring_loaf(&self, canonical: &CanonicalSymbolId) -> Option<&LoafEntry> {
        let SymbolOrigin::Package { library, .. } = &canonical.origin else {
            return None;
        };
        self.entries.values().find(|entry| {
            &entry.artifact.manifest.name == library
                && entry
                    .artifact
                    .manifest
                    .contract_metadata
                    .identity_graph
                    .exports
                    .iter()
                    .any(|export| export.canonical.as_ref().and_then(|id| id.hydrate()).as_ref() == Some(canonical))
        })
    }

    /// The public nominal export of `canonical` in its declaring Loaf: its shortest public path.
    pub fn public_nominal(&self, canonical: &CanonicalSymbolId) -> Option<(&LoafEntry, &ExportIdentity)> {
        let entry = self.declaring_loaf(canonical)?;
        let export = entry
            .artifact
            .manifest
            .contract_metadata
            .identity_graph
            .exports
            .iter()
            .filter(|export| export.canonical.as_ref().and_then(|id| id.hydrate()).as_ref() == Some(canonical))
            .filter(|export| {
                matches!(
                    export.kind,
                    ExportIdentityKind::Model
                        | ExportIdentityKind::Class
                        | ExportIdentityKind::Enum
                        | ExportIdentityKind::Newtype
                )
            })
            .min_by(|left, right| {
                left.public_path
                    .len()
                    .cmp(&right.public_path.len())
                    .then(left.public_path.cmp(&right.public_path))
            })?;
        Some((entry, export))
    }

    /// The manifest of the Loaf indexed as `key`.
    pub fn manifest(&self, key: &str) -> Option<&LibraryManifest> {
        self.entries.get(key).map(|entry| entry.artifact.manifest.as_ref())
    }

    /// Rust item metadata a Loaf's Rust facet ships in its manifest, from the first Loaf (in key order) that has it.
    pub fn rust_abi_item(&self, canonical_path: &str) -> Option<RustItemMetadata> {
        self.entries
            .values()
            .filter_map(|entry| entry.artifact.manifest.rust_abi.as_ref())
            .find_map(|abi| abi.get(canonical_path).cloned())
    }

    /// Keyword registrations the indexed Loaves export, keyed by Loaf, for the consumer's parser.
    pub fn imported_vocab(&self) -> crate::parser::ImportedLibraryVocab {
        let mut map = std::collections::HashMap::new();
        for (key, entry) in &self.entries {
            let Some(vocab) = &entry.artifact.manifest.vocab else {
                continue;
            };
            let registrations = vocab
                .keyword_registrations
                .iter()
                .filter_map(|registration| normalize_keyword_registration(key, registration))
                .collect::<Vec<_>>();
            if !registrations.is_empty() {
                map.insert(key.clone(), registrations);
            }
        }
        map
    }

    /// DSL surfaces the indexed Loaves export, keyed by Loaf.
    pub fn imported_dsl_surfaces(&self) -> crate::parser::ImportedLibraryDslSurfaces {
        let mut map = std::collections::HashMap::new();
        for (key, entry) in &self.entries {
            if let Some(vocab) = &entry.artifact.manifest.vocab
                && !vocab.dsl_surfaces.is_empty()
            {
                map.insert(key.clone(), vocab.dsl_surfaces.clone());
            }
        }
        map
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

/// One Loaf's keyword registration as its consumer's parser reads it: named keywords only, sorted and deduplicated,
/// with an import-activated registration defaulting to the Loaf's own key as its namespace.
fn normalize_keyword_registration(key: &str, registration: &KeywordRegistration) -> Option<KeywordRegistration> {
    let mut keywords: Vec<KeywordSpec> = registration
        .keywords
        .iter()
        .filter(|keyword| !keyword.name.trim().is_empty())
        .cloned()
        .collect();
    if keywords.is_empty() {
        return None;
    }
    keywords.sort_by(|left, right| left.name.cmp(&right.name));
    keywords.dedup_by(|left, right| left.name == right.name && left.surface_kind == right.surface_kind);
    let activation = match &registration.activation {
        KeywordActivation::Always => KeywordActivation::Always,
        KeywordActivation::OnImport { namespace } => KeywordActivation::OnImport {
            namespace: if namespace.trim().is_empty() {
                key.to_owned()
            } else {
                namespace.trim().to_owned()
            },
        },
        _ => return None,
    };
    Some(KeywordRegistration {
        activation,
        keywords,
        valid_decorators: registration.valid_decorators.clone(),
    })
}
