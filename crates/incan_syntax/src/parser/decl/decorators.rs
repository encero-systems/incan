/// Decorator parsing (`@decorator(...)`) for declarations and methods.
impl<'a> Parser<'a> {
    /// Parse all leading decorators for the next declaration.
    fn decorators(&mut self) -> Result<Vec<Spanned<Decorator>>, CompileError> {
        let mut decorators = Vec::new();
        while self.match_punct(PunctuationId::At) {
            let start = self.tokens[self.pos - 1].span.start;
            let path = self.import_path()?;
            let name = path
                .segments
                .last()
                .cloned()
                .ok_or_else(|| errors::decorator_path_expected(self.current_span()))?;
            let type_args = self.call_site_type_args()?;
            let is_call = self.match_punct(PunctuationId::LParen);
            let args = if is_call {
                let args = self.decorator_args()?;
                self.expect_punct(PunctuationId::RParen, "Expected ')' after decorator arguments")?;
                args
            } else {
                Vec::new()
            };
            let end = self.tokens[self.pos - 1].span.end;
            decorators.push(Spanned::new(
                Decorator {
                    path,
                    name,
                    type_args,
                    is_call,
                    args,
                },
                Span::new(start, end),
            ));
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
}
