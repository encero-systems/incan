/// Statement parsing methods.
///
/// This chunk parses statement forms (e.g. `if`, `while`, `for`, `return`, assignments)
/// as well as indentation-based blocks.
///
/// ## Notes
/// - Block parsing relies on `Indent` / `Dedent` layout tokens produced by the lexer.
impl<'a> Parser<'a> {
    // ========================================================================
    // Statements
    // ========================================================================

    fn block(&mut self) -> Result<Vec<Spanned<Statement>>, CompileError> {
        let mut stmts = Vec::new();
        self.skip_newlines();
        while !self.check(&TokenKind::Dedent) && !self.is_at_end() {
            stmts.push(self.statement()?);
            self.skip_newlines();
        }
        Ok(stmts)
    }

    fn statement(&mut self) -> Result<Spanned<Statement>, CompileError> {
        let start = self.current_span().start;

        let stmt = if self.check_keyword(KeywordId::Return) {
            self.return_stmt()?
        } else if self.check_keyword(KeywordId::If) {
            self.if_stmt()?
        } else if self.check_keyword(KeywordId::While) {
            self.while_stmt()?
        } else if self.check_keyword(KeywordId::For) {
            self.for_stmt()?
        } else if self.check_keyword(KeywordId::Assert) {
            self.assert_stmt()?
        } else if self.check_keyword(KeywordId::Break) {
            self.advance();
            Statement::Break
        } else if self.check_keyword(KeywordId::Continue) {
            self.advance();
            Statement::Continue
        } else if self.check_keyword(KeywordId::Pass) {
            self.advance();
            Statement::Pass
        } else if self.check(&TokenKind::Punctuation(PunctuationId::Ellipsis)) {
            // ... is equivalent to pass (Python-style placeholder)
            self.advance();
            Statement::Pass
        } else if self.check_keyword(KeywordId::Let) || self.check_keyword(KeywordId::Mut) {
            self.assignment_stmt()?
        } else {
            if let Some(err) = self.inactive_assert_statement_error() {
                return Err(err);
            }
            // Could be assignment or expression
            self.assignment_or_expr_stmt()?
        };

        let end = self.tokens[self.pos.saturating_sub(1)].span.end;
        Ok(Spanned::new(stmt, Span::new(start, end)))
    }

    /// Parse a single inline statement (for use in inline case arms)
    /// Supports: return, expression statements, pass
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
        } else if self.check_keyword(KeywordId::Pass) || self.check(&TokenKind::Punctuation(PunctuationId::Ellipsis)) {
            self.advance();
            Statement::Pass
        } else if self.check_keyword(KeywordId::Assert) {
            self.assert_stmt()?
        } else {
            if let Some(err) = self.inactive_assert_statement_error() {
                return Err(err);
            }
            // Expression statement
            let expr = self.expression()?;
            Statement::Expr(expr)
        };

        let end = self.tokens[self.pos.saturating_sub(1)].span.end;
        Ok(Spanned::new(stmt, Span::new(start, end)))
    }

    fn return_stmt(&mut self) -> Result<Statement, CompileError> {
        self.expect(&TokenKind::Keyword(KeywordId::Return), "Expected 'return'")?;
        let expr = if !self.check(&TokenKind::Newline) && !self.check(&TokenKind::Dedent) {
            Some(self.expression()?)
        } else {
            None
        };
        Ok(Statement::Return(expr))
    }

    fn if_stmt(&mut self) -> Result<Statement, CompileError> {
        self.expect(&TokenKind::Keyword(KeywordId::If), "Expected 'if'")?;
        let condition = self.expression()?;
        self.expect(
            &TokenKind::Punctuation(PunctuationId::Colon),
            "Expected ':' after if condition",
        )?;
        self.expect(&TokenKind::Newline, "Expected newline after ':'")?;
        self.expect(&TokenKind::Indent, "Expected indented block")?;
        let then_body = self.block()?;
        self.expect(&TokenKind::Dedent, "Expected dedent after if body")?;

        let mut elif_branches = vec![];
        while self.match_token(&TokenKind::Keyword(KeywordId::Elif)) {
            let elif_condition = self.expression()?;
            self.expect(
                &TokenKind::Punctuation(PunctuationId::Colon),
                "Expected ':' after elif condition",
            )?;
            self.expect(&TokenKind::Newline, "Expected newline after ':'")?;
            self.expect(&TokenKind::Indent, "Expected indented block")?;
            let elif_body = self.block()?;
            self.expect(&TokenKind::Dedent, "Expected dedent after elif body")?;
            elif_branches.push((elif_condition, elif_body));
        }

        let else_body = if self.match_token(&TokenKind::Keyword(KeywordId::Else)) {
            self.expect(&TokenKind::Punctuation(PunctuationId::Colon), "Expected ':' after else")?;
            self.expect(&TokenKind::Newline, "Expected newline after ':'")?;
            self.expect(&TokenKind::Indent, "Expected indented block")?;
            let body = self.block()?;
            self.expect(&TokenKind::Dedent, "Expected dedent after else body")?;
            Some(body)
        } else {
            None
        };

        Ok(Statement::If(IfStmt {
            condition,
            then_body,
            elif_branches,
            else_body,
        }))
    }

    fn while_stmt(&mut self) -> Result<Statement, CompileError> {
        self.expect(&TokenKind::Keyword(KeywordId::While), "Expected 'while'")?;
        let condition = self.expression()?;
        self.expect(
            &TokenKind::Punctuation(PunctuationId::Colon),
            "Expected ':' after while condition",
        )?;
        self.expect(&TokenKind::Newline, "Expected newline after ':'")?;
        self.expect(&TokenKind::Indent, "Expected indented block")?;
        let body = self.block()?;
        self.expect(&TokenKind::Dedent, "Expected dedent after while body")?;

        Ok(Statement::While(WhileStmt { condition, body }))
    }

    fn for_stmt(&mut self) -> Result<Statement, CompileError> {
        self.expect(&TokenKind::Keyword(KeywordId::For), "Expected 'for'")?;
        let var = self.identifier()?;
        self.expect(&TokenKind::Keyword(KeywordId::In), "Expected 'in' after for variable")?;
        let iter = self.expression()?;
        self.expect(
            &TokenKind::Punctuation(PunctuationId::Colon),
            "Expected ':' after for expression",
        )?;
        self.expect(&TokenKind::Newline, "Expected newline after ':'")?;
        self.expect(&TokenKind::Indent, "Expected indented block")?;
        let body = self.block()?;
        self.expect(&TokenKind::Dedent, "Expected dedent after for body")?;

        Ok(Statement::For(ForStmt { var, iter, body }))
    }

    fn assert_stmt(&mut self) -> Result<Statement, CompileError> {
        self.expect_keyword(KeywordId::Assert, "Expected 'assert'")?;
        let condition = self.expression()?;
        let message = if self.match_token(&TokenKind::Punctuation(PunctuationId::Comma)) {
            Some(self.expression()?)
        } else {
            None
        };
        Ok(Statement::Assert(AssertStmt { condition, message }))
    }

    /// Targeted soft-keyword diagnostic for `assert <expr>` when `std.testing` is not imported.
    ///
    /// Keep `assert(...)` valid as a normal function call for backwards compatibility.
    fn inactive_assert_statement_error(&self) -> Option<CompileError> {
        let TokenKind::Ident(name) = &self.peek().kind else {
            return None;
        };
        if name != incan_core::lang::keywords::as_str(KeywordId::Assert)
            || self.active_soft_keywords.contains(&KeywordId::Assert)
        {
            return None;
        }

        let looks_like_identifier_usage = matches!(
            self.peek_next().kind,
            TokenKind::Punctuation(PunctuationId::LParen)
                | TokenKind::Operator(OperatorId::Eq)
                | TokenKind::Punctuation(PunctuationId::Colon)
                | TokenKind::Punctuation(PunctuationId::Comma)
                | TokenKind::Operator(OperatorId::PlusEq)
                | TokenKind::Operator(OperatorId::MinusEq)
                | TokenKind::Operator(OperatorId::StarEq)
                | TokenKind::Operator(OperatorId::SlashEq)
                | TokenKind::Operator(OperatorId::SlashSlashEq)
                | TokenKind::Operator(OperatorId::PercentEq)
        );
        if looks_like_identifier_usage {
            return None;
        }

        Some(errors::soft_keyword_requires_import(name, "testing", self.current_span()))
    }

    fn assignment_stmt(&mut self) -> Result<Statement, CompileError> {
        let binding = if self.match_token(&TokenKind::Keyword(KeywordId::Let)) {
            BindingKind::Let
        } else if self.match_token(&TokenKind::Keyword(KeywordId::Mut)) {
            BindingKind::Mutable
        } else {
            BindingKind::Inferred
        };

        let name = self.identifier()?;

        // Check for tuple unpacking: a, b, c = expr
        if self.match_token(&TokenKind::Punctuation(PunctuationId::Comma)) {
            let mut names = vec![name];
            loop {
                names.push(self.identifier()?);
                if !self.match_token(&TokenKind::Punctuation(PunctuationId::Comma)) {
                    break;
                }
            }
            self.expect(&TokenKind::Operator(OperatorId::Eq), "Expected '=' in tuple unpacking")?;
            let value = self.expression()?;
            return Ok(Statement::TupleUnpack(TupleUnpackStmt { binding, names, value }));
        }

        let ty = if self.match_token(&TokenKind::Punctuation(PunctuationId::Colon)) {
            Some(self.type_expr()?)
        } else {
            None
        };
        self.expect(&TokenKind::Operator(OperatorId::Eq), "Expected '=' in assignment")?;

        // Check for chained assignment: x = y = z = 5
        // Collect all targets before the final value
        let mut targets = vec![name];
        while let TokenKind::Ident(_) = &self.peek().kind {
            if self.peek_next().kind == TokenKind::Operator(OperatorId::Eq) {
                targets.push(self.identifier()?);
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
                value,
            }))
        } else {
            Ok(Statement::Assignment(AssignmentStmt {
                binding,
                name: targets.remove(0),
                ty,
                value,
            }))
        }
    }

    fn assignment_or_expr_stmt(&mut self) -> Result<Statement, CompileError> {
        // Look for `ident = expr` or `ident, ident = expr` pattern (simple or tuple assignment)
        if let TokenKind::Ident(_) = &self.peek().kind {
            // Check if next is = or : (for assignment) or , (for tuple unpacking)
            if self.peek_next().kind == TokenKind::Operator(OperatorId::Eq)
                || self.peek_next().kind == TokenKind::Punctuation(PunctuationId::Colon)
                || self.peek_next().kind == TokenKind::Punctuation(PunctuationId::Comma)
            {
                return self.assignment_stmt();
            }
            // Check for compound assignment: ident += expr, ident -= expr, etc.
            let compound_op = match &self.peek_next().kind {
                TokenKind::Operator(OperatorId::PlusEq) => Some(CompoundOp::Add),
                TokenKind::Operator(OperatorId::MinusEq) => Some(CompoundOp::Sub),
                TokenKind::Operator(OperatorId::StarEq) => Some(CompoundOp::Mul),
                TokenKind::Operator(OperatorId::SlashEq) => Some(CompoundOp::Div),
                TokenKind::Operator(OperatorId::SlashSlashEq) => Some(CompoundOp::FloorDiv),
                TokenKind::Operator(OperatorId::PercentEq) => Some(CompoundOp::Mod),
                _ => None,
            };
            if let Some(op) = compound_op {
                let name = self.identifier()?;
                self.advance(); // consume the compound operator
                let value = self.expression()?;
                return Ok(Statement::CompoundAssignment(CompoundAssignmentStmt {
                    name,
                    op,
                    value,
                }));
            }
        }

        // Parse the expression (could be field access like self.field or index like arr[i])
        let expr = self.expression()?;

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
            let value = self.expression()?;
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
                        value,
                    }));
                }
                Expr::Index(object, index) => {
                    let value = self.expression()?;
                    return Ok(Statement::IndexAssignment(IndexAssignmentStmt {
                        object: *object,
                        index: *index,
                        value,
                    }));
                }
                _ => {
                    return Err(errors::invalid_assignment_target(expr.span));
                }
            }
        }

        // Check for compound assignment on field/index: expr.field += value, expr[i] -= value
        let compound_op = match &self.peek().kind {
            TokenKind::Operator(OperatorId::PlusEq) => Some(CompoundOp::Add),
            TokenKind::Operator(OperatorId::MinusEq) => Some(CompoundOp::Sub),
            TokenKind::Operator(OperatorId::StarEq) => Some(CompoundOp::Mul),
            TokenKind::Operator(OperatorId::SlashEq) => Some(CompoundOp::Div),
            TokenKind::Operator(OperatorId::SlashSlashEq) => Some(CompoundOp::FloorDiv),
            TokenKind::Operator(OperatorId::PercentEq) => Some(CompoundOp::Mod),
            _ => None,
        };
        if let Some(op) = compound_op {
            self.advance(); // consume the compound operator
            let rhs = self.expression()?;
            match expr.node {
                Expr::Field(object, field) => {
                    // Convert field += rhs to field = field + rhs
                    let field_expr = Spanned::new(Expr::Field(object.clone(), field.clone()), expr.span);
                    let bin_op = match op {
                        CompoundOp::Add => BinaryOp::Add,
                        CompoundOp::Sub => BinaryOp::Sub,
                        CompoundOp::Mul => BinaryOp::Mul,
                        CompoundOp::Div => BinaryOp::Div,
                        CompoundOp::FloorDiv => BinaryOp::FloorDiv,
                        CompoundOp::Mod => BinaryOp::Mod,
                    };
                    let new_value = Spanned::new(Expr::Binary(Box::new(field_expr), bin_op, Box::new(rhs)), expr.span);
                    return Ok(Statement::FieldAssignment(FieldAssignmentStmt {
                        target_span: expr.span,
                        object: *object,
                        field,
                        value: new_value,
                    }));
                }
                Expr::Index(object, index) => {
                    // Convert arr[i] += rhs to arr[i] = arr[i] + rhs
                    let index_expr = Spanned::new(Expr::Index(object.clone(), index.clone()), expr.span);
                    let bin_op = match op {
                        CompoundOp::Add => BinaryOp::Add,
                        CompoundOp::Sub => BinaryOp::Sub,
                        CompoundOp::Mul => BinaryOp::Mul,
                        CompoundOp::Div => BinaryOp::Div,
                        CompoundOp::FloorDiv => BinaryOp::FloorDiv,
                        CompoundOp::Mod => BinaryOp::Mod,
                    };
                    let new_value = Spanned::new(Expr::Binary(Box::new(index_expr), bin_op, Box::new(rhs)), expr.span);
                    return Ok(Statement::IndexAssignment(IndexAssignmentStmt {
                        object: *object,
                        index: *index,
                        value: new_value,
                    }));
                }
                Expr::Ident(name) => {
                    // Fallback: simple ident compound assignment
                    return Ok(Statement::CompoundAssignment(CompoundAssignmentStmt {
                        name,
                        op,
                        value: rhs,
                    }));
                }
                _ => {
                    return Err(errors::invalid_compound_assignment_target(expr.span));
                }
            }
        }

        // Otherwise it's an expression statement
        Ok(Statement::Expr(expr))
    }

}
