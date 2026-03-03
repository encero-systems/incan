//! Core error types for the Incan compiler diagnostics pipeline.
//!
//! This module defines [`CompileError`] — the single error type threaded through the
//! lexer, parser, and typechecker — along with its severity ([`ErrorKind`]) and
//! plain-text rendering helpers.

use crate::ast::Span;

/// A compile-time error with location information.
///
/// Every diagnostic produced by the Incan compiler is represented as a
/// `CompileError`.  The [`errors`](super::errors) and [`lints`](super::lints)
/// catalog modules provide constructor functions that build well-formatted
/// instances with appropriate hints and notes.
#[derive(Debug, Clone, PartialEq)]
pub struct CompileError {
    /// Human-readable error message (the main line shown to the user).
    pub message: String,
    /// Source location where the error was detected.
    pub span: Span,
    /// Severity / category of the diagnostic.
    pub kind: ErrorKind,
    /// Additional context lines ("= note: …") rendered after the source snippet.
    pub notes: Vec<String>,
    /// Actionable suggestions ("= hint: …") rendered after the notes.
    pub hints: Vec<String>,
}

impl CompileError {
    /// Create a generic error (kind = [`ErrorKind::Error`]).
    pub fn new(message: String, span: Span) -> Self {
        Self {
            message,
            span,
            kind: ErrorKind::Error,
            notes: Vec::new(),
            hints: Vec::new(),
        }
    }

    /// Create a syntax error (kind = [`ErrorKind::Syntax`]).
    pub fn syntax(message: String, span: Span) -> Self {
        Self {
            message,
            span,
            kind: ErrorKind::Syntax,
            notes: Vec::new(),
            hints: Vec::new(),
        }
    }

    /// Create a type error (kind = [`ErrorKind::Type`]).
    pub fn type_error(message: String, span: Span) -> Self {
        Self {
            message,
            span,
            kind: ErrorKind::Type,
            notes: Vec::new(),
            hints: Vec::new(),
        }
    }

    /// Create a non-fatal warning (kind = [`ErrorKind::Warning`]).
    ///
    /// Warnings do not prevent compilation. They surface in the CLI output and LSP diagnostics as yellow squiggles /
    /// `warning:` labels.
    pub fn warning(message: String, span: Span) -> Self {
        Self {
            message,
            span,
            kind: ErrorKind::Warning,
            notes: Vec::new(),
            hints: Vec::new(),
        }
    }

    /// Append a contextual note ("= note: …") to this error.
    pub fn with_note(mut self, note: impl Into<String>) -> Self {
        self.notes.push(note.into());
        self
    }

    /// Append an actionable hint ("= hint: …") to this error.
    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hints.push(hint.into());
        self
    }
}

/// Severity level for a [`CompileError`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    /// General compilation error.
    Error,
    /// Syntax / parse error.
    Syntax,
    /// Type-checking error.
    Type,
    /// Non-fatal warning.
    Warning,
    /// Style / lint advisory.
    Lint,
}

impl std::fmt::Display for ErrorKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ErrorKind::Error => write!(f, "error"),
            ErrorKind::Syntax => write!(f, "syntax error"),
            ErrorKind::Type => write!(f, "type error"),
            ErrorKind::Warning => write!(f, "warning"),
            ErrorKind::Lint => write!(f, "lint"),
        }
    }
}

/// Format an error with source context and return as a `String`.
///
/// Produces a rustc-style diagnostic with coloured header, source line,
/// underline caret, notes and hints.  Useful for CLI error handling where
/// errors are collected into a `Result` instead of printed immediately.
pub fn format_error(file_name: &str, source: &str, error: &CompileError) -> String {
    let (line_num, col_num, line_text) = get_line_info(source, error.span.start);

    // Color codes
    let red = "\x1b[31m";
    let cyan = "\x1b[36m";
    let yellow = "\x1b[33m";
    let bold = "\x1b[1m";
    let reset = "\x1b[0m";

    let kind_color = match error.kind {
        ErrorKind::Error | ErrorKind::Syntax | ErrorKind::Type => red,
        ErrorKind::Warning | ErrorKind::Lint => yellow,
    };

    let mut out = String::new();

    // Header
    out.push_str(&format!(
        "{bold}{kind_color}{kind}{reset}{bold}: {message}{reset}\n",
        kind = error.kind,
        message = error.message,
    ));

    // Location
    out.push_str(&format!(
        "  {cyan}-->{reset} {file}:{line}:{col}\n",
        file = file_name,
        line = line_num,
        col = col_num,
    ));

    // Source line with line number
    let line_num_width = format!("{}", line_num).len();
    out.push_str(&format!("  {cyan}{:>width$} |{reset}\n", "", width = line_num_width));
    out.push_str(&format!(
        "  {cyan}{:>width$} |{reset} {}\n",
        line_num,
        line_text,
        width = line_num_width
    ));

    // Caret pointing to error
    let underline_len = if error.span.end > error.span.start && col_num > 0 {
        let start_offset = error.span.start.saturating_sub(col_num.saturating_sub(1));
        let end_in_line = error.span.end.saturating_sub(start_offset);
        end_in_line
            .min(line_text.len())
            .saturating_sub(col_num.saturating_sub(1))
            .max(1)
    } else {
        1
    };

    out.push_str(&format!(
        "  {cyan}{:>width$} |{reset} {}{kind_color}{}{reset}\n",
        "",
        " ".repeat(col_num - 1),
        "^".repeat(underline_len),
        width = line_num_width
    ));

    // Notes
    for note in &error.notes {
        out.push_str(&format!("  {cyan}= note:{reset} {}\n", note));
    }

    // Hints
    for hint in &error.hints {
        out.push_str(&format!("  {cyan}= hint:{reset} {}\n", hint));
    }

    out
}

/// Print an error with source context (simple implementation)
pub fn print_error(file_name: &str, source: &str, error: &CompileError) {
    eprint!("{}", format_error(file_name, source, error));
}

/// Get line number, column number, and line text for a byte offset
fn get_line_info(source: &str, offset: usize) -> (usize, usize, &str) {
    let offset = offset.min(source.len());
    let mut line_num = 1;
    let mut line_start = 0;

    for (i, c) in source.char_indices() {
        if i >= offset {
            break;
        }
        if c == '\n' {
            line_num += 1;
            line_start = i + 1;
        }
    }

    let line_end = source[line_start..]
        .find('\n')
        .map(|i| line_start + i)
        .unwrap_or(source.len());

    let line_text = &source[line_start..line_end];
    let col_num = offset - line_start + 1;

    (line_num, col_num, line_text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_line_info() {
        let source = "line 1\nline 2\nline 3";

        let (line, col, text) = get_line_info(source, 0);
        assert_eq!(line, 1);
        assert_eq!(col, 1);
        assert_eq!(text, "line 1");

        let (line, col, text) = get_line_info(source, 7);
        assert_eq!(line, 2);
        assert_eq!(col, 1);
        assert_eq!(text, "line 2");

        let (line, col, text) = get_line_info(source, 10);
        assert_eq!(line, 2);
        assert_eq!(col, 4);
        assert_eq!(text, "line 2");
    }
}
