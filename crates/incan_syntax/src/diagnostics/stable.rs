//! Stable diagnostic codes and machine-readable projection helpers.
//!
//! The compiler still has many specialized human diagnostic constructors. This module is the stable reporting layer
//! used by CLI JSON output and explain/help surfaces. It intentionally starts with broad phase-level codes, then can
//! grow narrower codes without making callers scrape terminal prose.

use serde::Serialize;

use crate::ast::{Declaration, Program, Span};

use super::{CompileError, ErrorKind};

/// Schema version for machine-readable diagnostic reports.
pub const DIAGNOSTIC_SCHEMA_VERSION: u32 = 1;

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
    /// Command users can run to read the catalog explanation for `code`.
    pub explain: String,
}

const PARSER_SYNTAX: DiagnosticCatalogEntry = DiagnosticCatalogEntry {
    code: "INCAN-P0001",
    title: "Syntax error",
    severity: "error",
    phase: "parse",
    summary: "The source text does not match Incan syntax.",
    explanation: "The lexer or parser could not turn the source into a valid Incan AST. The primary span points at the token or source region where parsing stopped.",
    examples: &["def broken(:", "if value"],
    common_causes: &[
        "A missing expression, colon, delimiter, or indentation boundary.",
        "Using vocabulary syntax without the required imported vocabulary surface.",
    ],
    fixes: &[
        "Check the source around the highlighted span.",
        "Run `incan fmt --check` after the file parses if the intended syntax is valid.",
    ],
    docs_url: Some("https://encero-systems.github.io/incan/language/reference/syntax/"),
};

const TYPECHECK: DiagnosticCatalogEntry = DiagnosticCatalogEntry {
    code: "INCAN-T0001",
    title: "Type checking error",
    severity: "error",
    phase: "typecheck",
    summary: "A parsed program violates Incan's type, symbol, or semantic rules.",
    explanation: "The type checker resolved declarations and expressions but found an invalid symbol, type mismatch, unsupported call shape, or related semantic issue.",
    examples: &["value: int = \"text\"", "unknown_name()"],
    common_causes: &[
        "A missing import or definition.",
        "A value passed to a function, assignment, or return position does not match the expected type.",
    ],
    fixes: &[
        "Read the message, notes, and hints in the diagnostic payload.",
        "Prefer fixing the source contract rather than adding casts or wrappers that hide the mismatch.",
    ],
    docs_url: Some("https://encero-systems.github.io/incan/language/reference/types/"),
};

const IMPORT: DiagnosticCatalogEntry = DiagnosticCatalogEntry {
    code: "INCAN-I0001",
    title: "Import or module resolution error",
    severity: "error",
    phase: "import",
    summary: "The compiler could not resolve or load a source, stdlib, Rust, or public package import.",
    explanation: "Import diagnostics cover missing source modules, private exports, unresolved `pub::` libraries, invalid dependency manifests, and Rust bridge resolution failures.",
    examples: &["from missing import value", "from pub::unknown import helper"],
    common_causes: &[
        "The imported module does not exist relative to the source root.",
        "A dependency library has not been built with `incan build --lib`.",
        "The symbol exists but is not exported publicly.",
    ],
    fixes: &[
        "Check the import path and public exports.",
        "For `pub::` imports, build the dependency library and verify `incan.toml` dependencies.",
    ],
    docs_url: Some("https://encero-systems.github.io/incan/language/reference/modules/"),
};

const TOOLING: DiagnosticCatalogEntry = DiagnosticCatalogEntry {
    code: "INCAN-C0001",
    title: "CLI or tooling error",
    severity: "error",
    phase: "tooling",
    summary: "The compiler command could not read inputs or complete a tooling operation.",
    explanation: "Tooling diagnostics are produced before or around the compiler pipeline, such as missing files, unreadable inputs, invalid command targets, or toolchain setup failures.",
    examples: &["incan check missing.incn"],
    common_causes: &[
        "The path does not exist.",
        "The file is too large or cannot be read.",
        "The local toolchain is missing a required target or dependency.",
    ],
    fixes: &[
        "Verify the command path and filesystem permissions.",
        "Run `incan tools doctor` for local toolchain problems.",
    ],
    docs_url: Some("https://encero-systems.github.io/incan/tooling/reference/cli_reference/"),
};

const UNKNOWN: DiagnosticCatalogEntry = DiagnosticCatalogEntry {
    code: "INCAN-U0001",
    title: "Unknown diagnostic code",
    severity: "error",
    phase: "unknown",
    summary: "The requested diagnostic code is not in this compiler's catalog.",
    explanation: "Diagnostic codes are versioned with the compiler. A code may be misspelled, from a newer compiler, or not yet assigned to a catalog entry.",
    examples: &["incan explain INCAN-NOPE"],
    common_causes: &[
        "Typo in the diagnostic code.",
        "Using docs from a different compiler version.",
    ],
    fixes: &[
        "Check the code printed by `incan check --format json`.",
        "Upgrade the compiler if the code comes from newer documentation.",
    ],
    docs_url: Some("https://encero-systems.github.io/incan/tooling/reference/cli_reference/"),
};

const CATALOG: &[DiagnosticCatalogEntry] = &[PARSER_SYNTAX, TYPECHECK, IMPORT, TOOLING, UNKNOWN];

/// Look up a public diagnostic explanation entry.
pub fn explain(code: &str) -> Option<&'static DiagnosticCatalogEntry> {
    CATALOG.iter().find(|entry| entry.code.eq_ignore_ascii_case(code))
}

/// Return every public catalog entry in deterministic order.
pub fn catalog_entries() -> &'static [DiagnosticCatalogEntry] {
    CATALOG
}

/// Select the stable public code for a compiler diagnostic.
pub fn code_for_error(error: &CompileError, phase: DiagnosticPhase) -> &'static str {
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
pub fn phase_for_typecheck_span(program: &Program, span: Span) -> DiagnosticPhase {
    if program
        .declarations
        .iter()
        .any(|declaration| matches!(declaration.node, Declaration::Import(_)) && spans_overlap(span, declaration.span))
    {
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
        notes: error.notes.clone(),
        hints: error.hints.clone(),
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
fn position_for_offset(source: &str, offset: usize) -> DiagnosticPosition {
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
mod tests {
    use crate::ast::{ImportDecl, ImportKind, Spanned, Visibility};

    use super::*;

    #[test]
    fn typecheck_phase_uses_import_span_for_import_diagnostics() {
        let program = Program {
            declarations: vec![
                Spanned::new(
                    Declaration::Import(ImportDecl {
                        visibility: Visibility::Private,
                        kind: ImportKind::PubLibrary {
                            library: "missing".to_string(),
                        },
                        alias: None,
                    }),
                    Span::new(4, 24),
                ),
                Spanned::new(Declaration::Docstring("body".to_string()), Span::new(40, 46)),
            ],
            ..Program::default()
        };

        assert_eq!(
            phase_for_typecheck_span(&program, Span::new(8, 12)),
            DiagnosticPhase::Import
        );
        assert_eq!(
            phase_for_typecheck_span(&program, Span::new(42, 44)),
            DiagnosticPhase::Typecheck
        );
    }

    #[test]
    fn stable_diagnostic_preserves_structured_facts_and_related_spans() {
        let error = CompileError::type_error("type mismatch".to_string(), Span::new(8, 13))
            .with_expected_actual("int", "str")
            .with_related_span(Span::new(0, 7), "Function parameter declared here");
        let diagnostic = stable_diagnostic("main.incn", "takes_x\n\"text\"\n", &error, DiagnosticPhase::Typecheck);

        assert_eq!(diagnostic.origin, DiagnosticOrigin::Typechecker);
        assert_eq!(diagnostic.expected.as_deref(), Some("int"));
        assert_eq!(diagnostic.actual.as_deref(), Some("str"));
        assert_eq!(diagnostic.related_spans.len(), 1);
        assert_eq!(diagnostic.related_spans[0].label, "Function parameter declared here");
        assert_eq!(diagnostic.related_spans[0].span.start.offset, 0);
    }
}
