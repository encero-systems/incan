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
        } else if self.is_module_tests_header() {
            if visibility == Visibility::Public {
                return Err(CompileError::syntax(
                    "`module tests:` cannot be public".to_string(),
                    self.current_span(),
                ));
            }
            if !decorators.is_empty() {
                return Err(CompileError::syntax(
                    "`module tests:` cannot have decorators".to_string(),
                    decorators[0].span,
                ));
            }
            Declaration::TestModule(self.test_module_decl()?)
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

    /// Return `true` when the current declaration starts the reserved RFC 018 inline test module.
    fn is_module_tests_header(&self) -> bool {
        matches!(&self.peek().kind, TokenKind::Ident(name) if name == "module")
            && matches!(&self.peek_next().kind, TokenKind::Ident(name) if name == "tests")
            && matches!(
                self.tokens.get(self.pos + 2).map(|token| &token.kind),
                Some(TokenKind::Punctuation(PunctuationId::Colon))
            )
    }

    /// Parse `module tests:` as a parser-owned inline test-only scope.
    ///
    /// The parser keeps declarations inside the block structurally scoped and restores soft-keyword state afterward so
    /// imports that appear only in the inline test module do not affect production declarations that follow it.
    fn test_module_decl(&mut self) -> Result<TestModuleDecl, CompileError> {
        self.expect(&TokenKind::Ident(String::new()), "Expected 'module'")?;
        let name = self.identifier()?;
        if name != "tests" {
            return Err(CompileError::syntax(
                "Only `module tests:` is supported".to_string(),
                self.current_span(),
            ));
        }
        self.expect_punct(PunctuationId::Colon, "Expected ':' after `module tests`")?;
        self.expect(&TokenKind::Newline, "Expected newline after `module tests:`")?;
        self.expect_suite_indent("Expected indented block after `module tests:`")?;

        let outer_soft_keywords = self.active_soft_keywords.clone();
        let outer_imported_specs = self.active_imported_keyword_specs.clone();
        let body_result = self.test_module_body();
        self.active_soft_keywords = outer_soft_keywords;
        self.active_imported_keyword_specs = outer_imported_specs;

        let body = body_result?;
        self.expect(&TokenKind::Dedent, "Expected dedent after `module tests:` body")?;
        Ok(TestModuleDecl { name, body })
    }

    /// Parse declarations within a `module tests:` block.
    ///
    /// `pass` and `...` are accepted as empty placeholders so authors can stub an inline test module before adding test
    /// declarations.
    fn test_module_body(&mut self) -> Result<Vec<Spanned<Declaration>>, CompileError> {
        let mut body = Vec::new();
        self.skip_newlines();
        while !self.check(&TokenKind::Dedent) && !self.is_at_end() {
            if self.match_keyword(KeywordId::Pass) || self.match_punct(PunctuationId::Ellipsis) {
                self.skip_newlines();
                continue;
            }

            let decl = self.declaration()?;
            self.activate_soft_keywords_for_declaration(&decl.node);
            body.push(decl);
            self.skip_newlines();
        }
        Ok(body)
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
