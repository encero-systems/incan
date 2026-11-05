//! Call and partial arguments, lists, logical chains and fluent method chains.

use super::*;

impl Formatter {
    /// Return whether a binary operator is a logical chain breakpoint.
    pub(super) fn is_logical_binary_op(op: &BinaryOp) -> bool {
        matches!(op, BinaryOp::And | BinaryOp::Or)
    }

    /// Return whether an expression starts a logical binary chain.
    pub(in crate::formatter) fn is_logical_binary_chain(expr: &Expr) -> bool {
        matches!(expr, Expr::Binary(_, op, _) if Self::is_logical_binary_op(op))
    }

    /// Write one formatted call argument.
    pub(super) fn write_call_arg(&mut self, arg: &CallArg) {
        match arg {
            CallArg::Positional(expr) => self.format_spanned_expr(expr),
            CallArg::Named(name, expr) => {
                self.writer.write(&name.node);
                self.writer.write("=");
                self.format_spanned_expr(expr);
            }
            CallArg::PositionalUnpack(expr) => {
                self.writer.write("*");
                self.format_spanned_expr(expr);
            }
            CallArg::KeywordUnpack(expr) => {
                self.writer.write("**");
                self.format_spanned_expr(expr);
            }
        }
    }

    /// Format one keyword preset in a partial callable template.
    pub(super) fn write_partial_arg(&mut self, arg: &PartialArg) {
        self.writer.write(&arg.name);
        self.writer.write("=");
        self.format_spanned_expr(&arg.value);
    }

    /// Keep list elements together when they fit, otherwise start each element on its own line.
    pub(super) fn format_list(&mut self, items: &[ListEntry]) {
        let checkpoint = self.writer.checkpoint();
        self.writer.write("[");
        for (index, item) in items.iter().enumerate() {
            if index > 0 {
                self.writer.write(", ");
            }
            self.format_list_entry(item);
        }
        self.writer.write("]");
        if items.is_empty()
            || (!self.writer.output_since_contains_newline(checkpoint) && !self.writer.line_length_exceeded())
        {
            return;
        }
        self.writer.restore(checkpoint);
        self.writer.write("[");
        self.writer.newline();
        self.writer.indent();
        for (index, item) in items.iter().enumerate() {
            self.format_list_entry(item);
            if self.writer.config().trailing_commas || index + 1 < items.len() {
                self.writer.write(",");
            }
            self.writer.newline();
        }
        self.writer.dedent();
        self.writer.write("]");
    }

    /// Format a list element or its spread prefix identically in inline and multiline layouts.
    pub(super) fn format_list_entry(&mut self, item: &ListEntry) {
        match item {
            ListEntry::Element(value) => self.format_spanned_expr(value),
            ListEntry::Spread(value) => {
                self.writer.write("*");
                self.format_spanned_expr(value);
            }
        }
    }

    /// Format the comma-separated keyword preset list in a partial callable template.
    pub(in crate::formatter) fn format_partial_args(&mut self, args: &[PartialArg]) {
        for (i, arg) in args.iter().enumerate() {
            if i > 0 {
                self.writer.write(", ");
            }
            self.write_partial_arg(arg);
        }
    }

    /// Format call arguments with line wrapping when needed.
    pub(super) fn format_call_args_with_wrapping(&mut self, args: &[CallArg]) {
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

    /// Format a parenthesized logical chain on one line when it fits there; otherwise put one operand per line.
    ///
    /// A one-line attempt that only fits by wrapping an operand's own arguments does not count as fitting: that hides
    /// the chain's operators mid-line, so the chain takes its block layout instead.
    pub(in crate::formatter) fn format_parenthesized_logical_chain(&mut self, inner: &Spanned<Expr>) {
        let checkpoint = self.writer.checkpoint();
        self.writer.write("(");
        self.format_spanned_expr(inner);
        self.writer.write(")");
        if !self.writer.output_since_contains_newline(checkpoint) && !self.writer.line_length_exceeded() {
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
    pub(super) fn format_logical_chain_lines(
        &mut self,
        expr: &Spanned<Expr>,
        leading_op: Option<&BinaryOp>,
        wrote_line: &mut bool,
    ) {
        match &expr.node {
            Expr::Binary(left, op, right) if Self::is_logical_binary_op(op) => {
                self.format_logical_chain_lines(left, leading_op, wrote_line);
                self.format_logical_chain_lines(right, Some(op), wrote_line);
            }
            _ => {
                if *wrote_line {
                    self.writer.newline();
                }
                if let Some(op) = leading_op {
                    self.format_binary_op(op);
                    self.writer.write(" ");
                }
                self.format_spanned_expr(expr);
                *wrote_line = true;
            }
        }
    }

    /// Collect a nested method-call chain into its root expression plus source-ordered method segments.
    pub(super) fn collect_method_chain<'a>(expr: &'a Expr, segments: &mut Vec<MethodChainSegment<'a>>) -> &'a Expr {
        let Expr::MethodCall(receiver, method, type_args, args) = expr else {
            return expr;
        };

        let root = Self::collect_method_chain(&receiver.node, segments);
        segments.push(MethodChainSegment {
            method,
            type_args,
            args,
        });
        root
    }

    /// Return whether a fluent chain root can be printed without adding precedence-protecting parentheses.
    pub(super) fn fluent_chain_root_is_simple(expr: &Expr) -> bool {
        matches!(
            expr,
            Expr::Ident(_) | Expr::SelfExpr | Expr::Call(_, _, _) | Expr::Field(_, _) | Expr::Index(_, _)
        )
    }

    /// Try inline method-chain formatting first, then fall back to a leading-dot fluent layout when it overflows.
    pub(super) fn format_fluent_method_chain_if_needed(&mut self, expr: &Expr) -> bool {
        let mut segments = Vec::new();
        let root = Self::collect_method_chain(expr, &mut segments);
        if segments.len() < 2 || !Self::fluent_chain_root_is_simple(root) {
            return false;
        }

        let checkpoint = self.writer.checkpoint();
        self.format_method_chain_inline(root, &segments);
        if !self.writer.output_since_contains_newline(checkpoint) && !self.writer.line_length_exceeded() {
            return true;
        }

        self.writer.restore(checkpoint);
        self.format_expr(root);
        self.writer.newline();
        self.writer.indent();
        for (idx, segment) in segments.iter().enumerate() {
            if idx > 0 {
                self.writer.newline();
            }
            self.format_method_chain_segment(segment);
        }
        self.writer.dedent();
        true
    }

    /// Write a full method chain without inserting fluent line breaks.
    pub(super) fn format_method_chain_inline(&mut self, root: &Expr, segments: &[MethodChainSegment<'_>]) {
        self.format_expr(root);
        for segment in segments {
            self.format_method_chain_segment(segment);
        }
    }

    /// Write one `.method[...]?()` segment of a method chain.
    pub(super) fn format_method_chain_segment(&mut self, segment: &MethodChainSegment<'_>) {
        self.writer.write(".");
        self.writer.write(segment.method);
        self.format_call_type_args(segment.type_args);
        self.writer.write("(");
        self.format_call_args_with_wrapping(segment.args);
        self.writer.write(")");
    }

    /// Format a source expression, retaining triple-quoted literal spelling instead of reconstructing its value.
    ///
    /// Spans distinguish equal-valued literals with different spellings and remain stable during speculative layout.
    pub(in crate::formatter) fn format_spanned_expr(&mut self, expr: &Spanned<Expr>) {
        if matches!(expr.node, Expr::Literal(Literal::String(_))) && self.write_source_string(expr.span) {
            return;
        }
        self.format_expr(&expr.node);
    }

    /// Write an original triple-quoted token verbatim, including content indentation, escapes and delimiters.
    pub(super) fn write_source_string(&mut self, span: Span) -> bool {
        let Some(raw) = self
            .source
            .as_deref()
            .and_then(|source| source.get(span.start..span.end))
        else {
            return false;
        };
        if !raw.starts_with("\"\"\"") && !raw.starts_with("'''") {
            return false;
        }
        self.writer.write(raw);
        true
    }
}
