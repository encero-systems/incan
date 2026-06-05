/// Expression parsing methods.
///
/// This chunk implements the expression grammar using a precedence ladder:
/// `or` → `and` → `not` → comparison → range → additive → multiplicative → power → unary → postfix → primary.
///
/// ## Notes
/// - Operator identities are carried by [`TokenKind::Operator`] / [`OperatorId`] rather than string spellings.
/// - Many helpers here return [`Spanned<Expr>`] to preserve accurate spans for diagnostics.
impl<'a> Parser<'a> {
    // ========================================================================
    // Expressions
    // ========================================================================

    fn expression(&mut self) -> Result<Spanned<Expr>, CompileError> {
        self.or_expr()
    }

    fn or_expr(&mut self) -> Result<Spanned<Expr>, CompileError> {
        let mut left = self.and_expr()?;
        while self.match_token(&TokenKind::Keyword(KeywordId::Or)) {
            let right = self.and_expr()?;
            let span = left.span.merge(right.span);
            left = Spanned::new(Expr::Binary(Box::new(left), BinaryOp::Or, Box::new(right)), span);
        }
        Ok(left)
    }

    fn and_expr(&mut self) -> Result<Spanned<Expr>, CompileError> {
        let mut left = self.not_expr()?;
        while self.match_token(&TokenKind::Keyword(KeywordId::And)) {
            let right = self.not_expr()?;
            let span = left.span.merge(right.span);
            left = Spanned::new(Expr::Binary(Box::new(left), BinaryOp::And, Box::new(right)), span);
        }
        Ok(left)
    }

    fn not_expr(&mut self) -> Result<Spanned<Expr>, CompileError> {
        if self.match_token(&TokenKind::Keyword(KeywordId::Not)) {
            let start = self.tokens[self.pos - 1].span.start;
            let expr = self.not_expr()?;
            let span = Span::new(start, expr.span.end);
            Ok(Spanned::new(Expr::Unary(UnaryOp::Not, Box::new(expr)), span))
        } else {
            self.comparison()
        }
    }

    /// Parse comparison expressions and route registered scoped glyphs before ordinary core operators.
    fn comparison(&mut self) -> Result<Spanned<Expr>, CompileError> {
        let mut left = self.range_expr()?;

        loop {
            if let Some((active, glyph)) = self.consume_active_scoped_glyph() {
                let right = if active.descriptor.family == incan_vocab::ScopedSurfaceFamily::BindingLike {
                    self.comparison()?
                } else {
                    self.range_expr()?
                };
                let span = left.span.merge(right.span);
                left = self.scoped_glyph_binary_from_active(&active, &glyph, left.clone(), right, span);
                continue;
            }

            let op = if self.match_token(&TokenKind::Operator(OperatorId::PipeForward)) {
                BinaryOp::PipeForward
            } else if self.match_token(&TokenKind::Operator(OperatorId::PipeBackward)) {
                BinaryOp::PipeBackward
            } else if self.match_token(&TokenKind::Operator(OperatorId::EqEq)) {
                BinaryOp::Eq
            } else if self.match_token(&TokenKind::Operator(OperatorId::NotEq)) {
                BinaryOp::NotEq
            } else if self.match_token(&TokenKind::Operator(OperatorId::Lt)) {
                BinaryOp::Lt
            } else if self.match_token(&TokenKind::Operator(OperatorId::Gt)) {
                BinaryOp::Gt
            } else if self.match_token(&TokenKind::Operator(OperatorId::LtEq)) {
                BinaryOp::LtEq
            } else if self.match_token(&TokenKind::Operator(OperatorId::GtEq)) {
                BinaryOp::GtEq
            } else if self.match_token(&TokenKind::Keyword(KeywordId::In)) {
                BinaryOp::In
            } else if self.check_keyword(KeywordId::Not) && self.peek_next().kind == TokenKind::Keyword(KeywordId::In) {
                self.advance(); // not
                self.advance(); // in
                BinaryOp::NotIn
            } else if self.match_token(&TokenKind::Keyword(KeywordId::Is)) {
                if self.match_token(&TokenKind::Keyword(KeywordId::Not)) {
                    BinaryOp::IsNot
                } else {
                    BinaryOp::Is
                }
            } else {
                break;
            };

            let right = self.range_expr()?;
            let span = left.span.merge(right.span);
            let glyph = match op {
                BinaryOp::PipeForward => Some("|>"),
                BinaryOp::PipeBackward => Some("<|"),
                BinaryOp::Eq => Some("=="),
                BinaryOp::NotEq => Some("!="),
                BinaryOp::Lt => Some("<"),
                BinaryOp::Gt => Some(">"),
                BinaryOp::LtEq => Some("<="),
                BinaryOp::GtEq => Some(">="),
                _ => None,
            };
            left = if let Some(glyph) = glyph
                && let Some(surface) = self.scoped_glyph_binary(glyph, left.clone(), right.clone(), span)
            {
                surface
            } else {
                Spanned::new(Expr::Binary(Box::new(left), op, Box::new(right)), span)
            };
        }

        Ok(left)
    }

    /// Parse range expressions: `start..end` or `start..=end`
    fn range_expr(&mut self) -> Result<Spanned<Expr>, CompileError> {
        let left = self.bit_or()?;

        // Check for range operators
        let is_inclusive = if self.match_token(&TokenKind::Operator(OperatorId::DotDotEq)) {
            true
        } else if self.match_token(&TokenKind::Operator(OperatorId::DotDot)) {
            false
        } else {
            return Ok(left);
        };

        let right = self.bit_or()?;
        let span = left.span.merge(right.span);

        Ok(Spanned::new(
            Expr::Range {
                start: Box::new(left),
                end: Box::new(right),
                inclusive: is_inclusive,
            },
            span,
        ))
    }

    /// Parse bitwise-or expressions while preserving scoped vocab ownership of `|`.
    fn bit_or(&mut self) -> Result<Spanned<Expr>, CompileError> {
        let mut left = self.bit_xor()?;

        loop {
            if self.active_scoped_glyph_surface_descriptor("|").is_some() {
                break;
            }
            if !(self.match_token(&TokenKind::Punctuation(PunctuationId::Pipe))
                || self.match_token(&TokenKind::Operator(OperatorId::Pipe)))
            {
                break;
            }

            let right = self.bit_xor()?;
            let span = left.span.merge(right.span);
            left = Spanned::new(Expr::Binary(Box::new(left), BinaryOp::BitOr, Box::new(right)), span);
        }

        Ok(left)
    }

    /// Parse bitwise-xor expressions unless the active vocab scope owns `^`.
    fn bit_xor(&mut self) -> Result<Spanned<Expr>, CompileError> {
        let mut left = self.bit_and()?;

        loop {
            if self.active_scoped_glyph_surface_descriptor("^").is_some()
                || !self.match_token(&TokenKind::Operator(OperatorId::Caret))
            {
                break;
            }

            let right = self.bit_and()?;
            let span = left.span.merge(right.span);
            left = Spanned::new(Expr::Binary(Box::new(left), BinaryOp::BitXor, Box::new(right)), span);
        }

        Ok(left)
    }

    /// Parse bitwise-and expressions unless the active vocab scope owns `&`.
    fn bit_and(&mut self) -> Result<Spanned<Expr>, CompileError> {
        let mut left = self.shift()?;

        loop {
            if self.active_scoped_glyph_surface_descriptor("&").is_some()
                || !self.match_token(&TokenKind::Operator(OperatorId::Amp))
            {
                break;
            }

            let right = self.shift()?;
            let span = left.span.merge(right.span);
            left = Spanned::new(Expr::Binary(Box::new(left), BinaryOp::BitAnd, Box::new(right)), span);
        }

        Ok(left)
    }

    /// Parse bit-shift expressions, deferring to vocab glyph handlers when scoped.
    fn shift(&mut self) -> Result<Spanned<Expr>, CompileError> {
        let mut left = self.additive()?;

        loop {
            let op = if self.active_scoped_glyph_surface_descriptor("<<").is_none()
                && self.match_token(&TokenKind::Operator(OperatorId::Shl))
            {
                BinaryOp::Shl
            } else if self.active_scoped_glyph_surface_descriptor(">>").is_none()
                && self.match_token(&TokenKind::Operator(OperatorId::Shr))
            {
                BinaryOp::Shr
            } else {
                break;
            };

            let right = self.additive()?;
            let span = left.span.merge(right.span);
            left = Spanned::new(Expr::Binary(Box::new(left), op, Box::new(right)), span);
        }

        Ok(left)
    }

    /// Parse additive expressions, preserving DSL-owned `+`/`-` glyphs in eligible vocab blocks.
    fn additive(&mut self) -> Result<Spanned<Expr>, CompileError> {
        let mut left = self.multiplicative()?;

        loop {
            let op = if self.match_token(&TokenKind::Operator(OperatorId::Plus)) {
                BinaryOp::Add
            } else if self.match_token(&TokenKind::Operator(OperatorId::Minus)) {
                BinaryOp::Sub
            } else {
                break;
            };

            let right = self.multiplicative()?;
            let span = left.span.merge(right.span);
            let glyph = match op {
                BinaryOp::Add => "+",
                BinaryOp::Sub => "-",
                _ => unreachable!("additive parser only emits additive operators"),
            };
            left = self
                .scoped_glyph_binary(glyph, left.clone(), right.clone(), span)
                .unwrap_or_else(|| Spanned::new(Expr::Binary(Box::new(left), op, Box::new(right)), span));
        }

        Ok(left)
    }

    /// Parse multiplicative expressions, preserving DSL-owned glyphs in eligible vocab blocks.
    fn multiplicative(&mut self) -> Result<Spanned<Expr>, CompileError> {
        let mut left = self.power()?;

        loop {
            let op = if self.match_token(&TokenKind::Operator(OperatorId::Star)) {
                BinaryOp::Mul
            } else if self.active_scoped_glyph_surface_descriptor("@").is_none()
                && (self.match_token(&TokenKind::Punctuation(PunctuationId::At))
                    || self.match_token(&TokenKind::Operator(OperatorId::MatMul)))
            {
                BinaryOp::MatMul
            } else if self.match_token(&TokenKind::Operator(OperatorId::SlashSlash)) {
                // Check // before / since lexer produces distinct tokens
                BinaryOp::FloorDiv
            } else if self.match_token(&TokenKind::Operator(OperatorId::Slash)) {
                BinaryOp::Div
            } else if self.match_token(&TokenKind::Operator(OperatorId::Percent)) {
                BinaryOp::Mod
            } else {
                break;
            };

            let right = self.power()?;
            let span = left.span.merge(right.span);
            let glyph = match op {
                BinaryOp::Mul => "*",
                BinaryOp::MatMul => "@",
                BinaryOp::FloorDiv => "//",
                BinaryOp::Div => "/",
                BinaryOp::Mod => "%",
                _ => unreachable!("multiplicative parser only emits multiplicative operators"),
            };
            left = self
                .scoped_glyph_binary(glyph, left.clone(), right.clone(), span)
                .unwrap_or_else(|| Spanned::new(Expr::Binary(Box::new(left), op, Box::new(right)), span));
        }

        Ok(left)
    }

    /// Build a scoped-surface binary glyph expression when an active descriptor owns the glyph.
    fn scoped_glyph_binary(
        &self,
        glyph: &str,
        left: Spanned<Expr>,
        right: Spanned<Expr>,
        span: Span,
    ) -> Option<Spanned<Expr>> {
        let active = self.active_scoped_glyph_surface_descriptor(glyph)?;
        Some(self.scoped_glyph_binary_from_active(active, glyph, left, right, span))
    }

    /// Build a scoped-surface binary glyph expression from a descriptor that has already matched the current context.
    fn scoped_glyph_binary_from_active(
        &self,
        active: &ActiveScopedSurfaceDescriptor,
        glyph: &str,
        left: Spanned<Expr>,
        right: Spanned<Expr>,
        span: Span,
    ) -> Spanned<Expr> {
        let owner = self.scoped_surface_owner(active);

        Spanned::new(
            Expr::Surface(Box::new(SurfaceExpr {
                key: SurfaceFeatureKey::ScopedDslSurface {
                    dependency_key: active.dependency_key.clone(),
                    descriptor_key: active.descriptor.key.clone(),
                },
                payload: SurfaceExprPayload::ScopedGlyph {
                    glyph: glyph.to_string(),
                    left: Box::new(left),
                    right: Box::new(right),
                    owner,
                },
            })),
            span,
        )
    }

    fn power(&mut self) -> Result<Spanned<Expr>, CompileError> {
        let mut left = self.unary()?;

        // Right-associative: 2**3**2 = 2**(3**2)
        if self.match_token(&TokenKind::Operator(OperatorId::StarStar)) {
            let right = self.power()?; // recursive for right-associativity
            let span = left.span.merge(right.span);
            left = Spanned::new(Expr::Binary(Box::new(left), BinaryOp::Pow, Box::new(right)), span);
        }

        Ok(left)
    }

    /// Parse prefix unary expressions, including RFC 028 bitwise inversion.
    fn unary(&mut self) -> Result<Spanned<Expr>, CompileError> {
        if self.match_token(&TokenKind::Operator(OperatorId::Minus)) {
            let start = self.tokens[self.pos - 1].span.start;
            let expr = self.unary()?;
            let span = Span::new(start, expr.span.end);
            Ok(Spanned::new(Expr::Unary(UnaryOp::Neg, Box::new(expr)), span))
        } else if self.match_token(&TokenKind::Operator(OperatorId::Tilde)) {
            let start = self.tokens[self.pos - 1].span.start;
            let expr = self.unary()?;
            let span = Span::new(start, expr.span.end);
            Ok(Spanned::new(Expr::Unary(UnaryOp::Invert, Box::new(expr)), span))
        } else if let Some(id) = self.current_surface_keyword(KeywordSurfaceKind::PrefixExpression) {
            self.advance();
            let start = self.tokens[self.pos - 1].span.start;
            let expr = self.unary()?;
            let span = Span::new(start, expr.span.end);
            Ok(Spanned::new(
                Expr::Surface(Box::new(SurfaceExpr {
                    key: SurfaceFeatureKey::SoftKeyword(id),
                    payload: SurfaceExprPayload::PrefixUnary(Box::new(expr)),
                })),
                span,
            ))
        } else {
            self.postfix()
        }
    }

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
    /// This is intentionally conservative: we only treat brackets as call-site type args when the matching `]` is followed immediately by `(`.
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
        if next_idx >= self.tokens.len()
            || self.tokens[next_idx].kind != TokenKind::Punctuation(PunctuationId::LParen)
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

    /// Parse index or slice expression inside brackets
    /// Handles: [expr], [start:end], [start:end:step], [:end], [start:], [::step]
    fn index_or_slice(&mut self) -> Result<IndexOrSlice, CompileError> {
        // Check for immediate colon (slice starting with no start value)
        if self.check(&TokenKind::Punctuation(PunctuationId::Colon)) {
            return self.parse_slice(None);
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

        // Just a regular index
        Ok(IndexOrSlice::Index(first))
    }

    /// Parse slice syntax after optional start expression
    /// start is already parsed, now parse [:end[:step]]
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

    /// Parse primary expressions, including descriptor-enabled leading-dot scoped surfaces.
    fn primary(&mut self) -> Result<Spanned<Expr>, CompileError> {
        let start = self.current_span().start;

        if self.check(&TokenKind::Punctuation(PunctuationId::Dot)) {
            if let Some(expr) = self.try_scoped_leading_dot_path(start)? {
                return Ok(expr);
            }
            if let Some(err) = self.scoped_leading_dot_outside_scope_error(start) {
                return Err(err);
            }
        }

        // Yield expression (for fixtures/generators)
        if self.match_token(&TokenKind::Keyword(KeywordId::Yield)) {
            // yield can be followed by an expression or stand alone
            let end_span = self.tokens[self.pos - 1].span.end;
            if self.is_at_expr_start() {
                let inner = self.expression()?;
                let end = inner.span.end;
                return Ok(Spanned::new(Expr::Yield(Some(Box::new(inner))), Span::new(start, end)));
            } else {
                return Ok(Spanned::new(Expr::Yield(None), Span::new(start, end_span)));
            }
        }

        // Match expression
        if self.match_token(&TokenKind::Keyword(KeywordId::Match)) {
            return self.match_expr(start);
        }

        // If expression (when used as expression)
        if self.check_keyword(KeywordId::If) {
            return self.if_expr(start);
        }

        // Loop expression
        if self.check_keyword(KeywordId::Loop) {
            return self.loop_expr(start);
        }

        if self.is_active_race_for_expr() {
            return self.race_for_expr(start);
        }

        if let Some(expr) = self.try_vocab_block_expression(start)? {
            return Ok(expr);
        }

        // self
        if self.match_token(&TokenKind::Keyword(KeywordId::SelfKw)) {
            let end = self.tokens[self.pos - 1].span.end;
            return Ok(Spanned::new(Expr::SelfExpr, Span::new(start, end)));
        }

        // Local partial callable preset expression.
        if self.match_ident_text("partial") {
            return self.partial_expr(start);
        }

        // Literals
        if let Some(lit) = self.try_literal() {
            let end = self.tokens[self.pos - 1].span.end;
            return Ok(Spanned::new(Expr::Literal(lit), Span::new(start, end)));
        }

        // f-string
        if let TokenKind::FString(parts) = &self.peek().kind {
            let parts = parts.clone();
            self.advance();
            let fparts = self.convert_fstring_parts(&parts);
            let end = self.tokens[self.pos - 1].span.end;
            return Ok(Spanned::new(Expr::FString(fparts), Span::new(start, end)));
        }

        // List literal or comprehension
        if self.match_token(&TokenKind::Punctuation(PunctuationId::LBracket)) {
            return self.list_or_comp(start);
        }

        // Dict literal or comprehension
        if self.match_token(&TokenKind::Punctuation(PunctuationId::LBrace)) {
            return self.dict_or_comp(start);
        }

        // Parenthesized expression or tuple
        if self.match_token(&TokenKind::Punctuation(PunctuationId::LParen)) {
            return self.paren_or_tuple(start);
        }

        // Identifier (or constructor)
        if let TokenKind::Ident(name) = &self.peek().kind {
            let name = name.clone();
            self.advance();

            // Check if it's a constructor call (identifier followed by parentheses with named args)
            // This is tricky - we'll let the type checker figure it out
            let end = self.tokens[self.pos - 1].span.end;
            return Ok(Spanned::new(Expr::Ident(name), Span::new(start, end)));
        }

        Err(errors::expected_expression(
            &format!("{:?}", self.peek().kind),
            self.current_span(),
        ))
    }

    /// Parse a metadata-declared vocab block in expression position.
    ///
    /// Only declarations whose rich DSL surface says `desugars_to_expression()` are accepted here. The low-level
    /// keyword activation still supplies the parser entrypoint, while the rich declaration metadata decides whether the
    /// raw block can occupy a value position.
    fn try_vocab_block_expression(&mut self, start: usize) -> Result<Option<Spanned<Expr>>, CompileError> {
        let Some(keyword_name) = self.current_vocab_keyword_name() else {
            return Ok(None);
        };
        let parent_keyword = self.vocab_block_stack.last().cloned();
        let Some(spec) = self
            .find_active_vocab_block_spec(&keyword_name, parent_keyword.as_deref())
            .cloned()
        else {
            return Ok(None);
        };
        if spec.desugar_target != incan_vocab::DesugarTarget::Expression {
            return Ok(None);
        }
        if !self.has_top_level_colon_before_statement_end(self.pos + 1)
            && !self.vocab_expression_block_has_brace_delimiter(&spec)
        {
            return Ok(None);
        }

        let block = self.parse_vocab_expression_block(keyword_name, spec)?;
        let end = self.tokens[self.pos.saturating_sub(1)].span.end;
        Ok(Some(Spanned::new(
            Expr::VocabBlock(Box::new(block)),
            Span::new(start, end),
        )))
    }

    /// Return whether a vocab expression declaration has a top-level `{` delimiter after its optional header args.
    fn vocab_expression_block_has_brace_delimiter(&self, spec: &ActiveImportedKeywordSpec) -> bool {
        let mut idx = self.pos + 1 + spec.compound_tokens.len();
        let mut paren_depth = 0usize;
        let mut bracket_depth = 0usize;
        let mut brace_depth = 0usize;

        while let Some(token) = self.tokens.get(idx) {
            match token.kind {
                TokenKind::Punctuation(PunctuationId::LParen) => paren_depth += 1,
                TokenKind::Punctuation(PunctuationId::RParen) => {
                    paren_depth = paren_depth.saturating_sub(1);
                }
                TokenKind::Punctuation(PunctuationId::LBracket) => bracket_depth += 1,
                TokenKind::Punctuation(PunctuationId::RBracket) => {
                    bracket_depth = bracket_depth.saturating_sub(1);
                }
                TokenKind::Punctuation(PunctuationId::LBrace)
                    if paren_depth == 0 && bracket_depth == 0 && brace_depth == 0 =>
                {
                    return true;
                }
                TokenKind::Punctuation(PunctuationId::LBrace) => brace_depth += 1,
                TokenKind::Punctuation(PunctuationId::RBrace) => {
                    brace_depth = brace_depth.saturating_sub(1);
                }
                TokenKind::Newline | TokenKind::Dedent | TokenKind::Eof
                    if paren_depth == 0 && bracket_depth == 0 && brace_depth == 0 =>
                {
                    return false;
                }
                _ => {}
            }
            idx += 1;
        }

        false
    }

    /// Parse the raw block carrier shared by expression-position vocab declarations.
    fn parse_vocab_expression_block(
        &mut self,
        keyword_name: String,
        spec: ActiveImportedKeywordSpec,
    ) -> Result<VocabBlockStmt, CompileError> {
        let spec_compound_tokens = spec.compound_tokens.clone();
        self.advance();
        self.consume_vocab_compound_tokens(&spec_compound_tokens)?;

        let mut header_args = Vec::new();
        while !self.check_punct(PunctuationId::Colon) && !self.check_punct(PunctuationId::LBrace) {
            header_args.push(self.expression()?);
            if !self.match_punct(PunctuationId::Comma) {
                break;
            }
        }

        let (body, body_item_trailing_commas) = if self.match_punct(PunctuationId::Colon) {
            self.expect(&TokenKind::Newline, "Expected newline after ':'")?;
            self.expect_suite_indent("Expected indented block after vocab keyword")?;
            let body = self.parse_scoped_vocab_indented_body(
                &keyword_name,
                spec.clause_body_kind,
                spec.expression_item_modifiers.clone(),
            )?;
            self.expect(&TokenKind::Dedent, "Expected dedent after vocab block body")?;
            let body_item_trailing_commas = vec![false; body.len()];
            (body, body_item_trailing_commas)
        } else if self.match_punct(PunctuationId::LBrace) {
            self.parse_scoped_vocab_braced_body(
                &keyword_name,
                spec.clause_body_kind,
                spec.expression_item_modifiers.clone(),
            )?
        } else {
            return Err(errors::expected_token_message(
                "Expected ':' or '{' after vocab expression declaration header",
                &format!("{:?}", self.peek().kind),
                self.current_span(),
            ));
        };

        Ok(VocabBlockStmt {
            keyword: keyword_name,
            keyword_binding: VocabKeywordBinding {
                dependency_key: spec.dependency_key,
                activation_namespace: spec.activation_namespace,
                surface_kind: spec.surface_kind,
                compound_tokens: spec_compound_tokens,
                placement: spec.placement,
                clause_body_kind: spec.clause_body_kind,
            },
            decorators: Vec::new(),
            header_args,
            body,
            body_item_trailing_commas,
        })
    }

    /// Parse an indentation-delimited vocab body with the same scoped context used by statement vocab blocks.
    fn parse_scoped_vocab_indented_body(
        &mut self,
        keyword_name: &str,
        clause_body_kind: Option<incan_vocab::ClauseBodyKind>,
        expression_item_modifiers: Vec<incan_vocab::ExpressionItemModifierSurface>,
    ) -> Result<Vec<Spanned<Statement>>, CompileError> {
        self.vocab_block_stack.push(keyword_name.to_string());
        self.vocab_body_kind_stack.push(clause_body_kind);
        self.vocab_expression_item_modifier_stack
            .push(expression_item_modifiers);
        let body = self.block();
        self.vocab_block_stack.pop();
        self.vocab_body_kind_stack.pop();
        self.vocab_expression_item_modifier_stack.pop();
        body
    }

    /// Parse a brace-delimited vocab body.
    ///
    /// Braced vocab syntax does not receive lexer newline/indent tokens, so clause boundaries are recognized from the
    /// active child keyword metadata for the owning declaration rather than from source line breaks.
    fn parse_scoped_vocab_braced_body(
        &mut self,
        keyword_name: &str,
        clause_body_kind: Option<incan_vocab::ClauseBodyKind>,
        expression_item_modifiers: Vec<incan_vocab::ExpressionItemModifierSurface>,
    ) -> Result<(Vec<Spanned<Statement>>, Vec<bool>), CompileError> {
        self.vocab_block_stack.push(keyword_name.to_string());
        self.vocab_body_kind_stack.push(clause_body_kind);
        self.vocab_expression_item_modifier_stack
            .push(expression_item_modifiers);
        let body = self.braced_vocab_body(None);
        self.vocab_block_stack.pop();
        self.vocab_body_kind_stack.pop();
        self.vocab_expression_item_modifier_stack.pop();
        body
    }

    /// Parse braced body items until the matching `}`.
    fn braced_vocab_body(
        &mut self,
        sibling_parent_keyword: Option<String>,
    ) -> Result<(Vec<Spanned<Statement>>, Vec<bool>), CompileError> {
        let mut body = Vec::new();
        let mut body_item_trailing_commas = Vec::new();
        self.skip_newlines();
        while !self.check_punct(PunctuationId::RBrace) && !self.is_at_end() {
            if self.braced_vocab_body_boundary(sibling_parent_keyword.as_deref()) {
                break;
            }
            if let Some(child) = self.try_braced_vocab_child()? {
                body.push(child);
            } else if self.vocab_expression_list_items_enabled() {
                body.push(self.braced_vocab_expression_list_item()?);
            } else {
                let start = self.current_span().start;
                let stmt = self.assignment_or_expr_stmt()?;
                let end = self.tokens[self.pos.saturating_sub(1)].span.end;
                body.push(Spanned::new(stmt, Span::new(start, end)));
            }
            self.skip_newlines();
            body_item_trailing_commas.push(self.match_punct(PunctuationId::Comma));
            self.skip_newlines();
        }
        if sibling_parent_keyword.is_none() {
            self.expect_punct(PunctuationId::RBrace, "Expected '}' after vocab expression block")?;
        }
        Ok((body, body_item_trailing_commas))
    }

    /// Parse one child clause/declaration inside a braced vocab body when metadata says the current token starts one.
    fn try_braced_vocab_child(&mut self) -> Result<Option<Spanned<Statement>>, CompileError> {
        let parent_keyword = self.vocab_block_stack.last().cloned();
        let Some(keyword_name) = self.current_vocab_keyword_name() else {
            return Ok(None);
        };
        let Some(spec) = self
            .find_active_vocab_block_spec(&keyword_name, parent_keyword.as_deref())
            .cloned()
        else {
            return Ok(None);
        };
        let start = self.current_span().start;
        let block = self.parse_braced_vocab_child_from_spec(keyword_name, spec, parent_keyword)?;
        let end = self.tokens[self.pos.saturating_sub(1)].span.end;
        Ok(Some(Spanned::new(
            Statement::VocabBlock(block),
            Span::new(start, end),
        )))
    }

    /// Parse one metadata-selected child item in braced vocab syntax.
    fn parse_braced_vocab_child_from_spec(
        &mut self,
        keyword_name: String,
        spec: ActiveImportedKeywordSpec,
        sibling_parent_keyword: Option<String>,
    ) -> Result<VocabBlockStmt, CompileError> {
        let spec_compound_tokens = spec.compound_tokens.clone();
        self.advance();
        self.consume_vocab_compound_tokens(&spec_compound_tokens)?;

        let (body, body_item_trailing_commas) = if self.match_punct(PunctuationId::Colon) {
            self.expect(&TokenKind::Newline, "Expected newline after ':'")?;
            self.expect_suite_indent("Expected indented block after vocab keyword")?;
            let body = self.parse_scoped_vocab_indented_body(
                &keyword_name,
                spec.clause_body_kind,
                spec.expression_item_modifiers.clone(),
            )?;
            self.expect(&TokenKind::Dedent, "Expected dedent after vocab block body")?;
            let body_item_trailing_commas = vec![false; body.len()];
            (body, body_item_trailing_commas)
        } else if self.match_punct(PunctuationId::LBrace) {
            self.parse_scoped_vocab_braced_body(
                &keyword_name,
                spec.clause_body_kind,
                spec.expression_item_modifiers.clone(),
            )?
        } else {
            self.parse_braced_vocab_inline_body(
                &keyword_name,
                spec.clause_body_kind,
                spec.expression_item_modifiers.clone(),
                sibling_parent_keyword,
            )?
        };

        Ok(VocabBlockStmt {
            keyword: keyword_name,
            keyword_binding: VocabKeywordBinding {
                dependency_key: spec.dependency_key,
                activation_namespace: spec.activation_namespace,
                surface_kind: spec.surface_kind,
                compound_tokens: spec_compound_tokens,
                placement: spec.placement,
                clause_body_kind: spec.clause_body_kind,
            },
            decorators: Vec::new(),
            header_args: Vec::new(),
            body,
            body_item_trailing_commas,
        })
    }

    /// Parse the inline body after a braced child keyword, e.g. `FROM orders` or `SELECT amount as total`.
    fn parse_braced_vocab_inline_body(
        &mut self,
        keyword_name: &str,
        clause_body_kind: Option<incan_vocab::ClauseBodyKind>,
        expression_item_modifiers: Vec<incan_vocab::ExpressionItemModifierSurface>,
        sibling_parent_keyword: Option<String>,
    ) -> Result<(Vec<Spanned<Statement>>, Vec<bool>), CompileError> {
        self.vocab_block_stack.push(keyword_name.to_string());
        self.vocab_body_kind_stack.push(clause_body_kind);
        self.vocab_expression_item_modifier_stack
            .push(expression_item_modifiers);

        let body = match clause_body_kind {
            Some(incan_vocab::ClauseBodyKind::ExpressionList) => {
                self.braced_expression_items_until_boundary(sibling_parent_keyword.as_deref())
            }
            Some(incan_vocab::ClauseBodyKind::Expression) | None => {
                self.braced_single_expression_until_boundary(sibling_parent_keyword.as_deref())
            }
            _ => self.braced_vocab_body(sibling_parent_keyword),
        };

        self.vocab_block_stack.pop();
        self.vocab_body_kind_stack.pop();
        self.vocab_expression_item_modifier_stack.pop();
        body
    }

    /// Parse one expression clause body in braced syntax.
    fn braced_single_expression_until_boundary(
        &mut self,
        sibling_parent_keyword: Option<&str>,
    ) -> Result<(Vec<Spanned<Statement>>, Vec<bool>), CompileError> {
        if self.braced_vocab_body_boundary(sibling_parent_keyword) {
            return Ok((Vec::new(), Vec::new()));
        }
        let start = self.current_span().start;
        let expr = self.expression()?;
        let end = expr.span.end;
        Ok((
            vec![Spanned::new(Statement::Expr(expr), Span::new(start, end))],
            vec![false],
        ))
    }

    /// Parse expression-list entries in braced syntax until the next sibling clause/declaration.
    fn braced_expression_items_until_boundary(
        &mut self,
        sibling_parent_keyword: Option<&str>,
    ) -> Result<(Vec<Spanned<Statement>>, Vec<bool>), CompileError> {
        let mut statements = Vec::new();
        let mut trailing_commas = Vec::new();
        while !self.braced_vocab_body_boundary(sibling_parent_keyword) {
            statements.push(self.braced_vocab_expression_list_item()?);
            self.skip_newlines();
            let had_comma = self.match_punct(PunctuationId::Comma);
            trailing_commas.push(had_comma);
            if !had_comma && self.braced_vocab_body_boundary(sibling_parent_keyword) {
                break;
            }
            self.skip_newlines();
        }
        Ok((statements, trailing_commas))
    }

    /// Parse one expression-list item in braced syntax.
    fn braced_vocab_expression_list_item(&mut self) -> Result<Spanned<Statement>, CompileError> {
        let start = self.current_span().start;
        let expr = self.expression()?;
        let statement = if let Some(item) = self.vocab_expression_list_item_tail(expr.clone())? {
            Statement::VocabExpressionItem(item)
        } else {
            Statement::Expr(expr)
        };
        let end = self.tokens[self.pos.saturating_sub(1)].span.end;
        Ok(Spanned::new(statement, Span::new(start, end)))
    }

    /// Return whether braced parsing reached a terminator for the current inline body.
    fn braced_vocab_body_boundary(&self, sibling_parent_keyword: Option<&str>) -> bool {
        self.check_punct(PunctuationId::RBrace)
            || self.is_at_end()
            || sibling_parent_keyword
                .is_some_and(|parent| self.current_starts_vocab_block_for_parent(Some(parent)))
    }

    /// Parse `partial Target(name=value)` after the `partial` marker has already been consumed.
    fn partial_expr(&mut self, start: usize) -> Result<Spanned<Expr>, CompileError> {
        let template = self.postfix()?;
        let end = template.span.end;
        let partial = self.partial_expr_from_call_template(template)?;
        Ok(Spanned::new(Expr::Partial(Box::new(partial)), Span::new(start, end)))
    }

    /// Split the final call in a local partial template into a callable target plus keyword presets.
    fn partial_expr_from_call_template(&mut self, template: Spanned<Expr>) -> Result<PartialExpr, CompileError> {
        match template.node {
            Expr::Call(target, type_args, args) => Ok(PartialExpr {
                target,
                type_args,
                args: self.partial_args_from_call_args(args, template.span)?,
            }),
            Expr::MethodCall(receiver, method, type_args, args) => {
                let target_span = Span::new(receiver.span.start, template.span.end);
                Ok(PartialExpr {
                    target: Box::new(Spanned::new(Expr::Field(receiver, method), target_span)),
                    type_args,
                    args: self.partial_args_from_call_args(args, template.span)?,
                })
            }
            _ => Err(CompileError::syntax(
                "Expected partial target call template `Target(name=value)`".to_string(),
                template.span,
            )),
        }
    }

    /// Parse a descriptor-enabled leading-dot path if the current DSL block accepts one.
    fn try_scoped_leading_dot_path(&mut self, start: usize) -> Result<Option<Spanned<Expr>>, CompileError> {
        let Some(active) = self.active_leading_dot_surface_descriptor() else {
            return Ok(None);
        };
        let dependency_key = active.dependency_key.clone();
        let descriptor_key = active.descriptor.key.clone();
        let receiver = active
            .descriptor
            .receiver
            .clone()
            .unwrap_or(incan_vocab::ScopedSurfaceReceiver::OwningDeclaration);
        let owner = self.scoped_surface_owner(active);
        let (min_segments, max_segments) = match &active.descriptor.syntax {
            incan_vocab::ScopedSurfaceSyntax::LeadingDotPath {
                min_segments,
                max_segments,
            } => (*min_segments as usize, max_segments.map(usize::from)),
            _ => return Ok(None),
        };

        let mut segments = Vec::new();
        loop {
            self.expect_punct(PunctuationId::Dot, "Expected '.' at start of scoped leading-dot path")?;
            segments.push(self.identifier_or_any_keyword()?);
            if max_segments.is_some_and(|max_segments| segments.len() >= max_segments) {
                break;
            }
            if !self.check(&TokenKind::Punctuation(PunctuationId::Dot)) {
                break;
            }
            if !matches!(self.peek_next().kind, TokenKind::Ident(_) | TokenKind::Keyword(_)) {
                break;
            }
        }

        if segments.len() < min_segments {
            return Err(errors::expected_expression(
                "scoped leading-dot path",
                Span::new(start, self.current_span().end),
            ));
        }

        let end = self.tokens[self.pos.saturating_sub(1)].span.end;
        Ok(Some(Spanned::new(
            Expr::Surface(Box::new(SurfaceExpr {
                key: SurfaceFeatureKey::ScopedDslSurface {
                    dependency_key,
                    descriptor_key,
                },
                payload: SurfaceExprPayload::LeadingDotPath {
                    segments,
                    receiver,
                    owner,
                },
            })),
            Span::new(start, end),
        )))
    }

    /// Return the first active leading-dot descriptor accepted by the current scoped context.
    fn active_leading_dot_surface_descriptor(&self) -> Option<&ActiveScopedSurfaceDescriptor> {
        self.active_scoped_surface_descriptors.iter().find(|active| {
            active.descriptor.family == incan_vocab::ScopedSurfaceFamily::ExpressionForm
                && matches!(
                    active.descriptor.syntax,
                    incan_vocab::ScopedSurfaceSyntax::LeadingDotPath { .. }
                )
                && active.descriptor.eligible_in.iter().any(|eligibility| {
                    self.scoped_surface_eligibility_accepts_current_context(eligibility)
                })
        })
    }

    /// Build an author-provided outside-scope diagnostic for an active leading-dot descriptor.
    fn scoped_leading_dot_outside_scope_error(&self, start: usize) -> Option<CompileError> {
        if !matches!(self.peek_next().kind, TokenKind::Ident(_) | TokenKind::Keyword(_)) {
            return None;
        }
        let active = self.active_scoped_surface_descriptors.iter().find(|active| {
            active.descriptor.family == incan_vocab::ScopedSurfaceFamily::ExpressionForm
                && !matches!(
                    active.descriptor.misuse_scope,
                    incan_vocab::ScopedSurfaceMisuseScope::None
                )
                && matches!(
                    active.descriptor.syntax,
                    incan_vocab::ScopedSurfaceSyntax::LeadingDotPath { .. }
                )
        })?;
        let diagnostic = active.descriptor.diagnostics.iter().find(|diagnostic| {
            diagnostic.kind == incan_vocab::ScopedSurfaceDiagnosticKind::OutsideScope
        });
        let message = diagnostic
            .map(|diagnostic| diagnostic.message.clone())
            .unwrap_or_else(|| {
                format!(
                    "Scoped surface `{}` is not valid in this position",
                    active.descriptor.key
                )
            });
        let mut error = CompileError::syntax(message, Span::new(start, self.peek_next().span.end));
        if let Some(code) = diagnostic.map(|diagnostic| diagnostic.code.as_str()).filter(|code| !code.is_empty()) {
            error = error.with_note(format!("diagnostic code: {code}"));
        }
        if let Some(help) = diagnostic.and_then(|diagnostic| diagnostic.help.as_deref()) {
            error = error.with_hint(help);
        }
        Some(error)
    }

    /// Return the first active operator-like or binding-like glyph descriptor accepted by the current context.
    fn active_scoped_glyph_surface_descriptor(&self, glyph: &str) -> Option<&ActiveScopedSurfaceDescriptor> {
        self.active_scoped_surface_descriptors.iter().find(|active| {
            matches!(
                active.descriptor.family,
                incan_vocab::ScopedSurfaceFamily::OperatorLike | incan_vocab::ScopedSurfaceFamily::BindingLike
            )
                && matches!(
                    &active.descriptor.syntax,
                    incan_vocab::ScopedSurfaceSyntax::Glyph { spelling } if spelling == glyph
                )
                && active.descriptor.eligible_in.iter().any(|eligibility| {
                    self.scoped_surface_eligibility_accepts_current_context(eligibility)
                })
        })
    }

    /// Return the innermost active scoped-symbol descriptor accepted by the current context.
    fn active_scoped_symbol_descriptor(
        &self,
        symbol: &str,
        span: Span,
    ) -> Result<Option<&ActiveScopedSymbolDescriptor>, CompileError> {
        let matches: Vec<_> = self
            .active_scoped_symbol_descriptors
            .iter()
            .filter(|active| {
                active.descriptor.symbol == symbol
                    && active
                        .descriptor
                        .eligible_in
                        .iter()
                        .any(|eligibility| self.scoped_symbol_eligibility_accepts_current_context(eligibility))
            })
            .collect();
        let Some(max_depth) = matches
            .iter()
            .map(|active| self.scoped_symbol_owner_depth(active))
            .max()
        else {
            return Ok(None);
        };
        let mut best = matches
            .into_iter()
            .filter(|active| self.scoped_symbol_owner_depth(active) == max_depth);
        let first = best.next();
        if best.next().is_some() {
            return Err(CompileError::syntax(
                format!("Ambiguous scoped symbol `{symbol}` in this DSL position"),
                span,
            )
            .with_hint("Use explicit qualification or adjust the active DSL symbol descriptors"));
        }
        Ok(first)
    }

    /// Build a descriptor-owned diagnostic for a scoped symbol used inside its active DSL but outside an eligible position.
    fn scoped_symbol_misuse_error(
        &self,
        symbol: &str,
        span: Span,
    ) -> Result<Option<CompileError>, CompileError> {
        let matches: Vec<_> = self
            .active_scoped_symbol_descriptors
            .iter()
            .filter(|active| {
                active.descriptor.symbol == symbol
                    && active.descriptor.misuse_scope == incan_vocab::ScopedSymbolMisuseScope::ActiveDsl
                    && active
                        .descriptor
                        .eligible_in
                        .iter()
                        .any(|eligibility| self.scoped_symbol_eligibility_is_inside_owning_dsl(eligibility))
            })
            .collect();
        let Some(max_depth) = matches
            .iter()
            .filter_map(|active| self.scoped_symbol_owner_declaration_depth(active))
            .max()
        else {
            return Ok(None);
        };
        let mut best = matches
            .into_iter()
            .filter(|active| self.scoped_symbol_owner_declaration_depth(active) == Some(max_depth));
        let Some(active) = best.next() else {
            return Ok(None);
        };
        if best.next().is_some() {
            return Err(CompileError::syntax(
                format!("Ambiguous scoped symbol `{symbol}` in this DSL position"),
                span,
            )
            .with_hint("Use explicit qualification or adjust the active DSL symbol descriptors"));
        }

        let diagnostic = active.descriptor.diagnostics.iter().find(|diagnostic| {
            diagnostic.kind == incan_vocab::ScopedSymbolDiagnosticKind::OutsideEligiblePosition
        });
        let message = diagnostic
            .map(|diagnostic| diagnostic.message.clone())
            .unwrap_or_else(|| {
                format!(
                    "Scoped symbol `{}` is not valid in this DSL position",
                    active.descriptor.symbol
                )
            });
        let mut error = CompileError::syntax(message, span);
        if let Some(code) = diagnostic.map(|diagnostic| diagnostic.code.as_str()).filter(|code| !code.is_empty()) {
            error = error.with_note(format!("diagnostic code: {code}"));
        }
        if let Some(help) = diagnostic.and_then(|diagnostic| diagnostic.help.as_deref()) {
            error = error.with_hint(help);
        }
        Ok(Some(error))
    }

    /// Consume the longest active scoped glyph at the current token position.
    fn consume_active_scoped_glyph(&mut self) -> Option<(ActiveScopedSurfaceDescriptor, String)> {
        let mut best: Option<(ActiveScopedSurfaceDescriptor, String, usize)> = None;

        for active in &self.active_scoped_surface_descriptors {
            if !matches!(
                active.descriptor.family,
                incan_vocab::ScopedSurfaceFamily::OperatorLike | incan_vocab::ScopedSurfaceFamily::BindingLike
            ) {
                continue;
            }
            if !active
                .descriptor
                .eligible_in
                .iter()
                .any(|eligibility| self.scoped_surface_eligibility_accepts_current_context(eligibility))
            {
                continue;
            }
            let incan_vocab::ScopedSurfaceSyntax::Glyph { spelling } = &active.descriptor.syntax else {
                continue;
            };
            let Some(token_count) = self.scoped_glyph_token_count_at(spelling, self.pos) else {
                continue;
            };

            let should_replace = best
                .as_ref()
                .map(|(_, current_spelling, current_count)| {
                    token_count > *current_count
                        || (token_count == *current_count && spelling.len() > current_spelling.len())
                })
                .unwrap_or(true);
            if should_replace {
                best = Some((active.clone(), spelling.clone(), token_count));
            }
        }

        let (active, spelling, token_count) = best?;
        for _ in 0..token_count {
            self.advance();
        }
        Some((active, spelling))
    }

    /// Return whether an active scoped glyph starts `offset` tokens after the current parser position.
    fn active_scoped_glyph_starts_at_offset(&self, offset: usize) -> bool {
        let pos = self.pos.saturating_add(offset);
        self.active_scoped_surface_descriptors.iter().any(|active| {
            matches!(
                active.descriptor.family,
                incan_vocab::ScopedSurfaceFamily::OperatorLike | incan_vocab::ScopedSurfaceFamily::BindingLike
            ) && active
                .descriptor
                .eligible_in
                .iter()
                .any(|eligibility| self.scoped_surface_eligibility_accepts_current_context(eligibility))
                && matches!(
                    &active.descriptor.syntax,
                    incan_vocab::ScopedSurfaceSyntax::Glyph { spelling }
                        if self.scoped_glyph_token_count_at(spelling, pos).is_some()
                )
        })
    }

    /// Return the number of tokens that compose `spelling` at `pos`, if the token spellings match exactly.
    fn scoped_glyph_token_count_at(&self, spelling: &str, pos: usize) -> Option<usize> {
        let mut matched = String::new();
        let mut idx = pos;

        while matched.len() < spelling.len() {
            let piece = self.token_symbol_spelling(idx)?;
            matched.push_str(piece);
            if !spelling.starts_with(&matched) {
                return None;
            }
            idx += 1;
        }

        if matched == spelling {
            Some(idx - pos)
        } else {
            None
        }
    }

    /// Return the source spelling for an operator or punctuation token at `idx`.
    fn token_symbol_spelling(&self, idx: usize) -> Option<&'static str> {
        match self.tokens.get(idx).map(|token| &token.kind)? {
            TokenKind::Operator(id) => Some(incan_core::lang::operators::info_for(*id).spellings[0]),
            TokenKind::Punctuation(id) => Some(incan_core::lang::punctuation::as_str(*id)),
            _ => None,
        }
    }

    /// Return whether a scoped-surface eligibility matches the parser's current scoped context.
    fn scoped_surface_eligibility_accepts_current_context(
        &self,
        eligibility: &incan_vocab::ScopedSurfaceEligibility,
    ) -> bool {
        match eligibility.position {
            incan_vocab::ScopedSurfacePosition::DeclarationBody => {
                self.vocab_block_stack.last() == Some(&eligibility.declaration)
            }
            incan_vocab::ScopedSurfacePosition::ClauseBody => self.current_vocab_clause_matches(
                eligibility.declaration.as_str(),
                eligibility.clause.as_deref(),
            ),
            incan_vocab::ScopedSurfacePosition::CallArgument => self
                .scoped_call_argument_stack
                .last()
                .is_some_and(|context| eligibility.call.as_deref() == Some(context.call.as_str())),
            incan_vocab::ScopedSurfacePosition::DeclarationHead => false,
            _ => false,
        }
    }

    /// Return whether a scoped-symbol eligibility matches the parser's current scoped context.
    fn scoped_symbol_eligibility_accepts_current_context(
        &self,
        eligibility: &incan_vocab::ScopedSymbolEligibility,
    ) -> bool {
        match eligibility.position {
            incan_vocab::ScopedSymbolPosition::DeclarationBody => {
                self.vocab_block_stack.last() == Some(&eligibility.declaration)
            }
            incan_vocab::ScopedSymbolPosition::ClauseBody => self.current_vocab_clause_matches(
                eligibility.declaration.as_str(),
                eligibility.clause.as_deref(),
            ),
            incan_vocab::ScopedSymbolPosition::CallArgument => self
                .scoped_call_argument_stack
                .last()
                .is_some_and(|context| eligibility.call.as_deref() == Some(context.call.as_str()))
                && self.vocab_block_stack.iter().any(|declaration| declaration == &eligibility.declaration),
            _ => false,
        }
    }

    /// Return whether the current parser position is inside the owning DSL declaration for an eligibility rule.
    fn scoped_symbol_eligibility_is_inside_owning_dsl(
        &self,
        eligibility: &incan_vocab::ScopedSymbolEligibility,
    ) -> bool {
        self.vocab_block_stack
            .iter()
            .any(|declaration| declaration == &eligibility.declaration)
    }

    /// Return whether the current vocab stack is positioned inside a clause-like nested vocab block.
    fn current_vocab_clause_matches(&self, declaration: &str, clause: Option<&str>) -> bool {
        let Some(clause) = clause else {
            return false;
        };
        let Some(clause_depth) = self
            .vocab_block_stack
            .iter()
            .rposition(|active| active == clause)
        else {
            return false;
        };
        self.vocab_block_stack
            .iter()
            .take(clause_depth)
            .any(|active| active == declaration)
    }

    /// Build the owner metadata attached to a scoped-surface AST payload.
    fn scoped_surface_owner(&self, active: &ActiveScopedSurfaceDescriptor) -> ScopedSurfaceOwner {
        active
            .descriptor
            .eligible_in
            .iter()
            .find(|eligibility| self.scoped_surface_eligibility_accepts_current_context(eligibility))
            .map(|eligibility| ScopedSurfaceOwner {
                declaration: eligibility.declaration.clone(),
                clause: eligibility.clause.clone(),
                call: eligibility.call.clone(),
            })
            .unwrap_or_else(|| ScopedSurfaceOwner {
                declaration: self.vocab_block_stack.last().cloned().unwrap_or_default(),
                clause: None,
                call: self
                    .scoped_call_argument_stack
                    .last()
                    .map(|context| context.call.clone()),
            })
    }

    /// Build the owner metadata attached to a scoped-symbol AST payload.
    fn scoped_symbol_owner(&self, active: &ActiveScopedSymbolDescriptor) -> ScopedSurfaceOwner {
        active
            .descriptor
            .eligible_in
            .iter()
            .find(|eligibility| self.scoped_symbol_eligibility_accepts_current_context(eligibility))
            .map(|eligibility| ScopedSurfaceOwner {
                declaration: eligibility.declaration.clone(),
                clause: eligibility.clause.clone(),
                call: eligibility.call.clone(),
            })
            .unwrap_or_else(|| ScopedSurfaceOwner {
                declaration: self.vocab_block_stack.last().cloned().unwrap_or_default(),
                clause: None,
                call: self
                    .scoped_call_argument_stack
                    .last()
                    .map(|context| context.call.clone()),
            })
    }

    /// Return a lexical-depth proxy for nested scoped-symbol ownership.
    fn scoped_symbol_owner_depth(&self, active: &ActiveScopedSymbolDescriptor) -> usize {
        self.scoped_symbol_owner_declaration_depth(active).unwrap_or(0)
    }

    /// Return the innermost active declaration depth that can own this scoped symbol.
    fn scoped_symbol_owner_declaration_depth(&self, active: &ActiveScopedSymbolDescriptor) -> Option<usize> {
        active
            .descriptor
            .eligible_in
            .iter()
            .filter_map(|eligibility| {
                self.vocab_block_stack
                    .iter()
                    .rposition(|declaration| declaration == &eligibility.declaration)
            })
            .max()
    }

    /// Return the function or method name whose argument list is about to be parsed.
    fn call_argument_target(&self, expr: &Spanned<Expr>) -> Option<String> {
        match &expr.node {
            Expr::Ident(name) | Expr::Field(_, name) => Some(name.clone()),
            _ => None,
        }
    }

    /// Parse a literal token at the current parser position, if one is present.
    fn try_literal(&mut self) -> Option<Literal> {
        match &self.peek().kind {
            TokenKind::Int(il) => {
                let il = il.clone();
                self.advance();
                Some(Literal::Int(il))
            }
            TokenKind::Float(fl) => {
                let fl = fl.clone();
                self.advance();
                Some(Literal::Float(fl))
            }
            TokenKind::Decimal(dl) => {
                let dl = dl.clone();
                self.advance();
                Some(Literal::Decimal(dl))
            }
            TokenKind::String(s) => {
                let s = s.clone();
                self.advance();
                Some(Literal::String(s))
            }
            TokenKind::Bytes(b) => {
                let b = b.clone();
                self.advance();
                Some(Literal::Bytes(b))
            }
            TokenKind::Keyword(KeywordId::True) => {
                self.advance();
                Some(Literal::Bool(true))
            }
            TokenKind::Keyword(KeywordId::False) => {
                self.advance();
                Some(Literal::Bool(false))
            }
            TokenKind::Keyword(KeywordId::None) => {
                self.advance();
                Some(Literal::None)
            }
            _ => None,
        }
    }

    /// Convert lexer f-string segments into parsed AST parts while preserving interpolation spans and format markers.
    fn convert_fstring_parts(&self, parts: &[LexFStringPart]) -> Vec<FStringPart> {
        parts
            .iter()
            .map(|p| match p {
                LexFStringPart::Literal(s) => FStringPart::Literal(s.clone()),
                LexFStringPart::Expr { text, offset } => {
                    let expr_span = Span::new(*offset, offset + text.len() + 2);
                    let (expr_text, format) = split_fstring_format(text);
                    let mut expr = self.parse_fstring_expr(expr_text);
                    self.shift_expr_spans(&mut expr, offset + 1);
                    FStringPart::Expr {
                        expr: Spanned::new(expr, expr_span),
                        format,
                    }
                }
            })
            .collect()
    }

    fn shift_spanned_expr(&self, expr: &mut Spanned<Expr>, offset: usize) {
        expr.span = Span::new(expr.span.start + offset, expr.span.end + offset);
        self.shift_expr_spans(&mut expr.node, offset);
    }

    /// Recursively shift expression spans parsed from an f-string interpolation back into outer-source coordinates.
    fn shift_expr_spans(&self, expr: &mut Expr, offset: usize) {
        match expr {
            Expr::Ident(_) | Expr::Literal(_) | Expr::SelfExpr | Expr::Yield(None) => {}
            Expr::Binary(left, _, right) => {
                self.shift_spanned_expr(left, offset);
                self.shift_spanned_expr(right, offset);
            }
            Expr::Unary(_, operand) | Expr::Try(operand) | Expr::Paren(operand) => {
                self.shift_spanned_expr(operand, offset);
            }
            Expr::Call(callee, _type_args, args) => {
                self.shift_spanned_expr(callee, offset);
                for arg in args {
                    match arg {
                        CallArg::Positional(value)
                        | CallArg::Named(_, value)
                        | CallArg::PositionalUnpack(value)
                        | CallArg::KeywordUnpack(value) => {
                            self.shift_spanned_expr(value, offset);
                        }
                    }
                }
            }
            Expr::Index(base, index) => {
                self.shift_spanned_expr(base, offset);
                self.shift_spanned_expr(index, offset);
            }
            Expr::Slice(base, slice) => {
                self.shift_spanned_expr(base, offset);
                if let Some(start) = &mut slice.start {
                    self.shift_spanned_expr(start, offset);
                }
                if let Some(end) = &mut slice.end {
                    self.shift_spanned_expr(end, offset);
                }
                if let Some(step) = &mut slice.step {
                    self.shift_spanned_expr(step, offset);
                }
            }
            Expr::Field(base, _) => {
                self.shift_spanned_expr(base, offset);
            }
            Expr::MethodCall(base, _, _type_args, args) => {
                self.shift_spanned_expr(base, offset);
                for arg in args {
                    match arg {
                        CallArg::Positional(value)
                        | CallArg::Named(_, value)
                        | CallArg::PositionalUnpack(value)
                        | CallArg::KeywordUnpack(value) => {
                            self.shift_spanned_expr(value, offset);
                        }
                    }
                }
            }
            Expr::Partial(partial) => {
                self.shift_spanned_expr(&mut partial.target, offset);
                for arg in &mut partial.type_args {
                    arg.span = Span::new(arg.span.start + offset, arg.span.end + offset);
                }
                for arg in &mut partial.args {
                    self.shift_spanned_expr(&mut arg.value, offset);
                }
            }
            Expr::Match(subject, arms) => {
                self.shift_spanned_expr(subject, offset);
                for arm in arms {
                    arm.span = Span::new(arm.span.start + offset, arm.span.end + offset);
                    if let Some(guard) = &mut arm.node.guard {
                        self.shift_spanned_expr(guard, offset);
                    }
                    match &mut arm.node.body {
                        MatchBody::Expr(value) => self.shift_spanned_expr(value, offset),
                        MatchBody::Block(_) => {}
                    }
                }
            }
            Expr::If(if_expr) => {
                self.shift_spanned_expr(&mut if_expr.condition, offset);
            }
            Expr::Loop(_) => {}
            Expr::Generator(generator) => {
                self.shift_spanned_expr(&mut generator.expr, offset);
                for clause in &mut generator.clauses {
                    self.shift_comprehension_clause(clause, offset);
                }
            }
            Expr::ListComp(comp) => {
                self.shift_spanned_expr(&mut comp.expr, offset);
                self.shift_spanned_expr(&mut comp.iter, offset);
                if let Some(filter) = &mut comp.filter {
                    self.shift_spanned_expr(filter, offset);
                }
                for clause in &mut comp.clauses {
                    self.shift_comprehension_clause(clause, offset);
                }
            }
            Expr::DictComp(comp) => {
                self.shift_spanned_expr(&mut comp.key, offset);
                self.shift_spanned_expr(&mut comp.value, offset);
                self.shift_spanned_expr(&mut comp.iter, offset);
                if let Some(filter) = &mut comp.filter {
                    self.shift_spanned_expr(filter, offset);
                }
                for clause in &mut comp.clauses {
                    self.shift_comprehension_clause(clause, offset);
                }
            }
            Expr::Closure(_, body) => {
                self.shift_spanned_expr(body, offset);
            }
            Expr::Tuple(elems) | Expr::Set(elems) => {
                for elem in elems {
                    self.shift_spanned_expr(elem, offset);
                }
            }
            Expr::List(entries) => {
                for entry in entries {
                    match entry {
                        ListEntry::Element(value) | ListEntry::Spread(value) => {
                            self.shift_spanned_expr(value, offset);
                        }
                    }
                }
            }
            Expr::Dict(entries) => {
                for entry in entries {
                    match entry {
                        DictEntry::Pair(key, value) => {
                            self.shift_spanned_expr(key, offset);
                            self.shift_spanned_expr(value, offset);
                        }
                        DictEntry::Spread(value) => self.shift_spanned_expr(value, offset),
                    }
                }
            }
            Expr::Constructor(_, args) => {
                for arg in args {
                    match arg {
                        CallArg::Positional(value)
                        | CallArg::Named(_, value)
                        | CallArg::PositionalUnpack(value)
                        | CallArg::KeywordUnpack(value) => {
                            self.shift_spanned_expr(value, offset);
                        }
                    }
                }
            }
            Expr::FString(parts) => {
                for part in parts {
                    if let FStringPart::Expr { expr, .. } = part {
                        self.shift_spanned_expr(expr, offset);
                    }
                }
            }
            Expr::Yield(Some(value)) => {
                self.shift_spanned_expr(value, offset);
            }
            Expr::Range {
                start,
                end,
                inclusive: _,
            } => {
                self.shift_spanned_expr(start, offset);
                self.shift_spanned_expr(end, offset);
            }
            Expr::Surface(surface_expr) => match &mut surface_expr.payload {
                SurfaceExprPayload::PrefixUnary(value) => self.shift_spanned_expr(value, offset),
                SurfaceExprPayload::RaceFor(race) => {
                    for arm in &mut race.arms {
                        self.shift_spanned_expr(&mut arm.awaitable, offset);
                        if let RaceForBody::Expr(value) = &mut arm.body {
                            self.shift_spanned_expr(value, offset);
                        }
                    }
                }
                SurfaceExprPayload::LeadingDotPath { .. } => {}
                SurfaceExprPayload::ScopedGlyph { left, right, .. } => {
                    self.shift_spanned_expr(left, offset);
                    self.shift_spanned_expr(right, offset);
                }
                SurfaceExprPayload::ScopedSymbolCall { args, .. } => {
                    for arg in args {
                        match arg {
                            CallArg::Positional(expr)
                            | CallArg::Named(_, expr)
                            | CallArg::PositionalUnpack(expr)
                            | CallArg::KeywordUnpack(expr) => self.shift_spanned_expr(expr, offset),
                        }
                    }
                }
            },
            Expr::VocabBlock(_) => {}
        }
    }

    /// Shift expression spans that live inside a comprehension clause.
    fn shift_comprehension_clause(&self, clause: &mut ComprehensionClause, offset: usize) {
        match clause {
            ComprehensionClause::For { iter, .. } => self.shift_spanned_expr(iter, offset),
            ComprehensionClause::If(condition) => self.shift_spanned_expr(condition, offset),
        }
    }

    fn parse_fstring_expr(&self, s: &str) -> Expr {
        // Properly parse the expression string by lexing and parsing it
        use crate::lexer;

        // Try to lex and parse the expression
        if let Ok(mut tokens) = lexer::lex(s) {
            // Ensure we have an EOF token at the end for the parser
            if tokens.is_empty() || !matches!(tokens.last().map(|t| &t.kind), Some(TokenKind::Eof)) {
                tokens.push(Token {
                    kind: TokenKind::Eof,
                    span: Span::default(),
                });
            }

            if tokens.len() > 1 {
                // At least one real token plus EOF
                let mut parser = Parser::new(&tokens);
                if let Ok(expr) = parser.expression() {
                    return expr.node;
                }
            }
        }

        // Fallback: treat as simple identifier
        Expr::Ident(s.to_string())
    }

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
            if next_leading > 0
                && self.check(&TokenKind::Dedent)
                && self.dedent_is_followed_by_outer_statement()
            {
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

    /// Parse one arm of a `match` expression.
    fn match_arm(&mut self) -> Result<Spanned<MatchArm>, CompileError> {
        let start = self.current_span().start;

        // Support both `case Pattern:` and `Pattern =>` syntax
        let pattern = if self.match_token(&TokenKind::Keyword(KeywordId::Case)) {
            let pat = self.pattern()?;

            // Check for optional guard: `case pattern if condition:`
            let guard = if self.match_token(&TokenKind::Keyword(KeywordId::If)) {
                Some(self.expression()?)
            } else {
                None
            };

            self.expect(
                &TokenKind::Punctuation(PunctuationId::Colon),
                "Expected ':' after case pattern",
            )?;

            // Check if inline or block
            if self.match_token(&TokenKind::Newline) {
                self.expect_suite_indent("Expected indented block")?;
                let body = self.block()?;
                self.expect(&TokenKind::Dedent, "Expected dedent after case body")?;
                let end = self.tokens[self.pos - 1].span.end;
                return Ok(Spanned::new(
                    MatchArm {
                        pattern: pat,
                        guard,
                        body: MatchBody::Block(body),
                    },
                    Span::new(start, end),
                ));
            } else {
                // Inline: could be expression or statement (like `return 0`)
                // Try parsing as a single statement and wrap in block
                let stmt = self.inline_statement()?;
                let end = stmt.span.end;
                return Ok(Spanned::new(
                    MatchArm {
                        pattern: pat,
                        guard,
                        body: MatchBody::Block(vec![stmt]),
                    },
                    Span::new(start, end),
                ));
            }
        } else {
            self.pattern()?
        };

        // Rust-style => syntax
        self.expect(
            &TokenKind::Punctuation(PunctuationId::FatArrow),
            "Expected '=>' after pattern",
        )?;

        // Check for block or expression
        if self.match_token(&TokenKind::Newline) {
            self.expect_suite_indent("Expected indented block")?;
            let body = self.block()?;
            self.expect(&TokenKind::Dedent, "Expected dedent after arm body")?;
            let end = self.tokens[self.pos - 1].span.end;
            Ok(Spanned::new(
                MatchArm {
                    pattern,
                    guard: None,
                    body: MatchBody::Block(body),
                },
                Span::new(start, end),
            ))
        } else {
            let stmt = self.inline_statement()?;
            let end = stmt.span.end;
            let body = if let Statement::Expr(expr) = &stmt.node {
                MatchBody::Expr(expr.clone())
            } else {
                MatchBody::Block(vec![stmt])
            };
            Ok(Spanned::new(
                MatchArm {
                    pattern,
                    guard: None,
                    body,
                },
                Span::new(start, end),
            ))
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
                return Ok(Spanned::new(
                    Pattern::Group(Box::new(first)),
                    Span::new(start, end),
                ));
            }

            let mut patterns = Vec::new();
            patterns.push(first);
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
            return Ok(Spanned::new(Pattern::Tuple(patterns), Span::new(start, end)));
        }

        // Identifier (binding) or constructor pattern
        if let TokenKind::Ident(name) = &self.peek().kind {
            let mut name = name.clone();
            self.advance();

            // Check for qualified pattern: Type.Variant or Type.Variant(args)
            if self.match_token(&TokenKind::Punctuation(PunctuationId::Dot)) {
                let variant = match &self.peek().kind {
                    TokenKind::Ident(v) => {
                        let v = v.clone();
                        self.advance();
                        v
                    }
                    TokenKind::Keyword(KeywordId::None) => {
                        // Allow "None" as a variant name (e.g., Maybe.None)
                        self.advance();
                        "None".to_string()
                    }
                    _ => {
                        return Err(errors::expected_variant_name_after_dot(self.current_span()));
                    }
                };
                // Build qualified name: "Type::Variant" for Rust
                name = format!("{}::{}", name, variant);
            }

            if self.match_token(&TokenKind::Punctuation(PunctuationId::LParen)) {
                // Constructor pattern: Some(x), Ok(value), Shape::Circle(r), Type(name=pat), etc.
                let mut patterns = Vec::new();
                if !self.check(&TokenKind::Punctuation(PunctuationId::RParen)) {
                    loop {
                        if matches!(
                            self.peek().kind,
                            TokenKind::Ident(_) | TokenKind::Keyword(_)
                        ) && self.peek_next().kind == TokenKind::Operator(OperatorId::Eq)
                        {
                            let name = self.identifier_or_any_keyword()?;
                            self.expect(
                                &TokenKind::Operator(OperatorId::Eq),
                                "Expected '=' after pattern field name",
                            )?;
                            let pat = self.pattern()?;
                            patterns.push(PatternArg::Named(name, pat));
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
            if name.contains("::") {
                let end = self.tokens[self.pos - 1].span.end;
                return Ok(Spanned::new(
                    Pattern::Constructor(name, vec![]),
                    Span::new(start, end),
                ));
            }

            // Just a binding
            let end = self.tokens[self.pos - 1].span.end;
            return Ok(Spanned::new(Pattern::Binding(name), Span::new(start, end)));
        }

        Err(errors::expected_pattern(
            &format!("{:?}", self.peek().kind),
            self.current_span(),
        ))
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
        self.expect(
            &TokenKind::Punctuation(PunctuationId::Colon),
            "Expected ':' after loop",
        )?;
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
        let binding = self.identifier_spanned()?.node;
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

    /// Parse a bracketed expression as either a list literal, list comprehension, or list spread form.
    fn list_or_comp(&mut self, start: usize) -> Result<Spanned<Expr>, CompileError> {
        // Implicit line continuation: skip newlines after [
        self.skip_newlines();

        // Empty list
        if self.match_token(&TokenKind::Punctuation(PunctuationId::RBracket)) {
            let end = self.tokens[self.pos - 1].span.end;
            return Ok(Spanned::new(Expr::List(Vec::new()), Span::new(start, end)));
        }

        let first = self.list_entry()?;
        self.skip_newlines();

        // Check for comprehension
        if let ListEntry::Element(first_expr) = &first
            && self.match_token(&TokenKind::Keyword(KeywordId::For))
        {
            let (pattern, iter, for_clause) = self.comprehension_for_clause_after_for()?;
            self.skip_newlines();
            let filter = if self.match_token(&TokenKind::Keyword(KeywordId::If)) {
                self.skip_newlines();
                Some(self.expression()?)
            } else {
                None
            };
            let mut clauses = vec![for_clause];
            if let Some(condition) = &filter {
                clauses.push(ComprehensionClause::If(condition.clone()));
            }
            self.skip_newlines();
            self.expect(
                &TokenKind::Punctuation(PunctuationId::RBracket),
                "Expected ']' after comprehension",
            )?;
            let end = self.tokens[self.pos - 1].span.end;
            return Ok(Spanned::new(
                Expr::ListComp(Box::new(ListComp {
                    expr: first_expr.clone(),
                    pattern,
                    iter,
                    filter,
                    clauses,
                })),
                Span::new(start, end),
            ));
        }

        // List literal
        let mut elements = vec![first];
        while self.match_token(&TokenKind::Punctuation(PunctuationId::Comma)) {
            self.skip_newlines();
            if self.check(&TokenKind::Punctuation(PunctuationId::RBracket)) {
                break;
            }
            elements.push(self.list_entry()?);
            self.skip_newlines();
        }
        self.expect(
            &TokenKind::Punctuation(PunctuationId::RBracket),
            "Expected ']' after list",
        )?;
        let end = self.tokens[self.pos - 1].span.end;
        Ok(Spanned::new(Expr::List(elements), Span::new(start, end)))
    }

    /// Parse one list literal entry, including RFC 038 `*expr` spread and invalid `**expr` diagnostics.
    fn list_entry(&mut self) -> Result<ListEntry, CompileError> {
        if self.match_token(&TokenKind::Operator(OperatorId::StarStar)) {
            return Err(errors::invalid_list_spread_marker(self.tokens[self.pos - 1].span));
        }

        if self.match_token(&TokenKind::Operator(OperatorId::Star)) {
            let marker_start = self.tokens[self.pos - 1].span.start;
            let value = self.expression()?;
            return Ok(ListEntry::Spread(Spanned::new(
                value.node,
                Span::new(marker_start, value.span.end),
            )));
        }

        Ok(ListEntry::Element(self.expression()?))
    }

    /// Parse a brace expression as a dictionary, set, comprehension, or RFC 038 dictionary spread literal.
    fn dict_or_comp(&mut self, start: usize) -> Result<Spanned<Expr>, CompileError> {
        // Implicit line continuation: skip newlines after {
        self.skip_newlines();

        // Empty dict/set
        if self.match_token(&TokenKind::Punctuation(PunctuationId::RBrace)) {
            let end = self.tokens[self.pos - 1].span.end;
            return Ok(Spanned::new(Expr::Dict(Vec::new()), Span::new(start, end)));
        }

        if self.check(&TokenKind::Operator(OperatorId::StarStar)) {
            let first = self.dict_entry()?;
            return self.finish_dict_literal(start, first);
        }

        if self.match_token(&TokenKind::Operator(OperatorId::Star)) {
            return Err(errors::invalid_dict_spread_marker(self.tokens[self.pos - 1].span));
        }

        let first = self.expression()?;
        self.skip_newlines();

        // Determine if this is a dict (has :) or set (no :)
        if self.match_token(&TokenKind::Punctuation(PunctuationId::Colon)) {
            self.skip_newlines();
            // It's a dict
            let first_value = self.expression()?;
            self.skip_newlines();

            // Check for comprehension
            if self.match_token(&TokenKind::Keyword(KeywordId::For)) {
                let (pattern, iter, for_clause) = self.comprehension_for_clause_after_for()?;
                self.skip_newlines();
                let filter = if self.match_token(&TokenKind::Keyword(KeywordId::If)) {
                    self.skip_newlines();
                    Some(self.expression()?)
                } else {
                    None
                };
                let mut clauses = vec![for_clause];
                if let Some(condition) = &filter {
                    clauses.push(ComprehensionClause::If(condition.clone()));
                }
                self.skip_newlines();
                self.expect(
                    &TokenKind::Punctuation(PunctuationId::RBrace),
                    "Expected '}' after comprehension",
                )?;
                let end = self.tokens[self.pos - 1].span.end;
                return Ok(Spanned::new(
                    Expr::DictComp(Box::new(DictComp {
                        key: first,
                        value: first_value,
                        pattern,
                        iter,
                        filter,
                        clauses,
                    })),
                    Span::new(start, end),
                ));
            }

            // Dict literal
            self.finish_dict_literal(start, DictEntry::Pair(first, first_value))
        } else {
            // It's a set literal: {expr, expr, ...}
            let mut elements = vec![first];
            while self.match_token(&TokenKind::Punctuation(PunctuationId::Comma)) {
                self.skip_newlines();
                if self.check(&TokenKind::Punctuation(PunctuationId::RBrace)) {
                    break;
                }
                if self.match_token(&TokenKind::Operator(OperatorId::Star))
                    || self.match_token(&TokenKind::Operator(OperatorId::StarStar))
                {
                    return Err(errors::set_literal_spread_not_supported(self.tokens[self.pos - 1].span));
                }
                elements.push(self.expression()?);
                self.skip_newlines();
            }
            self.expect(&TokenKind::Punctuation(PunctuationId::RBrace), "Expected '}' after set")?;
            let end = self.tokens[self.pos - 1].span.end;
            Ok(Spanned::new(Expr::Set(elements), Span::new(start, end)))
        }
    }

    /// Finish parsing a dictionary literal after the first entry has already been disambiguated.
    fn finish_dict_literal(&mut self, start: usize, first: DictEntry) -> Result<Spanned<Expr>, CompileError> {
        self.skip_newlines();

        let mut entries = vec![first];
        while self.match_token(&TokenKind::Punctuation(PunctuationId::Comma)) {
            self.skip_newlines();
            if self.check(&TokenKind::Punctuation(PunctuationId::RBrace)) {
                break;
            }
            entries.push(self.dict_entry()?);
            self.skip_newlines();
        }
        self.expect(
            &TokenKind::Punctuation(PunctuationId::RBrace),
            "Expected '}' after dict",
        )?;
        let end = self.tokens[self.pos - 1].span.end;
        Ok(Spanned::new(Expr::Dict(entries), Span::new(start, end)))
    }

    /// Parse one dictionary literal entry, including `key: value`, `**expr` spread, and invalid `*expr` diagnostics.
    fn dict_entry(&mut self) -> Result<DictEntry, CompileError> {
        if self.match_token(&TokenKind::Operator(OperatorId::StarStar)) {
            let marker_start = self.tokens[self.pos - 1].span.start;
            let value = self.expression()?;
            return Ok(DictEntry::Spread(Spanned::new(
                value.node,
                Span::new(marker_start, value.span.end),
            )));
        }

        if self.match_token(&TokenKind::Operator(OperatorId::Star)) {
            return Err(errors::invalid_dict_spread_marker(self.tokens[self.pos - 1].span));
        }

        let key = self.expression()?;
        self.skip_newlines();
        self.expect(
            &TokenKind::Punctuation(PunctuationId::Colon),
            "Expected ':' in dict entry",
        )?;
        self.skip_newlines();
        let value = self.expression()?;
        Ok(DictEntry::Pair(key, value))
    }

    /// Parse a `for pattern in iter` comprehension clause after the leading `for` token has been consumed.
    fn comprehension_for_clause_after_for(
        &mut self,
    ) -> Result<(Spanned<Pattern>, Spanned<Expr>, ComprehensionClause), CompileError> {
        self.skip_newlines();
        let pattern = self.for_binding_pattern()?;
        self.skip_newlines();
        self.expect(&TokenKind::Keyword(KeywordId::In), "Expected 'in' in comprehension")?;
        self.skip_newlines();
        let iter = self.expression()?;
        let clause = ComprehensionClause::For {
            pattern: pattern.clone(),
            iter: iter.clone(),
        };
        Ok((pattern, iter, clause))
    }

    /// Parse the full comprehension-clause tail used by generator expressions.
    fn generator_clauses_after_for(&mut self) -> Result<Vec<ComprehensionClause>, CompileError> {
        let (_, _, first_for) = self.comprehension_for_clause_after_for()?;
        let mut clauses = vec![first_for];

        loop {
            self.skip_newlines();
            if self.match_token(&TokenKind::Keyword(KeywordId::If)) {
                self.skip_newlines();
                clauses.push(ComprehensionClause::If(self.expression()?));
                continue;
            }
            if self.match_token(&TokenKind::Keyword(KeywordId::For)) {
                let (_, _, clause) = self.comprehension_for_clause_after_for()?;
                clauses.push(clause);
                continue;
            }
            break;
        }

        Ok(clauses)
    }

    /// Parse a parenthesized expression or tuple literal.
    fn paren_or_tuple(&mut self, start: usize) -> Result<Spanned<Expr>, CompileError> {
        // Implicit line continuation: skip newlines after (
        self.skip_newlines();

        // Empty parens - could be () => expr (closure) or () (unit tuple)
        if self.match_token(&TokenKind::Punctuation(PunctuationId::RParen)) {
            // Check for arrow function: () => expr
            if self.match_token(&TokenKind::Punctuation(PunctuationId::FatArrow)) {
                self.skip_newlines();
                let body = self.expression()?;
                let end = body.span.end;
                return Ok(Spanned::new(
                    Expr::Closure(Vec::new(), Box::new(body)),
                    Span::new(start, end),
                ));
            }
            let end = self.tokens[self.pos - 1].span.end;
            return Ok(Spanned::new(Expr::Tuple(Vec::new()), Span::new(start, end)));
        }

        let first = self.expression()?;
        self.skip_newlines();

        if self.match_token(&TokenKind::Keyword(KeywordId::For)) {
            let clauses = self.generator_clauses_after_for()?;
            self.skip_newlines();
            self.expect(
                &TokenKind::Punctuation(PunctuationId::RParen),
                "Expected ')' after generator expression",
            )?;
            let end = self.tokens[self.pos - 1].span.end;
            return Ok(Spanned::new(
                Expr::Generator(Box::new(GeneratorExpr {
                    expr: first,
                    clauses,
                })),
                Span::new(start, end),
            ));
        }

        // Check for tuple (needs comma)
        if self.match_token(&TokenKind::Punctuation(PunctuationId::Comma)) {
            self.skip_newlines();
            let mut elements = vec![first];
            if !self.check(&TokenKind::Punctuation(PunctuationId::RParen)) {
                loop {
                    elements.push(self.expression()?);
                    self.skip_newlines();
                    if !self.match_token(&TokenKind::Punctuation(PunctuationId::Comma)) {
                        break;
                    }
                    self.skip_newlines();
                    if self.check(&TokenKind::Punctuation(PunctuationId::RParen)) {
                        break;
                    }
                }
            }
            self.expect(
                &TokenKind::Punctuation(PunctuationId::RParen),
                "Expected ')' after tuple",
            )?;

            // Check for arrow function: (x, y) => expr
            if self.match_token(&TokenKind::Punctuation(PunctuationId::FatArrow)) {
                self.skip_newlines();
                let params = self.exprs_to_params(&elements)?;
                let body = self.expression()?;
                let end = body.span.end;
                return Ok(Spanned::new(
                    Expr::Closure(params, Box::new(body)),
                    Span::new(start, end),
                ));
            }

            let end = self.tokens[self.pos - 1].span.end;
            return Ok(Spanned::new(Expr::Tuple(elements), Span::new(start, end)));
        }

        // Just parenthesized expression (or single-param closure)
        self.expect(&TokenKind::Punctuation(PunctuationId::RParen), "Expected ')'")?;

        // Check for arrow function: (x) => expr
        if self.match_token(&TokenKind::Punctuation(PunctuationId::FatArrow)) {
            self.skip_newlines();
            let params = self.exprs_to_params(std::slice::from_ref(&first))?;
            let body = self.expression()?;
            let end = body.span.end;
            return Ok(Spanned::new(
                Expr::Closure(params, Box::new(body)),
                Span::new(start, end),
            ));
        }

        let end = self.tokens[self.pos - 1].span.end;
        Ok(Spanned::new(Expr::Paren(Box::new(first)), Span::new(start, end)))
    }

    /// Convert expressions to closure parameters
    /// Only identifiers are valid as closure params
    fn exprs_to_params(&self, exprs: &[Spanned<Expr>]) -> Result<Vec<Spanned<Param>>, CompileError> {
        let mut params = Vec::new();
        for expr in exprs {
            match &expr.node {
                Expr::Ident(name) => {
                    // Closure params have inferred types (represented as "_")
                    let inferred_ty = Spanned::new(Type::Simple("_".to_string()), expr.span);
                    params.push(Spanned::new(
                        Param {
                            is_mut: false,
                            kind: ParamKind::Normal,
                            name: name.clone(),
                            ty: inferred_ty,
                            default: None,
                        },
                        expr.span,
                    ));
                }
                _ => {
                    return Err(errors::closure_params_must_be_identifiers(expr.span));
                }
            }
        }
        Ok(params)
    }

    /// Parse call arguments while temporarily enabling descriptors scoped to the named call target.
    fn call_args_for(&mut self, call: Option<String>) -> Result<Vec<CallArg>, CompileError> {
        let pushed_context = call.map(|call| {
            self.scoped_call_argument_stack
                .push(ScopedCallArgumentContext { call });
        });
        let result = self.call_args();
        if pushed_context.is_some() {
            self.scoped_call_argument_stack.pop();
        }
        result
    }

    /// Parse call arguments.
    fn call_args(&mut self) -> Result<Vec<CallArg>, CompileError> {
        // Implicit line continuation: skip newlines after (
        self.skip_newlines();

        let mut args = Vec::new();
        if !self.check(&TokenKind::Punctuation(PunctuationId::RParen)) {
            loop {
                // Allow trailing comma: check for ) at start of loop iteration
                self.skip_newlines();
                if self.check(&TokenKind::Punctuation(PunctuationId::RParen)) {
                    break;
                }

                // Check for named argument (allow keywords)
                if matches!(
                    self.peek().kind,
                    TokenKind::Ident(_) | TokenKind::Keyword(_)
                ) && self.peek_next().kind == TokenKind::Operator(OperatorId::Eq)
                {
                    let name = self.identifier_or_any_keyword()?;
                    self.expect(
                        &TokenKind::Operator(OperatorId::Eq),
                        "Expected '=' after named argument",
                    )?;
                    self.skip_newlines();
                    let value = self.expression()?;
                    self.skip_newlines();
                    args.push(CallArg::Named(name, value));
                    if !self.match_token(&TokenKind::Punctuation(PunctuationId::Comma)) {
                        break;
                    }
                    continue;
                }
                if self.match_token(&TokenKind::Operator(OperatorId::StarStar)) {
                    let expr = self.expression()?;
                    self.skip_newlines();
                    args.push(CallArg::KeywordUnpack(expr));
                    if !self.match_token(&TokenKind::Punctuation(PunctuationId::Comma)) {
                        break;
                    }
                    continue;
                }
                if self.match_token(&TokenKind::Operator(OperatorId::Star)) {
                    let expr = self.expression()?;
                    self.skip_newlines();
                    args.push(CallArg::PositionalUnpack(expr));
                    if !self.match_token(&TokenKind::Punctuation(PunctuationId::Comma)) {
                        break;
                    }
                    continue;
                }
                let expr = self.expression()?;
                self.skip_newlines();
                args.push(CallArg::Positional(expr));
                if !self.match_token(&TokenKind::Punctuation(PunctuationId::Comma)) {
                    break;
                }
            }
        }
        Ok(args)
    }

    /// Parse the keyword-only argument list inside a partial call template.
    fn partial_args(&mut self) -> Result<Vec<PartialArg>, CompileError> {
        self.skip_newlines();

        let mut args = Vec::new();
        if !self.check(&TokenKind::Punctuation(PunctuationId::RParen)) {
            loop {
                self.skip_newlines();
                if self.check(&TokenKind::Punctuation(PunctuationId::RParen)) {
                    break;
                }

                if !matches!(self.peek().kind, TokenKind::Ident(_) | TokenKind::Keyword(_))
                    || !self.peek_next().kind.is_operator(OperatorId::Eq)
                {
                    return Err(CompileError::syntax(
                        "Partial presets only support keyword arguments".to_string(),
                        self.current_span(),
                    ));
                }

                let name = self.identifier_or_any_keyword()?;
                self.expect_op(OperatorId::Eq, "Expected '=' after partial preset name")?;
                self.skip_newlines();
                let value = self.expression()?;
                self.skip_newlines();
                args.push(PartialArg { name, value });

                if !self.match_token(&TokenKind::Punctuation(PunctuationId::Comma)) {
                    break;
                }
            }
        }
        Ok(args)
    }

    /// Convert an ordinary parsed call argument list into RFC 084 keyword-only partial presets.
    fn partial_args_from_call_args(
        &self,
        args: Vec<CallArg>,
        _template_span: Span,
    ) -> Result<Vec<PartialArg>, CompileError> {
        let mut partial_args = Vec::new();
        for arg in args {
            match arg {
                CallArg::Named(name, value) => partial_args.push(PartialArg { name, value }),
                CallArg::Positional(value) | CallArg::PositionalUnpack(value) | CallArg::KeywordUnpack(value) => {
                    return Err(CompileError::syntax(
                        "Partial presets only support keyword arguments".to_string(),
                        value.span,
                    ));
                }
            }
        }
        Ok(partial_args)
    }

}

/// Split a raw f-string interpolation body into expression text plus the supported top-level format marker.
fn split_fstring_format(text: &str) -> (&str, FStringFormat) {
    let mut depth = 0usize;
    let mut quote = None;
    let mut escaped = false;
    let mut format_colon = None;

    for (idx, ch) in text.char_indices() {
        if let Some(active_quote) = quote {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == active_quote {
                quote = None;
            }
            continue;
        }

        match ch {
            '\'' | '"' => quote = Some(ch),
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth = depth.saturating_sub(1),
            ':' if depth == 0 => format_colon = Some(idx),
            _ => {}
        }
    }

    if let Some(idx) = format_colon {
        let spec = text[idx + 1..].trim();
        if spec == "?" {
            return (text[..idx].trim_end(), FStringFormat::Debug);
        }
    }

    (text, FStringFormat::Display)
}
