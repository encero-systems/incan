/// Literals, f-string parts, and span shifting for embedded expressions.
impl<'a> Parser<'a> {
    /// Parse a literal token at the current parser position, if one is present.
    fn try_literal(&mut self) -> Option<Literal> {
        match &self.peek().kind {
            TokenKind::Int(il) => {
                let il = il.clone();
                self.advance();
                Some(Literal::Int(il))
            }
            TokenKind::Float(fl) => {
                let fl = fl.clone();
                self.advance();
                Some(Literal::Float(fl))
            }
            TokenKind::Decimal(dl) => {
                let dl = dl.clone();
                self.advance();
                Some(Literal::Decimal(dl))
            }
            TokenKind::String(s) => {
                let s = s.clone();
                self.advance();
                Some(Literal::String(s))
            }
            TokenKind::Bytes(b) => {
                let b = b.clone();
                self.advance();
                Some(Literal::Bytes(b))
            }
            TokenKind::Keyword(KeywordId::True) => {
                self.advance();
                Some(Literal::Bool(true))
            }
            TokenKind::Keyword(KeywordId::False) => {
                self.advance();
                Some(Literal::Bool(false))
            }
            TokenKind::Keyword(KeywordId::None) => {
                self.advance();
                Some(Literal::None)
            }
            _ => None,
        }
    }

    /// Convert lexer f-string segments into parsed AST parts while preserving interpolation spans and format markers.
    fn convert_fstring_parts(&self, parts: &[LexFStringPart]) -> Vec<FStringPart> {
        parts
            .iter()
            .map(|p| match p {
                LexFStringPart::Literal(s) => FStringPart::Literal(s.clone()),
                LexFStringPart::Expr { text, offset } => {
                    let expr_span = Span::new(*offset, offset + text.len() + 2);
                    let (expr_text, format) = split_fstring_format(text);
                    let mut expr = self.parse_fstring_expr(expr_text);
                    self.shift_expr_spans(&mut expr, offset + 1);
                    FStringPart::Expr {
                        expr: Spanned::new(expr, expr_span),
                        format,
                    }
                }
            })
            .collect()
    }

    fn shift_spanned_expr(&self, expr: &mut Spanned<Expr>, offset: usize) {
        expr.span = Span::new(expr.span.start + offset, expr.span.end + offset);
        self.shift_expr_spans(&mut expr.node, offset);
    }

    /// Shift one parsed call argument, including an authored keyword label that is not part of the value span.
    fn shift_call_arg(&self, arg: &mut CallArg, offset: usize) {
        match arg {
            CallArg::Named(name, value) => {
                name.span = Span::new(name.span.start + offset, name.span.end + offset);
                self.shift_spanned_expr(value, offset);
            }
            CallArg::Positional(value) | CallArg::PositionalUnpack(value) | CallArg::KeywordUnpack(value) => {
                self.shift_spanned_expr(value, offset);
            }
        }
    }

    /// Shift a pattern parsed inside an interpolated expression, preserving constructor and named-field labels.
    fn shift_spanned_pattern(&self, pattern: &mut Spanned<Pattern>, offset: usize) {
        pattern.span = Span::new(pattern.span.start + offset, pattern.span.end + offset);
        match &mut pattern.node {
            Pattern::Constructor(name, args) => {
                name.span = Span::new(name.span.start + offset, name.span.end + offset);
                for arg in args {
                    match arg {
                        PatternArg::Positional(pattern) => self.shift_spanned_pattern(pattern, offset),
                        PatternArg::Named(name, pattern) => {
                            name.span = Span::new(name.span.start + offset, name.span.end + offset);
                            self.shift_spanned_pattern(pattern, offset);
                        }
                    }
                }
            }
            Pattern::Tuple(patterns) | Pattern::Or(patterns) => {
                for pattern in patterns {
                    self.shift_spanned_pattern(pattern, offset);
                }
            }
            Pattern::Group(inner) => self.shift_spanned_pattern(inner, offset),
            Pattern::Wildcard | Pattern::Binding(_) | Pattern::Literal(_) => {}
        }
    }

    /// Recursively shift expression spans parsed from an f-string interpolation back into outer-source coordinates.
    fn shift_expr_spans(&self, expr: &mut Expr, offset: usize) {
        match expr {
            Expr::Ident(_) | Expr::Literal(_) | Expr::SelfExpr | Expr::Yield(None) => {}
            Expr::Binary(left, _, right) => {
                self.shift_spanned_expr(left, offset);
                self.shift_spanned_expr(right, offset);
            }
            Expr::Unary(_, operand) | Expr::Try(operand) | Expr::Paren(operand) => {
                self.shift_spanned_expr(operand, offset);
            }
            Expr::Call(callee, _type_args, args) => {
                self.shift_spanned_expr(callee, offset);
                for arg in args {
                    self.shift_call_arg(arg, offset);
                }
            }
            Expr::Index(base, index) => {
                self.shift_spanned_expr(base, offset);
                self.shift_spanned_expr(index, offset);
            }
            Expr::Slice(base, slice) => {
                self.shift_spanned_expr(base, offset);
                self.shift_slice_spans(slice, offset);
            }
            Expr::Field(base, _) => {
                self.shift_spanned_expr(base, offset);
            }
            Expr::MethodCall(base, _, _type_args, args) => {
                self.shift_spanned_expr(base, offset);
                for arg in args {
                    self.shift_call_arg(arg, offset);
                }
            }
            Expr::Partial(partial) => self.shift_partial_spans(partial, offset),
            Expr::Match(subject, arms) => {
                self.shift_spanned_expr(subject, offset);
                for arm in arms {
                    self.shift_match_arm_spans(arm, offset);
                }
            }
            Expr::If(if_expr) => {
                self.shift_spanned_expr(&mut if_expr.condition, offset);
            }
            Expr::Loop(_) => {}
            Expr::Generator(generator) => {
                self.shift_spanned_expr(&mut generator.expr, offset);
                for clause in &mut generator.clauses {
                    self.shift_comprehension_clause(clause, offset);
                }
            }
            Expr::ListComp(comp) => self.shift_list_comprehension_spans(comp, offset),
            Expr::DictComp(comp) => self.shift_dict_comprehension_spans(comp, offset),
            Expr::Closure(params, body) => {
                for param in params {
                    param.span = Span::new(param.span.start + offset, param.span.end + offset);
                    param.node.ty.span = Span::new(param.node.ty.span.start + offset, param.node.ty.span.end + offset);
                    if let Some(default) = &mut param.node.default {
                        self.shift_spanned_expr(default, offset);
                    }
                }
                self.shift_spanned_expr(body, offset);
            }
            Expr::Tuple(elems) | Expr::Set(elems) => {
                for elem in elems {
                    self.shift_spanned_expr(elem, offset);
                }
            }
            Expr::List(entries) => {
                for entry in entries {
                    match entry {
                        ListEntry::Element(value) | ListEntry::Spread(value) => {
                            self.shift_spanned_expr(value, offset);
                        }
                    }
                }
            }
            Expr::Dict(entries) => {
                for entry in entries {
                    match entry {
                        DictEntry::Pair(key, value) => {
                            self.shift_spanned_expr(key, offset);
                            self.shift_spanned_expr(value, offset);
                        }
                        DictEntry::Spread(value) => self.shift_spanned_expr(value, offset),
                    }
                }
            }
            Expr::Constructor(_, args) => {
                for arg in args {
                    self.shift_call_arg(arg, offset);
                }
            }
            Expr::FString(parts) => {
                for part in parts {
                    if let FStringPart::Expr { expr, .. } = part {
                        self.shift_spanned_expr(expr, offset);
                    }
                }
            }
            Expr::Yield(Some(value)) => {
                self.shift_spanned_expr(value, offset);
            }
            Expr::Range {
                start,
                end,
                inclusive: _,
            } => {
                self.shift_spanned_expr(start, offset);
                self.shift_spanned_expr(end, offset);
            }
            Expr::Surface(surface_expr) => self.shift_surface_expr_spans(&mut surface_expr.payload, offset),
            Expr::VocabBlock(_) => {}
            Expr::Embedded(fragment) => {
                for node in &mut fragment.nodes {
                    self.shift_embedded_node_spans(node, offset);
                }
            }
        }
    }

    /// Rebase the optional bounds of a slice parsed inside an interpolation.
    fn shift_slice_spans(&self, slice: &mut SliceExpr, offset: usize) {
        if let Some(start) = &mut slice.start {
            self.shift_spanned_expr(start, offset);
        }
        if let Some(end) = &mut slice.end {
            self.shift_spanned_expr(end, offset);
        }
        if let Some(step) = &mut slice.step {
            self.shift_spanned_expr(step, offset);
        }
    }

    /// Rebase a partial callable's target, explicit type-argument spans and preset values.
    fn shift_partial_spans(&self, partial: &mut PartialExpr, offset: usize) {
        self.shift_spanned_expr(&mut partial.target, offset);
        for arg in &mut partial.type_args {
            arg.span = Span::new(arg.span.start + offset, arg.span.end + offset);
        }
        for arg in &mut partial.args {
            self.shift_spanned_expr(&mut arg.value, offset);
        }
    }

    /// Rebase a match arm's pattern, guard and expression body; block bodies retain their existing coordinates.
    fn shift_match_arm_spans(&self, arm: &mut Spanned<MatchArm>, offset: usize) {
        arm.span = Span::new(arm.span.start + offset, arm.span.end + offset);
        self.shift_spanned_pattern(&mut arm.node.pattern, offset);
        if let Some(guard) = &mut arm.node.guard {
            self.shift_spanned_expr(guard, offset);
        }
        match &mut arm.node.body {
            MatchBody::Expr(value) => self.shift_spanned_expr(value, offset),
            MatchBody::Block(_) => {}
        }
    }

    /// Rebase the expressions and clauses carried by a list comprehension.
    fn shift_list_comprehension_spans(&self, comp: &mut ListComp, offset: usize) {
        self.shift_spanned_expr(&mut comp.expr, offset);
        self.shift_spanned_expr(&mut comp.iter, offset);
        if let Some(filter) = &mut comp.filter {
            self.shift_spanned_expr(filter, offset);
        }
        for clause in &mut comp.clauses {
            self.shift_comprehension_clause(clause, offset);
        }
    }

    /// Rebase the key, value, iterator, filter and clauses carried by a dictionary comprehension.
    fn shift_dict_comprehension_spans(&self, comp: &mut DictComp, offset: usize) {
        self.shift_spanned_expr(&mut comp.key, offset);
        self.shift_spanned_expr(&mut comp.value, offset);
        self.shift_spanned_expr(&mut comp.iter, offset);
        if let Some(filter) = &mut comp.filter {
            self.shift_spanned_expr(filter, offset);
        }
        for clause in &mut comp.clauses {
            self.shift_comprehension_clause(clause, offset);
        }
    }

    /// Rebase the expression spans owned by a surface payload without changing its feature identity.
    fn shift_surface_expr_spans(&self, payload: &mut SurfaceExprPayload, offset: usize) {
        match payload {
            SurfaceExprPayload::PrefixUnary(value) => self.shift_spanned_expr(value, offset),
            SurfaceExprPayload::RaceFor(race) => {
                race.binding.span = Span::new(race.binding.span.start + offset, race.binding.span.end + offset);
                for arm in &mut race.arms {
                    self.shift_spanned_expr(&mut arm.awaitable, offset);
                    if let RaceForBody::Expr(value) = &mut arm.body {
                        self.shift_spanned_expr(value, offset);
                    }
                }
            }
            SurfaceExprPayload::LeadingDotPath { .. } => {}
            SurfaceExprPayload::ScopedGlyph { left, right, .. } => {
                self.shift_spanned_expr(left, offset);
                self.shift_spanned_expr(right, offset);
            }
            SurfaceExprPayload::ScopedSymbolCall { args, .. } => {
                for arg in args {
                    self.shift_call_arg(arg, offset);
                }
            }
        }
    }

    /// Recursively shift the spans of one embedded-fragment node (and everything nested inside it) by `offset`.
    ///
    /// This mirrors [`Parser::shift_expr_spans`] for the one case an embedded fragment can appear inside a
    /// re-entrant sub-parse whose spans still need rebasing into outer-source coordinates (an expression hole
    /// re-entering `self.expression()` from within an f-string interpolation). The submode tokenizer normally
    /// tracks absolute source offsets directly (see `parser/embedded`), so this path is defensive rather than the
    /// common case.
    fn shift_embedded_node_spans(&self, node: &mut Spanned<EmbeddedNode>, offset: usize) {
        node.span = Span::new(node.span.start + offset, node.span.end + offset);
        match &mut node.node {
            EmbeddedNode::Text(_)
            | EmbeddedNode::EntityRef(_)
            | EmbeddedNode::Comment(_)
            | EmbeddedNode::Value(_)
            | EmbeddedNode::Regex { .. }
            | EmbeddedNode::TypeShape(_) => {}
            EmbeddedNode::Hole(expr) => self.shift_spanned_expr(expr, offset),
            EmbeddedNode::Element(element) => {
                for attr in &mut element.attrs {
                    if let Some(value) = &mut attr.value {
                        self.shift_embedded_node_spans(value, offset);
                    }
                }
                for child in &mut element.children {
                    self.shift_embedded_node_spans(child, offset);
                }
            }
            EmbeddedNode::StyleRule(rule) => {
                for selector in &mut rule.selectors {
                    self.shift_embedded_node_spans(selector, offset);
                }
                for declaration in &mut rule.declarations {
                    self.shift_embedded_node_spans(declaration, offset);
                }
            }
            EmbeddedNode::Declaration(declaration) => {
                for value in &mut declaration.value {
                    self.shift_embedded_node_spans(value, offset);
                }
            }
        }
    }

    /// Shift expression spans that live inside a comprehension clause.
    fn shift_comprehension_clause(&self, clause: &mut ComprehensionClause, offset: usize) {
        match clause {
            ComprehensionClause::For { iter, .. } => self.shift_spanned_expr(iter, offset),
            ComprehensionClause::If(condition) => self.shift_spanned_expr(condition, offset),
        }
    }

    fn parse_fstring_expr(&self, s: &str) -> Expr {
        // Properly parse the expression string by lexing and parsing it
        use crate::lexer;

        // Try to lex and parse the expression
        if let Ok(mut tokens) = lexer::lex(s) {
            // Ensure we have an EOF token at the end for the parser
            if tokens.is_empty() || !matches!(tokens.last().map(|t| &t.kind), Some(TokenKind::Eof)) {
                tokens.push(Token {
                    kind: TokenKind::Eof,
                    span: Span::default(),
                });
            }

            if tokens.len() > 1 {
                // At least one real token plus EOF
                let mut parser = Parser::new(&tokens);
                if let Ok(expr) = parser.expression() {
                    return expr.node;
                }
            }
        }

        // Fallback: treat as simple identifier
        Expr::Ident(s.to_string())
    }
}
