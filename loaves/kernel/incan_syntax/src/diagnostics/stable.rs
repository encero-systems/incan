//! Stable diagnostic codes and machine-readable projection helpers.
//!
//! The compiler still has many specialized human diagnostic constructors. This module is the stable reporting layer
//! used by CLI JSON output and explain/help surfaces. It intentionally starts with broad phase-level codes, then can
//! grow narrower codes without making callers scrape terminal prose.

use incan_semantics_core::CanonicalSymbolId;
use serde::Serialize;

use crate::ast::{Declaration, Program, Span};

use super::{CompileError, ErrorKind};

mod catalog;

pub use catalog::*;

/// Schema version for machine-readable diagnostic reports.
pub const DIAGNOSTIC_SCHEMA_VERSION: u32 = 2;

/// Pipeline phase that produced a diagnostic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticPhase {
    Lex,
    Parse,
    Typecheck,
    Import,
    Tooling,
    Unknown,
}

/// Compiler subsystem that produced a diagnostic fact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticOrigin {
    /// Lexer tokenization.
    Lexer,
    /// Parser or syntax validation.
    Parser,
    /// Import, module, or package resolution.
    ImportResolver,
    /// Type checking and semantic validation.
    Typechecker,
    /// CLI or surrounding tooling.
    Tooling,
    /// The producer is not known precisely enough yet.
    Unknown,
}

impl DiagnosticOrigin {
    /// Return the stable lowercase origin label for non-Serde consumers.
    pub fn as_str(self) -> &'static str {
        match self {
            DiagnosticOrigin::Lexer => "lexer",
            DiagnosticOrigin::Parser => "parser",
            DiagnosticOrigin::ImportResolver => "import_resolver",
            DiagnosticOrigin::Typechecker => "typechecker",
            DiagnosticOrigin::Tooling => "tooling",
            DiagnosticOrigin::Unknown => "unknown",
        }
    }
}

impl DiagnosticPhase {
    /// Return the stable lowercase phase label used by human text and non-Serde call sites.
    pub fn as_str(self) -> &'static str {
        match self {
            DiagnosticPhase::Lex => "lex",
            DiagnosticPhase::Parse => "parse",
            DiagnosticPhase::Typecheck => "typecheck",
            DiagnosticPhase::Import => "import",
            DiagnosticPhase::Tooling => "tooling",
            DiagnosticPhase::Unknown => "unknown",
        }
    }
}

/// Public diagnostic catalog entry returned by `incan explain`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct DiagnosticCatalogEntry {
    /// Stable public diagnostic code, such as `INCAN-T0001`.
    pub code: &'static str,
    /// Short human-readable title for the diagnostic family.
    pub title: &'static str,
    /// Default severity label exposed by the diagnostic catalog.
    pub severity: &'static str,
    /// Compiler or tooling phase that owns this catalog entry.
    pub phase: &'static str,
    /// One-sentence description of the problem class.
    pub summary: &'static str,
    /// Longer explanation printed by `incan explain`.
    pub explanation: &'static str,
    /// Small source or command examples that can produce this diagnostic family.
    pub examples: &'static [&'static str],
    /// Common root causes shown in text and JSON explain output.
    pub common_causes: &'static [&'static str],
    /// Suggested remediation steps for this diagnostic family.
    pub fixes: &'static [&'static str],
    /// Optional documentation URL with deeper guidance.
    pub docs_url: Option<&'static str>,
}

/// 1-based source position plus original byte offset.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DiagnosticPosition {
    /// 1-based source line.
    pub line: usize,
    /// 1-based source column counted in Unicode scalar values.
    pub column: usize,
    /// Original UTF-8 byte offset into the source text.
    pub offset: usize,
}

/// Primary diagnostic span.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DiagnosticSpan {
    /// Source file path used for this diagnostic projection.
    pub file: String,
    /// Inclusive start position.
    pub start: DiagnosticPosition,
    /// Exclusive end position.
    pub end: DiagnosticPosition,
}

/// One compiler-owned related location in a structured diagnostic fact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DiagnosticRelatedSpan {
    /// The secondary source span.
    pub span: DiagnosticSpan,
    /// Why this source span is related to the primary diagnostic.
    pub label: String,
}

/// One declaration related to a diagnostic, retaining its canonical source origin and provider-local byte span.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DiagnosticRelatedDeclaration {
    pub identity: CanonicalSymbolId,
    pub label: String,
}

/// Machine-readable diagnostic payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StableDiagnostic {
    /// Stable public diagnostic code selected from the catalog.
    pub code: &'static str,
    /// Concrete severity for this diagnostic instance.
    pub severity: &'static str,
    /// Compiler or tooling phase that produced this diagnostic.
    pub phase: DiagnosticPhase,
    /// Compiler subsystem that produced the fact.
    pub origin: DiagnosticOrigin,
    /// User-facing diagnostic message.
    pub message: String,
    /// Primary source span for editors and structured tooling.
    pub primary_span: DiagnosticSpan,
    /// Additional explanatory notes carried by the compiler diagnostic.
    pub notes: Vec<String>,
    /// Suggested fixes or hints carried by the compiler diagnostic.
    pub hints: Vec<String>,
    /// Structured expected value or type when available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected: Option<String>,
    /// Structured actual value or type when available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actual: Option<String>,
    /// Related compiler-owned source locations.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub related_spans: Vec<DiagnosticRelatedSpan>,
    /// Related declarations whose offsets are not projected into the primary file's coordinate system.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub related_declarations: Vec<DiagnosticRelatedDeclaration>,
    /// Command users can run to read the catalog explanation for `code`.
    pub explain: String,
}

/// Select the stable public code for a compiler diagnostic.
pub fn code_for_error(error: &CompileError, phase: DiagnosticPhase) -> &'static str {
    if let Some(code) = error.stable_code() {
        return code;
    }
    match phase {
        DiagnosticPhase::Lex | DiagnosticPhase::Parse => PARSER_SYNTAX.code,
        DiagnosticPhase::Typecheck => TYPECHECK.code,
        DiagnosticPhase::Import => IMPORT.code,
        DiagnosticPhase::Tooling => TOOLING.code,
        DiagnosticPhase::Unknown => match error.kind {
            ErrorKind::Syntax => PARSER_SYNTAX.code,
            ErrorKind::Type => TYPECHECK.code,
            ErrorKind::Error | ErrorKind::Warning | ErrorKind::Lint => TOOLING.code,
        },
    }
}

/// Classify diagnostics that are emitted during typechecking but originate from import declaration spans.
///
/// An import the source wrote owns its span. A vocabulary declaration such as `binding` lowers to several declarations
/// that all carry the block's span, an import of the vocabulary's support names among them; that import does not own
/// the span, so a refusal inside the block keeps the typecheck phase (#1561). Such an import is recognized by a
/// declaration of another kind with the same span, which a written import never has.
pub fn phase_for_typecheck_span(program: &Program, span: Span) -> DiagnosticPhase {
    let lowered_with_another_declaration = |import_span: Span| {
        program
            .declarations
            .iter()
            .any(|other| !matches!(other.node, Declaration::Import(_)) && other.span == import_span)
    };
    if program.declarations.iter().any(|declaration| {
        matches!(declaration.node, Declaration::Import(_))
            && spans_overlap(span, declaration.span)
            && !lowered_with_another_declaration(declaration.span)
    }) {
        DiagnosticPhase::Import
    } else {
        DiagnosticPhase::Typecheck
    }
}

/// Convert a compiler diagnostic into the stable JSON-ready representation.
pub fn stable_diagnostic(
    file_name: &str,
    source: &str,
    error: &CompileError,
    phase: DiagnosticPhase,
) -> StableDiagnostic {
    let code = code_for_error(error, phase);
    let severity = match error.kind {
        ErrorKind::Error | ErrorKind::Syntax | ErrorKind::Type => "error",
        ErrorKind::Warning => "warning",
        ErrorKind::Lint => "hint",
    };
    StableDiagnostic {
        code,
        severity,
        phase,
        origin: origin_for_phase(phase),
        message: error.message.clone(),
        primary_span: diagnostic_span(file_name, source, error.span),
        notes: error.notes().to_vec(),
        hints: error.hints().to_vec(),
        expected: error.expected().map(str::to_owned),
        actual: error.actual().map(str::to_owned),
        related_spans: error
            .related_spans()
            .iter()
            .map(|related| DiagnosticRelatedSpan {
                span: diagnostic_span(file_name, source, related.span),
                label: related.label.clone(),
            })
            .collect(),
        related_declarations: error
            .related_declarations()
            .iter()
            .map(|related| DiagnosticRelatedDeclaration {
                identity: related.identity.clone(),
                label: related.label.clone(),
            })
            .collect(),
        explain: format!("incan explain {code}"),
    }
}

/// Select the stable producer identity for one compiler pipeline phase.
fn origin_for_phase(phase: DiagnosticPhase) -> DiagnosticOrigin {
    match phase {
        DiagnosticPhase::Lex => DiagnosticOrigin::Lexer,
        DiagnosticPhase::Parse => DiagnosticOrigin::Parser,
        DiagnosticPhase::Typecheck => DiagnosticOrigin::Typechecker,
        DiagnosticPhase::Import => DiagnosticOrigin::ImportResolver,
        DiagnosticPhase::Tooling => DiagnosticOrigin::Tooling,
        DiagnosticPhase::Unknown => DiagnosticOrigin::Unknown,
    }
}

/// Convert a compiler byte span into the public source-span projection.
fn diagnostic_span(file_name: &str, source: &str, span: Span) -> DiagnosticSpan {
    DiagnosticSpan {
        file: file_name.to_string(),
        start: position_for_offset(source, span.start),
        end: position_for_offset(source, span.end.max(span.start + 1)),
    }
}

/// Convert a byte offset into the 1-based line and column shape used by the stable diagnostic schema.
pub fn position_for_offset(source: &str, offset: usize) -> DiagnosticPosition {
    let offset = offset.min(source.len());
    let mut line = 1usize;
    let mut column = 1usize;
    for (idx, ch) in source.char_indices() {
        if idx >= offset {
            break;
        }
        if ch == '\n' {
            line += 1;
            column = 1;
        } else {
            column += 1;
        }
    }
    DiagnosticPosition { line, column, offset }
}

/// Return whether two source spans overlap after treating zero-width spans as one-byte spans.
fn spans_overlap(left: Span, right: Span) -> bool {
    let left_end = left.end.max(left.start.saturating_add(1));
    let right_end = right.end.max(right.start.saturating_add(1));
    left.start < right_end && right.start < left_end
}

#[cfg(test)]
mod tests;
