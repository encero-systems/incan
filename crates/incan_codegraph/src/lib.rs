//! Storage-agnostic codegraph records for Incan tooling.
//!
//! This crate owns the JSONL wire shape for compiler-backed codegraph exports. It deliberately has no dependency on
//! compiler internals, graph databases, embeddings, MCP servers, or storage engines: the compiler extracts facts, and
//! downstream tools decide how to index or visualize them.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Current codegraph JSONL schema version.
pub const CODEGRAPH_SCHEMA_VERSION: u32 = 7;

/// Storage-neutral projection of one compiler-owned canonical symbol identity.
///
/// This mirrors the semantic identity fields deliberately instead of depending on compiler crates. Equality of this
/// value answers whether two checked codegraph facts name the same declaration; source spellings and record ids are
/// presentation/linkage projections only.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct CodegraphCanonicalSymbolId {
    /// Namespace in which the declaration is resolved.
    pub namespace: String,
    /// Compiler-owned declaration origin.
    pub origin: CodegraphSymbolOrigin,
    /// Spelling at the original declaration site.
    pub declaration_name: String,
    /// Semantic declaration category.
    pub declaration_kind: String,
    /// Scope discriminator for non-module bindings.
    pub scope_discriminant: Option<usize>,
    /// Original declaration byte span.
    pub declaration_span: CodegraphIdentitySpan,
}

/// Origin of a canonical codegraph symbol identity.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CodegraphSymbolOrigin {
    /// Declaration in a project source module.
    Module { path: Vec<String> },
    /// Declaration loaded from a compiled public package.
    Package { library: String, module_path: Vec<String> },
    /// Item owned by a Rust crate path.
    RustCrate { path: Vec<String> },
    /// Compiler-owned builtin registry entry.
    Builtin,
}

/// Source-independent byte-span component of a canonical identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct CodegraphIdentitySpan {
    pub start: usize,
    pub end: usize,
}

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

/// Canonical declaration identity related to a diagnostic fact.
///
/// Unlike [`CodegraphDiagnosticRelatedSpan`], this keeps the provider-owned declaration coordinates embedded in
/// the identity instead of projecting them into the primary file's line/column system.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodegraphDiagnosticRelatedDeclaration {
    /// Compiler-owned identity of the declaration involved in the diagnostic.
    pub identity: CodegraphCanonicalSymbolId,
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
    /// Compiler-owned declaration identity. `None` is an explicit unproven result.
    #[serde(default)]
    pub canonical_identity: Option<CodegraphCanonicalSymbolId>,
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
    /// Per-binding checked identity projections in source binding order.
    #[serde(default)]
    pub bindings: Vec<CodegraphImportBinding>,
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

/// One local binding introduced by an import declaration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodegraphImportBinding {
    /// Spelling introduced in the importing module.
    pub local_name: String,
    /// Identity of the original declaration, unchanged through aliases and re-exports.
    pub canonical_identity: Option<CodegraphCanonicalSymbolId>,
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
    /// Identity exported under `name`; aliases and re-exports keep the original declaration identity.
    #[serde(default)]
    pub canonical_identity: Option<CodegraphCanonicalSymbolId>,
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
    /// Compiler-owned identity of the resolved target, independent of source spelling and graph record availability.
    #[serde(default)]
    pub canonical_identity: Option<CodegraphCanonicalSymbolId>,
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
    /// Compiler-owned identity of the selected callable, independent of source spelling and graph record availability.
    #[serde(default)]
    pub canonical_identity: Option<CodegraphCanonicalSymbolId>,
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
    /// Canonical declarations related to this diagnostic, including declarations owned by another source file or
    /// compiled provider.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub related_declarations: Vec<CodegraphDiagnosticRelatedDeclaration>,
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

/// Compiler-checked typed registry entry.
///
/// Registry records describe declaration, compilation-unit, or package facts. They never claim that a process has
/// loaded the corresponding runtime entry; `provenance: checked` distinguishes this complete compiler projection from
/// future runtime observations.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodegraphRegistryRecord {
    /// Stable id unique within the export.
    pub id: String,
    /// Source language for this graph fact.
    pub language: CodegraphLanguage,
    /// Module that owns the source registration.
    pub module_id: String,
    /// Canonical registry identity.
    pub registry_identity: String,
    /// Whether this registry is public to package consumers.
    pub registry_public: bool,
    /// Complete compiler-checked structural key value.
    pub key: Value,
    /// Complete compiler-checked structural descriptor value.
    pub descriptor: Value,
    /// Subject category: function, method, compilation_unit, or package.
    pub subject_kind: String,
    /// Canonical source-owned subject identity.
    pub subject_identity: String,
    /// Source span of the `@describe` or `RegistryEntry` registration.
    pub registration_span: CodegraphSourceSpan,
    /// Source span of the declaration or explicit subject expression.
    pub subject_span: CodegraphSourceSpan,
    /// Fact provenance.
    pub provenance: CodegraphProvenance,
    /// Public facade imports that resolve to this source-owned registry binding or subject.
    ///
    /// These are checked projections, not independently registered runtime entries.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reexport_paths: Vec<CodegraphRegistryReexportProjection>,
    /// Registry records are emitted only for successful checked modules.
    pub degraded: bool,
}

/// Compiler-checked C binding declaration.
///
/// A binding record projects the source-level ABI contract admitted by the same typechecking pass as the surrounding
/// codegraph. It is not an artifact-resolution receipt, a generated Rust ABI, or evidence that a runtime library has
/// been loaded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodegraphCBindingRecord {
    /// Stable id unique within the export.
    pub id: String,
    /// Source language for this graph fact.
    pub language: CodegraphLanguage,
    /// Module that owns the binding declaration.
    pub module_id: String,
    /// Ordinary class declaration record for this binding.
    pub declaration_id: String,
    /// Binding class name visible to Incan source.
    pub name: String,
    /// Relocation-stable compiler identity for the complete checked binding descriptor.
    ///
    /// This is the portable join key for inspection, codegraph, and target-handoff tooling. It excludes source spans
    /// and source-file locations but changes for every ABI-affecting checked descriptor field, including the declared
    /// header spelling.
    #[serde(default)]
    pub binding_identity: String,
    /// Header spelling declared by the binding.
    pub header: String,
    /// Logical system-library capability selected by the binding.
    pub system_library: String,
    /// Source-level native-link capability kind: `system_library` or `framework`.
    ///
    /// Older graph exports did not distinguish the link kind, so deserialization preserves their explicit
    /// `system_library` interpretation.
    #[serde(default = "default_c_binding_link_capability")]
    pub link_capability: String,
    /// Opaque resource declarations and their release associations.
    pub resources: Vec<CodegraphCBindingResource>,
    /// Raw native symbol contracts in declaration order.
    pub symbols: Vec<CodegraphCBindingSymbol>,
    /// C enum carrier and constant contracts in declaration order.
    pub enums: Vec<CodegraphCBindingEnum>,
    /// Plain C structure contracts in declaration order.
    pub structs: Vec<CodegraphCBindingStruct>,
    /// Source span for the binding declaration.
    pub span: CodegraphSourceSpan,
    /// Fact provenance.
    pub provenance: CodegraphProvenance,
    /// Binding records are emitted only for successful checked modules.
    pub degraded: bool,
}

/// One nominal opaque resource in a checked C binding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodegraphCBindingResource {
    /// Binding-local resource name.
    pub name: String,
    /// Native opaque C type spelling.
    pub native: String,
    /// Binding-local symbol that releases one owned resource.
    pub release: String,
}

/// One raw native symbol in a checked C binding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodegraphCBindingSymbol {
    /// Binding-local source name.
    pub name: String,
    /// Native linker symbol spelling.
    pub native: String,
    /// Parameter contracts in source order.
    pub parameters: Vec<CodegraphCBindingParameter>,
    /// Return contract.
    pub return_type: CodegraphCBindingType,
    /// Compiler-checked pointer-to-length associations for bounded spans.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub buffers: Vec<CodegraphCBindingBuffer>,
    /// Output-slot state transitions declared for selected results.
    pub outcomes: Vec<CodegraphCBindingOutcome>,
}

/// One descriptor-owned checked pointer-to-length association for a bounded span.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodegraphCBindingBuffer {
    /// Pointer parameter that starts the caller-owned span.
    pub pointer_parameter: String,
    /// Length parameter that bounds the caller-owned span.
    pub length_parameter: String,
    /// Exact scalar element spelling retained by the compiler descriptor.
    pub element: String,
}

/// Preserve the interpretation of graph exports produced before framework linkage was represented.
fn default_c_binding_link_capability() -> String {
    "system_library".to_string()
}

/// One named raw C parameter contract.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodegraphCBindingParameter {
    /// Parameter name.
    pub name: String,
    /// Checked C type contract.
    #[serde(rename = "type")]
    pub ty: CodegraphCBindingType,
}

/// One outcome that changes compiler-managed output-slot state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodegraphCBindingOutcome {
    /// Binding-local enum and variant spelling.
    pub result: String,
    /// `c.Out[...]` parameters made readable on this outcome.
    pub initializes: Vec<String>,
    /// `c.InOut[...]` parameters updated on this outcome.
    pub updates: Vec<String>,
    /// `c.InOut[...]` parameters invalidated on this outcome.
    pub invalidates: Vec<String>,
}

/// A structural C type from the checked binding vocabulary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CodegraphCBindingType {
    /// Fixed-width or target-sized C scalar spelling.
    Scalar {
        /// Canonical Incan C vocabulary spelling such as `c.i32`.
        spelling: String,
    },
    /// C pointer contract.
    Pointer {
        /// Whether the pointee is mutable through the pointer.
        mutable: bool,
        /// Nested pointee contract.
        pointee: Box<CodegraphCBindingType>,
    },
    /// Plain by-value C structure named by a binding member.
    Struct {
        /// Binding-local structure name.
        name: String,
    },
    /// Nominal opaque resource passed with an ownership contract.
    Resource {
        /// `owned`, `borrowed`, or `borrowed_mut`.
        access: String,
        /// Binding-local resource declaration name.
        resource: String,
    },
    /// Compiler-managed C output storage.
    Output {
        /// `out` or `in_out`.
        mode: String,
        /// Native value contract stored in this output position.
        value: Box<CodegraphCBindingType>,
    },
    /// Nullable owned-resource result.
    Nullable {
        /// Nested resource contract.
        value: Box<CodegraphCBindingType>,
    },
    /// C `void` result.
    Void,
}

/// One target-verified C enum declaration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodegraphCBindingEnum {
    /// Binding-local enum name.
    pub name: String,
    /// Checked scalar carrier spelling.
    pub carrier: String,
    /// Native constant contracts in declaration order.
    pub variants: Vec<CodegraphCBindingEnumVariant>,
}

/// One target-verified native enum constant spelling.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodegraphCBindingEnumVariant {
    /// Binding-local variant name.
    pub name: String,
    /// Native constant spelling.
    pub native: String,
}

/// One checked plain C structure declaration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodegraphCBindingStruct {
    /// Binding-local structure name.
    pub name: String,
    /// Native C tag or typedef spelling.
    pub native: String,
    /// Fields in declared layout order.
    pub fields: Vec<CodegraphCBindingStructField>,
}

/// One checked plain C structure field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodegraphCBindingStructField {
    /// Source and native field name.
    pub name: String,
    /// Checked C type contract.
    #[serde(rename = "type")]
    pub ty: CodegraphCBindingType,
}

/// One direct raw C call admitted by an explicit `unsafe:` acknowledgement.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodegraphCBindingCallRecord {
    /// Stable id unique within the export.
    pub id: String,
    /// Source language for this graph fact.
    pub language: CodegraphLanguage,
    /// Module that owns the call.
    pub module_id: String,
    /// Generic source-level `call` record for this expression, when the export contains it.
    pub call_id: Option<String>,
    /// Checked binding declaration selected by the call.
    pub binding_id: String,
    /// Portable compiler identity of the checked binding declaration selected by the call.
    #[serde(default)]
    pub binding_identity: String,
    /// Owning Incan callable declaration for this direct native call, when it occurs in a named function.
    ///
    /// This lets tooling traverse a public caller to its private bridge and then the raw declaration without
    /// inferring the relationship from generated Rust or a naming convention.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_declaration_id: Option<String>,
    /// Visibility recorded by the typechecker for the owning callable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_visibility: Option<String>,
    /// Binding class name visible to Incan source.
    pub binding: String,
    /// Binding-local native symbol name.
    pub symbol: String,
    /// Raw calls are admitted only through an explicit `unsafe:` acknowledgement.
    pub unsafe_acknowledged: bool,
    /// Source span for the full call expression.
    pub span: CodegraphSourceSpan,
    /// Fact provenance.
    pub provenance: CodegraphProvenance,
    /// Raw-call records are emitted only for successful checked modules.
    pub degraded: bool,
}

/// One compiler-proven public facade to private checked-C bridge relation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodegraphCBindingFacadeRecord {
    /// Stable id unique within the export.
    pub id: String,
    /// Source language for this graph fact.
    pub language: CodegraphLanguage,
    /// Module that owns both the facade and its private bridge.
    pub module_id: String,
    /// Public callable declaration that directly calls the bridge.
    pub facade_declaration_id: String,
    /// Private callable declaration that owns the checked raw C calls.
    pub bridge_declaration_id: String,
    /// Generic source-level call record for the proven facade-to-bridge call, when present.
    pub call_id: Option<String>,
    /// Checked raw C call records owned by the bridge.
    pub raw_call_ids: Vec<String>,
    /// Source span for the facade-to-bridge call expression.
    pub span: CodegraphSourceSpan,
    /// Fact provenance.
    pub provenance: CodegraphProvenance,
    /// Facade records are emitted only for successful checked modules.
    pub degraded: bool,
}

/// One public facade path attached to a source-owned checked registry fact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodegraphRegistryReexportProjection {
    /// Fully-qualified import path, split into source module and local alias segments.
    pub path: Vec<String>,
    /// Public import source span that created this projection.
    pub span: CodegraphSourceSpan,
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
    /// Compiler-checked typed registry entry.
    Registry(CodegraphRegistryRecord),
    /// Compiler-checked C binding declaration.
    CBinding(CodegraphCBindingRecord),
    /// Direct C binding call admitted by explicit `unsafe:` source.
    CBindingCall(CodegraphCBindingCallRecord),
    /// Compiler-proven public facade to private checked-C bridge relation.
    CBindingFacade(CodegraphCBindingFacadeRecord),
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
        CODEGRAPH_SCHEMA_VERSION, CodegraphCanonicalSymbolId, CodegraphDiagnosticRecord,
        CodegraphDiagnosticRelatedDeclaration, CodegraphFileRecord, CodegraphHeaderRecord, CodegraphIdentitySpan,
        CodegraphLanguage, CodegraphMode, CodegraphProvenance, CodegraphRecord, CodegraphReferenceRecord,
        CodegraphSourceSpan, CodegraphSymbolOrigin, to_jsonl,
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
        assert!(lines[0].contains(&format!("\"schema_version\":{CODEGRAPH_SCHEMA_VERSION}")));
        assert!(lines[1].contains("\"record\":\"file\""));
        Ok(())
    }

    #[test]
    fn canonical_identity_is_structured_and_independent_of_reference_spelling() -> Result<(), Box<dyn std::error::Error>>
    {
        let identity = CodegraphCanonicalSymbolId {
            namespace: "ordinary_lexical".to_string(),
            origin: CodegraphSymbolOrigin::Module {
                path: vec!["provider".to_string()],
            },
            declaration_name: "helper".to_string(),
            declaration_kind: "function".to_string(),
            scope_discriminant: None,
            declaration_span: CodegraphIdentitySpan { start: 4, end: 31 },
        };
        let record = CodegraphRecord::Reference(CodegraphReferenceRecord {
            id: "reference:consumer:0".to_string(),
            language: CodegraphLanguage::Incan,
            module_id: "module:consumer".to_string(),
            owner_id: None,
            name: "renamed_helper".to_string(),
            kind: "identifier".to_string(),
            target_id: None,
            canonical_identity: Some(identity.clone()),
            span: None,
            provenance: CodegraphProvenance::Checked,
            degraded: false,
        });

        let encoded = serde_json::to_value(&record)?;
        assert_eq!(encoded["name"], "renamed_helper");
        assert_eq!(encoded["canonical_identity"]["declaration_name"], "helper");
        assert_eq!(encoded["canonical_identity"]["origin"]["kind"], "module");
        assert_eq!(serde_json::from_value::<CodegraphRecord>(encoded)?, record);
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
            related_declarations: vec![CodegraphDiagnosticRelatedDeclaration {
                identity: CodegraphCanonicalSymbolId {
                    namespace: "ordinary_lexical".to_string(),
                    origin: CodegraphSymbolOrigin::Package {
                        library: "provider".to_string(),
                        module_path: vec!["api".to_string()],
                    },
                    declaration_name: "expected_value".to_string(),
                    declaration_kind: "function".to_string(),
                    scope_discriminant: None,
                    declaration_span: CodegraphIdentitySpan { start: 7, end: 42 },
                },
                label: "previous declaration".to_string(),
            }],
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
        assert_eq!(diagnostic.related_declarations.len(), 1);
        assert_eq!(
            diagnostic.related_declarations[0].identity.declaration_name,
            "expected_value"
        );
        Ok(())
    }
}
