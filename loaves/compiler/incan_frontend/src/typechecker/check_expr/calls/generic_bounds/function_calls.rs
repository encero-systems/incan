//! Generic function call validation, type binding seeds, and checked function call completion.

use super::*;

impl TypeChecker {
    /// Check one method call after overload/trait resolution proved the selected declaration.
    ///
    /// Diagnostic fallback paths must call [`Self::check_generic_method_call`] directly: they may borrow a candidate
    /// merely to explain why no overload is viable and therefore must not publish a false reference identity.
    #[allow(clippy::too_many_arguments)]
    pub(in crate::typechecker::check_expr) fn check_resolved_generic_method_call(
        &mut self,
        method: &str,
        method_info: MethodInfo,
        explicit_type_args: &[Spanned<Type>],
        args: &[CallArg],
        arg_types: &[ResolvedType],
        call_site_span: Span,
        receiver_ty: &ResolvedType,
        expected_return_ty: Option<&ResolvedType>,
    ) -> ResolvedType {
        let identity = method_info.identity.clone();
        let receiver = method_info.receiver;
        let declared_params = method_info.params.clone();
        let returns_receiver = method_info.receiver.is_some() && method_info.return_type == ResolvedType::SelfType;
        if let Some(identity) = identity.clone() {
            self.type_info.record_resolved_identity(call_site_span, identity);
        }
        let first_error = self.errors.len();
        self.refuse_inaccessible_method(method, &method_info, receiver_ty, call_site_span);
        let result = self.check_generic_method_call(
            method,
            method_info,
            explicit_type_args,
            args,
            arg_types,
            call_site_span,
            receiver_ty,
            expected_return_ty,
        );
        let site = call_site_span;
        if self.errors.len() == first_error {
            self.record_selected_method_input_call(identity.as_ref(), receiver, &declared_params, call_site_span);
        }
        self.type_info.calls.body_ir_self_result_methods.remove(&site);
        if returns_receiver
            && self.errors.len() == first_error
            && let Some(identity) = &identity
        {
            self.type_info
                .calls
                .body_ir_self_result_methods
                .insert(site, identity.clone());
        }
        if let Some(identity) = identity {
            self.attach_related_declaration_to_new_errors(first_error, &identity);
        }
        result
    }

    /// Attach one already-resolved declaration to errors produced while validating its call surface.
    pub(in crate::typechecker::check_expr::calls) fn attach_related_declaration_to_new_errors(
        &mut self,
        first_error: usize,
        identity: &CanonicalSymbolId,
    ) {
        for error in &mut self.errors[first_error..] {
            if error
                .related_declarations()
                .iter()
                .any(|related| related.identity == *identity)
            {
                continue;
            }
            *error = error.clone().with_related_declaration(
                identity.clone(),
                format!("declaration of `{}`", identity.declaration_name),
            );
        }
    }

    /// Validate generic function call type arguments, contextual return bindings, value arguments, and explicit
    /// type-parameter bounds.
    ///
    /// Bounds and hashed type parameters (#1758) are checked against what the call instantiates the callee with: the
    /// inferred bindings, with any binding a literal argument leaves open closed from the literal's elements, and the
    /// side an `Ok(...)` or `Err(...)` argument leaves open settled. The arguments that bind one type parameter bind it
    /// to one type, and a bounded type parameter no argument fixes is refused (#1561).
    /// Successful calls share the fully checked native result with enclosing expressions and Body IR, preserving
    /// associated outputs inferred from future and callable bounds.
    pub(in crate::typechecker::check_expr::calls) fn validate_function_call(
        &mut self,
        func_name: &str,
        info: &FunctionInfo,
        explicit_type_args: &[Spanned<Type>],
        args: &[CallArg],
        call_span: Span,
        expected_return_ty: Option<&ResolvedType>,
    ) -> ResolvedType {
        let callee_identity = self.called_function_identity(func_name);
        self.validate_function_call_with_identity(
            func_name,
            info,
            explicit_type_args,
            args,
            call_span,
            expected_return_ty,
            callee_identity,
        )
    }

    /// Validate against one exact callable identity, including a tentative overload candidate. Explicit ownership
    /// changes the call's value-type view without changing the declaration's published mode contract.
    #[allow(clippy::too_many_arguments)] // Call syntax, expected type and selected declaration are independent inputs.
    pub(in crate::typechecker::check_expr) fn validate_function_call_with_identity(
        &mut self,
        func_name: &str,
        info: &FunctionInfo,
        explicit_type_args: &[Spanned<Type>],
        args: &[CallArg],
        call_span: Span,
        expected_return_ty: Option<&ResolvedType>,
        callee_identity: Option<CanonicalSymbolId>,
    ) -> ResolvedType {
        let projected = match (
            self.ownership_call_value_parameters(callee_identity.as_ref(), &info.params, None, call_span),
            self.ownership_call_value_result(callee_identity.as_ref(), &info.return_type, call_span),
        ) {
            (Ok(None), Ok(None)) => None,
            (Ok(params), Ok(return_type)) => Some(FunctionInfo {
                params: params.unwrap_or_else(|| info.params.clone()),
                return_type: return_type.unwrap_or_else(|| info.return_type.clone()),
                ..info.clone()
            }),
            (Err(error), _) | (_, Err(error)) => {
                self.errors.push(error);
                return ResolvedType::Unknown;
            }
        };
        let info = projected.as_ref().unwrap_or(info);
        let errors_before_call = self.errors.len();
        let seeded_type_bindings =
            self.seed_function_type_bindings(func_name, info, explicit_type_args, expected_return_ty, call_span);
        let params_with_explicit = Self::substitute_callable_params(&info.params, &seeded_type_bindings);
        let contextual_params = self.contextualize_source_callable_params(
            &params_with_explicit,
            args,
            &info.type_param_bound_details,
            &seeded_type_bindings,
        );
        let arg_types =
            self.check_call_arg_types_binding_closures_last(args, &contextual_params, &seeded_type_bindings);
        let mut type_bindings = seeded_type_bindings;
        self.seed_contextual_callable_type_param_bindings(&params_with_explicit, args, &arg_types, &mut type_bindings);
        self.validate_callable_arg_bindings(
            func_name,
            &contextual_params,
            args,
            &arg_types,
            &mut type_bindings,
            call_span,
        );
        self.refuse_non_task_arguments(
            func_name,
            &contextual_params,
            args,
            &arg_types,
            &info.type_param_bound_details,
            &mut type_bindings,
        );
        self.infer_type_param_bindings_from_source_bounds(&info.type_param_bound_details, &mut type_bindings);
        let arguments = Self::arguments_with_parameter_types(&params_with_explicit, args);
        self.unify_argument_type_bindings(&info.type_params, &arguments, &mut type_bindings);
        let mut instantiation =
            self.bindings_closed_by_literal_arguments(&info.type_params, &params_with_explicit, args, &type_bindings);
        if self.errors.len() == errors_before_call {
            self.close_bindings_from_literal_arguments(
                func_name,
                &info.type_params,
                &params_with_explicit,
                args,
                &instantiation,
                &mut type_bindings,
            );
        }
        self.settle_constructor_argument_sides(&info.type_params, &params_with_explicit, args, &mut type_bindings);
        Self::close_instantiation_from_bindings(&mut instantiation, &type_bindings, &info.type_params);
        if self.errors.len() == errors_before_call {
            self.settle_native_call_arguments_at_instantiation(
                &info.type_params,
                &params_with_explicit,
                args,
                &type_bindings,
            );
        }
        if self.errors.len() == errors_before_call {
            self.refuse_conflicting_type_arguments(func_name, &info.type_params, &arguments, &type_bindings);
        }
        if self.errors.len() == errors_before_call {
            self.record_argument_destinations_at_bindings(&info.type_params, &arguments, &type_bindings);
        }
        if self.errors.len() == errors_before_call && explicit_type_args.is_empty() {
            self.refuse_bounded_type_params_left_open(
                func_name,
                &info.type_params,
                &info.type_param_bound_details,
                &arguments,
                &instantiation,
                call_span,
            );
        }
        self.refuse_unhashable_type_arguments(
            func_name,
            callee_identity.as_ref(),
            &info.type_params,
            &instantiation,
            call_span,
        );
        self.finish_checked_function_call(FunctionCallCompletion {
            func_name,
            info,
            explicit_type_args,
            args,
            call_span,
            errors_before_call,
            callee_identity,
            contextual_params,
            type_bindings,
            instantiation,
            arguments,
        })
    }

    /// Seed a selected function's binders from written type arguments and the contextual result, before argument
    /// checking may refine them. Arity errors retain their original call anchor and diagnostic ordering.
    pub(super) fn seed_function_type_bindings(
        &mut self,
        func_name: &str,
        info: &FunctionInfo,
        explicit_type_args: &[Spanned<Type>],
        expected_return_ty: Option<&ResolvedType>,
        span: Span,
    ) -> HashMap<String, ResolvedType> {
        let mut bindings = HashMap::new();
        if !explicit_type_args.is_empty() {
            if explicit_type_args.len() != info.type_params.len() {
                self.errors.push(errors::explicit_type_arg_arity(
                    func_name,
                    &info.type_params,
                    &Self::written_type_args(explicit_type_args),
                    Self::explicit_type_argument_span(explicit_type_args, span),
                ));
            } else {
                let resolved: Vec<_> = explicit_type_args
                    .iter()
                    .map(|ty| self.resolve_type_checked(ty))
                    .collect();
                bindings = type_param_subst_map_call_site(&info.type_params, &resolved);
            }
        }
        if let Some(expected) = expected_return_ty {
            self.infer_type_param_bindings(&info.return_type, expected, &mut bindings);
        }
        bindings
    }

    /// Publish a checked function call's parameter, bound, constructor and result facts in their established order.
    /// Conditional facts use the validator's original diagnostic checkpoint, including explicit arity refusals.
    pub(super) fn finish_checked_function_call(&mut self, completion: FunctionCallCompletion<'_>) -> ResolvedType {
        let FunctionCallCompletion {
            func_name,
            info,
            explicit_type_args,
            args,
            call_span,
            errors_before_call,
            callee_identity,
            contextual_params,
            type_bindings,
            instantiation,
            arguments,
        } = completion;
        let resolved_params = Self::substitute_callable_params(&contextual_params, &type_bindings);
        if self.literal_argument_spells_a_type_param(&info.type_params, &info.params, args)
            && resolved_params
                .iter()
                .all(|param| first_open_type_param(&param.ty, &info.type_params).is_none())
        {
            // A collection or `None` literal, or an `Ok(...)`/`Err(...)` constructor, is written with its parameter's
            // type, and the declared `list[T]` or `Result[T, E]` would spell the callee's own `T` at the call site, so
            // the call carries its instantiated parameter types (#1862, #1561).
            self.type_info
                .record_call_site_callable_params_exact(call_span, &resolved_params);
        } else {
            self.type_info
                .record_call_site_callable_params(call_span, &resolved_params);
        }
        self.emit_explicit_bound_errors(
            func_name,
            &info.type_param_bounds,
            &info.type_param_bound_details,
            &instantiation,
            Self::type_argument_origin(explicit_type_args),
            call_span,
        );
        if info.is_async {
            self.warn_if_unawaited_async_call(func_name, call_span);
        }

        let explicit_arity_ok = explicit_type_args.is_empty() || explicit_type_args.len() == info.type_params.len();
        if !explicit_type_args.is_empty() && explicit_arity_ok {
            self.assert_call_site_type_params_inferred(func_name, &info.type_params, &type_bindings, call_span);
        }
        if explicit_arity_ok {
            self.record_call_site_monomorph_if_complete(call_span, &info.type_params, &type_bindings);
            if explicit_type_args.is_empty() {
                self.retain_constructor_call_instantiation(
                    call_span,
                    &info.type_params,
                    &arguments,
                    &type_bindings,
                    &info.return_type,
                );
                self.retain_body_ir_constructor_call_instantiation(
                    call_span,
                    &info.type_params,
                    &arguments,
                    &type_bindings,
                    &info.return_type,
                    &info.type_param_bound_details,
                );
            }
        }

        // Written type arguments seed the same bindings inference closes, and their call publishes the same closed
        // executable result: a native template such as `FrozenList<T>` is substituted only here (#2447).
        if explicit_arity_ok && self.errors.len() == errors_before_call {
            self.retain_body_ir_call_arguments(
                info,
                args,
                &type_bindings,
                call_span,
                callee_identity.as_ref(),
                !explicit_type_args.is_empty(),
            );
        }
        if explicit_arity_ok && self.errors.len() == errors_before_call {
            self.retain_body_ir_parameter_requirements(&resolved_params, args, callee_identity.as_ref());
            let templates =
                self.contextual_parameter_templates(&resolved_params, callee_identity.as_ref(), &info.params);
            for ((expression, expected), (_, template)) in Self::arguments_with_parameter_types(&resolved_params, args)
                .into_iter()
                .zip(Self::arguments_with_parameter_types(&templates, args))
            {
                self.retain_contextual_literal_type(expression, expected, callee_identity.as_ref(), Some(template));
            }
            self.retain_declared_nominal_result(call_span, &info.return_type, &type_bindings, callee_identity.as_ref());
            self.record_explicit_input_call(callee_identity.as_ref(), &resolved_params, None, call_span);
        }
        // Expression checking reads a closed native frozen result through its source frozen view.
        self.type_info
            .calls
            .body_ir_call_result_types
            .get(&call_span)
            .or_else(|| self.type_info.calls.inferred_call_result_types.get(&call_span))
            .map(crate::typechecker::native_storage::source_frozen_view)
            .unwrap_or_else(|| substitute_resolved_type(&info.return_type, &type_bindings))
    }
}
