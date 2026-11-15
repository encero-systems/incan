//! Literals, operators, closure parameters, `if` and `match` forms, patterns and types.

use super::*;

impl Formatter {
    // ---- Literals ----

    /// Format a literal while preserving source-sensitive numeric spellings.
    pub(super) fn format_literal(&mut self, lit: &Literal) {
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
                    // A quote or backslash byte is printable, so writing it raw closed the literal early or began a
                    // bogus escape: `b"a\\"b"` came back as `b"a"b"`, which no longer parses. Both need escaping
                    // before the printable-range shortcut, exactly as the string arm escapes through
                    // `escape_string` (#1401).
                    match byte {
                        b'"' => self.writer.write("\\\""),
                        b'\\' => self.writer.write("\\\\"),
                        _ if *byte >= 32 && *byte < 127 => self.writer.write(&(*byte as char).to_string()),
                        _ => self.writer.write(&format!("\\x{:02x}", byte)),
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
    pub(super) fn format_binary_op(&mut self, op: &BinaryOp) {
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
    pub(super) fn format_unary_op(&mut self, op: &UnaryOp) {
        self.writer.write(match op {
            UnaryOp::Neg => "-",
            UnaryOp::Not => "not ",
            UnaryOp::Invert => "~",
        });
    }

    // ---- Call args ----

    /// Format closure parameters.
    pub(super) fn format_closure_params(&mut self, params: &[Spanned<Param>]) {
        for (i, param) in params.iter().enumerate() {
            if i > 0 {
                self.writer.write(", ");
            }
            self.writer.write(&param.node.name);
        }
    }

    /// Format call arguments.
    pub(super) fn format_call_args(&mut self, args: &[CallArg]) {
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
    pub(super) fn write_pattern_constructor_name(&mut self, name: &str) {
        if name.contains("::") {
            self.writer.write(&name.replace("::", "."));
        } else {
            self.writer.write(name);
        }
    }

    /// Format both suites of an if expression, retaining every statement instead of emitting a ternary fragment.
    pub(super) fn format_if_expression(&mut self, expression: &IfExpr) {
        self.writer.write("if ");
        self.format_suite_header_expr(&expression.condition);
        self.writer.writeln(":");
        self.writer.indent();
        for statement in &expression.then_body {
            self.format_statement(statement);
        }
        if expression.then_body.is_empty() {
            self.writer.writeln("pass");
        }
        self.writer.dedent();
        if let Some(body) = &expression.else_body {
            self.writer.writeln("else:");
            self.writer.indent();
            for statement in body {
                self.format_statement(statement);
            }
            if body.is_empty() {
                self.writer.writeln("pass");
            }
            self.writer.dedent();
        }
    }

    /// Format one match arm.
    pub(super) fn format_match_arm(&mut self, arm: &Spanned<MatchArm>) {
        self.writer.blank_lines(arm.leading_blank_lines as usize);
        let arm = &arm.node;

        self.format_spanned_pattern(&arm.pattern);
        // A guard belongs to the arm, not to one of its two spellings, so it is written the same way before a `:`
        // and before a `=>`. Writing it only in `case` form used to be a workaround for the arrow form's parser
        // rejecting `<pattern> if <cond> =>`; that grammar gap is closed, so the arm keeps the shape it was
        // written in (#1401).
        if let Some(guard) = &arm.guard {
            self.writer.write(" if ");
            self.format_spanned_expr(guard);
        }
        match &arm.body {
            MatchBody::Expr(expr) => {
                self.writer.write(" => ");
                self.format_spanned_expr(expr);
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

    /// Try to format a match arm body as an inline statement.
    pub(super) fn try_format_inline_match_statement(&mut self, stmts: &[Spanned<Statement>]) -> bool {
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

    /// Format only statements accepted by the parser in inline match-arm contexts.
    pub(super) fn format_statement_inline(&mut self, stmt: &Statement) -> bool {
        match stmt {
            Statement::Expr(expr) => self.format_spanned_expr(expr),
            Statement::Return(expr) => {
                self.writer.write("return");
                if let Some(e) = expr {
                    self.writer.write(" ");
                    self.format_spanned_expr(e);
                }
            }
            Statement::Pass => self.writer.write("pass"),
            Statement::Break(value) => {
                self.writer.write("break");
                if let Some(expr) = value {
                    self.writer.write(" ");
                    self.format_spanned_expr(expr);
                }
            }
            Statement::Continue => self.writer.write("continue"),
            other => return self.write_assignment_statement(other),
        }
        true
    }

    /// Preserve source-sensitive literal patterns before emitting the pattern's structural syntax.
    pub(in crate::formatter) fn format_spanned_pattern(&mut self, pattern: &Spanned<Pattern>) {
        if matches!(pattern.node, Pattern::Literal(Literal::String(_))) && self.write_source_string(pattern.span) {
            return;
        }
        self.format_pattern(&pattern.node);
    }

    /// Format a pattern in match-arm, `if let`, and nested constructor-pattern positions.
    pub(in crate::formatter) fn format_pattern(&mut self, pattern: &Pattern) {
        match pattern {
            Pattern::Wildcard => self.writer.write("_"),
            Pattern::Binding(name) => self.writer.write(name),
            Pattern::Literal(lit) => self.format_literal(lit),
            Pattern::Constructor(name, patterns) => {
                self.write_pattern_constructor_name(&name.node);
                if !patterns.is_empty() {
                    self.writer.write("(");
                    for (i, p) in patterns.iter().enumerate() {
                        if i > 0 {
                            self.writer.write(", ");
                        }
                        match p {
                            PatternArg::Positional(pat) => {
                                self.format_spanned_pattern(pat);
                            }
                            PatternArg::Named(name, pat) => {
                                self.writer.write(&name.node);
                                self.writer.write("=");
                                self.format_spanned_pattern(pat);
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
                    self.format_spanned_pattern(p);
                }
                self.writer.write(")");
            }
            Pattern::Group(pattern) => {
                if let Pattern::Or(patterns) = &pattern.node {
                    self.format_grouped_or_pattern(patterns);
                } else {
                    self.writer.write("(");
                    self.format_spanned_pattern(pattern);
                    self.writer.write(")");
                }
            }
            Pattern::Or(patterns) => {
                self.format_or_pattern(patterns);
            }
        }
    }

    /// Format pattern alternation inline first, then fall back to a grouped multiline layout when needed.
    pub(super) fn format_or_pattern(&mut self, patterns: &[Spanned<Pattern>]) {
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
    pub(super) fn format_grouped_or_pattern(&mut self, patterns: &[Spanned<Pattern>]) {
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
            self.format_spanned_pattern(pattern);
            self.writer.newline();
        }
        self.writer.dedent();
        self.writer.write(")");
    }

    /// Write alternation alternatives on the current line without adding grouping parentheses.
    pub(super) fn format_or_pattern_inline(&mut self, patterns: &[Spanned<Pattern>]) {
        for (i, pattern) in patterns.iter().enumerate() {
            if i > 0 {
                self.writer.write(" | ");
            }
            self.format_spanned_pattern(pattern);
        }
    }

    // ---- Types ----

    /// Format a type annotation or type argument.
    pub(in crate::formatter) fn format_type(&mut self, ty: &Type) {
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
            Type::Dotted(segments) => {
                for (i, seg) in segments.iter().enumerate() {
                    if i > 0 {
                        self.writer.write(".");
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
            Type::DottedGeneric(segments, args) => {
                for (i, seg) in segments.iter().enumerate() {
                    if i > 0 {
                        self.writer.write(".");
                    }
                    self.writer.write(seg);
                }
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
            Type::MutParam(inner) => {
                self.writer.write("mut ");
                self.format_type(&inner.node);
            }
            Type::SelfType => self.writer.write("Self"),
            Type::Unit => self.writer.write("None"),
            Type::Infer => self.writer.write("_"),
        }
    }
}
