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
