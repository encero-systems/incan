//! Class declaration lowering, including inherited field/method collection.

use super::super::super::decl::{IrStruct, StructField};
use super::super::AstLowering;
use super::super::errors::LoweringError;
use crate::frontend::ast::{self, Spanned};
use incan_core::lang::derives::{self, DeriveId};

impl AstLowering {
    /// Lower a class declaration to struct.
    pub(in crate::backend::ir::lower) fn lower_class(&mut self, c: &ast::ClassDecl) -> Result<IrStruct, LoweringError> {
        let mut fields: Vec<StructField> = Vec::new();

        // If class extends a parent, include parent fields first
        if let Some(parent_name) = &c.extends {
            self.collect_inherited_fields(parent_name, &mut fields)?;
        }

        // Add this class' own fields
        for f in &c.fields {
            let default = f
                .node
                .default
                .as_ref()
                .map(|d| self.lower_expr_spanned(d))
                .transpose()?;
            fields.push(StructField {
                name: f.node.name.clone(),
                ty: self.lower_type(&f.node.ty.node),
                visibility: Self::map_visibility(f.node.visibility),
                default,
                alias: f.node.metadata.alias.clone(),
                description: f.node.metadata.description.clone(),
            });
        }

        let (mut derives, derive_rust_modules) = self.extract_derives(&c.decorators);
        self.extend_derives_with_adopted_serde_traits(&mut derives, &c.traits);

        let debug = derives::as_str(DeriveId::Debug);
        let clone = derives::as_str(DeriveId::Clone);

        // Classes normally get Debug by default. Direct Rust state can be opaque to Debug, so private adapter classes
        // with Rust-import fields opt out of the automatic derive unless the user requested it explicitly.
        let has_opaque_rust_field = c.name.starts_with('_')
            && c.fields
                .iter()
                .any(|f| self.field_uses_direct_rust_import(&f.node.ty.node));
        if !has_opaque_rust_field && !derives.iter().any(|d| d == debug) {
            derives.push(debug.to_string());
        }
        if !derives.iter().any(|d| d == clone) {
            derives.push(clone.to_string());
        }
        // Classes always get FieldInfo for reflection.
        if !derives.iter().any(|d| d == derives::FIELD_INFO_DERIVE_NAME) {
            derives.push(derives::FIELD_INFO_DERIVE_NAME.to_string());
        }
        // Classes always get IncanClass for __class_name__() and __fields__() methods.
        if !derives.iter().any(|d| d == derives::INCAN_CLASS_DERIVE_NAME) {
            derives.push(derives::INCAN_CLASS_DERIVE_NAME.to_string());
        }

        Ok(IrStruct {
            name: c.name.clone(),
            docstring: c.docstring.clone(),
            fields,
            derives,
            visibility: self.map_type_visibility(c.visibility),
            type_params: Self::lower_type_params(&c.type_params),
            derive_rust_modules,
            lint_allows: self.extract_rust_lint_allows(&c.decorators),
        })
    }

    /// Return whether a class field annotation names a direct Rust import.
    fn field_uses_direct_rust_import(&self, ty: &ast::Type) -> bool {
        match ty {
            ast::Type::Simple(name) | ast::Type::ConstrainedPrimitive(name, _) => {
                self.rust_import_aliases.contains_key(name)
            }
            ast::Type::Qualified(segments) => segments
                .first()
                .is_some_and(|name| self.rust_import_aliases.contains_key(name)),
            ast::Type::Generic(base, args) => {
                self.rust_import_aliases.contains_key(base)
                    || args.iter().any(|arg| self.field_uses_direct_rust_import(&arg.node))
            }
            ast::Type::Function(params, ret) => {
                params
                    .iter()
                    .any(|param| self.field_uses_direct_rust_import(&param.node))
                    || self.field_uses_direct_rust_import(&ret.node)
            }
            ast::Type::Ref(inner) | ast::Type::RefMut(inner) => self.field_uses_direct_rust_import(&inner.node),
            ast::Type::Tuple(items) => items.iter().any(|item| self.field_uses_direct_rust_import(&item.node)),
            ast::Type::Unit | ast::Type::SelfType | ast::Type::IntLiteral(_) | ast::Type::Infer => false,
        }
    }

    /// Recursively collect all inherited fields from parent classes.
    pub(in crate::backend::ir::lower) fn collect_inherited_fields(
        &mut self,
        class_name: &str,
        fields: &mut Vec<StructField>,
    ) -> Result<(), LoweringError> {
        // Clone to avoid borrowing `self.class_decls` across recursive calls and expression lowering.
        let parent_class = self.class_decls.get(class_name).cloned();
        if let Some(parent_class) = parent_class {
            // First, collect grandparent fields if any
            if let Some(grandparent_name) = &parent_class.extends {
                self.collect_inherited_fields(grandparent_name, fields)?;
            }

            // Then add parent's own fields
            for f in &parent_class.fields {
                let default = f
                    .node
                    .default
                    .as_ref()
                    .map(|d| self.lower_expr_spanned(d))
                    .transpose()?;
                fields.push(StructField {
                    name: f.node.name.clone(),
                    ty: self.lower_type(&f.node.ty.node),
                    visibility: Self::map_visibility(f.node.visibility),
                    default,
                    alias: f.node.metadata.alias.clone(),
                    description: f.node.metadata.description.clone(),
                });
            }
        }
        Ok(())
    }

    /// Recursively collect all methods from this class and parent classes.
    pub(in crate::backend::ir::lower) fn collect_inherited_methods(
        &self,
        class_name: &str,
        methods: &mut Vec<Spanned<ast::MethodDecl>>,
    ) -> Result<(), LoweringError> {
        if let Some(class) = self.class_decls.get(class_name) {
            // First, collect grandparent methods if any
            if let Some(parent_name) = &class.extends {
                self.collect_inherited_methods(parent_name, methods)?;
            }

            // Then add/override with this class's own methods. Remove inherited methods shadowed by this class, but
            // keep same-name overloads declared together in the class.
            let local_method_names: std::collections::HashSet<&str> =
                class.methods.iter().map(|m| m.node.name.as_str()).collect();
            methods.retain(|existing| !local_method_names.contains(existing.node.name.as_str()));
            methods.extend(class.methods.iter().cloned());
        }
        Ok(())
    }

    /// Recursively collect all computed properties from this class and parent classes.
    pub(in crate::backend::ir::lower) fn collect_inherited_properties(
        &self,
        class_name: &str,
        properties: &mut Vec<Spanned<ast::PropertyDecl>>,
    ) -> Result<(), LoweringError> {
        if let Some(class) = self.class_decls.get(class_name) {
            if let Some(parent_name) = &class.extends {
                self.collect_inherited_properties(parent_name, properties)?;
            }

            for property in &class.properties {
                properties.retain(|existing| existing.node.name != property.node.name);
                properties.push(property.clone());
            }
        }
        Ok(())
    }
}
