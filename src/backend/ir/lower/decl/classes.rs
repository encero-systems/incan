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

        let mut derives = self.extract_derives(&c.decorators);

        let debug = derives::as_str(DeriveId::Debug);
        let clone = derives::as_str(DeriveId::Clone);

        // Classes always get Debug and Clone by default
        if !derives.iter().any(|d| d == debug) {
            derives.push(debug.to_string());
        }
        if !derives.iter().any(|d| d == clone) {
            derives.push(clone.to_string());
        }
        // Classes always get FieldInfo for reflection
        if !derives.contains(&"FieldInfo".to_string()) {
            derives.push("FieldInfo".to_string());
        }
        // Classes always get IncanClass for __class__() and __fields__() methods
        if !derives.contains(&"IncanClass".to_string()) {
            derives.push("IncanClass".to_string());
        }

        Ok(IrStruct {
            name: c.name.clone(),
            fields,
            derives,
            visibility: Self::map_visibility(c.visibility),
            type_params: Self::lower_type_params(&c.type_params),
        })
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

            // Then add/override with this class's own methods
            // If a method with the same name exists, remove it first (child overrides parent)
            for m in &class.methods {
                // Remove any existing method with the same name
                methods.retain(|existing| existing.node.name != m.node.name);
                // Add the new method
                methods.push(m.clone());
            }
        }
        Ok(())
    }
}
