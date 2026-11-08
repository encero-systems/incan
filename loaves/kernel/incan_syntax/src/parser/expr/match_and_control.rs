/// Match, pattern, `if`, `loop` and `race for` expression parsing.
impl<'a> Parser<'a> {
    /// Parse a `match` expression.
    fn match_expr(&mut self, start: usize) -> Result<Spanned<Expr>, CompileError> {
        let subject = self.expression()?;
        self.expect(
            &TokenKind::Punctuation(PunctuationId::Colon),
            "Expected ':' after match subject",
        )?;
        self.expect(&TokenKind::Newline, "Expected newline after ':'")?;
        self.expect_suite_indent("Expected indented block")?;

        let mut arms = Vec::new();
        self.skip_newlines();
        let mut next_leading = 0u8;
        while !self.check(&TokenKind::Dedent) && !self.is_at_end() {
            let mut arm = self.match_arm()?;
            arm.leading_blank_lines = next_leading;
            arms.push(arm);

            next_leading = self.consume_inter_statement_blank_prefix();
            if next_leading > 0 && self.check(&TokenKind::Dedent) && self.dedent_is_followed_by_outer_statement() {
                self.pending_dedent_blank_lines = self.pending_dedent_blank_lines.max(next_leading);
                next_leading = 0;
            }
        }
        self.expect(&TokenKind::Dedent, "Expected dedent after match body")?;
        let end = self.tokens[self.pos - 1].span.end;
        Ok(Spanned::new(
            Expr::Match(Box::new(subject), arms),
            Span::new(start, end),
        ))
    }

    /// Parse one arm of a `match` expression, in either spelling.
    ///
    /// `case Pattern:` and `Pattern =>` are two spellings of one arm grammar, not two grammars. Everything between
    /// the pattern and the arm's terminator is common to both, so it is parsed once here. Splitting it per spelling
    /// is what let the two surfaces drift: the `case` form accepted a guard and the arrow form rejected one with
    /// `Expected '=>' after pattern`, for no reason the language states anywhere (#1401).
    fn match_arm(&mut self) -> Result<Spanned<MatchArm>, CompileError> {
        let start = self.current_span().start;

        let is_case_form = self.match_token(&TokenKind::Keyword(KeywordId::Case));
        let pattern = self.pattern()?;

        // ---- Optional guard, shared by both spellings: `Pattern if condition` ----
        let guard = if self.match_token(&TokenKind::Keyword(KeywordId::If)) {
            Some(self.expression()?)
        } else {
            None
        };

        // ---- The arm terminator is the only thing the two spellings genuinely differ on ----
        if is_case_form {
            self.expect(
                &TokenKind::Punctuation(PunctuationId::Colon),
                "Expected ':' after case pattern",
            )?;
        } else {
            self.expect(
                &TokenKind::Punctuation(PunctuationId::FatArrow),
                "Expected '=>' after pattern",
            )?;
        }

        // ---- Body: an indented suite when a newline follows, otherwise a single inline statement ----
        if self.match_token(&TokenKind::Newline) {
            self.expect_suite_indent("Expected indented block")?;
            let body = self.block()?;
            let dedent_message = if is_case_form {
                "Expected dedent after case body"
            } else {
                "Expected dedent after arm body"
            };
            self.expect(&TokenKind::Dedent, dedent_message)?;
            let end = self.tokens[self.pos - 1].span.end;
            Ok(Spanned::new(
                MatchArm {
                    pattern,
                    guard,
                    body: MatchBody::Block(body),
                },
                Span::new(start, end),
            ))
        } else {
            let stmt = self.inline_statement()?;
            let end = stmt.span.end;
            // `=> expr` is an expression arm and lowering treats it as one; an inline `case` body is a suite
            // written on one line, so it stays a block even when it holds a single expression.
            let body = match (&stmt.node, is_case_form) {
                (Statement::Expr(expr), false) => MatchBody::Expr(expr.clone()),
                _ => MatchBody::Block(vec![stmt]),
            };
            Ok(Spanned::new(MatchArm { pattern, guard, body }, Span::new(start, end)))
        }
    }

    /// Parse a full match-pattern grammar node, including top-level `|` alternation.
    fn pattern(&mut self) -> Result<Spanned<Pattern>, CompileError> {
        let start = self.current_span().start;
        let first = self.pattern_atom()?;

        if !self.match_token(&TokenKind::Punctuation(PunctuationId::Pipe)) {
            return Ok(first);
        }

        let mut patterns = vec![first];
        loop {
            patterns.push(self.pattern_atom()?);
            if !self.match_token(&TokenKind::Punctuation(PunctuationId::Pipe)) {
                break;
            }
        }

        let end = patterns.last().map(|pattern| pattern.span.end).unwrap_or(start);
        Ok(Spanned::new(Pattern::Or(patterns), Span::new(start, end)))
    }

    /// Parse one non-alternation pattern atom.
    ///
    /// Grouped patterns recurse through [`Self::pattern`] so `(A | B)` remains one grouped alternation rather than a
    /// tuple pattern.
    fn pattern_atom(&mut self) -> Result<Spanned<Pattern>, CompileError> {
        let start = self.current_span().start;

        // Wildcard
        if let TokenKind::Ident(name) = &self.peek().kind
            && name == "_"
        {
            self.advance();
            let end = self.tokens[self.pos - 1].span.end;
            return Ok(Spanned::new(Pattern::Wildcard, Span::new(start, end)));
        }

        // Literal patterns
        if let Some(lit) = self.try_literal() {
            let end = self.tokens[self.pos - 1].span.end;
            return Ok(Spanned::new(Pattern::Literal(lit), Span::new(start, end)));
        }

        // Tuple pattern
        if self.match_token(&TokenKind::Punctuation(PunctuationId::LParen)) {
            return self.parenthesized_pattern(start);
        }

        // Identifier (binding) or constructor pattern
        if let TokenKind::Ident(name) = &self.peek().kind {
            let mut name = Spanned::new(name.clone(), self.current_span());
            self.advance();

            // Normalize every segment of a qualified constructor path. Match checking already resolves the final
            // segment as the variant and treats everything before it as one path to the enum declaration.
            while self.match_token(&TokenKind::Punctuation(PunctuationId::Dot)) {
                let segment_span = self.current_span();
                let segment = match &self.peek().kind {
                    TokenKind::Ident(segment) => {
                        let segment = segment.clone();
                        self.advance();
                        segment
                    }
                    TokenKind::Keyword(KeywordId::None) => {
                        // Allow `None` as a variant name, including through a module-qualified enum.
                        self.advance();
                        "None".to_string()
                    }
                    _ => {
                        return Err(errors::expected_variant_name_after_dot(self.current_span()));
                    }
                };
                name = Spanned::new(
                    format!("{}::{}", name.node, segment),
                    Span::new(start, segment_span.end),
                );
            }

            if self.match_token(&TokenKind::Punctuation(PunctuationId::LParen)) {
                // Constructor pattern: Some(x), Ok(value), Shape::Circle(r), Type(name=pat), etc.
                let mut patterns = Vec::new();
                if !self.check(&TokenKind::Punctuation(PunctuationId::RParen)) {
                    loop {
                        if matches!(self.peek().kind, TokenKind::Ident(_) | TokenKind::Keyword(_))
                            && self.peek_next().kind == TokenKind::Operator(OperatorId::Eq)
                        {
                            let name_span = self.current_span();
                            let name = self.identifier_or_any_keyword()?;
                            self.expect(
                                &TokenKind::Operator(OperatorId::Eq),
                                "Expected '=' after pattern field name",
                            )?;
                            let pat = self.pattern()?;
                            patterns.push(PatternArg::Named(Spanned::new(name, name_span), pat));
                        } else {
                            patterns.push(PatternArg::Positional(self.pattern()?));
                        }
                        if !self.match_token(&TokenKind::Punctuation(PunctuationId::Comma)) {
                            break;
                        }
                    }
                }
                self.expect(
                    &TokenKind::Punctuation(PunctuationId::RParen),
                    "Expected ')' after constructor pattern",
                )?;
                let end = self.tokens[self.pos - 1].span.end;
                return Ok(Spanned::new(
                    Pattern::Constructor(name, patterns),
                    Span::new(start, end),
                ));
            }

            // Check if this is a unit variant (qualified without parens): Type.Variant
            if name.node.contains("::") {
                let end = self.tokens[self.pos - 1].span.end;
                return Ok(Spanned::new(Pattern::Constructor(name, vec![]), Span::new(start, end)));
            }

            // Just a binding
            let end = self.tokens[self.pos - 1].span.end;
            return Ok(Spanned::new(Pattern::Binding(name.node), Span::new(start, end)));
        }

        Err(errors::expected_pattern(
            &format!("{:?}", self.peek().kind),
            self.current_span(),
        ))
    }

    /// Parse a tuple or grouped pattern after consuming `(`, keeping alternation inside a group intact.
    fn parenthesized_pattern(&mut self, start: usize) -> Result<Spanned<Pattern>, CompileError> {
        if self.check(&TokenKind::Punctuation(PunctuationId::RParen)) {
            self.advance();
            let end = self.tokens[self.pos - 1].span.end;
            return Ok(Spanned::new(Pattern::Tuple(Vec::new()), Span::new(start, end)));
        }

        let first = self.pattern()?;
        if !self.match_token(&TokenKind::Punctuation(PunctuationId::Comma)) {
            self.expect(
                &TokenKind::Punctuation(PunctuationId::RParen),
                "Expected ')' after grouped pattern",
            )?;
            let end = self.tokens[self.pos - 1].span.end;
            return Ok(Spanned::new(Pattern::Group(Box::new(first)), Span::new(start, end)));
        }

        let mut patterns = vec![first];
        if !self.check(&TokenKind::Punctuation(PunctuationId::RParen)) {
            loop {
                patterns.push(self.pattern()?);
                if !self.match_token(&TokenKind::Punctuation(PunctuationId::Comma)) {
                    break;
                }
            }
        }
        self.expect(
            &TokenKind::Punctuation(PunctuationId::RParen),
            "Expected ')' after tuple pattern",
        )?;
        let end = self.tokens[self.pos - 1].span.end;
        Ok(Spanned::new(Pattern::Tuple(patterns), Span::new(start, end)))
    }

    /// Parse an `if` expression.
    fn if_expr(&mut self, start: usize) -> Result<Spanned<Expr>, CompileError> {
        self.expect(&TokenKind::Keyword(KeywordId::If), "Expected 'if'")?;
        let condition = self.expression()?;
        self.expect(
            &TokenKind::Punctuation(PunctuationId::Colon),
            "Expected ':' after if condition",
        )?;
        self.expect(&TokenKind::Newline, "Expected newline after ':'")?;
        self.expect_suite_indent("Expected indented block")?;
        let then_body = self.block()?;
        self.expect(&TokenKind::Dedent, "Expected dedent after if body")?;

        let else_body = if self.match_token(&TokenKind::Keyword(KeywordId::Else)) {
            self.expect(&TokenKind::Punctuation(PunctuationId::Colon), "Expected ':' after else")?;
            self.expect(&TokenKind::Newline, "Expected newline after ':'")?;
            self.expect_suite_indent("Expected indented block")?;
            let body = self.block()?;
            self.expect(&TokenKind::Dedent, "Expected dedent after else body")?;
            Some(body)
        } else {
            None
        };

        let end = self.tokens[self.pos - 1].span.end;
        Ok(Spanned::new(
            Expr::If(Box::new(IfExpr {
                condition,
                then_body,
                else_body,
            })),
            Span::new(start, end),
        ))
    }

    /// Parse a `loop` expression.
    fn loop_expr(&mut self, start: usize) -> Result<Spanned<Expr>, CompileError> {
        self.expect(&TokenKind::Keyword(KeywordId::Loop), "Expected 'loop'")?;
        self.expect(&TokenKind::Punctuation(PunctuationId::Colon), "Expected ':' after loop")?;
        self.expect(&TokenKind::Newline, "Expected newline after ':'")?;
        self.expect(&TokenKind::Indent, "Expected indented block")?;
        let body = self.block()?;
        self.expect(&TokenKind::Dedent, "Expected dedent after loop body")?;

        let end = self.tokens[self.pos - 1].span.end;
        Ok(Spanned::new(
            Expr::Loop(Box::new(LoopExpr { body })),
            Span::new(start, end),
        ))
    }

    /// Parse an active `race for value:` expression block.
    fn race_for_expr(&mut self, start: usize) -> Result<Spanned<Expr>, CompileError> {
        self.expect(&TokenKind::Ident("race".to_string()), "Expected 'race'")?;
        self.expect_keyword(KeywordId::For, "Expected 'for' after 'race'")?;
        if self.check_punct(PunctuationId::LParen) {
            return Err(errors::expected_token_message(
                "Pattern-binding race headers are not supported; use a single binding name and match inside the arm body",
                &format!("{:?}", self.peek().kind),
                self.current_span(),
            ));
        }
        let binding = self.identifier_spanned()?;
        if self.check_punct(PunctuationId::LParen) || self.check_punct(PunctuationId::Comma) {
            return Err(errors::expected_token_message(
                "Pattern-binding race headers are not supported; use a single binding name and match inside the arm body",
                &format!("{:?}", self.peek().kind),
                self.current_span(),
            ));
        }
        self.expect_punct(PunctuationId::Colon, "Expected ':' after race binding")?;
        self.expect(&TokenKind::Newline, "Expected newline after race header")?;
        self.expect_suite_indent("Expected indented block after race header")?;

        self.skip_newlines();
        if self.check(&TokenKind::Dedent) {
            return Err(errors::expected_token_message(
                "Expected at least one race arm",
                &format!("{:?}", self.peek().kind),
                self.current_span(),
            ));
        }

        let mut arms = Vec::new();
        while !self.check(&TokenKind::Dedent) && !self.is_at_end() {
            arms.push(self.race_for_arm()?);
            self.skip_newlines();
        }
        self.expect(&TokenKind::Dedent, "Expected dedent after race body")?;

        let end = self.tokens[self.pos.saturating_sub(1)].span.end;
        Ok(Spanned::new(
            Expr::Surface(Box::new(SurfaceExpr {
                key: SurfaceFeatureKey::ScopedDslSurface {
                    dependency_key: "std.async".to_string(),
                    descriptor_key: "race_for".to_string(),
                },
                payload: SurfaceExprPayload::RaceFor(Box::new(RaceForExpr { binding, arms })),
            })),
            Span::new(start, end),
        ))
    }

    /// Parse one `await expr => body` race arm.
    fn race_for_arm(&mut self) -> Result<RaceForArm, CompileError> {
        if matches!(&self.peek().kind, TokenKind::Ident(name) if name == "default") {
            return Err(errors::expected_token_message(
                "Default race arms are not supported; race an explicit timeout awaitable instead",
                &format!("{:?}", self.peek().kind),
                self.current_span(),
            ));
        }
        if matches!(&self.peek().kind, TokenKind::Ident(name) if matches!(name.as_str(), "fair" | "biased")) {
            return Err(errors::expected_token_message(
                "Race fairness controls are not supported",
                &format!("{:?}", self.peek().kind),
                self.current_span(),
            ));
        }
        self.expect_keyword(KeywordId::Await, "Expected 'await' to start race arm")?;
        let awaitable = self.expression()?;
        if self.check_keyword(KeywordId::If) {
            return Err(errors::expected_token_message(
                "Race arm guards are not supported; put the conditional logic in the arm body",
                &format!("{:?}", self.peek().kind),
                self.current_span(),
            ));
        }
        self.expect_punct(PunctuationId::FatArrow, "Expected '=>' after race arm awaitable")?;
        let body = if self.match_token(&TokenKind::Newline) {
            self.expect_suite_indent("Expected indented block after race arm '=>'")?;
            let body = self.block()?;
            if body.is_empty() {
                return Err(errors::expected_token_message(
                    "Expected race arm body",
                    &format!("{:?}", self.peek().kind),
                    self.current_span(),
                ));
            }
            self.expect(&TokenKind::Dedent, "Expected dedent after race arm body")?;
            RaceForBody::Block(body)
        } else {
            RaceForBody::Expr(self.expression()?)
        };

        Ok(RaceForArm { awaitable, body })
    }
}
