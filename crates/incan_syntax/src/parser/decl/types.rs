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
        // ---- `type X = Y`                            → type alias ----
        if is_newtype_keyword || self.match_keyword(KeywordId::Newtype) {
            Ok(TypeOrNewtype::Newtype(self.finish_newtype(decorators, visibility, name, type_params)?))
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
    ) -> Result<NewtypeDecl, CompileError> {
        let underlying = self.type_expr()?;

        let methods = if self.match_punct(PunctuationId::Colon) {
            self.expect(&TokenKind::Newline, "Expected newline after ':'")?;
            self.expect(&TokenKind::Indent, "Expected indented block")?;

            let mut methods = Vec::new();
            self.skip_newlines();

            // Skip optional docstring at the start of the newtype body
            if let TokenKind::String(_) = &self.peek().kind {
                self.advance();
                self.match_token(&TokenKind::Newline);
                self.skip_newlines();
            }

            while !self.check(&TokenKind::Dedent) && !self.is_at_end() {
                let method_decorators = self.decorators()?;
                methods.push(self.method_decl(method_decorators)?);
                self.skip_newlines();
            }

            self.expect(&TokenKind::Dedent, "Expected dedent after newtype body")?;
            methods
        } else {
            Vec::new()
        };

        Ok(NewtypeDecl { visibility, decorators, name, type_params, underlying, methods })
    }
}

/// Result of parsing a `type` / `newtype` declaration.
pub(super) enum TypeOrNewtype {
    Alias(TypeAliasDecl),
    Newtype(NewtypeDecl),
}
