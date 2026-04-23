//! Expression formatting: expressions, literals, operators, patterns, match arms, and types.

use crate::frontend::ast::*;
use incan_core::lang::keywords;
use incan_semantics_core::SurfaceFeatureKey;

use super::Formatter;

impl Formatter {
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
                self.format_call_args(args);
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
                self.format_call_args(args);
                self.writer.write(")");
            }
            Expr::Surface(surface_expr) => match (&surface_expr.key, &surface_expr.payload) {
                (SurfaceFeatureKey::SoftKeyword(id), SurfaceExprPayload::PrefixUnary(inner)) => {
                    self.writer.write(keywords::as_str(*id));
                    self.writer.write(" ");
                    self.format_expr(&inner.node);
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
                    self.format_match_arm(&arm.node);
                }
                self.writer.dedent();
            }
            Expr::If(if_expr) => {
                self.format_expr(&if_expr.condition.node);
                self.writer.write(" if ");
                // Note: This handles ternary-style if expressions
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
                    self.format_expr(&item.node);
                }
                self.writer.write("]");
            }
            Expr::Dict(pairs) => {
                self.writer.write("{");
                for (i, (k, v)) in pairs.iter().enumerate() {
                    if i > 0 {
                        self.writer.write(", ");
                    }
                    self.format_expr(&k.node);
                    self.writer.write(": ");
                    self.format_expr(&v.node);
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
                self.writer.write("(");
                self.format_expr(&inner.node);
                self.writer.write(")");
            }
            Expr::Constructor(name, args) => {
                self.writer.write(name);
                self.writer.write("(");
                self.format_call_args(args);
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
                self.writer.write(" for ");
                self.writer.write(&comp.var);
                self.writer.write(" in ");
                self.format_expr(&comp.iter.node);
                if let Some(filter) = &comp.filter {
                    self.writer.write(" if ");
                    self.format_expr(&filter.node);
                }
                self.writer.write("]");
            }
            Expr::DictComp(comp) => {
                self.writer.write("{");
                self.format_expr(&comp.key.node);
                self.writer.write(": ");
                self.format_expr(&comp.value.node);
                self.writer.write(" for ");
                self.writer.write(&comp.var);
                self.writer.write(" in ");
                self.format_expr(&comp.iter.node);
                if let Some(filter) = &comp.filter {
                    self.writer.write(" if ");
                    self.format_expr(&filter.node);
                }
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

    // ---- Literals ----

    fn format_literal(&mut self, lit: &Literal) {
        match lit {
            Literal::Int(il) => self.writer.write(&il.repr),
            // Emit source `FloatLiteral::repr`, not `f64` `Display` (which drops `.0`, etc.).
            Literal::Float(fl) => self.writer.write(&fl.repr),
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

    fn format_binary_op(&mut self, op: &BinaryOp) {
        self.writer.write(match op {
            BinaryOp::Add => "+",
            BinaryOp::Sub => "-",
            BinaryOp::Mul => "*",
            BinaryOp::Div => "/",
            BinaryOp::FloorDiv => "//",
            BinaryOp::Mod => "%",
            BinaryOp::Pow => "**",
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
        });
    }

    fn format_unary_op(&mut self, op: &UnaryOp) {
        self.writer.write(match op {
            UnaryOp::Neg => "-",
            UnaryOp::Not => "not ",
        });
    }

    // ---- Call args ----

    fn format_call_args(&mut self, args: &[CallArg]) {
        for (i, arg) in args.iter().enumerate() {
            if i > 0 {
                self.writer.write(", ");
            }
            match arg {
                CallArg::Positional(expr) => self.format_expr(&expr.node),
                CallArg::Named(name, expr) => {
                    self.writer.write(name);
                    self.writer.write("=");
                    self.format_expr(&expr.node);
                }
            }
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

    fn format_match_arm(&mut self, arm: &MatchArm) {
        self.format_pattern(&arm.pattern.node);
        if let Some(guard) = &arm.guard {
            self.writer.write(" if ");
            self.format_expr(&guard.node);
        }
        self.writer.write(" => ");
        match &arm.body {
            MatchBody::Expr(expr) => {
                self.format_expr(&expr.node);
                self.writer.newline();
            }
            MatchBody::Block(stmts) => {
                self.writer.newline();
                self.writer.indent();
                for stmt in stmts {
                    self.format_statement(stmt);
                }
                self.writer.dedent();
            }
        }
    }

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
        }
    }

    // ---- Types ----

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
