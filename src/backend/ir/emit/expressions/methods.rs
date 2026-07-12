//! Emit Rust code for method calls.
//!
//! This module handles emission of both known methods (enum-based dispatch via `MethodKind`) and ordinary method calls
//! that should remain Rust method syntax.

use proc_macro2::TokenStream;
use quote::{format_ident, quote};

use super::super::super::FunctionSignature;
use super::super::super::decl::FunctionParam;
use super::super::super::expr::{
    CollectionMethodKind, InternalMethodKind, IrCallArg, IrCallArgKind, IrExprKind, IrMethodDispatch,
    MethodCallArgPolicy, MethodKind, TypedExpr, VarAccess, VarRefKind,
};
use super::super::super::ownership::{
    ArgumentPassingPlan, AssociatedFunctionArgumentContext, RegularMethodArgumentContext, ValueUseSite,
    associated_function_argument_use_site, is_byte_buffer_type, is_string_buffer_type,
    regular_method_argument_use_site,
};
use super::super::super::reference_shape::{expr_has_rust_reference_shape, type_has_rust_reference_shape};
use super::super::super::types::IrType;
use super::super::{EmitError, IrEmitter};
use incan_core::interop::{
    METADATA_FREE_METHOD_BORROW_RULES, MetadataFreeArgClass, MetadataFreeMethodArgBorrowPolicy,
    MetadataFreeReceiverClass, RustCollectionFamily,
};
use incan_core::lang::surface::result_methods::{self, ResultMethodId};
use incan_core::lang::{magic_methods, trait_bounds::rust as tb};

mod collection_methods;
mod fast_paths;
mod iterator_methods;
mod string_methods;

use collection_methods::emit_collection_method;
use fast_paths::emit_registered_method_fast_path;
use iterator_methods::emit_iterator_method;
use string_methods::emit_string_method;

/// Shared settings for emitting one method call's argument list.
#[derive(Clone, Copy)]
struct MethodCallArgEmission<'sig, 'site> {
    callable_signature: Option<&'sig FunctionSignature>,
    base_use_site: ValueUseSite<'site>,
    result_target_ty: Option<&'site IrType>,
    infer_unresolved_generic_args: bool,
}

/// Return the trait path used for type-level reflection.
fn type_reflection_trait_path(method: &str) -> Option<&'static str> {
    match magic_methods::from_str(method) {
        Some(magic_methods::MagicMethodId::ClassName) => Some(tb::INCAN_TYPE_CLASS_NAME),
        Some(magic_methods::MagicMethodId::Fields) => Some(tb::INCAN_TYPE_FIELD_METADATA),
        _ => None,
    }
}

/// Compute common receiver setup for method emission.
///
/// This deduplicates the pattern of:
/// - Detecting `FrozenStr` receivers
/// - Viewing them through `AsRef<str>`
pub(super) struct ReceiverInfo {
    /// The receiver token stream, possibly viewed as `&str` for frozen/imported string values.
    pub(super) r: TokenStream,
    /// A borrow of the receiver: `&#r`.
    pub(super) r_borrow: TokenStream,
}

impl ReceiverInfo {
    /// Build receiver info from the receiver type and emitted receiver tokens.
    fn new(receiver_ty: &IrType, emitted: TokenStream) -> Self {
        let is_frozen_str = matches!(receiver_ty, IrType::FrozenStr);
        let r = if is_frozen_str {
            quote! { <_ as AsRef<str>>::as_ref(&#emitted) }
        } else {
            emitted
        };
        let r_borrow = quote! { &#r };
        Self { r, r_borrow }
    }
}

/// Classify an IR type as a Rust collection family.
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
            Ok(quote! { (#callback_tokens)(#payload_tokens) })
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
                Ok(quote! { (#callback_tokens)(#borrowed_payload) })
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
                    let helper_path = Self::result_stdlib_helper_path();
                    return Ok(quote! {
                        #helper_path::#method_ident(#receiver_tokens, #callback_tokens)
                    });
                }
                if matches!(callback.kind, IrExprKind::Closure { .. }) {
                    let callback_tokens = self.emit_expr(callback)?;
                    return Ok(quote! {
                        #receiver_tokens.#method_ident(#callback_tokens)
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
                    let helper_path = Self::result_stdlib_helper_path();
                    return Ok(quote! {
                        #helper_path::#method_ident(#receiver_tokens, #callback_tokens)
                    });
                }
                let body = self.emit_result_observer_callback_call(callback, &observed_ty)?;
                quote! {
                    #receiver_tokens.#method_ident(|__incan_result_value| {
                        #body;
                    })
                }
            }
            ResultMethodId::Unwrap | ResultMethodId::UnwrapOr => {
                return Err(EmitError::Unsupported(format!(
                    "Result.{method_name} is not a callback combinator"
                )));
            }
        };
        Ok(call)
    }

    /// Return the Result helper module for the current generated crate.
    ///
    /// Normal consumers link the compiled stdlib artifact, while the artifact
    /// build itself still uses its crate-local compatibility facade.
    fn result_stdlib_helper_path() -> TokenStream {
        if std::env::var_os("INCAN_INTERNAL_BUILTIN_STDLIB_ARTIFACT_BUILD").is_some() {
            quote! { crate::__incan_std::result }
        } else {
            quote! { incan_builtin_stdlib::result }
        }
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
        type_has_rust_reference_shape(arg_ty)
    }

    /// Return whether an external Rust generic parameter should infer from this argument's original string shape.
    fn unresolved_external_generic_should_infer_from_string_arg(arg: &TypedExpr, param_ty: &IrType) -> bool {
        matches!(param_ty, IrType::Generic(_))
            && (matches!(
                arg.kind,
                IrExprKind::String(_) | IrExprKind::Literal(super::super::super::expr::Literal::StaticStr(_))
            ) || matches!(
                arg.ty,
                IrType::String | IrType::StaticStr | IrType::StrRef | IrType::FrozenStr
            ) || is_string_buffer_type(&arg.ty))
    }

    /// Return whether `std::path::Path.new` should preserve its source string shape for a borrowed generic parameter.
    ///
    /// Rust's `Path::new` takes a generic `S: AsRef<OsStr>`. A direct literal
    /// or compiler-managed static string provides the necessary inference shape; adding `.into()`
    /// introduces an unconstrained intermediate target once consumer dependencies
    /// provide additional `AsRef<OsStr>` implementations. Owned runtime strings must instead follow the ordinary
    /// external borrow boundary and emit `&value` for Path's `&S` parameter.
    fn std_path_new_preserves_static_string_argument_shape(
        receiver: &TypedExpr,
        method: &str,
        arg: &TypedExpr,
    ) -> bool {
        Self::is_std_path_new_call(receiver, method)
            && (Self::static_string_source_shape(arg)
                || matches!(
                    arg.ty,
                    IrType::String | IrType::StaticStr | IrType::StrRef | IrType::FrozenStr
                ))
    }

    /// Return whether this associated call is the Rust standard-library `Path::new` constructor.
    fn is_std_path_new_call(receiver: &TypedExpr, method: &str) -> bool {
        method == "new"
            && matches!(
                &receiver.ty,
                IrType::Struct(path) | IrType::RustDisplay(path) | IrType::NamedGeneric(path, _)
                    if matches!(path.as_str(), "std::path::Path" | "Path")
            )
    }

    /// Return whether an expression is a literal or compiler-static string after transparent interop coercions.
    ///
    /// Const bindings can be wrapped while their declared Incan `str` type is reconciled with an external method
    /// parameter. The wrapper must not turn the source into an owned `String`: `Path::new(CONST.into())` leaves Rust
    /// with no concrete generic target, whereas the original static string is already a valid `AsRef<OsStr>` input.
    fn static_string_source_shape(expr: &TypedExpr) -> bool {
        match &expr.kind {
            IrExprKind::String(_) | IrExprKind::Literal(super::super::super::expr::Literal::StaticStr(_)) => true,
            IrExprKind::StaticRead { .. }
            | IrExprKind::StaticBinding { .. }
            | IrExprKind::Var {
                ref_kind: VarRefKind::StaticBinding,
                ..
            } => true,
            IrExprKind::InteropCoerce { expr, .. } => Self::static_string_source_shape(expr),
            _ => false,
        }
    }

    /// Emit method-call arguments with Rust-boundary borrowing and union wrapping applied from callable metadata.
    ///
    /// The context's `infer_unresolved_generic_args` flag is enabled only for calls without explicit call-site type
    /// arguments. It lets string-like arguments keep their original Rust shape for unresolved external generics such as
    /// `E: Into<_>`, while explicit calls like `encode[str](...)` stay target-driven.
    fn emit_method_call_args(
        &self,
        method: &str,
        receiver: &TypedExpr,
        args: &[IrCallArg],
        context: MethodCallArgEmission<'_, '_>,
    ) -> Result<Vec<TokenStream>, EmitError> {
        let receiver_target_ty = match context.result_target_ty {
            Some(IrType::Result(ok_ty, _)) => Some(ok_ty.as_ref()),
            other => other,
        };
        let receiver_specialized_signature = self.specialized_method_signature_for_receiver(&receiver.ty, method);
        let target_specialized_signature =
            receiver_target_ty.and_then(|ty| self.specialized_method_signature_for_receiver(ty, method));
        let result_specialized_call_signature = context.callable_signature.and_then(|signature| {
            context
                .result_target_ty
                .and_then(|ty| Self::specialize_signature_by_result_target(signature, ty))
        });
        let receiver_specialized_call_signature = context.callable_signature.and_then(|signature| {
            receiver_target_ty.and_then(|ty| Self::specialize_signature_by_receiver_args(signature, ty))
        });
        let callable_signature = result_specialized_call_signature
            .as_ref()
            .or(receiver_specialized_call_signature.as_ref())
            .or(context.callable_signature);
        let receiver_signature = receiver_specialized_signature
            .as_ref()
            .or_else(|| self.method_signature_for_receiver(&receiver.ty, method))
            .or(target_specialized_signature.as_ref());
        let has_incan_receiver_signature = receiver_signature.is_some();
        let callable_signature =
            FunctionSignature::merge_default_source_by(callable_signature, receiver_signature, |left, right| {
                self.call_signature_type_matches(left, right)
            });
        if let Some(sig) = callable_signature.as_ref()
            && sig
                .params
                .iter()
                .any(|param| param.kind != crate::frontend::ast::ParamKind::Normal)
        {
            return self.emit_rest_aware_call_args(receiver, args, sig);
        }
        let receiver_union_qualifier = Self::pub_library_union_qualifier_for_method_receiver(receiver);

        let ordered_args: Vec<(TypedExpr, bool)> = if let Some(sig) = callable_signature.as_ref() {
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
                let param = callable_signature.as_ref().and_then(|sig| sig.params.get(idx));
                let external_method_shape = matches!(
                    context.base_use_site,
                    ValueUseSite::ExternalCallArg { .. } | ValueUseSite::MethodArg
                );
                let arg_use_site = if Self::std_path_new_preserves_static_string_argument_shape(receiver, method, arg) {
                    ValueUseSite::MethodArg
                } else {
                    match (context.base_use_site, param) {
                        (ValueUseSite::ExternalCallArg { .. }, Some(param))
                            if context.infer_unresolved_generic_args
                                && Self::unresolved_external_generic_should_infer_from_string_arg(arg, &param.ty) =>
                        {
                            ValueUseSite::ExternalInferredGenericArg {
                                target_ty: Some(&param.ty),
                            }
                        }
                        (ValueUseSite::ExternalCallArg { .. }, Some(param)) => ValueUseSite::ExternalCallArg {
                            target_ty: Some(&param.ty),
                        },
                        (ValueUseSite::IncanCallArg { in_return, .. }, Some(param)) => ValueUseSite::IncanCallArg {
                            target_ty: Some(&param.ty),
                            callee_param: Some(param),
                            in_return,
                        },
                        _ => context.base_use_site,
                    }
                };
                let previous_qualify = if *from_default {
                    Some(self.qualify_internal_canonical_paths.replace(true))
                } else {
                    None
                };
                let direct_mut_trait_receiver = external_method_shape
                    && idx == 0
                    && Self::external_trait_first_arg_needs_mut_borrow(receiver, method);
                let target_arg_plan = ArgumentPassingPlan::for_use_site(arg, arg_use_site);
                let metadata_free_policy = if (external_method_shape || !has_incan_receiver_signature)
                    && !target_arg_plan.has_external_value_adapter()
                    && idx == 0
                    && !param.is_some_and(|param| Self::method_arg_already_borrowed_for_ref_param(&param.ty))
                {
                    Self::metadata_free_method_arg_borrow_policy(receiver, method, &arg.ty)
                } else {
                    None
                };
                let effective_arg_use_site = if metadata_free_policy.is_some() {
                    ValueUseSite::MethodArg
                } else {
                    arg_use_site
                };
                let arg_plan = if metadata_free_policy.is_some() {
                    ArgumentPassingPlan::for_use_site(arg, effective_arg_use_site)
                } else {
                    target_arg_plan
                };
                let emitted = if direct_mut_trait_receiver {
                    self.emit_expr(arg)
                } else {
                    self.emit_expr_for_use_with_union_qualifier(
                        arg,
                        effective_arg_use_site,
                        receiver_union_qualifier.as_deref(),
                    )
                };
                if let Some(previous) = previous_qualify {
                    self.qualify_internal_canonical_paths.replace(previous);
                }
                let mut emitted = emitted?;
                if direct_mut_trait_receiver {
                    return Ok(quote! { &mut #emitted });
                }
                if Self::is_std_path_new_call(receiver, method)
                    && matches!(arg.ty, IrType::String)
                    && !Self::static_string_source_shape(arg)
                    && !expr_has_rust_reference_shape(arg)
                {
                    emitted = quote! { &#emitted };
                }
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
                if let Some(policy) = metadata_free_policy {
                    emitted = match policy {
                        MetadataFreeMethodArgBorrowPolicy::Shared if !expr_has_rust_reference_shape(arg) => {
                            quote! { &#emitted }
                        }
                        MetadataFreeMethodArgBorrowPolicy::Mutable => quote! { &mut #emitted },
                        MetadataFreeMethodArgBorrowPolicy::Shared => emitted,
                    };
                }
                let Some(param) = param else {
                    return Ok(emitted);
                };
                if let Some(wrapped) =
                    self.emit_union_payload_arg(arg, &param.ty, receiver_union_qualifier.as_deref())?
                {
                    return Ok(arg_plan.apply_after_value_plan(wrapped));
                }
                Ok(arg_plan.apply_after_value_plan(emitted))
            })
            .collect()
    }

    /// Return the dependency qualifier for method arguments when the receiver is produced by a public dependency call.
    fn pub_library_union_qualifier_for_method_receiver(receiver: &TypedExpr) -> Option<Vec<String>> {
        match &receiver.kind {
            IrExprKind::Call {
                canonical_path: Some(path),
                ..
            } => Self::pub_library_union_qualifier(Some(path)),
            IrExprKind::InteropCoerce { expr, .. } => Self::pub_library_union_qualifier_for_method_receiver(expr),
            _ => None,
        }
    }

    /// Return the explicitly registered compatibility borrow policy for a metadata-free external method argument.
    ///
    /// Signature metadata remains the source of truth for Rust-boundary borrowing. These policies are only for
    /// default-build interop surfaces emitted without rust-inspect metadata.
    fn metadata_free_method_arg_borrow_policy(
        receiver: &TypedExpr,
        method: &str,
        arg_ty: &IrType,
    ) -> Option<MetadataFreeMethodArgBorrowPolicy> {
        METADATA_FREE_METHOD_BORROW_RULES.iter().find_map(|rule| {
            if !rule.methods.contains(&method) {
                return None;
            }
            if !Self::metadata_free_receiver_matches(receiver, rule.receiver) {
                return None;
            }
            if !Self::metadata_free_arg_matches(arg_ty, rule.arg) {
                return None;
            }
            Some(rule.policy)
        })
    }

    /// Return whether a receiver type matches without relying on metadata.
    fn metadata_free_receiver_matches(receiver: &TypedExpr, class: MetadataFreeReceiverClass) -> bool {
        match class {
            MetadataFreeReceiverClass::IoValue => Self::receiver_allows_io_method_fallback(receiver),
            MetadataFreeReceiverClass::EncodingInstance => {
                Self::receiver_type_matches_any(receiver, &["Encoding", "encoding_rs::Encoding"])
            }
            MetadataFreeReceiverClass::ExternalAssociated => Self::is_external_associated_receiver(receiver),
        }
    }

    /// Return whether an argument type matches without relying on metadata.
    fn metadata_free_arg_matches(arg_ty: &IrType, class: MetadataFreeArgClass) -> bool {
        match class {
            MetadataFreeArgClass::StringBuffer => is_string_buffer_type(arg_ty),
            MetadataFreeArgClass::ByteBuffer => is_byte_buffer_type(arg_ty),
            MetadataFreeArgClass::Any => true,
        }
    }

    /// Return whether a metadata-free receiver is eligible for std::io-style compatibility borrowing.
    fn receiver_allows_io_method_fallback(receiver: &TypedExpr) -> bool {
        !Self::expr_is_type_like(receiver)
    }

    /// Return whether an external Rust trait-style associated call needs `&mut` for its first argument.
    fn external_trait_first_arg_needs_mut_borrow(receiver: &TypedExpr, method: &str) -> bool {
        if !matches!(method, "update" | "finalize_xof_reset") {
            return false;
        }
        matches!(
            &receiver.kind,
            IrExprKind::Var {
                name,
                ref_kind: VarRefKind::ExternalRustName,
                ..
            } if matches!(name.as_str(), "Digest" | "Update" | "ExtendableOutputReset")
        )
    }

    /// Return whether a metadata-free method receiver is an external Rust associated-call target.
    fn is_external_associated_receiver(receiver: &TypedExpr) -> bool {
        matches!(
            &receiver.kind,
            IrExprKind::Var {
                ref_kind: VarRefKind::ExternalRustName,
                ..
            }
        ) && Self::expr_is_type_like(receiver)
    }

    /// Return whether the receiver's nominal type name matches one of the expected Rust compatibility surfaces.
    fn receiver_type_matches_any(receiver: &TypedExpr, expected: &[&str]) -> bool {
        Self::receiver_type_for_method_dispatch(&receiver.ty)
            .nominal_type_name()
            .is_some_and(|name| {
                let short_name = name.rsplit("::").next().unwrap_or(name);
                expected.iter().any(|expected_name| {
                    name == *expected_name || short_name == expected_name.rsplit("::").next().unwrap_or(expected_name)
                })
            })
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
    fn materialize_storage_rooted_args<'site>(
        &self,
        args: &[IrCallArg],
        callable_signature: Option<&'site FunctionSignature>,
        base_use_site: ValueUseSite<'site>,
    ) -> Result<(Vec<TokenStream>, Vec<IrCallArg>), EmitError> {
        let mut bindings = Vec::with_capacity(args.len());
        let mut rewritten = Vec::with_capacity(args.len());
        for (idx, arg) in args.iter().enumerate() {
            let name = format!("__incan_static_arg_{idx}");
            let ident = format_ident!("{}", name);
            let param = Self::signature_param_for_original_call_arg(args, idx, callable_signature);
            let materialize_site = Self::storage_arg_materialization_use_site(base_use_site, param);
            let emitted = self.emit_expr_for_use(&arg.expr, materialize_site)?;
            let mutable =
                param.is_some_and(|param| matches!(param.mutability, super::super::super::types::Mutability::Mutable));
            let binding = if mutable {
                quote! { let mut #ident = #emitted; }
            } else {
                quote! { let #ident = #emitted; }
            };
            bindings.push(binding);
            let rewritten_ty = param
                .map(|param| param.ty.clone())
                .unwrap_or_else(|| arg.expr.ty.clone());
            let rewritten_expr = TypedExpr::new(
                IrExprKind::Var {
                    name,
                    access: VarAccess::Move,
                    ref_kind: VarRefKind::Value,
                },
                rewritten_ty,
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

    /// Combine pre-lock argument materialization with the storage access expression as one Rust expression block.
    fn storage_rooted_method_expr(arg_bindings: Vec<TokenStream>, wrapped: TokenStream) -> TokenStream {
        quote! {{
            #(#arg_bindings)*
            #wrapped
        }}
    }

    /// Return the callable parameter matched by one original call argument before storage-lock materialization.
    fn signature_param_for_original_call_arg<'sig>(
        args: &[IrCallArg],
        idx: usize,
        callable_signature: Option<&'sig FunctionSignature>,
    ) -> Option<&'sig FunctionParam> {
        let signature = callable_signature?;
        let arg = args.get(idx)?;
        if matches!(arg.kind, IrCallArgKind::PositionalUnpack | IrCallArgKind::KeywordUnpack) {
            return None;
        }
        if let Some(name) = arg.name.as_deref() {
            return signature.params.iter().find(|param| param.name == name);
        }
        let positional_idx = args
            .iter()
            .take(idx)
            .filter(|arg| arg.name.is_none() && matches!(arg.kind, IrCallArgKind::Positional))
            .count();
        signature.params.get(positional_idx)
    }

    /// Pick the use-site plan used when evaluating one storage-rooted method argument before taking the storage lock.
    fn storage_arg_materialization_use_site<'site>(
        base_use_site: ValueUseSite<'site>,
        param: Option<&'site FunctionParam>,
    ) -> ValueUseSite<'site> {
        match (base_use_site, param) {
            (ValueUseSite::IncanCallArg { in_return, .. }, Some(param)) => ValueUseSite::IncanCallArg {
                target_ty: Some(&param.ty),
                callee_param: Some(param),
                in_return,
            },
            (ValueUseSite::ExternalCallArg { .. }, Some(param)) | (ValueUseSite::MethodArg, Some(param)) => {
                ValueUseSite::ExternalCallArg {
                    target_ty: Some(&param.ty),
                }
            }
            (site, _) => site,
        }
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
                let short_name = name.rsplit("::").next().unwrap_or(name);
                self.struct_field_names.contains_key(name)
                    || self.struct_field_names.contains_key(short_name)
                    || self.rusttype_alias_names.contains(name)
                    || self.rusttype_alias_names.contains(short_name)
                    || self.type_module_paths.contains_key(name)
                    || self.type_module_paths.contains_key(short_name)
            }
            IrType::Enum(name) => {
                let short_name = name.rsplit("::").next().unwrap_or(name);
                self.enum_variant_fields
                    .keys()
                    .any(|(enum_name, _)| enum_name == name || enum_name == short_name)
            }
            IrType::Trait(_) => true,
            _ => false,
        }
    }

    /// Return whether a receiver is a zero-cost `rusttype` alias over an external Rust type.
    fn is_rusttype_alias_receiver(&self, receiver_ty: &IrType) -> bool {
        match Self::receiver_type_for_method_dispatch(receiver_ty) {
            IrType::Struct(name) | IrType::NamedGeneric(name, _) => {
                let short_name = name.rsplit("::").next().unwrap_or(name);
                self.rusttype_alias_names.contains(name) || self.rusttype_alias_names.contains(short_name)
            }
            _ => false,
        }
    }

    /// Recover a field receiver's declared surface type before choosing method-call ownership policy.
    fn receiver_with_known_field_type(&self, receiver: &TypedExpr) -> Option<TypedExpr> {
        let IrExprKind::Field { object, field } = &receiver.kind else {
            return None;
        };
        let owner = Self::receiver_type_for_method_dispatch(&object.ty).nominal_type_name()?;
        let field_ty = self
            .struct_field_types
            .get(&(owner.to_string(), field.clone()))
            .cloned()
            .or_else(|| {
                owner.rsplit("::").next().and_then(|short| {
                    self.struct_field_types
                        .get(&(short.to_string(), field.clone()))
                        .cloned()
                })
            })?;
        let mut receiver = receiver.clone();
        receiver.ty = field_ty;
        Some(receiver)
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
            let (arg_bindings, rewritten_args) =
                self.materialize_storage_rooted_args(args, None, ValueUseSite::MethodArg)?;
            if matches!(kind, MethodKind::Collection(CollectionMethodKind::Get)) {
                let rewritten_receiver = Self::rewrite_storage_root_expr(receiver, "__incan_static_value");
                let arg_exprs: Vec<TypedExpr> = rewritten_args.iter().map(|a| a.expr.clone()).collect();
                let inner = self.emit_static_collection_get(&rewritten_receiver, &arg_exprs)?;
                let wrapped = self.emit_storage_with_ref(receiver, inner)?;
                return Ok(Self::storage_rooted_method_expr(arg_bindings, wrapped));
            }

            let use_mut = super::method_kind_uses_mutable_receiver(kind);
            let rewritten_receiver = if use_mut {
                Self::rewrite_storage_root_expr_for_mut(receiver, "__incan_static_value")
            } else {
                Self::rewrite_storage_root_expr(receiver, "__incan_static_value")
            };
            let inner = self.emit_known_method_call(&rewritten_receiver, kind, &rewritten_args)?;
            let wrapped = if use_mut {
                self.emit_storage_with_mut(receiver, inner)
            } else {
                self.emit_storage_with_ref(receiver, inner)
            }?;
            return Ok(Self::storage_rooted_method_expr(arg_bindings, wrapped));
        }

        let r0 = self.emit_expr(receiver)?;
        let info = ReceiverInfo::new(&receiver.ty, r0);
        let arg_exprs: Vec<TypedExpr> = args.iter().map(|a| a.expr.clone()).collect();
        match kind {
            MethodKind::String(kind) => emit_string_method(self, &info, kind, &arg_exprs),
            MethodKind::Collection(kind) => emit_collection_method(self, receiver, &info, kind, &arg_exprs),
            MethodKind::Iterator(kind) => emit_iterator_method(self, receiver, &info, kind, &arg_exprs),
            MethodKind::Result(ResultMethodId::Unwrap) => {
                if !arg_exprs.is_empty() {
                    return Err(EmitError::Unsupported("Result.unwrap expects no arguments".to_string()));
                }
                let receiver_tokens = &info.r;
                Ok(quote! {
                    match #receiver_tokens {
                        Ok(__incan_ok) => __incan_ok,
                        Err(_) => panic!("called Result.unwrap() on an Err value"),
                    }
                })
            }
            MethodKind::Result(ResultMethodId::UnwrapOr) => {
                let Some(default) = arg_exprs.first() else {
                    return Err(EmitError::Unsupported(
                        "Result.unwrap_or expects one default argument".to_string(),
                    ));
                };
                let default_tokens = self.emit_expr(default)?;
                let receiver_tokens = &info.r;
                Ok(quote! { #receiver_tokens.unwrap_or(#default_tokens) })
            }
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
        self.emit_method_call_expr_with_result_use(
            receiver,
            method,
            dispatch,
            type_args,
            args,
            callable_signature,
            arg_policy,
            None,
        )
    }

    /// Emit a method call while preserving the surrounding value-use target for argument shaping.
    #[allow(clippy::too_many_arguments)]
    pub(in super::super) fn emit_method_call_expr_for_use(
        &self,
        receiver: &TypedExpr,
        method: &str,
        dispatch: Option<&IrMethodDispatch>,
        type_args: &[IrType],
        args: &[IrCallArg],
        callable_signature: Option<&FunctionSignature>,
        arg_policy: MethodCallArgPolicy,
        result_use_site: ValueUseSite<'_>,
    ) -> Result<TokenStream, EmitError> {
        let emitted = self.emit_method_call_expr_with_result_use(
            receiver,
            method,
            dispatch,
            type_args,
            args,
            callable_signature,
            arg_policy,
            Some(result_use_site),
        )?;
        if magic_methods::from_str(method) == Some(magic_methods::MagicMethodId::ClassName)
            && matches!(Self::use_site_target_ty(result_use_site), Some(IrType::String))
        {
            return Ok(quote! { (#emitted).to_string() });
        }
        Ok(emitted)
    }

    /// Shared method-call emitter used by plain and target-aware method emission.
    #[allow(clippy::too_many_arguments)]
    fn emit_method_call_expr_with_result_use(
        &self,
        receiver: &TypedExpr,
        method: &str,
        dispatch: Option<&IrMethodDispatch>,
        type_args: &[IrType],
        args: &[IrCallArg],
        callable_signature: Option<&FunctionSignature>,
        arg_policy: MethodCallArgPolicy,
        result_use_site: Option<ValueUseSite<'_>>,
    ) -> Result<TokenStream, EmitError> {
        if Self::expr_is_storage_rooted(receiver) {
            let use_mut = !matches!(arg_policy, MethodCallArgPolicy::PreserveShape);
            let rewritten_receiver = if use_mut {
                Self::rewrite_storage_root_expr_for_mut(receiver, "__incan_static_value")
            } else {
                Self::rewrite_storage_root_expr(receiver, "__incan_static_value")
            };
            let in_return = *self.in_return_context.borrow();
            let receiver_ref_kind = match &rewritten_receiver.kind {
                IrExprKind::Var { ref_kind, .. } => Some(*ref_kind),
                _ => None,
            };
            let has_incan_method_signature = self
                .method_signature_for_receiver(&rewritten_receiver.ty, method)
                .is_some();
            let preserve_lookup_arg_shape = matches!(arg_policy, MethodCallArgPolicy::PreserveShape)
                || rust_collection_family_for_ir_type(&rewritten_receiver.ty)
                    .is_some_and(|family| family.preserves_lookup_arg_shape(method));
            let rusttype_alias_receiver = self.is_rusttype_alias_receiver(&rewritten_receiver.ty);
            let base_use_site = regular_method_argument_use_site(
                RegularMethodArgumentContext {
                    arg_policy,
                    receiver_ref_kind,
                    has_incan_method_signature,
                    is_incan_owned_nominal_receiver: self.is_incan_owned_nominal_receiver(&rewritten_receiver.ty),
                    is_rusttype_alias_receiver: rusttype_alias_receiver,
                    preserves_lookup_arg_shape: preserve_lookup_arg_shape,
                    in_return,
                },
                None,
            );
            let (arg_bindings, rewritten_args) =
                self.materialize_storage_rooted_args(args, callable_signature, base_use_site)?;
            let inner = self.emit_method_call_expr_with_result_use(
                &rewritten_receiver,
                method,
                dispatch,
                type_args,
                &rewritten_args,
                callable_signature,
                arg_policy,
                result_use_site,
            )?;
            let wrapped = if use_mut {
                self.emit_storage_with_mut(receiver, inner)
            } else {
                self.emit_storage_with_ref(receiver, inner)
            }?;
            return Ok(Self::storage_rooted_method_expr(arg_bindings, wrapped));
        }

        let inferred_receiver = self.receiver_with_known_field_type(receiver);
        let receiver = inferred_receiver.as_ref().unwrap_or(receiver);
        let r0 = self.emit_expr(receiver)?;
        let info = ReceiverInfo::new(&receiver.ty, r0);
        let r = &info.r;
        if let Some(call) = emit_registered_method_fast_path(self, receiver, method, args, r)? {
            return Ok(call);
        }
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
        let result_target_ty = result_use_site.and_then(Self::use_site_target_ty);

        // Check if this is an enum variant construction.
        //
        // Important: do NOT treat any uppercase variable as a type name. Only rewrite when we actually know this
        // (Type, Variant) pair exists in the enum variant registry.
        if let IrExprKind::Var { name, .. } = &receiver.kind {
            let key = (name.to_string(), method.to_string());
            let canonical_method = self
                .enum_variant_aliases
                .get(&key)
                .map(String::as_str)
                .unwrap_or(method);
            let canonical_key = (name.to_string(), canonical_method.to_string());
            if self.enum_variant_fields.contains_key(&canonical_key) {
                return self.emit_enum_variant_call(name, canonical_method, &arg_exprs);
            }
        }

        if let IrExprKind::Var {
            name,
            ref_kind: VarRefKind::TypeName,
            ..
        } = &receiver.kind
            && args.is_empty()
            && type_args.is_empty()
            && let Some(trait_path) = type_reflection_trait_path(method)
        {
            let receiver_ty = match &receiver.ty {
                IrType::Unknown => IrType::Struct(name.clone()),
                ty => ty.clone(),
            };
            let receiver_tokens = self.emit_type(&receiver_ty);
            let path_tokens: Vec<TokenStream> = trait_path
                .split("::")
                .map(|segment| {
                    let ident = Self::rust_ident(segment);
                    quote! { #ident }
                })
                .collect();
            let trait_tokens = super::super::decls::join_path_tokens(&path_tokens);
            let m = Self::rust_ident(method);
            return Ok(quote! { <#receiver_tokens as #trait_tokens>::#m() });
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
                    _ if !type_args.is_empty() => quote! { #type_ident #method_turbofish },
                    _ => quote! { #type_ident },
                };
                let m = Self::rust_ident(method);
                let in_return = *self.in_return_context.borrow();
                let receiver_ref_kind = match &receiver.kind {
                    IrExprKind::Var { ref_kind, .. } => Some(*ref_kind),
                    _ => None,
                };
                let use_site = associated_function_argument_use_site(
                    AssociatedFunctionArgumentContext {
                        receiver_ref_kind,
                        is_incan_owned_nominal_receiver: self.is_incan_owned_nominal_receiver(&receiver.ty),
                        in_return,
                    },
                    None,
                );
                let arg_tokens = self.emit_method_call_args(
                    method,
                    receiver,
                    args,
                    MethodCallArgEmission {
                        callable_signature,
                        base_use_site: use_site,
                        result_target_ty,
                        infer_unresolved_generic_args: type_args.is_empty(),
                    },
                )?;
                return Ok(quote! { #type_path::#m (#(#arg_tokens),*) });
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
            let in_return = *self.in_return_context.borrow();
            let use_site = if callable_signature.is_some() || self.is_incan_owned_nominal_receiver(&receiver.ty) {
                ValueUseSite::IncanCallArg {
                    target_ty: None,
                    callee_param: None,
                    in_return,
                }
            } else {
                ValueUseSite::MethodArg
            };
            let arg_tokens = self.emit_method_call_args(
                method,
                receiver,
                args,
                MethodCallArgEmission {
                    callable_signature,
                    base_use_site: use_site,
                    result_target_ty,
                    infer_unresolved_generic_args: false,
                },
            )?;
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
        let has_incan_method_signature = self.method_signature_for_receiver(&receiver.ty, method).is_some();
        let preserve_lookup_arg_shape = matches!(arg_policy, MethodCallArgPolicy::PreserveShape)
            || rust_collection_family_for_ir_type(&receiver.ty)
                .is_some_and(|family| family.preserves_lookup_arg_shape(method));
        let rusttype_alias_receiver = self.is_rusttype_alias_receiver(&receiver.ty);
        let use_site = regular_method_argument_use_site(
            RegularMethodArgumentContext {
                arg_policy,
                receiver_ref_kind,
                has_incan_method_signature,
                is_incan_owned_nominal_receiver: self.is_incan_owned_nominal_receiver(&receiver.ty),
                is_rusttype_alias_receiver: rusttype_alias_receiver,
                preserves_lookup_arg_shape: preserve_lookup_arg_shape,
                in_return,
            },
            None,
        );
        let arg_tokens = self.emit_method_call_args(
            method,
            receiver,
            args,
            MethodCallArgEmission {
                callable_signature,
                base_use_site: use_site,
                result_target_ty,
                infer_unresolved_generic_args: type_args.is_empty(),
            },
        )?;
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
