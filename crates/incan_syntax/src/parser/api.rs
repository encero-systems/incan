use std::collections::HashMap;

/// Imported-library vocab registrations keyed by dependency key (`pub::name`).
pub type ImportedLibraryVocab = HashMap<String, Vec<incan_vocab::KeywordRegistration>>;

/// Imported-library DSL surfaces keyed by dependency key (`pub::name`).
pub type ImportedLibraryDslSurfaces = HashMap<String, Vec<incan_vocab::DslSurface>>;

/// Parse a token stream into an AST [`Program`].
///
/// This is the main public entrypoint for parsing.
///
/// ## Parameters
/// - `tokens`: Token stream produced by `incan_syntax::lexer`.
///
/// ## Errors
/// Returns `Err(Vec<CompileError>)` if parsing fails.
#[tracing::instrument(skip_all, fields(token_count = tokens.len()))]
pub fn parse(tokens: &[Token]) -> Result<Program, Vec<CompileError>> {
    parse_with_module_path(tokens, None)
}

/// Parse a token stream into an AST [`Program`] with optional module-path context.
///
/// The `module_path` is used for context-sensitive declaration diagnostics (for example,
/// `pub from ... import ...` is only valid in modules under `src/`).
#[tracing::instrument(skip_all, fields(token_count = tokens.len(), has_module_path = module_path.is_some()))]
pub fn parse_with_module_path(tokens: &[Token], module_path: Option<&str>) -> Result<Program, Vec<CompileError>> {
    parse_with_context(tokens, module_path, None)
}

/// Parse a token stream into an AST [`Program`] with full contextual information.
///
/// `library_imported_vocab` maps dependency keys (from `pub::key`) to the full keyword registrations serialized in
/// dependency `.incnlib` manifests.
///
/// This enables consumer-side parser activation for library-defined vocabulary without reparsing producer sources.
#[tracing::instrument(skip_all, fields(token_count = tokens.len(), has_module_path = module_path.is_some(), has_library_keywords = library_imported_vocab.is_some()))]
pub fn parse_with_context(
    tokens: &[Token],
    module_path: Option<&str>,
    library_imported_vocab: Option<&ImportedLibraryVocab>,
) -> Result<Program, Vec<CompileError>> {
    parse_with_context_and_surfaces(tokens, module_path, library_imported_vocab, None)
}

/// Parse a token stream with keyword and scoped-surface vocab context.
#[tracing::instrument(skip_all, fields(token_count = tokens.len(), has_module_path = module_path.is_some(), has_library_keywords = library_imported_vocab.is_some(), has_library_surfaces = library_imported_dsl_surfaces.is_some()))]
pub fn parse_with_context_and_surfaces(
    tokens: &[Token],
    module_path: Option<&str>,
    library_imported_vocab: Option<&ImportedLibraryVocab>,
    library_imported_dsl_surfaces: Option<&ImportedLibraryDslSurfaces>,
) -> Result<Program, Vec<CompileError>> {
    Parser::new_with_context(
        tokens,
        module_path.map(str::to_owned),
        library_imported_vocab,
        library_imported_dsl_surfaces,
    )
    .parse()
}

/// Parse a token stream with full contextual information plus the original source text.
///
/// This is the only public entrypoint that enables RFC 081 (`#1023`) descriptor-gated embedded fragments: the
/// parser needs `source` to slice a claimed submode fragment's raw byte range directly out of the original file
/// rather than reinterpreting whatever the ordinary lexer already did to that range (see
/// [`crate::parser::Parser::new_with_source`] for why). Every other `parse*` entrypoint above is unaffected and
/// keeps parsing embedded-fragment-eligible vocab-block bodies as an ordinary RFC 040/045 statement list, since
/// they never supply `source`.
///
/// ## Parameters
/// - `tokens`: Token stream produced by `incan_syntax::lexer::lex(source)`.
/// - `source`: The exact source string that `tokens` was lexed from.
///
/// ## Errors
/// Returns `Err(Vec<CompileError>)` if parsing fails.
#[tracing::instrument(skip_all, fields(token_count = tokens.len(), has_module_path = module_path.is_some(), has_library_keywords = library_imported_vocab.is_some(), has_library_surfaces = library_imported_dsl_surfaces.is_some()))]
pub fn parse_with_source(
    tokens: &[Token],
    module_path: Option<&str>,
    library_imported_vocab: Option<&ImportedLibraryVocab>,
    library_imported_dsl_surfaces: Option<&ImportedLibraryDslSurfaces>,
    source: &str,
) -> Result<Program, Vec<CompileError>> {
    Parser::new_with_source(
        tokens,
        module_path.map(str::to_owned),
        library_imported_vocab,
        library_imported_dsl_surfaces,
        source,
    )
    .parse()
}

/// Parse a token stream produced by [`crate::lexer::lex_tolerant`], reconciling its lex errors against RFC 081
/// (`#1023`) embedded fragments the parse actually claims.
///
/// This is the production entrypoint for source the ordinary strict [`crate::lexer::lex`] could not tokenize
/// outright -- for example a file whose embedded-fragment content contains bytes (`;`, `` ` ``, `$`, and similar)
/// that are not valid ordinary-Incan token starts. `lex_errors` should be the tolerant lexer's own collected
/// errors for the same `tokens`; a lex error whose span falls inside a fragment this parse successfully claims is
/// dropped as expected noise (that byte range is re-tokenized independently by the fragment's own submode
/// grammar), while every other lex error still surfaces as a real diagnostic, exactly as if the strict lexer had
/// produced it directly.
///
/// ## Parameters
/// - `tokens`: Token stream produced by `incan_syntax::lexer::lex_tolerant(source)`.
/// - `source`: The exact source string that `tokens` was lexed from.
/// - `lex_errors`: The tolerant lexer's own collected errors for `tokens`.
///
/// ## Errors
/// Returns `Err(Vec<CompileError>)` if parsing fails, or if any tolerant-lex error survives reconciliation.
#[tracing::instrument(skip_all, fields(token_count = tokens.len(), lex_error_count = lex_errors.len(), has_module_path = module_path.is_some()))]
pub fn parse_with_source_and_lex_errors(
    tokens: &[Token],
    module_path: Option<&str>,
    library_imported_vocab: Option<&ImportedLibraryVocab>,
    library_imported_dsl_surfaces: Option<&ImportedLibraryDslSurfaces>,
    source: &str,
    lex_errors: Vec<CompileError>,
) -> Result<Program, Vec<CompileError>> {
    Parser::new_with_source(
        tokens,
        module_path.map(str::to_owned),
        library_imported_vocab,
        library_imported_dsl_surfaces,
        source,
    )
    .with_pending_lex_errors(lex_errors)
    .parse()
}
