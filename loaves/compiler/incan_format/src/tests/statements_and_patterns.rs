//! Statement and pattern formatting: short match arm blocks, pattern alternation and its wrapping, the blank line after
//! a match arm arrow or an `elif` / `else` header, blank lines around multi-line `match` statements, qualified
//! constructor patterns, `if let` / `while let` headers and bodies, and the bare right side of a tuple statement.

use super::*;

#[test]
fn test_format_source_preserves_short_match_arm_blocks_inline() -> Result<(), FormatError> {
    let source = r#"def authored_node_kind_name(node: PrismNode) -> str:
    match node.kind:
        PrismNodeKind.ReadNamedTable => return str("ReadNamedTable")
        PrismNodeKind.Filter => return str("Filter")
"#;
    let formatted = format_source(source)?;
    assert!(
        formatted.contains(r#"        PrismNodeKind.ReadNamedTable => return str("ReadNamedTable")"#),
        "expected short return arm to stay inline; got:\n{formatted}"
    );
    assert!(
        !formatted.contains("PrismNodeKind.ReadNamedTable => \n"),
        "formatter should not split short return arm after fat arrow; got:\n{formatted}"
    );
    Ok(())
}

#[test]
fn test_format_source_preserves_pattern_alternation() -> Result<(), FormatError> {
    let source = r#"def authored_node_kind_name(node: PrismNode) -> str:
    match node.kind:
        PrismNodeKind.Filter|PrismNodeKind.OrderBy|_=>return str("passthrough")
"#;
    let expected = r#"def authored_node_kind_name(node: PrismNode) -> str:
    match node.kind:
        PrismNodeKind.Filter | PrismNodeKind.OrderBy | _ => return str("passthrough")
"#;
    assert_eq!(format_source(source)?, expected);
    Ok(())
}

#[test]
fn test_format_source_wraps_long_pattern_alternation() -> Result<(), FormatError> {
    let source = r#"def authored_node_kind_name(node: PrismNode) -> str:
    match node.kind:
        PrismNodeKind.FilterStageWithLongName | PrismNodeKind.OrderByStageWithLongName | PrismNodeKind.LimitStageWithLongName => return str("passthrough")
"#;
    let expected = r#"def authored_node_kind_name(node: PrismNode) -> str:
    match node.kind:
        (
            PrismNodeKind.FilterStageWithLongName
            | PrismNodeKind.OrderByStageWithLongName
            | PrismNodeKind.LimitStageWithLongName
        ) => return str("passthrough")
"#;
    let config = FormatConfig::new().with_line_length(80);
    assert_eq!(format_source_with_config(source, config.clone())?, expected);
    assert_eq!(format_source_with_config(expected, config)?, expected);
    Ok(())
}

#[test]
fn test_format_source_normalizes_blank_after_match_arm_arrow() -> Result<(), FormatError> {
    let source = r#"def f(result: Result[int, str]) -> int:
    match result:
        Ok(value) =>
            value = value + 1
            return value
        Err(err) =>

            message = err
            return 0
"#;
    let formatted = format_source(source)?;
    assert!(
        formatted.contains("        Err(err) =>\n            message = err\n            return 0"),
        "expected blank after match arm arrow to be removed; got:\n{formatted}"
    );
    assert!(
        !formatted.contains("=> \n"),
        "block match arms should not carry trailing whitespace after the arrow; got:\n{formatted}"
    );
    Ok(())
}

#[test]
fn test_format_source_normalizes_blank_after_match_arm_arrow_before_statement() -> Result<(), FormatError> {
    let source = r#"def f(result: Result[int, str]) -> None:
    match result:
        Ok(_) => pass
        Err(err) =>

            message = err
"#;
    let formatted = format_source(source)?;
    assert!(
        formatted.contains("        Err(err) =>\n            message = err"),
        "expected blank after match arm arrow before statement to be removed; got:\n{formatted}"
    );
    assert!(
        !formatted.contains("        Err(err) =>\n\n            message = err"),
        "formatter left an empty line between match arm arrow and statement body; got:\n{formatted}"
    );
    Ok(())
}

#[test]
fn test_format_source_match_arm_body_does_not_inherit_blank_line_after_nested_arm() -> Result<(), FormatError> {
    let source = r#"def f(result: Result[int, str]) -> int:
    match result:
        Ok(value) => match value:
            Ready(x) => return x

            Failed(err) => return 0

        Err(err) =>
            return Err(problem.report_with_context())
"#;
    let formatted = format_source(source)?;
    assert!(
        formatted.contains("        Err(err) => return Err(problem.report_with_context())")
            || formatted.contains("        Err(err) =>\n            return Err(problem.report_with_context())"),
        "expected outer Err arm body to stay tight after nested-arm spacing; got:\n{formatted}"
    );
    assert!(
        !formatted.contains("        Err(err) =>\n\n            return Err(problem.report_with_context())"),
        "formatter leaked a preserved outer blank line into the Err arm body; got:\n{formatted}"
    );
    Ok(())
}

#[test]
fn test_format_source_normalizes_blank_after_elif_and_else_headers() -> Result<(), FormatError> {
    let source = r#"def f(kind: str) -> int:
    if kind == "a":
        return 1
    elif kind == "b":

        return 2
    else:

        return 3
"#;
    let formatted = format_source(source)?;
    assert!(
        formatted.contains("    elif kind == \"b\":\n        return 2"),
        "expected blank after elif header to be removed; got:\n{formatted}"
    );
    assert!(
        formatted.contains("    else:\n        return 3"),
        "expected blank after else header to be removed; got:\n{formatted}"
    );
    Ok(())
}

#[test]
fn test_format_source_does_not_double_space_after_multiline_match_statement() -> Result<(), FormatError> {
    let source = r#"def f(first: Result[int, str], second: Result[int, str]) -> None:
    match first:
        Ok(_) => pass
        Err(err) =>
            message = err


    match second:
        Ok(_) => pass
        Err(err) =>
            message = err
"#;
    let formatted = format_source(source)?;
    assert!(
        formatted.contains("            message = err\n\n    match second:"),
        "expected exactly one blank line after multiline match statement; got:\n{formatted}"
    );
    assert!(
        !formatted.contains("            message = err\n\n\n    match second:"),
        "formatter emitted two blank lines after multiline match statement; got:\n{formatted}"
    );
    Ok(())
}

/// Regression #235: qualified constructor patterns use `::` in the AST; the formatter must print Incan surface `.`.
#[test]
fn test_format_source_qualified_match_pattern_round_trip() -> Result<(), FormatError> {
    let source = r#"def f(x: int) -> int:
    match x:
        E.V =>
            return 1
"#;
    let formatted = assert_format_round_trip_lex_parse(source)?;
    assert!(
        formatted.contains("E.V"),
        "expected dot-qualified pattern in output; got: {formatted}"
    );
    assert!(
        !formatted.contains("E::V"),
        "formatter must not emit internal :: spelling for match patterns; got: {formatted}"
    );
    Ok(())
}

/// Regression (GitHub #235): qualified constructor patterns with payloads must also round-trip.
#[test]
fn test_format_source_qualified_match_pattern_with_args_round_trip() -> Result<(), FormatError> {
    let source = r#"def f(x: int) -> int:
    match x:
        E.V(y) =>
            return y
"#;
    let formatted = assert_format_round_trip_lex_parse(source)?;
    assert!(
        formatted.contains("E.V(") && formatted.contains("y"),
        "expected dot-qualified pattern with args in output; got: {formatted}"
    );
    assert!(
        !formatted.contains("E::V"),
        "formatter must not emit internal :: spelling for match patterns; got: {formatted}"
    );
    Ok(())
}

#[test]
fn test_format_source_if_let_round_trip() -> Result<(), FormatError> {
    let source = r#"def first(opt: Option[int]) -> int:
    if let Some(value) = opt:
        return value
    return 0
"#;
    let formatted = assert_format_round_trip_lex_parse(source)?;
    assert!(
        formatted.contains("if let Some(value) = opt:"),
        "expected formatter to preserve if-let header; got: {formatted}"
    );
    Ok(())
}

#[test]
fn test_format_source_while_let_round_trip() -> Result<(), FormatError> {
    let source = r#"def sum_once(opt: Option[int]) -> int:
    mut total = 0
    mut current = opt
    while let Some(value) = current:
        total = total + value
        current = None
    return total
"#;
    let formatted = assert_format_round_trip_lex_parse(source)?;
    assert!(
        formatted.contains("while let Some(value) = current:"),
        "expected formatter to preserve while-let header; got: {formatted}"
    );
    Ok(())
}

#[test]
fn test_format_source_if_let_normalizes_header_and_body_indentation() -> Result<(), FormatError> {
    let source = "def first(opt: Option[int]) -> int:\n  if let Some(value)=opt:\n   return value\n  return 0\n";
    let formatted = format_source(source)?;
    let expected = r#"def first(opt: Option[int]) -> int:
    if let Some(value) = opt:
        return value
    return 0
"#;
    assert_eq!(formatted, expected);
    Ok(())
}

#[test]
fn test_format_source_while_let_normalizes_header_and_body_indentation() -> Result<(), FormatError> {
    let source = "def sum_once(opt: Option[int]) -> int:\n  mut total=0\n  mut current=opt\n  while let Some(value)=current:\n   total=total+value\n   current=None\n  return total\n";
    let formatted = format_source(source)?;
    let expected = r#"def sum_once(opt: Option[int]) -> int:
    mut total = 0
    mut current = opt
    while let Some(value) = current:
        total = total + value
        current = None
    return total
"#;
    assert_eq!(formatted, expected);
    Ok(())
}

#[test]
fn test_format_source_writes_a_tuple_statement_value_bare_issue1789() -> Result<(), FormatError> {
    // The right side of a tuple unpacking or tuple assignment is written bare, `a, b = b, a`, which parses to the same
    // tuple; a tuple anywhere else keeps its parentheses.
    let source = r#"def swap() -> None:
    mut a = 1
    mut b = 2
    a, b = (b, a)
    mut items = [1, 2]
    items[0], items[1] = (items[1], items[0])
    pair = (a, b)
"#;
    let expected = r#"def swap() -> None:
    mut a = 1
    mut b = 2
    a, b = b, a
    mut items = [1, 2]
    items[0], items[1] = items[1], items[0]
    pair = (a, b)
"#;
    assert_eq!(format_source(source)?, expected);
    assert_eq!(format_source(expected)?, expected);
    Ok(())
}
