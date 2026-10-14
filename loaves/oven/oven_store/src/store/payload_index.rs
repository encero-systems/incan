//! Request-key projections over the Store's existing immutable manifest catalog.

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use super::{
    OvenArtifactManifest, OvenStoreError, RetainedManifestCatalog, canonical_published_entry_root,
    prove_entry_manifest, read_manifest_memo, read_published_entry_manifest, retained_store_manifests,
    store_directory_stamp, validate_execution_identities, verified_payload_bytes,
};

/// Deterministic consumer-owned projection of verified immutable payloads into request keys.
///
/// The namespace versions the projection contract. Partition matching must depend only on its string argument and
/// the manifest; key extraction must depend only on the verified manifest and payload. Keys narrow discovery only:
/// every returned identity still goes through the ordinary exact admission and lease checks.
pub struct OvenStorePayloadIndex {
    /// Unique versioned name of this payload projection.
    pub namespace: &'static str,
    /// Select the manifest partition whose payload keys are needed by this request.
    pub matches_partition: fn(&str, &OvenArtifactManifest) -> bool,
    /// Derive a request key, or exclude a payload that this projection does not understand.
    pub key: fn(&OvenArtifactManifest, &[u8]) -> Option<String>,
}

/// One exact-owner admission request, expressed directly or through retained immutable payload keys.
pub enum OvenStoreExecutionRequest<'a> {
    /// A unique, non-empty batch of canonical artifact identities.
    Identities(&'a [String]),
    /// Resolve matching keys in one retained Store partition before applying the same exact-owner admission.
    Indexed {
        /// Versioned deterministic payload projection.
        index: &'static OvenStorePayloadIndex,
        /// Manifest-only partition for this request, such as target and profile.
        partition: &'a str,
        /// Alternative accepted request keys; overlapping results are admitted only once.
        keys: &'a [String],
    },
}

impl<'a> OvenStoreExecutionRequest<'a> {
    /// Reject invalid explicit identities before any filesystem access; indexed misses are allowed to be empty.
    pub(super) fn validate_identities(&self) -> Result<(), OvenStoreError> {
        match self {
            Self::Identities(identities) => validate_execution_identities(identities),
            Self::Indexed { .. } => Ok(()),
        }
    }
}

impl<'a> From<&'a [String]> for OvenStoreExecutionRequest<'a> {
    /// Preserve the existing borrowed identity-slice admission surface.
    fn from(identities: &'a [String]) -> Self {
        Self::Identities(identities)
    }
}

impl<'a> From<&'a Vec<String>> for OvenStoreExecutionRequest<'a> {
    /// Borrow existing identity batches without copying their records.
    fn from(identities: &'a Vec<String>) -> Self {
        Self::Identities(identities)
    }
}

impl<'a, const N: usize> From<&'a [String; N]> for OvenStoreExecutionRequest<'a> {
    /// Preserve fixed-size identity batches used by ordinary single-owner admission.
    fn from(identities: &'a [String; N]) -> Self {
        Self::Identities(identities)
    }
}

/// Complete key membership for one observed catalog generation and manifest partition; no leases are retained.
pub(super) struct RetainedPayloadIndex {
    descriptor: &'static OvenStorePayloadIndex,
    by_key: BTreeMap<String, BTreeSet<String>>,
}

/// Resolve request keys while the caller holds the Store manager lock, then leave admission to its exact selector.
pub(super) fn resolve_execution_request<'a>(
    root: &Path,
    request: OvenStoreExecutionRequest<'a>,
) -> Result<Cow<'a, [String]>, OvenStoreError> {
    let (index, partition, keys) = match request {
        OvenStoreExecutionRequest::Identities(identities) => return Ok(Cow::Borrowed(identities)),
        OvenStoreExecutionRequest::Indexed { index, partition, keys } => (index, partition, keys),
    };
    let _invocation = tracing::trace_span!(target: "incan::invocation", "oven_store::lookup_payload_index", namespace = index.namespace).entered();
    let entries = retained_store_manifests(root)?;
    let stamp = store_directory_stamp(root);
    let coordinate = (index.namespace, partition.to_owned());
    if let Ok(memo) = read_manifest_memo().lock()
        && let Some(catalog) = memo.catalogs.get(root)
        && Some(&catalog.stamp) == stamp.as_ref()
        && let Some(retained) = catalog.payload_indexes.get(&coordinate)
        && std::ptr::eq(retained.descriptor, index)
    {
        tracing::trace!(target: "incan::invocation", decision = "reuse_payload_index", namespace = index.namespace);
        return Ok(Cow::Owned(matching_identities(retained, keys)));
    }

    let _build = tracing::trace_span!(target: "incan::invocation", "oven_store::build_payload_index", namespace = index.namespace).entered();
    let mut retained = RetainedPayloadIndex {
        descriptor: index,
        by_key: BTreeMap::new(),
    };
    for (path, manifest) in entries.iter() {
        if !(index.matches_partition)(partition, manifest) {
            continue;
        }
        let path = canonical_published_entry_root(path)?;
        let read = read_published_entry_manifest(&path)?;
        if !(index.matches_partition)(partition, &read.manifest) {
            continue;
        }
        prove_entry_manifest(&read)?;
        let bytes = verified_payload_bytes(&path, &read.manifest)?;
        if let Some(key) = (index.key)(&read.manifest, &bytes) {
            retained.by_key.entry(key).or_default().insert(read.manifest.identity);
        }
    }
    let identities = matching_identities(&retained, keys);
    if let Ok(mut memo) = read_manifest_memo().lock()
        && let Some(catalog) = memo.catalogs.get_mut(root)
        && Some(&catalog.stamp) == stamp.as_ref()
        && store_directory_stamp(root) == stamp
    {
        catalog.payload_indexes.insert(coordinate, retained);
    }
    Ok(Cow::Owned(identities))
}

/// Collect exact owners for alternative request keys, deterministically deduplicating overlapping matches.
fn matching_identities(index: &RetainedPayloadIndex, keys: &[String]) -> Vec<String> {
    keys.iter()
        .filter_map(|key| index.by_key.get(key))
        .flat_map(|identities| identities.iter())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .cloned()
        .collect()
}

/// Advance complete payload projections with this publisher's already-verified committed bytes.
pub(super) fn advance_payload_indexes(
    catalog: &mut RetainedManifestCatalog,
    manifest: &OvenArtifactManifest,
    payload: &[u8],
) {
    for ((_, partition), retained) in &mut catalog.payload_indexes {
        if (retained.descriptor.matches_partition)(partition, manifest)
            && let Some(key) = (retained.descriptor.key)(manifest, payload)
        {
            retained
                .by_key
                .entry(key)
                .or_default()
                .insert(manifest.identity.clone());
            tracing::trace!(target: "incan::invocation", decision = "advance_payload_index", namespace = retained.descriptor.namespace);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    #[cfg(unix)]
    use std::path::Path;

    use super::{OvenStoreExecutionRequest, OvenStorePayloadIndex};
    #[cfg(unix)]
    use crate::store::OvenStoreError;
    use crate::store::{OvenStore, OvenStoreLimits, PublishedOvenStore};
    use crate::test_support::{request, write_project};

    /// Use domain partitions and literal payload text so the tests exercise Store mechanics independently of compilers.
    static INDEX: OvenStorePayloadIndex = OvenStorePayloadIndex {
        namespace: "oven.store-test-payload/1",
        matches_partition: |partition, manifest| manifest.domain == partition,
        key: |_, bytes| std::str::from_utf8(bytes).ok().map(str::to_owned),
    };

    /// Build the same indexed request used by writable and published exact admission.
    fn indexed<'a>(keys: &'a [String]) -> OvenStoreExecutionRequest<'a> {
        OvenStoreExecutionRequest::Indexed {
            index: &INDEX,
            partition: "indexed",
            keys,
        }
    }

    /// Warm misses do not inspect historical payloads, own publication advances the index and old keys remain reusable.
    #[cfg(unix)]
    #[test]
    fn indexed_selection_reuses_and_advances_history() -> Result<(), Box<dyn std::error::Error>> {
        use std::sync::atomic::{AtomicUsize, Ordering};

        static CALLS: AtomicUsize = AtomicUsize::new(0);
        static COUNTED: OvenStorePayloadIndex = OvenStorePayloadIndex {
            namespace: "oven.store-test-counted-payload/1",
            matches_partition: |partition, manifest| manifest.domain == partition,
            key: |_, bytes| {
                CALLS.fetch_add(1, Ordering::SeqCst);
                std::str::from_utf8(bytes).ok().map(str::to_owned)
            },
        };
        let root = tempfile::tempdir()?;
        let project = tempfile::tempdir()?;
        write_project(project.path())?;
        let store = OvenStore::new(root.path(), OvenStoreLimits::new(1_000_000, 1_000_000, 1_000_000));
        let first = store.publish(&request(project.path(), "indexed", b"first")?)?;
        let keys = ["missing".to_owned()];
        let missing = || OvenStoreExecutionRequest::Indexed {
            index: &COUNTED,
            partition: "indexed",
            keys: &keys,
        };
        assert!(store.select_payloads_for_execution(missing())?.is_empty());
        assert_eq!(CALLS.load(Ordering::SeqCst), 1);
        assert!(store.select_payloads_for_execution(missing())?.is_empty());
        assert_eq!(CALLS.load(Ordering::SeqCst), 1, "warm miss reparsed history");
        let second = store.publish(&request(project.path(), "indexed", b"second")?)?;
        assert_eq!(
            CALLS.load(Ordering::SeqCst),
            2,
            "publisher did not advance the existing index"
        );
        let keys = ["first".to_owned(), "second".to_owned(), "first".to_owned()];
        let selected = store.select_payloads_for_execution(OvenStoreExecutionRequest::Indexed {
            index: &COUNTED,
            partition: "indexed",
            keys: &keys,
        })?;
        assert_eq!(selected.len(), 2);
        assert_eq!(CALLS.load(Ordering::SeqCst), 2, "exact admission rebuilt payload keys");
        assert!(selected.iter().any(|owner| owner.manifest.identity == first.identity));
        assert!(selected.iter().any(|owner| owner.manifest.identity == second.identity));
        Ok(())
    }

    /// A retained key never substitutes for fresh payload integrity checks, including preserved modification time.
    #[cfg(unix)]
    #[test]
    fn indexed_selection_refuses_selected_payload_tampering() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let project = tempfile::tempdir()?;
        write_project(project.path())?;
        let store = OvenStore::new(root.path(), OvenStoreLimits::new(1_000_000, 1_000_000, 1_000_000));
        let manifest = store.publish(&request(project.path(), "indexed", b"first")?)?;
        let keys = ["first".to_owned()];
        assert_eq!(store.select_payloads_for_execution(indexed(&keys))?.len(), 1);
        let payload = store.entry_root(&manifest.identity).join(super::super::PAYLOAD_FILE);
        let modified = fs::metadata(&payload)?.modified()?;
        fs::write(&payload, b"other")?;
        fs::File::options()
            .write(true)
            .open(&payload)?
            .set_times(fs::FileTimes::new().set_modified(modified))?;
        assert!(matches!(
            store.select_payloads_for_execution(indexed(&keys)),
            Err(OvenStoreError::Integrity { .. })
        ));
        Ok(())
    }

    /// Manifest partitions exclude unrelated damaged payloads without relaxing verification inside the selected
    /// partition.
    #[test]
    fn indexed_selection_respects_manifest_partitions() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let project = tempfile::tempdir()?;
        write_project(project.path())?;
        let store = OvenStore::new(root.path(), OvenStoreLimits::new(1_000_000, 1_000_000, 1_000_000));
        let outside = store.publish(&request(project.path(), "other", b"outside")?)?;
        fs::write(
            store.entry_root(&outside.identity).join(super::super::PAYLOAD_FILE),
            b"damaged",
        )?;
        let selected = store.publish(&request(project.path(), "indexed", b"first")?)?;
        let keys = ["first".to_owned()];
        let owners = store.select_payloads_for_execution(indexed(&keys))?;
        assert_eq!(owners.len(), 1);
        assert_eq!(owners[0].manifest.identity, selected.identity);
        Ok(())
    }

    /// An independent publisher invalidates key membership; removing a former match does not leave a stale owner.
    #[cfg(unix)]
    #[test]
    fn indexed_selection_reconciles_external_publication_and_removal() -> Result<(), Box<dyn std::error::Error>> {
        const ROOT: &str = "INCAN_PAYLOAD_INDEX_TEST_ROOT";
        const PROJECT: &str = "INCAN_PAYLOAD_INDEX_TEST_PROJECT";
        if let Some(root) = std::env::var_os(ROOT) {
            let project = std::env::var_os(PROJECT).ok_or("child project missing")?;
            OvenStore::new(Path::new(&root), OvenStoreLimits::new(1_000_000, 1_000_000, 1_000_000))
                .publish(&request(Path::new(&project), "indexed", b"second")?)?;
            return Ok(());
        }
        let root = tempfile::tempdir()?;
        let project = tempfile::tempdir()?;
        write_project(project.path())?;
        let store = OvenStore::new(root.path(), OvenStoreLimits::new(1_000_000, 1_000_000, 1_000_000));
        let first = store.publish(&request(project.path(), "indexed", b"first")?)?;
        let keys = ["first".to_owned(), "second".to_owned()];
        assert_eq!(store.select_payloads_for_execution(indexed(&keys))?.len(), 1);
        let child = std::process::Command::new(std::env::current_exe()?)
            .args([
                "--exact",
                "store::payload_index::tests::indexed_selection_reconciles_external_publication_and_removal",
            ])
            .env(ROOT, root.path())
            .env(PROJECT, project.path())
            .output()?;
        assert!(child.status.success(), "{}", String::from_utf8_lossy(&child.stderr));
        assert_eq!(store.select_payloads_for_execution(indexed(&keys))?.len(), 2);
        fs::remove_dir_all(store.entry_root(&first.identity))?;
        let owners = store.select_payloads_for_execution(indexed(&keys))?;
        assert_eq!(owners.len(), 1);
        assert_eq!(owners[0].payload, b"second");
        Ok(())
    }

    /// Published consumers share key projection and retain the same lease protection without changing Store files.
    #[test]
    fn published_indexed_selection_preserves_active_leases() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let project = tempfile::tempdir()?;
        write_project(project.path())?;
        let store = OvenStore::new(root.path(), OvenStoreLimits::new(1_000_000, 1_000_000, 1_000_000));
        let manifest = store.publish(&request(project.path(), "indexed", b"first")?)?;
        let keys = ["first".to_owned()];
        let owners = PublishedOvenStore::new(root.path()).select_payloads_for_execution(indexed(&keys))?;
        assert_eq!(owners.len(), 1);
        assert!(super::super::try_lock_entry(&store.entry_root(&manifest.identity))?.is_none());
        drop(owners);
        assert!(super::super::try_lock_entry(&store.entry_root(&manifest.identity))?.is_some());
        Ok(())
    }
}
