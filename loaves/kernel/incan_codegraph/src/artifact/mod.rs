//! Versioned indexed-postcard Loaf graphs. The index locates independent records; decoding does not need source or a
//! checker.

mod paths;
mod value;

use crate::CodegraphRecord;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::Path;
use value::GraphValue;

/// Layout version of `.incngraph`: little-endian u32 version, u64 index length, postcard index, then postcard records.
pub const GRAPH_ARTIFACT_VERSION: u32 = 1;

/// A malformed or unsupported graph never supplies partial semantic facts to an inspection consumer.
#[derive(Debug, thiserror::Error)]
pub enum GraphArtifactError {
    /// The producer or reader could not encode or decode a graph wire value.
    #[error("invalid graph wire value: {0}")]
    Wire(#[from] serde_json::Error),
    /// The artifact's header, dictionary, index or payload violates the versioned binary layout.
    #[error("malformed graph artifact: {0}")]
    Malformed(String),
    /// The installed reader does not implement the artifact's layout version.
    #[error("graph artifact version {0}; this compiler reads version {GRAPH_ARTIFACT_VERSION}")]
    Version(u32),
}

/// One independently decodable graph record in payload order.
#[derive(Serialize, Deserialize)]
struct Entry {
    kind: String,
    id: String,
    offset: u64,
    length: u64,
}

/// Shared field names and record ranges are decoded once; record payloads remain lazy.
#[derive(Serialize, Deserialize)]
struct Index {
    fields: Vec<String>,
    entries: Vec<Entry>,
}

/// Encode compiler-owned facts without persisting JSON, physical source roots or span-bearing declaration IDs.
pub fn encode_graph(records: &[CodegraphRecord], root: &Path) -> Result<Vec<u8>, GraphArtifactError> {
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let ids = paths::identities(records, &root)?;
    let mut fields = BTreeMap::new();
    let mut payload = Vec::new();
    let mut entries = Vec::with_capacity(records.len());
    for record in records {
        let mut wire = serde_json::to_value(record)?;
        paths::portable(&mut wire, &root, &ids);
        let kind = wire
            .get("record")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let id = wire.get("id").and_then(Value::as_str).unwrap_or_default().to_owned();
        let bytes = postcard::to_allocvec(&GraphValue::pack(wire, &mut fields)?)
            .map_err(|error| GraphArtifactError::Malformed(error.to_string()))?;
        entries.push(Entry {
            kind,
            id,
            offset: u64::try_from(payload.len()).map_err(|error| GraphArtifactError::Malformed(error.to_string()))?,
            length: u64::try_from(bytes.len()).map_err(|error| GraphArtifactError::Malformed(error.to_string()))?,
        });
        payload.extend_from_slice(&bytes);
    }
    let mut names: Vec<_> = fields.into_iter().collect();
    names.sort_by_key(|(_, index)| *index);
    let index = Index {
        fields: names.into_iter().map(|(name, _)| name).collect(),
        entries,
    };
    let index = postcard::to_allocvec(&index).map_err(|error| GraphArtifactError::Malformed(error.to_string()))?;
    let mut bytes = GRAPH_ARTIFACT_VERSION.to_le_bytes().to_vec();
    bytes.extend_from_slice(
        &u64::try_from(index.len())
            .map_err(|error| GraphArtifactError::Malformed(error.to_string()))?
            .to_le_bytes(),
    );
    bytes.extend_from_slice(&index);
    bytes.extend_from_slice(&payload);
    Ok(bytes)
}

/// A validated artifact index and its uninterpreted payload. Readers can request record groups without decoding
/// unrelated facts.
pub struct GraphArtifact {
    index: Index,
    payload: Vec<u8>,
}

impl GraphArtifact {
    /// Project compiler-owned package origins from reference, call and import records. This is a storage
    /// projection for selecting dependency Loaves; package selection and closure traversal remain with the consumer.
    pub fn origins(&self) -> Result<Vec<Value>, GraphArtifactError> {
        let mut origins = std::collections::BTreeMap::new();
        self.value(
            self.index
                .entries
                .first()
                .ok_or_else(|| GraphArtifactError::Malformed("missing header".into()))?,
        )?;
        for entry in &self.index.entries {
            if !matches!(entry.kind.as_str(), "reference" | "call" | "import") {
                continue;
            }
            let record = self.value(entry)?;
            let bindings = record.get("bindings").and_then(Value::as_array);
            for value in std::iter::once(&record).chain(bindings.into_iter().flatten()) {
                for field in ["canonical_identity", "stable_identity"] {
                    if let Some(origin) = value.get(field).and_then(|value| value.get("origin"))
                        && origin.get("kind").and_then(Value::as_str) == Some("package")
                    {
                        origins.insert(serde_json::to_string(origin)?, origin.clone());
                    }
                }
            }
        }
        Ok(origins.into_values().collect())
    }

    /// Decode and validate the complete directory of record ranges before exposing any graph values.
    pub fn decode(bytes: &[u8]) -> Result<Self, GraphArtifactError> {
        let malformed = || GraphArtifactError::Malformed("invalid header or record range".into());
        let version = u32::from_le_bytes(
            bytes
                .get(..4)
                .ok_or_else(malformed)?
                .try_into()
                .map_err(|_| malformed())?,
        );
        if version != GRAPH_ARTIFACT_VERSION {
            return Err(GraphArtifactError::Version(version));
        }
        let length = u64::from_le_bytes(
            bytes
                .get(4..12)
                .ok_or_else(malformed)?
                .try_into()
                .map_err(|_| malformed())?,
        );
        let end = 12usize
            .checked_add(usize::try_from(length).map_err(|_| malformed())?)
            .ok_or_else(malformed)?;
        let index: Index = postcard::from_bytes(bytes.get(12..end).ok_or_else(malformed)?)
            .map_err(|error| GraphArtifactError::Malformed(error.to_string()))?;
        let payload = bytes.get(end..).ok_or_else(malformed)?.to_vec();
        if index.entries.first().is_none_or(|entry| entry.kind != "header")
            || index.entries.iter().filter(|entry| entry.kind == "header").count() != 1
        {
            return Err(GraphArtifactError::Malformed(
                "expected exactly one leading header".into(),
            ));
        }
        let mut next = 0usize;
        for entry in &index.entries {
            let offset = usize::try_from(entry.offset).map_err(|_| malformed())?;
            let length = usize::try_from(entry.length).map_err(|_| malformed())?;
            let stop = offset.checked_add(length).ok_or_else(malformed)?;
            if offset != next || stop > payload.len() {
                return Err(malformed());
            }
            next = stop;
        }
        if next != payload.len() {
            return Err(malformed());
        }
        Ok(Self { index, payload })
    }

    /// Decode selected wire records for JSON query output. An empty group exports everything; `nodes` keeps module
    /// and declaration nodes, and every group retains headers so provenance remains available to consumers.
    pub fn records(&self, root: &Path, group: &str) -> Result<Vec<Value>, GraphArtifactError> {
        let mut records = Vec::new();
        for entry in &self.index.entries {
            let selected = group.split(',').any(|group| {
                entry.kind == group || (group == "nodes" && matches!(entry.kind.as_str(), "module" | "declaration"))
            });
            if !group.is_empty() && entry.kind != "header" && !selected {
                continue;
            }
            let mut value = self.value(entry)?;
            paths::restore(&mut value, root);
            records.push(value);
        }
        Ok(records)
    }

    /// Decode one already range-checked payload and verify its index tag and export-local linkage key.
    fn value(&self, entry: &Entry) -> Result<Value, GraphArtifactError> {
        let offset = usize::try_from(entry.offset).map_err(|error| GraphArtifactError::Malformed(error.to_string()))?;
        let length = usize::try_from(entry.length).map_err(|error| GraphArtifactError::Malformed(error.to_string()))?;
        let value: GraphValue = postcard::from_bytes(&self.payload[offset..offset + length])
            .map_err(|error| GraphArtifactError::Malformed(error.to_string()))?;
        let value = value.unpack(&self.index.fields)?;
        if value.get("record").and_then(Value::as_str) != Some(entry.kind.as_str())
            || value.get("id").and_then(Value::as_str).unwrap_or_default() != entry.id
        {
            return Err(GraphArtifactError::Malformed("record does not match its index".into()));
        }
        if entry.kind == "header"
            && value.get("schema_version").and_then(Value::as_u64) != Some(u64::from(crate::CODEGRAPH_SCHEMA_VERSION))
        {
            return Err(GraphArtifactError::Malformed("unsupported graph wire schema".into()));
        }
        Ok(value)
    }
}
