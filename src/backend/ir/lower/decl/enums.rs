//! Enum declaration lowering.

use super::super::super::decl::{EnumVariant, IrEnum, IrEnumValue, IrEnumValueType, VariantFields};
use super::super::super::types::IrType;
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
        let variants: Vec<EnumVariant> = e
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

        // Enums always get Debug and Clone by default. Enums also get PartialEq when their payloads are structurally
        // comparable by default. Payloads that name ordinary models/classes need explicit equality adoption because
        // Rust's derived PartialEq requires every payload type to be comparable, and many payload enums are used only
        // for pattern matching.
        let debug = derives::as_str(DeriveId::Debug);
        let clone = derives::as_str(DeriveId::Clone);
        let partial_eq = derives::as_str(DeriveId::PartialEq);
        let can_default_partial_eq = variants.iter().all(enum_variant_payloads_default_partial_eq);
        if !derives.iter().any(|d| d == debug) {
            derives.push(debug.to_string());
        }
        if !derives.iter().any(|d| d == clone) {
            derives.push(clone.to_string());
        }
        if can_default_partial_eq && !derives.iter().any(|d| d == partial_eq) {
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

/// Return whether a variant can participate in the enum's implicit `PartialEq` derive.
fn enum_variant_payloads_default_partial_eq(variant: &EnumVariant) -> bool {
    match &variant.fields {
        VariantFields::Unit => true,
        VariantFields::Tuple(types) => types.iter().all(type_defaults_partial_eq),
        VariantFields::Struct(fields) => fields.iter().all(|field| type_defaults_partial_eq(&field.ty)),
    }
}

/// Return whether a type's generated Rust representation has equality without an explicit Incan equality derive.
fn type_defaults_partial_eq(ty: &IrType) -> bool {
    match ty {
        IrType::Unit
        | IrType::Bool
        | IrType::Int
        | IrType::Float
        | IrType::String
        | IrType::Bytes
        | IrType::StaticStr
        | IrType::StaticBytes
        | IrType::FrozenStr
        | IrType::FrozenBytes
        | IrType::StrRef => true,
        IrType::List(inner) | IrType::Set(inner) | IrType::Option(inner) => type_defaults_partial_eq(inner),
        IrType::Dict(key, value) | IrType::Result(key, value) => {
            type_defaults_partial_eq(key) && type_defaults_partial_eq(value)
        }
        IrType::Tuple(items) => items.iter().all(type_defaults_partial_eq),
        IrType::Ref(inner) | IrType::RefMut(inner) => type_defaults_partial_eq(inner),
        IrType::Struct(_)
        | IrType::Enum(_)
        | IrType::Trait(_)
        | IrType::NamedGeneric(_, _)
        | IrType::ImplTrait(_)
        | IrType::Function { .. }
        | IrType::Generic(_)
        | IrType::SelfType
        | IrType::Unknown => false,
    }
}
