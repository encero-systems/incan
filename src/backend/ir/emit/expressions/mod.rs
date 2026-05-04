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
//! - [`comprehensions`]: List and dict comprehensions
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
mod lvalue;
mod methods;
mod structs_enums;

use proc_macro2::{Literal, TokenStream};
use quote::{ToTokens, format_ident, quote};

use super::super::decl::IrInteropAdapterKind;
use super::super::expr::{
    CollectionMethodKind, IrDictEntry, IrExprKind, IrInteropCoercionKind, IrListEntry, Literal as IrLiteral,
    MethodKind, TypedExpr, UnaryOp, VarRefKind,
};
use super::super::types::IrType;
use super::{EmitError, IrEmitter};
use crate::backend::ir::ownership::{ValueUseSite, plan_value_use};

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
            return Ok(quote! { HashMap::new() });
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
            return Ok(quote! { [#(#pair_tokens),*].into_iter().collect::<HashMap<_, _>>() });
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
            let mut __incan_dict = HashMap::new();
            #(#steps)*
            __incan_dict
        }})
    }

    /// Return the target type carried by a value-use site, if the site has one.
    fn use_site_target_ty<'b>(site: ValueUseSite<'b>) -> Option<&'b IrType> {
        match site {
            ValueUseSite::IncanCallArg { target_ty, .. }
            | ValueUseSite::ExternalCallArg { target_ty }
            | ValueUseSite::StructField { target_ty }
            | ValueUseSite::CollectionElement { target_ty }
            | ValueUseSite::Assignment { target_ty }
            | ValueUseSite::ReturnValue { target_ty }
            | ValueUseSite::MatchScrutinee { target_ty } => target_ty,
            ValueUseSite::MethodArg => None,
        }
    }

    /// Rebuild a parent value-use site for one tuple item while preserving the parent ownership context.
    fn tuple_item_use_site<'b>(site: ValueUseSite<'b>, target_ty: Option<&'b IrType>) -> ValueUseSite<'b> {
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

    /// Emit an expression directly against an ownership-planned sink/source boundary.
    ///
    /// Aggregate literals are handled recursively so element-level ownership policy is applied before the outer
    /// expression is emitted. Non-aggregate expressions are emitted normally, then the planned conversion is applied to
    /// the resulting token stream.
    pub(super) fn emit_expr_for_use(&self, expr: &TypedExpr, site: ValueUseSite<'_>) -> Result<TokenStream, EmitError> {
        match &expr.kind {
            IrExprKind::List(items) => {
                let item_target_ty = match Self::use_site_target_ty(site) {
                    Some(IrType::List(elem)) => Some(elem.as_ref()),
                    _ => match &expr.ty {
                        IrType::List(elem) => Some(elem.as_ref()),
                        _ => None,
                    },
                };
                return self.emit_list_literal_entries(items, item_target_ty);
            }
            IrExprKind::Dict(pairs) => {
                let (key_target_ty, value_target_ty) = match Self::use_site_target_ty(site) {
                    Some(IrType::Dict(key, value)) => (Some(key.as_ref()), Some(value.as_ref())),
                    _ => match &expr.ty {
                        IrType::Dict(key, value) => (Some(key.as_ref()), Some(value.as_ref())),
                        _ => (None, None),
                    },
                };
                return self.emit_dict_literal_entries(pairs, key_target_ty, value_target_ty);
            }
            IrExprKind::Set(items) => {
                if items.is_empty() {
                    return Ok(quote! { HashSet::new() });
                }
                let item_target_ty = match Self::use_site_target_ty(site) {
                    Some(IrType::Set(elem)) => Some(elem.as_ref()),
                    _ => match &expr.ty {
                        IrType::Set(elem) => Some(elem.as_ref()),
                        _ => None,
                    },
                };
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
                return Ok(quote! { [#(#item_tokens),*].into_iter().collect::<HashSet<_>>() });
            }
            IrExprKind::Tuple(items) => {
                let tuple_target_items = match Self::use_site_target_ty(site) {
                    Some(IrType::Tuple(items)) => Some(items.as_slice()),
                    _ => match &expr.ty {
                        IrType::Tuple(items) => Some(items.as_slice()),
                        _ => None,
                    },
                };
                let item_tokens: Vec<TokenStream> = items
                    .iter()
                    .enumerate()
                    .map(|(idx, item)| {
                        let item_target_ty = tuple_target_items.and_then(|items| items.get(idx));
                        self.emit_expr_for_use(item, Self::tuple_item_use_site(site, item_target_ty))
                    })
                    .collect::<Result<_, _>>()?;
                return Ok(quote! { (#(#item_tokens),*) });
            }
            _ => {}
        }

        let emitted = self.emit_expr(expr)?;
        let plan = plan_value_use(expr, site);
        Ok(plan.apply(emitted))
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

    pub(super) fn rewrite_storage_root_expr(expr: &TypedExpr, local_name: &str) -> TypedExpr {
        let replacement = || {
            TypedExpr::new(
                IrExprKind::Var {
                    name: local_name.to_string(),
                    access: super::super::expr::VarAccess::Read,
                    ref_kind: VarRefKind::Value,
                },
                expr.ty.clone(),
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
                    object: Box::new(Self::rewrite_storage_root_expr(object, local_name)),
                    field: field.clone(),
                },
                expr.ty.clone(),
            ),
            IrExprKind::Index { object, index } => TypedExpr::new(
                IrExprKind::Index {
                    object: Box::new(Self::rewrite_storage_root_expr(object, local_name)),
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

    pub(super) fn emit_storage_with_ref(&self, expr: &TypedExpr, body: TokenStream) -> Result<TokenStream, EmitError> {
        let local_name = format_ident!("__incan_static_value");
        match Self::expr_storage_root(expr) {
            Some(StorageRoot::Static(name)) => {
                let ident = Self::rust_static_ident(&name);
                let init_call = self.emit_module_static_init_call();
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

    pub(super) fn emit_storage_with_mut(&self, expr: &TypedExpr, body: TokenStream) -> Result<TokenStream, EmitError> {
        let local_name = format_ident!("__incan_static_value");
        match Self::expr_storage_root(expr) {
            Some(StorageRoot::Static(name)) => {
                let ident = Self::rust_static_ident(&name);
                let init_call = self.emit_module_static_init_call();
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
            IrExprKind::Float(n) => Ok(quote! { #n }),
            IrExprKind::String(s) => Ok(quote! { #s }),
            IrExprKind::Bytes(bytes) => {
                let lit = Literal::byte_string(bytes);
                Ok(lit.to_token_stream())
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
                let n = Self::rust_ident(name);
                Ok(quote! { #n })
            }

            IrExprKind::StaticRead { name } => {
                let n = Self::rust_static_ident(name);
                if *self.in_static_initializer.borrow() {
                    Ok(quote! { #n.get() })
                } else {
                    let init_call = self.emit_module_static_init_call();
                    Ok(quote! {{
                        #init_call
                        #n.get()
                    }})
                }
            }

            IrExprKind::StaticBinding { name } => {
                let n = Self::rust_static_ident(name);
                if *self.in_static_initializer.borrow() {
                    Ok(quote! { incan_stdlib::storage::StaticBinding::from_static(&#n) })
                } else {
                    let init_call = self.emit_module_static_init_call();
                    Ok(quote! {{
                        #init_call
                        incan_stdlib::storage::StaticBinding::from_static(&#n)
                    }})
                }
            }

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
                type_args,
                args,
                callable_signature,
                arg_policy,
            } => self.emit_method_call_expr(
                receiver,
                method,
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
                    Ok(quote! { HashSet::new() })
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
                    Ok(quote! { [#(#item_tokens),*].into_iter().collect::<HashSet<_>>() })
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
                let s = self.emit_expr_for_use(
                    scrutinee,
                    ValueUseSite::MatchScrutinee {
                        target_ty: Some(&scrutinee.ty),
                    },
                )?;
                let arm_tokens: Vec<TokenStream> = arms
                    .iter()
                    .map(|arm| {
                        let pat = self.emit_pattern(&arm.pattern);
                        let body = self.emit_expr(&arm.body)?;
                        if let Some(guard) = &arm.guard {
                            let g = self.emit_expr(guard)?;
                            Ok(quote! { #pat if #g => #body })
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
                    .map(|(pname, _pty)| {
                        let n = Self::rust_ident(pname);
                        quote! { #n }
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

            IrExprKind::Try(inner) => {
                let i = self.emit_expr(inner)?;
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

            IrExprKind::InteropCoerce {
                expr: inner,
                from_ty: _,
                to_ty: _,
                kind,
            } => {
                let inner = self.emit_expr(inner)?;
                match kind {
                    IrInteropCoercionKind::Builtin { policy, rust_target } => {
                        let rust_target = rust_target.replace(' ', "");
                        let emitted = match policy {
                            incan_core::interop::CoercionPolicy::Exact => match rust_target.as_str() {
                                "String" | "std::string::String" => {
                                    quote! { (#inner).to_string() }
                                }
                                "Vec<u8>" | "std::vec::Vec<u8>" => {
                                    quote! { (#inner).to_vec() }
                                }
                                _ => quote! { #inner },
                            },
                            incan_core::interop::CoercionPolicy::Borrow => match rust_target.as_str() {
                                "&str" | "&[u8]" => quote! { #inner },
                                "&String" | "&std::string::String" | "&alloc::string::String" => {
                                    quote! { &(#inner).to_string() }
                                }
                                "&Vec<u8>" | "&std::vec::Vec<u8>" | "&alloc::vec::Vec<u8>" => {
                                    quote! { &(#inner).to_vec() }
                                }
                                _ => quote! { &#inner },
                            },
                            incan_core::interop::CoercionPolicy::Lossy => match rust_target.as_str() {
                                "f32" => quote! { (#inner) as f32 },
                                _ => quote! { #inner },
                            },
                        };
                        Ok(emitted)
                    }
                    IrInteropCoercionKind::AdapterCall { adapter, adapter_kind } => {
                        let adapter = self.emit_expr(adapter)?;
                        let call = quote! { #adapter(#inner) };
                        let emitted = match adapter_kind {
                            IrInteropAdapterKind::Via => call,
                            IrInteropAdapterKind::Try => quote! { #call? },
                        };
                        Ok(emitted)
                    }
                    IrInteropCoercionKind::RustTypeUnwrap => Ok(quote! { #inner }),
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
                    serde_json::from_str::<#type_ident>(&s)
                        .map_err(|e| incan_stdlib::errors::json_decode_error_string(e))
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::ir::FunctionRegistry;
    use crate::backend::ir::expr::{
        CollectionMethodKind, IrCallArg, IrCallArgKind, MethodCallArgPolicy, MethodKind, VarAccess, VarRefKind,
    };

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
                to_ty: IrType::Ref(Box::new(IrType::String)),
                kind: IrInteropCoercionKind::Builtin {
                    policy: incan_core::interop::CoercionPolicy::Borrow,
                    rust_target: "&String".to_string(),
                },
            },
            IrType::Ref(Box::new(IrType::String)),
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
                callable_signature: None,
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
}
