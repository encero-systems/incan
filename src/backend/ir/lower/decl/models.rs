//! Model declaration lowering.

use super::super::super::decl::{IrStruct, StructField};
use super::super::AstLowering;
use super::super::errors::LoweringError;
use crate::frontend::ast;
use incan_core::lang::derives::{self, DeriveId};

impl AstLowering {
    /// Lower a model declaration to struct.
    pub(in crate::backend::ir::lower) fn lower_model(&mut self, m: &ast::ModelDecl) -> Result<IrStruct, LoweringError> {
        // RFC 021: Register field aliases for alias-aware resolution in expressions.
        self.register_field_aliases(&m.name, &m.fields);

        let mut fields: Vec<StructField> = Vec::new();
        for f in &m.fields {
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
        // Models always get FieldInfo for reflection
        if !derives.contains(&"FieldInfo".to_string()) {
            derives.push("FieldInfo".to_string());
        }
        // Models always get IncanClass for __class__() and __fields__() methods
        if !derives.contains(&"IncanClass".to_string()) {
            derives.push("IncanClass".to_string());
        }

        Ok(IrStruct {
            name: m.name.clone(),
            fields,
            derives,
            visibility: Self::map_visibility(m.visibility),
            type_params: Self::lower_type_params(&m.type_params),
            derive_rust_modules,
            lint_allows: self.extract_rust_lint_allows(&m.decorators),
        })
    }
}
