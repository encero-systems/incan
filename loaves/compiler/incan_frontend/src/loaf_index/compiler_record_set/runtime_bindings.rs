//! A Loaf's runtime callable bindings, read by name from the installed set (RFC 179): native lowering opens the one
//! runtime-bindings record, never the contract metadata the bindings are derived from.

use std::path::Path;
use std::sync::{Arc, PoisonError};

use incan_semantics_core::body_ir::RuntimeCallableBinding;

use super::{
    LoafIndexError, RUNTIME_BINDINGS, RecordSet, installed, open_value, read_summary, record_failure, unreadable,
    validate_payload_directory,
};
use crate::library_manifest::LibraryManifest;

/// Read the runtime callable bindings native lowering needs of one installed Loaf.
pub(in crate::loaf_index) fn read_runtime_bindings(
    dir: &Path,
    name: &str,
) -> Result<Option<(Arc<Vec<RuntimeCallableBinding>>, String)>, LoafIndexError> {
    let Some(set) = installed().map_err(|reason| unreadable(dir, name, reason))? else {
        return Ok(None);
    };
    read_runtime_bindings_in(set, dir, name)
}

/// Read one Loaf group's runtime callable bindings from `set` by name, with the manifest digest of the group they
/// belong to: its runtime-bindings record alone, beside the inventory summary the index already admitted it by.
///
/// A set converted before bindings were published has no runtime-bindings record; the caller then derives them from
/// the executable projection.
pub(super) fn read_runtime_bindings_in(
    set: &RecordSet,
    dir: &Path,
    name: &str,
) -> Result<Option<(Arc<Vec<RuntimeCallableBinding>>, String)>, LoafIndexError> {
    let profile = dir
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| unreadable(dir, name, "Loaf profile directory is not UTF-8".to_owned()))?;
    let prefix = format!("loaf/{profile}/{name}/{RUNTIME_BINDINGS}/");
    if let Some(found) = set.bindings.lock().unwrap_or_else(PoisonError::into_inner).get(&prefix) {
        return Ok(Some(found.clone()));
    }
    let entries = set
        .entries
        .iter()
        .filter(|entry| entry.key.starts_with(&prefix))
        .collect::<Vec<_>>();
    let entry = match entries.as_slice() {
        [] => return Ok(None),
        [entry] if entry.revision == entry.value => *entry,
        [entry] => {
            return Err(unreadable(
                dir,
                name,
                format!(
                    "compiler-record `{}` has distinct revision and value identities",
                    entry.key
                ),
            ));
        }
        _ => {
            return Err(unreadable(
                dir,
                name,
                "compiler-record runtime bindings require exactly one value".to_owned(),
            ));
        }
    };
    let Some(summary) = read_summary(set, dir, name)? else {
        return Ok(None);
    };
    validate_payload_directory(&set.root).map_err(|reason| unreadable(dir, name, reason))?;
    let bytes = open_value(&set.root, &entry.value)
        .map_err(|reason| unreadable(dir, name, record_failure(&entry.key, reason)))?;
    set.count_read(bytes.len());
    let process = set.reads();
    tracing::trace!(
        target: "incan::invocation",
        phase = "open",
        loaf = name,
        records = 1usize,
        bytes = bytes.len(),
        process_records = process.records,
        process_bytes = process.bytes
    );
    let bindings: Vec<RuntimeCallableBinding> =
        LibraryManifest::decode_compiler_record_value(&bytes).map_err(|error| {
            unreadable(
                dir,
                name,
                format!("invalid compiler-record family `{RUNTIME_BINDINGS}`: {error}"),
            )
        })?;
    let found = (Arc::new(bindings), summary.identity.digest);
    set.bindings
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(prefix, found.clone());
    Ok(Some(found))
}
