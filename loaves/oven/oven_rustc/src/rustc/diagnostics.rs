//! Structured rustc diagnostics carried back to the caller.
//!
//! What a caller reads when a direct-rustc invocation fails: rustc's own JSON spans and messages, and the report
//! that renders them beside the invocation that produced them.

use std::fmt;

use serde::{Deserialize, Serialize};

/// Structured source span emitted by rustc's JSON diagnostic stream.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OvenRustcDiagnosticSpan {
    /// Rust source filename from rustc.
    pub file_name: String,
    /// One-based start line.
    pub line_start: u32,
    /// One-based start column.
    pub column_start: u32,
    /// One-based end line.
    pub line_end: u32,
    /// One-based end column.
    pub column_end: u32,
    /// Whether rustc identified this as the primary span.
    pub is_primary: bool,
}

/// One structured rustc diagnostic preserved without terminal-only parsing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OvenRustcDiagnostic {
    /// rustc severity level.
    pub level: String,
    /// Human-readable diagnostic message.
    pub message: String,
    /// Optional rustc error code.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    /// Structured source spans.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub spans: Vec<OvenRustcDiagnosticSpan>,
    /// Optional rustc-rendered display form.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rendered: Option<String>,
}

/// Rustc diagnostic transcript for a failed direct Oven consumer compilation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OvenRustcDiagnosticReport {
    /// JSON diagnostics decoded from rustc output.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diagnostics: Vec<OvenRustcDiagnostic>,
    /// Non-JSON rustc output retained verbatim for diagnostics that lack a JSON record.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub unstructured_output: String,
    /// Bounded direct-Rustc command evidence for a failed Oven compilation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invocation: Option<String>,
}

impl fmt::Display for OvenRustcDiagnosticReport {
    /// Render a bounded, actionable terminal summary while the complete structured report remains available to callers.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        const MAX_DIAGNOSTICS: usize = 12;
        const MAX_UNSTRUCTURED_CHARS: usize = 4_000;

        if self.diagnostics.is_empty() && self.unstructured_output.trim().is_empty() && self.invocation.is_none() {
            return formatter.write_str("rustc exited unsuccessfully without emitting diagnostics");
        }
        for (index, diagnostic) in self.diagnostics.iter().take(MAX_DIAGNOSTICS).enumerate() {
            if index > 0 {
                formatter.write_str("\n")?;
            }
            write!(formatter, "{}", diagnostic.level)?;
            if let Some(code) = &diagnostic.code {
                write!(formatter, "[{code}]")?;
            }
            write!(formatter, ": {}", diagnostic.message)?;
            if let Some(span) = diagnostic.spans.iter().find(|span| span.is_primary) {
                write!(
                    formatter,
                    " at {}:{}:{}",
                    span.file_name, span.line_start, span.column_start
                )?;
            }
        }
        if self.diagnostics.len() > MAX_DIAGNOSTICS {
            write!(
                formatter,
                "\n… {} additional rustc diagnostic(s) omitted from terminal summary",
                self.diagnostics.len() - MAX_DIAGNOSTICS
            )?;
        }
        let unstructured = self.unstructured_output.trim();
        if !unstructured.is_empty() {
            if !self.diagnostics.is_empty() {
                formatter.write_str("\n")?;
            }
            for character in unstructured.chars().take(MAX_UNSTRUCTURED_CHARS) {
                write!(formatter, "{character}")?;
            }
            if unstructured.chars().count() > MAX_UNSTRUCTURED_CHARS {
                formatter.write_str("\n… rustc unstructured output truncated")?;
            }
        }
        if let Some(invocation) = &self.invocation {
            if !self.diagnostics.is_empty() || !unstructured.is_empty() {
                formatter.write_str("\n")?;
            }
            write!(formatter, "direct rustc invocation: {invocation}")?;
        }
        Ok(())
    }
}
