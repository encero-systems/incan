/// Miscellaneous parser utilities.
///
/// This chunk contains small shared parsing helpers that don’t cleanly fit into
/// “decl”, “stmt”, “expr”, or “types” (e.g. identifier parsing and string literal handling).
impl<'a> Parser<'a> {
    // ========================================================================
    // Utilities
    // ========================================================================

    /// Parse an identifier.
    fn identifier(&mut self) -> Result<Ident, CompileError> {
        match &self.peek().kind {
            TokenKind::Ident(name) => {
                let name = name.clone();
                self.advance();
                Ok(name)
            }
            _ => Err(errors::expected_identifier(
                &format!("{:?}", self.peek().kind),
                self.current_span(),
            )),
        }
    }

    /// Parse an identifier and return it as a spanned token.
    fn identifier_spanned(&mut self) -> Result<Spanned<Ident>, CompileError> {
        match &self.peek().kind {
            TokenKind::Ident(name) => {
                let span = self.current_span();
                let name = name.clone();
                self.advance();
                Ok(Spanned::new(name, span))
            }
            _ => Err(errors::expected_identifier(
                &format!("{:?}", self.peek().kind),
                self.current_span(),
            )),
        }
    }

    /// Parse an identifier, allowing certain keywords in specific contexts (like enum variants).
    fn identifier_or_keyword(&mut self) -> Result<Ident, CompileError> {
        match &self.peek().kind {
            TokenKind::Ident(name) => {
                let name = name.clone();
                self.advance();
                Ok(name)
            }
            TokenKind::Keyword(KeywordId::None) => {
                // Allow "None" as an identifier in enum variant context
                self.advance();
                Ok("None".to_string())
            }
            _ => Err(errors::expected_identifier(
                &format!("{:?}", self.peek().kind),
                self.current_span(),
            )),
        }
    }

    /// Parse an identifier, allowing any keyword token (RFC 021 limited contexts).
    fn identifier_or_any_keyword(&mut self) -> Result<Ident, CompileError> {
        match &self.peek().kind {
            TokenKind::Ident(name) => {
                let name = name.clone();
                self.advance();
                Ok(name)
            }
            TokenKind::Keyword(kw) => {
                let name = incan_core::lang::keywords::as_str(*kw).to_string();
                self.advance();
                Ok(name)
            }
            _ => Err(errors::expected_identifier(
                &format!("{:?}", self.peek().kind),
                self.current_span(),
            )),
        }
    }

    /// Parse an identifier in import/decorator paths, allowing specific keyword segments (e.g. `std.async`, `rust.extern`).
    fn identifier_or_import_keyword(&mut self) -> Result<Ident, CompileError> {
        match &self.peek().kind {
            TokenKind::Ident(name) => {
                let name = name.clone();
                self.advance();
                Ok(name)
            }
            TokenKind::Keyword(KeywordId::Rust) => {
                self.advance();
                Ok("rust".to_string())
            }
            _ => Err(errors::expected_identifier(
                &format!("{:?}", self.peek().kind),
                self.current_span(),
            )),
        }
    }

    /// Parse a list of identifiers and return them as spanned tokens.
    fn identifier_list_spanned(&mut self) -> Result<Vec<Spanned<Ident>>, CompileError> {
        let mut idents = vec![self.identifier_spanned()?];
        while self.match_token(&TokenKind::Punctuation(PunctuationId::Comma)) {
            idents.push(self.identifier_spanned()?);
        }
        Ok(idents)
    }

    /// Parse a string literal and return it as a spanned token.
    fn string_literal(&mut self) -> Result<String, CompileError> {
        match &self.peek().kind {
            TokenKind::String(s) => {
                let s = s.clone();
                self.advance();
                Ok(s)
            }
            _ => Err(errors::expected_string_literal(
                &format!("{:?}", self.peek().kind),
                self.current_span(),
            )),
        }
    }
}
