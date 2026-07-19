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

/// Backend-neutral provider, SDK-component, and package-feature context for one project represented in an export.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodegraphSemanticContext {
    /// Canonical project root whose selection produced this context.
    pub project_root: String,
    /// Active SDK and component projection, when the toolchain is component-aware.
    pub sdk: Option<CodegraphSdkProjection>,
    /// Public package-feature closures participating in this project graph.
    pub packages: Vec<CodegraphPackageFeatureProjection>,
    /// Exact compiled-provider records known to the shared compiler plan.
    pub providers: Vec<CodegraphProviderProjection>,
}

/// Active SDK identity and expanded component selection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodegraphSdkProjection {
    /// Stable SDK identity.
    pub identity: String,
    /// Selected release-owned profile name.
    pub profile: String,
    /// Every component known to the SDK, including unavailable and disabled components.
    pub components: Vec<CodegraphSdkComponentProjection>,
}

/// Availability, enablement, dependencies, and selection provenance for one SDK component.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodegraphSdkComponentProjection {
    /// Stable component id.
    pub id: String,
    /// Component version from the active inventory.
    pub version: String,
    /// Whether this installation contains an integrity-checked artifact for the component.
    pub available: bool,
    /// Whether project/profile resolution enabled the component.
    pub enabled: bool,
    /// Whether the SDK requires the component in every profile.
    pub mandatory: bool,
    /// Direct component dependencies.
    pub dependencies: Vec<String>,
    /// Reason the component entered the expanded selection, when enabled.
    pub reason: Option<CodegraphComponentSelectionReason>,
}

/// Stable reason that one SDK component entered the expanded project selection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "source", rename_all = "snake_case")]
pub enum CodegraphComponentSelectionReason {
    /// Component is mandatory for this SDK release.
    Mandatory,
    /// Component belongs to the selected SDK profile.
    Profile(String),
    /// Project manifest selected the component explicitly.
    Explicit,
    /// Another selected component requires this component.
    Dependency(String),
}

/// Additive public feature closure for one concrete package root.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodegraphPackageFeatureProjection {
    /// Declared package name.
    pub package: String,
    /// Concrete package root used for path-dependency unification.
    pub project_root: String,
    /// Unified active public feature set.
    pub active_features: Vec<String>,
    /// Optional Incan dependencies activated by the feature closure.
    pub active_optional_dependencies: Vec<String>,
    /// Public feature requests sent to active dependency packages.
    pub dependency_features: Vec<CodegraphDependencyFeatureProjection>,
    /// SDK components required by the active package feature projection.
    pub required_sdk_components: Vec<String>,
    /// Stable activation provenance for each active feature.
    pub reasons: Vec<CodegraphFeatureReasonProjection>,
}

/// Public features requested from one active Incan dependency.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodegraphDependencyFeatureProjection {
    /// Dependency key from the requesting manifest.
    pub dependency: String,
    /// Unified requested feature set.
    pub features: Vec<String>,
}

/// Activation provenance for one active public package feature.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodegraphFeatureReasonProjection {
    /// Active package-owned feature.
    pub feature: String,
    /// Every reason that contributed the feature to the additive closure.
    pub reasons: Vec<CodegraphFeatureActivationReason>,
}

/// Stable reason that one public feature entered a package projection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "source", rename_all = "snake_case")]
pub enum CodegraphFeatureActivationReason {
    /// Conventional package default selected the feature.
    Default,
    /// Command or parent request selected the feature explicitly.
    Requested,
    /// `--all-features` selected the feature.
    AllFeatures,
    /// Another local feature includes this feature.
    IncludedBy(String),
    /// A parent package dependency edge requested this feature.
    DependencyRequest {
        /// Requesting package name.
        package: String,
        /// Dependency key on the requesting package.
        dependency: String,
    },
}

/// Exact provider identity, state, semantic use, implementation closure, artifact, and authority provenance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodegraphProviderProjection {
    /// Stable provider identity including version, digest, and feature projection.
    pub identity: String,
    /// Provider artifact availability.
    pub available: bool,
    /// Provider enablement after component and package-feature resolution.
    pub enabled: bool,
    /// Provider participation derived from reached canonical modules.
    pub participation: CodegraphProviderParticipation,
    /// Authority chain that introduced the provider.
    pub provenance: CodegraphProviderProvenance,
    /// Exact canonical modules claimed by the provider.
    pub namespace_claims: Vec<Vec<String>>,
    /// Canonical provider modules reached by this compilation graph.
    pub used_modules: Vec<Vec<String>>,
    /// Public feature projection used by this physical artifact.
    pub active_features: Vec<String>,
    /// Private implementation facets selected by semantic use.
    pub implementation_facets: Vec<String>,
    /// Backend requirements derived from selected facets.
    pub backend_requirements: Vec<String>,
    /// Relocatable or installed artifact-manifest location, when available.
    pub manifest_path: Option<String>,
}

/// Provider participation state with availability, enablement, and semantic use kept distinct.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodegraphProviderParticipation {
    /// Provider artifact is absent from the active installation or artifact store.
    Unavailable,
    /// Provider is available but disabled by the selected semantic graph.
    Disabled,
    /// Provider is enabled and available but no claimed module is reached.
    Enabled,
    /// At least one claimed module is reached by the compilation graph.
    Used,
}

/// Authority and source chain that introduced one provider record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CodegraphProviderProvenance {
    /// Ordinary Incan dependency selected from a project graph.
    ProjectDependency {
        /// Dependency key used under `pub::<key>`.
        dependency_key: String,
        /// Manifest that declared the dependency.
        manifest_path: String,
    },
    /// Provider authorized by the active SDK inventory.
    Sdk {
        /// Active SDK identity.
        sdk_identity: String,
        /// SDK component that supplies the provider.
        component_id: String,
        /// Inventory file that granted reserved namespace authority, when installed.
        inventory_path: Option<String>,
    },
    /// Compiler-owned symbolic provider without a compiled artifact.
    Compiler,
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
    /// Fact came from checked compiler artifacts.
    Checked,
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

/// Labeled secondary source location attached to a diagnostic fact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodegraphDiagnosticRelatedSpan {
    /// Secondary source span.
    pub span: CodegraphSourceSpan,
    /// Compiler-owned explanation for this relationship.
    pub label: String,
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
    /// Typed semantic contexts that determined provider and feature projection for represented projects.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub semantic_contexts: Vec<CodegraphSemanticContext>,
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
    /// Compiler subsystem that produced the diagnostic fact.
    #[serde(default = "unknown_diagnostic_origin")]
    pub origin: String,
    /// User-facing diagnostic message.
    pub message: String,
    /// Primary source span.
    pub primary_span: CodegraphSourceSpan,
    /// Additional notes.
    pub notes: Vec<String>,
    /// Suggested fixes or hints.
    pub hints: Vec<String>,
    /// Structured expected value or type when available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected: Option<String>,
    /// Structured actual value or type when available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actual: Option<String>,
    /// Secondary compiler-owned source locations.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub related_spans: Vec<CodegraphDiagnosticRelatedSpan>,
    /// Explain command for the diagnostic code.
    pub explain: String,
    /// Fact provenance.
    pub provenance: CodegraphProvenance,
    /// Diagnostic records always indicate degraded graph state.
    pub degraded: bool,
}

/// Supply the safe legacy value when a schema-v1 diagnostic has no origin field.
fn unknown_diagnostic_origin() -> String {
    "unknown".to_string()
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
        CODEGRAPH_SCHEMA_VERSION, CodegraphDiagnosticRecord, CodegraphFileRecord, CodegraphHeaderRecord,
        CodegraphLanguage, CodegraphMode, CodegraphProvenance, CodegraphRecord, CodegraphSourceSpan, to_jsonl,
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

        let jsonl = to_jsonl(&records)?;
        let lines = jsonl.lines().collect::<Vec<_>>();

        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("\"record\":\"header\""));
        assert!(lines[0].contains("\"schema_version\":1"));
        assert!(lines[1].contains("\"record\":\"file\""));
        Ok(())
    }

    #[test]
    fn diagnostic_records_without_origin_remain_readable() -> Result<(), Box<dyn std::error::Error>> {
        let record = CodegraphRecord::Diagnostic(CodegraphDiagnosticRecord {
            id: "diagnostic:0".to_string(),
            language: CodegraphLanguage::Incan,
            code: "INCAN-T0001".to_string(),
            severity: "error".to_string(),
            phase: "typecheck".to_string(),
            origin: "typechecker".to_string(),
            message: "type mismatch".to_string(),
            primary_span: CodegraphSourceSpan {
                file: "main.incn".to_string(),
                start: 0,
                end: 1,
                start_line: 1,
                start_column: 1,
                end_line: 1,
                end_column: 2,
            },
            notes: Vec::new(),
            hints: Vec::new(),
            expected: None,
            actual: None,
            related_spans: Vec::new(),
            explain: "incan explain INCAN-T0001".to_string(),
            provenance: CodegraphProvenance::Diagnostic,
            degraded: true,
        });
        let mut legacy = serde_json::to_value(record)?;
        legacy.as_object_mut().ok_or("expected record object")?.remove("origin");

        let parsed: CodegraphRecord = serde_json::from_value(legacy)?;
        let CodegraphRecord::Diagnostic(diagnostic) = parsed else {
            return Err("expected diagnostic record".into());
        };
        assert_eq!(diagnostic.origin, "unknown");
        Ok(())
    }
}
