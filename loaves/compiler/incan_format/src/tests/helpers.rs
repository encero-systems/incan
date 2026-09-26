//! Helpers that more than one formatter test module uses: parsing a source string to a `Program` and the
//! format-then-lex-and-parse round trip. A helper only one module needs stays private in that module.

use super::*;

pub(super) fn program_from_source(source: &str) -> Result<Program, FormatError> {
    let tokens = lexer::lex(source).map_err(|errs| {
        FormatError::SyntaxError(errs.iter().map(|e| e.message.clone()).collect::<Vec<_>>().join("\n"))
    })?;
    parser::parse(&tokens)
        .map_err(|errs| FormatError::SyntaxError(errs.iter().map(|e| e.message.clone()).collect::<Vec<_>>().join("\n")))
}

/// Formats `source` and checks the result lexes and parses (regression harness for formatter output validity).
pub(super) fn assert_format_round_trip_lex_parse(source: &str) -> Result<String, FormatError> {
    let formatted = format_source(source)?;
    let tokens = incan_syntax::lexer::lex(&formatted).map_err(|errs| {
        FormatError::SyntaxError(errs.iter().map(|e| e.message.clone()).collect::<Vec<_>>().join("\n"))
    })?;
    incan_syntax::parser::parse(&tokens).map_err(|errs| {
        FormatError::SyntaxError(errs.iter().map(|e| e.message.clone()).collect::<Vec<_>>().join("\n"))
    })?;
    Ok(formatted)
}
