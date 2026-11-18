//! One-time conversion of installed Loaf manifests into validated compiler-record values (#2946).
//!
//! This command is an installation boundary, not a compilation fallback. It is the only records-mode path that
//! opens `.incnlib` artifacts: the candidate compiler validates the manifest with its owning semantic model, then
//! writes independently content-named values for Oven to admit through the sealed-set path. Which records a Loaf has,
//! and their two digests, are the front end's (`LibraryManifest::compiler_records`); this module only writes them.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use incan_frontend::library_manifest::{
    LibraryManifest, capabilities::CapabilityCatalog, compiler_records::RecordFamily,
};
use serde::Serialize;
use serde_json::json;
use sha2::{Digest, Sha256};

const EXPORT_SCHEMA: &str = "incan.compiler-record-export/2";

/// One value the compiler validated and Oven will place in the sealed set.
#[derive(Serialize)]
struct ExportedRecord {
    key: String,
    revision: String,
    object: String,
}

/// Convert every installed standard Loaf manifest below `loaves` into independently addressed record values.
pub(crate) fn export(arguments: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let [installed, output] = arguments else {
        return Err("usage: incan-rustc-driver --compiler-records INSTALLED OUTPUT".into());
    };
    let installed = Path::new(installed);
    let output = Path::new(output);
    if output.exists() {
        return Err(format!("compiler-record export already exists: {}", output.display()).into());
    }
    fs::create_dir_all(output.join("objects"))?;
    let mut manifests = manifest_paths(installed)?;
    manifests.sort();
    let mut records = Vec::new();
    let mut catalogs = BTreeMap::<String, CapabilityCatalog>::new();
    for path in manifests {
        let manifest_digest = format!("{:x}", Sha256::digest(fs::read(&path)?));
        let manifest = LibraryManifest::read_from_path(&path)?;
        let profile = path
            .parent()
            .and_then(Path::file_name)
            .and_then(|name| name.to_str())
            .ok_or_else(|| format!("manifest has no UTF-8 profile directory: {}", path.display()))?;
        let catalog = matches!(profile, "debug" | "release").then(|| catalogs.entry(profile.to_owned()).or_default());
        export_manifest(output, profile, &manifest_digest, &manifest, &mut records, catalog)?;
        if manifest.contract_metadata.executable_representation.is_some() {
            let sidecar = path.with_extension("incnbody");
            match fs::read(&sidecar) {
                Ok(bytes) => {
                    let prefix = format!("loaf/{profile}/{}", manifest.name);
                    for (key, value) in incan_frontend::library_manifest::executable_ranges::export_ranges(
                        &manifest,
                        &manifest_digest,
                        &bytes,
                    )? {
                        write_object(output, format!("{prefix}/{key}"), value, &mut records)?;
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
    }
    for (profile, catalog) in catalogs {
        let (key, value) = catalog.encode(&profile)?;
        write_object(output, key, value, &mut records)?;
    }
    records.sort_by(|left, right| left.key.cmp(&right.key));
    let index = json!({"schema": EXPORT_SCHEMA, "records": records});
    fs::write(output.join("index.json"), serde_json::to_vec(&index)?)?;
    Ok(())
}

/// Inventory ordinary manifest files without following directory symlinks or accepting another artifact suffix.
fn manifest_paths(root: &Path) -> io::Result<Vec<PathBuf>> {
    let mut pending = vec![root.to_path_buf()];
    let mut manifests = Vec::new();
    while let Some(directory) = pending.pop() {
        let mut entries = fs::read_dir(directory)?.collect::<Result<Vec<_>, _>>()?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let kind = entry.file_type()?;
            if kind.is_symlink() {
                continue;
            }
            if kind.is_dir() {
                pending.push(entry.path());
            } else if kind.is_file() && entry.path().extension().is_some_and(|extension| extension == "incnlib") {
                manifests.push(entry.path());
            }
        }
    }
    Ok(manifests)
}

/// Write one validated manifest's compiler records as content-named objects and list their keys.
fn export_manifest(
    output: &Path,
    profile: &str,
    manifest_digest: &str,
    manifest: &LibraryManifest,
    records: &mut Vec<ExportedRecord>,
    mut catalog: Option<&mut CapabilityCatalog>,
) -> Result<(), Box<dyn std::error::Error>> {
    let prefix = format!("loaf/{profile}/{}", manifest.name);
    for record in manifest.compiler_records(manifest_digest)?.records {
        if record.family == RecordFamily::Metadata
            && let Some(catalog) = catalog.as_deref_mut()
        {
            catalog.insert(&manifest.name, &record.value)?;
        }
        write_object(output, format!("{prefix}/{}", record.key()), record.value, records)?;
    }
    Ok(())
}

/// Address and write one encoded record value without combining it with sibling records.
fn write_object(
    output: &Path,
    key: String,
    bytes: Vec<u8>,
    records: &mut Vec<ExportedRecord>,
) -> Result<(), Box<dyn std::error::Error>> {
    let revision = format!("{:x}", Sha256::digest(&bytes));
    let object = format!("objects/{revision}");
    let path = output.join(&object);
    if path.exists() {
        if fs::read(&path)? != bytes {
            return Err(format!("compiler-record object collision at {}", path.display()).into());
        }
    } else {
        fs::write(&path, &bytes)?;
    }
    records.push(ExportedRecord { key, revision, object });
    Ok(())
}
