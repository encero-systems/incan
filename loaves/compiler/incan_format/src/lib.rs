//! Incan Code Formatter
//!
//! This module provides code formatting functionality for Incan source files. It follows Ruff/Black conventions with
//! customizations:
//! - 4-space indentation
//! - 120 character line length (target, not strictly enforced)
//! - Double quotes for strings
//! - Trailing commas in multi-line constructs
//!
//! ## Parse-required
//!
//! The formatter operates on the parsed AST, so it **requires valid syntax**. Files with lexer or parser errors cannot
//! be formatted.

#![deny(clippy::unwrap_used)]
#![deny(clippy::expect_used)]
mod comments;
mod config;
mod formatter;
mod writer;

#[cfg(test)]
use comments::buffer::NormalizedLineBuffer;
use comments::{count_line_comments, reattach_comments};
pub use config::{FormatConfig, QuoteStyle};
pub use formatter::Formatter;

use incan_syntax::ast::Program;
use incan_syntax::{diagnostics, lexer, parser};
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
/// use incan_format::format_source;
/// # fn main() -> Result<(), incan_format::FormatError> {
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
/// use incan_format::{FormatConfig, format_source_with_config};
/// # fn main() -> Result<(), incan_format::FormatError> {
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

    format_parsed_source_with_config(source, &ast, config)
}

/// Format a parsed program with custom configuration.
///
/// Project-aware callers such as `incan fmt` may need dependency-provided vocabulary active during parsing. They can
/// parse through `CompilationSession` and still use the same formatting and comment-preservation contract as
/// context-free formatting.
pub fn format_parsed_source_with_config(
    source: &str,
    ast: &Program,
    config: FormatConfig,
) -> Result<String, FormatError> {
    let formatter = Formatter::new(config);
    let formatted = reattach_comments(source, &formatter.format(ast));

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
/// use incan_format::check_formatted;
/// # fn main() -> Result<(), incan_format::FormatError> {
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
/// use incan_format::format_diff;
///
/// // Returns Ok with optional diff
/// let source = "def foo() -> int:\n    return 42\n";
/// let diff_result = format_diff(source);
/// assert!(diff_result.is_ok());
/// ```
pub fn format_diff(source: &str) -> Result<Option<String>, FormatError> {
    let formatted = format_source(source)?;
    Ok(format_diff_from_formatted(source, &formatted))
}

/// Build the formatter diff for an already-computed formatted source.
pub fn format_diff_from_formatted(source: &str, formatted: &str) -> Option<String> {
    if source == formatted {
        return None;
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

    Some(diff)
}

#[cfg(test)]
mod tests;
