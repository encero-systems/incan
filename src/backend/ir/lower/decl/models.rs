//! Model declaration lowering.

use super::super::super::decl::{IrStruct, IrStructKind, StructField};
use super::super::AstLowering;
use super::super::errors::LoweringError;
use crate::frontend::ast;
use incan_core::lang::derives::{self, DeriveId};

impl AstLowering {
    /// Lower a model declaration to struct.
    pub(in crate::backend::ir::lower) fn lower_model(&mut self, m: &ast::ModelDecl) -> Result<IrStruct, LoweringError> {
        // RFC 021: Register field aliases for alias-aware resolution in expressions.
        self.register_field_aliases(&m.name, &m.fields);

        let checked_visibilities = self
            .type_info
            .as_ref()
            .and_then(|info| info.declarations.model_field_visibilities.get(&m.name))
            .cloned()
            .ok_or_else(|| LoweringError {
                message: format!(
                    "model `{}` reached lowering without typechecker-owned field visibility",
                    m.name
                ),
                span: Default::default(),
            })?;

        // A field type naming one of the model's own type parameters is a generic, not a nominal type. Lowering it
        // without them in scope resolves `Elem` in `items: list[Elem]` to `IrType::Struct("Elem")`, which every
        // later stage reads as a concrete type that happens not to exist. The call-site seed guard
        // (`is_unresolved_call_seed_type`) then cannot see that it needs substituting, because it treats `Struct`
        // as resolved, and an empty collection literal is emitted as `Vec::<Elem>::new()` in a scope where `Elem`
        // is not bound. See #1507.
        let type_param_names: std::collections::HashSet<&str> =
            m.type_params.iter().map(|param| param.name.as_str()).collect();

        let mut fields: Vec<StructField> = Vec::new();
        for f in &m.fields {
            let visibility = checked_visibilities
                .get(&f.node.name)
                .copied()
                .ok_or_else(|| LoweringError {
                    message: format!(
                        "checked model `{}` has no field visibility for `{}`",
                        m.name, f.node.name
                    ),
                    span: f.span.into(),
                })?;
            let default = f
                .node
                .default
                .as_ref()
                .map(|d| self.lower_expr_spanned(d))
                .transpose()?;
            fields.push(StructField {
                name: f.node.name.clone(),
                ty: self.lower_type_with_type_params(&f.node.ty.node, Some(&type_param_names)),
                surface_type_name: None,
                visibility: Self::map_visibility(visibility),
                is_type_private: self.type_info.as_ref().is_some_and(|info| {
                    info.declarations
                        .model_type_private_fields
                        .contains(&(m.name.clone(), f.node.name.clone()))
                }),
                default,
                alias: f.node.metadata.alias.clone(),
                description: f.node.metadata.description.clone(),
            });
        }

        let (mut derives, derive_rust_modules) = self.extract_derives(&m.decorators);
        self.extend_derives_with_adopted_serde_traits(&mut derives, &m.traits);

        let debug = derives::as_str(DeriveId::Debug);
        let clone = derives::as_str(DeriveId::Clone);

        // Models always get Debug and Clone by default
        if !derives.iter().any(|d| d == debug) {
            derives.push(debug.to_string());
        }
        if !derives.iter().any(|d| d == clone) {
            derives.push(clone.to_string());
        }
        // Models always get FieldInfo for reflection.
        if !derives.iter().any(|d| d == derives::FIELD_INFO_DERIVE_NAME) {
            derives.push(derives::FIELD_INFO_DERIVE_NAME.to_string());
        }
        // Models always get IncanClass for __class_name__() and __fields__() methods.
        if !derives.iter().any(|d| d == derives::INCAN_CLASS_DERIVE_NAME) {
            derives.push(derives::INCAN_CLASS_DERIVE_NAME.to_string());
        }

        Ok(IrStruct {
            kind: IrStructKind::Model,
            name: m.name.clone(),
            docstring: m.docstring.clone(),
            fields,
            derives,
            visibility: self.map_type_visibility(m.visibility),
            type_params: self.lower_type_params(&m.type_params),
            derive_rust_modules,
            lint_allows: self.extract_rust_lint_allows(&m.decorators),
        })
    }
}
