/// List, dict, comprehension, tuple and parenthesized expressions, and call and partial arguments.
impl<'a> Parser<'a> {
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
                Expr::Generator(Box::new(GeneratorExpr { expr: first, clauses })),
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

    /// Convert expressions to closure parameters Only identifiers are valid as closure params
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
            self.scoped_call_argument_stack.push(ScopedCallArgumentContext { call });
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
                if matches!(self.peek().kind, TokenKind::Ident(_) | TokenKind::Keyword(_))
                    && self.peek_next().kind == TokenKind::Operator(OperatorId::Eq)
                {
                    let name_span = self.current_span();
                    let name = self.identifier_or_any_keyword()?;
                    self.expect(
                        &TokenKind::Operator(OperatorId::Eq),
                        "Expected '=' after named argument",
                    )?;
                    self.skip_newlines();
                    let value = self.expression()?;
                    self.skip_newlines();
                    args.push(CallArg::Named(Spanned::new(name, name_span), value));
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
                CallArg::Named(name, value) => partial_args.push(PartialArg { name: name.node, value }),
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
