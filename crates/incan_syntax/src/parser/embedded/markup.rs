// Markup submode grammar (RFC 081, `#1023`): tags, attributes, text nodes, entity references, comments, and
// expression holes. This is the fixed, representative grammar for the `Markup` submode — see
// `incan_vocab::EmbeddedFragmentSubmode::Markup`'s rustdoc for the catalog this belongs to.
//
// Accepted constructs:
// - Open/close tags: `<name ...>...</name>`; self-closing: `<name .../>`.
// - Attributes: `name`, `name="literal text"`, `name={expr}`.
// - Text nodes: any run of characters not starting `<`, `{`, or `&`.
// - Entity references: `&name;`.
// - Comments: `<!-- ... -->`, stored verbatim.
// - Expression holes: `{expr}`, valid in content position and as an attribute value.
//
// Anything else (mismatched close tags, unterminated tags/comments/holes, a bare `&` that is not a well-formed
// entity reference) is a parse error, per RFC 081's "unrecognized syntax is a parse error, not a silent
// reinterpretation" design decision.

/// Parse a whole `Markup`-submode fragment.
///
/// ## Errors
/// Returns a [`CompileError`] for any malformed construct, an unmatched closing tag, or trailing content that
/// does not form a well-formed node sequence.
fn parse_markup_fragment(raw: &str, base_offset: usize) -> Result<Vec<Spanned<EmbeddedNode>>, CompileError> {
    let mut cursor = EmbeddedCursor::new(raw, base_offset);
    let nodes = parse_markup_nodes(&mut cursor)?;
    if cursor.starts_with("</") {
        return Err(CompileError::syntax(
            "Closing tag has no matching open tag in this markup fragment".to_string(),
            cursor.span_from(cursor.pos),
        ));
    }
    Ok(nodes)
}

/// Parse a sequence of sibling markup nodes until end of input or an unmatched `</...` closing tag is reached.
///
/// The caller (either [`parse_markup_fragment`] for the fragment's top level, or [`parse_markup_element`] for one
/// element's children) decides what a stop at `</` means: a real error at the top level, or "consume my own
/// closing tag" inside an element.
fn parse_markup_nodes(cursor: &mut EmbeddedCursor<'_>) -> Result<Vec<Spanned<EmbeddedNode>>, CompileError> {
    let mut nodes = Vec::new();
    while !cursor.is_eof() {
        if cursor.starts_with("</") {
            break;
        }
        if cursor.starts_with("<!--") {
            nodes.push(parse_markup_comment(cursor)?);
        } else if cursor.starts_with("<") {
            nodes.push(parse_markup_element(cursor)?);
        } else if cursor.starts_with("{") {
            nodes.push(parse_embedded_brace_hole(cursor)?);
        } else if cursor.starts_with("&") {
            nodes.push(parse_markup_entity_ref(cursor)?);
        } else {
            nodes.push(parse_markup_text(cursor));
        }
    }
    Ok(nodes)
}

/// Parse one `<name ...>children</name>` element, or a self-closing `<name .../>`.
fn parse_markup_element(cursor: &mut EmbeddedCursor<'_>) -> Result<Spanned<EmbeddedNode>, CompileError> {
    let start = cursor.pos;
    cursor.eat_str("<");
    let name = cursor.eat_while(is_embedded_ident_char).to_string();
    if name.is_empty() {
        return Err(CompileError::syntax(
            "Expected a tag name after `<`".to_string(),
            cursor.span_from(start),
        ));
    }

    let mut attrs = Vec::new();
    loop {
        cursor.skip_ws();
        if cursor.eat_str("/>") {
            return Ok(Spanned::new(
                EmbeddedNode::Element(EmbeddedElement {
                    name,
                    attrs,
                    children: Vec::new(),
                    self_closing: true,
                }),
                cursor.span_from(start),
            ));
        }
        if cursor.eat_str(">") {
            break;
        }
        if cursor.is_eof() {
            return Err(CompileError::syntax(
                format!("Unterminated tag `<{name}`: expected `>` or `/>`"),
                cursor.span_from(start),
            ));
        }
        attrs.push(parse_markup_attr(cursor)?);
    }

    let children = parse_markup_nodes(cursor)?;
    if !cursor.eat_str("</") {
        return Err(CompileError::syntax(
            format!("Unterminated element `<{name}>`: expected a matching `</{name}>`"),
            cursor.span_from(start),
        ));
    }
    let close_start = cursor.pos;
    let close_name = cursor.eat_while(is_embedded_ident_char);
    if close_name != name {
        return Err(CompileError::syntax(
            format!("Mismatched closing tag: expected `</{name}>`, found `</{close_name}>`"),
            cursor.span_from(close_start),
        ));
    }
    cursor.skip_ws();
    if !cursor.eat_str(">") {
        return Err(CompileError::syntax(
            format!("Expected `>` to close `</{name}`"),
            cursor.span_from(start),
        ));
    }

    Ok(Spanned::new(
        EmbeddedNode::Element(EmbeddedElement {
            name,
            attrs,
            children,
            self_closing: false,
        }),
        cursor.span_from(start),
    ))
}

/// Parse one attribute: a bare name, `name="literal"`, or `name={expr}`.
fn parse_markup_attr(cursor: &mut EmbeddedCursor<'_>) -> Result<EmbeddedAttr, CompileError> {
    let start = cursor.pos;
    let name = cursor.eat_while(is_embedded_ident_char).to_string();
    if name.is_empty() {
        return Err(CompileError::syntax(
            "Expected an attribute name, `/>`, or `>`".to_string(),
            cursor.span_from(start),
        ));
    }
    cursor.skip_ws();
    if !cursor.eat_str("=") {
        return Ok(EmbeddedAttr { name, value: None });
    }
    cursor.skip_ws();
    let value = if cursor.starts_with("{") {
        parse_embedded_brace_hole(cursor)?
    } else {
        parse_markup_attr_string(cursor)?
    };
    Ok(EmbeddedAttr {
        name,
        value: Some(value),
    })
}

/// Parse a quoted attribute value (`"literal"` or `'literal'`) as a verbatim `Text` node.
fn parse_markup_attr_string(cursor: &mut EmbeddedCursor<'_>) -> Result<Spanned<EmbeddedNode>, CompileError> {
    let start = cursor.pos;
    if !matches!(cursor.peek(), Some('"' | '\'')) {
        return Err(CompileError::syntax(
            "Expected a quoted attribute value or `{expr}`".to_string(),
            cursor.span_from(start),
        ));
    }
    let (text_start, text) = scan_quoted_body(cursor, "Unterminated attribute value")?;
    Ok(Spanned::new(
        EmbeddedNode::Text(text.to_string()),
        cursor.span_from(text_start),
    ))
}

/// Parse an `&name;` entity reference.
fn parse_markup_entity_ref(cursor: &mut EmbeddedCursor<'_>) -> Result<Spanned<EmbeddedNode>, CompileError> {
    let start = cursor.pos;
    cursor.eat_str("&");
    let name = cursor.eat_while(|c| c.is_ascii_alphanumeric()).to_string();
    if name.is_empty() || !cursor.eat_str(";") {
        return Err(CompileError::syntax(
            "Expected a well-formed entity reference `&name;`".to_string(),
            cursor.span_from(start),
        ));
    }
    Ok(Spanned::new(EmbeddedNode::EntityRef(name), cursor.span_from(start)))
}

/// Parse a `<!-- ... -->` comment, preserving its content verbatim.
fn parse_markup_comment(cursor: &mut EmbeddedCursor<'_>) -> Result<Spanned<EmbeddedNode>, CompileError> {
    scan_delimited_comment(cursor, "<!--", "-->")
}

/// Parse a run of plain text up to the next `<`, `{`, or `&`.
fn parse_markup_text(cursor: &mut EmbeddedCursor<'_>) -> Spanned<EmbeddedNode> {
    let start = cursor.pos;
    let text = cursor.eat_while(|c| c != '<' && c != '{' && c != '&').to_string();
    Spanned::new(EmbeddedNode::Text(text), cursor.span_from(start))
}
