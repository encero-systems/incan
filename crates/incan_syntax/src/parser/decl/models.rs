/// Parsing for models, classes, traits, and their fields/methods.
impl<'a> Parser<'a> {
    /// Parse a model declaration.
    fn model_decl(
        &mut self,
        decorators: Vec<Spanned<Decorator>>,
        visibility: Visibility,
    ) -> Result<ModelDecl, CompileError> {
        self.expect_keyword(KeywordId::Model, "Expected 'model'")?;
        let name = self.identifier()?;
        let type_params = self.type_params()?;
        let traits = if self.match_keyword(KeywordId::With) {
            self.identifier_list_spanned()?
        } else {
            Vec::new()
        };
        self.expect_punct(PunctuationId::Colon, "Expected ':' after model name")?;
        self.expect(&TokenKind::Newline, "Expected newline after ':'")?;
        self.expect(&TokenKind::Indent, "Expected indented block")?;

        let (mut fields, methods) = self.fields_and_methods()?;

        self.expect(&TokenKind::Dedent, "Expected dedent after model body")?;

        // If the model is public, promote all field visibilities to public.
        if matches!(visibility, Visibility::Public) {
            for f in &mut fields {
                f.node.visibility = Visibility::Public;
            }
        }

        Ok(ModelDecl {
            visibility,
            decorators,
            name,
            type_params,
            traits,
            fields,
            methods,
        })
    }

    /// Parse a class declaration.
    fn class_decl(
        &mut self,
        decorators: Vec<Spanned<Decorator>>,
        visibility: Visibility,
    ) -> Result<ClassDecl, CompileError> {
        self.expect_keyword(KeywordId::Class, "Expected 'class'")?;
        let name = self.identifier()?;
        let type_params = self.type_params()?;

        let extends = if self.match_keyword(KeywordId::Extends) {
            Some(self.identifier()?)
        } else {
            None
        };

        let traits = if self.match_keyword(KeywordId::With) {
            self.identifier_list_spanned()?
        } else {
            Vec::new()
        };

        self.expect_punct(PunctuationId::Colon, "Expected ':' after class header")?;
        self.expect(&TokenKind::Newline, "Expected newline after ':'")?;
        self.expect(&TokenKind::Indent, "Expected indented block")?;

        let (mut fields, methods) = self.fields_and_methods()?;

        self.expect(&TokenKind::Dedent, "Expected dedent after class body")?;

        // If the class is public, promote all field visibilities to public.
        if matches!(visibility, Visibility::Public) {
            for f in &mut fields {
                f.node.visibility = Visibility::Public;
            }
        }

        Ok(ClassDecl {
            visibility,
            decorators,
            name,
            type_params,
            extends,
            traits,
            fields,
            methods,
        })
    }

    /// Parse a trait declaration.
    fn trait_decl(
        &mut self,
        decorators: Vec<Spanned<Decorator>>,
        visibility: Visibility,
    ) -> Result<TraitDecl, CompileError> {
        self.expect_keyword(KeywordId::Trait, "Expected 'trait'")?;
        let name = self.identifier()?;
        let type_params = self.type_params()?;
        self.expect_punct(PunctuationId::Colon, "Expected ':' after trait name")?;
        self.expect(&TokenKind::Newline, "Expected newline after ':'")?;
        self.expect(&TokenKind::Indent, "Expected indented block")?;

        let mut methods = Vec::new();
        self.skip_newlines();

        // Skip optional docstring at the start of the trait body
        if let TokenKind::String(_) = &self.peek().kind {
            self.advance();
            self.skip_newlines();
        }

        // Allow empty trait body with just 'pass'
        if self.match_keyword(KeywordId::Pass) {
            self.skip_newlines();
        } else {
            while !self.check(&TokenKind::Dedent) && !self.is_at_end() {
                let method_decorators = self.decorators()?;
                if let Some(err) = self.inactive_soft_keyword_error() {
                    return Err(err);
                }
                methods.push(self.method_decl(method_decorators)?);
                self.skip_newlines();
            }
        }

        self.expect(&TokenKind::Dedent, "Expected dedent after trait body")?;

        Ok(TraitDecl {
            visibility,
            decorators,
            name,
            type_params,
            methods,
        })
    }

    /// Parse fields and methods.
    fn fields_and_methods(&mut self) -> Result<FieldsAndMethods, CompileError> {
        let mut fields = Vec::new();
        let mut methods = Vec::new();

        self.skip_newlines();
        while !self.check(&TokenKind::Dedent) && !self.is_at_end() {
            if let TokenKind::String(_) = &self.peek().kind {
                self.advance();
                // Optional newline after docstring
                self.match_token(&TokenKind::Newline);
                self.skip_newlines();
                continue;
            }
            let decorators = self.decorators()?;
            if let Some(err) = self.inactive_soft_keyword_error() {
                return Err(err);
            }

            // Check if it's a method (starts with def or async def)
            if self.check_keyword(KeywordId::Def) || self.check_keyword(KeywordId::Async) {
                methods.push(self.method_decl(decorators)?);
            } else {
                // It's a field
                if !decorators.is_empty() {
                    return Err(errors::decorators_on_fields_not_supported(decorators[0].span));
                }
                fields.push(self.field_decl()?);
            }
            self.skip_newlines();
        }

        Ok((fields, methods))
    }

    fn field_metadata(&mut self) -> Result<FieldMetadata, CompileError> {
        let mut metadata = FieldMetadata::default();

        loop {
            let key = self.identifier_spanned()?;
            let key_span = key.span;
            let key_raw = key.node;
            self.expect(
                &TokenKind::Operator(OperatorId::Eq),
                "Expected '=' after field metadata key",
            )?;
            let value = self.string_literal()?;

            let Some(key) = field_metadata::from_str(&key_raw) else {
                return Err(errors::unknown_field_metadata_key(&key_raw, key_span));
            };

            match key {
                FieldMetadataKey::Alias => {
                    if metadata.alias.is_some() {
                        return Err(errors::duplicate_field_metadata_key(
                            field_metadata::as_str(key),
                            key_span,
                        ));
                    }
                    metadata.alias = Some(value);
                }
                FieldMetadataKey::Description => {
                    if metadata.description.is_some() {
                        return Err(errors::duplicate_field_metadata_key(
                            field_metadata::as_str(key),
                            key_span,
                        ));
                    }
                    metadata.description = Some(value);
                }
            }

            if !self.match_token(&TokenKind::Punctuation(PunctuationId::Comma)) {
                break;
            }
        }

        Ok(metadata)
    }

    /// Parse a field declaration.
    fn field_decl(&mut self) -> Result<Spanned<FieldDecl>, CompileError> {
        let start = self.current_span().start;
        let visibility = if self.match_token(&TokenKind::Keyword(KeywordId::Pub)) {
            Visibility::Public
        } else {
            Visibility::Private
        };
        let name = self.identifier()?;
        let mut metadata = FieldMetadata::default();
        if self.match_token(&TokenKind::Punctuation(PunctuationId::LBracket)) {
            metadata = self.field_metadata()?;
            self.expect(
                &TokenKind::Punctuation(PunctuationId::RBracket),
                "Expected ']' after field metadata",
            )?;
        }
        if self.match_keyword(KeywordId::As) {
            if metadata.alias.is_some() {
                return Err(errors::field_alias_as_conflict(self.tokens[self.pos - 1].span));
            }
            let alias = self.string_literal()?;
            metadata.alias = Some(alias);
        }
        self.expect(
            &TokenKind::Punctuation(PunctuationId::Colon),
            "Expected ':' after field name",
        )?;
        let ty = self.type_expr()?;
        let default = if self.match_token(&TokenKind::Operator(OperatorId::Eq)) {
            Some(self.expression()?)
        } else {
            None
        };
        let end = self.tokens[self.pos - 1].span.end;
        Ok(Spanned::new(
            FieldDecl {
                visibility,
                name,
                metadata,
                ty,
                default,
            },
            Span::new(start, end),
        ))
    }
}
