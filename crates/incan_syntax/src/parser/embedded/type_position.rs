// TypePosition submode grammar (RFC 081, `#1023`): a minimal, representative type-shaped grammar —
// namespace-qualified names, generics, nullable, array, and union — not a full external type-system grammar. See
// `EmbeddedTypeShape`'s rustdoc for the exact node shapes this produces.
//
// Grammar (informal):
//   type      := union
//   union     := postfix ('|' postfix)*
//   postfix   := primary ('?' | '[]')*
//   primary   := NAME ('.' NAME)* ('<' type (',' type)* '>')?

/// Parse a whole `TypePosition`-submode fragment: exactly one type shape.
///
/// ## Errors
/// Returns a [`CompileError`] if the fragment is empty, malformed, or has trailing content after the type shape.
fn parse_type_position_fragment(raw: &str, base_offset: usize) -> Result<Vec<Spanned<EmbeddedNode>>, CompileError> {
    let mut cursor = EmbeddedCursor::new(raw, base_offset);
    cursor.skip_ws();
    let shape = parse_type_union(&mut cursor)?;
    cursor.skip_ws();
    if !cursor.is_eof() {
        return Err(CompileError::syntax(
            "Unexpected trailing content after this type-position fragment".to_string(),
            cursor.span_from(cursor.pos),
        ));
    }
    Ok(vec![Spanned::new(EmbeddedNode::TypeShape(shape), cursor.full_span())])
}

/// Parse a `|`-separated union of postfix type shapes; collapses to the single member when there is only one.
fn parse_type_union(cursor: &mut EmbeddedCursor<'_>) -> Result<EmbeddedTypeShape, CompileError> {
    let mut members = vec![parse_type_postfix(cursor)?];
    loop {
        cursor.skip_ws();
        if !cursor.eat_str("|") {
            break;
        }
        cursor.skip_ws();
        members.push(parse_type_postfix(cursor)?);
    }
    if members.len() == 1 {
        let Some(only) = members.pop() else {
            return Err(CompileError::syntax(
                "Internal error: expected exactly one type-union member".to_string(),
                cursor.full_span(),
            ));
        };
        Ok(only)
    } else {
        Ok(EmbeddedTypeShape::Union(members))
    }
}

/// Parse a primary type shape followed by any number of `?` (nullable) or `[]` (array) postfix markers.
fn parse_type_postfix(cursor: &mut EmbeddedCursor<'_>) -> Result<EmbeddedTypeShape, CompileError> {
    let mut shape = parse_type_primary(cursor)?;
    loop {
        if cursor.eat_str("[]") {
            shape = EmbeddedTypeShape::Array(Box::new(shape));
        } else if cursor.eat_str("?") {
            shape = EmbeddedTypeShape::Nullable(Box::new(shape));
        } else {
            break;
        }
    }
    Ok(shape)
}

/// Parse a namespace-qualified name, optionally followed by a `<...>` generic argument list.
fn parse_type_primary(cursor: &mut EmbeddedCursor<'_>) -> Result<EmbeddedTypeShape, CompileError> {
    let start = cursor.pos;
    let mut segments = vec![parse_type_name_segment(cursor)?];
    while cursor.starts_with(".") {
        cursor.eat_str(".");
        segments.push(parse_type_name_segment(cursor)?);
    }
    let mut shape = EmbeddedTypeShape::Name(segments);

    if cursor.eat_str("<") {
        cursor.skip_ws();
        let mut args = vec![parse_type_union(cursor)?];
        loop {
            cursor.skip_ws();
            if !cursor.eat_str(",") {
                break;
            }
            cursor.skip_ws();
            args.push(parse_type_union(cursor)?);
        }
        cursor.skip_ws();
        if !cursor.eat_str(">") {
            return Err(CompileError::syntax(
                "Expected `>` to close this generic argument list".to_string(),
                cursor.span_from(start),
            ));
        }
        shape = EmbeddedTypeShape::Generic(Box::new(shape), args);
    }

    Ok(shape)
}

/// Parse one `.`-separated name segment.
fn parse_type_name_segment(cursor: &mut EmbeddedCursor<'_>) -> Result<String, CompileError> {
    let start = cursor.pos;
    let name = cursor.eat_while(|c| c.is_ascii_alphanumeric() || c == '_').to_string();
    if name.is_empty() {
        return Err(CompileError::syntax(
            "Expected a type name".to_string(),
            cursor.span_from(start),
        ));
    }
    Ok(name)
}
