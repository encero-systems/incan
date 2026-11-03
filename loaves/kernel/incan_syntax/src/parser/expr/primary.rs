/// Primary expression parsing, vocab expression blocks, braced vocab bodies, and partial expressions.
impl<'a> Parser<'a> {
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
                is_declaration_owned_clause: spec.is_declaration_owned_clause,
                dependency_key: spec.dependency_key,
                activation_namespace: spec.activation_namespace,
                surface_kind: spec.surface_kind,
                compound_tokens: spec_compound_tokens,
                placement: spec.placement,
                clause_body_kind: spec.clause_body_kind,
            },
            decorators: Vec::new(),
            signature_head: None,
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
                let stmt = self.assignment_or_expr_item(TupleValueSpelling::SingleExpression)?;
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
        Ok(Some(Spanned::new(Statement::VocabBlock(block), Span::new(start, end))))
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
                is_declaration_owned_clause: spec.is_declaration_owned_clause,
                dependency_key: spec.dependency_key,
                activation_namespace: spec.activation_namespace,
                surface_kind: spec.surface_kind,
                compound_tokens: spec_compound_tokens,
                placement: spec.placement,
                clause_body_kind: spec.clause_body_kind,
            },
            decorators: Vec::new(),
            signature_head: None,
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
            || sibling_parent_keyword.is_some_and(|parent| self.current_starts_vocab_block_for_parent(Some(parent)))
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
}
