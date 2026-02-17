/// Function and method parsing (including parameters and receivers).
impl<'a> Parser<'a> {
    /// Parse a function declaration.
    fn function_decl(
        &mut self,
        decorators: Vec<Spanned<Decorator>>,
        visibility: Visibility,
    ) -> Result<FunctionDecl, CompileError> {
        let mut surface_modifiers = Vec::new();
        while let Some(id) = self.match_surface_keyword(KeywordSurfaceKind::DeclarationModifier) {
            surface_modifiers.push(SurfaceModifier {
                key: SurfaceFeatureKey::SoftKeyword(id),
            });
        }
        self.expect_keyword(KeywordId::Def, "Expected 'def'")?;
        let name = self.identifier()?;

        // Parse optional generic type parameters: def func[T, E](...)
        let type_params = self.type_params()?;

        self.expect_punct(PunctuationId::LParen, "Expected '(' after function name")?;
        let params = self.params()?;
        self.expect_punct(PunctuationId::RParen, "Expected ')' after parameters")?;
        self.expect_punct(PunctuationId::Arrow, "Expected '->' before return type")?;
        let return_type = self.type_expr()?;
        self.expect_punct(PunctuationId::Colon, "Expected ':' after return type")?;
        self.expect(&TokenKind::Newline, "Expected newline after ':'")?;
        self.expect(&TokenKind::Indent, "Expected indented block")?;

        let body = self.block()?;

        self.expect(&TokenKind::Dedent, "Expected dedent after function body")?;

        Ok(FunctionDecl {
            visibility,
            decorators,
            surface_modifiers,
            name,
            type_params,
            params,
            return_type,
            body,
        })
    }

    /// Parse a method declaration.
    fn method_decl(&mut self, decorators: Vec<Spanned<Decorator>>) -> Result<Spanned<MethodDecl>, CompileError> {
        let start = self.current_span().start;
        let mut surface_modifiers = Vec::new();
        while let Some(id) = self.match_surface_keyword(KeywordSurfaceKind::DeclarationModifier) {
            surface_modifiers.push(SurfaceModifier {
                key: SurfaceFeatureKey::SoftKeyword(id),
            });
        }
        self.expect_keyword(KeywordId::Def, "Expected 'def'")?;
        let name = self.identifier()?;
        self.expect_punct(PunctuationId::LParen, "Expected '(' after method name")?;

        // Parse receiver and params
        let (receiver, params) = self.receiver_and_params()?;

        self.expect_punct(PunctuationId::RParen, "Expected ')' after parameters")?;
        self.expect_punct(PunctuationId::Arrow, "Expected '->' before return type")?;
        let return_type = self.type_expr()?;

        // Check for abstract method (no body), ellipsis, or block
        let body = if self.check(&TokenKind::Newline) {
            // Abstract method with just newline (trait definition)
            None
        } else if self.match_punct(PunctuationId::Colon) {
            if self.match_punct(PunctuationId::Ellipsis) {
                None
            } else {
                self.expect(&TokenKind::Newline, "Expected newline after ':'")?;
                self.expect(&TokenKind::Indent, "Expected indented block")?;
                let b = self.block()?;
                self.expect(&TokenKind::Dedent, "Expected dedent after method body")?;
                Some(b)
            }
        } else {
            return Err(errors::method_decl_expected_body(self.current_span()));
        };

        let end = self.tokens[self.pos - 1].span.end;

        Ok(Spanned::new(
            MethodDecl {
                decorators,
                surface_modifiers,
                name,
                receiver,
                params,
                return_type,
                body,
            },
            Span::new(start, end),
        ))
    }

    /// Parse a receiver and parameters.
    fn receiver_and_params(&mut self) -> Result<(Option<Receiver>, Vec<Spanned<Param>>), CompileError> {
        // Check for receiver
        let receiver = if self.check_keyword(KeywordId::Mut) {
            self.advance();
            self.expect(&TokenKind::Keyword(KeywordId::SelfKw), "Expected 'self' after 'mut'")?;
            if self.check(&TokenKind::Punctuation(PunctuationId::Comma)) {
                self.advance();
            }
            Some(Receiver::Mutable)
        } else if self.check_keyword(KeywordId::SelfKw) {
            self.advance();
            if self.check(&TokenKind::Punctuation(PunctuationId::Comma)) {
                self.advance();
            }
            Some(Receiver::Immutable)
        } else {
            None
        };

        let params = if !self.check(&TokenKind::Punctuation(PunctuationId::RParen)) {
            self.params()?
        } else {
            Vec::new()
        };

        Ok((receiver, params))
    }

    /// Parse parameters.
    fn params(&mut self) -> Result<Vec<Spanned<Param>>, CompileError> {
        let mut params = Vec::new();
        if !self.check(&TokenKind::Punctuation(PunctuationId::RParen)) {
            loop {
                params.push(self.param()?);
                if !self.match_token(&TokenKind::Punctuation(PunctuationId::Comma)) {
                    break;
                }
            }
        }
        Ok(params)
    }

    /// Parse a parameter.
    fn param(&mut self) -> Result<Spanned<Param>, CompileError> {
        let start = self.current_span().start;
        // Check for optional 'mut' keyword
        let is_mut = self.match_token(&TokenKind::Keyword(KeywordId::Mut));
        let name = self.identifier()?;
        self.expect(
            &TokenKind::Punctuation(PunctuationId::Colon),
            "Expected ':' after parameter name",
        )?;
        let ty = self.type_expr()?;
        let default = if self.match_token(&TokenKind::Operator(OperatorId::Eq)) {
            Some(self.expression()?)
        } else {
            None
        };
        let end = self.tokens[self.pos - 1].span.end;
        Ok(Spanned::new(
            Param {
                is_mut,
                name,
                ty,
                default,
            },
            Span::new(start, end),
        ))
    }
}
