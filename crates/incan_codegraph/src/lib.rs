//! Stable code-index graph facts for Incan tooling.
//!
//! This crate is deliberately storage-agnostic. It does not know about CodeGraph, SurrealDB, embeddings, MCP, or the
//! compiler pipeline. The compiler/tooling layer extracts authoritative facts and uses these types as the wire format
//! for downstream indexers.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Current `incan_codegraph` schema version.
pub const CODEGRAPH_SCHEMA_VERSION: &str = "incan-codegraph.v1";

/// A stable, serializable code-index document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodegraphDocument {
    /// Schema version for downstream compatibility checks.
    pub schema_version: String,
    /// Optional package identity for project-level exports.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub package: Option<CodegraphPackage>,
    /// Graph nodes in deterministic order.
    pub nodes: Vec<CodegraphNode>,
    /// Graph edges in deterministic order.
    pub edges: Vec<CodegraphEdge>,
}

impl CodegraphDocument {
    /// Create an empty document with the current schema version.
    #[must_use]
    pub fn new(package: Option<CodegraphPackage>) -> Self {
        Self {
            schema_version: CODEGRAPH_SCHEMA_VERSION.to_string(),
            package,
            nodes: Vec::new(),
            edges: Vec::new(),
        }
    }

    /// Append one node to the document.
    pub fn push_node(&mut self, node: CodegraphNode) {
        self.nodes.push(node);
    }

    /// Append one edge to the document.
    pub fn push_edge(&mut self, edge: CodegraphEdge) {
        self.edges.push(edge);
    }

    /// Serialize the document as stable pretty JSON.
    pub fn to_pretty_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }

    /// Serialize the document as newline-delimited records: a header, then nodes, then edges.
    pub fn to_jsonl(&self) -> Result<String, serde_json::Error> {
        let mut lines = Vec::with_capacity(self.nodes.len() + self.edges.len() + 1);
        lines.push(serde_json::to_string(&CodegraphJsonlRecord::Document {
            schema_version: self.schema_version.clone(),
            package: self.package.clone(),
        })?);
        for node in &self.nodes {
            lines.push(serde_json::to_string(&CodegraphJsonlRecord::Node(node.clone()))?);
        }
        for edge in &self.edges {
            lines.push(serde_json::to_string(&CodegraphJsonlRecord::Edge(edge.clone()))?);
        }
        lines.push(String::new());
        Ok(lines.join("\n"))
    }
}

/// Project/package identity attached to a code-index export.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodegraphPackage {
    /// Project name from `incan.toml`, when present.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Project version from `incan.toml`, when present.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// Export root path, normalized by the producing command.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub root_path: Option<String>,
}

/// One node in the code-index graph.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodegraphNode {
    /// Stable node id unique within one export.
    pub id: String,
    /// Node kind.
    pub kind: CodegraphNodeKind,
    /// Human-readable label.
    pub label: String,
    /// Source file path, when the node is source-backed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_path: Option<String>,
    /// Logical Incan module path, when known.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub module_path: Vec<String>,
    /// Byte-span in the source file, when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub span: Option<CodegraphSpan>,
    /// Additional stable facts for indexers that do not need a new schema field yet.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub facts: BTreeMap<String, String>,
}

/// Supported node categories.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodegraphNodeKind {
    /// Project/package root node.
    Package,
    /// Source file node.
    File,
    /// Incan module node.
    Module,
    /// Source declaration node.
    Declaration,
    /// Checked public API member derived from RFC 048 metadata.
    ApiMember,
    /// Import declaration node.
    Import,
    /// Source match expression dispatch over a syntactic domain.
    MatchDispatch,
    /// Source call expression or constructor invocation.
    CallSite,
    /// Source identifier, receiver, or field reference.
    Reference,
    /// External or unresolved import target.
    External,
}

/// One directed relationship between graph nodes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodegraphEdge {
    /// Stable edge id unique within one export.
    pub id: String,
    /// Relationship kind.
    pub kind: CodegraphEdgeKind,
    /// Source node id.
    pub source_id: String,
    /// Target node id.
    pub target_id: String,
    /// Source span for the relationship, when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub span: Option<CodegraphSpan>,
    /// Additional stable facts for downstream indexers.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub facts: BTreeMap<String, String>,
}

/// Supported relationship categories.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodegraphEdgeKind {
    /// Parent contains child.
    Contains,
    /// Module or declaration defines a symbol.
    Defines,
    /// Module imports or depends on another module/symbol.
    Imports,
    /// Call site targets a syntactic callee.
    Calls,
    /// Reference site targets a syntactic symbol/reference.
    References,
}

/// A byte-span in one source file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodegraphSpan {
    /// Start byte offset, inclusive.
    pub start: usize,
    /// End byte offset, exclusive.
    pub end: usize,
}

/// Newline-delimited record shape.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "record", rename_all = "snake_case")]
pub enum CodegraphJsonlRecord {
    /// Document header.
    Document {
        /// Schema version for downstream compatibility checks.
        schema_version: String,
        /// Optional package identity for project-level exports.
        #[serde(skip_serializing_if = "Option::is_none")]
        package: Option<CodegraphPackage>,
    },
    /// Graph node record.
    Node(CodegraphNode),
    /// Graph edge record.
    Edge(CodegraphEdge),
}

#[cfg(test)]
mod tests {
    use super::{
        CODEGRAPH_SCHEMA_VERSION, CodegraphDocument, CodegraphEdge, CodegraphEdgeKind, CodegraphNode,
        CodegraphNodeKind, CodegraphPackage, CodegraphSpan,
    };

    #[test]
    fn pretty_json_carries_schema_and_nodes() -> Result<(), Box<dyn std::error::Error>> {
        let mut document = CodegraphDocument::new(Some(CodegraphPackage {
            name: Some("demo".to_string()),
            version: Some("0.1.0".to_string()),
            root_path: None,
        }));
        document.push_node(CodegraphNode {
            id: "file:src/main.incn".to_string(),
            kind: CodegraphNodeKind::File,
            label: "src/main.incn".to_string(),
            file_path: Some("src/main.incn".to_string()),
            module_path: Vec::new(),
            span: None,
            facts: Default::default(),
        });

        let json = document.to_pretty_json()?;

        assert!(json.contains(CODEGRAPH_SCHEMA_VERSION));
        assert!(json.contains("\"kind\": \"file\""));
        Ok(())
    }

    #[test]
    fn jsonl_emits_header_nodes_then_edges() -> Result<(), Box<dyn std::error::Error>> {
        let mut document = CodegraphDocument::new(None);
        document.push_node(CodegraphNode {
            id: "module:main".to_string(),
            kind: CodegraphNodeKind::Module,
            label: "main".to_string(),
            file_path: None,
            module_path: vec!["main".to_string()],
            span: None,
            facts: Default::default(),
        });
        document.push_edge(CodegraphEdge {
            id: "edge:module:main:contains:decl:main.main".to_string(),
            kind: CodegraphEdgeKind::Contains,
            source_id: "module:main".to_string(),
            target_id: "decl:main.main".to_string(),
            span: Some(CodegraphSpan { start: 0, end: 3 }),
            facts: Default::default(),
        });

        let jsonl = document.to_jsonl()?;
        let lines = jsonl.lines().collect::<Vec<_>>();

        assert_eq!(lines.len(), 3);
        assert!(lines[0].contains("\"record\":\"document\""));
        assert!(lines[1].contains("\"record\":\"node\""));
        assert!(lines[2].contains("\"record\":\"edge\""));
        Ok(())
    }

    #[test]
    fn pretty_json_supports_body_fact_kinds() -> Result<(), Box<dyn std::error::Error>> {
        let mut document = CodegraphDocument::new(None);
        document.push_node(CodegraphNode {
            id: "body-fact:match:main:label:10-40".to_string(),
            kind: CodegraphNodeKind::MatchDispatch,
            label: "match kind".to_string(),
            file_path: Some("main.incn".to_string()),
            module_path: vec!["main".to_string()],
            span: Some(CodegraphSpan { start: 10, end: 40 }),
            facts: Default::default(),
        });
        document.push_edge(CodegraphEdge {
            id: "edge:body-fact:call:target".to_string(),
            kind: CodegraphEdgeKind::Calls,
            source_id: "body-fact:call".to_string(),
            target_id: "external:target".to_string(),
            span: Some(CodegraphSpan { start: 12, end: 20 }),
            facts: Default::default(),
        });

        let json = document.to_pretty_json()?;

        assert!(json.contains("\"kind\": \"match_dispatch\""));
        assert!(json.contains("\"kind\": \"calls\""));
        Ok(())
    }
}
