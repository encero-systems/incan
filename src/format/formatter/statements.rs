//! Statement formatting: assignments, control flow (if/elif/else, while, for), and compound statements.

use crate::frontend::ast::*;
use incan_core::lang::keywords;
use incan_semantics_core::SurfaceFeatureKey;

use super::Formatter;

impl Formatter {
    pub(super) fn format_statement(&mut self, stmt: &Spanned<Statement>) {
        self.writer.blank_lines(stmt.leading_blank_lines as usize);
        match &stmt.node {
            Statement::Expr(expr) => {
                self.format_expr(&expr.node);
                if !self.writer.is_at_line_start() {
                    self.writer.newline();
                }
            }
            Statement::Assert(assert_stmt) => self.format_assert(assert_stmt),
            Statement::Assignment(assign) => {
                self.format_assignment(assign);
            }
            Statement::FieldAssignment(assign) => {
                self.format_expr(&assign.object.node);
                self.writer.write(".");
                self.writer.write(&assign.field);
                self.writer.write(" = ");
                self.format_expr(&assign.value.node);
                self.writer.newline();
            }
            Statement::IndexAssignment(assign) => {
                self.format_expr(&assign.object.node);
                self.writer.write("[");
                self.format_expr(&assign.index.node);
                self.writer.write("] = ");
                self.format_expr(&assign.value.node);
                self.writer.newline();
            }
            Statement::CompoundAssignment(assign) => {
                self.writer.write(&assign.name);
                self.writer.write(" ");
                self.writer.write(match assign.op {
                    CompoundOp::Add => "+=",
                    CompoundOp::Sub => "-=",
                    CompoundOp::Mul => "*=",
                    CompoundOp::Div => "/=",
                    CompoundOp::FloorDiv => "//=",
                    CompoundOp::Mod => "%=",
                });
                self.writer.write(" ");
                self.format_expr(&assign.value.node);
                self.writer.newline();
            }
            Statement::Return(expr) => {
                self.writer.write("return");
                if let Some(e) = expr {
                    self.writer.write(" ");
                    self.format_expr(&e.node);
                }
                self.writer.newline();
            }
            Statement::If(if_stmt) => self.format_if(if_stmt),
            Statement::Loop(loop_stmt) => self.format_loop(loop_stmt),
            Statement::While(while_stmt) => self.format_while(while_stmt),
            Statement::For(for_stmt) => self.format_for(for_stmt),
            Statement::Surface(surface_stmt) => match (&surface_stmt.key, &surface_stmt.payload) {
                (SurfaceFeatureKey::SoftKeyword(id), SurfaceStmtPayload::KeywordArgs(args)) => {
                    self.writer.write(keywords::as_str(*id));
                    self.writer.write(" ");
                    for (idx, arg) in args.iter().enumerate() {
                        if idx > 0 {
                            self.writer.write(", ");
                        }
                        self.format_expr(&arg.node);
                    }
                    self.writer.newline();
                }
                _ => self.writer.writeln("<surface_stmt>"),
            },
            Statement::VocabBlock(vocab_block) => {
                for decorator in &vocab_block.decorators {
                    self.writer.write("@");
                    self.writer.writeln(&decorator.node.path.segments.join("."));
                }
                self.writer.write(&vocab_block.keyword);
                if !vocab_block.header_args.is_empty() {
                    self.writer.write(" ");
                    for (idx, arg) in vocab_block.header_args.iter().enumerate() {
                        if idx > 0 {
                            self.writer.write(", ");
                        }
                        self.format_expr(&arg.node);
                    }
                }
                self.writer.writeln(":");
                self.writer.indent();
                for stmt in &vocab_block.body {
                    self.format_statement(stmt);
                }
                if vocab_block.body.is_empty() {
                    self.writer.writeln("pass");
                }
                self.writer.dedent();
            }
            Statement::Pass => self.writer.writeln("pass"),
            Statement::Break(value) => {
                self.writer.write("break");
                if let Some(value) = value {
                    self.writer.write(" ");
                    self.format_expr(&value.node);
                }
                self.writer.newline();
            }
            Statement::Continue => self.writer.writeln("continue"),
            Statement::TupleUnpack(unpack) => {
                match unpack.binding {
                    BindingKind::Let => self.writer.write("let "),
                    BindingKind::Mutable => self.writer.write("mut "),
                    BindingKind::Inferred | BindingKind::Reassign => {}
                }
                for (i, name) in unpack.names.iter().enumerate() {
                    if i > 0 {
                        self.writer.write(", ");
                    }
                    self.writer.write(name);
                }
                self.writer.write(" = ");
                self.format_expr(&unpack.value.node);
                self.writer.newline();
            }
            Statement::TupleAssign(assign) => {
                for (i, target) in assign.targets.iter().enumerate() {
                    if i > 0 {
                        self.writer.write(", ");
                    }
                    self.format_expr(&target.node);
                }
                self.writer.write(" = ");
                self.format_expr(&assign.value.node);
                self.writer.newline();
            }
            Statement::ChainedAssignment(ca) => {
                match ca.binding {
                    BindingKind::Let => self.writer.write("let "),
                    BindingKind::Mutable => self.writer.write("mut "),
                    BindingKind::Inferred | BindingKind::Reassign => {}
                }
                for (i, target) in ca.targets.iter().enumerate() {
                    if i > 0 {
                        self.writer.write(" = ");
                    }
                    self.writer.write(target);
                }
                self.writer.write(" = ");
                self.format_expr(&ca.value.node);
                self.writer.newline();
            }
        }
    }

    fn format_assignment(&mut self, assign: &AssignmentStmt) {
        match assign.binding {
            BindingKind::Let => self.writer.write("let "),
            BindingKind::Mutable => self.writer.write("mut "),
            BindingKind::Inferred | BindingKind::Reassign => {}
        }
        self.writer.write(&assign.name);
        if let Some(ty) = &assign.ty {
            self.writer.write(": ");
            self.format_type(&ty.node);
        }
        self.writer.write(" = ");
        self.format_expr(&assign.value.node);
        self.writer.newline();
    }

    fn format_assert(&mut self, assert_stmt: &AssertStmt) {
        self.writer.write("assert ");
        match &assert_stmt.kind {
            AssertKind::Condition(condition) => self.format_expr(&condition.node),
            AssertKind::IsPattern { value, pattern } => {
                self.format_expr(&value.node);
                self.writer.write(" is ");
                self.format_pattern(&pattern.node);
            }
            AssertKind::Raises { call, error_type } => {
                self.format_expr(&call.node);
                self.writer.write(" raises ");
                self.format_type(&error_type.node);
            }
        }
        if let Some(message) = &assert_stmt.message {
            self.writer.write(", ");
            self.format_expr(&message.node);
        }
        self.writer.newline();
    }

    fn format_if(&mut self, if_stmt: &IfStmt) {
        self.writer.write("if ");
        self.format_condition(&if_stmt.condition);
        self.writer.writeln(":");
        self.writer.indent();
        for stmt in &if_stmt.then_body {
            self.format_statement(stmt);
        }
        if if_stmt.then_body.is_empty() {
            self.writer.writeln("pass");
        }
        self.writer.dedent();

        for (elif_cond, elif_body) in &if_stmt.elif_branches {
            self.writer.write("elif ");
            self.format_expr(&elif_cond.node);
            self.writer.writeln(":");
            self.writer.indent();
            for stmt in elif_body {
                self.format_statement(stmt);
            }
            if elif_body.is_empty() {
                self.writer.writeln("pass");
            }
            self.writer.dedent();
        }

        if let Some(else_body) = &if_stmt.else_body {
            self.writer.writeln("else:");
            self.writer.indent();
            for stmt in else_body {
                self.format_statement(stmt);
            }
            if else_body.is_empty() {
                self.writer.writeln("pass");
            }
            self.writer.dedent();
        }
    }

    fn format_loop(&mut self, loop_stmt: &LoopStmt) {
        self.writer.writeln("loop:");
        self.writer.indent();
        for stmt in &loop_stmt.body {
            self.format_statement(stmt);
        }
        if loop_stmt.body.is_empty() {
            self.writer.writeln("pass");
        }
        self.writer.dedent();
    }

    fn format_while(&mut self, while_stmt: &WhileStmt) {
        self.writer.write("while ");
        self.format_condition(&while_stmt.condition);
        self.writer.writeln(":");
        self.writer.indent();
        for stmt in &while_stmt.body {
            self.format_statement(stmt);
        }
        if while_stmt.body.is_empty() {
            self.writer.writeln("pass");
        }
        self.writer.dedent();
    }

    fn format_for(&mut self, for_stmt: &ForStmt) {
        self.writer.write("for ");
        self.format_for_pattern(&for_stmt.pattern.node);
        self.writer.write(" in ");
        self.format_expr(&for_stmt.iter.node);
        self.writer.writeln(":");
        self.writer.indent();
        for stmt in &for_stmt.body {
            self.format_statement(stmt);
        }
        if for_stmt.body.is_empty() {
            self.writer.writeln("pass");
        }
        self.writer.dedent();
    }

    fn format_for_pattern(&mut self, pattern: &Pattern) {
        if let Pattern::Tuple(items) = pattern {
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    self.writer.write(", ");
                }
                self.format_pattern(&item.node);
            }
        } else {
            self.format_pattern(pattern);
        }
    }

    fn format_condition(&mut self, condition: &Condition) {
        match condition {
            Condition::Expr(expr) => self.format_expr(&expr.node),
            Condition::Let { pattern, value } => {
                self.writer.write("let ");
                self.format_pattern(&pattern.node);
                self.writer.write(" = ");
                self.format_expr(&value.node);
            }
        }
    }
}
