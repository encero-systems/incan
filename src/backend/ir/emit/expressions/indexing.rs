//! Emit Rust code for index, slice, and field access expressions.
//!
//! This module handles:
//! - Index expressions (`list[i]`, `dict[k]`)
//! - Slice expressions (`list[start:end]`)
//! - Field access expressions (`obj.field`)
//!
//! ## Negative index handling
//!
//! Python-style negative indices are converted to `len() - offset` at emit time.
//! This logic is shared across index expressions, lvalue emission, and assignment targets.

use proc_macro2::TokenStream;
use quote::{format_ident, quote};

use super::super::super::expr::{IrExprKind, TypedExpr, UnaryOp, VarRefKind};
use super::super::super::types::IrType;
use super::super::{EmitError, IrEmitter};

impl<'a> IrEmitter<'a> {
    /// Emit an index expression.
    ///
    /// Handles `list[i]` and `dict[k]` access with:
    /// - Negative index conversion (Python-style)
    /// - Clone insertion for non-Copy types
    /// - Type-aware bracket vs method access
    pub(in super::super) fn emit_index_expr(
        &self,
        object: &TypedExpr,
        index: &TypedExpr,
    ) -> Result<TokenStream, EmitError> {
        let o = self.emit_expr(object)?;
        let obj_ty = match &object.ty {
            IrType::Ref(inner) | IrType::RefMut(inner) => inner.as_ref(),
            other => other,
        };

        // Strings: delegate to stdlib helper (Unicode-scalar indexing with bounds/negative support).
        if matches!(obj_ty, IrType::String | IrType::FrozenStr) {
            let idx_tokens = self.emit_expr(index)?;
            return Ok(quote! { incan_stdlib::strings::str_index(&#o, (#idx_tokens) as i64) });
        }

        match obj_ty {
            IrType::Dict(_, v) => {
                let i = self.emit_expr(index)?;
                if v.is_copy() {
                    Ok(quote! { *incan_stdlib::collections::dict_get(&#o, &#i) })
                } else {
                    Ok(quote! { incan_stdlib::collections::dict_get(&#o, &#i).clone() })
                }
            }
            IrType::List(elem) => {
                let idx_tokens = self.emit_expr(index)?;
                let idx_i64 = quote! { (#idx_tokens) as i64 };
                if elem.is_copy() {
                    Ok(quote! { *incan_stdlib::collections::list_get(&#o, #idx_i64) })
                } else {
                    Ok(quote! { incan_stdlib::collections::list_get(&#o, #idx_i64).clone() })
                }
            }
            // Fallback for unknown/unsupported index targets.
            //
            // We keep this path total so codegen can continue even when the IR type is not a known container.
            // This may panic with Rust-native messages for invalid indices; we will tighten this as more container
            // types are routed through typed stdlib helpers.
            _ => {
                let index_expr = self.emit_index_with_negative_handling(object, index, &o)?;
                match obj_ty {
                    IrType::Unknown => Ok(quote! { #o[#index_expr].clone() }),
                    _ => Ok(quote! { #o[#index_expr] }),
                }
            }
        }
    }

    /// Emit a slice expression.
    ///
    /// Handles `list[start:end]` → `list[start..end].to_vec()`.
    pub(in super::super) fn emit_slice_expr(
        &self,
        target: &TypedExpr,
        start: &Option<Box<TypedExpr>>,
        end: &Option<Box<TypedExpr>>,
        step: &Option<Box<TypedExpr>>,
    ) -> Result<TokenStream, EmitError> {
        let t_raw = self.emit_expr(target)?;

        // Distinguish string vs list slices to honor Unicode-scalar policy for strings.
        let obj_ty = match &target.ty {
            IrType::Ref(inner) | IrType::RefMut(inner) => inner.as_ref(),
            other => other,
        };

        if matches!(obj_ty, IrType::String | IrType::FrozenStr) {
            // Strings: delegate to stdlib, which calls into incan_core for policy/alignment.
            let s_tokens = quote! { #t_raw };
            let start_expr = if let Some(s) = start {
                let s_tokens = self.emit_expr(s)?;
                quote! { Some((#s_tokens) as i64) }
            } else {
                quote! { None }
            };
            let end_expr = if let Some(e) = end {
                let e_tokens = self.emit_expr(e)?;
                quote! { Some((#e_tokens) as i64) }
            } else {
                quote! { None }
            };
            let step_expr = if let Some(st) = step {
                let st_tokens = self.emit_expr(st)?;
                quote! { Some((#st_tokens) as i64) }
            } else {
                quote! { None }
            };
            return Ok(quote! { incan_stdlib::strings::str_slice(&#s_tokens, #start_expr, #end_expr, #step_expr) });
        }

        // Lists/other: use stdlib helper for Python-like semantics (negative indices, clamping, step, step==0 error).
        let start_expr = if let Some(s) = start {
            let s_tokens = self.emit_expr(s)?;
            quote! { Some((#s_tokens) as i64) }
        } else {
            quote! { None }
        };

        let end_expr = if let Some(e) = end {
            let e_tokens = self.emit_expr(e)?;
            quote! { Some((#e_tokens) as i64) }
        } else {
            quote! { None }
        };

        let step_expr = if let Some(st) = step {
            let st_tokens = self.emit_expr(st)?;
            quote! { Some((#st_tokens) as i64) }
        } else {
            quote! { None }
        };

        Ok(quote! { incan_stdlib::collections::list_slice(&#t_raw, #start_expr, #end_expr, #step_expr) })
    }

    /// Emit a field access expression.
    ///
    /// Handles:
    /// - Enum variant access (`Type.Variant` → `Type::Variant`)
    /// - Tuple field access (`tuple.0` → `tuple.0`)
    /// - Regular struct field access (`obj.field` → `obj.field`)
    pub(in super::super) fn emit_field_expr(&self, object: &TypedExpr, field: &str) -> Result<TokenStream, EmitError> {
        let o = self.emit_expr(object)?;

        // Check if this is an enum variant access using the actual enum registry, not capitalization heuristics
        if let IrExprKind::Var { name, ref_kind, .. } = &object.kind {
            let key = (name.to_string(), field.to_string());
            if self.enum_variant_fields.contains_key(&key) {
                let type_ident = format_ident!("{}", name);
                let f = format_ident!("{}", field);
                return Ok(quote! { #type_ident::#f });
            }
            if matches!(ref_kind, VarRefKind::TypeName | VarRefKind::ExternalName) {
                let type_ident = format_ident!("{}", name);
                let f = format_ident!("{}", field);
                return Ok(quote! { #type_ident::#f });
            }
        }

        // Check if field is a numeric index (tuple access)
        if field.chars().all(|c| c.is_ascii_digit()) {
            let idx: syn::Index = field
                .parse::<usize>()
                .map(syn::Index::from)
                .unwrap_or_else(|_| syn::Index::from(0));
            Ok(quote! { #o.#idx })
        } else {
            let f = format_ident!("{}", field);
            Ok(quote! { #o.#f })
        }
    }

    /// Helper: emit an index expression with negative-index handling.
    ///
    /// Converts Python-style negative indices to `len() - offset`.
    /// This helper is used by both `emit_index_expr` and lvalue emission.
    pub(in super::super) fn emit_index_with_negative_handling(
        &self,
        _object: &TypedExpr,
        index: &TypedExpr,
        obj_tokens: &TokenStream,
    ) -> Result<TokenStream, EmitError> {
        match &index.kind {
            IrExprKind::Int(n) if *n < 0 => {
                let offset = n.abs();
                Ok(quote! { #obj_tokens.len() - #offset })
            }
            IrExprKind::UnaryOp {
                op: UnaryOp::Neg,
                operand,
            } => {
                if let IrExprKind::Int(n) = &operand.kind {
                    Ok(quote! { #obj_tokens.len() - #n })
                } else {
                    let i = self.emit_expr(operand)?;
                    Ok(quote! { #obj_tokens.len() - (#i) as usize })
                }
            }
            _ => {
                let i = self.emit_expr(index)?;
                match &index.ty {
                    IrType::Int | IrType::Unknown => Ok(quote! { (#i) as usize }),
                    _ => Ok(i),
                }
            }
        }
    }
}
