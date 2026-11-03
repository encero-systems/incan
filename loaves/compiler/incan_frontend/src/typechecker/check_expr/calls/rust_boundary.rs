//! Rust boundary matching, Rust call validation, and coercion metadata recording.

use std::collections::HashMap;

use super::TypeChecker;
use crate::ast::{CallArg, Expr, ParamKind, Span, Spanned};
use crate::diagnostics::errors;
use crate::resolved_type_subst::{map_resolved_type_leaves, substitute_resolved_type};
use crate::symbols::{CallableParam, ResolvedType, TypeInfo};
use crate::typechecker::helpers::{collection_type_id, render_resolved_type_as_rust_arg};
use crate::typechecker::{RustArgCoercionInfo, RustArgCoercionKind};
use incan_lang::interop::{CoercionPolicy, RustFunctionSig, RustItemKind, RustParam, admitted_builtin_coercion};
use incan_lang::lang::types::collections::CollectionTypeId;
use incan_lang::lang::types::numerics;
use quote::ToTokens;

mod boundary_matching;
mod call_binding;
mod native_types;
mod signature_inference;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RustArgBoundaryMatch {
    /// Argument already matches the Rust parameter shape; no lowering-time adapter is required.
    Exact,
    /// Argument is admissible if lowering inserts a boundary coercion/adapter.
    Coercion(RustArgCoercionKind),
    /// Argument cannot satisfy the Rust parameter shape.
    NoMatch,
}

/// A written argument paired with its inspected parameter and declaration-order slot before specialization.
struct RustCallArgBinding<'a> {
    arg: &'a CallArg,
    arg_ty: &'a ResolvedType,
    param: &'a RustParam,
    parameter_index: usize,
}

impl TypeChecker {
    /// Validate a Rust method call, letting the type its result is expected to have bind method binders the
    /// arguments leave open (`parse::<F>()` reads `F` from `Result<i64, _>`). Literal aggregates are checked against
    /// the specialized parameter before their concrete storage is retained for Body IR. Unsafe methods and associated
    /// constructors receive the same RFC 005 refusal as free functions, before any executable identity is retained.
    pub(in crate::typechecker) fn validate_rust_method_call_with_expected(
        &mut self,
        callable_display: &str,
        sig: &RustFunctionSig,
        args: &[CallArg],
        arg_types: &[ResolvedType],
        preserves_lookup_arg_shape: bool,
        span: Span,
        expected_return_ty: Option<&ResolvedType>,
    ) -> ResolvedType {
        if sig.is_unsafe {
            let path = callable_display.strip_prefix("rust::").unwrap_or(callable_display);
            self.errors.push(errors::rust_unsafe_call_not_supported(path, span));
            return ResolvedType::Unknown;
        }
        let owner_path = Self::rust_method_owner_path(callable_display);
        let contextual_types =
            self.contextualize_rust_bound_closures(sig, owner_path, args, arg_types, expected_return_ty);
        let arg_types = contextual_types.as_slice();
        if let Some(contract) = sig.receiver_contract {
            self.type_info.rust.receiver_contracts.insert(span, contract);
        }
        if sig.is_async {
            self.type_info.rust.async_call_realizations.insert(span);
        }
        if sig.is_async && !self.is_in_await_operand(span) {
            self.errors
                .push(errors::async_call_without_await(callable_display, span));
        }
        let callable_returns =
            self.infer_rust_signature_bindings(sig, owner_path, &sig.type_params, args, arg_types, expected_return_ty);
        self.validate_rust_scalar_predicates(sig, &callable_returns, span);
        self.record_inferred_rust_arguments(callable_display, &sig.type_params, &callable_returns, span);
        let declared_sig = sig;
        let declared_params = if Self::rust_signature_has_receiver(sig) {
            &sig.params[1..]
        } else {
            &sig.params
        };
        let effective_sig = self.specialize_inferred_rust_signature(sig, owner_path, &callable_returns);
        let sig = &effective_sig;
        let params = if Self::rust_signature_has_receiver(sig) {
            &sig.params[1..]
        } else {
            &sig.params
        };
        self.record_rust_call_site_params(span, params, owner_path);

        let binding_errors_before = self.errors.len();
        let bindings = self.bind_rust_call_args(callable_display, params, args, arg_types, span);
        if self.errors.len() != binding_errors_before {
            return self.resolved_rust_call_type_from_sig(sig, owner_path, span);
        }

        for binding in bindings {
            let arg_expr = Self::call_arg_expr(binding.arg);
            let param = binding.param;
            if let Some(declared) = declared_params.get(binding.parameter_index)
                && self.validate_rust_bound_callable_argument(
                    declared_sig,
                    &declared.type_display,
                    &callable_returns,
                    arg_expr,
                    binding.arg_ty,
                )
            {
                continue;
            }
            let target_ty = self.resolved_rust_boundary_target_from_param_display_for_owner_path(
                param.type_display.as_str(),
                owner_path,
            );
            let contextual_arg_ty = if (matches!(arg_expr.node, Expr::Closure(_, _))
                && matches!(&target_ty, ResolvedType::Function(_, _)))
                || super::super::ops::is_unsuffixed_integer_literal(arg_expr)
                || matches!(
                    arg_expr.node,
                    Expr::Literal(_)
                        | Expr::Unary(_, _)
                        | Expr::List(_)
                        | Expr::Dict(_)
                        | Expr::Set(_)
                        | Expr::Tuple(_)
                )
                || (matches!(arg_expr.node, Expr::Call(..) | Expr::MethodCall(..))
                    && Self::rust_call_arg_wants_owner_expectation(binding.arg_ty, &target_ty))
            {
                Some(self.check_expr_with_expected(arg_expr, Some(&target_ty)))
            } else {
                None
            };
            let arg_ty = contextual_arg_ty.as_ref().unwrap_or(binding.arg_ty);
            if preserves_lookup_arg_shape && self.rust_lookup_probe_boundary_match(arg_ty, &target_ty) {
                continue;
            }
            let instantiated = declared_params.get(binding.parameter_index).map(|declared| {
                self.instantiate_rust_signature_type(
                    &self.native_rust_signature_type(&declared.type_display, owner_path),
                    &declared_sig.type_params,
                    &callable_returns,
                )
            });
            self.validate_rust_boundary_value_with_target(
                owner_path,
                param.type_display.as_str(),
                arg_expr,
                arg_ty,
                false,
                instantiated.as_ref(),
            );
        }

        let ret = self.resolved_rust_call_type_from_sig(sig, owner_path, span);
        self.record_rust_return_coercion_from_display(sig.return_type.as_str(), &ret, span);
        ret
    }
}

#[cfg(test)]
mod validate_rust_function_call_tests;
