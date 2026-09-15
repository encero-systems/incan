// RawText submode grammar (RFC 081, `#1023`): content preserved verbatim, with expression holes recognized at
// `{expr}` boundaries. This is the grammar for narrow raw-text/comment fragments that intentionally have no
// structural parsing beyond hole recognition (matching RFC 081's "raw text, comments" phrasing) — no tags,
// selectors, or other structure is understood here, unlike `Markup` or `Style`.

/// Parse a whole `RawText`-submode fragment: a sequence of verbatim text runs interleaved with expression holes.
///
/// ## Errors
/// Returns a [`CompileError`] if an expression hole is malformed or unterminated.
fn parse_raw_text_fragment(raw: &str, base_offset: usize) -> Result<Vec<Spanned<EmbeddedNode>>, CompileError> {
    let mut cursor = EmbeddedCursor::new(raw, base_offset);
    let mut nodes = Vec::new();
    while !cursor.is_eof() {
        if cursor.starts_with("{") {
            nodes.push(parse_embedded_brace_hole(&mut cursor)?);
        } else {
            let start = cursor.pos;
            let text = cursor.eat_while(|c| c != '{').to_string();
            nodes.push(Spanned::new(EmbeddedNode::Text(text), cursor.span_from(start)));
        }
    }
    Ok(nodes)
}
