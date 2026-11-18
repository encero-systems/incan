//! Demand loading behind an admitted, generation-bound inventory. Summaries contain only facts derived from the
//! ordinary manifest reader; an unavailable summary takes that reader again rather than guessing at absent facts.

use incan_loaf_names::artifact_stem;
use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::decoded_manifest::compiler_generation;
use super::manifest_input::{self, Generation, generation};
use super::{LoafArtifact, LoafEntry, LoafIdentity, LoafIndexError, LoafNamespace};
use crate::library_manifest::VocabExports;
use crate::library_manifest::capabilities::Contributions;
use crate::library_manifest::compiler_records::RecordFamily;
use crate::library_manifest::declaration_records::{DeclarationFacet, declaration_facts};
use crate::library_manifest::executable_surface::{ExecutableSurface, runtime_bindings};
use crate::library_manifest::type_scope::ModuleTypeScope;
use crate::rust_abi_cache::RustAbiCache;
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
    /// The digest of the native ABI cache the manifest names, so the cache is read and authenticated without the
    /// manifest. A summary recorded before it carried the digest has none; the manifest then names the cache.
    #[serde(default)]
    pub rust_abi_cache_digest: Option<String>,
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
            rust_abi_cache_digest: manifest.rust_abi.as_ref().and_then(|abi| abi.cache_digest.clone()),
        }
    }
}

/// One indexed location whose full manifest is read at most once, with a retained failure as well as success.
#[derive(Debug, Clone)]
pub(super) struct IndexedLoaf {
    pub key: String,
    pub namespace: LoafNamespace,
    pub dir: PathBuf,
    pub name: String,
    pub globals: Contributions,
    initial_summary: Option<Summary>,
    selected_summary: Arc<OnceLock<Result<Summary, LoafIndexError>>>,
    entry: Arc<OnceLock<Result<LoafEntry, LoafIndexError>>>,
    execution: Arc<OnceLock<Result<Arc<crate::library_manifest::LibraryManifest>, LoafIndexError>>>,
    surface: Arc<OnceLock<Result<Arc<ExecutableSurface>, LoafIndexError>>>,
    vocabulary: Arc<OnceLock<Result<Option<Arc<VocabExports>>, LoafIndexError>>>,
    bindings: Arc<OnceLock<Result<Arc<Vec<RuntimeCallableBinding>>, LoafIndexError>>>,
    /// The shipped native ABI cache, read by the digest the summary names when the manifest is not demanded.
    rust_abi: Arc<OnceLock<Result<Option<Arc<RustAbiCache>>, LoafIndexError>>>,
    /// The records read by family and name, kept for the rest of the compilation so each is read once; `None` where
    /// the installed set holds no such record.
    named: Arc<Mutex<NamedRecords>>,
}

/// Records read by name, keyed by family and name.
type NamedRecords = HashMap<(&'static str, String), Option<Arc<Vec<serde_json::Value>>>>;

impl IndexedLoaf {
    fn new(
        key: String,
        namespace: LoafNamespace,
        dir: PathBuf,
        name: String,
        globals: Contributions,
        initial_summary: Option<Summary>,
    ) -> Self {
        Self {
            key,
            namespace,
            dir,
            name,
            globals,
            initial_summary,
            selected_summary: Arc::default(),
            entry: Arc::default(),
            execution: Arc::default(),
            surface: Arc::default(),
            bindings: Arc::default(),
            vocabulary: Arc::default(),
            rust_abi: Arc::default(),
            named: Arc::default(),
        }
    }

    pub(super) fn known(key: String, namespace: LoafNamespace, dir: PathBuf, summary: Summary) -> Self {
        let globals = Contributions {
            vocabulary: summary.vocabulary,
            provider_operations: summary.provider_operations,
        };
        Self::new(
            key,
            namespace,
            dir,
            summary.identity.name.clone(),
            globals,
            Some(summary),
        )
    }

    pub fn contributor(
        key: String,
        namespace: LoafNamespace,
        dir: PathBuf,
        name: String,
        globals: Contributions,
    ) -> Self {
        Self::new(key, namespace, dir, name, globals, None)
    }

    /// Only an actual identity, Rust ABI or declaration request needs the package's individual metadata record.
    pub fn summary(&self) -> Result<&Summary, LoafIndexError> {
        if let Some(summary) = &self.initial_summary {
            return Ok(summary);
        }
        self.selected_summary
            .get_or_init(|| {
                let summary = super::compiler_record_set::summary(&self.dir, &self.name)?
                    .ok_or_else(|| self.inventory_mismatch("contributor package has no installed metadata"))?;
                if summary.vocabulary != self.globals.vocabulary
                    || summary.provider_operations != self.globals.provider_operations
                {
                    return Err(self.inventory_mismatch("package metadata disagrees with global contributors"));
                }
                Ok(summary)
            })
            .as_ref()
            .map_err(Clone::clone)
    }

    /// An indexed installed package has an already verified ABI cache by construction. Only a legacy artifact
    /// admitted with a stale cache can contribute the paths its compiling unit must re-extract.
    pub fn stale_rust_paths(&self) -> impl Iterator<Item = &String> {
        self.initial_summary
            .iter()
            .filter(|s| s.rust_abi_cache_declared && !s.rust_abi_cache)
            .flat_map(|s| s.rust_paths.iter())
    }

    pub fn admitted(entry: LoafEntry) -> Self {
        let indexed = Self::known(
            entry.key.clone(),
            entry.namespace.clone(),
            entry.artifact.dir.clone(),
            Summary::from_artifact(&entry.artifact),
        );
        let _ = indexed.entry.set(Ok(entry));
        indexed
    }

    #[cfg(test)]
    pub(super) fn summary_only(key: String, namespace: LoafNamespace, identity: LoafIdentity) -> Self {
        Self::known(
            key,
            namespace,
            PathBuf::new(),
            Summary {
                identity,
                vocabulary: false,
                provider_operations: false,
                native_unions: false,
                rust_paths: BTreeSet::new(),
                rust_abi_cache_declared: false,
                rust_abi_cache: false,
                rust_abi_cache_digest: None,
            },
        )
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
            return Ok(Self::known(key, namespace, dir, summary));
        }
        if !super::compiler_record_set::configured()
            && let Some(summary) = retained(&path)
            && summary.identity.name == name
        {
            return Ok(Self::known(key, namespace, dir, summary));
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
            let _ = retain(&path, before, indexed.summary()?);
        }
        Ok(indexed)
    }

    /// Load the exact admitted owner on demand. A file changed since inventory admission refuses this compilation.
    pub fn demand(&self) -> Result<&LoafEntry, LoafIndexError> {
        self.entry
            .get_or_init(|| {
                let artifact = LoafArtifact::read(&self.dir, &self.name)?;
                if artifact.identity != self.summary()?.identity {
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

    /// The Loaf's shipped native ABI cache. A demanded manifest already holds it; otherwise it is read and
    /// authenticated by the digest the summary names, without the manifest. A summary without that digest demands
    /// the manifest, which names the cache.
    pub fn rust_abi_cache(&self) -> Result<Option<Arc<RustAbiCache>>, LoafIndexError> {
        if let Some(entry) = self.entry.get() {
            return entry
                .as_ref()
                .map(|entry| entry.artifact.rust_abi_cache.clone())
                .map_err(Clone::clone);
        }
        let Some(expected) = self.summary()?.rust_abi_cache_digest.as_deref() else {
            return Ok(self.demand()?.artifact.rust_abi_cache.clone());
        };
        self.rust_abi
            .get_or_init(|| {
                LoafArtifact::read_digested_rust_abi_cache(&self.dir, &self.name, expected).map(|(cache, _)| cache)
            })
            .clone()
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
                    && let Some((manifest, digest)) = super::compiler_record_set::read_execution(&self.dir, &self.name)?
                {
                    if digest != self.summary()?.identity.digest {
                        return Err(LoafIndexError::Unreadable {
                            path: self.dir.join(format!("{}.incnlib", artifact_stem(&self.name))),
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
                    && let Some((surface, digest)) = super::compiler_record_set::read_surface(&self.dir, &self.name)?
                {
                    if digest != self.summary()?.identity.digest {
                        return Err(LoafIndexError::Unreadable {
                            path: self.dir.join(format!("{}.incnlib", artifact_stem(&self.name))),
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
                        super::compiler_record_set::read_runtime_bindings(&self.dir, &self.name)?
                {
                    if digest != self.summary()?.identity.digest {
                        return Err(LoafIndexError::Unreadable {
                            path: self.dir.join(format!("{}.incnlib", artifact_stem(&self.name))),
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
            path: self.dir.join(format!("{}.incnlib", artifact_stem(&self.name))),
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

    /// The type scope of the module `module` of this Loaf, from its own record. `None` when the installed set holds no
    /// one such record, including a set converted before modules had type scope records and every Loaf outside records
    /// mode; such a caller reads the scope from the manifest's checked API.
    pub fn type_scope(&self, module: &[String]) -> Result<Option<ModuleTypeScope>, LoafIndexError> {
        if !super::compiler_record_set::configured() {
            return Ok(None);
        }
        let Some(values) = self.named_records(RecordFamily::TypeScope.as_str(), &module.join("::"))? else {
            return Ok(None);
        };
        let [scope] = values.as_slice() else {
            return Ok(None);
        };
        Ok(serde_json::from_value(scope.clone()).ok())
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
        let read = super::compiler_record_set::read_declaration(&self.dir, &self.name, family, name)?;
        let values = match read {
            Some((_, digest)) if digest != self.summary()?.identity.digest => {
                return Err(self.inventory_mismatch("records read by name differ from their admitted inventory"));
            }
            Some((values, _)) => Some(Arc::new(values)),
            None => None,
        };
        kept.insert((family, name.to_owned()), values.clone());
        Ok(values)
    }

    /// Validate every record selected under this path, including aliases, before exposing its canonical identity.
    pub fn runtime_binding(&self, path: &str) -> Result<Option<RuntimeCallableBinding>, LoafIndexError> {
        let mut selected = None;
        if let Some(values) = self.named_records("runtime-binding", path)? {
            for value in values.iter() {
                let binding = serde_json::from_value(value.clone())
                    .map_err(|_| self.inventory_mismatch("invalid runtime-binding value"))?;
                self.admit_runtime_binding(path, binding, &mut selected)?;
            }
        } else if self.names_record(&["runtime-binding"], None)? != Some(true) {
            // An aggregate-only package has no individual member to select. Every matching legacy row still
            // has to agree with the path, owner, and any other declaration of this public facade.
            for binding in self.runtime_bindings()?.iter().filter(|binding| binding.path == path) {
                self.admit_runtime_binding(path, binding.clone(), &mut selected)?;
            }
        }
        Ok(selected)
    }

    fn admit_runtime_binding(
        &self,
        path: &str,
        binding: RuntimeCallableBinding,
        selected: &mut Option<RuntimeCallableBinding>,
    ) -> Result<(), LoafIndexError> {
        if binding.path != path
            || binding.canonical.kind != incan_semantics_core::SemanticSourceTargetKind::Function
            || !matches!(&binding.canonical.origin, incan_semantics_core::SymbolOrigin::Package { library, .. } if library == &self.name)
            || selected.as_ref().is_some_and(|previous| previous != &binding)
        {
            return Err(self.inventory_mismatch("runtime binding path, identity, or owner differs from its selection"));
        }
        *selected = Some(binding);
        Ok(())
    }

    /// The refusal of a record that does not belong to the inventory the index admitted this Loaf by.
    fn inventory_mismatch(&self, reason: &str) -> LoafIndexError {
        LoafIndexError::Unreadable {
            path: self.dir.join(format!("{}.incnlib", artifact_stem(&self.name))),
            reason: reason.to_owned(),
        }
    }

    /// Whether the installed set holds a record of one of `families` for this Loaf, named `record` when one is given,
    /// without opening any value; `None` outside records mode.
    pub fn names_record(&self, families: &[&str], record: Option<&str>) -> Result<Option<bool>, LoafIndexError> {
        if !super::compiler_record_set::configured() {
            return Ok(None);
        }
        super::compiler_record_set::names_record(&self.dir, &self.name, families, record)
    }

    /// The governed bodies sidecar can be located from admitted identity without demanding declaration records.
    pub fn bodies_path(&self) -> PathBuf {
        self.dir.join(format!("{}.incnbody", artifact_stem(&self.name)))
    }

    /// Return only this parser vocabulary; a non-indexed producer retains normal manifest validation.
    pub fn parser_vocabulary(&self) -> Result<Option<Arc<VocabExports>>, LoafIndexError> {
        self.vocabulary
            .get_or_init(|| {
                if super::compiler_record_set::configured()
                    && let Some((vocabulary, digest)) =
                        super::compiler_record_set::read_vocabulary(&self.dir, &self.name)?
                {
                    if digest != self.summary()?.identity.digest {
                        return Err(self.inventory_mismatch("vocabulary differs from its admitted inventory"));
                    }
                    return Ok(Some(vocabulary));
                }
                Ok(self.demand()?.artifact.manifest.vocab.clone().map(Arc::new))
            })
            .clone()
    }

    /// Report a failed demand without opening an untouched manifest.
    pub fn failure(&self) -> Option<&LoafIndexError> {
        self.entry
            .get()
            .and_then(|result| result.as_ref().err())
            .or_else(|| self.selected_summary.get().and_then(|result| result.as_ref().err()))
            .or_else(|| self.vocabulary.get().and_then(|result| result.as_ref().err()))
    }

    /// Resolve namespace ownership from the index's declared binding, before reading manifest contents.
    pub fn owns(&self, module: &[String]) -> bool {
        super::owns(&self.namespace, module)
    }
}

/// Exact source and compiler generations bound to an optional private summary.
#[derive(Debug, PartialEq, Eq)]
struct Observation {
    path: PathBuf,
    source: Generation,
    compiler: (PathBuf, Generation),
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
    Some(manifest_input::cache_path(&key)?.with_file_name(format!("{key}.inventory.crumb")))
}

/// Refuse stale, malformed, oversized or corrupted records and fall back to full public admission.
fn retained(path: &Path) -> Option<Summary> {
    let observed = observation(path)?;
    let cache = record_path(path)?;
    let guard = manifest_input::checked_guard(&observed.path, observed.source, &observed.compiler);
    let (digest, payload) = manifest_input::read_cache(&cache, "inventory", guard, MAX_INVENTORY_RECORD_BYTES)?;
    let row = internal_models::decode_manifest_inventory(&payload).ok()?;
    if observation(path).as_ref() != Some(&observed) {
        return None;
    }
    Some(Summary {
        identity: LoafIdentity {
            name: row.name,
            version: row.version,
            digest,
            feature_projection: row.features.into_iter().collect(),
        },
        vocabulary: row.vocabulary,
        provider_operations: row.operations,
        native_unions: row.native_unions,
        rust_paths: row.rust_paths.into_iter().collect(),
        rust_abi_cache_declared: row.declares_abi_cache,
        rust_abi_cache: row.has_abi_cache,
        rust_abi_cache_digest: row.abi_digest,
    })
}

/// Atomically publish an optional inventory only after ordinary manifest admission has succeeded.
fn retain(path: &Path, observation: Observation, summary: &Summary) -> std::io::Result<()> {
    let Some(cache) = record_path(path) else { return Ok(()) };
    let guard = manifest_input::checked_guard(&observation.path, observation.source, &observation.compiler);
    let row = internal_models::ManifestInventory {
        name: summary.identity.name.clone(),
        version: summary.identity.version.clone(),
        features: summary.identity.feature_projection.iter().cloned().collect(),
        vocabulary: summary.vocabulary,
        operations: summary.provider_operations,
        native_unions: summary.native_unions,
        rust_paths: summary.rust_paths.iter().cloned().collect(),
        declares_abi_cache: summary.rust_abi_cache_declared,
        has_abi_cache: summary.rust_abi_cache,
        abi_digest: summary.rust_abi_cache_digest.clone(),
    };
    let payload = internal_models::encode_manifest_inventory(&row).map_err(std::io::Error::other)?;
    manifest_input::write_cache(&cache, "inventory", guard, &summary.identity.digest, payload)
}
