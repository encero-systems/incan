//! Unary operators, index, length, containment and call dunders, truthiness, and iteration protocols.

use super::*;

impl TypeChecker {
    /// Type-check a unary operation and return its result type.
    pub(in crate::typechecker::check_expr) fn check_unary(
        &mut self,
        op: UnaryOp,
        operand: &Spanned<Expr>,
        span: Span,
    ) -> ResolvedType {
        self.check_unary_with_expected(op, operand, span, None)
    }

    /// Type-check a unary operation with an optional contextual result type for overload disambiguation.
    pub(in crate::typechecker::check_expr) fn check_unary_with_expected(
        &mut self,
        op: UnaryOp,
        operand: &Spanned<Expr>,
        span: Span,
        expected_return_ty: Option<&ResolvedType>,
    ) -> ResolvedType {
        if matches!(op, UnaryOp::Neg)
            && let Expr::Literal(Literal::Int(value)) = &operand.node
            && let Some(suffix) = value.suffix
        {
            let operand_ty = self.check_suffixed_int_literal(value, true, operand.span);
            self.record_expr_type(operand.span, operand_ty.clone());
            if numerics::info_for(suffix).family == NumericFamily::UnsignedInteger {
                self.errors
                    .push(errors::type_mismatch("signed numeric", &operand_ty.to_string(), span));
                return ResolvedType::Unknown;
            }
            return operand_ty;
        }
        let operand_ty = self.check_expr(operand);
        match op {
            UnaryOp::Neg => {
                let exact_family = match &operand_ty {
                    ResolvedType::Numeric(id) => Some(numerics::info_for(*id).family),
                    _ => None,
                };
                if matches!(
                    exact_family,
                    Some(NumericFamily::SignedInteger | NumericFamily::BinaryFloat)
                ) {
                    // A negated exact-width value keeps its type, as same-type arithmetic does.
                    operand_ty
                } else if exact_family == Some(NumericFamily::UnsignedInteger) {
                    self.errors
                        .push(errors::type_mismatch("signed numeric", &operand_ty.to_string(), span));
                    ResolvedType::Unknown
                } else if self.types_compatible(&operand_ty, &ResolvedType::Int) {
                    ResolvedType::Int
                } else if self.types_compatible(&operand_ty, &ResolvedType::Float) {
                    ResolvedType::Float
                } else {
                    let method = "__neg__";
                    if let Some(ret) =
                        self.resolve_operator_dunder(&operand_ty, method, &[], &[], span, expected_return_ty)
                    {
                        self.type_info
                            .record_resolved_operator_call(span, method, ResolvedOperatorKind::Unary);
                        return ret;
                    }
                    if self.is_user_operator_receiver(&operand_ty) {
                        self.errors
                            .push(errors::missing_method(&operand_ty.to_string(), method, span));
                        return ResolvedType::Unknown;
                    }
                    self.errors
                        .push(errors::type_mismatch("numeric", &operand_ty.to_string(), span));
                    ResolvedType::Unknown
                }
            }
            UnaryOp::Not => {
                if !self.types_compatible(&operand_ty, &ResolvedType::Bool) {
                    self.errors
                        .push(errors::type_mismatch("bool", &operand_ty.to_string(), span));
                }
                ResolvedType::Bool
            }
            UnaryOp::Invert => {
                // RFC 009: `~` keeps its integer operand's type, as the binary bitwise operators do.
                if matches!(numeric_ty_from_resolved(&operand_ty), Some(NumericTy::Int)) {
                    operand_ty
                } else if self.types_compatible(&operand_ty, &ResolvedType::Int) {
                    ResolvedType::Int
                } else {
                    let method = "__invert__";
                    if let Some(ret) =
                        self.resolve_operator_dunder(&operand_ty, method, &[], &[], span, expected_return_ty)
                    {
                        self.type_info
                            .record_resolved_operator_call(span, method, ResolvedOperatorKind::Unary);
                        return ret;
                    }
                    if self.is_user_operator_receiver(&operand_ty) {
                        self.errors
                            .push(errors::missing_method(&operand_ty.to_string(), method, span));
                        return ResolvedType::Unknown;
                    }
                    self.errors
                        .push(errors::type_mismatch("int", &operand_ty.to_string(), span));
                    ResolvedType::Unknown
                }
            }
        }
    }

    /// Resolve a user-defined indexing operation (`base[index]`) through `__getitem__`, if available.
    pub(in crate::typechecker) fn resolve_index_dunder(
        &mut self,
        base_ty: &ResolvedType,
        index: &Spanned<Expr>,
        index_ty: &ResolvedType,
        span: Span,
    ) -> Option<ResolvedType> {
        let method = "__getitem__";
        let args = vec![CallArg::Positional(index.clone())];
        let arg_types = vec![index_ty.clone()];
        let ret = self.resolve_operator_dunder(base_ty, method, &args, &arg_types, span, None)?;
        self.type_info
            .record_resolved_operator_call(span, method, ResolvedOperatorKind::Index);
        Some(ret)
    }

    /// Validate a boolean control-flow condition using RFC 068 structural `__bool__` when needed. Preserve its
    /// receiver's original value identity separately for Body IR; legacy keeps the synthetic method's reference.
    pub(in crate::typechecker) fn validate_truthiness_condition(&mut self, cond_ty: &ResolvedType, span: Span) {
        let key = span;
        self.type_info.protocols.body_ir_truthiness_receivers.remove(&key);
        if self.types_compatible(cond_ty, &ResolvedType::Bool) || matches!(cond_ty, ResolvedType::Unknown) {
            return;
        }
        if cond_ty.is_option() || cond_ty.is_result() {
            self.errors
                .push(errors::type_mismatch("bool", &cond_ty.to_string(), span));
            return;
        }

        if self.is_user_operator_receiver(cond_ty) {
            let receiver_identity = self.type_info.resolved_identity(span).cloned();
            let method = "__bool__";
            let args: Vec<CallArg> = Vec::new();
            let arg_types: Vec<ResolvedType> = Vec::new();
            match self.resolve_operator_dunder(cond_ty, method, &args, &arg_types, span, Some(&ResolvedType::Bool)) {
                Some(ret) => {
                    if !self.types_compatible(&ret, &ResolvedType::Bool) {
                        self.errors.push(errors::type_mismatch("bool", &ret.to_string(), span));
                        return;
                    }
                    self.type_info
                        .record_resolved_operator_call(span, method, ResolvedOperatorKind::Truthiness);
                    if let Some(identity) = receiver_identity {
                        self.type_info
                            .protocols
                            .body_ir_truthiness_receivers
                            .insert(key, identity);
                    }
                }
                None => self
                    .errors
                    .push(errors::missing_method(&cond_ty.to_string(), method, span)),
            }
            return;
        }

        self.errors
            .push(errors::type_mismatch("bool", &cond_ty.to_string(), span));
    }

    /// Resolve `len(x)` for a user-defined receiver through `__len__(self) -> int`.
    pub(in crate::typechecker) fn resolve_len_dunder(
        &mut self,
        receiver_ty: &ResolvedType,
        span: Span,
    ) -> Option<ResolvedType> {
        let method = "__len__";
        let args: Vec<CallArg> = Vec::new();
        let arg_types: Vec<ResolvedType> = Vec::new();
        let ret = self.resolve_operator_dunder(receiver_ty, method, &args, &arg_types, span, Some(&ResolvedType::Int));
        match ret {
            Some(ret) => {
                if !self.types_compatible(&ret, &ResolvedType::Int) {
                    self.errors.push(errors::type_mismatch("int", &ret.to_string(), span));
                    return Some(ResolvedType::Unknown);
                }
                self.type_info
                    .record_resolved_operator_call(span, method, ResolvedOperatorKind::Len);
                Some(ResolvedType::Int)
            }
            None => {
                self.errors
                    .push(errors::missing_method(&receiver_ty.to_string(), method, span));
                None
            }
        }
    }

    /// Resolve `item in receiver` for a user-defined receiver through `__contains__(self, item) -> bool`.
    pub(in crate::typechecker) fn resolve_contains_dunder(
        &mut self,
        receiver_ty: &ResolvedType,
        item: &Spanned<Expr>,
        item_ty: &ResolvedType,
        span: Span,
    ) -> Option<ResolvedType> {
        let method = "__contains__";
        let args = vec![CallArg::Positional(item.clone())];
        let arg_types = vec![item_ty.clone()];
        let ret = self.resolve_operator_dunder(receiver_ty, method, &args, &arg_types, span, Some(&ResolvedType::Bool));
        match ret {
            Some(ret) => {
                if !self.types_compatible(&ret, &ResolvedType::Bool) {
                    self.errors.push(errors::type_mismatch("bool", &ret.to_string(), span));
                    return Some(ResolvedType::Unknown);
                }
                self.type_info
                    .record_resolved_operator_call(span, method, ResolvedOperatorKind::Contains);
                Some(ResolvedType::Bool)
            }
            None => {
                self.errors
                    .push(errors::missing_method(&receiver_ty.to_string(), method, span));
                None
            }
        }
    }

    /// Resolve `receiver(...)` for callable user-defined objects through `__call__`.
    pub(in crate::typechecker) fn resolve_call_dunder(
        &mut self,
        receiver_ty: &ResolvedType,
        args: &[CallArg],
        arg_types: &[ResolvedType],
        span: Span,
    ) -> Option<ResolvedType> {
        let method = "__call__";
        let ret = self.resolve_operator_dunder(receiver_ty, method, args, arg_types, span, None);
        match ret {
            Some(ret) => {
                self.type_info
                    .record_resolved_operator_call(span, method, ResolvedOperatorKind::Call);
                Some(ret)
            }
            None => {
                self.errors
                    .push(errors::missing_method(&receiver_ty.to_string(), method, span));
                None
            }
        }
    }

    /// Resolve custom `for` iteration through `__iter__(self)` and `iterator.__next__() -> Option[T]`.
    pub(in crate::typechecker) fn resolve_iteration_protocol(
        &mut self,
        receiver_ty: &ResolvedType,
        span: Span,
    ) -> Option<ResolvedType> {
        let iter_method = magic_methods::as_str(MagicMethodId::Iter);
        let next_method = magic_methods::as_str(MagicMethodId::Next);
        let hooks = self.resolve_iteration_protocol_hooks(receiver_ty, span, span)?;
        let iterator_ty = hooks.iterator_type;
        let next_ret = hooks.next_type;

        let Some(item_ty) = next_ret.option_inner_type().cloned() else {
            self.errors
                .push(errors::type_mismatch("Option[_]", &next_ret.to_string(), span));
            return Some(ResolvedType::Unknown);
        };

        self.type_info.record_protocol_iteration(
            span,
            ProtocolIterationInfo {
                iter_method: iter_method.to_string(),
                iterator_type: iterator_ty,
                next_method: next_method.to_string(),
                item_type: item_ty.clone(),
                iter_dispatch: hooks.iter_dispatch,
                next_dispatch: hooks.next_dispatch,
                fallible_error_type: None,
            },
        );
        Some(item_ty)
    }

    /// Retain polling for an owned source `Iterator[T]` returned by its checked protocol call. This value already
    /// owns its iteration state, so no nonexistent `__iter__` slot or additional clone is introduced. Synthetic
    /// lookup preserves the written call's facts and retains the actual source trait's mutable `__next__` contract.
    pub(in crate::typechecker) fn retain_owned_source_iterator_protocol(
        &mut self,
        receiver_ty: &ResolvedType,
        span: Span,
    ) -> Option<ResolvedType> {
        let dispatch = self.type_info.resolved_method_call(span)?.dispatch.clone();
        let written_call = self.type_info.take_call_site_facts(span);
        let next_method = magic_methods::as_str(MagicMethodId::Next);
        let returned = self.resolve_protocol_operator_dunder(receiver_ty, next_method, &[], &[], span, Some(&dispatch));
        let selected = returned
            .as_ref()
            .map(|returned| self.body_ir_protocol_call(receiver_ty, next_method, returned, span));
        self.type_info.restore_call_site_facts(span, written_call);
        let Some(returned) = returned else {
            self.errors
                .push(errors::missing_method(&receiver_ty.to_string(), next_method, span));
            return None;
        };
        let item = returned.option_inner_type()?.clone();
        let next = selected.filter(|next| next.canonical.is_some() && next.receiver_is_mutable == Some(true))?;
        self.type_info
            .protocols
            .body_ir_iterations
            .insert(span, BodyIrIterationHooksInfo { iter: None, next });
        Some(item)
    }

    /// Resolve `for item in iterable?` through `__iter__(self)` and a fallible `__next__() -> Result[Option[T], E]`
    /// protocol.
    pub(in crate::typechecker) fn resolve_fallible_iteration_protocol(
        &mut self,
        receiver_ty: &ResolvedType,
        receiver_span: Span,
        protocol_span: Span,
    ) -> Option<ResolvedType> {
        let iter_method = magic_methods::as_str(MagicMethodId::Iter);
        let next_method = magic_methods::as_str(MagicMethodId::Next);
        let hooks = self.resolve_iteration_protocol_hooks(receiver_ty, receiver_span, protocol_span)?;
        let iterator_ty = hooks.iterator_type;
        let next_ret = hooks.next_type;

        let Some(item_ty) = next_ret
            .result_ok_type()
            .and_then(ResolvedType::option_inner_type)
            .cloned()
        else {
            self.errors.push(errors::type_mismatch(
                "Result[Option[_], _]",
                &next_ret.to_string(),
                protocol_span,
            ));
            return Some(ResolvedType::Unknown);
        };
        let Some(error_ty) = next_ret.result_err_type().cloned() else {
            return Some(ResolvedType::Unknown);
        };

        match self.current_return_error_type.clone() {
            Some(expected_error) if !self.types_compatible(&error_ty, &expected_error) => {
                self.errors.push(errors::incompatible_error_type(
                    &expected_error.to_string(),
                    &error_ty.to_string(),
                    protocol_span,
                ));
            }
            None => self.errors.push(errors::try_without_result_return(protocol_span)),
            _ => {}
        }

        self.type_info.record_protocol_iteration(
            protocol_span,
            ProtocolIterationInfo {
                iter_method: iter_method.to_string(),
                iterator_type: iterator_ty,
                next_method: next_method.to_string(),
                item_type: item_ty.clone(),
                iter_dispatch: hooks.iter_dispatch,
                next_dispatch: hooks.next_dispatch,
                fallible_error_type: Some(error_ty),
            },
        );
        Some(item_ty)
    }

    /// Probe the fallible iteration shape while rolling back diagnostics and semantic artifacts.
    pub(in crate::typechecker) fn probe_fallible_iteration_protocol(
        &mut self,
        receiver_ty: &ResolvedType,
        span: Span,
    ) -> bool {
        if !self.is_user_operator_receiver(receiver_ty) {
            return false;
        }

        let baseline_errors = self.errors.clone();
        let baseline_warnings = self.warnings.clone();
        let baseline_type_info = self.type_info.clone();
        let baseline_consumed_iterator_bindings = self.consumed_iterator_bindings.clone();
        let result = self
            .resolve_iteration_protocol_hooks(receiver_ty, span, span)
            .is_some_and(|hooks| {
                hooks
                    .next_type
                    .result_ok_type()
                    .and_then(ResolvedType::option_inner_type)
                    .is_some()
                    && hooks.next_type.result_err_type().is_some()
            });
        self.errors = baseline_errors;
        self.warnings = baseline_warnings;
        self.type_info = baseline_type_info;
        self.consumed_iterator_bindings = baseline_consumed_iterator_bindings;
        result
    }

    /// Resolve the shared `__iter__` and `__next__` hook pair for one custom iteration route.
    ///
    /// The hooks are resolved as calls at `protocol_span`, and their dispatches reach lowering through the protocol's
    /// own record. For `for item in source.items():` that span is also the span of the call `source.items()`, whose
    /// facts it holds: the hooks' facts replaced them, so lowering built `items` with the declaration, parameters and
    /// dispatch of `__next__`, and a `__next__` dispatched through `Iterator[T]` named `Iterator::<T>::items` (E0782,
    /// #1561). The facts written at `protocol_span` are set aside while the hooks resolve and put back after.
    pub(super) fn resolve_iteration_protocol_hooks(
        &mut self,
        receiver_ty: &ResolvedType,
        receiver_span: Span,
        protocol_span: Span,
    ) -> Option<ResolvedIterationHooks> {
        let receiver_dispatch = self
            .type_info
            .resolved_method_call(receiver_span)
            .map(|call| call.dispatch.clone());
        let written_call = self.type_info.take_call_site_facts(protocol_span);
        let hooks = self.resolve_iteration_protocol_hook_calls(receiver_ty, receiver_dispatch, protocol_span);
        self.type_info.restore_call_site_facts(protocol_span, written_call);
        if let Some(hooks) = &hooks {
            self.type_info
                .protocols
                .body_ir_iterations
                .insert(protocol_span, hooks.body_ir.clone());
        }
        hooks
    }

    /// Capture the selected hook before the next probe replaces its span facts; no legacy artifact is changed here.
    pub(super) fn body_ir_protocol_call(
        &self,
        receiver: &ResolvedType,
        method: &str,
        returned: &ResolvedType,
        span: Span,
    ) -> BodyIrProtocolCallInfo {
        // ---- Selected declaration ----
        let key = span;
        let canonical = self
            .type_info
            .resolved_identity(span)
            .or_else(|| self.type_info.traits.self_method_identities.get(&key))
            .cloned();
        // ---- Ordinary and method-scoped argument frames ----
        let params = self
            .type_info
            .call_site_callable_params(span)
            .or_else(|| self.type_info.traits.self_method_params.get(&key).map(Vec::as_slice))
            .map(<[_]>::to_vec);
        let type_args = self
            .type_info
            .calls
            .body_ir_call_type_arguments
            .get(&key)
            .or_else(|| self.type_info.calls.call_site_monomorph_type_args.get(&key))
            .cloned()
            .unwrap_or_default();
        // ---- Exact declared receiver mode ----
        let receiver_is_mutable = match self.type_info.resolved_method_call(span).map(|call| &call.dispatch) {
            Some(ResolvedMethodDispatch::Trait {
                receiver_is_mutable, ..
            }) => Some(*receiver_is_mutable),
            None => self.body_ir_protocol_receiver(receiver, method),
        };
        BodyIrProtocolCallInfo {
            canonical,
            dispatch: self
                .type_info
                .resolved_method_call(span)
                .map(|call| call.dispatch.clone()),
            params,
            type_args,
            return_type: returned.clone(),
            receiver_is_mutable,
        }
    }

    /// Retain a local hook's mode from the same declared-method metadata, including aliases and overloads.
    pub(super) fn body_ir_protocol_receiver(&self, receiver: &ResolvedType, method: &str) -> Option<bool> {
        let name = match receiver {
            ResolvedType::Named(name) | ResolvedType::Generic(name, _) => name,
            _ => return None,
        };
        let (receivers, _) = self.declared_method_receivers_and_adoptions(name, method)?;
        let first = receivers.first().copied().flatten()?;
        receivers
            .iter()
            .all(|receiver| *receiver == Some(first))
            .then_some(first == Receiver::Mutable)
    }

    /// Resolve the `__iter__` and `__next__` hooks of [`Self::resolve_iteration_protocol_hooks`] at `protocol_span`,
    /// with `receiver_dispatch` the dispatch the iterable's own call selected.
    pub(super) fn resolve_iteration_protocol_hook_calls(
        &mut self,
        receiver_ty: &ResolvedType,
        receiver_dispatch: Option<ResolvedMethodDispatch>,
        protocol_span: Span,
    ) -> Option<ResolvedIterationHooks> {
        let args: Vec<CallArg> = Vec::new();
        let arg_types: Vec<ResolvedType> = Vec::new();
        let iter_method = magic_methods::as_str(MagicMethodId::Iter);
        let next_method = magic_methods::as_str(MagicMethodId::Next);
        let iterator_ty = match self.resolve_protocol_operator_dunder(
            receiver_ty,
            iter_method,
            &args,
            &arg_types,
            protocol_span,
            receiver_dispatch.as_ref(),
        ) {
            Some(iterator_ty) => iterator_ty,
            None => {
                self.errors.push(errors::missing_method(
                    &receiver_ty.to_string(),
                    iter_method,
                    protocol_span,
                ));
                return None;
            }
        };
        let mut iter_dispatch = self
            .type_info
            .resolved_method_call(protocol_span)
            .filter(|call| call.method == iter_method)
            .map(|call| call.dispatch.clone());
        inherit_same_trait_dispatch_module(receiver_dispatch.as_ref(), &mut iter_dispatch);
        let iter = self.body_ir_protocol_call(receiver_ty, iter_method, &iterator_ty, protocol_span);
        self.type_info.clear_body_ir_call_site_facts(protocol_span);
        let next_ret = match self.resolve_protocol_operator_dunder(
            &iterator_ty,
            next_method,
            &args,
            &arg_types,
            protocol_span,
            iter_dispatch.as_ref(),
        ) {
            Some(next_ret) => next_ret,
            None => {
                self.errors.push(errors::missing_method(
                    &iterator_ty.to_string(),
                    next_method,
                    protocol_span,
                ));
                return None;
            }
        };
        let mut next_dispatch = self
            .type_info
            .resolved_method_call(protocol_span)
            .filter(|call| call.method == next_method)
            .map(|call| call.dispatch.clone());
        inherit_same_trait_dispatch_module(iter_dispatch.as_ref(), &mut next_dispatch);
        let next = self.body_ir_protocol_call(&iterator_ty, next_method, &next_ret, protocol_span);
        Some(ResolvedIterationHooks {
            iterator_type: iterator_ty,
            next_type: next_ret,
            iter_dispatch,
            next_dispatch,
            body_ir: BodyIrIterationHooksInfo { iter: Some(iter), next },
        })
    }
}
