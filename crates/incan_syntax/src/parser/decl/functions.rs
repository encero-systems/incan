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
        let name = self.identifier_or_from_keyword()?;
        let type_params = self.type_params()?;
        self.expect_punct(PunctuationId::LParen, "Expected '(' after method name")?;

        // Parse receiver and params
        let is_classmethod = decorators.iter().any(|decorator| {
            incan_core::lang::decorators::from_segments(&decorator.node.path.segments)
                == Some(incan_core::lang::decorators::DecoratorId::ClassMethod)
        });
        let (receiver, params) = self.receiver_and_params(is_classmethod)?;

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
                type_params,
                receiver,
                params,
                return_type,
                body,
            },
            Span::new(start, end),
        ))
    }

    /// Parse a receiver and parameters.
    fn receiver_and_params(
        &mut self,
        is_classmethod: bool,
    ) -> Result<(Option<Receiver>, Vec<Spanned<Param>>), CompileError> {
        self.skip_newlines();

        // Check for receiver
        let receiver = if self.check_keyword(KeywordId::Mut) {
            self.advance();
            self.expect(&TokenKind::Keyword(KeywordId::SelfKw), "Expected 'self' after 'mut'")?;
            self.skip_newlines();
            if self.check(&TokenKind::Punctuation(PunctuationId::Comma)) {
                self.advance();
                self.skip_newlines();
            }
            Some(Receiver::Mutable)
        } else if self.check_keyword(KeywordId::SelfKw) {
            self.advance();
            self.skip_newlines();
            if self.check(&TokenKind::Punctuation(PunctuationId::Comma)) {
                self.advance();
                self.skip_newlines();
            }
            Some(Receiver::Immutable)
        } else if is_classmethod && self.peek_ident_text("cls") {
            self.advance();
            self.skip_newlines();
            if self.check(&TokenKind::Punctuation(PunctuationId::Comma)) {
                self.advance();
                self.skip_newlines();
            }
            None
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
        // Implicit line continuation: skip newlines after (
        self.skip_newlines();

        let mut params = Vec::new();
        if !self.check(&TokenKind::Punctuation(PunctuationId::RParen)) {
            loop {
                // Allow trailing comma before )
                self.skip_newlines();
                if self.check(&TokenKind::Punctuation(PunctuationId::RParen)) {
                    break;
                }

                params.push(self.param()?);
                self.skip_newlines();
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
