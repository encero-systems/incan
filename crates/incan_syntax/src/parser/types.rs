/// Type-expression parsing methods.
///
/// This chunk parses syntactic type expressions (annotations), including:
/// - Simple names (`int`, `Foo`)
/// - Generic applications (`List[int]`, `Callable[int, int]`)
/// - Tuple types (`(int, str)`)
/// - Function types (`(int, str) -> bool`)
/// - Type parameters with trait bounds (`[T with (Eq, Debug)]`)
///
/// ## Notes
/// - `Type` parsing is purely syntactic; semantic meaning is handled by later compiler phases.
impl<'a> Parser<'a> {
    // ========================================================================
    // Types
    // ========================================================================

    /// Parse optional type parameters: `[T, E]` or `[T with (Eq, Debug), E with Clone]`.
    ///
    /// RFC 023: Supports the `with` bound annotation syntax per the grammar:
    /// ```text
    /// type_param = IDENT [ "with" bounds ] ;
    /// bounds     = bound | "(" bound { "," bound } ")" ;
    /// bound      = IDENT [ "[" type_args "]" ] ;
    /// ```
    fn type_params(&mut self) -> Result<Vec<TypeParam>, CompileError> {
        if self.match_token(&TokenKind::Punctuation(PunctuationId::LBracket)) {
            let mut params = Vec::new();
            loop {
                let name = self.identifier()?;
                let bounds = self.type_param_bounds()?;
                params.push(TypeParam { name, bounds });
                if !self.match_token(&TokenKind::Punctuation(PunctuationId::Comma)) {
                    break;
                }
                // Allow trailing comma before `]`
                if self.check(&TokenKind::Punctuation(PunctuationId::RBracket)) {
                    break;
                }
            }
            self.expect(
                &TokenKind::Punctuation(PunctuationId::RBracket),
                "Expected ']' after type parameters",
            )?;
            Ok(params)
        } else {
            Ok(Vec::new())
        }
    }

    /// Parse optional `with` bounds on a type parameter.
    ///
    /// Returns an empty vec if no `with` keyword follows.
    fn type_param_bounds(&mut self) -> Result<Vec<TraitBound>, CompileError> {
        if !self.match_keyword(KeywordId::With) {
            return Ok(Vec::new());
        }

        // ---- Single bound (bare word) vs multiple bounds (parenthesised) ----
        if self.match_token(&TokenKind::Punctuation(PunctuationId::LParen)) {
            // Multiple bounds: `with (Eq, Debug, From[U])`
            let mut bounds = Vec::new();
            loop {
                bounds.push(self.trait_bound()?);
                if !self.match_token(&TokenKind::Punctuation(PunctuationId::Comma)) {
                    break;
                }
                // Allow trailing comma before `)`
                if self.check(&TokenKind::Punctuation(PunctuationId::RParen)) {
                    break;
                }
            }
            self.expect(
                &TokenKind::Punctuation(PunctuationId::RParen),
                "Expected ')' after trait bounds",
            )?;
            Ok(bounds)
        } else {
            // Single bound: `with Clone` or `with From[U]`
            Ok(vec![self.trait_bound()?])
        }
    }

    /// Parse a single trait bound: `Eq` or `From[U]`.
    fn trait_bound(&mut self) -> Result<TraitBound, CompileError> {
        let name = self.identifier_or_from_keyword()?;
        let type_args = if self.match_token(&TokenKind::Punctuation(PunctuationId::LBracket)) {
            let args = self.type_list()?;
            self.expect(
                &TokenKind::Punctuation(PunctuationId::RBracket),
                "Expected ']' after trait bound type arguments",
            )?;
            args
        } else {
            Vec::new()
        };
        Ok(TraitBound { name, type_args })
    }

    /// Parse one trait bound and attach a span covering the full bound (RFC 042: supertraits on `trait` decls).
    fn trait_bound_spanned(&mut self) -> Result<Spanned<TraitBound>, CompileError> {
        let start = self.current_span().start;
        let bound = self.trait_bound()?;
        let end = self.tokens[self.pos - 1].span.end;
        Ok(Spanned::new(bound, Span::new(start, end)))
    }

    /// Comma-separated supertrait bounds after `with` on a trait declaration.
    fn trait_supertrait_list_spanned(&mut self) -> Result<Vec<Spanned<TraitBound>>, CompileError> {
        let mut bounds = vec![self.trait_bound_spanned()?];
        while self.match_token(&TokenKind::Punctuation(PunctuationId::Comma)) {
            bounds.push(self.trait_bound_spanned()?);
        }
        Ok(bounds)
    }

    /// Parse a type expression, including RFC 029 `A | B` union sugar.
    fn type_expr(&mut self) -> Result<Spanned<Type>, CompileError> {
        let first = self.type_atom()?;
        if !self.match_punct(PunctuationId::Pipe) {
            return Ok(first);
        }

        let start = first.span.start;
        let mut members = vec![first];
        loop {
            members.push(self.type_atom()?);
            if !self.match_punct(PunctuationId::Pipe) {
                break;
            }
        }
        let end = members
            .last()
            .map(|member| member.span.end)
            .unwrap_or(start);
        let mut flattened = Vec::new();
        for member in members {
            match member.node {
                Type::Generic(name, args) if name == "Union" => flattened.extend(args),
                other => flattened.push(Spanned::new(other, member.span)),
            }
        }

        Ok(Spanned::new(
            Type::Generic("Union".to_string(), flattened),
            Span::new(start, end),
        ))
    }

    /// Parse a single type atom before any outer union composition is applied.
    fn type_atom(&mut self) -> Result<Spanned<Type>, CompileError> {
        let start = self.current_span().start;

        // Unit type
        if self.match_token(&TokenKind::Punctuation(PunctuationId::LParen)) {
            if self.match_token(&TokenKind::Punctuation(PunctuationId::RParen)) {
                // Could be unit type () or zero-arg function type () -> T
                if self.match_token(&TokenKind::Punctuation(PunctuationId::Arrow)) {
                    let ret = self.type_expr()?;
                    let end = ret.span.end;
                    return Ok(Spanned::new(
                        Type::Function(vec![], Box::new(ret)),
                        Span::new(start, end),
                    ));
                }
                let end = self.tokens[self.pos - 1].span.end;
                return Ok(Spanned::new(Type::Unit, Span::new(start, end)));
            }
            // Could be tuple type or function type
            let first = self.type_expr()?;
            if self.match_token(&TokenKind::Punctuation(PunctuationId::Comma)) {
                // Tuple type
                let mut types = vec![first];
                if !self.check(&TokenKind::Punctuation(PunctuationId::RParen)) {
                    loop {
                        types.push(self.type_expr()?);
                        if !self.match_token(&TokenKind::Punctuation(PunctuationId::Comma)) {
                            break;
                        }
                    }
                }
                self.expect(
                    &TokenKind::Punctuation(PunctuationId::RParen),
                    "Expected ')' after tuple type",
                )?;

                // Check for function type
                if self.match_token(&TokenKind::Punctuation(PunctuationId::Arrow)) {
                    let ret = self.type_expr()?;
                    let end = ret.span.end;
                    return Ok(Spanned::new(
                        Type::Function(types, Box::new(ret)),
                        Span::new(start, end),
                    ));
                }

                let end = self.tokens[self.pos - 1].span.end;
                return Ok(Spanned::new(Type::Tuple(types), Span::new(start, end)));
            }
            self.expect(&TokenKind::Punctuation(PunctuationId::RParen), "Expected ')'")?;

            // Check for function type
            if self.match_token(&TokenKind::Punctuation(PunctuationId::Arrow)) {
                let ret = self.type_expr()?;
                let end = ret.span.end;
                return Ok(Spanned::new(
                    Type::Function(vec![first], Box::new(ret)),
                    Span::new(start, end),
                ));
            }

            // Just a parenthesized type
            return Ok(first);
        }

        // Handle None as a type (alias for unit/void)
        if self.match_token(&TokenKind::Keyword(KeywordId::None)) {
            let end = self.tokens[self.pos - 1].span.end;
            return Ok(Spanned::new(Type::Simple("None".to_string()), Span::new(start, end)));
        }

        if let TokenKind::Int(value) = &self.peek().kind {
            let value = value.clone();
            self.advance();
            let end = self.tokens[self.pos - 1].span.end;
            return Ok(Spanned::new(Type::IntLiteral(value), Span::new(start, end)));
        }

        // Named type (optionally `::`-qualified for Rust paths: `proto_mod::Binary`)
        let name = self.identifier()?;

        // Check for Self type (refers to the implementing type in traits)
        if name == "Self" {
            let end = self.tokens[self.pos - 1].span.end;
            return Ok(Spanned::new(Type::SelfType, Span::new(start, end)));
        }

        let mut path = vec![name];
        while self.match_punct(PunctuationId::ColonColon) {
            path.push(self.identifier_or_any_keyword()?);
        }

        // Check for generic arguments (only on a simple name, not `a::B[T]` yet)
        if self.match_token(&TokenKind::Punctuation(PunctuationId::LBracket)) {
            if path.len() != 1 {
                return Err(CompileError::syntax(
                    "Generics on qualified type paths (`a::B[T]`) are not supported yet; import the concrete type directly"
                        .to_string(),
                    Span::new(start, self.current_span().start),
                ));
            }
            let type_name = path[0].clone();
            let args = self.type_list()?;
            self.expect(
                &TokenKind::Punctuation(PunctuationId::RBracket),
                "Expected ']' after type arguments",
            )?;
            let end = self.tokens[self.pos - 1].span.end;
            if type_name == "Callable" {
                return self.desugar_callable_type(args, start, end);
            }
            Ok(Spanned::new(
                Type::Generic(type_name, args),
                Span::new(start, end),
            ))
        } else if path.len() == 1 {
            let end = self.tokens[self.pos - 1].span.end;
            Ok(Spanned::new(
                Type::Simple(path[0].clone()),
                Span::new(start, end),
            ))
        } else {
            let end = self.tokens[self.pos - 1].span.end;
            Ok(Spanned::new(Type::Qualified(path), Span::new(start, end)))
        }
    }

    /// RFC 035: desugar `Callable[Params, R]` into `Type::Function`.
    ///
    /// - `Callable[(), R]` => `() -> R`
    /// - `Callable[A, R]` => `(A) -> R`
    /// - `Callable[(A, B), R]` => `(A, B) -> R`
    fn desugar_callable_type(
        &mut self,
        mut args: Vec<Spanned<Type>>,
        start: usize,
        end: usize,
    ) -> Result<Spanned<Type>, CompileError> {
        if args.len() != 2 {
            return Err(CompileError::new(
                "Callable[...] expects exactly 2 type arguments: Callable[Params, Return]".to_string(),
                Span::new(start, end),
            ));
        }

        let params_arg = args.remove(0);
        let ret = Box::new(args.remove(0));
        let params = match params_arg.node {
            Type::Tuple(types) => types,
            Type::Unit => Vec::new(),
            other => vec![Spanned::new(other, params_arg.span)],
        };

        Ok(Spanned::new(
            Type::Function(params, ret),
            Span::new(start, end),
        ))
    }

    fn type_list(&mut self) -> Result<Vec<Spanned<Type>>, CompileError> {
        let mut types = Vec::new();
        if !self.check(&TokenKind::Punctuation(PunctuationId::RBracket))
            && !self.check(&TokenKind::Punctuation(PunctuationId::RParen))
        {
            loop {
                types.push(self.type_expr()?);
                if !self.match_token(&TokenKind::Punctuation(PunctuationId::Comma)) {
                    break;
                }
            }
        }
        Ok(types)
    }

}
