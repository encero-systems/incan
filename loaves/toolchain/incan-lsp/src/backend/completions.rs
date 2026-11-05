//! Decorator, member, builtin, contextual and document completions.

use super::*;

/// Find a decorator at `offset` and resolve it to its registry info.
///
/// Returns `(decorator_id, resolved_path_segments)` if a recognized decorator is found.
pub(super) fn find_decorator_at_position(
    ast: &Program,
    offset: usize,
    aliases: &HashMap<String, Vec<String>>,
) -> Option<(decorators::DecoratorId, Vec<String>)> {
    let check_decorators = |decs: &[incan_frontend::ast::Spanned<incan_frontend::ast::Decorator>]| {
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

pub(super) fn push_completion(
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

pub(super) fn completion_kind_for_module_child(kind: RustModuleChildKind) -> CompletionItemKind {
    match kind {
        RustModuleChildKind::Module => CompletionItemKind::MODULE,
        RustModuleChildKind::Type => CompletionItemKind::CLASS,
        RustModuleChildKind::Function => CompletionItemKind::FUNCTION,
        RustModuleChildKind::Constant => CompletionItemKind::CONSTANT,
        RustModuleChildKind::Trait => CompletionItemKind::INTERFACE,
        RustModuleChildKind::Other => CompletionItemKind::REFERENCE,
    }
}

pub(super) fn rust_member_completion_context(line_prefix: &str) -> Option<(&str, &str)> {
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
pub(super) fn builtin_list_repeat_detail() -> String {
    collection_helpers::signature(BuiltinCollectionHelperId::ListRepeat).to_string()
}

/// Return hover markdown for the built-in `list.repeat` helper.
pub(super) fn builtin_list_repeat_markdown() -> String {
    format!(
        "```incan\n{}\n```\n\n*list helper*\n\nCreates a list with `count` clone-derived copies of `value`. Negative counts raise `ValueError`.",
        builtin_list_repeat_detail()
    )
}

/// Return hover markdown when `ident` is the `repeat` member in `list.repeat`.
pub(super) fn builtin_list_repeat_hover(source: &str, ident: &str, span: Span) -> Option<String> {
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
pub(super) fn builtin_list_member_completions(line_prefix: &str) -> Option<Vec<CompletionItem>> {
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

pub(super) fn rust_member_completions(line_prefix: &str, symbols: &[RustOriginSymbol]) -> Option<Vec<CompletionItem>> {
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

/// Select context-specific completions before general document symbols. The ordering preserves Loaf imports,
/// decorators, builtin members, Rust members and explicit call-site type arguments as distinct early-return contexts.
pub(super) fn specific_completion_items(
    doc: &DocumentState,
    line_prefix: &str,
    position: Position,
) -> Option<Vec<CompletionItem>> {
    if let Some(items) = loaves::import_item_completions(line_prefix, &doc.loaves) {
        return Some(items);
    }
    if let Some(items) = loaves::module_completions(line_prefix, &doc.loaves) {
        return Some(items);
    }
    if let Some(items) = decorator_completions(line_prefix) {
        return Some(items);
    }
    if let Some(items) = builtin_list_member_completions(line_prefix) {
        return Some(items);
    }
    if let Some(items) = rust_member_completions(line_prefix, &doc.rust_origin_symbols) {
        return Some(items);
    }
    if let Some(offset) = position_to_offset(&doc.source, position)
        && call_site_type_args::offset_in_call_site_type_argument_list(&doc.source, offset)
    {
        return Some(call_site_type_args::call_site_type_argument_completion_items(
            doc.ast.as_ref(),
        ));
    }
    None
}

/// Add contextual receiver and scoped DSL symbols before builtin and declaration completions, retaining source
/// aliases and descriptor identities rather than deriving another declaration surface.
pub(super) fn push_contextual_completions(
    doc: &DocumentState,
    position: Position,
    items: &mut Vec<CompletionItem>,
    seen: &mut HashSet<String>,
) {
    let Some(ast) = &doc.ast else {
        return;
    };
    let aliases = collect_import_aliases(ast);
    if let Some(offset) = position_to_offset(&doc.source, position)
        && let Some(context) = classmethod_context_at_offset(ast, offset, &aliases)
    {
        push_completion(
            items,
            seen,
            keywords::as_str(keywords::KeywordId::Cls),
            CompletionItemKind::VARIABLE,
            Some(format!(
                "{} — classmethod receiver constructor",
                classmethod_cls_detail(&context)
            )),
            Some("0_cls".to_string()),
        );
    }
    if let Some(offset) = position_to_offset(&doc.source, position) {
        for symbol in active_scoped_symbol_completions(ast, &doc.library_imported_dsl_surfaces, offset) {
            push_completion(
                items,
                seen,
                &symbol.descriptor.symbol,
                CompletionItemKind::FUNCTION,
                Some(scoped_symbol_completion_detail(
                    symbol.dependency_key,
                    symbol.descriptor,
                )),
                Some(format!("0_scoped_{}", symbol.descriptor.symbol)),
            );
        }
    }
}

/// Add canonical registry spellings before their aliases through the existing first-label-wins completion rule.
pub(super) fn push_registry_completion_names(
    canonical: &str,
    aliases: &[&str],
    kind: CompletionItemKind,
    items: &mut Vec<CompletionItem>,
    seen: &mut HashSet<String>,
) {
    push_completion(items, seen, canonical, kind, None, None);
    for alias in aliases {
        push_completion(items, seen, alias, kind, None, None);
    }
}

/// Add keyword, constructor and collection vocabulary in its original registry order and completion classifications.
pub(super) fn push_builtin_completions(items: &mut Vec<CompletionItem>, seen: &mut HashSet<String>) {
    for info in keywords::KEYWORDS {
        push_registry_completion_names(info.canonical, info.aliases, CompletionItemKind::KEYWORD, items, seen);
    }
    for info in constructors::CONSTRUCTORS {
        push_registry_completion_names(
            info.canonical,
            info.aliases,
            CompletionItemKind::CONSTRUCTOR,
            items,
            seen,
        );
    }
    for info in collections::COLLECTION_TYPES {
        push_registry_completion_names(info.canonical, info.aliases, CompletionItemKind::CLASS, items, seen);
    }
    for mode in incan_lang::lang::types::ownership::OWNERSHIP_MODES {
        push_completion(
            items,
            seen,
            mode.as_str(),
            CompletionItemKind::CLASS,
            Some(mode.description().to_string()),
            None,
        );
    }
}

/// Describe one source declaration's ordinary completion without cloning its syntax. Feature admission belongs to
/// the caller; field, property and enum-variant completions follow this item rather than changing its classification.
pub(super) fn document_declaration_completion<'decl>(
    declaration: &'decl Declaration,
    source: &str,
) -> Option<(&'decl str, CompletionItemKind, String)> {
    Some(match declaration {
        Declaration::Const(value) => (
            &value.name,
            CompletionItemKind::CONSTANT,
            if let Some(ty) = &value.ty {
                format!("const {}: {}", value.name, format_type(&ty.node))
            } else {
                format!("const {}", value.name)
            },
        ),
        Declaration::Static(value) => (
            &value.name,
            CompletionItemKind::CONSTANT,
            format!("static {}: {}", value.name, format_type(&value.ty.node)),
        ),
        Declaration::Function(value) => (
            &value.name,
            CompletionItemKind::FUNCTION,
            format_function_signature(value, source),
        ),
        Declaration::Partial(value) => (
            &value.name,
            CompletionItemKind::FUNCTION,
            format_partial_decl_signature(value, source),
        ),
        Declaration::Model(value) => (&value.name, CompletionItemKind::STRUCT, format!("model {}", value.name)),
        Declaration::Class(value) => (&value.name, CompletionItemKind::CLASS, format!("class {}", value.name)),
        Declaration::Trait(value) => (
            &value.name,
            CompletionItemKind::INTERFACE,
            format!("trait {}", value.name),
        ),
        Declaration::Enum(value) => (&value.name, CompletionItemKind::ENUM, enum_completion_detail(value)),
        Declaration::TypeAlias(value) => (
            &value.name,
            CompletionItemKind::TYPE_PARAMETER,
            format!("type {} = {}", value.name, format_type(&value.target.node)),
        ),
        Declaration::Newtype(value) => {
            let kind = if value.is_rusttype { "rusttype" } else { "newtype" };
            (
                &value.name,
                CompletionItemKind::STRUCT,
                format!("{} {} = {}", kind, value.name, format_type(&value.underlying.node)),
            )
        }
        _ => return None,
    })
}

/// Add canonical field names before distinct metadata aliases, retaining the owner's source kind and existing sort
/// prefixes. Models and classes share this mapping rule while their declaration classification stays separate.
pub(super) fn push_owner_field_completions(
    fields: &[Spanned<incan_frontend::ast::FieldDecl>],
    owner: &str,
    owner_kind: &str,
    items: &mut Vec<CompletionItem>,
    seen: &mut HashSet<String>,
) {
    for field in fields {
        let canonical = field.node.name.as_str();
        push_completion(
            items,
            seen,
            canonical,
            CompletionItemKind::FIELD,
            Some(format!("field on {owner_kind} {owner}")),
            Some(format!("1_{canonical}")),
        );
        if let Some(alias) = field.node.metadata.alias.as_deref()
            && alias != canonical
        {
            push_completion(
                items,
                seen,
                alias,
                CompletionItemKind::FIELD,
                Some(format!("{alias} → {canonical} ({owner})")),
                Some(format!("0_{alias}")),
            );
        }
    }
}

/// Add an owner's properties in source order with their existing field classification and signature details.
pub(super) fn push_owner_property_completions(
    properties: &[Spanned<incan_frontend::ast::PropertyDecl>],
    owner: &str,
    items: &mut Vec<CompletionItem>,
    seen: &mut HashSet<String>,
) {
    for property in properties {
        push_completion(
            items,
            seen,
            &property.node.name,
            CompletionItemKind::FIELD,
            Some(format_property_signature(owner, &property.node)),
            Some(format!("1_{}", property.node.name)),
        );
    }
}

/// Publish active source declarations and then each owner's members in source order. The shared seen set preserves
/// the original precedence against contextual and builtin items, including labels shared by unrelated declarations.
pub(super) fn push_document_completions(
    doc: &DocumentState,
    items: &mut Vec<CompletionItem>,
    seen: &mut HashSet<String>,
) {
    let Some(ast) = &doc.ast else {
        return;
    };
    for declaration in &ast.declarations {
        if !declaration_active_for_lsp(declaration, &doc.active_features) {
            continue;
        }
        let Some((name, kind, detail)) = document_declaration_completion(&declaration.node, &doc.source) else {
            continue;
        };
        push_completion(items, seen, name, kind, Some(detail), None);
        match &declaration.node {
            Declaration::Model(value) => {
                push_owner_field_completions(&value.fields, &value.name, "model", items, seen);
                push_owner_property_completions(&value.properties, &value.name, items, seen);
            }
            Declaration::Class(value) => {
                push_owner_field_completions(&value.fields, &value.name, "class", items, seen);
                push_owner_property_completions(&value.properties, &value.name, items, seen);
            }
            Declaration::Trait(value) => push_owner_property_completions(&value.properties, &value.name, items, seen),
            Declaration::Enum(value) => {
                for variant in &value.variants {
                    let label = enum_variant_completion_label(value, &variant.node);
                    push_completion(
                        items,
                        seen,
                        &label,
                        CompletionItemKind::CONSTRUCTOR,
                        Some(enum_variant_completion_detail(value, &variant.node)),
                        Some(format!("1_{label}")),
                    );
                }
            }
            _ => {}
        }
    }
}

/// Add imported Rust bindings after source declarations, preserving inspected item kinds and the metadata-unavailable
/// fallback. Canonical path details are display text here, not a new identity-resolution rule.
pub(super) fn push_rust_binding_completions(
    doc: &DocumentState,
    items: &mut Vec<CompletionItem>,
    seen: &mut HashSet<String>,
) {
    for symbol in &doc.rust_origin_symbols {
        let (kind, detail) = if let Some(metadata) = &symbol.info.metadata {
            let item_kind = rust_item_kind_label(&metadata.kind);
            (
                match &metadata.kind {
                    RustItemKind::Module(_) => CompletionItemKind::MODULE,
                    RustItemKind::Type(_) => CompletionItemKind::CLASS,
                    RustItemKind::Function(_) => CompletionItemKind::FUNCTION,
                    RustItemKind::Constant { .. } => CompletionItemKind::CONSTANT,
                    RustItemKind::Trait(_) => CompletionItemKind::INTERFACE,
                    RustItemKind::Macro(_) => CompletionItemKind::FUNCTION,
                    RustItemKind::Unsupported { .. } => CompletionItemKind::REFERENCE,
                },
                format!("rust::{} ({item_kind})", symbol.info.path),
            )
        } else {
            (
                CompletionItemKind::REFERENCE,
                format!("rust::{} (metadata unavailable)", symbol.info.path),
            )
        };
        push_completion(
            items,
            seen,
            &symbol.local_name,
            kind,
            Some(detail),
            Some(format!("0_{}", symbol.local_name)),
        );
    }
}

/// Extract the text of the current line up to (but not including) the cursor position.
pub(super) fn line_text_before_cursor(source: &str, position: Position) -> String {
    let lines: Vec<&str> = source.lines().collect();
    let line_idx = position.line as usize;
    if line_idx >= lines.len() {
        return String::new();
    }
    let line = lines[line_idx];
    let col = (position.character as usize).min(line.len());
    line[..col].to_string()
}

/// If the cursor is on a decorator line (starts with `@`), return completions for known decorator names. Returns `None`
/// if not in a decorator context.
pub(super) fn decorator_completions(line_prefix: &str) -> Option<Vec<CompletionItem>> {
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
