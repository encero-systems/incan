//! LSP (Language Server Protocol) backend implementation for Incan
//!
//! Call-site explicit generics (`callee[T](...)`, `recv.m[U](...)`) get type-oriented completions and hover
//! (see `call_site_type_args.rs`, RFC 054).

#[cfg(feature = "rust_inspect")]
use std::collections::BTreeSet;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
#[cfg(feature = "rust_inspect")]
use std::sync::{Mutex, OnceLock};
use tokio::sync::RwLock;

use serde::Deserialize;
use serde_json::json;
use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer};

use crate::cli::commands::common::CompilationSession;
#[cfg(feature = "rust_inspect")]
use crate::cli::commands::common::{
    build_source_map, collect_inline_rust_imports, collect_project_requirements, collect_rust_inspect_query_paths,
    ensure_rust_inspect_workspace, format_dependency_error, merge_project_requirement_dependencies,
    prewarm_rust_inspect_workspace,
};
use crate::cli::prelude::ParsedModule;
#[cfg(feature = "rust_inspect")]
use crate::dependency_resolver::{ResolvedDependencies, resolve_dependencies};
use crate::frontend::api_metadata::{
    ApiClass, ApiConst, ApiDeclaration, ApiEnum, ApiFunction, ApiMethod, ApiModel, ApiNewtype, ApiStatic, ApiTrait,
    ApiTypeAlias, CheckedApiMetadata, SourceAnchor, collect_checked_api_metadata, validate_checked_api_docstrings,
};
use crate::frontend::ast::{Declaration, MethodDecl, Program, Span, Type, TypeParam};
use crate::frontend::contract_metadata::{
    CanonicalModelBundle, materialize_contract_models, read_model_bundles_from_json, read_project_model_bundles,
};
use crate::frontend::diagnostics::CompileError;
use crate::frontend::library_manifest_index::LibraryManifestIndex;
use crate::frontend::module::{SourceModuleImportResolution, resolve_program_source_imports};
use crate::frontend::{lexer, parser, typechecker};
use crate::library_manifest::{
    EnumValueExport, EnumValueTypeExport, FieldExport, ParamExport, ParamKindExport, ReceiverExport, TypeBoundExport,
    TypeParamExport, TypeRef,
};
#[cfg(feature = "rust_inspect")]
use crate::lockfile::CargoFeatureSelection;
use crate::lsp::call_site_type_args;
use crate::lsp::diagnostics::{compile_error_to_diagnostic, position_to_offset, span_to_range};
use crate::manifest::ProjectManifest;
use incan_core::interop::{RustItemKind, RustModuleChildKind, RustTraitAssoc};
use incan_core::lang::decorators;
use incan_core::lang::keywords;
use incan_core::lang::stdlib;
use incan_core::lang::surface::collection_helpers::{self, BuiltinCollectionHelperId};
use incan_core::lang::surface::constructors;
use incan_core::lang::types::collections;

const EMIT_CONTRACT_MODEL_COMMAND: &str = "incan.metadata.model.emit";

/// Document state stored by the LSP
#[derive(Debug, Clone)]
pub struct DocumentState {
    pub source: String,
    pub ast: Option<Program>,
    pub version: i32,
    /// Resolved const types from the typechecker (post “const-freezing”).
    ///
    /// This is used to make hover text reflect the actual type of a const binding, even if the user annotated
    /// `str`/`List[T]` and the compiler froze it to `FrozenStr`/`FrozenList[T]`.
    pub const_types: HashMap<String, String>,
    /// Local symbols that originate from `rust::...` imports with canonical Rust path provenance.
    rust_origin_symbols: Vec<RustOriginSymbol>,
    /// For `rusttype` newtypes: maps the Incan type name to the canonical Rust path of the underlying type (e.g.
    /// `"Name"` -> `"std::string::String"`).  Populated from the typechecker's resolved `NewtypeInfo.underlying`.
    rusttype_info: HashMap<String, String>,
    /// Checked public API metadata snippets that can be shown through hover after a successful typecheck.
    api_metadata_previews: Vec<ApiMetadataPreview>,
}

#[derive(Debug, Clone)]
struct RustOriginSymbol {
    local_name: String,
    span: Span,
    info: crate::frontend::symbols::RustItemInfo,
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

/// Incan Language Server
pub struct IncanLanguageServer {
    client: Client,
    documents: Arc<RwLock<HashMap<Url, DocumentState>>>,
}

impl IncanLanguageServer {
    pub fn new(client: Client) -> Self {
        Self {
            client,
            documents: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Analyze a document and publish diagnostics
    async fn analyze_document(&self, uri: &Url, source: &str, version: i32) {
        let mut diagnostics = Vec::new();

        // Step 1: Discover shared compilation context.
        let module_path = uri.to_file_path().ok();
        let compilation_session = if let Some(path) = &module_path {
            match CompilationSession::discover(path) {
                Ok(session) => Some(session),
                Err(error) => {
                    diagnostics.push(lsp_root_error_diagnostic(error.to_string()));
                    self.client
                        .publish_diagnostics(uri.clone(), diagnostics, Some(version))
                        .await;
                    return;
                }
            }
        } else {
            None
        };
        let declared_crates = compilation_session
            .as_ref()
            .map(CompilationSession::declared_crate_names)
            .unwrap_or_default();
        let library_manifest_index = compilation_session
            .as_ref()
            .map(|session| session.library_manifest_index.clone())
            .unwrap_or_default();
        #[cfg(feature = "rust_inspect")]
        let project_manifest = compilation_session
            .as_ref()
            .and_then(|session| session.manifest.clone());
        #[cfg(feature = "rust_inspect")]
        let mut rust_inspect_context: Option<(PathBuf, Vec<String>)> = None;

        // Step 2: Parse
        //
        // Pass the on-disk file path as `module_path` so context-sensitive syntax matches the CLI.
        // In particular, `pub from ... import ...` is only accepted when this path resolves under `src/` (RFC 031 /
        // `incan_syntax` parser). If `uri.to_file_path()` fails, `module_path` is omitted and those rules are
        // skipped during parsing (prefer fixing the client URI scheme / workspace roots).
        let ast = match if let (Some(session), Some(path)) = (compilation_session.as_ref(), module_path.as_ref()) {
            session.parse_source(path, source, false)
        } else {
            lexer::lex(source).and_then(|tokens| parser::parse_with_context_and_surfaces(&tokens, None, None, None))
        } {
            Ok(ast) => {
                // Forward non-fatal parser warnings (e.g. RFC 005 dot-notation nudges) to the LSP.
                for warn in &ast.warnings {
                    diagnostics.push(compile_error_to_diagnostic(warn, source, uri));
                }
                ast
            }
            Err(errors) => {
                for error in &errors {
                    diagnostics.push(compile_error_to_diagnostic(error, source, uri));
                }
                self.client
                    .publish_diagnostics(uri.clone(), diagnostics, Some(version))
                    .await;
                return;
            }
        };
        let mut typecheck_ast = ast.clone();
        if let Some(session) = &compilation_session
            && let Err(error) = materialize_contract_models(&mut typecheck_ast, &session.contract_model_bundles)
        {
            diagnostics.push(lsp_root_error_diagnostic(format!(
                "Invalid checked contract metadata: {error}"
            )));
            self.client
                .publish_diagnostics(uri.clone(), diagnostics, Some(version))
                .await;
            return;
        }

        let (deps, mut dep_summary_diags) = self
            .collect_dependency_modules(uri, &ast, source, compilation_session.as_ref())
            .await;
        let mut typecheck_deps = deps.clone();
        if let Some(session) = &compilation_session {
            for dep in &mut typecheck_deps {
                if let Err(error) = materialize_contract_models(&mut dep.ast, &session.contract_model_bundles) {
                    diagnostics.push(lsp_root_error_diagnostic(format!(
                        "Invalid checked contract metadata: {error}"
                    )));
                    self.client
                        .publish_diagnostics(uri.clone(), diagnostics, Some(version))
                        .await;
                    return;
                }
            }
        }
        #[cfg(feature = "rust_inspect")]
        if let (Some(manifest), Some(path)) = (project_manifest.as_ref(), module_path.as_ref()) {
            let mut metadata_modules = Vec::with_capacity(deps.len() + 1);
            metadata_modules.push(parsed_module_for_lsp_document(path, source, &ast));
            metadata_modules.extend(deps.iter().cloned());
            rust_inspect_context =
                match prepare_lsp_rust_inspect_workspace(manifest, &metadata_modules, &library_manifest_index) {
                    Ok(ctx) => Some(ctx),
                    Err(err) => {
                        tracing::warn!("failed to prepare rust-inspect workspace for lsp: {err}");
                        None
                    }
                };
        }

        // Step 3: Type check (with multi-file import resolution)
        let mut checker = typechecker::TypeChecker::new();
        checker.set_declared_crate_names(declared_crates);
        checker.set_library_manifest_index(library_manifest_index.clone());
        #[cfg(feature = "rust_inspect")]
        if let Some((dir, metadata_query_paths)) = rust_inspect_context {
            spawn_rust_inspect_prewarm(dir.clone(), metadata_query_paths);
            checker.set_rust_inspect_manifest_dir(dir);
        }

        let dep_refs: Vec<(&str, &Program)> = typecheck_deps
            .iter()
            .map(|module| (module.name.as_str(), &module.ast))
            .collect();

        let check_result = checker.check_with_imports(&typecheck_ast, &dep_refs);
        let api_metadata_previews = if check_result.is_ok() {
            let metadata =
                collect_checked_api_metadata(&ast, &checker, lsp_metadata_module_path(module_path.as_deref()));
            for diagnostic in validate_checked_api_docstrings(std::slice::from_ref(&metadata)) {
                diagnostics.push(compile_error_to_diagnostic(&diagnostic.error, source, uri));
            }
            api_metadata_previews(&ast, &metadata)
        } else {
            Vec::new()
        };
        let rust_origin_symbols = collect_rust_origin_symbols(&checker);

        if let Err(errors) = check_result {
            for error in &errors {
                diagnostics.push(compile_error_to_diagnostic_with_rust_context(
                    error,
                    source,
                    uri,
                    &rust_origin_symbols,
                ));
            }
        }
        // Always include non-fatal diagnostics (warnings/lints) in LSP output.
        for warn in checker.warnings() {
            diagnostics.push(compile_error_to_diagnostic_with_rust_context(
                warn,
                source,
                uri,
                &rust_origin_symbols,
            ));
        }
        diagnostics.append(&mut dep_summary_diags);

        // Collect resolved const types for hover display (post-const-freezing).
        let mut const_types: HashMap<String, String> = HashMap::new();
        let mut rusttype_info: HashMap<String, String> = HashMap::new();
        for decl in &ast.declarations {
            if let Declaration::Const(konst) = &decl.node
                && let Some(id) = checker.symbols.lookup(&konst.name)
                && let Some(sym) = checker.symbols.get(id)
                && let crate::frontend::symbols::SymbolKind::Variable(var) = &sym.kind
            {
                const_types.insert(konst.name.clone(), var.ty.to_string());
            }
            if let Declaration::Static(static_decl) = &decl.node
                && let Some(id) = checker.symbols.lookup(&static_decl.name)
                && let Some(sym) = checker.symbols.get(id)
                && let crate::frontend::symbols::SymbolKind::Static(info) = &sym.kind
            {
                const_types.insert(static_decl.name.clone(), info.ty.to_string());
            }
            if let Declaration::Newtype(nt) = &decl.node
                && nt.is_rusttype
                && let Some(id) = checker.symbols.lookup(&nt.name)
                && let Some(sym) = checker.symbols.get(id)
                && let crate::frontend::symbols::SymbolKind::Type(crate::frontend::symbols::TypeInfo::Newtype(info)) =
                    &sym.kind
                && let crate::frontend::symbols::ResolvedType::RustPath(path) = &info.underlying
            {
                rusttype_info.insert(nt.name.clone(), path.clone());
            }
        }

        // Store AST for hover/goto
        {
            let mut docs = self.documents.write().await;
            docs.insert(
                uri.clone(),
                DocumentState {
                    source: source.to_string(),
                    ast: Some(ast),
                    version,
                    const_types,
                    rust_origin_symbols,
                    rusttype_info,
                    api_metadata_previews,
                },
            );
        }

        // Publish diagnostics (even if empty, to clear old ones)
        self.client
            .publish_diagnostics(uri.clone(), diagnostics, Some(version))
            .await;
    }

    /// Collect and parse dependency modules referenced by imports in `ast`.
    ///
    /// - Uses the on-disk file system for dependency sources
    /// - If a dependency is currently open in the editor, uses its in-memory contents
    async fn collect_dependency_modules(
        &self,
        uri: &Url,
        ast: &Program,
        entry_source: &str,
        compilation_session: Option<&CompilationSession>,
    ) -> (Vec<ParsedModule>, Vec<Diagnostic>) {
        let Ok(entry_path) = uri.to_file_path() else {
            return (Vec::new(), Vec::new());
        };
        let entry_base = entry_path.parent().unwrap_or(Path::new(".")).to_path_buf();

        let docs = self.documents.read().await;

        let mut result: Vec<ParsedModule> = Vec::new();
        let mut entry_diags: Vec<Diagnostic> = Vec::new();
        let mut seen: HashSet<PathBuf> = HashSet::new();
        let mut stack = Vec::new();
        let source_root = compilation_session.map(|session| session.source_root.as_path());

        // Seed stack with direct imports from the entry AST
        for resolved in resolve_program_source_imports(ast, &entry_base, source_root) {
            if let SourceModuleImportResolution::Local(module_ref) = resolved.resolution {
                stack.push((module_ref, resolved.span));
            }
        }

        while let Some((module_ref, import_span)) = stack.pop() {
            let canonical = module_ref
                .file_path
                .canonicalize()
                .unwrap_or_else(|_| module_ref.file_path.clone());
            if !seen.insert(canonical.clone()) {
                continue;
            }

            // Prefer in-memory source if this file is open.
            let dep_uri = Url::from_file_path(&canonical).ok();
            let dep_doc = dep_uri.as_ref().and_then(|u| docs.get(u));
            let dep_source = dep_doc
                .map(|d| d.source.clone())
                .or_else(|| fs::read_to_string(&canonical).ok());

            let Some(dep_source) = dep_source else {
                // If we can't read it, we can't typecheck it; skip.
                continue;
            };

            let dep_ast = match if let Some(session) = compilation_session {
                session.parse_source(&canonical, &dep_source, false)
            } else {
                let dep_path_display = canonical.to_string_lossy();
                lexer::lex(&dep_source).and_then(|tokens| {
                    parser::parse_with_context_and_surfaces(&tokens, Some(dep_path_display.as_ref()), None, None)
                })
            } {
                Ok(a) => a,
                Err(errors) => {
                    // Guardrail: surface dependency parse errors.
                    if let Some(u) = dep_uri.clone() {
                        let mut diags = Vec::new();
                        for e in &errors {
                            diags.push(compile_error_to_diagnostic(e, &dep_source, &u));
                        }
                        let ver = dep_doc.map(|d| d.version);
                        self.client.publish_diagnostics(u.clone(), diags, ver).await;
                    }

                    let range = span_to_range(entry_source, import_span.start, import_span.end);
                    entry_diags.push(Diagnostic {
                        range,
                        severity: Some(DiagnosticSeverity::ERROR),
                        code: None,
                        code_description: None,
                        source: Some("incan".to_string()),
                        message: format!(
                            "Failed to analyze dependency '{}'; open that file for details",
                            canonical.display()
                        ),
                        related_information: None,
                        tags: None,
                        data: None,
                    });
                    continue;
                }
            };

            // Dependency parsed successfully: clear old dependency diagnostics if any.
            if let Some(u) = dep_uri.clone() {
                let ver = dep_doc.map(|d| d.version);
                self.client.publish_diagnostics(u.clone(), vec![], ver).await;
            }

            // Queue nested dependencies
            let current_base = canonical.parent().unwrap_or(&entry_base);
            for resolved in resolve_program_source_imports(&dep_ast, current_base, source_root) {
                if let SourceModuleImportResolution::Local(nested_ref) = resolved.resolution {
                    stack.push((nested_ref, Span::default()));
                }
            }

            result.push(ParsedModule {
                name: module_ref.module_name,
                path_segments: module_ref.path_segments,
                file_path: canonical,
                source: dep_source,
                ast: dep_ast,
            });
        }

        (result, entry_diags)
    }

    /// Find the symbol at a position in the AST
    fn find_symbol_at_position(&self, ast: &Program, source: &str, position: Position) -> Option<SymbolInfo> {
        let offset = position_to_offset(source, position)?;

        for decl in &ast.declarations {
            if let Some(info) = self.find_in_declaration(&decl.node, decl.span, offset) {
                return Some(info);
            }
        }

        None
    }

    /// Return hover-oriented symbol information when the cursor falls inside one top-level declaration span.
    fn find_in_declaration(&self, decl: &Declaration, span: Span, offset: usize) -> Option<SymbolInfo> {
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
                    detail: format_function_signature(func),
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

    /// Find the definition location of a symbol
    fn find_definition(&self, ast: &Program, name: &str) -> Option<Span> {
        for decl in &ast.declarations {
            match &decl.node {
                Declaration::Const(konst) if konst.name == name => {
                    return Some(decl.span);
                }
                Declaration::Static(static_decl) if static_decl.name == name => {
                    return Some(decl.span);
                }
                Declaration::Function(func) if func.name == name => {
                    return Some(decl.span);
                }
                Declaration::Model(model) if model.name == name => {
                    return Some(decl.span);
                }
                Declaration::Class(class) if class.name == name => {
                    return Some(decl.span);
                }
                Declaration::Trait(tr) if tr.name == name => {
                    return Some(decl.span);
                }
                Declaration::Enum(en) if en.name == name => {
                    return Some(decl.span);
                }
                Declaration::TypeAlias(alias) if alias.name == name => {
                    return Some(decl.span);
                }
                Declaration::Newtype(nt) if nt.name == name => {
                    return Some(decl.span);
                }
                _ => {}
            }
        }
        None
    }
}

#[cfg(feature = "rust_inspect")]
#[derive(Default)]
struct PrewarmQueueEntry {
    /// Whether a worker task is currently draining this workspace queue.
    in_flight: bool,
    /// Canonical query paths accumulated while the worker is busy.
    pending: BTreeSet<String>,
}

#[cfg(feature = "rust_inspect")]
fn prewarm_queue() -> &'static Mutex<HashMap<PathBuf, PrewarmQueueEntry>> {
    static PREWARM_QUEUE: OnceLock<Mutex<HashMap<PathBuf, PrewarmQueueEntry>>> = OnceLock::new();
    PREWARM_QUEUE.get_or_init(|| Mutex::new(HashMap::new()))
}

#[cfg(feature = "rust_inspect")]
fn enqueue_prewarm_paths(
    queue: &mut HashMap<PathBuf, PrewarmQueueEntry>,
    manifest_dir: &Path,
    query_paths: impl IntoIterator<Item = String>,
) -> bool {
    let entry = queue.entry(manifest_dir.to_path_buf()).or_default();
    for path in query_paths {
        if !path.is_empty() {
            entry.pending.insert(path);
        }
    }
    if entry.in_flight {
        return false;
    }
    entry.in_flight = true;
    true
}

#[cfg(feature = "rust_inspect")]
fn take_next_prewarm_batch(
    queue: &mut HashMap<PathBuf, PrewarmQueueEntry>,
    manifest_dir: &Path,
) -> Option<Vec<String>> {
    let entry = queue.get_mut(manifest_dir)?;
    if entry.pending.is_empty() {
        entry.in_flight = false;
        queue.remove(manifest_dir);
        return None;
    }
    Some(std::mem::take(&mut entry.pending).into_iter().collect())
}

#[cfg(feature = "rust_inspect")]
/// Queue prewarm work for one workspace.
///
/// Contract:
/// - at most one worker runs per manifest directory
/// - requests arriving during a run are coalesced into `pending`
/// - the worker loops until `pending` is empty under lock, then exits
fn spawn_rust_inspect_prewarm(manifest_dir: PathBuf, query_paths: Vec<String>) {
    if query_paths.is_empty() {
        return;
    }
    let mut queue = match prewarm_queue().lock() {
        Ok(guard) => guard,
        Err(err) => {
            tracing::warn!("rust-inspect prewarm queue lock poisoned; recovering");
            err.into_inner()
        }
    };
    if !enqueue_prewarm_paths(&mut queue, &manifest_dir, query_paths) {
        tracing::debug!(
            "coalescing rust-inspect prewarm request while prior run is active (workspace={})",
            manifest_dir.display()
        );
        return;
    }
    tokio::spawn(async move {
        run_rust_inspect_prewarm_queue(manifest_dir).await;
    });
}

#[cfg(feature = "rust_inspect")]
async fn run_rust_inspect_prewarm_queue(manifest_dir: PathBuf) {
    loop {
        let batch: Vec<String> = {
            let mut queue = match prewarm_queue().lock() {
                Ok(guard) => guard,
                Err(err) => {
                    tracing::warn!("rust-inspect prewarm queue lock poisoned; recovering");
                    err.into_inner()
                }
            };
            let Some(batch) = take_next_prewarm_batch(&mut queue, &manifest_dir) else {
                return;
            };
            batch
        };

        match tokio::task::spawn_blocking({
            let manifest_dir = manifest_dir.clone();
            move || prewarm_rust_inspect_workspace(&manifest_dir, &batch)
        })
        .await
        {
            Ok(Ok(())) => {}
            Ok(Err(err)) => {
                tracing::warn!("rust-inspect prewarm failed in lsp: {err}");
            }
            Err(err) => {
                tracing::warn!("rust-inspect prewarm join error in lsp: {err}");
            }
        }
    }
}

#[cfg(feature = "rust_inspect")]
fn parsed_module_for_lsp_document(path: &Path, source: &str, ast: &Program) -> ParsedModule {
    let module_name = path
        .file_stem()
        .and_then(|name| name.to_str())
        .unwrap_or("main")
        .to_string();
    ParsedModule {
        name: module_name.clone(),
        path_segments: vec![module_name],
        file_path: path.to_path_buf(),
        source: source.to_string(),
        ast: ast.clone(),
    }
}

#[cfg(feature = "rust_inspect")]
fn resolved_rust_inspect_dependencies(
    manifest: &ProjectManifest,
    modules: &[ParsedModule],
    library_manifest_index: &LibraryManifestIndex,
) -> std::result::Result<ResolvedDependencies, String> {
    let project_requirements =
        collect_project_requirements(modules, library_manifest_index).map_err(|err| err.to_string())?;
    let mut inline_imports = Vec::new();
    for module in modules {
        inline_imports.extend(collect_inline_rust_imports(module, false));
    }

    let cargo_features = CargoFeatureSelection::default();
    let mut resolved =
        resolve_dependencies(Some(manifest), &inline_imports, true, &cargo_features).map_err(|errors| {
            let sources = build_source_map(modules);
            let mut msg = String::new();
            for err in errors {
                msg.push_str(&format_dependency_error(&err, &sources));
            }
            msg.trim_end().to_string()
        })?;
    merge_project_requirement_dependencies(&mut resolved, &project_requirements).map_err(|err| err.to_string())?;
    Ok(resolved)
}

#[cfg(feature = "rust_inspect")]
/// Build the rust-inspect workspace for LSP analysis after collecting the document's effective Rust dependencies.
///
/// The shared CLI helper owns workspace generation; this wrapper only translates the LSP document set into the
/// resolved dependency inputs that helper expects.
fn prepare_lsp_rust_inspect_workspace(
    manifest: &ProjectManifest,
    modules: &[ParsedModule],
    library_manifest_index: &LibraryManifestIndex,
) -> std::result::Result<(PathBuf, Vec<String>), String> {
    let project_name = manifest
        .project
        .as_ref()
        .and_then(|project| project.name.clone())
        .or_else(|| {
            manifest
                .project_root()
                .file_name()
                .and_then(|name| name.to_str())
                .map(str::to_owned)
        })
        .unwrap_or_else(|| "incan_lsp".to_string());

    let resolved = resolved_rust_inspect_dependencies(manifest, modules, library_manifest_index)?;
    let project_requirements =
        collect_project_requirements(modules, library_manifest_index).map_err(|err| err.to_string())?;
    let rust_inspect_manifest_dir = ensure_rust_inspect_workspace(
        manifest.project_root(),
        project_name.as_str(),
        manifest.build.as_ref().and_then(|build| build.rust_edition.clone()),
        &resolved,
        &project_requirements,
        None,
    )
    .map_err(|err| err.to_string())?;
    let query_paths = collect_rust_inspect_query_paths(modules);
    Ok((rust_inspect_manifest_dir, query_paths))
}

#[cfg(all(test, feature = "rust_inspect"))]
mod tests {
    use std::collections::HashMap;
    use std::path::PathBuf;

    use super::{
        PrewarmQueueEntry, enqueue_prewarm_paths, prepare_lsp_rust_inspect_workspace, take_next_prewarm_batch,
    };
    use crate::cli::prelude::ParsedModule;
    use crate::frontend::library_manifest_index::LibraryManifestIndex;
    use crate::frontend::{lexer, parser};
    use crate::manifest::ProjectManifest;

    #[test]
    fn prewarm_queue_coalesces_followup_requests_for_same_workspace() {
        let mut queue = HashMap::<PathBuf, PrewarmQueueEntry>::new();
        let root = PathBuf::from("/tmp/project");
        assert!(enqueue_prewarm_paths(
            &mut queue,
            &root,
            vec!["a::f".to_string(), "b::g".to_string()]
        ));
        assert!(!enqueue_prewarm_paths(
            &mut queue,
            &root,
            vec!["b::g".to_string(), "c::h".to_string()]
        ));

        let first = take_next_prewarm_batch(&mut queue, &root);
        assert_eq!(
            first,
            Some(vec!["a::f".to_string(), "b::g".to_string(), "c::h".to_string()])
        );
        assert!(take_next_prewarm_batch(&mut queue, &root).is_none());
        assert!(!queue.contains_key(&root));
    }

    #[test]
    fn prewarm_queue_keeps_new_paths_arriving_while_worker_active() {
        let mut queue = HashMap::<PathBuf, PrewarmQueueEntry>::new();
        let root = PathBuf::from("/tmp/project2");
        assert!(enqueue_prewarm_paths(&mut queue, &root, vec!["a::f".to_string()]));
        let first = take_next_prewarm_batch(&mut queue, &root);
        assert_eq!(first, Some(vec!["a::f".to_string()]));

        assert!(!enqueue_prewarm_paths(&mut queue, &root, vec!["z::k".to_string()]));
        let second = take_next_prewarm_batch(&mut queue, &root);
        assert_eq!(second, Some(vec!["z::k".to_string()]));
        assert!(take_next_prewarm_batch(&mut queue, &root).is_none());
        assert!(!queue.contains_key(&root));
    }

    #[test]
    fn lsp_rust_inspect_workspace_includes_resolved_inline_and_stdlib_requirements()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let manifest_path = tmp.path().join("incan.toml");
        std::fs::write(&manifest_path, "[project]\nname = \"demo\"\n")?;
        let manifest = ProjectManifest::from_str("[project]\nname = \"demo\"\n", &manifest_path)?;

        let source = r#"
import std.serde.json
from rust::serde import Serialize

def use_it(x: Serialize) -> None:
  pass
"#;
        let tokens = lexer::lex(source).map_err(|errs| std::io::Error::other(format!("lex failed: {errs:?}")))?;
        let ast = parser::parse_with_context(&tokens, Some("src/main.incn"), Some(&std::collections::HashMap::new()))
            .map_err(|errs| std::io::Error::other(format!("parse failed: {errs:?}")))?;
        let module = ParsedModule {
            name: "main".to_string(),
            path_segments: vec!["main".to_string()],
            file_path: tmp.path().join("src").join("main.incn"),
            source: source.to_string(),
            ast,
        };

        let (out_dir, _query_paths) =
            prepare_lsp_rust_inspect_workspace(&manifest, &[module], &LibraryManifestIndex::default())
                .map_err(std::io::Error::other)?;
        let cargo_toml = std::fs::read_to_string(out_dir.join("Cargo.toml"))?;

        assert!(
            cargo_toml.contains("serde"),
            "expected inline rust import dependency in generated Cargo.toml, got:\n{cargo_toml}"
        );
        assert!(
            cargo_toml.contains("incan_stdlib") && cargo_toml.contains("json"),
            "expected stdlib feature propagation in generated Cargo.toml, got:\n{cargo_toml}"
        );
        Ok(())
    }
}

#[cfg(test)]
mod lsp_parse_tests {
    use crate::frontend::{lexer, parser};

    #[test]
    fn lsp_parse_context_accepts_for_tuple_unpack_binding() {
        let source = r#"
def bind(input_columns: list[str]) -> list[str]:
    mut bindings: list[str] = []
    for idx, name in enumerate(input_columns):
        bindings.append(name)
    return bindings
"#;

        let tokens = match lexer::lex(source) {
            Ok(tokens) => tokens,
            Err(errors) => panic!("lexer failed for LSP tuple-for regression: {errors:?}"),
        };
        let parsed = parser::parse_with_context(
            &tokens,
            Some("/workspace/src/substrait/plan.incn"),
            Some(&std::collections::HashMap::new()),
        );

        assert!(
            parsed.is_ok(),
            "LSP parser context should accept tuple-unpack for bindings, got {parsed:?}"
        );
    }
}

#[cfg(test)]
mod lsp_classmethod_tests {
    use std::collections::HashMap;

    use super::{classmethod_cls_detail, classmethod_context_at_offset, identifier_at_offset};
    use crate::frontend::{lexer, parser};

    fn parse_source(source: &str) -> Result<crate::frontend::ast::Program, String> {
        let tokens = lexer::lex(source).map_err(|errors| format!("lexer failed: {errors:?}"))?;
        parser::parse_with_context(&tokens, Some("src/main.incn"), Some(&HashMap::new()))
            .map_err(|errors| format!("parser failed: {errors:?}"))
    }

    #[test]
    fn classmethod_context_surfaces_cls_receiver_for_lsp() -> Result<(), String> {
        let source = r#"
class Box[T with Clone]:
    value: T

    @classmethod
    def make(cls, value: T) -> Self:
        return cls(value=value)
"#;
        let ast = parse_source(source)?;
        let offset = source
            .find("cls(value")
            .ok_or_else(|| "expected cls call".to_string())?;
        let aliases = HashMap::new();

        let context = classmethod_context_at_offset(&ast, offset, &aliases)
            .ok_or_else(|| "expected classmethod context".to_string())?;
        assert_eq!(context.owner_type, "Box[T]");
        assert_eq!(classmethod_cls_detail(&context), "cls: type[Box[T]]");

        let (ident, span) =
            identifier_at_offset(source, offset).ok_or_else(|| "expected identifier at cls call".to_string())?;
        assert_eq!(ident, "cls");
        assert_eq!(&source[span.start..span.end], "cls");
        Ok(())
    }

    #[test]
    fn staticmethod_body_does_not_surface_cls_receiver_for_lsp() -> Result<(), String> {
        let source = r#"
class Box[T with Clone]:
    value: T

    @staticmethod
    def make(value: T) -> Self:
        return Box(value=value)
"#;
        let ast = parse_source(source)?;
        let offset = source
            .find("return Box")
            .ok_or_else(|| "expected static factory body".to_string())?;
        let aliases = HashMap::new();

        assert!(classmethod_context_at_offset(&ast, offset, &aliases).is_none());
        Ok(())
    }
}

#[cfg(test)]
mod lsp_api_metadata_preview_tests {
    use super::{
        api_metadata_preview_at_offset, api_metadata_previews, enum_completion_detail, enum_variant_completion_detail,
        enum_variant_completion_label,
    };
    use crate::frontend::api_metadata::{CheckedApiMetadata, collect_checked_api_metadata};
    use crate::frontend::ast::{Declaration, ParamKind, Span};
    use crate::frontend::symbols::{CallableParam, ResolvedType, Symbol, SymbolKind, VariableInfo};
    use crate::frontend::{lexer, parser, typechecker};

    fn checked_metadata_for(source: &str) -> Result<(crate::frontend::ast::Program, CheckedApiMetadata), String> {
        let tokens = lexer::lex(source).map_err(|errors| format!("lexer failed: {errors:?}"))?;
        let ast = parser::parse(&tokens).map_err(|errors| format!("parser failed: {errors:?}"))?;
        let mut checker = typechecker::TypeChecker::new();
        checker
            .check_program(&ast)
            .map_err(|errors| format!("typecheck failed: {errors:?}"))?;
        let metadata = collect_checked_api_metadata(&ast, &checker, vec!["lib".to_string()]);
        Ok((ast, metadata))
    }

    #[test]
    fn checked_api_previews_use_callable_rebound_function_signature() -> Result<(), String> {
        let source = r#"
pub def endpoint() -> str:
    return "raw"
"#;
        let tokens = lexer::lex(source).map_err(|errors| format!("lexer failed: {errors:?}"))?;
        let ast = parser::parse(&tokens).map_err(|errors| format!("parser failed: {errors:?}"))?;
        let mut checker = typechecker::TypeChecker::new();
        checker
            .check_program(&ast)
            .map_err(|errors| format!("typecheck failed: {errors:?}"))?;

        checker.symbols.define(Symbol {
            name: "endpoint".to_string(),
            kind: SymbolKind::Variable(VariableInfo {
                ty: ResolvedType::Function(
                    vec![CallableParam::named("id", ResolvedType::Int, ParamKind::Normal)],
                    Box::new(ResolvedType::Bool),
                ),
                is_mutable: false,
                is_used: false,
            }),
            span: Span::default(),
            scope: 0,
        });

        let metadata = collect_checked_api_metadata(&ast, &checker, vec!["lib".to_string()]);
        let previews = api_metadata_previews(&ast, &metadata);
        let function_offset = source
            .find("endpoint")
            .ok_or_else(|| "expected function name in fixture".to_string())?;
        let preview = api_metadata_preview_at_offset(&previews, function_offset)
            .ok_or_else(|| "expected checked function preview".to_string())?;

        assert!(
            preview.markdown.contains("pub def endpoint(id: int) -> bool"),
            "expected rebound callable signature in LSP preview, got:\n{}",
            preview.markdown
        );

        Ok(())
    }

    #[test]
    fn checked_api_previews_include_public_model_fields_and_methods() -> Result<(), String> {
        let source = r#"
pub const DEFAULT_LABEL = "none"

@derive(Clone)
pub model Order:
    """
    Order contract.
    """
    id [description="Stable id"] as "orderId": int
    label: str = DEFAULT_LABEL

    def label(self) -> str:
        """
        Return the display label.
        """
        return DEFAULT_LABEL
"#;
        let (ast, metadata) = checked_metadata_for(source)?;
        let previews = api_metadata_previews(&ast, &metadata);

        let field_offset = source
            .find("orderId")
            .ok_or_else(|| "expected field alias in fixture".to_string())?;
        let field_preview = api_metadata_preview_at_offset(&previews, field_offset)
            .ok_or_else(|| "expected checked field preview".to_string())?;
        assert!(
            field_preview.markdown.contains("*checked API metadata: public field*"),
            "expected public field metadata preview, got:\n{}",
            field_preview.markdown
        );
        assert!(
            field_preview.markdown.contains("alias: `orderId`"),
            "expected field alias in preview, got:\n{}",
            field_preview.markdown
        );
        assert!(
            field_preview.markdown.contains("description: `Stable id`"),
            "expected field description in preview, got:\n{}",
            field_preview.markdown
        );

        let method_offset = source
            .find("def label")
            .ok_or_else(|| "expected method in fixture".to_string())?;
        let method_preview = api_metadata_preview_at_offset(&previews, method_offset)
            .ok_or_else(|| "expected checked method preview".to_string())?;
        assert!(
            method_preview.markdown.contains("def Order.label(self) -> str"),
            "expected checked method signature, got:\n{}",
            method_preview.markdown
        );
        assert!(
            method_preview
                .markdown
                .contains("docstring: `Return the display label.`"),
            "expected method docstring in preview, got:\n{}",
            method_preview.markdown
        );

        Ok(())
    }

    #[test]
    fn checked_api_previews_skip_private_declarations() -> Result<(), String> {
        let source = r#"
model Secret:
    value: int

pub model Public:
    value: int
"#;
        let (ast, metadata) = checked_metadata_for(source)?;
        let previews = api_metadata_previews(&ast, &metadata);

        let private_offset = source
            .find("Secret")
            .ok_or_else(|| "expected private model in fixture".to_string())?;
        assert!(
            api_metadata_preview_at_offset(&previews, private_offset).is_none(),
            "private declarations must not expose checked API metadata previews"
        );

        let public_offset = source
            .find("Public")
            .ok_or_else(|| "expected public model in fixture".to_string())?;
        assert!(
            api_metadata_preview_at_offset(&previews, public_offset).is_some(),
            "public declarations should expose checked API metadata previews"
        );

        Ok(())
    }

    #[test]
    fn checked_api_previews_include_value_enum_backing_and_variant_values() -> Result<(), String> {
        let source = r#"
pub enum Environment(str):
    Dev = "development"
    Prod = "production"
"#;
        let (ast, metadata) = checked_metadata_for(source)?;
        let previews = api_metadata_previews(&ast, &metadata);

        let enum_offset = source
            .find("Environment")
            .ok_or_else(|| "expected enum name in fixture".to_string())?;
        let enum_preview = api_metadata_preview_at_offset(&previews, enum_offset)
            .ok_or_else(|| "expected checked enum preview".to_string())?;
        assert!(
            enum_preview.markdown.contains("value type: `str`"),
            "expected enum backing type in preview, got:\n{}",
            enum_preview.markdown
        );
        assert!(
            !enum_preview.markdown.contains("value type: `Str`"),
            "enum backing type should use Incan spelling, got:\n{}",
            enum_preview.markdown
        );

        let variant_offset = source
            .find("Prod =")
            .ok_or_else(|| "expected value enum variant in fixture".to_string())?;
        let variant_preview = api_metadata_preview_at_offset(&previews, variant_offset)
            .ok_or_else(|| "expected checked enum variant preview".to_string())?;
        assert!(
            variant_preview
                .markdown
                .contains("*checked API metadata: public enum variant*"),
            "expected enum variant metadata preview, got:\n{}",
            variant_preview.markdown
        );
        assert!(
            variant_preview.markdown.contains("value type: `str`"),
            "expected variant backing type in preview, got:\n{}",
            variant_preview.markdown
        );
        assert!(
            variant_preview.markdown.contains("raw value: `\"production\"`"),
            "expected variant raw value in preview, got:\n{}",
            variant_preview.markdown
        );

        Ok(())
    }

    #[test]
    fn local_value_enum_completion_details_include_raw_values() -> Result<(), String> {
        let source = r#"
enum HttpStatus(int):
    Ok = 200
"#;
        let (ast, _metadata) = checked_metadata_for(source)?;
        let enum_decl = ast
            .declarations
            .iter()
            .find_map(|decl| match &decl.node {
                Declaration::Enum(en) => Some(en),
                _ => None,
            })
            .ok_or_else(|| "expected enum declaration in fixture".to_string())?;
        let variant = enum_decl
            .variants
            .first()
            .ok_or_else(|| "expected enum variant in fixture".to_string())?;

        assert_eq!(enum_completion_detail(enum_decl), "enum HttpStatus(int)");
        assert_eq!(
            enum_variant_completion_detail(enum_decl, &variant.node),
            "variant HttpStatus.Ok: int = 200"
        );
        assert_eq!(enum_variant_completion_label(enum_decl, &variant.node), "HttpStatus.Ok");

        Ok(())
    }

    #[test]
    fn checked_api_previews_escape_backticks_in_inline_metadata() -> Result<(), String> {
        let escaped = super::inline_code("Use `code` here.");
        assert_eq!(escaped, "`` Use `code` here. ``");
        Ok(())
    }
}

#[cfg(test)]
mod lsp_computed_property_tests {
    use super::{find_property_symbol_info, format_property_signature};
    use crate::frontend::ast::Declaration;
    use crate::frontend::{lexer, parser};

    #[test]
    fn computed_property_hover_surfaces_owner_and_type() -> Result<(), String> {
        let source = r#"
model Account:
    cents: int

    property dollars -> int:
        return self.cents
"#;
        let tokens = lexer::lex(source).map_err(|errors| format!("lexer failed: {errors:?}"))?;
        let ast = parser::parse(&tokens).map_err(|errors| format!("parser failed: {errors:?}"))?;
        let model = ast
            .declarations
            .iter()
            .find_map(|decl| match &decl.node {
                Declaration::Model(model) => Some(model),
                _ => None,
            })
            .ok_or_else(|| "expected model declaration".to_string())?;
        let property = model
            .properties
            .first()
            .ok_or_else(|| "expected computed property".to_string())?;

        assert_eq!(
            format_property_signature(&model.name, &property.node),
            "property Account.dollars -> int"
        );
        let offset = source
            .find("dollars")
            .ok_or_else(|| "expected property name in source".to_string())?;
        let info = find_property_symbol_info(&model.name, &model.properties, offset)
            .ok_or_else(|| "expected property hover symbol".to_string())?;
        assert_eq!(info.kind, "property");
        assert_eq!(info.detail, "property Account.dollars -> int");
        Ok(())
    }
}

#[cfg(test)]
mod lsp_contract_model_command_tests {
    use super::{
        ContractModelCommandFormat, emit_contract_model_command_payload, parse_emit_contract_model_command_args,
    };

    fn write_project_bundle(root: &std::path::Path) -> std::io::Result<()> {
        std::fs::create_dir_all(root.join("contracts"))?;
        std::fs::write(
            root.join("incan.toml"),
            r#"[project]
name = "lsp_contract_model"
version = "0.1.0"

[tool.incan.metadata]
model-bundles = ["contracts/order_summary.json"]
"#,
        )?;
        std::fs::write(
            root.join("contracts").join("order_summary.json"),
            r#"{
  "schema_version": 1,
  "stable_model_id": "orders.summary",
  "logical_type_name": "OrderSummary",
  "publishable": true,
  "fields": [
    {
      "name": "order_id",
      "type": "str",
      "alias": "orderId"
    }
  ]
}
"#,
        )?;
        Ok(())
    }

    #[test]
    fn emit_contract_model_command_payload_returns_incan_source() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        write_project_bundle(tmp.path())?;

        let payload =
            emit_contract_model_command_payload(tmp.path(), "orders.summary", ContractModelCommandFormat::Incan)?;

        assert_eq!(
            payload.pointer("/format").and_then(serde_json::Value::as_str),
            Some("incan")
        );
        assert_eq!(
            payload.pointer("/model").and_then(serde_json::Value::as_str),
            Some("OrderSummary")
        );
        let source = payload
            .pointer("/source")
            .and_then(serde_json::Value::as_str)
            .ok_or("expected source payload")?;
        assert!(
            source.contains("pub model OrderSummary:"),
            "expected model source payload, got:\n{source}"
        );
        assert!(
            source.contains("order_id as \"orderId\": str") || source.contains("order_id [alias=\"orderId\"]: str"),
            "expected alias-preserving field source, got:\n{source}"
        );
        Ok(())
    }

    #[test]
    fn parse_emit_contract_model_command_args_accepts_object_argument() -> Result<(), Box<dyn std::error::Error>> {
        let args = parse_emit_contract_model_command_args(vec![serde_json::json!({
            "uri": "file:///tmp/project/src/main.incn",
            "model": "OrderSummary",
            "format": "json"
        })])?;

        assert_eq!(args.uri.as_deref(), Some("file:///tmp/project/src/main.incn"));
        assert_eq!(args.model, "OrderSummary");
        assert_eq!(args.format.as_deref(), Some("json"));
        Ok(())
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

/// Format a function signature for display
fn format_function_signature(func: &crate::frontend::ast::FunctionDecl) -> String {
    let mut sig = String::new();

    if func.is_async() {
        sig.push_str("async ");
    }

    sig.push_str("def ");
    sig.push_str(&func.name);
    sig.push('(');

    let params: Vec<String> = func
        .params
        .iter()
        .map(|p| format!("{}: {}", p.node.name, format_type(&p.node.ty.node)))
        .collect();

    sig.push_str(&params.join(", "));
    sig.push(')');

    sig.push_str(" -> ");
    sig.push_str(&format_type(&func.return_type.node));

    sig
}

/// Format a computed property declaration for hover and completion details.
fn format_property_signature(owner: &str, property: &crate::frontend::ast::PropertyDecl) -> String {
    format!(
        "property {}.{} -> {}",
        owner,
        property.name,
        format_type(&property.return_type.node)
    )
}

/// Return symbol information when an offset falls inside a computed property declaration.
fn find_property_symbol_info(
    owner: &str,
    properties: &[crate::frontend::ast::Spanned<crate::frontend::ast::PropertyDecl>],
    offset: usize,
) -> Option<SymbolInfo> {
    for property in properties {
        if property.span.start <= offset && offset < property.span.end {
            return Some(SymbolInfo {
                name: property.node.name.clone(),
                kind: "property".to_string(),
                detail: format_property_signature(owner, &property.node),
                span: property.span,
            });
        }
    }
    None
}

/// Format a Type for display
fn format_type(ty: &Type) -> String {
    match ty {
        Type::Simple(name) => name.clone(),
        Type::Qualified(segments) => segments.join("::"),
        Type::Generic(name, params) => {
            let params_str: Vec<String> = params.iter().map(|p| format_type(&p.node)).collect();
            format!("{}[{}]", name, params_str.join(", "))
        }
        Type::Tuple(types) => {
            let types_str: Vec<String> = types.iter().map(|t| format_type(&t.node)).collect();
            format!("({})", types_str.join(", "))
        }
        Type::Function(params, ret) => {
            let params_str: Vec<String> = params.iter().map(|p| format_type(&p.node)).collect();
            format!("({}) -> {}", params_str.join(", "), format_type(&ret.node))
        }
        Type::Ref(inner) => format!("&{}", format_type(&inner.node)),
        Type::RefMut(inner) => format!("&mut {}", format_type(&inner.node)),
        Type::IntLiteral(value) => value.repr.clone(),
        Type::Unit => "()".to_string(),
        Type::SelfType => "Self".to_string(),
        Type::Infer => "_".to_string(),
    }
}

/// Return the logical metadata module path used for LSP previews of one open document.
fn lsp_metadata_module_path(path: Option<&Path>) -> Vec<String> {
    path.and_then(|path| path.file_stem())
        .and_then(|stem| stem.to_str())
        .map(|stem| vec![stem.to_string()])
        .unwrap_or_else(|| vec!["main".to_string()])
}

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

/// Build hover-ready checked API snippets for public metadata declarations in an analyzed document.
fn api_metadata_previews(ast: &Program, metadata: &CheckedApiMetadata) -> Vec<ApiMetadataPreview> {
    let mut previews = Vec::new();
    for declaration in &metadata.declarations {
        match declaration {
            ApiDeclaration::Function(function) => {
                previews.push(preview_for_anchor(&function.anchor, api_function_markdown(function)))
            }
            ApiDeclaration::Model(model) => {
                previews.push(preview_for_anchor(&model.anchor, api_model_markdown(model)));
                if let Some(Declaration::Model(ast_model)) = find_top_level_decl(ast, &model.name) {
                    push_field_previews(&mut previews, &model.name, &model.fields, &ast_model.fields);
                }
                push_method_previews(&mut previews, &model.name, &model.methods);
            }
            ApiDeclaration::Class(class) => {
                previews.push(preview_for_anchor(&class.anchor, api_class_markdown(class)));
                if let Some(Declaration::Class(ast_class)) = find_top_level_decl(ast, &class.name) {
                    push_field_previews(&mut previews, &class.name, &class.fields, &ast_class.fields);
                }
                push_method_previews(&mut previews, &class.name, &class.methods);
            }
            ApiDeclaration::Trait(trait_decl) => {
                previews.push(preview_for_anchor(&trait_decl.anchor, api_trait_markdown(trait_decl)));
                push_method_previews(&mut previews, &trait_decl.name, &trait_decl.methods);
            }
            ApiDeclaration::Enum(enum_decl) => {
                previews.push(preview_for_anchor(&enum_decl.anchor, api_enum_markdown(enum_decl)));
                if let Some(Declaration::Enum(ast_enum)) = find_top_level_decl(ast, &enum_decl.name) {
                    push_enum_variant_previews(
                        &mut previews,
                        &enum_decl.name,
                        enum_decl.value_type,
                        &enum_decl.variants,
                        &ast_enum.variants,
                    );
                }
            }
            ApiDeclaration::Newtype(newtype) => {
                previews.push(preview_for_anchor(&newtype.anchor, api_newtype_markdown(newtype)));
                push_method_previews(&mut previews, &newtype.name, &newtype.methods);
            }
            ApiDeclaration::TypeAlias(alias) => {
                previews.push(preview_for_anchor(&alias.anchor, api_type_alias_markdown(alias)))
            }
            ApiDeclaration::Const(konst) => previews.push(preview_for_anchor(&konst.anchor, api_const_markdown(konst))),
            ApiDeclaration::Static(static_decl) => previews.push(preview_for_anchor(
                &static_decl.anchor,
                api_static_markdown(static_decl),
            )),
            ApiDeclaration::Alias(alias) => previews.push(ApiMetadataPreview {
                span: source_anchor_span(&alias.anchor),
                markdown: checked_api_markdown(
                    format!("pub alias {} = {}", alias.name, alias.target_path.join("::")),
                    "public import alias",
                    Vec::new(),
                ),
            }),
        }
    }
    previews
}

/// Return the most precise checked API preview containing `offset`.
fn api_metadata_preview_at_offset(previews: &[ApiMetadataPreview], offset: usize) -> Option<&ApiMetadataPreview> {
    previews
        .iter()
        .filter(|preview| preview.span.start <= offset && offset < preview.span.end)
        .min_by_key(|preview| preview.span.end.saturating_sub(preview.span.start))
}

/// Convert a metadata anchor to an LSP preview entry.
fn preview_for_anchor(anchor: &SourceAnchor, markdown: String) -> ApiMetadataPreview {
    ApiMetadataPreview {
        span: source_anchor_span(anchor),
        markdown,
    }
}

/// Convert a metadata anchor span to the frontend span type used by LSP ranges.
fn source_anchor_span(anchor: &SourceAnchor) -> Span {
    Span::new(anchor.span.start, anchor.span.end)
}

/// Find a top-level declaration by its exported source name.
fn find_top_level_decl<'a>(ast: &'a Program, name: &str) -> Option<&'a Declaration> {
    ast.declarations.iter().find_map(|decl| match &decl.node {
        Declaration::Function(function) if function.name == name => Some(&decl.node),
        Declaration::Model(model) if model.name == name => Some(&decl.node),
        Declaration::Class(class) if class.name == name => Some(&decl.node),
        Declaration::Trait(trait_decl) if trait_decl.name == name => Some(&decl.node),
        Declaration::Enum(enum_decl) if enum_decl.name == name => Some(&decl.node),
        Declaration::TypeAlias(alias) if alias.name == name => Some(&decl.node),
        Declaration::Newtype(newtype) if newtype.name == name => Some(&decl.node),
        Declaration::Const(konst) if konst.name == name => Some(&decl.node),
        Declaration::Static(static_decl) if static_decl.name == name => Some(&decl.node),
        _ => None,
    })
}

/// Add variant-level previews for checked public enum variants using AST source spans.
fn push_enum_variant_previews(
    previews: &mut Vec<ApiMetadataPreview>,
    owner: &str,
    value_type: Option<EnumValueTypeExport>,
    checked_variants: &[crate::frontend::api_metadata::ApiEnumVariant],
    ast_variants: &[crate::frontend::ast::Spanned<crate::frontend::ast::VariantDecl>],
) {
    for variant in ast_variants {
        let Some(checked) = checked_variants
            .iter()
            .find(|checked| checked.name == variant.node.name)
        else {
            continue;
        };
        previews.push(ApiMetadataPreview {
            span: variant.span,
            markdown: api_enum_variant_markdown(owner, value_type, checked),
        });
    }
}

/// Add field-level previews for checked public model/class fields using AST source spans.
fn push_field_previews(
    previews: &mut Vec<ApiMetadataPreview>,
    owner: &str,
    checked_fields: &[FieldExport],
    ast_fields: &[crate::frontend::ast::Spanned<crate::frontend::ast::FieldDecl>],
) {
    for field in ast_fields {
        let Some(checked) = checked_fields.iter().find(|checked| checked.name == field.node.name) else {
            continue;
        };
        previews.push(ApiMetadataPreview {
            span: field.span,
            markdown: api_field_markdown(owner, checked),
        });
    }
}

/// Add method-level previews using checked API metadata method anchors.
fn push_method_previews(previews: &mut Vec<ApiMetadataPreview>, owner: &str, methods: &[ApiMethod]) {
    for method in methods {
        previews.push(preview_for_anchor(&method.anchor, api_method_markdown(owner, method)));
    }
}

/// Format a checked public function preview as hover markdown.
fn api_function_markdown(function: &ApiFunction) -> String {
    let mut facts = Vec::new();
    push_docstring_fact(&mut facts, function.docstring.as_deref());
    if !function.decorators.is_empty() {
        facts.push(format!("decorators: {}", function.decorators.len()));
    }
    checked_api_markdown(
        format!("pub {}", format_api_function_signature(function)),
        "public function",
        facts,
    )
}

/// Format a checked public model preview as hover markdown.
fn api_model_markdown(model: &ApiModel) -> String {
    let mut facts = Vec::new();
    push_docstring_fact(&mut facts, model.docstring.as_deref());
    push_list_fact(&mut facts, "derives", &model.derives);
    push_list_fact(&mut facts, "traits", &model.traits);
    facts.push(format!("fields: {}", model.fields.len()));
    facts.push(format!("methods: {}", model.methods.len()));
    checked_api_markdown(
        format!("pub model {}{}", model.name, format_type_params(&model.type_params)),
        "public model",
        facts,
    )
}

/// Format a checked public class preview as hover markdown.
fn api_class_markdown(class: &ApiClass) -> String {
    let mut facts = Vec::new();
    push_docstring_fact(&mut facts, class.docstring.as_deref());
    if let Some(parent) = &class.extends {
        facts.push(format!("extends: `{parent}`"));
    }
    push_list_fact(&mut facts, "derives", &class.derives);
    push_list_fact(&mut facts, "traits", &class.traits);
    facts.push(format!("fields: {}", class.fields.len()));
    facts.push(format!("methods: {}", class.methods.len()));
    checked_api_markdown(
        format!("pub class {}{}", class.name, format_type_params(&class.type_params)),
        "public class",
        facts,
    )
}

/// Format a checked public trait preview as hover markdown.
fn api_trait_markdown(trait_decl: &ApiTrait) -> String {
    let mut facts = Vec::new();
    push_docstring_fact(&mut facts, trait_decl.docstring.as_deref());
    if !trait_decl.supertraits.is_empty() {
        facts.push(format!(
            "supertraits: {}",
            trait_decl
                .supertraits
                .iter()
                .map(format_type_bound)
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    facts.push(format!("requirements: {}", trait_decl.requires.len()));
    facts.push(format!("methods: {}", trait_decl.methods.len()));
    checked_api_markdown(
        format!(
            "pub trait {}{}",
            trait_decl.name,
            format_type_params(&trait_decl.type_params)
        ),
        "public trait",
        facts,
    )
}

/// Format a checked public enum preview as hover markdown.
fn api_enum_markdown(enum_decl: &ApiEnum) -> String {
    let mut facts = Vec::new();
    push_docstring_fact(&mut facts, enum_decl.docstring.as_deref());
    push_list_fact(&mut facts, "derives", &enum_decl.derives);
    facts.push(format!("variants: {}", enum_decl.variants.len()));
    if let Some(value_type) = enum_decl.value_type {
        facts.push(format!("value type: `{}`", enum_value_type_display(value_type)));
    }
    checked_api_markdown(
        format!(
            "pub enum {}{}",
            enum_decl.name,
            format_type_params(&enum_decl.type_params)
        ),
        "public enum",
        facts,
    )
}

/// Format a checked public enum variant preview as hover markdown.
fn api_enum_variant_markdown(
    owner: &str,
    value_type: Option<EnumValueTypeExport>,
    variant: &crate::frontend::api_metadata::ApiEnumVariant,
) -> String {
    let mut facts = Vec::new();
    if !variant.fields.is_empty() {
        facts.push(format!(
            "fields: {}",
            variant
                .fields
                .iter()
                .map(format_type_ref)
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if let Some(value_type) = value_type {
        facts.push(format!("value type: `{}`", enum_value_type_display(value_type)));
    }
    if let Some(value) = &variant.value {
        facts.push(format!("raw value: `{}`", enum_value_display(value)));
    }
    checked_api_markdown(
        format!("variant {owner}.{}", variant.name),
        "public enum variant",
        facts,
    )
}

/// Format a checked public newtype or rusttype preview as hover markdown.
fn api_newtype_markdown(newtype: &ApiNewtype) -> String {
    let mut facts = Vec::new();
    push_docstring_fact(&mut facts, newtype.docstring.as_deref());
    facts.push(format!("underlying: `{}`", format_type_ref(&newtype.underlying)));
    facts.push(format!("methods: {}", newtype.methods.len()));
    let kind = if newtype.is_rusttype { "rusttype" } else { "newtype" };
    checked_api_markdown(
        format!(
            "pub {kind} {}{} = {}",
            newtype.name,
            format_type_params(&newtype.type_params),
            format_type_ref(&newtype.underlying)
        ),
        &format!("public {kind}"),
        facts,
    )
}

/// Format a checked public type-alias preview as hover markdown.
fn api_type_alias_markdown(alias: &ApiTypeAlias) -> String {
    checked_api_markdown(
        format!(
            "pub type {}{} = {}",
            alias.name,
            format_type_params(&alias.type_alias.type_params),
            format_type_ref(&alias.type_alias.target)
        ),
        "public type alias",
        Vec::new(),
    )
}

/// Format a checked public const preview as hover markdown.
fn api_const_markdown(konst: &ApiConst) -> String {
    let mut facts = Vec::new();
    if let Some(value) = &konst.value {
        facts.push(format!("safe value: `{value:?}`"));
    }
    checked_api_markdown(
        format!("pub const {}: {}", konst.name, format_type_ref(&konst.ty)),
        "public const",
        facts,
    )
}

/// Format a checked public static preview as hover markdown.
fn api_static_markdown(static_decl: &ApiStatic) -> String {
    checked_api_markdown(
        format!("pub static {}: {}", static_decl.name, format_type_ref(&static_decl.ty)),
        "public static",
        Vec::new(),
    )
}

/// Format a checked public model/class field preview as hover markdown.
fn api_field_markdown(owner: &str, field: &FieldExport) -> String {
    let mut facts = Vec::new();
    if field.has_default {
        facts.push("has default: `true`".to_string());
    }
    if let Some(alias) = &field.alias {
        facts.push(format!("alias: `{alias}`"));
    }
    if let Some(description) = &field.description {
        facts.push(format!("description: {}", inline_code(description)));
    }
    checked_api_markdown(
        format!("field {}.{}: {}", owner, field.name, format_type_ref(&field.ty)),
        "public field",
        facts,
    )
}

/// Format a checked public method preview as hover markdown.
fn api_method_markdown(owner: &str, method: &ApiMethod) -> String {
    let mut facts = Vec::new();
    push_docstring_fact(&mut facts, method.docstring.as_deref());
    if !method.decorators.is_empty() {
        facts.push(format!("decorators: {}", method.decorators.len()));
    }
    if !method.has_body {
        facts.push("body: abstract".to_string());
    }
    checked_api_markdown(format_api_method_signature(owner, method), "public method", facts)
}

/// Wrap a checked metadata signature and fact list in LSP markdown.
fn checked_api_markdown(signature: String, kind: &str, facts: Vec<String>) -> String {
    let mut markdown = format!("```incan\n{signature}\n```\n\n*checked API metadata: {kind}*");
    for fact in facts {
        markdown.push_str("\n\n");
        markdown.push_str(&fact);
    }
    markdown
}

/// Add a trimmed raw docstring fact when metadata carries one.
fn push_docstring_fact(facts: &mut Vec<String>, docstring: Option<&str>) {
    if let Some(docstring) = docstring.map(str::trim).filter(|docstring| !docstring.is_empty()) {
        facts.push(format!("docstring: {}", inline_code(docstring)));
    }
}

/// Add a comma-separated list fact when the list is non-empty.
fn push_list_fact(facts: &mut Vec<String>, label: &str, values: &[String]) {
    if values.is_empty() {
        return;
    }
    facts.push(format!(
        "{label}: {}",
        values
            .iter()
            .map(|value| inline_code(value))
            .collect::<Vec<_>>()
            .join(", ")
    ));
}

/// Format a checked API function signature.
fn format_api_function_signature(function: &ApiFunction) -> String {
    let prefix = if function.is_async { "async def" } else { "def" };
    format!(
        "{prefix} {}{}({}) -> {}",
        function.name,
        format_type_params(&function.type_params),
        format_params(&function.params),
        format_type_ref(&function.return_type)
    )
}

/// Format a checked API method signature with its owning type.
fn format_api_method_signature(owner: &str, method: &ApiMethod) -> String {
    let prefix = if method.is_async { "async def" } else { "def" };
    let mut params = Vec::new();
    if let Some(receiver) = &method.receiver {
        params.push(match receiver {
            ReceiverExport::Immutable => "self".to_string(),
            ReceiverExport::Mutable => "mut self".to_string(),
        });
    }
    params.extend(method.params.iter().map(format_param));
    format!(
        "{prefix} {owner}.{}{}({}) -> {}",
        method.name,
        format_type_params(&method.type_params),
        params.join(", "),
        format_type_ref(&method.return_type)
    )
}

/// Format checked API type parameters in Incan source-like syntax.
fn format_type_params(type_params: &[TypeParamExport]) -> String {
    if type_params.is_empty() {
        return String::new();
    }
    format!(
        "[{}]",
        type_params
            .iter()
            .map(|param| {
                if param.bounds.is_empty() {
                    param.name.clone()
                } else {
                    format!(
                        "{} with {}",
                        param.name,
                        param
                            .bounds
                            .iter()
                            .map(format_type_bound)
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                }
            })
            .collect::<Vec<_>>()
            .join(", ")
    )
}

/// Format a checked API type bound.
fn format_type_bound(bound: &TypeBoundExport) -> String {
    if bound.type_args.is_empty() {
        return bound.name.clone();
    }
    format!(
        "{}[{}]",
        bound.name,
        bound
            .type_args
            .iter()
            .map(format_type_ref)
            .collect::<Vec<_>>()
            .join(", ")
    )
}

/// Format a checked API parameter list.
fn format_params(params: &[ParamExport]) -> String {
    params.iter().map(format_param).collect::<Vec<_>>().join(", ")
}

/// Format one checked API parameter.
fn format_param(param: &ParamExport) -> String {
    let prefix = match param.kind {
        ParamKindExport::Normal => "",
        ParamKindExport::RestPositional => "*",
        ParamKindExport::RestKeyword => "**",
    };
    let default = if param.has_default { " = ..." } else { "" };
    format!("{prefix}{}: {}{default}", param.name, format_type_ref(&param.ty))
}

/// Format a manifest-level type reference for concise hover display.
fn format_type_ref(ty: &TypeRef) -> String {
    match ty {
        TypeRef::Named { name } => name.clone(),
        TypeRef::Applied { name, args } => {
            format!(
                "{name}[{}]",
                args.iter().map(format_type_ref).collect::<Vec<_>>().join(", ")
            )
        }
        TypeRef::Function { params, return_type } => {
            format!(
                "({}) -> {}",
                params.iter().map(format_type_ref).collect::<Vec<_>>().join(", "),
                format_type_ref(return_type)
            )
        }
        TypeRef::Tuple { elements } => {
            format!(
                "({})",
                elements.iter().map(format_type_ref).collect::<Vec<_>>().join(", ")
            )
        }
        TypeRef::TypeParam { name } => name.clone(),
        TypeRef::SelfType => "Self".to_string(),
        TypeRef::Ref { inner } => format!("ref {}", format_type_ref(inner)),
        TypeRef::RustPath { path } => format!("rust::{path}"),
        TypeRef::Unknown => "_".to_string(),
    }
}

/// Format checked value-enum backing metadata using Incan surface spellings.
fn enum_value_type_display(value_type: EnumValueTypeExport) -> &'static str {
    match value_type {
        EnumValueTypeExport::Str => "str",
        EnumValueTypeExport::Int => "int",
    }
}

/// Format a checked value-enum raw value as it appears in source-like LSP metadata.
fn enum_value_display(value: &EnumValueExport) -> String {
    match value {
        EnumValueExport::Str(value) => format!("{value:?}"),
        EnumValueExport::Int(value) => value.to_string(),
    }
}

/// Format parsed value-enum backing metadata using Incan surface spellings.
fn ast_value_enum_type_display(value_type: crate::frontend::ast::ValueEnumType) -> &'static str {
    match value_type {
        crate::frontend::ast::ValueEnumType::Str => "str",
        crate::frontend::ast::ValueEnumType::Int => "int",
    }
}

/// Format a parsed value-enum literal as source-like LSP metadata.
fn ast_value_enum_literal_display(value: &crate::frontend::ast::ValueEnumLiteral) -> String {
    match value {
        crate::frontend::ast::ValueEnumLiteral::Str(value) => format!("{value:?}"),
        crate::frontend::ast::ValueEnumLiteral::Int(value) => value.value.to_string(),
    }
}

/// Format local enum completion detail, including RFC 032 backing metadata when present.
fn enum_completion_detail(en: &crate::frontend::ast::EnumDecl) -> String {
    match en.value_type.as_ref() {
        Some(value_type) => format!("enum {}({})", en.name, ast_value_enum_type_display(value_type.node)),
        None => format!("enum {}", en.name),
    }
}

/// Format local enum variant completion labels as qualified enum constructors.
fn enum_variant_completion_label(
    en: &crate::frontend::ast::EnumDecl,
    variant: &crate::frontend::ast::VariantDecl,
) -> String {
    format!("{}.{}", en.name, variant.name)
}

/// Format local enum variant completion detail, including RFC 032 raw value metadata when present.
fn enum_variant_completion_detail(
    en: &crate::frontend::ast::EnumDecl,
    variant: &crate::frontend::ast::VariantDecl,
) -> String {
    let mut detail = format!("variant {}.{}", en.name, variant.name);
    if let Some(value_type) = &en.value_type {
        detail.push_str(&format!(": {}", ast_value_enum_type_display(value_type.node)));
    }
    if let Some(value) = &variant.value {
        detail.push_str(&format!(" = {}", ast_value_enum_literal_display(&value.node)));
    }
    detail
}

/// Escape a short metadata value as inline markdown code.
fn inline_code(value: &str) -> String {
    if !value.contains('`') {
        return format!("`{value}`");
    }
    let mut max_run = 0;
    let mut current_run = 0;
    for ch in value.chars() {
        if ch == '`' {
            current_run += 1;
            max_run = max_run.max(current_run);
        } else {
            current_run = 0;
        }
    }
    let fence = "`".repeat(max_run + 1);
    format!("{fence} {value} {fence}")
}

fn collect_import_aliases(ast: &Program) -> HashMap<String, Vec<String>> {
    crate::frontend::decorator_resolution::collect_import_aliases(ast)
}

fn resolve_decorator_path(
    dec: &crate::frontend::ast::Decorator,
    aliases: &HashMap<String, Vec<String>>,
) -> Vec<String> {
    crate::frontend::decorator_resolution::resolve_decorator_path(dec, aliases)
}

/// Return whether a method has the requested decorator after resolving import aliases.
fn method_has_decorator(
    method: &MethodDecl,
    id: decorators::DecoratorId,
    aliases: &HashMap<String, Vec<String>>,
) -> bool {
    method
        .decorators
        .iter()
        .any(|decorator| decorators::from_segments(&resolve_decorator_path(&decorator.node, aliases)) == Some(id))
}

/// Format the owner type as it should appear in LSP details.
fn owner_type_display(owner_name: &str, type_params: &[TypeParam]) -> String {
    if type_params.is_empty() {
        owner_name.to_string()
    } else {
        let params: Vec<&str> = type_params.iter().map(|param| param.name.as_str()).collect();
        format!("{owner_name}[{}]", params.join(", "))
    }
}

/// Return whether an offset falls inside a method body rather than its signature.
fn method_body_contains_offset(method: &MethodDecl, method_span: Span, offset: usize) -> bool {
    let Some(body) = &method.body else {
        return false;
    };
    let Some(first) = body.first() else {
        return false;
    };
    first.span.start <= offset && offset < method_span.end
}

/// Build contextual classmethod receiver metadata when the offset is inside a classmethod body.
fn classmethod_context_for_method(
    owner_name: &str,
    owner_type_params: &[TypeParam],
    method: &crate::frontend::ast::Spanned<MethodDecl>,
    offset: usize,
    aliases: &HashMap<String, Vec<String>>,
) -> Option<ClassmethodContext> {
    if !method_body_contains_offset(&method.node, method.span, offset) {
        return None;
    }
    if !method_has_decorator(&method.node, decorators::DecoratorId::ClassMethod, aliases) {
        return None;
    }
    Some(ClassmethodContext {
        owner_type: owner_type_display(owner_name, owner_type_params),
    })
}

/// Find the active classmethod receiver context for an offset in a parsed program.
fn classmethod_context_at_offset(
    ast: &Program,
    offset: usize,
    aliases: &HashMap<String, Vec<String>>,
) -> Option<ClassmethodContext> {
    for decl in &ast.declarations {
        match &decl.node {
            Declaration::Model(model) => {
                for method in &model.methods {
                    if let Some(context) =
                        classmethod_context_for_method(&model.name, &model.type_params, method, offset, aliases)
                    {
                        return Some(context);
                    }
                }
            }
            Declaration::Class(class) => {
                for method in &class.methods {
                    if let Some(context) =
                        classmethod_context_for_method(&class.name, &class.type_params, method, offset, aliases)
                    {
                        return Some(context);
                    }
                }
            }
            Declaration::Newtype(newtype) => {
                for method in &newtype.methods {
                    if let Some(context) =
                        classmethod_context_for_method(&newtype.name, &newtype.type_params, method, offset, aliases)
                    {
                        return Some(context);
                    }
                }
            }
            _ => {}
        }
    }
    None
}

/// Return the identifier containing or immediately preceding an offset.
fn identifier_at_offset(source: &str, offset: usize) -> Option<(String, Span)> {
    if source.is_empty() {
        return None;
    }
    let mut cursor = offset.min(source.len().saturating_sub(1));
    if !source
        .as_bytes()
        .get(cursor)
        .is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_')
    {
        if cursor == 0 {
            return None;
        }
        cursor -= 1;
        if !source
            .as_bytes()
            .get(cursor)
            .is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_')
        {
            return None;
        }
    }

    let mut start = cursor;
    while start > 0 {
        let prev = source.as_bytes()[start - 1];
        if !(prev.is_ascii_alphanumeric() || prev == b'_') {
            break;
        }
        start -= 1;
    }

    let mut end = cursor + 1;
    while end < source.len() {
        let next = source.as_bytes()[end];
        if !(next.is_ascii_alphanumeric() || next == b'_') {
            break;
        }
        end += 1;
    }

    Some((source[start..end].to_string(), Span::new(start, end)))
}

/// Format the LSP detail string for the contextual `cls` receiver binding.
fn classmethod_cls_detail(context: &ClassmethodContext) -> String {
    format!(
        "{}: type[{}]",
        keywords::as_str(keywords::KeywordId::Cls),
        context.owner_type
    )
}

fn stdlib_location_for_path(path: &[String]) -> Option<Location> {
    let stub_rel = stdlib::stdlib_stub_path(path)?;
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let stub_abs = root.join(stub_rel);
    let uri = Url::from_file_path(stub_abs).ok()?;
    Some(Location {
        uri,
        range: Range {
            start: Position::new(0, 0),
            end: Position::new(0, 0),
        },
    })
}

fn find_stdlib_import_path(ast: &Program, offset: usize) -> Option<Vec<String>> {
    for decl in &ast.declarations {
        let Declaration::Import(import) = &decl.node else {
            continue;
        };
        if !(decl.span.start <= offset && offset < decl.span.end) {
            continue;
        }
        let segments = match &import.kind {
            crate::frontend::ast::ImportKind::Module(path) => &path.segments,
            crate::frontend::ast::ImportKind::From { module, .. } => &module.segments,
            _ => continue,
        };
        if segments.first().map(|s| s.as_str()) != Some(stdlib::STDLIB_ROOT) {
            continue;
        }
        return Some(segments.clone());
    }
    None
}

/// Find a decorator at `offset` and resolve it to its registry info.
///
/// Returns `(decorator_id, resolved_path_segments)` if a recognized decorator is found.
fn find_decorator_at_position(
    ast: &Program,
    offset: usize,
    aliases: &HashMap<String, Vec<String>>,
) -> Option<(decorators::DecoratorId, Vec<String>)> {
    let check_decorators = |decs: &[crate::frontend::ast::Spanned<crate::frontend::ast::Decorator>]| {
        for dec in decs {
            if !(dec.span.start <= offset && offset < dec.span.end) {
                continue;
            }
            let resolved = resolve_decorator_path(&dec.node, aliases);
            let id = decorators::from_segments(&resolved)?;
            return Some((id, resolved));
        }
        None
    };

    for decl in &ast.declarations {
        match &decl.node {
            Declaration::Model(m) => {
                if let Some(r) = check_decorators(&m.decorators) {
                    return Some(r);
                }
                for method in &m.methods {
                    if let Some(r) = check_decorators(&method.node.decorators) {
                        return Some(r);
                    }
                }
            }
            Declaration::Class(c) => {
                if let Some(r) = check_decorators(&c.decorators) {
                    return Some(r);
                }
                for method in &c.methods {
                    if let Some(r) = check_decorators(&method.node.decorators) {
                        return Some(r);
                    }
                }
            }
            Declaration::Trait(t) => {
                if let Some(r) = check_decorators(&t.decorators) {
                    return Some(r);
                }
                for method in &t.methods {
                    if let Some(r) = check_decorators(&method.node.decorators) {
                        return Some(r);
                    }
                }
            }
            Declaration::Enum(e) => {
                if let Some(r) = check_decorators(&e.decorators) {
                    return Some(r);
                }
            }
            Declaration::Function(f) => {
                if let Some(r) = check_decorators(&f.decorators) {
                    return Some(r);
                }
            }
            _ => {}
        }
    }
    None
}

fn push_completion(
    items: &mut Vec<CompletionItem>,
    seen: &mut HashSet<String>,
    label: &str,
    kind: CompletionItemKind,
    detail: Option<String>,
    sort_text: Option<String>,
) {
    if seen.insert(label.to_string()) {
        items.push(CompletionItem {
            label: label.to_string(),
            kind: Some(kind),
            detail,
            sort_text,
            ..Default::default()
        });
    }
}

fn collect_rust_origin_symbols(checker: &typechecker::TypeChecker) -> Vec<RustOriginSymbol> {
    checker
        .symbols
        .all_symbols()
        .iter()
        .filter_map(|sym| match &sym.kind {
            crate::frontend::symbols::SymbolKind::RustItem(info) => Some(RustOriginSymbol {
                local_name: sym.name.clone(),
                span: sym.span,
                info: info.clone(),
            }),
            _ => None,
        })
        .collect()
}

fn rust_symbol_at_offset(symbols: &[RustOriginSymbol], offset: usize) -> Option<&RustOriginSymbol> {
    symbols
        .iter()
        .find(|sym| sym.span.start <= offset && offset < sym.span.end)
}

fn rust_symbol_for_span(symbols: &[RustOriginSymbol], span: Span) -> Option<&RustOriginSymbol> {
    symbols.iter().find(|sym| {
        let overlaps_start = sym.span.start <= span.start && span.start < sym.span.end;
        let overlaps_reverse = span.start <= sym.span.start && sym.span.start < span.end;
        overlaps_start || overlaps_reverse
    })
}

fn compile_error_to_diagnostic_with_rust_context(
    error: &CompileError,
    source: &str,
    uri: &Url,
    rust_symbols: &[RustOriginSymbol],
) -> Diagnostic {
    let mut enriched = error.clone();
    if let Some(sym) = rust_symbol_for_span(rust_symbols, error.span) {
        let note = format!(
            "Rust origin: `{}` resolves to `rust::{}`",
            sym.local_name, sym.info.path
        );
        if !enriched.notes.iter().any(|n| n == &note) {
            enriched.notes.push(note);
        }
    }
    compile_error_to_diagnostic(&enriched, source, uri)
}

fn rust_item_kind_label(kind: &RustItemKind) -> &'static str {
    match kind {
        RustItemKind::Module(_) => "module",
        RustItemKind::Type(_) => "type",
        RustItemKind::Function(_) => "function",
        RustItemKind::Constant { .. } => "constant",
        RustItemKind::Trait(_) => "trait",
        RustItemKind::Unsupported { .. } => "unsupported item",
    }
}

fn rust_binding_kind_label(binding: crate::frontend::symbols::RustImportBindingKind) -> &'static str {
    match binding {
        crate::frontend::symbols::RustImportBindingKind::CrateRoot => "crate root import",
        crate::frontend::symbols::RustImportBindingKind::RootedPath => "path import",
        crate::frontend::symbols::RustImportBindingKind::FromImport => "from-import binding",
    }
}

fn completion_kind_for_module_child(kind: RustModuleChildKind) -> CompletionItemKind {
    match kind {
        RustModuleChildKind::Module => CompletionItemKind::MODULE,
        RustModuleChildKind::Type => CompletionItemKind::CLASS,
        RustModuleChildKind::Function => CompletionItemKind::FUNCTION,
        RustModuleChildKind::Constant => CompletionItemKind::CONSTANT,
        RustModuleChildKind::Trait => CompletionItemKind::INTERFACE,
        RustModuleChildKind::Other => CompletionItemKind::REFERENCE,
    }
}

fn rust_member_completion_context(line_prefix: &str) -> Option<(&str, &str)> {
    let trimmed = line_prefix.trim_end();
    let dot_idx = trimmed.rfind('.')?;
    let (base_part, partial_part) = trimmed.split_at(dot_idx);
    let partial = &partial_part[1..];
    if !partial.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return None;
    }
    let base = base_part
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .next_back()?;
    if base.is_empty() {
        return None;
    }
    Some((base, partial))
}

/// Return the LSP detail string for the built-in `list.repeat` helper.
fn builtin_list_repeat_detail() -> String {
    collection_helpers::signature(BuiltinCollectionHelperId::ListRepeat).to_string()
}

/// Return hover markdown for the built-in `list.repeat` helper.
fn builtin_list_repeat_markdown() -> String {
    format!(
        "```incan\n{}\n```\n\n*list helper*\n\nCreates a list with `count` clone-derived copies of `value`. Negative counts raise `ValueError`.",
        builtin_list_repeat_detail()
    )
}

/// Return hover markdown when `ident` is the `repeat` member in `list.repeat`.
fn builtin_list_repeat_hover(source: &str, ident: &str, span: Span) -> Option<String> {
    let helper = BuiltinCollectionHelperId::ListRepeat;
    if ident != collection_helpers::member(helper) {
        return None;
    }
    let prefix = &source[..span.start.min(source.len())];
    let receiver_prefix = format!("{}.", collection_helpers::receiver(helper));
    prefix
        .trim_end()
        .ends_with(&receiver_prefix)
        .then(builtin_list_repeat_markdown)
}

/// Return completions for built-in members on the import-free `list` surface.
fn builtin_list_member_completions(line_prefix: &str) -> Option<Vec<CompletionItem>> {
    let (base, partial) = rust_member_completion_context(line_prefix)?;
    if base != collection_helpers::receiver(BuiltinCollectionHelperId::ListRepeat) {
        return None;
    }

    let mut items = Vec::new();
    let mut seen = HashSet::new();
    for helper in collection_helpers::BUILTIN_COLLECTION_HELPERS
        .iter()
        .filter(|helper| helper.receiver == base && helper.member.starts_with(partial))
    {
        push_completion(
            &mut items,
            &mut seen,
            helper.member,
            CompletionItemKind::METHOD,
            Some(helper.signature.to_string()),
            Some(format!("0_{}", helper.member)),
        );
    }
    if items.is_empty() { None } else { Some(items) }
}

fn rust_member_completions(line_prefix: &str, symbols: &[RustOriginSymbol]) -> Option<Vec<CompletionItem>> {
    let (base, partial) = rust_member_completion_context(line_prefix)?;
    let rust_symbol = symbols.iter().find(|sym| sym.local_name == base)?;
    let metadata = rust_symbol.info.metadata.as_ref()?;
    let mut items = Vec::new();
    let mut seen = HashSet::new();

    match &metadata.kind {
        RustItemKind::Module(module) => {
            for child in &module.children {
                if !child.name.starts_with(partial) {
                    continue;
                }
                push_completion(
                    &mut items,
                    &mut seen,
                    &child.name,
                    completion_kind_for_module_child(child.kind_hint),
                    Some(format!(
                        "rust::{}::{} ({})",
                        rust_symbol.info.path, child.name, rust_symbol.local_name
                    )),
                    Some(format!("0_{}", child.name)),
                );
            }
        }
        RustItemKind::Type(type_info) => {
            for method in &type_info.methods {
                if !method.name.starts_with(partial) {
                    continue;
                }
                if typechecker::TypeChecker::rust_signature_has_receiver(&method.signature) {
                    continue;
                }
                push_completion(
                    &mut items,
                    &mut seen,
                    &method.name,
                    CompletionItemKind::FUNCTION,
                    Some(format!(
                        "rust::{}::{} (associated function)",
                        rust_symbol.info.path, method.name
                    )),
                    Some(format!("0_{}", method.name)),
                );
            }
        }
        RustItemKind::Trait(trait_info) => {
            for item in &trait_info.items {
                let (name, kind, detail) = match item {
                    RustTraitAssoc::Function { name, .. } => (
                        name.as_str(),
                        CompletionItemKind::FUNCTION,
                        format!("rust::{}::{} (trait function)", rust_symbol.info.path, name),
                    ),
                    RustTraitAssoc::TypeAlias { name } => (
                        name.as_str(),
                        CompletionItemKind::TYPE_PARAMETER,
                        format!("rust::{}::{} (trait type alias)", rust_symbol.info.path, name),
                    ),
                    RustTraitAssoc::Constant { name, .. } => (
                        name.as_str(),
                        CompletionItemKind::CONSTANT,
                        format!("rust::{}::{} (trait constant)", rust_symbol.info.path, name),
                    ),
                };
                if !name.starts_with(partial) {
                    continue;
                }
                push_completion(
                    &mut items,
                    &mut seen,
                    name,
                    kind,
                    Some(detail),
                    Some(format!("0_{}", name)),
                );
            }
        }
        _ => {}
    }

    if items.is_empty() { None } else { Some(items) }
}

fn lsp_symbol_kind_for_decl(decl: &Declaration) -> Option<SymbolKind> {
    match decl {
        Declaration::Const(_) | Declaration::Static(_) => Some(SymbolKind::CONSTANT),
        Declaration::Function(_) => Some(SymbolKind::FUNCTION),
        Declaration::Model(_) => Some(SymbolKind::STRUCT),
        Declaration::Class(_) => Some(SymbolKind::CLASS),
        Declaration::Trait(_) => Some(SymbolKind::INTERFACE),
        Declaration::Enum(_) => Some(SymbolKind::ENUM),
        Declaration::TypeAlias(_) => Some(SymbolKind::TYPE_PARAMETER),
        Declaration::Newtype(_) => Some(SymbolKind::STRUCT),
        _ => None,
    }
}

/// Build the display name and detail string used for one LSP document symbol entry.
fn lsp_document_symbol_name_and_detail(decl: &Declaration) -> Option<(String, String)> {
    match decl {
        Declaration::Const(konst) => Some((
            konst.name.clone(),
            if let Some(ty) = &konst.ty {
                format!("const {}: {}", konst.name, format_type(&ty.node))
            } else {
                format!("const {}", konst.name)
            },
        )),
        Declaration::Static(static_decl) => Some((
            static_decl.name.clone(),
            format!("static {}: {}", static_decl.name, format_type(&static_decl.ty.node)),
        )),
        Declaration::Function(func) => Some((func.name.clone(), format_function_signature(func))),
        Declaration::Model(model) => Some((model.name.clone(), format!("model {}", model.name))),
        Declaration::Class(class) => Some((class.name.clone(), format!("class {}", class.name))),
        Declaration::Trait(tr) => Some((tr.name.clone(), format!("trait {}", tr.name))),
        Declaration::Enum(en) => Some((en.name.clone(), enum_completion_detail(en))),
        Declaration::TypeAlias(alias) => Some((
            alias.name.clone(),
            format!("type {} = {}", alias.name, format_type(&alias.target.node)),
        )),
        Declaration::Newtype(nt) => {
            let kind = if nt.is_rusttype { "rusttype" } else { "newtype" };
            Some((
                nt.name.clone(),
                format!("{} {} = {}", kind, nt.name, format_type(&nt.underlying.node)),
            ))
        }
        _ => None,
    }
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

/// Parse model-emit command arguments from either one object argument or positional `[uriOrPath, model, format?]`.
fn parse_emit_contract_model_command_args(
    mut args: Vec<serde_json::Value>,
) -> std::result::Result<EmitContractModelCommandArgs, String> {
    if args.len() == 1 && args[0].is_object() {
        return serde_json::from_value(args.remove(0)).map_err(|error| error.to_string());
    }
    if args.len() < 2 {
        return Err(format!(
            "{EMIT_CONTRACT_MODEL_COMMAND} expects either an object argument or positional [uriOrPath, model, format?]"
        ));
    }
    let uri = args[0]
        .as_str()
        .ok_or_else(|| "first positional argument must be a URI or path string".to_string())?
        .to_string();
    let model = args[1]
        .as_str()
        .ok_or_else(|| "second positional argument must be a model name or stable model id".to_string())?
        .to_string();
    let format = args.get(2).and_then(|value| value.as_str()).map(str::to_string);
    Ok(EmitContractModelCommandArgs {
        uri: Some(uri),
        path: None,
        model,
        format,
    })
}

/// Convert a file URI or raw client path into a local filesystem path.
fn path_from_lsp_uri_or_path(value: &str) -> std::result::Result<PathBuf, String> {
    if let Ok(uri) = Url::parse(value)
        && uri.scheme() == "file"
    {
        return uri
            .to_file_path()
            .map_err(|_| format!("file URI `{value}` could not be converted to a local path"));
    }
    Ok(PathBuf::from(value))
}

/// Load model bundles for the LSP command from a project, source path, bundle JSON file, or library artifact.
fn collect_lsp_contract_model_bundles(path: &Path) -> std::result::Result<Vec<CanonicalModelBundle>, String> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|error| format!("failed to determine current directory: {error}"))?
            .join(path)
    };
    if absolute.is_file()
        && absolute
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension == "json")
    {
        return read_model_bundles_from_json(&absolute).map_err(|error| error.to_string());
    }
    if absolute.is_file()
        && absolute
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension == "incnlib")
    {
        let manifest =
            crate::library_manifest::LibraryManifest::read_from_path(&absolute).map_err(|error| error.to_string())?;
        let bundles = manifest.contract_metadata.models.model_bundles;
        if bundles.is_empty() {
            return Err(format!(
                "artifact {} does not carry checked model metadata",
                absolute.display()
            ));
        }
        return Ok(bundles);
    }

    let start_dir = if absolute.is_dir() {
        absolute.as_path()
    } else {
        absolute.parent().unwrap_or(Path::new("."))
    };
    let manifest = ProjectManifest::discover(start_dir)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| {
            format!(
                "model emit requires a project manifest, bundle JSON, or `.incnlib` artifact: {}",
                path.display()
            )
        })?;
    read_project_model_bundles(manifest.project_root(), &manifest.contract_model_bundle_paths())
        .map_err(|error| error.to_string())
}

/// Resolve a single LSP model-emit bundle by logical type name or stable model id.
fn find_lsp_contract_model_bundle(path: &Path, model: &str) -> std::result::Result<CanonicalModelBundle, String> {
    let bundles = collect_lsp_contract_model_bundles(path)?;
    bundles
        .into_iter()
        .find(|bundle| bundle.logical_type_name == model || bundle.stable_model_id.as_deref() == Some(model))
        .ok_or_else(|| {
            format!(
                "model `{model}` was not found in checked model metadata for {}",
                path.display()
            )
        })
}

/// Build the JSON-RPC response payload for model source or bundle JSON emission.
fn emit_contract_model_command_payload(
    path: &Path,
    model: &str,
    format: ContractModelCommandFormat,
) -> std::result::Result<serde_json::Value, String> {
    let bundle = find_lsp_contract_model_bundle(path, model)?;
    match format {
        ContractModelCommandFormat::Incan => {
            let source = bundle.emit_incan_model_source().map_err(|error| error.to_string())?;
            Ok(json!({
                "format": format.as_str(),
                "model": bundle.logical_type_name,
                "stableModelId": bundle.stable_model_id,
                "source": source
            }))
        }
        ContractModelCommandFormat::Json => Ok(json!({
            "format": format.as_str(),
            "model": bundle.logical_type_name,
            "stableModelId": bundle.stable_model_id,
            "bundle": bundle
        })),
    }
}

#[tower_lsp::async_trait]
impl LanguageServer for IncanLanguageServer {
    /// Advertise the LSP capabilities implemented by the Incan language server.
    async fn initialize(&self, _: InitializeParams) -> Result<InitializeResult> {
        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                // Real-time diagnostics via text sync
                text_document_sync: Some(TextDocumentSyncCapability::Kind(TextDocumentSyncKind::FULL)),
                // Hover support
                hover_provider: Some(HoverProviderCapability::Simple(true)),
                // Go-to-definition
                definition_provider: Some(OneOf::Left(true)),
                // Document symbols (outline)
                document_symbol_provider: Some(OneOf::Left(true)),
                // Completions (basic)
                completion_provider: Some(CompletionOptions {
                    trigger_characters: Some(vec![".".to_string(), ":".to_string(), "[".to_string()]),
                    ..Default::default()
                }),
                execute_command_provider: Some(ExecuteCommandOptions {
                    commands: vec![EMIT_CONTRACT_MODEL_COMMAND.to_string()],
                    ..Default::default()
                }),
                ..Default::default()
            },
            server_info: Some(ServerInfo {
                name: "incan-lsp".to_string(),
                version: Some(env!("CARGO_PKG_VERSION").to_string()),
            }),
        })
    }

    async fn initialized(&self, _: InitializedParams) {
        self.client
            .log_message(MessageType::INFO, "Incan LSP initialized")
            .await;
    }

    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }

    /// Handle editor commands that need checked metadata not otherwise exposed by standard LSP requests.
    async fn execute_command(&self, params: ExecuteCommandParams) -> Result<Option<serde_json::Value>> {
        if params.command != EMIT_CONTRACT_MODEL_COMMAND {
            self.client
                .log_message(
                    MessageType::WARNING,
                    format!("unsupported Incan LSP command `{}`", params.command),
                )
                .await;
            return Ok(Some(json!({
                "error": format!("unsupported command `{}`", params.command)
            })));
        }

        let parsed = match parse_emit_contract_model_command_args(params.arguments) {
            Ok(parsed) => parsed,
            Err(error) => {
                return Ok(Some(json!({ "error": error })));
            }
        };
        let Some(uri_or_path) = parsed.uri.as_deref().or(parsed.path.as_deref()) else {
            return Ok(Some(json!({
                "error": format!("{EMIT_CONTRACT_MODEL_COMMAND} requires `uri` or `path`")
            })));
        };
        let path = match path_from_lsp_uri_or_path(uri_or_path) {
            Ok(path) => path,
            Err(error) => return Ok(Some(json!({ "error": error }))),
        };
        let format = match ContractModelCommandFormat::parse(parsed.format.as_deref()) {
            Ok(format) => format,
            Err(error) => return Ok(Some(json!({ "error": error }))),
        };
        match emit_contract_model_command_payload(&path, &parsed.model, format) {
            Ok(payload) => Ok(Some(payload)),
            Err(error) => Ok(Some(json!({ "error": error }))),
        }
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let uri = params.text_document.uri;
        let source = params.text_document.text;
        let version = params.text_document.version;

        self.analyze_document(&uri, &source, version).await;
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let uri = params.text_document.uri;
        let version = params.text_document.version;

        // We use FULL sync, so there's only one change with the full content
        if let Some(change) = params.content_changes.into_iter().next() {
            self.analyze_document(&uri, &change.text, version).await;
        }
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        let uri = params.text_document.uri;

        // Remove document from cache
        let mut docs = self.documents.write().await;
        docs.remove(&uri);

        // Clear diagnostics
        self.client.publish_diagnostics(uri, vec![], None).await;
    }

    /// Provide hover text for symbols, call-site type arguments, Rust imports, and contextual receiver bindings.
    async fn hover(&self, params: HoverParams) -> Result<Option<Hover>> {
        let uri = &params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;

        let docs = self.documents.read().await;
        let doc = match docs.get(uri) {
            Some(doc) => doc,
            None => return Ok(None),
        };

        let ast = match &doc.ast {
            Some(ast) => ast,
            None => return Ok(None),
        };

        if let Some(offset) = position_to_offset(&doc.source, position) {
            let aliases = collect_import_aliases(ast);

            // Decorator hover: show decorator name + description from registry
            if let Some((id, resolved)) = find_decorator_at_position(ast, offset, &aliases) {
                let info = decorators::info_for(id);
                let canonical = info.canonical;
                let description = info.description;
                // If the decorator's owning module has a stdlib stub, show the path
                let module_path: Vec<String> = resolved[..resolved.len().saturating_sub(1)].to_vec();
                let stub_note = stdlib::stdlib_stub_path(&module_path)
                    .map(|p| format!("\n\n`{}`", p))
                    .unwrap_or_default();
                let markdown = format!("```incan\n@{}\n```\n\n{}{}", canonical, description, stub_note);
                return Ok(Some(Hover {
                    contents: HoverContents::Markup(MarkupContent {
                        kind: MarkupKind::Markdown,
                        value: markdown,
                    }),
                    range: None,
                }));
            }

            // Stdlib import hover: show module path + stub path
            if let Some(path) = find_stdlib_import_path(ast, offset) {
                let module_path = path.join(".");
                let stub_path = stdlib::stdlib_stub_path(&path).unwrap_or_default();
                let markdown = if stub_path.is_empty() {
                    format!("```incan\n{}\n```\n\n*stdlib module*", module_path)
                } else {
                    format!("```incan\n{}\n```\n\n*stdlib module*\n\n`{}`", module_path, stub_path)
                };
                return Ok(Some(Hover {
                    contents: HoverContents::Markup(MarkupContent {
                        kind: MarkupKind::Markdown,
                        value: markdown,
                    }),
                    range: None,
                }));
            }

            if let Some(rust_symbol) = rust_symbol_at_offset(&doc.rust_origin_symbols, offset) {
                let mut markdown = format!(
                    "```incan\n{}\n```\n\n*rust import* (`{}`)\n\n`rust::{}`",
                    rust_symbol.local_name,
                    rust_binding_kind_label(rust_symbol.info.binding),
                    rust_symbol.info.path
                );
                if let Some(metadata) = &rust_symbol.info.metadata {
                    markdown.push_str(&format!(
                        "\n\nresolved kind: `{}`",
                        rust_item_kind_label(&metadata.kind)
                    ));
                }
                return Ok(Some(Hover {
                    contents: HoverContents::Markup(MarkupContent {
                        kind: MarkupKind::Markdown,
                        value: markdown,
                    }),
                    range: Some(span_to_range(&doc.source, rust_symbol.span.start, rust_symbol.span.end)),
                }));
            }

            if let Some((ident, span)) = identifier_at_offset(&doc.source, offset)
                && keywords::from_str(ident.as_str()) == Some(keywords::KeywordId::Cls)
                && let Some(context) = classmethod_context_at_offset(ast, offset, &aliases)
            {
                let detail = classmethod_cls_detail(&context);
                let markdown = format!(
                    "```incan\n{detail}\n```\n\n*classmethod receiver* — callable constructor for `{}`.",
                    context.owner_type
                );
                return Ok(Some(Hover {
                    contents: HoverContents::Markup(MarkupContent {
                        kind: MarkupKind::Markdown,
                        value: markdown,
                    }),
                    range: Some(span_to_range(&doc.source, span.start, span.end)),
                }));
            }

            if let Some((ident, span)) = identifier_at_offset(&doc.source, offset)
                && let Some(markdown) = builtin_list_repeat_hover(&doc.source, &ident, span)
            {
                return Ok(Some(Hover {
                    contents: HoverContents::Markup(MarkupContent {
                        kind: MarkupKind::Markdown,
                        value: markdown,
                    }),
                    range: Some(span_to_range(&doc.source, span.start, span.end)),
                }));
            }

            // Call-site explicit type arguments: `f[T](...)`, `_.method[U](...)`
            if let Some(ty_spanned) = call_site_type_args::call_site_innermost_type_at_offset(ast, offset) {
                let display = format_type(&ty_spanned.node);
                let markdown = match &ty_spanned.node {
                    Type::Infer => "```incan\n_\n```\n\n*Call-site inference placeholder* — this type parameter is filled from the value arguments (RFC 054)."
                        .to_string(),
                    Type::Simple(name) => {
                        let mut md = format!("```incan\n{display}\n```");
                        if self.find_definition(ast, name).is_some() {
                            md.push_str("\n\n*Type argument* — local declaration; use go-to-definition for its source.");
                        } else {
                            md.push_str("\n\n*Type argument* — builtin or unqualified type name.");
                        }
                        md
                    }
                    _ => format!("```incan\n{display}\n```\n\n*Type argument* at call site (RFC 054)."),
                };
                return Ok(Some(Hover {
                    contents: HoverContents::Markup(MarkupContent {
                        kind: MarkupKind::Markdown,
                        value: markdown,
                    }),
                    range: Some(span_to_range(&doc.source, ty_spanned.span.start, ty_spanned.span.end)),
                }));
            }

            if let Some(preview) = api_metadata_preview_at_offset(&doc.api_metadata_previews, offset) {
                return Ok(Some(Hover {
                    contents: HoverContents::Markup(MarkupContent {
                        kind: MarkupKind::Markdown,
                        value: preview.markdown.clone(),
                    }),
                    range: Some(span_to_range(&doc.source, preview.span.start, preview.span.end)),
                }));
            }
        }

        if let Some(info) = self.find_symbol_at_position(ast, &doc.source, position) {
            let detail = if info.kind == "const" {
                if let Some(resolved) = doc.const_types.get(&info.name) {
                    format!("const {}: {}", info.name, resolved)
                } else {
                    info.detail.clone()
                }
            } else {
                info.detail.clone()
            };

            let mut markdown = format!("```incan\n{}\n```\n\n*{}*", detail, info.kind);

            if info.kind == "rusttype"
                && let Some(rust_path) = doc.rusttype_info.get(&info.name)
            {
                markdown.push_str(&format!("\n\nunderlying Rust type: `rust::{}`", rust_path));
            }

            return Ok(Some(Hover {
                contents: HoverContents::Markup(MarkupContent {
                    kind: MarkupKind::Markdown,
                    value: markdown,
                }),
                range: Some(span_to_range(&doc.source, info.span.start, info.span.end)),
            }));
        }

        Ok(None)
    }

    async fn document_symbol(&self, params: DocumentSymbolParams) -> Result<Option<DocumentSymbolResponse>> {
        let uri = &params.text_document.uri;
        let docs = self.documents.read().await;
        let doc = match docs.get(uri) {
            Some(doc) => doc,
            None => return Ok(None),
        };
        let ast = match &doc.ast {
            Some(ast) => ast,
            None => return Ok(None),
        };

        let mut symbols = Vec::new();
        for decl in &ast.declarations {
            let Some(kind) = lsp_symbol_kind_for_decl(&decl.node) else {
                continue;
            };
            let Some((name, detail)) = lsp_document_symbol_name_and_detail(&decl.node) else {
                continue;
            };
            let range = span_to_range(&doc.source, decl.span.start, decl.span.end);
            #[allow(deprecated)]
            let symbol = DocumentSymbol {
                name,
                detail: Some(detail),
                kind,
                tags: None,
                deprecated: None,
                range,
                selection_range: range,
                children: None,
            };
            symbols.push(symbol);
        }

        Ok(Some(DocumentSymbolResponse::Nested(symbols)))
    }

    async fn goto_definition(&self, params: GotoDefinitionParams) -> Result<Option<GotoDefinitionResponse>> {
        let uri = &params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;

        let docs = self.documents.read().await;
        let doc = match docs.get(uri) {
            Some(doc) => doc,
            None => return Ok(None),
        };

        let ast = match &doc.ast {
            Some(ast) => ast,
            None => return Ok(None),
        };

        let Some(offset) = position_to_offset(&doc.source, position) else {
            return Ok(None);
        };
        if let Some(path) = find_stdlib_import_path(ast, offset)
            && let Some(location) = stdlib_location_for_path(&path)
        {
            return Ok(Some(GotoDefinitionResponse::Scalar(location)));
        }
        let aliases = collect_import_aliases(ast);
        // Decorator go-to-definition: navigate to the owning module's stdlib stub (if any)
        if let Some((_id, resolved)) = find_decorator_at_position(ast, offset, &aliases) {
            let module_path: Vec<String> = resolved[..resolved.len().saturating_sub(1)].to_vec();
            if let Some(location) = stdlib_location_for_path(&module_path) {
                return Ok(Some(GotoDefinitionResponse::Scalar(location)));
            }
        }

        // Find what symbol the cursor is on
        if let Some(info) = self.find_symbol_at_position(ast, &doc.source, position) {
            // Find definition of that symbol
            if let Some(def_span) = self.find_definition(ast, &info.name) {
                let range = span_to_range(&doc.source, def_span.start, def_span.end);
                return Ok(Some(GotoDefinitionResponse::Scalar(Location {
                    uri: uri.clone(),
                    range,
                })));
            }
        }

        Ok(None)
    }

    /// Provide context-aware completions for decorators, imports, type arguments, symbols, and receiver bindings.
    async fn completion(&self, params: CompletionParams) -> Result<Option<CompletionResponse>> {
        let uri = &params.text_document_position.text_document.uri;
        let position = params.text_document_position.position;

        let docs = self.documents.read().await;
        let doc = match docs.get(uri) {
            Some(doc) => doc,
            None => return Ok(None),
        };

        let mut items: Vec<CompletionItem> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();

        // Extract the current line text before the cursor for context-aware completions.
        let line_prefix = line_text_before_cursor(&doc.source, position);

        // ---- Context: stdlib module completions (`from std.` / `import std::`) ----
        if let Some(stdlib_items) = stdlib_module_completions(&line_prefix) {
            return Ok(Some(CompletionResponse::Array(stdlib_items)));
        }

        // ---- Context: decorator completions (`@` at line start) ----
        if let Some(decorator_items) = decorator_completions(&line_prefix) {
            return Ok(Some(CompletionResponse::Array(decorator_items)));
        }

        // ---- Context: built-in collection member completions (`list.<member>`) ----
        if let Some(list_member_items) = builtin_list_member_completions(&line_prefix) {
            return Ok(Some(CompletionResponse::Array(list_member_items)));
        }

        // ---- Context: Rust-origin member completions (`Alias.<member>`) ----
        if let Some(rust_member_items) = rust_member_completions(&line_prefix, &doc.rust_origin_symbols) {
            return Ok(Some(CompletionResponse::Array(rust_member_items)));
        }

        // ---- Context: call-site type arguments (`callee[T](...)`, `recv.m[U](...)`, including `_`) ----
        if let Some(off) = position_to_offset(&doc.source, position)
            && call_site_type_args::offset_in_call_site_type_argument_list(&doc.source, off)
        {
            let items = call_site_type_args::call_site_type_argument_completion_items(doc.ast.as_ref());
            return Ok(Some(CompletionResponse::Array(items)));
        }

        // ---- General completions (not in a specific context) ----

        if let Some(ast) = &doc.ast {
            let aliases = collect_import_aliases(ast);
            if let Some(off) = position_to_offset(&doc.source, position)
                && let Some(context) = classmethod_context_at_offset(ast, off, &aliases)
            {
                push_completion(
                    &mut items,
                    &mut seen,
                    keywords::as_str(keywords::KeywordId::Cls),
                    CompletionItemKind::VARIABLE,
                    Some(format!(
                        "{} — classmethod receiver constructor",
                        classmethod_cls_detail(&context)
                    )),
                    Some("0_cls".to_string()),
                );
            }
        }

        // Add keywords from the registry (canonical + aliases).
        for info in keywords::KEYWORDS {
            push_completion(
                &mut items,
                &mut seen,
                info.canonical,
                CompletionItemKind::KEYWORD,
                None,
                None,
            );
            for &alias in info.aliases {
                push_completion(&mut items, &mut seen, alias, CompletionItemKind::KEYWORD, None, None);
            }
        }

        // Add surface constructors (`Ok`, `Err`, `Some`, `None`).
        for info in constructors::CONSTRUCTORS {
            push_completion(
                &mut items,
                &mut seen,
                info.canonical,
                CompletionItemKind::CONSTRUCTOR,
                None,
                None,
            );
            for &alias in info.aliases {
                push_completion(
                    &mut items,
                    &mut seen,
                    alias,
                    CompletionItemKind::CONSTRUCTOR,
                    None,
                    None,
                );
            }
        }

        // Add core collection/generic type names (`Option`, `Result`, frozen variants, etc.).
        for info in collections::COLLECTION_TYPES {
            push_completion(
                &mut items,
                &mut seen,
                info.canonical,
                CompletionItemKind::CLASS,
                None,
                None,
            );
            for &alias in info.aliases {
                push_completion(&mut items, &mut seen, alias, CompletionItemKind::CLASS, None, None);
            }
        }

        // Add symbols from the current document
        if let Some(ast) = &doc.ast {
            for decl in &ast.declarations {
                match &decl.node {
                    Declaration::Const(konst) => {
                        push_completion(
                            &mut items,
                            &mut seen,
                            &konst.name,
                            CompletionItemKind::CONSTANT,
                            Some(if let Some(ty) = &konst.ty {
                                format!("const {}: {}", konst.name, format_type(&ty.node))
                            } else {
                                format!("const {}", konst.name)
                            }),
                            None,
                        );
                    }
                    Declaration::Static(static_decl) => {
                        push_completion(
                            &mut items,
                            &mut seen,
                            &static_decl.name,
                            CompletionItemKind::CONSTANT,
                            Some(format!(
                                "static {}: {}",
                                static_decl.name,
                                format_type(&static_decl.ty.node)
                            )),
                            None,
                        );
                    }
                    Declaration::Function(func) => {
                        push_completion(
                            &mut items,
                            &mut seen,
                            &func.name,
                            CompletionItemKind::FUNCTION,
                            Some(format_function_signature(func)),
                            None,
                        );
                    }
                    Declaration::Model(model) => {
                        push_completion(
                            &mut items,
                            &mut seen,
                            &model.name,
                            CompletionItemKind::STRUCT,
                            Some(format!("model {}", model.name)),
                            None,
                        );
                        for field in &model.fields {
                            let canonical = field.node.name.as_str();
                            push_completion(
                                &mut items,
                                &mut seen,
                                canonical,
                                CompletionItemKind::FIELD,
                                Some(format!("field on model {}", model.name)),
                                Some(format!("1_{}", canonical)),
                            );
                            if let Some(alias) = field.node.metadata.alias.as_deref()
                                && alias != canonical
                            {
                                // RFC 021: show mapping detail (e.g. `type → type_`)
                                push_completion(
                                    &mut items,
                                    &mut seen,
                                    alias,
                                    CompletionItemKind::FIELD,
                                    Some(format!("{} → {} ({})", alias, canonical, model.name)),
                                    Some(format!("0_{}", alias)),
                                );
                            }
                        }
                        for property in &model.properties {
                            push_completion(
                                &mut items,
                                &mut seen,
                                &property.node.name,
                                CompletionItemKind::FIELD,
                                Some(format_property_signature(&model.name, &property.node)),
                                Some(format!("1_{}", property.node.name)),
                            );
                        }
                    }
                    Declaration::Class(class) => {
                        push_completion(
                            &mut items,
                            &mut seen,
                            &class.name,
                            CompletionItemKind::CLASS,
                            Some(format!("class {}", class.name)),
                            None,
                        );
                        for field in &class.fields {
                            let canonical = field.node.name.as_str();
                            push_completion(
                                &mut items,
                                &mut seen,
                                canonical,
                                CompletionItemKind::FIELD,
                                Some(format!("field on class {}", class.name)),
                                Some(format!("1_{}", canonical)),
                            );
                            if let Some(alias) = field.node.metadata.alias.as_deref()
                                && alias != canonical
                            {
                                // RFC 021: show mapping detail (e.g. `type → type_`)
                                push_completion(
                                    &mut items,
                                    &mut seen,
                                    alias,
                                    CompletionItemKind::FIELD,
                                    Some(format!("{} → {} ({})", alias, canonical, class.name)),
                                    Some(format!("0_{}", alias)),
                                );
                            }
                        }
                        for property in &class.properties {
                            push_completion(
                                &mut items,
                                &mut seen,
                                &property.node.name,
                                CompletionItemKind::FIELD,
                                Some(format_property_signature(&class.name, &property.node)),
                                Some(format!("1_{}", property.node.name)),
                            );
                        }
                    }
                    Declaration::Trait(tr) => {
                        push_completion(
                            &mut items,
                            &mut seen,
                            &tr.name,
                            CompletionItemKind::INTERFACE,
                            Some(format!("trait {}", tr.name)),
                            None,
                        );
                        for property in &tr.properties {
                            push_completion(
                                &mut items,
                                &mut seen,
                                &property.node.name,
                                CompletionItemKind::FIELD,
                                Some(format_property_signature(&tr.name, &property.node)),
                                Some(format!("1_{}", property.node.name)),
                            );
                        }
                    }
                    Declaration::Enum(en) => {
                        push_completion(
                            &mut items,
                            &mut seen,
                            &en.name,
                            CompletionItemKind::ENUM,
                            Some(enum_completion_detail(en)),
                            None,
                        );
                        for variant in &en.variants {
                            let label = enum_variant_completion_label(en, &variant.node);
                            push_completion(
                                &mut items,
                                &mut seen,
                                &label,
                                CompletionItemKind::CONSTRUCTOR,
                                Some(enum_variant_completion_detail(en, &variant.node)),
                                Some(format!("1_{}", label)),
                            );
                        }
                    }
                    Declaration::TypeAlias(alias) => {
                        push_completion(
                            &mut items,
                            &mut seen,
                            &alias.name,
                            CompletionItemKind::TYPE_PARAMETER,
                            Some(format!("type {} = {}", alias.name, format_type(&alias.target.node))),
                            None,
                        );
                    }
                    Declaration::Newtype(nt) => {
                        let kind = if nt.is_rusttype { "rusttype" } else { "newtype" };
                        push_completion(
                            &mut items,
                            &mut seen,
                            &nt.name,
                            CompletionItemKind::STRUCT,
                            Some(format!("{} {} = {}", kind, nt.name, format_type(&nt.underlying.node))),
                            None,
                        );
                    }
                    _ => {}
                }
            }
        }

        // Add local rust-import bindings with canonical-path details.
        for rust_symbol in &doc.rust_origin_symbols {
            let (kind, detail) = if let Some(metadata) = &rust_symbol.info.metadata {
                let item_kind = rust_item_kind_label(&metadata.kind);
                (
                    match &metadata.kind {
                        RustItemKind::Module(_) => CompletionItemKind::MODULE,
                        RustItemKind::Type(_) => CompletionItemKind::CLASS,
                        RustItemKind::Function(_) => CompletionItemKind::FUNCTION,
                        RustItemKind::Constant { .. } => CompletionItemKind::CONSTANT,
                        RustItemKind::Trait(_) => CompletionItemKind::INTERFACE,
                        RustItemKind::Unsupported { .. } => CompletionItemKind::REFERENCE,
                    },
                    format!("rust::{} ({item_kind})", rust_symbol.info.path),
                )
            } else {
                (
                    CompletionItemKind::REFERENCE,
                    format!("rust::{} (metadata unavailable)", rust_symbol.info.path),
                )
            };
            push_completion(
                &mut items,
                &mut seen,
                &rust_symbol.local_name,
                kind,
                Some(detail),
                Some(format!("0_{}", rust_symbol.local_name)),
            );
        }

        // Add `std` as a module name so import completions can start from it.
        push_completion(
            &mut items,
            &mut seen,
            "std",
            CompletionItemKind::MODULE,
            Some("Incan standard library namespace".to_string()),
            None,
        );

        Ok(Some(CompletionResponse::Array(items)))
    }
}

/// Extract the text of the current line up to (but not including) the cursor position.
fn line_text_before_cursor(source: &str, position: Position) -> String {
    let lines: Vec<&str> = source.lines().collect();
    let line_idx = position.line as usize;
    if line_idx >= lines.len() {
        return String::new();
    }
    let line = lines[line_idx];
    let col = (position.character as usize).min(line.len());
    line[..col].to_string()
}

/// If the cursor is inside a `from std.<...>` or `import std::<...>` context,
/// return completions for stdlib module names. Returns `None` if not in that context.
fn stdlib_module_completions(line_prefix: &str) -> Option<Vec<CompletionItem>> {
    let trimmed = line_prefix.trim_start();

    // Detect `from std.X.` or `from std.` patterns
    let after_std = trimmed
        .strip_prefix("from std.")
        .or_else(|| trimmed.strip_prefix("import std::"))
        .or_else(|| trimmed.strip_prefix("import std."))?;

    // Split what the user has typed after `std.` to determine depth.
    // e.g. "serde." → ["serde", ""] → user wants children of std.serde
    // e.g. "" → user wants top-level std.* modules
    // e.g. "web import " → user has completed the module path, don't intercept
    let parts: Vec<&str> = after_std.split(['.', ':']).collect();

    // If we see " import " in the remainder, the user is selecting items from the module — bail.
    if after_std.contains(" import ") {
        return None;
    }

    // Determine the prefix segments the user has already typed.
    // The last element is the partial text being typed (could be empty after a dot).
    let (completed, _partial) = if parts.is_empty() {
        (vec![], "")
    } else {
        let last = parts.last().unwrap_or(&"");
        let completed: Vec<&str> = parts[..parts.len() - 1]
            .iter()
            .copied()
            .filter(|s| !s.is_empty())
            .collect();
        (completed, *last)
    };

    let mut items = Vec::new();
    let mut seen = HashSet::new();

    if completed.is_empty() {
        // Top-level: suggest namespace names (web, testing, async, ...)
        for ns in stdlib::STDLIB_NAMESPACES {
            if seen.insert(ns.name.to_string()) {
                let detail = ns.feature.map(|f| format!("enables {} feature", f));
                items.push(CompletionItem {
                    label: ns.name.to_string(),
                    kind: Some(CompletionItemKind::MODULE),
                    detail: Some(detail.unwrap_or_else(|| format!("std.{} module", ns.name))),
                    sort_text: Some(format!("0_{}", ns.name)),
                    ..Default::default()
                });
            }
        }
    } else if completed.len() == 1 {
        // One level deep: suggest submodules of the namespace (e.g. std.async.time)
        if let Some(ns) = stdlib::find_namespace(completed[0]) {
            for sub in ns.submodules {
                if seen.insert(sub.to_string()) {
                    items.push(CompletionItem {
                        label: sub.to_string(),
                        kind: Some(CompletionItemKind::MODULE),
                        detail: Some(format!("std.{}.{} module", ns.name, sub)),
                        sort_text: Some(format!("0_{}", sub)),
                        ..Default::default()
                    });
                }
            }
        }
    }

    if items.is_empty() {
        return None;
    }

    Some(items)
}

/// If the cursor is on a decorator line (starts with `@`), return completions for known decorator names.
/// Returns `None` if not in a decorator context.
fn decorator_completions(line_prefix: &str) -> Option<Vec<CompletionItem>> {
    let trimmed = line_prefix.trim_start();
    if !trimmed.starts_with('@') {
        return None;
    }

    let mut items = Vec::new();
    for info in decorators::DECORATORS {
        // Show the short name (after the last dot) as the label, with the full canonical path as detail.
        let short_name = info.canonical.rsplit('.').next().unwrap_or(info.canonical);
        items.push(CompletionItem {
            label: short_name.to_string(),
            kind: Some(CompletionItemKind::FUNCTION),
            detail: Some(format!("@{} — {}", info.canonical, info.description)),
            insert_text: Some(short_name.to_string()),
            sort_text: Some(format!("0_{}", short_name)),
            ..Default::default()
        });
        // Also offer the full canonical path (e.g. `std.web.route`)
        if info.canonical.contains('.') {
            items.push(CompletionItem {
                label: info.canonical.to_string(),
                kind: Some(CompletionItemKind::FUNCTION),
                detail: Some(info.description.to_string()),
                insert_text: Some(info.canonical.to_string()),
                sort_text: Some(format!("1_{}", info.canonical)),
                ..Default::default()
            });
        }
    }

    if items.is_empty() {
        return None;
    }

    Some(items)
}

#[cfg(test)]
mod completion_tests {
    use super::{builtin_list_member_completions, builtin_list_repeat_hover, stdlib_module_completions};
    use crate::frontend::ast::Span;

    #[test]
    fn stdlib_module_completions_include_std_fs() -> Result<(), String> {
        let items = stdlib_module_completions("from std.")
            .ok_or_else(|| "expected stdlib completions for `from std.`".to_string())?;
        assert!(
            items
                .iter()
                .any(|item| item.label == "fs" && item.detail.as_deref() == Some("std.fs module")),
            "expected std.fs to be exposed through stdlib registry completions: {items:?}"
        );
        Ok(())
    }

    #[test]
    fn builtin_list_member_completions_include_repeat() -> Result<(), String> {
        let items = builtin_list_member_completions("let xs = list.re")
            .ok_or_else(|| "expected built-in list member completions".to_string())?;
        assert!(
            items.iter().any(|item| {
                item.label == "repeat"
                    && item.detail.as_deref() == Some("list.repeat[T](value: T, count: int) -> list[T]")
            }),
            "expected list.repeat completion: {items:?}"
        );
        Ok(())
    }

    #[test]
    fn builtin_list_repeat_hover_documents_helper() -> Result<(), String> {
        let source = "let xs = list.repeat(0, 3)";
        let start = source
            .find("repeat")
            .ok_or_else(|| "expected repeat in fixture".to_string())?;
        let markdown = builtin_list_repeat_hover(source, "repeat", Span::new(start, start + "repeat".len()))
            .ok_or_else(|| "expected list.repeat hover".to_string())?;
        assert!(
            markdown.contains("list.repeat[T](value: T, count: int) -> list[T]"),
            "expected signature in hover markdown: {markdown}"
        );
        assert!(
            markdown.contains("Negative counts raise `ValueError`."),
            "expected negative-count detail in hover markdown: {markdown}"
        );
        Ok(())
    }
}
