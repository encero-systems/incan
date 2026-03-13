/// Parser core types and entrypoint.
///
/// This chunk defines the [`Parser`] type and its top-level `parse()` entrypoint.
/// It also contains a few small internal helper types shared across the other parser chunks.
///
/// ## Notes
/// - This file is `include!`'d into `crate::parser` to keep all parser methods in a
///   single module while avoiding a single “god file”.
type FieldsAndMethods = (Vec<Spanned<FieldDecl>>, Vec<Spanned<MethodDecl>>);

/// Result of parsing `[...]` postfix syntax: either a single index or a slice.
enum IndexOrSlice {
    Index(Spanned<Expr>),
    Slice(SliceExpr),
}

/// Parser state.
///
/// ## Notes
/// - The parser is intentionally single-pass and recovers from errors where possible by
///   synchronizing at statement/declaration boundaries.
/// - Most parsing helpers are implemented on `Parser` but split across multiple files.
pub struct Parser<'a> {
    tokens: &'a [Token],
    pos: usize,
    errors: Vec<CompileError>,
    /// Non-fatal warnings accumulated during parsing (e.g. style nudges that don't block compilation).
    warnings: Vec<CompileError>,
    active_soft_keywords: std::collections::HashSet<KeywordId>,
    module_path: Option<String>,
}

impl<'a> Parser<'a> {
    /// Create a new parser for a token stream.
    ///
    /// ## Parameters
    /// - `tokens`: Token stream produced by `incan_syntax::lexer`.
    pub fn new(tokens: &'a [Token]) -> Self {
        Self::new_with_module_path(tokens, None)
    }

    /// Create a new parser for a token stream with optional module path context.
    pub fn new_with_module_path(tokens: &'a [Token], module_path: Option<String>) -> Self {
        Self {
            tokens,
            pos: 0,
            errors: Vec::new(),
            warnings: Vec::new(),
            active_soft_keywords: std::collections::HashSet::new(),
            module_path,
        }
    }

    /// Parse the entire token stream into a [`Program`].
    ///
    /// ## Errors
    /// Returns a list of [`CompileError`]s if parsing fails. The parser attempts to recover and continue after an error
    /// to report multiple issues in one pass.
    pub fn parse(mut self) -> Result<Program, Vec<CompileError>> {
        let mut declarations = Vec::new();
        let mut rust_module_path: Option<Spanned<String>> = None;
        let mut seen_non_doc_decl = false;

        // Skip leading newlines
        self.skip_newlines();
        // Stray top-level DEDENT can appear after error recovery (e.g. unexpected indentation).
        // Ignore it at the module level to avoid cascaded errors.
        self.skip_dedents();

        while !self.is_at_end() {
            // ---- Context: `rust.module("...")` directive (RFC 023) ----
            if self.check_keyword(KeywordId::Rust)
                && self.peek_next().kind == TokenKind::Punctuation(PunctuationId::Dot)
            {
                match self.rust_module_directive() {
                    Ok(directive) => {
                        if seen_non_doc_decl {
                            self.errors.push(errors::rust_module_not_at_top(directive.span));
                        }
                        if rust_module_path.is_some() {
                            self.errors.push(errors::duplicate_rust_module(directive.span));
                        } else {
                            rust_module_path = Some(directive);
                        }
                    }
                    Err(e) => {
                        self.errors.push(e);
                        self.synchronize();
                    }
                }
                self.skip_newlines();
                self.skip_dedents();
                continue;
            }

            // ---- Context: normal declarations ----
            match self.declaration() {
                Ok(decl) => {
                    self.activate_soft_keywords_for_declaration(&decl.node);
                    if !matches!(decl.node, Declaration::Docstring(_)) {
                        seen_non_doc_decl = true;
                    }
                    declarations.push(decl)
                }
                Err(e) => {
                    self.errors.push(e);
                    self.synchronize();
                }
            }
            self.skip_newlines();
            // Same rationale as above: at the module level we should not see DEDENT tokens,
            // but the lexer may emit them and recovery may leave us positioned on them.
            self.skip_dedents();
        }

        if self.errors.is_empty() {
            Ok(Program {
                declarations,
                rust_module_path,
                warnings: self.warnings,
            })
        } else {
            // Fold non-fatal warnings into the error list so callers don't silently lose them when parsing fails.
            // Warnings retain their `ErrorKind::Warning` kind so callers can still distinguish them from errors if needed.
            self.errors.append(&mut self.warnings);
            Err(self.errors)
        }
    }

    /// Parse a `rust.module("path::to::module")` directive.
    ///
    /// Expects the current token to be `Keyword(Rust)`. Consumes `rust . module ( "..." )`.
    fn rust_module_directive(&mut self) -> Result<Spanned<String>, CompileError> {
        let start = self.current_span().start;

        // Consume `rust`
        self.expect_keyword(KeywordId::Rust, "Expected 'rust'")?;

        // Consume `.`
        self.expect_punct(PunctuationId::Dot, "Expected '.' after 'rust'")?;

        // Consume `module` (an identifier, not a keyword)
        let name = self.identifier_spanned()?;
        if name.node != "module" {
            return Err(errors::expected_token_message(
                "Expected 'module' after 'rust.'",
                &name.node,
                name.span,
            ));
        }

        // Consume `(` string_literal `)`
        self.expect_punct(PunctuationId::LParen, "Expected '(' after 'rust.module'")?;
        let path = self.string_literal()?;
        self.expect_punct(PunctuationId::RParen, "Expected ')' after rust.module path")?;

        let end = self.tokens[self.pos.saturating_sub(1)].span.end;
        Ok(Spanned::new(path, Span::new(start, end)))
    }

    /// Whether the parser is currently parsing `src/lib.incn`.
    fn is_library_entrypoint_module(&self) -> bool {
        let Some(module_path) = self.module_path.as_deref() else {
            return false;
        };

        let path = std::path::Path::new(module_path);
        let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
            return false;
        };
        if file_name != "lib.incn" {
            return false;
        }

        path.parent()
            .and_then(std::path::Path::file_name)
            .and_then(|name| name.to_str())
            == Some("src")
    }

    /// Activate soft keywords introduced by stdlib imports in this declaration.
    fn activate_soft_keywords_for_declaration(&mut self, decl: &Declaration) {
        let import_path = match decl {
            Declaration::Import(import) => match &import.kind {
                ImportKind::Module(path) => Some(path),
                ImportKind::From { module, .. } => Some(module),
                _ => None,
            },
            _ => None,
        };

        let Some(path) = import_path else {
            return;
        };

        for kw in incan_core::lang::stdlib::soft_keywords_for_import(&path.segments) {
            self.active_soft_keywords.insert(kw);
        }
    }
}
