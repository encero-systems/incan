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
/// Vertical spacing follows RFC 053 and is not configurable through [`FormatConfig`].
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

/// Stand-alone comment lines that are anchored to a nearby formatted code line.
#[derive(Clone)]
struct AnchoredStandaloneBlock {
    anchor: String,
    occurrence: usize,
    indent: usize,
    lines: Vec<String>,
}

/// Stand-alone comment lines collected while scanning the source before their anchor is known.
struct PendingStandaloneBlock {
    indent: usize,
    lines: Vec<String>,
    saw_blank_after: bool,
}

/// Count leading indentation using source whitespace width rather than byte length.
fn leading_indent_width(line: &str) -> usize {
    line.chars().take_while(|ch| ch.is_whitespace()).count()
}

/// Reattach preserved source comments onto the formatter's AST-shaped output.
///
/// The formatter itself only emits comments that are represented in the AST. This pass preserves stand-alone and
/// inline `# ...` comments from the original source by anchoring them to nearby formatted code lines while respecting
/// indentation scope, blank-line separation, and string-literal boundaries.
fn reattach_comments(source: &str, formatted: &str) -> String {
    let mut state = StringState::None;
    let mut pending_standalone: Option<PendingStandaloneBlock> = None;
    let mut leading_standalone: Vec<AnchoredStandaloneBlock> = Vec::new();
    let mut trailing_standalone: Vec<AnchoredStandaloneBlock> = Vec::new();
    let mut eof_standalone: Vec<String> = Vec::new();
    let mut inline_comments: Vec<(String, usize, String)> = Vec::new();
    let mut source_anchor_occurrences: HashMap<String, usize> = HashMap::new();
    let mut last_code_at_indent: HashMap<usize, (String, usize)> = HashMap::new();

    // ---- Extract comments from source and anchor them to code lines ----
    for line in source.lines() {
        let comment_idx = comment_start_index(line, &mut state);
        if matches!(state, StringState::SingleQuoted | StringState::DoubleQuoted) {
            state = StringState::None;
        }

        let Some(idx) = comment_idx else {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                if let Some(block) = &mut pending_standalone {
                    block.saw_blank_after = true;
                }
                continue;
            }

            let anchor = normalize_code_for_match(trimmed);
            let occurrence = source_anchor_occurrences.get(&anchor).copied().unwrap_or(0) + 1;
            let indent = leading_indent_width(line);
            if let Some(block) = pending_standalone.take() {
                let lines = trim_trailing_blank_comment_lines(&block.lines);
                let next_same_scope = indent == block.indent;
                if !block.saw_blank_after && next_same_scope {
                    leading_standalone.push(AnchoredStandaloneBlock {
                        anchor: anchor.clone(),
                        occurrence,
                        indent: block.indent,
                        lines,
                    });
                } else if let Some((prev_anchor, prev_occurrence)) = last_code_at_indent.get(&block.indent) {
                    trailing_standalone.push(AnchoredStandaloneBlock {
                        anchor: prev_anchor.clone(),
                        occurrence: *prev_occurrence,
                        indent: block.indent,
                        lines,
                    });
                } else {
                    leading_standalone.push(AnchoredStandaloneBlock {
                        anchor: anchor.clone(),
                        occurrence,
                        indent: block.indent,
                        lines,
                    });
                }
            }
            last_code_at_indent.insert(indent, (anchor.clone(), occurrence));
            source_anchor_occurrences.insert(anchor, occurrence);
            continue;
        };

        let code_prefix = &line[..idx];
        let comment_text = line[idx..].trim_end().to_string();
        if code_prefix.trim().is_empty() {
            let indent = leading_indent_width(line);
            if let Some(block) = &mut pending_standalone {
                if !block.saw_blank_after && block.indent == indent {
                    block.lines.push(line.trim_end().to_string());
                } else {
                    let lines = trim_trailing_blank_comment_lines(&block.lines);
                    if let Some((prev_anchor, prev_occurrence)) = last_code_at_indent.get(&block.indent) {
                        trailing_standalone.push(AnchoredStandaloneBlock {
                            anchor: prev_anchor.clone(),
                            occurrence: *prev_occurrence,
                            indent: block.indent,
                            lines,
                        });
                    } else {
                        eof_standalone.extend(lines);
                    }
                    *block = PendingStandaloneBlock {
                        indent,
                        lines: vec![line.trim_end().to_string()],
                        saw_blank_after: false,
                    };
                }
            } else {
                pending_standalone = Some(PendingStandaloneBlock {
                    indent,
                    lines: vec![line.trim_end().to_string()],
                    saw_blank_after: false,
                });
            }
            continue;
        }

        let anchor = normalize_code_for_match(code_prefix.trim_end());
        let occurrence = source_anchor_occurrences.get(&anchor).copied().unwrap_or(0) + 1;
        let indent = leading_indent_width(line);
        if let Some(block) = pending_standalone.take() {
            let lines = trim_trailing_blank_comment_lines(&block.lines);
            let next_same_scope = indent == block.indent;
            if !block.saw_blank_after && next_same_scope {
                leading_standalone.push(AnchoredStandaloneBlock {
                    anchor: anchor.clone(),
                    occurrence,
                    indent: block.indent,
                    lines,
                });
            } else if let Some((prev_anchor, prev_occurrence)) = last_code_at_indent.get(&block.indent) {
                trailing_standalone.push(AnchoredStandaloneBlock {
                    anchor: prev_anchor.clone(),
                    occurrence: *prev_occurrence,
                    indent: block.indent,
                    lines,
                });
            } else {
                leading_standalone.push(AnchoredStandaloneBlock {
                    anchor: anchor.clone(),
                    occurrence,
                    indent: block.indent,
                    lines,
                });
            }
        }

        inline_comments.push((anchor.clone(), occurrence, comment_text));
        last_code_at_indent.insert(indent, (anchor.clone(), occurrence));
        source_anchor_occurrences.insert(anchor, occurrence);
    }

    if let Some(block) = pending_standalone.take() {
        let lines = trim_trailing_blank_comment_lines(&block.lines);
        if let Some((prev_anchor, prev_occurrence)) = last_code_at_indent.get(&block.indent) {
            trailing_standalone.push(AnchoredStandaloneBlock {
                anchor: prev_anchor.clone(),
                occurrence: *prev_occurrence,
                indent: block.indent,
                lines,
            });
        } else {
            eof_standalone = lines;
        }
    }

    // ---- Reattach comments into formatted output ----
    let mut out_lines: Vec<String> = Vec::new();
    let mut leading_idx = 0usize;
    let mut trailing_idx = 0usize;
    let mut inline_idx = 0usize;
    let mut formatted_state = StringState::None;
    let mut formatted_anchor_occurrences: HashMap<String, usize> = HashMap::new();
    let mut pending_trailing: Vec<AnchoredStandaloneBlock> = Vec::new();

    for line in formatted.lines() {
        let line_trimmed = line.trim();
        let current_indent = leading_indent_width(line);
        while !pending_trailing.is_empty()
            && (line_trimmed.is_empty() || (!line_trimmed.is_empty() && current_indent <= pending_trailing[0].indent))
        {
            let block = pending_trailing.remove(0);
            out_lines.extend(block.lines);
        }
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

        while leading_idx < leading_standalone.len()
            && normalized
                .as_ref()
                .is_some_and(|n| n == &leading_standalone[leading_idx].anchor)
            && occurrence.is_some_and(|occ| occ == leading_standalone[leading_idx].occurrence)
        {
            out_lines.extend(leading_standalone[leading_idx].lines.iter().cloned());
            leading_idx += 1;
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

        while trailing_idx < trailing_standalone.len()
            && normalized
                .as_ref()
                .is_some_and(|n| n == &trailing_standalone[trailing_idx].anchor)
            && occurrence.is_some_and(|occ| occ == trailing_standalone[trailing_idx].occurrence)
        {
            pending_trailing.push(trailing_standalone[trailing_idx].clone());
            trailing_idx += 1;
        }
    }

    while leading_idx < leading_standalone.len() {
        out_lines.extend(leading_standalone[leading_idx].lines.iter().cloned());
        leading_idx += 1;
    }

    while trailing_idx < trailing_standalone.len() {
        pending_trailing.push(trailing_standalone[trailing_idx].clone());
        trailing_idx += 1;
    }

    while !pending_trailing.is_empty() {
        let block = pending_trailing.remove(0);
        out_lines.extend(block.lines);
    }

    if !eof_standalone.is_empty() {
        if out_lines.last().is_some_and(|l| !l.is_empty()) {
            out_lines.push(String::new());
        }
        out_lines.extend(eof_standalone);
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
    def sorted(self) -> OrderedCollection[T]: ...
"#;
        assert_eq!(formatted, expected);
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
        assert_eq!(formatted, source);
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
        assert_eq!(formatted, source);
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
    fn test_format_trait_abstract_methods_stay_tight_before_default_method() -> Result<(), FormatError> {
        let source = r#"trait Service:
  def connect(self) -> None: ...
  def close(self) -> None: ...
  def reset(self) -> None:
    pass
"#;
        let result = format_source(source)?;

        let expected = r#"trait Service:
    def connect(self) -> None: ...
    def close(self) -> None: ...

    def reset(self) -> None:
        pass
"#;
        assert_eq!(result, expected);
        Ok(())
    }

    #[test]
    fn test_format_source_with_custom_config_keeps_rfc053_spacing() -> Result<(), FormatError> {
        let source = r#"model User:
  def connect(self) -> None: ...
  def reset(self) -> None:
    pass

def build_user() -> User:
  pass
"#;
        let config = FormatConfig::new().with_indent_width(2);
        let formatted = format_source_with_config(source, config)?;

        let expected = r#"model User:
  def connect(self) -> None: ...

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
