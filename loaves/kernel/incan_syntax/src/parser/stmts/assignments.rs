/// Assignment, tuple assignment, compound assignment and vocab expression-list statements.
impl<'a> Parser<'a> {
    /// Parse a named assignment, chained assignment or tuple-unpacking statement.
    ///
    /// A type annotation declares one binding, so an annotated chain (`x: T = y = value`) is refused. `spelling` says
    /// whether a tuple unpacking's value may be a bare comma-separated tuple (`a, b = b, a`); see
    /// [`Self::tuple_assignment_value`].
    fn assignment_stmt(&mut self, spelling: TupleValueSpelling) -> Result<Statement, CompileError> {
        let binding = if self.match_token(&TokenKind::Keyword(KeywordId::Let)) {
            BindingKind::Let
        } else if self.match_token(&TokenKind::Keyword(KeywordId::Mut)) {
            BindingKind::Mutable
        } else {
            BindingKind::Inferred
        };

        let name = self.identifier_spanned()?;

        // Check for tuple unpacking: a, b, c = expr
        if self.match_token(&TokenKind::Punctuation(PunctuationId::Comma)) {
            let mut names = vec![name.node];
            let mut name_spans = vec![name.span];
            loop {
                let name = self.identifier_spanned()?;
                names.push(name.node);
                name_spans.push(name.span);
                if !self.match_token(&TokenKind::Punctuation(PunctuationId::Comma)) {
                    break;
                }
            }
            self.expect(&TokenKind::Operator(OperatorId::Eq), "Expected '=' in tuple unpacking")?;
            let value = self.tuple_assignment_value(spelling)?;
            return Ok(Statement::TupleUnpack(TupleUnpackStmt {
                binding,
                names,
                name_spans,
                value,
            }));
        }

        let ty = if self.match_token(&TokenKind::Punctuation(PunctuationId::Colon)) {
            Some(self.type_expr()?)
        } else {
            None
        };
        self.expect(&TokenKind::Operator(OperatorId::Eq), "Expected '=' in assignment")?;

        // Check for chained assignment: x = y = z = 5
        // Collect all targets before the final value
        let mut targets = vec![name.node];
        let mut target_spans = vec![name.span];
        while let TokenKind::Ident(_) = &self.peek().kind {
            if self.peek_next().kind == TokenKind::Operator(OperatorId::Eq) {
                if let Some(annotation) = &ty {
                    return Err(errors::annotated_chained_assignment(annotation.span));
                }
                let target = self.identifier_spanned()?;
                targets.push(target.node);
                target_spans.push(target.span);
                self.expect(
                    &TokenKind::Operator(OperatorId::Eq),
                    "Expected '=' in chained assignment",
                )?;
            } else {
                break;
            }
        }

        let value = self.expression()?;

        // If we have multiple targets, create a ChainedAssignment
        if targets.len() > 1 {
            Ok(Statement::ChainedAssignment(ChainedAssignmentStmt {
                binding,
                targets,
                target_spans,
                value,
            }))
        } else {
            Ok(Statement::Assignment(AssignmentStmt {
                binding,
                name: targets.remove(0),
                name_span: target_spans.remove(0),
                ty,
                value,
            }))
        }
    }

    /// Parse the value side of a tuple unpacking (`a, b = ...`) or a tuple assignment (`self.x, self.y = ...`).
    ///
    /// The value may be written as a bare comma-separated tuple, the Python swap idiom `a, b = b, a`. It builds the
    /// same [`Expr::Tuple`] as the parenthesized spelling `a, b = (b, a)`, spanning the first element to the last,
    /// so the checker and everything after it see one shape whichever spelling the source uses. A single expression
    /// is returned unchanged.
    ///
    /// With [`TupleValueSpelling::SingleExpression`] (an item of a braced vocab body, where a comma separates items)
    /// the value is one expression and a following comma is left for the body.
    fn tuple_assignment_value(&mut self, spelling: TupleValueSpelling) -> Result<Spanned<Expr>, CompileError> {
        let first = self.expression()?;
        if spelling == TupleValueSpelling::SingleExpression || !self.tuple_value_continues() {
            return Ok(first);
        }

        let start = first.span.start;
        let mut elements = vec![first];
        while self.tuple_value_continues() {
            self.advance(); // the comma separating the next element
            elements.push(self.expression()?);
        }
        let end = elements.last().map_or(start, |element| element.span.end);
        Ok(Spanned::new(Expr::Tuple(elements), Span::new(start, end)))
    }

    /// Return whether the current token is a comma that introduces another element of a bare tuple value.
    ///
    /// A comma followed by the end of the line, the end of the block or a closing delimiter introduces no element, so
    /// it is left in place for the enclosing construct.
    fn tuple_value_continues(&self) -> bool {
        self.check(&TokenKind::Punctuation(PunctuationId::Comma))
            && !matches!(
                self.peek_next().kind,
                TokenKind::Newline
                    | TokenKind::Dedent
                    | TokenKind::Eof
                    | TokenKind::Punctuation(PunctuationId::RParen | PunctuationId::RBracket | PunctuationId::RBrace)
            )
    }

    /// Parse either an assignment-like statement or a plain expression statement in statement position.
    ///
    /// A tuple assignment's value may be a bare comma-separated tuple (`self.x, self.y = self.y, self.x`); see
    /// [`Self::tuple_assignment_value`].
    fn assignment_or_expr_stmt(&mut self) -> Result<Statement, CompileError> {
        self.assignment_or_expr_item(TupleValueSpelling::BareOrParenthesized)
    }

    /// Parse either an assignment-like statement or a plain expression statement.
    ///
    /// `spelling` says whether a tuple statement's right side may be a bare comma-separated tuple: it may in statement
    /// position, and may not in an item of a braced vocab body, where a comma separates items. There
    /// `{ a, b = pair, c = 3 }` is two items and `{ a, b = x, y }` is the unpacking `a, b = x` followed by the item
    /// `y`.
    fn assignment_or_expr_item(&mut self, spelling: TupleValueSpelling) -> Result<Statement, CompileError> {
        if matches!(
            &self.peek().kind,
            TokenKind::Keyword(KeywordId::Async | KeywordId::Await)
        ) && matches!(
            self.peek_next().kind,
            TokenKind::Operator(OperatorId::Eq) | TokenKind::Punctuation(PunctuationId::Colon | PunctuationId::Comma)
        ) {
            return Err(self.identifier_error());
        }

        // Look for `ident = expr` or `ident, ident = expr` pattern (simple or tuple assignment)
        if let TokenKind::Ident(_) = &self.peek().kind {
            // Check if next is = or : (for assignment) or , (for tuple unpacking)
            if self.peek_next().kind == TokenKind::Operator(OperatorId::Eq)
                || (self.peek_next().kind == TokenKind::Punctuation(PunctuationId::Colon)
                    && !self.active_scoped_glyph_starts_at_offset(1))
                || self.peek_next().kind == TokenKind::Punctuation(PunctuationId::Comma)
            {
                return self.assignment_stmt(spelling);
            }
            // Check for compound assignment: ident += expr, ident -= expr, etc.
            let compound_op = Self::compound_op_from_token_kind(&self.peek_next().kind);
            if let Some(op) = compound_op {
                let name = self.identifier_spanned()?;
                self.advance(); // consume the compound operator
                let value = self.expression()?;
                return Ok(Statement::CompoundAssignment(CompoundAssignmentStmt {
                    name: name.node,
                    name_span: name.span,
                    op,
                    value,
                }));
            }
        }

        // Parse the expression (could be field access like self.field or index like arr[i])
        let expr = self.expression()?;

        if let Some(item) = self.vocab_expression_list_item_tail(expr.clone())? {
            return Ok(Statement::VocabExpressionItem(item));
        }

        // Check for tuple assignment: expr, expr, ... = value
        // This handles patterns like: arr[i], arr[j] = arr[j], arr[i]
        if self.match_token(&TokenKind::Punctuation(PunctuationId::Comma)) {
            let mut targets = vec![expr];
            loop {
                let target = self.expression()?;
                targets.push(target);
                if !self.match_token(&TokenKind::Punctuation(PunctuationId::Comma)) {
                    break;
                }
            }
            self.expect(&TokenKind::Operator(OperatorId::Eq), "Expected '=' in tuple assignment")?;
            let value = self.tuple_assignment_value(spelling)?;
            return Ok(Statement::TupleAssign(TupleAssignStmt { targets, value }));
        }

        // Check for assignment: expr.field = value or expr[index] = value
        if self.match_token(&TokenKind::Operator(OperatorId::Eq)) {
            match expr.node {
                Expr::Field(object, field) => {
                    let value = self.expression()?;
                    return Ok(Statement::FieldAssignment(FieldAssignmentStmt {
                        target_span: expr.span,
                        object: *object,
                        field,
                        compound_op: None,
                        value,
                    }));
                }
                Expr::Index(object, index) => {
                    let value = self.expression()?;
                    return Ok(Statement::IndexAssignment(IndexAssignmentStmt {
                        object: *object,
                        index: *index,
                        compound_op: None,
                        value,
                    }));
                }
                _ => {
                    return Err(errors::invalid_assignment_target(expr.span));
                }
            }
        }

        // Check for compound assignment on field/index: expr.field += value, expr[i] -= value
        let compound_op = Self::compound_op_from_token_kind(&self.peek().kind);
        if let Some(op) = compound_op {
            self.advance(); // consume the compound operator
            let rhs = self.expression()?;
            return Self::compound_assignment_for_target(expr, op, rhs);
        }

        // Otherwise it's an expression statement
        Ok(Statement::Expr(expr))
    }

    /// Preserve the compound operation and its expanded value on a field, index or identifier assignment target.
    ///
    /// Invalid targets retain their original span. Field and index expressions are copied into the binary value as
    /// required by the existing AST representation; later stages still receive the explicit compound-operation fact.
    fn compound_assignment_for_target(
        expr: Spanned<Expr>,
        op: CompoundOp,
        rhs: Spanned<Expr>,
    ) -> Result<Statement, CompileError> {
        match expr.node {
            Expr::Field(object, field) => {
                let field_expr = Spanned::new(Expr::Field(object.clone(), field.clone()), expr.span);
                let bin_op = op.binary_op();
                let new_value = Spanned::new(Expr::Binary(Box::new(field_expr), bin_op, Box::new(rhs)), expr.span);
                Ok(Statement::FieldAssignment(FieldAssignmentStmt {
                    target_span: expr.span,
                    object: *object,
                    field,
                    compound_op: Some(op),
                    value: new_value,
                }))
            }
            Expr::Index(object, index) => {
                let index_expr = Spanned::new(Expr::Index(object.clone(), index.clone()), expr.span);
                let bin_op = op.binary_op();
                let new_value = Spanned::new(Expr::Binary(Box::new(index_expr), bin_op, Box::new(rhs)), expr.span);
                Ok(Statement::IndexAssignment(IndexAssignmentStmt {
                    object: *object,
                    index: *index,
                    compound_op: Some(op),
                    value: new_value,
                }))
            }
            Expr::Ident(name) => Ok(Statement::CompoundAssignment(CompoundAssignmentStmt {
                name,
                name_span: expr.span,
                op,
                value: rhs,
            })),
            _ => Err(errors::invalid_compound_assignment_target(expr.span)),
        }
    }

    /// Return whether the current vocab body is contractually an expression list.
    fn vocab_expression_list_items_enabled(&self) -> bool {
        matches!(
            self.vocab_body_kind_stack.last(),
            Some(Some(incan_vocab::ClauseBodyKind::ExpressionList))
        )
    }

    /// Parse declared trailing keyword payloads for one expression-list item.
    fn vocab_expression_list_item_tail(
        &mut self,
        expr: Spanned<Expr>,
    ) -> Result<Option<VocabExpressionItemStmt>, CompileError> {
        if !self.vocab_expression_list_items_enabled() {
            return Ok(None);
        }

        let mut alias = None;
        let mut modifiers = Vec::new();
        let mut saw_tail = false;

        while let Some(surface) = self.current_expression_item_modifier_surface() {
            let keyword_span = self.current_span();
            self.advance();
            saw_tail = true;
            match surface.kind {
                incan_vocab::ExpressionItemModifierKind::Alias => {
                    if alias.is_some() {
                        return Err(CompileError::syntax(
                            format!("Duplicate expression-list alias modifier `{}`", surface.keyword),
                            keyword_span,
                        ));
                    }
                    alias = Some(self.identifier()?);
                }
                incan_vocab::ExpressionItemModifierKind::Expression => {
                    let value = self.expression()?;
                    let span = Span::new(keyword_span.start, value.span.end);
                    modifiers.push(VocabExpressionItemModifierStmt {
                        keyword: surface.keyword,
                        value,
                        span,
                    });
                }
                _ => {
                    return Err(CompileError::syntax(
                        format!("Unsupported expression-list modifier kind for `{}`", surface.keyword),
                        keyword_span,
                    ));
                }
            }
        }

        if saw_tail {
            Ok(Some(VocabExpressionItemStmt { expr, alias, modifiers }))
        } else {
            Ok(None)
        }
    }

    /// Return the declared expression-list item modifier matching the current token.
    fn current_expression_item_modifier_surface(&self) -> Option<incan_vocab::ExpressionItemModifierSurface> {
        let keyword = self.current_vocab_word_token()?;
        self.vocab_expression_item_modifier_stack
            .last()
            .and_then(|modifiers| modifiers.iter().find(|modifier| modifier.keyword == keyword))
            .cloned()
    }

    /// Return the current identifier/keyword spelling when it can start a DSL-owned item modifier.
    fn current_vocab_word_token(&self) -> Option<&str> {
        match &self.peek().kind {
            TokenKind::Ident(name) => Some(name.as_str()),
            TokenKind::Keyword(id) => Some(incan_lang::lang::keywords::as_str(*id)),
            _ => None,
        }
    }

    /// Convert an assignment operator token such as `+=` or `<<=` into its AST compound operator.
    fn compound_op_from_token_kind(kind: &TokenKind) -> Option<CompoundOp> {
        match kind {
            TokenKind::Operator(OperatorId::PlusEq) => Some(CompoundOp::Add),
            TokenKind::Operator(OperatorId::MinusEq) => Some(CompoundOp::Sub),
            TokenKind::Operator(OperatorId::StarEq) => Some(CompoundOp::Mul),
            TokenKind::Operator(OperatorId::SlashEq) => Some(CompoundOp::Div),
            TokenKind::Operator(OperatorId::SlashSlashEq) => Some(CompoundOp::FloorDiv),
            TokenKind::Operator(OperatorId::PercentEq) => Some(CompoundOp::Mod),
            TokenKind::Operator(OperatorId::MatMulEq) => Some(CompoundOp::MatMul),
            TokenKind::Operator(OperatorId::AmpEq) => Some(CompoundOp::BitAnd),
            TokenKind::Operator(OperatorId::PipeEq) => Some(CompoundOp::BitOr),
            TokenKind::Operator(OperatorId::CaretEq) => Some(CompoundOp::BitXor),
            TokenKind::Operator(OperatorId::ShlEq) => Some(CompoundOp::Shl),
            TokenKind::Operator(OperatorId::ShrEq) => Some(CompoundOp::Shr),
            _ => None,
        }
    }
}
