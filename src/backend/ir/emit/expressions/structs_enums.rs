//! Emit Rust code for struct constructor expressions.
//!
//! This module handles struct instantiation with both named and positional fields.

use proc_macro2::TokenStream;
use quote::quote;

use super::super::super::expr::TypedExpr;
use super::super::super::ownership::ValueUseSite;
use super::super::super::types::IrType;
use super::super::{EmitError, IrEmitter};
use incan_core::lang::surface::constructors::{self, ConstructorId};

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
        if fields.len() == 1
            && fields.first().is_some_and(|(field_name, _)| field_name.is_empty())
            && let Some(IrType::Result(ok_ty, err_ty)) = self.current_function_return_type.borrow().as_ref()
            && let Some((_, first_arg)) = fields.first()
            && let Some(result) = self.emit_result_constructor_with_context(name, first_arg, ok_ty, err_ty)?
        {
            return Ok(result);
        }

        if name == constructors::as_str(ConstructorId::Some)
            && fields.len() == 1
            && fields.first().is_some_and(|(field_name, _)| field_name.is_empty())
            && let Some((_, inner_expr)) = fields.first()
            && matches!(inner_expr.kind, super::super::super::expr::IrExprKind::None)
            && let IrType::Option(inner_ty) = &inner_expr.ty
        {
            let n = Self::rust_ident(name);
            let inner_tokens = self.emit_type(inner_ty);
            return Ok(quote! { #n(None::<#inner_tokens>) });
        }

        let n = Self::rust_ident(name);
        let all_named = fields.iter().all(|(fname, _)| !fname.is_empty());

        if !all_named && !fields.is_empty() {
            // Positional (tuple-style) construction: emit Type(arg0, arg1, ...)
            let value_tokens: Vec<TokenStream> = fields
                .iter()
                .map(|(_, fval)| {
                    self.emit_expr_for_use(
                        fval,
                        ValueUseSite::IncanCallArg {
                            target_ty: None,
                            callee_param: None,
                            in_return: false,
                        },
                    )
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

            let Some(metadata) = self.struct_constructor_metadata_for_fields(name, fields) else {
                // Unknown or ambiguous struct to the emitter; fall back to emitting only provided fields.
                tracing::debug!(
                    struct_name = %name,
                    ambiguous = self.ambiguous_type_names.contains(name),
                    "struct constructor metadata unavailable, emitting provided fields only"
                );
                if fields.is_empty() {
                    return Ok(quote! { #n {} });
                }
                let field_tokens: Vec<TokenStream> = fields
                    .iter()
                    .map(|(fname, fval)| {
                        let fn_ident = Self::rust_ident(fname);
                        let fv = self.emit_expr_for_use(fval, ValueUseSite::StructField { target_ty: None })?;
                        Ok(quote! { #fn_ident: #fv })
                    })
                    .collect::<Result<_, EmitError>>()?;
                return Ok(quote! { #n { #(#field_tokens),* } });
            };
            let field_names = &metadata.fields;

            if field_names.is_empty() {
                return Ok(quote! { #n {} });
            }

            let mut out_fields: Vec<TokenStream> = Vec::new();
            for fname in field_names {
                let fn_ident = Self::rust_ident(fname);
                let target_type = metadata.field_types.get(fname);
                if let Some(fval) = provided.get(fname.as_str()) {
                    let fv = self.emit_expr_for_use(fval, ValueUseSite::StructField { target_ty: target_type })?;
                    out_fields.push(quote! { #fn_ident: #fv });
                } else if let Some(default_expr) = metadata.field_defaults.get(fname) {
                    let fv =
                        self.emit_expr_for_use(default_expr, ValueUseSite::StructField { target_ty: target_type })?;
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
