//! Hover for Rust function calls, projected from the checker's retained call signature rather than name lookup.

use tower_lsp::lsp_types::{Hover, HoverContents, MarkupContent, MarkupKind};

use super::{DocumentState, identifier_at_offset, span_to_range};

/// Present a checked free-function call at its authored callee token.
///
/// The checker owns the selected path, instantiated parameters and result. Source text only limits the hover's
/// range to the callee, so an argument or receiver cannot acquire the enclosing call's signature. Missing checked
/// calls return no hover; there is no spelling-based fallback that could confuse a shadowing local with an import.
pub(super) fn checked_call_hover(document: &DocumentState, offset: usize) -> Option<Hover> {
    let (name, token) = identifier_at_offset(&document.source, offset)?;
    let calls = &document.checked_identities.type_info.rust.checked_calls;
    let (&span, call) = calls
        .iter()
        .filter(|(span, call)| {
            span.expansion == token.expansion && span.start == token.start && token.end < span.end && !call.has_receiver
        })
        .min_by_key(|(span, _)| span.end)?;
    let tail = document.source.get(token.end..span.end)?.trim_start();
    if !tail.starts_with('(') && !tail.starts_with('[') {
        return None;
    }
    let parameters = call
        .parameters
        .iter()
        .map(|parameter| {
            let name = parameter.name.as_deref().unwrap_or("_");
            let modifier = if parameter.is_mut { "mut " } else { "" };
            format!("{modifier}{name}: {}", parameter.ty)
        })
        .collect::<Vec<_>>()
        .join(", ");
    let markdown = format!(
        "```incan\ndef {name}({parameters}) -> {}\n```\n\n`rust::{}`",
        call.return_type, call.path
    );
    Some(Hover {
        contents: HoverContents::Markup(MarkupContent {
            kind: MarkupKind::Markdown,
            value: markdown,
        }),
        range: Some(span_to_range(&document.source, token.start, token.end)),
    })
}
