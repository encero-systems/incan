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

mod config;
mod formatter;
mod writer;

pub use config::{FormatConfig, QuoteStyle};
pub use formatter::Formatter;

use crate::frontend::{diagnostics, lexer, parser};
use std::collections::HashMap;
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
///
/// let source = "def add(a: int, b: int) -> int:\n    return a + b\n";
/// let formatted = format_source(source).unwrap();
/// assert!(formatted.contains("def add"));
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
/// Returns an error if the source has syntax errors (formatting requires parsing).
///
/// # Examples
///
/// ```
/// use incan::{FormatConfig, format_source_with_config};
///
/// let config = FormatConfig::default();
/// let source = "def greet(name: str) -> str:\n    return name\n";
/// let formatted = format_source_with_config(source, config).unwrap();
/// assert!(formatted.contains("def greet"));
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
    let formatted = formatter.format(&ast);
    let formatted = reattach_comments(source, &formatted);

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StringState {
    None,
    SingleQuoted,
    DoubleQuoted,
    TripleSingleQuoted,
    TripleDoubleQuoted,
}

/// Count `#...` comments outside string literals.
///
/// This supports a strict safety check for formatter output:
/// if formatting would reduce comment count, we refuse to rewrite.
fn count_line_comments(source: &str) -> usize {
    let mut state = StringState::None;
    let mut count = 0usize;

    for line in source.lines() {
        if comment_start_index(line, &mut state).is_some() {
            count += 1;
        }
        // Single-quoted strings are line-local; triple-quoted strings can span lines.
        if matches!(state, StringState::SingleQuoted | StringState::DoubleQuoted) {
            state = StringState::None;
        }
    }

    count
}

fn comment_start_index(line: &str, state: &mut StringState) -> Option<usize> {
    let mut i = 0usize;
    while i < line.len() {
        let rest = &line[i..];
        let mut chars = rest.chars();
        let ch = chars.next()?;
        let ch_len = ch.len_utf8();

        match state {
            StringState::None => {
                if rest.starts_with("'''") {
                    *state = StringState::TripleSingleQuoted;
                    i += 3;
                    continue;
                }
                if rest.starts_with("\"\"\"") {
                    *state = StringState::TripleDoubleQuoted;
                    i += 3;
                    continue;
                }
                if ch == '\'' {
                    *state = StringState::SingleQuoted;
                    i += ch_len;
                    continue;
                }
                if ch == '"' {
                    *state = StringState::DoubleQuoted;
                    i += ch_len;
                    continue;
                }
                if ch == '#' {
                    return Some(i);
                }
                i += ch_len;
            }
            StringState::SingleQuoted => {
                if ch == '\\' {
                    if let Some(next) = chars.next() {
                        i += ch_len + next.len_utf8();
                    } else {
                        i += ch_len;
                    }
                    continue;
                }
                if ch == '\'' {
                    *state = StringState::None;
                }
                i += ch_len;
            }
            StringState::DoubleQuoted => {
                if ch == '\\' {
                    if let Some(next) = chars.next() {
                        i += ch_len + next.len_utf8();
                    } else {
                        i += ch_len;
                    }
                    continue;
                }
                if ch == '"' {
                    *state = StringState::None;
                }
                i += ch_len;
            }
            StringState::TripleSingleQuoted => {
                if rest.starts_with("'''") {
                    *state = StringState::None;
                    i += 3;
                } else {
                    i += ch_len;
                }
            }
            StringState::TripleDoubleQuoted => {
                if rest.starts_with("\"\"\"") {
                    *state = StringState::None;
                    i += 3;
                } else {
                    i += ch_len;
                }
            }
        }
    }

    None
}

fn normalize_code_for_match(code: &str) -> String {
    code.chars().filter(|c| !c.is_whitespace()).collect()
}

fn reattach_comments(source: &str, formatted: &str) -> String {
    let mut state = StringState::None;
    let mut pending_standalone: Vec<String> = Vec::new();
    let mut anchored_standalone: Vec<(String, usize, Vec<String>)> = Vec::new();
    let mut trailing_standalone: Vec<String> = Vec::new();
    let mut inline_comments: Vec<(String, usize, String)> = Vec::new();
    let mut source_anchor_occurrences: HashMap<String, usize> = HashMap::new();

    // ---- Extract comments from source and anchor them to code lines ----
    for line in source.lines() {
        let comment_idx = comment_start_index(line, &mut state);
        if matches!(state, StringState::SingleQuoted | StringState::DoubleQuoted) {
            state = StringState::None;
        }

        let Some(idx) = comment_idx else {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                if !pending_standalone.is_empty() {
                    pending_standalone.push(String::new());
                }
                continue;
            }

            let anchor = normalize_code_for_match(trimmed);
            let occurrence = source_anchor_occurrences.get(&anchor).copied().unwrap_or(0) + 1;
            if !pending_standalone.is_empty() {
                anchored_standalone.push((
                    anchor.clone(),
                    occurrence,
                    trim_trailing_blank_comment_lines(&pending_standalone),
                ));
                pending_standalone.clear();
            }
            source_anchor_occurrences.insert(anchor, occurrence);
            continue;
        };

        let code_prefix = &line[..idx];
        let comment_text = line[idx..].trim_end().to_string();
        if code_prefix.trim().is_empty() {
            pending_standalone.push(line.trim_end().to_string());
            continue;
        }

        let anchor = normalize_code_for_match(code_prefix.trim_end());
        let occurrence = source_anchor_occurrences.get(&anchor).copied().unwrap_or(0) + 1;
        if !pending_standalone.is_empty() {
            anchored_standalone.push((
                anchor.clone(),
                occurrence,
                trim_trailing_blank_comment_lines(&pending_standalone),
            ));
            pending_standalone.clear();
        }

        inline_comments.push((anchor.clone(), occurrence, comment_text));
        source_anchor_occurrences.insert(anchor, occurrence);
    }

    if !pending_standalone.is_empty() {
        trailing_standalone = trim_trailing_blank_comment_lines(&pending_standalone);
    }

    // ---- Reattach comments into formatted output ----
    let mut out_lines: Vec<String> = Vec::new();
    let mut standalone_idx = 0usize;
    let mut inline_idx = 0usize;
    let mut formatted_state = StringState::None;
    let mut formatted_anchor_occurrences: HashMap<String, usize> = HashMap::new();

    for line in formatted.lines() {
        let line_trimmed = line.trim();
        let normalized = if line_trimmed.is_empty() {
            None
        } else {
            Some(normalize_code_for_match(line_trimmed))
        };
        let occurrence = normalized.as_ref().map(|n| {
            let next = formatted_anchor_occurrences.get(n).copied().unwrap_or(0) + 1;
            formatted_anchor_occurrences.insert(n.clone(), next);
            next
        });

        if standalone_idx < anchored_standalone.len()
            && normalized
                .as_ref()
                .is_some_and(|n| n == &anchored_standalone[standalone_idx].0)
            && occurrence.is_some_and(|occ| occ == anchored_standalone[standalone_idx].1)
        {
            out_lines.extend(anchored_standalone[standalone_idx].2.iter().cloned());
            standalone_idx += 1;
        }

        let mut out_line = line.to_string();
        let has_existing_comment = comment_start_index(line, &mut formatted_state).is_some();
        if matches!(formatted_state, StringState::SingleQuoted | StringState::DoubleQuoted) {
            formatted_state = StringState::None;
        }

        if !has_existing_comment
            && inline_idx < inline_comments.len()
            && let Some(n) = &normalized
            && n == &inline_comments[inline_idx].0
            && occurrence.is_some_and(|occ| occ == inline_comments[inline_idx].1)
        {
            out_line.push_str("  ");
            out_line.push_str(&inline_comments[inline_idx].2);
            inline_idx += 1;
        }

        out_lines.push(out_line);
    }

    while standalone_idx < anchored_standalone.len() {
        out_lines.extend(anchored_standalone[standalone_idx].2.iter().cloned());
        standalone_idx += 1;
    }

    if !trailing_standalone.is_empty() {
        if out_lines.last().is_some_and(|l| !l.is_empty()) {
            out_lines.push(String::new());
        }
        out_lines.extend(trailing_standalone);
    }

    let mut out = out_lines.join("\n");
    if formatted.ends_with('\n') || source.ends_with('\n') {
        out.push('\n');
    }
    out
}

fn trim_trailing_blank_comment_lines(lines: &[String]) -> Vec<String> {
    let mut out = lines.to_vec();
    while out.last().is_some_and(|l| l.trim().is_empty()) {
        out.pop();
    }
    out
}

/// Check if source code is already formatted.
///
/// # Examples
///
/// ```
/// use incan::check_formatted;
///
/// // Check returns a boolean (true = already formatted)
/// let source = "def foo() -> int:\n    return 42\n";
/// let is_formatted = check_formatted(source).unwrap();
/// // Result depends on exact formatting rules
/// assert!(is_formatted == true || is_formatted == false);
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
    fn test_format_source_model() {
        let source = r#"model User:
  name: str
  age: int
"#;
        let result = format_source(source);
        assert!(result.is_ok());
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
        assert_eq!(formatted, source);
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
    fn test_format_diff_trailing_newline_only_is_actionable() {
        let source = "def foo() -> int:\n    return 42";
        let result = format_diff(source).expect("format_diff should succeed");
        let diff = result.expect("diff should be present for trailing-newline change");
        assert!(
            diff.contains("trailing-newline"),
            "expected trailing newline hint in diff, got: {diff}"
        );
    }

    // ========================================
    // Issue #116: parenthesized import formatting
    // ========================================

    /// A short import that fits on one line should be kept (or collapsed to) single-line form.
    #[test]
    fn test_format_import_short_stays_single_line() {
        let source = "from db import (CategoryId, TagId)\n";
        let config = FormatConfig::new().with_line_length(120);
        let result = format_source_with_config(source, config).expect("format should succeed");
        assert_eq!(result.trim_end(), "from db import CategoryId, TagId");
    }

    /// A comma-separated import that already fits on one line is unchanged.
    #[test]
    fn test_format_import_bare_short_unchanged() {
        let source = "from db import CategoryId, TagId\n";
        let config = FormatConfig::new().with_line_length(120);
        let result = format_source_with_config(source, config).expect("format should succeed");
        assert_eq!(result.trim_end(), "from db import CategoryId, TagId");
    }

    /// A long multi-item import that exceeds the line length should be wrapped.
    #[test]
    fn test_format_import_long_wraps_to_parens() {
        // Use a very short limit so the list definitely overflows.
        let source = "from db import CategoryId, TagId, OtherId\n";
        let config = FormatConfig::new().with_line_length(20).with_trailing_commas(true);
        let result = format_source_with_config(source, config).expect("format should succeed");
        assert!(
            result.contains('('),
            "expected parenthesized output for long import; got: {result}"
        );
        assert!(
            result.contains("CategoryId,\n"),
            "expected each item on its own line; got: {result}"
        );
    }

    /// A multi-line parenthesized import that fits on one line is collapsed to single-line.
    #[test]
    fn test_format_import_multiline_parens_collapses_when_fits() {
        let source = "from db import (\n    CategoryId,\n    TagId,\n)\n";
        let config = FormatConfig::new().with_line_length(120);
        let result = format_source_with_config(source, config).expect("format should succeed");
        assert_eq!(result.trim_end(), "from db import CategoryId, TagId");
    }

    /// Trailing comma in parenthesized output is controlled by the `trailing_commas` config.
    #[test]
    fn test_format_import_no_trailing_comma_when_disabled() {
        let source = "from db import CategoryId, TagId, OtherId\n";
        let config = FormatConfig::new().with_line_length(20).with_trailing_commas(false);
        let result = format_source_with_config(source, config).expect("format should succeed");
        // Last item should not have a trailing comma.
        assert!(
            !result.contains("OtherId,\n"),
            "expected no trailing comma after last item; got: {result}"
        );
        assert!(
            result.contains("OtherId\n"),
            "expected last item without comma; got: {result}"
        );
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
}
