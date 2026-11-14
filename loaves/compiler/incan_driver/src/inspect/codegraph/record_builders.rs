//! Import, containment, export and diagnostic records, declaration summaries, and display helpers.

use super::*;

/// Return a compact source-facing callee label for a call expression.
pub(super) fn expr_label(expr: &Expr) -> String {
    match expr {
        Expr::Ident(name) => name.clone(),
        Expr::SelfExpr => "self".to_string(),
        Expr::Field(base, field) => format!("{}.{}", expr_label(&base.node), field),
        Expr::Paren(inner) => expr_label(&inner.node),
        Expr::Surface(surface) => match &surface.payload {
            SurfaceExprPayload::ScopedSymbolCall { symbol, .. } => symbol.clone(),
            SurfaceExprPayload::LeadingDotPath { segments, .. } => segments.join("."),
            _ => "<expr>".to_string(),
        },
        _ => "<expr>".to_string(),
    }
}

/// Sanitize free-form labels so record ids stay readable and single-line.
pub(super) fn sanitize_record_label(label: &str) -> String {
    let sanitized = label
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.') {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();
    if sanitized.is_empty() {
        "_".to_string()
    } else {
        sanitized
    }
}

/// Build one import record from source AST import syntax.
#[allow(clippy::too_many_arguments)] // The constructor mirrors the independent persisted import-record fields.
pub(super) fn import_record(
    module: &ParsedModule,
    module_id: &str,
    import_id: &str,
    import: &ImportDecl,
    span: Span,
    bindings: Vec<CodegraphImportBinding>,
    provenance: CodegraphProvenance,
    degraded: bool,
) -> CodegraphImportRecord {
    let (kind, path, items) = import_shape(import);
    CodegraphImportRecord {
        id: import_id.to_string(),
        language: import_language(import),
        module_id: module_id.to_string(),
        kind,
        path,
        items,
        bindings,
        alias: import.alias.clone(),
        visibility: visibility_spelling(import.visibility).to_string(),
        span: Some(source_span(module, span)),
        provenance,
        degraded,
    }
}

/// Build one containment edge between two source-backed records.
pub(super) fn containment_record(
    parent_id: &str,
    child_id: &str,
    kind: &str,
    module: &ParsedModule,
    span: Span,
    degraded: bool,
) -> CodegraphContainmentRecord {
    CodegraphContainmentRecord {
        id: format!("contains:{parent_id}:{child_id}"),
        language: CodegraphLanguage::Incan,
        parent_id: parent_id.to_string(),
        child_id: child_id.to_string(),
        kind: kind.to_string(),
        span: Some(source_span(module, span)),
        provenance: CodegraphProvenance::Syntax,
        degraded,
    }
}

/// Build one public export fact from either a declaration or public import source record.
#[allow(clippy::too_many_arguments)] // The constructor mirrors the independent persisted export-record fields.
pub(super) fn export_record(
    module: &ParsedModule,
    module_id: &str,
    source_id: &str,
    name: &str,
    kind: &str,
    span: Span,
    canonical_identity: Option<CodegraphCanonicalSymbolId>,
    stable_identity: Option<CodegraphStableDeclarationId>,
    degraded: bool,
) -> CodegraphExportRecord {
    CodegraphExportRecord {
        id: format!("export:{module_id}:{name}:{kind}"),
        language: CodegraphLanguage::Incan,
        module_id: module_id.to_string(),
        name: name.to_string(),
        kind: kind.to_string(),
        source_id: source_id.to_string(),
        canonical_identity: canonical_identity.clone(),
        stable_identity,
        span: Some(source_span(module, span)),
        provenance: if canonical_identity.is_some() {
            CodegraphProvenance::Checked
        } else {
            CodegraphProvenance::Syntax
        },
        degraded,
    }
}

/// Convert a stable diagnostic into the codegraph diagnostic record shape.
pub(super) fn diagnostic_record(index: usize, diagnostic: &StableDiagnostic) -> CodegraphDiagnosticRecord {
    CodegraphDiagnosticRecord {
        id: format!(
            "diagnostic:{}:{}:{}",
            diagnostic.primary_span.file, diagnostic.primary_span.start.offset, index
        ),
        language: CodegraphLanguage::Incan,
        code: diagnostic.code.to_string(),
        severity: diagnostic.severity.to_string(),
        phase: diagnostic.phase.as_str().to_string(),
        origin: diagnostic.origin.as_str().to_string(),
        message: diagnostic.message.clone(),
        primary_span: CodegraphSourceSpan {
            file: diagnostic.primary_span.file.clone(),
            start: diagnostic.primary_span.start.offset,
            end: diagnostic.primary_span.end.offset,
            start_line: diagnostic.primary_span.start.line,
            start_column: diagnostic.primary_span.start.column,
            end_line: diagnostic.primary_span.end.line,
            end_column: diagnostic.primary_span.end.column,
        },
        notes: diagnostic.notes.clone(),
        hints: diagnostic.hints.clone(),
        expected: diagnostic.expected.clone(),
        actual: diagnostic.actual.clone(),
        related_spans: diagnostic
            .related_spans
            .iter()
            .map(|related| CodegraphDiagnosticRelatedSpan {
                span: CodegraphSourceSpan {
                    file: related.span.file.clone(),
                    start: related.span.start.offset,
                    end: related.span.end.offset,
                    start_line: related.span.start.line,
                    start_column: related.span.start.column,
                    end_line: related.span.end.line,
                    end_column: related.span.end.column,
                },
                label: related.label.clone(),
            })
            .collect(),
        related_declarations: diagnostic
            .related_declarations
            .iter()
            .map(|related| CodegraphDiagnosticRelatedDeclaration {
                identity: codegraph_canonical_identity(&related.identity),
                label: related.label.clone(),
            })
            .collect(),
        explain: diagnostic.explain.clone(),
        provenance: CodegraphProvenance::Diagnostic,
        degraded: true,
    }
}

/// Summarize a top-level source declaration for the baseline codegraph record set.
pub(super) fn declaration_summary(declaration: &Declaration) -> Option<DeclarationSummary> {
    match declaration {
        Declaration::Const(decl) => Some(DeclarationSummary {
            kind: "const".to_string(),
            name: decl.name.clone(),
            visibility: decl.visibility,
            type_params: Vec::new(),
            signature: decl.ty.as_ref().map(|ty| format!("const {}: {}", decl.name, ty.node)),
        }),
        Declaration::Capability(decl) => Some(DeclarationSummary {
            kind: "capability".to_string(),
            name: decl.name.clone(),
            visibility: decl.visibility,
            type_params: Vec::new(),
            signature: Some(format!("capability {}", decl.name)),
        }),
        Declaration::Static(decl) => Some(DeclarationSummary {
            kind: "static".to_string(),
            name: decl.name.clone(),
            visibility: decl.visibility,
            type_params: Vec::new(),
            signature: Some(format!("static {}: {}", decl.name, decl.ty.node)),
        }),
        Declaration::Model(decl) => Some(DeclarationSummary {
            kind: "model".to_string(),
            name: decl.name.clone(),
            visibility: decl.visibility,
            type_params: type_param_names(&decl.type_params),
            signature: Some(format_type_decl_signature("model", &decl.name, &decl.type_params)),
        }),
        Declaration::Class(decl) => Some(DeclarationSummary {
            kind: "class".to_string(),
            name: decl.name.clone(),
            visibility: decl.visibility,
            type_params: type_param_names(&decl.type_params),
            signature: Some(format_type_decl_signature("class", &decl.name, &decl.type_params)),
        }),
        Declaration::Trait(decl) => Some(DeclarationSummary {
            kind: "trait".to_string(),
            name: decl.name.clone(),
            visibility: decl.visibility,
            type_params: type_param_names(&decl.type_params),
            signature: Some(format_type_decl_signature("trait", &decl.name, &decl.type_params)),
        }),
        Declaration::Alias(decl) => Some(DeclarationSummary {
            kind: "alias".to_string(),
            name: decl.name.clone(),
            visibility: decl.visibility,
            type_params: Vec::new(),
            signature: Some(format!("{} = alias {}", decl.name, import_path_display(&decl.target))),
        }),
        Declaration::Partial(decl) => Some(DeclarationSummary {
            kind: "partial".to_string(),
            name: decl.name.clone(),
            visibility: decl.visibility,
            type_params: Vec::new(),
            signature: Some(format!("{} = partial {}", decl.name, import_path_display(&decl.target))),
        }),
        Declaration::TypeAlias(decl) => Some(DeclarationSummary {
            kind: "type_alias".to_string(),
            name: decl.name.clone(),
            visibility: decl.visibility,
            type_params: type_param_names(&decl.type_params),
            signature: Some(format!(
                "type {}{} = {}",
                decl.name,
                format_type_params(&decl.type_params),
                decl.target.node
            )),
        }),
        Declaration::Newtype(decl) => Some(DeclarationSummary {
            kind: if decl.is_rusttype { "rusttype" } else { "newtype" }.to_string(),
            name: decl.name.clone(),
            visibility: decl.visibility,
            type_params: type_param_names(&decl.type_params),
            signature: Some(format_type_decl_signature(
                if decl.is_rusttype { "rusttype" } else { "newtype" },
                &decl.name,
                &decl.type_params,
            )),
        }),
        Declaration::Enum(decl) => Some(DeclarationSummary {
            kind: "enum".to_string(),
            name: decl.name.clone(),
            visibility: decl.visibility,
            type_params: type_param_names(&decl.type_params),
            signature: Some(format_type_decl_signature("enum", &decl.name, &decl.type_params)),
        }),
        Declaration::Function(decl) => Some(DeclarationSummary {
            kind: "function".to_string(),
            name: decl.name.clone(),
            visibility: decl.visibility,
            type_params: type_param_names(&decl.type_params),
            signature: Some(function_signature(decl)),
        }),
        Declaration::TestModule(decl) => Some(DeclarationSummary {
            kind: "test_module".to_string(),
            name: decl.name.clone(),
            visibility: Visibility::Private,
            type_params: Vec::new(),
            signature: Some(format!("module {}", decl.name)),
        }),
        Declaration::Import(_) | Declaration::VocabBlock(_) | Declaration::Docstring(_) => None,
    }
}

/// Format a source-level function signature from parsed parameter and return annotations.
pub(super) fn function_signature(decl: &FunctionDecl) -> String {
    let params = decl
        .params
        .iter()
        .map(|param| format!("{}: {}", param.node.name, param.node.ty.node))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "def {}{}({}) -> {}",
        decl.name,
        format_type_params(&decl.type_params),
        params,
        decl.return_type.node
    )
}

/// Format a declaration signature prefix for type-bearing declarations.
pub(super) fn format_type_decl_signature(kind: &str, name: &str, type_params: &[TypeParam]) -> String {
    format!("{kind} {name}{}", format_type_params(type_params))
}

/// Extract generic parameter names without serializing their full bounds yet.
pub(super) fn type_param_names(type_params: &[TypeParam]) -> Vec<String> {
    type_params.iter().map(|param| param.name.clone()).collect()
}

/// Format generic parameters in source syntax for signature summaries.
pub(super) fn format_type_params(type_params: &[TypeParam]) -> String {
    if type_params.is_empty() {
        String::new()
    } else {
        format!(
            "[{}]",
            type_params
                .iter()
                .map(|param| param.name.clone())
                .collect::<Vec<_>>()
                .join(", ")
        )
    }
}

/// Return the import kind, path, and item list for one parsed import declaration. Return the language of the items an
/// import brings into scope.
///
/// The import statement is Incan source either way; what this names is the language of what it imports, which is
/// the axis RFC 106 defined `language` for. One graph spans both languages and `language` is an attribute of the
/// fact rather than a partition of the schema -- that is why the reserved `rust_item` node kind and
/// `uses_rust_item` edge kind were retired rather than added. Filling the field with a constant left the graph
/// unable to answer which Rust items a module reaches, which is the question build invalidation has to ask.
pub(super) fn import_language(import: &ImportDecl) -> CodegraphLanguage {
    match &import.kind {
        ImportKind::RustCrate { .. } | ImportKind::RustFrom { .. } => CodegraphLanguage::Rust,
        ImportKind::Module(_)
        | ImportKind::From { .. }
        | ImportKind::PubLibrary { .. }
        | ImportKind::PubFrom { .. }
        | ImportKind::Python(_) => CodegraphLanguage::Incan,
    }
}

/// Describe one import as the codegraph records it: its kind, the path it names, and the items it brings in.
pub(super) fn import_shape(import: &ImportDecl) -> (String, String, Vec<String>) {
    match &import.kind {
        ImportKind::Module(path) => ("module".to_string(), import_path_display(path), Vec::new()),
        ImportKind::From { module, items } => (
            "from".to_string(),
            import_path_display(module),
            items.iter().map(import_item_display).collect(),
        ),
        ImportKind::PubLibrary { library, path } => (
            "pub_library".to_string(),
            pub_import_path_display(library, path),
            Vec::new(),
        ),
        ImportKind::PubFrom { library, path, items } => (
            "pub_from".to_string(),
            pub_import_path_display(library, path),
            items.iter().map(import_item_display).collect(),
        ),
        ImportKind::Python(module) => ("python".to_string(), module.clone(), Vec::new()),
        ImportKind::RustCrate {
            crate_name,
            path,
            version: _,
            features: _,
        } => (
            "rust_crate".to_string(),
            rust_path_display(crate_name, path),
            Vec::new(),
        ),
        ImportKind::RustFrom {
            crate_name,
            path,
            version: _,
            features: _,
            items,
        } => (
            "rust_from".to_string(),
            rust_path_display(crate_name, path),
            items.iter().map(import_item_display).collect(),
        ),
    }
}

/// Return the public names produced by a public import declaration.
pub(super) fn import_export_names(import: &ImportDecl) -> Vec<String> {
    match &import.kind {
        ImportKind::Module(path) => vec![import.alias.clone().unwrap_or_else(|| {
            path.segments
                .last()
                .cloned()
                .unwrap_or_else(|| import_path_display(path))
        })],
        ImportKind::From { items, .. } | ImportKind::PubFrom { items, .. } | ImportKind::RustFrom { items, .. } => {
            items
                .iter()
                .map(|item| item.alias.clone().unwrap_or_else(|| item.name.clone()))
                .collect()
        }
        ImportKind::PubLibrary { library, path } => vec![
            import
                .alias
                .clone()
                .or_else(|| path.last().cloned())
                .unwrap_or_else(|| library.clone()),
        ],
        ImportKind::Python(module) => vec![import.alias.clone().unwrap_or_else(|| module.clone())],
        ImportKind::RustCrate { crate_name, path, .. } => vec![
            import
                .alias
                .clone()
                .unwrap_or_else(|| path.last().cloned().unwrap_or_else(|| crate_name.clone())),
        ],
    }
}

/// Format one public-package import path using the canonical codegraph spelling.
pub(super) fn pub_import_path_display(library: &str, path: &[String]) -> String {
    let mut display = format!("pub::{library}");
    for segment in path {
        display.push('.');
        display.push_str(segment);
    }
    display
}

/// Format one imported item, preserving local alias spelling when present.
pub(super) fn import_item_display(item: &ImportItem) -> String {
    if let Some(alias) = &item.alias {
        format!("{} as {alias}", item.name)
    } else {
        item.name.clone()
    }
}

/// Parse the source-path spelling retained by a checked public `from` import.
pub(super) fn codegraph_source_import_path(path: &str) -> Option<Vec<String>> {
    let mut segments = path.split("::").map(str::to_string).collect::<Vec<_>>();
    if segments.first().is_some_and(|segment| segment == "crate") {
        segments.remove(0);
    }
    (!segments.is_empty() && !segments.iter().any(|segment| segment == ".." || segment.is_empty())).then_some(segments)
}

/// Recover source and local names from the deterministic import-item display form.
pub(super) fn codegraph_import_item_names(item: &str) -> Option<(String, String)> {
    let (source, local) = item.split_once(" as ").unwrap_or((item, item));
    (!source.is_empty() && !local.is_empty()).then(|| (source.to_string(), local.to_string()))
}

/// Follow public facade aliases to the source-owned target path, refusing cycles.
pub(super) fn resolve_codegraph_alias_target(
    path: &[String],
    aliases: &BTreeMap<Vec<String>, (Vec<String>, CodegraphSourceSpan)>,
) -> Option<Vec<String>> {
    let mut current = aliases.get(path)?.0.clone();
    let mut visited = BTreeSet::new();
    visited.insert(path.to_vec());
    while let Some((next, _)) = aliases.get(&current) {
        if !visited.insert(current.clone()) {
            return None;
        }
        current = next.clone();
    }
    Some(current)
}

/// Convert `<module>::<binding>` to the import-path vocabulary used by source exports.
pub(super) fn registry_identity_path(identity: &str) -> Vec<String> {
    identity.split("::").map(str::to_string).collect()
}

/// Convert function subject identity such as `pkg::text.normalize` into an import path.
///
/// A model reexport does not create a named import path for one of its methods, so methods deliberately retain only
/// their canonical source subject in the first registry schema.
pub(super) fn registry_subject_path(identity: &str) -> Option<Vec<String>> {
    let (module, declaration) = identity.rsplit_once('.')?;
    if declaration.contains('.') {
        return None;
    }
    let mut path = module.split("::").map(str::to_string).collect::<Vec<_>>();
    path.push(declaration.to_string());
    Some(path)
}

/// Format a parsed Incan import path without resolving it to a filesystem path.
pub(super) fn import_path_display(path: &ImportPath) -> String {
    let mut parts = Vec::new();
    if path.is_absolute {
        parts.push("crate".to_string());
    }
    for _ in 0..path.parent_levels {
        parts.push("..".to_string());
    }
    parts.extend(path.segments.clone());
    parts.join("::")
}

/// Format a Rust import path with the `rust::` namespace marker used by Incan source.
pub(super) fn rust_path_display(crate_name: &str, path: &[String]) -> String {
    if path.is_empty() {
        format!("rust::{crate_name}")
    } else {
        format!("rust::{crate_name}::{}", path.join("::"))
    }
}

/// Return the stable JSON spelling for source visibility.
pub(super) fn visibility_spelling(visibility: Visibility) -> &'static str {
    match visibility {
        Visibility::Private => "private",
        Visibility::Public => "public",
    }
}

/// Build a module id that keeps same-named modules in different files distinct.
pub(super) fn module_id(module: &ParsedModule) -> String {
    format!(
        "module:{}:{}",
        module.file_path.to_string_lossy(),
        module.path_segments.join("::")
    )
}

/// Build a declaration id from file, span, and symbol name so declaration order changes do not alone rename ids.
pub(super) fn declaration_id(module: &ParsedModule, declaration: &Spanned<Declaration>, index: usize) -> String {
    let name = declaration_summary(&declaration.node)
        .map(|summary| summary.name)
        .unwrap_or_else(|| format!("decl-{index}"));
    format!(
        "decl:{}:{}:{name}",
        module.file_path.to_string_lossy(),
        declaration.span.start
    )
}

/// Build an import id from file and declaration index.
pub(super) fn import_id(module: &ParsedModule, index: usize) -> String {
    format!("import:{}:{index}", module.file_path.to_string_lossy())
}

/// Infer a fallback module name for independently parsed directory files.
pub(super) fn module_name_for_file(path: &Path) -> String {
    path.file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("module")
        .to_string()
}

/// Infer fallback module path segments for independently parsed directory files.
pub(super) fn module_segments_for_file(path: &Path, base: &Path) -> Vec<String> {
    let relative = path.strip_prefix(base).unwrap_or(path);
    let mut segments = relative
        .components()
        .filter_map(|component| component.as_os_str().to_str().map(str::to_string))
        .collect::<Vec<_>>();
    if let Some(last) = segments.last_mut()
        && let Some(stem) = last.strip_suffix(".incn")
    {
        *last = stem.to_string();
    }
    if segments.is_empty() {
        vec![module_name_for_file(path)]
    } else {
        segments
    }
}

/// Convert an AST byte span into the public codegraph source span shape.
pub(super) fn source_span(module: &ParsedModule, span: Span) -> CodegraphSourceSpan {
    let source = &module.source;
    let start = span.start.min(source.len());
    let end = span.end.min(source.len()).max(start);
    let start_position = module.source_positions.position_for_offset(source, start);
    let end_position = module.source_positions.position_for_offset(source, end);
    let (start_line, start_column) = (start_position.line, start_position.column);
    let (end_line, end_column) = (end_position.line, end_position.column);
    CodegraphSourceSpan {
        file: path_string(&module.file_path),
        start,
        end,
        start_line,
        start_column,
        end_line,
        end_column,
    }
}

/// Format a filesystem path using the process-native display spelling.
pub(super) fn path_string(path: &Path) -> String {
    path.to_string_lossy().to_string()
}
