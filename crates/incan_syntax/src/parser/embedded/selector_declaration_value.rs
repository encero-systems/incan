// SelectorDeclarationValue submode grammar (RFC 081, `#1023`): a single declaration-value expression outside a
// full style block — dimension, color, custom-property reference, identifier, string, number, or a bare expression
// hole. This is also the shared declaration-value token grammar `style.rs` uses inside a full `{ ... }` block, per
// RFC 081's design that a declaration value means the same thing in both positions.
//
// Accepted constructs:
// - Dimension: `16px`, `2rem`, `1.5em` (a decimal number followed by a unit).
// - Color: `#1166ff`, `#fff`, `#1166ffcc` (3, 4, 6, or 8 hex digits after `#`).
// - Custom-property reference: `var(--accent-color)`.
// - Bare number: `10`, `1.5`.
// - Quoted string literal: `"sans-serif"`.
// - Bare identifier: `solid`, `center`.
// - Expression hole: `{expr}`.

/// Parse a whole `SelectorDeclarationValue`-submode fragment: exactly one value token, nothing else.
///
/// ## Errors
/// Returns a [`CompileError`] if the fragment is empty, does not start with a recognized value token, or has
/// trailing content after the single value.
fn parse_selector_declaration_value_fragment(
    raw: &str,
    base_offset: usize,
) -> Result<Vec<Spanned<EmbeddedNode>>, CompileError> {
    let mut cursor = EmbeddedCursor::new(raw, base_offset);
    cursor.skip_ws();
    let value = parse_embedded_value_token(&mut cursor)?;
    cursor.skip_ws();
    if !cursor.is_eof() {
        return Err(CompileError::syntax(
            "Unexpected trailing content after this declaration-value fragment".to_string(),
            cursor.span_from(cursor.pos),
        ));
    }
    Ok(vec![value])
}

/// Parse one declaration-value token at the cursor's current position.
///
/// Shared between `SelectorDeclarationValue` fragments and `Style` declaration values (`style.rs`).
///
/// ## Errors
/// Returns a [`CompileError`] if the current position does not start any recognized value token shape.
fn parse_embedded_value_token(cursor: &mut EmbeddedCursor<'_>) -> Result<Spanned<EmbeddedNode>, CompileError> {
    if cursor.starts_with("{") {
        return parse_embedded_brace_hole(cursor);
    }
    match cursor.peek() {
        Some('#') => parse_embedded_color(cursor),
        Some('"') | Some('\'') => parse_embedded_string_lit(cursor),
        Some(c) if c.is_ascii_digit() => parse_embedded_number_or_dimension(cursor),
        Some('.') if cursor.peek2().is_some_and(|c| c.is_ascii_digit()) => parse_embedded_number_or_dimension(cursor),
        Some(c) if is_embedded_ident_start(c) => parse_embedded_ident_or_custom_property_ref(cursor),
        _ => Err(CompileError::syntax(
            "Expected a declaration value: a dimension, color, `var(--name)` reference, identifier, string, \
             number, or `{expr}`"
                .to_string(),
            cursor.span_from(cursor.pos),
        )),
    }
}

/// Parse a `#rgb`/`#rrggbb`/(with alpha) color literal.
fn parse_embedded_color(cursor: &mut EmbeddedCursor<'_>) -> Result<Spanned<EmbeddedNode>, CompileError> {
    let start = cursor.pos;
    cursor.eat_str("#");
    let hex = cursor.eat_while(|c| c.is_ascii_hexdigit()).to_string();
    if !matches!(hex.len(), 3 | 4 | 6 | 8) {
        return Err(CompileError::syntax(
            "Color literal must have 3, 4, 6, or 8 hex digits after `#`".to_string(),
            cursor.span_from(start),
        ));
    }
    Ok(Spanned::new(
        EmbeddedNode::Value(EmbeddedValue::Color(format!("#{hex}"))),
        cursor.span_from(start),
    ))
}

/// Parse a quoted string literal value.
fn parse_embedded_string_lit(cursor: &mut EmbeddedCursor<'_>) -> Result<Spanned<EmbeddedNode>, CompileError> {
    let start = cursor.pos;
    let (_, text) = scan_quoted_body(cursor, "Unterminated string literal")?;
    Ok(Spanned::new(
        EmbeddedNode::Value(EmbeddedValue::StringLit(text.to_string())),
        cursor.span_from(start),
    ))
}

/// Parse a bare number or a number-plus-unit dimension (`16px`, `2rem`, `1.5`).
fn parse_embedded_number_or_dimension(cursor: &mut EmbeddedCursor<'_>) -> Result<Spanned<EmbeddedNode>, CompileError> {
    let start = cursor.pos;
    let mut number = cursor.eat_while(|c| c.is_ascii_digit()).to_string();
    if cursor.starts_with(".") && cursor.peek2().is_some_and(|c| c.is_ascii_digit()) {
        cursor.advance();
        number.push('.');
        number.push_str(cursor.eat_while(|c| c.is_ascii_digit()));
    }
    let unit = cursor.eat_while(|c| c.is_ascii_alphabetic() || c == '%').to_string();
    if unit.is_empty() {
        Ok(Spanned::new(
            EmbeddedNode::Value(EmbeddedValue::Number(number)),
            cursor.span_from(start),
        ))
    } else {
        Ok(Spanned::new(
            EmbeddedNode::Value(EmbeddedValue::Dimension { number, unit }),
            cursor.span_from(start),
        ))
    }
}

/// Parse a bare identifier value, or a `var(--name)` custom-property reference.
fn parse_embedded_ident_or_custom_property_ref(
    cursor: &mut EmbeddedCursor<'_>,
) -> Result<Spanned<EmbeddedNode>, CompileError> {
    let start = cursor.pos;
    let ident = cursor.eat_while(is_embedded_ident_char).to_string();
    if ident == "var" && cursor.starts_with("(") {
        cursor.eat_str("(");
        let inner = cursor.eat_while(|c| c != ')').to_string();
        if !cursor.eat_str(")") {
            return Err(CompileError::syntax(
                "Unterminated `var(...)` reference: expected `)`".to_string(),
                cursor.span_from(start),
            ));
        }
        if !inner.starts_with("--") {
            return Err(CompileError::syntax(
                "`var(...)` must reference a custom property name starting with `--`".to_string(),
                cursor.span_from(start),
            ));
        }
        return Ok(Spanned::new(
            EmbeddedNode::Value(EmbeddedValue::CustomPropertyRef(inner)),
            cursor.span_from(start),
        ));
    }
    Ok(Spanned::new(
        EmbeddedNode::Value(EmbeddedValue::Ident(ident)),
        cursor.span_from(start),
    ))
}
