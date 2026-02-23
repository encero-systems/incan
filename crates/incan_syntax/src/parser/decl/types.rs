/// Type and newtype declarations.
impl<'a> Parser<'a> {
    /// Parse a newtype declaration.
    fn newtype_decl(&mut self, visibility: Visibility) -> Result<NewtypeDecl, CompileError> {
        // Support both: "type X = newtype T" and "newtype X = T"
        if self.match_keyword(KeywordId::Newtype) {
            // newtype X = T syntax
        } else {
            self.expect_keyword(KeywordId::Type, "Expected 'type' or 'newtype'")?;
        }
        let name = self.identifier()?;
        let type_params = self.type_params()?;
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
            type_params,
            underlying,
            methods,
        })
    }
}
