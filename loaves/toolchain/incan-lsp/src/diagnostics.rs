//! Convert Incan compiler diagnostics to LSP diagnostics
//!
//! This module provides utilities for converting between:
//! - Byte offsets (used by the Incan compiler) and LSP Positions (line/character)
//! - Compiler errors and LSP Diagnostics
//!
//! ## Position/Offset Conversion
//!
//! All conversion functions translate UTF-8 byte offsets into the UTF-16 code-unit columns required by LSP.
//! Positions are 0-based (line 0, character 0 is the first).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use incan_semantics_core::{CanonicalSymbolId, SymbolOrigin};
use incan_syntax::diagnostics::DiagnosticRelatedDeclaration;
use serde::Deserialize;
use serde_json::Value;
use tower_lsp::lsp_types::{
    Diagnostic, DiagnosticRelatedInformation, DiagnosticSeverity, Location, NumberOrString, Position, Range, Url,
};

use incan_frontend::diagnostics::{CompileError, DiagnosticPhase, ErrorKind, stable_diagnostic};

/// Source text and URI resolved for one canonical declaration origin.
#[derive(Debug, Clone)]
pub struct RelatedDeclarationSource {
    pub uri: Url,
    pub source: String,
}

/// Source-aware projection map for canonical declaration origins visible to one LSP typecheck.
pub type RelatedDeclarationSources = HashMap<SymbolOrigin, RelatedDeclarationSource>;

// ============================================================================
// Position/Offset Conversion Utilities
// ============================================================================
// These are the single authoritative implementations for converting between
// byte offsets and LSP positions. All LSP code should use these.

/// Convert a byte offset to LSP Position (0-based line and character).
///
/// Counts UTF-16 code units, as required by the LSP default position encoding. If the offset is beyond the end of the
/// source, returns the position at the end of the last line.
pub fn offset_to_position(source: &str, offset: usize) -> Position {
    let offset = offset.min(source.len());
    let mut line = 0u32;
    let mut col = 0u32;

    for (i, c) in source.char_indices() {
        if i >= offset {
            break;
        }
        if c == '\n' {
            line += 1;
            col = 0;
        } else {
            col += c.len_utf16() as u32;
        }
    }

    Position::new(line, col)
}

/// Convert an LSP Position (0-based line and character) to a byte offset.
///
/// Returns `None` if the position is beyond the end of the source. Counts UTF-16 code units, as required by the LSP
/// default position encoding. A position beyond a line's end is clamped to that line's terminating newline (or end of
/// file), matching [`offset_to_position`]'s bounded behavior.
pub fn position_to_offset(source: &str, position: Position) -> Option<usize> {
    let mut line = 0u32;
    let mut col = 0u32;

    for (i, c) in source.char_indices() {
        if line == position.line {
            if col >= position.character {
                return Some(i);
            }
            if c == '\n' {
                return Some(i);
            }
            let next_col = col + c.len_utf16() as u32;
            if position.character < next_col {
                // A byte offset cannot point between a surrogate pair. Clamp such a malformed LSP position to the
                // beginning of the scalar value instead of manufacturing a non-character-boundary offset.
                return Some(i);
            }
            col = next_col;
        } else if c == '\n' {
            line += 1;
            col = 0;
        }
    }

    if line == position.line {
        Some(source.len())
    } else {
        None
    }
}

/// Convert a span (start, end byte offsets) to an LSP Range.
pub fn span_to_range(source: &str, start: usize, end: usize) -> Range {
    let start_pos = offset_to_position(source, start);
    let end_pos = offset_to_position(source, end.max(start + 1));
    Range::new(start_pos, end_pos)
}

/// Convert ErrorKind to LSP DiagnosticSeverity
fn error_kind_to_severity(kind: ErrorKind) -> DiagnosticSeverity {
    match kind {
        ErrorKind::Error | ErrorKind::Syntax | ErrorKind::Type => DiagnosticSeverity::ERROR,
        ErrorKind::Warning => DiagnosticSeverity::WARNING,
        ErrorKind::Lint => DiagnosticSeverity::HINT,
    }
}

/// Convert a CompileError to LSP Diagnostic
pub fn compile_error_to_diagnostic(error: &CompileError, source: &str, uri: &Url) -> Diagnostic {
    compile_error_to_diagnostic_with_phase(error, source, uri, DiagnosticPhase::Unknown)
}

/// Convert a CompileError with known compiler phase to LSP Diagnostic.
pub fn compile_error_to_diagnostic_with_phase(
    error: &CompileError,
    source: &str,
    uri: &Url,
    phase: DiagnosticPhase,
) -> Diagnostic {
    compile_error_to_diagnostic_with_phase_and_sources(error, source, uri, phase, &HashMap::new())
}

/// Convert a compiler error while resolving canonical related declarations only through their actual source origin.
pub fn compile_error_to_diagnostic_with_phase_and_sources(
    error: &CompileError,
    source: &str,
    uri: &Url,
    phase: DiagnosticPhase,
    related_sources: &RelatedDeclarationSources,
) -> Diagnostic {
    let stable = stable_diagnostic(uri.as_str(), source, error, phase);
    let range = span_to_range(source, stable.primary_span.start.offset, stable.primary_span.end.offset);
    let severity = if error.stable_code() == Some("INCAN-T0134") {
        DiagnosticSeverity::INFORMATION
    } else {
        error_kind_to_severity(error.kind)
    };

    let related_spans = stable
        .related_spans
        .iter()
        .map(|related| DiagnosticRelatedInformation {
            location: Location {
                uri: uri.clone(),
                range: span_to_range(source, related.span.start.offset, related.span.end.offset),
            },
            message: related.label.clone(),
        })
        .collect();
    present_stable_diagnostic(
        StablePresentation {
            code: stable.code,
            severity,
            message: &stable.message,
            primary: Location {
                uri: uri.clone(),
                range,
            },
            notes: &stable.notes,
            hints: &stable.hints,
            related_spans,
            related_declarations: &stable.related_declarations,
            data: serde_json::to_value(&stable).ok(),
        },
        related_sources,
    )
}

/// Read only the stable report fields the existing presenter consumes.
#[derive(Deserialize)]
struct OverlayPresentation {
    code: String,
    message: String,
    severity: OverlaySeverity,
    primary_span: OverlaySpan,
    notes: Vec<String>,
    hints: Vec<String>,
    #[serde(default)]
    related_spans: Vec<OverlayRelatedSpan>,
    #[serde(default)]
    related_declarations: Vec<OverlayRelatedDeclaration>,
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum OverlaySeverity {
    Error,
    Warning,
    Hint,
    Information,
}

#[derive(Deserialize)]
struct OverlaySpan {
    file: PathBuf,
    start: OverlayPosition,
    end: OverlayPosition,
}

#[derive(Deserialize)]
struct OverlayPosition {
    offset: usize,
}

#[derive(Deserialize)]
struct OverlayRelatedSpan {
    span: OverlaySpan,
    label: String,
}

#[derive(Deserialize)]
struct OverlayRelatedDeclaration {
    identity: CanonicalSymbolId,
    label: String,
}

impl OverlaySpan {
    /// Present an offset only against the source bound by this editor request.
    fn location(&self, path: &Path, uri: &Url, source: &str) -> Result<Location, String> {
        if self.file.as_path() != path {
            return Err("stable overlay diagnostic names another document".into());
        }
        let start = self.start.offset;
        let end = self.end.offset;
        if start > end || !source.is_char_boundary(start) || !source.is_char_boundary(end) {
            return Err(format!(
                "overlay diagnostic has invalid offsets for {}",
                self.file.display()
            ));
        }
        Ok(Location {
            uri: uri.clone(),
            range: span_to_range(source, start, end),
        })
    }
}

/// Admit the served compiler record and present it with the matching editor source. See row 14.1's host intake gap.
pub(crate) fn stable_overlay_diagnostic(
    data: &Value,
    path: &Path,
    uri: &Url,
    source: &str,
) -> Result<Diagnostic, String> {
    let checked = OverlayPresentation::deserialize(data)
        .map_err(|error| format!("invalid stable overlay diagnostic: {error}"))?;
    let primary = checked.primary_span.location(path, uri, source)?;
    let related_spans = checked
        .related_spans
        .into_iter()
        .map(|related| {
            Ok(DiagnosticRelatedInformation {
                location: related.span.location(path, uri, source)?,
                message: related.label,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    let related_declarations = checked
        .related_declarations
        .into_iter()
        .map(|related| DiagnosticRelatedDeclaration {
            identity: related.identity,
            label: related.label,
        })
        .collect::<Vec<_>>();

    let mut data = data.clone();
    data["primary_span"]["file"] = Value::String(uri.to_string());
    if let Some(spans) = data.get_mut("related_spans").and_then(Value::as_array_mut) {
        for entry in spans {
            entry["span"]["file"] = Value::String(uri.to_string());
        }
    }
    let severity = match checked.severity {
        OverlaySeverity::Error => DiagnosticSeverity::ERROR,
        OverlaySeverity::Warning => DiagnosticSeverity::WARNING,
        OverlaySeverity::Hint => DiagnosticSeverity::HINT,
        OverlaySeverity::Information => DiagnosticSeverity::INFORMATION,
    };
    // The served record does not name a checked source for a declaration's canonical origin.
    Ok(present_stable_diagnostic(
        StablePresentation {
            code: &checked.code,
            severity,
            message: &checked.message,
            primary,
            notes: &checked.notes,
            hints: &checked.hints,
            related_spans,
            related_declarations: &related_declarations,
            data: Some(data),
        },
        &RelatedDeclarationSources::new(),
    ))
}

struct StablePresentation<'a> {
    code: &'a str,
    severity: DiagnosticSeverity,
    message: &'a str,
    primary: Location,
    notes: &'a [String],
    hints: &'a [String],
    related_spans: Vec<DiagnosticRelatedInformation>,
    related_declarations: &'a [DiagnosticRelatedDeclaration],
    data: Option<Value>,
}

/// Keep compiler notes, hints and checked locations in the same order for both inputs.
fn present_stable_diagnostic(
    stable: StablePresentation<'_>,
    related_sources: &RelatedDeclarationSources,
) -> Diagnostic {
    let mut message = stable.message.to_owned();
    let mut related_information = Vec::new();

    for (kind, details) in [("note", stable.notes), ("hint", stable.hints)] {
        for detail in details {
            let detail = format!("{kind}: {detail}");
            message.push_str("\n\n");
            message.push_str(&detail);
            related_information.push(DiagnosticRelatedInformation {
                location: stable.primary.clone(),
                message: detail,
            });
        }
    }
    related_information.extend(stable.related_spans);

    for related in stable.related_declarations {
        if let Some(declaration_source) = related_sources.get(&related.identity.origin) {
            related_information.push(DiagnosticRelatedInformation {
                location: Location {
                    uri: declaration_source.uri.clone(),
                    range: span_to_range(
                        &declaration_source.source,
                        related.identity.declaration_span.start,
                        related.identity.declaration_span.end,
                    ),
                },
                message: related.label.clone(),
            });
        } else {
            message.push_str("\n\nnote: ");
            message.push_str(&related.label);
            message.push_str(": ");
            message.push_str(&related.identity.render_compact());
        }
    }

    Diagnostic {
        range: stable.primary.range,
        severity: Some(stable.severity),
        code: Some(NumberOrString::String(stable.code.to_string())),
        code_description: None,
        source: Some("incan".to_string()),
        message,
        related_information: if related_information.is_empty() {
            None
        } else {
            Some(related_information)
        },
        tags: None,
        data: stable.data,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_offset_to_position() {
        let source = "line 1\nline 2\nline 3";

        let pos = offset_to_position(source, 0);
        assert_eq!(pos.line, 0);
        assert_eq!(pos.character, 0);

        let pos = offset_to_position(source, 7); // Start of "line 2"
        assert_eq!(pos.line, 1);
        assert_eq!(pos.character, 0);

        let pos = offset_to_position(source, 10); // "e 2"
        assert_eq!(pos.line, 1);
        assert_eq!(pos.character, 3);
    }

    #[test]
    fn test_position_to_offset() {
        let source = "line 1\nline 2\nline 3";

        // Start of file
        let offset = position_to_offset(source, Position::new(0, 0));
        assert_eq!(offset, Some(0));

        // Start of line 2
        let offset = position_to_offset(source, Position::new(1, 0));
        assert_eq!(offset, Some(7));

        // Middle of line 2 ("e 2")
        let offset = position_to_offset(source, Position::new(1, 3));
        assert_eq!(offset, Some(10));

        // End of file
        let offset = position_to_offset(source, Position::new(2, 6));
        assert_eq!(offset, Some(20));
    }

    #[test]
    fn test_roundtrip_offset_position() {
        let source = "def foo():\n    pass\n";

        // Test round-trip for various offsets
        for offset in [0, 5, 10, 15, 19] {
            let pos = offset_to_position(source, offset);
            let back = position_to_offset(source, pos);
            assert_eq!(back, Some(offset), "roundtrip failed for offset {}", offset);
        }
    }

    #[test]
    fn position_conversion_uses_utf16_code_units() {
        let source = "😀x\n";

        assert_eq!(offset_to_position(source, "😀".len()), Position::new(0, 2));
        assert_eq!(offset_to_position(source, "😀x".len()), Position::new(0, 3));
        assert_eq!(position_to_offset(source, Position::new(0, 2)), Some("😀".len()));
        assert_eq!(position_to_offset(source, Position::new(0, 3)), Some("😀x".len()));
    }

    /// A compile-time trust stop is informational in the editor even though it still prevents a checked expansion.
    #[test]
    fn missing_compile_time_trust_is_information() -> Result<(), Box<dyn std::error::Error>> {
        let uri = Url::parse("file:///workspace/main.incn")?;
        let error = CompileError::new(
            "vocabulary block is not yet checked".to_string(),
            incan_frontend::ast::Span::new(0, 5),
        )
        .with_stable_code("INCAN-T0134");

        let diagnostic = compile_error_to_diagnostic(&error, "query\n", &uri);

        assert_eq!(diagnostic.severity, Some(DiagnosticSeverity::INFORMATION));
        assert_eq!(diagnostic.code, Some(NumberOrString::String("INCAN-T0134".to_string())));
        Ok(())
    }

    #[test]
    fn lsp_diagnostic_projects_the_shared_compiler_fact() -> Result<(), Box<dyn std::error::Error>> {
        let source = "first\nsecond\n";
        let uri = Url::parse("file:///workspace/main.incn")?;
        let error = CompileError::type_error("duplicate argument".to_string(), incan_frontend::ast::Span::new(6, 12))
            .with_expected_actual("int", "str")
            .with_related_span(incan_frontend::ast::Span::new(0, 5), "First argument named 'value'");

        let diagnostic = compile_error_to_diagnostic_with_phase(&error, source, &uri, DiagnosticPhase::Typecheck);
        let related = diagnostic
            .related_information
            .as_ref()
            .ok_or("expected related information")?;
        assert!(
            related
                .iter()
                .any(|item| item.message == "First argument named 'value'")
        );
        let data = diagnostic.data.ok_or("expected compiler fact data")?;
        assert_eq!(data["origin"], serde_json::json!("typechecker"));
        assert_eq!(data["expected"], serde_json::json!("int"));
        assert_eq!(data["actual"], serde_json::json!("str"));
        Ok(())
    }

    #[test]
    fn lsp_related_declaration_uses_provider_uri_and_utf16_range() -> Result<(), Box<dyn std::error::Error>> {
        use incan_semantics_core::{CanonicalSymbolId, HirSourceSpan, SemanticSourceTargetKind, SymbolOrigin};

        let consumer_uri = Url::parse("file:///workspace/consumer.incn")?;
        let provider_uri = Url::parse("file:///workspace/provider.incn")?;
        let provider_source = "😀 def parse(value: int) -> int:\n  return value\n";
        let declaration_start = provider_source.find("def parse").ok_or("missing declaration")?;
        let declaration_end = declaration_start + "def parse".len();
        let declaration = CanonicalSymbolId::module_declaration(
            vec!["provider".to_string()],
            "parse",
            SemanticSourceTargetKind::Function,
            HirSourceSpan::new(declaration_start, declaration_end),
        );
        let error = CompileError::type_error(
            "alias argument mismatch".to_string(),
            incan_frontend::ast::Span::new(0, 5),
        )
        .with_related_declaration(declaration, "declaration of `parse`");
        let mut sources = RelatedDeclarationSources::new();
        sources.insert(
            SymbolOrigin::Module(vec!["provider".to_string()]),
            RelatedDeclarationSource {
                uri: provider_uri.clone(),
                source: provider_source.to_string(),
            },
        );

        let diagnostic = compile_error_to_diagnostic_with_phase_and_sources(
            &error,
            "alias()\n",
            &consumer_uri,
            DiagnosticPhase::Typecheck,
            &sources,
        );
        let related = diagnostic.related_information.ok_or("missing related location")?;
        let [related] = related.as_slice() else {
            return Err(format!("expected one related location, got {related:?}").into());
        };
        assert_eq!(related.location.uri, provider_uri);
        assert_eq!(related.location.range.start, Position::new(0, 3));
        Ok(())
    }

    #[test]
    fn lsp_unmapped_declaration_never_fabricates_a_consumer_location() -> Result<(), Box<dyn std::error::Error>> {
        use incan_semantics_core::{CanonicalSymbolId, HirSourceSpan, SemanticSourceTargetKind};

        let consumer_uri = Url::parse("file:///workspace/consumer.incn")?;
        let declaration = CanonicalSymbolId::module_declaration(
            vec!["unloaded".to_string()],
            "parse",
            SemanticSourceTargetKind::Function,
            HirSourceSpan::new(50, 80),
        );
        let error = CompileError::type_error(
            "alias argument mismatch".to_string(),
            incan_frontend::ast::Span::new(0, 5),
        )
        .with_related_declaration(declaration, "declaration of `parse`");
        let diagnostic = compile_error_to_diagnostic_with_phase_and_sources(
            &error,
            "alias()\n",
            &consumer_uri,
            DiagnosticPhase::Typecheck,
            &RelatedDeclarationSources::new(),
        );

        assert!(diagnostic.related_information.is_none());
        assert!(diagnostic.message.contains("function:unloaded::parse@50..80"));
        assert_eq!(
            diagnostic.data.ok_or("missing stable diagnostic")?["related_declarations"][0]["identity"]["declaration_span"]
                ["start"],
            serde_json::json!(50)
        );
        Ok(())
    }
}
