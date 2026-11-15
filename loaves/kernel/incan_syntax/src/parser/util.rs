/// Miscellaneous parser utilities.
///
/// This chunk contains small shared parsing helpers that don’t cleanly fit into “decl”, “stmt”, “expr”, or “types”
/// (e.g. identifier parsing and string literal handling).
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
            _ => Err(self.identifier_error()),
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
            _ => Err(self.identifier_error()),
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
            _ => Err(self.identifier_error()),
        }
    }

    /// Build the targeted reserved-word diagnostic for `async` and `await`, or the ordinary identifier diagnostic.
    fn identifier_error(&self) -> CompileError {
        match &self.peek().kind {
            TokenKind::Keyword(KeywordId::Async) => errors::reserved_keyword_name("async", self.current_span()),
            TokenKind::Keyword(KeywordId::Await) => errors::reserved_keyword_name("await", self.current_span()),
            _ => errors::expected_identifier(&format!("{:?}", self.peek().kind), self.current_span()),
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
                let name = incan_lang::lang::keywords::as_str(*kw).to_string();
                self.advance();
                Ok(name)
            }
            _ => Err(self.identifier_error()),
        }
    }

    /// Parse an identifier, allowing the hard `from` keyword in declaration-name position.
    ///
    /// This keeps `def from(...)` available for stdlib conversion hooks without widening every declaration-name site to
    /// arbitrary keywords.
    fn identifier_or_from_keyword(&mut self) -> Result<Ident, CompileError> {
        match &self.peek().kind {
            TokenKind::Ident(name) => {
                let name = name.clone();
                self.advance();
                Ok(name)
            }
            TokenKind::Keyword(KeywordId::From) => {
                self.advance();
                Ok("from".to_string())
            }
            _ => Err(self.identifier_error()),
        }
    }

    /// Parse an identifier in import/decorator paths, allowing reserved module-path segments.
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
            TokenKind::Keyword(KeywordId::Async) => {
                self.advance();
                Ok("async".to_string())
            }
            TokenKind::Keyword(KeywordId::Await) => {
                self.advance();
                Ok("await".to_string())
            }
            _ => Err(errors::expected_identifier(
                &format!("{:?}", self.peek().kind),
                self.current_span(),
            )),
        }
    }

    /// Parse an imported item name, allowing the `async` module without allowing the word as an alias or source name.
    fn identifier_or_reserved_module_keyword(&mut self) -> Result<Ident, CompileError> {
        match &self.peek().kind {
            TokenKind::Keyword(KeywordId::Async) => {
                self.advance();
                Ok("async".to_string())
            }
            _ => self.identifier(),
        }
    }

    /// Return `true` when the current token is an identifier with exact text `value`.
    fn peek_ident_text(&self, value: &str) -> bool {
        matches!(&self.peek().kind, TokenKind::Ident(name) if name == value)
    }

    /// Consume the current token when it is an identifier with exact text `value`.
    fn match_ident_text(&mut self, value: &str) -> bool {
        if self.peek_ident_text(value) {
            self.advance();
            true
        } else {
            false
        }
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
