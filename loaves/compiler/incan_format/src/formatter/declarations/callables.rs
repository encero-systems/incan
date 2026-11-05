//! Function, method and property formatting, receivers, parameters, decorators, fields and bounds.

use super::*;

impl Formatter {
    // ---- Functions and methods ----

    /// Separate a parsed leading string statement from the executable suite for docstring formatting.
    pub(super) fn split_leading_docstring<'a>(
        &self,
        body: &'a [Spanned<Statement>],
    ) -> (Option<&'a str>, &'a [Spanned<Statement>]) {
        let Some(first) = body.first() else {
            return (None, body);
        };

        match &first.node {
            Statement::Expr(expr) => match &expr.node {
                Expr::Literal(Literal::String(doc)) => (Some(doc.as_str()), &body[1..]),
                _ => (None, body),
            },
            _ => (None, body),
        }
    }

    /// Format a function declaration, wrapping its parameter list when the header exceeds the line-length target.
    pub(super) fn format_function(&mut self, func: &FunctionDecl) {
        for dec in &func.decorators {
            self.format_decorator(&dec.node);
        }

        let checkpoint = self.writer.checkpoint();
        self.write_function_prefix(func.visibility, func.is_async(), &func.name, &func.type_params);
        self.writer.write("(");
        self.format_params(&func.params);
        self.writer.write(") -> ");
        self.format_type(&func.return_type.node);
        self.format_result_source(func.result_source.as_ref());
        self.writer.write(":");
        if self.writer.line_length_exceeded() {
            self.writer.restore(checkpoint);
            self.write_function_prefix(func.visibility, func.is_async(), &func.name, &func.type_params);
            self.writer.write("(");
            self.format_params_multiline(None, &func.params);
            self.writer.write(") -> ");
            self.format_type(&func.return_type.node);
            self.format_result_source(func.result_source.as_ref());
            self.writer.write(":");
        }
        self.writer.newline();

        self.writer.indent();

        if func.body.is_empty() {
            self.writer.writeln("pass");
        } else {
            let (docstring, body) = self.split_leading_docstring(&func.body);
            if let Some(docstring) = docstring {
                self.format_docstring(docstring);
            }

            if body.is_empty() && docstring.is_none() {
                self.writer.writeln("pass");
            } else {
                for stmt in body {
                    self.format_statement(stmt);
                }
            }
        }

        self.writer.dedent();
    }

    /// Write the parsed result-source relationship without reconstructing it from a type display.
    pub(super) fn format_result_source(&mut self, source: Option<&Spanned<ResultSource>>) {
        if let Some(source) = source {
            self.writer.write(" from ");
            self.writer.write(&source.node.binding.node);
            for field in &source.node.fields {
                self.writer.write(".");
                self.writer.write(&field.node);
            }
        }
    }

    /// Format a method declaration, wrapping receiver and parameter lines when the header exceeds the target length.
    pub(super) fn format_method(&mut self, method: &MethodDecl) {
        for dec in &method.decorators {
            self.format_decorator(&dec.node);
        }

        let checkpoint = self.writer.checkpoint();
        self.write_method_prefix(method);
        self.writer.write("(");
        self.format_receiver_and_params(method.receiver.as_ref(), &method.params);
        self.writer.write(")");
        self.format_method_trait_target(method);
        self.writer.write(" -> ");
        self.format_type(&method.return_type.node);
        self.format_result_source(method.result_source.as_ref());

        if method.body.is_none() {
            if self.writer.line_length_exceeded() {
                self.writer.restore(checkpoint);
                self.write_method_prefix(method);
                self.writer.write("(");
                self.format_params_multiline(method.receiver.as_ref(), &method.params);
                self.writer.write(")");
                self.format_method_trait_target(method);
                self.writer.write(" -> ");
                self.format_type(&method.return_type.node);
                self.format_result_source(method.result_source.as_ref());
            }
            self.writer.newline();
            return;
        }

        self.writer.write(":");
        if self.writer.line_length_exceeded() {
            self.writer.restore(checkpoint);
            self.write_method_prefix(method);
            self.writer.write("(");
            self.format_params_multiline(method.receiver.as_ref(), &method.params);
            self.writer.write(")");
            self.format_method_trait_target(method);
            self.writer.write(" -> ");
            self.format_type(&method.return_type.node);
            self.format_result_source(method.result_source.as_ref());
            self.writer.write(":");
        }
        self.writer.newline();
        self.writer.indent();

        if let Some(body) = &method.body {
            let (docstring, body) = self.split_leading_docstring(body);
            if let Some(docstring) = docstring {
                self.format_docstring(docstring);
            }

            if body.is_empty() && docstring.is_none() {
                self.writer.writeln("pass");
            } else {
                for stmt in body {
                    self.format_statement(stmt);
                }
            }
        }

        self.writer.dedent();
    }

    /// Format a computed property declaration.
    pub(super) fn format_property(&mut self, property: &PropertyDecl) {
        self.write_visibility(property.visibility);
        self.writer.write("property ");
        self.writer.write(&property.name);
        self.writer.write(" -> ");
        self.format_type(&property.return_type.node);

        if property.body.is_none() {
            self.writer.newline();
            return;
        }

        self.writer.writeln(":");
        self.writer.indent();

        if let Some(body) = &property.body {
            let (docstring, body) = self.split_leading_docstring(body);
            if let Some(docstring) = docstring {
                self.format_docstring(docstring);
            }

            if body.is_empty() && docstring.is_none() {
                self.writer.writeln("pass");
            } else {
                for stmt in body {
                    self.format_statement(stmt);
                }
            }
        }

        self.writer.dedent();
    }

    /// Write the reusable prefix before a function's opening parameter parenthesis.
    pub(super) fn write_function_prefix(
        &mut self,
        visibility: Visibility,
        is_async: bool,
        name: &str,
        type_params: &[TypeParam],
    ) {
        self.write_visibility(visibility);
        if is_async {
            self.writer.write("async ");
        }
        self.writer.write("def ");
        self.writer.write(name);
        self.format_type_params(type_params);
    }

    /// Write the reusable prefix before a method's opening parameter parenthesis.
    pub(super) fn write_method_prefix(&mut self, method: &MethodDecl) {
        self.write_visibility(method.visibility);
        if method.is_async() {
            self.writer.write("async ");
        }
        self.writer.write("def ");
        self.writer.write(&method.name);
        if !method.type_params.is_empty() {
            self.format_type_params(&method.type_params);
        }
    }

    /// Format the optional `for Trait` target before a method return annotation.
    pub(super) fn format_method_trait_target(&mut self, method: &MethodDecl) {
        if let Some(target) = &method.trait_target {
            self.writer.write(" for ");
            self.format_trait_bound(&target.node);
        }
    }

    /// Format an explicit method receiver.
    pub(super) fn format_receiver(&mut self, receiver: &Receiver) {
        match receiver {
            Receiver::Immutable => self.writer.write("self"),
            Receiver::Mutable => self.writer.write("mut self"),
        }
    }

    /// Format an inline method receiver followed by ordinary parameters.
    pub(super) fn format_receiver_and_params(&mut self, receiver: Option<&Receiver>, params: &[Spanned<Param>]) {
        if let Some(receiver) = receiver {
            self.format_receiver(receiver);
            if !params.is_empty() {
                self.writer.write(", ");
            }
        }
        self.format_params(params);
    }

    /// Format a multiline receiver/parameter list inside an already-opened signature.
    pub(super) fn format_params_multiline(&mut self, receiver: Option<&Receiver>, params: &[Spanned<Param>]) {
        let trailing_commas = self.writer.config().trailing_commas;
        self.writer.newline();
        self.writer.indent();
        if let Some(receiver) = receiver {
            self.format_receiver(receiver);
            if trailing_commas || !params.is_empty() {
                self.writer.write(",");
            }
            self.writer.newline();
        }
        for (i, param) in params.iter().enumerate() {
            self.format_param(&param.node);
            if trailing_commas || i + 1 < params.len() {
                self.writer.write(",");
            }
            self.writer.newline();
        }
        self.writer.dedent();
    }

    // ---- Decorators ----

    /// Format one declaration decorator, preserving `@name` versus `@name()`.
    pub(super) fn format_decorator(&mut self, dec: &Decorator) {
        self.writer.write("@");
        self.format_decorator_path(&dec.path);
        if !dec.type_args.is_empty() {
            self.writer.write("[");
            for (idx, arg) in dec.type_args.iter().enumerate() {
                if idx > 0 {
                    self.writer.write(", ");
                }
                self.format_type(&arg.node);
            }
            self.writer.write("]");
        }
        if dec.is_call {
            self.writer.write("(");
            for (i, arg) in dec.args.iter().enumerate() {
                if i > 0 {
                    self.writer.write(", ");
                }
                match arg {
                    DecoratorArg::Positional(expr) => self.format_spanned_expr(expr),
                    DecoratorArg::Named(name, value) => {
                        self.writer.write(name);
                        self.writer.write("=");
                        match value {
                            DecoratorArgValue::Type(ty) => self.format_type(&ty.node),
                            DecoratorArgValue::Expr(expr) => self.format_spanned_expr(expr),
                        }
                    }
                }
            }
            self.writer.write(")");
        }
        self.writer.newline();
    }

    /// Write a decorator path with the namespace separators accepted for decorator lookup.
    pub(super) fn format_decorator_path(&mut self, path: &ImportPath) {
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

    // ---- Fields and params ----

    /// Render a field with its authored visibility, emitting `pub` only when the field is explicitly public.
    pub(super) fn format_field(&mut self, field: &FieldDecl) {
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
            self.format_spanned_expr(default);
        }
        self.writer.newline();
    }

    /// Write an inline parameter list, preserving each parameter's default expression spelling.
    pub(in crate::formatter) fn format_params(&mut self, params: &[Spanned<Param>]) {
        for (i, param) in params.iter().enumerate() {
            if i > 0 {
                self.writer.write(", ");
            }
            self.format_param(&param.node);
        }
    }

    /// Format one function parameter.
    pub(super) fn format_param(&mut self, param: &Param) {
        if param.is_mut {
            self.writer.write("mut ");
        }
        match param.kind {
            ParamKind::Normal => {}
            ParamKind::RestPositional => self.writer.write("*"),
            ParamKind::RestKeyword => self.writer.write("**"),
        }
        self.writer.write(&param.name);
        self.writer.write(": ");
        self.format_type(&param.ty.node);
        if let Some(default) = &param.default {
            self.writer.write(" = ");
            self.format_spanned_expr(default);
        }
    }

    // ---- Type parameters and trait bounds ----

    /// Write type parameters and their optional bounds in declaration order.
    pub(super) fn format_type_params(&mut self, params: &[TypeParam]) {
        if !params.is_empty() {
            self.writer.write("[");
            for (i, param) in params.iter().enumerate() {
                if i > 0 {
                    self.writer.write(", ");
                }
                self.writer.write(&param.name);
                if !param.bounds.is_empty() {
                    self.writer.write(" with ");
                    if param.bounds.len() == 1 {
                        self.format_trait_bound(&param.bounds[0]);
                    } else {
                        self.writer.write("(");
                        for (j, bound) in param.bounds.iter().enumerate() {
                            if j > 0 {
                                self.writer.write(", ");
                            }
                            self.format_trait_bound(bound);
                        }
                        self.writer.write(")");
                    }
                }
            }
            self.writer.write("]");
        }
    }

    /// Write a trait bound and its type arguments using the parsed bound path.
    pub(super) fn format_trait_bound(&mut self, bound: &TraitBound) {
        self.writer.write(&bound.name);
        if !bound.type_args.is_empty() {
            self.writer.write("[");
            for (i, arg) in bound.type_args.iter().enumerate() {
                if i > 0 {
                    self.writer.write(", ");
                }
                self.format_type(&arg.node);
            }
            self.writer.write("]");
        }
    }

    /// Format a comma-separated trait adoption list on the current line.
    pub(super) fn format_trait_bounds_inline(&mut self, bounds: &[Spanned<TraitBound>]) {
        for (i, bound) in bounds.iter().enumerate() {
            if i > 0 {
                self.writer.write(", ");
            }
            self.format_trait_bound(&bound.node);
        }
    }

    /// Format a parenthesized one-bound-per-line trait adoption list.
    pub(super) fn format_trait_bounds_multiline(&mut self, bounds: &[Spanned<TraitBound>]) {
        let trailing_commas = self.writer.config().trailing_commas;
        self.writer.write("(");
        self.writer.newline();
        self.writer.indent();
        for (i, bound) in bounds.iter().enumerate() {
            self.format_trait_bound(&bound.node);
            if trailing_commas || i + 1 < bounds.len() {
                self.writer.write(",");
            }
            self.writer.newline();
        }
        self.writer.dedent();
        self.writer.write(")");
    }
}
