//! Enum declaration lowering.

use super::super::super::decl::{EnumVariant, IrEnum, VariantFields};
use super::super::AstLowering;
use super::super::errors::LoweringError;
use crate::frontend::ast;
use incan_core::lang::derives::{self, DeriveId};

impl AstLowering {
    /// Lower an enum declaration.
    pub(in crate::backend::ir::lower) fn lower_enum(&mut self, e: &ast::EnumDecl) -> Result<IrEnum, LoweringError> {
        let variants = e
            .variants
            .iter()
            .map(|v| {
                let fields = if v.node.fields.is_empty() {
                    VariantFields::Unit
                } else {
                    VariantFields::Tuple(v.node.fields.iter().map(|t| self.lower_type(&t.node)).collect())
                };
                EnumVariant {
                    name: v.node.name.clone(),
                    fields,
                }
            })
            .collect();

        // Extract user-specified derives from decorators
        let (mut derives, derive_rust_modules) = self.extract_derives(&e.decorators);

        // Enums always get Debug, Clone, PartialEq by default (if not already specified)
        let debug = derives::as_str(DeriveId::Debug);
        let clone = derives::as_str(DeriveId::Clone);
        let partial_eq = derives::as_str(DeriveId::PartialEq);
        if !derives.iter().any(|d| d == debug) {
            derives.push(debug.to_string());
        }
        if !derives.iter().any(|d| d == clone) {
            derives.push(clone.to_string());
        }
        if !derives.iter().any(|d| d == partial_eq) {
            derives.push(partial_eq.to_string());
        }

        Ok(IrEnum {
            name: e.name.clone(),
            variants,
            derives,
            visibility: Self::map_visibility(e.visibility),
            type_params: Self::lower_type_params(&e.type_params),
            derive_rust_modules,
            lint_allows: self.extract_rust_lint_allows(&e.decorators),
        })
    }
}
