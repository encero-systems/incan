//! Demand loading behind an admitted, generation-bound inventory. Summaries contain only facts derived from the
//! ordinary manifest reader; an unavailable summary takes that reader again rather than guessing at absent facts.

use std::collections::BTreeSet;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::decoded_manifest::compiler_generation;
use super::manifest_input::{self, Generation, generation};
use super::{LoafArtifact, LoafEntry, LoafIdentity, LoafIndexError, LoafNamespace};

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
                .map(|abi| abi.items.iter().map(|item| item.canonical_path.clone()).collect())
                .unwrap_or_default(),
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
}

impl IndexedLoaf {
    /// Wrap a caller's already admitted entry without reopening its files.
    pub fn admitted(entry: LoafEntry) -> Self {
        let indexed = Self {
            key: entry.key.clone(),
            namespace: entry.namespace.clone(),
            dir: entry.artifact.dir.clone(),
            summary: Summary::from_artifact(&entry.artifact),
            entry: Arc::default(),
        };
        let _ = indexed.entry.set(Ok(entry));
        indexed
    }

    /// Admit a small inventory, or use full admission when its private proof is missing or stale.
    pub fn open(key: String, namespace: LoafNamespace, dir: PathBuf, name: &str) -> Result<Self, LoafIndexError> {
        let path = dir.join(format!("{name}.incnlib"));
        if let Some(summary) = retained(&path)
            && summary.identity.name == name
        {
            return Ok(Self {
                key,
                namespace,
                dir,
                summary,
                entry: Arc::default(),
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
