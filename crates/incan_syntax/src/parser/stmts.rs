/// Statement parsing methods.
///
/// This chunk parses statement forms (e.g. `if`, `while`, `for`, `return`, assignments)
/// as well as indentation-based blocks.
///
/// ## Notes
/// - Block parsing relies on `Indent` / `Dedent` layout tokens produced by the lexer.
impl<'a> Parser<'a> {
    // ========================================================================
    // Statements
    // ========================================================================

    fn block(&mut self) -> Result<Vec<Spanned<Statement>>, CompileError> {
        let mut stmts = Vec::new();
        let mut next_leading = self.consume_inter_statement_blank_prefix();
        while !self.check(&TokenKind::Dedent) && !self.is_at_end() {
            let mut stmt = self.statement()?;
            stmt.leading_blank_lines = next_leading;
            stmts.push(stmt);
            next_leading = self.consume_inter_statement_blank_prefix();
            if next_leading > 0
                && self.check(&TokenKind::Dedent)
                && self.dedent_is_followed_by_outer_statement()
            {
                self.pending_dedent_blank_lines = self.pending_dedent_blank_lines.max(next_leading);
                next_leading = 0;
            }
        }
        Ok(stmts)
    }

    /// Return whether the current `Dedent` leads back to an outer statement, not a declaration/member boundary.
    fn dedent_is_followed_by_outer_statement(&self) -> bool {
        let mut idx = self.pos;
        while matches!(self.tokens.get(idx).map(|token| &token.kind), Some(TokenKind::Dedent)) {
            idx += 1;
        }

        !matches!(
            self.tokens.get(idx).map(|token| &token.kind),
            None | Some(TokenKind::Eof)
                | Some(TokenKind::Keyword(
                    KeywordId::Async
                        | KeywordId::Class
                        | KeywordId::Const
                        | KeywordId::Def
                        | KeywordId::Enum
                        | KeywordId::From
                        | KeywordId::Import
                        | KeywordId::Model
                        | KeywordId::Newtype
                        | KeywordId::Pub
                        | KeywordId::Rust
                        | KeywordId::Static
                        | KeywordId::Trait
                        | KeywordId::Type
                ))
        )
    }

    fn statement(&mut self) -> Result<Spanned<Statement>, CompileError> {
        let start = self.current_span().start;

        let stmt = if self.check_keyword(KeywordId::Return) {
            self.return_stmt()?
        } else if self.check_keyword(KeywordId::If) {
            self.if_stmt()?
        } else if self.check_keyword(KeywordId::Loop) {
            self.loop_stmt()?
        } else if self.check_keyword(KeywordId::While) {
            self.while_stmt()?
        } else if self.check_keyword(KeywordId::For) {
            self.for_stmt()?
        } else if self.is_assert_statement_keyword() {
            self.assert_stmt()?
        } else if let Some(vocab_block) = self.try_vocab_block_statement()? {
            vocab_block
        } else if let Some(surface_stmt) = self.try_surface_keyword_statement()? {
            surface_stmt
        } else if self.check_keyword(KeywordId::Break) {
            self.break_stmt()?
        } else if self.check_keyword(KeywordId::Continue) {
            self.advance();
            Statement::Continue
        } else if self.check_keyword(KeywordId::Pass) {
            self.advance();
            Statement::Pass
        } else if self.check_keyword(KeywordId::Static) {
            return Err(errors::static_only_allowed_at_module_scope(self.current_span()));
        } else if self.check(&TokenKind::Punctuation(PunctuationId::Ellipsis)) {
            // ... is equivalent to pass (Python-style placeholder)
            self.advance();
            Statement::Pass
        } else if self.check_keyword(KeywordId::Let) || self.check_keyword(KeywordId::Mut) {
            self.assignment_stmt()?
        } else {
            // Could be assignment or expression
            self.assignment_or_expr_stmt()?
        };

        let end = self.tokens[self.pos.saturating_sub(1)].span.end;
        Ok(Spanned::new(stmt, Span::new(start, end)))
    }

    /// Parse a single inline statement (for use in inline case arms)
    /// Supports: return, expression statements, pass
    fn inline_statement(&mut self) -> Result<Spanned<Statement>, CompileError> {
        let start = self.current_span().start;

        let stmt = if self.check_keyword(KeywordId::Return) {
            self.advance();
            let expr = if !self.check(&TokenKind::Newline)
                && !self.check(&TokenKind::Keyword(KeywordId::Case))
                && !self.check(&TokenKind::Dedent)
            {
                Some(self.expression()?)
            } else {
                None
            };
            Statement::Return(expr)
        } else if self.check_keyword(KeywordId::Break) {
            self.break_stmt()?
        } else if self.check_keyword(KeywordId::Pass) || self.check(&TokenKind::Punctuation(PunctuationId::Ellipsis)) {
            self.advance();
            Statement::Pass
        } else if self.check_keyword(KeywordId::Static) {
            return Err(errors::static_only_allowed_at_module_scope(self.current_span()));
        } else if self.is_assert_statement_keyword() {
            self.assert_stmt()?
        } else if let Some(surface_stmt) = self.try_surface_keyword_statement()? {
            surface_stmt
        } else {
            // Expression statement
            let expr = self.expression()?;
            Statement::Expr(expr)
        };

        let end = self.tokens[self.pos.saturating_sub(1)].span.end;
        Ok(Spanned::new(stmt, Span::new(start, end)))
    }

    /// Parse a raw vocab block statement driven by imported keyword registrations.
    fn try_vocab_block_statement(&mut self) -> Result<Option<Statement>, CompileError> {
        let decorators = if self.check_punct(PunctuationId::At) {
            self.decorators()?
        } else {
            Vec::new()
        };

        let keyword_name = match &self.peek().kind {
            TokenKind::Ident(name) => name.clone(),
            TokenKind::Keyword(id) => incan_core::lang::keywords::as_str(*id).to_string(),
            _ => {
                if decorators.is_empty() {
                    return Ok(None);
                }
                return Err(errors::expected_token_message(
                    "Expected vocab block keyword after decorator",
                    &format!("{:?}", self.peek().kind),
                    self.current_span(),
                ));
            }
        };

        let parent_keyword = self.vocab_block_stack.last().cloned();
        let Some(spec) = self.find_active_vocab_block_spec(&keyword_name, parent_keyword.as_deref()) else {
            if decorators.is_empty() {
                return Ok(None);
            }
            return Err(errors::expected_token_message(
                "Decorator can only target a registered vocab block keyword",
                &format!("{:?}", self.peek().kind),
                self.current_span(),
            ));
        };
        let spec_keyword_name = spec.keyword_name.clone();
        let spec_dependency_key = spec.dependency_key.clone();
        let spec_activation_namespace = spec.activation_namespace.clone();
        let spec_surface_kind = spec.surface_kind;
        let spec_placement = spec.placement.clone();
        let spec_valid_decorators = spec.valid_decorators.clone();

        // Avoid committing to vocab-block parsing unless a top-level header-delimiting `:` is visible ahead. This
        // preserves `assignment_or_expr_stmt` fallback for statements like `route = "/health"`, `route(args)`, and
        // `route: str = "/health"` when `route` is an imported vocab keyword.
        if decorators.is_empty() && !self.has_top_level_colon_before_statement_end(self.pos + 1) {
            return Ok(None);
        }

        self.advance();

        let mut header_args = Vec::new();
        if !self.check_punct(PunctuationId::Colon) {
            header_args.push(self.expression()?);
            while self.match_punct(PunctuationId::Comma) {
                header_args.push(self.expression()?);
            }
        }
        self.expect_punct(PunctuationId::Colon, "Expected ':' after vocab block header")?;
        self.expect(&TokenKind::Newline, "Expected newline after ':'")?;
        self.expect_suite_indent("Expected indented block after vocab keyword")?;

        if !spec_valid_decorators.is_empty() {
            for decorator in &decorators {
                let decorator_name = decorator.node.name.as_str();
                let decorator_full_name = decorator.node.path.segments.join(".");
                let is_valid = spec_valid_decorators.iter().any(|allowed| {
                    let normalized = allowed.trim().trim_start_matches('@');
                    normalized == decorator_name || normalized == decorator_full_name
                });
                if !is_valid {
                    return Err(errors::expected_token_message(
                        &format!(
                            "Decorator `{decorator_full_name}` is not valid on vocab block `{}`",
                            spec_keyword_name
                        ),
                        &format!("{:?}", decorator.node),
                        decorator.span,
                    ));
                }
            }
        }

        self.vocab_block_stack.push(keyword_name.clone());
        let body = self.block();
        self.vocab_block_stack.pop();
        let body = body?;
        self.expect(&TokenKind::Dedent, "Expected dedent after vocab block body")?;

        Ok(Some(Statement::VocabBlock(VocabBlockStmt {
            keyword: keyword_name,
            keyword_binding: VocabKeywordBinding {
                dependency_key: spec_dependency_key,
                activation_namespace: spec_activation_namespace,
                surface_kind: spec_surface_kind,
                placement: spec_placement,
            },
            decorators,
            header_args,
            body,
        })))
    }

    /// Return `true` if there is a top-level block-header `:` before the current statement ends.
    ///
    /// This is used as a lookahead gate for imported vocab block keywords so we only consume the keyword token when the
    /// block header delimiter is actually present. We require the matching `:` to terminate the header immediately,
    /// which avoids stealing ordinary assignments with type annotations such as `route: str = "/health"`.
    fn has_top_level_colon_before_statement_end(&self, mut idx: usize) -> bool {
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
                TokenKind::Punctuation(PunctuationId::LBrace) => brace_depth += 1,
                TokenKind::Punctuation(PunctuationId::RBrace) => {
                    brace_depth = brace_depth.saturating_sub(1);
                }
                TokenKind::Punctuation(PunctuationId::Colon)
                    if paren_depth == 0 && bracket_depth == 0 && brace_depth == 0 =>
                {
                    return matches!(
                        self.tokens.get(idx + 1).map(|token| &token.kind),
                        Some(TokenKind::Newline)
                    );
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

    fn find_active_vocab_block_spec(
        &self,
        keyword_name: &str,
        parent_keyword: Option<&str>,
    ) -> Option<&ActiveImportedKeywordSpec> {
        let specs = self.active_imported_keyword_specs.get(keyword_name)?;
        specs.iter().find(|spec| {
            matches!(
                spec.surface_kind,
                incan_vocab::KeywordSurfaceKind::BlockDeclaration
                    | incan_vocab::KeywordSurfaceKind::BlockContextKeyword
                    | incan_vocab::KeywordSurfaceKind::SubBlock
            ) && match (&spec.placement, parent_keyword) {
                (incan_vocab::KeywordPlacement::TopLevel, None) => true,
                (incan_vocab::KeywordPlacement::TopLevel, Some(_)) => false,
                (incan_vocab::KeywordPlacement::InBlock(allowed), Some(parent)) => {
                    allowed.iter().any(|value| value == parent)
                }
                (incan_vocab::KeywordPlacement::InBlock(_), None) => false,
                _ => false,
            }
        })
    }

    /// Parse a generic soft-keyword statement payload (`kw expr[, expr]`) and hand off to semantics.
    fn try_surface_keyword_statement(&mut self) -> Result<Option<Statement>, CompileError> {
        let Some(id) = self.current_surface_keyword(KeywordSurfaceKind::StatementKeywordArgs) else {
            return Ok(None);
        };
        self.advance();
        let first = self.expression()?;
        let mut args = vec![first];
        if self.match_token(&TokenKind::Punctuation(PunctuationId::Comma)) {
            args.push(self.expression()?);
        }
        Ok(Some(Statement::Surface(SurfaceStmt {
            key: SurfaceFeatureKey::SoftKeyword(id),
            payload: SurfaceStmtPayload::KeywordArgs(args),
        })))
    }

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
            TokenKind::Ident(name) if name == incan_core::lang::keywords::as_str(KeywordId::Assert)
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
        Ok(AssertKind::IsPattern {
            value: *left,
            pattern,
        })
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
                Pattern::Constructor("None".to_string(), Vec::new()),
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
                Ok(Spanned::new(Pattern::Constructor(name, pattern_args), expr.span))
            }
            _ => Err(CompileError::syntax(
                "`assert ... is` only supports Some/Ok/Err/None patterns".to_string(),
                expr.span,
            )),
        }
    }

    /// Convert positional call arguments into the single binding/wildcard pattern allowed by RFC 018.
    fn assert_pattern_args(
        &self,
        args: Vec<CallArg>,
        span: Span,
    ) -> Result<Vec<PatternArg>, CompileError> {
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
    /// Ordinary boolean conditions stay as expression-backed conditions, while
    /// `let PATTERN = VALUE` is captured explicitly so later stages can apply
    /// match-equivalent semantics without reparsing the surface spelling.
    fn control_flow_condition(&mut self) -> Result<Condition, CompileError> {
        if self.match_token(&TokenKind::Keyword(KeywordId::Let)) {
            let pattern = self.pattern()?;
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

    fn if_stmt(&mut self) -> Result<Statement, CompileError> {
        self.expect(&TokenKind::Keyword(KeywordId::If), "Expected 'if'")?;
        let condition = self.control_flow_condition()?;
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

    fn while_stmt(&mut self) -> Result<Statement, CompileError> {
        self.expect(&TokenKind::Keyword(KeywordId::While), "Expected 'while'")?;
        let condition = self.control_flow_condition()?;
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

    fn loop_stmt(&mut self) -> Result<Statement, CompileError> {
        self.expect(&TokenKind::Keyword(KeywordId::Loop), "Expected 'loop'")?;
        self.expect(
            &TokenKind::Punctuation(PunctuationId::Colon),
            "Expected ':' after loop",
        )?;
        self.expect(&TokenKind::Newline, "Expected newline after ':'")?;
        self.expect(&TokenKind::Indent, "Expected indented block")?;
        let body = self.block()?;
        self.expect(&TokenKind::Dedent, "Expected dedent after loop body")?;

        Ok(Statement::Loop(LoopStmt { body }))
    }

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

    /// Parse the restricted binding-pattern subset accepted in `for` headers.
    ///
    /// Match patterns stay broader; loop bindings only need identifiers, `_`, and comma-separated tuple bindings.
    fn for_binding_pattern(&mut self) -> Result<Spanned<Pattern>, CompileError> {
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

        let end = items
            .last()
            .map(|item| item.span.end)
            .unwrap_or(start);
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

    fn assignment_stmt(&mut self) -> Result<Statement, CompileError> {
        let binding = if self.match_token(&TokenKind::Keyword(KeywordId::Let)) {
            BindingKind::Let
        } else if self.match_token(&TokenKind::Keyword(KeywordId::Mut)) {
            BindingKind::Mutable
        } else {
            BindingKind::Inferred
        };

        let name = self.identifier()?;

        // Check for tuple unpacking: a, b, c = expr
        if self.match_token(&TokenKind::Punctuation(PunctuationId::Comma)) {
            let mut names = vec![name];
            loop {
                names.push(self.identifier()?);
                if !self.match_token(&TokenKind::Punctuation(PunctuationId::Comma)) {
                    break;
                }
            }
            self.expect(&TokenKind::Operator(OperatorId::Eq), "Expected '=' in tuple unpacking")?;
            let value = self.expression()?;
            return Ok(Statement::TupleUnpack(TupleUnpackStmt { binding, names, value }));
        }

        let ty = if self.match_token(&TokenKind::Punctuation(PunctuationId::Colon)) {
            Some(self.type_expr()?)
        } else {
            None
        };
        self.expect(&TokenKind::Operator(OperatorId::Eq), "Expected '=' in assignment")?;

        // Check for chained assignment: x = y = z = 5
        // Collect all targets before the final value
        let mut targets = vec![name];
        while let TokenKind::Ident(_) = &self.peek().kind {
            if self.peek_next().kind == TokenKind::Operator(OperatorId::Eq) {
                targets.push(self.identifier()?);
                self.expect(
                    &TokenKind::Operator(OperatorId::Eq),
                    "Expected '=' in chained assignment",
                )?;
            } else {
                break;
            }
        }

        let value = self.expression()?;

        // If we have multiple targets, create a ChainedAssignment
        if targets.len() > 1 {
            Ok(Statement::ChainedAssignment(ChainedAssignmentStmt {
                binding,
                targets,
                value,
            }))
        } else {
            Ok(Statement::Assignment(AssignmentStmt {
                binding,
                name: targets.remove(0),
                ty,
                value,
            }))
        }
    }

    /// Parse either an assignment-like statement or a plain expression statement.
    fn assignment_or_expr_stmt(&mut self) -> Result<Statement, CompileError> {
        // Look for `ident = expr` or `ident, ident = expr` pattern (simple or tuple assignment)
        if let TokenKind::Ident(_) = &self.peek().kind {
            // Check if next is = or : (for assignment) or , (for tuple unpacking)
            if self.peek_next().kind == TokenKind::Operator(OperatorId::Eq)
                || (self.peek_next().kind == TokenKind::Punctuation(PunctuationId::Colon)
                    && !self.active_scoped_glyph_starts_at_offset(1))
                || self.peek_next().kind == TokenKind::Punctuation(PunctuationId::Comma)
            {
                return self.assignment_stmt();
            }
            // Check for compound assignment: ident += expr, ident -= expr, etc.
            let compound_op = match &self.peek_next().kind {
                TokenKind::Operator(OperatorId::PlusEq) => Some(CompoundOp::Add),
                TokenKind::Operator(OperatorId::MinusEq) => Some(CompoundOp::Sub),
                TokenKind::Operator(OperatorId::StarEq) => Some(CompoundOp::Mul),
                TokenKind::Operator(OperatorId::SlashEq) => Some(CompoundOp::Div),
                TokenKind::Operator(OperatorId::SlashSlashEq) => Some(CompoundOp::FloorDiv),
                TokenKind::Operator(OperatorId::PercentEq) => Some(CompoundOp::Mod),
                _ => None,
            };
            if let Some(op) = compound_op {
                let name = self.identifier()?;
                self.advance(); // consume the compound operator
                let value = self.expression()?;
                return Ok(Statement::CompoundAssignment(CompoundAssignmentStmt {
                    name,
                    op,
                    value,
                }));
            }
        }

        // Parse the expression (could be field access like self.field or index like arr[i])
        let expr = self.expression()?;

        // Check for tuple assignment: expr, expr, ... = value
        // This handles patterns like: arr[i], arr[j] = arr[j], arr[i]
        if self.match_token(&TokenKind::Punctuation(PunctuationId::Comma)) {
            let mut targets = vec![expr];
            loop {
                let target = self.expression()?;
                targets.push(target);
                if !self.match_token(&TokenKind::Punctuation(PunctuationId::Comma)) {
                    break;
                }
            }
            self.expect(&TokenKind::Operator(OperatorId::Eq), "Expected '=' in tuple assignment")?;
            let value = self.expression()?;
            return Ok(Statement::TupleAssign(TupleAssignStmt { targets, value }));
        }

        // Check for assignment: expr.field = value or expr[index] = value
        if self.match_token(&TokenKind::Operator(OperatorId::Eq)) {
            match expr.node {
                Expr::Field(object, field) => {
                    let value = self.expression()?;
                    return Ok(Statement::FieldAssignment(FieldAssignmentStmt {
                        target_span: expr.span,
                        object: *object,
                        field,
                        value,
                    }));
                }
                Expr::Index(object, index) => {
                    let value = self.expression()?;
                    return Ok(Statement::IndexAssignment(IndexAssignmentStmt {
                        object: *object,
                        index: *index,
                        value,
                    }));
                }
                _ => {
                    return Err(errors::invalid_assignment_target(expr.span));
                }
            }
        }

        // Check for compound assignment on field/index: expr.field += value, expr[i] -= value
        let compound_op = match &self.peek().kind {
            TokenKind::Operator(OperatorId::PlusEq) => Some(CompoundOp::Add),
            TokenKind::Operator(OperatorId::MinusEq) => Some(CompoundOp::Sub),
            TokenKind::Operator(OperatorId::StarEq) => Some(CompoundOp::Mul),
            TokenKind::Operator(OperatorId::SlashEq) => Some(CompoundOp::Div),
            TokenKind::Operator(OperatorId::SlashSlashEq) => Some(CompoundOp::FloorDiv),
            TokenKind::Operator(OperatorId::PercentEq) => Some(CompoundOp::Mod),
            _ => None,
        };
        if let Some(op) = compound_op {
            self.advance(); // consume the compound operator
            let rhs = self.expression()?;
            match expr.node {
                Expr::Field(object, field) => {
                    // Convert field += rhs to field = field + rhs
                    let field_expr = Spanned::new(Expr::Field(object.clone(), field.clone()), expr.span);
                    let bin_op = match op {
                        CompoundOp::Add => BinaryOp::Add,
                        CompoundOp::Sub => BinaryOp::Sub,
                        CompoundOp::Mul => BinaryOp::Mul,
                        CompoundOp::Div => BinaryOp::Div,
                        CompoundOp::FloorDiv => BinaryOp::FloorDiv,
                        CompoundOp::Mod => BinaryOp::Mod,
                    };
                    let new_value = Spanned::new(Expr::Binary(Box::new(field_expr), bin_op, Box::new(rhs)), expr.span);
                    return Ok(Statement::FieldAssignment(FieldAssignmentStmt {
                        target_span: expr.span,
                        object: *object,
                        field,
                        value: new_value,
                    }));
                }
                Expr::Index(object, index) => {
                    // Convert arr[i] += rhs to arr[i] = arr[i] + rhs
                    let index_expr = Spanned::new(Expr::Index(object.clone(), index.clone()), expr.span);
                    let bin_op = match op {
                        CompoundOp::Add => BinaryOp::Add,
                        CompoundOp::Sub => BinaryOp::Sub,
                        CompoundOp::Mul => BinaryOp::Mul,
                        CompoundOp::Div => BinaryOp::Div,
                        CompoundOp::FloorDiv => BinaryOp::FloorDiv,
                        CompoundOp::Mod => BinaryOp::Mod,
                    };
                    let new_value = Spanned::new(Expr::Binary(Box::new(index_expr), bin_op, Box::new(rhs)), expr.span);
                    return Ok(Statement::IndexAssignment(IndexAssignmentStmt {
                        object: *object,
                        index: *index,
                        value: new_value,
                    }));
                }
                Expr::Ident(name) => {
                    // Fallback: simple ident compound assignment
                    return Ok(Statement::CompoundAssignment(CompoundAssignmentStmt {
                        name,
                        op,
                        value: rhs,
                    }));
                }
                _ => {
                    return Err(errors::invalid_compound_assignment_target(expr.span));
                }
            }
        }

        // Otherwise it's an expression statement
        Ok(Statement::Expr(expr))
    }

}
