//! Emit Rust code for list and dict comprehensions.
//!
//! This module handles:
//! - List comprehensions: `[expr for var in iter if cond]`
//! - Dict comprehensions: `{key: value for var in iter if cond}`

use proc_macro2::TokenStream;
use quote::{format_ident, quote};

use super::super::super::expr::{IrExprKind, TypedExpr};
use super::super::{EmitError, IrEmitter};

impl<'a> IrEmitter<'a> {
    /// Emit a list comprehension.
    ///
    /// Converts `[expr for var in iter if cond]` to Rust iterator chain:
    /// - Without filter: `iter.iter().cloned().map(|var| expr).collect::<Vec<_>>()`
    /// - With filter over ranges: `iter.filter(|&var| cond).map(|var| expr).collect::<Vec<_>>()`
    /// - With filter over non-range iterables: `iter.iter().filter_map(|var| { let var = (*var).clone(); if cond {
    ///   Some(expr) } else { None } })`
    ///
    /// For range iterables, we skip `.iter().cloned()` since ranges are already iterators.
    pub(in super::super) fn emit_list_comp(
        &self,
        element: &TypedExpr,
        variable: &str,
        iterable: &TypedExpr,
        filter: Option<&TypedExpr>,
    ) -> Result<TokenStream, EmitError> {
        // ---- Context: iterator setup ----
        let iter = self.emit_expr(iterable)?;
        let var_ident = format_ident!("{}", variable);
        let elem = self.emit_expr(element)?;

        let is_range = self.is_range_iterable(iterable);
        let iter_wrapped = quote! { (#iter) };

        // ---- Context: filtered comprehensions ----
        if let Some(filter_expr) = filter {
            let filter_tokens = self.emit_expr(filter_expr)?;
            if is_range {
                Ok(quote! {
                    #iter_wrapped.filter(|&#var_ident| #filter_tokens).map(|#var_ident| #elem).collect::<Vec<_>>()
                })
            } else {
                Ok(quote! {
                    #iter_wrapped
                        .iter()
                        .filter_map(|#var_ident| {
                            let #var_ident = (*#var_ident).clone();
                            if #filter_tokens {
                                Some(#elem)
                            } else {
                                None
                            }
                        })
                        .collect::<Vec<_>>()
                })
            }
        // ---- Context: unfiltered comprehensions ----
        } else if is_range {
            Ok(quote! {
                #iter_wrapped.map(|#var_ident| #elem).collect::<Vec<_>>()
            })
        } else {
            Ok(quote! {
                #iter_wrapped.iter().cloned().map(|#var_ident| #elem).collect::<Vec<_>>()
            })
        }
    }

    /// Emit a dict comprehension.
    ///
    /// Converts `{key: value for var in iter if cond}` to Rust iterator chain:
    /// - Without filter: `iter.iter().cloned().map(|var| (key, value)).collect::<HashMap<_, _>>()`
    /// - With filter over borrowed iterables: `iter.iter().filter_map(|var| { let var = (*var).clone(); if cond {
    ///   Some((key, value)) } else { None } })`
    pub(in super::super) fn emit_dict_comp(
        &self,
        key: &TypedExpr,
        value: &TypedExpr,
        variable: &str,
        iterable: &TypedExpr,
        filter: Option<&TypedExpr>,
    ) -> Result<TokenStream, EmitError> {
        // ---- Context: iterator setup ----
        let iter = self.emit_expr(iterable)?;
        let var_ident = format_ident!("{}", variable);
        let key_tokens = self.emit_expr(key)?;
        let value_tokens = self.emit_expr(value)?;

        // ---- Context: key ownership for collected map entries ----
        // Dict comprehensions build `(key, value)` tuples left-to-right. For non-Copy keys we clone before the tuple so
        // the value expression can still read the loop variable afterward (for example `{name: len(name) for name in
        // names}`).
        let needs_clone = !key.ty.is_copy();
        let cloned_key = if needs_clone {
            quote! { #key_tokens.clone() }
        } else {
            quote! { #key_tokens }
        };

        // ---- Context: filtered vs unfiltered comprehensions ----
        if let Some(filter_expr) = filter {
            let filter_tokens = self.emit_expr(filter_expr)?;
            Ok(quote! {
                #iter
                    .iter()
                    .filter_map(|#var_ident| {
                        let #var_ident = (*#var_ident).clone();
                        if #filter_tokens {
                            Some((#cloned_key, #value_tokens))
                        } else {
                            None
                        }
                    })
                    .collect::<std::collections::HashMap<_, _>>()
            })
        } else {
            Ok(quote! {
                #iter.iter().cloned().map(|#var_ident| (#cloned_key, #value_tokens)).collect::<std::collections::HashMap<_, _>>()
            })
        }
    }

    /// Check if an iterable expression is a range (which doesn't need `.iter().cloned()`).
    fn is_range_iterable(&self, iterable: &TypedExpr) -> bool {
        matches!(&iterable.kind, IrExprKind::Range { .. })
            || matches!(&iterable.kind, IrExprKind::Call { func, .. }
                if matches!(&func.kind, IrExprKind::Var { name, .. }
                    if incan_core::lang::builtins::from_str(name.as_str())
                        == Some(incan_core::lang::builtins::BuiltinFnId::Range)))
            || matches!(&iterable.kind, IrExprKind::BuiltinCall { func, .. }
                if matches!(func, super::super::super::expr::BuiltinFn::Range))
    }
}
