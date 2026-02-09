/// Token-stream helpers and error recovery.
///
/// This chunk contains the low-level primitives used throughout parsing:
/// - Peeking/consuming tokens (`peek`, `advance`)
/// - Matching / expecting keywords, operators, and punctuation
/// - Layout handling (`skip_newlines`, `skip_dedents`)
/// - Error recovery (`synchronize`)
///
/// Most functions in this file are internal (`fn`) and are documented primarily
/// to aid maintenance and onboarding.
impl<'a> Parser<'a> {
    // ========================================================================
    // Helpers
    // ========================================================================

    /// Return `true` if the current token is [`TokenKind::Eof`].
    fn is_at_end(&self) -> bool {
        matches!(self.peek().kind, TokenKind::Eof)
    }

    /// Return the current token without consuming it.
    fn peek(&self) -> &Token {
        &self.tokens[self.pos]
    }

    /// Return the token after the current token without consuming it.
    fn peek_next(&self) -> &Token {
        if self.pos + 1 < self.tokens.len() {
            &self.tokens[self.pos + 1]
        } else {
            &self.tokens[self.tokens.len() - 1]
        }
    }

    /// Advance to the next token and return the token we just consumed.
    fn advance(&mut self) -> &Token {
        if !self.is_at_end() {
            self.pos += 1;
        }
        &self.tokens[self.pos - 1]
    }

    /// Return `true` if the current token “matches” `kind`.
    ///
    /// ## Notes
    /// - For ID-carrying tokens (keywords/operators/punctuation), the IDs must match.
    /// - For data-bearing tokens (identifiers/literals), the variant is compared and the
    ///   payload value is ignored.
    fn check(&self, kind: &TokenKind) -> bool {
        match (kind, &self.peek().kind) {
            (TokenKind::Keyword(k1), TokenKind::Keyword(k2)) => k1 == k2,
            (TokenKind::Operator(o1), TokenKind::Operator(o2)) => o1 == o2,
            (TokenKind::Punctuation(p1), TokenKind::Punctuation(p2)) => p1 == p2,
            (TokenKind::Ident(_), TokenKind::Ident(_))
            | (TokenKind::Int(_), TokenKind::Int(_))
            | (TokenKind::Float(_), TokenKind::Float(_))
            | (TokenKind::String(_), TokenKind::String(_))
            | (TokenKind::Bytes(_), TokenKind::Bytes(_))
            | (TokenKind::FString(_), TokenKind::FString(_)) => true,
            _ => std::mem::discriminant(kind) == std::mem::discriminant(&self.peek().kind),
        }
    }

    /// Return `true` if the current token is the given keyword.
    fn check_keyword(&self, id: KeywordId) -> bool {
        self.peek().kind.is_keyword(id)
    }

    /// Return `true` if the current token is the given punctuation.
    fn check_punct(&self, id: PunctuationId) -> bool {
        self.peek().kind.is_punctuation(id)
    }

    /// Return `true` if the current token is the given operator.
    fn check_op(&self, id: OperatorId) -> bool {
        self.peek().kind.is_operator(id)
    }

    /// If the current token matches `kind`, consume it and return `true`.
    fn match_token(&mut self, kind: &TokenKind) -> bool {
        if self.check(kind) {
            self.advance();
            true
        } else {
            false
        }
    }

    fn match_keyword(&mut self, id: KeywordId) -> bool {
        if self.check_keyword(id) {
            self.advance();
            true
        } else {
            false
        }
    }

    fn match_punct(&mut self, id: PunctuationId) -> bool {
        if self.check_punct(id) {
            self.advance();
            true
        } else {
            false
        }
    }

    fn match_op(&mut self, id: OperatorId) -> bool {
        if self.check_op(id) {
            self.advance();
            true
        } else {
            false
        }
    }

    fn expect(&mut self, kind: &TokenKind, msg: &str) -> Result<&Token, CompileError> {
        if self.check(kind) {
            Ok(self.advance())
        } else {
            Err(errors::expected_token_message(
                msg,
                &format!("{:?}", self.peek().kind),
                self.peek().span,
            ))
        }
    }

    fn expect_keyword(&mut self, id: KeywordId, msg: &str) -> Result<&Token, CompileError> {
        if self.check_keyword(id) {
            Ok(self.advance())
        } else {
            Err(errors::expected_token_message(
                msg,
                &format!("{:?}", self.peek().kind),
                self.peek().span,
            ))
        }
    }

    fn expect_punct(&mut self, id: PunctuationId, msg: &str) -> Result<&Token, CompileError> {
        if self.check_punct(id) {
            Ok(self.advance())
        } else {
            Err(errors::expected_token_message(
                msg,
                &format!("{:?}", self.peek().kind),
                self.peek().span,
            ))
        }
    }

    fn expect_op(&mut self, id: OperatorId, msg: &str) -> Result<&Token, CompileError> {
        if self.check_op(id) {
            Ok(self.advance())
        } else {
            Err(errors::expected_token_message(
                msg,
                &format!("{:?}", self.peek().kind),
                self.peek().span,
            ))
        }
    }

    fn skip_newlines(&mut self) {
        while self.match_token(&TokenKind::Newline) {}
    }

    /// Skip stray DEDENT tokens at the current position.
    ///
    /// These should not normally appear at module level, but can show up after error recovery.
    fn skip_dedents(&mut self) {
        while self.match_token(&TokenKind::Dedent) {}
    }

    fn synchronize(&mut self) {
        self.advance();
        while !self.is_at_end() {
            if self.check_keyword(KeywordId::Def)
                || self.check_keyword(KeywordId::Class)
                || self.check_keyword(KeywordId::Model)
                || self.check_keyword(KeywordId::Trait)
                || self.check_keyword(KeywordId::Enum)
                || self.check_keyword(KeywordId::Type)
                || self.check_keyword(KeywordId::Const)
                || self.check_keyword(KeywordId::Import)
            {
                return;
            }
            if matches!(self.peek().kind, TokenKind::Newline) {
                self.advance();
                return;
            }
            self.advance();
        }
    }

    fn current_span(&self) -> Span {
        self.peek().span
    }

    /// Check if the current token can start an expression
    fn is_at_expr_start(&self) -> bool {
        matches!(
            self.peek().kind,
            TokenKind::Ident(_)
                | TokenKind::Int(_)
                | TokenKind::Float(_)
                | TokenKind::String(_)
                | TokenKind::Bytes(_)
                | TokenKind::FString(_)
        ) || self.check_keyword(KeywordId::True)
            || self.check_keyword(KeywordId::False)
            || self.check_keyword(KeywordId::None)
            || self.check_keyword(KeywordId::Not)
            || self.check_keyword(KeywordId::SelfKw)
            || self.check_keyword(KeywordId::Await)
            || self.check_keyword(KeywordId::Match)
            || self.check_keyword(KeywordId::If)
            || self.check_punct(PunctuationId::LParen)
            || self.check_punct(PunctuationId::LBracket)
            || self.check_punct(PunctuationId::LBrace)
            || self.check_op(OperatorId::Minus)
    }

}
