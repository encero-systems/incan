//! A Loaf's metadata record and the inventory summary read from it (#2946): the small authenticated record an index
//! admits an installed Loaf by, read once per process before any of the Loaf's other records.

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::PoisonError;

use serde::Deserialize;

use super::{RecordSet, installed, read_value, record_failure, unreadable, valid_digest};
use crate::library_manifest::{
    LibraryManifest,
    capabilities::{self, Capability, CapabilityCatalog},
};
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
    /// The digest of the native ABI cache the manifest names; a set converted before it was recorded has none.
    #[serde(default)]
    pub(super) rust_abi_cache_digest: Option<String>,
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
        .members(&prefix, None, 2)
        .map_err(|reason| unreadable(dir, name, reason))?;
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
    let entry = &entries[0];
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
    let metadata: Capability = LibraryManifest::decode_compiler_record_value(&value)
        .map_err(|error| unreadable(dir, name, format!("invalid compiler-record metadata: {error}")))?;
    if metadata.name != name {
        return Err(unreadable(
            dir,
            name,
            format!("compiler records name the Loaf `{}`", metadata.name),
        ));
    }
    valid_digest(&metadata.manifest_digest).map_err(|reason| unreadable(dir, name, reason))?;
    let summary = from_capability(metadata);
    set.summaries
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(prefix, summary.clone());
    Ok(Some(summary))
}

/// Admit the profile's global contributors once. Installations without a catalog retain individual metadata reads.
pub(in crate::loaf_index) fn admit_capabilities(dir: &Path, names: &BTreeSet<String>) -> Result<(), LoafIndexError> {
    let profile = dir.file_name().and_then(|n| n.to_str()).unwrap_or("");
    let Some(set) = installed().map_err(|e| unreadable(dir, profile, e))? else {
        return Ok(());
    };
    load_capabilities(set, profile, names).map_err(|reason| unreadable(dir, profile, reason))
}

fn load_capabilities(set: &RecordSet, profile: &str, names: &BTreeSet<String>) -> Result<(), String> {
    let key = capabilities::key(profile, names);
    let hash = key.rsplit('/').next().ok_or("invalid compiler catalog key")?;
    let mut checked = set.capability_profiles.lock().unwrap_or_else(PoisonError::into_inner);
    if checked.contains(&key) {
        return Ok(());
    }
    let entries = set.members(&capabilities::prefix(profile), Some(hash), 2)?;
    if let Some(entry) = entries.first() {
        if entries.len() != 1 || entry.key != key || entry.value != entry.revision {
            return Err("ambiguous or mismatched installed capability record".into());
        }
        let bytes = read_value(&set.root, &entry.value).map_err(|reason| record_failure(&entry.key, reason))?;
        set.count_read(&entry.key, bytes.len());
        let members = CapabilityCatalog::decode(&bytes, profile)?;
        if members.len() != names.len()
            || members
                .iter()
                .zip(names)
                .any(|(member, expected)| &member.name != expected)
        {
            return Err("installed compiler capability members differ from the requested package roster".into());
        }
        let mut admitted = Vec::with_capacity(members.len());
        for member in members {
            valid_digest(&member.manifest_digest)?;
            if let Some(digest) = &member.rust_abi_cache_digest {
                valid_digest(digest)?;
            }
            let key = format!("loaf/{profile}/{}/metadata/", member.name);
            admitted.push((key, from_capability(member)));
        }
        let mut summaries = set.summaries.lock().unwrap_or_else(PoisonError::into_inner);
        summaries.extend(admitted);
    }
    checked.insert(key);
    Ok(())
}

fn from_capability(member: Capability) -> Summary {
    Summary {
        identity: LoafIdentity {
            name: member.name,
            version: member.version,
            digest: member.manifest_digest,
            feature_projection: BTreeSet::new(),
        },
        vocabulary: member.vocabulary,
        provider_operations: member.provider_operations,
        native_unions: member.native_unions,
        rust_paths: member.rust_paths,
        rust_abi_cache_declared: member.rust_abi_cache_declared,
        rust_abi_cache: member.rust_abi_cache_declared,
        rust_abi_cache_digest: member.rust_abi_cache_digest,
    }
}
