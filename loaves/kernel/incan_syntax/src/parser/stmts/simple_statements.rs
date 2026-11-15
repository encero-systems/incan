/// `return`, `assert`, `break`, `if`, `while`, `loop`, `for` and `unsafe` statements, and `for` bindings.
impl<'a> Parser<'a> {
    fn return_stmt(&mut self) -> Result<Statement, CompileError> {
        self.expect(&TokenKind::Keyword(KeywordId::Return), "Expected 'return'")?;
        let expr = if !self.check(&TokenKind::Newline) && !self.check(&TokenKind::Dedent) {
            Some(self.expression()?)
        } else {
            None
        };
        Ok(Statement::Return(expr))
    }

    /// Return `true` when the current token is statement-position `assert`.
    ///
    /// RFC 018 keeps `assert` soft in the lexer, so statement parsing recognizes the identifier spelling directly
    /// while assignment-like uses (`assert = value`, `assert: T = value`) remain ordinary identifiers.
    fn is_assert_statement_keyword(&self) -> bool {
        if !matches!(
            &self.peek().kind,
            TokenKind::Ident(name) if name == incan_lang::lang::keywords::as_str(KeywordId::Assert)
        ) {
            return false;
        }

        !matches!(
            self.peek_next().kind,
            TokenKind::Operator(OperatorId::Eq)
                | TokenKind::Punctuation(PunctuationId::Colon)
                | TokenKind::Punctuation(PunctuationId::Comma)
                | TokenKind::Operator(OperatorId::PlusEq)
                | TokenKind::Operator(OperatorId::MinusEq)
                | TokenKind::Operator(OperatorId::StarEq)
                | TokenKind::Operator(OperatorId::SlashEq)
                | TokenKind::Operator(OperatorId::SlashSlashEq)
                | TokenKind::Operator(OperatorId::PercentEq)
        )
    }

    /// Return whether the current tokens form the contextual `unsafe:` acknowledgment block.
    ///
    /// `unsafe` remains available as an identifier; requiring a newline after the colon keeps typed assignments
    /// such as `unsafe: int = value` outside this grammar form.
    fn starts_unsafe_block(&self) -> bool {
        self.peek_ident_text("unsafe")
            && self.peek_next().kind.is_punctuation(PunctuationId::Colon)
            && matches!(
                self.tokens.get(self.pos + 2).map(|token| &token.kind),
                Some(TokenKind::Newline)
            )
    }

    /// Parse the RFC 018 `assert` statement family.
    ///
    /// Ordinary expressions, `is` pattern assertions, `raises` assertions, and optional messages are represented
    /// distinctly so later compiler stages can lower without reparsing expression syntax.
    fn assert_stmt(&mut self) -> Result<Statement, CompileError> {
        self.expect(&TokenKind::Ident(String::new()), "Expected 'assert'")?;

        let condition = self.expression()?;
        let kind = if self.match_ident_text("raises") {
            self.assert_raises_kind(condition)?
        } else {
            self.assert_condition_kind(condition)?
        };
        let message = if self.match_punct(PunctuationId::Comma) {
            Some(self.expression()?)
        } else {
            None
        };

        Ok(Statement::Assert(AssertStmt { kind, message }))
    }

    /// Convert the parsed condition expression into a structured assertion form.
    fn assert_condition_kind(&self, condition: Spanned<Expr>) -> Result<AssertKind, CompileError> {
        let is_pattern_assert = matches!(
            &condition.node,
            Expr::Binary(_, BinaryOp::Is, right) if Self::is_assert_pattern_candidate(&right.node)
        );
        if !is_pattern_assert {
            return Ok(AssertKind::Condition(condition));
        }

        let Expr::Binary(left, BinaryOp::Is, right) = condition.node else {
            unreachable!("pattern assertion candidates are filtered to `is` binary expressions")
        };

        let pattern = self.expr_to_assert_pattern(*right)?;
        Ok(AssertKind::IsPattern { value: *left, pattern })
    }

    /// Return true when the RHS of `assert value is <rhs>` is one of the RFC 018 pattern forms.
    fn is_assert_pattern_candidate(expr: &Expr) -> bool {
        match expr {
            Expr::Literal(Literal::None) => true,
            Expr::Call(callee, _, _) => {
                matches!(&callee.node, Expr::Ident(name) if matches!(name.as_str(), "Some" | "Ok" | "Err"))
            }
            _ => false,
        }
    }

    /// Parse the tail of `assert call() raises ErrorType`.
    fn assert_raises_kind(&mut self, call: Spanned<Expr>) -> Result<AssertKind, CompileError> {
        if !matches!(call.node, Expr::Call(_, _, _) | Expr::MethodCall(_, _, _, _)) {
            return Err(CompileError::syntax(
                "`assert ... raises` requires a call expression".to_string(),
                call.span,
            ));
        }

        let error_type = self.type_expr()?;
        Ok(AssertKind::Raises { call, error_type })
    }

    /// Convert the expression form parsed after `is` into the limited RFC 018 pattern subset.
    fn expr_to_assert_pattern(&self, expr: Spanned<Expr>) -> Result<Spanned<Pattern>, CompileError> {
        match expr.node {
            Expr::Literal(Literal::None) => Ok(Spanned::new(
                Pattern::Constructor(Spanned::new("None".to_string(), expr.span), Vec::new()),
                expr.span,
            )),
            Expr::Call(callee, type_args, args) => {
                if !type_args.is_empty() {
                    return Err(CompileError::syntax(
                        "`assert ... is` patterns do not support explicit type arguments".to_string(),
                        expr.span,
                    ));
                }
                let Expr::Ident(name) = callee.node else {
                    return Err(CompileError::syntax(
                        "`assert ... is` only supports Some/Ok/Err/None patterns".to_string(),
                        callee.span,
                    ));
                };
                if !matches!(name.as_str(), "Some" | "Ok" | "Err") {
                    return Err(CompileError::syntax(
                        "`assert ... is` only supports Some/Ok/Err/None patterns".to_string(),
                        callee.span,
                    ));
                }
                let pattern_args = self.assert_pattern_args(args, expr.span)?;
                Ok(Spanned::new(
                    Pattern::Constructor(Spanned::new(name, callee.span), pattern_args),
                    expr.span,
                ))
            }
            _ => Err(CompileError::syntax(
                "`assert ... is` only supports Some/Ok/Err/None patterns".to_string(),
                expr.span,
            )),
        }
    }

    /// Convert positional call arguments into the single binding/wildcard pattern allowed by RFC 018.
    fn assert_pattern_args(&self, args: Vec<CallArg>, span: Span) -> Result<Vec<PatternArg>, CompileError> {
        if args.len() != 1 {
            return Err(CompileError::syntax(
                "`assert ... is` patterns require exactly one binding or `_`".to_string(),
                span,
            ));
        }

        let Some(arg) = args.into_iter().next() else {
            return Err(CompileError::syntax(
                "`assert ... is` patterns require exactly one binding or `_`".to_string(),
                span,
            ));
        };

        let CallArg::Positional(value) = arg else {
            return Err(CompileError::syntax(
                "`assert ... is` patterns do not support named fields".to_string(),
                span,
            ));
        };

        let pattern = match value.node {
            Expr::Ident(name) if name == "_" => Pattern::Wildcard,
            Expr::Ident(name) => Pattern::Binding(name),
            _ => {
                return Err(CompileError::syntax(
                    "`assert ... is` patterns only support a single identifier or `_`".to_string(),
                    value.span,
                ));
            }
        };
        Ok(vec![PatternArg::Positional(Spanned::new(pattern, value.span))])
    }

    /// Parse a `break` statement.
    fn break_stmt(&mut self) -> Result<Statement, CompileError> {
        self.expect(&TokenKind::Keyword(KeywordId::Break), "Expected 'break'")?;
        let value = if !self.check(&TokenKind::Newline)
            && !self.check(&TokenKind::Dedent)
            && !self.check(&TokenKind::Keyword(KeywordId::Case))
        {
            Some(self.expression()?)
        } else {
            None
        };
        Ok(Statement::Break(value))
    }

    /// Parse an `if` / `while` condition, including RFC 049 let-pattern forms.
    ///
    /// Ordinary boolean conditions stay as expression-backed conditions, while `let PATTERN = VALUE` is captured
    /// explicitly so later stages can apply match-equivalent semantics without reparsing the surface spelling.
    fn control_flow_condition(&mut self, allow_pattern_alternation: bool) -> Result<Condition, CompileError> {
        if self.match_token(&TokenKind::Keyword(KeywordId::Let)) {
            let pattern = self.pattern()?;
            if !allow_pattern_alternation && Self::pattern_contains_alternation(&pattern.node) {
                return Err(CompileError::syntax(
                    "Pattern alternation is only supported in match arms and if let patterns".to_string(),
                    pattern.span,
                ));
            }
            self.expect(
                &TokenKind::Operator(OperatorId::Eq),
                "Expected '=' after let pattern in control-flow condition",
            )?;
            let value = self.expression()?;
            Ok(Condition::Let { pattern, value })
        } else {
            Ok(Condition::Expr(self.expression()?))
        }
    }

    /// Return whether a parsed pattern contains an alternation at any nested level.
    fn pattern_contains_alternation(pattern: &Pattern) -> bool {
        match pattern {
            Pattern::Or(_) => true,
            Pattern::Group(inner) => Self::pattern_contains_alternation(&inner.node),
            Pattern::Tuple(items) => items.iter().any(|item| Self::pattern_contains_alternation(&item.node)),
            Pattern::Constructor(_, args) => args.iter().any(|arg| match arg {
                PatternArg::Positional(pattern) => Self::pattern_contains_alternation(&pattern.node),
                PatternArg::Named(_, pattern) => Self::pattern_contains_alternation(&pattern.node),
            }),
            Pattern::Binding(_) | Pattern::Wildcard | Pattern::Literal(_) => false,
        }
    }

    /// Parse a statement-form `if`, including `if let` conditions that accept pattern alternation.
    fn if_stmt(&mut self) -> Result<Statement, CompileError> {
        self.expect(&TokenKind::Keyword(KeywordId::If), "Expected 'if'")?;
        let condition = self.control_flow_condition(true)?;
        self.expect(
            &TokenKind::Punctuation(PunctuationId::Colon),
            "Expected ':' after if condition",
        )?;
        self.expect(&TokenKind::Newline, "Expected newline after ':'")?;
        self.expect_suite_indent("Expected indented block")?;
        let then_body = self.block()?;
        self.expect(&TokenKind::Dedent, "Expected dedent after if body")?;

        if matches!(condition, Condition::Let { .. })
            && (self.check_keyword(KeywordId::Elif) || self.check_keyword(KeywordId::Else))
        {
            let branch = if self.check_keyword(KeywordId::Elif) {
                "`elif`"
            } else {
                "`else`"
            };
            return Err(CompileError::syntax(
                format!("`if let` does not support {branch} branches"),
                self.current_span(),
            )
            .with_hint("Use `match` when the non-match case matters"));
        }

        let mut elif_branches = vec![];
        while self.match_token(&TokenKind::Keyword(KeywordId::Elif)) {
            let elif_condition = self.expression()?;
            self.expect(
                &TokenKind::Punctuation(PunctuationId::Colon),
                "Expected ':' after elif condition",
            )?;
            self.expect(&TokenKind::Newline, "Expected newline after ':'")?;
            self.expect_suite_indent("Expected indented block")?;
            let elif_body = self.block()?;
            self.expect(&TokenKind::Dedent, "Expected dedent after elif body")?;
            elif_branches.push((elif_condition, elif_body));
        }

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

        Ok(Statement::If(IfStmt {
            condition,
            then_body,
            elif_branches,
            else_body,
        }))
    }

    /// Parse a statement-form `while`, keeping `while let` on the narrower non-alternation pattern surface.
    fn while_stmt(&mut self) -> Result<Statement, CompileError> {
        self.expect(&TokenKind::Keyword(KeywordId::While), "Expected 'while'")?;
        let condition = self.control_flow_condition(false)?;
        self.expect(
            &TokenKind::Punctuation(PunctuationId::Colon),
            "Expected ':' after while condition",
        )?;
        self.expect(&TokenKind::Newline, "Expected newline after ':'")?;
        self.expect_suite_indent("Expected indented block")?;
        let body = self.block()?;
        self.expect(&TokenKind::Dedent, "Expected dedent after while body")?;

        Ok(Statement::While(WhileStmt { condition, body }))
    }

    /// Parse a `loop` statement.
    fn loop_stmt(&mut self) -> Result<Statement, CompileError> {
        self.expect(&TokenKind::Keyword(KeywordId::Loop), "Expected 'loop'")?;
        self.expect(&TokenKind::Punctuation(PunctuationId::Colon), "Expected ':' after loop")?;
        self.expect(&TokenKind::Newline, "Expected newline after ':'")?;
        self.expect(&TokenKind::Indent, "Expected indented block")?;
        let body = self.block()?;
        self.expect(&TokenKind::Dedent, "Expected dedent after loop body")?;

        Ok(Statement::Loop(LoopStmt { body }))
    }

    /// Parse a `for` statement.
    fn for_stmt(&mut self) -> Result<Statement, CompileError> {
        self.expect(&TokenKind::Keyword(KeywordId::For), "Expected 'for'")?;
        let pattern = self.for_binding_pattern()?;
        self.expect(&TokenKind::Keyword(KeywordId::In), "Expected 'in' after for variable")?;
        let iter = self.expression()?;
        self.expect(
            &TokenKind::Punctuation(PunctuationId::Colon),
            "Expected ':' after for expression",
        )?;
        self.expect(&TokenKind::Newline, "Expected newline after ':'")?;
        self.expect_suite_indent("Expected indented block")?;
        let body = self.block()?;
        self.expect(&TokenKind::Dedent, "Expected dedent after for body")?;

        Ok(Statement::For(ForStmt { pattern, iter, body }))
    }

    /// Parse an `unsafe:` acknowledgment region without introducing a new local scope.
    fn unsafe_stmt(&mut self) -> Result<Statement, CompileError> {
        self.advance();
        self.expect(
            &TokenKind::Punctuation(PunctuationId::Colon),
            "Expected ':' after unsafe",
        )?;
        self.expect(&TokenKind::Newline, "Expected newline after 'unsafe:'")?;
        self.expect_suite_indent("Expected indented unsafe block")?;
        let body = self.block()?;
        self.expect(&TokenKind::Dedent, "Expected dedent after unsafe body")?;
        Ok(Statement::Unsafe(UnsafeStmt { body }))
    }

    /// Parse the restricted binding-pattern subset accepted in `for` headers.
    ///
    /// Match patterns stay broader; loop bindings only need identifiers, `_`, and comma-separated tuple bindings.
    pub(super) fn for_binding_pattern(&mut self) -> Result<Spanned<Pattern>, CompileError> {
        let start = self.current_span().start;
        let first = self.for_binding_pattern_item()?;

        if !self.match_punct(PunctuationId::Comma) {
            return Ok(first);
        }

        let mut items = vec![first];
        loop {
            items.push(self.for_binding_pattern_item()?);
            if !self.match_punct(PunctuationId::Comma) {
                break;
            }
        }

        let end = items.last().map(|item| item.span.end).unwrap_or(start);
        Ok(Spanned::new(Pattern::Tuple(items), Span::new(start, end)))
    }

    /// Parse one loop-binding item in a `for` header.
    fn for_binding_pattern_item(&mut self) -> Result<Spanned<Pattern>, CompileError> {
        let span = self.current_span();
        if matches!(&self.peek().kind, TokenKind::Ident(name) if name == "_") {
            self.advance();
            return Ok(Spanned::new(Pattern::Wildcard, span));
        }

        let name = self.identifier()?;
        Ok(Spanned::new(Pattern::Binding(name), span))
    }
}
