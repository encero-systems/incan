//! Emit Rust expressions from Incan IR.
//!
//! This module converts IR expressions ([`TypedExpr`]/[`IrExprKind`]) into Rust expression
//! fragments ([`TokenStream`]).
//!
//! It is used by [`IrEmitter`] to implement the "IR → Rust" portion of the backend at the
//! expression level (literals, operators, calls, method calls, comprehensions, indexing/slicing,
//! and control flow).
//!
//! ## Module organization
//!
//! The expression emitter is split into focused submodules:
//!
//! - [`builtins`]: Built-in function calls (`print`, `len`, `range`, etc.)
//! - [`methods`]: Method calls (both known methods via `MethodKind` and regular Rust method-call emission)
//! - [`calls`]: Regular function calls and binary operations
//! - [`indexing`]: Index, slice, and field access expressions
//! - [`comprehensions`]: List comprehensions, dict comprehensions, and generator expressions
//! - [`structs_enums`]: Struct constructor expressions
//! - [`mod@format`]: Format strings and range expressions
//! - [`lvalue`]: Assignment target expressions
//!
//! ## Notes
//!
//! - **Not lexer tokens**: [`TokenStream`] here is `proc_macro2::TokenStream` used for Rust codegen. Lexer output is a
//!   separate token type in the frontend.
//! - **Ownership planning is centralized**: Ownership/borrow/copy/string adjustments should go through
//!   `backend::ir::ownership` instead of being hand-coded inline.
//! - **Side-effect free**: Emission is pure codegen; it does not touch the filesystem.
//!
//! ## Examples
//!
//! ```rust,ignore
//! // Pseudocode: IrEmitter is constructed by the backend codegen pipeline.
//! let tokens: proc_macro2::TokenStream = emitter.emit_expr(&typed_expr)?;
//! ```
//!
//! ## See also
//!
//! - `src/backend/ir/ownership.rs`: ownership/coercion planner for emitted Rust boundaries
//! - `src/backend/ir/emit/mod.rs`: higher-level emission (items/statements) that calls into this module

mod builtins;
mod calls;
mod comprehensions;
mod format;
mod indexing;
mod interop_coercions;
mod lvalue;
mod methods;
mod structs_enums;

use proc_macro2::{Literal, TokenStream};
use quote::{ToTokens, format_ident, quote};

use super::super::decl::IrInteropAdapterKind;
use super::super::expr::{
    CollectionMethodKind, IrDictEntry, IrExprKind, IrInteropCoercionKind, IrListEntry, Literal as IrLiteral,
    MethodKind, NumericResizePolicy, TypedExpr, UnaryOp, VarRefKind,
};
use super::super::types::IrType;
use super::{EmitError, IrEmitter};
use crate::backend::ir::ownership::{ValueUseSite, plan_value_use, value_use_site_target_ty};
use incan_core::lang::types::collections::{self, CollectionTypeId};

#[derive(Debug, Clone)]
pub(super) enum StorageRoot {
    /// A module-level static storage slot.
    Static(String),
    /// A local alias that wraps static storage in the current emitted statement slice.
    Binding(String),
}

/// Whether a lowered known method mutates its receiver.
///
/// This is the canonical receiver-mutability policy for `MethodKind` in IR emission. Keep method mutability decisions
/// in one place to avoid drift between statement analysis, parameter mutation scan, and emitted storage-lock behavior.
pub(in crate::backend::ir::emit) fn method_kind_uses_mutable_receiver(kind: &MethodKind) -> bool {
    matches!(
        kind,
        MethodKind::Collection(
            CollectionMethodKind::Insert
                | CollectionMethodKind::Remove
                | CollectionMethodKind::Append
                | CollectionMethodKind::Extend
                | CollectionMethodKind::Pop
                | CollectionMethodKind::Swap
                | CollectionMethodKind::Reserve
                | CollectionMethodKind::ReserveExact
        )
    )
}

impl<'a> IrEmitter<'a> {
    /// Build a typed tuple-field read for compiler-expanded tuple unpacking.
    pub(super) fn tuple_field_expr(expr: &TypedExpr, idx: usize, ty: IrType) -> TypedExpr {
        TypedExpr::new(
            IrExprKind::Field {
                object: Box::new(expr.clone()),
                field: idx.to_string(),
            },
            ty,
        )
        .with_span(expr.span)
    }

    /// Emit explicit callable-name metadata for a concrete function pointer.
    fn emit_register_callable_name(&self, callable: &TypedExpr, source_name: &str) -> Result<TokenStream, EmitError> {
        let IrType::Function { params, ret } = &callable.ty else {
            return Ok(quote! { () });
        };
        let Some(signature_key) = Self::callable_name_signature_key(params, ret) else {
            return Ok(quote! { () });
        };
        let register = Self::callable_name_register_ident(&signature_key);
        let fn_ty = self.emit_callable_fn_type(params, ret);
        let callable = self.emit_expr(callable)?;
        let source_name = Literal::string(source_name);
        Ok(quote! {{
            let __incan_callable: #fn_ty = #callable;
            #register(__incan_callable, #source_name);
        }})
    }

    /// Emit a cached wrapper for a generic decorated function.
    fn emit_cache_generic_decorated_function(
        &self,
        cache_name: &str,
        type_param_names: &[String],
        value: &TypedExpr,
    ) -> Result<TokenStream, EmitError> {
        if !matches!(value.ty, IrType::Function { .. }) {
            return Err(EmitError::Unsupported(
                "generic decorated function cache requires a function pointer type".to_string(),
            ));
        }
        let cache_ident = Self::rust_static_ident(&format!("__incan_generic_decorated_{cache_name}"));
        let fn_ty = self.emit_type(&value.ty);
        let value_tokens = self.emit_expr(value)?;
        let type_key_parts = type_param_names
            .iter()
            .map(|name| {
                let ident = Self::rust_ident(name);
                quote! { std::any::type_name::<#ident>() }
            })
            .collect::<Vec<_>>();
        let type_key = if type_key_parts.is_empty() {
            quote! { String::new() }
        } else {
            quote! { [#(#type_key_parts),*].join("\u{1f}") }
        };

        Ok(quote! {{
            static #cache_ident: std::sync::OnceLock<std::sync::Mutex<Vec<(String, #fn_ty)>>> =
                std::sync::OnceLock::new();
            let __incan_type_key = #type_key;
            let mut __incan_entries = #cache_ident
                .get_or_init(|| std::sync::Mutex::new(Vec::new()))
                .lock()
                .unwrap_or_else(|__incan_poisoned| __incan_poisoned.into_inner());
            if let Some((_, __incan_cached)) = __incan_entries
                .iter()
                .find(|(__incan_key, _)| __incan_key == &__incan_type_key)
            {
                *__incan_cached
            } else {
                let __incan_decorated = #value_tokens;
                __incan_entries.push((__incan_type_key, __incan_decorated));
                __incan_decorated
            }
        }})
    }

    /// Emit one list-literal element, materializing owned sink semantics at the literal boundary.
    ///
    /// Incan `list[str]` literals should store owned Rust `String` elements up front, but ordinary Incan-to-Incan
    /// helper calls should not re-lower already-owned `list[str]` variables through consuming iterator conversions.
    /// Keeping this as a dedicated helper makes that ownership rule explicit instead of leaking a more incidental
    /// conversion context into the call site.
    fn emit_list_literal_item(
        &self,
        item: &TypedExpr,
        item_target_ty: Option<&IrType>,
    ) -> Result<TokenStream, EmitError> {
        self.emit_expr_for_use(
            item,
            ValueUseSite::CollectionElement {
                target_ty: item_target_ty,
            },
        )
    }

    /// Emit a list literal while preserving direct and spread entry order.
    fn emit_list_literal_entries(
        &self,
        items: &[IrListEntry],
        item_target_ty: Option<&IrType>,
    ) -> Result<TokenStream, EmitError> {
        if items.iter().all(|entry| matches!(entry, IrListEntry::Element(_))) {
            let item_tokens: Vec<TokenStream> = items
                .iter()
                .map(|entry| match entry {
                    IrListEntry::Element(item) => self.emit_list_literal_item(item, item_target_ty),
                    IrListEntry::Spread(_) => Err(EmitError::Unsupported(
                        "internal error: unexpected list spread in direct-only literal emission".to_string(),
                    )),
                })
                .collect::<Result<_, _>>()?;
            return Ok(quote! { vec![#(#item_tokens),*] });
        }

        let steps: Vec<TokenStream> = items
            .iter()
            .map(|entry| match entry {
                IrListEntry::Element(item) => {
                    let item_tokens = self.emit_list_literal_item(item, item_target_ty)?;
                    Ok(quote! { __incan_list.push(#item_tokens); })
                }
                IrListEntry::Spread(value) => {
                    if let IrType::Tuple(items) = &value.ty {
                        let mut pushes = Vec::with_capacity(items.len());
                        for (idx, item_ty) in items.iter().enumerate() {
                            let item = Self::tuple_field_expr(value, idx, item_ty.clone());
                            let item_tokens = self.emit_list_literal_item(&item, item_target_ty)?;
                            pushes.push(quote! { __incan_list.push(#item_tokens); });
                        }
                        Ok(quote! { #(#pushes)* })
                    } else {
                        let value_tokens = self.emit_expr(value)?;
                        Ok(quote! { __incan_list.extend((#value_tokens).into_iter()); })
                    }
                }
            })
            .collect::<Result<_, EmitError>>()?;

        Ok(quote! {{
            let mut __incan_list = Vec::new();
            #(#steps)*
            __incan_list
        }})
    }

    /// Emit a dictionary literal while preserving direct and spread entry order.
    fn emit_dict_literal_entries(
        &self,
        pairs: &[IrDictEntry],
        key_target_ty: Option<&IrType>,
        value_target_ty: Option<&IrType>,
    ) -> Result<TokenStream, EmitError> {
        if pairs.is_empty() {
            return Ok(quote! { std::collections::HashMap::new() });
        }

        if pairs.iter().all(|entry| matches!(entry, IrDictEntry::Pair(_, _))) {
            let pair_tokens: Vec<TokenStream> = pairs
                .iter()
                .map(|entry| match entry {
                    IrDictEntry::Pair(key, value) => {
                        let key_tokens = self.emit_expr_for_use(
                            key,
                            ValueUseSite::CollectionElement {
                                target_ty: key_target_ty,
                            },
                        )?;
                        let value_tokens = self.emit_expr_for_use(
                            value,
                            ValueUseSite::CollectionElement {
                                target_ty: value_target_ty,
                            },
                        )?;
                        Ok(quote! { (#key_tokens, #value_tokens) })
                    }
                    IrDictEntry::Spread(_) => Err(EmitError::Unsupported(
                        "internal error: unexpected dict spread in direct-only literal emission".to_string(),
                    )),
                })
                .collect::<Result<_, EmitError>>()?;
            return Ok(quote! { [#(#pair_tokens),*].into_iter().collect::<std::collections::HashMap<_, _>>() });
        }

        let steps: Vec<TokenStream> = pairs
            .iter()
            .map(|entry| match entry {
                IrDictEntry::Pair(key, value) => {
                    let key_tokens = self.emit_expr_for_use(
                        key,
                        ValueUseSite::CollectionElement {
                            target_ty: key_target_ty,
                        },
                    )?;
                    let value_tokens = self.emit_expr_for_use(
                        value,
                        ValueUseSite::CollectionElement {
                            target_ty: value_target_ty,
                        },
                    )?;
                    Ok(quote! { __incan_dict.insert(#key_tokens, #value_tokens); })
                }
                IrDictEntry::Spread(value) => {
                    let value_tokens = self.emit_expr(value)?;
                    Ok(quote! {
                        for (__incan_key, __incan_value) in (#value_tokens).into_iter() {
                            __incan_dict.insert(__incan_key, __incan_value);
                        }
                    })
                }
            })
            .collect::<Result<_, EmitError>>()?;

        Ok(quote! {{
            let mut __incan_dict = std::collections::HashMap::new();
            #(#steps)*
            __incan_dict
        }})
    }

    /// Return the target type carried by a value-use site, if the site has one.
    fn use_site_target_ty<'b>(site: ValueUseSite<'b>) -> Option<&'b IrType> {
        value_use_site_target_ty(site)
    }

    /// Prefer the call-site target type for aggregate literal elements.
    ///
    /// Generic targets still matter for ownership conversion: a string literal passed into `list[K]` should materialize
    /// as an owned `String` for Incan calls, not leak Rust's `&str` literal type into the generic container.
    fn concrete_literal_target<'b>(
        target_ty: Option<&'b IrType>,
        inferred_ty: Option<&'b IrType>,
    ) -> Option<&'b IrType> {
        match target_ty {
            Some(ty) => Some(ty),
            None => inferred_ty,
        }
    }

    /// Rebuild a parent value-use site for one tuple item while preserving the parent ownership context.
    fn tuple_item_use_site<'b>(site: ValueUseSite<'b>, target_ty: Option<&'b IrType>) -> ValueUseSite<'b> {
        Self::retarget_value_use_site(site, target_ty)
    }

    /// Rebuild a value-use site with a more specific target type while preserving the context kind.
    fn retarget_value_use_site<'b>(site: ValueUseSite<'b>, target_ty: Option<&'b IrType>) -> ValueUseSite<'b> {
        match site {
            ValueUseSite::IncanCallArg { in_return, .. } => ValueUseSite::IncanCallArg {
                target_ty,
                callee_param: None,
                in_return,
            },
            ValueUseSite::ExternalCallArg { .. } => ValueUseSite::ExternalCallArg { target_ty },
            ValueUseSite::StructField { .. } => ValueUseSite::StructField { target_ty },
            ValueUseSite::CollectionElement { .. } => ValueUseSite::CollectionElement { target_ty },
            ValueUseSite::Assignment { .. } => ValueUseSite::Assignment { target_ty },
            ValueUseSite::ReturnValue { .. } => ValueUseSite::ReturnValue { target_ty },
            ValueUseSite::MatchScrutinee { .. } => ValueUseSite::MatchScrutinee { target_ty },
            ValueUseSite::MethodArg => ValueUseSite::MethodArg,
        }
    }

    /// Return the `Result[output, error]` target type for the inner expression of `output?`.
    fn try_inner_target_type(&self, output_ty: &IrType, inner: &TypedExpr) -> Option<IrType> {
        if matches!(output_ty, IrType::Unknown) {
            return None;
        }
        let err_ty = match &inner.ty {
            IrType::Result(_, err_ty) => Some(err_ty.as_ref().clone()),
            _ => self
                .current_function_return_type
                .borrow()
                .as_ref()
                .and_then(|return_ty| {
                    if let IrType::Result(_, err_ty) = return_ty {
                        Some(err_ty.as_ref().clone())
                    } else {
                        None
                    }
                }),
        }?;
        Some(IrType::Result(Box::new(output_ty.clone()), Box::new(err_ty)))
    }

    /// Emit an expression directly against an ownership-planned sink/source boundary.
    ///
    /// Aggregate literals are handled recursively so element-level ownership policy is applied before the outer
    /// expression is emitted. Non-aggregate expressions are emitted normally, then the planned conversion is applied to
    /// the resulting token stream.
    pub(super) fn emit_expr_for_use(&self, expr: &TypedExpr, site: ValueUseSite<'_>) -> Result<TokenStream, EmitError> {
        let resolved_target_ty = Self::use_site_target_ty(site).map(|ty| self.resolve_type_aliases_for_emit(ty));
        if let Some(target_ty) = resolved_target_ty.as_ref() {
            if let Some(wrapped) = self.emit_union_payload_arg_for_site(expr, target_ty, None, site)? {
                return Ok(wrapped);
            }
            if matches!(site, ValueUseSite::CollectionElement { .. })
                && let Some(wrapped) = self.emit_inference_seeded_literal_arg(expr, target_ty)?
            {
                return Ok(wrapped);
            }
        }

        match &expr.kind {
            IrExprKind::InteropCoerce { expr: inner, .. }
                if Self::use_site_target_ty(site).is_some()
                    && matches!(
                        inner.kind,
                        IrExprKind::List(_) | IrExprKind::Dict(_) | IrExprKind::Set(_) | IrExprKind::Tuple(_)
                    ) =>
            {
                return self.emit_expr_for_use(inner, site);
            }
            IrExprKind::InteropCoerce { expr: inner, .. }
                if Self::use_site_target_ty(site).is_some()
                    && matches!(inner.kind, IrExprKind::Call { .. } | IrExprKind::MethodCall { .. }) =>
            {
                return self.emit_expr_for_use(inner, site);
            }
            IrExprKind::List(items) => {
                let site_item_ty = match resolved_target_ty.as_ref() {
                    Some(IrType::List(elem)) => Some(elem.as_ref()),
                    _ => None,
                };
                let inferred_item_ty = match &expr.ty {
                    IrType::List(elem) => Some(elem.as_ref()),
                    _ => None,
                };
                let item_target_ty = Self::concrete_literal_target(site_item_ty, inferred_item_ty);
                return self.emit_list_literal_entries(items, item_target_ty);
            }
            IrExprKind::Dict(pairs) => {
                let (site_key_ty, site_value_ty) = match resolved_target_ty.as_ref() {
                    Some(IrType::Dict(key, value)) => (Some(key.as_ref()), Some(value.as_ref())),
                    _ => (None, None),
                };
                let (inferred_key_ty, inferred_value_ty) = match &expr.ty {
                    IrType::Dict(key, value) => (Some(key.as_ref()), Some(value.as_ref())),
                    _ => (None, None),
                };
                let key_target_ty = Self::concrete_literal_target(site_key_ty, inferred_key_ty);
                let value_target_ty = Self::concrete_literal_target(site_value_ty, inferred_value_ty);
                return self.emit_dict_literal_entries(pairs, key_target_ty, value_target_ty);
            }
            IrExprKind::Set(items) => {
                if items.is_empty() {
                    return Ok(quote! { std::collections::HashSet::new() });
                }
                let site_item_ty = match resolved_target_ty.as_ref() {
                    Some(IrType::Set(elem)) => Some(elem.as_ref()),
                    _ => None,
                };
                let inferred_item_ty = match &expr.ty {
                    IrType::Set(elem) => Some(elem.as_ref()),
                    _ => None,
                };
                let item_target_ty = Self::concrete_literal_target(site_item_ty, inferred_item_ty);
                let item_tokens: Vec<TokenStream> = items
                    .iter()
                    .map(|item| {
                        self.emit_expr_for_use(
                            item,
                            ValueUseSite::CollectionElement {
                                target_ty: item_target_ty,
                            },
                        )
                    })
                    .collect::<Result<_, _>>()?;
                return Ok(quote! { [#(#item_tokens),*].into_iter().collect::<std::collections::HashSet<_>>() });
            }
            IrExprKind::Tuple(items) => {
                let site_tuple_items = match resolved_target_ty.as_ref() {
                    Some(IrType::Tuple(items)) => Some(items.as_slice()),
                    _ => None,
                };
                let inferred_tuple_items = match &expr.ty {
                    IrType::Tuple(items) => Some(items.as_slice()),
                    _ => None,
                };
                let item_tokens: Vec<TokenStream> = items
                    .iter()
                    .enumerate()
                    .map(|(idx, item)| {
                        let site_item_ty = site_tuple_items.and_then(|items| items.get(idx));
                        let inferred_item_ty = inferred_tuple_items.and_then(|items| items.get(idx));
                        let item_target_ty = Self::concrete_literal_target(site_item_ty, inferred_item_ty);
                        self.emit_expr_for_use(item, Self::tuple_item_use_site(site, item_target_ty))
                    })
                    .collect::<Result<_, _>>()?;
                return Ok(quote! { (#(#item_tokens),*) });
            }
            IrExprKind::Try(inner) => {
                let site_target_ty = Self::use_site_target_ty(site);
                let inner_tokens = if let Some(inner_target_ty) =
                    site_target_ty.and_then(|target_ty| self.try_inner_target_type(target_ty, inner))
                {
                    self.emit_expr_for_use(inner, Self::retarget_value_use_site(site, Some(&inner_target_ty)))?
                } else {
                    self.emit_expr(inner)?
                };
                return Ok(quote! { #inner_tokens? });
            }
            IrExprKind::MethodCall {
                receiver,
                method,
                dispatch,
                type_args,
                args,
                callable_signature,
                arg_policy,
            } => {
                return self.emit_method_call_expr_for_use(
                    receiver,
                    method,
                    dispatch.as_ref(),
                    type_args,
                    args,
                    callable_signature.as_ref(),
                    *arg_policy,
                    site,
                );
            }
            IrExprKind::Call {
                func,
                type_args,
                args,
                callable_signature,
                canonical_path,
            } => {
                let target_site = if let Some(target_ty) = resolved_target_ty.as_ref() {
                    Self::retarget_value_use_site(site, Some(target_ty))
                } else {
                    site
                };
                return self.emit_call_expr_for_use(
                    func,
                    type_args,
                    args,
                    callable_signature.as_ref(),
                    canonical_path.as_deref(),
                    target_site,
                );
            }
            _ => {}
        }

        let emitted = self.emit_expr(expr)?;
        let plan = plan_value_use(expr, site);
        Ok(plan.apply(emitted))
    }

    /// Return whether match scrutinee emission should preserve a `Result` value without extra ownership shaping.
    fn type_is_result_like(ty: &IrType) -> bool {
        match ty {
            IrType::Result(_, _) => true,
            IrType::NamedGeneric(name, args) if args.len() == 2 => {
                collections::from_str(name.rsplit("::").next().unwrap_or(name)) == Some(CollectionTypeId::Result)
            }
            _ => false,
        }
    }

    /// Emit the scrutinee expression for a match statement.
    pub(super) fn emit_match_scrutinee(&self, scrutinee: &TypedExpr) -> Result<TokenStream, EmitError> {
        if matches!(scrutinee.ty, IrType::Unknown) || Self::type_is_result_like(&scrutinee.ty) {
            return self.emit_expr(scrutinee);
        }
        self.emit_expr_for_use(
            scrutinee,
            ValueUseSite::MatchScrutinee {
                target_ty: Some(&scrutinee.ty),
            },
        )
    }

    /// Check whether an expression is a type-like identifier that should use Rust path syntax.
    ///
    /// This covers Incan type names, enum variants, module placeholders, and external Rust imports.
    pub(super) fn expr_is_type_like(expr: &TypedExpr) -> bool {
        match &expr.kind {
            IrExprKind::Var { ref_kind, .. } => {
                matches!(
                    ref_kind,
                    VarRefKind::TypeName | VarRefKind::ExternalName | VarRefKind::ExternalRustName
                )
            }
            _ => false,
        }
    }

    pub(super) fn expr_storage_root(expr: &TypedExpr) -> Option<StorageRoot> {
        match &expr.kind {
            IrExprKind::StaticRead { name } => Some(StorageRoot::Static(name.clone())),
            IrExprKind::Var {
                name,
                ref_kind: VarRefKind::StaticBinding,
                ..
            } => Some(StorageRoot::Binding(name.clone())),
            IrExprKind::Field { object, .. } | IrExprKind::Index { object, .. } => Self::expr_storage_root(object),
            _ => None,
        }
    }

    pub(super) fn expr_is_storage_rooted(expr: &TypedExpr) -> bool {
        Self::expr_storage_root(expr).is_some()
    }

    /// Rewrite a static/storage binding root to the local borrowed value used inside `with_ref`.
    pub(super) fn rewrite_storage_root_expr(expr: &TypedExpr, local_name: &str) -> TypedExpr {
        Self::rewrite_storage_root_expr_inner(expr, local_name, false)
    }

    /// Rewrite a static/storage binding root to the local mutable borrow used inside `with_mut`.
    pub(super) fn rewrite_storage_root_expr_for_mut(expr: &TypedExpr, local_name: &str) -> TypedExpr {
        Self::rewrite_storage_root_expr_inner(expr, local_name, true)
    }

    /// Rewrite the root of a storage-backed path while preserving the original field/index chain.
    fn rewrite_storage_root_expr_inner(expr: &TypedExpr, local_name: &str, mutable_root: bool) -> TypedExpr {
        let replacement = || {
            let ty = if mutable_root {
                IrType::RefMut(Box::new(expr.ty.clone()))
            } else {
                expr.ty.clone()
            };
            TypedExpr::new(
                IrExprKind::Var {
                    name: local_name.to_string(),
                    access: super::super::expr::VarAccess::Read,
                    ref_kind: VarRefKind::Value,
                },
                ty,
            )
        };

        let mut rewritten = match &expr.kind {
            IrExprKind::StaticRead { .. } => replacement(),
            IrExprKind::Var {
                ref_kind: VarRefKind::StaticBinding,
                ..
            } => replacement(),
            IrExprKind::Field { object, field } => TypedExpr::new(
                IrExprKind::Field {
                    object: Box::new(Self::rewrite_storage_root_expr_inner(object, local_name, mutable_root)),
                    field: field.clone(),
                },
                expr.ty.clone(),
            ),
            IrExprKind::Index { object, index } => TypedExpr::new(
                IrExprKind::Index {
                    object: Box::new(Self::rewrite_storage_root_expr_inner(object, local_name, mutable_root)),
                    index: index.clone(),
                },
                expr.ty.clone(),
            ),
            _ => expr.clone(),
        };
        rewritten.ownership = expr.ownership;
        rewritten.span = expr.span;
        rewritten
    }

    /// Emit storage access while preserving a shared reference.
    pub(super) fn emit_storage_with_ref(&self, expr: &TypedExpr, body: TokenStream) -> Result<TokenStream, EmitError> {
        let local_name = format_ident!("__incan_static_value");
        match Self::expr_storage_root(expr) {
            Some(StorageRoot::Static(name)) => {
                let ident = Self::rust_static_ident(&name);
                let init_call = self.emit_static_init_call_for_static(&name);
                Ok(quote! {{
                    #init_call
                    #ident.with_ref(|#local_name| { #body })
                }})
            }
            Some(StorageRoot::Binding(name)) => {
                let ident = Self::rust_ident(&name);
                Ok(quote! { #ident.with_ref(|#local_name| { #body }) })
            }
            None => Err(EmitError::Unsupported("expected storage-rooted expression".to_string())),
        }
    }

    /// Emit storage access while preserving a mutable reference.
    pub(super) fn emit_storage_with_mut(&self, expr: &TypedExpr, body: TokenStream) -> Result<TokenStream, EmitError> {
        let local_name = format_ident!("__incan_static_value");
        match Self::expr_storage_root(expr) {
            Some(StorageRoot::Static(name)) => {
                let ident = Self::rust_static_ident(&name);
                let init_call = self.emit_static_init_call_for_static(&name);
                Ok(quote! {{
                    #init_call
                    #ident.with_mut(|#local_name| { #body })
                }})
            }
            Some(StorageRoot::Binding(name)) => {
                let ident = Self::rust_ident(&name);
                Ok(quote! { #ident.with_mut(|#local_name| { #body }) })
            }
            None => Err(EmitError::Unsupported("expected storage-rooted expression".to_string())),
        }
    }

    /// Emit an IR expression as a Rust `TokenStream`.
    ///
    /// ## Parameters
    /// - `expr`: The typed IR expression to emit.
    ///
    /// ## Returns
    /// - A Rust `TokenStream` representing an expression.
    ///
    /// ## Errors
    /// - `EmitError`: if the IR contains an unsupported construct or emission of a sub-expression fails.
    ///
    /// ## Notes
    /// - This is the main entry point for expression emission; it delegates to specialized helpers in submodules for
    ///   complex expression kinds.
    pub(super) fn emit_expr(&self, expr: &TypedExpr) -> Result<TokenStream, EmitError> {
        match &expr.kind {
            IrExprKind::Unit => Ok(quote! { () }),
            IrExprKind::None => match &expr.ty {
                IrType::Option(inner) => {
                    let inner_ty = self.emit_type(inner);
                    Ok(quote! { None::<#inner_ty> })
                }
                _ => Ok(quote! { None }),
            },
            IrExprKind::Bool(b) => Ok(if *b {
                quote! { true }
            } else {
                quote! { false }
            }),
            IrExprKind::Int(n) => {
                // Emit integers without suffix to let Rust infer the type
                let lit = if *n >= 0 {
                    Literal::u64_unsuffixed(*n as u64)
                } else {
                    Literal::i64_unsuffixed(*n)
                };
                Ok(lit.to_token_stream())
            }
            IrExprKind::IntLiteral(repr) => repr
                .replace('_', "")
                .parse::<TokenStream>()
                .map_err(|err| EmitError::SynParse(format!("invalid integer literal `{repr}`: {err}"))),
            IrExprKind::Float(n) => Ok(quote! { #n }),
            IrExprKind::Decimal(repr) => Ok(quote! { incan_stdlib::num::Decimal128::from_literal(#repr) }),
            IrExprKind::String(s) => Ok(quote! { #s }),
            IrExprKind::Bytes(bytes) => {
                let lit = Literal::byte_string(bytes);
                if matches!(expr.ty, IrType::StaticBytes | IrType::FrozenBytes) {
                    Ok(lit.to_token_stream())
                } else {
                    Ok(quote! { #lit.to_vec() })
                }
            }

            IrExprKind::Var {
                name,
                access: _,
                ref_kind: VarRefKind::StaticBinding,
            } => {
                let n = Self::rust_ident(name);
                Ok(quote! { #n.get() })
            }
            IrExprKind::Var { name, access: _, .. } => {
                if *self.qualify_internal_canonical_paths.borrow()
                    && let Some(path) = self.emit_dependency_value_path(name)
                {
                    return Ok(path);
                }
                let n = Self::rust_ident(name);
                Ok(quote! { #n })
            }

            IrExprKind::StaticRead { name } => {
                let n = Self::rust_static_ident(name);
                if *self.in_static_initializer.borrow() && !self.static_needs_imported_init_call(name) {
                    Ok(quote! { #n.get() })
                } else {
                    let init_call = self.emit_static_init_call_for_static(name);
                    Ok(quote! {{
                        #init_call
                        #n.get()
                    }})
                }
            }

            IrExprKind::StaticBinding { name } => {
                let n = Self::rust_static_ident(name);
                if *self.in_static_initializer.borrow() && !self.static_needs_imported_init_call(name) {
                    Ok(quote! { incan_stdlib::storage::StaticBinding::from_static(&#n) })
                } else {
                    let init_call = self.emit_static_init_call_for_static(name);
                    Ok(quote! {{
                        #init_call
                        incan_stdlib::storage::StaticBinding::from_static(&#n)
                    }})
                }
            }

            IrExprKind::AssociatedFunction {
                type_name,
                function_name,
            } => {
                let type_ident = Self::rust_ident(type_name);
                let function_ident = Self::rust_ident(function_name);
                Ok(quote! { #type_ident :: #function_ident })
            }

            IrExprKind::FunctionItem { name, type_args } => {
                let ident = Self::rust_ident(name);
                if type_args.is_empty() {
                    Ok(quote! { #ident })
                } else {
                    let args: Vec<_> = type_args.iter().map(|ty| self.emit_type(ty)).collect();
                    Ok(quote! { #ident :: < #(#args),* > })
                }
            }

            IrExprKind::RegisterCallableName { callable, source_name } => {
                self.emit_register_callable_name(callable, source_name)
            }

            IrExprKind::CacheGenericDecoratedFunction {
                cache_name,
                type_param_names,
                value,
            } => self.emit_cache_generic_decorated_function(cache_name, type_param_names, value),

            IrExprKind::BinOp { op, left, right } => self.emit_binop_expr(op, left, right),

            IrExprKind::UnaryOp { op, operand } => {
                let o = self.emit_expr(operand)?;
                match op {
                    UnaryOp::Neg => Ok(quote! { -#o }),
                    UnaryOp::Not => Ok(quote! { !#o }),
                    UnaryOp::Deref => Ok(quote! { *#o }),
                    UnaryOp::Ref => Ok(quote! { (&#o) }),
                    UnaryOp::RefMut => Ok(quote! { (&mut #o) }),
                }
            }

            IrExprKind::Call {
                func,
                type_args,
                args,
                callable_signature,
                canonical_path,
            } => self.emit_call_expr(
                func,
                type_args,
                args,
                callable_signature.as_ref(),
                canonical_path.as_deref(),
            ),
            IrExprKind::BuiltinCall { func, args } => self.emit_builtin_call(func, args),
            IrExprKind::MethodCall {
                receiver,
                method,
                dispatch,
                type_args,
                args,
                callable_signature,
                arg_policy,
            } => self.emit_method_call_expr(
                receiver,
                method,
                dispatch.as_ref(),
                type_args,
                args,
                callable_signature.as_ref(),
                *arg_policy,
            ),
            IrExprKind::KnownMethodCall { receiver, kind, args } => self.emit_known_method_call(receiver, kind, args),

            IrExprKind::Field { object, field } => self.emit_field_expr(object, field),
            IrExprKind::Index { object, index } => self.emit_index_expr(object, index),
            IrExprKind::Slice {
                target,
                start,
                end,
                step,
            } => self.emit_slice_expr(target, start, end, step),

            IrExprKind::ListComp {
                element,
                pattern,
                iterable,
                filter,
            } => self.emit_list_comp(element, pattern, iterable, filter.as_deref()),
            IrExprKind::DictComp {
                key,
                value,
                pattern,
                iterable,
                filter,
            } => self.emit_dict_comp(key, value, pattern, iterable, filter.as_deref()),
            IrExprKind::Generator { element, clauses } => self.emit_generator_expr(element, clauses),

            IrExprKind::List(items) => {
                let item_target_ty = match &expr.ty {
                    IrType::List(elem) => Some(elem.as_ref()),
                    _ => None,
                };
                self.emit_list_literal_entries(items, item_target_ty)
            }

            IrExprKind::Dict(pairs) => {
                let (key_target_ty, value_target_ty) = match &expr.ty {
                    IrType::Dict(key, value) => (Some(key.as_ref()), Some(value.as_ref())),
                    _ => (None, None),
                };
                self.emit_dict_literal_entries(pairs, key_target_ty, value_target_ty)
            }

            IrExprKind::Set(items) => {
                if items.is_empty() {
                    Ok(quote! { std::collections::HashSet::new() })
                } else {
                    let item_target_ty = match &expr.ty {
                        IrType::Set(elem) => Some(elem.as_ref()),
                        _ => None,
                    };
                    let item_tokens: Vec<TokenStream> = items
                        .iter()
                        .map(|i| {
                            self.emit_expr_for_use(
                                i,
                                ValueUseSite::CollectionElement {
                                    target_ty: item_target_ty,
                                },
                            )
                        })
                        .collect::<Result<_, _>>()?;
                    Ok(quote! { [#(#item_tokens),*].into_iter().collect::<std::collections::HashSet<_>>() })
                }
            }

            IrExprKind::Tuple(items) => {
                let tuple_target_items = match &expr.ty {
                    IrType::Tuple(items) => Some(items.as_slice()),
                    _ => None,
                };
                let item_tokens: Vec<TokenStream> = items
                    .iter()
                    .enumerate()
                    .map(|(idx, item)| {
                        let item_target_ty = tuple_target_items.and_then(|items| items.get(idx));
                        self.emit_expr_for_use(
                            item,
                            ValueUseSite::CollectionElement {
                                target_ty: item_target_ty,
                            },
                        )
                    })
                    .collect::<Result<_, _>>()?;
                Ok(quote! { (#(#item_tokens),*) })
            }

            IrExprKind::Struct { name, fields } => self.emit_struct_expr(name, fields),

            IrExprKind::If {
                condition,
                then_branch,
                else_branch,
            } => {
                let c = self.emit_expr(condition)?;
                let t = self.emit_expr(then_branch)?;
                if let Some(e) = else_branch {
                    let ee = self.emit_expr(e)?;
                    Ok(quote! { if #c { #t } else { #ee } })
                } else {
                    Ok(quote! { if #c { #t } })
                }
            }

            IrExprKind::Match { scrutinee, arms } => {
                let s = self.emit_match_scrutinee(scrutinee)?;
                let arm_tokens: Vec<TokenStream> = arms
                    .iter()
                    .map(|arm| {
                        let (pat, pattern_guard) = self.emit_pattern_for_scrutinee(&arm.pattern, &scrutinee.ty);
                        let body = self.emit_expr(&arm.body)?;
                        let guard = match (&pattern_guard, &arm.guard) {
                            (Some(pattern_guard), Some(arm_guard)) => {
                                let arm_guard = self.emit_expr(arm_guard)?;
                                Some(quote! { (#pattern_guard) && (#arm_guard) })
                            }
                            (Some(pattern_guard), None) => Some(pattern_guard.clone()),
                            (None, Some(arm_guard)) => Some(self.emit_expr(arm_guard)?),
                            (None, None) => None,
                        };
                        if let Some(guard) = guard {
                            Ok(quote! { #pat if #guard => #body })
                        } else {
                            Ok(quote! { #pat => #body })
                        }
                    })
                    .collect::<Result<_, _>>()?;
                Ok(quote! {
                    match #s {
                        #(#arm_tokens),*
                    }
                })
            }

            IrExprKind::Closure {
                params,
                body,
                captures: _,
            } => {
                let param_tokens: Vec<TokenStream> = params
                    .iter()
                    .map(|(pname, pty)| {
                        let n = Self::rust_ident(pname);
                        if matches!(pty, IrType::RustDisplay(_)) {
                            let ty = self.emit_type(pty);
                            quote! { #n: #ty }
                        } else {
                            quote! { #n }
                        }
                    })
                    .collect();
                let b = self.emit_expr(body)?;
                Ok(quote! { |#(#param_tokens),*| #b })
            }

            IrExprKind::Block { stmts, value } => {
                if let Some(v) = value {
                    let stmt_tokens = self.emit_stmts_before_expr(stmts, v)?;
                    let vv = self.emit_expr(v)?;
                    Ok(quote! {
                        {
                            #(#stmt_tokens)*
                            #vv
                        }
                    })
                } else {
                    let stmt_tokens = self.emit_stmts(stmts)?;
                    Ok(quote! {
                        {
                            #(#stmt_tokens)*
                        }
                    })
                }
            }

            IrExprKind::Loop { body } => {
                let body_tokens = self.emit_stmts(body)?;
                Ok(quote! {
                    loop {
                        #(#body_tokens)*
                    }
                })
            }

            IrExprKind::Await(inner) => {
                let i = self.emit_expr(inner)?;
                Ok(quote! { #i.await })
            }

            IrExprKind::Race { binding, arms } => {
                let binding_ident = format_ident!("{}", binding);
                let mut branch_tokens = Vec::with_capacity(arms.len());
                for arm in arms {
                    let awaitable = self.emit_expr(&arm.awaitable)?;
                    let body = self.emit_expr(&arm.body)?;
                    branch_tokens.push(quote! {
                        incan_stdlib::r#async::race::scoped_arm(#awaitable, |#binding_ident| #body)
                    });
                }
                Ok(quote! {
                    incan_stdlib::r#async::race::scoped_race(vec![#(#branch_tokens),*]).await
                })
            }

            IrExprKind::Try(inner) => {
                let i = if let Some(inner_target_ty) = self.try_inner_target_type(&expr.ty, inner) {
                    self.emit_expr_for_use(
                        inner,
                        ValueUseSite::Assignment {
                            target_ty: Some(&inner_target_ty),
                        },
                    )?
                } else {
                    self.emit_expr(inner)?
                };
                Ok(quote! { #i? })
            }

            IrExprKind::Range { start, end, inclusive } => {
                self.emit_range_expr(start.as_deref(), end.as_deref(), *inclusive)
            }

            IrExprKind::Cast { expr, to_type } => {
                let e = self.emit_expr(expr)?;
                let t = self.emit_type(to_type);
                Ok(quote! { #e as #t })
            }

            IrExprKind::NumericResize {
                expr: inner,
                policy,
                to_type,
            } => {
                let e = self.emit_expr(inner)?;
                let t = self.emit_type(to_type);
                match policy {
                    NumericResizePolicy::Lossless | NumericResizePolicy::Wrapping => Ok(quote! { (#e) as #t }),
                    NumericResizePolicy::Try => Ok(quote! { incan_stdlib::num::try_resize::<_, #t>(#e) }),
                    NumericResizePolicy::Saturating => Ok(quote! { incan_stdlib::num::saturating_resize::<_, #t>(#e) }),
                }
            }

            IrExprKind::InteropCoerce {
                expr: inner,
                from_ty: _,
                to_ty,
                kind,
            } => {
                let inner_tokens = self.emit_expr(inner)?;
                match kind {
                    IrInteropCoercionKind::Builtin { policy, rust_target } => {
                        let emitted = match policy {
                            incan_core::interop::CoercionPolicy::Exact => match to_ty {
                                IrType::String => quote! { (#inner_tokens).to_string() },
                                IrType::Bytes => quote! { (#inner_tokens).to_vec() },
                                _ => quote! { #inner_tokens },
                            },
                            incan_core::interop::CoercionPolicy::Lossless => {
                                let target = self.emit_type(to_ty);
                                let _: syn::Type = syn::parse2(target.clone()).map_err(|err| {
                                    EmitError::SynParse(format!(
                                        "invalid Rust boundary cast target `{rust_target}`: {err}"
                                    ))
                                })?;
                                quote! { (#inner_tokens) as #target }
                            }
                            incan_core::interop::CoercionPolicy::Borrow => {
                                interop_coercions::emit_builtin_borrow_coercion(inner, inner_tokens, to_ty)
                            }
                            incan_core::interop::CoercionPolicy::Lossy => match rust_target.as_str() {
                                "f32" => quote! { (#inner_tokens) as f32 },
                                _ => quote! { #inner_tokens },
                            },
                        };
                        Ok(emitted)
                    }
                    IrInteropCoercionKind::AdapterCall { adapter, adapter_kind } => {
                        let adapter = self.emit_expr(adapter)?;
                        let call = quote! { #adapter(#inner_tokens) };
                        let emitted = match adapter_kind {
                            IrInteropAdapterKind::Via => call,
                            IrInteropAdapterKind::Try => quote! { #call? },
                        };
                        Ok(emitted)
                    }
                    IrInteropCoercionKind::RustTypeUnwrap => Ok(quote! { #inner_tokens }),
                }
            }

            IrExprKind::Format { parts } => self.emit_format_expr(parts),

            IrExprKind::Literal(lit) => match lit {
                IrLiteral::StaticStr(s) => Ok(quote! { #s }),
            },

            IrExprKind::FieldsList(fields) => Ok(quote! { vec![#(#fields),*] }),

            IrExprKind::SerdeToJson => {
                Ok(quote! { incan_stdlib::json::__private::stringify_or_raise(self, std::any::type_name::<Self>()) })
            }

            IrExprKind::SerdeFromJson(type_name) => {
                let type_ident = format_ident!("{}", type_name);
                Ok(quote! {
                    incan_stdlib::json::__private::parse_or_error::<#type_ident>(&s)
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::ir::expr::{
        CollectionMethodKind, IrCallArg, IrCallArgKind, IteratorMethodKind, MethodCallArgPolicy, MethodKind, VarAccess,
        VarRefKind,
    };
    use crate::backend::ir::{FunctionParam, FunctionRegistry, FunctionSignature, Mutability};
    use incan_core::lang::traits::{self as core_traits, TraitId};

    fn prost_decode_signature(return_type: IrType) -> FunctionSignature {
        FunctionSignature {
            params: vec![FunctionParam {
                name: "buf".to_string(),
                ty: IrType::Generic("Buf".to_string()),
                mutability: Mutability::Immutable,
                is_self: false,
                kind: crate::frontend::ast::ParamKind::Normal,
                default: None,
            }],
            return_type,
        }
    }

    #[test]
    fn type_name_associated_call_does_not_borrow_string_arguments() -> Result<(), String> {
        let registry = FunctionRegistry::new();
        let emitter = IrEmitter::new(&registry);
        let expr = TypedExpr::new(
            IrExprKind::MethodCall {
                receiver: Box::new(TypedExpr::new(
                    IrExprKind::Var {
                        name: "Response".to_string(),
                        access: VarAccess::Move,
                        ref_kind: VarRefKind::TypeName,
                    },
                    IrType::Struct("Response".to_string()),
                )),
                method: "html".to_string(),
                dispatch: None,
                type_args: Vec::new(),
                args: vec![IrCallArg {
                    name: None,
                    kind: IrCallArgKind::Positional,
                    expr: TypedExpr::new(
                        IrExprKind::Var {
                            name: "html".to_string(),
                            access: VarAccess::Move,
                            ref_kind: VarRefKind::Value,
                        },
                        IrType::String,
                    ),
                }],
                callable_signature: None,
                arg_policy: MethodCallArgPolicy::Default,
            },
            IrType::Struct("Response".to_string()),
        );

        let emitted = emitter
            .emit_expr(&expr)
            .map_err(|err| format!("expected successful expression emission, got {err:?}"))?;
        let rendered = emitted.to_string();
        assert!(
            rendered.contains("Response") && rendered.contains("html"),
            "expected associated call emission, got `{rendered}`"
        );
        assert!(
            !rendered.contains("& html") && !rendered.contains("&html"),
            "TypeName associated calls use Incan arg rules (owned String), got `{rendered}`"
        );
        Ok(())
    }

    #[test]
    fn external_method_call_uses_by_value_signature_for_field_argument() -> Result<(), String> {
        let registry = FunctionRegistry::new();
        let emitter = IrEmitter::new(&registry);
        let rust_duration = IrType::Struct("std::time::Duration".to_string());
        let expr = TypedExpr::new(
            IrExprKind::MethodCall {
                receiver: Box::new(TypedExpr::new(
                    IrExprKind::Field {
                        object: Box::new(TypedExpr::new(
                            IrExprKind::Var {
                                name: "self".to_string(),
                                access: VarAccess::Read,
                                ref_kind: VarRefKind::Value,
                            },
                            IrType::Struct("Duration".to_string()),
                        )),
                        field: "value".to_string(),
                    },
                    rust_duration.clone(),
                )),
                method: "saturating_add".to_string(),
                dispatch: None,
                type_args: Vec::new(),
                args: vec![IrCallArg {
                    name: None,
                    kind: IrCallArgKind::Positional,
                    expr: TypedExpr::new(
                        IrExprKind::Field {
                            object: Box::new(TypedExpr::new(
                                IrExprKind::Var {
                                    name: "other".to_string(),
                                    access: VarAccess::Read,
                                    ref_kind: VarRefKind::Value,
                                },
                                IrType::Struct("Duration".to_string()),
                            )),
                            field: "value".to_string(),
                        },
                        rust_duration.clone(),
                    ),
                }],
                callable_signature: Some(FunctionSignature {
                    params: vec![FunctionParam {
                        name: "rhs".to_string(),
                        ty: rust_duration.clone(),
                        mutability: Mutability::Immutable,
                        is_self: false,
                        kind: crate::frontend::ast::ParamKind::Normal,
                        default: None,
                    }],
                    return_type: rust_duration,
                }),
                arg_policy: MethodCallArgPolicy::Default,
            },
            IrType::Struct("std::time::Duration".to_string()),
        );

        let emitted = emitter
            .emit_expr(&expr)
            .map_err(|err| format!("expected successful expression emission, got {err:?}"))?;
        let rendered = emitted.to_string();
        assert!(
            rendered.contains("saturating_add") && rendered.contains("other . value"),
            "expected direct field argument in external method call, got `{rendered}`"
        );
        assert!(
            !rendered.contains("& other . value") && !rendered.contains("&other . value"),
            "by-value Rust method params must not borrow field arguments, got `{rendered}`"
        );
        Ok(())
    }

    #[test]
    fn external_decode_with_generic_by_value_signature_does_not_borrow_argument() -> Result<(), String> {
        let registry = FunctionRegistry::new();
        let emitter = IrEmitter::new(&registry);
        let descriptor_set = IrType::Struct("prost_types::FileDescriptorSet".to_string());
        let expr = TypedExpr::new(
            IrExprKind::MethodCall {
                receiver: Box::new(TypedExpr::new(
                    IrExprKind::Var {
                        name: "FileDescriptorSet".to_string(),
                        access: VarAccess::Copy,
                        ref_kind: VarRefKind::ExternalRustName,
                    },
                    descriptor_set.clone(),
                )),
                method: "decode".to_string(),
                dispatch: None,
                type_args: Vec::new(),
                args: vec![IrCallArg {
                    name: None,
                    kind: IrCallArgKind::Positional,
                    expr: TypedExpr::new(
                        IrExprKind::Var {
                            name: "cursor".to_string(),
                            access: VarAccess::Move,
                            ref_kind: VarRefKind::Value,
                        },
                        IrType::Struct("std::io::Cursor<Vec<u8>>".to_string()),
                    ),
                }],
                callable_signature: Some(FunctionSignature {
                    params: vec![FunctionParam {
                        name: "buf".to_string(),
                        ty: IrType::Generic("Buf".to_string()),
                        mutability: Mutability::Immutable,
                        is_self: false,
                        kind: crate::frontend::ast::ParamKind::Normal,
                        default: None,
                    }],
                    return_type: IrType::Result(
                        Box::new(descriptor_set.clone()),
                        Box::new(IrType::Struct("prost::DecodeError".to_string())),
                    ),
                }),
                arg_policy: MethodCallArgPolicy::Default,
            },
            IrType::Result(
                Box::new(descriptor_set),
                Box::new(IrType::Struct("prost::DecodeError".to_string())),
            ),
        );

        let emitted = emitter
            .emit_expr(&expr)
            .map_err(|err| format!("expected successful expression emission, got {err:?}"))?;
        let rendered = emitted.to_string();
        assert!(
            rendered.contains("FileDescriptorSet :: decode (cursor)"),
            "generic by-value Rust method params must pass the value directly, got `{rendered}`"
        );
        assert!(
            !rendered.contains("FileDescriptorSet :: decode (& cursor)"),
            "generic by-value Rust method params must not borrow the argument, got `{rendered}`"
        );
        Ok(())
    }

    #[test]
    fn external_decode_metadata_keeps_explicit_slice_argument_shape() -> Result<(), String> {
        let registry = FunctionRegistry::new();
        let emitter = IrEmitter::new(&registry);
        let descriptor_set = IrType::Struct("prost_types::FileDescriptorSet".to_string());
        let result_ty = IrType::Result(
            Box::new(descriptor_set.clone()),
            Box::new(IrType::Struct("prost::DecodeError".to_string())),
        );
        let expr = TypedExpr::new(
            IrExprKind::MethodCall {
                receiver: Box::new(TypedExpr::new(
                    IrExprKind::Var {
                        name: "FileDescriptorSet".to_string(),
                        access: VarAccess::Copy,
                        ref_kind: VarRefKind::ExternalRustName,
                    },
                    descriptor_set.clone(),
                )),
                method: "decode".to_string(),
                dispatch: None,
                type_args: Vec::new(),
                args: vec![IrCallArg {
                    name: None,
                    kind: IrCallArgKind::Positional,
                    expr: TypedExpr::new(
                        IrExprKind::MethodCall {
                            receiver: Box::new(TypedExpr::new(
                                IrExprKind::Var {
                                    name: "data".to_string(),
                                    access: VarAccess::Read,
                                    ref_kind: VarRefKind::Value,
                                },
                                IrType::Bytes,
                            )),
                            method: "as_slice".to_string(),
                            dispatch: None,
                            type_args: Vec::new(),
                            args: Vec::new(),
                            callable_signature: None,
                            arg_policy: MethodCallArgPolicy::Default,
                        },
                        IrType::Bytes,
                    ),
                }],
                callable_signature: Some(prost_decode_signature(result_ty.clone())),
                arg_policy: MethodCallArgPolicy::Default,
            },
            result_ty,
        );

        let emitted = emitter
            .emit_expr(&expr)
            .map_err(|err| format!("expected successful expression emission, got {err:?}"))?;
        let rendered = emitted.to_string();
        assert!(
            rendered.contains("FileDescriptorSet :: decode (data . as_slice ())"),
            "explicit slice arguments should be passed through, got `{rendered}`"
        );
        assert!(
            !rendered.contains("FileDescriptorSet :: decode (& data . as_slice ())"),
            "decode metadata must not add a fallback borrow to explicit slice arguments, got `{rendered}`"
        );
        Ok(())
    }

    #[test]
    fn external_decode_metadata_keeps_explicit_rust_vec_slice_argument_shape() -> Result<(), String> {
        let registry = FunctionRegistry::new();
        let emitter = IrEmitter::new(&registry);
        let descriptor_set = IrType::Struct("prost_types::FileDescriptorSet".to_string());
        let result_ty = IrType::Result(
            Box::new(descriptor_set.clone()),
            Box::new(IrType::Struct("prost::DecodeError".to_string())),
        );
        let expr = TypedExpr::new(
            IrExprKind::MethodCall {
                receiver: Box::new(TypedExpr::new(
                    IrExprKind::Var {
                        name: "FileDescriptorSet".to_string(),
                        access: VarAccess::Copy,
                        ref_kind: VarRefKind::ExternalRustName,
                    },
                    descriptor_set.clone(),
                )),
                method: "decode".to_string(),
                dispatch: None,
                type_args: Vec::new(),
                args: vec![IrCallArg {
                    name: None,
                    kind: IrCallArgKind::Positional,
                    expr: TypedExpr::new(
                        IrExprKind::MethodCall {
                            receiver: Box::new(TypedExpr::new(
                                IrExprKind::Var {
                                    name: "encoded".to_string(),
                                    access: VarAccess::Read,
                                    ref_kind: VarRefKind::Value,
                                },
                                IrType::Struct("alloc::vec::Vec<u8>".to_string()),
                            )),
                            method: "as_slice".to_string(),
                            dispatch: None,
                            type_args: Vec::new(),
                            args: Vec::new(),
                            callable_signature: None,
                            arg_policy: MethodCallArgPolicy::Default,
                        },
                        IrType::Bytes,
                    ),
                }],
                callable_signature: Some(prost_decode_signature(result_ty.clone())),
                arg_policy: MethodCallArgPolicy::Default,
            },
            result_ty,
        );

        let emitted = emitter
            .emit_expr(&expr)
            .map_err(|err| format!("expected successful expression emission, got {err:?}"))?;
        let rendered = emitted.to_string();
        assert!(
            rendered.contains("FileDescriptorSet :: decode (encoded . as_slice ())"),
            "explicit Rust Vec slice arguments should be passed through, got `{rendered}`"
        );
        assert!(
            !rendered.contains("FileDescriptorSet :: decode (& encoded . as_slice ())"),
            "decode metadata must not add a fallback borrow to explicit Rust Vec slice arguments, got `{rendered}`"
        );
        Ok(())
    }

    #[test]
    fn external_decode_fallback_still_borrows_owned_bytes_argument() -> Result<(), String> {
        let registry = FunctionRegistry::new();
        let emitter = IrEmitter::new(&registry);
        let descriptor_set = IrType::Struct("prost_types::FileDescriptorSet".to_string());
        let expr = TypedExpr::new(
            IrExprKind::MethodCall {
                receiver: Box::new(TypedExpr::new(
                    IrExprKind::Var {
                        name: "FileDescriptorSet".to_string(),
                        access: VarAccess::Copy,
                        ref_kind: VarRefKind::ExternalRustName,
                    },
                    descriptor_set.clone(),
                )),
                method: "decode".to_string(),
                dispatch: None,
                type_args: Vec::new(),
                args: vec![IrCallArg {
                    name: None,
                    kind: IrCallArgKind::Positional,
                    expr: TypedExpr::new(
                        IrExprKind::Var {
                            name: "data".to_string(),
                            access: VarAccess::Read,
                            ref_kind: VarRefKind::Value,
                        },
                        IrType::Bytes,
                    ),
                }],
                callable_signature: None,
                arg_policy: MethodCallArgPolicy::Default,
            },
            IrType::Result(
                Box::new(descriptor_set),
                Box::new(IrType::Struct("prost::DecodeError".to_string())),
            ),
        );

        let emitted = emitter
            .emit_expr(&expr)
            .map_err(|err| format!("expected successful expression emission, got {err:?}"))?;
        let rendered = emitted.to_string();
        assert!(
            rendered.contains("FileDescriptorSet :: decode (& data)"),
            "owned bytes should still use the decode fallback borrow, got `{rendered}`"
        );
        Ok(())
    }

    #[test]
    fn encoding_decode_compatibility_policy_overrides_incomplete_by_value_signature() -> Result<(), String> {
        let registry = FunctionRegistry::new();
        let emitter = IrEmitter::new(&registry);
        let expr = TypedExpr::new(
            IrExprKind::MethodCall {
                receiver: Box::new(TypedExpr::new(
                    IrExprKind::Var {
                        name: "enc".to_string(),
                        access: VarAccess::Read,
                        ref_kind: VarRefKind::Value,
                    },
                    IrType::Struct("encoding_rs::Encoding".to_string()),
                )),
                method: "decode".to_string(),
                dispatch: None,
                type_args: Vec::new(),
                args: vec![IrCallArg {
                    name: None,
                    kind: IrCallArgKind::Positional,
                    expr: TypedExpr::new(
                        IrExprKind::Var {
                            name: "data".to_string(),
                            access: VarAccess::Read,
                            ref_kind: VarRefKind::Value,
                        },
                        IrType::Bytes,
                    ),
                }],
                callable_signature: Some(FunctionSignature {
                    params: vec![FunctionParam {
                        name: "bytes".to_string(),
                        ty: IrType::Bytes,
                        mutability: Mutability::Immutable,
                        is_self: false,
                        kind: crate::frontend::ast::ParamKind::Normal,
                        default: None,
                    }],
                    return_type: IrType::Unknown,
                }),
                arg_policy: MethodCallArgPolicy::Default,
            },
            IrType::Unknown,
        );

        let emitted = emitter
            .emit_expr(&expr)
            .map_err(|err| format!("expected successful expression emission, got {err:?}"))?;
        let rendered = emitted.to_string();
        assert!(
            rendered.contains("enc . decode (& data)"),
            "encoding_rs decode should borrow bytes even when the recovered signature is incomplete, got `{rendered}`"
        );
        Ok(())
    }

    #[test]
    fn unregistered_decode_method_with_by_value_metadata_preserves_argument_shape() -> Result<(), String> {
        let registry = FunctionRegistry::new();
        let emitter = IrEmitter::new(&registry);
        let expr = TypedExpr::new(
            IrExprKind::MethodCall {
                receiver: Box::new(TypedExpr::new(
                    IrExprKind::Var {
                        name: "decoder".to_string(),
                        access: VarAccess::Read,
                        ref_kind: VarRefKind::Value,
                    },
                    IrType::Struct("ExternalDecoder".to_string()),
                )),
                method: "decode".to_string(),
                dispatch: None,
                type_args: Vec::new(),
                args: vec![IrCallArg {
                    name: None,
                    kind: IrCallArgKind::Positional,
                    expr: TypedExpr::new(
                        IrExprKind::Var {
                            name: "data".to_string(),
                            access: VarAccess::Read,
                            ref_kind: VarRefKind::Value,
                        },
                        IrType::Bytes,
                    ),
                }],
                callable_signature: Some(FunctionSignature {
                    params: vec![FunctionParam {
                        name: "data".to_string(),
                        ty: IrType::Bytes,
                        mutability: Mutability::Immutable,
                        is_self: false,
                        kind: crate::frontend::ast::ParamKind::Normal,
                        default: None,
                    }],
                    return_type: IrType::Unknown,
                }),
                arg_policy: MethodCallArgPolicy::Default,
            },
            IrType::Unknown,
        );

        let emitted = emitter
            .emit_expr(&expr)
            .map_err(|err| format!("expected successful expression emission, got {err:?}"))?;
        let rendered = emitted.to_string();
        assert!(
            rendered.contains("decoder . decode (data)"),
            "explicit by-value metadata must preserve argument shape, got `{rendered}`"
        );
        assert!(
            !rendered.contains("decoder . decode (& data)") && !rendered.contains("decoder.decode(&data)"),
            "explicit by-value metadata must not use the metadata-free byte borrow default, got `{rendered}`"
        );
        Ok(())
    }

    #[test]
    fn metadata_free_read_to_string_fallback_requires_string_buffer() -> Result<(), String> {
        let registry = FunctionRegistry::new();
        let emitter = IrEmitter::new(&registry);
        let expr = TypedExpr::new(
            IrExprKind::MethodCall {
                receiver: Box::new(TypedExpr::new(
                    IrExprKind::Var {
                        name: "reader".to_string(),
                        access: VarAccess::Read,
                        ref_kind: VarRefKind::Value,
                    },
                    IrType::Struct("ExternalReader".to_string()),
                )),
                method: "read_to_string".to_string(),
                dispatch: None,
                type_args: Vec::new(),
                args: vec![IrCallArg {
                    name: None,
                    kind: IrCallArgKind::Positional,
                    expr: TypedExpr::new(
                        IrExprKind::Var {
                            name: "count".to_string(),
                            access: VarAccess::Read,
                            ref_kind: VarRefKind::Value,
                        },
                        IrType::Int,
                    ),
                }],
                callable_signature: None,
                arg_policy: MethodCallArgPolicy::Default,
            },
            IrType::Unknown,
        );

        let emitted = emitter
            .emit_expr(&expr)
            .map_err(|err| format!("expected successful expression emission, got {err:?}"))?;
        let rendered = emitted.to_string();
        assert!(
            rendered.contains("reader . read_to_string (count)"),
            "read_to_string fallback should preserve non-string argument shape, got `{rendered}`"
        );
        assert!(
            !rendered.contains("reader . read_to_string (& mut count)")
                && !rendered.contains("reader.read_to_string(&mut count)"),
            "read_to_string fallback must not mutably borrow non-string arguments, got `{rendered}`"
        );
        Ok(())
    }

    #[test]
    fn interop_try_adapter_emits_question_mark() -> Result<(), String> {
        let registry = FunctionRegistry::new();
        let emitter = IrEmitter::new(&registry);
        let expr = TypedExpr::new(
            IrExprKind::InteropCoerce {
                expr: Box::new(TypedExpr::new(
                    IrExprKind::Var {
                        name: "value".to_string(),
                        access: VarAccess::Read,
                        ref_kind: VarRefKind::Value,
                    },
                    IrType::String,
                )),
                from_ty: IrType::String,
                to_ty: IrType::Struct("Email".to_string()),
                kind: IrInteropCoercionKind::AdapterCall {
                    adapter: Box::new(TypedExpr::new(
                        IrExprKind::Var {
                            name: "email_parse".to_string(),
                            access: VarAccess::Read,
                            ref_kind: VarRefKind::Value,
                        },
                        IrType::Unknown,
                    )),
                    adapter_kind: IrInteropAdapterKind::Try,
                },
            },
            IrType::Struct("Email".to_string()),
        );

        let emitted = emitter
            .emit_expr(&expr)
            .map_err(|err| format!("expected successful expression emission, got {err:?}"))?;
        assert!(
            emitted.to_string().contains('?'),
            "expected try-adapter emission to include `?`, got `{}`",
            emitted
        );
        Ok(())
    }

    #[test]
    fn interop_borrowed_string_coercion_materializes_owned_string_before_borrow() -> Result<(), String> {
        let registry = FunctionRegistry::new();
        let emitter = IrEmitter::new(&registry);
        let expr = TypedExpr::new(
            IrExprKind::InteropCoerce {
                expr: Box::new(TypedExpr::new(
                    IrExprKind::String("payload".to_string()),
                    IrType::String,
                )),
                from_ty: IrType::String,
                to_ty: IrType::Ref(Box::new(IrType::Struct("String".to_string()))),
                kind: IrInteropCoercionKind::Builtin {
                    policy: incan_core::interop::CoercionPolicy::Borrow,
                    rust_target: "&str".to_string(),
                },
            },
            IrType::Ref(Box::new(IrType::Struct("String".to_string()))),
        );

        let emitted = emitter
            .emit_expr(&expr)
            .map_err(|err| format!("expected successful expression emission, got {err:?}"))?;
        let rendered = emitted.to_string();
        assert!(
            rendered.contains(". to_string ()"),
            "expected borrowed String interop coercion to materialize an owned String, got `{rendered}`"
        );
        assert!(
            rendered.starts_with("&"),
            "expected borrowed String interop coercion to emit a borrow, got `{rendered}`"
        );
        Ok(())
    }

    #[test]
    fn interop_borrowed_string_coercion_borrows_owned_string_without_materializing() -> Result<(), String> {
        let registry = FunctionRegistry::new();
        let emitter = IrEmitter::new(&registry);
        let expr = TypedExpr::new(
            IrExprKind::InteropCoerce {
                expr: Box::new(TypedExpr::new(
                    IrExprKind::Var {
                        name: "text".to_string(),
                        access: VarAccess::Read,
                        ref_kind: VarRefKind::Value,
                    },
                    IrType::String,
                )),
                from_ty: IrType::String,
                to_ty: IrType::Ref(Box::new(IrType::Struct("String".to_string()))),
                kind: IrInteropCoercionKind::Builtin {
                    policy: incan_core::interop::CoercionPolicy::Borrow,
                    rust_target: "&str".to_string(),
                },
            },
            IrType::Ref(Box::new(IrType::Struct("String".to_string()))),
        );

        let emitted = emitter
            .emit_expr(&expr)
            .map_err(|err| format!("expected successful expression emission, got {err:?}"))?;
        let rendered = emitted.to_string();
        assert!(
            rendered == "& text" || rendered == "&text",
            "expected borrowed owned String interop coercion to borrow directly, got `{rendered}`"
        );
        assert!(
            !rendered.contains("to_string"),
            "owned String borrow coercions must not clone through `.to_string()`, got `{rendered}`"
        );
        Ok(())
    }

    #[test]
    fn interop_structural_list_borrow_coercion_projects_str_items() -> Result<(), String> {
        let registry = FunctionRegistry::new();
        let emitter = IrEmitter::new(&registry);
        let expr = TypedExpr::new(
            IrExprKind::InteropCoerce {
                expr: Box::new(TypedExpr::new(
                    IrExprKind::Var {
                        name: "items".to_string(),
                        access: VarAccess::Read,
                        ref_kind: VarRefKind::Value,
                    },
                    IrType::List(Box::new(IrType::String)),
                )),
                from_ty: IrType::List(Box::new(IrType::String)),
                to_ty: IrType::List(Box::new(IrType::StrRef)),
                kind: IrInteropCoercionKind::Builtin {
                    policy: incan_core::interop::CoercionPolicy::Borrow,
                    rust_target: "Vec<&str>".to_string(),
                },
            },
            IrType::List(Box::new(IrType::StrRef)),
        );

        let emitted = emitter
            .emit_expr(&expr)
            .map_err(|err| format!("expected successful expression emission, got {err:?}"))?;
        let rendered = emitted.to_string();
        assert!(
            rendered.contains("items . iter ()"),
            "expected structural borrow coercion to iterate source list, got `{rendered}`"
        );
        assert!(
            rendered.contains("as_str ()"),
            "expected structural borrow coercion to project string items as &str, got `{rendered}`"
        );
        assert!(
            rendered.contains("collect :: < Vec < _ >> ()"),
            "expected structural borrow coercion to collect a Rust Vec, got `{rendered}`"
        );
        Ok(())
    }

    #[test]
    fn interop_wrapped_dict_literal_keeps_call_site_value_target() -> Result<(), String> {
        let registry = FunctionRegistry::new();
        let emitter = IrEmitter::new(&registry);
        let union_ty = IrType::Option(Box::new(IrType::NamedGeneric(
            crate::backend::ir::types::IR_UNION_TYPE_NAME.to_string(),
            vec![IrType::Bool, IrType::Int],
        )));
        let target_ty = IrType::Dict(Box::new(IrType::String), Box::new(union_ty.clone()));
        let dict = TypedExpr::new(
            IrExprKind::Dict(vec![
                IrDictEntry::Pair(
                    TypedExpr::new(IrExprKind::String("count".to_string()), IrType::String),
                    Box::new(TypedExpr::new(IrExprKind::Int(1), IrType::Int)),
                ),
                IrDictEntry::Pair(
                    TypedExpr::new(IrExprKind::String("ok".to_string()), IrType::String),
                    Box::new(TypedExpr::new(IrExprKind::Bool(true), IrType::Bool)),
                ),
            ]),
            IrType::Dict(Box::new(IrType::String), Box::new(IrType::Int)),
        );
        let expr = TypedExpr::new(
            IrExprKind::InteropCoerce {
                expr: Box::new(dict),
                from_ty: IrType::Dict(Box::new(IrType::String), Box::new(IrType::Int)),
                to_ty: target_ty.clone(),
                kind: IrInteropCoercionKind::RustTypeUnwrap,
            },
            target_ty.clone(),
        );

        let emitted = emitter
            .emit_expr_for_use(
                &expr,
                ValueUseSite::IncanCallArg {
                    target_ty: Some(&target_ty),
                    callee_param: None,
                    in_return: false,
                },
            )
            .map_err(|err| format!("expected successful expression emission, got {err:?}"))?;
        let rendered = emitted.to_string();
        let some_constructor = incan_core::lang::surface::constructors::as_str(
            incan_core::lang::surface::constructors::ConstructorId::Some,
        );
        assert!(
            rendered.contains(some_constructor) && rendered.contains("__IncanUnion"),
            "expected target union wrapping to survive interop aggregate wrapper, got `{rendered}`"
        );
        Ok(())
    }

    #[test]
    fn generic_collection_literal_target_uses_inferred_tuple_item_types() -> Result<(), String> {
        let registry = FunctionRegistry::new();
        let emitter = IrEmitter::new(&registry);
        let inferred_tuple_ty = IrType::Tuple(vec![IrType::String, IrType::Int]);
        let target_ty = IrType::List(Box::new(IrType::Tuple(vec![
            IrType::Generic("K".to_string()),
            IrType::Generic("V".to_string()),
        ])));
        let expr = TypedExpr::new(
            IrExprKind::List(vec![IrListEntry::Element(TypedExpr::new(
                IrExprKind::Tuple(vec![
                    TypedExpr::new(IrExprKind::String("host".to_string()), IrType::String),
                    TypedExpr::new(IrExprKind::Int(1), IrType::Int),
                ]),
                inferred_tuple_ty,
            ))]),
            IrType::List(Box::new(IrType::Tuple(vec![IrType::String, IrType::Int]))),
        );

        let emitted = emitter
            .emit_expr_for_use(
                &expr,
                ValueUseSite::IncanCallArg {
                    target_ty: Some(&target_ty),
                    callee_param: None,
                    in_return: false,
                },
            )
            .map_err(|err| format!("expected successful expression emission, got {err:?}"))?;
        let rendered = emitted.to_string();
        assert!(
            rendered.contains("\"host\" . to_string ()") || rendered.contains("\"host\".to_string()"),
            "expected generic collection target to preserve concrete string item conversion, got `{rendered}`"
        );
        Ok(())
    }

    #[test]
    fn interop_borrowed_bytes_slice_coercion_borrows_owned_bytes() -> Result<(), String> {
        let registry = FunctionRegistry::new();
        let emitter = IrEmitter::new(&registry);
        let expr = TypedExpr::new(
            IrExprKind::InteropCoerce {
                expr: Box::new(TypedExpr::new(
                    IrExprKind::Var {
                        name: "data".to_string(),
                        access: VarAccess::Read,
                        ref_kind: VarRefKind::Value,
                    },
                    IrType::Bytes,
                )),
                from_ty: IrType::Bytes,
                to_ty: IrType::Ref(Box::new(IrType::Bytes)),
                kind: IrInteropCoercionKind::Builtin {
                    policy: incan_core::interop::CoercionPolicy::Borrow,
                    rust_target: "&[u8]".to_string(),
                },
            },
            IrType::Ref(Box::new(IrType::Bytes)),
        );

        let emitted = emitter
            .emit_expr(&expr)
            .map_err(|err| format!("expected successful expression emission, got {err:?}"))?;
        let rendered = emitted.to_string();
        assert!(
            rendered.starts_with("&"),
            "expected borrowed bytes slice, got `{rendered}`"
        );
        assert!(
            rendered.contains("data"),
            "expected borrowed bytes coercion to preserve the source expression, got `{rendered}`"
        );
        Ok(())
    }

    #[test]
    fn interop_borrowed_vec_bytes_coercion_materializes_owned_bytes_before_borrow() -> Result<(), String> {
        let registry = FunctionRegistry::new();
        let emitter = IrEmitter::new(&registry);
        let expr = TypedExpr::new(
            IrExprKind::InteropCoerce {
                expr: Box::new(TypedExpr::new(IrExprKind::Bytes(b"abc".to_vec()), IrType::StaticBytes)),
                from_ty: IrType::StaticBytes,
                to_ty: IrType::Ref(Box::new(IrType::Struct("Vec<u8>".to_string()))),
                kind: IrInteropCoercionKind::Builtin {
                    policy: incan_core::interop::CoercionPolicy::Borrow,
                    rust_target: "&[u8]".to_string(),
                },
            },
            IrType::Ref(Box::new(IrType::Struct("Vec<u8>".to_string()))),
        );

        let emitted = emitter
            .emit_expr(&expr)
            .map_err(|err| format!("expected successful expression emission, got {err:?}"))?;
        let rendered = emitted.to_string();
        assert!(
            rendered.contains(". to_vec ()"),
            "expected borrowed Vec<u8> interop coercion to materialize owned bytes, got `{rendered}`"
        );
        assert!(
            rendered.starts_with("&"),
            "expected borrowed Vec<u8> interop coercion to emit a borrow, got `{rendered}`"
        );
        Ok(())
    }

    #[test]
    fn non_string_method_call_join_stays_regular_method_call() -> Result<(), String> {
        let registry = FunctionRegistry::new();
        let emitter = IrEmitter::new(&registry);
        let expr = TypedExpr::new(
            IrExprKind::MethodCall {
                receiver: Box::new(TypedExpr::new(
                    IrExprKind::Var {
                        name: "dataset".to_string(),
                        access: VarAccess::Read,
                        ref_kind: VarRefKind::Value,
                    },
                    IrType::Struct("Dataset".to_string()),
                )),
                method: "join".to_string(),
                dispatch: None,
                type_args: Vec::new(),
                args: vec![
                    IrCallArg {
                        name: None,
                        kind: IrCallArgKind::Positional,
                        expr: TypedExpr::new(
                            IrExprKind::Var {
                                name: "other".to_string(),
                                access: VarAccess::Read,
                                ref_kind: VarRefKind::Value,
                            },
                            IrType::Struct("Dataset".to_string()),
                        ),
                    },
                    IrCallArg {
                        name: None,
                        kind: IrCallArgKind::Positional,
                        expr: TypedExpr::new(IrExprKind::Bool(true), IrType::Bool),
                    },
                ],
                callable_signature: Some(FunctionSignature {
                    params: vec![FunctionParam {
                        name: "k".to_string(),
                        ty: IrType::Ref(Box::new(IrType::Generic("Q".to_string()))),
                        mutability: Mutability::Immutable,
                        is_self: false,
                        kind: crate::frontend::ast::ParamKind::Normal,
                        default: None,
                    }],
                    return_type: IrType::Option(Box::new(IrType::Ref(Box::new(IrType::Int)))),
                }),
                arg_policy: MethodCallArgPolicy::Default,
            },
            IrType::Struct("Dataset".to_string()),
        );

        let emitted = emitter
            .emit_expr(&expr)
            .map_err(|err| format!("expected successful expression emission, got {err:?}"))?;
        let rendered = emitted.to_string();
        assert!(
            rendered.contains("dataset . join"),
            "expected regular method-call emission, got `{rendered}`"
        );
        assert!(
            !rendered.contains("str_join"),
            "plain MethodCall must not be reclassified as string join, got `{rendered}`"
        );
        Ok(())
    }

    #[test]
    fn dict_get_with_borrowed_key_does_not_double_borrow() -> Result<(), String> {
        let registry = FunctionRegistry::new();
        let emitter = IrEmitter::new(&registry);
        let expr = TypedExpr::new(
            IrExprKind::KnownMethodCall {
                receiver: Box::new(TypedExpr::new(
                    IrExprKind::Var {
                        name: "counts".to_string(),
                        access: VarAccess::Read,
                        ref_kind: VarRefKind::Value,
                    },
                    IrType::Dict(Box::new(IrType::String), Box::new(IrType::Int)),
                )),
                kind: MethodKind::Collection(CollectionMethodKind::Get),
                args: vec![IrCallArg {
                    name: None,
                    kind: IrCallArgKind::Positional,
                    expr: TypedExpr::new(
                        IrExprKind::Var {
                            name: "word".to_string(),
                            access: VarAccess::Read,
                            ref_kind: VarRefKind::Value,
                        },
                        IrType::StrRef,
                    ),
                }],
            },
            IrType::Option(Box::new(IrType::Int)),
        );

        let emitted = emitter
            .emit_expr(&expr)
            .map_err(|err| format!("expected successful expression emission, got {err:?}"))?;
        let rendered = emitted.to_string();
        assert!(
            rendered.contains("counts . get (word)"),
            "expected borrowed dict key to stay singly borrowed, got `{rendered}`"
        );
        assert!(
            !rendered.contains("counts . get (& word)"),
            "dict get must not double-borrow borrowed keys, got `{rendered}`"
        );
        Ok(())
    }

    #[test]
    fn regular_method_call_policy_preserves_lookup_arg_shape() -> Result<(), String> {
        let registry = FunctionRegistry::new();
        let emitter = IrEmitter::new(&registry);
        let expr = TypedExpr::new(
            IrExprKind::MethodCall {
                receiver: Box::new(TypedExpr::new(
                    IrExprKind::Var {
                        name: "counts".to_string(),
                        access: VarAccess::Read,
                        ref_kind: VarRefKind::Value,
                    },
                    IrType::Struct("std::collections::HashMap".to_string()),
                )),
                method: "get".to_string(),
                dispatch: None,
                type_args: Vec::new(),
                args: vec![IrCallArg {
                    name: None,
                    kind: IrCallArgKind::Positional,
                    expr: TypedExpr::new(IrExprKind::String("the".to_string()), IrType::String),
                }],
                callable_signature: None,
                arg_policy: MethodCallArgPolicy::PreserveShape,
            },
            IrType::Option(Box::new(IrType::Int)),
        );

        let emitted = emitter
            .emit_expr(&expr)
            .map_err(|err| format!("expected successful expression emission, got {err:?}"))?;
        let rendered = emitted.to_string();
        assert!(
            rendered.contains("counts . get (\"the\")"),
            "expected preserved method-call lookup shape, got `{rendered}`"
        );
        assert!(
            !rendered.contains(". into ()"),
            "preserved method-call lookup shape must not apply external string coercion, got `{rendered}`"
        );
        Ok(())
    }

    #[test]
    fn regular_hash_map_get_preserves_borrowed_probe_shape() -> Result<(), String> {
        let registry = FunctionRegistry::new();
        let emitter = IrEmitter::new(&registry);
        let expr = TypedExpr::new(
            IrExprKind::MethodCall {
                receiver: Box::new(TypedExpr::new(
                    IrExprKind::Var {
                        name: "counts".to_string(),
                        access: VarAccess::Read,
                        ref_kind: VarRefKind::Value,
                    },
                    IrType::NamedGeneric("HashMap".to_string(), vec![IrType::String, IrType::Int]),
                )),
                method: "get".to_string(),
                dispatch: None,
                type_args: Vec::new(),
                args: vec![IrCallArg {
                    name: None,
                    kind: IrCallArgKind::Positional,
                    expr: TypedExpr::new(
                        IrExprKind::Var {
                            name: "word".to_string(),
                            access: VarAccess::Read,
                            ref_kind: VarRefKind::Value,
                        },
                        IrType::StrRef,
                    ),
                }],
                callable_signature: None,
                arg_policy: MethodCallArgPolicy::Default,
            },
            IrType::Option(Box::new(IrType::Int)),
        );

        let emitted = emitter
            .emit_expr(&expr)
            .map_err(|err| format!("expected successful expression emission, got {err:?}"))?;
        let rendered = emitted.to_string();
        assert!(
            rendered.contains("counts . get (word)"),
            "expected HashMap::get to keep borrowed probe shape, got `{rendered}`"
        );
        assert!(
            !rendered.contains("counts . get (& word)"),
            "HashMap::get must not double-borrow borrowed probes, got `{rendered}`"
        );
        Ok(())
    }

    #[test]
    fn known_dict_get_with_string_literal_uses_str_lookup_shape() -> Result<(), String> {
        let registry = FunctionRegistry::new();
        let emitter = IrEmitter::new(&registry);
        let expr = TypedExpr::new(
            IrExprKind::KnownMethodCall {
                receiver: Box::new(TypedExpr::new(
                    IrExprKind::Var {
                        name: "counts".to_string(),
                        access: VarAccess::Read,
                        ref_kind: VarRefKind::Value,
                    },
                    IrType::Dict(Box::new(IrType::String), Box::new(IrType::Int)),
                )),
                kind: MethodKind::Collection(CollectionMethodKind::Get),
                args: vec![IrCallArg {
                    name: None,
                    kind: IrCallArgKind::Positional,
                    expr: TypedExpr::new(IrExprKind::String("the".to_string()), IrType::String),
                }],
            },
            IrType::Option(Box::new(IrType::Int)),
        );

        let emitted = emitter
            .emit_expr(&expr)
            .map_err(|err| format!("expected successful expression emission, got {err:?}"))?;
        let rendered = emitted.to_string();
        assert!(
            rendered.contains("counts . get (< _ as AsRef < str >> :: as_ref (& \"the\"))"),
            "expected string-key dict lookup to normalize via fully-qualified `AsRef<str>`, got `{rendered}`"
        );
        Ok(())
    }

    #[test]
    fn dict_index_with_string_literal_uses_str_lookup_shape() -> Result<(), String> {
        let registry = FunctionRegistry::new();
        let emitter = IrEmitter::new(&registry);
        let expr = TypedExpr::new(
            IrExprKind::Index {
                object: Box::new(TypedExpr::new(
                    IrExprKind::Var {
                        name: "counts".to_string(),
                        access: VarAccess::Read,
                        ref_kind: VarRefKind::Value,
                    },
                    IrType::Dict(Box::new(IrType::String), Box::new(IrType::Int)),
                )),
                index: Box::new(TypedExpr::new(IrExprKind::String("the".to_string()), IrType::String)),
            },
            IrType::Int,
        );

        let emitted = emitter
            .emit_expr(&expr)
            .map_err(|err| format!("expected successful expression emission, got {err:?}"))?;
        let rendered = emitted.to_string();
        assert!(
            rendered.contains(
                "incan_stdlib :: collections :: dict_get (& counts , < _ as AsRef < str >> :: as_ref (& \"the\"))"
            ),
            "expected dict index to normalize string probes via fully-qualified `AsRef<str>`, got `{rendered}`"
        );
        Ok(())
    }

    #[test]
    fn known_list_methods_emit_checked_runtime_helpers() -> Result<(), String> {
        let registry = FunctionRegistry::new();
        let emitter = IrEmitter::new(&registry);

        let receiver = || {
            Box::new(TypedExpr::new(
                IrExprKind::Var {
                    name: "items".to_string(),
                    access: VarAccess::Read,
                    ref_kind: VarRefKind::Value,
                },
                IrType::List(Box::new(IrType::Int)),
            ))
        };

        let render = |expr: TypedExpr| -> Result<String, String> {
            emitter
                .emit_expr(&expr)
                .map(|tokens| tokens.to_string())
                .map_err(|err| format!("expected successful expression emission, got {err:?}"))
        };

        let index_rendered = render(TypedExpr::new(
            IrExprKind::KnownMethodCall {
                receiver: receiver(),
                kind: MethodKind::Collection(CollectionMethodKind::Index),
                args: vec![IrCallArg {
                    name: None,
                    kind: IrCallArgKind::Positional,
                    expr: TypedExpr::new(IrExprKind::Int(9), IrType::Int),
                }],
            },
            IrType::Int,
        ))?;
        assert!(
            index_rendered.contains("incan_stdlib :: collections :: list_index (& items , & 9)"),
            "expected list.index to route through checked runtime helper, got `{index_rendered}`"
        );

        let count_rendered = render(TypedExpr::new(
            IrExprKind::KnownMethodCall {
                receiver: receiver(),
                kind: MethodKind::Collection(CollectionMethodKind::Count),
                args: vec![IrCallArg {
                    name: None,
                    kind: IrCallArgKind::Positional,
                    expr: TypedExpr::new(IrExprKind::Int(9), IrType::Int),
                }],
            },
            IrType::Int,
        ))?;
        assert!(
            count_rendered.contains("incan_stdlib :: collections :: list_count (& items , & 9)"),
            "expected list.count to route through checked runtime helper, got `{count_rendered}`"
        );

        let remove_rendered = render(TypedExpr::new(
            IrExprKind::KnownMethodCall {
                receiver: receiver(),
                kind: MethodKind::Collection(CollectionMethodKind::Remove),
                args: vec![IrCallArg {
                    name: None,
                    kind: IrCallArgKind::Positional,
                    expr: TypedExpr::new(IrExprKind::Int(9), IrType::Int),
                }],
            },
            IrType::Unit,
        ))?;
        assert!(
            remove_rendered.contains("incan_stdlib :: collections :: list_remove"),
            "expected list.remove to route through checked runtime helper, got `{remove_rendered}`"
        );

        let swap_rendered = render(TypedExpr::new(
            IrExprKind::KnownMethodCall {
                receiver: receiver(),
                kind: MethodKind::Collection(CollectionMethodKind::Swap),
                args: vec![
                    IrCallArg {
                        name: None,
                        kind: IrCallArgKind::Positional,
                        expr: TypedExpr::new(IrExprKind::Int(0), IrType::Int),
                    },
                    IrCallArg {
                        name: None,
                        kind: IrCallArgKind::Positional,
                        expr: TypedExpr::new(IrExprKind::Int(9), IrType::Int),
                    },
                ],
            },
            IrType::Unit,
        ))?;
        assert!(
            swap_rendered.contains("incan_stdlib :: collections :: list_swap"),
            "expected list.swap to route through checked runtime helper, got `{swap_rendered}`"
        );

        Ok(())
    }

    #[test]
    fn external_nominal_method_call_keeps_external_string_conversion() -> Result<(), String> {
        let registry = FunctionRegistry::new();
        let emitter = IrEmitter::new(&registry);
        let expr = TypedExpr::new(
            IrExprKind::MethodCall {
                receiver: Box::new(TypedExpr::new(
                    IrExprKind::Var {
                        name: "builder".to_string(),
                        access: VarAccess::Read,
                        ref_kind: VarRefKind::Value,
                    },
                    IrType::Struct("ExternalBuilder".to_string()),
                )),
                method: "rename".to_string(),
                dispatch: None,
                type_args: Vec::new(),
                args: vec![IrCallArg {
                    name: None,
                    kind: IrCallArgKind::Positional,
                    expr: TypedExpr::new(IrExprKind::String("logs".to_string()), IrType::String),
                }],
                callable_signature: None,
                arg_policy: MethodCallArgPolicy::Default,
            },
            IrType::Unknown,
        );

        let emitted = emitter
            .emit_expr(&expr)
            .map_err(|err| format!("expected successful expression emission, got {err:?}"))?;
        let rendered = emitted.to_string();
        assert!(
            rendered.contains("builder . rename (\"logs\" . into ())"),
            "expected external nominal method call to preserve `.into()` coercion, got `{rendered}`"
        );
        assert!(
            !rendered.contains("\"logs\" . to_string ()"),
            "external nominal method call must not use Incan-owned string coercion, got `{rendered}`"
        );
        Ok(())
    }

    #[test]
    fn internal_dependency_nominal_method_call_does_not_borrow_string_arguments() -> Result<(), String> {
        let registry = FunctionRegistry::new();
        let mut emitter = IrEmitter::new(&registry);
        emitter.set_type_module_paths(
            std::collections::HashMap::from([("Session".to_string(), vec!["session".to_string()])]),
            std::collections::HashSet::new(),
        );
        let expr = TypedExpr::new(
            IrExprKind::MethodCall {
                receiver: Box::new(TypedExpr::new(
                    IrExprKind::Var {
                        name: "session".to_string(),
                        access: VarAccess::Read,
                        ref_kind: VarRefKind::Value,
                    },
                    IrType::Struct("Session".to_string()),
                )),
                method: "read_csv".to_string(),
                dispatch: None,
                type_args: vec![IrType::Struct("OrderLine".to_string())],
                args: vec![
                    IrCallArg {
                        name: None,
                        kind: IrCallArgKind::Positional,
                        expr: TypedExpr::new(IrExprKind::String("order_lines".to_string()), IrType::String),
                    },
                    IrCallArg {
                        name: None,
                        kind: IrCallArgKind::Positional,
                        expr: TypedExpr::new(
                            IrExprKind::Var {
                                name: "input_uri".to_string(),
                                access: VarAccess::Read,
                                ref_kind: VarRefKind::Value,
                            },
                            IrType::String,
                        ),
                    },
                ],
                callable_signature: None,
                arg_policy: MethodCallArgPolicy::Default,
            },
            IrType::Unknown,
        );

        let emitted = emitter
            .emit_expr(&expr)
            .map_err(|err| format!("expected successful expression emission, got {err:?}"))?;
        let rendered = emitted.to_string();
        assert!(
            rendered.contains("session . read_csv :: < OrderLine >"),
            "expected regular method-call emission on internal dependency type, got `{rendered}`"
        );
        assert!(
            !rendered.contains("& input_uri") && !rendered.contains("&input_uri"),
            "internal dependency method call must not borrow owned string args like an external Rust receiver, got `{rendered}`"
        );
        Ok(())
    }

    #[test]
    fn external_name_namespace_call_uses_incan_function_arg_conversion() -> Result<(), String> {
        let registry = FunctionRegistry::new();
        let emitter = IrEmitter::new(&registry);
        let expr = TypedExpr::new(
            IrExprKind::MethodCall {
                receiver: Box::new(TypedExpr::new(
                    IrExprKind::Var {
                        name: "widgets".to_string(),
                        access: VarAccess::Read,
                        ref_kind: VarRefKind::ExternalName,
                    },
                    IrType::Struct("widgets".to_string()),
                )),
                method: "make_widget".to_string(),
                dispatch: None,
                type_args: Vec::new(),
                args: vec![IrCallArg {
                    name: None,
                    kind: IrCallArgKind::Positional,
                    expr: TypedExpr::new(
                        IrExprKind::Var {
                            name: "DEFAULT_NAME".to_string(),
                            access: VarAccess::Read,
                            ref_kind: VarRefKind::Value,
                        },
                        IrType::String,
                    ),
                }],
                callable_signature: None,
                arg_policy: MethodCallArgPolicy::Default,
            },
            IrType::Unknown,
        );

        let emitted = emitter
            .emit_expr(&expr)
            .map_err(|err| format!("expected successful expression emission, got {err:?}"))?;
        let rendered = emitted.to_string();
        assert!(
            rendered.contains("widgets :: make_widget (DEFAULT_NAME"),
            "expected namespace call to stay on the ordinary function-conversion path, got `{rendered}`"
        );
        assert!(
            !rendered.contains("& DEFAULT_NAME"),
            "namespace call must not borrow owned string args like an external Rust receiver, got `{rendered}`"
        );
        assert!(
            !rendered.contains(". into ()"),
            "namespace call must not apply external-Rust `.into()` coercions, got `{rendered}`"
        );
        Ok(())
    }

    #[test]
    fn external_rust_call_coerces_list_elements_to_target_vec_element() -> Result<(), String> {
        let registry = FunctionRegistry::new();
        let emitter = IrEmitter::new(&registry);
        let expr = TypedExpr::new(
            IrExprKind::Call {
                func: Box::new(TypedExpr::new(
                    IrExprKind::Var {
                        name: "build_frame".to_string(),
                        access: VarAccess::Copy,
                        ref_kind: VarRefKind::ExternalRustName,
                    },
                    IrType::Function {
                        params: vec![IrType::List(Box::new(IrType::Struct(
                            "polars::prelude::Column".to_string(),
                        )))],
                        ret: Box::new(IrType::Unit),
                    },
                )),
                type_args: Vec::new(),
                args: vec![IrCallArg {
                    name: None,
                    kind: IrCallArgKind::Positional,
                    expr: TypedExpr::new(
                        IrExprKind::Var {
                            name: "columns".to_string(),
                            access: VarAccess::Move,
                            ref_kind: VarRefKind::Value,
                        },
                        IrType::List(Box::new(IrType::Struct("polars::series::Series".to_string()))),
                    ),
                }],
                callable_signature: None,
                canonical_path: None,
            },
            IrType::Unit,
        );

        let emitted = emitter
            .emit_expr(&expr)
            .map_err(|err| format!("expected successful expression emission, got {err:?}"))?;
        let rendered = emitted.to_string();
        assert!(
            rendered.contains("(columns) . into_iter () . map"),
            "expected external Rust list arg to map elements through Into, got `{rendered}`"
        );
        assert!(
            rendered.contains(":: std :: convert :: Into :: into"),
            "expected external Rust list arg to use fully qualified Into::into, got `{rendered}`"
        );
        assert!(
            rendered.contains("collect :: < Vec < _ >> ()"),
            "expected external Rust list arg to collect into Vec<_>, got `{rendered}`"
        );

        let literal_expr = TypedExpr::new(
            IrExprKind::Call {
                func: Box::new(TypedExpr::new(
                    IrExprKind::Var {
                        name: "build_frame".to_string(),
                        access: VarAccess::Copy,
                        ref_kind: VarRefKind::ExternalRustName,
                    },
                    IrType::Function {
                        params: vec![IrType::List(Box::new(IrType::Struct(
                            "polars::prelude::Column".to_string(),
                        )))],
                        ret: Box::new(IrType::Unit),
                    },
                )),
                type_args: Vec::new(),
                args: vec![IrCallArg {
                    name: None,
                    kind: IrCallArgKind::Positional,
                    expr: TypedExpr::new(
                        IrExprKind::List(vec![
                            IrListEntry::Element(TypedExpr::new(
                                IrExprKind::Var {
                                    name: "id_series".to_string(),
                                    access: VarAccess::Move,
                                    ref_kind: VarRefKind::Value,
                                },
                                IrType::Struct("polars::series::Series".to_string()),
                            )),
                            IrListEntry::Element(TypedExpr::new(
                                IrExprKind::Var {
                                    name: "value_series".to_string(),
                                    access: VarAccess::Move,
                                    ref_kind: VarRefKind::Value,
                                },
                                IrType::Struct("polars::series::Series".to_string()),
                            )),
                        ]),
                        IrType::List(Box::new(IrType::Struct("polars::series::Series".to_string()))),
                    ),
                }],
                callable_signature: None,
                canonical_path: None,
            },
            IrType::Unit,
        );
        let literal_rendered = emitter
            .emit_expr(&literal_expr)
            .map_err(|err| format!("expected successful expression emission, got {err:?}"))?
            .to_string();
        assert!(
            literal_rendered.contains("vec ! [id_series , value_series]") && literal_rendered.contains("into_iter"),
            "expected list literal external arg to get element coercion wrapper, got `{literal_rendered}`"
        );
        Ok(())
    }

    #[test]
    fn external_rust_call_leaves_matching_list_elements_unmapped() -> Result<(), String> {
        let registry = FunctionRegistry::new();
        let emitter = IrEmitter::new(&registry);
        let series_ty = IrType::Struct("polars::series::Series".to_string());
        let expr = TypedExpr::new(
            IrExprKind::Call {
                func: Box::new(TypedExpr::new(
                    IrExprKind::Var {
                        name: "use_series".to_string(),
                        access: VarAccess::Copy,
                        ref_kind: VarRefKind::ExternalRustName,
                    },
                    IrType::Function {
                        params: vec![IrType::List(Box::new(series_ty.clone()))],
                        ret: Box::new(IrType::Unit),
                    },
                )),
                type_args: Vec::new(),
                args: vec![IrCallArg {
                    name: None,
                    kind: IrCallArgKind::Positional,
                    expr: TypedExpr::new(
                        IrExprKind::Var {
                            name: "series".to_string(),
                            access: VarAccess::Move,
                            ref_kind: VarRefKind::Value,
                        },
                        IrType::List(Box::new(series_ty)),
                    ),
                }],
                callable_signature: None,
                canonical_path: None,
            },
            IrType::Unit,
        );

        let emitted = emitter
            .emit_expr(&expr)
            .map_err(|err| format!("expected successful expression emission, got {err:?}"))?;
        let rendered = emitted.to_string();
        assert!(
            rendered.contains("use_series (series)"),
            "expected matching Vec element types to pass through directly, got `{rendered}`"
        );
        assert!(
            !rendered.contains("into_iter"),
            "matching Vec element types must not add element coercion, got `{rendered}`"
        );
        Ok(())
    }

    #[test]
    fn external_rust_associated_method_coerces_list_elements_to_target_vec_element() -> Result<(), String> {
        let registry = FunctionRegistry::new();
        let emitter = IrEmitter::new(&registry);
        let expr = TypedExpr::new(
            IrExprKind::MethodCall {
                receiver: Box::new(TypedExpr::new(
                    IrExprKind::Var {
                        name: "DataFrame".to_string(),
                        access: VarAccess::Copy,
                        ref_kind: VarRefKind::ExternalRustName,
                    },
                    IrType::Struct("polars::prelude::DataFrame".to_string()),
                )),
                method: "new".to_string(),
                dispatch: None,
                type_args: Vec::new(),
                args: vec![IrCallArg {
                    name: None,
                    kind: IrCallArgKind::Positional,
                    expr: TypedExpr::new(
                        IrExprKind::Var {
                            name: "columns".to_string(),
                            access: VarAccess::Move,
                            ref_kind: VarRefKind::Value,
                        },
                        IrType::List(Box::new(IrType::Struct("polars::series::Series".to_string()))),
                    ),
                }],
                callable_signature: Some(FunctionSignature {
                    params: vec![FunctionParam {
                        name: "columns".to_string(),
                        ty: IrType::List(Box::new(IrType::Struct("polars::prelude::Column".to_string()))),
                        mutability: Mutability::Immutable,
                        is_self: false,
                        kind: crate::frontend::ast::ParamKind::Normal,
                        default: None,
                    }],
                    return_type: IrType::Struct("polars::prelude::DataFrame".to_string()),
                }),
                arg_policy: MethodCallArgPolicy::Default,
            },
            IrType::Struct("polars::prelude::DataFrame".to_string()),
        );

        let emitted = emitter
            .emit_expr(&expr)
            .map_err(|err| format!("expected successful expression emission, got {err:?}"))?;
        let rendered = emitted.to_string();
        assert!(
            rendered.contains("DataFrame :: new ((columns) . into_iter () . map"),
            "expected external Rust associated method list arg to map elements through Into, got `{rendered}`"
        );
        assert!(
            rendered.contains(":: std :: convert :: Into :: into"),
            "expected external Rust associated method list arg to use fully qualified Into::into, got `{rendered}`"
        );
        assert!(
            rendered.contains("collect :: < Vec < _ >> ()"),
            "expected external Rust associated method list arg to collect into Vec<_>, got `{rendered}`"
        );
        Ok(())
    }

    #[test]
    fn rusttype_surface_associated_function_uses_incan_string_conversion() -> Result<(), String> {
        let registry = FunctionRegistry::new();
        let mut emitter = IrEmitter::new(&registry);
        emitter.rusttype_alias_names.insert("Name".to_string());
        let expr = TypedExpr::new(
            IrExprKind::MethodCall {
                receiver: Box::new(TypedExpr::new(
                    IrExprKind::Var {
                        name: "Name".to_string(),
                        access: VarAccess::Read,
                        ref_kind: VarRefKind::TypeName,
                    },
                    IrType::Struct("Name".to_string()),
                )),
                method: "parse".to_string(),
                dispatch: None,
                type_args: Vec::new(),
                args: vec![IrCallArg {
                    name: None,
                    kind: IrCallArgKind::Positional,
                    expr: TypedExpr::new(IrExprKind::String("alice@example.com".to_string()), IrType::String),
                }],
                callable_signature: None,
                arg_policy: MethodCallArgPolicy::Default,
            },
            IrType::Struct("Name".to_string()),
        );

        let emitted = emitter
            .emit_expr(&expr)
            .map_err(|err| format!("expected successful expression emission, got {err:?}"))?;
        let rendered = emitted.to_string();
        assert!(
            rendered.contains("Name :: parse (\"alice@example.com\" . to_string ())"),
            "expected rusttype surface associated function to use Incan string conversion, got `{rendered}`"
        );
        assert!(
            !rendered.contains(". into ()"),
            "rusttype surface associated function must not use external-Rust `.into()` conversion, got `{rendered}`"
        );
        Ok(())
    }

    #[test]
    fn qualified_rusttype_receiver_method_uses_rust_signature_borrowing() -> Result<(), String> {
        let registry = FunctionRegistry::new();
        let mut emitter = IrEmitter::new(&registry);
        emitter.rusttype_alias_names.insert("_RawRegex".to_string());
        emitter.struct_field_types.insert(
            ("Regex".to_string(), "raw".to_string()),
            IrType::Struct("crate::__incan_std::regex::_RawRegex".to_string()),
        );
        let expr = TypedExpr::new(
            IrExprKind::MethodCall {
                receiver: Box::new(TypedExpr::new(
                    IrExprKind::Field {
                        object: Box::new(TypedExpr::new(
                            IrExprKind::Var {
                                name: "self".to_string(),
                                access: VarAccess::Read,
                                ref_kind: VarRefKind::Value,
                            },
                            IrType::Struct("Regex".to_string()),
                        )),
                        field: "raw".to_string(),
                    },
                    IrType::Unknown,
                )),
                method: "find_iter".to_string(),
                dispatch: None,
                type_args: Vec::new(),
                args: vec![IrCallArg {
                    name: None,
                    kind: IrCallArgKind::Positional,
                    expr: TypedExpr::new(
                        IrExprKind::Var {
                            name: "text".to_string(),
                            access: VarAccess::Read,
                            ref_kind: VarRefKind::Value,
                        },
                        IrType::String,
                    ),
                }],
                callable_signature: Some(FunctionSignature {
                    params: vec![FunctionParam {
                        name: "text".to_string(),
                        ty: IrType::Ref(Box::new(IrType::String)),
                        mutability: Mutability::Immutable,
                        is_self: false,
                        kind: crate::frontend::ast::ParamKind::Normal,
                        default: None,
                    }],
                    return_type: IrType::Struct("_RawMatchIterator".to_string()),
                }),
                arg_policy: MethodCallArgPolicy::Default,
            },
            IrType::Struct("_RawMatchIterator".to_string()),
        );

        let emitted = emitter
            .emit_expr(&expr)
            .map_err(|err| format!("expected successful expression emission, got {err:?}"))?;
        let rendered = emitted.to_string();
        assert!(
            rendered.contains("self . raw . find_iter"),
            "expected regular method-call emission on qualified rusttype receiver, got `{rendered}`"
        );
        assert!(
            rendered.contains("find_iter (& text)") || rendered.contains("find_iter (&text)"),
            "metadata-resolved rusttype receiver methods should borrow owned strings for Rust &str params, got `{rendered}`"
        );
        assert!(
            !rendered.contains("to_string"),
            "metadata-resolved rusttype receiver methods should not clone strings before borrowing, got `{rendered}`"
        );
        Ok(())
    }

    #[test]
    fn known_iterator_adapter_methods_emit_incan_stdlib_models() -> Result<(), String> {
        let registry = FunctionRegistry::new();
        let emitter = IrEmitter::new(&registry);

        let render = |kind: IteratorMethodKind, args: Vec<IrCallArg>| -> Result<String, String> {
            let expr = TypedExpr::new(
                IrExprKind::KnownMethodCall {
                    receiver: Box::new(iterator_receiver()),
                    kind: MethodKind::Iterator(kind),
                    args,
                },
                IrType::Unknown,
            );
            emitter
                .emit_expr(&expr)
                .map(|tokens| tokens.to_string())
                .map_err(|err| format!("expected successful expression emission, got {err:?}"))
        };

        let callback = || {
            vec![IrCallArg {
                name: None,
                kind: IrCallArgKind::Positional,
                expr: function_var("transform"),
            }]
        };
        let count = |value| {
            vec![IrCallArg {
                name: None,
                kind: IrCallArgKind::Positional,
                expr: TypedExpr::new(IrExprKind::Int(value), IrType::Int),
            }]
        };
        let other = || {
            vec![IrCallArg {
                name: None,
                kind: IrCallArgKind::Positional,
                expr: TypedExpr::new(
                    IrExprKind::Var {
                        name: "others".to_string(),
                        access: VarAccess::Read,
                        ref_kind: VarRefKind::Value,
                    },
                    IrType::NamedGeneric(core_traits::as_str(TraitId::Iterator).to_string(), vec![IrType::Int]),
                ),
            }]
        };

        let map_rendered = render(IteratorMethodKind::Map, callback())?;
        assert!(
            map_rendered.contains("collection :: MapIterator") && map_rendered.contains("f : transform"),
            "unexpected map emission: {map_rendered}"
        );

        let filter_rendered = render(IteratorMethodKind::Filter, callback())?;
        assert!(
            filter_rendered.contains("collection :: FilterIterator") && filter_rendered.contains("f : transform"),
            "unexpected filter emission: {filter_rendered}"
        );

        let enumerate_rendered = render(IteratorMethodKind::Enumerate, Vec::new())?;
        assert!(
            enumerate_rendered.contains("collection :: EnumerateIterator")
                && enumerate_rendered.contains("index : 0i64"),
            "unexpected enumerate emission: {enumerate_rendered}"
        );

        let zip_rendered = render(IteratorMethodKind::Zip, other())?;
        assert!(
            zip_rendered.contains("collection :: ZipIterator") && zip_rendered.contains("right : (others)"),
            "unexpected zip emission: {zip_rendered}"
        );

        let take_rendered = render(IteratorMethodKind::Take, count(3))?;
        assert!(
            take_rendered.contains("collection :: TakeIterator") && take_rendered.contains("remaining : 3"),
            "unexpected take emission: {take_rendered}"
        );

        let skip_rendered = render(IteratorMethodKind::Skip, count(-2))?;
        assert!(
            skip_rendered.contains("collection :: SkipIterator") && skip_rendered.contains("remaining : - 2"),
            "unexpected skip emission: {skip_rendered}"
        );

        let take_while_rendered = render(IteratorMethodKind::TakeWhile, callback())?;
        assert!(
            take_while_rendered.contains("collection :: TakeWhileIterator")
                && take_while_rendered.contains("f : transform"),
            "unexpected take_while emission: {take_while_rendered}"
        );

        let skip_while_rendered = render(IteratorMethodKind::SkipWhile, callback())?;
        assert!(
            skip_while_rendered.contains("collection :: SkipWhileIterator")
                && skip_while_rendered.contains("f : transform"),
            "unexpected skip_while emission: {skip_while_rendered}"
        );

        let chain_rendered = render(IteratorMethodKind::Chain, other())?;
        assert!(
            chain_rendered.contains("collection :: ChainIterator") && chain_rendered.contains("second : (others)"),
            "unexpected chain emission: {chain_rendered}"
        );

        let flat_map_rendered = render(IteratorMethodKind::FlatMap, callback())?;
        assert!(
            flat_map_rendered.contains("collection :: FlatMapIterator")
                && flat_map_rendered.contains("current : Vec :: new ()"),
            "unexpected flat_map emission: {flat_map_rendered}"
        );

        let batch_rendered = render(IteratorMethodKind::Batch, count(2))?;
        assert!(
            batch_rendered.contains("collection :: BatchIterator") && batch_rendered.contains("size : 2"),
            "unexpected batch emission: {batch_rendered}"
        );

        Ok(())
    }

    #[test]
    fn known_iterator_terminal_methods_emit_incan_next_loops() -> Result<(), String> {
        let registry = FunctionRegistry::new();
        let emitter = IrEmitter::new(&registry);

        let render = |kind: IteratorMethodKind, args: Vec<IrCallArg>| -> Result<String, String> {
            let expr = TypedExpr::new(
                IrExprKind::KnownMethodCall {
                    receiver: Box::new(iterator_receiver()),
                    kind: MethodKind::Iterator(kind),
                    args,
                },
                IrType::Unknown,
            );
            emitter
                .emit_expr(&expr)
                .map(|tokens| tokens.to_string())
                .map_err(|err| format!("expected successful expression emission, got {err:?}"))
        };

        let callback = || {
            vec![IrCallArg {
                name: None,
                kind: IrCallArgKind::Positional,
                expr: function_var("predicate"),
            }]
        };

        let collect_rendered = render(IteratorMethodKind::Collect, Vec::new())?;
        assert!(
            collect_rendered.contains("collection :: Iterator :: __next__")
                && collect_rendered.contains("__incan_items . push"),
            "unexpected collect emission: {collect_rendered}"
        );

        let count_rendered = render(IteratorMethodKind::Count, Vec::new())?;
        assert!(
            count_rendered.contains("collection :: Iterator :: __next__")
                && count_rendered.contains("__incan_total += 1"),
            "unexpected count emission: {count_rendered}"
        );

        let reduce_rendered = render(
            IteratorMethodKind::Reduce,
            vec![
                IrCallArg {
                    name: None,
                    kind: IrCallArgKind::Positional,
                    expr: TypedExpr::new(IrExprKind::Int(0), IrType::Int),
                },
                IrCallArg {
                    name: None,
                    kind: IrCallArgKind::Positional,
                    expr: function_var("predicate"),
                },
            ],
        )?;
        assert!(
            reduce_rendered.contains("collection :: Iterator :: __next__")
                && reduce_rendered.contains("__incan_acc = (predicate) (__incan_acc , __incan_item)"),
            "unexpected reduce emission: {reduce_rendered}"
        );

        let fold_rendered = render(
            IteratorMethodKind::Fold,
            vec![
                IrCallArg {
                    name: None,
                    kind: IrCallArgKind::Positional,
                    expr: TypedExpr::new(IrExprKind::Int(0), IrType::Int),
                },
                IrCallArg {
                    name: None,
                    kind: IrCallArgKind::Positional,
                    expr: function_var("predicate"),
                },
            ],
        )?;
        assert!(
            fold_rendered.contains("collection :: Iterator :: __next__")
                && fold_rendered.contains("__incan_acc = (predicate) (__incan_acc , __incan_item)"),
            "unexpected fold emission: {fold_rendered}"
        );

        let any_rendered = render(IteratorMethodKind::Any, callback())?;
        assert!(
            any_rendered.contains("collection :: Iterator :: __next__") && any_rendered.contains("(predicate)"),
            "unexpected any emission: {any_rendered}"
        );

        let all_rendered = render(IteratorMethodKind::All, callback())?;
        assert!(
            all_rendered.contains("collection :: Iterator :: __next__") && all_rendered.contains("(predicate)"),
            "unexpected all emission: {all_rendered}"
        );

        let find_rendered = render(IteratorMethodKind::Find, callback())?;
        assert!(
            find_rendered.contains("collection :: Iterator :: __next__") && find_rendered.contains("(predicate)"),
            "unexpected find emission: {find_rendered}"
        );

        let for_each_rendered = render(IteratorMethodKind::ForEach, callback())?;
        assert!(
            for_each_rendered.contains("collection :: Iterator :: __next__")
                && for_each_rendered.contains("(predicate) (__incan_item)"),
            "unexpected for_each emission: {for_each_rendered}"
        );

        let sum_rendered = render(IteratorMethodKind::Sum, Vec::new())?;
        assert!(
            sum_rendered.contains("collection :: Iterator :: __next__")
                && sum_rendered.contains("__incan_sum += __incan_item"),
            "unexpected sum emission: {sum_rendered}"
        );

        Ok(())
    }

    fn iterator_receiver() -> TypedExpr {
        TypedExpr::new(
            IrExprKind::Var {
                name: "items".to_string(),
                access: VarAccess::Read,
                ref_kind: VarRefKind::Value,
            },
            IrType::NamedGeneric(core_traits::as_str(TraitId::Iterator).to_string(), vec![IrType::Int]),
        )
    }

    fn function_var(name: &str) -> TypedExpr {
        TypedExpr::new(
            IrExprKind::Var {
                name: name.to_string(),
                access: VarAccess::Read,
                ref_kind: VarRefKind::Value,
            },
            IrType::Unknown,
        )
    }
}
