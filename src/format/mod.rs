//! Incan Code Formatter
//!
//! This module provides code formatting functionality for Incan source files.
//! It follows Ruff/Black conventions with customizations:
//! - 4-space indentation
//! - 120 character line length (target, not strictly enforced)
//! - Double quotes for strings
//! - Trailing commas in multi-line constructs
//!
//! ## Parse-required
//!
//! The formatter operates on the parsed AST, so it **requires valid syntax**.
//! Files with lexer or parser errors cannot be formatted.

mod comments;
mod config;
mod formatter;
mod writer;

#[cfg(test)]
use comments::buffer::NormalizedLineBuffer;
use comments::{count_line_comments, reattach_comments};
pub use config::{FormatConfig, QuoteStyle};
pub use formatter::Formatter;

use crate::frontend::{diagnostics, lexer, parser};
use thiserror::Error;

/// Errors that occur during formatting
#[derive(Debug, Error)]
pub enum FormatError {
    #[error("syntax error (formatting requires valid syntax):\\n{0}")]
    SyntaxError(String),

    #[error("formatter would remove comments (before: {before}, after: {after}); refusing to rewrite source")]
    CommentLoss { before: usize, after: usize },

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

/// Format Incan source code with default settings.
///
/// Returns an error if the source has syntax errors (formatting requires parsing).
///
/// # Examples
///
/// ```
/// use incan::format_source;
/// # fn main() -> Result<(), incan::FormatError> {
///
/// let source = "def add(a: int, b: int) -> int:\n    return a + b\n";
/// let formatted = format_source(source)?;
/// assert!(formatted.contains("def add"));
/// # Ok(())
/// # }
/// ```
///
/// # Errors
///
/// Returns [`FormatError::SyntaxError`] if the source cannot be parsed.
pub fn format_source(source: &str) -> Result<String, FormatError> {
    format_source_with_config(source, FormatConfig::default())
}

/// Format Incan source code with custom configuration.
///
/// Vertical spacing follows the documented formatter contract and is not configurable through [`FormatConfig`].
///
/// Returns an error if the source has syntax errors (formatting requires parsing).
///
/// # Examples
///
/// ```
/// use incan::{FormatConfig, format_source_with_config};
/// # fn main() -> Result<(), incan::FormatError> {
///
/// let config = FormatConfig::default();
/// let source = "def greet(name: str) -> str:\n    return name\n";
/// let formatted = format_source_with_config(source, config)?;
/// assert!(formatted.contains("def greet"));
/// # Ok(())
/// # }
/// ```
pub fn format_source_with_config(source: &str, config: FormatConfig) -> Result<String, FormatError> {
    // Parse the source - formatter requires valid syntax
    let tokens = lexer::lex(source).map_err(|errs| {
        let mut msg = String::new();
        for err in &errs {
            msg.push_str(&diagnostics::format_error("<input>", source, err));
        }
        FormatError::SyntaxError(msg)
    })?;

    let ast = parser::parse(&tokens).map_err(|errs| {
        let mut msg = String::new();
        for err in &errs {
            msg.push_str(&diagnostics::format_error("<input>", source, err));
        }
        FormatError::SyntaxError(msg)
    })?;

    // Format the AST
    let formatter = Formatter::new(config);
    let formatted = reattach_comments(source, &formatter.format(&ast));

    // Safety guard: never allow the formatter to silently drop comments.
    let source_comments = count_line_comments(source);
    let formatted_comments = count_line_comments(&formatted);
    if formatted_comments < source_comments {
        return Err(FormatError::CommentLoss {
            before: source_comments,
            after: formatted_comments,
        });
    }

    Ok(formatted)
}

/// Check if source code is already formatted.
///
/// # Examples
///
/// ```
/// use incan::check_formatted;
/// # fn main() -> Result<(), incan::FormatError> {
///
/// // Check returns a boolean (true = already formatted)
/// let source = "def foo() -> int:\n    return 42\n";
/// let is_formatted = check_formatted(source)?;
/// // Result depends on exact formatting rules
/// assert!(is_formatted == true || is_formatted == false);
/// # Ok(())
/// # }
/// ```
pub fn check_formatted(source: &str) -> Result<bool, FormatError> {
    let formatted = format_source(source)?;
    Ok(source == formatted)
}

/// Get the diff between original and formatted source.
///
/// Returns `None` if the source is already formatted.
///
/// # Examples
///
/// ```
/// use incan::format_diff;
///
/// // Returns Ok with optional diff
/// let source = "def foo() -> int:\n    return 42\n";
/// let diff_result = format_diff(source);
/// assert!(diff_result.is_ok());
/// ```
pub fn format_diff(source: &str) -> Result<Option<String>, FormatError> {
    let formatted = format_source(source)?;

    if source == formatted {
        return Ok(None);
    }

    let mut diff = String::new();
    diff.push_str("--- original\n");
    diff.push_str("+++ formatted\n");

    let source_has_nl = source.ends_with('\n');
    let formatted_has_nl = formatted.ends_with('\n');

    let source_lines: Vec<&str> = source.lines().collect();
    let formatted_lines: Vec<&str> = formatted.lines().collect();

    let mut line_diffs = String::new();
    let max_lines = source_lines.len().max(formatted_lines.len());
    for i in 0..max_lines {
        let orig = source_lines.get(i).unwrap_or(&"");
        let fmt = formatted_lines.get(i).unwrap_or(&"");

        if orig != fmt {
            if !orig.is_empty() {
                line_diffs.push_str(&format!("-{:4} | {}\n", i + 1, orig));
            }
            if !fmt.is_empty() {
                line_diffs.push_str(&format!("+{:4} | {}\n", i + 1, fmt));
            }
        }
    }

    // If only trailing newline differs, surface an explicit, actionable diff.
    let trailing_newline_only = line_diffs.is_empty()
        && source.trim_end_matches('\n') == formatted.trim_end_matches('\n')
        && source_has_nl != formatted_has_nl;

    if trailing_newline_only {
        diff.push_str("@@ trailing-newline @@\n");
        if !source_has_nl {
            diff.push_str("-<no trailing newline>\n");
        }
        if formatted_has_nl {
            diff.push_str("+<adds trailing newline>\n");
        } else {
            diff.push_str("+<no trailing newline>\n");
        }
    } else {
        diff.push_str(&line_diffs);
    }

    Ok(Some(diff))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frontend::ast::{Declaration, Program};
    use crate::frontend::{lexer, parser};

    fn program_from_source(source: &str) -> Result<Program, FormatError> {
        let tokens = lexer::lex(source).map_err(|errs| {
            FormatError::SyntaxError(errs.iter().map(|e| e.message.clone()).collect::<Vec<_>>().join("\n"))
        })?;
        parser::parse(&tokens).map_err(|errs| {
            FormatError::SyntaxError(errs.iter().map(|e| e.message.clone()).collect::<Vec<_>>().join("\n"))
        })
    }

    fn assert_comment_line_immediately_before(
        lines: &[&str],
        stmt_label: &str,
        line_matches: impl Fn(&str) -> bool,
        comment_needle: &str,
    ) -> Result<(), FormatError> {
        let idx = lines.iter().position(|&l| line_matches(l)).ok_or_else(|| {
            FormatError::SyntaxError(format!(
                "missing formatted line for {stmt_label} (comment-anchoring regression)"
            ))
        })?;
        assert!(
            idx > 0 && lines[idx - 1].contains(comment_needle),
            "expected comment {comment_needle:?} on the line immediately before {stmt_label}; lines={lines:?}"
        );
        Ok(())
    }

    fn assert_decl_block_docstring_markers(doc: Option<&str>, context: &str) -> Result<(), FormatError> {
        let Some(doc) = doc else {
            return Err(FormatError::SyntaxError(format!(
                "{context}: expected declaration body docstring for API/tooling extraction"
            )));
        };
        let trimmed = doc.trim();
        if !trimmed.contains("Line A documents the class API.") {
            return Err(FormatError::SyntaxError(format!(
                "{context}: docstring missing marker line A: {trimmed:?}"
            )));
        }
        if !trimmed.contains("Line B keeps interior newlines after trim().") {
            return Err(FormatError::SyntaxError(format!(
                "{context}: docstring missing marker line B: {trimmed:?}"
            )));
        }
        Ok(())
    }

    fn assert_first_class_decl_has_marker_docstring(program: &Program, context: &str) -> Result<(), FormatError> {
        let class = match &program.declarations[0].node {
            Declaration::Class(c) => c,
            other => {
                return Err(FormatError::SyntaxError(format!(
                    "{context}: expected class declaration, got {other:?}"
                )));
            }
        };
        assert_decl_block_docstring_markers(class.docstring.as_deref(), context)
    }

    fn assert_first_model_decl_has_marker_docstring(program: &Program, context: &str) -> Result<(), FormatError> {
        let model = match &program.declarations[0].node {
            Declaration::Model(m) => m,
            other => {
                return Err(FormatError::SyntaxError(format!(
                    "{context}: expected model declaration, got {other:?}"
                )));
            }
        };
        assert_decl_block_docstring_markers(model.docstring.as_deref(), context)
    }

    fn assert_first_enum_decl_has_marker_docstring(program: &Program, context: &str) -> Result<(), FormatError> {
        let en = match &program.declarations[0].node {
            Declaration::Enum(e) => e,
            other => {
                return Err(FormatError::SyntaxError(format!(
                    "{context}: expected enum declaration, got {other:?}"
                )));
            }
        };
        assert_decl_block_docstring_markers(en.docstring.as_deref(), context)
    }

    fn assert_first_trait_decl_has_marker_docstring(program: &Program, context: &str) -> Result<(), FormatError> {
        let tr = match &program.declarations[0].node {
            Declaration::Trait(t) => t,
            other => {
                return Err(FormatError::SyntaxError(format!(
                    "{context}: expected trait declaration, got {other:?}"
                )));
            }
        };
        assert_decl_block_docstring_markers(tr.docstring.as_deref(), context)
    }

    // ========================================
    // format_source tests
    // ========================================

    #[test]
    fn test_format_source_simple_function() {
        let source = r#"def foo() -> int:
  return 42
"#;
        let result = format_source(source);
        assert!(result.is_ok());
    }

    #[test]
    fn test_format_source_rest_params_and_call_unpacking() -> Result<(), FormatError> {
        let source = r#"def collect(prefix: str, *items: int, **labels: str) -> int:
  return collect(prefix,*items,**labels)
"#;
        let formatted = format_source(source)?;
        assert_eq!(
            formatted,
            r#"def collect(prefix: str, *items: int, **labels: str) -> int:
    return collect(prefix, *items, **labels)
"#
        );
        Ok(())
    }

    #[test]
    fn test_format_source_collection_literal_spread_entries() -> Result<(), FormatError> {
        let source = r#"def f(xs: list[int], headers: dict[str, str]) -> None:
  values=[1,*xs,4]
  merged={"accept":"json",**headers}
"#;
        let formatted = format_source(source)?;
        assert_eq!(
            formatted,
            r#"def f(xs: list[int], headers: dict[str, str]) -> None:
    values = [1, *xs, 4]
    merged = {"accept": "json", **headers}
"#
        );
        Ok(())
    }

    #[test]
    fn test_format_source_generator_expression_full_clause_shape() -> Result<(), FormatError> {
        let source = r#"def run(xs: list[int], ys: list[int]) -> Generator[int]:
  return (x*y for x in xs if x>0 for y in ys if y>x)
"#;
        let expected = r#"def run(xs: list[int], ys: list[int]) -> Generator[int]:
    return (x * y for x in xs if x > 0 for y in ys if y > x)
"#;
        let formatted = format_source(source)?;
        assert_eq!(formatted, expected);
        assert_eq!(format_source(&formatted)?, expected);
        Ok(())
    }

    #[test]
    fn test_format_source_rfc028_operator_spellings() -> Result<(), FormatError> {
        let source = r#"def ops(a: Any, b: Any, c: Any) -> None:
  mat=a@b
  piped=a|>b<|c
  bits=~a&b|c^a<<b>>c
  a @= b
  a &= b
  a |= b
  a ^= b
  a <<= b
  a >>= b
"#;
        let formatted = format_source(source)?;
        assert_eq!(
            formatted,
            r#"def ops(a: Any, b: Any, c: Any) -> None:
    mat = a @ b
    piped = a |> b <| c
    bits = ~a & b | c ^ a << b >> c
    a @= b
    a &= b
    a |= b
    a ^= b
    a <<= b
    a >>= b
"#
        );
        Ok(())
    }

    #[test]
    fn test_format_source_model() {
        let source = r#"model User:
  name: str
  age: int
"#;
        let result = format_source(source);
        assert!(result.is_ok());
    }

    #[test]
    fn test_format_source_trait_with_supertraits() -> Result<(), FormatError> {
        let source = r#"trait OrderedCollection[T] with Collection[T], Serializable:
  def sorted(self) -> OrderedCollection[T]: ...
"#;
        let formatted = format_source(source)?;
        let expected = r#"trait OrderedCollection[T] with Collection[T], Serializable:
    def sorted(self) -> OrderedCollection[T]
"#;
        assert_eq!(formatted, expected);
        Ok(())
    }

    #[test]
    fn test_format_source_rust_allow_decorator() -> Result<(), FormatError> {
        let source = r#"@rust.allow("dead_code","clippy::too_many_arguments")
def MixedName() -> int:
  return 1
"#;
        let formatted = format_source(source)?;
        let expected = r#"@rust.allow("dead_code", "clippy::too_many_arguments")
def MixedName() -> int:
    return 1
"#;
        assert_eq!(formatted, expected);
        Ok(())
    }

    #[test]
    fn test_format_source_wraps_long_function_signature() -> Result<(), FormatError> {
        let source = r#"def append_node(store_id: int, kind: PrismNodeKind, input_ids: list[int], named_table: str, predicate: bool, limit_count: int) -> int:
  return 1
"#;
        let formatted = format_source(source)?;
        let expected = r#"def append_node(
    store_id: int,
    kind: PrismNodeKind,
    input_ids: list[int],
    named_table: str,
    predicate: bool,
    limit_count: int,
) -> int:
    return 1
"#;
        assert_eq!(formatted, expected);
        assert!(
            formatted.lines().all(|line| line.len() <= 120),
            "expected wrapped signature to stay within 120 columns; got:\n{formatted}"
        );
        Ok(())
    }

    #[test]
    fn test_format_source_wraps_long_method_signature() -> Result<(), FormatError> {
        let source = r#"class Store:
  def append_node(self, store_id: int, kind: PrismNodeKind, input_ids: list[int], named_table: str, predicate: bool, limit_count: int) -> int:
    return 1
"#;
        let formatted = format_source(source)?;
        let expected = r#"class Store:
    def append_node(
        self,
        store_id: int,
        kind: PrismNodeKind,
        input_ids: list[int],
        named_table: str,
        predicate: bool,
        limit_count: int,
    ) -> int:
        return 1
"#;
        assert_eq!(formatted, expected);
        assert!(
            formatted.lines().all(|line| line.len() <= 120),
            "expected wrapped method signature to stay within 120 columns; got:\n{formatted}"
        );
        Ok(())
    }

    #[test]
    fn test_format_source_wraps_long_class_trait_adoption_header() -> Result<(), FormatError> {
        let source = r#"pub class _BytesIO with BinaryReader, BinaryRead[u8], BinaryRead[i8], BinaryRead[u16], BinaryRead[i16], BinaryRead[u32], BinaryRead[i32], BinaryRead[u64], BinaryRead[i64], BinaryWrite[u8], BinaryWrite[i8], BinaryWrite[u16], BinaryWrite[i16], BinaryWrite[u32], BinaryWrite[i32], BinaryWrite[u64], BinaryWrite[i64]:
  handle: Cursor[bytes]
"#;
        let formatted = format_source(source)?;
        let expected = r#"pub class _BytesIO with (
    BinaryReader,
    BinaryRead[u8],
    BinaryRead[i8],
    BinaryRead[u16],
    BinaryRead[i16],
    BinaryRead[u32],
    BinaryRead[i32],
    BinaryRead[u64],
    BinaryRead[i64],
    BinaryWrite[u8],
    BinaryWrite[i8],
    BinaryWrite[u16],
    BinaryWrite[i16],
    BinaryWrite[u32],
    BinaryWrite[i32],
    BinaryWrite[u64],
    BinaryWrite[i64],
):
    handle: Cursor[bytes]
"#;
        assert_eq!(formatted, expected);
        assert_eq!(format_source(&formatted)?, expected);
        assert!(
            formatted.lines().all(|line| line.len() <= 120),
            "expected wrapped class trait adoption header to stay within 120 columns; got:\n{formatted}"
        );
        Ok(())
    }

    #[test]
    fn test_format_source_invalid_syntax() {
        let source = "def foo(";
        let result = format_source(source);
        assert!(result.is_err());
    }

    #[test]
    fn test_format_source_empty() {
        let source = "";
        let result = format_source(source);
        assert!(result.is_ok());
    }

    /// Regression (GitHub #189): declarations already end with a newline; an extra `newline()` at EOF produced `\n\n`.
    #[test]
    fn test_format_source_eof_has_single_trailing_newline_only() -> Result<(), FormatError> {
        let source = r#"def f() -> int:
    return 1


"#;
        let formatted = format_source(source)?;
        let trailing_nl = formatted.chars().rev().take_while(|c| *c == '\n').count();
        assert_eq!(
            trailing_nl, 1,
            "expected exactly one trailing newline at EOF; got {trailing_nl}: {formatted:?}"
        );
        Ok(())
    }

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
            formatted.contains(
                "                Ok(_) =>\n                    # -- Assert --\n                    assert_eq("
            ),
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

    /// Formats `source` and checks the result lexes and parses (regression harness for formatter output validity).
    fn assert_format_round_trip_lex_parse(source: &str) -> Result<String, FormatError> {
        let formatted = format_source(source)?;
        let tokens = crate::frontend::lexer::lex(&formatted).map_err(|errs| {
            FormatError::SyntaxError(errs.iter().map(|e| e.message.clone()).collect::<Vec<_>>().join("\n"))
        })?;
        crate::frontend::parser::parse(&tokens).map_err(|errs| {
            FormatError::SyntaxError(errs.iter().map(|e| e.message.clone()).collect::<Vec<_>>().join("\n"))
        })?;
        Ok(formatted)
    }

    #[test]
    fn test_format_source_race_for_expression_round_trip() -> Result<(), FormatError> {
        let source = r#"import std.async

async def run() -> int:
    return race for value:
        await fast() => value
        await slow() =>
            return value
"#;
        let formatted = assert_format_round_trip_lex_parse(source)?;
        assert!(
            formatted.contains("race for value:"),
            "expected formatter to preserve race header; got: {formatted}"
        );
        assert!(
            formatted.contains("await fast() => value"),
            "expected formatter to preserve inline race arm; got: {formatted}"
        );
        assert!(
            formatted.contains("await slow() =>\n            return value"),
            "expected formatter to preserve block race arm; got: {formatted}"
        );
        Ok(())
    }

    #[test]
    fn test_format_source_union_annotations_round_trip_as_canonical_generic() -> Result<(), FormatError> {
        let source = r#"def parse(values: List[int | str], maybe: int | str | None) -> int | str:
    return values[0]
"#;
        let formatted = assert_format_round_trip_lex_parse(source)?;
        assert!(
            formatted.contains("values: List[Union[int, str]]"),
            "expected nested pipe annotation to format as canonical Union generic; got: {formatted}"
        );
        assert!(
            formatted.contains("maybe: Union[int, str, None]"),
            "expected None-containing pipe annotation to remain parseable in formatted AST form; got: {formatted}"
        );
        assert!(
            formatted.contains("-> Union[int, str]:"),
            "expected return pipe annotation to format as canonical Union generic; got: {formatted}"
        );
        Ok(())
    }

    /// Regression (GitHub #289): escaped newlines inside f-strings must stay textual (`\\n`) after formatting.
    #[test]
    fn test_format_source_preserves_fstring_escaped_newline() -> Result<(), FormatError> {
        let source = "def main() -> str:\n    return f\"a\\n{1}\"\n";
        let formatted = assert_format_round_trip_lex_parse(source)?;
        assert!(
            formatted.contains(r#"f"a\n{1}""#),
            "expected formatter to preserve escaped newline text in f-string, got: {formatted}"
        );
        assert!(
            !formatted.contains("f\"a\n{1}\""),
            "formatter must not materialize a physical newline in f-string output, got: {formatted}"
        );
        Ok(())
    }

    /// Regression #235: qualified constructor patterns use `::` in the AST; the formatter must print Incansurface `.`.
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
    fn test_format_source_generic_method_round_trip() -> Result<(), FormatError> {
        let source = r#"class Box:
    def get[T with Clone](self, value: T) -> T:
        return value
"#;
        let formatted = assert_format_round_trip_lex_parse(source)?;
        assert!(
            formatted.contains("def get[T with Clone](self, value: T) -> T:"),
            "expected method type params preserved by formatter; got: {formatted}"
        );
        Ok(())
    }

    #[test]
    fn test_format_source_method_trait_target_round_trip() -> Result<(), FormatError> {
        let source = r#"type UserId = rusttype i64 with Display:
    def fmt(self, f: Formatter) for Display -> Result[None, FmtError]:
        pass
"#;
        let formatted = assert_format_round_trip_lex_parse(source)?;
        assert_eq!(formatted, source);
        Ok(())
    }

    #[test]
    fn test_format_source_constrained_primitive_newtype_round_trip() -> Result<(), FormatError> {
        let source = "type Digit = newtype int[gt=-1, lt=10]\n";
        let formatted = assert_format_round_trip_lex_parse(source)?;
        assert_eq!(formatted, source);
        Ok(())
    }

    #[test]
    fn test_format_source_constrained_primitive_spacing() -> Result<(), FormatError> {
        let source = "type Ratio = newtype float[ge = 0,le=1]\n";
        let formatted = format_source(source)?;
        let expected = "type Ratio = newtype float[ge=0, le=1]\n";
        assert_eq!(formatted, expected);
        assert_eq!(assert_format_round_trip_lex_parse(&formatted)?, expected);
        Ok(())
    }

    #[test]
    fn test_format_source_preserves_mut_function_params() -> Result<(), FormatError> {
        let source = r#"def bump(mut session: Session, mut count: int) -> int:
    return count
"#;
        let formatted = assert_format_round_trip_lex_parse(source)?;
        assert!(
            formatted.contains("def bump(mut session: Session, mut count: int) -> int:"),
            "expected mut parameter markers preserved by formatter; got: {formatted}"
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

    /// Regression (GitHub #250): `f64::Display` drops `.0` for whole numbers, which broke
    /// `normalize_code_for_match` anchors in [`reattach_comments`] and flushed standalone `#` lines to EOF.
    ///
    /// Covers `120.0`, distinct `1E6` / `1e6` exponents, `1_000.0`, and underscored int `1_000`, each with a standalone
    /// comment on the line above. See also [`test_format_source_preserves_numeric_literal_source_substring`].
    #[test]
    fn test_format_source_preserves_float_spelling_for_comment_anchors() -> Result<(), FormatError> {
        let source = r#"def main() -> None:
    """Docstring."""
    # Comment before float line.
    x = 120.0
    # Comment before int line.
    y = 1
    # Comment before E float line.
    z = 1E6
    # Comment before e float line.
    w = 1e6
    # Comment before underscore float line.
    v = 1_000.0
    # Comment before underscore int line.
    u = 1_000
"#;
        let formatted = format_source(source)?;
        assert!(
            formatted.contains("120.0"),
            "expected 120.0 spelling preserved; got: {formatted:?}"
        );
        assert!(
            formatted.contains("z = 1E6"),
            "expected uppercase E on z line; got: {formatted:?}"
        );
        assert!(
            formatted.contains("w = 1e6"),
            "expected lowercase e on w line; got: {formatted:?}"
        );
        assert!(
            formatted.contains("v = 1_000.0"),
            "expected underscores on v line; got: {formatted:?}"
        );
        assert!(
            formatted.contains("u = 1_000"),
            "expected underscores on int u line; got: {formatted:?}"
        );

        let lines: Vec<&str> = formatted.lines().collect();
        assert_comment_line_immediately_before(
            &lines,
            "x = 120.0",
            |l| l.trim_start().starts_with("x = "),
            "# Comment before float",
        )?;
        assert_comment_line_immediately_before(
            &lines,
            "y = 1",
            |l| l.trim_start().starts_with("y = "),
            "# Comment before int",
        )?;
        assert_comment_line_immediately_before(
            &lines,
            "z = 1E6",
            |l| l.trim_start().starts_with("z = "),
            "# Comment before E float",
        )?;
        assert_comment_line_immediately_before(
            &lines,
            "w = 1e6",
            |l| l.trim_start().starts_with("w = "),
            "# Comment before e float",
        )?;
        assert_comment_line_immediately_before(
            &lines,
            "v = 1_000.0",
            |l| l.trim_start().starts_with("v = "),
            "# Comment before underscore float",
        )?;
        assert_comment_line_immediately_before(
            &lines,
            "u = 1_000",
            |l| l.trim_start().starts_with("u = "),
            "# Comment before underscore int",
        )?;
        Ok(())
    }

    /// [`IntLiteral::repr`](incan_syntax::ast::IntLiteral) / [`FloatLiteral::repr`](incan_syntax::ast::FloatLiteral)
    /// use the lexer source slice (`_`, `E`/`e` preserved).
    #[test]
    fn test_format_source_preserves_numeric_literal_source_substring() -> Result<(), FormatError> {
        let source = r#"def f() -> None:
    a = 1_200.0
    b = 1E6
    c = 1_000
    d = 1e6
    e = 1000.0
    f = 19.99d
"#;
        let formatted = format_source(source)?;
        assert!(
            formatted.contains("1_200.0"),
            "expected underscore separators preserved in float literal; got: {formatted:?}"
        );
        assert!(
            formatted.contains("b = 1E6"),
            "expected uppercase E on b line; got: {formatted:?}"
        );
        assert!(
            formatted.contains("c = 1_000"),
            "expected underscore separators preserved in int literal; got: {formatted:?}"
        );
        assert!(
            formatted.contains("d = 1e6"),
            "expected lowercase e on d line; got: {formatted:?}"
        );
        assert!(
            formatted.contains("e = 1000.0"),
            "expected plain 1000.0 preserved; got: {formatted:?}"
        );
        assert!(
            formatted.contains("f = 19.99d"),
            "expected decimal literal suffix preserved; got: {formatted:?}"
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
    fn test_format_source_preserves_pub_class_field_visibility() -> Result<(), FormatError> {
        let source = r#"pub class LazyFrame:
    _cursor: int
    pub schema: str
"#;
        let formatted = format_source(source)?;
        assert_eq!(formatted, source);
        Ok(())
    }

    /// Regression (GitHub #247): class body docstrings must round-trip for several body shapes **and** stay attached
    /// on [`ClassDecl::docstring`] after lex+parse of the formatted source (tooling / API extraction path).
    ///
    /// Covers decorators, `extends`, `with` bounds, fields-only, methods-only, multiple methods, and mixed
    /// field+method layouts. Omits `pass`-only class bodies: unlike traits, class bodies do not parse a bare
    /// `pass` statement today.
    ///
    /// Model, enum, and trait body docstrings use the same AST and formatter rules (see
    /// `test_format_source_preserves_model_enum_body_docstrings_ast` and
    /// `test_format_source_preserves_trait_body_docstring_ast` below).
    ///
    /// Two non-empty lines so `trim()` retains an interior newline; `format_docstring` keeps multi-line form (same
    /// constraint as newtype docstring tests).
    #[test]
    fn test_format_source_preserves_class_docstring_common_shapes() -> Result<(), FormatError> {
        const DOC: &str = r#"    """
    Line A documents the class API.
    Line B keeps interior newlines after trim().
    """"#;

        let cases: &[(&str, String)] = &[
            (
                "decorator_generic_field",
                format!(
                    r#"@derive(Clone)
pub class WithDecorator[T]:
{DOC}

    pub cell: T
"#
                ),
            ),
            (
                "generic_with_trait_bound_fields_and_method",
                format!(
                    r#"pub class PrismCursor[T with Clone]:
{DOC}

    pub row_schema_marker: T

    def clone(self) -> Self:
        """Method doc."""
        pass
"#
                ),
            ),
            (
                "fields_only",
                format!(
                    r#"pub class FieldBucket[T]:
{DOC}

    pub first: T
    pub second: int
"#
                ),
            ),
            (
                "private_class_private_fields",
                format!(
                    r#"class InternalState:
{DOC}

    x: int
    y: int
"#
                ),
            ),
            (
                "methods_only",
                format!(
                    r#"class MethodsOnly:
{DOC}

    def one(self) -> int:
        return 1
"#
                ),
            ),
            (
                "two_methods",
                format!(
                    r#"class TwoMethods:
{DOC}

    def a(self) -> None:
        pass

    def b(self) -> None:
        pass
"#
                ),
            ),
            (
                "extends_and_field",
                format!(
                    r#"class Pup extends Animal:
{DOC}

    tag: str
"#
                ),
            ),
        ];

        for (label, source) in cases {
            let formatted = format_source(source)?;
            assert_eq!(formatted, *source, "class docstring round-trip ({label})");
            let program = program_from_source(&formatted)?;
            assert_first_class_decl_has_marker_docstring(&program, &format!("formatted source, case {label}"))?;
        }
        Ok(())
    }

    /// Body docstrings on `model` and `enum` use the same storage and formatting path as `class` (GitHub #247).
    #[test]
    fn test_format_source_preserves_model_enum_body_docstrings_ast() -> Result<(), FormatError> {
        const DOC: &str = r#"    """
    Line A documents the class API.
    Line B keeps interior newlines after trim().
    """"#;

        let model_src = format!(
            r#"model LedgerEntry:
{DOC}

    id: int
    name: str
"#
        );
        let formatted_model = format_source(&model_src)?;
        assert_eq!(formatted_model, model_src);
        let prog_m = program_from_source(&formatted_model)?;
        assert_first_model_decl_has_marker_docstring(&prog_m, "model + fields after format")?;

        let enum_src = format!(
            r#"enum JobState:
{DOC}

    Pending
    Running
    Done
"#
        );
        let formatted_enum = format_source(&enum_src)?;
        assert_eq!(formatted_enum, enum_src);
        let prog_e = program_from_source(&formatted_enum)?;
        assert_first_enum_decl_has_marker_docstring(&prog_e, "enum + variants after format")?;

        Ok(())
    }

    #[test]
    fn test_format_source_preserves_value_enum_declarations() -> Result<(), FormatError> {
        let source = r#"enum Color(str):
  Red="red"
  Blue = "blue"
  Quote = "say \"hi\""
  Path = "c:\\tmp"
  Tabs = "a\tb"

enum Status(int):
    Pending=1
    Done = 2
"#;

        let expected = r#"enum Color(str):
    Red = "red"
    Blue = "blue"
    Quote = "say \"hi\""
    Path = "c:\\tmp"
    Tabs = "a\tb"


enum Status(int):
    Pending = 1
    Done = 2
"#;

        assert_eq!(format_source(source)?, expected);
        assert_eq!(format_source(expected)?, expected);
        Ok(())
    }

    #[test]
    fn test_format_source_preserves_trait_body_docstring_ast() -> Result<(), FormatError> {
        const DOC: &str = r#"    """
    Line A documents the class API.
    Line B keeps interior newlines after trim().
    """"#;
        let source = format!(
            r#"trait Described:
{DOC}

    def tag(self) -> str: ...
"#
        );
        let formatted = format_source(&source)?;
        let expected = format!(
            r#"trait Described:
{DOC}

    def tag(self) -> str
"#
        );
        assert_eq!(formatted, expected);
        let prog = program_from_source(&formatted)?;
        assert_first_trait_decl_has_marker_docstring(&prog, "trait + method after format")?;
        Ok(())
    }

    /// `const` / `static` have no inline body docstring field; module-level `"""..."""` is a separate
    /// [`Declaration::Docstring`] and must round-trip before those items.
    #[test]
    fn test_format_source_preserves_module_docstring_before_const_and_static() -> Result<(), FormatError> {
        let source = r#""""Module-level API notes."""

const ANSWER: int = 42
static COUNTER: int = 0
"#;
        let formatted = format_source(source)?;
        assert_eq!(formatted, source);
        let prog = program_from_source(&formatted)?;
        let doc = match &prog.declarations[0].node {
            Declaration::Docstring(doc) => doc,
            other => {
                return Err(FormatError::SyntaxError(format!(
                    "expected leading module docstring declaration, got {other:?}"
                )));
            }
        };
        if !doc.contains("Module-level API notes.") {
            return Err(FormatError::SyntaxError(format!("docstring text lost: {doc:?}")));
        }
        let c = match &prog.declarations[1].node {
            Declaration::Const(c) => c,
            other => {
                return Err(FormatError::SyntaxError(format!("expected const: {other:?}")));
            }
        };
        assert_eq!(c.name, "ANSWER");
        let s = match &prog.declarations[2].node {
            Declaration::Static(s) => s,
            other => {
                return Err(FormatError::SyntaxError(format!("expected static: {other:?}")));
            }
        };
        assert_eq!(s.name, "COUNTER");
        Ok(())
    }

    #[test]
    fn test_format_source_preserves_rich_newtype_round_trip() -> Result<(), FormatError> {
        let source = r#"@derive(Clone)
pub type MutexGuard[T with Clone] = newtype RawMutexGuard[T]:
    # XXX: keep this comment anchored to the type docstring
    """
    Guard providing access to mutex-protected data.
    The lock is released when the guard goes out of scope.
    """

    def get(self) -> T:
        """Get the current value (by reference)"""
        return value

    def example(self) -> None:
        shared_counter = Mutex.new(0)  # XXX: constructor lives on Mutex
        return None
"#;
        let formatted = format_source(source)?;
        assert_eq!(formatted, source);
        Ok(())
    }

    #[test]
    fn test_format_source_associated_type_in_rusttype_body() -> Result<(), FormatError> {
        let source = r#"type UserId = rusttype i64 with Add[int, UserId]:
    type Output for Add[int] = UserId

    def add(self, rhs: int) for Add[int] -> UserId:
        pass
"#;
        let formatted = assert_format_round_trip_lex_parse(source)?;
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

    // ========================================
    // format_source_with_config tests
    // ========================================

    #[test]
    fn test_format_source_with_custom_config() {
        let source = r#"def foo() -> int:
  return 42
"#;
        let config = FormatConfig::new().with_indent_width(2);
        let result = format_source_with_config(source, config);
        assert!(result.is_ok());
    }

    #[test]
    fn test_format_source_with_different_line_length() {
        let source = r#"def foo() -> int:
  return 42
"#;
        let config = FormatConfig::new().with_line_length(80);
        let result = format_source_with_config(source, config);
        assert!(result.is_ok());
    }

    // ========================================
    // check_formatted tests
    // ========================================

    #[test]
    fn test_check_formatted_simple() {
        let source = r#"def foo() -> int:
    return 42
"#;
        let result = check_formatted(source);
        assert!(result.is_ok());
    }

    #[test]
    fn test_check_formatted_invalid_syntax() {
        let source = "def foo(";
        let result = check_formatted(source);
        assert!(result.is_err());
    }

    // ========================================
    // format_diff tests
    // ========================================

    #[test]
    fn test_format_diff_no_changes() {
        let source = r#"def foo() -> int:
    return 42
"#;
        let result = format_diff(source);
        // May have no changes if already formatted, or may have changes
        assert!(result.is_ok());
    }

    #[test]
    fn test_format_diff_invalid_syntax() {
        let source = "def foo(";
        let result = format_diff(source);
        assert!(result.is_err());
    }

    #[test]
    fn test_format_diff_returns_diff() {
        // Improperly indented source
        let source = r#"def foo() -> int:
 return 42
"#;
        let result = format_diff(source);
        assert!(result.is_ok());
        // The diff may or may not be Some depending on formatter behavior
    }

    #[test]
    fn test_format_diff_trailing_newline_only_is_actionable() -> Result<(), FormatError> {
        let source = "def foo() -> int:\n    return 42";
        let result = format_diff(source)?;
        let diff = result.ok_or_else(|| {
            FormatError::SyntaxError("diff should be present for trailing-newline change".to_string())
        })?;
        assert!(
            diff.contains("trailing-newline"),
            "expected trailing newline hint in diff, got: {diff}"
        );
        Ok(())
    }

    // ========================================
    // Issue #116: parenthesized import formatting
    // ========================================

    /// A short import that fits on one line should be kept (or collapsed to) single-line form.
    #[test]
    fn test_format_import_short_stays_single_line() -> Result<(), FormatError> {
        let source = "from db import (CategoryId, TagId)\n";
        let config = FormatConfig::new().with_line_length(120);
        let result = format_source_with_config(source, config)?;
        assert_eq!(result.trim_end(), "from db import CategoryId, TagId");
        Ok(())
    }

    /// A comma-separated import that already fits on one line is unchanged.
    #[test]
    fn test_format_import_bare_short_unchanged() -> Result<(), FormatError> {
        let source = "from db import CategoryId, TagId\n";
        let config = FormatConfig::new().with_line_length(120);
        let result = format_source_with_config(source, config)?;
        assert_eq!(result.trim_end(), "from db import CategoryId, TagId");
        Ok(())
    }

    /// A long multi-item import that exceeds the line length should be wrapped.
    #[test]
    fn test_format_import_long_wraps_to_parens() -> Result<(), FormatError> {
        // Use a very short limit so the list definitely overflows.
        let source = "from db import CategoryId, TagId, OtherId\n";
        let config = FormatConfig::new().with_line_length(20).with_trailing_commas(true);
        let result = format_source_with_config(source, config)?;
        assert!(
            result.contains('('),
            "expected parenthesized output for long import; got: {result}"
        );
        assert!(
            result.contains("CategoryId,\n"),
            "expected each item on its own line; got: {result}"
        );
        Ok(())
    }

    /// A multi-line parenthesized import that fits on one line is collapsed to single-line.
    #[test]
    fn test_format_import_multiline_parens_collapses_when_fits() -> Result<(), FormatError> {
        let source = "from db import (\n    CategoryId,\n    TagId,\n)\n";
        let config = FormatConfig::new().with_line_length(120);
        let result = format_source_with_config(source, config)?;
        assert_eq!(result.trim_end(), "from db import CategoryId, TagId");
        Ok(())
    }

    /// Trailing comma in parenthesized output is controlled by the `trailing_commas` config.
    #[test]
    fn test_format_import_no_trailing_comma_when_disabled() -> Result<(), FormatError> {
        let source = "from db import CategoryId, TagId, OtherId\n";
        let config = FormatConfig::new().with_line_length(20).with_trailing_commas(false);
        let result = format_source_with_config(source, config)?;
        // Last item should not have a trailing comma.
        assert!(
            !result.contains("OtherId,\n"),
            "expected no trailing comma after last item; got: {result}"
        );
        assert!(
            result.contains("OtherId\n"),
            "expected last item without comma; got: {result}"
        );
        Ok(())
    }

    #[test]
    fn test_format_long_constructor_call_wraps_args() -> Result<(), FormatError> {
        let source = r#"def build_schema() -> Schema:
    return CarrierSchema(declared_columns=declared_columns(), planned_columns=planned_columns(), resolved_columns=resolved_columns())
"#;
        let config = FormatConfig::new().with_line_length(60).with_trailing_commas(true);
        let result = format_source_with_config(source, config)?;
        let expected = r#"def build_schema() -> Schema:
    return CarrierSchema(
        declared_columns=declared_columns(),
        planned_columns=planned_columns(),
        resolved_columns=resolved_columns(),
    )
"#;
        assert_eq!(result, expected);
        Ok(())
    }

    #[test]
    fn test_format_long_constructor_call_wraps_without_trailing_comma_when_disabled() -> Result<(), FormatError> {
        let source = r#"def build_schema() -> Schema:
    return CarrierSchema(declared_columns=declared_columns(), planned_columns=planned_columns(), resolved_columns=resolved_columns())
"#;
        let config = FormatConfig::new().with_line_length(60).with_trailing_commas(false);
        let result = format_source_with_config(source, config)?;
        let expected = r#"def build_schema() -> Schema:
    return CarrierSchema(
        declared_columns=declared_columns(),
        planned_columns=planned_columns(),
        resolved_columns=resolved_columns()
    )
"#;
        assert_eq!(result, expected);
        Ok(())
    }

    #[test]
    fn test_format_pub_library_import_round_trip() -> Result<(), FormatError> {
        let source = "import pub::mylib as lib\n";
        let formatted = format_source(source)?;
        assert_eq!(formatted.trim_end(), source.trim_end());
        Ok(())
    }

    #[test]
    fn test_format_pub_from_import_collapses_parenthesized_list() -> Result<(), FormatError> {
        let source = "from pub::mylib import (\n    Widget,\n    make_widget as build_widget,\n)\n";
        let config = FormatConfig::new().with_line_length(120);
        let formatted = format_source_with_config(source, config)?;
        assert_eq!(
            formatted.trim_end(),
            "from pub::mylib import Widget, make_widget as build_widget"
        );
        Ok(())
    }

    #[test]
    fn test_format_top_level_spacing_imports_consts_and_function() -> Result<(), FormatError> {
        let source = r#"from rust::std::f64::consts import PI, E
from rust::std::f64 import INFINITY, NAN
const A: int = 1
const B: int = 2
def sum_constants() -> int:
  return A + B
"#;
        let result = format_source(source)?;

        let expected = r#"from rust::std::f64::consts import PI, E
from rust::std::f64 import INFINITY, NAN

const A: int = 1
const B: int = 2


def sum_constants() -> int:
    return A + B
"#;
        assert_eq!(result, expected);
        Ok(())
    }

    #[test]
    fn test_format_top_level_spacing_single_line_alias_then_body_bearing_decl() -> Result<(), FormatError> {
        let source = r#"type UserId = str
model User:
  id: UserId

def load_user(id: UserId) -> User:
  pass
"#;
        let result = format_source(source)?;

        let expected = r#"type UserId = str


model User:
    id: UserId


def load_user(id: UserId) -> User:
    pass
"#;
        assert_eq!(result, expected);
        Ok(())
    }

    #[test]
    fn test_format_top_level_spacing_static_then_function_uses_two_blank_lines() -> Result<(), FormatError> {
        let source = r#"static prism_store_node_counts: list[int] = []
pub def allocate_prism_store_id() -> int:
  return len(prism_store_node_counts)
"#;
        let result = format_source(source)?;

        let expected = r#"static prism_store_node_counts: list[int] = []


pub def allocate_prism_store_id() -> int:
    return len(prism_store_node_counts)
"#;
        assert_eq!(result, expected);
        Ok(())
    }

    #[test]
    fn test_format_trait_abstract_methods_prefer_bodyless_form_before_default_method() -> Result<(), FormatError> {
        let source = r#"trait Service:
  def connect(self) -> None: ...
  def close(self) -> None: ...
  def reset(self) -> None:
    pass
"#;
        let result = format_source(source)?;

        let expected = r#"trait Service:
    def connect(self) -> None
    def close(self) -> None

    def reset(self) -> None:
        pass
"#;
        assert_eq!(result, expected);
        Ok(())
    }

    #[test]
    fn test_format_source_computed_properties() -> Result<(), FormatError> {
        let source = r#"class Dataset:
  fields: list[str]
  property schema_fields -> list[str]:
    return self.fields
  def len(self) -> int:
    return 0
"#;
        let result = format_source(source)?;

        let expected = r#"class Dataset:
    fields: list[str]

    property schema_fields -> list[str]:
        return self.fields

    def len(self) -> int:
        return 0
"#;
        assert_eq!(result, expected);
        Ok(())
    }

    #[test]
    fn test_format_source_trait_abstract_properties_stay_tight() -> Result<(), FormatError> {
        let source = r#"trait HasShape:
  property area -> float: ...
  property perimeter -> float: ...
  def describe(self) -> str:
    return "shape"
"#;
        let result = format_source(source)?;

        let expected = r#"trait HasShape:
    property area -> float
    property perimeter -> float

    def describe(self) -> str:
        return "shape"
"#;
        assert_eq!(result, expected);
        Ok(())
    }

    #[test]
    fn test_format_source_with_custom_config_keeps_rfc053_spacing() -> Result<(), FormatError> {
        let source = r#"trait User:
  def connect(self) -> None: ...
  def reset(self) -> None:
    pass

def build_user() -> User:
  pass
"#;
        let config = FormatConfig::new().with_indent_width(2);
        let formatted = format_source_with_config(source, config)?;

        let expected = r#"trait User:
  def connect(self) -> None

  def reset(self) -> None:
    pass


def build_user() -> User:
  pass
"#;
        assert_eq!(formatted, expected);
        Ok(())
    }

    #[test]
    fn test_format_rust_from_import_with_version_wraps_black_style() -> Result<(), FormatError> {
        let source = r#"from rust::libm @ "0.2" import sqrt as rust_sqrt, fabs as rust_abs, floor as rust_floor, ceil as rust_ceil, pow as rust_pow, exp as rust_exp
"#;
        let config = FormatConfig::new().with_line_length(80).with_trailing_commas(true);
        let result = format_source_with_config(source, config)?;

        assert!(
            result.starts_with("from rust::libm @ \"0.2\" import (\n"),
            "expected parenthesized rust import list; got: {result}"
        );
        assert!(
            result.contains("sqrt as rust_sqrt,\n") && result.contains("pow as rust_pow,\n"),
            "expected one item per line with trailing commas; got: {result}"
        );
        Ok(())
    }

    #[test]
    fn test_format_source_loop_expression_with_break_value_round_trip() -> Result<(), FormatError> {
        let source = r#"def run() -> int:
    return loop:
        break 42
"#;
        let formatted = format_source(source)?;
        assert_eq!(formatted.trim_end(), source.trim_end());
        Ok(())
    }

    #[test]
    fn test_format_merges_adjacent_rust_from_imports_same_target() -> Result<(), FormatError> {
        let source = r#"from rust::libm @ "0.2" import sqrt as rust_sqrt, fabs as rust_abs
from rust::libm @ "0.2" import floor as rust_floor, ceil as rust_ceil
from rust::libm @ "0.2" import pow as rust_pow, exp as rust_exp
"#;
        let config = FormatConfig::new().with_line_length(80).with_trailing_commas(true);
        let result = format_source_with_config(source, config)?;

        let import_prefix = "from rust::libm @ \"0.2\" import";
        assert_eq!(
            result.matches(import_prefix).count(),
            1,
            "expected adjacent compatible rust imports to merge; got: {result}"
        );
        assert!(
            result.contains("sqrt as rust_sqrt,\n")
                && result.contains("floor as rust_floor,\n")
                && result.contains("pow as rust_pow,\n"),
            "expected all merged import items present in wrapped output; got: {result}"
        );
        Ok(())
    }

    #[test]
    fn test_format_source_partial_declarations_and_expression() -> Result<(), FormatError> {
        let source = r#"pub BronzeReader = partial readers.TableReader(layer="bronze", format="delta")
JsonRoute = partial web.route(method="GET")

def reader_for(layer: str) -> Reader:
  return partial make_factory().reader(layer=layer, options={"format": "delta"})
"#;
        let formatted = format_source(source)?;

        let expected = r#"pub BronzeReader = partial readers.TableReader(layer="bronze", format="delta")
JsonRoute = partial web.route(method="GET")


def reader_for(layer: str) -> Reader:
    return partial make_factory().reader(layer=layer, options={"format": "delta"})
"#;
        assert_eq!(formatted, expected);
        assert_eq!(format_source(&formatted)?, expected);
        Ok(())
    }

    #[test]
    fn test_format_source_method_and_trait_partials() -> Result<(), FormatError> {
        let source = r#"model Cell:
  alive: bool
  set_alive = partial set_state(state=true)
  def set_state(mut self, state: bool) -> None:
    self.alive = state

trait Named:
  display = partial name(prefix="name")
  def name(self, prefix: str) -> str
"#;
        let formatted = format_source(source)?;

        let expected = r#"model Cell:
    alive: bool
    set_alive = partial set_state(state=true)

    def set_state(mut self, state: bool) -> None:
        self.alive = state


trait Named:
    display = partial name(prefix="name")

    def name(self, prefix: str) -> str
"#;
        assert_eq!(formatted, expected);
        assert_eq!(format_source(&formatted)?, expected);
        Ok(())
    }
}
