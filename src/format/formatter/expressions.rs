//! Expression formatting: expressions, literals, operators, patterns, match arms, and types.

use crate::frontend::ast::*;
use incan_core::lang::keywords;
use incan_semantics_core::SurfaceFeatureKey;

use super::Formatter;

impl Formatter {
    /// Return whether a binary operator is a logical chain breakpoint.
    fn is_logical_binary_op(op: &BinaryOp) -> bool {
        matches!(op, BinaryOp::And | BinaryOp::Or)
    }

    /// Return whether an expression starts a logical binary chain.
    fn is_logical_binary_chain(expr: &Expr) -> bool {
        matches!(expr, Expr::Binary(_, op, _) if Self::is_logical_binary_op(op))
    }

    fn write_call_arg(&mut self, arg: &CallArg) {
        match arg {
            CallArg::Positional(expr) => self.format_expr(&expr.node),
            CallArg::Named(name, expr) => {
                self.writer.write(name);
                self.writer.write("=");
                self.format_expr(&expr.node);
            }
            CallArg::PositionalUnpack(expr) => {
                self.writer.write("*");
                self.format_expr(&expr.node);
            }
            CallArg::KeywordUnpack(expr) => {
                self.writer.write("**");
                self.format_expr(&expr.node);
            }
        }
    }

    /// Format one keyword preset in a partial callable template.
    fn write_partial_arg(&mut self, arg: &PartialArg) {
        self.writer.write(&arg.name);
        self.writer.write("=");
        self.format_expr(&arg.value.node);
    }

    /// Format the comma-separated keyword preset list in a partial callable template.
    pub(super) fn format_partial_args(&mut self, args: &[PartialArg]) {
        for (i, arg) in args.iter().enumerate() {
            if i > 0 {
                self.writer.write(", ");
            }
            self.write_partial_arg(arg);
        }
    }

    fn format_call_args_with_wrapping(&mut self, args: &[CallArg]) {
        if args.is_empty() {
            return;
        }

        let checkpoint = self.writer.checkpoint();
        self.format_call_args(args);
        if !(self.writer.output_since_contains_newline(checkpoint) || self.writer.line_length_exceeded()) {
            return;
        }
        self.writer.restore(checkpoint);

        let trailing_commas = self.writer.config().trailing_commas;
        self.writer.newline();
        self.writer.indent();
        for (i, arg) in args.iter().enumerate() {
            self.write_call_arg(arg);
            if trailing_commas || i + 1 < args.len() {
                self.writer.write(",");
            }
            self.writer.newline();
        }
        self.writer.dedent();
    }

    /// Format a parenthesized logical chain inline first, then wrap it if the line overflows.
    fn format_parenthesized_logical_chain(&mut self, inner: &Expr) {
        let checkpoint = self.writer.checkpoint();
        self.writer.write("(");
        self.format_expr(inner);
        self.writer.write(")");
        if self.writer.output_since_contains_newline(checkpoint) || !self.writer.line_length_exceeded() {
            return;
        }

        self.writer.restore(checkpoint);
        self.writer.write("(");
        self.writer.newline();
        self.writer.indent();
        let mut wrote_line = false;
        self.format_logical_chain_lines(inner, None, &mut wrote_line);
        self.writer.newline();
        self.writer.dedent();
        self.writer.write(")");
    }

    /// Emit one logical-chain operand per line with leading `and` / `or` operators after the first operand.
    fn format_logical_chain_lines(&mut self, expr: &Expr, leading_op: Option<&BinaryOp>, wrote_line: &mut bool) {
        match expr {
            Expr::Binary(left, op, right) if Self::is_logical_binary_op(op) => {
                self.format_logical_chain_lines(&left.node, leading_op, wrote_line);
                self.format_logical_chain_lines(&right.node, Some(op), wrote_line);
            }
            _ => {
                if *wrote_line {
                    self.writer.newline();
                }
                if let Some(op) = leading_op {
                    self.format_binary_op(op);
                    self.writer.write(" ");
                }
                self.format_expr(expr);
                *wrote_line = true;
            }
        }
    }

    /// Format one expression node, preserving call/collection entry structure and surface-expression payload syntax.
    pub(super) fn format_expr(&mut self, expr: &Expr) {
        match expr {
            Expr::Ident(name) => self.writer.write(name),
            Expr::Literal(lit) => self.format_literal(lit),
            Expr::SelfExpr => self.writer.write("self"),
            Expr::Binary(left, op, right) => {
                self.format_expr(&left.node);
                self.writer.write(" ");
                self.format_binary_op(op);
                self.writer.write(" ");
                self.format_expr(&right.node);
            }
            Expr::Unary(op, operand) => {
                self.format_unary_op(op);
                self.format_expr(&operand.node);
            }
            Expr::Call(callee, type_args, args) => {
                self.format_expr(&callee.node);
                if !type_args.is_empty() {
                    self.writer.write("[");
                    for (i, arg) in type_args.iter().enumerate() {
                        if i > 0 {
                            self.writer.write(", ");
                        }
                        self.format_type(&arg.node);
                    }
                    self.writer.write("]");
                }
                self.writer.write("(");
                self.format_call_args_with_wrapping(args);
                self.writer.write(")");
            }
            Expr::Index(base, index) => {
                self.format_expr(&base.node);
                self.writer.write("[");
                self.format_expr(&index.node);
                self.writer.write("]");
            }
            Expr::Slice(base, slice) => {
                self.format_expr(&base.node);
                self.writer.write("[");
                if let Some(start) = &slice.start {
                    self.format_expr(&start.node);
                }
                self.writer.write(":");
                if let Some(end) = &slice.end {
                    self.format_expr(&end.node);
                }
                if let Some(step) = &slice.step {
                    self.writer.write(":");
                    self.format_expr(&step.node);
                }
                self.writer.write("]");
            }
            Expr::Field(base, field) => {
                self.format_expr(&base.node);
                self.writer.write(".");
                self.writer.write(field);
            }
            Expr::MethodCall(receiver, method, type_args, args) => {
                self.format_expr(&receiver.node);
                self.writer.write(".");
                self.writer.write(method);
                if !type_args.is_empty() {
                    self.writer.write("[");
                    for (i, arg) in type_args.iter().enumerate() {
                        if i > 0 {
                            self.writer.write(", ");
                        }
                        self.format_type(&arg.node);
                    }
                    self.writer.write("]");
                }
                self.writer.write("(");
                self.format_call_args_with_wrapping(args);
                self.writer.write(")");
            }
            Expr::Partial(partial) => {
                self.writer.write("partial ");
                self.format_expr(&partial.target.node);
                if !partial.type_args.is_empty() {
                    self.writer.write("[");
                    for (i, arg) in partial.type_args.iter().enumerate() {
                        if i > 0 {
                            self.writer.write(", ");
                        }
                        self.format_type(&arg.node);
                    }
                    self.writer.write("]");
                }
                self.writer.write("(");
                self.format_partial_args(&partial.args);
                self.writer.write(")");
            }
            Expr::Surface(surface_expr) => match (&surface_expr.key, &surface_expr.payload) {
                (SurfaceFeatureKey::SoftKeyword(id), SurfaceExprPayload::PrefixUnary(inner)) => {
                    self.writer.write(keywords::as_str(*id));
                    self.writer.write(" ");
                    self.format_expr(&inner.node);
                }
                (_, SurfaceExprPayload::RaceFor(race)) => {
                    self.format_race_for_expr(race);
                }
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
                    self.format_expr(&left.node);
                    self.writer.write(" ");
                    self.writer.write(glyph);
                    self.writer.write(" ");
                    self.format_expr(&right.node);
                }
                (
                    SurfaceFeatureKey::ScopedDslSurface { .. },
                    SurfaceExprPayload::ScopedSymbolCall { symbol, args, .. },
                ) => {
                    self.writer.write(symbol);
                    self.writer.write("(");
                    self.format_call_args_with_wrapping(args);
                    self.writer.write(")");
                }
                _ => self.writer.write("<surface_expr>"),
            },
            Expr::Try(inner) => {
                self.format_expr(&inner.node);
                self.writer.write("?");
            }
            Expr::Match(value, arms) => {
                self.writer.write("match ");
                self.format_expr(&value.node);
                self.writer.writeln(":");
                self.writer.indent();
                for arm in arms {
                    self.format_match_arm(arm);
                }
                self.writer.dedent();
            }
            Expr::If(if_expr) => {
                self.format_expr(&if_expr.condition.node);
                self.writer.write(" if ");
                // Note: This handles ternary-style if expressions
            }
            Expr::Loop(loop_expr) => {
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
            Expr::Generator(generator) => {
                self.writer.write("(");
                self.format_expr(&generator.expr.node);
                self.format_comprehension_clauses(&generator.clauses);
                self.writer.write(")");
            }
            Expr::Closure(params, body) => {
                self.writer.write("(");
                self.format_params(params);
                self.writer.write(") => ");
                self.format_expr(&body.node);
            }
            Expr::Tuple(items) => {
                self.writer.write("(");
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        self.writer.write(", ");
                    }
                    self.format_expr(&item.node);
                }
                if items.len() == 1 {
                    self.writer.write(",");
                }
                self.writer.write(")");
            }
            Expr::List(items) => {
                self.writer.write("[");
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        self.writer.write(", ");
                    }
                    match item {
                        ListEntry::Element(value) => self.format_expr(&value.node),
                        ListEntry::Spread(value) => {
                            self.writer.write("*");
                            self.format_expr(&value.node);
                        }
                    }
                }
                self.writer.write("]");
            }
            Expr::Dict(pairs) => {
                self.writer.write("{");
                for (i, entry) in pairs.iter().enumerate() {
                    if i > 0 {
                        self.writer.write(", ");
                    }
                    match entry {
                        DictEntry::Pair(k, v) => {
                            self.format_expr(&k.node);
                            self.writer.write(": ");
                            self.format_expr(&v.node);
                        }
                        DictEntry::Spread(value) => {
                            self.writer.write("**");
                            self.format_expr(&value.node);
                        }
                    }
                }
                self.writer.write("}");
            }
            Expr::Set(items) => {
                self.writer.write("{");
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        self.writer.write(", ");
                    }
                    self.format_expr(&item.node);
                }
                self.writer.write("}");
            }
            Expr::Paren(inner) => {
                if Self::is_logical_binary_chain(&inner.node) {
                    self.format_parenthesized_logical_chain(&inner.node);
                } else {
                    self.writer.write("(");
                    self.format_expr(&inner.node);
                    self.writer.write(")");
                }
            }
            Expr::Constructor(name, args) => {
                self.writer.write(name);
                self.writer.write("(");
                self.format_call_args_with_wrapping(args);
                self.writer.write(")");
            }
            Expr::FString(parts) => {
                self.writer.write("f\"");
                for part in parts {
                    match part {
                        FStringPart::Literal(s) => self.writer.write(&escape_fstring_literal(s)),
                        FStringPart::Expr(expr) => {
                            self.writer.write("{");
                            self.format_expr(&expr.node);
                            self.writer.write("}");
                        }
                    }
                }
                self.writer.write("\"");
            }
            Expr::ListComp(comp) => {
                self.writer.write("[");
                self.format_expr(&comp.expr.node);
                self.format_comprehension_clauses(&comp.clauses);
                self.writer.write("]");
            }
            Expr::DictComp(comp) => {
                self.writer.write("{");
                self.format_expr(&comp.key.node);
                self.writer.write(": ");
                self.format_expr(&comp.value.node);
                self.format_comprehension_clauses(&comp.clauses);
                self.writer.write("}");
            }
            Expr::Yield(inner) => {
                self.writer.write("yield");
                if let Some(inner) = inner {
                    self.writer.write(" ");
                    self.format_expr(&inner.node);
                }
            }
            Expr::Range { start, end, inclusive } => {
                self.format_expr(&start.node);
                if *inclusive {
                    self.writer.write("..=");
                } else {
                    self.writer.write("..");
                }
                self.format_expr(&end.node);
            }
        }
    }

    /// Format an import-activated `race for value:` expression block.
    fn format_race_for_expr(&mut self, race: &RaceForExpr) {
        self.writer.write("race for ");
        self.writer.write(&race.binding);
        self.writer.writeln(":");
        self.writer.indent();
        for arm in &race.arms {
            self.writer.write("await ");
            self.format_expr(&arm.awaitable.node);
            self.writer.write(" =>");
            match &arm.body {
                RaceForBody::Expr(expr) => {
                    self.writer.write(" ");
                    self.format_expr(&expr.node);
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
    fn format_comprehension_clauses(&mut self, clauses: &[ComprehensionClause]) {
        for clause in clauses {
            match clause {
                ComprehensionClause::For { pattern, iter } => {
                    self.writer.write(" for ");
                    self.format_for_pattern(&pattern.node);
                    self.writer.write(" in ");
                    self.format_expr(&iter.node);
                }
                ComprehensionClause::If(condition) => {
                    self.writer.write(" if ");
                    self.format_expr(&condition.node);
                }
            }
        }
    }

    // ---- Literals ----

    /// Format a literal while preserving source-sensitive numeric spellings.
    fn format_literal(&mut self, lit: &Literal) {
        match lit {
            Literal::Int(il) => self.writer.write(&il.repr),
            // Emit source `FloatLiteral::repr`, not `f64` `Display` (which drops `.0`, etc.).
            Literal::Float(fl) => self.writer.write(&fl.repr),
            Literal::Decimal(dl) => self.writer.write(&dl.repr),
            Literal::String(s) => {
                self.writer.write("\"");
                self.writer.write(&escape_string(s));
                self.writer.write("\"");
            }
            Literal::Bytes(b) => {
                self.writer.write("b\"");
                for byte in b {
                    if *byte >= 32 && *byte < 127 {
                        self.writer.write(&(*byte as char).to_string());
                    } else {
                        self.writer.write(&format!("\\x{:02x}", byte));
                    }
                }
                self.writer.write("\"");
            }
            Literal::Bool(b) => self.writer.write(if *b { "true" } else { "false" }),
            Literal::None => self.writer.write("None"),
        }
    }

    // ---- Operators ----

    /// Write a binary operator using the formatter's canonical spelling.
    fn format_binary_op(&mut self, op: &BinaryOp) {
        self.writer.write(match op {
            BinaryOp::Add => "+",
            BinaryOp::Sub => "-",
            BinaryOp::Mul => "*",
            BinaryOp::Div => "/",
            BinaryOp::FloorDiv => "//",
            BinaryOp::Mod => "%",
            BinaryOp::Pow => "**",
            BinaryOp::MatMul => "@",
            BinaryOp::PipeForward => "|>",
            BinaryOp::PipeBackward => "<|",
            BinaryOp::BitAnd => "&",
            BinaryOp::BitOr => "|",
            BinaryOp::BitXor => "^",
            BinaryOp::Shl => "<<",
            BinaryOp::Shr => ">>",
            BinaryOp::Eq => "==",
            BinaryOp::NotEq => "!=",
            BinaryOp::Lt => "<",
            BinaryOp::Gt => ">",
            BinaryOp::LtEq => "<=",
            BinaryOp::GtEq => ">=",
            BinaryOp::And => "and",
            BinaryOp::Or => "or",
            BinaryOp::In => "in",
            BinaryOp::NotIn => "not in",
            BinaryOp::Is => "is",
            BinaryOp::IsNot => "is not",
        });
    }

    /// Write the source spelling for a unary operator.
    fn format_unary_op(&mut self, op: &UnaryOp) {
        self.writer.write(match op {
            UnaryOp::Neg => "-",
            UnaryOp::Not => "not ",
            UnaryOp::Invert => "~",
        });
    }

    // ---- Call args ----

    fn format_call_args(&mut self, args: &[CallArg]) {
        for (i, arg) in args.iter().enumerate() {
            if i > 0 {
                self.writer.write(", ");
            }
            self.write_call_arg(arg);
        }
    }

    // ---- Match arms and patterns ----

    /// Writes a constructor pattern's type/variant path for Incan surface syntax.
    ///
    /// The parser stores qualified patterns as `Type::Variant` for lowering to Rust; match patterns in source use `.`
    /// between segments. Emitting `::` would produce output the parser cannot re-read (GitHub #235).
    fn write_pattern_constructor_name(&mut self, name: &str) {
        if name.contains("::") {
            self.writer.write(&name.replace("::", "."));
        } else {
            self.writer.write(name);
        }
    }

    fn format_match_arm(&mut self, arm: &Spanned<MatchArm>) {
        self.writer.blank_lines(arm.leading_blank_lines as usize);
        let arm = &arm.node;
        self.format_pattern(&arm.pattern.node);
        if let Some(guard) = &arm.guard {
            self.writer.write(" if ");
            self.format_expr(&guard.node);
        }
        match &arm.body {
            MatchBody::Expr(expr) => {
                self.writer.write(" => ");
                self.format_expr(&expr.node);
                self.writer.newline();
            }
            MatchBody::Block(stmts) => {
                let checkpoint = self.writer.checkpoint();
                self.writer.write(" => ");
                if self.try_format_inline_match_statement(stmts) {
                    return;
                }
                self.writer.restore(checkpoint);
                self.writer.write(" =>");
                self.writer.newline();
                self.writer.indent();
                for stmt in stmts {
                    self.format_statement(stmt);
                }
                self.writer.dedent();
            }
        }
    }

    fn try_format_inline_match_statement(&mut self, stmts: &[Spanned<Statement>]) -> bool {
        let [stmt] = stmts else {
            return false;
        };
        if stmt.leading_blank_lines > 0 {
            return false;
        }

        let checkpoint = self.writer.checkpoint();
        if !self.format_statement_inline(&stmt.node)
            || self.writer.output_since_contains_newline(checkpoint)
            || self.writer.line_length_exceeded()
        {
            self.writer.restore(checkpoint);
            return false;
        }

        self.writer.newline();
        true
    }

    fn format_statement_inline(&mut self, stmt: &Statement) -> bool {
        match stmt {
            Statement::Expr(expr) => self.format_expr(&expr.node),
            Statement::Return(expr) => {
                self.writer.write("return");
                if let Some(e) = expr {
                    self.writer.write(" ");
                    self.format_expr(&e.node);
                }
            }
            Statement::Pass => self.writer.write("pass"),
            Statement::Break(value) => {
                self.writer.write("break");
                if let Some(expr) = value {
                    self.writer.write(" ");
                    self.format_expr(&expr.node);
                }
            }
            Statement::Continue => self.writer.write("continue"),
            _ => return false,
        }
        true
    }

    /// Format a pattern in match-arm, `if let`, and nested constructor-pattern positions.
    pub(super) fn format_pattern(&mut self, pattern: &Pattern) {
        match pattern {
            Pattern::Wildcard => self.writer.write("_"),
            Pattern::Binding(name) => self.writer.write(name),
            Pattern::Literal(lit) => self.format_literal(lit),
            Pattern::Constructor(name, patterns) => {
                self.write_pattern_constructor_name(name);
                if !patterns.is_empty() {
                    self.writer.write("(");
                    for (i, p) in patterns.iter().enumerate() {
                        if i > 0 {
                            self.writer.write(", ");
                        }
                        match p {
                            PatternArg::Positional(pat) => {
                                self.format_pattern(&pat.node);
                            }
                            PatternArg::Named(name, pat) => {
                                self.writer.write(name);
                                self.writer.write("=");
                                self.format_pattern(&pat.node);
                            }
                        }
                    }
                    self.writer.write(")");
                }
            }
            Pattern::Tuple(patterns) => {
                self.writer.write("(");
                for (i, p) in patterns.iter().enumerate() {
                    if i > 0 {
                        self.writer.write(", ");
                    }
                    self.format_pattern(&p.node);
                }
                self.writer.write(")");
            }
            Pattern::Group(pattern) => {
                if let Pattern::Or(patterns) = &pattern.node {
                    self.format_grouped_or_pattern(patterns);
                } else {
                    self.writer.write("(");
                    self.format_pattern(&pattern.node);
                    self.writer.write(")");
                }
            }
            Pattern::Or(patterns) => {
                self.format_or_pattern(patterns);
            }
        }
    }

    /// Format pattern alternation inline first, then fall back to a grouped multiline layout when needed.
    fn format_or_pattern(&mut self, patterns: &[Spanned<Pattern>]) {
        let checkpoint = self.writer.checkpoint();
        self.format_or_pattern_inline(patterns);
        if !self.writer.line_length_exceeded() {
            return;
        }

        self.writer.restore(checkpoint);
        self.format_grouped_or_pattern(patterns);
    }

    /// Format alternation inside one pair of grouping parentheses.
    ///
    /// This helper preserves idempotence for already-grouped alternations: reformatting `(A | B)` must not produce
    /// nested parentheses just because the inner alternation is long.
    fn format_grouped_or_pattern(&mut self, patterns: &[Spanned<Pattern>]) {
        let checkpoint = self.writer.checkpoint();
        self.writer.write("(");
        self.format_or_pattern_inline(patterns);
        self.writer.write(")");
        if !self.writer.line_length_exceeded() {
            return;
        }

        self.writer.restore(checkpoint);
        self.writer.write("(");
        self.writer.newline();
        self.writer.indent();
        for (i, pattern) in patterns.iter().enumerate() {
            if i > 0 {
                self.writer.write("| ");
            }
            self.format_pattern(&pattern.node);
            self.writer.newline();
        }
        self.writer.dedent();
        self.writer.write(")");
    }

    /// Write alternation alternatives on the current line without adding grouping parentheses.
    fn format_or_pattern_inline(&mut self, patterns: &[Spanned<Pattern>]) {
        for (i, pattern) in patterns.iter().enumerate() {
            if i > 0 {
                self.writer.write(" | ");
            }
            self.format_pattern(&pattern.node);
        }
    }

    // ---- Types ----

    /// Format a type annotation or type argument.
    pub(super) fn format_type(&mut self, ty: &Type) {
        match ty {
            Type::Simple(name) => self.writer.write(name),
            Type::Qualified(segments) => {
                for (i, seg) in segments.iter().enumerate() {
                    if i > 0 {
                        self.writer.write("::");
                    }
                    self.writer.write(seg);
                }
            }
            Type::Generic(name, args) => {
                self.writer.write(name);
                self.writer.write("[");
                for (i, arg) in args.iter().enumerate() {
                    if i > 0 {
                        self.writer.write(", ");
                    }
                    self.format_type(&arg.node);
                }
                self.writer.write("]");
            }
            Type::ConstrainedPrimitive(name, constraints) => {
                self.writer.write(name);
                self.writer.write("[");
                for (i, constraint) in constraints.iter().enumerate() {
                    if i > 0 {
                        self.writer.write(", ");
                    }
                    self.writer.write(constraint.node.key.as_str());
                    self.writer.write("=");
                    self.writer.write(&constraint.node.value.repr);
                }
                self.writer.write("]");
            }
            Type::IntLiteral(value) => self.writer.write(&value.repr),
            Type::Tuple(types) => {
                self.writer.write("Tuple[");
                for (i, t) in types.iter().enumerate() {
                    if i > 0 {
                        self.writer.write(", ");
                    }
                    self.format_type(&t.node);
                }
                self.writer.write("]");
            }
            Type::Function(params, return_type) => {
                self.writer.write("(");
                for (i, p) in params.iter().enumerate() {
                    if i > 0 {
                        self.writer.write(", ");
                    }
                    self.format_type(&p.node);
                }
                self.writer.write(") -> ");
                self.format_type(&return_type.node);
            }
            Type::Ref(inner) => {
                self.writer.write("&");
                self.format_type(&inner.node);
            }
            Type::RefMut(inner) => {
                self.writer.write("&mut ");
                self.format_type(&inner.node);
            }
            Type::SelfType => self.writer.write("Self"),
            Type::Unit => self.writer.write("None"),
            Type::Infer => self.writer.write("_"),
        }
    }
}

/// Escape special characters in a string.
fn escape_string(s: &str) -> String {
    let mut result = String::new();
    for c in s.chars() {
        match c {
            '\n' => result.push_str("\\n"),
            '\r' => result.push_str("\\r"),
            '\t' => result.push_str("\\t"),
            '\\' => result.push_str("\\\\"),
            '"' => result.push_str("\\\""),
            c => result.push(c),
        }
    }
    result
}

/// Escape an f-string literal segment so formatter output stays parseable and semantically stable.
///
/// Unlike ordinary strings, `{` and `}` are control characters in f-strings and must be doubled when emitted as
/// literal text. We also preserve escaped control characters (`\n`, `\r`, `\t`) as textual escapes instead of
/// materializing physical whitespace in formatter output.
fn escape_fstring_literal(s: &str) -> String {
    let mut result = String::new();
    for c in s.chars() {
        match c {
            '\n' => result.push_str("\\n"),
            '\r' => result.push_str("\\r"),
            '\t' => result.push_str("\\t"),
            '\\' => result.push_str("\\\\"),
            '"' => result.push_str("\\\""),
            '{' => result.push_str("{{"),
            '}' => result.push_str("}}"),
            c => result.push(c),
        }
    }
    result
}
