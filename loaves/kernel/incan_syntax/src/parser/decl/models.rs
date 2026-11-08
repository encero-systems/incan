/// Parsing for models, classes, traits, and their fields/methods.

enum ParsedFieldMetadataValue {
    String(Spanned<String>),
    StringList { values: Vec<Spanned<String>>, span: Span },
    Boolean(Spanned<bool>),
}

impl ParsedFieldMetadataValue {
    /// Return whether this parsed syntax satisfies the registry's accepted value shape.
    fn is_accepted_by(&self, expected: FieldMetadataValueKind) -> bool {
        matches!(
            (self, expected),
            (
                Self::String(_),
                FieldMetadataValueKind::String | FieldMetadataValueKind::StringOrStringList
            ) | (Self::StringList { .. }, FieldMetadataValueKind::StringOrStringList)
                | (Self::Boolean(_), FieldMetadataValueKind::Boolean)
        )
    }

    /// Return the source span covering this metadata value.
    fn span(&self) -> Span {
        match self {
            Self::String(value) => value.span,
            Self::StringList { span, .. } => *span,
            Self::Boolean(value) => value.span,
        }
    }
}

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
            self.trait_supertrait_list_spanned()?
        } else {
            Vec::new()
        };
        self.expect_punct(PunctuationId::Colon, "Expected ':' after model name")?;
        self.expect(&TokenKind::Newline, "Expected newline after ':'")?;
        self.expect_suite_indent("Expected indented block")?;

        let docstring = self.optional_leading_block_docstring();

        let (fields, method_aliases, method_partials, properties, methods) = self.fields_and_methods()?;

        self.expect(&TokenKind::Dedent, "Expected dedent after model body")?;

        Ok(ModelDecl {
            visibility,
            decorators,
            name,
            type_params,
            traits,
            docstring,
            fields,
            method_aliases,
            method_partials,
            properties,
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
            self.trait_supertrait_list_spanned()?
        } else {
            Vec::new()
        };

        self.expect_punct(PunctuationId::Colon, "Expected ':' after class header")?;
        self.expect(&TokenKind::Newline, "Expected newline after ':'")?;
        self.expect_suite_indent("Expected indented block")?;

        let docstring = self.optional_leading_block_docstring();

        let (fields, method_aliases, method_partials, properties, methods) = self.fields_and_methods()?;

        self.expect(&TokenKind::Dedent, "Expected dedent after class body")?;

        Ok(ClassDecl {
            visibility,
            decorators,
            name,
            type_params,
            extends,
            traits,
            docstring,
            fields,
            method_aliases,
            method_partials,
            properties,
            methods,
            declarative_members: Vec::new(),
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
        let traits = if self.match_keyword(KeywordId::With) {
            self.trait_supertrait_list_spanned()?
        } else {
            Vec::new()
        };
        self.expect_punct(PunctuationId::Colon, "Expected ':' after trait header")?;
        self.expect(&TokenKind::Newline, "Expected newline after ':'")?;
        self.expect_suite_indent("Expected indented block")?;

        let docstring = self.optional_leading_block_docstring();

        let mut method_aliases = Vec::new();
        let mut method_partials = Vec::new();
        let mut properties = Vec::new();
        let mut methods = Vec::new();
        // Allow empty trait body with just 'pass'
        if self.match_keyword(KeywordId::Pass) {
            self.skip_newlines();
        } else {
            while !self.check(&TokenKind::Dedent) && !self.is_at_end() {
                let method_decorators = self.decorators()?;
                if let Some(err) = self.inactive_soft_keyword_error() {
                    return Err(err);
                }
                if self.starts_method_partial_decl() {
                    if !method_decorators.is_empty() {
                        return Err(CompileError::syntax(
                            "Method partial declarations cannot have decorators".to_string(),
                            method_decorators[0].span,
                        ));
                    }
                    method_partials.push(self.method_partial_decl()?);
                } else if self.starts_method_alias_decl() {
                    if !method_decorators.is_empty() {
                        return Err(CompileError::syntax(
                            "Method alias declarations cannot have decorators".to_string(),
                            method_decorators[0].span,
                        ));
                    }
                    method_aliases.push(self.method_alias_decl()?);
                } else if self.starts_property_decl() {
                    properties.push(self.property_decl(method_decorators, true)?);
                } else {
                    if self.check_keyword(KeywordId::Pub) && self.starts_method_decl() {
                        // A trait method takes the trait's visibility (#2570); the rest of the method still parses.
                        self.errors
                            .push(errors::pub_modifier_not_allowed_on_trait_method(self.current_span()));
                    }
                    methods.push(self.method_decl(method_decorators, true)?);
                }
                self.skip_newlines();
            }
        }

        self.expect(&TokenKind::Dedent, "Expected dedent after trait body")?;

        Ok(TraitDecl {
            visibility,
            decorators,
            name,
            type_params,
            traits,
            docstring,
            method_aliases,
            method_partials,
            properties,
            methods,
        })
    }

    /// Parse fields and methods.
    fn fields_and_methods(&mut self) -> Result<FieldsAndMethods, CompileError> {
        let mut fields = Vec::new();
        let mut method_aliases = Vec::new();
        let mut method_partials = Vec::new();
        let mut properties = Vec::new();
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

            // Check if it's a method (`def` or surface-modifier-prefixed `def`).
            if self.starts_method_decl() {
                methods.push(self.method_decl(decorators, false)?);
            } else if self.starts_surface_property_decl() {
                properties.push(self.property_decl(decorators, false)?);
            } else if self.starts_method_partial_decl() {
                if !decorators.is_empty() {
                    return Err(CompileError::syntax(
                        "Method partial declarations cannot have decorators".to_string(),
                        decorators[0].span,
                    ));
                }
                method_partials.push(self.method_partial_decl()?);
            } else if self.starts_method_alias_decl() {
                if !decorators.is_empty() {
                    return Err(CompileError::syntax(
                        "Method alias declarations cannot have decorators".to_string(),
                        decorators[0].span,
                    ));
                }
                method_aliases.push(self.method_alias_decl()?);
            } else {
                // It's a field
                if !decorators.is_empty() {
                    return Err(errors::decorators_on_fields_not_supported(decorators[0].span));
                }
                fields.push(self.field_decl()?);
            }
            self.skip_newlines();
        }

        Ok((fields, method_aliases, method_partials, properties, methods))
    }

    /// Return whether the current tokens start a contextual RFC 046 property declaration.
    fn starts_property_decl(&self) -> bool {
        self.peek_ident_text("property")
    }

    /// Return whether the current tokens start either a plain property or a modifier-prefixed property declaration.
    fn starts_surface_property_decl(&self) -> bool {
        let mut idx = self.pos;
        if matches!(
            self.tokens.get(idx).map(|token| &token.kind),
            Some(TokenKind::Keyword(KeywordId::Pub))
        ) {
            idx += 1;
        }

        let mut saw_modifier = false;
        while let Some(token) = self.tokens.get(idx) {
            let id = match &token.kind {
                TokenKind::Keyword(id) => Some(*id),
                TokenKind::Ident(name) => incan_lang::lang::keywords::from_str(name),
                _ => None,
            };
            let Some(id) = id else {
                break;
            };
            if !self.active_soft_keywords.contains(&id)
                || !self.keyword_supports_surface_usage(id, KeywordSurfaceKind::DeclarationModifier)
            {
                break;
            }
            saw_modifier = true;
            idx += 1;
        }

        (idx == self.pos || saw_modifier || self.check_keyword(KeywordId::Pub))
            && matches!(
                self.tokens.get(idx).map(|token| &token.kind),
                Some(TokenKind::Ident(name)) if name == "property"
            )
    }

    /// Return whether the current token pair starts a same-type method alias declaration.
    fn starts_method_alias_decl(&self) -> bool {
        matches!(self.peek().kind, TokenKind::Ident(_)) && self.peek_next().kind.is_operator(OperatorId::Eq)
    }

    /// Return whether the current tokens start a same-type method partial declaration.
    fn starts_method_partial_decl(&self) -> bool {
        matches!(self.peek().kind, TokenKind::Ident(_))
            && self.peek_next().kind.is_operator(OperatorId::Eq)
            && matches!(
                self.tokens.get(self.pos + 2).map(|token| &token.kind),
                Some(TokenKind::Ident(name)) if name == "partial"
            )
    }

    /// Parse a same-type method alias declaration inside a type body.
    fn method_alias_decl(&mut self) -> Result<Spanned<MethodAliasDecl>, CompileError> {
        let start = self.current_span().start;
        let name = self.identifier_or_from_keyword()?;
        self.expect_op(OperatorId::Eq, "Expected '=' in method alias declaration")?;
        let explicit_marker = self.match_ident_text("alias");
        let target = self.identifier_or_from_keyword()?;
        let end = self.tokens[self.pos.saturating_sub(1)].span.end;
        Ok(Spanned::new(
            MethodAliasDecl {
                name,
                target,
                explicit_marker,
            },
            Span::new(start, end),
        ))
    }

    /// Parse a same-type method partial declaration inside a method-bearing type body.
    fn method_partial_decl(&mut self) -> Result<Spanned<MethodPartialDecl>, CompileError> {
        let start = self.current_span().start;
        let name = self.identifier_or_from_keyword()?;
        self.expect_op(OperatorId::Eq, "Expected '=' in method partial declaration")?;
        if !self.match_ident_text("partial") {
            return Err(errors::expected_token_message(
                "Expected 'partial' in method partial declaration",
                &format!("{:?}", self.peek().kind),
                self.current_span(),
            ));
        }
        let target = self.identifier_or_from_keyword()?;
        self.expect_punct(PunctuationId::LParen, "Expected '(' after method partial target")?;
        let args = self.partial_args()?;
        self.expect_punct(PunctuationId::RParen, "Expected ')' after method partial arguments")?;
        let end = self.tokens[self.pos.saturating_sub(1)].span.end;
        Ok(Spanned::new(
            MethodPartialDecl { name, target, args },
            Span::new(start, end),
        ))
    }

    /// Parse one field-metadata value without deciding which AST slot owns it.
    fn field_metadata_value(
        &mut self,
        key: &str,
        expected: FieldMetadataValueKind,
        field: &str,
    ) -> Result<ParsedFieldMetadataValue, CompileError> {
        let span = self.current_span();
        match self.peek().kind.clone() {
            TokenKind::String(value) => {
                self.advance();
                Ok(ParsedFieldMetadataValue::String(Spanned::new(value, span)))
            }
            TokenKind::Keyword(KeywordId::True) => {
                self.advance();
                Ok(ParsedFieldMetadataValue::Boolean(Spanned::new(true, span)))
            }
            TokenKind::Keyword(KeywordId::False) => {
                self.advance();
                Ok(ParsedFieldMetadataValue::Boolean(Spanned::new(false, span)))
            }
            TokenKind::Punctuation(PunctuationId::LBracket) => {
                self.advance();
                let mut values = Vec::new();
                while !self.check(&TokenKind::Punctuation(PunctuationId::RBracket)) {
                    let value_span = self.current_span();
                    let TokenKind::String(value) = self.peek().kind.clone() else {
                        return Err(errors::invalid_field_metadata_value(
                            key,
                            field_metadata::value_kind_description(expected),
                            field,
                            value_span,
                        ));
                    };
                    self.advance();
                    values.push(Spanned::new(value, value_span));
                    if !self.match_token(&TokenKind::Punctuation(PunctuationId::Comma)) {
                        break;
                    }
                }
                self.expect_punct(PunctuationId::RBracket, "Expected ']' after field metadata list")?;
                let end = self.tokens[self.pos.saturating_sub(1)].span.end;
                Ok(ParsedFieldMetadataValue::StringList {
                    values,
                    span: Span::new(span.start, end),
                })
            }
            _ => Err(errors::invalid_field_metadata_value(
                key,
                field_metadata::value_kind_description(expected),
                field,
                span,
            )),
        }
    }

    /// Store a registry-validated field-metadata entry while rejecting duplicate keys.
    fn apply_field_metadata(
        metadata: &mut FieldMetadata,
        key: FieldMetadataKey,
        key_raw: &str,
        key_span: Span,
        expected: FieldMetadataValueKind,
        field: &str,
        value: ParsedFieldMetadataValue,
    ) -> Result<(), CompileError> {
        match key {
            FieldMetadataKey::Alias => {
                if metadata.alias.is_some() {
                    return Err(errors::duplicate_field_metadata_key(key_raw, key_span));
                }
                let ParsedFieldMetadataValue::String(value) = value else {
                    return Err(errors::invalid_field_metadata_value(
                        key_raw,
                        field_metadata::value_kind_description(expected),
                        field,
                        key_span,
                    ));
                };
                metadata.alias = Some(value.node);
            }
            FieldMetadataKey::Description => {
                if metadata.description.is_some() {
                    return Err(errors::duplicate_field_metadata_key(key_raw, key_span));
                }
                let ParsedFieldMetadataValue::String(value) = value else {
                    return Err(errors::invalid_field_metadata_value(
                        key_raw,
                        field_metadata::value_kind_description(expected),
                        field,
                        key_span,
                    ));
                };
                metadata.description = Some(value.node);
            }
            FieldMetadataKey::RenamedFrom => {
                if metadata.renamed_from_key_span.is_some() {
                    return Err(errors::duplicate_field_metadata_key(key_raw, key_span));
                }
                metadata.renamed_from_key_span = Some(key_span);
                match value {
                    ParsedFieldMetadataValue::String(value) => metadata.renamed_from.push(value),
                    ParsedFieldMetadataValue::StringList { values, .. } => {
                        metadata.renamed_from = values;
                        metadata.renamed_from_was_list = true;
                    }
                    ParsedFieldMetadataValue::Boolean(value) => {
                        return Err(errors::invalid_field_metadata_value(
                            key_raw,
                            field_metadata::value_kind_description(expected),
                            field,
                            value.span,
                        ));
                    }
                }
            }
            FieldMetadataKey::Canonical => {
                if metadata.canonical_key_span.is_some() {
                    return Err(errors::duplicate_field_metadata_key(key_raw, key_span));
                }
                let ParsedFieldMetadataValue::Boolean(value) = value else {
                    return Err(errors::invalid_field_metadata_value(
                        key_raw,
                        field_metadata::value_kind_description(expected),
                        field,
                        key_span,
                    ));
                };
                metadata.canonical_key_span = Some(key_span);
                metadata.canonical = Some(value);
            }
        }

        Ok(())
    }

    /// Parse a comma-separated metadata block using the shared key registry.
    fn field_metadata(&mut self, field: &str) -> Result<FieldMetadata, CompileError> {
        let mut metadata = FieldMetadata::default();

        loop {
            let key = self.identifier_spanned()?;
            let key_span = key.span;
            let key_raw = key.node;
            self.expect(
                &TokenKind::Operator(OperatorId::Eq),
                "Expected '=' after field metadata key",
            )?;
            let Some(key) = field_metadata::from_str(&key_raw) else {
                return Err(errors::unknown_field_metadata_key(&key_raw, key_span));
            };
            let expected = field_metadata::value_kind(key);
            let value = self.field_metadata_value(&key_raw, expected, field)?;
            if !value.is_accepted_by(expected) {
                return Err(errors::invalid_field_metadata_value(
                    &key_raw,
                    field_metadata::value_kind_description(expected),
                    field,
                    value.span(),
                ));
            }

            Self::apply_field_metadata(
                &mut metadata,
                key,
                field_metadata::as_str(key),
                key_span,
                expected,
                field,
                value,
            )?;

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
            metadata = self.field_metadata(&name)?;
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
