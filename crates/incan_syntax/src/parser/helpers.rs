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
        if self.peek().kind.is_keyword(id) {
            return true;
        }
        if !incan_core::lang::keywords::is_soft(id) || !self.active_soft_keywords.contains(&id) {
            return false;
        }
        matches!(
            &self.peek().kind,
            TokenKind::Ident(name) if name == incan_core::lang::keywords::as_str(id)
        )
    }

    /// Return a targeted error if the current token is an inactive soft keyword.
    fn inactive_soft_keyword_error(&self) -> Option<CompileError> {
        let TokenKind::Ident(name) = &self.peek().kind else {
            return None;
        };
        let id = incan_core::lang::keywords::from_str(name)?;
        if !incan_core::lang::keywords::is_soft(id) || self.active_soft_keywords.contains(&id) {
            return None;
        }
        let namespace = incan_core::lang::keywords::activation(id)?;
        Some(errors::soft_keyword_requires_import(name, namespace, self.current_span()))
    }

    /// Return the currently-active soft keyword id, if the current token is an active soft keyword spelling.
    fn current_active_soft_keyword(&self) -> Option<KeywordId> {
        match &self.peek().kind {
            TokenKind::Keyword(id) if incan_core::lang::keywords::is_soft(*id) && self.active_soft_keywords.contains(id) => {
                Some(*id)
            }
            TokenKind::Ident(name) => {
                let id = incan_core::lang::keywords::from_str(name)?;
                if incan_core::lang::keywords::is_soft(id) && self.active_soft_keywords.contains(&id) {
                    Some(id)
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    /// Return `true` if keyword `id` is allowed in the requested parser surface.
    ///
    /// Imported-library registrations are checked first so consumer-side vocab metadata can widen accepted surfaces for active soft keywords; builtin metadata is used as fallback.
    fn keyword_supports_surface_usage(&self, id: KeywordId, usage: KeywordSurfaceKind) -> bool {
        let keyword_name = incan_core::lang::keywords::as_str(id);
        if let Some(specs) = self.active_imported_keyword_specs.get(keyword_name)
            && specs.iter().any(|spec| {
                keyword_surface_supports_usage(spec.surface_kind, usage)
                    && matches!(spec.placement, incan_vocab::KeywordPlacement::TopLevel)
            })
        {
            return true;
        }
        incan_core::lang::keywords::supports_surface_kind(id, usage)
    }

    /// Return the active soft keyword id if it is valid for the requested parser surface.
    fn current_surface_keyword(&self, usage: KeywordSurfaceKind) -> Option<KeywordId> {
        let id = self.current_active_soft_keyword()?;
        if self.keyword_supports_surface_usage(id, usage) {
            Some(id)
        } else {
            None
        }
    }

    /// Consume and return the active soft keyword id if it is valid for the requested parser surface.
    fn match_surface_keyword(&mut self, usage: KeywordSurfaceKind) -> Option<KeywordId> {
        let id = self.current_surface_keyword(usage)?;
        self.advance();
        Some(id)
    }

    /// Whether the current token stream starts a function/method declaration (`[soft_kw ...] def`).
    fn starts_surface_function_decl(&self) -> bool {
        if self.check_keyword(KeywordId::Def) {
            return true;
        }

        let mut idx = self.pos;
        let mut saw_modifier = false;
        while let Some(token) = self.tokens.get(idx) {
            let id = match &token.kind {
                TokenKind::Keyword(id) => Some(*id),
                TokenKind::Ident(name) => incan_core::lang::keywords::from_str(name),
                _ => None,
            };
            let Some(id) = id else {
                break;
            };
            if !self.active_soft_keywords.contains(&id)
                || !self.keyword_supports_surface_usage(id, KeywordSurfaceKind::DeclarationModifier)
            {
                break;
            }
            saw_modifier = true;
            idx += 1;
        }

        saw_modifier
            && matches!(
                self.tokens.get(idx).map(|t| &t.kind),
                Some(TokenKind::Keyword(KeywordId::Def))
            )
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

    /// If the current token matches `id` (including active soft-keyword spellings), consume it.
    fn match_keyword(&mut self, id: KeywordId) -> bool {
        if self.check_keyword(id) {
            self.advance();
            true
        } else {
            false
        }
    }

    /// If the current token matches punctuation `id`, consume it.
    fn match_punct(&mut self, id: PunctuationId) -> bool {
        if self.check_punct(id) {
            self.advance();
            true
        } else {
            false
        }
    }

    /// If the current token matches operator `id`, consume it.
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

    /// Expect keyword `id` at the current position.
    ///
    /// Accepts either a hard-keyword token or an active soft-keyword identifier spelling.
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

    /// Expect punctuation `id` at the current position.
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

    /// Expect operator `id` at the current position.
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

    /// Advance until a likely declaration boundary after a parse error.
    ///
    /// This targets top-level declaration starters and newline boundaries to reduce cascading diagnostics.
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

    /// Span of the current token.
    fn current_span(&self) -> Span {
        self.peek().span
    }

    /// Return `true` when the current token can begin an expression.
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
            || self.current_surface_keyword(KeywordSurfaceKind::PrefixExpression).is_some()
            || self.check_keyword(KeywordId::Match)
            || self.check_keyword(KeywordId::If)
            || self.check_punct(PunctuationId::LParen)
            || self.check_punct(PunctuationId::LBracket)
            || self.check_punct(PunctuationId::LBrace)
            || self.check_op(OperatorId::Minus)
    }

}

fn keyword_surface_supports_usage(
    surface_kind: incan_vocab::KeywordSurfaceKind,
    usage: KeywordSurfaceKind,
) -> bool {
    matches!(
        (surface_kind, usage),
        (
            incan_vocab::KeywordSurfaceKind::ControlFlow | incan_vocab::KeywordSurfaceKind::BlockContextKeyword,
            KeywordSurfaceKind::StatementKeywordArgs
        ) | (
            incan_vocab::KeywordSurfaceKind::FunctionDecl,
            KeywordSurfaceKind::DeclarationModifier
        ) | (
            incan_vocab::KeywordSurfaceKind::TryBlock,
            KeywordSurfaceKind::PrefixExpression
        )
    )
}
