/// Parsing for RFC 104 `capability` declarations.
impl<'a> Parser<'a> {
    /// Parse a `capability` declaration.
    ///
    /// ```text
    /// capability refund:
    ///     description = "Issue a refund for a captured charge"
    ///     scope:
    ///         tenant: str
    ///     requires = [host.http.request]
    /// ```
    ///
    /// The body's three clauses are all optional at the grammar level and may appear in any order; which of them are
    /// *required* is a checked-semantics question rather than a syntax one, so an omitted `description` parses here
    /// and is reported by the typechecker with the declaration's own span.
    ///
    /// `requires` entries are parsed as ordinary expressions rather than strings, because RFC 104 requires them to be
    /// checked symbol references to other capabilities. Holding a capability never grants what it `requires`.
    fn capability_decl(
        &mut self,
        decorators: Vec<Spanned<Decorator>>,
        visibility: Visibility,
    ) -> Result<CapabilityDecl, CompileError> {
        if !self.is_capability_declaration_keyword() {
            return Err(errors::expected_token_message(
                "Expected 'capability'",
                &format!("{:?}", self.peek().kind),
                self.peek().span,
            ));
        }
        self.advance();
        let name = self.identifier()?;
        self.expect_punct(PunctuationId::Colon, "Expected ':' after capability name")?;
        self.expect(&TokenKind::Newline, "Expected newline after ':'")?;
        self.expect_suite_indent("Expected indented block")?;

        let docstring = self.optional_leading_block_docstring();

        let mut description = None;
        let mut scope = Vec::new();
        let mut requires = Vec::new();

        loop {
            self.skip_newlines();
            if self.check(&TokenKind::Dedent) || self.is_at_end() {
                break;
            }
            let clause_span = self.current_span();
            let clause = self.identifier()?;
            match clause.as_str() {
                "description" => {
                    self.expect(
                        &TokenKind::Operator(OperatorId::Eq),
                        "Expected '=' after `description`",
                    )?;
                    description = Some(self.expression()?);
                }
                "requires" => {
                    self.expect(&TokenKind::Operator(OperatorId::Eq), "Expected '=' after `requires`")?;
                    requires = self.capability_requires_list()?;
                }
                "scope" => {
                    self.expect_punct(PunctuationId::Colon, "Expected ':' after `scope`")?;
                    self.expect(&TokenKind::Newline, "Expected newline after `scope:`")?;
                    self.expect_suite_indent("Expected indented scope block")?;
                    scope = self.capability_scope_dims()?;
                    self.expect(&TokenKind::Dedent, "Expected dedent after scope block")?;
                }
                other => {
                    return Err(CompileError::syntax(
                        format!("Unknown capability clause `{other}`; expected `description`, `scope`, or `requires`"),
                        clause_span,
                    ));
                }
            }
            self.match_token(&TokenKind::Newline);
        }

        self.expect(&TokenKind::Dedent, "Expected dedent after capability body")?;

        Ok(CapabilityDecl {
            visibility,
            decorators,
            name,
            docstring,
            description,
            scope,
            requires,
        })
    }

    /// Return `true` when the current token opens a `capability` declaration.
    ///
    /// RFC 104's `capability` is contextual rather than reserved, so it lexes as an ordinary identifier and stays
    /// usable as a variable, field, or method name. Only the declaration shape — the spelling followed by a name —
    /// claims it as a keyword; `capability = x`, `capability(x)`, and `capability: T` all remain identifiers.
    fn is_capability_declaration_keyword(&self) -> bool {
        if !matches!(
            &self.peek().kind,
            TokenKind::Ident(name) if name == incan_core::lang::keywords::as_str(KeywordId::Capability)
        ) {
            return false;
        }

        matches!(self.peek_next().kind, TokenKind::Ident(_))
    }

    /// Parse the typed scope dimensions inside a capability's `scope:` block.
    fn capability_scope_dims(&mut self) -> Result<Vec<Spanned<CapabilityScopeDim>>, CompileError> {
        let mut dims = Vec::new();
        loop {
            self.skip_newlines();
            if self.check(&TokenKind::Dedent) || self.is_at_end() {
                break;
            }
            let start = self.current_span().start;
            let name = self.identifier()?;
            self.expect_punct(PunctuationId::Colon, "Expected ':' after scope dimension name")?;
            let ty = self.type_expr()?;
            let end = self.tokens[self.pos - 1].span.end;
            dims.push(Spanned::new(CapabilityScopeDim { name, ty }, Span::new(start, end)));
            self.match_token(&TokenKind::Newline);
        }
        Ok(dims)
    }

    /// Parse the bracketed capability references in a `requires = [...]` clause.
    fn capability_requires_list(&mut self) -> Result<Vec<Spanned<Expr>>, CompileError> {
        self.expect_punct(PunctuationId::LBracket, "Expected '[' to start `requires` list")?;
        let mut entries = Vec::new();
        loop {
            self.skip_newlines();
            if self.match_token(&TokenKind::Punctuation(PunctuationId::RBracket)) {
                break;
            }
            entries.push(self.expression()?);
            self.skip_newlines();
            if self.match_token(&TokenKind::Punctuation(PunctuationId::Comma)) {
                continue;
            }
            self.expect_punct(PunctuationId::RBracket, "Expected ',' or ']' in `requires` list")?;
            break;
        }
        Ok(entries)
    }
}
