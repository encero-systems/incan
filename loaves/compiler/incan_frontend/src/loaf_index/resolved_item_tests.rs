//! Resolved Rust item tests: a Loaf index reads its retained Rust ABI items and names them by their resolved paths.

use super::lazy_entry::IndexedLoaf;
use super::manifest_input::read_bytes;
use super::{LoafIdentity, LoafIndex, LoafNamespace};
use crate::rust_abi_cache::encode_rust_abi_cache;
use incan_lang::interop::{RustItemKind, RustItemMetadata, RustTypeInfo, RustVisibility};
use incan_loaf_names::artifact_stem;
use std::collections::BTreeSet;
use std::sync::Arc;

/// A Rust path no Loaf's summary names is looked up in each Loaf's shipped ABI cache, read and authenticated by
/// the digest the Loaf's summary names, so the lookup demands no Loaf's manifest.
#[test]
fn a_shipped_abi_cache_answers_without_its_loafs_manifest() -> Result<(), Box<dyn std::error::Error>> {
    let path = "native::Header";
    let directory = tempfile::tempdir()?;
    let cache = directory.path().join(format!("{}.rustabi", artifact_stem("catalog")));
    let item = RustItemMetadata {
        canonical_path: path.to_owned(),
        definition_path: None,
        visibility: RustVisibility::Public,
        kind: RustItemKind::Type(RustTypeInfo::default()),
    };
    std::fs::write(
        &cache,
        encode_rust_abi_cache(&[(path.to_owned(), item)].into_iter().collect())?,
    )?;
    let (_, digest) = read_bytes(&cache)?;
    let identity = LoafIdentity {
        name: "catalog".to_owned(),
        version: "1.0.0".to_owned(),
        digest: "admitted-digest".to_owned(),
        feature_projection: BTreeSet::new(),
    };
    let namespace = LoafNamespace::Dependency {
        key: "catalog".to_owned(),
    };
    let mut indexed = IndexedLoaf::summary_only("catalog".to_owned(), namespace, identity);
    // The directory holds the cache alone: a demanded manifest cannot be read, so it cannot answer either.
    indexed.dir = directory.path().to_path_buf();
    let mut summary = serde_json::to_value(indexed.summary()?)?;
    summary["rust_abi_cache_declared"] = true.into();
    summary["rust_abi_cache"] = true.into();
    summary["rust_abi_cache_digest"] = digest.into();
    let updated = serde_json::from_value(summary)?;
    indexed = IndexedLoaf::known(indexed.key, indexed.namespace, indexed.dir, updated);
    let observation = indexed.clone();
    let mut index = LoafIndex::default();
    index.entries.insert("catalog".to_owned(), indexed);

    let found = index
        .rust_abi_item(path)
        .ok_or("the shipped ABI cache did not answer")?;

    assert_eq!(found.canonical_path, path);
    assert!(!observation.was_demanded(), "the lookup demanded the Loaf's manifest");
    Ok(())
}

/// Releasing decoded items after checking drops every shared item the index held, and a later lookup answers
/// again through the same lookup order instead of from a retained copy.
#[test]
fn released_items_leave_the_index_and_decode_again() -> Result<(), Box<dyn std::error::Error>> {
    let path = "native::Header";
    let mut index = LoafIndex::default();
    index.add_rust_facts(
        &Default::default(),
        [(
            path.to_owned(),
            RustItemMetadata {
                canonical_path: path.to_owned(),
                definition_path: None,
                visibility: RustVisibility::Public,
                kind: RustItemKind::Type(RustTypeInfo::default()),
            },
        )]
        .into_iter()
        .collect(),
    );
    let first = index.rust_abi_item(path).ok_or("the installed fact did not answer")?;
    assert_eq!(Arc::strong_count(&first), 2, "the index shares the item it decoded");

    index.release_resolved_rust_items();

    assert_eq!(
        Arc::strong_count(&first),
        1,
        "the released index still holds the decoded item"
    );
    let again = index.rust_abi_item(path).ok_or("a released item no longer answers")?;
    assert!(!Arc::ptr_eq(&first, &again), "the lookup answered from a retained copy");
    assert_eq!(again.canonical_path, path);
    Ok(())
}
