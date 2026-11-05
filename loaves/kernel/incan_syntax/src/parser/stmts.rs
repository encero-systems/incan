/// How the right side of a tuple statement may be spelled where it is parsed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TupleValueSpelling {
    /// Statement position: `a, b = b, a` reads the right side as the tuple `(b, a)`, like `a, b = (b, a)`.
    BareOrParenthesized,
    /// An item of a braced vocab body, where a comma separates items: the right side is one expression.
    SingleExpression,
}

/// Statement parsing methods.
///
/// This chunk parses statement forms (e.g. `if`, `while`, `for`, `return`, assignments) as well as indentation-based
/// blocks.
///
/// ## Notes
/// - Block parsing relies on `Indent` / `Dedent` layout tokens produced by the lexer.
impl<'a> Parser<'a> {
    // ========================================================================
    // Statements
    // ========================================================================

    /// Parse a statement block.
    fn block(&mut self) -> Result<Vec<Spanned<Statement>>, CompileError> {
        let mut stmts = Vec::new();
        let mut next_leading = self.consume_inter_statement_blank_prefix();
        while !self.check(&TokenKind::Dedent) && !self.is_at_end() {
            let mut stmt = self.statement()?;
            stmt.leading_blank_lines = next_leading;
            stmts.push(stmt);
            next_leading = self.consume_inter_statement_blank_prefix();
            if next_leading > 0 && self.check(&TokenKind::Dedent) && self.dedent_is_followed_by_outer_statement() {
                self.pending_dedent_blank_lines = self.pending_dedent_blank_lines.max(next_leading);
                next_leading = 0;
            }
        }
        Ok(stmts)
    }

    /// Return whether the current `Dedent` leads back to an outer statement, not a declaration/member boundary.
    fn dedent_is_followed_by_outer_statement(&self) -> bool {
        let mut idx = self.pos;
        while matches!(self.tokens.get(idx).map(|token| &token.kind), Some(TokenKind::Dedent)) {
            idx += 1;
        }

        !matches!(
            self.tokens.get(idx).map(|token| &token.kind),
            None | Some(TokenKind::Eof)
                | Some(TokenKind::Keyword(
                    KeywordId::Async
                        | KeywordId::Class
                        | KeywordId::Const
                        | KeywordId::Def
                        | KeywordId::Enum
                        | KeywordId::From
                        | KeywordId::Import
                        | KeywordId::Model
                        | KeywordId::Newtype
                        | KeywordId::Pub
                        | KeywordId::Rust
                        | KeywordId::Static
                        | KeywordId::Trait
                        | KeywordId::Type
                ))
        )
    }

    /// Parse one statement.
    fn statement(&mut self) -> Result<Spanned<Statement>, CompileError> {
        let start = self.current_span().start;

        let stmt = if self.check_keyword(KeywordId::Return) {
            self.return_stmt()?
        } else if self.check_keyword(KeywordId::If) {
            self.if_stmt()?
        } else if self.check_keyword(KeywordId::Loop) {
            self.loop_stmt()?
        } else if self.check_keyword(KeywordId::While) {
            self.while_stmt()?
        } else if self.check_keyword(KeywordId::For) {
            self.for_stmt()?
        } else if self.starts_unsafe_block() {
            self.unsafe_stmt()?
        } else if self.is_assert_statement_keyword() {
            self.assert_stmt()?
        } else if let Some(vocab_block) = self.try_vocab_block_statement()? {
            vocab_block
        } else if let Some(surface_stmt) = self.try_surface_keyword_statement()? {
            surface_stmt
        } else if self.check_keyword(KeywordId::Break) {
            self.break_stmt()?
        } else if self.check_keyword(KeywordId::Continue) {
            self.advance();
            Statement::Continue
        } else if self.check_keyword(KeywordId::Pass) {
            self.advance();
            Statement::Pass
        } else if self.check_keyword(KeywordId::Static) {
            return Err(errors::static_only_allowed_at_module_scope(self.current_span()));
        } else if self.check(&TokenKind::Punctuation(PunctuationId::Ellipsis)) {
            // ... is equivalent to pass (Python-style placeholder)
            self.advance();
            Statement::Pass
        } else if self.check_keyword(KeywordId::Let) || self.check_keyword(KeywordId::Mut) {
            self.assignment_stmt(TupleValueSpelling::BareOrParenthesized)?
        } else {
            // Could be assignment or expression
            self.assignment_or_expr_stmt()?
        };

        let end = self.tokens[self.pos.saturating_sub(1)].span.end;
        Ok(Spanned::new(stmt, Span::new(start, end)))
    }

    /// Parse a single inline statement (for use in inline case arms).
    fn inline_statement(&mut self) -> Result<Spanned<Statement>, CompileError> {
        let start = self.current_span().start;

        let stmt = if self.check_keyword(KeywordId::Return) {
            self.advance();
            let expr = if !self.check(&TokenKind::Newline)
                && !self.check(&TokenKind::Keyword(KeywordId::Case))
                && !self.check(&TokenKind::Dedent)
            {
                Some(self.expression()?)
            } else {
                None
            };
            Statement::Return(expr)
        } else if self.check_keyword(KeywordId::Break) {
            self.break_stmt()?
        } else if self.check_keyword(KeywordId::Continue) {
            self.advance();
            Statement::Continue
        } else if self.check_keyword(KeywordId::Pass) || self.check(&TokenKind::Punctuation(PunctuationId::Ellipsis)) {
            self.advance();
            Statement::Pass
        } else if self.check_keyword(KeywordId::Static) {
            return Err(errors::static_only_allowed_at_module_scope(self.current_span()));
        } else if self.is_assert_statement_keyword() {
            self.assert_stmt()?
        } else if let Some(surface_stmt) = self.try_surface_keyword_statement()? {
            surface_stmt
        } else if self.check_keyword(KeywordId::Let) || self.check_keyword(KeywordId::Mut) {
            self.assignment_stmt(TupleValueSpelling::BareOrParenthesized)?
        } else {
            self.assignment_or_expr_stmt()?
        };

        let end = self.tokens[self.pos.saturating_sub(1)].span.end;
        Ok(Spanned::new(stmt, Span::new(start, end)))
    }

    /// Parse a raw vocab block statement driven by imported keyword registrations.
    fn try_vocab_block_statement(&mut self) -> Result<Option<Statement>, CompileError> {
        let decorators = if self.check_punct(PunctuationId::At) {
            self.decorators()?
        } else {
            Vec::new()
        };

        self.try_vocab_block(decorators)
            .map(|block| block.map(Statement::VocabBlock))
    }
}

include!("stmts/vocab_blocks.rs");
include!("stmts/simple_statements.rs");
include!("stmts/assignments.rs");
