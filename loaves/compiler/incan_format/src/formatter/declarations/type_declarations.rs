//! Model, class, capability, trait, enum, alias, partial, type alias and newtype formatting.

use super::*;

impl Formatter {
    // ---- Models, classes, traits ----

    /// Format a model declaration, including field and method alias members.
    pub(super) fn format_model(&mut self, model: &ModelDecl) {
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
                self.writer.write(&trait_name.node.name);
                if !trait_name.node.type_args.is_empty() {
                    self.writer.write("[");
                    for (j, arg) in trait_name.node.type_args.iter().enumerate() {
                        if j > 0 {
                            self.writer.write(", ");
                        }
                        self.format_type(&arg.node);
                    }
                    self.writer.write("]");
                }
            }
        }
        self.writer.writeln(":");
        self.writer.indent();

        if let Some(docstring) = &model.docstring {
            self.format_docstring(docstring);
            if !model.fields.is_empty()
                || !model.method_aliases.is_empty()
                || !model.method_partials.is_empty()
                || !model.properties.is_empty()
                || !model.methods.is_empty()
            {
                self.writer.newline();
            }
        }

        let has_fields = !model.fields.is_empty();
        for field in &model.fields {
            self.format_field(&field.node);
        }
        self.format_method_aliases(&model.method_aliases);
        self.format_method_partials(&model.method_partials);
        if (!model.method_aliases.is_empty() || !model.method_partials.is_empty())
            && (!model.properties.is_empty() || !model.methods.is_empty())
        {
            self.writer.newline();
        }

        let seen_before_properties =
            has_fields || !model.method_aliases.is_empty() || !model.method_partials.is_empty();
        self.format_properties_with_spacing(&model.properties, seen_before_properties);
        self.format_methods_with_spacing(&model.methods, seen_before_properties || !model.properties.is_empty());

        if model.fields.is_empty()
            && model.method_aliases.is_empty()
            && model.method_partials.is_empty()
            && model.properties.is_empty()
            && model.methods.is_empty()
        {
            if model.docstring.is_some() {
                self.writer.newline();
            }
            self.writer.writeln("pass");
        }

        self.writer.dedent();
    }

    /// Format a class declaration, including field and method alias members.
    pub(super) fn format_class(&mut self, class: &ClassDecl) {
        for dec in &class.decorators {
            self.format_decorator(&dec.node);
        }

        let checkpoint = self.writer.checkpoint();
        self.write_class_header(class, false);
        self.writer.write(":");
        if !class.traits.is_empty() && self.writer.line_length_exceeded() {
            self.writer.restore(checkpoint);
            self.write_class_header(class, true);
            self.writer.write(":");
        }

        self.writer.newline();
        self.writer.indent();

        if let Some(docstring) = &class.docstring {
            self.format_docstring(docstring);
            if !class.fields.is_empty()
                || !class.method_aliases.is_empty()
                || !class.method_partials.is_empty()
                || !class.properties.is_empty()
                || !class.methods.is_empty()
            {
                self.writer.newline();
            }
        }

        let has_fields = !class.fields.is_empty();
        for field in &class.fields {
            self.format_field(&field.node);
        }
        self.format_method_aliases(&class.method_aliases);
        self.format_method_partials(&class.method_partials);
        if (!class.method_aliases.is_empty() || !class.method_partials.is_empty())
            && (!class.properties.is_empty() || !class.methods.is_empty())
        {
            self.writer.newline();
        }

        let seen_before_properties =
            has_fields || !class.method_aliases.is_empty() || !class.method_partials.is_empty();
        self.format_properties_with_spacing(&class.properties, seen_before_properties);
        self.format_methods_with_spacing(&class.methods, seen_before_properties || !class.properties.is_empty());

        if class.fields.is_empty()
            && class.method_aliases.is_empty()
            && class.method_partials.is_empty()
            && class.properties.is_empty()
            && class.methods.is_empty()
        {
            if class.docstring.is_some() {
                self.writer.newline();
            }
            self.writer.writeln("pass");
        }

        self.writer.dedent();
    }

    /// Write a class declaration header, optionally wrapping its trait adoption list.
    pub(super) fn write_class_header(&mut self, class: &ClassDecl, multiline_traits: bool) {
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
            if multiline_traits {
                self.format_trait_bounds_multiline(&class.traits);
            } else {
                self.format_trait_bounds_inline(&class.traits);
            }
        }
    }

    /// Format a trait declaration, including same-trait method aliases. Format an RFC 104 `capability` declaration.
    ///
    /// Clause order is normalized to description, scope, then requires, regardless of how the source ordered them:
    /// the parser accepts any order, and a formatter that preserved it would make two identical capabilities format
    /// differently.
    pub(super) fn format_capability(&mut self, cap: &CapabilityDecl) {
        for dec in &cap.decorators {
            self.format_decorator(&dec.node);
        }

        self.write_visibility(cap.visibility);
        self.writer.write("capability ");
        self.writer.write(&cap.name);
        self.writer.writeln(":");
        self.writer.indent();

        if let Some(docstring) = &cap.docstring {
            self.format_docstring(docstring);
        }

        if let Some(description) = &cap.description {
            self.writer.write("description = ");
            self.format_spanned_expr(description);
            self.writer.newline();
        }

        if !cap.scope.is_empty() {
            self.writer.writeln("scope:");
            self.writer.indent();
            for dim in &cap.scope {
                self.writer.write(&dim.node.name);
                self.writer.write(": ");
                self.writer.write(&dim.node.ty.node.to_string());
                self.writer.newline();
            }
            self.writer.dedent();
        }

        if !cap.requires.is_empty() {
            self.writer.write("requires = [");
            for (index, entry) in cap.requires.iter().enumerate() {
                if index > 0 {
                    self.writer.write(", ");
                }
                self.format_spanned_expr(entry);
            }
            self.writer.writeln("]");
        }

        self.writer.dedent();
    }

    /// Write a trait's declarations and methods with member spacing and docstrings.
    pub(super) fn format_trait(&mut self, tr: &TraitDecl) {
        for dec in &tr.decorators {
            self.format_decorator(&dec.node);
        }

        self.write_visibility(tr.visibility);
        self.writer.write("trait ");
        self.writer.write(&tr.name);
        self.format_type_params(&tr.type_params);
        if !tr.traits.is_empty() {
            self.writer.write(" with ");
            for (i, bound) in tr.traits.iter().enumerate() {
                if i > 0 {
                    self.writer.write(", ");
                }
                self.format_trait_bound(&bound.node);
            }
        }
        self.writer.writeln(":");
        self.writer.indent();

        if let Some(docstring) = &tr.docstring {
            self.format_docstring(docstring);
            if !tr.method_aliases.is_empty()
                || !tr.method_partials.is_empty()
                || !tr.properties.is_empty()
                || !tr.methods.is_empty()
            {
                self.writer.newline();
            }
        }

        self.format_method_aliases(&tr.method_aliases);
        self.format_method_partials(&tr.method_partials);
        if (!tr.method_aliases.is_empty() || !tr.method_partials.is_empty())
            && (!tr.properties.is_empty() || !tr.methods.is_empty())
        {
            self.writer.newline();
        }

        let seen_before_properties = !tr.method_aliases.is_empty() || !tr.method_partials.is_empty();
        self.format_properties_with_spacing(&tr.properties, seen_before_properties);
        self.format_methods_with_spacing(&tr.methods, seen_before_properties || !tr.properties.is_empty());

        if tr.method_aliases.is_empty()
            && tr.method_partials.is_empty()
            && tr.properties.is_empty()
            && tr.methods.is_empty()
        {
            if tr.docstring.is_some() {
                self.writer.newline();
            }
            self.writer.writeln("pass");
        }

        self.writer.dedent();
    }

    // ---- Enums and newtypes ----

    /// Format an enum declaration, including optional value-enum backing metadata.
    pub(super) fn format_enum(&mut self, en: &EnumDecl) {
        for dec in &en.decorators {
            self.format_decorator(&dec.node);
        }

        let checkpoint = self.writer.checkpoint();
        self.write_enum_header(en, false);
        self.writer.write(":");
        if !en.traits.is_empty() && self.writer.line_length_exceeded() {
            self.writer.restore(checkpoint);
            self.write_enum_header(en, true);
            self.writer.write(":");
        }
        self.writer.newline();
        self.writer.indent();

        if let Some(docstring) = &en.docstring {
            self.format_docstring(docstring);
            if !en.variants.is_empty() || !en.variant_aliases.is_empty() || !en.methods.is_empty() {
                self.writer.newline();
            }
        }

        for variant in &en.variants {
            self.format_enum_variant(&variant.node);
        }
        for alias in &en.variant_aliases {
            self.format_enum_variant_alias(&alias.node);
        }

        let seen_member_before_methods = !en.variants.is_empty() || !en.variant_aliases.is_empty();
        self.format_methods_with_spacing(&en.methods, seen_member_before_methods);

        if en.variants.is_empty() && en.variant_aliases.is_empty() && en.methods.is_empty() {
            if en.docstring.is_some() {
                self.writer.newline();
            }
            self.writer.writeln("pass");
        }

        self.writer.dedent();
    }

    /// Format one enum variant with optional payload fields and raw value assignment. Write an enum's header line:
    /// visibility, name, type parameters, value-enum carrier, and adopted traits.
    ///
    /// The adoption clause is part of the declaration's meaning, not decoration: `enum Level with Display` and
    /// `enum Level` are different types. Omitting it here silently rewrote the first into the second, so this mirrors
    /// `write_class_header` rather than inventing a second shape (#1401).
    pub(super) fn write_enum_header(&mut self, en: &EnumDecl, multiline_traits: bool) {
        self.write_visibility(en.visibility);
        self.writer.write("enum ");
        self.writer.write(&en.name);
        self.format_type_params(&en.type_params);
        if let Some(value_type) = &en.value_type {
            self.writer.write("(");
            self.format_value_enum_type(value_type.node);
            self.writer.write(")");
        }
        if !en.traits.is_empty() {
            self.writer.write(" with ");
            if multiline_traits {
                self.format_trait_bounds_multiline(&en.traits);
            } else {
                self.format_trait_bounds_inline(&en.traits);
            }
        }
    }

    /// Write an enum variant's name and optional tuple payload types.
    pub(super) fn format_enum_variant(&mut self, variant: &VariantDecl) {
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
        if let Some(value) = &variant.value {
            self.writer.write(" = ");
            self.format_value_enum_literal(&value.node);
        }
        self.writer.newline();
    }

    /// Format one enum variant alias declaration.
    pub(super) fn format_enum_variant_alias(&mut self, alias: &EnumVariantAliasDecl) {
        self.writer.write(&alias.name);
        self.writer.write(" = alias ");
        self.writer.write(&alias.target);
        self.writer.newline();
    }

    /// Format the backing type specifier used by a value enum.
    pub(super) fn format_value_enum_type(&mut self, value_type: ValueEnumType) {
        match value_type {
            ValueEnumType::Str => self.writer.write("str"),
            ValueEnumType::Int => self.writer.write("int"),
        }
    }

    /// Format the raw literal assigned to a value enum variant.
    pub(super) fn format_value_enum_literal(&mut self, value: &ValueEnumLiteral) {
        match value {
            ValueEnumLiteral::Str(value) => {
                self.writer.write("\"");
                self.writer.write(&escape_value_enum_string(value));
                self.writer.write("\"");
            }
            ValueEnumLiteral::Int(value) => self.writer.write(&value.repr),
        }
    }

    /// Format a module-level symbol alias declaration.
    pub(super) fn format_alias(&mut self, alias: &AliasDecl) {
        self.write_visibility(alias.visibility);
        self.writer.write(&alias.name);
        self.writer.write(" = ");
        if alias.explicit_marker {
            self.writer.write("alias ");
        }
        self.writer.write(&alias.target.segments.join("."));
        self.writer.newline();
    }

    /// Format a module-level partial callable preset declaration.
    pub(super) fn format_partial(&mut self, partial: &PartialDecl) {
        self.write_visibility(partial.visibility);
        self.writer.write(&partial.name);
        self.writer.write(" = partial ");
        self.writer.write(&partial.target.segments.join("."));
        self.writer.write("(");
        self.format_partial_args(&partial.args);
        self.writer.write(")");
        self.writer.newline();
    }

    /// Write a type alias and its type parameters without changing its target type.
    pub(super) fn format_type_alias(&mut self, alias: &TypeAliasDecl) {
        self.write_visibility(alias.visibility);
        self.writer.write("type ");
        self.writer.write(&alias.name);
        if !alias.type_params.is_empty() {
            self.format_type_params(&alias.type_params);
        }
        self.writer.write(" = ");
        self.format_type(&alias.target.node);
        self.writer.newline();
    }

    /// Format a newtype or rusttype declaration, emitting each alias through its declaration view once.
    ///
    /// The parser also records aliases as rebindings for checking; that compatibility view is not additional syntax.
    pub(super) fn format_newtype(&mut self, nt: &NewtypeDecl) {
        for dec in &nt.decorators {
            self.format_decorator(&dec.node);
        }

        self.write_visibility(nt.visibility);
        self.writer.write("type ");
        self.writer.write(&nt.name);
        self.format_type_params(&nt.type_params);
        if nt.is_rusttype {
            self.writer.write(" = rusttype ");
        } else {
            self.writer.write(" = newtype ");
        }
        self.format_type(&nt.underlying.node);
        if !nt.traits.is_empty() {
            self.writer.write(" with ");
            for (i, bound) in nt.traits.iter().enumerate() {
                if i > 0 {
                    self.writer.write(", ");
                }
                self.format_trait_bound(&bound.node);
            }
        }

        let has_body = nt.docstring.is_some()
            || !nt.associated_types.is_empty()
            || !nt.method_aliases.is_empty()
            || !nt.method_partials.is_empty()
            || !nt.interop_edges.is_empty()
            || !nt.methods.is_empty();
        if !has_body {
            self.writer.newline();
            return;
        }

        self.writer.writeln(":");
        self.writer.indent();

        if let Some(docstring) = &nt.docstring {
            self.format_docstring(docstring);
            if !nt.associated_types.is_empty()
                || !nt.method_aliases.is_empty()
                || !nt.method_partials.is_empty()
                || !nt.interop_edges.is_empty()
                || !nt.methods.is_empty()
            {
                self.writer.newline();
            }
        }

        for associated_type in &nt.associated_types {
            self.format_associated_type(&associated_type.node);
        }
        if !nt.associated_types.is_empty()
            && (!nt.method_aliases.is_empty() || !nt.interop_edges.is_empty() || !nt.methods.is_empty())
        {
            self.writer.newline();
        }

        self.format_method_aliases(&nt.method_aliases);
        self.format_method_partials(&nt.method_partials);
        if (!nt.method_aliases.is_empty() || !nt.method_partials.is_empty())
            && (!nt.interop_edges.is_empty() || !nt.methods.is_empty())
        {
            self.writer.newline();
        }

        if !nt.interop_edges.is_empty() {
            self.writer.writeln("interop:");
            self.writer.indent();
            for edge in &nt.interop_edges {
                match edge.node.direction {
                    InteropDirection::From => self.writer.write("from "),
                    InteropDirection::Into => self.writer.write("into "),
                }
                self.format_type(&edge.node.ty.node);
                self.writer.write(" ");
                match edge.node.adapter_kind {
                    InteropAdapterKind::Via => self.writer.write("via "),
                    InteropAdapterKind::Try => self.writer.write("try "),
                }
                self.format_spanned_expr(&edge.node.adapter);
                self.writer.newline();
            }
            self.writer.dedent();
            if !nt.methods.is_empty() {
                self.writer.newline();
            }
        }

        self.format_methods_with_spacing(
            &nt.methods,
            !nt.rebindings.is_empty()
                || !nt.method_aliases.is_empty()
                || !nt.method_partials.is_empty()
                || !nt.associated_types.is_empty()
                || !nt.interop_edges.is_empty(),
        );

        self.writer.dedent();
    }

    /// Format a targeted associated type declaration in a newtype/rusttype body.
    pub(super) fn format_associated_type(&mut self, associated_type: &AssociatedTypeDecl) {
        self.writer.write("type ");
        self.writer.write(&associated_type.name);
        self.writer.write(" for ");
        self.format_trait_bound(&associated_type.trait_target.node);
        self.writer.write(" = ");
        self.format_type(&associated_type.ty.node);
        self.writer.newline();
    }
}
