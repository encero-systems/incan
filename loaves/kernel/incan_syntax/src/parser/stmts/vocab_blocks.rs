/// Vocab block statements: signature heads, decorators, indented and inline bodies, and compound keywords.
impl<'a> Parser<'a> {
    /// Parse one raw imported vocabulary block with decorators already collected by the owning grammar position.
    fn try_vocab_block(&mut self, decorators: Vec<Spanned<Decorator>>) -> Result<Option<VocabBlockStmt>, CompileError> {
        let keyword_name = match &self.peek().kind {
            TokenKind::Ident(name) => name.clone(),
            TokenKind::Keyword(id) => incan_lang::lang::keywords::as_str(*id).to_string(),
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
        let spec_compound_tokens = spec.compound_tokens.clone();
        let spec_dependency_key = spec.dependency_key.clone();
        let spec_activation_namespace = spec.activation_namespace.clone();
        let spec_surface_kind = spec.surface_kind;
        let spec_placement = spec.placement.clone();
        let spec_declaration_head_kind = spec.declaration_head_kind;
        let spec_valid_decorators = spec.valid_decorators.clone();
        let spec_clause_body_kind = spec.clause_body_kind;
        let spec_is_declaration_owned_clause = spec.is_declaration_owned_clause;
        let spec_expression_item_modifiers = spec.expression_item_modifiers.clone();

        // Avoid committing to vocab-block parsing unless a top-level header-delimiting `:` is visible ahead. This
        // preserves `assignment_or_expr_stmt` fallback for statements like `route = "/health"`, `route(args)`, and
        // `route: str = "/health"` when `route` is an imported vocab keyword. Clause keywords inside an owning vocab
        // block may still use an inline body (`FROM orders`), but only when the registered clause body kind makes that
        // expression payload explicit.
        let has_header_colon = self.has_top_level_colon_before_statement_end(self.pos + 1);
        let parses_inline_clause = decorators.is_empty()
            && parent_keyword.is_some()
            && matches!(
                spec_surface_kind,
                incan_vocab::KeywordSurfaceKind::BlockContextKeyword | incan_vocab::KeywordSurfaceKind::SubBlock
            )
            && matches!(
                spec_clause_body_kind,
                Some(incan_vocab::ClauseBodyKind::Expression | incan_vocab::ClauseBodyKind::ExpressionList)
            );
        if decorators.is_empty() && !has_header_colon && !parses_inline_clause {
            return Ok(None);
        }

        self.advance();
        self.consume_vocab_compound_tokens(&spec_compound_tokens)?;

        let mut header_args = Vec::new();
        let mut signature_head = None;
        let (body, body_item_trailing_commas) = if parses_inline_clause && !has_header_colon {
            let body = self.parse_inline_vocab_clause_body(
                &keyword_name,
                spec_clause_body_kind,
                spec_expression_item_modifiers,
            )?;
            let body_item_trailing_commas = vec![false; body.len()];
            (body, body_item_trailing_commas)
        } else {
            if matches!(spec_declaration_head_kind, incan_vocab::DeclarationHeadKind::Signature) {
                signature_head = Some(self.parse_vocab_signature_head()?);
            } else if !self.check_punct(PunctuationId::Colon) {
                header_args.push(self.expression()?);
                while self.match_punct(PunctuationId::Comma) {
                    header_args.push(self.expression()?);
                }
            }
            self.expect_punct(PunctuationId::Colon, "Expected ':' after vocab block header")?;
            self.expect(&TokenKind::Newline, "Expected newline after ':'")?;
            self.expect_suite_indent("Expected indented block after vocab keyword")?;

            Self::validate_vocab_block_decorators(&spec_keyword_name, &spec_valid_decorators, &decorators)?;
            let body =
                self.parse_indented_vocab_body(&keyword_name, spec_clause_body_kind, spec_expression_item_modifiers)?;
            let body_item_trailing_commas = vec![false; body.len()];
            (body, body_item_trailing_commas)
        };

        Ok(Some(VocabBlockStmt {
            keyword: keyword_name,
            keyword_binding: VocabKeywordBinding {
                is_declaration_owned_clause: spec_is_declaration_owned_clause,
                dependency_key: spec_dependency_key,
                activation_namespace: spec_activation_namespace,
                surface_kind: spec_surface_kind,
                compound_tokens: spec_compound_tokens,
                placement: spec_placement,
                clause_body_kind: spec_clause_body_kind,
            },
            decorators,
            signature_head,
            header_args,
            body,
            body_item_trailing_commas,
            not_checked: false,
        }))
    }

    /// Parse a descriptor-selected vocabulary signature before the block's header colon.
    fn parse_vocab_signature_head(&mut self) -> Result<VocabSignatureHead, CompileError> {
        let name = self.identifier()?;
        self.expect_punct(PunctuationId::LParen, "Expected '(' after vocabulary signature name")?;
        let parameters = self.params()?;
        self.expect_punct(
            PunctuationId::RParen,
            "Expected ')' after vocabulary signature parameters",
        )?;
        let return_type = if self.match_punct(PunctuationId::Arrow) {
            Some(self.type_expr()?)
        } else {
            None
        };
        Ok(VocabSignatureHead {
            name,
            parameters,
            return_type,
        })
    }

    /// Validate decorators against a vocabulary descriptor's allowed short or qualified names.
    fn validate_vocab_block_decorators(
        keyword: &str,
        allowed: &[String],
        decorators: &[Spanned<Decorator>],
    ) -> Result<(), CompileError> {
        if allowed.is_empty() {
            return Ok(());
        }
        for decorator in decorators {
            let decorator_name = decorator.node.name.as_str();
            let decorator_full_name = decorator.node.path.segments.join(".");
            let is_valid = allowed.iter().any(|allowed| {
                let normalized = allowed.trim().trim_start_matches('@');
                normalized == decorator_name || normalized == decorator_full_name
            });
            if !is_valid {
                return Err(errors::expected_token_message(
                    &format!("Decorator `{decorator_full_name}` is not valid on vocab block `{keyword}`"),
                    &format!("{:?}", decorator.node),
                    decorator.span,
                ));
            }
        }
        Ok(())
    }

    /// Parse a vocabulary body after its indent, selecting descriptor-owned embedded grammar when claimed.
    ///
    /// Ordinary blocks restore all vocabulary context stacks even on failure. Both grammars leave the matching dedent
    /// for this boundary to consume, so callers receive a complete body with the same outer token position.
    fn parse_indented_vocab_body(
        &mut self,
        keyword: &str,
        clause_body_kind: Option<incan_vocab::ClauseBodyKind>,
        expression_item_modifiers: Vec<incan_vocab::ExpressionItemModifierSurface>,
    ) -> Result<Vec<Spanned<Statement>>, CompileError> {
        if let Some(body) = self.try_embedded_fragment_body(keyword)? {
            self.expect(&TokenKind::Dedent, "Expected dedent after vocab block body")?;
            return Ok(body);
        }

        self.vocab_block_stack.push(keyword.to_string());
        self.vocab_body_kind_stack.push(clause_body_kind);
        self.vocab_expression_item_modifier_stack
            .push(expression_item_modifiers);
        let body = self.block();
        self.vocab_block_stack.pop();
        self.vocab_body_kind_stack.pop();
        self.vocab_expression_item_modifier_stack.pop();
        let body = body?;
        self.expect(&TokenKind::Dedent, "Expected dedent after vocab block body")?;
        Ok(body)
    }

    /// Parse an indentation-line clause with an inline expression payload, such as `FROM orders`.
    fn parse_inline_vocab_clause_body(
        &mut self,
        keyword_name: &str,
        clause_body_kind: Option<incan_vocab::ClauseBodyKind>,
        expression_item_modifiers: Vec<incan_vocab::ExpressionItemModifierSurface>,
    ) -> Result<Vec<Spanned<Statement>>, CompileError> {
        self.vocab_block_stack.push(keyword_name.to_string());
        self.vocab_body_kind_stack.push(clause_body_kind);
        self.vocab_expression_item_modifier_stack
            .push(expression_item_modifiers);
        let body = match clause_body_kind {
            Some(incan_vocab::ClauseBodyKind::ExpressionList) => self.inline_vocab_expression_list_body(),
            Some(incan_vocab::ClauseBodyKind::Expression) => self.inline_vocab_expression_body(),
            _ => Ok(Vec::new()),
        };
        self.vocab_block_stack.pop();
        self.vocab_body_kind_stack.pop();
        self.vocab_expression_item_modifier_stack.pop();
        body
    }

    /// Parse one inline expression clause body until the physical statement boundary.
    fn inline_vocab_expression_body(&mut self) -> Result<Vec<Spanned<Statement>>, CompileError> {
        if self.current_ends_inline_vocab_clause() {
            return Ok(Vec::new());
        }
        let start = self.current_span().start;
        let expr = self.expression()?;
        let end = expr.span.end;
        Ok(vec![Spanned::new(Statement::Expr(expr), Span::new(start, end))])
    }

    /// Parse comma-separated inline expression-list items until the physical statement boundary.
    fn inline_vocab_expression_list_body(&mut self) -> Result<Vec<Spanned<Statement>>, CompileError> {
        let mut body = Vec::new();
        while !self.current_ends_inline_vocab_clause() {
            body.push(self.braced_vocab_expression_list_item()?);
            if !self.match_punct(PunctuationId::Comma) {
                break;
            }
        }
        Ok(body)
    }

    /// Return whether the current token ends an inline clause payload.
    fn current_ends_inline_vocab_clause(&self) -> bool {
        matches!(
            self.peek().kind,
            TokenKind::Newline | TokenKind::Dedent | TokenKind::Eof
        )
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

    /// Find the active vocab block metadata for the current keyword and parent block context.
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
            ) && self.vocab_compound_tokens_match_at(&spec.compound_tokens, self.pos + 1)
                && match (&spec.placement, parent_keyword) {
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

    /// Return whether the current token starts a registered vocab block in the requested parent context.
    fn current_starts_vocab_block_for_parent(&self, parent_keyword: Option<&str>) -> bool {
        let Some(keyword_name) = self.current_vocab_keyword_name() else {
            return false;
        };
        self.find_active_vocab_block_spec(&keyword_name, parent_keyword)
            .is_some()
    }

    /// Return the current identifier or keyword spelling if it can name a vocab keyword.
    fn current_vocab_keyword_name(&self) -> Option<String> {
        match &self.peek().kind {
            TokenKind::Ident(name) => Some(name.clone()),
            TokenKind::Keyword(id) => Some(incan_lang::lang::keywords::as_str(*id).to_string()),
            _ => None,
        }
    }

    /// Return true when the token stream contains the expected compound keyword tail at `start_idx`.
    fn vocab_compound_tokens_match_at(&self, compound_tokens: &[String], start_idx: usize) -> bool {
        compound_tokens.iter().enumerate().all(|(offset, expected)| {
            self.tokens
                .get(start_idx + offset)
                .and_then(|token| Self::vocab_word_token_from_kind(&token.kind))
                .is_some_and(|actual| actual == expected)
        })
    }

    /// Consume a compound keyword tail already selected by metadata-driven lookahead.
    fn consume_vocab_compound_tokens(&mut self, compound_tokens: &[String]) -> Result<(), CompileError> {
        for expected in compound_tokens {
            let Some(actual) = Self::vocab_word_token_from_kind(&self.peek().kind) else {
                return Err(errors::expected_token_message(
                    &format!("Expected compound vocab keyword token `{expected}`"),
                    &format!("{:?}", self.peek().kind),
                    self.current_span(),
                ));
            };
            if actual != expected {
                return Err(errors::expected_token_message(
                    &format!("Expected compound vocab keyword token `{expected}`"),
                    actual,
                    self.current_span(),
                ));
            }
            self.advance();
        }
        Ok(())
    }

    /// Return a token spelling usable for metadata-driven vocab keyword matching.
    fn vocab_word_token_from_kind(kind: &TokenKind) -> Option<&str> {
        match kind {
            TokenKind::Ident(name) => Some(name.as_str()),
            TokenKind::Keyword(id) => Some(incan_lang::lang::keywords::as_str(*id)),
            _ => None,
        }
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
}
