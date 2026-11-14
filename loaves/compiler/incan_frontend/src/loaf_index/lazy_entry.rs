//! Demand loading behind an admitted, generation-bound inventory. Summaries contain only facts derived from the
//! ordinary manifest reader; an unavailable summary takes that reader again rather than guessing at absent facts.

use incan_loaf_names::artifact_stem;
use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::decoded_manifest::compiler_generation;
use super::manifest_input::{self, Generation, generation};
use super::{LoafArtifact, LoafEntry, LoafIdentity, LoafIndexError, LoafNamespace};
use crate::library_manifest::compiler_records::RecordFamily;
use crate::library_manifest::declaration_records::{DeclarationFacet, declaration_facts};
use crate::library_manifest::executable_surface::{ExecutableSurface, runtime_bindings};
use incan_semantics_core::body_ir::RuntimeCallableBinding;

/// Maximum encoded inventory proof size; larger optional records fall back to ordinary manifest admission.
const MAX_INVENTORY_RECORD_BYTES: u64 = 4 * 1024 * 1024;

/// Demand-independent inventory derived from an admitted manifest, including every global contributor.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct Summary {
    pub identity: LoafIdentity,
    pub vocabulary: bool,
    pub provider_operations: bool,
    pub native_unions: bool,
    pub rust_paths: BTreeSet<String>,
    /// Whether the manifest names an external native ABI cache, independently of this compiler accepting its layout.
    #[serde(default)]
    pub rust_abi_cache_declared: bool,
    /// Whether admission validated an external indexed native ABI asset for this Loaf.
    pub rust_abi_cache: bool,
}

impl Summary {
    /// Observe the admitted manifest itself, never source imports or an assumed negative lookup.
    fn from_artifact(artifact: &LoafArtifact) -> Self {
        let manifest = &artifact.manifest;
        Self {
            identity: artifact.identity.clone(),
            vocabulary: manifest.vocab.is_some(),
            provider_operations: !manifest.contract_metadata.provider.operation_descriptors.is_empty(),
            native_unions: !manifest.contract_metadata.native_unions.is_empty(),
            rust_paths: manifest
                .rust_abi
                .as_ref()
                .map(|abi| {
                    abi.items
                        .iter()
                        .map(|item| item.canonical_path.clone())
                        .collect::<BTreeSet<_>>()
                })
                .unwrap_or_default()
                .into_iter()
                .chain(artifact.stale_rust_paths.iter().cloned())
                .collect(),
            rust_abi_cache_declared: manifest
                .rust_abi
                .as_ref()
                .and_then(|abi| abi.cache_digest.as_ref())
                .is_some(),
            rust_abi_cache: artifact.rust_abi_cache.is_some(),
        }
    }
}

/// One indexed location whose full manifest is read at most once, with a retained failure as well as success.
#[derive(Debug, Clone)]
pub(super) struct IndexedLoaf {
    pub key: String,
    pub namespace: LoafNamespace,
    pub dir: PathBuf,
    pub summary: Summary,
    entry: Arc<OnceLock<Result<LoafEntry, LoafIndexError>>>,
    execution: Arc<OnceLock<Result<Arc<crate::library_manifest::LibraryManifest>, LoafIndexError>>>,
    surface: Arc<OnceLock<Result<Arc<ExecutableSurface>, LoafIndexError>>>,
    bindings: Arc<OnceLock<Result<Arc<Vec<RuntimeCallableBinding>>, LoafIndexError>>>,
    /// The records read by family and name, kept for the rest of the compilation so each is read once; `None` where
    /// the installed set holds no such record.
    named: Arc<Mutex<NamedRecords>>,
}

/// Records read by name, keyed by family and name.
type NamedRecords = HashMap<(&'static str, String), Option<Arc<Vec<serde_json::Value>>>>;

impl IndexedLoaf {
    /// Wrap a caller's already admitted entry without reopening its files.
    pub fn admitted(entry: LoafEntry) -> Self {
        let indexed = Self {
            key: entry.key.clone(),
            namespace: entry.namespace.clone(),
            dir: entry.artifact.dir.clone(),
            summary: Summary::from_artifact(&entry.artifact),
            entry: Arc::default(),
            execution: Arc::default(),
            surface: Arc::default(),
            bindings: Arc::default(),
            named: Arc::default(),
        };
        let _ = indexed.entry.set(Ok(entry));
        indexed
    }

    /// Construct an inventory-only entry for tests that prove a metadata projection preserves demand loading.
    #[cfg(test)]
    pub(super) fn summary_only(key: String, namespace: LoafNamespace, identity: LoafIdentity) -> Self {
        Self {
            key,
            namespace,
            dir: PathBuf::new(),
            summary: Summary {
                identity,
                vocabulary: false,
                provider_operations: false,
                native_unions: false,
                rust_paths: BTreeSet::new(),
                rust_abi_cache_declared: false,
                rust_abi_cache: false,
            },
            entry: Arc::default(),
            execution: Arc::default(),
            surface: Arc::default(),
            bindings: Arc::default(),
            named: Arc::default(),
        }
    }

    /// Whether a focused test caused this inventory-only entry to demand its complete manifest.
    #[cfg(test)]
    pub(super) fn was_demanded(&self) -> bool {
        self.entry.get().is_some()
    }

    /// Admit a small inventory, or use full admission when its private proof is missing or stale.
    pub fn open(key: String, namespace: LoafNamespace, dir: PathBuf, name: &str) -> Result<Self, LoafIndexError> {
        let path = dir.join(format!("{}.incnlib", artifact_stem(name)));
        if super::compiler_record_set::configured()
            && let Some(summary) = super::compiler_record_set::summary(&dir, name)?
        {
            return Ok(Self {
                key,
                namespace,
                dir,
                summary,
                entry: Arc::default(),
                execution: Arc::default(),
                surface: Arc::default(),
                bindings: Arc::default(),
                named: Arc::default(),
            });
        }
        if !super::compiler_record_set::configured()
            && let Some(summary) = retained(&path)
            && summary.identity.name == name
        {
            return Ok(Self {
                key,
                namespace,
                dir,
                summary,
                entry: Arc::default(),
                execution: Arc::default(),
                surface: Arc::default(),
                bindings: Arc::default(),
                named: Arc::default(),
            });
        }
        let before = observation(&path);
        let artifact = LoafArtifact::read(&dir, name)?;
        let indexed = Self::admitted(LoafEntry {
            key,
            namespace,
            artifact,
        });
        if let Some(before) = before
            && observation(&path).as_ref() == Some(&before)
        {
            let _ = retain(&path, before, &indexed.summary);
        }
        Ok(indexed)
    }

    /// Load the exact admitted owner on demand. A file changed since inventory admission refuses this compilation.
    pub fn demand(&self) -> Result<&LoafEntry, LoafIndexError> {
        self.entry
            .get_or_init(|| {
                let artifact = LoafArtifact::read(&self.dir, &self.summary.identity.name)?;
                if artifact.identity != self.summary.identity {
                    return Err(LoafIndexError::Unreadable {
                        path: artifact.manifest_path(),
                        reason: "the manifest changed after its inventory was admitted".to_owned(),
                    });
                }
                Ok(LoafEntry {
                    key: self.key.clone(),
                    namespace: self.namespace.clone(),
                    artifact,
                })
            })
            .as_ref()
            .map_err(Clone::clone)
    }

    /// Read the record projection needed to admit executable declarations without demanding the complete manifest.
    pub fn execution_manifest(&self) -> Result<Arc<crate::library_manifest::LibraryManifest>, LoafIndexError> {
        self.execution
            .get_or_init(|| {
                if let Some(entry) = self.entry.get() {
                    return entry
                        .as_ref()
                        .map(|entry| Arc::clone(&entry.artifact.manifest))
                        .map_err(Clone::clone);
                }
                if super::compiler_record_set::configured()
                    && let Some((manifest, digest)) =
                        super::compiler_record_set::read_execution(&self.dir, &self.summary.identity.name)?
                {
                    if digest != self.summary.identity.digest {
                        return Err(LoafIndexError::Unreadable {
                            path: self
                                .dir
                                .join(format!("{}.incnlib", artifact_stem(&self.summary.identity.name))),
                            reason: "executable record projection differs from its admitted inventory".to_owned(),
                        });
                    }
                    return Ok(manifest);
                }
                Ok(Arc::clone(&self.demand()?.artifact.manifest))
            })
            .as_ref()
            .map(Arc::clone)
            .map_err(Clone::clone)
    }

    /// What executable resolution reads of this Loaf: from its own records when the installed set publishes them,
    /// otherwise derived from the executable projection.
    pub fn executable_surface(&self) -> Result<Arc<ExecutableSurface>, LoafIndexError> {
        self.surface
            .get_or_init(|| {
                if super::compiler_record_set::configured()
                    && let Some((surface, digest)) =
                        super::compiler_record_set::read_surface(&self.dir, &self.summary.identity.name)?
                {
                    if digest != self.summary.identity.digest {
                        return Err(LoafIndexError::Unreadable {
                            path: self
                                .dir
                                .join(format!("{}.incnlib", artifact_stem(&self.summary.identity.name))),
                            reason: "executable surface records differ from their admitted inventory".to_owned(),
                        });
                    }
                    return Ok(surface);
                }
                let manifest = self.execution_manifest()?;
                Ok(Arc::new(ExecutableSurface::of(&manifest)))
            })
            .as_ref()
            .map(Arc::clone)
            .map_err(Clone::clone)
    }

    /// The runtime callables native lowering binds in this Loaf: its runtime-bindings record, read by name, when the
    /// installed set publishes one, otherwise derived from the executable projection.
    pub fn runtime_bindings(&self) -> Result<Arc<Vec<RuntimeCallableBinding>>, LoafIndexError> {
        self.bindings
            .get_or_init(|| {
                if super::compiler_record_set::configured()
                    && let Some((bindings, digest)) =
                        super::compiler_record_set::read_runtime_bindings(&self.dir, &self.summary.identity.name)?
                {
                    if digest != self.summary.identity.digest {
                        return Err(LoafIndexError::Unreadable {
                            path: self
                                .dir
                                .join(format!("{}.incnlib", artifact_stem(&self.summary.identity.name))),
                            reason: "runtime bindings records differ from their admitted inventory".to_owned(),
                        });
                    }
                    return Ok(bindings);
                }
                Ok(Arc::new(runtime_bindings(&*self.execution_manifest()?)))
            })
            .as_ref()
            .map(Arc::clone)
            .map_err(Clone::clone)
    }

    /// One declaration's signature, layout or defaults, by its module-qualified name: its own records when the
    /// installed set publishes them, otherwise projected from the executable projection's checked API. An overloaded
    /// name has one value per overload.
    ///
    /// A set that publishes declaration records holds one for every fact a declaration has, so a missing record there
    /// is a fact the declaration does not have, such as the defaults of a function without parameters; only a set
    /// converted before declarations had records is projected.
    pub fn declaration_facts(
        &self,
        facet: DeclarationFacet,
        declaration: &str,
    ) -> Result<Vec<serde_json::Value>, LoafIndexError> {
        let family = RecordFamily::of_declaration(facet).as_str();
        if super::compiler_record_set::configured() {
            if let Some(values) = self.named_records(family, declaration)? {
                return Ok(values.as_ref().clone());
            }
            if self.names_record(&[RecordFamily::DeclarationSignature.as_str()], None)? == Some(true) {
                return Ok(Vec::new());
            }
        }
        let manifest = self.execution_manifest()?;
        let Some(api) = manifest.contract_metadata.split_documentation().0.api else {
            return Ok(Vec::new());
        };
        let facts = declaration_facts(&api).map_err(|error| LoafIndexError::Unreadable {
            path: self
                .dir
                .join(format!("{}.incnlib", artifact_stem(&self.summary.identity.name))),
            reason: format!("cannot project its declarations: {error}"),
        })?;
        Ok(facts
            .into_iter()
            .filter(|fact| fact.facet == facet && fact.name == declaration)
            .map(|fact| fact.value)
            .collect())
    }

    /// One public namespace by its path, from its own record. `None` when the installed set holds no namespace record
    /// at that path, including a set converted before namespaces had records of their own and every Loaf outside
    /// records mode; such a caller reads the namespace from the manifest.
    pub fn public_namespace(&self, path: &[String]) -> Result<Option<serde_json::Value>, LoafIndexError> {
        if !super::compiler_record_set::configured() {
            return Ok(None);
        }
        let Some(values) = self.named_records(RecordFamily::PublicNamespace.as_str(), &path.join("::"))? else {
            return Ok(None);
        };
        match values.as_slice() {
            [namespace] => Ok(Some(namespace.clone())),
            _ => Err(self.inventory_mismatch("a public namespace record differs from its admitted inventory")),
        }
    }

    /// The `family` records named `name`, read from the installed set the first time they are asked for and kept for
    /// the rest of the compilation; `None` when the set holds no such record.
    fn named_records(
        &self,
        family: &'static str,
        name: &str,
    ) -> Result<Option<Arc<Vec<serde_json::Value>>>, LoafIndexError> {
        let mut kept = self.named.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(found) = kept.get(&(family, name.to_owned())) {
            return Ok(found.clone());
        }
        let read = super::compiler_record_set::read_declaration(&self.dir, &self.summary.identity.name, family, name)?;
        let values = match read {
            Some((_, digest)) if digest != self.summary.identity.digest => {
                return Err(self.inventory_mismatch("records read by name differ from their admitted inventory"));
            }
            Some((values, _)) => Some(Arc::new(values)),
            None => None,
        };
        kept.insert((family, name.to_owned()), values.clone());
        Ok(values)
    }

    /// The refusal of a record that does not belong to the inventory the index admitted this Loaf by.
    fn inventory_mismatch(&self, reason: &str) -> LoafIndexError {
        LoafIndexError::Unreadable {
            path: self
                .dir
                .join(format!("{}.incnlib", artifact_stem(&self.summary.identity.name))),
            reason: reason.to_owned(),
        }
    }

    /// Whether the installed set holds a record of one of `families` for this Loaf, named `record` when one is given,
    /// without opening any value; `None` outside records mode.
    pub fn names_record(&self, families: &[&str], record: Option<&str>) -> Result<Option<bool>, LoafIndexError> {
        if !super::compiler_record_set::configured() {
            return Ok(None);
        }
        super::compiler_record_set::names_record(&self.dir, &self.summary.identity.name, families, record)
    }

    /// The governed bodies sidecar can be located from admitted identity without demanding declaration records.
    pub fn bodies_path(&self) -> PathBuf {
        self.dir
            .join(format!("{}.incnbody", artifact_stem(&self.summary.identity.name)))
    }

    /// Report a failed demand without opening an untouched manifest.
    pub fn failure(&self) -> Option<&LoafIndexError> {
        self.entry.get().and_then(|result| result.as_ref().err())
    }

    /// Resolve namespace ownership from the index's declared binding, before reading manifest contents.
    pub fn owns(&self, module: &[String]) -> bool {
        super::owns(&self.namespace, module)
    }
}

/// Exact source and compiler generations bound to an optional private summary.
#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
struct Observation {
    path: PathBuf,
    source: Generation,
    compiler: (PathBuf, Generation),
}

/// A separately checksummed small inventory, with no executable or acceptance result.
#[derive(Serialize, Deserialize)]
struct Record {
    version: u32,
    observation: Observation,
    summary: Summary,
    checksum: String,
}

/// Observe the exact file and running compiler; unsupported filesystems keep full admission.
fn observation(path: &Path) -> Option<Observation> {
    let compiler = compiler_generation()?;
    Some(Observation {
        path: path.canonicalize().ok()?,
        source: generation(&fs::metadata(path).ok()?)?,
        compiler: compiler.clone(),
    })
}

/// Place the inventory beside existing optional identity proofs in the explicitly selected compiler home.
fn record_path(path: &Path) -> Option<PathBuf> {
    let canonical = path.canonicalize().ok()?;
    let key = format!("{:x}", Sha256::digest(canonical.as_os_str().as_encoded_bytes()));
    Some(manifest_input::cache_path(&key)?.with_extension("inventory"))
}

/// Refuse stale, malformed, oversized or corrupted records and fall back to full public admission.
fn retained(path: &Path) -> Option<Summary> {
    let observed = observation(path)?;
    let cache = record_path(path)?;
    let metadata = fs::symlink_metadata(&cache).ok()?;
    if !metadata.is_file() || metadata.len() > MAX_INVENTORY_RECORD_BYTES {
        return None;
    }
    let mut bytes = Vec::new();
    fs::File::open(cache)
        .ok()?
        .take(MAX_INVENTORY_RECORD_BYTES + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if u64::try_from(bytes.len()).ok()? > MAX_INVENTORY_RECORD_BYTES {
        return None;
    }
    let record: Record = serde_json::from_slice(&bytes).ok()?;
    if record.version != 1
        || record.observation != observed
        || record.checksum != checksum(&record.observation, &record.summary).ok()?
        || observation(path).as_ref() != Some(&observed)
    {
        return None;
    }
    Some(record.summary)
}

/// Bind both the positive and negative inventory facts to their complete observation.
fn checksum(observed: &Observation, summary: &Summary) -> std::io::Result<String> {
    let bytes = serde_json::to_vec(&(1_u32, observed, summary)).map_err(std::io::Error::other)?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

/// Atomically publish an optional inventory only after ordinary manifest admission has succeeded.
fn retain(path: &Path, observation: Observation, summary: &Summary) -> std::io::Result<()> {
    let Some(cache) = record_path(path) else {
        return Ok(());
    };
    let record = Record {
        version: 1,
        checksum: checksum(&observation, summary)?,
        observation,
        summary: summary.clone(),
    };
    let bytes = serde_json::to_vec(&record).map_err(std::io::Error::other)?;
    super::publish_private_record(&cache, &bytes, "inventory-pending")
}
