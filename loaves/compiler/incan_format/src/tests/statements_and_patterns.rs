//! Statement and pattern formatting: short match arm blocks, pattern alternation and its wrapping, the blank line after
//! a match arm arrow or an `elif` / `else` header, blank lines around multi-line `match` statements, qualified
//! constructor patterns, `if let` / `while let` headers and bodies, and the bare right side of a tuple statement.

use super::*;

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

/// Formatting loop control arms must preserve parser validity and reach a fixed point.
#[test]
fn continue_match_arm_stays_parseable_and_idempotent() -> Result<(), FormatError> {
    for statement in ["continue", "break", "break 1", "return", "pass"] {
        let source = format!(
            "def main() -> None:\n    for value in range(3):\n        match value:\n            _ =>\n                {statement}\n"
        );
        let formatted = assert_format_round_trip_lex_parse(&source)?;
        assert_eq!(format_source(&formatted)?, formatted);
        assert!(formatted.contains(&format!("_ => {statement}")), "{formatted}");
    }
    Ok(())
}

#[test]
fn statement_and_pattern_formatting_cases() {
    run_format_cases(STATEMENT_AND_PATTERN_CASES);
}

const STATEMENT_AND_PATTERN_CASES: &[FormatCase] = &[
    FormatCase {
        name: "test_format_source_preserves_short_match_arm_blocks_inline",
        source: r#"def authored_node_kind_name(node: PrismNode) -> str:
    match node.kind:
        PrismNodeKind.ReadNamedTable => return str("ReadNamedTable")
        PrismNodeKind.Filter => return str("Filter")
"#,
        checks: &[
            Check::Has(r#"        PrismNodeKind.ReadNamedTable => return str("ReadNamedTable")"#),
            Check::Lacks("PrismNodeKind.ReadNamedTable => \n"),
        ],
        ..FormatCase::DEFAULT
    },
    FormatCase {
        name: "test_format_source_preserves_pattern_alternation",
        source: r#"def authored_node_kind_name(node: PrismNode) -> str:
    match node.kind:
        PrismNodeKind.Filter|PrismNodeKind.OrderBy|_=>return str("passthrough")
"#,
        checks: &[Check::Is(
            r#"def authored_node_kind_name(node: PrismNode) -> str:
    match node.kind:
        PrismNodeKind.Filter | PrismNodeKind.OrderBy | _ => return str("passthrough")
"#,
        )],
        ..FormatCase::DEFAULT
    },
    FormatCase {
        name: "test_format_source_wraps_long_pattern_alternation",
        source: r#"def authored_node_kind_name(node: PrismNode) -> str:
    match node.kind:
        PrismNodeKind.FilterStageWithLongName | PrismNodeKind.OrderByStageWithLongName | PrismNodeKind.LimitStageWithLongName => return str("passthrough")
"#,
        line_length: Some(80),
        checks: &[
            Check::Is(
                r#"def authored_node_kind_name(node: PrismNode) -> str:
    match node.kind:
        (
            PrismNodeKind.FilterStageWithLongName
            | PrismNodeKind.OrderByStageWithLongName
            | PrismNodeKind.LimitStageWithLongName
        ) => return str("passthrough")
"#,
            ),
            Check::Stable,
        ],
        ..FormatCase::DEFAULT
    },
    FormatCase {
        name: "test_format_source_normalizes_blank_after_match_arm_arrow",
        source: r#"def f(result: Result[int, str]) -> int:
    match result:
        Ok(value) =>
            value = value + 1
            return value
        Err(err) =>

            message = err
            return 0
"#,
        checks: &[
            Check::Has("        Err(err) =>\n            message = err\n            return 0"),
            Check::Lacks("=> \n"),
        ],
        ..FormatCase::DEFAULT
    },
    // A blank after an arrow is removed when its single assignment fits inline.
    FormatCase {
        name: "test_format_source_normalizes_blank_after_match_arm_arrow_before_statement",
        source: r#"def f(result: Result[int, str]) -> None:
    match result:
        Ok(_) => pass
        Err(err) =>

            message = err
"#,
        checks: &[
            Check::Has("        Err(err) => message = err"),
            Check::Lacks("        Err(err) =>\n\n            message = err"),
            Check::Stable,
        ],
        ..FormatCase::DEFAULT
    },
    FormatCase {
        name: "test_format_source_normalizes_blank_after_elif_and_else_headers",
        source: r#"def f(kind: str) -> int:
    if kind == "a":
        return 1
    elif kind == "b":

        return 2
    else:

        return 3
"#,
        checks: &[
            Check::Has("    elif kind == \"b\":\n        return 2"),
            Check::Has("    else:\n        return 3"),
        ],
        ..FormatCase::DEFAULT
    },
    // Joining a short assignment arm still leaves exactly one blank line between match statements.
    FormatCase {
        name: "test_format_source_does_not_double_space_after_multiline_match_statement",
        source: r#"def f(first: Result[int, str], second: Result[int, str]) -> None:
    match first:
        Ok(_) => pass
        Err(err) =>
            message = err


    match second:
        Ok(_) => pass
        Err(err) =>
            message = err
"#,
        checks: &[
            Check::Has("        Err(err) => message = err\n\n    match second:"),
            Check::Lacks("        Err(err) => message = err\n\n\n    match second:"),
            Check::Stable,
        ],
        ..FormatCase::DEFAULT
    },
    // Regression #235: qualified constructor patterns use `::` in the AST; the formatter must print Incan surface `.`.
    FormatCase {
        name: "test_format_source_qualified_match_pattern_round_trip",
        source: r#"def f(x: int) -> int:
    match x:
        E.V =>
            return 1
"#,
        checks: &[Check::Parses, Check::Has("E.V"), Check::Lacks("E::V")],
        ..FormatCase::DEFAULT
    },
    // Regression (GitHub #235): qualified constructor patterns with payloads must also round-trip.
    FormatCase {
        name: "test_format_source_qualified_match_pattern_with_args_round_trip",
        source: r#"def f(x: int) -> int:
    match x:
        E.V(y) =>
            return y
"#,
        checks: &[Check::Parses, Check::Has("E.V("), Check::Has("y"), Check::Lacks("E::V")],
        ..FormatCase::DEFAULT
    },
    FormatCase {
        name: "test_format_source_if_let_round_trip",
        source: r#"def first(opt: Option[int]) -> int:
    if let Some(value) = opt:
        return value
    return 0
"#,
        checks: &[Check::Parses, Check::Has("if let Some(value) = opt:")],
        ..FormatCase::DEFAULT
    },
    FormatCase {
        name: "test_format_source_while_let_round_trip",
        source: r#"def sum_once(opt: Option[int]) -> int:
    mut total = 0
    mut current = opt
    while let Some(value) = current:
        total = total + value
        current = None
    return total
"#,
        checks: &[Check::Parses, Check::Has("while let Some(value) = current:")],
        ..FormatCase::DEFAULT
    },
    FormatCase {
        name: "test_format_source_if_let_normalizes_header_and_body_indentation",
        source: "def first(opt: Option[int]) -> int:\n  if let Some(value)=opt:\n   return value\n  return 0\n",
        checks: &[Check::Is(
            r#"def first(opt: Option[int]) -> int:
    if let Some(value) = opt:
        return value
    return 0
"#,
        )],
        ..FormatCase::DEFAULT
    },
    FormatCase {
        name: "test_format_source_while_let_normalizes_header_and_body_indentation",
        source: "def sum_once(opt: Option[int]) -> int:\n  mut total=0\n  mut current=opt\n  while let Some(value)=current:\n   total=total+value\n   current=None\n  return total\n",
        checks: &[Check::Is(
            r#"def sum_once(opt: Option[int]) -> int:
    mut total = 0
    mut current = opt
    while let Some(value) = current:
        total = total + value
        current = None
    return total
"#,
        )],
        ..FormatCase::DEFAULT
    },
    FormatCase {
        name: "test_format_source_writes_a_tuple_statement_value_bare_issue1789",
        source: r#"def swap() -> None:
    mut a = 1
    mut b = 2
    a, b = (b, a)
    mut items = [1, 2]
    items[0], items[1] = (items[1], items[0])
    pair = (a, b)
"#,
        checks: &[
            Check::Is(
                r#"def swap() -> None:
    mut a = 1
    mut b = 2
    a, b = b, a
    mut items = [1, 2]
    items[0], items[1] = items[1], items[0]
    pair = (a, b)
"#,
            ),
            Check::Stable,
        ],
        ..FormatCase::DEFAULT
    },
];
