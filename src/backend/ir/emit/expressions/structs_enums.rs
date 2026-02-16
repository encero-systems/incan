//! Emit Rust code for struct constructor expressions.
//!
//! This module handles struct instantiation with both named and positional fields.

use proc_macro2::TokenStream;
use quote::{format_ident, quote};

use incan_core::lang::surface::types::{self as surface_types, SurfaceTypeId};

use super::super::super::conversions::{ConversionContext, determine_conversion};
use super::super::super::expr::TypedExpr;
use super::super::{EmitError, IrEmitter};

impl<'a> IrEmitter<'a> {
    /// Emit a struct constructor expression.
    ///
    /// Handles:
    /// - Named field construction: `Point { x: 1, y: 2 }`
    /// - Positional (tuple-style) construction: `Point(1, 2)`
    /// - Empty struct construction: `Unit {}`
    pub(in super::super) fn emit_struct_expr(
        &self,
        name: &str,
        fields: &[(String, TypedExpr)],
    ) -> Result<TokenStream, EmitError> {
        let n = Self::rust_ident(name);
        let all_named = fields.iter().all(|(fname, _)| !fname.is_empty());

        if !all_named && !fields.is_empty() {
            // Positional (tuple-style) construction
            if matches!(
                surface_types::from_str(name),
                Some(SurfaceTypeId::Json | SurfaceTypeId::Query)
            ) {
                if fields.len() != 1 {
                    return Err(EmitError::Unsupported(format!(
                        "{} expects exactly one positional argument",
                        name
                    )));
                }
                let (_, fval) = &fields[0];
                let emitted = self.emit_expr(fval)?;
                let conversion = determine_conversion(fval, None, ConversionContext::IncanFunctionArg);
                let value = conversion.apply(emitted);
                return Ok(quote! { #n { value: #value } });
            }

            let value_tokens: Vec<TokenStream> = fields
                .iter()
                .map(|(_, fval)| {
                    let emitted = self.emit_expr(fval)?;
                    let conversion = determine_conversion(fval, None, ConversionContext::IncanFunctionArg);
                    Ok(conversion.apply(emitted))
                })
                .collect::<Result<_, EmitError>>()?;
            Ok(quote! { #n(#(#value_tokens),*) })
        } else {
            // Named field construction (including empty constructor calls).
            //
            // Fill omitted fields using declared defaults (if present). If a required field is missing, fail emission
            // (the typechecker should reject this earlier).
            let mut provided: std::collections::HashMap<&str, &TypedExpr> = std::collections::HashMap::new();
            for (fname, fval) in fields {
                if !fname.is_empty() {
                    provided.insert(fname.as_str(), fval);
                }
            }

            let Some(field_names) = self.struct_field_names.get(name) else {
                // Unknown struct to the emitter; fall back to emitting only provided fields.
                // This can occur for cross-crate types or if struct wasn't registered during lowering.
                tracing::debug!(struct_name = %name, "struct field metadata not found, emitting provided fields only");
                if fields.is_empty() {
                    return Ok(quote! { #n {} });
                }
                let field_tokens: Vec<TokenStream> = fields
                    .iter()
                    .map(|(fname, fval)| {
                        let fn_ident = format_ident!("{}", fname);
                        let emitted = self.emit_expr(fval)?;
                        let target_type = self.struct_field_types.get(&(name.to_string(), fname.clone()));
                        let conversion = determine_conversion(fval, target_type, ConversionContext::StructField);
                        let fv = conversion.apply(emitted);
                        Ok(quote! { #fn_ident: #fv })
                    })
                    .collect::<Result<_, EmitError>>()?;
                return Ok(quote! { #n { #(#field_tokens),* } });
            };

            if field_names.is_empty() {
                return Ok(quote! { #n {} });
            }

            let mut out_fields: Vec<TokenStream> = Vec::new();
            for fname in field_names {
                let fn_ident = format_ident!("{}", fname);
                if let Some(fval) = provided.get(fname.as_str()) {
                    let emitted = self.emit_expr(fval)?;
                    let target_type = self.struct_field_types.get(&(name.to_string(), fname.clone()));
                    let conversion = determine_conversion(fval, target_type, ConversionContext::StructField);
                    let fv = conversion.apply(emitted);
                    out_fields.push(quote! { #fn_ident: #fv });
                } else if let Some(default_expr) = self.struct_field_defaults.get(&(name.to_string(), fname.clone())) {
                    let emitted = self.emit_expr(default_expr)?;
                    let target_type = self.struct_field_types.get(&(name.to_string(), fname.clone()));
                    let conversion = determine_conversion(default_expr, target_type, ConversionContext::StructField);
                    let fv = conversion.apply(emitted);
                    out_fields.push(quote! { #fn_ident: #fv });
                } else {
                    return Err(EmitError::Unsupported(format!(
                        "missing required field '{}' when constructing '{}'",
                        fname, name
                    )));
                }
            }

            Ok(quote! { #n { #(#out_fields),* } })
        }
    }
}
