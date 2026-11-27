//! Helpers that more than one parser test module uses: lexing and parsing a source string, the exact-span fixture
//! lookup, and the declaration-shape accessors. A helper only one module needs stays private in that module.

use super::*;

pub(super) fn parse_str(source: &str) -> Result<Program, Vec<CompileError>> {
    let tokens = lexer::lex(source).map_err(|_| vec![])?;
    parse(&tokens)
}

pub(super) fn parse_str_err(source: &str, context: &str) -> Vec<CompileError> {
    match parse_str(source) {
        Err(errs) => errs,
        Ok(_) => panic!("{context}"),
    }
}

/// Return the exact span of one fixture occurrence without introducing a panic path in fallible parser tests.
pub(super) fn require_source_span(source: &str, needle: &str, occurrence: usize) -> Result<Span, Vec<CompileError>> {
    source
        .match_indices(needle)
        .nth(occurrence)
        .map(|(start, matched)| Span::new(start, start + matched.len()))
        .ok_or_else(|| {
            vec![CompileError::new(
                format!("parser test internal error: occurrence {occurrence} of `{needle}` not found"),
                Span::default(),
            )]
        })
}

/// Test helper: surface a structured failure instead of panicking when a declaration is not a trait.
pub(super) fn require_trait_decl(decl: &Spanned<Declaration>) -> Result<&TraitDecl, Vec<CompileError>> {
    match &decl.node {
        Declaration::Trait(t) => Ok(t),
        _ => Err(vec![CompileError::new(
            "parser test internal error: expected trait declaration".to_string(),
            decl.span,
        )]),
    }
}

pub(super) fn require_newtype_decl(decl: &Spanned<Declaration>) -> Result<&NewtypeDecl, Vec<CompileError>> {
    match &decl.node {
        Declaration::Newtype(nt) => Ok(nt),
        _ => Err(vec![CompileError::new(
            "parser test internal error: expected newtype/rusttype declaration".to_string(),
            decl.span,
        )]),
    }
}

pub(super) fn require_model_decl(decl: &Spanned<Declaration>) -> Result<&ModelDecl, Vec<CompileError>> {
    match &decl.node {
        Declaration::Model(m) => Ok(m),
        _ => Err(vec![CompileError::new(
            "parser test internal error: expected model declaration".to_string(),
            decl.span,
        )]),
    }
}

pub(super) fn require_class_decl(decl: &Spanned<Declaration>) -> Result<&ClassDecl, Vec<CompileError>> {
    match &decl.node {
        Declaration::Class(c) => Ok(c),
        _ => Err(vec![CompileError::new(
            "parser test internal error: expected class declaration".to_string(),
            decl.span,
        )]),
    }
}

pub(super) fn require_function_decl(decl: &Spanned<Declaration>) -> Result<&FunctionDecl, Vec<CompileError>> {
    match &decl.node {
        Declaration::Function(f) => Ok(f),
        _ => Err(vec![CompileError::new(
            "parser test internal error: expected function declaration".to_string(),
            decl.span,
        )]),
    }
}

/// A source that parses, with the checks its program must pass.
pub(super) struct ParseCase {
    /// The row's name, reported when it fails.
    pub name: &'static str,
    pub source: &'static str,
    pub check: fn(&str, &Program) -> Result<(), Vec<CompileError>>,
}

/// A source the parser refuses, with the checks its errors must pass.
pub(super) struct RefusalCase {
    /// The row's name, reported when it fails.
    pub name: &'static str,
    pub source: &'static str,
    pub check: fn(&str, &[CompileError]),
}

fn messages(errors: &[CompileError]) -> Vec<&str> {
    errors.iter().map(|error| error.message.as_str()).collect()
}

/// Runs `row`, turning a returned error or a panic into a failure message.
fn run_row(row: impl FnOnce() -> Result<(), String>) -> Option<String> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(row)) {
        Ok(outcome) => outcome.err(),
        Err(payload) => Some(
            payload
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| payload.downcast_ref::<&str>().map(|text| text.to_string()))
                .unwrap_or_else(|| "panicked".to_string()),
        ),
    }
}

fn report(kind: &str, failures: Vec<String>) {
    assert!(
        failures.is_empty(),
        "{} {kind} row(s) failed:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// Parses every row's source and runs its checks; fails once, naming each failing row.
pub(super) fn run_parse_cases(cases: &[ParseCase]) {
    let failures = cases
        .iter()
        .filter_map(|case| {
            run_row(|| {
                let program =
                    parse_str(case.source).map_err(|errors| format!("parse failed: {:?}", messages(&errors)))?;
                (case.check)(case.source, &program).map_err(|errors| format!("check failed: {:?}", messages(&errors)))
            })
            .map(|problem| format!("{}: {problem}", case.name))
        })
        .collect();
    report("parse", failures);
}

/// Requires every row's source to be refused and runs its checks; fails once, naming each failing row.
pub(super) fn run_refusal_cases(cases: &[RefusalCase]) {
    let failures = cases
        .iter()
        .filter_map(|case| {
            run_row(|| match parse_str(case.source) {
                Ok(_) => Err("the source parsed".to_string()),
                Err(errors) => {
                    (case.check)(case.source, &errors);
                    Ok(())
                }
            })
            .map(|problem| format!("{}: {problem}", case.name))
        })
        .collect();
    report("refusal", failures);
}

pub(super) type VocabKeywords = std::collections::HashMap<String, Vec<incan_vocab::KeywordRegistration>>;
pub(super) type VocabSurfaces = std::collections::HashMap<String, Vec<incan_vocab::DslSurface>>;

/// A source parsed with the vocabulary registrations its row builds, and the checks its parse result must pass.
pub(super) struct VocabCase {
    /// The row's name, reported when it fails.
    pub name: &'static str,
    pub source: &'static str,
    /// Builds the imported keyword registrations and, when the row declares surfaces, the DSL surfaces.
    pub registrations: fn() -> (VocabKeywords, Option<VocabSurfaces>),
    /// Receives the source and the parse result, accepted or refused.
    pub check: fn(&str, Result<Program, Vec<CompileError>>) -> Result<(), Box<dyn std::error::Error>>,
}

/// Lexes each row's source, parses it with the row's registrations and runs its checks; fails once, naming each
/// failing row.
pub(super) fn run_vocab_cases(cases: &[VocabCase]) {
    let failures = cases
        .iter()
        .filter_map(|case| {
            run_row(|| {
                let tokens = lexer::lex(case.source).map_err(|errs| format!("lex errors: {errs:?}"))?;
                let (keywords, surfaces) = (case.registrations)();
                let parsed =
                    crate::parser::parse_with_context_and_surfaces(&tokens, None, Some(&keywords), surfaces.as_ref());
                (case.check)(case.source, parsed).map_err(|error| error.to_string())
            })
            .map(|problem| format!("{}: {problem}", case.name))
        })
        .collect();
    report("vocabulary", failures);
}
