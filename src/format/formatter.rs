//! Core formatting logic for Incan source code
//!
//! Walks the AST and emits properly formatted source code.

use super::config::FormatConfig;
use super::writer::FormatWriter;
use crate::frontend::ast::*;

/// Formatter that transforms AST back to formatted source code
pub struct Formatter {
    writer: FormatWriter,
}

impl Formatter {
    /// Create a new formatter with the given config
    pub fn new(config: FormatConfig) -> Self {
        Self {
            writer: FormatWriter::new(config),
        }
    }

    /// Format a program and return the formatted source
    pub fn format(mut self, program: &Program) -> String {
        self.format_program(program);
        self.writer.finish()
    }

    fn write_visibility(&mut self, visibility: crate::frontend::ast::Visibility) {
        if matches!(visibility, crate::frontend::ast::Visibility::Public) {
            self.writer.write("pub ");
        }
    }

    // ========================================================================
    // Program
    // ========================================================================

    fn format_program(&mut self, program: &Program) {
        let mut first = true;
        let mut prev_was_docstring = false;

        for decl in &program.declarations {
            // Add blank lines between top-level declarations
            if !first {
                if prev_was_docstring {
                    // Single blank line after module docstring
                    self.writer.newline();
                } else {
                    // Two blank lines between other declarations
                    self.writer.blank_lines(2);
                }
            }

            prev_was_docstring = matches!(&decl.node, Declaration::Docstring(_));
            self.format_declaration(&decl.node);
            first = false;
        }

        // Ensure file ends with newline
        self.writer.newline();
    }

    // ========================================================================
    // Declarations
    // ========================================================================

    fn format_declaration(&mut self, decl: &Declaration) {
        match decl {
            Declaration::Import(import) => self.format_import(import),
            Declaration::Const(konst) => self.format_const(konst),
            Declaration::Model(model) => self.format_model(model),
            Declaration::Class(class) => self.format_class(class),
            Declaration::Trait(tr) => self.format_trait(tr),
            Declaration::Newtype(nt) => self.format_newtype(nt),
            Declaration::Enum(en) => self.format_enum(en),
            Declaration::Function(func) => self.format_function(func),
            Declaration::Docstring(doc) => self.format_docstring(doc),
        }
    }

    fn format_const(&mut self, konst: &ConstDecl) {
        self.write_visibility(konst.visibility);
        self.writer.write("const ");
        self.writer.write(&konst.name);
        if let Some(ty) = &konst.ty {
            self.writer.write(": ");
            self.format_type(&ty.node);
        }
        self.writer.write(" = ");
        self.format_expr(&konst.value.node);
        self.writer.newline();
    }

    fn format_docstring(&mut self, doc: &str) {
        // Trim leading and trailing whitespace from the docstring content
        // to ensure idempotent formatting
        let trimmed = doc.trim();
        if trimmed.is_empty() {
            self.writer.writeln("\"\"\"\"\"\"");
        } else if trimmed.contains('\n') {
            // Multi-line docstring
            self.writer.writeln("\"\"\"");
            for line in trimmed.lines() {
                self.writer.writeln(line);
            }
            self.writer.writeln("\"\"\"");
        } else {
            // Single-line docstring
            self.writer.write("\"\"\"");
            self.writer.write(trimmed);
            self.writer.writeln("\"\"\"");
        }
    }

    fn format_import(&mut self, import: &ImportDecl) {
        match &import.kind {
            ImportKind::Module(path) => {
                self.writer.write("import ");
                self.format_import_path(path);
                if let Some(alias) = &import.alias {
                    self.writer.write(" as ");
                    self.writer.write(alias);
                }
                self.writer.newline();
            }
            ImportKind::From { module, items } => {
                self.writer.write("from ");
                self.format_import_path(module);
                self.writer.write(" import ");
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        self.writer.write(", ");
                    }
                    self.writer.write(&item.name);
                    if let Some(alias) = &item.alias {
                        self.writer.write(" as ");
                        self.writer.write(alias);
                    }
                }
                self.writer.newline();
            }
            ImportKind::Python(name) => {
                self.writer.write("import python \"");
                self.writer.write(name);
                self.writer.write("\"");
                if let Some(alias) = &import.alias {
                    self.writer.write(" as ");
                    self.writer.write(alias);
                }
                self.writer.newline();
            }
            ImportKind::RustCrate { crate_name, path } => {
                self.writer.write("import rust::");
                self.writer.write(crate_name);
                for segment in path {
                    self.writer.write("::");
                    self.writer.write(segment);
                }
                if let Some(alias) = &import.alias {
                    self.writer.write(" as ");
                    self.writer.write(alias);
                }
                self.writer.newline();
            }
            ImportKind::RustFrom {
                crate_name,
                path,
                items,
            } => {
                self.writer.write("from rust::");
                self.writer.write(crate_name);
                for segment in path {
                    self.writer.write("::");
                    self.writer.write(segment);
                }
                self.writer.write(" import ");
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        self.writer.write(", ");
                    }
                    self.writer.write(&item.name);
                    if let Some(alias) = &item.alias {
                        self.writer.write(" as ");
                        self.writer.write(alias);
                    }
                }
                self.writer.newline();
            }
        }
    }

    fn format_import_path(&mut self, path: &ImportPath) {
        let mut parts: Vec<&str> = Vec::new();
        if path.is_absolute {
            parts.push("crate");
        } else {
            parts.extend(std::iter::repeat_n("super", path.parent_levels));
        }
        for segment in &path.segments {
            parts.push(segment);
        }
        for (i, part) in parts.iter().enumerate() {
            if i > 0 {
                self.writer.write(".");
            }
            self.writer.write(part);
        }
    }

    fn format_model(&mut self, model: &ModelDecl) {
        // Decorators
        for dec in &model.decorators {
            self.format_decorator(&dec.node);
        }

        // model Name[T] with Trait1, Trait2:
        self.write_visibility(model.visibility);
        self.writer.write("model ");
        self.writer.write(&model.name);
        self.format_type_params(&model.type_params);
        if !model.traits.is_empty() {
            self.writer.write(" with ");
            for (i, trait_name) in model.traits.iter().enumerate() {
                if i > 0 {
                    self.writer.write(", ");
                }
                self.writer.write(&trait_name.node);
            }
        }
        self.writer.writeln(":");
        self.writer.indent();

        // Fields
        let has_fields = !model.fields.is_empty();
        for field in &model.fields {
            self.format_field(&field.node);
        }

        // Methods
        let mut first_method = true;
        for method in &model.methods {
            if has_fields || !first_method {
                self.writer.newline();
            }
            self.format_method(&method.node);
            first_method = false;
        }

        // Empty body
        if model.fields.is_empty() && model.methods.is_empty() {
            self.writer.writeln("pass");
        }

        self.writer.dedent();
    }

    fn format_class(&mut self, class: &ClassDecl) {
        // Decorators
        for dec in &class.decorators {
            self.format_decorator(&dec.node);
        }

        // class Name[T] extends Base with Trait1:
        self.write_visibility(class.visibility);
        self.writer.write("class ");
        self.writer.write(&class.name);
        self.format_type_params(&class.type_params);

        if let Some(base) = &class.extends {
            self.writer.write(" extends ");
            self.writer.write(base);
        }

        if !class.traits.is_empty() {
            self.writer.write(" with ");
            for (i, trait_name) in class.traits.iter().enumerate() {
                if i > 0 {
                    self.writer.write(", ");
                }
                self.writer.write(&trait_name.node);
            }
        }

        self.writer.writeln(":");
        self.writer.indent();

        // Fields
        let has_fields = !class.fields.is_empty();
        for field in &class.fields {
            self.format_field(&field.node);
        }

        // Methods
        let mut first_method = true;
        for method in &class.methods {
            if has_fields || !first_method {
                self.writer.newline();
            }
            self.format_method(&method.node);
            first_method = false;
        }

        // Empty body
        if class.fields.is_empty() && class.methods.is_empty() {
            self.writer.writeln("pass");
        }

        self.writer.dedent();
    }

    fn format_trait(&mut self, tr: &TraitDecl) {
        // Decorators
        for dec in &tr.decorators {
            self.format_decorator(&dec.node);
        }

        // trait Name[T]:
        self.write_visibility(tr.visibility);
        self.writer.write("trait ");
        self.writer.write(&tr.name);
        self.format_type_params(&tr.type_params);
        self.writer.writeln(":");
        self.writer.indent();

        // Methods
        let mut first = true;
        for method in &tr.methods {
            if !first {
                self.writer.newline();
            }
            self.format_method(&method.node);
            first = false;
        }

        // Empty body
        if tr.methods.is_empty() {
            self.writer.writeln("pass");
        }

        self.writer.dedent();
    }

    fn format_enum(&mut self, en: &EnumDecl) {
        // Decorators
        for dec in &en.decorators {
            self.format_decorator(&dec.node);
        }

        // enum Name[T]:
        self.write_visibility(en.visibility);
        self.writer.write("enum ");
        self.writer.write(&en.name);
        self.format_type_params(&en.type_params);
        self.writer.writeln(":");
        self.writer.indent();

        for variant in &en.variants {
            self.format_enum_variant(&variant.node);
        }

        if en.variants.is_empty() {
            self.writer.writeln("pass");
        }

        self.writer.dedent();
    }

    fn format_enum_variant(&mut self, variant: &VariantDecl) {
        self.writer.write(&variant.name);
        if !variant.fields.is_empty() {
            self.writer.write("(");
            for (i, field) in variant.fields.iter().enumerate() {
                if i > 0 {
                    self.writer.write(", ");
                }
                self.format_type(&field.node);
            }
            self.writer.write(")");
        }
        self.writer.newline();
    }

    fn format_newtype(&mut self, nt: &NewtypeDecl) {
        // type Name = newtype underlying
        self.write_visibility(nt.visibility);
        self.writer.write("type ");
        self.writer.write(&nt.name);
        self.writer.write(" = newtype ");
        self.format_type(&nt.underlying.node);
        self.writer.newline();

        // Methods if any
        if !nt.methods.is_empty() {
            self.writer.indent();
            for method in &nt.methods {
                self.writer.newline();
                self.format_method(&method.node);
            }
            self.writer.dedent();
        }
    }

    fn format_function(&mut self, func: &FunctionDecl) {
        // Decorators
        for dec in &func.decorators {
            self.format_decorator(&dec.node);
        }

        // async def name(params) -> ReturnType:
        self.write_visibility(func.visibility);
        if func.is_async {
            self.writer.write("async ");
        }
        self.writer.write("def ");
        self.writer.write(&func.name);
        self.writer.write("(");
        self.format_params(&func.params);
        self.writer.write(") -> ");
        self.format_type(&func.return_type.node);
        self.writer.writeln(":");

        self.writer.indent();

        // Body
        if func.body.is_empty() {
            self.writer.writeln("pass");
        } else {
            for stmt in &func.body {
                self.format_statement(&stmt.node);
            }
        }

        self.writer.dedent();
    }

    fn format_method(&mut self, method: &MethodDecl) {
        // Decorators
        for dec in &method.decorators {
            self.format_decorator(&dec.node);
        }

        // async def name(self, params) -> ReturnType:
        if method.is_async {
            self.writer.write("async ");
        }
        self.writer.write("def ");
        self.writer.write(&method.name);
        self.writer.write("(");

        // Receiver
        let has_receiver = method.receiver.is_some();
        if let Some(receiver) = &method.receiver {
            match receiver {
                Receiver::Immutable => self.writer.write("self"),
                Receiver::Mutable => self.writer.write("mut self"),
            }
        }

        // Parameters
        if has_receiver && !method.params.is_empty() {
            self.writer.write(", ");
        }
        self.format_params(&method.params);

        self.writer.write(") -> ");
        self.format_type(&method.return_type.node);

        // Abstract method (ellipsis body)
        if method.body.is_none() {
            self.writer.writeln(": ...");
            return;
        }

        self.writer.writeln(":");
        self.writer.indent();

        if let Some(body) = &method.body {
            if body.is_empty() {
                self.writer.writeln("pass");
            } else {
                for stmt in body {
                    self.format_statement(&stmt.node);
                }
            }
        }

        self.writer.dedent();
    }

    fn format_decorator(&mut self, dec: &Decorator) {
        self.writer.write("@");
        self.format_decorator_path(&dec.path);
        if !dec.args.is_empty() {
            self.writer.write("(");
            for (i, arg) in dec.args.iter().enumerate() {
                if i > 0 {
                    self.writer.write(", ");
                }
                match arg {
                    DecoratorArg::Positional(expr) => self.format_expr(&expr.node),
                    DecoratorArg::Named(name, value) => {
                        self.writer.write(name);
                        self.writer.write("=");
                        match value {
                            DecoratorArgValue::Type(ty) => self.format_type(&ty.node),
                            DecoratorArgValue::Expr(expr) => self.format_expr(&expr.node),
                        }
                    }
                }
            }
            self.writer.write(")");
        }
        self.writer.newline();
    }

    fn format_decorator_path(&mut self, path: &ImportPath) {
        let mut parts: Vec<&str> = Vec::new();
        if path.is_absolute {
            parts.push("crate");
        } else {
            parts.extend(std::iter::repeat_n("super", path.parent_levels));
        }
        for segment in &path.segments {
            parts.push(segment);
        }
        for (i, part) in parts.iter().enumerate() {
            if i > 0 {
                self.writer.write(".");
            }
            self.writer.write(part);
        }
    }

    fn format_field(&mut self, field: &FieldDecl) {
        self.write_visibility(field.visibility);
        self.writer.write(&field.name);
        let alias = field.metadata.alias.as_deref();
        let description = field.metadata.description.as_deref();
        let use_as_sugar = alias.is_some() && description.is_none();

        if !use_as_sugar && (alias.is_some() || description.is_some()) {
            self.writer.write(" [");
            let mut wrote = false;
            if let Some(alias) = alias {
                self.writer.write("alias=\"");
                self.writer.write(alias);
                self.writer.write("\"");
                wrote = true;
            }
            if let Some(description) = description {
                if wrote {
                    self.writer.write(", ");
                }
                self.writer.write("description=\"");
                self.writer.write(description);
                self.writer.write("\"");
            }
            self.writer.write("]");
        }
        if use_as_sugar {
            self.writer.write(" as \"");
            self.writer.write(alias.unwrap_or_default());
            self.writer.write("\"");
        }
        self.writer.write(": ");
        self.format_type(&field.ty.node);
        if let Some(default) = &field.default {
            self.writer.write(" = ");
            self.format_expr(&default.node);
        }
        self.writer.newline();
    }

    fn format_params(&mut self, params: &[Spanned<Param>]) {
        for (i, param) in params.iter().enumerate() {
            if i > 0 {
                self.writer.write(", ");
            }
            self.format_param(&param.node);
        }
    }

    fn format_param(&mut self, param: &Param) {
        self.writer.write(&param.name);
        self.writer.write(": ");
        self.format_type(&param.ty.node);
        if let Some(default) = &param.default {
            self.writer.write(" = ");
            self.format_expr(&default.node);
        }
    }

    fn format_type_params(&mut self, params: &[Ident]) {
        if !params.is_empty() {
            self.writer.write("[");
            for (i, param) in params.iter().enumerate() {
                if i > 0 {
                    self.writer.write(", ");
                }
                self.writer.write(param);
            }
            self.writer.write("]");
        }
    }

    // ========================================================================
    // Types
    // ========================================================================

    fn format_type(&mut self, ty: &Type) {
        match ty {
            Type::Simple(name) => self.writer.write(name),
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
        }
    }

    // ========================================================================
    // Statements
    // ========================================================================

    fn format_statement(&mut self, stmt: &Statement) {
        match stmt {
            Statement::Expr(expr) => {
                self.format_expr(&expr.node);
                self.writer.newline();
            }
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
            Statement::While(while_stmt) => self.format_while(while_stmt),
            Statement::For(for_stmt) => self.format_for(for_stmt),
            Statement::Pass => self.writer.writeln("pass"),
            Statement::Break => self.writer.writeln("break"),
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

    fn format_if(&mut self, if_stmt: &IfStmt) {
        self.writer.write("if ");
        self.format_expr(&if_stmt.condition.node);
        self.writer.writeln(":");
        self.writer.indent();
        for stmt in &if_stmt.then_body {
            self.format_statement(&stmt.node);
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
                self.format_statement(&stmt.node);
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
                self.format_statement(&stmt.node);
            }
            if else_body.is_empty() {
                self.writer.writeln("pass");
            }
            self.writer.dedent();
        }
    }

    fn format_while(&mut self, while_stmt: &WhileStmt) {
        self.writer.write("while ");
        self.format_expr(&while_stmt.condition.node);
        self.writer.writeln(":");
        self.writer.indent();
        for stmt in &while_stmt.body {
            self.format_statement(&stmt.node);
        }
        if while_stmt.body.is_empty() {
            self.writer.writeln("pass");
        }
        self.writer.dedent();
    }

    fn format_for(&mut self, for_stmt: &ForStmt) {
        self.writer.write("for ");
        self.writer.write(&for_stmt.var);
        self.writer.write(" in ");
        self.format_expr(&for_stmt.iter.node);
        self.writer.writeln(":");
        self.writer.indent();
        for stmt in &for_stmt.body {
            self.format_statement(&stmt.node);
        }
        if for_stmt.body.is_empty() {
            self.writer.writeln("pass");
        }
        self.writer.dedent();
    }

    // ========================================================================
    // Expressions
    // ========================================================================

    fn format_expr(&mut self, expr: &Expr) {
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
            Expr::Call(callee, args) => {
                self.format_expr(&callee.node);
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
            Expr::MethodCall(receiver, method, args) => {
                self.format_expr(&receiver.node);
                self.writer.write(".");
                self.writer.write(method);
                self.writer.write("(");
                self.format_call_args(args);
                self.writer.write(")");
            }
            Expr::Await(inner) => {
                self.writer.write("await ");
                self.format_expr(&inner.node);
            }
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
                        FStringPart::Literal(s) => self.writer.write(s),
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

    fn format_literal(&mut self, lit: &Literal) {
        match lit {
            Literal::Int(n) => self.writer.write(&n.to_string()),
            Literal::Float(f) => self.writer.write(&f.to_string()),
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
                    self.format_statement(&stmt.node);
                }
                self.writer.dedent();
            }
        }
    }

    fn format_pattern(&mut self, pattern: &Pattern) {
        match pattern {
            Pattern::Wildcard => self.writer.write("_"),
            Pattern::Binding(name) => self.writer.write(name),
            Pattern::Literal(lit) => self.format_literal(lit),
            Pattern::Constructor(name, patterns) => {
                self.writer.write(name);
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
}

/// Escape special characters in a string
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
