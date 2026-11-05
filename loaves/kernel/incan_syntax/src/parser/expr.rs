/// Expression parsing methods.
///
/// This chunk implements the expression grammar using a precedence ladder: `or` → `and` → `not` → comparison → range →
/// `|` → `^` → `&` → shift → additive → multiplicative → prefix `-` / `~` → power → postfix → primary. `**` binds
/// tighter than a prefix operator on its left and looser than one on its right: its exponent is read at the prefix
/// level.
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
            left = self.scoped_or_builtin_binary(glyph, op, left, right, span);
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
            left = self.scoped_or_builtin_binary(Some(glyph), op, left, right, span);
        }

        Ok(left)
    }

    /// Parse multiplicative expressions, preserving DSL-owned glyphs in eligible vocab blocks.
    fn multiplicative(&mut self) -> Result<Spanned<Expr>, CompileError> {
        let mut left = self.unary()?;

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

            let right = self.unary()?;
            let span = left.span.merge(right.span);
            let glyph = match op {
                BinaryOp::Mul => "*",
                BinaryOp::MatMul => "@",
                BinaryOp::FloorDiv => "//",
                BinaryOp::Div => "/",
                BinaryOp::Mod => "%",
                _ => unreachable!("multiplicative parser only emits multiplicative operators"),
            };
            left = self.scoped_or_builtin_binary(Some(glyph), op, left, right, span);
        }

        Ok(left)
    }

    /// Select the active glyph owner before moving either operand. Both scoped and ordinary binary expressions
    /// consume the parsed trees once; probing an inactive vocabulary must not clone every accumulated left subtree.
    fn scoped_or_builtin_binary(
        &self,
        glyph: Option<&str>,
        op: BinaryOp,
        left: Spanned<Expr>,
        right: Spanned<Expr>,
        span: Span,
    ) -> Spanned<Expr> {
        if let Some(glyph) = glyph
            && let Some(active) = self.active_scoped_glyph_surface_descriptor(glyph)
        {
            return self.scoped_glyph_binary_from_active(active, glyph, left, right, span);
        }
        Spanned::new(Expr::Binary(Box::new(left), op, Box::new(right)), span)
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
}

include!("expr/postfix.rs");
include!("expr/primary.rs");
include!("expr/scoped_surfaces.rs");
include!("expr/literals_and_spans.rs");
include!("expr/match_and_control.rs");
include!("expr/collections_and_calls.rs");

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
        return (text[..idx].trim_end(), FStringFormat::Unsupported(spec.to_string()));
    }

    (text, FStringFormat::Display)
}
