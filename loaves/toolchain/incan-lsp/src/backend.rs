//! LSP (Language Server Protocol) backend implementation for Incan
//!
//! Call-site explicit generics (`callee[T](...)`, `recv.m[U](...)`) get type-oriented completions and hover (see
//! `call_site_type_args.rs`, RFC 054).

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;

use serde::Deserialize;
use serde_json::json;
use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer};

use crate::call_site_type_args;
use crate::diagnostics::{
    RelatedDeclarationSource, RelatedDeclarationSources, compile_error_to_diagnostic_with_phase,
    compile_error_to_diagnostic_with_phase_and_sources, position_to_offset, span_to_range,
};
use crate::semantic_tokens;
use incan_driver::session::CompilationSession;
use incan_frontend::api_metadata::{
    ApiClass, ApiConst, ApiDeclaration, ApiEnum, ApiFunction, ApiMethod, ApiModel, ApiNewtype, ApiPartial, ApiStatic,
    ApiTrait, ApiTypeAlias, CheckedApiMetadata, CheckedApiMetadataPackage, SourceAnchor,
    checked_api_declaration_is_public_namespace_member, checked_api_modules_for_public_namespace,
    checked_api_public_module_paths, collect_checked_api_metadata, validate_checked_api_docstrings,
};
use incan_frontend::ast::{
    CallArg, Condition, Declaration, DictEntry, EmbeddedOwnership, Expr, ListEntry, MatchBody, MethodDecl, Param,
    ParamKind, Program, RaceForBody, Span, Spanned, Statement, SurfaceExprPayload, Type, TypeParam,
};
use incan_frontend::contract_metadata::{
    CanonicalModelBundle, read_model_bundles_from_json, read_project_model_bundles,
};
use incan_frontend::diagnostics::{CompileError, DiagnosticPhase, phase_for_typecheck_span};
use incan_frontend::library_manifest::{
    EnumValueExport, EnumValueTypeExport, FieldExport, MemberVisibilityExport, ParamExport, ParamKindExport,
    ReceiverExport, TypeBoundExport, TypeParamExport, TypeRef,
};
use incan_frontend::loaf_index::LIBRARY_MANIFEST_EXTENSION;
use incan_frontend::loaf_index::{LoafIndex, LoafNamespace};
use incan_frontend::parsed_module::ParsedModule;
use incan_frontend::symbols::{ResolvedType, SymbolKind as FrontendSymbolKind, TypeInfo};
use incan_frontend::typechecker::{
    CAbiInteropArtifacts, CBindingType, COutputMode, CResourceAccess, TypeCheckInfo, c_binding_descriptor_identity,
};
use incan_frontend::{ast_walk, lexer, parser, typechecker};
use incan_lang::interop::{RustItemKind, RustModuleChildKind, RustTraitAssoc};
use incan_lang::lang::c_abi::{link_capability_as_str, scalar_type_as_str};
use incan_lang::lang::decorators;
use incan_lang::lang::keywords;
use incan_lang::lang::stdlib;
use incan_lang::lang::surface::collection_helpers::{self, BuiltinCollectionHelperId};
use incan_lang::lang::surface::constructors;
use incan_lang::lang::types::collections;
use incan_semantics_core::{CanonicalSymbolId, SymbolOrigin};

mod analysis;
mod analysis_cache;
mod api_markdown;
mod completions;
mod contract_model_command;
mod graph_navigation;
mod hover_context;
mod hovers_and_symbols;
mod language_server;
mod loaves;
mod metadata_previews;
mod rust_hover;
mod scoped_symbol_contexts;
mod scoped_symbols;
mod signature_help;

use api_markdown::*;
use completions::*;
use contract_model_command::*;
use hover_context::*;
use hovers_and_symbols::*;
use metadata_previews::*;
use scoped_symbol_contexts::*;
use scoped_symbols::*;
use signature_help::*;

const EMIT_CONTRACT_MODEL_COMMAND: &str = "incan.metadata.model.emit";

/// Document state stored by the LSP
#[derive(Debug, Clone)]
pub struct DocumentState {
    pub source: String,
    pub ast: Option<Program>,
    pub version: i32,
    /// Exporter-owned graph records for this live analysis; invalidated before dependent buffers change.
    graph_navigation: Option<graph_navigation::GraphSnapshot>,
    /// Canonical source identities produced by the same typecheck as this document state.
    ///
    /// Navigation consumers fail closed when a reference has no recorded identity; they never recover a target from
    /// the identifier spelling.
    checked_identities: CheckedIdentitySnapshot,
    /// Resolved const types from the typechecker (post “const-freezing”).
    ///
    /// This is used to make hover text reflect the actual type of a const binding, even if the user annotated
    /// `str`/`List[T]` and the compiler froze it to `FrozenStr`/`FrozenList[T]`.
    pub const_types: HashMap<String, String>,
    /// Resolved local value/static types with declaration spans for context-sensitive hover affordances.
    value_types: Vec<ValueTypeFact>,
    /// Local symbols that originate from `rust::...` imports with canonical Rust path provenance.
    rust_origin_symbols: Vec<RustOriginSymbol>,
    /// For `rusttype` newtypes: maps the Incan type name to the canonical Rust path of the underlying type (e.g.
    /// `"Name"` -> `"std::string::String"`).  Populated from the typechecker's resolved `NewtypeInfo.underlying`.
    rusttype_info: HashMap<String, String>,
    /// Checked public API metadata snippets that can be shown through hover after a successful typecheck.
    api_metadata_previews: Vec<ApiMetadataPreview>,
    /// Compiler-owned registry memberships used for hover and navigation without reinterpreting decorators.
    registry_previews: Vec<RegistryPreview>,
    /// Checked C binding declarations and raw-call links used for hover and navigation without reading generated Rust.
    c_binding_previews: Vec<CBindingPreview>,
    /// Imported DSL surfaces from loaded `pub::` library manifests, used for scoped symbol LSP affordances.
    library_imported_dsl_surfaces: parser::ImportedLibraryDslSurfaces,
    /// The Loaves the document's compilation reaches, whose manifests drive module and member completion.
    loaves: Arc<LoafIndex>,
    /// Active root-package features used to keep completion aligned with the compiler's declaration projection.
    active_features: BTreeSet<String>,
}

/// Source-aware canonical identity facts retained from one completed document analysis.
#[derive(Debug, Clone, Default)]
struct CheckedIdentitySnapshot {
    type_info: TypeCheckInfo,
    declaration_sources: RelatedDeclarationSources,
}

#[derive(Debug, Clone)]
struct RustOriginSymbol {
    local_name: String,
    span: Span,
    info: incan_frontend::symbols::RustItemInfo,
}

#[derive(Debug, Clone)]
struct ClassmethodContext {
    owner_type: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ApiMetadataPreview {
    span: Span,
    markdown: String,
}

/// One checked RFC 113 registry membership projected into LSP affordances.
#[derive(Debug, Clone, PartialEq, Eq)]
struct RegistryPreview {
    registration_span: Span,
    subject_span: Span,
    markdown: String,
}

/// One checked C declaration or source call projected into LSP affordances.
#[derive(Debug, Clone, PartialEq, Eq)]
struct CBindingPreview {
    span: Span,
    declaration_span: Span,
    markdown: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ValueTypeFact {
    name: String,
    span: Span,
    kind: ValueTypeKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ValueTypeKind {
    OrdinalMap,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LocalCallSignature {
    label: String,
    parameters: Vec<String>,
    active_parameter: Option<u32>,
}

/// Incan Language Server
pub struct IncanLanguageServer {
    client: Client,
    /// Artifact profile selected once when the server starts and applied to every Oven unit description.
    profile: Arc<str>,
    documents: Arc<RwLock<HashMap<Url, DocumentState>>>,
    /// Only the most recently requested analysis may publish, including after a close and reopen.
    analysis_tickets: RwLock<HashMap<Url, Arc<()>>>,
    /// Sessions hold compiled facts only; each analysis still asks Oven to prepare current dependency artifacts.
    analysis_sessions: Arc<std::sync::Mutex<analysis_cache::Sessions>>,
    /// Latest text, including edits awaiting the debounce interval or a checking pass.
    pending_sources: RwLock<HashMap<Url, String>>,
}

impl IncanLanguageServer {
    /// Initialize one client connection whose unit descriptions all use `profile`.
    pub fn new(client: Client, profile: Arc<str>) -> Self {
        Self {
            client,
            profile,
            documents: Arc::new(RwLock::new(HashMap::new())),
            analysis_tickets: RwLock::new(HashMap::new()),
            analysis_sessions: Arc::new(std::sync::Mutex::new(analysis_cache::Sessions::default())),
            pending_sources: RwLock::new(HashMap::new()),
        }
    }

    /// Find the symbol at a position in the AST
    fn find_symbol_at_position(&self, ast: &Program, source: &str, position: Position) -> Option<SymbolInfo> {
        let offset = position_to_offset(source, position)?;

        for decl in &ast.declarations {
            if let Some(info) = self.find_in_declaration(&decl.node, decl.span, offset, source) {
                return Some(info);
            }
        }

        None
    }

    /// Return hover-oriented symbol information when the cursor falls inside one top-level declaration span.
    fn find_in_declaration(&self, decl: &Declaration, span: Span, offset: usize, source: &str) -> Option<SymbolInfo> {
        match decl {
            Declaration::Const(konst) if span.start <= offset && offset < span.end => {
                return Some(SymbolInfo {
                    name: konst.name.clone(),
                    kind: "const".to_string(),
                    detail: if let Some(ty) = &konst.ty {
                        format!("const {}: {}", konst.name, format_type(&ty.node))
                    } else {
                        format!("const {}", konst.name)
                    },
                    span,
                });
            }
            Declaration::Static(static_decl) if span.start <= offset && offset < span.end => {
                return Some(SymbolInfo {
                    name: static_decl.name.clone(),
                    kind: "static".to_string(),
                    detail: format!("static {}: {}", static_decl.name, format_type(&static_decl.ty.node)),
                    span,
                });
            }
            Declaration::Function(func) if span.start <= offset && offset < span.end => {
                return Some(SymbolInfo {
                    name: func.name.clone(),
                    kind: "function".to_string(),
                    detail: format_function_signature(func, source),
                    span,
                });
            }
            Declaration::Partial(partial) if span.start <= offset && offset < span.end => {
                return Some(SymbolInfo {
                    name: partial.name.clone(),
                    kind: "partial".to_string(),
                    detail: format_partial_decl_signature(partial, source),
                    span,
                });
            }
            Declaration::Model(model) if span.start <= offset && offset < span.end => {
                if let Some(info) = find_property_symbol_info(&model.name, &model.properties, offset) {
                    return Some(info);
                }
                return Some(SymbolInfo {
                    name: model.name.clone(),
                    kind: "model".to_string(),
                    detail: format!("model {}", model.name),
                    span,
                });
            }
            Declaration::Class(class) if span.start <= offset && offset < span.end => {
                if let Some(info) = find_property_symbol_info(&class.name, &class.properties, offset) {
                    return Some(info);
                }
                return Some(SymbolInfo {
                    name: class.name.clone(),
                    kind: "class".to_string(),
                    detail: format!("class {}", class.name),
                    span,
                });
            }
            Declaration::Trait(tr) if span.start <= offset && offset < span.end => {
                if let Some(info) = find_property_symbol_info(&tr.name, &tr.properties, offset) {
                    return Some(info);
                }
                return Some(SymbolInfo {
                    name: tr.name.clone(),
                    kind: "trait".to_string(),
                    detail: format!("trait {}", tr.name),
                    span,
                });
            }
            Declaration::Enum(en) if span.start <= offset && offset < span.end => {
                return Some(SymbolInfo {
                    name: en.name.clone(),
                    kind: "enum".to_string(),
                    detail: enum_completion_detail(en),
                    span,
                });
            }
            Declaration::TypeAlias(alias) if span.start <= offset && offset < span.end => {
                return Some(SymbolInfo {
                    name: alias.name.clone(),
                    kind: "type".to_string(),
                    detail: format!("type {} = {}", alias.name, format_type(&alias.target.node)),
                    span,
                });
            }
            Declaration::Newtype(nt) if span.start <= offset && offset < span.end => {
                let kind = if nt.is_rusttype { "rusttype" } else { "newtype" };
                return Some(SymbolInfo {
                    name: nt.name.clone(),
                    kind: kind.to_string(),
                    detail: format!("{} {} = {}", kind, nt.name, format_type(&nt.underlying.node)),
                    span,
                });
            }
            _ => {}
        }

        None
    }
}

/// Symbol information for hover/goto
#[derive(Debug, Clone)]
pub struct SymbolInfo {
    pub name: String,
    pub kind: String,
    pub detail: String,
    pub span: Span,
}

/// Return the logical metadata module path used for LSP previews of one open document.
/// Build a document-level LSP diagnostic for errors that do not have a source span in the open file.
fn lsp_root_error_diagnostic(message: String) -> Diagnostic {
    Diagnostic {
        range: Range {
            start: Position::new(0, 0),
            end: Position::new(0, 0),
        },
        severity: Some(DiagnosticSeverity::ERROR),
        code: None,
        code_description: None,
        source: Some("incan".to_string()),
        message,
        related_information: None,
        tags: None,
        data: None,
    }
}

/// One compiler-proven identity occurrence under an LSP cursor.
#[derive(Debug, Clone, PartialEq, Eq)]
struct CheckedIdentityOccurrence {
    identity: CanonicalSymbolId,
    span: Span,
}
#[derive(Debug, Clone, PartialEq, Eq)]
struct ScopedSymbolLspContext {
    vocab_stack: Vec<String>,
    call_target: Option<String>,
}

#[derive(Debug, Clone)]
struct ScopedSymbolLspDescriptor<'a> {
    dependency_key: &'a str,
    descriptor: &'a incan_vocab::ScopedSymbolDescriptor,
    depth: usize,
}

#[derive(Debug, Clone)]
struct ScopedSymbolOccurrence<'a> {
    dependency_key: &'a str,
    descriptor: &'a incan_vocab::ScopedSymbolDescriptor,
    symbol_span: Span,
}

// ============================================================================
// Embedded-fragment ownership (RFC 081, #1022)
// ============================================================================

/// Ownership facts for a source offset that landed on an embedded fragment's DSL-owned syntax.
///
/// Only produced for the DSL-owned side of the boundary. An offset inside an expression hole is ordinary Incan and
/// deliberately produces nothing here, so every ordinary resolution path keeps working there unchanged.
#[derive(Debug, Clone)]
struct EmbeddedDslOwnership {
    /// The fixed submode grammar the claiming descriptor named.
    submode: incan_vocab::EmbeddedFragmentSubmode,
    /// Dependency the claiming descriptor came from, empty when the fragment was parsed without one.
    dependency_key: String,
    /// Stable descriptor key that claimed this fragment.
    descriptor_key: String,
    /// Source extent of the whole fragment, used to show the reader where DSL ownership starts and stops.
    span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ContractModelCommandFormat {
    Incan,
    Json,
}

impl ContractModelCommandFormat {
    /// Parse a client-provided format string, defaulting omitted values to Incan source output.
    fn parse(value: Option<&str>) -> std::result::Result<Self, String> {
        match value.unwrap_or("incan") {
            "incan" => Ok(Self::Incan),
            "json" => Ok(Self::Json),
            other => Err(format!("unsupported format `{other}`; expected `incan` or `json`")),
        }
    }

    /// Return the wire spelling used in command responses.
    fn as_str(self) -> &'static str {
        match self {
            Self::Incan => "incan",
            Self::Json => "json",
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct EmitContractModelCommandArgs {
    /// Source URI, bundle JSON path, project directory path, or `.incnlib` artifact path.
    uri: Option<String>,
    /// Alternative path field for clients that do not have a document URI.
    path: Option<String>,
    /// Logical model name or stable model id.
    model: String,
    /// Output format: `incan` or `json`.
    format: Option<String>,
}

#[cfg(test)]
mod ownership_signature_tests {
    use super::format_api_function_signature;
    use incan_frontend::api_metadata::{ApiDeclaration, collect_checked_api_metadata};
    use incan_frontend::typechecker::TypeChecker;

    /// Imported signatures render the renamed parameter slot and canonical stored fields, not stale source text.
    #[test]
    fn checked_source_signature_survives_parameter_rename() -> Result<(), Box<dyn std::error::Error>> {
        let source = "model Page:\n    title: str\npub def view(page: Borrowed[Page]) -> Borrowed[str] from page.title:\n    return page.title\n";
        let tokens = incan_frontend::lexer::lex(source).map_err(|errors| format!("{errors:?}"))?;
        let program = incan_frontend::parser::parse(&tokens).map_err(|errors| format!("{errors:?}"))?;
        let mut checker = TypeChecker::new();
        checker.set_current_package_identity(Some("provider".into()));
        checker
            .check_program(&program)
            .map_err(|errors| format!("{errors:?}"))?;
        let api = collect_checked_api_metadata(&program, &checker, vec!["provider".into()]);
        let mut function = api
            .declarations
            .into_iter()
            .find_map(|declaration| match declaration {
                ApiDeclaration::Function(function) => Some(function),
                _ => None,
            })
            .ok_or("missing checked public function")?;
        function.params.first_mut().ok_or("missing parameter")?.name = "document".into();
        let signature = format_api_function_signature(&function);
        assert!(signature.contains("document: Borrowed[Page]"), "{signature}");
        assert!(
            signature.ends_with("-> Borrowed[str] from document.title"),
            "{signature}"
        );
        Ok(())
    }
}
