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

    fn comparison(&mut self) -> Result<Spanned<Expr>, CompileError> {
        let mut left = self.range_expr()?;

        loop {
            let op = if self.match_token(&TokenKind::Operator(OperatorId::EqEq)) {
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
                BinaryOp::Is
            } else {
                break;
            };

            let right = self.range_expr()?;
            let span = left.span.merge(right.span);
            left = Spanned::new(Expr::Binary(Box::new(left), op, Box::new(right)), span);
        }

        Ok(left)
    }

    /// Parse range expressions: `start..end` or `start..=end`
    fn range_expr(&mut self) -> Result<Spanned<Expr>, CompileError> {
        let left = self.additive()?;

        // Check for range operators
        let is_inclusive = if self.match_token(&TokenKind::Operator(OperatorId::DotDotEq)) {
            true
        } else if self.match_token(&TokenKind::Operator(OperatorId::DotDot)) {
            false
        } else {
            return Ok(left);
        };

        let right = self.additive()?;
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
            left = Spanned::new(Expr::Binary(Box::new(left), op, Box::new(right)), span);
        }

        Ok(left)
    }

    fn multiplicative(&mut self) -> Result<Spanned<Expr>, CompileError> {
        let mut left = self.power()?;

        loop {
            let op = if self.match_token(&TokenKind::Operator(OperatorId::Star)) {
                BinaryOp::Mul
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
            left = Spanned::new(Expr::Binary(Box::new(left), op, Box::new(right)), span);
        }

        Ok(left)
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

    fn unary(&mut self) -> Result<Spanned<Expr>, CompileError> {
        if self.match_token(&TokenKind::Operator(OperatorId::Minus)) {
            let start = self.tokens[self.pos - 1].span.start;
            let expr = self.unary()?;
            let span = Span::new(start, expr.span.end);
            Ok(Spanned::new(Expr::Unary(UnaryOp::Neg, Box::new(expr)), span))
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

    fn postfix(&mut self) -> Result<Spanned<Expr>, CompileError> {
        let mut expr = self.primary()?;

        loop {
            if self.match_token(&TokenKind::Punctuation(PunctuationId::Question)) {
                let span = Span::new(expr.span.start, self.tokens[self.pos - 1].span.end);
                expr = Spanned::new(Expr::Try(Box::new(expr)), span);
            } else if self.match_token(&TokenKind::Punctuation(PunctuationId::Dot)) {
                // Check for tuple index access (.0, .1, etc) vs field/method access
                if let TokenKind::Int(n) = &self.peek().kind {
                    // Tuple index access: expr.0, expr.1
                    let idx = *n;
                    self.advance();
                    let span = Span::new(expr.span.start, self.tokens[self.pos - 1].span.end);
                    // Use the index as a string field name
                    expr = Spanned::new(Expr::Field(Box::new(expr), idx.to_string()), span);
                } else {
                    // Allow keywords like "None" as field/variant names
                    let name = self.identifier_or_any_keyword()?;
                    if self.match_token(&TokenKind::Punctuation(PunctuationId::LParen)) {
                        let args = self.call_args()?;
                        self.expect(
                            &TokenKind::Punctuation(PunctuationId::RParen),
                            "Expected ')' after arguments",
                        )?;
                        let span = Span::new(expr.span.start, self.tokens[self.pos - 1].span.end);
                        expr = Spanned::new(Expr::MethodCall(Box::new(expr), name, args), span);
                    } else {
                        let span = Span::new(expr.span.start, self.tokens[self.pos - 1].span.end);
                        expr = Spanned::new(Expr::Field(Box::new(expr), name), span);
                    }
                }
            } else if self.match_token(&TokenKind::Punctuation(PunctuationId::LBracket)) {
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
                let args = self.call_args()?;
                self.expect(
                    &TokenKind::Punctuation(PunctuationId::RParen),
                    "Expected ')' after arguments",
                )?;
                let span = Span::new(expr.span.start, self.tokens[self.pos - 1].span.end);
                expr = Spanned::new(Expr::Call(Box::new(expr), args), span);
            } else {
                break;
            }
        }

        Ok(expr)
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

    fn primary(&mut self) -> Result<Spanned<Expr>, CompileError> {
        let start = self.current_span().start;

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

        // self
        if self.match_token(&TokenKind::Keyword(KeywordId::SelfKw)) {
            let end = self.tokens[self.pos - 1].span.end;
            return Ok(Spanned::new(Expr::SelfExpr, Span::new(start, end)));
        }

        // Literals
        if let Some(lit) = self.try_literal() {
            let end = self.tokens[self.pos - 1].span.end;
            return Ok(Spanned::new(Expr::Literal(lit), Span::new(start, end)));
        }

        // f-string
        if let TokenKind::FString(parts) = &self.peek().kind {
            let parts = parts.clone();
            let fstring_span = self.peek().span; // Capture span before advancing
            self.advance();
            let fparts = self.convert_fstring_parts(&parts, fstring_span);
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

    fn try_literal(&mut self) -> Option<Literal> {
        match &self.peek().kind {
            TokenKind::Int(n) => {
                let n = *n;
                self.advance();
                Some(Literal::Int(n))
            }
            TokenKind::Float(f) => {
                let f = *f;
                self.advance();
                Some(Literal::Float(f))
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

    fn convert_fstring_parts(&self, parts: &[LexFStringPart], fstring_span: Span) -> Vec<FStringPart> {
        parts
            .iter()
            .map(|p| match p {
                LexFStringPart::Literal(s) => FStringPart::Literal(s.clone()),
                LexFStringPart::Expr(s) => {
                    // Parse simple field access chains like "user.name" or "obj.field.sub"
                    let expr = self.parse_fstring_expr(s);
                    // Use the f-string's span so errors point to the f-string, not line 1
                    FStringPart::Expr(Spanned::new(expr, fstring_span))
                }
            })
            .collect()
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

    fn match_expr(&mut self, start: usize) -> Result<Spanned<Expr>, CompileError> {
        let subject = self.expression()?;
        self.expect(
            &TokenKind::Punctuation(PunctuationId::Colon),
            "Expected ':' after match subject",
        )?;
        self.expect(&TokenKind::Newline, "Expected newline after ':'")?;
        self.expect(&TokenKind::Indent, "Expected indented block")?;

        let mut arms = Vec::new();
        self.skip_newlines();
        while !self.check(&TokenKind::Dedent) && !self.is_at_end() {
            arms.push(self.match_arm()?);
            self.skip_newlines();
        }

        self.expect(&TokenKind::Dedent, "Expected dedent after match body")?;
        let end = self.tokens[self.pos - 1].span.end;
        Ok(Spanned::new(
            Expr::Match(Box::new(subject), arms),
            Span::new(start, end),
        ))
    }

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
                self.expect(&TokenKind::Indent, "Expected indented block")?;
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
                // Consume trailing newline after inline statement
                self.match_token(&TokenKind::Newline);
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
            self.expect(&TokenKind::Indent, "Expected indented block")?;
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
            self.match_token(&TokenKind::Newline);
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

    fn pattern(&mut self) -> Result<Spanned<Pattern>, CompileError> {
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
            let mut patterns = Vec::new();
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

    fn if_expr(&mut self, start: usize) -> Result<Spanned<Expr>, CompileError> {
        self.expect(&TokenKind::Keyword(KeywordId::If), "Expected 'if'")?;
        let condition = self.expression()?;
        self.expect(
            &TokenKind::Punctuation(PunctuationId::Colon),
            "Expected ':' after if condition",
        )?;
        self.expect(&TokenKind::Newline, "Expected newline after ':'")?;
        self.expect(&TokenKind::Indent, "Expected indented block")?;
        let then_body = self.block()?;
        self.expect(&TokenKind::Dedent, "Expected dedent after if body")?;

        let else_body = if self.match_token(&TokenKind::Keyword(KeywordId::Else)) {
            self.expect(&TokenKind::Punctuation(PunctuationId::Colon), "Expected ':' after else")?;
            self.expect(&TokenKind::Newline, "Expected newline after ':'")?;
            self.expect(&TokenKind::Indent, "Expected indented block")?;
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

    fn list_or_comp(&mut self, start: usize) -> Result<Spanned<Expr>, CompileError> {
        // Implicit line continuation: skip newlines after [
        self.skip_newlines();

        // Empty list
        if self.match_token(&TokenKind::Punctuation(PunctuationId::RBracket)) {
            let end = self.tokens[self.pos - 1].span.end;
            return Ok(Spanned::new(Expr::List(Vec::new()), Span::new(start, end)));
        }

        let first = self.expression()?;
        self.skip_newlines();

        // Check for comprehension
        if self.match_token(&TokenKind::Keyword(KeywordId::For)) {
            self.skip_newlines();
            let var = self.identifier()?;
            self.skip_newlines();
            self.expect(&TokenKind::Keyword(KeywordId::In), "Expected 'in' in comprehension")?;
            self.skip_newlines();
            let iter = self.expression()?;
            self.skip_newlines();
            let filter = if self.match_token(&TokenKind::Keyword(KeywordId::If)) {
                self.skip_newlines();
                Some(self.expression()?)
            } else {
                None
            };
            self.skip_newlines();
            self.expect(
                &TokenKind::Punctuation(PunctuationId::RBracket),
                "Expected ']' after comprehension",
            )?;
            let end = self.tokens[self.pos - 1].span.end;
            return Ok(Spanned::new(
                Expr::ListComp(Box::new(ListComp {
                    expr: first,
                    var,
                    iter,
                    filter,
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
            elements.push(self.expression()?);
            self.skip_newlines();
        }
        self.expect(
            &TokenKind::Punctuation(PunctuationId::RBracket),
            "Expected ']' after list",
        )?;
        let end = self.tokens[self.pos - 1].span.end;
        Ok(Spanned::new(Expr::List(elements), Span::new(start, end)))
    }

    fn dict_or_comp(&mut self, start: usize) -> Result<Spanned<Expr>, CompileError> {
        // Implicit line continuation: skip newlines after {
        self.skip_newlines();

        // Empty dict/set
        if self.match_token(&TokenKind::Punctuation(PunctuationId::RBrace)) {
            let end = self.tokens[self.pos - 1].span.end;
            return Ok(Spanned::new(Expr::Dict(Vec::new()), Span::new(start, end)));
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
                self.skip_newlines();
                let var = self.identifier()?;
                self.skip_newlines();
                self.expect(&TokenKind::Keyword(KeywordId::In), "Expected 'in' in comprehension")?;
                self.skip_newlines();
                let iter = self.expression()?;
                self.skip_newlines();
                let filter = if self.match_token(&TokenKind::Keyword(KeywordId::If)) {
                    self.skip_newlines();
                    Some(self.expression()?)
                } else {
                    None
                };
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
                        var,
                        iter,
                        filter,
                    })),
                    Span::new(start, end),
                ));
            }

            // Dict literal
            let mut entries = vec![(first, first_value)];
            while self.match_token(&TokenKind::Punctuation(PunctuationId::Comma)) {
                self.skip_newlines();
                if self.check(&TokenKind::Punctuation(PunctuationId::RBrace)) {
                    break;
                }
                let key = self.expression()?;
                self.skip_newlines();
                self.expect(
                    &TokenKind::Punctuation(PunctuationId::Colon),
                    "Expected ':' in dict entry",
                )?;
                self.skip_newlines();
                let value = self.expression()?;
                self.skip_newlines();
                entries.push((key, value));
            }
            self.expect(
                &TokenKind::Punctuation(PunctuationId::RBrace),
                "Expected '}' after dict",
            )?;
            let end = self.tokens[self.pos - 1].span.end;
            Ok(Spanned::new(Expr::Dict(entries), Span::new(start, end)))
        } else {
            // It's a set literal: {expr, expr, ...}
            let mut elements = vec![first];
            while self.match_token(&TokenKind::Punctuation(PunctuationId::Comma)) {
                self.skip_newlines();
                if self.check(&TokenKind::Punctuation(PunctuationId::RBrace)) {
                    break;
                }
                elements.push(self.expression()?);
                self.skip_newlines();
            }
            self.expect(&TokenKind::Punctuation(PunctuationId::RBrace), "Expected '}' after set")?;
            let end = self.tokens[self.pos - 1].span.end;
            Ok(Spanned::new(Expr::Set(elements), Span::new(start, end)))
        }
    }

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

}

