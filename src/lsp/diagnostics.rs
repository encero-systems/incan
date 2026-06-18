//! Convert Incan compiler diagnostics to LSP diagnostics
//!
//! This module provides utilities for converting between:
//! - Byte offsets (used by the Incan compiler) and LSP Positions (line/character)
//! - Compiler errors and LSP Diagnostics
//!
//! ## Position/Offset Conversion
//!
//! All conversion functions handle UTF-8 correctly by counting characters,
//! not bytes. LSP positions are 0-based (line 0, character 0 is the first).

use tower_lsp::lsp_types::{
    Diagnostic, DiagnosticRelatedInformation, DiagnosticSeverity, Location, NumberOrString, Position, Range, Url,
};

use crate::frontend::diagnostics::{CompileError, DiagnosticPhase, ErrorKind, code_for_error};

// ============================================================================
// Position/Offset Conversion Utilities
// ============================================================================
// These are the single authoritative implementations for converting between
// byte offsets and LSP positions. All LSP code should use these.

/// Convert a byte offset to LSP Position (0-based line and character).
///
/// Handles UTF-8 correctly by iterating over characters, not bytes.
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
            col += 1;
        }
    }

    Position::new(line, col)
}

/// Convert an LSP Position (0-based line and character) to a byte offset.
///
/// Returns `None` if the position is beyond the end of the source.
/// Handles UTF-8 correctly by iterating over characters, not bytes.
pub fn position_to_offset(source: &str, position: Position) -> Option<usize> {
    let mut line = 0u32;
    let mut col = 0u32;
    let mut offset = 0usize;

    for (i, c) in source.char_indices() {
        if line == position.line && col == position.character {
            return Some(i);
        }
        if c == '\n' {
            if line == position.line {
                // Position is beyond line end - return end of line
                return Some(i);
            }
            line += 1;
            col = 0;
        } else {
            col += 1;
        }
        offset = i + c.len_utf8();
    }

    // Position at end of file
    if line == position.line && col == position.character {
        Some(offset)
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
    let range = span_to_range(source, error.span.start, error.span.end);
    let severity = error_kind_to_severity(error.kind);

    // Build the message with notes and hints
    let mut message = error.message.clone();

    // Add notes
    for note in &error.notes {
        message.push_str("\n\nnote: ");
        message.push_str(note);
    }

    // Add hints
    for hint in &error.hints {
        message.push_str("\n\nhint: ");
        message.push_str(hint);
    }

    // Create related information for notes/hints (shows in Problems panel)
    let mut related_information = Vec::new();

    for note in &error.notes {
        related_information.push(DiagnosticRelatedInformation {
            location: Location {
                uri: uri.clone(),
                range,
            },
            message: format!("note: {}", note),
        });
    }

    for hint in &error.hints {
        related_information.push(DiagnosticRelatedInformation {
            location: Location {
                uri: uri.clone(),
                range,
            },
            message: format!("hint: {}", hint),
        });
    }

    Diagnostic {
        range,
        severity: Some(severity),
        code: Some(NumberOrString::String(code_for_error(error, phase).to_string())),
        code_description: None,
        source: Some("incan".to_string()),
        message,
        related_information: if related_information.is_empty() {
            None
        } else {
            Some(related_information)
        },
        tags: None,
        data: None,
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
}
