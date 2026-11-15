//! Member spacing, declaration dispatch, consts, statics, test modules, docstrings and imports.

use super::*;

impl Formatter {
    /// Return whether a method declaration owns a formatted body.
    pub(super) fn method_is_body_bearing(method: &MethodDecl) -> bool {
        method.body.is_some()
    }

    /// Return whether a property body should participate in body-bearing member spacing.
    pub(super) fn property_is_body_bearing(property: &PropertyDecl) -> bool {
        property.body.is_some()
    }

    /// Format methods with declaration spacing preserved.
    pub(super) fn format_methods_with_spacing(
        &mut self,
        methods: &[Spanned<MethodDecl>],
        seen_member_before_methods: bool,
    ) {
        let mut seen_member = seen_member_before_methods;
        for method in methods {
            if Self::method_is_body_bearing(&method.node) && seen_member {
                self.writer.blank_lines(RFC053_METHOD_BLANK_LINES);
            }
            self.format_method(&method.node);
            seen_member = true;
        }
    }

    /// Format computed properties with the same body-bearing spacing contract used for methods.
    pub(super) fn format_properties_with_spacing(
        &mut self,
        properties: &[Spanned<PropertyDecl>],
        seen_member_before_properties: bool,
    ) {
        let mut seen_member = seen_member_before_properties;
        for property in properties {
            if Self::property_is_body_bearing(&property.node) && seen_member {
                self.writer.blank_lines(RFC053_METHOD_BLANK_LINES);
            }
            self.format_property(&property.node);
            seen_member = true;
        }
    }

    /// Format same-type method aliases in their declaration form.
    pub(super) fn format_method_aliases(&mut self, aliases: &[Spanned<MethodAliasDecl>]) {
        for alias in aliases {
            self.writer.write(&alias.node.name);
            self.writer.write(" = ");
            if alias.node.explicit_marker {
                self.writer.write("alias ");
            }
            self.writer.write(&alias.node.target);
            self.writer.newline();
        }
    }

    /// Format same-type method partials in their declaration form.
    pub(super) fn format_method_partials(&mut self, partials: &[Spanned<MethodPartialDecl>]) {
        for partial in partials {
            self.writer.write(&partial.node.name);
            self.writer.write(" = partial ");
            self.writer.write(&partial.node.target);
            self.writer.write("(");
            self.format_partial_args(&partial.node.args);
            self.writer.write(")");
            self.writer.newline();
        }
    }

    /// Format one top-level or inline-test declaration.
    pub(in crate::formatter) fn format_declaration(&mut self, decl: &Declaration) {
        match decl {
            Declaration::Import(import) => self.format_import(import),
            Declaration::Const(konst) => self.format_const(konst),
            Declaration::Static(static_decl) => self.format_static(static_decl),
            Declaration::Model(model) => self.format_model(model),
            Declaration::Class(class) => self.format_class(class),
            Declaration::Capability(cap) => self.format_capability(cap),
            Declaration::Trait(tr) => self.format_trait(tr),
            Declaration::Alias(alias) => self.format_alias(alias),
            Declaration::Partial(partial) => self.format_partial(partial),
            Declaration::TypeAlias(alias) => self.format_type_alias(alias),
            Declaration::Newtype(nt) => self.format_newtype(nt),
            Declaration::Enum(en) => self.format_enum(en),
            Declaration::Function(func) => self.format_function(func),
            Declaration::TestModule(test_module) => self.format_test_module(test_module),
            Declaration::VocabBlock(vocab_block) => self.format_vocab_declaration(vocab_block),
            Declaration::Docstring(doc) => self.format_docstring(doc),
        }
    }

    /// Write a constant declaration with its annotation and source-sensitive initializer.
    pub(super) fn format_const(&mut self, konst: &ConstDecl) {
        self.write_visibility(konst.visibility);
        self.writer.write("const ");
        self.writer.write(&konst.name);
        if let Some(ty) = &konst.ty {
            self.writer.write(": ");
            self.format_type(&ty.node);
        }
        self.writer.write(" = ");
        self.format_spanned_expr(&konst.value);
        self.writer.newline();
    }

    /// Write a static declaration with its annotation and source-sensitive initializer.
    pub(super) fn format_static(&mut self, static_decl: &StaticDecl) {
        self.write_visibility(static_decl.visibility);
        self.writer.write("static ");
        self.writer.write(&static_decl.name);
        self.writer.write(": ");
        self.format_type(&static_decl.ty.node);
        self.writer.write(" = ");
        self.format_spanned_expr(&static_decl.value);
        self.writer.newline();
    }

    /// Format an inline test module declaration.
    pub(super) fn format_test_module(&mut self, test_module: &TestModuleDecl) {
        self.writer.write("module ");
        self.writer.write(&test_module.name);
        self.writer.writeln(":");
        self.writer.indent();
        let mut index = 0usize;
        while index < test_module.body.len() {
            let required_features = &test_module.body[index].required_features;
            if required_features.is_empty() {
                self.format_declaration(&test_module.body[index].node);
                index += 1;
                continue;
            }
            let group_end = test_module.body[index..]
                .iter()
                .position(|declaration| declaration.required_features != *required_features)
                .map(|offset| index + offset)
                .unwrap_or(test_module.body.len());
            self.format_feature_condition_header(required_features);
            self.writer.indent();
            for declaration in &test_module.body[index..group_end] {
                self.format_declaration(&declaration.node);
            }
            self.writer.dedent();
            index = group_end;
        }
        if test_module.body.is_empty() {
            self.writer.writeln("pass");
        }
        self.writer.dedent();
    }

    /// Format a docstring while preserving source prose.
    pub(in crate::formatter) fn format_docstring(&mut self, doc: &str) {
        // Trim leading and trailing whitespace from the docstring content to ensure idempotent formatting
        let escaped = doc.trim().replace('\\', "\\\\").replace("\"\"\"", "\\\"\\\"\\\"");
        let trimmed = escaped.as_str();
        if trimmed.is_empty() {
            self.writer.writeln("\"\"\"\"\"\"");
        } else if trimmed.contains('\n') {
            let lines = normalized_docstring_lines(trimmed);

            // Multi-line docstring
            self.writer.writeln("\"\"\"");
            for line in lines {
                if line.is_empty() {
                    self.writer.newline();
                } else {
                    self.writer.writeln(&line);
                }
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

    /// Format one import declaration while preserving its package, source, Python, or Rust namespace.
    pub(super) fn format_import(&mut self, import: &ImportDecl) {
        // Only `pub from ... import ...` can be public; `pub` on other import forms is parser-invalid.
        self.write_visibility(import.visibility);
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
            ImportKind::PubLibrary { library, path } => {
                self.writer.write("import pub::");
                self.writer.write(library);
                for segment in path {
                    self.writer.write(".");
                    self.writer.write(segment);
                }
                if let Some(alias) = &import.alias {
                    self.writer.write(" as ");
                    self.writer.write(alias);
                }
                self.writer.newline();
            }
            ImportKind::PubFrom { library, path, items } => {
                self.writer.write("from pub::");
                self.writer.write(library);
                for segment in path {
                    self.writer.write(".");
                    self.writer.write(segment);
                }
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

    /// Format a top-level vocabulary declaration before the desugaring pass lowers it to ordinary declarations.
    pub(super) fn format_vocab_declaration(&mut self, vocab_block: &VocabBlockStmt) {
        for decorator in &vocab_block.decorators {
            self.writer.write("@");
            self.writer.writeln(&decorator.node.path.segments.join("."));
        }
        self.format_vocab_block_header(vocab_block);
        self.writer.writeln(":");
        self.writer.indent();
        for statement in &vocab_block.body {
            self.format_statement(statement);
        }
        if vocab_block.body.is_empty() {
            self.writer.writeln("pass");
        }
        self.writer.dedent();
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
    pub(super) fn format_import_items(&mut self, items: &[ImportItem]) {
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
    pub(super) fn import_items_single_line(items: &[ImportItem]) -> String {
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

    /// Write the authored Rust crate version and feature selection after an import path.
    pub(super) fn format_rust_import_spec(&mut self, version: &Option<String>, features: &[String]) {
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

    /// Write a module path using its namespace's canonical separators.
    pub(super) fn format_import_path(&mut self, path: &ImportPath) {
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
}
