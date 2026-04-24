/// Type and newtype declarations.
impl<'a> Parser<'a> {
    /// Parse either a type alias or a newtype declaration.
    ///
    /// Dispatch logic (after consuming `type` / `newtype` keyword, name, type params, and `=`):
    ///
    /// - `type X[T] = newtype Y[T]` → newtype (struct wrapper)
    /// - `newtype X = Y`            → newtype (alternate form)
    /// - `type X[T] = Y[T]`         → type alias (`pub type X<T> = Y<T>`)
    pub(super) fn type_or_newtype_decl(
        &mut self,
        decorators: Vec<Spanned<Decorator>>,
        visibility: Visibility,
    ) -> Result<TypeOrNewtype, CompileError> {
        // Support both: "type X = ..." and "newtype X = T"
        let is_newtype_keyword = self.match_keyword(KeywordId::Newtype);
        if !is_newtype_keyword {
            self.expect_keyword(KeywordId::Type, "Expected 'type' or 'newtype'")?;
        }
        let name = self.identifier()?;
        let type_params = self.type_params()?;
        self.expect_op(OperatorId::Eq, "Expected '=' after type name")?;

        // ---- `newtype X = Y` or `type X = newtype Y` → newtype ----
        // ---- `type X = rusttype Y`                   → rusttype (RFC 041 surface form) ----
        // ---- `type X = Y`                            → type alias ----
        if is_newtype_keyword || self.match_keyword(KeywordId::Newtype) {
            Ok(TypeOrNewtype::Newtype(
                self.finish_newtype(decorators, visibility, name, type_params, false)?,
            ))
        } else if self.match_ident_text("rusttype") {
            Ok(TypeOrNewtype::Newtype(
                self.finish_newtype(decorators, visibility, name, type_params, true)?,
            ))
        } else {
            // Type alias: `type X[T] = Y[T]`  — no body block allowed.
            let target = self.type_expr()?;
            Ok(TypeOrNewtype::Alias(TypeAliasDecl { visibility, name, type_params, target }))
        }
    }

    /// Finish parsing a newtype body after the `=` (and optional `newtype` keyword) has been consumed.
    fn finish_newtype(
        &mut self,
        decorators: Vec<Spanned<Decorator>>,
        visibility: Visibility,
        name: Ident,
        type_params: Vec<TypeParam>,
        is_rusttype: bool,
    ) -> Result<NewtypeDecl, CompileError> {
        let underlying = self.type_expr()?;
        let mut docstring = None;
        let mut rebindings = Vec::new();
        let mut interop_edges = Vec::new();
        let mut seen_interop_block = false;

        let methods = if self.match_punct(PunctuationId::Colon) {
            self.expect(&TokenKind::Newline, "Expected newline after ':'")?;
            self.expect_suite_indent("Expected indented block")?;

            let mut methods = Vec::new();
            self.skip_newlines();

            // Capture optional docstring at the start of the newtype body.
            if let TokenKind::String(s) = &self.peek().kind {
                docstring = Some(s.clone());
                self.advance();
                self.match_token(&TokenKind::Newline);
                self.skip_newlines();
            }

            while !self.check(&TokenKind::Dedent) && !self.is_at_end() {
                if self.peek_ident_text("interop")
                    && self.peek_next().kind.is_punctuation(PunctuationId::Colon)
                {
                    if seen_interop_block {
                        return Err(errors::expected_token_message(
                            "Duplicate `interop:` block; only one is allowed",
                            &format!("{:?}", self.peek().kind),
                            self.current_span(),
                        ));
                    }
                    seen_interop_block = true;
                    self.advance(); // `interop`
                    self.expect_punct(PunctuationId::Colon, "Expected ':' after `interop`")?;
                    self.expect(&TokenKind::Newline, "Expected newline after `interop:`")?;
                    self.expect_suite_indent("Expected indented block for `interop:`")?;
                    self.skip_newlines();
                    while !self.check(&TokenKind::Dedent) && !self.is_at_end() {
                        interop_edges.push(self.interop_edge_decl()?);
                        self.skip_newlines();
                    }
                    self.expect(&TokenKind::Dedent, "Expected dedent after `interop:` block")?;
                    continue;
                }

                if self.check(&TokenKind::Ident(String::new()))
                    && self.peek_next().kind.is_operator(OperatorId::Eq)
                {
                    rebindings.push(self.rebinding_decl()?);
                    self.skip_newlines();
                    continue;
                }

                let method_decorators = self.decorators()?;
                methods.push(self.method_decl(method_decorators)?);
                self.skip_newlines();
            }

            self.expect(&TokenKind::Dedent, "Expected dedent after newtype body")?;
            methods
        } else {
            Vec::new()
        };

        Ok(NewtypeDecl {
            visibility,
            decorators,
            name,
            type_params,
            is_rusttype,
            underlying,
            docstring,
            rebindings,
            interop_edges,
            methods,
        })
    }

    fn rebinding_decl(&mut self) -> Result<Spanned<RebindingDecl>, CompileError> {
        let start = self.current_span().start;
        let name = self.identifier()?;
        self.expect_op(OperatorId::Eq, "Expected '=' in rebinding declaration")?;
        let target = self.expression()?;
        let end = self.tokens[self.pos.saturating_sub(1)].span.end;
        Ok(Spanned::new(
            RebindingDecl { name, target },
            Span::new(start, end),
        ))
    }

    fn interop_edge_decl(&mut self) -> Result<Spanned<InteropEdgeDecl>, CompileError> {
        let start = self.current_span().start;
        let direction = if self.match_keyword(KeywordId::From) {
            InteropDirection::From
        } else if self.match_ident_text("into") {
            InteropDirection::Into
        } else {
            return Err(errors::expected_token_message(
                "Expected `from` or `into` in interop edge",
                &format!("{:?}", self.peek().kind),
                self.current_span(),
            ));
        };

        let ty = self.type_expr()?;

        let adapter_kind = if self.match_ident_text("via") {
            InteropAdapterKind::Via
        } else if self.match_ident_text("try") {
            InteropAdapterKind::Try
        } else {
            return Err(errors::expected_token_message(
                "Expected `via` or `try` in interop edge",
                &format!("{:?}", self.peek().kind),
                self.current_span(),
            ));
        };

        let adapter = self.expression()?;
        let end = self.tokens[self.pos.saturating_sub(1)].span.end;
        Ok(Spanned::new(
            InteropEdgeDecl {
                direction,
                ty,
                adapter_kind,
                adapter,
            },
            Span::new(start, end),
        ))
    }
}

/// Result of parsing a `type` / `newtype` declaration.
pub(super) enum TypeOrNewtype {
    Alias(TypeAliasDecl),
    Newtype(NewtypeDecl),
}
