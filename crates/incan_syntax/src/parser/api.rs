use std::collections::HashMap;

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
/// `pub from ... import ...` is only valid in `src/lib.incn`).
#[tracing::instrument(skip_all, fields(token_count = tokens.len(), has_module_path = module_path.is_some()))]
pub fn parse_with_module_path(tokens: &[Token], module_path: Option<&str>) -> Result<Program, Vec<CompileError>> {
    parse_with_context(tokens, module_path, None)
}

/// Parse a token stream into an AST [`Program`] with full contextual information.
///
/// `library_soft_keywords` maps dependency keys (from `pub::key`) to the list of soft keywords activated by importing
/// from that library.
#[tracing::instrument(skip_all, fields(token_count = tokens.len(), has_module_path = module_path.is_some(), has_library_keywords = library_soft_keywords.is_some()))]
pub fn parse_with_context(
    tokens: &[Token],
    module_path: Option<&str>,
    library_soft_keywords: Option<&HashMap<String, Vec<KeywordId>>>,
) -> Result<Program, Vec<CompileError>> {
    Parser::new_with_context(tokens, module_path.map(str::to_owned), library_soft_keywords).parse()
}
