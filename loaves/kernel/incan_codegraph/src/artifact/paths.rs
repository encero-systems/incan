//! Portable graph source anchors and export-local links; semantic identities are never reconstructed here.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use serde_json::Value;
use sha2::{Digest, Sha256};

use super::GraphArtifactError;
use crate::CodegraphRecord;

/// Map declarations from compiler-proven span-free identities and make every other local linkage opaque. File and
/// call occurrence IDs remain export-local; their source anchors live separately in Loaf-relative span fields.
pub(super) fn identities(
    records: &[CodegraphRecord],
    root: &Path,
) -> Result<BTreeMap<String, String>, GraphArtifactError> {
    let scope = records
        .iter()
        .find_map(|record| match record {
            CodegraphRecord::Header(header) => header.package.as_ref().map(|package| {
                format!(
                    "{}@{}",
                    package.name.as_deref().unwrap_or_default(),
                    package.version.as_deref().unwrap_or_default()
                )
            }),
            _ => None,
        })
        .unwrap_or_default();
    let mut result = BTreeMap::new();
    for record in records {
        let value = serde_json::to_value(record)?;
        let Some(id) = value.get("id").and_then(Value::as_str) else {
            continue;
        };
        let identity = match record {
            CodegraphRecord::Declaration(declaration)
                if declaration.language == crate::CodegraphLanguage::Incan && declaration.stable_identity.is_some() =>
            {
                serde_json::to_vec(&declaration.stable_identity)?
            }
            _ if id.contains(root.to_string_lossy().as_ref()) => {
                id.replace(root.to_string_lossy().as_ref(), "@loaf").into_bytes()
            }
            _ => {
                result.insert(id.to_owned(), id.to_owned());
                continue;
            }
        };
        let mut digest = Sha256::new();
        digest.update(scope.as_bytes());
        digest.update([0]);
        digest.update(identity);
        result.insert(id.to_owned(), format!("graph:{:x}", digest.finalize()));
    }
    Ok(result)
}

/// Relativize a source anchor lexically, including legitimate shared sources outside a Loaf's own directory.
fn relative(root: &Path, path: &str) -> String {
    let path = Path::new(path);
    if !path.is_absolute() {
        return path.to_string_lossy().into_owned();
    }
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            component => normalized.push(component.as_os_str()),
        }
    }
    let path = normalized.as_path();
    let mut base = root;
    let mut prefix = PathBuf::new();
    while !path.starts_with(base) {
        let Some(parent) = base.parent() else {
            return path.to_string_lossy().into_owned();
        };
        base = parent;
        prefix.push("..");
    }
    let suffix = path.strip_prefix(base).unwrap_or(path);
    let result = prefix.join(suffix);
    if result.as_os_str().is_empty() {
        ".".into()
    } else {
        result.to_string_lossy().into_owned()
    }
}

/// Transform only graph path fields and source spans; user-authored strings in registry values stay intact.
fn source_paths(value: &mut Value, root: &Path, restore: bool) {
    match value {
        Value::Array(values) => {
            for value in values {
                source_paths(value, root, restore);
            }
        }
        Value::Object(fields) => {
            let source_span = fields.contains_key("start_line") && fields.contains_key("end_line");
            let file_record = fields.get("record").and_then(Value::as_str) == Some("file");
            for (key, value) in fields {
                if ((source_span && key == "file")
                    || (file_record && key == "path")
                    || matches!(key.as_str(), "root_path" | "project_root" | "source_root"))
                    && value.is_string()
                {
                    if let Some(path) = value.as_str() {
                        *value = Value::String(if restore {
                            root.join(path).to_string_lossy().into_owned()
                        } else {
                            relative(root, path)
                        });
                    }
                } else if !matches!(key.as_str(), "value" | "metadata" | "attributes") {
                    source_paths(value, root, restore);
                }
            }
        }
        _ => {}
    }
}

/// Store Loaf-relative source anchors and replace only typed top-level graph linkage fields.
pub(super) fn portable(value: &mut Value, root: &Path, ids: &BTreeMap<String, String>) {
    source_paths(value, root, false);
    if let Some(fields) = value.as_object_mut() {
        for key in [
            "id",
            "file_id",
            "module_id",
            "owner_id",
            "target_id",
            "parent_id",
            "child_id",
            "declaration_id",
            "import_id",
            "source_id",
        ] {
            if let Some(value) = fields.get_mut(key) {
                if let Some(mapped) = value.as_str().and_then(|id| ids.get(id)) {
                    *value = Value::String(mapped.clone());
                }
            }
        }
    }
}

/// Rebase source anchors for presentation at the consumer's known Loaf source root; graph identities stay portable.
pub(super) fn restore(value: &mut Value, root: &Path) {
    source_paths(value, root, true);
}
