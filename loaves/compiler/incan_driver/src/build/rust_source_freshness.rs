//! Stat-guarded acceleration of the existing recursive Rust source authority, without changing its digest scheme.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::file_freshness::{FileStamp, stat_file};
use crate::error::{CliError, CliResult};

/// Exact source roots visited by the authoritative digester and the observation guarding their aggregate digest.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ClosureRecord {
    schema: String,
    roots: Vec<PathBuf>,
    nodes: BTreeMap<PathBuf, String>,
    workspace_manifests: Vec<PathBuf>,
    stamp: String,
    digest: String,
}

/// Bind every regular file in the visited packages and their ancestor workspace manifests.
///
/// Coverage is deliberately conservative. Only the generated directories excluded by the owning Cargo source
/// authority are skipped; extra source files may cause a miss but cannot hide a semantic change. Symlinks refuse
/// acceleration rather than pretending their targets belong to this observed closure.
fn closure_stamp(roots: &[PathBuf], workspace_manifests: &[PathBuf]) -> CliResult<String> {
    let mut files = BTreeMap::<PathBuf, FileStamp>::new();
    for root in roots {
        let mut pending = vec![root.clone()];
        while let Some(directory) = pending.pop() {
            for entry in fs::read_dir(&directory).map_err(|error| CliError::failure(error.to_string()))? {
                let entry = entry.map_err(|error| CliError::failure(error.to_string()))?;
                let path = entry.path();
                let kind = entry
                    .file_type()
                    .map_err(|error| CliError::failure(error.to_string()))?;
                if kind.is_dir() {
                    if !matches!(
                        entry.file_name().to_str(),
                        Some(".git" | ".incan" | ".ralph-cache" | "target")
                    ) {
                        pending.push(path);
                    }
                } else {
                    files.insert(
                        path.clone(),
                        stat_file(&path).map_err(|error| CliError::failure(error.to_string()))?,
                    );
                }
            }
        }
        for ancestor in root.ancestors().skip(1) {
            let manifest = ancestor.join("Cargo.toml");
            if manifest
                .try_exists()
                .map_err(|error| CliError::failure(error.to_string()))?
            {
                files.insert(
                    manifest.clone(),
                    stat_file(&manifest).map_err(|error| CliError::failure(error.to_string()))?,
                );
            }
        }
    }
    for manifest in workspace_manifests {
        files.insert(
            manifest.clone(),
            stat_file(manifest).map_err(|error| CliError::failure(error.to_string()))?,
        );
    }
    let bytes = serde_json::to_vec(&files).map_err(|error| CliError::failure(error.to_string()))?;
    Ok(format!("sha256:{:x}", Sha256::digest(bytes)))
}

/// Capture explicit workspace pointers, which may name a sibling outside every visited package's ancestors.
fn explicit_workspace_manifests(roots: &[PathBuf]) -> CliResult<Vec<PathBuf>> {
    let mut manifests = Vec::new();
    for root in roots {
        let bytes =
            fs::read_to_string(root.join("Cargo.toml")).map_err(|error| CliError::failure(error.to_string()))?;
        let manifest: toml::Value = toml::from_str(&bytes).map_err(|error| CliError::failure(error.to_string()))?;
        if let Some(workspace) = manifest
            .get("package")
            .and_then(|package| package.get("workspace"))
            .and_then(toml::Value::as_str)
        {
            manifests.push(root.join(workspace).join("Cargo.toml"));
        }
    }
    manifests.sort();
    manifests.dedup();
    Ok(manifests)
}

/// Reuse only an observed source graph; cold observations run twice so the stored stamp cannot bless older bytes.
pub(super) fn digest(
    root: &Path,
    memo: &mut BTreeMap<PathBuf, String>,
    mut compute: impl FnMut(&mut BTreeMap<PathBuf, String>) -> CliResult<String>,
) -> CliResult<String> {
    if !cfg!(unix) {
        return compute(&mut BTreeMap::new());
    }
    let root = fs::canonicalize(root).map_err(|error| CliError::failure(error.to_string()))?;
    if memo.contains_key(&root) {
        return compute(memo);
    }
    let cache = std::env::var_os("INCAN_HOME")
        .filter(|home| !home.is_empty())
        .map(|home| {
            PathBuf::from(home).join("cache/rust-source-digests-v1").join(format!(
                "{:x}.json",
                Sha256::digest(root.as_os_str().as_encoded_bytes())
            ))
        });
    if let Some(record) = cache
        .as_ref()
        .and_then(|path| fs::read(path).ok())
        .and_then(|bytes| serde_json::from_slice::<ClosureRecord>(&bytes).ok())
        && record.schema == "cargo-path-authority-stat/3"
        && record.roots.contains(&root)
        && record.nodes.keys().eq(record.roots.iter())
        && record
            .digest
            .strip_prefix("sha256:")
            .is_some_and(|hex| hex.len() == 64 && hex.bytes().all(|byte| byte.is_ascii_hexdigit()))
        && closure_stamp(&record.roots, &record.workspace_manifests).ok().as_ref() == Some(&record.stamp)
    {
        memo.extend(record.nodes);
        return Ok(record.digest);
    }
    let mut visited = BTreeMap::new();
    let initial = compute(&mut visited)?;
    let roots = visited.keys().cloned().collect::<Vec<_>>();
    let workspace_manifests = explicit_workspace_manifests(&roots)?;
    let before = match closure_stamp(&roots, &workspace_manifests) {
        Ok(stamp) if roots.contains(&root) => stamp,
        _ => return Ok(initial),
    };
    let mut confirmed = BTreeMap::new();
    let digest = compute(&mut confirmed)?;
    if confirmed.keys().ne(visited.keys()) || closure_stamp(&roots, &workspace_manifests).ok().as_ref() != Some(&before)
    {
        return Err(CliError::failure(
            "Rust source authority changed while observing its freshness",
        ));
    }
    if let Some(cache) = cache {
        let record = ClosureRecord {
            schema: "cargo-path-authority-stat/3".into(),
            roots,
            nodes: confirmed.clone(),
            workspace_manifests,
            stamp: before,
            digest: digest.clone(),
        };
        let _published = publish(&cache, &record);
    }
    memo.extend(confirmed);
    Ok(digest)
}

/// Publish a complete local observation atomically, keeping cache I/O optional.
fn publish(path: &Path, record: &ClosureRecord) -> std::io::Result<()> {
    use std::io::Write;
    let parent = path
        .parent()
        .ok_or_else(|| std::io::Error::other("source cache has no parent"))?;
    fs::create_dir_all(parent)?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    temporary.write_all(&serde_json::to_vec(record)?)?;
    temporary.persist(path).map_err(|error| error.error)?;
    Ok(())
}
