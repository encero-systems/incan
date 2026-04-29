/// Algebraic data types (enums and variants).
impl<'a> Parser<'a> {
    /// Parse an enum declaration.
    fn enum_decl(
        &mut self,
        decorators: Vec<Spanned<Decorator>>,
        visibility: Visibility,
    ) -> Result<EnumDecl, CompileError> {
        self.expect_keyword(KeywordId::Enum, "Expected 'enum'")?;
        let name = self.identifier()?;
        let type_params = self.type_params()?;
        let value_type = self.value_enum_type_specifier()?;
        let value_enum_type = value_type.as_ref().map(|ty| ty.node);
        let traits = if self.match_keyword(KeywordId::With) {
            self.trait_supertrait_list_spanned()?
        } else {
            Vec::new()
        };
        self.expect_punct(PunctuationId::Colon, "Expected ':' after enum name")?;
        self.expect(&TokenKind::Newline, "Expected newline after ':'")?;
        self.expect_suite_indent("Expected indented block")?;

        let docstring = self.optional_leading_block_docstring();

        let mut variants = Vec::new();
        let mut methods = Vec::new();
        let mut parsing_methods = false;
        while !self.check(&TokenKind::Dedent) && !self.is_at_end() {
            let method_decorators = self.decorators()?;
            if let Some(err) = self.inactive_soft_keyword_error() {
                return Err(err);
            }

            if self.starts_surface_function_decl() {
                parsing_methods = true;
                methods.push(self.method_decl(method_decorators)?);
            } else {
                if !method_decorators.is_empty() {
                    return Err(CompileError::syntax(
                        "Decorators in enum bodies must target methods".to_string(),
                        method_decorators[0].span,
                    ));
                }
                if parsing_methods {
                    return Err(CompileError::syntax(
                        "Enum variants must be declared before enum methods".to_string(),
                        self.current_span(),
                    ));
                }
                variants.push(self.variant_decl(value_enum_type)?);
            }
            self.skip_newlines();
        }

        self.expect(&TokenKind::Dedent, "Expected dedent after enum body")?;

        Ok(EnumDecl {
            visibility,
            decorators,
            name,
            type_params,
            value_type,
            traits,
            docstring,
            variants,
            methods,
        })
    }

    /// Parse an RFC 032 value enum type specifier.
    fn value_enum_type_specifier(&mut self) -> Result<Option<Spanned<ValueEnumType>>, CompileError> {
        if !self.match_punct(PunctuationId::LParen) {
            return Ok(None);
        }

        let ty = self.type_expr()?;
        self.expect_punct(
            PunctuationId::RParen,
            "Expected ')' after value enum type specifier",
        )?;

        let value_type = match &ty.node {
            Type::Simple(name) if name == "str" => ValueEnumType::Str,
            Type::Simple(name) if name == "int" => ValueEnumType::Int,
            _ => {
                return Err(CompileError::syntax(
                    "Value enum type specifier must be 'str' or 'int'".to_string(),
                    ty.span,
                ));
            }
        };

        Ok(Some(Spanned::new(value_type, ty.span)))
    }

    /// Parse a variant declaration.
    fn variant_decl(&mut self, value_enum_type: Option<ValueEnumType>) -> Result<Spanned<VariantDecl>, CompileError> {
        let start = self.current_span().start;
        // Allow keywords like "None" as variant names (Rust allows this)
        let name = self.identifier_or_keyword()?;

        // Detect common mistakes in enum bodies and emit targeted diagnostics.
        if self.check_punct(PunctuationId::FatArrow) {
            return Err(errors::enum_variant_mapped_values(self.current_span()));
        }
        if self.check_punct(PunctuationId::Dot) {
            return Err(errors::enum_variant_contains_dots(self.current_span()));
        }
        if value_enum_type.is_none() && self.check_op(OperatorId::Eq) {
            return Err(errors::enum_variant_assigned_values(self.current_span()));
        }
        if self.check_punct(PunctuationId::Colon) {
            return Err(errors::enum_variant_type_annotations(self.current_span()));
        }

        let fields = if self.match_punct(PunctuationId::LParen) {
            let fields = self.type_list()?;
            self.expect_punct(PunctuationId::RParen, "Expected ')' after variant fields")?;
            if value_enum_type.is_some() {
                return Err(CompileError::syntax(
                    "Value enum variants cannot carry tuple or struct payloads".to_string(),
                    Span::new(start, self.tokens[self.pos - 1].span.end),
                ));
            }
            fields
        } else {
            Vec::new()
        };

        let value = if self.check_op(OperatorId::Eq) {
            let Some(value_enum_type) = value_enum_type else {
                return Err(errors::enum_variant_assigned_values(self.current_span()));
            };
            self.expect_op(OperatorId::Eq, "Expected '=' before value enum literal")?;
            Some(self.value_enum_literal(value_enum_type)?)
        } else {
            if value_enum_type.is_some() {
                return Err(CompileError::syntax(
                    "Value enum variants must have explicit literal values".to_string(),
                    self.current_span(),
                ));
            }
            None
        };

        let end = self.tokens[self.pos - 1].span.end;
        Ok(Spanned::new(VariantDecl { name, fields, value }, Span::new(start, end)))
    }

    /// Parse a raw literal for a value enum variant, constrained by the enum backing type.
    fn value_enum_literal(&mut self, value_type: ValueEnumType) -> Result<Spanned<ValueEnumLiteral>, CompileError> {
        let span = self.current_span();
        match (value_type, &self.peek().kind) {
            (ValueEnumType::Str, TokenKind::String(value)) => {
                let value = value.clone();
                self.advance();
                Ok(Spanned::new(ValueEnumLiteral::Str(value), span))
            }
            (ValueEnumType::Int, TokenKind::Int(value)) => {
                let value = value.clone();
                self.advance();
                Ok(Spanned::new(ValueEnumLiteral::Int(value), span))
            }
            (ValueEnumType::Str, _) => Err(CompileError::syntax(
                "Expected string literal value for value enum variant".to_string(),
                span,
            )),
            (ValueEnumType::Int, _) => Err(CompileError::syntax(
                "Expected integer literal value for value enum variant".to_string(),
                span,
            )),
        }
    }
}
