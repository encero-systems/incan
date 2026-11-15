//! A Loaf's metadata record and the inventory summary read from it (#2946): the small authenticated record an index
//! admits an installed Loaf by, read once per process before any of the Loaf's other records.

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::PoisonError;

use serde::Deserialize;

use super::{RecordSet, installed, read_value, record_failure, unreadable, valid_digest};
use crate::library_manifest::LibraryManifest;
use crate::loaf_index::lazy_entry::Summary;
use crate::loaf_index::{LoafIdentity, LoafIndexError};

/// Metadata retained separately from declarations so the original Loaf identity survives conversion.
#[derive(Deserialize)]
#[cfg_attr(test, derive(serde::Serialize))]
pub(super) struct Metadata {
    pub(super) name: String,
    pub(super) version: String,
    pub(super) incan_version: String,
    pub(super) manifest_format: u32,
    pub(super) manifest_digest: String,
    pub(super) vocabulary: bool,
    pub(super) provider_operations: bool,
    pub(super) native_unions: bool,
    pub(super) rust_paths: BTreeSet<String>,
    pub(super) rust_abi_cache_declared: bool,
}

/// Read only the small, authenticated inventory record used to index an installed Loaf lazily.
pub(in crate::loaf_index) fn summary(dir: &Path, name: &str) -> Result<Option<Summary>, LoafIndexError> {
    let Some(set) = installed().map_err(|reason| unreadable(dir, name, reason))? else {
        return Ok(None);
    };
    read_summary(set, dir, name)
}

/// Read the inventory record of one Loaf group of `set`, once per process.
pub(super) fn read_summary(set: &RecordSet, dir: &Path, name: &str) -> Result<Option<Summary>, LoafIndexError> {
    let profile = dir
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| unreadable(dir, name, "Loaf profile directory is not UTF-8".to_owned()))?;
    let prefix = format!("loaf/{profile}/{name}/metadata/");
    if let Some(summary) = set
        .summaries
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(&prefix)
    {
        return Ok(Some(summary.clone()));
    }
    let entries = set
        .entries
        .iter()
        .filter(|entry| entry.key.starts_with(&prefix))
        .collect::<Vec<_>>();
    if entries.is_empty() {
        return Ok(None);
    }
    if entries.len() != 1 {
        return Err(unreadable(
            dir,
            name,
            "compiler-record metadata requires exactly one value".to_owned(),
        ));
    }
    let entry = entries[0];
    if entry.revision != entry.value {
        return Err(unreadable(
            dir,
            name,
            format!(
                "compiler-record `{}` has distinct revision and value identities",
                entry.key
            ),
        ));
    }
    let value = read_value(&set.root, &entry.value)
        .map_err(|reason| unreadable(dir, name, record_failure(&entry.key, reason)))?;
    set.count_read(&entry.key, value.len());
    let metadata: Metadata = LibraryManifest::decode_compiler_record_value(&value)
        .map_err(|error| unreadable(dir, name, format!("invalid compiler-record metadata: {error}")))?;
    if metadata.name != name {
        return Err(unreadable(
            dir,
            name,
            format!("compiler records name the Loaf `{}`", metadata.name),
        ));
    }
    valid_digest(&metadata.manifest_digest).map_err(|reason| unreadable(dir, name, reason))?;
    let summary = Summary {
        identity: LoafIdentity {
            name: metadata.name,
            version: metadata.version,
            digest: metadata.manifest_digest,
            feature_projection: BTreeSet::new(),
        },
        vocabulary: metadata.vocabulary,
        provider_operations: metadata.provider_operations,
        native_unions: metadata.native_unions,
        rust_paths: metadata.rust_paths,
        rust_abi_cache_declared: metadata.rust_abi_cache_declared,
        rust_abi_cache: metadata.rust_abi_cache_declared,
    };
    set.summaries
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(prefix, summary.clone());
    Ok(Some(summary))
}
