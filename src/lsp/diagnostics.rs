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

use incan_semantics_core::SymbolOrigin;
use tower_lsp::lsp_types::{
    Diagnostic, DiagnosticRelatedInformation, DiagnosticSeverity, Location, NumberOrString, Position, Range, Url,
};

use crate::frontend::diagnostics::{CompileError, DiagnosticPhase, ErrorKind, stable_diagnostic};

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
/// Counts UTF-16 code units, as required by the LSP default position encoding.
/// If the offset is beyond the end of the source, returns the position
/// at the end of the last line.
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
/// Returns `None` if the position is beyond the end of the source.
/// Counts UTF-16 code units, as required by the LSP default position encoding. A position beyond a line's end is
/// clamped to that line's terminating newline (or end of file), matching [`offset_to_position`]'s bounded behavior.
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
    let severity = error_kind_to_severity(error.kind);

    // Build the message with notes and hints
    let mut message = stable.message.clone();

    // Add notes
    for note in &stable.notes {
        message.push_str("\n\nnote: ");
        message.push_str(note);
    }

    // Add hints
    for hint in &stable.hints {
        message.push_str("\n\nhint: ");
        message.push_str(hint);
    }

    // Create related information for notes/hints (shows in Problems panel)
    let mut related_information = Vec::new();

    for note in &stable.notes {
        related_information.push(DiagnosticRelatedInformation {
            location: Location {
                uri: uri.clone(),
                range,
            },
            message: format!("note: {}", note),
        });
    }

    for hint in &stable.hints {
        related_information.push(DiagnosticRelatedInformation {
            location: Location {
                uri: uri.clone(),
                range,
            },
            message: format!("hint: {}", hint),
        });
    }

    for related in &stable.related_spans {
        related_information.push(DiagnosticRelatedInformation {
            location: Location {
                uri: uri.clone(),
                range: span_to_range(source, related.span.start.offset, related.span.end.offset),
            },
            message: related.label.clone(),
        });
    }

    for related in &stable.related_declarations {
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
        range,
        severity: Some(severity),
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
        data: serde_json::to_value(&stable).ok(),
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

    #[test]
    fn lsp_diagnostic_projects_the_shared_compiler_fact() -> Result<(), Box<dyn std::error::Error>> {
        let source = "first\nsecond\n";
        let uri = Url::parse("file:///workspace/main.incn")?;
        let error = CompileError::type_error("duplicate argument".to_string(), crate::frontend::ast::Span::new(6, 12))
            .with_expected_actual("int", "str")
            .with_related_span(crate::frontend::ast::Span::new(0, 5), "First argument named 'value'");

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
            crate::frontend::ast::Span::new(0, 5),
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
            crate::frontend::ast::Span::new(0, 5),
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
