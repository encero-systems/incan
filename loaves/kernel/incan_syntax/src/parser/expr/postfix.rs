/// Postfix expression parsing: calls, fluent chains, call-site type arguments, indexing and slicing.
impl<'a> Parser<'a> {
    /// Parse postfix forms such as calls, method calls, field access, indexing, and `?`.
    fn postfix(&mut self) -> Result<Spanned<Expr>, CompileError> {
        let mut expr = self.primary()?;
        let mut fluent_layout_depth = 0usize;

        loop {
            if self.consume_fluent_chain_terminator(&mut fluent_layout_depth) {
                break;
            }
            self.consume_fluent_chain_continuation(&mut fluent_layout_depth);

            if self.match_token(&TokenKind::Punctuation(PunctuationId::Question)) {
                let span = Span::new(expr.span.start, self.tokens[self.pos - 1].span.end);
                expr = Spanned::new(Expr::Try(Box::new(expr)), span);
            } else if self.match_token(&TokenKind::Punctuation(PunctuationId::Dot)) {
                // Check for tuple index access (.0, .1, etc) vs field/method access
                if let TokenKind::Int(il) = &self.peek().kind {
                    // Tuple index access: expr.0, expr.1
                    let idx = il.value;
                    self.advance();
                    let span = Span::new(expr.span.start, self.tokens[self.pos - 1].span.end);
                    // Use the index as a string field name
                    expr = Spanned::new(Expr::Field(Box::new(expr), idx.to_string()), span);
                } else {
                    // Allow keywords like "None" as field/variant names
                    let name = self.identifier_or_any_keyword()?;
                    let type_args = self.call_site_type_args()?;
                    if !type_args.is_empty() && !self.match_token(&TokenKind::Punctuation(PunctuationId::LParen)) {
                        return Err(errors::expected_token_message(
                            "Expected '(' after explicit method type arguments",
                            &format!("{:?}", self.peek().kind),
                            self.peek().span,
                        ));
                    }
                    if (type_args.is_empty() && self.match_token(&TokenKind::Punctuation(PunctuationId::LParen)))
                        || !type_args.is_empty()
                    {
                        let args = self.call_args_for(Some(name.clone()))?;
                        self.expect(
                            &TokenKind::Punctuation(PunctuationId::RParen),
                            "Expected ')' after arguments",
                        )?;
                        let span = Span::new(expr.span.start, self.tokens[self.pos - 1].span.end);
                        expr = Spanned::new(Expr::MethodCall(Box::new(expr), name, type_args, args), span);
                    } else {
                        let span = Span::new(expr.span.start, self.tokens[self.pos - 1].span.end);
                        expr = Spanned::new(Expr::Field(Box::new(expr), name), span);
                    }
                }
            } else if self.check(&TokenKind::Punctuation(PunctuationId::LBracket)) {
                if matches!(expr.node, Expr::Ident(_) | Expr::Field(_, _)) {
                    let type_args = self.call_site_type_args()?;
                    if !type_args.is_empty() {
                        self.expect(
                            &TokenKind::Punctuation(PunctuationId::LParen),
                            "Expected '(' after explicit function type arguments",
                        )?;
                        let args = self.call_args_for(self.call_argument_target(&expr))?;
                        self.expect(
                            &TokenKind::Punctuation(PunctuationId::RParen),
                            "Expected ')' after arguments",
                        )?;
                        let span = Span::new(expr.span.start, self.tokens[self.pos - 1].span.end);
                        expr = Spanned::new(Expr::Call(Box::new(expr), type_args, args), span);
                        continue;
                    }
                }

                self.expect(
                    &TokenKind::Punctuation(PunctuationId::LBracket),
                    "Expected '[' before index/slice",
                )?;
                // Check for slice syntax: [start:end] or [start:end:step]
                let result = self.index_or_slice()?;
                self.expect(
                    &TokenKind::Punctuation(PunctuationId::RBracket),
                    "Expected ']' after index/slice",
                )?;
                let span = Span::new(expr.span.start, self.tokens[self.pos - 1].span.end);
                expr = match result {
                    IndexOrSlice::Index(index) => Spanned::new(Expr::Index(Box::new(expr), Box::new(index)), span),
                    IndexOrSlice::Slice(slice) => Spanned::new(Expr::Slice(Box::new(expr), slice), span),
                };
            } else if self.match_token(&TokenKind::Punctuation(PunctuationId::LParen)) {
                let args = self.call_args_for(self.call_argument_target(&expr))?;
                self.expect(
                    &TokenKind::Punctuation(PunctuationId::RParen),
                    "Expected ')' after arguments",
                )?;
                let span = Span::new(expr.span.start, self.tokens[self.pos - 1].span.end);
                expr = if let Expr::Ident(name) = &expr.node {
                    if let Some(active) = self.active_scoped_symbol_descriptor(name, span)? {
                        Spanned::new(
                            Expr::Surface(Box::new(SurfaceExpr {
                                key: SurfaceFeatureKey::ScopedDslSurface {
                                    dependency_key: active.dependency_key.clone(),
                                    descriptor_key: active.descriptor.key.clone(),
                                },
                                payload: SurfaceExprPayload::ScopedSymbolCall {
                                    symbol: name.clone(),
                                    args,
                                    owner: self.scoped_symbol_owner(active),
                                },
                            })),
                            span,
                        )
                    } else if let Some(error) = self.scoped_symbol_misuse_error(name, span)? {
                        return Err(error);
                    } else {
                        Spanned::new(Expr::Call(Box::new(expr), Vec::new(), args), span)
                    }
                } else {
                    Spanned::new(Expr::Call(Box::new(expr), Vec::new(), args), span)
                };
            } else {
                break;
            }
        }

        Ok(expr)
    }

    /// Consume the layout prefix for an indented fluent method/field continuation.
    ///
    /// This accepts source shaped as:
    ///
    /// ```text
    /// value = receiver
    ///     .method()
    ///     .field
    /// ```
    ///
    /// The lexer emits ordinary `Newline`/`Indent`/`Dedent` tokens for that layout. Treating the indented leading
    /// `.` as expression continuation keeps fluent APIs writable without requiring wrapping parentheses.
    fn consume_fluent_chain_continuation(&mut self, fluent_layout_depth: &mut usize) -> bool {
        if !self.check(&TokenKind::Newline) {
            return false;
        }

        let next = self.tokens.get(self.pos + 1).map(|token| &token.kind);
        let after_next = self.tokens.get(self.pos + 2).map(|token| &token.kind);
        if matches!(next, Some(TokenKind::Indent))
            && matches!(after_next, Some(TokenKind::Punctuation(PunctuationId::Dot)))
        {
            self.advance(); // newline
            self.advance(); // continuation indent
            *fluent_layout_depth += 1;
            return true;
        }

        if *fluent_layout_depth > 0 && matches!(next, Some(TokenKind::Punctuation(PunctuationId::Dot))) {
            self.advance(); // newline between continuation lines
            return true;
        }

        false
    }

    /// Consume the layout suffix that closes an indented fluent continuation.
    fn consume_fluent_chain_terminator(&mut self, fluent_layout_depth: &mut usize) -> bool {
        if *fluent_layout_depth == 0 || !self.check(&TokenKind::Newline) {
            return false;
        }

        let mut next_non_newline = self.pos;
        while matches!(
            self.tokens.get(next_non_newline).map(|token| &token.kind),
            Some(TokenKind::Newline)
        ) {
            next_non_newline += 1;
        }

        if !matches!(
            self.tokens.get(next_non_newline).map(|token| &token.kind),
            Some(TokenKind::Dedent)
        ) {
            return false;
        }

        while self.pos < next_non_newline {
            self.advance(); // final continuation newline or blank continuation line
        }
        self.advance(); // continuation dedent
        *fluent_layout_depth = fluent_layout_depth.saturating_sub(1);
        true
    }

    /// Parse one call-site type argument: either a full [`Type`] or the inference placeholder `_`.
    pub(super) fn call_site_type_arg(&mut self) -> Result<Spanned<Type>, CompileError> {
        if let TokenKind::Ident(name) = &self.peek().kind
            && name == "_"
        {
            let span = self.peek().span;
            self.advance();
            return Ok(Spanned::new(Type::Infer, span));
        }
        self.type_expr()
    }

    /// Parse optional explicit call-site type arguments (`[T, U]`) without consuming non-call brackets.
    ///
    /// This is intentionally conservative: we only treat brackets as call-site type args when the matching `]` is
    /// followed immediately by `(`.
    pub(super) fn call_site_type_args(&mut self) -> Result<Vec<Spanned<Type>>, CompileError> {
        if !self.check(&TokenKind::Punctuation(PunctuationId::LBracket)) {
            return Ok(Vec::new());
        }

        // Cheap lookahead: only attempt type parsing when the matching `]` is followed by `(`.
        // This prevents speculative type parsing from consuming ordinary index expressions like `arr[0]`.
        let mut depth: isize = 0;
        let mut i = self.pos;
        let mut closing: Option<usize> = None;
        while i < self.tokens.len() {
            match self.tokens[i].kind {
                TokenKind::Punctuation(PunctuationId::LBracket) => depth += 1,
                TokenKind::Punctuation(PunctuationId::RBracket) => {
                    depth -= 1;
                    if depth == 0 {
                        closing = Some(i);
                        break;
                    }
                }
                TokenKind::Eof => break,
                _ => {}
            }
            i += 1;
        }
        let Some(close_idx) = closing else {
            return Ok(Vec::new());
        };
        let next_idx = close_idx + 1;
        if next_idx >= self.tokens.len() || self.tokens[next_idx].kind != TokenKind::Punctuation(PunctuationId::LParen)
        {
            return Ok(Vec::new());
        }

        self.advance(); // consume `[`
        let mut args = Vec::new();
        if !self.check(&TokenKind::Punctuation(PunctuationId::RBracket)) {
            loop {
                args.push(self.call_site_type_arg()?);
                if !self.match_token(&TokenKind::Punctuation(PunctuationId::Comma)) {
                    break;
                }
                if self.check(&TokenKind::Punctuation(PunctuationId::RBracket)) {
                    break;
                }
            }
        }
        self.expect(
            &TokenKind::Punctuation(PunctuationId::RBracket),
            "Expected ']' after explicit call type arguments",
        )?;
        Ok(args)
    }

    /// Parse index or slice expression inside brackets Handles: [expr], [start:end], [start:end:step], [:end],
    /// [start:], [::step], [start::step]
    fn index_or_slice(&mut self) -> Result<IndexOrSlice, CompileError> {
        // Check for immediate colon (slice starting with no start value)
        if self.check(&TokenKind::Punctuation(PunctuationId::Colon)) {
            return self.parse_slice(None);
        }
        if self.check(&TokenKind::Punctuation(PunctuationId::ColonColon)) {
            return self.parse_slice_without_end(None);
        }

        // Check for immediate closing bracket (not valid, but let expression handle error)
        if self.check(&TokenKind::Punctuation(PunctuationId::RBracket)) {
            return Err(errors::empty_index_not_allowed(self.current_span()));
        }

        // Parse first expression
        let first = self.expression()?;

        // Check if this is a slice (has colon after first expression)
        if self.check(&TokenKind::Punctuation(PunctuationId::Colon)) {
            return self.parse_slice(Some(first));
        }
        if self.check(&TokenKind::Punctuation(PunctuationId::ColonColon)) {
            return self.parse_slice_without_end(Some(first));
        }

        // Just a regular index
        Ok(IndexOrSlice::Index(first))
    }

    /// Parse slice syntax after optional start expression start is already parsed, now parse [:end[:step]]
    fn parse_slice(&mut self, start: Option<Spanned<Expr>>) -> Result<IndexOrSlice, CompileError> {
        // Consume the first colon
        self.expect(&TokenKind::Punctuation(PunctuationId::Colon), "Expected ':' in slice")?;

        // Parse end (optional - check for ] or :)
        let end = if !self.check(&TokenKind::Punctuation(PunctuationId::RBracket))
            && !self.check(&TokenKind::Punctuation(PunctuationId::Colon))
        {
            Some(Box::new(self.expression()?))
        } else {
            None
        };

        // Parse step (optional - only if there's another colon)
        let step = if self.match_token(&TokenKind::Punctuation(PunctuationId::Colon)) {
            if !self.check(&TokenKind::Punctuation(PunctuationId::RBracket)) {
                Some(Box::new(self.expression()?))
            } else {
                None
            }
        } else {
            None
        };

        Ok(IndexOrSlice::Slice(SliceExpr {
            start: start.map(Box::new),
            end,
            step,
        }))
    }

    /// Parse a slice whose end is omitted and whose two colons the lexer reads as one `::` token (`[::step]`,
    /// `[start::step]`, `[::]`); the start, if any, is already parsed.
    fn parse_slice_without_end(&mut self, start: Option<Spanned<Expr>>) -> Result<IndexOrSlice, CompileError> {
        self.expect(
            &TokenKind::Punctuation(PunctuationId::ColonColon),
            "Expected '::' in slice",
        )?;
        let step = if self.check(&TokenKind::Punctuation(PunctuationId::RBracket)) {
            None
        } else {
            Some(Box::new(self.expression()?))
        };
        Ok(IndexOrSlice::Slice(SliceExpr {
            start: start.map(Box::new),
            end: None,
            step,
        }))
    }
}
