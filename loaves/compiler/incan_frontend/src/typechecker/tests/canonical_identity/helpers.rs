//! Fixture helpers every canonical-identity module uses: parsing and checking one conformance program, the
//! expected-failure path, and the span and identity readers.

use super::*;

/// Parse one test program and preserve fixture failures as ordinary test errors.
pub(super) fn parse(source: &str, context: &str) -> Result<Program, String> {
    let tokens = lexer::lex(source).map_err(|errors| format!("{context} lex failed: {errors:?}"))?;
    parser::parse(&tokens).map_err(|errors| format!("{context} parse failed: {errors:?}"))
}

/// Check one standalone program and return the checker for identity inspection.
pub(super) fn check(source: &str, context: &str) -> Result<TypeChecker, String> {
    let program = parse(source, context)?;
    let mut checker = TypeChecker::new();
    checker.set_current_module_path(Some(vec!["conformance".to_string()]));
    checker
        .check_program(&program)
        .map_err(|errors| format!("{context} should typecheck: {errors:?}"))?;
    Ok(checker)
}

/// Run a program that is expected to fail and return its structured diagnostics without a panic-based extractor.
pub(super) fn check_errors(
    checker: &mut TypeChecker,
    program: &Program,
    context: &str,
) -> Result<Vec<CompileError>, String> {
    match checker.check_program(program) {
        Ok(()) => Err(format!("{context}: program unexpectedly typechecked")),
        Err(errors) => Ok(errors),
    }
}

/// Return the span of the `occurrence`-th appearance (0-based) of `needle` in `source`.
pub(super) fn nth_span(source: &str, needle: &str, occurrence: usize) -> Result<Span, String> {
    source
        .match_indices(needle)
        .nth(occurrence)
        .map(|(start, matched)| Span::new(start, start + matched.len()))
        .ok_or_else(|| format!("occurrence {occurrence} of `{needle}` not found"))
}

/// Return the recorded reference identity at `span`, or an error naming the missing case.
pub(super) fn identity_at(checker: &TypeChecker, span: Span, context: &str) -> Result<CanonicalSymbolId, String> {
    checker.type_info().resolved_identity(span).cloned().ok_or_else(|| {
        format!(
            "{context}: no resolved identity recorded at {}..{}",
            span.start, span.end
        )
    })
}

/// Return the canonical declaration selected for one source binding write.
pub(super) fn write_identity_at(
    checker: &TypeChecker,
    span: Span,
    name: &str,
    context: &str,
) -> Result<CanonicalSymbolId, String> {
    checker
        .type_info()
        .resolved_write_identity(span, name)
        .cloned()
        .ok_or_else(|| {
            format!(
                "{context}: no resolved write identity recorded for `{name}` at {}..{}",
                span.start, span.end
            )
        })
}
