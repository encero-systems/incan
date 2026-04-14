/// Declaration entrypoints and visibility handling.
impl<'a> Parser<'a> {
    // ========================================================================
    // Declarations
    // ========================================================================

    fn declaration(&mut self) -> Result<Spanned<Declaration>, CompileError> {
        let start = self.current_span().start;

        // Handle module-level docstrings (string literals at top level)
        if let TokenKind::String(s) = &self.peek().kind {
            let doc = s.clone();
            self.advance();
            // Skip optional newline after docstring
            self.match_token(&TokenKind::Newline);
            let end = self.tokens[self.pos.saturating_sub(1)].span.end;
            return Ok(Spanned::new(Declaration::Docstring(doc), Span::new(start, end)));
        }

        // Collect decorators
        let decorators = self.decorators()?;

        let mut visibility = Visibility::Private;
        if self.check_keyword(KeywordId::Pub) {
            self.expect_keyword(KeywordId::Pub, "Expected 'pub'")?;
            visibility = Visibility::Public;
        }

        let decl = if self.check_keyword(KeywordId::From) {
            if visibility == Visibility::Public && self.module_path.is_some() && !self.is_src_module() {
                return Err(errors::pub_reexport_only_allowed_in_src_modules(
                    self.current_span(),
                ));
            }
            Declaration::Import(self.import_decl(visibility)?)
        } else if self.check_keyword(KeywordId::Import) {
            if visibility == Visibility::Public {
                return Err(errors::pub_modifier_not_allowed_on_import(self.current_span()));
            }
            Declaration::Import(self.import_decl(Visibility::Private)?)
        } else if self.check_keyword(KeywordId::Const) {
            Declaration::Const(self.const_decl_with_visibility(visibility)?)
        } else if self.check_keyword(KeywordId::Static) {
            Declaration::Static(self.static_decl_with_visibility(visibility)?)
        } else if self.check_keyword(KeywordId::Model) {
            Declaration::Model(self.model_decl(decorators, visibility)?)
        } else if self.check_keyword(KeywordId::Class) {
            Declaration::Class(self.class_decl(decorators, visibility)?)
        } else if self.check_keyword(KeywordId::Trait) {
            Declaration::Trait(self.trait_decl(decorators, visibility)?)
        } else if self.check_keyword(KeywordId::Type) || self.check_keyword(KeywordId::Newtype) {
            match self.type_or_newtype_decl(decorators, visibility)? {
                TypeOrNewtype::Alias(a) => Declaration::TypeAlias(a),
                TypeOrNewtype::Newtype(n) => Declaration::Newtype(n),
            }
        } else if self.check_keyword(KeywordId::Enum) {
            Declaration::Enum(self.enum_decl(decorators, visibility)?)
        } else if self.starts_surface_function_decl() {
            Declaration::Function(self.function_decl(decorators, visibility)?)
        } else {
            if let Some(err) = self.inactive_soft_keyword_error() {
                return Err(err);
            }
            return Err(errors::expected_declaration(
                &format!("{:?}", self.peek().kind),
                self.current_span(),
            ));
        };

        let end = self.tokens[self.pos.saturating_sub(1)].span.end;
        Ok(Spanned::new(decl, Span::new(start, end)))
    }

    fn const_decl_with_visibility(&mut self, visibility: Visibility) -> Result<ConstDecl, CompileError> {
        self.expect_keyword(KeywordId::Const, "Expected 'const'")?;
        let name = self.identifier()?;
        let ty = if self.match_punct(PunctuationId::Colon) {
            Some(self.type_expr()?)
        } else {
            None
        };
        self.expect_op(OperatorId::Eq, "Expected '=' after const name")?;
        let value = self.expression()?;
        Ok(ConstDecl {
            visibility,
            name,
            ty,
            value,
        })
    }

    fn static_decl_with_visibility(&mut self, visibility: Visibility) -> Result<StaticDecl, CompileError> {
        self.expect_keyword(KeywordId::Static, "Expected 'static'")?;
        let name = self.identifier()?;
        if !self.match_punct(PunctuationId::Colon) {
            return Err(errors::static_missing_type_annotation(&name, self.current_span()));
        }
        let ty = self.type_expr()?;
        if !self.match_op(OperatorId::Eq) {
            return Err(errors::static_missing_initializer(&name, self.current_span()));
        }
        let value = self.expression()?;
        Ok(StaticDecl {
            visibility,
            name,
            ty,
            value,
        })
    }
}
