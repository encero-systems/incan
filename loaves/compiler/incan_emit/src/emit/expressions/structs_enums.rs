//! Emit Rust code for struct constructor expressions.
//!
//! This module handles struct instantiation with both named and positional fields.

use proc_macro2::TokenStream;
use quote::quote;

use super::super::{EmitError, IrEmitter};
use crate::ownership::ValueUseSite;
use incan_ir::expr::TypedExpr;
use incan_ir::types::IrType;
use incan_lang::lang::surface::constructors::{self, ConstructorId};

impl<'a> IrEmitter<'a> {
    /// Emit a struct constructor expression.
    ///
    /// Handles:
    /// - Named field construction: `Point { x: 1, y: 2 }`
    /// - Positional (tuple-style) construction: `Point(1, 2)`
    /// - Empty struct construction: `Unit {}`
    ///
    /// Explicit source type arguments (`Column[T](...)`) are threaded onto the constructed path as a turbofish, and a
    /// struct literal for a type with phantom type parameters initialises the compiler-owned marker field. Both
    /// facts come from lowering; see #1370.
    pub(in super::super) fn emit_struct_expr(
        &self,
        name: &str,
        type_args: &[IrType],
        fields: &[(String, TypedExpr)],
        fill_defaults: bool,
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
            && matches!(inner_expr.kind, incan_ir::expr::IrExprKind::None)
            && let IrType::Option(inner_ty) = &inner_expr.ty
        {
            let n = Self::rust_ident(name);
            let inner_tokens = self.emit_type(inner_ty);
            return Ok(quote! { #n(None::<#inner_tokens>) });
        }

        let n = self.emit_type(&IrType::Struct(name.to_string()));
        let n = if type_args.is_empty() {
            n
        } else {
            let emitted: Vec<TokenStream> = type_args.iter().map(|ty| self.emit_type(ty)).collect();
            quote! { #n::<#(#emitted),*> }
        };
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
            let Some(metadata) = self.struct_constructor_metadata_for_fields(name, fields) else {
                // Unknown or ambiguous struct to the emitter; fall back to emitting only provided fields.
                tracing::debug!(
                    struct_name = %name,
                    ambiguous = self.ambiguous_type_names.contains(name),
                    "struct constructor metadata unavailable, emitting provided fields only"
                );
                let field_tokens: Vec<TokenStream> = fields
                    .iter()
                    .map(|(fname, fval)| {
                        let fn_ident = Self::rust_ident(fname);
                        let fv = self.emit_expr_for_use(fval, ValueUseSite::StructField { target_ty: None })?;
                        Ok(quote! { #fn_ident: #fv })
                    })
                    .collect::<Result<_, EmitError>>()?;
                return if fill_defaults {
                    Ok(quote! { #n { #(#field_tokens,)* ..Default::default() } })
                } else {
                    Ok(quote! { #n { #(#field_tokens),* } })
                };
            };
            let phantom_marker = metadata.phantom_marker_initializer();
            if metadata.fields.is_empty() {
                return Ok(quote! { #n { #phantom_marker } });
            }

            let out_fields = self.emit_named_constructor_arguments(name, metadata, fields)?;

            if metadata.uses_constructor_function() {
                let values = out_fields.iter().map(|(_, value)| value);
                Ok(quote! { #n(#(#values),*) })
            } else {
                let fields = out_fields
                    .iter()
                    .map(|(field, value)| quote! { #field: #value })
                    .chain(phantom_marker);
                Ok(quote! { #n { #(#fields),* } })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use incan_ir::expr::IrExprKind;
    use incan_ir::{FunctionRegistry, IrType};

    #[test]
    fn imported_named_struct_can_emit_metadata_proven_default_fill() -> Result<(), String> {
        let registry = FunctionRegistry::new();
        let emitter = IrEmitter::new(&registry);
        let fields = vec![("count".to_string(), TypedExpr::new(IrExprKind::Int(7), IrType::Int))];

        let rendered = emitter
            .emit_struct_expr("demo::Record", &[], &fields, true)
            .map_err(|error| format!("expected default-filled Rust struct emission, got {error:?}"))?
            .to_string();
        assert_eq!(rendered, "demo :: Record { count : 7 , .. Default :: default () }");
        Ok(())
    }
}
