//! Receipt-owned native macros for the direct inspector; serialized paths alone never authorize execution.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

use oven_store::store::{OvenStore, OvenStoreExecutionPayload, OvenStoreLimits};
use serde::{Deserialize, Serialize};

use crate::error::RustMetadataError;

/// Paired native macro coordinates published beside a frozen source graph.
pub const OVEN_DIRECT_PROC_MACRO_AUTHORITY_FILE: &str = ".incan_oven_proc_macros.json";

/// Exact native output coordinates, checked against its immutable owner before a macro server can execute it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OvenInspectionProcMacro {
    /// Canonical crate source module selected by the frozen inspection graph.
    pub root_module: PathBuf,
    /// Store whose immutable entry owns the source and native macro output together.
    pub store: PathBuf,
    /// Immutable entry identity; never inferred from an artifact filename.
    pub identity: String,
    /// Compilation receipt recorded in that immutable entry.
    pub receipt_identity: String,
    /// Regular macro library path relative to the selected owner's artifact root.
    pub relative_path: String,
    /// Native bytes recorded by the selected owner's admitted manifest.
    pub digest: String,
}

/// Persist the publisher's exact macro coordinates; the loader reacquires and verifies every owner itself.
pub fn write_oven_inspection_proc_macro_authority(
    manifest_dir: &Path,
    mut macros: Vec<OvenInspectionProcMacro>,
) -> Result<PathBuf, RustMetadataError> {
    macros.sort_by(|left, right| left.root_module.cmp(&right.root_module));
    let path = manifest_dir.join(OVEN_DIRECT_PROC_MACRO_AUTHORITY_FILE);
    let payload = serde_json::to_vec_pretty(&serde_json::json!({"schema_version": 1, "macros": macros}))
        .map_err(|error| invalid(&path, &format!("cannot encode native macro authority: {error}")))?;
    std::fs::write(&path, payload)?;
    Ok(path)
}

/// Inject only admitted native macro outputs, returning the leases that must outlive the macro client/database.
pub(super) fn select_native_macros(
    manifest_dir: &Path,
    graph: &mut serde_json::Value,
) -> Result<Vec<OvenStoreExecutionPayload>, RustMetadataError> {
    select_native_macros_with_probe(manifest_dir, graph, active_compiler_identity)
}

/// Exercise admission against a different compiler without mutating the process-wide Rustc selection.
#[cfg(test)]
pub(crate) fn select_native_macros_for_test_compiler(
    manifest_dir: &Path,
    graph: &mut serde_json::Value,
    compiler: &str,
    host: &str,
) -> Result<Vec<OvenStoreExecutionPayload>, RustMetadataError> {
    select_native_macros_with_probe(manifest_dir, graph, |_| Ok((compiler.to_string(), host.to_string())))
}

/// Probe the selected compiler only when the frozen graph actually needs native macro admission.
fn select_native_macros_with_probe(
    manifest_dir: &Path,
    graph: &mut serde_json::Value,
    probe: impl FnOnce(&Path) -> Result<(String, String), RustMetadataError>,
) -> Result<Vec<OvenStoreExecutionPayload>, RustMetadataError> {
    let path = manifest_dir.join(OVEN_DIRECT_PROC_MACRO_AUTHORITY_FILE);
    if contains_unowned_macro_path(graph) {
        return Err(invalid(
            &path,
            "native macro paths must come from admitted output authority",
        ));
    }
    let crates = graph
        .get_mut("crates")
        .and_then(serde_json::Value::as_array_mut)
        .ok_or_else(|| invalid(&path, "native inspection graph has no crates"))?;
    if !path.is_file() {
        return Ok(Vec::new());
    }
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Authority {
        schema_version: u32,
        macros: Vec<OvenInspectionProcMacro>,
    }
    let authority: Authority = serde_json::from_slice(&std::fs::read(&path)?)
        .map_err(|error| invalid(&path, &format!("invalid native macro authority: {error}")))?;
    if authority.schema_version != 1 {
        return Err(invalid(&path, "unsupported native macro authority schema"));
    }
    if authority.macros.is_empty() {
        return Ok(Vec::new());
    }
    let (compiler, host) = probe(&path)?;
    let mut stores = BTreeMap::<PathBuf, BTreeSet<String>>::new();
    let mut modules = BTreeSet::new();
    for entry in &authority.macros {
        if !modules.insert(entry.root_module.canonicalize()?) {
            return Err(invalid(&path, "native macro authority repeats a source module"));
        }
        let relative = Path::new(&entry.relative_path);
        if entry.relative_path.contains('\\')
            || relative.components().count() != 1
            || !matches!(relative.components().next(), Some(Component::Normal(_)))
            || !matches!(
                relative.extension().and_then(|ext| ext.to_str()),
                Some("dylib" | "so" | "dll")
            )
        {
            return Err(invalid(
                &path,
                "native macro output is not an owner-relative dynamic library",
            ));
        }
        stores
            .entry(entry.store.canonicalize()?)
            .or_default()
            .insert(entry.identity.clone());
    }
    let mut owners = Vec::new();
    for (store, identities) in stores {
        let selected_store = store.clone();
        let store = OvenStore::new(store, OvenStoreLimits::new(4 << 30, 4 << 30, 4 << 30));
        owners.extend(
            store
                .select_payloads_for_execution(&identities.into_iter().collect::<Vec<_>>())
                .map_err(|error| invalid(&path, &format!("cannot retain native macro owner: {error}")))?
                .into_iter()
                .map(|owner| (selected_store.clone(), owner)),
        );
    }
    for entry in authority.macros {
        let selected_store = entry.store.canonicalize()?;
        let owner = owners
            .iter()
            .find(|(store, owner)| store == &selected_store && owner.manifest.identity == entry.identity)
            .map(|(_, owner)| owner)
            .ok_or_else(|| invalid(&path, "native macro owner was not selected"))?;
        owner
            .verify_proven_native_payload()
            .map_err(|error| invalid(&path, &format!("native macro owner is not verified: {error}")))?;
        let root_module = entry.root_module.canonicalize()?;
        let source_root = owner.artifact_root.join("source").canonicalize()?;
        let source_manifest = super::read_inspection_source_manifest(&source_root)?;
        let library = source_manifest.get("lib");
        let declared_module = source_root
            .join(
                library
                    .and_then(|lib| lib.get("path"))
                    .and_then(toml::Value::as_str)
                    .unwrap_or("src/lib.rs"),
            )
            .canonicalize()?;
        if owner.manifest.receipt_identity != entry.receipt_identity
            || owner.manifest.domain != "sdk-source-unit-host"
            || owner.manifest.intent.toolchain != compiler
            || owner.manifest.intent.target != host
            || !root_module.starts_with(&source_root)
            || root_module != declared_module
            || library
                .and_then(|lib| lib.get("proc-macro"))
                .and_then(toml::Value::as_bool)
                != Some(true)
            || !owner
                .admitted_materialized_files()
                .iter()
                .any(|file| file.relative_path == entry.relative_path && file.digest == entry.digest)
        {
            return Err(invalid(
                &path,
                "native macro coordinates disagree with their source, receipt or toolchain owner",
            ));
        }
        let output = owner.artifact_root.join(&entry.relative_path).canonicalize()?;
        if !output.starts_with(owner.artifact_root.canonicalize()?) {
            return Err(invalid(&path, "native macro output escapes its retained owner"));
        }
        let mut matching = crates.iter_mut().filter(|record| {
            record.get("is_proc_macro").and_then(serde_json::Value::as_bool) == Some(true)
                && record
                    .get("root_module")
                    .and_then(serde_json::Value::as_str)
                    .and_then(|module| Path::new(module).canonicalize().ok())
                    == Some(root_module.clone())
        });
        let record = matching
            .next()
            .ok_or_else(|| invalid(&path, "native macro source is absent from the selected graph"))?;
        if matching.next().is_some() {
            return Err(invalid(&path, "native macro source is ambiguous in the selected graph"));
        }
        record["proc_macro_dylib_path"] = serde_json::json!(output);
    }
    Ok(owners.into_iter().map(|(_, owner)| owner).collect())
}

/// A nested sysroot or auxiliary crate graph cannot smuggle an executable path into an admitted projection.
pub(super) fn contains_unowned_macro_path(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Object(fields) => {
            fields.contains_key("proc_macro_dylib_path") || fields.values().any(contains_unowned_macro_path)
        }
        serde_json::Value::Array(values) => values.iter().any(contains_unowned_macro_path),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    /// Every graph level rejects a library path, including a field whose value is null.
    #[test]
    fn unowned_macro_paths_are_rejected_in_nested_graphs() {
        for graph in [
            serde_json::json!({"crates": [{"proc_macro_dylib_path": "/unowned.so"}]}),
            serde_json::json!({"sysroot_project": {"crates": [{"proc_macro_dylib_path": "/unowned.so"}]}}),
            serde_json::json!({"crates": [{"proc_macro_dylib_path": null}]}),
        ] {
            assert!(super::contains_unowned_macro_path(&graph));
        }
        assert!(!super::contains_unowned_macro_path(
            &serde_json::json!({"crates": [{"is_proc_macro": true}]})
        ));
    }
}

/// Read the same stable identity and host target used by the native publisher from the selected compiler.
fn active_compiler_identity(path: &Path) -> Result<(String, String), RustMetadataError> {
    let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let output = std::process::Command::new(rustc).arg("-vV").output()?;
    if !output.status.success() {
        return Err(invalid(path, "selected compiler could not report its identity"));
    }
    let text = std::str::from_utf8(&output.stdout).map_err(|error| invalid(path, &error.to_string()))?;
    let identity = text
        .lines()
        .next()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .ok_or_else(|| invalid(path, "selected compiler reported no identity"))?;
    let host = text
        .lines()
        .find_map(|line| line.strip_prefix("host: "))
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .ok_or_else(|| invalid(path, "selected compiler reported no host target"))?;
    Ok((identity.to_string(), host.to_string()))
}

/// Preserve the authority file as the diagnostic coordinate for an admission failure.
fn invalid(path: &Path, message: &str) -> RustMetadataError {
    RustMetadataError::LoadWorkspace {
        path: path.to_path_buf(),
        message: message.to_string(),
    }
}
