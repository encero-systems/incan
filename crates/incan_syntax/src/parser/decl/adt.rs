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
        self.expect_punct(PunctuationId::Colon, "Expected ':' after enum name")?;
        self.expect(&TokenKind::Newline, "Expected newline after ':'")?;
        self.expect_suite_indent("Expected indented block")?;

        let docstring = self.optional_leading_block_docstring();

        let mut variants = Vec::new();
        while !self.check(&TokenKind::Dedent) && !self.is_at_end() {
            variants.push(self.variant_decl()?);
            self.skip_newlines();
        }

        self.expect(&TokenKind::Dedent, "Expected dedent after enum body")?;

        Ok(EnumDecl {
            visibility,
            decorators,
            name,
            type_params,
            docstring,
            variants,
        })
    }

    /// Parse a variant declaration.
    fn variant_decl(&mut self) -> Result<Spanned<VariantDecl>, CompileError> {
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
        if self.check_op(OperatorId::Eq) {
            return Err(errors::enum_variant_assigned_values(self.current_span()));
        }
        if self.check_punct(PunctuationId::Colon) {
            return Err(errors::enum_variant_type_annotations(self.current_span()));
        }

        let fields = if self.match_punct(PunctuationId::LParen) {
            let fields = self.type_list()?;
            self.expect_punct(PunctuationId::RParen, "Expected ')' after variant fields")?;
            fields
        } else {
            Vec::new()
        };
        let end = self.tokens[self.pos - 1].span.end;
        Ok(Spanned::new(VariantDecl { name, fields }, Span::new(start, end)))
    }
}
