// RegexTemplate submode grammar (RFC 081, `#1023`): either a regex literal `/pattern/flags`, or a template string
// `` `...${expr}...` `` with expression-hole interpolation. A fragment claiming this submode must be exactly one
// of the two forms — nothing else is accepted.

/// Parse a whole `RegexTemplate`-submode fragment: a bare regex literal, or a backtick template string.
///
/// ## Errors
/// Returns a [`CompileError`] if the fragment starts with neither `/` nor `` ` ``, or if the chosen form is
/// malformed.
fn parse_regex_template_fragment(raw: &str, base_offset: usize) -> Result<Vec<Spanned<EmbeddedNode>>, CompileError> {
    let mut cursor = EmbeddedCursor::new(raw, base_offset);
    let nodes = if cursor.starts_with("/") {
        vec![parse_regex_literal(&mut cursor)?]
    } else if cursor.starts_with("`") {
        parse_template_string(&mut cursor)?
    } else {
        return Err(CompileError::syntax(
            "A `RegexTemplate` fragment must be a `/pattern/flags` regex literal or a `` `template` `` string"
                .to_string(),
            cursor.full_span(),
        ));
    };
    cursor.skip_ws();
    if !cursor.is_eof() {
        return Err(CompileError::syntax(
            "Unexpected trailing content after this regex/template fragment".to_string(),
            cursor.span_from(cursor.pos),
        ));
    }
    Ok(nodes)
}

/// Parse a `/pattern/flags` regex literal. `\x` is an escape pair inside the pattern (consumes both characters, so
/// an escaped `/` does not end the pattern).
fn parse_regex_literal(cursor: &mut EmbeddedCursor<'_>) -> Result<Spanned<EmbeddedNode>, CompileError> {
    let start = cursor.pos;
    cursor.eat_str("/");
    let pattern_start = cursor.pos;
    loop {
        match cursor.peek() {
            None => {
                return Err(CompileError::syntax(
                    "Unterminated regex literal: expected a closing `/`".to_string(),
                    cursor.span_from(start),
                ));
            }
            Some('\\') => {
                cursor.advance();
                if cursor.advance().is_none() {
                    return Err(CompileError::syntax(
                        "Unterminated regex literal: dangling escape before end of fragment".to_string(),
                        cursor.span_from(start),
                    ));
                }
            }
            Some('/') => break,
            Some(_) => {
                cursor.advance();
            }
        }
    }
    let pattern = cursor.text[pattern_start..cursor.pos].to_string();
    cursor.eat_str("/");
    let flags = cursor.eat_while(|c| c.is_ascii_alphabetic()).to_string();
    Ok(Spanned::new(
        EmbeddedNode::Regex { pattern, flags },
        cursor.span_from(start),
    ))
}

/// Parse a `` `...${expr}...` `` template string into alternating `Text`/`Hole` nodes.
///
/// `` \` ``, `\$`, `\\`, and `\n` are recognized escapes inside literal text; any other `\x` preserves both
/// characters verbatim.
fn parse_template_string(cursor: &mut EmbeddedCursor<'_>) -> Result<Vec<Spanned<EmbeddedNode>>, CompileError> {
    let open_start = cursor.pos;
    cursor.eat_str("`");
    let mut nodes = Vec::new();
    let mut text = String::new();
    let mut text_start = cursor.pos;
    loop {
        match cursor.peek() {
            None => {
                return Err(CompileError::syntax(
                    "Unterminated template string: expected a closing `` ` ``".to_string(),
                    cursor.span_from(open_start),
                ));
            }
            Some('`') => {
                if !text.is_empty() {
                    nodes.push(Spanned::new(
                        EmbeddedNode::Text(std::mem::take(&mut text)),
                        cursor.span_from(text_start),
                    ));
                }
                cursor.advance();
                break;
            }
            Some('\\') => {
                cursor.advance();
                match cursor.advance() {
                    Some('`') => text.push('`'),
                    Some('$') => text.push('$'),
                    Some('\\') => text.push('\\'),
                    Some('n') => text.push('\n'),
                    Some(other) => {
                        text.push('\\');
                        text.push(other);
                    }
                    None => {
                        return Err(CompileError::syntax(
                            "Unterminated template string: dangling escape before end of fragment".to_string(),
                            cursor.span_from(open_start),
                        ));
                    }
                }
            }
            Some('$') if cursor.peek2() == Some('{') => {
                if !text.is_empty() {
                    nodes.push(Spanned::new(
                        EmbeddedNode::Text(std::mem::take(&mut text)),
                        cursor.span_from(text_start),
                    ));
                }
                let dollar_start = cursor.pos;
                cursor.advance();
                let mut hole = parse_embedded_brace_hole(cursor)?;
                hole.span = cursor.span_from(dollar_start);
                nodes.push(hole);
                text_start = cursor.pos;
            }
            Some(c) => {
                text.push(c);
                cursor.advance();
            }
        }
    }
    Ok(nodes)
}
