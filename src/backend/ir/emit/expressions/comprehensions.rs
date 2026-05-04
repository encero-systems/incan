//! Emit Rust code for list and dict comprehensions.
//!
//! This module handles:
//! - List comprehensions: `[expr for var in iter if cond]`
//! - Dict comprehensions: `{key: value for var in iter if cond}`

use proc_macro2::TokenStream;
use quote::quote;

use super::super::super::expr::{BuiltinFn, IrExprKind, IrGeneratorClause, Pattern, TypedExpr};
use super::super::super::ownership::{
    ComprehensionIterationPlan, dict_comprehension_key_needs_clone, plan_dict_comprehension_iteration,
    plan_list_comprehension_iteration,
};
use super::super::super::types::IrType;
use super::super::{EmitError, IrEmitter};

impl<'a> IrEmitter<'a> {
    /// Emit a lazy RFC 006 generator expression.
    pub(in super::super) fn emit_generator_expr(
        &self,
        element: &TypedExpr,
        clauses: &[IrGeneratorClause],
    ) -> Result<TokenStream, EmitError> {
        let chain = self.emit_generator_chain(element, clauses)?;
        Ok(quote! { incan_stdlib::iter::Generator::new(#chain) })
    }

    /// Recursively emit source-ordered generator `for` / `if` clauses as lazy iterator adapters.
    fn emit_generator_chain(
        &self,
        element: &TypedExpr,
        clauses: &[IrGeneratorClause],
    ) -> Result<TokenStream, EmitError> {
        let Some((head, tail)) = clauses.split_first() else {
            let elem = self.emit_expr(element)?;
            return Ok(quote! { std::iter::once(#elem) });
        };

        match head {
            IrGeneratorClause::For { pattern, iterable } => {
                let pattern_tokens = self.emit_pattern(pattern);
                let iter = self.emit_generator_iterable(iterable)?;
                let body = self.emit_generator_chain(element, tail)?;
                Ok(quote! {
                    (#iter).flat_map(move |#pattern_tokens| {
                        incan_stdlib::iter::Generator::new(#body)
                    })
                })
            }
            IrGeneratorClause::If(condition) => {
                let condition_tokens = self.emit_expr(condition)?;
                let body = self.emit_generator_chain(element, tail)?;
                Ok(quote! {
                    if #condition_tokens {
                        incan_stdlib::iter::Generator::new(#body)
                    } else {
                        incan_stdlib::iter::Generator::new(std::iter::empty())
                    }
                })
            }
        }
    }

    /// Emit the owned iterator expression consumed by one generator `for` clause.
    fn emit_generator_iterable(&self, iterable: &TypedExpr) -> Result<TokenStream, EmitError> {
        match &iterable.kind {
            IrExprKind::BuiltinCall {
                func: BuiltinFn::Enumerate,
                args,
            } => {
                let iter = if let Some(arg) = args.first() {
                    let arg_tokens = self.emit_expr(arg)?;
                    quote! { (#arg_tokens).clone().into_iter().enumerate().map(|(idx, value)| (idx as i64, value)) }
                } else {
                    quote! { std::iter::empty::<(i64, ())>() }
                };
                Ok(iter)
            }
            _ if self.is_range_iterable(iterable) || Self::is_generator_iterable(iterable) => self.emit_expr(iterable),
            _ => {
                let iter = self.emit_expr(iterable)?;
                Ok(quote! { (#iter).clone().into_iter() })
            }
        }
    }

    /// Return whether an iterable expression already yields owned generator items.
    fn is_generator_iterable(iterable: &TypedExpr) -> bool {
        matches!(&iterable.ty, IrType::NamedGeneric(name, _)
            if incan_core::lang::types::collections::from_str(name.as_str())
                == Some(incan_core::lang::types::collections::CollectionTypeId::Generator))
    }

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
        pattern: &Pattern,
        iterable: &TypedExpr,
        filter: Option<&TypedExpr>,
    ) -> Result<TokenStream, EmitError> {
        // ---- Context: iterator setup ----
        let pattern_tokens = self.emit_pattern(pattern);
        let elem = self.emit_expr(element)?;

        if let Some(iter) = self.emit_direct_comprehension_iterable(iterable)? {
            return self.emit_direct_list_comp(iter, pattern_tokens, elem, filter);
        }

        let iter = self.emit_expr(iterable)?;
        let is_range = self.is_range_iterable(iterable);
        let iter_wrapped = quote! { (#iter) };

        match plan_list_comprehension_iteration(is_range, filter.is_some()) {
            ComprehensionIterationPlan::RangeFilter => {
                let Some(filter) = filter else {
                    return Err(EmitError::Unsupported(
                        "internal error: range comprehension filter plan requires a filter".to_string(),
                    ));
                };
                let filter_tokens = self.emit_expr(filter)?;
                Ok(quote! {
                    #iter_wrapped.filter(|&#pattern_tokens| #filter_tokens).map(|#pattern_tokens| #elem).collect::<Vec<_>>()
                })
            }
            ComprehensionIterationPlan::RangeDirect => Ok(quote! {
                #iter_wrapped.map(|#pattern_tokens| #elem).collect::<Vec<_>>()
            }),
            ComprehensionIterationPlan::FilterMapCloneBinding => {
                let Some(filter) = filter else {
                    return Err(EmitError::Unsupported(
                        "internal error: filtered comprehension plan requires a filter".to_string(),
                    ));
                };
                let filter_tokens = self.emit_expr(filter)?;
                let item_binding = Self::filter_map_item_binding(pattern, &pattern_tokens);
                Ok(quote! {
                    #iter_wrapped
                        .iter()
                        .filter_map(|#item_binding| {
                            let #pattern_tokens = (*#item_binding).clone();
                            if #filter_tokens {
                                Some(#elem)
                            } else {
                                None
                            }
                        })
                        .collect::<Vec<_>>()
                })
            }
            ComprehensionIterationPlan::IterCloned => Ok(quote! {
                #iter_wrapped.iter().cloned().map(|#pattern_tokens| #elem).collect::<Vec<_>>()
            }),
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
        pattern: &Pattern,
        iterable: &TypedExpr,
        filter: Option<&TypedExpr>,
    ) -> Result<TokenStream, EmitError> {
        // ---- Context: iterator setup ----
        let pattern_tokens = self.emit_pattern(pattern);
        let key_tokens = self.emit_expr(key)?;
        let value_tokens = self.emit_expr(value)?;

        // ---- Context: key ownership for collected map entries ----
        // Dict comprehensions build `(key, value)` tuples left-to-right. For non-Copy keys we clone before the tuple so
        // the value expression can still read the loop variable afterward (for example `{name: len(name) for name in
        // names}`).
        let needs_clone = dict_comprehension_key_needs_clone(&key.ty);
        let cloned_key = if needs_clone {
            quote! { #key_tokens.clone() }
        } else {
            quote! { #key_tokens }
        };

        if let Some(iter) = self.emit_direct_comprehension_iterable(iterable)? {
            return self.emit_direct_dict_comp(iter, pattern_tokens, cloned_key, value_tokens, filter);
        }

        let iter = self.emit_expr(iterable)?;
        match plan_dict_comprehension_iteration(filter.is_some()) {
            ComprehensionIterationPlan::FilterMapCloneBinding => {
                let Some(filter) = filter else {
                    return Err(EmitError::Unsupported(
                        "internal error: filtered dict comprehension plan requires a filter".to_string(),
                    ));
                };
                let filter_tokens = self.emit_expr(filter)?;
                let item_binding = Self::filter_map_item_binding(pattern, &pattern_tokens);
                Ok(quote! {
                    #iter
                        .iter()
                        .filter_map(|#item_binding| {
                            let #pattern_tokens = (*#item_binding).clone();
                            if #filter_tokens {
                                Some((#cloned_key, #value_tokens))
                            } else {
                                None
                            }
                        })
                        .collect::<std::collections::HashMap<_, _>>()
                })
            }
            ComprehensionIterationPlan::IterCloned => Ok(quote! {
                #iter.iter().cloned().map(|#pattern_tokens| (#cloned_key, #value_tokens)).collect::<std::collections::HashMap<_, _>>()
            }),
            ComprehensionIterationPlan::RangeDirect | ComprehensionIterationPlan::RangeFilter => {
                unreachable!("dict comprehensions do not use range-specific iteration plans")
            }
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

    /// Choose the borrowed closure binding used before cloning a filtered non-range comprehension item.
    fn filter_map_item_binding(pattern: &Pattern, pattern_tokens: &TokenStream) -> TokenStream {
        if matches!(pattern, Pattern::Var(_)) {
            pattern_tokens.clone()
        } else {
            quote! { __incan_comp_item }
        }
    }

    /// Emit iterable expressions that already produce iterator items with the ownership shape a comprehension expects.
    fn emit_direct_comprehension_iterable(&self, iterable: &TypedExpr) -> Result<Option<TokenStream>, EmitError> {
        match &iterable.kind {
            IrExprKind::BuiltinCall {
                func: BuiltinFn::Enumerate,
                args,
            } => self.emit_owned_enumerate_iter(args).map(Some),
            _ => Ok(None),
        }
    }

    /// Emit `enumerate(xs)` for comprehension closures, cloning values to match the typechecker's owned tuple item
    /// type.
    fn emit_owned_enumerate_iter(&self, args: &[TypedExpr]) -> Result<TokenStream, EmitError> {
        if let Some(arg) = args.first() {
            let a = self.emit_expr(arg)?;
            Ok(quote! {
                #a.iter().enumerate().map(|(idx, value)| (idx as i64, value.clone()))
            })
        } else {
            Ok(quote! { std::iter::empty::<(i64, ())>() })
        }
    }

    /// Emit a comprehension over an iterable expression that already returns owned values for closure binding.
    fn emit_direct_list_comp(
        &self,
        iter: TokenStream,
        pattern: TokenStream,
        elem: TokenStream,
        filter: Option<&TypedExpr>,
    ) -> Result<TokenStream, EmitError> {
        if let Some(filter) = filter {
            let filter_tokens = self.emit_expr(filter)?;
            Ok(quote! {
                (#iter)
                    .filter_map(|#pattern| {
                        if #filter_tokens {
                            Some(#elem)
                        } else {
                            None
                        }
                    })
                    .collect::<Vec<_>>()
            })
        } else {
            Ok(quote! { (#iter).map(|#pattern| #elem).collect::<Vec<_>>() })
        }
    }

    /// Emit a dict comprehension over an iterable expression that already returns owned values for closure binding.
    fn emit_direct_dict_comp(
        &self,
        iter: TokenStream,
        pattern: TokenStream,
        key: TokenStream,
        value: TokenStream,
        filter: Option<&TypedExpr>,
    ) -> Result<TokenStream, EmitError> {
        if let Some(filter) = filter {
            let filter_tokens = self.emit_expr(filter)?;
            Ok(quote! {
                (#iter)
                    .filter_map(|#pattern| {
                        if #filter_tokens {
                            Some((#key, #value))
                        } else {
                            None
                        }
                    })
                    .collect::<std::collections::HashMap<_, _>>()
            })
        } else {
            Ok(quote! { (#iter).map(|#pattern| (#key, #value)).collect::<std::collections::HashMap<_, _>>() })
        }
    }
}
