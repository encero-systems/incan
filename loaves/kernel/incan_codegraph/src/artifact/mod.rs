//! Versioned indexed Crumb-row Loaf graphs. The index locates independent records; decoding does not need source or a
//! checker.

mod crumb;
mod paths;
mod value;

use crate::CodegraphRecord;
use crumb::{decode_payload_row, encode_payload_row};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use value::GraphValue;

/// Layout version of `.incngraph`: little-endian u32 version, u64 index length, postcard index, then Crumb rows.
/// Version 3 added the index of the package origins the graph names.
pub const GRAPH_ARTIFACT_VERSION: u32 = 3;

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
    #[error(
        "graph artifact version {0}; this compiler reads version {GRAPH_ARTIFACT_VERSION}, the layout with the origins \
         index; rebuild the graph with this compiler"
    )]
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

/// Record ranges are decoded once; independently typed Crumb rows remain lazy.
#[derive(Serialize, Deserialize)]
struct Index {
    fields: Vec<String>,
    entries: Vec<Entry>,
    /// Every package origin a reference, call or import record names on its own or its bindings' identities, once,
    /// as its JSON text in text order: what dependency selection reads, so no reader decodes the records for it.
    origins: Vec<String>,
}

/// Encode compiler-owned facts without persisting JSON, physical source roots or span-bearing declaration IDs.
pub fn encode_graph(records: &[CodegraphRecord], root: &Path) -> Result<Vec<u8>, GraphArtifactError> {
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let ids = paths::identities(records, &root)?;
    let mut fields = BTreeMap::new();
    let mut payload = Vec::new();
    let mut entries = Vec::with_capacity(records.len());
    let mut origins = BTreeSet::new();
    for record in records {
        let mut wire = serde_json::to_value(record)?;
        paths::portable(&mut wire, &root, &ids);
        let kind = wire
            .get("record")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        if matches!(kind.as_str(), "reference" | "call" | "import") {
            package_origins(&wire, &mut origins)?;
        }
        let id = wire.get("id").and_then(Value::as_str).unwrap_or_default().to_owned();
        let document = postcard::to_allocvec(&GraphValue::pack(wire, &mut fields)?)
            .map_err(|error| GraphArtifactError::Malformed(error.to_string()))?;
        let (selector, model) = crumb_model(&kind)
            .ok_or_else(|| GraphArtifactError::Malformed(format!("unknown graph record kind {kind}")))?;
        let bytes = encode_payload_row(selector.to_vec(), model.to_string(), kind.clone(), id.clone(), document)
            .ok_or_else(|| GraphArtifactError::Malformed("cannot encode graph Crumb row".into()))?;
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
        origins: origins.into_iter().collect(),
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

/// Add the package origins one portable reference, call or import record names, on its own identities and its
/// bindings', to the graph's origins index as their JSON text.
fn package_origins(record: &Value, origins: &mut BTreeSet<String>) -> Result<(), GraphArtifactError> {
    let bindings = record.get("bindings").and_then(Value::as_array);
    for value in std::iter::once(record).chain(bindings.into_iter().flatten()) {
        for field in ["canonical_identity", "stable_identity"] {
            if let Some(origin) = value.get(field).and_then(|value| value.get("origin"))
                && origin.get("kind").and_then(Value::as_str) == Some("package")
            {
                origins.insert(serde_json::to_string(origin)?);
            }
        }
    }
    Ok(())
}

/// A validated artifact index and its uninterpreted payload. Readers can request record groups without decoding
/// unrelated facts.
pub struct GraphArtifact {
    index: Index,
    payload: Vec<u8>,
}

impl GraphArtifact {
    /// The compiler-owned package origins the graph's reference, call and import records name, read from the index the
    /// producer wrote, so no record is decoded. This is a storage projection for selecting dependency Loaves; package
    /// selection and closure traversal remain with the consumer.
    pub fn origins(&self) -> Result<Vec<Value>, GraphArtifactError> {
        self.value(
            self.index
                .entries
                .first()
                .ok_or_else(|| GraphArtifactError::Malformed("missing header".into()))?,
        )?;
        let mut origins = Vec::with_capacity(self.index.origins.len());
        for text in &self.index.origins {
            let origin: Value = serde_json::from_str(text)?;
            if origin.get("kind").and_then(Value::as_str) != Some("package") {
                return Err(GraphArtifactError::Malformed(
                    "the origins index names a non-package origin".into(),
                ));
            }
            origins.push(origin);
        }
        Ok(origins)
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
        self.projected_records(root, group, &BTreeMap::new())
    }

    /// Decode selected records with an optional consumer-owned field projection per record kind. Wire validation
    /// precedes projection, the complete header remains available, and retained source paths are restored without
    /// inferring any new facts.
    pub fn projected_records(
        &self,
        root: &Path,
        group: &str,
        fields: &BTreeMap<String, Vec<String>>,
    ) -> Result<Vec<Value>, GraphArtifactError> {
        let mut records = Vec::new();
        for entry in &self.index.entries {
            let selected = group.split(',').any(|group| {
                entry.kind == group || (group == "nodes" && matches!(entry.kind.as_str(), "module" | "declaration"))
            });
            if !group.is_empty() && entry.kind != "header" && !selected {
                continue;
            }
            let mut value = self.value(entry)?;
            if entry.kind != "header"
                && let Some(kept) = fields.get(&entry.kind)
                && let Some(object) = value.as_object_mut()
            {
                object.retain(|field, _| kept.contains(field));
            }
            paths::restore(&mut value, root);
            records.push(value);
        }
        Ok(records)
    }

    /// Decode one already range-checked payload and verify its index tag and export-local linkage key.
    fn value(&self, entry: &Entry) -> Result<Value, GraphArtifactError> {
        let offset = usize::try_from(entry.offset).map_err(|error| GraphArtifactError::Malformed(error.to_string()))?;
        let length = usize::try_from(entry.length).map_err(|error| GraphArtifactError::Malformed(error.to_string()))?;
        let row = decode_payload_row(&self.payload[offset..offset + length])
            .ok_or_else(|| GraphArtifactError::Malformed("invalid graph Crumb row".into()))?;
        let (selector, model) = crumb_model(&entry.kind)
            .ok_or_else(|| GraphArtifactError::Malformed(format!("unknown graph record kind {}", entry.kind)))?;
        if row.selector != selector || row.model != model || row.kind != entry.kind || row.id != entry.id {
            return Err(GraphArtifactError::Malformed("record does not match its index".into()));
        }
        let value: GraphValue =
            postcard::from_bytes(&row.payload).map_err(|error| GraphArtifactError::Malformed(error.to_string()))?;
        let value = value.unpack(&self.index.fields)?;
        if value.get("record").and_then(Value::as_str) != Some(row.kind.as_str())
            || value.get("id").and_then(Value::as_str).unwrap_or_default() != row.id
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

fn crumb_model(kind: &str) -> Option<(&'static [u8], &'static str)> {
    Some(match kind {
        "header" => (b"cghead\0", "incan.codegraph.header/2"),
        "file" => (b"cgfile\0", "incan.codegraph.file/2"),
        "namespace" => (b"cgnmsp\0", "incan.codegraph.namespace/2"),
        "module" => (b"cgmodu\0", "incan.codegraph.module/2"),
        "declaration" => (b"cgdecl\0", "incan.codegraph.declaration/2"),
        "import" => (b"cgimpt\0", "incan.codegraph.import/2"),
        "export" => (b"cgexpt\0", "incan.codegraph.export/2"),
        "reference" => (b"cgrefe\0", "incan.codegraph.reference/2"),
        "call" => (b"cgcall\0", "incan.codegraph.call/2"),
        "containment" => (b"cgcont\0", "incan.codegraph.containment/2"),
        "diagnostic" => (b"cgdiag\0", "incan.codegraph.diagnostic/2"),
        "registry" => (b"cgregi\0", "incan.codegraph.registry/2"),
        "c_binding" => (b"cgbind\0", "incan.codegraph.c-binding/2"),
        "c_binding_call" => (b"cgclal\0", "incan.codegraph.c-binding-call/2"),
        "c_binding_facade" => (b"cgfacd\0", "incan.codegraph.c-binding-facade/2"),
        "capability" => (b"cgcapb\0", "incan.codegraph.capability/2"),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::{GraphArtifact, GraphArtifactError, crumb_model, encode_graph};
    use crate::{
        CODEGRAPH_SCHEMA_VERSION, CodegraphFileRecord, CodegraphHeaderRecord, CodegraphLanguage, CodegraphMode,
        CodegraphProvenance, CodegraphRecord,
    };
    use serde_json::{Value, json};
    use std::collections::BTreeMap;
    use std::path::Path;

    #[test]
    fn all_sixteen_record_kinds_have_distinct_crumb_models() {
        let kinds = [
            "header",
            "file",
            "namespace",
            "module",
            "declaration",
            "import",
            "export",
            "reference",
            "call",
            "containment",
            "diagnostic",
            "registry",
            "c_binding",
            "c_binding_call",
            "c_binding_facade",
            "capability",
        ];
        let mut selectors = std::collections::BTreeSet::new();
        let mut models = std::collections::BTreeSet::new();
        for kind in kinds {
            let Some((selector, model)) = crumb_model(kind) else {
                panic!("missing Crumb model for {kind}");
            };
            assert_eq!(selector.len(), 7);
            assert!(selectors.insert(selector));
            assert!(models.insert(model));
        }
    }

    #[test]
    fn every_indexed_graph_record_is_an_independent_crumb_row() -> Result<(), Box<dyn std::error::Error>> {
        let records = vec![
            CodegraphRecord::Header(CodegraphHeaderRecord {
                schema_version: CODEGRAPH_SCHEMA_VERSION,
                compiler_version: "test".to_string(),
                mode: CodegraphMode::Strict,
                root_path: "src/main.incn".to_string(),
                languages: vec![CodegraphLanguage::Incan],
                package: None,
                semantic_contexts: Vec::new(),
                degraded: false,
            }),
            CodegraphRecord::File(CodegraphFileRecord {
                id: "file:src/main.incn".to_string(),
                language: CodegraphLanguage::Incan,
                path: "src/main.incn".to_string(),
                size_bytes: 12,
                provenance: CodegraphProvenance::Source,
                degraded: false,
            }),
        ];
        let encoded = encode_graph(&records, Path::new("."))?;
        let graph = GraphArtifact::decode(&encoded)?;
        for entry in &graph.index.entries {
            let offset = usize::try_from(entry.offset)?;
            let length = usize::try_from(entry.length)?;
            assert_eq!(&graph.payload[offset..offset + length][..4], b"CRMB");
        }
        let mut expected = records
            .iter()
            .map(serde_json::to_value)
            .collect::<Result<Vec<_>, _>>()?;
        for value in &mut expected {
            super::paths::restore(value, Path::new("."));
        }
        assert_eq!(graph.records(Path::new("."), "")?, expected);
        Ok(())
    }

    /// The projection the origins read made before the index existed, kept as the oracle: decode every reference,
    /// call and import record and collect the package origins on its own and its bindings' identities.
    fn decoded_origins(graph: &GraphArtifact) -> Result<Vec<Value>, GraphArtifactError> {
        let mut origins = BTreeMap::new();
        for entry in &graph.index.entries {
            if !matches!(entry.kind.as_str(), "reference" | "call" | "import") {
                continue;
            }
            let record = graph.value(entry)?;
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

    fn package(library: &str, module: &str) -> Value {
        json!({"kind": "package", "library": library, "module_path": [module]})
    }

    fn canonical(origin: Value, name: &str) -> Value {
        json!({
            "namespace": "value",
            "origin": origin,
            "declaration_name": name,
            "declaration_kind": "function",
            "scope_discriminant": null,
            "declaration_span": {"start": 0, "end": 1}
        })
    }

    fn stable(origin: Value, name: &str) -> Value {
        json!({
            "namespace": "value",
            "origin": origin,
            "declaration_name": name,
            "declaration_kind": "function",
            "location": {"kind": "module_level"}
        })
    }

    /// A header, then a reference, a call and an import whose identities and bindings name package, module and Rust
    /// origins, one package twice, and an export naming a package no reference, call or import names.
    fn origin_records() -> Result<Vec<CodegraphRecord>, serde_json::Error> {
        let rust_crate = json!({"kind": "rust_crate", "path": ["serde_json"]});
        let values = [
            json!({
                "record": "header",
                "schema_version": CODEGRAPH_SCHEMA_VERSION,
                "compiler_version": "test",
                "mode": "strict",
                "root_path": "src/main.incn",
                "languages": ["incan"],
                "package": null,
                "semantic_contexts": [],
                "degraded": false
            }),
            json!({
                "record": "reference",
                "id": "reference:1",
                "language": "incan",
                "module_id": "module:main",
                "name": "first",
                "kind": "identifier",
                "canonical_identity": canonical(package("lib_a", "m"), "first"),
                "stable_identity": stable(package("lib_b", "n"), "first"),
                "provenance": "checked",
                "degraded": false
            }),
            json!({
                "record": "call",
                "id": "call:1",
                "language": "incan",
                "module_id": "module:main",
                "callee": "second",
                "kind": "function",
                "argument_count": 0,
                "type_argument_count": 0,
                "canonical_identity": canonical(json!({"kind": "module", "path": ["main"]}), "second"),
                "stable_identity": stable(package("lib_c", "p"), "second"),
                "provenance": "checked",
                "degraded": false
            }),
            json!({
                "record": "import",
                "id": "import:1",
                "language": "incan",
                "module_id": "module:main",
                "kind": "from",
                "path": "lib_a.m",
                "items": ["first", "native", "third"],
                "bindings": [
                    {"local_name": "first", "canonical_identity": canonical(package("lib_a", "m"), "first")},
                    {"local_name": "native", "canonical_identity": canonical(rust_crate, "native")},
                    {"local_name": "third", "stable_identity": stable(package("lib_d", "q"), "third")}
                ],
                "visibility": "private",
                "provenance": "checked",
                "degraded": false
            }),
            json!({
                "record": "export",
                "id": "export:1",
                "language": "incan",
                "module_id": "module:main",
                "name": "exported",
                "kind": "declaration",
                "source_id": "call:1",
                "canonical_identity": canonical(package("lib_z", "z"), "exported"),
                "provenance": "checked",
                "degraded": false
            }),
        ];
        values.into_iter().map(serde_json::from_value).collect()
    }

    #[test]
    fn the_origins_index_holds_what_a_full_decode_of_the_records_collects() -> Result<(), Box<dyn std::error::Error>> {
        let graph = GraphArtifact::decode(&encode_graph(&origin_records()?, Path::new("."))?)?;
        let expected = vec![
            package("lib_a", "m"),
            package("lib_b", "n"),
            package("lib_c", "p"),
            package("lib_d", "q"),
        ];
        assert_eq!(decoded_origins(&graph)?, expected);
        assert_eq!(graph.origins()?, expected);
        Ok(())
    }

    #[test]
    fn the_origins_read_decodes_no_reference_call_or_import_record() -> Result<(), Box<dyn std::error::Error>> {
        let mut encoded = encode_graph(&origin_records()?, Path::new("."))?;
        let graph = GraphArtifact::decode(&encoded)?;
        let reference = graph
            .index
            .entries
            .iter()
            .find(|entry| entry.kind == "reference")
            .ok_or("the graph has no reference record")?;
        // Damage the reference row's Crumb magic: decoding that record now fails, while its range stays valid.
        let damaged = encoded.len() - graph.payload.len() + usize::try_from(reference.offset)?;
        encoded[damaged] ^= 0xff;
        let graph = GraphArtifact::decode(&encoded)?;
        assert!(graph.records(Path::new("."), "reference").is_err());
        assert_eq!(graph.origins()?.len(), 4);
        Ok(())
    }

    #[test]
    fn a_graph_without_the_origins_index_is_refused_with_its_reason() -> Result<(), Box<dyn std::error::Error>> {
        let mut encoded = encode_graph(&origin_records()?, Path::new("."))?;
        encoded[..4].copy_from_slice(&2u32.to_le_bytes());
        let Err(refusal) = GraphArtifact::decode(&encoded) else {
            return Err("a version 2 graph was read".into());
        };
        assert!(matches!(refusal, GraphArtifactError::Version(2)));
        let reason = refusal.to_string();
        assert!(reason.contains("version 2"), "{reason}");
        assert!(reason.contains("origins index"), "{reason}");
        assert!(reason.contains("rebuild"), "{reason}");
        Ok(())
    }

    /// The standard primitives graphs the selected installation ships, every profile it ships one for.
    fn installed_primitives_graphs(home: &Path) -> Result<Vec<std::path::PathBuf>, std::io::Error> {
        let mut graphs = Vec::new();
        for toolchain in std::fs::read_dir(home.join("toolchains"))? {
            let loaf = toolchain?.path().join("loaves/incan_stdlib_primitives");
            for profile in ["release", "debug"] {
                let graph = loaf.join(profile).join("incan_stdlib_primitives.incngraph");
                if graph.is_file() {
                    graphs.push(graph);
                }
            }
        }
        Ok(graphs)
    }

    /// The largest graph a release ships: its index must hold exactly what decoding all of its records collects. Run
    /// outside an installation, with no `INCAN_HOME`, there is no shipped graph to compare.
    #[test]
    fn the_installed_primitives_graph_indexes_the_origins_its_records_name() -> Result<(), Box<dyn std::error::Error>> {
        let Ok(home) = std::env::var("INCAN_HOME") else {
            return Ok(());
        };
        let graphs = installed_primitives_graphs(Path::new(&home))?;
        assert!(!graphs.is_empty(), "{home} ships no standard primitives graph");
        for path in graphs {
            let graph = GraphArtifact::decode(&std::fs::read(&path)?)?;
            let indexed = graph.origins()?;
            assert!(!indexed.is_empty(), "{} names no package origin", path.display());
            assert_eq!(indexed, decoded_origins(&graph)?, "{}", path.display());
        }
        Ok(())
    }
}
