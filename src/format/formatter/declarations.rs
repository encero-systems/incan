//! Declaration formatting: imports, models, classes, traits, enums, newtypes, functions, methods, decorators, fields,
//! params, and type params.

use crate::frontend::ast::*;

use super::Formatter;

impl Formatter {
    pub(super) fn format_declaration(&mut self, decl: &Declaration) {
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

    pub(super) fn format_docstring(&mut self, doc: &str) {
        // Trim leading and trailing whitespace from the docstring content to ensure idempotent formatting
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

    // ---- Imports ----

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
                self.format_import_items(items);
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
            ImportKind::RustCrate {
                crate_name,
                path,
                version,
                features,
            } => {
                self.writer.write("import rust::");
                self.writer.write(crate_name);
                for segment in path {
                    self.writer.write("::");
                    self.writer.write(segment);
                }
                self.format_rust_import_spec(version, features);
                if let Some(alias) = &import.alias {
                    self.writer.write(" as ");
                    self.writer.write(alias);
                }
                self.writer.newline();
            }
            ImportKind::RustFrom {
                crate_name,
                path,
                version,
                features,
                items,
            } => {
                self.writer.write("from rust::");
                self.writer.write(crate_name);
                for segment in path {
                    self.writer.write("::");
                    self.writer.write(segment);
                }
                self.format_rust_import_spec(version, features);
                self.writer.write(" import ");
                self.format_import_items(items);
            }
        }
    }

    /// Format a list of import items with line-length-aware wrapping.
    ///
    /// When the full item list fits on the current line (i.e. does not exceed the configured `line_length`), it is
    /// emitted as a bare comma-separated list and the line is closed. When the list would exceed the line limit and
    /// there are at least two items, the list is wrapped in parentheses with one item per indented line, followed by a
    /// trailing comma when `trailing_commas` is enabled in the formatter config.
    ///
    /// # Parameters
    ///
    /// * `items` - The import items to format.
    fn format_import_items(&mut self, items: &[ImportItem]) {
        // Build the single-line representation to check whether it fits.
        let single_line = Self::import_items_single_line(items);

        if items.len() > 1 && self.writer.would_exceed_line_length(single_line.len()) {
            // ---- Multi-line parenthesized form ----
            let trailing_commas = self.writer.config().trailing_commas;
            self.writer.write("(");
            self.writer.newline();
            self.writer.indent();
            for (i, item) in items.iter().enumerate() {
                self.writer.write(&item.name);
                if let Some(alias) = &item.alias {
                    self.writer.write(" as ");
                    self.writer.write(alias);
                }
                if trailing_commas || i + 1 < items.len() {
                    self.writer.write(",");
                }
                self.writer.newline();
            }
            self.writer.dedent();
            self.writer.write(")");
        } else {
            // ---- Single-line form ----
            self.writer.write(&single_line);
        }
        self.writer.newline();
    }

    /// Build a single-line string representation of import items: `Name, Other as alias, ...`.
    fn import_items_single_line(items: &[ImportItem]) -> String {
        items
            .iter()
            .map(|item| {
                if let Some(alias) = &item.alias {
                    format!("{} as {}", item.name, alias)
                } else {
                    item.name.clone()
                }
            })
            .collect::<Vec<_>>()
            .join(", ")
    }

    fn format_rust_import_spec(&mut self, version: &Option<String>, features: &[String]) {
        let Some(version) = version else {
            return;
        };

        self.writer.write(" @ \"");
        self.writer.write(version);
        self.writer.write("\"");

        if !features.is_empty() {
            self.writer.write(" with [");
            for (i, feature) in features.iter().enumerate() {
                if i > 0 {
                    self.writer.write(", ");
                }
                self.writer.write("\"");
                self.writer.write(feature);
                self.writer.write("\"");
            }
            self.writer.write("]");
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

    // ---- Models, classes, traits ----

    fn format_model(&mut self, model: &ModelDecl) {
        for dec in &model.decorators {
            self.format_decorator(&dec.node);
        }

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

        let has_fields = !model.fields.is_empty();
        for field in &model.fields {
            self.format_field(&field.node);
        }

        let mut first_method = true;
        for method in &model.methods {
            if has_fields || !first_method {
                self.writer.newline();
            }
            self.format_method(&method.node);
            first_method = false;
        }

        if model.fields.is_empty() && model.methods.is_empty() {
            self.writer.writeln("pass");
        }

        self.writer.dedent();
    }

    fn format_class(&mut self, class: &ClassDecl) {
        for dec in &class.decorators {
            self.format_decorator(&dec.node);
        }

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

        let has_fields = !class.fields.is_empty();
        for field in &class.fields {
            self.format_field(&field.node);
        }

        let mut first_method = true;
        for method in &class.methods {
            if has_fields || !first_method {
                self.writer.newline();
            }
            self.format_method(&method.node);
            first_method = false;
        }

        if class.fields.is_empty() && class.methods.is_empty() {
            self.writer.writeln("pass");
        }

        self.writer.dedent();
    }

    fn format_trait(&mut self, tr: &TraitDecl) {
        for dec in &tr.decorators {
            self.format_decorator(&dec.node);
        }

        self.write_visibility(tr.visibility);
        self.writer.write("trait ");
        self.writer.write(&tr.name);
        self.format_type_params(&tr.type_params);
        self.writer.writeln(":");
        self.writer.indent();

        let mut first = true;
        for method in &tr.methods {
            if !first {
                self.writer.newline();
            }
            self.format_method(&method.node);
            first = false;
        }

        if tr.methods.is_empty() {
            self.writer.writeln("pass");
        }

        self.writer.dedent();
    }

    // ---- Enums and newtypes ----

    fn format_enum(&mut self, en: &EnumDecl) {
        for dec in &en.decorators {
            self.format_decorator(&dec.node);
        }

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
        self.write_visibility(nt.visibility);
        self.writer.write("type ");
        self.writer.write(&nt.name);
        self.writer.write(" = newtype ");
        self.format_type(&nt.underlying.node);
        self.writer.newline();

        if !nt.methods.is_empty() {
            self.writer.indent();
            for method in &nt.methods {
                self.writer.newline();
                self.format_method(&method.node);
            }
            self.writer.dedent();
        }
    }

    // ---- Functions and methods ----

    fn format_function(&mut self, func: &FunctionDecl) {
        for dec in &func.decorators {
            self.format_decorator(&dec.node);
        }

        self.write_visibility(func.visibility);
        if func.is_async() {
            self.writer.write("async ");
        }
        self.writer.write("def ");
        self.writer.write(&func.name);
        self.format_type_params(&func.type_params);
        self.writer.write("(");
        self.format_params(&func.params);
        self.writer.write(") -> ");
        self.format_type(&func.return_type.node);
        self.writer.writeln(":");

        self.writer.indent();

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
        for dec in &method.decorators {
            self.format_decorator(&dec.node);
        }

        if method.is_async() {
            self.writer.write("async ");
        }
        self.writer.write("def ");
        self.writer.write(&method.name);
        self.writer.write("(");

        let has_receiver = method.receiver.is_some();
        if let Some(receiver) = &method.receiver {
            match receiver {
                Receiver::Immutable => self.writer.write("self"),
                Receiver::Mutable => self.writer.write("mut self"),
            }
        }

        if has_receiver && !method.params.is_empty() {
            self.writer.write(", ");
        }
        self.format_params(&method.params);

        self.writer.write(") -> ");
        self.format_type(&method.return_type.node);

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

    // ---- Decorators ----

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

    // ---- Fields and params ----

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

    pub(super) fn format_params(&mut self, params: &[Spanned<Param>]) {
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

    // ---- Type parameters and trait bounds ----

    fn format_type_params(&mut self, params: &[TypeParam]) {
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

    fn format_trait_bound(&mut self, bound: &TraitBound) {
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
}
