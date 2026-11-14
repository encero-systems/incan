//! A Loaf's declaration records, read by name from the installed set (RFC 179): a consumer that needs one
//! declaration's signature, layout or defaults opens that declaration's record of that family alone.

use std::path::Path;

use serde_json::Value;
use sha2::{Digest, Sha256};

use super::{
    DECLARATION_FAMILIES, LoafIndexError, RecordSet, installed, open_value, read_summary, record_failure, unreadable,
    validate_payload_directory,
};
use crate::library_manifest::LibraryManifest;

/// Read the `family` records of the declaration named `declaration` in one installed Loaf.
pub(in crate::loaf_index) fn read_declaration(
    dir: &Path,
    name: &str,
    family: &str,
    declaration: &str,
) -> Result<Option<(Vec<Value>, String)>, LoafIndexError> {
    let Some(set) = installed().map_err(|reason| unreadable(dir, name, reason))? else {
        return Ok(None);
    };
    read_declaration_in(set, dir, name, family, declaration)
}

/// Read the `family` records of the declaration named `declaration` in the Loaf group `name` of `set`, with the
/// manifest digest of the inventory the index admitted the group by. An overloaded name has one record per overload.
///
/// A set converted before declarations had records of their own has none; the reader then has the contract metadata.
pub(super) fn read_declaration_in(
    set: &RecordSet,
    dir: &Path,
    name: &str,
    family: &str,
    declaration: &str,
) -> Result<Option<(Vec<Value>, String)>, LoafIndexError> {
    if !DECLARATION_FAMILIES.contains(&family) {
        return Err(unreadable(
            dir,
            name,
            format!("`{family}` is not a declaration record family"),
        ));
    }
    let profile = dir
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| unreadable(dir, name, "Loaf profile directory is not UTF-8".to_owned()))?;
    let prefix = format!("loaf/{profile}/{name}/{family}/");
    let suffix = format!("/{:x}", Sha256::digest(declaration.as_bytes()));
    let entries = set
        .entries
        .iter()
        .filter(|entry| entry.key.starts_with(&prefix) && entry.key.ends_with(&suffix))
        .collect::<Vec<_>>();
    if entries.is_empty() {
        return Ok(None);
    }
    let Some(summary) = read_summary(set, dir, name)? else {
        return Ok(None);
    };
    validate_payload_directory(&set.root).map_err(|reason| unreadable(dir, name, reason))?;
    let mut values = Vec::new();
    let mut bytes = 0;
    for entry in entries {
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
        let value = open_value(&set.root, &entry.value)
            .map_err(|reason| unreadable(dir, name, record_failure(&entry.key, reason)))?;
        set.count_read(&entry.key, value.len());
        bytes += value.len();
        values.push(
            LibraryManifest::decode_compiler_record_value(&value).map_err(|error| {
                unreadable(dir, name, format!("invalid compiler-record family `{family}`: {error}"))
            })?,
        );
    }
    let process = set.reads();
    tracing::trace!(
        target: "incan::invocation",
        phase = "open",
        loaf = name,
        records = values.len(),
        bytes,
        process_records = process.records,
        process_bytes = process.bytes
    );
    Ok(Some((values, summary.identity.digest)))
}
