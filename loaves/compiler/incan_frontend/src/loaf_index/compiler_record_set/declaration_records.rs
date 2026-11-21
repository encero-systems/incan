//! A Loaf's records read by name from the installed set (RFC 179): a consumer that needs one declaration's signature,
//! layout or defaults, or one public namespace, opens that record of that family alone.

use std::path::Path;
use std::sync::PoisonError;

use serde_json::Value;
use sha2::{Digest, Sha256};

use super::{
    LoafIndexError, RecordSet, installed, open_value, read_summary, record_failure, unreadable,
    validate_payload_directory,
};
use crate::library_manifest::LibraryManifest;

/// The record families a consumer reads by name: a declaration's own facts, a public namespace, the names an enum
/// publishes at the package root and a module's type scope. A manifest projection never selects them.
pub(super) const NAMED_FAMILIES: [&str; 7] = [
    "declaration-signature",
    "layout",
    "default",
    "public-namespace",
    "enum-variant",
    "type-scope",
    "runtime-binding",
];

/// Read the `family` records named `declaration` (a declaration, or a public namespace by its path) in one installed
/// Loaf.
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
/// manifest digest of the inventory the index admitted the group by. An overloaded name has one record per overload,
/// read in declaration order.
///
/// A set converted before declarations had records of their own has none; the reader then has the contract metadata.
pub(super) fn read_declaration_in(
    set: &RecordSet,
    dir: &Path,
    name: &str,
    family: &str,
    declaration: &str,
) -> Result<Option<(Vec<Value>, String)>, LoafIndexError> {
    if !NAMED_FAMILIES.contains(&family) {
        return Err(unreadable(
            dir,
            name,
            format!("`{family}` is not a record family read by name"),
        ));
    }
    let profile = dir
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| unreadable(dir, name, "Loaf profile directory is not UTF-8".to_owned()))?;
    let prefix = format!("loaf/{profile}/{name}/{family}/");
    let hash = format!("{:x}", Sha256::digest(declaration.as_bytes()));
    let mut entries = set
        .members(&prefix, Some(&hash), 0)
        .map_err(|reason| unreadable(dir, name, reason))?;
    // A record key carries its ordinal in its family, so key order is declaration order: an overloaded name's
    // records are read in the order their overloads are declared.
    entries.sort_by(|left, right| left.key.cmp(&right.key));
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
    set.present_named_families
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(prefix);
    Ok(Some((values, summary.identity.digest)))
}

/// Whether the installed Loaf `name` holds a record of one of `families`, named `record` when one is given, answered
/// from the set's receipt alone without opening any value. `None` outside records mode, and for a Loaf the set does not
/// hold.
pub(in crate::loaf_index) fn names_record(
    dir: &Path,
    name: &str,
    families: &[&str],
    record: Option<&str>,
) -> Result<Option<bool>, LoafIndexError> {
    let Some(set) = installed().map_err(|reason| unreadable(dir, name, reason))? else {
        return Ok(None);
    };
    let profile = dir
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| unreadable(dir, name, "Loaf profile directory is not UTF-8".to_owned()))?;
    let group = format!("loaf/{profile}/{name}/");
    if record.is_none() {
        let present = set
            .present_named_families
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if families
            .iter()
            .any(|family| present.contains(&format!("{group}{family}/")))
        {
            return Ok(Some(true));
        }
    }
    if set
        .members(&format!("{group}metadata/"), None, 1)
        .map_err(|reason| unreadable(dir, name, reason))?
        .is_empty()
    {
        return Ok(None);
    }
    let hash = record.map(|member| format!("{:x}", Sha256::digest(member.as_bytes())));
    for family in families {
        if !set
            .members(&format!("{group}{family}/"), hash.as_deref(), 1)
            .map_err(|reason| unreadable(dir, name, reason))?
            .is_empty()
        {
            return Ok(Some(true));
        }
    }
    Ok(Some(false))
}
