/// Type-expression parsing methods.
///
/// This chunk parses syntactic type expressions (annotations), including:
/// - Simple names (`int`, `Foo`)
/// - Generic applications (`List[int]`)
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
        let name = self.identifier()?;
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

    fn type_expr(&mut self) -> Result<Spanned<Type>, CompileError> {
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

        // Named type
        let name = self.identifier()?;

        // Check for Self type (refers to the implementing type in traits)
        if name == "Self" {
            let end = self.tokens[self.pos - 1].span.end;
            return Ok(Spanned::new(Type::SelfType, Span::new(start, end)));
        }

        // Check for generic arguments
        if self.match_token(&TokenKind::Punctuation(PunctuationId::LBracket)) {
            let args = self.type_list()?;
            self.expect(
                &TokenKind::Punctuation(PunctuationId::RBracket),
                "Expected ']' after type arguments",
            )?;

            let end = self.tokens[self.pos - 1].span.end;

            // RFC 035: Callable[Params, R] desugars to arrow form (params) -> ret
            if name == "Callable" {
                Ok(self.desugar_callable(args, start, end)?)
            } else {
                let end = self.tokens[self.pos - 1].span.end;
                Ok(Spanned::new(Type::Generic(name, args), Span::new(start, end)))
            }
        } else {
            let end = self.tokens[self.pos - 1].span.end;
            Ok(Spanned::new(Type::Simple(name), Span::new(start, end)))
        }
    }

    /// RFC 035: Desugar Callable[Params, R] to arrow form (params) -> ret.
    ///
    /// Callable always has exactly two type arguments:
    /// - Callable[(), R] → () -> R
    /// - Callable[A, R] → (A) -> R
    /// - Callable[(A, B), R] → (A, B) -> R
    fn desugar_callable(
        &self,
        args: Vec<Spanned<Type>>,
        start: usize,
        end: usize,
    ) -> Result<Spanned<Type>, CompileError> {
        if args.len() != 2 {
            return Err(crate::diagnostics::errors::callable_invalid_arg_count(args.len(), Span::new(start, end)));
        }

        let params_type = &args[0];
        let return_type = &args[1];

        // Determine if params is a tuple or single type
        let param_types = match &params_type.node {
            Type::Tuple(types) => types.clone(),
            Type::Unit => vec![],  // () -> R means zero parameters
            _ => vec![params_type.clone()],  // A -> R means one parameter
        };

        // Build the function type: (param_types...) -> return_type
        let func_type = Type::Function(param_types, Box::new(return_type.clone()));
        Ok(Spanned::new(func_type, Span::new(start, end)))
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
