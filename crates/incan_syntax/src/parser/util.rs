/// Miscellaneous parser utilities.
///
/// This chunk contains small shared parsing helpers that don’t cleanly fit into
/// “decl”, “stmt”, “expr”, or “types” (e.g. identifier parsing and string literal handling).
impl<'a> Parser<'a> {
    // ========================================================================
    // Utilities
    // ========================================================================

    fn identifier(&mut self) -> Result<Ident, CompileError> {
        match &self.peek().kind {
            TokenKind::Ident(name) => {
                let name = name.clone();
                self.advance();
                Ok(name)
            }
            _ => Err(CompileError::syntax(
                format!("Expected identifier, found {:?}", self.peek().kind),
                self.current_span(),
            )),
        }
    }

    fn identifier_spanned(&mut self) -> Result<Spanned<Ident>, CompileError> {
        match &self.peek().kind {
            TokenKind::Ident(name) => {
                let span = self.current_span();
                let name = name.clone();
                self.advance();
                Ok(Spanned::new(name, span))
            }
            _ => Err(CompileError::syntax(
                format!("Expected identifier, found {:?}", self.peek().kind),
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
            _ => Err(CompileError::syntax(
                format!("Expected identifier, found {:?}", self.peek().kind),
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
            _ => Err(CompileError::syntax(
                format!("Expected identifier, found {:?}", self.peek().kind),
                self.current_span(),
            )),
        }
    }

    fn identifier_list(&mut self) -> Result<Vec<Ident>, CompileError> {
        let mut idents = vec![self.identifier()?];
        while self.match_token(&TokenKind::Punctuation(PunctuationId::Comma)) {
            idents.push(self.identifier()?);
        }
        Ok(idents)
    }

    fn identifier_list_spanned(&mut self) -> Result<Vec<Spanned<Ident>>, CompileError> {
        let mut idents = vec![self.identifier_spanned()?];
        while self.match_token(&TokenKind::Punctuation(PunctuationId::Comma)) {
            idents.push(self.identifier_spanned()?);
        }
        Ok(idents)
    }

    fn string_literal(&mut self) -> Result<String, CompileError> {
        match &self.peek().kind {
            TokenKind::String(s) => {
                let s = s.clone();
                self.advance();
                Ok(s)
            }
            _ => Err(CompileError::syntax(
                format!("Expected string literal, found {:?}", self.peek().kind),
                self.current_span(),
            )),
        }
    }
}
