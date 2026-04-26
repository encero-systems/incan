//! Enum declaration lowering.

use super::super::super::decl::{EnumVariant, IrEnum, IrEnumValue, IrEnumValueType, VariantFields};
use super::super::AstLowering;
use super::super::errors::LoweringError;
use crate::frontend::ast;
use incan_core::lang::derives::{self, DeriveId};

impl AstLowering {
    /// Lower an enum declaration.
    pub(in crate::backend::ir::lower) fn lower_enum(&mut self, e: &ast::EnumDecl) -> Result<IrEnum, LoweringError> {
        let value_type = e.value_type.as_ref().map(|ty| match ty.node {
            ast::ValueEnumType::Str => IrEnumValueType::String,
            ast::ValueEnumType::Int => IrEnumValueType::Int,
        });
        let variants = e
            .variants
            .iter()
            .map(|v| {
                let fields = if v.node.fields.is_empty() {
                    VariantFields::Unit
                } else {
                    VariantFields::Tuple(v.node.fields.iter().map(|t| self.lower_type(&t.node)).collect())
                };
                let raw_value = v.node.value.as_ref().map(|value| match &value.node {
                    ast::ValueEnumLiteral::Str(raw) => IrEnumValue::String(raw.clone()),
                    ast::ValueEnumLiteral::Int(raw) => IrEnumValue::Int(raw.value),
                });
                EnumVariant {
                    name: v.node.name.clone(),
                    fields,
                    raw_value,
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
            value_type,
            derives,
            visibility: Self::map_visibility(e.visibility),
            type_params: Self::lower_type_params(&e.type_params),
            derive_rust_modules,
            lint_allows: self.extract_rust_lint_allows(&e.decorators),
        })
    }
}
