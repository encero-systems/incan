//! Import aliases, classmethod contexts, identifiers, checked identities and value-type facts at an offset.

use super::*;

/// Collect import aliases visible to LSP decorator resolution.
pub(super) fn collect_import_aliases(ast: &Program) -> HashMap<String, Vec<String>> {
    incan_frontend::decorator_resolution::collect_import_aliases(ast)
}

/// Resolve a decorator path through visible import aliases.
pub(super) fn resolve_decorator_path(
    dec: &incan_frontend::ast::Decorator,
    aliases: &HashMap<String, Vec<String>>,
) -> Vec<String> {
    incan_frontend::decorator_resolution::resolve_decorator_path(dec, aliases)
}

/// Return whether a method has the requested decorator after resolving import aliases.
pub(super) fn method_has_decorator(
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
pub(super) fn owner_type_display(owner_name: &str, type_params: &[TypeParam]) -> String {
    if type_params.is_empty() {
        owner_name.to_string()
    } else {
        let params: Vec<&str> = type_params.iter().map(|param| param.name.as_str()).collect();
        format!("{owner_name}[{}]", params.join(", "))
    }
}

/// Return whether an offset falls inside a method body rather than its signature.
pub(super) fn method_body_contains_offset(method: &MethodDecl, method_span: Span, offset: usize) -> bool {
    let Some(body) = &method.body else {
        return false;
    };
    let Some(first) = body.first() else {
        return false;
    };
    first.span.start <= offset && offset < method_span.end
}

/// Build contextual classmethod receiver metadata when the offset is inside a classmethod body.
pub(super) fn classmethod_context_for_method(
    owner_name: &str,
    owner_type_params: &[TypeParam],
    method: &incan_frontend::ast::Spanned<MethodDecl>,
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
pub(super) fn classmethod_context_at_offset(
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
pub(super) fn identifier_at_offset(source: &str, offset: usize) -> Option<(String, Span)> {
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

/// Resolve the canonical identity under a cursor without guessing from its spelling.
pub(super) fn checked_identity_at_offset(
    document_uri: &Url,
    ast: &Program,
    source: &str,
    snapshot: &CheckedIdentitySnapshot,
    offset: usize,
) -> Option<CheckedIdentityOccurrence> {
    let (identifier, token_span) = identifier_at_offset(source, offset)?;

    if let Some((identity, reference_span)) = snapshot
        .type_info
        .references
        .resolved_identities
        .iter()
        .filter_map(|(span, identity)| {
            let reference_span = checked_reference_token_span(source, *span, identity)?;
            (reference_span == token_span).then_some((identity, reference_span))
        })
        .min_by_key(|(_, span)| span.end.saturating_sub(span.start))
    {
        return Some(CheckedIdentityOccurrence {
            identity: identity.clone(),
            span: reference_span,
        });
    }

    for (&declaration_span, bindings) in &snapshot.type_info.declarations.hir_bindings_by_span {
        for binding in bindings {
            if binding.local_name != identifier {
                continue;
            }
            let Some(binding_span) = checked_declaration_binding_span(ast, source, declaration_span, &identifier)
            else {
                continue;
            };
            if binding_span == token_span
                && let Some(identity) = &binding.canonical
            {
                return Some(CheckedIdentityOccurrence {
                    identity: identity.clone(),
                    span: token_span,
                });
            }
        }
    }

    for ((span, name), identity) in &snapshot.type_info.references.resolved_write_identities {
        if name != &identifier {
            continue;
        }
        let statement_span = *span;
        if first_identifier_span_named(source, statement_span, name) == Some(token_span) {
            return Some(CheckedIdentityOccurrence {
                identity: identity.clone(),
                span: token_span,
            });
        }
    }

    known_checked_identities(&snapshot.type_info)
        .into_iter()
        .find(|identity| {
            identity.declaration_name == identifier
                && snapshot
                    .declaration_sources
                    .get(&identity.origin)
                    .is_some_and(|declaration_source| &declaration_source.uri == document_uri)
                && first_identifier_span_named(source, identity.declaration_span, &identifier) == Some(token_span)
        })
        .map(|identity| CheckedIdentityOccurrence {
            identity,
            span: token_span,
        })
}

/// Return every distinct identity retained by the checked snapshot.
pub(super) fn known_checked_identities(type_info: &TypeCheckInfo) -> Vec<CanonicalSymbolId> {
    let mut seen = HashSet::new();
    let mut identities = Vec::new();
    let candidates = type_info
        .references
        .resolved_identities
        .values()
        .chain(type_info.references.resolved_write_identities.values())
        .chain(type_info.declarations.declaration_identities.values())
        .chain(type_info.declarations.member_declaration_identities.values())
        .chain(
            type_info
                .declarations
                .hir_bindings_by_span
                .values()
                .flatten()
                .filter_map(|binding| binding.canonical.as_ref()),
        );
    for identity in candidates {
        if seen.insert(identity.clone()) {
            identities.push(identity.clone());
        }
    }
    identities
}

/// Find the exact source token which one checked top-level binding introduced.
pub(super) fn checked_declaration_binding_span(
    ast: &Program,
    source: &str,
    declaration_span: Span,
    local_name: &str,
) -> Option<Span> {
    let declaration = ast
        .declarations
        .iter()
        .find(|declaration| declaration.span == declaration_span)?;
    let named_spans = identifier_spans(declaration_span, source)
        .into_iter()
        .filter(|(name, _)| name == local_name)
        .map(|(_, span)| span)
        .collect::<Vec<_>>();
    match &declaration.node {
        Declaration::Import(_) => named_spans.last().copied(),
        Declaration::Alias(_) | Declaration::Partial(_) => named_spans.first().copied(),
        declaration => {
            let keyword = declaration_binding_keyword(declaration)?;
            let keyword_end = identifier_spans(declaration_span, source)
                .into_iter()
                .find(|(name, _)| name == keyword)
                .map(|(_, span)| span.end)?;
            named_spans.into_iter().find(|span| span.start >= keyword_end)
        }
    }
}

/// Surface keyword immediately preceding a named top-level declaration.
pub(super) fn declaration_binding_keyword(declaration: &Declaration) -> Option<&'static str> {
    match declaration {
        Declaration::Const(_) => Some("const"),
        Declaration::Static(_) => Some("static"),
        Declaration::Model(_) => Some("model"),
        Declaration::Capability(_) => Some("capability"),
        Declaration::Class(_) => Some("class"),
        Declaration::Trait(_) => Some("trait"),
        Declaration::TypeAlias(_) | Declaration::Newtype(_) => Some("type"),
        Declaration::Enum(_) => Some("enum"),
        Declaration::Function(_) => Some("def"),
        Declaration::TestModule(_) => Some("test"),
        Declaration::Import(_)
        | Declaration::Alias(_)
        | Declaration::Partial(_)
        | Declaration::VocabBlock(_)
        | Declaration::Docstring(_) => None,
    }
}

/// Return every lexer-recognized identifier or keyword token inside a checked source span.
pub(super) fn identifier_spans(span: Span, source: &str) -> Vec<(String, Span)> {
    let Some(fragment) = source.get(span.start..span.end) else {
        return Vec::new();
    };
    let Ok(tokens) = lexer::lex(fragment) else {
        return Vec::new();
    };
    tokens
        .into_iter()
        .filter_map(|token| {
            let name = match token.kind {
                lexer::TokenKind::Ident(name) => name,
                lexer::TokenKind::Keyword(id) => keywords::as_str(id).to_string(),
                _ => return None,
            };
            Some((
                name,
                Span {
                    start: span.start + token.span.start,
                    end: span.start + token.span.end,
                    expansion: span.expansion,
                },
            ))
        })
        .collect()
}

/// Return the first identifier token named `name` within a checked source span.
pub(super) fn first_identifier_span_named(source: &str, span: Span, name: &str) -> Option<Span> {
    identifier_spans(span, source)
        .into_iter()
        .find_map(|(candidate, span)| (candidate == name).then_some(span))
}

/// Narrow a reference fact that covers a qualified access or call to the token which denotes its target.
pub(super) fn checked_reference_token_span(source: &str, span: Span, identity: &CanonicalSymbolId) -> Option<Span> {
    let fragment = source.get(span.start..span.end)?;
    let target_end = fragment.find('(').map(|offset| span.start + offset).unwrap_or(span.end);
    let target_span = Span {
        end: target_end,
        ..span
    };
    let tokens = identifier_spans(target_span, source);
    tokens
        .iter()
        .rev()
        .find(|(name, _)| name == &identity.declaration_name)
        .or_else(|| tokens.last())
        .map(|(_, span)| *span)
}

/// Project a canonical identity to its genuine source file, if that exact source was retained by the analysis.
pub(super) fn definition_location_for_identity(
    identity: &CanonicalSymbolId,
    declaration_sources: &RelatedDeclarationSources,
) -> Option<Location> {
    let declaration_source = declaration_sources.get(&identity.origin)?;
    let span = identity.declaration_span;
    if span.start >= span.end || declaration_source.source.get(span.start..span.end).is_none() {
        return None;
    }
    Some(Location {
        uri: declaration_source.uri.clone(),
        range: span_to_range(&declaration_source.source, span.start, span.end),
    })
}

/// Render hover data from the compiler-owned identity itself, preserving alias/re-export provenance.
pub(super) fn checked_identity_hover(source: &str, occurrence: &CheckedIdentityOccurrence) -> Hover {
    let spelling = source
        .get(occurrence.span.start..occurrence.span.end)
        .unwrap_or(&occurrence.identity.declaration_name);
    let markdown = format!(
        "```incan\n{}\n```\n\n*resolved {}*\n\nCanonical declaration: `{}`",
        spelling,
        occurrence.identity.kind.as_str(),
        occurrence.identity.render_compact()
    );
    Hover {
        contents: HoverContents::Markup(MarkupContent {
            kind: MarkupKind::Markdown,
            value: markdown,
        }),
        range: Some(span_to_range(source, occurrence.span.start, occurrence.span.end)),
    }
}

/// Format the LSP detail string for the contextual `cls` receiver binding.
pub(super) fn classmethod_cls_detail(context: &ClassmethodContext) -> String {
    format!(
        "{}: type[{}]",
        keywords::as_str(keywords::KeywordId::Cls),
        context.owner_type
    )
}

/// Return the receiver identifier immediately before a method name span.
pub(super) fn receiver_identifier_before_method(source: &str, span: Span) -> Option<(&str, Span)> {
    let prefix = source.get(..span.start)?;
    let before_dot = prefix.strip_suffix('.')?.trim_end();
    let end = before_dot.len();
    let start = before_dot
        .char_indices()
        .rev()
        .find_map(|(idx, ch)| {
            if ch == '_' || ch.is_ascii_alphanumeric() {
                None
            } else {
                Some(idx + ch.len_utf8())
            }
        })
        .unwrap_or(0);
    let ident = &before_dot[start..end];
    if ident.is_empty() || before_dot[..start].trim_end().ends_with('.') {
        None
    } else {
        Some((ident, Span::new(start, end)))
    }
}

/// Classify a resolved local binding type for LSP affordances that need semantic type shape.
pub(super) fn value_type_kind(ty: &ResolvedType) -> ValueTypeKind {
    match ty {
        ResolvedType::Generic(name, _) if is_ordinal_map_type_name(name) => ValueTypeKind::OrdinalMap,
        _ => ValueTypeKind::Other,
    }
}

/// Return whether a resolved generic type name refers to `OrdinalMap`.
pub(super) fn is_ordinal_map_type_name(name: &str) -> bool {
    name == "OrdinalMap" || name.ends_with(".OrdinalMap")
}

/// Return the nearest known type fact for a value before the given source offset.
pub(super) fn value_type_kind_at(facts: &[ValueTypeFact], name: &str, offset: usize) -> Option<ValueTypeKind> {
    facts
        .iter()
        .filter(|fact| fact.name == name && fact.span.start <= offset)
        .max_by_key(|fact| fact.span.start)
        .map(|fact| fact.kind)
}

/// Return warning hover markdown for explicit unchecked `OrdinalMap` lookup methods.
pub(super) fn unchecked_lookup_hover(
    source: &str,
    value_types: &[ValueTypeFact],
    ident: &str,
    span: Span,
) -> Option<String> {
    let signature = match ident {
        "get_unchecked" => "get_unchecked(...)",
        "get_many_unchecked" => "get_many_unchecked(...)",
        _ => return None,
    };
    let (receiver, receiver_span) = receiver_identifier_before_method(source, span)?;
    if value_type_kind_at(value_types, receiver, receiver_span.start) != Some(ValueTypeKind::OrdinalMap) {
        return None;
    }
    Some(format!(
        "```incan\n{signature}\n```\n\n*unchecked lookup* — only use when every queried key is known to be present. Missing-key behavior is implementation-specific."
    ))
}
