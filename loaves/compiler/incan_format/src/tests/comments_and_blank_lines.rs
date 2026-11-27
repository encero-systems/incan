//! Comment and blank-line handling: the normalized line buffer, blank lines inside and after nested suites, blank-line
//! runs clamped to two, comment-led logic blocks, leading and phase comments before wrapped statements and inside
//! nested `match` blocks, the comment-loss refusal, the comment counter, standalone comment lines, function docstrings
//! and docstring blank runs, literal `\n` sequences, duplicate comment anchors and backward-attaching comments after a
//! blank line.

use super::*;

/// Leading trivia starts the file with or without a module docstring, including gaps and comments before imports.
#[test]
fn test_format_source_preserves_comments_around_module_docstring() -> Result<(), FormatError> {
    for source in [
        "# A comment-only module.\n# Another comment.\n",
        "# A comment-only module.\n\n# A separate block.\n",
        "# A leading comment.\n# Another comment.\n\"\"\"Module documentation.\"\"\"\n",
        "# A leading comment.\n# Another comment.\n\n\"\"\"Module documentation.\"\"\"\n",
        "# A leading comment.\n\n\"\"\"Module documentation.\"\"\"\n\n# Import comment.\nimport rust::std::fmt\n",
        "def main() -> None:\n    # A leading suite comment.\n\n    pass\n",
        "# A file banner.\n\n# A module comment.\n\n\"\"\"Module documentation.\"\"\"\n",
        r#"# behavior: a header stays before a module docstring
# expect-accepted:

"""A module with a contract header."""


def main() -> None:
    """Keep the file executable."""
    pass
"#,
    ] {
        let formatted = format_source(source)?;
        assert_eq!(formatted, source);
        assert_eq!(format_source(&formatted)?, formatted);
    }
    let source = "# A file banner.\n\nfrom rust::std::fmt import Debug\nfrom rust::std::fmt import Display\n";
    let expected = "# A file banner.\n\nfrom rust::std::fmt import Debug, Display\n";
    let formatted = format_source(source)?;
    assert_eq!(formatted, expected);
    assert_eq!(format_source(&formatted)?, formatted);
    Ok(())
}

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
fn test_comment_counter_ignores_hash_in_string_literals() {
    let source = r##"def foo() -> str:
  return "# not a comment"
"##;
    assert_eq!(count_line_comments(source), 0);
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

/// A leading section comment takes declaration spacing; its following source gap stays attached.
#[test]
fn test_top_level_comment_after_two_paragraph_docstring() -> Result<(), FormatError> {
    let source = r#"def count() -> int:
    """
    First paragraph.

    Second paragraph.
    """
    return 0


# Section comment


def other() -> int:
    return 1
"#;
    let formatted = format_source(source)?;
    assert_eq!(formatted, source);
    assert_eq!(count_line_comments(&formatted), 1);
    assert_eq!(format_source(&formatted)?, formatted);
    for before in 0..=2 {
        for after in 0..=2 {
            let gap = format!("{}# Section comment{}", "\n".repeat(before + 1), "\n".repeat(after + 1));
            let variant = source.replace("\n\n\n# Section comment\n\n\n", &gap);
            let formatted = format_source(&variant)?;
            let expected_gap = format!("\n\n\n# Section comment{}", "\n".repeat(after + 1));
            let expected = source.replace("\n\n\n# Section comment\n\n\n", &expected_gap);
            assert_eq!(formatted, expected, "gaps: before={before}, after={after}");
            assert_eq!(format_source(&formatted)?, formatted);
        }
    }
    Ok(())
}

/// Root-level section comments retain both gaps after a function with nested match.
#[test]
fn test_top_level_comment_after_nested_match() -> Result<(), FormatError> {
    let source = r#"def count(items: list[int]) -> int:
    mut total = 0
    for item in items:
        match item:
            0 => total += 1
            _ => pass
    return total


# A comment after a function whose body ends in a nested for and match.


def other() -> int:
    return 1
"#;
    for source in [source.to_string(), source.replace("    return total\n", "")] {
        let formatted = format_source(&source)?;
        assert_eq!(formatted, source);
        assert_eq!(count_line_comments(&formatted), 1);
        assert_eq!(format_source(&formatted)?, formatted);
    }
    Ok(())
}

#[test]
fn comment_and_blank_line_formatting_cases() {
    run_format_cases(COMMENT_AND_BLANK_LINE_CASES);
}

const COMMENT_AND_BLANK_LINE_CASES: &[FormatCase] = &[
    // Single empty line between statements in a block must round-trip through the formatter.
    FormatCase {
        name: "test_format_source_preserves_single_blank_line_in_function_body",
        source: r#"# example
def function() -> int:
    """Line one.
    Line two."""
    foo = 1

    bar = 2
"#,
        checks: &[Check::Has("foo = 1\n\n    bar")],
        ..FormatCase::DEFAULT
    },
    // Multiple empty lines between statements collapse to a single blank line.
    FormatCase {
        name: "test_format_source_collapses_multiple_blank_lines_in_block",
        source: r#"def f() -> int:
    foo = 1



    bar = 2
"#,
        checks: &[Check::Lacks("\n\n\n    bar"), Check::Has("foo = 1\n\n    bar")],
        ..FormatCase::DEFAULT
    },
    // Single empty line after a nested suite belongs to the next outer statement.
    FormatCase {
        name: "test_format_source_preserves_single_blank_line_after_nested_suite",
        source: r#"def f(items: list[int]) -> int:
    for item in items:
        value = item

    result = 1
    return result
"#,
        checks: &[Check::Has("        value = item\n\n    result = 1")],
        ..FormatCase::DEFAULT
    },
    FormatCase {
        name: "test_format_source_preserves_single_blank_line_between_if_blocks_ending_in_match",
        source: r#"def f(a: bool, b: bool, result: Result[int, str]) -> None:
    if a:
        match result:
            Ok(_) => return
            Err(err) => return

    if b:
        match result:
            Ok(_) => return
            Err(err) => return

    z = 3
"#,
        checks: &[
            Check::Has("            Err(err) => return\n\n    if b:"),
            Check::Has("            Err(err) => return\n\n    z = 3"),
        ],
        ..FormatCase::DEFAULT
    },
    FormatCase {
        name: "test_format_source_clamps_blank_line_runs_to_two",
        source: r#"def main() -> None:
    pass



def helper() -> None:
    pass
"#,
        checks: &[Check::Is(
            r#"def main() -> None:
    pass


def helper() -> None:
    pass
"#,
        )],
        ..FormatCase::DEFAULT
    },
    FormatCase {
        name: "test_format_source_preserves_single_blank_line_before_comment_led_logic_block",
        source: r#"def f() -> int:
    foo = 1

    # logic block
    bar = 2
"#,
        checks: &[Check::Is(
            r#"def f() -> int:
    foo = 1

    # logic block
    bar = 2
"#,
        )],
        ..FormatCase::DEFAULT
    },
    FormatCase {
        name: "test_format_source_collapses_multiple_blank_lines_before_comment_led_logic_block",
        source: r#"def f() -> int:
    foo = 1



    # logic block
    bar = 2
"#,
        checks: &[Check::Is(
            r#"def f() -> int:
    foo = 1

    # logic block
    bar = 2
"#,
        )],
        ..FormatCase::DEFAULT
    },
    FormatCase {
        name: "test_format_source_keeps_leading_comments_before_wrapped_statements",
        source: r#"def test_case() -> None:
    # -- Arrange --
    scenario = SubstraitConformanceScenario(scenario_id="test.multi.required.rels", title="test", status=ConformanceStatus.Core, profile_tags=[ConformanceProfileTag.ReadQueryCore], capability_tags=_test_tags(["named-table"]), root_rel=ConformanceRel.Filter, required_rels=[ConformanceRel.Read, ConformanceRel.Filter], portability=ConformancePortability.Portable, intent="test", required_rel_shape="test", expected_constraints="test", references=_test_refs(["docs/rfcs/002_apache_substrait_integration.md"]))

    # -- Act --
    named_only_plan = plan_from_named_table("orders")

    # -- Assert --
    assert_eq(scenario_matches_root_shape(scenario, named_only_plan), false, "shape validation should fail when the root relation does not match the declared root contract")
"#,
        checks: &[
            Check::Has("    # -- Arrange --\n    scenario = SubstraitConformanceScenario("),
            Check::Has("    # -- Act --\n    named_only_plan = plan_from_named_table(\"orders\")"),
            Check::Has("    # -- Assert --\n    assert_eq("),
            Check::Lacks("    )\n\n    named_only_plan = plan_from_named_table(\"orders\")\n\n    assert_eq("),
        ],
        ..FormatCase::DEFAULT
    },
    FormatCase {
        name: "test_format_source_refuses_comment_loss_inline_comment",
        source: r#"def foo() -> int:
  x = 1  # keep this comment
  return x
"#,
        checks: &[Check::Has("# keep this comment")],
        ..FormatCase::DEFAULT
    },
    FormatCase {
        name: "test_format_source_preserves_standalone_comment_lines",
        source: r#"const A: int = 1
# ---- marker comment ----
const B: int = 2
"#,
        checks: &[Check::Has("# ---- marker comment ----")],
        ..FormatCase::DEFAULT
    },
    FormatCase {
        name: "test_format_source_collapses_docstring_interior_blank_runs",
        source: r#"def explain() -> str:
    """
    First paragraph.


    Second paragraph.
    """
    return "ok"
"#,
        checks: &[Check::Is(
            r#"def explain() -> str:
    """
    First paragraph.

    Second paragraph.
    """
    return "ok"
"#,
        )],
        ..FormatCase::DEFAULT
    },
    FormatCase {
        name: "test_format_source_collapses_many_docstring_blank_lines_but_preserves_slash_n_text",
        source: r#"def explain() -> str:
    """
    some docstring with a bunch of /n/n/n/ text





    inside it
    """
    return "ok"
"#,
        checks: &[Check::Is(
            r#"def explain() -> str:
    """
    some docstring with a bunch of /n/n/n/ text

    inside it
    """
    return "ok"
"#,
        )],
        ..FormatCase::DEFAULT
    },
    // Literal slash text and escaped docstring backslashes retain their meaning and spelling.
    FormatCase {
        name: "test_format_source_preserves_literal_slash_n_sequences",
        source: r#"def explain() -> str:
    """Explain \\n and \\t text."""
    # /n/n/n/n this is valid
    value = "/n/n/n/n and so is this"
    return value
"#,
        checks: &[Check::Unchanged, Check::Stable],
        ..FormatCase::DEFAULT
    },
    FormatCase {
        name: "test_format_source_preserves_duplicate_comment_anchors",
        source: r#"# ---- first ----
@derive(Clone)
type First = newtype int


@derive(Clone)
type Middle = newtype int


# ---- second ----
@derive(Clone)
type Second = newtype int
"#,
        checks: &[Check::Is(
            r#"# ---- first ----
@derive(Clone)
type First = newtype int


@derive(Clone)
type Middle = newtype int


# ---- second ----
@derive(Clone)
type Second = newtype int
"#,
        )],
        ..FormatCase::DEFAULT
    },
    FormatCase {
        name: "test_format_source_blank_line_separated_comment_attaches_backward",
        source: r#"type UserId = str
# comment about the alias

def load_user(id: UserId) -> User:
    pass
"#,
        checks: &[Check::Is(
            r#"type UserId = str
# comment about the alias


def load_user(id: UserId) -> User:
    pass
"#,
        )],
        ..FormatCase::DEFAULT
    },
    // A comment at EOF stays outside the function suite and keeps its preceding source gap.
    FormatCase {
        name: "test_format_source_trailing_comment_after_multiline_function_stays_after_suite",
        source: r#"def load_user(id: UserId) -> User:
    pass

# TODO: split retries
"#,
        checks: &[
            Check::Is(
                r#"def load_user(id: UserId) -> User:
    pass

# TODO: split retries
"#,
            ),
            Check::Parses,
        ],
        ..FormatCase::DEFAULT
    },
];
