//! Emit Rust code for method calls.
//!
//! This module handles emission of both known methods (enum-based dispatch via `MethodKind`) and ordinary method calls
//! that should remain Rust method syntax.

use proc_macro2::TokenStream;
use quote::{format_ident, quote};

use super::super::super::FunctionSignature;
use super::super::super::expr::{
    CollectionMethodKind, InternalMethodKind, IrCallArg, IrExprKind, IrMethodDispatch, MethodCallArgPolicy, MethodKind,
    TypedExpr, VarAccess, VarRefKind,
};
use super::super::super::ownership::ValueUseSite;
use super::super::super::types::IrType;
use super::super::{EmitError, IrEmitter};
use incan_core::interop::RustCollectionFamily;
use incan_core::lang::surface::result_methods::{self, ResultMethodId};

mod collection_methods;
mod iterator_methods;
mod string_methods;

use collection_methods::emit_collection_method;
use iterator_methods::emit_iterator_method;
use string_methods::emit_string_method;

/// Compute common receiver setup for method emission.
///
/// This deduplicates the pattern of:
/// - Detecting `FrozenStr` receivers
/// - Unwrapping them via `.as_str()`
pub(super) struct ReceiverInfo {
    /// The receiver token stream (possibly wrapped in `.as_str()` for FrozenStr).
    pub(super) r: TokenStream,
    /// A borrow of the receiver: `&#r`.
    pub(super) r_borrow: TokenStream,
}

impl ReceiverInfo {
    /// Build receiver info from the receiver type and emitted receiver tokens.
    fn new(receiver_ty: &IrType, emitted: TokenStream) -> Self {
        let is_frozen_str = matches!(receiver_ty, IrType::FrozenStr);
        let r = if is_frozen_str {
            quote! { #emitted.as_str() }
        } else {
            emitted
        };
        let r_borrow = quote! { &#r };
        Self { r, r_borrow }
    }
}

fn rust_collection_family_for_ir_type(ty: &IrType) -> Option<RustCollectionFamily> {
    match ty {
        IrType::Struct(name) | IrType::NamedGeneric(name, _) => {
            RustCollectionFamily::for_canonical_path(name).or(RustCollectionFamily::for_type_name(name))
        }
        IrType::Ref(inner) | IrType::RefMut(inner) => rust_collection_family_for_ir_type(inner),
        _ => None,
    }
}

impl<'a> IrEmitter<'a> {
    /// Emit a one-argument callback invocation for a `Result` combinator payload.
    fn emit_result_callback_call(
        &self,
        callback: &TypedExpr,
        payload_tokens: TokenStream,
    ) -> Result<TokenStream, EmitError> {
        let callback_tokens = self.emit_expr(callback)?;
        if matches!(callback.ty, IrType::Function { .. }) {
            Ok(quote! { #callback_tokens(#payload_tokens) })
        } else {
            Ok(quote! { #callback_tokens.__call__(#payload_tokens) })
        }
    }

    /// Emit a one-argument observer invocation for `Result.inspect` / `inspect_err`.
    fn emit_result_observer_callback_call(
        &self,
        callback: &TypedExpr,
        observed_ty: &IrType,
    ) -> Result<TokenStream, EmitError> {
        let borrowed_payload = quote! { __incan_result_value };
        if observed_ty.is_copy() {
            return self.emit_result_callback_call(callback, quote! { *#borrowed_payload });
        }

        match &callback.kind {
            _ if matches!(callback.ty, IrType::Function { .. }) => {
                let callback_tokens = self.emit_expr(callback)?;
                Ok(quote! { #callback_tokens(#borrowed_payload) })
            }
            _ => {
                let callback_tokens = self.emit_expr(callback)?;
                let method_name = callback
                    .ty
                    .nominal_type_name()
                    .filter(|type_name| self.needs_result_observer_callable_helper(type_name))
                    .map(|_| Self::result_observer_borrowed_method_name())
                    .unwrap_or("__call__");
                let method = Self::rust_ident(method_name);
                Ok(quote! { #callback_tokens.#method(#borrowed_payload) })
            }
        }
    }

    /// Return whether a Result observer callback can be routed through the Incan-authored `std.result` helper.
    fn result_observer_can_use_stdlib_helper(&self, callback: &TypedExpr) -> bool {
        match &callback.kind {
            IrExprKind::Var {
                name,
                ref_kind: VarRefKind::Value,
                ..
            } if matches!(callback.ty, IrType::Function { .. }) => self.function_registry.get(name).is_some(),
            _ => false,
        }
    }

    /// Emit the callback argument passed to an Incan-authored `inspect` / `inspect_err` helper.
    fn emit_result_observer_stdlib_callback_arg(
        &self,
        callback: &TypedExpr,
        observed_ty: &IrType,
    ) -> Result<TokenStream, EmitError> {
        if observed_ty.is_copy() {
            return self.emit_expr(callback);
        }
        if let IrExprKind::Var {
            name,
            ref_kind: VarRefKind::Value,
            ..
        } = &callback.kind
            && matches!(callback.ty, IrType::Function { .. })
            && self.needs_borrowed_function_adapter(name, &[0])
        {
            let helper_name = Self::borrowed_function_adapter_name(name, &[0]);
            let helper = Self::rust_ident(&helper_name);
            return Ok(quote! { #helper });
        }
        self.emit_expr(callback)
    }

    /// Return the branch payload type observed by `inspect` or `inspect_err`.
    fn result_observed_type(method: ResultMethodId, receiver_ty: &IrType, callback: &TypedExpr) -> Option<IrType> {
        match (method, receiver_ty) {
            (ResultMethodId::Inspect, IrType::Result(ok, _)) => Some(ok.as_ref().clone()),
            (ResultMethodId::InspectErr, IrType::Result(_, err)) => Some(err.as_ref().clone()),
            (ResultMethodId::Inspect | ResultMethodId::InspectErr, _) => match &callback.ty {
                IrType::Function { params, .. } => params.first().cloned(),
                _ => None,
            },
            _ => None,
        }
    }

    /// Emit Rust for an RFC 070 `Result` combinator call when `method` is in scope.
    fn emit_result_combinator_call(
        &self,
        receiver_tokens: &TokenStream,
        receiver_ty: &IrType,
        method: ResultMethodId,
        callback: &TypedExpr,
    ) -> Result<TokenStream, EmitError> {
        let method_name = result_methods::as_str(method);
        let method_ident = Self::rust_ident(method_name);
        let call = match method {
            ResultMethodId::Map | ResultMethodId::MapErr | ResultMethodId::AndThen | ResultMethodId::OrElse => {
                if self.result_value_combinator_can_use_stdlib_helper(callback) {
                    let callback_tokens = self.emit_expr(callback)?;
                    return Ok(quote! {
                        crate::__incan_std::result::#method_ident(#receiver_tokens, #callback_tokens)
                    });
                }
                let body = self.emit_result_callback_call(callback, quote! { __incan_result_value })?;
                quote! {
                    #receiver_tokens.#method_ident(|__incan_result_value| #body)
                }
            }
            ResultMethodId::Inspect | ResultMethodId::InspectErr => {
                let Some(observed_ty) = Self::result_observed_type(method, receiver_ty, callback) else {
                    return Err(EmitError::Unsupported(format!(
                        "cannot infer observed payload type for Result.{method_name}"
                    )));
                };
                if self.result_observer_can_use_stdlib_helper(callback) {
                    let callback_tokens = self.emit_result_observer_stdlib_callback_arg(callback, &observed_ty)?;
                    return Ok(quote! {
                        crate::__incan_std::result::#method_ident(#receiver_tokens, #callback_tokens)
                    });
                }
                let body = self.emit_result_observer_callback_call(callback, &observed_ty)?;
                quote! {
                    #receiver_tokens.#method_ident(|__incan_result_value| {
                        #body;
                    })
                }
            }
        };
        Ok(call)
    }

    /// Return whether a value-transforming Result combinator can dogfood the pure Incan std.result helper.
    ///
    /// The helpers currently take ordinary function-pointer callbacks, so keep callable objects and closure-shaped
    /// values on the direct Rust combinator path. That preserves the RFC surface while still routing plain named
    /// function references through stdlib-authored Incan code.
    fn result_value_combinator_can_use_stdlib_helper(&self, callback: &TypedExpr) -> bool {
        match &callback.kind {
            IrExprKind::Var {
                name,
                ref_kind: VarRefKind::Value,
                ..
            } if matches!(callback.ty, IrType::Function { .. }) => self.function_registry.get(name).is_some(),
            _ => false,
        }
    }

    /// Return whether an argument already has Rust reference shape for a method parameter.
    fn method_arg_already_borrowed_for_ref_param(arg_ty: &IrType) -> bool {
        matches!(
            arg_ty,
            IrType::Ref(_) | IrType::RefMut(_) | IrType::StrRef | IrType::StaticStr
        )
    }

    /// Emit method-call arguments with Rust-boundary borrowing and union wrapping applied from callable metadata.
    fn emit_method_call_args(
        &self,
        method: &str,
        receiver: &TypedExpr,
        args: &[IrCallArg],
        callable_signature: Option<&FunctionSignature>,
        base_use_site: ValueUseSite<'_>,
    ) -> Result<Vec<TokenStream>, EmitError> {
        let receiver_signature = self.method_signature_for_receiver(&receiver.ty, method);
        let callable_signature = match (callable_signature, receiver_signature) {
            (Some(call_sig), Some(method_sig))
                if call_sig.params.iter().all(|param| param.default.is_none())
                    && method_sig.params.iter().any(|param| param.default.is_some()) =>
            {
                Some(method_sig)
            }
            (Some(call_sig), _) => Some(call_sig),
            (None, method_sig) => method_sig,
        };
        if let Some(sig) = callable_signature
            && sig
                .params
                .iter()
                .any(|param| param.kind != crate::frontend::ast::ParamKind::Normal)
        {
            return self.emit_rest_aware_call_args(receiver, args, sig);
        }

        let ordered_args: Vec<(TypedExpr, bool)> = if let Some(sig) = callable_signature {
            if args.iter().any(|arg| arg.name.is_some()) {
                let mut positional: Vec<TypedExpr> = Vec::new();
                let mut named: std::collections::HashMap<&str, TypedExpr> = std::collections::HashMap::new();
                for arg in args {
                    if let Some(name) = arg.name.as_deref() {
                        named.insert(name, arg.expr.clone());
                    } else {
                        positional.push(arg.expr.clone());
                    }
                }

                let mut pos_idx = 0usize;
                let mut out = Vec::new();
                for param in &sig.params {
                    if let Some(value) = named.get(param.name.as_str()) {
                        out.push((value.clone(), false));
                    } else if pos_idx < positional.len() {
                        out.push((positional[pos_idx].clone(), false));
                        pos_idx += 1;
                    } else if let Some(default_arg) = &param.default {
                        out.push((default_arg.clone(), true));
                    }
                }
                out
            } else {
                let mut out: Vec<(TypedExpr, bool)> = args.iter().map(|arg| (arg.expr.clone(), false)).collect();
                for param in sig.params.iter().skip(out.len()) {
                    if let Some(default_arg) = &param.default {
                        out.push((default_arg.clone(), true));
                    } else {
                        break;
                    }
                }
                out
            }
        } else {
            args.iter().map(|arg| (arg.expr.clone(), false)).collect()
        };

        ordered_args
            .iter()
            .enumerate()
            .map(|(idx, (arg, from_default))| {
                let external_method_shape = matches!(
                    base_use_site,
                    ValueUseSite::ExternalCallArg { .. } | ValueUseSite::MethodArg
                );
                let param = callable_signature.and_then(|sig| sig.params.get(idx));
                let arg_use_site =
                    if external_method_shape && matches!(param.map(|param| &param.ty), Some(IrType::Generic(_))) {
                        ValueUseSite::MethodArg
                    } else {
                        base_use_site
                    };
                let previous_qualify = if *from_default {
                    Some(self.qualify_internal_canonical_paths.replace(true))
                } else {
                    None
                };
                let emitted = self.emit_expr_for_use(arg, arg_use_site);
                if let Some(previous) = previous_qualify {
                    self.qualify_internal_canonical_paths.replace(previous);
                }
                let mut emitted = emitted?;
                if idx == 0
                    && method == "take"
                    && matches!(arg.ty, IrType::Int)
                    && !Self::is_generator_receiver(receiver)
                {
                    emitted = quote! {
                        match u64::try_from(#emitted) {
                            Ok(__incan_take_count) => __incan_take_count,
                            Err(_) => incan_stdlib::errors::raise_value_error(
                                "take() count must be non-negative and fit u64",
                            ),
                        }
                    };
                }
                if idx == 0 && method == "by_ref" {
                    emitted = quote! { &mut *#emitted };
                }
                if idx == 0
                    && Self::receiver_type_for_method_dispatch(&receiver.ty).nominal_type_name() == Some("Path")
                    && Self::method_first_arg_is_path_or_str_union(method)
                    && let Some(wrapped) = self.emit_union_payload_arg(arg, &Self::path_or_str_union_type(), None)?
                {
                    return Ok(wrapped);
                }
                let Some(param) = param else {
                    if external_method_shape && idx == 0 && Self::method_arg_needs_fallback_mut_borrow(method, &arg.ty)
                    {
                        emitted = quote! { &mut #emitted };
                    } else if external_method_shape
                        && idx == 0
                        && Self::method_arg_needs_fallback_borrow(method, &arg.ty)
                    {
                        emitted = quote! { &#emitted };
                    }
                    return Ok(emitted);
                };
                if let Some(wrapped) = self.emit_union_payload_arg(arg, &param.ty, None)? {
                    return Ok(wrapped);
                }
                if external_method_shape && idx == 0 && Self::method_arg_needs_fallback_mut_borrow(method, &arg.ty) {
                    return Ok(quote! { &mut #emitted });
                }
                if external_method_shape && idx == 0 && Self::method_arg_needs_fallback_borrow(method, &arg.ty) {
                    return Ok(quote! { &#emitted });
                }
                match &param.ty {
                    IrType::Ref(_) if matches!(base_use_site, ValueUseSite::MethodArg) => {}
                    IrType::Ref(_) => match &arg.ty {
                        _ if Self::method_arg_already_borrowed_for_ref_param(&arg.ty) => {}
                        _ => emitted = quote! { &#emitted },
                    },
                    IrType::RefMut(_) => match &arg.ty {
                        IrType::Ref(_) | IrType::RefMut(_) => {}
                        _ => emitted = quote! { &mut #emitted },
                    },
                    _ => {}
                }
                Ok(emitted)
            })
            .collect()
    }

    /// Return whether an external Rust method's first argument should be emitted as a mutable borrow.
    fn method_arg_needs_fallback_mut_borrow(method: &str, arg_ty: &IrType) -> bool {
        match method {
            "read_to_string" => true,
            "read" | "read_to_end" | "read_exact" | "read_buf" | "read_buf_exact" => Self::is_byte_buffer_type(arg_ty),
            _ => false,
        }
    }

    /// Return whether an external Rust method's first argument should be emitted as a shared borrow.
    fn method_arg_needs_fallback_borrow(method: &str, arg_ty: &IrType) -> bool {
        match method {
            "write_all" => true,
            "for_label" | "decode" | "encode" => true,
            "write" => Self::is_byte_buffer_type(arg_ty),
            _ => false,
        }
    }

    /// Return whether an IR type can stand in for a mutable Rust byte buffer.
    fn is_byte_buffer_type(ty: &IrType) -> bool {
        matches!(ty, IrType::Bytes | IrType::FrozenBytes)
            || matches!(
                ty,
                IrType::NamedGeneric(name, args)
                    if matches!(name.as_str(), "Vec" | "std::vec::Vec")
                        && matches!(args.as_slice(), [IrType::Int])
            )
    }

    /// Return whether a std.fs method takes `Path | str` as its first user argument.
    fn method_first_arg_is_path_or_str_union(method: &str) -> bool {
        matches!(
            method,
            "copy" | "copy_into" | "move" | "move_into" | "rename" | "replace" | "symlink_to" | "hardlink_to"
        )
    }

    /// Build the canonical anonymous union type used by std.fs path-target methods.
    fn path_or_str_union_type() -> IrType {
        IrType::NamedGeneric(
            crate::backend::ir::types::IR_UNION_TYPE_NAME.to_string(),
            vec![IrType::Struct("Path".to_string()), IrType::String],
        )
    }

    /// Materialize method-call arguments before entering a static storage lock.
    ///
    /// This prevents lock reentry when argument expressions also read/write static-backed values.
    fn materialize_storage_rooted_args(
        &self,
        args: &[IrCallArg],
    ) -> Result<(Vec<TokenStream>, Vec<IrCallArg>), EmitError> {
        let mut bindings = Vec::with_capacity(args.len());
        let mut rewritten = Vec::with_capacity(args.len());
        for (idx, arg) in args.iter().enumerate() {
            let name = format!("__incan_static_arg_{idx}");
            let ident = format_ident!("{}", name);
            let emitted = self.emit_expr(&arg.expr)?;
            bindings.push(quote! { let #ident = #emitted; });
            let rewritten_expr = TypedExpr::new(
                IrExprKind::Var {
                    name,
                    access: VarAccess::Read,
                    ref_kind: VarRefKind::Value,
                },
                arg.expr.ty.clone(),
            )
            .with_ownership(arg.expr.ownership)
            .with_span(arg.expr.span);
            rewritten.push(IrCallArg {
                name: arg.name.clone(),
                kind: arg.kind,
                expr: rewritten_expr,
            });
        }
        Ok((bindings, rewritten))
    }

    /// Strip reference wrappers from a receiver type before builtin-family or ownership-sensitive dispatch.
    ///
    /// Method emission cares about the underlying receiver family (`Dict`, `Struct`, `Trait`, ...) rather than whether
    /// lowering represented the value as `T`, `&T`, or `&mut T`.
    fn receiver_type_for_method_dispatch(receiver_ty: &IrType) -> &IrType {
        let mut receiver_ty = receiver_ty;
        while let IrType::Ref(inner) | IrType::RefMut(inner) = receiver_ty {
            receiver_ty = inner.as_ref();
        }
        receiver_ty
    }

    /// Whether a receiver is a nominal type owned by this compilation unit rather than an external Rust surface.
    ///
    /// Incan-owned structs, enums, traits, and `rusttype` surface aliases use compiler-controlled argument conversion
    /// rules. External nominal types may share the same IR shape (`Struct(name)`) but must usually preserve Rust call
    /// semantics instead.
    fn is_incan_owned_nominal_receiver(&self, receiver_ty: &IrType) -> bool {
        match Self::receiver_type_for_method_dispatch(receiver_ty) {
            IrType::Struct(name) | IrType::NamedGeneric(name, _) => {
                self.struct_field_names.contains_key(name)
                    || self.rusttype_alias_names.contains(name)
                    || self.type_module_paths.contains_key(name)
            }
            IrType::Enum(name) => self.enum_variant_fields.keys().any(|(enum_name, _)| enum_name == name),
            IrType::Trait(_) => true,
            _ => false,
        }
    }

    /// Emit a known method call using enum-based dispatch.
    ///
    /// This handles calls that have been lowered to `IrExprKind::KnownMethodCall`.
    ///
    /// ## Parameters
    ///
    /// - `receiver`: The receiver expression
    /// - `kind`: The method kind enum variant
    /// - `args`: The method call arguments
    ///
    /// ## Returns
    ///
    /// - A Rust `TokenStream` for the method call
    pub(in super::super) fn emit_known_method_call(
        &self,
        receiver: &TypedExpr,
        kind: &MethodKind,
        args: &[IrCallArg],
    ) -> Result<TokenStream, EmitError> {
        if Self::expr_is_storage_rooted(receiver) {
            let (arg_bindings, rewritten_args) = self.materialize_storage_rooted_args(args)?;
            if matches!(kind, MethodKind::Collection(CollectionMethodKind::Get)) {
                let rewritten_receiver = Self::rewrite_storage_root_expr(receiver, "__incan_static_value");
                let arg_exprs: Vec<TypedExpr> = rewritten_args.iter().map(|a| a.expr.clone()).collect();
                let inner = self.emit_static_collection_get(&rewritten_receiver, &arg_exprs)?;
                let wrapped = self.emit_storage_with_ref(receiver, inner)?;
                return Ok(quote! {
                    #(#arg_bindings)*
                    #wrapped
                });
            }

            let rewritten_receiver = Self::rewrite_storage_root_expr(receiver, "__incan_static_value");
            let inner = self.emit_known_method_call(&rewritten_receiver, kind, &rewritten_args)?;
            let use_mut = super::method_kind_uses_mutable_receiver(kind);
            let wrapped = if use_mut {
                self.emit_storage_with_mut(receiver, inner)
            } else {
                self.emit_storage_with_ref(receiver, inner)
            }?;
            return Ok(quote! {
                #(#arg_bindings)*
                #wrapped
            });
        }

        let r0 = self.emit_expr(receiver)?;
        let info = ReceiverInfo::new(&receiver.ty, r0);
        let arg_exprs: Vec<TypedExpr> = args.iter().map(|a| a.expr.clone()).collect();
        match kind {
            MethodKind::String(kind) => emit_string_method(self, &info, kind, &arg_exprs),
            MethodKind::Collection(kind) => emit_collection_method(self, receiver, &info, kind, &arg_exprs),
            MethodKind::Iterator(kind) => emit_iterator_method(self, receiver, &info, kind, &arg_exprs),
            MethodKind::Result(kind) => {
                let Some(callback) = arg_exprs.first() else {
                    return Err(EmitError::Unsupported(format!(
                        "Result.{} expects one callback argument",
                        result_methods::as_str(*kind)
                    )));
                };
                self.emit_result_combinator_call(&info.r, &receiver.ty, *kind, callback)
            }
            MethodKind::Internal(InternalMethodKind::Slice) => self.emit_runtime_str_slice(&info, &arg_exprs),
        }
    }

    /// Emit a method call expression that remains a regular Rust method call.
    ///
    /// This handles `IrExprKind::MethodCall` when lowering did not classify the method as a builtin-family method.
    #[allow(clippy::too_many_arguments)]
    pub(in super::super) fn emit_method_call_expr(
        &self,
        receiver: &TypedExpr,
        method: &str,
        dispatch: Option<&IrMethodDispatch>,
        type_args: &[IrType],
        args: &[IrCallArg],
        callable_signature: Option<&FunctionSignature>,
        arg_policy: MethodCallArgPolicy,
    ) -> Result<TokenStream, EmitError> {
        if Self::expr_is_storage_rooted(receiver) {
            let (arg_bindings, rewritten_args) = self.materialize_storage_rooted_args(args)?;
            let rewritten_receiver = Self::rewrite_storage_root_expr(receiver, "__incan_static_value");
            let inner = self.emit_method_call_expr(
                &rewritten_receiver,
                method,
                dispatch,
                type_args,
                &rewritten_args,
                callable_signature,
                arg_policy,
            )?;
            let wrapped = if matches!(arg_policy, MethodCallArgPolicy::PreserveShape) {
                self.emit_storage_with_ref(receiver, inner)
            } else {
                self.emit_storage_with_mut(receiver, inner)
            }?;
            return Ok(quote! {
                #(#arg_bindings)*
                #wrapped
            });
        }

        let r0 = self.emit_expr(receiver)?;
        let info = ReceiverInfo::new(&receiver.ty, r0);
        let r = &info.r;
        if Self::is_generator_receiver(receiver) && method == "filter" && args.len() == 1 {
            let predicate = self.emit_expr(&args[0].expr)?;
            return Ok(quote! {
                #r.filter(move |__incan_gen_item| #predicate((*__incan_gen_item).clone()))
            });
        }
        let method_turbofish = if type_args.is_empty() {
            quote! {}
        } else {
            let emitted: Vec<TokenStream> = type_args.iter().map(|ty| self.emit_type(ty)).collect();
            quote! { ::<#(#emitted),*> }
        };
        let arg_exprs: Vec<TypedExpr> = args.iter().map(|a| a.expr.clone()).collect();

        // Check if this is an enum variant construction.
        //
        // Important: do NOT treat any uppercase variable as a type name. Only rewrite when we actually know this
        // (Type, Variant) pair exists in the enum variant registry.
        if let IrExprKind::Var { name, .. } = &receiver.kind {
            let key = (name.to_string(), method.to_string());
            if self.enum_variant_fields.contains_key(&key) {
                return self.emit_enum_variant_call(name, method, &arg_exprs);
            }
        }

        // Associated function call on a type: `Type.method(...)` → `Type::method(...)`
        //
        // This is needed for external Rust types like `Uuid`, `Instant`, `HashMap`, and also for
        // Incan-generated impl methods called in a "static" style (e.g. `User.from_json(...)`).
        if let IrExprKind::Var { name, .. } = &receiver.kind {
            // Rewrite `Type.method(...)` to `Type::method(...)` only when we have explicit metadata that this is
            // a type-like identifier (type name or external import placeholder).
            //
            // This avoids capitalization heuristics that can mis-emit runtime variables named `TitleCase`.
            if Self::expr_is_type_like(receiver) {
                let type_ident = Self::rust_ident(name);
                let type_path = match &receiver.ty {
                    IrType::NamedGeneric(type_name, type_args) if type_name == name => {
                        let emitted: Vec<TokenStream> = type_args.iter().map(|ty| self.emit_type(ty)).collect();
                        quote! { #type_ident :: <#(#emitted),*> }
                    }
                    _ => quote! { #type_ident },
                };
                let m = Self::rust_ident(method);
                let in_return = *self.in_return_context.borrow();
                let receiver_ref_kind = match &receiver.kind {
                    IrExprKind::Var { ref_kind, .. } => Some(*ref_kind),
                    _ => None,
                };
                // Apply Incan-style argument conversions when calling associated functions on Incan-owned types
                // (structs/enums/traits). This is important for `str` literals which are emitted as `&'static str`,
                // but many Incan-level signatures expect owned `String` in Rust (e.g., newtype `from_underlying(v:
                // str)`).
                //
                // `VarRefKind::TypeName` covers imported Incan types like `std.web.Response` (typechecker
                // `IdentKind::TypeName`). Those calls must use Incan arg rules — `ExternalFunctionArg`
                // would borrow `String` as `&String` and break signatures such as
                // `Response::html(content: String)` in generated stdlib.
                //
                // For external Rust types (VarRefKind::ExternalRustName), use ExternalFunctionArg conversions so that
                // string literals get `.into()` — this lets the Rust compiler resolve the target type via the Into
                // trait (e.g., Polars' PlSmallStr, sqlx identifiers, etc.).
                let use_site = if receiver_ref_kind != Some(VarRefKind::ExternalRustName)
                    && self.is_incan_owned_nominal_receiver(&receiver.ty)
                {
                    ValueUseSite::IncanCallArg {
                        target_ty: None,
                        callee_param: None,
                        in_return: false,
                    }
                } else if matches!(receiver_ref_kind, Some(VarRefKind::ExternalName | VarRefKind::TypeName)) {
                    ValueUseSite::IncanCallArg {
                        target_ty: None,
                        callee_param: None,
                        in_return,
                    }
                } else {
                    ValueUseSite::ExternalCallArg { target_ty: None }
                };
                let arg_tokens = self.emit_method_call_args(method, receiver, args, callable_signature, use_site)?;
                return Ok(quote! { #type_path::#m #method_turbofish (#(#arg_tokens),*) });
            }
        }

        if let Some(IrMethodDispatch::Trait { trait_path, type_args }) = dispatch {
            let path_tokens: Vec<TokenStream> = trait_path
                .split("::")
                .map(|segment| {
                    let ident = Self::rust_ident(segment);
                    quote! { #ident }
                })
                .collect();
            let trait_tokens = super::super::decls::join_path_tokens(&path_tokens);
            let trait_type_args: Vec<TokenStream> = type_args.iter().map(|ty| self.emit_type(ty)).collect();
            let trait_tokens = if trait_type_args.is_empty() {
                quote! { #trait_tokens }
            } else {
                quote! { #trait_tokens :: < #(#trait_type_args),* > }
            };
            let m = Self::rust_ident(method);
            let arg_tokens =
                self.emit_method_call_args(method, receiver, args, callable_signature, ValueUseSite::MethodArg)?;
            return Ok(quote! { #trait_tokens::#m(&#r, #(#arg_tokens),*) });
        }

        // Regular method call
        let m = Self::rust_ident(method);
        // Apply Incan-style argument conversions for methods on nominal types emitted by this compilation unit.
        // This is important for `str` literals: we often emit `"x"` as `&'static str`, but many Incan-level method
        // signatures expect owned `String` in Rust.
        //
        // Do not key this off `IrType::Struct` alone: Rust interop types such as `HashMap` also lower to nominal IR
        // types, but they should still use Rust call semantics rather than compiler-owned cloning/to_string policies.
        let in_return = *self.in_return_context.borrow();
        let receiver_ref_kind = match &receiver.kind {
            IrExprKind::Var { ref_kind, .. } => Some(*ref_kind),
            _ => None,
        };
        let preserve_lookup_arg_shape = matches!(arg_policy, MethodCallArgPolicy::PreserveShape)
            || rust_collection_family_for_ir_type(&receiver.ty)
                .is_some_and(|family| family.preserves_lookup_arg_shape(method));
        let use_site = if receiver_ref_kind != Some(VarRefKind::ExternalRustName)
            && self.is_incan_owned_nominal_receiver(&receiver.ty)
        {
            ValueUseSite::IncanCallArg {
                target_ty: None,
                callee_param: None,
                in_return: false,
            }
        } else if receiver_ref_kind == Some(VarRefKind::ExternalName) {
            // Module-qualified calls like `widgets.make_widget(...)` are function namespace lookups, not external Rust
            // methods. They should keep ordinary Incan/public-function conversions instead of Rust interop coercions.
            ValueUseSite::IncanCallArg {
                target_ty: None,
                callee_param: None,
                in_return,
            }
        } else if preserve_lookup_arg_shape {
            // Borrow-sensitive collection lookups must keep the source argument shape instead of applying
            // function-style coercions such as `.to_string()` / `.into()`.
            ValueUseSite::MethodArg
        } else {
            ValueUseSite::ExternalCallArg { target_ty: None }
        };
        let arg_tokens = self.emit_method_call_args(method, receiver, args, callable_signature, use_site)?;
        Ok(quote! { #r.#m #method_turbofish (#(#arg_tokens),*) })
    }

    /// Emit a runtime string slice call using shared stdlib/semantics helpers.
    ///
    /// This ensures emitted Rust uses the same Unicode/panic behavior as runtime and avoids drift
    /// from direct range slicing on Rust strings.
    fn emit_runtime_str_slice(&self, info: &ReceiverInfo, args: &[TypedExpr]) -> Result<TokenStream, EmitError> {
        let r_borrow = &info.r_borrow;

        let start_tokens = if let Some(arg0) = args.first() {
            let start = self.emit_expr(arg0)?;
            quote! { Some((#start) as i64) }
        } else {
            quote! { None }
        };

        let end_tokens = if let Some(arg1) = args.get(1) {
            if matches!(arg1.kind, IrExprKind::Int(-1)) {
                quote! { None }
            } else {
                let end = self.emit_expr(arg1)?;
                quote! { Some((#end) as i64) }
            }
        } else {
            quote! { None }
        };

        Ok(quote! { incan_stdlib::strings::str_slice(#r_borrow, #start_tokens, #end_tokens, None) })
    }

    fn emit_static_collection_get(&self, receiver: &TypedExpr, args: &[TypedExpr]) -> Result<TokenStream, EmitError> {
        let r = self.emit_expr(receiver)?;
        let Some(arg) = args.first() else {
            return Ok(quote! { None });
        };
        let emitted_arg = self.emit_expr(arg)?;
        match &receiver.ty {
            IrType::Dict(_, value_ty) => {
                let key = collection_methods::emit_dict_lookup_key(receiver, arg, emitted_arg);
                if value_ty.is_copy() {
                    Ok(quote! { #r.get(#key).copied() })
                } else {
                    Ok(quote! { #r.get(#key).cloned() })
                }
            }
            IrType::List(elem_ty) => {
                if elem_ty.is_copy() {
                    Ok(quote! { #r.get((#emitted_arg) as usize).copied() })
                } else {
                    Ok(quote! { #r.get((#emitted_arg) as usize).cloned() })
                }
            }
            _ => Ok(quote! { #r.get(#emitted_arg).cloned() }),
        }
    }

    /// Emit an enum variant construction call (Type.Variant(...) -> Type::Variant(...)).
    pub(in super::super) fn emit_enum_variant_call(
        &self,
        type_name: &str,
        variant: &str,
        args: &[TypedExpr],
    ) -> Result<TokenStream, EmitError> {
        let variant_key = (type_name.to_string(), variant.to_string());
        let arg_tokens: Vec<TokenStream> = if let Some(fields) = self.enum_variant_fields.get(&variant_key) {
            match fields {
                super::super::super::decl::VariantFields::Unit => Vec::new(),
                super::super::super::decl::VariantFields::Tuple(field_tys) => args
                    .iter()
                    .zip(field_tys.iter())
                    .map(|(a, ty)| {
                        self.emit_expr_for_use(
                            a,
                            ValueUseSite::IncanCallArg {
                                target_ty: Some(ty),
                                callee_param: None,
                                in_return: false,
                            },
                        )
                    })
                    .collect::<Result<_, _>>()?,
                super::super::super::decl::VariantFields::Struct(_) => args
                    .iter()
                    .map(|a| {
                        self.emit_expr_for_use(
                            a,
                            ValueUseSite::IncanCallArg {
                                target_ty: None,
                                callee_param: None,
                                in_return: false,
                            },
                        )
                    })
                    .collect::<Result<_, _>>()?,
            }
        } else {
            args.iter()
                .map(|a| {
                    self.emit_expr_for_use(
                        a,
                        ValueUseSite::IncanCallArg {
                            target_ty: Some(&IrType::String),
                            callee_param: None,
                            in_return: false,
                        },
                    )
                })
                .collect::<Result<_, _>>()?
        };

        let type_ident = format_ident!("{}", type_name);
        let m = format_ident!("{}", variant);
        Ok(quote! { #type_ident::#m(#(#arg_tokens),*) })
    }

    /// Return whether a method receiver is the RFC 006 runtime generator wrapper.
    fn is_generator_receiver(receiver: &TypedExpr) -> bool {
        matches!(&receiver.ty, IrType::NamedGeneric(name, _)
            if incan_core::lang::types::collections::from_str(name.as_str())
                == Some(incan_core::lang::types::collections::CollectionTypeId::Generator))
    }
}
