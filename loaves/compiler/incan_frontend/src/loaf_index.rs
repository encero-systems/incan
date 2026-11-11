//! The compiled Loaves a compilation reaches, and the one index every consumer reads them through (DD-0007).
//!
//! A compiled Loaf is one artifact directory per profile: its library manifest (`<name>.incnlib`), its governed bodies
//! (`<name>.incnbody`), its library unit (`lib<crate>.rlib`), the unit's native closure (`<name>.native.json`), and an
//! optional manifest-authenticated native ABI cache (`<name>.rustabi`). An
//! ordinary dependency owns `pub::<key>`; a standard package owns the `std.*` roots the compiler-pinned
//! [`STANDARD_PACKAGE_NAMESPACE_POLICIES`] assign to it. A module path resolves to exactly one entry.

//! Here `<name>` is the shared artifact stem: unchanged for bare packages and reversibly encoded for scoped ones.

use incan_loaf_names::artifact_stem;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use serde::{Deserialize, Serialize};

use incan_lang::interop::RustItemMetadata;
use incan_lang::lang::standard_packages::{STANDARD_PACKAGE_NAMESPACE_POLICIES, StandardPackageNamespacePolicy};
use incan_vocab::{KeywordActivation, KeywordRegistration, KeywordSpec};

use crate::rust_abi_cache::{RustAbiCache, RustTypeAliasLookup, ShippedRustAbiCache};

mod compiler_record_set;
mod decoded_manifest;
mod lazy_entry;
mod manifest_input;
mod retained;

use lazy_entry::IndexedLoaf;

/// Publish one private cache record atomically: write `bytes` to a new owner-only file beside `path`, named with this
/// process and `suffix`, and rename it over `path`. A failed write removes the temporary file; concurrent readers see
/// either the old record or the complete new one.
fn publish_private_record(path: &Path, bytes: &[u8], suffix: &str) -> std::io::Result<()> {
    use std::io::Write;
    #[cfg(unix)]
    use std::os::unix::fs::OpenOptionsExt;
    let parent = path
        .parent()
        .ok_or_else(|| std::io::Error::other("the private record has no parent directory"))?;
    std::fs::create_dir_all(parent)?;
    let temporary = path.with_extension(format!("{}.{suffix}", std::process::id()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(&temporary)?;
    let result = file.write_all(bytes).and_then(|()| std::fs::rename(&temporary, path));
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

use incan_semantics_core::{CanonicalSymbolId, SymbolOrigin};

use crate::library_manifest::{ExportIdentity, ExportIdentityKind, LibraryManifest};

/// The extension of a Loaf's library manifest, `<name>.incnlib`.
pub const LIBRARY_MANIFEST_EXTENSION: &str = "incnlib";

/// The extension of a Loaf's executable representation, `<name>.incnbody`.
pub const EXECUTABLE_REPRESENTATION_EXTENSION: &str = "incnbody";

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
    /// Manifest-authenticated native facts decoded lazily from the Loaf's indexed ABI asset.
    rust_abi_cache: Option<Arc<RustAbiCache>>,
    /// Paths from a version-10 cache index that the candidate must re-extract without decoding old records.
    stale_rust_paths: BTreeSet<String>,
}

impl LoafArtifact {
    /// Read and validate the Loaf `name`, deriving its identity from the exact bytes decoded into the manifest.
    pub fn read(dir: &Path, name: &str) -> Result<Self, LoafIndexError> {
        if compiler_record_set::configured() {
            return Self::read_current(dir, name);
        }
        retained::artifact(dir, name)
    }

    /// Read and validate one current generation; process-local reuse must observe every dependent file around it.
    fn read_current(dir: &Path, name: &str) -> Result<Self, LoafIndexError> {
        if let Some((manifest, digest)) = compiler_record_set::read(dir, name)? {
            let identity = LoafIdentity {
                name: manifest.name.clone(),
                version: manifest.version.clone(),
                digest,
                feature_projection: BTreeSet::new(),
            };
            let (rust_abi_cache, stale_rust_paths) = Self::read_rust_abi_cache(dir, name, &manifest)?;
            return Ok(Self {
                identity,
                name: name.to_owned(),
                dir: dir.to_path_buf(),
                manifest,
                rust_abi_cache,
                stale_rust_paths,
            });
        }
        let _invocation =
            tracing::trace_span!(target: "incan::invocation", "incan_frontend::read_loaf_manifest", loaf = name)
                .entered();
        let path = dir.join(format!("{}.{LIBRARY_MANIFEST_EXTENSION}", artifact_stem(name)));
        let unreadable = |reason: String| LoafIndexError::Unreadable {
            path: path.clone(),
            reason,
        };
        let (manifest, digest) = decoded_manifest::read(&path).map_err(unreadable)?;
        if manifest.name != name {
            return Err(unreadable(format!("the manifest names the Loaf `{}`", manifest.name)));
        }
        let identity = LoafIdentity {
            name: manifest.name.clone(),
            version: manifest.version.clone(),
            digest,
            feature_projection: BTreeSet::new(),
        };
        let (rust_abi_cache, stale_rust_paths) = Self::read_rust_abi_cache(dir, name, &manifest)?;
        Ok(Self {
            identity,
            name: name.to_owned(),
            dir: dir.to_path_buf(),
            manifest,
            rust_abi_cache,
            stale_rust_paths,
        })
    }

    /// Verify the declared cache's exact bytes before exposing its indexed facts; absent declarations retain legacy
    /// embedded metadata, while missing, altered, or incompatible declared assets refuse the Loaf. Content identities
    /// share manifests' generation-guarded reader; current ABI bytes are still decoded and checked on every load.
    fn read_rust_abi_cache(
        dir: &Path,
        name: &str,
        manifest: &LibraryManifest,
    ) -> Result<(Option<Arc<RustAbiCache>>, BTreeSet<String>), LoafIndexError> {
        let Some(expected) = manifest.rust_abi.as_ref().and_then(|abi| abi.cache_digest.as_ref()) else {
            return Ok((None, BTreeSet::new()));
        };
        let path = dir.join(format!("{}.rustabi", artifact_stem(name)));
        let unreadable = |reason: String| LoafIndexError::Unreadable {
            path: path.clone(),
            reason,
        };
        let _invocation =
            tracing::trace_span!(target: "incan::invocation", "incan_frontend::read_loaf_rust_facts", loaf = name)
                .entered();
        let (bytes, digest) = manifest_input::read_bytes(&path).map_err(|error| unreadable(error.to_string()))?;
        if digest != *expected {
            return Err(unreadable("native ABI cache digest differs from the manifest".into()));
        }
        match RustAbiCache::decode_shipped(bytes).map_err(unreadable)? {
            ShippedRustAbiCache::Current(cache) => Ok((Some(Arc::new(cache)), BTreeSet::new())),
            ShippedRustAbiCache::Previous(paths) => Ok((None, paths)),
        }
    }

    /// The library manifest file.
    pub fn manifest_path(&self) -> PathBuf {
        self.dir
            .join(format!("{}.{LIBRARY_MANIFEST_EXTENSION}", artifact_stem(&self.name)))
    }

    /// The governed bodies file: the executable representation callers instantiate.
    pub fn bodies_path(&self) -> PathBuf {
        self.dir.join(format!(
            "{}.{EXECUTABLE_REPRESENTATION_EXTENSION}",
            artifact_stem(&self.name)
        ))
    }

    /// The library unit's native closure: the rlibs and search paths it links.
    pub fn native_closure_path(&self) -> PathBuf {
        self.dir.join(format!("{}.native.json", artifact_stem(&self.name)))
    }

    /// Read the library unit's native closure. A relative path in it (a packed toolchain's Loaf) names its target from
    /// this Loaf's directory, so the closure returned holds only locatable paths.
    pub fn native_closure(&self) -> Result<LoafNativeClosure, LoafIndexError> {
        LoafNativeClosure::read(&self.dir, &self.name)
    }
}

impl LoafNativeClosure {
    /// Read the native closure of the Loaf `name` in `dir` without reading its manifest: linking needs only the
    /// closure file, and a Loaf the program never imports must not be decoded to link it.
    pub fn read(dir: &Path, name: &str) -> Result<Self, LoafIndexError> {
        let path = dir.join(format!("{}.native.json", artifact_stem(name)));
        let bytes = std::fs::read(&path).map_err(|error| LoafIndexError::Unreadable {
            path: path.clone(),
            reason: error.to_string(),
        })?;
        let mut closure: LoafNativeClosure =
            serde_json::from_slice(&bytes).map_err(|error| LoafIndexError::Unreadable {
                path,
                reason: error.to_string(),
            })?;
        closure.locate_in(dir);
        Ok(closure)
    }
}

/// `path` as a usable one: as written when absolute, otherwise joined onto `directory` with `.` and `..` components
/// collapsed lexically, so equal artifacts reached from different Loaves compare equal.
fn located(directory: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        return path.to_path_buf();
    }
    let mut result = directory.to_path_buf();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                result.pop();
            }
            other => result.push(other.as_os_str()),
        }
    }
    result
}

/// A library unit's native closure (`<name>.native.json`): the crates it links by name and the directories their own
/// dependencies are found in. A consumer links the unit with the same closure.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LoafNativeClosure {
    /// The crates the unit links, by crate name and rlib.
    pub externs: Vec<(String, PathBuf)>,
    /// The directories the linked crates' own dependencies are found in.
    pub search: Vec<PathBuf>,
    /// The crates a consumer must name directly because the unit exports attribute macros: rustc expands such a macro
    /// in the applying crate, where its output names the unit's own Rust crates. Empty unless the unit exports one.
    #[serde(default)]
    pub macro_externs: Vec<(String, PathBuf)>,
}

impl LoafNativeClosure {
    /// Resolve every relative path against the Loaf's `directory`; absolute paths stay as written.
    fn locate_in(&mut self, directory: &Path) {
        for (_, path) in self.externs.iter_mut().chain(self.macro_externs.iter_mut()) {
            *path = located(directory, path);
        }
        for path in &mut self.search {
            *path = located(directory, path);
        }
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
    /// Whether this Loaf owns `module` (`["pub", key, ..]` for a dependency, `["std", root, ..]` for a standard
    /// package).
    pub fn owns(&self, module: &[String]) -> bool {
        owns(&self.namespace, module)
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
    /// An artifact or native fact cache is missing, unreadable, malformed, or names another Loaf.
    #[error("cannot read the Loaf artifact {}: {reason}", path.display())]
    Unreadable {
        /// The artifact path.
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
    entries: BTreeMap<String, IndexedLoaf>,
    /// The ABI caches of the Rust crates the compilation links: the toolchain's standard crates and each compiled
    /// unit's.
    rust_abi_caches: Vec<Arc<RustAbiCache>>,
    /// The Rust crates the unit declares itself, read in the unit's session rather than from a standard Loaf's
    /// closure.
    declared_rust_crates: BTreeSet<String>,
    /// Bounded Rust-fact extractions for paths no complete cache holds, retained in the same lazy encoded form as
    /// shipped ABI caches instead of expanding every trait implementation into the session heap.
    rust_fact_caches: Vec<Arc<RustAbiCache>>,
    /// Direct facts used by focused frontend tests that do not run the ABI encoder.
    #[cfg(test)]
    rust_facts: BTreeMap<String, RustItemMetadata>,
    /// Paths the compiler asked rustc to describe that it described nothing at: no public item of the crate the unit
    /// selects has the path.
    absent_rust_paths: BTreeSet<String>,
    /// Items already resolved through the lookup order, decoded once and shared: the checker asks for the same
    /// items many times, and a trait's metadata carries every implementation it has.
    resolved_rust_items: Arc<Mutex<HashMap<String, Arc<RustItemMetadata>>>>,
    /// The first Rust ABI cache read that failed during a lookup, which `validate_demands` reports: a cache that
    /// cannot answer refuses the compilation instead of looking like a missing item.
    rust_abi_failure: Arc<OnceLock<LoafIndexError>>,
}

impl LoafIndex {
    /// Read an immutable Rust ABI index under current file-generation guards. The returned index owns no session or
    /// resolved source state; each compilation installs it into its own fresh lookup environment.
    pub fn read_rust_abi_cache(path: &Path) -> Result<Arc<RustAbiCache>, LoafIndexError> {
        if compiler_record_set::configured() {
            return RustAbiCache::open(path)
                .map(Arc::new)
                .map_err(|reason| LoafIndexError::Unreadable {
                    path: path.to_owned(),
                    reason,
                });
        }
        retained::abi(path).map_err(|reason| LoafIndexError::Unreadable {
            path: path.to_owned(),
            reason,
        })
    }

    /// Index `entries`, refusing two entries under one key.
    pub fn new(entries: impl IntoIterator<Item = LoafEntry>) -> Result<Self, LoafIndexError> {
        let mut indexed = BTreeMap::new();
        for entry in entries {
            let key = entry.key.clone();
            if indexed.insert(key.clone(), IndexedLoaf::admitted(entry)).is_some() {
                return Err(LoafIndexError::DuplicateKey { key });
            }
        }
        Ok(Self {
            entries: indexed,
            rust_abi_caches: Vec::new(),
            declared_rust_crates: BTreeSet::new(),
            rust_fact_caches: Vec::new(),
            #[cfg(test)]
            rust_facts: BTreeMap::new(),
            absent_rust_paths: BTreeSet::new(),
            resolved_rust_items: Arc::default(),
            rust_abi_failure: Arc::default(),
        })
    }

    /// Admit every location's inventory while deferring full manifests to canonical demand. Each inventory records
    /// all global contributors and shipped Rust paths; missing or stale private proofs require full admission.
    pub fn deferred(
        locations: impl IntoIterator<Item = (String, String, PathBuf, bool)>,
    ) -> Result<Self, LoafIndexError> {
        let mut index = Self::default();
        for (key, name, dir, standard) in locations {
            let namespace = if standard {
                let policy = STANDARD_PACKAGE_NAMESPACE_POLICIES
                    .iter()
                    .find(|policy| policy.package_name == name)
                    .ok_or_else(|| LoafIndexError::UnknownStandardPackage { package: name.clone() })?;
                LoafNamespace::Standard { policy }
            } else {
                LoafNamespace::Dependency { key: key.clone() }
            };
            let entry = IndexedLoaf::open(key.clone(), namespace, dir, &name)?;
            if index.entries.insert(key.clone(), entry).is_some() {
                return Err(LoafIndexError::DuplicateKey { key });
            }
        }
        Ok(index)
    }

    /// Canonical demand with explicit artifact failure, distinct from an absent namespace. Nullable checker lookup
    /// adapters retain failures in the entry; `validate_demands` must gate successful analysis and closure publication.
    pub fn demand(&self, key: &str) -> Result<Option<&LoafEntry>, LoafIndexError> {
        let Some(indexed) = self.entries.get(key) else {
            return Ok(None);
        };
        let entry = indexed.demand()?;
        Ok(Some(entry))
    }

    /// Refuse any failed demand, or a Rust ABI cache read that failed, before an analysis or executable closure can be
    /// accepted. Untouched entries stay lazy.
    pub fn validate_demands(&self) -> Result<(), LoafIndexError> {
        for entry in self.entries.values() {
            if let Some(error) = entry.failure() {
                return Err(error.clone());
            }
        }
        if let Some(error) = self.rust_abi_failure.get() {
            return Err(error.clone());
        }
        Ok(())
    }

    /// Declared keys without reading manifests or recording a semantic lookup.
    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.entries.keys().map(String::as_str)
    }

    /// Admitted identities without decoding complete manifests; package ambiguity is checked before loading bodies.
    pub fn identities(&self) -> impl Iterator<Item = (&str, &LoafIdentity)> {
        self.entries
            .iter()
            .map(|(key, entry)| (key.as_str(), &entry.summary.identity))
    }

    /// Admitted identities and their namespace ownership without decoding complete manifests. Consumers that need
    /// only graph or report headers must use this inventory view instead of demanding semantic declarations.
    pub fn admitted_identities(&self) -> impl Iterator<Item = (&str, &LoafIdentity, &LoafNamespace)> {
        self.entries
            .iter()
            .map(|(key, entry)| (key.as_str(), &entry.summary.identity, &entry.namespace))
    }

    /// Whether a compiler-pinned standard package owns a canonical package identity.
    pub fn is_standard_package(&self, package: &str) -> bool {
        self.entries.values().any(|entry| {
            matches!(entry.namespace, LoafNamespace::Standard { .. }) && entry.summary.identity.name == package
        })
    }

    /// Add the ABI cache of linked Rust crates; it answers after the Loaves' own shipped ABI.
    pub fn add_rust_abi_cache(&mut self, cache: Arc<RustAbiCache>) {
        self.rust_abi_caches.push(cache);
        self.forget_resolved_rust_items();
    }

    /// Paths whose manifest-authenticated version-10 cache was skipped and must be extracted by this compiler.
    pub fn stale_shipped_rust_paths(&self) -> BTreeSet<String> {
        self.entries
            .values()
            .filter(|entry| entry.summary.rust_abi_cache_declared && !entry.summary.rust_abi_cache)
            .flat_map(|entry| entry.summary.rust_paths.iter().cloned())
            .collect()
    }

    /// Name the Rust crates the unit declares. Private indexed snapshots do not answer for them, so their facts come
    /// from the unit's own caches and extraction. Explicit dependency reexports retain their declared ABI.
    pub fn declare_rust_crates(&mut self, crates: impl IntoIterator<Item = String>) {
        self.declared_rust_crates = crates.into_iter().collect();
        self.forget_resolved_rust_items();
    }

    /// Add one encoded bounded Rust-fact extraction for `requested`; it answers after complete ABI caches. A requested
    /// path the extraction does not contain is absent from the selected crate's public API.
    pub fn add_rust_fact_cache(&mut self, requested: &BTreeSet<String>, facts: Arc<RustAbiCache>) {
        self.absent_rust_paths
            .extend(requested.iter().filter(|path| !facts.contains(path)).cloned());
        self.rust_fact_caches.push(facts);
        self.forget_resolved_rust_items();
    }

    /// Add direct facts for a focused frontend test without introducing cache-format setup into that test.
    #[cfg(test)]
    pub fn add_rust_facts(&mut self, requested: &BTreeSet<String>, facts: BTreeMap<String, RustItemMetadata>) {
        self.absent_rust_paths
            .extend(requested.iter().filter(|path| !facts.contains_key(*path)).cloned());
        self.rust_facts.extend(facts);
        self.forget_resolved_rust_items();
    }

    /// Whether rustc, asked to describe `canonical_path`, described nothing there. A path rustc was never asked about
    /// is not absent: it has no facts either way.
    pub fn rust_path_is_absent(&self, canonical_path: &str) -> bool {
        self.absent_rust_paths.contains(canonical_path)
    }

    /// Drop resolved items after a source is added, so the lookup order decides again; this index gets its own
    /// store instead of clearing one that a clone shares.
    fn forget_resolved_rust_items(&mut self) {
        self.resolved_rust_items = Arc::default();
    }

    /// The entry for one dependency, read from its artifact directory.
    pub fn dependency(key: &str, name: &str, dir: &Path) -> Result<LoafEntry, LoafIndexError> {
        Ok(LoafEntry {
            key: key.to_owned(),
            namespace: LoafNamespace::Dependency { key: key.to_owned() },
            artifact: LoafArtifact::read(dir, name)?,
        })
    }

    /// The entry for one standard package, read from its artifact directory.
    pub fn standard(package: &str, dir: &Path) -> Result<LoafEntry, LoafIndexError> {
        let policy = STANDARD_PACKAGE_NAMESPACE_POLICIES
            .iter()
            .find(|policy| policy.package_name == package)
            .ok_or_else(|| LoafIndexError::UnknownStandardPackage {
                package: package.to_owned(),
            })?;
        Ok(LoafEntry {
            key: policy.package_name.to_owned(),
            namespace: LoafNamespace::Standard { policy },
            artifact: LoafArtifact::read(dir, policy.package_name)?,
        })
    }

    /// Every entry, in key order.
    pub fn entries(&self) -> impl Iterator<Item = &LoafEntry> {
        self.entries.values().filter_map(|entry| entry.demand().ok())
    }

    /// Read the provider-operation environment as a manifest contract, retaining every nonempty contributor. Empty
    /// inventories contribute no facts; deciding which operations execute remains with checked Body IR.
    pub fn provider_operation_entries(&self) -> impl Iterator<Item = &LoafEntry> {
        self.entries
            .values()
            .filter(|entry| entry.summary.provider_operations)
            .filter_map(|entry| entry.demand().ok())
    }

    /// Read the native-union catalog without silently dropping an unimported producer's layout contract. Lowering
    /// selects from this existing environment; reachability reuse must retain every nonempty contributor.
    pub fn native_union_entries(&self) -> impl Iterator<Item = &LoafEntry> {
        self.entries
            .values()
            .filter(|entry| entry.summary.native_unions)
            .filter_map(|entry| entry.demand().ok())
    }

    /// The entry indexed as `key`.
    pub fn entry(&self, key: &str) -> Option<&LoafEntry> {
        self.demand(key).ok().flatten()
    }

    /// The one entry owning `module`, if any.
    pub fn owner_of(&self, module: &[String]) -> Option<&LoafEntry> {
        let entry = self.entries.values().find(|entry| entry.owns(module))?;
        self.entry(&entry.key)
    }

    /// The Loaf that declares `canonical` (a package identity) among its public exports.
    ///
    /// Every Loaf a compilation reaches is indexed, transitive ones included, so the declaring Loaf is found by the
    /// identity's package and its identity graph rather than by walking import routes.
    pub fn declaring_loaf(&self, canonical: &CanonicalSymbolId) -> Option<&LoafEntry> {
        let SymbolOrigin::Package { library, .. } = &canonical.origin else {
            return None;
        };
        let entry = self
            .entries
            .values()
            .filter(|entry| &entry.summary.identity.name == library)
            .filter_map(|entry| entry.demand().ok())
            .find(|entry| {
                &entry.artifact.manifest.name == library
                    && entry
                        .artifact
                        .manifest
                        .contract_metadata
                        .identity_graph
                        .exports
                        .iter()
                        .any(|export| export.canonical.as_ref().is_some_and(|id| id.represents(canonical)))
            })?;
        Some(entry)
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
            .filter(|export| export.canonical.as_ref().is_some_and(|id| id.represents(canonical)))
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
        self.entry(key).map(|entry| entry.artifact.manifest.as_ref())
    }

    /// Rust item metadata for `canonical_path`: a Loaf's shipped manifest ABI first (in key order), then the linked
    /// crates' ABI caches, then facts read at check time. A standard Loaf's shipped facts do not answer for a crate the
    /// unit declares. Each item is decoded once and then shared.
    pub fn rust_abi_item(&self, canonical_path: &str) -> Option<Arc<RustItemMetadata>> {
        if let Some(resolved) = self.resolved_rust_items.lock().ok()?.get(canonical_path) {
            return Some(Arc::clone(resolved));
        }
        let resolved = Arc::new(self.resolve_rust_fact(canonical_path, Clone::clone, RustAbiCache::get)?);
        if let Ok(mut items) = self.resolved_rust_items.lock() {
            items.insert(canonical_path.to_owned(), Arc::clone(&resolved));
        }
        Some(resolved)
    }

    /// Alias-only Rust metadata for `canonical_path`, following the full-item lookup order without expanding a cache
    /// record's methods, trait implementations, fields or variants.
    ///
    /// A full item already resolved for another purpose remains the authority. Otherwise, encoded caches project the
    /// two alias fields directly and create no parallel memo or second metadata table.
    pub(crate) fn rust_type_alias(&self, canonical_path: &str) -> Option<RustTypeAliasLookup> {
        if let Some(resolved) = self.resolved_rust_items.lock().ok()?.get(canonical_path) {
            return Some(RustTypeAliasLookup::from_item(resolved));
        }
        self.resolve_rust_fact(
            canonical_path,
            RustTypeAliasLookup::from_item,
            RustAbiCache::get_type_alias,
        )
    }

    /// Resolve one projection through the common Rust-fact lookup order, without adding a parallel memo or provider
    /// traversal. A private indexed snapshot belongs to its producer's closure, where visible paths and
    /// implementations can differ from the unit's. For a crate the unit declares, only explicit dependency reexports
    /// precede the unit's own caches and extraction.
    fn resolve_rust_fact<T>(
        &self,
        canonical_path: &str,
        project_item: impl Fn(&RustItemMetadata) -> T,
        read_cache: impl Fn(&RustAbiCache, &str) -> Result<Option<T>, String>,
    ) -> Option<T> {
        let root = canonical_path.split("::").next().unwrap_or(canonical_path);
        let declared = self.declared_rust_crates.contains(root);
        let shipping = || {
            self.entries
                .values()
                .filter(move |entry| !(declared && matches!(entry.namespace, LoafNamespace::Standard { .. })))
        };
        // A Loaf whose demand fails is skipped here; `validate_demands` reports its retained failure.
        for entry in shipping().filter(|entry| entry.summary.rust_paths.contains(canonical_path)) {
            let Ok(entry) = entry.demand() else {
                continue;
            };
            if let Some(item) = entry
                .artifact
                .manifest
                .rust_abi
                .as_ref()
                .and_then(|abi| abi.get(canonical_path))
            {
                return Some(project_item(item));
            }
        }
        for entry in shipping().filter(|entry| !declared && entry.summary.rust_abi_cache) {
            let Ok(entry) = entry.demand() else {
                continue;
            };
            let Some(cache) = entry.artifact.rust_abi_cache.as_ref() else {
                continue;
            };
            if let Some(item) = self.cache_fact(read_cache(cache, canonical_path), || entry.artifact.dir.clone()) {
                return Some(item);
            }
        }
        for cache in &self.rust_abi_caches {
            if let Some(item) = self.cache_fact(read_cache(cache, canonical_path), || {
                PathBuf::from("the toolchain Rust ABI cache")
            }) {
                return Some(item);
            }
        }
        for cache in &self.rust_fact_caches {
            if let Some(item) = self.cache_fact(read_cache(cache, canonical_path), || {
                PathBuf::from("the bounded Rust facts cache")
            }) {
                return Some(item);
            }
        }
        #[cfg(test)]
        if let Some(item) = self.rust_facts.get(canonical_path) {
            return Some(project_item(item));
        }
        None
    }

    /// Admit one projected cache read while retaining a failure for [`Self::validate_demands`].
    fn cache_fact<T>(&self, result: Result<Option<T>, String>, origin: impl FnOnce() -> PathBuf) -> Option<T> {
        match result {
            Ok(item) => item,
            Err(reason) => {
                let _ = self
                    .rust_abi_failure
                    .set(LoafIndexError::Unreadable { path: origin(), reason });
                None
            }
        }
    }

    /// Keyword registrations the indexed Loaves export, keyed by Loaf, for the consumer's parser.
    pub fn imported_vocab(&self) -> crate::parser::ImportedLibraryVocab {
        let mut map = std::collections::HashMap::new();
        for (key, entry) in &self.entries {
            if !entry.summary.vocabulary {
                continue;
            }
            let Some(entry) = self.entry(key) else {
                continue;
            };
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
            if !entry.summary.vocabulary {
                continue;
            }
            let Some(entry) = self.entry(key) else {
                continue;
            };
            if let Some(vocab) = &entry.artifact.manifest.vocab
                && !vocab.dsl_surfaces.is_empty()
            {
                map.insert(key.clone(), vocab.dsl_surfaces.clone());
            }
        }
        map
    }
}

/// Namespace ownership depends only on the unit's declared binding and compiler-pinned standard-package policy.
fn owns(namespace: &LoafNamespace, module: &[String]) -> bool {
    match (namespace, module) {
        (LoafNamespace::Dependency { key }, [root, name, ..]) => root == "pub" && name == key,
        (LoafNamespace::Standard { policy }, [root, name, ..]) => {
            root == "std" && policy.namespace_roots.contains(&name.as_str())
        }
        _ => false,
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

#[cfg(test)]
mod scoped_name_tests {
    use super::{LoafArtifact, LoafIdentity, LoafIndex, LoafNamespace};
    use crate::library_manifest::LibraryManifest;
    use crate::loaf_index::lazy_entry::IndexedLoaf;
    use incan_loaf_names::artifact_stem;
    use std::collections::BTreeSet;

    /// The ordinary retained reader opens encoded filenames while retaining the exact scoped semantic owner.
    #[test]
    fn scoped_artifacts_retain_their_producer_name() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        for name in ["alpha/catalog", "beta/catalog", "scope-a/catalog", "scope/a-catalog"] {
            let manifest = LibraryManifest::new(name, "1.0.0");
            let stem = artifact_stem(name);
            let path = directory.path().join(format!("{stem}.incnlib"));
            std::fs::write(&path, manifest.to_json_string()?)?;
            for _ in 0..2 {
                let artifact = LoafArtifact::read(directory.path(), name)?;
                assert_eq!(artifact.manifest.name, name);
                assert_eq!(artifact.identity.name, name);
                assert_eq!(artifact.manifest_path(), path);
                assert_eq!(
                    artifact.bodies_path(),
                    directory.path().join(format!("{stem}.incnbody"))
                );
                assert_eq!(
                    artifact.native_closure_path(),
                    directory.path().join(format!("{stem}.native.json"))
                );
            }
        }
        Ok(())
    }

    /// Header-only consumers observe admitted identity records without demanding declaration manifests.
    #[test]
    fn admitted_identities_keep_manifests_lazy() -> Result<(), Box<dyn std::error::Error>> {
        let key = "catalog";
        let identity = LoafIdentity {
            name: "alpha/catalog".to_owned(),
            version: "1.0.0".to_owned(),
            digest: "admitted-digest".to_owned(),
            feature_projection: BTreeSet::new(),
        };
        let indexed = IndexedLoaf::summary_only(
            key.to_owned(),
            LoafNamespace::Dependency { key: key.to_owned() },
            identity.clone(),
        );
        let observation = indexed.clone();
        let mut index = LoafIndex::default();
        index.entries.insert(key.to_owned(), indexed);

        let entries = index.admitted_identities().collect::<Vec<_>>();

        assert_eq!(entries.len(), 1);
        let (observed_key, observed_identity, observed_namespace) = entries
            .first()
            .copied()
            .ok_or("the admitted identity projection is empty")?;
        assert_eq!(observed_key, key);
        assert_eq!(observed_identity, &identity);
        assert_eq!(observed_namespace, &LoafNamespace::Dependency { key: key.to_owned() });
        assert!(!observation.was_demanded());
        Ok(())
    }
}
