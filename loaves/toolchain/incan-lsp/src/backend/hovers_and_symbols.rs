//! Rust origin symbols and diagnostics, document symbols, source hovers, and inactive-declaration notes.

use super::*;

/// Every checked symbol that a Rust import bound, with the origin the binding recorded, for the completion and hover
/// surfaces that present Rust-backed names.
pub(super) fn collect_rust_origin_symbols(checker: &typechecker::TypeChecker) -> Vec<RustOriginSymbol> {
    checker
        .symbols
        .all_symbols()
        .iter()
        .filter_map(|sym| match &sym.kind {
            incan_frontend::symbols::SymbolKind::RustItem(info) => Some(RustOriginSymbol {
                local_name: sym.name.clone(),
                span: sym.span,
                info: info.clone(),
            }),
            _ => None,
        })
        .collect()
}

pub(super) fn rust_symbol_at_offset(symbols: &[RustOriginSymbol], offset: usize) -> Option<&RustOriginSymbol> {
    symbols
        .iter()
        .find(|sym| sym.span.start <= offset && offset < sym.span.end)
}

pub(super) fn rust_symbol_for_span(symbols: &[RustOriginSymbol], span: Span) -> Option<&RustOriginSymbol> {
    symbols.iter().find(|sym| {
        let overlaps_start = sym.span.start <= span.start && span.start < sym.span.end;
        let overlaps_reverse = span.start <= sym.span.start && sym.span.start < span.end;
        overlaps_start || overlaps_reverse
    })
}

/// Convert one compiler diagnostic to LSP output while preserving the compiler phase and adding Rust-origin context
/// when a Rust import symbol is responsible for the diagnostic span.
pub(super) fn compile_error_to_diagnostic_with_rust_context(
    error: &CompileError,
    source: &str,
    uri: &Url,
    phase: DiagnosticPhase,
    rust_symbols: &[RustOriginSymbol],
    related_sources: &RelatedDeclarationSources,
) -> Diagnostic {
    let mut enriched = error.clone();
    if let Some(sym) = rust_symbol_for_span(rust_symbols, error.span) {
        let note = format!(
            "Rust origin: `{}` resolves to `rust::{}`",
            sym.local_name, sym.info.path
        );
        if !enriched.notes().iter().any(|n| n == &note) {
            enriched = enriched.with_note(note);
        }
    }
    compile_error_to_diagnostic_with_phase_and_sources(&enriched, source, uri, phase, related_sources)
}

/// Return the user-facing LSP label for one inspected Rust item category.
pub(super) fn rust_item_kind_label(kind: &RustItemKind) -> &'static str {
    match kind {
        RustItemKind::Module(_) => "module",
        RustItemKind::Type(_) => "type",
        RustItemKind::Function(_) => "function",
        RustItemKind::Constant { .. } => "constant",
        RustItemKind::Trait(_) => "trait",
        RustItemKind::Macro(_) => "macro",
        RustItemKind::Unsupported { .. } => "unsupported item",
    }
}

/// The hover wording for how a Rust name entered scope: a crate root, a rooted path or a `from` import.
pub(super) fn rust_binding_kind_label(binding: incan_frontend::symbols::RustImportBindingKind) -> &'static str {
    match binding {
        incan_frontend::symbols::RustImportBindingKind::CrateRoot => "crate root import",
        incan_frontend::symbols::RustImportBindingKind::RootedPath => "path import",
        incan_frontend::symbols::RustImportBindingKind::FromImport => "from-import binding",
    }
}

/// Return the LSP document-symbol kind for a top-level declaration.
pub(super) fn lsp_symbol_kind_for_decl(decl: &Declaration) -> Option<SymbolKind> {
    match decl {
        Declaration::Const(_) | Declaration::Static(_) => Some(SymbolKind::CONSTANT),
        Declaration::Function(_) | Declaration::Partial(_) => Some(SymbolKind::FUNCTION),
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
pub(super) fn lsp_document_symbol_name_and_detail(decl: &Declaration, source: &str) -> Option<(String, String)> {
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
        Declaration::Function(func) => Some((func.name.clone(), format_function_signature(func, source))),
        Declaration::Partial(partial) => Some((partial.name.clone(), format_partial_decl_signature(partial, source))),
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

/// Wrap already selected Markdown and its optional exact source span without performing another symbol lookup.
pub(super) fn source_markdown_hover(source: &str, markdown: String, span: Option<Span>) -> Hover {
    Hover {
        contents: HoverContents::Markup(MarkupContent {
            kind: MarkupKind::Markdown,
            value: markdown,
        }),
        range: span.map(|span| span_to_range(source, span.start, span.end)),
    }
}

/// Select ownership, registry, C binding, decorator and Loaf import hovers in their established priority. Embedded
/// DSL ownership must win before source-text name lookup; expression holes continue to ordinary Incan hover paths.
pub(super) fn source_surface_hover(
    doc: &DocumentState,
    ast: &Program,
    offset: usize,
    aliases: &HashMap<String, Vec<String>>,
) -> Option<Hover> {
    if let Some(ownership) = embedded_dsl_ownership_at_offset(ast, offset) {
        return Some(source_markdown_hover(
            &doc.source,
            embedded_dsl_ownership_hover_markdown(&ownership),
            Some(ownership.span),
        ));
    }
    if let Some(preview) = registry_preview_at_offset(&doc.registry_previews, offset) {
        return Some(source_markdown_hover(
            &doc.source,
            preview.markdown.clone(),
            Some(preview.registration_span),
        ));
    }
    if let Some(preview) = c_binding_preview_at_offset(&doc.c_binding_previews, offset) {
        return Some(source_markdown_hover(
            &doc.source,
            preview.markdown.clone(),
            Some(preview.span),
        ));
    }
    if let Some((id, _resolved)) = find_decorator_at_position(ast, offset, aliases) {
        let info = decorators::info_for(id);
        let markdown = format!("```incan\n@{}\n```\n\n{}", info.canonical, info.description);
        return Some(source_markdown_hover(&doc.source, markdown, None));
    }
    if let Some((markdown, span)) = loaves::import_item_hover(ast, &doc.source, offset, &doc.loaves) {
        return Some(source_markdown_hover(&doc.source, markdown, Some(span)));
    }
    loaves::import_module_hover(ast, offset, &doc.loaves)
        .map(|markdown| source_markdown_hover(&doc.source, markdown, None))
}

/// Prefer checked Rust calls and imports, then contextual receivers and retained Incan identities. The source
/// identifier is scanned once for the remaining immutable lookups; no declaration or document snapshot is needed.
pub(super) fn resolved_value_hover(
    uri: &Url,
    doc: &DocumentState,
    ast: &Program,
    offset: usize,
    aliases: &HashMap<String, Vec<String>>,
) -> Option<Hover> {
    if let Some(hover) = rust_hover::checked_call_hover(doc, offset) {
        return Some(hover);
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
        return Some(source_markdown_hover(&doc.source, markdown, Some(rust_symbol.span)));
    }
    let identifier = identifier_at_offset(&doc.source, offset);
    if let Some((ident, span)) = &identifier
        && keywords::from_str(ident.as_str()) == Some(keywords::KeywordId::Cls)
        && let Some(context) = classmethod_context_at_offset(ast, offset, aliases)
    {
        let detail = classmethod_cls_detail(&context);
        let markdown = format!(
            "```incan\n{detail}\n```\n\n*classmethod receiver* — callable constructor for `{}`.",
            context.owner_type
        );
        return Some(source_markdown_hover(&doc.source, markdown, Some(*span)));
    }
    if let Some((ident, span)) = &identifier
        && let Some(markdown) = builtin_list_repeat_hover(&doc.source, ident, *span)
    {
        return Some(source_markdown_hover(&doc.source, markdown, Some(*span)));
    }
    if let Some(occurrence) = checked_identity_at_offset(uri, ast, &doc.source, &doc.checked_identities, offset) {
        return Some(checked_identity_hover(&doc.source, &occurrence));
    }
    if let Some((ident, span)) = &identifier
        && let Some(markdown) = unchecked_lookup_hover(&doc.source, &doc.value_types, ident, *span)
    {
        return Some(source_markdown_hover(&doc.source, markdown, Some(*span)));
    }
    None
}

/// Return whether every positive package-feature condition on a declaration is active in the editor projection.
pub(super) fn declaration_active_for_lsp(
    declaration: &Spanned<Declaration>,
    active_features: &BTreeSet<String>,
) -> bool {
    declaration
        .required_features
        .iter()
        .all(|feature| active_features.contains(feature))
}

/// Explain the positive feature conjunction hiding the declaration at one source offset.
pub(super) fn inactive_declaration_note_at_offset(
    ast: &Program,
    offset: usize,
    active_features: &BTreeSet<String>,
) -> Option<String> {
    let declaration = ast
        .declarations
        .iter()
        .find(|declaration| declaration.span.start <= offset && offset <= declaration.span.end)?;
    if declaration_active_for_lsp(declaration, active_features) {
        return None;
    }
    Some(format!(
        "inactive; requires {}.",
        declaration
            .required_features
            .iter()
            .map(|feature| format!("`{feature}`"))
            .collect::<Vec<_>>()
            .join(" and ")
    ))
}
