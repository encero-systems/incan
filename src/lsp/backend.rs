//! LSP (Language Server Protocol) backend implementation for Incan

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;

use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer};

use crate::frontend::ast::{Declaration, Program, Span, Type};
use crate::frontend::module::resolve_import_path;
use crate::frontend::{lexer, parser, typechecker};
use crate::lsp::diagnostics::{compile_error_to_diagnostic, position_to_offset, span_to_range};
use crate::manifest::ProjectManifest;
use incan_core::lang::decorators;
use incan_core::lang::keywords;
use incan_core::lang::stdlib;
use incan_core::lang::surface::constructors;
use incan_core::lang::types::collections;

/// Document state stored by the LSP
#[derive(Debug, Clone)]
pub struct DocumentState {
    pub source: String,
    pub ast: Option<Program>,
    pub version: i32,
    /// Resolved const types from the typechecker (post “const-freezing”).
    ///
    /// This is used to make hover text reflect the actual type of a const binding, even if the
    /// user annotated `str`/`List[T]` and the compiler froze it to `FrozenStr`/`FrozenList[T]`.
    pub const_types: HashMap<String, String>,
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

        // Step 1: Lex
        let tokens = match lexer::lex(source) {
            Ok(tokens) => tokens,
            Err(errors) => {
                // Convert all lexer errors to diagnostics
                for error in &errors {
                    diagnostics.push(compile_error_to_diagnostic(error, source, uri));
                }
                self.client
                    .publish_diagnostics(uri.clone(), diagnostics, Some(version))
                    .await;
                return;
            }
        };

        // Step 2: Parse
        let ast = match parser::parse(&tokens) {
            Ok(ast) => {
                // Forward non-fatal parser warnings (e.g. RFC 005 dot-notation nudges) to the LSP.
                for warn in &ast.warnings {
                    diagnostics.push(compile_error_to_diagnostic(warn, source, uri));
                }
                ast
            }
            Err(errors) => {
                // Convert all parse errors to diagnostics
                for error in &errors {
                    diagnostics.push(compile_error_to_diagnostic(error, source, uri));
                }
                self.client
                    .publish_diagnostics(uri.clone(), diagnostics, Some(version))
                    .await;
                return;
            }
        };

        // Step 3: Type check (with multi-file import resolution)
        let mut checker = typechecker::TypeChecker::new();
        // RFC 023: if a project manifest exists, use it to validate `rust.module()` crate segments.
        if let Ok(entry_path) = uri.to_file_path()
            && let Some(start_dir) = entry_path.parent()
            && let Ok(manifest) = ProjectManifest::discover(start_dir)
            && let Some(m) = manifest
        {
            checker.set_declared_crate_names(m.declared_crate_names());
        }
        let (deps, mut dep_summary_diags) = self.collect_dependency_modules(uri, &ast, source, version).await;
        let dep_refs: Vec<(&str, &Program)> = deps.iter().map(|(name, program)| (name.as_str(), program)).collect();

        if let Err(errors) = checker.check_with_imports(&ast, &dep_refs) {
            for error in &errors {
                diagnostics.push(compile_error_to_diagnostic(error, source, uri));
            }
        }
        // Always include non-fatal diagnostics (warnings/lints) in LSP output.
        for warn in checker.warnings() {
            diagnostics.push(compile_error_to_diagnostic(warn, source, uri));
        }
        diagnostics.append(&mut dep_summary_diags);

        // Collect resolved const types for hover display (post-const-freezing).
        let mut const_types: HashMap<String, String> = HashMap::new();
        for decl in &ast.declarations {
            if let Declaration::Const(konst) = &decl.node
                && let Some(id) = checker.symbols.lookup(&konst.name)
                && let Some(sym) = checker.symbols.get(id)
                && let crate::frontend::symbols::SymbolKind::Variable(var) = &sym.kind
            {
                const_types.insert(konst.name.clone(), var.ty.to_string());
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
        _entry_version: i32,
    ) -> (Vec<(String, Program)>, Vec<Diagnostic>) {
        let Ok(entry_path) = uri.to_file_path() else {
            return (Vec::new(), Vec::new());
        };
        let entry_base = entry_path.parent().unwrap_or(Path::new(".")).to_path_buf();

        let docs = self.documents.read().await;

        let mut result: Vec<(String, Program)> = Vec::new();
        let mut entry_diags: Vec<Diagnostic> = Vec::new();
        let mut seen: HashSet<PathBuf> = HashSet::new();
        let mut stack: Vec<(PathBuf, PathBuf, Span)> = Vec::new(); // (module_path, base_dir_for_that_module, import_span_in_entry)

        // Seed stack with direct imports from the entry AST
        for decl in &ast.declarations {
            if let Declaration::Import(import) = &decl.node
                && let Some(dep_path) = resolve_import_path(&entry_base, import)
            {
                let base = dep_path.parent().unwrap_or(&entry_base).to_path_buf();
                stack.push((dep_path, base, decl.span));
            }
        }

        while let Some((path, base_dir, import_span)) = stack.pop() {
            let canonical = path.canonicalize().unwrap_or(path.clone());
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

            let dep_tokens = match lexer::lex(&dep_source) {
                Ok(t) => t,
                Err(errors) => {
                    // Guardrail: surface dependency lex errors.
                    if let Some(u) = dep_uri.clone() {
                        let mut diags = Vec::new();
                        for e in &errors {
                            diags.push(compile_error_to_diagnostic(e, &dep_source, &u));
                        }
                        let ver = dep_doc.map(|d| d.version);
                        self.client.publish_diagnostics(u.clone(), diags, ver).await;
                    }

                    // Summarize in the entry file.
                    let range = span_to_range(entry_source, import_span.start, import_span.end);
                    entry_diags.push(Diagnostic {
                        range,
                        severity: Some(DiagnosticSeverity::ERROR),
                        code: None,
                        code_description: None,
                        source: Some("incan".to_string()),
                        message: format!(
                            "Failed to lex dependency '{}'; open that file for details",
                            canonical.display()
                        ),
                        related_information: None,
                        tags: None,
                        data: None,
                    });
                    continue;
                }
            };
            let dep_ast = match parser::parse(&dep_tokens) {
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
                            "Failed to parse dependency '{}'; open that file for details",
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
            for decl in &dep_ast.declarations {
                if let Declaration::Import(import) = &decl.node
                    && let Some(nested_path) = resolve_import_path(&base_dir, import)
                {
                    let nested_base = nested_path.parent().unwrap_or(&base_dir).to_path_buf();
                    stack.push((nested_path, nested_base, Span::default()));
                }
            }

            let module_name = canonical
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("module")
                .to_string();
            result.push((module_name, dep_ast));
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

    fn find_in_declaration(&self, decl: &Declaration, span: Span, offset: usize) -> Option<SymbolInfo> {
        match decl {
            Declaration::Const(konst) => {
                if span.start <= offset && offset < span.end {
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
            }
            Declaration::Function(func) => {
                if span.start <= offset && offset < span.end {
                    // Check if cursor is on function name. For now, return the function signature
                    return Some(SymbolInfo {
                        name: func.name.clone(),
                        kind: "function".to_string(),
                        detail: format_function_signature(func),
                        span,
                    });
                }
            }
            Declaration::Model(model) => {
                if span.start <= offset && offset < span.end {
                    return Some(SymbolInfo {
                        name: model.name.clone(),
                        kind: "model".to_string(),
                        detail: format!("model {}", model.name),
                        span,
                    });
                }
            }
            Declaration::Class(class) => {
                if span.start <= offset && offset < span.end {
                    return Some(SymbolInfo {
                        name: class.name.clone(),
                        kind: "class".to_string(),
                        detail: format!("class {}", class.name),
                        span,
                    });
                }
            }
            Declaration::Trait(tr) => {
                if span.start <= offset && offset < span.end {
                    return Some(SymbolInfo {
                        name: tr.name.clone(),
                        kind: "trait".to_string(),
                        detail: format!("trait {}", tr.name),
                        span,
                    });
                }
            }
            Declaration::Enum(en) => {
                if span.start <= offset && offset < span.end {
                    return Some(SymbolInfo {
                        name: en.name.clone(),
                        kind: "enum".to_string(),
                        detail: format!("enum {}", en.name),
                        span,
                    });
                }
            }
            Declaration::TypeAlias(alias) => {
                if span.start <= offset && offset < span.end {
                    return Some(SymbolInfo {
                        name: alias.name.clone(),
                        kind: "type".to_string(),
                        detail: format!("type {} = {}", alias.name, format_type(&alias.target.node)),
                        span,
                    });
                }
            }
            Declaration::Newtype(nt) => {
                if span.start <= offset && offset < span.end {
                    return Some(SymbolInfo {
                        name: nt.name.clone(),
                        kind: "newtype".to_string(),
                        detail: format!("newtype {} = {}", nt.name, format_type(&nt.underlying.node)),
                        span,
                    });
                }
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

/// Format a Type for display
fn format_type(ty: &Type) -> String {
    match ty {
        Type::Simple(name) => name.clone(),
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
        Type::Unit => "()".to_string(),
        Type::SelfType => "Self".to_string(),
    }
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

#[tower_lsp::async_trait]
impl LanguageServer for IncanLanguageServer {
    async fn initialize(&self, _: InitializeParams) -> Result<InitializeResult> {
        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                // Real-time diagnostics via text sync
                text_document_sync: Some(TextDocumentSyncCapability::Kind(TextDocumentSyncKind::FULL)),
                // Hover support
                hover_provider: Some(HoverProviderCapability::Simple(true)),
                // Go-to-definition
                definition_provider: Some(OneOf::Left(true)),
                // Completions (basic)
                completion_provider: Some(CompletionOptions {
                    trigger_characters: Some(vec![".".to_string(), ":".to_string()]),
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
        }

        if let Some(info) = self.find_symbol_at_position(ast, &doc.source, position) {
            let detail = if info.kind == "const" {
                if let Some(resolved) = doc.const_types.get(&info.name) {
                    // Prefer resolved typechecker type, since `const` may freeze annotations.
                    format!("const {}: {}", info.name, resolved)
                } else {
                    info.detail.clone()
                }
            } else {
                info.detail.clone()
            };

            let markdown = format!("```incan\n{}\n```\n\n*{}*", detail, info.kind);

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

        // ---- General completions (not in a specific context) ----

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
                    }
                    Declaration::Enum(en) => {
                        push_completion(
                            &mut items,
                            &mut seen,
                            &en.name,
                            CompletionItemKind::ENUM,
                            Some(format!("enum {}", en.name)),
                            None,
                        );
                    }
                    _ => {}
                }
            }
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
