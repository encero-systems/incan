//! Expression formatting by expression kind.

use super::*;

impl Formatter {
    /// Format one expression node, preserving call/collection entry structure and surface-expression payload syntax.
    pub(in crate::formatter) fn format_expr(&mut self, expr: &Expr) {
        // Each expression consumes the tail flag. Only a part that nothing follows on the statement's line inherits
        // it: a binary right operand, a unary operand and a postfix base, since postfix syntax continues a chain.
        let tail = std::mem::take(&mut self.fluent_tail);
        match expr {
            Expr::Ident(name) => self.writer.write(name),
            Expr::Literal(lit) => self.format_literal(lit),
            Expr::SelfExpr => self.writer.write("self"),
            Expr::Binary(left, op, right) => {
                self.format_spanned_expr(left);
                self.writer.write(" ");
                self.format_binary_op(op);
                self.writer.write(" ");
                self.fluent_tail = tail;
                self.format_spanned_expr(right);
            }
            Expr::Unary(op, operand) => {
                self.format_unary_op(op);
                self.fluent_tail = tail;
                self.format_spanned_expr(operand);
            }
            Expr::Call(callee, type_args, args) => {
                self.fluent_tail = tail;
                self.format_call_expression(callee, type_args, args);
            }
            Expr::Index(base, index) => {
                self.fluent_tail = tail;
                self.format_spanned_expr(base);
                self.writer.write("[");
                self.in_brackets(|formatter| formatter.format_spanned_expr(index));
                self.writer.write("]");
            }
            Expr::Slice(base, slice) => {
                self.fluent_tail = tail;
                self.format_slice_expression(base, slice);
            }
            Expr::Field(base, field) => {
                self.fluent_tail = tail;
                self.format_spanned_expr(base);
                self.writer.write(".");
                self.writer.write(field);
            }
            Expr::MethodCall(receiver, method, type_args, args) => {
                if !self.format_fluent_method_chain_if_needed(expr, tail) {
                    let segment = MethodChainSegment {
                        method,
                        type_args,
                        args,
                    };
                    self.fluent_tail = tail;
                    self.format_spanned_expr(receiver);
                    self.format_method_chain_segment(&segment);
                }
            }
            Expr::Partial(partial) => self.format_partial_expression(partial),
            Expr::Surface(surface_expr) => self.format_surface_expression(surface_expr),
            Expr::VocabBlock(block) => self.format_expression_vocab_block_braced(block),
            Expr::Embedded(fragment) => self.format_embedded_fragment(fragment),
            Expr::Try(inner) => {
                self.fluent_tail = tail;
                self.format_spanned_expr(inner);
                self.writer.write("?");
            }
            Expr::Match(value, arms) => self.format_match_expression(value, arms),
            Expr::If(if_expr) => self.format_if_expression(if_expr),
            Expr::Loop(loop_expr) => self.format_loop_expression(loop_expr),
            Expr::Generator(generator) => self.format_generator_expression(generator),
            Expr::Closure(params, body) => {
                self.writer.write("(");
                self.format_closure_params(params);
                self.writer.write(") => ");
                self.format_spanned_expr(body);
            }
            Expr::Tuple(items) => self.format_tuple_expression(items),
            Expr::List(items) => self.format_list(items),
            Expr::Dict(pairs) => self.format_dict_expression(pairs),
            Expr::Set(items) => self.format_set_expression(items),
            Expr::Paren(inner) => {
                if Self::is_logical_binary_chain(&inner.node) {
                    self.format_parenthesized_logical_chain(inner);
                } else {
                    self.format_parenthesized_expr(inner);
                }
            }
            Expr::Constructor(name, args) => {
                self.writer.write(name);
                self.writer.write("(");
                self.format_call_args_with_wrapping(args);
                self.writer.write(")");
            }
            Expr::FString(parts) => self.format_fstring_expression(parts),
            Expr::ListComp(comp) => self.format_list_comprehension(comp),
            Expr::DictComp(comp) => self.format_dict_comprehension(comp),
            Expr::Yield(inner) => {
                self.writer.write("yield");
                if let Some(inner) = inner {
                    self.writer.write(" ");
                    self.format_spanned_expr(inner);
                }
            }
            Expr::Range { start, end, inclusive } => {
                self.format_spanned_expr(start);
                if *inclusive {
                    self.writer.write("..=");
                } else {
                    self.writer.write("..");
                }
                self.format_spanned_expr(end);
            }
        }
    }

    /// Render a call with its explicit type arguments and wrapped value arguments.
    pub(super) fn format_call_expression(
        &mut self,
        callee: &Spanned<Expr>,
        type_args: &[Spanned<Type>],
        args: &[CallArg],
    ) {
        self.format_spanned_expr(callee);
        self.format_call_type_args(type_args);
        self.writer.write("(");
        self.format_call_args_with_wrapping(args);
        self.writer.write(")");
    }

    /// Render an explicit type argument list, omitting brackets when no arguments were authored.
    pub(super) fn format_call_type_args(&mut self, type_args: &[Spanned<Type>]) {
        if type_args.is_empty() {
            return;
        }
        self.writer.write("[");
        for (index, arg) in type_args.iter().enumerate() {
            if index > 0 {
                self.writer.write(", ");
            }
            self.format_type(&arg.node);
        }
        self.writer.write("]");
    }

    /// Render a slice, retaining omitted bounds and the presence of an explicit step.
    pub(super) fn format_slice_expression(&mut self, base: &Spanned<Expr>, slice: &SliceExpr) {
        self.format_spanned_expr(base);
        self.writer.write("[");
        self.in_brackets(|formatter| {
            if let Some(start) = &slice.start {
                formatter.format_spanned_expr(start);
            }
            formatter.writer.write(":");
            if let Some(end) = &slice.end {
                formatter.format_spanned_expr(end);
            }
            if let Some(step) = &slice.step {
                formatter.writer.write(":");
                formatter.format_spanned_expr(step);
            }
        });
        self.writer.write("]");
    }

    /// Render a partial callable's target, type arguments and keyword presets.
    pub(super) fn format_partial_expression(&mut self, partial: &PartialExpr) {
        self.writer.write("partial ");
        self.format_spanned_expr(&partial.target);
        self.format_call_type_args(&partial.type_args);
        self.writer.write("(");
        self.format_partial_args(&partial.args);
        self.writer.write(")");
    }

    /// Render the syntax selected by a surface feature and its parsed payload.
    pub(super) fn format_surface_expression(&mut self, surface: &SurfaceExpr) {
        match (&surface.key, &surface.payload) {
            (SurfaceFeatureKey::SoftKeyword(id), SurfaceExprPayload::PrefixUnary(inner)) => {
                self.writer.write(keywords::as_str(*id));
                self.writer.write(" ");
                self.format_spanned_expr(inner);
            }
            (_, SurfaceExprPayload::RaceFor(race)) => self.format_race_for_expr(race),
            (SurfaceFeatureKey::ScopedDslSurface { .. }, SurfaceExprPayload::LeadingDotPath { segments, .. }) => {
                for segment in segments {
                    self.writer.write(".");
                    self.writer.write(segment);
                }
            }
            (
                SurfaceFeatureKey::ScopedDslSurface { .. },
                SurfaceExprPayload::ScopedGlyph { glyph, left, right, .. },
            ) => {
                self.format_spanned_expr(left);
                self.writer.write(" ");
                self.writer.write(glyph);
                self.writer.write(" ");
                self.format_spanned_expr(right);
            }
            (SurfaceFeatureKey::ScopedDslSurface { .. }, SurfaceExprPayload::ScopedSymbolCall { symbol, args, .. }) => {
                self.writer.write(symbol);
                self.writer.write("(");
                self.format_call_args_with_wrapping(args);
                self.writer.write(")");
            }
            _ => self.writer.write("<surface_expr>"),
        }
    }

    /// Render a match expression and its indented arms.
    pub(super) fn format_match_expression(&mut self, value: &Spanned<Expr>, arms: &[Spanned<MatchArm>]) {
        self.writer.write("match ");
        self.format_suite_header_expr(value);
        self.writer.writeln(":");
        self.writer.indent();
        for arm in arms {
            self.format_match_arm(arm);
        }
        self.writer.dedent();
    }

    /// Render a loop expression, supplying `pass` for an empty suite.
    pub(super) fn format_loop_expression(&mut self, loop_expr: &LoopExpr) {
        self.writer.writeln("loop:");
        self.writer.indent();
        for stmt in &loop_expr.body {
            self.format_statement(stmt);
        }
        if loop_expr.body.is_empty() {
            self.writer.writeln("pass");
        }
        self.writer.dedent();
    }

    /// Render a parenthesized generator expression and its clauses.
    pub(super) fn format_generator_expression(&mut self, generator: &GeneratorExpr) {
        self.writer.write("(");
        self.in_brackets(|formatter| {
            formatter.format_spanned_expr(&generator.expr);
            formatter.format_comprehension_clauses(&generator.clauses);
        });
        self.writer.write(")");
    }

    /// Render a tuple display, keeping the comma that distinguishes a singleton tuple.
    pub(super) fn format_tuple_expression(&mut self, items: &[Spanned<Expr>]) {
        self.writer.write("(");
        self.in_brackets(|formatter| {
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    formatter.writer.write(", ");
                }
                formatter.format_spanned_expr(item);
            }
        });
        if items.len() == 1 {
            self.writer.write(",");
        }
        self.writer.write(")");
    }

    /// Render dictionary entries in their authored order, retaining spread prefixes.
    pub(super) fn format_dict_expression(&mut self, pairs: &[DictEntry]) {
        self.writer.write("{");
        self.in_brackets(|formatter| {
            for (index, entry) in pairs.iter().enumerate() {
                if index > 0 {
                    formatter.writer.write(", ");
                }
                match entry {
                    DictEntry::Pair(key, value) => {
                        formatter.format_spanned_expr(key);
                        formatter.writer.write(": ");
                        formatter.format_spanned_expr(value);
                    }
                    DictEntry::Spread(value) => {
                        formatter.writer.write("**");
                        formatter.format_spanned_expr(value);
                    }
                }
            }
        });
        self.writer.write("}");
    }

    /// Render a brace-delimited set display.
    pub(super) fn format_set_expression(&mut self, items: &[Spanned<Expr>]) {
        self.writer.write("{");
        self.in_brackets(|formatter| {
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    formatter.writer.write(", ");
                }
                formatter.format_spanned_expr(item);
            }
        });
        self.writer.write("}");
    }

    /// Render an f-string, escaping literal parts and retaining each interpolation's format marker.
    pub(super) fn format_fstring_expression(&mut self, parts: &[FStringPart]) {
        self.writer.write("f\"");
        for part in parts {
            match part {
                FStringPart::Literal(text) => self.writer.write(&escape_fstring_literal(text)),
                FStringPart::Expr { expr, format } => {
                    self.writer.write("{");
                    self.in_brackets(|formatter| formatter.format_spanned_expr(expr));
                    match format {
                        FStringFormat::Display => {}
                        FStringFormat::Debug => self.writer.write(":?"),
                        FStringFormat::Unsupported(spec) => {
                            self.writer.write(":");
                            self.writer.write(spec);
                        }
                    }
                    self.writer.write("}");
                }
            }
        }
        self.writer.write("\"");
    }

    /// Render a list comprehension and its clauses.
    pub(super) fn format_list_comprehension(&mut self, comp: &ListComp) {
        self.writer.write("[");
        self.in_brackets(|formatter| {
            formatter.format_spanned_expr(&comp.expr);
            formatter.format_comprehension_clauses(&comp.clauses);
        });
        self.writer.write("]");
    }

    /// Render a dictionary comprehension and its clauses.
    pub(super) fn format_dict_comprehension(&mut self, comp: &DictComp) {
        self.writer.write("{");
        self.in_brackets(|formatter| {
            formatter.format_spanned_expr(&comp.key);
            formatter.writer.write(": ");
            formatter.format_spanned_expr(&comp.value);
            formatter.format_comprehension_clauses(&comp.clauses);
        });
        self.writer.write("}");
    }

    /// Format an import-activated `race for value:` expression block.
    pub(super) fn format_race_for_expr(&mut self, race: &RaceForExpr) {
        self.writer.write("race for ");
        self.writer.write(&race.binding.node);
        self.writer.writeln(":");
        self.writer.indent();
        for arm in &race.arms {
            self.writer.write("await ");
            self.format_spanned_expr(&arm.awaitable);
            self.writer.write(" =>");
            match &arm.body {
                RaceForBody::Expr(expr) => {
                    self.writer.write(" ");
                    self.format_spanned_expr(expr);
                    self.writer.newline();
                }
                RaceForBody::Block(stmts) => {
                    self.writer.newline();
                    self.writer.indent();
                    for stmt in stmts {
                        self.format_statement(stmt);
                    }
                    if stmts.is_empty() {
                        self.writer.writeln("pass");
                    }
                    self.writer.dedent();
                }
            }
        }
        self.writer.dedent();
    }

    /// Write comprehension clauses in canonical source order.
    pub(super) fn format_comprehension_clauses(&mut self, clauses: &[ComprehensionClause]) {
        for clause in clauses {
            match clause {
                ComprehensionClause::For { pattern, iter } => {
                    self.writer.write(" for ");
                    self.format_for_pattern(&pattern.node);
                    self.writer.write(" in ");
                    self.format_spanned_expr(iter);
                }
                ComprehensionClause::If(condition) => {
                    self.writer.write(" if ");
                    self.format_spanned_expr(condition);
                }
            }
        }
    }
}
