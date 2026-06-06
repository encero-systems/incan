//! Storage-agnostic codegraph records for Incan tooling.
//!
//! This crate owns the JSONL wire shape for compiler-backed codegraph exports. It deliberately has no dependency on
//! compiler internals, graph databases, embeddings, MCP servers, or storage engines: the compiler extracts facts, and
//! downstream tools decide how to index or visualize them.

use serde::{Deserialize, Serialize};

/// Current codegraph JSONL schema version.
pub const CODEGRAPH_SCHEMA_VERSION: u32 = 1;

/// Package identity attached to a codegraph export when an `incan.toml` manifest is available.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodegraphPackage {
    /// Project name from `[project].name`.
    pub name: Option<String>,
    /// Project version from `[project].version`.
    pub version: Option<String>,
    /// Manifest root that bounded package-aware discovery.
    pub root_path: Option<String>,
}

/// Export mode recorded in the header.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodegraphMode {
    /// Strict export; diagnostics fail the command instead of producing a partial graph.
    Strict,
    /// Tolerant export; available syntax facts and diagnostics are emitted even when the source is broken.
    AllowErrors,
}

/// Source language represented by a graph fact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodegraphLanguage {
    /// Incan source or compiler-owned Incan metadata.
    Incan,
    /// Rust source, manifest, generated artifact, or interop metadata.
    Rust,
}

/// Provenance for one emitted graph fact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodegraphProvenance {
    /// Fact came directly from source text or filesystem shape.
    Source,
    /// Fact came from parsed syntax.
    Syntax,
    /// Fact came from checked compiler diagnostics.
    Diagnostic,
    /// Fact came from manifest/tooling context.
    Tooling,
}

/// Byte and line/column span for source-backed records.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodegraphSourceSpan {
    /// Source file path containing this span.
    pub file: String,
    /// Start byte offset, inclusive.
    pub start: usize,
    /// End byte offset, exclusive.
    pub end: usize,
    /// 1-based start line.
    pub start_line: usize,
    /// 1-based start column.
    pub start_column: usize,
    /// 1-based end line.
    pub end_line: usize,
    /// 1-based end column.
    pub end_column: usize,
}

/// Header record emitted first in every JSONL export.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodegraphHeaderRecord {
    /// Codegraph schema version.
    pub schema_version: u32,
    /// Producing Incan compiler version.
    pub compiler_version: String,
    /// Strict or tolerant export mode.
    pub mode: CodegraphMode,
    /// User-requested root path after CLI normalization.
    pub root_path: String,
    /// Languages represented by graph facts in this export.
    pub languages: Vec<CodegraphLanguage>,
    /// Project identity, when available.
    pub package: Option<CodegraphPackage>,
    /// Whether any emitted record is degraded or diagnostic-backed.
    pub degraded: bool,
}

/// Source file node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodegraphFileRecord {
    /// Stable id unique within the export.
    pub id: String,
    /// Source language for this graph fact.
    pub language: CodegraphLanguage,
    /// Source file path.
    pub path: String,
    /// File size in bytes.
    pub size_bytes: usize,
    /// Fact provenance.
    pub provenance: CodegraphProvenance,
    /// Whether this file record is part of a partial graph.
    pub degraded: bool,
}

/// Incan module node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodegraphModuleRecord {
    /// Stable id unique within the export.
    pub id: String,
    /// Source language for this graph fact.
    pub language: CodegraphLanguage,
    /// Parent file id.
    pub file_id: String,
    /// Module path segments.
    pub module_path: Vec<String>,
    /// Human-readable module name.
    pub name: String,
    /// Span covering the source file, when available.
    pub span: Option<CodegraphSourceSpan>,
    /// Fact provenance.
    pub provenance: CodegraphProvenance,
    /// Whether this module is partial due to diagnostics.
    pub degraded: bool,
}

/// Top-level declaration node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodegraphDeclarationRecord {
    /// Stable id unique within the export.
    pub id: String,
    /// Source language for this graph fact.
    pub language: CodegraphLanguage,
    /// Parent module id.
    pub module_id: String,
    /// Declaration kind such as `function`, `model`, or `type_alias`.
    pub kind: String,
    /// Source symbol name.
    pub name: String,
    /// Visibility spelling.
    pub visibility: String,
    /// Generic parameter names.
    pub type_params: Vec<String>,
    /// Human-readable declaration signature when cheaply available.
    pub signature: Option<String>,
    /// Source span for the declaration.
    pub span: Option<CodegraphSourceSpan>,
    /// Fact provenance.
    pub provenance: CodegraphProvenance,
    /// Whether this declaration is partial due to diagnostics.
    pub degraded: bool,
}

/// Import declaration node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodegraphImportRecord {
    /// Stable id unique within the export.
    pub id: String,
    /// Source language for this graph fact.
    pub language: CodegraphLanguage,
    /// Parent module id.
    pub module_id: String,
    /// Import kind such as `from`, `module`, `pub_from`, or `rust_from`.
    pub kind: String,
    /// Imported module/library/crate path.
    pub path: String,
    /// Imported item names for item imports.
    pub items: Vec<String>,
    /// Top-level import alias when present.
    pub alias: Option<String>,
    /// Visibility spelling.
    pub visibility: String,
    /// Source span for the import.
    pub span: Option<CodegraphSourceSpan>,
    /// Fact provenance.
    pub provenance: CodegraphProvenance,
    /// Whether this import is partial due to diagnostics.
    pub degraded: bool,
}

/// Public export fact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodegraphExportRecord {
    /// Stable id unique within the export.
    pub id: String,
    /// Source language for this graph fact.
    pub language: CodegraphLanguage,
    /// Module that owns the export.
    pub module_id: String,
    /// Public symbol name.
    pub name: String,
    /// Export kind such as `declaration` or `import`.
    pub kind: String,
    /// Source record id for the exported declaration/import.
    pub source_id: String,
    /// Source span for the export.
    pub span: Option<CodegraphSourceSpan>,
    /// Fact provenance.
    pub provenance: CodegraphProvenance,
    /// Whether this export is partial due to diagnostics.
    pub degraded: bool,
}

/// Source-level name reference inside declaration bodies.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodegraphReferenceRecord {
    /// Stable id unique within the export.
    pub id: String,
    /// Source language for this graph fact.
    pub language: CodegraphLanguage,
    /// Parent module id.
    pub module_id: String,
    /// Containing declaration id when the reference belongs to a declaration body.
    pub owner_id: Option<String>,
    /// Referenced source spelling.
    pub name: String,
    /// Reference shape such as `identifier`, `field`, or `self`.
    pub kind: String,
    /// Resolved target id when a semantic graph layer can prove it.
    pub target_id: Option<String>,
    /// Source span for the reference.
    pub span: Option<CodegraphSourceSpan>,
    /// Fact provenance.
    pub provenance: CodegraphProvenance,
    /// Whether this reference is partial due to diagnostics.
    pub degraded: bool,
}

/// Source-level call expression inside declaration bodies.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodegraphCallRecord {
    /// Stable id unique within the export.
    pub id: String,
    /// Source language for this graph fact.
    pub language: CodegraphLanguage,
    /// Parent module id.
    pub module_id: String,
    /// Containing declaration id when the call belongs to a declaration body.
    pub owner_id: Option<String>,
    /// Source-level callee spelling when cheaply available.
    pub callee: String,
    /// Call shape such as `function`, `method`, `constructor`, or `surface_symbol`.
    pub kind: String,
    /// Number of value arguments supplied at the call site.
    pub argument_count: usize,
    /// Number of explicit type arguments supplied at the call site.
    pub type_argument_count: usize,
    /// Resolved target id when a semantic graph layer can prove it.
    pub target_id: Option<String>,
    /// Source span for the call expression.
    pub span: Option<CodegraphSourceSpan>,
    /// Fact provenance.
    pub provenance: CodegraphProvenance,
    /// Whether this call is partial due to diagnostics.
    pub degraded: bool,
}

/// Containment relationship between graph records.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodegraphContainmentRecord {
    /// Stable id unique within the export.
    pub id: String,
    /// Source language for this graph fact.
    pub language: CodegraphLanguage,
    /// Parent record id.
    pub parent_id: String,
    /// Child record id.
    pub child_id: String,
    /// Relationship label.
    pub kind: String,
    /// Source span for the relationship.
    pub span: Option<CodegraphSourceSpan>,
    /// Fact provenance.
    pub provenance: CodegraphProvenance,
    /// Whether this edge is partial due to diagnostics.
    pub degraded: bool,
}

/// Diagnostic fact included in tolerant exports.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodegraphDiagnosticRecord {
    /// Stable id unique within the export.
    pub id: String,
    /// Source language for this graph fact.
    pub language: CodegraphLanguage,
    /// Public diagnostic code.
    pub code: String,
    /// Severity such as `error`, `warning`, or `hint`.
    pub severity: String,
    /// Compiler phase that produced the diagnostic.
    pub phase: String,
    /// User-facing diagnostic message.
    pub message: String,
    /// Primary source span.
    pub primary_span: CodegraphSourceSpan,
    /// Additional notes.
    pub notes: Vec<String>,
    /// Suggested fixes or hints.
    pub hints: Vec<String>,
    /// Explain command for the diagnostic code.
    pub explain: String,
    /// Fact provenance.
    pub provenance: CodegraphProvenance,
    /// Diagnostic records always indicate degraded graph state.
    pub degraded: bool,
}

/// One newline-delimited codegraph record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "record", rename_all = "snake_case")]
pub enum CodegraphRecord {
    /// Export header.
    Header(CodegraphHeaderRecord),
    /// Source file node.
    File(CodegraphFileRecord),
    /// Incan module node.
    Module(CodegraphModuleRecord),
    /// Top-level declaration node.
    Declaration(CodegraphDeclarationRecord),
    /// Import node.
    Import(CodegraphImportRecord),
    /// Public export fact.
    Export(CodegraphExportRecord),
    /// Source-level name reference.
    Reference(CodegraphReferenceRecord),
    /// Source-level call expression.
    Call(CodegraphCallRecord),
    /// Containment relationship.
    Containment(CodegraphContainmentRecord),
    /// Compiler diagnostic fact.
    Diagnostic(CodegraphDiagnosticRecord),
}

/// Serialize records as newline-delimited JSON, preserving caller-provided deterministic ordering.
pub fn to_jsonl(records: &[CodegraphRecord]) -> Result<String, serde_json::Error> {
    let mut lines = Vec::with_capacity(records.len() + 1);
    for record in records {
        lines.push(serde_json::to_string(record)?);
    }
    lines.push(String::new());
    Ok(lines.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::{
        CODEGRAPH_SCHEMA_VERSION, CodegraphFileRecord, CodegraphHeaderRecord, CodegraphLanguage, CodegraphMode,
        CodegraphProvenance, CodegraphRecord, to_jsonl,
    };

    #[test]
    fn jsonl_emits_header_then_facts() -> Result<(), Box<dyn std::error::Error>> {
        let records = vec![
            CodegraphRecord::Header(CodegraphHeaderRecord {
                schema_version: CODEGRAPH_SCHEMA_VERSION,
                compiler_version: "0.4.0-dev.5".to_string(),
                mode: CodegraphMode::Strict,
                root_path: "src/main.incn".to_string(),
                languages: vec![CodegraphLanguage::Incan],
                package: None,
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

        let jsonl = to_jsonl(&records)?;
        let lines = jsonl.lines().collect::<Vec<_>>();

        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("\"record\":\"header\""));
        assert!(lines[0].contains("\"schema_version\":1"));
        assert!(lines[1].contains("\"record\":\"file\""));
        Ok(())
    }
}
