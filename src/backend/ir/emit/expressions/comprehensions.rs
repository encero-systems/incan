//! Emit Rust code for comprehensions and generator expressions.
//!
//! This module handles:
//! - List comprehensions: `[expr for var in iter if cond]`
//! - Dict comprehensions: `{key: value for var in iter if cond}`
//! - Generator expressions: `(expr for var in iter if cond)`

use proc_macro2::TokenStream;
use quote::quote;

use super::super::super::expr::{
    BuiltinFn, FormatPart, IrCallArg, IrDictEntry, IrExprKind, IrGeneratorClause, IrListEntry, Pattern, TypedExpr,
};
use super::super::super::ownership::{
    ComprehensionIterationPlan, dict_comprehension_key_needs_clone, plan_dict_comprehension_iteration,
    plan_list_comprehension_iteration, plan_owned_iterator_source,
};
use super::super::super::stmt::{AssignTarget, IrStmt, IrStmtKind};
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
                    self.emit_enumerate_iter(arg)?
                } else {
                    quote! { std::iter::empty::<(i64, ())>() }
                };
                Ok(iter)
            }
            _ if self.is_range_iterable(iterable) || Self::is_generator_iterable(iterable) => self.emit_expr(iterable),
            _ => {
                let iter = self.emit_expr(iterable)?;
                let source = plan_owned_iterator_source(iterable).apply(iter);
                Ok(quote! { #source.into_iter() })
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
    /// - Without filter: `iter.iter().copied().map(...)` for Copy items, otherwise `iter.iter().cloned().map(...)`
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
        let body_can_propagate = Self::expr_contains_try(element) || filter.is_some_and(Self::expr_contains_try);

        if let Some(iter) = self.emit_direct_comprehension_iterable(iterable)? {
            if body_can_propagate {
                return self.emit_direct_list_comp_loop(iter, pattern_tokens, elem, filter);
            }
            return self.emit_direct_list_comp(iter, pattern_tokens, elem, filter);
        }

        let iter = self.emit_expr(iterable)?;
        let is_range = self.is_range_iterable(iterable);
        let iter_wrapped = quote! { (#iter) };
        let iter_for_loop = if is_range { iter.clone() } else { iter_wrapped.clone() };
        let plan = plan_list_comprehension_iteration(
            Self::comprehension_iterable_item_ty(&iterable.ty),
            is_range,
            filter.is_some(),
        );
        if body_can_propagate {
            return self.emit_list_comp_loop(plan, iter_for_loop, pattern, pattern_tokens, elem, filter);
        }

        match plan {
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
            ComprehensionIterationPlan::FilterMapCopyBinding => {
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
                            let #pattern_tokens = *#item_binding;
                            if #filter_tokens {
                                Some(#elem)
                            } else {
                                None
                            }
                        })
                        .collect::<Vec<_>>()
                })
            }
            ComprehensionIterationPlan::IterCopied => Ok(quote! {
                #iter_wrapped.iter().copied().map(|#pattern_tokens| #elem).collect::<Vec<_>>()
            }),
            ComprehensionIterationPlan::IterCloned => Ok(quote! {
                #iter_wrapped.iter().cloned().map(|#pattern_tokens| #elem).collect::<Vec<_>>()
            }),
        }
    }

    /// Emit a dict comprehension.
    ///
    /// Converts `{key: value for var in iter if cond}` to Rust iterator chain:
    /// - Without filter: `iter.iter().copied().map(...)` for Copy items, otherwise `iter.iter().cloned().map(...)`
    /// - With filter over borrowed iterables: `iter.iter().filter_map(...)`, copying or cloning the item before
    ///   predicate evaluation based on its IR type.
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
        let body_can_propagate = Self::expr_contains_try(key)
            || Self::expr_contains_try(value)
            || filter.is_some_and(Self::expr_contains_try);

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
            if body_can_propagate {
                return self.emit_direct_dict_comp_loop(iter, pattern_tokens, cloned_key, value_tokens, filter);
            }
            return self.emit_direct_dict_comp(iter, pattern_tokens, cloned_key, value_tokens, filter);
        }

        let iter = self.emit_expr(iterable)?;
        let plan =
            plan_dict_comprehension_iteration(Self::comprehension_iterable_item_ty(&iterable.ty), filter.is_some());
        if body_can_propagate {
            return self.emit_dict_comp_loop(
                plan,
                quote! { (#iter) },
                pattern,
                pattern_tokens,
                (cloned_key, value_tokens),
                filter,
            );
        }

        match plan {
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
            ComprehensionIterationPlan::FilterMapCopyBinding => {
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
                            let #pattern_tokens = *#item_binding;
                            if #filter_tokens {
                                Some((#cloned_key, #value_tokens))
                            } else {
                                None
                            }
                        })
                        .collect::<std::collections::HashMap<_, _>>()
                })
            }
            ComprehensionIterationPlan::IterCopied => Ok(quote! {
                #iter.iter().copied().map(|#pattern_tokens| (#cloned_key, #value_tokens)).collect::<std::collections::HashMap<_, _>>()
            }),
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

    /// Choose the borrowed closure binding used before materializing a filtered non-range comprehension item.
    fn filter_map_item_binding(pattern: &Pattern, pattern_tokens: &TokenStream) -> TokenStream {
        if matches!(pattern, Pattern::Var(_)) {
            pattern_tokens.clone()
        } else {
            quote! { __incan_comp_item }
        }
    }

    /// Return the item type a borrowed comprehension iterator yields before copy/clone materialization.
    fn comprehension_iterable_item_ty(iterable_ty: &IrType) -> Option<&IrType> {
        match iterable_ty {
            IrType::List(item_ty) | IrType::Set(item_ty) => Some(item_ty.as_ref()),
            IrType::Ref(inner) | IrType::RefMut(inner) => Self::comprehension_iterable_item_ty(inner),
            _ => None,
        }
    }

    /// Emit iterable expressions that already produce iterator items with the ownership shape a comprehension expects.
    fn emit_direct_comprehension_iterable(&self, iterable: &TypedExpr) -> Result<Option<TokenStream>, EmitError> {
        match &iterable.kind {
            IrExprKind::BuiltinCall {
                func: BuiltinFn::Enumerate,
                args,
            } => self.emit_owned_enumerate_iter(args).map(Some),
            IrExprKind::MethodCall {
                receiver, method, args, ..
            } if method == "keys" && args.is_empty() && matches!(receiver.ty, IrType::Dict(_, _)) => {
                let receiver_tokens = self.emit_expr(receiver)?;
                Ok(Some(quote! { (#receiver_tokens).keys().cloned() }))
            }
            _ => Ok(None),
        }
    }

    /// Emit `enumerate(xs)` for comprehension closures, cloning values to match the typechecker's owned tuple item
    /// type.
    fn emit_owned_enumerate_iter(&self, args: &[TypedExpr]) -> Result<TokenStream, EmitError> {
        if let Some(arg) = args.first() {
            self.emit_enumerate_iter(arg)
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

    /// Emit a direct-iterator list comprehension as an imperative block.
    ///
    /// This path is used when the element or filter contains `?`. A Rust iterator closure would make `?` target the
    /// closure's element-returning type instead of the enclosing Incan function's `Result` return type.
    fn emit_direct_list_comp_loop(
        &self,
        iter: TokenStream,
        pattern: TokenStream,
        elem: TokenStream,
        filter: Option<&TypedExpr>,
    ) -> Result<TokenStream, EmitError> {
        let body = self.emit_list_comp_push_body(elem, filter)?;
        Ok(quote! {{
            let mut __incan_list = Vec::new();
            for #pattern in (#iter) {
                #body
            }
            __incan_list
        }})
    }

    /// Emit a planned list comprehension as an imperative block.
    fn emit_list_comp_loop(
        &self,
        plan: ComprehensionIterationPlan,
        iter: TokenStream,
        pattern: &Pattern,
        pattern_tokens: TokenStream,
        elem: TokenStream,
        filter: Option<&TypedExpr>,
    ) -> Result<TokenStream, EmitError> {
        let body = self.emit_list_comp_push_body(elem, filter)?;
        match plan {
            ComprehensionIterationPlan::RangeDirect | ComprehensionIterationPlan::RangeFilter => Ok(quote! {{
                let mut __incan_list = Vec::new();
                for #pattern_tokens in #iter {
                    #body
                }
                __incan_list
            }}),
            ComprehensionIterationPlan::IterCopied => Ok(quote! {{
                let mut __incan_list = Vec::new();
                for #pattern_tokens in #iter.iter().copied() {
                    #body
                }
                __incan_list
            }}),
            ComprehensionIterationPlan::IterCloned => Ok(quote! {{
                let mut __incan_list = Vec::new();
                for #pattern_tokens in #iter.iter().cloned() {
                    #body
                }
                __incan_list
            }}),
            ComprehensionIterationPlan::FilterMapCloneBinding => {
                let item_binding = Self::filter_map_item_binding(pattern, &pattern_tokens);
                Ok(quote! {{
                    let mut __incan_list = Vec::new();
                    for #item_binding in #iter.iter() {
                        let #pattern_tokens = (*#item_binding).clone();
                        #body
                    }
                    __incan_list
                }})
            }
            ComprehensionIterationPlan::FilterMapCopyBinding => {
                let item_binding = Self::filter_map_item_binding(pattern, &pattern_tokens);
                Ok(quote! {{
                    let mut __incan_list = Vec::new();
                    for #item_binding in #iter.iter() {
                        let #pattern_tokens = *#item_binding;
                        #body
                    }
                    __incan_list
                }})
            }
        }
    }

    /// Emit one list-comprehension loop body, preserving filter semantics when present.
    fn emit_list_comp_push_body(
        &self,
        elem: TokenStream,
        filter: Option<&TypedExpr>,
    ) -> Result<TokenStream, EmitError> {
        if let Some(filter) = filter {
            let filter_tokens = self.emit_expr(filter)?;
            Ok(quote! {
                if #filter_tokens {
                    __incan_list.push(#elem);
                }
            })
        } else {
            Ok(quote! { __incan_list.push(#elem); })
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

    /// Emit a direct-iterator dict comprehension as an imperative block for propagating body expressions.
    fn emit_direct_dict_comp_loop(
        &self,
        iter: TokenStream,
        pattern: TokenStream,
        key: TokenStream,
        value: TokenStream,
        filter: Option<&TypedExpr>,
    ) -> Result<TokenStream, EmitError> {
        let body = self.emit_dict_comp_insert_body(key, value, filter)?;
        Ok(quote! {{
            let mut __incan_dict = std::collections::HashMap::new();
            for #pattern in (#iter) {
                #body
            }
            __incan_dict
        }})
    }

    /// Emit a planned dict comprehension as an imperative block.
    fn emit_dict_comp_loop(
        &self,
        plan: ComprehensionIterationPlan,
        iter: TokenStream,
        pattern: &Pattern,
        pattern_tokens: TokenStream,
        key_value: (TokenStream, TokenStream),
        filter: Option<&TypedExpr>,
    ) -> Result<TokenStream, EmitError> {
        let (key, value) = key_value;
        let body = self.emit_dict_comp_insert_body(key, value, filter)?;
        match plan {
            ComprehensionIterationPlan::IterCopied => Ok(quote! {{
                let mut __incan_dict = std::collections::HashMap::new();
                for #pattern_tokens in #iter.iter().copied() {
                    #body
                }
                __incan_dict
            }}),
            ComprehensionIterationPlan::IterCloned => Ok(quote! {{
                let mut __incan_dict = std::collections::HashMap::new();
                for #pattern_tokens in #iter.iter().cloned() {
                    #body
                }
                __incan_dict
            }}),
            ComprehensionIterationPlan::FilterMapCloneBinding => {
                let item_binding = Self::filter_map_item_binding(pattern, &pattern_tokens);
                Ok(quote! {{
                    let mut __incan_dict = std::collections::HashMap::new();
                    for #item_binding in #iter.iter() {
                        let #pattern_tokens = (*#item_binding).clone();
                        #body
                    }
                    __incan_dict
                }})
            }
            ComprehensionIterationPlan::FilterMapCopyBinding => {
                let item_binding = Self::filter_map_item_binding(pattern, &pattern_tokens);
                Ok(quote! {{
                    let mut __incan_dict = std::collections::HashMap::new();
                    for #item_binding in #iter.iter() {
                        let #pattern_tokens = *#item_binding;
                        #body
                    }
                    __incan_dict
                }})
            }
            ComprehensionIterationPlan::RangeDirect | ComprehensionIterationPlan::RangeFilter => {
                unreachable!("dict comprehensions do not use range-specific iteration plans")
            }
        }
    }

    /// Emit one dict-comprehension loop body, preserving filter semantics when present.
    fn emit_dict_comp_insert_body(
        &self,
        key: TokenStream,
        value: TokenStream,
        filter: Option<&TypedExpr>,
    ) -> Result<TokenStream, EmitError> {
        if let Some(filter) = filter {
            let filter_tokens = self.emit_expr(filter)?;
            Ok(quote! {
                if #filter_tokens {
                    __incan_dict.insert(#key, #value);
                }
            })
        } else {
            Ok(quote! { __incan_dict.insert(#key, #value); })
        }
    }

    /// Return whether an expression subtree contains `?` and therefore cannot be emitted inside a non-Result Rust
    /// iterator closure.
    fn expr_contains_try(expr: &TypedExpr) -> bool {
        match &expr.kind {
            IrExprKind::Try(_) => true,
            IrExprKind::BinOp { left, right, .. } => Self::expr_contains_try(left) || Self::expr_contains_try(right),
            IrExprKind::UnaryOp { operand, .. }
            | IrExprKind::Await(operand)
            | IrExprKind::Cast { expr: operand, .. }
            | IrExprKind::NumericResize { expr: operand, .. }
            | IrExprKind::InteropCoerce { expr: operand, .. } => Self::expr_contains_try(operand),
            IrExprKind::Call { func, args, .. } => {
                Self::expr_contains_try(func) || args.iter().any(Self::call_arg_contains_try)
            }
            IrExprKind::BuiltinCall { args, .. } => args.iter().any(Self::expr_contains_try),
            IrExprKind::MethodCall { receiver, args, .. } | IrExprKind::KnownMethodCall { receiver, args, .. } => {
                Self::expr_contains_try(receiver) || args.iter().any(Self::call_arg_contains_try)
            }
            IrExprKind::Field { object, .. } => Self::expr_contains_try(object),
            IrExprKind::Index { object, index } => Self::expr_contains_try(object) || Self::expr_contains_try(index),
            IrExprKind::Slice {
                target,
                start,
                end,
                step,
            } => {
                Self::expr_contains_try(target)
                    || start.as_ref().is_some_and(|expr| Self::expr_contains_try(expr))
                    || end.as_ref().is_some_and(|expr| Self::expr_contains_try(expr))
                    || step.as_ref().is_some_and(|expr| Self::expr_contains_try(expr))
            }
            IrExprKind::ListComp {
                element,
                iterable,
                filter,
                ..
            } => {
                Self::expr_contains_try(element)
                    || Self::expr_contains_try(iterable)
                    || filter.as_ref().is_some_and(|expr| Self::expr_contains_try(expr))
            }
            IrExprKind::DictComp {
                key,
                value,
                iterable,
                filter,
                ..
            } => {
                Self::expr_contains_try(key)
                    || Self::expr_contains_try(value)
                    || Self::expr_contains_try(iterable)
                    || filter.as_ref().is_some_and(|expr| Self::expr_contains_try(expr))
            }
            IrExprKind::Generator { element, clauses } => {
                Self::expr_contains_try(element) || clauses.iter().any(Self::generator_clause_contains_try)
            }
            IrExprKind::List(items) => items.iter().any(Self::list_entry_contains_try),
            IrExprKind::Dict(entries) => entries.iter().any(Self::dict_entry_contains_try),
            IrExprKind::Set(items) | IrExprKind::Tuple(items) => items.iter().any(Self::expr_contains_try),
            IrExprKind::Struct { fields, .. } => fields.iter().any(|(_, expr)| Self::expr_contains_try(expr)),
            IrExprKind::If {
                condition,
                then_branch,
                else_branch,
            } => {
                Self::expr_contains_try(condition)
                    || Self::expr_contains_try(then_branch)
                    || else_branch.as_ref().is_some_and(|expr| Self::expr_contains_try(expr))
            }
            IrExprKind::Match { scrutinee, arms } => {
                Self::expr_contains_try(scrutinee)
                    || arms.iter().any(|arm| {
                        arm.bindings.iter().any(|binding| {
                            Self::expr_contains_try(&binding.value)
                                || binding.guard_value.as_ref().is_some_and(Self::expr_contains_try)
                        }) || arm.guard.as_ref().is_some_and(Self::expr_contains_try)
                            || Self::expr_contains_try(&arm.body)
                    })
            }
            IrExprKind::Closure { body, .. } => Self::expr_contains_try(body),
            IrExprKind::Block { stmts, value } => {
                stmts.iter().any(Self::stmt_contains_try)
                    || value.as_ref().is_some_and(|expr| Self::expr_contains_try(expr))
            }
            IrExprKind::Loop { body } => body.iter().any(Self::stmt_contains_try),
            IrExprKind::Race { arms, .. } => arms
                .iter()
                .any(|arm| Self::expr_contains_try(&arm.awaitable) || Self::expr_contains_try(&arm.body)),
            IrExprKind::Range { start, end, .. } => {
                start.as_ref().is_some_and(|expr| Self::expr_contains_try(expr))
                    || end.as_ref().is_some_and(|expr| Self::expr_contains_try(expr))
            }
            IrExprKind::Format { parts } => parts.iter().any(|part| match part {
                FormatPart::Literal(_) => false,
                FormatPart::Expr { expr, .. } => Self::expr_contains_try(expr),
            }),
            IrExprKind::RegisterCallableName { callable, .. } => Self::expr_contains_try(callable),
            IrExprKind::CacheGenericDecoratedFunction { value, .. } => Self::expr_contains_try(value),
            IrExprKind::Unit
            | IrExprKind::None
            | IrExprKind::Bool(_)
            | IrExprKind::Int(_)
            | IrExprKind::IntLiteral(_)
            | IrExprKind::Float(_)
            | IrExprKind::Decimal(_)
            | IrExprKind::String(_)
            | IrExprKind::Bytes(_)
            | IrExprKind::Var { .. }
            | IrExprKind::StaticRead { .. }
            | IrExprKind::StaticBinding { .. }
            | IrExprKind::AssociatedFunction { .. }
            | IrExprKind::TypeToken { .. }
            | IrExprKind::FunctionItem { .. }
            | IrExprKind::Literal(_)
            | IrExprKind::FieldsList(_)
            | IrExprKind::SerdeToJson
            | IrExprKind::SerdeFromJson(_) => false,
        }
    }

    /// Return whether a call argument contains a `try` expression.
    fn call_arg_contains_try(arg: &IrCallArg) -> bool {
        Self::expr_contains_try(&arg.expr)
    }

    /// Return whether a list entry contains a `try` expression.
    fn list_entry_contains_try(entry: &IrListEntry) -> bool {
        match entry {
            IrListEntry::Element(expr) | IrListEntry::Spread(expr) => Self::expr_contains_try(expr),
        }
    }

    /// Return whether a dict entry contains a `try` expression.
    fn dict_entry_contains_try(entry: &IrDictEntry) -> bool {
        match entry {
            IrDictEntry::Pair(key, value) => Self::expr_contains_try(key) || Self::expr_contains_try(value),
            IrDictEntry::Spread(expr) => Self::expr_contains_try(expr),
        }
    }

    /// Return whether a generator clause contains a `try` expression.
    fn generator_clause_contains_try(clause: &IrGeneratorClause) -> bool {
        match clause {
            IrGeneratorClause::For { iterable, .. } => Self::expr_contains_try(iterable),
            IrGeneratorClause::If(condition) => Self::expr_contains_try(condition),
        }
    }

    /// Return whether a statement contains a `try` expression.
    fn stmt_contains_try(stmt: &IrStmt) -> bool {
        match &stmt.kind {
            IrStmtKind::Expr(expr) | IrStmtKind::Let { value: expr, .. } | IrStmtKind::Yield(expr) => {
                Self::expr_contains_try(expr)
            }
            IrStmtKind::Assign { target, value } => {
                Self::assign_target_contains_try(target) || Self::expr_contains_try(value)
            }
            IrStmtKind::CompoundAssign { target, value, .. } => {
                Self::assign_target_contains_try(target) || Self::expr_contains_try(value)
            }
            IrStmtKind::Return(value) | IrStmtKind::Break { value, .. } => {
                value.as_ref().is_some_and(Self::expr_contains_try)
            }
            IrStmtKind::While { condition, body, .. } => {
                Self::expr_contains_try(condition) || body.iter().any(Self::stmt_contains_try)
            }
            IrStmtKind::For { iterable, body, .. } => {
                Self::expr_contains_try(iterable) || body.iter().any(Self::stmt_contains_try)
            }
            IrStmtKind::Loop { body, .. } | IrStmtKind::Block(body) => body.iter().any(Self::stmt_contains_try),
            IrStmtKind::If {
                condition,
                then_branch,
                else_branch,
            } => {
                Self::expr_contains_try(condition)
                    || then_branch.iter().any(Self::stmt_contains_try)
                    || else_branch
                        .as_ref()
                        .is_some_and(|body| body.iter().any(Self::stmt_contains_try))
            }
            IrStmtKind::Match { scrutinee, arms } => {
                Self::expr_contains_try(scrutinee)
                    || arms.iter().any(|arm| {
                        arm.bindings.iter().any(|binding| {
                            Self::expr_contains_try(&binding.value)
                                || binding.guard_value.as_ref().is_some_and(Self::expr_contains_try)
                        }) || arm.guard.as_ref().is_some_and(Self::expr_contains_try)
                            || Self::expr_contains_try(&arm.body)
                    })
            }
            IrStmtKind::Continue(_) => false,
        }
    }

    /// Return whether an assignment target contains a `try` expression.
    fn assign_target_contains_try(target: &AssignTarget) -> bool {
        match target {
            AssignTarget::Field { object, .. } => Self::expr_contains_try(object),
            AssignTarget::Index { object, index } => Self::expr_contains_try(object) || Self::expr_contains_try(index),
            AssignTarget::Var(_) | AssignTarget::StaticBinding(_) | AssignTarget::Static(_) => false,
        }
    }
}
