//! Comment and blank-line handling: the normalized line buffer, blank lines inside and after nested suites, blank-line
//! runs clamped to two, comment-led logic blocks, leading and phase comments before wrapped statements and inside
//! nested `match` blocks, the comment-loss refusal, the comment counter, standalone comment lines, function docstrings
//! and docstring blank runs, literal `\n` sequences, duplicate comment anchors and backward-attaching comments after a
//! blank line.

use super::*;

#[test]
fn test_normalized_line_buffer_allows_double_blanks_only_at_root() {
    let mut buffer = NormalizedLineBuffer::new();
    buffer.push_line("def first() -> None:".to_string());
    buffer.push_line("    pass".to_string());
    buffer.push_line(String::new());
    buffer.push_line(String::new());
    buffer.push_line(String::new());
    buffer.push_line("    still_in_first = true".to_string());
    buffer.push_line(String::new());
    buffer.push_line(String::new());
    buffer.push_line(String::new());
    buffer.push_line("def second() -> None:".to_string());
    buffer.push_line("    pass".to_string());

    let formatted = buffer.finish(true);
    assert!(
        formatted.contains("    pass\n\n    still_in_first = true"),
        "expected one blank line inside indented code; got:\n{formatted}"
    );
    assert!(
        !formatted.contains("    pass\n\n\n    still_in_first = true"),
        "expected indented blank run to collapse below two visible blanks; got:\n{formatted}"
    );
    assert!(
        formatted.contains("    still_in_first = true\n\n\ndef second() -> None:"),
        "expected root-level blank run to allow two visible blanks; got:\n{formatted}"
    );
}

/// Single empty line between statements in a block must round-trip through the formatter.
#[test]
fn test_format_source_preserves_single_blank_line_in_function_body() -> Result<(), FormatError> {
    let source = r#"# example
def function() -> int:
    """Line one.
    Line two."""
    foo = 1

    bar = 2
"#;
    let formatted = format_source(source)?;
    assert!(
        formatted.contains("foo = 1\n\n    bar"),
        "expected one blank line between assignments; got:\n{formatted}"
    );
    Ok(())
}

/// Multiple empty lines between statements collapse to a single blank line.
#[test]
fn test_format_source_collapses_multiple_blank_lines_in_block() -> Result<(), FormatError> {
    let source = r#"def f() -> int:
    foo = 1



    bar = 2
"#;
    let formatted = format_source(source)?;
    assert!(
        !formatted.contains("\n\n\n    bar"),
        "expected at most one blank line before bar; got:\n{formatted}"
    );
    assert!(
        formatted.contains("foo = 1\n\n    bar"),
        "expected one blank line between statements; got:\n{formatted}"
    );
    Ok(())
}

/// Single empty line after a nested suite belongs to the next outer statement.
#[test]
fn test_format_source_preserves_single_blank_line_after_nested_suite() -> Result<(), FormatError> {
    let source = r#"def f(items: list[int]) -> int:
    for item in items:
        value = item

    result = 1
    return result
"#;
    let formatted = format_source(source)?;
    assert!(
        formatted.contains("        value = item\n\n    result = 1"),
        "expected one blank line after nested suite; got:\n{formatted}"
    );
    Ok(())
}

#[test]
fn test_format_source_preserves_single_blank_line_between_if_blocks_ending_in_match() -> Result<(), FormatError> {
    let source = r#"def f(a: bool, b: bool, result: Result[int, str]) -> None:
    if a:
        match result:
            Ok(_) => return
            Err(err) => return

    if b:
        match result:
            Ok(_) => return
            Err(err) => return

    z = 3
"#;
    let formatted = format_source(source)?;
    assert!(
        formatted.contains("            Err(err) => return\n\n    if b:"),
        "expected one blank line between sibling if blocks after inner match; got:\n{formatted}"
    );
    assert!(
        formatted.contains("            Err(err) => return\n\n    z = 3"),
        "expected one blank line before the trailing outer statement; got:\n{formatted}"
    );
    Ok(())
}

#[test]
fn test_format_source_clamps_blank_line_runs_to_two() -> Result<(), FormatError> {
    let source = r#"def main() -> None:
    pass



def helper() -> None:
    pass
"#;
    let formatted = format_source(source)?;
    let expected = r#"def main() -> None:
    pass


def helper() -> None:
    pass
"#;
    assert_eq!(formatted, expected);
    Ok(())
}

#[test]
fn test_format_source_preserves_single_blank_line_before_comment_led_logic_block() -> Result<(), FormatError> {
    let source = r#"def f() -> int:
    foo = 1

    # logic block
    bar = 2
"#;
    let formatted = format_source(source)?;
    let expected = r#"def f() -> int:
    foo = 1

    # logic block
    bar = 2
"#;
    assert_eq!(formatted, expected);
    Ok(())
}

#[test]
fn test_format_source_collapses_multiple_blank_lines_before_comment_led_logic_block() -> Result<(), FormatError> {
    let source = r#"def f() -> int:
    foo = 1



    # logic block
    bar = 2
"#;
    let formatted = format_source(source)?;
    let expected = r#"def f() -> int:
    foo = 1

    # logic block
    bar = 2
"#;
    assert_eq!(formatted, expected);
    Ok(())
}

#[test]
fn test_format_source_keeps_leading_comments_before_wrapped_statements() -> Result<(), FormatError> {
    let source = r#"def test_case() -> None:
    # -- Arrange --
    scenario = SubstraitConformanceScenario(scenario_id="test.multi.required.rels", title="test", status=ConformanceStatus.Core, profile_tags=[ConformanceProfileTag.ReadQueryCore], capability_tags=_test_tags(["named-table"]), root_rel=ConformanceRel.Filter, required_rels=[ConformanceRel.Read, ConformanceRel.Filter], portability=ConformancePortability.Portable, intent="test", required_rel_shape="test", expected_constraints="test", references=_test_refs(["docs/rfcs/002_apache_substrait_integration.md"]))

    # -- Act --
    named_only_plan = plan_from_named_table("orders")

    # -- Assert --
    assert_eq(scenario_matches_root_shape(scenario, named_only_plan), false, "shape validation should fail when the root relation does not match the declared root contract")
"#;
    let formatted = format_source(source)?;
    assert!(
        formatted.contains("    # -- Arrange --\n    scenario = SubstraitConformanceScenario("),
        "expected arrange comment to stay attached to wrapped constructor; got:\n{formatted}"
    );
    assert!(
        formatted.contains("    # -- Act --\n    named_only_plan = plan_from_named_table(\"orders\")"),
        "expected act comment to stay attached after preceding wrapped constructor; got:\n{formatted}"
    );
    assert!(
        formatted.contains("    # -- Assert --\n    assert_eq("),
        "expected assert comment to stay attached to wrapped assertion; got:\n{formatted}"
    );
    assert!(
        !formatted.contains("    )\n\n    named_only_plan = plan_from_named_table(\"orders\")\n\n    assert_eq("),
        "formatter stranded phase comments after wrapped anchors; got:\n{formatted}"
    );
    Ok(())
}

#[test]
fn test_format_source_keeps_phase_comments_attached_inside_nested_match_blocks() -> Result<(), FormatError> {
    let source = r#"def test_session_backend_datafusion__registered_named_table_executes_via_substrait() -> None:
    # -- Arrange --
    mut session = Session.default()
    fixture_uri = "../../../tests/fixtures/orders.csv"
    match session.register("orders", csv_source(fixture_uri)):
        Ok(_) =>
            pass
        Err(err) => assert_eq(true, false, err.error_message())

    # -- Act --
    match session.table[Order]("orders"):
        Ok(lazy) =>
            match session.execute(lazy):
                Ok(_) =>
                    # -- Assert --
                    pass
                Err(err) => assert_eq(true, false, err.error_message())

        Err(err) => assert_eq(true, false, err.error_message())


def test_session_backend_datafusion__session_write_csv_routes_through_execution_path() -> None:
    # -- Arrange --
    mut session = Session.default()
    output_uri = "../../../tests/target/session_backend_datafusion_output.csv"
    fixture_uri = "../../../tests/fixtures/orders.csv"
    match session.register("orders", csv_source(fixture_uri)):
        Ok(_) =>
            pass
        Err(err) => assert_eq(true, false, err.error_message())

    # -- Act --
    match session.table[Order]("orders"):
        Ok(lazy) =>
            match session.write_csv(lazy, output_uri):
                Ok(_) =>
                    # -- Assert --
                    assert_eq(Path.new(output_uri).exists(), true, "session.write_csv should produce an output artifact")
                Err(err) => assert_eq(true, false, err.error_message())

        Err(err) => assert_eq(true, false, err.error_message())
"#;

    let formatted = format_source(source)?;
    assert!(
        formatted.contains("    # -- Arrange --\n    mut session = Session.default()"),
        "expected arrange comment to stay attached to the outer setup statement; got:\n{formatted}"
    );
    assert!(
        formatted.contains("    # -- Act --\n    match session.table[Order](\"orders\"):"),
        "expected act comment to stay attached to the outer action match; got:\n{formatted}"
    );
    assert!(
        formatted
            .contains("                Ok(_) =>\n                    # -- Assert --\n                    assert_eq("),
        "expected assert comment to stay attached inside the nested Ok arm; got:\n{formatted}"
    );
    assert!(
        !formatted.contains(
            "        Err(err) => assert_eq(true, false, err.error_message())\n                    # -- Assert --"
        ),
        "formatter stranded assert comment after the outer Err arm; got:\n{formatted}"
    );
    assert!(
        !formatted.contains("    # -- Arrange --\n\n    # -- Act --\n                    # -- Assert --"),
        "formatter floated phase comments to the end of the function; got:\n{formatted}"
    );

    let reformatted = format_source(&formatted)?;
    assert_eq!(
        reformatted, formatted,
        "formatter must stay idempotent for nested match phase comments"
    );
    Ok(())
}

#[test]
fn test_format_source_refuses_comment_loss_inline_comment() -> Result<(), FormatError> {
    let source = r#"def foo() -> int:
  x = 1  # keep this comment
  return x
"#;
    let formatted = format_source(source)?;
    assert!(
        formatted.contains("# keep this comment"),
        "expected inline comment to survive formatting; got: {formatted}"
    );
    Ok(())
}

#[test]
fn test_comment_counter_ignores_hash_in_string_literals() {
    let source = r##"def foo() -> str:
  return "# not a comment"
"##;
    assert_eq!(count_line_comments(source), 0);
}

#[test]
fn test_format_source_preserves_standalone_comment_lines() -> Result<(), FormatError> {
    let source = r#"const A: int = 1
# ---- marker comment ----
const B: int = 2
"#;
    let formatted = format_source(source)?;
    assert!(
        formatted.contains("# ---- marker comment ----"),
        "expected standalone comment to survive formatting; got: {formatted}"
    );
    Ok(())
}

#[test]
fn test_format_source_preserves_function_docstring_statement() -> Result<(), FormatError> {
    let source = r#"def greet() -> str:
    """Return a greeting."""
    return "hi"
"#;
    let formatted = format_source(source)?;
    assert_eq!(formatted, source);

    let methods_only = r#""""Module docstring."""


enum Marker:
    """Marker enum docstring."""

    def label(self) -> str:
        return "marker"
"#;
    let formatted = format_source(methods_only)?;
    assert_eq!(formatted, methods_only);
    Ok(())
}

#[test]
fn test_format_source_collapses_docstring_interior_blank_runs() -> Result<(), FormatError> {
    let source = r#"def explain() -> str:
    """
    First paragraph.


    Second paragraph.
    """
    return "ok"
"#;
    let formatted = format_source(source)?;
    let expected = r#"def explain() -> str:
    """
    First paragraph.

    Second paragraph.
    """
    return "ok"
"#;
    assert_eq!(formatted, expected);
    Ok(())
}

#[test]
fn test_format_source_collapses_many_docstring_blank_lines_but_preserves_slash_n_text() -> Result<(), FormatError> {
    let source = r#"def explain() -> str:
    """
    some docstring with a bunch of /n/n/n/ text





    inside it
    """
    return "ok"
"#;
    let formatted = format_source(source)?;
    let expected = r#"def explain() -> str:
    """
    some docstring with a bunch of /n/n/n/ text

    inside it
    """
    return "ok"
"#;
    assert_eq!(formatted, expected);
    Ok(())
}

#[test]
fn test_format_source_preserves_literal_slash_n_sequences() -> Result<(), FormatError> {
    let source = r#"def explain() -> str:
    # /n/n/n/n this is valid
    value = "/n/n/n/n and so is this"
    return value
"#;
    let formatted = format_source(source)?;
    assert_eq!(formatted, source);
    Ok(())
}

#[test]
fn test_format_source_preserves_duplicate_comment_anchors() -> Result<(), FormatError> {
    let source = r#"# ---- first ----
@derive(Clone)
type First = newtype int


@derive(Clone)
type Middle = newtype int


# ---- second ----
@derive(Clone)
type Second = newtype int
"#;
    let formatted = format_source(source)?;
    let expected = r#"# ---- first ----
@derive(Clone)
type First = newtype int


@derive(Clone)
type Middle = newtype int


# ---- second ----
@derive(Clone)
type Second = newtype int
"#;
    assert_eq!(formatted, expected);
    Ok(())
}

#[test]
fn test_format_source_blank_line_separated_comment_attaches_backward() -> Result<(), FormatError> {
    let source = r#"type UserId = str
# comment about the alias

def load_user(id: UserId) -> User:
    pass
"#;
    let formatted = format_source(source)?;
    let expected = r#"type UserId = str
# comment about the alias


def load_user(id: UserId) -> User:
    pass
"#;
    assert_eq!(formatted, expected);
    Ok(())
}

#[test]
fn test_format_source_trailing_comment_after_multiline_function_stays_after_suite() -> Result<(), FormatError> {
    let source = r#"def load_user(id: UserId) -> User:
    pass

# TODO: split retries
"#;
    let formatted = format_source(source)?;
    let expected = r#"def load_user(id: UserId) -> User:
    pass
# TODO: split retries
"#;
    assert_eq!(formatted, expected);
    let _ = program_from_source(&formatted)?;
    Ok(())
}
