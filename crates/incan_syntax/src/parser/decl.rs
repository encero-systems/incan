/// Declaration parsing methods.
///
/// This chunk is responsible for parsing top-level declarations such as imports,
/// type/enum/newtype declarations, models/classes/traits, and functions/methods.
///
/// ## Notes
/// - Most entrypoints in this file return [`Spanned<T>`] to preserve source locations.
/// - Error recovery is handled by `Parser::synchronize()` (in `helpers.rs`).
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

        let decl = if self.check_keyword(KeywordId::Import) || self.check_keyword(KeywordId::From) {
            if visibility == Visibility::Public {
                return Err(CompileError::syntax(
                    "The 'pub' modifier is not supported on imports".to_string(),
                    self.current_span(),
                ));
            }
            Declaration::Import(self.import_decl()?)
        } else if self.check_keyword(KeywordId::Const) {
            Declaration::Const(self.const_decl_with_visibility(visibility)?)
        } else if self.check_keyword(KeywordId::Model) {
            Declaration::Model(self.model_decl(decorators, visibility)?)
        } else if self.check_keyword(KeywordId::Class) {
            Declaration::Class(self.class_decl(decorators, visibility)?)
        } else if self.check_keyword(KeywordId::Trait) {
            Declaration::Trait(self.trait_decl(decorators, visibility)?)
        } else if self.check_keyword(KeywordId::Type) || self.check_keyword(KeywordId::Newtype) {
            Declaration::Newtype(self.newtype_decl(visibility)?)
        } else if self.check_keyword(KeywordId::Enum) {
            Declaration::Enum(self.enum_decl(decorators, visibility)?)
        } else if self.check_keyword(KeywordId::Def) || self.check_keyword(KeywordId::Async) {
            Declaration::Function(self.function_decl(decorators, visibility)?)
        } else {
            return Err(CompileError::syntax(
                format!("Expected declaration, found {:?}", self.peek().kind),
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

    fn decorators(&mut self) -> Result<Vec<Spanned<Decorator>>, CompileError> {
        let mut decorators = Vec::new();
        while self.match_punct(PunctuationId::At) {
            let start = self.tokens[self.pos - 1].span.start;
            let name = self.identifier()?;
            let args = if self.match_punct(PunctuationId::LParen) {
                let args = self.decorator_args()?;
                self.expect_punct(PunctuationId::RParen, "Expected ')' after decorator arguments")?;
                args
            } else {
                Vec::new()
            };
            let end = self.tokens[self.pos - 1].span.end;
            decorators.push(Spanned::new(Decorator { name, args }, Span::new(start, end)));
            self.skip_newlines();
        }
        Ok(decorators)
    }

    fn decorator_args(&mut self) -> Result<Vec<DecoratorArg>, CompileError> {
        let mut args = Vec::new();
        if !self.check_punct(PunctuationId::RParen) {
            loop {
                // Check for named argument (name: Type or name=value)
                if let TokenKind::Ident(name) = &self.peek().kind {
                    let name = name.clone();
                    if self.peek_next().kind == TokenKind::Punctuation(PunctuationId::Colon) {
                        self.advance(); // consume name
                        self.advance(); // consume :
                        let ty = self.type_expr()?;
                        args.push(DecoratorArg::Named(name, DecoratorArgValue::Type(ty)));
                    } else if self.peek_next().kind == TokenKind::Operator(OperatorId::Eq) {
                        self.advance(); // consume name
                        self.advance(); // consume =
                        let expr = self.expression()?;
                        args.push(DecoratorArg::Named(name, DecoratorArgValue::Expr(expr)));
                    } else {
                        let expr = self.expression()?;
                        args.push(DecoratorArg::Positional(expr));
                    }
                } else {
                    let expr = self.expression()?;
                    args.push(DecoratorArg::Positional(expr));
                }

                if !self.match_punct(PunctuationId::Comma) {
                    break;
                }
            }
        }
        Ok(args)
    }

    fn import_decl(&mut self) -> Result<ImportDecl, CompileError> {
        // Check for "from ... import ..." syntax
        if self.match_keyword(KeywordId::From) {
            // Check for "from rust::crate import ..." syntax
            if self.match_keyword(KeywordId::Rust) {
                self.expect_punct(PunctuationId::ColonColon, "Expected '::' after 'rust'")?;
                let (crate_name, path) = self.rust_crate_path()?;
                self.expect_keyword(KeywordId::Import, "Expected 'import' after rust crate path")?;

                // Parse import items
                let mut items = Vec::new();
                loop {
                    let name = self.identifier()?;
                    let alias = if self.match_keyword(KeywordId::As) {
                        Some(self.identifier()?)
                    } else {
                        None
                    };
                    items.push(ImportItem { name, alias });

                    if !self.match_punct(PunctuationId::Comma) {
                        break;
                    }
                }

                return Ok(ImportDecl {
                    kind: ImportKind::RustFrom {
                        crate_name,
                        path,
                        items,
                    },
                    alias: None,
                });
            }

            // Regular from import
            let module = self.import_path()?;
            self.expect_keyword(KeywordId::Import, "Expected 'import' after module path")?;

            // Parse import items: item1, item2 as alias, item3, ...
            let mut items = Vec::new();
            loop {
                let name = self.identifier()?;
                let alias = if self.match_keyword(KeywordId::As) {
                    Some(self.identifier()?)
                } else {
                    None
                };
                items.push(ImportItem { name, alias });

                if !self.match_punct(PunctuationId::Comma) {
                    break;
                }
            }

            return Ok(ImportDecl {
                kind: ImportKind::From { module, items },
                alias: None,
            });
        }

        // Regular import syntax (Rust-style)
        self.expect_keyword(KeywordId::Import, "Expected 'import'")?;

        let kind = if self.match_keyword(KeywordId::Python) {
            // Python import: import python "package" as alias
            let pkg = self.string_literal()?;
            ImportKind::Python(pkg)
        } else if self.match_keyword(KeywordId::Rust) {
            // Rust crate import: import rust::serde_json or import rust::serde_json::Value
            self.expect_punct(PunctuationId::ColonColon, "Expected '::' after 'rust'")?;
            let (crate_name, path) = self.rust_crate_path()?;
            ImportKind::RustCrate { crate_name, path }
        } else {
            // Module import: import foo::bar::baz or import super::foo or import crate::foo
            let path = self.import_path()?;
            ImportKind::Module(path)
        };

        let alias = if self.match_keyword(KeywordId::As) {
            Some(self.identifier()?)
        } else {
            None
        };

        Ok(ImportDecl { kind, alias })
    }

    /// Parse a Rust crate path after `rust::`
    /// Returns (crate_name, optional_path_within_crate)
    /// Examples:
    /// - `serde_json` -> ("serde_json", [])
    /// - `serde_json::Value` -> ("serde_json", ["Value"])
    /// - `std::collections::HashMap` -> ("std", ["collections", "HashMap"])
    fn rust_crate_path(&mut self) -> Result<(String, Vec<Ident>), CompileError> {
        let crate_name = self.identifier()?;
        let mut path = Vec::new();

        while self.match_punct(PunctuationId::ColonColon) {
            let segment = self.identifier()?;
            path.push(segment);
        }

        Ok((crate_name, path))
    }

    /// Parse an import path, supporting:
    /// - Simple: `models`, `utils::helpers`
    /// - Relative with dots: `..common`, `...shared.utils`
    /// - Relative with super: `super::common`, `super::super::utils`
    /// - Absolute with crate: `crate::config`
    /// - Dotted paths: `db.models`, `api.handlers.auth`
    fn import_path(&mut self) -> Result<ImportPath, CompileError> {
        let mut parent_levels = 0;
        let mut is_absolute = false;
        let mut segments = Vec::new();

        // Check for leading `..` (Python-style parent navigation)
        while self.match_op(OperatorId::DotDot) {
            parent_levels += 1;
        }

        // Check for `crate` (absolute path)
        if parent_levels == 0 && self.match_keyword(KeywordId::Crate) {
            is_absolute = true;
            // Expect :: or . after crate
            if !self.match_punct(PunctuationId::ColonColon) && !self.match_punct(PunctuationId::Dot) {
                return Err(CompileError::syntax(
                    "Expected '::' or '.' after 'crate'".to_string(),
                    self.current_span(),
                ));
            }
        }

        // Check for `super` (Rust-style parent navigation)
        while self.match_keyword(KeywordId::Super) {
            parent_levels += 1;
            // Expect :: or . after super
            if !self.match_punct(PunctuationId::ColonColon) && !self.match_punct(PunctuationId::Dot) {
                // Could be end of path if no more segments
                if !self.check_keyword(KeywordId::Import)
                    && !self.check_keyword(KeywordId::As)
                    && !self.check(&TokenKind::Newline)
                {
                    return Err(CompileError::syntax(
                        "Expected '::' or '.' after 'super'".to_string(),
                        self.current_span(),
                    ));
                }
            }
        }

        // Parse the actual path segments
        // First segment
        if let Ok(first) = self.identifier() {
            segments.push(first);

            // Continue with :: or . separators
            loop {
                if self.match_punct(PunctuationId::ColonColon) || self.match_punct(PunctuationId::Dot) {
                    segments.push(self.identifier()?);
                } else {
                    break;
                }
            }
        }

        Ok(ImportPath {
            parent_levels,
            is_absolute,
            segments,
        })
    }

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

        // Allow empty trait body with just 'pass'
        if self.match_keyword(KeywordId::Pass) {
            self.skip_newlines();
        } else {
            while !self.check(&TokenKind::Dedent) && !self.is_at_end() {
                let method_decorators = self.decorators()?;
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

    /// Parse a newtype declaration.
    fn newtype_decl(&mut self, visibility: Visibility) -> Result<NewtypeDecl, CompileError> {
        // Support both: "type X = newtype T" and "newtype X = T"
        if self.match_keyword(KeywordId::Newtype) {
            // newtype X = T syntax
        } else {
            self.expect_keyword(KeywordId::Type, "Expected 'type' or 'newtype'")?;
        }
        let name = self.identifier()?;
        self.expect_op(OperatorId::Eq, "Expected '=' after type name")?;
        // Skip optional 'newtype' keyword if present (for "type X = newtype T" form)
        self.match_keyword(KeywordId::Newtype);
        let underlying = self.type_expr()?;

        let methods = if self.match_punct(PunctuationId::Colon) {
            self.expect(&TokenKind::Newline, "Expected newline after ':'")?;
            self.expect(&TokenKind::Indent, "Expected indented block")?;

            let mut methods = Vec::new();
            self.skip_newlines();
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

        Ok(NewtypeDecl {
            visibility,
            name,
            underlying,
            methods,
        })
    }

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
        self.expect(&TokenKind::Indent, "Expected indented block")?;

        // Skip optional docstring at the start of the enum body
        self.skip_newlines();
        if let TokenKind::String(_) = &self.peek().kind {
            // Consume the docstring (we don't store it for now, but allow it syntactically)
            self.advance();
            self.skip_newlines();
        }

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
            variants,
        })
    }

    /// Parse a variant declaration.
    fn variant_decl(&mut self) -> Result<Spanned<VariantDecl>, CompileError> {
        let start = self.current_span().start;
        // Allow keywords like "None" as variant names (Rust allows this)
        let name = self.identifier_or_keyword()?;
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

    /// Parse a function declaration.
    fn function_decl(
        &mut self,
        decorators: Vec<Spanned<Decorator>>,
        visibility: Visibility,
    ) -> Result<FunctionDecl, CompileError> {
        let is_async = self.match_keyword(KeywordId::Async);
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
            is_async,
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
        let is_async = self.match_keyword(KeywordId::Async);
        self.expect_keyword(KeywordId::Def, "Expected 'def'")?;
        let name = self.identifier()?;
        self.expect_punct(PunctuationId::LParen, "Expected '(' after method name")?;

        // Parse receiver and params
        let (receiver, params) = self.receiver_and_params()?;

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
            return Err(CompileError::syntax(
                "Expected ':' after return type or newline for abstract method".to_string(),
                self.current_span(),
            ));
        };

        let end = self.tokens[self.pos - 1].span.end;

        Ok(Spanned::new(
            MethodDecl {
                decorators,
                is_async,
                name,
                receiver,
                params,
                return_type,
                body,
            },
            Span::new(start, end),
        ))
    }

    /// Parse a receiver and parameters.
    fn receiver_and_params(&mut self) -> Result<(Option<Receiver>, Vec<Spanned<Param>>), CompileError> {
        // Check for receiver
        let receiver = if self.check_keyword(KeywordId::Mut) {
            self.advance();
            self.expect(&TokenKind::Keyword(KeywordId::SelfKw), "Expected 'self' after 'mut'")?;
            if self.check(&TokenKind::Punctuation(PunctuationId::Comma)) {
                self.advance();
            }
            Some(Receiver::Mutable)
        } else if self.check_keyword(KeywordId::SelfKw) {
            self.advance();
            if self.check(&TokenKind::Punctuation(PunctuationId::Comma)) {
                self.advance();
            }
            Some(Receiver::Immutable)
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
        let mut params = Vec::new();
        if !self.check(&TokenKind::Punctuation(PunctuationId::RParen)) {
            loop {
                params.push(self.param()?);
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

            // Check if it's a method (starts with def or async def)
            if self.check_keyword(KeywordId::Def) || self.check_keyword(KeywordId::Async) {
                methods.push(self.method_decl(decorators)?);
            } else {
                // It's a field
                if !decorators.is_empty() {
                    return Err(CompileError::syntax(
                        "Decorators on fields are not supported".to_string(),
                        decorators[0].span,
                    ));
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
                return Err(CompileError::syntax(
                    format!("Unknown field metadata key '{key_raw}'"),
                    key_span,
                ));
            };

            match key {
                FieldMetadataKey::Alias => {
                    if metadata.alias.is_some() {
                        return Err(CompileError::syntax(
                            format!("Duplicate '{}' metadata key", field_metadata::as_str(key)),
                            key_span,
                        ));
                    }
                    metadata.alias = Some(value);
                }
                FieldMetadataKey::Description => {
                    if metadata.description.is_some() {
                        return Err(CompileError::syntax(
                            format!("Duplicate '{}' metadata key", field_metadata::as_str(key)),
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
                return Err(CompileError::syntax(
                    "Cannot combine 'alias=\"...\"' with 'as \"...\"'".to_string(),
                    self.tokens[self.pos - 1].span,
                ));
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
