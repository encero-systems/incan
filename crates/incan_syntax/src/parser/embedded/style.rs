// Style submode grammar (RFC 081, `#1023`): a selector list followed by a `{ declarations }` block, with `/* */`
// comments between rules or declarations. Declaration values reuse the shared value-token grammar in
// `selector_declaration_value.rs` (dimension, color, `var()` reference, identifier, string, number, hole).
//
// Accepted constructs:
// - One or more comma-separated selectors, captured as flat token runs (this submode does not further parse
//   combinator/pseudo-class structure — see `EmbeddedValue::Selector`'s rustdoc).
// - A declaration block `{ property: value value ...; ... }`, including custom properties (`--name: value;`).
// - `/* ... */` comments between rules and between declarations.

/// Parse a whole `Style`-submode fragment: a sequence of rules and comments.
///
/// ## Errors
/// Returns a [`CompileError`] for any malformed rule, declaration, or unterminated comment.
fn parse_style_fragment(raw: &str, base_offset: usize) -> Result<Vec<Spanned<EmbeddedNode>>, CompileError> {
    let mut cursor = EmbeddedCursor::new(raw, base_offset);
    let mut nodes = Vec::new();
    loop {
        cursor.skip_ws();
        if cursor.is_eof() {
            break;
        }
        if cursor.starts_with("/*") {
            nodes.push(parse_style_comment(&mut cursor)?);
        } else {
            nodes.push(parse_style_rule(&mut cursor)?);
        }
    }
    Ok(nodes)
}

/// Parse one style rule: a selector list, then a `{ ... }` declaration block.
fn parse_style_rule(cursor: &mut EmbeddedCursor<'_>) -> Result<Spanned<EmbeddedNode>, CompileError> {
    let start = cursor.pos;
    let selectors = parse_style_selector_list(cursor)?;
    cursor.skip_ws();
    if !cursor.eat_str("{") {
        return Err(CompileError::syntax(
            "Expected `{` to start a declaration block after this selector list".to_string(),
            cursor.span_from(start),
        ));
    }
    let declarations = parse_style_declarations(cursor)?;
    if !cursor.eat_str("}") {
        return Err(CompileError::syntax(
            "Expected `}` to close this declaration block".to_string(),
            cursor.span_from(start),
        ));
    }
    Ok(Spanned::new(
        EmbeddedNode::StyleRule(EmbeddedStyleRule {
            selectors,
            declarations,
        }),
        cursor.span_from(start),
    ))
}

/// Parse a comma-separated selector list up to (not including) the rule's opening `{`.
fn parse_style_selector_list(cursor: &mut EmbeddedCursor<'_>) -> Result<Vec<Spanned<EmbeddedNode>>, CompileError> {
    let mut selectors = Vec::new();
    loop {
        cursor.skip_ws();
        let start = cursor.pos;
        let text = cursor.eat_while(|c| c != ',' && c != '{');
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return Err(CompileError::syntax(
                "Expected a selector before `{`".to_string(),
                cursor.span_from(start),
            ));
        }
        selectors.push(Spanned::new(
            EmbeddedNode::Value(EmbeddedValue::Selector(trimmed.to_string())),
            cursor.span_from(start),
        ));
        if cursor.eat_str(",") {
            continue;
        }
        break;
    }
    Ok(selectors)
}

/// Parse the declarations (and any interleaved comments) inside a rule's `{ ... }` block.
fn parse_style_declarations(cursor: &mut EmbeddedCursor<'_>) -> Result<Vec<Spanned<EmbeddedNode>>, CompileError> {
    let mut declarations = Vec::new();
    loop {
        cursor.skip_ws();
        if cursor.starts_with("}") || cursor.is_eof() {
            break;
        }
        if cursor.starts_with("/*") {
            declarations.push(parse_style_comment(cursor)?);
            continue;
        }
        declarations.push(parse_style_declaration(cursor)?);
    }
    Ok(declarations)
}

/// Parse one `property: value value ...;` declaration.
///
/// The trailing `;` is optional on the last declaration before `}`, matching ordinary CSS authoring convention.
fn parse_style_declaration(cursor: &mut EmbeddedCursor<'_>) -> Result<Spanned<EmbeddedNode>, CompileError> {
    let start = cursor.pos;
    let property = cursor.eat_while(is_embedded_ident_char).to_string();
    if property.is_empty() {
        return Err(CompileError::syntax(
            "Expected a declaration property name".to_string(),
            cursor.span_from(start),
        ));
    }
    cursor.skip_ws();
    if !cursor.eat_str(":") {
        return Err(CompileError::syntax(
            format!("Expected `:` after declaration property `{property}`"),
            cursor.span_from(start),
        ));
    }
    let mut value = Vec::new();
    loop {
        cursor.skip_ws();
        if cursor.starts_with(";") || cursor.starts_with("}") || cursor.is_eof() {
            break;
        }
        value.push(parse_embedded_value_token(cursor)?);
    }
    if value.is_empty() {
        return Err(CompileError::syntax(
            format!("Expected a value for declaration `{property}`"),
            cursor.span_from(start),
        ));
    }
    cursor.eat_str(";");
    Ok(Spanned::new(
        EmbeddedNode::Declaration(EmbeddedDeclaration { property, value }),
        cursor.span_from(start),
    ))
}

/// Parse a `/* ... */` comment, preserving its content verbatim.
fn parse_style_comment(cursor: &mut EmbeddedCursor<'_>) -> Result<Spanned<EmbeddedNode>, CompileError> {
    scan_delimited_comment(cursor, "/*", "*/")
}
